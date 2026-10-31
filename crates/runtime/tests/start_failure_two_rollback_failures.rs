//! Beyond the specified scenarios: two rollback stops fail. Both failures are counted in the terminal
//! record and kept, in rollback order, in the outcome; the service that did not fail still stops.

mod common;

use ck_runtime::{Failure, Outcome, ServiceError};
use common::{Spec, install};

#[test]
fn every_rollback_stop_failure_is_reported_in_order() {
    let run = install().run(vec![
        Spec::from("a"),
        Spec {
            fail_stop: true,
            ..Spec::from("b")
        },
        Spec {
            fail_stop: true,
            ..Spec::from("c")
        },
        Spec {
            fail_start: true,
            ..Spec::from("d")
        },
        Spec::from("e"),
    ]);
    assert_eq!(
        run.outcome,
        Outcome::StartFailed {
            service: "d".to_owned(),
            error: ServiceError::new("d refused to start"),
            stop_failures: vec![
                Failure {
                    service: "c".to_owned(),
                    error: ServiceError::new("c refused to stop"),
                },
                Failure {
                    service: "b".to_owned(),
                    error: ServiceError::new("b refused to stop"),
                },
            ],
        }
    );
    assert_eq!(
        run.calls,
        [
            "start a", "start b", "start c", "start d", "stop c", "stop b", "stop a"
        ]
    );
    assert_eq!(
        run.records,
        [
            "INFO  runtime: started a",
            "INFO  runtime: started b",
            "INFO  runtime: started c",
            "ERROR runtime: d failed to start: d refused to start",
            "ERROR runtime: c failed to stop: c refused to stop",
            "ERROR runtime: b failed to stop: b refused to stop",
            "INFO  runtime: stopped a",
            "ERROR runtime: shutdown finished with 2 stop failure(s)",
        ]
    );
}
