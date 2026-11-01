//! Tests of the `glfw-vulkan` path that need no window or GPU: the same [`Run`], backend and adapter the
//! production run uses, over a fake [`Stack`], in-process (the order of the creations and releases) and
//! through the failure boundary (a child process re-executing this test binary, whose exit status and log are
//! what the parent looks at), plus the source checks of this path (its `unsafe` calls, the test-only names).
//!
//! The fake stack records every creation and every release in one ordered log, so the tests can say that the
//! loader is released last on a proven end, in reverse creation order on a proven partial setup, and not at
//! all on an unproven one.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ck_window::{Backend, CleanupResult, CreatedWindow, Notifier, WindowDesc};

use super::fake::{self, FakeStack, Log, Script, WakeCase};
use super::{Run, VulkanAdapter, VulkanBackend, run_closure_with};
use crate::cli::RunOptions;
use crate::fakes::{End, PresenterScript};
use crate::windowed::{Plan, Setup};
use crate::{Shared, execute_with, lifecycle, mode};
use ck_render_2d::Gpu2d;

use crate::boundary::supervisor::{Seen, Supervised};

/// A log that keeps its events in a vector.
pub(super) fn vector_log() -> (Log, Arc<Mutex<Vec<String>>>) {
    crate::fakes::vector_recorder()
}

pub(super) fn events(events: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    events
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The index of the first event that equals `wanted`; panics (with the log) when there is none.
pub(super) fn at(log: &[String], wanted: &str) -> usize {
    log.iter()
        .position(|event| event == wanted)
        .unwrap_or_else(|| panic!("{wanted:?} not in {log:?}"))
}

pub(super) fn desc() -> WindowDesc {
    WindowDesc {
        title: "t".to_owned(),
        width: 100,
        height: 80,
        clear: [0.1, 0.2, 0.3],
    }
}

/// Starts the adapter's presenter on the window just created.
#[expect(
    clippy::unit_arg,
    reason = "the fake's window handles are the unit value"
)]
pub(super) fn start_on(
    adapter: &mut VulkanAdapter<FakeStack>,
    created: &CreatedWindow<()>,
) -> Result<(), String> {
    let sink = adapter.new_sink();
    Gpu2d::start(adapter, created.handles, &desc(), created.pixel_size, sink)
}

/// The error of a result that must be one (`expect_err` would need `Debug` on the backend).
fn error_of<T>(result: Result<T, String>) -> String {
    match result {
        Err(error) => error,
        Ok(_) => panic!("expected a failure"),
    }
}

/// A run over a fake stack with this script, and its log.
pub(super) fn run_over(script: Script) -> (Rc<Run<FakeStack>>, Arc<Mutex<Vec<String>>>) {
    let (log, events) = vector_log();
    (
        fake::new_run(
            FakeStack::new(script, log, Notifier::new()),
            Arc::new(|| {}),
        ),
        events,
    )
}

/// The backend and the adapter of a run.
pub(super) fn parts(
    run: &Rc<Run<FakeStack>>,
) -> (
    Result<VulkanBackend<FakeStack>, String>,
    VulkanAdapter<FakeStack>,
) {
    let adapter = VulkanAdapter::new(Rc::clone(run), Arc::new(|| {}));
    (VulkanBackend::open(Rc::clone(run)), adapter)
}

// ---------------------------------------------------------------------------------------------
// In-process: the order of creation and release
// ---------------------------------------------------------------------------------------------

#[test]
fn the_loader_is_loaded_first_and_glfw_is_created_with_it() {
    let (run, log) = run_over(Script::default());
    let (backend, _adapter) = parts(&run);
    assert!(backend.is_ok());
    assert_eq!(events(&log), ["load loader", "create glfw"]);
}

#[test]
fn a_missing_loader_creates_nothing_else() {
    let (run, log) = run_over(Script {
        loader_error: Some("no libvulkan".to_owned()),
        ..Script::default()
    });
    let (backend, _adapter) = parts(&run);
    let error = error_of(backend);
    assert!(
        error.contains("cannot load the Vulkan loader: no libvulkan"),
        "{error}"
    );
    assert_eq!(events(&log), ["load loader"], "no GLFW without the loader");
}

#[test]
fn a_glfw_that_does_not_start_releases_the_loader() {
    let (run, log) = run_over(Script {
        glfw_error: Some("glfwInit failed".to_owned()),
        ..Script::default()
    });
    let (backend, _adapter) = parts(&run);
    assert!(error_of(backend).contains("glfwInit failed"));
    assert_eq!(
        events(&log),
        ["load loader", "create glfw", "loader dropped"]
    );
}

/// A full proven life: the creation and the teardown, driven as the loop does.
fn proven_life(script: Script) -> Arc<Mutex<Vec<String>>> {
    let (run, log) = run_over(script);
    let (backend, mut adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    start_on(&mut adapter, &created).expect("started");
    adapter.request_stop();
    assert_eq!(
        adapter.poll_end(Duration::from_millis(10)),
        Some(CleanupResult::Proven)
    );
    backend.destroy_window();
    backend.terminate();
    drop(backend);
    drop(adapter);
    log
}

#[test]
fn a_proven_teardown_releases_the_presenter_the_surface_the_instance_the_window_and_glfw_then_the_loader()
 {
    let log = events(&proven_life(Script::default()));
    let order = [
        "presenter dropped",
        "destroy helper surface=7 ended=Proven",
        "surface destroyed",
        "instance destroyed",
        "instance dropped",
        "destroy_window",
        "terminate",
        "loader dropped",
    ];
    let positions: Vec<usize> = order.iter().map(|wanted| at(&log, wanted)).collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "the release order: {log:?}"
    );
    assert_eq!(
        log.iter().position(|event| event == "loader dropped"),
        Some(log.len() - 2)
            .filter(|_| false)
            .or_else(|| log.iter().rposition(|event| event == "loader dropped")),
    );
    assert_eq!(
        log.iter()
            .filter(|event| *event == "loader dropped")
            .count(),
        1,
        "the loader is released once: {log:?}"
    );
    assert!(
        at(&log, "loader dropped") > at(&log, "terminate"),
        "the loader after glfwTerminate: {log:?}"
    );
}

#[test]
fn the_loader_is_created_before_everything_and_released_after_everything_else() {
    let log = events(&proven_life(Script::default()));
    assert_eq!(log[0], "load loader", "{log:?}");
    let release = at(&log, "loader dropped");
    for event in [
        "presenter dropped",
        "instance dropped",
        "destroy_window",
        "terminate",
    ] {
        assert!(
            at(&log, event) < release,
            "{event} before the loader: {log:?}"
        );
    }
}

#[test]
fn a_surface_that_cannot_be_created_destroys_the_instance_then_the_window_then_releases_the_loader()
{
    let (run, log) = run_over(Script {
        surface_error: Some("no surface".to_owned()),
        ..Script::default()
    });
    let (backend, _adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let error = backend.create_window(&desc()).expect_err("failed");
    assert!(
        error.contains("cannot create the surface: no surface"),
        "{error}"
    );
    backend.terminate();
    let log = events(&log);
    assert!(
        at(&log, "destroy helper surface=none ended=NoPresenter") < at(&log, "destroy_window"),
        "the instance before the window: {log:?}"
    );
    assert!(
        at(&log, "instance destroyed") < at(&log, "destroy_window"),
        "{log:?}"
    );
    assert!(
        at(&log, "destroy_window") < at(&log, "terminate"),
        "{log:?}"
    );
    assert!(
        at(&log, "terminate") < at(&log, "loader dropped"),
        "{log:?}"
    );
    assert!(run.retained().is_none());
}

#[test]
fn an_instance_that_cannot_be_created_destroys_the_window_then_releases_the_loader() {
    let (run, log) = run_over(Script {
        instance_error: Some("no layer".to_owned()),
        ..Script::default()
    });
    let (backend, _adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let error = backend.create_window(&desc()).expect_err("failed");
    assert!(
        error.contains("cannot create the Vulkan instance: no layer"),
        "{error}"
    );
    backend.terminate();
    let log = events(&log);
    assert!(
        at(&log, "destroy_window") < at(&log, "terminate"),
        "{log:?}"
    );
    assert!(
        at(&log, "terminate") < at(&log, "loader dropped"),
        "{log:?}"
    );
    assert!(!log.contains(&"create surface".to_owned()), "{log:?}");
}

#[test]
fn a_refused_partial_setup_retains_everything_and_releases_nothing() {
    let (run, log) = run_over(Script {
        surface_error: Some("no surface".to_owned()),
        refuse: Some(false),
        ..Script::default()
    });
    let (backend, _adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    assert!(backend.create_window(&desc()).is_err());
    backend.terminate();
    drop(backend);
    let log = events(&log);
    assert!(
        run.retained()
            .is_some_and(|cause| cause.contains("the helper refused")),
        "{:?}",
        run.retained()
    );
    for released in [
        "instance dropped",
        "loader dropped",
        "destroy_window",
        "terminate",
        "backend dropped",
    ] {
        assert!(
            !log.contains(&released.to_owned()),
            "{released} must not happen: {log:?}"
        );
    }
}

/// A started run up to a presenter whose end is `refuse`d by the helper afterwards.
pub(super) fn refused_teardown_for(
    surface_destroyed: bool,
) -> (Rc<Run<FakeStack>>, Arc<Mutex<Vec<String>>>) {
    refused_teardown(surface_destroyed)
}

fn refused_teardown(surface_destroyed: bool) -> (Rc<Run<FakeStack>>, Arc<Mutex<Vec<String>>>) {
    let (run, log) = run_over(Script {
        refuse: Some(surface_destroyed),
        ..Script::default()
    });
    let (backend, mut adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    start_on(&mut adapter, &created).expect("started");
    adapter.request_stop();
    // The refusal is established when the presenter's end is observed, before the loop would close the
    // window: the end is `Unproven`, so the loop destroys nothing and leaks the backend and the presenter.
    assert!(matches!(
        adapter.poll_end(Duration::from_millis(10)),
        Some(CleanupResult::Unproven { reason }) if reason.contains("the helper refused")
    ));
    assert!(matches!(
        adapter.poll_end(Duration::from_millis(10)),
        Some(CleanupResult::Unproven { .. })
    ));
    std::mem::forget(backend);
    std::mem::forget(adapter);
    (run, log)
}

#[test]
fn a_refused_destroy_retains_the_instance_the_window_the_backend_and_the_loader() {
    for surface_destroyed in [true, false] {
        let (run, log) = refused_teardown(surface_destroyed);
        let log = events(&log);
        assert!(
            run.retained()
                .is_some_and(|cause| cause.contains("the helper refused"))
        );
        for released in [
            "instance dropped",
            "loader dropped",
            "destroy_window",
            "terminate",
            "backend dropped",
        ] {
            assert!(
                !log.contains(&released.to_owned()),
                "surface_destroyed={surface_destroyed}: {released} must not happen: {log:?}"
            );
        }
        assert_eq!(
            log.iter()
                .filter(|event| event.starts_with("destroy helper"))
                .count(),
            1,
            "the helper is asked once, so the surface is never destroyed twice: {log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|event| *event == "surface destroyed")
                .count(),
            usize::from(surface_destroyed),
            "{log:?}"
        );
    }
}

#[test]
fn an_unproven_presenter_end_leaves_the_instance_and_the_loader_alone() {
    let (run, log) = run_over(Script {
        presenter: PresenterScript {
            end: End::Unproven("no cleanup result".to_owned()),
            ..PresenterScript::default()
        },
        ..Script::default()
    });
    let (backend, mut adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    start_on(&mut adapter, &created).expect("started");
    adapter.request_stop();
    assert!(matches!(
        adapter.poll_end(Duration::from_millis(10)),
        Some(CleanupResult::Unproven { .. })
    ));
    // The loop calls neither `destroy_window` nor `terminate` after an unproven end, and leaks both objects.
    std::mem::forget(backend);
    std::mem::forget(adapter);
    let log = events(&log);
    for released in [
        "instance dropped",
        "loader dropped",
        "destroy_window",
        "terminate",
    ] {
        assert!(!log.contains(&released.to_owned()), "{released}: {log:?}");
    }
}

#[test]
fn a_presenter_that_failed_to_start_leaves_the_window_to_be_destroyed_with_the_helper_and_no_proof()
{
    let (run, log) = run_over(Script {
        presenter: PresenterScript {
            start_error: Some("no device".to_owned()),
            ..PresenterScript::default()
        },
        ..Script::default()
    });
    let (backend, mut adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    assert!(start_on(&mut adapter, &created).is_err());
    backend.destroy_window();
    backend.terminate();
    let log = events(&log);
    assert!(
        log.contains(&"destroy helper surface=7 ended=NoPresenter".to_owned()),
        "no presenter was started: {log:?}"
    );
    assert!(
        at(&log, "destroy_window") < at(&log, "terminate"),
        "{log:?}"
    );
    assert!(
        at(&log, "terminate") < at(&log, "loader dropped"),
        "{log:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// Through the failure boundary: a child process runs `execute_with` over the fake stack
// ---------------------------------------------------------------------------------------------

/// The case the child runs; absent in the parent and in an ordinary `cargo test`.
const CASE: &str = "CK_VULKAN_BOUNDARY_CASE";
/// The directory with the child's configuration, scripts, markers and ordered log.
const DIR: &str = "CK_VULKAN_BOUNDARY_DIR";
/// The longest a child may run.
const DEADLINE: Duration = Duration::from_secs(60);

/// The script of the case `name`.
fn script_of(name: &str) -> Script {
    let base = Script::default();
    match name {
        "proven_close" => base,
        "loader_missing" => Script {
            loader_error: Some("no libvulkan.1.dylib".to_owned()),
            ..base
        },
        "instance_fails" => Script {
            instance_error: Some("the validation layer is missing".to_owned()),
            ..base
        },
        "surface_fails_proven" => Script {
            surface_error: Some("the surface could not be created".to_owned()),
            ..base
        },
        "surface_fails_unproven" => Script {
            surface_error: Some("the surface could not be created".to_owned()),
            refuse: Some(false),
            ..base
        },
        "presenter_unproven" => Script {
            presenter: PresenterScript {
                end: End::Unproven("the render thread did not send a cleanup result".to_owned()),
                ..PresenterScript::default()
            },
            ..base
        },
        "refused_surface_gone" => Script {
            refuse: Some(true),
            ..base
        },
        "refused_surface_kept" => Script {
            refuse: Some(false),
            ..base
        },
        "refused_surface_gone_panic_in_drain" => Script {
            refuse: Some(true),
            panic_in_drain: true,
            ..base
        },
        "refused_surface_kept_panic_in_drain" => Script {
            refuse: Some(false),
            panic_in_drain: true,
            ..base
        },
        // The wake regressions (CK-93's P3 for this path): raised from another thread while the loop sleeps.
        "wake_report" => Script {
            wake_case: Some(WakeCase::Report),
            ..base
        },
        "wake_refusal" => Script {
            wake_case: Some(WakeCase::Refusal),
            ..base
        },
        // A whole run that sets a quad: no milestone-4 warning, the scene counts, the notice and the
        // first-frame record of the fake presenter.
        "quad_scene" => Script {
            drawn_generation: Some(7),
            notice: Some("R16G16B16A16_SFLOAT".to_owned()),
            ..base
        },
        other => panic!("no such Vulkan boundary case: {other}"),
    }
}

/// A recorder that appends one line per event to `dir/events.log`, in order.
fn file_log(dir: &Path) -> Log {
    let path = dir.join("events.log");
    Arc::new(move |event| {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("the event log");
        writeln!(file, "{event}").expect("a log line");
    })
}

/// Writes the configuration and the two scripts of a case into `dir`.
///
/// With `held` the `on_opened` callback closes the window and then waits for a gate file nobody creates (the
/// script fails after 5 s of CPU time), so the late-callback drain has a call outstanding for its whole bound.
fn write_fixture(dir: &Path, name: &str, held: bool) -> PathBuf {
    // The wake regressions keep the window open (only the raised report or refusal ends the run); the quad case
    // sets a quad before the window opens, from an assets directory holding one image.
    let stay_open = name.starts_with("wake_");
    let quad = name == "quad_scene";
    if quad {
        fs::write(
            dir.join("small.png"),
            crate::testing::png(8, 4, &[255; 8 * 4 * 4]),
        )
        .expect("an image");
    }
    let touch = |name: &str| {
        format!(
            "local f = assert(io.open([==[{}]==], 'w')); f:write('x'); f:close()",
            dir.join(name).display()
        )
    };
    let ui = format!(
        "{}window.open{{title='vulkan boundary', width=100, height=80, clear={{0, 0, 0}}}}\n\
         window.on_close_request(function(source) return false end)\n\
         window.on_opened(function() {}{} end)\n\
         function shutdown() {} end\n",
        if quad {
            "render.quad{image='small.png', x=10, y=5}\n"
        } else {
            ""
        },
        if stay_open { "" } else { "window.close()" },
        if held {
            format!(
                "\n  local started = os.clock()\n  while os.clock() - started < 5 do end\n  {}",
                touch("ui_released")
            )
        } else {
            String::new()
        },
        touch("ui_shutdown")
    );
    let gameplay = format!("function shutdown() {} end\n", touch("gameplay_shutdown"));
    fs::write(dir.join("ui.lua"), ui).expect("the ui script");
    fs::write(dir.join("gameplay.lua"), gameplay).expect("the gameplay script");
    let level = if quad { "level = \"debug\"\n" } else { "" };
    let render = if quad {
        format!("[render]\nassets = '{}'\n\n", dir.display())
    } else {
        String::new()
    };
    let config = format!(
        "[log]\ntimestamps = false\ncolour = \"never\"\n{level}\n[window]\nbackend = \"glfw-vulkan\"\n\n{render}\
         [lua.gameplay]\nentry = '{dir}/gameplay.lua'\n\n[lua.ui]\nentry = '{dir}/ui.lua'\n",
        dir = dir.display()
    );
    let path = dir.join("config.toml");
    fs::write(&path, config).expect("the configuration");
    path
}

/// The child's `main`: the real logger, panic hook and exit path around the real `execute_with`, with the
/// real `run_closure_with` over the fake stack. It never returns.
fn child_main(name: &str, dir: &Path) -> ! {
    let script = script_of(name);
    let config = write_fixture(dir, name, script.panic_in_drain);
    lifecycle::install_logger().expect("the logger");
    lifecycle::install_panic_hook();
    let options = RunOptions {
        config: Some(config),
        headless: false,
        backend: None,
    };
    let shared = Shared {
        quit: Arc::new(AtomicBool::new(false)),
        notifier: Notifier::new(),
    };
    let log = file_log(dir);
    let plan = move |setup: Setup| -> Plan {
        let mut env = setup.env;
        env.settings = ck_window::LoopSettings::new(
            64,
            16,
            Duration::from_millis(400),
            Duration::from_millis(100),
        )
        .expect("settings");
        let notifier = Arc::clone(&env.notifier);
        let run = run_closure_with(
            env,
            FakeStack::new(script, log, notifier),
            &lifecycle::RESOURCES,
            crate::windowed::DRAIN,
        );
        Plan {
            extension: setup.extension,
            run: Some(run),
        }
    };
    let status = execute_with(&options, shared, &[mode::Backend::GlfwVulkan], plan);
    // A wake regression: how long after it raised its report or refusal the run was over.
    let raised = fake::raised_at();
    if raised > 0 {
        let latency = fake::clock().saturating_sub(raised) / 1000;
        fs::write(dir.join("latency_ms"), latency.to_string()).expect("the latency marker");
    }
    // Whether the resource latch is still set when the run is over: the process's immediate exit depends on it.
    fs::write(
        dir.join(if lifecycle::RESOURCES.is_set() {
            "latch_set"
        } else {
            "latch_clear"
        }),
        "x",
    )
    .expect("the latch marker");
    let code = lifecycle::finish_code(status);
    std::process::exit(i32::from(code));
}

/// The child's entry: a no-op in an ordinary test run, the whole child when the parent asks for a case.
#[test]
fn child() {
    let (Some(name), Some(dir)) = (std::env::var_os(CASE), std::env::var_os(DIR)) else {
        return;
    };
    child_main(&name.to_string_lossy(), &PathBuf::from(dir));
}

/// A fresh directory for one case, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "codekarl-vulkan-boundary-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a case directory");
        Self(path)
    }

    fn has(&self, marker: &str) -> bool {
        self.0.join(marker).exists()
    }

    /// The ordered event log of the child.
    fn log(&self) -> Vec<String> {
        fs::read_to_string(self.0.join("events.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs `name` in a fresh child under the supervisor.
fn run(name: &str) -> (Seen, Dir) {
    let dir = Dir::new(name);
    let mut command = Command::new(std::env::current_exe().expect("this executable"));
    command
        .args([
            "--exact",
            "vulkan::tests::child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CASE, name)
        .env(DIR, &dir.0);
    let seen = Supervised::spawn(command, DEADLINE)
        .and_then(Supervised::finish)
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    (seen, dir)
}

/// Nothing the Vulkan path owns was released.
fn assert_all_retained(log: &[String], name: &str) {
    for released in [
        "loader dropped",
        "instance dropped",
        "destroy_window",
        "terminate",
        "backend dropped",
    ] {
        assert!(
            !log.contains(&released.to_owned()),
            "{name}: {released} must not happen: {log:?}"
        );
    }
}

#[test]
fn a_proven_run_releases_the_loader_last_and_exits_0() {
    let (seen, dir) = run("proven_close");
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    let log = dir.log();
    assert_eq!(log[0], "load loader", "{log:?}");
    assert!(at(&log, "presenter dropped") < at(&log, "destroy helper surface=7 ended=Proven"));
    assert!(at(&log, "instance destroyed") < at(&log, "destroy_window"));
    assert!(at(&log, "destroy_window") < at(&log, "terminate"));
    assert!(
        at(&log, "terminate") < at(&log, "loader dropped"),
        "{log:?}"
    );
    assert!(dir.has("ui_shutdown") && dir.has("gameplay_shutdown"));
    assert!(
        dir.has("latch_clear"),
        "everything was released: the latch is clear"
    );
}

#[test]
fn a_missing_loader_is_one_run_failed_record_naming_it_and_exit_1() {
    let (seen, dir) = run("loader_missing");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert!(seen.stdout.contains("run failed:"), "{}", seen.stdout);
    assert!(
        seen.stdout
            .contains("cannot load the Vulkan loader: no libvulkan.1.dylib"),
        "{}",
        seen.stdout
    );
    assert_eq!(dir.log(), ["load loader"], "nothing else was created");
    assert!(dir.has("ui_shutdown") && dir.has("gameplay_shutdown"));
}

#[test]
fn a_proven_partial_setup_destroys_the_instance_then_the_window_then_the_loader_and_exits_1() {
    for name in ["instance_fails", "surface_fails_proven"] {
        let (seen, dir) = run(name);
        assert_eq!(seen.code, Some(1), "{name}: {}", seen.stdout);
        assert!(
            seen.stdout.contains("run failed:"),
            "{name}: {}",
            seen.stdout
        );
        let log = dir.log();
        let cause = if name == "instance_fails" {
            "the validation layer is missing"
        } else {
            "the surface could not be created"
        };
        assert!(seen.stdout.contains(cause), "{name}: {}", seen.stdout);
        if name == "surface_fails_proven" {
            assert!(
                at(&log, "instance destroyed") < at(&log, "destroy_window"),
                "{name}: the instance before the window: {log:?}"
            );
        }
        assert!(
            at(&log, "destroy_window") < at(&log, "terminate"),
            "{name}: {log:?}"
        );
        assert!(
            at(&log, "terminate") < at(&log, "loader dropped"),
            "{name}: reverse order, loader last: {log:?}"
        );
        assert!(
            dir.has("ui_shutdown") && dir.has("gameplay_shutdown"),
            "{name}"
        );
        assert!(
            dir.has("latch_clear"),
            "{name}: everything was released: the latch is clear"
        );
    }
}

#[test]
fn a_partial_setup_whose_destruction_is_refused_exits_3_and_retains_everything() {
    let (seen, dir) = run("surface_fails_unproven");
    assert_eq!(seen.code, Some(3), "{}", seen.stdout);
    assert!(seen.stdout.contains("run failed:"), "{}", seen.stdout);
    assert!(
        seen.stdout.contains("the surface could not be created"),
        "{}",
        seen.stdout
    );
    assert!(
        seen.stdout.contains("the helper refused"),
        "{}",
        seen.stdout
    );
    assert_all_retained(&dir.log(), "surface_fails_unproven");
    assert!(
        dir.has("latch_set"),
        "objects are retained: the latch stays set"
    );
    assert!(
        dir.has("ui_shutdown") && dir.has("gameplay_shutdown"),
        "the stop phase still ran"
    );
}

#[test]
fn an_unproven_presenter_end_retains_the_loader_and_exits_3() {
    let (seen, dir) = run("presenter_unproven");
    assert_eq!(seen.code, Some(3), "{}", seen.stdout);
    assert!(seen.stdout.contains("unproven"), "{}", seen.stdout);
    assert_all_retained(&dir.log(), "presenter_unproven");
    assert!(
        dir.has("latch_set"),
        "objects are retained: the latch stays set"
    );
    assert!(dir.has("ui_shutdown") && dir.has("gameplay_shutdown"));
}

#[test]
fn a_refused_destroy_exits_3_retaining_the_instance_the_window_the_backend_and_the_loader() {
    for (name, surface_destroyed) in [
        ("refused_surface_gone", true),
        ("refused_surface_kept", false),
    ] {
        let (seen, dir) = run(name);
        assert_eq!(seen.code, Some(3), "{name}: {}", seen.stdout);
        assert!(
            seen.stdout.contains("the helper refused"),
            "{name}: {}",
            seen.stdout
        );
        let log = dir.log();
        assert_all_retained(&log, name);
        assert!(
            seen.stdout.contains("window opened") && !seen.stdout.contains("window closed"),
            "{name}: a kept window is never reported closed: {}",
            seen.stdout
        );
        assert!(
            dir.has("latch_set"),
            "{name}: objects are retained: the latch stays set"
        );
        assert_eq!(
            log.iter()
                .filter(|event| event.starts_with("destroy helper"))
                .count(),
            1,
            "{name}: asked once, so the surface is never destroyed twice: {log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|event| *event == "surface destroyed")
                .count(),
            usize::from(surface_destroyed),
            "{name}: {log:?}"
        );
        assert!(
            dir.has("ui_shutdown") && dir.has("gameplay_shutdown"),
            "{name}: the stop phase still ran"
        );
    }
}

#[test]
fn a_panic_during_the_late_drain_after_a_refused_destroy_exits_3_not_101() {
    for name in [
        "refused_surface_gone_panic_in_drain",
        "refused_surface_kept_panic_in_drain",
    ] {
        let (seen, dir) = run(name);
        assert!(
            seen.stdout
                .contains("probe: a panic during the late-callback drain"),
            "{name}: the panic happened: {}",
            seen.stdout
        );
        assert_eq!(
            seen.code,
            Some(3),
            "{name}: the latch stayed set through the drain, so the panic is an incomplete shutdown: {}",
            seen.stdout
        );
        assert_all_retained(&dir.log(), name);
        assert!(
            !seen.stdout.contains("window closed"),
            "{name}: a kept window is never reported closed: {}",
            seen.stdout
        );
        assert!(
            !dir.has("ui_released"),
            "{name}: the process ended while the callback was still held"
        );
    }
}

/// How long after the report or refusal was raised the child's run was over, in milliseconds.
fn latency_of(dir: &Dir) -> u64 {
    fs::read_to_string(dir.0.join("latency_ms"))
        .expect("the case raised something")
        .parse()
        .expect("a number of milliseconds")
}

/// The longest the loop may take to notice a report or refusal raised with the run's own wake. The fake window
/// backend sleeps up to ten seconds for a wake, so a wake that is not the notifier's costs all of them.
const NOTICED_WITHIN: u64 = 3_000;

/// CK-93's P3 for this path, through the production factory `run_closure_with` (not a hand-built adapter): a
/// report the presenter publishes, with the wake `Stack::start` was given called from another thread while the
/// loop sleeps, is taken without any input event, redraw or UI answer.
///
/// Negative control: the adapter hands `Stack::start` a wake that is not the run's (`Arc::new(|| {})` in place of
/// `Arc::clone(&self.wake)`), and the loop sleeps out the fake backend's ten seconds.
#[test]
fn a_report_raised_with_the_wake_the_adapter_was_given_reaches_a_sleeping_loop() {
    let (seen, dir) = run("wake_report");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert!(
        seen.stdout
            .contains("probe: a report raised while the loop sleeps"),
        "{}",
        seen.stdout
    );
    let latency = latency_of(&dir);
    assert!(
        latency < NOTICED_WITHIN,
        "the loop took {latency} ms to notice the report: its wake is not the run's"
    );
}

/// The same for the quad side's refusal, whose wake the factory also builds from the run's notifier.
///
/// Negative control: `quad_side` is built with a wake that is not the run's, and the loop sleeps out the ten
/// seconds.
#[test]
fn a_refusal_from_the_quad_side_reaches_a_sleeping_loop() {
    let (seen, dir) = run("wake_refusal");
    assert_eq!(seen.code, Some(1), "{}", seen.stdout);
    assert!(
        seen.stdout
            .contains("probe: the fake side refused a quad while the loop sleeps"),
        "{}",
        seen.stdout
    );
    let latency = latency_of(&dir);
    assert!(
        latency < NOTICED_WITHIN,
        "the loop took {latency} ms to notice the refusal: its wake is not the run's"
    );
}

/// A whole run that sets a quad, through the real executable code over the fake stack: no milestone-4 warning,
/// the scene counts, the format notice once and the first-frame record with the fake's drawn generation once.
#[test]
fn a_whole_run_that_sets_a_quad_logs_no_warning_the_scene_counts_the_notice_and_the_first_frame() {
    let (seen, dir) = run("quad_scene");
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_eq!(
        seen.stdout.matches("arrive in milestone 4.5").count(),
        0,
        "the milestone-4 warning is gone: {}",
        seen.stdout
    );
    assert!(
        seen.stdout
            .contains("render: 1 scene write(s) accepted, 0 discarded after the run ended"),
        "{}",
        seen.stdout
    );
    assert_eq!(
        seen.stdout
            .matches(
                "render: textured quad skipped: the glfw-vulkan surface format is R16G16B16A16_SFLOAT, not UNORM"
            )
            .count(),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(
        seen.stdout
            .matches("render: first frame drawn, generation 7")
            .count(),
        1,
        "{}",
        seen.stdout
    );
    assert!(dir.has("latch_clear"), "everything was released");
}

// ---------------------------------------------------------------------------------------------
// Source checks
// ---------------------------------------------------------------------------------------------

/// The Rust sources of `codekarl` (its `src` and `tests`), as (path, text).
fn sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("a readable directory") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    walk(&root.join("src"), &mut paths);
    walk(&root.join("tests"), &mut paths);
    paths
        .into_iter()
        .map(|path| {
            let text = fs::read_to_string(&path).expect("a readable source");
            (path, text)
        })
        .collect()
}

/// The text that opens an unsafe block (spelled in two pieces so this file, which the lint scan reads, has none).
const UNSAFE_BLOCK: &str = concat!("unsafe", " {");

/// `text` with its comments and its string, raw string and character literals blanked, so that only code
/// is left to count in (this file spells the word in messages and comments).
fn code_only(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut code = String::new();
    let mut at = 0;
    while at < chars.len() {
        let rest = &chars[at..];
        if rest.starts_with(&['/', '/']) {
            while at < chars.len() && chars[at] != '\n' {
                at += 1;
            }
        } else if rest.starts_with(&['/', '*']) {
            let mut depth = 0;
            while at < chars.len() {
                if chars[at..].starts_with(&['/', '*']) {
                    depth += 1;
                    at += 2;
                } else if chars[at..].starts_with(&['*', '/']) {
                    depth -= 1;
                    at += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    at += 1;
                }
            }
        } else if rest[0] == 'r' && matches!(rest.get(1), Some('"' | '#')) && {
            let hashes = rest[1..].iter().take_while(|c| **c == '#').count();
            rest.get(1 + hashes) == Some(&'"')
        } {
            // A raw string: up to the quote followed by the same number of hashes.
            let hashes = rest[1..].iter().take_while(|c| **c == '#').count();
            let closing: Vec<char> = std::iter::once('"')
                .chain(std::iter::repeat_n('#', hashes))
                .collect();
            at += 2 + hashes;
            while at < chars.len() && !chars[at..].starts_with(&closing) {
                at += 1;
            }
            at += closing.len();
        } else if rest[0] == '"' {
            at += 1;
            while at < chars.len() && chars[at] != '"' {
                at += if chars[at] == '\\' { 2 } else { 1 };
            }
            at += 1;
        } else if rest[0] == '\'' && (rest.get(2) == Some(&'\'') || rest.get(1) == Some(&'\\')) {
            // A character literal ('x' or an escape), not a lifetime.
            at += 1;
            while at < chars.len() && chars[at] != '\'' {
                at += if chars[at] == '\\' { 2 } else { 1 };
            }
            at += 1;
        } else {
            code.push(rest[0]);
            at += 1;
        }
    }
    code
}

/// How many times the word appears in the code of `text` (comments and literals excluded).
fn unsafe_words(text: &str) -> usize {
    code_only(text)
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| *word == "unsafe")
        .count()
}

/// The accepted `unsafe` of `codekarl`, per source (its `src` and `tests`, paths below the crate) and in
/// words: the native path's presenter start, the signal wrappers and their tests, the logger's immediate
/// exit, the boundary tests' liveness probe, the stderr redirection, the real-window tests' `kill` and filter FFI and the
/// test support's `fcntl` calls. The Vulkan integration's five are in `src/vulkan/real.rs`. A source that
/// is not here has none; a new `unsafe` anywhere (this crate's `lib.rs` included) changes a count and
/// fails [`every_unsafe_of_codekarl_is_the_accepted_baseline_plus_the_five_of_the_vulkan_integration`].
const BASELINE: [(&str, usize); 10] = [
    ("src/native.rs", 1),
    ("src/signals.rs", 16),
    ("src/lifecycle.rs", 1),
    ("src/boundary.rs", 2),
    ("src/lib.rs", 1),
    ("src/vulkan/real.rs", 5),
    // The real-window test's `kill`, and the worker's (CK-131): the filter and typist FFI, the signal mask
    // (`pthread_sigmask` and its set operations) and the mask probe, each with a SAFETY comment.
    ("tests/windowed.rs", 14),
    // CK-159: the `glfw-vulkan` worker's, the same operations as the windowed worker's plus the window helpers (miniaturize,
    // deminiaturize, state read, the marked key) and its driver's SIGTERM, each with a SAFETY comment. CK-164 adds the
    // test-only GLFW pump wake after the driver observes the native window restored, with a SAFETY comment.
    ("tests/vulkan_app.rs", 15),
    // CK-132: set_cloexec's F_GETFD and F_SETFD, each with a SAFETY comment.
    ("tests/common/mod.rs", 4),
    // CK-130: the scale worker's filter FFI (`ck_app_t_install_input_filter`, `ck_app_t_dropped_keys`) and
    // signal mask, the same operations as the windowed worker's, each with a SAFETY comment.
    ("tests/scale_lua.rs", 7),
];

#[test]
fn the_vulkan_integration_has_exactly_its_five_unsafe_operations_each_with_its_safety_comment() {
    let real = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/vulkan/real.rs");
    let text = fs::read_to_string(&real).expect("the real stack's source");
    let lines: Vec<&str> = text.lines().collect();
    let blocks: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            let code = line.trim_start();
            !code.starts_with("//") && (code.contains(UNSAFE_BLOCK) || code.contains("= unsafe"))
        })
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        blocks.len(),
        5,
        "exactly five unsafe operations in real.rs: {blocks:?}"
    );
    let expected = [
        "transmute",
        "VulkanLoaderForGlfw::new",
        "create_surface",
        "VulkanPresenter::start",
        "destroy_surface_and_instance",
    ];
    for (block, call) in blocks.iter().zip(expected) {
        let body = lines[*block..lines.len().min(block + 10)].join("\n");
        assert!(
            body.contains(call),
            "the unsafe block at line {} calls {call}:\n{body}",
            block + 1
        );
        // A SAFETY comment above, with only the comment's own lines, attributes and the statement's first
        // line (a `let` rustfmt wrapped before the block) in between.
        let above: Vec<&&str> = lines[..*block]
            .iter()
            .rev()
            .take(14)
            .take_while(|line| {
                !line.trim().is_empty()
                    && !line.contains(UNSAFE_BLOCK)
                    && !line.contains("= unsafe")
            })
            .collect();
        assert!(
            above
                .iter()
                .any(|line| line.trim_start().starts_with("// SAFETY:")),
            "a SAFETY comment above the {call} call: {above:?}"
        );
    }
    // The GLFW backend is always created with the program's loader (never `None`): the machine's dynamic
    // loader may find a Vulkan loader on its own, so a real run cannot tell, but this source can.
    assert!(
        text.contains("GlfwBackend::new(Some(value))") && !text.contains("GlfwBackend::new(None)"),
        "the GLFW backend is created with the loader's function"
    );
}

#[test]
fn every_unsafe_of_codekarl_is_the_accepted_baseline_plus_the_five_of_the_vulkan_integration() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut seen = std::collections::BTreeMap::new();
    for (path, text) in sources() {
        let words = unsafe_words(&text);
        if words > 0 {
            let name = path
                .strip_prefix(root)
                .expect("below the crate")
                .to_string_lossy()
                .into_owned();
            seen.insert(name, words);
        }
    }
    let wanted: std::collections::BTreeMap<String, usize> = BASELINE
        .iter()
        .map(|(name, count)| ((*name).to_owned(), *count))
        .collect();
    assert_eq!(seen, wanted, "the `unsafe` of codekarl, per source");
}

#[test]
fn the_unsafe_counter_sees_code_only_and_catches_an_added_operation() {
    assert_eq!(unsafe_words("fn f() { let x = 1; }"), 0);
    assert_eq!(
        unsafe_words(
            "// unsafe\n/* unsafe /* unsafe */ unsafe */ let s = \"unsafe\"; let r = r#\"un\"safe unsafe\"#; let c = 'u';"
        ),
        0
    );
    assert_eq!(
        unsafe_words(&format!(
            "let v = {UNSAFE_BLOCK} std::ptr::read_volatile(p) }};"
        )),
        1
    );
    assert_eq!(
        unsafe_words("unsafe fn f() {}\nunsafe impl Send for X {}"),
        2
    );
    assert_eq!(unsafe_words("let unsafe_code = 1;"), 0);
}
