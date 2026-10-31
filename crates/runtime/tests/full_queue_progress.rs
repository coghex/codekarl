//! A full log queue before a progress record: the lifecycle still finishes, in order, quickly.
//!
//! The logger has a capacity of 1 and a held sink, so its queue is full and nothing drains. A run of
//! three services with a 50 ms log wait must still start and stop every service in order and return
//! `Clean`, in a bound well under one second; each of its eight lifecycle records waits out its 50 ms
//! and is then dropped and counted by the logger, which shows only in the logger's flush result.

mod held;

use std::time::Duration;

use ck_log::FlushOutcome;
use ck_runtime::Outcome;
use held::{Spec, install_full};

#[test]
fn a_full_queue_before_every_progress_record_does_not_stall_the_lifecycle() {
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
                fail_start: false,
            },
            Spec {
                name: "c",
                fail_start: false,
            },
        ],
    );
    assert_eq!(run.outcome, Outcome::Clean);
    assert_eq!(
        run.calls,
        [
            "start a", "start b", "start c", "stop c", "stop b", "stop a"
        ]
    );
    // Eight records (three started, start-up complete, three stopped, the terminal record), each
    // given its full wait, none admitted: about 400 ms, and far from waiting for space.
    assert!(
        run.elapsed >= wait * 8,
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
            dropped: 8,
            sink_error: None,
        }
    );
    // Only the two records that filled the logger, and its report of the eight that were dropped.
    assert_eq!(
        text,
        "INFO  test: first\nINFO  test: second\nWARN  log: 8 record(s) dropped\n"
    );
}
