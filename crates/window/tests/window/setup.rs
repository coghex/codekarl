//! Setup: the first request batch as a whole, and what each failure at each step tears down.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use ck_window::{LoopEnd, LoopSettings, NotifierState, PresenterReport, WindowDesc};

use crate::fakes::{DEADLINE, Gate, Rig, at, bits, desc, lock, owned, scenario};

fn touched_the_backend(log: &[String]) -> bool {
    log.iter()
        .any(|line| line.starts_with("backend.") || line.starts_with("presenter."))
}

#[test]
fn no_open_request_gives_no_window_and_touches_no_backend() {
    owned(
        module_path!(),
        "no_open_request_gives_no_window_and_touches_no_backend",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let rig = Rig::empty();
                // A colour and nothing else in the first batch is still "no open".
                rig.writer.set_clear([0.5; 3]).expect("finite");
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::NoWindow);
            assert!(!touched_the_backend(&ran.log), "{:#?}", ran.log);
        },
    );
}

#[test]
fn writes_after_both_early_returns_are_discarded_and_counted() {
    owned(
        module_path!(),
        "writes_after_both_early_returns_are_discarded_and_counted",
        DEADLINE,
        || {
            let _scenario = scenario();
            for with_close in [false, true] {
                let counted = {
                    let rig = Rig::empty();
                    let writer = rig.writer.clone();
                    if with_close {
                        writer.open(desc()).expect("valid");
                        writer.close();
                    }
                    let requests = rig.requests.clone();
                    let ran = rig.run_collect();
                    assert_eq!(
                        ran.end,
                        if with_close {
                            LoopEnd::Closed
                        } else {
                            LoopEnd::NoWindow
                        }
                    );
                    assert!(!touched_the_backend(&ran.log));
                    let before = requests.discarded();
                    writer.set_clear([0.1; 3]).expect("finite");
                    writer.close();
                    writer.close();
                    requests.discarded() - before
                };
                assert_eq!(counted, 3, "with_close: {with_close}");
            }
        },
    );
}

#[test]
fn open_and_close_in_one_batch_close_without_creating_a_window() {
    owned(
        module_path!(),
        "open_and_close_in_one_batch_close_without_creating_a_window",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let rig = Rig::empty();
                rig.writer.open(desc()).expect("valid");
                rig.writer.close();
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert!(!touched_the_backend(&ran.log), "{:#?}", ran.log);
        },
    );
}

#[test]
fn the_three_field_batch_open_colour_and_close_closes_without_creating_a_window() {
    owned(
        module_path!(),
        "the_three_field_batch_open_colour_and_close_closes_without_creating_a_window",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let rig = Rig::empty();
                rig.writer.set_clear([0.9; 3]).expect("finite");
                rig.writer.open(desc()).expect("valid");
                rig.writer.close();
                rig.writer.set_clear([0.8; 3]).expect("finite");
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert!(!touched_the_backend(&ran.log));
        },
    );
}

#[test]
fn open_then_set_clear_in_one_batch_creates_the_window_with_the_later_colour() {
    owned(
        module_path!(),
        "open_then_set_clear_in_one_batch_creates_the_window_with_the_later_colour",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (ran, created, started) = {
                let mut rig = Rig::empty();
                rig.writer.open(desc()).expect("valid");
                rig.writer.set_clear([0.9, 0.8, 0.7]).expect("finite");
                let created = Arc::clone(&rig.backend.created_with);
                let started = Arc::clone(&rig.presenter.started_with);
                let writer = rig.writer.clone();
                rig.backend.hook = Some(Box::new(move |_| writer.close()));
                let ran = rig.run_collect();
                let created = lock(&created).clone();
                let started = lock(&started).clone();
                (ran, created, started)
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            let created = created.expect("the window was created");
            assert_eq!(
                bits(created.clear),
                bits([0.9, 0.8, 0.7]),
                "the later colour wins"
            );
            assert_eq!((created.width, created.height), (400, 300));
            let (handles, desc, pixel_size) = started.expect("the presenter started");
            assert_eq!(handles, 7, "the loop passed the backend's handles on");
            assert_eq!(bits(desc.clear), bits([0.9, 0.8, 0.7]));
            assert_eq!(
                (pixel_size.width, pixel_size.height),
                (800, 600),
                "and the initial pixel size"
            );
        },
    );
}

#[test]
fn invalid_input_followed_by_a_valid_first_open_creates_the_window() {
    owned(
        module_path!(),
        "invalid_input_followed_by_a_valid_first_open_creates_the_window",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::empty();
                assert!(rig.writer.open(WindowDesc { width: 0, ..desc() }).is_err());
                assert!(rig.writer.set_clear([f32::NAN; 3]).is_err());
                rig.writer.open(desc()).expect("valid");
                let writer = rig.writer.clone();
                rig.backend.hook = Some(Box::new(move |_| writer.close()));
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert!(ran.log.iter().any(|line| line == "backend.create_window"));
        },
    );
}

#[test]
fn a_quit_before_setup_creates_nothing() {
    owned(
        module_path!(),
        "a_quit_before_setup_creates_nothing",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let rig = Rig::new();
                rig.quit.store(true, Ordering::SeqCst);
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Quit);
            assert!(!touched_the_backend(&ran.log));
        },
    );
}

#[test]
fn a_failing_create_window_calls_only_terminate_and_returns_failed() {
    owned(
        module_path!(),
        "a_failing_create_window_calls_only_terminate_and_returns_failed",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (ran, discarded) = {
                let mut rig = Rig::new();
                rig.backend.create = Err("no display".to_owned());
                let writer = rig.writer.clone();
                let requests = rig.requests.clone();
                let ran = rig.run_collect();
                writer.close();
                (ran, requests.discarded())
            };
            assert_eq!(
                ran.end,
                LoopEnd::Failed {
                    message: "cannot create the window: no display".to_owned()
                }
            );
            assert_eq!(
                ran.log,
                [
                    "backend.create_window",
                    "backend.terminate notifier=Inactive",
                    "drop backend",
                    "drop presenter"
                ],
                "no wake target, no arm, no presenter, no destroy"
            );
            assert_eq!(discarded, 1, "the slot was ended");
        },
    );
}

#[test]
fn a_presenter_start_error_disarms_destroys_the_window_and_terminates_without_a_presenter_stop() {
    owned(
        module_path!(),
        "a_presenter_start_error_disarms_destroys_the_window_and_terminates_without_a_presenter_stop",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.start = Err("no gpu".to_owned());
                rig.run_collect()
            };
            assert_eq!(
                ran.end,
                LoopEnd::Failed {
                    message: "cannot start the presenter: no gpu".to_owned()
                }
            );
            assert_eq!(
                ran.log,
                [
                    "backend.create_window",
                    "backend.wake_target",
                    "presenter.start",
                    "backend.destroy_window notifier=Disarmed",
                    "backend.terminate notifier=Disarmed",
                    "drop backend",
                    "drop presenter"
                ],
                "no presenter stop or poll: there is no thread"
            );
        },
    );
}

#[test]
fn a_native_wake_in_flight_during_a_failed_presenter_start_makes_the_run_unproven() {
    owned(
        module_path!(),
        "a_native_wake_in_flight_during_a_failed_presenter_start_makes_the_run_unproven",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.start = Err("no gpu".to_owned());
                rig.settings =
                    LoopSettings::new(64, 16, Duration::from_secs(2), Duration::from_millis(50))
                        .expect("non-zero");
                let gate = Arc::new(Gate::default());
                rig.backend.native.gate_wakes(&gate);
                let notifier = Arc::clone(&rig.notifier);
                let producer_gate = Arc::clone(&gate);
                let producer: Arc<std::sync::Mutex<Option<thread::JoinHandle<()>>>> =
                    Arc::new(std::sync::Mutex::new(None));
                let slot = Arc::clone(&producer);
                rig.presenter.on_start = Some(Box::new(move || {
                    // A producer is inside the native wake while the start fails.
                    *lock(&slot) = Some(thread::spawn(move || notifier.wake()));
                    producer_gate.wait_entered(1);
                }));
                let state = Arc::clone(&rig.notifier);
                let ran = rig.run_collect();
                assert_eq!(
                    state.state(),
                    NotifierState::Disarming,
                    "the disarm timed out"
                );
                gate.release();
                let handle = lock(&producer).take().expect("the producer started");
                handle.join().expect("the producer finished");
                ran
            };
            assert!(matches!(ran.end, LoopEnd::Unproven { .. }), "{:?}", ran.end);
            assert!(
                !ran.log
                    .iter()
                    .any(|line| line.starts_with("backend.destroy_window")
                        || line.starts_with("backend.terminate")
                        || line.starts_with("drop ")),
                "nothing was destroyed or dropped: {:#?}",
                ran.log
            );
        },
    );
}

#[test]
fn a_started_presenter_that_reports_failed_is_torn_down_normally() {
    owned(
        module_path!(),
        "a_started_presenter_that_reports_failed_is_torn_down_normally",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let rig = Rig::new();
                lock(&rig.presenter.reports).push_back(PresenterReport::Failed {
                    message: "gpu lost".to_owned(),
                });
                rig.run_collect()
            };
            assert_eq!(
                ran.end,
                LoopEnd::PresenterFailed {
                    message: "the presenter failed: gpu lost".to_owned()
                }
            );
            let stop = at(&ran.log, "presenter.request_stop");
            assert!(stop < at(&ran.log, "presenter.poll_end"));
            assert!(
                at(&ran.log, "presenter.poll_end")
                    < at(&ran.log, "backend.destroy_window notifier=Disarmed")
            );
            assert!(at(&ran.log, "backend.destroy_window") < at(&ran.log, "backend.terminate"));
        },
    );
}
