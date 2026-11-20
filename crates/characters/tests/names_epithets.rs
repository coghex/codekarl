//! Namesakes and epithets (CK-365, characters slice 5b).
//!
//! Characters whose exact state a case needs are built as records and loaded through the canonical encoding. The events are none
//! (a quiet world), so a happening's roll cannot hide the behaviour under test. A chance that a case needs to be certain is set in a
//! copy of the shipped life-cycle data; the data itself is the shipped data.

use ck_characters::{
    Archive, Character, CharacterStore, Context, GeneticRecord, LifeLog, Lineage, NewCharacter,
    Office, PersonalityRecord, PopLocator, Populations, Sex, UNKNOWN, World, cycle::shown_name,
    genetics::Mendel,
};
use ck_economy::{
    Epithet, EventRules, LifeCycle, PopGenetics, Rules, events::Registry, genetics::PPB,
};
use ck_sim::{Canonical, Date, GameTime};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

fn at(year: u16, month: u8, day: u8, hour: u8) -> GameTime {
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour,
    })
    .expect("valid date")
}
fn date(year: u16, month: u8, day: u8) -> GameTime {
    at(year, month, day, 0)
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
/// No events at all: a birth or a happening then runs only what the life cycle itself does.
fn quiet() -> &'static EventRules {
    static QUIET: OnceLock<EventRules> = OnceLock::new();
    QUIET.get_or_init(|| {
        let pop_types: Vec<&str> = rules().pop_types.iter().map(|p| p.id.as_str()).collect();
        EventRules::parse(
            "character-events.toml",
            "",
            "attributes-skills.toml",
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../assets/game/events/attributes-skills.toml"),
            )
            .expect("shipped data"),
            &Registry {
                peoples: &rules().peoples,
                genetics: &rules().genetics,
                pop_types: &pop_types,
            },
        )
        .expect("an events file with no events")
    })
}
fn irish() -> PopGenetics {
    rules().genetics.seed_pop("ulaid").expect("Irish start set")
}

/// The pops: `a` an Irish noble pop (`ulaid`, gaelic) and `e` an English one (`northumbrian`); every pop has the same `religion`.
struct Pops {
    religion: &'static str,
}
impl Populations for Pops {
    fn heads(&self, _: &PopLocator) -> Option<u64> {
        Some(1_000_000)
    }
    fn culture(&self, locator: &PopLocator) -> Option<String> {
        Some(
            match locator.place.as_str() {
                "e" => "northumbrian",
                _ => "ulaid",
            }
            .to_owned(),
        )
    }
    fn kind(&self, _: &PopLocator) -> Option<String> {
        Some("noble".to_owned())
    }
    fn luxuries_pm(&self, _: &PopLocator) -> Option<u16> {
        Some(600)
    }
    fn religion(&self, _: &PopLocator) -> Option<String> {
        Some(self.religion.to_owned())
    }
    fn genetics(&self, _: &PopLocator) -> Option<PopGenetics> {
        Some(irish())
    }
    fn distance_m(&self, a: &PopLocator, b: &PopLocator) -> Option<u64> {
        (a.place == b.place).then_some(0)
    }
    fn move_head(
        &self,
        _: &PopLocator,
        _: &PopLocator,
        _: bool,
    ) -> Result<ck_economy::MoveUndo, ck_characters::Error> {
        Ok(ck_economy::MoveUndo::default())
    }
    fn restore_move(&self, _: &ck_economy::MoveUndo) {}
}
const PAGAN: Pops = Pops { religion: "" };
const CHRISTIAN: Pops = Pops {
    religion: "christianity",
};

fn unique(tag: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "ck-names-{}-{tag}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::remove_file(&path).ok();
    path
}
struct Env {
    archive: Archive,
    logs: LifeLog,
}
impl Env {
    fn new(tag: &str) -> Self {
        Self {
            archive: Archive::open(unique(&format!("{tag}-arch"))).expect("archive"),
            logs: LifeLog::open(unique(&format!("{tag}-logs"))).expect("logs"),
        }
    }
}
fn world<'a>(
    lifecycle: &'a LifeCycle,
    env: &'a mut Env,
    pops: &'a Pops,
) -> World<'a, Pops, Archive, LifeLog> {
    World {
        rules: quiet(),
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle,
        seed: 7,
        populations: pops,
        archive: &mut env.archive,
        logs: &mut env.logs,
    }
}

/// A record with ordinary stats (a man of 70 kg), humours of 5 and fecundity 10, in pop `place`, of the `irish` name pool.
fn person(id: u64, birth: GameTime, place: &str, sex: Sex) -> Character {
    let mut genetics = GeneticRecord::default();
    genetics.value[9..13].copy_from_slice(&[5000; 4]);
    for stat in 0..9 {
        genetics.value[stat] = 10_000;
    }
    genetics.value[1] = 70_000;
    let mut life = ck_characters::LifeRecord::default();
    "irish".clone_into(&mut life.pool);
    Character {
        id,
        name: format!("c{id}"),
        sex,
        birth,
        death: None,
        pop: locator(place),
        father: UNKNOWN,
        mother: UNKNOWN,
        spouses: Vec::new(),
        children: Vec::new(),
        office: None,
        full_until: None,
        genetics: Box::new(genetics),
        personality: PersonalityRecord::default(),
        life: Box::new(life),
    }
}
fn store_with_next(characters: Vec<Character>, next: u64) -> CharacterStore {
    let live: BTreeMap<u64, Character> = characters.into_iter().map(|c| (c.id, c)).collect();
    let mut bytes = Vec::new();
    next.encode(&mut bytes).expect("encode");
    live.encode(&mut bytes).expect("encode");
    CharacterStore::from_bytes(&bytes).expect("a store of these records")
}
fn store_of(characters: Vec<Character>) -> CharacterStore {
    let next = characters.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    store_with_next(characters, next)
}
fn get(store: &CharacterStore, id: u64) -> &Character {
    store.get(id).expect("live")
}
fn reloaded(store: &CharacterStore) -> CharacterStore {
    CharacterStore::from_bytes(&store.to_bytes().expect("encode")).expect("decode")
}

/// The shipped life-cycle data with `edit` applied, no risk of dying at a delivery and single births.
fn lifecycle_with(edit: impl FnOnce(&mut LifeCycle)) -> LifeCycle {
    let mut lifecycle = rules().lifecycle.clone();
    lifecycle.death.childbirth_ppb = 0;
    // One child at a delivery unless a case asks for more.
    lifecycle.conception.twins_ppb = 0;
    lifecycle.conception.triplets_ppb = 0;
    edit(&mut lifecycle);
    lifecycle
}
/// The shipped data where every epithet but `key` has no chance and `key` is certain.
fn only(key: &str) -> LifeCycle {
    lifecycle_with(|l| {
        assert!(l.epithets.iter().any(|e| e.key == key), "no epithet {key}");
        for epithet in &mut l.epithets {
            epithet.chance_ppb = if epithet.key == key { PPB } else { 0 };
        }
    })
}

// ======================================================================================================================
// Epithets
// ======================================================================================================================

/// The epithets held by a character born 700-5-5 in pop `place`, after the birthday pass of its `years`th birthday.
fn gained(
    lifecycle: &LifeCycle,
    years: u16,
    place: &str,
    sex: Sex,
    tweak: impl FnOnce(&mut Character),
) -> BTreeSet<String> {
    let mut c = person(1, date(700, 5, 5), place, sex);
    tweak(&mut c);
    let mut store = store_of(vec![c]);
    let mut env = Env::new("birthday");
    store
        .run_birthdays(
            &mut world(lifecycle, &mut env, &PAGAN),
            date(700 + years, 5, 5),
        )
        .expect("birthdays");
    get(&store, 1)
        .epithets()
        .map(|(key, _)| key.to_owned())
        .collect()
}
fn set(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|key| (*key).to_owned()).collect()
}
fn blind() -> Mendel {
    Mendel {
        copies: 2,
        shows: true,
    }
}

#[test]
fn each_gainable_epithet_is_gained_by_its_own_rule_and_only_then() {
    let g = |key: &str, years, place, tweak: &dyn Fn(&mut Character)| {
        gained(&only(key), years, place, Sex::Male, |c| tweak(c))
    };
    // gluindub: no condition, but gaelic only (the English pop `e` is of another subgroup).
    assert_eq!(g("gluindub", 30, "a", &|_| {}), set(&["gluindub"]));
    assert_eq!(g("gluindub", 30, "e", &|_| {}), set(&[]));
    // find: `grade_min = { beauty = 2 }`.
    let beauty = |level: u8| {
        move |c: &mut Character| {
            c.genetics.level.insert("beauty".into(), level);
        }
    };
    assert_eq!(g("find", 30, "a", &beauty(2)), set(&["find"]));
    assert_eq!(g("find", 30, "a", &beauty(6)), set(&["find"]));
    assert_eq!(g("find", 30, "a", &beauty(1)), set(&[]));
    // the_wise: `skill_min = { scholarship = 18 }` (the margins are wide: a birthday also moves skills).
    let scholarship = |level: i64| {
        move |c: &mut Character| {
            c.life.skills.insert("scholarship".into(), level);
        }
    };
    assert_eq!(
        g("the_wise", 30, "a", &scholarship(40_000)),
        set(&["the_wise"])
    );
    assert_eq!(g("the_wise", 30, "a", &scholarship(5_000)), set(&[]));
    // the_bold: `manner_min = { wary_daring = 12 }` and `count_min = { battles_survived = 3 }`.
    let bold = |manner: i64, battles: u64| {
        move |c: &mut Character| {
            c.personality.manner[3] = manner;
            c.life.counters.insert("battles_survived".into(), battles);
        }
    };
    assert_eq!(g("the_bold", 30, "a", &bold(15_000, 3)), set(&["the_bold"]));
    assert_eq!(g("the_bold", 30, "a", &bold(15_000, 9)), set(&["the_bold"]));
    assert_eq!(
        g("the_bold", 30, "a", &bold(15_000, 2)),
        set(&[]),
        "too few battles"
    );
    assert_eq!(
        g("the_bold", 30, "a", &bold(5_000, 3)),
        set(&[]),
        "not daring enough"
    );
    // the_blind and the_lame: a trait held (`has`), universal (also in the English pop).
    let has = |key: &'static str| {
        move |c: &mut Character| {
            c.genetics.mendelian.insert(key.into(), blind());
        }
    };
    assert_eq!(g("the_blind", 30, "a", &has("blind")), set(&["the_blind"]));
    assert_eq!(g("the_blind", 30, "e", &has("blind")), set(&["the_blind"]));
    assert_eq!(g("the_blind", 30, "a", &|_| {}), set(&[]));
    assert_eq!(
        g("the_lame", 30, "a", &has("club_foot")),
        set(&["the_lame"])
    );
    assert_eq!(g("the_lame", 30, "a", &has("blind")), set(&[]));
    // the_fat: the shown weight at least 1.3 times the rolled one. A shown weight is held to a floor of 30 kg, so a rolled
    // weight of 20 kg shows 30 (1.5), 25 kg shows 30 (1.2), and a rolled weight of nothing divides by nothing: not gained.
    let weight = |rolled: i64| move |c: &mut Character| c.genetics.value[1] = rolled;
    assert_eq!(g("the_fat", 30, "a", &weight(20_000)), set(&["the_fat"]));
    assert_eq!(
        g("the_fat", 30, "a", &weight(23_000)),
        set(&["the_fat"]),
        "30 / 23 is 1.304"
    );
    assert_eq!(
        g("the_fat", 30, "a", &weight(23_100)),
        set(&[]),
        "30 / 23.1 is 1.298"
    );
    assert_eq!(g("the_fat", 30, "a", &weight(25_000)), set(&[]));
    assert_eq!(g("the_fat", 30, "a", &weight(0)), set(&[]));
    // the_old: `age = [70, 200]`.
    assert_eq!(g("the_old", 70, "a", &|_| {}), set(&["the_old"]));
    assert_eq!(g("the_old", 71, "a", &|_| {}), set(&["the_old"]));
    assert_eq!(g("the_old", 69, "a", &|_| {}), set(&[]));
}

#[test]
fn becc_needs_adulthood_and_the_shown_height_at_most_the_limit_of_its_sex() {
    let becc = only("becc");
    let height = |value: i64| move |c: &mut Character| c.genetics.value[0] = value;
    // A man: at most 160 cm. Majority is 17 for a Gaelic man and 14 for a Gaelic woman.
    for (years, value, expect) in [
        (30, 160_000, true),
        (30, 160_001, false),
        (17, 120_000, true),
        (16, 120_000, false),
    ] {
        assert_eq!(
            gained(&becc, years, "a", Sex::Male, height(value)),
            if expect { set(&["becc"]) } else { set(&[]) },
            "man of {years}, {value}"
        );
    }
    // A woman shows her genetic value and a woman's offset: at most 150 cm.
    let offset = rules().genetics.female_offset(0);
    for (years, shown, expect) in [
        (30, 150_000, true),
        (30, 150_001, false),
        (14, 100_000, true),
        (13, 100_000, false),
    ] {
        assert_eq!(
            gained(&becc, years, "a", Sex::Female, height(shown - offset)),
            if expect { set(&["becc"]) } else { set(&[]) },
            "woman of {years}, shown {shown}"
        );
    }
}

#[test]
fn a_non_gaelic_character_gains_a_universal_epithet_but_not_a_gaelic_one() {
    // Every epithet is certain: the English character gains what its conditions and scope allow and no more.
    let certain = lifecycle_with(|l| {
        for epithet in &mut l.epithets {
            epithet.chance_ppb = PPB;
        }
    });
    let english = gained(&certain, 70, "e", Sex::Male, |c| {
        c.genetics.mendelian.insert("blind".into(), blind());
    });
    assert_eq!(english, set(&["the_blind", "the_old"]));
    // The Irish man also gains the two gaelic ones that hold: `gluindub` (no condition) and `becc` (an adult of shown height at most
    // 160 cm: the stats here are ordinary, which shows as the floor of 100 cm).
    let irish = gained(&certain, 70, "a", Sex::Male, |c| {
        c.genetics.mendelian.insert("blind".into(), blind());
    });
    assert_eq!(irish, set(&["becc", "gluindub", "the_blind", "the_old"]));
}

#[test]
fn the_pilgrimage_epithet_goes_to_a_christian_and_not_to_a_character_of_another_religion() {
    let lifecycle = only("the_pilgrim");
    let held = |pops: &Pops, happening: &str| {
        let mut store = store_of(vec![person(1, date(720, 5, 5), "a", Sex::Male)]);
        let mut env = Env::new("pilgrim");
        store
            .report_happening(
                &mut world(&lifecycle, &mut env, pops),
                1,
                happening,
                date(768, 3, 3),
            )
            .expect("happening");
        get(&store, 1)
            .epithets()
            .map(|(key, _)| key.to_owned())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(held(&CHRISTIAN, "pilgrimage"), set(&["the_pilgrim"]));
    assert_eq!(held(&PAGAN, "pilgrimage"), set(&[]));
    assert_eq!(
        held(&Pops { religion: "islam" }, "pilgrimage"),
        set(&[]),
        "another religion"
    );
    // Another happening gains nothing, and the yearly pass never gains a happening's epithet.
    assert_eq!(held(&CHRISTIAN, "marriage"), set(&[]));
    let mut store = store_of(vec![person(1, date(720, 5, 5), "a", Sex::Male)]);
    let mut env = Env::new("pilgrim-yearly");
    store
        .run_birthdays(
            &mut world(&lifecycle, &mut env, &CHRISTIAN),
            date(768, 5, 5),
        )
        .expect("birthdays");
    assert_eq!(get(&store, 1).epithets().count(), 0);
}

#[test]
fn an_epithet_is_kept_after_its_condition_fails_and_is_never_gained_twice() {
    let lifecycle = only("the_blind");
    let mut blind_man = person(1, date(700, 5, 5), "a", Sex::Male);
    blind_man.genetics.mendelian.insert("blind".into(), blind());
    let mut store = store_of(vec![blind_man]);
    let mut env = Env::new("kept");
    store
        .run_birthdays(&mut world(&lifecycle, &mut env, &PAGAN), date(730, 5, 5))
        .expect("birthdays");
    let first: Vec<_> = get(&store, 1).epithets().collect();
    assert_eq!(first, [("the_blind", date(730, 5, 5))]);
    // The sight comes back (the trait is gone) and a year goes by: the epithet stays, with the date it was gained.
    let mut loaded = reloaded(&store);
    let mut cured = get(&loaded, 1).clone();
    cured.genetics.mendelian.clear();
    loaded = store_of(vec![cured]);
    loaded
        .run_birthdays(&mut world(&lifecycle, &mut env, &PAGAN), date(731, 5, 5))
        .expect("birthdays");
    assert_eq!(get(&loaded, 1).epithets().collect::<Vec<_>>(), first);
    // And while the condition still holds it is not gained again (the first date stands).
    let mut again = get(&loaded, 1).clone();
    again.genetics.mendelian.insert("blind".into(), blind());
    let mut loaded = store_of(vec![again]);
    loaded
        .run_birthdays(&mut world(&lifecycle, &mut env, &PAGAN), date(732, 5, 5))
        .expect("birthdays");
    assert_eq!(get(&loaded, 1).epithets().collect::<Vec<_>>(), first);
}

#[test]
fn the_gain_is_a_roll_on_the_epithet_purpose_deterministic_exact_at_the_ends() {
    // A chance of one half over 200 characters: some gain and some do not, and the same world gives the same result.
    let half = lifecycle_with(|l| {
        for epithet in &mut l.epithets {
            epithet.chance_ppb = if epithet.key == "gluindub" {
                PPB / 2
            } else {
                0
            };
        }
    });
    let run = || {
        let mut all = Vec::new();
        for id in 1..=200 {
            all.push(person(id, date(700, 5, 5), "a", Sex::Male));
        }
        let mut store = store_of(all);
        let mut env = Env::new("half");
        store
            .run_birthdays(&mut world(&half, &mut env, &PAGAN), date(730, 5, 5))
            .expect("birthdays");
        store
    };
    let (first, second) = (run(), run());
    assert_eq!(first, second, "deterministic");
    let held = first.iter().filter(|c| c.epithets().count() == 1).count();
    assert!((70..=130).contains(&held), "{held} of 200 at one half");
    assert_eq!(reloaded(&first), first, "a save keeps the epithets");
    // The ends are exact: never at 0, always at 1 (`only` makes every other epithet 0).
    let none = lifecycle_with(|l| l.epithets.iter_mut().for_each(|e| e.chance_ppb = 0));
    assert_eq!(gained(&none, 30, "a", Sex::Male, |_| {}), set(&[]));
    assert_eq!(
        gained(&only("gluindub"), 30, "a", Sex::Male, |_| {}),
        set(&["gluindub"])
    );
}

// --- The shown epithet -----------------------------------------------------------------------------------------------------

fn holding(epithets: &[(&str, GameTime)]) -> Character {
    let mut c = person(1, date(700, 5, 5), "a", Sex::Male);
    for (key, when) in epithets {
        c.life.epithets.insert((*key).to_owned(), *when);
    }
    c
}
fn shown<'a>(c: &Character, rules: &'a [Epithet]) -> Option<&'a str> {
    c.shown_epithet(rules).map(|e| e.key.as_str())
}

#[test]
fn the_shown_epithet_is_the_most_important_then_the_earlier_acquired_then_the_first_listed() {
    let list = &rules().lifecycle.epithets;
    let (early, late) = (date(730, 1, 1), date(740, 1, 1));
    // Higher importance wins whatever the order: the_wise 35, find 20.
    assert_eq!(
        shown(&holding(&[("find", early), ("the_wise", late)]), list),
        Some("the_wise")
    );
    assert_eq!(
        shown(&holding(&[("find", late), ("the_wise", early)]), list),
        Some("the_wise")
    );
    // Equal importance (35): the earlier acquired stays, whichever is listed first (the_wise is listed before the_blind).
    assert_eq!(
        shown(&holding(&[("the_wise", late), ("the_blind", early)]), list),
        Some("the_blind")
    );
    assert_eq!(
        shown(&holding(&[("the_wise", early), ("the_blind", late)]), list),
        Some("the_wise")
    );
    // Equal importance on the same day: the one listed first. The hour of the day is not part of the day.
    assert_eq!(
        shown(&holding(&[("the_blind", early), ("the_wise", early)]), list),
        Some("the_wise")
    );
    // Nothing held: nothing shown; a key the rules do not know is ignored.
    assert_eq!(shown(&holding(&[]), list), None);
    assert_eq!(shown(&holding(&[("not_an_epithet", early)]), list), None);
}

#[test]
fn equal_importance_on_one_day_from_a_yearly_and_a_happening_hook_shows_the_one_listed_first_in_both_orders_and_across_a_save()
 {
    // the_lame (yearly, 25) is listed before the_pilgrim (happening, 25): on one day the lame one is shown whichever hook ran first.
    let lifecycle = lifecycle_with(|l| {
        for epithet in &mut l.epithets {
            epithet.chance_ppb = if ["the_lame", "the_pilgrim"].contains(&epithet.key.as_str()) {
                PPB
            } else {
                0
            };
        }
    });
    let position = |key: &str| {
        lifecycle
            .epithets
            .iter()
            .position(|e| e.key == key)
            .expect(key)
    };
    assert!(position("the_lame") < position("the_pilgrim"));
    let mut lame = person(1, date(720, 5, 5), "a", Sex::Male);
    lame.genetics.mendelian.insert("club_foot".into(), blind());
    let day = (768, 5, 5);
    for yearly_first in [true, false] {
        let mut store = store_of(vec![lame.clone()]);
        let mut env = Env::new("same-day");
        // The two hooks run at different hours of the one day, with a save and a load between them.
        let yearly = |store: &mut CharacterStore, env: &mut Env| {
            store
                .run_birthdays(
                    &mut world(&lifecycle, env, &CHRISTIAN),
                    at(day.0, day.1, day.2, 2),
                )
                .expect("birthdays");
        };
        let pilgrimage = |store: &mut CharacterStore, env: &mut Env| {
            store
                .report_happening(
                    &mut world(&lifecycle, env, &CHRISTIAN),
                    1,
                    "pilgrimage",
                    at(day.0, day.1, day.2, 15),
                )
                .expect("happening");
        };
        if yearly_first {
            yearly(&mut store, &mut env);
            store = reloaded(&store);
            pilgrimage(&mut store, &mut env);
        } else {
            pilgrimage(&mut store, &mut env);
            store = reloaded(&store);
            yearly(&mut store, &mut env);
        }
        let c = get(&store, 1);
        assert_eq!(
            c.epithets().count(),
            2,
            "both held (yearly first: {yearly_first})"
        );
        assert_eq!(
            shown(c, &lifecycle.epithets),
            Some("the_lame"),
            "yearly first: {yearly_first}"
        );
        // Each stores the start of the day it was gained, whatever the hour.
        assert!(
            c.epithets()
                .all(|(_, when)| when == date(day.0, day.1, day.2))
        );
    }
}

#[test]
fn the_order_of_the_file_and_not_of_the_keys_breaks_a_tie() {
    // `zzz` is listed first and sorts last: it is shown, whichever hook granted it first.
    let text = r#"
[epithet.zzz]
shown = "Zed"
importance = 10
scope = "universal"
trigger = "yearly"
when = {}
chance = 1.0

[epithet.aaa]
shown = "Ay"
importance = 10
scope = "universal"
trigger = "happening:pilgrimage"
when = {}
chance = 1.0
"#;
    let pop_types: Vec<&str> = rules().pop_types.iter().map(|p| p.id.as_str()).collect();
    let epithets = quiet()
        .parse_epithets(
            "epithets.toml",
            text,
            &Registry {
                peoples: &rules().peoples,
                genetics: &rules().genetics,
                pop_types: &pop_types,
            },
            &rules().religions,
        )
        .expect("a small epithets file");
    assert_eq!(
        epithets.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(),
        ["zzz", "aaa"]
    );
    let lifecycle = lifecycle_with(|l| l.epithets = epithets.clone());
    let day = date(768, 5, 5);
    for yearly_first in [true, false] {
        let mut store = store_of(vec![person(1, date(720, 5, 5), "a", Sex::Male)]);
        let mut env = Env::new("file-order");
        if yearly_first {
            store
                .run_birthdays(&mut world(&lifecycle, &mut env, &PAGAN), day)
                .expect("birthdays");
            store
                .report_happening(
                    &mut world(&lifecycle, &mut env, &PAGAN),
                    1,
                    "pilgrimage",
                    day,
                )
                .expect("happening");
        } else {
            store
                .report_happening(
                    &mut world(&lifecycle, &mut env, &PAGAN),
                    1,
                    "pilgrimage",
                    day,
                )
                .expect("happening");
            store
                .run_birthdays(&mut world(&lifecycle, &mut env, &PAGAN), day)
                .expect("birthdays");
        }
        assert_eq!(
            shown(get(&store, 1), &lifecycle.epithets),
            Some("zzz"),
            "yearly first: {yearly_first}"
        );
    }
}

#[test]
fn the_shown_name_ends_with_the_shown_epithet_and_a_save_and_the_archive_keep_it() {
    let lifecycle = &rules().lifecycle;
    let father = {
        let mut f = person(2, date(700, 1, 1), "a", Sex::Male);
        "Domnall".clone_into(&mut f.name);
        f
    };
    let mut son = holding(&[("find", date(740, 1, 1)), ("the_wise", date(741, 1, 1))]);
    "Áed".clone_into(&mut son.name);
    son.id = 3;
    son.father = 2;
    // The 5a shown name, then the shown epithet's text; the base name is untouched.
    assert_eq!(
        shown_name(&son, Some(&father), lifecycle),
        "Áed mac Domnaill the Wise"
    );
    assert_eq!(son.name, "Áed");
    son.life.epithets.clear();
    assert_eq!(
        shown_name(&son, Some(&father), lifecycle),
        "Áed mac Domnaill"
    );
    son.life.epithets.insert("find".into(), date(740, 1, 1));
    assert_eq!(shown_name(&son, None, lifecycle), "Áed Find ('the Fair')");
    // Gaining an epithet does not touch the stored base name, and an archived record keeps its epithets across a reopening.
    let mut store = store_of(vec![father, son]);
    let mut env = Env::new("archived-epithets");
    store
        .die_with_events(
            &mut world(lifecycle, &mut env, &PAGAN),
            3,
            date(770, 1, 1),
            "fever",
        )
        .expect("death");
    let read = env.archive.get(3).expect("read").expect("archived");
    assert_eq!(read.name, "Áed");
    assert_eq!(
        read.epithets().collect::<Vec<_>>(),
        [("find", date(740, 1, 1))]
    );
}

// ======================================================================================================================
// Namesakes
// ======================================================================================================================

/// Names no pool offers, so a child that carries one of them was named for that relative.
const GRAN: &str = "Zgran"; // father's father (20)
const NAN: &str = "Znan"; // father's mother (21)
const UNCLE: &str = "Zuncle"; // father's brother (22)
const AUNT: &str = "Zaunt"; // father's sister (23)
const FATHER: &str = "Zfather"; // the father (2)
const GRAMPS: &str = "Zgramps"; // mother's father (30)
const GRANNY: &str = "Zgranny"; // mother's mother (31)
const MOTHERS_SISTER: &str = "Zsister"; // (32)
const MOTHERS_BROTHER: &str = "Zbrother"; // (33)
const ANC_FF: &str = "Zancff"; // father's father's father (40)
const ANC_FFM: &str = "Zancffm"; // father's father's mother (41)
const ANC_MF: &str = "Zancmf"; // father's mother's father (42)
const ANC_MMM: &str = "Zancmmm"; // mother's mother's mother (43)

fn named(id: u64, sex: Sex, name: &str) -> Character {
    let mut c = person(id, date(680, 1, 1), "a", sex);
    name.clone_into(&mut c.name);
    c
}

/// The family of a child due on 768-10-1: mother 1 (pregnant by father 2); the father's parents 20 and 21 and their other children
/// 22 (a man) and 23 (a woman); the mother's parents 30 and 31 and their other children 32 (a woman) and 33 (a man); and three
/// generations above: 40 and 41 (the parents of 20), 42 (the father of 21) and 43 (the mother of 31).
fn family() -> BTreeMap<u64, Character> {
    let mut mother = person(1, date(743, 6, 1), "a", Sex::Female);
    mother.life.pregnant_by = Some(2);
    mother.life.due = Some(date(768, 10, 1));
    mother.father = 30;
    mother.mother = 31;
    mother.spouses = vec![2];
    let mut father = named(2, Sex::Male, FATHER);
    father.birth = date(738, 6, 1);
    father.father = 20;
    father.mother = 21;
    father.spouses = vec![1];
    let mut all = vec![mother, father];
    let mut add = |id, sex, name, father, mother, children: &[u64]| {
        let mut c = named(id, sex, name);
        c.father = father;
        c.mother = mother;
        c.children = children.to_vec();
        all.push(c);
    };
    add(20, Sex::Male, GRAN, 40, 41, &[2, 22, 23]);
    add(21, Sex::Female, NAN, 42, UNKNOWN, &[2, 22, 23]);
    add(22, Sex::Male, UNCLE, 20, 21, &[]);
    add(23, Sex::Female, AUNT, 20, 21, &[]);
    add(30, Sex::Male, GRAMPS, UNKNOWN, UNKNOWN, &[1, 32, 33]);
    add(31, Sex::Female, GRANNY, UNKNOWN, 43, &[1, 32, 33]);
    add(32, Sex::Female, MOTHERS_SISTER, 30, 31, &[]);
    add(33, Sex::Male, MOTHERS_BROTHER, 30, 31, &[]);
    add(40, Sex::Male, ANC_FF, UNKNOWN, UNKNOWN, &[20]);
    add(41, Sex::Female, ANC_FFM, UNKNOWN, UNKNOWN, &[20]);
    add(42, Sex::Male, ANC_MF, UNKNOWN, UNKNOWN, &[21]);
    add(43, Sex::Female, ANC_MMM, UNKNOWN, UNKNOWN, &[31]);
    all.into_iter().map(|c| (c.id, c)).collect()
}

/// Run the delivery of a family whose `dead` members are archived (died 760-1-1) and return the children, by identifier, when the
/// next free identifier is `next` (the identifier picks the child's sex and its rolls).
fn deliver(
    lifecycle: &LifeCycle,
    members: &BTreeMap<u64, Character>,
    dead: &[u64],
    next: u64,
) -> Vec<Character> {
    let mut env = Env::new("deliver");
    let mut live = Vec::new();
    for (id, c) in members {
        if dead.contains(id) {
            let mut c = c.clone();
            c.death = Some(date(760, 1, 1));
            env.archive.append_record(&c);
        } else {
            live.push(c.clone());
        }
    }
    let mut store = store_with_next(live, next);
    store
        .run_deliveries(&mut world(lifecycle, &mut env, &PAGAN), date(768, 10, 1))
        .expect("delivery");
    store.iter().filter(|c| c.id >= next).cloned().collect()
}
trait AppendRecord {
    fn append_record(&mut self, record: &Character);
}
impl AppendRecord for Archive {
    fn append_record(&mut self, record: &Character) {
        ck_characters::ArchiveSink::append(self, record).expect("archive a record");
    }
}

/// One child born when the next free identifier is `next`.
fn born(
    lifecycle: &LifeCycle,
    members: &BTreeMap<u64, Character>,
    dead: &[u64],
    next: u64,
) -> Character {
    let mut children = deliver(lifecycle, members, dead, next);
    assert_eq!(children.len(), 1);
    children.remove(0)
}
/// The names of the sons and of the daughters born over twenty-four trials.
fn trials(
    lifecycle: &LifeCycle,
    members: &BTreeMap<u64, Character>,
    dead: &[u64],
) -> (Vec<String>, Vec<String>) {
    let (mut sons, mut daughters) = (Vec::new(), Vec::new());
    for next in 1000..1024 {
        let child = born(lifecycle, members, dead, next);
        match child.sex {
            Sex::Male => sons.push(child.name),
            Sex::Female => daughters.push(child.name),
        }
    }
    assert!(
        sons.len() >= 3 && daughters.len() >= 3,
        "sons {sons:?} daughters {daughters:?}"
    );
    (sons, daughters)
}
/// Namesakes of one kind, certain when there is a candidate.
fn kind(kind: &str) -> LifeCycle {
    lifecycle_with(|l| l.namesakes.order = vec![(kind.to_owned(), PPB)])
}
fn all_are(names: &[String], expected: &str) -> bool {
    names.iter().all(|name| name == expected)
}
/// Whether no name is a relative's (a pool name instead).
fn none_is_a_relative(names: &[String]) -> bool {
    let pool = &rules().lifecycle.pools["irish"];
    names
        .iter()
        .all(|name| pool.men.iter().any(|(m, _)| m == name) || pool.women.contains(name))
}

#[test]
fn the_father_is_a_candidate_only_when_dead_and_for_sons_the_mother_is_never_dead_at_her_delivery()
{
    let family = family();
    let (sons, daughters) = trials(&kind("father"), &family, &[2]);
    assert!(all_are(&sons, FATHER), "{sons:?}");
    // A daughter's candidate would be her mother, who is delivering: none, so the pool.
    assert!(none_is_a_relative(&daughters), "{daughters:?}");
    // A living father: no candidate.
    let (sons, daughters) = trials(&kind("father"), &family, &[]);
    assert!(none_is_a_relative(&sons) && none_is_a_relative(&daughters));
}

#[test]
fn the_grandfather_is_the_fathers_father_for_a_son_and_the_mothers_mother_for_a_daughter_live_or_dead()
 {
    let family = family();
    for dead in [&[][..], &[20, 31][..]] {
        let (sons, daughters) = trials(&kind("grandfather"), &family, dead);
        assert!(all_are(&sons, GRAN), "{sons:?} (dead {dead:?})");
        assert!(all_are(&daughters, GRANNY), "{daughters:?} (dead {dead:?})");
    }
    // An unknown or unreadable grandparent is no candidate.
    let mut orphans = family.clone();
    orphans.get_mut(&2).unwrap().father = UNKNOWN;
    orphans.get_mut(&1).unwrap().mother = UNKNOWN;
    let (sons, daughters) = trials(&kind("grandfather"), &orphans, &[]);
    assert!(none_is_a_relative(&sons) && none_is_a_relative(&daughters));
}

#[test]
fn an_uncle_is_a_dead_brother_of_the_father_and_an_aunt_a_dead_sister_of_the_mother() {
    let family = family();
    // Dead: the father's brother (22) and sister (23), the mother's sister (32) and brother (33).
    let (sons, daughters) = trials(&kind("uncle"), &family, &[22, 23, 32, 33]);
    assert!(all_are(&sons, UNCLE), "{sons:?}");
    assert!(all_are(&daughters, MOTHERS_SISTER), "{daughters:?}");
    // Living ones are not candidates, and neither is anyone of the other side or sex.
    let (sons, daughters) = trials(&kind("uncle"), &family, &[]);
    assert!(none_is_a_relative(&sons) && none_is_a_relative(&daughters));
    let (sons, daughters) = trials(&kind("uncle"), &family, &[23, 33]);
    assert!(none_is_a_relative(&sons) && none_is_a_relative(&daughters));
}

#[test]
fn an_ancestor_is_a_dead_one_of_the_childs_sex_on_its_side_beyond_the_grandfather() {
    let family = family();
    // The great-grandfathers of a son (40 and 42) are candidates; the grandfather (20, dead) is not an ancestor, and neither is the
    // woman 41 or the mother's side. A daughter's is the mother's mother's mother (43).
    let (sons_of_the_grandfather, _) = trials(&kind("ancestor"), &family, &[20, 41, 30, 31]);
    assert!(
        none_is_a_relative(&sons_of_the_grandfather),
        "{sons_of_the_grandfather:?}"
    );
    let (sons, _) = trials(&kind("ancestor"), &family, &[40]);
    assert!(all_are(&sons, ANC_FF), "{sons:?}");
    let (sons, _) = trials(&kind("ancestor"), &family, &[42]);
    assert!(all_are(&sons, ANC_MF), "{sons:?}");
    let (sons, daughters) = trials(&kind("ancestor"), &family, &[40, 42, 43]);
    assert!(sons.iter().all(|n| n == ANC_FF || n == ANC_MF), "{sons:?}");
    assert!(
        sons.iter().any(|n| n == ANC_FF) && sons.iter().any(|n| n == ANC_MF),
        "both are drawn: {sons:?}"
    );
    assert!(all_are(&daughters, ANC_MMM), "{daughters:?}");
    // Live ancestors do not count.
    let (sons, daughters) = trials(&kind("ancestor"), &family, &[]);
    assert!(none_is_a_relative(&sons) && none_is_a_relative(&daughters));
}

#[test]
fn a_fathers_friend_is_no_candidate_until_slice_six() {
    let family = family();
    let (sons, daughters) = trials(&kind("fathers_friend"), &family, &[2, 20, 22, 40]);
    assert!(none_is_a_relative(&sons) && none_is_a_relative(&daughters));
}

#[test]
fn a_kind_without_a_candidate_passes_its_chance_on_added_to_the_next_kinds_base() {
    let family = family();
    let order = |order: &[(&str, u32)]| {
        lifecycle_with(|l| {
            l.namesakes.order = order.iter().map(|(k, c)| ((*k).to_owned(), *c)).collect();
        })
    };
    let half = PPB / 2;
    // The grandfather alone at one half: some sons are named for him and some are not.
    let (alone, _) = trials(&order(&[("grandfather", half)]), &family, &[]);
    assert!(
        alone.iter().any(|n| n == GRAN) && !all_are(&alone, GRAN),
        "{alone:?}"
    );
    // `fathers_friend` has no candidate: its half passes on, and half + half is certain (the sons are all named for him).
    let carried = order(&[("fathers_friend", half), ("grandfather", half)]);
    let (sons, _) = trials(&carried, &family, &[]);
    assert!(all_are(&sons, GRAN), "{sons:?}");
    // The carried chance is spent by the kind that has candidates, win or lose: `fathers_friend` carries 1/2 to `grandfather`
    // (chance 1/2, rolled), and `uncle` after it has only its own 1/2. Were the carry kept, `uncle` would be certain and no son
    // would be given a pool name.
    let spent = order(&[
        ("fathers_friend", half),
        ("grandfather", 0),
        ("uncle", half),
    ]);
    let (sons, _) = trials(&spent, &family, &[22]);
    assert!(sons.iter().any(|n| n == GRAN), "{sons:?}");
    assert!(sons.iter().any(|n| n == UNCLE), "{sons:?}");
    assert!(
        sons.iter().any(|n| !n.starts_with('Z')),
        "a pool name: {sons:?}"
    );
}

#[test]
fn a_rulers_importance_doubles_the_chance_after_the_carried_chance_is_added() {
    let mut family = family();
    let quarter = PPB / 4;
    let order = lifecycle_with(|l| {
        l.namesakes.order = vec![
            ("fathers_friend".to_owned(), quarter),
            ("grandfather".to_owned(), quarter),
        ];
    });
    // Not a ruler: (1/4 + 1/4) x 1 = 1/2, so some sons are not named for him.
    let (plain, _) = trials(&order, &family, &[]);
    assert!(
        !all_are(&plain, GRAN) && plain.iter().any(|n| n == GRAN),
        "{plain:?}"
    );
    // A ruler: (1/4 + 1/4) x 2 = 1, certain. (Were the factor applied to the base alone, 1/4 x 2 + 1/4 would be 3/4.)
    family.get_mut(&20).unwrap().office = Some(Office::Ruler);
    let (sons, _) = trials(&order, &family, &[]);
    assert!(all_are(&sons, GRAN), "{sons:?}");
    // Heir and high cleric count 0: the same as no office.
    for office in [Office::Heir, Office::HighCleric] {
        family.get_mut(&20).unwrap().office = Some(office);
        let (sons, _) = trials(&order, &family, &[]);
        assert!(!all_are(&sons, GRAN), "{office:?} {sons:?}");
    }
    // An archived ruler counts by the marker of its archived record.
    family.get_mut(&20).unwrap().office = Some(Office::Ruler);
    let (sons, _) = trials(&order, &family, &[20]);
    assert!(all_are(&sons, GRAN), "{sons:?}");
}

#[test]
fn a_member_of_a_ruling_house_adds_half_to_the_importance_through_a_living_ruler_of_its_house() {
    let mut family = family();
    // A chance of 2/3 is lifted to certain by the extra half (2/3 x 1.5) and not by nothing.
    let order =
        lifecycle_with(|l| l.namesakes.order = vec![("grandfather".to_owned(), 666_666_667)]);
    let (plain, _) = trials(&order, &family, &[]);
    assert!(
        !all_are(&plain, GRAN) && plain.iter().any(|n| n == GRAN),
        "{plain:?}"
    );
    // The grandfather (20) is of the house `ui_neill` whose living ruler is 60, another man: the grandfather himself is no ruler.
    "ui_neill".clone_into(&mut family.get_mut(&20).unwrap().life.house);
    let mut ruler = named(60, Sex::Male, "Zruler");
    ruler.office = Some(Office::Ruler);
    "ui_neill".clone_into(&mut ruler.life.house);
    family.insert(60, ruler);
    let (sons, _) = trials(&order, &family, &[]);
    assert!(all_are(&sons, GRAN), "{sons:?}");
    // The same with the grandfather dead and archived: the house of his record counts, the ruling being now.
    let (sons, _) = trials(&order, &family, &[20]);
    assert!(all_are(&sons, GRAN), "{sons:?}");
    // A house with no living ruler, and a member of another house, are plain.
    let mut lapsed = family.clone();
    lapsed.get_mut(&60).unwrap().office = None;
    let (sons, _) = trials(&order, &lapsed, &[]);
    assert!(!all_are(&sons, GRAN), "{sons:?}");
    let mut elsewhere = family.clone();
    "dal_riata".clone_into(&mut elsewhere.get_mut(&20).unwrap().life.house);
    let (sons, _) = trials(&order, &elsewhere, &[]);
    assert!(!all_are(&sons, GRAN), "{sons:?}");
    // A ruler of a ruling house counts both: 1 + 0.5. At 0.4 the chance is 0.4 x 2.5 = 1.
    let mut both = family.clone();
    both.get_mut(&20).unwrap().office = Some(Office::Ruler);
    let both_order =
        lifecycle_with(|l| l.namesakes.order = vec![("grandfather".to_owned(), 400_000_000)]);
    let (sons, _) = trials(&both_order, &both, &[]);
    assert!(all_are(&sons, GRAN), "{sons:?}");
}

#[test]
fn the_pick_is_weighted_by_one_plus_importance_over_a_large_sample() {
    // 400 couples with the same dead great-grandfathers 40 (the parents of the husband's father 20, a ruler or not) and 42 (a plain
    // man, the father of 21): a ruler weighs 2000 against 1000.
    let share = |ruler: bool| {
        let lifecycle = lifecycle_with(|l| {
            l.namesakes.order = vec![("ancestor".to_owned(), PPB)];
            l.conception.twins_ppb = 0;
            l.conception.triplets_ppb = 0;
        });
        let family = family();
        let mut env = Env::new("sample");
        let mut live = Vec::new();
        for id in [20, 21] {
            let mut c = family[&id].clone();
            c.children.clear();
            live.push(c);
        }
        for id in [40, 42] {
            let mut c = family[&id].clone();
            c.death = Some(date(760, 1, 1));
            if id == 40 && ruler {
                c.office = Some(Office::Ruler);
            }
            env.archive.append_record(&c);
        }
        for k in 0..400 {
            let mut wife = person(100 + 2 * k, date(743, 6, 1), "a", Sex::Female);
            wife.life.pregnant_by = Some(101 + 2 * k);
            wife.life.due = Some(date(768, 10, 1));
            let mut husband = person(101 + 2 * k, date(738, 6, 1), "a", Sex::Male);
            husband.father = 20;
            husband.mother = 21;
            live.push(wife);
            live.push(husband);
        }
        let mut store = store_with_next(live, 10_000);
        store
            .run_deliveries(&mut world(&lifecycle, &mut env, &PAGAN), date(768, 10, 1))
            .expect("delivery");
        let sons: Vec<&Character> = store
            .iter()
            .filter(|c| c.id >= 10_000 && c.sex == Sex::Male)
            .collect();
        assert!(sons.len() > 150, "{} sons", sons.len());
        let of_forty = sons.iter().filter(|c| c.name == ANC_FF).count();
        assert_eq!(
            of_forty + sons.iter().filter(|c| c.name == ANC_MF).count(),
            sons.len()
        );
        of_forty * 1000 / sons.len()
    };
    let with_ruler = share(true);
    let without = share(false);
    assert!(
        (560..770).contains(&with_ruler),
        "a ruler's share {with_ruler} per mille, about 2/3"
    );
    assert!(
        (400..600).contains(&without),
        "an equal share {without} per mille, about 1/2"
    );
}

// --- The names to avoid ---------------------------------------------------------------------------------------------------

/// The shipped data with the `irish` pool cut to `men` and `women` and no namesake kinds.
fn small_pool(men: &[&str], women: &[&str]) -> LifeCycle {
    lifecycle_with(|l| {
        l.namesakes.order.clear();
        let pool = l.pools.get_mut("irish").expect("irish pool");
        pool.men = men
            .iter()
            .map(|n| ((*n).to_owned(), format!("{n}s")))
            .collect();
        pool.women = women.iter().map(|n| (*n).to_owned()).collect();
    })
}
fn with_children(
    mut members: BTreeMap<u64, Character>,
    extra: Vec<Character>,
) -> BTreeMap<u64, Character> {
    for c in extra {
        members.insert(c.id, c);
    }
    members
}
fn child_of(id: u64, name: &str, father: u64, mother: u64) -> Character {
    let mut c = named(id, Sex::Male, name);
    c.birth = date(765, 1, 1);
    c.father = father;
    c.mother = mother;
    c
}

#[test]
fn a_living_full_siblings_name_is_excluded_from_the_pool_but_a_half_or_dead_sibling_does_not_exclude()
 {
    let lifecycle = small_pool(&["Aaa", "Bbb"], &["Aaa", "Bbb"]);
    // Without a sibling both names are drawn.
    let (sons, daughters) = trials(&lifecycle, &family(), &[]);
    assert!(
        sons.contains(&"Aaa".to_owned()) && sons.contains(&"Bbb".to_owned()),
        "{sons:?}"
    );
    assert!(
        daughters.contains(&"Aaa".to_owned()) && daughters.contains(&"Bbb".to_owned()),
        "{daughters:?}"
    );
    // A living full sibling `Aaa` (both parents the child's) takes it out of the draw; a half sibling (another mother) `Bbb` and a
    // dead full sibling `Bbb` take nothing out.
    let mut other_mother = named(60, Sex::Female, "Zother");
    other_mother.birth = date(745, 1, 1);
    let members = with_children(
        family(),
        vec![
            child_of(50, "Aaa", 2, 1),
            child_of(51, "Bbb", 2, 60),
            other_mother,
            child_of(52, "Bbb", 2, 1),
        ],
    );
    let (sons, daughters) = trials(&lifecycle, &members, &[52]);
    assert!(all_are(&sons, "Bbb"), "{sons:?}");
    assert!(all_are(&daughters, "Bbb"), "{daughters:?}");
    // The dead one is archived and does not exclude: were the living `Aaa` dead too, both names would be free again.
    let (sons, _) = trials(&lifecycle, &members, &[50, 52]);
    assert!(sons.contains(&"Aaa".to_owned()), "{sons:?}");
}

#[test]
fn a_siblings_name_is_excluded_from_the_namesake_candidates_and_the_pool() {
    // The only namesake candidate (the dead grandfather) is called Aaa, which is also a pool name, and a living full sibling is Aaa.
    let lifecycle = lifecycle_with(|l| {
        l.namesakes.order = vec![("grandfather".to_owned(), PPB)];
        let pool = l.pools.get_mut("irish").expect("irish pool");
        pool.men = vec![
            ("Aaa".to_owned(), "Aaas".to_owned()),
            ("Bbb".to_owned(), "Bbbs".to_owned()),
        ];
        pool.women = vec!["Aaa".to_owned(), "Bbb".to_owned()];
    });
    let mut members = family();
    for id in [20, 31] {
        "Aaa".clone_into(&mut members.get_mut(&id).unwrap().name);
    }
    // No sibling: the namesake is certain.
    let (sons, daughters) = trials(&lifecycle, &members, &[20, 31]);
    assert!(all_are(&sons, "Aaa") && all_are(&daughters, "Aaa"));
    // A living full sibling named Aaa: the candidate goes (the kind has none), and the pool draw avoids it too.
    let members = with_children(members, vec![child_of(50, "Aaa", 2, 1)]);
    let (sons, daughters) = trials(&lifecycle, &members, &[20, 31]);
    assert!(all_are(&sons, "Bbb"), "{sons:?}");
    assert!(all_are(&daughters, "Bbb"), "{daughters:?}");
}

#[test]
fn an_exhausted_pool_falls_back_to_the_full_pool() {
    let lifecycle = small_pool(&["Aaa"], &["Aaa"]);
    let members = with_children(family(), vec![child_of(50, "Aaa", 2, 1)]);
    let (sons, daughters) = trials(&lifecycle, &members, &[]);
    assert!(
        all_are(&sons, "Aaa") && all_are(&daughters, "Aaa"),
        "{sons:?} {daughters:?}"
    );
}

#[test]
fn in_a_twin_or_triplet_delivery_a_later_child_avoids_the_names_chosen_for_the_earlier_ones() {
    // The grandfathers of both sides (20 and 31) are called Zeno, who is also in the pool, and the namesake is certain: the first
    // child is named for a grandfather; the second cannot be (the name is taken) and the pool offers it, but not again.
    let multiple = |twins: u32, triplets: u32, men: &[&str]| {
        lifecycle_with(|l| {
            l.namesakes.order = vec![("grandfather".to_owned(), PPB)];
            l.conception.twins_ppb = twins;
            l.conception.triplets_ppb = triplets;
            let pool = l.pools.get_mut("irish").expect("irish pool");
            pool.men = men
                .iter()
                .map(|n| ((*n).to_owned(), format!("{n}s")))
                .collect();
            pool.women = men.iter().map(|n| (*n).to_owned()).collect();
        })
    };
    let mut members = family();
    for id in [20, 31] {
        "Zeno".clone_into(&mut members.get_mut(&id).unwrap().name);
    }
    for next in 1000..1016 {
        let twins = deliver(&multiple(PPB, 0, &["Zeno", "Qux"]), &members, &[], next);
        assert_eq!(twins.len(), 2);
        assert_eq!(twins[0].name, "Zeno", "named for a grandfather");
        assert_eq!(twins[1].name, "Qux", "the name is taken: another");
        let triplets = deliver(
            &multiple(0, PPB, &["Zeno", "Qux", "Rit"]),
            &members,
            &[],
            next,
        );
        assert_eq!(triplets.len(), 3);
        let names: BTreeSet<&str> = triplets.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            BTreeSet::from(["Zeno", "Qux", "Rit"]),
            "three different names"
        );
        assert_eq!(triplets[0].name, "Zeno");
    }
}

#[test]
fn resolution_reads_stored_base_names_so_a_relatives_epithet_changes_nothing() {
    let lifecycle = kind("grandfather");
    let plain = born(&lifecycle, &family(), &[20], 1000);
    assert_eq!(
        plain.name,
        if plain.sex == Sex::Male { GRAN } else { GRANNY }
    );
    let mut with_epithets = family();
    with_epithets
        .get_mut(&20)
        .unwrap()
        .life
        .epithets
        .insert("the_wise".into(), date(740, 1, 1));
    with_epithets
        .get_mut(&20)
        .unwrap()
        .life
        .epithets
        .insert("the_blind".into(), date(741, 1, 1));
    let again = born(&lifecycle, &with_epithets, &[20], 1000);
    assert_eq!(
        again.name, plain.name,
        "the epithet text is no part of the namesake's name"
    );
}

#[test]
fn the_same_world_gives_the_same_names_and_a_save_keeps_them() {
    let lifecycle = kind("grandfather");
    let first = deliver(&lifecycle, &family(), &[20], 1003);
    let second = deliver(&lifecycle, &family(), &[20], 1003);
    assert_eq!(first, second);
    let store = store_of(first);
    assert_eq!(reloaded(&store), store);
}

#[test]
fn a_child_made_directly_is_named_by_the_same_rules_and_an_unknown_parent_has_no_full_siblings() {
    let lifecycle = small_pool(&["Aaa", "Bbb"], &["Aaa", "Bbb"]);
    let ctx = |tick: GameTime| Context {
        rules: &rules().genetics,
        personality: &rules().personality,
        events: quiet(),
        lifecycle: &lifecycle,
        seed: 3,
        tick,
    };
    let newborn = |sex: Sex, birth: GameTime| NewCharacter {
        name: String::new(),
        sex,
        birth,
        pop: locator("a"),
        house: String::new(),
    };
    // Siblings of the same father with an unknown mother are not full siblings: nothing is excluded, so both names are drawn.
    let env = Env::new("direct");
    let mut seen = BTreeSet::new();
    for next in 100..140 {
        let mut store = store_with_next(
            vec![named(2, Sex::Male, "Zfather"), {
                let mut c = child_of(50, "Aaa", 2, UNKNOWN);
                c.birth = date(765, 1, 1);
                c
            }],
            next,
        );
        let id = store
            .state_create_child(
                &ctx(date(768, 1, 1)),
                newborn(Sex::Male, date(768, 1, 1)),
                &irish(),
                &Lineage {
                    father: 2,
                    mother: UNKNOWN,
                    archive: &env.archive,
                },
                &PAGAN,
            )
            .expect("child");
        seen.insert(get(&store, id).name.clone());
    }
    assert_eq!(seen, BTreeSet::from(["Aaa".to_owned(), "Bbb".to_owned()]));
    // Both parents known: the living full sibling's name is out.
    for next in 100..120 {
        let mut store = store_with_next(
            vec![
                named(2, Sex::Male, "Zfather"),
                named(1, Sex::Female, "Zmother"),
                child_of(50, "Aaa", 2, 1),
            ],
            next,
        );
        let id = store
            .state_create_child(
                &ctx(date(768, 1, 1)),
                newborn(Sex::Male, date(768, 1, 1)),
                &irish(),
                &Lineage {
                    father: 2,
                    mother: 1,
                    archive: &env.archive,
                },
                &PAGAN,
            )
            .expect("child");
        assert_eq!(get(&store, id).name, "Bbb");
    }
    // A name the caller gives is kept: no namesake, no exclusion.
    let mut store = store_with_next(
        vec![
            named(2, Sex::Male, "Zfather"),
            named(1, Sex::Female, "Zmother"),
            child_of(50, "Aaa", 2, 1),
        ],
        100,
    );
    let id = store
        .state_create_child(
            &ctx(date(768, 1, 1)),
            NewCharacter {
                name: "Aaa".into(),
                ..newborn(Sex::Male, date(768, 1, 1))
            },
            &irish(),
            &Lineage {
                father: 2,
                mother: 1,
                archive: &env.archive,
            },
            &PAGAN,
        )
        .expect("child");
    assert_eq!(get(&store, id).name, "Aaa");
}

#[test]
fn a_founder_takes_no_namesake_and_excludes_nothing() {
    let lifecycle = kind("father");
    let ctx = Context {
        rules: &rules().genetics,
        personality: &rules().personality,
        events: quiet(),
        lifecycle: &lifecycle,
        seed: 11,
        tick: date(768, 1, 1),
    };
    let mut store = store_of(vec![named(1, Sex::Male, "Zfather")]);
    let id = store
        .create_founder(
            &ctx,
            NewCharacter {
                name: String::new(),
                sex: Sex::Male,
                birth: date(740, 1, 1),
                pop: locator("a"),
                house: String::new(),
            },
            &irish(),
            &PAGAN,
        )
        .expect("founder");
    assert!(none_is_a_relative(&[get(&store, id).name.clone()]));
}

// --- Review 1 (CK-365): posthumous brothers and the hour of a death ------------------------------------------------------------

/// A world for the namesake of a son (or daughter) of father 2 whose own father 20 is dead (archived, the list of his children frozen
/// at 2, 22 and 23), when a half-brother `Zposthumous` of the father is born to him afterwards through the creation path with an
/// unknown mother and then dies: his record names 20 as father and no list names him.
fn with_a_posthumous_half_brother(next: u64) -> Character {
    let lifecycle = kind("uncle");
    let family = family();
    let mut env = Env::new("posthumous");
    let mut live = Vec::new();
    for (id, c) in &family {
        if *id == 20 {
            let mut c = c.clone();
            c.death = Some(date(760, 1, 1));
            env.archive.append_record(&c);
        } else {
            let mut c = c.clone();
            // The father's brother and sister of the family are alive here: only the posthumous one is dead.
            if *id == 22 || *id == 23 {
                c.father = 20;
            }
            live.push(c);
        }
    }
    let mut store = store_with_next(live, next);
    let ctx = Context {
        rules: &rules().genetics,
        personality: &rules().personality,
        events: quiet(),
        lifecycle: &lifecycle,
        seed: 5,
        tick: date(761, 3, 3),
    };
    let half_brother = store
        .state_create_child(
            &ctx,
            NewCharacter {
                name: "Zposthumous".into(),
                sex: Sex::Male,
                birth: date(761, 3, 3),
                pop: locator("a"),
                house: String::new(),
            },
            &irish(),
            &Lineage {
                father: 20,
                mother: UNKNOWN,
                archive: &env.archive,
            },
            &PAGAN,
        )
        .expect("a child born after its father died");
    assert_eq!(half_brother, next);
    // The father's father is archived: his child list is not changed by the birth.
    let grandfather = env.archive.get(20).expect("read").expect("archived");
    assert_eq!(grandfather.children, [2, 22, 23]);
    store
        .die_with_events(
            &mut world(&lifecycle, &mut env, &PAGAN),
            half_brother,
            date(765, 1, 1),
            "fever",
        )
        .expect("death");
    store
        .run_deliveries(&mut world(&lifecycle, &mut env, &PAGAN), date(768, 10, 1))
        .expect("delivery");
    store
        .iter()
        .find(|c| c.mother == 1)
        .expect("the delivered child")
        .clone()
}

#[test]
fn a_dead_half_brother_born_after_the_shared_parent_was_archived_is_an_uncle() {
    let mut sons = 0;
    for next in 1000..1016 {
        let child = with_a_posthumous_half_brother(next);
        if child.sex == Sex::Male {
            sons += 1;
            assert_eq!(child.name, "Zposthumous", "named for the dead half-brother");
        } else {
            // A daughter's side is the mother's: no uncle there.
            assert!(none_is_a_relative(&[child.name]));
        }
    }
    assert!(sons >= 3, "{sons} sons");
}

#[test]
fn a_father_who_dies_later_on_the_day_of_the_birth_is_not_a_namesake() {
    let lifecycle = kind("father");
    let env = Env::new("hour");
    let born_after_death = |died: GameTime| {
        let mut father = named(2, Sex::Male, "Zfather");
        father.death = Some(died);
        let mut archive = Env::new("hour-archive");
        archive.archive.append_record(&father);
        let mut store = store_with_next(vec![named(1, Sex::Female, "Zmother")], 100);
        let ctx = Context {
            rules: &rules().genetics,
            personality: &rules().personality,
            events: quiet(),
            lifecycle: &lifecycle,
            seed: 3,
            tick: at(768, 1, 1, 6),
        };
        let id = store
            .state_create_child(
                &ctx,
                NewCharacter {
                    name: String::new(),
                    sex: Sex::Male,
                    birth: at(768, 1, 1, 6),
                    pop: locator("a"),
                    house: String::new(),
                },
                &irish(),
                &Lineage {
                    father: 2,
                    mother: 1,
                    archive: &archive.archive,
                },
                &PAGAN,
            )
            .expect("child");
        get(&store, id).name.clone()
    };
    drop(env);
    // Dead before the birth and at the very tick of it (a death that runs before the delivery of the same tick): a namesake.
    assert_eq!(born_after_death(at(768, 1, 1, 5)), "Zfather");
    assert_eq!(born_after_death(at(768, 1, 1, 6)), "Zfather");
    assert_eq!(born_after_death(date(767, 12, 31)), "Zfather");
    // Dead later the same day, or any later day: not yet dead at the birth.
    assert_ne!(born_after_death(at(768, 1, 1, 18)), "Zfather");
    assert_ne!(born_after_death(date(768, 1, 2)), "Zfather");
}

#[test]
fn the_archive_finds_children_by_recorded_parent_and_rebuilds_that_index_on_opening() {
    use ck_characters::ArchiveSink;
    let path = unique("children-of");
    let mut archive = Archive::open(&path).expect("archive");
    let dead = |id: u64, father: u64, mother: u64| {
        let mut c = named(id, Sex::Male, "Zdead");
        c.father = father;
        c.mother = mother;
        c.death = Some(date(760, 1, 1));
        c
    };
    for record in [
        dead(5, 20, 21),
        dead(6, 20, UNKNOWN),
        dead(7, UNKNOWN, 21),
        dead(8, UNKNOWN, UNKNOWN),
    ] {
        archive.append(&record).expect("append");
    }
    let check = |archive: &Archive| {
        assert_eq!(archive.children_of(20).unwrap(), [5, 6]);
        assert_eq!(archive.children_of(21).unwrap(), [5, 7]);
        assert_eq!(archive.children_of(99).unwrap(), Vec::<u64>::new());
        // The unknown character is nobody's parent.
        assert_eq!(archive.children_of(UNKNOWN).unwrap(), Vec::<u64>::new());
    };
    check(&archive);
    check(&Archive::open(&path).expect("reopened"));
}

#[test]
fn an_uncle_or_an_ancestor_who_dies_later_on_the_day_of_the_birth_is_not_a_namesake() {
    // The father 2 (a son of the live 20) has an archived brother 22 (an uncle) and an archived grandfather's father 40, a
    // great-grandfather: `ancestor` reaches 40 through 20. The child is born at 06:00; the relative's death is before, at or after.
    let named_at = |which: &str, died: GameTime| {
        let lifecycle = kind(which);
        let mut archive = Env::new("hour-relatives");
        let mut father = named(2, Sex::Male, "Zfather");
        father.father = 20;
        let mut grandfather = named(20, Sex::Male, "Zgrand");
        grandfather.father = 40;
        let mut uncle = named(22, Sex::Male, UNCLE);
        uncle.father = 20;
        uncle.death = Some(died);
        let mut great = named(40, Sex::Male, ANC_FF);
        great.death = Some(died);
        archive
            .archive
            .append_record(&if which == "uncle" { uncle } else { great });
        let mut store = store_with_next(
            vec![named(1, Sex::Female, "Zmother"), father, grandfather],
            100,
        );
        let ctx = Context {
            rules: &rules().genetics,
            personality: &rules().personality,
            events: quiet(),
            lifecycle: &lifecycle,
            seed: 3,
            tick: at(768, 1, 1, 6),
        };
        let id = store
            .state_create_child(
                &ctx,
                NewCharacter {
                    name: String::new(),
                    sex: Sex::Male,
                    birth: at(768, 1, 1, 6),
                    pop: locator("a"),
                    house: String::new(),
                },
                &irish(),
                &Lineage {
                    father: 2,
                    mother: 1,
                    archive: &archive.archive,
                },
                &PAGAN,
            )
            .expect("child");
        get(&store, id).name.clone()
    };
    for (which, relative) in [("uncle", UNCLE), ("ancestor", ANC_FF)] {
        assert_eq!(
            named_at(which, at(768, 1, 1, 5)),
            relative,
            "{which} died before"
        );
        assert_eq!(
            named_at(which, at(768, 1, 1, 6)),
            relative,
            "{which} died at the tick"
        );
        assert_eq!(
            named_at(which, date(767, 6, 1)),
            relative,
            "{which} died long before"
        );
        assert_ne!(
            named_at(which, at(768, 1, 1, 18)),
            relative,
            "{which} died later that day"
        );
        assert_ne!(
            named_at(which, date(768, 1, 2)),
            relative,
            "{which} died the next day"
        );
    }
}
