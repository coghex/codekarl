//! Render-owner ledger for uploaded image revisions (milestone 5.5, issue 1). Its contract is the Metal
//! ledger's (`ck-gpu-metal`'s `ledger.rs`, CK-113, with CK-148's per-frame costs and CK-150's ticket
//! coherence); the differences are stated where they arise.
//!
//! Upload completion, service commitment, frame-boundary adoption and native reclamation are separate
//! transitions. A revision is owned until its upload and every exact frame reader have completion proofs.
//!
//! - **States.** `InFlight`, `Prepared`, `Committed`, `Active`, `Retired`, and released. One baseline charge per
//!   key; extras are at most `max_extra_revisions` and are created only by admission.
//! - **A descriptor set per revision** ([`descriptors`](crate::descriptors)). When `pump` sees a revision's
//!   upload proven it writes the revision's set, in one batched update, *before* it emits `Prepared`. A set goes
//!   back on the free list only when the revision is released, so a set is never written while a recorded or
//!   submitted command buffer may bind it. No set available is `UploadFailed` for that revision (its image
//!   released, the key's shown revision unchanged, nothing leaked).
//! - **Frame completion is a submission number, never a raw fence.** [`FrameTicket::submit`] performs the
//!   frame's `vkQueueSubmit` with a fence from the ledger's own pool of two and returns a
//!   [`FrameSubmission`]; the fence is never visible outside the ledger. Only [`TextureLedger::pump`] and
//!   [`TextureLedger::prove`] read frame fences, and when one is seen signalled the ledger records the proof
//!   (see `proofs.rs`) before anything else is done with that fence. A fence is reset only by
//!   [`TextureLedger::begin_frame`], when it is taken for a new ticket, and only if its previous submission's
//!   proof is recorded; otherwise `begin_frame` is busy. [`TextureLedger::frame_proven`] stays true after the
//!   fence is reused.
//! - **Release** needs no pin, the upload proven, and every submitted ticket that resolved the revision proven
//!   by its recorded proof (discarded tickets unpin at once). Then the view and the image are destroyed, the range
//!   returns to the pool and the set to the free list. Nothing waits for the device or the queue to go idle.
//! - **Not here:** the compatibility hold (CK-118/119, with the legacy lane in M5.5-2) and bindless tables
//!   (M5.5-5).

use std::cell::{Cell, RefCell};
use std::collections::{VecDeque, hash_map::Entry};
use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ash::vk;

use crate::Error;
use crate::context::{RenderQueue, VulkanContext};
use crate::descriptors::Descriptors;
use crate::error::vk_error;
use crate::hash::{FrameMap, FrameSet};
use crate::proofs::FenceProofs;
use crate::quad::PixelSource;
use crate::textures::{
    Completed, PreparedTexture, Proof, Refused, TestHooks, TextureLimits, TextureOwner,
    TextureStats, TextureStore, TextureWakes, UploadItem, UploadPort,
};

const RELEASES_PER_CALL: usize = 256;
/// The frame fences: one pending ticket, one being built.
const FRAME_FENCES: usize = 2;
static NEXT_LEDGER_ID: AtomicU64 = AtomicU64::new(1);

/// Admission limits for replacement revisions and boundary adoption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerLimits {
    /// Maximum live revisions beyond one baseline per key.
    pub max_extra_revisions: usize,
    /// Maximum committed revisions adopted at one boundary.
    pub max_adoptions_per_frame: usize,
}

impl Default for LedgerLimits {
    fn default() -> Self {
        Self {
            max_extra_revisions: 1_024,
            max_adoptions_per_frame: 256,
        }
    }
}

impl LedgerLimits {
    /// Validates each limit in the supported range `1..=65536`.
    ///
    /// # Errors
    ///
    /// Returns the first field outside the supported range.
    pub fn validate(self) -> Result<Self, Error> {
        for (name, value) in [
            ("max_extra_revisions", self.max_extra_revisions),
            ("max_adoptions_per_frame", self.max_adoptions_per_frame),
        ] {
            if !(1..=65_536).contains(&value) {
                return Err(Error(format!("{name}={value} is outside 1..=65536")));
            }
        }
        Ok(self)
    }
}

/// Lifecycle state of one native revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionState {
    /// Offered; the upload is not proven yet.
    InFlight,
    /// Uploaded, with its set written; awaiting the service's decision.
    Prepared,
    /// Approved; invisible until the next frame boundary adopts it.
    Committed,
    /// The key's shown revision.
    Active,
    /// Replaced, rejected or retiring; released when nothing reads it.
    Retired,
}

/// Refusal from the ledger's only upload-admission path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfferRefusal {
    /// The key is retiring; the upload and its CPU reservation have been dropped.
    KeyRetiring,
    /// The caller violated the reserved room contract or reused a live identity.
    Protocol(String),
}

/// An invalid ledger operation or a Vulkan failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    /// The key/revision pair is not live in this ledger.
    UnknownRevision,
    /// The operation requires a different revision state.
    InvalidState {
        /// The state found.
        state: RevisionState,
        /// The operation refused.
        operation: &'static str,
    },
    /// The one-building-ticket bound is occupied, one submitted ticket is pending, or no frame fence is free.
    TicketBusy,
    /// Shutdown started or a GPU failure was latched.
    Ended,
    /// [`TextureLedger::set_spare_limit`] was called a second time, or after the first
    /// [`TextureLedger::begin_frame`].
    SpareLimitFixed,
    /// A Vulkan call failed.
    Native(String),
}

/// Bounded keyed notifications produced by [`TextureLedger::pump`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerEvent<K> {
    /// An upload is ready for the service's compare-and-commit; its descriptor set is written.
    Prepared {
        /// The key.
        key: K,
        /// The revision.
        revision: u64,
    },
    /// One upload failed; the failure does not poison other revisions.
    UploadFailed {
        /// The key.
        key: K,
        /// The revision.
        revision: u64,
        /// Why.
        reason: String,
    },
    /// The upload of the key marked by [`TextureLedger::set_compat_key`] failed. It accompanies the ordinary
    /// [`UploadFailed`](Self::UploadFailed) of the same failure, emitted first: two observations of one failure, so
    /// neither consumer can take the other's. A failure of a key that is no longer marked produces only the ordinary
    /// event.
    CompatUploadFailed {
        /// The key.
        key: K,
        /// Why.
        reason: String,
    },
    /// A store failure was latched; emitted at most once.
    GpuFailure {
        /// What failed.
        message: String,
    },
    /// All revisions of a retiring key were released.
    KeyRetired {
        /// The key.
        key: K,
    },
}

/// The number of one frame submission: monotonic, and the whole caller contract for frame completion
/// ([`TextureLedger::frame_proven`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameSubmission(u64);

impl FrameSubmission {
    /// The raw number (the first submission is 1).
    #[must_use]
    pub fn number(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RevisionId<K> {
    key: K,
    revision: u64,
}

/// What an upload of this ledger's store is for: a revision of a logical key, or the presenter's checker (a baseline
/// item, which is no key's and holds no ledger charge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RevisionKey<K> {
    Revision(RevisionId<K>),
    Checker,
}

/// How far the checker's upload is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckerProgress {
    /// No checker was asked for, or its completion has not arrived.
    Pending,
    /// Resident, with its permanent descriptor set.
    Resident,
    /// Its upload failed.
    Failed(String),
}

/// The checker's state in the ledger.
enum CheckerSlot<K> {
    None,
    Pending,
    Resident {
        texture: PreparedTexture<RevisionKey<K>>,
        set: vk::DescriptorSet,
    },
    Failed(String),
}

/// The moment of this process's first look at the clock, so adoption times are small, comparable numbers.
fn now_ns() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let epoch = *EPOCH.get_or_init(Instant::now);
    // At least 1: zero means "none yet" to the callers, and the first look can be within the clock's resolution of the epoch.
    u64::try_from(epoch.elapsed().as_nanos())
        .unwrap_or(u64::MAX - 1)
        .saturating_add(1)
}

struct Revision<K> {
    id: RevisionId<K>,
    sequence: u64,
    state: RevisionState,
    extra: bool,
    pins: usize,
    release_queued: bool,
    set: Option<vk::DescriptorSet>,
    texture: Option<PreparedTexture<RevisionKey<K>>>,
}

struct KeyRevisions<K> {
    revisions: FrameMap<u64, Revision<K>>,
    baseline: Option<u64>,
    active: Option<u64>,
    retiring: bool,
}

/// The one submitted ticket whose proof is not recorded yet.
struct PendingTicket<K> {
    pins: Vec<RevisionId<K>>,
    slot: usize,
    submission: u64,
}

/// A ticket that ended without an explicit outcome, or whose submit may have reached the GPU: its pins are
/// kept for good.
struct UncertainTicket<K> {
    _pins: Vec<RevisionId<K>>,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent lifecycle facts are observed together for the end proof"
)]
struct LedgerState<K> {
    keys: FrameMap<K, KeyRevisions<K>>,
    completed_pending: VecDeque<Completed<RevisionKey<K>>>,
    committed: VecDeque<RevisionId<K>>,
    release_candidates: VecDeque<RevisionId<K>>,
    events: VecDeque<LedgerEvent<K>>,
    sequence: u64,
    extra_charges: usize,
    baseline_charges: usize,
    live_pins: usize,
    building: bool,
    pending: Option<PendingTicket<K>>,
    uncertain: Vec<UncertainTicket<K>>,
    proofs: FenceProofs<vk::Fence>,
    failure_latched: bool,
    failure_event_emitted: bool,
    ending: bool,
    /// The most entries a ticket's map may have held for the ledger to keep it for the next ticket (CK-148).
    spare_limit: usize,
    spare_limit_set: bool,
    frames_begun: bool,
    spare_resolved: Option<FrameMap<K, Option<u64>>>,
    spare_pinned: Option<FrameSet<RevisionId<K>>>,
    checker: CheckerSlot<K>,
    /// The one key whose upload failure is also the presenter's (the current frame's compatibility key).
    compat_key: Option<K>,
    /// The one revision held for the presenter as the last compatibility image it drew: a pin that outlives the key's
    /// retirement and newer adoptions.
    compat_hold: Option<RevisionId<K>>,
    /// Revisions adopted over the ledger's life, and when the first and the latest boundary that adopted any ran.
    adopted_total: u64,
    first_adopted_ns: u64,
    last_adopted_ns: u64,
}

impl<K> LedgerState<K> {
    fn empty(proofs: FenceProofs<vk::Fence>) -> Self {
        Self {
            keys: FrameMap::default(),
            completed_pending: VecDeque::new(),
            committed: VecDeque::new(),
            release_candidates: VecDeque::new(),
            events: VecDeque::new(),
            sequence: 0,
            extra_charges: 0,
            baseline_charges: 0,
            live_pins: 0,
            building: false,
            pending: None,
            uncertain: Vec::new(),
            proofs,
            failure_latched: false,
            failure_event_emitted: false,
            ending: false,
            spare_limit: 0,
            spare_limit_set: false,
            frames_begun: false,
            spare_resolved: None,
            spare_pinned: None,
            checker: CheckerSlot::None,
            compat_key: None,
            compat_hold: None,
            adopted_total: 0,
            first_adopted_ns: 0,
            last_adopted_ns: 0,
        }
    }
}

/// The unique render-owner ledger for an upload store and its native revisions.
///
/// It is `Send`, not `Sync` or `Clone`; all owner operations are serialized on its render thread.
pub struct TextureLedger<K: Copy + Eq + Hash + Send + 'static> {
    context: Arc<VulkanContext>,
    port: UploadPort<RevisionKey<K>>,
    owner: RefCell<Option<TextureOwner<RevisionKey<K>>>>,
    state: RefCell<LedgerState<K>>,
    descriptors: RefCell<Option<Descriptors>>,
    limits: LedgerLimits,
    wakes: TextureWakes,
    id: u64,
    released: Cell<bool>,
    _not_sync: PhantomData<Cell<()>>,
}

#[expect(
    clippy::missing_errors_doc,
    reason = "all operation errors are described by the public LedgerError or Error type"
)]
impl<K: Copy + Eq + Hash + Send + 'static> TextureLedger<K> {
    /// Starts the upload worker and creates the owner-thread revision ledger: the set layouts, the two pools
    /// (`ordinary_sets = max_textures + max_extra_revisions + 1` revision sets, and the two sampler sets,
    /// written), and the pool of two frame fences.
    pub fn start(
        context: Arc<VulkanContext>,
        texture_limits: TextureLimits,
        limits: LedgerLimits,
        max_textures: usize,
        wakes: TextureWakes,
    ) -> Result<Self, Error> {
        Self::start_inner(
            context,
            texture_limits,
            limits,
            max_textures,
            wakes,
            TestHooks::default(),
        )
    }

    /// Test-only fault boundary for the store's hooks.
    #[doc(hidden)]
    pub fn start_with_test_hooks(
        context: Arc<VulkanContext>,
        texture_limits: TextureLimits,
        limits: LedgerLimits,
        max_textures: usize,
        wakes: TextureWakes,
        hooks: TestHooks,
    ) -> Result<Self, Error> {
        Self::start_inner(context, texture_limits, limits, max_textures, wakes, hooks)
    }

    fn start_inner(
        context: Arc<VulkanContext>,
        texture_limits: TextureLimits,
        limits: LedgerLimits,
        max_textures: usize,
        wakes: TextureWakes,
        hooks: TestHooks,
    ) -> Result<Self, Error> {
        let limits = limits.validate()?;
        if max_textures == 0 {
            return Err(Error("max_textures must be at least 1".to_owned()));
        }
        let ordinary_sets = max_textures
            .checked_add(limits.max_extra_revisions)
            .and_then(|sets| sets.checked_add(1))
            .ok_or_else(|| {
                Error(format!(
                    "ordinary_sets overflows: {max_textures} textures + {} extra revisions + 1",
                    limits.max_extra_revisions
                ))
            })?;
        let mut descriptors = Descriptors::create(&context, ordinary_sets)?;
        let fences = match create_fences(&context) {
            Ok(fences) => fences,
            Err(error) => {
                descriptors.destroy();
                return Err(error);
            }
        };
        let (port, owner) = match TextureStore::start_with_test_hooks(
            Arc::clone(&context),
            texture_limits,
            wakes.clone(),
            hooks,
        ) {
            Ok(started) => started,
            Err(error) => {
                for fence in &fences {
                    // SAFETY: never submitted.
                    unsafe { context.device().destroy_fence(*fence, None) };
                }
                descriptors.destroy();
                return Err(error);
            }
        };
        Ok(Self {
            context,
            port,
            owner: RefCell::new(Some(owner)),
            state: RefCell::new(LedgerState::empty(FenceProofs::new(&fences))),
            descriptors: RefCell::new(Some(descriptors)),
            limits,
            wakes,
            id: NEXT_LEDGER_ID.fetch_add(1, Ordering::Relaxed),
            released: Cell::new(false),
            _not_sync: PhantomData,
        })
    }

    /// The pipeline layout (set 0 a sampler, set 1 a sampled image, both for the fragment stage) every frame
    /// binds the ledger's sets with.
    #[must_use]
    pub fn pipeline_layout(&self) -> vk::PipelineLayout {
        self.descriptors
            .borrow()
            .as_ref()
            .map_or_else(vk::PipelineLayout::null, Descriptors::pipeline_layout)
    }

    /// The two sampler sets (set 0): nearest, then linear. They are written at start and never again.
    #[must_use]
    pub fn sampler_sets(&self) -> [vk::DescriptorSet; 2] {
        self.descriptors
            .borrow()
            .as_ref()
            .map_or([vk::DescriptorSet::null(); 2], Descriptors::sampler_sets)
    }

    /// Marks `key` as the current frame's compatibility key (one slot: a later call replaces it, `None` clears it), so a
    /// failed upload of it is also reported as [`LedgerEvent::CompatUploadFailed`] by the next [`pump`](Self::pump). This
    /// affects failure reporting only; it does not change active mappings.
    pub fn set_compat_key(&self, key: Option<K>) {
        self.state.borrow_mut().compat_key = key;
    }

    /// Whether `key` has something a frame could draw or will draw at its next boundary: a committed revision waiting for
    /// adoption, or an active one. The gate's readiness predicate: a persistent fact, not an edge, so it cannot be lost.
    #[must_use]
    pub fn has_committed_or_active(&self, key: K) -> bool {
        let state = self.state.borrow();
        state.committed.iter().any(|id| id.key == key)
            || state
                .keys
                .get(&key)
                .is_some_and(|entry| entry.active.is_some())
    }

    /// Holds one revision of `key` for the presenter: the last compatibility image it drew. The hold is a pin: the
    /// revision stays alive and resolvable through [`FrameTicket::resolve_revision`] after the key retired or newer
    /// revisions were adopted, and the key's [`LedgerEvent::KeyRetired`] waits for it. Holding a different revision
    /// releases the previous hold (one slot), so at most one old revision is held; holding the held one again changes
    /// nothing. A building ticket's held-revision lookup is checked against the current hold on every call, not cached
    /// (**CK-150's rule**): replacing or releasing the hold can make a repeated lookup return `None`, and it can never
    /// return a wrong set, because the lookup requires the exact currently held key and revision.
    ///
    /// # Errors
    ///
    /// [`LedgerError::UnknownRevision`] if the revision is not live or has no texture.
    pub fn hold_compat(&self, key: K, revision: u64) -> Result<(), LedgerError> {
        let mut state = self.state.borrow_mut();
        let id = RevisionId { key, revision };
        if state.compat_hold == Some(id) {
            return Ok(());
        }
        {
            let item = state
                .keys
                .get_mut(&key)
                .and_then(|entry| entry.revisions.get_mut(&revision))
                .filter(|item| item.texture.is_some() && item.set.is_some())
                .ok_or(LedgerError::UnknownRevision)?;
            item.pins += 1;
        }
        state.live_pins += 1;
        if let Some(previous) = state.compat_hold.replace(id) {
            release_pins(&mut state, vec![previous]);
        }
        Ok(())
    }

    /// Releases the compatibility hold, if any (the frame drawn has no compatibility quad), immediately, including while a
    /// ticket is building. A revision that was only kept alive by the hold is then released by the ordinary bounded release
    /// work, and a repeated held-revision lookup on an open ticket returns `None`.
    pub fn release_compat_hold(&self) {
        let mut state = self.state.borrow_mut();
        if let Some(previous) = state.compat_hold.take() {
            release_pins(&mut state, vec![previous]);
        }
    }

    /// The revision held now, if any.
    #[must_use]
    pub fn compat_hold(&self) -> Option<(K, u64)> {
        self.state
            .borrow()
            .compat_hold
            .map(|id| (id.key, id.revision))
    }

    /// Starts the checker's upload: a baseline item of the store (no logical key, no ledger charge and no slot of the
    /// prepared window), uploaded like any image and resident, with its own permanent descriptor set, once a `pump` has
    /// seen it complete. It is never retired before the end. Asking again is refused.
    pub fn start_checker(
        &self,
        width: u32,
        height: u32,
        pixels: Arc<dyn PixelSource>,
    ) -> Result<(), Error> {
        let mut state = self.state.borrow_mut();
        if !matches!(state.checker, CheckerSlot::None) {
            return Err(Error("the checker was already started".to_owned()));
        }
        let item = UploadItem {
            tag: RevisionKey::Checker,
            width,
            height,
            pixels,
        };
        if self.port.offer_baseline(item).is_err() {
            state.checker = CheckerSlot::Failed("the upload store refused the checker".to_owned());
            return Err(Error("the upload store refused the checker".to_owned()));
        }
        state.checker = CheckerSlot::Pending;
        Ok(())
    }

    /// How far the checker's upload is (one non-blocking look at what the last `pump` found).
    #[must_use]
    pub fn checker_progress(&self) -> CheckerProgress {
        match &self.state.borrow().checker {
            CheckerSlot::None | CheckerSlot::Pending => CheckerProgress::Pending,
            CheckerSlot::Resident { .. } => CheckerProgress::Resident,
            CheckerSlot::Failed(reason) => CheckerProgress::Failed(reason.clone()),
        }
    }

    /// The checker's permanent descriptor set (set 1), once it is resident.
    #[must_use]
    pub fn checker_set(&self) -> Option<vk::DescriptorSet> {
        match &self.state.borrow().checker {
            CheckerSlot::Resident { set, .. } => Some(*set),
            _ => None,
        }
    }

    /// The bytes the driver asked for the checker's image (the presenter's baseline); zero until it is resident.
    #[must_use]
    pub fn checker_bytes(&self) -> u64 {
        match &self.state.borrow().checker {
            CheckerSlot::Resident { texture, .. } => texture.allocated_size,
            _ => 0,
        }
    }

    /// The store's cumulative figures (constant work).
    #[must_use]
    pub fn upload_stats(&self) -> TextureStats {
        self.owner
            .borrow()
            .as_ref()
            .map(TextureOwner::stats)
            .unwrap_or_default()
    }

    /// The revisions adopted over the ledger's life and the times (nanoseconds since this process first looked at the
    /// clock) of the first and the latest boundary that adopted any: a light read, for the presenter's stats.
    #[must_use]
    pub fn adoption_counts(&self) -> (u64, u64, u64) {
        let state = self.state.borrow();
        (
            state.adopted_total,
            state.first_adopted_ns,
            state.last_adopted_ns,
        )
    }

    /// Number of upload slots currently available.
    #[must_use]
    pub fn room(&self) -> usize {
        self.port.room()
    }

    /// Number of replacement slots currently available.
    #[must_use]
    pub fn extra_room(&self) -> usize {
        self.limits
            .max_extra_revisions
            .saturating_sub(self.state.borrow().extra_charges)
    }

    /// Offers one immutable CPU image revision. This creates an in-flight revision but does not change the active
    /// mapping. Callers must reserve at most `min(room(), extra_room())` offers before calling this method.
    pub fn offer(
        &self,
        key: K,
        revision: u64,
        width: u32,
        height: u32,
        pixels: Arc<dyn PixelSource>,
    ) -> Result<(), OfferRefusal> {
        let mut state = self.state.borrow_mut();
        if state.ending || state.failure_latched {
            return Err(OfferRefusal::Protocol(
                "ledger is ending or failed".to_owned(),
            ));
        }
        if state.keys.get(&key).is_some_and(|entry| entry.retiring) {
            return Err(OfferRefusal::KeyRetiring);
        }
        let existing = state.keys.get(&key);
        if existing.is_some_and(|entry| entry.revisions.contains_key(&revision)) {
            return Err(OfferRefusal::Protocol(
                "duplicate live key/revision".to_owned(),
            ));
        }
        let is_new_key = existing.is_none_or(|entry| entry.revisions.is_empty());
        if self.port.room() == 0
            || (!is_new_key && state.extra_charges >= self.limits.max_extra_revisions)
        {
            return Err(OfferRefusal::Protocol(
                "offer exceeded reserved room".to_owned(),
            ));
        }
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        let id = RevisionId { key, revision };
        let item = UploadItem {
            tag: RevisionKey::Revision(id),
            width,
            height,
            pixels,
        };
        if let Err(Refused(item)) = self.port.offer(item) {
            drop(item);
            return Err(OfferRefusal::Protocol(
                "upload port refused reserved room".to_owned(),
            ));
        }
        if is_new_key {
            state.baseline_charges = state.baseline_charges.saturating_add(1);
            state.keys.insert(
                key,
                KeyRevisions {
                    revisions: FrameMap::default(),
                    baseline: Some(revision),
                    active: None,
                    retiring: false,
                },
            );
        }
        let extra = state
            .keys
            .get(&key)
            .is_some_and(|entry| entry.baseline != Some(revision));
        if extra {
            state.extra_charges += 1;
        }
        let Some(entry) = state.keys.get_mut(&key) else {
            return Err(OfferRefusal::Protocol(
                "ledger key insertion failed".to_owned(),
            ));
        };
        entry.revisions.insert(
            revision,
            Revision {
                id,
                sequence,
                state: RevisionState::InFlight,
                extra,
                pins: 0,
                release_queued: false,
                set: None,
                texture: None,
            },
        );
        Ok(())
    }

    /// Commits a service-approved prepared revision; it remains invisible until adoption. Committing changes
    /// candidate state and the pending FIFO, not the active mapping.
    pub fn commit(&self, key: K, revision: u64) -> Result<(), LedgerError> {
        let mut state = self.state.borrow_mut();
        let ready_to_release = {
            let entry = state
                .keys
                .get_mut(&key)
                .ok_or(LedgerError::UnknownRevision)?;
            let item = entry
                .revisions
                .get_mut(&revision)
                .ok_or(LedgerError::UnknownRevision)?;
            if item.state != RevisionState::Prepared {
                return Err(LedgerError::InvalidState {
                    state: item.state,
                    operation: "commit",
                });
            }
            item.state = RevisionState::Committed;
            let old: Vec<u64> = entry
                .revisions
                .iter()
                .filter_map(|(&id, rev)| {
                    (id != revision && rev.state == RevisionState::Committed).then_some(id)
                })
                .collect();
            for old_revision in &old {
                if let Some(old_item) = entry.revisions.get_mut(old_revision) {
                    old_item.state = RevisionState::Retired;
                }
            }
            let ready = old
                .iter()
                .filter_map(|&old_revision| {
                    entry.revisions.get_mut(&old_revision).and_then(|item| {
                        if item.pins == 0 && !item.release_queued {
                            item.release_queued = true;
                            Some(item.id)
                        } else {
                            None
                        }
                    })
                })
                .collect::<Vec<_>>();
            (old, ready)
        };
        let (old, ready) = ready_to_release;
        state.release_candidates.extend(ready);
        for old_revision in old {
            state
                .committed
                .retain(|id| id.key != key || id.revision != old_revision);
        }
        state.committed.push_back(RevisionId { key, revision });
        Ok(())
    }

    /// Rejects a prepared revision for bounded release on a subsequent owner step. A prepared revision is not
    /// active, so rejection does not change any active mapping.
    pub fn reject(&self, key: K, revision: u64) -> Result<(), LedgerError> {
        let mut state = self.state.borrow_mut();
        let id = {
            let item = state
                .keys
                .get_mut(&key)
                .and_then(|e| e.revisions.get_mut(&revision))
                .ok_or(LedgerError::UnknownRevision)?;
            if item.state != RevisionState::Prepared {
                return Err(LedgerError::InvalidState {
                    state: item.state,
                    operation: "reject",
                });
            }
            item.state = RevisionState::Retired;
            if item.pins == 0 {
                item.release_queued = true;
                Some(item.id)
            } else {
                None
            }
        };
        if let Some(id) = id {
            state.release_candidates.push_back(id);
        }
        Ok(())
    }

    /// Retires all revisions for a key and refuses future offers for that key. This clears the active mapping
    /// immediately, even while a frame ticket is Building. A key already resolved by that ticket keeps returning
    /// its cached revision until the ticket ends (its pin keeps the revision alive); a first lookup after
    /// retirement returns `None`.
    pub fn retire_key(&self, key: K) {
        let mut state = self.state.borrow_mut();
        let (ready, empty) = {
            let Some(entry) = state.keys.get_mut(&key) else {
                state.events.push_back(LedgerEvent::KeyRetired { key });
                return;
            };
            entry.retiring = true;
            entry.active = None;
            let mut ready = Vec::new();
            for revision in entry.revisions.values_mut() {
                if revision.state != RevisionState::InFlight {
                    revision.state = RevisionState::Retired;
                    if revision.pins == 0 && !revision.release_queued {
                        revision.release_queued = true;
                        ready.push(revision.id);
                    }
                }
            }
            (ready, entry.revisions.is_empty())
        };
        if empty {
            state.keys.remove(&key);
            state.baseline_charges = state.baseline_charges.saturating_sub(1);
            state.events.push_back(LedgerEvent::KeyRetired { key });
        }
        state.release_candidates.extend(ready);
    }

    /// One owner pass: submits and polls uploads (through `queue`), writes the sets of the uploads that
    /// completed, reads the pending frame's fence and records its proof, releases what is released, and emits
    /// the queued events. It never waits. Returns whether this pass left runnable work, so a caller that sleeps
    /// on wakes pumps again instead.
    #[expect(
        clippy::too_many_lines,
        reason = "one owner pass: submit, poll, write sets, reap the frame proof, release and emit read best together"
    )]
    pub fn pump(&self, queue: &mut RenderQueue, out: &mut Vec<LedgerEvent<K>>) -> bool {
        let mut completed = Vec::new();
        let mut store_failure = None;
        if let Some(owner) = self.owner.borrow_mut().as_mut() {
            owner.submit(queue);
            owner.poll(&mut completed);
            store_failure = owner.take_failure();
        }
        let mut displaced = Vec::new();
        let mut wake_room = false;
        let mut descriptors = self.descriptors.borrow_mut();
        let mut writes: Vec<(vk::DescriptorSet, vk::ImageView)> = Vec::new();
        let more;
        {
            let mut state = self.state.borrow_mut();
            state.completed_pending.extend(completed);
            while let Some(result) = state.completed_pending.pop_front() {
                match result {
                    Completed::Ready(texture) if texture.tag == RevisionKey::Checker => {
                        // The checker is resident once its permanent set is written (a set of the revision pool the
                        // pool has one more of than its configured revisions need).
                        let made = descriptors.as_mut().map_or_else(
                            || Err("the ledger was released".to_owned()),
                            |descriptors| descriptors.checker_set(texture.view()),
                        );
                        match made {
                            Ok(set) => state.checker = CheckerSlot::Resident { texture, set },
                            Err(reason) => {
                                state.checker = CheckerSlot::Failed(format!(
                                    "the checker's descriptor set could not be made: {reason}"
                                ));
                                displaced.push(texture);
                            }
                        }
                    }
                    Completed::Ready(texture) => {
                        let RevisionKey::Revision(id) = texture.tag else {
                            continue;
                        };
                        if !state
                            .keys
                            .get(&id.key)
                            .is_some_and(|entry| entry.revisions.contains_key(&id.revision))
                        {
                            displaced.push(texture);
                            continue;
                        }
                        let acquired = descriptors.as_mut().map_or_else(
                            || Err("the ledger was released".to_owned()),
                            Descriptors::acquire,
                        );
                        let set = match acquired {
                            Ok(set) => set,
                            Err(reason) => {
                                remove_revision(&mut state, id, &mut displaced, &mut wake_room);
                                push_upload_failed(&mut state, id, reason);
                                displaced.push(texture);
                                continue;
                            }
                        };
                        writes.push((set, texture.view()));
                        let retiring = state.keys.get(&id.key).is_some_and(|entry| entry.retiring);
                        if let Some(item) = state
                            .keys
                            .get_mut(&id.key)
                            .and_then(|e| e.revisions.get_mut(&id.revision))
                        {
                            item.texture = Some(texture);
                            item.set = Some(set);
                            item.state = if retiring {
                                RevisionState::Retired
                            } else {
                                RevisionState::Prepared
                            };
                            if !retiring {
                                state.events.push_back(LedgerEvent::Prepared {
                                    key: id.key,
                                    revision: id.revision,
                                });
                            } else if item.pins == 0 && !item.release_queued {
                                item.release_queued = true;
                                state.release_candidates.push_back(id);
                            }
                        }
                    }
                    Completed::Failed {
                        tag: RevisionKey::Checker,
                        reason,
                    } => {
                        state.checker = CheckerSlot::Failed(format!(
                            "the checker texture upload failed: {reason}"
                        ));
                    }
                    Completed::Failed {
                        tag: RevisionKey::Revision(id),
                        reason,
                    } => {
                        remove_revision(&mut state, id, &mut displaced, &mut wake_room);
                        push_upload_failed(&mut state, id, reason);
                    }
                }
            }
            if let Some(message) = store_failure {
                latch_failure(&mut state, message);
            }
            // Every set a completed upload needs is written now, before its `Prepared` event leaves.
            if let Some(descriptors) = descriptors.as_ref() {
                descriptors.write(&writes);
            }
            reap_pending(&mut state, &self.context);
            release_retired(
                &mut state,
                &mut displaced,
                &mut wake_room,
                descriptors.as_mut(),
            );
            out.extend(state.events.drain(..));
            more = !state.release_candidates.is_empty() || !state.completed_pending.is_empty();
        }
        drop(descriptors);
        drop(displaced);
        if wake_room {
            self.wake_room();
        }
        more
    }

    /// A **non-destructive** look for a latched GPU failure: it submits nothing, polls no upload, reaps no ticket
    /// and releases no image, so it is safe while the end is not yet proven. The failure is latched like the
    /// ordinary pump's and returned once; the ordinary pump never reports it again.
    pub fn observe_failure(&self) -> Option<String> {
        let store_failure = self
            .owner
            .borrow()
            .as_ref()
            .and_then(TextureOwner::peek_failure);
        let mut state = self.state.borrow_mut();
        if let Some(message) = store_failure {
            latch_failure(&mut state, message);
        }
        let position = state
            .events
            .iter()
            .position(|event| matches!(event, LedgerEvent::GpuFailure { .. }))?;
        match state.events.remove(position) {
            Some(LedgerEvent::GpuFailure { message }) => Some(message),
            _ => None,
        }
    }

    /// Whether a prepared, closed envelope waits for the owner to submit it (tests only: a phase handshake that
    /// takes and releases nothing).
    #[doc(hidden)]
    #[must_use]
    pub fn envelope_ready_unsubmitted(&self) -> bool {
        self.owner
            .borrow()
            .as_ref()
            .is_some_and(TextureOwner::envelope_ready_unsubmitted)
    }

    /// Submits the closed upload batch, as the first step of `pump` does, and nothing else: no poll, so no
    /// completion is observed and no set is written (tests only: it lets a case order the GPU's completion of an
    /// upload before the ledger's observation of it).
    #[doc(hidden)]
    pub fn test_submit_upload(&self, queue: &mut RenderQueue) -> bool {
        self.owner
            .borrow_mut()
            .as_mut()
            .is_some_and(|owner| owner.submit(queue))
    }

    /// Waits until the submitted upload batch's fence has signalled or `timeout` passes, **recording nothing**
    /// (tests only: no proof, no set, no release). `true` when the GPU finished it (or none is in flight).
    #[doc(hidden)]
    #[must_use]
    pub fn test_wait_upload(&self, timeout: Duration) -> bool {
        self.owner
            .borrow()
            .as_ref()
            .is_none_or(|owner| owner.test_wait_submitted(timeout))
    }

    /// Whether the store has an upload batch in flight, so a render loop can bound its idle wait.
    #[must_use]
    pub fn has_submitted(&self) -> bool {
        self.owner
            .borrow()
            .as_ref()
            .is_some_and(TextureOwner::has_submitted)
    }

    /// Whether committed revisions wait for a frame boundary (the presenter then wants a frame, so adoption does
    /// not wait for an unrelated redraw).
    #[must_use]
    pub fn has_committed(&self) -> bool {
        !self.state.borrow().committed.is_empty()
    }

    /// Whether a submitted frame's proof has not been recorded yet (a render loop bounds its idle wait on it).
    #[must_use]
    pub fn frame_pending(&self) -> bool {
        self.state.borrow().pending.is_some()
    }

    /// Whether bounded release work is waiting (a retired revision whose last pin just went).
    #[must_use]
    pub fn has_release_work(&self) -> bool {
        !self.state.borrow().release_candidates.is_empty()
    }

    /// The state of one revision, or `None` once it is released (tests only).
    #[doc(hidden)]
    #[must_use]
    pub fn test_revision_state(&self, key: K, revision: u64) -> Option<RevisionState> {
        self.state
            .borrow()
            .keys
            .get(&key)
            .and_then(|entry| entry.revisions.get(&revision))
            .map(|item| item.state)
    }

    /// The revisions the ledger holds in its own maps, released ones excluded (tests only: a read of the live
    /// maps, not a counter).
    #[doc(hidden)]
    #[must_use]
    pub fn test_live_revisions(&self) -> usize {
        self.state
            .borrow()
            .keys
            .values()
            .map(|entry| entry.revisions.len())
            .sum()
    }

    /// One adoption pass in commit order, up to the supplied and configured caps. While a frame ticket is
    /// Building, committed revisions remain pending and no active mapping changes (CK-150); adoption resumes
    /// after the ticket ends.
    pub fn adopt(&self, limit: usize, out: &mut Vec<(K, u64)>) {
        let mut displaced = Vec::new();
        let mut wake_room = false;
        {
            let mut descriptors = self.descriptors.borrow_mut();
            let mut state = self.state.borrow_mut();
            let cap = limit.min(self.limits.max_adoptions_per_frame);
            let mut adopted = 0;
            let resolution_open = state.building;
            let mut remaining = state.committed.len();
            while !resolution_open && adopted < cap && remaining > 0 {
                remaining -= 1;
                let Some(id) = state.committed.pop_front() else {
                    break;
                };
                let (transfer, released_old) = {
                    let Some(entry) = state.keys.get_mut(&id.key) else {
                        continue;
                    };
                    let Some(next) = entry.revisions.get(&id.revision) else {
                        continue;
                    };
                    if next.state != RevisionState::Committed || next.texture.is_none() {
                        continue;
                    }
                    let old_baseline = entry.baseline;
                    let mut charge_delta = 0_isize;
                    if let Some(old) = old_baseline.filter(|old| *old != id.revision) {
                        if let Some(previous) = entry.revisions.get_mut(&old)
                            && !previous.extra
                        {
                            previous.extra = true;
                            charge_delta += 1;
                        }
                        if let Some(next) = entry.revisions.get_mut(&id.revision)
                            && next.extra
                        {
                            next.extra = false;
                            charge_delta -= 1;
                        }
                    }
                    let mut released_old = None;
                    if let Some(old_active) = entry.active.replace(id.revision)
                        && old_active != id.revision
                        && let Some(old) = entry.revisions.get_mut(&old_active)
                    {
                        old.state = RevisionState::Retired;
                        if old.pins == 0 && !old.release_queued {
                            old.release_queued = true;
                            released_old = Some(old.id);
                        }
                    }
                    entry.baseline = Some(id.revision);
                    if let Some(next) = entry.revisions.get_mut(&id.revision) {
                        next.state = RevisionState::Active;
                    }
                    (charge_delta, released_old)
                };
                if let Some(old) = released_old {
                    state.release_candidates.push_back(old);
                }
                if transfer != 0 {
                    state.extra_charges = state.extra_charges.saturating_add_signed(transfer);
                    if transfer < 0 {
                        wake_room = true;
                    }
                }
                out.push((id.key, id.revision));
                adopted += 1;
            }
            if adopted != 0 {
                let now = now_ns();
                state.adopted_total = state.adopted_total.saturating_add(adopted as u64);
                if state.first_adopted_ns == 0 {
                    state.first_adopted_ns = now;
                }
                state.last_adopted_ns = now;
            }
            release_retired(
                &mut state,
                &mut displaced,
                &mut wake_room,
                descriptors.as_mut(),
            );
        }
        drop(displaced);
        if wake_room {
            self.wake_room();
        }
    }

    /// Opens the single building frame ticket and takes its fence: the one whose previous submission's proof is
    /// recorded (reset now, and only now), else the result is busy.
    pub fn begin_frame(&self) -> Result<FrameTicket<'_, K>, LedgerError> {
        let mut state = self.state.borrow_mut();
        state.frames_begun = true;
        if state.ending || state.failure_latched {
            return Err(LedgerError::Ended);
        }
        if state.building {
            return Err(LedgerError::TicketBusy);
        }
        let Some(taken) = state.proofs.take() else {
            return Err(LedgerError::TicketBusy);
        };
        if taken.reset {
            let fence = state.proofs.fence(taken.slot);
            // SAFETY: the fence belongs to this device and was seen signalled (its previous submission's
            // proof is recorded), and no queue operation waits on it.
            if let Err(code) = unsafe { self.context.device().reset_fences(&[fence]) } {
                state.proofs.untake(taken.slot);
                let message = format!("vkResetFences(frame) failed: {code:?}");
                latch_failure(&mut state, message.clone());
                return Err(LedgerError::Native(message));
            }
            state.proofs.was_reset(taken.slot);
        }
        state.building = true;
        let spare_resolved = state.spare_resolved.take().unwrap_or_default();
        let spare_pinned = state.spare_pinned.take().unwrap_or_default();
        Ok(FrameTicket {
            ledger: self,
            pins: RefCell::new(Vec::new()),
            pinned: RefCell::new(spare_pinned),
            resolved: RefCell::new(spare_resolved),
            slot: taken.slot,
            phase: Cell::new(TicketPhase::Building),
        })
    }

    /// Whether the proof of frame submission `submission` is recorded: true once `pump` or `prove` saw its
    /// fence signalled, and it stays true after the fence is reset and reused. A later submission is never proven
    /// by an earlier signal. This is the whole caller contract for frame completion, teardown included.
    #[must_use]
    pub fn frame_proven(&self, submission: FrameSubmission) -> bool {
        self.frame_proven_number(submission.0)
    }

    /// [`frame_proven`](Self::frame_proven) for a submission number kept as a plain number.
    pub(crate) fn frame_proven_number(&self, number: u64) -> bool {
        self.state.borrow().proofs.is_proven(number)
    }

    /// Sets how many entries a ticket's map may have held for the ledger to keep it (empty, with its capacity)
    /// for the next ticket; 0, the default, keeps none. It can be set **once, and only before the first
    /// [`begin_frame`](Self::begin_frame)**, so the limit never changes while a ticket or a kept map can exist.
    pub fn set_spare_limit(&self, entries: usize) -> Result<(), LedgerError> {
        let mut state = self.state.borrow_mut();
        if state.spare_limit_set || state.frames_begun || state.building {
            return Err(LedgerError::SpareLimitFixed);
        }
        state.spare_limit = entries;
        state.spare_limit_set = true;
        Ok(())
    }

    /// Proves the worker, the uploads and the frame submissions against one absolute deadline. The pending
    /// frame is waited for with a bounded `vkWaitForFences`, and its proof is recorded like `pump`'s.
    pub fn prove(&self, deadline: Instant) -> LedgerProof {
        self.state.borrow_mut().ending = true;
        let incomplete = |reason: &str| LedgerProof {
            ledger_id: self.id,
            store: None,
            complete: false,
            reason: Some(reason.to_owned()),
        };
        {
            let state = self.state.borrow();
            if state.building {
                return incomplete("frame ticket is building");
            }
            if !state.uncertain.is_empty() {
                return incomplete("frame outcome is uncertain");
            }
        }
        let pending = {
            let state = self.state.borrow();
            state
                .pending
                .as_ref()
                .map(|pending| (state.proofs.fence(pending.slot), pending.submission))
        };
        if let Some((fence, submission)) = pending {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let nanoseconds = u64::try_from(remaining.as_nanos()).unwrap_or(u64::MAX);
            // SAFETY: the fence belongs to this device; the wait is bounded by the deadline.
            let waited = unsafe {
                self.context
                    .device()
                    .wait_for_fences(&[fence], true, nanoseconds)
            };
            if waited.is_err() {
                return incomplete("frame submission remained outstanding");
            }
            let mut state = self.state.borrow_mut();
            state.proofs.record_proven(submission);
            reap_pending(&mut state, &self.context);
        }
        let store = self
            .owner
            .borrow_mut()
            .as_mut()
            .map(|owner| owner.prove(deadline));
        let Some(store) = store else {
            return incomplete("upload owner already released");
        };
        let complete = store.is_complete();
        LedgerProof {
            ledger_id: self.id,
            store: Some(store),
            complete,
            reason: (!complete).then(|| "upload worker or batch remained outstanding".to_owned()),
        }
    }

    /// Destroys, only with this ledger's complete proof and in dependency order: the revisions' views and
    /// images, then the store's blocks, staging, fences and command pools, then the frame fences, then the
    /// descriptor pools, layouts and samplers.
    pub fn release(&self, proof: LedgerProof) -> Result<(), String> {
        if proof.ledger_id != self.id || !proof.complete {
            return Err(proof
                .reason
                .unwrap_or_else(|| "proof incomplete or belongs to another ledger".to_owned()));
        }
        if self.released.get() {
            return Ok(());
        }
        let store = proof
            .store
            .ok_or_else(|| "proof has no upload-store component".to_owned())?;
        let mut displaced = Vec::new();
        let fences = {
            let mut state = self.state.borrow_mut();
            for (_, mut entry) in state.keys.drain() {
                for (_, mut revision) in entry.revisions.drain() {
                    if let Some(texture) = revision.texture.take() {
                        displaced.push(texture);
                    }
                }
            }
            state.completed_pending.clear();
            state.committed.clear();
            state.compat_hold = None;
            if let CheckerSlot::Resident { texture, .. } =
                std::mem::replace(&mut state.checker, CheckerSlot::None)
            {
                displaced.push(texture);
            }
            state.pending.take();
            state.baseline_charges = 0;
            state.extra_charges = 0;
            state.live_pins = 0;
            state.proofs.fences()
        };
        drop(displaced);
        if let Some(owner) = self.owner.borrow_mut().as_mut() {
            owner.release(store);
        }
        for fence in fences {
            // SAFETY: every submission using the frame fences is proven finished (or none was made).
            unsafe { self.context.device().destroy_fence(fence, None) };
        }
        if let Some(mut descriptors) = self.descriptors.borrow_mut().take() {
            descriptors.destroy();
        }
        self.released.set(true);
        Ok(())
    }

    /// Performs proof then release with one deadline.
    pub fn shutdown(&self, deadline: Instant) -> LedgerCleanup {
        let proof = self.prove(deadline);
        if !proof.complete {
            return LedgerCleanup::Unproven {
                reason: proof
                    .reason
                    .unwrap_or_else(|| "ledger proof incomplete".to_owned()),
            };
        }
        match self.release(proof) {
            Ok(()) => LedgerCleanup::Proven,
            Err(reason) => LedgerCleanup::Unproven { reason },
        }
    }

    fn wake_room(&self) {
        // The one containment of the store's hooks: a panic payload whose `Drop` panics is disposed of too.
        crate::textures::notify_wake(self.wakes.room.as_ref());
    }
}

impl<K: Copy + Eq + Hash + Send + 'static> Drop for TextureLedger<K> {
    fn drop(&mut self) {
        if self.released.get() {
            return;
        }
        // An unproven end keeps the whole graph and the context, the crate's existing pattern.
        let state = std::mem::replace(
            self.state.get_mut(),
            LedgerState::empty(FenceProofs::new(&[])),
        );
        std::mem::forget(state);
        if let Some(owner) = self.owner.get_mut().take() {
            std::mem::forget(owner);
        }
        std::mem::forget(self.descriptors.get_mut().take());
        std::mem::forget(Arc::clone(&self.context));
    }
}

/// Two unsignalled fences for the frames.
fn create_fences(context: &Arc<VulkanContext>) -> Result<Vec<vk::Fence>, Error> {
    let device = context.device();
    let mut fences = Vec::with_capacity(FRAME_FENCES);
    for _ in 0..FRAME_FENCES {
        // SAFETY: the device is live.
        match unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) } {
            Ok(fence) => fences.push(fence),
            Err(code) => {
                for fence in fences {
                    // SAFETY: never submitted.
                    unsafe { device.destroy_fence(fence, None) };
                }
                return Err(vk_error("vkCreateFence(frame)", code));
            }
        }
    }
    Ok(fences)
}

fn latch_failure<K>(state: &mut LedgerState<K>, message: String) {
    state.failure_latched = true;
    if !state.failure_event_emitted {
        state.failure_event_emitted = true;
        state.events.push_back(LedgerEvent::GpuFailure { message });
    }
}

fn remove_revision<K: Copy + Eq + Hash>(
    state: &mut LedgerState<K>,
    id: RevisionId<K>,
    displaced: &mut Vec<PreparedTexture<RevisionKey<K>>>,
    wake_room: &mut bool,
) {
    let (removed_extra, baseline_removed, empty, retiring) = {
        let Some(entry) = state.keys.get_mut(&id.key) else {
            return;
        };
        let Some(mut removed) = entry.revisions.remove(&id.revision) else {
            return;
        };
        if let Some(texture) = removed.texture.take() {
            displaced.push(texture);
        }
        if entry.active == Some(id.revision) {
            entry.active = None;
        }
        let baseline_removed = entry.baseline == Some(id.revision);
        (
            removed.extra,
            baseline_removed,
            entry.revisions.is_empty(),
            entry.retiring,
        )
    };
    if removed_extra {
        state.extra_charges = state.extra_charges.saturating_sub(1);
        *wake_room = true;
    } else if baseline_removed {
        promote_baseline(state, id.key);
    }
    if empty {
        state.keys.remove(&id.key);
        state.baseline_charges = state.baseline_charges.saturating_sub(1);
        if retiring {
            state
                .events
                .push_back(LedgerEvent::KeyRetired { key: id.key });
        }
    }
}

fn promote_baseline<K: Copy + Eq + Hash>(state: &mut LedgerState<K>, key: K) {
    let Some(entry) = state.keys.get_mut(&key) else {
        return;
    };
    if let Some((&newest, replacement)) = entry.revisions.iter_mut().max_by_key(|(_, r)| r.sequence)
    {
        if replacement.extra {
            replacement.extra = false;
            state.extra_charges = state.extra_charges.saturating_sub(1);
        }
        entry.baseline = Some(newest);
    } else {
        entry.baseline = None;
    }
}

/// Reads the pending frame's fence. If it is signalled, **records the proof first**, then unpins the ticket's
/// revisions (a retired revision with no pin left becomes a release candidate). This is the only place, apart
/// from `prove`, that reads a frame fence.
fn reap_pending<K: Copy + Eq + Hash>(state: &mut LedgerState<K>, context: &VulkanContext) {
    let Some(pending) = state.pending.as_ref() else {
        return;
    };
    let fence = state.proofs.fence(pending.slot);
    // SAFETY: the fence belongs to this device and was submitted with the pending ticket.
    match unsafe { context.device().get_fence_status(fence) } {
        Ok(true) => {}
        Ok(false) => return,
        Err(code) => {
            latch_failure(state, format!("vkGetFenceStatus(frame) failed: {code:?}"));
            return;
        }
    }
    let submission = pending.submission;
    state.proofs.record_proven(submission);
    let Some(pending) = state.pending.take() else {
        return;
    };
    let mut released = Vec::new();
    for id in pending.pins {
        if let Some(rev) = state
            .keys
            .get_mut(&id.key)
            .and_then(|e| e.revisions.get_mut(&id.revision))
        {
            rev.pins = rev.pins.saturating_sub(1);
            state.live_pins = state.live_pins.saturating_sub(1);
            if rev.state == RevisionState::Retired && rev.pins == 0 && !rev.release_queued {
                rev.release_queued = true;
                released.push(id);
            }
        }
    }
    state.release_candidates.extend(released);
}

/// Releases the retired revisions that nothing reads, a bounded number per call: the view and the image go with
/// the texture (`displaced`), and the set goes back on the free list.
fn release_retired<K: Copy + Eq + Hash>(
    state: &mut LedgerState<K>,
    displaced: &mut Vec<PreparedTexture<RevisionKey<K>>>,
    wake_room: &mut bool,
    mut descriptors: Option<&mut Descriptors>,
) {
    for _ in 0..RELEASES_PER_CALL {
        let Some(id) = state.release_candidates.pop_front() else {
            break;
        };
        let Some((candidate_state, candidate_pins)) = state
            .keys
            .get(&id.key)
            .and_then(|entry| entry.revisions.get(&id.revision))
            .map(|candidate| (candidate.state, candidate.pins))
        else {
            continue;
        };
        if candidate_state != RevisionState::Retired {
            continue;
        }
        if candidate_pins != 0 {
            // A cached ticket resolution can pin a Retired revision after retirement first queued it. Let the
            // final unpin enqueue it again instead of leaving a stale queued flag behind.
            if let Some(candidate) = state
                .keys
                .get_mut(&id.key)
                .and_then(|entry| entry.revisions.get_mut(&id.revision))
            {
                candidate.release_queued = false;
            }
            continue;
        }
        let (extra, baseline_removed, empty, retiring) = {
            let Some(entry) = state.keys.get_mut(&id.key) else {
                continue;
            };
            let Some(candidate) = entry.revisions.get_mut(&id.revision) else {
                continue;
            };
            candidate.release_queued = false;
            let Some(mut removed) = entry.revisions.remove(&id.revision) else {
                continue;
            };
            if let Some(texture) = removed.texture.take() {
                displaced.push(texture);
            }
            if let (Some(set), Some(descriptors)) = (removed.set.take(), descriptors.as_deref_mut())
            {
                descriptors.recycle(set);
            }
            if entry.active == Some(id.revision) {
                entry.active = None;
            }
            (
                removed.extra,
                entry.baseline == Some(id.revision),
                entry.revisions.is_empty(),
                entry.retiring,
            )
        };
        if extra {
            state.extra_charges = state.extra_charges.saturating_sub(1);
            *wake_room = true;
        } else if baseline_removed {
            promote_baseline(state, id.key);
        }
        if empty {
            state.keys.remove(&id.key);
            state.baseline_charges = state.baseline_charges.saturating_sub(1);
            if retiring {
                state
                    .events
                    .push_back(LedgerEvent::KeyRetired { key: id.key });
            }
        }
    }
}

/// Proof tying a ledger and upload-owner proof together.
pub struct LedgerProof {
    ledger_id: u64,
    store: Option<Proof>,
    complete: bool,
    reason: Option<String>,
}

impl LedgerProof {
    /// Whether every worker, upload and frame submission was proven complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// What remained outstanding, when the proof is incomplete.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

/// Shutdown result after the non-destructive proof and release stages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerCleanup {
    /// Everything was proven finished and destroyed.
    Proven,
    /// Something remained outstanding; everything is retained.
    Unproven {
        /// Why.
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TicketPhase {
    Building,
    Ended,
}

/// One frame's exact revision pins. Dropping before an explicit end conservatively retains pins.
pub struct FrameTicket<'a, K: Copy + Eq + Hash + Send + 'static> {
    ledger: &'a TextureLedger<K>,
    pins: RefCell<Vec<RevisionId<K>>>,
    /// The revisions in `pins`, for the "already pinned" test: a scan of `pins` per distinct key would make
    /// resolving a frame's textures quadratic in their number (CK-116 found it at 4,096 distinct textures).
    pinned: RefCell<FrameSet<RevisionId<K>>>,
    resolved: RefCell<FrameMap<K, Option<u64>>>,
    /// The frame fence this ticket took at `begin_frame`.
    slot: usize,
    phase: Cell<TicketPhase>,
}

#[expect(
    clippy::missing_errors_doc,
    reason = "transition failures are named by LedgerError"
)]
impl<K: Copy + Eq + Hash + Send + 'static> FrameTicket<'_, K> {
    /// Resolves the exact active revision, pinning it at most once per key. The first successful lookup is
    /// cached for this ticket: an explicit later [`TextureLedger::retire_key`] does not change a cached answer,
    /// and repeated calls may still resolve that pinned Retired revision until the ticket ends. A first lookup
    /// after retirement returns `None`.
    pub fn resolve(&self, key: K) -> Option<FrameTexture<'_>> {
        if self.phase.get() != TicketPhase::Building {
            return None;
        }
        let mut state = self.ledger.state.borrow_mut();
        // One lookup of the key's record serves both the active revision (read once per key and cached) and
        // the revision's own record.
        let entry = state.keys.get_mut(&key);
        let (revision_id, cached) = match self.resolved.borrow_mut().entry(key) {
            Entry::Occupied(previous) => (*previous.get(), true),
            Entry::Vacant(slot) => (*slot.insert(entry.as_ref().and_then(|e| e.active)), false),
        };
        let revision_id = revision_id?;
        let id = RevisionId {
            key,
            revision: revision_id,
        };
        let (texture, new_pin) = {
            let revision = entry?.revisions.get_mut(&revision_id)?;
            if revision.state != RevisionState::Active
                && !(cached && revision.state == RevisionState::Retired)
            {
                return None;
            }
            let set = revision.set?;
            let texture = revision.texture.as_ref()?;
            let found = (
                texture.image(),
                texture.view(),
                set,
                texture.width,
                texture.height,
            );
            let new_pin = self.pinned.borrow_mut().insert(id);
            if new_pin {
                revision.pins += 1;
                self.pins.borrow_mut().push(id);
            }
            (found, new_pin)
        };
        if new_pin {
            state.live_pins += 1;
        }
        let (image, view, set, width, height) = texture;
        Some(FrameTexture {
            image,
            view,
            set,
            width,
            height,
            revision: revision_id,
            _ticket: PhantomData,
        })
    }

    /// Resolves one exact **currently held** revision (see [`TextureLedger::hold_compat`]) whatever its state: it may be
    /// retired or superseded, but the hold keeps it alive. Pins it for this ticket like [`resolve`](Self::resolve). Each
    /// call checks the current hold rather than caching it: replacing or releasing the hold during a building ticket makes
    /// a repeated lookup return `None` (CK-150).
    pub fn resolve_revision(&self, key: K, revision: u64) -> Option<FrameTexture<'_>> {
        if self.phase.get() != TicketPhase::Building {
            return None;
        }
        let id = RevisionId { key, revision };
        let mut state = self.ledger.state.borrow_mut();
        if state.compat_hold != Some(id) {
            return None;
        }
        let (found, new_pin) = {
            let held = state.keys.get_mut(&key)?.revisions.get_mut(&revision)?;
            let set = held.set?;
            let texture = held.texture.as_ref()?;
            let found = (
                texture.image(),
                texture.view(),
                set,
                texture.width,
                texture.height,
            );
            let new_pin = self.pinned.borrow_mut().insert(id);
            if new_pin {
                held.pins += 1;
                self.pins.borrow_mut().push(id);
            }
            (found, new_pin)
        };
        if new_pin {
            state.live_pins += 1;
        }
        let (image, view, set, width, height) = found;
        Some(FrameTexture {
            image,
            view,
            set,
            width,
            height,
            revision,
            _ticket: PhantomData,
        })
    }

    /// Performs the frame's `vkQueueSubmit` through `queue`, with this ticket's fence, waiting on `waits` and
    /// signalling `signals`, and returns the submission's number. **The fence is never visible to the caller.**
    ///
    /// A pending submission whose proof `pump` has not recorded makes this [`LedgerError::TicketBusy`]; a submit
    /// that fails with out of memory leaves the ticket Building (end it with [`not_submitted`](Self::not_submitted));
    /// one that may have reached the GPU (device loss) keeps the ticket's pins for good and latches the failure.
    pub fn submit(
        &self,
        queue: &mut RenderQueue,
        command_buffer: vk::CommandBuffer,
        waits: &[(vk::Semaphore, vk::PipelineStageFlags)],
        signals: &[vk::Semaphore],
    ) -> Result<FrameSubmission, LedgerError> {
        if self.phase.get() != TicketPhase::Building {
            return Err(LedgerError::InvalidState {
                state: RevisionState::Active,
                operation: "submit",
            });
        }
        let fence = {
            let state = self.ledger.state.borrow();
            if state.failure_latched {
                return Err(LedgerError::Ended);
            }
            if state.pending.is_some() {
                return Err(LedgerError::TicketBusy);
            }
            state.proofs.fence(self.slot)
        };
        let wait_semaphores: Vec<vk::Semaphore> =
            waits.iter().map(|&(semaphore, _)| semaphore).collect();
        let wait_stages: Vec<vk::PipelineStageFlags> =
            waits.iter().map(|&(_, stage)| stage).collect();
        let buffers = [command_buffer];
        let info = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(&buffers)
            .signal_semaphores(signals);
        if let Err(code) = queue.submit(&info, fence) {
            let message = format!("vkQueueSubmit(frame) failed: {code:?}");
            if matches!(
                code,
                vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY
            ) {
                return Err(LedgerError::Native(message));
            }
            let pins = std::mem::take(&mut *self.pins.borrow_mut());
            let mut state = self.ledger.state.borrow_mut();
            state.building = false;
            state.uncertain.push(UncertainTicket { _pins: pins });
            latch_failure(&mut state, message.clone());
            self.phase.set(TicketPhase::Ended);
            return Err(LedgerError::Native(message));
        }
        let pins = std::mem::take(&mut *self.pins.borrow_mut());
        let mut state = self.ledger.state.borrow_mut();
        let number = state.proofs.submitted(self.slot);
        state.building = false;
        state.pending = Some(PendingTicket {
            pins,
            slot: self.slot,
            submission: number,
        });
        self.phase.set(TicketPhase::Ended);
        Ok(FrameSubmission(number))
    }

    /// Explicitly ends a building ticket that never submitted GPU work: no fence was used and no number is
    /// assigned.
    pub fn discarded(self) -> Result<(), LedgerError> {
        self.end_unsubmitted("discarded")
    }

    /// Ends a ticket whose submit did not happen (a refusal, an out-of-memory submit): the same as
    /// [`discarded`](Self::discarded), named for the caller that tried to submit.
    pub fn not_submitted(self) -> Result<(), LedgerError> {
        self.end_unsubmitted("not_submitted")
    }

    fn end_unsubmitted(&self, operation: &'static str) -> Result<(), LedgerError> {
        if self.phase.get() != TicketPhase::Building {
            return Err(LedgerError::InvalidState {
                state: RevisionState::Active,
                operation,
            });
        }
        let pins = std::mem::take(&mut *self.pins.borrow_mut());
        let mut state = self.ledger.state.borrow_mut();
        release_pins(&mut state, pins);
        state.proofs.untake(self.slot);
        state.building = false;
        self.phase.set(TicketPhase::Ended);
        Ok(())
    }

    /// Gives the ticket's `resolved` map and `pinned` set back to the ledger for the next ticket: each cleared,
    /// and kept only if the entries it held (counted before clearing) are within the ledger's `spare_limit`, so
    /// a kept map's capacity never exceeds what the standard map allocates for that many entries.
    fn return_maps(&mut self) {
        let mut resolved = std::mem::take(self.resolved.get_mut());
        let mut pinned = std::mem::take(self.pinned.get_mut());
        // A ledger state that is borrowed (a drop during a panic inside the ledger) forgoes the reuse.
        let Ok(mut state) = self.ledger.state.try_borrow_mut() else {
            return;
        };
        let limit = state.spare_limit;
        if resolved.len() <= limit {
            resolved.clear();
            state.spare_resolved = Some(resolved);
        }
        if pinned.len() <= limit {
            pinned.clear();
            state.spare_pinned = Some(pinned);
        }
    }
}

impl<K: Copy + Eq + Hash + Send + 'static> Drop for FrameTicket<'_, K> {
    fn drop(&mut self) {
        self.return_maps();
        if self.phase.get() == TicketPhase::Ended {
            return;
        }
        let pins = std::mem::take(&mut *self.pins.borrow_mut());
        let mut state = self.ledger.state.borrow_mut();
        state.building = false;
        state.uncertain.push(UncertainTicket { _pins: pins });
        latch_failure(
            &mut state,
            "frame ticket dropped without an explicit outcome".to_owned(),
        );
        self.phase.set(TicketPhase::Ended);
    }
}

/// One failed upload, classified before any consumer sees it. Every way an offered revision can fail (the
/// store's per-item errors, the descriptor sets running out) goes through here.
fn push_upload_failed<K: Copy + Eq + Hash>(
    state: &mut LedgerState<K>,
    id: RevisionId<K>,
    reason: String,
) {
    if state.compat_key == Some(id.key) {
        state.events.push_back(LedgerEvent::CompatUploadFailed {
            key: id.key,
            reason: reason.clone(),
        });
    }
    state.events.push_back(LedgerEvent::UploadFailed {
        key: id.key,
        revision: id.revision,
        reason,
    });
}

fn release_pins<K: Copy + Eq + Hash>(state: &mut LedgerState<K>, pins: Vec<RevisionId<K>>) {
    let mut released = Vec::new();
    for id in pins {
        if let Some(revision) = state
            .keys
            .get_mut(&id.key)
            .and_then(|e| e.revisions.get_mut(&id.revision))
        {
            revision.pins = revision.pins.saturating_sub(1);
            state.live_pins = state.live_pins.saturating_sub(1);
            if revision.state == RevisionState::Retired
                && revision.pins == 0
                && !revision.release_queued
            {
                revision.release_queued = true;
                released.push(id);
            }
        }
    }
    state.release_candidates.extend(released);
}

/// Frame-scoped reference to one exact active revision: the image, its view and the revision's descriptor set,
/// valid while the ticket pins the revision.
///
/// A resolved texture cannot outlive its ticket:
///
/// ```compile_fail
/// use ck_gpu_vulkan::{FrameTexture, TextureLedger};
/// fn escape<'a>(ledger: &'a TextureLedger<u32>) -> FrameTexture<'a> {
///     let ticket = ledger.begin_frame().unwrap();
///     ticket.resolve(1).unwrap()
/// }
/// ```
pub struct FrameTexture<'a> {
    image: vk::Image,
    view: vk::ImageView,
    set: vk::DescriptorSet,
    width: u32,
    height: u32,
    revision: u64,
    _ticket: PhantomData<&'a ()>,
}

impl FrameTexture<'_> {
    /// The revision this texture is.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The revision's descriptor set (set 1 of the ledger's pipeline layout), written before `Prepared`.
    #[must_use]
    pub fn descriptor_set(&self) -> vk::DescriptorSet {
        self.set
    }

    /// The image, in `SHADER_READ_ONLY_OPTIMAL`.
    #[must_use]
    pub fn image(&self) -> vk::Image {
        self.image
    }

    /// The image view the set names.
    #[must_use]
    pub fn view(&self) -> vk::ImageView {
        self.view
    }

    /// The image's width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The image's height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }
}

#[cfg(test)]
mod tests {
    /// Zero means "no adoption yet" to the stats' readers, so a time is never zero, whatever the clock's resolution.
    #[test]
    fn an_adoption_time_is_never_zero() {
        for _ in 0..1000 {
            assert_ne!(super::now_ns(), 0);
        }
    }

    use super::{FrameSubmission, LedgerLimits, TextureLedger};

    /// The ledger moves to the render thread; it is never shared.
    #[test]
    fn the_ledger_is_send() {
        fn send<T: Send>() {}
        send::<TextureLedger<u64>>();
    }

    #[test]
    fn limits_keep_metals_defaults_and_ranges() {
        assert_eq!(
            LedgerLimits::default(),
            LedgerLimits {
                max_extra_revisions: 1_024,
                max_adoptions_per_frame: 256
            }
        );
        assert!(LedgerLimits::default().validate().is_ok());
        for limits in [
            LedgerLimits {
                max_extra_revisions: 0,
                ..LedgerLimits::default()
            },
            LedgerLimits {
                max_adoptions_per_frame: 65_537,
                ..LedgerLimits::default()
            },
        ] {
            assert!(limits.validate().is_err(), "{limits:?}");
        }
    }

    #[test]
    fn a_submission_number_orders_and_reports() {
        assert!(FrameSubmission(1) < FrameSubmission(2));
        assert_eq!(FrameSubmission(7).number(), 7);
    }
}
