//! `ck-lua` behaviour: the lifecycle, errors, panics, deadlines and abandonment, readiness already
//! reported, cancel, illegal calls, two owners in parallel, the host surface and stdout.
//!
//! Every wait has a deadline. Fixtures that could leave a thread running bound it (a script loop that
//! ends by the clock, or a child process that is killed and reaped). The hooks in `hooks.rs` hold the
//! owner thread at chosen points, so no test relies on a sleep to reach a state.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use mlua::{Lua, UserData};

use crate::hooks::{self, Hooks, Point};
use crate::{ConfigValue, VmError, VmOwner, VmSettings, VmState};

/// The longest a test waits for something that must happen.
const BOUND: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// A temporary directory for fixture scripts, removed when dropped.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ck-lua-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).expect("a temporary directory");
        Self(path)
    }

    fn script(&self, source: &str) -> PathBuf {
        let path = self.0.join("entry.lua");
        std::fs::write(&path, source).expect("the fixture script");
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn empty_config() -> ConfigValue {
    ConfigValue::Table(BTreeMap::new())
}

/// Spawns an owner called `name` on `source`; the directory must outlive the owner's reads.
fn spawn_with(name: &str, source: &str, config: ConfigValue) -> (VmOwner, Dir) {
    // The capture logger must be installed before any script can call `log.*`.
    let _ = buffer();
    let dir = Dir::new();
    let entry = dir.script(source);
    let owner = VmOwner::spawn(VmSettings::new(name, entry, config)).expect("the thread starts");
    (owner, dir)
}

fn spawn(name: &str, source: &str) -> (VmOwner, Dir) {
    spawn_with(name, source, empty_config())
}

/// What the capture logger wrote.
fn buffer() -> &'static Arc<Mutex<Vec<u8>>> {
    static BUFFER: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    BUFFER.get_or_init(|| {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let logger = ck_log::Logger::new(
            Box::new(Capture(Arc::clone(&buffer))),
            ck_log::Settings {
                filter: ck_log::Filter::new(ck_log::Level::Trace),
                timestamps: false,
                colour: false,
            },
        )
        .expect("the logger starts");
        ck_log::install(logger).expect("no other logger is installed in this test process");
        buffer
    })
}

struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The records written in category `lua.<name>`, as `LEVEL text`, after the logger has flushed.
fn records(name: &str) -> Vec<String> {
    let buffer = buffer();
    let logger = ck_log::installed().expect("the capture logger is installed");
    assert_eq!(
        logger.flush(BOUND),
        ck_log::FlushOutcome::Complete,
        "the capture logger flushes"
    );
    let text = String::from_utf8(
        buffer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone(),
    )
    .expect("log output is UTF-8");
    let marker = format!(" lua.{name}: ");
    text.lines()
        .filter_map(|line| {
            let (level, message) = line.split_once(&marker)?;
            Some(format!("{} {message}", level.trim()))
        })
        .collect()
}

/// A value that records when it is dropped, to show the Lua state was dropped.
struct Sentinel(Arc<AtomicBool>);

impl UserData for Sentinel {}

impl Drop for Sentinel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Puts a [`Sentinel`] into the state when it is created and returns the flag its drop sets.
fn track_state_drop(name: &str, extra: impl Fn(Point) + Send + Sync + 'static) -> Arc<AtomicBool> {
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&dropped);
    hooks::register(
        name,
        Hooks {
            at: Box::new(move |point, lua| {
                if let (Point::StateCreated, Some(lua)) = (point, lua) {
                    let sentinel = lua
                        .create_userdata(Sentinel(Arc::clone(&flag)))
                        .expect("a sentinel");
                    lua.globals().set("sentinel", sentinel).expect("a global");
                }
                extra(point);
            }),
            install: None,
        },
    );
    dropped
}

/// A one-shot hold: `arrived` fires when the thread reaches `point`, and the thread then waits for
/// the release (at most [`BOUND`], so a broken test cannot hang it).
fn hold_at(name: &str, point: Point) -> (Receiver<()>, Sender<()>) {
    let (arrived_tx, arrived) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let state = Mutex::new((Some(arrived_tx), Some(release_rx)));
    hooks::register(
        name,
        Hooks {
            at: Box::new(move |at, _| {
                if at != point {
                    return;
                }
                let (tx, rx) = {
                    let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
                    (state.0.take(), state.1.take())
                };
                if let (Some(tx), Some(rx)) = (tx, rx) {
                    let _ = tx.send(());
                    let _ = rx.recv_timeout(BOUND);
                }
            }),
            install: None,
        },
    );
    (arrived, release)
}

/// Gives the owner a global `entered()` that signals the test and returns; and counts reports.
fn signal_on_entered(name: &str) -> (Receiver<()>, Arc<AtomicUsize>) {
    let (entered_tx, entered) = mpsc::channel();
    let entered_tx = Mutex::new(entered_tx);
    let reports = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&reports);
    hooks::register(
        name,
        Hooks {
            at: Box::new(move |point, _| {
                if point == Point::Reported {
                    counted.fetch_add(1, Ordering::SeqCst);
                }
            }),
            install: Some(Box::new(move |lua: &Lua| {
                let sender = entered_tx
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                let entered = lua.create_function(move |_, ()| {
                    let _ = sender.send(());
                    Ok(())
                })?;
                add_spin(lua)?;
                lua.globals().set("entered", entered)
            })),
        },
    );
    (entered, reports)
}

fn script_error(error: &VmError, owner: &str) -> (String, String) {
    match error {
        VmError::Script {
            owner: named,
            message,
            traceback,
        } => {
            assert_eq!(named, owner);
            (message.clone(), traceback.clone())
        }
        other => panic!("expected a script error, got {other:?}"),
    }
}

/// A script call that busy-waits for `ms` of wall-clock time (`os.clock` is process CPU time, which
/// parallel tests would make race ahead). The host function behind it ends by the clock, so the
/// fixture is bounded.
fn spin(ms: u64) -> String {
    format!("spin_ms({ms})")
}

/// Adds `spin_ms(n)` to the state.
fn add_spin(lua: &Lua) -> mlua::Result<()> {
    let spin = lua.create_function(|_, ms: u64| {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            std::hint::spin_loop();
        }
        Ok(())
    })?;
    lua.globals().set("spin_ms", spin)
}

/// Gives the owner `spin_ms(n)`.
fn with_spin(name: &str) {
    hooks::register(
        name,
        Hooks {
            at: Box::new(|_, _| {}),
            install: Some(Box::new(add_spin)),
        },
    );
}

// ---------------------------------------------------------------------------------------------
// 1. Lifecycle
// ---------------------------------------------------------------------------------------------

#[test]
fn a_full_lifecycle_calls_startup_and_shutdown_and_ends_the_thread() {
    let (mut owner, _dir) = spawn(
        "life_full",
        "log.info('main')\n\
         function startup() log.info('startup') end\n\
         function shutdown() log.info('shutdown') end\n",
    );
    assert_eq!(owner.state(), VmState::Initialising);
    owner.wait_ready(BOUND).expect("ready");
    assert_eq!(owner.state(), VmState::Ready);
    owner.start(BOUND).expect("started");
    assert_eq!(owner.state(), VmState::Started);
    owner.stop(BOUND).expect("stopped");
    assert_eq!(owner.state(), VmState::Finished);
    assert!(owner.is_finished());
    assert_eq!(
        records("life_full"),
        ["INFO main", "INFO startup", "INFO shutdown"]
    );
}

#[test]
fn a_script_without_startup_and_shutdown_still_runs_the_lifecycle() {
    let (mut owner, _dir) = spawn("life_bare", "log.info('main')\n");
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    owner.stop(BOUND).expect("stopped");
    assert!(owner.is_finished());
    assert_eq!(records("life_bare"), ["INFO main"]);
}

#[test]
fn the_state_is_lua_5_4_with_the_safe_standard_libraries() {
    let (mut owner, _dir) = spawn(
        "life_libs",
        "log.info(_VERSION)\n\
         log.info(type(io) .. ' ' .. type(string) .. ' ' .. type(os) .. ' ' .. type(debug))\n",
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.stop(BOUND).expect("stopped");
    assert_eq!(
        records("life_libs"),
        ["INFO Lua 5.4", "INFO table table table nil"]
    );
}

// ---------------------------------------------------------------------------------------------
// 2. Script errors
// ---------------------------------------------------------------------------------------------

#[test]
fn a_syntax_error_and_a_main_chunk_error_are_script_errors_with_a_traceback() {
    for (name, source, expected) in [
        ("err_syntax", "this is not lua", "syntax"),
        (
            "err_main",
            "local function f() error('boom in main') end\nf()\n",
            "boom in main",
        ),
    ] {
        let dropped = track_state_drop(name, |_| {});
        let (mut owner, _dir) = spawn(name, source);
        let error = owner.wait_ready(BOUND).expect_err("initialisation fails");
        let (message, traceback) = script_error(&error, name);
        assert!(message.contains(expected), "{message}");
        assert!(!traceback.is_empty(), "{error}");
        assert_eq!(error.owner(), name);
        assert_eq!(owner.state(), VmState::Ending);
        assert!(owner.wait_finished(BOUND), "the thread ends");
        assert!(dropped.load(Ordering::SeqCst), "the state was dropped");
        assert_eq!(owner.state(), VmState::Finished);
    }
}

#[test]
fn an_error_in_startup_or_in_shutdown_is_a_script_error_and_the_thread_ends() {
    let (mut owner, _dir) = spawn(
        "err_start",
        "function startup() local x = nil; return x.field end\n",
    );
    owner.wait_ready(BOUND).expect("ready");
    let error = owner.start(BOUND).expect_err("startup fails");
    let (message, traceback) = script_error(&error, "err_start");
    assert!(message.contains("index a nil value"), "{message}");
    assert!(traceback.contains("stack traceback"), "{traceback}");
    assert_eq!(owner.state(), VmState::Ending);
    assert!(owner.wait_finished(BOUND));

    let (mut owner, _dir) = spawn(
        "err_stop",
        "function shutdown() error('shutdown boom') end\n",
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    let error = owner.stop(BOUND).expect_err("shutdown fails");
    let (message, traceback) = script_error(&error, "err_stop");
    assert!(message.contains("shutdown boom"), "{message}");
    assert!(!traceback.is_empty());
    assert!(owner.wait_finished(BOUND));
}

// ---------------------------------------------------------------------------------------------
// 3. Panic
// ---------------------------------------------------------------------------------------------

#[test]
fn a_host_panic_is_reported_and_the_thread_ends_with_its_state_dropped() {
    let name = "panic_main";
    let dropped = track_state_drop(name, |point| {
        assert!(point != Point::AfterMain, "injected host panic");
    });
    let (mut owner, _dir) = spawn(name, "log.info('main')");
    let error = owner.wait_ready(BOUND).expect_err("the owner panicked");
    match &error {
        VmError::Panicked {
            owner: named,
            message,
        } => {
            assert_eq!(named, name);
            assert!(message.contains("injected host panic"), "{message}");
        }
        other => panic!("expected a panic, got {other:?}"),
    }
    assert_eq!(owner.state(), VmState::Ending);
    assert!(owner.wait_finished(BOUND));
    assert!(dropped.load(Ordering::SeqCst), "the state was dropped");
}

// ---------------------------------------------------------------------------------------------
// 4. Deadline and abandonment
// ---------------------------------------------------------------------------------------------

#[test]
fn a_start_deadline_abandons_the_request_and_the_hook_ends_the_endless_loop() {
    // CK-55 replaces CK-53's bounded-loop test: with the hook, the abandoned `startup()` is interrupted.
    let (mut owner, _dir) = spawn(
        "deadline",
        "function startup() while true do end end\nfunction shutdown() log.info('shutdown called') end\n",
    );
    owner.wait_ready(BOUND).expect("ready");
    let deadline = Duration::from_millis(100);
    let begun = Instant::now();
    let error = owner.start(deadline).expect_err("startup never returns");
    let took = begun.elapsed();
    assert_eq!(
        error,
        VmError::Deadline {
            owner: "deadline".to_owned()
        }
    );
    assert!(
        took >= deadline && took < Duration::from_millis(400),
        "{took:?}"
    );
    assert_eq!(owner.state(), VmState::Ending);
    // Not joined by the call; later requests are refused.
    assert!(matches!(
        owner.start(BOUND),
        Err(VmError::State { call: "start", .. })
    ));
    assert!(matches!(
        owner.stop(BOUND),
        Err(VmError::State { call: "stop", .. })
    ));
    // No `cancel()` call: the abandonment alone sets the flag, and the hook unwinds the loop.
    let ended = Instant::now();
    assert!(
        owner.wait_finished(BOUND),
        "the hook interrupts the loop and the thread ends"
    );
    assert!(
        ended.elapsed() < Duration::from_secs(3),
        "{:?}",
        ended.elapsed()
    );
    assert!(
        records("deadline").is_empty(),
        "shutdown() was not called: {:?}",
        records("deadline")
    );
    assert!(matches!(
        owner.start(BOUND),
        Err(VmError::State {
            state: VmState::Finished,
            ..
        })
    ));
}

// ---------------------------------------------------------------------------------------------
// 5. Readiness already reported
// ---------------------------------------------------------------------------------------------

/// Lets the owner reach the report, and waits until it has been sent.
fn reported(name: &str) -> Receiver<()> {
    let (reported_tx, reported) = mpsc::channel();
    let reported_tx = Mutex::new(reported_tx);
    hooks::register(
        name,
        Hooks {
            at: Box::new(move |point, _| {
                if point == Point::Reported {
                    let _ = reported_tx
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .send(());
                }
            }),
            install: None,
        },
    );
    reported
}

#[test]
fn a_ready_report_that_arrived_earlier_is_consumed_at_once() {
    let reported = reported("early_ready");
    let (mut owner, _dir) = spawn("early_ready", "function startup() log.info('up') end");
    reported
        .recv_timeout(BOUND)
        .expect("the ready report was sent");
    assert_eq!(
        owner.state(),
        VmState::Initialising,
        "the handle has not consumed it"
    );
    let begun = Instant::now();
    owner
        .wait_ready(Duration::from_millis(500))
        .expect("ready at once");
    assert!(begun.elapsed() < Duration::from_millis(400));
    owner.start(BOUND).expect("start is legal now");
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn a_main_chunk_error_reported_earlier_is_returned_at_once() {
    let reported = reported("early_error");
    let (mut owner, _dir) = spawn("early_error", "error('early failure')");
    reported
        .recv_timeout(BOUND)
        .expect("the failure report was sent");
    let begun = Instant::now();
    let error = owner
        .wait_ready(Duration::from_millis(500))
        .expect_err("failed");
    assert!(begun.elapsed() < Duration::from_millis(400));
    let (message, _) = script_error(&error, "early_error");
    assert!(message.contains("early failure"));
    assert!(owner.wait_finished(BOUND));
}

// ---------------------------------------------------------------------------------------------
// 6. Cancel
// ---------------------------------------------------------------------------------------------

#[test]
fn cancel_before_the_main_chunk_ends_the_thread_and_the_chunk_never_runs() {
    let (arrived, release) = hold_at("cancel_early", Point::BeforeMain);
    let (mut owner, _dir) = spawn("cancel_early", "log.info('main ran')");
    arrived
        .recv_timeout(BOUND)
        .expect("the thread reaches the barrier");
    owner.cancel();
    assert_eq!(owner.state(), VmState::Ending);
    release.send(()).expect("the thread is waiting");
    assert!(owner.wait_finished(BOUND));
    assert!(
        records("cancel_early").is_empty(),
        "the main chunk never ran"
    );
}

#[test]
fn cancel_while_idle_ends_the_thread_without_calling_shutdown() {
    for (name, started) in [("cancel_ready", false), ("cancel_started", true)] {
        let (mut owner, _dir) = spawn(
            name,
            "function startup() log.info('startup') end\n\
             function shutdown() log.info('shutdown') end\n",
        );
        owner.wait_ready(BOUND).expect("ready");
        if started {
            owner.start(BOUND).expect("started");
        }
        owner.cancel();
        assert!(owner.wait_finished(BOUND), "an idle owner ends promptly");
        let expected: &[&str] = if started { &["INFO startup"] } else { &[] };
        assert_eq!(records(name), expected, "shutdown() is not called");
    }
}

#[test]
fn cancel_during_a_main_chunk_interrupts_it_and_no_readiness_is_reported() {
    // CK-55 replaces CK-53's "the chunk completes": a running chunk is now interrupted by the hook.
    let (rig, entered) = rig("cancel_main");
    let (mut owner, _dir) = spawn(
        "cancel_main",
        "entered()\nfor i = 1, 400000000 do end\nlog.info('main done')\nfunction startup() log.info('startup') end\n",
    );
    let canceller = cancel_on_entry("cancel_main", entered);
    let error = owner
        .wait_ready(BOUND)
        .expect_err("the chunk is interrupted, not completed");
    assert_eq!(
        error,
        VmError::Cancelled {
            owner: "cancel_main".to_owned()
        }
    );
    canceller.join().expect("the canceller ends");
    assert!(owner.wait_finished(BOUND));
    assert!(
        records("cancel_main").is_empty(),
        "the chunk did not finish: no `main done`, no startup()"
    );
    assert!(rig.dropped.load(Ordering::SeqCst), "the state was dropped");
}

#[test]
fn dropping_an_owner_never_blocks() {
    // A finished thread is joined.
    let (mut owner, _dir) = spawn("drop_done", "");
    owner.wait_ready(BOUND).expect("ready");
    owner.stop(BOUND).expect("stopped");
    let begun = Instant::now();
    drop(owner);
    assert!(begun.elapsed() < Duration::from_millis(200));

    // A thread still running a bounded loop is detached.
    let name = "drop_running";
    let (entered, _) = signal_on_entered(name);
    let (owner, _dir) = spawn(name, &format!("entered()\n{}\n", spin(400)));
    entered
        .recv_timeout(BOUND)
        .expect("the main chunk is running");
    let begun = Instant::now();
    drop(owner);
    assert!(
        begun.elapsed() < Duration::from_millis(200),
        "{:?}",
        begun.elapsed()
    );
}

// ---------------------------------------------------------------------------------------------
// 7. Out of order
// ---------------------------------------------------------------------------------------------

fn expect_state(result: Result<(), VmError>, call: &'static str, state: VmState) {
    match result {
        Err(VmError::State {
            call: refused,
            state: seen,
            ..
        }) => {
            assert_eq!((refused, seen), (call, state));
        }
        other => panic!("expected a state error for {call} in {state}, got {other:?}"),
    }
}

#[test]
fn every_illegal_call_returns_a_state_error_and_sends_nothing() {
    let name = "order";
    let (arrived, release) = hold_at(name, Point::BeforeMain);
    let (mut owner, _dir) = spawn(
        name,
        "function startup() log.info('startup') end\n\
         function shutdown() log.info('shutdown') end\n",
    );
    arrived
        .recv_timeout(BOUND)
        .expect("the thread is initialising");
    expect_state(owner.start(BOUND), "start", VmState::Initialising);
    expect_state(owner.stop(BOUND), "stop", VmState::Initialising);
    release.send(()).expect("release");
    owner.wait_ready(BOUND).expect("ready");
    expect_state(owner.wait_ready(BOUND), "wait_ready", VmState::Ready);
    owner.start(BOUND).expect("started");
    expect_state(owner.wait_ready(BOUND), "wait_ready", VmState::Started);
    expect_state(owner.start(BOUND), "start", VmState::Started);
    owner.cancel();
    expect_state(owner.wait_ready(BOUND), "wait_ready", VmState::Ending);
    expect_state(owner.start(BOUND), "start", VmState::Ending);
    expect_state(owner.stop(BOUND), "stop", VmState::Ending);
    assert!(owner.wait_finished(BOUND));
    expect_state(owner.wait_ready(BOUND), "wait_ready", VmState::Finished);
    expect_state(owner.start(BOUND), "start", VmState::Finished);
    expect_state(owner.stop(BOUND), "stop", VmState::Finished);
    assert_eq!(
        records(name),
        ["INFO startup"],
        "nothing illegal reached the script"
    );
}

// ---------------------------------------------------------------------------------------------
// 8. Two owners in parallel
// ---------------------------------------------------------------------------------------------

/// A rendezvous of `parties` threads that gives up after [`BOUND`] instead of waiting for ever.
struct Rendezvous {
    arrived: Mutex<usize>,
    changed: Condvar,
    parties: usize,
}

impl Rendezvous {
    fn meet(&self) -> bool {
        let mut arrived = self.arrived.lock().unwrap_or_else(PoisonError::into_inner);
        *arrived += 1;
        self.changed.notify_all();
        let give_up = Instant::now() + BOUND;
        while *arrived < self.parties {
            let left = give_up.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            arrived = self
                .changed
                .wait_timeout(arrived, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

fn meet_on(name: &str, rendezvous: &Arc<Rendezvous>) {
    let rendezvous = Arc::clone(rendezvous);
    hooks::register(
        name,
        Hooks {
            at: Box::new(|_, _| {}),
            install: Some(Box::new(move |lua: &Lua| {
                let rendezvous = Arc::clone(&rendezvous);
                let meet = lua.create_function(move |_, ()| {
                    if rendezvous.meet() {
                        Ok(())
                    } else {
                        Err(mlua::Error::runtime("the other owner never arrived"))
                    }
                })?;
                lua.globals().set("meet", meet)
            })),
        },
    );
}

#[test]
fn two_owners_initialise_in_parallel_and_one_failing_does_not_affect_the_other() {
    let rendezvous = Arc::new(Rendezvous {
        arrived: Mutex::new(0),
        changed: Condvar::new(),
        parties: 2,
    });
    meet_on("par_bad", &rendezvous);
    meet_on("par_good", &rendezvous);
    let (mut bad, _bad_dir) = spawn(
        "par_bad",
        "meet()\nfunction startup() error('bad owner fails') end\n",
    );
    let (mut good, _good_dir) = spawn(
        "par_good",
        "meet()\nfunction startup() log.info('good started') end\n\
         function shutdown() log.info('good stopped') end\n",
    );
    // Each main chunk waits for the other: they can only both become ready if they run at once.
    bad.wait_ready(BOUND).expect("the bad owner initialised");
    good.wait_ready(BOUND).expect("the good owner initialised");
    let error = bad
        .start(BOUND)
        .expect_err("the bad owner fails in startup");
    assert!(
        script_error(&error, "par_bad")
            .0
            .contains("bad owner fails")
    );
    good.start(BOUND).expect("the good owner starts");
    good.stop(BOUND).expect("the good owner stops cleanly");
    assert!(bad.wait_finished(BOUND));
    assert_eq!(
        records("par_good"),
        ["INFO good started", "INFO good stopped"]
    );
}

// ---------------------------------------------------------------------------------------------
// 9. Host surface
// ---------------------------------------------------------------------------------------------

#[test]
fn log_writes_every_level_in_the_owners_category() {
    let (mut owner, _dir) = spawn(
        "host_log",
        "log.error('e') log.warn('w') log.info('i') log.debug('d') log.trace('t')\n\
         log.info(42) log.info(true) log.info(nil)\n",
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.stop(BOUND).expect("stopped");
    assert_eq!(
        records("host_log"),
        [
            "ERROR e",
            "WARN w",
            "INFO i",
            "DEBUG d",
            "TRACE t",
            "INFO 42",
            "INFO true",
            "INFO nil"
        ]
    );
}

fn sample_config() -> ConfigValue {
    let mut nested = BTreeMap::new();
    nested.insert("inner".to_owned(), ConfigValue::Integer(7));
    nested.insert(
        "list".to_owned(),
        ConfigValue::Array(vec![ConfigValue::Integer(10), ConfigValue::Integer(20)]),
    );
    let mut top = BTreeMap::new();
    top.insert("name".to_owned(), ConfigValue::String("x".to_owned()));
    top.insert("count".to_owned(), ConfigValue::Integer(3));
    top.insert("ratio".to_owned(), ConfigValue::Float(1.5));
    top.insert("on".to_owned(), ConfigValue::Bool(true));
    top.insert("nested".to_owned(), ConfigValue::Table(nested));
    top.insert(
        "items".to_owned(),
        ConfigValue::Array(vec![
            ConfigValue::String("a".to_owned()),
            ConfigValue::String("b".to_owned()),
            ConfigValue::String("c".to_owned()),
        ]),
    );
    ConfigValue::Table(top)
}

const CONFIG_SCRIPT: &str = "
local function report(label, value) log.info(label .. '=' .. tostring(value)) end
report('name', config.name)
report('count', config.count)
report('ratio', config.ratio)
report('on', config.on)
report('inner', config.nested.inner)
report('list2', config.nested.list[2])
report('len', #config.items)
report('missing', config.nothing)
local keys = {}
for k in pairs(config) do keys[#keys + 1] = k end
table.sort(keys)
report('keys', table.concat(keys, ','))
local items = {}
for i, v in ipairs(config.items) do items[#items + 1] = i .. v end
report('ipairs', table.concat(items, ','))
local pairs_items = {}
for k, v in pairs(config.items) do pairs_items[#pairs_items + 1] = k .. v end
report('pairs-array', table.concat(pairs_items, ','))
local function attempt(label, f)
  local ok, err = pcall(f)
  report(label, ok and 'allowed' or 'refused')
  return err
end
attempt('set-existing', function() config.name = 'y' end)
attempt('set-new', function() config.extra = 1 end)
attempt('nested-existing', function() config.nested.inner = 8 end)
attempt('nested-new', function() config.nested.extra = 1 end)
attempt('array-set', function() config.items[1] = 'q' end)
report('rawset-error', attempt('rawset', function() rawset(config, 'name', 'z') end))
attempt('rawset-nested', function() rawset(config.nested, 'inner', 9) end)
attempt('rawset-new', function() rawset(config, 'fresh', 1) end)
attempt('after-rawset-assign', function() config.fresh = 2 end)
report('name-after', config.name)
report('inner-after', config.nested.inner)
report('fresh-after', config.fresh)
";

#[test]
fn config_reads_nested_values_and_refuses_every_write() {
    let (mut owner, _dir) = spawn_with("host_config", CONFIG_SCRIPT, sample_config());
    owner.wait_ready(BOUND).expect("ready");
    owner.stop(BOUND).expect("stopped");
    let lines = records("host_config");
    let find = |label: &str| -> String {
        let prefix = format!("INFO {label}=");
        lines
            .iter()
            .find_map(|line| line.strip_prefix(&prefix).map(str::to_owned))
            .unwrap_or_else(|| panic!("no `{label}` line in {lines:?}"))
    };
    assert_eq!(find("name"), "x");
    assert_eq!(find("count"), "3");
    assert_eq!(find("ratio"), "1.5");
    assert_eq!(find("on"), "true");
    assert_eq!(find("inner"), "7");
    assert_eq!(find("list2"), "20");
    assert_eq!(find("len"), "3");
    assert_eq!(find("missing"), "nil");
    assert_eq!(find("keys"), "count,items,name,nested,on,ratio");
    assert_eq!(find("ipairs"), "1a,2b,3c");
    assert_eq!(find("pairs-array"), "1a,2b,3c");
    for refused in [
        "set-existing",
        "set-new",
        "nested-existing",
        "nested-new",
        "array-set",
        "rawset",
        "rawset-nested",
        "rawset-new",
        "after-rawset-assign",
    ] {
        assert_eq!(find(refused), "refused", "{refused}");
    }
    assert!(find("rawset-error").contains("table expected"), "{lines:?}");
    assert_eq!(find("name-after"), "x");
    assert_eq!(find("inner-after"), "7");
    assert_eq!(find("fresh-after"), "nil");
}

#[test]
fn non_string_log_messages_convert_exactly_like_lua_tostring() {
    let (mut owner, _dir) = spawn(
        "host_tostring",
        "local values = {1.0, -0.0, 3, 0.1, 1e100, math.huge, -math.huge, true, false, 'text',\n\
           setmetatable({}, {__tostring = function() return 'custom' end})}\n\
         for _, value in ipairs(values) do log.info(value) log.info(tostring(value)) end\n\
         log.info(nil) log.info(tostring(nil))\n\
         local ok = pcall(function()\n\
           log.info(setmetatable({}, {__tostring = function() error('conversion failed') end}))\n\
         end)\n\
         log.info('conversion-error-propagates=' .. tostring(not ok))\n\
         tostring = function() return 'hijacked' end\n\
         log.info(2.0)\n",
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.stop(BOUND).expect("stopped");
    let lines = records("host_tostring");
    // Each value logged directly is the line the script's own `tostring` of it gives, next to it.
    let (pairs, rest) = lines.split_at(24);
    for pair in pairs.chunks(2) {
        assert_eq!(pair[0], pair[1], "{pairs:?}");
    }
    // Lua's own spellings (not Rust's): an integral float keeps its `.0`, negative zero its sign.
    assert_eq!(pairs[0], "INFO 1.0");
    assert_eq!(pairs[2], "INFO -0.0");
    assert_eq!(pairs[20], "INFO custom");
    assert_eq!(rest, ["INFO conversion-error-propagates=true", "INFO 2.0"]);
}

#[test]
fn array_proxies_take_integral_float_keys_as_lua_tables_do() {
    let (mut owner, _dir) = spawn_with(
        "host_floatkey",
        "local function report(label, value) log.info(label .. '=' .. tostring(value)) end\n\
         report('one', config.items[1])\n\
         report('one-float', config.items[1.0])\n\
         report('three-float', config.items[3.0])\n\
         report('fraction', config.items[1.5])\n\
         report('zero', config.items[0.0])\n\
         report('negative-zero', config.items[-0.0])\n\
         report('past-the-end', config.items[4.0])\n\
         report('huge', config.items[2^63])\n\
         report('nan', config.items[0/0])\n\
         report('infinity', config.items[math.huge])\n",
        sample_config(),
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.stop(BOUND).expect("stopped");
    assert_eq!(
        records("host_floatkey"),
        [
            "INFO one=a",
            "INFO one-float=a",
            "INFO three-float=c",
            "INFO fraction=nil",
            "INFO zero=nil",
            "INFO negative-zero=nil",
            "INFO past-the-end=nil",
            "INFO huge=nil",
            "INFO nan=nil",
            "INFO infinity=nil",
        ]
    );
}

/// The child's side of the no-logger test: no logger is installed in this process (it runs only this
/// test), and a script logs values whose `__tostring` fails and has a side effect.
#[test]
fn no_logger_child() {
    if std::env::var("CK_LUA_NOLOGGER_CHILD").is_err() {
        return;
    }
    assert!(ck_log::installed().is_none(), "this process has no logger");
    let (mut owner, _dir) = spawn_without_logger(
        "nolog",
        "local ran = false\n\
         local trap = setmetatable({}, {__tostring = function() ran = true error('must not run') end})\n\
         log.error(trap) log.warn(trap) log.info(trap) log.debug(trap) log.trace(trap)\n\
         log.info(1.0)\n\
         if ran then error('__tostring ran with no logger installed') end\n\
         function startup() log.info(trap) if ran then error('ran in startup') end end\n\
         function shutdown() log.info(trap) end\n",
    );
    owner.wait_ready(BOUND).expect("ready: log.* did nothing");
    owner.start(BOUND).expect("started");
    owner.stop(BOUND).expect("stopped");
    println!("no-logger-ok");
}

/// Like [`spawn`], but without installing the capture logger.
fn spawn_without_logger(name: &str, source: &str) -> (VmOwner, Dir) {
    let dir = Dir::new();
    let entry = dir.script(source);
    let owner =
        VmOwner::spawn(VmSettings::new(name, entry, empty_config())).expect("the thread starts");
    (owner, dir)
}

#[test]
fn with_no_logger_installed_log_returns_before_converting_anything() {
    let exe = std::env::current_exe().expect("the test binary");
    let child = Command::new(exe)
        .args([
            "--exact",
            "tests::no_logger_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("CK_LUA_NOLOGGER_CHILD", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the child starts");
    let mut reaper = Reaper(child);
    let stdout = pump(reaper.0.stdout.take().expect("piped stdout"));
    let stderr = pump(reaper.0.stderr.take().expect("piped stderr"));
    let give_up = Instant::now() + BOUND;
    let status = loop {
        if let Some(status) = reaper.0.try_wait().expect("the child can be polled") {
            break status;
        }
        assert!(Instant::now() < give_up, "the child did not finish");
        thread::sleep(Duration::from_millis(5));
    };
    let mut out = Vec::new();
    while let Ok(bytes) = stdout.recv_timeout(Duration::from_millis(200)) {
        out.extend_from_slice(&bytes);
    }
    let mut err = Vec::new();
    while let Ok(bytes) = stderr.recv_timeout(Duration::from_millis(200)) {
        err.extend_from_slice(&bytes);
    }
    let (out, err) = (String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
    assert!(status.success(), "the child failed: {status}\n{out}\n{err}");
    assert!(out.contains("no-logger-ok"), "{out}");
}

#[test]
fn an_invalid_name_is_refused_before_a_thread_starts() {
    let dir = Dir::new();
    let entry = dir.script("");
    for name in ["", "Bad", "has-dash", "with space", "dotted.name", "ünï"] {
        let error = VmOwner::spawn(VmSettings::new(name, entry.clone(), empty_config()))
            .expect_err("an invalid name");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{name:?}");
    }
    for name in ["ui", "gameplay", "vm_2", "_x", "0"] {
        let mut owner = VmOwner::spawn(VmSettings::new(name, entry.clone(), empty_config()))
            .expect("a valid name");
        owner.wait_ready(BOUND).expect("ready");
        owner.stop(BOUND).expect("stopped");
    }
}

#[test]
fn a_missing_entry_script_is_a_script_error() {
    let dir = Dir::new();
    let mut owner = VmOwner::spawn(VmSettings::new(
        "no_entry",
        dir.0.join("absent.lua"),
        empty_config(),
    ))
    .expect("the thread starts");
    let error = owner.wait_ready(BOUND).expect_err("no such script");
    let (message, traceback) = script_error(&error, "no_entry");
    assert!(message.contains("absent.lua"), "{message}");
    assert!(!traceback.is_empty());
}

// ---------------------------------------------------------------------------------------------
// CK-57: requests separate from waits
// ---------------------------------------------------------------------------------------------

/// Gives the owner a host function per rendezvous (`meet_start()`, `meet_stop()`), and `spin_ms(n)`.
fn meet_functions(name: &str, meetings: &[(&'static str, &Arc<Rendezvous>)]) {
    let meetings: Vec<(&'static str, Arc<Rendezvous>)> = meetings
        .iter()
        .map(|(function, rendezvous)| (*function, Arc::clone(rendezvous)))
        .collect();
    hooks::register(
        name,
        Hooks {
            at: Box::new(|_, _| {}),
            install: Some(Box::new(move |lua: &Lua| {
                add_spin(lua)?;
                for (function, rendezvous) in &meetings {
                    let rendezvous = Arc::clone(rendezvous);
                    let meet = lua.create_function(move |_, ()| {
                        if rendezvous.meet() {
                            Ok(())
                        } else {
                            Err(mlua::Error::runtime("the other request never arrived"))
                        }
                    })?;
                    lua.globals().set(*function, meet)?;
                }
                Ok(())
            })),
        },
    );
}

fn new_rendezvous() -> Arc<Rendezvous> {
    Arc::new(Rendezvous {
        arrived: Mutex::new(0),
        changed: Condvar::new(),
        parties: 2,
    })
}

#[test]
fn requests_to_two_owners_are_in_flight_together() {
    let starting = new_rendezvous();
    let stopping = new_rendezvous();
    let script = "function startup() meet_start() end\nfunction shutdown() meet_stop() end\n";
    let mut owners = Vec::new();
    let mut dirs = Vec::new();
    for name in ["split_a", "split_b"] {
        meet_functions(name, &[("meet_start", &starting), ("meet_stop", &stopping)]);
        let (owner, dir) = spawn(name, script);
        owners.push(owner);
        dirs.push(dir);
    }
    for owner in &mut owners {
        owner.wait_ready(BOUND).expect("ready");
    }
    // Both `startup()` calls wait for each other: neither completes unless both requests are in flight.
    for owner in &mut owners {
        owner.request_start().expect("the request is sent at once");
        assert_eq!(owner.state(), VmState::Starting);
    }
    for owner in &mut owners {
        owner
            .await_started(BOUND)
            .expect("both starts completed together");
        assert_eq!(owner.state(), VmState::Started);
    }
    for owner in &mut owners {
        owner.request_stop().expect("the request is sent at once");
        assert_eq!(owner.state(), VmState::Stopping);
    }
    for owner in &mut owners {
        owner
            .await_stopped(BOUND)
            .expect("both stops completed together");
        assert!(owner.is_finished());
        assert_eq!(owner.state(), VmState::Finished);
    }
}

#[test]
fn a_sibling_is_observed_even_after_the_other_owners_deadline_is_used_up() {
    with_spin("split_slow");
    let (mut slow, _slow_dir) = spawn(
        "split_slow",
        &format!(
            "function shutdown() log.info('slow shutdown') {} end\n",
            spin(500)
        ),
    );
    let (mut quick, _quick_dir) = spawn(
        "split_quick",
        "function shutdown() log.info('quick shutdown') end\n",
    );
    slow.wait_ready(BOUND).expect("ready");
    quick.wait_ready(BOUND).expect("ready");
    slow.request_stop().expect("sent");
    quick.request_stop().expect("sent");
    let begun = Instant::now();
    assert_eq!(
        slow.await_stopped(Duration::from_millis(100)),
        Err(VmError::Deadline {
            owner: "split_slow".to_owned()
        })
    );
    assert!(begun.elapsed() < Duration::from_millis(400));
    // The quick owner's thread has ended by now (waiting for it needs no join of its own).
    let give_up = Instant::now() + BOUND;
    while !quick.is_finished() {
        assert!(Instant::now() < give_up, "the quick owner never finished");
        thread::yield_now();
    }
    // No time left, yet a result and a finished thread that are already there are seen: one
    // non-waiting observation.
    quick
        .await_stopped(Duration::ZERO)
        .expect("observed with a zero deadline");
    assert_eq!(quick.state(), VmState::Finished);
    // The slow owner was abandoned and ends later, by itself.
    assert_eq!(slow.state(), VmState::Ending);
    assert!(slow.wait_finished(BOUND));
    assert_eq!(records("split_quick"), ["INFO quick shutdown"]);
    assert_eq!(
        records("split_slow"),
        ["INFO slow shutdown"],
        "the running shutdown() was not interrupted; the thread ended after it returned"
    );
}

#[test]
fn each_new_call_in_a_wrong_state_is_refused_and_sends_nothing() {
    let (mut owner, _dir) = spawn(
        "split_legal",
        "function startup() log.info('startup') end\nfunction shutdown() log.info('shutdown') end\n",
    );
    // Initialising.
    expect_state(
        owner.request_start(),
        "request_start",
        VmState::Initialising,
    );
    expect_state(
        owner.await_started(BOUND),
        "await_started",
        VmState::Initialising,
    );
    expect_state(owner.request_stop(), "request_stop", VmState::Initialising);
    expect_state(
        owner.await_stopped(BOUND),
        "await_stopped",
        VmState::Initialising,
    );
    owner.wait_ready(BOUND).expect("ready");
    // Ready.
    expect_state(owner.await_started(BOUND), "await_started", VmState::Ready);
    expect_state(owner.await_stopped(BOUND), "await_stopped", VmState::Ready);
    owner.request_start().expect("sent");
    // Starting: a second request, and every other call, is refused.
    expect_state(owner.request_start(), "request_start", VmState::Starting);
    expect_state(owner.start(BOUND), "start", VmState::Starting);
    expect_state(owner.request_stop(), "request_stop", VmState::Starting);
    expect_state(owner.stop(BOUND), "stop", VmState::Starting);
    expect_state(
        owner.await_stopped(BOUND),
        "await_stopped",
        VmState::Starting,
    );
    expect_state(owner.wait_ready(BOUND), "wait_ready", VmState::Starting);
    owner.await_started(BOUND).expect("started");
    // Started.
    expect_state(owner.request_start(), "request_start", VmState::Started);
    expect_state(
        owner.await_started(BOUND),
        "await_started",
        VmState::Started,
    );
    expect_state(
        owner.await_stopped(BOUND),
        "await_stopped",
        VmState::Started,
    );
    owner.request_stop().expect("sent");
    // Stopping.
    expect_state(owner.request_start(), "request_start", VmState::Stopping);
    expect_state(
        owner.await_started(BOUND),
        "await_started",
        VmState::Stopping,
    );
    expect_state(owner.request_stop(), "request_stop", VmState::Stopping);
    expect_state(owner.start(BOUND), "start", VmState::Stopping);
    expect_state(owner.stop(BOUND), "stop", VmState::Stopping);
    owner.await_stopped(BOUND).expect("stopped");
    // Finished.
    expect_state(owner.request_start(), "request_start", VmState::Finished);
    expect_state(
        owner.await_started(BOUND),
        "await_started",
        VmState::Finished,
    );
    expect_state(owner.request_stop(), "request_stop", VmState::Finished);
    expect_state(
        owner.await_stopped(BOUND),
        "await_stopped",
        VmState::Finished,
    );
    assert_eq!(
        records("split_legal"),
        ["INFO startup", "INFO shutdown"],
        "each ran once: no refused call reached the script"
    );
}

#[test]
fn start_and_stop_are_the_request_followed_by_the_await_with_the_same_results() {
    let (mut owner, _dir) = spawn(
        "split_whole",
        "function startup() log.info('up') end\nfunction shutdown() error('down failed') end\n",
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    assert_eq!(owner.state(), VmState::Started);
    let error = owner.stop(BOUND).expect_err("shutdown fails");
    assert!(
        script_error(&error, "split_whole")
            .0
            .contains("down failed")
    );
    assert_eq!(owner.state(), VmState::Ending);
    assert!(owner.wait_finished(BOUND));
}

#[test]
fn a_missed_await_deadline_abandons_the_request_like_start_and_stop_do() {
    with_spin("split_abandon");
    let (mut owner, _dir) = spawn(
        "split_abandon",
        &format!(
            "function startup() {} end\nfunction shutdown() log.info('shutdown called') end\n",
            spin(400)
        ),
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.request_start().expect("sent");
    assert_eq!(
        owner.await_started(Duration::from_millis(80)),
        Err(VmError::Deadline {
            owner: "split_abandon".to_owned()
        })
    );
    assert_eq!(owner.state(), VmState::Ending);
    expect_state(owner.request_stop(), "request_stop", VmState::Ending);
    assert!(
        owner.wait_finished(BOUND),
        "the thread ends by itself after the call returns"
    );
    assert!(
        records("split_abandon").is_empty(),
        "shutdown() was not called"
    );
}

// ---------------------------------------------------------------------------------------------
// CK-55: the instruction hook, the wrappers and the interruption
// ---------------------------------------------------------------------------------------------

/// What a hook test watches besides the `entered()` signal: `block()` released by a channel, and the
/// flag a sentinel in the state sets when the state is dropped.
struct Rig {
    release: Sender<()>,
    dropped: Arc<AtomicBool>,
}

/// Registers hooks for `name`: a sentinel in the state, and the host functions `entered()` (signals the
/// returned receiver), `block()` (blocks on a channel for at most [`BOUND`]) and `spin_ms(n)`.
fn rig(name: &str) -> (Rig, Receiver<()>) {
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&dropped);
    let (entered_tx, entered) = mpsc::channel();
    let entered_tx = Mutex::new(entered_tx);
    let (release, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    hooks::register(
        name,
        Hooks {
            at: Box::new(move |point, lua| {
                if let (Point::StateCreated, Some(lua)) = (point, lua) {
                    let sentinel = lua
                        .create_userdata(Sentinel(Arc::clone(&flag)))
                        .expect("a sentinel");
                    lua.globals().set("sentinel", sentinel).expect("a global");
                }
            }),
            install: Some(Box::new(move |lua: &Lua| {
                add_spin(lua)?;
                let sender = entered_tx
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                lua.globals().set(
                    "entered",
                    lua.create_function(move |_, ()| {
                        let _ = sender.send(());
                        Ok(())
                    })?,
                )?;
                let receiver = Arc::clone(&release_rx);
                lua.globals().set(
                    "block",
                    lua.create_function(move |_, ()| {
                        let _ = receiver
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .recv_timeout(BOUND);
                        Ok(())
                    })?,
                )
            })),
        },
    );
    (Rig { release, dropped }, entered)
}

/// Sets the owner's cancel flag from another thread once the script has signalled `entered()`, so the
/// waiting call on this thread sees the cancellation arrive while it waits. Call after `spawn`.
fn cancel_on_entry(name: &str, entered: Receiver<()>) -> thread::JoinHandle<()> {
    let flag = hooks::cancel_flag(name);
    thread::spawn(move || {
        if entered.recv_timeout(BOUND).is_ok() {
            flag.store(true, Ordering::SeqCst);
        }
    })
}

fn cancelled(name: &str) -> VmError {
    VmError::Cancelled {
        owner: name.to_owned(),
    }
}

/// Asserts the end of an interrupted owner: the thread ends, and the state was dropped.
fn assert_ended(owner: &mut VmOwner, rig: &Rig) {
    assert!(
        owner.wait_finished(BOUND),
        "the interrupted owner's thread ends"
    );
    assert!(rig.dropped.load(Ordering::SeqCst), "the state was dropped");
}

#[test]
fn an_endless_loop_is_interrupted_in_the_main_chunk_startup_and_shutdown() {
    // The main chunk: cancel during `wait_ready`.
    let (r, entered) = rig("loop_main");
    let (mut owner, _dir) = spawn("loop_main", "entered()\nwhile true do end\n");
    let canceller = cancel_on_entry("loop_main", entered);
    assert_eq!(owner.wait_ready(BOUND), Err(cancelled("loop_main")));
    canceller.join().expect("canceller");
    assert_eq!(owner.state(), VmState::Ending);
    assert_ended(&mut owner, &r);

    // `startup()`: cancel during `start`.
    let (r, entered) = rig("loop_start");
    let (mut owner, _dir) = spawn(
        "loop_start",
        "function startup() entered() while true do end end\n",
    );
    owner.wait_ready(BOUND).expect("ready");
    let canceller = cancel_on_entry("loop_start", entered);
    assert_eq!(owner.start(BOUND), Err(cancelled("loop_start")));
    canceller.join().expect("canceller");
    assert_ended(&mut owner, &r);

    // `shutdown()`: cancel during `stop`.
    let (r, entered) = rig("loop_stop");
    let (mut owner, _dir) = spawn(
        "loop_stop",
        "function shutdown() entered() while true do end end\n",
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    let canceller = cancel_on_entry("loop_stop", entered);
    assert_eq!(owner.stop(BOUND), Err(cancelled("loop_stop")));
    canceller.join().expect("canceller");
    assert_ended(&mut owner, &r);
}

/// The protected-call counterexamples: each loops forever and survives a hook that only raises.
const PROTECTED: [(&str, &str); 8] = [
    (
        "prot_pcall",
        "entered() while true do pcall(function() while true do end end) end",
    ),
    (
        "prot_xpcall",
        "entered() while true do xpcall(function() while true do end end, function(e) return 'handled' end) end",
    ),
    (
        "prot_resume",
        "entered() local co = coroutine.create(function() while true do end end) while true do coroutine.resume(co) end",
    ),
    (
        "prot_close",
        "entered() local function inner() while true do end end local x <close> = setmetatable({}, {__close = function() while true do end end}) inner()",
    ),
    (
        "prot_masking",
        "entered() while true do pcall(function() local x <close> = setmetatable({}, {__close = function() error('masked') end}) while true do end end) end",
    ),
    (
        "prot_handler_value",
        "entered() while true do xpcall(function() while true do end end, function(e) return 'an ordinary value' end) end",
    ),
    (
        "prot_handler_swallows",
        "entered() while true do xpcall(function() while true do end end, function(e) end) end",
    ),
    (
        "prot_coclose",
        "entered() while true do local co = coroutine.create(function() local x <close> = setmetatable({}, {__close = function() error('x') end}) coroutine.yield() end) coroutine.resume(co) pcall(coroutine.close, co) end",
    ),
];

#[test]
fn protected_calls_do_not_save_a_script_from_cancellation() {
    for (name, script) in PROTECTED {
        let (r, entered) = rig(name);
        let (mut owner, _dir) = spawn(name, script);
        let canceller = cancel_on_entry(name, entered);
        assert_eq!(
            owner.wait_ready(BOUND),
            Err(cancelled(name)),
            "{name}: {script}"
        );
        canceller.join().expect("canceller");
        assert_ended(&mut owner, &r);
    }
}

#[test]
fn an_endless_loop_inside_a_coroutine_is_interrupted() {
    let (r, entered) = rig("coroutine_loop");
    let (mut owner, _dir) = spawn(
        "coroutine_loop",
        "function startup() entered() local f = coroutine.wrap(function() while true do end end) f() end\n",
    );
    owner.wait_ready(BOUND).expect("ready");
    let canceller = cancel_on_entry("coroutine_loop", entered);
    assert_eq!(owner.start(BOUND), Err(cancelled("coroutine_loop")));
    canceller.join().expect("canceller");
    assert_ended(&mut owner, &r);
}

/// Expressions whose results the standard protected-call functions define; the summary is the same
/// text from the wrapped functions (in an owner, flag clear) and from the standard ones (a plain state).
const WRAPPER_SUMMARY: &str = "
local out = {}
local function note(label, ...)
  local parts = {}
  for i = 1, select('#', ...) do
    local v = select(i, ...)
    parts[#parts + 1] = type(v) == 'table' and 'table' or (tostring(v):gsub('^.*:%d+: ', ''))
  end
  out[#out + 1] = label .. '=' .. table.concat(parts, ',')
end
note('pcall-error', pcall(error, 'x'))
note('pcall-table', pcall(error, {}))
note('pcall-ok', pcall(function(...) return ... end, 1, 2, 3))
note('pcall-none', pcall(function() end))
note('xpcall-handler', xpcall(error, function(m) return 'H:' .. tostring(m) end, 'boom'))
note('xpcall-ok', xpcall(function(a, b) return a + b end, print, 3, 4))
local bad = coroutine.create(function() error('inside') end)
note('resume-error', coroutine.resume(bad))
note('resume-dead', coroutine.resume(bad))
local good = coroutine.create(function(a) local b = coroutine.yield(a + 1) return b * 2 end)
note('resume-1', coroutine.resume(good, 1))
note('resume-2', coroutine.resume(good, 5))
note('close-dead', coroutine.close(good))
local pending = coroutine.create(function() local x <close> = setmetatable({}, {__close = function() error('closing') end}) coroutine.yield() end)
coroutine.resume(pending)
note('close-pending', coroutine.close(pending))
return table.concat(out, ';')
";

#[test]
fn with_the_flag_clear_the_wrappers_return_exactly_what_the_standard_functions_return() {
    let plain: String = Lua::new()
        .load(WRAPPER_SUMMARY)
        .eval()
        .expect("the standard functions run the summary");
    let (mut owner, _dir) = spawn(
        "wrapped_same",
        &format!("log.info((function() {WRAPPER_SUMMARY} end)())"),
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.stop(BOUND).expect("stopped");
    assert_eq!(records("wrapped_same"), [format!("INFO {plain}")]);
    assert!(
        plain.contains("pcall-error=false,x") && plain.contains("close-dead=true"),
        "{plain}"
    );
}

#[test]
fn raising_the_interruption_text_with_the_flag_clear_is_an_ordinary_script_error() {
    let (mut owner, _dir) = spawn("text_only", "error('ck-lua: interrupted')");
    let error = owner.wait_ready(BOUND).expect_err("an ordinary error");
    let (message, _traceback) = script_error(&error, "text_only");
    assert!(message.contains("ck-lua: interrupted"), "{message}");
    assert!(matches!(error, VmError::Script { .. }));
}

/// Runs this test binary again for one stuck-owner case (`limit_child` does the work) and waits for its
/// report. The child's owner thread is stuck for good, so the reaper kills and reaps the child.
fn run_limit_child(case: &str) {
    let exe = std::env::current_exe().expect("the test binary");
    let child = Command::new(exe)
        .args([
            "--exact",
            "tests::limit_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("CK_LUA_LIMIT_CHILD", case)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the child starts");
    let mut reaper = Reaper(child);
    let stdout = pump(reaper.0.stdout.take().expect("piped stdout"));
    let mut seen = Vec::new();
    let marker = format!("limit-ok {case}");
    assert!(
        read_until(&stdout, &mut seen, &marker, BOUND),
        "the child did not report: {}",
        String::from_utf8_lossy(&seen)
    );
}

#[test]
fn an_endless_loop_in_a_gc_finalizer_cannot_be_interrupted_and_ends_as_a_missed_deadline() {
    run_limit_child("gc");
}

#[test]
fn an_endless_close_handler_of_the_interrupted_frame_cannot_be_interrupted() {
    run_limit_child("close");
}

/// The child's side of the stuck-owner tests. `startup()` runs a loop the hook cannot enter, and is
/// abandoned by a 300 ms deadline: the owner is not observed finished, the child reports, and the
/// parent kills it.
/// - `gc`: a table whose `__gc` loops forever, collected. Lua disables hooks in finalizers.
/// - `close`: a `__close` handler of a variable in the very frame the hook interrupts. `mlua` raises a
///   hook's error path clears the stack with `lua_settop` before `lua_error` (0 on its reserved-failure
///   path, 1 on its new-failure path), which closes that frame's own variables inside the hook, where
///   hooks are disabled. (A variable in an outer frame is closed during the unwind, hooks on.)
#[test]
fn limit_child() {
    let Ok(case) = std::env::var("CK_LUA_LIMIT_CHILD") else {
        return;
    };
    let startup = match case.as_str() {
        "gc" => {
            "function startup() local t = setmetatable({}, {__gc = function() entered() while true do end end}) t = nil collectgarbage() collectgarbage() end\n"
        }
        "close" => {
            "function startup() local x <close> = setmetatable({}, {__close = function() entered() while true do end end}) while true do end end\n"
        }
        other => panic!("unknown case {other}"),
    };
    let (_rig, entered) = rig("stuck");
    let (mut owner, _dir) = spawn("stuck", startup);
    owner.wait_ready(BOUND).expect("ready");
    let error = owner
        .start(Duration::from_millis(300))
        .expect_err("the loop never returns");
    // The handler was really entered (it signals before it loops), so the owner is stuck in it and not
    // somewhere else.
    entered
        .recv_timeout(BOUND)
        .expect("the finalizer or __close handler was entered");
    assert_eq!(
        error,
        VmError::Deadline {
            owner: "stuck".to_owned()
        }
    );
    assert!(
        !owner.wait_finished(Duration::from_millis(500)),
        "an owner stuck where hooks are off is not observed finished"
    );
    println!("limit-ok {case}");
}

#[test]
fn a_blocking_host_call_is_not_interrupted_until_it_returns() {
    let (r, entered) = rig("blocked");
    let (mut owner, _dir) = spawn("blocked", "entered()\nblock()\nwhile true do end\n");
    let canceller = cancel_on_entry("blocked", entered);
    canceller.join().expect("the cancel was set after entry");
    // Cancelled, but the host function has not returned: the thread does not end.
    assert!(
        !owner.wait_finished(Duration::from_millis(300)),
        "a blocking host function is outside the hook"
    );
    // Released (on every path the host function also gives up by itself after BOUND), it ends.
    r.release.send(()).expect("the host function is waiting");
    assert_eq!(owner.wait_ready(BOUND), Err(cancelled("blocked")));
    assert_ended(&mut owner, &r);
}

#[test]
fn with_the_flag_clear_a_long_loop_gives_the_right_result() {
    let (mut owner, _dir) = spawn(
        "no_interference",
        "local sum = 0 for i = 1, 100000 do sum = sum + i end log.info(sum)\n",
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.stop(BOUND).expect("stopped");
    assert_eq!(records("no_interference"), ["INFO 5000050000"]);
}

// ---------------------------------------------------------------------------------------------
// 10. Stdout, in a child process
// ---------------------------------------------------------------------------------------------

/// The child's side of the stdout tests. The parent re-runs this test binary on exactly this test
/// with `CK_LUA_CHILD` set; without it the test does nothing.
#[test]
fn stdout_child() {
    let Ok(mode) = std::env::var("CK_LUA_CHILD") else {
        return;
    };
    let script = match mode.as_str() {
        "print" => "print('a', 1)",
        "flush" => "io.stdout:setvbuf('full')\nio.stdout:write('fragment')\n",
        "flush_io_nil" => "io.stdout:setvbuf('full')\nio.stdout:write('fragment')\nio = nil\n",
        other => panic!("unknown child mode {other}"),
    };
    let (mut owner, _dir) = spawn("child", script);
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    owner.stop(BOUND).expect("stopped");
    // Tell the parent on the separate pipe, then stay alive until it has read the stdout pipe.
    eprintln!("stopped");
    let mut go = String::new();
    let _ = io::stdin().read_line(&mut go);
}

/// Kills and reaps the child on every path.
struct Reaper(Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Reads a pipe on a thread and sends what arrives.
fn pump(mut pipe: impl Read + Send + 'static) -> Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut chunk = [0_u8; 4096];
        while let Ok(read) = pipe.read(&mut chunk) {
            if read == 0 || tx.send(chunk[..read].to_vec()).is_err() {
                break;
            }
        }
    });
    rx
}

/// Reads from `pipe` until `wanted` is in what was read, within `within`.
fn read_until(
    pipe: &Receiver<Vec<u8>>,
    seen: &mut Vec<u8>,
    wanted: &str,
    within: Duration,
) -> bool {
    let give_up = Instant::now() + within;
    while !String::from_utf8_lossy(seen).contains(wanted) {
        let left = give_up.saturating_duration_since(Instant::now());
        match pipe.recv_timeout(left) {
            Ok(bytes) => seen.extend_from_slice(&bytes),
            Err(_) => return false,
        }
    }
    true
}

/// Runs the child in `mode` with all three standard streams piped, and returns whether `wanted`
/// was readable from its stdout pipe after `stop` returned and while the child was still alive.
fn child_writes(mode: &str, wanted: &str) -> bool {
    let exe = std::env::current_exe().expect("the test binary");
    let mut child = Command::new(exe)
        .args([
            "--exact",
            "tests::stdout_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("CK_LUA_CHILD", mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the child starts");
    let stdout = pump(child.stdout.take().expect("piped stdout"));
    let stderr = pump(child.stderr.take().expect("piped stderr"));
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut reaper = Reaper(child);
    // The child says on its stderr that `stop` has returned.
    let mut signalled = Vec::new();
    assert!(
        read_until(&stderr, &mut signalled, "stopped", BOUND),
        "the child never signalled that stop returned: {}",
        String::from_utf8_lossy(&signalled)
    );
    assert!(
        reaper
            .0
            .try_wait()
            .expect("the child can be polled")
            .is_none(),
        "the child must still be alive when its stdout is read"
    );
    let mut seen = Vec::new();
    let arrived = read_until(&stdout, &mut seen, wanted, Duration::from_secs(5));
    // Only now does the child get its go-ahead and exit (which would flush C stdout anyway).
    let _ = stdin.write_all(b"go\n");
    drop(stdin);
    let give_up = Instant::now() + BOUND;
    while reaper
        .0
        .try_wait()
        .expect("the child can be polled")
        .is_none()
    {
        assert!(
            Instant::now() < give_up,
            "the child did not exit after the go-ahead"
        );
        thread::sleep(Duration::from_millis(5));
    }
    arrived
}

#[test]
fn standard_print_arrives_on_a_piped_stdout() {
    assert!(child_writes("print", "a\t1\n"));
}

#[test]
fn the_owner_flushes_buffered_stdout_before_it_finishes() {
    assert!(
        child_writes("flush", "fragment"),
        "buffered io.stdout:write output must reach the pipe while the child is alive"
    );
}

#[test]
fn the_owner_flush_survives_a_script_that_replaces_io() {
    assert!(
        child_writes("flush_io_nil", "fragment"),
        "the stream captured at creation is flushed even after `io = nil`"
    );
}

#[path = "tests_calls.rs"]
mod calls;
