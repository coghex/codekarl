//! Teardown: its order, and what an unproven presenter or a disarm timeout leave alone.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use ck_window::{Answer, CleanupResult, LoopEnd, LoopSettings, NativeEvent, NotifierState};

use crate::fakes::{DEADLINE, End, Gate, Rig, at, lock, owned, scenario};

fn nothing_destroyed_or_dropped(log: &[String]) {
    assert!(
        !log.iter()
            .any(|line| line.starts_with("backend.destroy_window")
                || line.starts_with("backend.terminate")
                || line.starts_with("drop ")),
        "nothing may be destroyed or dropped: {log:#?}"
    );
}

fn short(teardown_ms: u64, disarm_ms: u64) -> LoopSettings {
    LoopSettings::new(
        64,
        16,
        Duration::from_millis(teardown_ms),
        Duration::from_millis(disarm_ms),
    )
    .expect("non-zero")
}

#[test]
fn teardown_runs_in_order_and_only_then_are_the_objects_dropped() {
    owned(
        module_path!(),
        "teardown_runs_in_order_and_only_then_are_the_objects_dropped",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                lock(&rig.ui.close_answers).push_back(Answer::Accept);
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            let order = [
                at(&ran.log, "presenter.request_stop"),
                at(&ran.log, "presenter.poll_end"),
                at(&ran.log, "backend.destroy_window notifier=Disarmed"),
                at(&ran.log, "backend.terminate notifier=Disarmed"),
                at(&ran.log, "drop backend"),
            ];
            assert!(
                order.windows(2).all(|pair| pair[0] < pair[1]),
                "{:#?}",
                ran.log
            );
            assert!(at(&ran.log, "drop presenter") > at(&ran.log, "backend.terminate"));
        },
    );
}

#[test]
fn an_unproven_cleanup_destroys_nothing_drops_nothing_and_returns_unproven() {
    owned(
        module_path!(),
        "an_unproven_cleanup_destroys_nothing_drops_nothing_and_returns_unproven",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (ran, state) = {
                let mut rig = Rig::new();
                rig.presenter.end = End::At(CleanupResult::Unproven {
                    reason: "the gpu wait timed out".to_owned(),
                });
                rig.close_at_pump(1);
                let notifier = Arc::clone(&rig.notifier);
                let ran = rig.run_collect();
                (ran, notifier.state())
            };
            match &ran.end {
                LoopEnd::Unproven { reason } => {
                    assert!(reason.contains("the gpu wait timed out"), "{reason}");
                }
                other => panic!("expected Unproven, got {other:?}"),
            }
            nothing_destroyed_or_dropped(&ran.log);
            assert_eq!(
                state,
                NotifierState::Armed,
                "the notifier was not disarmed either"
            );
        },
    );
}

#[test]
fn a_presenter_that_never_ends_is_unproven_after_the_teardown_deadline() {
    owned(
        module_path!(),
        "a_presenter_that_never_ends_is_unproven_after_the_teardown_deadline",
        DEADLINE,
        || {
            let _scenario = scenario();
            let begun = Instant::now();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.end = End::Never;
                rig.settings = short(80, 50);
                rig.close_at_pump(1);
                rig.run_collect()
            };
            let took = begun.elapsed();
            assert!(matches!(ran.end, LoopEnd::Unproven { .. }), "{:?}", ran.end);
            assert!(
                took >= Duration::from_millis(80) && took < Duration::from_secs(5),
                "{took:?}"
            );
            nothing_destroyed_or_dropped(&ran.log);
            assert!(
                ran.log
                    .iter()
                    .filter(|line| *line == "presenter.poll_end")
                    .count()
                    >= 2,
                "polled in slices: {:#?}",
                ran.log
            );
        },
    );
}

#[test]
fn a_disarm_timeout_destroys_nothing_and_returns_unproven() {
    owned(
        module_path!(),
        "a_disarm_timeout_destroys_nothing_and_returns_unproven",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (ran, state) = {
                let mut rig = Rig::new();
                rig.settings = short(500, 50);
                let gate = Arc::new(Gate::default());
                let native = Arc::clone(&rig.backend.native);
                let notifier = Arc::clone(&rig.notifier);
                let producer: Arc<std::sync::Mutex<Option<thread::JoinHandle<()>>>> =
                    Arc::new(std::sync::Mutex::new(None));
                let (slot, wake_gate) = (Arc::clone(&producer), Arc::clone(&gate));
                let in_hook = Arc::clone(&notifier);
                let writer = rig.writer.clone();
                rig.backend.hook = Some(Box::new(move |call| {
                    if call.index == 1 {
                        // The loop closes (its own write wakes through the open native target), and only then
                        // does a producer go into the native wake and stay there.
                        writer.close();
                        native.gate_wakes(&wake_gate);
                        let notifier = Arc::clone(&in_hook);
                        *lock(&slot) = Some(thread::spawn(move || notifier.wake()));
                        wake_gate.wait_entered(1);
                    }
                }));
                let ran = rig.run_collect();
                let state = notifier.state();
                gate.release();
                lock(&producer)
                    .take()
                    .expect("the producer started")
                    .join()
                    .expect("finished");
                (ran, state)
            };
            match &ran.end {
                LoopEnd::Unproven { reason } => assert!(reason.contains("native wake"), "{reason}"),
                other => panic!("expected Unproven, got {other:?}"),
            }
            assert_eq!(state, NotifierState::Disarming);
            nothing_destroyed_or_dropped(&ran.log);
            assert!(
                ran.log.iter().any(|line| line == "presenter.request_stop"),
                "the presenter was stopped and proven first"
            );
        },
    );
}

#[test]
fn an_unproven_teardown_overrides_the_failure_that_started_it_but_keeps_its_message() {
    owned(
        module_path!(),
        "an_unproven_teardown_overrides_the_failure_that_started_it_but_keeps_its_message",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.end = End::At(CleanupResult::Unproven {
                    reason: "no cleanup result".to_owned(),
                });
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                lock(&rig.ui.close_answers).push_back(Answer::Failed {
                    message: "on_close_request failed: boom".to_owned(),
                });
                rig.run_collect()
            };
            match &ran.end {
                LoopEnd::Unproven { reason } => {
                    assert!(reason.contains("no cleanup result"), "{reason}");
                    assert!(reason.contains("on_close_request failed: boom"), "{reason}");
                }
                other => panic!("expected Unproven, got {other:?}"),
            }
            nothing_destroyed_or_dropped(&ran.log);
        },
    );
}

#[test]
fn a_panic_inside_the_loop_releases_nothing() {
    owned(
        module_path!(),
        "a_panic_inside_the_loop_releases_nothing",
        DEADLINE,
        || {
            let _scenario = scenario();
            let log = {
                let mut rig = Rig::new();
                rig.presenter.panic_on_take = Some(1);
                lock(&rig.presenter.reports)
                    .push_back(ck_window::PresenterReport::FirstFrameQueued);
                let log = Arc::clone(&rig.log);
                let caught =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.run_collect()));
                assert!(caught.is_err(), "the injected panic unwinds through run");
                crate::fakes::calls(&log)
            };
            nothing_destroyed_or_dropped(&log);
            assert!(
                !log.iter().any(|line| line == "presenter.request_stop"),
                "no teardown ran during the unwind: {log:#?}"
            );
        },
    );
}

#[test]
fn closing_ends_the_slot_so_later_writes_are_counted() {
    owned(
        module_path!(),
        "closing_ends_the_slot_so_later_writes_are_counted",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (ran, discarded) = {
                let mut rig = Rig::new();
                rig.close_at_pump(1);
                let writer = rig.writer.clone();
                let requests = rig.requests.clone();
                let ran = rig.run_collect();
                writer.set_clear([0.5; 3]).expect("finite");
                writer.close();
                (ran, requests.discarded())
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert_eq!(
                discarded, 2,
                "both writes after the loop ended were discarded and counted"
            );
        },
    );
}
