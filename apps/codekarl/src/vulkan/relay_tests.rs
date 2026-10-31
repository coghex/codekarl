//! Tests of the `glfw-vulkan` path's resource seam that need no window or GPU: the side's slot and driver before anything
//! starts, the relay's one `connect` and its `disconnect` on every end path, the `Err` of either, and the presenter's last
//! numbers copied to the run's notes when its end is observed. Over the fake stack, so they run the production backend and
//! adapter with the fake side's recorded calls.

use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ck_render_2d::Gpu2d;
use ck_window::{Backend, CleanupResult};

use super::fake::{FakeStack, Script};
use super::logging::scenario;
use super::tests::{at, desc, events, parts, run_over, start_on};
use super::{Run, VulkanAdapter, VulkanBackend};
use crate::fakes::{End, PresenterScript};
use crate::windowed::VulkanFigures;

/// How often `event` is in `log`.
fn count(log: &[String], event: &str) -> usize {
    log.iter().filter(|seen| *seen == event).count()
}

/// The figures a scripted presenter reports.
fn figures() -> VulkanFigures {
    VulkanFigures {
        textures_created: 3,
        texture_bytes_peak: 64,
        staging_bytes_peak: 32,
        checker_bytes: 16384,
        frames: 9,
    }
}

/// A started run: its state, the backend, the adapter and the ordered log.
type Started = (
    Rc<Run<FakeStack>>,
    VulkanBackend<FakeStack>,
    VulkanAdapter<FakeStack>,
    Arc<Mutex<Vec<String>>>,
);

/// A run whose presenter started on a fake window.
fn started(script: Script) -> Started {
    let (run, log) = run_over(script);
    let (backend, mut adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    start_on(&mut adapter, &created).expect("started");
    (run, backend, adapter, log)
}

#[test]
fn a_failing_start_disconnects_the_relay_once_connects_nothing_and_starts_no_presenter() {
    let (run, log) = run_over(Script {
        presenter: PresenterScript {
            start_error: Some("no device".to_owned()),
            ..PresenterScript::default()
        },
        ..Script::default()
    });
    let (backend, mut adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    let Err(error) = start_on(&mut adapter, &created) else {
        panic!("the start must fail")
    };
    assert!(error.contains("no device"), "{error}");
    let log = events(&log);
    assert_eq!(count(&log, "relay disconnect"), 1, "{log:?}");
    assert_eq!(count(&log, "relay connect"), 0, "{log:?}");
    assert!(run.state.borrow().presenter.is_none(), "no presenter");
    // Negative control: a start that succeeds disconnects nothing at that point (the relay is connected instead).
    let (_run, _backend, _adapter, ok_log) = started(Script::default());
    let ok_log = events(&ok_log);
    assert_eq!(count(&ok_log, "relay disconnect"), 0, "{ok_log:?}");
    assert_eq!(count(&ok_log, "relay connect"), 1, "{ok_log:?}");
}

#[test]
fn a_side_that_cannot_give_its_slot_or_its_driver_is_a_start_failure_before_anything_starts() {
    for (script, reason) in [
        (
            Script {
                accept_error: Some("no frame slot".to_owned()),
                ..Script::default()
            },
            "no frame slot",
        ),
        (
            Script {
                driver_error: Some("no driver".to_owned()),
                ..Script::default()
            },
            "no driver",
        ),
    ] {
        let (run, log) = run_over(script);
        let (backend, mut adapter) = parts(&run);
        let mut backend = backend.expect("opened");
        let created = backend.create_window(&desc()).expect("a window");
        let Err(error) = start_on(&mut adapter, &created) else {
            panic!("{reason}: the start must fail")
        };
        assert_eq!(error, reason, "today's text is the side's reason");
        let log = events(&log);
        assert!(
            !log.contains(&"presenter start with the side's slot".to_owned()),
            "nothing was started: {log:?}"
        );
        assert_eq!(count(&log, "relay disconnect"), 1, "{reason}: {log:?}");
        assert_eq!(count(&log, "relay connect"), 0, "{reason}: {log:?}");
        assert!(run.state.borrow().presenter.is_none());
    }
}

#[test]
fn a_successful_start_connects_the_relay_exactly_once_with_the_started_presenters_texture_wake() {
    let (run, _backend, _adapter, log) = started(Script::default());
    let log = events(&log);
    assert_eq!(count(&log, "relay connect"), 1, "{log:?}");
    assert!(
        at(&log, "presenter start with the side's slot") < at(&log, "relay connect"),
        "connected after the start: {log:?}"
    );
    let connected = run
        .stack
        .relay
        .connected
        .lock()
        .expect("the relay log")
        .clone();
    assert_eq!(connected.len(), 1);
    assert!(
        Arc::ptr_eq(&connected[0], &run.stack.texture_wake),
        "the wake the relay was connected to is the started presenter's"
    );
}

#[test]
fn a_proven_end_disconnects_after_the_handle_is_released_and_before_the_destroy() {
    let (run, mut backend, mut adapter, log) = started(Script::default());
    adapter.request_stop();
    assert_eq!(
        adapter.poll_end(Duration::from_millis(10)),
        Some(CleanupResult::Proven)
    );
    backend.destroy_window();
    backend.terminate();
    let log = events(&log);
    assert_eq!(count(&log, "relay disconnect"), 1, "once: {log:?}");
    assert!(
        at(&log, "presenter dropped") < at(&log, "relay disconnect"),
        "after the handle is released: {log:?}"
    );
    assert!(
        at(&log, "relay disconnect") < at(&log, "destroy helper surface=7 ended=Proven"),
        "before the destroy: {log:?}"
    );
    assert_eq!(run.stack.relay.disconnects.load(Ordering::SeqCst), 1);
}

#[test]
fn an_unproven_end_disconnects_before_anything_is_retained() {
    let (_run, backend, mut adapter, log) = started(Script {
        presenter: PresenterScript {
            end: End::Unproven("no cleanup result".to_owned()),
            ..PresenterScript::default()
        },
        ..Script::default()
    });
    adapter.request_stop();
    assert!(matches!(
        adapter.poll_end(Duration::from_millis(10)),
        Some(CleanupResult::Unproven { .. })
    ));
    std::mem::forget(backend);
    std::mem::forget(adapter);
    let log = events(&log);
    assert_eq!(count(&log, "relay disconnect"), 1, "{log:?}");
    assert!(
        !log.contains(&"instance dropped".to_owned()),
        "nothing was released: {log:?}"
    );
}

#[test]
fn a_refused_destroy_disconnects_the_relay_once_before_the_owners_are_retained() {
    let (run, log) = super::tests::refused_teardown_for(false);
    let log = events(&log);
    assert_eq!(count(&log, "relay disconnect"), 1, "once: {log:?}");
    assert!(
        at(&log, "relay disconnect") < at(&log, "destroy helper surface=7 ended=Proven"),
        "the relay went before the helper was asked: {log:?}"
    );
    assert!(run.retained().is_some(), "and the owners were retained");
}

#[test]
fn a_presenter_not_proven_at_the_backends_destroy_disconnects_before_it_retains() {
    let (run, mut backend, _adapter, log) = started(Script::default());
    // The loop never does this (it destroys only after a proven end); the backend's own guard is what is under test.
    backend.destroy_window();
    let log = events(&log);
    assert_eq!(count(&log, "relay disconnect"), 1, "{log:?}");
    assert!(run.retained().is_some());
    std::mem::forget(backend);
}

#[test]
fn an_err_from_connect_or_disconnect_is_one_error_record_each_and_the_end_path_carries_on() {
    let scenario = scenario();
    let (run, mut backend, mut adapter, log) = started(Script {
        connect_error: Some("connect broke".to_owned()),
        disconnect_error: Some("disconnect broke".to_owned()),
        ..Script::default()
    });
    adapter.request_stop();
    assert_eq!(
        adapter.poll_end(Duration::from_millis(10)),
        Some(CleanupResult::Proven),
        "the end path runs to its usual outcome"
    );
    backend.destroy_window();
    backend.terminate();
    let records = scenario.records();
    for reason in ["connect broke", "disconnect broke"] {
        let matching: Vec<&String> = records
            .iter()
            .filter(|record| {
                record.contains("ERROR") && record.contains(&format!("render: {reason}"))
            })
            .collect();
        assert_eq!(matching.len(), 1, "{reason}: {records:?}");
    }
    let log = events(&log);
    assert!(
        log.contains(&"instance destroyed".to_owned()),
        "the destroy still ran: {log:?}"
    );
    assert_eq!(count(&log, "relay connect"), 1, "never retried: {log:?}");
    assert_eq!(count(&log, "relay disconnect"), 1, "never retried: {log:?}");
    drop(run);
}

#[test]
fn the_presenters_last_numbers_are_copied_to_the_notes_at_the_observed_end_before_the_handle_goes()
{
    let (run, mut backend, mut adapter, _log) = started(Script {
        figures: Some(figures()),
        ..Script::default()
    });
    assert!(
        run.notes.vulkan_gpu.borrow().is_none(),
        "nothing before the end"
    );
    adapter.request_stop();
    assert_eq!(
        adapter.poll_end(Duration::from_millis(10)),
        Some(CleanupResult::Proven)
    );
    assert_eq!(
        *run.notes.vulkan_gpu.borrow(),
        Some(figures()),
        "copied at the observed end, while the handle was alive"
    );
    backend.destroy_window();
    backend.terminate();
    // Negative control: a presenter with no figures leaves the notes empty (nothing stale or invented).
    let (run, _backend, mut adapter, _log) = started(Script::default());
    adapter.request_stop();
    let _ = adapter.poll_end(Duration::from_millis(10));
    assert!(run.notes.vulkan_gpu.borrow().is_none());
}

#[test]
fn the_backend_forwards_deadline_waits_to_glfw_so_a_paced_update_source_runs() {
    // A bug the first real window found: the wrapper kept the trait's defaults (no deadline support, and a `pump_until` that
    // stops), so a glfw-vulkan run with `[frame]` stopped at its first update wait.
    let (run, _log) = run_over(Script::default());
    let (backend, _adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    assert!(
        backend.supports_deadline(),
        "the fake GLFW can wait to a deadline"
    );
    let mut seen = 0;
    backend.pump_until(
        &mut |_| seen += 1,
        std::time::Instant::now() + Duration::from_millis(5),
        64,
    );
    assert_eq!(seen, 0, "the call went to the backend and returned");
}
