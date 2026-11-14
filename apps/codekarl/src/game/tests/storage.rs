use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use ck_log::Level;
use ck_sim::{Scheduler, Simulation};

use super::{Harness, after, identity, start};
use crate::game::core::{SaveCall, SaveReply};
use crate::game::{controls::Control, saves::QUICKSAVE, slot::PauseReason};

fn guarded_load(harness: &mut Harness, filename: &str) -> Result<(), String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let controls = crate::game::controls::SimControls::new(sender);
    controls.guarded_load(filename.to_owned());
    let crate::game::core::Message::GuardedLoad { filename, reply } = receiver.recv().unwrap()
    else {
        panic!("guarded load uses the ordered core message route");
    };
    harness.core.dispatch(
        crate::game::core::Message::GuardedLoad { filename, reply },
        harness.now,
    );
    controls.take_guarded_result().unwrap().result
}

fn mark_unsaved_through_controls(harness: &mut Harness) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let controls = crate::game::controls::SimControls::new(sender);
    controls.mark_unsaved();
    let message = receiver.recv().unwrap();
    assert_eq!(message, crate::game::core::Message::MarkUnsaved);
    harness.core.dispatch(message, harness.now);
}

#[test]
fn unsaved_changes_are_core_only_and_ticks_and_effective_save_loads_update_them() {
    let mut harness = Harness::new(42);
    assert!(!harness.core.has_unsaved_changes());
    let initial_digest = harness.digest();
    harness.control(Control::Speed(2));
    harness.control(Control::TogglePause);
    assert!(
        !harness.core.has_unsaved_changes(),
        "time controls are not game choices"
    );
    assert_eq!(initial_digest, harness.digest());
    let clean_snapshot = harness.core.snapshot();
    mark_unsaved_through_controls(&mut harness);
    assert_eq!(clean_snapshot, harness.core.snapshot());
    assert_eq!(initial_digest, harness.digest());
    assert!(harness.core.has_unsaved_changes());

    harness.run_until(1, &[1000], after(0, 1));
    assert!(
        harness.core.has_unsaved_changes(),
        "a simulation tick dirties the game"
    );
    assert_ne!(initial_digest, harness.digest());

    harness.control(Control::Quicksave);
    assert!(
        !harness.core.has_unsaved_changes(),
        "a successful quicksave clears it"
    );
    mark_unsaved_through_controls(&mut harness);
    assert!(harness.core.has_unsaved_changes());
    assert!(matches!(
        harness.save_call(SaveCall::New("Named".to_owned())),
        SaveReply::Finished(Ok(()))
    ));
    assert!(
        !harness.core.has_unsaved_changes(),
        "a successful new save clears it"
    );

    mark_unsaved_through_controls(&mut harness);
    assert!(matches!(
        harness.save_call(SaveCall::Over("named.cksave".to_owned())),
        SaveReply::Finished(Ok(()))
    ));
    assert!(
        !harness.core.has_unsaved_changes(),
        "a successful save-over clears it"
    );
    mark_unsaved_through_controls(&mut harness);
    harness.control(Control::Quickload);
    assert!(
        !harness.core.has_unsaved_changes(),
        "a successful quickload clears it"
    );
    assert_eq!(harness.paused(), Some(PauseReason::Load));
    mark_unsaved_through_controls(&mut harness);
    assert_eq!(
        guarded_load(&mut harness, "named.cksave"),
        Err("unsaved".to_owned())
    );
    assert!(
        harness.core.has_unsaved_changes(),
        "a guarded refusal preserves it"
    );
}

#[test]
fn failed_saves_and_loads_keep_the_unsaved_flag_and_guarded_refusal_does_no_io() {
    let mut harness = Harness::new(42);
    harness.run_until(1, &[1000], after(0, 1));
    harness.control(Control::TogglePause);
    let before = (
        harness.digest(),
        harness.core.snapshot().date,
        harness.paused(),
    );
    let errors_before = harness.texts(Level::Error).len();
    assert_eq!(
        guarded_load(&mut harness, "missing.cksave"),
        Err("unsaved".to_owned())
    );
    assert_eq!(
        harness.texts(Level::Error).len(),
        errors_before,
        "unsaved is a normal guarded outcome"
    );
    assert!(harness.core.has_unsaved_changes());
    assert_eq!(
        (
            harness.digest(),
            harness.core.snapshot().date,
            harness.paused()
        ),
        before,
        "guarding leaves the game state, date and pause unchanged"
    );

    let failed = harness.save_call(SaveCall::New("   ".to_owned()));
    assert!(matches!(failed, SaveReply::Finished(Err(_))));
    assert!(
        harness.core.has_unsaved_changes(),
        "a failed save leaves it set"
    );
    assert_eq!(
        guarded_load(&mut harness, "missing.cksave"),
        Err("unsaved".to_owned()),
        "a failed save does not clear the flag before a guarded request"
    );
    let (reply, result) = std::sync::mpsc::channel();
    harness.core.dispatch(
        crate::game::core::Message::Save(SaveCall::Load("missing.cksave".to_owned()), reply),
        harness.now,
    );
    assert!(matches!(
        result.recv().unwrap(),
        SaveReply::Finished(Err(_))
    ));
    assert!(
        harness.core.has_unsaved_changes(),
        "a failed plain load leaves it set"
    );
    assert_eq!(
        (
            harness.digest(),
            harness.core.snapshot().date,
            harness.paused()
        ),
        before,
        "a refused plain load leaves the game state, date and pause unchanged"
    );
}

#[test]
fn guarded_load_follows_dirty_marks_in_the_same_message_order() {
    let mut harness = Harness::new(42);
    harness.control(Control::Quicksave);
    let (sender, receiver) = std::sync::mpsc::channel();
    let controls = crate::game::controls::SimControls::new(sender);

    controls.mark_unsaved();
    controls.guarded_load("quicksave.cksave".to_owned());
    let mark = receiver.recv().unwrap();
    let guarded = receiver.recv().unwrap();
    harness.core.dispatch(mark, harness.now);
    harness.core.dispatch(guarded, harness.now);
    assert_eq!(
        controls.take_guarded_result().unwrap().result,
        Err("unsaved".to_owned())
    );
    assert!(harness.core.has_unsaved_changes());

    harness.control(Control::Quickload);
    assert!(!harness.core.has_unsaved_changes());
    controls.guarded_load("quicksave.cksave".to_owned());
    controls.mark_unsaved();
    let guarded = receiver.recv().unwrap();
    let mark = receiver.recv().unwrap();
    harness.core.dispatch(guarded, harness.now);
    harness.core.dispatch(mark, harness.now);
    assert_eq!(controls.take_guarded_result().unwrap().result, Ok(()));
    assert!(
        harness.core.has_unsaved_changes(),
        "the later UI choice remains dirty after the load"
    );
    assert_eq!(harness.paused(), Some(PauseReason::Load));
}

#[test]
fn a_running_tick_then_pause_and_an_erroring_tick_both_guard_the_load() {
    let mut running = Harness::new(42);
    running.run_until(1, &[1000], after(0, 1));
    running.control(Control::TogglePause);
    assert_eq!(
        guarded_load(&mut running, "missing.cksave"),
        Err("unsaved".to_owned())
    );
    assert_eq!(running.paused(), Some(PauseReason::Player));

    let mut erroring = Harness::with_faults(
        42,
        super::Faults {
            error_at: Some(start().hours() + 1),
            end_at: None,
        },
    );
    erroring.run_at(5);
    erroring.wake(SECOND);
    assert_eq!(erroring.paused(), Some(PauseReason::Error));
    assert_eq!(
        guarded_load(&mut erroring, "missing.cksave"),
        Err("unsaved".to_owned())
    );
}

#[test]
fn a_successful_save_then_a_ui_choice_then_a_guarded_load_refuses_in_that_order() {
    let mut harness = Harness::new(42);
    let (sender, receiver) = std::sync::mpsc::channel();
    let controls = crate::game::controls::SimControls::new(sender);
    let (save_reply, save_result) = std::sync::mpsc::channel();
    controls.send_save_for_test(SaveCall::New("Ordered".to_owned()), save_reply);
    controls.mark_unsaved();
    controls.guarded_load("ordered.cksave".to_owned());
    let save = receiver.recv().unwrap();
    let mark = receiver.recv().unwrap();
    let guarded = receiver.recv().unwrap();
    harness.core.dispatch(save, harness.now);
    assert_eq!(save_result.recv().unwrap(), SaveReply::Finished(Ok(())));
    harness.core.dispatch(mark, harness.now);
    harness.core.dispatch(guarded, harness.now);
    assert_eq!(
        controls.take_guarded_result().unwrap().result,
        Err("unsaved".to_owned())
    );
    assert!(harness.core.has_unsaved_changes());
}

#[test]
fn a_failed_tick_still_marks_the_game_unsaved() {
    let mut harness = Harness::with_faults(
        42,
        super::Faults {
            error_at: Some(start().hours() + 1),
            end_at: None,
        },
    );
    harness.run_until(1, &[1000], after(0, 1));
    assert!(harness.core.has_unsaved_changes());
}

#[test]
fn the_unsaved_flag_does_not_change_the_save_body_or_game_digest() {
    let mut harness = Harness::new(42);
    harness.run_until(1, &[1000], after(0, 1));
    let digest = harness.digest();
    harness.control(Control::Quicksave);
    let first = fs::read(harness.saves.join(QUICKSAVE)).unwrap();
    mark_unsaved_through_controls(&mut harness);
    assert_eq!(harness.digest(), digest);
    harness.control(Control::Quicksave);
    let second = fs::read(harness.saves.join(QUICKSAVE)).unwrap();
    let first_header = u32::from_le_bytes(first[8..12].try_into().unwrap()) as usize;
    let second_header = u32::from_le_bytes(second[8..12].try_into().unwrap()) as usize;
    let first_sections_end = first.len() - 12;
    let second_sections_end = second.len() - 12;
    assert_eq!(
        &first[12 + first_header..first_sections_end],
        &second[12 + second_header..second_sections_end],
        "the flag is outside serialized sections"
    );
}

#[test]
fn a_load_stays_clean_until_the_first_tick_after_resume_and_a_save_then_tick_is_dirty() {
    let mut harness = Harness::new(42);
    harness.run_until(1, &[1000], after(0, 1));
    harness.control(Control::Quicksave);
    harness.control(Control::Quickload);
    assert!(!harness.core.has_unsaved_changes());
    assert_eq!(harness.paused(), Some(PauseReason::Load));
    let loaded_time = harness.core.time();
    harness.wake(SECOND);
    assert_eq!(
        harness.core.time(),
        loaded_time,
        "a paused load does not tick"
    );
    assert!(!harness.core.has_unsaved_changes());
    harness.control(Control::TogglePause);
    harness.wake(SECOND);
    assert!(
        harness.core.has_unsaved_changes(),
        "the resumed tick marks the loaded game"
    );

    harness.control(Control::Quicksave);
    assert!(!harness.core.has_unsaved_changes());
    harness.wake(SECOND);
    assert!(
        harness.core.has_unsaved_changes(),
        "a tick after a successful save marks it again"
    );
}

#[test]
fn marking_unsaved_changes_at_multiple_points_keeps_the_same_seeded_state_digest() {
    let mut plain = Harness::new(42);
    let mut marked = Harness::new(42);
    for hour in [1, 3, 9, 17] {
        let target = after(0, hour);
        plain.run_until(1, &[1000], target);
        marked.run_until(1, &[1000], target);
        mark_unsaved_through_controls(&mut marked);
        assert!(marked.core.has_unsaved_changes());
    }
    assert_eq!(marked.digest(), plain.digest());
    assert_eq!(marked.core.state(), plain.core.state());
}

#[test]
fn a_refused_save_over_keeps_unsaved_changes_and_a_game_running() {
    let mut harness = Harness::new(42);
    assert!(matches!(
        harness.save_call(SaveCall::New("Existing".to_owned())),
        SaveReply::Finished(Ok(()))
    ));
    harness.run_until(1, &[1000], after(0, 1));
    assert!(harness.core.has_unsaved_changes());
    let before = harness.core.time();
    let _readonly = ReadOnlyDirectory::new(&harness.saves);
    assert!(matches!(
        harness.save_call(SaveCall::Over("existing.cksave".to_owned())),
        SaveReply::Finished(Err(_))
    ));
    assert!(harness.core.has_unsaved_changes());
    assert_eq!(harness.paused(), None);
    assert_eq!(harness.core.time(), before);
    assert_eq!(
        guarded_load(&mut harness, "existing.cksave"),
        Err("unsaved".to_owned()),
        "a real write failure leaves the next guarded load refused"
    );
}

#[test]
fn successful_plain_load_clears_dirty_state_and_successful_guarded_load_starts_clean() {
    let mut harness = Harness::new(42);
    assert!(matches!(
        harness.save_call(SaveCall::New("Named".to_owned())),
        SaveReply::Finished(Ok(()))
    ));
    mark_unsaved_through_controls(&mut harness);
    assert!(matches!(
        harness.save_call(SaveCall::Load("named.cksave".to_owned())),
        SaveReply::Finished(Ok(()))
    ));
    assert!(!harness.core.has_unsaved_changes());
    assert_eq!(harness.paused(), Some(PauseReason::Load));

    mark_unsaved_through_controls(&mut harness);
    assert_eq!(
        guarded_load(&mut harness, "named.cksave"),
        Err("unsaved".to_owned())
    );
    assert!(matches!(
        harness.save_call(SaveCall::Load("named.cksave".to_owned())),
        SaveReply::Finished(Ok(()))
    ));
    assert!(!harness.core.has_unsaved_changes());
    assert_eq!(harness.paused(), Some(PauseReason::Load));
}

const SECOND: Duration = Duration::from_secs(1);

struct ReadOnlyDirectory {
    path: PathBuf,
    original: fs::Permissions,
}

impl ReadOnlyDirectory {
    fn new(path: &Path) -> Self {
        let original = fs::metadata(path).unwrap().permissions();
        fs::set_permissions(path, fs::Permissions::from_mode(0o555)).unwrap();
        Self {
            path: path.to_owned(),
            original,
        }
    }
}

impl Drop for ReadOnlyDirectory {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.path, self.original.clone());
    }
}

fn day_37() -> ck_sim::GameTime {
    after(37, 0)
}

#[test]
fn quicksave_then_quickload_continues_to_the_uninterrupted_digest() {
    let mut harness = Harness::new(42);
    harness.run_until(1, &[1000], day_37());
    assert_eq!(harness.core.time(), day_37());
    let saved_digest = harness.digest();

    harness.control(Control::Quicksave);
    let path = harness.saves.join(QUICKSAVE);
    assert!(path.is_file());
    let quicksave = ck_sim::Save::<super::World>::from_bytes(
        &fs::read(&path).unwrap(),
        &identity("aa").expected(),
    )
    .expect("F5 writes the current format");
    assert_eq!(quicksave.meta.name, "quicksave");
    assert_eq!(quicksave.meta.written.len(), 20);
    assert_eq!(quicksave.digest().unwrap(), saved_digest);
    assert_eq!(
        harness.texts(Level::Info).last().unwrap(),
        &format!("saved {} at 0768-02-07 00:00", path.display())
    );
    // Saving carries on as it was: running, at the same speed, with nothing lost.
    assert_eq!(harness.paused(), None);
    assert_eq!(harness.core.snapshot().speed.level(), 1);
    assert_eq!(harness.digest(), saved_digest);

    // Saved at day 37, continued to day 400: the digest is the uninterrupted run's.
    harness.run_until(5, &[1000], after(400, 0));
    let stopped = harness.core.time();
    assert_eq!(harness.digest(), Harness::reference_digest(42, stopped));

    // Loaded, it starts paused at speed 1 with the reason `load`, and continues to the same digest.
    harness.control(Control::Quickload);
    assert_eq!(harness.core.time(), day_37());
    assert_eq!(harness.paused(), Some(PauseReason::Load));
    assert_eq!(harness.core.snapshot().speed.level(), 1);
    assert_eq!(harness.digest(), saved_digest);
    assert_eq!(
        harness.texts(Level::Info).last().unwrap(),
        &format!("loaded {} at 0768-02-07 00:00", path.display())
    );
    harness.run_until(5, &[1000], stopped);
    // It stops at the first wake that reached the target, which may be a few ticks later than before.
    let again = harness.core.time();
    assert_eq!(harness.digest(), Harness::reference_digest(42, again));
}

#[test]
fn same_build_saves_and_digests_ignore_derived_daily_market_orders() {
    let economy = ck_economy::EconomyState {
        places: vec![ck_economy::PlaceState {
            id: "fixture".to_owned(),
            coastal: false,
            pops: Vec::new(),
            buildings: Vec::new(),
            prices_milli: vec![96_500],
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: vec![0],
            last_market_orders: Some(ck_economy::DailyMarketOrders {
                buy_units: vec![1240],
                sell_units: vec![1410],
            }),
            counters: ck_economy::CounterBuckets::default(),
        }],
    };
    let mut cleared = economy.clone();
    cleared.places[0].last_market_orders = None;
    let make = |economy| {
        Simulation::<crate::game::GameState, crate::game::GameCommand>::new(
            start(),
            42,
            crate::game::GameState { economy },
            Scheduler::new(),
            crate::game::apply,
        )
    };
    let with_orders = make(economy);
    let without_orders = make(cleared);
    assert_eq!(
        with_orders.digest().unwrap(),
        without_orders.digest().unwrap()
    );
    let scenario_hash = "ab".repeat(32);
    let meta = identity(&scenario_hash).meta(42);
    let bytes = with_orders.save(meta.clone()).to_bytes().unwrap();
    assert_eq!(bytes, without_orders.save(meta.clone()).to_bytes().unwrap());
    let loaded = ck_sim::Save::<crate::game::GameState>::from_bytes(
        &bytes,
        &identity(&scenario_hash).expected(),
    )
    .unwrap();
    assert!(loaded.state.economy.places[0].last_market_orders.is_none());
    assert_eq!(
        loaded.to_bytes().unwrap(),
        bytes,
        "loading and saving again keeps the same-build bytes"
    );
}

#[test]
fn a_quicksave_while_paused_stays_paused_and_a_second_one_replaces_the_first() {
    let mut harness = Harness::new(7);
    harness.run_until(1, &[1000], after(0, 5));
    harness.control(Control::TogglePause);
    harness.control(Control::Quicksave);
    assert_eq!(harness.paused(), Some(PauseReason::Player));
    let first = fs::read(harness.saves.join(QUICKSAVE)).unwrap();
    harness.control(Control::TogglePause);
    harness.wake(SECOND);
    harness.control(Control::Quicksave);
    let second = fs::read(harness.saves.join(QUICKSAVE)).unwrap();
    assert_ne!(first, second);
    // No temporary file is left behind.
    let names: Vec<_> = fs::read_dir(&harness.saves)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, [std::ffi::OsString::from(QUICKSAVE)]);
}

#[test]
fn a_quicksave_that_cannot_be_written_is_one_error_record_and_the_game_continues() {
    let mut harness = Harness::new(7);
    harness.run_until(1, &[1000], after(0, 3));
    // The saves directory is a file, so it cannot be created.
    fs::write(&harness.saves, b"not a directory").unwrap();
    harness.control(Control::Quicksave);
    let errors = harness.texts(Level::Error);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("quicksave failed: "), "{errors:?}");
    assert_eq!(harness.paused(), None);
    let before = harness.core.time();
    harness.wake(SECOND);
    assert_eq!(harness.core.time(), before.next());
}

/// Quicksaves a game at day 37 and returns it, paused at the save.
fn saved_game() -> Harness {
    let mut harness = Harness::new(42);
    harness.run_until(1, &[1000], day_37());
    harness.control(Control::Quicksave);
    harness.control(Control::TogglePause);
    harness
}

fn assert_refused_and_unchanged(harness: &mut Harness, expected_text: &str) {
    let (time, digest, snapshot) = (
        harness.core.time(),
        harness.digest(),
        harness.core.snapshot(),
    );
    let errors_before = harness.texts(Level::Error).len();
    let wakes = harness.wake_count();
    harness.control(Control::Quickload);
    let errors = harness.texts(Level::Error);
    assert_eq!(errors.len(), errors_before + 1, "{errors:?}");
    let refusal = errors.last().unwrap();
    assert!(refusal.starts_with("quickload refused: "), "{refusal}");
    assert!(refusal.contains(expected_text), "{refusal}");
    // The current game continues unchanged.
    assert_eq!(harness.core.time(), time);
    assert_eq!(harness.digest(), digest);
    assert_eq!(harness.core.snapshot(), snapshot);
    assert_eq!(harness.wake_count(), wakes);
}

#[test]
fn a_quickload_of_a_changed_file_is_refused_by_name_and_keeps_the_game() {
    for (index_from_end, expected) in [(None, "this is not a save file"), (Some(1), "end checksum")]
    {
        let mut harness = saved_game();
        let path = harness.saves.join(QUICKSAVE);
        let mut bytes = fs::read(&path).unwrap();
        let index = index_from_end.map_or(0, |from_end| bytes.len() - from_end);
        bytes[index] ^= 1;
        fs::write(&path, bytes).unwrap();
        assert_refused_and_unchanged(&mut harness, expected);
    }
}

#[test]
fn a_quickload_of_another_scenarios_save_is_refused_naming_the_hash() {
    let harness = saved_game();
    let mut other = Harness::with_identity_in(42, identity("zz"), &harness);
    other.run_until(1, &[1000], after(0, 4));
    assert_refused_and_unchanged(&mut other, "scenario hash");

    let mut other_build = Harness::with_identity_in(
        42,
        crate::game::saves::Identity {
            build: "another-build".to_owned(),
            ..identity("aa")
        },
        &harness,
    );
    assert_refused_and_unchanged(&mut other_build, "build `test-build`");
}

#[test]
fn a_quickload_with_no_quicksave_is_refused_and_keeps_the_game() {
    let mut harness = Harness::new(42);
    harness.run_until(1, &[1000], after(0, 4));
    assert_refused_and_unchanged(&mut harness, "cannot read");
    assert_eq!(harness.core.time(), start().next().next().next().next());
}

#[test]
fn a_quickload_that_leaves_the_hour_unchanged_still_wakes_the_main_thread() {
    let mut harness = Harness::new(42);
    harness.run_until(1, &[1000], after(0, 5));
    harness.control(Control::Quicksave);
    // Still running, in the same hour the save was taken in: the load changes the pause (and nothing else the date shows).
    let wakes = harness.wake_count();
    let date = harness.core.snapshot().date;
    harness.control(Control::Quickload);
    assert_eq!(harness.core.snapshot().date, date);
    assert_eq!(harness.paused(), Some(PauseReason::Load));
    assert_eq!(harness.wake_count(), wakes + 1);
}

#[test]
fn named_save_calls_list_write_overwrite_load_and_delete_complete_results() {
    let mut harness = Harness::new(42);
    harness.run_until(1, &[1000], day_37());
    let digest = harness.digest();

    assert_eq!(
        harness.save_call(SaveCall::List),
        SaveReply::Listed(Ok(Vec::new()))
    );
    assert_eq!(
        harness.save_call(SaveCall::New("  My & Save  ".to_owned())),
        SaveReply::Finished(Ok(()))
    );
    assert_eq!(harness.digest(), digest, "a save does not change the game");
    let filename = "my-save.cksave";
    let path = harness.saves.join(filename);
    let bytes = fs::read(&path).expect("the named save was published");
    let saved = ck_sim::Save::<super::World>::from_bytes(&bytes, &identity("aa").expected())
        .expect("the written save loads");
    assert_eq!(saved.meta.name, "My & Save");
    assert_eq!(saved.meta.written.len(), 20);
    assert!(saved.meta.written.ends_with('Z'));
    assert_eq!(saved.digest().unwrap(), digest);

    let SaveReply::Listed(Ok(entries)) = harness.save_call(SaveCall::List) else {
        panic!("listing returns completed entries");
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].filename, filename);
    assert_eq!(entries[0].name, "My & Save");
    assert_eq!(
        entries[0].written.as_deref(),
        Some(saved.meta.written.as_str())
    );
    assert_eq!(entries[0].date.as_deref(), Some("7 Feb 768"));
    assert_eq!(entries[0].this_build, Some(true));
    assert_eq!(entries[0].fit.as_str(), "fits");

    harness.run_until(1, &[1000], after(80, 0));
    assert_ne!(harness.digest(), digest);
    assert_eq!(
        harness.save_call(SaveCall::Load(filename.to_owned())),
        SaveReply::Finished(Ok(()))
    );
    assert_eq!(harness.digest(), digest);
    assert_eq!(harness.paused(), Some(PauseReason::Load));

    assert_eq!(
        harness.save_call(SaveCall::Over(filename.to_owned())),
        SaveReply::Finished(Ok(()))
    );
    let after_overwrite = fs::read(&path).unwrap();
    let overwritten =
        ck_sim::Save::<super::World>::from_bytes(&after_overwrite, &identity("aa").expected())
            .unwrap();
    assert_eq!(
        overwritten.meta.name, "My & Save",
        "save-over keeps the name"
    );
    assert_eq!(overwritten.digest().unwrap(), digest);

    assert_eq!(
        harness.save_call(SaveCall::Delete(filename.to_owned())),
        SaveReply::Finished(Ok(()))
    );
    assert!(!path.exists());
    assert_eq!(
        harness.save_call(SaveCall::List),
        SaveReply::Listed(Ok(Vec::new()))
    );
}

#[test]
fn failed_named_save_over_new_save_and_delete_leave_the_directory_unchanged() {
    let mut harness = Harness::new(42);
    harness.run_until(1, &[1000], day_37());
    assert_eq!(
        harness.save_call(SaveCall::New("Existing".to_owned())),
        SaveReply::Finished(Ok(()))
    );
    let existing = harness.saves.join("existing.cksave");
    let original = fs::read(&existing).unwrap();
    let readonly = ReadOnlyDirectory::new(&harness.saves);

    for call in [
        SaveCall::Over("existing.cksave".to_owned()),
        SaveCall::New("Cannot publish".to_owned()),
        SaveCall::Delete("existing.cksave".to_owned()),
    ] {
        let SaveReply::Finished(Err(reason)) = harness.save_call(call) else {
            panic!("the operation fails while the saves directory is read-only");
        };
        assert!(!reason.is_empty());
    }

    assert_eq!(fs::read(&existing).unwrap(), original);
    let mut names: Vec<_> = fs::read_dir(&harness.saves)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, [std::ffi::OsString::from("existing.cksave")]);
    drop(readonly);
    assert_ne!(
        fs::metadata(&harness.saves).unwrap().permissions().mode() & 0o222,
        0
    );
}

#[test]
fn invalid_new_save_names_are_refused_before_a_file_is_written() {
    let mut harness = Harness::new(42);
    let long = "x".repeat(49);
    for name in [" ", "line\nbreak", long.as_str()] {
        assert!(matches!(
            harness.save_call(SaveCall::New(name.to_owned())),
            SaveReply::Finished(Err(_))
        ));
    }
    assert!(!harness.saves.exists());
}

#[test]
fn a_fitting_header_with_a_damaged_body_is_listed_but_its_load_keeps_the_game_unchanged() {
    let mut harness = Harness::new(42);
    harness.run_until(1, &[1000], day_37());
    harness.save_call(SaveCall::New("Body damage".to_owned()));
    let path = harness.saves.join("body-damage.cksave");
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&path, bytes).unwrap();

    let SaveReply::Listed(Ok(entries)) = harness.save_call(SaveCall::List) else {
        panic!("the header-only listing succeeds");
    };
    assert_eq!(entries[0].fit.as_str(), "fits");
    let before = (harness.digest(), harness.core.snapshot(), harness.paused());
    let SaveReply::Finished(Err(reason)) =
        harness.save_call(SaveCall::Load("body-damage.cksave".to_owned()))
    else {
        panic!("the full loader rejects the damaged body");
    };
    assert!(reason.contains("checksum"), "{reason}");
    assert_eq!(
        (harness.digest(), harness.core.snapshot(), harness.paused()),
        before
    );
}

#[test]
fn unsupported_saves_are_delete_only_and_file_arguments_cannot_escape_the_saves_directory() {
    let mut harness = Harness::new(42);
    harness.save_call(SaveCall::New("Old format".to_owned()));
    let path = harness.saves.join("old-format.cksave");
    let mut bytes = fs::read(&path).unwrap();
    let header_length =
        usize::try_from(u32::from_le_bytes(bytes[8..12].try_into().unwrap())).unwrap();
    let header = std::str::from_utf8(&bytes[12..12 + header_length]).unwrap();
    let old_header = header
        .lines()
        .filter(|line| {
            !line.starts_with("name = ")
                && !line.starts_with("written = ")
                && !line.starts_with("date = ")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .replace("format = 2", "format = 1")
        .replace("build = \"test-build\"", "build = \"previous-build\"");
    let mut old = bytes[..12].to_vec();
    old[8..12].copy_from_slice(&u32::try_from(old_header.len()).unwrap().to_le_bytes());
    old.extend_from_slice(old_header.as_bytes());
    old.extend_from_slice(&bytes[12 + header_length..]);
    bytes = old;
    fs::write(&path, bytes).unwrap();

    let SaveReply::Listed(Ok(entries)) = harness.save_call(SaveCall::List) else {
        panic!("old header remains listable");
    };
    assert_eq!(entries[0].fit.as_str(), "version");
    assert_eq!(entries[0].this_build, Some(false));
    assert_eq!(entries[0].name, "old-format");
    assert_eq!(entries[0].date, None);
    assert_eq!(entries[0].written, None);
    assert_eq!(
        harness.save_call(SaveCall::Over("old-format.cksave".to_owned())),
        SaveReply::Finished(Err("save cannot be overwritten: version".to_owned()))
    );
    assert!(matches!(
        harness.save_call(SaveCall::Load("old-format.cksave".to_owned())),
        SaveReply::Finished(Err(reason)) if reason == "save cannot be loaded: version"
    ));
    assert_eq!(
        harness.save_call(SaveCall::Delete("old-format.cksave".to_owned())),
        SaveReply::Finished(Ok(()))
    );

    let outside = harness.saves.parent().unwrap().join("outside.cksave");
    fs::write(&outside, b"outside remains").unwrap();
    for filename in [
        "../outside.cksave",
        "/tmp/outside.cksave",
        "a\\b.cksave",
        "..",
        "",
        "missing-extension",
    ] {
        for call in [SaveCall::Over, SaveCall::Delete, SaveCall::Load] {
            assert!(
                matches!(
                    harness.save_call(call(filename.to_owned())),
                    SaveReply::Finished(Err(_))
                ),
                "{filename}"
            );
        }
    }
    assert_eq!(fs::read(outside).unwrap(), b"outside remains");
}

fn install_catalog_header_fixtures(harness: &mut Harness) {
    let source = fs::read(harness.saves.join("fits.cksave")).unwrap();
    let read_header = |bytes: &[u8]| {
        let length = usize::try_from(u32::from_le_bytes(bytes[8..12].try_into().unwrap())).unwrap();
        std::str::from_utf8(&bytes[12..12 + length])
            .unwrap()
            .to_owned()
    };
    let with_header = |header: &str| {
        let mut bytes = source[..12].to_vec();
        bytes[8..12].copy_from_slice(&u32::try_from(header.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(header.as_bytes());
        let old_length =
            usize::try_from(u32::from_le_bytes(source[8..12].try_into().unwrap())).unwrap();
        bytes.extend_from_slice(&source[12 + old_length..]);
        bytes
    };
    let header = read_header(&source);
    fs::write(
        harness.saves.join("future.cksave"),
        with_header(&format!(
            "{}future_key = true\n",
            header.replace("format = 2", "format = 8")
        )),
    )
    .unwrap();
    fs::write(
        harness.saves.join("other-build.cksave"),
        with_header(&header.replace("test-build", "other-build")),
    )
    .unwrap();
    fs::write(
        harness.saves.join("wrong-content.cksave"),
        with_header(&header.replace("scenario_hash = \"aa\"", "scenario_hash = \"ee55\"")),
    )
    .unwrap();
    fs::write(
        harness.saves.join("bad-toml.cksave"),
        b"CKSAVE\0\x01\x08\0\0\0not=toml",
    )
    .unwrap();
    fs::write(harness.saves.join("bad-magic.cksave"), b"not a save").unwrap();
    for (filename, key) in [
        ("missing-format.cksave", "format = "),
        ("missing-build.cksave", "build = "),
    ] {
        let header_missing_line = header
            .lines()
            .filter(|line| !line.starts_with(key))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(
            harness.saves.join(filename),
            with_header(&header_missing_line),
        )
        .unwrap();
    }
    let mut oversized = b"CKSAVE\0\x01".to_vec();
    oversized.extend_from_slice(&65_537_u32.to_le_bytes());
    fs::write(harness.saves.join("too-large.cksave"), oversized).unwrap();
    fs::write(
        harness.saves.join("cut-header.cksave"),
        b"CKSAVE\0\x01\x10\0\0\0short",
    )
    .unwrap();
}

#[test]
fn catalog_keeps_scanning_past_unreadable_future_and_content_mismatch_headers() {
    let mut harness = Harness::new(42);
    harness.save_call(SaveCall::New("Fits".to_owned()));
    install_catalog_header_fixtures(&mut harness);
    let SaveReply::Listed(Ok(entries)) = harness.save_call(SaveCall::List) else {
        panic!("the catalog returns entries even when individual headers fail");
    };
    let fit = |filename: &str| {
        entries
            .iter()
            .find(|entry| entry.filename == filename)
            .unwrap_or_else(|| panic!("missing catalog row {filename}"))
            .fit
            .as_str()
    };
    assert_eq!(fit("fits.cksave"), "fits");
    assert_eq!(fit("future.cksave"), "version");
    assert_eq!(fit("other-build.cksave"), "version");
    assert_eq!(fit("wrong-content.cksave"), "content");
    for (filename, this_build) in [
        ("bad-magic.cksave", None),
        ("bad-toml.cksave", None),
        ("missing-format.cksave", Some(true)),
        ("missing-build.cksave", None),
        ("too-large.cksave", None),
        ("cut-header.cksave", None),
    ] {
        assert_eq!(fit(filename), "unreadable", "{filename}");
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.filename == filename)
                .unwrap()
                .this_build,
            this_build,
            "{filename}"
        );
    }
    for (filename, status) in [
        ("future.cksave", "version"),
        ("other-build.cksave", "version"),
        ("wrong-content.cksave", "content"),
        ("bad-magic.cksave", "unreadable"),
        ("bad-toml.cksave", "unreadable"),
        ("missing-format.cksave", "unreadable"),
        ("missing-build.cksave", "unreadable"),
        ("too-large.cksave", "unreadable"),
        ("cut-header.cksave", "unreadable"),
    ] {
        assert!(
            matches!(
                harness.save_call(SaveCall::Over(filename.to_owned())),
                SaveReply::Finished(Err(reason)) if reason.contains(status)
            ),
            "save-over {filename}"
        );
        assert!(
            matches!(
                harness.save_call(SaveCall::Load(filename.to_owned())),
                SaveReply::Finished(Err(reason)) if reason.contains(status)
            ),
            "load {filename}"
        );
        assert_eq!(
            harness.save_call(SaveCall::Delete(filename.to_owned())),
            SaveReply::Finished(Ok(())),
            "delete {filename}"
        );
    }
}

#[test]
fn directories_and_symlinks_are_not_saves_and_calls_never_follow_them() {
    let mut harness = Harness::new(42);
    harness.save_call(SaveCall::New("Good".to_owned()));
    let directory_entry = harness.saves.join("directory.cksave");
    fs::create_dir(&directory_entry).unwrap();
    let outside = harness.saves.parent().unwrap().join("outside.cksave");
    fs::write(&outside, b"outside remains unchanged").unwrap();
    let link = harness.saves.join("link.cksave");
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    let SaveReply::Listed(Ok(entries)) = harness.save_call(SaveCall::List) else {
        panic!("listing succeeds");
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].filename, "good.cksave");
    for filename in ["directory.cksave", "link.cksave"] {
        for call in [SaveCall::Over, SaveCall::Delete, SaveCall::Load] {
            assert!(
                matches!(
                    harness.save_call(call(filename.to_owned())),
                    SaveReply::Finished(Err(_))
                ),
                "{filename}"
            );
        }
    }
    assert_eq!(fs::read(outside).unwrap(), b"outside remains unchanged");
}
