//! The Metal presenter: a render thread that owns every GPU object and clears a window's `CAMetalLayer`.
//!
//! See the crate documentation for the contract. The pieces: [`MetalPresenter`] is the handle the main
//! thread drives; the render thread (below) owns the device, the command queue, the layer retain and the
//! tokens, in a native core that nothing releases except the explicit teardown; [`TestHooks`] is the
//! only way to inject a fault, and only through [`MetalPresenter::start_with_test_hooks`].

use std::collections::{HashMap, VecDeque};
use std::ffi::{c_char, c_void};
use std::hash::Hash;
use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::batch::{
    Batch, CompatDecision, FrameCounters, INSTANCE_STRIDE, InstanceRecord, Layout, Resolved,
    build_scene_in,
};
use crate::bindless::BindlessLimits;
use crate::frame::{InstanceRole, MetalFrameSlot, MetalInstance, MetalSceneSource, SceneStats};
use crate::ledger::LedgerLimits;
use crate::metrics::{
    EndError, FrameMarks, FrameStats, MetricsWindow, Recorder, RequestError, Tracked, now_ns,
};
use crate::quad::{
    DestinationRect, MetalFilter, MetalQuad, MetalQuadSlot, MetalScene, PixelSource,
};
use crate::scene::{
    Baseline, Boundary, BuildRequest, CHECKER_SIDE, CheckerProgress, FrameGate, FrameSeam, Scene,
    SceneLimits, SceneSetup, TextureDriver, TicketEnding, checker_pixels, wakes_of,
};
use crate::textures::{MetalContext, TextureLimits};
use crate::{Error, message, unwind};

/// The compiled quad shader library (`shaders/quad.metal`), embedded by the build script.
const QUAD_LIBRARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/quad.metallib"));

/// The stop budget: the moment of the first `request_stop` plus this.
const STOP_BUDGET: Duration = Duration::from_millis(1500);
/// How long the render thread waits between looks at outstanding completions.
const POLL: Duration = Duration::from_millis(2);

/// How long [`MetalPresenter::read_metrics`] waits between looks while a window's end has not passed on the clock.
const METRICS_WAIT_SLICE: Duration = Duration::from_millis(5);

/// A copy of one measurement window of the presenter (CK-129), as [`MetalPresenter::metrics_snapshot`] and
/// [`MetalPresenter::read_metrics`] return it.
#[derive(Debug, Clone)]
pub struct MetricsReading {
    /// The window, its adoption time, whether it was adopted late, the samples counted outside it and the results dropped late, with
    /// the presenter's statistics for it.
    pub window: Tracked<FrameStats>,
    /// Frames committed in the window whose GPU time or presentation was still outstanding when the reading was taken.
    pub outstanding: u64,
    /// This owner had passed the window's end and had nothing outstanding: the reading is the window's final one.
    pub complete: bool,
    /// Timing results the native ring had no room for (never expected).
    pub timing_lost: u64,
    /// The presenter's bound on committed frames with an unconsumed presentation was exceeded at some point (never expected: it
    /// holds more than the drawable pool and the one-frame-in-flight rule allow): the figures are insufficient evidence and
    /// `complete` stays false.
    pub evidence_lost: bool,
    /// The window's latest shown drawable lies inside it and no drawable has been shown at or after its end yet (the presenter is still
    /// running): the interval that will close across the end is still to come and `excluded_intervals` can gain it, so `complete`
    /// stays false until a drawable is shown after the end or the presenter has ended.
    pub closing_pending: bool,
}

/// The presenter's admission lock held for setting the end of a window ([`MetalPresenter::lock_metrics_end`]). With the window
/// loop's lock also held, one timestamp read under both is the instant before which everything either owner admitted lies, and at or
/// after which nothing was: both owners then cover exactly the same `[start, end)`.
pub struct MetricsEndGuard<'a> {
    metrics: MutexGuard<'a, Recorder>,
    changed: &'a Condvar,
}

impl MetricsEndGuard<'_> {
    /// Sets the end of window `id`, once, while nothing is being admitted.
    ///
    /// # Errors
    ///
    /// The window is unknown, already ended, or `end` is not after its start.
    pub fn end(&mut self, id: u32, end: u64) -> Result<(), EndError> {
        self.metrics.windows.end(id, end)
    }
}

impl Drop for MetricsEndGuard<'_> {
    fn drop(&mut self) {
        self.changed.notify_all();
    }
}

impl MetricsReading {
    fn of(
        window: Tracked<FrameStats>,
        timing_lost: u64,
        complete: bool,
        evidence_lost: bool,
        closing_pending: bool,
    ) -> Self {
        Self {
            outstanding: window.stats.outstanding,
            window,
            complete,
            timing_lost,
            evidence_lost,
            closing_pending,
        }
    }
}

/// The native core (`CKPrCore`): every GPU object the presenter owns. Used on the render thread only.
#[derive(Clone, Copy)]
struct Core(NonNull<c_void>);

// SAFETY: the pointer is moved to the render thread once, before any use, and every native call on it is
// made there. It has no `Drop`, so a thread that ends by unwinding releases nothing.
unsafe impl Send for Core {}

/// The native frame faults (`CKPrFaults`): all zero in production.
#[repr(C)]
#[derive(Clone, Copy)]
struct Faults {
    abandon_after_acquire: i32,
    nil_command_buffer: i32,
    nil_render_pass: i32,
    nil_encoder: i32,
    partial_encoding: i32,
    error_completion: i32,
    never_complete_drawable_tokens: i32,
    never_complete_command_tokens: i32,
    handler_delay_ms: i32,
    completion_delay_ms: i32,
    fail_texture_allocation: i32,
    fail_quad_encode: i32,
    retired_token_delay_ms: i32,
    pause_after_reap_for_token: i32,
    abandon_after_prepare: i32,
    skip_next_drawable: i32,
    capture: i32,
    completion_gate: *const AtomicI32,
    /// A test's blocked `nextDrawable`: the step waits inside the acquisition while this reads non-zero. Read inside the call.
    hold_acquire: *const AtomicI32,
    hold_after_commit: *const AtomicI32,
}

/// The seam through which a scene presenter's frame attaches its ledger token (`CKPrTicket`).
#[repr(C)]
struct NativeTicket {
    userdata: *mut c_void,
    prepare: Option<unsafe extern "C" fn(*mut c_void, *mut c_char, usize) -> *mut c_void>,
}

/// The textured quad of one frame (`CKPrQuad`); `present` 0 means none.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeQuad {
    present: i32,
    filter: i32,
    destination: [i32; 4],
    source: [f32; 4],
    width: u32,
    height: u32,
}

/// A scene frame's draw list (`CKPrPacket`): its instances are already in the core's instance buffer.
#[repr(C)]
struct NativePacket {
    batches: *const Batch,
    batch_count: u32,
    filter: i32,
    checker: *mut c_void,
    /// The frame's pinned bindless table (an `id<MTLBuffer>` of resource IDs, not retained) when the batches are runs drawn
    /// through it; null for the ordinary per-texture batches.
    table: *mut c_void,
    table_slots: u32,
    /// The off-screen readback's resident textures (null and 0 for the presenter, whose queue residency set holds them).
    use_resources: *const *const c_void,
    use_count: u32,
}

/// The pixels of a new texture (`CKPrUpload`).
#[repr(C)]
struct NativeUpload {
    pixels: *const u8,
    length: usize,
}

/// The pixels of one texture of an off-screen scene (`CKPrReadbackImage`).
#[repr(C)]
struct NativeReadbackImage {
    pixels: *const u8,
    length: usize,
    width: u32,
    height: u32,
}

/// What a frame step did to the quad texture (`CKPrQuadResult`).
#[repr(C)]
#[derive(Default)]
struct NativeQuadResult {
    uploaded: i32,
    removed: i32,
    /// The frame-timing marks the native step took (`CLOCK_UPTIME_RAW` nanoseconds; zero when not reached): `nextDrawable` called
    /// (t3), returned (t4), and the command buffer's `commit` returned (t5).
    acquire_begin: u64,
    acquire_end: u64,
    committed: u64,
    /// The clock read just before `commit` was called (see `CKPrQuadResult`).
    submitted: u64,
}

/// One timing result the native core consumed from a token (`CKPrTiming`): kind 1 a command buffer's GPU start and end (`a`, `b`),
/// kind 2 a drawable's presented time (`a`, zero if never shown); `t5` is the commit mark of the frame it belongs to.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeTiming {
    kind: u32,
    padding: u32,
    t5: u64,
    a: u64,
    b: u64,
}

impl NativeQuad {
    /// No quad.
    const NONE: Self = Self {
        present: 0,
        filter: 0,
        destination: [0; 4],
        source: [0.0; 4],
        width: 0,
        height: 0,
    };

    fn of(quad: &MetalQuad) -> Self {
        let destination = quad.destination();
        let source = quad.source();
        Self {
            present: 1,
            filter: i32::from(quad.filter() == MetalFilter::Linear),
            destination: [
                destination.left,
                destination.top,
                destination.right,
                destination.bottom,
            ],
            source: [source.left, source.top, source.right, source.bottom],
            width: quad.width(),
            height: quad.height(),
        }
    }
}

impl Faults {
    const NONE: Self = Self {
        abandon_after_acquire: 0,
        nil_command_buffer: 0,
        nil_render_pass: 0,
        nil_encoder: 0,
        partial_encoding: 0,
        error_completion: 0,
        never_complete_drawable_tokens: 0,
        never_complete_command_tokens: 0,
        handler_delay_ms: 0,
        completion_delay_ms: 0,
        fail_texture_allocation: 0,
        fail_quad_encode: 0,
        retired_token_delay_ms: 0,
        pause_after_reap_for_token: 0,
        abandon_after_prepare: 0,
        skip_next_drawable: 0,
        capture: 0,
        completion_gate: std::ptr::null(),
        hold_acquire: std::ptr::null(),
        hold_after_commit: std::ptr::null(),
    };
}

/// The result codes of `ck_pr_frame` (`CKPrFrame`).
const FRAME_DRAWN: i32 = 0;
const FRAME_SKIPPED: i32 = 1;
const FRAME_BUSY: i32 = 2;
/// A drawable was acquired and given back without drawing (a test fault): counted as a skipped frame.
const FRAME_ABANDONED: i32 = 3;
const FRAME_FAILED: i32 = 4;
/// The initial gate held the frame back before the native step (the compatibility image is not there yet): not a native
/// result, the presenter's own.
const FRAME_GATED: i32 = 100;

unsafe extern "C" {
    fn ck_pr_layer_retain(layer: *mut c_void, observer: *mut c_void) -> *mut c_void;
    fn ck_pr_layer_release(retained_layer: *mut c_void, observer: *mut c_void);
    fn ck_pr_core_new(retained_layer: *mut c_void, observer: *mut c_void) -> *mut c_void;
    fn ck_pr_setup(
        core: *mut c_void,
        context: *mut c_void,
        width: u32,
        height: u32,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_pr_quad_setup(
        core: *mut c_void,
        library: *const u8,
        length: usize,
        fail_pipeline: i32,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_pr_frame_quad(
        core: *mut c_void,
        rgb: *const f32,
        width: u32,
        height: u32,
        faults: *const Faults,
        quad: *const NativeQuad,
        upload: *const NativeUpload,
        packet: *const NativePacket,
        ticket: *const NativeTicket,
        result: *mut NativeQuadResult,
        message: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_pr_instance_buffer_setup(
        core: *mut c_void,
        bytes: u64,
        allocated: *mut u64,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_pr_instance_buffer_contents(core: *mut c_void) -> *mut c_void;
    fn ck_pr_instance_buffer_free(core: *mut c_void) -> i32;
    fn ck_pr_readback_scene(
        width: u32,
        height: u32,
        rgb: *const f32,
        library: *const u8,
        library_length: usize,
        quad: *const NativeQuad,
        quad_image: *const u8,
        quad_image_length: usize,
        images: *const NativeReadbackImage,
        image_count: usize,
        checker: *const NativeReadbackImage,
        instances: *const c_void,
        instance_bytes: usize,
        batches: *const Batch,
        batch_count: u32,
        filter: i32,
        table_slots: u32,
        pixels: *mut u8,
        length: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_pr_readback_quad(
        width: u32,
        height: u32,
        rgb: *const f32,
        library: *const u8,
        library_length: usize,
        quad: *const NativeQuad,
        image: *const u8,
        image_length: usize,
        pixels: *mut u8,
        length: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_pr_set_check_pause(core: *mut c_void, entry: i32, ms: i32);
    fn ck_pr_poll(core: *mut c_void, message: *mut c_char, capacity: usize) -> i32;
    fn ck_pr_take_timing(
        core: *mut c_void,
        out: *mut NativeTiming,
        capacity: u32,
        lost: *mut u64,
    ) -> u32;
    fn ck_pr_outstanding(core: *mut c_void) -> i32;
    fn ck_pr_tokens_state(core: *mut c_void, outstanding: *mut c_char, capacity: usize) -> i32;
    fn ck_pr_release_all(core: *mut c_void);
    fn ck_pr_readback(
        width: u32,
        height: u32,
        rgb: *const f32,
        pixels: *mut u8,
        length: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_pr_observer_new() -> *mut c_void;
    fn ck_pr_observer_free(observer: *mut c_void);
    fn ck_pr_observer_get(observer: *const c_void, counter: i32) -> i64;
    fn ck_pr_observer_capture(
        observer: *const c_void,
        which: i64,
        width: *mut u32,
        height: *mut u32,
        pixels: *mut u8,
        length: usize,
    ) -> i64;
    fn ck_pr_observer_abandoned_alive(observer: *const c_void) -> i32;
}

/// What the render thread reports through [`MetalPresenter::take_report`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenterReport {
    /// The first frame was committed (queued for presentation, not proof that it is visible). Once.
    FirstFrameQueued,
    /// Setup or a frame failed. The thread draws nothing more.
    Failed {
        /// What failed.
        message: String,
    },
}

/// A frame captured off screen by [`TestHooks::capture_frames`].
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// BGRA8, rows top first.
    pub pixels: Vec<u8>,
    /// How many captures had completed when this was read.
    pub sequence: i64,
}

/// How the presenter's cleanup ended, once the thread has been observed finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupResult {
    /// The thread sent an explicit success after every completion and drawable token arrived and every
    /// holder it owned was released.
    Proven,
    /// Anything else (a panic, a missing result, a token that did not arrive by the stop deadline). The
    /// holders the thread still owned were kept.
    Unproven {
        /// Why.
        reason: String,
    },
}

/// The wake callback type.
type Wake = Arc<dyn Fn() + Send + Sync>;

/// The latest-value slot the main thread writes and the render thread reads.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the wants and the stop are independent flags read and cleared under this one mutex, so a wake is never lost"
)]
struct Slot {
    clear: [f32; 3],
    size: (u32, u32),
    /// A frame is wanted: a changed colour or size, a redraw request, or the first frame.
    dirty: bool,
    /// Texture work is pending: a store wake, `texture_wake()`, or a pass that reported runnable work left.
    /// Read and cleared under this slot's mutex like `dirty` and `stop`, and every wake sets it under the mutex
    /// before signalling, so a wake arriving while the thread decides to sleep is never lost.
    texture_work: bool,
    stop: bool,
    /// The absolute stop deadline, set once, by the first `request_stop`.
    stop_deadline: Option<Instant>,
    /// A measurement window was requested: the thread adopts it at its next boundary. Set under this mutex like the other wants.
    metrics_check: bool,
}

struct Shared {
    slot: Mutex<Slot>,
    signal: Condvar,
    reports: Mutex<VecDeque<PresenterReport>>,
    /// The cleanup result the render thread sent, if it did.
    result: Mutex<Option<CleanupResult>>,
    wake: Wake,
    stop_budget: Duration,
    /// The generation of the last frame committed (with a quad, or with none), and whether there was one.
    /// Written by the render thread only; read without waiting.
    drawn_generation: AtomicU64,
    drawn_any: AtomicBool,
    /// The scene presenter's counters and charges; written by the render thread, read by [`MetalPresenter::stats`].
    scene_stats: Mutex<SceneStats>,
    /// The frame-timing windows (CK-129): recorded by the render thread (one lock per frame, nothing allocated), read by anyone.
    metrics: Mutex<Recorder>,
    /// Signalled by the render thread when it records or consumes timing, for [`MetalPresenter::read_metrics`].
    metrics_changed: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The handle that marks texture work pending under the slot's mutex and signals the render thread. It holds the
/// presenter weakly.
fn texture_marker(shared: &Arc<Shared>) -> Wake {
    let weak = Arc::downgrade(shared);
    Arc::new(move || {
        if let Some(shared) = weak.upgrade() {
            lock(&shared.slot).texture_work = true;
            shared.signal.notify_all();
        }
    })
}

impl Shared {
    /// Calls the wake. A panic in it is caught and ignored: it never changes a result already sent.
    fn wake(&self) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| (self.wake)())) {
            unwind::dispose(payload);
        }
    }

    fn report(&self, report: PresenterReport) {
        lock(&self.reports).push_back(report);
        self.wake();
    }
}

/// A gate on frame progress, for the tests: the render thread waits for a permit before each frame.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct Gate {
    permits: Mutex<usize>,
    changed: Condvar,
}

impl Gate {
    /// A closed gate.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Lets `count` more frames through.
    pub fn open(&self, count: usize) {
        *lock(&self.permits) += count;
        self.changed.notify_all();
    }

    /// Takes a permit, waiting in short slices while `keep_waiting` says to.
    fn take(&self, keep_waiting: impl Fn() -> bool) -> bool {
        let mut permits = lock(&self.permits);
        loop {
            if *permits > 0 {
                *permits -= 1;
                return true;
            }
            if !keep_waiting() {
                return false;
            }
            permits = self
                .changed
                .wait_timeout(permits, POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// A pause of the render thread at the point where it has found nothing to do and is about to decide whether to wait, for the
/// tests: the window in which a publication must not be lost. Armed by the test, the thread arrives at it once, tells the
/// test, and goes on when the test releases it.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct WaitPause {
    state: Mutex<WaitPauseState>,
    changed: Condvar,
}

#[derive(Debug, Default)]
struct WaitPauseState {
    armed: bool,
    arrived: bool,
    released: bool,
}

impl WaitPause {
    /// A pause that is not armed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The next time the render thread is about to decide whether to wait, it stops until [`release`](Self::release).
    pub fn arm(&self) {
        *lock(&self.state) = WaitPauseState {
            armed: true,
            ..WaitPauseState::default()
        };
    }

    /// Whether the render thread is stopped at the pause now.
    #[must_use]
    pub fn arrived(&self) -> bool {
        let state = lock(&self.state);
        state.arrived && !state.released
    }

    /// Lets the stopped render thread go on.
    pub fn release(&self) {
        lock(&self.state).released = true;
        self.changed.notify_all();
    }

    /// The render thread's side: stops here if armed, in short slices while `keep_waiting` says to.
    fn pause(&self, keep_waiting: impl Fn() -> bool) {
        let mut state = lock(&self.state);
        if !state.armed || state.arrived {
            return;
        }
        state.arrived = true;
        while !state.released && keep_waiting() {
            state = self
                .changed
                .wait_timeout(state, POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        state.armed = false;
    }
}

/// A record of what the presenter did, read by the tests through counters that outlive every callback.
#[doc(hidden)]
#[derive(Debug)]
pub struct Observer {
    raw: NonNull<c_void>,
}

// SAFETY: the native observer's counters are atomics and its weak reference is read atomically; nothing
// in it is tied to a thread.
unsafe impl Send for Observer {}
// SAFETY: as above: every access is an atomic read.
unsafe impl Sync for Observer {}

impl Default for Observer {
    fn default() -> Self {
        Self::new()
    }
}

impl Observer {
    /// A new observer.
    ///
    /// # Panics
    ///
    /// If the native observer cannot be allocated.
    #[must_use]
    pub fn new() -> Self {
        // SAFETY: creates one retained native object, which this value owns and frees on drop.
        let raw = unsafe { ck_pr_observer_new() };
        Self {
            raw: NonNull::new(raw).expect("the native observer"),
        }
    }

    fn get(&self, counter: i32) -> i64 {
        // SAFETY: `raw` is live while `self` is; the call reads one atomic counter.
        unsafe { ck_pr_observer_get(self.raw.as_ptr(), counter) }
    }

    /// Strong drawable references the presenter holds right now (zero between frame steps).
    #[must_use]
    pub fn drawable_refs(&self) -> i64 {
        self.get(0)
    }

    /// The most strong drawable references it ever held at once.
    #[must_use]
    pub fn drawable_refs_max(&self) -> i64 {
        self.get(1)
    }

    /// Drawables acquired.
    #[must_use]
    pub fn drawables_acquired(&self) -> i64 {
        self.get(2)
    }

    /// Presented handlers that ran.
    #[must_use]
    pub fn presented_handlers_run(&self) -> i64 {
        self.get(3)
    }

    /// Completion handlers that ran.
    #[must_use]
    pub fn completion_handlers_run(&self) -> i64 {
        self.get(4)
    }

    /// The most command buffers in flight at once.
    #[must_use]
    pub fn command_buffers_in_flight_max(&self) -> i64 {
        self.get(5)
    }

    /// Holders (command buffer, queue, device, layer) the teardown released: zero unless `Proven`.
    #[must_use]
    pub fn holders_released(&self) -> i64 {
        self.get(6)
    }

    /// The size of the last drawable's texture, in pixels.
    #[must_use]
    pub fn last_drawable_size(&self) -> (i64, i64) {
        (self.get(7), self.get(8))
    }

    /// Frame steps run.
    #[must_use]
    pub fn frame_steps(&self) -> i64 {
        self.get(9)
    }

    /// Handlers that ran after the holders were released.
    #[must_use]
    pub fn late_handlers(&self) -> i64 {
        self.get(10)
    }

    /// Command buffers committed.
    #[must_use]
    pub fn committed(&self) -> i64 {
        self.get(11)
    }

    /// The most drawable tokens outstanding at once.
    #[must_use]
    pub fn drawable_tokens_max(&self) -> i64 {
        self.get(12)
    }

    /// Retains taken on the layer minus releases of them: zero after a failed start or a proven end,
    /// one while the presenter (or an unproven end) still holds its retain.
    #[must_use]
    pub fn layer_retains_outstanding(&self) -> i64 {
        self.get(13) - self.get(14)
    }

    /// Command buffers the core holds right now: one from a commit until the next commit replaces it or
    /// the explicit release lets it go, and never fewer because of a look at the tokens.
    #[must_use]
    pub fn buffer_holders_now(&self) -> i64 {
        self.get(16) - self.get(17)
    }

    /// Times the core let go of a command buffer (a replacement by the next commit, or the explicit
    /// release): zero for a presenter whose first frame is its only one and whose end is unproven.
    #[must_use]
    pub fn buffer_drops(&self) -> i64 {
        self.get(17)
    }

    /// Quad textures allocated and filled.
    #[must_use]
    pub fn quad_textures_created(&self) -> i64 {
        self.get(18)
    }

    /// Quad textures the core holds right now (the resident one and at most one retired one).
    #[must_use]
    pub fn quad_textures_alive(&self) -> i64 {
        self.get(19)
    }

    /// The most quad textures alive at once.
    #[must_use]
    pub fn quad_textures_max(&self) -> i64 {
        self.get(20)
    }

    /// Quad textures the core let go of.
    #[must_use]
    pub fn quad_textures_released(&self) -> i64 {
        self.get(21)
    }

    /// Quad textures let go of before the token of the last command buffer that used them: zero.
    #[must_use]
    pub fn quad_released_early(&self) -> i64 {
        self.get(22)
    }

    /// Resident quad textures retired (replaced or removed).
    #[must_use]
    pub fn quad_retired(&self) -> i64 {
        self.get(23)
    }

    /// Frame steps that waited because a retired quad texture still waited for its token.
    #[must_use]
    pub fn quad_admission_waits(&self) -> i64 {
        self.get(24)
    }

    /// The size, in pixels, of the quad texture made last: (0, 0) before any.
    #[must_use]
    pub fn quad_last_texture_size(&self) -> (i64, i64) {
        (self.get(27), self.get(28))
    }

    /// Quad textures, pipeline and samplers the explicit teardown released: zero unless `Proven`.
    #[must_use]
    pub fn quad_holders_released(&self) -> i64 {
        self.get(25)
    }

    /// Command buffers committed with a quad in them.
    #[must_use]
    pub fn quad_draws(&self) -> i64 {
        self.get(26)
    }

    /// Frames captured off screen whose command buffer completed (see [`TestHooks::capture_frames`]).
    #[must_use]
    pub fn captures(&self) -> i64 {
        self.get(31)
    }

    /// The latest captured frame: its size, its BGRA8 pixels (rows top first) and the number of captures completed when it
    /// was read. `None` before the first.
    #[must_use]
    pub fn captured_frame(&self) -> Option<CapturedFrame> {
        self.captured(-1)
    }

    /// The `index`-th captured frame (0-based, in commit order), if it is still kept (the last 64 are).
    #[must_use]
    pub fn captured_frame_at(&self, index: u32) -> Option<CapturedFrame> {
        self.captured(i64::from(index))
    }

    fn captured(&self, which: i64) -> Option<CapturedFrame> {
        let (mut width, mut height) = (0_u32, 0_u32);
        // SAFETY: the observer is live; a null pixel pointer with length zero asks for the size alone.
        let probe = unsafe {
            ck_pr_observer_capture(
                self.raw.as_ptr(),
                which,
                &raw mut width,
                &raw mut height,
                std::ptr::null_mut(),
                0,
            )
        };
        if probe != -2 {
            return None;
        }
        let mut pixels = vec![0_u8; (width as usize) * (height as usize) * 4];
        // SAFETY: the observer is live and `pixels` is exactly width x height x 4 bytes, as the call requires.
        let sequence = unsafe {
            ck_pr_observer_capture(
                self.raw.as_ptr(),
                which,
                &raw mut width,
                &raw mut height,
                pixels.as_mut_ptr(),
                pixels.len(),
            )
        };
        (sequence >= 0).then_some(CapturedFrame {
            width,
            height,
            pixels,
            sequence,
        })
    }

    /// The identity (object address) of the device the presenter took from its context; compare with
    /// [`MetalContext::device_identity`].
    #[must_use]
    pub fn device_identity(&self) -> i64 {
        self.get(29)
    }

    /// The identity of the command queue the presenter took from its context; compare with
    /// [`MetalContext::queue_identity`].
    #[must_use]
    pub fn queue_identity(&self) -> i64 {
        self.get(30)
    }

    /// Whether the last abandoned drawable is still alive (a weak reference).
    #[must_use]
    pub fn abandoned_drawable_alive(&self) -> bool {
        // SAFETY: `raw` is live while `self` is; the call reads one weak reference.
        unsafe { ck_pr_observer_abandoned_alive(self.raw.as_ptr()) != 0 }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: this consumes the one reference this value owns; callbacks hold their own.
        unsafe { ck_pr_observer_free(self.raw.as_ptr()) };
    }
}

/// Which handler tokens never arrive (lost callbacks), for the tests.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NeverComplete {
    /// Every handler sets its token.
    #[default]
    Nothing,
    /// No handler ever sets its token.
    Everything,
    /// Only the drawables' presented handlers are lost.
    Drawables,
    /// Only the command buffers' completion handlers are lost.
    Commands,
}

/// How the render thread looks for command buffer errors while it runs, for the tests.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ErrorPoll {
    /// The thread polls after each frame step and each wait.
    #[default]
    Normal,
    /// After the first completion, the thread asks whether anything is outstanding before it polls (the
    /// order that used to lose the error).
    OutstandingFirst,
    /// The thread never polls while running: the error must come through the next frame step or the
    /// teardown.
    Never,
}

/// Where a test places a completion between the native read of a command token and what its caller does
/// with the answer.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PauseSite {
    /// No pause.
    #[default]
    Nowhere,
    /// In the idle "is anything outstanding" look, which decides whether the thread waits without a timer.
    IdleLook,
    /// In the frame step's look at the previous command buffer, which decides whether a new frame may
    /// replace its token.
    FrameStep,
}

impl PauseSite {
    fn code(self) -> i32 {
        match self {
            Self::Nowhere => 0,
            Self::IdleLook => 1,
            Self::FrameStep => 2,
        }
    }
}

/// The faults the tests inject, passed only through [`MetalPresenter::start_with_test_hooks`]. There is
/// no Cargo feature, environment variable or flag: nothing reachable from the `codekarl` binary can
/// enable a hook.
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a bag of independent test switches, each documented, is the point of this type"
)]
pub struct TestHooks {
    /// `start` fails before it spawns anything.
    pub fail_start: bool,
    /// The thread's setup finds no device.
    pub fail_setup: bool,
    /// Acquire and give back this many drawables without drawing.
    pub abandon_frames: u32,
    /// For this many frames, no command buffer after the drawable was acquired.
    pub nil_command_buffer_frames: u32,
    /// ... no render pass descriptor.
    pub nil_render_pass_frames: u32,
    /// ... no encoder.
    pub nil_encoder_frames: u32,
    /// ... an encoder started and ended, then the frame abandoned before it is committed.
    pub partial_encoding_frames: u32,
    /// For this many frames, the completion reports an `Error`.
    pub error_completion_frames: u32,
    /// Handlers that never set their tokens (lost callbacks).
    pub never_complete: NeverComplete,
    /// The handlers linger this long after setting their tokens (late callbacks).
    pub handler_delay: Duration,
    /// The completion handler waits this long before it sets its token (a late completion).
    pub completion_delay: Duration,
    /// While this reads non-zero (read on the render thread, with an atomic load), `nextDrawable` is not
    /// called (a blocked acquisition).
    pub hold_next_drawable: Option<Arc<AtomicI32>>,
    /// A gate on frame progress.
    pub gate: Option<Arc<Gate>>,
    /// A pause where the render thread has found nothing to do and is about to decide whether to wait.
    pub wait_pause: Option<Arc<WaitPause>>,
    /// Counts the frames committed.
    pub frames: Option<Arc<AtomicU64>>,
    /// Records the colour of every frame committed.
    pub clear_log: Option<Arc<Mutex<Vec<[f32; 3]>>>>,
    /// Records the generation of every frame committed, in commit order: entry `k` is the generation of the frame whose
    /// capture is `captured_frame_at(k)` (one frame in flight, every frame captured), so a capture can be bound to the frame
    /// that produced it.
    pub generation_log: Option<Arc<Mutex<Vec<u64>>>>,
    /// The observer, shared with the test.
    pub observer: Option<Arc<Observer>>,
    /// Set when the render thread starts.
    pub thread_started: Option<Arc<AtomicBool>>,
    /// The render thread panics after this many committed frames.
    pub panic_after_frames: Option<u64>,
    /// That panic raises the payload this function raises (a function so the hooks stay `Clone`), instead of
    /// its text: a payload whose `Drop` panics, for instance.
    pub forced_panic_payload: Option<fn() -> !>,
    /// The stop budget (default 1.5 s).
    pub stop_budget: Option<Duration>,
    /// The teardown thread is delayed this long inside its wait for the tokens, once (a late schedule).
    pub pause_in_wait: Option<Duration>,
    /// How the thread polls for command buffer errors while it runs.
    pub error_poll: ErrorPoll,
    /// Where, once, the thread is descheduled right after a native look read the command token as not
    /// done (the completion lands during the pause).
    pub pause_site: PauseSite,
    /// How long that pause lasts.
    pub pause_for: Duration,
    /// Receives what the `OutstandingFirst` look returned: -1 before it ran, then 0 or 1.
    pub outstanding_seen: Option<Arc<AtomicI32>>,
    /// The quad slot to start with (what [`MetalPresenter::start_2d`] passes).
    pub quads: Option<Arc<MetalQuadSlot>>,
    /// The quad pipeline cannot be created.
    pub fail_pipeline: bool,
    /// For this many frames, the quad texture cannot be allocated.
    pub fail_texture_allocation_frames: u32,
    /// For this many frames, the quad cannot be encoded.
    pub fail_quad_encode_frames: u32,
    /// The scene to start (what [`MetalPresenter::start_scene`] passes).
    pub scene: Option<SceneSetup>,
    /// For this many frames, the frame is abandoned after its ledger token was prepared and before the commit.
    pub abandon_after_prepare_frames: u32,
    /// For this many frames, `nextDrawable` is not called and the step is skipped.
    pub skipped_frames: u32,
    /// How each scene frame's ticket ended, in order.
    pub ticket_log: Option<Arc<Mutex<Vec<TicketEnding>>>>,
    /// The order of a proven end's releases: `"store_and_ledger"`, then `"presenter"`, then `"context"`.
    pub release_log: Option<Arc<Mutex<Vec<&'static str>>>>,
    /// A resident texture retired now is held by a stand-in token that arrives this much later, as if a
    /// command buffer still used it (zero: the token of the last command buffer, which has completed).
    pub retired_token_delay: Duration,
    /// The frame step waits, right after its first look at a retired texture, until that texture's pending
    /// token has arrived: a completion between the reap and the admission, placed deterministically.
    pub pause_after_reap_for_token: bool,
    /// Every frame is also rendered, by the same encoding, into an off-screen shared texture that the observer keeps once
    /// the frame's command buffer completes ([`Observer::captured_frame`]): the pixels of what the presenter drew.
    pub capture_frames: bool,
    /// A committed frame's completion waits (bounded at 30 s) until this reads non-zero: the test decides when it may arrive.
    pub frame_completion_gate: Option<Arc<AtomicI32>>,
    /// A bindless frame's draws take at most this many instances each (a test-injected small per-draw limit; the scene is
    /// split into runs, in painter order, with the same pixels). Unset: only the instance buffer's capacity limits a draw.
    pub draw_chunk_limit: Option<u32>,
    /// The native step waits **inside the acquisition** (between the marks t3 and t4, so it adds to the acquire interval only)
    /// while this reads non-zero, at most 30 s: a blocked `nextDrawable`.
    pub hold_in_acquire: Option<Arc<AtomicI32>>,
    /// The `n`-th instance a bindless frame draws has its slot forced out of the table: it is skipped and counted
    /// (`FrameCounters::slot_skipped`), never read by the GPU.
    pub force_out_of_range_slot_at: Option<usize>,
    /// Called with the six marks of every committed frame whose step finished (the frame's own timestamps, before they are recorded).
    pub frame_marks: Option<MarksProbe>,
    /// Replaces a consumed timing result before it is recorded: called with the kind (1 GPU: start and end; 2 presentation: the
    /// presented time and zero), the commit mark `t5` of the frame it belongs to and the two values, returns the pair to record. A
    /// test's inverted GPU pair, or a drawable that was never shown.
    pub timing_override: Option<TimingProbe>,
    /// The render thread waits (at most 30 s) before it adopts a requested measurement window while this reads non-zero: a delayed
    /// adoption, placed by the test.
    pub hold_metrics_adoption: Option<Arc<AtomicI32>>,
    /// The render thread waits (at most 30 s) after a frame step committed and before it records it while this reads non-zero: a
    /// frame committed before a window's end and not yet registered, placed by the test.
    pub hold_before_record: Option<Arc<AtomicI32>>,
    /// The thread keeps the presentation results it takes from the native ring (at most 16) instead of recording them while this
    /// reads non-zero, and records them, in order, at the first look after it reads zero: committed work whose presentation is
    /// still to come, placed by the test.
    pub hold_presentations: Option<Arc<AtomicI32>>,
    /// The native step waits (at most 30 s) after `commit` returned and before it reads the mark `t5` while this reads non-zero:
    /// a render thread descheduled right after the submission, so that `t5` is late and the drawable can already be shown.
    pub hold_after_commit: Option<Arc<AtomicI32>>,
    /// The thread waits (at most 30 s) between recording a frame step and moving its ready results into the windows while this
    /// reads non-zero.
    pub hold_before_drain: Option<Arc<AtomicI32>>,
}

/// A test's observer of a frame's marks (see [`TestHooks::frame_marks`]).
#[doc(hidden)]
#[derive(Clone)]
pub struct MarksProbe(pub Arc<dyn Fn(&FrameMarks) + Send + Sync>);

impl std::fmt::Debug for MarksProbe {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MarksProbe")
    }
}

/// A test's replacement of a timing result (see [`TestHooks::timing_override`]).
#[doc(hidden)]
#[derive(Clone)]
pub struct TimingProbe(pub Arc<TimingReplacement>);

/// What a [`TimingProbe`] calls: kind, commit mark, and the two values in; the two values to record out.
#[doc(hidden)]
pub type TimingReplacement = dyn Fn(u32, u64, u64, u64) -> (u64, u64) + Send + Sync;

impl std::fmt::Debug for TimingProbe {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TimingProbe")
    }
}

impl TestHooks {
    /// Takes one fault from `remaining` if any is left.
    fn consume(remaining: &mut u32) -> i32 {
        if *remaining > 0 {
            *remaining -= 1;
            1
        } else {
            0
        }
    }
}

/// The Metal presenter's handle, driven from the main thread. Its methods never wait beyond a brief lock
/// to copy a small value. Dropping it sets the stop request and never waits; it does not join and
/// releases nothing the thread owns.
pub struct MetalPresenter {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    ended: Option<CleanupResult>,
}

impl std::fmt::Debug for MetalPresenter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MetalPresenter")
    }
}

impl MetalPresenter {
    /// Spawns the render thread for `layer` and returns at once. The thread creates the device and queue,
    /// configures the layer and draws the first frame; a failure on the thread (no device, a failed
    /// configuration) comes through [`take_report`](Self::take_report) as `Failed`.
    ///
    /// `wake` is called on the render thread after each report and after its cleanup result. It must be
    /// short and must not block; a panic in it is caught and ignored and never changes a result already
    /// sent.
    ///
    /// # Errors
    ///
    /// `Err` means no thread was spawned and no GPU object exists (the retain on the layer is undone).
    ///
    /// # Safety
    ///
    /// `layer` must point to a live `CAMetalLayer` that stays alive at least until the presenter's end is
    /// proven ([`poll_end`](Self::poll_end) returned [`CleanupResult::Proven`]) or the process ends. The
    /// presenter takes its own retain on it and releases that retain only on a proven end.
    pub unsafe fn start(
        layer: *mut c_void,
        pixel_size: (u32, u32),
        clear: [f32; 3],
        wake: Wake,
    ) -> Result<Self, Error> {
        // SAFETY: the contract is this function's own.
        unsafe { Self::start_with_test_hooks(layer, pixel_size, clear, wake, TestHooks::default()) }
    }

    /// [`start`](Self::start) with a textured-quad slot: the render thread draws the slot's quad, if it holds
    /// one, over the clear in the same render pass, and reads the slot before its first frame.
    ///
    /// Everything else is as for `start`, which stays as it was and draws the clear only.
    ///
    /// # Errors
    ///
    /// As [`start`](Self::start).
    ///
    /// # Safety
    ///
    /// The same layer lifetime contract as [`start`](Self::start): `layer` must point to a live
    /// `CAMetalLayer` that stays alive at least until the presenter's end is proven or the process ends.
    pub unsafe fn start_2d(
        layer: *mut c_void,
        pixel_size: (u32, u32),
        clear: [f32; 3],
        wake: Wake,
        quads: Arc<MetalQuadSlot>,
    ) -> Result<Self, Error> {
        let hooks = TestHooks {
            quads: Some(quads),
            ..TestHooks::default()
        };
        // SAFETY: the contract is this function's own.
        unsafe { Self::start_with_test_hooks(layer, pixel_size, clear, wake, hooks) }
    }

    /// **The ordinary baseline** (hidden): [`start_scene_bindless`](Self::start_scene_bindless) without the bindless table,
    /// drawing CK-116's per-texture batches (one draw per run of adjacent equal textures). It exists only so that the
    /// scale acceptance (M5-10b) can measure the same content before and after, and for the tests of the ordinary path; the
    /// application's composition never names it, and the hook-isolation guard rejects any reference to it there.
    ///
    /// [`start_2d`](Self::start_2d) on a shared device context, with the texture store, ledger and driver
    /// of milestone 5, drawing a scene. The render thread takes its device and queue from `context` (the caller's, created
    /// by [`MetalContext::new`]), starts the upload store and the revision ledger on that context during setup, uploads the
    /// checker, runs `driver` on itself at every frame boundary and whenever texture work wakes it, ends every frame's
    /// ledger ticket by the frame's fate, and proves the presenter, the store, the checker and every submitted upload and
    /// frame in **one** teardown before anything device-dependent is released.
    ///
    /// Each frame is the latest [`MetalFrame`](crate::MetalFrame) of `frames`: the clear, the legacy quad (below
    /// everything), then the scene's quads in painter order, each resolved on the render thread through the frame's ticket
    /// (a key with nothing shown draws the checker) and drawn as one instanced draw per run of adjacent equal textures.
    /// **No frame is committed until the checker is resident**: the window opens and the presenter starts at once, and the
    /// checker's upload completion alone makes the waiting first frame run. `limits` size the one instance buffer
    /// (`max_quads` instances), whose measured allocation is charged to `binding_geometry_bytes`; a configuration that does
    /// not fit is a setup failure.
    ///
    /// # Errors
    ///
    /// As [`start`](Self::start).
    ///
    /// # Safety
    ///
    /// The same layer lifetime contract as [`start`](Self::start).
    #[expect(
        clippy::too_many_arguments,
        reason = "start_2d's arguments plus the context, the two limits and the driver the specification names"
    )]
    #[doc(hidden)]
    pub unsafe fn start_scene_ordinary_baseline<K, D>(
        layer: *mut c_void,
        pixel_size: (u32, u32),
        clear: [f32; 3],
        wake: Wake,
        frames: Arc<MetalFrameSlot<K>>,
        context: Arc<MetalContext>,
        texture_limits: TextureLimits,
        ledger_limits: LedgerLimits,
        limits: SceneLimits,
        driver: D,
    ) -> Result<Self, Error>
    where
        K: Copy + Eq + Hash + Send + 'static,
        D: TextureDriver<K> + 'static,
    {
        let hooks = TestHooks {
            scene: Some(SceneSetup::new(
                context,
                texture_limits,
                ledger_limits,
                limits,
                driver,
                frames,
                None,
                None,
            )),
            ..TestHooks::default()
        };
        // SAFETY: the contract is this function's own.
        unsafe { Self::start_with_test_hooks(layer, pixel_size, clear, wake, hooks) }
    }

    /// The scene presenter of the application: `bindless` names the table's slot count and the capacities it must cover
    /// (`BindlessLimits::validate`). The capability gate runs at setup and **a refusal is the presenter's `Failed` report with
    /// the gate's diagnostic** (the missing feature or limit and both values), with no fallback drawing; every scene frame is
    /// drawn through the table, one draw for the scene. There is no way to ask this entry for the ordinary per-texture
    /// path: that is the hidden `start_scene_ordinary_baseline`.
    ///
    /// # Errors
    ///
    /// As [`start`](Self::start).
    ///
    /// # Safety
    ///
    /// The same layer lifetime contract as [`start`](Self::start).
    #[expect(
        clippy::too_many_arguments,
        reason = "start_scene's arguments plus the bindless limits the specification names"
    )]
    pub unsafe fn start_scene_bindless<K, D>(
        layer: *mut c_void,
        pixel_size: (u32, u32),
        clear: [f32; 3],
        wake: Wake,
        frames: Arc<MetalFrameSlot<K>>,
        context: Arc<MetalContext>,
        texture_limits: TextureLimits,
        ledger_limits: LedgerLimits,
        bindless: BindlessLimits,
        limits: SceneLimits,
        driver: D,
    ) -> Result<Self, Error>
    where
        K: Copy + Eq + Hash + Send + 'static,
        D: TextureDriver<K> + 'static,
    {
        let hooks = TestHooks {
            scene: Some(SceneSetup::new_with_bindless_limits(
                context,
                texture_limits,
                ledger_limits,
                Some(bindless),
                limits,
                driver,
                frames,
                None,
                None,
            )),
            ..TestHooks::default()
        };
        // SAFETY: the contract is this function's own.
        unsafe { Self::start_with_test_hooks(layer, pixel_size, clear, wake, hooks) }
    }

    /// A handle that marks "texture work pending" and wakes the render thread, for the driver's owner to give its
    /// own producers (the resource service's upload and retirement wakes). It never blocks and holds the presenter
    /// weakly: it does nothing after the presenter ended.
    #[must_use]
    pub fn texture_wake(&self) -> Wake {
        texture_marker(&self.shared)
    }

    /// [`start`](Self::start) with faults injected by the tests. Not part of the interface.
    ///
    /// # Errors
    ///
    /// As [`start`](Self::start), and a forced `fail_start`.
    ///
    /// # Safety
    ///
    /// The same contract as [`start`](Self::start): `layer` must point to a live `CAMetalLayer` that stays
    /// alive until the end is proven or the process ends. `doc(hidden)` does not make the pointer safe.
    #[doc(hidden)]
    pub unsafe fn start_with_test_hooks(
        layer: *mut c_void,
        pixel_size: (u32, u32),
        clear: [f32; 3],
        wake: Wake,
        hooks: TestHooks,
    ) -> Result<Self, Error> {
        if layer.is_null() {
            return Err(Error("the layer pointer is null".to_owned()));
        }
        let observer = hooks
            .observer
            .clone()
            .unwrap_or_else(|| Arc::new(Observer::new()));
        // SAFETY: the caller promises `layer` is a live `CAMetalLayer`; this takes one retain on it.
        let retained = unsafe { ck_pr_layer_retain(layer, observer.raw.as_ptr()) };
        if hooks.fail_start {
            // SAFETY: undoes the retain just taken; nothing else holds it.
            unsafe { ck_pr_layer_release(retained, observer.raw.as_ptr()) };
            return Err(Error(
                "a forced failure before the thread was spawned".to_owned(),
            ));
        }
        // SAFETY: `retained` is the retain just taken, whose ownership passes to the core; the observer is
        // retained by the core and outlives it.
        let core = unsafe { ck_pr_core_new(retained, observer.raw.as_ptr()) };
        let Some(core) = NonNull::new(core).map(Core) else {
            // SAFETY: no core took the retain, so it is still ours to undo.
            unsafe { ck_pr_layer_release(retained, observer.raw.as_ptr()) };
            return Err(Error("the presenter core could not be created".to_owned()));
        };
        let shared = Arc::new(Shared {
            slot: Mutex::new(Slot {
                clear,
                size: pixel_size,
                dirty: true,
                texture_work: false,
                stop: false,
                stop_deadline: None,
                metrics_check: false,
            }),
            signal: Condvar::new(),
            reports: Mutex::new(VecDeque::new()),
            result: Mutex::new(None),
            wake,
            stop_budget: hooks.stop_budget.unwrap_or(STOP_BUDGET),
            drawn_generation: AtomicU64::new(0),
            drawn_any: AtomicBool::new(false),
            scene_stats: Mutex::new(SceneStats::default()),
            metrics: Mutex::new(Recorder::default()),
            metrics_changed: Condvar::new(),
        });
        if let Some(setup) = &hooks.scene {
            // A frame published to the scene's slot wants a frame; the wake holds the presenter's state weakly. A frame
            // published before this point is read by the first frame, which the slot's initial `dirty` guarantees.
            let weak = Arc::downgrade(&shared);
            setup.0.register_wake(Arc::new(move || {
                if let Some(shared) = weak.upgrade() {
                    lock(&shared.slot).dirty = true;
                    shared.signal.notify_all();
                }
            }));
        }
        if let Some(quads) = &hooks.quads {
            // A change in the slot wants a frame. The wake holds the presenter's state weakly, so a slot that
            // outlives the presenter keeps nothing of it alive. A value published before this point is read by
            // the first frame, which the slot's initial `dirty` guarantees.
            let weak = Arc::downgrade(&shared);
            quads.register_wake(Arc::new(move || {
                if let Some(shared) = weak.upgrade() {
                    lock(&shared.slot).dirty = true;
                    shared.signal.notify_all();
                }
            }));
        }
        let thread_shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("ck-metal-render".to_owned())
            .spawn(move || render_main(&thread_shared, core, hooks));
        match spawned {
            Ok(thread) => Ok(Self {
                shared,
                thread: Some(thread),
                ended: None,
            }),
            Err(error) => {
                // SAFETY: no thread exists, so nothing but this function holds the core: releasing it
                // undoes the layer retain.
                unsafe { ck_pr_release_all(core.0.as_ptr()) };
                Err(Error(format!(
                    "the render thread could not be spawned: {error}"
                )))
            }
        }
    }

    /// Sets the clear colour (components 0..=1). A different colour gives one new frame.
    pub fn set_clear(&self, rgb: [f32; 3]) {
        let mut slot = lock(&self.shared.slot);
        // Bit-for-bit: a colour set again to the very same value is no change.
        if slot.clear.map(f32::to_bits) != rgb.map(f32::to_bits) {
            slot.clear = rgb;
            slot.dirty = true;
            drop(slot);
            self.shared.signal.notify_all();
        }
    }

    /// Sets the drawable's size in pixels (a zero dimension is ignored). A different size gives one new
    /// frame, with `drawableSize` set on the render thread first.
    pub fn set_pixel_size(&self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let mut slot = lock(&self.shared.slot);
        if slot.size != (width, height) {
            slot.size = (width, height);
            slot.dirty = true;
            drop(slot);
            self.shared.signal.notify_all();
        }
    }

    /// Asks for a frame.
    pub fn request_redraw(&self) {
        lock(&self.shared.slot).dirty = true;
        self.shared.signal.notify_all();
    }

    /// The scene presenter's counters and charges (all zero for a presenter without a scene): per committed frame the latest
    /// value and the peak, the instance buffer's measured allocation against `binding_geometry_bytes`, and the checker's
    /// baseline bytes. A single latest value, read without waiting.
    #[must_use]
    pub fn stats(&self) -> SceneStats {
        *lock(&self.shared.scene_stats)
    }

    /// Requests the common measurement window `window` (`[start, end)` on `CLOCK_UPTIME_RAW` nanoseconds, the same definition sent to
    /// every owner). The render thread adopts it at its next boundary (before its next frame step, or when this request wakes it) and
    /// publishes its adoption time ([`metrics_adopted`](Self::metrics_adopted)); a sample enters the window only by its **own
    /// timestamp** in `[start, end)`, whatever the adoption times.
    ///
    /// # Errors
    ///
    /// A request that overlaps the current window, whose id is not new or whose end is not after its start is refused and changes
    /// nothing.
    pub fn request_metrics(&self, window: MetricsWindow) -> Result<(), RequestError> {
        lock(&self.shared.metrics).windows.request(window)?;
        lock(&self.shared.slot).metrics_check = true;
        self.shared.signal.notify_all();
        Ok(())
    }

    /// Sets the end of window `id`, once (the application's run end).
    ///
    /// # Errors
    ///
    /// The window is unknown, already ended, or `end` is not after its start.
    pub fn end_metrics(&self, id: u32, end: u64) -> Result<(), EndError> {
        lock(&self.shared.metrics).windows.end(id, end)?;
        self.shared.metrics_changed.notify_all();
        Ok(())
    }

    /// When the render thread adopted window `id`, once it has.
    #[must_use]
    pub fn metrics_adopted(&self, id: u32) -> Option<u64> {
        lock(&self.shared.metrics).windows.adopted(id)
    }

    /// Holds the presenter's admission lock, so that an end can be set while the render thread admits nothing (see
    /// [`MetricsEndGuard`]). Hold it briefly; the render thread's recording waits for it.
    #[must_use]
    pub fn lock_metrics_end(&self) -> MetricsEndGuard<'_> {
        MetricsEndGuard {
            metrics: lock(&self.shared.metrics),
            changed: &self.shared.metrics_changed,
        }
    }

    /// The whole run's figures so far, every sample whatever its window.
    #[must_use]
    pub fn metrics_totals(&self) -> FrameStats {
        lock(&self.shared.metrics).totals.clone()
    }

    /// A copy of window `id` as it stands, without waiting. `None` for a window the presenter does not keep (it keeps the current and
    /// the last ended one).
    #[must_use]
    pub fn metrics_snapshot(&self, id: u32) -> Option<MetricsReading> {
        let metrics = lock(&self.shared.metrics);
        let window = metrics.windows.get(id)?.clone();
        let closing_pending = metrics.closing_pending(&window.window);
        Some(MetricsReading::of(
            window,
            metrics.timing_lost,
            false,
            metrics.evidence_lost(),
            closing_pending,
        ))
    }

    /// Window `id` once this owner has passed its end and the window's **outstanding** work (frames committed in it whose GPU time or
    /// presentation is not yet consumed) is zero, waiting at most `within` for that. A reading that is not `complete`
    /// (`MetricsReading::complete`) says what was still outstanding at the bound.
    #[must_use]
    pub fn read_metrics(&self, id: u32, within: Duration) -> Option<MetricsReading> {
        let deadline = Instant::now() + within;
        let mut metrics = lock(&self.shared.metrics);
        loop {
            let (passed, outstanding, closing_pending) = {
                let tracked = metrics.windows.get(id)?;
                (
                    // Past the end on the clock AND this owner has recorded every step that began before it AND consumed the
                    // presentation of every frame submitted before it: a frame committed before the end may still be between its
                    // commit and its registration, or shown inside the window with its result still to come.
                    tracked
                        .window
                        .end
                        .is_some_and(|end| now_ns() >= end && metrics.accounted_through(end)),
                    tracked.stats.outstanding,
                    // And the window's closing interval is known.
                    metrics.closing_pending(&tracked.window),
                )
            };
            let complete = passed && outstanding == 0 && !closing_pending;
            let now = Instant::now();
            if complete || now >= deadline {
                let window = metrics.windows.get(id)?.clone();
                return Some(MetricsReading::of(
                    window,
                    metrics.timing_lost,
                    complete,
                    metrics.evidence_lost(),
                    closing_pending,
                ));
            }
            // The end is a point on the clock and nothing signals it passing, so the wait is in slices.
            let slice = (deadline - now).min(METRICS_WAIT_SLICE);
            metrics = self
                .shared
                .metrics_changed
                .wait_timeout(metrics, slice)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The generation of the last frame committed: that of the quad it drew, or of the change that left it
    /// without one. `None` until a frame was committed (and always for a presenter started without a slot).
    /// A single latest value, read without waiting; it is not a report per frame.
    #[must_use]
    pub fn drawn_generation(&self) -> Option<u64> {
        // The render thread writes the generation first and the flag second, so a set flag has a generation.
        self.shared
            .drawn_any
            .load(Ordering::Acquire)
            .then(|| self.shared.drawn_generation.load(Ordering::Acquire))
    }

    /// The next report, if any.
    #[must_use]
    pub fn take_report(&self) -> Option<PresenterReport> {
        lock(&self.shared.reports).pop_front()
    }

    /// Asks the thread to stop. The first call records the one **absolute stop deadline** (now plus 1.5 s)
    /// that the thread's teardown, and a `nextDrawable` that was blocking when the stop arrived, share; a
    /// repeated call does not move it.
    pub fn request_stop(&self) {
        let mut slot = lock(&self.shared.slot);
        if slot.stop_deadline.is_none() {
            slot.stop_deadline = Some(Instant::now() + self.shared.stop_budget);
        }
        slot.stop = true;
        drop(slot);
        self.shared.signal.notify_all();
    }

    /// Waits at most `within` for the render thread to end. `Some` only once the thread has been observed
    /// finished (`is_finished`, then joined); the result is [`CleanupResult::Proven`] only if the thread
    /// sent an explicit successful result before ending.
    pub fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        if let Some(result) = &self.ended {
            return Some(result.clone());
        }
        let give_up = Instant::now().checked_add(within);
        loop {
            if self.thread.as_ref().is_none_or(JoinHandle::is_finished) {
                if let Some(thread) = self.thread.take() {
                    unwind::join(thread);
                }
                let result =
                    lock(&self.shared.result)
                        .take()
                        .unwrap_or_else(|| CleanupResult::Unproven {
                            reason: "the render thread ended without a cleanup result".to_owned(),
                        });
                self.ended = Some(result.clone());
                return Some(result);
            }
            let left = give_up.map_or(POLL, |limit| {
                limit.saturating_duration_since(Instant::now())
            });
            if left.is_zero() {
                return None;
            }
            thread::sleep(left.min(Duration::from_millis(1)));
        }
    }
}

impl Drop for MetalPresenter {
    fn drop(&mut self) {
        // The same stop operation, deadline and all; it never waits, never joins and releases nothing.
        self.request_stop();
    }
}

/// The render thread: runs the presenter and publishes its cleanup result. A panic is caught here and
/// gives `Unproven` with every holder kept (the core is a raw pointer with no `Drop`).
fn render_main(shared: &Arc<Shared>, core: Core, hooks: TestHooks) {
    if let Some(started) = &hooks.thread_started {
        started.store(true, Ordering::SeqCst);
    }
    let quads = hooks.quads.clone();
    let scene = hooks.scene.clone();
    let outcome = catch_unwind(AssertUnwindSafe(|| run(shared, core, hooks)));
    finish(shared, quads.as_deref(), scene.as_ref(), outcome);
}

/// What the render thread does once `run` is over, however it ended: a panic (its payload is disposed of,
/// never dropped implicitly, so that no payload can end the thread before this is done) is `Unproven` with
/// every holder kept; the quad slot stops waking the presenter; the result is stored; the wake is called.
fn finish(
    shared: &Shared,
    quads: Option<&MetalQuadSlot>,
    scene: Option<&SceneSetup>,
    outcome: Result<CleanupResult, unwind::Payload>,
) {
    let result = match outcome {
        Ok(result) => result,
        Err(payload) => {
            unwind::dispose(payload);
            CleanupResult::Unproven {
                reason: "the render thread panicked; everything it owned was kept".to_owned(),
            }
        }
    };
    if let Some(quads) = quads {
        // The presenter is over: the slot stops waking it.
        quads.clear_wake();
    }
    if let Some(scene) = scene {
        scene.0.clear_wake();
    }
    *lock(&shared.result) = Some(result);
    shared.wake();
}

/// What the render thread keeps while it runs.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent latches of the render thread's progress, each documented"
)]
struct Loop<'a> {
    shared: &'a Shared,
    core: Core,
    hooks: TestHooks,
    first_reported: bool,
    failed: bool,
    committed: u64,
    /// The `OutstandingFirst` order was applied once.
    adverse_done: bool,
    /// The colour of the frame step in progress.
    last_clear: Option<[f32; 3]>,
    /// The quad slot, if the presenter was started with one.
    quads: Option<Arc<MetalQuadSlot>>,
    /// The image of the resident quad texture, by identity. Weak: it keeps the source's allocation (so its
    /// address is not reused for another image) but not its pixels.
    /// together with the layout it was uploaded as (width and height): the same source described with a
    /// different layout is a different texture.
    resident: Option<(Weak<dyn PixelSource>, u32, u32)>,
    /// The generation to record when the frame in progress is committed.
    pending_generation: Option<u64>,
    /// The texture scene (store, ledger, driver), if the presenter was started with one.
    scene: Option<Box<dyn Scene>>,
    /// The scene's instance buffer, if the presenter was started with a scene.
    instances: Option<InstanceBuffer>,
    /// Whether the checker is resident: always true without a scene; with one, **no frame is taken before it is**.
    checker_resident: bool,
    /// The frame completion gate of a test, as a pointer a late handler may read: one share of its `Arc` is deliberately
    /// leaked (once), so the pointer stays valid however long after the presenter a handler runs. Null in production.
    frame_gate: *const AtomicI32,
    /// The device context, never dropped implicitly (see `run`).
    context: Option<ManuallyDrop<Arc<MetalContext>>>,
    /// Whether the frame boundary pass already ran for the frame now being retried (a busy, skipped or abandoned
    /// step is attempted again): a boundary is once per frame built, not once per attempt.
    boundary: FrameBoundary,
    /// The frame-timing marks of the step in progress (`t0` .. `t5`, `CLOCK_UPTIME_RAW` nanoseconds) and its scene counters.
    marks: FrameMarks,
    /// The clock read just before the step's command buffer was committed (zero until one was): a drawable cannot be shown before it.
    submitted: u64,
    step_counters: Option<FrameCounters>,
    /// Presentation results a test asked to be kept back (`TestHooks::hold_presentations`); empty in every other run.
    held_presentations: Vec<NativeTiming>,
}

/// Where the frame being built stands with its boundary pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameBoundary {
    /// The next attempt runs the boundary pass first.
    Due,
    /// The pass ran; a retry of the step goes straight to the step.
    Passed,
}

/// What the render thread does next.
enum Work {
    /// A frame, with the latest colour and size.
    Frame([f32; 3], (u32, u32)),
    /// An ordinary texture pass.
    Texture,
}

/// Notes one release step for the tests.
fn released(log: Option<&Arc<Mutex<Vec<&'static str>>>>, step: &'static str) {
    if let Some(log) = log {
        lock(log).push(step);
    }
}

fn text(buffer: &[u8]) -> String {
    message(buffer).to_string()
}

#[expect(
    clippy::too_many_lines,
    reason = "the presenter thread's setup, its error paths and the loop's construction read best together"
)]
fn run(shared: &Arc<Shared>, core: Core, hooks: TestHooks) -> CleanupResult {
    let (width, height) = lock(&shared.slot).size;
    let mut error = [0_u8; 512];
    // The device context: the caller's for a scene presenter, else one made here, where the device and queue used to
    // be made (a missing device is the same `Failed` report from this thread, not a `start` error). It is never
    // dropped implicitly - a thread that unwinds, or an unproven end, keeps it - and is released last, on a proven
    // end only.
    let made = if hooks.fail_setup {
        Err(Error("No Metal device is available".to_owned()))
    } else if let Some(setup) = &hooks.scene {
        Ok(setup.0.context())
    } else {
        MetalContext::new()
    };
    let (context, mut failure) = match made {
        Ok(context) => (Some(ManuallyDrop::new(context)), None),
        Err(error) => (None, Some(error.to_string())),
    };
    if let Some(context) = context.as_ref() {
        // SAFETY: `core` is live and used on this thread only; the context is live and outlives the core; `error`
        // is valid for `error.len()` bytes.
        let setup = unsafe {
            ck_pr_setup(
                core.0.as_ptr(),
                context.raw(),
                width,
                height,
                error.as_mut_ptr().cast::<c_char>(),
                error.len(),
            )
        };
        failure = (setup != 0).then(|| text(&error));
    }
    // The store and ledger start on the context during setup, once the device and layer are set up.
    let mut scene: Option<Box<dyn Scene>> = None;
    if failure.is_none()
        && let Some(setup) = &hooks.scene
    {
        match setup
            .0
            .start(wakes_of(&texture_marker(shared)), hooks.ticket_log.clone())
        {
            Ok(started) => scene = Some(started),
            Err(started) => failure = Some(started.to_string()),
        }
    }
    if failure.is_none() && (hooks.quads.is_some() || scene.is_some()) {
        // SAFETY: `core` is live and used on this thread only; the library bytes and `error` are valid for the
        // call.
        let quad_setup = unsafe {
            ck_pr_quad_setup(
                core.0.as_ptr(),
                QUAD_LIBRARY.as_ptr(),
                QUAD_LIBRARY.len(),
                i32::from(hooks.fail_pipeline),
                error.as_mut_ptr().cast::<c_char>(),
                error.len(),
            )
        };
        failure = (quad_setup != 0).then(|| text(&error));
    }
    // The scene's one instance buffer, sized for `max_quads` instances and charged to `binding_geometry_bytes` at its
    // measured allocation. A configuration that does not fit is refused here, before any frame.
    let mut instances = None;
    if failure.is_none()
        && let Some(setup) = &hooks.scene
    {
        match instance_buffer(shared, core, setup.0.limits()) {
            Ok(buffer) => instances = Some(buffer),
            Err(reason) => failure = Some(reason),
        }
    }
    if let Some(message) = failure {
        shared.report(PresenterReport::Failed { message });
        return match scene {
            // A failure before the store started: nothing concurrent exists, nothing has been drawn, so nothing is
            // outstanding: release what setup made and end at once.
            None => {
                // SAFETY: nothing else uses the core; no token can be outstanding.
                unsafe { ck_pr_release_all(core.0.as_ptr()) };
                if let Some(context) = context {
                    drop(ManuallyDrop::into_inner(context));
                }
                shared.wake();
                CleanupResult::Proven
            }
            // A failure after the store started: its worker may be running, so the premise is gone - prove first.
            Some(scene) => roll_back(shared, core, context, scene, hooks.release_log.as_ref()),
        };
    }
    arm_check_pause(core, &hooks);
    let quads = hooks.quads.clone();
    let frame_gate = leak_frame_gate(&hooks);
    let mut state = Loop {
        shared,
        core,
        hooks,
        first_reported: false,
        failed: false,
        committed: 0,
        adverse_done: false,
        last_clear: None,
        quads,
        resident: None,
        pending_generation: None,
        checker_resident: scene.is_none(),
        frame_gate,
        scene,
        instances,
        context,
        boundary: FrameBoundary::Due,
        marks: FrameMarks::default(),
        step_counters: None,
        submitted: 0,
        held_presentations: Vec::new(),
    };
    state.frames();
    state.teardown()
}

/// The frame completion gate of a test as a pointer a late handler may read: one share of its `Arc` is deliberately leaked
/// (once per presenter), so the pointer stays valid however long after the presenter a handler runs. Null in production.
fn leak_frame_gate(hooks: &TestHooks) -> *const AtomicI32 {
    hooks
        .frame_completion_gate
        .as_ref()
        .map_or(std::ptr::null(), |gate| Arc::into_raw(Arc::clone(gate)))
}

/// Arms the test hook's one-shot pause in the native core, if the hooks ask for one.
fn arm_check_pause(core: Core, hooks: &TestHooks) {
    if hooks.pause_site != PauseSite::Nowhere {
        let pause = i32::try_from(hooks.pause_for.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: the core is live and used on this thread only; it arms a one-shot test pause.
        unsafe { ck_pr_set_check_pause(core.0.as_ptr(), hooks.pause_site.code(), pause) };
    }
}

/// The instance buffer a scene presenter writes: where it lives and how many instances it holds.
#[derive(Clone, Copy)]
struct InstanceBuffer {
    base: NonNull<InstanceRecord>,
    capacity: usize,
}

/// Creates the instance buffer and charges it. The size is computed with checked arithmetic and must fit
/// `binding_geometry_bytes` both as asked for and as measured (`allocatedSize`, alignment included); a refusal names both
/// numbers.
fn instance_buffer(
    shared: &Shared,
    core: Core,
    limits: SceneLimits,
) -> Result<InstanceBuffer, String> {
    let wanted = limits
        .max_quads
        .checked_mul(INSTANCE_STRIDE)
        .and_then(|bytes| u64::try_from(bytes).ok())
        .filter(|bytes| *bytes != 0)
        .ok_or_else(|| {
            format!(
                "max_quads {} instances of {INSTANCE_STRIDE} bytes is not a valid instance buffer size",
                limits.max_quads
            )
        })?;
    if wanted > limits.binding_geometry_bytes {
        return Err(format!(
            "the instance buffer for max_quads {} needs {wanted} bytes, over the binding_geometry_bytes budget of {}",
            limits.max_quads, limits.binding_geometry_bytes
        ));
    }
    let mut allocated = 0_u64;
    let mut error = [0_u8; 512];
    // SAFETY: `core` is live and used on this thread only; `allocated` and `error` are valid for the call.
    let created = unsafe {
        ck_pr_instance_buffer_setup(
            core.0.as_ptr(),
            wanted,
            &raw mut allocated,
            error.as_mut_ptr().cast::<c_char>(),
            error.len(),
        )
    };
    if created != 0 {
        return Err(text(&error));
    }
    {
        let mut stats = lock(&shared.scene_stats);
        stats.instance_buffer_bytes = wanted;
        stats.instance_buffer_allocated = allocated;
        stats.binding_geometry_bytes = limits.binding_geometry_bytes;
    }
    if allocated > limits.binding_geometry_bytes {
        return Err(format!(
            "the instance buffer for max_quads {} measured {allocated} bytes (allocatedSize), over the binding_geometry_bytes budget of {}",
            limits.max_quads, limits.binding_geometry_bytes
        ));
    }
    // SAFETY: `core` is live; the buffer was just created and is kept until the explicit release.
    let contents = unsafe { ck_pr_instance_buffer_contents(core.0.as_ptr()) };
    NonNull::new(contents.cast::<InstanceRecord>())
        .map(|base| InstanceBuffer {
            base,
            capacity: limits.max_quads,
        })
        .ok_or_else(|| "the instance buffer has no CPU-visible contents".to_owned())
}

/// Copies the upload store's cumulative counters and byte high-water marks into the scene stats. It only **reads** the ledger's
/// statistics (no submit, poll, reap or release), so it is allowed wherever the presenter may look at the store, the `Ending`
/// pass and the proof loop included. The peaks never decrease, so publishing again only adds what was charged since.
///
/// The published figures are the store's as of the presenter's **last look**: every pass with a scene (a failing one and the
/// `Ending` one too), once more right before the store is released, and once more before it is forgotten when the end is
/// `Unproven`. A worker the presenter could not prove finished (the `Unproven` end keeps it running) may still charge after
/// that last look, so for an unproven end the figures are a lower bound, not a completed-run maximum.
fn publish_upload_counters(shared: &Shared, scene: &dyn Scene) {
    let uploads = scene.uploads();
    let mut stats = lock(&shared.scene_stats);
    stats.textures_ready = uploads.adopted;
    stats.first_ready_ns = uploads.first_adopted_ns;
    stats.last_ready_ns = uploads.last_adopted_ns;
    stats.textures_created = uploads.textures_created;
    stats.uploads_submitted = uploads.uploads_submitted;
    stats.uploads_completed = uploads.uploads_completed;
    stats.upload_texture_bytes_peak = uploads.texture_bytes_peak;
    stats.upload_staging_bytes_peak = uploads.staging_bytes_peak;
}

/// A setup that failed after the store started (r3, F1): the presenter has reported `Failed`; it asks the store to
/// stop and proves the store and ledger against a deadline formed now (now plus the stop budget). Only with a
/// complete proof does it release everything and end `Proven`; otherwise it retains the whole graph (the store, the
/// ledger, the presenter's holders, the context, the layer retain) and ends `Unproven`.
fn roll_back(
    shared: &Shared,
    core: Core,
    context: Option<ManuallyDrop<Arc<MetalContext>>>,
    mut scene: Box<dyn Scene>,
    log: Option<&Arc<Mutex<Vec<&'static str>>>>,
) -> CleanupResult {
    let deadline = Instant::now() + shared.stop_budget;
    loop {
        match scene.prove() {
            None => {
                // The last look at the store's counters, before it is released (or, on a refused release, forgotten).
                publish_upload_counters(shared, &*scene);
                if let Err(reason) = scene.release() {
                    std::mem::forget(scene);
                    return CleanupResult::Unproven {
                        reason: format!(
                            "the setup failed and the texture store could not be released ({reason}); everything was kept"
                        ),
                    };
                }
                released(log, "store_and_ledger");
                // SAFETY: the store and ledger are proven finished and released; nothing was drawn, so no token of
                // the core is outstanding; nothing else uses the core.
                unsafe { ck_pr_release_all(core.0.as_ptr()) };
                released(log, "presenter");
                drop(scene);
                if let Some(context) = context {
                    released(log, "context");
                    drop(ManuallyDrop::into_inner(context));
                }
                shared.wake();
                return CleanupResult::Proven;
            }
            Some(outstanding) => {
                if Instant::now() >= deadline {
                    // Retained: the store, the ledger, the presenter's holders, the context and the layer retain.
                    publish_upload_counters(shared, &*scene);
                    std::mem::forget(scene);
                    return CleanupResult::Unproven {
                        reason: format!(
                            "the setup failed after the texture store started and its end was not proven by the deadline ({outstanding}); the device, queue, layer, store and ledger were kept"
                        ),
                    };
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

fn observer_presented_short(state: &Loop<'_>) -> bool {
    state.hooks.observer.as_ref().is_some_and(|observer| {
        observer.presented_handlers_run() < i64::try_from(state.committed).unwrap_or(0)
    })
}

impl Loop<'_> {
    /// Waits for work or the stop. `Some` is a frame to draw or an ordinary texture pass; `None` is the stop.
    ///
    /// A wanted frame goes first and its boundary pass serves any texture work pending, so texture work never
    /// delays a wanted frame beyond one pass; texture work alone runs an ordinary pass and goes back to waiting.
    fn next_work(&mut self) -> Option<Work> {
        loop {
            // The checker's upload may have completed (its completion woke the thread): no frame is taken before it is
            // resident, and its completion alone makes the waiting first frame run.
            self.poll_checker();
            // A requested measurement window is adopted at this boundary.
            if lock(&self.shared.slot).metrics_check {
                self.metrics_boundary();
            }
            // The initial gate: a frame held back for a compatibility image that has not appeared is not attempted again
            // until the facts change (the gating key has a revision to adopt or draw, or the frame published since has
            // another compatibility key). Read from the ledger and the frame slot each time, never from a remembered edge.
            let gate_open = self.gate_open();
            // 1. Wait until a frame or texture work is wanted, or the stop. Between frames the thread waits on the
            //    slot's signal, with a short timeout only while a completion or token is outstanding.
            let (wanted, texture) = {
                let slot = lock(&self.shared.slot);
                if slot.stop {
                    return None;
                }
                (
                    slot.dirty && !self.failed && self.checker_resident && gate_open,
                    slot.texture_work && !self.failed && self.scene.is_some(),
                )
            };
            if !wanted && !texture {
                // SAFETY: the core is live and used on this thread only.
                let outstanding = unsafe { ck_pr_outstanding(self.core.0.as_ptr()) } != 0;
                // The look at the tokens consumes the results of those that arrived (it moves them into the ring), and when it says
                // nothing is outstanding the thread sleeps without a timer: they are moved into the windows now, not at some later
                // wake, or a window would stay open on results that were already here.
                self.drain_timing();
                if let Some(pause) = &self.hooks.wait_pause {
                    pause.pause(|| !lock(&self.shared.slot).stop);
                }
                {
                    let slot = lock(&self.shared.slot);
                    // The gate is read again **under the mutex the wait releases**: a publication that landed since the read
                    // above has set `dirty` and notified under this mutex's protection of the flag, and its frame swap came
                    // before that, so either this read sees the new frame or the notify comes after the thread waits.
                    let gate_open = self.gate_open();
                    let texture_waits = slot.texture_work && !self.failed && self.scene.is_some();
                    if !slot.stop
                        && (!slot.dirty || self.failed || !self.checker_resident || !gate_open)
                        && !texture_waits
                        && !slot.metrics_check
                    {
                        let _released = if outstanding {
                            self.shared
                                .signal
                                .wait_timeout(slot, POLL)
                                .unwrap_or_else(PoisonError::into_inner)
                                .0
                        } else {
                            self.shared
                                .signal
                                .wait(slot)
                                .unwrap_or_else(PoisonError::into_inner)
                        };
                    }
                }
                if !self.failed {
                    self.poll_while_running();
                }
                continue;
            }
            if !wanted {
                // Texture work only: read and cleared under the slot's mutex, with the stop.
                let mut slot = lock(&self.shared.slot);
                if slot.stop {
                    return None;
                }
                if slot.texture_work && !self.failed {
                    slot.texture_work = false;
                    return Some(Work::Texture);
                }
                continue;
            }
            // 2. The gate, if a test holds frame progress.
            if let Some(gate) = &self.hooks.gate
                && !gate.take(|| !lock(&self.shared.slot).stop)
            {
                return None;
            }
            // 3. Take the latest values (the last colour wins) and clear the wants: the frame's own boundary pass
            //    serves the texture work pending now, and a wake after this point sets the flag again.
            let mut slot = lock(&self.shared.slot);
            if slot.stop {
                return None;
            }
            if slot.dirty {
                slot.dirty = false;
                if self.boundary == FrameBoundary::Due {
                    slot.texture_work = false;
                }
                return Some(Work::Frame(slot.clear, slot.size));
            }
        }
    }

    /// Whether the initial gate lets a frame be attempted (always, without a scene).
    fn gate_open(&mut self) -> bool {
        self.scene
            .as_mut()
            .is_none_or(|scene| scene.frame_gate() == FrameGate::Open)
    }

    /// One non-blocking look at the checker's upload. Resident opens the gate to frames; a failure is the presenter's one
    /// `Failed` report, and nothing is drawn.
    fn poll_checker(&mut self) {
        if self.checker_resident || self.failed {
            return;
        }
        let Some(scene) = self.scene.as_mut() else {
            return;
        };
        match scene.checker() {
            CheckerProgress::Pending => {}
            CheckerProgress::Resident => {
                self.checker_resident = true;
                let Baseline {
                    texture_bytes,
                    staging_bytes,
                    residency_set_allocation,
                    bindless_table_allocation,
                    bindless_slots,
                } = scene.baseline();
                let mut stats = lock(&self.shared.scene_stats);
                stats.checker_resident = true;
                stats.checker_texture_bytes = texture_bytes;
                stats.checker_staging_bytes = staging_bytes;
                stats.residency_set_bytes = residency_set_allocation;
                stats.bindless_table_bytes = bindless_table_allocation;
                stats.bindless_slots = bindless_slots;
            }
            CheckerProgress::Failed(reason) => self.fail(reason),
        }
    }

    /// Committed revisions waiting for a frame boundary make a frame wanted, so adoption never waits for an
    /// unrelated redraw (and never takes more than the per-boundary limit's worth of frames).
    fn want_frame_for_committed(&mut self) {
        if self.failed {
            return;
        }
        if self
            .scene
            .as_ref()
            .is_some_and(|scene| scene.has_committed())
        {
            lock(&self.shared.slot).dirty = true;
        }
    }

    /// One pass over the texture scene at `boundary`. The order is `pump`, then the presenter's own consumption of a
    /// `GpuFailure` (a `Failed` report under the existing rules, queued and woken **before** any driver callback
    /// runs, so a driver that panics cannot hide a failure already observed), then the driver with the remaining
    /// logical events. A pass that reports runnable work left marks texture work pending again before the thread next
    /// evaluates its wait, and committed revisions waiting for a frame boundary make a frame wanted, so adoption never
    /// waits for an unrelated redraw.
    ///
    /// The `Ending` pass belongs to the teardown, whose proof is still open: it only *observes* (no submit, poll, reap
    /// or release), so nothing device-dependent is dropped before the combined proof; it hands the driver no events.
    /// Publishes the upload store's cumulative counters and byte high-water marks into the scene stats (a presenter without a
    /// scene publishes nothing); see [`publish_upload_counters`].
    fn publish_uploads(&self) {
        if let Some(scene) = self.scene.as_ref() {
            publish_upload_counters(self.shared, &**scene);
        }
    }

    fn texture_pass(&mut self, boundary: Boundary) {
        let Some(scene) = self.scene.as_mut() else {
            return;
        };
        let (failure, pumped_more) = if boundary == Boundary::Ending {
            (scene.observe(), false)
        } else {
            let pumped = scene.pump();
            (pumped.failure, pumped.more)
        };
        if let Some(message) = failure {
            self.fail(message);
        }
        let Some(scene) = self.scene.as_mut() else {
            return;
        };
        let outcome = scene.drive(boundary);
        {
            // The compatibility counters, including a failure this very pass found (published before the early return).
            let counters = scene.compat_stats();
            let mut stats = lock(&self.shared.scene_stats);
            stats.gate_holds = counters.gate_holds;
            stats.gate_longest_hold_micros = counters.gate_longest_hold_micros;
            stats.held_compat_frames = counters.held_frames;
            stats.held_compat_revision = counters.held_revision;
            stats.compat_upload_failures = counters.upload_failures;
        }
        // The cumulative upload counters and byte peaks, after this pass's pump and driver (what it submitted, created or
        // committed). Published on **every** pass that has a scene, a failing one and the `Ending` one too: the figures the
        // application reads at the end are the store's high-water marks as of the last pass, so a pass that stopped early
        // must not leave out what it charged.
        self.publish_uploads();
        if self.failed || boundary == Boundary::Ending {
            return;
        }
        let committed = self
            .scene
            .as_ref()
            .is_some_and(|scene| scene.has_committed());
        let mut slot = lock(&self.shared.slot);
        if pumped_more || outcome.more {
            slot.texture_work = true;
        }
        if committed {
            slot.dirty = true;
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one frame step's boundary, marks, faults, native step and outcome read best together"
    )]
    fn frames(&mut self) {
        while let Some(work) = self.next_work() {
            let (rgb, (width, height)) = match work {
                Work::Texture => {
                    self.texture_pass(Boundary::Pass);
                    self.poll_while_running();
                    continue;
                }
                Work::Frame(rgb, size) => (rgb, size),
            };
            // A requested measurement window is adopted before the step. Then the frame-timing marks: `t0` is the latest frame taken
            // from the slot, `t1` the texture driver's pass and any boundary adoption returned, `t2` the frame built (see
            // `native_frame`), and the native step brings `t3`, `t4` and `t5`.
            self.metrics_boundary();
            let latest = self.take_latest();
            self.marks = FrameMarks {
                t0: now_ns(),
                ..FrameMarks::default()
            };
            lock(&self.shared.metrics).begin_step(self.marks.t0);
            self.submitted = 0;
            self.step_counters = None;
            // The frame boundary: the pass immediately before the frame is built, where the driver adopts. A step that
            // is attempted again (busy, skipped, abandoned) keeps its boundary: texture work that woke the thread
            // meanwhile gets an ordinary pass first.
            if self.boundary == FrameBoundary::Passed {
                if self.scene.is_some() && lock(&self.shared.slot).texture_work {
                    lock(&self.shared.slot).texture_work = false;
                    self.texture_pass(Boundary::Pass);
                }
            } else {
                self.texture_pass(Boundary::Frame);
                if self.scene.is_some() {
                    self.boundary = FrameBoundary::Passed;
                }
            }
            self.marks.t1 = now_ns();
            if self.failed {
                // A failure the pass found: nothing more is drawn.
                lock(&self.shared.metrics).end_step();
                self.shared.metrics_changed.notify_all();
                continue;
            }
            self.last_clear = Some(rgb);
            let mut faults = Faults::NONE;
            faults.abandon_after_acquire = TestHooks::consume(&mut self.hooks.abandon_frames);
            faults.nil_command_buffer =
                TestHooks::consume(&mut self.hooks.nil_command_buffer_frames);
            faults.nil_render_pass = TestHooks::consume(&mut self.hooks.nil_render_pass_frames);
            faults.nil_encoder = TestHooks::consume(&mut self.hooks.nil_encoder_frames);
            faults.partial_encoding = TestHooks::consume(&mut self.hooks.partial_encoding_frames);
            faults.error_completion = TestHooks::consume(&mut self.hooks.error_completion_frames);
            let never = self.hooks.never_complete;
            faults.never_complete_drawable_tokens = i32::from(matches!(
                never,
                NeverComplete::Everything | NeverComplete::Drawables
            ));
            faults.never_complete_command_tokens = i32::from(matches!(
                never,
                NeverComplete::Everything | NeverComplete::Commands
            ));
            faults.handler_delay_ms =
                i32::try_from(self.hooks.handler_delay.as_millis()).unwrap_or(i32::MAX);
            faults.completion_delay_ms =
                i32::try_from(self.hooks.completion_delay.as_millis()).unwrap_or(i32::MAX);
            faults.fail_texture_allocation =
                TestHooks::consume(&mut self.hooks.fail_texture_allocation_frames);
            faults.fail_quad_encode = TestHooks::consume(&mut self.hooks.fail_quad_encode_frames);
            faults.retired_token_delay_ms =
                i32::try_from(self.hooks.retired_token_delay.as_millis()).unwrap_or(i32::MAX);
            faults.pause_after_reap_for_token = i32::from(self.hooks.pause_after_reap_for_token);
            faults.abandon_after_prepare =
                TestHooks::consume(&mut self.hooks.abandon_after_prepare_frames);
            faults.skip_next_drawable = TestHooks::consume(&mut self.hooks.skipped_frames);
            faults.capture = i32::from(self.hooks.capture_frames);
            faults.completion_gate = self.frame_gate;
            faults.hold_acquire = self
                .hooks
                .hold_in_acquire
                .as_ref()
                .map_or(std::ptr::null(), Arc::as_ptr);
            faults.hold_after_commit = self
                .hooks
                .hold_after_commit
                .as_ref()
                .map_or(std::ptr::null(), Arc::as_ptr);
            self.hold_acquisition();
            let mut message_buffer = [0_u8; 512];
            let code =
                self.native_frame(rgb, (width, height), &faults, &mut message_buffer, latest);
            self.record_frame(code);
            // Test only: after a frame that committed, the thread stays here, between recording it and moving its ready results into
            // the windows (a step that committed nothing, a busy attempt for one, does not hold: it must be free to retry).
            if let Some(hold) = self
                .hooks
                .hold_before_drain
                .as_ref()
                .filter(|_| code == FRAME_DRAWN)
            {
                let limit = Instant::now() + Duration::from_secs(30);
                while hold.load(Ordering::SeqCst) != 0 && Instant::now() < limit {
                    thread::sleep(Duration::from_millis(1));
                }
            }
            self.drain_timing();
            // The step is over only now: recorded, and its ready results moved into the windows.
            lock(&self.shared.metrics).end_step();
            self.shared.metrics_changed.notify_all();
            match code {
                FRAME_DRAWN => {
                    self.boundary = FrameBoundary::Due;
                    self.drawn();
                    // After each frame boundary's adoption the presenter re-checks whether committed revisions
                    // remain and, if so, keeps a frame wanted (the retries of a busy step consumed the want the
                    // boundary pass set).
                    self.want_frame_for_committed();
                }
                FRAME_FAILED => {
                    self.boundary = FrameBoundary::Due;
                    self.fail(text(&message_buffer));
                }
                // Held back by the initial gate: the want was consumed and is not set again; the thread waits for the facts
                // (a commit, a newer frame) and its next attempt runs a frame boundary, where bounded adoption makes progress.
                FRAME_GATED => {
                    self.boundary = FrameBoundary::Due;
                }
                // Skipped, busy or abandoned: the redraw stays pending, and the thread looks again in a moment.
                _ => {
                    if !self.failed {
                        lock(&self.shared.slot).dirty = true;
                    }
                    thread::sleep(POLL);
                }
            }
            self.publish_compat();
            // A frame can free what it held (the compatibility hold moved on or was cleared): the release work it queued
            // is run by an ordinary pass now, not at some unrelated wake.
            if !self.failed
                && self
                    .scene
                    .as_ref()
                    .is_some_and(|scene| scene.has_release_work())
            {
                lock(&self.shared.slot).texture_work = true;
            }
            self.poll_while_running();
        }
    }

    /// Copies the scene's compatibility-quad counters into the presenter's stats.
    fn publish_compat(&self) {
        let Some(scene) = self.scene.as_ref() else {
            return;
        };
        let counters = scene.compat_stats();
        let mut stats = lock(&self.shared.scene_stats);
        stats.gate_holds = counters.gate_holds;
        stats.gate_longest_hold_micros = counters.gate_longest_hold_micros;
        stats.held_compat_frames = counters.held_frames;
        stats.held_compat_revision = counters.held_revision;
        stats.compat_upload_failures = counters.upload_failures;
    }

    /// The legacy quad of a frame as the native step takes it, and the quad whose image must be uploaded because it is not
    /// the resident texture's (`None` when the resident texture already shows it, or there is no quad).
    fn quad_to_draw<'q>(
        &self,
        latest: Option<&'q MetalScene>,
    ) -> (Option<NativeQuad>, Option<&'q MetalQuad>) {
        match latest.map(MetalScene::quad) {
            None => (None, None),
            Some(None) => (Some(NativeQuad::NONE), None),
            Some(Some(quad)) => {
                let resident = self
                    .resident
                    .as_ref()
                    .is_some_and(|(image, width, height)| {
                        std::ptr::addr_eq(image.as_ptr(), Arc::as_ptr(quad.pixels()))
                            && (*width, *height) == (quad.width(), quad.height())
                    });
                (Some(NativeQuad::of(quad)), (!resident).then_some(quad))
            }
        }
    }

    /// One native frame step, with the slot's latest value (if there is a slot) as the quad.
    ///
    /// The slot is read here, in the frame, after the gate: a quad published before the first frame is in it.
    /// When the quad's image is not the resident texture's, its pixels go to the native step, which makes the
    /// texture inside the call (admission, allocation and copy happen only in a frame that is being drawn).
    /// The clone of the quad, and with it this thread's only reference to the pixel source, is dropped as
    /// soon as the call returns.
    #[expect(
        clippy::too_many_lines,
        reason = "one frame's ticket, build, gate check and native step read best together"
    )]
    fn native_frame(
        &mut self,
        rgb: [f32; 3],
        (width, height): (u32, u32),
        faults: &Faults,
        message_buffer: &mut [u8; 512],
        latest: Option<MetalScene>,
    ) -> i32 {
        // `latest` is the frame taken from the scene's slot at the start of the step (the legacy quad is part of it); the scene
        // source and filter stay with the scene for this attempt's build.
        let mut built_at = 0_u64;
        let generation = latest.as_ref().map(MetalScene::generation);
        let (native, upload_from) = self.quad_to_draw(latest.as_ref());
        let upload = upload_from.map(|quad| {
            let pixels = quad.pixels().pixels();
            NativeUpload {
                pixels: pixels.as_ptr(),
                length: pixels.len(),
            }
        });
        let identity =
            upload_from.map(|quad| (Arc::downgrade(quad.pixels()), quad.width(), quad.height()));
        let mut result = NativeQuadResult::default();
        let native_core = self.core.0.as_ptr();
        let instances = self.instances;
        let mut step = |ticket: *const NativeTicket, packet: *const NativePacket| -> i32 {
            // SAFETY: the core is live and used on this thread only; `rgb`, `faults`, the quad, the upload (whose
            // pixels `latest` keeps alive until after the call), the ticket (the frame's own seam, valid for the
            // call), the result and the message buffer are valid for the call.
            unsafe {
                ck_pr_frame_quad(
                    native_core,
                    rgb.as_ptr(),
                    width,
                    height,
                    faults,
                    native.as_ref().map_or(std::ptr::null(), std::ptr::from_ref),
                    upload.as_ref().map_or(std::ptr::null(), std::ptr::from_ref),
                    packet,
                    ticket,
                    &raw mut result,
                    message_buffer.as_mut_ptr().cast::<c_char>(),
                    message_buffer.len(),
                )
            }
        };
        // A scene presenter's frame runs inside a ledger ticket, begun before the frame is built; it is ended by
        // the frame's fate. A frame that could not begin its ticket is skipped (the pass of the next attempt finds
        // out why).
        let mut no_ticket = false;
        let mut busy = false;
        let mut built: Option<FrameCounters> = None;
        let code = match self.scene.as_mut() {
            None => {
                built_at = now_ns();
                step(std::ptr::null(), std::ptr::null())
            }
            Some(scene) => scene
                .frame(&mut |seam| {
                    let started = Instant::now();
                    let mut build_micros = 0;
                    let mut seam: &mut dyn FrameSeam = seam;
                    let mut frame = None;
                    if let Some(buffer) = instances {
                        // The one instance buffer is free to write only once the previous frame's command buffer has
                        // completed (the presenter's one-frame-in-flight cap). That is proven here, by the command
                        // completion token and never by a drawable's presented token or an upload token, before
                        // anything is written; raising the cap needs a buffer per frame in flight.
                        // SAFETY: the core is live and used on this thread only.
                        if unsafe { ck_pr_instance_buffer_free(native_core) } == 0 {
                            busy = true;
                            return (FRAME_BUSY, false);
                        }
                        frame = seam.build(&BuildRequest {
                            size: (width, height),
                            buffer: buffer.base.as_ptr(),
                            capacity: buffer.capacity,
                            chunk: self.hooks.draw_chunk_limit.unwrap_or(u32::MAX),
                            force_out_of_range_at: self.hooks.force_out_of_range_slot_at,
                        });
                        build_micros =
                            u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                    }
                    if frame.as_ref().is_some_and(|built| built.gated) {
                        // The initial gate: nothing is drawn, the native step is not run and the ticket is discarded.
                        return (FRAME_GATED, false);
                    }
                    let packet = frame.as_ref().map(|frame| NativePacket {
                        batches: frame.batches,
                        batch_count: frame.batch_count,
                        filter: frame.filter,
                        checker: frame.checker,
                        table: frame.table,
                        table_slots: frame.table_slots,
                        use_resources: std::ptr::null(),
                        use_count: 0,
                    });
                    let ticket = NativeTicket {
                        userdata: (&raw mut seam).cast::<c_void>(),
                        prepare: Some(prepare_ticket),
                    };
                    // `t2`: the frame is built and the native step is entered.
                    built_at = now_ns();
                    let step_code = step(
                        &raw const ticket,
                        packet.as_ref().map_or(std::ptr::null(), std::ptr::from_ref),
                    );
                    // A scene presenter's frame without a scene source (clear and legacy quad only) has zero scene counters.
                    built = Some(FrameCounters {
                        build_micros,
                        micros: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                        ..frame.map_or_else(FrameCounters::default, |frame| frame.counters)
                    });
                    (step_code, step_code == FRAME_DRAWN)
                })
                .unwrap_or_else(|| {
                    no_ticket = true;
                    FRAME_SKIPPED
                }),
        };
        self.record_scene_stats(busy, built, code == FRAME_DRAWN);
        self.marks.t2 = built_at;
        self.marks.t3 = result.acquire_begin;
        self.marks.t4 = result.acquire_end;
        self.marks.t5 = result.committed;
        self.submitted = result.submitted;
        self.step_counters = built;
        if no_ticket {
            // Nothing ran: the next attempt starts with a boundary pass, which pumps and finds out why.
            self.boundary = FrameBoundary::Due;
        }
        // The upload is done: this thread lets go of the pixel source now, however the step ended.
        drop(latest);
        if result.uploaded != 0 {
            self.resident = identity;
        } else if result.removed != 0 {
            self.resident = None;
        }
        self.pending_generation = generation;
        code
    }

    /// Notes what a scene frame attempt did to the instance buffer and, when the frame was committed, its counters.
    fn record_scene_stats(&self, busy: bool, built: Option<FrameCounters>, drawn: bool) {
        if !busy && built.is_none() {
            return;
        }
        let mut stats = lock(&self.shared.scene_stats);
        stats.buffer_busy += u64::from(busy);
        if let Some(counters) = built {
            stats.instances_written += counters.instances;
            if drawn {
                stats.frames += 1;
                stats.last = counters;
                stats.peak = stats.peak.max(counters);
            }
        }
    }

    /// The latest frame from the slot: the scene's, or the legacy quad slot's.
    fn take_latest(&mut self) -> Option<MetalScene> {
        match self.scene.as_mut() {
            Some(scene) => Some(scene.read_frame()),
            None => self.quads.as_ref().map(|slot| slot.latest()),
        }
    }

    /// The render thread's boundary for the measurement windows: a pending request is adopted now (its adoption time is this
    /// instant, and it is late if after its `start`), and an ended window is retired once a newer one is waiting.
    fn metrics_boundary(&self) {
        if let Some(hold) = &self.hooks.hold_metrics_adoption
            && lock(&self.shared.metrics).windows.has_pending()
        {
            let limit = Instant::now() + Duration::from_secs(30);
            while hold.load(Ordering::SeqCst) != 0 && Instant::now() < limit {
                thread::sleep(Duration::from_millis(1));
            }
        }
        lock(&self.shared.slot).metrics_check = false;
        lock(&self.shared.metrics).windows.boundary(now_ns());
        self.shared.metrics_changed.notify_all();
    }

    /// Records one finished frame step in the measurement windows, by its own timestamps: a committed frame with valid marks
    /// gives its intervals and counts (the CPU total once, as that frame's sum) and two results still to come (its GPU time and
    /// its presentation); invalid marks and frames that committed nothing are counted and not sampled.
    fn record_frame(&mut self, code: i32) {
        // Test only: after a step that committed, the thread stays here, before recording it, while the flag reads non-zero (a busy
        // attempt that committed nothing does not hold: it must be free to retry).
        if let Some(hold) = self
            .hooks
            .hold_before_record
            .as_ref()
            .filter(|_| code == FRAME_DRAWN)
        {
            let limit = Instant::now() + Duration::from_secs(30);
            while hold.load(Ordering::SeqCst) != 0 && Instant::now() < limit {
                thread::sleep(Duration::from_millis(1));
            }
        }
        let marks = self.marks;
        let counters = self.step_counters.unwrap_or_default();
        if code == FRAME_DRAWN
            && let Some(probe) = &self.hooks.frame_marks
        {
            (probe.0)(&marks);
        }
        let mut metrics = lock(&self.shared.metrics);
        match code {
            FRAME_DRAWN => metrics.drawn_at(&marks, self.submitted, &counters),
            FRAME_SKIPPED | FRAME_ABANDONED => metrics.not_drawn(marks.t0),
            _ => {}
        }
        drop(metrics);
        self.shared.metrics_changed.notify_all();
    }

    /// Moves the timing results the native core consumed from its tokens into the windows, each by the commit mark (`t5`) of the
    /// frame it belongs to: a GPU time (an inverted pair or a zero start is invalid), a presented time (zero is not presented) and
    /// the interval between consecutive presented times (only when both lie in the window). Never waits.
    fn drain_timing(&mut self) {
        let holding = self
            .hooks
            .hold_presentations
            .as_ref()
            .is_some_and(|hold| hold.load(Ordering::SeqCst) != 0);
        if !holding && !self.held_presentations.is_empty() {
            // The test let the kept presentation results go: recorded now, in the order they were taken.
            let held = std::mem::take(&mut self.held_presentations);
            let mut metrics = lock(&self.shared.metrics);
            for sample in &held {
                self.apply_timing(&mut metrics, sample);
            }
            drop(metrics);
        }
        let mut samples = [NativeTiming::default(); 16];
        let mut lost = 0_u64;
        loop {
            // SAFETY: the core is live and used on this thread only; `samples` and `lost` are valid for the call.
            let taken = unsafe {
                ck_pr_take_timing(
                    self.core.0.as_ptr(),
                    samples.as_mut_ptr(),
                    16,
                    &raw mut lost,
                )
            } as usize;
            if taken == 0 {
                break;
            }
            let mut metrics = lock(&self.shared.metrics);
            for sample in &samples[..taken.min(16)] {
                if holding && sample.kind == 2 && self.held_presentations.len() < 16 {
                    self.held_presentations.push(*sample);
                } else {
                    self.apply_timing(&mut metrics, sample);
                }
            }
            drop(metrics);
        }
        if lost != 0 {
            lock(&self.shared.metrics).timing_lost += lost;
        }
        self.shared.metrics_changed.notify_all();
    }

    /// Records one consumed timing result, after the test's replacement of it when one is installed.
    fn apply_timing(&self, metrics: &mut Recorder, sample: &NativeTiming) {
        let (a, b) = self
            .hooks
            .timing_override
            .as_ref()
            .map_or((sample.a, sample.b), |probe| {
                (probe.0)(sample.kind, sample.t5, sample.a, sample.b)
            });
        match sample.kind {
            1 => metrics.gpu(sample.t5, a, b),
            2 => metrics.presented(sample.t5, a),
            _ => {}
        }
    }

    /// A test's blocked acquisition: waits (at most 30 s) while the shared flag reads non-zero. The flag is
    /// an `AtomicI32` that only Rust code reads and writes.
    fn hold_acquisition(&self) {
        if let Some(hold) = &self.hooks.hold_next_drawable {
            let limit = Instant::now() + Duration::from_secs(30);
            while hold.load(Ordering::SeqCst) != 0 && Instant::now() < limit {
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// The poll for completion errors between the frame steps, in the order the test hooks select.
    fn poll_while_running(&mut self) {
        match self.hooks.error_poll {
            ErrorPoll::Normal => {}
            ErrorPoll::Never => return,
            ErrorPoll::OutstandingFirst => {
                if self.committed > 0 && !self.adverse_done {
                    self.adverse_done = true;
                    self.ask_outstanding_after_the_completion();
                }
            }
        }
        self.poll_completion_errors();
    }

    /// Test order: the completion has arrived, then the thread asks whether anything is outstanding, then
    /// it polls. Bounded at 5 s; the observer's completion count says when the handler has run.
    fn ask_outstanding_after_the_completion(&self) {
        let limit = Instant::now() + Duration::from_secs(5);
        if let Some(observer) = &self.hooks.observer {
            while observer.completion_handlers_run() < i64::try_from(self.committed).unwrap_or(0)
                && Instant::now() < limit
            {
                thread::sleep(Duration::from_millis(1));
            }
        }
        // The handler sets its token a moment after it counts itself, and the drawable's presented handler
        // must have run too.
        while observer_presented_short(self) && Instant::now() < limit {
            thread::sleep(Duration::from_millis(1));
        }
        thread::sleep(Duration::from_millis(100));
        // The answer is recorded: with every token arrived and an error not yet taken it must be 1.
        // SAFETY: the core is live and used on this thread only.
        let outstanding = unsafe { ck_pr_outstanding(self.core.0.as_ptr()) };
        if let Some(seen) = &self.hooks.outstanding_seen {
            seen.store(outstanding, Ordering::SeqCst);
        }
    }

    fn drawn(&mut self) {
        self.committed += 1;
        if let Some(generation) = self.pending_generation {
            // Value first, flag second: a reader that sees the flag sees a generation.
            self.shared
                .drawn_generation
                .store(generation, Ordering::Release);
            self.shared.drawn_any.store(true, Ordering::Release);
        }
        if let Some(frames) = &self.hooks.frames {
            frames.fetch_add(1, Ordering::SeqCst);
        }
        if let (Some(log), Some(rgb)) = (&self.hooks.clear_log, self.last_clear) {
            lock(log).push(rgb);
        }
        if let (Some(log), Some(generation)) = (&self.hooks.generation_log, self.pending_generation)
        {
            lock(log).push(generation);
        }
        if !self.first_reported {
            self.first_reported = true;
            self.shared.report(PresenterReport::FirstFrameQueued);
        }
        if self
            .hooks
            .panic_after_frames
            .is_some_and(|limit| self.committed >= limit)
        {
            match self.hooks.forced_panic_payload {
                Some(raise) => raise(),
                None => panic!("a forced render thread panic"),
            }
        }
    }

    fn fail(&mut self, message_text: String) {
        if !self.failed {
            self.failed = true;
            self.shared.report(PresenterReport::Failed {
                message: message_text,
            });
        }
    }

    /// A command buffer that completed with an error: its token is set (Metal has finished with its
    /// resources) and the thread reports `Failed`.
    fn poll_completion_errors(&mut self) {
        let mut buffer = [0_u8; 512];
        // SAFETY: the core is live and used on this thread only; the buffer is valid for the call.
        let error = unsafe {
            ck_pr_poll(
                self.core.0.as_ptr(),
                buffer.as_mut_ptr().cast::<c_char>(),
                buffer.len(),
            )
        };
        // The tokens the poll just consumed hold timing results: moved into the windows now.
        self.drain_timing();
        if error != 0 {
            self.fail(text(&buffer));
        }
    }

    /// After the stop: stops drawing and calling the driver for new work (the driver's last pass is the `Ending`
    /// one), then gathers **every** proof - the texture store and ledger, and the presenter's own drawable and
    /// command tokens - without releasing anything, looking at each again without waiting until **both** are complete
    /// or the one absolute stop deadline (real time, re-read on every look) passes. Only when both are complete are
    /// the store's and ledger's objects released, then the presenter's holders, then the queue and device, and the
    /// context last: `Proven`. Otherwise nothing device-dependent is released and `Unproven` names every outstanding
    /// item from either side.
    fn teardown(&mut self) -> CleanupResult {
        let deadline = lock(&self.shared.slot)
            .stop_deadline
            .unwrap_or_else(Instant::now);
        // 1. The driver's last pass; no building ticket exists between frame steps.
        self.texture_pass(Boundary::Ending);
        let mut outstanding = [0_u8; 512];
        loop {
            // 2. Look at the store and ledger (this stops offering), and at the presenter's tokens. A completion
            //    error either side saw is a failure, reported before anything is released.
            if let Some(scene) = self.scene.as_mut()
                && let Some(message) = scene.observe()
            {
                self.fail(message);
            }
            let scene_outstanding = self.scene.as_mut().and_then(|scene| scene.prove());
            // SAFETY: the core is live and used on this thread only; the buffer is valid for the call.
            let presenter_done = unsafe {
                ck_pr_tokens_state(
                    self.core.0.as_ptr(),
                    outstanding.as_mut_ptr().cast::<c_char>(),
                    outstanding.len(),
                )
            } != 0;
            self.poll_completion_errors();
            if presenter_done && scene_outstanding.is_none() {
                // 3. Both complete. A failure the ledger latched while it was being proven is reported first.
                if let Some(scene) = self.scene.as_mut()
                    && let Some(message) = scene.observe()
                {
                    self.fail(message);
                }
                // Every token arrived and the poll above moved its result into the windows: nothing will be committed or shown again,
                // so no window has a closing interval still to come.
                lock(&self.shared.metrics).stop();
                self.shared.metrics_changed.notify_all();
                // The last look at the store's counters and peaks, before it is released and the scene dropped.
                self.publish_uploads();
                if let Some(scene) = self.scene.as_mut()
                    && let Err(reason) = scene.release()
                {
                    return self.unproven(format!(
                        "the texture store and ledger refused their release ({reason}); the device, queue, layer, store and ledger were kept"
                    ));
                }
                if self.scene.is_some() {
                    released(self.hooks.release_log.as_ref(), "store_and_ledger");
                }
                // SAFETY: every token arrived and the store and ledger are proven finished and released, so the GPU and
                // Core Animation are done with what the core owns; nothing else uses it.
                unsafe { ck_pr_release_all(self.core.0.as_ptr()) };
                released(self.hooks.release_log.as_ref(), "presenter");
                // The scene (released) and, last, the context.
                drop(self.scene.take());
                if let Some(context) = self.context.take() {
                    released(self.hooks.release_log.as_ref(), "context");
                    drop(ManuallyDrop::into_inner(context));
                }
                return CleanupResult::Proven;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let presenter = if presenter_done {
                    "no drawable or command token".to_owned()
                } else {
                    text(&outstanding)
                };
                let reason = match scene_outstanding {
                    None if self.scene.is_none() => format!(
                        "the stop deadline passed with {presenter}; the device, queue, layer and command buffer were kept"
                    ),
                    store => format!(
                        "the stop deadline passed with {presenter}{}; the device, queue, layer, command buffer, texture store and ledger were kept",
                        store.map_or_else(String::new, |reason| format!(
                            " and the texture store or ledger incomplete ({reason})"
                        ))
                    ),
                };
                return self.unproven(reason);
            }
            if let Some(pause) = self.hooks.pause_in_wait.take() {
                // A test's late schedule: whatever time this takes comes out of the same deadline, and
                // the tokens are looked at again before the deadline is judged.
                thread::sleep(pause);
            } else {
                thread::sleep(left.min(Duration::from_millis(1)));
            }
        }
    }

    /// The unproven end: nothing device-dependent is released - the store and ledger and the context are kept to the
    /// process end (a leak by design), like the core.
    fn unproven(&mut self, reason: String) -> CleanupResult {
        // The last look at the counters, before the scene is forgotten (a lower bound: see `publish_upload_counters`).
        self.publish_uploads();
        if let Some(scene) = self.scene.take() {
            std::mem::forget(scene);
        }
        CleanupResult::Unproven { reason }
    }
}

/// Writes `text` into the native message buffer, truncated, NUL-terminated.
fn write_message(buffer: *mut c_char, capacity: usize, text: &str) {
    if buffer.is_null() || capacity == 0 {
        return;
    }
    let bytes = text.as_bytes();
    let length = bytes.len().min(capacity - 1);
    // SAFETY: `buffer` is valid for `capacity` writable bytes (the native caller's array) and nothing else uses it
    // while this call runs.
    let target = unsafe { std::slice::from_raw_parts_mut(buffer.cast::<u8>(), capacity) };
    target[..length].copy_from_slice(&bytes[..length]);
    target[length] = 0;
}

/// The native frame step's request for its ledger token (`CKPrTicket::prepare`), made on the render thread inside
/// the step, after the frame is encoded and before the commit. It never unwinds into native code.
unsafe extern "C" fn prepare_ticket(
    userdata: *mut c_void,
    error: *mut c_char,
    capacity: usize,
) -> *mut c_void {
    // SAFETY: `userdata` is the address of the `&mut dyn FrameSeam` the frame step holds, valid for the whole
    // native call that made this one, which runs on this thread only.
    let seam = unsafe { &mut *userdata.cast::<&mut dyn FrameSeam>() };
    match catch_unwind(AssertUnwindSafe(|| seam.prepare())) {
        Ok(Ok(token)) => token,
        Ok(Err(refusal)) => {
            write_message(error, capacity, &refusal);
            std::ptr::null_mut()
        }
        Err(payload) => {
            unwind::dispose(payload);
            write_message(error, capacity, "the ticket preparation panicked");
            std::ptr::null_mut()
        }
    }
}

/// Renders the same clear into an off-screen `width` x `height` BGRA8 texture and reads its pixels back,
/// synchronously, on the calling thread. It shares the presenter's clear-encoding code and is used by
/// the readback test and by nothing else.
///
/// # Errors
///
/// The native layer's message when there is no device or the work fails.
pub fn readback_clear(width: u32, height: u32, rgb: [f32; 3]) -> Result<Vec<u8>, Error> {
    let length = (width as usize) * (height as usize) * 4;
    let mut pixels = vec![0_u8; length];
    let mut error = [0_u8; 512];
    // SAFETY: `rgb`, `pixels` (of the stated length) and `error` are valid for the synchronous call.
    let status = unsafe {
        ck_pr_readback(
            width,
            height,
            rgb.as_ptr(),
            pixels.as_mut_ptr(),
            pixels.len(),
            error.as_mut_ptr().cast::<c_char>(),
            error.len(),
        )
    };
    if status == 0 {
        Ok(pixels)
    } else {
        Err(message(&error))
    }
}

/// [`readback_clear`] with a quad drawn over the clear, by the presenter's own encoding code, pipeline and
/// samplers (so the pixel tests check exactly what the presenter draws). `quad` `None` is the clear alone.
/// Used by the readback tests and by nothing else.
///
/// # Errors
///
/// The native layer's message when there is no device or the work fails.
pub fn readback_quad(
    width: u32,
    height: u32,
    rgb: [f32; 3],
    quad: Option<&MetalQuad>,
) -> Result<Vec<u8>, Error> {
    let length = (width as usize) * (height as usize) * 4;
    let mut pixels = vec![0_u8; length];
    let mut error = [0_u8; 512];
    let native = quad.map_or(NativeQuad::NONE, NativeQuad::of);
    let image = quad.map_or(&[][..], |quad| quad.pixels().pixels());
    // SAFETY: `rgb`, the library bytes, `native`, `image`, `pixels` (of the stated length) and `error` are
    // valid for the synchronous call.
    let status = unsafe {
        ck_pr_readback_quad(
            width,
            height,
            rgb.as_ptr(),
            QUAD_LIBRARY.as_ptr(),
            QUAD_LIBRARY.len(),
            &raw const native,
            image.as_ptr(),
            image.len(),
            pixels.as_mut_ptr(),
            pixels.len(),
            error.as_mut_ptr().cast::<c_char>(),
            error.len(),
        )
    };
    if status == 0 {
        Ok(pixels)
    } else {
        Err(message(&error))
    }
}

/// A texture of an off-screen scene ([`readback_scene`]): `width` x `height` premultiplied RGBA8 pixels, rows top first.
#[derive(Debug, Clone, Copy)]
pub struct ReadbackImage<'a> {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// Exactly `width x height x 4` bytes.
    pub pixels: &'a [u8],
}

/// Which texture an off-screen instance shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadbackTexture {
    /// The checker, as for a key with nothing shown yet (always sampled nearest, over the instance's whole destination).
    Checker,
    /// The image at this index of the images given.
    Image(usize),
}

/// One instance of an off-screen scene ([`readback_scene`]); validated like the presenter validates a source's instances.
#[derive(Debug, Clone, Copy)]
pub struct ReadbackInstance {
    /// The texture it shows.
    pub texture: ReadbackTexture,
    /// The destination in whole pixels.
    pub destination: DestinationRect,
    /// The visible part of the texture as fractions of it, `[left, top, right, bottom]`.
    pub source: [f32; 4],
}

/// Which path an off-screen scene is drawn through ([`readback_scene_counted`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadbackPath {
    /// CK-116's per-texture batches (the hidden ordinary option M5-10b measures its baseline with).
    Ordinary,
    /// The bindless path: a table of `texture_slots` entries (slot 0 the checker, then the images given, the rest the checker),
    /// one run of instances selecting their image by slot, split only into draws of at most `chunk` instances.
    Bindless {
        /// The entry count of the table (at least one more than the images given).
        texture_slots: u32,
        /// The most instances one draw takes.
        chunk: u32,
        /// The `n`-th drawn instance has its slot forced out of the table: skipped and counted, never read by the GPU.
        force_out_of_range_at: Option<usize>,
    },
}

/// Renders the clear, an optional legacy quad and a scene's instances off screen and reads the pixels back, through the
/// presenter's own instance layout, batching, pipeline, samplers and encoding (so the pixel tests check exactly what the
/// presenter draws). `filter` samples every image batch; the checker is always nearest. Used by the readback tests and by
/// nothing else. This is the ordinary path; [`readback_scene_counted`] chooses the path and returns the frame's counters.
///
/// # Errors
///
/// An instance naming an image that was not given, a malformed image, or the native layer's message when there is no
/// device or the work fails.
pub fn readback_scene(
    width: u32,
    height: u32,
    rgb: [f32; 3],
    quad: Option<&MetalQuad>,
    filter: MetalFilter,
    images: &[ReadbackImage<'_>],
    instances: &[ReadbackInstance],
) -> Result<Vec<u8>, Error> {
    readback_scene_counted(
        width,
        height,
        rgb,
        quad,
        filter,
        images,
        instances,
        ReadbackPath::Ordinary,
    )
    .map(|(pixels, _)| pixels)
}

/// [`readback_scene`] through `path`, returning the pixels and what building the frame counted (draws, slots skipped, pins).
///
/// # Errors
///
/// As [`readback_scene`], and a bindless table smaller than the images given plus the checker.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the size, the clear colour, the quad, the filter, the images, the instances and the path the readback names, and the one native call that draws them"
)]
pub fn readback_scene_counted(
    width: u32,
    height: u32,
    rgb: [f32; 3],
    quad: Option<&MetalQuad>,
    filter: MetalFilter,
    images: &[ReadbackImage<'_>],
    instances: &[ReadbackInstance],
    path: ReadbackPath,
) -> Result<(Vec<u8>, FrameCounters), Error> {
    struct Listed<'a>(&'a [ReadbackInstance]);

    impl MetalSceneSource<usize> for Listed<'_> {
        fn place(&self, _width: u32, _height: u32, out: &mut dyn FnMut(MetalInstance<usize>)) {
            for instance in self.0 {
                out(MetalInstance {
                    key: match instance.texture {
                        ReadbackTexture::Checker => usize::MAX,
                        ReadbackTexture::Image(index) => index,
                    },
                    destination: instance.destination,
                    source: instance.source,
                    role: InstanceRole::Scene,
                });
            }
        }
    }

    if let Some(bad) = instances.iter().find(|instance| {
        matches!(instance.texture, ReadbackTexture::Image(index) if index >= images.len())
    }) {
        return Err(Error(format!(
            "an instance names the image {:?}, but {} were given",
            bad.texture,
            images.len()
        )));
    }
    let length = (width as usize) * (height as usize) * 4;
    let mut pixels = vec![0_u8; length];
    let mut error = [0_u8; 512];
    let native = quad.map_or(NativeQuad::NONE, NativeQuad::of);
    let quad_image = quad.map_or(&[][..], |quad| quad.pixels().pixels());
    let natives: Vec<NativeReadbackImage> = images
        .iter()
        .map(|image| NativeReadbackImage {
            pixels: image.pixels.as_ptr(),
            length: image.pixels.len(),
            width: image.width,
            height: image.height,
        })
        .collect();
    let checker = checker_pixels();
    let native_checker = NativeReadbackImage {
        pixels: checker.as_ptr(),
        length: checker.len(),
        width: CHECKER_SIDE,
        height: CHECKER_SIDE,
    };
    let mut records = vec![
        InstanceRecord {
            destination: [0.0; 4],
            source: [0.0; 4],
            slot: 0,
            padding: [0; 3],
        };
        instances.len()
    ];
    let mut batches = Vec::new();
    let mut resolved = HashMap::new();
    let capacity = records.len();
    let (layout, table_slots) = match path {
        ReadbackPath::Ordinary => (Layout::Ordinary, 0),
        ReadbackPath::Bindless {
            texture_slots,
            chunk,
            force_out_of_range_at,
        } => {
            if usize::try_from(texture_slots).unwrap_or(usize::MAX) <= images.len() {
                return Err(Error(format!(
                    "the table of {texture_slots} slots cannot hold the checker and {} images",
                    images.len()
                )));
            }
            (
                Layout::Bindless {
                    texture_slots,
                    chunk,
                    force_out_of_range_at,
                },
                texture_slots,
            )
        }
    };
    let counters = build_scene_in(
        layout,
        &Listed(instances),
        (width, height),
        capacity,
        &mut |slot, record| records[slot] = record,
        // The ordinary handle is 1 + the image's index (the native readback maps it to the texture it makes); the bindless
        // slot is the same number (slot 0 the checker), the handle null.
        &mut |key| {
            (key != usize::MAX).then(|| Resolved {
                handle: if matches!(path, ReadbackPath::Ordinary) {
                    (key + 1) as *mut c_void
                } else {
                    std::ptr::null_mut()
                },
                slot: u32::try_from(key + 1).unwrap_or(u32::MAX),
            })
        },
        // Every instance of an off-screen scene has the scene role: there is no compatibility quad to decide.
        false,
        &mut |_| CompatDecision::Gate,
        &mut resolved,
        &mut batches,
    )
    .counters;
    let drawn_bytes = usize::try_from(counters.instances).unwrap_or(0) * INSTANCE_STRIDE;
    // SAFETY: every pointer (`rgb`, the library, `native`, the images and the checker with their pixels, the records and
    // the batches) is valid for the synchronous call, and the lengths are those of the slices they come from.
    let status = unsafe {
        ck_pr_readback_scene(
            width,
            height,
            rgb.as_ptr(),
            QUAD_LIBRARY.as_ptr(),
            QUAD_LIBRARY.len(),
            &raw const native,
            quad_image.as_ptr(),
            quad_image.len(),
            natives.as_ptr(),
            natives.len(),
            &raw const native_checker,
            records.as_ptr().cast::<c_void>(),
            drawn_bytes,
            batches.as_ptr(),
            u32::try_from(batches.len()).unwrap_or(u32::MAX),
            i32::from(filter == MetalFilter::Linear),
            table_slots,
            pixels.as_mut_ptr(),
            pixels.len(),
            error.as_mut_ptr().cast::<c_char>(),
            error.len(),
        )
    };
    if status == 0 {
        Ok((pixels, counters))
    } else {
        Err(message(&error))
    }
}

#[cfg(test)]
mod tests {
    //! The three places the presenter catches an unwind or joins a thread that may have panicked, each with a
    //! payload whose `Drop` panics (`unwind::tests::Bomb`). None needs a device or a window: `Shared` is plain
    //! data, `finish` is what the render thread does once `run` is over, and `poll_end` joins whatever thread
    //! it is given.

    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::unwind::tests::{Bomb, caught, explode};

    const BOUND: Duration = Duration::from_secs(10);

    const PANICKED: &str = "the render thread panicked; everything it owned was kept";

    fn counter() -> Arc<AtomicUsize> {
        Arc::new(AtomicUsize::new(0))
    }

    fn shared(wake: Wake) -> Arc<Shared> {
        Arc::new(Shared {
            slot: Mutex::new(Slot {
                clear: [0.0; 3],
                size: (1, 1),
                dirty: false,
                texture_work: false,
                stop: false,
                stop_deadline: None,
                metrics_check: false,
            }),
            signal: Condvar::new(),
            reports: Mutex::new(VecDeque::new()),
            result: Mutex::new(None),
            wake,
            stop_budget: STOP_BUDGET,
            drawn_generation: AtomicU64::new(0),
            drawn_any: AtomicBool::new(false),
            scene_stats: Mutex::new(SceneStats::default()),
            metrics: Mutex::new(Recorder::default()),
            metrics_changed: Condvar::new(),
        })
    }

    /// A wake that counts its calls.
    fn counting(calls: &Arc<AtomicUsize>) -> Wake {
        let calls = Arc::clone(calls);
        Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
        })
    }

    /// A wake that panics with a `Bomb` on every call.
    fn exploding(drops: &Arc<AtomicUsize>) -> Wake {
        let drops = Arc::clone(drops);
        Arc::new(move || explode(&drops))
    }

    #[test]
    fn a_wake_that_panics_with_such_a_payload_returns_and_leaves_the_queued_report() {
        let drops = counter();
        let shared = shared(exploding(&drops));
        shared.report(PresenterReport::FirstFrameQueued);
        assert_eq!(
            lock(&shared.reports).front(),
            Some(&PresenterReport::FirstFrameQueued),
            "the report was queued before the wake"
        );
        shared.wake();
        assert_eq!(drops.load(Ordering::SeqCst), 2, "each payload dropped once");
    }

    #[test]
    fn a_render_panic_with_such_a_payload_is_still_unproven_stored_and_woken() {
        let drops = counter();
        let wakes = counter();
        let shared = shared(counting(&wakes));
        let quad_wakes = counter();
        let quads = MetalQuadSlot::new();
        quads.register_wake(counting(&quad_wakes));
        let payload = caught(Bomb {
            drops: Arc::clone(&drops),
            then: None,
        });

        let finished = catch_unwind(AssertUnwindSafe(|| {
            finish(&shared, Some(&quads), None, Err(payload));
        }));

        assert!(
            finished.is_ok(),
            "the payload's panic ended the thread early"
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(
            *lock(&shared.result),
            Some(CleanupResult::Unproven {
                reason: PANICKED.to_owned()
            })
        );
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            1,
            "the wake ran after the result"
        );
        quads.publish(MetalScene::new(1, None));
        assert_eq!(
            quad_wakes.load(Ordering::SeqCst),
            0,
            "the slot's wake was cleared"
        );
    }

    #[test]
    fn a_render_thread_that_ended_normally_keeps_its_result() {
        let wakes = counter();
        let shared = shared(counting(&wakes));
        finish(&shared, None, None, Ok(CleanupResult::Proven));
        assert_eq!(*lock(&shared.result), Some(CleanupResult::Proven));
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }

    /// A presenter over a thread that has stored `stored` and then ends by a panic with a `Bomb`.
    fn presenter_ended_by_a_bomb(
        stored: Option<CleanupResult>,
        drops: &Arc<AtomicUsize>,
    ) -> MetalPresenter {
        let shared = shared(Arc::new(|| {}));
        let thread = thread::spawn({
            let shared = Arc::clone(&shared);
            let drops = Arc::clone(drops);
            move || {
                *lock(&shared.result) = stored;
                explode(&drops)
            }
        });
        MetalPresenter {
            shared,
            thread: Some(thread),
            ended: None,
        }
    }

    #[test]
    fn poll_end_joins_a_thread_that_ended_by_such_a_panic_and_returns_its_stored_result() {
        let drops = counter();
        let stored = CleanupResult::Unproven {
            reason: PANICKED.to_owned(),
        };
        let mut presenter = presenter_ended_by_a_bomb(Some(stored.clone()), &drops);

        let ended = catch_unwind(AssertUnwindSafe(|| presenter.poll_end(BOUND)));

        assert_eq!(
            ended.ok(),
            Some(Some(stored.clone())),
            "poll_end unwound or returned nothing"
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(
            presenter.poll_end(Duration::ZERO),
            Some(stored),
            "and it stays so"
        );
    }

    #[test]
    fn poll_end_names_a_thread_that_ended_by_such_a_panic_before_storing_a_result() {
        let drops = counter();
        let mut presenter = presenter_ended_by_a_bomb(None, &drops);

        let ended = catch_unwind(AssertUnwindSafe(|| presenter.poll_end(BOUND)));

        assert_eq!(
            ended.ok(),
            Some(Some(CleanupResult::Unproven {
                reason: "the render thread ended without a cleanup result".to_owned()
            }))
        );
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
