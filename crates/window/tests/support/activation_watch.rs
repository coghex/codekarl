#![expect(
    dead_code,
    reason = "shared by several test targets with different watch modes"
)]

use std::ffi::CString;
use std::fs;
use std::path::Path;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct Snapshot {
    activate_calls: u64,
    activate_ignoring_calls: u64,
    make_key_calls: u64,
    became_active: u64,
    became_key: u64,
    is_active: i32,
    has_key_window: i32,
    is_frontmost: i32,
    expected_request: i32,
}

unsafe extern "C" {
    fn ck_activation_watch_set_report_path(path: *const std::ffi::c_char);
    fn ck_activation_watch_install(expected_request: i32);
    fn ck_activation_watch_read(out: *mut Snapshot);
}

pub(crate) fn install(expected_request: bool) {
    // SAFETY: clears the optional report path for this worker process.
    unsafe { ck_activation_watch_set_report_path(std::ptr::null()) };
    install_inner(expected_request);
}

pub(crate) fn install_report(path: &Path, expected_request: bool) {
    let path = CString::new(path.to_string_lossy().as_bytes()).expect("report path has no NUL");
    // SAFETY: `path` is NUL-terminated and remains alive through the native copy.
    unsafe { ck_activation_watch_set_report_path(path.as_ptr()) };
    install_inner(expected_request);
}

fn install_inner(expected_request: bool) {
    // SAFETY: the native watch is process-local, test-only, installed before any window is created,
    // and records/suppresses activation requests until `assert_*` reads its final snapshot.
    unsafe { ck_activation_watch_install(i32::from(expected_request)) };
}

pub(crate) fn assert_clean() {
    let snapshot = read();
    assert_eq!(
        snapshot.expected_request, 0,
        "ordinary activation watch mode"
    );
    assert_eq!(
        snapshot.activate_calls, 0,
        "NSApplication activate was requested"
    );
    assert_eq!(
        snapshot.activate_ignoring_calls, 0,
        "NSApplication activateIgnoringOtherApps: was requested"
    );
    assert_eq!(
        snapshot.make_key_calls, 0,
        "NSWindow makeKeyAndOrderFront: was requested"
    );
    assert_eq!(snapshot.became_active, 0, "the test process became active");
    assert_eq!(snapshot.became_key, 0, "a test window became key");
    assert_eq!(
        snapshot.is_active, 0,
        "the test process is active at the end"
    );
    assert_eq!(
        snapshot.has_key_window, 0,
        "a test window is key at the end"
    );
    assert_eq!(
        snapshot.is_frontmost, 0,
        "the test process is frontmost at the end"
    );
}

pub(crate) fn assert_expected_request_suppressed() {
    let snapshot = read();
    assert_eq!(snapshot.expected_request, 1, "expected-request watch mode");
    assert!(
        snapshot.activate_calls + snapshot.activate_ignoring_calls > 0,
        "production open did not request application activation"
    );
    assert!(
        snapshot.make_key_calls > 0,
        "production open did not request a key window"
    );
    assert_eq!(
        snapshot.became_active, 0,
        "suppressed request made the process active"
    );
    assert_eq!(
        snapshot.became_key, 0,
        "suppressed request made a window key"
    );
    assert_eq!(
        snapshot.is_active, 0,
        "the test process is active at the end"
    );
    assert_eq!(
        snapshot.has_key_window, 0,
        "a test window is key at the end"
    );
    assert_eq!(
        snapshot.is_frontmost, 0,
        "the test process is frontmost at the end"
    );
}

/// Checks the sidecar left by an application worker, including workers terminated before their Rust exit checks run.
pub(crate) fn assert_report(path: &Path) {
    let report = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("activation watch report {}: {error}", path.display()));
    assert!(
        report.lines().any(|line| line == "installed=1"),
        "activation watch did not install: {report}"
    );
    assert!(
        !report.lines().any(|line| line.starts_with("request=")),
        "activation request was made: {report}"
    );
    assert!(
        !report.lines().any(|line| line.starts_with("outcome=")),
        "activation outcome was observed: {report}"
    );
    if report.lines().any(|line| line.starts_with("end_")) {
        assert!(
            report
                .lines()
                .any(|line| line == "end_active=0 end_key=0 end_frontmost=0"),
            "activation watch ended in an active or key state: {report}"
        );
    }
}

fn read() -> Snapshot {
    let mut snapshot = Snapshot::default();
    // SAFETY: `snapshot` has the C layout used by the native test support and is writable.
    unsafe { ck_activation_watch_read(&raw mut snapshot) };
    snapshot
}
