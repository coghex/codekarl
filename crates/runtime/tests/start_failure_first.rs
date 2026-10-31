//! Scenario 3: the first of three fails to start. None is stopped; the records are exactly the start
//! failure and then `shutdown complete`.

mod common;

use ck_runtime::{Outcome, ServiceError};
use common::{Spec, install};

#[test]
fn a_failed_first_start_stops_nothing() {
    let run = install().run(vec![
        Spec {
            fail_start: true,
            ..Spec::from("a")
        },
        Spec::from("b"),
        Spec::from("c"),
    ]);
    assert_eq!(
        run.outcome,
        Outcome::StartFailed {
            service: "a".to_owned(),
            error: ServiceError::new("a refused to start"),
            stop_failures: Vec::new(),
        }
    );
    assert_eq!(run.calls, ["start a"]);
    assert_eq!(
        run.records,
        [
            "ERROR runtime: a failed to start: a refused to start",
            "INFO  runtime: shutdown complete",
        ]
    );
}
