//! A full log queue before a failure record: rollback and the outcome are unaffected.
//!
//! The same set-up as `full_queue_progress`, but the second service fails to start. Rollback still
//! stops the first service, and the outcome is `StartFailed`, within the same bound. Of the four lifecycle
//! records, the start failure is an error and is reserved by the logger, so it is written once the sink
//! opens; the three progress records (`started a`, `stopped a`, the terminal record) are dropped and
//! counted.

mod held;

use std::time::Duration;

use ck_log::FlushOutcome;
use ck_runtime::{Outcome, ServiceError};
use held::{Spec, install_full};

#[test]
fn a_full_queue_before_a_failure_record_does_not_stall_rollback() {
    let held = install_full();
    let wait = Duration::from_millis(50);
    let run = held.run(
        wait,
        vec![
            Spec {
                name: "a",
                fail_start: false,
            },
            Spec {
                name: "b",
                fail_start: true,
            },
            Spec {
                name: "c",
                fail_start: false,
            },
        ],
    );
    assert_eq!(
        run.outcome,
        Outcome::StartFailed {
            service: "b".to_owned(),
            error: ServiceError::new("b refused to start"),
            stop_failures: Vec::new(),
        }
    );
    assert_eq!(run.calls, ["start a", "start b", "stop a"]);
    assert!(
        run.elapsed >= wait * 4,
        "every record waited its {wait:?}: {:?}",
        run.elapsed
    );
    assert!(
        run.elapsed < Duration::from_secs(1),
        "the run took {:?}",
        run.elapsed
    );

    let (flush, text) = held.finish();
    assert_eq!(
        flush,
        FlushOutcome::Incomplete {
            queued: 0,
            in_flight: false,
            dropped: 3,
            sink_error: None,
        }
    );
    assert_eq!(
        text,
        "INFO  test: first\nERROR runtime: b failed to start: b refused to start\n\
         INFO  test: second\nWARN  log: 3 record(s) dropped\n"
    );
}
