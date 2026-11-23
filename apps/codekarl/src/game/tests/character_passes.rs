//! The character passes of CK-362 through the game's real scheduler: they run inside the economy's daily system, after that
//! day's economy work, never on a day the economy step fails, and in the stated order (monthly recovery, then birthdays).

use std::{collections::BTreeMap, sync::Arc};

use ck_characters::{Character, CharacterStore, GeneticRecord, PersonalityRecord, PopLocator, Sex};
use ck_economy::{CounterBuckets, EconomyState, PlaceState, Rules};
use ck_sim::{Canonical, Date, GameTime, Scheduler, Simulation};

use super::date;
use crate::game::{
    GameCommand, GameState, apply,
    core::ECONOMY_HOUR,
    setup::{register_economy_callback, run_character_passes},
};

fn shipped_rules() -> Rules {
    Rules::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
    )
    .expect("the shipped economy rules load")
}

fn at(year: u16, month: u8, day: u8, hour: u8) -> GameTime {
    GameTime::from_date(date(year, month, day, hour)).unwrap()
}

/// A live character with the given humours (blood, yellow bile, black bile, phlegm) and personality.
fn person(id: u64, born: GameTime, humours: [i64; 4], mind: PersonalityRecord) -> Character {
    let mut genetics = GeneticRecord::default();
    genetics.value[9..13].copy_from_slice(&humours);
    Character {
        id,
        name: format!("c{id}"),
        sex: Sex::Male,
        birth: born,
        death: None,
        pop: PopLocator {
            place: "fixture".to_owned(),
            slot: 0,
        },
        father: ck_characters::UNKNOWN,
        mother: ck_characters::UNKNOWN,
        spouses: Vec::new(),
        children: Vec::new(),
        office: None,
        full_until: None,
        genetics: Box::new(genetics),
        personality: mind,
        life: Box::default(),
        ties: Box::default(),
    }
}

fn store_of(characters: Vec<Character>) -> CharacterStore {
    let next = characters.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    let live: BTreeMap<u64, Character> = characters.into_iter().map(|c| (c.id, c)).collect();
    let mut bytes = Vec::new();
    next.encode(&mut bytes).unwrap();
    live.encode(&mut bytes).unwrap();
    0_u64.encode(&mut bytes).unwrap();
    BTreeMap::<String, String>::new()
        .encode(&mut bytes)
        .unwrap();
    CharacterStore::from_bytes(&bytes).unwrap()
}

/// An economy of no places, which runs without error, or of one place whose dimensions do not match the rules, which fails
/// every daily step: the injected fault.
fn economy(broken: bool) -> EconomyState {
    EconomyState {
        places: if broken {
            vec![PlaceState {
                id: "fixture".to_owned(),
                pops: Vec::new(),
                buildings: Vec::new(),
                prices_milli: Vec::new(),
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn mind(stress: i64, honour: i64) -> PersonalityRecord {
    PersonalityRecord {
        stress,
        honour,
        ..PersonalityRecord::default()
    }
}

/// What the passes changed, tick by tick, and the systems that failed.
struct Run {
    changes: Vec<(Date, u64, PersonalityRecord)>,
    failures: Vec<(Date, String)>,
    final_state: GameState,
}

/// Run `rules` over `characters` from `from` up to (not including) `until`, one tick at a time.
fn run(
    rules: Rules,
    broken: bool,
    characters: Vec<Character>,
    from: GameTime,
    until: GameTime,
) -> Run {
    let mut scheduler = Scheduler::<GameState>::new();
    register_economy_callback(&mut scheduler, Arc::new(rules));
    let mut simulation = Simulation::<GameState, GameCommand>::new(
        from,
        42,
        GameState {
            economy: economy(broken),
            characters: store_of(characters),
            stretches: BTreeMap::new(),
        },
        scheduler,
        apply,
    );
    let snapshot = |simulation: &Simulation<GameState, GameCommand>| {
        simulation
            .state()
            .characters
            .iter()
            .map(|c| (c.id, c.personality))
            .collect::<Vec<_>>()
    };
    let mut changes = Vec::new();
    let mut failures = Vec::new();
    let mut before = snapshot(&simulation);
    while simulation.date().unwrap() < date_of(until) {
        let tick = simulation.date().unwrap();
        for error in simulation.step().unwrap() {
            assert_eq!(error.system, "economy.daily");
            failures.push((error.date, error.error.to_string()));
        }
        let after = snapshot(&simulation);
        for ((id, was), (_, now)) in before.iter().zip(&after) {
            if was != now {
                changes.push((tick, *id, *now));
            }
        }
        before = after;
    }
    Run {
        changes,
        failures,
        final_state: simulation.state().clone(),
    }
}

fn date_of(time: GameTime) -> Date {
    time.date().unwrap()
}

fn worlds() -> Vec<Character> {
    let balanced = [5000; 4];
    vec![
        // a birthday on the 1st (born on the 1st, eight years before): recovery and the birthday pass both run.
        person(
            1,
            at(760, 2, 1, 0),
            [8000, 5000, 5000, 5000],
            mind(50_000, 1999),
        ),
        // born on the day itself: no birthday pass that day, but the monthly recovery.
        person(2, at(768, 2, 1, 0), balanced, mind(50_000, 1999)),
        // a birthday on the 2nd: no monthly recovery that day.
        person(3, at(760, 2, 2, 0), balanced, mind(50_000, 1000)),
    ]
}

#[test]
fn recovery_runs_once_on_the_first_and_birthdays_once_on_the_birthday_inside_the_economy_system() {
    let result = run(
        shipped_rules(),
        false,
        worlds(),
        at(768, 1, 30, 0),
        at(768, 2, 4, 0),
    );
    assert!(result.failures.is_empty(), "{:?}", result.failures);
    let first = date(768, 2, 1, ECONOMY_HOUR);
    let second = date(768, 2, 2, ECONOMY_HOUR);
    let found: Vec<(Date, u64)> = result.changes.iter().map(|(d, id, _)| (*d, *id)).collect();
    // The whole run changes exactly four records: the 1st (every character), and the 2nd (the one whose birthday it is);
    // nothing on 30 or 31 January, on any other hour, or on the 3rd.
    assert_eq!(found, [(first, 1), (first, 2), (first, 3), (second, 3)]);
    let state = |id| {
        result
            .changes
            .iter()
            .find(|(d, i, _)| *d == first && *i == id)
            .unwrap()
            .2
    };
    // balanced humours: 50 x 0.056 = 2.8 shed, once; character 1's blood 8 raises its recovery by 30%: 3.64.
    assert_eq!(state(1).stress, 46_360);
    assert_eq!(state(2).stress, 47_200);
    assert_eq!(state(3).stress, 47_200);
    // character 1's birthday pass ran the same tick: honour 1.999 x 0.95, blood 8 drifting lust by 0.03 toward the sin.
    assert_eq!(state(1).honour, 1899);
    assert_eq!(state(1).vice[2], -30);
    // character 2 was born that day and has no pass; character 3's birthday is the next day.
    assert_eq!(state(2).honour, 1999);
    assert_eq!(state(3).honour, 1000);
    let on_the_second = result
        .changes
        .iter()
        .find(|(d, _, _)| *d == second)
        .unwrap()
        .2;
    assert_eq!((on_the_second.honour, on_the_second.stress), (950, 47_200));
}

#[test]
fn neither_pass_runs_on_a_day_the_economy_step_fails() {
    let result = run(
        shipped_rules(),
        true,
        worlds(),
        at(768, 1, 30, 0),
        at(768, 2, 4, 0),
    );
    assert!(
        !result.failures.is_empty(),
        "the injected fault fails the economy"
    );
    assert!(
        result
            .failures
            .iter()
            .any(|(d, _)| *d == date(768, 2, 1, ECONOMY_HOUR)),
        "{:?}",
        result.failures
    );
    assert!(
        result.changes.is_empty(),
        "no pass ran: {:?}",
        result.changes
    );
    let untouched = store_of(worlds());
    assert_eq!(
        result.final_state.characters.to_bytes().unwrap(),
        untouched.to_bytes().unwrap()
    );
}

#[test]
fn the_monthly_recovery_has_already_run_when_the_birthday_pass_fails() {
    // A record whose birth lies outside the calendar makes the birthday pass fail (and change nothing). The recovery runs
    // first, so the other characters have recovered when it fails: were the order reversed, the failed birthday pass would
    // have stopped the recovery too.
    let mut characters = worlds();
    characters.push(person(
        4,
        GameTime::from_hours(u64::MAX / 2),
        [5000; 4],
        mind(0, 0),
    ));
    let result = run(
        shipped_rules(),
        false,
        characters,
        at(768, 2, 1, 0),
        at(768, 2, 2, 0),
    );
    assert_eq!(result.failures.len(), 1, "{:?}", result.failures);
    assert_eq!(result.failures[0].0, date(768, 2, 1, ECONOMY_HOUR));
    let recovered: Vec<(u64, i64, i64)> = result
        .changes
        .iter()
        .map(|(_, id, mind)| (*id, mind.stress, mind.honour))
        .collect();
    // Character 1 has its birthday that day; its honour is untouched because the birthday pass failed as a whole.
    assert_eq!(
        recovered,
        [(1, 46_360, 1999), (2, 47_200, 1999), (3, 47_200, 1000)]
    );
}

/// An economy of one place with one noble Irish pop (not run through the economy: only read by the character passes).
fn economy_with_a_noble_pop() -> EconomyState {
    let mut state = economy(true);
    state.places[0].pops.push(ck_economy::PopState {
        kind: "noble".to_owned(),
        culture: "ulaid".to_owned(),
        religion: String::new(),
        literacy_pm: 0,
        workers: 100,
        dependents: 0,
        employment: Vec::new(),
        stock: Vec::new(),
        consumption: ck_economy::PopConsumption::default(),
        counters: CounterBuckets::default(),
        short_on_rent: false,
        housed_last: false,
        genetics: ck_economy::PopGenetics::default(),
        pending_births: 0,
        pending_deaths: 0,
    });
    state
}

#[test]
fn the_daily_passes_run_the_event_engine_with_the_character_storage_on_the_first() {
    // CK-363: with a storage handle the pass is the engine's. A hospitaller in a pop with no luxuries loses it on the 1st, and the
    // line goes to the life log in the storage directory; on another day nothing is checked.
    let directory = std::env::temp_dir().join(format!("ck-passes-storage-{}", std::process::id()));
    std::fs::remove_dir_all(&directory).ok();
    std::fs::create_dir_all(&directory).unwrap();
    let handle: crate::game::character_storage::Handle =
        Arc::new(std::sync::Mutex::new(Some(directory.clone())));
    let rules = shipped_rules();
    let mut monk = person(1, at(740, 6, 15, 0), [5000; 4], mind(0, 0));
    monk.life.grades.insert("hospitaller".to_owned(), 1);
    let mut state = GameState {
        economy: economy_with_a_noble_pop(),
        characters: store_of(vec![monk]),
        stretches: BTreeMap::new(),
    };
    run_character_passes(
        &rules,
        &mut state,
        at(768, 5, 2, 12),
        9,
        &BTreeMap::new(),
        &BTreeMap::new(),
        Some(&handle),
    )
    .unwrap();
    assert_eq!(
        state.characters.get(1).unwrap().grade("hospitaller"),
        1,
        "not checked off the 1st"
    );
    run_character_passes(
        &rules,
        &mut state,
        at(768, 5, 1, 12),
        9,
        &BTreeMap::new(),
        &BTreeMap::new(),
        Some(&handle),
    )
    .unwrap();
    assert_eq!(state.characters.get(1).unwrap().grade("hospitaller"), 0);
    let lines = ck_characters::LifeLog::open(directory.join("LOGS"))
        .unwrap()
        .read(1)
        .unwrap();
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        ["c1 lost hospitaller."]
    );
    // A pop the engine cannot resolve is an error of the daily system, as an economy error is.
    state.economy.places[0].pops.clear();
    state.characters = store_of(vec![{
        let mut monk = person(1, at(740, 6, 15, 0), [5000; 4], mind(0, 0));
        monk.life.grades.insert("hospitaller".to_owned(), 1);
        monk
    }]);
    let error = run_character_passes(
        &rules,
        &mut state,
        at(768, 6, 1, 12),
        9,
        &BTreeMap::new(),
        &BTreeMap::new(),
        Some(&handle),
    );
    assert!(error.is_err());
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn an_unavailable_log_does_not_stop_the_state_updates_and_is_reported() {
    // CK-363 review: with the LOGS path unusable (a directory where the file should be), the birthday and the monthly precedence
    // still happen; the failure to write the line is the error the daily system returns.
    let directory = std::env::temp_dir().join(format!("ck-passes-nologs-{}", std::process::id()));
    std::fs::remove_dir_all(&directory).ok();
    std::fs::create_dir_all(directory.join("LOGS")).unwrap();
    let handle: crate::game::character_storage::Handle =
        Arc::new(std::sync::Mutex::new(Some(directory.clone())));
    let rules = shipped_rules();
    // Born on 1 May: today is the birthday and the 1st, with honour 1000 to fade and a hospitaller to lose.
    let mut monk = person(1, at(740, 5, 1, 0), [5000; 4], mind(0, 1000));
    monk.life.grades.insert("hospitaller".to_owned(), 1);
    let mut state = GameState {
        economy: economy_with_a_noble_pop(),
        characters: store_of(vec![monk]),
        stretches: BTreeMap::new(),
    };
    let result = run_character_passes(
        &rules,
        &mut state,
        at(768, 5, 1, 12),
        9,
        &BTreeMap::new(),
        &BTreeMap::new(),
        Some(&handle),
    );
    assert!(result.is_err(), "the unwritable line is reported");
    let monk = state.characters.get(1).unwrap();
    assert_eq!(monk.personality.honour, 950, "the birthday's fade ran");
    assert_eq!(monk.grade("hospitaller"), 0, "the monthly precedence ran");
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_place_passion_written_by_the_production_pass_names_the_place_by_its_authored_name() {
    // CK-368 §8: the log line a passion's gain writes shows the scenario's display name for a place, not the key the state keeps.
    let directory = std::env::temp_dir().join(format!("ck-passes-names-{}", std::process::id()));
    std::fs::remove_dir_all(&directory).ok();
    std::fs::create_dir_all(&directory).unwrap();
    let handle: crate::game::character_storage::Handle =
        Arc::new(std::sync::Mutex::new(Some(directory.clone())));
    let mut rules = shipped_rules();
    for moment in &mut rules.lifecycle.connections.passions.moments {
        if moment.key == "long_rule" {
            moment.chance_ppb = 1_000_000_000;
        }
    }
    // A ruler of long standing at the fixture place, on its birthday.
    let mut chief = person(1, at(740, 6, 15, 0), [5000; 4], mind(0, 0));
    chief.office = Some(ck_characters::Office::Ruler);
    chief.full_until = Some(at(900, 1, 1, 0));
    let mut state = GameState {
        economy: economy_with_a_noble_pop(),
        characters: store_of(vec![chief]),
        stretches: BTreeMap::new(),
    };
    let names: BTreeMap<String, String> = [("fixture".to_owned(), "Ard Mhacha".to_owned())].into();
    run_character_passes(
        &rules,
        &mut state,
        at(768, 6, 15, 12),
        9,
        &BTreeMap::new(),
        &names,
        Some(&handle),
    )
    .unwrap();
    let passions = &state.characters.get(1).unwrap().ties.passions;
    assert_eq!(passions.len(), 1);
    // The state keeps the key.
    assert_eq!(passions[0].target.key, "fixture");
    let lines: Vec<String> = ck_characters::LifeLog::open(directory.join("LOGS"))
        .unwrap()
        .read(1)
        .unwrap()
        .into_iter()
        .map(|line| line.text)
        .collect();
    assert!(
        lines.contains(&"c1 came to love Ard Mhacha.".to_owned()),
        "{lines:?}"
    );
}

#[test]
fn the_first_daily_phase_backfills_a_promotion_before_the_deaths_and_the_economy() {
    // CK-368 §9 (review 11): the pending backfill of a promoted character runs at the start of phase A, before the day's deaths can
    // archive it; it is committed (window cleared, last full-tier year advanced) by that phase alone.
    let directory = std::env::temp_dir().join(format!("ck-passes-backfill-{}", std::process::id()));
    std::fs::remove_dir_all(&directory).ok();
    std::fs::create_dir_all(&directory).unwrap();
    let handle: crate::game::character_storage::Handle =
        Arc::new(std::sync::Mutex::new(Some(directory.clone())));
    let rules = shipped_rules();
    let mut noble = person(1, at(700, 6, 15, 0), [5000; 4], mind(0, 0));
    noble.ties.promoted = Some(765);
    noble.ties.last_full = 765;
    let mut state = GameState {
        economy: economy_with_a_noble_pop(),
        characters: store_of(vec![noble]),
        stretches: BTreeMap::new(),
    };
    super::super::setup::run_pre_economy_passes(
        &rules,
        &mut state,
        at(768, 5, 2, 12),
        9,
        &BTreeMap::new(),
        &BTreeMap::new(),
        Some(&handle),
    )
    .unwrap();
    let held = state.characters.get(1).expect("alive");
    assert_eq!(held.ties.promoted, None, "the promotion was taken up");
    assert_eq!((held.ties.backfill, held.ties.last_full), ((0, 0), 767));
    std::fs::remove_dir_all(&directory).ok();
}
