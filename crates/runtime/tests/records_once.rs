//! Scenario 7: each step, and the terminal record, is logged exactly once on every path.
//!
//! One process runs the paths one after another over a single installed logger. Service names are
//! unique across runs, so a repeated record would be a duplicate line; each path also has an exact
//! record count and ends with exactly one terminal record.

mod common;

use std::collections::BTreeSet;

use ck_runtime::Outcome;
use common::{Run, Spec, install};

fn failing_start(name: &'static str) -> Spec {
    Spec {
        fail_start: true,
        ..Spec::from(name)
    }
}

fn failing_stop(name: &'static str) -> Spec {
    Spec {
        fail_stop: true,
        ..Spec::from(name)
    }
}

/// Every record of the run is distinct, there are `expected` of them, and the last one (and only
/// that one) is the terminal record. Each service is called at most once per step, and every service
/// that started is stopped.
fn assert_once(path: &str, run: &Run, expected: usize) {
    assert_eq!(run.records.len(), expected, "{path}: {:?}", run.records);
    let distinct: BTreeSet<&String> = run.records.iter().collect();
    assert_eq!(
        distinct.len(),
        run.records.len(),
        "{path}: a record repeats"
    );
    let terminal = |line: &String| {
        line == "INFO  runtime: shutdown complete"
            || line.starts_with("ERROR runtime: shutdown finished with ")
    };
    assert_eq!(
        run.records.iter().filter(|line| terminal(line)).count(),
        1,
        "{path}"
    );
    assert!(
        terminal(run.records.last().expect("a run writes records")),
        "{path}"
    );

    let distinct_calls: BTreeSet<&String> = run.calls.iter().collect();
    assert_eq!(
        distinct_calls.len(),
        run.calls.len(),
        "{path}: a call repeats"
    );
    let starts = run
        .calls
        .iter()
        .filter(|call| call.starts_with("start "))
        .count();
    let stops = run
        .calls
        .iter()
        .filter(|call| call.starts_with("stop "))
        .count();
    let started = match &run.outcome {
        // The failing service was started once but never counts as started.
        Outcome::StartFailed { .. } => starts - 1,
        Outcome::Clean | Outcome::StopFailed { .. } | Outcome::RunFailed { .. } => starts,
    };
    assert_eq!(
        stops, started,
        "{path}: every started service is stopped once"
    );
}

#[test]
fn every_step_and_the_terminal_record_appear_once_on_every_path() {
    let output = install();

    let clean = output.run(vec![Spec::from("a1"), Spec::from("a2")]);
    assert_once("clean", &clean, 6);

    let first_fails = output.run(vec![failing_start("b1"), Spec::from("b2")]);
    assert_once("first start fails", &first_fails, 2);

    let rollback = output.run(vec![
        Spec::from("c1"),
        failing_stop("c2"),
        failing_start("c3"),
        Spec::from("c4"),
    ]);
    assert_once("start failure with a rollback stop failure", &rollback, 6);

    let stop_failure = output.run(vec![Spec::from("d1"), failing_stop("d2"), Spec::from("d3")]);
    assert_once("stop failure", &stop_failure, 8);

    let empty = output.run(Vec::new());
    assert_once("no services", &empty, 2);
}
