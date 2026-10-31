//! Scenario 2: the second of three fails to start. Only the first is stopped, the third is never
//! started, and the last record is `shutdown complete`.

mod common;

use ck_runtime::{Outcome, ServiceError};
use common::{Spec, install};

#[test]
fn a_failed_second_start_stops_only_the_first() {
    let run = install().run(vec![
        Spec::from("a"),
        Spec {
            fail_start: true,
            ..Spec::from("b")
        },
        Spec::from("c"),
    ]);
    assert_eq!(
        run.outcome,
        Outcome::StartFailed {
            service: "b".to_owned(),
            error: ServiceError::new("b refused to start"),
            stop_failures: Vec::new(),
        }
    );
    assert_eq!(run.calls, ["start a", "start b", "stop a"]);
    assert_eq!(
        run.records,
        [
            "INFO  runtime: started a",
            "ERROR runtime: b failed to start: b refused to start",
            "INFO  runtime: stopped a",
            "INFO  runtime: shutdown complete",
        ]
    );
}
