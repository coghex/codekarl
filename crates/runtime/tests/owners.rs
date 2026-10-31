//! Owner supervision in `ck-runtime`, with fake owners (no Lua, no sleeps).
//!
//! One test binary means one process-wide `ck_log` logger, so these scenarios share one installed output
//! and run one at a time: each takes `scenario()`'s lock before it drives a runtime, because the records
//! of a run are read from the shared sink by offset.

mod fakes;
mod harness;
#[path = "owners/run_phase.rs"]
mod run_phase;
#[path = "owners/start_quit.rs"]
mod start_quit;

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use ck_runtime::{Failure, Outcome, Report, Runtime, ServiceError};
use fakes::{Cleanup, Fakes, Rendezvous, Script, Step};
use harness::{Output, install};

/// The shared output, and the lock that keeps scenarios from interleaving.
fn scenario() -> (MutexGuard<'static, ()>, &'static Output) {
    static SERIAL: Mutex<()> = Mutex::new(());
    static OUTPUT: OnceLock<Output> = OnceLock::new();
    let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    (serial, OUTPUT.get_or_init(install))
}

#[test]
fn two_owners_and_a_service_run_clean_with_requests_in_flight_together() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    // Neither owner's start (or stop) work completes until both requests have arrived, so a runtime that
    // sent a request and waited before sending the next would make the barrier fail.
    let starting = Rendezvous::new(2);
    let stopping = Rendezvous::new(2);
    let script = || Script {
        start_meeting: Some(Arc::clone(&starting)),
        stop_meeting: Some(Arc::clone(&stopping)),
        ..Script::default()
    };
    let runtime = Runtime::new(vec![fakes.service("svc")]).with_owners(vec![
        fakes.spawn("ui", script()),
        fakes.spawn("gameplay", script()),
    ]);
    let (report, records) = output.drive(runtime);
    assert_eq!(
        report,
        Report {
            outcome: Outcome::Clean,
            missed: Vec::new(),
            unfinished: Vec::new(),
            quit: false,
        }
    );
    assert_eq!(
        records,
        [
            "INFO  runtime: started svc",
            "INFO  runtime: started ui",
            "INFO  runtime: started gameplay",
            "INFO  runtime: start-up complete",
            "INFO  runtime: stopped gameplay",
            "INFO  runtime: stopped ui",
            "INFO  runtime: stopped svc",
            "INFO  runtime: shutdown complete",
        ]
    );
    assert_eq!(
        fakes.calls(),
        [
            "start svc",
            "spawn ui",
            "spawn gameplay",
            "request_start ui",
            "request_start gameplay",
            "request_stop gameplay",
            "request_stop ui",
            "await_stopped gameplay zero_budget=false",
            "await_stopped ui zero_budget=false",
            "stop svc",
        ],
        "every owner is spawned before any wait, and every request goes out before any wait"
    );
}

/// What a helper that drives a runtime hands back. `_cleanup` was armed by the helper before it spawned
/// anything, and lives as long as the test body that holds this value.
struct Run {
    report: Report,
    records: Vec<String>,
    _cleanup: Cleanup,
}

/// The slow-first fixture: `b` is healthy and spawned first, `a` is stuck in its stop work and spawned
/// last, so the reverse await visits `a` first and its wait uses up the stop phase before `b`'s
/// zero-budget observation. `a`'s await first waits (inside its own budget) until `b`'s thread has
/// really ended, so that observation is guaranteed to see a finished thread.
fn run_slow_first(output: &Output, fakes: &Arc<Fakes>, a: Script) -> Run {
    // Armed before any worker exists and before the drive, so a failed assertion anywhere in the drive
    // releases and joins every worker while it unwinds.
    let cleanup = Cleanup(Arc::clone(fakes));
    let runtime = Runtime::new(vec![fakes.service("svc")])
        .with_owners(vec![
            fakes.spawn("b", Script::default()),
            fakes.spawn("a", a),
        ])
        .with_owner_deadlines(
            Duration::from_secs(5),
            Duration::from_millis(300),
            Duration::from_secs(1),
        );
    let (report, records) = output.drive(runtime);
    Run {
        report,
        records,
        _cleanup: cleanup,
    }
}

#[test]
fn a_stuck_owner_awaited_first_does_not_delay_its_sibling_and_is_missed_and_unfinished() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let Run {
        report,
        records,
        _cleanup,
    } = run_slow_first(
        output,
        &fakes,
        Script {
            stop: Step::Stuck,
            wait_for_sibling: Some("b"),
            ..Script::default()
        },
    );
    assert_eq!(report.outcome, Outcome::Clean);
    assert_eq!(report.missed, ["a"]);
    assert_eq!(report.unfinished, ["a"]);
    assert_eq!(
        records,
        [
            "INFO  runtime: started svc",
            "INFO  runtime: started b",
            "INFO  runtime: started a",
            "INFO  runtime: start-up complete",
            "ERROR runtime: a missed its stop deadline",
            "INFO  runtime: stopped b",
            "ERROR runtime: a did not finish",
            "INFO  runtime: stopped svc",
            "INFO  runtime: shutdown complete",
        ],
        "a is awaited first and misses; b is still stopped and observed finished"
    );
    let calls = fakes.calls();
    let position = |what: &str| calls.iter().position(|call| call == what).expect(what);
    assert!(position("request_stop a") < position("await_stopped a zero_budget=false"));
    assert!(position("request_stop b") < position("await_stopped a zero_budget=false"));
    assert!(
        position("await_stopped a zero_budget=false")
            < position("await_stopped b zero_budget=true")
    );
    assert!(position("request_stop a") < position("cancel a"));
    assert!(calls.contains(&"not_finished a".to_owned()), "{calls:?}");
}

#[test]
fn an_owner_that_already_finished_is_observed_with_no_time_left() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let Run {
        report,
        records,
        _cleanup,
    } = run_slow_first(
        output,
        &fakes,
        Script {
            stop: Step::Stuck,
            wait_for_sibling: Some("b"),
            ..Script::default()
        },
    );
    // `b`'s await got no budget at all (a's wait used the phase up), yet `b` is observed finished: one
    // non-waiting look at a thread that has really ended.
    assert!(
        fakes
            .calls()
            .contains(&"await_stopped b zero_budget=true".to_owned()),
        "{:?}",
        fakes.calls()
    );
    assert!(records.contains(&"INFO  runtime: stopped b".to_owned()));
    assert!(
        !report.missed.contains(&"b".to_owned()) && !report.unfinished.contains(&"b".to_owned())
    );
}

#[test]
fn a_cancelled_owner_that_then_ends_is_missed_but_not_unfinished_without_cancel_waiting() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let Run {
        report,
        records,
        _cleanup,
    } = run_slow_first(
        output,
        &fakes,
        Script {
            stop: Step::Stuck,
            ends_on_cancel: true,
            // Its await reports the miss early, leaving the phase time for the cancel to take effect.
            await_cap: Some(Duration::from_millis(80)),
            ..Script::default()
        },
    );
    assert_eq!(report.outcome, Outcome::Clean);
    assert_eq!(report.missed, ["a"], "the missed deadline stays on record");
    assert!(
        report.unfinished.is_empty(),
        "a was observed finished after the cancel"
    );
    assert!(records.contains(&"ERROR runtime: a missed its stop deadline".to_owned()));
    assert!(records.contains(&"INFO  runtime: stopped b".to_owned()));
    assert!(
        !records
            .iter()
            .any(|record| record.contains("did not finish"))
    );
    let calls = fakes.calls();
    let position = |what: &str| calls.iter().position(|call| call == what).expect(what);
    assert!(position("cancel a") < position("observed_finished a"));
}

/// Regression for review 2's residual B3: a harness assertion that fails inside `Output::drive` (here an
/// injected one, at the point where the real flush assertions sit) unwinds out of the helper before it
/// returns, so only a guard the helper armed itself can release and join the stuck worker. Runs `helper`
/// with the failure injected and returns the workers still alive afterwards.
fn alive_after_injected_failure(
    output: &Output,
    helper: impl FnOnce(&Output, &Arc<Fakes>) -> Run,
) -> Vec<&'static str> {
    let fakes = Fakes::new();
    output.inject_failure_after_run();
    let result = panic::catch_unwind(AssertUnwindSafe(|| helper(output, &fakes)));
    // The unwind of the injected failure already ran the helper's guard.
    let alive = fakes.alive_workers();
    // Account for this run's records, so the next scenario starts from a clean offset.
    let _ = output.drive(Runtime::new(Vec::new()));
    let Err(payload) = result else {
        panic!("the injected harness failure did not unwind out of the helper");
    };
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("");
    assert!(
        message.contains("injected harness failure"),
        "the unwind came from the injected failure: {message:?}"
    );
    alive
}

#[test]
fn a_failed_harness_assertion_during_the_drive_leaves_no_worker_alive() {
    let (_serial, output) = scenario();
    let stuck = || Script {
        stop: Step::Stuck,
        ..Script::default()
    };
    assert_eq!(
        alive_after_injected_failure(output, |output, fakes| run_slow_first(
            output,
            fakes,
            stuck()
        )),
        Vec::<&str>::new(),
        "run_slow_first"
    );
    assert_eq!(
        alive_after_injected_failure(output, |output, fakes| {
            run_failing_and_stuck(output, fakes, stuck())
        }),
        Vec::<&str>::new(),
        "run_failing_and_stuck"
    );
}

/// A stop where `stuck` is spawned last (so it is awaited first), fails as `script` says and then
/// stays alive through the whole stop phase.
fn run_failing_and_stuck(output: &Output, fakes: &Arc<Fakes>, script: Script) -> Run {
    // Armed before any worker exists and before the drive, as in `run_slow_first`.
    let cleanup = Cleanup(Arc::clone(fakes));
    let runtime = Runtime::new(Vec::new())
        .with_owners(vec![
            fakes.spawn("ok", Script::default()),
            fakes.spawn("stuck", script),
        ])
        .with_owner_deadlines(
            Duration::from_secs(5),
            Duration::from_millis(200),
            Duration::from_secs(1),
        );
    let (report, records) = output.drive(runtime);
    Run {
        report,
        records,
        _cleanup: cleanup,
    }
}

#[test]
fn a_failed_stop_request_to_an_owner_that_stays_stuck_is_also_a_missed_deadline() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let Run {
        report,
        records,
        _cleanup,
    } = run_failing_and_stuck(
        output,
        &fakes,
        Script {
            request_stop_fails: Some("cannot send stop"),
            ignores_cancel_when_idle: true,
            ..Script::default()
        },
    );
    assert_eq!(
        report.outcome,
        Outcome::StopFailed {
            failures: vec![Failure {
                service: "stuck".to_owned(),
                error: ServiceError::new("cannot send stop"),
            }],
        }
    );
    assert_eq!(report.missed, ["stuck"], "the final observation expired");
    assert_eq!(report.unfinished, ["stuck"]);
    assert_eq!(
        records,
        [
            "INFO  runtime: started ok",
            "INFO  runtime: started stuck",
            "INFO  runtime: start-up complete",
            "ERROR runtime: stuck failed to stop: cannot send stop",
            "INFO  runtime: stopped ok",
            "ERROR runtime: stuck missed its stop deadline",
            "ERROR runtime: stuck did not finish",
            "ERROR runtime: shutdown finished with 1 stop failure(s)",
        ]
    );
}

#[test]
fn a_failed_stop_await_from_an_owner_that_stays_stuck_is_also_a_missed_deadline() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let Run {
        report,
        records,
        _cleanup,
    } = run_failing_and_stuck(
        output,
        &fakes,
        Script {
            stop: Step::FailThenStuck("stop exploded"),
            ..Script::default()
        },
    );
    assert_eq!(
        report.outcome,
        Outcome::StopFailed {
            failures: vec![Failure {
                service: "stuck".to_owned(),
                error: ServiceError::new("stop exploded"),
            }],
        }
    );
    assert_eq!(report.missed, ["stuck"], "the final observation expired");
    assert_eq!(report.unfinished, ["stuck"]);
    assert_eq!(
        records,
        [
            "INFO  runtime: started ok",
            "INFO  runtime: started stuck",
            "INFO  runtime: start-up complete",
            "ERROR runtime: stuck failed to stop: stop exploded",
            "INFO  runtime: stopped ok",
            "ERROR runtime: stuck missed its stop deadline",
            "ERROR runtime: stuck did not finish",
            "ERROR runtime: shutdown finished with 1 stop failure(s)",
        ]
    );
}

#[test]
fn a_failed_spawn_of_the_second_owner_cancels_the_first_and_rolls_services_back() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let runtime = Runtime::new(vec![fakes.service("svc")]).with_owners(vec![
        fakes.spawn("a", Script::default()),
        fakes.spawn_failing("b", "cannot create the thread"),
    ]);
    let (report, records) = output.drive(runtime);
    assert_eq!(
        report.outcome,
        Outcome::StartFailed {
            service: "b".to_owned(),
            error: ServiceError::new("cannot create the thread"),
            stop_failures: Vec::new(),
        }
    );
    assert!(report.unfinished.is_empty(), "a was observed finished");
    assert!(report.missed.is_empty());
    assert_eq!(
        records,
        [
            "INFO  runtime: started svc",
            "ERROR runtime: b failed to start: cannot create the thread",
            "INFO  runtime: stopped svc",
            "INFO  runtime: shutdown complete",
        ]
    );
    assert_eq!(
        fakes.calls(),
        [
            "start svc",
            "spawn a",
            "spawn b",
            "cancel a",
            "observed_finished a",
            "stop svc"
        ],
        "a is cancelled and observed; no stop was requested"
    );
}

const TRACEBACK: &str = "boom\nstack traceback:\n\t[C]: in ?";

#[test]
fn readiness_and_start_failures_cancel_every_owner_and_request_no_stop() {
    let (_serial, output) = scenario();

    // A readiness failure, with a multi-line traceback in one record.
    let fakes = Fakes::new();
    let cleanup = Cleanup(Arc::clone(&fakes));
    let runtime = Runtime::new(Vec::new()).with_owners(vec![
        fakes.spawn(
            "a",
            Script {
                ready: Step::Fail(TRACEBACK),
                ..Script::default()
            },
        ),
        fakes.spawn("b", Script::default()),
    ]);
    let (report, records) = output.drive(runtime);
    assert_eq!(
        report.outcome,
        Outcome::StartFailed {
            service: "a".to_owned(),
            error: ServiceError::new(TRACEBACK),
            stop_failures: Vec::new(),
        }
    );
    assert!(report.unfinished.is_empty() && report.missed.is_empty());
    assert_eq!(
        records,
        [
            "ERROR runtime: a failed to start: boom",
            "  stack traceback:",
            "  \t[C]: in ?",
            "INFO  runtime: shutdown complete",
        ],
        "the whole traceback is one record"
    );
    let calls = fakes.calls();
    assert!(calls.contains(&"cancel a".to_owned()) && calls.contains(&"cancel b".to_owned()));
    assert!(calls.contains(&"observed_finished a".to_owned()));
    assert!(calls.contains(&"observed_finished b".to_owned()));
    assert!(
        !calls.iter().any(|call| call.starts_with("request_stop")),
        "{calls:?}"
    );
    drop(cleanup);

    // The second owner's start fails: the first was started, both are cancelled, no stop is sent.
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let runtime = Runtime::new(vec![fakes.service("svc")]).with_owners(vec![
        fakes.spawn("a", Script::default()),
        fakes.spawn(
            "b",
            Script {
                start: Step::Fail("b refused to start"),
                ..Script::default()
            },
        ),
    ]);
    let (report, records) = output.drive(runtime);
    assert_eq!(
        report.outcome,
        Outcome::StartFailed {
            service: "b".to_owned(),
            error: ServiceError::new("b refused to start"),
            stop_failures: Vec::new(),
        }
    );
    assert!(report.unfinished.is_empty());
    assert_eq!(
        records,
        [
            "INFO  runtime: started svc",
            "INFO  runtime: started a",
            "ERROR runtime: b failed to start: b refused to start",
            "INFO  runtime: stopped svc",
            "INFO  runtime: shutdown complete",
        ]
    );
    let calls = fakes.calls();
    assert!(calls.contains(&"cancel a".to_owned()) && calls.contains(&"cancel b".to_owned()));
    assert!(calls.contains(&"observed_finished a".to_owned()));
    assert!(calls.contains(&"observed_finished b".to_owned()));
    assert!(
        !calls.iter().any(|call| call.starts_with("request_stop")),
        "{calls:?}"
    );
}

#[test]
fn a_stuck_readiness_misses_the_start_deadline_and_is_reported_after_the_cleanup() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let start = Duration::from_millis(200);
    let cleanup = Duration::from_millis(200);
    let runtime = Runtime::new(Vec::new())
        .with_owners(vec![
            fakes.spawn(
                "slow",
                Script {
                    ready: Step::Stuck,
                    ..Script::default()
                },
            ),
            fakes.spawn("fine", Script::default()),
        ])
        .with_owner_deadlines(start, Duration::from_secs(5), cleanup);
    let begun = Instant::now();
    let (report, records) = output.drive(runtime);
    let took = begun.elapsed();
    assert_eq!(
        report.outcome,
        Outcome::StartFailed {
            service: "slow".to_owned(),
            error: ServiceError::new("missed its start deadline"),
            stop_failures: Vec::new(),
        }
    );
    assert_eq!(report.missed, ["slow"]);
    assert_eq!(report.unfinished, ["slow"]);
    assert_eq!(
        records,
        [
            "ERROR runtime: slow missed its start deadline",
            "ERROR runtime: slow did not finish",
            "INFO  runtime: shutdown complete",
        ]
    );
    assert!(
        took >= start && took < start + cleanup + Duration::from_millis(700),
        "the run is bounded by the start phase plus the cleanup deadline: {took:?}"
    );
    assert!(
        fakes.when("cancel slow") >= begun + start,
        "the cancel comes only after the start deadline has passed"
    );
    let calls = fakes.calls();
    assert!(calls.contains(&"cancel slow".to_owned()) && calls.contains(&"cancel fine".to_owned()));
    assert!(calls.contains(&"observed_finished fine".to_owned()));
    assert!(
        !calls.iter().any(|call| call.starts_with("request_stop")),
        "{calls:?}"
    );
}

#[test]
fn a_failing_stop_is_logged_counted_and_does_not_stop_the_sibling_from_stopping() {
    let (_serial, output) = scenario();

    // `await_stopped` returns Failed.
    let fakes = Fakes::new();
    let cleanup = Cleanup(Arc::clone(&fakes));
    let runtime = Runtime::new(vec![fakes.service("svc")]).with_owners(vec![
        fakes.spawn("a", Script::default()),
        fakes.spawn(
            "b",
            Script {
                stop: Step::Fail("stop exploded"),
                ..Script::default()
            },
        ),
    ]);
    let (report, records) = output.drive(runtime);
    assert_eq!(
        report.outcome,
        Outcome::StopFailed {
            failures: vec![Failure {
                service: "b".to_owned(),
                error: ServiceError::new("stop exploded"),
            }],
        }
    );
    assert!(report.unfinished.is_empty() && report.missed.is_empty());
    assert_eq!(
        records,
        [
            "INFO  runtime: started svc",
            "INFO  runtime: started a",
            "INFO  runtime: started b",
            "INFO  runtime: start-up complete",
            "ERROR runtime: b failed to stop: stop exploded",
            "INFO  runtime: stopped a",
            "INFO  runtime: stopped svc",
            "ERROR runtime: shutdown finished with 1 stop failure(s)",
        ]
    );
    drop(cleanup);

    // `request_stop` returns Failed: the sibling still receives its request and stops, the failed
    // owner is cancelled and observed finished, and the outcome is `StopFailed`.
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let runtime = Runtime::new(Vec::new()).with_owners(vec![
        fakes.spawn("a", Script::default()),
        fakes.spawn(
            "b",
            Script {
                request_stop_fails: Some("cannot send stop"),
                ..Script::default()
            },
        ),
    ]);
    let (report, records) = output.drive(runtime);
    assert_eq!(
        report.outcome,
        Outcome::StopFailed {
            failures: vec![Failure {
                service: "b".to_owned(),
                error: ServiceError::new("cannot send stop"),
            }],
        }
    );
    assert!(
        report.unfinished.is_empty(),
        "b was cancelled and observed finished"
    );
    assert_eq!(
        records,
        [
            "INFO  runtime: started a",
            "INFO  runtime: started b",
            "INFO  runtime: start-up complete",
            "ERROR runtime: b failed to stop: cannot send stop",
            "INFO  runtime: stopped a",
            "ERROR runtime: shutdown finished with 1 stop failure(s)",
        ]
    );
    let calls = fakes.calls();
    let position = |what: &str| calls.iter().position(|call| call == what).expect(what);
    assert!(
        position("request_stop b") < position("request_stop a"),
        "reverse order"
    );
    assert!(calls.contains(&"cancel b".to_owned()));
    assert!(calls.contains(&"observed_finished b".to_owned()));
    assert!(
        !calls.contains(&"cancel a".to_owned()),
        "a stopped normally"
    );
}

#[test]
fn without_owners_run_equals_run_report_and_the_lists_are_empty() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let (report, records) =
        output.drive(Runtime::new(vec![fakes.service("a"), fakes.service("b")]));
    // `run` is the outcome of `run_report` (its records are not read here).
    let plain = Runtime::new(vec![fakes.service("a"), fakes.service("b")]).run();
    assert_eq!(plain, Outcome::Clean);
    // Read (and so discard) the records of that plain run, so they cannot leak into the next scenario.
    let (_, leftover) = output.drive(Runtime::new(Vec::new()));
    assert_eq!(
        leftover.len(),
        6 + 2,
        "the plain run's six records, then the empty run's two"
    );
    assert_eq!(
        report,
        Report {
            outcome: plain,
            missed: Vec::new(),
            unfinished: Vec::new(),
            quit: false,
        }
    );
    assert_eq!(
        records,
        [
            "INFO  runtime: started a",
            "INFO  runtime: started b",
            "INFO  runtime: start-up complete",
            "INFO  runtime: stopped b",
            "INFO  runtime: stopped a",
            "INFO  runtime: shutdown complete",
        ]
    );
}
