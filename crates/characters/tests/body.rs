//! Upbringing, height and weight (CK-367, characters slice 5d).
//!
//! Characters whose exact state a case needs are built as records and loaded through the canonical encoding; founders are made through
//! the store where the case is about creation. The events are none (a quiet world) except where a case needs one, so a roll cannot hide
//! the behaviour under test.

use ck_characters::{
    Archive, Character, CharacterStore, Context, GeneticRecord, LifeLog, NewCharacter,
    PersonalityRecord, PopLocator, Populations, Sex, UNKNOWN, World,
    body::{height_offset, weight_target},
};
use ck_economy::{EventRules, LifeCycle, PopGenetics, Rules, events::Registry};
use ck_sim::{Canonical, Date, GameTime};
use std::{cell::RefCell, collections::BTreeMap, path::PathBuf, sync::OnceLock};

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
fn events_of(text: &str) -> EventRules {
    let pop_types: Vec<&str> = rules().pop_types.iter().map(|p| p.id.as_str()).collect();
    EventRules::parse(
        "character-events.toml",
        text,
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
    .expect("an events file")
}
/// No events at all.
fn quiet() -> &'static EventRules {
    static QUIET: OnceLock<EventRules> = OnceLock::new();
    QUIET.get_or_init(|| events_of(""))
}
fn irish() -> PopGenetics {
    rules().genetics.seed_pop("ulaid").expect("Irish start set")
}
/// The shipped life-cycle data with `change` applied, kept for the length of the test run.
fn lifecycle_with(change: impl FnOnce(&mut LifeCycle)) -> &'static LifeCycle {
    let mut lifecycle = rules().lifecycle.clone();
    change(&mut lifecycle);
    Box::leak(Box::new(lifecycle))
}
/// The shipped data with the upbringing's noise taken away: only the pull is left to see.
fn no_noise() -> &'static LifeCycle {
    lifecycle_with(|l| {
        l.upbringing.noise_vice_virtue = 0;
        l.upbringing.noise_manner = 0;
    })
}

/// The pops: every pop is Irish (`ulaid`) of type `kind`; each has the necessities `food` gives it (1000 when it gives none) and the
/// luxuries `luxuries`.
struct Pops {
    kind: &'static str,
    luxuries: u16,
    food: RefCell<BTreeMap<String, u16>>,
}
impl Pops {
    fn new() -> Self {
        Self {
            kind: "noble",
            luxuries: 600,
            food: RefCell::new(BTreeMap::new()),
        }
    }
    fn feed(&self, place: &str, necessities: u16) {
        self.food.borrow_mut().insert(place.to_owned(), necessities);
    }
}
impl Populations for Pops {
    fn heads(&self, _: &PopLocator) -> Option<u64> {
        Some(1_000_000)
    }
    fn culture(&self, _: &PopLocator) -> Option<String> {
        Some("ulaid".to_owned())
    }
    fn kind(&self, _: &PopLocator) -> Option<String> {
        Some(self.kind.to_owned())
    }
    fn luxuries_pm(&self, _: &PopLocator) -> Option<u16> {
        Some(self.luxuries)
    }
    fn religion(&self, _: &PopLocator) -> Option<String> {
        Some(String::new())
    }
    fn necessities_pm(&self, locator: &PopLocator) -> Option<u16> {
        Some(
            self.food
                .borrow()
                .get(&locator.place)
                .copied()
                .unwrap_or(1000),
        )
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

fn path(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("ck-body-{}-{tag}", std::process::id()));
    std::fs::remove_file(&path).ok();
    path
}
/// An archive and a life log of one test.
struct Env {
    archive: Archive,
    logs: LifeLog,
}
impl Env {
    fn new(tag: &str) -> Self {
        Self {
            archive: Archive::open(path(&format!("{tag}-arch"))).expect("archive"),
            logs: LifeLog::open(path(&format!("{tag}-logs"))).expect("logs"),
        }
    }
    fn lines(&self, id: u64) -> Vec<String> {
        self.logs
            .read(id)
            .expect("log")
            .into_iter()
            .map(|line| line.text)
            .collect()
    }
}
fn world<'a>(
    events: &'a EventRules,
    lifecycle: &'a LifeCycle,
    env: &'a mut Env,
    pops: &'a Pops,
) -> World<'a, Pops, Archive, LifeLog> {
    World {
        rules: events,
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle,
        seed: 7,
        populations: pops,
        archive: &mut env.archive,
        logs: &mut env.logs,
    }
}

/// A record with ordinary stats (a man of 70 kg and 175 cm), humours of 5, in pop `place`, whose weight is its base weight.
fn person(id: u64, birth: GameTime, place: &str, sex: Sex) -> Character {
    let mut genetics = GeneticRecord::default();
    genetics.value[9..13].copy_from_slice(&[5000; 4]);
    for stat in 0..9 {
        genetics.value[stat] = 10_000;
    }
    genetics.value[0] = 175_000;
    genetics.value[1] = 70_000;
    let mut life = ck_characters::LifeRecord::default();
    "irish".clone_into(&mut life.pool);
    let mut character = Character {
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
        ties: Box::default(),
    };
    character.life.weight = character.base_weight(&rules().genetics);
    character
}
fn store_of(characters: Vec<Character>) -> CharacterStore {
    let next = characters.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    let live: BTreeMap<u64, Character> = characters.into_iter().map(|c| (c.id, c)).collect();
    let mut bytes = Vec::new();
    next.encode(&mut bytes).expect("encode");
    live.encode(&mut bytes).expect("encode");
    0_u64.encode(&mut bytes).expect("encode");
    CharacterStore::from_bytes(&bytes).expect("a store of these records")
}
fn get(store: &CharacterStore, id: u64) -> &Character {
    store.get(id).expect("live")
}

/// A founder of `sex` born on `birth` and created on `created`, in pop `place`.
fn founder(
    store: &mut CharacterStore,
    pops: &Pops,
    sex: Sex,
    place: &str,
    birth: GameTime,
    created: GameTime,
) -> u64 {
    let ctx = Context {
        rules: &rules().genetics,
        personality: &rules().personality,
        events: quiet(),
        lifecycle: &rules().lifecycle,
        seed: 11,
        tick: created,
    };
    store
        .create_founder(
            &ctx,
            NewCharacter {
                name: String::new(),
                sex,
                birth,
                pop: locator(place),
                house: String::new(),
            },
            &irish(),
            pops,
        )
        .expect("a founder")
}

/// The birthday pass of every birthday of a character born on 5 March `born`, from the one after `from` through `to` inclusive.
fn birthdays(store: &mut CharacterStore, pops: &Pops, tag: &str, born: u16, from: u16, to: u16) {
    let mut env = Env::new(tag);
    for year in (born + from + 1)..=(born + to) {
        store
            .run_birthdays(
                &mut world(quiet(), &rules().lifecycle, &mut env, pops),
                date(year, 3, 5),
            )
            .expect("birthdays");
    }
}

/// The nutrition offset by an independent route: exact integers, half away from zero, one rounding.
fn expected_offset(sum: i128, count: i128, age: i128) -> i64 {
    let denominator = 1000 * count;
    let adult =
        (30 * (1000 * sum - 900_000 * count)).clamp(-8000 * denominator, 2000 * denominator);
    let (numerator, scale) = (adult * age.min(18), denominator * 18);
    let half = scale / 2;
    let rounded = if numerator >= 0 {
        (numerator + half) / scale
    } else {
        -((-numerator + half) / scale)
    };
    i64::try_from(rounded).expect("fits")
}

// ======================================================================================================================
// Upbringing
// ======================================================================================================================

/// A pass on the day of the birthday of `child` (born 790-6-1 in a world of 800-6-1) with the household's members loaded as they stand.
fn brought_up(
    lifecycle: &'static LifeCycle,
    tag: &str,
    members: Vec<Character>,
    events: &EventRules,
    on: GameTime,
) -> (CharacterStore, Env) {
    let mut store = store_of(members);
    let mut env = Env::new(tag);
    store
        .run_birthdays(&mut world(events, lifecycle, &mut env, &Pops::new()), on)
        .expect("birthdays");
    (store, env)
}

/// A household: a father (1) and a mother (2), a sibling (4) and the child (3) turning `age` on 800-6-1, none of the others on their
/// birthday. The values are the first vice/virtue pair and the first manner pair.
fn household(
    age: u16,
    child: (i64, i64),
    parents: [(i64, i64); 2],
    sibling: (i64, i64),
) -> Vec<Character> {
    let mut members = Vec::new();
    for (id, birth, values, sex) in [
        (1, date(760, 1, 1), parents[0], Sex::Male),
        (2, date(762, 1, 1), parents[1], Sex::Female),
        (3, date(800 - age, 6, 1), child, Sex::Male),
        (4, date(795, 1, 1), sibling, Sex::Female),
    ] {
        let mut c = person(id, birth, "a", sex);
        c.personality.vice[0] = values.0;
        c.personality.manner[0] = values.1;
        if id >= 3 {
            (c.father, c.mother) = (1, 2);
        }
        members.push(c);
    }
    members
}

#[test]
fn a_worked_yearly_pull_toward_a_two_parent_one_sibling_household() {
    // Household average: vice (3000 - 1000 + 1000) / 3 = 1000, manner (10000 + 0 + 5000) / 3 = 5000. The child at 5000 and 0 moves 4%
    // of the way: 5000 + 0.04 (1000 - 5000) = 4840 and 0 + 0.04 (5000 - 0) = 200. With no noise nothing else moves it.
    let members = household(10, (5000, 0), [(3000, 10_000), (-1000, 0)], (1000, 5000));
    let (store, _) = brought_up(no_noise(), "worked", members, quiet(), date(800, 6, 1));
    let child = get(&store, 3);
    assert_eq!(child.personality.vice[0], 4840);
    assert_eq!(child.personality.manner[0], 200);
    // The other pairs are all 0 in a household of 0: nothing to move.
    assert_eq!(child.personality.vice[1..], [0; 6]);
    assert_eq!(child.personality.manner[1..], [0; 8]);
    // Only the child had a birthday: the household's own values stand.
    assert_eq!(get(&store, 1).personality.vice[0], 3000);
    assert_eq!(get(&store, 4).personality.manner[0], 5000);
}

#[test]
fn the_pull_is_exact_and_rounded_once_to_the_thousandth() {
    // A household of both parents at one value (the sibling taken away): a child at 0 toward 1 moves 0.04 thousandths, which is 0; toward
    // 13 it moves 0.52, which rounds to 1; a child at 13 toward 0 reaches 12.48, which is 12; at -13 toward 0 it reaches -12.48, which
    // is -12; and at 0 toward -13 it moves -0.52, which rounds away from zero to -1.
    for (child, parent, want) in [
        (0, 1, 0),
        (0, 13, 1),
        (13, 0, 12),
        (-13, 0, -12),
        (0, -13, -1),
    ] {
        let mut members = household(10, (child, 0), [(parent, 0), (parent, 0)], (parent, 0));
        members.remove(3);
        let (store, _) = brought_up(no_noise(), "rounded", members, quiet(), date(800, 6, 1));
        assert_eq!(
            get(&store, 3).personality.vice[0],
            want,
            "child {child} toward {parent}"
        );
    }
}

#[test]
fn a_child_with_no_household_takes_only_the_noise() {
    // No parent and no sibling in the store: with no noise nothing moves, and with the shipped noise the values are drawn.
    let mut child = person(3, date(790, 6, 1), "a", Sex::Male);
    child.personality.vice[0] = 5000;
    child.personality.manner[0] = -3000;
    let (store, _) = brought_up(
        no_noise(),
        "alone-still",
        vec![child.clone()],
        quiet(),
        date(800, 6, 1),
    );
    assert_eq!(get(&store, 3).personality, child.personality);
    let (store, _) = brought_up(
        &rules().lifecycle,
        "alone-drawn",
        vec![child.clone()],
        quiet(),
        date(800, 6, 1),
    );
    assert_ne!(get(&store, 3).personality.vice, child.personality.vice);
    // Parents who are not live (here: recorded but absent) are no household either.
    let mut orphan = child.clone();
    (orphan.father, orphan.mother) = (1, 2);
    let (store, _) = brought_up(no_noise(), "orphan", vec![orphan], quiet(), date(800, 6, 1));
    assert_eq!(get(&store, 3).personality, child.personality);
}

#[test]
fn the_noise_has_the_spread_of_the_data_over_a_large_sample() {
    // 400 children with no household and every value at 0 turn 10 on one day: each value is one draw of the noise, so the sample's
    // spread is the data's (0.48 for a vice/virtue, 1.28 for a manner), its mean 0. The seed is fixed, so the result is exact.
    let children: Vec<Character> = (1..=400)
        .map(|id| person(id, date(790, 6, 1), "a", Sex::Male))
        .collect();
    let (store, _) = brought_up(
        &rules().lifecycle,
        "spread",
        children,
        quiet(),
        date(800, 6, 1),
    );
    let (mut vice, mut manner) = (Vec::new(), Vec::new());
    for c in store.iter() {
        vice.extend(c.personality.vice);
        manner.extend(c.personality.manner);
    }
    for (name, sample, spread) in [("vice", &vice, 480_i128), ("manner", &manner, 1280)] {
        let n = i128::try_from(sample.len()).expect("a count");
        let total: i128 = sample.iter().map(|v| i128::from(*v)).sum();
        // Squares about the mean, scaled by n^2 to stay in integers: n sum(v^2) - (sum v)^2 = n^2 variance.
        let squares: i128 = sample.iter().map(|v| i128::from(*v) * i128::from(*v)).sum();
        let variance_n2 = n * squares - total * total;
        let expected_n2 = n * n * spread * spread;
        assert!(
            variance_n2 * 100 > expected_n2 * 88 && variance_n2 * 100 < expected_n2 * 112,
            "{name}: variance {variance_n2} against {expected_n2} (n^2 x spread^2)"
        );
        assert!(
            total.abs() * 100 < n * spread * 6,
            "{name}: mean {total} / {n}"
        );
    }
}

#[test]
fn vice_and_virtue_are_held_to_seven_and_manner_is_not_bounded() {
    // A noise 50 points wide throws values far past 7: a vice/virtue stays within -7..=7 (and reaches the bound), a manner does not.
    let wide = lifecycle_with(|l| {
        l.upbringing.noise_vice_virtue = 50_000;
        l.upbringing.noise_manner = 50_000;
    });
    let children: Vec<Character> = (1..=100)
        .map(|id| person(id, date(790, 6, 1), "a", Sex::Male))
        .collect();
    let (store, _) = brought_up(wide, "clamp", children, quiet(), date(800, 6, 1));
    let vice: Vec<i64> = store.iter().flat_map(|c| c.personality.vice).collect();
    let manner: Vec<i64> = store.iter().flat_map(|c| c.personality.manner).collect();
    assert!(vice.iter().all(|v| (-7000..=7000).contains(v)));
    assert!(
        vice.contains(&7000) && vice.contains(&-7000),
        "both bounds reached"
    );
    assert!(manner.iter().any(|v| v.abs() > 7000), "a manner passes 7");
}

#[test]
fn upbringing_stops_at_to_age() {
    // `to_age` is 14: the birthday on which a character turns 13 brings it up, the one on which it turns 14 does not.
    for (age, moves) in [(13, true), (14, false), (20, false)] {
        let members = household(age, (5000, 0), [(0, 0), (0, 0)], (0, 0));
        let (store, _) = brought_up(no_noise(), "stops", members, quiet(), date(800, 6, 1));
        let vice = get(&store, 3).personality.vice[0];
        assert_eq!(vice != 5000, moves, "turning {age}: {vice}");
    }
}

#[test]
fn a_relative_in_the_snapshot_who_dies_earlier_that_day_still_counts() {
    // The father (id 1, 40) has his birthday too and a yearly event kills him in his own pass, before the child's (id 3). The child's
    // household is the snapshot taken before the pass, so he still counts: the father at 10,000 and the mother at 0 average 5000, and
    // the child at 0 moves 0.04 of the way to 200.
    let fatal = events_of(
        r#"
[[event]]
id = "fatal"
trigger = "yearly"
chance = 1
effects = [{ die = "fever" }]
text = "{name} died of a fever."
"#,
    );
    let mut members = household(10, (0, 0), [(10_000, 0), (0, 0)], (0, 0));
    members.remove(3);
    members[0].birth = date(760, 6, 1);
    members[1].birth = date(762, 1, 1);
    let (store, env) = brought_up(no_noise(), "dies", members, &fatal, date(800, 6, 1));
    assert!(store.get(1).is_none(), "the father died that day");
    assert_eq!(env.lines(1).len(), 1);
    assert_eq!(get(&store, 3).personality.vice[0], 200, "he still counted");
}

#[test]
fn an_upbringing_change_that_invalidates_a_calling_is_removed_that_day() {
    // An ascetic (a monk's calling) falls when his gluttony or lust passes 0. A monk child at 0 whose household sits at -4000 is pulled
    // to -160 by upbringing, and the same day's precedence removes the calling, with its line. With a household at 0 he keeps it.
    let monastic = || {
        let mut pops = Pops::new();
        pops.kind = "monk";
        pops
    };
    for (household_value, kept) in [(-4000, false), (0, true)] {
        let mut members = household(
            10,
            (0, 0),
            [(household_value, 0), (household_value, 0)],
            (household_value, 0),
        );
        for c in &mut members {
            c.personality.vice[4] = c.personality.vice[0];
            c.personality.vice[0] = 0;
        }
        members[2].life.grades.insert("ascetic".to_owned(), 1);
        let mut store = store_of(members);
        let mut env = Env::new("calling");
        store
            .run_birthdays(
                &mut world(quiet(), no_noise(), &mut env, &monastic()),
                date(800, 6, 1),
            )
            .expect("birthdays");
        let child = get(&store, 3);
        assert_eq!(
            child.grade("ascetic") > 0,
            kept,
            "household at {household_value}"
        );
        assert_eq!(
            env.lines(3).iter().any(|line| line == "c3 lost ascetic."),
            !kept
        );
    }
}

// ======================================================================================================================
// Height
// ======================================================================================================================

#[test]
fn born_at_800_then_1000_has_two_samples_a_mean_of_900_and_no_offset() {
    let (pops, mut store) = (Pops::new(), CharacterStore::default());
    pops.feed("a", 800);
    let id = founder(
        &mut store,
        &pops,
        Sex::Male,
        "a",
        date(780, 3, 5),
        date(780, 3, 5),
    );
    let c = get(&store, id);
    assert_eq!(
        (c.life.food_count, c.life.food_sum, c.life.height_offset),
        (1, 800, 0)
    );
    pops.feed("a", 1000);
    birthdays(&mut store, &pops, "two", 780, 0, 1);
    let c = get(&store, id);
    assert_eq!((c.life.food_count, c.life.food_sum), (2, 1800));
    // The mean is 900, the reference: no offset now, and none at adulthood if nothing else is sampled.
    assert_eq!(c.life.height_offset, 0);
    assert_eq!(
        height_offset(&rules().lifecycle.nutrition, 2, 1800, 18),
        Ok(0)
    );
}

#[test]
fn the_offset_of_age_7_with_eight_samples_of_7201_is_one_thousandth_not_two() {
    // Mean 900.125: the adult offset is 0.03 x 0.125 cm = 3.75 thousandths; x 7/18 is 1.458, which rounds once to 1 (rounding the
    // adult offset first, to 4, would give 1.56, which is 2).
    let nutrition = &rules().lifecycle.nutrition;
    assert_eq!(height_offset(nutrition, 8, 7201, 7), Ok(1));
    assert_eq!(expected_offset(7201, 8, 7), 1);
    // And through a character: birth at 901 and seven birthdays at 900 give 8 samples summing 7201 at age 7.
    let (pops, mut store) = (Pops::new(), CharacterStore::default());
    pops.feed("a", 901);
    let id = founder(
        &mut store,
        &pops,
        Sex::Male,
        "a",
        date(780, 3, 5),
        date(780, 3, 5),
    );
    pops.feed("a", 900);
    birthdays(&mut store, &pops, "seven", 780, 0, 7);
    let c = get(&store, id);
    assert_eq!((c.life.food_count, c.life.food_sum), (8, 7201));
    assert_eq!(c.life.height_offset, 1);
}

#[test]
fn a_varying_childhood_with_a_pop_move_gives_the_mean_based_offset() {
    // 700 in pop a to the 9th birthday, then a move to pop b at 1000: samples at ages 0..=9 are 700 (10 of them), at 10..=18 are 1000.
    let (pops, mut store) = (Pops::new(), CharacterStore::default());
    pops.feed("a", 700);
    pops.feed("b", 1000);
    let id = founder(
        &mut store,
        &pops,
        Sex::Male,
        "a",
        date(780, 3, 5),
        date(780, 3, 5),
    );
    birthdays(&mut store, &pops, "move-a", 780, 0, 9);
    assert_eq!(get(&store, id).life.food_count, 10);
    let mut env = Env::new("move");
    store
        .move_to_with_events(
            &mut world(quiet(), &rules().lifecycle, &mut env, &pops),
            id,
            locator("b"),
            date(789, 4, 1),
        )
        .expect("moved");
    birthdays(&mut store, &pops, "move-b", 780, 9, 18);
    let c = get(&store, id);
    assert_eq!((c.life.food_count, c.life.food_sum), (19, 7000 + 9000));
    // 0.03 x (16000 / 19 - 900) = -1.7368 cm: -1737 thousandths.
    assert_eq!(c.life.height_offset, -1737);
    assert_eq!(c.life.height_offset, expected_offset(16_000, 19, 18));
}

#[test]
fn the_offset_is_capped_at_plus_2_and_minus_8_centimetres() {
    for (food, adult, half) in [
        (1000_u16, 2000_i64, 1000_i64),
        (0, -8000, -4000),
        (1400, 2000, 1000),
    ] {
        let (pops, mut store) = (Pops::new(), CharacterStore::default());
        pops.feed("a", food);
        let id = founder(
            &mut store,
            &pops,
            Sex::Male,
            "a",
            date(780, 3, 5),
            date(780, 3, 5),
        );
        birthdays(&mut store, &pops, "cap-9", 780, 0, 9);
        assert_eq!(get(&store, id).life.height_offset, half, "food {food} at 9");
        birthdays(&mut store, &pops, "cap-18", 780, 9, 18);
        assert_eq!(
            get(&store, id).life.height_offset,
            adult,
            "food {food} at 18"
        );
    }
}

#[test]
fn the_18th_birthdays_sample_is_the_last_and_the_offset_is_fixed_after() {
    let (pops, mut store) = (Pops::new(), CharacterStore::default());
    pops.feed("a", 500);
    let id = founder(
        &mut store,
        &pops,
        Sex::Male,
        "a",
        date(780, 3, 5),
        date(780, 3, 5),
    );
    birthdays(&mut store, &pops, "last-17", 780, 0, 17);
    let before = get(&store, id).life.clone();
    assert_eq!(before.food_count, 18);
    // The 18th birthday adds its sample (up to 19 in all) and fixes the offset; later birthdays, even at a different food, change nothing.
    pops.feed("a", 1000);
    birthdays(&mut store, &pops, "last-18", 780, 17, 18);
    let fixed = get(&store, id).life.clone();
    assert_eq!((fixed.food_count, fixed.food_sum), (19, 18 * 500 + 1000));
    assert_eq!(fixed.height_offset, expected_offset(10_000, 19, 18));
    pops.feed("a", 0);
    birthdays(&mut store, &pops, "last-30", 780, 18, 30);
    let later = get(&store, id).life.clone();
    assert_eq!(
        (later.food_count, later.food_sum, later.height_offset),
        (fixed.food_count, fixed.food_sum, fixed.height_offset)
    );
}

#[test]
fn a_founder_created_at_10_and_one_created_at_20() {
    let (pops, mut store) = (Pops::new(), CharacterStore::default());
    pops.feed("a", 600);
    let at_10 = founder(
        &mut store,
        &pops,
        Sex::Male,
        "a",
        date(780, 3, 5),
        date(790, 3, 5),
    );
    let at_20 = founder(
        &mut store,
        &pops,
        Sex::Male,
        "a",
        date(770, 3, 5),
        date(790, 3, 5),
    );
    // One sample at creation. At 10 the offset is the adult one x 10/18; at 20 it is the adult one at once.
    let (young, old) = (
        get(&store, at_10).life.clone(),
        get(&store, at_20).life.clone(),
    );
    assert_eq!((young.food_count, young.food_sum), (1, 600));
    assert_eq!((old.food_count, old.food_sum), (1, 600));
    assert_eq!(young.height_offset, expected_offset(600, 1, 10));
    assert_eq!(old.height_offset, expected_offset(600, 1, 18));
    assert_eq!(old.height_offset, -8000, "0.03 x -300 = -9, capped at -8");
    // The one created at 10 goes on sampling to 18; the one created at 20 never does.
    pops.feed("a", 1000);
    birthdays(&mut store, &pops, "created-10", 780, 10, 18);
    birthdays(&mut store, &pops, "created-20", 770, 20, 25);
    let (young, old) = (
        get(&store, at_10).life.clone(),
        get(&store, at_20).life.clone(),
    );
    assert_eq!((young.food_count, young.food_sum), (9, 600 + 8 * 1000));
    assert_eq!(young.height_offset, expected_offset(8600, 9, 18));
    assert_eq!((old.food_count, old.height_offset), (1, -8000));
}

#[test]
fn a_woman_with_neutral_food_reads_her_base_height_across_adulthood_save_load_and_archive() {
    let (pops, mut store) = (Pops::new(), CharacterStore::default());
    pops.feed("a", 900);
    let id = founder(
        &mut store,
        &pops,
        Sex::Female,
        "a",
        date(780, 3, 5),
        date(780, 3, 5),
    );
    let genetics = &rules().genetics;
    let genetic = get(&store, id).genetics.value[0];
    assert_eq!(genetics.female_offset(0), -12_000);
    let base = genetic - 12_000;
    assert_eq!(get(&store, id).base_height(genetics), base);
    for (years, tag) in [(18, "w-18"), (30, "w-30"), (45, "w-45")] {
        birthdays(&mut store, &pops, tag, 780, 0, years);
        let c = get(&store, id);
        assert_eq!(c.life.height_offset, 0);
        assert_eq!(
            c.shown_height(genetics),
            base,
            "at {years}: not her genetic height"
        );
        assert_eq!(c.shown_stat(genetics, 0), base);
    }
    // Save and load: the record, its food record and its shown height come back as they were.
    let bytes = store.to_bytes().expect("encode");
    let loaded = CharacterStore::from_bytes(&bytes).expect("decode");
    assert_eq!(get(&loaded, id), get(&store, id));
    assert_eq!(get(&loaded, id).shown_height(genetics), base);
    assert_eq!(get(&loaded, id).life.food_count, 19);
    // Archive: she dies, and her archived record reads the same height and weight.
    let mut env = Env::new("w-archive");
    let shown_weight = get(&store, id).shown_weight();
    store
        .die_with_events(
            &mut world(quiet(), &rules().lifecycle, &mut env, &pops),
            id,
            date(830, 1, 1),
            "fever",
        )
        .expect("died");
    let archived = env.archive.get(id).expect("read").expect("archived");
    assert_eq!(archived.shown_height(genetics), base);
    assert_eq!(archived.base_height(genetics), base);
    assert_eq!(archived.shown_weight(), shown_weight);
    assert_eq!(archived.life.food_count, 19);
}

#[test]
fn shown_height_is_held_to_the_floor_once_and_last() {
    // A genetic 105 cm woman has a base of 93 (the -12 offset). With a +2 nutrition offset she is 95, which the floor of 100 holds: the
    // floor applies to the total, once, last. Flooring the base first would show 102.
    let genetics = &rules().genetics;
    let mut c = person(1, date(780, 3, 5), "a", Sex::Female);
    c.genetics.value[0] = 105_000;
    c.life.height_offset = 2000;
    assert_eq!(c.base_height(genetics), 93_000);
    assert_eq!(c.shown_height(genetics), 100_000);
    // Above the floor the total is shown as it is.
    c.genetics.value[0] = 115_000;
    assert_eq!(c.shown_height(genetics), 105_000);
}

// ======================================================================================================================
// Weight
// ======================================================================================================================

#[test]
fn a_worked_month_moves_an_exact_twelfth_by_integer_division() {
    // A man of 70 kg at necessities 1000 and luxuries 600: target 70,000 x (0.8 + 0.2 + 0.06) = 74,200. He shows 40,000: the step is
    // (74,200 - 40,000) / 12 = 2850 (a 0.0833 step would be 2848.86, so 2849).
    let pops = Pops::new();
    let mut c = person(1, date(760, 1, 1), "a", Sex::Male);
    c.life.weight = 40_000;
    let mut store = store_of(vec![c]);
    let mut env = Env::new("month");
    store
        .run_monthly_weights(&mut world(quiet(), &rules().lifecycle, &mut env, &pops))
        .expect("weights");
    assert_eq!(get(&store, 1).shown_weight(), 42_850);
    // A weight above the target steps down by the quotient truncated toward zero: 99,999 - 74,200 = 25,799, a twelfth is 2149.9.
    let mut heavy = person(1, date(760, 1, 1), "a", Sex::Male);
    heavy.life.weight = 99_999;
    let mut store = store_of(vec![heavy]);
    store
        .run_monthly_weights(&mut world(quiet(), &rules().lifecycle, &mut env, &pops))
        .expect("weights");
    assert_eq!(get(&store, 1).shown_weight(), 99_999 - 2149);
    // At the target it stays.
    let mut level = person(1, date(760, 1, 1), "a", Sex::Male);
    level.life.weight = 74_200;
    let mut store = store_of(vec![level]);
    store
        .run_monthly_weights(&mut world(quiet(), &rules().lifecycle, &mut env, &pops))
        .expect("weights");
    assert_eq!(get(&store, 1).shown_weight(), 74_200);
}

#[test]
fn a_womans_target_is_read_from_her_base_weight() {
    // Her base is 70,000 - 10,000 = 60,000: the target is 60,000 x 1.06 = 63,600, not 74,200.
    let nutrition = &rules().lifecycle.nutrition;
    let woman = person(1, date(760, 1, 1), "a", Sex::Female);
    let base = woman.base_weight(&rules().genetics);
    assert_eq!(base, 60_000);
    assert_eq!(weight_target(nutrition, base, 1000, 600, 0), Ok(63_600));
    let pops = Pops::new();
    let mut store = store_of(vec![woman]);
    let mut env = Env::new("woman-month");
    store
        .run_monthly_weights(&mut world(quiet(), &rules().lifecycle, &mut env, &pops))
        .expect("weights");
    // She shows her base weight (60,000): the step is (63,600 - 60,000) / 12 = 300.
    assert_eq!(get(&store, 1).shown_weight(), 60_300);
}

#[test]
fn gluttony_raises_the_target_and_temperance_does_not() {
    let nutrition = &rules().lifecycle.nutrition;
    // +1% per point of gluttony_temperance below 0: -5 points is x1.05 (63,600 x 1.05 = 66,780), +5 points changes nothing.
    assert_eq!(
        weight_target(nutrition, 60_000, 1000, 600, -5000),
        Ok(66_780)
    );
    assert_eq!(
        weight_target(nutrition, 60_000, 1000, 600, 5000),
        Ok(63_600)
    );
    assert_eq!(weight_target(nutrition, 60_000, 1000, 600, 0), Ok(63_600));
    // Less food and less luxury: 60,000 x (0.8 + 0.2 x 0.5 + 0.1 x 0) = 54,000.
    assert_eq!(weight_target(nutrition, 60_000, 500, 0, 0), Ok(54_000));
    // Through the pass: a glutton at -5 points and shown 60,000 steps (66,780 - 60,000) / 12 = 565.
    let pops = Pops::new();
    let mut glutton = person(1, date(760, 1, 1), "a", Sex::Female);
    glutton.personality.vice[4] = -5000;
    let mut store = store_of(vec![glutton]);
    let mut env = Env::new("glutton");
    store
        .run_monthly_weights(&mut world(quiet(), &rules().lifecycle, &mut env, &pops))
        .expect("weights");
    assert_eq!(get(&store, 1).shown_weight(), 60_565);
}

#[test]
fn the_weight_is_held_to_the_floor() {
    // A rolled 20 kg shows the floor of 30 at creation; the target of 21,200 pulls it down by a twelfth (-733), and the floor holds it.
    let pops = Pops::new();
    let mut light = person(1, date(760, 1, 1), "a", Sex::Male);
    light.genetics.value[1] = 20_000;
    light.life.weight = 30_000;
    let mut store = store_of(vec![light]);
    let mut env = Env::new("floor");
    store
        .run_monthly_weights(&mut world(quiet(), &rules().lifecycle, &mut env, &pops))
        .expect("weights");
    assert_eq!(get(&store, 1).shown_weight(), 30_000);
    // A new character starts at its base weight, floored.
    let mut founders = CharacterStore::default();
    let id = founder(
        &mut founders,
        &pops,
        Sex::Female,
        "a",
        date(780, 3, 5),
        date(780, 3, 5),
    );
    let c = get(&founders, id);
    assert_eq!(
        c.shown_weight(),
        c.base_weight(&rules().genetics).max(30_000)
    );
}

#[test]
fn a_pop_that_cannot_say_its_food_leaves_the_weights_as_they_were() {
    // A source with no necessities or luxuries for a pop invents none: its characters' weights stay as they were.
    struct Unknown(Pops);
    impl Populations for Unknown {
        fn heads(&self, l: &PopLocator) -> Option<u64> {
            self.0.heads(l)
        }
        fn culture(&self, l: &PopLocator) -> Option<String> {
            self.0.culture(l)
        }
        fn kind(&self, l: &PopLocator) -> Option<String> {
            self.0.kind(l)
        }
        fn distance_m(&self, a: &PopLocator, b: &PopLocator) -> Option<u64> {
            self.0.distance_m(a, b)
        }
        fn move_head(
            &self,
            from: &PopLocator,
            to: &PopLocator,
            minor: bool,
        ) -> Result<ck_economy::MoveUndo, ck_characters::Error> {
            self.0.move_head(from, to, minor)
        }
        fn restore_move(&self, undo: &ck_economy::MoveUndo) {
            self.0.restore_move(undo);
        }
    }
    let pops = Unknown(Pops::new());
    let mut c = person(1, date(760, 1, 1), "a", Sex::Male);
    c.life.weight = 40_000;
    let mut store = store_of(vec![c]);
    let mut env = Env::new("unknown-food");
    let mut world = World {
        rules: quiet(),
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle: &rules().lifecycle,
        seed: 7,
        populations: &pops,
        archive: &mut env.archive,
        logs: &mut env.logs,
    };
    store.run_monthly_weights(&mut world).expect("weights");
    assert_eq!(get(&store, 1).shown_weight(), 40_000);
}

// ======================================================================================================================
// Determinism and the records
// ======================================================================================================================

#[test]
fn a_childhood_is_deterministic_under_the_seed_and_its_record_round_trips() {
    let run = |tag: &str| {
        let (pops, mut store) = (Pops::new(), CharacterStore::default());
        pops.feed("a", 750);
        let id = founder(
            &mut store,
            &pops,
            Sex::Female,
            "a",
            date(780, 3, 5),
            date(780, 3, 5),
        );
        birthdays(&mut store, &pops, tag, 780, 0, 20);
        let mut env = Env::new(&format!("{tag}-m"));
        store
            .run_monthly_weights(&mut world(quiet(), &rules().lifecycle, &mut env, &pops))
            .expect("weights");
        (store.to_bytes().expect("encode"), id)
    };
    let (first, id) = run("det-a");
    let (second, _) = run("det-b");
    assert_eq!(first, second, "the same seed gives the same bytes");
    let loaded = CharacterStore::from_bytes(&first).expect("decode");
    assert_eq!(loaded.to_bytes().expect("encode"), first);
    let c = get(&loaded, id);
    assert_eq!(c.life.food_count, 19);
    assert!(c.life.height_offset < 0 && c.life.height_offset >= -8000);
    assert!(c.shown_weight() >= 30_000);
}

#[test]
fn a_child_born_into_the_store_starts_its_food_record_at_birth() {
    // `create_child` takes the first sample at birth, and the child's weight is its base weight.
    let (pops, mut store) = (Pops::new(), CharacterStore::default());
    pops.feed("a", 850);
    let father = founder(
        &mut store,
        &pops,
        Sex::Male,
        "a",
        date(760, 3, 5),
        date(780, 3, 5),
    );
    let mother = founder(
        &mut store,
        &pops,
        Sex::Female,
        "a",
        date(762, 3, 5),
        date(780, 3, 5),
    );
    let archive = Archive::open(path("born-arch")).expect("archive");
    let ctx = Context {
        rules: &rules().genetics,
        personality: &rules().personality,
        events: quiet(),
        lifecycle: &rules().lifecycle,
        seed: 3,
        tick: date(781, 1, 1),
    };
    let mut env = Env::new("born");
    let child = store
        .create_child_with_events(
            &mut world(quiet(), &rules().lifecycle, &mut env, &pops),
            &ctx,
            NewCharacter {
                name: String::new(),
                sex: Sex::Female,
                birth: date(781, 1, 1),
                pop: locator("a"),
                house: String::new(),
            },
            &irish(),
            &ck_characters::Lineage {
                father,
                mother,
                archive: &archive,
            },
        )
        .expect("a child");
    let c = get(&store, child);
    assert_eq!(
        (c.life.food_count, c.life.food_sum, c.life.food_age),
        (1, 850, 0)
    );
    assert_eq!(c.life.height_offset, 0);
    assert_eq!(
        c.shown_weight(),
        c.base_weight(&rules().genetics).max(30_000)
    );
    assert_eq!(
        c.shown_height(&rules().genetics),
        c.base_height(&rules().genetics).max(100_000)
    );
}

#[test]
fn the_birth_day_sample_is_replaced_once_after_the_step_and_only_for_the_days_newborns() {
    // A child created before the step holds the value it saw; the phase C pass replaces it with the day's, keeping one sample, and
    // leaves a character born another day alone.
    let (pops, mut store) = (Pops::new(), CharacterStore::default());
    pops.feed("a", 810);
    let born = founder(
        &mut store,
        &pops,
        Sex::Male,
        "a",
        date(780, 3, 5),
        date(780, 3, 5),
    );
    let older = founder(
        &mut store,
        &pops,
        Sex::Male,
        "a",
        date(779, 3, 5),
        date(780, 3, 5),
    );
    assert_eq!(get(&store, born).life.food_sum, 810);
    pops.feed("a", 819);
    let mut env = Env::new("birth-sample");
    store
        .run_birth_samples(
            &mut world(quiet(), &rules().lifecycle, &mut env, &pops),
            date(780, 3, 5),
        )
        .expect("samples");
    let c = get(&store, born);
    assert_eq!(
        (c.life.food_count, c.life.food_sum, c.life.food_age),
        (1, 819, 0)
    );
    assert_eq!(c.life.height_offset, 0);
    assert_eq!(get(&store, older).life.food_sum, 810, "born another day");
    // Run again on the same day: still one sample.
    store
        .run_birth_samples(
            &mut world(quiet(), &rules().lifecycle, &mut env, &pops),
            date(780, 3, 5),
        )
        .expect("samples");
    assert_eq!(get(&store, born).life.food_count, 1);
}

#[test]
fn a_weight_coefficient_beyond_the_arithmetic_is_an_error_not_a_panic() {
    let mut nutrition = rules().lifecycle.nutrition.clone();
    nutrition.weight_base = i64::MAX;
    assert!(weight_target(&nutrition, i64::MAX, 1000, 1000, -7000).is_err());
}

#[test]
fn a_height_coefficient_the_loader_admits_reaches_the_cap_instead_of_overflowing() {
    // `height_per_food_point` and `height_reference` of 9e15 are 9e18 thousandths each and fit 64 bits: the product of the coefficient
    // and the reference term is far past 128 bits, and the cap of -8 cm still decides the adult offset. At age 2 it is -8 x 2/18 = -0.889.
    let huge = 9_000_000_000_000_000_000_i64;
    let lifecycle = lifecycle_with(|l| {
        l.nutrition.height_per_food_milli = huge;
        l.nutrition.height_reference_milli = huge;
    });
    let nutrition = &lifecycle.nutrition;
    assert_eq!(height_offset(nutrition, 3, 2700, 2), Ok(-889));
    assert_eq!(height_offset(nutrition, 3, 2700, 18), Ok(-8000));
    // Above the reference the same size reaches the other cap: 9e18 x (1e6 - 9e18/1000) is negative too, so use a tiny reference.
    let rich = lifecycle_with(|l| {
        l.nutrition.height_per_food_milli = huge;
        l.nutrition.height_reference_milli = 0;
    });
    assert_eq!(height_offset(&rich.nutrition, 3, 2700, 18), Ok(2000));
    assert_eq!(height_offset(&rich.nutrition, 3, 2700, 9), Ok(1000));
    // Through the pass: the birthdays at 1 and 2 keep all three samples and the offset follows.
    let (pops, mut store) = (Pops::new(), CharacterStore::default());
    pops.feed("a", 900);
    let id = founder(
        &mut store,
        &pops,
        Sex::Male,
        "a",
        date(780, 3, 5),
        date(780, 3, 5),
    );
    let mut env = Env::new("huge-height");
    for year in [781, 782] {
        store
            .run_birthdays(
                &mut world(quiet(), lifecycle, &mut env, &pops),
                date(year, 3, 5),
            )
            .expect("a birthday with admitted coefficients");
    }
    let c = get(&store, id);
    assert_eq!((c.life.food_count, c.life.food_sum), (3, 2700));
    assert_eq!(c.life.height_offset, -889);
}

#[test]
fn a_noise_spread_the_loader_admits_clamps_a_vice_instead_of_failing() {
    // A vice/virtue noise as wide as the representation: the value is far past 7 and is held to it, in a household of several.
    let wide = lifecycle_with(|l| l.upbringing.noise_vice_virtue = i64::MAX);
    let mut members = household(10, (0, 0), [(3000, 0), (-1000, 0)], (1000, 0));
    let mut second = members[3].clone();
    second.id = 5;
    members.push(second);
    let (store, _) = brought_up(wide, "wide-noise", members, quiet(), date(800, 6, 1));
    assert!(
        get(&store, 3)
            .personality
            .vice
            .iter()
            .all(|v| (-7000..=7000).contains(v))
    );
}
