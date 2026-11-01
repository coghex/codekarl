//! `Renderer` over the Vulkan scene presenter on a real `ck-window-glfw` window, driven by `ck_window::run` with a scripted UI
//! (milestone 5.5, issue 4, part A; CK-117's Vulkan counterpart): the real resource path, end to end, on a GLFW window with the
//! surface's own pixels read back through `TestHooks::capture_frames`.
//!
//! GLFW's Cocoa window needs the process's main thread, so this executable is a **supervisor and a worker** (`harness = false`),
//! like `ck-gpu-vulkan`'s window test: the supervisor starts it again as the worker (`--worker <scenario>`), under the shared
//! bounded supervisor, gives it a whole-process deadline, and fails on any validation message. Every scenario runs twice, on a
//! quiet keyboard and with a thread typing without stopping; the worker installs `ck-window-glfw`'s test-only filter (a local
//! event monitor that drops every keyboard event without the tests' mark) before its first window, fails if a close request
//! reached its UI, and in the typing run waits until the filter has dropped keys.
//!
//! **Validation is on, with synchronization validation, and any validation message fails the case.** The backend under test is a
//! small `Gpu2d` over `VulkanSinkSide::with_resources` (CK-158) and `VulkanPresenter::start_scene_with_test_hooks` (CK-157),
//! with a real `Backend`-mode `ResourceService`, its `WakeRelay`, a real GLFW window, instance, surface and `VulkanContext`.
//!
//! The scenarios are CK-117's, on Vulkan:
//!
//! 1. **lifecycle**: two file-loaded textures and their quads, published before the window opens, with their jobs held by the
//!    service's `JobGate`: the first captured frame shows the checker inside each quad; the gate is released and a later frame
//!    shows each quad's own colour at the generation that adopted it; the quads move (no upload, no texture created); one
//!    texture is reloaded (new colour, same rectangle); the other quad is destroyed and its texture released (1 live and 0
//!    retiring); the window closes (`Proven`, relay disconnected) and after the teardown the registry has 0 live, 0 retiring and
//!    0 referenced. `movement-reupload` is its negative control: one texture is uploaded again while the quads move, and the
//!    cumulative upload counters must show it.
//! 2. **legacy**: a `LegacyLane` quad set before the window opens: the first captured frame contains its image, not a checker or a
//!    bare clear (CK-119's gate on Vulkan).
//! 3. **early-close**: the checker's upload is held by the store's stall gate; the window closes while it is held; the stall gate
//!    is opened only after the presenter's stop was forwarded; the end is `Proven` with zero frames.

#![cfg(target_os = "macos")]

use std::env;
use std::ffi::c_void;
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ck_gpu_vulkan::vk::Handle as _;
use ck_gpu_vulkan::{
    BindlessLimits, CleanupResult as GpuCleanup, FrameCapture, FrameCaptures, Instance,
    InstanceOptions, LedgerLimits, Loader, PresenterReport as GpuReport, SceneLimits, SceneSetup,
    SceneStats, StallGate, TestHooks, TextureLimits, TextureTestHooks, ValidationLogHandle,
    VulkanContext, VulkanPresenter, create_instance_with, destroy_surface_and_instance,
    load_loader, vk,
};
use ck_render_2d::{
    Filter, Gpu2d, LegacyLane, Phase, QuadKey, QuadSink, Renderer, SceneBridge, SceneWriter,
    VulkanResourceLimits, VulkanSinkSide, WakeRelay,
};
use ck_resources::{
    Assets, JobGate, LoaderPool, Mode, ResourceLimits, ResourceService, ServiceWakes, StatusState,
    TextureKey,
};
use ck_window::{
    Answer, CleanupResult, LoopEnd, LoopSettings, Notifier, PixelSize, PostError, PresenterReport,
    RequestWriter, Requests, UiAnswer, UiEvent, UiLink, WindowDesc, run,
};
use ck_window_glfw::{GlfwBackend, GlfwHandles, VulkanLoaderForGlfw};
#[path = "../../window/tests/support/activation_watch.rs"]
mod activation_watch;

#[path = "../../../apps/codekarl/tests/supervisor/mod.rs"]
#[expect(
    dead_code,
    reason = "the shared supervisor serves other tests; this one uses its spawn and finish"
)]
mod supervisor;

// The one binding of the shared native info structure and its layout check, included from `ck-window-glfw`'s test support (this
// crate reads that crate's test files by path, as `build.rs` does for the native source).
#[path = "../../window-glfw/tests/support/info_abi.rs"]
#[expect(
    dead_code,
    reason = "the binding's other checks serve the crates that own the native support"
)]
mod info_abi;

use supervisor::Supervised;

const WORKER_FLAG: &str = "--worker";
const FORWARDING_FIXTURE: &str = "external_forwarding_fixture";
/// `1` makes a worker run with the owner typing without stopping.
const TYPING_VARIABLE: &str = "CK_RENDER2D_VULKAN_WINDOW_TYPING";
/// The longest a worker may run.
const DEADLINE: Duration = Duration::from_secs(60);
/// The scenarios, named by the worker flag's value.
const SCENARIOS: [&str; 4] = ["lifecycle", "movement-reupload", "legacy", "early-close"];

unsafe extern "C" {
    fn ck_glfw_test_install_input_filter();
    fn ck_glfw_test_start_typist();
}

fn typing() -> bool {
    env::var_os(TYPING_VARIABLE).is_some_and(|value| value == "1")
}

/// Drops the owner's keyboard before any window exists, and starts the typist in a typing run. Main thread.
fn guard_the_keyboard() {
    // SAFETY: a plain AppKit call on the main thread, before the first window; idempotent.
    unsafe { ck_glfw_test_install_input_filter() };
    if typing() {
        // SAFETY: after the filter; starts one thread that only posts events, which AppKit allows from any thread.
        unsafe { ck_glfw_test_start_typist() };
    }
}

/// Whether the keyboard guard is satisfied: a quiet run needs nothing, a typing run needs the filter to have dropped keys (so it
/// proves something about typing). Main thread.
fn typing_was_dropped() -> bool {
    !typing() || info_abi::read().filtered_keys > 0
}

/// Where the script is, in words, for the watchdog's message.
static PROGRESS: Mutex<String> = Mutex::new(String::new());

/// Close requests that reached the worker's UI. The filter drops Escape before the window sees it, so none may.
static CLOSE_REQUESTS: AtomicUsize = AtomicUsize::new(0);

fn main() -> ExitCode {
    let arguments: Vec<String> = env::args().skip(1).collect();
    match arguments.as_slice() {
        [flag, scenario] if flag == WORKER_FLAG => worker(scenario),
        // Cargo may pass its own arguments (a filter, `--nocapture`); the scenarios always all run.
        _ => supervisor(),
    }
}

// ---------------------------------------------------------------------------------------------
// The supervisor
// ---------------------------------------------------------------------------------------------

/// The keyboard filter lives in this test's archive and in no library artifact.
fn input_filter_isolation() -> Result<(), String> {
    let archive = std::fs::read(env!("CK_RENDER_2D_GLFW_TESTING_ARCHIVE"))
        .map_err(|error| format!("isolation: the test support archive cannot be read: {error}"))?;
    let has = |haystack: &[u8], needle: &str| {
        haystack
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    };
    for name in [
        "ck_glfw_test_install_input_filter",
        "ck_glfw_test_start_typist",
        "ck_glfw_test_info",
    ] {
        if !has(&archive, name) {
            return Err(format!("isolation: the test support archive lacks {name}"));
        }
    }
    let exe = env::current_exe().map_err(|error| format!("isolation: {error}"))?;
    let directory = exe
        .parent()
        .ok_or("isolation: the test executable has no directory")?;
    let mut libraries = 0;
    for entry in std::fs::read_dir(directory).map_err(|error| format!("isolation: {error}"))? {
        let path = entry.map_err(|error| format!("isolation: {error}"))?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if name.starts_with("libck_render_2d-")
            && std::path::Path::new(name)
                .extension()
                .is_some_and(|extension| extension == "rlib")
        {
            libraries += 1;
            let bytes =
                std::fs::read(&path).map_err(|error| format!("isolation: {name}: {error}"))?;
            if has(&bytes, "ck_glfw_test_") {
                return Err(format!(
                    "isolation: {name} contains test support (ck_glfw_test_)"
                ));
            }
        }
    }
    if libraries == 0 {
        return Err("isolation: no libck_render_2d artifact was found to check".to_owned());
    }
    Ok(())
}

/// Runs one scenario in a worker under the shared supervisor; `Ok` when it exited 0.
fn run_worker(scenario: &str, typing: bool) -> Result<(), String> {
    let mut command = Command::new(env::current_exe().map_err(|error| error.to_string())?);
    command.arg(WORKER_FLAG).arg(scenario);
    command.env(TYPING_VARIABLE, if typing { "1" } else { "0" });
    // The worker must find the Vulkan loader by its absolute path alone, so no dynamic-loader variable reaches it.
    for (name, _) in env::vars() {
        if name.starts_with("DYLD_") || name == "LD_LIBRARY_PATH" {
            command.env_remove(name);
        }
    }
    let supervised = Supervised::spawn(command, DEADLINE)?;
    let seen = supervised.finish()?;
    let forwarded = activation_watch::forwarded_external_summaries(
        "render2d.vulkan-window",
        scenario,
        if typing { "typing" } else { "quiet" },
        &seen.stdout,
    );
    for line in &forwarded {
        println!("{line}");
    }
    if scenario == FORWARDING_FIXTURE {
        activation_watch::check_fixture_forwarding(
            "render2d.vulkan-window",
            scenario,
            if typing { "typing" } else { "quiet" },
            &forwarded,
        )?;
    }
    if seen.code == Some(0) {
        Ok(())
    } else {
        Err(format!(
            "{scenario} ended with {:?}\n{}",
            seen.code, seen.stdout
        ))
    }
}

fn supervisor() -> ExitCode {
    let mut failures = Vec::new();
    let isolation = input_filter_isolation();
    println!(
        "test supervisor: the keyboard filter is in the test archive and in no library artifact ... {}",
        if isolation.is_ok() { "ok" } else { "FAILED" }
    );
    if let Err(message) = isolation {
        failures.push(("isolation".to_owned(), message));
    }
    let mut total = 2;
    let forwarding = run_worker(FORWARDING_FIXTURE, false);
    println!(
        "test supervisor: a passing fixture's external summary reaches supervisor output ... {}",
        if forwarding.is_ok() { "ok" } else { "FAILED" }
    );
    if let Err(message) = forwarding {
        failures.push(("external forwarding".to_owned(), message));
    }
    for typing in [false, true] {
        for scenario in SCENARIOS {
            total += 1;
            let label = if typing {
                format!("{scenario} (owner typing)")
            } else {
                scenario.to_owned()
            };
            let started = Instant::now();
            let result = run_worker(scenario, typing);
            println!(
                "test {label} ... {} ({:.1} s)",
                if result.is_ok() { "ok" } else { "FAILED" },
                started.elapsed().as_secs_f64()
            );
            if let Err(message) = result {
                failures.push((label, message));
            }
        }
    }
    for (name, message) in &failures {
        println!("---- {name} ----\n{message}");
    }
    println!(
        "test result: {}. {} passed; {} failed",
        if failures.is_empty() { "ok" } else { "FAILED" },
        total - failures.len(),
        failures.len()
    );
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

// ---------------------------------------------------------------------------------------------
// The fixtures
// ---------------------------------------------------------------------------------------------

/// The side of the 8 x 8 solid textures the scenarios use.
const SIDE: u32 = 8;
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const ORANGE: [u8; 4] = [255, 128, 0, 255];
/// The window's clear colour, [0.1, 0.2, 0.3], as bytes.
const CLEAR: [u8; 4] = [26, 51, 77, 255];
/// The checker's two colours.
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
/// The quads' top-left corners, in points (the window is 200 x 150 points), and their side.
const FIRST_AT: (f64, f64) = (10.0, 10.0);
const SECOND_AT: (f64, f64) = (110.0, 70.0);
const QUAD_SIDE: f64 = 40.0;
/// How many times the quads move, each to a position of its own.
const MOVES: usize = 40;
/// The window's width in points, for the capture's scale.
const WINDOW_POINTS: f64 = 200.0;

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1_u32, 0_u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn png_chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
    out.extend_from_slice(
        &u32::try_from(data.len())
            .expect("a small chunk")
            .to_be_bytes(),
    );
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// A solid `SIDE` x `SIDE` RGBA8 PNG (one stored deflate block), for the scenario's assets directory.
fn solid_png(colour: [u8; 4]) -> Vec<u8> {
    let mut raw = Vec::new();
    for _ in 0..SIDE {
        // Each row starts with the filter type 0 (none).
        raw.extend(std::iter::once(0).chain(colour.repeat(SIDE as usize)));
    }
    let length = u16::try_from(raw.len()).expect("a small image");
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&length.to_le_bytes());
    zlib.extend_from_slice(&(!length).to_le_bytes());
    zlib.extend_from_slice(&raw);
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut header = Vec::new();
    header.extend_from_slice(&SIDE.to_be_bytes());
    header.extend_from_slice(&SIDE.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    png_chunk(&mut out, *b"IHDR", &header);
    png_chunk(&mut out, *b"IDAT", &zlib);
    png_chunk(&mut out, *b"IEND", &[]);
    out
}

/// The pixel of a captured (`B8G8R8A8_UNORM`) frame at a point of the window, as RGBA.
fn pixel_at(frame: &FrameCapture, x: f64, y: f64) -> [u8; 4] {
    assert_eq!(
        frame.format,
        vk::Format::B8G8R8A8_UNORM,
        "this test reads B8G8R8A8_UNORM"
    );
    let scale = f64::from(frame.size.0) / WINDOW_POINTS;
    // Truncation is the point: the pixel the point falls in.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "window points inside the frame scaled to pixel indices"
    )]
    let (px, py) = ((x * scale) as usize, (y * scale) as usize);
    let offset = (py * frame.size.0 as usize + px) * 4;
    [
        frame.bytes[offset + 2],
        frame.bytes[offset + 1],
        frame.bytes[offset],
        frame.bytes[offset + 3],
    ]
}

/// The pixel in the middle of the quad whose top left is `at`.
fn middle(frame: &FrameCapture, at: (f64, f64)) -> [u8; 4] {
    pixel_at(frame, at.0 + QUAD_SIDE / 2.0, at.1 + QUAD_SIDE / 2.0)
}

/// Whether the frame shows a quad of `colour` exactly at `at` (its top-left corner, in points): the colour just inside each of the
/// four corners and the centre, and not that colour half a point outside each edge's middle (the pixel next to the edge). A quad one point off fails it, so a
/// frame that passes was drawn from a scene with the quad at exactly this position, and no earlier one.
fn is_exactly_at(frame: &FrameCapture, at: (f64, f64), colour: [u8; 4]) -> bool {
    let (far_x, far_y) = (at.0 + QUAD_SIDE, at.1 + QUAD_SIDE);
    let inside = [
        (at.0 + 1.0, at.1 + 1.0),
        (far_x - 1.0, at.1 + 1.0),
        (at.0 + 1.0, far_y - 1.0),
        (far_x - 1.0, far_y - 1.0),
        (at.0 + QUAD_SIDE / 2.0, at.1 + QUAD_SIDE / 2.0),
    ];
    let outside = [
        (at.0 - 0.5, at.1 + QUAD_SIDE / 2.0),
        (far_x + 0.5, at.1 + QUAD_SIDE / 2.0),
        (at.0 + QUAD_SIDE / 2.0, at.1 - 0.5),
        (at.0 + QUAD_SIDE / 2.0, far_y + 0.5),
    ];
    inside.iter().all(|&(x, y)| pixel_at(frame, x, y) == colour)
        && outside
            .iter()
            .all(|&(x, y)| pixel_at(frame, x, y) != colour)
}

/// Whether the quad at `at` shows the checker: the rectangle holds both the checker's magenta and its black cells (8-pixel cells
/// of a 64 x 64 image, scaled to the quad and sampled nearest, so the colours are exact).
fn shows_the_checker(frame: &FrameCapture, at: (f64, f64)) -> bool {
    let mut magenta = 0_usize;
    let mut black = 0_usize;
    let steps = 24;
    for row in 0..steps {
        for column in 0..steps {
            let x = at.0 + 1.0 + (QUAD_SIDE - 2.0) * f64::from(column) / f64::from(steps);
            let y = at.1 + 1.0 + (QUAD_SIDE - 2.0) * f64::from(row) / f64::from(steps);
            match pixel_at(frame, x, y) {
                colour if colour == MAGENTA => magenta += 1,
                colour if colour == BLACK => black += 1,
                _ => {}
            }
        }
    }
    magenta >= 20 && black >= 20
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------------------------
// The backend under test
// ---------------------------------------------------------------------------------------------

/// What the main thread saw of a run.
#[derive(Default)]
struct Probe {
    /// The presenter's latest counters, read after every report poll and every end poll.
    stats: Mutex<Option<SceneStats>>,
    /// How many times `stats` was read from the presenter: a reading with a higher number was taken after one with a lower.
    stats_reads: AtomicU64,
    /// Every way the contract was seen broken, as it happened.
    violations: Mutex<Vec<String>>,
    /// `Failed` reports taken.
    failures: AtomicUsize,
    /// `FirstFrameQueued` reports taken.
    first_frames: AtomicUsize,
    /// The backend's `request_stop` forwarded the call to the real presenter and it returned (published **after** the call).
    stop_forwarded: AtomicBool,
    /// The frames the presenter captured.
    captures: FrameCaptures,
}

impl Probe {
    fn stats(&self) -> Option<SceneStats> {
        *lock(&self.stats)
    }

    /// The number of the latest reading of the presenter's counters.
    fn stats_read(&self) -> u64 {
        self.stats_reads.load(Ordering::SeqCst)
    }

    fn violate(&self, message: impl Into<String>) {
        lock(&self.violations).push(message.into());
    }

    fn captured(&self) -> usize {
        lock(&self.captures).len()
    }

    fn newest(&self) -> Option<FrameCapture> {
        lock(&self.captures).last().cloned()
    }
}

/// The instance, surface and presenter of a started run.
struct Started {
    instance: Option<Instance>,
    surface: vk::SurfaceKHR,
    log: ValidationLogHandle,
    presenter: Option<VulkanPresenter>,
}

/// The Vulkan backend of the test: `VulkanSinkSide::with_resources`, a real instance, surface and context, and
/// `start_scene_with_test_hooks`.
struct VulkanTestBackend {
    loader: Loader,
    side: VulkanSinkSide,
    probe: Arc<Probe>,
    stall: Option<(StallGate, Arc<AtomicBool>)>,
    stop_budget: Option<Duration>,
    /// The loop's wake, given to the presenter.
    wake: Arc<dyn Fn() + Send + Sync>,
    started: Option<Started>,
}

impl VulkanTestBackend {
    fn observe(&self) {
        if let Some(presenter) = self.started.as_ref().and_then(|s| s.presenter.as_ref()) {
            *lock(&self.probe.stats) = Some(presenter.scene_stats());
            self.probe.stats_reads.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Disconnects the relay; the result is asserted `Ok` (it fails only on a legacy-only side, and this side has resources).
fn disconnect_relay(side: &VulkanSinkSide, probe: &Probe) {
    if let Err(reason) = side.disconnect() {
        probe.violate(format!("the relay's disconnect returned Err: {reason}"));
    }
}

impl Gpu2d for VulkanTestBackend {
    type Handles = GlfwHandles;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        self.side.new_sink()
    }

    fn start(
        &mut self,
        handles: GlfwHandles,
        _desc: &WindowDesc,
        pixel_size: PixelSize,
        sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        let frames = self.side.accept_frames(&sink)?;
        let driver = self.side.resident_driver()?;
        let instance = create_instance_with(
            &self.loader,
            handles.required_instance_extensions(),
            InstanceOptions {
                validation: true,
                synchronization: true,
            },
        )
        .map_err(|error| error.to_string())?;
        let log = instance.validation_log();
        let raw = usize::try_from(instance.raw_handle())
            .map_err(|_| "the instance handle does not fit a pointer".to_owned())?;
        // SAFETY: `raw` is the live `VkInstance` of `instance`, which this function holds for the call; this runs on the main
        // thread (the loop's thread) and the window behind `handles` is alive (it is destroyed only by the loop's teardown,
        // after the surface).
        let surface =
            unsafe { handles.create_surface(std::ptr::without_provenance_mut::<c_void>(raw)) }
                .map(vk::SurfaceKHR::from_raw)?;
        let context =
            VulkanContext::new(&instance, Some(surface)).map_err(|error| error.to_string())?;
        let hooks = TestHooks {
            capture_frames: Some(Arc::clone(&self.probe.captures)),
            stop_budget: self.stop_budget,
            ..TestHooks::default()
        };
        let texture_hooks = self.stall.as_ref().map(|(gate, reached)| TextureTestHooks {
            stall_gate: Some(Arc::clone(gate)),
            stall_reached: Some(Arc::clone(reached)),
            ..TextureTestHooks::default()
        });
        let setup = SceneSetup::new(
            context,
            TextureLimits::default(),
            LedgerLimits::default(),
            1024,
            BindlessLimits {
                texture_slots: 131_072,
            },
            SceneLimits {
                max_quads: 1024,
                binding_geometry_bytes: 16 * 1024 * 1024,
            },
            driver,
            frames,
            texture_hooks,
        );
        let wake = Arc::clone(&self.wake);
        // SAFETY: `surface` was created on `instance` for the window `ck_window::run` holds, which outlives the presenter's
        // proven end (the loop destroys the window only after a `Proven` end, and never otherwise); the context was made for
        // this surface; nothing else uses the surface meanwhile, and the surface and the instance are destroyed only after that
        // end, in that order (`poll_end`).
        let presenter = unsafe {
            VulkanPresenter::start_scene_with_test_hooks(
                surface,
                (pixel_size.width, pixel_size.height),
                [0.1, 0.2, 0.3],
                wake,
                &setup,
                hooks,
            )
        };
        // The setup's holds go now: the presenter's thread keeps its own, and a stray clone of the context would make every
        // end a retained one (the destroy helper refuses while anything holds the instance).
        drop(setup);
        let presenter = match presenter {
            Ok(presenter) => presenter,
            Err(error) => {
                disconnect_relay(&self.side, &self.probe);
                // SAFETY: the surface belongs to `instance`; no presenter was started, so nothing uses it.
                let _ = unsafe { destroy_surface_and_instance(instance, surface, None) };
                return Err(error.to_string());
            }
        };
        if let Err(reason) = self.side.connect(presenter.texture_wake()) {
            self.probe
                .violate(format!("the relay's connect returned Err: {reason}"));
        }
        self.started = Some(Started {
            instance: Some(instance),
            surface,
            log,
            presenter: Some(presenter),
        });
        Ok(())
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        if let Some(presenter) = self.started.as_ref().and_then(|s| s.presenter.as_ref()) {
            presenter.set_clear(rgb);
        }
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        if let Some(presenter) = self.started.as_ref().and_then(|s| s.presenter.as_ref()) {
            presenter.set_pixel_size(width, height);
        }
    }

    fn request_redraw(&mut self) {
        if let Some(presenter) = self.started.as_ref().and_then(|s| s.presenter.as_ref()) {
            presenter.request_redraw();
        }
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        self.observe();
        if let Some(report) = self.side.take_report() {
            return Some(report);
        }
        let report = self
            .started
            .as_ref()
            .and_then(|s| s.presenter.as_ref())
            .and_then(VulkanPresenter::take_report)?;
        Some(match report {
            GpuReport::FirstFrameQueued => {
                self.probe.first_frames.fetch_add(1, Ordering::SeqCst);
                PresenterReport::FirstFrameQueued
            }
            GpuReport::Failed { message } => {
                self.probe.failures.fetch_add(1, Ordering::SeqCst);
                PresenterReport::Failed { message }
            }
        })
    }

    fn request_stop(&mut self) {
        if let Some(presenter) = self.started.as_ref().and_then(|s| s.presenter.as_ref()) {
            presenter.request_stop();
        }
        // Published only after the real presenter's stop flag is set and the call returned: a releaser that waits for this can
        // never open a gate while the presenter's stop is still unset.
        self.probe.stop_forwarded.store(true, Ordering::SeqCst);
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        self.observe();
        let started = self.started.as_mut()?;
        let ended = started.presenter.as_mut()?.poll_end(within)?;
        self.observe();
        let started = self.started.as_mut()?;
        match ended {
            GpuCleanup::Proven => {
                // The presenter's handle goes, the relay is disconnected, and only then are the surface and the instance
                // destroyed (the helper refuses unless the presenter was proven and nothing holds the instance).
                started.presenter = None;
                disconnect_relay(&self.side, &self.probe);
                let messages = started.log.messages();
                if !messages.is_empty() {
                    self.probe
                        .violate(format!("validation reported: {messages:?}"));
                }
                let instance = started.instance.take()?;
                // SAFETY: `surface` was created on `instance`; the presenter ended `Proven` and its handle is gone, so nothing
                // uses it, and the window it belongs to is alive: the loop destroys the window after this call.
                let destroyed = unsafe {
                    destroy_surface_and_instance(
                        instance,
                        started.surface,
                        Some(&GpuCleanup::Proven),
                    )
                };
                if let Err(refused) = destroyed {
                    // Kept for the process's end: nothing is released on a refusal.
                    let reason = format!("the surface and instance were not destroyed: {refused}");
                    std::mem::forget(refused);
                    return Some(CleanupResult::Unproven { reason });
                }
                let messages = started.log.messages();
                if !messages.is_empty() {
                    self.probe
                        .violate(format!("validation reported at destruction: {messages:?}"));
                }
                Some(CleanupResult::Proven)
            }
            GpuCleanup::Unproven { reason } => {
                disconnect_relay(&self.side, &self.probe);
                let messages = started.log.messages();
                if !messages.is_empty() {
                    self.probe
                        .violate(format!("validation reported: {messages:?}"));
                }
                Some(CleanupResult::Unproven { reason })
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------------------------

/// A hang in the loop ends the process, so the supervisor reports it.
fn watchdog() {
    thread::spawn(|| {
        thread::sleep(DEADLINE - Duration::from_secs(10));
        eprintln!(
            "worker: the window loop did not finish in time; the script was at {}",
            lock(&PROGRESS)
        );
        std::process::exit(2);
    });
}

/// Frames after the first produce no report, so nothing would wake an idle loop: a ticker does (a test-only stimulus).
fn start_ticker(notifier: &Arc<Notifier>) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
    let running = Arc::new(AtomicBool::new(true));
    let ticker = {
        let (notifier, running) = (Arc::clone(notifier), Arc::clone(&running));
        thread::spawn(move || {
            while running.load(Ordering::Acquire) {
                notifier.wake();
                thread::sleep(Duration::from_millis(5));
            }
        })
    };
    (running, ticker)
}

fn release(gate: &StallGate) {
    *lock(&gate.0) = true;
    gate.1.notify_all();
}

/// Whether the relay delivers nothing to a presenter any more: fires a service wake and connects a counting wake; a
/// disconnected relay held the wake pending and delivers it once on `connect`, a connected one called its old target.
fn relay_is_disconnected(relay: &WakeRelay) -> bool {
    let delivered = Arc::new(AtomicUsize::new(0));
    (relay.wake())();
    let counter = Arc::clone(&delivered);
    relay.connect(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    relay.disconnect();
    delivered.load(Ordering::SeqCst) == 1
}

/// What a scenario does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    Lifecycle,
    MovementReupload,
    Legacy,
    EarlyClose,
}

impl Plan {
    fn of(name: &str) -> Option<Self> {
        match name {
            "lifecycle" => Some(Self::Lifecycle),
            "movement-reupload" => Some(Self::MovementReupload),
            "legacy" => Some(Self::Legacy),
            "early-close" => Some(Self::EarlyClose),
            _ => None,
        }
    }
}

/// Where a script is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The first frame (or, for the held scenarios, its absence) is awaited.
    First,
    /// The jobs ran; waiting for both textures to draw in their own colours.
    Ready,
    /// Both drawn at this frame count; waiting for one more frame, so every counter of the uploads that made them ready has been
    /// published.
    Settling(u64),
    /// Quads are moving; `usize` moves were made.
    Moving(usize),
    /// A texture was reloaded; waiting for its second revision on screen.
    Reloaded,
    /// A quad was destroyed and its texture released; waiting for the texture to retire.
    Released,
    /// The window was asked to close.
    Closing,
}

/// The cumulative upload work, from the presenter's stats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Work {
    textures_created: u64,
    uploads_submitted: u64,
}

impl Work {
    fn of(stats: &SceneStats) -> Self {
        Self {
            textures_created: stats.textures_created,
            uploads_submitted: stats.upload_batches_submitted,
        }
    }
}

/// The script's state: the resources and quads of a run, and what each step measured.
struct Script {
    probe: Arc<Probe>,
    plan: Plan,
    service: ResourceService,
    /// The assets directory.
    root: std::path::PathBuf,
    gate: Arc<JobGate>,
    scene: SceneWriter,
    requests: RequestWriter,
    /// Held so the legacy lane lives as long as the run.
    _lane: Option<LegacyLane>,
    stall: Option<(StallGate, Arc<AtomicBool>)>,
    textures: Vec<TextureKey>,
    quads: Vec<QuadKey>,
    stage: Stage,
    opened: bool,
    /// The first captured frame's generation.
    first_generation: Option<u64>,
    baseline: Option<(u64, Work)>,
    /// The reading of the presenter's counters that was the latest when the final movement frame was first seen.
    final_seen: Option<u64>,
    reuploaded: bool,
    /// How far (in points, on both axes) the quads are from their first corners now.
    shift: f64,
    answers: Vec<UiAnswer>,
}

impl Script {
    fn status(&self, index: usize) -> (u64, Option<u64>, StatusState) {
        let status = self
            .service
            .status(self.textures[index])
            .expect("a live texture");
        (
            status.requested,
            status.shown.map(|shown| shown.revision),
            status.state,
        )
    }

    fn violate(&self, message: impl Into<String>) {
        self.probe.violate(message);
    }

    fn close_when_keys_were_dropped(&mut self) {
        if typing_was_dropped() {
            self.requests.close();
            self.stage = Stage::Closing;
        }
    }

    fn step(&mut self) {
        *lock(&PROGRESS) = format!(
            "{:?} (opened {}, captured {}, stats {:?}, statuses {:?})",
            self.stage,
            self.opened,
            self.probe.captured(),
            self.probe.stats(),
            (0..self.textures.len())
                .map(
                    |index| self.service.status(self.textures[index]).map(|status| (
                        status.requested,
                        status.shown.map(|s| s.revision),
                        status.state
                    ))
                )
                .collect::<Vec<_>>()
        );
        match self.plan {
            Plan::Lifecycle | Plan::MovementReupload => self.step_lifecycle(),
            Plan::Legacy => self.step_legacy(),
            Plan::EarlyClose => self.step_early_close(),
        }
    }

    /// The legacy quad (set before the window opens) is the compatibility quad: every captured frame, the first included, shows
    /// its image at the quad's rectangle, never a checker or a bare clear (CK-119's gate on Vulkan: the image is admitted
    /// before the window opens, and no frame is drawn until it is adopted).
    fn step_legacy(&mut self) {
        let Some(first) = lock(&self.probe.captures).first().cloned() else {
            return;
        };
        if !self.opened || self.stage != Stage::First {
            return;
        }
        let shown = middle(&first, FIRST_AT);
        if shown != RED {
            self.violate(format!(
                "the first frame's legacy quad should show the image {RED:?}, not {shown:?}"
            ));
        }
        if shows_the_checker(&first, FIRST_AT) {
            self.violate("the first frame shows a checker for the legacy quad");
        }
        self.close_when_keys_were_dropped();
    }

    /// The checker's upload is held by the store's stall gate: close once the worker holds the batch.
    fn step_early_close(&mut self) {
        let Some((_, reached)) = &self.stall else {
            return;
        };
        if self.stage == Stage::First && reached.load(Ordering::SeqCst) {
            if self.probe.captured() != 0 {
                self.violate("a frame was captured while the checker's upload was held");
            }
            self.requests.close();
            self.stage = Stage::Closing;
        }
    }

    fn first_frame_shows_the_checkers(&mut self, first: &FrameCapture) {
        for (index, at) in [FIRST_AT, SECOND_AT].into_iter().enumerate() {
            if !shows_the_checker(first, at) {
                self.violate(format!(
                    "the first frame should show the checker inside quad {index}, found the middle {:?}",
                    middle(first, at)
                ));
            }
        }
    }

    fn step_lifecycle(&mut self) {
        match self.stage {
            Stage::First => {
                let Some(first) = lock(&self.probe.captures).first().cloned() else {
                    return;
                };
                if !self.opened {
                    return;
                }
                self.first_generation = first.generation;
                self.first_frame_shows_the_checkers(&first);
                // The negative control of the pending check: the same assertion applied to a frame after adoption must fail
                // (checked when that frame arrives).
                self.gate.release_all();
                self.stage = Stage::Ready;
            }
            Stage::Ready => {
                let ready = (0..2).all(|index| self.status(index).2 == StatusState::Ready);
                let Some(frame) = self.probe.newest() else {
                    return;
                };
                if ready && middle(&frame, FIRST_AT) == GREEN && middle(&frame, SECOND_AT) == BLUE {
                    if frame.generation != self.first_generation {
                        self.violate(format!(
                            "the textures were adopted at generation {:?}, not the first frame's {:?}",
                            frame.generation, self.first_generation
                        ));
                    }
                    // The negative control: the checker assertion applied to the adopted frame fails (each quad holds only
                    // its colour).
                    if shows_the_checker(&frame, FIRST_AT) || shows_the_checker(&frame, SECOND_AT) {
                        self.violate("an adopted quad still shows checker cells");
                    }
                    // A small move asks for one more frame, so every counter of the uploads is published before the baseline.
                    self.shift = 1.0;
                    for (index, key) in self.quads.clone().into_iter().enumerate() {
                        let at = self.position(index);
                        self.scene
                            .set_position(key, at.0, at.1)
                            .expect("a live quad moves");
                    }
                    self.scene.publish();
                    self.stage =
                        Stage::Settling(self.probe.stats().map_or(0, |stats| stats.frames));
                }
            }
            Stage::Settling(frames) => {
                if let Some(stats) = self.probe.stats().filter(|stats| stats.frames > frames) {
                    self.baseline = Some((stats.frames, Work::of(&stats)));
                    self.stage = Stage::Moving(0);
                }
            }
            Stage::Moving(moves) => self.move_quads(moves),
            Stage::Reloaded => {
                let (requested, shown, state) = self.status(0);
                let Some(frame) = self.probe.newest() else {
                    return;
                };
                if requested == 2
                    && shown == Some(2)
                    && state == StatusState::Ready
                    && middle(&frame, self.position(0)) == ORANGE
                {
                    if self.scene.quad_count() != 2 {
                        self.violate("the reload changed the quads");
                    }
                    // The rectangle is unchanged: just outside its corner is the clear.
                    let at = self.position(0);
                    if pixel_at(&frame, at.0 - 2.0, at.1 - 2.0) != CLEAR {
                        self.violate("the reloaded quad's rectangle moved");
                    }
                    // The removed quad is drawn where it last moved to, so the later clear there is the removal and nothing else.
                    if !is_exactly_at(&frame, self.position(1), BLUE) {
                        self.violate("the quad to be removed was not drawn at its last position");
                    }
                    // Release: destroy quad 1 and drop its texture's token.
                    self.scene.destroy_quad(self.quads[1]).expect("a live quad");
                    self.scene.publish();
                    self.service
                        .release(self.textures[1])
                        .expect("a live texture");
                    self.stage = Stage::Released;
                }
            }
            Stage::Released => {
                let registry = self.service.stats().registry;
                let Some(frame) = self.probe.newest() else {
                    return;
                };
                // 1 live (the reloaded texture) and 0 retiring, the released texture has no entry left, and a frame shows the removed
                // quad's last place cleared (every frame before the destroy had it drawn there, checked at the reload) and the
                // survivor in place.
                if registry.live == 1
                    && registry.retiring == 0
                    && self.service.status(self.textures[1]).is_err()
                    && middle(&frame, self.position(1)) == CLEAR
                    && middle(&frame, self.position(0)) == ORANGE
                {
                    self.close_when_keys_were_dropped();
                }
            }
            Stage::Closing => {}
        }
    }

    /// Where quad `index` is now, in points.
    fn position(&self, index: usize) -> (f64, f64) {
        let base = [FIRST_AT, SECOND_AT][index];
        (base.0 + self.shift, base.1 + self.shift)
    }

    /// Whether the newest captured frame shows both quads exactly where they were last moved to.
    fn last_move_is_drawn(&self) -> bool {
        self.probe.newest().is_some_and(|frame| {
            is_exactly_at(&frame, self.position(0), GREEN)
                && is_exactly_at(&frame, self.position(1), BLUE)
        })
    }

    /// One move of every quad (and its publication), forty of them, each to a position of its own (the last, 39 points on, is held
    /// by none before it). **A move is published only once the previous one has been drawn**: the newest captured frame must show
    /// both quads exactly at the previous position, so no publication is coalesced away, every position is a presented frame,
    /// and the phase never waits for a frame nothing asks for (each publication wakes the presenter).
    fn move_quads(&mut self, moves: usize) {
        if moves > 0 && !self.last_move_is_drawn() {
            return;
        }
        if moves >= MOVES {
            self.after_the_last_move();
            return;
        }
        self.shift = f64::from(u32::try_from(moves).unwrap_or(0));
        for (index, key) in self.quads.clone().into_iter().enumerate() {
            let at = self.position(index);
            self.scene
                .set_position(key, at.0, at.1)
                .expect("a live quad moves");
        }
        self.scene.publish();
        self.stage = Stage::Moving(moves + 1);
        if self.plan == Plan::MovementReupload && !self.reuploaded && moves == 3 {
            // The control: the same pixels again (a redundant upload), while the quads keep moving.
            self.reuploaded = true;
            if self.service.reload(self.textures[0]).is_err() {
                self.violate("the control's reload was refused");
            }
        }
    }

    /// The final frame is drawn (checked by [`move_quads`](Self::move_quads)). The counters are read **after** it: the first time
    /// it is seen the number of the latest counter reading is noted, and the movement ends only on a reading taken after that
    /// one, which therefore includes every upload before that frame was captured.
    fn after_the_last_move(&mut self) {
        if self.baseline.is_none() {
            return;
        }
        if self.plan == Plan::MovementReupload && self.status(0).1 != Some(2) {
            return;
        }
        let seen = *self.final_seen.get_or_insert(self.probe.stats_read());
        if self.probe.stats_read() > seen {
            self.moved();
        }
    }

    /// The movement is over: no upload and no texture created; the captured colours follow the quads. For the control, the
    /// cumulative counters alone must show the redundant upload.
    fn moved(&mut self) {
        let Some((_, before)) = self.baseline.take() else {
            return;
        };
        let Some(stats) = self.probe.stats() else {
            self.violate("no presenter stats at the end of the movement");
            return;
        };
        let after = Work::of(&stats);
        let changed = after != before;
        if self.plan == Plan::MovementReupload {
            eprintln!("control: the cumulative counters saw {before:?} then {after:?}");
            if after.uploads_submitted <= before.uploads_submitted
                || after.textures_created <= before.textures_created
            {
                self.violate(format!(
                    "the control's redundant upload was not seen by the cumulative counters: {before:?} then {after:?}"
                ));
            }
            self.close_when_keys_were_dropped();
            return;
        }
        if changed {
            self.violate(format!(
                "moving quads changed the upload work: {before:?} then {after:?}"
            ));
            self.close_when_keys_were_dropped();
            return;
        }
        // The captured colours followed the quads: the frame that released this check shows each exactly at its final position.
        let Some(frame) = self.probe.newest() else {
            return;
        };
        if !is_exactly_at(&frame, self.position(0), GREEN)
            || !is_exactly_at(&frame, self.position(1), BLUE)
        {
            self.violate("the captured colours did not follow the quads to their final positions");
        }
        // The negative control of the predicate: the same frame does not show the quads one point earlier, so "exactly at" can fail.
        let (first, second) = (self.position(0), self.position(1));
        if is_exactly_at(&frame, (first.0 - 1.0, first.1), GREEN)
            || is_exactly_at(&frame, (second.0, second.1 - 1.0), BLUE)
        {
            self.violate("the exact-position check accepted a quad one point away");
        }
        // Reload one texture: a new revision, adopted at a frame boundary, with no change to the quad.
        // The file gets its new colour, then the texture is reloaded (there is no file watching).
        if let Err(error) = std::fs::write(self.root.join("a.png"), solid_png(ORANGE)) {
            self.violate(format!("the new image could not be written: {error}"));
        }
        match self.service.reload(self.textures[0]) {
            Ok(2) => {}
            Ok(other) => self.violate(format!("reload gave revision {other}, not 2")),
            Err(error) => self.violate(format!("reload was refused: {error:?}")),
        }
        self.stage = Stage::Reloaded;
    }
}

impl UiLink for Script {
    fn post(&mut self, event: UiEvent) -> Result<(), PostError> {
        match event {
            UiEvent::Opened => {
                self.opened = true;
            }
            UiEvent::CloseRequested { .. } => {
                CLOSE_REQUESTS.fetch_add(1, Ordering::SeqCst);
                self.answers.push(UiAnswer {
                    event,
                    answer: Answer::Decline,
                });
            }
            UiEvent::Update(update) => panic!("unexpected UI update in test script: {update:?}"),
        }
        Ok(())
    }

    fn take_answer(&mut self) -> Option<UiAnswer> {
        self.step();
        self.answers.pop()
    }

    fn is_disconnected(&self) -> bool {
        false
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one scenario's setup, run and ordered teardown read best together"
)]
fn worker(name: &str) -> ExitCode {
    if name == FORWARDING_FIXTURE {
        println!(
            "{}",
            activation_watch::format_external_summary(
                &std::process::id().to_string(),
                1,
                0,
                0,
                0,
                0,
            )
        );
        return ExitCode::SUCCESS;
    }
    let Some(plan) = Plan::of(name) else {
        eprintln!("unknown worker scenario {name:?}");
        return ExitCode::FAILURE;
    };
    ck_window::test_support::open_windows_ignoring_mouse();
    ck_window::test_support::open_windows_without_activation();
    activation_watch::install(false);
    watchdog();
    let notifier = Notifier::new();
    let requests = Requests::new();
    let writer = requests.writer(Arc::clone(&notifier));
    let reader = requests.reader();
    writer
        .open(WindowDesc {
            title: "Codekarl render-2d Vulkan window test".to_owned(),
            width: 200,
            height: 150,
            clear: [0.1, 0.2, 0.3],
        })
        .expect("a valid description");

    // The assets: small solid images the test writes.
    let root = env::temp_dir().join(format!("ck-render2d-vulkan-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir(&root).expect("a unique assets directory");
    std::fs::write(root.join("a.png"), solid_png(GREEN)).expect("the first image");
    std::fs::write(root.join("b.png"), solid_png(BLUE)).expect("the second image");
    std::fs::write(root.join("legacy.png"), solid_png(RED)).expect("the legacy image");

    // The service, with the relay's wake for what the backend must hear.
    let relay = WakeRelay::default();
    let wakes = ServiceWakes {
        jobs: None,
        uploads: Some(relay.wake()),
        retirements: Some(relay.wake()),
    };
    let service = ResourceService::with_assets(
        ResourceLimits::DEFAULT,
        Mode::Backend,
        wakes,
        Some(Assets::new(&root)),
    )
    .expect("a backend-mode service");
    let gate = Arc::new(JobGate::new());
    service.set_job_gate(Arc::clone(&gate));
    let pool = LoaderPool::start(service.clone()).expect("a loader pool");
    let bridge = SceneBridge::new(Filter::Nearest);
    let mut scene = bridge.take_writer().expect("the one writer");
    let mut textures = Vec::new();
    let mut quads = Vec::new();
    let mut lane = None;
    match plan {
        Plan::Lifecycle | Plan::MovementReupload | Plan::EarlyClose => {
            gate.hold("a.png");
            gate.hold("b.png");
            for (name, at) in [("a.png", FIRST_AT), ("b.png", SECOND_AT)] {
                let texture = service.load(name).expect("the load is admitted");
                let reference = service.retain(texture).expect("a live texture");
                quads.push(
                    scene
                        .create_quad(reference, at.0, at.1, QUAD_SIDE, QUAD_SIDE)
                        .expect("a quad"),
                );
                textures.push(texture);
            }
        }
        Plan::Legacy => {
            let legacy = LegacyLane::new(service.clone(), Some(Assets::new(&root)));
            legacy
                .quad(
                    &scene,
                    "legacy.png",
                    FIRST_AT.0,
                    FIRST_AT.1,
                    Some(QUAD_SIDE),
                    Some(QUAD_SIDE),
                )
                .expect("the legacy quad");
            lane = Some(legacy);
        }
    }
    // Before the window exists: the scene is published to the sink at the attach.
    assert!(scene.publish());
    // A service wake before the presenter exists stays pending in the relay until the backend connects it.
    (relay.wake())();

    let probe = Arc::new(Probe::default());
    let wake: Arc<dyn Fn() + Send + Sync> = {
        let notifier = Arc::clone(&notifier);
        Arc::new(move || notifier.wake())
    };
    let stall: Option<(StallGate, Arc<AtomicBool>)> = (plan == Plan::EarlyClose).then(|| {
        (
            Arc::new((Mutex::new(false), Condvar::new())),
            Arc::new(AtomicBool::new(false)),
        )
    });
    let loader = match load_loader() {
        Ok(loader) => loader,
        Err(error) => {
            eprintln!("worker: no Vulkan loader: {error}");
            return ExitCode::FAILURE;
        }
    };
    let backend = VulkanTestBackend {
        loader: loader.clone(),
        side: VulkanSinkSide::with_resources(
            Arc::clone(&wake),
            service.clone(),
            relay.clone(),
            VulkanResourceLimits {
                max_adoptions_per_frame: 256,
            },
        ),
        probe: Arc::clone(&probe),
        stall: stall.clone(),
        stop_budget: None,
        wake: Arc::clone(&wake),
        started: None,
    };
    let renderer = Renderer::new(backend, Arc::clone(&bridge));
    // The early-close scenario opens the stall gate only after the presenter's stop was forwarded.
    let releaser = stall.clone().map(|(gate, _)| {
        let probe = Arc::clone(&probe);
        thread::spawn(move || {
            let limit = Instant::now() + Duration::from_secs(30);
            while !probe.stop_forwarded.load(Ordering::SeqCst) && Instant::now() < limit {
                thread::yield_now();
            }
            let forwarded = probe.stop_forwarded.load(Ordering::SeqCst);
            if forwarded {
                release(&gate);
            } else {
                // A releaser that timed out fails the case rather than opening the gate silently.
                probe.violate(
                    "the stop was never forwarded to the presenter: the stall gate stays closed",
                );
            }
            forwarded
        })
    });
    let ui = Script {
        probe: Arc::clone(&probe),
        plan,
        service: service.clone(),
        root: root.clone(),
        gate: Arc::clone(&gate),
        scene,
        requests: writer,
        _lane: lane,
        stall: stall.clone(),
        textures,
        quads,
        stage: Stage::First,
        opened: false,
        first_generation: None,
        baseline: None,
        final_seen: None,
        reuploaded: false,
        shift: 0.0,
        answers: Vec::new(),
    };
    let proc_addr = loader.get_instance_proc_addr();
    // SAFETY: both types are the C-ABI `vkGetInstanceProcAddr` signature (a pointer-sized instance handle and a C string in, a
    // function pointer out), so the conversion keeps the callee; it is valid while a clone of `loader` lives.
    let proc_addr = unsafe {
        std::mem::transmute::<ck_gpu_vulkan::GetInstanceProcAddr, ck_window_glfw::GetInstanceProcAddr>(
            proc_addr,
        )
    };
    // SAFETY: `proc_addr` is this loader's own `vkGetInstanceProcAddr`, and the keep-alive is a clone of that loader, which keeps
    // the library loaded for as long as the value lives (the backend holds it until after `glfwTerminate`).
    let for_glfw = unsafe { VulkanLoaderForGlfw::new(proc_addr, Box::new(loader.clone())) };
    let glfw = match GlfwBackend::new(Some(for_glfw)) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("worker: no window backend (a desktop session is required): {error}");
            return ExitCode::FAILURE;
        }
    };
    guard_the_keyboard();
    let (running, ticker) = start_ticker(&notifier);
    let quit = AtomicBool::new(false);
    let end = run(
        glfw,
        renderer,
        ui,
        &reader,
        &notifier,
        &quit,
        &LoopSettings::default(),
    );
    running.store(false, Ordering::Release);
    let _ = ticker.join();
    let mut problems = run_problems(plan, &end, &probe, &bridge, &relay);
    if let Some(releaser) = releaser {
        let forwarded = releaser.join().unwrap_or(false);
        if !forwarded {
            problems.push("the stall gate's releaser timed out".to_owned());
        }
    }
    // The end: every CPU holder is gone before any resource count is claimed zero. The scene writer went with the loop; the
    // bridge is detached, so its last published snapshot is gone; what is left is the worker's own handles. The held file
    // jobs are let go after the close was observed, so the loader shutdown ends within its budget.
    gate.release_all();
    drop(bridge);
    let mut pool = pool;
    let shutdown = pool.shutdown(Duration::from_secs(10));
    if !shutdown.unfinished.is_empty() {
        problems.push(format!("loader threads did not finish: {shutdown:?}"));
    }
    let _ = service.finish_teardown();
    let stats = service.stats();
    if stats.jobs.reserved != 0 || stats.cpu_pixels.reserved != 0 || stats.metadata.reserved != 0 {
        problems.push(format!("a budget is not zero after the end: {stats:?}"));
    }
    if stats.registry.live != 0 || stats.registry.retiring != 0 {
        problems.push(format!(
            "a texture is still live after the end: {:?}",
            stats.registry
        ));
    }
    if stats.loaders.referenced_at_teardown != 0 {
        problems.push(format!(
            "{} resource(s) were still referenced at teardown",
            stats.loaders.referenced_at_teardown
        ));
    }
    let _ = std::fs::remove_dir_all(&root);
    if !typing_was_dropped() {
        problems.push("the typist's keys were never dropped".to_owned());
    }
    if problems.is_empty() {
        activation_watch::assert_clean();
        println!("worker: {name} ended {end:?}");
        ExitCode::SUCCESS
    } else {
        for problem in problems {
            eprintln!("worker: {problem}");
        }
        ExitCode::FAILURE
    }
}

/// What is wrong with how a run went, if anything.
fn run_problems(
    plan: Plan,
    end: &LoopEnd,
    probe: &Probe,
    bridge: &SceneBridge,
    relay: &WakeRelay,
) -> Vec<String> {
    let mut problems: Vec<String> = lock(&probe.violations).clone();
    if *end != LoopEnd::Closed {
        problems.push(format!(
            "the loop ended with {end:?}, not Closed (a proven teardown)"
        ));
    }
    let failures = probe.failures.load(Ordering::SeqCst);
    if failures != 0 {
        problems.push(format!("{failures} presenter failure(s) were reported"));
    }
    if plan == Plan::EarlyClose {
        let frames = probe.stats().map_or(0, |stats| stats.frames);
        if frames != 0 || probe.captured() != 0 || probe.first_frames.load(Ordering::SeqCst) != 0 {
            problems.push(format!(
                "frames were drawn with the checker held: {frames} counted, {} captured, {} FirstFrameQueued",
                probe.captured(),
                probe.first_frames.load(Ordering::SeqCst)
            ));
        }
    } else if probe.first_frames.load(Ordering::SeqCst) != 1 {
        problems.push(format!(
            "{} FirstFrameQueued reports, not one",
            probe.first_frames.load(Ordering::SeqCst)
        ));
    }
    if CLOSE_REQUESTS.load(Ordering::SeqCst) != 0 {
        problems.push(
            "a close request reached the UI: the keyboard filter let Escape through".to_owned(),
        );
    }
    if bridge.phase() != Phase::Ended {
        problems.push(format!("the bridge is {:?}, not Ended", bridge.phase()));
    }
    if !relay_is_disconnected(relay) {
        problems.push("the service's wake relay is still connected after the end".to_owned());
    }
    problems
}
