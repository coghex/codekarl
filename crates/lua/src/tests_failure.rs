//! CK-66: `take_failure` observes a started owner's terminal failure without waiting.
//!
//! Every wait is bounded. The hold before the thread returns is released by the test, and also by the
//! release sender being dropped when an assertion fails, so a failed test cannot leave the thread
//! waiting longer than `BOUND`.

use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use mlua::Lua;

use super::super::{BOUND, Hooks, Point, hooks, reported, script_error};
use super::{Probe, Wakes, hold_points, int, next_result, wait_thread_ended};
use crate::{VmError, VmState};

/// The script of the owners below: callbacks `echo` (returns its argument) and `boom` (a host panic).
const SCRIPT: &str =
    "ext.on_echo(function(a) return a end)\next.on_boom(function() ext.boom() end)\n";

/// The text of the failure given for a report channel that closed with no report.
const NO_REPORT: &str = "the owner thread ended without a report";

fn panicked(name: &str, message: &str) -> VmError {
    VmError::Panicked {
        owner: name.to_owned(),
        message: message.to_owned(),
    }
}

fn assert_panic_message(error: &VmError, name: &str, wanted: &str) {
    assert!(
        matches!(error, VmError::Panicked { owner, message }
            if owner == name && message.contains(wanted)),
        "{error:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// 1. After a host panic in a posted callback
// ---------------------------------------------------------------------------------------------

#[test]
fn a_host_panic_in_a_posted_callback_is_seen_while_the_thread_is_held_before_it_returns() {
    let name = "tf_panic_held";
    let probe = Probe::new();
    let wakes = Wakes::new();
    let (arrived, release) = hold_points(name, &[Point::BeforeReturn]);
    let (mut owner, _dir) = probe.spawn(name, SCRIPT, 2, Some(wakes.wake()));
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    owner.post("boom", Vec::new()).expect("posted");

    // The thread has made its final wake and is held before it returns.
    assert_eq!(arrived.recv_timeout(BOUND), Ok(Point::BeforeReturn));
    assert_eq!(
        wakes.count(),
        4,
        "ready, start, the failure's report wake and the final wake"
    );
    assert!(!owner.is_finished(), "the thread is held before it returns");
    assert_eq!(owner.state(), VmState::Started);

    let error = owner.take_failure().expect("the failure was reported");
    assert_panic_message(&error, name, "boom msg");
    assert!(!owner.is_finished(), "still held");
    assert_eq!(owner.state(), VmState::Ending);
    assert!(
        owner.take_result().is_none(),
        "no result for the panicked call"
    );

    release.send(()).expect("release the thread");
    assert!(owner.wait_finished(BOUND));
    assert_eq!(owner.state(), VmState::Finished);
}

#[test]
fn a_failure_set_aside_by_an_earlier_drain_is_still_returned_and_the_queued_results_are_kept() {
    let name = "tf_set_aside";
    let probe = Probe::new();
    let wakes = Wakes::new();
    let (arrived, release) = hold_points(name, &[Point::BeforeReturn]);
    let (mut owner, _dir) = probe.spawn(name, SCRIPT, 4, Some(wakes.wake()));
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    owner
        .post("echo", vec![crate::ConfigValue::Integer(1)])
        .expect("posted");
    owner
        .post("echo", vec![crate::ConfigValue::Integer(2)])
        .expect("posted");
    owner.post("boom", Vec::new()).expect("posted");
    assert_eq!(arrived.recv_timeout(BOUND), Ok(Point::BeforeReturn));

    // `take_result` drains the channel and sets the failure aside; it must not hide it.
    let first = owner.take_result().expect("the first result");
    assert_eq!(super::returned(&first), [int(1)]);
    let error = owner.take_failure().expect("the failure is still there");
    assert_panic_message(&error, name, "boom msg");
    assert_eq!(owner.state(), VmState::Ending);

    // The results queued before the failure are returned in order after it.
    let second = owner.take_result().expect("the second result");
    assert_eq!(super::returned(&second), [int(2)]);
    assert!(owner.take_result().is_none());
    assert!(owner.take_failure().is_none(), "once");
    release.send(()).expect("release the thread");
}

#[test]
fn the_reported_failure_is_preferred_to_the_closed_channel_and_comes_back_once() {
    let name = "tf_closed_after_failure";
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started(name, SCRIPT, 2);
    owner.post("boom", Vec::new()).expect("posted");
    wait_thread_ended(&owner);

    let error = owner.take_failure().expect("the failure was reported");
    assert_panic_message(&error, name, "boom msg");
    assert_eq!(owner.state(), VmState::Ending);
    assert!(
        owner.take_failure().is_none(),
        "no second error from the closed channel"
    );
    assert!(owner.wait_finished(BOUND));
    assert!(owner.take_failure().is_none());
}

// ---------------------------------------------------------------------------------------------
// 2. Cancelled
// ---------------------------------------------------------------------------------------------

#[test]
fn an_error_that_ends_a_running_callback_after_the_cancel_flag_was_set_is_cancelled() {
    let name = "tf_cancelled";
    let probe = Probe::new();
    let wakes = Wakes::new();
    hooks::register(
        name,
        Hooks {
            at: Box::new(|_, _| {}),
            install: Some(Box::new(move |lua: &Lua| {
                let flag = hooks::cancel_flag(name);
                lua.globals().set(
                    "flag_then_fail",
                    lua.create_function(move |_, ()| {
                        flag.store(true, Ordering::SeqCst);
                        Err::<(), _>(mlua::Error::runtime("ended by an error"))
                    })?,
                )
            })),
        },
    );
    let (mut owner, _dir) = probe.spawn(
        name,
        "ext.on_fail(function() flag_then_fail() end)\n",
        2,
        Some(wakes.wake()),
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    assert!(
        owner.take_failure().is_none(),
        "nothing yet, and the flag is not set by the call"
    );
    owner.post("fail", Vec::new()).expect("posted");
    // ready, start, the failure's report wake, the final wake.
    wakes.wait_for(4);
    assert_eq!(
        owner.state(),
        VmState::Started,
        "the handle stays Started until the failure is observed"
    );
    assert_eq!(
        owner.take_failure(),
        Some(VmError::Cancelled {
            owner: name.to_owned()
        })
    );
    assert_eq!(owner.state(), VmState::Ending);
    assert!(owner.take_failure().is_none());
}

#[test]
fn after_the_public_cancel_the_handle_is_ending_and_there_is_no_failure_to_take() {
    let name = "tf_public_cancel";
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started(name, SCRIPT, 2);
    owner.cancel();
    assert_eq!(owner.state(), VmState::Ending);
    assert!(owner.take_failure().is_none());
    assert_eq!(owner.state(), VmState::Ending);
    assert!(owner.wait_finished(BOUND));
    assert_eq!(owner.state(), VmState::Finished);
    assert!(owner.take_failure().is_none());
}

// ---------------------------------------------------------------------------------------------
// 3. A closed channel without a report
// ---------------------------------------------------------------------------------------------

#[test]
fn a_thread_that_ends_without_a_report_gives_the_fixed_panicked_text_once() {
    let name = "tf_no_report";
    let probe = Probe::new();
    let wakes = Wakes::new();
    hooks::suppress_report(name);
    let (mut owner, _dir) = probe.spawn(name, SCRIPT, 2, Some(wakes.wake()));
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    owner.post("boom", Vec::new()).expect("posted");
    // ready, start and the final wake: the failure's report and its wake were suppressed.
    wakes.wait_for(3);
    wait_thread_ended(&owner);
    assert_eq!(owner.state(), VmState::Started);

    assert_eq!(owner.take_failure(), Some(panicked(name, NO_REPORT)));
    assert_eq!(owner.state(), VmState::Ending);
    assert!(owner.take_failure().is_none(), "once");
    assert!(owner.wait_finished(BOUND));
    assert!(owner.take_failure().is_none());
}

// ---------------------------------------------------------------------------------------------
// 4. Nothing to report
// ---------------------------------------------------------------------------------------------

#[test]
fn with_nothing_to_report_it_changes_nothing_and_the_results_come_back_in_order() {
    let name = "tf_nothing";
    let probe = Probe::new();
    let wakes = Wakes::new();
    let (mut owner, _dir) = probe.spawn(name, SCRIPT, 4, Some(wakes.wake()));
    owner.wait_ready(BOUND).expect("ready");
    assert!(owner.take_failure().is_none(), "legal in Ready");
    assert_eq!(owner.state(), VmState::Ready);
    owner.start(BOUND).expect("started");
    owner
        .post("echo", vec![crate::ConfigValue::Integer(10)])
        .expect("posted");
    owner
        .post("echo", vec![crate::ConfigValue::Integer(20)])
        .expect("posted");
    wakes.wait_for(4);

    let flag = hooks::cancel_flag(name);
    let sent = hooks::sent_so_far(name);
    for _ in 0..3 {
        assert!(owner.take_failure().is_none());
    }
    assert_eq!(owner.state(), VmState::Started);
    assert!(!flag.load(Ordering::SeqCst), "the cancel flag stays clear");
    assert_eq!(hooks::sent_so_far(name), sent, "no command was sent");
    assert_eq!(owner.in_flight(), 2);
    assert_eq!(super::returned(&next_result(&mut owner)), [int(10)]);
    assert_eq!(super::returned(&next_result(&mut owner)), [int(20)]);
    assert!(owner.take_result().is_none());
    owner.stop(BOUND).expect("a clean stop");
}

// ---------------------------------------------------------------------------------------------
// 5. Other states: nothing is consumed, the awaited report stays for its call
// ---------------------------------------------------------------------------------------------

#[test]
fn in_initialising_the_failure_stays_for_wait_ready() {
    let name = "tf_initialising";
    let reports = reported(name);
    let (mut owner, _dir) = super::super::spawn(name, "error('main failed')\n");
    reports
        .recv_timeout(BOUND)
        .expect("the failure was reported");
    assert!(owner.take_failure().is_none());
    assert_eq!(owner.state(), VmState::Initialising);
    let error = owner
        .wait_ready(BOUND)
        .expect_err("the failure is delivered here");
    assert!(script_error(&error, name).0.contains("main failed"));
}

#[test]
fn in_starting_the_failure_stays_for_await_started() {
    let name = "tf_starting";
    let reports = reported(name);
    let (mut owner, _dir) =
        super::super::spawn(name, "function startup() error('startup failed') end\n");
    owner.wait_ready(BOUND).expect("ready");
    reports
        .recv_timeout(BOUND)
        .expect("the ready report was marked");
    owner.request_start().expect("sent");
    reports
        .recv_timeout(BOUND)
        .expect("the failure was reported");
    assert!(owner.take_failure().is_none());
    assert_eq!(owner.state(), VmState::Starting);
    let error = owner
        .await_started(BOUND)
        .expect_err("the failure is delivered here");
    assert!(script_error(&error, name).0.contains("startup failed"));
}

#[test]
fn in_stopping_the_failure_stays_for_await_stopped() {
    let name = "tf_stopping";
    let reports = reported(name);
    let (mut owner, _dir) =
        super::super::spawn(name, "function shutdown() error('shutdown failed') end\n");
    owner.wait_ready(BOUND).expect("ready");
    reports
        .recv_timeout(BOUND)
        .expect("the ready report was marked");
    owner.request_stop().expect("sent");
    reports
        .recv_timeout(BOUND)
        .expect("the failure was reported");
    assert!(owner.take_failure().is_none());
    assert_eq!(owner.state(), VmState::Stopping);
    let error = owner
        .await_stopped(BOUND)
        .expect_err("the failure is delivered here");
    assert!(script_error(&error, name).0.contains("shutdown failed"));
}

#[test]
fn a_poll_still_sees_the_failure_that_take_failure_left_alone() {
    let name = "tf_poll";
    let reports = reported(name);
    let (mut owner, _dir) = super::super::spawn(name, "error('main failed')\n");
    reports
        .recv_timeout(BOUND)
        .expect("the failure was reported");
    assert!(owner.take_failure().is_none());
    let end = Instant::now() + BOUND;
    let error = loop {
        match owner.poll_ready(Duration::from_millis(10)) {
            Ok(crate::Progress::Pending) => {
                assert!(Instant::now() < end, "the poll never completed");
                thread::yield_now();
            }
            other => break other.expect_err("the failure"),
        }
    };
    assert!(script_error(&error, name).0.contains("main failed"));
}

#[test]
fn in_ending_and_finished_there_is_nothing_to_take() {
    let name = "tf_ending_finished";
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started(name, SCRIPT, 2);
    owner.post("boom", Vec::new()).expect("posted");
    wait_thread_ended(&owner);
    assert!(owner.take_failure().is_some());
    assert_eq!(owner.state(), VmState::Ending);
    assert!(owner.take_failure().is_none());
    assert!(owner.wait_finished(BOUND));
    assert_eq!(owner.state(), VmState::Finished);
    assert!(owner.take_failure().is_none());
}
