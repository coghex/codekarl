//! CK-109: the completion hook (`VmSettings::on_complete`) runs on the owner thread after `startup()` and after
//! every posted call that has a result, immediately before its report and wake, and nowhere else.
//!
//! The hook and the report wake append to one shared log, so each test reads the order they ran in. A hook that
//! panics ends the owner (the existing `catch_unwind` in `thread_main` covers it: the hook is called from
//! `serve`, inside `run`). The negative controls are in the result, run with
//! `ck109-evidence/scripts/mutation_probe.py`.

use std::panic::panic_any;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use super::{
    BOUND, Dir, host, lock, next_result, returned, spawn_with_settings, wait_thread_ended,
};
use crate::runner::Wake;
use crate::unwind::tests::Bomb;
use crate::{
    CallId, CallOutcome, Completion, ConfigValue, Extension, VmError, VmOwner, VmSettings, VmState,
};

/// A posted callback, the value `ext.set` left when its hook ran, and the test of how the call ended.
type Case = (&'static str, i64, fn(&CallOutcome) -> bool);

/// One thing the owner thread did, in the order it did it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    /// The hook ran, and `ext.value()` was this at that moment.
    Complete(Completion, i64),
    /// The report wake ran.
    Wake,
}

type Hook = Arc<dyn Fn(Completion) + Send + Sync>;

/// The host functions the scripts call, and the log the hook and the wake write.
#[derive(Clone)]
struct Trace {
    events: Arc<Mutex<Vec<Event>>>,
    /// What `ext.set(n)` last stored: the "side effect" of a callback.
    value: Arc<AtomicI64>,
    /// The thread of each hook run and each wake.
    hook_threads: Arc<Mutex<Vec<ThreadId>>>,
    wake_threads: Arc<Mutex<Vec<ThreadId>>>,
    /// Set by `ext.entered()`.
    entered: Arc<AtomicBool>,
    /// `ext.after()` calls.
    after: Arc<AtomicUsize>,
    /// While set, a report wake waits (at most `BOUND`) before it records itself: the report is then visible to
    /// the caller with its wake not yet in the log.
    hold_wake: Arc<AtomicBool>,
    /// Lets one `ext.block()` return.
    release: Sender<()>,
    gate: Arc<Mutex<Receiver<()>>>,
}

impl Trace {
    fn new() -> Self {
        let (release, gate) = mpsc::channel();
        Self {
            release,
            gate: Arc::new(Mutex::new(gate)),
            events: Arc::new(Mutex::new(Vec::new())),
            value: Arc::new(AtomicI64::new(0)),
            hook_threads: Arc::new(Mutex::new(Vec::new())),
            wake_threads: Arc::new(Mutex::new(Vec::new())),
            entered: Arc::new(AtomicBool::new(false)),
            after: Arc::new(AtomicUsize::new(0)),
            hold_wake: Arc::new(AtomicBool::new(false)),
        }
    }

    fn events(&self) -> Vec<Event> {
        lock(&self.events).clone()
    }

    /// The log once it holds at least `wanted` events, waiting at most `BOUND` for it. A result is visible to
    /// the caller as soon as its report is sent, which is before the report's wake runs, so the log is read only
    /// after the observation the assertion needs. (The wake records its thread before its event.)
    fn wait_for_events(&self, wanted: usize) -> Vec<Event> {
        let end = Instant::now() + BOUND;
        loop {
            let events = self.events();
            if events.len() >= wanted {
                return events;
            }
            assert!(
                Instant::now() < end,
                "only {} of {wanted} events were logged: {events:?}",
                events.len()
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// The completions the hook saw, in order.
    fn completions(&self) -> Vec<Completion> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Complete(completion, _) => Some(completion),
                Event::Wake => None,
            })
            .collect()
    }

    /// The extension `ext`: `set(n)`, `entered()`, `after()` and `boom()`, and a setter `on_<name>` for each of
    /// the callbacks the scripts register.
    fn extension(&self) -> Extension {
        let (value, entered, after, gate) = (
            Arc::clone(&self.value),
            Arc::clone(&self.entered),
            Arc::clone(&self.after),
            Arc::clone(&self.gate),
        );
        let mut extension = Extension::new("ext")
            .function(
                "set",
                host(move |args| {
                    let Some(Some(ConfigValue::Integer(n))) = args.first() else {
                        return Err("set wants an integer".to_owned());
                    };
                    value.store(*n, Ordering::SeqCst);
                    Ok(Vec::new())
                }),
            )
            .function(
                "entered",
                host(move |_| {
                    entered.store(true, Ordering::SeqCst);
                    Ok(Vec::new())
                }),
            )
            .function(
                "after",
                host(move |_| {
                    after.fetch_add(1, Ordering::SeqCst);
                    Ok(Vec::new())
                }),
            )
            .function(
                "block",
                host(move |_| {
                    let _ = lock(&gate).recv_timeout(BOUND);
                    Ok(Vec::new())
                }),
            )
            .function("boom", host(|_| panic!("boom msg")));
        for name in ["ok", "fails", "bad", "forever", "boom", "slow", "gc"] {
            extension = extension.callback(&format!("on_{name}"), name);
        }
        extension
    }

    /// A hook that records the completion, the value at that moment and its thread.
    fn hook(&self) -> Hook {
        let (events, value, threads) = (
            Arc::clone(&self.events),
            Arc::clone(&self.value),
            Arc::clone(&self.hook_threads),
        );
        Arc::new(move |completion| {
            lock(&threads).push(thread::current().id());
            let seen = value.load(Ordering::SeqCst);
            lock(&events).push(Event::Complete(completion, seen));
        })
    }

    /// A report wake that records itself in the same log.
    fn wake(&self) -> Wake {
        let (events, threads, hold) = (
            Arc::clone(&self.events),
            Arc::clone(&self.wake_threads),
            Arc::clone(&self.hold_wake),
        );
        Arc::new(move || {
            let end = Instant::now() + BOUND;
            while hold.load(Ordering::SeqCst) && Instant::now() < end {
                thread::sleep(Duration::from_millis(1));
            }
            lock(&threads).push(thread::current().id());
            lock(&events).push(Event::Wake);
        })
    }

    /// Spawns an owner on `source` with the kit, room for four calls, the report wake and `hook`.
    fn spawn(&self, name: &str, source: &str, hook: Option<Hook>) -> (VmOwner, Dir) {
        let extension = self.extension();
        let wake = self.wake();
        spawn_with_settings(name, source, move |settings: &mut VmSettings| {
            settings.extensions = vec![extension];
            settings.call_limit = 4;
            settings.on_report = Some(wake);
            settings.on_complete = hook;
        })
    }

    /// An owner brought to Started.
    fn started(&self, name: &str, source: &str, hook: Option<Hook>) -> (VmOwner, Dir) {
        let (mut owner, dir) = self.spawn(name, source, hook);
        owner.wait_ready(BOUND).expect("ready");
        owner.start(BOUND).expect("started");
        (owner, dir)
    }

    fn wait_entered(&self) {
        let end = Instant::now() + BOUND;
        while !self.entered.load(Ordering::SeqCst) {
            assert!(Instant::now() < end, "the callback never ran");
            thread::sleep(Duration::from_millis(1));
        }
    }
}

const SCRIPT: &str = "\
ext.on_ok(function() ext.set(1) return 5 end)
ext.on_fails(function() ext.set(2) error('the callback raised') end)
ext.on_bad(function() ext.set(3) return function() end end)
ext.on_forever(function() ext.entered() while true do end end)
ext.on_boom(function() ext.set(4) ext.boom() ext.after() end)
ext.on_slow(function() ext.entered() ext.block() return 'late' end)
ext.on_gc(function()
  setmetatable({}, {__gc = function() ext.boom() end})
  collectgarbage()
  return 'returned normally'
end)
";

fn panicked(name: &str, message: &str) -> VmError {
    VmError::Panicked {
        owner: name.to_owned(),
        message: message.to_owned(),
    }
}

// ---------------------------------------------------------------------------------------------
// Where it runs, and in what order
// ---------------------------------------------------------------------------------------------

#[test]
fn the_hook_runs_before_the_report_wake_and_sees_what_the_callback_did() {
    let name = "cm_order";
    let trace = Trace::new();
    let (mut owner, _dir) = trace.started(name, SCRIPT, Some(trace.hook()));
    let id = owner.post("ok", Vec::new()).expect("posted");
    let result = next_result(&mut owner);
    assert_eq!(result.id, id);
    assert_eq!(returned(&result), [Some(ConfigValue::Integer(5))]);
    // ready (wake), start (hook, then wake), the call (hook sees ext.set(1), then wake). The hook's value is
    // read inside the hook: the callback's side effect had happened, and the wake had not yet run.
    assert_eq!(
        trace.wait_for_events(5),
        [
            Event::Wake,
            Event::Complete(Completion::Startup, 0),
            Event::Wake,
            Event::Complete(Completion::Call(id), 1),
            Event::Wake,
        ]
    );
}

#[test]
fn a_result_can_be_visible_before_its_wake_is_logged_so_the_log_is_read_after_waiting() {
    // The schedule behind the waits above, made to happen: the wake of the call's report is held, so the caller
    // takes the result while the log has the hook's event and not yet the wake's. Reading the log at that point
    // is incomplete evidence; the exact assertion is made after the bounded wait for the wake.
    let name = "cm_visible_first";
    let trace = Trace::new();
    let (mut owner, _dir) = trace.started(name, SCRIPT, Some(trace.hook()));
    trace.wait_for_events(3);
    trace.hold_wake.store(true, Ordering::SeqCst);
    let id = owner.post("ok", Vec::new()).expect("posted");
    let result = next_result(&mut owner);
    assert_eq!(result.id, id);
    assert_eq!(
        trace.events().last(),
        Some(&Event::Complete(Completion::Call(id), 1)),
        "the result is in hand and its wake is not logged yet"
    );
    trace.hold_wake.store(false, Ordering::SeqCst);
    assert_eq!(
        trace.wait_for_events(5),
        [
            Event::Wake,
            Event::Complete(Completion::Startup, 0),
            Event::Wake,
            Event::Complete(Completion::Call(id), 1),
            Event::Wake,
        ]
    );
}

#[test]
fn the_hook_and_the_wake_run_on_the_owner_thread() {
    let name = "cm_thread";
    let trace = Trace::new();
    let (mut owner, _dir) = trace.started(name, SCRIPT, Some(trace.hook()));
    owner.post("ok", Vec::new()).expect("posted");
    next_result(&mut owner);
    trace.wait_for_events(5);
    let hooks = lock(&trace.hook_threads).clone();
    let wakes = lock(&trace.wake_threads).clone();
    assert_eq!(hooks.len(), 2);
    assert_ne!(hooks[0], thread::current().id(), "not the caller's thread");
    assert!(hooks.iter().all(|thread| *thread == hooks[0]));
    // The wake runs on the owner thread too: the hook shares its thread, so the hook is not a second thread.
    assert!(wakes.iter().all(|thread| *thread == hooks[0]));
}

#[test]
fn a_start_with_no_startup_defined_runs_the_hook_once() {
    let name = "cm_no_startup";
    let trace = Trace::new();
    let (owner, _dir) = trace.started(name, "ext.set(9)\n", Some(trace.hook()));
    // The main chunk set 9 before the start; the hook ran once, for the start.
    assert_eq!(
        trace.wait_for_events(3),
        [
            Event::Wake,
            Event::Complete(Completion::Startup, 9),
            Event::Wake
        ]
    );
    drop(owner);
}

#[test]
fn a_startup_that_returns_runs_the_hook_after_it_and_sees_its_changes() {
    let name = "cm_startup";
    let trace = Trace::new();
    let (owner, _dir) = trace.started(
        name,
        "function startup() ext.set(7) end\n",
        Some(trace.hook()),
    );
    assert_eq!(trace.completions(), [Completion::Startup]);
    assert_eq!(
        trace.wait_for_events(3)[1],
        Event::Complete(Completion::Startup, 7),
        "startup's change is visible to the hook"
    );
    drop(owner);
}

// ---------------------------------------------------------------------------------------------
// Every outcome of a posted call
// ---------------------------------------------------------------------------------------------

#[test]
fn every_outcome_runs_the_hook_exactly_once_with_its_own_id_before_its_result_and_after_its_changes()
 {
    let name = "cm_outcomes";
    let trace = Trace::new();
    let (mut owner, _dir) = trace.started(name, SCRIPT, Some(trace.hook()));
    let mut expected = vec![
        Event::Wake,
        Event::Complete(Completion::Startup, 0),
        Event::Wake,
    ];
    let mut ids: Vec<CallId> = Vec::new();
    // (callback, the value ext.set left when the hook ran, how the call ended)
    let cases: [Case; 4] = [
        ("ok", 1, |outcome| {
            matches!(outcome, CallOutcome::Returned(_))
        }),
        (
            "fails",
            2,
            |outcome| matches!(outcome, CallOutcome::Failed { message, .. } if message.contains("the callback raised")),
        ),
        (
            "bad",
            3,
            |outcome| matches!(outcome, CallOutcome::Failed { message, .. } if message.starts_with("return value #1")),
        ),
        // Not registered: no Lua ran, so the value is the previous call's.
        ("unregistered", 3, |outcome| {
            matches!(outcome, CallOutcome::NotRegistered)
        }),
    ];
    for (callback, seen, ended) in cases {
        let id = owner.post(callback, Vec::new()).expect("posted");
        let result = next_result(&mut owner);
        assert_eq!((result.id, result.callback.as_str()), (id, callback));
        assert!(ended(&result.outcome), "{callback}: {:?}", result.outcome);
        ids.push(id);
        // One hook run, with this call's id and the callback's own change (no rollback after the error),
        // and the wake after it.
        expected.push(Event::Complete(Completion::Call(id), seen));
        expected.push(Event::Wake);
        assert_eq!(
            trace.wait_for_events(expected.len()),
            expected,
            "{callback}"
        );
    }
    let distinct: std::collections::BTreeSet<_> = ids.iter().collect();
    assert_eq!(distinct.len(), 4, "each call has its own id");
    assert_eq!(
        owner.state(),
        VmState::Started,
        "a failed call does not end the VM"
    );
}

// ---------------------------------------------------------------------------------------------
// Where it does not run
// ---------------------------------------------------------------------------------------------

#[test]
fn the_main_chunk_and_shutdown_do_not_run_the_hook() {
    let name = "cm_main_stop";
    let trace = Trace::new();
    let (mut owner, _dir) = trace.spawn(
        name,
        "ext.set(5)\nfunction shutdown() ext.set(6) end\n",
        Some(trace.hook()),
    );
    owner.wait_ready(BOUND).expect("ready");
    assert!(
        trace.completions().is_empty(),
        "the main chunk ran: no hook"
    );
    owner.start(BOUND).expect("started");
    assert_eq!(trace.completions(), [Completion::Startup]);
    owner.stop(BOUND).expect("stopped");
    assert!(owner.wait_finished(BOUND));
    assert_eq!(trace.value.load(Ordering::SeqCst), 6, "shutdown ran");
    assert_eq!(
        trace.completions(),
        [Completion::Startup],
        "shutdown: no hook"
    );
}

#[test]
fn a_startup_that_fails_does_not_run_the_hook() {
    let name = "cm_bad_startup";
    let trace = Trace::new();
    let (mut owner, _dir) = trace.spawn(
        name,
        "function startup() ext.set(8) error('start broke') end\n",
        Some(trace.hook()),
    );
    owner.wait_ready(BOUND).expect("ready");
    let error = owner.start(BOUND).expect_err("startup failed");
    assert!(matches!(error, VmError::Script { .. }), "{error:?}");
    assert!(owner.wait_finished(BOUND));
    assert_eq!(trace.value.load(Ordering::SeqCst), 8, "startup ran");
    assert!(trace.completions().is_empty());
}

#[test]
fn a_call_interrupted_by_cancel_does_not_run_the_hook() {
    let name = "cm_cancel";
    let trace = Trace::new();
    let (mut owner, _dir) = trace.started(name, SCRIPT, Some(trace.hook()));
    owner.post("forever", Vec::new()).expect("posted");
    trace.wait_entered();
    owner.cancel();
    assert!(owner.wait_finished(BOUND));
    assert!(owner.take_result().is_none());
    // Only the start's completion: nothing for the interrupted call.
    assert_eq!(trace.completions(), [Completion::Startup]);
}

#[test]
fn a_call_ended_by_a_host_function_panic_does_not_run_the_hook() {
    let name = "cm_host_panic";
    let trace = Trace::new();
    let (mut owner, _dir) = trace.started(name, SCRIPT, Some(trace.hook()));
    owner.post("boom", Vec::new()).expect("posted");
    wait_thread_ended(&owner);
    assert_eq!(owner.take_failure(), Some(panicked(name, "boom msg")));
    assert!(owner.take_result().is_none());
    assert_eq!(
        trace.after.load(Ordering::SeqCst),
        0,
        "the script was interrupted"
    );
    assert_eq!(trace.completions(), [Completion::Startup]);
}

#[test]
fn a_call_that_returns_after_cancel_sets_the_flag_does_not_run_the_hook() {
    let name = "cm_cancel_late";
    let trace = Trace::new();
    let (mut owner, _dir) = trace.started(name, SCRIPT, Some(trace.hook()));
    owner.post("slow", Vec::new()).expect("posted");
    trace.wait_entered();
    owner.cancel();
    trace.release.send(()).expect("release the callback");
    assert!(owner.wait_finished(BOUND));
    assert!(owner.take_result().is_none(), "its result is discarded");
    assert_eq!(trace.completions(), [Completion::Startup]);
}

#[test]
fn a_host_panic_the_script_survives_does_not_run_the_hook() {
    // Lua turns an error in a `__gc` finalizer into a warning, so the callback returns normally; the recorded
    // panic still ends the owner, and the call's result is not sent, so there is nothing to complete.
    let name = "cm_gc_panic";
    let trace = Trace::new();
    let (mut owner, _dir) = trace.started(name, SCRIPT, Some(trace.hook()));
    owner.post("gc", Vec::new()).expect("posted");
    wait_thread_ended(&owner);
    assert_eq!(owner.take_failure(), Some(panicked(name, "boom msg")));
    assert!(owner.take_result().is_none());
    assert_eq!(trace.completions(), [Completion::Startup]);
}

#[test]
fn without_a_hook_a_vm_behaves_as_before() {
    let name = "cm_none";
    let trace = Trace::new();
    let (mut owner, _dir) = trace.started(name, SCRIPT, None);
    owner.post("ok", Vec::new()).expect("posted");
    assert_eq!(
        returned(&next_result(&mut owner)),
        [Some(ConfigValue::Integer(5))]
    );
    assert_eq!(
        trace.wait_for_events(3),
        [Event::Wake, Event::Wake, Event::Wake],
        "ready, start and the result wake once each"
    );
    let settings = VmSettings::new(
        name,
        "x.lua",
        ConfigValue::Table(std::collections::BTreeMap::default()),
    );
    assert!(settings.on_complete.is_none());
    assert!(format!("{settings:?}").contains("on_complete: \"absent\""));
    let mut with_hook = settings;
    with_hook.on_complete = Some(Arc::new(|_| {}));
    assert!(format!("{with_hook:?}").contains("on_complete: \"present\""));
}

// ---------------------------------------------------------------------------------------------
// A panic in the hook
// ---------------------------------------------------------------------------------------------

/// A hook that panics when it completes `at`: with the text `"hook boom"` or with a `Bomb`.
fn panicking(at: fn(Completion) -> bool, drops: Option<&Arc<AtomicUsize>>) -> Hook {
    let drops = drops.map(Arc::clone);
    Arc::new(move |completion| {
        if at(completion) {
            match &drops {
                Some(drops) => panic_any(Bomb {
                    drops: Arc::clone(drops),
                    then: None,
                }),
                None => panic!("hook boom"),
            }
        }
    })
}

#[test]
fn a_hook_that_panics_after_a_call_ends_the_owner_with_panicked_and_sends_no_result() {
    let name = "cm_panic_call";
    let trace = Trace::new();
    let hook = panicking(|completion| matches!(completion, Completion::Call(_)), None);
    let (mut owner, _dir) = trace.started(name, SCRIPT, Some(hook));
    owner.post("ok", Vec::new()).expect("posted");
    wait_thread_ended(&owner);
    assert!(owner.take_result().is_none(), "no result for that call");
    assert_eq!(owner.take_failure(), Some(panicked(name, "hook boom")));
    assert!(owner.wait_finished(BOUND));
    // The callback ran (its change stands); ready, start, the failure's report and the final wake each woke.
    assert_eq!(trace.value.load(Ordering::SeqCst), 1);
    assert_eq!(trace.wait_for_events(4), [Event::Wake; 4]);
}

#[test]
fn a_hook_that_panics_with_a_payload_whose_drop_panics_is_the_same_failure() {
    let name = "cm_panic_bomb";
    let trace = Trace::new();
    let drops = Arc::new(AtomicUsize::new(0));
    let hook = panicking(
        |completion| matches!(completion, Completion::Call(_)),
        Some(&drops),
    );
    let (mut owner, _dir) = trace.started(name, SCRIPT, Some(hook));
    owner.post("fails", Vec::new()).expect("posted");
    wait_thread_ended(&owner);
    assert!(owner.take_result().is_none());
    assert_eq!(
        owner.take_failure(),
        Some(panicked(name, "a panic with a non-string payload"))
    );
    assert!(owner.wait_finished(BOUND));
    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "the payload was disposed of once"
    );
}

#[test]
fn a_hook_that_panics_after_startup_fails_the_start_and_ends_the_owner() {
    let name = "cm_panic_start";
    let trace = Trace::new();
    let hook = panicking(|completion| completion == Completion::Startup, None);
    let (mut owner, _dir) = trace.spawn(name, SCRIPT, Some(hook));
    owner.wait_ready(BOUND).expect("ready");
    assert_eq!(owner.start(BOUND), Err(panicked(name, "hook boom")));
    assert_eq!(owner.state(), VmState::Ending);
    assert!(owner.wait_finished(BOUND));
}
