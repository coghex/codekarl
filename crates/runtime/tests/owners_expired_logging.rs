//! Owners, expired-phase logging: a stalled sink behind a full queue, a log wait of one second and a
//! start deadline miss. Owner-phase records are capped by their phase's deadline, so once it has
//! passed each makes one non-waiting attempt: the run is not charged a log wait per record.

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

use ck_runtime::{Outcome, Runtime, ServiceError};
use fakes::{Cleanup, Fakes, Script, Step};
use held::install_full;

#[test]
fn owner_records_after_the_phase_deadline_do_not_wait_for_the_log() {
    let held = install_full();
    let fakes = Fakes::new();
    let _cleanup = Cleanup(Arc::clone(&fakes));
    let log_wait = Duration::from_secs(1);
    let start = Duration::from_millis(150);
    let cleanup = Duration::from_millis(150);
    let (report, elapsed) = held.run_runtime({
        let fakes = Arc::clone(&fakes);
        move || {
            Runtime::new(Vec::new())
                .with_log_wait(log_wait)
                .with_owners(vec![fakes.spawn(
                    "stuck",
                    Script {
                        ready: Step::Stuck,
                        ..Script::default()
                    },
                )])
                .with_owner_deadlines(start, Duration::from_secs(5), cleanup)
        }
    });
    assert_eq!(
        report.outcome,
        Outcome::StartFailed {
            service: "stuck".to_owned(),
            error: ServiceError::new("missed its start deadline"),
            stop_failures: Vec::new(),
        }
    );
    assert_eq!(report.missed, ["stuck"]);
    assert_eq!(
        report.unfinished,
        ["stuck"],
        "the list is right whatever became of the records"
    );

    // The owner-record path itself: from the moment the readiness wait expired (the deadline passed)
    // to the cancel that follows the `missed its start deadline` record. A fresh one-second wait for
    // that record would show here; one non-waiting attempt does not.
    let expired = fakes.when("spawn stuck") + start;
    let cancelled = fakes.when("cancel stuck");
    assert!(
        cancelled >= expired,
        "the cancel follows the missed deadline"
    );
    assert!(
        cancelled - expired < Duration::from_millis(400),
        "the expired missed-deadline record waited: {:?}",
        cancelled - expired
    );

    // The whole run: the start phase, the cleanup, and the terminal record, which keeps its own log
    // wait (there are no service records here). Each of the two owner records waiting its own second
    // would add two more.
    assert!(elapsed >= start + cleanup, "{elapsed:?}");
    assert!(
        elapsed < start + cleanup + log_wait + Duration::from_millis(700),
        "the run was charged more than the terminal record's log wait: {elapsed:?}"
    );

    // The error records are reserved by the logger (CK-52), so they are written once the sink opens; only
    // the info-level terminal record, which waited its whole log wait, was lost and counted.
    let (flush, text) = held.finish();
    let ck_log::FlushOutcome::Incomplete { dropped, .. } = flush else {
        panic!("a stalled sink behind a full queue must lose a record: {flush:?}");
    };
    assert_eq!(dropped, 1, "only the terminal info record");
    assert!(text.starts_with("INFO  test: first\n"), "{text:?}");
    assert!(
        text.contains("ERROR runtime: stuck missed its start deadline\n")
            && text.contains("ERROR runtime: stuck did not finish\n"),
        "{text:?}"
    );
}
