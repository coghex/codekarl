//! Scenario 4: with four services the third fails to start, and during rollback the second fails to
//! stop. The first still stops, the last record counts the stop failure, and the outcome names both
//! services.

mod common;

use ck_runtime::{Failure, Outcome, ServiceError};
use common::{Spec, install};

#[test]
fn a_start_failure_and_a_rollback_stop_failure_are_both_reported() {
    let run = install().run(vec![
        Spec::from("a"),
        Spec {
            fail_stop: true,
            ..Spec::from("b")
        },
        Spec {
            fail_start: true,
            ..Spec::from("c")
        },
        Spec::from("d"),
    ]);
    assert_eq!(
        run.outcome,
        Outcome::StartFailed {
            service: "c".to_owned(),
            error: ServiceError::new("c refused to start"),
            stop_failures: vec![Failure {
                service: "b".to_owned(),
                error: ServiceError::new("b refused to stop"),
            }],
        }
    );
    assert_eq!(
        run.calls,
        ["start a", "start b", "start c", "stop b", "stop a"]
    );
    assert_eq!(
        run.records,
        [
            "INFO  runtime: started a",
            "INFO  runtime: started b",
            "ERROR runtime: c failed to start: c refused to start",
            "ERROR runtime: b failed to stop: b refused to stop",
            "INFO  runtime: stopped a",
            "ERROR runtime: shutdown finished with 1 stop failure(s)",
        ]
    );
}
