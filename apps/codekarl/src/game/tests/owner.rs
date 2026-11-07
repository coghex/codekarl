use std::{sync::Arc, time::Duration};

use ck_runtime::{Owner, OwnerError, Progress};

use super::{Faults, Waiter, spawn_test_owner, start};
use crate::game::{controls::Control, core::Message, slot::PauseReason};

const BOUND: Duration = Duration::from_secs(10);

#[test]
fn the_owner_is_ready_needs_no_start_and_stops_cleanly_without_running_a_tick() {
    let waiter = Arc::new(Waiter::default());
    let (mut owner, _sender, slot) = spawn_test_owner(Faults::default(), &waiter, Vec::new());
    assert_eq!(owner.name(), "sim");
    assert_eq!(owner.wait_ready(BOUND), Ok(()));
    assert_eq!(owner.poll_ready(Duration::ZERO), Ok(Progress::Done));
    assert_eq!(owner.request_start(), Ok(()));
    assert_eq!(owner.await_started(BOUND), Ok(()));
    assert_eq!(owner.poll_started(Duration::ZERO), Ok(Progress::Done));
    assert_eq!(owner.request_stop(), Ok(()));
    assert_eq!(owner.await_stopped(BOUND), Ok(()));
    assert!(owner.wait_finished(Duration::ZERO));
    // Paused from the start: it published its first snapshot and ran nothing.
    let snapshot = slot.latest().expect("the first snapshot");
    assert_eq!(snapshot.tick, start());
    assert_eq!(snapshot.paused, Some(PauseReason::Load));
    assert_eq!(slot.failure(), None);
}

#[test]
fn cancel_ends_the_thread_without_running_a_tick() {
    let waiter = Arc::new(Waiter::default());
    // Everything is queued before the thread starts: it speeds up, resumes and is then cancelled before any time passes.
    let (mut owner, _sender, slot) = spawn_test_owner(
        Faults::default(),
        &waiter,
        vec![
            Message::Control(Control::Speed(5)),
            Message::Control(Control::TogglePause),
            Message::Cancel,
        ],
    );
    assert!(owner.wait_finished(BOUND));
    assert_eq!(slot.latest().expect("a snapshot").tick, start());
    assert_eq!(owner.await_stopped(Duration::ZERO), Ok(()));
}

#[test]
fn a_running_owner_ticks_in_real_time_and_a_stop_ends_it_after_a_whole_tick() {
    let waiter = Arc::new(Waiter::default());
    let (mut owner, _sender, slot) = spawn_test_owner(
        Faults::default(),
        &waiter,
        vec![
            Message::Control(Control::Speed(5)),
            Message::Control(Control::TogglePause),
        ],
    );
    assert_eq!(owner.wait_ready(BOUND), Ok(()));
    waiter.wait_for(|| {
        slot.latest()
            .is_some_and(|snapshot| snapshot.tick > start().next().next())
    });
    assert_eq!(owner.request_stop(), Ok(()));
    assert_eq!(owner.await_stopped(BOUND), Ok(()));
    assert!(owner.wait_finished(Duration::ZERO));
    let last = slot.latest().expect("a snapshot");
    assert!(last.tick > start());
    assert_eq!(last.paused, None, "a stop is not a pause");
}

#[test]
fn a_terminal_failure_ends_the_owner_failed_and_is_never_a_pause() {
    let waiter = Arc::new(Waiter::default());
    let faults = Faults {
        error_at: None,
        end_at: Some(start().hours() + 3),
    };
    let (mut owner, _sender, slot) = spawn_test_owner(
        faults,
        &waiter,
        vec![
            Message::Control(Control::Speed(5)),
            Message::Control(Control::TogglePause),
        ],
    );
    waiter.wait_for(|| slot.failure().is_some());
    assert_eq!(
        owner.await_stopped(BOUND),
        Err(OwnerError::Failed {
            message: "the gameplay VM ended".to_owned()
        })
    );
    assert_eq!(slot.failure().as_deref(), Some("the gameplay VM ended"));
    // The thread ended after the tick that raised it, and the snapshot never said "paused".
    assert!(owner.wait_finished(Duration::ZERO));
    assert_ne!(
        slot.latest().expect("a snapshot").paused,
        Some(PauseReason::Error)
    );
    // A second failure would change nothing: it is recorded once.
    slot.fail("another".to_owned());
    assert_eq!(slot.failure().as_deref(), Some("the gameplay VM ended"));
}

#[test]
fn the_runtime_counts_a_terminal_failure_as_a_failed_stop() {
    let waiter = Arc::new(Waiter::default());
    let faults = Faults {
        error_at: None,
        end_at: Some(start().hours() + 3),
    };
    let (owner, _sender, slot) = spawn_test_owner(
        faults,
        &waiter,
        vec![
            Message::Control(Control::Speed(5)),
            Message::Control(Control::TogglePause),
        ],
    );
    // The run phase of this runtime ends at once, so the simulation must have failed before the stop phase begins.
    waiter.wait_for(|| slot.failure().is_some());
    let spawn = ck_runtime::OwnerSpawn::new("sim", move || Ok(Box::new(owner) as Box<dyn Owner>));
    let report = ck_runtime::Runtime::new(Vec::new())
        .with_owners(vec![spawn])
        .run_report();
    assert!(
        matches!(report.outcome, ck_runtime::Outcome::StopFailed { .. }),
        "{:?}",
        report.outcome
    );
    assert!(report.missed.is_empty() && report.unfinished.is_empty());
    // A headless run ends with status 1 on it, exactly as a failed stop of any owner.
    assert_eq!(crate::vm::status_of(&report, false, false).code, 1);
}
