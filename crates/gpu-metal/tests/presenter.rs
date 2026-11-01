//! The Metal presenter, tested on real windows under Metal validation and the Main Thread Checker, as a
//! supervisor and a worker from this one executable (`harness = false`, like `gpu_probe`).
//!
//! Every case runs in a **worker**: this executable started again with `--worker <case>`, in its own
//! process group, with `MTL_DEBUG_LAYER=1`, `MTL_SHADER_VALIDATION=1` and Apple's Main Thread Checker
//! (`libMainThreadChecker.dylib` from the selected Xcode) inserted from the first instant, and
//! `MTC_CRASH_ON_REPORT=1`. The worker first checks that the checker is really loaded. The **supervisor**
//! owns each worker from the instant it is spawned, gives it 30 s, and on expiry kills its process group,
//! reaps it and fails the case; every helper it runs (`kill`, `ps`, `xcode-select`, `nm`) is owned and
//! bounded the same way. A worker fails (nothing is skipped) without a GPU, a desktop session, the
//! checker library, with any checker or validation report on stderr, and with any failed assertion.
//!
//! Windows come from `ck-window-cocoa` and are driven by a small loop in the test that ignores close
//! events. **The owner may be typing while this runs, Escape included:** the worker installs, before its
//! first window, a test-only filter that drops every keyboard event (these tests post none), and every
//! case runs twice, once on a quiet keyboard and once with a thread typing without stopping. The filter
//! and the typist live in this test's own native archive (`tests/native`), which only the test
//! executable links; a supervisor check proves the library's archive has none of it.

use std::collections::BTreeMap;
use std::env;
use std::ffi::{CStr, OsString, c_char, c_void};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ck_gpu_metal::{
    CleanupResult, ErrorPoll, Gate, MetalPresenter, NeverComplete, Observer, PauseSite,
    PresenterReport, TestHooks, readback_clear,
};
use ck_window::{Backend, WindowDesc};
use ck_window_cocoa::CocoaBackend;
#[path = "../../window/tests/support/activation_watch.rs"]
mod activation_watch;

/// The textured quad's cases (CK-91): off-screen readbacks and presenters with a quad slot.
#[path = "presenter_quad/cases.rs"]
mod quad;

/// The instanced scene's off-screen cases (CK-116): `readback_scene_*` draw through the presenter's own encoding.
#[path = "presenter_instanced/cases.rs"]
mod instanced;

/// Drawing through the bindless table (CK-128), off screen: one draw for a homogeneous scene, painter order, the checker, bounds
/// and chunks, each compared with the ordinary path.
#[path = "presenter_bindless/cases.rs"]
mod bindless;

/// The presenter's frame-timing windows (CK-129): the marks, the window's frames, the timestamp rules and the delayed adoption and
/// results, on a real window.
#[path = "presenter_metrics/cases.rs"]
mod metrics;

/// The scene presenter's cases (CK-114): the shared device context, the texture driver and its wakes, frame
/// tickets, the combined proof and the setup rollback.
#[path = "presenter_scene/cases.rs"]
mod scene;

const WORKER_FLAG: &str = "--worker";
/// `1` makes a worker run with the owner typing without stopping.
const TYPING_VARIABLE: &str = "CK_METAL_PRESENTER_TYPING";
/// The longest a worker may run.
const DEADLINE: Duration = Duration::from_secs(30);
static WORKER_STARTED: OnceLock<Instant> = OnceLock::new();
/// How long the supervisor waits for output pipes, for an emptied group and for each reap attempt.
const PIPE_GRACE: Duration = Duration::from_secs(2);
const REAP_GRACE: Duration = Duration::from_secs(1);
/// What the Main Thread Checker prints for a violation.
const CHECKER_REPORT: &str = "Main Thread Checker";
/// What Metal's validation layers print when something is wrong (they also abort the process).
const VALIDATION_MARKERS: [&str; 4] = [
    "failed assertion",
    "MTLDebug",
    "Validation failed",
    "validation error",
];

// ---------------------------------------------------------------------------------------------
// The test natives (tests/native/testing.m) and the checker check
// ---------------------------------------------------------------------------------------------

unsafe extern "C" {
    fn _dyld_image_count() -> u32;
    fn _dyld_get_image_name(index: u32) -> *const c_char;
    fn ck_t_install_input_filter();
    fn ck_t_dropped_keys() -> u64;
    fn ck_t_start_typist();
    fn ck_t_set_rig_layer(layer: *mut c_void);
    fn ck_t_miniaturize(match_count: *mut u64) -> i32;
    fn ck_t_is_miniaturized(match_count: *mut u64) -> i32;
    fn ck_t_test_window_lookup(mode: u32, match_count: *mut u64) -> i32;
    fn ck_t_open_minimise_decoy(
        decoy_window_number: *mut u64,
        first_object_window_number: *mut u64,
        first_object_matches_rig: *mut u64,
    ) -> i32;
    fn ck_t_minimise_decoy_explicitly(
        target_window_number: *mut u64,
        target_matches_rig_layer: *mut u64,
    );
    fn ck_t_decoy_is_miniaturized() -> i32;
    fn ck_t_close_minimise_decoy();
    fn ck_t_rig_is_miniaturized() -> i32;
    fn ck_t_print_minimise_diagnostics(
        phase: *const c_char,
        observed_ms: u64,
        observed: i32,
        truncated: i32,
    );
    fn ck_t_violate_main_thread();
    fn ck_t_watch_rig_occlusion() -> i32;
    fn ck_t_rig_ever_occluded() -> i32;
    fn ck_t_rig_occlusion_state() -> i64;
}

/// Whether `libMainThreadChecker.dylib` is among the images loaded into this process.
fn checker_loaded() -> bool {
    // SAFETY: both functions are dyld's documented image enumeration; they are called on the main
    // thread before any other thread exists.
    let count = unsafe { _dyld_image_count() };
    (0..count).any(|index| {
        // SAFETY: `index` is below the image count; the returned string is NUL-terminated and valid
        // for the process's life (or null, which is handled).
        let name = unsafe { _dyld_get_image_name(index) };
        // SAFETY: a non-null pointer from dyld is a valid C string.
        !name.is_null()
            && unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .ends_with("libMainThreadChecker.dylib")
    })
}

/// Whether the window is miniaturized.
fn miniaturized() -> bool {
    let mut match_count = 0;
    // SAFETY: main thread; resolves the rig's live window by its content layer and reads one property.
    let state = unsafe { ck_t_is_miniaturized(&raw mut match_count) };
    assert_ne!(
        state, -1,
        "the rig window lookup found {match_count} matching windows; expected exactly one"
    );
    state != 0
}

fn typing() -> bool {
    env::var_os(TYPING_VARIABLE).is_some_and(|value| value == "1")
}

// ---------------------------------------------------------------------------------------------
// The rig: a real window, a loop that ignores close events, and a presenter on its layer
// ---------------------------------------------------------------------------------------------

const BOUND: Duration = Duration::from_secs(10);

struct Rig {
    backend: CocoaBackend,
    layer: *mut c_void,
    pixels: (u32, u32),
    /// Set when a presenter may still be using the layer (an unproven end): the window is then left to
    /// the process's end.
    keep_alive: bool,
}

impl Rig {
    fn open() -> Self {
        let mut backend = CocoaBackend::new().expect("the backend is created on the main thread");
        // SAFETY: main thread; installs one local monitor, once, before the first window.
        unsafe { ck_t_install_input_filter() };
        if typing() {
            // SAFETY: main thread, after the filter; starts one thread that only posts events.
            unsafe { ck_t_start_typist() };
        }
        let created = backend
            .create_window(&WindowDesc {
                title: "Codekarl presenter test".to_owned(),
                width: 200,
                height: 150,
                clear: [0.0; 3],
            })
            .expect("the window is created (a desktop session is required)");
        let mut rig = Self {
            layer: created.handles.layer(),
            pixels: (created.pixel_size.width, created.pixel_size.height),
            backend,
            keep_alive: false,
        };
        // SAFETY: the Cocoa window owns this live layer for the rig's lifetime; the test-support archive
        // only compares the pointer while reporting this worker's diagnostics.
        unsafe { ck_t_set_rig_layer(rig.layer) };
        rig.pump_for(Duration::from_millis(300));
        rig
    }

    /// One pass of the loop: close events are ignored, never acted on.
    fn pump_once(&mut self) {
        self.backend
            .pump(&mut |_event| {}, ck_window::Wait::Poll, 64);
        thread::sleep(Duration::from_millis(2));
    }

    fn pump_for(&mut self, within: Duration) {
        let end = Instant::now() + within;
        while Instant::now() < end {
            self.pump_once();
        }
    }

    /// Pumps until `done` holds; `false` when `within` passed first.
    fn pump_until(&mut self, within: Duration, mut done: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + within;
        loop {
            if done() {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            self.pump_once();
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if !self.keep_alive {
            self.backend.destroy_window();
            self.backend.terminate();
        }
    }
}

/// What a case watches of a presenter.
struct Probes {
    wakes: Arc<AtomicUsize>,
    frames: Arc<AtomicU64>,
    observer: Arc<Observer>,
    log: Arc<Mutex<Vec<[f32; 3]>>>,
    started: Arc<AtomicBool>,
}

impl Probes {
    fn new() -> Self {
        Self {
            wakes: Arc::new(AtomicUsize::new(0)),
            frames: Arc::new(AtomicU64::new(0)),
            observer: Arc::new(Observer::new()),
            log: Arc::new(Mutex::new(Vec::new())),
            started: Arc::new(AtomicBool::new(false)),
        }
    }

    fn hooks(&self) -> TestHooks {
        TestHooks {
            frames: Some(Arc::clone(&self.frames)),
            observer: Some(Arc::clone(&self.observer)),
            clear_log: Some(Arc::clone(&self.log)),
            thread_started: Some(Arc::clone(&self.started)),
            ..TestHooks::default()
        }
    }

    fn wake(&self) -> Arc<dyn Fn() + Send + Sync> {
        let wakes = Arc::clone(&self.wakes);
        Arc::new(move || {
            wakes.fetch_add(1, Ordering::SeqCst);
        })
    }

    fn frames(&self) -> u64 {
        self.frames.load(Ordering::SeqCst)
    }

    fn start(&self, rig: &Rig, hooks: TestHooks) -> Result<MetalPresenter, ck_gpu_metal::Error> {
        // SAFETY: `rig.layer` is the live `CAMetalLayer` of the rig's window, which `Rig` keeps (it
        // is destroyed only when the rig drops, after every presenter of a case ended `Proven`, or it
        // is left alone by `keep_alive`).
        unsafe {
            MetalPresenter::start_with_test_hooks(
                rig.layer,
                rig.pixels,
                [0.1, 0.2, 0.3],
                self.wake(),
                hooks,
            )
        }
    }

    fn start_ok(&self, rig: &Rig, hooks: TestHooks) -> MetalPresenter {
        self.start(rig, hooks).expect("the presenter starts")
    }
}

/// Collects the reports a presenter made, pumping the window meanwhile.
fn reports_until(
    rig: &mut Rig,
    presenter: &MetalPresenter,
    reports: &mut Vec<PresenterReport>,
    wanted: usize,
) -> bool {
    rig.pump_until(BOUND, || {
        while let Some(report) = presenter.take_report() {
            reports.push(report);
        }
        reports.len() >= wanted
    })
}

/// Stops the presenter and waits (bounded, pumping) for its end.
fn end(rig: &mut Rig, presenter: &mut MetalPresenter) -> CleanupResult {
    presenter.request_stop();
    let mut result = None;
    assert!(
        rig.pump_until(BOUND, || {
            result = presenter.poll_end(Duration::ZERO);
            result.is_some()
        }),
        "the presenter never ended"
    );
    result.expect("a result")
}

fn near(a: u8, b: u8) -> bool {
    a.abs_diff(b) <= 1
}

// ---------------------------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------------------------

/// 1. The off-screen clear reads back exactly the colour, with no window.
fn readback() {
    let pixels = readback_clear(16, 8, [0.25, 0.5, 0.75]).expect("the readback works");
    assert_eq!(pixels.len(), 16 * 8 * 4);
    for pixel in pixels.chunks_exact(4) {
        // BGRA8: blue 0.75, green 0.5, red 0.25, opaque.
        assert!(
            near(pixel[0], 191) && near(pixel[1], 128) && near(pixel[2], 64) && pixel[3] == 255,
            "{pixel:?}"
        );
    }
    assert!(
        readback_clear(0, 8, [0.0; 3]).is_err(),
        "an empty size is refused"
    );
}

/// 2. The first frame is reported once, and the wake is called.
fn first_frame() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut presenter = probes.start_ok(&rig, probes.hooks());
    let mut reports = Vec::new();
    assert!(reports_until(&mut rig, &presenter, &mut reports, 1));
    assert_eq!(reports, [PresenterReport::FirstFrameQueued]);
    rig.pump_for(Duration::from_millis(300));
    while let Some(report) = presenter.take_report() {
        reports.push(report);
    }
    assert_eq!(reports.len(), 1, "once: {reports:?}");
    assert!(
        probes.wakes.load(Ordering::SeqCst) >= 1,
        "the wake was called"
    );
    assert_eq!(probes.frames(), 1);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// 3. Redraw on demand.
fn redraw_on_demand() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut presenter = probes.start_ok(&rig, probes.hooks());
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    rig.pump_for(Duration::from_millis(300));
    assert_eq!(probes.frames(), 1, "no change gives no frame");
    let mut reports = Vec::new();

    presenter.set_clear([0.9, 0.1, 0.1]);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 2));
    rig.pump_for(Duration::from_millis(300));
    assert_eq!(probes.frames(), 2, "one change gives one new frame");

    presenter.set_clear([0.9, 0.1, 0.1]);
    presenter.set_pixel_size(rig.pixels.0, rig.pixels.1);
    rig.pump_for(Duration::from_millis(300));
    assert_eq!(probes.frames(), 2, "the same colour and size give no frame");

    let before = probes.frames();
    for step in 0..100_u8 {
        presenter.set_clear([f32::from(step) / 100.0, 0.5, 0.25]);
    }
    rig.pump_for(Duration::from_millis(700));
    let burst = probes.frames() - before;
    assert!(
        (1..=8).contains(&burst),
        "a burst of 100 changes gave {burst} frames"
    );
    assert_eq!(
        probes.observer.command_buffers_in_flight_max(),
        1,
        "never more than one command buffer in flight"
    );
    while let Some(report) = presenter.take_report() {
        reports.push(report);
    }
    assert_eq!(
        reports,
        [PresenterReport::FirstFrameQueued],
        "reported once, however many frames"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// 4. A pixel-size change makes the next drawable that size (set on the render thread).
fn pixel_size() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut presenter = probes.start_ok(&rig, probes.hooks());
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let first = probes.observer.last_drawable_size();
    assert_eq!(first, (i64::from(rig.pixels.0), i64::from(rig.pixels.1)));
    presenter.set_pixel_size(120, 90);
    assert!(
        rig.pump_until(BOUND, || probes.observer.last_drawable_size() == (120, 90)),
        "the next drawable has the new size: {:?}",
        probes.observer.last_drawable_size()
    );
    presenter.set_pixel_size(rig.pixels.0, rig.pixels.1);
    assert!(rig.pump_until(BOUND, || probes.observer.last_drawable_size() == first));
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// 5. A visible presenter tears down `Proven`, and every presented handler ran.
fn proven_teardown_visible() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut presenter = probes.start_ok(&rig, probes.hooks());
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    presenter.set_clear([0.2, 0.7, 0.3]);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 2));
    let started = Instant::now();
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert!(
        started.elapsed() < Duration::from_millis(2000),
        "within the budget"
    );
    let observer = &probes.observer;
    assert_eq!(
        observer.presented_handlers_run(),
        observer.drawables_acquired()
    );
    assert_eq!(observer.completion_handlers_run(), observer.committed());
    assert!(
        observer.holders_released() >= 3,
        "the device, queue and layer were released"
    );
    assert_eq!(
        observer.layer_retains_outstanding(),
        0,
        "the layer retain was released"
    );
    assert_eq!(observer.drawable_refs(), 0);
    assert_eq!(
        observer.buffer_holders_now(),
        0,
        "the command buffer holder was released by the explicit teardown"
    );
}

/// 6. A drawable acquired and abandoned still gives `Proven`, because its presented handler runs.
fn proven_teardown_abandoned_drawable() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let hooks = TestHooks {
        abandon_frames: 1,
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(
        rig.pump_until(BOUND, || probes.frames() >= 1),
        "a later frame is drawn"
    );
    assert!(
        probes.observer.drawables_acquired() >= 2,
        "the abandoned drawable and a drawn one: {}",
        probes.observer.drawables_acquired()
    );
    assert_eq!(
        probes.observer.drawable_refs(),
        0,
        "no strong drawable reference between steps"
    );
    assert!(
        rig.pump_until(BOUND, || !probes.observer.abandoned_drawable_alive()),
        "the abandoned drawable can be released by everyone else"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    let observer = &probes.observer;
    assert_eq!(
        observer.presented_handlers_run(),
        observer.drawables_acquired()
    );
}

/// 7. Frames presented while the window is minimised still give `Proven`.
fn proven_teardown_minimised() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut presenter = probes.start_ok(&rig, probes.hooks());
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let mut match_count = 0;
    // SAFETY: main thread; resolves the rig's live window by its content layer before minimising it.
    let found = unsafe { ck_t_miniaturize(&raw mut match_count) };
    assert_eq!(
        found, 1,
        "the rig window lookup found {match_count} matching windows; expected exactly one"
    );
    if !rig.pump_until(BOUND, miniaturized) {
        // Save the failure state before the report-only observation can change it. The native helper
        // flushes stderr so the supervisor retains this block even if its worker deadline later expires.
        // SAFETY: called on the main thread with a static, NUL-terminated phase label.
        unsafe { ck_t_print_minimise_diagnostics(c"minimised wait failed".as_ptr(), 0, 0, 0) };
        let observed_started = Instant::now();
        let remaining = DEADLINE.saturating_sub(
            WORKER_STARTED
                .get()
                .map_or(Duration::ZERO, Instant::elapsed),
        );
        let observation_budget =
            Duration::from_secs(10).min(remaining.saturating_sub(Duration::from_secs(3)));
        let observed = rig.pump_until(observation_budget, || {
            // SAFETY: main thread; this diagnostic-only helper queries the rig's layer, not the old
            // first-window target used by the production test helper.
            unsafe { ck_t_rig_is_miniaturized() != 0 }
        });
        // SAFETY: called on the main thread with a static, NUL-terminated phase label.
        unsafe {
            ck_t_print_minimise_diagnostics(
                c"observed until T".as_ptr(),
                u64::try_from(observed_started.elapsed().as_millis()).unwrap_or(u64::MAX),
                i32::from(observed),
                i32::from(!observed && observation_budget < Duration::from_secs(10)),
            );
        };
        panic!("minimised");
    }
    let before = probes.frames();
    presenter.set_clear([0.6, 0.2, 0.8]);
    assert!(
        rig.pump_until(BOUND, || probes.frames() > before),
        "a frame while minimised"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    let observer = &probes.observer;
    assert_eq!(
        observer.presented_handlers_run(),
        observer.drawables_acquired()
    );
}

struct MinimiseDecoy;

impl Drop for MinimiseDecoy {
    fn drop(&mut self) {
        // SAFETY: the native test support owns the decoy until this guard drops on the main thread.
        unsafe { ck_t_close_minimise_decoy() };
    }
}

/// The minimisation lookup rejects zero and duplicate layer matches and selects the rig over an explicit decoy.
fn the_minimise_helpers_address_the_rigs_window() {
    let mut rig = Rig::open();

    let mut matches = u64::MAX;
    // SAFETY: main thread; mode 0 supplies the real window list and a fresh unhosted layer to the shared selector.
    let zero_ok = unsafe { ck_t_test_window_lookup(0, &raw mut matches) };
    assert_eq!(zero_ok, 0, "zero matches must be rejected");
    assert_eq!(matches, 0, "zero-match count");

    // SAFETY: main thread; mode 1 supplies NSApp.windows, which contains the rig window exactly once.
    let one_ok = unsafe { ck_t_test_window_lookup(1, &raw mut matches) };
    assert_eq!(one_ok, 1, "one match must select the rig window");
    assert_eq!(matches, 1, "one-match count");

    // SAFETY: main thread; mode 2 supplies the same rig window twice to the shared selector.
    let several_ok = unsafe { ck_t_test_window_lookup(2, &raw mut matches) };
    assert_eq!(several_ok, 0, "several matches must be rejected");
    assert_eq!(matches, 2, "multiple-match count");

    let mut decoy_window_number = u64::MAX;
    let mut first_object_window_number = u64::MAX;
    let mut first_object_matches_rig = u64::MAX;
    // SAFETY: main thread; creates one borderless decoy after the keyboard filter is installed by Rig::open.
    let opened = unsafe {
        ck_t_open_minimise_decoy(
            &raw mut decoy_window_number,
            &raw mut first_object_window_number,
            &raw mut first_object_matches_rig,
        )
    };
    assert_eq!(opened, 1, "the borderless decoy opens");
    let _decoy = MinimiseDecoy;
    eprintln!(
        "CK136_MINIMISE_CONTROL first_object_window_number={first_object_window_number} first_object_matches_rig={first_object_matches_rig}"
    );

    let mut decoy_matches = 0;
    // SAFETY: main thread; the explicit decoy is still owned by native test support.
    let decoy_lookup = unsafe { ck_t_test_window_lookup(1, &raw mut decoy_matches) };
    assert_eq!(decoy_lookup, 1, "the real window list has one rig match");
    assert_eq!(
        decoy_matches, 1,
        "adding a decoy does not add a rig-layer match"
    );

    let mut wrong_target_window_number = u64::MAX;
    let mut wrong_target_matches_rig_layer = u64::MAX;
    // SAFETY: main thread; this deterministic wrong-target control explicitly miniaturises the non-miniaturisable decoy.
    unsafe {
        ck_t_minimise_decoy_explicitly(
            &raw mut wrong_target_window_number,
            &raw mut wrong_target_matches_rig_layer,
        );
    };
    assert_eq!(
        wrong_target_window_number, decoy_window_number,
        "the wrong-target control called miniaturize on the decoy"
    );
    assert_eq!(
        wrong_target_matches_rig_layer, 0,
        "the recorded decoy target does not match the rig layer"
    );

    assert!(
        !rig.pump_until(BOUND, miniaturized),
        "the wrong-target control leaves the rig window not miniaturized within the bound"
    );
    // SAFETY: main thread; reads the decoy after the explicit wrong-target control.
    let decoy_minimized = unsafe { ck_t_decoy_is_miniaturized() };
    assert_eq!(decoy_minimized, 0, "the decoy refuses minimisation");

    let mut rig_matches = 0;
    // SAFETY: main thread; selects the unique rig-layer window and minimises it.
    let selected = unsafe { ck_t_miniaturize(&raw mut rig_matches) };
    assert_eq!(selected, 1, "the fixed helper selects one rig window");
    assert_eq!(
        rig_matches, 1,
        "the fixed helper reports the rig match count"
    );
    assert!(
        rig.pump_until(BOUND, miniaturized),
        "the rig window is minimised"
    );
}

/// 8a. A panic on the render thread is `Unproven` and releases nothing.
fn unproven_after_a_panic() {
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let probes = Probes::new();
    let hooks = TestHooks {
        panic_after_frames: Some(1),
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    let mut result = None;
    assert!(rig.pump_until(BOUND, || {
        result = presenter.poll_end(Duration::ZERO);
        result.is_some()
    }));
    match result.expect("a result") {
        CleanupResult::Unproven { reason } => assert!(reason.contains("panicked"), "{reason}"),
        CleanupResult::Proven => panic!("a panic must not be proven"),
    }
    assert_eq!(
        probes.observer.holders_released(),
        0,
        "nothing was released"
    );
    assert_eq!(
        probes.observer.layer_retains_outstanding(),
        1,
        "the layer retain was kept"
    );
}

/// How many times the payload below was dropped.
static BOMB_DROPS: AtomicUsize = AtomicUsize::new(0);

/// A panic payload whose `Drop` panics.
struct Bomb;

impl Drop for Bomb {
    fn drop(&mut self) {
        BOMB_DROPS.fetch_add(1, Ordering::SeqCst);
        panic!("a payload whose drop panics");
    }
}

fn raise_bomb() -> ! {
    std::panic::panic_any(Bomb)
}

/// 8a'. The same panic with a payload whose `Drop` panics (CK-102): the thread still ends with its result,
/// the same `Unproven` with the same reason, nothing released, and the wake runs after the result.
fn unproven_after_a_panic_whose_payload_drop_panics() {
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let probes = Probes::new();
    let hooks = TestHooks {
        panic_after_frames: Some(1),
        forced_panic_payload: Some(raise_bomb),
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    let mut result = None;
    assert!(rig.pump_until(BOUND, || {
        result = presenter.poll_end(Duration::ZERO);
        result.is_some()
    }));
    assert_eq!(
        result.expect("a result"),
        CleanupResult::Unproven {
            reason: "the render thread panicked; everything it owned was kept".to_owned()
        }
    );
    assert_eq!(
        BOMB_DROPS.load(Ordering::SeqCst),
        1,
        "the payload was dropped once"
    );
    assert!(
        probes.wakes.load(Ordering::SeqCst) >= 2,
        "the first report's wake and the result's"
    );
    assert_eq!(
        probes.observer.holders_released(),
        0,
        "nothing was released"
    );
    assert_eq!(
        probes.observer.layer_retains_outstanding(),
        1,
        "the layer retain was kept"
    );
}

/// 8b. A teardown budget that expires before the handlers arrive is `Unproven` and releases nothing.
fn unproven_when_the_budget_expires() {
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let probes = Probes::new();
    let hooks = TestHooks {
        never_complete: NeverComplete::Everything,
        stop_budget: Some(Duration::from_millis(300)),
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let started = Instant::now();
    match end(&mut rig, &mut presenter) {
        CleanupResult::Unproven { reason } => assert!(reason.contains("outstanding"), "{reason}"),
        CleanupResult::Proven => panic!("lost callbacks must not be proven"),
    }
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(250) && took < Duration::from_secs(3),
        "{took:?}"
    );
    assert_eq!(
        probes.observer.holders_released(),
        0,
        "nothing was released"
    );
    assert_eq!(probes.observer.layer_retains_outstanding(), 1);
}

/// 9. Start failures: before the thread, and on it.
fn start_failure() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let hooks = TestHooks {
        fail_start: true,
        ..probes.hooks()
    };
    assert!(
        probes.start(&rig, hooks).is_err(),
        "a forced failure before spawning is an Err"
    );
    assert!(
        !probes.started.load(Ordering::SeqCst),
        "no thread was spawned"
    );
    assert_eq!(
        probes.observer.layer_retains_outstanding(),
        0,
        "the layer retain was undone"
    );
    assert_eq!(probes.observer.drawables_acquired(), 0);

    let on_thread = Probes::new();
    let hooks = TestHooks {
        fail_setup: true,
        ..on_thread.hooks()
    };
    let mut presenter = on_thread.start_ok(&rig, hooks);
    let mut reports = Vec::new();
    assert!(reports_until(&mut rig, &presenter, &mut reports, 1));
    match &reports[0] {
        PresenterReport::Failed { message } => {
            assert!(message.contains("No Metal device"), "{message}");
        }
        other @ PresenterReport::FirstFrameQueued => panic!("expected Failed, got {other:?}"),
    }
    // A setup failure ends at once, without request_stop, with a cleanup result.
    let mut result = None;
    assert!(rig.pump_until(BOUND, || {
        result = presenter.poll_end(Duration::ZERO);
        result.is_some()
    }));
    assert_eq!(result, Some(CleanupResult::Proven));
    assert_eq!(on_thread.observer.layer_retains_outstanding(), 0);
    assert_eq!(on_thread.frames(), 0);
}

/// 10. Dropping the handle without a stop never blocks; the thread then stops itself.
fn handle_drop_never_blocks() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let presenter = probes.start_ok(&rig, probes.hooks());
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let started = Instant::now();
    drop(presenter);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the drop did not wait"
    );
    assert!(
        rig.pump_until(BOUND, || probes.observer.holders_released() > 0),
        "the thread stopped itself and released what it owned"
    );
}

/// Ownership: the presenter holds no drawable between steps, and a handler holds none at all.
fn ownership() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let hooks = TestHooks {
        abandon_frames: 1,
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    for step in 0..5_u8 {
        let before = probes.frames();
        presenter.set_clear([f32::from(step) / 5.0, 0.3, 0.3]);
        assert!(rig.pump_until(BOUND, || probes.frames() > before));
        assert_eq!(
            probes.observer.drawable_refs(),
            0,
            "zero strong references after each step"
        );
    }
    let observer = &probes.observer;
    assert_eq!(
        observer.drawable_refs_max(),
        1,
        "at most one at a time, within a step"
    );
    assert!(
        observer.drawable_tokens_max() <= 3,
        "{}",
        observer.drawable_tokens_max()
    );
    assert!(
        rig.pump_until(BOUND, || !observer.abandoned_drawable_alive()),
        "no handler block captured the abandoned drawable"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// Deterministic redraw: with frame progress gated, a burst gives exactly one frame per opened gate, and
/// the last colour wins.
fn deterministic_redraw() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let gate = Arc::new(Gate::new());
    let hooks = TestHooks {
        gate: Some(Arc::clone(&gate)),
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    rig.pump_for(Duration::from_millis(200));
    assert_eq!(probes.frames(), 0, "the gate holds the first frame");
    gate.open(1);
    assert!(rig.pump_until(BOUND, || probes.frames() == 1));
    for step in 0..10_u8 {
        presenter.set_clear([f32::from(step) / 10.0, 0.0, 1.0]);
    }
    rig.pump_for(Duration::from_millis(300));
    assert_eq!(
        probes.frames(),
        1,
        "nothing is drawn while the gate is closed"
    );
    gate.open(1);
    assert!(rig.pump_until(BOUND, || probes.frames() == 2));
    rig.pump_for(Duration::from_millis(200));
    assert_eq!(probes.frames(), 2, "exactly one frame for the opened gate");
    assert_eq!(
        probes
            .log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last()
            .copied(),
        Some([0.9, 0.0, 1.0]),
        "the last colour won"
    );
    presenter.set_clear([0.5, 0.5, 0.5]);
    gate.open(1);
    assert!(rig.pump_until(BOUND, || probes.frames() == 3));
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A fault injected into a presenter's hooks.
type Injection = fn(&mut TestHooks);

/// Frame failures: each reports `Failed`, draws nothing more, and ends `Proven` once its tokens arrive.
fn frame_failures() {
    let mut rig = Rig::open();
    let faults: [(&str, Injection); 5] = [
        ("nil command buffer", |hooks| {
            hooks.nil_command_buffer_frames = 1;
        }),
        ("nil render pass", |hooks| hooks.nil_render_pass_frames = 1),
        ("nil encoder", |hooks| hooks.nil_encoder_frames = 1),
        ("partial encoding", |hooks| {
            hooks.partial_encoding_frames = 1;
        }),
        ("error completion", |hooks| {
            hooks.error_completion_frames = 1;
        }),
    ];
    for (name, inject) in faults {
        let probes = Probes::new();
        let mut hooks = probes.hooks();
        inject(&mut hooks);
        let mut presenter = probes.start_ok(&rig, hooks);
        let mut reports = Vec::new();
        assert!(
            rig.pump_until(BOUND, || {
                while let Some(report) = presenter.take_report() {
                    reports.push(report);
                }
                reports
                    .iter()
                    .any(|report| matches!(report, PresenterReport::Failed { .. }))
            }),
            "{name}: Failed"
        );
        let committed = probes.frames();
        presenter.set_clear([0.9, 0.9, 0.1]);
        presenter.request_redraw();
        rig.pump_for(Duration::from_millis(300));
        assert_eq!(
            probes.frames(),
            committed,
            "{name}: nothing more is drawn after Failed"
        );
        assert_eq!(probes.observer.drawable_refs(), 0, "{name}");
        let result = end(&mut rig, &mut presenter);
        assert_eq!(
            result,
            CleanupResult::Proven,
            "{name}: ends Proven once its tokens arrive"
        );
        let observer = &probes.observer;
        assert_eq!(
            observer.presented_handlers_run(),
            observer.drawables_acquired(),
            "{name}"
        );
        assert_eq!(
            observer.completion_handlers_run(),
            observer.committed(),
            "{name}"
        );
    }
}

/// The stop deadline is absolute: a stop that arrives while `nextDrawable` is blocked shares the budget
/// with the acquisition, and a repeated stop does not extend it.
fn stop_deadline() {
    // The budget, when the second stop is made, and when the blocked acquisition is let go (after the
    // budget ran out), all counted from the first stop.
    const BUDGET: Duration = Duration::from_millis(500);
    const SECOND_STOP_AT: Duration = Duration::from_millis(350);
    const RELEASED_AT: Duration = Duration::from_millis(600);
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let probes = Probes::new();
    // The very first frame is held before `nextDrawable`, so the acquisition is what the stop finds.
    let hold = Arc::new(AtomicI32::new(1));
    let hooks = TestHooks {
        hold_next_drawable: Some(Arc::clone(&hold)),
        never_complete: NeverComplete::Everything,
        stop_budget: Some(BUDGET),
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    rig.pump_for(Duration::from_millis(150));
    assert_eq!(
        probes.observer.drawables_acquired(),
        0,
        "the acquisition is blocked"
    );
    let stopped = Instant::now();
    presenter.request_stop();
    rig.pump_for(SECOND_STOP_AT);
    presenter.request_stop();
    rig.pump_for(RELEASED_AT.saturating_sub(stopped.elapsed()));
    let released = stopped.elapsed();
    hold.store(0, Ordering::SeqCst);
    let mut result = None;
    assert!(rig.pump_until(BOUND, || {
        result = presenter.poll_end(Duration::ZERO);
        result.is_some()
    }));
    let took = stopped.elapsed();
    assert!(
        matches!(result, Some(CleanupResult::Unproven { .. })),
        "{result:?}: the tokens never arrive"
    );
    // One deadline, set by the first stop, covers the blocked acquisition and the teardown: once the
    // acquisition was let go (after the deadline) nothing was left to wait for. A fresh budget would have
    // kept the thread for another 500 ms, and a second stop that moved the deadline for 350 ms more.
    assert!(
        took < released + Duration::from_millis(150),
        "ended after {took:?}, the acquisition was released at {released:?}"
    );
    assert_eq!(probes.observer.holders_released(), 0);
}

/// One frame in flight: while the previous command buffer has not completed, a wanted redraw stays
/// pending and nothing more is committed.
fn one_frame_in_flight() {
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let probes = Probes::new();
    let hooks = TestHooks {
        never_complete: NeverComplete::Everything,
        stop_budget: Some(Duration::from_millis(200)),
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    for step in 0..5_u8 {
        presenter.set_clear([f32::from(step) / 5.0, 0.4, 0.4]);
        presenter.request_redraw();
    }
    rig.pump_for(Duration::from_millis(500));
    assert_eq!(
        probes.frames(),
        1,
        "the first command buffer never completed: nothing more is committed"
    );
    assert_eq!(probes.observer.command_buffers_in_flight_max(), 1);
    assert!(matches!(
        end(&mut rig, &mut presenter),
        CleanupResult::Unproven { .. }
    ));
}

fn failure_count(reports: &[PresenterReport]) -> usize {
    reports
        .iter()
        .filter(|report| matches!(report, PresenterReport::Failed { .. }))
        .count()
}

/// Collects what is left to take after a presenter ended.
fn rest(presenter: &MetalPresenter, reports: &mut Vec<PresenterReport>) {
    while let Some(report) = presenter.take_report() {
        reports.push(report);
    }
}

/// A presenter whose first frame completes with an error, started with `error_poll`; returns once that
/// frame was committed and its completion handler ran (so the token is set), with only the first-frame
/// report taken.
fn presenter_with_a_failed_completion(
    rig: &mut Rig,
    probes: &Probes,
    error_poll: ErrorPoll,
    seen: Option<Arc<AtomicI32>>,
) -> (MetalPresenter, Vec<PresenterReport>) {
    let hooks = TestHooks {
        error_completion_frames: 1,
        error_poll,
        outstanding_seen: seen,
        ..probes.hooks()
    };
    let presenter = probes.start_ok(rig, hooks);
    assert!(
        rig.pump_until(BOUND, || probes.observer.completion_handlers_run() >= 1),
        "the completion handler ran"
    );
    // The handler sets its token a moment after it counts itself.
    rig.pump_for(Duration::from_millis(200));
    let mut reports = Vec::new();
    assert!(reports_until(rig, &presenter, &mut reports, 1));
    (presenter, reports)
}

/// B1. A command buffer's completion error is never lost, however the completion falls between the
/// operations that look at tokens, and it is reported exactly once, drawing nothing more.
fn completion_error_never_lost() {
    let mut rig = Rig::open();

    // (a) The completion has arrived, then the thread asks whether anything is outstanding (which used to
    // drop the buffer and its error), then it polls.
    let probes = Probes::new();
    let seen = Arc::new(AtomicI32::new(-1));
    let (mut presenter, mut reports) = presenter_with_a_failed_completion(
        &mut rig,
        &probes,
        ErrorPoll::OutstandingFirst,
        Some(Arc::clone(&seen)),
    );
    assert!(
        reports_until(&mut rig, &presenter, &mut reports, 2),
        "a: {reports:?}"
    );
    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "a: with every token arrived and the error not yet taken, the look must still say outstanding"
    );
    presenter.set_clear([0.9, 0.9, 0.1]);
    presenter.request_redraw();
    rig.pump_for(Duration::from_millis(300));
    rest(&presenter, &mut reports);
    assert_eq!(failure_count(&reports), 1, "a: exactly once: {reports:?}");
    assert_eq!(probes.frames(), 1, "a: nothing more is drawn");
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    rest(&presenter, &mut reports);
    assert_eq!(failure_count(&reports), 1, "a: still once: {reports:?}");

    // (b) The thread never polls while running; the next frame step finds the latched error, reports it
    // and draws nothing.
    let probes = Probes::new();
    let (mut presenter, mut reports) =
        presenter_with_a_failed_completion(&mut rig, &probes, ErrorPoll::Never, None);
    rig.pump_for(Duration::from_millis(200));
    rest(&presenter, &mut reports);
    assert_eq!(
        reports,
        [PresenterReport::FirstFrameQueued],
        "b: nothing polled, nothing reported yet"
    );
    presenter.set_clear([0.9, 0.9, 0.1]);
    assert!(
        reports_until(&mut rig, &presenter, &mut reports, 2),
        "b: the next frame step reports it: {reports:?}"
    );
    rig.pump_for(Duration::from_millis(200));
    rest(&presenter, &mut reports);
    assert_eq!(failure_count(&reports), 1, "b: exactly once: {reports:?}");
    assert_eq!(probes.frames(), 1, "b: the frame step drew nothing");
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    rest(&presenter, &mut reports);
    assert_eq!(failure_count(&reports), 1, "b: still once: {reports:?}");

    // (c) The thread never polls while running and no frame follows; the teardown reports it, before it
    // releases anything.
    let probes = Probes::new();
    let (mut presenter, mut reports) =
        presenter_with_a_failed_completion(&mut rig, &probes, ErrorPoll::Never, None);
    rig.pump_for(Duration::from_millis(200));
    rest(&presenter, &mut reports);
    assert_eq!(reports, [PresenterReport::FirstFrameQueued], "c");
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    rest(&presenter, &mut reports);
    assert_eq!(
        failure_count(&reports),
        1,
        "c: the teardown reported it: {reports:?}"
    );
    assert_eq!(probes.frames(), 1, "c");

    // (d) The error arrives during the teardown: the completion handler waits 400 ms before it sets its
    // token, and the stop comes right after the commit.
    let probes = Probes::new();
    let hooks = TestHooks {
        error_completion_frames: 1,
        completion_delay: Duration::from_millis(400),
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let stopped = Instant::now();
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    let took = stopped.elapsed();
    assert!(
        took >= Duration::from_millis(250) && took < Duration::from_millis(1400),
        "d: the teardown waited for the late completion: {took:?}"
    );
    let mut reports = Vec::new();
    rest(&presenter, &mut reports);
    assert_eq!(
        failure_count(&reports),
        1,
        "d: reported once, from the teardown: {reports:?}"
    );
    assert_eq!(probes.frames(), 1, "d");
    assert_eq!(
        probes.observer.buffer_holders_now(),
        0,
        "d: released at the end"
    );
}

/// B2. A partial proof keeps every holder: the stop deadline passes with only one kind of token
/// arrived, and the command buffer holder (and everything else) is still owned.
fn partial_teardown_keeps_holders(lost: NeverComplete) {
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let probes = Probes::new();
    let hooks = TestHooks {
        never_complete: lost,
        stop_budget: Some(Duration::from_millis(300)),
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    let observer = &probes.observer;
    match lost {
        NeverComplete::Drawables => assert!(
            rig.pump_until(BOUND, || observer.completion_handlers_run() >= 1),
            "the command buffer completed"
        ),
        _ => assert!(
            rig.pump_until(BOUND, || observer.presented_handlers_run() >= 1),
            "the drawable was presented"
        ),
    }
    // Idle for a while: every look at the tokens happens, and none may drop the holder.
    rig.pump_for(Duration::from_millis(300));
    assert_eq!(probes.frames(), 1);
    assert_eq!(observer.buffer_holders_now(), 1, "idle: the holder is kept");
    assert_eq!(observer.buffer_drops(), 0, "idle: nothing was dropped");
    match end(&mut rig, &mut presenter) {
        CleanupResult::Unproven { reason } => {
            let wanted = match lost {
                NeverComplete::Drawables => "1 drawable presented handler(s) and no command buffer",
                _ => "0 drawable presented handler(s) and 1 command buffer completion",
            };
            assert!(reason.contains(wanted), "{reason}");
        }
        CleanupResult::Proven => panic!("a missing token must not be proven"),
    }
    assert_eq!(
        observer.buffer_holders_now(),
        1,
        "the command buffer holder was kept"
    );
    assert_eq!(observer.buffer_drops(), 0, "no holder transition at all");
    assert_eq!(observer.holders_released(), 0, "nothing was released");
    assert_eq!(
        observer.layer_retains_outstanding(),
        1,
        "the layer retain was kept"
    );
}

/// B2, command buffer complete, drawable token pending.
fn unproven_drawable_pending() {
    partial_teardown_keeps_holders(NeverComplete::Drawables);
}

/// B2, drawable token complete, command buffer pending.
fn unproven_command_pending() {
    partial_teardown_keeps_holders(NeverComplete::Commands);
}

/// B3. The stop deadline is real time even when the thread is delayed inside its wait for the tokens: a
/// 750 ms delay against a 300 ms budget ends right after the delay, not after the delay plus more waiting.
fn deadline_paused_in_wait() {
    const PAUSE: Duration = Duration::from_millis(750);
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let probes = Probes::new();
    let hooks = TestHooks {
        never_complete: NeverComplete::Everything,
        stop_budget: Some(Duration::from_millis(300)),
        pause_in_wait: Some(PAUSE),
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let stopped = Instant::now();
    assert!(matches!(
        end(&mut rig, &mut presenter),
        CleanupResult::Unproven { .. }
    ));
    let took = stopped.elapsed();
    assert!(
        took >= PAUSE && took < PAUSE + Duration::from_millis(250),
        "the deadline had already passed when the delay ended; it took {took:?}"
    );
    assert_eq!(probes.observer.holders_released(), 0);
}

/// B3. With the default 1.5 s budget a lost token ends `Unproven` at the budget, well inside the 2 s outer
/// limit.
fn deadline_default_budget() {
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let probes = Probes::new();
    let hooks = TestHooks {
        never_complete: NeverComplete::Everything,
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let stopped = Instant::now();
    assert!(matches!(
        end(&mut rig, &mut presenter),
        CleanupResult::Unproven { .. }
    ));
    let took = stopped.elapsed();
    assert!(
        took >= Duration::from_millis(1450) && took < Duration::from_millis(1750),
        "1.5 s budget, 250 ms margin: it took {took:?}"
    );
    assert_eq!(probes.observer.holders_released(), 0);
}

/// B1 (review 2). The completion lands **between** a native look at the command token and what the caller
/// does with the answer. The look read the token as not done, so it authorises nothing but a timed wait or
/// a busy frame; the error is taken on the next look, reported once, and nothing more is drawn.
fn completion_between_the_reads() {
    let mut rig = Rig::open();
    for (site, name) in [
        (PauseSite::IdleLook, "idle look"),
        (PauseSite::FrameStep, "frame step"),
    ] {
        let probes = Probes::new();
        let hooks = TestHooks {
            error_completion_frames: 1,
            // The completion handler sets its token 300 ms after it ran; the look is paused for 700 ms
            // right after it read the token as not done, so the completion lands inside the pause.
            completion_delay: Duration::from_millis(300),
            pause_site: site,
            pause_for: Duration::from_millis(700),
            ..probes.hooks()
        };
        let mut presenter = probes.start_ok(&rig, hooks);
        assert!(rig.pump_until(BOUND, || probes.frames() >= 1), "{name}");
        if site == PauseSite::FrameStep {
            // A redraw is wanted while the first command buffer is still in flight: the frame step looks.
            presenter.set_clear([0.9, 0.9, 0.1]);
            presenter.request_redraw();
        }
        let mut reports = Vec::new();
        assert!(
            reports_until(&mut rig, &presenter, &mut reports, 2),
            "{name}: the error is reported, not stuck behind an untimed wait: {reports:?}"
        );
        rig.pump_for(Duration::from_millis(300));
        rest(&presenter, &mut reports);
        assert_eq!(
            failure_count(&reports),
            1,
            "{name}: exactly once: {reports:?}"
        );
        assert_eq!(probes.frames(), 1, "{name}: no second frame");
        assert_eq!(
            probes.observer.committed(),
            1,
            "{name}: nothing replaced the failed token"
        );
        assert_eq!(
            end(&mut rig, &mut presenter),
            CleanupResult::Proven,
            "{name}"
        );
        rest(&presenter, &mut reports);
        assert_eq!(
            failure_count(&reports),
            1,
            "{name}: still once: {reports:?}"
        );
    }
}

/// A wake that panics on every call (the first report, the cleanup result) is caught and ignored: it
/// never changes a result already sent and never stops a report from being made.
fn wake_panic() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    // SAFETY: as in `Probes::start`: the rig's layer is live for the whole case.
    let mut presenter = unsafe {
        MetalPresenter::start_with_test_hooks(
            rig.layer,
            rig.pixels,
            [0.1, 0.2, 0.3],
            Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                panic!("a wake that panics");
            }),
            probes.hooks(),
        )
    }
    .expect("the presenter starts");
    let mut reports = Vec::new();
    assert!(reports_until(&mut rig, &presenter, &mut reports, 1));
    assert_eq!(
        reports,
        [PresenterReport::FirstFrameQueued],
        "the report was made"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(
        presenter.poll_end(Duration::ZERO),
        Some(CleanupResult::Proven),
        "and it stays so"
    );
    assert!(
        calls.load(Ordering::SeqCst) >= 2,
        "after the report and after the result"
    );
}

/// A callback that runs after the presenter ended and the handle was dropped touches only its token and
/// the observer.
fn late_callbacks() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let hooks = TestHooks {
        handler_delay: Duration::from_millis(500),
        ..probes.hooks()
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    drop(presenter);
    assert!(probes.observer.holders_released() >= 3);
    assert!(
        rig.pump_until(BOUND, || probes.observer.late_handlers() >= 1),
        "a handler returned after the release and the handle's drop, and nothing crashed"
    );
    assert_eq!(probes.observer.drawable_refs(), 0);
}

/// The input filter drops the owner's typing: it saw keys in the typing run, and nothing reached a window.
fn typing_is_dropped() {
    let mut rig = Rig::open();
    if typing() {
        assert!(
            rig.pump_until(BOUND, || {
                // SAFETY: reads an atomic counter.
                unsafe { ck_t_dropped_keys() > 0 }
            }),
            "the typist is not typing, so this run proves nothing about typing"
        );
    }
    let probes = Probes::new();
    let mut presenter = probes.start_ok(&rig, probes.hooks());
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// The fixtures' workers.
fn checker_control() {
    let _rig = Rig::open();
    let thread = thread::spawn(|| {
        // SAFETY: deliberately not on the main thread, to provoke the Main Thread Checker; the native call
        // only marks the content view dirty.
        unsafe { ck_t_violate_main_thread() };
    });
    let _ = thread.join();
}

fn hang() {
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

type Case = fn();

const CASES: [(&str, Case); 25] = [
    ("readback", readback),
    ("first_frame", first_frame),
    ("redraw_on_demand", redraw_on_demand),
    ("pixel_size", pixel_size),
    ("proven_teardown_visible", proven_teardown_visible),
    (
        "proven_teardown_abandoned_drawable",
        proven_teardown_abandoned_drawable,
    ),
    ("proven_teardown_minimised", proven_teardown_minimised),
    (
        "the_minimise_helpers_address_the_rigs_window",
        the_minimise_helpers_address_the_rigs_window,
    ),
    ("unproven_after_a_panic", unproven_after_a_panic),
    (
        "unproven_after_a_panic_whose_payload_drop_panics",
        unproven_after_a_panic_whose_payload_drop_panics,
    ),
    (
        "unproven_when_the_budget_expires",
        unproven_when_the_budget_expires,
    ),
    ("start_failure", start_failure),
    ("handle_drop_never_blocks", handle_drop_never_blocks),
    ("ownership", ownership),
    ("deterministic_redraw", deterministic_redraw),
    ("frame_failures", frame_failures),
    ("stop_deadline", stop_deadline),
    ("one_frame_in_flight", one_frame_in_flight),
    ("wake_panic", wake_panic),
    ("completion_error_never_lost", completion_error_never_lost),
    ("completion_between_the_reads", completion_between_the_reads),
    ("unproven_drawable_pending", unproven_drawable_pending),
    ("unproven_command_pending", unproven_command_pending),
    ("deadline_paused_in_wait", deadline_paused_in_wait),
    ("deadline_default_budget", deadline_default_budget),
];

/// Cases besides the table: they run in both modes too.
const MORE_CASES: [(&str, Case); 2] = [
    ("late_callbacks", late_callbacks),
    ("typing_is_dropped", typing_is_dropped),
];

/// Cases only the supervisor's fixtures start.
const FIXTURE_CASES: [(&str, Case); 2] = [("checker_control", checker_control), ("hang", hang)];

/// Every case with the set it belongs to. A name starting `readback` is `Gpu` (the off-screen readbacks open no window); every
/// other case's set comes from **the array it is listed in**, not from its name, so a case added to an array joins that array's
/// set with no other edit: `CASES`, `MORE_CASES`, `quad::CASES`, `quad::MORE` are `WindowCore`; `scene::CASES` and `scene::MORE`
/// are `WindowScene`; `scene::compat::CASES`, `scene::compat::MORE` and `scene::compat::BINDLESS` are `WindowCompat`;
/// `scene::instanced::CASES`, `scene::instanced::MORE` and `scene::instanced::BINDLESS` are `WindowInstanced`; `instanced::CASES`
/// holds only readbacks (`Gpu`).
fn tagged_cases() -> impl Iterator<Item = (&'static (&'static str, Case), CaseSet)> {
    let core = CASES
        .iter()
        .chain(MORE_CASES.iter())
        .chain(quad::CASES.iter())
        .chain(quad::MORE.iter())
        .chain(metrics::CASES.iter())
        .chain(metrics::MORE.iter())
        .map(|case| (case, CaseSet::WindowCore));
    // The off-screen readbacks (`instanced::CASES`, `bindless::CASES`): `Gpu` by their names, whatever tag they get here.
    let readbacks = instanced::CASES
        .iter()
        .chain(bindless::CASES.iter())
        .map(|case| (case, CaseSet::WindowCore));
    let scene = scene::CASES
        .iter()
        .chain(scene::MORE.iter())
        .map(|case| (case, CaseSet::WindowScene));
    let compat = scene::compat::CASES
        .iter()
        .chain(scene::compat::MORE.iter())
        .chain(scene::compat::BINDLESS.iter())
        .map(|case| (case, CaseSet::WindowCompat));
    let instanced = scene::instanced::CASES
        .iter()
        .chain(scene::instanced::MORE.iter())
        .chain(scene::instanced::BINDLESS.iter())
        .map(|case| (case, CaseSet::WindowInstanced));
    core.chain(readbacks)
        .chain(scene)
        .chain(compat)
        .chain(instanced)
        .map(|(case, set)| {
            let set = if case.0.starts_with("readback") {
                CaseSet::Gpu
            } else {
                set
            };
            (case, set)
        })
}

fn all_cases() -> impl Iterator<Item = &'static (&'static str, Case)> {
    tagged_cases().map(|(case, _)| case)
}

fn worker(name: &str) -> ExitCode {
    let _ = WORKER_STARTED.get_or_init(Instant::now);
    if !checker_loaded() {
        eprintln!("worker: libMainThreadChecker.dylib is not loaded; the checker must be inserted");
        return ExitCode::from(2);
    }
    for variable in ["MTL_DEBUG_LAYER", "MTL_SHADER_VALIDATION"] {
        if env::var(variable).as_deref() != Ok("1") {
            eprintln!("worker: {variable} is not 1; stopping before any Metal object exists");
            return ExitCode::from(2);
        }
    }
    let Some((_, case)) = all_cases()
        .chain(FIXTURE_CASES.iter())
        .find(|(case, _)| *case == name)
    else {
        eprintln!("worker: no case called {name}");
        return ExitCode::from(1);
    };
    ck_window::test_support::open_windows_without_activation();
    activation_watch::install(false);
    match catch_unwind(AssertUnwindSafe(case)) {
        Ok(()) => {
            activation_watch::assert_clean();
            ExitCode::SUCCESS
        }
        Err(payload) => {
            let text = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_owned())
                })
                .unwrap_or_else(|| "a panic with a non-string payload".to_owned());
            eprintln!("worker: {name} failed: {text}");
            ExitCode::from(1)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The supervisor
// ---------------------------------------------------------------------------------------------

/// How long a helper program of the supervisor (`kill`, `ps`, `xcode-select`, `nm`) may run before it is
/// killed. After the kill it is reaped within `REAP_GRACE`, and its output within `PIPE_GRACE`.
const HELPER_BUDGET: Duration = Duration::from_secs(2);

/// The helper programs the supervisor uses. The fixtures replace them with stalled or failing ones.
#[derive(Debug, Clone)]
struct Tools {
    kill: PathBuf,
    shell: PathBuf,
    ps: PathBuf,
    xcode_select: PathBuf,
}

impl Tools {
    fn system() -> Self {
        Self {
            kill: PathBuf::from("/bin/kill"),
            shell: PathBuf::from("/bin/sh"),
            ps: PathBuf::from("/bin/ps"),
            xcode_select: PathBuf::from("/usr/bin/xcode-select"),
        }
    }
}

/// What reading a stream to its end came to: all of its text, or the error that ended the read. Bytes read
/// before an error are not kept: a prefix that looks clean is no evidence of what the stream held.
type Drained = Result<String, String>;

/// Reads `stream` to its end. An error is a failure that says how much had been read before it.
fn read_all<R: Read>(stream: Option<R>) -> Drained {
    let mut bytes = Vec::new();
    if let Some(mut stream) = stream
        && let Err(error) = stream.read_to_end(&mut bytes)
    {
        return Err(format!("{error} (after {} bytes)", bytes.len()));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Reads `stream` to its end on a thread of its own, and sends the result: a read error is sent as such.
fn drain<R: Read + Send + 'static>(stream: Option<R>) -> std::io::Result<mpsc::Receiver<Drained>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new().spawn(move || {
        let _ = sender.send(read_all(stream));
    })?;
    Ok(receiver)
}

/// Waits at most `grace` for the child to be reaped; `Ok(false)` if it is still running.
fn reaped_within(child: &mut Child, grace: Duration) -> std::io::Result<bool> {
    let expiry = Instant::now() + grace;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        if Instant::now() >= expiry {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Why a helper gave no answer. The kinds matter to callers: a helper that ran and exited unsuccessfully
/// (`pgrep` found no match, `kill -0` found no process) is an answer for some of them; one whose output
/// could not be read, or that did not finish, is never an answer.
#[derive(Debug, PartialEq, Eq)]
enum HelperError {
    /// It ran to its end and exited unsuccessfully (`code` is `None` when a signal ended it).
    Exit {
        name: String,
        status: String,
        code: Option<i32>,
    },
    /// Its output could not be read to the end (whatever its status said).
    Read { name: String, error: String },
    /// It could not be run, waited for, ended or drained in time.
    Other(String),
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exit { name, status, .. } => write!(formatter, "{name} exited with {status}"),
            Self::Read { name, error } => {
                write!(formatter, "{name}: its output could not be read: {error}")
            }
            Self::Other(text) => formatter.write_str(text),
        }
    }
}

impl From<HelperError> for String {
    fn from(error: HelperError) -> Self {
        error.to_string()
    }
}

/// What a finished helper came to, from its status and what reading its output gave. A failed read comes
/// first: with the output unread, neither a zero status nor an exit status of 1 means what it usually does.
fn helper_result(name: &str, status: ExitStatus, output: Drained) -> Result<String, HelperError> {
    let text = output.map_err(|error| HelperError::Read {
        name: name.to_owned(),
        error,
    })?;
    if status.success() {
        Ok(text)
    } else {
        Err(HelperError::Exit {
            name: name.to_owned(),
            status: status.to_string(),
            code: status.code(),
        })
    }
}

/// A helper process while the supervisor owns it, from the instant it is spawned: if it is not settled
/// when this drops (an error, a panic), the helper is killed and reaped, waiting at most `REAP_GRACE`.
struct Helper {
    child: Child,
    settled: bool,
}

impl Drop for Helper {
    fn drop(&mut self) {
        if !self.settled {
            let _ = self.child.kill();
            let _ = reaped_within(&mut self.child, REAP_GRACE);
        }
    }
}

/// Runs `program` and returns what it wrote to stdout. The helper is owned from the instant it is spawned,
/// and nothing about it can take longer than `budget` plus `REAP_GRACE` plus `PIPE_GRACE`: at the deadline
/// it is killed and reaped (or the failure to reap is the error), and its output is waited for only so
/// long. A helper that fails, times out or cannot be reaped is an error, never an empty answer.
fn run_helper(program: &Path, args: &[&str], budget: Duration) -> Result<String, HelperError> {
    let name = program.display().to_string();
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let child = command
        .spawn()
        .map_err(|error| HelperError::Other(format!("{name} could not run: {error}")))?;
    let mut helper = Helper {
        child,
        settled: false,
    };
    let output = drain(helper.child.stdout.take()).map_err(|error| {
        HelperError::Other(format!("{name}: output could not be read: {error}"))
    })?;
    let expiry = Instant::now() + budget;
    let status = loop {
        match helper.child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < expiry => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = helper.child.kill();
                return match reaped_within(&mut helper.child, REAP_GRACE) {
                    Ok(true) => {
                        helper.settled = true;
                        Err(HelperError::Other(format!(
                            "{name} did not finish within {budget:?}; it was killed"
                        )))
                    }
                    _ => Err(HelperError::Other(format!(
                        "{name} did not finish within {budget:?} and could not be reaped"
                    ))),
                };
            }
            Err(error) => {
                return Err(HelperError::Other(format!(
                    "{name} could not be waited for: {error}"
                )));
            }
        }
    };
    helper.settled = true;
    // A timeout here is a descendant holding the pipe open; a read error arrives as the answer.
    let output = output
        .recv_timeout(PIPE_GRACE)
        .map_err(|_| HelperError::Other(format!("{name}: its output pipe stayed open")))?;
    helper_result(&name, status, output)
}

/// Kills every process in the group (the worker is its leader): `kill`, then the shell's own. Both are
/// bounded helpers.
fn kill_group(tools: &Tools, group: u32) -> Result<(), String> {
    let target = format!("-{group}");
    let first = run_helper(&tools.kill, &["-KILL", "--", &target], HELPER_BUDGET);
    let Err(first) = first else {
        return Ok(());
    };
    let group_text = group.to_string();
    run_helper(
        &tools.shell,
        &["-c", "kill -KILL -- \"-$0\"", &group_text],
        HELPER_BUDGET,
    )
    .map(drop)
    .map_err(|second| format!("{first}; then {second}"))
}

/// The ids of the processes in the group. Failing to list them is an error: not being able to look is
/// not evidence that the group is empty.
fn group_members(tools: &Tools, group: u32) -> Result<Vec<u32>, String> {
    members_of(
        run_helper(&tools.ps, &["-axo", "pid=,pgid="], HELPER_BUDGET),
        group,
    )
}

/// [`group_members`] from what the listing helper gave: a listing that was not read to its end, whatever
/// its prefix shows, is an error and never an empty or a complete group.
fn members_of(listing: Result<String, HelperError>, group: u32) -> Result<Vec<u32>, String> {
    let listing = listing?;
    Ok(listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace().map(str::parse::<u32>);
            match (fields.next(), fields.next()) {
                (Some(Ok(member)), Some(Ok(leader))) if leader == group => Some(member),
                _ => None,
            }
        })
        .collect())
}

/// The worker while the supervisor owns it, from the instant it is spawned. If the supervisor returns
/// for any reason with the worker not reaped (an error, a panic), dropping this kills the group and the
/// worker and reaps it, waiting at most `REAP_GRACE` per attempt.
struct Owned {
    child: Child,
    group: u32,
    tools: Tools,
    settled: bool,
    /// What the most recent group kill said.
    group_kill: Result<(), String>,
}

impl Owned {
    fn end(&mut self) -> Result<(), String> {
        let group = kill_group(&self.tools, self.group);
        self.group_kill.clone_from(&group);
        let direct = match self.child.kill() {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
            Err(error) => Err(error.to_string()),
        };
        match reaped_within(&mut self.child, REAP_GRACE) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "worker {} is still running after the kill attempts (group kill {group:?}, direct kill {direct:?})",
                self.group
            )),
            Err(error) => Err(format!(
                "worker {} could not be waited for: {error}",
                self.group
            )),
        }
    }

    fn end_with_retry(&mut self) -> Result<(), String> {
        let result = self.end().or_else(|_| self.end());
        if result.is_ok() {
            self.settled = true;
        }
        result
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.settled
            && let Err(message) = self.end_with_retry()
        {
            eprintln!("presenter: cleanup failed: {message}");
        }
    }
}

/// How a supervised worker ended.
#[derive(Debug)]
enum Outcome {
    Exited {
        code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    /// The deadline passed; the worker's group was killed and is empty, and the worker is reaped.
    TimedOut,
    /// The deadline passed (or a descendant held the pipes) and the group could not be shown to be
    /// ended: it could not be killed, listed or emptied.
    Unterminated(String),
    /// The worker exited but a diagnostic stream could not be read to its end: whatever the status and the
    /// bytes read say, the case did not produce complete evidence.
    ReadFailed(String),
}

/// The outcome of a worker that exited, from what reading its two streams gave. A read error in either is
/// a failure naming the stream, even for a zero status and streams that look clean.
fn exited(code: Option<i32>, stdout: Drained, stderr: Drained) -> Outcome {
    match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => Outcome::Exited {
            code,
            stdout,
            stderr,
        },
        (stdout, stderr) => Outcome::ReadFailed(
            [("stdout", stdout), ("stderr", stderr)]
                .into_iter()
                .filter_map(|(stream, read)| {
                    read.err()
                        .map(|error| format!("the worker's {stream} could not be read: {error}"))
                })
                .collect::<Vec<_>>()
                .join("; "),
        ),
    }
}

/// The Xcode developer directory: `DEVELOPER_DIR` when it is set, else what `xcode-select -p` says (a
/// bounded helper).
fn developer_dir(variable: Option<OsString>, tools: &Tools) -> Result<PathBuf, String> {
    match variable {
        Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
        _ => developer_path(run_helper(&tools.xcode_select, &["-p"], HELPER_BUDGET)),
    }
}

/// The developer directory `xcode-select -p` printed; output that was not read to its end is an error.
fn developer_path(printed: Result<String, HelperError>) -> Result<PathBuf, String> {
    Ok(PathBuf::from(printed?.trim()))
}

/// The Main Thread Checker library of the selected Xcode.
fn checker_library(tools: &Tools) -> Result<PathBuf, String> {
    let developer = developer_dir(env::var_os("DEVELOPER_DIR"), tools)?;
    let library = developer.join("usr/lib/libMainThreadChecker.dylib");
    if library.is_file() {
        Ok(library)
    } else {
        Err(format!(
            "the Main Thread Checker library {} is missing; the full check needs the selected Xcode's",
            library.display()
        ))
    }
}

/// How a worker is started.
#[derive(Debug, Clone, Copy)]
struct Run {
    /// How long it may run.
    deadline: Duration,
    /// Whether the checker crashes the worker on a report (`MTC_CRASH_ON_REPORT`).
    crash_on_report: bool,
    /// Whether the owner is typing without stopping while it runs.
    typing: bool,
}

/// Starts this executable as the worker for `case`, in its own process group, with the checker inserted,
/// and waits at most `run.deadline`. Every path after the spawn ends the worker, and every wait is bounded.
fn supervise(case: &str, run: Run, tools: &Tools) -> Result<Outcome, String> {
    let library = checker_library(tools)?;
    let mut command = Command::new(env::current_exe().map_err(|error| error.to_string())?);
    command
        .arg(WORKER_FLAG)
        .arg(case)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("DYLD_INSERT_LIBRARIES", &library);
    if run.crash_on_report {
        command.env("MTC_CRASH_ON_REPORT", "1");
    } else {
        command.env_remove("MTC_CRASH_ON_REPORT");
    }
    // Metal's validation layers, in from the first instant so no Metal object exists before they apply.
    command
        .env("MTL_DEBUG_LAYER", "1")
        .env("MTL_SHADER_VALIDATION", "1");
    if run.typing {
        command.env(TYPING_VARIABLE, "1");
    } else {
        command.env_remove(TYPING_VARIABLE);
    }
    let child = command
        .spawn()
        .map_err(|error| format!("the worker could not be started: {error}"))?;
    let group = child.id();
    let mut owned = Owned {
        child,
        group,
        tools: tools.clone(),
        settled: false,
        group_kill: Ok(()),
    };
    let stdout = drain(owned.child.stdout.take()).map_err(|error| error.to_string())?;
    let stderr = drain(owned.child.stderr.take()).map_err(|error| error.to_string())?;
    let expiry = Instant::now() + run.deadline;
    loop {
        if let Some(status) = owned.child.try_wait().map_err(|error| error.to_string())? {
            owned.settled = true;
            return Ok(finish_worker(status.code(), &stdout, &stderr, tools, group));
        }
        if Instant::now() >= expiry {
            return Ok(end_after_deadline(&mut owned));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// A worker that exited: its two streams are waited for at most `PIPE_GRACE` each, and what they gave
/// decides the outcome. The pipes close when every member of the group is gone, so a descendant that kept
/// them open must not hang the supervisor (the group is killed and that is reported). A stream that could
/// not be read is a failure of its own, with the group killed as well, and it is reported with its stream
/// and its error even when the other stream did not finish.
fn finish_worker(
    code: Option<i32>,
    stdout: &mpsc::Receiver<Drained>,
    stderr: &mpsc::Receiver<Drained>,
    tools: &Tools,
    group: u32,
) -> Outcome {
    // Each stream's result is kept on its own: a sibling that times out or disconnects must not hide a
    // read error already received.
    let (stdout, stderr) = (
        stdout.recv_timeout(PIPE_GRACE).ok(),
        stderr.recv_timeout(PIPE_GRACE).ok(),
    );
    let mut errors = Vec::new();
    for (stream, read) in [("stdout", &stdout), ("stderr", &stderr)] {
        if let Some(Err(error)) = read {
            errors.push(format!("the worker's {stream} could not be read: {error}"));
        }
    }
    if errors.is_empty() {
        if let (Some(stdout), Some(stderr)) = (stdout, stderr) {
            return exited(code, stdout, stderr);
        }
        let killed = kill_group(tools, group);
        return Outcome::Unterminated(format!(
            "a descendant of the worker kept its output pipes open (kill result {killed:?})"
        ));
    }
    if stdout.is_none() || stderr.is_none() {
        errors.push(
            "its other stream did not finish within the pipe grace, or its reader ended without a result"
                .to_owned(),
        );
    }
    // The streams may be held by a survivor that made them fail: end the group as well.
    let killed = kill_group(tools, group);
    Outcome::ReadFailed(format!("{} (kill result {killed:?})", errors.join("; ")))
}

/// Ends the group after a deadline: kill it, reap the worker, then insist that the group is shown to be
/// empty. A group that cannot be killed or listed is reported as not ended.
fn end_after_deadline(owned: &mut Owned) -> Outcome {
    let group = owned.group;
    if let Err(survivor) = owned.end_with_retry() {
        owned.settled = true;
        return Outcome::Unterminated(survivor);
    }
    if let Err(killed) = owned.group_kill.clone() {
        return Outcome::Unterminated(format!("group {group} could not be killed: {killed}"));
    }
    let expiry = Instant::now() + PIPE_GRACE;
    loop {
        match group_members(&owned.tools, group) {
            Err(error) => {
                return Outcome::Unterminated(format!(
                    "group {group} could not be listed: {error}"
                ));
            }
            Ok(members) if members.is_empty() => return Outcome::TimedOut,
            Ok(members) if Instant::now() >= expiry => {
                return Outcome::Unterminated(format!("group {group} not emptied: {members:?}"));
            }
            Ok(_) => thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// `Ok` when a case's worker passed: it exited 0 and the checker reported nothing.
fn judge(case: &str, outcome: &Outcome) -> Result<(), String> {
    match outcome {
        Outcome::TimedOut => Err(format!(
            "{case} timed out after {} s and the worker's process group was killed",
            DEADLINE.as_secs()
        )),
        Outcome::Unterminated(detail) => Err(format!(
            "{case} did not finish and its process group could not be ended: {detail}"
        )),
        Outcome::ReadFailed(detail) => Err(format!(
            "{case}: its diagnostics are incomplete, so the case cannot pass: {detail}"
        )),
        Outcome::Exited {
            code,
            stdout,
            stderr,
        } => {
            if stderr.contains(CHECKER_REPORT) {
                return Err(format!(
                    "{case}: the Main Thread Checker reported:\n{stderr}"
                ));
            }
            if let Some(line) = stderr.lines().find(|line| {
                VALIDATION_MARKERS
                    .iter()
                    .any(|marker| line.contains(marker))
            }) {
                return Err(format!(
                    "{case}: Metal validation reported: {line}\n{stderr}"
                ));
            }
            if *code == Some(0) {
                Ok(())
            } else {
                Err(format!(
                    "{case}: the worker ended with {code:?}; stdout {stdout:?}, stderr {stderr:?}"
                ))
            }
        }
    }
}

fn run_case(name: &str, typing: bool) -> Result<(), String> {
    let run = Run {
        deadline: DEADLINE,
        crash_on_report: true,
        typing,
    };
    judge(name, &supervise(name, run, &Tools::system())?)
}

// -- The supervisor's own checks ----------------------------------------------------------------

/// A directory of fixture helper scripts, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = env::temp_dir().join(format!(
            "ck-window-cocoa-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self(path)
    }

    /// An executable script with this body.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.0.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("a helper script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("an executable helper script");
        path
    }

    /// A helper that records its pid in `pids/<name>` and then stalls (the `exec` leaves no child behind).
    fn stalled(&self, name: &str) -> (PathBuf, PathBuf) {
        let pid_file = self.0.join(format!("{name}.pid"));
        let script = self.script(
            name,
            &format!("echo $$ > '{}'\nexec /bin/sleep 60", pid_file.display()),
        );
        (script, pid_file)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether a stalled helper (this script, which records its pid in `pid_file`) is still running.
///
/// Under load the supervisor can kill the helper before its shell runs the line that records the pid
/// (or between that line's truncate and write). Such a helper never reached its `exec`, so a survivor
/// would still be the shell running the script: it is looked for by the script's unique path.
fn running(script: &Path, pid_file: &Path) -> Result<bool, String> {
    let text = match std::fs::read_to_string(pid_file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("the helper's pid file could not be read: {error}")),
    };
    let pid = text.trim();
    if pid.is_empty() {
        return script_running(script);
    }
    // `kill -0` exits 0 only for a process that exists.
    pid_verdict(
        run_helper(Path::new("/bin/kill"), &["-0", pid], HELPER_BUDGET),
        pid,
    )
}

/// Whether process `pid` exists, from `kill -0`: success is yes and its own unsuccessful exit is no;
/// anything else (a read error, a stall, a failure to run) cannot tell and is an error.
fn pid_verdict(result: Result<String, HelperError>, pid: &str) -> Result<bool, String> {
    match result {
        Ok(_) => Ok(true),
        Err(HelperError::Exit { .. }) => Ok(false),
        Err(error) => Err(format!(
            "could not tell whether process {pid} still runs: {error}"
        )),
    }
}

/// Whether some process runs this script, by its path; fails closed when that cannot be told.
fn script_running(script: &Path) -> Result<bool, String> {
    script_running_with(Path::new("/usr/bin/pgrep"), script)
}

/// [`script_running`] with the `pgrep` to run (a fake one in the checks of its error handling).
fn script_running_with(pgrep: &Path, script: &Path) -> Result<bool, String> {
    let path = script.display().to_string();
    // `-f` takes an extended regular expression over the whole command line (`/bin/sh <script>` for a
    // shell script), so the path is matched literally and whole. Run directly (no shell wrapper), the
    // pattern is only in pgrep's own arguments, and pgrep never matches itself.
    let mut pattern = String::from("^(/bin/sh )?");
    for character in path.chars() {
        if "\\.^$|?*+()[]{}".contains(character) {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push_str("( |$)");
    pgrep_verdict(
        run_helper(pgrep, &["-f", "--", &pattern], HELPER_BUDGET),
        &path,
    )
}

/// What `pgrep` said: it exits 0 when a process matches and 1 when none does, and otherwise on an error.
/// Only a complete read of an exit of 1 is "none"; an output that could not be read is an error.
fn pgrep_verdict(result: Result<String, HelperError>, path: &str) -> Result<bool, String> {
    match result {
        Ok(_) => Ok(true),
        Err(HelperError::Exit { code: Some(1), .. }) => Ok(false),
        Err(error) => Err(format!("could not tell whether {path} still runs: {error}")),
    }
}

/// A hanging worker is killed and reaped at its deadline, and leaves no process behind.
fn check_a_hung_worker_is_killed() -> Result<(), String> {
    let run = Run {
        deadline: Duration::from_secs(2),
        crash_on_report: true,
        typing: false,
    };
    match supervise("hang", run, &Tools::system())? {
        Outcome::TimedOut => Ok(()),
        other => Err(format!(
            "a hung worker should time out cleanly, got {other:?}"
        )),
    }
}

/// The checker is live in the worker: an `AppKit` call off the main thread is reported, and with
/// crash-on-report the worker does not end successfully.
fn check_the_checker_reports_a_violation() -> Result<(), String> {
    let mut run = Run {
        deadline: DEADLINE,
        crash_on_report: false,
        typing: false,
    };
    let stderr = match supervise("checker_control", run, &Tools::system())? {
        Outcome::Exited { stderr, .. } => stderr,
        Outcome::ReadFailed(detail) => {
            return Err(format!(
                "the control worker's diagnostics could not be read: {detail}"
            ));
        }
        other => return Err(format!("the control worker did not exit: {other:?}")),
    };
    if !stderr.contains(CHECKER_REPORT) {
        return Err(format!(
            "the checker reported nothing for a call off the main thread; stderr {stderr:?}"
        ));
    }
    run.crash_on_report = true;
    match supervise("checker_control", run, &Tools::system())? {
        Outcome::Exited { code: Some(0), .. } => {
            Err("with crash-on-report the control worker still ended successfully".to_owned())
        }
        Outcome::Exited { .. } => Ok(()),
        Outcome::ReadFailed(detail) => Err(format!(
            "the crashing control worker's diagnostics could not be read: {detail}"
        )),
        other => Err(format!(
            "the crashing control worker did not exit: {other:?}"
        )),
    }
}

/// The longest the faulty-helper checks may take: the worker's deadline, a helper's budget, its reap and
/// pipe waits, and a margin.
const FAULT_BOUND: Duration = Duration::from_secs(14);

/// A stalled or failing `ps` after a hung worker's deadline is a bounded, reported failure (not an empty
/// group), within `FAULT_BOUND`, and leaves neither the worker nor the helper running.
fn check_a_faulty_process_list_is_bounded_and_fails_closed() -> Result<(), String> {
    let scratch = Scratch::new();
    let (stalled, pid_file) = scratch.stalled("stalled-ps");
    let failing = scratch.script("failing-ps", "exit 1");
    for (label, ps) in [("stalled", stalled.clone()), ("failing", failing)] {
        let tools = Tools {
            ps,
            ..Tools::system()
        };
        let run = Run {
            deadline: Duration::from_secs(2),
            crash_on_report: true,
            typing: false,
        };
        let started = Instant::now();
        let outcome = supervise("hang", run, &tools)?;
        let took = started.elapsed();
        match outcome {
            Outcome::Unterminated(detail) if detail.contains("could not be listed") => {}
            other => {
                return Err(format!(
                    "a {label} process list must be a reported failure, got {other:?}"
                ));
            }
        }
        if took > FAULT_BOUND {
            return Err(format!(
                "a {label} process list took {took:?}, over {FAULT_BOUND:?}"
            ));
        }
    }
    if running(&stalled, &pid_file)? {
        return Err("the stalled process-list helper is still running".to_owned());
    }
    // The worker itself was ended: nothing of that run is left in a group the system tools can see.
    Ok(())
}

/// A stalled `kill` helper (and a failing shell fallback) cannot stall the supervisor either.
fn check_a_stalled_kill_helper_is_bounded() -> Result<(), String> {
    let scratch = Scratch::new();
    let (stalled, pid_file) = scratch.stalled("stalled-kill");
    let failing = scratch.script("failing-shell", "exit 1");
    let tools = Tools {
        kill: stalled.clone(),
        shell: failing,
        ..Tools::system()
    };
    let run = Run {
        deadline: Duration::from_secs(2),
        crash_on_report: true,
        typing: false,
    };
    let started = Instant::now();
    let outcome = supervise("hang", run, &tools)?;
    let took = started.elapsed();
    if !matches!(&outcome, Outcome::Unterminated(detail) if detail.contains("could not be killed"))
    {
        return Err(format!(
            "a stalled kill helper must be reported, got {outcome:?}"
        ));
    }
    if took > FAULT_BOUND * 2 {
        return Err(format!("a stalled kill helper took {took:?}"));
    }
    if running(&stalled, &pid_file)? {
        return Err("the stalled kill helper is still running".to_owned());
    }
    Ok(())
}

/// A stalled `xcode-select` is a bounded error, and its helper is gone.
fn check_a_stalled_xcode_select_is_bounded() -> Result<(), String> {
    let scratch = Scratch::new();
    let (stalled, pid_file) = scratch.stalled("stalled-xcode-select");
    let tools = Tools {
        xcode_select: stalled.clone(),
        ..Tools::system()
    };
    let started = Instant::now();
    let result = developer_dir(None, &tools);
    let took = started.elapsed();
    if result.is_ok() {
        return Err("a stalled xcode-select must be an error".to_owned());
    }
    if took > HELPER_BUDGET + REAP_GRACE + PIPE_GRACE {
        return Err(format!("a stalled xcode-select took {took:?}"));
    }
    if running(&stalled, &pid_file)? {
        return Err("the stalled xcode-select is still running".to_owned());
    }
    Ok(())
}

/// A helper killed before it recorded its pid is looked for by its script's path: none is found for a
/// script nothing runs (and a missing pid file uses that), and one is found while the script runs.
fn check_a_helper_without_a_pid_is_found_by_its_script() -> Result<(), String> {
    let scratch = Scratch::new();
    // No `exec` and no child: the shell itself keeps running the script, as a helper killed before its
    // first line would, and killing it leaves nothing behind.
    let script = scratch.script("looping", "while :; do :; done");
    let pid_file = scratch.0.join("looping.pid");
    if running(&script, &pid_file)? {
        return Err("a script nothing runs was reported running".to_owned());
    }
    let child = Command::new(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("the looping script could not start: {error}"))?;
    let mut helper = Helper {
        child,
        settled: false,
    };
    // pgrep sees the process once it has executed the script (bounded: the shell starts at once).
    let expiry = Instant::now() + HELPER_BUDGET;
    let mut seen = false;
    while !seen && Instant::now() < expiry {
        seen = running(&script, &pid_file)?;
        if !seen {
            thread::sleep(Duration::from_millis(20));
        }
    }
    let _ = helper.child.kill();
    if reaped_within(&mut helper.child, REAP_GRACE).map_err(|error| error.to_string())? {
        helper.settled = true;
    } else {
        return Err("the looping script could not be reaped".to_owned());
    }
    if !seen {
        return Err("a running script was not found by its path".to_owned());
    }
    if running(&script, &pid_file)? {
        return Err("a killed and reaped script was still reported running".to_owned());
    }
    // An empty pid file takes the same path.
    std::fs::write(&pid_file, "").map_err(|error| error.to_string())?;
    if running(&script, &pid_file)? {
        return Err("an empty pid file was reported running".to_owned());
    }
    // A pgrep that fails or stalls is an error, never "not running".
    let failing = scratch.script("failing-pgrep", "exit 2");
    if script_running_with(&failing, &script).is_ok() {
        return Err("a failing pgrep was taken as an answer".to_owned());
    }
    let (stalled, _) = scratch.stalled("stalled-pgrep");
    if script_running_with(&stalled, &script).is_ok() {
        return Err("a stalled pgrep was taken as an answer".to_owned());
    }
    Ok(())
}

/// A reader that yields `prefix` and then fails (at once when the prefix is empty): what a pipe that breaks
/// mid-read does, which no real helper can be made to do on demand.
struct FailingRead {
    prefix: &'static [u8],
}

impl Read for FailingRead {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.prefix.is_empty() {
            return Err(std::io::Error::other("injected read failure"));
        }
        let taken = self.prefix.len().min(buffer.len());
        buffer[..taken].copy_from_slice(&self.prefix[..taken]);
        self.prefix = &self.prefix[taken..];
        Ok(taken)
    }
}

/// What reading a stream that fails after `prefix` gives.
fn failing(prefix: &'static [u8]) -> Drained {
    read_all(Some(FailingRead { prefix }))
}

/// The wait status of a process that exited with `code`.
fn exit_status(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(code << 8)
}

/// The two shapes of a read failure: before any byte, and after a prefix that looks clean.
const FAILURE_SHAPES: [&[u8]; 2] = [b"", b"1 1\n2 2\n"];

/// A read failure on either stream of a worker fails its case, even for a zero status and streams that
/// look clean, and the failure names the stream and the error.
fn check_a_read_failure_fails_a_worker() -> Result<(), String> {
    for prefix in FAILURE_SHAPES {
        for (stream, outcome) in [
            (
                "stdout",
                exited(Some(0), failing(prefix), Ok(String::new())),
            ),
            (
                "stderr",
                exited(Some(0), Ok(String::new()), failing(prefix)),
            ),
        ] {
            let Outcome::ReadFailed(detail) = &outcome else {
                return Err(format!(
                    "a failed {stream} read after {} bytes gave {outcome:?}",
                    prefix.len()
                ));
            };
            if !detail.contains(stream) || !detail.contains("injected read failure") {
                return Err(format!(
                    "the failure does not name {stream} and the error: {detail}"
                ));
            }
            match judge("control", &outcome) {
                Err(message)
                    if message.contains(stream) && message.contains("injected read failure") => {}
                other => {
                    return Err(format!(
                        "a failed {stream} read must fail the case and say so, got {other:?}"
                    ));
                }
            }
        }
    }
    let both = exited(Some(0), failing(b""), failing(b"x\n"));
    match &both {
        Outcome::ReadFailed(detail) if detail.contains("stdout") && detail.contains("stderr") => {}
        other => {
            return Err(format!(
                "two failed reads must both be named, got {other:?}"
            ));
        }
    }
    // The real reader thread hands the failure over as a failure, with how much it had read.
    let received = drain(Some(FailingRead { prefix: b"abc" }))
        .map_err(|error| error.to_string())?
        .recv_timeout(PIPE_GRACE)
        .map_err(|error| format!("the reader sent nothing: {error}"))?;
    match received {
        Err(error)
            if error.contains("injected read failure") && error.contains("after 3 bytes") => {}
        other => {
            return Err(format!(
                "the reader thread must send the failure, got {other:?}"
            ));
        }
    }
    // Controls: clean streams still pass, and a clean checker report is still the checker's.
    judge(
        "control",
        &exited(Some(0), Ok("fine\n".to_owned()), Ok(String::new())),
    )?;
    match judge(
        "control",
        &exited(
            Some(0),
            Ok(String::new()),
            Ok(format!("{CHECKER_REPORT}: a violation")),
        ),
    ) {
        Err(message) if message.contains(CHECKER_REPORT) => Ok(()),
        other => Err(format!(
            "a checker report must still fail the case, got {other:?}"
        )),
    }
}

/// The step `supervise` takes once the worker has exited, with a zero status and one stream that fails.
fn check_the_exit_step_fails_on_a_read_failure() -> Result<(), String> {
    // The step `supervise` takes when the worker has exited, with a zero status. The kill helpers do
    // nothing, so no real process group is touched.
    let scratch = Scratch::new();
    let marker = scratch.0.join("kill-called");
    let harmless = scratch.script("kill-nothing", &format!("touch '{}'", marker.display()));
    let tools = Tools {
        kill: harmless.clone(),
        shell: harmless,
        ..Tools::system()
    };
    let group = 4_000_000;
    for prefix in FAILURE_SHAPES {
        let bad = || drain(Some(FailingRead { prefix })).map_err(|error| error.to_string());
        let clean = || drain(Some(&b"fine\n"[..])).map_err(|error| error.to_string());
        for (stream, outcome) in [
            (
                "stdout",
                finish_worker(Some(0), &bad()?, &clean()?, &tools, group),
            ),
            (
                "stderr",
                finish_worker(Some(0), &clean()?, &bad()?, &tools, group),
            ),
        ] {
            match &outcome {
                Outcome::ReadFailed(detail)
                    if detail.contains(stream) && detail.contains("injected read failure") => {}
                other => {
                    return Err(format!(
                        "a worker that exited 0 with a failed {stream} read gave {other:?}"
                    ));
                }
            }
        }
    }
    if !marker.exists() {
        return Err("a read failure did not end the worker's group as well".to_owned());
    }
    std::fs::remove_file(&marker).map_err(|error| error.to_string())?;
    let clean = |text: &'static [u8]| drain(Some(text)).map_err(|error| error.to_string());
    let outcome = finish_worker(Some(0), &clean(b"out\n")?, &clean(b"")?, &tools, group);
    match outcome {
        Outcome::Exited {
            code: Some(0),
            stdout,
            ..
        } if stdout == "out\n" => {}
        other => return Err(format!("a worker with clean streams gave {other:?}")),
    }
    if marker.exists() {
        return Err("a worker with clean streams had its group killed".to_owned());
    }
    Ok(())
}

/// A worker that exited with one stream failing to be read and the other unable to finish (held open past
/// the pipe grace, or its reader gone): the read error is still reported, with its stream and error text.
fn check_a_read_error_survives_a_sibling_that_cannot_finish() -> Result<(), String> {
    let scratch = Scratch::new();
    let harmless = scratch.script("kill-nothing", "exit 0");
    let tools = Tools {
        kill: harmless.clone(),
        shell: harmless,
        ..Tools::system()
    };
    let group = 4_000_000;
    for bad_is_stdout in [true, false] {
        for prefix in FAILURE_SHAPES {
            for held_open in [false, true] {
                let bad = drain(Some(FailingRead { prefix })).map_err(|error| error.to_string())?;
                let (sender, sibling) = mpsc::channel::<Drained>();
                // Held open: the sender lives through the call and sends nothing. Disconnected: it is gone.
                let kept = held_open.then_some(sender);
                let (stdout, stderr) = if bad_is_stdout {
                    (&bad, &sibling)
                } else {
                    (&sibling, &bad)
                };
                let stream = if bad_is_stdout { "stdout" } else { "stderr" };
                let outcome = finish_worker(Some(0), stdout, stderr, &tools, group);
                drop(kept);
                let after = format!("after {} bytes", prefix.len());
                match &outcome {
                    Outcome::ReadFailed(detail)
                        if detail.contains(&format!("the worker's {stream} could not be read"))
                            && detail.contains("injected read failure")
                            && detail.contains(&after)
                            && detail.contains("did not finish") => {}
                    other => {
                        return Err(format!(
                            "a failed {stream} read ({after}) with its sibling {} gave {other:?}",
                            if held_open {
                                "held open"
                            } else {
                                "disconnected"
                            }
                        ));
                    }
                }
                match judge("control", &outcome) {
                    Err(message)
                        if message.contains(stream)
                            && message.contains("injected read failure") => {}
                    other => {
                        return Err(format!(
                            "the case must fail naming the {stream} read error, got {other:?}"
                        ));
                    }
                }
            }
        }
    }
    // Control: with no read error, a sibling held open is still the pipes-held-open failure.
    let (_sender, open) = mpsc::channel::<Drained>();
    let clean = drain(Some(&b"fine\n"[..])).map_err(|error| error.to_string())?;
    match finish_worker(Some(0), &clean, &open, &tools, group) {
        Outcome::Unterminated(detail) if detail.contains("kept its output pipes open") => Ok(()),
        other => Err(format!("a held-open stream alone gave {other:?}")),
    }
}

/// A read failure never makes a helper's answer: a listing that would look empty or complete if truncated,
/// a developer directory, a symbol list, `pgrep`'s "no match" and `kill -0`'s "no process" are all errors.
fn check_a_read_failure_is_never_a_helper_answer() -> Result<(), String> {
    for prefix in FAILURE_SHAPES {
        let shape = format!("after {} bytes", prefix.len());
        let result = helper_result("ps", exit_status(0), failing(prefix));
        match &result {
            Err(HelperError::Read { name, error })
                if name == "ps" && error.contains("injected read failure") => {}
            other => return Err(format!("a ps read failure {shape} gave {other:?}")),
        }
        let text = result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        if !text.contains("ps") || !text.contains("injected read failure") {
            return Err(format!(
                "the reported error does not name the helper and the error: {text}"
            ));
        }
        if let Ok(members) = members_of(result, 7) {
            return Err(format!(
                "a partial listing {shape} gave the group {members:?}"
            ));
        }
        let developer = developer_path(helper_result(
            "xcode-select",
            exit_status(0),
            failing(prefix),
        ));
        if developer.is_ok() {
            return Err(format!(
                "a partial developer directory {shape} was taken: {developer:?}"
            ));
        }
        let symbols = helper_result("nm", exit_status(0), failing(prefix));
        if symbols.is_ok() {
            return Err(format!(
                "a partial symbol list {shape} was taken: {symbols:?}"
            ));
        }
        for code in [0, 1] {
            let pgrep = pgrep_verdict(
                helper_result("pgrep", exit_status(code), failing(prefix)),
                "/x",
            );
            if pgrep.is_ok() {
                return Err(format!(
                    "pgrep exit {code} with a read failure {shape} gave {pgrep:?}"
                ));
            }
            let kill = pid_verdict(
                helper_result("kill", exit_status(code), failing(prefix)),
                "1",
            );
            if kill.is_ok() {
                return Err(format!(
                    "kill -0 exit {code} with a read failure {shape} gave {kill:?}"
                ));
            }
        }
    }
    Ok(())
}

/// The positive controls: a helper read to its end, including an empty listing and the exit statuses that
/// mean something, is still an answer, through the real helper runner as well.
fn check_clean_helper_output_is_still_an_answer() -> Result<(), String> {
    let listing = |text: &str, group| {
        members_of(
            helper_result("ps", exit_status(0), Ok(text.to_owned())),
            group,
        )
    };
    if listing("1 7\n2 7\n3 9\n", 7) != Ok(vec![1, 2]) {
        return Err("a complete listing did not give its members".to_owned());
    }
    if listing("", 7) != Ok(Vec::new()) {
        return Err("a complete empty listing is an empty group".to_owned());
    }
    let pgrep = |code| {
        pgrep_verdict(
            helper_result("pgrep", exit_status(code), Ok(String::new())),
            "/x",
        )
    };
    if pgrep(0) != Ok(true) || pgrep(1) != Ok(false) || pgrep(2).is_ok() {
        return Err("pgrep's complete answers (0 yes, 1 no, other an error) changed".to_owned());
    }
    let kill = |code| {
        pid_verdict(
            helper_result("kill", exit_status(code), Ok(String::new())),
            "1",
        )
    };
    if kill(0) != Ok(true) || kill(1) != Ok(false) {
        return Err("kill -0's complete answers changed".to_owned());
    }
    let printed = developer_path(helper_result(
        "xcode-select",
        exit_status(0),
        Ok("/Applications/Xcode.app/Contents/Developer\n".to_owned()),
    ));
    if printed != Ok(PathBuf::from("/Applications/Xcode.app/Contents/Developer")) {
        return Err(format!(
            "a complete developer directory changed: {printed:?}"
        ));
    }
    let scratch = Scratch::new();
    let speaks = scratch.script("speaks", "printf '4 7\\n'");
    match run_helper(&speaks, &[], HELPER_BUDGET) {
        Ok(text) if text == "4 7\n" => {}
        other => return Err(format!("a helper that prints and exits 0 gave {other:?}")),
    }
    let silent = scratch.script("silent-exit-1", "exit 1");
    match run_helper(&silent, &[], HELPER_BUDGET) {
        Err(HelperError::Exit { code: Some(1), .. }) => Ok(()),
        other => Err(format!("a helper that exits 1 gave {other:?}")),
    }
}

/// The library carries none of the test support: its archive has the presenter's symbols and no `ck_t_`
/// or bindless-probe function; the test archive has both support families.
fn check_test_support_is_not_in_the_library() -> Result<(), String> {
    let nm = Path::new("/usr/bin/nm");
    let library = run_helper(nm, &["-g", env!("CK_METAL_ARCHIVE")], HELPER_BUDGET)?;
    let testing = run_helper(nm, &["-g", env!("CK_METAL_TESTING_ARCHIVE")], HELPER_BUDGET)?;
    if !library.contains("_ck_pr_frame") || !library.contains("_ck_metal_create") {
        return Err("the library archive does not have the presenter's symbols".to_owned());
    }
    if library.contains("ck_t_") || library.contains("_ck_test_bindless_") {
        return Err("the library archive contains presenter or bindless test support".to_owned());
    }
    if !testing.contains("_ck_t_install_input_filter")
        || !testing.contains("_ck_t_start_typist")
        || !testing.contains("_ck_t_set_rig_layer")
        || !testing.contains("_ck_t_print_minimise_diagnostics")
        || !testing.contains("_ck_t_rig_is_miniaturized")
        || !testing.contains("_ck_test_bindless_probe_new")
        || !testing.contains("_ck_test_bindless_table_entry")
        || !testing.contains("_ck_test_bindless_table_length")
    {
        return Err(
            "the test support archive lacks its presenter or bindless probe helpers".to_owned(),
        );
    }
    if testing.contains("_ck_pr_") || testing.contains("_ck_metal_") {
        return Err("the test support archive repeats the library".to_owned());
    }
    Ok(())
}

/// Which part of this target a run executes. The target mixes two kinds of work that the project's check groups keep apart:
/// `gpu` needs the GPU and the Metal toolchain but opens **no window** (the off-screen readback and the supervisor's own
/// machinery); the window cases open real windows without taking desktop focus, and are split by the module they are listed in
/// into `window-core` (`presenter.rs` and `presenter_quad/`), `window-scene` (`presenter_scene/cases.rs`), `window-compat`
/// (`presenter_scene/compat.rs`) and `window-instanced` (`presenter_scene/instanced.rs`), so that no check group nears the batch
/// timing gate. With no `--set` all five run, as plain `cargo test --test presenter` always has. An unknown, empty, repeated or valueless `--set` is an error
/// (the old name `window` included: a stale caller fails loudly instead of quietly running a third of the cases), and so is a set
/// that selects nothing: never "run nothing". A single `--case` is allowed only after one explicit set and only for a case in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaseSet {
    Gpu,
    WindowCore,
    WindowScene,
    WindowCompat,
    WindowInstanced,
}

/// The argument that restricts a run to one set.
const SET_FLAG: &str = "--set";
/// The argument that restricts a window-set run to one named case.
const CASE_FLAG: &str = "--case";
/// The sets by name.
const SETS: [(&str, CaseSet); 5] = [
    ("gpu", CaseSet::Gpu),
    ("window-core", CaseSet::WindowCore),
    ("window-scene", CaseSet::WindowScene),
    ("window-compat", CaseSet::WindowCompat),
    ("window-instanced", CaseSet::WindowInstanced),
];
/// The names, for the messages.
const SET_NAMES: &str = "gpu, window-core, window-scene, window-compat and window-instanced";

/// The set of the worker case `case` (see [`tagged_cases`]).
fn case_set(case: &str) -> CaseSet {
    tagged_cases()
        .find(|((name, _), _)| *name == case)
        .map_or(CaseSet::WindowCore, |(_, set)| set)
}

/// The sets a run was asked for: all five without `--set`, else exactly the named one.
fn requested_sets(arguments: &[String]) -> Result<Vec<CaseSet>, String> {
    let mut named: Option<CaseSet> = None;
    let mut position = 0;
    while position < arguments.len() {
        let argument = &arguments[position];
        position += 1;
        if argument != SET_FLAG {
            if argument.starts_with(SET_FLAG) {
                return Err(format!(
                    "{argument:?} is not understood; use `{SET_FLAG} <set>` with one of {SET_NAMES}"
                ));
            }
            continue;
        }
        let Some(value) = arguments.get(position) else {
            return Err(format!("{SET_FLAG} needs a value ({SET_NAMES})"));
        };
        position += 1;
        let Some((_, set)) = SETS.iter().find(|(name, _)| name == value) else {
            return Err(format!(
                "unknown case set {value:?}; the sets are {SET_NAMES}"
            ));
        };
        if named.replace(*set).is_some() {
            return Err(format!("{SET_FLAG} was given more than once"));
        }
    }
    Ok(named.map_or_else(
        || SETS.iter().map(|(_, set)| *set).collect(),
        |set| vec![set],
    ))
}

/// An optional single case selection. It is valid only after one explicit set and only when that
/// set owns the named case; `requested_sets` remains the source of truth for set validation.
fn requested_case(arguments: &[String], sets: &[CaseSet]) -> Result<Option<String>, String> {
    let mut explicit_set_seen = false;
    let mut selected = None;
    let mut position = 0;
    while position < arguments.len() {
        let argument = &arguments[position];
        position += 1;
        if argument == SET_FLAG {
            explicit_set_seen = true;
            position += 1; // `requested_sets` has already validated the set and its value.
            continue;
        }
        if argument != CASE_FLAG {
            if argument.starts_with(CASE_FLAG) {
                return Err(format!(
                    "{argument:?} is not understood; use `{CASE_FLAG} <case>`"
                ));
            }
            continue;
        }
        if !explicit_set_seen {
            return Err(format!(
                "{CASE_FLAG} must follow an explicit `{SET_FLAG} <set>`"
            ));
        }
        let Some(name) = arguments.get(position) else {
            return Err(format!("{CASE_FLAG} needs a case name"));
        };
        position += 1;
        if name.is_empty() {
            return Err(format!("{CASE_FLAG} needs a non-empty case name"));
        }
        if selected.is_some() {
            return Err(format!("{CASE_FLAG} was given more than once"));
        }
        if !all_cases().any(|(case, _)| *case == name.as_str()) {
            return Err(format!("unknown presenter case {name:?}"));
        }
        if !sets.contains(&case_set(name)) {
            return Err(format!(
                "case {name:?} is not in the requested case set {sets:?}"
            ));
        }
        selected = Some(name.clone());
    }
    Ok(selected)
}

/// The sets partition the target: every case is in exactly one, each set has a case and a check, the `gpu` set is exactly the
/// readbacks, and each module's window cases are in the window set that module belongs to.
fn check_the_case_sets_partition_the_target() -> Result<(), String> {
    let tagged: Vec<(&str, CaseSet)> = tagged_cases().map(|(case, set)| (case.0, set)).collect();
    // A name is listed once: a repeat would make one case belong to two arrays, hence possibly to two sets.
    let mut names: Vec<&str> = tagged.iter().map(|(name, _)| *name).collect();
    names.sort_unstable();
    if let Some(twice) = names.windows(2).find_map(|pair| match pair {
        [first, second] if first == second => Some(*first),
        _ => None,
    }) {
        return Err(format!("the case {twice:?} is listed twice"));
    }
    for (name, set) in SETS {
        if !tagged.iter().any(|(_, own)| *own == set) {
            return Err(format!("the {name} set has no case"));
        }
        if !CHECKS.iter().any(|(_, sets, _)| sets.contains(&set)) {
            return Err(format!("the {name} set has no supervisor check"));
        }
    }
    let gpu: Vec<&str> = tagged
        .iter()
        .filter(|(_, set)| *set == CaseSet::Gpu)
        .map(|(name, _)| *name)
        .collect();
    if !gpu.contains(&"readback") || !gpu.iter().all(|case| case.starts_with("readback")) {
        return Err(format!(
            "the gpu set is {gpu:?}; only the off-screen readbacks open no window"
        ));
    }
    if tagged
        .iter()
        .any(|(name, set)| name.starts_with("readback") && *set != CaseSet::Gpu)
    {
        return Err("a readback is not in the gpu set".to_owned());
    }
    // The array rule, stated per array here (not through `tagged_cases`): a module's window cases are in its set.
    let modules: [(&str, Vec<&str>, CaseSet); 5] = [
        (
            "presenter.rs and presenter_quad/",
            CASES
                .iter()
                .chain(MORE_CASES.iter())
                .chain(quad::CASES.iter())
                .chain(quad::MORE.iter())
                .map(|case| case.0)
                .collect(),
            CaseSet::WindowCore,
        ),
        (
            "presenter_metrics/",
            metrics::CASES
                .iter()
                .chain(metrics::MORE.iter())
                .map(|case| case.0)
                .collect(),
            CaseSet::WindowCore,
        ),
        (
            "presenter_scene/cases.rs",
            scene::CASES
                .iter()
                .chain(scene::MORE.iter())
                .map(|case| case.0)
                .collect(),
            CaseSet::WindowScene,
        ),
        (
            "presenter_scene/compat.rs",
            scene::compat::CASES
                .iter()
                .chain(scene::compat::MORE.iter())
                .chain(scene::compat::BINDLESS.iter())
                .map(|case| case.0)
                .collect(),
            CaseSet::WindowCompat,
        ),
        (
            "presenter_scene/instanced.rs",
            scene::instanced::CASES
                .iter()
                .chain(scene::instanced::MORE.iter())
                .chain(scene::instanced::BINDLESS.iter())
                .map(|case| case.0)
                .collect(),
            CaseSet::WindowInstanced,
        ),
    ];
    for (module, cases, expected) in modules {
        for case in cases {
            if case.starts_with("readback") {
                continue;
            }
            let seen = case_set(case);
            if seen != expected {
                return Err(format!(
                    "the case {case:?} of {module} is in {seen:?}, not {expected:?}"
                ));
            }
        }
    }
    Ok(())
}

/// The set arguments are validated: none means all five, one name means that set, anything else is an error (the old name
/// `window` included).
fn check_the_case_set_arguments_are_validated() -> Result<(), String> {
    let args = |words: &[&str]| {
        words
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>()
    };
    let all = vec![
        CaseSet::Gpu,
        CaseSet::WindowCore,
        CaseSet::WindowScene,
        CaseSet::WindowCompat,
        CaseSet::WindowInstanced,
    ];
    if requested_sets(&args(&[])) != Ok(all.clone())
        || requested_sets(&args(&["--nocapture", "filter"])) != Ok(all)
    {
        return Err("no --set must mean all five sets, ignoring Cargo's own arguments".to_owned());
    }
    for (name, set) in SETS {
        if requested_sets(&args(&["--set", name])) != Ok(vec![set])
            || requested_sets(&args(&["--nocapture", "--set", name])) != Ok(vec![set])
        {
            return Err(format!("--set {name} must select exactly that set"));
        }
    }
    for bad in [
        &["--set"][..],
        &["--set", ""],
        &["--set", "all"],
        &["--set", "GPU"],
        &["--set", "window"],
        &["--set", "gpu", "--set", "window-core"],
        &["--set", "gpu", "--set", "gpu"],
        &["--set=gpu"],
        &["--sets", "gpu"],
    ] {
        if requested_sets(&args(bad)).is_ok() {
            return Err(format!("{bad:?} must be an error, not a run"));
        }
    }
    // The stale name fails loudly and names the five sets.
    let stale = requested_sets(&args(&["--set", "window"])).expect_err("`window` is not a set");
    if !(stale.contains("unknown case set") && SETS.iter().all(|(name, _)| stale.contains(name))) {
        return Err(format!(
            "the stale name's error does not list the sets: {stale}"
        ));
    }
    let case = "proven_teardown_minimised";
    let core = [CaseSet::WindowCore];
    if requested_case(&args(&["--set", "window-core", "--case", case]), &core)
        != Ok(Some(case.to_owned()))
        || requested_case(&args(&["--set", "window-core"]), &core) != Ok(None)
    {
        return Err("a case filter must select one case inside its explicit set".to_owned());
    }
    for bad in [
        &["--case", case][..],
        &["--set", "window-core", "--case"],
        &["--set", "window-core", "--case", ""],
        &["--set", "window-core", "--case", "missing_case"],
        &["--set", "window-scene", "--case", case],
        &["--set", "window-core", "--case", case, "--case", case],
        &["--case", case, "--set", "window-core"],
        &["--set", "window-core", "--case=proven_teardown_minimised"],
    ] {
        let bad_args = args(bad);
        let invalid = match requested_sets(&bad_args) {
            Ok(sets) => requested_case(&bad_args, &sets).is_ok(),
            Err(_) => false,
        };
        if invalid {
            return Err(format!("invalid case filter {bad:?} must be refused"));
        }
    }
    Ok(())
}

/// A check of the supervisor's own machinery.
type SupervisorCheck = fn() -> Result<(), String>;

/// A check that belongs to the `gpu` set only.
const GPU_ONLY: &[CaseSet] = &[CaseSet::Gpu];
/// A check that runs in **every window set's** run (once in a run of several): the checker control proves the Main Thread
/// Checker is live for that run.
const WINDOW_SETS: &[CaseSet] = &[
    CaseSet::WindowCore,
    CaseSet::WindowScene,
    CaseSet::WindowCompat,
    CaseSet::WindowInstanced,
];

/// The supervisor's own checks, each in the set it belongs to: only the checker control opens a window
/// (its worker is `checker_control`, which opens one); the rest use the windowless `hang` fixture or no
/// worker at all.
static CHECKS: [(&str, &[CaseSet], SupervisorCheck); 14] = [
    (
        "a hung worker is killed and reaped",
        GPU_ONLY,
        check_a_hung_worker_is_killed,
    ),
    (
        "the checker reports a violation off the main thread",
        WINDOW_SETS,
        check_the_checker_reports_a_violation,
    ),
    (
        "a faulty process list is bounded and fails closed",
        GPU_ONLY,
        check_a_faulty_process_list_is_bounded_and_fails_closed,
    ),
    (
        "a stalled kill helper is bounded",
        GPU_ONLY,
        check_a_stalled_kill_helper_is_bounded,
    ),
    (
        "a stalled xcode-select is bounded",
        GPU_ONLY,
        check_a_stalled_xcode_select_is_bounded,
    ),
    (
        "a helper without a pid is found by its script",
        GPU_ONLY,
        check_a_helper_without_a_pid_is_found_by_its_script,
    ),
    (
        "a read failure fails a worker whatever its status and bytes",
        GPU_ONLY,
        check_a_read_failure_fails_a_worker,
    ),
    (
        "the exit step fails on a read failure whatever the status",
        GPU_ONLY,
        check_the_exit_step_fails_on_a_read_failure,
    ),
    (
        "a read error survives a sibling stream that cannot finish",
        GPU_ONLY,
        check_a_read_error_survives_a_sibling_that_cannot_finish,
    ),
    (
        "a read failure is never a complete helper answer",
        GPU_ONLY,
        check_a_read_failure_is_never_a_helper_answer,
    ),
    (
        "a helper that reads cleanly is still an answer",
        GPU_ONLY,
        check_clean_helper_output_is_still_an_answer,
    ),
    (
        "the test support is not in the library",
        GPU_ONLY,
        check_test_support_is_not_in_the_library,
    ),
    (
        "the case sets partition the target",
        GPU_ONLY,
        check_the_case_sets_partition_the_target,
    ),
    (
        "the case set arguments are validated",
        GPU_ONLY,
        check_the_case_set_arguments_are_validated,
    ),
];

fn supervisor(arguments: &[String]) -> ExitCode {
    let sets = match requested_sets(arguments) {
        Ok(sets) => sets,
        Err(message) => {
            eprintln!("presenter: {message}");
            return ExitCode::FAILURE;
        }
    };
    let selected_case = match requested_case(arguments, &sets) {
        Ok(case) => case,
        Err(message) => {
            eprintln!("presenter: {message}");
            return ExitCode::FAILURE;
        }
    };
    let checks: Vec<_> = CHECKS
        .iter()
        .filter(|(_, own, _)| own.iter().any(|set| sets.contains(set)))
        .collect();
    let cases: Vec<_> = all_cases()
        .filter(|(name, _)| {
            sets.contains(&case_set(name))
                && selected_case
                    .as_ref()
                    .is_none_or(|selected| selected.as_str() == *name)
        })
        .collect();
    if checks.is_empty() || cases.is_empty() {
        eprintln!("presenter: the selected case set {sets:?} selects no check or no case");
        return ExitCode::FAILURE;
    }
    if let Err(message) = checker_library(&Tools::system()) {
        eprintln!("presenter: {message}");
        return ExitCode::FAILURE;
    }
    let mut failures = BTreeMap::new();
    for (name, _, check) in &checks {
        let result = check();
        println!(
            "test supervisor: {name} ... {}",
            if result.is_ok() { "ok" } else { "FAILED" }
        );
        if let Err(message) = result {
            failures.insert((*name).to_owned(), message);
        }
    }
    // Every case runs on a quiet keyboard, and again while the owner types without stopping.
    for typing in [false, true] {
        for (name, _) in &cases {
            let label = if typing {
                format!("{name} (owner typing)")
            } else {
                (*name).to_owned()
            };
            let started = Instant::now();
            let result = run_case(name, typing);
            println!(
                "test {label} ... {} ({:.1} s)",
                if result.is_ok() { "ok" } else { "FAILED" },
                started.elapsed().as_secs_f64()
            );
            if let Err(message) = result {
                failures.insert(label, message);
            }
        }
    }
    for (name, message) in &failures {
        println!("---- {name} ----\n{message}");
    }
    let total = checks.len() + 2 * cases.len();
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

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [flag, case] if flag == WORKER_FLAG => worker(case),
        // Cargo may pass its own arguments (a filter, `--nocapture`); the supervisor validates any
        // optional --case selection after the single explicit --set.
        _ => supervisor(&args),
    }
}
