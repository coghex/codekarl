//! The `codekarl` runtime command, end to end: the built binary is run with configuration files written
//! to a fresh temporary directory (absolute paths), and its stdout, stderr and exit status are checked.
//!
//! Every child has a deadline and is killed and reaped when it expires, so a regression fails quickly
//! instead of hanging. The tests pass with and without the `metal` and `vulkan` features (the `features:` record
//! follows the build).
//!
//! The helpers for running the binary with a deadline, scratch directories and a full pipe (the only
//! `unsafe`: two minimal `fcntl` calls on a descriptor the test owns) are in `common`.

use std::{
    ffi::OsStr,
    fmt::Write as _,
    os::fd::OwnedFd,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

mod common;

use common::{CHILD_DEADLINE, Run, STALLED_DEADLINE, Scratch, full_pipe, run_in};

const FEATURES: &str = match (cfg!(feature = "metal"), cfg!(feature = "vulkan")) {
    (false, false) => "none",
    (true, false) => "metal",
    (false, true) => "vulkan",
    (true, true) => "metal,vulkan",
};
const VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------------------------
// Running the binary
// ---------------------------------------------------------------------------------------------

/// Runs the binary with `args` and `stdout` (a pipe the test reads when `None`), and kills and reaps it
/// at `deadline`.
fn run_with(args: &[&OsStr], stdout: Option<Stdio>, deadline: Duration) -> Run {
    run_in(None, args, stdout, deadline)
}

fn run(args: &[&OsStr]) -> Run {
    let run = run_with(args, None, CHILD_DEADLINE);
    assert!(!run.timed_out, "the child outlived its deadline: {run:?}");
    run
}

fn config_run(text: &str) -> (PathBuf, Run) {
    let scratch = Scratch::new();
    let path = scratch.file("config.toml", text);
    let result = run(&[OsStr::new("--config"), path.as_os_str()]);
    (path, result)
}

// ---------------------------------------------------------------------------------------------
// The strict output checker
// ---------------------------------------------------------------------------------------------

/// One expected record: padded level token, category and message.
type Record<'a> = (&'a str, &'a str, String);

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        _ => 28,
    }
}

/// Whether `text` is exactly `YYYY-MM-DDTHH:MM:SS.mmmZ` with valid ranges.
fn is_timestamp(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 24 {
        return false;
    }
    let separators = [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'.'),
        (23, b'Z'),
    ];
    if separators.iter().any(|&(at, byte)| bytes[at] != byte) {
        return false;
    }
    let number = |from: usize, to: usize| -> Option<u32> {
        let digits = &text[from..to];
        digits
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| digits.parse().ok())
            .flatten()
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second), Some(_)) = (
        number(0, 4),
        number(5, 7),
        number(8, 10),
        number(11, 13),
        number(14, 16),
        number(17, 19),
        number(20, 23),
    ) else {
        return false;
    };
    (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && hour < 24
        && minute < 60
        && second < 60
}

/// Checks `stdout` line by line: with `timestamps`, each line starts with a valid timestamp and a space;
/// then the level token padded to five characters, a space, the category, `: ` and the message. The
/// records must be exactly `expected`, in order, and the text must end in a newline.
fn check_records(stdout: &str, timestamps: bool, expected: &[Record<'_>]) -> Result<(), String> {
    if !stdout.is_empty() && !stdout.ends_with('\n') {
        return Err(format!("output does not end with a newline: {stdout:?}"));
    }
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.len() != expected.len() {
        return Err(format!(
            "expected {} records, found {}: {lines:?}",
            expected.len(),
            lines.len()
        ));
    }
    for (index, (line, (level, category, message))) in lines.iter().zip(expected).enumerate() {
        let rest = if timestamps {
            let (stamp, rest) = line
                .split_at_checked(24)
                .ok_or_else(|| format!("record {index} is too short: {line:?}"))?;
            if !is_timestamp(stamp) {
                return Err(format!(
                    "record {index} has a malformed timestamp: {line:?}"
                ));
            }
            rest.strip_prefix(' ')
                .ok_or_else(|| format!("record {index}: no space after the timestamp: {line:?}"))?
        } else {
            line
        };
        let wanted = format!("{level} {category}: {message}");
        if rest != wanted {
            return Err(format!("record {index} is {rest:?}, expected {wanted:?}"));
        }
    }
    Ok(())
}

/// The eight records of a run: the start, configuration source, features, window mode, resource-service
/// start/stop, and runtime start-up/shutdown records.
fn run_records(source: &str) -> Vec<Record<'static>> {
    vec![
        ("INFO ", "runtime", format!("codekarl {VERSION} starting")),
        ("INFO ", "config", source.to_owned()),
        ("INFO ", "runtime", format!("features: {FEATURES}")),
        ("INFO ", "runtime", "window: none (headless)".to_owned()),
        ("INFO ", "runtime", "started resources".to_owned()),
        ("INFO ", "runtime", "start-up complete".to_owned()),
        ("INFO ", "runtime", "stopped resources".to_owned()),
        ("INFO ", "runtime", "shutdown complete".to_owned()),
    ]
}

#[test]
fn the_checker_accepts_the_expected_output_and_rejects_every_kind_of_difference() {
    let stamp = "2026-10-19T02:03:04.567Z";
    let mut good = String::new();
    for (level, category, message) in run_records("built-in defaults") {
        writeln!(good, "{stamp} {level} {category}: {message}").expect("a string write");
    }
    let expected = run_records("built-in defaults");
    assert_eq!(check_records(&good, true, &expected), Ok(()));

    // A malformed timestamp: wrong separators, an out-of-range field, a missing field, a wrong length.
    for bad in [
        good.replacen(stamp, "2026-10-19 02:03:04.567Z", 1),
        good.replacen(stamp, "2026-13-19T02:03:04.567Z", 1),
        good.replacen(stamp, "2026-02-30T02:03:04.567Z", 1),
        good.replacen(stamp, "2026-10-19T24:03:04.567Z", 1),
        good.replacen(stamp, "2026-10-19T02:03:04Z", 1),
        good.replacen(stamp, "2026-10-19T02:03:04.5671Z", 1),
        good.replacen(stamp, "2026-10-19T0x:03:04.567Z", 1),
    ] {
        assert!(check_records(&bad, true, &expected).is_err(), "{bad}");
    }
    // A missing record, an extra record, a reordered record and the wrong feature line.
    let lines: Vec<&str> = good.lines().collect();
    let joined = |lines: &[&str]| {
        let mut text = String::new();
        for line in lines {
            writeln!(text, "{line}").expect("a string write");
        }
        text
    };
    assert!(
        check_records(&joined(&lines[1..]), true, &expected).is_err(),
        "missing"
    );
    let mut extra = lines.clone();
    extra.push(lines[0]);
    assert!(
        check_records(&joined(&extra), true, &expected).is_err(),
        "extra"
    );
    let mut reordered = lines.clone();
    reordered.swap(0, 1);
    assert!(
        check_records(&joined(&reordered), true, &expected).is_err(),
        "reordered"
    );
    let other = if FEATURES == "none" { "metal" } else { "none" };
    let wrong = good.replace(
        &format!("features: {FEATURES}"),
        &format!("features: {other}"),
    );
    assert!(
        check_records(&wrong, true, &expected).is_err(),
        "wrong feature line"
    );
    // Other differences: no final newline, a record without the timestamp the run should have.
    assert!(check_records(good.trim_end(), true, &expected).is_err());
    assert!(check_records(&joined(&["INFO  runtime: x"]), true, &expected[..1]).is_err());
}

// ---------------------------------------------------------------------------------------------
// Runs that complete
// ---------------------------------------------------------------------------------------------

#[test]
fn the_default_run_logs_five_timestamped_records_and_exits_cleanly() {
    let run = run(&[]);
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stderr, "");
    assert_eq!(
        check_records(&run.stdout, true, &run_records("built-in defaults")),
        Ok(()),
        "{}",
        run.stdout
    );
}

#[test]
fn an_explicit_configuration_without_timestamps_or_colour_gives_the_exact_text() {
    let (path, run) = config_run("[log]\ntimestamps = false\ncolour = \"never\"\n");
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stderr, "");
    assert_eq!(
        run.stdout,
        format!(
            "INFO  runtime: codekarl {VERSION} starting\nINFO  config: loaded {}\n\
             INFO  runtime: features: {FEATURES}\nINFO  runtime: window: none (headless)\nINFO  runtime: started resources\n\
             INFO  runtime: start-up complete\n\
             INFO  runtime: stopped resources\nINFO  runtime: shutdown complete\n",
            path.display()
        )
    );
}

#[test]
fn an_empty_file_is_the_defaults_with_the_file_named() {
    let (path, run) = config_run("");
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(
        check_records(
            &run.stdout,
            true,
            &run_records(&format!("loaded {}", path.display()))
        ),
        Ok(()),
        "{}",
        run.stdout
    );
}

#[test]
fn the_level_filters_records_and_a_category_level_overrides_it() {
    let (_, quiet) = config_run("[log]\nlevel = \"warn\"\n");
    assert_eq!(quiet.code, Some(0), "{quiet:?}");
    assert_eq!(quiet.stdout, "");
    assert_eq!(quiet.stderr, "");

    let (_, runtime_only) = config_run(
        "[log]\nlevel = \"warn\"\ntimestamps = false\n[log.categories]\nruntime = \"info\"\n",
    );
    assert_eq!(runtime_only.code, Some(0), "{runtime_only:?}");
    assert_eq!(
        runtime_only.stdout,
        format!(
            "INFO  runtime: codekarl {VERSION} starting\nINFO  runtime: features: {FEATURES}\nINFO  runtime: window: none (headless)\n\
             INFO  runtime: started resources\nINFO  runtime: start-up complete\n\
             INFO  runtime: stopped resources\nINFO  runtime: shutdown complete\n"
        ),
        "only the runtime records, not the config record"
    );
}

#[test]
fn colour_always_wraps_the_level_token_in_ck_logs_escape_codes() {
    let (path, run) = config_run("[log]\ntimestamps = false\ncolour = \"always\"\n");
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(
        run.stdout,
        format!(
            "\x1b[32mINFO\x1b[0m  runtime: codekarl {VERSION} starting\n\
             \x1b[32mINFO\x1b[0m  config: loaded {}\n\
             \x1b[32mINFO\x1b[0m  runtime: features: {FEATURES}\n\
             \x1b[32mINFO\x1b[0m  runtime: window: none (headless)\n\
             \x1b[32mINFO\x1b[0m  runtime: started resources\n\
             \x1b[32mINFO\x1b[0m  runtime: start-up complete\n\
             \x1b[32mINFO\x1b[0m  runtime: stopped resources\n\
             \x1b[32mINFO\x1b[0m  runtime: shutdown complete\n",
            path.display()
        )
    );
}

// ---------------------------------------------------------------------------------------------
// Configuration errors: exit 1 with exactly one `ERROR config:` record and nothing on stderr
// ---------------------------------------------------------------------------------------------

#[test]
fn every_configuration_error_gives_exit_1_and_one_config_record() {
    let cases: [(&str, &str, &[&str]); 9] = [
        ("invalid TOML", "[log]\nlevel = = 3\n", &["line 2"]),
        (
            "unknown top-level key",
            "verbose = true\n",
            &["line 1", "verbose"],
        ),
        (
            "unknown [log] key",
            "[log]\nlouder = true\n",
            &["line 2", "louder"],
        ),
        (
            "invalid level",
            "[log]\nlevel = \"loud\"\n",
            &["line 2", "loud"],
        ),
        (
            "invalid colour",
            "[log]\ncolour = \"bright\"\n",
            &["line 2", "bright"],
        ),
        (
            "invalid category name",
            "[log.categories]\n\"bad name\" = \"info\"\n",
            &["bad name"],
        ),
        (
            "an empty array for log",
            "log = []\n",
            &["line 1", "`log`", "table"],
        ),
        (
            "a single-item array for log",
            "log = [\"warn\"]\n",
            &["line 1", "`log`", "table"],
        ),
        (
            "a multi-item array for log",
            "\nlog = [\"info\", false, \"never\"]\n",
            &["line 2", "`log`", "table"],
        ),
    ];
    for (name, text, expected) in cases {
        let (path, run) = config_run(text);
        assert_eq!(run.code, Some(1), "{name}: {run:?}");
        assert_eq!(run.stderr, "", "{name}");
        let lines: Vec<&str> = run.stdout.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "{name}: exactly one record: {:?}",
            run.stdout
        );
        assert!(run.stdout.ends_with('\n'), "{name}");
        let message = lines[0]
            .strip_prefix("ERROR config: ")
            .unwrap_or_else(|| panic!("{name}: not an ERROR config record: {:?}", run.stdout));
        assert!(
            message.contains(&path.display().to_string()),
            "{name}: {message}"
        );
        for part in expected {
            assert!(message.contains(part), "{name}: {message:?} lacks {part:?}");
        }
    }
}

#[test]
fn a_missing_file_is_reported_with_the_os_error() {
    let run = run(&[
        OsStr::new("--config"),
        OsStr::new("/nonexistent/codekarl-test/missing.toml"),
    ]);
    assert_eq!(run.code, Some(1), "{run:?}");
    assert_eq!(
        run.stdout,
        "ERROR config: cannot read /nonexistent/codekarl-test/missing.toml: \
         No such file or directory (os error 2)\n"
    );
    assert_eq!(run.stderr, "");
}

// ---------------------------------------------------------------------------------------------
// A stalled or closed stdout: exit 3 within the deadline
// ---------------------------------------------------------------------------------------------

#[test]
fn a_stalled_stdout_ends_the_process_with_status_3_within_the_deadline() {
    let (reader, writer) = full_pipe();
    // The read end stays open (and unread) until the child is gone.
    let run = run_with(&[], Some(Stdio::from(writer)), STALLED_DEADLINE);
    drop(reader);
    assert!(
        !run.timed_out,
        "the child did not exit within {STALLED_DEADLINE:?} and was killed: {run:?}"
    );
    assert_eq!(run.code, Some(3), "{run:?}");
    assert_eq!(run.stderr, "", "no further diagnostic is attempted");
}

#[test]
fn a_closed_stdout_ends_the_process_with_status_3_without_a_panic() {
    let (reader, writer) = common::pipe_locked(&common::spawn_guard()).expect("a pipe");
    drop(reader); // the read end is closed before the child starts
    let run = run_with(
        &[],
        Some(Stdio::from(OwnedFd::from(writer))),
        STALLED_DEADLINE,
    );
    assert!(!run.timed_out, "{run:?}");
    assert_eq!(run.code, Some(3), "{run:?}");
    assert!(!run.stderr.contains("panicked"), "{run:?}");
}

struct HeldChild {
    child: Child,
    release: Option<ChildStdin>,
}

impl HeldChild {
    fn release_and_reap(&mut self) -> Result<(), String> {
        self.release.take();
        let deadline = Instant::now() + STALLED_DEADLINE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => {}
                Err(error) => return Err(format!("could not poll the holder: {error}")),
            }
            if Instant::now() >= deadline {
                self.child
                    .kill()
                    .map_err(|error| format!("could not kill the holder: {error}"))?;
                let reap_deadline = Instant::now() + STALLED_DEADLINE;
                loop {
                    match self.child.try_wait() {
                        Ok(Some(_)) => {
                            return Err("the holder exceeded its release bound".to_owned());
                        }
                        Ok(None) if Instant::now() < reap_deadline => thread::yield_now(),
                        Ok(None) => {
                            return Err(
                                "the killed holder was not reaped within its bound".to_owned()
                            );
                        }
                        Err(error) => return Err(format!("could not reap the holder: {error}")),
                    }
                }
            }
            thread::yield_now();
        }
    }
}

impl Drop for HeldChild {
    fn drop(&mut self) {
        let _ = self.release_and_reap();
    }
}

fn hold_until_released(guard: &common::SpawnGuard) -> HeldChild {
    let mut command = Command::new("sh");
    command
        .args(["-c", "read -r _"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = common::spawn_locked(guard, &mut command).expect("the holder starts");
    let release = child.stdin.take().expect("the holder has a release pipe");
    HeldChild {
        child,
        release: Some(release),
    }
}

fn run_with_pipe_reader(expose_to_exec: bool) -> (Run, usize) {
    let scratch = Scratch::new();
    let config = scratch.file(
        "config.toml",
        "[log]\ntimestamps = false\ncolour = \"never\"\n",
    );
    let expected = format!(
        "INFO  runtime: codekarl {VERSION} starting\nINFO  config: loaded {}\n\
         INFO  runtime: features: {FEATURES}\nINFO  runtime: window: none (headless)\n\
         INFO  runtime: start-up complete\nINFO  runtime: shutdown complete\n",
        config.display()
    );
    let guard = common::spawn_guard();
    let (reader, writer) = common::pipe_locked(&guard).expect("the test pipe is created");
    let reader = OwnedFd::from(reader);
    let writer = OwnedFd::from(writer);
    if expose_to_exec {
        common::set_cloexec(&reader, false);
    }
    let mut holder = hold_until_released(&guard);
    drop(reader);
    drop(guard);

    let run = run_with(
        &[OsStr::new("--config"), config.as_os_str()],
        Some(Stdio::from(writer)),
        STALLED_DEADLINE,
    );
    holder
        .release_and_reap()
        .expect("the holder exits when its stdin is closed");
    (run, expected.len())
}

#[test]
fn a_child_cannot_inherit_an_exposed_pipe_from_a_parallel_test() {
    let (exposed, exposed_bytes) = run_with_pipe_reader(true);
    assert!(!exposed.timed_out, "{exposed:?}");
    assert_eq!(
        exposed.code,
        Some(0),
        "the exposed child accepts {exposed_bytes} bytes: {exposed:?}"
    );

    let (kept, _) = run_with_pipe_reader(false);
    assert!(!kept.timed_out, "{kept:?}");
    assert_eq!(
        kept.code,
        Some(3),
        "the close-on-exec control closes the last reader: {kept:?}"
    );
    eprintln!("the exposed child writes {exposed_bytes} bytes before exiting");
}

fn spawn_bypassing_the_guard_for_the_negative_control(stdout: Stdio) -> Child {
    let mut command = common::command_for(None, &[], Some(stdout));
    command.spawn().expect("the negative-control child starts")
}

fn spawn_decision(spawned: &mpsc::Receiver<()>, target: thread::ThreadId) -> Result<bool, String> {
    let deadline = Instant::now() + STALLED_DEADLINE;
    loop {
        match spawned.try_recv() {
            Ok(()) => return Ok(true),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err("the target ended without its post-spawn acknowledgement".to_owned());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if common::is_waiting(target) {
            return Ok(false);
        }
        if Instant::now() >= deadline {
            return Err(
                "the target neither waited for the guard nor spawned within the bound".to_owned(),
            );
        }
        thread::yield_now();
    }
}

fn prove_guard_decision_detects_bypass(bypass: bool) -> (bool, Run) {
    let guard = common::spawn_guard();
    let (reader, writer) = common::pipe_locked(&guard).expect("the proof pipe is created");
    let reader = OwnedFd::from(reader);
    let writer = OwnedFd::from(writer);
    common::set_cloexec(&reader, false);

    let unrelated = thread::spawn(|| drop(common::spawn_guard()));
    let unrelated_id = unrelated.thread().id();
    let wait_deadline = Instant::now() + STALLED_DEADLINE;
    while !common::is_waiting(unrelated_id) && Instant::now() < wait_deadline {
        thread::yield_now();
    }
    let unrelated_waiting = common::is_waiting(unrelated_id);

    let (spawned_tx, spawned_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let target_thread = thread::spawn(move || {
        let run = if bypass {
            let child = spawn_bypassing_the_guard_for_the_negative_control(Stdio::from(writer));
            spawned_tx
                .send(())
                .expect("the post-spawn acknowledgement is received");
            common::finish_child(child, false, STALLED_DEADLINE)
        } else {
            common::run_in_with_spawn_notice(
                None,
                &[],
                Some(Stdio::from(writer)),
                STALLED_DEADLINE,
                || {
                    spawned_tx
                        .send(())
                        .expect("the post-spawn acknowledgement is received");
                },
            )
        };
        result_tx.send(run).expect("the child result is received");
    });
    let target_id = target_thread.thread().id();
    let decision = spawn_decision(&spawned_rx, target_id);
    drop(reader);
    drop(guard);

    let run = result_rx
        .recv_timeout(STALLED_DEADLINE + CHILD_DEADLINE)
        .expect("the target child finishes within its bound");
    unrelated.join().expect("the unrelated waiter exits");
    target_thread.join().expect("the target thread exits");
    assert!(unrelated_waiting, "the unrelated waiter was not registered");
    let bypass_detected = decision.expect("the target reaches a guard decision");
    (bypass_detected, run)
}

#[test]
fn the_guard_proof_names_the_target_waiter_and_detects_a_bypass() {
    let (guarded_bypass, guarded_run) = prove_guard_decision_detects_bypass(false);
    assert!(
        !guarded_bypass,
        "the guarded target waited until the exposed end closed"
    );
    assert_eq!(guarded_run.code, Some(3), "{guarded_run:?}");

    let (negative_bypass, negative_run) = prove_guard_decision_detects_bypass(true);
    assert!(
        negative_bypass,
        "the negative control spawned while the guard was held"
    );
    assert_eq!(
        negative_run.code,
        Some(0),
        "the child inherited the exposed reader: {negative_run:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// The control-path rule, structurally
// ---------------------------------------------------------------------------------------------

#[test]
fn the_main_thread_and_the_panic_hook_never_call_the_waiting_log_macros() {
    let sources = [
        ("main.rs", include_str!("../src/main.rs")),
        ("lifecycle.rs", include_str!("../src/lifecycle.rs")),
        ("config.rs", include_str!("../src/config.rs")),
        ("cli.rs", include_str!("../src/cli.rs")),
    ];
    for (name, source) in sources {
        for forbidden in [
            "ck_log::error!",
            "ck_log::warn!",
            "ck_log::info!",
            "ck_log::debug!",
            "ck_log::trace!",
            "ck_log::log_global",
            ".log(",
        ] {
            assert!(
                !source.contains(forbidden),
                "{name} contains the waiting call `{forbidden}`"
            );
        }
    }
    assert!(include_str!("../src/lifecycle.rs").contains("ck_log::log_within!"));
}

#[test]
fn a_table_is_still_accepted_as_an_inline_table() {
    let (path, run) = config_run("log = { timestamps = false, colour = \"never\" }\n");
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stderr, "");
    assert_eq!(
        run.stdout,
        format!(
            "INFO  runtime: codekarl {VERSION} starting\nINFO  config: loaded {}\n\
             INFO  runtime: features: {FEATURES}\nINFO  runtime: window: none (headless)\nINFO  runtime: started resources\n\
             INFO  runtime: start-up complete\n\
             INFO  runtime: stopped resources\nINFO  runtime: shutdown complete\n",
            path.display()
        )
    );
}

// ---------------------------------------------------------------------------------------------
// `--config` followed by a command flag
// ---------------------------------------------------------------------------------------------

const VALID_CONFIG: &str = "[log]\ntimestamps = false\ncolour = \"never\"\n";
const UNKNOWN_ARGUMENTS: &str = "codekarl: unknown arguments; use --help\n";

#[test]
fn a_command_flag_after_config_is_unknown_arguments_even_if_a_file_of_that_name_exists() {
    let scratch = Scratch::new();
    // Valid configuration files literally named like the flags, in the working directory.
    for flag in ["--help", "--version", "--metal-smoke"] {
        scratch.file(flag, VALID_CONFIG);
    }
    for flag in ["--help", "--version", "--metal-smoke"] {
        let run = run_in(
            Some(&scratch.0),
            &[OsStr::new("--config"), OsStr::new(flag)],
            None,
            CHILD_DEADLINE,
        );
        assert!(!run.timed_out, "{flag}: {run:?}");
        assert_eq!(run.code, Some(1), "{flag}: {run:?}");
        assert_eq!(
            run.stdout, UNKNOWN_ARGUMENTS,
            "{flag}: nothing was loaded or started"
        );
        assert_eq!(run.stderr, "", "{flag}");
    }
}

#[test]
fn an_explicit_path_that_looks_like_a_flag_is_still_a_path() {
    let scratch = Scratch::new();
    scratch.file("--version", VALID_CONFIG);
    let run = run_in(
        Some(&scratch.0),
        &[OsStr::new("--config"), OsStr::new("./--version")],
        None,
        CHILD_DEADLINE,
    );
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stderr, "");
    assert_eq!(
        run.stdout,
        format!(
            "INFO  runtime: codekarl {VERSION} starting\nINFO  config: loaded ./--version\n\
             INFO  runtime: features: {FEATURES}\nINFO  runtime: window: none (headless)\nINFO  runtime: started resources\n\
             INFO  runtime: start-up complete\n\
             INFO  runtime: stopped resources\nINFO  runtime: shutdown complete\n"
        )
    );
}
