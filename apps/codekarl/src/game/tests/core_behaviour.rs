use std::{collections::VecDeque, time::Duration};

use ck_log::Level;
use ck_sim::GameTime;

use super::{Faults, Harness, after, date, start};
use crate::game::{
    controls::Control,
    core::{Flow, Message},
    slot::PauseReason,
};

const SECOND: Duration = Duration::from_secs(1);

/// A scripted control channel: each call is one `poll()` result, and an empty script has nothing to say.
fn script(results: Vec<Option<Message>>) -> impl FnMut() -> Option<Message> {
    let mut results: VecDeque<_> = results.into();
    move || results.pop_front().flatten()
}

#[test]
fn a_new_game_starts_paused_at_speed_one_with_the_reason_load_at_the_start_tick() {
    let mut harness = Harness::new(42);
    let snapshot = harness.core.snapshot();
    assert_eq!(snapshot.tick, start());
    assert_eq!(snapshot.date, date(768, 1, 1, 0));
    assert_eq!(snapshot.speed.level(), 1);
    assert_eq!(snapshot.paused, Some(PauseReason::Load));
    // Paused, no amount of real time runs a tick.
    harness.wake(Duration::from_secs(3600));
    assert_eq!(harness.core.time(), start());
    assert!(harness.core.wait_hint().is_none());
}

#[test]
fn a_stop_during_a_running_wake_ends_after_a_whole_tick() {
    let mut harness = Harness::new(42);
    harness.run_at(5);
    harness.now += SECOND;
    // The wake's first drain, the drain before tick 1, and then a stop before tick 2.
    let mut poll = script(vec![None, None, Some(Message::Stop)]);
    assert_eq!(harness.core.run_wake(harness.now, &mut poll), Flow::End);
    assert_eq!(harness.core.time(), after(0, 1));
    // The tick that ran was a whole one: the state is that of exactly one tick.
    assert_eq!(harness.digest(), Harness::reference_digest(42, after(0, 1)));
}

#[test]
fn a_pause_during_a_wake_stops_it_at_the_next_tick_boundary() {
    let mut harness = Harness::new(42);
    harness.run_at(5);
    harness.now += SECOND;
    // 64 ticks are due; the pause arrives before the third.
    let mut poll = script(vec![
        None,
        None,
        None,
        Some(Message::Control(Control::TogglePause)),
    ]);
    assert_eq!(
        harness.core.run_wake(harness.now, &mut poll),
        Flow::Continue
    );
    assert_eq!(harness.core.time(), after(0, 2));
    assert_eq!(harness.paused(), Some(PauseReason::Player));
    // The ticks the wake had left are not made up later.
    harness.wake(Duration::from_secs(60));
    assert_eq!(harness.core.time(), after(0, 2));
}

#[test]
fn every_speed_runs_its_ticks_per_fake_second() {
    for (level, per_second) in [(1, 1), (2, 6), (3, 24), (4, 72), (5, 168)] {
        let mut harness = Harness::new(1);
        harness.run_at(level);
        for _ in 0..100 {
            harness.wake(Duration::from_millis(100));
        }
        assert_eq!(
            harness.core.time().hours() - start().hours(),
            per_second * 10,
            "speed {level}"
        );
    }
}

#[test]
fn faster_and_slower_move_one_speed_and_clamp_at_1_and_5() {
    let mut harness = Harness::new(1);
    let level = |harness: &Harness| harness.core.snapshot().speed.level();
    harness.control(Control::Slower);
    assert_eq!(level(&harness), 1);
    for expected in [2, 3, 4, 5, 5, 5] {
        harness.control(Control::Faster);
        assert_eq!(level(&harness), expected);
    }
    for expected in [4, 3, 2, 1, 1] {
        harness.control(Control::Slower);
        assert_eq!(level(&harness), expected);
    }
    // A paused game stays paused when the speed changes.
    harness.control(Control::Speed(4));
    assert_eq!(level(&harness), 4);
    assert_eq!(harness.paused(), Some(PauseReason::Load));
}

#[test]
fn a_stalled_wake_is_capped_at_64_ticks_and_then_the_normal_rate_returns() {
    let mut harness = Harness::new(1);
    harness.run_at(5);
    harness.wake(Duration::from_secs(30));
    assert_eq!(harness.core.time(), after(0, 64));
    // No burst follows: 100 ms at speed 5 is 16 ticks.
    harness.wake(Duration::from_millis(100));
    assert_eq!(harness.core.time(), after(0, 64 + 16));
}

#[test]
fn the_player_pausing_and_resuming_never_catches_up_the_time_that_passed() {
    let mut harness = Harness::new(1);
    harness.run_at(2);
    harness.wake(SECOND);
    assert_eq!(harness.core.time(), after(0, 6));
    harness.control(Control::TogglePause);
    assert_eq!(harness.paused(), Some(PauseReason::Player));
    harness.wake(Duration::from_secs(600));
    harness.control(Control::TogglePause);
    assert_eq!(harness.paused(), None);
    harness.wake(Duration::from_millis(500));
    assert_eq!(harness.core.time(), after(0, 9));
}

#[test]
fn two_runs_of_400_days_at_seed_42_give_equal_digests() {
    let target = after(400, 0);
    let mut first = Harness::new(42);
    let mut second = Harness::new(42);
    first.run_until(5, &[1000], target);
    second.run_until(5, &[1000], target);
    assert_eq!(first.core.time(), target);
    assert_eq!(first.digest(), second.digest());
    // Negative control: the seed matters.
    let mut other = Harness::new(43);
    other.run_until(5, &[1000], target);
    assert_ne!(first.digest(), other.digest());
}

/// Runs a pattern with pauses and speed changes in it until `target`, and returns the harness.
fn run_with_pauses(target: GameTime) -> Harness {
    let mut harness = Harness::new(42);
    harness.run_at(5);
    let mut index = 0_u64;
    while harness.core.time() < target {
        index += 1;
        assert!(index < 1_000_000, "the run does not reach its target");
        if index.is_multiple_of(5) {
            harness.control(Control::TogglePause);
            for _ in 0..3 {
                harness.wake(SECOND);
            }
            harness.control(Control::Speed(u8::try_from(index / 5 % 5).unwrap() + 1));
            harness.control(Control::TogglePause);
        }
        harness.wake(Duration::from_millis(100 + index % 900));
    }
    harness
}

#[test]
fn real_time_changes_nothing_pauses_speeds_wake_sizes_and_stalls_give_the_same_digest() {
    let target = after(400, 0);
    let mut runs = Vec::new();
    for (level, wakes) in [
        (5, vec![1000]),
        (3, vec![100]),
        (4, vec![7, 13, 2]),
        // Stalled wakes, each capped at 64 ticks, with short ones between.
        (5, vec![30_000, 1]),
    ] {
        let mut harness = Harness::new(42);
        harness.run_until(level, &wakes, target);
        runs.push(harness);
    }
    runs.push(run_with_pauses(target));
    for run in &runs {
        // Each run ends at the first wake that reached the target, so compare at the tick it stopped on.
        let stopped = run.core.time();
        assert!(stopped >= target);
        assert_eq!(run.digest(), Harness::reference_digest(42, stopped));
    }
}

#[test]
fn a_system_error_pauses_with_the_reason_error_at_the_same_tick_in_two_runs() {
    let fault_at = start().hours() + 50;
    let run = || {
        let mut harness = Harness::with_faults(
            42,
            Faults {
                error_at: Some(fault_at),
                end_at: None,
            },
        );
        harness.run_at(5);
        harness.wake(SECOND);
        harness
    };
    let (first, second) = (run(), run());
    for harness in [&first, &second] {
        // The tick completed, then the game paused at its boundary.
        assert_eq!(harness.core.time(), GameTime::from_hours(fault_at + 1));
        assert_eq!(harness.paused(), Some(PauseReason::Error));
        assert_eq!(
            harness.texts(Level::Error),
            ["system `flaky` failed at 0768-01-03 02:00: a test flaw"]
        );
    }
    assert_eq!(first.digest(), second.digest());
}

#[test]
fn after_an_error_pause_the_player_may_resume() {
    let mut harness = Harness::with_faults(
        42,
        Faults {
            error_at: Some(start().hours() + 3),
            end_at: None,
        },
    );
    harness.run_at(5);
    harness.wake(SECOND);
    assert_eq!(harness.paused(), Some(PauseReason::Error));
    harness.control(Control::TogglePause);
    assert_eq!(harness.paused(), None);
    harness.wake(SECOND);
    assert_eq!(harness.core.time(), after(0, 4 + 64));
    assert_eq!(harness.texts(Level::Error).len(), 1);
}

#[test]
fn the_end_of_the_calendar_pauses_with_one_record_and_refuses_a_resume() {
    let last_evening = GameTime::from_date(date(9999, 12, 31, 22)).unwrap();
    let mut harness = Harness::starting_at(1, last_evening);
    harness.run_at(5);
    harness.wake(SECOND);
    assert_eq!(harness.paused(), Some(PauseReason::Error));
    let errors = harness.texts(Level::Error);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("9999-12-31 23:00"), "{errors:?}");
    assert_eq!(harness.core.snapshot().date, date(9999, 12, 31, 23));

    harness.control(Control::TogglePause);
    assert_eq!(harness.paused(), Some(PauseReason::Error));
    let warnings = harness.texts(Level::Warn);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("calendar has ended"), "{warnings:?}");
    harness.wake(SECOND);
    assert_eq!(harness.core.time(), last_evening.next().next());
}

#[test]
fn one_record_follows_the_first_tick_of_each_new_day() {
    let mut harness = Harness::new(1);
    harness.run_at(5);
    harness.wake(SECOND);
    assert_eq!(
        harness.texts(Level::Info),
        ["0768-01-01", "0768-01-02", "0768-01-03"]
    );
}

#[test]
fn a_terminal_failure_is_its_own_signal_never_a_pause() {
    let fail_at = start().hours() + 5;
    let mut harness = Harness::with_faults(
        42,
        Faults {
            error_at: None,
            end_at: Some(fail_at),
        },
    );
    harness.run_at(5);
    assert_eq!(harness.wake(SECOND), Flow::End);
    // The tick that raised it completed, nothing ran after it, and it was not reported as a pause.
    assert_eq!(harness.core.time(), GameTime::from_hours(fail_at + 1));
    assert_eq!(harness.failures(), ["the gameplay VM ended"]);
    assert_eq!(harness.paused(), None);
    assert!(harness.texts(Level::Error).is_empty());
    assert!(harness.core.wait_hint().is_none());
}

#[test]
fn the_snapshot_wakes_the_main_thread_exactly_when_a_displayed_field_changes() {
    let mut harness = Harness::new(1);
    assert_eq!(harness.wake_count(), 1, "the first snapshot");
    let expect = |harness: &Harness, count: usize, what: &str| {
        assert_eq!(harness.wake_count(), count, "{what}");
    };

    // A speed picked while paused changes what is shown; the same speed again does not.
    harness.control(Control::Speed(3));
    expect(&harness, 2, "a speed picked while paused");
    harness.control(Control::Speed(3));
    expect(&harness, 2, "the same speed again");

    // Resuming changes the pause; each new hour while running is a change.
    harness.control(Control::TogglePause);
    expect(&harness, 3, "a resume");
    // Speed 3 is 24 hours per second: one tick per 41.7 ms.
    harness.wake(Duration::from_millis(42));
    expect(&harness, 4, "the first new hour");
    harness.wake(Duration::from_millis(42));
    expect(&harness, 5, "the second new hour");
    harness.wake(Duration::from_millis(20));
    expect(&harness, 5, "a wake with no tick due");

    // A pause changes only the pause.
    harness.control(Control::TogglePause);
    expect(&harness, 6, "a pause");
    let paused_at = harness.wake_count();
    for _ in 0..3 {
        harness.wake(Duration::from_secs(3600));
    }
    expect(
        &harness,
        paused_at,
        "a paused, idle game across an advance of fake time",
    );
}

#[test]
fn an_error_pause_is_published() {
    let mut harness = Harness::with_faults(
        1,
        Faults {
            error_at: Some(start().hours() + 2),
            end_at: None,
        },
    );
    harness.run_at(1);
    assert_eq!(harness.wake_count(), 2, "the first snapshot and the resume");
    for _ in 0..3 {
        harness.wake(SECOND);
    }
    // The ticks at hours 0 and 1 each changed the date; the third changed the date and paused in one publication.
    assert_eq!(harness.wake_count(), 5);
    assert_eq!(harness.paused(), Some(PauseReason::Error));
}

#[test]
fn each_published_snapshot_is_also_a_debug_record() {
    let mut harness = Harness::new(1);
    harness.control(Control::Speed(5));
    harness.control(Control::TogglePause);
    harness.wake(Duration::from_millis(7));
    harness.control(Control::TogglePause);
    assert_eq!(
        harness.texts(Level::Debug),
        [
            "snapshot 0768-01-01 00:00 speed 1 paused (load)",
            "snapshot 0768-01-01 00:00 speed 5 paused (load)",
            "snapshot 0768-01-01 00:00 speed 5 running",
            "snapshot 0768-01-01 01:00 speed 5 running",
            "snapshot 0768-01-01 01:00 speed 5 paused (player)",
        ]
    );
}
