use std::{fs, time::Duration};

use ck_log::Level;

use super::{Harness, after, identity, start};
use crate::game::{controls::Control, saves::QUICKSAVE, slot::PauseReason};

const SECOND: Duration = Duration::from_secs(1);

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
