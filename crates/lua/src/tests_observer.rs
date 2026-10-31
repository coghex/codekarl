//! CK-125: `Extension::callback_observed` (the observer runs on the owner thread after the setter changed the callback
//! table, with `true` for a registration and `false` for a clear) and `Extension::unavailable_callback` (the same argument
//! validation as any setter, then a given refusal, no mutation and no observer).
//!
//! The negative controls named in the tests are the observed setter that keeps a callback registered whatever the
//! observer does, and the plain setter next to the unavailable one.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use super::{
    BOUND, Dir, host, lock, next_result, returned, spawn_with_settings, text, wait_thread_ended,
};
use crate::{CallOutcome, ConfigValue, Extension, VmError, VmOwner};

/// What the observer of the `frame` table saw.
#[derive(Default)]
struct Seen {
    /// Every call, in order.
    calls: Mutex<Vec<bool>>,
    /// The threads the observer ran on.
    threads: Mutex<Vec<ThreadId>>,
    /// The last value, as a script reads it back through `probe.state()`.
    state: AtomicBool,
    /// The calls to `probe.read()` and the thread they ran on.
    host_threads: Mutex<Vec<ThreadId>>,
}

/// Extensions: `frame` with the observed setter `on_update` (callback `frame_update`) and `probe` with `state()`, which
/// reads what the observer stored.
fn observed(seen: &Arc<Seen>) -> Vec<Extension> {
    let observer = {
        let seen = Arc::clone(seen);
        Arc::new(move |registered: bool| {
            lock(&seen.calls).push(registered);
            lock(&seen.threads).push(thread::current().id());
            seen.state.store(registered, Ordering::SeqCst);
        })
    };
    let state = {
        let seen = Arc::clone(seen);
        host(move |_| {
            lock(&seen.host_threads).push(thread::current().id());
            Ok(vec![ConfigValue::Bool(seen.state.load(Ordering::SeqCst))])
        })
    };
    vec![
        Extension::new("frame").callback_observed("on_update", "frame_update", observer),
        Extension::new("probe").function("state", state),
    ]
}

fn started(name: &str, source: &str, extensions: Vec<Extension>, limit: usize) -> (VmOwner, Dir) {
    let (mut owner, dir) = spawn_with_settings(name, source, move |settings| {
        settings.extensions = extensions;
        settings.call_limit = limit;
    });
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    (owner, dir)
}

#[test]
fn the_observer_hears_a_registration_and_a_clear_on_the_owner_thread_before_the_script_goes_on() {
    let seen = Arc::new(Seen::default());
    let (mut owner, _dir) = started(
        "obs_basic",
        "assert(probe.state() == false)\n\
         frame.on_update(function() return 'ran' end)\n\
         assert(probe.state() == true, 'the observer ran before the next statement')\n\
         frame.on_update(function() return 'again' end)\n\
         frame.on_update(nil)\n\
         assert(probe.state() == false)\n\
         frame.on_update(function() return 'last' end)\n\
         function startup() end\n",
        observed(&seen),
        2,
    );
    assert_eq!(*lock(&seen.calls), [true, true, false, true]);
    let threads = lock(&seen.threads).clone();
    assert!(threads.iter().all(|id| *id != thread::current().id()));
    assert!(
        threads
            .iter()
            .all(|id| lock(&seen.host_threads).contains(id)),
        "the observer ran on the thread of the host functions: the owner thread"
    );
    // The callback is the one the setter registered (the observer cannot undo or refuse the registration).
    owner.post("frame_update", Vec::new()).expect("posted");
    assert_eq!(returned(&next_result(&mut owner)), [text("last")]);
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn a_bad_argument_to_an_observed_setter_is_the_ordinary_error_and_the_observer_is_not_called() {
    let seen = Arc::new(Seen::default());
    let (mut owner, _dir) = started(
        "obs_bad",
        "frame.on_update(function() return 'kept' end)\n\
         for _, call in ipairs({\n\
           function() return frame.on_update(7) end,\n\
           function() return frame.on_update() end,\n\
           function() return frame.on_update(function() end, 2) end,\n\
           function() return frame.on_update('x') end,\n\
         }) do\n\
           local ok, err = pcall(call)\n\
           assert(ok == false and tostring(err):find(\"to 'frame.on_update'\", 1, true), tostring(err))\n\
         end\n\
         function startup() end\n",
        observed(&seen),
        2,
    );
    assert_eq!(
        *lock(&seen.calls),
        [true],
        "only the good call was observed"
    );
    owner.post("frame_update", Vec::new()).expect("posted");
    assert_eq!(returned(&next_result(&mut owner)), [text("kept")]);
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn a_panic_in_the_observer_is_owner_fatal_also_under_pcall() {
    let observer = Arc::new(|_: bool| panic!("observer boom"));
    let (mut owner, _dir) = spawn_with_settings(
        "obs_panic",
        "pcall(frame.on_update, function() end)\nfunction startup() end\n",
        |settings| {
            settings.extensions = vec![Extension::new("frame").callback_observed(
                "on_update",
                "frame_update",
                observer,
            )];
        },
    );
    // The main chunk runs before the owner is ready: the panic ends it there, whatever `pcall` was asked to catch.
    let error = owner.wait_ready(BOUND).expect_err("the owner ended");
    assert!(
        matches!(&error, VmError::Panicked { owner, message }
            if owner == "obs_panic" && message.contains("observer boom")),
        "{error:?}"
    );
    wait_thread_ended(&owner);
}

/// The `ui` side of the unavailable setter: `frame.on_update` refuses with a message, while a plain setter `alt.on_alt`
/// registers the callback `frame_update` (no unavailable setter registers one, so the name is free).
fn unavailable(observer_calls: &Arc<AtomicUsize>) -> Vec<Extension> {
    let observer = {
        let calls = Arc::clone(observer_calls);
        Arc::new(move |_: bool| {
            calls.fetch_add(1, Ordering::SeqCst);
        })
    };
    vec![
        Extension::new("frame")
            .unavailable_callback("on_update", "frame.on_update is not available here"),
        Extension::new("alt").callback_observed("on_alt", "frame_update", observer),
    ]
}

#[test]
fn an_unavailable_setter_validates_like_a_setter_then_refuses_and_changes_nothing() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (mut owner, _dir) = started(
        "unavail_refuses",
        "alt.on_alt(function() return 'old' end)\n\
         local before = 1\n\
         -- a valid function and nil: the exact message, and the registered callback stays\n\
         local ok, err = pcall(frame.on_update, function() return 'new' end)\n\
         assert(ok == false and tostring(err):find('frame.on_update is not available here', 1, true), tostring(err))\n\
         ok, err = pcall(frame.on_update, nil)\n\
         assert(ok == false and tostring(err):find('frame.on_update is not available here', 1, true), tostring(err))\n\
         -- argument errors are the ordinary ones\n\
         for _, call in ipairs({\n\
           function() return frame.on_update(3) end,\n\
           function() return frame.on_update() end,\n\
           function() return frame.on_update(function() end, 1) end,\n\
         }) do\n\
           local ok2, err2 = pcall(call)\n\
           assert(ok2 == false and tostring(err2):find(\"to 'frame.on_update'\", 1, true), tostring(err2))\n\
           assert(not tostring(err2):find('not available', 1, true), tostring(err2))\n\
         end\n\
         function startup() end\n",
        unavailable(&calls),
        2,
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "only alt.on_alt's own registration reached an observer"
    );
    owner.post("frame_update", Vec::new()).expect("posted");
    assert_eq!(
        returned(&next_result(&mut owner)),
        [text("old")],
        "the callback registered before is untouched"
    );
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn an_unavailable_setter_registers_nothing_where_nothing_was_registered() {
    let calls = Arc::new(AtomicUsize::new(0));
    let (mut owner, _dir) = started(
        "unavail_none",
        "assert(pcall(frame.on_update, function() return 'x' end) == false)\nfunction startup() end\n",
        unavailable(&calls),
        2,
    );
    owner.post("frame_update", Vec::new()).expect("posted");
    let result = next_result(&mut owner);
    assert!(
        matches!(result.outcome, CallOutcome::NotRegistered),
        "no callback appeared: {result:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn an_unavailable_setter_takes_part_in_the_tables_name_rules_but_claims_no_callback_name() {
    let (_dir, entry) = {
        let dir = Dir::new();
        let entry = dir.script("function startup() end\n");
        (dir, entry)
    };
    let function = || -> crate::HostFunction { host(|_| Ok(Vec::new())) };
    let cases = [
        (
            "unavail_twice",
            Extension::new("ok")
                .unavailable_callback("s", "no")
                .unavailable_callback("s", "no"),
        ),
        (
            "unavail_vs_function",
            Extension::new("ok")
                .function("s", function())
                .unavailable_callback("s", "no"),
        ),
        (
            "unavail_vs_setter",
            Extension::new("ok")
                .callback("s", "x")
                .unavailable_callback("s", "no"),
        ),
        (
            "unavail_bad_name",
            Extension::new("ok").unavailable_callback("not-ident", "no"),
        ),
    ];
    for (name, extension) in cases {
        let mut settings =
            crate::VmSettings::new(name, entry.clone(), super::super::empty_config());
        settings.extensions = vec![extension];
        let error = VmOwner::spawn(settings).expect_err(name);
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::InvalidInput,
            "{name}: {error}"
        );
    }
    // Two unavailable setters (and a real one) in different tables do not collide on a callback name.
    let mut settings = crate::VmSettings::new("unavail_fine", entry, super::super::empty_config());
    settings.extensions = vec![
        Extension::new("one")
            .unavailable_callback("a", "no")
            .unavailable_callback("b", "no"),
        Extension::new("two").callback("a", "frame_update"),
    ];
    let mut owner = VmOwner::spawn(settings).expect("valid");
    owner.wait_ready(BOUND).expect("ready");
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn an_observed_setter_keeps_its_callback_whatever_the_observer_does_with_the_flag_negative_control()
{
    // The observer cannot refuse: it only watches. A "refusing" observer (one that records false for a registration) leaves
    // the callback registered.
    let refused = Arc::new(AtomicUsize::new(0));
    let observer = {
        let refused = Arc::clone(&refused);
        Arc::new(move |_registered: bool| {
            refused.fetch_add(1, Ordering::SeqCst);
        })
    };
    let (mut owner, _dir) = started(
        "obs_refusing",
        "frame.on_update(function() return 'still here' end)\nfunction startup() end\n",
        vec![Extension::new("frame").callback_observed("on_update", "frame_update", observer)],
        2,
    );
    assert_eq!(refused.load(Ordering::SeqCst), 1);
    owner.post("frame_update", Vec::new()).expect("posted");
    assert_eq!(returned(&next_result(&mut owner)), [text("still here")]);
    owner.stop(BOUND).expect("stopped");
}
