//! Scenario 1: three services start in order and stop in reverse; the record sequence is exact.

mod common;

use ck_runtime::Outcome;
use common::{Spec, install};

#[test]
fn three_services_start_in_order_and_stop_in_reverse() {
    let run = install().run(vec![Spec::from("a"), Spec::from("b"), Spec::from("c")]);
    assert_eq!(run.outcome, Outcome::Clean);
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
            "INFO  runtime: stopped b",
            "INFO  runtime: stopped a",
            "INFO  runtime: shutdown complete",
        ]
    );
}
