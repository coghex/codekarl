//! Character personality, stress, standing and opinion contracts (CK-362, characters slice 3).
//!
//! Characters whose exact state a case needs are built as records and loaded through the canonical encoding; the creation
//! path is tested through the store. Statistical tests use fixed seeds and integer arithmetic; each states its tolerance.

use ck_characters::{
    Archive, Character, CharacterStore, Context, Error, EventKind, GeneticRecord, LevelTrait,
    Lineage, NewCharacter, Office, PersonalityRecord, PopLocator, Populations, Sex, Standing,
    StressKind, UNKNOWN,
    personality::{child_value, threshold},
};
use ck_economy::{
    Personality, PopGenetics, Rules,
    personality::{MANNER_PAIRS, VICE_PAIRS},
};
use ck_sim::{Canonical, Date, GameTime};
use std::{collections::BTreeMap, path::PathBuf, sync::OnceLock};

fn date(year: u16, month: u8, day: u8) -> GameTime {
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour: 0,
    })
    .expect("valid date")
}
fn locator(place: &str) -> PopLocator {
    PopLocator {
        place: place.into(),
        slot: 0,
    }
}
fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        Rules::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
        )
        .expect("shipped rules")
    })
}
fn shipped() -> &'static Personality {
    &rules().personality
}
fn irish() -> PopGenetics {
    rules().genetics.seed_pop("ulaid").expect("Irish start set")
}

/// The pops of the tests: place `a` is Irish (`ulaid`), `b` is Pictish (`cruthin`), `n` has a culture the data does not
/// hold, and `x` has none. Every pop has room for a million.
struct Pops;
impl Populations for Pops {
    fn heads(&self, _: &PopLocator) -> Option<u64> {
        Some(1_000_000)
    }
    fn culture(&self, locator: &PopLocator) -> Option<String> {
        match locator.place.as_str() {
            "a" => Some("ulaid".to_owned()),
            "b" => Some("cruthin".to_owned()),
            "n" => Some("narnia".to_owned()),
            _ => None,
        }
    }
}

/// The shipped personality data with `edit` applied: a fixture for the draws.
fn fixture(edit: impl FnOnce(&mut Personality)) -> Personality {
    let mut data = shipped().clone();
    edit(&mut data);
    data
}
fn context(personality: &Personality, seed: u64) -> Context<'_> {
    Context {
        rules: &rules().genetics,
        personality,
        seed,
        tick: date(768, 1, 1),
    }
}
fn new(sex: Sex, place: &str) -> NewCharacter {
    NewCharacter {
        name: "x".into(),
        sex,
        birth: date(768, 1, 1),
        pop: locator(place),
    }
}
fn archive(tag: &str) -> Archive {
    let path: PathBuf =
        std::env::temp_dir().join(format!("ck-personality-{}-{tag}", std::process::id()));
    std::fs::remove_file(&path).ok();
    Archive::open(path).expect("archive")
}

/// A record with the given humours (blood, yellow bile, black bile, phlegm, in thousandths) and personality.
fn person(id: u64, birth: GameTime, humours: [i64; 4], mind: PersonalityRecord) -> Character {
    let mut genetics = GeneticRecord::default();
    genetics.value[9..13].copy_from_slice(&humours);
    Character {
        id,
        name: format!("c{id}"),
        sex: Sex::Male,
        birth,
        death: None,
        pop: locator("a"),
        father: UNKNOWN,
        mother: UNKNOWN,
        spouses: Vec::new(),
        children: Vec::new(),
        office: None,
        full_until: None,
        genetics: Box::new(genetics),
        personality: mind,
    }
}
const BALANCED: [i64; 4] = [5000; 4];
fn mind() -> PersonalityRecord {
    PersonalityRecord::default()
}
fn calm(id: u64, humours: [i64; 4]) -> Character {
    person(id, date(750, 6, 1), humours, mind())
}
/// A store holding exactly these records.
fn store_of(characters: Vec<Character>) -> CharacterStore {
    let next = characters.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    let live: BTreeMap<u64, Character> = characters.into_iter().map(|c| (c.id, c)).collect();
    let mut bytes = Vec::new();
    next.encode(&mut bytes).expect("encode");
    live.encode(&mut bytes).expect("encode");
    CharacterStore::from_bytes(&bytes).expect("a store of these records")
}
fn mind_of(store: &CharacterStore, id: u64) -> PersonalityRecord {
    store.get(id).expect("live").personality
}
fn bytes(store: &CharacterStore) -> Vec<u8> {
    store.to_bytes().expect("encode")
}

// --- Threshold, intake, recovery, performance, events -----------------------------------------------------------

#[test]
fn the_threshold_is_exact_floored_at_zero_and_has_no_cap() {
    let store = store_of(vec![
        calm(1, BALANCED),
        calm(2, [5000, 5000, 0, 10_000]), // phlegmatic
        calm(3, [5000, 5000, 10_000, 0]), // melancholic
        calm(4, [10_000, 0, 0, 10_000]),  // 50 + 80 + 30 = 160
    ]);
    assert_eq!(store.threshold(1), Ok(50_000));
    assert_eq!(store.threshold(2), Ok(130_000));
    assert_eq!(store.threshold(3), Ok(0));
    assert_eq!(store.threshold(4), Ok(160_000), "above 100: no upper cap");
    assert_eq!(threshold(store.get(2).unwrap()), 130_000);
    assert_eq!(store.threshold(9), Err(Error::MissingCharacter));
}

#[test]
fn monthly_recovery_matches_the_worked_case_and_never_goes_below_zero() {
    let rules = shipped();
    let mut worked = person(
        1,
        date(750, 1, 1),
        [8000, 5000, 7000, 5000],
        PersonalityRecord {
            stress: 50_000,
            melancholy: 2,
            ..mind()
        },
    );
    let balanced = PersonalityRecord {
        stress: 50_000,
        ..mind()
    };
    worked.pop = locator("a");
    let mut store = store_of(vec![
        worked,
        person(2, date(750, 1, 1), BALANCED, balanced),
        person(3, date(750, 1, 1), BALANCED, mind()),
        person(
            4,
            date(750, 1, 1),
            BALANCED,
            PersonalityRecord {
                stress: 10,
                ..mind()
            },
        ),
    ]);
    store.monthly_recovery(rules).expect("pass");
    assert_eq!(
        mind_of(&store, 1).stress,
        47_671,
        "deduction 2.3296 rounded down"
    );
    assert_eq!(mind_of(&store, 2).stress, 47_200, "50 x 0.056 = 2.8");
    assert_eq!(mind_of(&store, 3).stress, 0, "never below 0");
    assert_eq!(
        mind_of(&store, 4).stress,
        10,
        "a deduction under a thousandth rounds down to nothing"
    );
}

#[test]
fn recovery_with_huge_factors_is_held_to_its_bounds_and_a_zero_factor_still_gives_zero() {
    // Accepted factors of 9223372036854775.807: the uncapped product does not fit, but the result is bounded to 0..=100.
    let wild = fixture(|data| {
        data.stress.recovery_per_blood = i64::MAX;
        data.stress.recovery_per_black = i64::MAX;
        data.stress.recovery_per_melancholy_grade = i64::MAX;
    });
    let stressed = PersonalityRecord {
        stress: 100_000,
        melancholy: 6,
        ..mind()
    };
    let mut store = store_of(vec![person(1, date(750, 1, 1), [10_000; 4], stressed)]);
    store
        .monthly_recovery(&wild)
        .expect("a valid case never fails");
    assert_eq!(
        mind_of(&store, 1).stress,
        0,
        "stress 100 with every factor huge recovers to zero"
    );
    // a zero factor (a hugely negative weight, held at zero) makes the product zero whatever the others are.
    let zeroed = fixture(|data| {
        data.stress.recovery_per_blood = i64::MIN;
        data.stress.recovery_per_black = i64::MAX;
        data.stress.recovery_per_melancholy_grade = i64::MAX;
    });
    let mut store = store_of(vec![person(1, date(750, 1, 1), [10_000; 4], stressed)]);
    store.monthly_recovery(&zeroed).expect("pass");
    assert_eq!(
        mind_of(&store, 1).stress,
        100_000,
        "a zero factor recovers nothing"
    );
}

#[test]
fn intake_with_huge_factors_saturates_at_100_and_a_zero_factor_still_gives_zero() {
    let wild = fixture(|data| {
        data.stress.intake_per_phlegm = i64::MAX;
        data.stress.insult_per_yellow = i64::MAX;
        data.stress.intake_per_temperance = i64::MAX;
    });
    let temperate = PersonalityRecord {
        vice: [0, 0, 0, 0, 7000, 0, 0],
        ..mind()
    };
    let mut store = store_of(vec![person(
        1,
        date(750, 1, 1),
        [5000, 10_000, 5000, 10_000],
        temperate,
    )]);
    assert_eq!(
        store.add_stress(&wild, 1, 1000, StressKind::Insult),
        Ok(100_000)
    );
    let zeroed = fixture(|data| {
        data.stress.intake_per_phlegm = i64::MIN;
        data.stress.insult_per_yellow = i64::MAX;
        data.stress.intake_per_temperance = i64::MAX;
    });
    let mut store = store_of(vec![person(
        1,
        date(750, 1, 1),
        [5000, 10_000, 5000, 10_000],
        temperate,
    )]);
    assert_eq!(
        store.add_stress(&zeroed, 1, 1000, StressKind::Insult),
        Ok(0)
    );
    assert_eq!(
        store.add_stress(&zeroed, 1, i64::MAX, StressKind::Insult),
        Ok(0)
    );
}

#[test]
fn intake_scales_with_phlegm_temperance_and_for_insults_yellow_bile_and_never_passes_100() {
    let rules = shipped();
    let temperate = PersonalityRecord {
        vice: [0, 0, 0, 0, 4000, 0, 0],
        ..mind()
    };
    let mut store = store_of(vec![
        calm(1, BALANCED),
        calm(2, [5000, 5000, 5000, 9000]),
        person(3, date(750, 1, 1), [5000, 9000, 5000, 9000], temperate),
        calm(4, [5000, 5000, 5000, 9000]),
    ]);
    // ordinary: nothing changes at balance; phlegm 9 lessens it by 0.05 x 4.
    assert_eq!(
        store.add_stress(rules, 1, 10_000, StressKind::Other),
        Ok(10_000)
    );
    assert_eq!(
        store.add_stress(rules, 2, 10_000, StressKind::Other),
        Ok(8000)
    );
    // temperance 4 lessens it by 0.03 x 4: 10 x 0.8 x 0.88 = 7.04; an insult adds yellow bile 9: x 1.4 = 9.856.
    assert_eq!(
        store.add_stress(rules, 3, 10_000, StressKind::Other),
        Ok(7040)
    );
    store
        .add_stress(rules, 3, 10_000, StressKind::Insult)
        .expect("insult");
    assert_eq!(mind_of(&store, 3).stress, 7040 + 9856);
    // a defeat scales like an insult, and an amount rounds down to the thousandth (3 x 0.8 = 2.4).
    assert_eq!(store.add_stress(rules, 4, 3, StressKind::Defeat), Ok(2));
    // never above 100
    store
        .add_stress(rules, 1, 95_000, StressKind::Other)
        .expect("large");
    assert_eq!(mind_of(&store, 1).stress, 100_000);
    assert_eq!(
        store.add_stress(rules, 1, 5000, StressKind::Other),
        Ok(100_000)
    );
    assert_eq!(
        store.add_stress(rules, 1, -1, StressKind::Other),
        Err(Error::Amount)
    );
    assert_eq!(
        store.add_stress(rules, 99, 1, StressKind::Other),
        Err(Error::MissingCharacter)
    );
}

#[test]
fn performance_is_full_up_to_the_threshold_then_falls_to_a_floor() {
    let rules = shipped();
    let at = |stress: i64| PersonalityRecord { stress, ..mind() };
    let store = store_of(vec![
        person(1, date(750, 1, 1), BALANCED, at(50_000)), // at the threshold of 50
        person(2, date(750, 1, 1), BALANCED, at(49_000)),
        person(3, date(750, 1, 1), BALANCED, at(60_000)), // 10 over
        person(4, date(750, 1, 1), BALANCED, at(53_333)), // 3.333 over: 966.67 rounds down
        person(5, date(750, 1, 1), BALANCED, at(100_000)), // 50 over: 0.5 is the floor
        person(6, date(750, 1, 1), [5000, 5000, 10_000, 0], at(100_000)), // threshold 0: 100 over
    ]);
    let performance = |id| store.performance(rules, id).expect("performance");
    assert_eq!(performance(1), 1000);
    assert_eq!(performance(2), 1000);
    assert_eq!(performance(3), 900);
    assert_eq!(performance(4), 966);
    assert_eq!(performance(5), 500);
    assert_eq!(performance(6), 500, "never below half");
}

#[test]
fn the_event_multiplier_raises_causing_events_and_lowers_lifting_ones() {
    let rules = shipped();
    let at = |stress: i64| PersonalityRecord { stress, ..mind() };
    let melancholic = [5000, 5000, 10_000, 0]; // threshold 0
    let store = store_of(vec![
        person(1, date(750, 1, 1), melancholic, at(0)),
        person(2, date(750, 1, 1), melancholic, at(100_000)),
        person(3, date(750, 1, 1), BALANCED, at(30_000)), // under the threshold: no excess
    ]);
    let multiplier = |id, kind| store.event_multiplier(rules, id, kind).expect("multiplier");
    assert_eq!(multiplier(1, EventKind::Causing), 1000);
    assert_eq!(multiplier(1, EventKind::Lifting), 1000);
    assert_eq!(multiplier(2, EventKind::Causing), 6000);
    assert_eq!(multiplier(2, EventKind::Lifting), 166, "1/6 rounded down");
    assert_eq!(multiplier(3, EventKind::Causing), 1000);
    assert_eq!(multiplier(3, EventKind::Lifting), 1000);
}

#[test]
fn indulging_and_confessing_relieve_stress_and_move_the_pairs() {
    let rules = shipped();
    let mut store = store_of(vec![
        person(
            1,
            date(750, 1, 1),
            BALANCED,
            PersonalityRecord {
                stress: 5000,
                ..mind()
            },
        ),
        person(
            2,
            date(750, 1, 1),
            BALANCED,
            PersonalityRecord {
                stress: 40_000,
                vice: [-7000, 0, 0, 0, 0, 0, 0],
                ..mind()
            },
        ),
        person(
            3,
            date(750, 1, 1),
            BALANCED,
            PersonalityRecord {
                stress: 5000,
                vice: [3000, -2000, -2000, 0, 500, 1000, 7000],
                ..mind()
            },
        ),
        person(
            4,
            date(750, 1, 1),
            BALANCED,
            PersonalityRecord {
                stress: 30_000,
                vice: [7000; 7],
                ..mind()
            },
        ),
    ]);
    // stress 5 floors at 0; the pair moves one toward its sin.
    store.indulge(rules, 1, "greed_charity").expect("indulge");
    assert_eq!(mind_of(&store, 1).stress, 0);
    assert_eq!(mind_of(&store, 1).vice[1], -1000);
    // a pair at its bound stays there; stress still falls by 15.
    store.indulge(rules, 2, "pride_humility").expect("indulge");
    assert_eq!(mind_of(&store, 2).stress, 25_000);
    assert_eq!(mind_of(&store, 2).vice[0], -7000);
    // confess: the most sinful pair (lowest; the tie of greed and lust goes to data order) moves half toward its virtue.
    store.confess(rules, 3).expect("confess");
    assert_eq!(mind_of(&store, 3).stress, 0);
    assert_eq!(
        mind_of(&store, 3).vice,
        [3000, -1500, -2000, 0, 500, 1000, 7000]
    );
    store.confess(rules, 3).expect("confess");
    assert_eq!(
        mind_of(&store, 3).vice[1..3],
        [-1500, -1500],
        "lust, now the lowest, moves next"
    );
    // a pair at its upper bound stays there.
    store.confess(rules, 4).expect("confess");
    assert_eq!(mind_of(&store, 4).stress, 20_000);
    assert_eq!(mind_of(&store, 4).vice, [7000; 7]);
}

#[test]
fn indulging_a_manner_pair_or_an_unknown_name_is_refused_and_changes_nothing() {
    let rules = shipped();
    let mut store = store_of(vec![person(
        1,
        date(750, 1, 1),
        BALANCED,
        PersonalityRecord {
            stress: 40_000,
            ..mind()
        },
    )]);
    let before = bytes(&store);
    for name in MANNER_PAIRS.iter().copied().chain(["piety", ""]) {
        assert_eq!(
            store.indulge(rules, 1, name),
            Err(Error::NotAVicePair),
            "{name}"
        );
        assert_eq!(bytes(&store), before);
    }
    for name in VICE_PAIRS {
        store.indulge(rules, 1, name).expect("a vice pair");
    }
    assert_eq!(
        store.indulge(rules, 7, "pride_humility"),
        Err(Error::MissingCharacter)
    );
}

#[test]
fn trait_levels_are_set_through_the_api_up_to_sss() {
    let mut store = store_of(vec![calm(1, BALANCED)]);
    for (which, level) in [
        (LevelTrait::Drunkard, 6),
        (LevelTrait::RageFits, 3),
        (LevelTrait::Melancholy, 1),
    ] {
        store.set_trait_level(1, which, level).expect("level");
    }
    let record = mind_of(&store, 1);
    assert_eq!(
        (record.drunkard, record.rage_fits, record.melancholy),
        (6, 3, 1)
    );
    let before = bytes(&store);
    assert_eq!(
        store.set_trait_level(1, LevelTrait::Drunkard, 7),
        Err(Error::Level)
    );
    assert_eq!(
        store.set_trait_level(2, LevelTrait::Drunkard, 1),
        Err(Error::MissingCharacter)
    );
    assert_eq!(bytes(&store), before);
}

// --- Standing ---------------------------------------------------------------------------------------------------

#[test]
fn standing_changes_are_checked_and_a_result_that_does_not_fit_is_a_named_error() {
    let mut store = store_of(vec![calm(1, BALANCED)]);
    store
        .change_standing(1, Standing::Honour, 1500)
        .expect("add");
    store
        .change_standing(1, Standing::Honour, -4000)
        .expect("subtract");
    store
        .change_standing(1, Standing::Fame, i64::MAX)
        .expect("up to the bound");
    store
        .change_standing(1, Standing::Popularity, i64::MIN)
        .expect("down to the bound");
    let record = mind_of(&store, 1);
    assert_eq!(
        (record.honour, record.fame, record.popularity),
        (-2500, i64::MAX, i64::MIN)
    );
    let before = bytes(&store);
    assert_eq!(
        store.change_standing(1, Standing::Fame, 1),
        Err(Error::StandingRange)
    );
    assert_eq!(
        store.change_standing(1, Standing::Popularity, -1),
        Err(Error::StandingRange)
    );
    assert_eq!(bytes(&store), before, "the value is unchanged");
    assert_eq!(
        store.change_standing(5, Standing::Fame, 1),
        Err(Error::MissingCharacter)
    );
}

#[test]
fn the_yearly_fade_rounds_toward_zero_and_is_exact_at_the_64_bit_bounds() {
    let rules = shipped();
    let held = |popularity, fame, honour| PersonalityRecord {
        popularity,
        fame,
        honour,
        ..mind()
    };
    let birth = date(750, 3, 5);
    let mut store = store_of(vec![
        person(1, birth, BALANCED, held(1999, -1999, 1)),
        person(2, birth, BALANCED, held(i64::MAX, i64::MIN, i64::MAX - 1)),
        person(3, birth, BALANCED, held(0, 19, -19)),
    ]);
    store.birthday_pass(rules, date(751, 3, 5)).expect("pass");
    let fade = |value: i64| i64::try_from(i128::from(value) * 950 / 1000).expect("fits");
    let one = mind_of(&store, 1);
    assert_eq!(
        (one.popularity, one.fame, one.honour),
        (1899, -1899, 0),
        "toward zero, not down"
    );
    let two = mind_of(&store, 2);
    assert_eq!(two.popularity, fade(i64::MAX));
    assert_eq!(two.fame, fade(i64::MIN));
    assert_eq!(two.honour, fade(i64::MAX - 1));
    assert_eq!(
        two.popularity, 8_762_203_435_012_037_016,
        "i64::MAX x 0.95, with no intermediate wrap"
    );
    assert_eq!(two.fame, -8_762_203_435_012_037_017, "i64::MIN x 0.95");
    let three = mind_of(&store, 3);
    assert_eq!((three.popularity, three.fame, three.honour), (0, 18, -18));
}

// --- The birthday pass -------------------------------------------------------------------------------------------

#[test]
fn the_birthday_pass_runs_on_the_birthday_and_not_on_the_birth_date() {
    let rules = shipped();
    let marked = PersonalityRecord {
        honour: 1000,
        ..mind()
    };
    let leap = date(772, 2, 29);
    assert!(ck_sim::is_leap_year(772) && !ck_sim::is_leap_year(773) && ck_sim::is_leap_year(776));
    let born = |id, birth| person(id, birth, BALANCED, marked);
    let run = |today: GameTime| {
        let mut store = store_of(vec![
            born(1, date(768, 3, 5)),
            born(2, leap),
            born(3, date(769, 3, 5)),
        ]);
        store.birthday_pass(rules, today).expect("pass");
        [1, 2, 3].map(|id| mind_of(&store, id).honour != 1000)
    };
    assert_eq!(
        run(date(769, 3, 5)),
        [true, false, false],
        "after the birth year"
    );
    assert_eq!(
        run(date(768, 3, 5)),
        [false, false, false],
        "a character born today has no pass that day"
    );
    assert_eq!(run(date(769, 3, 6)), [false, false, false]);
    assert_eq!(run(date(770, 3, 5)), [true, false, true]);
    // 29 February: on 1 March in a common year, on the leap day in a leap year, never on 28 February or the day after.
    assert_eq!(run(date(773, 3, 1)), [false, true, false]);
    assert_eq!(run(date(773, 2, 28)), [false, false, false]);
    assert_eq!(run(date(776, 2, 29)), [false, true, false]);
    assert_eq!(run(date(776, 3, 1)), [false, false, false]);
    assert_eq!(
        run(date(772, 3, 1)),
        [false, false, false],
        "the leap year of birth itself: before the date's anniversary"
    );
}

#[test]
fn humour_drift_pulls_each_pair_and_rounds_to_the_nearest_thousandth() {
    let rules = shipped();
    let birth = date(750, 3, 5);
    let today = date(751, 3, 5);
    let drift = |humours: [i64; 4], vice: [i64; 7]| {
        let mut store = store_of(vec![person(
            1,
            birth,
            humours,
            PersonalityRecord { vice, ..mind() },
        )]);
        store.birthday_pass(rules, today).expect("pass");
        mind_of(&store, 1).vice
    };
    // blood 8 leans to lust (index 2) and gluttony (4): 0.01 x 3 = 0.03 toward the sin; black bile 7 to envy (3);
    // yellow bile 3 to wrath (5) and pride (0) toward the virtue; phlegm 5 to nothing.
    assert_eq!(
        drift([8000, 3000, 7000, 5000], [0; 7]),
        [20, 0, -30, -20, -30, 20, 0]
    );
    // greed has no humour; humours at 5 leave every pair alone.
    assert_eq!(
        drift(BALANCED, [1000, 2000, 3000, 4000, 5000, 6000, 7000]),
        [1000, 2000, 3000, 4000, 5000, 6000, 7000]
    );
    // exact halves round away from zero: the pull 0.5 thousandths is 1, the pull -0.5 is -1.
    assert_eq!(drift([5050, 5000, 5000, 5000], [0; 7])[2], -1);
    assert_eq!(drift([4950, 5000, 5000, 5000], [0; 7])[2], 1);
    // held to -7..=7 at both ends.
    assert_eq!(drift([10_000, 0, 10_000, 10_000], [-7000; 7])[2], -7000);
    assert_eq!(drift([0, 10_000, 0, 0], [7000; 7])[2], 7000);
}

#[test]
fn the_passes_depend_only_on_a_characters_own_state() {
    let rules = shipped();
    let marked = |stress, honour| PersonalityRecord {
        stress,
        honour,
        melancholy: 2,
        vice: [1000, -2000, 3000, -4000, 5000, -6000, 7000],
        ..mind()
    };
    let characters = || {
        vec![
            person(
                1,
                date(750, 3, 5),
                [8000, 3000, 7000, 5000],
                marked(50_000, 1999),
            ),
            person(
                2,
                date(740, 3, 5),
                [2000, 9000, 1000, 6000],
                marked(80_000, -1999),
            ),
            person(3, date(751, 3, 5), BALANCED, marked(10_000, 3)),
            person(
                4,
                date(700, 7, 9),
                [9000, 9000, 9000, 9000],
                marked(99_999, 77),
            ),
        ]
    };
    let mut together = store_of(characters());
    together.monthly_recovery(rules).expect("recovery");
    together
        .birthday_pass(rules, date(756, 3, 5))
        .expect("birthday");
    for character in characters() {
        let id = character.id;
        let mut alone = store_of(vec![character]);
        alone.monthly_recovery(rules).expect("recovery");
        alone
            .birthday_pass(rules, date(756, 3, 5))
            .expect("birthday");
        assert_eq!(
            mind_of(&alone, id),
            mind_of(&together, id),
            "character {id}"
        );
    }
    // the same results whatever order the records were inserted in.
    let mut reversed = characters();
    reversed.reverse();
    let mut other = store_of(reversed);
    other.monthly_recovery(rules).expect("recovery");
    other
        .birthday_pass(rules, date(756, 3, 5))
        .expect("birthday");
    assert_eq!(bytes(&other), bytes(&together));
    // one pass of each changed something, so the comparison is not vacuous.
    assert_ne!(mind_of(&together, 1), marked(50_000, 1999));
}

// --- Birth --------------------------------------------------------------------------------------------------------

/// Mean and standard deviation (rounded to whole thousandths) of `values`, integer arithmetic only.
fn mean_and_sd(values: &[i64]) -> (i64, i64) {
    let n = i128::try_from(values.len()).expect("count");
    let mean = values.iter().map(|v| i128::from(*v)).sum::<i128>() / n;
    let variance = values
        .iter()
        .map(|v| (i128::from(*v) - mean).pow(2))
        .sum::<i128>()
        / n;
    (
        i64::try_from(mean).expect("fits"),
        i64::try_from(variance.isqrt()).expect("fits"),
    )
}
fn founders(data: &Personality, count: usize, place: &str, seed: u64) -> CharacterStore {
    let mut store = CharacterStore::default();
    let ctx = context(data, seed);
    for index in 0..count {
        let sex = if index % 2 == 0 {
            Sex::Male
        } else {
            Sex::Female
        };
        store
            .create_founder(&ctx, new(sex, place), &irish(), &Pops)
            .expect("founder");
    }
    store
}
fn column(store: &CharacterStore, pair: usize) -> Vec<i64> {
    store
        .iter()
        .map(|c| {
            if pair < 7 {
                c.personality.vice[pair]
            } else {
                c.personality.manner[pair - 7]
            }
        })
        .collect()
}

#[test]
fn a_founders_pairs_centre_on_the_culture_mean_with_the_spread() {
    let data = fixture(|data| {
        data.culture_means.get_mut("ulaid").unwrap()[1] = 1000; // greed_charity
        data.culture_means.get_mut("ulaid").unwrap()[7] = -8000; // guarded_open
    });
    let store = founders(&data, 3000, "a", 11);
    let (greed, greed_sd) = mean_and_sd(&column(&store, 1));
    let (open, open_sd) = mean_and_sd(&column(&store, 7));
    // standard error of the mean: 2400 / sqrt(3000) = 44, 6400 / sqrt(3000) = 117; tolerances are about four of them.
    assert!((greed - 1000).abs() <= 180, "vice mean {greed}");
    assert!((open + 8000).abs() <= 470, "manner mean {open}");
    // the spread: within 6% (the standard error of a standard deviation is about 1.3%; clamping trims the vice a little).
    assert!(
        (greed_sd - 2400).abs() <= 2400 * 6 / 100 + 60,
        "vice spread {greed_sd}"
    );
    assert!(
        (open_sd - 6400).abs() <= 6400 * 6 / 100,
        "manner spread {open_sd}"
    );
    // every pair has its own draw: the nine and seven columns are not copies of each other.
    assert_ne!(column(&store, 1), column(&store, 2));
}

#[test]
fn vice_virtue_is_held_to_seven_and_a_manner_beyond_twenty_is_kept() {
    let data = fixture(|data| {
        data.culture_means.get_mut("ulaid").unwrap()[0] = 6900;
        data.culture_means.get_mut("ulaid").unwrap()[7] = 20_000;
    });
    let store = founders(&data, 600, "a", 5);
    let vice = column(&store, 0);
    assert!(vice.iter().all(|v| (-7000..=7000).contains(v)));
    assert!(
        vice.contains(&7000),
        "clamped draws sit exactly on the bound"
    );
    let manner = column(&store, 7);
    assert!(
        manner.iter().any(|v| *v > 20_000),
        "manner is stored unbounded"
    );
    // a manner mean of +24 with no spread is kept as +24 and feeds opinion unclamped.
    let exact = fixture(|data| {
        data.manner_spread = 0;
        data.culture_means.get_mut("ulaid").unwrap()[7] = 24_000;
    });
    let kept = founders(&exact, 2, "a", 3);
    assert!(kept.iter().all(|c| c.personality.manner[0] == 24_000));
    let mut a = person(1, date(750, 1, 1), BALANCED, mind());
    let b = person(
        2,
        date(750, 1, 1),
        BALANCED,
        PersonalityRecord {
            manner: [24_000, 0, 0, 0, 0, 0, 0, 0, 0],
            ..mind()
        },
    );
    // (9 x 10 - 24) / 9 = 7.333 plus the humour compatibility 5: clamping the 24 to 20 would give 7.778.
    assert_eq!(ck_characters::opinion(shipped(), &a, &b), Ok(7333 + 5000));
    a.personality.manner[0] = 24_000;
    assert_eq!(ck_characters::opinion(shipped(), &a, &b), Ok(10_000 + 5000));
}

#[test]
fn birth_defaults_are_zero_for_stress_standing_and_the_trait_levels() {
    let data = shipped();
    let ctx = context(data, 9);
    let mut store = CharacterStore::default();
    let father = store
        .create_founder(&ctx, new(Sex::Male, "a"), &irish(), &Pops)
        .unwrap();
    let mother = store
        .create_founder(&ctx, new(Sex::Female, "a"), &irish(), &Pops)
        .unwrap();
    let none = archive("defaults");
    let child = store
        .create_child(
            &ctx,
            new(Sex::Male, "a"),
            &irish(),
            &Lineage {
                father,
                mother,
                archive: &none,
            },
            &Pops,
        )
        .unwrap();
    for id in [father, mother, child] {
        let c = mind_of(&store, id);
        assert_eq!(
            (
                c.stress,
                c.popularity,
                c.fame,
                c.honour,
                c.drunkard,
                c.rage_fits,
                c.melancholy
            ),
            (0, 0, 0, 0, 0, 0, 0)
        );
    }
}

#[test]
fn a_child_of_extreme_parents_regresses_halfway_toward_its_pops_culture_mean() {
    // The parents' pop is Irish (mean 0), the child's pop Pictish (mean +10 on guarded_open).
    let data = fixture(|data| {
        data.culture_means.get_mut("cruthin").unwrap()[7] = 10_000;
    });
    let ctx = context(&data, 21);
    let extreme = PersonalityRecord {
        manner: [-20_000, 0, 0, 0, 0, 0, 0, 0, 0],
        ..mind()
    };
    let mut father = person(1, date(730, 1, 1), BALANCED, extreme);
    let mut mother = person(2, date(731, 1, 1), BALANCED, extreme);
    mother.sex = Sex::Female;
    father.sex = Sex::Male;
    let none = archive("regress");
    let mut store = store_of(vec![father, mother]);
    let lineage = Lineage {
        father: 1,
        mother: 2,
        archive: &none,
    };
    let mut open = Vec::new();
    for _ in 0..1500 {
        let id = store
            .create_child(&ctx, new(Sex::Male, "b"), &irish(), &lineage, &Pops)
            .expect("child");
        open.push(mind_of(&store, id).manner[0]);
    }
    // 10 + 0.5 x (-20 - 10) = -5; the noise has spread 6.4 x 0.935 = 5.98 (standard error 154): within about five.
    let (mean, sd) = mean_and_sd(&open);
    assert!((mean + 5000).abs() <= 700, "child mean {mean}");
    assert!((sd - 5984).abs() <= 450, "child spread {sd}");
    // the culture of the *child's* pop is what counts: the same children born into the Irish pop centre on -10.
    let mut at_home = Vec::new();
    for _ in 0..1500 {
        let id = store
            .create_child(&ctx, new(Sex::Male, "a"), &irish(), &lineage, &Pops)
            .expect("child");
        at_home.push(mind_of(&store, id).manner[0]);
    }
    let (home_mean, _) = mean_and_sd(&at_home);
    assert!((home_mean + 10_000).abs() <= 700, "home mean {home_mean}");
}

#[test]
fn an_unknown_parent_is_a_founder_of_the_childs_pop_culture() {
    let data = fixture(|data| {
        data.culture_means.get_mut("cruthin").unwrap()[7] = 10_000;
    });
    let ctx = context(&data, 33);
    let mut father = person(
        1,
        date(730, 1, 1),
        BALANCED,
        PersonalityRecord {
            manner: [-20_000, 0, 0, 0, 0, 0, 0, 0, 0],
            ..mind()
        },
    );
    father.sex = Sex::Male;
    let none = archive("unknown-parent");
    let mut store = store_of(vec![father]);
    let lineage = Lineage {
        father: 1,
        mother: UNKNOWN,
        archive: &none,
    };
    let mut open = Vec::new();
    for _ in 0..1500 {
        let id = store
            .create_child(&ctx, new(Sex::Male, "b"), &irish(), &lineage, &Pops)
            .expect("child");
        open.push(mind_of(&store, id).manner[0]);
    }
    // The synthetic mother is Pictish (mean 10, spread 6.4): the parents' average is (-20 + 10) / 2 = -5 and the child
    // centres on 10 + 0.5 x (-5 - 10) = 2.5, with spread sqrt(5.98^2 + (0.5 x 3.2)^2) = 6.2 (standard error 160).
    let (mean, _) = mean_and_sd(&open);
    assert!((mean - 2500).abs() <= 750, "child mean {mean}");
}

#[test]
fn a_closed_population_keeps_each_pairs_spread_for_ten_generations() {
    let data = shipped();
    let ctx = context(data, 77);
    let none = archive("closed");
    let mut store = founders(data, 300, "a", 77);
    let spread_at = |store: &CharacterStore, ids: &[u64], pair: usize| {
        let values: Vec<i64> = ids
            .iter()
            .map(|id| {
                let c = store.get(*id).unwrap();
                if pair < 7 {
                    c.personality.vice[pair]
                } else {
                    c.personality.manner[pair - 7]
                }
            })
            .collect();
        mean_and_sd(&values).1
    };
    let mut generation: Vec<u64> = store.iter().map(|c| c.id).collect();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut pick = |limit: usize| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(state >> 33).unwrap() % limit
    };
    for _ in 0..10 {
        let men: Vec<u64> = generation
            .iter()
            .copied()
            .filter(|id| store.get(*id).unwrap().sex == Sex::Male)
            .collect();
        let women: Vec<u64> = generation
            .iter()
            .copied()
            .filter(|id| store.get(*id).unwrap().sex == Sex::Female)
            .collect();
        let mut next = Vec::new();
        for index in 0..300 {
            let sex = if index % 2 == 0 {
                Sex::Male
            } else {
                Sex::Female
            };
            let lineage = Lineage {
                father: men[pick(men.len())],
                mother: women[pick(women.len())],
                archive: &none,
            };
            next.push(
                store
                    .create_child(&ctx, new(sex, "a"), &irish(), &lineage, &Pops)
                    .expect("child"),
            );
        }
        generation = next;
    }
    // Every pair, after ten generations of 300: within 15% of the data's spread (the sampling error of a
    // standard deviation of 300 is 4%; the vice pairs are trimmed a little by their bounds).
    for pair in 0..16 {
        let spread = if pair < 7 { 2400 } else { 6400 };
        let found = spread_at(&store, &generation, pair);
        assert!(
            (found - spread).abs() <= spread * 15 / 100,
            "pair {pair}: spread {found} against {spread}"
        );
    }
}

#[test]
fn the_child_formula_rounds_its_exact_sum_once() {
    // culture mean 0, parents' average 0.001, weight 0.5, spread 0.001 and z = -0.5: the exact terms +0.5 and -0.4675 sum to
    // 0.0325 thousandths, which rounds to 0 (rounding the noise first would give 1).
    assert_eq!(child_value(0, 1, 1, 500, 1, -500_000), Ok(0));
    assert_eq!(
        child_value(0, 1, 1, 500, 1, 0),
        Ok(1),
        "without noise the half rounds away from zero"
    );
    assert_eq!(child_value(0, -1, -1, 500, 1, 0), Ok(-1));
    // with the shipped weight and a mean away from zero, no noise: mean + w (average - mean).
    assert_eq!(
        child_value(10_000, -20_000, -20_000, 500, 6400, 0),
        Ok(-5000)
    );
    // a weight beyond 1 is refused.
    assert_eq!(child_value(0, 0, 0, -1, 1, 0), Err(Error::Arithmetic));
}

#[test]
fn a_creation_into_an_unresolvable_culture_is_refused_with_nothing_changed() {
    let data = shipped();
    let ctx = context(data, 1);
    let mut store = founders(data, 2, "a", 1);
    let none = archive("culture");
    let before = bytes(&store);
    // no pop at all, a pop whose culture is not in the data, and a pop with no culture.
    for place in ["x", "n"] {
        assert_eq!(
            store.create_founder(&ctx, new(Sex::Male, place), &irish(), &Pops),
            Err(Error::Culture),
            "founder into `{place}`"
        );
        let lineage = Lineage {
            father: UNKNOWN,
            mother: UNKNOWN,
            archive: &none,
        };
        assert_eq!(
            store.create_child(&ctx, new(Sex::Male, place), &irish(), &lineage, &Pops),
            Err(Error::Culture),
            "child into `{place}`"
        );
        assert_eq!(bytes(&store), before);
    }
    // a bare head-count function knows no culture.
    assert_eq!(
        store.create_founder(&ctx, new(Sex::Male, "a"), &irish(), &|_: &PopLocator| Some(
            9
        )),
        Err(Error::Culture)
    );
    assert_eq!(bytes(&store), before);
    // and the next creation still gets the next identifier.
    assert_eq!(
        store.create_founder(&ctx, new(Sex::Male, "a"), &irish(), &Pops),
        Ok(3)
    );
}

#[test]
fn the_same_seed_and_creation_sequence_give_the_same_personalities() {
    let first = founders(shipped(), 200, "a", 4);
    let second = founders(shipped(), 200, "a", 4);
    assert_eq!(bytes(&first), bytes(&second));
    let other = founders(shipped(), 200, "a", 5);
    assert_ne!(bytes(&first), bytes(&other));
    // a creation appended after another leaves the earlier personalities alone.
    let mut longer = founders(shipped(), 200, "a", 4);
    let ctx = context(shipped(), 4);
    longer
        .create_founder(&ctx, new(Sex::Male, "a"), &irish(), &Pops)
        .unwrap();
    for (a, b) in first.iter().zip(longer.iter()) {
        assert_eq!(a.personality, b.personality);
    }
}

// --- Opinion ------------------------------------------------------------------------------------------------------

fn bare(id: u64) -> Character {
    person(id, date(750, 1, 1), BALANCED, mind())
}
/// Opinion of `b` by `a` less the opinion of two bare characters (manner 10 + humour 5 = 15 points).
fn extra(a: &Character, b: &Character) -> i64 {
    let base = ck_characters::opinion(shipped(), &bare(1), &bare(2)).expect("opinion");
    assert_eq!(base, 15_000);
    ck_characters::opinion(shipped(), a, b).expect("opinion") - base
}

#[test]
fn opinion_manner_and_humour_compatibility_are_clamped_to_their_maxima() {
    let rules = shipped();
    let apart = |gap: i64| PersonalityRecord {
        manner: [gap; 9],
        ..mind()
    };
    let a = bare(1);
    let manner = |gap: i64| {
        let b = person(2, date(750, 1, 1), BALANCED, apart(gap));
        ck_characters::opinion(rules, &a, &b).unwrap() - 5000
    };
    assert_eq!(manner(0), 10_000);
    assert_eq!(manner(9000), 1000);
    assert_eq!(manner(10_000), 0);
    assert_eq!(manner(30_000), -10_000, "-20 is clamped to -10");
    assert_eq!(manner(-7000), 3000, "distance, not difference");
    let humour = |gap: i64| {
        let b = person(
            2,
            date(750, 1, 1),
            [5000 + gap, 5000 - gap, 5000 + gap, 5000 - gap],
            mind(),
        );
        ck_characters::opinion(rules, &a, &b).unwrap() - 10_000
    };
    assert_eq!(humour(0), 5000);
    assert_eq!(humour(2000), 3000);
    assert_eq!(humour(5000), 0);
    assert_eq!(humour(-5000), 0);
    // a gap beyond the humour range cannot be built (values are held to 0..10), so check the clamp on the record directly.
    let far = person(2, date(750, 1, 1), [0, 10_000, 0, 10_000], mind());
    let mid = person(3, date(750, 1, 1), [10_000, 0, 10_000, 0], mind());
    assert_eq!(
        ck_characters::opinion(rules, &far, &mid).unwrap(),
        10_000 - 5000
    );
    // manner is a mean over nine pairs, rounded to the nearest thousandth: (90 - 1) / 9 = 9.889.
    let one = person(
        2,
        date(750, 1, 1),
        BALANCED,
        PersonalityRecord {
            manner: [1000, 0, 0, 0, 0, 0, 0, 0, 0],
            ..mind()
        },
    );
    assert_eq!(
        ck_characters::opinion(rules, &a, &one).unwrap(),
        10_000 - 111 + 5000
    );
}

#[test]
fn opinion_counts_the_others_standing_up_to_a_cap() {
    let standing = |honour, fame, popularity| {
        person(
            2,
            date(750, 1, 1),
            BALANCED,
            PersonalityRecord {
                honour,
                fame,
                popularity,
                ..mind()
            },
        )
    };
    // 0.10 x 10 + 0.05 x 20 + 0.05 x 40 = 4 points.
    assert_eq!(extra(&bare(1), &standing(10_000, 20_000, 40_000)), 4000);
    assert_eq!(extra(&bare(1), &standing(-10_000, 0, 0)), -1000);
    assert_eq!(
        extra(&bare(1), &standing(0, 1, 0)),
        0,
        "0.05 thousandths rounds to nothing"
    );
    assert_eq!(
        extra(&bare(1), &standing(0, 10, 0)),
        1,
        "a half rounds away from zero"
    );
    assert_eq!(extra(&bare(1), &standing(0, -10, 0)), -1);
    // the three together are clamped to plus or minus 20, near the representation bound too.
    assert_eq!(extra(&bare(1), &standing(1_000_000, 0, 0)), 20_000);
    assert_eq!(
        extra(&bare(1), &standing(i64::MAX, i64::MAX, i64::MAX)),
        20_000
    );
    assert_eq!(
        extra(&bare(1), &standing(i64::MIN, i64::MIN, i64::MIN)),
        -20_000
    );
    assert_eq!(
        extra(&bare(1), &standing(0, i64::MAX, i64::MIN)),
        0,
        "equal weights offset before the clamp"
    );
    // it is the other's standing that counts.
    assert_eq!(extra(&standing(10_000, 20_000, 40_000), &bare(2)), 0);
}

#[test]
fn opinion_adds_a_term_for_each_of_the_others_graded_traits() {
    let with = |name: &str, level: u8| {
        let mut b = bare(2);
        b.genetics.level.insert(name.to_owned(), level);
        b
    };
    let beauty = [0, 5000, 10_000, 15_000, 20_000, 25_000];
    let ill = [0, -5000, -10_000, -15_000, -20_000, -25_000];
    let fool = [5000, 5000, 10_000, 10_000, 15_000, 20_000];
    let madness = [-2000, -4000, -6000, -8000, -10_000, -12_000];
    for level in 1..=6_u8 {
        let index = usize::from(level) - 1;
        assert_eq!(
            extra(&bare(1), &with("beauty", level)),
            beauty[index],
            "beauty {level}"
        );
        assert_eq!(
            extra(&bare(1), &with("ill_favoured", level)),
            ill[index],
            "ill_favoured {level}"
        );
        assert_eq!(
            extra(&bare(1), &with("fool", level)),
            fool[index],
            "fool {level}"
        );
        assert_eq!(
            extra(&bare(1), &with("madness", level)),
            madness[index],
            "madness {level}"
        );
    }
    assert_eq!(
        extra(&bare(1), &with("beauty", 0)),
        0,
        "level 0 adds nothing"
    );
    assert_eq!(extra(&bare(1), &with("fool", 0)), 0);
    // rage fits and drunkard are the character's own levels.
    let rage = [-2000, -5000, -8000, -10_000, -15_000, -20_000];
    let drunk = [10_000, 5000, 0, -5000, -10_000, -20_000];
    for level in 1..=6_u8 {
        let index = usize::from(level) - 1;
        let mut b = bare(2);
        b.personality.rage_fits = level;
        assert_eq!(extra(&bare(1), &b), rage[index], "rage fits {level}");
        let mut b = bare(2);
        b.personality.drunkard = level;
        assert_eq!(extra(&bare(1), &b), drunk[index], "drunkard {level}");
    }
    // melancholy is not an opinion term.
    let mut b = bare(2);
    b.personality.melancholy = 6;
    assert_eq!(extra(&bare(1), &b), 0);
}

#[test]
fn a_fool_in_power_counts_by_the_office_marker_until_titles() {
    let mut ruler = bare(2);
    ruler.genetics.level.insert("fool".to_owned(), 6);
    assert_eq!(
        extra(&bare(1), &ruler),
        20_000,
        "an ordinary fool of level 6"
    );
    ruler.office = Some(Office::Ruler);
    // 20 + (-15 x power 2) = -10
    assert_eq!(extra(&bare(1), &ruler), -10_000);
    ruler.genetics.level.insert("fool".to_owned(), 1);
    assert_eq!(extra(&bare(1), &ruler), 5000 + -3000 * 2);
    // any other office counts 0.
    ruler.office = Some(Office::Heir);
    assert_eq!(extra(&bare(1), &ruler), 5000);
    ruler.office = Some(Office::HighCleric);
    assert_eq!(extra(&bare(1), &ruler), 5000);
}

#[test]
fn two_drunkards_add_the_kinship_by_the_lower_level() {
    let drunk = |id, level| {
        let mut c = bare(id);
        c.personality.drunkard = level;
        c
    };
    // two SSS: -20 + 40 = +20.
    assert_eq!(extra(&drunk(1, 6), &drunk(2, 6)), 20_000);
    // a SSS opinion of a C: the C's term 10 plus the kinship of the lower level (C) 5; and the other way round -20 + 5.
    assert_eq!(extra(&drunk(1, 6), &drunk(2, 1)), 15_000);
    assert_eq!(extra(&drunk(1, 1), &drunk(2, 6)), -15_000);
    // only one drunkard: no kinship.
    assert_eq!(extra(&drunk(1, 6), &bare(2)), 0);
    assert_eq!(extra(&bare(1), &drunk(2, 6)), -20_000);
    assert_eq!(extra(&drunk(1, 3), &drunk(2, 3)), 15_000);
}

#[test]
fn reading_opinion_changes_nothing() {
    let mut a = bare(1);
    a.personality.honour = 5;
    let mut b = bare(2);
    b.personality.drunkard = 2;
    let store = store_of(vec![a, b]);
    let before = bytes(&store);
    let first = store.opinion(shipped(), 1, 2).unwrap();
    assert_eq!(store.opinion(shipped(), 1, 2), Ok(first));
    assert_ne!(store.opinion(shipped(), 2, 1), Ok(first + 1));
    assert_eq!(bytes(&store), before);
    assert_eq!(store.opinion(shipped(), 1, 9), Err(Error::MissingCharacter));
    assert_eq!(store.opinion(shipped(), 9, 1), Err(Error::MissingCharacter));
}

// --- Save, archive, no floating point --------------------------------------------------------------------------------

#[test]
fn save_load_and_the_archive_keep_every_new_field() {
    let rich = PersonalityRecord {
        vice: [-7000, -1, 0, 1, 2, 3, 7000],
        manner: [-24_000, 1, 2, 3, 4, 5, 6, 7, 31_000],
        stress: 77_777,
        popularity: i64::MIN,
        fame: -3,
        honour: i64::MAX,
        drunkard: 6,
        rage_fits: 3,
        melancholy: 1,
    };
    let store = store_of(vec![person(1, date(750, 1, 1), [1, 2, 3, 4], rich)]);
    let restored = CharacterStore::from_bytes(&bytes(&store)).expect("load");
    assert_eq!(mind_of(&restored, 1), rich);
    assert_eq!(bytes(&restored), bytes(&store));
    let mut store = store;
    let mut grave = archive("save");
    store.die(1, date(760, 1, 1), &mut grave).expect("die");
    let reopened = Archive::open(
        std::env::temp_dir().join(format!("ck-personality-{}-save", std::process::id())),
    )
    .expect("reopen");
    let found = reopened.get(1).expect("read").expect("archived");
    assert_eq!(found.personality, rich);
    assert_eq!(found.genetics.value[9..13], [1, 2, 3, 4]);
}

#[test]
fn the_new_state_and_its_data_use_no_floating_point() {
    let sources = ["src/personality.rs", "../economy/src/personality.rs"];
    for source in sources {
        let text =
            std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(source))
                .expect("source");
        let code: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!code.contains("f32") && !code.contains("f64"), "{source}");
        assert!(
            text.contains("#![deny(clippy::float_arithmetic)]"),
            "{source}"
        );
    }
}

#[test]
fn opinion_standing_with_extreme_weights_is_held_to_the_cap_without_overflow() {
    let weighted = |weights: [i64; 3], standing: [i64; 3]| {
        let data = fixture(|data| {
            data.opinion.honour_weight = weights[0];
            data.opinion.fame_weight = weights[1];
            data.opinion.popularity_weight = weights[2];
        });
        let b = person(
            2,
            date(750, 1, 1),
            BALANCED,
            PersonalityRecord {
                honour: standing[0],
                fame: standing[1],
                popularity: standing[2],
                ..mind()
            },
        );
        ck_characters::opinion(&data, &bare(1), &b).expect("a valid case never fails") - 15_000
    };
    // three products of 2^126: their sum overflows 128 bits, the cap decides.
    assert_eq!(weighted([i64::MIN; 3], [i64::MIN; 3]), 20_000);
    assert_eq!(weighted([i64::MAX; 3], [i64::MIN; 3]), -20_000);
    // two positive terms that overflow together, then a negative one: the sum is still far past the cap.
    assert_eq!(
        weighted([i64::MIN; 3], [i64::MIN, i64::MIN, i64::MAX]),
        20_000
    );
    // exact cancellation is kept: equal and opposite terms give nothing.
    assert_eq!(
        weighted([i64::MAX, i64::MAX, 0], [i64::MAX, i64::MIN + 1, 0]),
        0
    );
}

#[test]
fn a_vice_draw_near_the_64_bit_limit_is_held_to_seven_before_it_narrows() {
    let edge = |mean: i64| {
        fixture(|data| {
            data.vice_spread = 1;
            data.parents_weight = 0;
            data.culture_means.get_mut("ulaid").unwrap()[0] = mean;
            data.culture_means.get_mut("ulaid").unwrap()[7] = mean;
            data.manner_spread = 1000;
        })
    };
    for (mean, bound) in [(i64::MAX, 7000), (i64::MIN, -7000)] {
        let data = edge(mean);
        let none = archive("edge");
        let mut made = 0;
        for seed in 0..40 {
            // the manner pair at this magnitude may not fit; the vice pair must always be held to its bound. A refused
            // creation allocates no identifier, so each attempt uses its own seed.
            let ctx = context(&data, seed);
            let mut store = CharacterStore::default();
            match store.create_founder(&ctx, new(Sex::Male, "a"), &irish(), &Pops) {
                Ok(id) => {
                    made += 1;
                    assert_eq!(mind_of(&store, id).vice[0], bound);
                }
                Err(error) => {
                    assert_eq!(error, Error::Arithmetic);
                    assert_eq!(bytes(&store), bytes(&CharacterStore::default()));
                }
            }
        }
        assert!(
            made > 0 && made < 40,
            "some manner draws fit and some do not: {made}"
        );
        // a child of two known parents with no parental weight draws the same way. A refused creation allocates no
        // identifier, so a retry would draw the same variate: each attempt uses its own seed.
        let mut children = 0;
        for seed in 0..40 {
            let ctx = context(&data, seed);
            let mut mother = person(2, date(730, 1, 1), BALANCED, mind());
            mother.sex = Sex::Female;
            let mut parents = store_of(vec![person(1, date(730, 1, 1), BALANCED, mind()), mother]);
            let lineage = Lineage {
                father: 1,
                mother: 2,
                archive: &none,
            };
            match parents.create_child(&ctx, new(Sex::Male, "a"), &irish(), &lineage, &Pops) {
                Ok(id) => {
                    children += 1;
                    assert_eq!(mind_of(&parents, id).vice[0], bound);
                }
                Err(error) => assert_eq!(error, Error::Arithmetic, "mean {mean}"),
            }
        }
        assert!(children > 0 && children < 40, "{children}");
    }
    // the unbounded wide value is not narrowed or bounded by the formula itself.
    assert_eq!(
        child_value(i64::MAX, i64::MAX, i64::MAX, 0, 1, 500_000),
        Ok(i128::from(i64::MAX) + 1)
    );
}

#[test]
fn event_multipliers_that_cannot_be_represented_are_named_errors() {
    // The loader refuses a negative `events_per_point`, but rules built by hand must not divide by zero.
    let negative = fixture(|data| data.stress.events_per_point = -1_000_000);
    let huge = fixture(|data| data.stress.events_per_point = i64::MAX);
    let melancholic = [5000, 5000, 10_000, 0];
    let stressed = PersonalityRecord {
        stress: 100_000,
        ..mind()
    };
    let store = store_of(vec![person(1, date(750, 1, 1), melancholic, stressed)]);
    for kind in [EventKind::Causing, EventKind::Lifting] {
        assert_eq!(
            store.event_multiplier(&negative, 1, kind),
            Err(Error::Arithmetic)
        );
    }
    // a causing multiplier past 64 bits is a named error; the lifting one shrinks to zero.
    assert_eq!(
        store.event_multiplier(&huge, 1, EventKind::Causing),
        Err(Error::Arithmetic)
    );
    assert_eq!(store.event_multiplier(&huge, 1, EventKind::Lifting), Ok(0));
}
