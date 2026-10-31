//! Scenario 5: the middle of three services fails to stop. The remaining one still stops, the failure
//! is logged, and the last record counts it.

mod common;

use ck_runtime::{Failure, Outcome, ServiceError};
use common::{Spec, install};

#[test]
fn a_failing_stop_does_not_prevent_the_rest_from_stopping() {
    let run = install().run(vec![
        Spec::from("a"),
        Spec {
            fail_stop: true,
            ..Spec::from("b")
        },
        Spec::from("c"),
    ]);
    assert_eq!(
        run.outcome,
        Outcome::StopFailed {
            failures: vec![Failure {
                service: "b".to_owned(),
                error: ServiceError::new("b refused to stop"),
            }],
        }
    );
    assert_eq!(
        run.calls,
        [
            "start a", "start b", "start c", "stop c", "stop b", "stop a"
        ]
    );
    assert_eq!(
        run.records,
        [
            "INFO  runtime: started a",
            "INFO  runtime: started b",
            "INFO  runtime: started c",
            "INFO  runtime: start-up complete",
            "INFO  runtime: stopped c",
            "ERROR runtime: b failed to stop: b refused to stop",
            "INFO  runtime: stopped a",
            "ERROR runtime: shutdown finished with 1 stop failure(s)",
        ]
    );
}
