//! The native GPU probe under Metal validation, run as a supervisor and a worker from this one
//! executable (`harness = false`, so its results appear in plain `cargo test` output).
//!
//! The supervisor starts this executable again as the worker, in its own process group and with
//! `MTL_DEBUG_LAYER=1` and `MTL_SHADER_VALIDATION=1` in the worker's environment from the first
//! instant, so no Metal object exists before they apply. The worker asserts both variables, only then
//! creates a `Device`, runs the probe and checks all 64 values. The supervisor gives the worker 30 s:
//! on expiry it kills the worker's process group, reaps it and fails the case, in every mode.
//!
//! The worker exits 0 on success, 77 when no GPU is available, 2 when a validation variable is
//! missing and 1 for anything else. `CK_REQUIRE_FULL_SPEC=1` turns "GPU unavailable" from a visible
//! skip into a failure; a timeout, a missing variable and a wrong value fail in every mode. Two
//! always-run fixtures check the supervisor itself (a blocked worker is killed and leaves no process
//! behind; a worker without the variables stops before any device exists).
//!
//! Every wait on the worker is bounded: if the worker cannot be killed (a kill that is denied, a
//! missing kill command) the supervisor gives up after `REAP_GRACE` per attempt and fails the case,
//! naming the surviving worker, instead of waiting for the outer step watchdog. The worker is owned
//! (and so cleaned up on every path, including a panic) from the instant it is spawned.
//!
//! `CK_GPU_TEST_DEADLINE_MS` shortens the deadline and `CK_GPU_TEST_BLOCK=1` makes the worker block
//! before creating a device. Both are test-only and ignored unless explicitly set, as are the flags
//! the supervisor's own fixtures use to start this executable as a nested supervisor.

use std::{
    env,
    ffi::OsString,
    fs,
    io::Read,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    process::{Child, Command, ExitCode, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use std::os::unix::process::CommandExt;

use ck_gpu_metal::Device;

const WORKER_FLAG: &str = "--worker";
/// Fixture-only: run the blocking worker under a supervisor and print how it ended.
const NESTED_FLAG: &str = "--nested-blocked";
/// Fixture-only: where a supervisor writes the worker's process group as soon as it is known.
const GROUP_FILE_VARIABLE: &str = "CK_GPU_TEST_GROUP_FILE";
const FULL_SPEC_VARIABLE: &str = "CK_REQUIRE_FULL_SPEC";
const DEADLINE_VARIABLE: &str = "CK_GPU_TEST_DEADLINE_MS";
const BLOCK_VARIABLE: &str = "CK_GPU_TEST_BLOCK";
const KILL_VARIABLE: &str = "CK_GPU_TEST_KILL";
/// How long the supervisor waits for output pipes and for an emptied group after a kill.
const PIPE_GRACE: Duration = Duration::from_secs(2);
/// How long the supervisor waits for the worker to be reaped after each attempt to kill it.
const REAP_GRACE: Duration = Duration::from_secs(1);
const LAYERS: [&str; 2] = ["MTL_DEBUG_LAYER", "MTL_SHADER_VALIDATION"];
const DEADLINE: Duration = Duration::from_secs(30);
const EXIT_FAILED: u8 = 1;
const EXIT_LAYERS_MISSING: u8 = 2;
const EXIT_GPU_UNAVAILABLE: u8 = 77;
/// What the native layer reports when no Metal device exists (`native/bridge.m`).
const NO_DEVICE: &str = "No Metal device is available";
const CASE: &str = "native GPU probe under Metal validation";

/// Why the worker did not succeed.
enum Failure {
    Unavailable(String),
    Other(String),
}

fn run_probe() -> Result<(), Failure> {
    let mut device = Device::new().map_err(|error| {
        let message = error.to_string();
        if message == NO_DEVICE {
            Failure::Unavailable(message)
        } else {
            Failure::Other(message)
        }
    })?;
    let values = device
        .probe()
        .map_err(|error| Failure::Other(error.to_string()))?;
    for (index, actual) in values.iter().enumerate() {
        let expected =
            u32::try_from(index).map_err(|error| Failure::Other(error.to_string()))? * 3 + 7;
        if *actual != expected {
            return Err(Failure::Other(format!(
                "GPU result {index}: expected {expected}, got {actual}"
            )));
        }
    }
    println!("worker: {} GPU results verified", values.len());
    Ok(())
}

/// Blocks for ever, leaving a child in this process group so a group kill has something to reap.
fn block() -> ! {
    let _child = Command::new("sleep").arg("300").spawn();
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

fn worker() -> ExitCode {
    for variable in LAYERS {
        if env::var(variable).as_deref() != Ok("1") {
            eprintln!("worker: {variable} is not 1; stopping before any device exists");
            return ExitCode::from(EXIT_LAYERS_MISSING);
        }
    }
    if env::var(BLOCK_VARIABLE).as_deref() == Ok("1") {
        block();
    }
    match run_probe() {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Unavailable(message)) => {
            eprintln!("worker: {message}");
            ExitCode::from(EXIT_GPU_UNAVAILABLE)
        }
        Err(Failure::Other(message)) => {
            eprintln!("worker: {message}");
            ExitCode::from(EXIT_FAILED)
        }
    }
}

/// How the supervised worker ended.
#[derive(Debug)]
enum Outcome {
    Exited {
        code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    /// The deadline passed; the worker's group was killed and is empty, and the worker is reaped.
    TimedOut { group: u32 },
    /// The deadline passed (or a descendant held the pipes) and the group could not be emptied. The
    /// worker itself is reaped; `detail` says what survived and why.
    Unterminated { group: u32, detail: String },
}

/// How the process group is terminated; `CK_GPU_TEST_KILL` selects a fault for the supervisor's own
/// proofs (`primary-broken`: the first command is missing, so the fallback must work; `none`: every
/// command fails). Ignored unless explicitly set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Termination {
    Normal,
    PrimaryBroken,
    Disabled,
}

impl Termination {
    fn from_env() -> Result<Self, String> {
        match env::var_os(KILL_VARIABLE) {
            None => Ok(Self::Normal),
            Some(value) => match value.to_str() {
                Some("primary-broken") => Ok(Self::PrimaryBroken),
                Some("none") => Ok(Self::Disabled),
                _ => Err(format!(
                    "{KILL_VARIABLE} must be primary-broken or none, not {}",
                    value.to_string_lossy()
                )),
            },
        }
    }
}

/// Runs a command and waits for it at most `PIPE_GRACE`; a command that does not finish is killed.
fn status_of(command: &mut Command) -> Result<(), String> {
    let name = command.get_program().display().to_string();
    let mut child = command
        .spawn()
        .map_err(|error| format!("{name} could not run: {error}"))?;
    let expiry = Instant::now() + PIPE_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{name} exited with {status}")),
            Ok(None) if Instant::now() < expiry => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.try_wait(); // reap if it is gone; never wait on it
                return Err(format!("{name} did not finish within {PIPE_GRACE:?}"));
            }
            Err(error) => return Err(format!("{name} could not be waited for: {error}")),
        }
    }
}

/// Kills every process in the group (the worker is its leader): `/bin/kill`, then the shell's own
/// `kill` if that fails. The result says whether either worked.
fn kill_group(group: u32, termination: Termination) -> Result<(), String> {
    if termination == Termination::Disabled {
        return Err("group termination is disabled by the test fault".to_owned());
    }
    let primary = if termination == Termination::PrimaryBroken {
        "/nonexistent/kill"
    } else {
        "/bin/kill"
    };
    let first = status_of(Command::new(primary).args(["-KILL", "--", &format!("-{group}")]));
    let Err(first) = first else {
        return Ok(());
    };
    status_of(Command::new("/bin/sh").args(["-c", "kill -KILL -- \"-$0\"", &group.to_string()]))
        .map_err(|second| format!("{first}; then {second}"))
}

/// The ids of the processes still in the group.
fn group_members(group: u32) -> Vec<u32> {
    let listing = Command::new("/bin/ps")
        .args(["-axo", "pid=,pgid="])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace().map(str::parse::<u32>);
            match (fields.next(), fields.next()) {
                (Some(Ok(member)), Some(Ok(leader))) if leader == group => Some(member),
                _ => None,
            }
        })
        .collect()
}

/// Reads `stream` to its end on a thread of its own. Starting the thread can fail (and `fail` makes
/// it, for the fixture that proves the worker is already owned when that happens).
fn drain<R: Read + Send + 'static>(
    stream: Option<R>,
    fail: bool,
) -> std::io::Result<mpsc::Receiver<String>> {
    if fail {
        return Err(std::io::Error::other(
            "injected: a drain thread cannot start",
        ));
    }
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new().spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stream) = stream {
            // A read error leaves what arrived so far, which is what the report should show.
            let _ = stream.read_to_end(&mut bytes);
        }
        // The supervisor may have stopped waiting; nothing is left to tell then.
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    })?;
    Ok(receiver)
}

/// Waits at most `grace` for the worker to be reaped; `Ok(false)` if it is still running.
fn reaped_within(child: &mut Child, grace: Duration) -> std::io::Result<bool> {
    let expiry = Instant::now() + grace;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        if Instant::now() >= expiry {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// The worker while the supervisor owns it, from the instant it is spawned. If the supervisor
/// returns for any reason with the worker not reaped (an error, a panic), dropping this kills the
/// group and the worker and reaps it, waiting at most `REAP_GRACE` per attempt.
struct Owned {
    child: Child,
    group: u32,
    termination: Termination,
    /// Reaped, or deliberately abandoned after a reported failure to end it: nothing left to do.
    settled: bool,
    /// What the most recent group kill said.
    group_kill: Result<(), String>,
}

impl Owned {
    /// One attempt to end the worker: its group, then the worker itself (a denied or failed direct
    /// kill is reported, not ignored), then a bounded wait for it to be reaped. `Ok` means it was
    /// reaped; otherwise what went wrong.
    fn end(&mut self) -> Result<(), String> {
        let group = kill_group(self.group, self.termination);
        self.group_kill.clone_from(&group);
        let direct = match self.child.kill() {
            // Already exited: not a failure to end it.
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
            Err(error) => Err(error.to_string()),
        };
        match reaped_within(&mut self.child, REAP_GRACE) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "worker {} is still running after the kill attempts (group kill {group:?}, direct kill {direct:?})",
                self.group
            )),
            Err(error) => Err(format!(
                "worker {} could not be waited for: {error}",
                self.group
            )),
        }
    }

    /// Ends the worker with one retry (the second attempt escalates after the first had its chance).
    fn end_with_retry(&mut self) -> Result<(), String> {
        let first = self.end();
        let result = if first.is_ok() { first } else { self.end() };
        if result.is_ok() {
            self.settled = true;
        }
        result
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.settled
            && let Err(message) = self.end_with_retry()
        {
            // A destructor cannot return it; say so where the step's output is read.
            eprintln!("gpu_probe: cleanup failed: {message}");
        }
    }
}

/// Waits (bounded) for the group to be empty.
fn group_emptied(group: u32) -> Result<(), Vec<u32>> {
    let expiry = Instant::now() + PIPE_GRACE;
    loop {
        let members = group_members(group);
        if members.is_empty() {
            return Ok(());
        }
        if Instant::now() >= expiry {
            return Err(members);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Ends the group after a deadline: kill it, reap the worker within a bound, then insist that the
/// group is empty, within a bound. A worker that cannot be ended is a reported failure naming it.
fn end_after_deadline(owned: &mut Owned) -> Outcome {
    let group = owned.group;
    if let Err(survivor) = owned.end_with_retry() {
        owned.settled = true; // reported here; the destructor must not try again
        return Outcome::Unterminated {
            group,
            detail: survivor,
        };
    }
    match (owned.group_kill.clone(), group_emptied(group)) {
        (Ok(()), Ok(())) => Outcome::TimedOut { group },
        (killed, emptied) => Outcome::Unterminated {
            group,
            detail: format!(
                "group {group} not emptied: kill result {killed:?}, survivors {:?}",
                emptied.err().unwrap_or_default()
            ),
        },
    }
}

/// What a supervised run needs; the defaults run a normal worker.
struct Setup<'a> {
    layers: bool,
    extra: &'a [(&'a str, &'a str)],
    deadline: Duration,
    termination: Termination,
    /// Fixture-only: starting the drain threads fails.
    drain_fails: bool,
    /// Fixture-only: the file the worker's group is written to once it is known.
    group_file: Option<&'a Path>,
}

impl Setup<'_> {
    fn new(deadline: Duration, termination: Termination) -> Self {
        Self {
            layers: true,
            extra: &[],
            deadline,
            termination,
            drain_fails: false,
            group_file: None,
        }
    }
}

/// Starts this executable as the worker, in its own process group, and waits at most the deadline.
/// The worker is owned from the instant it exists: every path after the spawn, including a failure
/// to start a drain thread, ends it, and every wait on it and on its output is bounded.
fn supervise(setup: &Setup) -> std::io::Result<Outcome> {
    let Setup {
        layers,
        extra,
        deadline,
        termination,
        drain_fails,
        group_file,
    } = *setup;
    let mut command = Command::new(env::current_exe()?);
    command
        .arg(WORKER_FLAG)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for variable in LAYERS {
        if layers {
            command.env(variable, "1");
        } else {
            command.env_remove(variable);
        }
    }
    command.envs(extra.iter().copied());
    let child = command.spawn()?;
    let group = child.id();
    let mut owned = Owned {
        child,
        group,
        termination,
        settled: false,
        group_kill: Ok(()),
    };
    if let Some(file) = group_file {
        fs::write(file, group.to_string())?;
    }
    let stdout = drain(owned.child.stdout.take(), drain_fails)?;
    let stderr = drain(owned.child.stderr.take(), drain_fails)?;
    let expiry = Instant::now() + deadline;
    loop {
        if let Some(status) = owned.child.try_wait()? {
            owned.settled = true;
            // The pipes close when every member of the group is gone; a descendant that kept them
            // open must not hang the supervisor.
            let (Ok(stdout), Ok(stderr)) = (
                stdout.recv_timeout(PIPE_GRACE),
                stderr.recv_timeout(PIPE_GRACE),
            ) else {
                let killed = kill_group(group, termination);
                return Ok(Outcome::Unterminated {
                    group,
                    detail: format!(
                        "a descendant of the worker kept its output pipes open (kill result {killed:?})"
                    ),
                });
            };
            return Ok(Outcome::Exited {
                code: status.code(),
                stdout,
                stderr,
            });
        }
        if Instant::now() >= expiry {
            return Ok(end_after_deadline(&mut owned));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Normal,
    Full,
}

/// Reads `CK_REQUIRE_FULL_SPEC` as the OS holds it: absent is normal mode, a value that is not valid
/// Unicode or is anything but `0` or `1` is an error naming the variable.
fn mode(value: Option<&OsString>) -> Result<Mode, String> {
    match value.map(|value| value.to_str()) {
        None | Some(Some("0")) => Ok(Mode::Normal),
        Some(Some("1")) => Ok(Mode::Full),
        Some(Some(other)) => Err(format!(
            "{FULL_SPEC_VARIABLE} must be unset, 0 or 1, not {other:?}"
        )),
        Some(None) => Err(format!(
            "{FULL_SPEC_VARIABLE} is not valid Unicode: {value:?}"
        )),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Passed,
    Skipped(String),
    Failed(String),
}

fn judge(outcome: &Outcome, mode: Mode, deadline: Duration) -> Verdict {
    match outcome {
        Outcome::TimedOut { .. } => Verdict::Failed(format!(
            "{CASE} timed out after {} s and the worker's process group was killed",
            deadline.as_secs_f64()
        )),
        Outcome::Unterminated { detail, .. } => Verdict::Failed(format!(
            "{CASE} did not finish and its process group could not be ended: {detail}"
        )),
        Outcome::Exited {
            code,
            stdout,
            stderr,
        } => match code {
            Some(0) => Verdict::Passed,
            Some(code) if *code == i32::from(EXIT_GPU_UNAVAILABLE) => {
                let why = format!("GPU unavailable: {}", stderr.trim());
                if mode == Mode::Full {
                    Verdict::Failed(why)
                } else {
                    Verdict::Skipped(why)
                }
            }
            other => Verdict::Failed(format!(
                "worker ended with {other:?}; stdout {stdout:?}, stderr {stderr:?}"
            )),
        },
    }
}

fn deadline() -> Result<Duration, String> {
    match env::var(DEADLINE_VARIABLE) {
        Err(env::VarError::NotPresent) => Ok(DEADLINE),
        Ok(value) => value
            .parse::<u64>()
            .map(Duration::from_millis)
            .map_err(|_| {
                format!("{DEADLINE_VARIABLE} must be a number of milliseconds, not {value:?}")
            }),
        Err(error) => Err(format!("{DEADLINE_VARIABLE}: {error}")),
    }
}

/// A fixture check of the supervisor itself: `Ok` when it behaves, else what went wrong.
type Check = fn() -> Result<(), String>;

/// A blocked worker is killed at the deadline, reaped, reported as timed out and fails in both modes.
fn check_timeout_kills_the_group() -> Result<(), String> {
    let short = Duration::from_millis(500);
    timed_out_with(Termination::Normal, short)
}

/// Runs the blocking worker to a timeout with the given termination commands and checks the result:
/// named timeout, group empty, a failure in both modes.
fn timed_out_with(termination: Termination, short: Duration) -> Result<(), String> {
    let outcome = supervise(&Setup {
        extra: &[(BLOCK_VARIABLE, "1")],
        ..Setup::new(short, termination)
    })
    .map_err(|error| error.to_string())?;
    let Outcome::TimedOut { group } = outcome else {
        return Err(format!("expected a timeout, got {outcome:?}"));
    };
    let expiry = Instant::now() + Duration::from_secs(5);
    while !group_members(group).is_empty() {
        if Instant::now() >= expiry {
            return Err(format!(
                "processes survive in group {group}: {:?}",
                group_members(group)
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
    for mode in [Mode::Normal, Mode::Full] {
        match judge(&outcome, mode, short) {
            Verdict::Failed(message) if message.contains("timed out after") => {}
            other => {
                return Err(format!(
                    "a timeout must fail in {mode:?} mode, got {other:?}"
                ));
            }
        }
    }
    Ok(())
}

/// With the first termination command missing, the fallback still ends the group.
fn check_fallback_termination_works() -> Result<(), String> {
    timed_out_with(Termination::PrimaryBroken, Duration::from_millis(500))
}

/// When no termination command works, the supervisor still reaps the worker, gives up within a bound
/// and reports a failure naming the survivors; the fixture then kills them itself.
fn check_unkillable_group_is_bounded() -> Result<(), String> {
    let started = Instant::now();
    let outcome = supervise(&Setup {
        extra: &[(BLOCK_VARIABLE, "1")],
        ..Setup::new(Duration::from_millis(500), Termination::Disabled)
    })
    .map_err(|error| error.to_string())?;
    let elapsed = started.elapsed();
    let Outcome::Unterminated { group, detail } = &outcome else {
        return Err(format!("expected an unterminated group, got {outcome:?}"));
    };
    let survivors = group_members(*group);
    // Clean up with a working command whatever the checks below find.
    let cleaned = kill_group(*group, Termination::Normal);
    let emptied = group_emptied(*group);
    if elapsed > Duration::from_millis(500) + PIPE_GRACE * 2 {
        return Err(format!("the supervisor was not bounded: {elapsed:?}"));
    }
    if survivors.is_empty() || !detail.contains("not emptied") {
        return Err(format!("survivors {survivors:?}, detail {detail:?}"));
    }
    if cleaned.is_err() || emptied.is_err() {
        return Err(format!(
            "the fixture could not clean up: {cleaned:?}, {emptied:?}"
        ));
    }
    match judge(&outcome, Mode::Normal, DEADLINE) {
        Verdict::Failed(_) => Ok(()),
        other => Err(format!("an unterminated group must fail, got {other:?}")),
    }
}

/// A native interposer for the fixtures: inside the process that loads it, `kill(pid > 0, SIGKILL)`
/// fails with EPERM, as a security product that denies the signal would. Process-group and
/// SIGTERM kills are untouched.
const DENY_KILL_C: &str = r#"
#include <errno.h>
#include <signal.h>
#include <sys/types.h>
static int denied_kill(pid_t pid, int sig) {
    if (pid > 0 && sig == SIGKILL) { errno = EPERM; return -1; }
    return kill(pid, sig);
}
__attribute__((used)) static struct { const void *replacement; const void *original; } interposed
    __attribute__((section("__DATA,__interpose"))) = { (const void *)denied_kill, (const void *)kill };
"#;

/// A scratch directory removed when it goes out of scope.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Result<Self, String> {
        let path = env::temp_dir().join(format!("ck-gpu-probe-{}-{label}", std::process::id()));
        fs::create_dir(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0); // a leftover scratch directory is harmless
    }
}

fn compile_interposer(directory: &Path) -> Result<PathBuf, String> {
    let source = directory.join("deny_kill.c");
    let library = directory.join("deny_kill.dylib");
    fs::write(&source, DENY_KILL_C).map_err(|error| error.to_string())?;
    let output = Command::new("/usr/bin/cc")
        .args(["-dynamiclib", "-o"])
        .arg(&library)
        .arg(&source)
        .output()
        .map_err(|error| format!("cc could not run: {error}"))?;
    if output.status.success() {
        Ok(library)
    } else {
        Err(format!(
            "the interposer did not compile: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// The nested supervisor a fixture starts: the blocking worker under the deadline given by the
/// environment, with the kill fault the environment selects; it prints how the worker ended.
fn nested() -> ExitCode {
    let (deadline, termination) = match (deadline(), Termination::from_env()) {
        (Ok(deadline), Ok(termination)) => (deadline, termination),
        (Err(message), _) | (_, Err(message)) => {
            eprintln!("gpu_probe: {message}");
            return ExitCode::FAILURE;
        }
    };
    let file = env::var_os(GROUP_FILE_VARIABLE).map(PathBuf::from);
    let setup = Setup {
        extra: &[(BLOCK_VARIABLE, "1")],
        group_file: file.as_deref(),
        ..Setup::new(deadline, termination)
    };
    match supervise(&setup) {
        Ok(outcome) => {
            println!("{outcome:?}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            println!("supervisor error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// How long a nested supervisor may take in a fixture before it counts as unbounded.
const NESTED_BOUND: Duration = Duration::from_secs(6);

/// Runs the blocking worker under a nested supervisor whose direct `SIGKILL` of the worker is denied
/// (EPERM) by a native interposer, with the given group termination. The nested supervisor must end
/// within `NESTED_BOUND` and say `expected`; the fixture then removes any survivor itself.
fn denied_direct_kill(
    termination: Termination,
    expected: &[&str],
    survives: bool,
) -> Result<(), String> {
    let scratch = Scratch::new("deny")?;
    let library = compile_interposer(&scratch.0)?;
    let group_file = scratch.0.join("group");
    let mut command = Command::new(env::current_exe().map_err(|error| error.to_string())?);
    command
        .arg(NESTED_FLAG)
        .env("DYLD_INSERT_LIBRARIES", &library)
        .env(DEADLINE_VARIABLE, "500")
        .env(GROUP_FILE_VARIABLE, &group_file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if termination == Termination::Disabled {
        command.env(KILL_VARIABLE, "none");
    } else {
        command.env_remove(KILL_VARIABLE);
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = drain(child.stdout.take(), false).map_err(|error| error.to_string())?;
    let started = Instant::now();
    let bounded = reaped_within(&mut child, NESTED_BOUND).map_err(|error| error.to_string())?;
    let elapsed = started.elapsed();
    if !bounded {
        let _ = child.kill();
        let _ = child.try_wait();
    }
    let report = stdout.recv_timeout(PIPE_GRACE).unwrap_or_default();
    // Whatever happened, find the worker's group and remove every survivor ourselves.
    let group = fs::read_to_string(&group_file)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .ok_or_else(|| {
            format!("the nested supervisor never started a worker; it said {report:?}")
        })?;
    let survivors = group_members(group);
    let cleaned = if survivors.is_empty() {
        Ok(()) // nothing left to remove
    } else {
        kill_group(group, Termination::Normal)
    };
    let emptied = group_emptied(group);
    if !bounded {
        return Err(format!(
            "the supervisor was still waiting after {elapsed:?} with the direct kill denied"
        ));
    }
    if cleaned.is_err() || emptied.is_err() {
        return Err(format!(
            "the fixture could not clean up: {cleaned:?}, {emptied:?}"
        ));
    }
    for fragment in expected {
        if !report.contains(fragment) {
            return Err(format!("expected {fragment:?} in {report:?}"));
        }
    }
    if survivors.contains(&group) != survives {
        return Err(format!(
            "worker {group} should {}have survived; members {survivors:?}",
            if survives { "" } else { "not " }
        ));
    }
    Ok(())
}

/// The direct kill of the worker is denied and no group kill works: the worker stays alive, and the
/// supervisor must give up within a bound with a failure naming that worker, not wait past the
/// deadline for ever.
fn check_denied_direct_kill_is_bounded() -> Result<(), String> {
    denied_direct_kill(
        Termination::Disabled,
        &["Unterminated", "is still running after the kill attempts"],
        true,
    )
}

/// The direct kill is denied but the group kill works: the worker is ended and reaped anyway.
fn check_group_kill_covers_a_denied_direct_kill() -> Result<(), String> {
    denied_direct_kill(Termination::Normal, &["TimedOut"], false)
}

/// Starting a drain thread fails right after the worker was spawned: the error is returned and the
/// worker is already owned, so it is ended instead of being left running.
fn check_failure_after_spawn_leaves_no_worker() -> Result<(), String> {
    let scratch = Scratch::new("drain")?;
    let group_file = scratch.0.join("group");
    let result = supervise(&Setup {
        extra: &[(BLOCK_VARIABLE, "1")],
        drain_fails: true,
        group_file: Some(&group_file),
        ..Setup::new(Duration::from_secs(30), Termination::Normal)
    });
    let group = fs::read_to_string(&group_file)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .ok_or("the worker was never started")?;
    let expiry = Instant::now() + Duration::from_secs(5);
    let mut members = group_members(group);
    while !members.is_empty() && Instant::now() < expiry {
        thread::sleep(Duration::from_millis(20));
        members = group_members(group);
    }
    if !members.is_empty() {
        let _ = kill_group(group, Termination::Normal); // do not leave it behind
        return Err(format!(
            "the worker's group survives the failure: {members:?}"
        ));
    }
    match result {
        Err(error) if error.to_string().contains("injected") => Ok(()),
        other => Err(format!("expected the injected error, got {other:?}")),
    }
}

/// A `CK_REQUIRE_FULL_SPEC` value that is not valid Unicode stops this executable at once, naming the
/// variable, instead of selecting normal mode.
fn check_undecodable_value_is_rejected() -> Result<(), String> {
    let output = Command::new(env::current_exe().map_err(|error| error.to_string())?)
        .env(
            FULL_SPEC_VARIABLE,
            OsString::from_vec(vec![0x66, 0xff, 0x6f]),
        )
        .output()
        .map_err(|error| error.to_string())?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() || !stderr.contains(FULL_SPEC_VARIABLE) || !output.stdout.is_empty()
    {
        return Err(format!(
            "expected an immediate failure naming the variable; got {:?}, stdout {:?}, stderr {stderr:?}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout)
        ));
    }
    Ok(())
}

/// Without the validation layers the worker stops before any device exists.
fn check_layers_are_required() -> Result<(), String> {
    let outcome = supervise(&Setup {
        layers: false,
        ..Setup::new(Duration::from_secs(10), Termination::Normal)
    })
    .map_err(|error| error.to_string())?;
    let Outcome::Exited { code, stderr, .. } = &outcome else {
        return Err(format!("expected an exit, got {outcome:?}"));
    };
    if *code != Some(i32::from(EXIT_LAYERS_MISSING)) || !stderr.contains("before any device exists")
    {
        return Err(format!("worker without the layers: {outcome:?}"));
    }
    match judge(&outcome, Mode::Normal, DEADLINE) {
        Verdict::Failed(_) => Ok(()),
        other => Err(format!("a missing layer must fail, got {other:?}")),
    }
}

/// How each worker status is judged in each mode.
fn check_verdicts() -> Result<(), String> {
    let exited = |code| Outcome::Exited {
        code: Some(code),
        stdout: String::new(),
        stderr: "no GPU".to_owned(),
    };
    let table = [
        (0, Verdict::Passed, Verdict::Passed),
        (
            i32::from(EXIT_GPU_UNAVAILABLE),
            Verdict::Skipped("GPU unavailable: no GPU".to_owned()),
            Verdict::Failed("GPU unavailable: no GPU".to_owned()),
        ),
    ];
    for (code, normal, full) in table {
        let outcome = exited(code);
        if judge(&outcome, Mode::Normal, DEADLINE) != normal
            || judge(&outcome, Mode::Full, DEADLINE) != full
        {
            return Err(format!("status {code} is judged wrongly"));
        }
    }
    for code in [1, 2, 3, 139] {
        for mode in [Mode::Normal, Mode::Full] {
            if !matches!(judge(&exited(code), mode, DEADLINE), Verdict::Failed(_)) {
                return Err(format!("status {code} must fail in {mode:?} mode"));
            }
        }
    }
    let unterminated = Outcome::Unterminated {
        group: 1,
        detail: "survivors".to_owned(),
    };
    for mode in [Mode::Normal, Mode::Full] {
        if !matches!(judge(&unterminated, mode, DEADLINE), Verdict::Failed(_)) {
            return Err(format!("an unended group must fail in {mode:?} mode"));
        }
    }
    let signalled = Outcome::Exited {
        code: None,
        stdout: String::new(),
        stderr: String::new(),
    };
    if !matches!(
        judge(&signalled, Mode::Normal, DEADLINE),
        Verdict::Failed(_)
    ) {
        return Err("a crash must fail".to_owned());
    }
    Ok(())
}

fn supervisor() -> ExitCode {
    let (mode, deadline, termination) = match (
        mode(env::var_os(FULL_SPEC_VARIABLE).as_ref()),
        deadline(),
        Termination::from_env(),
    ) {
        (Ok(mode), Ok(deadline), Ok(termination)) => (mode, deadline, termination),
        (Err(message), _, _) | (_, Err(message), _) | (_, _, Err(message)) => {
            eprintln!("gpu_probe: {message}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "gpu_probe: {CASE} ({mode:?} mode, deadline {} s)",
        deadline.as_secs_f64()
    );
    let mut failures = 0_u32;
    let fixtures: [(&str, Check); 9] = [
        (
            "supervisor kills and reaps a blocked worker (fixture)",
            check_timeout_kills_the_group,
        ),
        (
            "supervisor ends the group with the fallback when the first kill command is missing (fixture)",
            check_fallback_termination_works,
        ),
        (
            "supervisor gives up within a bound when no kill command works (fixture)",
            check_unkillable_group_is_bounded,
        ),
        (
            "supervisor gives up within a bound when the worker cannot be killed (native fixture)",
            check_denied_direct_kill_is_bounded,
        ),
        (
            "supervisor ends the group when only the direct kill of the worker is denied (native fixture)",
            check_group_kill_covers_a_denied_direct_kill,
        ),
        (
            "supervisor ends the worker when a drain thread cannot start (fixture)",
            check_failure_after_spawn_leaves_no_worker,
        ),
        (
            "full-spec variable that is not valid Unicode is rejected (this executable)",
            check_undecodable_value_is_rejected,
        ),
        (
            "worker refuses to run without the validation layers (fixture)",
            check_layers_are_required,
        ),
        (
            "worker statuses are judged correctly (fixture)",
            check_verdicts,
        ),
    ];
    for (name, check) in fixtures {
        match check() {
            Ok(()) => println!("test {name} ... ok"),
            Err(message) => {
                failures += 1;
                println!("test {name} ... FAILED ({message})");
            }
        }
    }
    match supervise(&Setup::new(deadline, termination)) {
        Ok(outcome) => match judge(&outcome, mode, deadline) {
            Verdict::Passed => println!("test {CASE} ... ok"),
            Verdict::Skipped(why) => println!("test {CASE} ... skipped ({why})"),
            Verdict::Failed(why) => {
                failures += 1;
                println!("test {CASE} ... FAILED ({why})");
            }
        },
        Err(error) => {
            failures += 1;
            println!("test {CASE} ... FAILED (could not run the worker: {error})");
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        println!("gpu_probe: {failures} problem(s)");
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    match env::args().nth(1).as_deref() {
        Some(WORKER_FLAG) => worker(),
        Some(NESTED_FLAG) => nested(),
        _ => supervisor(),
    }
}
