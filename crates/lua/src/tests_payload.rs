//! CK-101: a panic's payload whose `Drop` panics is disposed of without unwinding again, at every place the
//! crate catches an unwind or joins a thread that may have panicked.
//!
//! Each test raises `Bomb` (a payload whose `Drop` panics and counts itself) at one site and shows the site's
//! own result still happens: the same outcome as the same panic with an ordinary payload. Without the
//! disposal the drop's panic would unwind out of the boundary that caught the first, and each test fails (the
//! negative controls are in the result, run with `ck101-evidence/scripts/mutation_probe.py`).

use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{
    BOUND, Extension, Hooks, Point, Probe, VmError, VmState, Wakes, hooks, host, next_result,
    returned, spawn_with_settings, wait_thread_ended,
};
use crate::ConfigValue;
use crate::runner::Wake;
use crate::unwind::tests::Bomb;

/// The text of a panic whose payload is neither a `String` nor a `&str`: the payload of every `Bomb`.
const NON_STRING: &str = "a panic with a non-string payload";

fn counter() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

/// Raises a `Bomb` that counts its drop in `drops`.
fn explode(drops: &Arc<AtomicUsize>) -> ! {
    panic_any(Bomb {
        drops: Arc::clone(drops),
        then: None,
    })
}

/// Hooks for the owner called `name` that raise a `Bomb` at `point` (and do nothing else).
fn bomb_at(name: &str, point: Point, drops: &Arc<AtomicUsize>) {
    let drops = Arc::clone(drops);
    hooks::register(
        name,
        Hooks {
            at: Box::new(move |at, _| {
                if at == point {
                    explode(&drops);
                }
            }),
            install: None,
        },
    );
}

fn panicked(name: &str, message: &str) -> VmError {
    VmError::Panicked {
        owner: name.to_owned(),
        message: message.to_owned(),
    }
}

// ---------------------------------------------------------------------------------------------
// The report wake
// ---------------------------------------------------------------------------------------------

#[test]
fn a_wake_that_panics_with_such_a_payload_leaves_the_reports_and_the_run_as_they_are() {
    let name = "pl_wake";
    let drops = counter();
    let wakes = counter();
    let wake: Wake = {
        let drops = Arc::clone(&drops);
        let wakes = Arc::clone(&wakes);
        Arc::new(move || {
            wakes.fetch_add(1, Ordering::SeqCst);
            explode(&drops);
        })
    };
    let probe = Probe::new();
    let (mut owner, _dir) = probe.spawn(
        name,
        "ext.on_echo(function(a) return a end)\n",
        2,
        Some(wake),
    );
    // Every report was queued before its wake panicked, and the thread went on to serve the next request.
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    owner
        .post("echo", vec![ConfigValue::Integer(7)])
        .expect("posted");
    assert_eq!(
        returned(&next_result(&mut owner)),
        [Some(ConfigValue::Integer(7))]
    );
    owner.stop(BOUND).expect("a clean stop");
    assert!(owner.wait_finished(BOUND));
    assert_eq!(owner.state(), VmState::Finished);
    // ready, start, the call's result, the stop's reply and the final wake: each payload dropped once.
    assert_eq!(wakes.load(Ordering::SeqCst), 5);
    assert_eq!(drops.load(Ordering::SeqCst), 5);
}

// ---------------------------------------------------------------------------------------------
// The final flush
// ---------------------------------------------------------------------------------------------

#[test]
fn a_flush_that_panics_with_such_a_payload_still_ends_the_stop_normally_with_the_state_dropped() {
    let name = "pl_flush";
    let drops = counter();
    let wakes = Wakes::new();
    let dropped = super::super::track_state_drop(name, {
        let drops = Arc::clone(&drops);
        move |point| {
            if point == Point::Flushing {
                explode(&drops);
            }
        }
    });
    let probe = Probe::new();
    let (mut owner, _dir) = probe.spawn(
        name,
        "ext.on_echo(function(a) return a end)\n",
        2,
        Some(wakes.wake()),
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    // Not `Failed(Panicked)`: the reply of the stop is decided after the flush and the state's drop.
    owner.stop(BOUND).expect("the stop is answered Done");
    assert!(owner.wait_finished(BOUND));
    assert_eq!(owner.state(), VmState::Finished);
    assert_eq!(drops.load(Ordering::SeqCst), 1, "the payload was dropped");
    assert!(dropped.load(Ordering::SeqCst), "the state was dropped");
    // ready, start, the stop's reply and the final wake.
    assert_eq!(wakes.count(), 4);
}

// ---------------------------------------------------------------------------------------------
// The thread's body
// ---------------------------------------------------------------------------------------------

#[test]
fn a_panic_in_the_threads_body_with_such_a_payload_is_the_same_failed_reply_and_final_wake() {
    let name = "pl_body";
    let drops = counter();
    let wakes = Wakes::new();
    let dropped = super::super::track_state_drop(name, {
        let drops = Arc::clone(&drops);
        move |point| {
            if point == Point::AfterMain {
                explode(&drops);
            }
        }
    });
    let probe = Probe::new();
    let (mut owner, _dir) = probe.spawn(name, "ext.count()\n", 2, Some(wakes.wake()));
    // The same failure an ordinary non-string payload gives, not the text of the payload's own drop panic.
    let error = owner.wait_ready(BOUND).expect_err("the owner panicked");
    assert_eq!(error, panicked(name, NON_STRING));
    assert_eq!(owner.state(), VmState::Ending);
    assert!(owner.wait_finished(BOUND));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(dropped.load(Ordering::SeqCst), "the state was dropped");
    // The failure's report and the final wake.
    assert_eq!(wakes.count(), 2);
}

// ---------------------------------------------------------------------------------------------
// The host function boundary
// ---------------------------------------------------------------------------------------------

#[test]
fn a_host_function_that_panics_with_such_a_payload_is_recorded_and_interrupts_the_script() {
    let name = "pl_extension";
    let drops = counter();
    let after = counter();
    let wakes = Wakes::new();
    let extension = Extension::new("ext")
        .function("bomb", {
            let drops = Arc::clone(&drops);
            host(move |_| explode(&drops))
        })
        .function("after", {
            let after = Arc::clone(&after);
            host(move |_| {
                after.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            })
        })
        .callback("on_boom", "boom");
    let (mut owner, _dir) = spawn_with_settings(
        name,
        "ext.on_boom(function() ext.bomb() ext.after() end)\n",
        {
            let wake = wakes.wake();
            move |settings| {
                settings.extensions = vec![extension];
                settings.call_limit = 2;
                settings.on_report = Some(wake);
            }
        },
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    owner.post("boom", Vec::new()).expect("posted");
    wait_thread_ended(&owner);

    // The recorded message, not the text of the payload's own drop panic; the script did not go on.
    assert_eq!(owner.take_failure(), Some(panicked(name, NON_STRING)));
    assert_eq!(
        after.load(Ordering::SeqCst),
        0,
        "the script was interrupted"
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert!(owner.wait_finished(BOUND));
    // ready, start, the failure's report and the final wake.
    assert_eq!(wakes.count(), 4);
}

// ---------------------------------------------------------------------------------------------
// The two joins of a finished thread
// ---------------------------------------------------------------------------------------------
//
// `Point::BeforeReturn` is reached after `thread_main`'s own `catch_unwind`, so a panic raised there ends the
// thread with its payload: `join` returns it, as it would for a thread lost in any other way.

#[test]
fn a_finished_thread_that_ended_by_such_a_panic_is_joined_and_settled_by_wait_finished() {
    let name = "pl_join_wait";
    let drops = counter();
    bomb_at(name, Point::BeforeReturn, &drops);
    let (mut owner, _dir) = super::super::spawn(name, "log.info('main')\n");
    owner.wait_ready(BOUND).expect("ready");
    owner.cancel();
    wait_thread_ended(&owner);

    let settled = catch_unwind(AssertUnwindSafe(|| owner.wait_finished(BOUND)));
    assert_eq!(
        settled.ok(),
        Some(true),
        "the payload's panic escaped the join"
    );
    assert_eq!(owner.state(), VmState::Finished);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn a_finished_thread_that_ended_by_such_a_panic_is_joined_by_the_owners_drop() {
    let name = "pl_join_drop";
    let drops = counter();
    bomb_at(name, Point::BeforeReturn, &drops);
    let (mut owner, _dir) = super::super::spawn(name, "log.info('main')\n");
    owner.wait_ready(BOUND).expect("ready");
    owner.cancel();
    wait_thread_ended(&owner);

    let dropped = catch_unwind(AssertUnwindSafe(move || drop(owner)));
    assert!(dropped.is_ok(), "the payload's panic escaped the drop");
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
