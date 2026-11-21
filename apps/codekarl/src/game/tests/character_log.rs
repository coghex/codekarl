//! The life log and the pop tab's data (CK-378): what `game.character_log` reads of a character's log as of one publication, its boundary, its lookup, the old
//! world's files across a load, and the shipped 768 start's pop tab and log through the shipped UI.

use std::{io::Write, sync::Arc};

use ck_characters::{CharacterStore, LifeLog, LogView};
use ck_lua::ConfigValue;
use ck_sim::GameTime;

use super::character_view::{man, on, pop, store_of, viewed};
use crate::{
    game::{character_storage::Storage, character_view::CharacterView},
    testing::Dir,
};

/// A scratch storage with a life log open for appending.
struct Logs {
    _dir: Dir,
    storage: Storage,
    log: LifeLog,
}

impl Logs {
    fn new() -> Self {
        let dir = Dir::new();
        let storage = Storage::empty(&dir.0).expect("storage");
        let log = LifeLog::open(storage.path.join("LOGS")).expect("a log");
        Self {
            _dir: dir,
            storage,
            log,
        }
    }

    fn append(&mut self, id: u64, date: GameTime, text: &str) {
        self.log.append(id, date, text).expect("appended");
    }

    /// The log as published now.
    fn view(&self) -> LogView {
        self.storage
            .log_source()
            .view()
            .expect("a view of the life log")
    }

    fn file(&self) -> std::path::PathBuf {
        self.storage.path.join("LOGS")
    }
}

/// A view of the live characters of `store` carrying `log`.
fn with_log(store: &CharacterStore, log: Option<LogView>) -> CharacterView {
    let mut view = viewed(store);
    view.log = log;
    view
}

fn texts(lines: &[(ck_sim::Date, String)]) -> Vec<&str> {
    lines.iter().map(|(_, text)| text.as_str()).collect()
}

#[test]
fn a_view_reads_exactly_the_records_inside_its_boundary_even_for_a_later_append_with_the_same_date()
{
    let noble = pop("noble", 0);
    let store = store_of(vec![man(1, &noble), man(2, &noble)]);
    let mut logs = Logs::new();
    logs.append(1, on(768, 5, 1), "first");
    logs.append(2, on(768, 5, 1), "other");
    let early = with_log(&store, Some(logs.view()));
    // A later append with the same date, and one with an earlier date: neither is in the earlier view, both are in the next.
    logs.append(1, on(768, 5, 1), "second");
    logs.append(1, on(768, 2, 1), "zero");
    let late = with_log(&store, Some(logs.view()));
    assert_eq!(texts(&early.log_of(1).unwrap()), ["first"]);
    assert_eq!(texts(&late.log_of(1).unwrap()), ["zero", "first", "second"]);
    // The earlier view, kept, still reads as it did.
    assert_eq!(texts(&early.log_of(1).unwrap()), ["first"]);
    assert_eq!(texts(&late.log_of(2).unwrap()), ["other"]);
}

#[test]
fn game_character_log_reads_each_revision_inside_its_own_boundary_for_a_later_append_with_the_same_date()
 {
    use crate::{
        game::view::EconomyView,
        game_api::{ViewCache, character_log_function},
    };

    let noble = pop("noble", 0);
    let store = store_of(vec![man(1, &noble)]);
    let mut logs = Logs::new();
    logs.append(1, on(768, 5, 1), "first");
    let early = with_log(&store, Some(logs.view()));
    // The append lands after revision 1 was published, with the same date; revision 2 is published after it.
    logs.append(1, on(768, 5, 1), "second");
    let late = with_log(&store, Some(logs.view()));
    let cache = Arc::new(ViewCache::default());
    for (revision, characters) in [(1, early), (2, late)] {
        cache.post(
            revision,
            Arc::new(EconomyView {
                characters,
                ..EconomyView::default()
            }),
        );
    }
    let function = character_log_function(Arc::clone(&cache));
    let read = |revision: i64| -> Vec<String> {
        let answer = function(&[
            Some(ConfigValue::Integer(1)),
            Some(ConfigValue::Integer(revision)),
        ])
        .expect("the read completes");
        let ConfigValue::Array(lines) = &answer[0] else {
            panic!("an array")
        };
        lines
            .iter()
            .map(|line| match line {
                ConfigValue::Table(fields) => match &fields["text"] {
                    ConfigValue::String(text) => text.clone(),
                    other => panic!("{other:?}"),
                },
                other => panic!("{other:?}"),
            })
            .collect()
    };
    // Through the host function, revision 1 does not see the later append even though it has the same date; revision 2 does.
    assert_eq!(read(1), ["first"]);
    assert_eq!(read(2), ["first", "second"]);
    assert_eq!(
        read(1),
        ["first"],
        "the earlier revision still reads as it did"
    );
}

#[test]
fn a_record_being_written_is_outside_the_boundary_until_it_is_whole() {
    let noble = pop("noble", 0);
    let store = store_of(vec![man(1, &noble)]);
    let mut logs = Logs::new();
    logs.append(1, on(768, 5, 1), "whole");
    // The bytes of a second record, cut at three points: inside its length, inside its body, one byte short.
    let mut other = Logs::new();
    other.append(1, on(768, 5, 2), "torn");
    let bytes = std::fs::read(other.file()).expect("its bytes");
    for cut in [2, 9, bytes.len() - 1] {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(logs.file())
            .expect("the file");
        file.write_all(&bytes[..cut]).expect("a part");
        let view = with_log(&store, Some(logs.view()));
        assert_eq!(
            texts(&view.log_of(1).unwrap()),
            ["whole"],
            "a record cut at {cut} bytes is not published"
        );
        // The write fails and the file is cut back to where the record began (as `LifeLog::append` does).
        let published = std::fs::metadata(logs.file()).unwrap().len() - cut as u64;
        std::fs::OpenOptions::new()
            .write(true)
            .open(logs.file())
            .unwrap()
            .set_len(published)
            .unwrap();
    }
    // The record written whole is in the next view.
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(logs.file())
        .expect("the file");
    file.write_all(&bytes[..bytes.len() - 1]).unwrap();
    let before = with_log(&store, Some(logs.view()));
    file.write_all(&bytes[bytes.len() - 1..]).unwrap();
    let after = with_log(&store, Some(logs.view()));
    assert_eq!(texts(&before.log_of(1).unwrap()), ["whole"]);
    assert_eq!(texts(&after.log_of(1).unwrap()), ["whole", "torn"]);
}

#[test]
fn a_read_touches_only_the_selected_characters_records() {
    let noble = pop("noble", 0);
    let store = store_of(vec![man(1, &noble), man(2, &noble)]);
    let mut logs = Logs::new();
    logs.append(1, on(768, 1, 1), "one a");
    logs.append(2, on(768, 1, 2), "two a");
    logs.append(1, on(768, 1, 3), "one b");
    logs.append(2, on(768, 1, 4), "two b");
    let log = logs.view();
    // The index holds the offsets of each character's own records, in the order written, and the read goes only to those.
    let one = log.offsets(1);
    let two = log.offsets(2);
    assert_eq!((one.len(), two.len()), (2, 2));
    assert!(one[0] < two[0] && two[0] < one[1] && one[1] < two[1]);
    assert!(log.offsets(3).is_empty());
    // The proof that nothing else is read: the other character's records are spoiled on disk after the view was made, and this character reads on
    // while the other's does not.
    let mut bytes = std::fs::read(logs.file()).expect("the file");
    for offset in &two {
        let at = usize::try_from(*offset).unwrap();
        let length = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        // The last byte of the record is the last of its text: not UTF-8 any more, so the record no longer decodes.
        bytes[at + 4 + length - 1] = 0xFF;
    }
    std::fs::write(logs.file(), &bytes).expect("spoiled");
    let view = with_log(&store, Some(log));
    assert_eq!(texts(&view.log_of(1).unwrap()), ["one a", "one b"]);
    assert!(
        view.log_of(2).is_err(),
        "the spoiled records are the ones read for 2"
    );
}

#[test]
fn a_line_dated_after_the_view_is_left_out_and_equal_dates_keep_the_order_written() {
    let noble = pop("noble", 0);
    let store = store_of(vec![man(1, &noble)]);
    let mut logs = Logs::new();
    logs.append(1, on(768, 10, 9), "today b");
    logs.append(1, on(768, 10, 10), "tomorrow");
    logs.append(1, on(768, 10, 9), "today c");
    logs.append(1, on(768, 3, 1), "spring");
    logs.append(1, on(769, 1, 1), "next year");
    // The view is built for 9 October 768: its day is the last one it shows.
    let view = with_log(&store, Some(logs.view()));
    let lines = view.log_of(1).unwrap();
    assert_eq!(texts(&lines), ["spring", "today b", "today c"]);
    assert_eq!(
        lines
            .iter()
            .map(|(date, _)| (date.month, date.day))
            .collect::<Vec<_>>(),
        [(3, 1), (10, 9), (10, 9)]
    );
}

#[test]
fn a_log_refuses_for_a_character_not_live_and_for_a_view_without_a_log() {
    let noble = pop("noble", 0);
    let store = store_of(vec![man(1, &noble)]);
    let mut logs = Logs::new();
    logs.append(1, on(768, 1, 1), "a line");
    logs.append(9, on(768, 1, 1), "a dead man's line");
    let view = with_log(&store, Some(logs.view()));
    assert!(view.log_of(9).is_err(), "not live at this revision");
    assert!(with_log(&store, None).log_of(1).is_err(), "no log");
    assert!(view.log_of(1).is_ok());
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "two real games, a real quicksave and quickload and the real function over both worlds, through the cache's retirement, in one story"
)]
fn a_real_load_keeps_the_old_worlds_log_readable_while_pinned_then_releases_it_and_the_new_world_reads_its_own()
 {
    use crate::{
        game::{
            controls::Control,
            core::{Message, Outputs},
            setup::test_core,
        },
        game_api::{ViewCache, character_log_function},
    };

    let (dir_a, a, _) = super::start_768::shipped(7);
    let (_dir_b, b, _) = super::start_768::shipped(8);
    let saves = dir_a.0.join("saves");
    let (path_a, path_b) = (a.storage.path.clone(), b.storage.path.clone());
    // One id live in both worlds whose log text differs: the same id names two lives.
    let log_a = LifeLog::open(path_a.join("LOGS")).expect("log a");
    let log_b = LifeLog::open(path_b.join("LOGS")).expect("log b");
    let (id, lines_a, lines_b) = a
        .simulation
        .state()
        .characters
        .iter()
        .filter(|c| b.simulation.state().characters.get(c.id).is_some())
        .find_map(|c| {
            let (x, y) = (log_a.read(c.id).ok()?, log_b.read(c.id).ok()?);
            let words = |lines: &[ck_characters::LogLine]| -> Vec<String> {
                lines.iter().map(|line| line.text.clone()).collect()
            };
            (!x.is_empty() && !y.is_empty() && words(&x) != words(&y))
                .then(|| (c.id, words(&x), words(&y)))
        })
        .expect("an id live in both worlds with different lives");
    let quiet = || Outputs {
        log: Box::new(|_, _| {}),
        publish: Box::new(|_| {}),
        fail: Box::new(|message| panic!("{message}")),
    };
    let mut core_a = test_core(a, saves.clone(), quiet());
    let mut core_b = test_core(b, saves, quiet());
    // World B is saved; A is loaded from that save: a real load with its storage and its view builder.
    core_b.dispatch(
        Message::Control(Control::Quicksave),
        std::time::Duration::ZERO,
    );
    let old = core_a.snapshot();
    assert_eq!(old.world, 0);
    let old_view = old.economy.clone().expect("a view");
    let read = |view: &crate::game::view::EconomyView| -> Vec<String> {
        view.characters
            .log_of(id)
            .expect("the log")
            .into_iter()
            .map(|(_, text)| text)
            .collect()
    };
    assert_eq!(read(&old_view), lines_a, "the old world's own log");
    core_a.dispatch(
        Message::Control(Control::Quickload),
        std::time::Duration::ZERO,
    );
    let new = core_a.snapshot();
    assert_eq!(new.world, 1, "the load replaced the world");
    let new_view = new.economy.clone().expect("a view");
    assert_eq!(read(&new_view), lines_b, "the new world reads its own");
    assert_eq!(read(&old_view), lines_a, "the pinned old view still reads");
    // Through the real function with the old revision pinned and the new one posted after it: each reads its own world's text.
    let cache = Arc::new(ViewCache::default());
    cache.post(old.economy_revision, Arc::clone(&old_view));
    cache.post(new.economy_revision, Arc::clone(&new_view));
    let function = character_log_function(Arc::clone(&cache));
    let first_text = |revision: u64| -> String {
        let answer = function(&[
            Some(ConfigValue::Integer(i64::try_from(id).unwrap())),
            Some(ConfigValue::Integer(i64::try_from(revision).unwrap())),
        ])
        .expect("the read completes");
        let ConfigValue::Array(lines) = &answer[0] else {
            panic!("an array")
        };
        let ConfigValue::Table(line) = &lines[0] else {
            panic!("a table")
        };
        match &line["text"] {
            ConfigValue::String(text) => text.clone(),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(first_text(old.economy_revision), lines_a[0]);
    assert_eq!(first_text(new.economy_revision), lines_b[0]);
    // The old world's files stay while a view of it lives. The snapshots and the local handles are let go: only the cache keeps the old view, as the
    // UI link's does until a later call completes.
    assert!(path_a.exists() && path_b.exists());
    let (old_revision, new_revision) = (old.economy_revision, new.economy_revision);
    drop((old, old_view, new, new_view));
    assert!(path_a.exists(), "the cache still holds the old view");
    // The call that carried the old revision completes: it is the last completed, so it still reads.
    cache.complete(old_revision);
    assert_eq!(first_text(old_revision), lines_a[0]);
    assert!(path_a.exists());
    // The call that carried the new revision completes: the old revision retires (the host refuses it by name), its storage is released, and the
    // new revision, still held, reads on.
    cache.complete(new_revision);
    let refusal = function(&[
        Some(ConfigValue::Integer(i64::try_from(id).unwrap())),
        Some(ConfigValue::Integer(i64::try_from(old_revision).unwrap())),
    ])
    .expect_err("the retired revision is refused");
    assert_eq!(
        refusal,
        format!("game.character_log: revision {old_revision} is retired")
    );
    assert!(
        !path_a.exists(),
        "the old world's storage is released when its revision retires"
    );
    assert!(
        path_b.exists(),
        "the game that was loaded from is still running"
    );
    assert_eq!(
        first_text(new_revision),
        lines_b[0],
        "the new world is still readable after the old one retired"
    );
    drop((function, cache));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the real start's data, the real functions and the shipped Lua in one witness"
)]
fn the_shipped_start_shows_niall_frossachs_noble_pop_at_ailech_and_his_stored_life_log() {
    use crate::{
        game::{setup::view_builder, view::EconomyView},
        game_api::{
            ViewCache, character_function, character_log_function, character_pop_function,
            characters_function, place_function, places_of,
        },
    };

    let (_dir, prepared, _) = super::start_768::shipped(7);
    let state = prepared.simulation.state();
    let date = prepared.simulation.date().expect("a date");
    let build = view_builder(prepared.view_wiring());
    let view: EconomyView = build(state, date);
    let niall = state
        .characters
        .iter()
        .find(|c| c.life.figure == "niall_frossach")
        .expect("Niall");
    assert_eq!(
        (niall.pop.place.as_str(), niall.pop.slot),
        ("ailech", 1),
        "his noble pop is the second pop of Ailech"
    );
    // What the economy holds of that pop, and who lives in it.
    let economy_pop = &state
        .economy
        .places
        .iter()
        .find(|place| place.id == "ailech")
        .expect("Ailech")
        .pops[1];
    assert_eq!(economy_pop.kind, "noble");
    let members: Vec<u64> = state
        .characters
        .iter()
        .filter(|c| c.pop == niall.pop)
        .map(|c| c.id)
        .collect();
    assert!(
        members.contains(&niall.id) && members.len() > 1,
        "{members:?}"
    );
    assert_eq!(
        view.characters.pop_members("ailech", 1).len(),
        members.len(),
        "the view lists the pop's live characters"
    );
    // His log: the stored dated lines (89 at seed 7), the same as the older reader finds in the file.
    let stored = LifeLog::open(prepared.storage.path.join("LOGS"))
        .expect("the log")
        .read(niall.id)
        .expect("his lines");
    assert_eq!(stored.len(), 89, "his stored lines at seed 7");
    let read = view.characters.log_of(niall.id).expect("his log");
    assert_eq!(read.len(), stored.len(), "none is dated after the start");
    assert_eq!(
        read.iter()
            .map(|(_, text)| text.clone())
            .collect::<Vec<_>>(),
        stored
            .iter()
            .map(|line| line.text.clone())
            .collect::<Vec<_>>()
    );
    let rank = view.characters.card(niall.id).expect("his card").importance as usize;
    let places = places_of(&prepared.scenario);
    let cache = Arc::new(ViewCache::default());
    cache.post(1, Arc::new(view));
    let id = i64::try_from(niall.id).expect("a small id");
    let arguments = [
        Some(ConfigValue::Integer(id)),
        Some(ConfigValue::Integer(1)),
    ];
    let cards =
        characters_function(Arc::clone(&cache))(&[Some(ConfigValue::Integer(1))]).expect("cards");
    let record = character_function(Arc::clone(&cache))(&arguments).expect("his record");
    let pop_answer = character_pop_function(Arc::clone(&cache))(&arguments).expect("his pop");
    let place = place_function(Arc::clone(&cache), places)(&[
        Some(ConfigValue::String("ailech".to_owned())),
        Some(ConfigValue::Integer(1)),
    ])
    .expect("his place");
    let log = character_log_function(cache)(&arguments).expect("his log");
    let lua = super::character_view::lua_of;
    let notes = crate::ui_link::location_tests::character_pop::real_pop_and_log(
        &lua(&cards[0]),
        &lua(&record[0]),
        &lua(&pop_answer[0]),
        &lua(&place[0]),
        &lua(&log[0]),
        id,
        rank,
    );
    // The tab shows his pop (Ailech, slot 1, the economy's numbers) and every character of it, most important first, Niall among them.
    let pop_line = notes
        .iter()
        .find(|line| line.starts_with("pop real "))
        .unwrap_or_else(|| panic!("{notes:?}"));
    assert!(
        pop_line.contains("place=ailech slot=1 revision=1")
            && pop_line.contains(&format!("workers={}", economy_pop.workers))
            && pop_line.contains(&format!("dependents={}", economy_pop.dependents)),
        "{pop_line}"
    );
    let shown: Vec<&str> = pop_line
        .rsplit("members=")
        .next()
        .unwrap()
        .split(',')
        .collect();
    assert_eq!(shown.len(), members.len(), "{pop_line}");
    assert!(shown.contains(&niall.id.to_string().as_str()), "{pop_line}");
    assert!(
        notes.iter().any(|line| line.starts_with("pop_items 4 ")),
        "{notes:?}"
    );
    // The life log opens with his stored lines.
    let opened = notes
        .iter()
        .find(|line| line.starts_with("log open "))
        .unwrap_or_else(|| panic!("{notes:?}"));
    assert!(opened.contains("lines=89 items=89 at_end=true"), "{opened}");
}
