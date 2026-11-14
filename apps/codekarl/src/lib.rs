//! The Codekarl program: the runtime command, and a Metal smoke test. The library holds all of it; the `codekarl`
//! executable is a thin `main` that hands [`main_entry`] its arguments (see `src/main.rs`).
//!
//! `codekarl [--config PATH] [--headless | --backend NAME] [--load PATH | --seed N]` loads a TOML configuration (or uses the
//! built-in defaults), sets up logging on stdout, runs the runtime and shuts down. With a configuration
//! that has `[lua.<name>]` tables, the runtime also supervises one Lua VM per table (see `vm`); with
//! none, as with the built-in defaults, no VM runs. `--help`, `--version` and `--metal-smoke` do not start
//! the runtime.
//!
//! **The simulation** (see `game`): with `[game] scenario` the run also has the game, on its own owner thread `sim`. A new game
//! starts paused at the scenario's start date with a master seed from `--seed N` or the operating system; `--load PATH`
//! starts from a save instead. Space pauses, `Digit1` to `Digit5` pick a speed, F5 quicksaves and F9 quickloads. Without
//! `[game] scenario` there is no simulation and every run is as it was.
//!
//! **Windowed or headless** (see `mode`): a run is headless when there is no `--config`, when the
//! configuration has no `[window]` table or one without `backend`, or with `--headless`; otherwise it is
//! windowed with `--backend`'s backend or the configuration's. `native-metal` (`AppKit` and Metal) is
//! available in builds with the `metal` feature on macOS and `glfw-vulkan` (GLFW and Vulkan) in builds with
//! the `vulkan` feature (see `vulkan`); each opens the window the `ui` VM asks for (see `window_api`,
//! `ui_link` and `windowed`), and a backend this build lacks is a configuration error
//! (`backend <name> is not available in this build`, exit 1). One
//! `window: <mode>` record follows the `features:` record. A headless run gives the `ui` VM the same
//! `window` API, which validates the same way and ignores a valid `open` with one record.
//!
//! **Rendering** (see `render_api`): the `ui` VM also has a `render` API (`render.quad`,
//! `render.clear_quad`) that decodes a PNG from the configured assets directory and writes the run's one scene
//! bridge; `native-metal` draws it through `ck-render-2d`'s Metal backend, and `glfw-vulkan` through its own
//! `Gpu2d` backend over `ck-render-2d`'s resource-aware Vulkan sink side and the Vulkan scene presenter (milestones 4.5
//! and 5.5), so the two are interchangeable to the rest of the program: both give the `ui` VM the `texture` and `frame` APIs
//! and the scene calls over a resource service. Only `native` and `vulkan` name a GPU crate.
//!
//! **Signals** (see `signals`): the runtime command blocks SIGINT and SIGTERM before any other thread
//! exists and waits for them on one thread. The first asks for a quit (the runtime's quit flag) and is
//! logged; a second ends the process at once with status 3. A child process started from Lua inherits
//! the blocked mask, so it ignores SIGINT and SIGTERM unless it resets its own mask; in this build a quit
//! during the stop phase changes nothing, since the stop phase is bounded by its deadlines.
//!
//! Standard error follows standard output: the very first thing [`main_entry`] does, before it parses an
//! argument or writes a byte, is make file descriptor 2 a duplicate of file descriptor 1, so
//! everything the process writes to either stream arrives at stdout's destination.
//!
//! # Exit statuses
//!
//! - **0**: a clean run.
//! - **1**: a usage, configuration or runtime failure (a service or a VM failed to start or stop, the
//!   window or its presenter failed, or a `ui` callback failed, also late, after the run ended).
//! - **130** or **143**: a clean run in which SIGINT (130) or SIGTERM (143) asked for the quit, decided
//!   after the final flush; a worse result (3 or 1) wins.
//! - **3**: an incomplete shutdown: the log was not fully written (a stalled or closed stdout, or a
//!   dropped record), or some VM missed a start or stop deadline (even if it later finished) or was
//!   not observed finished, or the window teardown was unproven (the stop phase still runs first), or a
//!   panic on a thread that is not a VM owner while native or GPU objects may be in use. The process
//!   ends at once, without exit handlers.
//! - **4**: stderr could not be routed to stdout.
//!
//! The statuses are documented here, not printed. The control-path rule (every record the main thread
//! or the panic hook emits goes through `log_within`) is in `lifecycle`.
use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use ck_log::Level;
use ck_render_2d::SceneBridge;
use ck_runtime::{Report, Runtime};
use ck_window::{Notifier, Requests};

#[cfg(test)]
mod boundary;
mod cli;
mod config;
#[cfg(test)]
mod demo_tests;
mod economy_soak;
#[cfg(test)]
mod fakes;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the `font` extension of the readout slice loads it"
    )
)]
mod font;
mod frame_api;
mod game;
mod game_api;
mod glossary;
mod input_stash;
mod lifecycle;
mod map_app;
mod map_modes;
mod metrics;
#[cfg_attr(
    all(
        not(test),
        not(all(feature = "metal", target_os = "macos")),
        not(feature = "vulkan")
    ),
    expect(dead_code, reason = "only the windowed runs, and the tests, use this")
)]
mod metrics_record;
#[cfg(all(test, feature = "metal", target_os = "macos"))]
mod metrics_windows_tests;
mod mode;
#[cfg(all(feature = "metal", target_os = "macos"))]
mod native;
#[cfg(test)]
mod readout_tests;
mod render_api;
mod resource_runtime;
#[cfg(test)]
mod scene_api_tests;
mod settings;
mod signals;
#[cfg(test)]
mod testing;
mod texture_api;
mod theme;
mod ui_link;
mod vm;
#[cfg_attr(
    all(not(test), not(feature = "vulkan")),
    expect(
        dead_code,
        reason = "only the glfw-vulkan windowed run, and the tests, use this"
    )
)]
mod vulkan;
#[cfg(test)]
mod widget_tests;
mod window_api;

/// Run the headless economy soak tool from its command-line arguments.
///
/// # Errors
/// Returns a message for invalid arguments, scenario/setup failures, or economy arithmetic refusal.
pub fn economy_soak_main(arguments: impl IntoIterator<Item = String>) -> Result<String, String> {
    economy_soak::run(arguments)
}
#[cfg_attr(
    all(
        not(test),
        not(all(feature = "metal", target_os = "macos")),
        not(feature = "vulkan")
    ),
    expect(dead_code, reason = "only the windowed runs, and the tests, use this")
)]
mod windowed;

use cli::{Command, RunOptions};
use lifecycle::{Status, apply_settings, emit};
use ui_link::{CodekarlUi, UiSlot};
use window_api::HeadlessNotes;
use windowed::{Plan, RunEnv, RunNotes, Setup};

/// The exit status when stderr cannot be routed to stdout.
const ROUTING_FAILED: u8 = 4;

#[cfg(all(feature = "metal", target_os = "macos"))]
fn metal_smoke() -> Result<(), String> {
    native::smoke()
}

#[cfg(not(all(feature = "metal", target_os = "macos")))]
fn metal_smoke() -> Result<(), String> {
    Err("Metal is disabled; build with --features metal".into())
}

/// Makes file descriptor 2 a duplicate of file descriptor 1: the one `unsafe` call of the executable.
fn duplicate_stdout_onto_stderr() -> io::Result<()> {
    // SAFETY: `dup2(1, 2)` names the process's own standard descriptors and touches no memory.
    // Rust's start-up code has already reopened a closed descriptor 0, 1 or 2 as `/dev/null`, so
    // both exist; a failure is reported through the return value, which is checked below.
    let result = unsafe { libc::dup2(1, 2) };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Routes stderr to stdout with `duplicate`. If that fails, the reason is written to `stdout` on a
/// best-effort basis (to `stderr` if that write fails too) and the status to exit with is returned.
fn route_stderr_to_stdout(
    duplicate: impl FnOnce() -> io::Result<()>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), u8> {
    let Err(error) = duplicate() else {
        return Ok(());
    };
    let message = format!("codekarl: cannot route stderr to stdout: {error}\n");
    if stdout
        .write_all(message.as_bytes())
        .and_then(|()| stdout.flush())
        .is_err()
    {
        // Best effort: nowhere is left to report a failure of the fallback itself.
        let _ = stderr.write_all(message.as_bytes());
    }
    Err(ROUTING_FAILED)
}

/// Routes stderr first and runs `command` only if that worked.
fn enter(
    duplicate: impl FnOnce() -> io::Result<()>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    command: impl FnOnce() -> ExitCode,
) -> ExitCode {
    match route_stderr_to_stdout(duplicate, stdout, stderr) {
        Ok(()) => command(),
        Err(status) => ExitCode::from(status),
    }
}

/// The features this binary was built with, for the `features:` record: `none`, or the enabled ones in
/// the order `metal`, `vulkan`, separated by a comma.
const FEATURES: &str = match (cfg!(feature = "metal"), cfg!(feature = "vulkan")) {
    (false, false) => "none",
    (true, false) => "metal",
    (false, true) => "vulkan",
    (true, true) => "metal,vulkan",
};

/// What a failed configuration or start-up ends with: status 1, no owner left behind.
const FAILED: Status = Status {
    code: 1,
    owners_incomplete: false,
};

/// The production plan of a windowed run: the run closure of the backend the mode chose. A backend this build
/// lacks cannot have been chosen (the mode refuses it first), so such a run closure is never asked for.
fn production_plan(setup: Setup) -> Plan {
    // The native path makes its window backend here, on the main thread before the UI VM is spawned, and reads the main screen from it.
    // The Vulkan path makes its backend inside its run (it loads the Vulkan loader first), so it has no snapshot yet: sizes in pixels
    // resolve at one pixel per point until its catch-up slice.
    let (run, screen): (
        Option<windowed::RunClosure>,
        Option<ck_window::ScreenSnapshot>,
    ) = match setup.backend {
        #[cfg(all(feature = "metal", target_os = "macos"))]
        mode::Backend::NativeMetal => {
            let (run, screen) = native::run_closure(setup.env);
            (Some(run), screen)
        }
        #[cfg(feature = "vulkan")]
        mode::Backend::GlfwVulkan => (Some(vulkan::run_closure(setup.env)), None),
        #[cfg(not(all(feature = "metal", target_os = "macos")))]
        mode::Backend::NativeMetal => (None, None),
        #[cfg(not(feature = "vulkan"))]
        mode::Backend::GlfwVulkan => (None, None),
    };
    Plan {
        extension: setup.extension,
        run,
        screen,
    }
}

/// The part of a run that does not change with the mode: the process-wide pieces the signal thread and
/// the window loop share.
pub(crate) struct Shared {
    /// The quit flag the signal thread sets.
    pub(crate) quit: Arc<AtomicBool>,
    /// The notifier every producer wakes.
    pub(crate) notifier: Arc<Notifier>,
}

/// Steps 4 to 6: loads the configuration, chooses windowed or headless, applies the configuration, logs
/// the start and runs the runtime with the configured VMs. Returns how the run ended (see
/// [`vm::status_of`]).
fn execute(options: &RunOptions, shared: Shared) -> Status {
    execute_with(options, shared, mode::AVAILABLE, production_plan)
}

/// [`execute`] with the available backends and the plan of a windowed run supplied: the failure-boundary
/// tests run this very code with fakes. The production binary passes [`mode::AVAILABLE`] and
/// `production_plan`; nothing a user can choose reaches the other arguments.
fn execute_with(
    options: &RunOptions,
    shared: Shared,
    available: &[mode::Backend],
    windowed_plan: impl FnOnce(Setup) -> Plan,
) -> Status {
    let (config, source) = match options.config.as_deref() {
        None => (config::Config::default(), "built-in defaults".to_owned()),
        Some(path) => match config::load(path) {
            Ok(config) => (config, format!("loaded {}", path.display())),
            Err(message) => {
                emit(Level::Error, "config", format_args!("{message}"));
                return FAILED;
            }
        },
    };
    for (given, option) in [
        (options.load.is_some(), "--load"),
        (options.seed.is_some(), "--seed"),
    ] {
        if given && config.game.is_none() {
            emit(
                Level::Error,
                "config",
                format_args!("{}", cli::needs_game(option)),
            );
            return FAILED;
        }
    }
    let mode = match mode::select(
        options.headless,
        options.backend,
        config.window_backend,
        available,
    ) {
        Ok(mode) => mode,
        Err(message) => {
            emit(Level::Error, "config", format_args!("{message}"));
            return FAILED;
        }
    };
    let settings = config.log.settings(
        io::stdout().is_terminal(),
        std::env::var_os("NO_COLOR").as_deref(),
    );
    match settings {
        Ok(settings) => apply_settings(settings),
        Err(message) => {
            emit(Level::Error, "config", format_args!("{message}"));
            return FAILED;
        }
    }
    for warning in &config.settings_warnings {
        emit(Level::Warn, "config", format_args!("{warning}"));
    }
    emit(
        Level::Info,
        "runtime",
        format_args!("codekarl {} starting", env!("CARGO_PKG_VERSION")),
    );
    emit(Level::Info, "config", format_args!("{source}"));
    emit(Level::Info, "runtime", format_args!("features: {FEATURES}"));
    emit(Level::Info, "runtime", format_args!("window: {mode}"));
    supervise(&config, options, mode, shared, windowed_plan)
}

/// What the headless and windowed runs both end with, for the final records and the status.
struct Wiring {
    requests: Requests,
    headless: Option<Arc<HeadlessNotes>>,
    ui: CodekarlUi,
    notes: Rc<RunNotes>,
    bridge: Arc<SceneBridge>,
    render_end: render_api::RenderEnd,
}

/// The window loop's settings: the defaults and the configuration's `[input.bindings]` (both backends' loops map keys
/// through them). A windowed run, **native-metal** or **`glfw-vulkan`**, also gets the configured `[frame]` pacing as
/// its update source, created **off**: no update is posted and no timed wake happens until the control is turned on
/// (the Lua `frame.on_update` does). A headless run has no window loop.
#[cfg(test)]
fn loop_settings(config: &config::Config, mode: mode::Mode) -> ck_window::LoopSettings {
    loop_settings_measured(config, mode, None)
}

/// [`loop_settings`] with the run's measurement window attached to the update source, when there is one.
fn loop_settings_measured(
    config: &config::Config,
    mode: mode::Mode,
    metrics: Option<&metrics::MetricsHub>,
) -> ck_window::LoopSettings {
    let settings = ck_window::LoopSettings::default().with_bindings(config.bindings.clone());
    match mode {
        mode::Mode::Windowed(_) => {
            config
                .frame
                .update_settings()
                .map_or(settings.clone(), |updates| {
                    // The loop's measurement windows count the updates it delivers and the opportunities it coalesces
                    // (only a native-metal run has a measurement hub).
                    let updates = match metrics {
                        Some(hub) => updates.with_metrics(hub.loop_metrics().clone()),
                        None => updates,
                    };
                    settings.with_updates(updates)
                })
        }
        mode::Mode::Headless => settings,
    }
}

/// The run's measurement window (`diagnostics.metrics_mark`): it exists in a windowed run (native-metal or `glfw-vulkan`, each with a
/// presenter to measure), and the loop counts updates when the run has an update source.
fn measurement_hub(config: &config::Config, mode: mode::Mode) -> Option<Arc<metrics::MetricsHub>> {
    matches!(mode, mode::Mode::Windowed(_)).then(|| {
        Arc::new(metrics::MetricsHub::new(
            metrics::uptime_clock(),
            config.frame.update_settings().is_ok(),
        ))
    })
}

fn compose_resources(
    config: &config::Config,
    mode: mode::Mode,
    relay: &ck_render_2d::WakeRelay,
) -> Result<ck_resources::ResourceService, Status> {
    resource_runtime::compose(config.resources, mode, relay, config.render.assets.clone()).map_err(
        |error| {
            emit(
                Level::Error,
                "resources",
                format_args!("cannot create resource service: {error}"),
            );
            FAILED
        },
    )
}

fn scene_bridge(config: &config::Config) -> Arc<SceneBridge> {
    let bridge = SceneBridge::with_limits(
        config.render.filter,
        ck_render_2d::SceneLimits {
            max_quads: config.render.max_quads,
            max_groups: config.render.max_groups,
        },
    )
    .expect("configuration has validated render.max_quads and render.max_groups");
    // `[ui] scale`, read at startup: the limit on the UI scale of a window opened in pixels.
    bridge.set_ui_scale_limit(config.ui_scale_limit_k);
    bridge
}

/// The runtime services of a run: the resource service's loader pool and its teardown, when the mode has a service.
fn runtime_services(
    service: Option<&ck_resources::ResourceService>,
    within: std::time::Duration,
    notes: &Rc<RunNotes>,
    render_end: &render_api::RenderEnd,
) -> Vec<Box<dyn ck_runtime::Service>> {
    service
        .map(|service| {
            Box::new(resource_runtime::RuntimeService::new(
                service.clone(),
                within,
                Rc::clone(notes),
                render_end.clone(),
            )) as Box<dyn ck_runtime::Service>
        })
        .into_iter()
        .collect()
}

/// The `render`, `texture` and `frame` extensions of the UI VM for `mode`, and the end of the render extension's writer.
#[expect(
    clippy::too_many_arguments,
    reason = "the run's wiring passes each dependency explicitly, as `supervise` does"
)]
fn scene_extensions(
    config: &config::Config,
    mode: mode::Mode,
    writer: ck_render_2d::SceneWriter,
    headless: Option<Arc<HeadlessNotes>>,
    service: Option<ck_resources::ResourceService>,
    window_loop_settings: &ck_window::LoopSettings,
    metrics: Option<Arc<metrics::MetricsHub>>,
    registration: frame_api::FrameRegistration,
) -> (Vec<ck_lua::Extension>, render_api::RenderEnd) {
    // The legacy `render.quad` goes through the compatibility lane only on an upgraded backend: both windowed backends are
    // (`native.rs` draws on `MetalBackend::with_resources` and `vulkan.rs` on `VulkanSinkSide::with_resources`, so CK-119's
    // gate applies); headless keeps M4's path.
    let upgraded = matches!(mode, mode::Mode::Windowed(_));
    let (render, render_end) = render_api::extension_with_end(
        config.render.assets.clone(),
        writer,
        headless,
        service.clone(),
        upgraded,
    );
    let texture = texture_api::extension_noting(
        service,
        metrics.as_ref().map(metrics::MetricsHub::load_notes),
    );
    let frame = frame_api::extension(
        config.map.is_some(),
        match mode {
            mode::Mode::Windowed(_) => window_loop_settings
                .updates()
                .map_or(frame_api::FrameBackend::Headless, |updates| {
                    frame_api::FrameBackend::Control(updates.control())
                }),
            mode::Mode::Headless => frame_api::FrameBackend::Headless,
        },
        registration,
    );
    // `diagnostics.metrics_mark`: the mark in a windowed run (either backend), accepted and ignored in a headless one.
    let diagnostics = metrics::extension(metrics);
    (vec![render, texture, frame, diagnostics], render_end)
}

/// Builds the request slot, the UI wiring and the run closure for `mode`, runs the runtime and decides the
/// status (the late-failure fold and the final counts included).
fn supervise(
    config: &config::Config,
    options: &RunOptions,
    mode: mode::Mode,
    shared: Shared,
    windowed_plan: impl FnOnce(Setup) -> Plan,
) -> Status {
    let (loaded_map, game) = match &config.game {
        Some(game_config) => {
            let request = game::setup::Request {
                game: game_config,
                vms: &config.vms,
                load: options.load.as_deref(),
                seed: options.seed,
            };
            let prepared = game::setup::prepare(
                &request,
                game::setup::operating_system_seed,
                &mut |level, message| emit(level, "sim", format_args!("{message}")),
            );
            match prepared {
                Ok(prepared) => (Some(prepared.scenario.map.clone()), Some(prepared)),
                Err(message) => {
                    emit(Level::Error, "config", format_args!("{message}"));
                    return FAILED;
                }
            }
        }
        None => match &config.map {
            Some(map) => match ck_map::load_map(&map.directory) {
                Ok(loaded) => (Some(loaded), None),
                Err(error) => {
                    emit(Level::Error, "config", format_args!("[map] {error}"));
                    return FAILED;
                }
            },
            None => (None, None),
        },
    };
    // The pixel font is read and checked now, so that an invalid one stops the run before anything starts. With a game it is
    // required (a missing font is a refusal too). Without one, a `ui` VM with `[render] assets` gets it when
    // `<assets>/ui/font/font.toml` exists, and no `font` extension when it does not. `game` stays scenario-only.
    let font = match pixel_font(config) {
        Ok(font) => font,
        Err(message) => {
            emit(Level::Error, "config", format_args!("{message}"));
            return FAILED;
        }
    };
    // The theme is read and checked the same way, with the same rule (`<assets>/ui/theme.toml`).
    let theme = match ui_theme(config) {
        Ok(theme) => theme,
        Err(message) => {
            emit(Level::Error, "config", format_args!("{message}"));
            return FAILED;
        }
    };
    let glossary = match ui_glossary(config, font.as_deref()) {
        Ok(glossary) => glossary,
        Err(message) => {
            emit(Level::Error, "config", format_args!("{message}"));
            return FAILED;
        }
    };
    supervise_with_owner_factory(
        config,
        mode,
        shared,
        Content {
            map: loaded_map,
            game,
            font,
            theme,
            glossary,
        },
        windowed_plan,
        production_headless_plan,
        |wiring, _, _| vm::spawns(&config.vms, &config.whole, Some(wiring)),
    )
}

/// The three pixel fonts a run loads at start-up: required with a game, optional for a `ui` VM with an assets directory.
///
/// # Errors
/// The message of a refusal: with a game, a missing or invalid font; without one, an invalid font.
fn pixel_font(config: &config::Config) -> Result<Option<Arc<font::FontSet>>, String> {
    if config.game.is_some() {
        let assets = config.render.assets.clone().unwrap_or_default();
        return font::load_all(&assets)
            .map(|font| Some(Arc::new(font)))
            .map_err(|message| format!("[game] font: {message}"));
    }
    let has_ui = config.vms.iter().any(|vm| vm.name == vm::UI_VM);
    let Some(assets) = config.render.assets.as_ref().filter(|_| has_ui) else {
        return Ok(None);
    };
    if !assets.join("ui/font/font.toml").is_file() {
        return Ok(None);
    }
    font::load_all(assets)
        .map(|font| Some(Arc::new(font)))
        .map_err(|message| format!("[ui] font: {message}"))
}

/// The theme a run loads at start-up, under the font's rule: required with a game, optional for a `ui` VM with an assets directory
/// (loaded when `<assets>/ui/theme.toml` exists).
///
/// # Errors
/// The message of a refusal: with a game, a missing or invalid theme; without one, an invalid theme.
fn ui_theme(config: &config::Config) -> Result<Option<Arc<theme::Theme>>, String> {
    if config.game.is_some() {
        let assets = config.render.assets.clone().unwrap_or_default();
        return theme::load(&assets)
            .map(|theme| Some(Arc::new(theme)))
            .map_err(|message| format!("[game] theme: {message}"));
    }
    let has_ui = config.vms.iter().any(|vm| vm.name == vm::UI_VM);
    let Some(assets) = config.render.assets.as_ref().filter(|_| has_ui) else {
        return Ok(None);
    };
    if !assets.join("ui/theme.toml").is_file() {
        return Ok(None);
    }
    theme::load(assets)
        .map(|theme| Some(Arc::new(theme)))
        .map_err(|message| format!("[ui] theme: {message}"))
}

/// The optional UI glossary, validated against the loaded small font when an asset file is present.
fn ui_glossary(
    config: &config::Config,
    fonts: Option<&font::FontSet>,
) -> Result<Option<Arc<glossary::Glossary>>, String> {
    let has_ui = config.vms.iter().any(|vm| vm.name == vm::UI_VM);
    let Some(assets) = config
        .render
        .assets
        .as_ref()
        .filter(|_| config.game.is_some() || has_ui)
    else {
        return Ok(None);
    };
    let path = assets.join("ui/glossary.toml");
    if !path.is_file() {
        return Ok(None);
    }
    let fonts = fonts.ok_or_else(|| {
        format!(
            "[ui] glossary requires the small UI font: {}",
            path.display()
        )
    })?;
    glossary::load(assets, fonts)
        .map(|glossary| Some(Arc::new(glossary)))
        .map_err(|message| format!("[ui] glossary: {message}"))
}

/// What a run loads before it starts: the map, and the game when the configuration has one (its map is the scenario's).
#[derive(Default)]
struct Content {
    map: Option<ck_map::LoadedMap>,
    game: Option<game::setup::Prepared>,
    /// The standard, small and tiny pixel fonts, loaded with a game or when a `ui` VM's assets hold `ui/font/font.toml`.
    font: Option<Arc<font::FontSet>>,
    /// The theme, loaded under the same rule from `ui/theme.toml`.
    theme: Option<Arc<theme::Theme>>,
    /// The optional glossary, checked against the small font and given to the UI VM.
    glossary: Option<Arc<glossary::Glossary>>,
}

/// What a headless run does between start-up and the stop phase: nothing. The request slot is ended where the run phase
/// would have started, so the window calls return at once and the run finishes.
fn production_headless_plan(setup: Setup) -> Plan {
    Plan {
        run: Some(windowed::headless_run(setup.env.reader)),
        extension: setup.extension,
        screen: None,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the run's composition reads best in the order it is built"
)]
fn supervise_with_owner_factory(
    config: &config::Config,
    mode: mode::Mode,
    shared: Shared,
    content: Content,
    windowed_plan: impl FnOnce(Setup) -> Plan,
    headless_plan: impl FnOnce(Setup) -> Plan,
    owner_factory: impl FnOnce(
        &vm::UiWiring,
        Option<&ck_resources::ResourceService>,
        &render_api::RenderEnd,
    ) -> Vec<ck_runtime::OwnerSpawn>,
) -> Status {
    let Content {
        map: loaded_map,
        game,
        font,
        theme,
        glossary,
    } = content;
    let requests = Requests::new();
    let headless = matches!(mode, mode::Mode::Headless).then(|| Arc::new(HeadlessNotes::logging()));
    let slot = UiSlot::default();
    let notes = Rc::new(RunNotes::default());
    // One bridge per run, with the configured filter. Its one writer goes to the `render` extension. The end of
    // the bridge is the application's to declare (below): a late write is discarded and counted in every kind of
    // run, a headless one included, where no renderer ever attaches.
    let bridge = scene_bridge(config);
    let map_controller = match (&loaded_map, &game, &config.map) {
        (Some(_), Some(prepared), Some(map)) => match map_app::MapController::from_scenario(
            &prepared.scenario,
            Arc::clone(&bridge),
            map.pan_pixels_per_frame,
            map.zoom_steps_per_second,
            map.scroll_points_per_step,
        ) {
            Ok(controller) => Some(map_app::shared(controller)),
            Err(message) => {
                emit(Level::Error, "config", format_args!("{message}"));
                return FAILED;
            }
        },
        (Some(loaded), None, Some(map)) => match map_app::MapController::new(
            loaded,
            Arc::clone(&bridge),
            map.pan_pixels_per_frame,
            map.zoom_steps_per_second,
            map.scroll_points_per_step,
        ) {
            Ok(controller) => Some(map_app::shared(controller)),
            Err(message) => {
                emit(Level::Error, "config", format_args!("{message}"));
                return FAILED;
            }
        },
        (None, _, _) => None,
        (Some(_), _, None) => unreachable!("map loading follows map configuration"),
    };
    let wake = {
        let notifier = Arc::clone(&shared.notifier);
        Arc::new(move || notifier.wake()) as Arc<dyn Fn() + Send + Sync>
    };
    // The simulation owner, when the configuration has a game: the UI link carries the time presses to it and watches for its
    // terminal failure, and the runtime spawns it after every VM.
    let places = game
        .as_ref()
        .map(|prepared| game_api::places_of(&prepared.scenario))
        .unwrap_or_default();
    let connected = game.map(|prepared| game::setup::connect(prepared, Arc::clone(&wake)));
    // The UI VM's `frame.on_update` registration, published by the setter's observer and read by the input tick.
    let frame_registration = frame_api::FrameRegistration::default();
    let ui = map_controller
        .as_ref()
        .map_or_else(
            || CodekarlUi::new(Rc::clone(&slot)),
            |map| CodekarlUi::with_map(Rc::clone(&slot), Arc::clone(map)),
        )
        .with_scene_bridge(Arc::clone(&bridge))
        .with_registration(frame_registration.clone());
    let gameplay_wiring = connected
        .as_ref()
        .and_then(|connected| connected.gameplay.clone());
    let time_registered = Arc::new(AtomicBool::new(false));
    let hover_registered = Arc::new(AtomicBool::new(false));
    let select_registered = Arc::new(AtomicBool::new(false));
    let view_cache = Arc::new(game_api::ViewCache::default());
    // The map modes (the UI's two buttons choose one): their values come from each view the UI link hands the VM.
    let map_modes = Arc::new(map_modes::MapModes::new(Arc::clone(&bridge), places.len()));
    view_cache.observe_with({
        let modes = Arc::clone(&map_modes);
        Arc::new(move |revision, view| modes.observe(revision, view))
    });
    let (ui, sim_spawn, game_extension) = match connected {
        Some(connected) => (
            ui.with_sim(ui_link::SimLink {
                controls: connected.controls.clone(),
                slot: connected.slot,
                time_registered: Arc::clone(&time_registered),
                hover_registered: Arc::clone(&hover_registered),
                select_registered: Arc::clone(&select_registered),
                views: Arc::clone(&view_cache),
            }),
            Some(connected.spawn),
            Some(
                game_api::extension_with_select(
                    time_registered,
                    hover_registered,
                    select_registered,
                    Arc::clone(&wake),
                    connected.controls,
                )
                .function("places", game_api::places_function(places))
                .function("place", game_api::place_function(view_cache))
                .function("set_map_mode", game_api::set_map_mode_function(map_modes)),
            ),
        ),
        None => (ui, None, None),
    };
    let writer = bridge
        .take_writer()
        .expect("a new bridge hands out its one writer");
    let wake_relay = ck_render_2d::WakeRelay::default();
    // Every run has a resource service now; the downstream seams keep an `Option` for the tests that run without one.
    let resource_service = match compose_resources(config, mode, &wake_relay) {
        Ok(service) => Some(service),
        Err(status) => return status,
    };
    let metrics = measurement_hub(config, mode);
    let window_loop_settings = loop_settings_measured(config, mode, metrics.as_deref());
    let (mut ui_extensions, render_end) = scene_extensions(
        config,
        mode,
        writer,
        headless.clone(),
        resource_service.clone(),
        &window_loop_settings,
        metrics.clone(),
        frame_registration,
    );
    // `game.on_time` and `game.on_hover` (the UI VM's view of the simulation and map, with a game only) and `font.layout` (the pixel
    // font: with a game, or when the assets hold one).
    ui_extensions.extend(game_extension);
    ui_extensions.extend(font.map(font::extension));
    ui_extensions.extend(theme.map(theme::extension));
    ui_extensions.extend(glossary.map(glossary::extension));
    let screen = window_api::ScreenCell::default();
    let settings_requests = requests.writer(Arc::clone(&shared.notifier));
    let resize = window_api::ResizeSession::new(
        settings_requests.clone(),
        Arc::clone(&screen),
        headless.clone(),
    );
    let update_control = window_loop_settings
        .updates()
        .map(ck_window::UpdateSettings::control);
    if let Some(settings) = settings::Runtime::new_live(
        config,
        map_controller.clone(),
        settings_requests,
        Arc::clone(&bridge),
        update_control,
        resize,
    ) {
        ui_extensions.push(settings.extension());
    } else {
        ui_extensions.push(config.tooltips.extension());
    }
    // The main screen: filled below from the plan (made on this thread, before the UI VM is spawned), read by `window.open`.
    let setup = Setup {
        // Replaced by the mode's backend for a windowed run (a headless run has no backend).
        backend: mode::Backend::NativeMetal,
        extension: window_api::extension_with_screen(
            requests.writer(Arc::clone(&shared.notifier)),
            headless.clone(),
            map_controller.clone(),
            Arc::clone(&screen),
            config.ui_scale_limit_k,
        ),
        env: RunEnv {
            ui: ui.clone(),
            reader: requests.reader(),
            notifier: Arc::clone(&shared.notifier),
            quit: Arc::clone(&shared.quit),
            settings: window_loop_settings,
            metrics,
            notes: Rc::clone(&notes),
            bridge: Arc::clone(&bridge),
            wake_relay: wake_relay.clone(),
            resources: resource_service.clone(),
            resource_config: config.resources,
            max_quads: usize::try_from(config.render.max_quads).unwrap_or(usize::MAX),
            bindless_texture_slots: config.render.bindless_texture_slots,
        },
    };
    let plan = match mode {
        mode::Mode::Headless => headless_plan(setup),
        mode::Mode::Windowed(backend) => windowed_plan(Setup { backend, ..setup }),
    };
    if let Some(snapshot) = plan.screen {
        let _ = screen.set(snapshot);
    }
    let wiring = vm::UiWiring {
        extensions: std::iter::once(plan.extension)
            .chain(ui_extensions)
            .collect(),
        slot,
        wake: Arc::clone(&wake),
        bridge: Arc::clone(&bridge),
        notes: Rc::clone(&notes),
        on_complete: Some(render_end.completion_hook()),
        gameplay: gameplay_wiring,
    };
    let deadlines = config.deadlines;
    let mut runtime = Runtime::new(runtime_services(
        resource_service.as_ref(),
        config.deadlines.stop,
        &notes,
        &render_end,
    ))
    .with_owners(
        owner_factory(&wiring, resource_service.as_ref(), &render_end)
            .into_iter()
            // After every configured VM: its stop request is the first one sent.
            .chain(sim_spawn)
            .collect(),
    )
    .with_owner_deadlines(deadlines.start, deadlines.stop, deadlines.cleanup)
    .with_quit(shared.quit);
    if let Some(run) = plan.run {
        // Whatever the run closure did (a headless one has no renderer at all), the bridge is ended when the run
        // phase is over and before the stop phase runs the VMs' `shutdown()`. Idempotent: `run_window` ends it
        // itself as soon as the loop returns, before it drains late callbacks.
        let ended = Arc::clone(&bridge);
        let close_resources = resource_service.clone();
        let run: windowed::RunClosure = Box::new(move |context: &ck_runtime::RunContext| {
            let end = run(context);
            ended.detach();
            if let Some(resources) = close_resources {
                resources.close();
            }
            end
        });
        runtime = runtime.with_run(run);
    }
    let report = runtime.run_report();
    let wiring = Wiring {
        requests,
        headless,
        ui,
        notes,
        bridge,
        render_end,
    };
    conclude_run(&report, &wiring)
}

/// After the stop phase: the final `debug` counts, the late callback failures (every remaining call result,
/// taken without waiting) and the status.
fn conclude_run(report: &Report, wiring: &Wiring) -> Status {
    wiring.render_end.end();
    if let Some(text) = wiring
        .headless
        .as_ref()
        .and_then(|notes| notes.final_count())
    {
        emit(Level::Debug, "window", format_args!("{text}"));
    }
    // A windowed run's scene writes: how many the bridge took, and how many came after the renderer let go
    // (a late decode, or `shutdown()`), which it discarded. A headless run's are in the record above.
    let scene = wiring.bridge.counts();
    if scene.accepted > 0 {
        emit(
            Level::Debug,
            "render",
            format_args!(
                "{} scene write(s) accepted, {} discarded after the run ended",
                scene.accepted, scene.discarded
            ),
        );
    }
    let discarded = wiring.requests.discarded();
    if discarded > 0 {
        emit(
            Level::Debug,
            "window",
            format_args!("{discarded} window request(s) discarded after the run ended"),
        );
    }
    // Every remaining call result, never waiting. A callback failure is logged with its message and
    // traceback exactly once: the first as the run's failure when no run failure was logged, every other
    // one (and any when an unrelated run failure was already logged) as its own record, so an earlier
    // presenter or setup failure never hides it. A failure the loop or the drain already took is not
    // here: each result is taken from the link once.
    let late = wiring.ui.take_remaining_failures();
    let run_failure_logged = matches!(report.outcome, ck_runtime::Outcome::RunFailed { .. });
    for (index, failure) in late.iter().enumerate() {
        if index == 0 && !run_failure_logged {
            emit(
                Level::Error,
                "runtime",
                format_args!("run failed: {failure}"),
            );
        } else {
            windowed::log_late_failure(failure);
        }
    }
    vm::status_of(
        report,
        wiring.notes.unproven.get() || wiring.notes.resources_incomplete.get(),
        !late.is_empty(),
    )
}

/// Steps 3 to 7: the signals, the logger, the panic hook, the work and the exit.
fn run_runtime(options: &RunOptions) -> ExitCode {
    // Before the logger's writer or any other thread exists, so that every thread inherits the mask.
    let notifier = Notifier::new();
    let quit = match signals::install(Arc::clone(&notifier)) {
        Ok(quit) => quit,
        Err(error) => {
            eprintln!("codekarl: cannot start the signal thread: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = lifecycle::install_logger() {
        eprintln!("codekarl: cannot start the logger: {error}");
        return ExitCode::FAILURE;
    }
    lifecycle::install_panic_hook();
    lifecycle::finish(execute(options, Shared { quit, notifier }))
}

/// Parses `args` (the program's arguments without its name) and runs the command they ask for.
fn command(args: &[OsString]) -> ExitCode {
    match cli::parse(args) {
        Ok(Command::Help) => {
            println!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("codekarl {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Command::MetalSmoke) => match metal_smoke() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("codekarl: {error}");
                ExitCode::FAILURE
            }
        },
        Ok(Command::Run(options)) => run_runtime(&options),
        Err(message) => {
            eprintln!("codekarl: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The program, given its arguments **without** the program name (what `std::env::args_os().skip(1)` yields).
///
/// The `codekarl` executable and the windowed test executable use this; it is not a general API. The library
/// never reads the process arguments itself, and nothing in it knows about keyboard filtering: the shipped
/// binary is this code with no feature, flag, setting or environment switch.
#[must_use]
pub fn main_entry(args: Vec<OsString>) -> ExitCode {
    enter(
        duplicate_stdout_onto_stderr,
        &mut io::stdout(),
        &mut io::stderr(),
        move || command(&args),
    )
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use ck_runtime::{Owner, OwnerError, OwnerSpawn, Progress};

    use super::*;

    /// A writer that fails every write.
    struct Broken;

    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    fn failing(code: i32) -> impl FnOnce() -> io::Result<()> {
        move || Err(io::Error::from_raw_os_error(code))
    }

    fn native() -> mode::Mode {
        mode::Mode::Windowed(mode::Backend::NativeMetal)
    }

    fn vulkan() -> mode::Mode {
        mode::Mode::Windowed(mode::Backend::GlfwVulkan)
    }

    #[test]
    fn a_bad_configured_map_is_a_startup_error_before_the_window_plan() {
        let missing = testing::Dir::new().0.join("missing-map");
        let config = config::load_text_for_test(&format!("[map]\ndirectory = {missing:?}\n"));
        let status = supervise(
            &config,
            &RunOptions {
                config: None,
                headless: true,
                backend: None,
                load: None,
                seed: None,
            },
            mode::Mode::Headless,
            Shared {
                quit: Arc::new(AtomicBool::new(false)),
                notifier: Notifier::new(),
            },
            |_| panic!("a failed map load must stop before any window plan"),
        );
        assert_eq!(status.code, FAILED.code);
    }

    /// A configuration with a `ui` VM running `script` (no `[game]`), and assets in `dir`; `font` is the text of
    /// `ui/font/font.toml`, if the case has one.
    fn ui_without_game(dir: &testing::Dir, script: &str, font: Option<&str>) -> config::Config {
        let assets = dir.0.join("assets");
        std::fs::create_dir_all(assets.join("ui/font")).expect("the assets directory");
        if let Some(font) = font {
            std::fs::write(assets.join("ui/font/font.toml"), font).expect("the font file");
            let shipped =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui/font");
            for name in ["small.toml", "tiny.toml"] {
                std::fs::copy(shipped.join(name), assets.join("ui/font").join(name))
                    .expect("the named font file");
            }
        }
        let entry = dir.0.join("ui.lua");
        std::fs::write(&entry, script).expect("the ui script");
        config::load_text_for_test(&format!(
            "[lua.ui]\nentry = {entry:?}\n[render]\nassets = {assets:?}\n"
        ))
    }

    fn shipped_font() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui/font/font.toml"),
        )
        .expect("the shipped font")
    }

    fn headless(config: &config::Config) -> Status {
        supervise(
            config,
            &RunOptions {
                config: None,
                headless: true,
                backend: None,
                load: None,
                seed: None,
            },
            mode::Mode::Headless,
            Shared {
                quit: Arc::new(AtomicBool::new(false)),
                notifier: Notifier::new(),
            },
            |_| panic!("a headless run has no window plan"),
        )
    }

    #[test]
    fn a_ui_vm_without_a_game_gets_the_font_when_the_assets_hold_one() {
        let dir = testing::Dir::new();
        let config = ui_without_game(
            &dir,
            "assert(font ~= nil and font.layout('a').width > 0, 'the font extension')\n\
             assert(game == nil, 'game stays scenario-only')",
            Some(&shipped_font()),
        );
        assert!(config.game.is_none());
        assert!(pixel_font(&config).expect("a valid font").is_some());
        assert_eq!(headless(&config).code, 0);
    }

    #[test]
    fn a_ui_vm_without_a_game_and_without_a_font_file_gets_no_font_extension() {
        let dir = testing::Dir::new();
        let config = ui_without_game(&dir, "assert(font == nil and game == nil)", None);
        assert!(
            pixel_font(&config)
                .expect("no font is not an error")
                .is_none()
        );
        assert_eq!(headless(&config).code, 0);
    }

    #[test]
    fn an_invalid_font_without_a_game_is_a_startup_refusal_before_any_vm_starts() {
        let dir = testing::Dir::new();
        let config = ui_without_game(
            &dir,
            "error('the ui script must not run')",
            Some("not a font"),
        );
        let message = pixel_font(&config).expect_err("an invalid font");
        assert!(
            message.starts_with("[ui] font: ") && message.contains("font.toml"),
            "{message}"
        );
        assert_eq!(headless(&config).code, FAILED.code);
    }

    #[test]
    fn a_run_without_a_ui_vm_loads_no_font_and_a_game_still_requires_one() {
        let dir = testing::Dir::new();
        let assets = dir.0.join("assets");
        std::fs::create_dir_all(assets.join("ui/font")).expect("the assets directory");
        std::fs::write(assets.join("ui/font/font.toml"), "not a font").expect("the font file");
        let config = config::load_text_for_test(&format!("[render]\nassets = {assets:?}\n"));
        assert!(pixel_font(&config).expect("no ui VM, no font").is_none());
        // With a game a missing font is refused (as before).
        let empty = dir.0.join("empty");
        std::fs::create_dir_all(&empty).expect("an empty assets directory");
        let config = config::load_text_for_test(&format!(
            "[game]\nscenario = \"s\"\n[render]\nassets = {empty:?}\n"
        ));
        let message = pixel_font(&config).expect_err("a game needs the font");
        assert!(message.starts_with("[game] font: "), "{message}");
    }

    fn shipped_theme() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui/theme.toml"),
        )
        .expect("the shipped theme")
    }

    /// Writes `text` as the assets' `ui/theme.toml` (the directory `ui_without_game` builds its configuration on).
    fn put_theme(dir: &testing::Dir, text: &str) {
        let assets = dir.0.join("assets");
        std::fs::create_dir_all(assets.join("ui")).expect("the assets directory");
        std::fs::write(assets.join("ui/theme.toml"), text).expect("the theme file");
    }

    #[test]
    fn a_ui_vm_without_a_game_gets_the_theme_when_the_assets_hold_one() {
        let dir = testing::Dir::new();
        put_theme(&dir, &shipped_theme());
        let config = ui_without_game(
            &dir,
            "assert(theme ~= nil, 'the theme extension')\n\
             assert(theme.colour('body') == '#15110a', 'a colour')\n\
             assert(theme.kind('place').divider == '#c9932e', 'the place divider')\n\
             local kind = theme.kind('place')\n\
             kind.divider = '#000000'\n\
             assert(theme.kind('place').divider == '#c9932e', 'a changed result changes nothing else')\n\
             assert(not pcall(theme.kind, 'nope') and not pcall(theme.colour, 3), 'errors')",
            None,
        );
        assert!(ui_theme(&config).expect("a valid theme").is_some());
        assert_eq!(headless(&config).code, 0);
    }

    #[test]
    fn a_ui_vm_without_a_game_and_without_a_theme_file_gets_no_theme_extension() {
        let dir = testing::Dir::new();
        let config = ui_without_game(&dir, "assert(theme == nil)", None);
        assert!(
            ui_theme(&config)
                .expect("no theme is not an error")
                .is_none()
        );
        assert_eq!(headless(&config).code, 0);
    }

    #[test]
    fn an_invalid_theme_is_a_startup_refusal_naming_the_table_and_key() {
        let dir = testing::Dir::new();
        put_theme(&dir, "[kind.a]\noutline = \"#000000\"\n");
        let config = ui_without_game(&dir, "error('the ui script must not run')", None);
        let message = ui_theme(&config).expect_err("an invalid theme");
        assert!(
            message.starts_with("[ui] theme: ") && message.ends_with("[kind.a]: missing `hot`"),
            "{message}"
        );
        assert_eq!(headless(&config).code, FAILED.code);
    }

    #[test]
    fn a_game_requires_the_theme_and_refuses_an_invalid_one() {
        let dir = testing::Dir::new();
        let empty = dir.0.join("empty");
        std::fs::create_dir_all(&empty).expect("an empty assets directory");
        let config = config::load_text_for_test(&format!(
            "[game]\nscenario = \"s\"\n[render]\nassets = {empty:?}\n"
        ));
        let message = ui_theme(&config).expect_err("a game needs the theme");
        assert!(
            message.starts_with("[game] theme: cannot read ") && message.contains("theme.toml"),
            "{message}"
        );
        std::fs::create_dir_all(empty.join("ui")).expect("the ui directory");
        std::fs::write(empty.join("ui/theme.toml"), "[colour]\nbody = \"red\"\n").expect("a theme");
        let message = ui_theme(&config).expect_err("an invalid theme");
        assert!(
            message.starts_with("[game] theme: ")
                && message.ends_with("[colour] body: `red` is not a colour (#rrggbb or #rrggbbaa)"),
            "{message}"
        );
        std::fs::write(empty.join("ui/theme.toml"), shipped_theme()).expect("the shipped theme");
        assert!(ui_theme(&config).expect("a valid theme").is_some());
    }

    struct CompletedOwner;

    impl Owner for CompletedOwner {
        fn name(&self) -> &'static str {
            "ui"
        }

        fn wait_ready(&mut self, _: Duration) -> Result<(), OwnerError> {
            Ok(())
        }

        fn poll_ready(&mut self, _: Duration) -> Result<Progress, OwnerError> {
            Ok(Progress::Done)
        }

        fn request_start(&mut self) -> Result<(), OwnerError> {
            Ok(())
        }

        fn await_started(&mut self, _: Duration) -> Result<(), OwnerError> {
            Ok(())
        }

        fn poll_started(&mut self, _: Duration) -> Result<Progress, OwnerError> {
            Ok(Progress::Done)
        }

        fn request_stop(&mut self) -> Result<(), OwnerError> {
            Ok(())
        }

        fn await_stopped(&mut self, _: Duration) -> Result<(), OwnerError> {
            Ok(())
        }

        fn cancel(&mut self) {}

        fn wait_finished(&mut self, _: Duration) -> bool {
            true
        }
    }

    #[test]
    fn real_supervise_composition_ends_parent_and_spawn_closure_writers_before_finish() {
        use std::sync::atomic::AtomicBool;

        let assets = testing::Dir::new();
        std::fs::write(
            assets.0.join("startup.png"),
            testing::png(1, 1, &[1, 2, 3, 4]),
        )
        .expect("write startup texture");
        let config = config::load_text_for_test(&format!(
            "[render]\nassets = {:?}\nmax_quads = 1\n",
            assets.0
        ));
        let retained_extensions = Arc::new(std::sync::Mutex::new(None));
        let observed_service =
            Arc::new(std::sync::Mutex::new(None::<ck_resources::ResourceService>));
        let outcome_status = supervise_with_owner_factory(
            &config,
            native(),
            Shared {
                quit: Arc::new(AtomicBool::new(false)),
                notifier: Notifier::new(),
            },
            Content::default(),
            |setup| {
                let backend_resources = Arc::clone(&observed_service);
                Plan {
                    extension: setup.extension,
                    run: Some(Box::new(move |_| {
                        backend_resources
                            .lock()
                            .unwrap()
                            .as_ref()
                            .expect("native fake has resources")
                            .detach_backend();
                        ck_runtime::RunEnd::Finished
                    })),

                    screen: None,
                }
            },
            production_headless_plan,
            |wiring, resources, render_end| {
                let service = resources.expect("native supervise composes resources");
                let key = service.load("startup.png").expect("admit startup texture");
                let job = service.take_job().expect("startup load job");
                service.complete(job.run());
                render_end.add_test_quad(service.retain(key).expect("quad texture"));
                service.release(key).expect("release startup token");
                *observed_service.lock().unwrap() = Some(service.clone());
                let extensions = wiring.extensions.clone();
                let keep = Arc::clone(&retained_extensions);
                vec![OwnerSpawn::new("ui", move || {
                    *keep.lock().unwrap() = Some(extensions);
                    Ok(Box::new(CompletedOwner))
                })]
            },
        );
        assert_eq!(outcome_status.code, 0);
        let service = observed_service
            .lock()
            .unwrap()
            .clone()
            .expect("observe composed resources after supervise");
        let counts = service.stats();
        assert_eq!(counts.jobs.reserved, 0, "job slots");
        assert_eq!(counts.cpu_pixels.reserved, 0, "CPU pixel bytes");
        assert_eq!(counts.metadata.reserved, 0, "metadata bytes");
        assert_eq!(counts.registry.live, 0, "registry entries");
        assert_eq!(counts.loaders.referenced_at_teardown, 0);
        assert!(
            retained_extensions.lock().unwrap().is_some(),
            "spawn closure's UI extension clones remain alive after supervise returns"
        );
    }

    /// The real composition for a mode, over real UI VMs (the script at `ui`), and `check` run as the window plan's run closure
    /// after the VMs started, with the run's bridge and its update control (`None` without an update source).
    fn composed_run(
        mode: mode::Mode,
        script: &str,
        check: impl FnOnce(&Arc<SceneBridge>, Option<ck_window::UpdateControl>) + 'static,
    ) -> Status {
        use std::sync::atomic::AtomicBool;

        let assets = testing::Dir::new();
        std::fs::write(
            assets.0.join("a.png"),
            testing::png(8, 4, &[200; 8 * 4 * 4]),
        )
        .expect("an image");
        let ui = assets.0.join("ui.lua");
        std::fs::write(&ui, script).expect("the ui script");
        let config = config::load_text_for_test(&format!(
            "[render]\nassets = {:?}\n\n[lua.ui]\nentry = {:?}\n",
            assets.0, ui
        ));
        supervise_with_owner_factory(
            &config,
            mode,
            Shared {
                quit: Arc::new(AtomicBool::new(false)),
                notifier: Notifier::new(),
            },
            Content::default(),
            |setup| {
                let bridge = Arc::clone(&setup.env.bridge);
                let control = setup
                    .env
                    .settings
                    .updates()
                    .map(ck_window::UpdateSettings::control);
                Plan {
                    extension: setup.extension,
                    run: Some(Box::new(move |_| {
                        check(&bridge, control);
                        ck_runtime::RunEnd::Finished
                    })),

                    screen: None,
                }
            },
            production_headless_plan,
            |wiring, _, _| vm::spawns(&config.vms, &config.whole, Some(wiring)),
        )
    }

    /// A windowed run, on either backend: the UI VM has `texture`, the scene API and `frame`; its startup's scene is published by
    /// the completion hook the composition gave it; `frame.on_update(fn)` turned the loop's own update control on and
    /// `frame.on_update(nil)` turned it off again. Negative control: a composition without the hook publishes nothing
    /// (`published` stays 0), and one that did not wire the control leaves it off.
    #[test]
    fn a_windowed_composition_gives_the_ui_vm_the_texture_scene_and_frame_apis_and_the_publishing_hook()
     {
        for mode in [native(), vulkan()] {
            let status = composed_run(
                mode,
                "local image = texture.load('a.png')\n\
                 local quad\n\
                 function startup() quad = render.create_quad{texture = image, width = 8, height = 4} end\n\
                 frame.on_update(function() end)\n\
                 function shutdown() local ok, err = pcall(render.destroy_quad, quad)\n\
                   assert(not ok and tostring(err):find('the resource service has ended', 1, true), tostring(err))\n\
                 end\n",
                |bridge, control| {
                    assert_eq!(
                        bridge.counts().published,
                        1,
                        "the completion hook published startup's quad"
                    );
                    let control = control.expect("a windowed loop has an update source");
                    assert!(
                        format!("{control:?}").contains("enabled: true"),
                        "frame.on_update turned the loop's control on: {control:?}"
                    );
                },
            );
            assert_eq!(status.code, 0, "{mode:?}: {status:?}");
            let status = composed_run(
                mode,
                "frame.on_update(function() end)\nframe.on_update(nil)\n",
                |_, control| {
                    let control = control.expect("a windowed loop has an update source");
                    assert!(
                        format!("{control:?}").contains("enabled: false"),
                        "frame.on_update(nil) turned the control off: {control:?}"
                    );
                },
            );
            assert_eq!(status.code, 0, "{mode:?}: {status:?}");
        }
    }

    /// Both windowed compositions are **upgraded**: the legacy `render.quad` goes through the compatibility lane, so it is a
    /// scene publication (a compat entry, with CK-119's gate on the backend). Negative control: a headless run keeps M4's path
    /// and publishes nothing to the scene.
    #[test]
    fn the_windowed_compositions_send_the_legacy_render_quad_through_the_compatibility_lane() {
        let script = "render.quad{image = 'a.png', width = 8, height = 4}\n";
        for mode in [native(), vulkan()] {
            let status = composed_run(mode, script, |bridge, _| {
                assert_eq!(
                    bridge.counts().published,
                    1,
                    "the lane's compat quad was published as a scene frame"
                );
            });
            assert_eq!(status.code, 0, "{mode:?}: {status:?}");
        }
        let status = composed_run(mode::Mode::Headless, script, |bridge, _| {
            assert_eq!(bridge.counts().published, 0, "M4's path publishes no scene");
        });
        assert_eq!(status.code, 0, "{status:?}");
    }

    /// `glfw-vulkan` refuses nothing of the scene path, and `diagnostics.metrics_mark` is accepted (milestone 5.5, issue 6): the mark
    /// opens the run's window as it does on native-metal. A second call changes nothing.
    #[test]
    fn the_glfw_vulkan_composition_accepts_the_metrics_mark() {
        let status = composed_run(
            vulkan(),
            "diagnostics.metrics_mark()\n\
             diagnostics.metrics_mark()\n\
             local image = texture.load('a.png')\n\
             assert(type(image) == 'string')\n\
             frame.on_update(function() end)\n",
            |_, control| {
                let control = control.expect("glfw-vulkan has an update source");
                assert!(
                    format!("{control:?}").contains("enabled: true"),
                    "{control:?}"
                );
            },
        );
        assert_eq!(status.code, 0, "{status:?}");
        let status = composed_run(native(), "diagnostics.metrics_mark()\n", |_, _| {});
        assert_eq!(status.code, 0, "{status:?}");
    }

    #[test]
    fn the_native_loop_carries_the_frame_values_with_the_update_source_off() {
        let default = loop_settings(&config::Config::default(), native());
        let updates = default.updates().expect("an update source");
        assert_eq!(updates.updates_per_second(), 60);
        assert_eq!(updates.max_delta(), Duration::from_millis(100));
        assert!(
            format!("{:?}", updates.control()).contains("enabled: false"),
            "the control starts off: {:?}",
            updates.control()
        );
        let configured = config::load_text_for_test(
            "[frame]\nupdates_per_second = 144\nmax_delta_seconds = 0.05\n",
        );
        let settings = loop_settings(&configured, native());
        let updates = settings.updates().expect("an update source");
        assert_eq!(updates.updates_per_second(), 144);
        assert_eq!(updates.max_delta(), Duration::from_millis(50));
        assert!(format!("{:?}", updates.control()).contains("enabled: false"));
    }

    #[test]
    fn the_glfw_vulkan_loop_carries_the_frame_values_with_the_update_source_off_and_a_headless_run_has_none()
     {
        let configured = config::load_text_for_test(
            "[frame]\nupdates_per_second = 144\nmax_delta_seconds = 0.05\n",
        );
        let settings = loop_settings(&configured, vulkan());
        let updates = settings.updates().expect("an update source");
        assert_eq!(updates.updates_per_second(), 144);
        assert_eq!(updates.max_delta(), Duration::from_millis(50));
        assert!(
            format!("{:?}", updates.control()).contains("enabled: false"),
            "the control starts off: {:?}",
            updates.control()
        );
        assert!(
            loop_settings(&configured, mode::Mode::Headless)
                .updates()
                .is_none(),
            "a headless run configures no update source"
        );
    }

    #[test]
    fn scene_composition_applies_the_configured_max_quads() {
        let assets = testing::Dir::new();
        std::fs::write(assets.0.join("one.png"), testing::png(1, 1, &[1, 2, 3, 4]))
            .expect("write scene texture");
        let config = config::load_text_for_test(&format!(
            "[render]\nassets = {:?}\nmax_quads = 1\n",
            assets.0
        ));
        let service = ck_resources::ResourceService::with_assets(
            config.resources.service,
            ck_resources::Mode::Backend,
            ck_resources::ServiceWakes::default(),
            config.render.assets.clone().map(ck_resources::Assets::new),
        )
        .expect("compose resource service");
        let decoded = service
            .create_decoded("one.png")
            .expect("decode configured image");
        let bridge = scene_bridge(&config);
        let mut writer = bridge.take_writer().expect("composed scene writer");
        writer
            .create_quad(
                service.retain(decoded.key).expect("first quad texture"),
                0.0,
                0.0,
                1.0,
                1.0,
            )
            .expect("first quad within configured cap");
        assert!(
            writer
                .create_quad(
                    service.retain(decoded.key).expect("second quad texture"),
                    1.0,
                    0.0,
                    1.0,
                    1.0,
                )
                .is_err(),
            "the composed bridge refuses a second quad at max_quads = 1"
        );
        drop(writer);
        service.release(decoded.key).expect("release decoded owner");
        service.close();
        let _ = service.finish_teardown();
    }

    #[test]
    fn the_loop_gets_the_configured_bindings() {
        let default = config::Config::default();
        assert_eq!(
            loop_settings(&default, native()).bindings(),
            &default.bindings
        );
        let text = "[input.bindings]\nclose = [\"KeyQ\"]\n";
        let configured = config::load_text_for_test(text);
        assert_ne!(configured.bindings, default.bindings);
        assert_eq!(
            loop_settings(&configured, native()).bindings(),
            &configured.bindings
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "three configurations, each run to the window plan, read best together"
    )]
    fn glfw_vulkan_accepts_custom_close_and_frame_and_reaches_the_window_plan() {
        use std::sync::atomic::AtomicBool;

        let directory = testing::Dir::new();
        let config_path = directory.0.join("custom-close.toml");
        std::fs::write(
            &config_path,
            "[window]\nbackend = \"glfw-vulkan\"\n[input.bindings]\nclose = [\"KeyQ\"]\n",
        )
        .expect("write custom close config");
        let options = RunOptions {
            config: Some(config_path),
            headless: false,
            backend: None,
            load: None,
            seed: None,
        };
        let planned = Cell::new(false);
        let saw_custom = Cell::new(false);
        let status = execute_with(
            &options,
            Shared {
                quit: Arc::new(AtomicBool::new(false)),
                notifier: Notifier::new(),
            },
            &[mode::Backend::GlfwVulkan],
            |setup| {
                planned.set(true);
                saw_custom.set(
                    setup.env.settings.bindings()
                        == &config::load_text_for_test("[input.bindings]\nclose = [\"KeyQ\"]\n")
                            .bindings,
                );
                Plan {
                    extension: setup.extension,
                    run: Some(Box::new(|_| ck_runtime::RunEnd::Finished)),

                    screen: None,
                }
            },
        );
        assert_eq!(status.code, 0);
        assert!(
            planned.get() && saw_custom.get(),
            "the custom binding reaches the glfw-vulkan window plan"
        );

        let frame_path = directory.0.join("frame.toml");
        std::fs::write(
            &frame_path,
            "[window]\nbackend = \"glfw-vulkan\"\n[frame]\nupdates_per_second = 90\n",
        )
        .expect("write frame config");
        let frame_options = RunOptions {
            config: Some(frame_path),
            headless: false,
            backend: None,
            load: None,
            seed: None,
        };
        let planned_frame = Cell::new(0);
        let status = execute_with(
            &frame_options,
            Shared {
                quit: Arc::new(AtomicBool::new(false)),
                notifier: Notifier::new(),
            },
            &[mode::Backend::GlfwVulkan],
            |setup| {
                planned_frame.set(
                    setup
                        .env
                        .settings
                        .updates()
                        .map_or(0, ck_window::UpdateSettings::updates_per_second),
                );
                Plan {
                    extension: setup.extension,
                    run: Some(Box::new(|_| ck_runtime::RunEnd::Finished)),

                    screen: None,
                }
            },
        );
        assert_eq!(status.code, 0, "[frame] is accepted on glfw-vulkan");
        assert_eq!(
            planned_frame.get(),
            90,
            "the configured pacing reaches the window plan as the update source"
        );

        // An old configuration without explicit bindings keeps the default close binding.
        let old_path = directory.0.join("legacy.toml");
        std::fs::write(&old_path, "[window]\nbackend = \"glfw-vulkan\"\n")
            .expect("write legacy config");
        let old_options = RunOptions {
            config: Some(old_path),
            headless: false,
            backend: None,
            load: None,
            seed: None,
        };
        let legacy_bindings = config::Config::default().bindings;
        let saw_legacy = Cell::new(false);
        let status = execute_with(
            &old_options,
            Shared {
                quit: Arc::new(AtomicBool::new(false)),
                notifier: Notifier::new(),
            },
            &[mode::Backend::GlfwVulkan],
            |setup| {
                saw_legacy.set(setup.env.settings.bindings() == &legacy_bindings);
                Plan {
                    extension: setup.extension,
                    run: Some(Box::new(|_| ck_runtime::RunEnd::Finished)),

                    screen: None,
                }
            },
        );
        assert_eq!(status.code, 0);
        assert!(
            saw_legacy.get(),
            "legacy mode retains Escape as its close binding"
        );
    }

    fn exit_code_text(code: ExitCode) -> String {
        format!("{code:?}")
    }

    #[test]
    fn a_failed_duplication_exits_with_status_4_and_says_why_on_stdout() {
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let result = route_stderr_to_stdout(failing(libc::EBADF), &mut stdout, &mut stderr);
        assert_eq!(result, Err(4));
        assert_eq!(
            String::from_utf8(stdout).expect("UTF-8"),
            format!(
                "codekarl: cannot route stderr to stdout: {}\n",
                io::Error::from_raw_os_error(libc::EBADF)
            )
        );
        assert!(
            stderr.is_empty(),
            "stdout took the message, so stderr is not used"
        );
    }

    #[test]
    fn the_message_falls_back_to_stderr_when_stdout_cannot_be_written() {
        let mut stderr = Vec::new();
        let result = route_stderr_to_stdout(failing(libc::EMFILE), &mut Broken, &mut stderr);
        assert_eq!(result, Err(4));
        assert_eq!(
            String::from_utf8(stderr).expect("UTF-8"),
            format!(
                "codekarl: cannot route stderr to stdout: {}\n",
                io::Error::from_raw_os_error(libc::EMFILE)
            )
        );
    }

    #[test]
    fn a_failure_of_both_streams_still_exits_with_status_4() {
        assert_eq!(
            route_stderr_to_stdout(failing(libc::EBADF), &mut Broken, &mut Broken),
            Err(4)
        );
    }

    #[test]
    fn a_successful_duplication_writes_nothing_and_calls_the_duplication_once() {
        let calls = Cell::new(0);
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let result = route_stderr_to_stdout(
            || {
                calls.set(calls.get() + 1);
                Ok(())
            },
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(result, Ok(()));
        assert_eq!(calls.get(), 1);
        assert!(stdout.is_empty() && stderr.is_empty());
    }

    #[test]
    fn nothing_else_runs_after_a_failed_duplication() {
        let ran = Cell::new(false);
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let code = enter(failing(libc::EBADF), &mut stdout, &mut stderr, || {
            ran.set(true);
            ExitCode::SUCCESS
        });
        assert!(!ran.get(), "the command must not run");
        assert_eq!(exit_code_text(code), exit_code_text(ExitCode::from(4)));
        assert!(!stdout.is_empty());
    }

    #[test]
    fn the_command_runs_and_decides_the_status_after_a_successful_duplication() {
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let code = enter(|| Ok(()), &mut stdout, &mut stderr, || ExitCode::FAILURE);
        assert_eq!(exit_code_text(code), exit_code_text(ExitCode::FAILURE));
        assert!(stdout.is_empty() && stderr.is_empty());
    }
}
