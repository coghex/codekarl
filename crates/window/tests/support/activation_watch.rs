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
    running_application_activate_calls: u64,
    running_application_activate_from_calls: u64,
    became_active: u64,
    became_key: u64,
    is_active: i32,
    has_key_window: i32,
    is_frontmost: i32,
    expected_request: i32,
    exposed_count: u64,
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
    report_classification(&snapshot_lines(&snapshot), false);
}

pub(crate) fn assert_expected_request_suppressed() {
    let snapshot = read();
    report_classification(&snapshot_lines(&snapshot), true);
}

/// Checks an application worker's sidecar, including workers terminated before their Rust exit checks run.
pub(crate) fn assert_report(path: &Path, target: &str, case: &str, mode: &str) {
    let report = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("activation watch report {}: {error}", path.display()));
    let lines: Vec<String> = report.lines().map(str::to_owned).collect();
    match classify(&lines, false) {
        Ok(Classification {
            external_line: Some(line),
        }) => {
            let forwarded = format!("activation watch external [{target} {case} {mode}]: {line}");
            if case == "external_forwarding_fixture" {
                check_fixture_forwarding(target, case, mode, std::slice::from_ref(&forwarded))
                    .unwrap_or_else(|error| panic!("{error}"));
            }
            eprintln!("{forwarded}");
        }
        Ok(Classification {
            external_line: None,
        }) => {}
        Err(message) => panic!("{message}; activation watch report: {}", lines.join("\n")),
    }
}

fn read() -> Snapshot {
    let mut snapshot = Snapshot::default();
    // SAFETY: `snapshot` has the C layout used by the native test support and is writable.
    unsafe { ck_activation_watch_read(&raw mut snapshot) };
    snapshot
}

fn snapshot_lines(snapshot: &Snapshot) -> Vec<String> {
    let mut lines = vec!["installed=1".to_owned()];
    for (name, count) in [
        ("activate", snapshot.activate_calls),
        (
            "activateIgnoringOtherApps",
            snapshot.activate_ignoring_calls,
        ),
        ("makeKeyAndOrderFront", snapshot.make_key_calls),
        (
            "runningApplicationActivate",
            snapshot.running_application_activate_calls,
        ),
        (
            "runningApplicationActivateFrom",
            snapshot.running_application_activate_from_calls,
        ),
    ] {
        lines.extend(std::iter::repeat_n(
            format!("request={name}"),
            usize::try_from(count).expect("request count fits address space"),
        ));
    }
    lines.extend(std::iter::repeat_n(
        "outcome=becameActive".to_owned(),
        usize::try_from(snapshot.became_active).expect("activation count fits address space"),
    ));
    lines.extend(std::iter::repeat_n(
        "outcome=becameKey".to_owned(),
        usize::try_from(snapshot.became_key).expect("key-window count fits address space"),
    ));
    lines.push(format!(
        "end_active={} end_key={} end_frontmost={}",
        snapshot.is_active, snapshot.has_key_window, snapshot.is_frontmost
    ));
    lines.extend(std::iter::repeat_n(
        "exposed=1".to_owned(),
        usize::try_from(snapshot.exposed_count).expect("exposure count fits address space"),
    ));
    lines
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Classification {
    pub(super) external_line: Option<String>,
}

/// One platform-free rule for both native snapshots and report-file workers.
pub(super) fn classify(lines: &[String], expected_request: bool) -> Result<Classification, String> {
    if !lines.iter().any(|line| line == "installed=1") {
        return Err("activation watch did not install".to_owned());
    }
    let requests: Vec<&str> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("request="))
        .collect();
    let outcomes: Vec<&str> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("outcome="))
        .collect();
    let exposure = lines.iter().find_map(|line| line.strip_prefix("exposed="));
    if let Some(window) = exposure {
        return Err(format!("a window accepted mouse input (exposed={window})"));
    }
    let ends = lines.iter().find(|line| line.starts_with("end_active="));
    let end_state = ends
        .map(|end_line| {
            parse_end_states(end_line)
                .ok_or_else(|| format!("malformed activation end state: {end_line}"))
        })
        .transpose()?;
    if expected_request {
        if !requests
            .iter()
            .any(|name| matches!(*name, "activate" | "activateIgnoringOtherApps"))
        {
            return Err("production open did not request application activation".to_owned());
        }
        if !requests.contains(&"makeKeyAndOrderFront") {
            return Err("production open did not request a key window".to_owned());
        }
        let (active, key, frontmost) =
            end_state.ok_or_else(|| "activation watch omitted its end state".to_owned())?;
        if !outcomes.is_empty() || active != 0 || key != 0 || frontmost != 0 {
            return Err("suppressed request made the process active or a window key".to_owned());
        }
        return Ok(Classification {
            external_line: None,
        });
    }
    if end_state.is_none() && !lines.iter().any(|line| line == "expected_request=0") {
        return Err(
            "activation watch omitted its end state outside an ordinary worker report".to_owned(),
        );
    }
    if !requests.is_empty() {
        return Err(format!(
            "activation request was made: {}",
            requests.join(", ")
        ));
    }
    let (active, key, frontmost) = end_state.unwrap_or((0, 0, 0));
    let external = !outcomes.is_empty() || active != 0 || key != 0 || frontmost != 0;
    let external_line = external.then(|| {
        let pid = lines
            .iter()
            .find_map(|line| line.strip_prefix("pid="))
            .unwrap_or("unknown");
        let count = |name: &str| {
            lines
                .iter()
                .filter(|line| line.as_str() == format!("outcome={name}"))
                .count()
        };
        format_external_summary(
            pid,
            count("becameActive"),
            count("becameKey"),
            active,
            key,
            frontmost,
        )
    });
    Ok(Classification { external_line })
}

pub(super) fn format_external_summary(
    pid: &str,
    became_active: usize,
    became_key: usize,
    end_active: i32,
    end_key: i32,
    end_frontmost: i32,
) -> String {
    format!(
        "ck-activation-external: pid={pid} becameActive={became_active} becameKey={became_key} end_active={end_active} end_key={end_key} end_frontmost={end_frontmost}"
    )
}

fn parse_end_states(line: &str) -> Option<(i32, i32, i32)> {
    let value = |field: &str| {
        line.split_whitespace()
            .find_map(|part| part.strip_prefix(field)?.strip_prefix('='))?
            .parse::<i32>()
            .ok()
    };
    Some((
        value("end_active")?,
        value("end_key")?,
        value("end_frontmost")?,
    ))
}

fn report_classification(lines: &[String], expected_request: bool) {
    match classify(lines, expected_request) {
        Ok(Classification {
            external_line: Some(line),
        }) => eprintln!("{line}"),
        Ok(Classification {
            external_line: None,
        }) => {}
        Err(message) => panic!("{message}; activation watch report: {}", lines.join("\n")),
    }
}

/// Extracts activation and probe notices from already-captured worker output; this never reads a pipe or waits.
pub(super) fn forwarded_external_summaries(
    target: &str,
    case: &str,
    mode: &str,
    text: &str,
) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            if line.starts_with("ck-activation-external:") {
                Some(format!(
                    "activation watch external [{target} {case} {mode}]: {line}"
                ))
            } else if line.starts_with("ck-probe-not-run:") {
                Some(format!(
                    "pass-through probe not run [{target} {case} {mode}]: {line}"
                ))
            } else {
                None
            }
        })
        .collect()
}

pub(super) fn check_fixture_forwarding(
    target: &str,
    case: &str,
    mode: &str,
    lines: &[String],
) -> Result<(), String> {
    let prefix =
        format!("activation watch external [{target} {case} {mode}]: ck-activation-external: pid=");
    if lines.len() == 1 && lines[0].starts_with(&prefix) {
        Ok(())
    } else {
        Err(format!(
            "the capture path forwarded {lines:?}, expected one summary with {prefix:?}"
        ))
    }
}
