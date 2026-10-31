//! Owners, failed stop request under a full log queue: every sibling's request is sent before any log
//! admission that might wait on a failure, so a failing request cannot use up a healthy sibling's stop
//! window.

#[expect(
    dead_code,
    reason = "this scenario uses the held-sink fixture's runtime runner, not its service fakes"
)]
mod held;

#[expect(
    dead_code,
    reason = "this scenario uses the fakes' spawn and call log, not every script option"
)]
mod fakes;

use std::sync::Arc;
use std::time::Duration;

use ck_runtime::{Failure, Outcome, Runtime, ServiceError};
use fakes::{Cleanup, Fakes, Script};
use held::install_full;

#[test]
fn a_failing_stop_request_does_not_delay_the_healthy_siblings_request() {
    let held = install_full();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let stop = Duration::from_millis(100);
    let (report, _elapsed) = held.run_runtime({
        let fakes = Arc::clone(&fakes);
        move || {
            // Reverse order: `reject` is asked first and fails, then `healthy`.
            Runtime::new(Vec::new())
                .with_log_wait(Duration::from_millis(400))
                .with_owners(vec![
                    fakes.spawn("healthy", Script::default()),
                    fakes.spawn(
                        "reject",
                        Script {
                            request_stop_fails: Some("rejected"),
                            ..Script::default()
                        },
                    ),
                ])
                .with_owner_deadlines(Duration::from_secs(5), stop, Duration::from_secs(1))
        }
    });
    // The full queue made logging the failure wait out the stop phase; the healthy request must have
    // gone out first.
    let gap = fakes
        .when("request_stop healthy")
        .duration_since(fakes.when("request_stop reject"));
    assert!(
        gap < Duration::from_millis(50),
        "the healthy sibling's request waited {gap:?} behind the failure's log record"
    );
    assert_eq!(
        report.outcome,
        Outcome::StopFailed {
            failures: vec![Failure {
                service: "reject".to_owned(),
                error: ServiceError::new("rejected"),
            }],
        }
    );
    assert!(
        report.missed.is_empty(),
        "the healthy sibling was not made late by delayed dispatch: {:?}",
        report.missed
    );
    assert!(report.unfinished.is_empty());
    let _ = held.finish();
}
