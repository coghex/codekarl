//! The texture scene of a presenter: the render thread's ledger, the driver that owns the logical side, and the
//! frame tickets the presenter ends according to each frame's fate.
//!
//! The presenter (`presenter.rs`) knows none of the key type: everything it needs of a scene is the private
//! [`Scene`] trait, which [`SceneSetup`] builds on the render thread, during setup, from the caller's context,
//! limits and [`TextureDriver`].

use crate::hash::FrameMap;
use std::cell::RefCell;
use std::ffi::c_void;
use std::hash::Hash;
use std::mem::size_of;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use crate::Error;
use crate::batch::{
    Batch, CompatDecision, FrameCounters, InstanceRecord, Layout, Resolved, build_scene_in,
};
use crate::bindless::BindlessLimits;
use crate::frame::{MetalFrame, MetalFrameSlot, MetalSceneSource};
use crate::ledger::{FrameTexture, LedgerEvent, LedgerLimits, LedgerProof, TextureLedger};
use crate::quad::{DestinationRect, MetalFilter, MetalScene, SlotWake};
use crate::textures::{
    BaselineUpload, MetalContext, TestHooks as TextureTestHooks, TextureLimits, TextureWakes,
};

/// The side of the checker texture, in pixels.
pub const CHECKER_SIDE: u32 = 64;
/// The side of one checker cell, in pixels.
pub const CHECKER_CELL: u32 = 8;

/// The checker's pixels (RGBA8, rows top first): 8 x 8-pixel cells of opaque magenta `(255, 0, 255, 255)` and opaque black
/// `(0, 0, 0, 255)`, the top-left cell magenta (design §5.2). Opaque, so premultiplication changes nothing.
#[must_use]
pub fn checker_pixels() -> Vec<u8> {
    let side = CHECKER_SIDE as usize;
    let cell = CHECKER_CELL as usize;
    let mut pixels = Vec::with_capacity(side * side * 4);
    for row in 0..side {
        for column in 0..side {
            let magenta = (row / cell + column / cell).is_multiple_of(2);
            pixels.extend_from_slice(if magenta {
                &[255, 0, 255, 255]
            } else {
                &[0, 0, 0, 255]
            });
        }
    }
    pixels
}

/// The bounds of a scene presenter's instance buffer: the most quads a frame draws, and the budget its measured
/// allocation is charged to (`[render] max_quads` and `binding_geometry_bytes` of the configuration).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneLimits {
    /// Instances the buffer holds; a source emitting more draws the first `max_quads` in painter order and the rest are
    /// counted as dropped.
    pub max_quads: usize,
    /// The budget for the GPU tables and instance/upload control buffers: the instance buffer's measured allocation must
    /// fit it, or the presenter's setup is refused.
    pub binding_geometry_bytes: u64,
}

/// Where a driver pass happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    /// An ordinary pass: texture work woke the render thread (an upload completed, room appeared, the driver's
    /// owner asked for a pass).
    Pass,
    /// The frame boundary: immediately before a frame is built. Adoption (`TextureLedger::adopt`) happens here and
    /// only here.
    Frame,
    /// The last pass, at the start of the presenter's teardown: the driver detaches whatever feeds it. No pass
    /// follows it.
    Ending,
}

/// What a driver pass reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PassOutcome {
    /// The pass stopped at a per-pass bound with **runnable** work left (more retirements or events to relay): the
    /// presenter runs another ordinary pass without waiting for a wake. Work blocked on room or a completion is not
    /// runnable: report `false`, and the store's `room` or `completed` wake resumes it.
    pub more: bool,
}

/// The logical side of the textures, called on the render thread with the ledger: offering uploads, relaying
/// commit and reject, calling `adopt` at a boundary and acknowledging adoptions, relaying retirements.
///
/// A pass **must not block** (no waiting for a lock another thread may hold across a wait, a completion or the
/// presenter), and is never called with a lock of the presenter held.
pub trait TextureDriver<K: Copy + Eq + Hash + Send + 'static>: Send {
    /// One pass. `events` are the ledger's logical events (prepared, upload failed, key retired) that
    /// [`TextureLedger::pump`] produced just before this pass, handed over by value: the presenter takes every
    /// `GpuFailure` itself first, so none is in `events`, and no event has two consumers.
    fn pass(
        &mut self,
        ledger: &mut TextureLedger<K>,
        boundary: Boundary,
        events: Vec<LedgerEvent<K>>,
    ) -> PassOutcome;
}

/// How a frame's ticket ended (test evidence; see `TestHooks::ticket_log`).
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketEnding {
    /// The frame's command buffer was committed; the ticket waits for its completion token.
    Submitted,
    /// The frame was abandoned before its token was prepared.
    Discarded,
    /// The frame was abandoned after its token was prepared and before the commit.
    NotCommitted,
    /// The ledger refused an operation of the ticket (never expected: `TicketBusy` must not happen).
    Refused(String),
}

/// The upload store's cumulative counters, as the presenter publishes them in its scene stats.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct UploadCounts {
    pub(crate) textures_created: u64,
    pub(crate) uploads_submitted: u64,
    pub(crate) uploads_completed: u64,
    pub(crate) texture_bytes_peak: u64,
    pub(crate) staging_bytes_peak: u64,
    /// Revisions the render thread adopted (became ready) and when the first and the latest adopting boundary ran.
    pub(crate) adopted: u64,
    pub(crate) first_adopted_ns: u64,
    pub(crate) last_adopted_ns: u64,
}

/// What the ordinary pump found.
pub(crate) struct Pumped {
    /// The first `GpuFailure` message the ledger produced; the presenter publishes it **before** the driver runs.
    pub(crate) failure: Option<String>,
    /// Runnable work remains in the ledger's own release backlog.
    pub(crate) more: bool,
}

/// The ticket of the frame in progress, as the native frame step reaches it.
pub(crate) trait FrameSeam {
    /// The native token the frame's completion handler completes before it commits; `Err` is the ledger's refusal.
    fn prepare(&mut self) -> Result<*mut c_void, String>;
    /// Builds the scene's draw list into the instance buffer, resolving every key through this frame's ticket (which pins
    /// what it reads). `None` when the frame has no scene source. The returned pointers stay valid until the seam ends.
    fn build(&mut self, request: &BuildRequest) -> Option<BuiltFrame>;
}

/// Where and how much a frame may write.
pub(crate) struct BuildRequest {
    /// The drawable's current size (informational for a source that carries its own metrics).
    pub(crate) size: (u32, u32),
    /// The instance buffer's contents. The caller proved the previous frame's command buffer completed before this call.
    pub(crate) buffer: *mut InstanceRecord,
    /// The instances the buffer holds.
    pub(crate) capacity: usize,
    /// The most instances one draw takes in the bindless path (`u32::MAX`: only the buffer's capacity limits a draw).
    pub(crate) chunk: u32,
    /// Tests only: the `n`-th drawn instance's slot is forced out of the table (bindless path).
    pub(crate) force_out_of_range_at: Option<usize>,
}

/// A scene frame's draw list, ready for the native step.
pub(crate) struct BuiltFrame {
    pub(crate) batches: *const Batch,
    pub(crate) batch_count: u32,
    /// 1 for a linear frame, else 0.
    pub(crate) filter: i32,
    /// The checker's Metal texture.
    pub(crate) checker: *mut c_void,
    pub(crate) counters: FrameCounters,
    /// The frame's pinned bindless table (an `id<MTLBuffer>` of resource IDs; null for the ordinary path) and its entry count.
    pub(crate) table: *mut c_void,
    pub(crate) table_slots: u32,
    /// The initial gate (milestone 5, issue 5d-2): the compatibility quad has no image to draw and none was drawn before, so
    /// this frame must not be drawn; the presenter ends its ticket discarded and waits (see [`FrameGate`]).
    pub(crate) gated: bool,
}

/// Whether the presenter may attempt a frame now, as the scene sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameGate {
    /// A frame may be attempted: no frame is held, or the held one can make progress (the gating key has a revision to
    /// adopt or draw), or the frame published since has another compatibility key.
    Open,
    /// The last frame was held by the initial gate and nothing it waits for has appeared: no frame step runs, and the
    /// thread sleeps on its normal condition until a commit or a newer frame changes the answer.
    Waiting,
}

/// What the scene counts about the compatibility quad, for the presenter's stats.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CompatStats {
    pub(crate) gate_holds: u64,
    pub(crate) gate_longest_hold_micros: u64,
    pub(crate) held_frames: u64,
    pub(crate) held_revision: u64,
    pub(crate) upload_failures: u64,
}

/// How far the checker's upload is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckerProgress {
    /// Its completion has not arrived: no frame is committed yet.
    Pending,
    /// Resident: frames may be taken.
    Resident,
    /// Its upload failed (at once, or by a completion error).
    Failed(String),
}

/// The baseline bytes of a scene: the checker's measured texture and staging buffer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Baseline {
    pub(crate) texture_bytes: u64,
    pub(crate) staging_bytes: u64,
    pub(crate) residency_set_allocation: u64,
    pub(crate) bindless_table_allocation: u64,
    /// The entry count of each bindless table (zero without bindless limits).
    pub(crate) bindless_slots: u32,
}

/// What the presenter needs of a scene. Owned and used on the render thread only.
pub(crate) trait Scene {
    /// The latest frame the renderer published: its generation and legacy quad. The rest (the scene source and the
    /// filter) is held for this attempt's [`FrameSeam::build`].
    fn read_frame(&mut self) -> MetalScene;
    /// How far the checker's upload is (one non-blocking look).
    fn checker(&mut self) -> CheckerProgress;
    /// The checker's measured bytes, charged once to the presenter's baseline.
    fn baseline(&self) -> Baseline;
    /// The ordinary `pump` (submits and polls uploads, reaps the frame ticket, releases what was retired). Its
    /// logical events are held for the next [`drive`](Self::drive); a `GpuFailure` among them is returned instead.
    fn pump(&mut self) -> Pumped;
    /// One driver pass at `boundary` with the events the last pump held (none after
    /// [`observe`](Self::observe)); returns whether the driver has runnable work left.
    fn drive(&mut self, boundary: Boundary) -> PassOutcome;
    /// A **non-destructive** look for a GPU failure, for the teardown (the `Ending` pass included): it submits,
    /// polls, reaps and releases nothing, so nothing device-dependent is dropped before the combined proof.
    /// Whether committed revisions wait for a frame boundary.
    fn has_committed(&self) -> bool;
    /// The upload store's cumulative counters (constant work).
    fn uploads(&self) -> UploadCounts;
    /// Whether a frame may be attempted now (see [`FrameGate`]): re-evaluated from the facts on every call, never from a
    /// remembered edge.
    fn frame_gate(&mut self) -> FrameGate;
    /// The compatibility quad's counters.
    fn compat_stats(&self) -> CompatStats;
    /// Whether release work is waiting in the ledger (see `TextureLedger::has_release_work`).
    fn has_release_work(&self) -> bool;
    /// One frame step inside a ticket. `run` returns the frame's result code and whether its command buffer was
    /// committed; the ticket ends accordingly. `None` when no ticket could begin (nothing was run).
    fn frame(&mut self, run: &mut dyn FnMut(&mut dyn FrameSeam) -> (i32, bool)) -> Option<i32>;
    fn observe(&mut self) -> Option<String>;
    /// A non-destructive, non-waiting look at the whole proof (stops offering). `None` when complete; otherwise what
    /// is outstanding.
    fn prove(&mut self) -> Option<String>;
    /// Releases the store's and ledger's objects with the complete proof the last `prove` kept.
    fn release(&mut self) -> Result<(), String>;
}

/// Builds a scene on the render thread.
pub(crate) trait SceneStart: Send + Sync {
    fn context(&self) -> Arc<MetalContext>;
    fn start(
        &self,
        wakes: TextureWakes,
        log: Option<Arc<Mutex<Vec<TicketEnding>>>>,
    ) -> Result<Box<dyn Scene>, Error>;
    /// The instance buffer's bounds.
    fn limits(&self) -> SceneLimits;
    /// Registers the presenter's wake on the frame slot; a published frame wants a frame.
    fn register_wake(&self, wake: SlotWake);
    /// Forgets the wake (the presenter ended).
    fn clear_wake(&self);
}

/// What a scene presenter is started with: the caller's context, the store's and ledger's limits, the texture
/// driver, the frame slot the renderer publishes to and the instance buffer's bounds, and (tests only) the store's and
/// the checker upload's fault hooks. The render thread starts the store and ledger on the context during setup and
/// uploads the checker.
#[doc(hidden)]
#[derive(Clone)]
pub struct SceneSetup(pub(crate) Arc<dyn SceneStart>);

impl std::fmt::Debug for SceneSetup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SceneSetup")
    }
}

impl SceneSetup {
    /// A scene over `context`. `texture_hooks` are the store's fault switches and `checker_hooks` those of the checker's
    /// own upload (tests only; `None` in production).
    #[expect(
        clippy::too_many_arguments,
        reason = "the context, the three limits, the driver, the frame slot and the two hook sets the specification names"
    )]
    pub fn new<K, D>(
        context: Arc<MetalContext>,
        texture_limits: TextureLimits,
        ledger_limits: LedgerLimits,
        limits: SceneLimits,
        driver: D,
        frames: Arc<MetalFrameSlot<K>>,
        texture_hooks: Option<TextureTestHooks>,
        checker_hooks: Option<TextureTestHooks>,
    ) -> Self
    where
        K: Copy + Eq + Hash + Send + 'static,
        D: TextureDriver<K> + 'static,
    {
        Self::new_with_bindless_limits(
            context,
            texture_limits,
            ledger_limits,
            None,
            limits,
            driver,
            frames,
            texture_hooks,
            checker_hooks,
        )
    }

    /// A scene over `context` with the bindless residency path enabled.
    #[expect(
        clippy::too_many_arguments,
        reason = "the context, the three limits, the driver, the frame slot and the two hook sets the specification names"
    )]
    pub fn new_with_bindless_limits<K, D>(
        context: Arc<MetalContext>,
        texture_limits: TextureLimits,
        ledger_limits: LedgerLimits,
        bindless_limits: Option<BindlessLimits>,
        limits: SceneLimits,
        driver: D,
        frames: Arc<MetalFrameSlot<K>>,
        texture_hooks: Option<TextureTestHooks>,
        checker_hooks: Option<TextureTestHooks>,
    ) -> Self
    where
        K: Copy + Eq + Hash + Send + 'static,
        D: TextureDriver<K> + 'static,
    {
        Self(Arc::new(Parts {
            context,
            texture_limits,
            ledger_limits,
            bindless_limits,
            limits,
            driver: Mutex::new(Some(driver)),
            frames,
            texture_hooks,
            checker_hooks,
        }))
    }
}

struct Parts<K, D> {
    context: Arc<MetalContext>,
    texture_limits: TextureLimits,
    ledger_limits: LedgerLimits,
    bindless_limits: Option<BindlessLimits>,
    limits: SceneLimits,
    driver: Mutex<Option<D>>,
    frames: Arc<MetalFrameSlot<K>>,
    texture_hooks: Option<TextureTestHooks>,
    checker_hooks: Option<TextureTestHooks>,
}

impl<K, D> SceneStart for Parts<K, D>
where
    K: Copy + Eq + Hash + Send + 'static,
    D: TextureDriver<K> + 'static,
{
    fn context(&self) -> Arc<MetalContext> {
        Arc::clone(&self.context)
    }

    fn start(
        &self,
        wakes: TextureWakes,
        log: Option<Arc<Mutex<Vec<TicketEnding>>>>,
    ) -> Result<Box<dyn Scene>, Error> {
        let driver = self
            .driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(|| Error("the scene was already started".to_owned()))?;
        let context = Arc::clone(&self.context);
        let ledger = if let Some(bindless_limits) = self.bindless_limits {
            let mut hooks = self.texture_hooks.clone().unwrap_or_default();
            if let Some(checker_hooks) = &self.checker_hooks {
                hooks.force_bindless_tier1 = checker_hooks.force_bindless_tier1;
                hooks.fail_residency_creation = checker_hooks.fail_residency_creation;
            }
            TextureLedger::start_bindless(
                context,
                self.texture_limits,
                self.ledger_limits,
                bindless_limits,
                self.limits.binding_geometry_bytes,
                u64::try_from(self.limits.max_quads)
                    .ok()
                    .and_then(|quads| {
                        u64::try_from(size_of::<InstanceRecord>())
                            .ok()
                            .and_then(|bytes| quads.checked_mul(bytes))
                    })
                    .ok_or_else(|| Error("instance buffer byte count overflow".to_owned()))?,
                wakes.clone(),
                hooks,
            )?
        } else {
            match &self.texture_hooks {
                Some(hooks) => TextureLedger::start_with_test_hooks(
                    context,
                    self.texture_limits,
                    self.ledger_limits,
                    wakes.clone(),
                    hooks.clone(),
                ),
                None => TextureLedger::start(
                    context,
                    self.texture_limits,
                    self.ledger_limits,
                    wakes.clone(),
                ),
            }?
        };
        // The most distinct keys one frame resolves: an instance for each of the `max_quads` it has room for, plus the compatibility
        // key and its held revision. The ledger keeps a ticket's map for the next frame only within this bound (CK-148); the setting
        // is made once, before any frame, so a refusal is a programming error.
        ledger
            .set_spare_limit(self.limits.max_quads.saturating_add(2))
            .map_err(|error| Error(format!("the ledger's spare limit was refused: {error:?}")))?;
        // The checker's upload, on the context's queue, with its own completion token. Its completion is the store's
        // `completed` wake: it alone runs the first frame. A failure to start it is not a setup error here (the store
        // and ledger are running and need the combined proof): it is the checker's state, which the presenter turns into
        // its one `Failed` report before any frame.
        let checker_hooks = self.checker_hooks.clone().unwrap_or_default();
        let (checker, checker_failure) = match BaselineUpload::start(
            &self.context,
            CHECKER_SIDE,
            CHECKER_SIDE,
            &checker_pixels(),
            wakes.completed.clone(),
            &checker_hooks,
        ) {
            Ok(upload) => (Some(upload), None),
            Err(error) => (
                None,
                Some(format!(
                    "the checker texture could not be uploaded: {error}"
                )),
            ),
        };
        Ok(Box::new(SceneState {
            ledger,
            driver,
            held: Vec::new(),
            proof: None,
            log,
            frames: Arc::clone(&self.frames),
            current: None,
            checker,
            checker_failure,
            checker_resident: false,
            scratch: Some(Scratch::default()),
            drawn: None,
            blocked: None,
            compat: CompatStats::default(),
        }))
    }

    fn limits(&self) -> SceneLimits {
        self.limits
    }

    fn register_wake(&self, wake: SlotWake) {
        self.frames.register_wake(wake);
    }

    fn clear_wake(&self) {
        self.frames.clear_wake();
    }
}

/// The per-frame working storage a seam borrows and gives back, so a frame allocates nothing once warm.
struct Scratch<K> {
    resolved: FrameMap<K, Option<Resolved>>,
    batches: Vec<Batch>,
}

impl<K> Default for Scratch<K> {
    fn default() -> Self {
        Self {
            resolved: FrameMap::default(),
            batches: Vec::new(),
        }
    }
}

struct SceneState<K: Copy + Eq + Hash + Send + 'static, D> {
    ledger: TextureLedger<K>,
    driver: D,
    /// The logical events the last pump produced, for the next driver pass.
    held: Vec<LedgerEvent<K>>,
    proof: Option<LedgerProof>,
    log: Option<Arc<Mutex<Vec<TicketEnding>>>>,
    /// The renderer's latest-value slot of frames.
    frames: Arc<MetalFrameSlot<K>>,
    /// The frame read for the attempt in progress.
    current: Option<MetalFrame<K>>,
    /// The checker's upload, until it is released with the proof.
    checker: Option<BaselineUpload>,
    /// Why the checker could not be uploaded or its upload failed.
    checker_failure: Option<String>,
    checker_resident: bool,
    /// Taken by the seam of a frame and put back after it.
    scratch: Option<Scratch<K>>,
    /// The last compatibility image a committed frame drew, with the rectangle and source it was drawn at. The ledger holds
    /// its revision for as long as this is set.
    drawn: Option<DrawnCompat<K>>,
    /// The frame held back by the initial gate, if any: the compatibility key it waits for and when the wait began.
    blocked: Option<Blocked<K>>,
    compat: CompatStats,
}

/// The compatibility image a frame drew.
#[derive(Clone, Copy)]
struct DrawnCompat<K> {
    key: K,
    revision: u64,
    destination: DestinationRect,
    source: [f32; 4],
}

/// A frame held by the initial gate.
struct Blocked<K> {
    key: K,
    since: Instant,
}

/// What a frame did with the compatibility quad.
#[derive(Clone, Copy)]
enum CompatOutcome<K> {
    /// No compatibility instance was drawn (none placed, or none in the frame).
    Absent,
    /// The current image resolved and was drawn.
    Drew(DrawnCompat<K>),
    /// The current image did not resolve; the held one was drawn in its place.
    DrewHeld,
    /// Neither resolved: the frame is held back.
    Gated(K),
}

impl<K: Copy + Eq + Hash + Send + 'static, D> SceneState<K, D> {
    /// What a frame's ending means for the compatibility quad. Called after the ticket ended; a frame that was only
    /// *attempted* (not committed) never changes what was last drawn.
    fn note_compat(&mut self, outcome: &CompatOutcome<K>, committed: bool) {
        match *outcome {
            CompatOutcome::Gated(key) => {
                // Held back: the frame was not drawn. The wait started with the first held attempt.
                let since = self.blocked.take().map_or_else(Instant::now, |b| b.since);
                self.blocked = Some(Blocked { key, since });
                self.compat.gate_holds += 1;
            }
            CompatOutcome::Drew(drawn) if committed => {
                if let Some(blocked) = self.blocked.take() {
                    let micros =
                        u64::try_from(blocked.since.elapsed().as_micros()).unwrap_or(u64::MAX);
                    self.compat.gate_longest_hold_micros =
                        self.compat.gate_longest_hold_micros.max(micros);
                }
                // The image just drawn is the one kept on screen: hold its revision (a newer hold releases the older).
                if self.ledger.hold_compat(drawn.key, drawn.revision).is_ok() {
                    self.drawn = Some(drawn);
                }
            }
            CompatOutcome::DrewHeld if committed => {
                self.compat.held_frames += 1;
            }
            CompatOutcome::Absent if committed => {
                // A drawn frame that drew nothing for the compatibility quad (no entry, or an entry that resolved and was clipped
                // away) clears what was kept.
                self.drawn = None;
                self.blocked = None;
                self.ledger.release_compat_hold();
            }
            _ => {}
        }
    }

    fn record(&self, ending: TicketEnding) {
        if let Some(log) = &self.log {
            log.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(ending);
        }
    }
}

/// The scene a frame draws: its source, its filter and the checker, with the textures this frame resolved (each keeps the
/// frame's ticket borrowed, so it cannot outlive the ticket).
struct SceneInput<'t, K> {
    source: Arc<dyn MetalSceneSource<K>>,
    filter: MetalFilter,
    checker: *mut c_void,
    /// The frame's compatibility key (plain data, whether or not its quad placed anything).
    compat_key: Option<K>,
    textures: Vec<FrameTexture<'t>>,
}

/// The ticket of one frame step, as the native step sees it.
struct TicketSeam<'t, 'a, K: Copy + Eq + Hash + Send + 'static> {
    ticket: &'t crate::ledger::FrameTicket<'a, K>,
    prepared: bool,
    refused: Option<String>,
    scene: Option<SceneInput<'t, K>>,
    scratch: Scratch<K>,
    /// The image the last committed frame drew for the compatibility quad (the one to keep on screen).
    drawn: Option<DrawnCompat<K>>,
    /// What this frame did with the compatibility quad.
    outcome: CompatOutcome<K>,
}

impl<K: Copy + Eq + Hash + Send + 'static> FrameSeam for TicketSeam<'_, '_, K> {
    #[expect(
        clippy::too_many_lines,
        reason = "one frame build: the layout, both resolvers and the compatibility decisions read best together"
    )]
    fn build(&mut self, request: &BuildRequest) -> Option<BuiltFrame> {
        let ticket = self.ticket;
        let scene = self.scene.as_mut()?;
        let SceneInput {
            source,
            filter,
            checker,
            compat_key,
            textures,
        } = scene;
        let compat_key = *compat_key;
        let Scratch { resolved, batches } = &mut self.scratch;
        let drawn = self.drawn;
        let outcome = &mut self.outcome;
        let buffer = request.buffer;
        let capacity = request.capacity;
        // A frame with a pinned table draws through it: one run, the epoch pin alone, no revision pinned. A scene without
        // bindless limits (the hidden ordinary option) keeps CK-116's per-texture batches and pins.
        let table = ticket.bindless();
        let layout = table.map_or(Layout::Ordinary, |table| Layout::Bindless {
            texture_slots: table.slots,
            chunk: request.chunk,
            force_out_of_range_at: request.force_out_of_range_at,
        });
        // Both resolvers keep the textures they resolve (each borrows the ticket).
        let kept = RefCell::new(std::mem::take(textures));
        let built = build_scene_in(
            layout,
            &**source,
            request.size,
            capacity,
            &mut |slot, record| {
                if slot < capacity {
                    // SAFETY: `slot` is below the buffer's capacity, so the offset stays inside the one allocation the
                    // caller sized for `capacity` instances.
                    let target = unsafe { buffer.add(slot) };
                    // SAFETY: `target` is in bounds; the caller proved the previous frame's command buffer completed, so
                    // the GPU is not reading the buffer, and this is the only thread that writes it. The write
                    // initialises the slot.
                    unsafe { target.write(record) };
                }
            },
            &mut |key| {
                if table.is_some() {
                    return ticket.resolve_slot(key).map(|(slot, _)| Resolved {
                        handle: std::ptr::null_mut(),
                        slot,
                    });
                }
                ticket.resolve(key).map(|texture| {
                    let handle = texture.metal();
                    kept.borrow_mut().push(texture);
                    Resolved { handle, slot: 0 }
                })
            },
            compat_key.is_some(),
            &mut |instance| {
                // The current image if it resolves; otherwise the last one drawn, at the rectangle it was drawn at;
                // otherwise the frame is held back. Never the checker.
                let Some(instance) = instance else {
                    // The compatibility entry placed to nothing (clipped away). While its image is pending the last image
                    // drawn stays where it was drawn; once it resolves its own placement rules, which is to draw nothing.
                    let Some(key) = compat_key else {
                        return CompatDecision::Skip;
                    };
                    let resolved_now = if table.is_some() {
                        ticket.resolve_slot(key).is_some()
                    } else if let Some(texture) = ticket.resolve(key) {
                        kept.borrow_mut().push(texture);
                        true
                    } else {
                        false
                    };
                    if resolved_now {
                        // Resolved and placed to nothing: nothing is drawn, the held image is not needed any more (the
                        // `Absent` ending releases it).
                        return CompatDecision::Skip;
                    }
                    if let Some(held) = drawn {
                        if table.is_some() {
                            if let Some(slot) =
                                ticket.resolve_revision_slot(held.key, held.revision)
                            {
                                *outcome = CompatOutcome::DrewHeld;
                                return CompatDecision::DrawSlot {
                                    slot,
                                    destination: held.destination,
                                    source: held.source,
                                };
                            }
                        } else if let Some(texture) =
                            ticket.resolve_revision(held.key, held.revision)
                        {
                            let handle = texture.metal();
                            kept.borrow_mut().push(texture);
                            *outcome = CompatOutcome::DrewHeld;
                            return CompatDecision::Draw {
                                texture: handle,
                                destination: held.destination,
                                source: held.source,
                            };
                        }
                    }
                    return CompatDecision::Skip;
                };
                if table.is_some() {
                    if let Some((slot, revision)) = ticket.resolve_slot(instance.key) {
                        *outcome = CompatOutcome::Drew(DrawnCompat {
                            key: instance.key,
                            revision,
                            destination: instance.destination,
                            source: instance.source,
                        });
                        return CompatDecision::DrawSlot {
                            slot,
                            destination: instance.destination,
                            source: instance.source,
                        };
                    }
                    if let Some(held) = drawn
                        && let Some(slot) = ticket.resolve_revision_slot(held.key, held.revision)
                    {
                        *outcome = CompatOutcome::DrewHeld;
                        return CompatDecision::DrawSlot {
                            slot,
                            destination: held.destination,
                            source: held.source,
                        };
                    }
                    *outcome = CompatOutcome::Gated(instance.key);
                    return CompatDecision::Gate;
                }
                if let Some(texture) = ticket.resolve(instance.key) {
                    let (handle, revision) = (texture.metal(), texture.revision());
                    kept.borrow_mut().push(texture);
                    *outcome = CompatOutcome::Drew(DrawnCompat {
                        key: instance.key,
                        revision,
                        destination: instance.destination,
                        source: instance.source,
                    });
                    return CompatDecision::Draw {
                        texture: handle,
                        destination: instance.destination,
                        source: instance.source,
                    };
                }
                if let Some(held) = drawn
                    && let Some(texture) = ticket.resolve_revision(held.key, held.revision)
                {
                    let handle = texture.metal();
                    kept.borrow_mut().push(texture);
                    *outcome = CompatOutcome::DrewHeld;
                    return CompatDecision::Draw {
                        texture: handle,
                        destination: held.destination,
                        source: held.source,
                    };
                }
                *outcome = CompatOutcome::Gated(instance.key);
                CompatDecision::Gate
            },
            resolved,
            batches,
        );
        *textures = kept.into_inner();
        Some(BuiltFrame {
            batches: batches.as_ptr(),
            batch_count: u32::try_from(batches.len()).unwrap_or(u32::MAX),
            filter: i32::from(*filter == MetalFilter::Linear),
            checker: *checker,
            counters: built.counters,
            table: table.map_or(std::ptr::null_mut(), |table| table.buffer),
            table_slots: table.map_or(0, |table| table.slots),
            gated: built.gated,
        })
    }

    fn prepare(&mut self) -> Result<*mut c_void, String> {
        match self.ticket.prepare_native() {
            Ok(token) => {
                self.prepared = true;
                Ok(token)
            }
            Err(error) => {
                let text = format!("{error:?}");
                self.refused = Some(text.clone());
                Err(text)
            }
        }
    }
}

impl<K, D> Scene for SceneState<K, D>
where
    K: Copy + Eq + Hash + Send + 'static,
    D: TextureDriver<K>,
{
    fn read_frame(&mut self) -> MetalScene {
        let frame = self.frames.latest();
        // Each newer frame gives the ledger its current compatibility key (one slot), so a failed upload of it is also the
        // presenter's, whatever was placed.
        self.ledger.set_compat_key(frame.compat_key().copied());
        let scene = MetalScene::new(frame.generation(), frame.quad().cloned());
        self.current = Some(frame);
        scene
    }

    fn checker(&mut self) -> CheckerProgress {
        if let Some(reason) = &self.checker_failure {
            return CheckerProgress::Failed(reason.clone());
        }
        if self.checker_resident {
            return CheckerProgress::Resident;
        }
        let Some(upload) = &self.checker else {
            return CheckerProgress::Failed("the checker texture is missing".to_owned());
        };
        if !upload.is_done() {
            return CheckerProgress::Pending;
        }
        match upload.status() {
            Ok(()) => {
                if let Some(upload) = &self.checker
                    && let Err(error) = self
                        .ledger
                        .bind_checker(upload.metal(), upload.resource_id())
                {
                    return CheckerProgress::Failed(format!(
                        "bindless checker table initialization failed: {error}"
                    ));
                }
                self.checker_resident = true;
                CheckerProgress::Resident
            }
            Err(error) => {
                let reason = format!("the checker texture upload failed: {error}");
                self.checker_failure = Some(reason.clone());
                CheckerProgress::Failed(reason)
            }
        }
    }

    fn baseline(&self) -> Baseline {
        self.checker
            .as_ref()
            .map_or_else(Baseline::default, |upload| Baseline {
                texture_bytes: upload.texture_bytes(),
                staging_bytes: upload.staging_bytes(),
                residency_set_allocation: self.ledger.residency_set_allocation(),
                bindless_table_allocation: self.ledger.table_allocation(),
                bindless_slots: self.ledger.table_slots(),
            })
    }

    fn pump(&mut self) -> Pumped {
        // The key is marked before the pump that may see its failure: an upload failing before any frame was built is
        // classified all the same.
        let marked = self.frames.latest().compat_key().copied();
        self.ledger.set_compat_key(marked);
        let mut events = Vec::new();
        let more = self.ledger.pump(&mut events);
        let mut failure = None;
        let mut compat_failures = 0_u64;
        events.retain(|event| match event {
            LedgerEvent::GpuFailure { message } => {
                failure.get_or_insert_with(|| message.clone());
                false
            }
            // The presenter's observation of a compatibility upload failure; the driver keeps the ordinary
            // `UploadFailed` of the same failure (the service's status shows it).
            LedgerEvent::CompatUploadFailed { reason, .. } => {
                compat_failures += 1;
                failure.get_or_insert_with(|| {
                    format!("the legacy quad's image upload failed: {reason}")
                });
                false
            }
            _ => true,
        });
        self.compat.upload_failures += compat_failures;
        self.held.extend(events);
        Pumped { failure, more }
    }

    fn frame_gate(&mut self) -> FrameGate {
        let Some(blocked) = &self.blocked else {
            return FrameGate::Open;
        };
        // A newer frame with another compatibility key (replaced, or cleared) re-evaluates; so does the gating key
        // having something to adopt or draw. Both are facts read now, so no wake can be lost.
        let latest = self.frames.latest();
        if latest.compat_key().copied() != Some(blocked.key)
            || self.ledger.has_committed_or_active(blocked.key)
        {
            FrameGate::Open
        } else {
            FrameGate::Waiting
        }
    }

    fn has_release_work(&self) -> bool {
        self.ledger.has_release_work()
    }

    fn compat_stats(&self) -> CompatStats {
        CompatStats {
            held_revision: self
                .ledger
                .compat_hold()
                .map_or(0, |(_, revision)| revision),
            ..self.compat
        }
    }

    fn drive(&mut self, boundary: Boundary) -> PassOutcome {
        let events = std::mem::take(&mut self.held);
        self.driver.pass(&mut self.ledger, boundary, events)
    }

    fn observe(&mut self) -> Option<String> {
        // The checker's outcome too: a completion token being done proves it is safe to release, not that the upload
        // worked, so an error completion that arrives while the presenter is stopping (or being proven) is still a failure,
        // reported before anything is released. The presenter's exactly-once latch keeps it from being reported twice.
        let ledger = self.ledger.observe_failure();
        match self.checker() {
            CheckerProgress::Failed(reason) => ledger.or(Some(reason)),
            CheckerProgress::Pending | CheckerProgress::Resident => ledger,
        }
    }

    fn has_committed(&self) -> bool {
        self.ledger.has_committed()
    }

    fn uploads(&self) -> UploadCounts {
        let stats = self.ledger.upload_stats();
        let (adopted, first_adopted_ns, last_adopted_ns) = self.ledger.adoption_counts();
        UploadCounts {
            adopted,
            first_adopted_ns,
            last_adopted_ns,
            textures_created: stats.textures_created,
            uploads_submitted: stats.batches_submitted,
            uploads_completed: stats.batches_completed,
            texture_bytes_peak: stats.texture_bytes_peak,
            staging_bytes_peak: stats.staging_bytes_peak,
        }
    }

    fn frame(&mut self, run: &mut dyn FnMut(&mut dyn FrameSeam) -> (i32, bool)) -> Option<i32> {
        let ticket = match self.ledger.begin_frame() {
            Ok(ticket) => ticket,
            Err(error) => {
                self.record(TicketEnding::Refused(format!("{error:?}")));
                return None;
            }
        };
        let checker = self
            .checker
            .as_ref()
            .map_or(std::ptr::null_mut(), BaselineUpload::metal);
        let current = self.current.take();
        let input = current.and_then(|frame| {
            frame.scene().map(|source| SceneInput {
                source: Arc::clone(source),
                filter: frame.filter(),
                checker,
                compat_key: frame.compat_key().copied(),
                textures: Vec::new(),
            })
        });
        let (code, committed, prepared, refused, scratch, outcome) = {
            let mut seam = TicketSeam {
                ticket: &ticket,
                prepared: false,
                refused: None,
                scene: input,
                scratch: self.scratch.take().unwrap_or_default(),
                drawn: self.drawn,
                outcome: CompatOutcome::Absent,
            };
            let (code, committed) = run(&mut seam);
            // The resolved textures are let go with the seam; the ticket's pins keep their revisions until it ends.
            (
                code,
                committed,
                seam.prepared,
                seam.refused,
                seam.scratch,
                seam.outcome,
            )
        };
        self.scratch = Some(scratch);
        if let Some(refusal) = refused {
            self.record(TicketEnding::Refused(refusal));
        }
        // The ticket ends exactly as the frame did: committed -> submitted; prepared but not committed ->
        // not_committed; neither -> discarded. A ticket dropped without an ending would be uncertain, never this.
        let (ending, result) = if prepared && committed {
            ticket.mark_committed();
            (TicketEnding::Submitted, ticket.submitted())
        } else if prepared {
            (TicketEnding::NotCommitted, ticket.not_committed())
        } else {
            (TicketEnding::Discarded, ticket.discarded())
        };
        let submitted = matches!(ending, TicketEnding::Submitted) && result.is_ok();
        match result {
            Ok(()) => self.record(ending),
            Err(error) => self.record(TicketEnding::Refused(format!("{error:?}"))),
        }
        self.note_compat(&outcome, submitted);
        Some(code)
    }

    fn prove(&mut self) -> Option<String> {
        let proof = self.ledger.prove(Instant::now());
        // The checker's upload token is part of the same proof: complete, or what is outstanding is named.
        let checker_outstanding = self
            .checker
            .as_ref()
            .is_some_and(|upload| !upload.is_done());
        if proof.is_complete() && !checker_outstanding {
            self.proof = Some(proof);
            None
        } else {
            self.proof = None;
            let mut reasons = Vec::new();
            if let Some(reason) = proof.reason().filter(|_| !proof.is_complete()) {
                reasons.push(reason.to_owned());
            } else if !proof.is_complete() {
                reasons.push("the texture ledger's proof is incomplete".to_owned());
            }
            if checker_outstanding {
                reasons.push("the checker upload's completion has not arrived".to_owned());
            }
            Some(reasons.join("; "))
        }
    }

    fn release(&mut self) -> Result<(), String> {
        let proof = self
            .proof
            .take()
            .ok_or_else(|| "no complete proof of the texture ledger to release with".to_owned())?;
        self.ledger.release(proof)?;
        // The ledger removes and commits residency membership, then detaches the set before this texture is released.
        if let Some(upload) = self.checker.take() {
            upload.release();
        }
        Ok(())
    }
}

/// The wakes a presenter gives its store: each marks "texture work pending" and signals the render thread.
pub(crate) fn wakes_of(mark: &Arc<dyn Fn() + Send + Sync>) -> TextureWakes {
    TextureWakes {
        ready_to_submit: Some(Arc::clone(mark)),
        completed: Some(Arc::clone(mark)),
        room: Some(Arc::clone(mark)),
    }
}
