//! The runtime's source makes no waiting `ck_log` call: every record goes through `log_within`.
//!
//! A structural check on `src/lib.rs`: the waiting macros and the waiting `log` method are the
//! calls that could stall the lifecycle behind a full queue.

use std::time::Duration;

use ck_runtime::Runtime;

const SOURCE: &str = include_str!("../src/lib.rs");

#[test]
fn no_waiting_ck_log_call_remains_in_the_runtime_source() {
    for forbidden in [
        "ck_log::error!",
        "ck_log::warn!",
        "ck_log::info!",
        "ck_log::debug!",
        "ck_log::trace!",
        "ck_log::log_global",
        ".log(",
        "ck_log::try_log",
    ] {
        assert!(
            !SOURCE.contains(forbidden),
            "src/lib.rs contains the waiting or unbounded call `{forbidden}`"
        );
    }
    assert!(SOURCE.contains("ck_log::log_within!"));
}

#[test]
fn the_default_log_wait_is_100_ms_and_can_be_changed() {
    assert_eq!(
        Runtime::new(Vec::new()).log_wait(),
        Duration::from_millis(100)
    );
    assert_eq!(
        Runtime::new(Vec::new())
            .with_log_wait(Duration::from_millis(7))
            .log_wait(),
        Duration::from_millis(7)
    );
}
