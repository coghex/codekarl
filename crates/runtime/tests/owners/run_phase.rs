//! The run phase of `ck-runtime`: the run closure, `RunEnd`, `Report::quit` and the outcome precedence.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ck_runtime::{Failure, Outcome, Report, RunEnd, Runtime, ServiceError};

use super::fakes::{Cleanup, Fakes, Script, Step};
use super::scenario;

/// A run closure that logs a record in category `test`, notes `run` in the call log and ends as `end`.
fn run_closure(
    fakes: &Arc<Fakes>,
    end: RunEnd,
) -> impl FnOnce(&ck_runtime::RunContext) -> RunEnd + use<> {
    let fakes = Arc::clone(fakes);
    move |_| {
        ck_log::info!("test", "run closure");
        fakes.mark("run");
        end
    }
}

fn runtime(fakes: &Arc<Fakes>) -> Runtime {
    Runtime::new(vec![fakes.service("svc")]).with_owners(vec![fakes.spawn("a", Script::default())])
}

const WITH_RUN: [&str; 7] = [
    "INFO  runtime: started svc",
    "INFO  runtime: started a",
    "INFO  runtime: start-up complete",
    "INFO  test: run closure",
    "INFO  runtime: stopped a",
    "INFO  runtime: stopped svc",
    "INFO  runtime: shutdown complete",
];

#[test]
fn the_run_closure_runs_after_start_up_complete_and_before_the_stop_phase() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let (report, records) =
        output.drive(runtime(&fakes).with_run(run_closure(&fakes, RunEnd::Finished)));
    assert_eq!(records, WITH_RUN);
    assert_eq!(report.outcome, Outcome::Clean);
    assert!(!report.quit);
    let calls = fakes.calls();
    let position = |what: &str| calls.iter().position(|call| call == what).expect(what);
    assert!(position("request_start a") < position("run"));
    assert!(position("run") < position("request_stop a"), "{calls:?}");
}

#[test]
fn a_finished_run_has_exactly_the_records_and_outcome_of_a_run_without_a_closure() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let (without, without_records) = output.drive(runtime(&fakes));
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let (finished, finished_records) = output.drive(runtime(&fakes).with_run(|_| RunEnd::Finished));
    assert_eq!(finished, without);
    assert_eq!(finished_records, without_records);
}

#[test]
fn a_run_that_ends_with_quit_is_clean_with_quit_reported() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let (report, records) =
        output.drive(runtime(&fakes).with_run(run_closure(&fakes, RunEnd::Quit)));
    assert_eq!(
        report,
        Report {
            outcome: Outcome::Clean,
            missed: Vec::new(),
            unfinished: Vec::new(),
            quit: true,
        }
    );
    assert_eq!(records, WITH_RUN, "the stop phase ran as usual");
}

#[test]
fn quit_requested_reflects_the_flag_and_is_false_without_one() {
    let (_serial, output) = scenario();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let flag = Arc::new(AtomicBool::new(false));
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let (written, setter) = (Arc::clone(&seen), Arc::clone(&flag));
    let (report, _) = output.drive(runtime(&fakes).with_quit(Arc::clone(&flag)).with_run(
        move |context| {
            let mut seen = written.lock().expect("not poisoned");
            seen.push(context.quit_requested());
            setter.store(true, Ordering::SeqCst);
            seen.push(context.quit_requested());
            RunEnd::Finished
        },
    ));
    assert_eq!(*seen.lock().expect("not poisoned"), [false, true]);
    assert_eq!(report.outcome, Outcome::Clean);
    assert!(
        !report.quit,
        "the flag was set after start-up, and the closure did not return Quit"
    );

    let without = Arc::new(Mutex::new(Vec::new()));
    let written = Arc::clone(&without);
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let _ = output.drive(runtime(&fakes).with_run(move |context| {
        written
            .lock()
            .expect("not poisoned")
            .push(context.quit_requested());
        RunEnd::Finished
    }));
    assert_eq!(*without.lock().expect("not poisoned"), [false]);
}

#[test]
fn a_failed_run_logs_its_whole_message_and_the_stop_phase_still_runs() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let message = "the window broke\nsecond line\nthird line";
    let (report, records) = output
        .drive(runtime(&fakes).with_run(run_closure(&fakes, RunEnd::Failed(message.to_owned()))));
    assert_eq!(
        report,
        Report {
            outcome: Outcome::RunFailed {
                error: ServiceError::new(message),
                stop_failures: Vec::new(),
            },
            missed: Vec::new(),
            unfinished: Vec::new(),
            quit: false,
        }
    );
    assert_eq!(
        records,
        [
            "INFO  runtime: started svc",
            "INFO  runtime: started a",
            "INFO  runtime: start-up complete",
            "INFO  test: run closure",
            "ERROR runtime: run failed: the window broke",
            "  second line",
            "  third line",
            "INFO  runtime: stopped a",
            "INFO  runtime: stopped svc",
            "INFO  runtime: shutdown complete",
        ],
        "the multi-line message is intact"
    );
}

#[test]
fn a_run_failure_carries_every_stop_failure_in_the_order_the_stops_ran() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let failing_stop = |name| Script {
        stop: Step::Fail(name),
        ..Script::default()
    };
    let (report, records) = output.drive(
        Runtime::new(vec![
            fakes.service("svc"),
            fakes.service_failing_stop("last"),
        ])
        .with_owners(vec![
            fakes.spawn("ui", failing_stop("ui stop broke")),
            fakes.spawn("gameplay", failing_stop("gameplay stop broke")),
        ])
        .with_run(run_closure(&fakes, RunEnd::Failed("run broke".to_owned()))),
    );
    // Owners stop first, in reverse order; then the services in reverse order.
    let failure = |service: &str, message: &str| Failure {
        service: service.to_owned(),
        error: ServiceError::new(message),
    };
    assert_eq!(
        report.outcome,
        Outcome::RunFailed {
            error: ServiceError::new("run broke"),
            stop_failures: vec![
                failure("gameplay", "gameplay stop broke"),
                failure("ui", "ui stop broke"),
                failure("last", "last refused to stop"),
            ],
        }
    );
    assert!(
        records.contains(&"ERROR runtime: run failed: run broke".to_owned()),
        "{records:?}"
    );
    assert_eq!(
        records.last().map(String::as_str),
        Some("ERROR runtime: shutdown finished with 3 stop failure(s)")
    );
    // Every owner has its stop request before either is awaited (CK-56): the failure of one cannot
    // delay the request to the other.
    let calls = fakes.calls();
    let position = |what: &str| calls.iter().position(|call| call == what).expect(what);
    assert!(
        position("request_stop gameplay") < position("await_stopped gameplay zero_budget=false")
    );
    assert!(position("request_stop ui") < position("await_stopped gameplay zero_budget=false"));
    assert!(position("run") < position("request_stop gameplay"));
}

#[test]
fn a_start_failure_outranks_everything_and_the_run_closure_is_not_called() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let (report, _) = output.drive(
        Runtime::new(vec![fakes.service("svc")])
            .with_owners(vec![fakes.spawn_failing("a", "no thread")])
            .with_run(run_closure(&fakes, RunEnd::Failed("never".to_owned()))),
    );
    assert!(
        matches!(report.outcome, Outcome::StartFailed { .. }),
        "{:?}",
        report.outcome
    );
    assert!(!fakes.calls().iter().any(|call| call == "run"));
}

#[test]
fn run_equals_run_report_outcome_for_a_failed_run() {
    let (_serial, output) = scenario();
    let outcome = Runtime::new(Vec::new())
        .with_run(|_| RunEnd::Failed("x".to_owned()))
        .run();
    assert_eq!(
        outcome,
        Outcome::RunFailed {
            error: ServiceError::new("x"),
            stop_failures: Vec::new(),
        }
    );
    // The harness reads records by offset: the plain `run` above wrote its three records, then the
    // empty run writes its two.
    let (_, records) = output.drive(Runtime::new(Vec::new()));
    assert_eq!(
        records,
        [
            "INFO  runtime: start-up complete",
            "ERROR runtime: run failed: x",
            "INFO  runtime: shutdown complete",
            "INFO  runtime: start-up complete",
            "INFO  runtime: shutdown complete",
        ]
    );
}
