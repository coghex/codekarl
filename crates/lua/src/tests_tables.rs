//! CK-85: several extension tables on one VM.
//!
//! The second table here is `aux`, built beside the `ext` kit of the parent module: host functions
//! `mark(name)` (recorded as `aux:<name>`), `count()` (the probe's counter), `double(n)`, `open()`,
//! `fail()` and `boom()` (a panic with its own text), and setters `on_echo` and `on_forever` for the
//! callbacks `aux_echo` and `aux_forever`. `ext` keeps its own `on_echo`: setter names belong to their
//! table, callback names to the VM.

use std::sync::atomic::Ordering;

use super::{
    BOUND, CallOutcome, Dir, Probe, VmError, VmState, host, lock, next_result, returned, text,
};
use super::{ConfigValue, Extension, VmOwner, VmSettings};
use crate::hooks;

impl Probe {
    /// The extension `aux`, described in the module documentation.
    fn aux(&self) -> Extension {
        let marks = std::sync::Arc::clone(&self.marks);
        let calls = std::sync::Arc::clone(&self.calls);
        let entered = std::sync::Arc::clone(&self.entered_tx);
        Extension::new("aux")
            .function(
                "mark",
                host(move |args| {
                    if let Some(Some(ConfigValue::String(name))) = args.first() {
                        lock(&marks).push(format!("aux:{name}"));
                    }
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
                "double",
                host(|args| match args {
                    [Some(ConfigValue::Integer(n))] => Ok(vec![ConfigValue::Integer(n * 2)]),
                    other => Err(format!("double wants one integer, got {other:?}")),
                }),
            )
            .function(
                "open",
                host(|_| Ok(vec![ConfigValue::String("aux open".into())])),
            )
            .function("fail", host(|_| Err("aux said no".to_owned())))
            .function("boom", host(|_| panic!("aux boom msg")))
            .function(
                "entered",
                host(move |_| {
                    let _ = lock(&entered).send(());
                    Ok(Vec::new())
                }),
            )
            .callback("on_echo", "aux_echo")
            .callback("on_forever", "aux_forever")
    }

    /// An owner with the `ext` kit and `aux`, in that order, and `limit` calls in flight, brought to Started.
    fn started_with_two_tables(&self, name: &str, source: &str, limit: usize) -> (VmOwner, Dir) {
        let (mut owner, dir) = super::spawn_with_settings(name, source, |settings| {
            settings.extensions = vec![self.kit(), self.aux()];
            settings.call_limit = limit;
        });
        owner.wait_ready(BOUND).expect("ready");
        owner.start(BOUND).expect("started");
        (owner, dir)
    }
}

#[test]
fn functions_of_both_tables_are_callable_and_the_setters_of_both_register_callbacks_that_post_reaches()
 {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started_with_two_tables(
        "two_tables",
        "assert(ext.sum(1, 2) == 3 and aux.double(21) == 42)\n\
         assert(type(ext) == 'table' and type(aux) == 'table' and ext ~= aux)\n\
         ext.on_echo(function() return 'from ext' end)\n\
         aux.on_echo(function() return 'from aux' end)\n\
         aux.on_forever(function() return 'aux forever' end)\n\
         function startup() end\n",
        4,
    );
    owner.post("echo", Vec::new()).expect("posted");
    owner.post("aux_echo", Vec::new()).expect("posted");
    owner.post("aux_forever", Vec::new()).expect("posted");
    let results: Vec<_> = (0..3).map(|_| next_result(&mut owner)).collect();
    assert_eq!(returned(&results[0]), [text("from ext")]);
    assert_eq!(returned(&results[1]), [text("from aux")]);
    assert_eq!(returned(&results[2]), [text("aux forever")]);
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn a_host_error_from_one_table_is_catchable_and_leaves_the_other_table_and_later_calls_working() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started_with_two_tables(
        "two_tables_error",
        "local ok, err = pcall(aux.fail)\n\
         assert(ok == false and tostring(err):find('aux said no', 1, true), tostring(err))\n\
         local ok2, err2 = pcall(ext.fail)\n\
         assert(ok2 == false and tostring(err2):find('the host said no', 1, true), tostring(err2))\n\
         assert(ext.sum(2, 3) == 5 and aux.double(4) == 8)\n\
         aux.on_echo(function() return aux.double(5) end)\n\
         ext.on_echo(function() return ext.sum(5, 5) end)\n\
         function startup() end\n",
        4,
    );
    owner.post("aux_echo", Vec::new()).expect("posted");
    owner.post("echo", Vec::new()).expect("posted");
    assert_eq!(returned(&next_result(&mut owner)), [super::int(10)]);
    assert_eq!(returned(&next_result(&mut owner)), [super::int(10)]);
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn replacing_or_clearing_a_callback_of_one_table_leaves_the_other_tables_callbacks_registered() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started_with_two_tables(
        "two_tables_clear",
        "ext.on_echo(function() return 'ext first' end)\n\
         aux.on_echo(function() return 'aux first' end)\n\
         aux.on_forever(function() return 'aux forever' end)\n\
         -- replace one table's callback, clear another of the same table\n\
         ext.on_echo(function() return 'ext second' end)\n\
         aux.on_forever(nil)\n\
         function startup() end\n",
        6,
    );
    owner.post("echo", Vec::new()).expect("posted");
    owner.post("aux_echo", Vec::new()).expect("posted");
    owner.post("aux_forever", Vec::new()).expect("posted");
    assert_eq!(returned(&next_result(&mut owner)), [text("ext second")]);
    assert_eq!(returned(&next_result(&mut owner)), [text("aux first")]);
    assert_eq!(
        next_result(&mut owner).outcome,
        CallOutcome::NotRegistered,
        "nil cleared the other table's own callback only"
    );
    owner.stop(BOUND).expect("stopped");

    // The same setter name in two tables: clearing one leaves the other.
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started_with_two_tables(
        "two_tables_clear_same_setter",
        "ext.on_echo(function() return 'ext' end)\n\
         aux.on_echo(function() return 'aux' end)\n\
         ext.on_echo(nil)\n\
         function startup() end\n",
        4,
    );
    owner.post("echo", Vec::new()).expect("posted");
    owner.post("aux_echo", Vec::new()).expect("posted");
    assert_eq!(next_result(&mut owner).outcome, CallOutcome::NotRegistered);
    assert_eq!(returned(&next_result(&mut owner)), [text("aux")]);
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn function_and_setter_names_are_scoped_to_their_table_and_callback_names_to_the_vm() {
    let dir = Dir::new();
    let entry = dir.script(
        "assert(first.open() == 'first' and second.open() == 'second')\n\
         first.on_close(function() return 'first close' end)\n\
         second.on_close(function() return 'second close' end)\n\
         function startup() end\n",
    );
    let function =
        |value: &'static str| host(move |_| Ok(vec![ConfigValue::String(value.to_owned())]));
    let mut settings = VmSettings::new("scoped", entry, super::super::empty_config());
    settings.call_limit = 2;
    settings.extensions = vec![
        Extension::new("first")
            .function("open", function("first"))
            .callback("on_close", "close_first"),
        Extension::new("second")
            .function("open", function("second"))
            .callback("on_close", "close_second"),
    ];
    let mut owner = VmOwner::spawn(settings).expect("a valid list");
    owner.wait_ready(BOUND).expect("ready");
    owner.start(BOUND).expect("started");
    owner.post("close_first", Vec::new()).expect("posted");
    owner.post("close_second", Vec::new()).expect("posted");
    assert_eq!(returned(&next_result(&mut owner)), [text("first close")]);
    assert_eq!(returned(&next_result(&mut owner)), [text("second close")]);
    owner.stop(BOUND).expect("stopped");
}

#[test]
fn an_invalid_list_is_refused_before_any_thread_starts_naming_the_table_and_the_name() {
    let dir = Dir::new();
    let entry = dir.script("");
    let function = || host(|_| Ok(Vec::new()));
    let ok = |table: &str| Extension::new(table).function("f", function());
    let cases: Vec<(&str, Vec<Extension>, &[&str])> = vec![
        (
            "list_duplicate_table",
            vec![ok("one"), ok("two"), ok("one")],
            &["`one`", "twice"],
        ),
        (
            "list_duplicate_callback",
            vec![
                Extension::new("one").callback("a", "shared"),
                Extension::new("two").callback("b", "shared"),
            ],
            &["`shared`", "`one`", "`two`"],
        ),
        ("list_bad_first", vec![ok("Bad"), ok("two")], &["`Bad`"]),
        (
            "list_bad_second",
            vec![ok("one"), ok("has-dash")],
            &["`has-dash`"],
        ),
        (
            "list_bad_third",
            vec![ok("one"), ok("two"), ok("")],
            &["``"],
        ),
        (
            "list_reserved_log",
            vec![ok("one"), ok("log")],
            &["`log`", "reserved"],
        ),
        (
            "list_reserved_config",
            vec![ok("config"), ok("one")],
            &["`config`", "reserved"],
        ),
        (
            "list_reserved_string",
            vec![ok("one"), ok("two"), ok("string")],
            &["`string`"],
        ),
        (
            "list_reserved_startup",
            vec![ok("one"), ok("startup")],
            &["`startup`"],
        ),
        (
            "list_bad_name_in_second",
            vec![
                ok("one"),
                Extension::new("two").function("not-ident", function()),
            ],
            &["`not-ident`", "`two`"],
        ),
        (
            "list_callback_twice_in_second",
            vec![
                ok("one"),
                Extension::new("two").callback("s", "x").callback("t", "x"),
            ],
            &["`x`", "`two`"],
        ),
    ];
    for (name, extensions, expected) in cases {
        let mut settings = VmSettings::new(name, entry.clone(), super::super::empty_config());
        settings.extensions = extensions;
        let error = VmOwner::spawn(settings).expect_err(name);
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::InvalidInput,
            "{name}: {error}"
        );
        for part in expected {
            assert!(
                error.to_string().contains(part),
                "{name}: {error} lacks {part}"
            );
        }
        assert!(!hooks::was_spawned(name), "{name}: no thread was created");
    }
}

#[test]
fn a_later_table_cannot_replace_an_earlier_one_or_a_reserved_global_the_list_is_refused() {
    // By validation, not by installation order: whichever way round, the same list is refused, so no
    // script ever sees a table that a later one replaced.
    let dir = Dir::new();
    let entry = dir.script("");
    let table = |name: &str, value: &'static str| {
        Extension::new(name).function(
            "who",
            host(move |_| Ok(vec![ConfigValue::String(value.into())])),
        )
    };
    for (name, extensions) in [
        (
            "later_same",
            vec![table("same", "first"), table("same", "second")],
        ),
        (
            "later_same_rev",
            vec![table("same", "second"), table("same", "first")],
        ),
        ("later_log", vec![table("one", "x"), table("log", "x")]),
        (
            "later_config",
            vec![table("one", "x"), table("config", "x")],
        ),
        ("later_pcall", vec![table("one", "x"), table("pcall", "x")]),
        (
            "later_shutdown",
            vec![table("one", "x"), table("shutdown", "x")],
        ),
    ] {
        let mut settings = VmSettings::new(name, entry.clone(), super::super::empty_config());
        settings.extensions = extensions;
        assert_eq!(
            VmOwner::spawn(settings).expect_err(name).kind(),
            std::io::ErrorKind::InvalidInput,
            "{name}"
        );
        assert!(!hooks::was_spawned(name), "{name}");
    }
}

#[test]
fn an_empty_list_leaves_no_extension_global_and_one_element_behaves_as_a_single_table() {
    let probe = Probe::new();
    // No extension: neither global exists.
    let (mut none, _dir) = super::spawn_with_settings(
        "no_tables",
        "assert(ext == nil and aux == nil and window == nil and render == nil)\n\
         assert(type(log) == 'table' and type(config) == 'userdata')\n",
        |settings| assert!(settings.extensions.is_empty()),
    );
    none.wait_ready(BOUND).expect("ready");
    // One element: exactly the single-table VM, `aux` still absent.
    let (mut one, _dir) = super::spawn_with_settings(
        "one_table",
        "assert(ext.sum(1, 1) == 2 and aux == nil)\n",
        |settings| settings.extensions = vec![probe.kit()],
    );
    one.wait_ready(BOUND).expect("ready");
}

#[test]
fn an_installation_failure_in_any_table_is_an_initialisation_failure_and_the_script_never_runs() {
    use mlua::Lua;
    let probe = Probe::new();
    let name = "install_second_fails";
    // The state's globals refuse a new global called `aux`: installing the second table fails.
    hooks::register(
        name,
        super::Hooks {
            at: Box::new(|point, lua| {
                if let (super::Point::StateCreated, Some(lua)) = (point, lua) {
                    let guard = |lua: &Lua| -> mlua::Result<()> {
                        let refuse = lua.create_function(
                            |_, (table, key, value): (mlua::Table, String, mlua::Value)| {
                                if key == "aux" {
                                    return Err(mlua::Error::runtime("aux is refused"));
                                }
                                table.raw_set(key, value)
                            },
                        )?;
                        let meta = lua.create_table()?;
                        meta.set("__newindex", refuse)?;
                        lua.globals().set_metatable(Some(meta))?;
                        Ok(())
                    };
                    guard(lua).expect("a guard on the globals");
                }
            }),
            install: None,
        },
    );
    let (mut owner, _dir) =
        super::spawn_with_settings(name, "ext.mark('the entry script ran')\n", |settings| {
            settings.extensions = vec![probe.kit(), probe.aux()];
        });
    let error = owner
        .wait_ready(BOUND)
        .expect_err("the installation failed");
    assert!(
        super::script_error(&error, name)
            .0
            .contains("aux is refused"),
        "{error:?}"
    );
    assert!(probe.marks().is_empty(), "the entry script never ran");
    assert!(owner.wait_finished(BOUND));
}

#[test]
fn cancel_interrupts_a_looping_callback_of_the_second_table_and_a_queued_call_of_the_first() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started_with_two_tables(
        "two_tables_cancel",
        "aux.on_forever(function() aux.entered() while true do end end)\n\
         ext.on_echo(function() ext.count() end)\n",
        4,
    );
    owner.post("aux_forever", Vec::new()).expect("posted");
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
    assert_eq!(probe.calls(), 0, "the queued call never ran");
}

#[test]
fn the_instruction_hook_ends_a_protected_loop_that_calls_both_tables() {
    let probe = Probe::new();
    let (mut owner, _dir) = super::spawn_with_settings(
        "two_tables_hook",
        "aux.entered()\n\
         while true do\n\
           pcall(function() ext.count() aux.count() error('loop') end)\n\
         end\n",
        |settings| settings.extensions = vec![probe.kit(), probe.aux()],
    );
    probe
        .entered
        .recv_timeout(BOUND)
        .expect("the loop is running");
    owner.cancel();
    assert!(
        owner.wait_finished(BOUND),
        "a loop under pcall is ended by the hook"
    );
    assert!(probe.calls() > 0, "both tables were called from the loop");
}

#[test]
fn a_panic_in_the_second_table_is_owner_fatal_also_under_pcall_and_xpcall() {
    for (name, script) in [
        ("two_panic_direct", "aux.boom() ext.mark('after')"),
        ("two_panic_pcall", "pcall(aux.boom) ext.mark('after')"),
        (
            "two_panic_xpcall",
            "xpcall(aux.boom, function(e) ext.mark('handler') return e end) ext.mark('after')",
        ),
    ] {
        let probe = Probe::new();
        let (mut owner, _dir) = super::spawn_with_settings(name, script, |settings| {
            settings.extensions = vec![probe.kit(), probe.aux()];
        });
        let error = owner.wait_ready(BOUND).expect_err(name);
        assert!(
            matches!(&error, VmError::Panicked { owner: named, message }
                if named == name && message.contains("aux boom msg")),
            "{name}: {error:?}"
        );
        assert_eq!(owner.state(), VmState::Ending);
        assert!(owner.wait_finished(BOUND), "{name}");
        assert!(
            probe.marks().is_empty(),
            "{name}: no further Lua ran: {:?}",
            probe.marks()
        );
    }
}

#[test]
fn a_posted_call_that_panics_in_the_second_table_is_owner_fatal_too() {
    let probe = Probe::new();
    let (mut owner, _dir) = probe.started_with_two_tables(
        "two_tables_post_panic",
        "aux.on_echo(function() pcall(aux.boom) ext.mark('after') end)\n",
        2,
    );
    owner.post("aux_echo", Vec::new()).expect("posted");
    owner.request_stop().expect("sent");
    let error = owner.await_stopped(BOUND).expect_err("the panic");
    assert!(
        matches!(&error, VmError::Panicked { message, .. } if message.contains("aux boom msg"))
    );
    assert!(owner.wait_finished(BOUND));
    assert!(owner.take_result().is_none());
    assert!(probe.marks().is_empty());
}
