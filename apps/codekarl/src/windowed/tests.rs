//! The windowed run, with real Lua VMs and the fake backend and presenter (`crate::fakes`): the end
//! mapping, the lazy backend and the resource latch, the late callback failures, and the UI link's
//! disconnection rules (the final-wake regression is proven with gates, not sleeps).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, PoisonError};
use std::thread;

use ck_lua::HostFunction;
use ck_window::{NativeEvent, UiEvent};

use super::*;
use crate::fakes::{
    BackendScript, End, FakeBackend, FakePresenter, PresenterScript, Recorder, vector_recorder,
};
use crate::testing::{Extras, Gate, Harness, WINDOW};

/// What a run did.
struct Outcome {
    end: RunEnd,
    events: Vec<String>,
    latch_during_make: Option<bool>,
}

/// Runs the windowed code over the fakes.
fn run_with(
    harness: &Harness,
    backend: BackendScript,
    presenter: PresenterScript,
    latch: &ResourceLatch,
    drain: Duration,
) -> Outcome {
    run_hooked(harness, backend, presenter, latch, drain, |_| {})
}

fn run_hooked(
    harness: &Harness,
    backend: BackendScript,
    presenter: PresenterScript,
    latch: &ResourceLatch,
    drain: Duration,
    hook: impl FnOnce(&mut FakeBackend),
) -> Outcome {
    run_hooked_in(
        &harness.env(),
        harness,
        backend,
        presenter,
        latch,
        drain,
        hook,
    )
}

/// `run_hooked` with the run environment the caller made (its loop settings, for one).
fn run_hooked_in(
    env: &RunEnv,
    harness: &Harness,
    backend: BackendScript,
    presenter: PresenterScript,
    latch: &ResourceLatch,
    drain: Duration,
    hook: impl FnOnce(&mut FakeBackend),
) -> Outcome {
    let (record, events) = vector_recorder();
    let seen = Cell::new(None);
    let make = {
        let record: Recorder = Arc::clone(&record);
        let seen = &seen;
        move || {
            seen.set(Some(latch.is_set()));
            let mut made = FakeBackend::new(backend, record);
            hook(&mut made);
            Ok(made)
        }
    };
    let presenter = FakePresenter::new(Arc::clone(&harness.notifier), presenter, record);
    let end = run_window(env, make, presenter, latch, drain);
    let events = events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    Outcome {
        end,
        events,
        latch_during_make: seen.get(),
    }
}

fn ok_run(harness: &Harness, latch: &ResourceLatch) -> Outcome {
    run_with(
        harness,
        BackendScript::default(),
        PresenterScript::default(),
        latch,
        DRAIN,
    )
}

fn failed(end: &RunEnd) -> &str {
    match end {
        RunEnd::Failed(message) => message,
        other => panic!("expected a failed run, got {other:?}"),
    }
}

#[test]
fn a_run_where_the_ui_never_opens_a_window_creates_nothing_and_never_sets_the_latch() {
    let harness = Harness::new("", Extras::default());
    let latch = ResourceLatch::new();
    let outcome = ok_run(&harness, &latch);
    assert_eq!(outcome.end, RunEnd::Finished);
    assert_eq!(
        outcome.latch_during_make, None,
        "the backend was never made"
    );
    assert!(
        outcome
            .events
            .iter()
            .all(|event| event == "presenter dropped"),
        "{:?}",
        outcome.events
    );
    assert!(!latch.is_set());
    assert!(!harness.notes.unproven.get());
}

#[test]
fn an_open_and_a_close_in_the_first_batch_touch_no_backend() {
    let harness = Harness::new(&format!("{WINDOW}\nwindow.close()"), Extras::default());
    let latch = ResourceLatch::new();
    let outcome = ok_run(&harness, &latch);
    assert_eq!(outcome.end, RunEnd::Finished);
    assert_eq!(outcome.latch_during_make, None);
    assert!(!latch.is_set());
}

#[test]
fn a_window_closed_by_the_script_sets_the_latch_before_the_backend_exists_and_clears_it_after_a_proven_end()
 {
    let harness = Harness::new(
        &format!("{WINDOW}\nwindow.on_opened(function() window.close() end)"),
        Extras::default(),
    );
    let latch = ResourceLatch::new();
    let outcome = ok_run(&harness, &latch);
    assert_eq!(outcome.end, RunEnd::Finished, "{:?}", outcome.events);
    assert_eq!(
        outcome.latch_during_make,
        Some(true),
        "the latch is set immediately before the first native object"
    );
    assert!(!latch.is_set(), "a proven teardown clears it");
    let order: Vec<_> = outcome
        .events
        .iter()
        .map(String::as_str)
        .filter(|event| !event.ends_with("dropped"))
        .collect();
    assert_eq!(
        order,
        [
            "create_window",
            "presenter start",
            "presenter stop",
            "destroy_window",
            "terminate"
        ]
    );
    assert!(!harness.notes.unproven.get());
}

#[test]
fn a_quit_during_the_run_ends_it_with_quit_after_a_proven_teardown() {
    let harness = Harness::new(WINDOW, Extras::default());
    let latch = ResourceLatch::new();
    let quit = Arc::clone(&harness.quit);
    let outcome = run_hooked(
        &harness,
        BackendScript::default(),
        PresenterScript::default(),
        &latch,
        DRAIN,
        move |backend| {
            backend.on_pump = Some(Box::new(move |pumps| {
                if pumps >= 2 {
                    quit.store(true, Ordering::SeqCst);
                }
            }));
        },
    );
    assert_eq!(outcome.end, RunEnd::Quit, "{:?}", outcome.events);
    assert!(!latch.is_set());
    assert!(outcome.events.iter().any(|event| event == "destroy_window"));
}

#[test]
fn a_presenter_failure_published_while_stopping_fails_a_proven_close() {
    let harness = Harness::new(
        &format!("{WINDOW}\nwindow.on_opened(function() window.close() end)"),
        Extras::default(),
    );
    let latch = ResourceLatch::new();
    let outcome = run_with(
        &harness,
        BackendScript::default(),
        PresenterScript {
            fail_at_stop: vec!["gpu lost at stop".to_owned()],
            ..PresenterScript::default()
        },
        &latch,
        DRAIN,
    );
    assert_eq!(
        outcome.end,
        RunEnd::Failed("the presenter failed: gpu lost at stop".to_owned()),
        "{:?}",
        outcome.events
    );
    assert!(!latch.is_set(), "the teardown was proven");
    assert!(outcome.events.iter().any(|event| event == "destroy_window"));
    assert!(!harness.notes.unproven.get());
}

#[test]
fn a_presenter_failure_published_while_stopping_fails_a_quit_too() {
    let harness = Harness::new(WINDOW, Extras::default());
    let latch = ResourceLatch::new();
    let quit = Arc::clone(&harness.quit);
    let outcome = run_hooked(
        &harness,
        BackendScript::default(),
        PresenterScript {
            fail_at_end: vec!["gpu lost in the last poll".to_owned()],
            ..PresenterScript::default()
        },
        &latch,
        DRAIN,
        move |backend| {
            backend.on_pump = Some(Box::new(move |pumps| {
                if pumps >= 2 {
                    quit.store(true, Ordering::SeqCst);
                }
            }));
        },
    );
    assert_eq!(
        outcome.end,
        RunEnd::Failed("the presenter failed: gpu lost in the last poll".to_owned()),
        "{:?}",
        outcome.events
    );
    assert!(!latch.is_set());
}

#[test]
fn an_unproven_teardown_with_a_late_presenter_failure_stays_incomplete() {
    let harness = Harness::new(
        &format!("{WINDOW}\nwindow.on_opened(function() window.close() end)"),
        Extras::default(),
    );
    let latch = ResourceLatch::new();
    let outcome = run_with(
        &harness,
        BackendScript::default(),
        PresenterScript {
            fail_at_stop: vec!["gpu lost at stop".to_owned()],
            end: End::Unproven("an unproven GPU wait".to_owned()),
            ..PresenterScript::default()
        },
        &latch,
        DRAIN,
    );
    assert!(
        failed(&outcome.end).starts_with("the window teardown is unproven: "),
        "{:?}",
        outcome.end
    );
    assert!(latch.is_set(), "unproven keeps the latch");
    assert!(harness.notes.unproven.get());
    assert!(!outcome.events.iter().any(|event| event == "destroy_window"));
}

#[test]
fn a_callback_that_raises_ends_the_run_failed_with_its_traceback_after_a_proven_teardown() {
    let harness = Harness::new(
        &format!("{WINDOW}\nwindow.on_opened(function() error('opened exploded') end)"),
        Extras::default(),
    );
    let latch = ResourceLatch::new();
    let outcome = ok_run(&harness, &latch);
    let message = failed(&outcome.end);
    assert!(
        message.starts_with("ui callback opened failed: "),
        "{message}"
    );
    assert!(message.contains("opened exploded"), "{message}");
    assert!(message.contains("stack traceback"), "{message}");
    assert!(!latch.is_set());
    assert!(outcome.events.iter().any(|event| event == "destroy_window"));
    assert!(
        !harness.notes.unproven.get(),
        "a callback failure is not an unproven teardown"
    );
}

#[test]
fn a_backend_that_cannot_be_made_fails_the_run_with_nothing_unproven() {
    let harness = Harness::new(WINDOW, Extras::default());
    let latch = ResourceLatch::new();
    let (record, events) = vector_recorder();
    let presenter = FakePresenter::new(
        Arc::clone(&harness.notifier),
        PresenterScript::default(),
        record,
    );
    let seen = Cell::new(None);
    let end = run_window(
        &harness.env(),
        || {
            seen.set(Some(latch.is_set()));
            Err::<FakeBackend, _>("AppKit refused".to_owned())
        },
        presenter,
        &latch,
        DRAIN,
    );
    assert!(failed(&end).contains("AppKit refused"), "{end:?}");
    assert_eq!(
        seen.get(),
        Some(true),
        "the latch was set before the attempt"
    );
    assert!(
        !latch.is_set(),
        "nothing exists, so the proven end clears it"
    );
    assert!(!harness.notes.unproven.get());
    assert!(
        !events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|event| event == "presenter start")
    );
}

#[test]
fn a_presenter_that_cannot_start_fails_the_run_and_tears_the_window_down() {
    let harness = Harness::new(WINDOW, Extras::default());
    let latch = ResourceLatch::new();
    let outcome = run_with(
        &harness,
        BackendScript::default(),
        PresenterScript {
            start_error: Some("no Metal device".to_owned()),
            ..PresenterScript::default()
        },
        &latch,
        DRAIN,
    );
    assert!(
        failed(&outcome.end).contains("no Metal device"),
        "{:?}",
        outcome.end
    );
    assert!(
        !latch.is_set(),
        "no render thread was spawned, so the cleanup is proven"
    );
    assert!(outcome.events.iter().any(|event| event == "destroy_window"));
    assert!(!harness.notes.unproven.get());
}

#[test]
fn a_presenter_failure_with_a_proven_end_is_a_plain_failure() {
    let harness = Harness::new(WINDOW, Extras::default());
    let latch = ResourceLatch::new();
    let outcome = run_with(
        &harness,
        BackendScript::default(),
        PresenterScript {
            first_frame: false,
            fail: Some("the GPU setup failed".to_owned()),
            ..PresenterScript::default()
        },
        &latch,
        DRAIN,
    );
    assert!(
        failed(&outcome.end).contains("the GPU setup failed"),
        "{:?}",
        outcome.end
    );
    assert!(!latch.is_set());
    assert!(!harness.notes.unproven.get());
}

#[test]
fn an_unproven_teardown_keeps_the_latch_and_every_object_and_marks_the_run_incomplete() {
    for end in [
        End::Unproven("the render thread panicked".to_owned()),
        End::Never,
    ] {
        let harness = Harness::new(
            &format!("{WINDOW}\nwindow.on_opened(function() window.close() end)"),
            Extras::default(),
        );
        let latch = ResourceLatch::new();
        let outcome = run_with(
            &harness,
            BackendScript::default(),
            PresenterScript {
                end,
                ..PresenterScript::default()
            },
            &latch,
            DRAIN,
        );
        assert!(
            failed(&outcome.end).contains("unproven"),
            "{:?}",
            outcome.end
        );
        assert!(latch.is_set(), "the latch stays set");
        assert!(harness.notes.unproven.get(), "the run is incomplete");
        assert_eq!(
            &*harness.notes.resource_holders.borrow(),
            &["render thread (unproven presenter end)"],
            "the fake presenter leaves its frames with a named render-thread holder"
        );
        for released in [
            "destroy_window",
            "terminate",
            "backend dropped",
            "presenter dropped",
        ] {
            assert!(
                !outcome.events.iter().any(|event| event == released),
                "{released} must not happen: {:?}",
                outcome.events
            );
        }
    }
}

/// A host function that blocks until the gate opens.
fn wait_for(gate: &Arc<Gate>) -> HostFunction {
    let gate = Arc::clone(gate);
    Arc::new(move |_| {
        gate.wait();
        Ok(Vec::new())
    })
}

const CLOSE_THEN_FAIL: &str =
    "window.on_opened(function()\n  window.close()\n  window.hold()\n  error('late failure')\nend)";

#[test]
fn a_callback_that_fails_within_the_drain_after_the_close_still_fails_the_run() {
    let gate = Gate::new();
    let harness = Harness::new(
        &format!("{WINDOW}\n{CLOSE_THEN_FAIL}"),
        Extras {
            functions: vec![("hold", wait_for(&gate))],
            after_wake: None,
            ..Extras::default()
        },
    );
    let latch = ResourceLatch::new();
    let opener = {
        let gate = Arc::clone(&gate);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            gate.open();
        })
    };
    let outcome = run_with(
        &harness,
        BackendScript::default(),
        PresenterScript::default(),
        &latch,
        Duration::from_secs(10),
    );
    opener.join().expect("the opener");
    let message = failed(&outcome.end);
    assert!(message.contains("late failure"), "{message}");
    assert!(message.contains("stack traceback"), "{message}");
    assert_eq!(harness.ui.outstanding(), 0);
}

#[test]
fn a_callback_still_running_at_the_drain_bound_is_left_for_the_post_stop_fold() {
    let gate = Gate::new();
    let harness = Harness::new(
        &format!("{WINDOW}\n{CLOSE_THEN_FAIL}"),
        Extras {
            functions: vec![("hold", wait_for(&gate))],
            after_wake: None,
            ..Extras::default()
        },
    );
    let latch = ResourceLatch::new();
    let started = Instant::now();
    let outcome = run_with(
        &harness,
        BackendScript::default(),
        PresenterScript::default(),
        &latch,
        Duration::from_millis(300),
    );
    assert_eq!(outcome.end, RunEnd::Finished, "the run itself closed");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(harness.ui.outstanding_names(), ["opened"]);
    // The stop phase would now run; the callback is released during it and fails.
    gate.open();
    let give_up = Instant::now() + Duration::from_secs(10);
    let late = loop {
        if let Some(message) = harness.ui.take_remaining_failures().into_iter().next() {
            break message;
        }
        assert!(Instant::now() < give_up, "the late failure never arrived");
        thread::sleep(Duration::from_millis(2));
    };
    assert!(late.starts_with("ui callback opened failed: "), "{late}");
    assert!(late.contains("late failure"), "{late}");
}

#[test]
fn the_drain_does_not_wait_once_a_quit_is_requested() {
    let gate = Gate::new();
    let harness = Harness::new(
        &format!("{WINDOW}\n{CLOSE_THEN_FAIL}"),
        Extras {
            functions: vec![("hold", wait_for(&gate))],
            after_wake: None,
            ..Extras::default()
        },
    );
    let latch = ResourceLatch::new();
    harness.quit.store(true, Ordering::SeqCst);
    // The quit is seen at once: the loop never even creates the window, and nothing is outstanding.
    let _ = ok_run(&harness, &latch);
    // Directly: with a call outstanding and the quit flag set, the drain returns at once.
    let mut ui = harness.ui.clone();
    ui.post(UiEvent::Opened).expect("posted");
    let started = Instant::now();
    assert!(drain_late(&harness.quit, &harness.ui, Duration::from_secs(10)).is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    gate.open();
}

#[test]
fn the_loop_ends_are_mapped_to_the_runtimes_ends() {
    let harness = Harness::new("", Extras::default());
    let notes = RunNotes::default();
    for (end, wanted) in [
        (LoopEnd::NoWindow, RunEnd::Finished),
        (LoopEnd::Closed, RunEnd::Finished),
        (LoopEnd::Quit, RunEnd::Quit),
        (
            LoopEnd::CallbackFailed {
                message: "ui callback close_request failed: x".to_owned(),
            },
            RunEnd::Failed("ui callback close_request failed: x".to_owned()),
        ),
        (
            LoopEnd::Failed {
                message: "cannot create the window: y".to_owned(),
            },
            RunEnd::Failed("cannot create the window: y".to_owned()),
        ),
        (
            LoopEnd::PresenterFailed {
                message: "the presenter failed: y".to_owned(),
            },
            RunEnd::Failed("the presenter failed: y".to_owned()),
        ),
    ] {
        assert_eq!(run_end(end, &harness.ui, &notes), wanted);
    }
    assert!(!notes.unproven.get());
    let unproven = run_end(
        LoopEnd::Unproven {
            reason: "z".to_owned(),
        },
        &harness.ui,
        &notes,
    );
    assert_eq!(
        unproven,
        RunEnd::Failed("the window teardown is unproven: z".to_owned())
    );
    assert!(notes.unproven.get());
}

#[test]
fn a_presenter_failure_is_not_replaced_by_a_ui_failure_the_link_saw() {
    let ran = Arc::new(AtomicBool::new(false));
    let boom = panic_function(&ran);
    let harness = Harness::new(
        "window.on_opened(function() window.boom() end)",
        Extras {
            functions: vec![("boom", boom)],
            after_wake: None,
            ..Extras::default()
        },
    );
    let mut link = harness.ui.clone();
    link.post(UiEvent::Opened).expect("posted");
    let give_up = Instant::now() + Duration::from_secs(10);
    while !link.is_disconnected() {
        assert!(Instant::now() < give_up, "the UI failure was never seen");
        thread::sleep(Duration::from_millis(1));
    }
    let notes = RunNotes::default();
    // A failure of the UI VM is not the presenter's, and the presenter's failure is not the UI's.
    let ended = run_end(
        LoopEnd::PresenterFailed {
            message: "the presenter failed: y".to_owned(),
        },
        &harness.ui,
        &notes,
    );
    let message = failed(&ended);
    assert!(message.starts_with("the presenter failed: y"), "{message}");
    assert!(
        message.contains("the ui VM also failed: ")
            && message.contains("a host panic in a callback"),
        "{message}"
    );
}

// ---------------------------------------------------------------------------------------------
// The UI link: disconnection (CK-62 §7.3)
// ---------------------------------------------------------------------------------------------

/// A host function that records it ran and then panics (a host panic in a callback sends no call result).
fn panic_function(ran: &Arc<AtomicBool>) -> HostFunction {
    let ran = Arc::clone(ran);
    Arc::new(move |_| {
        ran.store(true, Ordering::SeqCst);
        panic!("a host panic in a callback");
    })
}

#[test]
fn the_final_wake_regression_a_host_panic_with_the_owner_held_after_its_wake_ends_the_loop_failed()
{
    // The UI VM's callback hits a host panic: no call result is sent, only the terminal failure report,
    // and then the report wake. The gate holds the owner thread *inside* that wake, so it never returns
    // and no further wake follows. The loop, woken by that wake, must see the failure through
    // `is_disconnected()` on its next pass instead of going back to sleep.
    let gate = Gate::new();
    let ran = Arc::new(AtomicBool::new(false));
    let hold = Arc::clone(&gate);
    let flag = Arc::clone(&ran);
    let harness = Harness::new(
        &format!("{WINDOW}\nwindow.on_opened(function() window.boom() end)"),
        Extras {
            functions: vec![("boom", panic_function(&ran))],
            after_wake: Some(Arc::new(move || {
                if flag.load(Ordering::SeqCst) {
                    hold.wait();
                }
            })),
            ..Extras::default()
        },
    );
    let latch = ResourceLatch::new();
    let started = Instant::now();
    let outcome = run_with(
        &harness,
        BackendScript {
            // A missed wake would leave the loop asleep for this long: far past the assertion below.
            wait_bound: Duration::from_secs(15),
            ..BackendScript::default()
        },
        PresenterScript::default(),
        &latch,
        DRAIN,
    );
    let took = started.elapsed();
    gate.open();
    assert!(ran.load(Ordering::SeqCst));
    let message = failed(&outcome.end);
    assert!(message.contains("the ui VM failed"), "{message}");
    assert!(message.contains("a host panic in a callback"), "{message}");
    assert!(
        took < Duration::from_secs(8),
        "the loop slept instead of seeing the failure: {took:?}"
    );
    assert!(!harness.vm.borrow().is_finished() || harness.ui.failure().is_some());
}

#[test]
fn a_ui_vm_that_was_cancelled_ends_the_loop_failed() {
    let harness = Harness::new(WINDOW, Extras::default());
    // `cancel` moves the handle to Ending at once: it has ended for the caller.
    harness.vm.borrow_mut().cancel();
    let latch = ResourceLatch::new();
    let started = Instant::now();
    let outcome = run_with(
        &harness,
        BackendScript {
            wait_bound: Duration::from_secs(15),
            ..BackendScript::default()
        },
        PresenterScript::default(),
        &latch,
        DRAIN,
    );
    assert!(
        failed(&outcome.end).contains("the ui VM ended"),
        "{:?}",
        outcome.end
    );
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(!latch.is_set());
}

// ---------------------------------------------------------------------------------------------
// The owner typing (spec r5)
// ---------------------------------------------------------------------------------------------

#[test]
fn extra_close_requests_from_a_typing_owner_are_declined_and_change_nothing() {
    // The fixture declines every close request (as every windowed fixture does) and counts them. Escape
    // presses and close-button clicks are injected into the fake backend's queue at chosen passes, as real
    // typing would be; the run still ends exactly as it does without them: by the quit, not by a close.
    let declined = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&declined);
    let decline: HostFunction = Arc::new(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    });
    let harness = Harness::new(
        &format!(
            "{WINDOW}\nwindow.on_close_request(function(source) window.declined() return false end)"
        ),
        Extras {
            functions: vec![("declined", decline)],
            after_wake: None,
            ..Extras::default()
        },
    );
    let latch = ResourceLatch::new();
    let quit = Arc::clone(&harness.quit);
    let seen = Arc::clone(&declined);
    let outcome = run_hooked(
        &harness,
        BackendScript {
            wait_bound: Duration::from_millis(20),
            ..BackendScript::default()
        },
        PresenterScript::default(),
        &latch,
        DRAIN,
        |backend| {
            let queue = Arc::clone(&backend.pending);
            backend.on_pump = Some(Box::new(move |pass| {
                let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
                match pass {
                    2 | 80 => queue.extend([
                        NativeEvent::Key {
                            key: ck_window::Key::Escape,
                            down: true,
                            repeat: false,
                        },
                        NativeEvent::Key {
                            key: ck_window::Key::Escape,
                            down: false,
                            repeat: false,
                        },
                    ]),
                    // A burst: only one request is ever outstanding, the rest are absorbed.
                    40 => queue.extend([NativeEvent::CloseButton; 3]),
                    _ => {}
                }
                drop(queue);
                if seen.load(Ordering::SeqCst) >= 3 {
                    quit.store(true, Ordering::SeqCst);
                }
            }));
        },
    );
    assert_eq!(outcome.end, RunEnd::Quit, "{:?}", outcome.events);
    assert!(
        declined.load(Ordering::SeqCst) >= 3,
        "{}",
        declined.load(Ordering::SeqCst)
    );
    assert!(!latch.is_set());
    assert!(outcome.events.iter().any(|event| event == "destroy_window"));
    assert!(!harness.notes.unproven.get());
}

#[test]
fn the_configured_close_binding_replaces_escape_in_the_native_loop() {
    // The configuration's `[input.bindings]` reach the loop's mapper: with `close = ["KeyQ"]` a normalised Escape
    // requests nothing and a normalised Q requests a close; with the defaults Escape is the close binding (it is no
    // longer a hardcoded path of the loop) and Q requests nothing. The fixture declines every request it gets.
    for (bindings, escape_closes) in [
        (
            ck_window::Bindings::from_entries([("close".to_owned(), vec!["KeyQ".to_owned()])])
                .expect("valid"),
            false,
        ),
        (
            ck_window::Bindings::from_entries(std::iter::empty()).expect("the defaults"),
            true,
        ),
    ] {
        let declined = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&declined);
        let decline: HostFunction = Arc::new(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        });
        let harness = Harness::new(
            &format!(
                "{WINDOW}\nwindow.on_close_request(function(source) window.declined() return false end)"
            ),
            Extras {
                functions: vec![("declined", decline)],
                after_wake: None,
                ..Extras::default()
            },
        );
        let latch = ResourceLatch::new();
        let quit = Arc::clone(&harness.quit);
        let seen = Arc::clone(&declined);
        // How many requests the fixture had declined when Q went in.
        let before_q = Arc::new(AtomicUsize::new(usize::MAX));
        let recorded = Arc::clone(&before_q);
        let mut env = harness.env();
        env.settings = env.settings.clone().with_bindings(bindings);
        let key = |key, down| NativeEvent::Key {
            key,
            down,
            repeat: false,
        };
        let outcome = run_hooked_in(
            &env,
            &harness,
            BackendScript {
                wait_bound: Duration::from_millis(20),
                ..BackendScript::default()
            },
            PresenterScript::default(),
            &latch,
            DRAIN,
            |backend| {
                let queue = Arc::clone(&backend.pending);
                let started = Cell::new(None::<Instant>);
                backend.on_pump = Some(Box::new(move |pass| {
                    let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
                    if pass == 2 {
                        queue.extend([
                            key(ck_window::Key::Escape, true),
                            key(ck_window::Key::Escape, false),
                        ]);
                        started.set(Some(Instant::now()));
                    }
                    // Q goes in once Escape's request was declined (when Escape is bound), or after a good wait
                    // that shows Escape requested nothing.
                    if let Some(at) = started.get()
                        && recorded.load(Ordering::SeqCst) == usize::MAX
                        && (seen.load(Ordering::SeqCst) >= 1
                            || at.elapsed() > Duration::from_millis(400))
                    {
                        recorded.store(seen.load(Ordering::SeqCst), Ordering::SeqCst);
                        queue.push_back(key(ck_window::Key::KeyQ, true));
                    }
                    drop(queue);
                    let done = recorded.load(Ordering::SeqCst) != usize::MAX
                        && (!escape_closes && seen.load(Ordering::SeqCst) >= 1
                            || escape_closes
                                && started
                                    .get()
                                    .is_some_and(|at| at.elapsed() > Duration::from_millis(900)));
                    if done || pass > 4000 {
                        quit.store(true, Ordering::SeqCst);
                    }
                }));
            },
        );
        assert_eq!(outcome.end, RunEnd::Quit, "{:?}", outcome.events);
        assert_eq!(
            before_q.load(Ordering::SeqCst),
            usize::from(escape_closes),
            "Escape requests a close exactly when it is bound to close"
        );
        assert_eq!(
            declined.load(Ordering::SeqCst),
            1,
            "and in either case exactly one request in all: Q closes only when it is bound, and then Escape did not"
        );
        assert!(!latch.is_set());
    }
}

// --- the paced UI update (`frame_update`), over the real loop and a real UI VM

/// A run environment whose loop has the configured bindings and the `[frame]` update source, **off**, with the handle
/// that turns it on (milestone 5-8's `frame.on_update` will).
fn updating_env(harness: &Harness, config_text: &str) -> (RunEnv, ck_window::UpdateControl) {
    let config = crate::config::load_text_for_test(config_text);
    let updates = config.frame.update_settings().expect("valid pacing");
    let control = updates.control();
    let mut env = harness.env();
    env.settings = env
        .settings
        .clone()
        .with_bindings(config.bindings.clone())
        .with_updates(updates);
    (env, control)
}

/// A host function `window.record(value)` that collects what a script passes it.
fn recorder_function() -> (
    HostFunction,
    Arc<std::sync::Mutex<Vec<ck_lua::ConfigValue>>>,
) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let function: HostFunction = {
        let seen = Arc::clone(&seen);
        Arc::new(move |arguments| {
            if let Some(Some(value)) = arguments.first() {
                seen.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(value.clone());
            }
            Ok(Vec::new())
        })
    };
    (function, seen)
}

fn float(value: f64) -> ck_lua::ConfigValue {
    ck_lua::ConfigValue::Float(value)
}

fn flags(held: bool, pressed: bool, released: bool) -> ck_lua::ConfigValue {
    ck_lua::ConfigValue::Table(
        [("held", held), ("pressed", pressed), ("released", released)]
            .into_iter()
            .map(|(name, value)| (name.to_owned(), ck_lua::ConfigValue::Bool(value)))
            .collect(),
    )
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one run, then the table it produced checked key by key"
)]
fn the_native_loop_posts_the_frame_update_table_of_a_real_input_snapshot() {
    // Keys go in, then the update source is switched on: the first update carries a held action, a tapped one, an
    // unbound one (false) and the transitions in order, for the window the fake backend opened (100 x 80 points, twice
    // that in pixels).
    let (record, seen) = recorder_function();
    let harness = Harness::new(
        &format!("{WINDOW}\nframe.on_update(function(f) window.record(f) end)"),
        Extras {
            functions: vec![("record", record)],
            extensions: vec![crate::testing::frame_extension()],
            ..Extras::default()
        },
    );
    let (env, control) = updating_env(&harness, "[input.bindings]\nmove_up = []\n");
    let latch = ResourceLatch::new();
    let quit = Arc::clone(&harness.quit);
    let key = |key, down| NativeEvent::Key {
        key,
        down,
        repeat: false,
    };
    let looked = Arc::clone(&seen);
    let outcome = run_hooked_in(
        &env,
        &harness,
        BackendScript {
            wait_bound: Duration::from_millis(20),
            ..BackendScript::default()
        },
        PresenterScript::default(),
        &latch,
        DRAIN,
        |backend| {
            let queue = Arc::clone(&backend.pending);
            backend.on_pump = Some(Box::new(move |pass| {
                if pass == 2 {
                    queue
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .extend([
                            key(ck_window::Key::KeyA, true),
                            key(ck_window::Key::KeyR, true),
                            key(ck_window::Key::KeyR, false),
                        ]);
                }
                if pass == 8 {
                    control.set(true);
                }
                let got = looked.lock().unwrap_or_else(PoisonError::into_inner).len();
                if got >= 2 || pass > 4000 {
                    quit.store(true, Ordering::SeqCst);
                }
            }));
        },
    );
    assert_eq!(outcome.end, RunEnd::Quit, "{:?}", outcome.events);
    let seen = seen.lock().unwrap_or_else(PoisonError::into_inner);
    assert!(seen.len() >= 2, "two updates reached the script: {seen:?}");
    let ck_lua::ConfigValue::Table(first) = &seen[0] else {
        panic!("a table: {:?}", seen[0]);
    };
    assert_eq!(first["sequence"], ck_lua::ConfigValue::Integer(1));
    assert_eq!(
        first["dt"],
        float(0.0),
        "no time has passed before the first"
    );
    assert_eq!(first["elapsed"], float(0.0));
    for (key, value) in [
        ("width", 100),
        ("height", 80),
        ("pixel_width", 200),
        ("pixel_height", 160),
    ] {
        assert_eq!(first[key], ck_lua::ConfigValue::Integer(value), "{key}");
    }
    let ck_lua::ConfigValue::Table(actions) = &first["actions"] else {
        panic!("actions: {:?}", first["actions"]);
    };
    assert_eq!(
        actions["move_left"],
        flags(true, true, false),
        "held and pressed"
    );
    assert_eq!(
        actions["reload_texture"],
        flags(false, true, true),
        "tapped"
    );
    assert_eq!(
        actions["move_up"],
        flags(false, false, false),
        "unbound, present and false"
    );
    assert_eq!(
        actions["close"],
        flags(false, false, false),
        "an action nothing touched"
    );
    let edge = |action: &str, kind: &str| {
        ck_lua::ConfigValue::Table(
            [("action", action), ("kind", kind)]
                .into_iter()
                .map(|(name, value)| {
                    (
                        name.to_owned(),
                        ck_lua::ConfigValue::String(value.to_owned()),
                    )
                })
                .collect(),
        )
    };
    assert_eq!(
        first["transitions"],
        ck_lua::ConfigValue::Array(vec![
            edge("move_left", "press"),
            edge("reload_texture", "press"),
            edge("reload_texture", "release"),
        ]),
        "the transitions in order"
    );
    assert_eq!(first["overflow"], ck_lua::ConfigValue::Bool(false));
    assert_eq!(first["reset"], ck_lua::ConfigValue::Bool(false));
    // The second update: later, and the edges were acknowledged by the first (the held action stays held).
    let ck_lua::ConfigValue::Table(second) = &seen[1] else {
        panic!("a table: {:?}", seen[1]);
    };
    assert_eq!(second["sequence"], ck_lua::ConfigValue::Integer(2));
    let ck_lua::ConfigValue::Table(later) = &second["actions"] else {
        panic!("actions: {:?}", second["actions"]);
    };
    assert_eq!(later["move_left"], flags(true, false, false));
    assert_eq!(later["reload_texture"], flags(false, false, false));
}

#[test]
fn a_raising_frame_update_ends_the_run_failed_and_is_not_retried() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let count: HostFunction = Arc::new(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(Vec::new())
    });
    let harness = Harness::new(
        &format!("{WINDOW}\nframe.on_update(function(f) window.count() error('update boom') end)"),
        Extras {
            functions: vec![("count", count)],
            extensions: vec![crate::testing::frame_extension()],
            ..Extras::default()
        },
    );
    let (env, control) = updating_env(&harness, "");
    let latch = ResourceLatch::new();
    let quit = Arc::clone(&harness.quit);
    let outcome = run_hooked_in(
        &env,
        &harness,
        BackendScript {
            wait_bound: Duration::from_millis(20),
            ..BackendScript::default()
        },
        PresenterScript::default(),
        &latch,
        DRAIN,
        |backend| {
            backend.on_pump = Some(Box::new(move |pass| {
                if pass == 4 {
                    control.set(true);
                }
                if pass > 4000 {
                    quit.store(true, Ordering::SeqCst);
                }
            }));
        },
    );
    let message = failed(&outcome.end);
    assert!(
        message.starts_with("ui callback frame_update failed: "),
        "{message}"
    );
    assert!(message.contains("update boom"), "{message}");
    assert!(message.contains("stack traceback"), "{message}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a callback with side effects is not retried"
    );
}

/// The final-publication rule end to end (design §11.1, CK-109): a frame callback that moves a quad and then raises still
/// publishes the move (the completion hook runs on failure too), and the run ends failed (status 1, as for any raising
/// callback). Negative control: a hook that skipped failed callbacks would leave one publication, not two.
#[test]
fn a_frame_callback_that_moves_a_quad_then_raises_publishes_the_move_and_ends_the_run_failed() {
    use ck_resources::{Assets, Mode, ResourceLimits, ResourceService, ServiceWakes};

    let assets = crate::testing::Dir::new();
    std::fs::write(
        assets.0.join("a.png"),
        crate::testing::png(8, 4, &[200; 8 * 4 * 4]),
    )
    .expect("an image");
    let service = ResourceService::with_assets(
        ResourceLimits::DEFAULT,
        Mode::Backend,
        ServiceWakes {
            jobs: None,
            uploads: None,
            retirements: None,
        },
        Some(Assets::new(assets.0.clone())),
    )
    .expect("a service");
    let harness = Harness::new(
        &format!(
            "{WINDOW}\nlocal t = texture.load('a.png')\nlocal q\n\
             function startup() q = render.create_quad{{texture = t, x = 1, y = 1, width = 5, height = 5}} end\n\
             frame.on_update(function(f) render.set_position(q, 50, 60) error('update boom') end)"
        ),
        Extras {
            assets: Some(assets.0.clone()),
            service: Some(service),
            extensions: vec![crate::testing::frame_extension()],
            ..Extras::default()
        },
    );
    assert_eq!(
        harness.bridge.counts().published,
        1,
        "startup published the created quad"
    );
    let (env, control) = updating_env(&harness, "");
    let latch = ResourceLatch::new();
    let quit = Arc::clone(&harness.quit);
    let outcome = run_hooked_in(
        &env,
        &harness,
        BackendScript {
            wait_bound: Duration::from_millis(20),
            ..BackendScript::default()
        },
        PresenterScript::default(),
        &latch,
        DRAIN,
        |backend| {
            backend.on_pump = Some(Box::new(move |pass| {
                if pass == 4 {
                    control.set(true);
                }
                if pass > 4000 {
                    quit.store(true, Ordering::SeqCst);
                }
            }));
        },
    );
    let message = failed(&outcome.end);
    assert!(message.contains("update boom"), "{message}");
    assert_eq!(
        harness.bridge.counts().published,
        2,
        "the move was published although the callback raised: {:?}",
        outcome.events
    );
}

#[test]
fn nothing_is_posted_while_the_update_source_is_off() {
    // The update source is configured (the native path's default) and a script has registered `frame_update`, but the
    // control was never turned on: no update is posted and the run is as it was.
    let (record, seen) = recorder_function();
    let harness = Harness::new(
        &format!("{WINDOW}\nframe.on_update(function(f) window.record(f) end)"),
        Extras {
            functions: vec![("record", record)],
            extensions: vec![crate::testing::frame_extension()],
            ..Extras::default()
        },
    );
    let (env, _control) = updating_env(&harness, "");
    let latch = ResourceLatch::new();
    let quit = Arc::clone(&harness.quit);
    let outcome = run_hooked_in(
        &env,
        &harness,
        BackendScript {
            wait_bound: Duration::from_millis(20),
            ..BackendScript::default()
        },
        PresenterScript::default(),
        &latch,
        DRAIN,
        |backend| {
            backend.on_pump = Some(Box::new(move |pass| {
                if pass > 60 {
                    quit.store(true, Ordering::SeqCst);
                }
            }));
        },
    );
    assert_eq!(outcome.end, RunEnd::Quit, "{:?}", outcome.events);
    assert!(
        seen.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty(),
        "the control is off: no update reached the script"
    );
}
