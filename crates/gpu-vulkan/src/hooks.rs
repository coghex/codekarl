//! The test hooks: a thin seam over acquire, present, submit and swap chain creation that can return each
//! error the ledger names, a gate on frame progress, a forced stop-time panic and an observer of the ledger
//! and of every destroy call.
//!
//! The hooks reach the presenter only through `VulkanPresenter::start_with_test_hooks` (`doc(hidden)`):
//! there is no Cargo feature, environment variable or flag, and a test checks that `codekarl`'s sources
//! never name the constructor. **A hook models the underlying operation faithfully.** An injected
//! out-of-date, suboptimal or surface-lost present performs the real present first, so it has queued its
//! semaphore wait and its present fence, exactly as the real error does; an injected out-of-memory present
//! queues nothing (the real call is not made). An injected acquire error makes no acquire call (nothing was
//! signalled), except `VK_SUBOPTIMAL_KHR`, which is a success and so acquires for real. An injected swap
//! chain creation error makes no creation call; the ledger retires the old swap chain anyway, as the
//! specification says a failed creation does.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use ash::vk;

use crate::ledger::LedgerSnapshot;
use crate::slot::VulkanQuadSlot;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long the render thread waits between looks when something is outstanding.
pub(crate) const POLL: Duration = Duration::from_millis(2);

/// A scheduling pause for the tests: the render thread, when it reaches the pause point, waits while the
/// pause is held (at most 10 s), and says that it has reached it.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct Pause {
    held: AtomicBool,
    reached: AtomicBool,
}

impl Pause {
    /// A pause that is not held.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Holds the thread the next time it reaches the pause point.
    pub fn hold(&self) {
        self.held.store(true, Ordering::SeqCst);
    }

    /// Lets the thread go.
    pub fn release(&self) {
        self.held.store(false, Ordering::SeqCst);
    }

    /// Whether the thread is waiting at the pause point right now.
    #[must_use]
    pub fn reached(&self) -> bool {
        self.reached.load(Ordering::SeqCst)
    }

    /// The pause point: waits while held.
    pub(crate) fn wait(&self) {
        if !self.held.load(Ordering::SeqCst) {
            return;
        }
        self.reached.store(true, Ordering::SeqCst);
        let limit = std::time::Instant::now() + Duration::from_secs(10);
        while self.held.load(Ordering::SeqCst) && std::time::Instant::now() < limit {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.reached.store(false, Ordering::SeqCst);
    }
}

/// A surface `currentExtent` a test sets (see [`TestHooks::surface_extent`]).
#[doc(hidden)]
pub type SurfaceExtent = Arc<Mutex<Option<(u32, u32)>>>;

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

    /// Takes a permit without waiting: whether there was one. For the tests, to drain the gate.
    pub fn take_for_test(&self) -> bool {
        let mut permits = lock(&self.permits);
        if *permits > 0 {
            *permits -= 1;
            true
        } else {
            false
        }
    }

    /// Takes a permit, waiting in short slices while `keep_waiting` says to.
    pub(crate) fn take(&self, keep_waiting: impl Fn() -> bool) -> bool {
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

/// One thing the render thread did, in order, for the tests.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// An image was acquired.
    Acquire {
        /// Its index.
        index: u32,
    },
    /// An acquire returned an error (injected or real) and acquired nothing.
    AcquireError(vk::Result),
    /// A frame's commands were submitted.
    Submit,
    /// A present on swap chain `chain` returned this result (the one the ledger saw).
    Present {
        /// The swap chain.
        chain: u64,
        /// The result.
        result: vk::Result,
    },
    /// An empty submit waiting on a semaphore was queued to consume its pending signal.
    Drain {
        /// What it drains: `"acquire"` or `"render-finished"`.
        what: &'static str,
    },
    /// `vkReleaseSwapchainImagesEXT` was called for an abandoned image.
    ReleaseImages,
    /// A fence was seen signalled; it proved this kind of operation, for this swap chain.
    FenceSignalled {
        /// `"submit"`, `"present"`, `"acquire-drain"` or `"render-drain"`.
        proof: &'static str,
        /// The swap chain the proof belongs to, if any.
        chain: Option<u64>,
    },
    /// A swap chain was created for this window size and became current.
    ChainCreated {
        /// The swap chain.
        id: u64,
        /// The pixel size the window asked for.
        size: (u32, u32),
    },
    /// A swap chain was retired.
    ChainRetired(u64),
    /// A swap chain creation failed with this error.
    CreationFailed(vk::Result),
    /// A swap chain's image views and framebuffers were destroyed (just before the swap chain itself).
    ChainChildrenDestroyed(u64),
    /// A swap chain was destroyed.
    ChainDestroyed(u64),
    /// A class of objects was destroyed by the teardown, in the order of design §7.
    Destroyed(&'static str),
    /// The presenter reported `Failed`.
    Failed(String),
    /// The swap chain's format was chosen from a restricted list ([`TestHooks::forced_surface_formats`] only): what
    /// the real surface offered, and the format chosen from it.
    Formats {
        /// Every format the surface offered, before the restriction.
        offered: Vec<vk::Format>,
        /// The format chosen.
        chosen: vk::Format,
    },
    /// The quad pipeline and its layouts, pool and samplers were created.
    QuadPipelineCreated,
    /// A quad texture's creation succeeded (a partial one that failed is not announced).
    QuadTextureCreated {
        /// The image's size.
        size: (u32, u32),
    },
    /// An upload was recorded into a frame's command buffer (it is not uploaded until that frame is submitted).
    QuadUploadRecorded,
    /// The resident texture was retired (replaced, or removed by a scene without a quad).
    QuadTextureRetired,
    /// A quad texture was destroyed: `teardown` says whether by the proven teardown or by retirement.
    QuadTextureDestroyed {
        /// By the teardown.
        teardown: bool,
    },
    /// A quad staging buffer was destroyed.
    QuadStagingDestroyed {
        /// By the teardown.
        teardown: bool,
    },
    /// A frame waited because the quad objects' cap was reached.
    QuadAdmissionWait,
    /// The quad was not drawn: the surface format is not one textured drawing supports. Once per run.
    QuadSkipped(vk::Format),
    /// A filter change waited for the last submit that drew the texture to be proven.
    QuadRefilterWait,
}

/// Where creating the quad's pipeline objects fails (a test): after the objects before it exist.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineFault {
    /// The second sampler's creation fails, with the first sampler the only object that exists.
    SecondSampler,
    /// The graphics pipeline's creation fails, with the samplers, layouts and pool made.
    Pipeline,
}

/// Faults a test injects into creating the quad's objects (each makes the call fail after the objects before it
/// exist, so the cleanup of a partial creation is exercised).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default)]
pub struct QuadFaults {
    /// Creating the quad's pipeline objects fails here.
    pub pipeline: Option<PipelineFault>,
    /// The texture's memory allocation fails (after its image exists).
    pub texture_allocation: bool,
    /// The staging buffer's memory allocation fails (after the buffer exists).
    pub staging_allocation: bool,
}

/// One quad object's memory allocation, as queried (`VkMemoryRequirements::size`).
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuadAllocation {
    /// `"texture"` or `"staging"`.
    pub kind: &'static str,
    /// The queried size in bytes.
    pub size: u64,
}

/// What the observer counted of the quad objects. An object is counted from the first handle that exists, so a
/// partial creation counts and a call that made nothing does not.
#[doc(hidden)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuadStats {
    /// Textures created (partial ones included).
    pub textures_created: u64,
    /// Textures alive now.
    pub textures_alive: u64,
    /// The most textures alive at once.
    pub textures_max_alive: u64,
    /// Textures destroyed.
    pub textures_released: u64,
    /// Staging buffers created (partial ones included).
    pub staging_created: u64,
    /// Staging buffers alive now.
    pub staging_alive: u64,
    /// The most staging buffers alive at once.
    pub staging_max_alive: u64,
    /// Staging buffers destroyed.
    pub staging_released: u64,
    /// Frames that waited for room under the cap.
    pub admission_waits: u64,
    /// Uploads carried by a submitted frame.
    pub uploads: u64,
    /// Each allocation's queried size, in order.
    pub allocations: Vec<QuadAllocation>,
}

/// What bounds a wait of the scene presenter's render thread.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    /// No deadline: nothing is outstanding and no swap-chain creation retry is pending.
    Unbounded,
    /// The 1 ms look at GPU work that is outstanding (an upload batch, a submitted frame, release work).
    GpuPoll,
    /// The pending swap-chain creation retry, `creation_retry_at`, is the earliest deadline.
    RetryDeadline,
}

/// Records each wait the scene presenter's render thread enters and each frame attempt it makes. Test evidence, not
/// an engine counter.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct WaitObserver {
    entries: Mutex<Vec<(WaitKind, std::time::Instant)>>,
    attempts: AtomicU64,
}

impl WaitObserver {
    /// A new observer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn entered(&self, kind: WaitKind) {
        lock(&self.entries).push((kind, std::time::Instant::now()));
    }

    pub(crate) fn attempted(&self) {
        self.attempts.fetch_add(1, Ordering::SeqCst);
    }

    /// Every wait entered so far, in order.
    #[must_use]
    pub fn entries(&self) -> Vec<WaitKind> {
        lock(&self.entries).iter().map(|(kind, _)| *kind).collect()
    }

    /// When each wait was entered, in the same order as [`entries`](Self::entries).
    #[must_use]
    pub fn times(&self) -> Vec<std::time::Instant> {
        lock(&self.entries).iter().map(|(_, time)| *time).collect()
    }

    /// How many waits were entered.
    #[must_use]
    pub fn waits(&self) -> usize {
        lock(&self.entries).len()
    }

    /// How many frame steps the thread attempted.
    #[must_use]
    pub fn attempts(&self) -> u64 {
        self.attempts.load(Ordering::SeqCst)
    }
}

/// What a frame did with the compatibility quad.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatKind {
    /// No compatibility instance was drawn (none placed, or none in the frame).
    Absent,
    /// The current image resolved and was drawn.
    Drew,
    /// The current image did not resolve; the held one was drawn in its place.
    DrewHeld,
    /// Neither resolved: the frame is held back (the initial gate).
    Gated,
}

/// What one frame build did, for the tests that read the instance buffer's write rule.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildRecord {
    /// What the build counted.
    pub counters: crate::FrameCounters,
    /// `frame_proven(previous submission)` read at the moment of the first instance write (true when there was no
    /// previous submission).
    pub previous_proven: bool,
    /// What the frame did with the compatibility quad.
    pub compat: CompatKind,
}

/// The scene presenter's test hooks: the specification's wait-entry observer, a pause between the idle recheck and the
/// wait, a held frame, and the records the cases read. Nothing here changes what the presenter does.
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
pub struct SceneHooks {
    /// Records each wait entry and each frame attempt.
    pub waits: Option<Arc<WaitObserver>>,
    /// The thread waits here, while the pause is held, after the last recheck under the slot lock and before it waits.
    pub pause_before_wait: Option<Arc<Pause>>,
    /// How each frame's ticket ended.
    pub tickets: Option<Arc<Mutex<Vec<crate::scene::TicketEnding>>>>,
    /// What each frame build counted.
    pub builds: Option<Arc<Mutex<Vec<BuildRecord>>>>,
    /// Every scene frame's commands wait on this event first (a host-set `VkEvent`), so the frame is held until a test
    /// sets it: M5.5-1's pattern for a frame the GPU has not finished.
    pub frame_event: Option<vk::Event>,
}

/// A record of what the presenter did, shared with the test.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct Observer {
    events: Mutex<Vec<Event>>,
    /// When each event happened, in the same order.
    times: Mutex<Vec<std::time::Instant>>,
    snapshot: Mutex<LedgerSnapshot>,
    /// Frames presented (a present call that queued).
    presents: AtomicU64,
    /// Swap chains created.
    chains_created: AtomicU64,
    quad: Mutex<QuadStats>,
}

impl Observer {
    /// A new observer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push(&self, event: Event) {
        let mut events = lock(&self.events);
        lock(&self.times).push(std::time::Instant::now());
        events.push(event);
    }

    pub(crate) fn set_snapshot(&self, snapshot: LedgerSnapshot) {
        *lock(&self.snapshot) = snapshot;
    }

    pub(crate) fn count_present(&self) {
        self.presents.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn count_chain(&self) {
        self.chains_created.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn quad_texture_began(&self) {
        let mut stats = lock(&self.quad);
        stats.textures_created += 1;
        stats.textures_alive += 1;
        stats.textures_max_alive = stats.textures_max_alive.max(stats.textures_alive);
    }

    pub(crate) fn quad_texture_released(&self) {
        let mut stats = lock(&self.quad);
        stats.textures_released += 1;
        stats.textures_alive = stats.textures_alive.saturating_sub(1);
    }

    pub(crate) fn quad_staging_began(&self) {
        let mut stats = lock(&self.quad);
        stats.staging_created += 1;
        stats.staging_alive += 1;
        stats.staging_max_alive = stats.staging_max_alive.max(stats.staging_alive);
    }

    pub(crate) fn quad_staging_released(&self) {
        let mut stats = lock(&self.quad);
        stats.staging_released += 1;
        stats.staging_alive = stats.staging_alive.saturating_sub(1);
    }

    pub(crate) fn quad_admission_wait(&self) {
        lock(&self.quad).admission_waits += 1;
    }

    pub(crate) fn quad_upload(&self) {
        lock(&self.quad).uploads += 1;
    }

    pub(crate) fn quad_allocation(&self, kind: &'static str, size: u64) {
        lock(&self.quad)
            .allocations
            .push(QuadAllocation { kind, size });
    }

    /// What was counted of the quad objects.
    #[must_use]
    pub fn quad_stats(&self) -> QuadStats {
        lock(&self.quad).clone()
    }

    /// Every event so far, in order.
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        lock(&self.events).clone()
    }

    /// When each event happened, in the same order as [`events`](Self::events).
    #[must_use]
    pub fn event_times(&self) -> Vec<std::time::Instant> {
        lock(&self.times).clone()
    }

    /// The ledger as the thread last published it.
    #[must_use]
    pub fn ledger(&self) -> LedgerSnapshot {
        *lock(&self.snapshot)
    }

    /// Present calls that queued.
    #[must_use]
    pub fn presents(&self) -> u64 {
        self.presents.load(Ordering::SeqCst)
    }

    /// Swap chains created.
    #[must_use]
    pub fn chains_created(&self) -> u64 {
        self.chains_created.load(Ordering::SeqCst)
    }

    /// How many times `event` happened.
    #[must_use]
    pub fn count(&self, event: &Event) -> usize {
        lock(&self.events)
            .iter()
            .filter(|seen| *seen == event)
            .count()
    }
}

/// The faults the tests inject, passed only through `VulkanPresenter::start_with_test_hooks`. Each list is
/// consumed front to back, one entry per call of that operation; a `None` entry lets that call run for real.
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
pub struct TestHooks {
    /// `start` fails before it spawns anything.
    pub fail_start: bool,
    /// Device selection behaves as if the implementation did not advertise this extension.
    pub hide_extension: Option<String>,
    /// Results for the successive acquire calls.
    pub acquire: Vec<Option<vk::Result>>,
    /// Results for the successive submits.
    pub submit: Vec<Option<vk::Result>>,
    /// Results for the successive presents.
    pub present: Vec<Option<vk::Result>>,
    /// Results for the successive swap chain creations.
    pub swapchain: Vec<Option<vk::Result>>,
    /// Acquire an image and give it back unused (release it and drain its semaphore) for this many frames.
    pub abandon_frames: u32,
    /// While this reads true, every fence reads as not signalled (a fence that never proves anything).
    pub fences_never_signal: Option<Arc<AtomicBool>>,
    /// Results for the successive capability queries that decide something in the thread (whether a suboptimal
    /// result needs a rebuild, and whether a rebuild can run); the queries of setup and of the creation itself
    /// are not injected.
    pub capability_query: Vec<Option<vk::Result>>,
    /// The surface's `currentExtent` as the thread sees it, in place of the real one while this holds a value
    /// (a surface whose configuration changed: it must lie within the real minimum and maximum).
    pub surface_extent: Option<SurfaceExtent>,
    /// While this reads true, only the present fences read as not signalled (submit and drain fences
    /// signal as usual): the presentation engine seems never to finish, so the pools run empty.
    pub present_fences_held: Option<Arc<AtomicBool>>,
    /// The thread waits here, while the pause is held and a swap chain is retired, after an idle wake and
    /// before it services the fences: a legal interleaving of a stop with the idle service, chosen
    /// deterministically.
    pub pause_before_idle_service: Option<Arc<Pause>>,
    /// While this reads true (read on the render thread), the acquire is not called (a blocked acquisition).
    pub hold_acquire: Option<Arc<AtomicBool>>,
    /// A gate on frame progress.
    pub gate: Option<Arc<Gate>>,
    /// Counts the frames presented.
    pub frames: Option<Arc<AtomicU64>>,
    /// Records the colour of every frame presented.
    pub clear_log: Option<Arc<Mutex<Vec<[f32; 3]>>>>,
    /// The observer, shared with the test.
    pub observer: Option<Arc<Observer>>,
    /// Set when the render thread starts.
    pub thread_started: Option<Arc<AtomicBool>>,
    /// The render thread panics after this many presented frames.
    pub panic_after_frames: Option<u64>,
    /// The render thread panics as the teardown begins.
    pub panic_at_stop: bool,
    /// What the forced panics (`panic_after_frames`, `panic_at_stop`) raise, in place of a plain message: a
    /// function that panics with any payload, such as one whose `Drop` panics.
    pub forced_panic_payload: Option<fn() -> !>,
    /// The stop budget (default 1.5 s).
    pub stop_budget: Option<Duration>,
    /// The quad slot to start with (what [`VulkanPresenter::start_2d`](crate::VulkanPresenter::start_2d)
    /// passes), for the tests that start through the hidden constructor.
    pub quad_slot: Option<Arc<VulkanQuadSlot>>,
    /// The surface formats the thread sees: a **subset of what the real surface offers** (never an invented
    /// format). The swap chain's format is chosen from the formats that remain; the offered list and the choice are
    /// recorded as [`Event::Formats`].
    pub forced_surface_formats: Option<Vec<vk::Format>>,
    /// The `maxImageDimension2D` the quad's device checks see on the render thread, in place of the device's own.
    pub quad_image_limit: Option<u32>,
    /// Faults in creating the quad's objects.
    pub quad_faults: QuadFaults,
    /// The drawn frames are copied here (the swap chain's images get `TRANSFER_SRC` usage): the pixels the surface
    /// path drew, handed over when each frame's submit is proven.
    pub capture_frames: Option<crate::capture::FrameCaptures>,
    /// Acquire, record (upload included) and then give the image back unused, for this many frames that have a
    /// quad to draw: the abandon comes after the recording and before the submit.
    pub abandon_after_record: u32,
    /// The scene presenter's hooks.
    pub scene: SceneHooks,
}

impl TestHooks {
    /// Takes the next entry of `list`, if any is left.
    pub(crate) fn next(list: &mut Vec<Option<vk::Result>>) -> Option<vk::Result> {
        if list.is_empty() {
            None
        } else {
            list.remove(0)
        }
    }
}
