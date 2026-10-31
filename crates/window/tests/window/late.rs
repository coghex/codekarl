//! Presenter failures that arrive late: queued before a close or quit, published while the presenter
//! stops, or published in the very `poll_end` call that first observes it finished. A presenter may
//! report `Failed` and still prove its resources released; teardown takes every report once the presenter
//! is observed finished (or, at a missed deadline, whatever is already there), before anything is
//! destroyed, and a failure it finds is never lost.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ck_window::{Answer, CleanupResult, LoopEnd, LoopSettings, NativeEvent, PresenterReport};

use crate::fakes::{DEADLINE, End, Ran, Rig, at, calls, lock, owned, scenario};

fn failed(message: &str) -> PresenterReport {
    PresenterReport::Failed {
        message: message.to_owned(),
    }
}

fn presenter_failed(message: &str) -> LoopEnd {
    LoopEnd::PresenterFailed {
        message: format!("the presenter failed: {message}"),
    }
}

/// The records of the scenario that mention a presenter failure.
fn failure_records(records: &[String]) -> Vec<&String> {
    records
        .iter()
        .filter(|line| line.contains("the presenter failed:"))
        .collect()
}

/// The window was torn down in the proven order, with the reports taken before the notifier was disarmed.
fn assert_torn_down(ran: &Ran) {
    let poll = at(&ran.log, "presenter.poll_end");
    let last_report = ran
        .log
        .iter()
        .rposition(|line| line == "presenter.report")
        .expect("a report was taken");
    assert!(
        poll < last_report && last_report < at(&ran.log, "backend.destroy_window"),
        "the last report is taken after the end is observed and before anything is destroyed: {:#?}",
        ran.log
    );
    assert!(poll < at(&ran.log, "backend.destroy_window notifier=Disarmed"));
    assert!(at(&ran.log, "backend.destroy_window") < at(&ran.log, "backend.terminate"));
    assert!(at(&ran.log, "drop presenter") > at(&ran.log, "backend.terminate"));
}

fn nothing_destroyed_or_dropped(log: &[String]) {
    assert!(
        !log.iter()
            .any(|line| line.starts_with("backend.destroy_window")
                || line.starts_with("backend.terminate")
                || line.starts_with("drop ")),
        "nothing may be destroyed or dropped: {log:#?}"
    );
}

fn short(items: usize, teardown_ms: u64) -> LoopSettings {
    LoopSettings::new(
        64,
        items,
        Duration::from_millis(teardown_ms),
        Duration::from_millis(50),
    )
    .expect("non-zero")
}

// ---------------------------------------------------------------------------------------------
// Queued before the close or quit
// ---------------------------------------------------------------------------------------------

#[test]
fn a_failure_queued_before_a_slot_close_is_not_bypassed() {
    owned(
        module_path!(),
        "a_failure_queued_before_a_slot_close_is_not_bypassed",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                lock(&rig.presenter.reports).push_back(failed("queued before the close"));
                // The close is read from the slot before the loop takes any report.
                rig.close_at_pump(1);
                rig.run_collect()
            };
            assert_eq!(ran.end, presenter_failed("queued before the close"));
            assert_torn_down(&ran);
            assert!(
                failure_records(&scenario.records()).is_empty(),
                "the failure is the end, not also a record: {:?}",
                scenario.records()
            );
        },
    );
}

#[test]
fn a_failure_queued_before_a_quit_is_not_bypassed() {
    owned(
        module_path!(),
        "a_failure_queued_before_a_quit_is_not_bypassed",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                lock(&rig.presenter.reports).push_back(failed("queued before the quit"));
                rig.quit_at_pump(1);
                rig.run_collect()
            };
            assert_eq!(ran.end, presenter_failed("queued before the quit"));
            assert_torn_down(&ran);
        },
    );
}

#[test]
fn a_failure_queued_behind_an_accepted_close_answer_is_not_bypassed() {
    owned(
        module_path!(),
        "a_failure_queued_behind_an_accepted_close_answer_is_not_bypassed",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                // 16 reports are taken per pass. The close request is posted at the end of pass 1 and its
                // accepted answer is taken in pass 2, after that pass took its 16 reports: the failure,
                // the 33rd, is still queued when the loop ends.
                {
                    let mut reports = lock(&rig.presenter.reports);
                    reports.extend((0..32).map(|_| PresenterReport::FirstFrameQueued));
                    reports.push_back(failed("behind the cap"));
                }
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                lock(&rig.ui.close_answers).push_back(Answer::Accept);
                rig.run_collect()
            };
            assert_eq!(ran.end, presenter_failed("behind the cap"));
            assert_torn_down(&ran);
        },
    );
}

// ---------------------------------------------------------------------------------------------
// Arriving during teardown
// ---------------------------------------------------------------------------------------------

#[test]
fn a_failure_published_while_the_presenter_stops_ends_a_proven_close_failed() {
    owned(
        module_path!(),
        "a_failure_published_while_the_presenter_stops_ends_a_proven_close_failed",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.at_stop = vec![failed("at request_stop")];
                rig.close_at_pump(1);
                rig.run_collect()
            };
            assert_eq!(ran.end, presenter_failed("at request_stop"));
            assert_torn_down(&ran);
            assert!(failure_records(&scenario.records()).is_empty());
        },
    );
}

#[test]
fn a_failure_published_while_the_presenter_stops_ends_a_quit_failed() {
    owned(
        module_path!(),
        "a_failure_published_while_the_presenter_stops_ends_a_quit_failed",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.at_stop = vec![failed("at request_stop")];
                rig.quit_at_pump(1);
                rig.run_collect()
            };
            assert_eq!(ran.end, presenter_failed("at request_stop"));
            assert_torn_down(&ran);
        },
    );
}

#[test]
fn a_failure_published_in_the_poll_that_first_observes_the_end_is_seen() {
    owned(
        module_path!(),
        "a_failure_published_in_the_poll_that_first_observes_the_end_is_seen",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.at_end = vec![failed("inside the final poll")];
                rig.close_at_pump(1);
                rig.run_collect()
            };
            assert_eq!(ran.end, presenter_failed("inside the final poll"));
            assert_torn_down(&ran);
        },
    );
}

#[test]
fn the_reports_are_taken_after_the_end_is_observed_and_before_anything_is_destroyed() {
    owned(
        module_path!(),
        "the_reports_are_taken_after_the_end_is_observed_and_before_anything_is_destroyed",
        DEADLINE,
        || {
            let _scenario = scenario();
            let (ran, taken) = {
                let mut rig = Rig::new();
                rig.presenter.at_end = vec![failed("late")];
                rig.close_at_pump(1);
                let taken = Arc::clone(&rig.presenter.taken);
                let ran = rig.run_collect();
                (ran, taken)
            };
            assert_eq!(ran.end, presenter_failed("late"));
            assert_eq!(
                taken.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "exactly the one report was handed out"
            );
        },
    );
}

#[test]
fn reports_that_are_not_failures_change_nothing_at_teardown() {
    owned(
        module_path!(),
        "reports_that_are_not_failures_change_nothing_at_teardown",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.at_stop = vec![PresenterReport::FirstFrameQueued];
                rig.close_at_pump(1);
                rig.run_collect()
            };
            assert_eq!(ran.end, LoopEnd::Closed);
            assert!(
                scenario.records().is_empty(),
                "a first frame reported after the loop ended opens nothing: {:?}",
                scenario.records()
            );
        },
    );
}

// ---------------------------------------------------------------------------------------------
// Once each
// ---------------------------------------------------------------------------------------------

#[test]
fn each_late_failure_is_reported_once_the_first_as_the_end_and_the_rest_as_records() {
    owned(
        module_path!(),
        "each_late_failure_is_reported_once_the_first_as_the_end_and_the_rest_as_records",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.at_stop = vec![failed("first late"), failed("second late")];
                rig.presenter.at_end = vec![failed("third late")];
                rig.close_at_pump(1);
                rig.run_collect()
            };
            assert_eq!(ran.end, presenter_failed("first late"));
            assert_eq!(
                scenario.records(),
                [
                    "ERROR window: the presenter failed: second late",
                    "ERROR window: the presenter failed: third late"
                ]
            );
        },
    );
}

#[test]
fn a_late_failure_next_to_an_earlier_presenter_failure_is_one_record_and_the_end_stays() {
    owned(
        module_path!(),
        "a_late_failure_next_to_an_earlier_presenter_failure_is_one_record_and_the_end_stays",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                // The first pass takes this one and ends the loop with it.
                lock(&rig.presenter.reports).push_back(failed("primary"));
                rig.presenter.at_stop = vec![failed("late")];
                rig.run_collect()
            };
            assert_eq!(ran.end, presenter_failed("primary"));
            assert_torn_down(&ran);
            assert_eq!(
                scenario.records(),
                ["ERROR window: the presenter failed: late"]
            );
        },
    );
}

#[test]
fn a_late_failure_next_to_a_callback_failure_is_one_record_and_the_end_stays() {
    owned(
        module_path!(),
        "a_late_failure_next_to_a_callback_failure_is_one_record_and_the_end_stays",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.at_stop = vec![failed("late")];
                rig.backend.script.push_back(vec![NativeEvent::CloseButton]);
                lock(&rig.ui.close_answers).push_back(Answer::Failed {
                    message: "on_close_request failed: boom".to_owned(),
                });
                rig.run_collect()
            };
            assert_eq!(
                ran.end,
                LoopEnd::CallbackFailed {
                    message: "on_close_request failed: boom".to_owned()
                }
            );
            assert_eq!(
                scenario.records(),
                ["ERROR window: the presenter failed: late"]
            );
        },
    );
}

#[test]
fn a_late_failure_next_to_a_dead_ui_vm_is_one_record_and_the_end_stays() {
    owned(
        module_path!(),
        "a_late_failure_next_to_a_dead_ui_vm_is_one_record_and_the_end_stays",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.at_stop = vec![failed("late")];
                rig.ui
                    .disconnected
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                rig.run_collect()
            };
            assert_eq!(
                ran.end,
                LoopEnd::Failed {
                    message: "the ui VM ended".to_owned()
                }
            );
            assert_eq!(
                scenario.records(),
                ["ERROR window: the presenter failed: late"]
            );
        },
    );
}

// ---------------------------------------------------------------------------------------------
// Unproven and a missed deadline: status 3 wins, nothing is released, the failure is still not lost
// ---------------------------------------------------------------------------------------------

#[test]
fn an_unproven_cleanup_with_a_late_failure_stays_unproven_and_records_the_failure_once() {
    owned(
        module_path!(),
        "an_unproven_cleanup_with_a_late_failure_stays_unproven_and_records_the_failure_once",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.end = End::At(CleanupResult::Unproven {
                    reason: "the gpu wait timed out".to_owned(),
                });
                rig.presenter.at_stop = vec![failed("queued by the stop")];
                rig.presenter.at_end = vec![failed("inside the final poll")];
                rig.close_at_pump(1);
                rig.run_collect()
            };
            match &ran.end {
                LoopEnd::Unproven { reason } => {
                    assert!(reason.contains("the gpu wait timed out"), "{reason}");
                }
                other => panic!("expected Unproven, got {other:?}"),
            }
            nothing_destroyed_or_dropped(&ran.log);
            assert_eq!(
                scenario.records(),
                [
                    "ERROR window: the presenter failed: queued by the stop",
                    "ERROR window: the presenter failed: inside the final poll"
                ]
            );
        },
    );
}

#[test]
fn a_missed_deadline_takes_what_is_there_without_waiting_and_stays_unproven() {
    owned(
        module_path!(),
        "a_missed_deadline_takes_what_is_there_without_waiting_and_stays_unproven",
        DEADLINE,
        || {
            let scenario = scenario();
            let begun = Instant::now();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.end = End::Never;
                rig.settings = short(16, 80);
                rig.presenter.at_stop = vec![failed("before the deadline")];
                rig.close_at_pump(1);
                rig.run_collect()
            };
            let took = begun.elapsed();
            assert!(matches!(ran.end, LoopEnd::Unproven { .. }), "{:?}", ran.end);
            assert!(took < Duration::from_secs(5), "{took:?}");
            nothing_destroyed_or_dropped(&ran.log);
            assert_eq!(
                scenario.records(),
                ["ERROR window: the presenter failed: before the deadline"]
            );
        },
    );
}

#[test]
fn an_unproven_teardown_after_an_earlier_failure_records_each_distinct_failure_once() {
    owned(
        module_path!(),
        "an_unproven_teardown_after_an_earlier_failure_records_each_distinct_failure_once",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.end = End::At(CleanupResult::Unproven {
                    reason: "no cleanup result".to_owned(),
                });
                lock(&rig.presenter.reports).push_back(failed("primary"));
                rig.presenter.at_stop = vec![failed("late")];
                rig.run_collect()
            };
            match &ran.end {
                LoopEnd::Unproven { reason } => {
                    assert!(reason.contains("no cleanup result"), "{reason}");
                    assert!(reason.contains("the presenter failed: primary"), "{reason}");
                }
                other => panic!("expected Unproven, got {other:?}"),
            }
            assert_eq!(
                scenario.records(),
                ["ERROR window: the presenter failed: late"]
            );
        },
    );
}

// ---------------------------------------------------------------------------------------------
// Bounded work, and a panic in the new take
// ---------------------------------------------------------------------------------------------

#[test]
fn a_failure_behind_a_first_frame_is_found_at_the_smallest_cap_in_the_finishing_poll() {
    owned(
        module_path!(),
        "a_failure_behind_a_first_frame_is_found_at_the_smallest_cap_in_the_finishing_poll",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.settings = short(1, 500);
                rig.presenter.at_end = vec![PresenterReport::FirstFrameQueued, failed("hidden")];
                rig.close_at_pump(1);
                rig.run_collect()
            };
            assert_eq!(ran.end, presenter_failed("hidden"));
            assert_torn_down(&ran);
            assert!(scenario.records().is_empty(), "{:?}", scenario.records());
        },
    );
}

#[test]
fn a_failure_behind_a_first_frame_queued_before_a_quit_is_found_at_the_smallest_cap() {
    owned(
        module_path!(),
        "a_failure_behind_a_first_frame_queued_before_a_quit_is_found_at_the_smallest_cap",
        DEADLINE,
        || {
            let _scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.settings = short(1, 500);
                {
                    let mut reports = lock(&rig.presenter.reports);
                    reports.push_back(PresenterReport::FirstFrameQueued);
                    reports.push_back(failed("hidden"));
                }
                rig.quit_at_pump(1);
                rig.run_collect()
            };
            assert_eq!(ran.end, presenter_failed("hidden"));
            assert_torn_down(&ran);
        },
    );
}

#[test]
fn distinct_failures_beyond_one_pass_are_each_reported_once() {
    owned(
        module_path!(),
        "distinct_failures_beyond_one_pass_are_each_reported_once",
        DEADLINE,
        || {
            let scenario = scenario();
            let (ran, taken) = {
                let mut rig = Rig::new();
                rig.settings = short(2, 500);
                rig.presenter.at_stop = vec![failed("a"), failed("b"), failed("c")];
                rig.close_at_pump(1);
                let taken = Arc::clone(&rig.presenter.taken);
                let ran = rig.run_collect();
                (ran, taken)
            };
            assert_eq!(ran.end, presenter_failed("a"));
            assert_eq!(taken.load(std::sync::atomic::Ordering::SeqCst), 3);
            assert_eq!(
                scenario.records(),
                [
                    "ERROR window: the presenter failed: b",
                    "ERROR window: the presenter failed: c"
                ]
            );
        },
    );
}

#[test]
fn a_cap_of_two_still_finds_two_failures_in_the_finishing_poll() {
    owned(
        module_path!(),
        "a_cap_of_two_still_finds_two_failures_in_the_finishing_poll",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.settings = short(2, 500);
                rig.presenter.at_end = vec![PresenterReport::FirstFrameQueued, failed("hidden")];
                rig.close_at_pump(1);
                rig.run_collect()
            };
            assert_eq!(ran.end, presenter_failed("hidden"));
            assert!(scenario.records().is_empty(), "{:?}", scenario.records());
        },
    );
}

#[test]
fn a_presenter_that_never_stops_reporting_is_a_reported_failure_after_a_fixed_bound() {
    owned(
        module_path!(),
        "a_presenter_that_never_stops_reporting_is_a_reported_failure_after_a_fixed_bound",
        DEADLINE,
        || {
            let scenario = scenario();
            let begun = Instant::now();
            let (ran, taken) = {
                let mut rig = Rig::new();
                rig.presenter.endless_after_stop = true;
                rig.close_at_pump(1);
                let taken = Arc::clone(&rig.presenter.taken);
                let ran = rig.run_collect();
                (ran, taken)
            };
            assert!(begun.elapsed() < Duration::from_secs(5));
            match &ran.end {
                LoopEnd::PresenterFailed { message } => {
                    assert!(
                        message.contains("did not end within 4096 reports"),
                        "{message}"
                    );
                }
                other => panic!("expected PresenterFailed, got {other:?}"),
            }
            assert_eq!(taken.load(std::sync::atomic::Ordering::SeqCst), 4096);
            assert_torn_down(&ran);
            assert!(scenario.records().is_empty(), "{:?}", scenario.records());
        },
    );
}

#[test]
fn a_failure_found_before_the_bound_is_kept_next_to_the_bound_failure() {
    owned(
        module_path!(),
        "a_failure_found_before_the_bound_is_kept_next_to_the_bound_failure",
        DEADLINE,
        || {
            let scenario = scenario();
            let ran = {
                let mut rig = Rig::new();
                rig.presenter.endless_after_stop = true;
                rig.presenter.at_stop = vec![failed("real")];
                rig.close_at_pump(1);
                rig.run_collect()
            };
            assert_eq!(ran.end, presenter_failed("real"));
            let records = scenario.records();
            assert_eq!(records.len(), 1, "{records:?}");
            assert!(
                records[0].starts_with(
                    "ERROR window: the presenter failed: its report queue did not end"
                ),
                "{records:?}"
            );
        },
    );
}

#[test]
fn a_panic_in_the_teardown_take_releases_nothing() {
    owned(
        module_path!(),
        "a_panic_in_the_teardown_take_releases_nothing",
        DEADLINE,
        || {
            let _scenario = scenario();
            let log = {
                let mut rig = Rig::new();
                rig.presenter.at_stop = vec![failed("late")];
                // The first report handed out is the one teardown takes.
                rig.presenter.panic_on_take = Some(1);
                rig.close_at_pump(1);
                let log = Arc::clone(&rig.log);
                let caught =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.run_collect()));
                assert!(caught.is_err(), "the injected panic unwinds through run");
                calls(&log)
            };
            assert!(
                log.iter().any(|line| line == "presenter.poll_end"),
                "the panic came after the presenter was observed finished: {log:#?}"
            );
            nothing_destroyed_or_dropped(&log);
        },
    );
}
