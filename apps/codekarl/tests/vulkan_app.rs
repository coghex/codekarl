//! `codekarl` with a real GLFW window and Vulkan, end to end, run **in process** behind a test-only keyboard filter
//! (builds with the `vulkan` feature on macOS).
//!
//! The cases need a desktop session, the Vulkan loader and a GPU (`MoltenVK` on macOS): they **fail rather than skip**
//! without them (owner).
//!
//! **The program under test is the library's `main_entry`, not the shipped binary.** GLFW needs the process's main
//! thread on macOS, so this executable is a **supervisor and a worker** (`harness = false`), like `windowed.rs` is for
//! `native-metal` (CK-131's pattern). The supervisor starts this executable again as the worker for each case, with a
//! whole-process deadline, kills and reaps it on every path (also when a case panics), and checks its output and exit
//! status. The worker recognises its own flag (`--ck-vulkan-test-worker`) in **its** `main`, takes the arguments after
//! its own, and calls `codekarl::main_entry` with them: the application never sees the worker flag, and the worker
//! process **is** the application plus the filter. (`windowed.rs` proves a worker and the binary give byte-identical
//! output for the same arguments; both go through the same `main_entry`.)
//!
//! **The worker's order matters** (CK-131's): the application installs its signal mask on the main thread before any
//! other thread exists, so every later thread inherits blocked SIGINT and SIGTERM. The worker therefore
//! 1. blocks SIGINT and SIGTERM on the main thread, before any framework call, filter, typist or other thread;
//! 2. installs the keyboard filter;
//! 3. in a typing run, starts the typist;
//! 4. starts the reporter thread (what the filter dropped) and, when the case asks for one, the window driver, then
//!    calls `main_entry`.
//!
//! **The owner may be typing while this runs, Escape included.** The filter (`tests/native/testing.m`, linked only into
//! the integration tests) drops every unmarked keyboard event before it reaches a window, so the cases need no quiet
//! keyboard. Every case runs twice: on a quiet keyboard and with a thread typing without stopping. A close request from
//! a key reaching the application fails a case (except the demo's, which closes by a **marked** Escape on purpose),
//! and in a typing run a case that opened a window waits until the filter has dropped keys. The fixture `ui` scripts
//! register an `on_close_request` that **declines** and logs the source.
//!
//! **The window driver** is a worker thread for the cases that need the window itself touched, through three test-only
//! helpers that act on the process's one GLFW window on the main queue (zero or several windows is an error they return,
//! never a guess): `go` posts a marked Escape once the supervisor wrote the go file; `restore` miniaturizes the window,
//! waits until it **reads** miniaturized, writes the "minimised" marker, deminiaturizes it, waits until it reads not
//! miniaturized and visible, writes the "restored" marker; `hold` does the first half only. Requesting a transition, or a
//! fixed delay, is never an acknowledgement; each wait is bounded at 5 s and a miss ends the run with the state it last
//! read.
//!
//! **No dynamic-loader variable is needed**: the program loads the Vulkan loader itself and hands its
//! `vkGetInstanceProcAddr` to GLFW, so every `DYLD_*` and `LD_LIBRARY_PATH` variable is removed from every worker's
//! environment here, and the Vulkan validation layer is enabled (the loader's own `VK_INSTANCE_LAYERS`), so a validation
//! message anywhere in the output fails a case. The path's failure-boundary cases (fakes, drop observers, refused
//! destroys) are in `src/vulkan/tests.rs`.
//!
//! The supervisor also proves the filter's symbols are in the test-only archive and in neither the `codekarl` binary nor
//! the library's artifact. The production binary has no test hook: the scripts leave files with Lua's standard `io.open`.

#![cfg(all(feature = "vulkan", target_os = "macos"))]

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    mem::MaybeUninit,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
    thread,
    time::{Duration, Instant},
};

#[expect(
    dead_code,
    reason = "this file uses the scratch directory and the binary path, not the runner or the full pipe"
)]
mod common;

use common::{BINARY, Scratch};

/// The output rule for validation messages, shared with the library's tests over the fake stack.
mod validation_rule;

#[expect(
    dead_code,
    reason = "this file reads its worker's output as one text; the raw split capture is the windowed test's"
)]
#[path = "supervisor/mod.rs"]
mod supervisor;
use supervisor::{Seen, Supervised};
#[path = "../../../crates/window/tests/support/activation_watch.rs"]
mod activation_watch;
#[path = "supervisor/signal.rs"]
mod signal;

/// The longest a window worker may run.
const DEADLINE: Duration = Duration::from_secs(60);

/// The worker's own flag: the first argument of a worker, followed by the mode, the report file and then the
/// application's arguments.
const WORKER_FLAG: &str = "--ck-vulkan-test-worker";
/// Internal worker argument, consumed here rather than passed to the application.
const ALLOW_UNFOCUSED_UPDATES_FLAG: &str = "--ck-vulkan-test-allow-unfocused-updates";
const QUEUE_FOCUS_LOST_FOR_CK163_FLAG: &str = "--ck-vulkan-test-queue-focus-lost-for-ck163";
const REAL_FOCUS_GATE_CASES: [&str; 1] =
    ["synthetic_focus_loss_without_worker_opt_in_keeps_the_pause"];

/// Which window driver a worker runs (`go`, `restore` or `hold`), and the directory it reads its markers from and writes
/// its own to. Set by the supervisor in the worker's environment.
const DRIVER_ENV: &str = "CK_VULKAN_TEST_DRIVER";
const DRIVER_DIR_ENV: &str = "CK_VULKAN_TEST_DIR";

unsafe extern "C" {
    fn ck_app_t_install_input_filter();
    fn ck_app_t_dropped_keys() -> u64;
    fn ck_app_t_start_typist();
    fn ck_app_t_post_key(
        key_code: u16,
        modifier_flags: u64,
        repeat: i32,
        characters: *const std::ffi::c_char,
        marked: i32,
    ) -> i32;
    fn ck_app_t_miniaturize() -> i32;
    fn ck_app_t_deminiaturize() -> i32;
    fn ck_app_t_window_state() -> i32;
}

/// What a worker runs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The filter, no typing.
    Quiet,
    /// The filter, and a thread typing without stopping.
    Typing,
}

impl Mode {
    const fn word(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Typing => "typing",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        [Self::Quiet, Self::Typing]
            .into_iter()
            .find(|mode| mode.word() == word)
    }

    const fn typing(self) -> bool {
        matches!(self, Self::Typing)
    }

    const fn code(self) -> u8 {
        self as u8
    }

    fn from_code(code: u8) -> Self {
        [Self::Quiet, Self::Typing][usize::from(code)]
    }
}

/// The mode the supervisor is running its cases in.
static MODE: AtomicU8 = AtomicU8::new(0);
static ALLOW_UNFOCUSED_UPDATES: AtomicBool = AtomicBool::new(false);
static QUEUE_FOCUS_LOST_FOR_CK163: AtomicBool = AtomicBool::new(false);

fn mode() -> Mode {
    Mode::from_code(MODE.load(Ordering::SeqCst))
}

// ---------------------------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------------------------

/// Step 1: blocks SIGINT and SIGTERM on the calling (main) thread, so every thread created afterwards inherits them.
fn block_signals() {
    let mut set = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: `sigemptyset` initialises the set it is given.
    let emptied = unsafe { libc::sigemptyset(set.as_mut_ptr()) };
    assert_eq!(emptied, 0, "sigemptyset failed");
    // SAFETY: `set` was initialised above and SIGINT is a valid signal number.
    let added = unsafe { libc::sigaddset(set.as_mut_ptr(), libc::SIGINT) };
    assert_eq!(added, 0, "sigaddset failed");
    // SAFETY: `set` was initialised above and SIGTERM is a valid signal number.
    let added = unsafe { libc::sigaddset(set.as_mut_ptr(), libc::SIGTERM) };
    assert_eq!(added, 0, "sigaddset failed");
    // SAFETY: blocks the signals of `set` on this thread only; the old mask is not wanted.
    let blocked =
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, set.as_ptr(), std::ptr::null_mut()) };
    assert_eq!(blocked, 0, "pthread_sigmask failed");
}

/// The keys the filter has dropped so far.
fn dropped_keys() -> u64 {
    // SAFETY: reads an atomic counter.
    unsafe { ck_app_t_dropped_keys() }
}

/// The reporter thread: publishes the filter's drop count to `path` (written whole and renamed, so a reader never sees half
/// a line), every 20 ms, until the process ends.
fn spawn_reporter(path: PathBuf) {
    thread::spawn(move || {
        let temporary = path.with_extension("tmp");
        loop {
            if fs::write(&temporary, format!("dropped={}\n", dropped_keys())).is_ok() {
                let _ = fs::rename(&temporary, &path);
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
}

/// The reporter's latest drop count, if it has written one.
fn read_dropped(path: &Path) -> Option<u64> {
    fs::read_to_string(path)
        .ok()?
        .trim()
        .strip_prefix("dropped=")?
        .parse()
        .ok()
}

/// The longest a driver step waits for a marker or for the window's state.
const STEP: Duration = Duration::from_secs(5);

/// Waits for `path` to exist, for at most `within`.
fn wait_for_file(path: &Path, within: Duration) -> Result<(), String> {
    let give_up = Instant::now() + within;
    while !path.exists() {
        if Instant::now() >= give_up {
            return Err(format!("{} never appeared", path.display()));
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

/// The window's actual state now (see `ck_app_t_window_state`).
fn window_state() -> i32 {
    // SAFETY: a read on the main queue of the one GLFW window; the helper takes no pointer.
    unsafe { ck_app_t_window_state() }
}

/// Leave the latest driver transition in its per-run directory so a supervisor timeout can distinguish a
/// miniaturize call that did not return from a returned request whose window state did not change.
fn record_driver_stage(dir: &Path, stage: &str) -> Result<(), String> {
    fs::write(dir.join("driver_stage"), stage)
        .map_err(|error| format!("cannot record driver stage: {error}"))
}

/// Waits until the window's state satisfies `reached`, for at most [`STEP`] in all: a reading counts as an acknowledgement only if
/// it **finished** by the deadline (each read is itself bounded at one second by the helper, which gives an error rather than a
/// late answer), so a read begun near the deadline cannot acknowledge a state later. The failure names the state last read.
fn wait_for_state(dir: &Path, what: &str, reached: impl Fn(i32) -> bool) -> Result<(), String> {
    let give_up = Instant::now() + STEP;
    let mut last_state = None;
    loop {
        let state = window_state();
        if last_state != Some(state) {
            record_driver_stage(dir, &format!("waiting for {what}: state={state}"))?;
            last_state = Some(state);
        }
        let in_time = Instant::now() <= give_up;
        if in_time && state >= 0 && reached(state) {
            record_driver_stage(dir, &format!("reached {what}: state={state}"))?;
            return Ok(());
        }
        if !in_time {
            return Err(format!(
                "the window was not {what} within {} s; last state read {state} (bit 0 miniaturized, bit 1 visible, negative: error)",
                STEP.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Leaves the marker `name` in `dir`.
fn mark(dir: &Path, name: &str) -> Result<(), String> {
    fs::write(dir.join(name), "x").map_err(|error| format!("cannot write {name}: {error}"))
}

/// The `go` driver: once the supervisor wrote the go file, posts a **marked** Escape (it passes the filter; the owner's
/// does not).
fn drive_go(dir: &Path) -> Result<(), String> {
    wait_for_file(&dir.join("go"), DEADLINE)?;
    // SAFETY: posts one key-down event with a NUL-terminated string that outlives the call; `postEvent:` is thread-safe.
    let posted = unsafe { ck_app_t_post_key(53, 0, 0, c"\x1b".as_ptr(), 1) };
    if posted == 1 {
        Ok(())
    } else {
        Err(format!("the marked Escape was not posted ({posted})"))
    }
}

/// The minimise drivers: on the script's request (and, in a typing run, once the filter has dropped keys) miniaturizes the
/// window, waits until it reads miniaturized and writes "minimised"; with `restore` goes on to deminiaturize it, waits until
/// it reads not miniaturized and visible, and writes "restored".
fn drive_minimise(dir: &Path, restore: bool, typing: bool) -> Result<(), String> {
    wait_for_file(&dir.join("min_request"), DEADLINE)?;
    record_driver_stage(dir, "min_request observed")?;
    if typing {
        let give_up = Instant::now() + Duration::from_secs(30);
        while dropped_keys() == 0 {
            if Instant::now() >= give_up {
                return Err("the filter dropped no key in 30 s of typing".to_owned());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    record_driver_stage(dir, "calling miniaturize")?;
    // SAFETY: a request on the main queue to the one GLFW window; the helper takes no pointer.
    let asked = unsafe { ck_app_t_miniaturize() };
    record_driver_stage(dir, &format!("miniaturize returned {asked}"))?;
    if asked != 1 {
        return Err(format!("the miniaturize request failed ({asked})"));
    }
    wait_for_state(dir, "miniaturized", |state| state & 1 == 1)?;
    mark(dir, "minimised")?;
    if restore {
        record_driver_stage(dir, "calling deminiaturize")?;
        // SAFETY: a request on the main queue to the one GLFW window; the helper takes no pointer.
        let asked = unsafe { ck_app_t_deminiaturize() };
        record_driver_stage(dir, &format!("deminiaturize returned {asked}"))?;
        if asked != 1 {
            return Err(format!("the deminiaturize request failed ({asked})"));
        }
        wait_for_state(dir, "restored (not miniaturized, visible)", |state| {
            state & 1 == 0 && state & 2 == 2
        })?;
        mark(dir, "restored")?;
    }
    Ok(())
}

/// Starts the window driver the supervisor asked for, if it asked for one. A driver that fails leaves its reason in
/// `driver_failed` and ends the run with SIGTERM (the application's graceful end), so the case fails at once with the state
/// the driver last read instead of at the deadline.
fn spawn_driver(typing: bool) {
    let (Some(kind), Some(dir)) = (env::var_os(DRIVER_ENV), env::var_os(DRIVER_DIR_ENV)) else {
        return;
    };
    let dir = PathBuf::from(dir);
    thread::spawn(move || {
        let outcome = match kind.to_str() {
            Some("go") => drive_go(&dir),
            Some("restore") => drive_minimise(&dir, true, typing),
            Some("hold") => drive_minimise(&dir, false, typing),
            _ => Err(format!("unknown window driver {}", kind.display())),
        };
        if let Err(reason) = outcome {
            let _ = fs::write(dir.join("driver_failed"), reason);
            let own = std::process::id();
            let target =
                signal::target(own).expect("the current process id is a positive signal target");
            // SAFETY: `signal::target` rejects zero and out-of-range pids; this is this worker process,
            // whose signal thread takes SIGTERM (the worker blocked it on every thread).
            unsafe { libc::kill(target.get(), libc::SIGTERM) };
        }
    });
}

/// The worker: the application plus the keyboard filter. `arguments` are the mode, the report file and then the
/// application's own arguments.
fn worker(arguments: Vec<OsString>) -> ExitCode {
    let mut arguments = arguments.into_iter();
    let (Some(mode), Some(report)) = (arguments.next(), arguments.next()) else {
        eprintln!("the worker needs a mode and a report file");
        return ExitCode::FAILURE;
    };
    let Some(mode) = mode.to_str().and_then(Mode::parse) else {
        eprintln!("unknown worker mode {}", mode.display());
        return ExitCode::FAILURE;
    };
    ck_window::test_support::open_windows_without_activation();
    // Step 1, before anything that can create a thread.
    block_signals();
    // Step 2, before the application's first event dispatch (inside `main_entry`).
    // SAFETY: main thread; installs one local event monitor, once, before the first window.
    unsafe { ck_app_t_install_input_filter() };
    let report = PathBuf::from(report);
    activation_watch::install_report(&report.with_extension("activation"), false);
    if mode.typing() {
        // Step 3.
        // SAFETY: main thread, after the filter; starts one thread that only posts events.
        unsafe { ck_app_t_start_typist() };
    }
    // Step 4: created after step 1, so they inherit the block.
    spawn_reporter(report);
    spawn_driver(mode.typing());
    let mut application_arguments = arguments.collect::<Vec<_>>();
    let mut allow_unfocused_updates = false;
    let mut queue_focus_lost_for_ck163 = false;
    while let Some(argument) = application_arguments.first() {
        if argument == OsStr::new(ALLOW_UNFOCUSED_UPDATES_FLAG) {
            allow_unfocused_updates = true;
            application_arguments.remove(0);
        } else if argument == OsStr::new(QUEUE_FOCUS_LOST_FOR_CK163_FLAG) {
            queue_focus_lost_for_ck163 = true;
            application_arguments.remove(0);
        } else {
            break;
        }
    }
    let _unfocused_updates =
        allow_unfocused_updates.then(ck_window::test_support::allow_updates_while_unfocused);
    if queue_focus_lost_for_ck163 {
        ck_window::test_support::queue_focus_lost_on_next_pump();
    }
    let result = codekarl::main_entry(application_arguments);
    activation_watch::assert_clean();
    result
}

// ---------------------------------------------------------------------------------------------
// The fixtures
// ---------------------------------------------------------------------------------------------

/// Lua helpers for the fixture scripts: `touch(path)` leaves a marker file, `exists(path)` tests for one and `wait_for(path)`
/// polls for one in a loop that fails the script after 5 seconds of CPU time.
const LUA_HELPERS: &str = r"
local function touch(path)
  local file = assert(io.open(path, 'w'))
  file:write('x')
  file:close()
end
local function exists(path)
  local file = io.open(path, 'r')
  if file then
    file:close()
    return true
  end
  return false
end
local function wait_for(path)
  local started = os.clock()
  while true do
    if exists(path) then
      return
    end
    if os.clock() - started > 5 then
      error('timed out waiting for ' .. path)
    end
  end
end
";

/// The close callback every windowed fixture registers: a real Escape or close-button press, from an owner who is typing, is
/// declined and logged, so it changes nothing.
const DECLINE: &str = "window.on_close_request(function(source)\n  log.info('declined close request from ' .. source)\n  return false\nend)\n";

/// A scratch directory with a configuration and scripts.
struct Fixture {
    scratch: Scratch,
}

fn lua_path(path: &Path) -> String {
    format!("[==[{}]==]", path.to_str().expect("a UTF-8 path"))
}

impl Fixture {
    fn new() -> Self {
        Self {
            scratch: Scratch::new(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.scratch.0.join(name)
    }

    fn has(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    /// Lets a script that waits on `HOLD` go on. A windowed case holds its window open (the script waits for this file) until
    /// the supervisor has seen the window open and, in a typing run, the filter drop keys.
    fn release(&self) {
        fs::write(self.path("hold"), "x").expect("the hold file");
    }

    /// The `ui` script `body` after the helpers and the declining callback, with the shutdown logs. `HOLD` names the file
    /// [`release`](Self::release) writes, `DIR` the fixture's directory (with a trailing separator).
    fn windowed(&self, extra_config: &str, body: &str) -> PathBuf {
        let directory = format!(
            "[==[{}/]==]",
            self.scratch.0.to_str().expect("a UTF-8 path")
        );
        let ui = format!(
            "{LUA_HELPERS}\nlocal HOLD = {}\nlocal DIR = {directory}\n{DECLINE}\n{body}\nfunction shutdown()\n  log.info('ui shutdown')\n  touch({})\nend\n",
            lua_path(&self.path("hold")),
            lua_path(&self.path("ui_shutdown"))
        );
        let gameplay = format!(
            "{LUA_HELPERS}\nfunction shutdown()\n  log.info('gameplay shutdown')\n  touch({})\nend\n",
            lua_path(&self.path("gameplay_shutdown"))
        );
        let ui = self.scratch.file("ui.lua", &ui);
        let gameplay = self.scratch.file("gameplay.lua", &gameplay);
        self.scratch.file(
            "config.toml",
            &format!(
                "[log]\ntimestamps = false\ncolour = \"never\"\n\n{extra_config}\n[lua.gameplay]\nentry = '{}'\n\n[lua.ui]\nentry = '{}'\n",
                gameplay.display(),
                ui.display()
            ),
        )
    }

    /// A windowed configuration (glfw-vulkan) running `body`.
    fn vulkan(&self, body: &str) -> PathBuf {
        self.windowed("[window]\nbackend = \"glfw-vulkan\"\n", body)
    }
}

// ---------------------------------------------------------------------------------------------
// Supervision (the shared bounded supervisor: `tests/supervisor`)
// ---------------------------------------------------------------------------------------------

/// A worker under supervision, with the report file its reporter thread writes.
struct Worker {
    live: Supervised,
    report: PathBuf,
    /// Where the window driver, if there is one, leaves `driver_failed`.
    driver_dir: Option<PathBuf>,
    /// Keeps the report's directory until the case is over.
    _scratch: Scratch,
}

impl Worker {
    /// The keys the filter has dropped, as the reporter last said.
    fn dropped(&self) -> Option<u64> {
        read_dropped(&self.report)
    }
}

/// The names of the dynamic-loader variables in this process's environment.
fn dynamic_loader_variables() -> Vec<OsString> {
    env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| {
            let name = name.to_string_lossy();
            name.starts_with("DYLD_") || name == "LD_LIBRARY_PATH"
        })
        .collect()
}

/// Starts a worker (in the current mode) running the application with `args` under the supervisor, with no dynamic-loader
/// variable and the validation layer on; `driver` names the window driver and its directory.
fn start_with(args: &[&OsStr], driver: Option<(&str, &Path)>) -> Worker {
    let scratch = Scratch::new();
    let report = scratch.0.join("report");
    let exe = env::current_exe().expect("this executable's path");
    let mut command = Command::new(exe);
    command.arg(WORKER_FLAG).arg(mode().word()).arg(&report);
    if ALLOW_UNFOCUSED_UPDATES.load(Ordering::SeqCst) {
        command.arg(ALLOW_UNFOCUSED_UPDATES_FLAG);
    }
    if QUEUE_FOCUS_LOST_FOR_CK163.load(Ordering::SeqCst) {
        command.arg(QUEUE_FOCUS_LOST_FOR_CK163_FLAG);
    }
    command.args(args);
    for name in dynamic_loader_variables() {
        command.env_remove(name);
    }
    command.env("VK_INSTANCE_LAYERS", "VK_LAYER_KHRONOS_validation");
    if let Some((kind, dir)) = driver {
        command.env(DRIVER_ENV, kind).env(DRIVER_DIR_ENV, dir);
    }
    let live = Supervised::spawn(command, DEADLINE).unwrap_or_else(|error| panic!("{error}"));
    Worker {
        live,
        report,
        driver_dir: driver.map(|(_, dir)| dir.to_path_buf()),
        _scratch: scratch,
    }
}

fn start(args: &[&OsStr]) -> Worker {
    start_with(args, None)
}

/// Waits until the worker's output contains `needle`.
fn wait_for(worker: &mut Worker, needle: &str, within: Duration) {
    worker
        .live
        .wait_for(needle, within)
        .unwrap_or_else(|error| panic!("{error}"));
}

/// In a typing run, waits until the filter has dropped keys, so the case proves something about typing (a quiet run needs
/// nothing). Bounded.
fn wait_for_drops(worker: &Worker) {
    if !mode().typing() {
        return;
    }
    let give_up = Instant::now() + Duration::from_secs(30);
    while worker.dropped().is_none_or(|dropped| dropped == 0) {
        assert!(
            Instant::now() < give_up,
            "the filter dropped no key in 30 s of typing"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

/// Sends `signal` to the worker.
fn signal(worker: &mut Worker, kind: i32) {
    let pid = worker.live.id();
    let pid = signal::live_pid(worker.live.process.as_mut(), pid)
        .unwrap_or_else(|error| panic!("{error}"));
    // SAFETY: the shared guard confirmed a positive pid for this supervisor's live direct child.
    let sent = unsafe { libc::kill(pid.get(), kind) };
    assert_eq!(sent, 0, "kill of the live child failed");
}

/// A validation message anywhere in the output fails the case, unless it is the one tolerated record: marked by the in-process scope
/// that decided it, and naming the tolerated VUID (see [`validation_rule`]). The rule never compares counts.
fn assert_no_validation(seen: &Seen) {
    if let Err(message) = validation_rule::check(&seen.stdout) {
        panic!("{message}\nfull output:\n{}", seen.stdout);
    }
}

/// Waits for the worker to end and reads everything it wrote; a deadline, a failed poll, kill or reap and a read error fail
/// the case. A close request from a key that reached the application fails it too (the filter drops every unmarked key, so
/// none may; the demo case, whose close **is** a marked key, uses [`finish_allowing_keys`]), and so does a validation message
/// or a window driver that gave up.
fn finish(worker: Worker) -> Seen {
    let seen = finish_allowing_keys(worker);
    assert!(
        !seen.stdout.contains("declined close request from key"),
        "a key reached the application:\n{}",
        seen.stdout
    );
    seen
}

/// [`finish`] for a case whose close is a marked key.
fn finish_allowing_keys(worker: Worker) -> Seen {
    let activation = worker.report.with_extension("activation");
    let driver_failed = worker.driver_dir.map(|dir| dir.join("driver_failed"));
    let driver_stage = driver_failed
        .as_ref()
        .map(|path| path.with_file_name("driver_stage"));
    let seen = worker.live.finish().unwrap_or_else(|error| {
        let stage = driver_stage
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .unwrap_or_else(|| "no driver stage recorded".to_owned());
        panic!("{error}; window driver last stage: {stage}");
    });
    activation_watch::assert_report(&activation);
    assert_no_validation(&seen);
    if let Some(failed) = driver_failed {
        assert!(
            !failed.exists(),
            "the window driver gave up: {}\n{}",
            fs::read_to_string(&failed).unwrap_or_default(),
            seen.stdout
        );
    }
    seen
}

/// Runs the application on `config` (and `extra` arguments) to its end.
fn run(config: &Path, extra: &[&str]) -> Seen {
    let mut args: Vec<&OsStr> = vec![OsStr::new("--config"), config.as_os_str()];
    args.extend(extra.iter().map(OsStr::new));
    finish(start(&args))
}

/// Runs a windowed `config`, holding its window open (the script waits for `fixture.release()`) until the window is open and,
/// in a typing run, the filter has dropped keys, then lets it go on to its end.
fn run_windowed(fixture: &Fixture, config: &Path, extra: &[&str]) -> Seen {
    let mut args: Vec<&OsStr> = vec![OsStr::new("--config"), config.as_os_str()];
    args.extend(extra.iter().map(OsStr::new));
    let mut worker = start(&args);
    wait_for(
        &mut worker,
        "window: window opened",
        Duration::from_secs(30),
    );
    wait_for_drops(&worker);
    fixture.release();
    finish(worker)
}

/// Asserts that each needle occurs in `text` after the previous one (other records, such as declined close requests, may sit
/// between them).
fn assert_in_order(text: &str, needles: &[&str]) {
    let mut from = 0;
    for needle in needles {
        match text[from..].find(needle) {
            Some(at) => from += at + needle.len(),
            None => panic!("{needle:?} not found in order after offset {from} in:\n{text}"),
        }
    }
}

/// Asserts that every needle occurs after `after` and before `before` (in any order among themselves: the VMs' stop requests
/// are sent together, so their records may come in either order).
fn assert_between(text: &str, after: &str, before: &str, needles: &[&str]) {
    let start = text
        .find(after)
        .unwrap_or_else(|| panic!("{after:?} not found in:\n{text}"));
    let end = text
        .rfind(before)
        .unwrap_or_else(|| panic!("{before:?} not found in:\n{text}"));
    for needle in needles {
        let at = text[start..].find(needle).map(|at| start + at);
        assert!(
            at.is_some_and(|at| at < end),
            "{needle:?} is not between {after:?} and {before:?} in:\n{text}"
        );
    }
}

/// How many times `needle` occurs.
fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

// ---------------------------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------------------------

/// The repository's committed `assets` directory (it holds the demonstration's placeholder image).
fn shipped_assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .canonicalize()
        .expect("the shipped assets directory")
}

fn a_window_opened_and_closed_by_the_script_exits_0_and_both_shutdowns_follow_the_close() {
    let fixture = Fixture::new();
    let config = fixture.vulkan(
        "window.open{title='close me', width=200, height=150, clear={0.2, 0.3, 0.4}}\nwindow.on_opened(function() wait_for(HOLD) window.close() end)",
    );
    let seen = run_windowed(&fixture, &config, &[]);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "runtime: window: glfw-vulkan",
            "runtime: start-up complete",
            "window: window opened",
            "window: window closed",
            "runtime: shutdown complete",
        ],
    );
    assert_between(
        &seen.stdout,
        "window: window closed",
        "runtime: shutdown complete",
        &["ui shutdown", "gameplay shutdown"],
    );
    assert!(
        fixture.has("ui_shutdown") && fixture.has("gameplay_shutdown"),
        "both shutdown() ran"
    );
    assert_eq!(count(&seen.stdout, "window: window opened"), 1);
    assert!(!seen.stdout.contains("ERROR"), "{}", seen.stdout);
}

/// Milestone 4.5 on a real window, now through the compatibility lane: the quad set before `window.open` is drawn by the
/// Vulkan scene presenter once its image is adopted (CK-119's gate), and the first-frame record names the generation. The
/// fixture registers the declining close callback and closes the window itself once the test has seen the record and says go.
fn the_demo_placeholder_is_drawn_with_the_quads_generation_in_the_first_frame_and_the_run_closes_cleanly()
 {
    let fixture = Fixture::new();
    let go = fixture.path("go");
    // The quad is placed before the window opens, then the window stays open until the test has seen the first frame's
    // record and says go.
    let ui = format!(
        "render.quad{{image='demo-quad.png', x=10, y=10, width=128, height=128}}\n\
         window.open{{title='quad', width=200, height=150, clear={{0.1, 0.2, 0.3}}}}\n\
         window.on_opened(function()\n  wait_for({})\n  window.close()\nend)",
        lua_path(&go)
    );
    let config = fixture.windowed(
        &format!(
            "[window]\nbackend = \"glfw-vulkan\"\n\n[render]\nfilter = \"nearest\"\nassets = '{}'\n\n\
             [log.categories]\nrender = \"debug\"\n",
            shipped_assets().display()
        ),
        &ui,
    );
    let mut worker = start(&[OsStr::new("--config"), config.as_os_str()]);
    wait_for(
        &mut worker,
        "render: first frame drawn, generation 2",
        Duration::from_secs(30),
    );
    wait_for_drops(&worker);
    fs::write(&go, "x").expect("the go file");
    let seen = finish(worker);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "runtime: start-up complete",
            "render: first frame drawn, generation 2",
            "window: window closed",
            "runtime: shutdown complete",
            "render: 1 scene write(s) accepted, 0 discarded after the run ended",
        ],
    );
    assert_eq!(
        count(&seen.stdout, "window: window opened"),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(&seen.stdout, "render: first frame drawn"),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(&seen.stdout, "arrive in milestone 4.5"),
        0,
        "the Vulkan path draws the quad now: {}",
        seen.stdout
    );
    assert!(!seen.stdout.contains("ERROR"), "{}", seen.stdout);
}

fn sigterm_after_window_opened_shuts_down_cleanly_and_exits_143() {
    let fixture = Fixture::new();
    let config =
        fixture.vulkan("window.open{title='wait', width=200, height=150, clear={0, 0, 0}}");
    let mut worker = start(&[OsStr::new("--config"), config.as_os_str()]);
    wait_for(
        &mut worker,
        "window: window opened",
        Duration::from_secs(30),
    );
    wait_for_drops(&worker);
    signal(&mut worker, libc::SIGTERM);
    let seen = finish(worker);
    assert_eq!(seen.code, Some(143), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "window: window opened",
            "received SIGTERM, quitting",
            "window: window closed",
            "runtime: shutdown complete",
        ],
    );
    assert!(fixture.has("ui_shutdown") && fixture.has("gameplay_shutdown"));
}

fn backend_glfw_vulkan_over_a_configuration_naming_native_metal_opens_the_glfw_window() {
    let fixture = Fixture::new();
    // The configured backend is not the one used, so it need not be available in this build.
    let config = fixture.windowed(
        "[window]\nbackend = \"native-metal\"\n",
        "window.open{title='override', width=200, height=150, clear={0.3, 0.2, 0.1}}\nwindow.on_opened(function() wait_for(HOLD) window.close() end)",
    );
    let seen = run_windowed(&fixture, &config, &["--backend", "glfw-vulkan"]);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "runtime: window: glfw-vulkan",
            "window: window opened",
            "window: window closed",
            "runtime: shutdown complete",
        ],
    );
    assert!(!seen.stdout.contains("native-metal"), "{}", seen.stdout);
    assert!(!seen.stdout.contains("ERROR"), "{}", seen.stdout);
}

fn the_shipped_demonstration_configurations_run_headless() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    for name in [
        "demo-glfw-vulkan.toml",
        "demo-glfw-vulkan-decline.toml",
        "demo-glfw-vulkan-callback-error.toml",
        "demo-quad-vulkan.toml",
    ] {
        let config = root.join(name);
        let text = fs::read_to_string(&config).expect("a shipped configuration");
        assert!(text.contains("backend = \"glfw-vulkan\""), "{name}");
        // `--headless` loads the configuration, runs the scripts without a window and ends (the callback-error
        // demonstration fails only when its window opens).
        let seen = run(&config, &["--headless"]);
        assert_eq!(seen.code, Some(0), "{name}: {}", seen.stdout);
        assert!(
            seen.stdout.contains("runtime: window: none (headless)")
                && seen.stdout.contains("window.open ignored: headless run"),
            "{name}: {}",
            seen.stdout
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The common demonstration, unchanged, on glfw-vulkan (CK-159 test 4)
// ---------------------------------------------------------------------------------------------

/// The shipped `config/demo-scene.toml` with its paths made absolute (it is read from the scratch directory) and the
/// `render` and `resources` records shown at `debug`: nothing else differs, and each replacement is asserted to have
/// happened. The script and the assets are the shipped ones.
fn demo_config(fixture: &Fixture) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut text =
        fs::read_to_string(root.join("config/demo-scene.toml")).expect("the demo configuration");
    for (relative, name) in [
        ("../assets", "assets"),
        ("../scripts/ui/demo_scene.lua", "scripts/ui/demo_scene.lua"),
        ("../scripts/gameplay/main.lua", "scripts/gameplay/main.lua"),
    ] {
        let from = format!("\"{relative}\"");
        assert!(
            text.contains(&from),
            "{relative} is in the demo configuration"
        );
        let absolute = root.join(name).canonicalize().expect("a shipped path");
        text = text.replace(&from, &format!("'{}'", absolute.display()));
    }
    text.push_str("\n[log.categories]\nrender = \"debug\"\nresources = \"debug\"\n");
    fixture.scratch.file("demo-scene.toml", &text)
}

/// The demo runs on `glfw-vulkan`: the player and eleven tiles become `ready`, the missing tile `failed`; the supervisor then
/// says go, the worker's driver posts a **marked** Escape (the typist's unmarked ones are dropped), the close is accepted,
/// and the run ends `Closed` with exit 0, a proven presenter end, the first-frame record and a complete final resource
/// record with nothing live, retiring or referenced. Negative control: without the go file, the run keeps its window until
/// the case's own SIGTERM (exit 143), so the close comes from the marked key, not the typist.
#[expect(
    clippy::too_many_lines,
    reason = "the run, its notice, its final record and the negative control read best as one case"
)]
fn the_demo_scene_runs_unchanged_on_glfw_vulkan_and_closes_by_a_marked_escape() {
    let fixture = Fixture::new();
    let config = demo_config(&fixture);
    let mut worker = start_with(
        &[
            OsStr::new("--config"),
            config.as_os_str(),
            OsStr::new("--backend"),
            OsStr::new("glfw-vulkan"),
        ],
        Some(("go", &fixture.scratch.0)),
    );
    wait_for(
        &mut worker,
        "window: window opened",
        Duration::from_secs(30),
    );
    for tile in [
        "demo-quad.png",
        "demo-scene/tile-01-stripes-h.png",
        "demo-scene/tile-02-stripes-v.png",
        "demo-scene/tile-03-diagonal.png",
        "demo-scene/tile-04-checker.png",
        "demo-scene/tile-05-dots.png",
        "demo-scene/tile-06-cross.png",
        "demo-scene/tile-07-ring.png",
        "demo-scene/tile-08-frame.png",
        "demo-scene/tile-09-diamond.png",
        "demo-scene/tile-10-quarters.png",
        "demo-scene/tile-11-bars.png",
    ] {
        wait_for(
            &mut worker,
            &format!("texture ready: {tile}"),
            Duration::from_secs(30),
        );
    }
    wait_for(
        &mut worker,
        "texture failed: demo-scene/missing-tile.png",
        Duration::from_secs(30),
    );
    wait_for_drops(&worker);
    // The go file: the worker's driver sees it and posts the marked Escape.
    fs::write(fixture.path("go"), "x").expect("the go file");
    let seen = finish_allowing_keys(worker);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "runtime: window: glfw-vulkan",
            "render: first frame drawn, generation ",
            "close requested by key",
            "window: window closed",
            "runtime: shutdown complete",
        ],
    );
    assert_eq!(
        count(&seen.stdout, "texture ready: "),
        12,
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(&seen.stdout, "texture failed: "),
        1,
        "{}",
        seen.stdout
    );
    assert!(!seen.stdout.contains("ERROR"), "{}", seen.stdout);
    assert!(!seen.stdout.contains("incomplete"), "{}", seen.stdout);
    let record = final_record(&seen.stdout);
    assert_reconciled(&record);
    // The binding notice, once, through the table at the demo's default 131,072 slots (the support query's answer is the
    // device's: the text says which), and the final record counts the same tables.
    assert_eq!(
        count(
            &seen.stdout,
            "render: bindless table of 131072 slots (vkGetDescriptorSetLayoutSupport: "
        ),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(
        number_after(&record, "bindless_slots="),
        131_072,
        "{record}"
    );
    assert!(
        !seen.stdout.contains("bindless unavailable"),
        "this device passes the gate:\n{}",
        seen.stdout
    );

    // The negative control.
    let fixture = Fixture::new();
    let config = demo_config(&fixture);
    let mut worker = start_with(
        &[
            OsStr::new("--config"),
            config.as_os_str(),
            OsStr::new("--backend"),
            OsStr::new("glfw-vulkan"),
        ],
        Some(("go", &fixture.scratch.0)),
    );
    wait_for(
        &mut worker,
        "texture failed: demo-scene/missing-tile.png",
        Duration::from_secs(30),
    );
    wait_for_drops(&worker);
    // Long enough for a close that came from the typist to have happened.
    thread::sleep(Duration::from_secs(1));
    signal(&mut worker, libc::SIGTERM);
    let seen = finish(worker);
    assert_eq!(seen.code, Some(143), "{}", seen.stdout);
    assert!(
        !seen.stdout.contains("close requested by key"),
        "{}",
        seen.stdout
    );
}

// ---------------------------------------------------------------------------------------------
// The texture lifecycle on a real window, scripts driving themselves (CK-159 test 5)
// ---------------------------------------------------------------------------------------------
//
// The scripts drive everything themselves from their frame callback and log what they observe; the window is closed by the
// script (`window.close()`), never by a key. A script closes only after the supervisor released it (`HOLD`), so a typing run
// proves its keys were dropped. No case asserts an order between `Opened`, the first-frame record and a first `ready`: a
// startup load may finish before the window opens and the first-frame report is consumed asynchronously.

/// Lua helpers of the lifecycle scripts: whether the supervisor released the window, and the close once a script is done.
const LIFECYCLE_HELPERS: &str = r"
local function released()
  return exists(HOLD)
end
local updates = 0
local MAX_UPDATES = 3000
-- Counts an update and closes the window once `done` holds and the supervisor released it (or gives up).
local function close_when(done)
  updates = updates + 1
  if (done and released()) or updates > MAX_UPDATES then
    if updates > MAX_UPDATES then log.info('gave up') end
    window.close()
  end
end
";

/// Three textures, loaded in `startup()` with a quad each: every one must become `ready`.
const PENDING_THEN_READY: &str = r"
local names = {'demo-quad.png', 'demo-scene/tile-02-stripes-v.png', 'demo-scene/tile-03-diagonal.png'}
local tokens, ready, settled = {}, {}, 0
function startup()
  for index, name in ipairs(names) do
    tokens[index] = texture.load(name)
    render.create_quad{texture = tokens[index], x = 20 + (index - 1) * 80, y = 20, width = 64, height = 64}
  end
  frame.on_update(function(f)
    for index, name in ipairs(names) do
      if not ready[index] then
        local status = texture.status(tokens[index])
        if status.state == 'ready' then
          ready[index] = true
          settled = settled + 1
          log.info('ready: ' .. name .. ' shown ' .. tostring(status.shown))
        end
      end
    end
    close_when(settled == #names)
  end)
  window.open{title = 'pending then ready', width = 400, height = 160, clear = {0.1, 0.2, 0.3}}
end
";

/// A quad of a file that does not exist keeps drawing: its status becomes `failed` with the error, and the run ends normally.
const FAILED_INITIAL_LOAD: &str = r"
local good, missing
local seen = {}
function startup()
  good = texture.load('demo-quad.png')
  missing = texture.load('demo-scene/no-such-tile.png')
  render.create_quad{texture = good, x = 20, y = 20, width = 64, height = 64}
  render.create_quad{texture = missing, x = 120, y = 20, width = 64, height = 64}
  frame.on_update(function(f)
    local status = texture.status(missing)
    if status.state == 'failed' and not seen.failed then
      seen.failed = true
      log.info('failed: ' .. tostring(status.error))
    end
    if texture.status(good).state == 'ready' and not seen.ready then
      seen.ready = true
      log.info('ready: good')
    end
    close_when(seen.failed and seen.ready)
  end)
  window.open{title = 'failed load', width = 400, height = 160, clear = {0.1, 0.2, 0.3}}
end
";

/// A quad moved on every update for sixty updates: no texture beyond the one loaded is ever created.
const MOVEMENT: &str = r"
local token, quad
local x, ready, moved = 20, false, 0
function startup()
  token = texture.load('demo-quad.png')
  quad = render.create_quad{texture = token, x = x, y = 20, width = 64, height = 64}
  frame.on_update(function(f)
    moved = moved + 1
    x = 20 + (moved % 200)
    render.set_position(quad, x, 20)
    if not ready and texture.status(token).state == 'ready' then
      ready = true
      log.info('ready: gem')
    end
    close_when(ready and moved >= 60)
  end)
  window.open{title = 'movement', width = 400, height = 160, clear = {0.1, 0.2, 0.3}}
end
";

/// `texture.reload`: a new revision becomes shown and the quad is unchanged.
const RELOAD: &str = r"
local token
local reloaded, shown_again = false, false
function startup()
  token = texture.load('demo-quad.png')
  render.create_quad{texture = token, x = 20, y = 20, width = 64, height = 64}
  frame.on_update(function(f)
    local status = texture.status(token)
    if not reloaded and status.state == 'ready' and status.shown == 1 then
      reloaded = true
      log.info('reload requested: revision ' .. tostring(texture.reload(token)))
    elseif reloaded and not shown_again and status.shown == 2 then
      shown_again = true
      log.info('reloaded: shown 2')
    end
    close_when(shown_again)
  end)
  window.open{title = 'reload', width = 400, height = 160, clear = {0.1, 0.2, 0.3}}
end
";

/// `render.destroy_quad` then `texture.release`: the texture retires before the run ends.
const RELEASE: &str = r"
local token, quad
local released_texture = false
function startup()
  token = texture.load('demo-quad.png')
  quad = render.create_quad{texture = token, x = 20, y = 20, width = 64, height = 64}
  frame.on_update(function(f)
    if not released_texture and texture.status(token).state == 'ready' then
      released_texture = true
      render.destroy_quad(quad)
      texture.release(token)
      log.info('released')
    end
    close_when(released_texture and updates > 60)
  end)
  window.open{title = 'release', width = 400, height = 160, clear = {0.1, 0.2, 0.3}}
end
";

/// A windowed configuration for the lifecycle scripts: glfw-vulkan, the shipped assets, nearest filtering, a paced update
/// source and the resource and render records at `debug`.
fn lifecycle_config() -> String {
    format!(
        "[window]\nbackend = \"glfw-vulkan\"\n\n[render]\nfilter = \"nearest\"\nassets = '{}'\n\n\
         [frame]\nupdates_per_second = 60\n\n[log.categories]\nresources = \"debug\"\nrender = \"debug\"\n",
        shipped_assets().display()
    )
}

/// Runs a lifecycle `script` on a real window and returns what the run wrote. The run must end `Closed`, exit 0, with both
/// shutdowns after the close and no error or incomplete record.
fn lifecycle(script: &str) -> Seen {
    lifecycle_in(&lifecycle_config(), script)
}

/// [`lifecycle`] over this configuration text.
fn lifecycle_in(configuration: &str, script: &str) -> Seen {
    let fixture = Fixture::new();
    let config = fixture.windowed(configuration, &format!("{LIFECYCLE_HELPERS}\n{script}"));
    let seen = run_windowed(&fixture, &config, &[]);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "runtime: window: glfw-vulkan",
            "window: window opened",
            "window: window closed",
            "runtime: shutdown complete",
        ],
    );
    assert!(
        !seen.stdout.contains("ERROR")
            && !seen.stdout.contains("incomplete")
            && !seen.stdout.contains("gave up"),
        "{}",
        seen.stdout
    );
    assert!(
        fixture.has("ui_shutdown") && fixture.has("gameplay_shutdown"),
        "both shutdown() ran"
    );
    seen
}

/// The run's final resource record (one line), asserting there is exactly one and that it is written once the VMs have
/// stopped and before the resources' own stop completes (the reconciliation is the stop's last step).
fn final_record(text: &str) -> String {
    assert_eq!(
        count(text, "resources: final resources: "),
        1,
        "exactly one final resource record:\n{text}"
    );
    assert_in_order(
        text,
        &[
            "runtime: stopped ui",
            "resources: final resources: ",
            "runtime: stopped resources",
        ],
    );
    text.lines()
        .find(|line| line.contains("resources: final resources: "))
        .expect("the final record")
        .to_owned()
}

/// The number that follows `key` in `record`.
fn number_after(record: &str, key: &str) -> u64 {
    let at = record
        .find(key)
        .unwrap_or_else(|| panic!("{key:?} not in {record}"));
    record[at + key.len()..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| panic!("no number after {key:?} in {record}"))
}

/// A reconciled run: nothing live, retiring or referenced after `finish_teardown`, and a complete run.
fn assert_reconciled(record: &str) {
    assert!(
        record.contains(" 0 live,")
            && record.contains(" 0 retiring,")
            && record.contains(" 0 referenced at finish_teardown"),
        "{record}"
    );
    assert!(record.ends_with("complete"), "{record}");
    // The Vulkan record writes its own figures (`bindless_slots=N` among them) and none of the Metal-only ones.
    assert!(
        record.contains("bindless_slots=") && !record.contains("instance buffer"),
        "{record}"
    );
}

/// The texture count of the Vulkan record: the loaded textures plus the checker the presenter makes first.
fn created(record: &str) -> u64 {
    number_after(record, "textures created ")
}

fn pending_textures_become_ready_and_the_run_reconciles_to_zero() {
    let seen = lifecycle(PENDING_THEN_READY);
    // Independent textures settle in whatever order the loaders and the upload batches give: each exact ready/shown record is
    // required once, in no particular order relative to the others.
    for record in [
        "ready: demo-quad.png shown 1",
        "ready: demo-scene/tile-02-stripes-v.png shown 1",
        "ready: demo-scene/tile-03-diagonal.png shown 1",
    ] {
        assert_eq!(count(&seen.stdout, record), 1, "{record}: {}", seen.stdout);
    }
    let record = final_record(&seen.stdout);
    assert_eq!(
        created(&record),
        3 + 1,
        "three textures and the checker: {record}"
    );
    assert_reconciled(&record);
}

fn a_missing_file_stays_a_failed_checker_quad_and_the_run_ends_normally() {
    let seen = lifecycle(FAILED_INITIAL_LOAD);
    assert!(seen.stdout.contains("failed: "), "{}", seen.stdout);
    assert!(seen.stdout.contains("no-such-tile.png"), "{}", seen.stdout);
    let record = final_record(&seen.stdout);
    // One texture was loaded (the good one) and the checker made: the missing file's quad drew the checker.
    assert_eq!(created(&record), 1 + 1, "{record}");
    assert_reconciled(&record);
}

fn moving_a_quad_for_many_updates_uploads_nothing_but_a_reload_does() {
    // The movement case: 60 updates move the quad; the upload store created exactly the one texture loaded and the checker.
    let moved = lifecycle(MOVEMENT);
    let record = final_record(&moved.stdout);
    assert_eq!(created(&record), 1 + 1, "{record}");
    // The record carries the upload store's measured high-water bytes: one image was charged, and its staging with it.
    let single = number_after(&record, "upload texture peak ");
    assert!(single >= 4096, "{record}");
    assert!(
        number_after(&record, "upload staging peak ") >= 4096,
        "{record}"
    );
    assert_reconciled(&record);
    // The negative control: the same counter does move when something does upload (a reload creates a second texture).
    let reloaded = lifecycle(RELOAD);
    assert!(
        reloaded.stdout.contains("reload requested: revision 2"),
        "{}",
        reloaded.stdout
    );
    assert!(
        reloaded.stdout.contains("reloaded: shown 2"),
        "{}",
        reloaded.stdout
    );
    let record = final_record(&reloaded.stdout);
    assert_eq!(created(&record), 2 + 1, "{record}");
    assert_reconciled(&record);
}

/// The setting reaches the presenter (CK-128, now on `glfw-vulkan`): a fixture with `max_textures = 8`, `max_extra_revisions = 8`
/// and `bindless_texture_slots = 17` (the rule's exact minimum) ends with `bindless_slots=17` in the final record, the observed
/// capacity of the tables the presenter allocated, and the notice says so once; the default configuration shows 131072 (the
/// negative control: a hard-coded count would show the same number twice).
fn the_configured_bindless_slot_count_reaches_the_presenter_and_the_final_record() {
    let small = format!(
        "{}\n[resources]\nmax_textures = 8\nmax_extra_revisions = 8\n",
        lifecycle_config().replace("[render]\n", "[render]\nbindless_texture_slots = 17\n")
    );
    let seen = lifecycle_in(&small, MOVEMENT);
    let record = final_record(&seen.stdout);
    assert_eq!(number_after(&record, "bindless_slots="), 17, "{record}");
    assert_eq!(
        created(&record),
        2,
        "the loaded texture and the checker: {record}"
    );
    assert_eq!(
        count(
            &seen.stdout,
            "render: bindless table of 17 slots (vkGetDescriptorSetLayoutSupport: "
        ),
        1,
        "{}",
        seen.stdout
    );
    assert_reconciled(&record);
    let default = lifecycle(MOVEMENT);
    let record = final_record(&default.stdout);
    assert_eq!(
        number_after(&record, "bindless_slots="),
        131_072,
        "{record}"
    );
}

// Frame timing on a real window (milestone 5.5, issue 6, part B): `diagnostics.metrics_mark` and the one `metrics` record
// ---------------------------------------------------------------------------------------------

/// A moving quad, as `MOVEMENT`, whose script marks the measured window once its texture is ready (twice: the second call changes
/// nothing) and closes after many more updates.
const MARKED: &str = r"
local token, quad
local x, ready, marked = 20, false, false
function startup()
  token = texture.load('demo-quad.png')
  quad = render.create_quad{texture = token, x = x, y = 20, width = 64, height = 64}
  frame.on_update(function(f)
    x = 20 + (updates % 200)
    render.set_position(quad, x, 20)
    if not ready and texture.status(token).state == 'ready' then
      ready = true
      log.info('ready: gem')
    end
    if ready and not marked then
      marked = true
      diagnostics.metrics_mark()
      diagnostics.metrics_mark()
      log.info('marked')
    end
    close_when(marked and updates > 300)
  end)
  window.open{title = 'marked', width = 400, height = 160, clear = {0.1, 0.2, 0.3}}
end
";

/// The configuration of the lifecycle scripts with the `metrics` record shown.
fn metrics_config() -> String {
    format!("{}metrics = \"debug\"\n", lifecycle_config())
}

/// The run's one `metrics` record, asserting there is exactly one record in the category (nothing is written per frame) and that it
/// comes while the run is up, before the shutdown completes.
fn metrics_record(text: &str) -> String {
    assert_eq!(
        count(text, "metrics: "),
        1,
        "exactly one record in the category:\n{text}"
    );
    assert_in_order(
        text,
        &[
            "runtime: window: glfw-vulkan",
            "metrics: run: frames=",
            "runtime: shutdown complete",
        ],
    );
    text.lines()
        .find(|line| line.contains("metrics: "))
        .expect("the metrics record")
        .to_owned()
}

/// The number after `key` at or after `from` in `record`.
fn number_in(record: &str, from: &str, key: &str) -> u64 {
    let at = record
        .find(from)
        .unwrap_or_else(|| panic!("{from:?} not in {record}"));
    number_after(&record[at..], key)
}

/// A marked `glfw-vulkan` run writes exactly one record: the measured window beside the whole-run totals, both owners' readings of the
/// same window final, the second mark counted, and the sources the device does not offer said (never a zero in their place).
fn a_marked_run_writes_one_metrics_record_with_the_window_beside_the_totals() {
    let seen = lifecycle_in(&metrics_config(), MARKED);
    assert_no_validation(&seen);
    let record = metrics_record(&seen.stdout);
    for field in [
        "run: frames=",
        " skipped=",
        " invalid_marks=",
        " invalid_gpu=",
        " presented=",
        " not_presented=",
        " presentation_expired=",
        " outstanding=",
        " draw_calls_max=",
        " instances_max=",
        "cpu_total[n=",
        "build[n=",
        "encode[n=",
        "ledger[n=",
        "acquire[n=",
        "gpu[n=",
        "presentation_interval[n=",
        "run updates delivered=",
        "timing_lost=",
        "uploads: files_decoded=",
        " textures_created(native)=",
        " textures_ready=",
        "; window id=1 start=",
        " end=",
        " duration=",
        " marks_after_the_first=1",
        "; presenter window adopted=",
        " late_adopted=false end_overrun=false",
        "; window: frames=",
        "; loop window adopted=",
        " window updates delivered=",
    ] {
        assert!(record.contains(field), "{field:?} missing from {record}");
    }
    assert_eq!(number_after(&record, "files_decoded="), 1, "{record}");
    assert_eq!(number_after(&record, "textures_ready="), 1, "{record}");
    let frames = number_after(&record, "run: frames=");
    let window_frames = number_in(&record, "; window: frames=", "frames=");
    assert!(
        frames > 0 && window_frames > 0 && window_frames <= frames,
        "{record}"
    );
    assert_eq!(
        number_in(&record, "; window: frames=", "cpu_total[n="),
        window_frames,
        "one CPU total per frame in the window: {record}"
    );
    assert_eq!(
        number_in(&record, "; window: frames=", "draw_calls_max="),
        1,
        "one draw per frame through the table: {record}"
    );
    assert!(
        number_in(&record, "; window id=1", "duration=") > 0,
        "{record}"
    );
    assert!(
        number_in(&record, "; loop window", "window updates delivered=") > 0,
        "{record}"
    );
    assert_eq!(
        record.matches("complete=true").count(),
        2,
        "both owners' readings are final: {record}"
    );
    // A source the device does not offer is said, with its reason; where it is offered there is no note.
    for (note, reason) in [
        ("gpu: unavailable", "timestampValidBits 0"),
        (
            "presentation: unavailable",
            "VK_GOOGLE_display_timing not advertised",
        ),
    ] {
        if record.contains(note) {
            assert!(record.contains(reason), "{record}");
        }
    }
}

/// A run that never marks reports the whole-run totals only.
fn a_run_without_a_mark_reports_the_totals_only() {
    let seen = lifecycle_in(&metrics_config(), MOVEMENT);
    assert_no_validation(&seen);
    let record = metrics_record(&seen.stdout);
    assert!(
        record.contains("window: none (no metrics_mark)"),
        "{record}"
    );
    assert!(!record.contains("presenter window"), "{record}");
    assert!(number_after(&record, "run: frames=") > 0, "{record}");
}

/// The output rule over supplied outputs (the same function every case's output goes through): the one genuinely tolerated
/// record passes, and every substitute fails, whatever the counts.
fn the_validation_output_rule_accepts_only_a_marked_record_naming_the_tolerated_vuid() {
    validation_rule::supplied_outputs_behave();
}

fn destroying_the_quad_and_releasing_the_texture_retires_it_before_the_run_ends() {
    let seen = lifecycle(RELEASE);
    assert!(seen.stdout.contains("released"), "{}", seen.stdout);
    let record = final_record(&seen.stdout);
    assert_eq!(created(&record), 1 + 1, "{record}");
    assert_reconciled(&record);
}

fn update_driven_worker_continues_after_synthetic_focus_loss() {
    let seen = lifecycle(MOVEMENT);
    assert!(
        seen.stdout
            .contains("test-support: synthetic FocusLost consumed by the shared window loop"),
        "{}",
        seen.stdout
    );
    assert!(
        seen.stdout.contains("runtime: shutdown complete"),
        "{}",
        seen.stdout
    );
}

fn synthetic_focus_loss_without_worker_opt_in_keeps_the_pause() {
    let fixture = Fixture::new();
    let config = fixture.windowed(
        &lifecycle_config(),
        &format!("{LIFECYCLE_HELPERS}\n{MOVEMENT}"),
    );
    let args = [OsStr::new("--config"), config.as_os_str()];
    let mut worker = start(&args);
    wait_for(
        &mut worker,
        "test-support: synthetic FocusLost consumed by the shared window loop",
        Duration::from_secs(10),
    );
    // The opted-in movement fixture completes in about a second here; the longer observation
    // makes an accidental opt-in or a resumed update observable in this negative control.
    // On the expected timeout, wait_for kills and reaps its child as part of the failed wait.
    let result = worker
        .live
        .wait_for("runtime: shutdown complete", Duration::from_secs(4));
    assert!(
        result
            .as_ref()
            .is_err_and(|error| { error.contains("no \"runtime: shutdown complete\" in time") }),
        "without the update opt-in, injected FocusLost must retain the normal update pause: {result:?}"
    );
    activation_watch::assert_report(&worker.report.with_extension("activation"));
}

// ---------------------------------------------------------------------------------------------
// Minimise (CK-159 test 6): every transition is acknowledged by the window's actual state
// ---------------------------------------------------------------------------------------------

/// After ten updates the script asks for the minimise; the worker's driver miniaturizes the window, waits until it reads
/// miniaturized (then writes "minimised"), deminiaturizes it and waits until it reads not miniaturized and visible (then
/// writes "restored"). The first update that finds the "restored" marker logs `update after restore` and the script closes.
const MINIMISE_RESTORE: &str = r"
local count, requested, closed = 0, false, false
function startup()
  frame.on_update(function(f)
    count = count + 1
    if count == 10 and not requested then
      requested = true
      touch(DIR .. 'min_request')
      log.info('minimise requested')
    end
    if not closed and exists(DIR .. 'restored') then
      closed = true
      log.info('update after restore')
      window.close()
    end
    if count > 100000 and not closed then
      closed = true
      log.info('gave up')
      window.close()
    end
  end)
  window.open{title = 'minimise', width = 400, height = 160, clear = {0.1, 0.2, 0.3}}
end
";

/// The script asks for the minimise as soon as the window is open and then waits for the end.
const MINIMISE_HOLD: &str = r"
window.on_opened(function()
  touch(DIR .. 'min_request')
  log.info('minimise requested')
end)
function startup()
  window.open{title = 'minimised', width = 400, height = 160, clear = {0.1, 0.2, 0.3}}
end
";

fn a_minimised_window_is_restored_and_the_next_update_closes_the_run() {
    let fixture = Fixture::new();
    let config = fixture.windowed(
        &lifecycle_config(),
        &format!("{LIFECYCLE_HELPERS}\n{MINIMISE_RESTORE}"),
    );
    let worker = start_with(
        &[OsStr::new("--config"), config.as_os_str()],
        Some(("restore", &fixture.scratch.0)),
    );
    let seen = finish(worker);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "window: window opened",
            "minimise requested",
            "update after restore",
            "window: window closed",
            "runtime: shutdown complete",
        ],
    );
    assert!(
        fixture.has("minimised") && fixture.has("restored"),
        "both transitions were acknowledged by the window's state"
    );
    assert!(
        !seen.stdout.contains("ERROR")
            && !seen.stdout.contains("incomplete")
            && !seen.stdout.contains("gave up"),
        "{}",
        seen.stdout
    );
    assert_reconciled(&final_record(&seen.stdout));
}

fn sigterm_while_the_window_is_minimised_exits_143_with_a_proven_presenter_end() {
    let fixture = Fixture::new();
    let config = fixture.windowed(
        &lifecycle_config(),
        &format!("{LIFECYCLE_HELPERS}\n{MINIMISE_HOLD}"),
    );
    let mut worker = start_with(
        &[OsStr::new("--config"), config.as_os_str()],
        Some(("hold", &fixture.scratch.0)),
    );
    // The driver acknowledges the minimise by the window's state and then writes the marker; a missing acknowledgement ends
    // the run with the driver's reason instead of falling back to an ordinary visible-window end.
    let marker = fixture.path("minimised");
    let give_up = Instant::now() + Duration::from_secs(40);
    while !marker.exists() && !fixture.has("driver_failed") {
        assert!(
            Instant::now() < give_up,
            "the minimised marker never appeared"
        );
        thread::sleep(Duration::from_millis(10));
    }
    if marker.exists() {
        signal(&mut worker, libc::SIGTERM);
    }
    let seen = finish(worker);
    assert!(
        marker.exists(),
        "the window was never acknowledged minimised"
    );
    assert_eq!(seen.code, Some(143), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "window: window opened",
            "minimise requested",
            "received SIGTERM, quitting",
            "window: window closed",
            "runtime: shutdown complete",
        ],
    );
    assert!(
        !seen.stdout.contains("ERROR")
            && !seen.stdout.contains("incomplete")
            && !seen.stdout.contains("unproven"),
        "{}",
        seen.stdout
    );
    assert_reconciled(&final_record(&seen.stdout));
}

// ---------------------------------------------------------------------------------------------
// The guard: the filter's symbols are in the test-only archive and in no production artifact
// ---------------------------------------------------------------------------------------------

/// The names the test support exports.
const FILTER_SYMBOLS: [&str; 8] = [
    "ck_app_t_install_input_filter",
    "ck_app_t_dropped_keys",
    "ck_app_t_start_typist",
    "ck_app_t_typist_mask",
    "ck_app_t_post_key",
    "ck_app_t_miniaturize",
    "ck_app_t_deminiaturize",
    "ck_app_t_window_state",
];

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// The filter lives in this test's archive and in neither the `codekarl` binary nor any `libcodekarl` artifact built beside
/// this executable.
fn input_filter_isolation() -> Result<(), String> {
    let archive = fs::read(env!("CK_CODEKARL_TESTING_ARCHIVE"))
        .map_err(|error| format!("isolation: the test support archive cannot be read: {error}"))?;
    for name in FILTER_SYMBOLS {
        if !contains(&archive, name) {
            return Err(format!("isolation: the test support archive lacks {name}"));
        }
    }
    let binary = fs::read(BINARY).map_err(|error| format!("isolation: {BINARY}: {error}"))?;
    if contains(&binary, "ck_app_t_") {
        return Err("isolation: the codekarl binary contains test support (ck_app_t_)".to_owned());
    }
    let exe = env::current_exe().map_err(|error| format!("isolation: {error}"))?;
    let directory = exe
        .parent()
        .ok_or("isolation: the test executable has no directory")?;
    let mut libraries = 0;
    for entry in fs::read_dir(directory).map_err(|error| format!("isolation: {error}"))? {
        let path = entry.map_err(|error| format!("isolation: {error}"))?.path();
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
        if name.starts_with("libcodekarl-")
            && Path::new(name).extension().is_some_and(|e| e == "rlib")
        {
            libraries += 1;
            let bytes = fs::read(&path).map_err(|error| format!("isolation: {name}: {error}"))?;
            if contains(&bytes, "ck_app_t_") {
                return Err(format!(
                    "isolation: {name} contains test support (ck_app_t_)"
                ));
            }
        }
    }
    if libraries == 0 {
        return Err("isolation: no libcodekarl artifact was found to check".to_owned());
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The supervisor
// ---------------------------------------------------------------------------------------------

macro_rules! case {
    ($name:ident) => {
        (stringify!($name), $name as fn())
    };
}

/// The cases, in the order they run.
fn cases() -> Vec<(&'static str, fn())> {
    vec![
        case!(a_window_opened_and_closed_by_the_script_exits_0_and_both_shutdowns_follow_the_close),
        case!(the_demo_placeholder_is_drawn_with_the_quads_generation_in_the_first_frame_and_the_run_closes_cleanly),
        case!(sigterm_after_window_opened_shuts_down_cleanly_and_exits_143),
        case!(backend_glfw_vulkan_over_a_configuration_naming_native_metal_opens_the_glfw_window),
        case!(the_shipped_demonstration_configurations_run_headless),
        case!(the_demo_scene_runs_unchanged_on_glfw_vulkan_and_closes_by_a_marked_escape),
        case!(pending_textures_become_ready_and_the_run_reconciles_to_zero),
        case!(a_missing_file_stays_a_failed_checker_quad_and_the_run_ends_normally),
        case!(moving_a_quad_for_many_updates_uploads_nothing_but_a_reload_does),
        case!(destroying_the_quad_and_releasing_the_texture_retires_it_before_the_run_ends),
        case!(update_driven_worker_continues_after_synthetic_focus_loss),
        case!(synthetic_focus_loss_without_worker_opt_in_keeps_the_pause),
        case!(the_configured_bindless_slot_count_reaches_the_presenter_and_the_final_record),
        case!(a_marked_run_writes_one_metrics_record_with_the_window_beside_the_totals),
        case!(a_run_without_a_mark_reports_the_totals_only),
        case!(the_validation_output_rule_accepts_only_a_marked_record_naming_the_tolerated_vuid),
        case!(a_minimised_window_is_restored_and_the_next_update_closes_the_run),
        case!(sigterm_while_the_window_is_minimised_exits_143_with_a_proven_presenter_end),
    ]
}

/// Runs one case, catching its panic; returns whether it passed.
fn run_case(name: &str, case: fn()) -> bool {
    assert!(
        REAL_FOCUS_GATE_CASES
            .iter()
            .all(|name| cases().iter().any(|(case, _)| *case == *name)),
        "every named real-focus case must exist"
    );
    ALLOW_UNFOCUSED_UPDATES.store(!REAL_FOCUS_GATE_CASES.contains(&name), Ordering::SeqCst);
    QUEUE_FOCUS_LOST_FOR_CK163.store(
        matches!(
            name,
            "update_driven_worker_continues_after_synthetic_focus_loss"
                | "synthetic_focus_loss_without_worker_opt_in_keeps_the_pause"
        ),
        Ordering::SeqCst,
    );
    let started = Instant::now();
    let passed = panic::catch_unwind(AssertUnwindSafe(case)).is_ok();
    ALLOW_UNFOCUSED_UPDATES.store(false, Ordering::SeqCst);
    QUEUE_FOCUS_LOST_FOR_CK163.store(false, Ordering::SeqCst);
    println!(
        "test {name} [{}] ... {} ({:.1}s)",
        mode().word(),
        if passed { "ok" } else { "FAILED" },
        started.elapsed().as_secs_f64()
    );
    passed
}

fn supervisor(arguments: &[String]) -> ExitCode {
    let all = cases();
    if arguments.iter().any(|argument| argument == "--list") {
        for (name, _) in &all {
            println!("{name}: test");
        }
        return ExitCode::SUCCESS;
    }
    // Plain words select cases by substring, as the default harness does; other flags are ignored.
    let filters: Vec<&str> = arguments
        .iter()
        .map(String::as_str)
        .filter(|argument| !argument.starts_with("--"))
        .collect();
    let selected: Vec<_> = all
        .into_iter()
        .filter(|(name, _)| {
            filters.is_empty() || filters.iter().any(|filter| name.contains(filter))
        })
        .collect();
    let mut failures = Vec::new();
    if let Err(error) = input_filter_isolation() {
        println!("test input_filter_isolation ... FAILED\n{error}");
        failures.push("input_filter_isolation".to_owned());
    } else {
        println!("test input_filter_isolation ... ok");
    }
    let mut passed = 0;
    println!(
        "running {} cases, each on a quiet keyboard and with typing",
        selected.len()
    );
    for typing in [Mode::Quiet, Mode::Typing] {
        MODE.store(typing.code(), Ordering::SeqCst);
        for (name, case) in &selected {
            if run_case(name, *case) {
                passed += 1;
            } else {
                failures.push(format!("{name} [{}]", typing.word()));
            }
        }
    }
    println!(
        "\ntest result: {}. {passed} passed; {} failed",
        if failures.is_empty() { "ok" } else { "FAILED" },
        failures.len()
    );
    for failure in &failures {
        println!("  failed: {failure}");
    }
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let first = arguments.next();
    if first.as_deref() == Some(OsStr::new(WORKER_FLAG)) {
        return worker(arguments.collect());
    }
    let arguments: Vec<String> = first
        .into_iter()
        .chain(arguments)
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    supervisor(&arguments)
}
