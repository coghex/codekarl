//! `codekarl` with a real native window and Metal, end to end, run **in process** behind a test-only keyboard filter
//! (builds with the `metal` feature on macOS).
//!
//! The windowed cases need a desktop session and a GPU: they **fail rather than skip** without them (owner).
//!
//! **The program under test is the library's `main_entry`, not the shipped binary.** `AppKit` needs the process's
//! main thread, so this executable is a **supervisor and a worker** (`harness = false`), like `ck-render-2d`'s
//! `metal_window`. The supervisor starts this executable again as the worker for each case, with a whole-process
//! deadline, kills and reaps it on every path (also when a case panics), and checks its output and exit status as
//! the cases always did. The worker recognises its own flag (`--ck-windowed-test-worker`) in **its** `main`, takes the
//! arguments after its own, and calls `codekarl::main_entry` with them: the application never sees the worker flag,
//! and the worker process **is** the application plus the filter.
//!
//! **The worker's order matters.** The application installs its signal mask on the main thread before any other thread
//! exists, so every later thread inherits blocked SIGINT and SIGTERM. A thread created earlier would not, and a
//! signal delivered to it would take the default action instead of reaching the application's `sigwait` thread. The
//! typing helper (a global dispatch task) and the framework can create threads, so the worker does, in order:
//! 1. block SIGINT and SIGTERM on the main thread (`pthread_sigmask`), before any framework call, filter, typist or
//!    other thread;
//! 2. install the keyboard filter;
//! 3. in a typing run, start the typist;
//! 4. start a reporter thread (it writes what the filter dropped and what the helper threads' signal masks are to the
//!    report file the supervisor reads), then call `main_entry`.
//!
//! **The owner may be typing while this runs, Escape included.** The filter (`tests/native/testing.m`, linked only
//! into the integration tests) drops every unmarked keyboard event before it reaches a window, its key equivalents or
//! a responder, so the cases need no quiet keyboard and none can be closed, or beeped at, by the owner's keys. Every
//! case runs twice: on a quiet keyboard and with a thread typing without stopping (Escape, repeats, key-ups, modifier
//! changes). A case fails if a close request from a key reached the application, and (in the typing run) a case that
//! opened a window waits until the filter has dropped keys. The fixture `ui` scripts still register an
//! `on_close_request` that **declines** and logs the source, and records are looked for in order with such records
//! allowed between them, as before.
//!
//! The supervisor also proves the filter's symbols are in the test-only archive and in neither the `codekarl` binary
//! nor the library's artifact, and that a worker and the binary give byte-identical output for the same arguments.
//! `-- --controls` runs the two negative controls once (they are not part of the suite: one lets typing reach the
//! application on purpose).
//!
//! The production binary has no test hook: the scripts leave files with Lua's standard `io.open`.

#![cfg(all(feature = "metal", target_os = "macos"))]

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

#[path = "supervisor/mod.rs"]
mod supervisor;
use supervisor::{Seen, Split, Supervised};
#[path = "../../../crates/window/tests/support/activation_watch.rs"]
mod activation_watch;
#[path = "supervisor/signal.rs"]
mod signal;

/// The longest a window child may run.
const DEADLINE: Duration = Duration::from_secs(60);
/// How long a process that should end at once is given.
const PROMPT: Duration = Duration::from_secs(10);

/// The worker's own flag: the first argument of a worker, followed by the mode, the report file and then the
/// application's arguments.
const WORKER_FLAG: &str = "--ck-windowed-test-worker";
/// Internal worker argument, consumed here rather than passed to the application.
const ALLOW_UNFOCUSED_UPDATES_FLAG: &str = "--ck-windowed-test-allow-unfocused-updates";
const QUEUE_FOCUS_LOST_FOR_CK163_FLAG: &str = "--ck-windowed-test-queue-focus-lost-for-ck163";
const REAL_FOCUS_GATE_CASES: [&str; 1] =
    ["synthetic_focus_loss_without_worker_opt_in_keeps_the_pause"];

unsafe extern "C" {
    fn ck_app_t_install_input_filter();
    fn ck_app_t_dropped_keys() -> u64;
    fn ck_app_t_start_typist();
    fn ck_app_t_typist_mask() -> i32;
}

/// What a worker runs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The filter, no typing.
    Quiet,
    /// The filter, and a thread typing without stopping.
    Typing,
    /// Control: typing with the filter **not** installed.
    TypingWithoutFilter,
    /// Control: the signal block (step 1) after the typist starts.
    TypingBlockLate,
}

impl Mode {
    const fn word(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Typing => "typing",
            Self::TypingWithoutFilter => "typing-without-filter",
            Self::TypingBlockLate => "typing-block-late",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        [
            Self::Quiet,
            Self::Typing,
            Self::TypingWithoutFilter,
            Self::TypingBlockLate,
        ]
        .into_iter()
        .find(|mode| mode.word() == word)
    }

    const fn typing(self) -> bool {
        !matches!(self, Self::Quiet)
    }

    const fn code(self) -> u8 {
        self as u8
    }

    fn from_code(code: u8) -> Self {
        [
            Self::Quiet,
            Self::Typing,
            Self::TypingWithoutFilter,
            Self::TypingBlockLate,
        ][usize::from(code)]
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

/// What the worker's reporter thread publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Report {
    /// Keys the filter dropped.
    dropped: u64,
    /// What the typist's helper thread saw of its own signal mask (0: not run; see [`MASK_BLOCKED`]).
    typist: i32,
    /// The same for the reporter thread itself.
    helper: i32,
}

/// The bits of a mask report: 1 for "reported", 2 if SIGINT was blocked and 4 if SIGTERM was.
const MASK_BLOCKED: i32 = 1 | 2 | 4;

/// The calling thread's signal mask as a report (1, 3, 5 or 7), or -1 if it cannot be read.
fn mask_bits() -> i32 {
    let mut current = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: a null new set only reads the calling thread's mask into `current`.
    let read =
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), current.as_mut_ptr()) };
    if read != 0 {
        return -1;
    }
    // SAFETY: the successful `pthread_sigmask` initialised `current`.
    let current = unsafe { current.assume_init() };
    let mut bits = 1;
    // SAFETY: `current` is an initialised set and SIGINT a valid signal number.
    if unsafe { libc::sigismember(&raw const current, libc::SIGINT) } == 1 {
        bits |= 2;
    }
    // SAFETY: `current` is an initialised set and SIGTERM a valid signal number.
    if unsafe { libc::sigismember(&raw const current, libc::SIGTERM) } == 1 {
        bits |= 4;
    }
    bits
}

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

/// The reporter thread: publishes the filter's drop count and the masks of the typist's helper and of itself to
/// `path` (written whole and renamed, so a reader never sees half a line), every 20 ms, until the process ends.
fn spawn_reporter(path: PathBuf) {
    thread::spawn(move || {
        let helper = mask_bits();
        let temporary = path.with_extension("tmp");
        loop {
            // SAFETY: reads an atomic counter.
            let dropped = unsafe { ck_app_t_dropped_keys() };
            // SAFETY: reads an atomic.
            let typist = unsafe { ck_app_t_typist_mask() };
            let line = format!("dropped={dropped} typist={typist} helper={helper}\n");
            if fs::write(&temporary, line).is_ok() {
                let _ = fs::rename(&temporary, &path);
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
}

/// The reporter's latest report, if it has written one.
fn read_report(path: &Path) -> Option<Report> {
    let text = fs::read_to_string(path).ok()?;
    let field = |name: &str| -> Option<i64> {
        text.split_whitespace()
            .find_map(|part| part.strip_prefix(name)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some(Report {
        dropped: u64::try_from(field("dropped")?).ok()?,
        typist: i32::try_from(field("typist")?).ok()?,
        helper: i32::try_from(field("helper")?).ok()?,
    })
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
    let report = PathBuf::from(report);
    if mode == Mode::TypingBlockLate {
        // Control: a helper thread that exists before the block does not inherit it.
        spawn_reporter(report.clone());
    } else {
        // Step 1, before anything that can create a thread.
        block_signals();
    }
    if mode != Mode::TypingWithoutFilter {
        // Step 2, before the application's first event dispatch (inside `main_entry`).
        // SAFETY: main thread; installs one local event monitor, once, before the first window.
        unsafe { ck_app_t_install_input_filter() };
    }
    activation_watch::install_report(&report.with_extension("activation"), false);
    if mode.typing() {
        // Step 3.
        // SAFETY: main thread, after the filter; starts one thread that only posts events.
        unsafe { ck_app_t_start_typist() };
    }
    if mode == Mode::TypingBlockLate {
        block_signals();
    } else {
        // Step 4: created after step 1, so it inherits the block, like the typist's helper.
        spawn_reporter(report);
    }
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

/// Lua helpers for the fixture scripts: `touch(path)` leaves a marker file, `wait_for(path)` polls for one
/// in a loop that fails the script after 5 seconds of CPU time.
const LUA_HELPERS: &str = r"
local function touch(path)
  local file = assert(io.open(path, 'w'))
  file:write('x')
  file:close()
end
local function wait_for(path)
  local started = os.clock()
  while true do
    local file = io.open(path, 'r')
    if file then
      file:close()
      return
    end
    if os.clock() - started > 5 then
      error('timed out waiting for ' .. path)
    end
  end
end
";

/// The close callback every windowed fixture registers: a real Escape or close-button press, from an owner
/// who is typing, is declined and logged, so it changes nothing.
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

    /// Lets a script that waits on `HOLD` go on. A windowed case holds its window open (the script waits for this
    /// file) until the supervisor has seen the window open and, in a typing run, the filter drop keys.
    fn release(&self) {
        fs::write(self.path("hold"), "x").expect("the hold file");
    }

    /// The `ui` script `body` after the helpers and the declining callback, with the shutdown logs. `HOLD` names the
    /// file [`release`](Self::release) writes.
    fn windowed(&self, extra_config: &str, body: &str) -> PathBuf {
        let ui = format!(
            "{LUA_HELPERS}\nlocal HOLD = {}\n{DECLINE}\n{body}\nfunction shutdown()\n  log.info('ui shutdown')\n  touch({})\nend\n",
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

    /// A windowed configuration (native-metal) running `body`.
    fn native(&self, body: &str) -> PathBuf {
        self.windowed("[window]\nbackend = \"native-metal\"\n", body)
    }
}

// ---------------------------------------------------------------------------------------------
// Supervision (the shared bounded supervisor: `tests/supervisor`)
// ---------------------------------------------------------------------------------------------

/// A worker under supervision, with the report file its reporter thread writes.
struct Worker {
    live: Supervised,
    report: PathBuf,
    /// Keeps the report's directory until the case is over.
    _scratch: Scratch,
}

impl Worker {
    fn report(&self) -> Option<Report> {
        read_report(&self.report)
    }
}

/// Starts a worker (in the current mode) running the application with `args` under the supervisor.
fn start(args: &[&OsStr]) -> Worker {
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
    let live = Supervised::spawn(command, DEADLINE).unwrap_or_else(|error| panic!("{error}"));
    Worker {
        live,
        report,
        _scratch: scratch,
    }
}

/// Waits until the child's output contains `needle`.
fn wait_for(worker: &mut Worker, needle: &str, within: Duration) {
    worker
        .live
        .wait_for(needle, within)
        .unwrap_or_else(|error| panic!("{error}"));
}

/// In a typing run, waits until the filter has dropped keys, so the case proves something about typing (a quiet
/// run needs nothing). Bounded.
fn wait_for_drops(worker: &Worker) {
    if !mode().typing() {
        return;
    }
    let give_up = Instant::now() + Duration::from_secs(30);
    while worker.report().is_none_or(|report| report.dropped == 0) {
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

fn signal_target_refuses_zero_pid_or_unowned_child() {
    assert!(signal::target(0).is_err());
    assert!(signal::live_pid(None, 1).is_err());
}

/// Waits for the worker to end and reads everything it wrote; a deadline, a failed poll, kill or reap and a read
/// error fail the case. A close request from a key that reached the application fails it too: the filter drops
/// every unmarked key, so none may.
fn finish(worker: Worker) -> Seen {
    let activation = worker.report.with_extension("activation");
    let seen = worker
        .live
        .finish()
        .unwrap_or_else(|error| panic!("{error}"));
    activation_watch::assert_report(&activation);
    assert!(
        !seen.stdout.contains("declined close request from key"),
        "a key reached the application:\n{}",
        seen.stdout
    );
    seen
}

/// Runs the application on `config` (and `extra` arguments) to its end.
fn run(config: &Path, extra: &[&str]) -> Seen {
    let mut args: Vec<&OsStr> = vec![OsStr::new("--config"), config.as_os_str()];
    args.extend(extra.iter().map(OsStr::new));
    finish(start(&args))
}

/// Runs a windowed `config`, holding its window open (the script waits for `fixture.release()`) until the window
/// is open and, in a typing run, the filter has dropped keys, then lets it go on to its end.
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

/// Asserts that each needle occurs in `text` after the previous one (other records, such as declined
/// close requests, may sit between them).
fn assert_in_order(text: &str, needles: &[&str]) {
    let mut from = 0;
    for needle in needles {
        match text[from..].find(needle) {
            Some(at) => from += at + needle.len(),
            None => panic!("{needle:?} not found in order after offset {from} in:\n{text}"),
        }
    }
}

/// Asserts that every needle occurs after `after` and before `before` (in any order among themselves: the
/// VMs' stop requests are sent together, so their records may come in either order).
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

fn the_demo_placeholder_is_drawn_with_the_quads_generation_in_the_first_frame_and_the_run_closes_cleanly()
 {
    let fixture = Fixture::new();
    let go = fixture.path("go");
    // The quad is placed before the window opens (scene generation 1; the attach makes generation 2, which
    // carries it), then the window stays open until the test has seen the first frame's record and says go.
    let ui = format!(
        "render.quad{{image='demo-quad.png', x=10, y=10, width=128, height=128}}\n\
         window.open{{title='quad', width=200, height=150, clear={{0.1, 0.2, 0.3}}}}\n\
         window.on_opened(function()\n  wait_for({})\n  window.close()\nend)",
        lua_path(&go)
    );
    let config = fixture.windowed(
        &format!(
            "[window]\nbackend = \"native-metal\"\n\n[render]\nfilter = \"nearest\"\nassets = '{}'\n\n\
             [log.categories]\nrender = \"debug\"\n",
            shipped_assets().display()
        ),
        &ui,
    );
    let mut live = start(&[OsStr::new("--config"), config.as_os_str()]);
    wait_for(
        &mut live,
        "render: first frame drawn, generation 2",
        Duration::from_secs(30),
    );
    wait_for_drops(&live);
    fs::write(&go, "x").expect("the go file");
    let seen = finish(live);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    // The first frame is reported to the loop before the UI is told the window opened; the scene counts are
    // written once the stop phase is over.
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
    assert!(!seen.stdout.contains("ERROR"), "{}", seen.stdout);
}

fn a_window_opened_and_closed_by_the_script_exits_0_and_both_shutdowns_follow_the_close() {
    let fixture = Fixture::new();
    let config = fixture.native(
        "window.open{title='close me', width=200, height=150, clear={0.2, 0.3, 0.4}}\nwindow.on_opened(function() wait_for(HOLD) window.close() end)",
    );
    let seen = run_windowed(&fixture, &config, &[]);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "runtime: window: native-metal",
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

fn sigterm_after_window_opened_shuts_down_cleanly_and_exits_143() {
    let fixture = Fixture::new();
    let config =
        fixture.native("window.open{title='wait', width=200, height=150, clear={0, 0, 0}}");
    let mut live = start(&[OsStr::new("--config"), config.as_os_str()]);
    wait_for(&mut live, "window: window opened", Duration::from_secs(30));
    wait_for_drops(&live);
    signal(&mut live, libc::SIGTERM);
    let seen = finish(live);
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

/// The ui body of the late-failure cases: closes the window, waits on the gate file, then raises.
fn late_failure_body(gate: &Path) -> String {
    format!(
        "window.open{{title='late', width=200, height=150, clear={{0, 0, 0}}}}\n\
         window.on_opened(function()\n  wait_for(HOLD)\n  window.close()\n  wait_for({})\n  error('late callback failure')\nend)",
        lua_path(gate)
    )
}

fn a_callback_failing_within_the_drain_after_the_close_exits_1_with_its_traceback() {
    let fixture = Fixture::new();
    let gate = fixture.path("gate");
    let config = fixture.native(&late_failure_body(&gate));
    let mut live = start(&[OsStr::new("--config"), config.as_os_str()]);
    wait_for(&mut live, "window: window opened", Duration::from_secs(30));
    wait_for_drops(&live);
    fixture.release();
    wait_for(&mut live, "window: window closed", Duration::from_secs(30));
    // Released well within the 1 s drain.
    fs::write(&gate, "x").expect("the gate file");
    let seen = finish(live);
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "window: window closed",
            "run failed: ui callback opened failed: ",
            "late callback failure",
            "stack traceback",
            "runtime: shutdown complete",
        ],
    );
    // The fixture's own callback ended within the drain. A close request the owner's Escape queued behind
    // it (declined by the fixture) is not this test's business: only the fixture's own warning is looked for.
    assert!(
        !seen
            .stdout
            .contains("ui callback opened still running at shutdown"),
        "{}",
        seen.stdout
    );
    assert_eq!(count(&seen.stdout, "run failed:"), 1, "{}", seen.stdout);
}

fn a_callback_held_beyond_the_drain_and_failing_in_the_stop_phase_still_exits_1_with_its_traceback()
{
    let fixture = Fixture::new();
    let gate = fixture.path("gate");
    let config = fixture.native(&late_failure_body(&gate));
    let mut live = start(&[OsStr::new("--config"), config.as_os_str()]);
    wait_for(&mut live, "window: window opened", Duration::from_secs(30));
    wait_for_drops(&live);
    fixture.release();
    // The drain gives up after 1 s and warns; only then is the callback released, in the stop phase.
    wait_for(
        &mut live,
        "ui callback opened still running at shutdown",
        Duration::from_secs(30),
    );
    fs::write(&gate, "x").expect("the gate file");
    let seen = finish(live);
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "window: window closed",
            "ui callback opened still running at shutdown",
            "run failed: ui callback opened failed: ",
            "late callback failure",
            "stack traceback",
        ],
    );
    assert_eq!(
        count(&seen.stdout, "run failed:"),
        1,
        "one record: {}",
        seen.stdout
    );
    // Exactly the fixture's own warning. A real Escape queued behind the held callback adds a legitimate
    // `close_request` warning of its own (declined by the fixture), which is tolerated, not counted.
    assert_eq!(
        count(&seen.stdout, "ui callback opened still running at shutdown"),
        1,
        "{}",
        seen.stdout
    );
    assert!(
        count(&seen.stdout, "still running at shutdown") >= 1,
        "{}",
        seen.stdout
    );
}

fn an_error_in_on_opened_logs_its_traceback_closes_the_window_and_exits_1() {
    let fixture = Fixture::new();
    let config = fixture.native(
        "window.open{title='boom', width=200, height=150, clear={0, 0, 0}}\nwindow.on_opened(function() wait_for(HOLD) error('opened exploded') end)",
    );
    let seen = run_windowed(&fixture, &config, &[]);
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert_eq!(count(&seen.stdout, "run failed:"), 1, "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "window: window closed",
            "run failed: ui callback opened failed: ",
            "opened exploded",
            "stack traceback",
            "runtime: shutdown complete",
        ],
    );
    assert_between(
        &seen.stdout,
        "window: window closed",
        "runtime: shutdown complete",
        &["ui shutdown", "gameplay shutdown"],
    );
    assert!(fixture.has("ui_shutdown") && fixture.has("gameplay_shutdown"));
}

/// A script that exercises the window API's errors and logs each message, then closes a window if there
/// is one. The same file runs windowed and headless.
const CHECKS: &str = r"
local function check(name, call)
  local ok, err = pcall(call)
  if ok then
    log.info('check ' .. name .. ': ok')
  else
    log.info('check ' .. name .. ': ' .. (tostring(err):match('^[^\n]*')))
  end
end
check('size', function() window.open{title='t', width=0, height=10, clear={0, 0, 0}} end)
check('colour', function() window.open{title='t', width=10, height=10, clear={0, 0, 1/0}} end)
check('first', function() window.open{title='t', width=200, height=150, clear={0, 0, 0}} end)
check('second', function() window.open{title='t', width=200, height=150, clear={0, 0, 0}} end)
window.on_opened(function() wait_for(HOLD) window.close() end)
";

/// The `check` records of a run.
fn checks(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.split_once("check ").map(|(_, rest)| rest.to_owned()))
        .collect()
}

fn the_same_script_gives_the_same_errors_headless_and_windowed_and_headless_logs_one_ignored_record()
 {
    let headless = Fixture::new();
    let config = headless.windowed("", CHECKS);
    let seen = run(&config, &[]);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert!(
        seen.stdout.contains("runtime: window: none (headless)"),
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(&seen.stdout, "window: window.open ignored: headless run"),
        1,
        "one record: {}",
        seen.stdout
    );
    assert!(
        !seen.stdout.contains("window opened"),
        "no window: {}",
        seen.stdout
    );
    let headless_checks = checks(&seen.stdout);
    assert_eq!(headless_checks.len(), 4, "{headless_checks:?}");

    let windowed = Fixture::new();
    let config = windowed.native(CHECKS);
    let seen = run_windowed(&windowed, &config, &[]);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_eq!(
        checks(&seen.stdout),
        headless_checks,
        "the same Lua errors, windowed and headless"
    );
    assert!(
        headless_checks[0].contains("width 0 is not from 1 to 16384"),
        "{headless_checks:?}"
    );
    assert!(
        headless_checks[1].contains("colour component 2 is not finite"),
        "{headless_checks:?}"
    );
    assert_eq!(headless_checks[2], "first: ok");
    assert!(
        headless_checks[3].contains("the window was already opened"),
        "{headless_checks:?}"
    );
}

/// A copy of the shipped `config/` and `scripts/` in a scratch directory.
fn shipped(fixture: &Fixture) -> PathBuf {
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).expect("a directory");
        for entry in fs::read_dir(from).expect("a readable directory") {
            let entry = entry.expect("an entry");
            let target = to.join(entry.file_name());
            if entry.file_type().expect("a file type").is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), &target).expect("a copied file");
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let tree = fixture.path("tree");
    copy_tree(&root.join("config"), &tree.join("config"));
    copy_tree(&root.join("scripts"), &tree.join("scripts"));
    tree.join("config/codekarl.toml")
}

fn the_shipped_configuration_runs_headless_with_the_window_call_ignored() {
    let fixture = Fixture::new();
    let config = shipped(&fixture);
    let seen = run(&config, &["--headless"]);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert!(
        seen.stdout.contains("runtime: window: none (headless)"),
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(&seen.stdout, "window.open ignored: headless run"),
        1,
        "{}",
        seen.stdout
    );
    assert!(!seen.stdout.contains("ERROR"), "{}", seen.stdout);
}

fn the_demonstration_configurations_load_and_run_headless() {
    for name in ["demo-decline", "demo-callback-error", "demo-scene"] {
        let fixture = Fixture::new();
        let root = shipped(&fixture);
        let config = root.with_file_name(format!("{name}.toml"));
        let seen = run(&config, &["--headless"]);
        assert_eq!(seen.code, Some(0), "{name}: {}", seen.stdout);
        assert!(
            seen.stdout.contains("window.open ignored: headless run"),
            "{name}: {}",
            seen.stdout
        );
    }
}

/// The reverse override (builds with both features): `--backend native-metal` over a configuration that
/// names `glfw-vulkan` opens the **native** window, so this case is native coverage, run by the native
/// window group and never by a Vulkan-only plan.
#[cfg(feature = "vulkan")]
fn backend_native_metal_over_a_glfw_vulkan_configuration_opens_the_native_window() {
    let fixture = Fixture::new();
    let config = fixture.windowed(
        "[window]\nbackend = \"glfw-vulkan\"\n",
        "window.open{title='native over vulkan', width=200, height=150, clear={0.2, 0.3, 0.4}}\nwindow.on_opened(function() wait_for(HOLD) window.close() end)",
    );
    let seen = run_windowed(&fixture, &config, &["--backend", "native-metal"]);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "runtime: window: native-metal",
            "window: window opened",
            "window: window closed",
            "runtime: shutdown complete",
        ],
    );
    assert!(
        !seen.stdout.contains("glfw-vulkan"),
        "the GLFW path was not opened: {}",
        seen.stdout
    );
    assert!(!seen.stdout.contains("ERROR"), "{}", seen.stdout);
}

// ---------------------------------------------------------------------------------------------
// Worker and binary equivalence, signals in typing workers
// ---------------------------------------------------------------------------------------------

/// Runs a (non-window) command to its end under the supervisor ([`Supervised::spawn_split`]): a whole-process
/// deadline, one cleanup on every path (the process killed and reaped with a bounded poll, both readers visited
/// against one grace, a reader that does not end reported and kept in the leak registry) and the two streams as
/// separate raw bytes.
fn captured(command: &mut Command) -> Split {
    Supervised::spawn_split(command, Duration::from_secs(30))
        .and_then(Supervised::finish_split)
        .unwrap_or_else(|error| panic!("{error}"))
}

/// The supervisor's raw split capture keeps the streams apart and byte for byte (a non-UTF-8 byte included). The
/// error paths (a deadline with a separately held pipe, kill and poll failures, read errors, partial spawns,
/// unwinds) are the supervisor tests' (`src/boundary.rs`), which run without a window.
fn the_split_capture_keeps_the_streams_apart_and_byte_for_byte() {
    let mut both = Command::new("sh");
    both.args(["-c", "printf 'out\\377'; printf 'err' >&2; exit 3"]);
    let seen = captured(&mut both);
    assert_eq!(seen.stdout, [b'o', b'u', b't', 0xff]);
    assert_eq!(seen.stderr, b"err");
    assert_eq!(seen.code, Some(3));
}

/// For `--help`, `--version`, an unknown option, a missing configuration and an application argument equal to the
/// worker's own flag, the worker and the shipped binary give byte-identical stdout and stderr and the same exit
/// status: the library never sees the worker's prefix, and it is the same code.
fn the_worker_and_the_binary_give_byte_identical_output() {
    let missing = "/nonexistent/ck131/missing.toml";
    let argument_lists: [&[&str]; 5] = [
        &["--help"],
        &["--version"],
        &["--bogus-option"],
        &["--config", missing],
        // A case argument equal to the worker's flag text, after the worker's own prefix, is an ordinary
        // (unknown-option) argument to the application.
        &[WORKER_FLAG],
    ];
    let exe = env::current_exe().expect("this executable's path");
    for arguments in argument_lists {
        let scratch = Scratch::new();
        let binary = captured(Command::new(BINARY).args(arguments));
        let worker = captured(
            Command::new(&exe)
                .arg(WORKER_FLAG)
                .arg(mode().word())
                .arg(scratch.0.join("report"))
                .args(arguments),
        );
        assert_eq!(
            worker.stdout,
            binary.stdout,
            "{arguments:?}: stdout differs:\nworker: {}\nbinary: {}",
            String::from_utf8_lossy(&worker.stdout),
            String::from_utf8_lossy(&binary.stdout)
        );
        assert_eq!(
            worker.stderr, binary.stderr,
            "{arguments:?}: stderr differs"
        );
        assert_eq!(
            worker.code, binary.code,
            "{arguments:?}: exit status differs"
        );
    }
    // The flag text really is an unknown-option failure, in both.
    let flag = captured(Command::new(BINARY).arg(WORKER_FLAG));
    assert_eq!(flag.code, Some(1));
    assert!(String::from_utf8_lossy(&flag.stdout).contains("unknown arguments"));
}

/// A second `SIGTERM` while the first one's shutdown is still running ends the worker at once with status 3
/// (the existing immediate exit), also in a typing run.
fn two_sigterms_end_the_worker_at_once_with_3() {
    let fixture = Fixture::new();
    let gate = fixture.path("never");
    let ui = format!(
        "{LUA_HELPERS}\n{DECLINE}\nwindow.open{{title='second', width=200, height=150, clear={{0, 0, 0}}}}\nfunction shutdown()\n  log.info('ui shutdown waiting')\n  wait_for({})\nend\n",
        lua_path(&gate)
    );
    let ui = fixture.scratch.file("ui.lua", &ui);
    let config = fixture.scratch.file(
        "config.toml",
        &format!(
            "[log]\ntimestamps = false\ncolour = \"never\"\n\n[window]\nbackend = \"native-metal\"\n\n[lua.ui]\nentry = '{}'\n",
            ui.display()
        ),
    );
    let mut worker = start(&[OsStr::new("--config"), config.as_os_str()]);
    wait_for(
        &mut worker,
        "window: window opened",
        Duration::from_secs(30),
    );
    wait_for_drops(&worker);
    signal(&mut worker, libc::SIGTERM);
    wait_for(&mut worker, "received SIGTERM, quitting", PROMPT);
    wait_for(&mut worker, "ui shutdown waiting", PROMPT);
    signal(&mut worker, libc::SIGTERM);
    let seen = finish(worker);
    assert_eq!(seen.code, Some(3), "{}", seen.stdout);
}

/// The typist's helper thread (reporting from inside its dispatch task) and the reporter thread run with SIGINT
/// and SIGTERM blocked, so a signal stays pending until the application's `sigwait` thread takes it; then the first
/// `SIGTERM` gives the graceful outcome.
fn helper_threads_run_with_sigint_and_sigterm_blocked() {
    let fixture = Fixture::new();
    let config =
        fixture.native("window.open{title='mask', width=200, height=150, clear={0, 0, 0}}");
    let mut worker = start(&[OsStr::new("--config"), config.as_os_str()]);
    wait_for(
        &mut worker,
        "window: window opened",
        Duration::from_secs(30),
    );
    wait_for_drops(&worker);
    let give_up = Instant::now() + Duration::from_secs(30);
    let report = loop {
        if let Some(report) = worker.report()
            && report.helper != 0
            && (!mode().typing() || report.typist != 0)
        {
            break report;
        }
        assert!(Instant::now() < give_up, "no mask report arrived");
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(
        report.helper, MASK_BLOCKED,
        "the reporter thread: {report:?}"
    );
    if mode().typing() {
        assert_eq!(
            report.typist, MASK_BLOCKED,
            "the typist's helper: {report:?}"
        );
    }
    signal(&mut worker, libc::SIGTERM);
    let seen = finish(worker);
    assert_eq!(seen.code, Some(143), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &["received SIGTERM, quitting", "runtime: shutdown complete"],
    );
}

// ---------------------------------------------------------------------------------------------
// The guards: the filter's symbols are in the test-only archive and in no production artifact
// ---------------------------------------------------------------------------------------------

/// The names the filter exports.
const FILTER_SYMBOLS: [&str; 5] = [
    "ck_app_t_install_input_filter",
    "ck_app_t_dropped_keys",
    "ck_app_t_start_typist",
    "ck_app_t_typist_mask",
    "ck_app_t_post_key",
];

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// The filter lives in this test's archive and in neither the `codekarl` binary nor any `libcodekarl` artifact built
/// beside this executable.
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
// The negative controls (run once, with `-- --controls`; not part of the suite)
// ---------------------------------------------------------------------------------------------

/// Control 1: with the filter **not** installed, a typing run reaches the application (a close request from a key
/// is logged), so the clean typing runs of the suite prove the filter does the work.
fn control_without_the_filter() -> Result<String, String> {
    MODE.store(Mode::TypingWithoutFilter.code(), Ordering::SeqCst);
    let fixture = Fixture::new();
    let config =
        fixture.native("window.open{title='control', width=200, height=150, clear={0, 0, 0}}");
    let mut worker = start(&[OsStr::new("--config"), config.as_os_str()]);
    worker
        .live
        .wait_for("window: window opened", Duration::from_secs(30))?;
    let reached = worker
        .live
        .wait_for("declined close request from key", Duration::from_secs(30));
    signal(&mut worker, libc::SIGTERM);
    let seen = worker.live.finish()?;
    reached.map_err(|error| format!("control 1: typing did not reach the application: {error}"))?;
    Ok(format!(
        "control 1 (no filter): a close request from a key reached the application ({} such record(s)), exit {:?}",
        count(&seen.stdout, "declined close request from key"),
        seen.code
    ))
}

/// Control 2: with step 1 (the signal block) moved after the typist starts, a helper thread created before it runs
/// **unblocked**, while the same thread created after step 1 (the suite's order) is blocked.
fn control_block_after_the_typist() -> Result<String, String> {
    let mut lines = Vec::new();
    for mode in [Mode::Typing, Mode::TypingBlockLate] {
        MODE.store(mode.code(), Ordering::SeqCst);
        let fixture = Fixture::new();
        let config = fixture
            .native("window.open{title='mask control', width=200, height=150, clear={0, 0, 0}}");
        let mut worker = start(&[OsStr::new("--config"), config.as_os_str()]);
        worker
            .live
            .wait_for("window: window opened", Duration::from_secs(30))?;
        let give_up = Instant::now() + Duration::from_secs(30);
        let report = loop {
            if let Some(report) = worker.report()
                && report.helper != 0
                && report.typist != 0
            {
                break report;
            }
            if Instant::now() >= give_up {
                return Err("control 2: no mask report arrived".to_owned());
            }
            thread::sleep(Duration::from_millis(10));
        };
        signal(&mut worker, libc::SIGTERM);
        let seen = worker.live.finish()?;
        lines.push(format!(
            "control 2 ({}): typist helper mask {} (7 = reported, SIGINT and SIGTERM blocked), reporter thread mask {}, exit {:?}",
            mode.word(),
            report.typist,
            report.helper,
            seen.code
        ));
        if mode == Mode::Typing && report.helper != MASK_BLOCKED {
            return Err(format!(
                "control 2: the suite's order left a thread unblocked: {report:?}"
            ));
        }
        if mode == Mode::TypingBlockLate && report.helper == MASK_BLOCKED {
            return Err(format!(
                "control 2: blocking late did not matter: {report:?}"
            ));
        }
    }
    Ok(lines.join("\n"))
}

fn controls() -> ExitCode {
    let mut failed = false;
    for result in [
        control_without_the_filter(),
        control_block_after_the_typist(),
    ] {
        match result {
            Ok(text) => println!("{text}"),
            Err(error) => {
                println!("CONTROL FAILED: {error}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

// ---------------------------------------------------------------------------------------------
// The supervisor
// ---------------------------------------------------------------------------------------------

// ---------------------------------------------------------------------------------------------
// The texture lifecycle on a real window (milestone 5-8c)
// ---------------------------------------------------------------------------------------------
//
// The scripts drive everything themselves from their frame callback and log what they observe; the window is closed by
// the script (`window.close()`), never by a key: the fixtures keep the default bindings and the filter drops the owner's
// keys. A script closes only after the supervisor released it (`HOLD`), so a typing run proves its keys were dropped. No
// case asserts an order between `Opened`, the first-frame record and a first `ready`: a startup load may finish before the
// window opens and the first-frame report is consumed asynchronously. The no-wait and pending-checker requirements are
// owned by the component evidence (CK-116/CK-117 gated tests); the application-level checker evidence is the missing-file
// quad, whose checker-instance total in the final record is non-zero.

/// Lua helpers of the lifecycle scripts: whether the supervisor released the window, and the close once a script is done.
const LIFECYCLE_HELPERS: &str = r"
local function released()
  local file = io.open(HOLD, 'r')
  if file then
    file:close()
    return true
  end
  return false
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

/// A quad moved on every update for many updates: no texture beyond the one loaded is ever created.
const MOVEMENT: &str = r"
local token, quad
local x, ready = 20, false
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
    close_when(ready and updates > 150)
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

/// Runs a lifecycle `script` on a real window and returns what the run wrote. The run must end `Closed`, exit 0, with both
/// shutdowns after the close and no error or incomplete record.
fn lifecycle(script: &str) -> Seen {
    lifecycle_with(&lifecycle_config(), script)
}

/// [`lifecycle`] with the configuration text given (the same checks).
fn lifecycle_with(config_text: &str, script: &str) -> Seen {
    let fixture = Fixture::new();
    let config = fixture.windowed(config_text, &format!("{LIFECYCLE_HELPERS}\n{script}"));
    let seen = run_windowed(&fixture, &config, &[]);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "runtime: window: native-metal",
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

/// A windowed configuration for the lifecycle scripts: the shipped assets, nearest filtering, a paced update source and the
/// resource records at `debug`.
fn lifecycle_config() -> String {
    format!(
        "[window]\nbackend = \"native-metal\"\n\n[render]\nfilter = \"nearest\"\nassets = '{}'\n\n\
         [frame]\nupdates_per_second = 60\n\n[log.categories]\nresources = \"debug\"\n",
        shipped_assets().display()
    )
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

/// A reconciled run: nothing live or referenced after `finish_teardown`, and a complete run.
fn assert_reconciled(record: &str) {
    assert!(
        record.contains(" 0 live,") && record.contains(" 0 referenced at finish_teardown"),
        "{record}"
    );
    assert!(record.ends_with("complete"), "{record}");
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
    assert_eq!(number_after(&record, "textures created "), 3, "{record}");
    assert_reconciled(&record);
}

fn a_missing_file_stays_a_failed_checker_quad_and_the_run_ends_normally() {
    let seen = lifecycle(FAILED_INITIAL_LOAD);
    assert!(seen.stdout.contains("failed: "), "{}", seen.stdout);
    assert!(seen.stdout.contains("no-such-tile.png"), "{}", seen.stdout);
    let record = final_record(&seen.stdout);
    // One texture was created (the good one); the missing file's quad drew the checker, the application-level evidence of
    // the pending appearance.
    assert_eq!(number_after(&record, "textures created "), 1, "{record}");
    assert!(
        number_after(&record, "checker instances (most in one frame) ") >= 1,
        "{record}"
    );
    assert_reconciled(&record);
}

fn moving_a_quad_for_many_updates_uploads_nothing_but_a_reload_does() {
    // The movement case: 150+ updates move the quad; the upload store created exactly the one texture loaded.
    let moved = lifecycle(MOVEMENT);
    let record = final_record(&moved.stdout);
    assert_eq!(number_after(&record, "textures created "), 1, "{record}");
    // The record carries the upload store's measured high-water bytes: one 32 x 32 RGBA image (4096 bytes of pixels) was
    // charged, and its staging with it.
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
    assert_eq!(number_after(&record, "textures created "), 2, "{record}");
    // A reload holds the old and the new storage together: the store's peak exceeds the single-texture run's.
    assert!(
        number_after(&record, "upload texture peak ") > single,
        "the reload's peak is above the movement run's {single} bytes: {record}"
    );
    assert_reconciled(&record);
}

/// The setting reaches the presenter (CK-128): a fixture with `max_textures = 8`, `max_extra_revisions = 8` and
/// `bindless_texture_slots = 17` (the rule's exact minimum) ends with `bindless_slots=17` in the final record, the observed
/// capacity of the tables the presenter allocated; the default configuration shows 131072 (the negative control: a hard-coded
/// count would show the same number twice). Native exhaustion is not claimed: a valid configuration cannot reach it.
fn the_configured_bindless_slot_count_reaches_the_presenter_and_the_final_record() {
    let small = format!(
        "{}\n[resources]\nmax_textures = 8\nmax_extra_revisions = 8\n",
        lifecycle_config().replace("[render]\n", "[render]\nbindless_texture_slots = 17\n")
    );
    let seen = lifecycle_with(&small, MOVEMENT);
    let record = final_record(&seen.stdout);
    assert_eq!(number_after(&record, "bindless_slots="), 17, "{record}");
    assert_eq!(number_after(&record, "textures created "), 1, "{record}");
    assert_reconciled(&record);
    let default = lifecycle(MOVEMENT);
    let record = final_record(&default.stdout);
    assert_eq!(
        number_after(&record, "bindless_slots="),
        131_072,
        "{record}"
    );
}

fn destroying_the_quad_and_releasing_the_texture_retires_it_before_the_run_ends() {
    let seen = lifecycle(RELEASE);
    assert!(seen.stdout.contains("released"), "{}", seen.stdout);
    let record = final_record(&seen.stdout);
    assert_eq!(number_after(&record, "textures created "), 1, "{record}");
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
    // With the opt-in, MOVEMENT closes in under 3 seconds on this host; this longer observation
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
// Frame timing on a real window (milestone 5, issue 10b-1): the one `metrics` record
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
            "runtime: window: native-metal",
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

/// A marked run writes exactly one record with every field, the measured window beside the whole-run totals, both owners on the same
/// window, and the second mark counted.
fn a_marked_run_writes_one_metrics_record_with_the_window_beside_the_totals() {
    let seen = lifecycle_with(&metrics_config(), MARKED);
    let record = metrics_record(&seen.stdout);
    for field in [
        "run: frames=",
        " skipped=",
        " invalid_marks=",
        " invalid_gpu=",
        " presented=",
        " not_presented=",
        " excluded_intervals=",
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
        " missed=",
        "timing_lost=",
        "uploads: files_decoded=",
        " decoded_bytes=",
        " queue_depth_peak=",
        " batches_completed=",
        " textures_created(native)=",
        " textures_ready=",
        " time_to_last_ready=",
        " files_per_s=",
        " decoded_mib_per_s=",
        " uploads_per_s=",
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
    // One file was loaded, decoded and made ready: the pipeline's figures come from their own sources and the rates are numbers.
    assert_eq!(number_after(&record, "files_decoded="), 1, "{record}");
    assert!(number_after(&record, "decoded_bytes=") > 0, "{record}");
    assert!(number_after(&record, "queue_depth_peak=") >= 1, "{record}");
    assert_eq!(number_after(&record, "textures_ready="), 1, "{record}");
    assert!(!record.contains("time_to_last_ready=none"), "{record}");
    assert!(!record.contains("files_per_s=none"), "{record}");
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
}

/// A run that never marks reports the whole-run totals only.
fn a_run_without_a_mark_reports_the_totals_only() {
    let seen = lifecycle_with(&metrics_config(), MOVEMENT);
    let record = metrics_record(&seen.stdout);
    assert!(
        record.contains("window: none (no metrics_mark)"),
        "{record}"
    );
    assert!(!record.contains("presenter window"), "{record}");
    assert!(number_after(&record, "run: frames=") > 0, "{record}");
}

macro_rules! case {
    ($name:ident) => {
        (stringify!($name), $name as fn())
    };
}

/// The cases that need the `vulkan` feature as well (the reverse override of a configuration naming `glfw-vulkan`).
#[cfg(feature = "vulkan")]
fn feature_cases() -> Vec<(&'static str, fn())> {
    vec![case!(
        backend_native_metal_over_a_glfw_vulkan_configuration_opens_the_native_window
    )]
}

#[cfg(not(feature = "vulkan"))]
fn feature_cases() -> Vec<(&'static str, fn())> {
    Vec::new()
}

/// The cases, in the order they run.
fn cases() -> Vec<(&'static str, fn())> {
    let mut cases = vec![
        case!(the_demo_placeholder_is_drawn_with_the_quads_generation_in_the_first_frame_and_the_run_closes_cleanly),
        case!(a_window_opened_and_closed_by_the_script_exits_0_and_both_shutdowns_follow_the_close),
        case!(sigterm_after_window_opened_shuts_down_cleanly_and_exits_143),
        case!(a_callback_failing_within_the_drain_after_the_close_exits_1_with_its_traceback),
        case!(a_callback_held_beyond_the_drain_and_failing_in_the_stop_phase_still_exits_1_with_its_traceback),
        case!(an_error_in_on_opened_logs_its_traceback_closes_the_window_and_exits_1),
        case!(the_same_script_gives_the_same_errors_headless_and_windowed_and_headless_logs_one_ignored_record),
        case!(the_shipped_configuration_runs_headless_with_the_window_call_ignored),
        case!(the_demonstration_configurations_load_and_run_headless),
        case!(pending_textures_become_ready_and_the_run_reconciles_to_zero),
        case!(a_missing_file_stays_a_failed_checker_quad_and_the_run_ends_normally),
        case!(moving_a_quad_for_many_updates_uploads_nothing_but_a_reload_does),
        case!(destroying_the_quad_and_releasing_the_texture_retires_it_before_the_run_ends),
        case!(update_driven_worker_continues_after_synthetic_focus_loss),
        case!(synthetic_focus_loss_without_worker_opt_in_keeps_the_pause),
        case!(signal_target_refuses_zero_pid_or_unowned_child),
        case!(the_configured_bindless_slot_count_reaches_the_presenter_and_the_final_record),
        case!(a_marked_run_writes_one_metrics_record_with_the_window_beside_the_totals),
        case!(a_run_without_a_mark_reports_the_totals_only),
        case!(the_split_capture_keeps_the_streams_apart_and_byte_for_byte),
        case!(the_worker_and_the_binary_give_byte_identical_output),
        case!(two_sigterms_end_the_worker_at_once_with_3),
        case!(helper_threads_run_with_sigint_and_sigterm_blocked),
    ];
    cases.extend(feature_cases());
    cases
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
    if arguments.iter().any(|argument| argument == "--controls") {
        return controls();
    }
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
