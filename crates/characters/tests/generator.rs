//! The generator (CK-369, characters slice 7): the recipe, the generated past, heads, languages and the transaction.
//!
//! Characters are made through `generate` on the shipped rules and events: a fixed seed makes every outcome exact, a rule is checked on every
//! seed of a range, and a rate is held to its reference band over a large sample.

mod support;

use ck_characters::{
    CharacterStore, Error, EventError, Sex, UNKNOWN,
    generator::{Birth, GenerateRequest, Member},
};
use ck_economy::{events::ROLES, genetics::PPB};
use ck_sim::Canonical;
use support::*;

fn tara() -> Pops {
    Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000)
}
fn today() -> ck_sim::GameTime {
    date(768, 10, 9)
}
fn request(sex: Sex, from: ck_sim::GameTime, to: ck_sim::GameTime, key: &str) -> GenerateRequest {
    GenerateRequest::new(
        locator("tara", 0),
        sex,
        Birth::Between(from, to),
        key,
        today(),
    )
}

#[test]
fn a_birth_is_drawn_in_the_range_and_the_mother_is_alive_at_it_and_the_father_at_conception() {
    let (from, to) = (date(722, 1, 1), date(724, 12, 31));
    let (mut dead_fathers, mut dead_mothers, mut both_alive) = (0, 0, 0);
    for seed in 0..80 {
        let pops = tara();
        let mut env = Env::new("recipe");
        let mut store = CharacterStore::default();
        let made = store
            .generate(
                &mut world(shipped(), &mut env, &pops, seed),
                &request(Sex::Male, from, to, "k"),
            )
            .expect("generated");
        let c = store.get(made.id).expect("live").clone();
        assert!(c.birth >= from && c.birth <= to, "seed {seed}");
        assert!(made.output.is_none());
        let father = record(&store, &env, c.father);
        let mother = record(&store, &env, c.mother);
        assert_eq!((father.sex, mother.sex), (Sex::Male, Sex::Female));
        // Their own parents are the unknown character.
        for parent in [&father, &mother] {
            assert_eq!((parent.father, parent.mother), (UNKNOWN, UNKNOWN));
            assert_eq!(parent.pop, c.pop);
            assert_eq!(parent.spouses.len(), 1);
            assert_eq!(parent.children, [c.id]);
        }
        // Alive when they must be, and fertile at the birth.
        assert!(
            mother.death.is_none_or(|died| died > c.birth),
            "seed {seed}"
        );
        assert!(
            father
                .death
                .is_none_or(|died| died > months_before(c.birth, 9)),
            "seed {seed}"
        );
        assert!((15..=51).contains(&age_on(&mother, c.birth)), "seed {seed}");
        assert!((15..=80).contains(&age_on(&father, c.birth)), "seed {seed}");
        // Dead ones are archived with their death date and cause, no later than today; living ones are placed.
        for parent in [&father, &mother] {
            match parent.death {
                Some(died) => {
                    assert!(died <= today());
                    assert!(parent.life.death_cause.is_some());
                    assert!(store.get(parent.id).is_none() && env.archive.contains(parent.id));
                    assert!(made.archived.contains(&parent.id));
                }
                None => assert!(store.get(parent.id).is_some()),
            }
        }
        dead_fathers += i32::from(father.death.is_some());
        dead_mothers += i32::from(mother.death.is_some());
        both_alive += i32::from(father.death.is_none() && mother.death.is_none());
        // The lineage: the child's paternal line is its father's, its maternal line its mother's.
        assert_eq!(c.genetics.paternal, father.genetics.paternal);
        assert_eq!(c.genetics.maternal, mother.genetics.maternal);
        assert_whole(&store, &env, &pops);
    }
    assert!(
        dead_fathers > 0 && dead_mothers > 0 && both_alive > 0,
        "{dead_fathers} {dead_mothers} {both_alive}"
    );
}

#[test]
fn the_same_request_makes_the_same_character_and_another_seed_another() {
    let make = |seed: u64, key: &str| {
        let pops = tara();
        let mut env = Env::new("determinism");
        let mut store = CharacterStore::default();
        let made = store
            .generate(
                &mut world(shipped(), &mut env, &pops, seed),
                &request(Sex::Female, date(730, 1, 1), date(733, 12, 31), key),
            )
            .expect("generated");
        let ids = env.archived_ids();
        let lines = env.lines(made.id);
        (store.to_bytes().expect("encode"), ids, lines, made.id)
    };
    assert_eq!(make(5, "a"), make(5, "a"));
    assert_ne!(make(5, "a").0, make(6, "a").0);
    // The request's key is part of the identity of its dates.
    assert_ne!(make(5, "a").0, make(5, "b").0);
}

#[test]
fn a_generated_character_is_saved_loaded_and_read_back_from_the_archive() {
    let pops = tara();
    let mut env = Env::new("readback");
    let mut store = CharacterStore::default();
    let mut ids = Vec::new();
    for seed in 0..6 {
        let made = store
            .generate(
                &mut world(shipped(), &mut env, &pops, seed),
                &request(
                    Sex::Male,
                    date(690, 1, 1),
                    date(700, 12, 31),
                    &format!("k{seed}"),
                ),
            )
            .expect("generated");
        ids.push(made.id);
    }
    assert!(
        !env.archived_ids().is_empty(),
        "old characters have dead parents"
    );
    let reopened = ck_characters::Archive::open(env.archive_path()).expect("reopen");
    for id in env.archived_ids() {
        let held = reopened.get(id).expect("read").expect("held");
        assert!(held.death.is_some() && held.life.death_cause.is_some());
        assert_eq!(held, env.archive.get(id).expect("read").expect("held"));
    }
    let loaded = CharacterStore::from_bytes(&store.to_bytes().expect("encode")).expect("decode");
    assert_eq!(loaded, store);
    for id in ids {
        assert_eq!(loaded.get(id), store.get(id));
    }
    assert_whole(&store, &env, &pops);
}

#[test]
fn a_named_parent_is_used_and_lists_the_child_and_a_parent_of_the_wrong_sex_is_refused_with_nothing_made()
 {
    let pops = tara();
    let mut env = Env::new("named");
    let father = person(
        1,
        Sex::Male,
        date(720, 3, 3),
        locator("tara", 0),
        "uí_néill",
    );
    let mother = person(2, Sex::Female, date(728, 4, 4), locator("tara", 0), "");
    let mut store = store_of(vec![father, mother]);
    let mut asked = request(Sex::Female, date(750, 1, 1), date(750, 12, 31), "named");
    asked.father = 1;
    asked.mother = 2;
    let made = store
        .generate(&mut world(shipped(), &mut env, &pops, 3), &asked)
        .expect("generated");
    assert_eq!(made.created, [3]);
    let c = store.get(3).expect("live");
    assert_eq!((c.father, c.mother), (1, 2));
    // The child takes its father's house, and both parents list it.
    assert_eq!(c.life.house, "uí_néill");
    assert_eq!(store.get(1).expect("live").children, [3]);
    assert_eq!(store.get(2).expect("live").children, [3]);
    // A father who is a woman.
    let before = store.clone();
    asked.father = 2;
    let error = store
        .generate(&mut world(shipped(), &mut env, &pops, 3), &asked)
        .unwrap_err();
    assert!(
        matches!(error, EventError::Rules(Error::WrongSex)),
        "{error:?}"
    );
    assert_eq!(store, before);
    assert_whole(&store, &env, &pops);
}

#[test]
fn a_role_is_stored_and_is_not_an_office_and_a_role_outside_the_vocabulary_is_refused() {
    let pops = tara();
    let mut env = Env::new("role");
    let mut store = CharacterStore::default();
    let mut asked = request(Sex::Male, date(730, 1, 1), date(735, 12, 31), "role");
    asked.role = Some("steward".into());
    asked.office = Some(ck_characters::Office::HighCleric);
    let made = store
        .generate(&mut world(shipped(), &mut env, &pops, 1), &asked)
        .expect("generated");
    let c = store.get(made.id).expect("live");
    assert_eq!(c.life.role, "steward");
    assert_eq!(c.office, Some(ck_characters::Office::HighCleric));
    asked.role = Some("cardinal".into());
    let before = store.clone();
    let error = store
        .generate(&mut world(shipped(), &mut env, &pops, 1), &asked)
        .unwrap_err();
    assert!(matches!(error, EventError::Rules(Error::Role)), "{error:?}");
    assert_eq!(store, before);
    // Every role of the vocabulary is accepted.
    for (n, role) in ROLES.iter().enumerate() {
        asked.role = Some((*role).to_owned());
        asked.key = format!("role{n}");
        store
            .generate(&mut world(shipped(), &mut env, &pops, 1), &asked)
            .unwrap_or_else(|error| panic!("{role}: {error:?}"));
    }
}

#[test]
fn a_fatal_yearly_event_in_a_generated_past_is_skipped_and_others_still_apply() {
    // `doom` kills every character every year; `gift` counts a deed; the battle happening counts a battle.
    let events = events_from(
        r#"
[[event]]
id = "doom"
trigger = "yearly"
chance = 1
effects = [{ die = "doom" }]
text = "{name} died."

[[event]]
id = "gift"
trigger = "yearly"
chance = 1
effects = [{ count = "duels_won", add = 1 }]
text = "{name} had a year."

[[event]]
id = "battle_survived"
trigger = "happening:battle_survived"
chance = 1
effects = [{ count = "battles_survived", add = 1 }]
text = "{name} fought."
"#,
    );
    let mut data = shipped().clone();
    let ppb = PPB;
    for chance in data.generator.past.battles.values_mut() {
        *chance = ppb;
    }
    // No other happening acts: the battle and the fatal event are the subject.
    for chance in data.generator.past.happenings.values_mut() {
        *chance = 0;
    }
    data.generator.past.new_friend_ppb = 0;
    data.generator.past.raids_noble_m_ppb = 0;
    data.generator.past.raids_martial_focus_ppb = 0;
    data.generator.past.duel_chance_ppb = 0;
    let pops = tara();
    let mut env = Env::new("fatal");
    let mut store = CharacterStore::default();
    // Sixty-nine today (the 70th birthday is the day after): its birthdays at 15..=69 are yearly checks, those at 16..=55 are battles.
    let man = store
        .generate(
            &mut world_with(events, &data, &mut env, &pops, 11),
            &request(Sex::Male, date(698, 10, 10), date(698, 10, 10), "man"),
        )
        .expect("generated");
    let c = store
        .get(man.id)
        .expect("a character whose past never kills lives");
    assert!(c.death.is_none() && !env.archive.contains(man.id));
    let lines = env.lines(man.id);
    assert!(lines.iter().all(|line| !line.contains("died")), "{lines:?}");
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("had a year"))
            .count(),
        55
    );
    assert_eq!(c.life.counters["duels_won"], 55);
    assert_eq!(
        c.life.counters["battles_survived"], 40,
        "battles only from 16 to 55"
    );
    // A woman fights no battle; a man of 29 fights from 16 to 29.
    let woman = store
        .generate(
            &mut world_with(events, &data, &mut env, &pops, 11),
            &request(Sex::Female, date(698, 10, 10), date(698, 10, 10), "woman"),
        )
        .expect("generated");
    assert!(
        !store
            .get(woman.id)
            .expect("live")
            .life
            .counters
            .contains_key("battles_survived")
    );
    let young = store
        .generate(
            &mut world_with(events, &data, &mut env, &pops, 11),
            &request(Sex::Male, date(738, 10, 10), date(738, 10, 10), "young"),
        )
        .expect("generated");
    assert_eq!(
        store.get(young.id).expect("live").life.counters["battles_survived"],
        14
    );
    // No head was debited, and no birth booked: a generated live character represents someone already counted.
    assert_eq!((pops.deaths.get(), pops.births.get()), (0, 0));
}

#[test]
fn the_past_is_a_dated_life_log_that_ends_before_today_and_a_birth_after_today_is_refused() {
    let pops = tara();
    let mut env = Env::new("dated");
    let mut store = CharacterStore::default();
    let made = store
        .generate(
            &mut world(shipped(), &mut env, &pops, 7),
            &GenerateRequest::new(
                locator("tara", 0),
                Sex::Male,
                Birth::On(date(723, 4, 4)),
                "dated",
                today(),
            ),
        )
        .expect("generated");
    let lines = env.logs.read(made.id).expect("log");
    assert!(!lines.is_empty());
    // Each line is on a birthday (4 April), from the 15th to the 45th, and none is on or after today.
    for line in &lines {
        let d = line.date.date().expect("a date");
        assert_eq!((d.month, d.day), (4, 4), "{line:?}");
        assert!(line.date < today() && d.year >= 723 + 12, "{line:?}");
    }
    // A birth after today is refused with nothing made.
    let before = store.clone();
    let error = store
        .generate(
            &mut world(shipped(), &mut env, &pops, 7),
            &GenerateRequest::new(
                locator("tara", 0),
                Sex::Male,
                Birth::On(date(769, 1, 1)),
                "future",
                today(),
            ),
        )
        .unwrap_err();
    assert!(
        matches!(error, EventError::Rules(Error::Generator)),
        "{error:?}"
    );
    assert_eq!(store, before);
}

/// A life log that refuses every line.
struct Down;
impl ck_characters::LogSink for Down {
    fn append(&mut self, _: u64, _: ck_sim::GameTime, _: &str) -> std::io::Result<()> {
        Err(std::io::Error::other("the log is down"))
    }
}

#[test]
fn a_life_log_that_fails_after_the_commit_is_reported_and_does_not_undo_it() {
    let pops = tara();
    let mut archive = ck_characters::Archive::open(path("down-arch")).expect("archive");
    let mut logs = Down;
    let mut store = CharacterStore::default();
    let mut w = ck_characters::World {
        rules: &rules().events,
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle: shipped(),
        seed: 7,
        populations: &pops,
        archive: &mut archive,
        logs: &mut logs,
    };
    let made = store
        .generate(
            &mut w,
            &GenerateRequest::new(
                locator("tara", 0),
                Sex::Male,
                Birth::On(date(723, 4, 4)),
                "down",
                today(),
            ),
        )
        .expect("the commit stands");
    assert!(
        matches!(made.output, Some(EventError::Log(_))),
        "{:?}",
        made.output
    );
    assert!(store.get(made.id).is_some());
}

#[test]
fn a_slot_of_an_unknown_kind_is_refused_and_a_court_that_does_not_exist_too() {
    use ck_characters::generator::SlotRequest;
    let pops = tara();
    let mut env = Env::new("kinds");
    let mut store = CharacterStore::default();
    let mut asked = SlotRequest {
        key: "k".into(),
        kind: "squire".into(),
        court: locator("tara", 0),
        house: String::new(),
        count: 1,
        ages: None,
        today: today(),
    };
    let error = store
        .fill_slot(&mut world(shipped(), &mut env, &pops, 1), &asked)
        .unwrap_err();
    assert!(matches!(error, EventError::Rules(Error::Slot)), "{error:?}");
    asked.kind = "fosterling".into();
    asked.court = locator("nowhere", 0);
    let error = store
        .fill_slot(&mut world(shipped(), &mut env, &pops, 1), &asked)
        .unwrap_err();
    assert!(
        matches!(error, EventError::Rules(Error::MissingPop)),
        "{error:?}"
    );
    assert_eq!(store, CharacterStore::default());
}

#[test]
fn a_pop_with_no_head_to_spare_refuses_the_generation_and_nothing_remains() {
    let mut env = Env::new("floor");
    // One head, held already by a character of the pop.
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1);
    let held = person(1, Sex::Male, date(740, 1, 1), locator("tara", 0), "");
    let mut store = store_of(vec![held]);
    let before = store.clone();
    let error = store
        .generate(
            &mut world(shipped(), &mut env, &pops, 2),
            &request(Sex::Male, date(745, 1, 1), date(745, 12, 31), "full"),
        )
        .unwrap_err();
    assert!(
        matches!(error, EventError::Rules(Error::PopFull)),
        "{error:?}"
    );
    assert_eq!(store, before);
    assert!(env.archived_ids().is_empty());
    assert!(env.lines(2).is_empty());
    // The next identifier is the one a fresh generation would have had.
    pops.set_heads("tara", 0, 10);
    let made = store
        .generate(
            &mut world(shipped(), &mut env, &pops, 2),
            &request(Sex::Male, date(745, 1, 1), date(745, 12, 31), "full"),
        )
        .expect("generated");
    assert!(made.created.contains(&made.id) && made.created.iter().all(|id| *id >= 2));
}

#[test]
fn an_archived_parent_takes_no_head() {
    // One head and no characters: it fits the child only when both parents are dead.
    let (mut fits, mut refused) = (0, 0);
    for seed in 0..60 {
        let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1);
        let mut env = Env::new("headless");
        let mut store = CharacterStore::default();
        let result = store.generate(
            &mut world(shipped(), &mut env, &pops, seed),
            &request(Sex::Male, date(715, 1, 1), date(718, 12, 31), "old"),
        );
        match result {
            Ok(made) => {
                assert_eq!(made.created.len(), 1, "seed {seed}");
                assert_eq!(made.archived.len(), 2, "seed {seed}");
                assert_eq!(store.pop_count(&locator("tara", 0)), 1);
                fits += 1;
            }
            Err(error) => {
                assert!(
                    matches!(error, EventError::Rules(Error::PopFull)),
                    "{error:?}"
                );
                assert_eq!(store, CharacterStore::default());
                assert!(env.archived_ids().is_empty());
                refused += 1;
            }
        }
    }
    assert!(fits > 0 && refused > 0, "{fits} {refused}");
}

/// A request for a man whose two dead parents are both archived, so that two archive records are published together.
fn old_man() -> GenerateRequest {
    request(Sex::Male, date(690, 1, 1), date(692, 12, 31), "old")
}
/// A seed for which `old_man` has two dead parents.
fn seed_with_two_dead_parents() -> u64 {
    (0..80)
        .find(|seed| {
            let pops = tara();
            let mut env = Env::new("probe");
            let mut store = CharacterStore::default();
            store
                .generate(&mut world(shipped(), &mut env, &pops, *seed), &old_man())
                .is_ok_and(|made| made.archived.len() == 2)
        })
        .expect("a seed with two dead parents")
}
fn fail_after(first: u32, batch: bool, tag: &str) -> FailAfter {
    FailAfter {
        inner: ck_characters::Archive::open(path(tag)).expect("archive"),
        ok: std::cell::Cell::new(first),
        batch,
    }
}

#[test]
fn a_refused_batch_leaves_the_archive_and_the_store_as_they_were_and_the_retry_links_everyone() {
    let seed = seed_with_two_dead_parents();
    let pops = tara();
    let mut archive = fail_after(1, true, "batch-arch");
    let mut logs = ck_characters::LifeLog::open(path("batch-logs")).expect("logs");
    let mut store = CharacterStore::default();
    {
        let mut w = ck_characters::World {
            rules: &rules().events,
            personality: &rules().personality,
            genetics: &rules().genetics,
            lifecycle: shipped(),
            seed,
            populations: &pops,
            archive: &mut archive,
            logs: &mut logs,
        };
        let error = store.generate(&mut w, &old_man()).unwrap_err();
        assert!(matches!(error, EventError::Archive(_)), "{error:?}");
    }
    // Nothing of the rejected family was published, and the live state is as it was.
    assert_eq!(store, CharacterStore::default());
    assert_eq!(archive.inner.ids().count(), 0);
    assert_whole_in(&store, &archive.inner, &pops);
    // The retry makes the same family, and every spouse and child link resolves both ways.
    archive.ok.set(10);
    let mut w = ck_characters::World {
        rules: &rules().events,
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle: shipped(),
        seed,
        populations: &pops,
        archive: &mut archive,
        logs: &mut logs,
    };
    let made = store.generate(&mut w, &old_man()).expect("the retry");
    assert_eq!(made.archived.len(), 2);
    let child = store.get(made.id).expect("live").clone();
    for parent in [child.father, child.mother] {
        let p = record_in(&store, &archive.inner, parent);
        assert_eq!(p.children, [child.id], "{parent} names its child");
        assert_eq!(p.spouses.len(), 1);
        let spouse = record_in(&store, &archive.inner, p.spouses[0]);
        assert_eq!(spouse.spouses, [p.id], "spouses name each other");
    }
    assert_whole_in(&store, &archive.inner, &pops);
}

#[test]
fn an_archive_that_cannot_publish_a_batch_is_refused_before_the_first_of_several_writes() {
    let seed = seed_with_two_dead_parents();
    let pops = tara();
    let mut archive = fail_after(10, false, "nobatch-arch");
    let mut logs = ck_characters::LifeLog::open(path("nobatch-logs")).expect("logs");
    let mut store = CharacterStore::default();
    {
        let mut w = ck_characters::World {
            rules: &rules().events,
            personality: &rules().personality,
            genetics: &rules().genetics,
            lifecycle: shipped(),
            seed,
            populations: &pops,
            archive: &mut archive,
            logs: &mut logs,
        };
        let error = store.generate(&mut w, &old_man()).unwrap_err();
        assert!(matches!(error, EventError::Archive(_)), "{error:?}");
    }
    assert_eq!(store, CharacterStore::default());
    assert_eq!(archive.inner.ids().count(), 0);
    assert_whole_in(&store, &archive.inner, &pops);
    // A character with a single dead parent needs no batch and is published.
    let single = request(Sex::Male, date(715, 1, 1), date(718, 12, 31), "one");
    let published = (0..60).any(|seed| {
        let mut store = CharacterStore::default();
        let mut archive = fail_after(10, false, "nobatch-one");
        let mut w = ck_characters::World {
            rules: &rules().events,
            personality: &rules().personality,
            genetics: &rules().genetics,
            lifecycle: shipped(),
            seed,
            populations: &pops,
            archive: &mut archive,
            logs: &mut logs,
        };
        store
            .generate(&mut w, &single)
            .is_ok_and(|made| made.archived.len() == 1)
    });
    assert!(published);
}

#[test]
fn the_archive_publishes_a_batch_whole_or_not_at_all() {
    let mut env = Env::new("batch-whole");
    let dead = |id: u64| {
        let mut c = person(id, Sex::Male, date(700, 1, 1), locator("tara", 0), "");
        c.death = Some(date(760, 1, 1));
        c
    };
    ck_characters::ArchiveSink::append(&mut env.archive, &dead(1)).expect("one");
    // A batch with a record that is already archived is refused whole: neither of its records is readable, and the file is as it was.
    let size = std::fs::metadata(env.archive_path()).expect("file").len();
    let error =
        ck_characters::ArchiveSink::append_all(&mut env.archive, &[dead(2), dead(1)]).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(
        std::fs::metadata(env.archive_path()).expect("file").len(),
        size
    );
    assert!(!env.archive.contains(2));
    // A batch that repeats an id, or holds a live record, is refused too.
    assert!(ck_characters::ArchiveSink::append_all(&mut env.archive, &[dead(3), dead(3)]).is_err());
    let live = person(4, Sex::Male, date(700, 1, 1), locator("tara", 0), "");
    assert!(ck_characters::ArchiveSink::append_all(&mut env.archive, &[dead(5), live]).is_err());
    assert_eq!(env.archived_ids(), [1]);
    // A good batch is all readable, also after the archive is opened again.
    ck_characters::ArchiveSink::append_all(&mut env.archive, &[dead(6), dead(7), dead(8)])
        .expect("batch");
    let reopened = ck_characters::Archive::open(env.archive_path()).expect("reopen");
    assert_eq!(reopened.ids().collect::<Vec<_>>(), [1, 6, 7, 8]);
    for id in [6, 7, 8] {
        assert_eq!(reopened.get(id).expect("read").expect("held").id, id);
    }
}

/// A dead character for the archive tests.
fn dead(id: u64) -> ck_characters::Character {
    let mut c = person(id, Sex::Male, date(700, 1, 1), locator("tara", 0), "");
    c.death = Some(date(760, 1, 1));
    c
}

#[test]
fn a_torn_batch_is_never_published_and_the_next_publication_cleans_it_away() {
    use ck_characters::ArchiveSink;
    use std::io::Write;
    // The bytes of a two-record batch, as the archive writes it.
    let source = path("torn-source");
    let mut other = ck_characters::Archive::open(&source).expect("archive");
    other.append_all(&[dead(2), dead(3)]).expect("batch");
    let whole = std::fs::read(&source).expect("bytes");
    // A batch cut anywhere short of its end (inside the length, inside the header, at the header, inside a record) leaves what a refused write
    // followed by a failed truncate leaves: the published records stay readable and the torn bytes are not published.
    for cut in [2, 9, 16, 20, whole.len() - 1] {
        let target = path("torn-target");
        let mut archive = ck_characters::Archive::open(&target).expect("archive");
        archive.append(&dead(1)).expect("one");
        let published = std::fs::metadata(&target).expect("file").len();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&target)
            .expect("file");
        file.write_all(&whole[..cut]).expect("torn tail");
        drop(file);
        let reopened = ck_characters::Archive::open(&target).expect("a torn tail is not an error");
        assert_eq!(reopened.ids().collect::<Vec<_>>(), [1], "cut {cut}");
        assert!(reopened.get(1).expect("read").is_some());
        // The archive that held it open when it happened publishes next, and so does one that opened it afterwards; each cuts the tail first.
        for mut next in [archive, reopened] {
            let fresh = path("torn-copy");
            std::fs::copy(&target, &fresh).expect("copy");
            let mut again = ck_characters::Archive::open(&fresh).expect("open");
            again
                .append_all(&[dead(4), dead(5)])
                .expect("a clean retry");
            assert!(std::fs::metadata(&fresh).expect("file").len() > published);
            let last = ck_characters::Archive::open(&fresh).expect("reopen");
            assert_eq!(last.ids().collect::<Vec<_>>(), [1, 4, 5], "cut {cut}");
            // The same on the handle that was there when the tail was torn.
            next.append_all(&[dead(6), dead(7)]).expect("a clean retry");
            let after = ck_characters::Archive::open(&target).expect("reopen");
            assert_eq!(after.ids().collect::<Vec<_>>(), [1, 6, 7], "cut {cut}");
            std::fs::copy(&target, path("torn-restore")).expect("copy");
            // Back to the torn state for the other handle.
            std::fs::OpenOptions::new()
                .write(true)
                .open(&target)
                .expect("file")
                .set_len(published)
                .expect("cut");
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&target)
                .expect("file");
            file.write_all(&whole[..cut]).expect("torn tail");
        }
    }
}

#[test]
fn a_corrupt_batch_header_is_refused_not_mistaken_for_a_torn_write() {
    use ck_characters::ArchiveSink;
    let target = path("corrupt-frame");
    let mut archive = ck_characters::Archive::open(&target).expect("archive");
    archive.append_all(&[dead(1), dead(2)]).expect("batch");
    let mut bytes = std::fs::read(&target).expect("bytes");
    // The count of the frame, changed: its checksum no longer holds.
    bytes[4] ^= 1;
    std::fs::write(&target, &bytes).expect("corrupt");
    assert!(ck_characters::Archive::open(&target).is_err());
}

#[test]
fn a_stale_archive_handle_is_refused_and_never_deletes_what_another_handle_committed() {
    use ck_characters::ArchiveSink;
    let target = path("stale");
    let mut first = ck_characters::Archive::open(&target).expect("archive");
    first.append(&dead(1)).expect("one");
    // A second handle to the same file, opened after record 1: it knows only that.
    let mut second = ck_characters::Archive::open(&target).expect("second handle");
    first
        .append_all(&[dead(2), dead(3)])
        .expect("committed by the first");
    let committed = std::fs::read(&target).expect("bytes");
    // The stale handle cannot publish: what follows what it read is a whole frame, not a torn write, and is not its to remove.
    let error = second.append_all(&[dead(4), dead(5)]).unwrap_err();
    assert!(error.to_string().contains("changed since"), "{error}");
    assert!(second.append(&dead(6)).is_err());
    assert_eq!(
        std::fs::read(&target).expect("bytes"),
        committed,
        "the committed bytes are untouched"
    );
    let reopened = ck_characters::Archive::open(&target).expect("reopen");
    assert_eq!(reopened.ids().collect::<Vec<_>>(), [1, 2, 3]);
    // A handle opened afresh publishes.
    let mut fresh = reopened;
    fresh
        .append_all(&[dead(4), dead(5)])
        .expect("a current handle");
    assert_eq!(
        ck_characters::Archive::open(&target)
            .expect("reopen")
            .ids()
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
}

#[test]
fn a_truncated_legacy_length_is_refused_and_only_the_start_of_a_frame_marker_is_a_torn_write() {
    use ck_characters::ArchiveSink;
    use std::io::Write;
    let tail = |bytes: &[u8], tag: &str| {
        let target = path(tag);
        let mut archive = ck_characters::Archive::open(&target).expect("archive");
        archive.append(&dead(1)).expect("one");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&target)
            .expect("file")
            .write_all(bytes)
            .expect("tail");
        ck_characters::Archive::open(&target)
    };
    // The first three bytes of a second legacy record's length are a truncated record, as ever.
    assert!(tail(&[0xc1, 0x02, 0x00], "legacy-cut").is_err());
    assert!(tail(&[0x01], "one-byte").is_err());
    // The first one to three bytes of the frame marker are what a torn frame can leave.
    for torn in [&[0xf0][..], &[0xf0, 0xff][..], &[0xf0, 0xff, 0xff][..]] {
        let reopened = tail(torn, "marker-prefix").expect("a torn frame marker is not an error");
        assert_eq!(reopened.ids().collect::<Vec<_>>(), [1]);
    }
}

#[test]
fn a_mother_is_not_archived_on_the_day_her_child_is_born_when_that_is_today() {
    let pops = tara();
    let mut env = Env::new("birthday-mother");
    let mut store = CharacterStore::default();
    // Every character is certain to die from its first year: a roll in an empty window would archive her.
    let mut lifecycle = shipped().clone();
    lifecycle.death.certain_at = 1;
    let asked = GenerateRequest::new(
        locator("tara", 0),
        Sex::Male,
        Birth::On(today()),
        "today",
        today(),
    );
    let made = store
        .generate(&mut world(&lifecycle, &mut env, &pops, 0), &asked)
        .expect("generated");
    let child = store.get(made.id).expect("live");
    assert_eq!(record(&store, &env, child.mother).death, None);
}

#[test]
fn generated_parents_in_a_monk_pop_are_clerics_with_children_straight_or_bisexual() {
    use ck_characters::Orientation;
    let pops = Pops::new().pop("tara", 0, "monk", "ulaid", 1_000_000);
    // Weights that favour gay heavily: only the clergy-with-children rule keeps a parent straight.
    let mut lifecycle = shipped().clone();
    lifecycle.connections.orientation.weights = [1, 100, 0, 0];
    for seed in 0..8 {
        let mut env = Env::new("monk-parents");
        let mut store = CharacterStore::default();
        let asked = GenerateRequest::new(
            locator("tara", 0),
            Sex::Male,
            Birth::On(today()),
            "orientation",
            today(),
        );
        let made = store
            .generate(&mut world(&lifecycle, &mut env, &pops, seed), &asked)
            .expect("generated");
        let child = store.get(made.id).expect("live");
        for parent in [child.father, child.mother] {
            // Live or archived, the parent is read back from where it is kept.
            let ancestor = record(&store, &env, parent);
            assert_eq!(
                ancestor.ties.orientation,
                Orientation::Straight,
                "seed {seed}"
            );
        }
    }
}

#[test]
fn a_candidate_log_failure_reaches_the_birthday_that_asked_for_it() {
    let pops = tara();
    let mut archive = ck_characters::Archive::open(path("candidate-arch")).expect("archive");
    let mut logs = Down;
    let mut data = shipped().clone();
    data.marriage.wish.men = vec![(12, PPB.into())];
    // Nobody accepts: the birthday ends after the candidates are made.
    data.marriage.accept.base_ppb = 0;
    data.marriage.accept.per_score_ppb = 0;
    let events = events_from(
        r#"
[[event]]
id = "candidate_line"
trigger = "yearly"
chance = 1
when = { sex = "f" }
effects = [{ stress = 1 }]
text = "{name} has a memory."
"#,
    );
    let mut w = ck_characters::World {
        rules: events,
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle: &data,
        seed: 0,
        populations: &pops,
        archive: &mut archive,
        logs: &mut logs,
    };
    let mut store = store_of(vec![person(
        1,
        Sex::Male,
        date(728, 10, 9),
        locator("tara", 0),
        "",
    )]);
    let result = store.run_birthdays(&mut w, today());
    assert!(matches!(result, Err(EventError::Log(_))), "{result:?}");
    // The candidates stand.
    assert!(store.iter().any(|c| c.sex == Sex::Female && c.id > 1));
}

#[test]
fn a_monk_who_gains_latinist_in_the_past_gets_the_higher_latin_and_the_role_alone_the_lower() {
    let events = events_from(
        r#"
[[event]]
id = "learned"
trigger = "yearly"
chance = 1
effects = [{ grant = "latinist" }]
text = "{name} learned Latin."
"#,
    );
    let pops = Pops::new().pop("clonard", 0, "monk", "ulaid", 1_000_000);
    let mut env = Env::new("latin");
    let mut store = CharacterStore::default();
    let build = |store: &mut CharacterStore, env: &mut Env, quiet: bool, key: &str| {
        let events = if quiet { &rules().events } else { events };
        let mut asked = GenerateRequest::new(
            locator("clonard", 0),
            Sex::Male,
            Birth::On(date(723, 5, 5)),
            key,
            today(),
        );
        asked.role = Some("monk".into());
        let made = store
            .generate(&mut world_with(events, shipped(), env, &pops, 4), &asked)
            .expect("generated");
        store.get(made.id).expect("live").clone()
    };
    let learned = build(&mut store, &mut env, false, "learned");
    assert!(learned.grade("latinist") > 0);
    let latin = learned.life.languages["latin"];
    let (monk, latinist) = (
        shipped().generator.languages.latin_by_office["monk"],
        shipped().generator.languages.latinist,
    );
    assert!((latinist.low..=latinist.high).contains(&latin), "{latin}");
    assert!(latin > monk.high, "the higher of the two sources stands");
    // The same character without the craft has only what its role gives, and Irish is its mother tongue, never a second language.
    let plain = build(&mut store, &mut env, true, "plain");
    assert_eq!(plain.grade("latinist"), 0);
    let latin = plain.life.languages["latin"];
    assert!((monk.low..=monk.high).contains(&latin), "{latin}");
    assert!(!plain.life.languages.contains_key("old_irish"));
    assert_eq!(
        (plain.life.tongue.as_str(), plain.life.dialect.as_str()),
        ("old_irish", "irish")
    );
    // A role with no Latin has none (in a world with no events, in which no craft is gained).
    let mut asked = GenerateRequest::new(
        locator("clonard", 0),
        Sex::Male,
        Birth::On(date(723, 5, 5)),
        "herd",
        today(),
    );
    asked.role = Some("herdsman".into());
    let quiet = events_from("");
    let made = store
        .generate(
            &mut world_with(quiet, shipped(), &mut env, &pops, 4),
            &asked,
        )
        .expect("generated");
    assert!(store.get(made.id).expect("live").life.languages.is_empty());
}

#[test]
fn a_mother_of_another_tongue_gives_her_language_at_the_data_strength() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000).pop(
        "york",
        0,
        "noble",
        "northumbrian",
        1_000_000,
    );
    let mut env = Env::new("mother");
    let mother = {
        let mut m = person(1, Sex::Female, date(728, 4, 4), locator("york", 0), "");
        "old_english".clone_into(&mut m.life.tongue);
        "northumbrian".clone_into(&mut m.life.dialect);
        m
    };
    let father = person(2, Sex::Male, date(720, 3, 3), locator("tara", 0), "");
    let mut store = store_of(vec![mother, father]);
    let mut asked = request(Sex::Male, date(750, 1, 1), date(750, 12, 31), "mixed");
    asked.father = 2;
    asked.mother = 1;
    let made = store
        .generate(&mut world(shipped(), &mut env, &pops, 9), &asked)
        .expect("generated");
    let c = store.get(made.id).expect("live");
    assert_eq!(c.life.tongue, "old_irish");
    let range = shipped().generator.languages.foreign_mother;
    assert!((range.low..=range.high).contains(&c.life.languages["old_english"]));
    // A mother of the same tongue gives nothing.
    let mut asked = request(Sex::Male, date(750, 1, 1), date(750, 12, 31), "same");
    asked.father = 2;
    let own = store
        .generate(&mut world(shipped(), &mut env, &pops, 9), &asked)
        .expect("generated");
    assert!(store.get(own.id).expect("live").life.languages.is_empty());
}

#[test]
fn a_generated_cleric_with_children_is_straight_or_bisexual_and_clergy_weights_apply_in_a_monk_pop()
{
    use ck_characters::Orientation;
    let pops = Pops::new().pop("clonard", 0, "monk", "ulaid", 1_000_000);
    let mut env = Env::new("clergy");
    let mut store = CharacterStore::default();
    let mut tally = [0_u32; 4];
    for n in 0..300 {
        let mut asked = GenerateRequest::new(
            locator("clonard", 0),
            Sex::Male,
            Birth::On(date(723, 5, 5)),
            &format!("p{n}"),
            today(),
        );
        asked.has_children = true;
        let made = store
            .generate(&mut world(shipped(), &mut env, &pops, 8), &asked)
            .expect("generated");
        let o = store.get(made.id).expect("live").ties.orientation;
        assert!(
            matches!(o, Orientation::Straight | Orientation::Bisexual),
            "{o:?}"
        );
        tally[usize::from(o == Orientation::Bisexual)] += 1;
    }
    assert!(tally[0] > 0 && tally[1] > 0, "{tally:?}");
}

#[test]
fn members_of_a_slot_are_generated_into_its_pop_with_their_role_in_one_transaction() {
    let pops = Pops::new().pop("clonard", 0, "monk", "ulaid", 1_000_000);
    let mut env = Env::new("members");
    let mut store = CharacterStore::default();
    let member = |role: &str, sex: Sex, n: u32| Member {
        role: role.to_owned(),
        sex,
        birth: Birth::Between(date(710, 1, 1), date(730, 12, 31)),
        house: String::new(),
        office: None,
        key: format!("{role}.{n}"),
    };
    let members = [
        member("abbot", Sex::Male, 0),
        member("scribe", Sex::Male, 1),
        member("nun", Sex::Female, 2),
    ];
    let made = store
        .fill_members(
            &mut world(shipped(), &mut env, &pops, 6),
            &locator("clonard", 0),
            &members,
            today(),
        )
        .expect("generated");
    assert_eq!(made.ids.len(), 3);
    for (id, member) in made.ids.iter().zip(&members) {
        let c = store.get(*id).expect("live");
        assert_eq!(c.life.role, member.role);
        assert_eq!(c.pop, locator("clonard", 0));
        assert_ne!(c.father, UNKNOWN);
    }
    // Latin by office: an abbot's is in his range.
    let abbot = shipped().generator.languages.latin_by_office["abbot"];
    let latin = store.get(made.ids[0]).expect("live").life.languages["latin"];
    assert!((abbot.low..=abbot.high).contains(&latin));
    // A refused member leaves nothing of the others.
    let before = store.clone();
    let mut bad = members.to_vec();
    bad.push(member("cardinal", Sex::Male, 3));
    assert!(
        store
            .fill_members(
                &mut world(shipped(), &mut env, &pops, 6),
                &locator("clonard", 0),
                &bad,
                today()
            )
            .is_err()
    );
    assert_eq!(store, before);
    assert_whole(&store, &env, &pops);
}

/// The share of `of` among `sample`, in ppb.
fn share(
    sample: &[&ck_characters::Character],
    of: impl Fn(&ck_characters::Character) -> bool,
) -> u64 {
    sample.iter().filter(|c| of(c)).count() as u64 * u64::from(PPB) / sample.len() as u64
}
fn within(rate: u64, reference: ck_economy::events::Reference, what: &str) {
    let (target, band) = (u64::from(reference.rate_ppb), u64::from(reference.band_ppb));
    assert!(
        rate.abs_diff(target) <= band,
        "{what}: {} per mille is outside {} +- {}",
        rate / 1_000_000,
        target / 1_000_000,
        band / 1_000_000
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one sample of three thousand men and every reference rate read from it"
)]
fn the_reference_rates_hold_for_a_thousand_men_of_forty_five_in_each_class() {
    let pops = Pops::new()
        .pop("tara", 0, "noble", "ulaid", 100_000_000)
        .pop("clonard", 0, "monk", "ulaid", 100_000_000)
        .pop("farm", 0, "peasant", "ulaid", 100_000_000);
    let mut env = Env::new("reference");
    let mut store = CharacterStore::default();
    let mut make = |place: &str, n: u32| {
        let mut ids = Vec::new();
        for i in 0..n {
            let asked = GenerateRequest::new(
                locator(place, 0),
                Sex::Male,
                Birth::Between(date(722, 10, 10), date(723, 10, 9)),
                &format!("{place}{i}"),
                today(),
            );
            ids.push(
                store
                    .generate(&mut world(shipped(), &mut env, &pops, 768), &asked)
                    .expect("generated")
                    .id,
            );
        }
        ids
    };
    let noble = make("tara", 1000);
    let clergy = make("clonard", 1000);
    let peasant = make("farm", 1000);
    let pick = |ids: &[u64]| -> Vec<&ck_characters::Character> {
        ids.iter().map(|id| store.get(*id).expect("live")).collect()
    };
    let (noble, clergy, peasant) = (pick(&noble), pick(&clergy), pick(&peasant));
    let reference = &shipped().generator.reference;
    within(
        share(&noble, |c| c.grade("veteran") > 0),
        reference["veteran_noble_men_45"],
        "veteran noble men",
    );
    within(
        share(&clergy, |c| c.grade("scribe") > 0),
        reference["scribe_clergy_men_45"],
        "scribe clergy",
    );
    within(
        share(&clergy, |c| c.grade("latinist") > 0),
        reference["latinist_clergy_men_45"],
        "latinist clergy",
    );
    within(
        share(&noble, |c| c.grade("melancholy") > 0),
        reference["melancholy_noble_men_45"],
        "melancholy noble men",
    );
    within(
        share(&clergy, |c| c.grade("melancholy") > 0),
        reference["melancholy_clergy_men_45"],
        "melancholy clergy",
    );
    within(
        share(&noble, |c| c.grade("rage_fits") > 0),
        reference["rage_fits_noble_men_45"],
        "rage fits noble men",
    );
    within(
        share(&clergy, |c| c.grade("recluse") > 0),
        reference["recluse_clergy_men_45"],
        "recluse clergy",
    );
    within(
        share(&peasant, |c| c.grade("drunkard") > 0),
        reference["drunkard_peasant_men_45"],
        "drunkard peasant men",
    );
    let everyone: Vec<_> = noble
        .iter()
        .chain(&clergy)
        .chain(&peasant)
        .copied()
        .collect();
    within(
        share(&everyone, |c| c.grade("madness") > 0),
        reference["madness_any_men_45"],
        "madness",
    );
    // Orientation of lay characters.
    let lay: Vec<_> = noble.iter().chain(&peasant).copied().collect();
    let o = &shipped().generator.orientation;
    for (n, which) in [
        ck_characters::Orientation::Straight,
        ck_characters::Orientation::Gay,
        ck_characters::Orientation::Bisexual,
        ck_characters::Orientation::Asexual,
    ]
    .into_iter()
    .enumerate()
    {
        let rate = share(&lay, |c| c.ties.orientation == which);
        assert!(
            rate.abs_diff(u64::from(o.rates_ppb[n])) <= u64::from(o.band_ppb),
            "{which:?}: {rate}"
        );
    }
    assert_whole(&store, &env, &pops);
}

#[test]
fn the_generator_sources_use_no_floating_point() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/generator");
    for entry in std::fs::read_dir(dir).expect("sources") {
        let path = entry.expect("entry").path();
        let text = std::fs::read_to_string(&path).expect("source");
        assert!(
            text.contains("#![deny(clippy::float_arithmetic)]"),
            "{path:?}"
        );
        for line in text
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
        {
            assert!(
                !line.contains("f64") && !line.contains("f32"),
                "{path:?}: {line}"
            );
        }
    }
}
