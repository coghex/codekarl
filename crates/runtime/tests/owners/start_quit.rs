//! The start phase of `ck-runtime` with polls and a quit request: slicing without cancelling, one real
//! deadline, a quit during start-up, and the fixed order when a quit coincides with something else.
//!
//! The fakes keep the cooperative contract: a poll that has nothing to report waits its whole budget,
//! so a deadline is reached by waiting, never by sleeping past a budget. The overlaps are made by fake
//! scripts (a poll that sets the flag as it returns), not by racing threads.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ck_runtime::{Failure, Outcome, Report, RunEnd, Runtime, ServiceError};

use super::fakes::{Cleanup, Fakes, NeverCompletes, QuitAt, Script, Step};
use super::harness::Output;
use super::scenario;

const QUIT_RECORD: &str = "INFO  runtime: quit requested during start-up";

fn flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Adds a run closure that notes `run` in the call log, to show that it was (not) called.
fn with_marked_run(runtime: Runtime, fakes: &Arc<Fakes>) -> Runtime {
    let fakes = Arc::clone(fakes);
    runtime.with_run(move |_| {
        fakes.mark("run");
        RunEnd::Finished
    })
}

fn started_failed(service: &str, message: &str) -> Outcome {
    Outcome::StartFailed {
        service: service.to_owned(),
        error: ServiceError::new(message),
        stop_failures: Vec::new(),
    }
}

/// A report whose outcome is `Clean` with the quit flag seen.
fn clean_quit(unfinished: &[&str]) -> Report {
    Report {
        outcome: Outcome::Clean,
        missed: Vec::new(),
        unfinished: unfinished.iter().map(|name| (*name).to_owned()).collect(),
        quit: true,
    }
}

fn has(calls: &[String], what: &str) -> bool {
    calls.iter().any(|call| call == what)
}

// ---------------------------------------------------------------------------------------------
// Slicing and the one real deadline
// ---------------------------------------------------------------------------------------------

#[test]
fn slicing_does_not_cancel_a_slow_owner_and_the_records_are_today_s() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let slow = Script {
        pending_ready_polls: 20,
        pending_started_polls: 20,
        ..Script::default()
    };
    let begun = Instant::now();
    let (report, records) = output.drive(
        Runtime::new(vec![fakes.service("svc")])
            .with_owners(vec![fakes.spawn("a", slow)])
            .with_poll_slice(Duration::from_millis(10)),
    );
    assert!(
        begun.elapsed() >= Duration::from_millis(350),
        "20 pending polls of 10 ms each, twice: {:?}",
        begun.elapsed()
    );
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
            "INFO  runtime: started a",
            "INFO  runtime: start-up complete",
            "INFO  runtime: stopped a",
            "INFO  runtime: stopped svc",
            "INFO  runtime: shutdown complete",
        ]
    );
    let (ready_polls, started_polls) = fakes.polls_of("a");
    assert!(
        ready_polls >= 21 && started_polls >= 21,
        "slicing really happened: {ready_polls} and {started_polls} polls"
    );
    assert!(
        !has(&fakes.calls(), "cancel a"),
        "no cancel: {:?}",
        fakes.calls()
    );
    assert!(
        fakes.waits().is_empty(),
        "no abandoning wait was made: {:?}",
        fakes.waits()
    );
}

#[test]
fn an_owner_that_never_becomes_ready_gets_exactly_one_zero_budget_wait_at_the_deadline() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let start = Duration::from_millis(300);
    let cleanup = Duration::from_secs(1);
    let begun = Instant::now();
    let (report, records) = output.drive(
        Runtime::new(Vec::new())
            .with_owners(vec![fakes.spawn(
                "a",
                Script {
                    ready: Step::Stuck,
                    ends_on_cancel: true,
                    ..Script::default()
                },
            )])
            .with_owner_deadlines(start, Duration::from_secs(5), cleanup),
    );
    let elapsed = begun.elapsed();
    assert_eq!(
        report.outcome,
        started_failed("a", "missed its start deadline")
    );
    assert_eq!(report.missed, ["a"]);
    assert!(!report.quit);
    assert_eq!(fakes.waits(), ["wait_ready a zero_budget=true"]);
    assert_eq!(
        records,
        [
            "ERROR runtime: a missed its start deadline",
            "INFO  runtime: shutdown complete"
        ]
    );
    assert!(has(&fakes.calls(), "cancel a"), "{:?}", fakes.calls());
    assert!(
        elapsed >= start && elapsed < start + cleanup + Duration::from_millis(700),
        "{elapsed:?}"
    );
    assert!(fakes.polls_of("a").0 >= 10, "the wait was sliced");
}

#[test]
fn a_start_that_never_completes_gets_exactly_one_zero_budget_wait_at_the_deadline() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let start = Duration::from_millis(300);
    let cleanup = Duration::from_secs(1);
    let begun = Instant::now();
    let (report, records) = output.drive(
        Runtime::new(Vec::new())
            .with_owners(vec![fakes.spawn(
                "a",
                Script {
                    start: Step::Stuck,
                    ends_on_cancel: true,
                    ..Script::default()
                },
            )])
            .with_owner_deadlines(start, Duration::from_secs(5), cleanup),
    );
    let elapsed = begun.elapsed();
    assert_eq!(
        report.outcome,
        started_failed("a", "missed its start deadline")
    );
    assert_eq!(report.missed, ["a"]);
    assert_eq!(fakes.waits(), ["await_started a zero_budget=true"]);
    assert_eq!(
        records,
        [
            "ERROR runtime: a missed its start deadline",
            "INFO  runtime: shutdown complete"
        ]
    );
    assert!(has(&fakes.calls(), "cancel a"));
    assert!(
        elapsed >= start && elapsed < start + cleanup + Duration::from_millis(700),
        "{elapsed:?}"
    );
}

#[test]
fn a_result_that_arrives_at_the_last_moment_is_used_normally_not_counted_as_missed() {
    // The polls never report the result, but it is there when the zero-budget observation is made at
    // the deadline: for readiness, for the start, or for both (the start reply is put in at once, so no
    // worker has to be scheduled in between).
    let (_serial, output) = scenario();
    let cases: [(NeverCompletes, &[&str]); 3] = [
        (NeverCompletes::Ready, &["wait_ready a zero_budget=true"]),
        (
            NeverCompletes::Started,
            &["await_started a zero_budget=true"],
        ),
        (
            NeverCompletes::Both,
            &[
                "wait_ready a zero_budget=true",
                "await_started a zero_budget=true",
            ],
        ),
    ];
    for (never, waits) in cases {
        let fakes = Fakes::new();
        let _cleanup = Cleanup(Arc::clone(&fakes));
        let (report, records) = output.drive(
            Runtime::new(Vec::new())
                .with_owners(vec![fakes.spawn(
                    "a",
                    Script {
                        polls_never_complete: never,
                        instant_start: true,
                        ..Script::default()
                    },
                )])
                .with_owner_deadlines(
                    Duration::from_millis(150),
                    Duration::from_secs(5),
                    Duration::from_secs(1),
                ),
        );
        assert_eq!(
            report,
            Report {
                outcome: Outcome::Clean,
                missed: Vec::new(),
                unfinished: Vec::new(),
                quit: false,
            }
        );
        assert_eq!(fakes.waits(), waits);
        assert_eq!(
            records,
            [
                "INFO  runtime: started a",
                "INFO  runtime: start-up complete",
                "INFO  runtime: stopped a",
                "INFO  runtime: shutdown complete",
            ]
        );
        assert!(!has(&fakes.calls(), "cancel a"));
    }
}

// ---------------------------------------------------------------------------------------------
// A quit during start-up
// ---------------------------------------------------------------------------------------------

/// Drives `runtime`, with a run closure that marks `run`, and returns the report, the records and the
/// call log.
fn drive_marked(
    output: &Output,
    fakes: &Arc<Fakes>,
    runtime: Runtime,
) -> (Report, Vec<String>, Vec<String>) {
    let (report, records) = output.drive(with_marked_run(runtime, fakes));
    (report, records, fakes.calls())
}

#[test]
fn a_flag_set_before_the_first_service_starts_nothing() {
    let (_serial, output) = scenario();
    for with_owners in [false, true] {
        let fakes = Fakes::new();
        let _cleanup = Cleanup(Arc::clone(&fakes));
        let quit = flag();
        quit.store(true, Ordering::SeqCst);
        let mut runtime = Runtime::new(vec![fakes.service("svc")]).with_quit(Arc::clone(&quit));
        if with_owners {
            runtime = runtime.with_owners(vec![fakes.spawn("a", Script::default())]);
        }
        let (report, records, calls) = drive_marked(output, &fakes, runtime);
        assert_eq!(report, clean_quit(&[]), "owners: {with_owners}");
        assert_eq!(
            records,
            [QUIT_RECORD, "INFO  runtime: shutdown complete"],
            "owners: {with_owners}"
        );
        assert!(calls.is_empty(), "nothing started or spawned: {calls:?}");
    }
}

#[test]
fn no_owners_and_the_flag_set_before_the_run_takes_the_quit_path() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let quit = flag();
    quit.store(true, Ordering::SeqCst);
    let (report, records, calls) = drive_marked(
        output,
        &fakes,
        Runtime::new(Vec::new()).with_quit(Arc::clone(&quit)),
    );
    assert_eq!(report, clean_quit(&[]));
    assert_eq!(records, [QUIT_RECORD, "INFO  runtime: shutdown complete"]);
    assert!(calls.is_empty());
}

#[test]
fn the_last_service_setting_the_flag_with_no_owners_is_caught_by_the_final_check() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let quit = flag();
    let (report, records, calls) = drive_marked(
        output,
        &fakes,
        Runtime::new(vec![
            fakes.service("first"),
            fakes.service_setting_quit("last", &quit),
        ])
        .with_quit(Arc::clone(&quit)),
    );
    assert_eq!(report, clean_quit(&[]));
    assert_eq!(
        records,
        [
            "INFO  runtime: started first",
            "INFO  runtime: started last",
            QUIT_RECORD,
            "INFO  runtime: stopped last",
            "INFO  runtime: stopped first",
            "INFO  runtime: shutdown complete",
        ]
    );
    assert_eq!(
        calls,
        ["start first", "start last", "stop last", "stop first"]
    );
}

#[test]
fn a_flag_set_before_a_spawn_spawns_no_more_and_cancels_the_ones_spawned() {
    let (_serial, output) = scenario();
    // Before the first spawn: a service sets it.
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let quit = flag();
    let (report, records, calls) = drive_marked(
        output,
        &fakes,
        Runtime::new(vec![fakes.service_setting_quit("svc", &quit)])
            .with_owners(vec![
                fakes.spawn("a", Script::default()),
                fakes.spawn("b", Script::default()),
            ])
            .with_quit(Arc::clone(&quit)),
    );
    assert_eq!(report, clean_quit(&[]));
    assert_eq!(
        records,
        [
            "INFO  runtime: started svc",
            QUIT_RECORD,
            "INFO  runtime: stopped svc",
            "INFO  runtime: shutdown complete",
        ]
    );
    assert_eq!(calls, ["start svc", "stop svc"], "no owner was spawned");

    // Before the second spawn: the first owner's creation sets it.
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let quit = flag();
    let (report, records, calls) = drive_marked(
        output,
        &fakes,
        Runtime::new(Vec::new())
            .with_owners(vec![
                fakes.spawn_setting_quit(
                    "a",
                    Script {
                        ends_on_cancel: true,
                        ..Script::default()
                    },
                    &quit,
                ),
                fakes.spawn("b", Script::default()),
            ])
            .with_quit(Arc::clone(&quit)),
    );
    assert_eq!(report, clean_quit(&[]));
    assert_eq!(records, [QUIT_RECORD, "INFO  runtime: shutdown complete"]);
    assert_eq!(
        calls,
        ["spawn a", "cancel a", "observed_finished a"],
        "b was never spawned, a was cancelled and observed, and no stop request was sent"
    );
}

#[test]
fn a_flag_set_before_a_request_start_sends_no_start_and_cancels_every_owner() {
    let (_serial, output) = scenario();
    // Set by the last readiness: the check before the first request_start sees it.
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let quit = flag();
    let cancels = Script {
        ends_on_cancel: true,
        ..Script::default()
    };
    let (report, records, calls) = drive_marked(
        output,
        &fakes,
        Runtime::new(vec![fakes.service("svc")])
            .with_owners(vec![
                fakes.spawn(
                    "a",
                    Script {
                        sets_quit: Some((Arc::clone(&quit), QuitAt::ReadyResult)),
                        ..cancels.clone()
                    },
                ),
                fakes.spawn("b", cancels.clone()),
            ])
            .with_quit(Arc::clone(&quit)),
    );
    assert_eq!(report, clean_quit(&[]));
    assert_eq!(
        records,
        [
            "INFO  runtime: started svc",
            QUIT_RECORD,
            "INFO  runtime: stopped svc",
            "INFO  runtime: shutdown complete",
        ]
    );
    assert!(
        !calls.iter().any(|call| call.starts_with("request_")),
        "no request at all: {calls:?}"
    );
    assert!(
        has(&calls, "cancel a") && has(&calls, "cancel b"),
        "{calls:?}"
    );
    assert!(!has(&calls, "run"));

    // Set in the first request_start: the check before the second one sees it.
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let quit = flag();
    let (report, _records, calls) = drive_marked(
        output,
        &fakes,
        Runtime::new(Vec::new())
            .with_owners(vec![
                fakes.spawn(
                    "a",
                    Script {
                        sets_quit: Some((Arc::clone(&quit), QuitAt::RequestStart)),
                        ..cancels.clone()
                    },
                ),
                fakes.spawn("b", cancels),
            ])
            .with_quit(Arc::clone(&quit)),
    );
    assert_eq!(report, clean_quit(&[]));
    assert!(has(&calls, "request_start a"), "{calls:?}");
    assert!(!has(&calls, "request_start b"), "{calls:?}");
    assert!(!calls.iter().any(|call| call.starts_with("request_stop")));
    assert!(
        has(&calls, "cancel a") && has(&calls, "cancel b"),
        "{calls:?}"
    );
    assert!(!has(&calls, "run"));
}

/// An owner stuck in the stage, which sets the flag in its n-th `Pending` poll and ends when cancelled.
fn stuck_then_quit(stage: &str, quit: &Arc<AtomicBool>) -> Script {
    let (at, ready, start) = match stage {
        "ready" => (QuitAt::ReadyPending(3), Step::Stuck, Step::Done),
        _ => (QuitAt::StartedPending(3), Step::Done, Step::Stuck),
    };
    Script {
        ready,
        start,
        ends_on_cancel: true,
        sets_quit: Some((Arc::clone(quit), at)),
        ..Script::default()
    }
}

#[test]
fn a_quit_while_an_owner_is_stuck_in_readiness_or_start_is_noticed_within_one_slice() {
    let (_serial, output) = scenario();
    for stage in ["ready", "start"] {
        let fakes = Fakes::new();
        let _cleanup = Cleanup(Arc::clone(&fakes));
        let quit = flag();
        let slice = Duration::from_millis(20);
        let begun = Instant::now();
        let (report, records, calls) = drive_marked(
            output,
            &fakes,
            Runtime::new(vec![fakes.service("svc")])
                .with_owners(vec![fakes.spawn("a", stuck_then_quit(stage, &quit))])
                // A long start deadline: only the quit can end the phase early.
                .with_owner_deadlines(
                    Duration::from_secs(20),
                    Duration::from_secs(5),
                    Duration::from_secs(1),
                )
                .with_poll_slice(slice)
                .with_quit(Arc::clone(&quit)),
        );
        assert!(
            begun.elapsed() < Duration::from_secs(5),
            "{stage}: {:?}",
            begun.elapsed()
        );
        assert_eq!(report, clean_quit(&[]), "{stage}");
        assert_eq!(
            records,
            [
                "INFO  runtime: started svc",
                QUIT_RECORD,
                "INFO  runtime: stopped svc",
                "INFO  runtime: shutdown complete",
            ],
            "{stage}"
        );
        assert!(!has(&calls, "run"), "{stage}");
        assert!(
            has(&calls, "cancel a") && has(&calls, "observed_finished a"),
            "{stage}: {calls:?}"
        );
        assert!(
            !calls.iter().any(|call| call.starts_with("request_stop")),
            "{stage}"
        );
        let gap = fakes.when("cancel a") - fakes.when("set quit a");
        assert!(
            gap < slice + Duration::from_millis(250),
            "{stage}: the quit was noticed {gap:?} after it was set"
        );
        assert!(
            fakes.waits().is_empty(),
            "{stage}: no abandoning wait: {:?}",
            fakes.waits()
        );
    }
}

// ---------------------------------------------------------------------------------------------
// A quit that coincides with something else
// ---------------------------------------------------------------------------------------------

#[test]
fn the_last_poll_started_setting_the_flag_and_returning_done_takes_the_quit_path() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let quit = flag();
    let (report, records, calls) = drive_marked(
        output,
        &fakes,
        Runtime::new(vec![fakes.service("svc")])
            .with_owners(vec![fakes.spawn(
                "a",
                Script {
                    ends_on_cancel: true,
                    sets_quit: Some((Arc::clone(&quit), QuitAt::StartedResult)),
                    ..Script::default()
                },
            )])
            .with_quit(Arc::clone(&quit)),
    );
    assert_eq!(report, clean_quit(&[]));
    assert_eq!(
        records,
        [
            "INFO  runtime: started svc",
            "INFO  runtime: started a",
            QUIT_RECORD,
            "INFO  runtime: stopped svc",
            "INFO  runtime: shutdown complete",
        ],
        "no `start-up complete`"
    );
    assert!(!has(&calls, "run"));
    assert!(
        !calls.iter().any(|call| call.starts_with("request_stop")),
        "{calls:?}"
    );
    assert!(
        has(&calls, "cancel a") && has(&calls, "observed_finished a"),
        "{calls:?}"
    );
}

#[test]
fn a_failure_returned_together_with_the_flag_is_a_start_failure_and_quit_is_reported() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let quit = flag();
    let (report, records, calls) = drive_marked(
        output,
        &fakes,
        Runtime::new(vec![fakes.service("svc")])
            .with_owners(vec![fakes.spawn(
                "a",
                Script {
                    start: Step::Fail("start broke"),
                    sets_quit: Some((Arc::clone(&quit), QuitAt::StartedResult)),
                    ..Script::default()
                },
            )])
            .with_quit(Arc::clone(&quit)),
    );
    assert_eq!(
        report,
        Report {
            outcome: Outcome::StartFailed {
                service: "a".to_owned(),
                error: ServiceError::new("start broke"),
                stop_failures: Vec::new(),
            },
            missed: Vec::new(),
            unfinished: Vec::new(),
            quit: true,
        }
    );
    assert_eq!(
        records,
        [
            "INFO  runtime: started svc",
            "ERROR runtime: a failed to start: start broke",
            "INFO  runtime: stopped svc",
            "INFO  runtime: shutdown complete",
        ],
        "the failure record, and no quit record"
    );
    assert!(!has(&calls, "run"));
}

#[test]
fn a_pending_poll_after_the_deadline_with_the_flag_set_takes_the_missed_path_first() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let quit = flag();
    // The slice is longer than the deadline, so the first poll's budget is the time left: the fake
    // waits it out, sets the flag, and returns `Pending` with the deadline passed.
    let (report, records, calls) = drive_marked(
        output,
        &fakes,
        Runtime::new(Vec::new())
            .with_owners(vec![fakes.spawn(
                "a",
                Script {
                    ready: Step::Stuck,
                    ends_on_cancel: true,
                    sets_quit: Some((Arc::clone(&quit), QuitAt::ReadyPending(1))),
                    ..Script::default()
                },
            )])
            .with_owner_deadlines(
                Duration::from_millis(150),
                Duration::from_secs(5),
                Duration::from_secs(1),
            )
            .with_poll_slice(Duration::from_secs(1))
            .with_quit(Arc::clone(&quit)),
    );
    assert_eq!(
        report.outcome,
        started_failed("a", "missed its start deadline")
    );
    assert_eq!(report.missed, ["a"]);
    assert!(report.quit, "the flag is read at the end of the phase");
    assert_eq!(
        fakes.waits(),
        ["wait_ready a zero_budget=true"],
        "the zero-budget observation was made"
    );
    assert_eq!(
        records,
        [
            "ERROR runtime: a missed its start deadline",
            "INFO  runtime: shutdown complete"
        ],
        "the miss, not a quit"
    );
    assert!(has(&calls, "cancel a") && !has(&calls, "run"));
}

#[test]
fn a_quit_leaves_a_stuck_sibling_in_unfinished_and_reports_only_failures_actually_returned() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let quit = flag();
    let (report, records, calls) = drive_marked(
        output,
        &fakes,
        Runtime::new(Vec::new())
            .with_owners(vec![
                // Stuck in readiness, ignores cancel, and sets the flag in its second pending poll.
                fakes.spawn(
                    "a",
                    Script {
                        ready: Step::Stuck,
                        sets_quit: Some((Arc::clone(&quit), QuitAt::ReadyPending(2))),
                        ..Script::default()
                    },
                ),
                // Would fail at once, but the quit comes first: the supervisor never polls it, and the
                // quit path does not sweep it either.
                fakes.spawn(
                    "b",
                    Script {
                        ready: Step::Fail("never returned"),
                        ..Script::default()
                    },
                ),
            ])
            .with_owner_deadlines(
                Duration::from_secs(20),
                Duration::from_secs(5),
                Duration::from_millis(300),
            )
            .with_quit(Arc::clone(&quit)),
    );
    assert_eq!(
        report.outcome,
        Outcome::Clean,
        "b's failure was never returned"
    );
    assert_eq!(report.unfinished, ["a"]);
    assert!(report.missed.is_empty() && report.quit);
    assert_eq!(
        records,
        [
            QUIT_RECORD,
            "ERROR runtime: a did not finish",
            "INFO  runtime: shutdown complete"
        ]
    );
    assert!(
        has(&calls, "cancel a") && has(&calls, "cancel b"),
        "{calls:?}"
    );
    assert!(has(&calls, "not_finished a"), "{calls:?}");
    assert!(!has(&calls, "run"));
}

#[test]
fn a_quit_with_a_failing_service_stop_still_reports_the_stop_failure() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let quit = flag();
    let (report, _records, _calls) = drive_marked(
        output,
        &fakes,
        Runtime::new(vec![
            fakes.service_failing_stop("svc"),
            fakes.service_setting_quit("last", &quit),
        ])
        .with_quit(Arc::clone(&quit)),
    );
    assert_eq!(
        report,
        Report {
            outcome: Outcome::StopFailed {
                failures: vec![Failure {
                    service: "svc".to_owned(),
                    error: ServiceError::new("svc refused to stop"),
                }],
            },
            missed: Vec::new(),
            unfinished: Vec::new(),
            quit: true,
        },
        "quit never hides a failure"
    );
}

#[test]
fn without_a_flag_or_a_run_closure_the_runtime_behaves_as_before_and_the_flag_is_only_read() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let (report, records) = output.drive(
        Runtime::new(vec![fakes.service("svc")])
            .with_owners(vec![fakes.spawn("a", Script::default())]),
    );
    assert_eq!(report.outcome, Outcome::Clean);
    assert!(!report.quit);
    assert_eq!(
        records,
        [
            "INFO  runtime: started svc",
            "INFO  runtime: started a",
            "INFO  runtime: start-up complete",
            "INFO  runtime: stopped a",
            "INFO  runtime: stopped svc",
            "INFO  runtime: shutdown complete",
        ]
    );

    // A flag that is never set changes nothing, and the runtime never writes it.
    let quit = flag();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let (report, _) = output.drive(
        Runtime::new(vec![fakes.service("svc")])
            .with_owners(vec![fakes.spawn("a", Script::default())])
            .with_quit(Arc::clone(&quit)),
    );
    assert_eq!(report.outcome, Outcome::Clean);
    assert!(!report.quit);
    assert!(
        !quit.load(Ordering::SeqCst),
        "the runtime only reads the flag"
    );
}

#[test]
fn a_zero_poll_slice_means_the_default_slice() {
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let begun = Instant::now();
    let (report, _) = output.drive(
        Runtime::new(Vec::new())
            .with_owners(vec![fakes.spawn(
                "a",
                Script {
                    pending_ready_polls: 3,
                    ..Script::default()
                },
            )])
            .with_poll_slice(Duration::ZERO),
    );
    assert_eq!(report.outcome, Outcome::Clean);
    assert!(fakes.polls_of("a").0 >= 4, "polled in slices");
    assert!(
        begun.elapsed() >= Duration::from_millis(25),
        "three pending polls of the default 10 ms slice: {:?}",
        begun.elapsed()
    );
}

#[test]
fn time_spent_starting_services_is_never_charged_to_the_owners_start_deadline() {
    // The services take 300 ms to start; the owner needs about 100 ms of polling inside a 250 ms start
    // deadline. The deadline is formed just before the first spawn, after the services, so it holds.
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let (report, records) = output.drive(
        Runtime::new(vec![fakes.slow_service("slow", Duration::from_millis(300))])
            .with_owners(vec![fakes.spawn(
                "a",
                Script {
                    pending_ready_polls: 10,
                    ..Script::default()
                },
            )])
            .with_owner_deadlines(
                Duration::from_millis(250),
                Duration::from_secs(5),
                Duration::from_secs(1),
            ),
    );
    assert_eq!(report.outcome, Outcome::Clean, "{records:?}");
    assert!(report.missed.is_empty());
}

const MANY: [&str; 16] = [
    "o01", "o02", "o03", "o04", "o05", "o06", "o07", "o08", "o09", "o10", "o11", "o12", "o13",
    "o14", "o15", "o16",
];

#[test]
fn a_quit_set_as_one_owner_completes_is_seen_before_the_next_owner_is_polled() {
    let (_serial, output) = scenario();
    for stage in ["ready", "start"] {
        let fakes = Fakes::new();
        let _cleanup = Cleanup(Arc::clone(&fakes));
        let quit = flag();
        let (at, b) = if stage == "ready" {
            (
                QuitAt::ReadyResult,
                Script {
                    ready: Step::Fail("b must never be polled"),
                    ..Script::default()
                },
            )
        } else {
            (
                QuitAt::StartedResult,
                Script {
                    start: Step::Fail("b must never be polled"),
                    ..Script::default()
                },
            )
        };
        let a = Script {
            ends_on_cancel: true,
            sets_quit: Some((Arc::clone(&quit), at)),
            ..Script::default()
        };
        let (report, records, calls) = drive_marked(
            output,
            &fakes,
            Runtime::new(Vec::new())
                .with_owners(vec![fakes.spawn("a", a), fakes.spawn("b", b)])
                .with_quit(Arc::clone(&quit)),
        );
        assert_eq!(
            report,
            clean_quit(&[]),
            "{stage}: B's would-be failure never appears"
        );
        let (ready_polls, started_polls) = fakes.polls_of("b");
        if stage == "ready" {
            assert_eq!((ready_polls, started_polls), (0, 0), "B was never polled");
            assert_eq!(records, [QUIT_RECORD, "INFO  runtime: shutdown complete"]);
        } else {
            assert!(ready_polls >= 1, "B's readiness was polled before the quit");
            assert_eq!(started_polls, 0, "but B's start never was");
            assert_eq!(
                records,
                [
                    "INFO  runtime: started a",
                    QUIT_RECORD,
                    "INFO  runtime: shutdown complete"
                ],
                "the quit is seen right after A's started record, before B is polled"
            );
        }
        assert!(
            has(&calls, "cancel a") && has(&calls, "cancel b"),
            "{stage}: {calls:?}"
        );
        assert!(
            !calls.iter().any(|call| call.starts_with("request_stop")),
            "{stage}"
        );
        assert!(!has(&calls, "run"), "{stage}");
    }
}

#[test]
fn with_many_cooperative_owners_the_quit_is_seen_before_the_next_owner_not_after_all_of_them() {
    let (_serial, output) = scenario();
    for stage in ["ready", "start"] {
        let fakes = Fakes::new();
        let _cleanup = Cleanup(Arc::clone(&fakes));
        let quit = flag();
        let at = if stage == "ready" {
            QuitAt::ReadyResult
        } else {
            QuitAt::StartedResult
        };
        let owners = MANY
            .iter()
            .enumerate()
            .map(|(index, name)| {
                fakes.spawn(
                    name,
                    Script {
                        ends_on_cancel: true,
                        sets_quit: (index == 0).then(|| (Arc::clone(&quit), at)),
                        ..Script::default()
                    },
                )
            })
            .collect();
        let (report, _records, calls) = drive_marked(
            output,
            &fakes,
            Runtime::new(Vec::new())
                .with_owners(owners)
                .with_quit(Arc::clone(&quit)),
        );
        assert_eq!(report, clean_quit(&[]), "{stage}");
        for name in &MANY[1..] {
            let (ready, started) = fakes.polls_of(name);
            if stage == "ready" {
                assert_eq!(
                    (ready, started),
                    (0, 0),
                    "{stage}: {name} was polled after the quit"
                );
            } else {
                assert_eq!(
                    started, 0,
                    "{stage}: {name}'s start was polled after the quit"
                );
            }
        }
        assert!(
            MANY.iter()
                .all(|name| has(&calls, &format!("cancel {name}"))),
            "{stage}"
        );
    }
}

#[test]
fn a_failure_returned_by_the_poll_that_set_the_flag_still_wins_over_the_quit() {
    // The same arrangement as `a_failure_returned_together_with_the_flag...`, but with a sibling behind
    // it: the issued poll's failure is processed first, and the sibling is not polled either.
    let (_serial, output) = scenario();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let quit = flag();
    let (report, records, _calls) = drive_marked(
        output,
        &fakes,
        Runtime::new(Vec::new())
            .with_owners(vec![
                fakes.spawn(
                    "a",
                    Script {
                        ready: Step::Fail("a broke"),
                        sets_quit: Some((Arc::clone(&quit), QuitAt::ReadyResult)),
                        ..Script::default()
                    },
                ),
                fakes.spawn("b", Script::default()),
            ])
            .with_quit(Arc::clone(&quit)),
    );
    assert_eq!(report.outcome, started_failed("a", "a broke"));
    assert!(report.quit);
    assert_eq!(
        records,
        [
            "ERROR runtime: a failed to start: a broke",
            "INFO  runtime: shutdown complete"
        ]
    );
    assert_eq!(fakes.polls_of("b"), (0, 0));
}
