//! The failure boundary (design §7, §10), end to end: this test executable re-executes itself as a **child**
//! that runs the executable's own `execute_with` (the very code `main` runs) with a fake backend and a fake
//! presenter, through the real logger, the real panic hook and the real exit path. The parent checks only
//! what a process shows from outside: its exit status, its output and the marker files it left.
//!
//! Nothing here is in the production binary: the fakes live in this test build, and the child is started
//! by the parent with an environment variable that only this test reads.
//!
//! Every child is supervised by `tests/supervisor` (shared with the real-window tests): a whole-process
//! deadline, a bounded kill and reap on every path (also when the parent panics), readers whose errors fail
//! the case, and a failure to inspect fails closed. The supervisor's own failure paths are tested at the end
//! of this file.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

use ck_render_2d::Renderer;
use ck_window::Notifier;

use crate::cli::RunOptions;
use crate::fakes::{
    BackendScript, End, FakeBackend, FakeGpu, FakePresenter, PresenterScript, Recorder,
};
use crate::windowed::{self, Plan, RunClosure, Setup};
use crate::{Shared, execute_with, lifecycle, mode};

#[path = "../tests/supervisor/mod.rs"]
pub(crate) mod supervisor;
use supervisor::{Seen, Supervised};

/// The case the child runs; absent in the parent and in an ordinary `cargo test`.
const CASE: &str = "CK_BOUNDARY_CASE";
/// The directory with the child's configuration, scripts and marker files.
const DIR: &str = "CK_BOUNDARY_DIR";
/// The longest a child may run.
const DEADLINE: Duration = Duration::from_secs(60);

/// What a case does to the fakes and the scripts.
#[expect(
    clippy::struct_excessive_bools,
    reason = "a bag of independent case switches, each documented, is the point of this type"
)]
struct Case {
    backend: BackendScript,
    presenter: PresenterScript,
    /// The UI script's body after `window.open`.
    ui: String,
    /// The teardown deadline the loop gets.
    teardown: Duration,
    /// How the fake backend drives a held callback (the late-failure cases).
    held: Option<Held>,
    /// The fake backend's pump with this index sets the quit flag, as the signal thread does.
    quit_at_pump: Option<u32>,
    /// Which renderer wraps the presenter (CK-93): none, or a Metal-shaped backend. (The Vulkan path's own
    /// renderer is tested in `vulkan::tests`, over its fake stack.)
    render: Render,
    /// The log level of the case's configuration is `debug` (so the end-of-run counts are in the output).
    debug: bool,
    /// The UI VM's `shutdown()` also calls `render.quad` (the bridge has ended by then, so it is discarded).
    shutdown_quad: bool,
    /// The run is headless (`--headless`): no backend, no renderer.
    headless: bool,
    /// The UI script does not call `window.open` at all.
    no_open: bool,
    /// The fake backend sets the quit flag as the window is created, before the presenter starts.
    quit_in_make: bool,
}

/// How a case's presenter is wrapped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Render {
    /// Not at all: the presenter is `ck-window`'s, as in the other cases.
    Plain,
    /// A [`Renderer`] over a Metal-shaped fake: it draws what it is given.
    Shaped,
}

/// The late-failure cases: the UI's `on_opened` touches `started`, then waits for `gate`.
#[derive(Clone, Copy)]
struct Held {
    /// The gate file is created this long after the window was destroyed.
    release_after: Duration,
    /// An Escape press is delivered to the loop once the callback is held, and the callback waits for
    /// the pass that posted it (the `go` file) before it closes the window.
    escape: bool,
}

/// The Lua helpers of the fixtures: `touch(path)` leaves a marker file, `wait_for(path)` polls for one
/// (failing the script after 20 s of CPU time, so a bug ends the case instead of hanging it).
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
    if os.clock() - started > 20 then
      error('timed out waiting for ' .. path)
    end
  end
end
";

/// A Lua long string for `path`.
fn lua_path(path: &Path) -> String {
    format!("[==[{}]==]", path.display())
}

/// The UI body of the late-failure cases: `on_opened` is held until the gate file exists, then raises.
/// With `escape`, it first waits for the `go` file (created by the fake backend one pump after it queued an
/// Escape, so the resulting close request is posted behind this held callback), then closes the window.
fn held_body(dir: &Path, escape: bool) -> String {
    let wait_go = if escape {
        format!(
            "wait_for({})\n  window.close()\n  ",
            lua_path(&dir.join("go"))
        )
    } else {
        String::new()
    };
    format!(
        "{LUA_HELPERS}\nwindow.on_opened(function()\n  touch({})\n  {wait_go}wait_for({})\n  error('late callback failure')\nend)",
        lua_path(&dir.join("started")),
        lua_path(&dir.join("gate")),
    )
}

/// The UI body of the render cases: a quad from the assets directory, then a close.
const RENDER_UI: &str =
    "render.quad{image='small.png', x=2, y=3}\nwindow.on_opened(function() window.close() end)";

/// The case called `name`.
fn case(name: &str, dir: &Path) -> Case {
    let close = "window.on_opened(function() window.close() end)".to_owned();
    let base = Case {
        backend: BackendScript::default(),
        presenter: PresenterScript::default(),
        ui: close,
        teardown: Duration::from_millis(400),
        held: None,
        quit_at_pump: None,
        render: Render::Plain,
        debug: false,
        shutdown_quad: false,
        headless: false,
        no_open: false,
        quit_in_make: false,
    };
    match name {
        "proven_close" => base,
        "main_panic" => Case {
            backend: BackendScript {
                panic_in_pump: true,
                ..BackendScript::default()
            },
            ui: String::new(),
            ..base
        },
        "setup_proven" => Case {
            presenter: PresenterScript {
                start_error: Some("no Metal device".to_owned()),
                ..PresenterScript::default()
            },
            ui: String::new(),
            ..base
        },
        "setup_unproven" => Case {
            presenter: PresenterScript {
                first_frame: false,
                fail: Some("the GPU setup failed".to_owned()),
                end: End::Unproven("the render thread did not send a cleanup result".to_owned()),
                ..PresenterScript::default()
            },
            ui: String::new(),
            ..base
        },
        "thread_no_result" => Case {
            presenter: PresenterScript {
                end: End::Never,
                ..PresenterScript::default()
            },
            ..base
        },
        "thread_failed_result" => Case {
            presenter: PresenterScript {
                end: End::Unproven("the render thread reported a failed cleanup".to_owned()),
                ..PresenterScript::default()
            },
            ..base
        },
        "unproven_loop_end" => Case {
            presenter: PresenterScript {
                end: End::Unproven("an unproven GPU wait".to_owned()),
                ..PresenterScript::default()
            },
            ..base
        },
        "owner_panic" => Case {
            ui: "window.on_opened(function() window.boom() end)".to_owned(),
            ..base
        },
        "other_thread_panic" => Case {
            presenter: PresenterScript {
                panic_on_other_thread: true,
                ..PresenterScript::default()
            },
            ui: String::new(),
            ..base
        },
        // A presenter failure ends the run while `on_opened` is held; the callback then fails late, within
        // the drain (the gate opens 100 ms after the window is destroyed).
        "late_after_primary_in_drain" => late_after_primary(dir, Duration::from_millis(100), base),
        // ... and in the stop phase (the gate opens 1.5 s after, past the 1 s drain).
        "late_after_primary_in_stop" => late_after_primary(dir, Duration::from_millis(1500), base),
        // Escape queued behind the held `on_opened`: its close request waits behind the callback.
        "escape_behind_held_callback" => Case {
            ui: held_body(dir, true),
            held: Some(Held {
                release_after: Duration::from_millis(1500),
                escape: true,
            }),
            ..base
        },
        // Two callbacks fail late: the held `opened`, and the `close_request` queued behind it.
        "two_late_failures" => Case {
            ui: format!(
                "{}\nwindow.on_close_request(function(source) error('close callback failure') end)",
                held_body(dir, true)
            ),
            held: Some(Held {
                release_after: Duration::from_millis(1500),
                escape: true,
            }),
            ..base
        },
        other => late_presenter_case(other, base)
            .unwrap_or_else(|| panic!("no such boundary case: {other}")),
    }
}

/// The late presenter failure cases (CK-73): the presenter publishes failures while it stops.
fn late_presenter_case(name: &str, base: Case) -> Option<Case> {
    Some(match name {
        // The presenter publishes a failure while it stops, then proves its cleanup: after a UI close ...
        "late_failure_after_close" => late_failure(&["late GPU error"], base),
        // ... after a quit (the window stays open until the pump sets the flag) ...
        "late_failure_after_quit" => Case {
            quit_at_pump: Some(2),
            ui: String::new(),
            ..late_failure(&["late GPU error"], base)
        },
        // ... two of them ...
        "two_late_failures_in_presenter" => {
            late_failure(&["first late GPU error", "second late GPU error"], base)
        }
        // ... next to a presenter failure that ended the run already ...
        "late_failure_after_primary_presenter_failure" => Case {
            presenter: PresenterScript {
                fail: Some("primary presenter failure".to_owned()),
                fail_at_stop: vec!["late GPU error".to_owned()],
                ..PresenterScript::default()
            },
            ui: String::new(),
            ..base
        },
        // ... next to a UI VM failure ...
        "late_failure_after_ui_failure" => Case {
            ui: "window.on_opened(function() window.boom() end)".to_owned(),
            ..late_failure(&["late GPU error"], base)
        },
        // ... and with an unproven cleanup, where the exit status is 3 and nothing is released.
        "late_failure_with_unproven_cleanup" => Case {
            presenter: PresenterScript {
                fail_at_stop: vec!["late GPU error".to_owned()],
                end: End::Unproven("an unproven GPU wait".to_owned()),
                ..PresenterScript::default()
            },
            ..base
        },
        _ => return render_case(name, base),
    })
}

/// The render cases (CK-93): the same script, a quad placed before the window opens, over the two renderers; and
/// the paths that end a run, each with a `render.quad` in the UI VM's `shutdown()` that must be discarded.
fn render_case(name: &str, base: Case) -> Option<Case> {
    let render = match name {
        "render_shaped" => Render::Shaped,
        _ => return shutdown_quad_case(name, base),
    };
    Some(Case {
        ui: RENDER_UI.to_owned(),
        render,
        debug: true,
        ..base
    })
}

/// A run that ends by `name`'s path, whose UI `shutdown()` calls `render.quad`: the bridge was ended by the
/// application when the run phase ended, so the write is discarded and counted (CK-93 r2).
fn shutdown_quad_case(name: &str, base: Case) -> Option<Case> {
    let case = Case {
        debug: true,
        shutdown_quad: true,
        render: Render::Shaped,
        ..base
    };
    Some(match name {
        "shutdown_quad_normal_close" => case,
        "shutdown_quad_headless" => Case {
            headless: true,
            ui: String::new(),
            ..case
        },
        "shutdown_quad_no_window" => Case {
            no_open: true,
            ui: String::new(),
            ..case
        },
        "shutdown_quad_open_and_close_in_the_first_batch" => Case {
            ui: "window.close()".to_owned(),
            ..case
        },
        "shutdown_quad_quit_before_the_presenter_starts" => Case {
            quit_in_make: true,
            ui: String::new(),
            ..case
        },
        "shutdown_quad_failed_native_creation" => Case {
            backend: BackendScript {
                create_error: Some("no window could be created".to_owned()),
                ..BackendScript::default()
            },
            ui: String::new(),
            ..case
        },
        _ => return None,
    })
}

/// A presenter that publishes these failures while it stops and then proves its cleanup.
fn late_failure(messages: &[&str], base: Case) -> Case {
    Case {
        presenter: PresenterScript {
            fail_at_stop: messages
                .iter()
                .map(|message| (*message).to_owned())
                .collect(),
            ..PresenterScript::default()
        },
        ..base
    }
}

/// A non-callback failure (the presenter's) while the UI's callback is held, and a late callback failure.
fn late_after_primary(dir: &Path, release_after: Duration, base: Case) -> Case {
    Case {
        presenter: PresenterScript {
            fail: Some("primary presenter failure".to_owned()),
            fail_when_file: Some(dir.join("started")),
            ..PresenterScript::default()
        },
        ui: held_body(dir, false),
        held: Some(Held {
            release_after,
            escape: false,
        }),
        ..base
    }
}

/// A recorder that leaves one empty file per event in `dir`.
fn marker_recorder(dir: &Path) -> Recorder {
    let dir = dir.to_path_buf();
    Arc::new(move |event| {
        let _ = fs::write(dir.join(format!("rec-{}", event.replace(' ', "_"))), "x");
    })
}

/// The Lua helper that leaves a marker file.
fn touch(path: &Path) -> String {
    format!(
        "local f = assert(io.open([==[{}]==], 'w')); f:write('x'); f:close()",
        path.display()
    )
}

/// Writes the configuration and the two scripts of a case into `dir`.
fn write_fixture(dir: &Path, case: &Case) -> PathBuf {
    let open = if case.no_open {
        ""
    } else {
        "window.open{title='boundary', width=100, height=80, clear={0, 0, 0}}\n"
    };
    let late_quad = if case.shutdown_quad {
        "render.quad{image='small.png'}"
    } else {
        ""
    };
    let ui = format!(
        "{open}\
         window.on_close_request(function(source)\n  log.info('declined close request from ' .. tostring(source))\n  return false\nend)\n\
         {}\n\
         function shutdown() {} {late_quad} end\n",
        case.ui,
        touch(&dir.join("ui_shutdown"))
    );
    let gameplay = format!(
        "function shutdown() {} end\n",
        touch(&dir.join("gameplay_shutdown"))
    );
    fs::write(dir.join("ui.lua"), ui).expect("the ui script");
    fs::write(dir.join("gameplay.lua"), gameplay).expect("the gameplay script");
    let level = if case.debug {
        "level = \"debug\"\n"
    } else {
        ""
    };
    let config = format!(
        "[log]\n{level}timestamps = false\ncolour = \"never\"\n\n[window]\nbackend = \"native-metal\"\n\n\
         [render]\nfilter = \"linear\"\nassets = '{0}/assets'\n\n\
         [lua.gameplay]\nentry = '{0}/gameplay.lua'\n\n[lua.ui]\nentry = '{0}/ui.lua'\n",
        dir.display()
    );
    fs::create_dir_all(dir.join("assets")).expect("an assets directory");
    fs::write(
        dir.join("assets/small.png"),
        crate::testing::png(8, 4, &[255; 8 * 4 * 4]),
    )
    .expect("an image");
    let path = dir.join("config.toml");
    fs::write(&path, config).expect("the configuration");
    path
}

/// The child's `main`: the real logger, panic hook and exit path around the real `execute_with`. It
/// never returns.
fn child_main(name: &str, dir: &Path) -> ! {
    let case = case(name, dir);
    let config = write_fixture(dir, &case);
    lifecycle::install_logger().expect("the logger");
    lifecycle::install_panic_hook();
    let options = RunOptions {
        config: Some(config),
        headless: case.headless,
        backend: None,
    };
    let shared = Shared {
        quit: Arc::new(AtomicBool::new(false)),
        notifier: Notifier::new(),
    };
    let (quit, quit_notifier) = (Arc::clone(&shared.quit), Arc::clone(&shared.notifier));
    let record = marker_recorder(dir);
    let plan = move |setup: Setup| -> Plan {
        let mut env = setup.env;
        env.settings =
            ck_window::LoopSettings::new(64, 16, case.teardown, Duration::from_millis(100))
                .expect("settings");
        let boom: ck_lua::HostFunction = Arc::new(|_| panic!("a forced panic in the VM owner"));
        let extension = setup.extension.function("boom", boom);
        let notifier = Arc::clone(&env.notifier);
        let (backend_script, presenter_script, held) = (case.backend, case.presenter, case.held);
        let render = case.render;
        let quit_in_make = case.quit_in_make;
        let make_quit = (Arc::clone(&quit), Arc::clone(&quit_notifier));
        let quit_at_pump = case.quit_at_pump;
        let dir = dir.to_path_buf();
        let run: RunClosure = Box::new(move |_| {
            let backend_record = Arc::clone(&record);
            let presenter =
                FakePresenter::new(Arc::clone(&notifier), presenter_script, Arc::clone(&record));
            let make = move || {
                let mut backend = FakeBackend::new(backend_script, backend_record);
                if let Some(held) = held {
                    drive_held_callback(&mut backend, &dir, held);
                }
                if quit_in_make {
                    // The quit arrives as the window is being created, before the presenter starts.
                    make_quit.0.store(true, std::sync::atomic::Ordering::SeqCst);
                    make_quit.1.wake();
                }
                if let Some(index) = quit_at_pump {
                    backend.on_pump = Some(Box::new(move |pump| {
                        if pump == index {
                            quit.store(true, std::sync::atomic::Ordering::SeqCst);
                            quit_notifier.wake();
                        }
                    }));
                }
                Ok(backend)
            };
            let bridge = Arc::clone(&env.bridge);
            match render {
                Render::Plain => windowed::run_window(
                    &env,
                    make,
                    presenter,
                    &lifecycle::RESOURCES,
                    windowed::DRAIN,
                ),
                Render::Shaped => windowed::run_window(
                    &env,
                    make,
                    Renderer::new(FakeGpu::recording(Arc::clone(&record)), bridge),
                    &lifecycle::RESOURCES,
                    windowed::DRAIN,
                ),
            }
        });
        Plan {
            extension,
            run: Some(run),
        }
    };
    let status = execute_with(&options, shared, &[mode::Backend::NativeMetal], plan);
    let code = lifecycle::finish_code(status);
    std::process::exit(i32::from(code));
}

/// Wires the fake backend of a late-failure case: the gate file appears `release_after` the window's
/// destruction, and with `escape` an Escape press is queued once the callback is held and the `go` file is
/// created one pump later (so the pass that posted the close request has finished).
fn drive_held_callback(backend: &mut FakeBackend, dir: &Path, held: Held) {
    let gate = dir.join("gate");
    backend.on_destroy = Some(Box::new(move || {
        let gate = gate.clone();
        std::thread::spawn(move || {
            std::thread::sleep(held.release_after);
            let _ = fs::write(&gate, "x");
        });
    }));
    if held.escape {
        let (started, go) = (dir.join("started"), dir.join("go"));
        let pending = Arc::clone(&backend.pending);
        let mut pushed_at = None;
        backend.on_pump = Some(Box::new(move |pump| match pushed_at {
            None if started.exists() => {
                pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_back(ck_window::NativeEvent::EscapePressed);
                pushed_at = Some(pump);
            }
            Some(at) if pump > at => {
                let _ = fs::write(&go, "x");
            }
            _ => {}
        }));
    }
}

/// The child's entry: a no-op in an ordinary test run, the whole child when the parent asks for a case.
#[test]
fn child() {
    let (Some(name), Some(dir)) = (std::env::var_os(CASE), std::env::var_os(DIR)) else {
        return;
    };
    child_main(&name.to_string_lossy(), &PathBuf::from(dir));
}

// ---------------------------------------------------------------------------------------------
// The parent: supervision
// ---------------------------------------------------------------------------------------------

/// Runs `name` in a fresh child under the supervisor; the marker files stay in `dir`.
fn run_child(name: &str, dir: &Path) -> Seen {
    let mut command = Command::new(std::env::current_exe().expect("this executable"));
    command
        .args([
            "--exact",
            "boundary::child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CASE, name)
        .env(DIR, dir);
    Supervised::spawn(command, DEADLINE)
        .and_then(Supervised::finish)
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// A fresh directory for one case, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("codekarl-boundary-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a case directory");
        Self(path)
    }

    fn has(&self, marker: &str) -> bool {
        self.0.join(marker).exists()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs the case and returns what it showed with its directory.
fn run(name: &str) -> (Seen, Dir) {
    let dir = Dir::new(name);
    let seen = run_child(name, &dir.0);
    (seen, dir)
}

/// Nothing a window or presenter owns was released: no teardown call, no drop.
fn assert_nothing_released(dir: &Dir, name: &str) {
    for released in [
        "rec-destroy_window",
        "rec-terminate",
        "rec-backend_dropped",
        "rec-presenter_dropped",
    ] {
        assert!(!dir.has(released), "{name}: {released} must not exist");
    }
}

#[test]
fn the_control_case_closes_cleanly_and_exits_0() {
    let (seen, dir) = run("proven_close");
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert!(dir.has("rec-destroy_window") && dir.has("rec-terminate"));
    assert!(dir.has("ui_shutdown") && dir.has("gameplay_shutdown"));
}

/// The record of a run whose UI `shutdown()` wrote one quad after the end of the run phase.
const SHUTDOWN_QUAD_COUNTS: &str =
    "render: 1 scene write(s) accepted, 1 discarded after the run ended";

/// Every path that ends the run phase ends the bridge, so `shutdown()`'s `render.quad` is discarded.
#[test]
fn a_render_quad_in_shutdown_is_discarded_and_counted_on_every_path_that_runs_the_stop_phase() {
    for name in [
        "shutdown_quad_normal_close",
        "shutdown_quad_headless",
        "shutdown_quad_no_window",
        "shutdown_quad_open_and_close_in_the_first_batch",
        "shutdown_quad_quit_before_the_presenter_starts",
        "shutdown_quad_failed_native_creation",
    ] {
        let (seen, dir) = run(name);
        assert!(
            dir.has("ui_shutdown"),
            "{name}: the stop phase ran shutdown()"
        );
        assert!(
            seen.stdout.contains(SHUTDOWN_QUAD_COUNTS),
            "{name}: {}",
            seen.stdout
        );
        assert_eq!(
            seen.code,
            Some(i32::from(name == "shutdown_quad_failed_native_creation")),
            "{name}: {}",
            seen.stdout
        );
    }
}

#[test]
fn the_same_quad_over_a_metal_shaped_backend_logs_no_warning_and_gets_the_configured_filter() {
    let (seen, dir) = run("render_shaped");
    assert!(
        dir.has("rec-snapshot_filter_linear") && !dir.has("rec-snapshot_filter_nearest"),
        "the `[render]` filter reaches the bridge"
    );
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_eq!(
        count(&seen.stdout, "textured quads on"),
        0,
        "{}",
        seen.stdout
    );
    assert!(
        seen.stdout
            .contains("render: 1 scene write(s) accepted, 0 discarded after the run ended"),
        "{}",
        seen.stdout
    );
}

#[test]
fn a_main_thread_panic_during_the_run_with_the_latch_set_exits_3_and_releases_nothing() {
    let (seen, dir) = run("main_panic");
    assert_eq!(seen.code, Some(3), "{}", seen.stdout);
    assert!(
        dir.has("rec-create_window"),
        "the window existed (the latch was set before it)"
    );
    assert_nothing_released(&dir, "main_panic");
    assert!(seen.stdout.contains("panic:"), "{}", seen.stdout);
    assert!(
        !dir.has("ui_shutdown") && !dir.has("gameplay_shutdown"),
        "no stop phase after the panic"
    );
}

#[test]
fn a_partial_setup_failure_exits_1_only_when_the_cleanup_is_proven() {
    let (seen, dir) = run("setup_proven");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert!(seen.stdout.contains("run failed:"), "{}", seen.stdout);
    assert!(seen.stdout.contains("no Metal device"), "{}", seen.stdout);
    assert!(
        dir.has("rec-destroy_window") && dir.has("rec-terminate"),
        "proven: the window was torn down"
    );
    assert!(dir.has("ui_shutdown") && dir.has("gameplay_shutdown"));

    let (seen, dir) = run("setup_unproven");
    assert_eq!(seen.code, Some(3), "{}", seen.stdout);
    assert_nothing_released(&dir, "setup_unproven");
    assert!(
        dir.has("ui_shutdown") && dir.has("gameplay_shutdown"),
        "the normal stop phase still ran"
    );
}

#[test]
fn a_render_thread_that_ends_with_no_result_or_a_failed_one_exits_3_after_the_stop_phase() {
    for name in ["thread_no_result", "thread_failed_result"] {
        let (seen, dir) = run(name);
        assert_eq!(seen.code, Some(3), "{name}: {}", seen.stdout);
        assert!(seen.stdout.contains("unproven"), "{name}: {}", seen.stdout);
        assert_nothing_released(&dir, name);
        assert!(
            dir.has("ui_shutdown") && dir.has("gameplay_shutdown"),
            "{name}: both shutdown() ran"
        );
    }
}

#[test]
fn an_unproven_loop_end_runs_both_shutdowns_and_then_exits_3() {
    let (seen, dir) = run("unproven_loop_end");
    assert_eq!(seen.code, Some(3), "{}", seen.stdout);
    assert!(dir.has("ui_shutdown"), "the ui VM's shutdown() ran");
    assert!(
        dir.has("gameplay_shutdown"),
        "the gameplay VM's shutdown() ran"
    );
    let order = |needle: &str| {
        seen.stdout
            .find(needle)
            .unwrap_or_else(|| panic!("{needle}: {}", seen.stdout))
    };
    assert!(
        order("run failed:") < order("shutdown complete"),
        "the stop phase follows the failed run: {}",
        seen.stdout
    );
    assert_nothing_released(&dir, "unproven_loop_end");
}

#[test]
fn a_vm_owner_panic_with_the_latch_set_keeps_its_accepted_handling() {
    let (seen, dir) = run("owner_panic");
    // Reported as that owner's failure, then the normal, proven shutdown: status 1, not 3.
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert!(
        seen.stdout.contains("a forced panic in the VM owner"),
        "{}",
        seen.stdout
    );
    assert!(
        dir.has("rec-destroy_window") && dir.has("rec-terminate"),
        "the proven teardown ran"
    );
    assert!(
        dir.has("gameplay_shutdown"),
        "the other VM shut down normally"
    );
}

#[test]
fn a_panic_on_another_thread_that_is_not_a_vm_owner_with_the_latch_set_exits_3() {
    let (seen, dir) = run("other_thread_panic");
    assert_eq!(seen.code, Some(3), "{}", seen.stdout);
    assert!(
        seen.stdout
            .contains("a forced panic on a thread that is not a VM owner"),
        "{}",
        seen.stdout
    );
    assert_nothing_released(&dir, "other_thread_panic");
}

// ---------------------------------------------------------------------------------------------
// Late callback failures (spec §3.5, review 1 B1 and B3)
// ---------------------------------------------------------------------------------------------

/// How many times `needle` occurs.
fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

/// A late callback failure was logged exactly once, with its traceback, as a record of its own.
fn assert_late_failure_logged_once(stdout: &str) {
    assert_eq!(count(stdout, "ui callback opened failed: "), 1, "{stdout}");
    assert_eq!(count(stdout, "late callback failure"), 1, "{stdout}");
    assert!(stdout.contains("stack traceback"), "{stdout}");
}

#[test]
fn a_late_callback_failure_after_a_presenter_failure_is_logged_once_when_it_arrives_within_the_drain()
 {
    let (seen, _dir) = run("late_after_primary_in_drain");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert_eq!(
        count(
            &seen.stdout,
            "run failed: the presenter failed: primary presenter failure"
        ),
        1,
        "the primary failure stays the run's failure: {}",
        seen.stdout
    );
    assert_late_failure_logged_once(&seen.stdout);
    assert!(
        !seen.stdout.contains("still running at shutdown"),
        "the callback ended within the drain: {}",
        seen.stdout
    );
    // The late failure is its own record, not a second `run failed`.
    assert_eq!(count(&seen.stdout, "run failed:"), 1, "{}", seen.stdout);
}

#[test]
fn a_late_callback_failure_after_a_presenter_failure_is_logged_once_when_it_arrives_in_the_stop_phase()
 {
    let (seen, _dir) = run("late_after_primary_in_stop");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert_eq!(
        count(
            &seen.stdout,
            "run failed: the presenter failed: primary presenter failure"
        ),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(&seen.stdout, "ui callback opened still running at shutdown"),
        1,
        "{}",
        seen.stdout
    );
    assert_late_failure_logged_once(&seen.stdout);
    let at = |needle: &str| {
        seen.stdout
            .find(needle)
            .unwrap_or_else(|| panic!("{needle}: {}", seen.stdout))
    };
    assert!(
        at("still running at shutdown") < at("run failed:")
            && at("run failed:") < at("ui callback opened failed: "),
        "the warning, then the primary failure, then the late callback failure (after the stop phase): {}",
        seen.stdout
    );
    assert_eq!(count(&seen.stdout, "run failed:"), 1, "{}", seen.stdout);
}

#[test]
fn an_escape_queued_behind_a_held_callback_adds_its_own_warning_and_changes_nothing_else() {
    let (seen, _dir) = run("escape_behind_held_callback");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    // The held `opened` and the close request queued behind it are both still running at the drain bound.
    assert_eq!(
        count(&seen.stdout, "ui callback opened still running at shutdown"),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(
            &seen.stdout,
            "ui callback close_request still running at shutdown"
        ),
        1,
        "{}",
        seen.stdout
    );
    // The close callback declines as the fixture intends, after the held callback failed.
    assert_eq!(
        count(&seen.stdout, "declined close request from key"),
        1,
        "{}",
        seen.stdout
    );
    assert_late_failure_logged_once(&seen.stdout);
    assert_eq!(count(&seen.stdout, "run failed:"), 1, "{}", seen.stdout);
    assert!(
        seen.stdout.contains("window closed") || seen.stdout.contains("runtime: shutdown complete"),
        "{}",
        seen.stdout
    );
}

// ---------------------------------------------------------------------------------------------
// Late presenter failures (CK-73): reported once, status 1, unless an incomplete cleanup makes it 3
// ---------------------------------------------------------------------------------------------

/// The run's failure record names this presenter failure, once.
fn assert_run_failed_with(stdout: &str, message: &str) {
    assert_eq!(
        count(
            stdout,
            &format!("run failed: the presenter failed: {message}")
        ),
        1,
        "{stdout}"
    );
    assert_eq!(count(stdout, message), 1, "reported once: {stdout}");
}

/// The proven teardown ran, in full, around the failure.
fn assert_torn_down_and_stopped(dir: &Dir) {
    assert!(
        dir.has("rec-destroy_window") && dir.has("rec-terminate"),
        "the proven teardown ran"
    );
    assert!(dir.has("ui_shutdown") && dir.has("gameplay_shutdown"));
}

#[test]
fn a_presenter_failure_published_while_stopping_fails_a_proven_close_with_status_1() {
    let (seen, dir) = run("late_failure_after_close");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert_run_failed_with(&seen.stdout, "late GPU error");
    assert_eq!(count(&seen.stdout, "ERROR window:"), 0, "{}", seen.stdout);
    assert_torn_down_and_stopped(&dir);
}

#[test]
fn a_presenter_failure_published_while_stopping_fails_a_quit_with_status_1_not_a_signal_status() {
    let (seen, dir) = run("late_failure_after_quit");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert_run_failed_with(&seen.stdout, "late GPU error");
    assert_torn_down_and_stopped(&dir);
}

#[test]
fn two_late_presenter_failures_are_one_run_failure_and_one_record() {
    let (seen, dir) = run("two_late_failures_in_presenter");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert_run_failed_with(&seen.stdout, "first late GPU error");
    assert_eq!(
        count(
            &seen.stdout,
            "ERROR window: the presenter failed: second late GPU error"
        ),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(count(&seen.stdout, "second late GPU error"), 1);
    assert_torn_down_and_stopped(&dir);
}

#[test]
fn a_late_presenter_failure_next_to_an_earlier_one_keeps_the_earlier_as_the_runs_failure() {
    let (seen, dir) = run("late_failure_after_primary_presenter_failure");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert_run_failed_with(&seen.stdout, "primary presenter failure");
    assert_eq!(
        count(
            &seen.stdout,
            "ERROR window: the presenter failed: late GPU error"
        ),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(count(&seen.stdout, "run failed:"), 1, "{}", seen.stdout);
    assert_torn_down_and_stopped(&dir);
}

#[test]
fn a_late_presenter_failure_next_to_a_ui_vm_failure_is_its_own_record() {
    let (seen, dir) = run("late_failure_after_ui_failure");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert_eq!(
        count(&seen.stdout, "run failed: the ui VM failed: "),
        1,
        "{}",
        seen.stdout
    );
    assert!(
        seen.stdout.contains("a forced panic in the VM owner"),
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(
            &seen.stdout,
            "ERROR window: the presenter failed: late GPU error"
        ),
        1,
        "{}",
        seen.stdout
    );
    // The dead UI VM runs no shutdown(); the proven teardown and the other VM's shutdown are as usual.
    assert!(dir.has("rec-destroy_window") && dir.has("rec-terminate"));
    assert!(dir.has("gameplay_shutdown"));
}

#[test]
fn a_late_presenter_failure_with_an_unproven_cleanup_exits_3_keeps_everything_and_still_reports_it()
{
    let (seen, dir) = run("late_failure_with_unproven_cleanup");
    assert_eq!(seen.code, Some(3), "{}", seen.stdout);
    assert_nothing_released(&dir, "late_failure_with_unproven_cleanup");
    assert!(seen.stdout.contains("unproven"), "{}", seen.stdout);
    assert_eq!(
        count(
            &seen.stdout,
            "ERROR window: the presenter failed: late GPU error"
        ),
        1,
        "{}",
        seen.stdout
    );
    assert!(
        dir.has("ui_shutdown") && dir.has("gameplay_shutdown"),
        "the normal stop phase still ran"
    );
}

// ---------------------------------------------------------------------------------------------
// The supervisor's own failure paths (review 1 B2)
// ---------------------------------------------------------------------------------------------

/// A process the supervisor tests drive: it can fail to be killed, fail to be polled, or never exit.
struct FakeProcess {
    kill: Kill,
    poll_fails: bool,
    /// The process has exited (a kill that works makes it exit).
    exited: bool,
    killed: Arc<AtomicBool>,
}

/// What a kill does to the fake process.
#[derive(Clone, Copy)]
enum Kill {
    /// It works: the process exits.
    Works,
    /// It is refused: an error, and the process is left as it was.
    Refused,
}

impl FakeProcess {
    fn new() -> Self {
        Self {
            kill: Kill::Works,
            poll_fails: false,
            exited: false,
            killed: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl supervisor::Process for FakeProcess {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        use std::os::unix::process::ExitStatusExt;
        if self.poll_fails {
            return Err(std::io::Error::other("injected poll failure"));
        }
        Ok(self.exited.then(|| std::process::ExitStatus::from_raw(0)))
    }

    fn kill(&mut self) -> std::io::Result<()> {
        self.killed.store(true, std::sync::atomic::Ordering::SeqCst);
        match self.kill {
            Kill::Refused => Err(std::io::Error::other("injected kill failure")),
            Kill::Works => {
                self.exited = true;
                Ok(())
            }
        }
    }
}

/// A stream that yields some bytes and then fails.
struct FailingRead(bool);

impl std::io::Read for FailingRead {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.0 {
            return Err(std::io::Error::other("injected read failure"));
        }
        self.0 = true;
        let partial = b"partial output";
        buffer[..partial.len()].copy_from_slice(partial);
        Ok(partial.len())
    }
}

fn shared_text() -> Arc<std::sync::Mutex<String>> {
    Arc::new(std::sync::Mutex::new(String::new()))
}

#[test]
fn a_child_held_past_the_run_bound_is_killed_and_reaped_within_the_budget() {
    let mut command = Command::new("sh");
    command.args(["-c", "echo ready; exec sleep 60"]);
    let mut supervised =
        Supervised::spawn(command, Duration::from_millis(400)).expect("the child starts");
    supervised
        .wait_for("ready", Duration::from_secs(10))
        .expect("the child says it is ready");
    let pid = i32::try_from(supervised.id()).expect("a pid");
    let started = std::time::Instant::now();
    let error = supervised
        .finish()
        .expect_err("the child outlives its deadline");
    assert!(error.contains("outlived its deadline"), "{error}");
    assert!(error.contains("killed and reaped"), "{error}");
    assert!(
        started.elapsed() < Duration::from_millis(400) + supervisor::REAP_BUDGET,
        "within the deadline and the reap budget: {:?}",
        started.elapsed()
    );
    // SAFETY: signal 0 only asks whether the process still exists; the pid is the reaped child's.
    let alive = unsafe { libc::kill(pid, 0) };
    assert_eq!(alive, -1, "the child is gone");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH),
        "and was reaped, not left a zombie"
    );
}

#[test]
fn a_pipe_read_error_fails_the_case_and_never_counts_as_complete_output() {
    let text = shared_text();
    let reader =
        supervisor::Reader::spawn(FailingRead(false), Arc::clone(&text)).expect("a reader");
    let mut process = FakeProcess::new();
    process.exited = true;
    let supervised = Supervised::from_parts(process, vec![reader], text, Duration::from_secs(10));
    let error = supervised
        .finish()
        .expect_err("a read error fails the case");
    assert!(
        error.contains("read error: injected read failure"),
        "{error}"
    );
    assert!(
        error.contains("partial output"),
        "the partial text is shown, not accepted: {error}"
    );
}

#[test]
fn a_failed_kill_and_a_process_that_never_exits_are_reported_within_the_budget() {
    let mut process = FakeProcess::new();
    process.kill = Kill::Refused;
    let started = std::time::Instant::now();
    let error = supervisor::kill_and_reap(&mut process, Duration::from_millis(150))
        .expect_err("it cannot be reaped");
    assert!(
        error.contains("kill failed: injected kill failure"),
        "{error}"
    );
    assert!(error.contains("not reaped within"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_failed_kill_of_a_process_that_is_gone_is_still_reported() {
    let mut process = FakeProcess::new();
    process.kill = Kill::Refused;
    process.exited = true;
    let error = supervisor::kill_and_reap(&mut process, Duration::from_millis(150))
        .expect_err("the failed kill is reported even though the process is gone");
    assert!(error.contains("kill failed"), "{error}");
}

#[test]
fn a_failure_to_inspect_the_process_fails_closed() {
    let mut process = FakeProcess::new();
    process.poll_fails = true;
    let started = std::time::Instant::now();
    let supervised =
        Supervised::from_parts(process, Vec::new(), shared_text(), Duration::from_secs(10));
    let error = supervised
        .finish()
        .expect_err("an unpollable process fails the case");
    assert!(
        error.contains("cannot poll the process: injected poll failure"),
        "{error}"
    );
    // At once: not by waiting out the deadline and failing then.
    assert!(!error.contains("outlived its deadline"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_reader_that_never_ends_is_reported_and_stays_accounted_until_it_does() {
    let (stream, writer) = supervisor::pipe_locked(&supervisor::spawn_guard()).expect("a pipe");
    let mut reader = supervisor::Reader::spawn(stream, shared_text()).expect("a reader");
    let started = std::time::Instant::now();
    let error = reader
        .finish(Duration::from_millis(150))
        .expect_err("the pipe is still open");
    assert!(error.contains("did not exit within"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    // Closing the write end lets the thread end; it is then joined, not abandoned.
    drop(writer);
    reader
        .finish(Duration::from_secs(5))
        .expect("the reader ended and was joined");
}

#[test]
fn dropping_a_supervised_process_kills_and_reaps_it_with_a_bound() {
    let process = FakeProcess::new();
    let killed = Arc::clone(&process.killed);
    let supervised =
        Supervised::from_parts(process, Vec::new(), shared_text(), Duration::from_secs(10));
    drop(supervised);
    assert!(
        killed.load(std::sync::atomic::Ordering::SeqCst),
        "the drop killed the process"
    );
}

#[test]
fn every_late_callback_failure_is_logged_once_not_only_the_first() {
    let (seen, _dir) = run("two_late_failures");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    // The first is the run's failure, the second a record of its own; neither hides the other.
    assert_eq!(
        count(&seen.stdout, "run failed: ui callback opened failed: "),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(&seen.stdout, "late callback failure"),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(&seen.stdout, "ui callback close_request failed: "),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(&seen.stdout, "close callback failure"),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(count(&seen.stdout, "run failed:"), 1, "{}", seen.stdout);
    assert_eq!(
        count(&seen.stdout, "stack traceback"),
        2,
        "one traceback each: {}",
        seen.stdout
    );
}

// ---------------------------------------------------------------------------------------------
// Readers: ownership and actual completion on every path (review 2 B2)
// ---------------------------------------------------------------------------------------------

/// A flag a test opens; whatever waits on it does so with a bound, so a bug fails a test, not hangs it.
#[derive(Clone, Default)]
struct Gate(Arc<AtomicBool>);

impl Gate {
    fn open(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn wait(&self) {
        let give_up = std::time::Instant::now() + Duration::from_secs(30);
        while !self.0.load(std::sync::atomic::Ordering::SeqCst)
            && std::time::Instant::now() < give_up
        {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// Opens its gate when dropped, so a failing test never leaves a reader blocked.
struct OpenOnDrop(Gate);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        self.0.open();
    }
}

/// A stream whose read blocks until the gate opens, then ends.
struct GatedRead(Gate);

impl std::io::Read for GatedRead {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        self.0.wait();
        Ok(0)
    }
}

/// A stream that ends at once, and whose destructor blocks until the gate opens.
struct EofThenSlowDrop(Gate);

impl std::io::Read for EofThenSlowDrop {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
}

impl Drop for EofThenSlowDrop {
    fn drop(&mut self) {
        self.0.wait();
    }
}

/// Finishes the readers the leak registry holds for `label` within `grace`; the ones that ended leave it.
fn drain_leaked(label: &str, grace: Duration) -> Result<(), String> {
    let give_up = std::time::Instant::now() + grace;
    let mine: Vec<_> = {
        let mut registry = supervisor::LEAKED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mine, rest): (Vec<_>, Vec<_>) =
            registry.drain(..).partition(|(owner, _)| owner == label);
        *registry = rest;
        mine
    };
    let mut problems = Vec::new();
    let mut still = Vec::new();
    for (owner, mut reader) in mine {
        if let Err(problem) =
            reader.finish(give_up.saturating_duration_since(std::time::Instant::now()))
        {
            problems.push(problem);
            if reader.is_running() {
                still.push((owner, reader));
            }
        }
    }
    supervisor::LEAKED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend(still);
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

/// How many readers the registry holds for `label`.
fn leaked(label: &str) -> usize {
    supervisor::LEAKED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|(owner, _)| owner == label)
        .count()
}

/// A supervisor over a fake process and one gated reader, with short graces.
fn gated(process: FakeProcess, deadline: Duration) -> (Supervised<FakeProcess>, Gate) {
    let gate = Gate::default();
    let text = shared_text();
    let reader =
        supervisor::Reader::spawn(GatedRead(gate.clone()), Arc::clone(&text)).expect("a reader");
    let mut supervised = Supervised::from_parts(process, vec![reader], text, deadline);
    supervised.pipe_grace = Duration::from_millis(100);
    supervised.reap_budget = Duration::from_secs(1);
    (supervised, gate)
}

/// The registry holds this test's reader, and it can be finished once the gate opens.
fn assert_kept_then_released(label: &str, gate: &Gate) {
    assert_eq!(
        leaked(label),
        1,
        "the reader is owned by the registry, not dropped"
    );
    assert!(
        drain_leaked(label, Duration::from_millis(50)).is_err(),
        "still blocked"
    );
    assert_eq!(leaked(label), 1, "and still owned");
    gate.open();
    drain_leaked(label, Duration::from_secs(5)).expect("it ended and was joined");
    assert_eq!(leaked(label), 0);
}

#[test]
fn a_reader_still_blocked_when_the_supervisor_is_dropped_is_reported_and_kept_not_detached() {
    let process = FakeProcess::new();
    let killed = Arc::clone(&process.killed);
    let (supervised, gate) = gated(process, Duration::from_secs(10));
    let _open = OpenOnDrop(gate.clone());
    let label = supervised.label.clone();
    let started = std::time::Instant::now();
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(supervised)))
        .expect_err("the drop reports the reader that did not end");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(message.contains("did not exit within"), "{message}");
    assert!(message.contains("leak registry"), "{message}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(killed.load(std::sync::atomic::Ordering::SeqCst));
    assert_kept_then_released(&label, &gate);
}

#[test]
fn a_reader_at_eof_that_is_still_dropping_its_stream_has_not_ended() {
    let gate = Gate::default();
    let _open = OpenOnDrop(gate.clone());
    let mut reader =
        supervisor::Reader::spawn(EofThenSlowDrop(gate.clone()), shared_text()).expect("a reader");
    // The read ended at once; only the stream's destructor is held.
    std::thread::sleep(Duration::from_millis(100));
    let started = std::time::Instant::now();
    let error = reader
        .finish(Duration::from_millis(20))
        .expect_err("EOF is not the end: the stream is still being dropped");
    assert!(error.contains("did not exit within"), "{error}");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert!(reader.is_running(), "and the reader is still owned");
    gate.open();
    reader.finish(Duration::from_secs(5)).expect("now it ended");
    assert!(!reader.is_running());
}

#[test]
fn a_partial_spawn_failure_cleans_up_the_readers_already_started_and_the_process() {
    let gate = Gate::default();
    let _open = OpenOnDrop(gate.clone());
    let process = FakeProcess::new();
    let killed = Arc::clone(&process.killed);
    let calls = std::cell::Cell::new(0);
    let label = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned();
    let streams: Vec<Box<dyn std::io::Read + Send>> = vec![
        Box::new(GatedRead(gate.clone())),
        Box::new(GatedRead(gate.clone())),
    ];
    let result = Supervised::adopt(
        process,
        streams,
        Duration::from_secs(10),
        (Duration::from_millis(100), Duration::from_secs(1)),
        |stream, text| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                supervisor::Reader::spawn(stream, text)
            } else {
                Err(std::io::Error::other("no threads left"))
            }
        },
    );
    let error = result.err().expect("the second reader cannot start");
    assert!(
        error.contains("a reader thread did not start: no threads left"),
        "{error}"
    );
    assert!(
        error.contains("leak registry"),
        "the first reader is accounted: {error}"
    );
    assert!(
        killed.load(std::sync::atomic::Ordering::SeqCst),
        "and the process was killed"
    );
    assert_kept_then_released(&label, &gate);
}

#[test]
fn a_pipe_timeout_in_finish_reports_the_reader_and_keeps_it() {
    let mut process = FakeProcess::new();
    process.exited = true;
    let (supervised, gate) = gated(process, Duration::from_secs(10));
    let _open = OpenOnDrop(gate.clone());
    let label = supervised.label.clone();
    let started = std::time::Instant::now();
    let error = supervised
        .finish()
        .expect_err("the pipe never reaches its end");
    assert!(error.contains("did not exit within"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_kept_then_released(&label, &gate);
}

#[test]
fn a_failed_poll_cleans_up_the_process_and_accounts_for_the_reader() {
    let mut process = FakeProcess::new();
    process.poll_fails = true;
    let killed = Arc::clone(&process.killed);
    let (supervised, gate) = gated(process, Duration::from_secs(10));
    let _open = OpenOnDrop(gate.clone());
    let label = supervised.label.clone();
    let error = supervised
        .finish()
        .expect_err("an unpollable process fails the case");
    assert!(error.contains("cannot poll the process"), "{error}");
    assert!(error.contains("leak registry"), "{error}");
    assert!(killed.load(std::sync::atomic::Ordering::SeqCst));
    assert_kept_then_released(&label, &gate);
}

#[test]
fn a_deadline_cleans_up_the_process_and_accounts_for_the_reader() {
    let (supervised, gate) = gated(FakeProcess::new(), Duration::from_millis(100));
    let _open = OpenOnDrop(gate.clone());
    let label = supervised.label.clone();
    let error = supervised
        .finish()
        .expect_err("the fake process never exits by itself");
    assert!(error.contains("outlived its deadline"), "{error}");
    assert!(error.contains("leak registry"), "{error}");
    assert_kept_then_released(&label, &gate);
}

#[test]
fn an_assertion_unwinding_through_a_supervisor_keeps_the_reader_owned_without_aborting() {
    let (supervised, gate) = gated(FakeProcess::new(), Duration::from_secs(10));
    let _open = OpenOnDrop(gate.clone());
    let label = supervised.label.clone();
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _held = supervised;
        panic!("an assertion failed in the case");
    }));
    let payload = unwound.expect_err("the case panicked");
    let message = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .unwrap_or_default();
    assert!(
        message.contains("an assertion failed"),
        "the original failure is the panic: {message}"
    );
    assert_kept_then_released(&label, &gate);
}

// ---------------------------------------------------------------------------------------------
// Raw split readers (CK-131 revision 1): the same ownership and cleanup, the streams kept apart as bytes
// ---------------------------------------------------------------------------------------------

/// Two gated byte readers (standard output and standard error) over `process`, with short graces.
fn gated_split(process: FakeProcess, deadline: Duration) -> (Supervised<FakeProcess>, Gate) {
    let gate = Gate::default();
    let streams: Vec<Box<dyn std::io::Read + Send>> = vec![
        Box::new(GatedRead(gate.clone())),
        Box::new(GatedRead(gate.clone())),
    ];
    let supervised = Supervised::adopt_split(
        process,
        streams,
        deadline,
        (Duration::from_millis(100), Duration::from_secs(1)),
        supervisor::Reader::spawn_bytes,
    )
    .expect("two readers start");
    (supervised, gate)
}

/// Both readers are owned by the registry, and both end once the gate opens.
fn assert_both_kept_then_released(label: &str, gate: &Gate) {
    assert_eq!(leaked(label), 2, "both readers are owned, none dropped");
    assert!(
        drain_leaked(label, Duration::from_millis(50)).is_err(),
        "still blocked"
    );
    assert_eq!(leaked(label), 2, "and still owned");
    gate.open();
    drain_leaked(label, Duration::from_secs(5)).expect("both ended and were joined");
    assert_eq!(leaked(label), 0);
}

#[test]
fn a_deadline_with_a_separately_held_pipe_accounts_for_every_reader() {
    // The shell is held past its deadline while a background process keeps both pipes open after the shell is
    // killed: the combined path the single-reader tests do not reach.
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 3 & exec sleep 60"]);
    let mut supervised = Supervised::spawn_split(&mut command, Duration::from_millis(300))
        .expect("the child starts");
    supervised.pipe_grace = Duration::from_millis(300);
    let label = supervised.label.clone();
    let pid = i32::try_from(supervised.id()).expect("a pid");
    let started = std::time::Instant::now();
    let error = supervised
        .finish_split()
        .expect_err("the deadline passes with a pipe held");
    assert!(error.contains("outlived its deadline"), "{error}");
    assert!(
        !error.contains("kill failed"),
        "the child was reaped: {error}"
    );
    assert!(error.contains("did not exit within"), "{error}");
    assert!(error.contains("2 reader(s) did not end"), "{error}");
    assert!(error.contains("leak registry"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "within the deadline and the graces: {:?}",
        started.elapsed()
    );
    // SAFETY: signal 0 only asks whether the process exists; the pid is the reaped child's.
    let alive = unsafe { libc::kill(pid, 0) };
    assert_eq!(alive, -1, "the held child is gone");
    assert_eq!(leaked(&label), 2, "both readers are owned by the registry");
    drain_leaked(&label, Duration::from_secs(10))
        .expect("the readers end when the background process closes the pipes");
    assert_eq!(leaked(&label), 0);
}

#[test]
fn a_failed_kill_at_the_deadline_is_reported_and_every_reader_is_still_visited() {
    let mut process = FakeProcess::new();
    process.kill = Kill::Refused;
    let killed = Arc::clone(&process.killed);
    let (supervised, gate) = gated_split(process, Duration::from_millis(100));
    let _open = OpenOnDrop(gate.clone());
    let label = supervised.label.clone();
    let error = supervised
        .finish_split()
        .expect_err("the fake process never exits and cannot be killed");
    assert!(error.contains("outlived its deadline"), "{error}");
    assert!(
        error.contains("kill failed: injected kill failure"),
        "{error}"
    );
    assert!(error.contains("not reaped within"), "{error}");
    assert!(
        error.contains("leak registry"),
        "both readers reported: {error}"
    );
    assert!(
        killed.load(std::sync::atomic::Ordering::SeqCst),
        "a kill was tried"
    );
    assert_both_kept_then_released(&label, &gate);
}

#[test]
fn a_failed_poll_cleans_up_a_split_process_and_both_readers() {
    let mut process = FakeProcess::new();
    process.poll_fails = true;
    let (supervised, gate) = gated_split(process, Duration::from_secs(10));
    let _open = OpenOnDrop(gate.clone());
    let label = supervised.label.clone();
    let error = supervised
        .finish_split()
        .expect_err("an unpollable process fails the case");
    assert!(
        error.contains("cannot poll the process: injected poll failure"),
        "{error}"
    );
    assert!(error.contains("leak registry"), "{error}");
    assert_both_kept_then_released(&label, &gate);
}

#[test]
fn a_split_partial_spawn_failure_cleans_up_the_reader_already_started_and_the_process() {
    let gate = Gate::default();
    let _open = OpenOnDrop(gate.clone());
    let process = FakeProcess::new();
    let killed = Arc::clone(&process.killed);
    let calls = std::cell::Cell::new(0);
    let label = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned();
    let streams: Vec<Box<dyn std::io::Read + Send>> = vec![
        Box::new(GatedRead(gate.clone())),
        Box::new(GatedRead(gate.clone())),
    ];
    let error = Supervised::adopt_split(
        process,
        streams,
        Duration::from_secs(10),
        (Duration::from_millis(100), Duration::from_secs(1)),
        |stream, bytes| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                supervisor::Reader::spawn_bytes(stream, bytes)
            } else {
                Err(std::io::Error::other("no threads left"))
            }
        },
    )
    .err()
    .expect("the second reader cannot start");
    assert!(
        error.contains("a reader thread did not start: no threads left"),
        "{error}"
    );
    assert!(error.contains("leak registry"), "{error}");
    assert!(killed.load(std::sync::atomic::Ordering::SeqCst));
    assert_kept_then_released(&label, &gate);
}

#[test]
fn a_read_error_on_one_split_stream_fails_the_case_and_still_visits_the_other() {
    let mut process = FakeProcess::new();
    process.exited = true;
    let streams: Vec<Box<dyn std::io::Read + Send>> = vec![
        Box::new(FailingRead(false)),
        Box::new(std::io::Cursor::new(b"err\xff".to_vec())),
    ];
    let supervised = Supervised::adopt_split(
        process,
        streams,
        Duration::from_secs(10),
        (Duration::from_secs(2), Duration::from_secs(1)),
        supervisor::Reader::spawn_bytes,
    )
    .expect("two readers start");
    let label = supervised.label.clone();
    let error = supervised
        .finish_split()
        .expect_err("a read error fails the case");
    assert!(
        error.contains("read error: injected read failure"),
        "{error}"
    );
    assert_eq!(leaked(&label), 0, "the other reader ended and was joined");
}

#[test]
fn the_raw_readers_keep_each_stream_apart_byte_for_byte() {
    let mut process = FakeProcess::new();
    process.exited = true;
    let streams: Vec<Box<dyn std::io::Read + Send>> = vec![
        Box::new(std::io::Cursor::new(vec![b'o', 0xff, 0x00, b'k'])),
        Box::new(std::io::Cursor::new(b"e\xfe".to_vec())),
    ];
    let seen = Supervised::adopt_split(
        process,
        streams,
        Duration::from_secs(10),
        (Duration::from_secs(2), Duration::from_secs(1)),
        supervisor::Reader::spawn_bytes,
    )
    .expect("two readers start")
    .finish_split()
    .expect("a clean finish");
    assert_eq!(seen.stdout, [b'o', 0xff, 0x00, b'k']);
    assert_eq!(seen.stderr, [b'e', 0xfe]);
    assert_eq!(seen.code, Some(0));
}

#[test]
fn an_unwind_through_a_split_supervisor_with_a_failing_kill_reports_without_a_second_panic() {
    let mut process = FakeProcess::new();
    process.kill = Kill::Refused;
    let (supervised, gate) = gated_split(process, Duration::from_secs(10));
    let _open = OpenOnDrop(gate.clone());
    let label = supervised.label.clone();
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _held = supervised;
        panic!("an assertion failed in the case");
    }));
    let payload = unwound.expect_err("the case panicked");
    let message = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .unwrap_or_default();
    assert!(
        message.contains("an assertion failed"),
        "the original failure is the panic, the cleanup failure is only reported: {message}"
    );
    assert_both_kept_then_released(&label, &gate);
}

// ---------------------------------------------------------------------------------------------
// Thread exit: no join that a thread-local destructor can hold (review 3 B2)
// ---------------------------------------------------------------------------------------------

thread_local! {
    /// A thread-local the stream below initialises while the thread reads, whose destructor is held.
    static SLOW: std::cell::RefCell<Option<SlowGuard>> = const { std::cell::RefCell::new(None) };
}

/// A thread-local value whose destructor blocks until the gate opens.
struct SlowGuard(Gate);

impl Drop for SlowGuard {
    fn drop(&mut self) {
        self.0.wait();
    }
}

/// A stream that ends at once, after initialising a thread-local whose destructor is held: the thread's
/// closure has returned and the stream is gone, but the thread has not exited.
struct EofWithSlowThreadLocal(Gate);

impl std::io::Read for EofWithSlowThreadLocal {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        SLOW.with(|slot| *slot.borrow_mut() = Some(SlowGuard(self.0.clone())));
        Ok(0)
    }
}

#[test]
fn thread_local_destructors_run_in_reverse_order_of_registration_on_this_platform() {
    // The ordering the supervisor's exit guard relies on, checked here rather than assumed: a guard
    // registered first runs last, after one registered later.
    use std::sync::Mutex as StdMutex;
    thread_local! {
        static FIRST: std::cell::RefCell<Option<Recorded>> = const { std::cell::RefCell::new(None) };
        static SECOND: std::cell::RefCell<Option<Recorded>> = const { std::cell::RefCell::new(None) };
    }
    struct Recorded(&'static str, Arc<StdMutex<Vec<&'static str>>>);
    impl Drop for Recorded {
        fn drop(&mut self) {
            self.1
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(self.0);
        }
    }
    let order = Arc::new(StdMutex::new(Vec::new()));
    let seen = Arc::clone(&order);
    std::thread::spawn(move || {
        FIRST.with(|slot| {
            *slot.borrow_mut() = Some(Recorded("first registered", Arc::clone(&seen)));
        });
        SECOND.with(|slot| *slot.borrow_mut() = Some(Recorded("second registered", seen)));
    })
    .join()
    .expect("the thread");
    assert_eq!(
        *order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        ["second registered", "first registered"],
        "the guard registered first runs last"
    );
}

#[test]
fn a_reader_whose_thread_local_destructor_is_held_has_not_exited_and_finish_does_not_join() {
    let gate = Gate::default();
    let _open = OpenOnDrop(gate.clone());
    let mut reader = supervisor::Reader::spawn(EofWithSlowThreadLocal(gate.clone()), shared_text())
        .expect("a reader");
    // The read ended and the stream was dropped; the thread-local destructor is held.
    std::thread::sleep(Duration::from_millis(100));
    let started = std::time::Instant::now();
    let error = reader
        .finish(Duration::from_millis(20))
        .expect_err("the thread has not exited, so finish fails and does not join");
    assert!(error.contains("did not exit within"), "{error}");
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "{:?}",
        started.elapsed()
    );
    assert!(reader.is_running(), "and the reader is still owned");
    gate.open();
    reader
        .finish(Duration::from_secs(5))
        .expect("now it exited and was joined");
    assert!(!reader.is_running());
}

#[test]
fn the_owning_supervisor_fails_closed_within_its_grace_while_a_thread_local_destructor_is_held() {
    let gate = Gate::default();
    let _open = OpenOnDrop(gate.clone());
    let text = shared_text();
    let reader = supervisor::Reader::spawn(EofWithSlowThreadLocal(gate.clone()), Arc::clone(&text))
        .expect("a reader");
    let mut process = FakeProcess::new();
    process.exited = true;
    let mut supervised =
        Supervised::from_parts(process, vec![reader], text, Duration::from_secs(10));
    supervised.pipe_grace = Duration::from_millis(20);
    let label = supervised.label.clone();
    std::thread::sleep(Duration::from_millis(100));
    let started = std::time::Instant::now();
    let error = supervised
        .finish()
        .expect_err("a reader that has not exited fails the case");
    assert!(error.contains("did not exit within"), "{error}");
    assert!(error.contains("leak registry"), "{error}");
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "{:?}",
        started.elapsed()
    );
    assert_kept_then_released(&label, &gate);
}
