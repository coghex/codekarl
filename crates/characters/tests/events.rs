//! The event engine, the yearly check, deeds, precedence and skills (CK-363, characters slice 4).
//!
//! Characters whose exact state a case needs are built as records and loaded through the canonical encoding. Events are the
//! shipped ones, or the shipped text with a stated edit (usually a chance of 1, so a roll cannot hide the behaviour under test).

use ck_characters::{
    Archive, ArchiveSink, Character, CharacterStore, Context, EventError, GeneticRecord, LifeLog,
    Lineage, LogSink, NewCharacter, PersonalityRecord, PopLocator, Populations, Sex, UNKNOWN,
    World,
    events::{PopFacts, chance_ppb},
    personality::threshold,
};
use ck_economy::{
    EventRules, PopGenetics, Rules,
    events::{Registry, Trigger},
};
use ck_sim::{Canonical, Date, GameTime};
use std::{collections::BTreeMap, io, path::PathBuf, sync::OnceLock};

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
fn asset(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/game/")
            .join(name),
    )
    .expect("shipped data")
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
fn shipped() -> &'static EventRules {
    &rules().events
}
/// The shipped data with each `(from, to)` applied once, to the events file or to the attributes file.
fn edited(events: &[(&str, &str)], attributes: &[(&str, &str)]) -> EventRules {
    let apply = |mut text: String, edits: &[(&str, &str)]| {
        for (from, to) in edits {
            assert!(
                text.contains(from),
                "the shipped text no longer contains {from:?}"
            );
            text = text.replacen(from, to, 1);
        }
        text
    };
    let pop_types: Vec<&str> = rules().pop_types.iter().map(|p| p.id.as_str()).collect();
    EventRules::parse(
        "character-events.toml",
        &apply(asset("events/character-events.toml"), events),
        "attributes-skills.toml",
        &apply(asset("events/attributes-skills.toml"), attributes),
        &Registry {
            peoples: &rules().peoples,
            genetics: &rules().genetics,
            pop_types: &pop_types,
        },
    )
    .expect("edited rules")
}
/// The shipped events plus these extra events (a TOML text of `[[event]]` tables).
fn with_events(extra: &str) -> EventRules {
    let pop_types: Vec<&str> = rules().pop_types.iter().map(|p| p.id.as_str()).collect();
    EventRules::parse(
        "character-events.toml",
        &format!("{}\n{extra}", asset("events/character-events.toml")),
        "attributes-skills.toml",
        &asset("events/attributes-skills.toml"),
        &Registry {
            peoples: &rules().peoples,
            genetics: &rules().genetics,
            pop_types: &pop_types,
        },
    )
    .expect("rules with extra events")
}
/// Only these events (the attributes file is the shipped one).
fn only_events(text: &str) -> EventRules {
    let pop_types: Vec<&str> = rules().pop_types.iter().map(|p| p.id.as_str()).collect();
    EventRules::parse(
        "character-events.toml",
        text,
        "attributes-skills.toml",
        &asset("events/attributes-skills.toml"),
        &Registry {
            peoples: &rules().peoples,
            genetics: &rules().genetics,
            pop_types: &pop_types,
        },
    )
    .expect("rules of only these events")
}

/// The pops: `a` noble, `m` monk, `p` peasant (all Irish, `ulaid`); `b` is a Pictish noble pop. Luxuries are a field.
struct Pops {
    luxuries: u16,
}
impl Populations for Pops {
    fn heads(&self, _: &PopLocator) -> Option<u64> {
        Some(1_000_000)
    }
    fn culture(&self, locator: &PopLocator) -> Option<String> {
        Some(
            if locator.place == "b" {
                "cruthin"
            } else {
                "ulaid"
            }
            .to_owned(),
        )
    }
    fn kind(&self, locator: &PopLocator) -> Option<String> {
        Some(
            match locator.place.as_str() {
                "m" => "monk",
                "p" => "peasant",
                _ => "noble",
            }
            .to_owned(),
        )
    }
    fn luxuries_pm(&self, _: &PopLocator) -> Option<u16> {
        Some(self.luxuries)
    }
}
const POPS: Pops = Pops { luxuries: 600 };

fn path(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("ck-events-{}-{tag}", std::process::id()));
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
    rules: &'a EventRules,
    env: &'a mut Env,
    pops: &'a Pops,
) -> World<'a, Pops, Archive, LifeLog> {
    World {
        rules,
        personality: &ck_rules().personality,
        genetics: &ck_rules().genetics,
        lifecycle: &ck_rules().lifecycle,
        seed: 7,
        populations: pops,
        archive: &mut env.archive,
        logs: &mut env.logs,
    }
}
fn ck_rules() -> &'static Rules {
    rules()
}

/// A record with the given humours and personality, in pop `place`.
fn person(id: u64, birth: GameTime, place: &str, sex: Sex) -> Character {
    let mut genetics = GeneticRecord::default();
    genetics.value[9..13].copy_from_slice(&[5000; 4]);
    for stat in 0..9 {
        genetics.value[stat] = 10_000;
    }
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
        life: Box::default(),
    }
}
fn man(id: u64, place: &str) -> Character {
    person(id, date(740, 6, 1), place, Sex::Male)
}
fn store_of(characters: Vec<Character>) -> CharacterStore {
    let next = characters.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    let live: BTreeMap<u64, Character> = characters.into_iter().map(|c| (c.id, c)).collect();
    let mut bytes = Vec::new();
    next.encode(&mut bytes).expect("encode");
    live.encode(&mut bytes).expect("encode");
    CharacterStore::from_bytes(&bytes).expect("a store of these records")
}
fn get(store: &CharacterStore, id: u64) -> &Character {
    store.get(id).expect("live")
}

// --- The event engine -------------------------------------------------------------------------------------------------

#[test]
fn events_run_in_data_order_and_read_what_earlier_events_changed() {
    let first_then_second = only_events(
        r#"
[[event]]
id = "first"
trigger = "happening:test"
chance = 1
effects = [{ raise_grade = "melancholy" }]
text = "{name} brooded."
[[event]]
id = "second"
trigger = "happening:test"
when = { has = ["melancholy"] }
chance = 1
effects = [{ stress = 7 }]
text = "{name} fretted."
"#,
    );
    let second_then_first = only_events(
        r#"
[[event]]
id = "second"
trigger = "happening:test"
when = { has = ["melancholy"] }
chance = 1
effects = [{ stress = 7 }]
text = "{name} fretted."
[[event]]
id = "first"
trigger = "happening:test"
chance = 1
effects = [{ raise_grade = "melancholy" }]
text = "{name} brooded."
"#,
    );
    for (rules, fretted) in [(&first_then_second, true), (&second_then_first, false)] {
        let mut store = store_of(vec![man(1, "a")]);
        let mut env = Env::new(&format!("order-{fretted}"));
        store
            .report_happening(
                &mut world(rules, &mut env, &POPS),
                1,
                "test",
                date(768, 1, 1),
            )
            .expect("happening");
        assert_eq!(get(&store, 1).personality.melancholy, 1);
        assert_eq!(get(&store, 1).personality.stress > 0, fretted);
        assert_eq!(env.lines(1).len(), if fretted { 2 } else { 1 });
    }
}

#[test]
fn the_exact_chance_is_one_rational_rounded_down_once_and_capped_at_one() {
    // birth_of_child_joy: 0.25 x blood 5 (exactly 1) / (1 + 100/20): 0.25 / 6 = 41,666,666.67 ppb.
    let event = &shipped().events[0];
    assert_eq!(event.id, "birth_of_child_joy");
    let mut c = man(1, "a");
    c.genetics.value[9..13].copy_from_slice(&[5000, 10_000, 10_000, 0]);
    c.personality.stress = 100_000;
    assert_eq!(threshold(&c), 0);
    let pop = PopFacts {
        kind: "noble".into(),
        group: "gaelic".into(),
        luxuries_pm: 600,
    };
    assert_eq!(chance_ppb(event, &c, &pop).expect("chance"), 41_666_666);
    // Below the threshold the stress factor is 1.
    c.personality.stress = 0;
    assert_eq!(chance_ppb(event, &c, &pop).expect("chance"), 250_000_000);
    // A cause multiplies: 0.03 x black bile 10 (1.75) x over 6 = 0.315 exactly.
    let cause = shipped()
        .events
        .iter()
        .find(|e| e.id == "death_in_family_melancholy")
        .expect("event");
    c.personality.stress = 100_000;
    c.genetics.value[11] = 10_000;
    assert_eq!(chance_ppb(cause, &c, &pop).expect("chance"), 315_000_000);
    // And the product is capped at 1.
    let capped = only_events(
        r#"
[[event]]
id = "capped"
trigger = "happening:test"
chance = 0.5
factors = [{ stress = "cause" }]
effects = []
text = "{name}."
"#,
    );
    assert_eq!(
        chance_ppb(&capped.events[0], &c, &pop).expect("chance"),
        1_000_000_000
    );
}

#[test]
fn the_same_seed_character_and_tick_roll_the_same() {
    let rules = edited(
        &[(
            "chance = 0.25\nfactors = [{ humour = \"blood\" }, { stress = \"lift\" }]",
            "chance = 0.5\nfactors = [{ humour = \"blood\" }, { stress = \"lift\" }]",
        )],
        &[],
    );
    let outcome = |tick: GameTime, tag: &str| {
        let mut c = man(1, "a");
        c.personality.melancholy = 2;
        let mut store = store_of(vec![c]);
        let mut env = Env::new(tag);
        store
            .report_happening(&mut world(&rules, &mut env, &POPS), 1, "child_born", tick)
            .expect("happening");
        get(&store, 1).personality.melancholy
    };
    assert_eq!(
        outcome(date(768, 3, 3), "same-a"),
        outcome(date(768, 3, 3), "same-b")
    );
    // Over many ticks the roll is neither always a success nor always a failure.
    let lifted = (1..=28)
        .filter(|day| outcome(date(768, 3, *day), &format!("spread-{day}")) == 1)
        .count();
    assert!((4..=24).contains(&lifted), "{lifted} of 28 at chance 1/2");
}

#[test]
fn an_insult_adds_more_stress_at_high_yellow_bile_than_the_same_stress_otherwise() {
    let rules = with_events(
        r#"
[[event]]
id = "plain_ten"
trigger = "happening:plain"
chance = 1
effects = [{ stress = 10 }]
text = "{name} was troubled."
"#,
    );
    let stress_after = |happening: &str, tag: &str| {
        let mut c = man(1, "a");
        c.genetics.value[10] = 9000;
        let mut store = store_of(vec![c]);
        let mut env = Env::new(tag);
        store
            .report_happening(
                &mut world(&rules, &mut env, &POPS),
                1,
                happening,
                date(768, 1, 1),
            )
            .expect("happening");
        get(&store, 1).personality.stress
    };
    let insulted = stress_after("insulted", "insult");
    let plain = stress_after("plain", "plain");
    assert!(insulted > plain, "{insulted} vs {plain}");
}

#[test]
fn an_events_effects_commit_as_one_unit() {
    let rules = only_events(
        r#"
[[event]]
id = "overflow"
trigger = "happening:test"
chance = 1
effects = [{ stress = 10 }, { raise_grade = "melancholy" }, { standing = "fame", add = 1 }]
text = "{name} was undone."
"#,
    );
    let mut c = man(1, "a");
    c.personality.fame = i64::MAX;
    let mut store = store_of(vec![c]);
    let before = store.to_bytes().expect("encode");
    let mut env = Env::new("unit");
    let error = store
        .report_happening(
            &mut world(&rules, &mut env, &POPS),
            1,
            "test",
            date(768, 1, 1),
        )
        .unwrap_err();
    assert!(
        matches!(
            error,
            EventError::Rules(ck_characters::Error::StandingRange)
        ),
        "{error:?}"
    );
    assert_eq!(
        store.to_bytes().expect("encode"),
        before,
        "the character is unchanged"
    );
    assert!(env.lines(1).is_empty());
}

#[test]
fn die_stops_the_later_effects_and_writes_its_line_to_the_archived_log() {
    let rules = only_events(
        r#"
[[event]]
id = "fatal"
trigger = "happening:test"
chance = 1
effects = [{ standing = "fame", add = 3 }, { die = "fever" }, { stress = 50 }]
text = "{name} died of a fever."
"#,
    );
    let mut store = store_of(vec![man(1, "a")]);
    let mut env = Env::new("die");
    store
        .report_happening(
            &mut world(&rules, &mut env, &POPS),
            1,
            "test",
            date(768, 5, 5),
        )
        .expect("happening");
    assert!(store.get(1).is_none(), "the character is dead");
    let dead = env.archive.get(1).expect("archive").expect("archived");
    assert_eq!(dead.death, Some(date(768, 5, 5)));
    assert_eq!(dead.life.death_cause.as_deref(), Some("fever"));
    assert_eq!(
        dead.personality.fame, 3000,
        "effects before the death were applied"
    );
    assert_eq!(dead.personality.stress, 0, "effects after it did not run");
    let lines = env.logs.read(1).expect("log");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].text, "c1 died of a fever.");
    assert_eq!(lines[0].date, date(768, 5, 5));
}

#[test]
fn the_shipped_drank_less_at_c_logs_with_none() {
    let rules = edited(
        &[(
            "chance = 0.05\nfactors = [{ stress = \"lift\" }]\neffects = [{ lower_grade = \"drunkard\" }]",
            "chance = 1\nfactors = [{ stress = \"lift\" }]\neffects = [{ lower_grade = \"drunkard\" }]",
        )],
        &[],
    );
    let mut c = person(1, date(730, 4, 4), "a", Sex::Male);
    c.personality.drunkard = 1;
    let mut store = store_of(vec![c]);
    let mut env = Env::new("drank");
    store
        .run_birthdays(&mut world(&rules, &mut env, &POPS), date(768, 4, 4))
        .expect("birthday");
    assert_eq!(get(&store, 1).personality.drunkard, 0);
    assert!(
        env.lines(1).contains(&"c1 drank less (none).".to_owned()),
        "{:?}",
        env.lines(1)
    );
}

#[test]
fn a_successful_roll_that_changes_nothing_writes_no_line() {
    let rules = edited(
        &[(
            "chance = 0.03\nfactors = [{ humour = \"black_bile\" }, { stress = \"cause\" }]\neffects = [{ raise_grade = \"melancholy\" }]\ntext = \"Grief settled",
            "chance = 1\nfactors = [{ humour = \"black_bile\" }, { stress = \"cause\" }]\neffects = [{ raise_grade = \"melancholy\" }]\ntext = \"Grief settled",
        )],
        &[],
    );
    let mut c = man(1, "a");
    c.personality.melancholy = 6;
    let mut store = store_of(vec![c]);
    let mut env = Env::new("sss");
    store
        .report_happening(
            &mut world(&rules, &mut env, &POPS),
            1,
            "death_in_family",
            date(768, 1, 1),
        )
        .expect("happening");
    assert_eq!(get(&store, 1).personality.melancholy, 6);
    let lines = env.lines(1);
    assert!(
        lines.iter().all(|line| !line.contains("melancholy")),
        "{lines:?}"
    );
    // The grief line (a stress effect) is written whatever the roll.
    assert!(
        lines.contains(&"c1 mourned a death in the family.".to_owned()),
        "{lines:?}"
    );
}

#[test]
fn an_event_with_no_effects_and_a_stress_event_at_zero_always_log() {
    let mut c = person(1, date(750, 1, 1), "a", Sex::Female);
    c.genetics.value[9] = 5000;
    let mut store = store_of(vec![c]);
    let mut env = Env::new("always");
    let mut world = world(shipped(), &mut env, &POPS);
    store
        .report_happening(&mut world, 1, "coming_of_age", date(768, 1, 1))
        .expect("happening");
    store
        .report_happening(&mut world, 1, "marriage", date(768, 1, 2))
        .expect("happening");
    assert_eq!(get(&store, 1).personality.stress, 0);
    assert_eq!(
        env.lines(1),
        vec!["c1 came of age.".to_owned(), "c1 married.".to_owned()]
    );
}

/// A pop source that knows culture and size but no pop type.
struct NoKinds;
impl Populations for NoKinds {
    fn heads(&self, _: &PopLocator) -> Option<u64> {
        Some(10)
    }
    fn culture(&self, _: &PopLocator) -> Option<String> {
        Some("ulaid".to_owned())
    }
}

#[test]
fn a_missing_character_and_an_unresolvable_pop_are_refused() {
    let mut store = store_of(vec![man(1, "a")]);
    let mut env = Env::new("refuse");
    let mut world = world(shipped(), &mut env, &POPS);
    assert!(matches!(
        store.report_happening(&mut world, 9, "marriage", date(768, 1, 1)),
        Err(EventError::Rules(ck_characters::Error::MissingCharacter))
    ));
    let mut env = Env::new("refuse-pop");
    let mut blind = World {
        rules: shipped(),
        personality: &rules().personality,
        genetics: &ck_rules().genetics,
        lifecycle: &ck_rules().lifecycle,
        seed: 7,
        populations: &NoKinds,
        archive: &mut env.archive,
        logs: &mut env.logs,
    };
    assert!(matches!(
        store.report_happening(&mut blind, 1, "marriage", date(768, 1, 1)),
        Err(EventError::Rules(ck_characters::Error::Pop))
    ));
}

// --- Life events that emit happenings ---------------------------------------------------------------------------------

fn new_child(place: &str) -> NewCharacter {
    NewCharacter {
        name: "kid".into(),
        sex: Sex::Female,
        birth: date(768, 2, 2),
        pop: locator(place),
    }
}
fn irish() -> PopGenetics {
    rules().genetics.seed_pop("ulaid").expect("Irish start set")
}
fn context(tick: GameTime) -> Context<'static> {
    Context {
        rules: &rules().genetics,
        personality: &rules().personality,
        events: shipped(),
        lifecycle: &rules().lifecycle,
        seed: 7,
        tick,
    }
}

#[test]
fn a_new_child_emits_child_born_to_each_live_parent_only() {
    let father = man(1, "a");
    let mother = person(2, date(745, 1, 1), "a", Sex::Female);
    let mut store = store_of(vec![father, mother]);
    let mut env = Env::new("born");
    let archive_for_lineage = Archive::open(path("born-lineage")).expect("archive");
    let lineage = Lineage {
        father: 1,
        mother: 2,
        archive: &archive_for_lineage,
    };
    let child = store
        .create_child_with_events(
            &mut world(shipped(), &mut env, &POPS),
            &context(date(768, 2, 2)),
            new_child("a"),
            &irish(),
            &lineage,
        )
        .expect("child");
    assert_eq!(child, 3);
    for parent in [1, 2] {
        assert!(
            env.lines(parent)
                .contains(&format!("A child was born to c{parent}.")),
            "{:?}",
            env.lines(parent)
        );
    }
    assert!(env.lines(child).is_empty(), "the child is not a recipient");
    // An unknown parent is not a recipient: only the mother's line is added.
    let lineage = Lineage {
        father: UNKNOWN,
        mother: 2,
        archive: &archive_for_lineage,
    };
    store
        .create_child_with_events(
            &mut world(shipped(), &mut env, &POPS),
            &context(date(768, 2, 3)),
            new_child("a"),
            &irish(),
            &lineage,
        )
        .expect("child");
    assert_eq!(env.lines(1).len(), 1);
    assert_eq!(env.lines(2).len(), 2);
}

#[test]
fn a_death_emits_death_in_family_to_its_live_family() {
    let mut father = man(1, "a");
    father.children = vec![3];
    let mut mother = person(2, date(745, 1, 1), "a", Sex::Female);
    mother.children = vec![3];
    mother.spouses = vec![1];
    father.spouses = vec![2];
    let mut dead_child = person(3, date(765, 1, 1), "a", Sex::Female);
    dead_child.father = 1;
    dead_child.mother = 2;
    let stranger = man(4, "a");
    let mut store = store_of(vec![father, mother, dead_child, stranger]);
    let mut env = Env::new("bereave");
    store
        .die_with_events(
            &mut world(shipped(), &mut env, &POPS),
            3,
            date(768, 6, 6),
            "plague",
        )
        .expect("death");
    assert!(store.get(3).is_none());
    assert_eq!(
        env.archive
            .get(3)
            .expect("archive")
            .expect("archived")
            .life
            .death_cause
            .as_deref(),
        Some("plague")
    );
    for relative in [1, 2] {
        assert!(
            env.lines(relative)
                .contains(&format!("c{relative} mourned a death in the family."))
        );
        assert_eq!(get(&store, relative).personality.stress, 25_000);
    }
    assert!(env.lines(4).is_empty(), "a stranger is not family");
    assert!(env.lines(3).is_empty(), "nor is the dead");
}

#[test]
fn marrying_an_existing_spouse_emits_nothing_and_applies_no_relief() {
    let mut a = man(1, "a");
    a.personality.stress = 30_000;
    let mut b = person(2, date(745, 1, 1), "a", Sex::Female);
    b.personality.stress = 30_000;
    let mut store = store_of(vec![a, b]);
    let mut env = Env::new("marry");
    let mut world = world(shipped(), &mut env, &POPS);
    store
        .marry_with_events(&mut world, 1, 2, date(768, 4, 4))
        .expect("marriage");
    assert_eq!(get(&store, 1).personality.stress, 20_000);
    assert_eq!(get(&store, 2).personality.stress, 20_000);
    store
        .marry_with_events(&mut world, 1, 2, date(768, 4, 5))
        .expect("repeat");
    assert_eq!(
        get(&store, 1).personality.stress,
        20_000,
        "no second relief"
    );
    assert_eq!(env.lines(1), vec!["c1 married.".to_owned()]);
    assert_eq!(env.lines(2), vec!["c2 married.".to_owned()]);
}

/// An archive whose append fails while `armed`.
struct FlakyArchive {
    inner: Archive,
    armed: bool,
}
impl ArchiveSink for FlakyArchive {
    fn append(&mut self, record: &Character) -> io::Result<()> {
        if self.armed {
            Err(io::Error::other("injected archive failure"))
        } else {
            self.inner.append(record)
        }
    }
}
/// A log whose append fails while `armed`.
struct FlakyLog {
    inner: LifeLog,
    armed: bool,
}
impl LogSink for FlakyLog {
    fn append(&mut self, id: u64, date: GameTime, text: &str) -> io::Result<()> {
        if self.armed {
            Err(io::Error::other("injected log failure"))
        } else {
            self.inner.append(id, date, text)
        }
    }
}

fn fatal_rules() -> EventRules {
    only_events(
        r#"
[[event]]
id = "fatal"
trigger = "happening:test"
chance = 1
effects = [{ stress = 5 }, { die = "fever" }]
text = "{name} died of a fever."
[[event]]
id = "grief"
trigger = "happening:death_in_family"
chance = 1
effects = [{ stress = 25 }]
text = "{name} mourned."
"#,
    )
}

#[test]
fn a_failing_archive_append_leaves_the_character_live_with_no_line_and_no_bereavement() {
    let rules = fatal_rules();
    let mut son = person(2, date(765, 1, 1), "a", Sex::Male);
    son.father = 1;
    let mut father = man(1, "a");
    father.children = vec![2];
    let mut store = store_of(vec![father, son]);
    let before = store.to_bytes().expect("encode");
    let mut env = Env::new("flaky-archive");
    let mut archive = FlakyArchive {
        inner: Archive::open(path("flaky-archive-arch")).expect("archive"),
        armed: true,
    };
    let mut world = World {
        rules: &rules,
        personality: &ck_rules().personality,
        genetics: &ck_rules().genetics,
        lifecycle: &ck_rules().lifecycle,
        seed: 7,
        populations: &POPS,
        archive: &mut archive,
        logs: &mut env.logs,
    };
    let error = store
        .report_happening(&mut world, 1, "test", date(768, 1, 1))
        .unwrap_err();
    assert!(matches!(error, EventError::Archive(_)), "{error:?}");
    assert_eq!(
        store.to_bytes().expect("encode"),
        before,
        "nothing changed, the earlier stress effect included"
    );
    assert!(env.lines(1).is_empty());
    assert!(env.lines(2).is_empty(), "no bereavement was emitted");
    assert!(!archive.inner.contains(1));
}

#[test]
fn a_failing_log_append_after_a_committed_death_returns_the_error_and_the_death_stands() {
    let rules = fatal_rules();
    let mut store = store_of(vec![man(1, "a")]);
    let mut archive = Archive::open(path("flaky-log-arch")).expect("archive");
    let mut logs = FlakyLog {
        inner: LifeLog::open(path("flaky-log-logs")).expect("logs"),
        armed: true,
    };
    let mut world = World {
        rules: &rules,
        personality: &ck_rules().personality,
        genetics: &ck_rules().genetics,
        lifecycle: &ck_rules().lifecycle,
        seed: 7,
        populations: &POPS,
        archive: &mut archive,
        logs: &mut logs,
    };
    let error = store
        .report_happening(&mut world, 1, "test", date(768, 1, 1))
        .unwrap_err();
    assert!(matches!(error, EventError::Log(_)), "{error:?}");
    assert!(store.get(1).is_none(), "the death stands");
    assert!(archive.contains(1));
    assert!(logs.inner.read(1).expect("log").is_empty());
}

// --- Deeds ------------------------------------------------------------------------------------------------------------

#[test]
fn a_deeds_grade_follows_its_counter_and_never_falls() {
    let mut store = store_of(vec![man(1, "a")]);
    let mut env = Env::new("deeds");
    let mut world = world(shipped(), &mut env, &POPS);
    let mut grades = Vec::new();
    for battle in 1..=30_u16 {
        store
            .report_happening(&mut world, 1, "battle_survived", date(768, 1, 1))
            .expect("battle");
        let c = get(&store, 1);
        assert_eq!(c.life.counters["battles_survived"], u64::from(battle));
        grades.push(c.grade("veteran"));
    }
    assert_eq!(grades[1], 0, "two battles: not yet");
    assert_eq!(grades[2], 1, "veteran C at 3 battles");
    assert_eq!(grades[5], 2, "B at 6");
    assert_eq!(grades[27], 6, "SSS at 28");
    assert!(
        grades.windows(2).all(|pair| pair[0] <= pair[1]),
        "a deed never falls: {grades:?}"
    );
    assert_eq!(get(&store, 1).personality.fame, 30_000);
}

// --- Precedence -------------------------------------------------------------------------------------------------------

fn monk_with(grades: &[&str]) -> Character {
    let mut c = man(1, "m");
    for key in grades {
        c.life.grades.insert((*key).to_owned(), 1);
    }
    c
}

#[test]
fn an_ascetic_whose_gluttony_passes_zero_loses_ascetic_then_its_dependants_the_same_day() {
    let mut c = monk_with(&["ascetic", "cele_de", "saintly"]);
    c.personality.vice[4] = -1000;
    let mut store = store_of(vec![c]);
    let mut env = Env::new("ascetic");
    store
        .apply_precedence(&mut world(shipped(), &mut env, &POPS), 1, date(768, 1, 1))
        .expect("precedence");
    let c = get(&store, 1);
    assert!(
        ["ascetic", "cele_de", "saintly"]
            .iter()
            .all(|key| c.grade(key) == 0)
    );
    assert_eq!(
        env.lines(1),
        vec![
            "c1 lost ascetic.".to_owned(),
            "c1 lost cele de.".to_owned(),
            "c1 lost saintly.".to_owned()
        ]
    );
    // A held attribute whose conditions hold stays.
    let mut keeper = monk_with(&["ascetic"]);
    keeper.personality.vice[4] = 0;
    keeper.personality.vice[2] = 0;
    let mut store = store_of(vec![keeper]);
    let mut env = Env::new("ascetic-keeps");
    store
        .apply_precedence(&mut world(shipped(), &mut env, &POPS), 1, date(768, 1, 1))
        .expect("precedence");
    assert_eq!(get(&store, 1).grade("ascetic"), 1);
    assert!(env.lines(1).is_empty());
}

#[test]
fn inherited_land_removes_fennid_sets_the_flag_then_precedence_removes_rigfennid() {
    let mut c = person(1, date(748, 1, 1), "a", Sex::Male);
    c.life.grades.insert("fennid".into(), 1);
    c.life.grades.insert("rigfennid".into(), 1);
    let mut store = store_of(vec![c]);
    let mut env = Env::new("land");
    store
        .report_happening(
            &mut world(shipped(), &mut env, &POPS),
            1,
            "inherited_land",
            date(768, 1, 1),
        )
        .expect("happening");
    let c = get(&store, 1);
    assert!(c.life.holds_land);
    assert_eq!((c.grade("fennid"), c.grade("rigfennid")), (0, 0));
    assert_eq!(
        env.lines(1),
        vec![
            "c1 came into his land and left the fían.".to_owned(),
            "c1 lost rigfennid.".to_owned()
        ]
    );
}

#[test]
fn a_hospitaller_below_five_hundred_permille_luxuries_loses_it_on_the_first_only() {
    let make = || {
        store_of(vec![{
            let mut c = man(1, "a");
            c.life.grades.insert("hospitaller".into(), 1);
            c.personality.melancholy = 3;
            c
        }])
    };
    let (mut poor, mut rich) = (make(), make());
    let (mut poor_env, mut rich_env) = (Env::new("poor"), Env::new("rich"));
    let poor_pops = Pops { luxuries: 400 };
    poor.monthly_precedence(
        &mut world(shipped(), &mut poor_env, &poor_pops),
        date(768, 5, 1),
    )
    .expect("monthly");
    rich.monthly_precedence(&mut world(shipped(), &mut rich_env, &POPS), date(768, 5, 1))
        .expect("monthly");
    assert_eq!(get(&poor, 1).grade("hospitaller"), 0);
    assert_eq!(poor_env.lines(1), vec!["c1 lost hospitaller.".to_owned()]);
    assert_eq!(get(&rich, 1).grade("hospitaller"), 1);
    assert_eq!(
        get(&poor, 1).personality.melancholy,
        3,
        "a mind trait is untouched by precedence"
    );
}

// --- The birthday pass -------------------------------------------------------------------------------------------------

#[test]
fn the_birthday_pass_runs_one_order_and_a_craft_gained_that_day_first_rises_next_year() {
    // Every chance of interest is 1: a saintly grant, a poet grant, a craft rise of 1 at oratory 15.
    let rules = edited(
        &[
            (
                "chance = 0.005\neffects = [{ grant = \"saintly\" }",
                "chance = 1\neffects = [{ grant = \"saintly\" }",
            ),
            ("chance = 0.00067\nfactors", "chance = 1\nfactors"),
        ],
        &[("rise = 0.05", "rise = 1")],
    );
    let mut c = person(1, date(730, 4, 4), "m", Sex::Male);
    c.life.grades.insert("ascetic".into(), 1);
    c.life.skills.insert("theology".into(), 20_000);
    c.life.skills.insert("oratory".into(), 40_000);
    c.life.focus = "balanced".into();
    c.personality.vice = [5000; 7];
    c.personality.fame = 0;
    let mut store = store_of(vec![c]);
    let mut env = Env::new("order");
    store
        .run_birthdays(&mut world(&rules, &mut env, &POPS), date(768, 4, 4))
        .expect("birthday");
    let c = get(&store, 1);
    assert_eq!(c.grade("saintly"), 1);
    assert_eq!(
        c.personality.fame, 5000,
        "the grant came after the fade: fame ends the birthday at 5, not 4.75"
    );
    assert_eq!(c.grade("poet"), 1, "poet is granted at C");
    assert_eq!(
        c.grade("ascetic"),
        1,
        "precedence ran last and the ascetic held"
    );
    store
        .run_birthdays(&mut world(&rules, &mut env, &POPS), date(769, 4, 4))
        .expect("birthday");
    assert_eq!(
        get(&store, 1).grade("poet"),
        2,
        "the poet rises the next year (rise 1 x oratory 15 / 15)"
    );
    assert!(
        env.lines(1)
            .iter()
            .any(|line| line.contains("grew in skill as poet (B)")),
        "{:?}",
        env.lines(1)
    );
}

#[test]
fn a_birthday_that_kills_ends_the_characters_later_stages() {
    let rules = only_events(
        r#"
[[event]]
id = "fatal_year"
trigger = "yearly"
when = { age = [30, 200] }
chance = 1
effects = [{ die = "age" }]
text = "{name} died."
"#,
    );
    let mut c = person(1, date(730, 4, 4), "a", Sex::Male);
    c.life.skills.insert("leadership".into(), 5000);
    let mut store = store_of(vec![c]);
    let mut env = Env::new("dies");
    store
        .run_birthdays(&mut world(&rules, &mut env, &POPS), date(768, 4, 4))
        .expect("birthday");
    let dead = env.archive.get(1).expect("archive").expect("archived");
    assert_eq!(
        dead.skill("leadership"),
        5000,
        "the growth stage did not run for the dead"
    );
    assert_eq!(env.lines(1), vec!["c1 died.".to_owned()]);
}

// --- Skills -----------------------------------------------------------------------------------------------------------

fn use_factor_one(c: &mut Character) {
    for skill in &shipped().skills.list {
        c.life.use_factor.insert(skill.clone(), 1000);
    }
}

#[test]
fn growth_by_use_and_decay_follow_the_worked_formula() {
    // A noble man's leadership use is 0.6, at factor 1: year one is 3.0 x 0.6 = 1.8 and nothing decays from 0.
    let mut c = person(1, date(750, 4, 4), "a", Sex::Male);
    use_factor_one(&mut c);
    let mut store = store_of(vec![c]);
    let mut env = Env::new("growth");
    let quiet = only_events("");
    store
        .run_birthdays(&mut world(&quiet, &mut env, &POPS), date(768, 4, 4))
        .expect("birthday");
    assert_eq!(get(&store, 1).skill("leadership"), 1800);
}

#[test]
fn a_second_year_of_growth_and_decay_gives_the_worked_value() {
    // Year two: level 1.8, use 0.6: gain 1.8 / (1 + (1.8 / 12)^2) = 1.7604, loss 1.8 x (0.04 x 0.6 + 0.2 x 0.4) = 0.1872.
    let mut c = person(1, date(750, 4, 4), "a", Sex::Male);
    use_factor_one(&mut c);
    let mut store = store_of(vec![c]);
    let mut env = Env::new("growth2");
    let quiet = only_events("");
    for year in [768, 769] {
        store
            .run_birthdays(&mut world(&quiet, &mut env, &POPS), date(year, 4, 4))
            .expect("birthday");
    }
    assert_eq!(get(&store, 1).skill("leadership"), 3373);
}

#[test]
fn a_fully_used_skill_settles_near_twenty_over_decades() {
    let rules = edited(
        &[],
        &[(
            "husbandry = 0.9\ncraftsmanship = 0.3\nmartial_prowess = 0.2\n[skills.use.peasant.f]",
            "husbandry = 1.0\ncraftsmanship = 0.3\nmartial_prowess = 0.2\n[skills.use.peasant.f]",
        )],
    );
    let mut c = person(1, date(700, 4, 4), "p", Sex::Male);
    use_factor_one(&mut c);
    let mut store = store_of(vec![c]);
    let mut env = Env::new("settle");
    let quiet = {
        let mut quiet = rules.clone();
        quiet
            .events
            .retain(|event| event.trigger == Trigger::Happening("never".into()));
        quiet
    };
    for year in 768..=840 {
        store
            .run_birthdays(&mut world(&quiet, &mut env, &POPS), date(year, 4, 4))
            .expect("birthday");
    }
    let level = get(&store, 1).skill("husbandry");
    assert!((19_000..=21_000).contains(&level), "{level}");
}

#[test]
fn home_education_reads_the_snapshot_so_a_teacher_who_dies_that_day_still_teaches() {
    let rules = only_events(
        r#"
[[event]]
id = "fatal_year"
trigger = "yearly"
when = { age = [30, 200] }
chance = 1
effects = [{ die = "age" }]
text = "{name} died."
"#,
    );
    let outcome = |teacher: u64, pupil: u64, tag: &str| {
        let mut old = person(teacher, date(730, 4, 4), "a", Sex::Male);
        old.life.skills.insert("leadership".into(), 10_000);
        old.life.grades.insert("poet".into(), 3);
        let child = person(pupil, date(763, 4, 4), "a", Sex::Female);
        let mut store = store_of(vec![old, child]);
        let mut env = Env::new(tag);
        store
            .run_birthdays(&mut world(&rules, &mut env, &POPS), date(768, 4, 4))
            .expect("birthday");
        assert!(store.get(teacher).is_none(), "the teacher died in stage 3");
        store.get(pupil).expect("the pupil lives").clone()
    };
    // The pupil is five, under the training age, with neutral aptitude, scholarship 0 and a balanced focus:
    // 0.05 x 1.0 x 1.0 x (10 - 0) = 0.5 exactly, whichever of them goes first.
    let after_teacher = outcome(1, 2, "teach-1");
    let before_teacher = outcome(2, 1, "teach-2");
    assert_eq!(after_teacher.skill("leadership"), 500);
    assert_eq!(before_teacher.skill("leadership"), 500);
    // Skills only, never attributes: the pupil gained no craft, and nothing but skills moved.
    assert_eq!(after_teacher.grade("poet"), 0);
    assert_eq!(after_teacher.life.grades, BTreeMap::new());
    assert_eq!(after_teacher.genetics, before_teacher.genetics);
}

#[test]
fn home_education_moves_a_child_toward_the_teacher_by_the_stated_fraction_and_focus() {
    // A child of 10 learns 0.05 x (10 - level) a year from the best of its pop; a church focus makes theology x1.5 and the rest x0.7.
    let rules = only_events("");
    let mut teacher = person(1, date(720, 9, 9), "m", Sex::Male);
    teacher.life.skills.insert("theology".into(), 10_000);
    teacher.life.skills.insert("husbandry".into(), 10_000);
    let mut pupil = person(2, date(758, 4, 4), "m", Sex::Male);
    pupil.life.focus = "church".into();
    use_factor_one(&mut pupil);
    let mut store = store_of(vec![teacher, pupil]);
    let mut env = Env::new("church");
    store
        .run_birthdays(&mut world(&rules, &mut env, &POPS), date(768, 4, 4))
        .expect("birthday");
    let c = get(&store, 2);
    // Theology: growth from 0 at use 0.9 (monk) with aptitude (intellect 10) is 3 x 0.9 = 2.7; then, at age 10 (factor 1.0),
    // 0.05 x (10 - 2.7) x 1.5 = 0.5475, which rounds to 0.548.
    assert_eq!(c.skill("theology"), 2700 + 548, "church focus x1.5");
    // Husbandry: no use by a monk, so 0 from growth; then 0.05 x 10 x 0.7 = 0.35.
    assert_eq!(c.skill("husbandry"), 350, "off-focus x0.7");
}

#[test]
fn focus_and_use_are_drawn_once_at_birth_by_the_stated_odds() {
    let mut store = CharacterStore::default();
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    let samples = 2000_u16;
    for n in 0..samples {
        let id = store
            .create_founder(
                &context(date(768, 1, 1)),
                NewCharacter {
                    name: "x".into(),
                    sex: Sex::Male,
                    birth: date(740, 1, 1),
                    pop: locator("a"),
                },
                &irish(),
                &POPS,
            )
            .expect("founder");
        let c = get(&store, id);
        assert!(
            c.life
                .use_factor
                .values()
                .all(|factor| (0..=1000).contains(factor))
        );
        assert!(
            c.life.skills.values().all(|level| *level == 0),
            "skills are 0 at birth"
        );
        *counts.entry(c.life.focus.clone()).or_default() += 1;
        if n == 0 {
            // The same seed, character and tick draw the same.
            let mut again = CharacterStore::default();
            let id = again
                .create_founder(
                    &context(date(768, 1, 1)),
                    NewCharacter {
                        name: "x".into(),
                        sex: Sex::Male,
                        birth: date(740, 1, 1),
                        pop: locator("a"),
                    },
                    &irish(),
                    &POPS,
                )
                .expect("founder");
            assert_eq!(get(&again, id).life, c.life);
        }
    }
    // Noble men: balanced 40, martial 30, statecraft 15, learning 10, church 5 (percent); tolerance 4 points.
    for (focus, percent) in [
        ("balanced", 40),
        ("martial", 30),
        ("statecraft", 15),
        ("learning", 10),
        ("church", 5),
    ] {
        let share = i64::from(counts.get(focus).copied().unwrap_or(0)) * 100 / i64::from(samples);
        assert!(
            (share - percent).abs() <= 4,
            "{focus}: {share}% vs {percent}%"
        );
    }
    // The record never changes after birth: a birthday leaves focus and use factors alone.
    let before = get(&store, 1).life.clone();
    let mut env = Env::new("drawn-once");
    store
        .run_birthdays(
            &mut world(&only_events(""), &mut env, &POPS),
            date(768, 1, 1),
        )
        .expect("birthday");
    let after = get(&store, 1).life.clone();
    assert_eq!(
        (&before.focus, &before.use_factor),
        (&after.focus, &after.use_factor)
    );
}

#[test]
fn the_record_saves_loads_and_reads_back_from_the_archive() {
    let mut c = man(1, "a");
    c.life.grades.insert("poet".into(), 2);
    c.life.counters.insert("battles_survived".into(), 7);
    c.life.skills.insert("oratory".into(), 4321);
    c.life.use_factor.insert("oratory".into(), 777);
    c.life.focus = "learning".into();
    c.life.holds_land = true;
    let store = store_of(vec![c.clone()]);
    let bytes = store.to_bytes().expect("encode");
    let loaded = CharacterStore::from_bytes(&bytes).expect("decode");
    assert_eq!(get(&loaded, 1).life, c.life);
    assert_eq!(loaded.to_bytes().expect("encode"), bytes);
    let archive_path = path("readback-arch");
    let mut archive = Archive::open(&archive_path).expect("archive");
    let mut loaded = loaded;
    loaded
        .state_die_of(1, date(768, 1, 1), Some("fever"), &mut archive)
        .expect("death");
    let dead = archive.get(1).expect("archive").expect("archived");
    assert_eq!(dead.life.grades, c.life.grades);
    assert_eq!(dead.life.counters, c.life.counters);
    assert_eq!(dead.life.skills, c.life.skills);
    assert_eq!(dead.life.use_factor, c.life.use_factor);
    assert_eq!(
        (dead.life.focus.as_str(), dead.life.holds_land),
        ("learning", true)
    );
    assert_eq!(dead.life.death_cause.as_deref(), Some("fever"));
    // The archive reopens with the record intact.
    let reopened = Archive::open(&archive_path).expect("reopen");
    assert_eq!(reopened.get(1).expect("archive").expect("archived"), dead);
}

#[test]
fn a_run_of_events_is_deterministic() {
    let run = |tag: &str| {
        let mut store = store_of(vec![
            man(1, "a"),
            person(2, date(745, 1, 1), "a", Sex::Female),
        ]);
        let mut env = Env::new(tag);
        let mut world = world(shipped(), &mut env, &POPS);
        store
            .marry_with_events(&mut world, 1, 2, date(768, 1, 1))
            .expect("marriage");
        for day in 1..=20 {
            store
                .report_happening(&mut world, 1, "insulted", date(768, 2, day))
                .expect("insult");
            store
                .report_happening(&mut world, 2, "famine", date(768, 2, day))
                .expect("famine");
        }
        store
            .run_birthdays(&mut world, date(768, 6, 1))
            .expect("birthday");
        (
            store.to_bytes().expect("encode"),
            env.lines(1),
            env.lines(2),
        )
    };
    assert_eq!(run("det-a"), run("det-b"));
}

#[test]
fn the_new_sources_use_no_floating_point() {
    for file in ["events.rs", "life.rs", "skills.rs"] {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join(file),
        )
        .expect("source");
        for word in ["f32", "f64", "as f"] {
            assert!(!text.contains(word), "{file} mentions {word}");
        }
    }
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../economy/src/events.rs"),
    )
    .expect("source");
    for word in ["f32", "f64", "as f"] {
        assert!(!text.contains(word), "economy events.rs mentions {word}");
    }
}

// --- Review 1 ---------------------------------------------------------------------------------------------------------

#[test]
fn an_indulged_vice_cascades_through_precedence_the_same_day() {
    let mut c = monk_with(&["ascetic", "cele_de", "saintly"]);
    c.personality.vice[4] = 0;
    c.personality.vice[2] = 0;
    let mut store = store_of(vec![c]);
    let mut env = Env::new("indulge");
    store
        .indulge_with_events(
            &mut world(shipped(), &mut env, &POPS),
            1,
            "gluttony_temperance",
            date(768, 1, 1),
        )
        .expect("indulge");
    assert!(get(&store, 1).personality.vice[4] < 0);
    assert!(
        ["ascetic", "cele_de", "saintly"]
            .iter()
            .all(|key| get(&store, 1).grade(key) == 0)
    );
    assert_eq!(env.lines(1).len(), 3, "{:?}", env.lines(1));
}

#[test]
fn a_move_runs_precedence_because_the_pop_type_is_an_input() {
    let mut c = monk_with(&["ascetic"]);
    c.personality.vice[4] = 1000;
    c.personality.vice[2] = 1000;
    let mut store = store_of(vec![c]);
    let mut env = Env::new("move");
    store
        .move_to_with_events(
            &mut world(shipped(), &mut env, &POPS),
            1,
            locator("a"),
            date(768, 1, 1),
        )
        .expect("move");
    assert_eq!(
        get(&store, 1).grade("ascetic"),
        0,
        "ascetic needs a monk pop"
    );
    assert_eq!(env.lines(1), vec!["c1 lost ascetic.".to_owned()]);
}

#[test]
fn precedence_scans_in_the_authored_catalog_order_not_alphabetical() {
    // The catalog writes hospitaller before fennid; both fail here (land held, luxuries 400), so the order of the lines shows it.
    let mut c = man(1, "a");
    c.life.holds_land = true;
    c.life.grades.insert("fennid".into(), 1);
    c.life.grades.insert("hospitaller".into(), 1);
    let mut store = store_of(vec![c]);
    let mut env = Env::new("authored");
    let poor = Pops { luxuries: 400 };
    store
        .apply_precedence(&mut world(shipped(), &mut env, &poor), 1, date(768, 1, 1))
        .expect("precedence");
    assert_eq!(
        env.lines(1),
        vec![
            "c1 lost hospitaller.".to_owned(),
            "c1 lost fennid.".to_owned()
        ]
    );
}

#[test]
fn a_chance_is_exact_when_an_intermediate_product_is_far_past_128_bits() {
    let pop = PopFacts {
        kind: "noble".into(),
        group: "gaelic".into(),
        luxuries_pm: 600,
    };
    let c = man(1, "a");
    let huge = "{ pop_type = { noble = 1000000000000 } }";
    let rules = |factors: &str| {
        only_events(&format!(
            "[[event]]\nid = \"big\"\ntrigger = \"happening:test\"\nchance = 1\nfactors = [{factors}]\neffects = []\ntext = \"{{name}}.\"\n"
        ))
    };
    // Three huge multipliers cap at 1.
    let capped = rules(&format!("{huge}, {huge}, {huge}"));
    assert_eq!(
        chance_ppb(&capped.events[0], &c, &pop).expect("chance"),
        1_000_000_000
    );
    // A late multiplier the subject's type lacks makes the whole product zero.
    let zero = rules(&format!(
        "{huge}, {huge}, {huge}, {{ pop_type = {{ monk = 1 }} }}"
    ));
    assert_eq!(chance_ppb(&zero.events[0], &c, &pop).expect("chance"), 0);
    // Late fractions bring a huge intermediate back down to an exact value: 1e36 (three huge multipliers) x 0.001^13 = 0.001.
    let tail = std::iter::repeat_n("{ pop_type = { noble = 0.001 } }", 13)
        .collect::<Vec<_>>()
        .join(", ");
    let reduced = rules(&format!("{huge}, {huge}, {huge}, {tail}"));
    assert_eq!(
        chance_ppb(&reduced.events[0], &c, &pop).expect("chance"),
        1_000_000
    );
}

#[test]
fn a_use_spread_near_the_64_bit_limit_clamps_the_draw_instead_of_failing() {
    let rules = edited(
        &[],
        &[("use_spread = 0.4", "use_spread = 9223372036854775.807")],
    );
    let mut store = CharacterStore::default();
    let ctx = Context {
        rules: &self::rules().genetics,
        personality: &self::rules().personality,
        events: &rules,
        lifecycle: &self::rules().lifecycle,
        seed: 7,
        tick: date(768, 1, 1),
    };
    for _ in 0..20 {
        let id = store
            .create_founder(
                &ctx,
                NewCharacter {
                    name: "x".into(),
                    sex: Sex::Male,
                    birth: date(740, 1, 1),
                    pop: locator("a"),
                },
                &irish(),
                &POPS,
            )
            .expect("a wide use spread draws and clamps");
        assert!(
            get(&store, id)
                .life
                .use_factor
                .values()
                .all(|factor| (0..=1000).contains(factor))
        );
    }
}

#[test]
fn a_representable_skill_level_survives_a_large_growth_rate() {
    // gain_rate 10000: a noble man at use 0.8 reaches 8,000,000 on the first training birthday; the next gives 7,424,018.
    let mut rules = edited(&[], &[("gain_rate = 3.0", "gain_rate = 10000.0")]);
    rules.events.clear();
    let mut c = person(1, date(750, 4, 4), "a", Sex::Male);
    use_factor_one(&mut c);
    let mut store = store_of(vec![c]);
    let mut env = Env::new("big-growth");
    store
        .run_birthdays(&mut world(&rules, &mut env, &POPS), date(768, 4, 4))
        .expect("birthday");
    assert_eq!(get(&store, 1).skill("martial_prowess"), 8_000_000);
    store
        .run_birthdays(&mut world(&rules, &mut env, &POPS), date(769, 4, 4))
        .expect("birthday");
    assert_eq!(get(&store, 1).skill("martial_prowess"), 7_424_018);
}

// --- Review 2 ---------------------------------------------------------------------------------------------------------

#[test]
fn a_later_yearly_event_sees_the_precedence_that_the_earlier_one_caused() {
    let rules = only_events(
        r#"
[[event]]
id = "drop"
trigger = "yearly"
when = { age = [15, 200] }
chance = 1
effects = [{ remove = "ascetic" }]
text = "{name} left the ascetic life."
[[event]]
id = "reward"
trigger = "yearly"
when = { has = ["cele_de"] }
chance = 1
effects = [{ standing = "fame", add = 1 }]
text = "{name} was rewarded."
"#,
    );
    let mut c = person(1, date(730, 4, 4), "m", Sex::Male);
    c.life.grades.insert("ascetic".into(), 1);
    c.life.grades.insert("cele_de".into(), 1);
    let mut store = store_of(vec![c]);
    let mut env = Env::new("yearly-cascade");
    store
        .run_birthdays(&mut world(&rules, &mut env, &POPS), date(768, 4, 4))
        .expect("birthday");
    assert_eq!(
        get(&store, 1).personality.fame,
        0,
        "the second event ran after cele_de fell"
    );
    assert_eq!(get(&store, 1).grade("cele_de"), 0);
    assert_eq!(
        env.lines(1),
        vec![
            "c1 left the ascetic life.".to_owned(),
            "c1 lost cele de.".to_owned()
        ]
    );
}

#[test]
fn a_level_education_produces_still_grows_at_the_next_birthday() {
    // Education rate 1, age factor 1, scholarship coefficient 1 and focus x10, the loader's ceilings: a pupil with scholarship 80,000
    // and a teacher at 80,000 martial prowess gains 80,000 x 80,001 x 10 points. The next birthday, at the training age, decays it.
    let mut rules = edited(
        &[],
        &[
            ("rate = 0.05 ", "rate = 1.0 "),
            (
                "[[0, 1.0], [14, 0.3], [25, 0.1]]",
                "[[0, 1.0], [14, 1.0], [25, 1.0]]",
            ),
            (
                "scholarship_per_point = 0.02",
                "scholarship_per_point = 1.0",
            ),
            ("on = 1.5", "on = 10.0"),
        ],
    );
    rules.events.clear();
    let mut teacher = person(1, date(700, 6, 1), "a", Sex::Male);
    teacher
        .life
        .skills
        .insert("martial_prowess".into(), 80_000_000);
    let mut pupil = person(2, date(762, 4, 4), "a", Sex::Male);
    pupil.life.skills.insert("scholarship".into(), 80_000_000);
    pupil.life.focus = "martial".into();
    use_factor_one(&mut pupil);
    let mut store = store_of(vec![teacher, pupil]);
    let mut env = Env::new("big-education");
    store
        .run_birthdays(&mut world(&rules, &mut env, &POPS), date(768, 4, 4))
        .expect("birthday");
    assert_eq!(get(&store, 2).skill("martial_prowess"), 64_000_800_000_000);
    store
        .run_birthdays(&mut world(&rules, &mut env, &POPS), date(769, 4, 4))
        .expect("a representable level grows and decays");
    assert_eq!(get(&store, 2).skill("martial_prowess"), 59_392_742_400_000);
}
