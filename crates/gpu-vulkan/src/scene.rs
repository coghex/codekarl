//! The texture scene of a presenter: the render thread's ledger, the driver that owns the logical side, and the frame
//! tickets the presenter ends according to each frame's fate (milestone 5.5, issue 2a). Its contract is the Metal
//! scene's (`ck-gpu-metal`'s `scene.rs`, CK-114/116), with the differences the specification names.
//!
//! The presenter (`render/scene.rs`) knows none of the key type: everything it needs of a scene is the private
//! [`Scene`] trait, which [`SceneSetup`] builds on the render thread, during setup, from the caller's context, limits
//! and [`TextureDriver`].

use std::hash::Hash;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use ash::vk;

use crate::Error;
use crate::batch::{Batch, Binding, CompatDecision, FrameCounters, InstanceLayout, build_scene};
use crate::context::{RenderQueue, VulkanContext};
use crate::frame::{VulkanFrame, VulkanFrameSlot};
use crate::gate::BindlessLimits;
use crate::hash::FrameMap;
use crate::hooks::CompatKind;
use crate::metrics::MetricsClock;
use crate::quad::{PixelSource, VulkanFilter, VulkanQuad};
use crate::revisions::{
    BindingChoice, BindingNotice, CheckerProgress, FrameSubmission, FrameTicket, LedgerError,
    LedgerEvent, LedgerLimits, LedgerProof, TextureLedger,
};
use crate::scene_draw::InstanceBuffer;
use crate::slot::SlotWake;
use crate::textures::{TestHooks as TextureTestHooks, TextureLimits, TextureWakes};

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

/// The checker as a pixel source.
struct CheckerSource(Vec<u8>);

impl PixelSource for CheckerSource {
    fn pixels(&self) -> &[u8] {
        &self.0
    }
}

/// The bounds of a scene presenter's instance buffer: the most quads a frame draws, and the budget its allocation is
/// charged to (`[render] max_quads` and `binding_geometry_bytes` of the configuration).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneLimits {
    /// Instances the buffer holds; a source emitting more draws the first `max_quads` in painter order and the rest are
    /// counted as dropped.
    pub max_quads: usize,
    /// The budget for the instance buffer: its queried allocation must fit it, or the presenter's setup is refused.
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
    /// runnable: report `false`, and the store's `room` wake or the next poll resumes it.
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

/// How a frame's ticket ended (test evidence; see `SceneHooks::ticket_log`).
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketEnding {
    /// The frame was submitted; the ticket waits for its proof.
    Submitted,
    /// The frame was abandoned before its submit.
    Discarded,
    /// The ledger refused an operation of the ticket (never expected).
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
    /// Something completed or was reported in this pass (an upload, a frame's proof): the thread looks again at once.
    pub(crate) progressed: bool,
}

/// How a frame's instances were built.
pub(crate) struct BuiltFrame {
    pub(crate) counters: FrameCounters,
    /// The frame's filter.
    pub(crate) filter: VulkanFilter,
    /// The initial gate: the compatibility quad had no image and none was held, so the frame must not be drawn.
    pub(crate) gated: bool,
    /// What the frame did with the compatibility quad.
    pub(crate) compat: CompatKind,
}

/// What the render thread reads of the latest frame.
pub(crate) struct FrameRead {
    pub(crate) generation: u64,
    /// The legacy quad (a pixel source shared by the clone, not copied).
    pub(crate) quad: Option<VulkanQuad>,
    /// The frame has a compatibility key: the compatibility instance draws the quad through the ledger, so the legacy quad
    /// is not drawn.
    pub(crate) has_compat: bool,
}

/// The compatibility image a frame drew, with the rectangle and source it was drawn at. The ledger holds its revision for as
/// long as the presenter keeps it (see `TextureLedger::hold_compat`).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeldCompat<K> {
    /// The compatibility key.
    pub key: K,
    /// The revision drawn.
    pub revision: u64,
    /// Where it was drawn.
    pub destination: crate::quad::DestinationRect,
    /// The visible part of it, as fractions.
    pub source: [f32; 4],
}

/// What a frame did with the compatibility quad.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum CompatOutcome<K> {
    Absent,
    Drew(HeldCompat<K>),
    DrewHeld,
    Gated(K),
}

impl<K> CompatOutcome<K> {
    fn kind(&self) -> CompatKind {
        match self {
            Self::Absent => CompatKind::Absent,
            Self::Drew(_) => CompatKind::Drew,
            Self::DrewHeld => CompatKind::DrewHeld,
            Self::Gated(_) => CompatKind::Gated,
        }
    }
}

/// The ticket of the frame in progress, as the render thread's frame step reaches it.
pub(crate) trait FrameSeam {
    /// Builds the scene's draw list into the instance buffer, resolving every key through this frame's ticket (which pins
    /// what it reads). `None` when the frame has no scene source. The batches are then readable with
    /// [`batches`](Self::batches) until the seam ends.
    fn build(
        &mut self,
        size: (u32, u32),
        buffer: &InstanceBuffer,
        max_quads: usize,
    ) -> Option<BuiltFrame>;
    /// The batches the last build produced.
    fn batches(&self) -> &[Batch];
    /// The checker's permanent set (the per-revision mode).
    fn checker_set(&self) -> vk::DescriptorSet;
    /// The set of the table version this frame's ticket pinned (set 1 of the bindless mode); null in the per-revision mode.
    fn table(&self) -> vk::DescriptorSet;
    /// Whether the proof of an earlier frame submission (by number) is recorded (the texture ledger's latched record).
    fn proven(&self, number: u64) -> bool;
    /// Submits the frame through the ticket and `queue`: the frame's number.
    fn submit(
        &mut self,
        queue: &mut RenderQueue,
        command_buffer: vk::CommandBuffer,
        waits: &[(vk::Semaphore, vk::PipelineStageFlags)],
        signals: &[vk::Semaphore],
    ) -> Result<FrameSubmission, LedgerError>;
}

/// What the draws bind besides a batch's own set.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SceneBindings {
    pub(crate) layout: vk::PipelineLayout,
    pub(crate) samplers: [vk::DescriptorSet; 2],
    /// The textures are bound through the bindless table (one set per frame, the slot pushed per batch).
    pub(crate) bindless: bool,
}

/// The per-frame working storage a seam borrows and gives back, so a frame allocates nothing once warm.
pub(crate) struct Scratch<K> {
    resolved: FrameMap<K, Option<Binding>>,
    batches: Vec<Batch>,
}

impl<K> Scratch<K> {
    /// The batches the last build produced.
    pub(crate) fn batches(&self) -> &[Batch] {
        &self.batches
    }
}

impl<K> Default for Scratch<K> {
    fn default() -> Self {
        Self {
            resolved: FrameMap::default(),
            batches: Vec::new(),
        }
    }
}

/// Builds one frame's draw list through `ticket`: the one function the presenter's frames and the off-screen readback
/// share. The instances go to `buffer` in painter order, adjacent equal revisions share a batch, and a key that resolves
/// to nothing draws the checker. The compatibility quad is never drawn with the checker: it is drawn with its current image
/// if that resolves, else with the held one (`held`, at the rectangle it was drawn at), else the frame is gated.
pub(crate) fn build_frame<K: Copy + Eq + Hash + Send + 'static>(
    ticket: &FrameTicket<'_, K>,
    frame: &VulkanFrame<K>,
    size: (u32, u32),
    buffer: &InstanceBuffer,
    max_quads: usize,
    scratch: &mut Scratch<K>,
    held: Option<HeldCompat<K>>,
) -> Option<(BuiltFrame, CompatOutcome<K>)> {
    let source = frame.scene()?;
    let capacity = buffer.capacity.min(max_quads);
    let compat_key = frame.compat_key().copied();
    let Scratch { resolved, batches } = scratch;
    let mut outcome = CompatOutcome::Absent;
    let built = build_scene(
        &**source,
        size,
        capacity,
        ticket.bindless_slots(),
        &mut |slot, record| buffer.write(slot, record),
        &mut |key| ticket.resolve(key).map(|texture| texture.binding()),
        compat_key.is_some(),
        &mut |instance| {
            let Some(instance) = instance else {
                // The compatibility entry placed to nothing (clipped away). While its image is pending the last image drawn
                // stays where it was drawn; once it resolves its own placement rules, which is to draw nothing.
                let Some(key) = compat_key else {
                    return CompatDecision::Skip;
                };
                if ticket.resolve(key).is_some() {
                    return CompatDecision::Skip;
                }
                if let Some(held) = held
                    && let Some(texture) = ticket.resolve_revision(held.key, held.revision)
                {
                    outcome = CompatOutcome::DrewHeld;
                    return CompatDecision::Draw {
                        binding: texture.binding(),
                        destination: held.destination,
                        source: held.source,
                    };
                }
                return CompatDecision::Skip;
            };
            if let Some(texture) = ticket.resolve(instance.key) {
                outcome = CompatOutcome::Drew(HeldCompat {
                    key: instance.key,
                    revision: texture.revision(),
                    destination: instance.destination,
                    source: instance.source,
                });
                return CompatDecision::Draw {
                    binding: texture.binding(),
                    destination: instance.destination,
                    source: instance.source,
                };
            }
            if let Some(held) = held
                && let Some(texture) = ticket.resolve_revision(held.key, held.revision)
            {
                outcome = CompatOutcome::DrewHeld;
                return CompatDecision::Draw {
                    binding: texture.binding(),
                    destination: held.destination,
                    source: held.source,
                };
            }
            outcome = CompatOutcome::Gated(instance.key);
            CompatDecision::Gate
        },
        resolved,
        batches,
    );
    let kind = outcome.kind();
    Some((
        BuiltFrame {
            counters: built.counters,
            filter: frame.filter(),
            gated: built.gated,
            compat: kind,
        },
        outcome,
    ))
}

/// Builds a scene on the render thread.
pub(crate) trait SceneStart: Send + Sync {
    fn context(&self) -> Arc<VulkanContext>;
    fn start(
        &self,
        wakes: TextureWakes,
        log: Option<Arc<Mutex<Vec<TicketEnding>>>>,
    ) -> Result<Box<dyn SceneObject>, Error>;
    /// The instance buffer's bounds.
    fn limits(&self) -> SceneLimits;
    /// How an instance is laid out in the buffer, decided before the buffer is made: the bindless mode's padded 36 bytes when the
    /// gate will pass, else the per-revision mode's 32. The same pure rows the ledger evaluates at its start, which the presenter
    /// checks against what the ledger then did.
    ///
    /// # Errors
    ///
    /// The slot count's refusal (the ledger refuses the same one at its start).
    fn instance_layout(&self) -> Result<InstanceLayout, Error>;
    /// Registers the presenter's wake on the frame slot; a published frame wants a frame.
    fn register_wake(&self, wake: SlotWake);
    /// Forgets the wake (the presenter ended).
    fn clear_wake(&self);
}

/// A scene as the render thread holds it: the object-safe face of [`Scene`] (the generic `frame` is replaced by a
/// boxed form).
pub(crate) trait SceneObject {
    fn read_frame(&mut self) -> FrameRead;
    /// Whether a frame may be attempted as far as the compatibility gate goes: no frame is held back, or the held one can
    /// make progress (its key has a revision to adopt or draw), or the frame published since has another compat key.
    fn frame_gate_open(&self) -> bool;
    fn checker(&mut self) -> CheckerProgress;
    fn baseline_bytes(&self) -> u64;
    fn pump(&mut self, queue: &mut RenderQueue) -> Pumped;
    fn drive(&mut self, boundary: Boundary) -> PassOutcome;
    fn observe(&mut self) -> Option<String>;
    fn has_committed(&self) -> bool;
    fn uploads(&self) -> UploadCounts;
    fn has_release_work(&self) -> bool;
    fn envelope_ready(&self) -> bool;
    fn gpu_outstanding(&self) -> bool;
    fn frame_proven(&self, number: u64) -> bool;
    fn bindings(&self) -> SceneBindings;
    /// The ledger's once-per-run binding notice, taken (a fallback or the bindless table's).
    fn binding_notice(&mut self) -> Option<BindingNotice>;
    /// The bindless table's slots (0 in the per-revision mode).
    fn bindless_slots(&self) -> u32;
    /// Runs `run` inside a frame ticket and ends the ticket by its fate: it returns the submission when the frame was
    /// submitted (the submit ended the ticket) and `None` when it was not (the ticket is ended not-submitted). Returns
    /// `false` without running `run` when no ticket could begin.
    fn frame_dyn(
        &mut self,
        run: &mut dyn FnMut(&mut dyn FrameSeam) -> Option<FrameSubmission>,
    ) -> bool;
    fn prove(&mut self, deadline: Instant) -> Option<String>;
    fn release(&mut self) -> Result<(), String>;
}

/// What a scene presenter is started with: the caller's context, the store's and ledger's limits, the texture
/// driver, the frame slot the renderer publishes to and the instance buffer's bounds, and (tests only) the store's
/// fault hooks. The render thread starts the store and ledger on the context during setup and uploads the checker.
#[doc(hidden)]
#[derive(Clone)]
pub struct SceneSetup(
    pub(crate) Arc<dyn SceneStart>,
    pub(crate) Option<MetricsClock>,
);

impl std::fmt::Debug for SceneSetup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SceneSetup")
    }
}

impl SceneSetup {
    /// The clock the presenter takes its frame-timing marks from (nanoseconds, monotonic, never zero): the application's, so one
    /// measurement window is one interval on one clock for every owner. Without one the presenter uses a clock of its own.
    #[must_use]
    pub fn with_clock(mut self, clock: MetricsClock) -> Self {
        self.1 = Some(clock);
        self
    }

    /// A scene over `context`. `bindless` is the tables' slot count: the device gate decides at the scene's start whether the
    /// textures are bound through the bindless tables or, with one notice, through per-revision sets (the owner's logged
    /// fallback). `texture_hooks` are the store's fault switches (tests only; `None` in production).
    #[expect(
        clippy::too_many_arguments,
        reason = "the context, the four limits, the driver, the frame slot and the hook set the specification names"
    )]
    pub fn new<K, D>(
        context: Arc<VulkanContext>,
        texture_limits: TextureLimits,
        ledger_limits: LedgerLimits,
        max_textures: usize,
        bindless: BindlessLimits,
        limits: SceneLimits,
        driver: D,
        frames: Arc<VulkanFrameSlot<K>>,
        texture_hooks: Option<TextureTestHooks>,
    ) -> Self
    where
        K: Copy + Eq + Hash + Send + 'static,
        D: TextureDriver<K> + 'static,
    {
        Self::with_binding_choice(
            context,
            texture_limits,
            ledger_limits,
            max_textures,
            BindingChoice::Auto(bindless),
            limits,
            driver,
            frames,
            texture_hooks,
        )
    }

    /// [`new`](Self::new) with the binding forced: [`BindingChoice::PerRevision`] is the baseline a measurement compares the
    /// bindless mode against, and the tests of the per-revision path. No configuration, `start_scene` or production path
    /// reaches it.
    #[doc(hidden)]
    #[expect(
        clippy::too_many_arguments,
        reason = "new's arguments with the binding choice in place of the slot count"
    )]
    pub fn with_binding_choice<K, D>(
        context: Arc<VulkanContext>,
        texture_limits: TextureLimits,
        ledger_limits: LedgerLimits,
        max_textures: usize,
        binding: BindingChoice,
        limits: SceneLimits,
        driver: D,
        frames: Arc<VulkanFrameSlot<K>>,
        texture_hooks: Option<TextureTestHooks>,
    ) -> Self
    where
        K: Copy + Eq + Hash + Send + 'static,
        D: TextureDriver<K> + 'static,
    {
        Self(
            Arc::new(Parts {
                context,
                texture_limits,
                ledger_limits,
                max_textures,
                binding,
                limits,
                driver: Mutex::new(Some(driver)),
                frames,
                texture_hooks,
            }),
            None,
        )
    }
}

struct Parts<K, D> {
    context: Arc<VulkanContext>,
    texture_limits: TextureLimits,
    ledger_limits: LedgerLimits,
    max_textures: usize,
    binding: BindingChoice,
    limits: SceneLimits,
    driver: Mutex<Option<D>>,
    frames: Arc<VulkanFrameSlot<K>>,
    texture_hooks: Option<TextureTestHooks>,
}

impl<K, D> SceneStart for Parts<K, D>
where
    K: Copy + Eq + Hash + Send + 'static,
    D: TextureDriver<K> + 'static,
{
    fn context(&self) -> Arc<VulkanContext> {
        Arc::clone(&self.context)
    }

    fn start(
        &self,
        wakes: TextureWakes,
        log: Option<Arc<Mutex<Vec<TicketEnding>>>>,
    ) -> Result<Box<dyn SceneObject>, Error> {
        let driver = self
            .driver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(|| Error("the scene was already started".to_owned()))?;
        let ledger = TextureLedger::start_with_binding(
            Arc::clone(&self.context),
            self.texture_limits,
            self.ledger_limits,
            self.max_textures,
            self.binding,
            wakes,
            self.texture_hooks.clone().unwrap_or_default(),
        )?;
        // The most distinct keys one frame resolves: an instance for each of the `max_quads` it has room for. The ledger keeps
        // a ticket's map for the next frame only within this bound (CK-148); the setting is made once, before any frame, so a
        // refusal is a programming error.
        ledger
            .set_spare_limit(self.limits.max_quads.saturating_add(2))
            .map_err(|error| Error(format!("the ledger's spare limit was refused: {error:?}")))?;
        // The checker's upload, through the store as a baseline item (a refusal is the checker's failure, which the
        // presenter reports before any frame).
        let _ = ledger.start_checker(
            CHECKER_SIDE,
            CHECKER_SIDE,
            Arc::new(CheckerSource(checker_pixels())),
        );
        Ok(Box::new(SceneState {
            ledger,
            driver,
            held: Vec::new(),
            proof: None,
            log,
            frames: Arc::clone(&self.frames),
            current: None,
            scratch: Some(Scratch::default()),
            drawn: None,
            blocked: None,
        }))
    }

    fn limits(&self) -> SceneLimits {
        self.limits
    }

    fn instance_layout(&self) -> Result<InstanceLayout, Error> {
        let BindingChoice::Auto(bindless) = self.binding else {
            return Ok(InstanceLayout::PER_REVISION);
        };
        let bindless =
            bindless.validate(self.max_textures, self.ledger_limits.max_extra_revisions)?;
        let caps = self.context.bindless_caps();
        if !crate::gate::evaluate(caps, bindless.texture_slots).is_empty() {
            return Ok(InstanceLayout::PER_REVISION);
        }
        InstanceLayout::bindless(caps.limits.min_vertex_input_binding_stride_alignment)
            .ok_or_else(|| Error("the bindless instance stride overflows".to_owned()))
    }

    fn register_wake(&self, wake: SlotWake) {
        self.frames.register_wake(wake);
    }

    fn clear_wake(&self) {
        self.frames.clear_wake();
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
    frames: Arc<VulkanFrameSlot<K>>,
    /// The frame read for the attempt in progress.
    current: Option<VulkanFrame<K>>,
    /// Taken by the seam of a frame and put back after it.
    scratch: Option<Scratch<K>>,
    /// The last compatibility image a committed frame drew. The ledger holds its revision for as long as this is set.
    drawn: Option<HeldCompat<K>>,
    /// The compatibility key of the frame held back by the initial gate, if any.
    blocked: Option<K>,
}

impl<K: Copy + Eq + Hash + Send + 'static, D> SceneState<K, D> {
    /// What a frame's ending means for the compatibility quad. Called after the ticket ended; a frame that was only
    /// *attempted* (not submitted) never changes what was last drawn.
    fn note_compat(&mut self, outcome: &CompatOutcome<K>, submitted: bool) {
        match *outcome {
            CompatOutcome::Gated(key) => self.blocked = Some(key),
            CompatOutcome::Drew(drawn) if submitted => {
                self.blocked = None;
                // The image just drawn is the one kept on screen: hold its revision (a newer hold releases the older).
                if self.ledger.hold_compat(drawn.key, drawn.revision).is_ok() {
                    self.drawn = Some(drawn);
                }
            }
            CompatOutcome::Absent if submitted => {
                // A drawn frame that drew nothing for the compatibility quad clears what was kept.
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

/// The ticket of one frame step, as the frame step sees it.
struct TicketSeam<'t, 'a, K: Copy + Eq + Hash + Send + 'static> {
    ledger: &'a TextureLedger<K>,
    ticket: &'t FrameTicket<'a, K>,
    frame: Option<VulkanFrame<K>>,
    scratch: Scratch<K>,
    checker: vk::DescriptorSet,
    /// The table version's set this ticket pinned (null in the per-revision mode).
    table: vk::DescriptorSet,
    /// The image the last committed frame drew for the compatibility quad.
    drawn: Option<HeldCompat<K>>,
    /// What this frame did with the compatibility quad.
    outcome: CompatOutcome<K>,
}

impl<K: Copy + Eq + Hash + Send + 'static> FrameSeam for TicketSeam<'_, '_, K> {
    fn build(
        &mut self,
        size: (u32, u32),
        buffer: &InstanceBuffer,
        max_quads: usize,
    ) -> Option<BuiltFrame> {
        let frame = self.frame.as_ref()?;
        let (built, outcome) = build_frame(
            self.ticket,
            frame,
            size,
            buffer,
            max_quads,
            &mut self.scratch,
            self.drawn,
        )?;
        self.outcome = outcome;
        Some(built)
    }

    fn batches(&self) -> &[Batch] {
        &self.scratch.batches
    }

    fn checker_set(&self) -> vk::DescriptorSet {
        self.checker
    }

    fn table(&self) -> vk::DescriptorSet {
        self.table
    }

    fn proven(&self, number: u64) -> bool {
        self.ledger.frame_proven_number(number)
    }

    fn submit(
        &mut self,
        queue: &mut RenderQueue,
        command_buffer: vk::CommandBuffer,
        waits: &[(vk::Semaphore, vk::PipelineStageFlags)],
        signals: &[vk::Semaphore],
    ) -> Result<FrameSubmission, LedgerError> {
        self.ticket.submit(queue, command_buffer, waits, signals)
    }
}

impl<K, D> SceneObject for SceneState<K, D>
where
    K: Copy + Eq + Hash + Send + 'static,
    D: TextureDriver<K>,
{
    fn read_frame(&mut self) -> FrameRead {
        let frame = self.frames.latest();
        // Each newer frame gives the ledger its current compatibility key (one slot), so a failed upload of it is also the
        // presenter's, whatever was placed.
        self.ledger.set_compat_key(frame.compat_key().copied());
        let read = FrameRead {
            generation: frame.generation(),
            quad: frame.quad().cloned(),
            has_compat: frame.compat_key().is_some(),
        };
        self.current = Some(frame);
        read
    }

    fn frame_gate_open(&self) -> bool {
        let Some(blocked) = self.blocked else {
            return true;
        };
        // A newer frame with another compatibility key (replaced, or cleared) re-evaluates; so does the gating key having
        // something to adopt or draw. Both are facts read now, so no wake can be lost.
        self.frames.latest().compat_key().copied() != Some(blocked)
            || self.ledger.has_committed_or_active(blocked)
    }

    fn checker(&mut self) -> CheckerProgress {
        self.ledger.checker_progress()
    }

    fn baseline_bytes(&self) -> u64 {
        self.ledger.checker_bytes()
    }

    fn pump(&mut self, queue: &mut RenderQueue) -> Pumped {
        // The key is marked before the pump that may see its failure: an upload failing before any frame was built is
        // classified all the same.
        let marked = self.frames.latest().compat_key().copied();
        self.ledger.set_compat_key(marked);
        let mut events = Vec::new();
        let before = self.ledger.upload_stats().batches_completed;
        let more = self.ledger.pump(queue, &mut events);
        let mut failure = None;
        events.retain(|event| match event {
            LedgerEvent::GpuFailure { message } => {
                failure.get_or_insert_with(|| message.clone());
                false
            }
            // The presenter's observation of a compatibility upload failure; the driver keeps the ordinary
            // `UploadFailed` of the same failure (the service's status shows it).
            LedgerEvent::CompatUploadFailed { reason, .. } => {
                failure.get_or_insert_with(|| {
                    format!("the legacy quad's image upload failed: {reason}")
                });
                false
            }
            _ => true,
        });
        let progressed =
            self.ledger.upload_stats().batches_completed != before || !events.is_empty();
        self.held.extend(events);
        Pumped {
            failure,
            more,
            progressed,
        }
    }

    fn has_release_work(&self) -> bool {
        self.ledger.has_release_work()
    }

    fn envelope_ready(&self) -> bool {
        self.ledger.envelope_ready_unsubmitted()
    }

    fn gpu_outstanding(&self) -> bool {
        self.ledger.has_submitted() || self.ledger.frame_pending() || self.ledger.has_release_work()
    }

    fn frame_proven(&self, number: u64) -> bool {
        self.ledger.frame_proven_number(number)
    }

    fn bindings(&self) -> SceneBindings {
        SceneBindings {
            layout: self.ledger.pipeline_layout(),
            samplers: self.ledger.sampler_sets(),
            bindless: self.ledger.bindless_slots() != 0,
        }
    }

    fn binding_notice(&mut self) -> Option<BindingNotice> {
        self.ledger.take_binding_notice()
    }

    fn bindless_slots(&self) -> u32 {
        self.ledger.bindless_slots()
    }

    fn drive(&mut self, boundary: Boundary) -> PassOutcome {
        let events = std::mem::take(&mut self.held);
        self.driver.pass(&mut self.ledger, boundary, events)
    }

    fn observe(&mut self) -> Option<String> {
        // The checker's outcome too: a checker whose upload failed while the presenter is stopping is still a failure,
        // reported before anything is released. The presenter's exactly-once latch keeps it from being reported twice.
        let ledger = self.ledger.observe_failure();
        match self.ledger.checker_progress() {
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

    fn frame_dyn(
        &mut self,
        run: &mut dyn FnMut(&mut dyn FrameSeam) -> Option<FrameSubmission>,
    ) -> bool {
        let ticket = match self.ledger.begin_frame() {
            Ok(ticket) => ticket,
            Err(error) => {
                self.record(TicketEnding::Refused(format!("{error:?}")));
                return false;
            }
        };
        let checker = self
            .ledger
            .checker_set()
            .unwrap_or_else(vk::DescriptorSet::null);
        let (submission, scratch, outcome) = {
            let mut seam = TicketSeam {
                ledger: &self.ledger,
                ticket: &ticket,
                frame: self.current.take(),
                scratch: self.scratch.take().unwrap_or_default(),
                checker,
                table: ticket.table().unwrap_or_else(vk::DescriptorSet::null),
                drawn: self.drawn,
                outcome: CompatOutcome::Absent,
            };
            let submission = run(&mut seam);
            (submission, seam.scratch, seam.outcome)
        };
        self.scratch = Some(scratch);
        // The ticket ends exactly as the frame did: submitted -> the submit ended it (its pins wait for the proof);
        // anything else -> not submitted. A ticket dropped without an ending would be uncertain, never this.
        if submission.is_some() {
            // `FrameTicket::submit` already ended the ticket; dropping it only returns its maps.
            drop(ticket);
            self.record(TicketEnding::Submitted);
        } else {
            match ticket.not_submitted() {
                Ok(()) => self.record(TicketEnding::Discarded),
                Err(error) => self.record(TicketEnding::Refused(format!("{error:?}"))),
            }
        }
        self.note_compat(&outcome, submission.is_some());
        true
    }

    fn prove(&mut self, deadline: Instant) -> Option<String> {
        let proof = self.ledger.prove(deadline);
        if proof.is_complete() {
            self.proof = Some(proof);
            None
        } else {
            self.proof = None;
            Some(
                proof
                    .reason()
                    .unwrap_or("the texture ledger's proof is incomplete")
                    .to_owned(),
            )
        }
    }

    fn release(&mut self) -> Result<(), String> {
        let proof = self
            .proof
            .take()
            .ok_or_else(|| "no complete proof of the texture ledger to release with".to_owned())?;
        self.ledger.release(proof)
    }
}

/// The wakes a presenter gives its store: each marks "texture work pending" and signals the render thread.
pub(crate) fn wakes_of(mark: &Arc<dyn Fn() + Send + Sync>) -> TextureWakes {
    TextureWakes {
        ready_to_submit: Some(Arc::clone(mark)),
        room: Some(Arc::clone(mark)),
    }
}

#[cfg(test)]
mod tests {
    use super::{CHECKER_CELL, CHECKER_SIDE, checker_pixels};

    #[test]
    fn the_checker_is_64_by_64_of_8_pixel_cells_with_a_magenta_top_left() {
        let pixels = checker_pixels();
        let side = CHECKER_SIDE as usize;
        let cell = CHECKER_CELL as usize;
        assert_eq!(pixels.len(), side * side * 4);
        let at = |x: usize, y: usize| pixels[(y * side + x) * 4..][..4].to_vec();
        let magenta = vec![255, 0, 255, 255];
        let black = vec![0, 0, 0, 255];
        assert_eq!(at(0, 0), magenta, "the top-left cell is magenta");
        assert_eq!(
            at(cell - 1, cell - 1),
            magenta,
            "and fills its 8 x 8 pixels"
        );
        assert_eq!(at(cell, 0), black, "the next cell to the right is black");
        assert_eq!(at(0, cell), black, "and the next one down");
        assert_eq!(at(cell, cell), magenta);
        assert_eq!(
            at(side - 1, side - 1),
            magenta,
            "8 x 8 cells: the corners agree"
        );
        assert!(
            pixels.chunks_exact(4).all(|pixel| pixel[3] == 255),
            "opaque, so premultiplication changes nothing"
        );
    }
}
