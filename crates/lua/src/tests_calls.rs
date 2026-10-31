//! CK-59: non-abandoning polls, host extensions, posted callback calls and the report wake.
//!
//! Every wait is bounded. A fixture that blocks a callback releases it when its `Probe` drops, so a
//! failed assertion cannot leave a worker waiting longer than `BOUND`; owners cancel when dropped.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use mlua::{Lua, LuaOptions, StdLib};

use super::{
    BOUND, Dir, Hooks, Point, hooks, records, reported, script_error, spawn, track_state_drop,
};
use crate::runner::Wake;
use crate::{
    CallOutcome, CallResult, ConfigValue, Extension, HostFunction, Progress, VmError, VmOwner,
    VmSettings, VmState,
};

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn host(
    function: impl Fn(&[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String>
    + Send
    + Sync
    + 'static,
) -> HostFunction {
    Arc::new(function)
}

/// Holds the owner thread at each of `points` (every time it reaches one) until the test releases it,
/// at most `BOUND` per hold. `arrived` says which point the thread is at.
fn hold_points(name: &str, points: &'static [Point]) -> (Receiver<Point>, Sender<()>) {
    let (arrived_tx, arrived) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let state = Mutex::new((arrived_tx, release_rx));
    hooks::register(
        name,
        Hooks {
            at: Box::new(move |at, _| {
                if points.contains(&at) {
                    let state = lock(&state);
                    let _ = state.0.send(at);
                    let _ = state.1.recv_timeout(BOUND);
                }
            }),
            install: None,
        },
    );
    (arrived, release)
}

/// Spawns an owner on `source` with settings adjusted by `adjust`.
fn spawn_with_settings(
    name: &str,
    source: &str,
    adjust: impl FnOnce(&mut VmSettings),
) -> (VmOwner, Dir) {
    let _ = super::buffer();
    let dir = Dir::new();
    let entry = dir.script(source);
    let mut settings = VmSettings::new(name, entry, super::empty_config());
    adjust(&mut settings);
    (VmOwner::spawn(settings).expect("the thread starts"), dir)
}

/// What the host functions of the `ext` table record, and the gates they wait on.
struct Probe {
    /// `ext.count()` calls.
    calls: Arc<AtomicUsize>,
    /// The arguments of every `ext.record(...)` call.
    seen: Arc<Mutex<Vec<Vec<Option<ConfigValue>>>>>,
    /// The names passed to `ext.mark(name)`.
    marks: Arc<Mutex<Vec<String>>>,
    /// Lets `ext.block()` return (once per send).
    release: Sender<()>,
    /// Signalled by `ext.entered()`.
    entered: Receiver<()>,
    gate: Arc<Mutex<Receiver<()>>>,
    entered_tx: Arc<Mutex<Sender<()>>>,
}

impl Drop for Probe {
    fn drop(&mut self) {
        for _ in 0..16 {
            let _ = self.release.send(());
        }
    }
}

/// The callback names `kit` has a setter `ext.on_<name>` for.
const CALLBACKS: [&str; 12] = [
    "add", "echo", "fail", "forever", "blocker", "shape", "touch", "touched", "boom", "wrap",
    "invalid", "note",
];

impl Probe {
    fn new() -> Self {
        let (release, gate) = mpsc::channel();
        let (entered_tx, entered) = mpsc::channel();
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            seen: Arc::new(Mutex::new(Vec::new())),
            marks: Arc::new(Mutex::new(Vec::new())),
            release,
            entered,
            gate: Arc::new(Mutex::new(gate)),
            entered_tx: Arc::new(Mutex::new(entered_tx)),
        }
    }

    fn marks(&self) -> Vec<String> {
        lock(&self.marks).clone()
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// The extension `ext`: host functions `record`, `count`, `mark`, `boom`, `boom_text`, `fail`,
    /// `pair`, `sum`, `block`, `entered` and `spin`, and a setter `on_<name>` for each of `CALLBACKS`.
    fn kit(&self) -> Extension {
        let seen = Arc::clone(&self.seen);
        let calls = Arc::clone(&self.calls);
        let marks = Arc::clone(&self.marks);
        let gate = Arc::clone(&self.gate);
        let entered = Arc::clone(&self.entered_tx);
        let mut extension = Extension::new("ext")
            .function(
                "record",
                host(move |args| {
                    lock(&seen).push(args.to_vec());
                    Ok(Vec::new())
                }),
            )
            .function(
                "count",
                host(move |_| {
                    let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    Ok(vec![ConfigValue::Integer(i64::try_from(n).expect("small"))])
                }),
            )
            .function(
                "mark",
                host(move |args| {
                    if let Some(Some(ConfigValue::String(name))) = args.first() {
                        lock(&marks).push(name.clone());
                    }
                    Ok(Vec::new())
                }),
            )
            .function("boom", host(|_| panic!("boom msg")))
            .function(
                "boom_text",
                host(|_| std::panic::panic_any(String::from("owned boom msg"))),
            )
            .function("fail", host(|_| Err("the host said no".to_owned())))
            .function(
                "pair",
                host(|_| {
                    Ok(vec![
                        ConfigValue::Integer(1),
                        ConfigValue::String("two".to_owned()),
                        ConfigValue::Table(
                            [(
                                "k".to_owned(),
                                ConfigValue::Array(vec![ConfigValue::Bool(true)]),
                            )]
                            .into(),
                        ),
                    ])
                }),
            )
            .function(
                "sum",
                host(|args| match args {
                    [Some(ConfigValue::Integer(a)), Some(ConfigValue::Integer(b))] => {
                        Ok(vec![ConfigValue::Integer(a + b)])
                    }
                    other => Err(format!("sum wants two integers, got {other:?}")),
                }),
            )
            .function(
                "block",
                host(move |_| {
                    let _ = lock(&gate).recv_timeout(BOUND);
                    Ok(Vec::new())
                }),
            )
            .function(
                "entered",
                host(move |_| {
                    let _ = lock(&entered).send(());
                    Ok(Vec::new())
                }),
            )
            .function(
                "spin",
                host(|args| {
                    let Some(Some(ConfigValue::Integer(ms))) = args.first() else {
                        return Err("spin wants milliseconds".to_owned());
                    };
                    let end =
                        Instant::now() + Duration::from_millis(u64::try_from(*ms).unwrap_or(0));
                    while Instant::now() < end {
                        std::hint::spin_loop();
                    }
                    Ok(Vec::new())
                }),
            );
        for name in CALLBACKS {
            extension = extension.callback(&format!("on_{name}"), name);
        }
        extension
    }

    /// Spawns an owner with the kit, `limit` calls in flight and the optional wake.
    fn spawn(&self, name: &str, source: &str, limit: usize, wake: Option<Wake>) -> (VmOwner, Dir) {
        let extension = self.kit();
        spawn_with_settings(name, source, move |settings| {
            settings.extensions = vec![extension];
            settings.call_limit = limit;
            settings.on_report = wake;
        })
    }

    /// An owner brought to Started.
    fn started(&self, name: &str, source: &str, limit: usize) -> (VmOwner, Dir) {
        let (mut owner, dir) = self.spawn(name, source, limit, None);
        owner.wait_ready(BOUND).expect("ready");
        owner.start(BOUND).expect("started");
        (owner, dir)
    }
}

/// The next result, waiting at most `BOUND` (`take_result` itself never waits).
fn next_result(owner: &mut VmOwner) -> CallResult {
    let end = Instant::now() + BOUND;
    loop {
        if let Some(result) = owner.take_result() {
            return result;
        }
        assert!(Instant::now() < end, "no call result arrived");
        thread::sleep(Duration::from_millis(1));
    }
}

fn returned(result: &CallResult) -> &[Option<ConfigValue>] {
    match &result.outcome {
        CallOutcome::Returned(values) => values,
        other => panic!("expected a returned value, got {other:?}"),
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "it builds the expected `Option` element of a returned value list"
)]
fn int(value: i64) -> Option<ConfigValue> {
    Some(ConfigValue::Integer(value))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "it builds the expected `Option` element of a returned value list"
)]
fn text(value: &str) -> Option<ConfigValue> {
    Some(ConfigValue::String(value.to_owned()))
}

/// Polls `poll` in 10 ms slices until it is no longer `Pending`; returns how many slices were pending.
fn poll_until_done(
    owner: &mut VmOwner,
    mut poll: impl FnMut(&mut VmOwner) -> Result<Progress, VmError>,
) -> Result<usize, VmError> {
    let end = Instant::now() + BOUND;
    let mut pending = 0;
    loop {
        match poll(owner)? {
            Progress::Done => return Ok(pending),
            Progress::Pending => pending += 1,
        }
        assert!(Instant::now() < end, "the poll never completed");
    }
}

fn poll_ready(owner: &mut VmOwner) -> Result<Progress, VmError> {
    owner.poll_ready(Duration::from_millis(10))
}

fn poll_started(owner: &mut VmOwner) -> Result<Progress, VmError> {
    owner.poll_started(Duration::from_millis(10))
}

// ---------------------------------------------------------------------------------------------
// 1. Non-abandoning polls
// ---------------------------------------------------------------------------------------------

#[test]
fn polls_do_not_cancel_a_slow_main_chunk_or_a_slow_startup() {
    let name = "poll_slow";
    let (arrived, release) = hold_points(name, &[Point::BeforeMain, Point::BeforeStartup]);
    let (mut owner, _dir) = spawn(
        name,
        "function startup() log.info('up') end\nfunction shutdown() log.info('down') end\n",
    );
    let flag = hooks::cancel_flag(name);
    assert_eq!(arrived.recv_timeout(BOUND), Ok(Point::BeforeMain));
    let sent = hooks::sent_so_far(name);
    for _ in 0..8 {
        assert_eq!(poll_ready(&mut owner), Ok(Progress::Pending));
        assert!(
            !flag.load(Ordering::SeqCst),
            "a poll never sets the cancel flag"
        );
        assert_eq!(owner.state(), VmState::Initialising);
    }
    assert_eq!(owner.poll_ready(Duration::ZERO), Ok(Progress::Pending));
    assert_eq!(hooks::sent_so_far(name), sent, "a poll sends no command");
    release.send(()).expect("release the main chunk");
    poll_until_done(&mut owner, poll_ready).expect("ready");
    assert_eq!(owner.state(), VmState::Ready);

    owner.request_start().expect("sent");
    assert_eq!(arrived.recv_timeout(BOUND), Ok(Point::BeforeStartup));
    let sent = hooks::sent_so_far(name);
    for _ in 0..8 {
        assert_eq!(poll_started(&mut owner), Ok(Progress::Pending));
        assert!(!flag.load(Ordering::SeqCst));
        assert_eq!(owner.state(), VmState::Starting);
    }
    assert_eq!(hooks::sent_so_far(name), sent);
    release.send(()).expect("release startup");
    poll_until_done(&mut owner, poll_started).expect("started");
    assert_eq!(owner.state(), VmState::Started);

    owner.stop(BOUND).expect("the VM stops normally");
    assert_eq!(owner.state(), VmState::Finished);
    assert_eq!(records(name), ["INFO up", "INFO down"]);
}

#[test]
fn a_failing_main_chunk_and_a_failing_startup_are_reported_through_the_poll() {
    let (mut owner, _dir) = spawn("poll_fail_main", "error('main broke')");
    let error = poll_until_done(&mut owner, poll_ready).expect_err("the main chunk failed");
    let (message, traceback) = script_error(&error, "poll_fail_main");
    assert!(message.contains("main broke"), "{message}");
    assert!(traceback.contains("stack traceback"), "{traceback}");
    assert_eq!(owner.state(), VmState::Ending);
    assert!(matches!(
        owner.poll_ready(Duration::ZERO),
        Err(VmError::State {
            call: "poll_ready",
            state: VmState::Ending,
            ..
        })
    ));

    let (mut owner, _dir) = spawn(
        "poll_fail_start",
        "function startup() error('start broke') end",
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.request_start().expect("sent");
    let error = poll_until_done(&mut owner, poll_started).expect_err("startup failed");
    assert!(
        script_error(&error, "poll_fail_start")
            .0
            .contains("start broke")
    );
    assert_eq!(owner.state(), VmState::Ending);

    // A panic is reported through the poll as `wait_ready` reports it.
    let name = "poll_panic";
    let dropped = track_state_drop(name, |point| {
        assert!(point != Point::AfterMain, "injected host panic");
    });
    let (mut owner, _dir) = spawn(name, "log.info('main')");
    let error = poll_until_done(&mut owner, poll_ready).expect_err("the owner panicked");
    assert!(
        matches!(&error, VmError::Panicked { message, .. } if message.contains("injected host panic")),
        "{error:?}"
    );
    assert_eq!(owner.state(), VmState::Ending);
    assert!(owner.wait_finished(BOUND));
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn a_zero_poll_observes_an_arrived_report_and_a_later_wait_still_sees_one() {
    let name = "poll_zero";
    let reported = reported(name);
    let (mut owner, _dir) = spawn(name, "function startup() end");
    reported.recv_timeout(BOUND).expect("the thread reported");
    assert_eq!(owner.poll_ready(Duration::ZERO), Ok(Progress::Done));
    assert_eq!(owner.state(), VmState::Ready);
    owner.stop(BOUND).expect("stopped");

    // Pending leaves the request in flight: `wait_ready` still consumes the report.
    let name = "poll_then_wait";
    let (arrived, release) = hold_points(name, &[Point::BeforeMain]);
    let (mut owner, _dir) = spawn(name, "");
    assert_eq!(arrived.recv_timeout(BOUND), Ok(Point::BeforeMain));
    assert_eq!(owner.poll_ready(Duration::ZERO), Ok(Progress::Pending));
    release.send(()).expect("release");
    owner.wait_ready(BOUND).expect("ready after a pending poll");
    assert_eq!(owner.state(), VmState::Ready);
}

#[test]
fn the_existing_waits_still_abandon_on_a_missed_deadline() {
    let name = "poll_then_abandon";
    let (arrived, release) = hold_points(name, &[Point::BeforeMain]);
    let (mut owner, _dir) = spawn(name, "");
    let flag = hooks::cancel_flag(name);
    assert_eq!(arrived.recv_timeout(BOUND), Ok(Point::BeforeMain));
    assert_eq!(owner.poll_ready(Duration::ZERO), Ok(Progress::Pending));
    assert!(!flag.load(Ordering::SeqCst));
    assert_eq!(
        owner.wait_ready(Duration::ZERO),
        Err(VmError::Deadline {
            owner: name.to_owned()
        })
    );
    assert!(
        flag.load(Ordering::SeqCst),
        "the wait abandoned the request"
    );
    assert_eq!(owner.state(), VmState::Ending);
    release.send(()).expect("release");
    assert!(owner.wait_finished(BOUND));
}

#[test]
fn polls_in_the_wrong_state_are_refused_and_send_nothing() {
    fn refused(result: Result<Progress, VmError>, call: &'static str, state: VmState) {
        match result {
            Err(VmError::State {
                call: named,
                state: at,
                ..
            }) => {
                assert_eq!((named, at), (call, state));
            }
            other => panic!("expected a state error for {call}, got {other:?}"),
        }
    }
    let name = "poll_states";
    let (mut owner, _dir) = spawn(name, "function startup() end");
    let sent = hooks::sent_so_far(name);
    refused(
        owner.poll_started(Duration::ZERO),
        "poll_started",
        VmState::Initialising,
    );
    owner.wait_ready(BOUND).expect("ready");
    refused(
        owner.poll_ready(Duration::ZERO),
        "poll_ready",
        VmState::Ready,
    );
    refused(
        owner.poll_started(Duration::ZERO),
        "poll_started",
        VmState::Ready,
    );
    assert_eq!(hooks::sent_so_far(name), sent);
    owner.request_start().expect("sent");
    refused(
        owner.poll_ready(Duration::ZERO),
        "poll_ready",
        VmState::Starting,
    );
    poll_until_done(&mut owner, poll_started).expect("started");
    refused(
        owner.poll_ready(Duration::ZERO),
        "poll_ready",
        VmState::Started,
    );
    refused(
        owner.poll_started(Duration::ZERO),
        "poll_started",
        VmState::Started,
    );
    owner.request_stop().expect("sent");
    refused(
        owner.poll_ready(Duration::ZERO),
        "poll_ready",
        VmState::Stopping,
    );
    refused(
        owner.poll_started(Duration::ZERO),
        "poll_started",
        VmState::Stopping,
    );
    owner.await_stopped(BOUND).expect("stopped");
    refused(
        owner.poll_ready(Duration::ZERO),
        "poll_ready",
        VmState::Finished,
    );
    refused(
        owner.poll_started(Duration::ZERO),
        "poll_started",
        VmState::Finished,
    );
    let after = hooks::sent_so_far(name);
    assert_eq!(
        (after.start, after.stop),
        (1, 1),
        "only the two real requests were sent"
    );
}

// ---------------------------------------------------------------------------------------------
// 2. Host extensions
// ---------------------------------------------------------------------------------------------

#[test]
fn host_functions_receive_converted_arguments_and_return_plain_lua_values() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.spawn(
        "ext_values",
        "ext.record(nil, true, 3, 2.5, 'x\\0y', '\\u{e9}', {a = 1, b = {true}}, {10, 20, 30}, {}, 1.0)\n\
         ext.record(nil, nil)\n\
         ext.record()\n\
         ext.record({[1.0] = 'a', [2.0] = 'b'}, {['k\\0'] = {}})\n\
         assert(ext.sum(40, 2) == 42)\n\
         local one, two, three = ext.pair()\n\
         assert(one == 1 and two == 'two')\n\
         assert(type(three) == 'table' and getmetatable(three) == nil)\n\
         assert(type(three.k) == 'table' and three.k[1] == true and #three.k == 1)\n\
         assert(select('#', ext.pair()) == 3)\n\
         -- 32 levels (the argument table is level 1) are accepted\n\
         local deep = {}\n\
         local cur = deep\n\
         for _ = 1, 31 do cur.n = {} cur = cur.n end\n\
         ext.record(deep)\n",
        0,
        None,
    );
    owner
        .wait_ready(BOUND)
        .expect("the main chunk's own assertions held");
    let seen = lock(&probe.seen).clone();
    assert_eq!(seen.len(), 5);
    assert_eq!(
        seen[0],
        vec![
            None,
            Some(ConfigValue::Bool(true)),
            int(3),
            Some(ConfigValue::Float(2.5)),
            text("x\u{0}y"),
            text("\u{e9}"),
            Some(ConfigValue::Table(
                [
                    ("a".to_owned(), ConfigValue::Integer(1)),
                    (
                        "b".to_owned(),
                        ConfigValue::Array(vec![ConfigValue::Bool(true)])
                    ),
                ]
                .into()
            )),
            Some(ConfigValue::Array(vec![
                ConfigValue::Integer(10),
                ConfigValue::Integer(20),
                ConfigValue::Integer(30)
            ])),
            Some(ConfigValue::Table(BTreeMap::new())),
            Some(ConfigValue::Float(1.0)),
        ]
    );
    assert_eq!(seen[1], vec![None, None], "nil positions are kept");
    assert_eq!(seen[2], Vec::new(), "no arguments is an empty slice");
    assert_eq!(
        seen[3],
        vec![
            Some(ConfigValue::Array(vec![
                ConfigValue::String("a".to_owned()),
                ConfigValue::String("b".to_owned())
            ])),
            Some(ConfigValue::Table(
                [("k\u{0}".to_owned(), ConfigValue::Table(BTreeMap::new()))].into()
            )),
        ]
    );
    let mut level = seen[4][0].clone().expect("the deep table");
    let mut levels = 1;
    while let ConfigValue::Table(entries) = level {
        match entries.get("n") {
            Some(next) => {
                level = next.clone();
                levels += 1;
            }
            None => break,
        }
    }
    assert_eq!(levels, 32);
}

#[test]
fn every_rejected_argument_is_a_named_lua_error_and_the_host_function_is_not_called() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.spawn(
        "ext_rejects",
        "local function rejected(reason, ...)\n\
           local ok, err = pcall(ext.record, ...)\n\
           assert(ok == false, 'not rejected: ' .. reason)\n\
           err = tostring(err)\n\
           assert(err:find(\"bad argument #\", 1, true) and err:find(\"'ext.record'\", 1, true), err)\n\
           assert(err:find(reason, 1, true), reason .. ' not in: ' .. err)\n\
         end\n\
         rejected('a function', print)\n\
         rejected('a userdata', config)\n\
         rejected('a thread', coroutine.create(print))\n\
         rejected('mixed or other keys', {1, a = 2})\n\
         rejected('mixed or other keys', {[1] = 1, [3] = 3})\n\
         rejected('mixed or other keys', {[true] = 1})\n\
         rejected('mixed or other keys', {[2] = 'x'})\n\
         rejected('mixed or other keys', {[0] = 1, [1] = 2})\n\
         rejected('a function cannot be converted (at [1])', {print})\n\
         rejected('mixed or other keys', {[{}] = 1})\n\
         local deep = {}\n\
         local cur = deep\n\
         for _ = 1, 32 do cur.n = {} cur = cur.n end\n\
         rejected('deeper than 32 levels', deep)\n\
         local cycle = {}\n\
         cycle.self = cycle\n\
         rejected('deeper than 32 levels', cycle)\n\
         rejected('not valid UTF-8', string.char(255))\n\
         rejected('not valid UTF-8', {a = {string.char(255)}})\n\
         rejected('a table key is not valid UTF-8', {[string.char(255)] = 1})\n\
         rejected('a table key is not valid UTF-8', {fine = {[string.char(0xC3)] = 1}})\n\
         rejected('a function cannot be converted (at .a.b)', {a = {b = print}})\n\
         local ok, err = pcall(ext.record, 1, 2, string.char(0xC3))\n\
         assert(not ok and tostring(err):find('#3', 1, true), tostring(err))\n\
         -- valid UTF-8 and embedded NUL are accepted, so nothing above was rejected for its shape\n\
         ext.record('ok\\0', '\\u{1F600}', {['\\u{e9}'] = 1})\n",
        0,
        None,
    );
    owner.wait_ready(BOUND).expect("every rejection held");
    let seen = lock(&probe.seen).clone();
    assert_eq!(
        seen.len(),
        1,
        "the host function ran only for the accepted call: {seen:?}"
    );
    assert_eq!(seen[0][0], text("ok\u{0}"));
    assert_eq!(seen[0][1], text("\u{1F600}"));
}

#[test]
fn an_err_from_a_host_function_is_a_catchable_lua_error_with_its_text() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.spawn(
        "ext_err",
        "local ok, err = pcall(ext.fail)\n\
         assert(ok == false and tostring(err):find('the host said no', 1, true), tostring(err))\n\
         local ok2, err2 = xpcall(ext.fail, function(e) return 'handled: ' .. tostring(e) end)\n\
         assert(ok2 == false and tostring(err2):find('handled: ', 1, true))\n\
         ext.mark('survived')\n",
        0,
        None,
    );
    owner.wait_ready(BOUND).expect("ready");
    assert_eq!(probe.marks(), ["survived"]);

    let (mut owner, _dir) = probe.spawn("ext_err_uncaught", "ext.fail()", 0, None);
    let error = owner.wait_ready(BOUND).expect_err("uncaught");
    assert!(
        script_error(&error, "ext_err_uncaught")
            .0
            .contains("the host said no")
    );
}

#[test]
fn the_main_chunk_can_call_the_extension_and_it_is_installed_after_the_standard_surface() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.spawn(
        "ext_main",
        "log.info('has ext: ' .. type(ext) .. ' ' .. type(ext.sum) .. ' ' .. type(config))\n\
         assert(ext.sum(1, 2) == 3)\n",
        0,
        None,
    );
    owner.wait_ready(BOUND).expect("ready");
    assert_eq!(
        records("ext_main"),
        ["INFO has ext: table function userdata"]
    );
}

#[test]
fn a_panicking_extension_function_ends_the_owner_as_a_panic_even_under_pcall_and_xpcall() {
    let cases: [(&str, &str); 7] = [
        ("panic_direct", "ext.boom()\next.mark('after')"),
        ("panic_pcall", "pcall(ext.boom)\next.mark('after')"),
        (
            "panic_xpcall",
            "xpcall(ext.boom, function(e) ext.mark('handler') return e end)\next.mark('after')",
        ),
        (
            "panic_resume",
            "local co = coroutine.create(ext.boom)\ncoroutine.resume(co)\next.mark('after')",
        ),
        (
            "panic_loop",
            "for _ = 1, 100000 do pcall(ext.boom) ext.mark('loop') end\next.mark('after')",
        ),
        (
            "panic_nested",
            "pcall(function() pcall(function() pcall(ext.boom) ext.mark('inner') end) ext.mark('middle') end)\next.mark('after')",
        ),
        (
            "panic_wrapped",
            "local f = coroutine.wrap(function() pcall(ext.boom) ext.mark('co') end)\npcall(f)\next.mark('after')",
        ),
    ];
    for (name, script) in cases {
        let probe = Probe::new();
        let dropped = track_state_drop(name, |_| {});
        let (mut owner, _dir) = probe.spawn(name, script, 0, None);
        let error = owner.wait_ready(BOUND).expect_err(name);
        assert!(
            matches!(&error, VmError::Panicked { owner: named, message }
                if named == name && message.contains("boom msg")),
            "{name}: {error:?}"
        );
        assert_eq!(owner.state(), VmState::Ending);
        assert!(owner.wait_finished(BOUND), "{name}");
        assert!(
            dropped.load(Ordering::SeqCst),
            "{name}: the state was dropped"
        );
        assert!(
            probe.marks().is_empty(),
            "{name}: no further Lua ran, handler included: {:?}",
            probe.marks()
        );
    }
}

#[test]
fn a_panic_with_an_owned_string_payload_keeps_its_message_and_is_not_a_cancellation() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.spawn(
        "panic_owned",
        "pcall(ext.boom_text)\next.mark('after')",
        0,
        None,
    );
    let error = owner.wait_ready(BOUND).expect_err("panicked");
    assert!(
        matches!(&error, VmError::Panicked { message, .. } if message == "owned boom msg"),
        "{error:?}"
    );
    assert!(!matches!(error, VmError::Cancelled { .. }));
    assert!(owner.wait_finished(BOUND));
    assert!(probe.marks().is_empty());
}

#[test]
fn a_panic_in_startup_and_in_shutdown_is_terminal_through_the_lifecycle_waits() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.spawn(
        "panic_startup",
        "function startup() pcall(ext.boom) ext.mark('after') end",
        0,
        None,
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.request_start().expect("sent");
    let error = poll_until_done(&mut owner, poll_started).expect_err("startup panicked");
    assert!(matches!(&error, VmError::Panicked { message, .. } if message.contains("boom msg")));
    assert!(owner.wait_finished(BOUND));
    assert!(probe.marks().is_empty());

    let (mut owner, _dir) = probe.spawn(
        "panic_shutdown",
        "function shutdown() xpcall(ext.boom, function() ext.mark('handler') end) ext.mark('after') end",
        0,
        None,
    );
    owner.wait_ready(BOUND).expect("ready");
    let error = owner.stop(BOUND).expect_err("shutdown panicked");
    assert!(matches!(&error, VmError::Panicked { message, .. } if message.contains("boom msg")));
    assert!(owner.wait_finished(BOUND));
    assert!(probe.marks().is_empty());
}

#[test]
fn an_invalid_extension_is_refused_by_spawn_and_no_thread_starts() {
    let dir = Dir::new();
    let entry = dir.script("");
    let function = || host(|_| Ok(Vec::new()));
    let cases: Vec<(&str, Extension)> = vec![
        ("ext_bad_empty", Extension::new("")),
        ("ext_bad_upper", Extension::new("Bad")),
        ("ext_bad_dash", Extension::new("has-dash")),
        ("ext_bad_log", Extension::new("log")),
        ("ext_bad_config", Extension::new("config")),
        ("ext_bad_string", Extension::new("string")),
        ("ext_bad_startup", Extension::new("startup")),
        (
            "ext_bad_function_twice",
            Extension::new("ok")
                .function("f", function())
                .function("f", function()),
        ),
        (
            "ext_bad_function_setter",
            Extension::new("ok")
                .function("f", function())
                .callback("f", "x"),
        ),
        (
            "ext_bad_setter_twice",
            Extension::new("ok").callback("s", "x").callback("s", "y"),
        ),
        (
            "ext_bad_callback_twice",
            Extension::new("ok").callback("s", "x").callback("t", "x"),
        ),
        (
            "ext_bad_name",
            Extension::new("ok").function("not-ident", function()),
        ),
        (
            "ext_bad_empty_name",
            Extension::new("ok").function("", function()),
        ),
        (
            "ext_bad_callback_name",
            Extension::new("ok").callback("s", "1x"),
        ),
    ];
    for (name, extension) in cases {
        let mut settings = VmSettings::new(name, entry.clone(), super::empty_config());
        settings.extensions = vec![extension];
        let error = VmOwner::spawn(settings).expect_err(name);
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::InvalidInput,
            "{name}: {error}"
        );
        assert!(!hooks::was_spawned(name), "{name}: no thread was created");
    }
    let mut settings = VmSettings::new("ext_fine", entry, super::empty_config());
    settings.extensions = vec![
        Extension::new("window_2")
            .function("open", function())
            .callback("on_close", "close"),
    ];
    let mut owner = VmOwner::spawn(settings).expect("a valid extension");
    owner.wait_ready(BOUND).expect("ready");
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn every_global_a_fresh_state_defines_is_reserved() {
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default()).expect("a state");
    let mut checked = 0;
    for pair in lua.globals().pairs::<String, mlua::Value>() {
        let (name, _) = pair.expect("a global");
        if crate::owner::valid_name(&name) {
            checked += 1;
            assert!(
                Extension::new(&name).validate().is_err(),
                "`{name}` is a standard global and must be reserved"
            );
        }
    }
    assert!(checked >= 20, "the check looked at {checked} globals");
}

#[test]
fn settings_default_to_no_extensions_no_calls_and_no_wake_and_show_a_wake_as_present() {
    let settings = VmSettings::new("s", "entry.lua", super::empty_config());
    assert!(settings.extensions.is_empty());
    assert_eq!(settings.call_limit, 0);
    assert!(settings.on_report.is_none());
    assert!(format!("{settings:?}").contains("on_report: \"absent\""));
    let mut with_wake = settings.clone();
    with_wake.on_report = Some(Arc::new(|| {}));
    assert!(format!("{with_wake:?}").contains("on_report: \"present\""));
    with_wake.extensions = vec![Extension::new("ext").callback("on_x", "x")];
    assert!(format!("{with_wake:?}").contains("Extension"));
}

#[test]
fn setters_register_replace_and_clear_callbacks_and_reject_anything_else() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started(
        "setters",
        "local function rejects(...)\n\
           local ok, err = pcall(ext.on_add, ...)\n\
           assert(ok == false and tostring(err):find('function or nil expected', 1, true), tostring(err))\n\
         end\n\
         rejects(5)\nrejects('x')\nrejects({})\nrejects(true)\nrejects()\n\
         ext.on_add(function() return 'first' end)\n\
         ext.on_add(function() return 'second' end)\n\
         ext.on_echo(function() return 'echo' end)\n\
         ext.on_echo(nil)\n\
         function startup() end\n",
        4,
    );
    owner.post("add", Vec::new()).expect("posted");
    owner.post("echo", Vec::new()).expect("posted");
    let first = next_result(&mut owner);
    assert_eq!(
        returned(&first),
        [text("second")],
        "the second registration replaced the first"
    );
    let second = next_result(&mut owner);
    assert_eq!(
        second.outcome,
        CallOutcome::NotRegistered,
        "nil cleared the callback"
    );
    owner.stop(BOUND).expect("stopped");
}

// ---------------------------------------------------------------------------------------------
// 3. Posted calls
// ---------------------------------------------------------------------------------------------

fn state_refused(result: Result<crate::CallId, VmError>, state: VmState) {
    match result {
        Err(VmError::State {
            call: "post",
            state: at,
            ..
        }) => assert_eq!(at, state),
        other => panic!("expected a state error in {state}, got {other:?}"),
    }
}

/// Waits (bounded) until the owner's thread has ended, without observing it through the handle's
/// `wait_finished`, so the handle stays Started.
fn wait_thread_ended(owner: &VmOwner) {
    let end = Instant::now() + BOUND;
    while !owner.is_finished() {
        assert!(Instant::now() < end, "the owner thread never ended");
        thread::sleep(Duration::from_millis(1));
    }
}

/// Counts report wakes and records the thread each ran on.
struct Wakes {
    count: Arc<AtomicUsize>,
    threads: Arc<Mutex<Vec<ThreadId>>>,
}

impl Wakes {
    fn new() -> Self {
        Self {
            count: Arc::new(AtomicUsize::new(0)),
            threads: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn wake(&self) -> Wake {
        let count = Arc::clone(&self.count);
        let threads = Arc::clone(&self.threads);
        Arc::new(move || {
            lock(&threads).push(thread::current().id());
            count.fetch_add(1, Ordering::SeqCst);
        })
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn wait_for(&self, wanted: usize) {
        let end = Instant::now() + BOUND;
        while self.count() < wanted {
            assert!(
                Instant::now() < end,
                "only {} of {wanted} wakes happened",
                self.count()
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
}

#[test]
fn post_checks_state_then_disconnection_then_the_limit_and_a_refusal_sends_nothing() {
    let name = "post_order";
    let probe = Probe::new();
    let wakes = Wakes::new();
    let (mut owner, _dir) = probe.spawn(
        name,
        "ext.on_blocker(function() ext.count() ext.entered() ext.block() return 'done' end)\n\
         ext.on_echo(function(...) ext.count() return ... end)\n\
         ext.on_boom(function() ext.count() ext.boom() end)\n",
        1,
        Some(wakes.wake()),
    );
    // take_result is legal in every state and never waits.
    assert!(owner.take_result().is_none());
    state_refused(owner.post("echo", Vec::new()), VmState::Initialising);
    owner.wait_ready(BOUND).expect("ready");
    state_refused(owner.post("echo", Vec::new()), VmState::Ready);
    owner.request_start().expect("sent");
    state_refused(owner.post("echo", Vec::new()), VmState::Starting);
    owner.await_started(BOUND).expect("started");
    assert_eq!(hooks::sent_so_far(name).call, 0, "no refusal sent a call");

    // Started, within the limit: sent.
    let first = owner.post("blocker", Vec::new()).expect("posted");
    assert_eq!(first.get(), 1);
    assert_eq!(hooks::sent_so_far(name).call, 1);
    probe
        .entered
        .recv_timeout(BOUND)
        .expect("the callback runs");
    // At the limit with the call running: Full, and nothing is sent (proved by the send count and by
    // the callback's run count, which a stray send would raise once the callback is released).
    for _ in 0..3 {
        assert_eq!(
            owner.post("echo", Vec::new()),
            Err(VmError::Full {
                owner: name.to_owned()
            })
        );
    }
    assert_eq!(hooks::sent_so_far(name).call, 1);
    assert_eq!(owner.in_flight(), 1);
    probe.release.send(()).expect("release the callback");
    wakes.wait_for(3); // ready, started, the call's result
    // The result is waiting in the handle: it still counts as in flight.
    assert_eq!(
        owner.post("echo", Vec::new()),
        Err(VmError::Full {
            owner: name.to_owned()
        })
    );
    assert_eq!(hooks::sent_so_far(name).call, 1);
    assert_eq!(probe.calls(), 1, "only the first call ever ran");
    let result = owner.take_result().expect("the result");
    assert_eq!((result.id, result.callback.as_str()), (first, "blocker"));
    assert_eq!(returned(&result), [text("done")]);
    assert!(owner.take_result().is_none());
    // Taking the result freed the place.
    let second = owner
        .post("echo", vec![ConfigValue::Integer(7)])
        .expect("posted again");
    assert!(second > first);
    assert_eq!(returned(&next_result(&mut owner)), [int(7)]);
    assert_eq!(probe.calls(), 2);

    // The thread ends (a panic in the callback) while the handle is still Started and at its limit:
    // Disconnected comes before Full, and nothing is sent.
    owner.post("boom", Vec::new()).expect("posted");
    wait_thread_ended(&owner);
    assert_eq!(owner.state(), VmState::Started);
    let sent = hooks::sent_so_far(name);
    assert_eq!(sent.call, 3);
    assert_eq!(
        owner.post("echo", Vec::new()),
        Err(VmError::Disconnected {
            owner: name.to_owned()
        })
    );
    assert_eq!(
        hooks::sent_so_far(name),
        sent,
        "the refusal sent nothing, not even to the dead thread"
    );
    assert_eq!(
        probe.calls(),
        3,
        "the callback ran once more for boom, and no more"
    );
    assert!(
        owner.take_result().is_none(),
        "the panicked call has no result"
    );
    // The terminal report is seen through the legal lifecycle waits.
    owner.request_stop().expect("sent");
    let error = owner
        .await_stopped(BOUND)
        .expect_err("the panic is the terminal report");
    assert!(
        matches!(&error, VmError::Panicked { message, .. } if message.contains("boom msg")),
        "{error:?}"
    );
    assert_eq!(owner.state(), VmState::Ending);
    assert!(owner.wait_finished(BOUND));
    assert_eq!(owner.in_flight(), 0, "nothing is in flight once Finished");
    state_refused(owner.post("echo", Vec::new()), VmState::Finished);
    assert_eq!(hooks::sent_so_far(name).call, 3);
}

#[test]
fn a_stale_started_handle_below_its_limit_is_disconnected_too_and_ending_is_a_state_error() {
    let name = "post_stale";
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started(name, "ext.on_boom(function() ext.boom() end)", 4);
    owner.post("boom", Vec::new()).expect("posted");
    wait_thread_ended(&owner);
    assert!(matches!(
        owner.post("boom", Vec::new()),
        Err(VmError::Disconnected { .. })
    ));
    assert_eq!(hooks::sent_so_far(name).call, 1);

    let (mut owner, _dir) = probe.started("post_ending", "ext.on_echo(function() end)", 4);
    owner.cancel();
    state_refused(owner.post("echo", Vec::new()), VmState::Ending);
    assert!(owner.wait_finished(BOUND));
    assert_eq!(hooks::sent_so_far("post_ending").call, 0);
}

#[test]
fn results_come_back_in_posting_order_with_ids_values_and_nil_positions() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started(
        "post_results",
        "ext.on_add(function(a, b) return a + b end)\n\
         ext.on_echo(function(...) return ... end)\n\
         ext.on_shape(function(which)\n\
           if which == 1 then return\n\
           elseif which == 2 then return nil\n\
           elseif which == 3 then return nil, false, nil\n\
           else return {1, {a = 'x'}}, 2.5, 'tail' end\n\
         end)\n",
        16,
    );
    let calls = [
        (
            "add",
            vec![ConfigValue::Integer(2), ConfigValue::Integer(3)],
        ),
        (
            "echo",
            vec![
                ConfigValue::String("a\u{0}b".to_owned()),
                ConfigValue::Bool(true),
            ],
        ),
        ("nothing", vec![ConfigValue::Integer(1)]),
        ("shape", vec![ConfigValue::Integer(1)]),
        ("shape", vec![ConfigValue::Integer(2)]),
        ("shape", vec![ConfigValue::Integer(3)]),
        ("shape", vec![ConfigValue::Integer(4)]),
        (
            "echo",
            vec![ConfigValue::Table(
                [(
                    "k".to_owned(),
                    ConfigValue::Array(vec![ConfigValue::Float(0.5)]),
                )]
                .into(),
            )],
        ),
    ];
    let mut ids = Vec::new();
    for (callback, args) in &calls {
        ids.push(owner.post(callback, args.clone()).expect("posted"));
    }
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "ordered ids");
    let results: Vec<CallResult> = calls.iter().map(|_| next_result(&mut owner)).collect();
    for ((result, id), (callback, _)) in results.iter().zip(&ids).zip(&calls) {
        assert_eq!((result.id, result.callback.as_str()), (*id, *callback));
    }
    assert_eq!(returned(&results[0]), [int(5)]);
    assert_eq!(
        returned(&results[1]),
        [text("a\u{0}b"), Some(ConfigValue::Bool(true))]
    );
    assert_eq!(results[2].outcome, CallOutcome::NotRegistered);
    assert_eq!(returned(&results[3]), []);
    assert_eq!(returned(&results[4]), [None]);
    assert_eq!(
        returned(&results[5]),
        [None, Some(ConfigValue::Bool(false)), None]
    );
    assert_eq!(
        returned(&results[6]),
        [
            Some(ConfigValue::Array(vec![
                ConfigValue::Integer(1),
                ConfigValue::Table([("a".to_owned(), ConfigValue::String("x".to_owned()))].into())
            ])),
            Some(ConfigValue::Float(2.5)),
            text("tail")
        ]
    );
    assert_eq!(
        returned(&results[7]),
        [Some(ConfigValue::Table(
            [(
                "k".to_owned(),
                ConfigValue::Array(vec![ConfigValue::Float(0.5)])
            )]
            .into()
        ))]
    );
    assert_eq!(owner.in_flight(), 0);
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn a_return_value_that_cannot_be_converted_fails_the_call_and_the_vm_keeps_serving() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started(
        "post_invalid",
        "ext.on_invalid(function(which)\n\
           if which == 1 then return 'fine', string.char(255)\n\
           elseif which == 2 then return {[string.char(255)] = 1}\n\
           elseif which == 3 then return print\n\
           elseif which == 4 then return 1, {1, 2, [4] = 4}\n\
           else return {nested = {'ok', string.char(0xC3)}} end\n\
         end)\n\
         ext.on_touch(function() return 'still serving' end)\n",
        8,
    );
    let expected = [
        (1, "return value #2: not valid UTF-8"),
        (
            2,
            "return value #1: a table key is not valid UTF-8 (at .<key>)",
        ),
        (3, "return value #1: a function cannot be converted"),
        (4, "return value #2: a table with mixed or other keys"),
        (5, "return value #1: not valid UTF-8 (at .nested[2])"),
    ];
    for (which, wanted) in expected {
        owner
            .post("invalid", vec![ConfigValue::Integer(which)])
            .expect("posted");
        let result = next_result(&mut owner);
        match result.outcome {
            CallOutcome::Failed { message, traceback } => {
                assert_eq!(message, wanted);
                assert!(
                    traceback.contains("no Lua frame was running"),
                    "{traceback}"
                );
            }
            other => panic!("case {which}: expected a failed call, got {other:?}"),
        }
        owner.post("touch", Vec::new()).expect("posted");
        assert_eq!(returned(&next_result(&mut owner)), [text("still serving")]);
        assert_eq!(owner.state(), VmState::Started);
    }
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn a_failing_callback_is_caught_and_not_rolled_back_and_the_vm_keeps_serving() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started(
        "post_failing",
        "ext.on_fail(function() touched = 'set before raising' ext.count() error('callback broke') end)\n\
         ext.on_touched(function() return touched end)\n\
         ext.on_echo(function(...) return ... end)\n\
         function shutdown() log.info('shutdown still runs') end\n",
        4,
    );
    owner.post("fail", Vec::new()).expect("posted");
    let failed = next_result(&mut owner);
    match &failed.outcome {
        CallOutcome::Failed { message, traceback } => {
            assert!(message.contains("callback broke"), "{message}");
            assert!(traceback.contains("stack traceback"), "{traceback}");
        }
        other => panic!("expected a failed call, got {other:?}"),
    }
    assert_eq!(
        owner.state(),
        VmState::Started,
        "posting does not change the state"
    );
    owner.post("touched", Vec::new()).expect("posted");
    assert_eq!(
        returned(&next_result(&mut owner)),
        [text("set before raising")],
        "what the callback did before failing is not undone"
    );
    owner
        .post("echo", vec![ConfigValue::Integer(1)])
        .expect("posted");
    assert_eq!(returned(&next_result(&mut owner)), [int(1)]);
    owner.stop(BOUND).expect("shutdown() still runs");
    assert_eq!(records("post_failing"), ["INFO shutdown still runs"]);
}

#[test]
fn a_call_posted_before_request_stop_runs_before_shutdown_and_its_result_survives_the_wait() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started(
        "post_before_stop",
        "ext.on_note(function(n) log.info('call ' .. n) return n end)\n\
         function shutdown() log.info('shutdown') end\n",
        4,
    );
    for n in 1..=3 {
        owner
            .post("note", vec![ConfigValue::Integer(n)])
            .expect("posted");
    }
    owner.request_stop().expect("sent");
    owner.await_stopped(BOUND).expect("stopped");
    assert_eq!(owner.state(), VmState::Finished);
    assert_eq!(
        records("post_before_stop"),
        ["INFO call 1", "INFO call 2", "INFO call 3", "INFO shutdown"]
    );
    // The results arrived while the handle waited for the stop: kept, not mistaken for it.
    for n in 1..=3 {
        let result = owner.take_result().expect("a kept result");
        assert_eq!(returned(&result), [int(n)]);
    }
    assert!(owner.take_result().is_none());
    assert_eq!(owner.in_flight(), 0);
}

#[test]
fn cancel_interrupts_a_looping_callback_and_a_queued_call_and_neither_yields_a_result() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started(
        "post_cancel",
        "ext.on_forever(function() ext.entered() while true do end end)\n\
         ext.on_echo(function() ext.count() end)\n",
        4,
    );
    owner.post("forever", Vec::new()).expect("posted");
    owner.post("echo", Vec::new()).expect("posted");
    probe
        .entered
        .recv_timeout(BOUND)
        .expect("the loop is running");
    owner.cancel();
    assert!(
        owner.wait_finished(BOUND),
        "the hook ends the loop and the thread ends"
    );
    assert!(owner.take_result().is_none());
    assert_eq!(owner.in_flight(), 0, "once Finished nothing is in flight");
    assert_eq!(probe.calls(), 0, "the queued call never ran");
    state_refused(owner.post("echo", Vec::new()), VmState::Finished);
}

#[test]
fn a_panic_in_a_posted_callback_is_owner_fatal_also_under_pcall_and_xpcall() {
    let cases = [
        (
            "post_panic_direct",
            "ext.on_boom(function() ext.boom() ext.mark('after') end)",
        ),
        (
            "post_panic_pcall",
            "ext.on_boom(function() pcall(ext.boom) ext.mark('after') end)",
        ),
        (
            "post_panic_xpcall",
            "ext.on_boom(function() xpcall(ext.boom, function(e) ext.mark('handler') return e end) ext.mark('after') end)",
        ),
        (
            "post_panic_resume",
            "ext.on_boom(function() coroutine.resume(coroutine.create(ext.boom)) ext.mark('after') end)",
        ),
    ];
    for (name, script) in cases {
        let probe = Probe::new();
        let (mut owner, _dir) = probe.started(name, script, 2);
        owner.post("boom", Vec::new()).expect("posted");
        owner.request_stop().expect("sent");
        let error = owner.await_stopped(BOUND).expect_err(name);
        assert!(
            matches!(&error, VmError::Panicked { owner: named, message }
                if named == name && message.contains("boom msg")),
            "{name}: {error:?}"
        );
        assert!(owner.wait_finished(BOUND), "{name}");
        assert!(
            owner.take_result().is_none(),
            "{name}: no result for the panicked call"
        );
        assert_eq!(owner.in_flight(), 0, "{name}");
        assert!(probe.marks().is_empty(), "{name}: {:?}", probe.marks());
    }
}

// ---------------------------------------------------------------------------------------------
// 4. The report wake
// ---------------------------------------------------------------------------------------------

#[test]
fn the_report_wake_runs_on_the_owner_thread_after_each_report_and_once_at_the_end() {
    let name = "wake_all";
    let owner_thread: Arc<Mutex<Option<ThreadId>>> = Arc::new(Mutex::new(None));
    let recorded = Arc::clone(&owner_thread);
    hooks::register(
        name,
        Hooks {
            at: Box::new(move |point, _| {
                if point == Point::StateCreated {
                    *lock(&recorded) = Some(thread::current().id());
                }
            }),
            install: None,
        },
    );
    let probe = Probe::new();
    let wakes = Wakes::new();
    let (mut owner, _dir) = probe.spawn(
        name,
        "ext.on_echo(function(...) return ... end)\n\
         function startup() end\nfunction shutdown() end\n",
        2,
        Some(wakes.wake()),
    );
    owner.wait_ready(BOUND).expect("ready");
    wakes.wait_for(1); // ready
    owner.start(BOUND).expect("started");
    wakes.wait_for(2); // the start result
    owner
        .post("echo", vec![ConfigValue::Integer(1)])
        .expect("posted");
    wakes.wait_for(3); // the call result
    assert_eq!(returned(&next_result(&mut owner)), [int(1)]);
    owner.request_stop().expect("sent");
    wakes.wait_for(4); // the stop result
    owner.await_stopped(BOUND).expect("stopped");
    // `await_stopped` returned after the thread was observed finished, so the final wake has run.
    assert_eq!(wakes.count(), 5, "ready, start, call, stop, and the end");
    let owner_thread = lock(&owner_thread).expect("the owner thread's id");
    let threads = lock(&wakes.threads).clone();
    assert!(
        threads.iter().all(|id| *id == owner_thread),
        "every wake ran on the owner thread"
    );
    assert_ne!(owner_thread, thread::current().id());
}

#[test]
fn a_failure_wakes_and_so_does_the_end_and_a_cancelled_idle_owner_wakes_only_at_the_end() {
    let probe = Probe::new();
    let wakes = Wakes::new();
    let (mut owner, _dir) = probe.spawn("wake_failure", "error('broke')", 0, Some(wakes.wake()));
    owner.wait_ready(BOUND).expect_err("the main chunk failed");
    assert!(owner.wait_finished(BOUND));
    assert_eq!(wakes.count(), 2, "the failure report and the end");

    let wakes = Wakes::new();
    let (mut owner, _dir) = probe.spawn("wake_cancel", "", 0, Some(wakes.wake()));
    owner.wait_ready(BOUND).expect("ready");
    wakes.wait_for(1);
    owner.cancel();
    assert!(owner.wait_finished(BOUND));
    assert_eq!(wakes.count(), 2, "ready, and the end (no result was sent)");
}

#[test]
fn a_panicking_wake_changes_no_outcome() {
    let probe = Probe::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let wake: Wake = Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        panic!("the wake panics");
    });
    let (mut owner, _dir) = probe.spawn(
        "wake_panics",
        "ext.on_echo(function(...) return ... end)\nfunction shutdown() log.info('down') end",
        2,
        Some(Arc::clone(&wake)),
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    owner
        .post("echo", vec![ConfigValue::Integer(9)])
        .expect("posted");
    assert_eq!(returned(&next_result(&mut owner)), [int(9)]);
    owner.stop(BOUND).expect("stopped");
    assert_eq!(records("wake_panics"), ["INFO down"]);
    assert_eq!(calls.load(Ordering::SeqCst), 5, "every wake was attempted");

    let (mut owner, _dir) =
        probe.spawn("wake_panics_fail", "error('still reported')", 0, Some(wake));
    let error = owner
        .wait_ready(BOUND)
        .expect_err("the failure is still reported");
    assert!(
        script_error(&error, "wake_panics_fail")
            .0
            .contains("still reported")
    );
    assert!(owner.wait_finished(BOUND));
}

// ---------------------------------------------------------------------------------------------
// 5. Independence
// ---------------------------------------------------------------------------------------------

#[test]
fn calls_to_one_owner_do_not_run_on_or_wait_for_the_other() {
    let slow = Probe::new();
    let quick = Probe::new();
    let (mut a, _dir_a) = slow.started(
        "indep_a",
        "ext.on_blocker(function() ext.entered() ext.block() return 'a done' end)",
        2,
    );
    let (mut b, _dir_b) = quick.started(
        "indep_b",
        "ext.on_echo(function(...) ext.count() return ... end)",
        2,
    );
    a.post("blocker", Vec::new()).expect("posted");
    slow.entered
        .recv_timeout(BOUND)
        .expect("a is inside its callback");
    // While `a` is stuck in its callback, `b` serves calls and stops.
    b.post("echo", vec![ConfigValue::Integer(4)])
        .expect("posted");
    assert_eq!(returned(&next_result(&mut b)), [int(4)]);
    assert_eq!(quick.calls(), 1);
    assert_eq!(slow.calls(), 0);
    b.stop(BOUND).expect("b stops while a is busy");
    assert!(a.take_result().is_none(), "a's call has not finished");
    slow.release.send(()).expect("release a");
    assert_eq!(returned(&next_result(&mut a)), [text("a done")]);
    a.stop(BOUND).expect("a stops");
}

#[test]
fn a_panic_raised_from_a_finalizer_cannot_unwind_into_the_script_but_still_ends_the_owner_as_a_panic()
 {
    // Lua turns an error in a `__gc` finalizer into a warning, so the script goes on and finishes
    // normally; the recorded panic still decides the outcome.
    let probe = Probe::new();
    let (mut owner, _dir) = probe.spawn(
        "panic_gc_main",
        "setmetatable({}, {__gc = function() ext.boom() end})\ncollectgarbage()\n",
        0,
        None,
    );
    let error = owner.wait_ready(BOUND).expect_err("the finalizer panicked");
    assert!(
        matches!(&error, VmError::Panicked { message, .. } if message.contains("boom msg")),
        "{error:?}"
    );
    assert!(owner.wait_finished(BOUND));

    let (mut owner, _dir) = probe.started(
        "panic_gc_call",
        "ext.on_boom(function()\n\
           setmetatable({}, {__gc = function() ext.boom() end})\n\
           collectgarbage()\n\
           return 'returned normally'\n\
         end)\n",
        2,
    );
    owner.post("boom", Vec::new()).expect("posted");
    owner.request_stop().expect("sent");
    let error = owner
        .await_stopped(BOUND)
        .expect_err("the owner ended as a panic");
    assert!(
        matches!(&error, VmError::Panicked { message, .. } if message.contains("boom msg")),
        "{error:?}"
    );
    assert!(owner.wait_finished(BOUND));
    assert!(
        owner.take_result().is_none(),
        "the call that panicked has no result"
    );
}

// ---------------------------------------------------------------------------------------------
// Fix 1: wrappers independent of script globals, and panics during teardown
// ---------------------------------------------------------------------------------------------

#[test]
fn rebinding_or_removing_type_neither_bypasses_the_post_panic_guard_nor_breaks_xpcall() {
    let rebinds = [
        (
            "rebind_type_fn",
            "type = function() return 'not-function' end",
        ),
        ("rebind_type_nil", "type = nil"),
        (
            "rebind_many",
            "type = nil select = nil assert = nil error = nil tostring = nil pcall = nil \
             setmetatable = nil getmetatable = nil rawequal = nil next = nil _G.xpcall = nil",
        ),
    ];
    for (name, rebind) in rebinds {
        let probe = Probe::new();
        let (mut owner, _dir) = probe.spawn(
            name,
            &format!(
                "local xpcall, mark, boom = xpcall, ext.mark, ext.boom\n\
                 {rebind}\n\
                 xpcall(boom, function(e) mark('handler') return e end)\n\
                 mark('after')\n"
            ),
            0,
            None,
        );
        let error = owner.wait_ready(BOUND).expect_err(name);
        assert!(
            matches!(&error, VmError::Panicked { message, .. } if message.contains("boom msg")),
            "{name}: {error:?}"
        );
        assert!(owner.wait_finished(BOUND));
        assert!(probe.marks().is_empty(), "{name}: {:?}", probe.marks());
    }
}

#[test]
fn an_ordinary_xpcall_and_pcall_still_work_after_type_and_friends_are_removed() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.spawn(
        "ordinary_xpcall",
        "local xpcall, pcall, mark, tostring = xpcall, pcall, ext.mark, tostring\n\
         type = nil\n\
         local ok, v = xpcall(function(a, b) return a + b end, function(e) return e end, 3, 4)\n\
         assert(ok == true and v == 7)\n\
         local ok2, e2 = xpcall(function() error('x', 0) end, function(e) mark('handler') return 'handled ' .. e end)\n\
         assert(ok2 == false and e2 == 'handled x', tostring(e2))\n\
         assert(select('#', xpcall(function() return 1, 2, 3 end, print)) == 4)\n\
         local ok3 = pcall(error, 'y')\n\
         assert(ok3 == false)\n\
         assert(not pcall(xpcall))\n\
         mark('done')\n",
        0,
        None,
    );
    owner
        .wait_ready(BOUND)
        .expect("the standard behaviour is preserved");
    assert_eq!(probe.marks(), ["handler", "done"]);
}

#[test]
fn a_finite_close_handler_may_still_run_after_a_host_panic_and_the_outcome_stays_panicked() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.spawn(
        "panic_close",
        "local function f()\n\
           local guard <close> = setmetatable({}, {__close = function() ext.mark('closed') end})\n\
           pcall(ext.boom)\n\
           ext.mark('after')\n\
         end\n\
         f()\n\
         ext.mark('later')\n",
        0,
        None,
    );
    let error = owner.wait_ready(BOUND).expect_err("panicked");
    assert!(
        matches!(&error, VmError::Panicked { message, .. } if message.contains("boom msg")),
        "{error:?}"
    );
    assert!(owner.wait_finished(BOUND));
    assert_eq!(probe.marks(), ["closed"], "cleanup ran; nothing else did");
}

#[test]
fn setters_reject_two_arguments_and_register_nothing() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started(
        "setter_two",
        "local ok, err = pcall(ext.on_add, function() return 1 end, function() return 2 end)\n\
         assert(ok == false and tostring(err):find('bad argument', 1, true), tostring(err))\n",
        2,
    );
    owner.post("add", Vec::new()).expect("posted");
    assert_eq!(next_result(&mut owner).outcome, CallOutcome::NotRegistered);
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn a_host_panic_during_state_drop_makes_stop_return_panicked_never_ok() {
    let probe = Probe::new();
    let name = "teardown_stop";
    let wakes = Wakes::new();
    let (mut owner, _dir) = probe.spawn(
        name,
        "keeper = setmetatable({}, {__gc = function() ext.boom() end})\n",
        0,
        Some(wakes.wake()),
    );
    owner
        .wait_ready(BOUND)
        .expect("ready: nothing has panicked yet");
    owner.start(BOUND).expect("started");
    let error = owner
        .stop(BOUND)
        .expect_err("the finalizer panicked during the drop");
    assert!(
        matches!(&error, VmError::Panicked { owner: named, message }
            if named == name && message.contains("boom msg")),
        "{error:?}"
    );
    assert!(owner.wait_finished(BOUND));
    assert_eq!(owner.state(), VmState::Finished);
    assert!(
        records(name).is_empty(),
        "the panic was reported to the caller, so it is not also logged: {:?}",
        records(name)
    );
    // The final wake still comes after the state drop and after the failure report.
    assert_eq!(
        wakes.count(),
        4,
        "ready, start, the failed stop, and the end"
    );
}

/// The record a teardown panic leaves when an earlier failure was already selected as the outcome.
const TEARDOWN_RECORD: &str = "ERROR host function panicked during teardown: owned boom msg";

#[test]
fn a_failure_selected_before_teardown_stands_and_the_teardown_panic_is_logged_once() {
    // The rule is about selection, not delivery: the failure is chosen before the state is dropped (the
    // report is only sent after the drop), it stays the primary cause, and the late panic gets one record.
    let probe = Probe::new();

    // A Lua error in the main chunk.
    let name = "teardown_after_main_error";
    let (mut owner, _dir) = probe.spawn(
        name,
        "keeper = setmetatable({}, {__gc = function() ext.boom_text() end})\n\
         error('original Lua failure')\n",
        0,
        None,
    );
    let error = owner.wait_ready(BOUND).expect_err("the main chunk failed");
    assert!(
        script_error(&error, name)
            .0
            .contains("original Lua failure"),
        "{error:?}"
    );
    assert!(owner.wait_finished(BOUND));
    assert_eq!(records(name), [TEARDOWN_RECORD]);

    // A Lua error in `shutdown()`: readiness succeeds, the stop fails with the Lua error, not with the
    // teardown panic, and the teardown panic is logged.
    let name = "teardown_after_shutdown_error";
    let (mut owner, _dir) = probe.spawn(
        name,
        "keeper = setmetatable({}, {__gc = function() ext.boom_text() end})\n\
         function shutdown() error('shutdown Lua failure') end\n",
        0,
        None,
    );
    owner
        .wait_ready(BOUND)
        .expect("ready: nothing has failed yet");
    owner.start(BOUND).expect("started");
    let error = owner
        .stop(BOUND)
        .expect_err("shutdown() raised a Lua error");
    assert!(
        script_error(&error, name)
            .0
            .contains("shutdown Lua failure"),
        "{error:?}"
    );
    assert!(owner.wait_finished(BOUND));
    assert_eq!(records(name), [TEARDOWN_RECORD]);

    // An earlier host panic.
    let name = "teardown_after_panic";
    let (mut owner, _dir) = probe.spawn(
        name,
        "keeper = setmetatable({}, {__gc = function() ext.boom_text() end})\next.boom()\n",
        0,
        None,
    );
    let error = owner.wait_ready(BOUND).expect_err("panicked");
    assert!(
        matches!(&error, VmError::Panicked { message, .. } if message == "boom msg"),
        "{error:?}"
    );
    assert!(owner.wait_finished(BOUND));
    assert_eq!(records(name), [TEARDOWN_RECORD]);
}

#[test]
fn a_teardown_panic_of_a_cancelled_owner_has_no_caller_to_tell_so_it_is_logged() {
    let probe = Probe::new();
    let name = "teardown_cancelled";
    let (mut owner, _dir) = probe.spawn(
        name,
        "keeper = setmetatable({}, {__gc = function() ext.boom() end})\n",
        0,
        None,
    );
    owner.wait_ready(BOUND).expect("ready");
    owner.cancel();
    assert!(owner.wait_finished(BOUND));
    assert_eq!(
        records(name),
        ["ERROR host function panicked during teardown: boom msg"]
    );
}

#[path = "tests_failure.rs"]
mod failure;

#[path = "tests_tables.rs"]
mod tables;

#[path = "tests_payload.rs"]
mod payload;

#[path = "tests_completion.rs"]
mod completion;

#[path = "tests_observer.rs"]
mod observer;
