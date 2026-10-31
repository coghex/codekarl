//! Scenario 6: with no services only the two lifecycle records are written, and the outcome is clean.

mod common;

use ck_runtime::Outcome;
use common::install;

#[test]
fn no_services_log_only_start_up_and_shutdown() {
    let run = install().run(Vec::new());
    assert_eq!(run.outcome, Outcome::Clean);
    assert!(run.calls.is_empty());
    assert_eq!(
        run.records,
        [
            "INFO  runtime: start-up complete",
            "INFO  runtime: shutdown complete",
        ]
    );
}
