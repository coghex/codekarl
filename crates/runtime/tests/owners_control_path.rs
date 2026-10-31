//! Owners, control path: every owner-phase record goes through `log_within` only, and owner-phase
//! records are capped by the phase deadline.

const OWNERS: &str = include_str!("../src/owners.rs");

#[test]
fn owner_phase_records_use_log_within_only() {
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
            !OWNERS.contains(forbidden),
            "src/owners.rs contains the waiting or unbounded call `{forbidden}`"
        );
    }
    assert_eq!(
        OWNERS.matches("ck_log::log_within!").count(),
        1,
        "one place emits owner-phase records"
    );
    assert!(
        OWNERS.contains("deadline(self.log_wait).min(until)"),
        "a record's deadline is the earlier of the log wait and the phase deadline"
    );
}

const LIB: &str = include_str!("../src/lib.rs");

#[test]
fn the_runtime_s_own_records_use_log_within_only() {
    // The records of the start phase, the quit path and the run phase are emitted by `lib.rs` through the
    // `record!` macro (`log_within` with the log wait) or by the owner-phase `record`; none waits.
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
            !LIB.contains(forbidden),
            "src/lib.rs contains the waiting or unbounded call `{forbidden}`"
        );
    }
    assert_eq!(
        LIB.matches("ck_log::log_within!").count(),
        1,
        "one macro emits the lifecycle records"
    );
    for text in ["quit requested during start-up", "run failed: {message}"] {
        let at = LIB
            .find(&format!("\"{text}\""))
            .unwrap_or_else(|| panic!("the record `{text}` is in src/lib.rs"));
        let before = &LIB[..at];
        let call = before
            .rfind("record!(")
            .max(before.rfind("supervision.record("));
        assert!(
            call.is_some_and(|call| at - call < 200),
            "`{text}` is not emitted through record! or supervision.record"
        );
    }
}
