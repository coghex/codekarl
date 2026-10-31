//! Render-owner ledger for uploaded native texture revisions.
//!
//! Upload completion, service commitment, frame-boundary adoption and native reclamation are
//! separate transitions. A revision remains owned until its upload and every exact frame reader
//! have completion proofs.

use std::{
    cell::{Cell, RefCell},
    collections::{VecDeque, hash_map::Entry},
    hash::Hash,
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    Error,
    bindless::{
        BindlessLimits, MembershipBudget, RESIDENCY_CHANGES_PER_PASS, ResidencySet, ResourceSlots,
        ResourceTables, TableView,
    },
    hash::{FrameMap, FrameSet},
    quad::PixelSource,
    textures::{
        Completed, FrameCommandBuffer, FrameCompletion, MetalContext, NativeTexture,
        PreparedTexture, Proof, Refused, TextureLimits, TextureOwner, TextureStats, TextureStore,
        TextureWakes, UploadItem, UploadPort, new_command_token,
    },
};

const RELEASES_PER_CALL: usize = 256;
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
#[expect(missing_docs, reason = "the state names are the lifecycle contract")]
pub enum RevisionState {
    InFlight,
    Prepared,
    Committed,
    Active,
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

/// An invalid ledger operation or native frame-reader failure.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "variant payloads are transition diagnostics")]
pub enum LedgerError {
    /// The key/revision pair is not live in this ledger.
    UnknownRevision,
    /// The operation requires a different revision state.
    InvalidState {
        state: RevisionState,
        operation: &'static str,
    },
    /// The one-building/one-submitted-ticket bound is occupied.
    TicketBusy,
    /// Shutdown started or a GPU failure was latched.
    Ended,
    /// [`TextureLedger::set_spare_limit`] was called a second time, or after the first [`TextureLedger::begin_frame`].
    SpareLimitFixed,
    /// Native command preparation failed.
    Native(String),
}

/// Bounded keyed notifications produced by [`TextureLedger::pump`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "variant fields are keyed event payloads")]
pub enum LedgerEvent<K> {
    /// An upload is ready for the service's compare-and-commit.
    Prepared { key: K, revision: u64 },
    /// One upload failed; the failure does not poison other revisions.
    UploadFailed {
        key: K,
        revision: u64,
        reason: String,
    },
    /// The upload of the key marked by [`TextureLedger::set_compat_key`] failed (a resource error, such as the texture
    /// budget). It accompanies the ordinary [`UploadFailed`](Self::UploadFailed) of the same failure, which the driver
    /// relays to the service: two observations of one failure, so neither consumer can take the other's. A failure of a key
    /// that is no longer marked produces only the ordinary event.
    CompatUploadFailed { key: K, reason: String },
    /// A store or frame completion failed; emitted at most once.
    GpuFailure { message: String },
    /// All revisions of a retiring key were released.
    KeyRetired { key: K },
}

/// A deliberate defect of the ticket-map reuse, switched on by [`TextureLedger::test_spare_fault`] so the checks that prove the reuse
/// can show they fail without it (their negative controls, CK-148).
///
/// Compiled only with the `spare-faults` feature, which only the textures probe enables (through a dev-dependency of this crate on
/// itself): a normal build has no such defect to switch on, and its ledger always enforces the setup-only limit, the clearing of a
/// returned map and the retention bound.
#[cfg(feature = "spare-faults")]
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpareFault {
    /// A returned map is kept without being cleared.
    SkipClear,
    /// A returned map is kept whatever it held, ignoring the spare limit.
    KeepOversized,
    /// The maps of a ticket dropped without an ending are not returned (only explicit endings return them).
    ExplicitOnly,
    /// [`TextureLedger::set_spare_limit`] always applies, so it can lower the limit after a spare has warmed.
    AlwaysApply,
}

/// The deliberate defects switched on. Without the `spare-faults` feature there is no state to hold them and [`faults`] is a constant
/// with none on.
#[derive(Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent deliberate defects, each switched on by its own negative control"
)]
struct Faults {
    skip_clear: bool,
    keep_oversized: bool,
    explicit_only: bool,
    always_apply: bool,
}

impl Faults {
    const NONE: Self = Self {
        skip_clear: false,
        keep_oversized: false,
        explicit_only: false,
        always_apply: false,
    };
}

#[cfg(feature = "spare-faults")]
fn faults<K>(state: &LedgerState<K>) -> Faults {
    state.faults
}

#[cfg(not(feature = "spare-faults"))]
const fn faults<K>(_state: &LedgerState<K>) -> Faults {
    Faults::NONE
}

/// Bounded aggregate ledger and upload-store diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[expect(missing_docs, reason = "aggregate counters are named by their fields")]
pub struct LedgerStats {
    pub in_flight: usize,
    pub prepared: usize,
    pub committed_waiting_for_boundary: usize,
    pub active: usize,
    pub retired: usize,
    pub baseline_charges: usize,
    pub extra_revisions: usize,
    pub extra_revisions_peak: usize,
    pub refused_key_retiring: u64,
    pub refused_protocol: u64,
    pub adoptions_last_boundary: usize,
    pub adoptions_peak_boundary: usize,
    pub adoptions_total: u64,
    pub first_adopted_ns: u64,
    pub last_adopted_ns: u64,
    pub live_pins: usize,
    pub tickets_building: usize,
    pub tickets_submitting: usize,
    pub tickets_submitted_pending: usize,
    pub tickets_uncertain: usize,
    pub releases: u64,
    /// The revision held for the presenter as the last compatibility image it drew (0: none).
    pub compat_held_revision: u64,
    pub residency_changes_last_pass: usize,
    pub residency_commits_last_pass: usize,
    /// The entries held by the ticket maps the ledger keeps for reuse (always 0: a map is cleared before it is kept).
    pub spare_ticket_entries: usize,
    /// The summed capacity of those maps.
    pub spare_ticket_capacity: usize,
    pub upload: TextureStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RevisionId<K> {
    key: K,
    revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RevisionKey<K>(RevisionId<K>);

struct Revision<K> {
    id: RevisionId<K>,
    sequence: u64,
    state: RevisionState,
    extra: bool,
    pins: usize,
    release_queued: bool,
    native_slot: Option<u32>,
    texture: Option<PreparedTexture<RevisionKey<K>>>,
}

struct BindlessLedger {
    residency: ResidencySet,
    slots: ResourceSlots,
    checker: Option<*mut std::ffi::c_void>,
    checker_owner: Option<NativeTexture>,
    tables: ResourceTables,
    table_bytes: u64,
    checker_resource_id: Option<u64>,
}

impl BindlessLedger {
    fn commit(&mut self) {
        self.residency.commit();
    }
}

struct KeyRevisions<K> {
    revisions: FrameMap<u64, Revision<K>>,
    baseline: Option<u64>,
    active: Option<u64>,
    retiring: bool,
}

struct PendingTicket<K> {
    pins: Vec<RevisionId<K>>,
    table_version: Option<usize>,
    completion: FrameCompletion,
}
struct UncertainTicket<K> {
    _pins: Vec<RevisionId<K>>,
    _table_version: Option<usize>,
    _completion: Option<FrameCompletion>,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent lifecycle facts are observed together for shutdown proof"
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
    extra_peak: usize,
    refused_key_retiring: u64,
    refused_protocol: u64,
    adoptions_last: usize,
    adoptions_peak: usize,
    /// Revisions adopted over the ledger's life, and when the first and the latest boundary that adopted any ran
    /// (`CLOCK_UPTIME_RAW` nanoseconds): the moments a texture became ready on the render thread.
    adoptions_total: u64,
    first_adopted_ns: u64,
    last_adopted_ns: u64,
    live_pins: usize,
    table_pins: [usize; 2],
    building: bool,
    submitting: bool,
    pending: Option<PendingTicket<K>>,
    uncertain: Vec<UncertainTicket<K>>,
    failure_latched: bool,
    failure_event_emitted: bool,
    ending: bool,
    releases: u64,
    /// The one key whose upload failure is also the presenter's (the current frame's compatibility key).
    compat_key: Option<K>,
    /// The one revision held for the presenter as the last compatibility image it drew: a pin that outlives the key's
    /// retirement and the adoption of newer revisions, so that image stays resolvable until a newer one is drawn.
    compat_hold: Option<RevisionId<K>>,
    residency_changes_last_pass: usize,
    residency_commits_last_pass: usize,
    /// The most entries a ticket's map may have held for the ledger to keep it for the next ticket (CK-148). Fixed at setup.
    spare_limit: usize,
    /// Whether [`TextureLedger::set_spare_limit`] has been applied.
    spare_limit_set: bool,
    /// Whether [`TextureLedger::begin_frame`] has been called, whether or not it succeeded.
    frames_begun: bool,
    /// The defects a negative control switched on (tests only; compiled with the `spare-faults` feature).
    #[cfg(feature = "spare-faults")]
    faults: Faults,
    /// The kept, empty `resolved` map of the last ticket, with its capacity.
    spare_resolved: Option<FrameMap<K, Option<u64>>>,
    /// The kept, empty `pinned` set of the last ticket, with its capacity.
    spare_pinned: Option<FrameSet<RevisionId<K>>>,
}

impl<K> Default for LedgerState<K> {
    fn default() -> Self {
        Self {
            keys: FrameMap::default(),
            completed_pending: VecDeque::new(),
            committed: VecDeque::new(),
            release_candidates: VecDeque::new(),
            events: VecDeque::new(),
            sequence: 0,
            extra_charges: 0,
            baseline_charges: 0,
            extra_peak: 0,
            refused_key_retiring: 0,
            refused_protocol: 0,
            adoptions_last: 0,
            adoptions_peak: 0,
            adoptions_total: 0,
            first_adopted_ns: 0,
            last_adopted_ns: 0,
            live_pins: 0,
            table_pins: [0; 2],
            building: false,
            submitting: false,
            pending: None,
            uncertain: Vec::new(),
            failure_latched: false,
            failure_event_emitted: false,
            ending: false,
            releases: 0,
            compat_key: None,
            compat_hold: None,
            residency_changes_last_pass: 0,
            residency_commits_last_pass: 0,
            spare_limit: 0,
            spare_limit_set: false,
            frames_begun: false,
            #[cfg(feature = "spare-faults")]
            faults: Faults::NONE,
            spare_resolved: None,
            spare_pinned: None,
        }
    }
}

/// The unique render-owner ledger for an upload store and its native revisions.
///
/// It is `Send`, not `Sync` or `Clone`; all owner operations are serialized on its render thread.
pub struct TextureLedger<K: Copy + Eq + Hash + Send + 'static> {
    port: UploadPort<RevisionKey<K>>,
    owner: RefCell<Option<TextureOwner<RevisionKey<K>>>>,
    state: RefCell<LedgerState<K>>,
    bindless: RefCell<Option<BindlessLedger>>,
    residency_budget_per_pass: usize,
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
    /// Starts the upload worker and creates the owner-thread revision ledger.
    pub fn start(
        context: Arc<MetalContext>,
        texture_limits: TextureLimits,
        limits: LedgerLimits,
        wakes: TextureWakes,
    ) -> Result<Self, Error> {
        let limits = limits.validate()?;
        let (port, owner) = TextureStore::start(context, texture_limits, wakes.clone())?;
        Ok(Self {
            port,
            owner: RefCell::new(Some(owner)),
            state: RefCell::new(LedgerState::default()),
            bindless: RefCell::new(None),
            residency_budget_per_pass: RESIDENCY_CHANGES_PER_PASS,
            limits,
            wakes,
            id: NEXT_LEDGER_ID.fetch_add(1, Ordering::Relaxed),
            released: Cell::new(false),
            _not_sync: PhantomData,
        })
    }

    /// Test-only fault boundary for delayed uploads and injected Metal completion failures.
    #[doc(hidden)]
    pub fn start_with_test_hooks(
        context: Arc<MetalContext>,
        texture_limits: TextureLimits,
        limits: LedgerLimits,
        wakes: TextureWakes,
        hooks: crate::TextureTestHooks,
    ) -> Result<Self, Error> {
        let limits = limits.validate()?;
        let (port, owner) =
            TextureStore::start_with_test_hooks(context, texture_limits, wakes.clone(), hooks)?;
        Ok(Self {
            port,
            owner: RefCell::new(Some(owner)),
            state: RefCell::new(LedgerState::default()),
            bindless: RefCell::new(None),
            residency_budget_per_pass: RESIDENCY_CHANGES_PER_PASS,
            limits,
            wakes,
            id: NEXT_LEDGER_ID.fetch_add(1, Ordering::Relaxed),
            released: Cell::new(false),
            _not_sync: PhantomData,
        })
    }

    /// Starts a ledger with a checked bindless gate and its queue-owned residency set.
    #[expect(
        clippy::too_many_arguments,
        reason = "the bindless start receives the scene geometry budget and its reserved instance-buffer bytes"
    )]
    pub fn start_bindless(
        context: Arc<MetalContext>,
        texture_limits: TextureLimits,
        limits: LedgerLimits,
        bindless_limits: BindlessLimits,
        binding_geometry_bytes: u64,
        instance_buffer_bytes: u64,
        wakes: TextureWakes,
        hooks: crate::TextureTestHooks,
    ) -> Result<Self, Error> {
        let limits = limits.validate()?;
        let residency_budget_per_pass = hooks
            .residency_change_budget
            .unwrap_or(RESIDENCY_CHANGES_PER_PASS);
        let mut residency = ResidencySet::start(
            &context,
            bindless_limits,
            hooks.force_bindless_tier1,
            hooks.fail_residency_creation,
        )?;
        let tables = ResourceTables::new(
            &context,
            bindless_limits.texture_slots,
            binding_geometry_bytes,
            instance_buffer_bytes,
        )?;
        tables.add_residency(&mut residency);
        residency.commit();
        let table_bytes = tables.allocated_size();
        let (port, owner) =
            TextureStore::start_with_test_hooks(context, texture_limits, wakes.clone(), hooks)?;
        Ok(Self {
            port,
            owner: RefCell::new(Some(owner)),
            state: RefCell::new(LedgerState::default()),
            bindless: RefCell::new(Some(BindlessLedger {
                residency,
                slots: ResourceSlots::new(bindless_limits.texture_slots),
                checker: None,
                checker_owner: None,
                tables,
                table_bytes,
                checker_resource_id: None,
            })),
            residency_budget_per_pass,
            limits,
            wakes,
            id: NEXT_LEDGER_ID.fetch_add(1, Ordering::Relaxed),
            released: Cell::new(false),
            _not_sync: PhantomData,
        })
    }

    /// Adds and commits the presenter-owned checker in reserved native slot zero.
    pub(crate) fn bind_checker(
        &self,
        texture: *mut std::ffi::c_void,
        resource_id: u64,
    ) -> Result<(), Error> {
        if let Some(bindless) = self.bindless.borrow_mut().as_mut() {
            bindless.tables.initialize_checker(resource_id)?;
            bindless.residency.add(texture);
            bindless.commit();
            bindless.checker = Some(texture);
            bindless.checker_resource_id = Some(resource_id);
        }
        Ok(())
    }

    /// Binds a separately owned checker texture in off-screen bindless tests.
    #[doc(hidden)]
    pub fn test_bind_checker(&self, checker: &FrameTexture<'_>) -> Result<(), Error> {
        let native = checker.native.clone_for_frame();
        let texture = native.metal();
        let resource_id = native.resource_id();
        let mut bindless = self.bindless.borrow_mut();
        let Some(bindless) = bindless.as_mut() else {
            return Err(Error(
                "bindless checker requested from an ordinary ledger".to_owned(),
            ));
        };
        if bindless.checker.is_some() {
            return Err(Error("bindless checker is already installed".to_owned()));
        }
        bindless.tables.initialize_checker(resource_id)?;
        bindless.residency.add(texture);
        bindless.commit();
        bindless.checker = Some(texture);
        bindless.checker_owner = Some(native);
        bindless.checker_resource_id = Some(resource_id);
        Ok(())
    }

    /// Returns the two CPU-mirrored table entries when neither version is pinned.
    #[doc(hidden)]
    pub fn test_bindless_entries(&self, slot: u32) -> Option<[u64; 2]> {
        if self.state.borrow().table_pins != [0, 0] {
            return None;
        }
        let bindless = self.bindless.borrow();
        let tables = &bindless.as_ref()?.tables;
        Some([tables.contents(0, slot)?, tables.contents(1, slot)?])
    }

    /// Returns both table versions' views (native buffer, epoch, entry count) so a test can observe each buffer's own length;
    /// `None` for an ordinary ledger. The buffers stay valid while the ledger lives.
    #[doc(hidden)]
    pub fn test_table_views(&self) -> Option<[TableView; 2]> {
        let bindless = self.bindless.borrow();
        let tables = &bindless.as_ref()?.tables;
        Some([tables.view(0), tables.view(1)])
    }

    /// Returns a revision's native slot to the off-screen lifecycle tests.
    #[doc(hidden)]
    pub fn test_native_slot(&self, key: K, revision: u64) -> Option<u32> {
        self.state
            .borrow()
            .keys
            .get(&key)?
            .revisions
            .get(&revision)?
            .native_slot
    }

    /// Native commit calls made to this bindless residency set, for off-screen accounting tests.
    #[doc(hidden)]
    pub fn test_residency_commit_count(&self) -> Option<u64> {
        self.bindless
            .borrow()
            .as_ref()
            .map(|bindless| bindless.residency.commit_count())
    }

    /// The residency set's measured allocation, separate from texture charges.
    pub(crate) fn residency_set_allocation(&self) -> u64 {
        self.bindless
            .borrow()
            .as_ref()
            .map_or(0, |bindless| bindless.residency.allocated_size())
    }

    /// The entry count of each resource-ID table (zero for an ordinary ledger).
    pub(crate) fn table_slots(&self) -> u32 {
        self.bindless.borrow().as_ref().map_or(0, |bindless| {
            bindless.tables.view(bindless.tables.current()).slots
        })
    }

    /// Measured allocation for the two resource-ID tables, separate from texture charges.
    pub(crate) fn table_allocation(&self) -> u64 {
        self.bindless
            .borrow()
            .as_ref()
            .map_or(0, |bindless| bindless.table_bytes)
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

    /// Offers one immutable CPU image revision. This creates an in-flight revision but does not change the active mapping.
    /// Callers must reserve at most `min(room(), extra_room())` offers before calling this method.
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
            state.refused_protocol = state.refused_protocol.saturating_add(1);
            return Err(OfferRefusal::Protocol(
                "ledger is ending or failed".to_owned(),
            ));
        }
        if state.keys.get(&key).is_some_and(|entry| entry.retiring) {
            state.refused_key_retiring = state.refused_key_retiring.saturating_add(1);
            return Err(OfferRefusal::KeyRetiring);
        }
        let existing = state.keys.get(&key);
        if existing.is_some_and(|entry| entry.revisions.contains_key(&revision)) {
            state.refused_protocol = state.refused_protocol.saturating_add(1);
            return Err(OfferRefusal::Protocol(
                "duplicate live key/revision".to_owned(),
            ));
        }
        let is_new_key = existing.is_none_or(|entry| entry.revisions.is_empty());
        if self.port.room() == 0
            || (!is_new_key && state.extra_charges >= self.limits.max_extra_revisions)
        {
            state.refused_protocol = state.refused_protocol.saturating_add(1);
            return Err(OfferRefusal::Protocol(
                "offer exceeded reserved room".to_owned(),
            ));
        }
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        let id = RevisionId { key, revision };
        let item = UploadItem {
            tag: RevisionKey(id),
            width,
            height,
            pixels,
        };
        if let Err(Refused(item)) = self.port.offer(item) {
            drop(item);
            state.refused_protocol = state.refused_protocol.saturating_add(1);
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
            state.extra_peak = state.extra_peak.max(state.extra_charges);
        }
        let Some(entry) = state.keys.get_mut(&key) else {
            state.refused_protocol = state.refused_protocol.saturating_add(1);
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
                native_slot: None,
                texture: None,
            },
        );
        Ok(())
    }

    /// Commits a service-approved prepared revision; it remains invisible until adoption. Committing changes candidate
    /// state and the pending FIFO, not the active mapping or either published table.
    pub fn commit(&self, key: K, revision: u64) -> Result<(), LedgerError> {
        let mut state = self.state.borrow_mut();
        let (old, ready_to_release) = {
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
        state.release_candidates.extend(ready_to_release);
        for old_revision in old {
            state
                .committed
                .retain(|id| id.key != key || id.revision != old_revision);
        }
        state.committed.push_back(RevisionId { key, revision });
        Ok(())
    }

    /// Rejects a prepared revision for bounded release on a subsequent owner step. A prepared revision is not active, so
    /// rejection does not change any active mapping or published table entry.
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

    /// Retires all revisions for a key and refuses future offers for that key. This explicitly clears the active mapping
    /// immediately, even while a frame ticket is Building. A key already resolved by that ticket keeps returning its
    /// cached revision until the ticket ends; a first lookup after retirement returns `None`. `resolve` pins that revision
    /// directly, while `resolve_slot` is protected by the ticket's immutable table-version pin: catch-up never writes a
    /// pinned table, and release waits until the slot is sanitized in both table versions.
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

    /// One owner pass: submits and polls uploads and frame tokens without waiting; emits queued events. Every
    /// `pump` or `adopt` invocation has its own 1,024 membership-change budget and at most one residency commit.
    /// Returns whether this pass left runnable work, so a caller that sleeps on wakes pumps again instead.
    #[expect(
        clippy::too_many_lines,
        reason = "one owner pass keeps upload completions, residency changes, and the single commit ordered"
    )]
    pub fn pump(&self, out: &mut Vec<LedgerEvent<K>>) -> bool {
        let mut completed = Vec::new();
        let mut store_failure = None;
        if let Some(owner) = self.owner.borrow_mut().as_mut() {
            owner.submit();
            owner.poll(&mut completed);
            store_failure = owner.take_failure();
        }
        let mut displaced = Vec::new();
        let mut wake_room = false;
        let mut bindless = self.bindless.borrow_mut();
        let mut membership_dirty = false;
        let mut membership_edits = 0;
        let mut residency_budget = bindless
            .as_ref()
            .map(|_| MembershipBudget::new(self.residency_budget_per_pass));
        let more;
        {
            let mut state = self.state.borrow_mut();
            state.completed_pending.extend(completed);
            while let Some(result) = state.completed_pending.pop_front() {
                match result {
                    Completed::Ready(texture) => {
                        if residency_budget
                            .as_ref()
                            .is_some_and(|budget| budget.remaining() == 0)
                        {
                            state
                                .completed_pending
                                .push_front(Completed::Ready(texture));
                            break;
                        }
                        let RevisionKey(id) = texture.tag;
                        if !state
                            .keys
                            .get(&id.key)
                            .is_some_and(|entry| entry.revisions.contains_key(&id.revision))
                        {
                            displaced.push(texture);
                            continue;
                        }
                        let native_slot = if let Some(bindless) = bindless.as_mut() {
                            if let Some(slot) = bindless.slots.allocate() {
                                bindless.residency.add(texture.metal());
                                bindless.tables.stage(slot, texture.resource_id());
                                membership_dirty = true;
                                membership_edits += 1;
                                if let Some(budget) = residency_budget.as_mut() {
                                    let took = budget.take();
                                    debug_assert!(took);
                                }
                                Some(slot)
                            } else {
                                remove_revision(&mut state, id, &mut displaced, &mut wake_room);
                                let reason = format!(
                                    "bindless texture slots exhausted ({} in use, {} configured)",
                                    bindless.slots.in_use(),
                                    bindless.slots.capacity()
                                );
                                push_upload_failed(&mut state, id, reason);
                                displaced.push(texture);
                                continue;
                            }
                        } else {
                            None
                        };
                        let retiring = state.keys.get(&id.key).is_some_and(|entry| entry.retiring);
                        if let Some(item) = state
                            .keys
                            .get_mut(&id.key)
                            .and_then(|e| e.revisions.get_mut(&id.revision))
                        {
                            item.texture = Some(texture);
                            item.native_slot = native_slot;
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
                        tag: RevisionKey(id),
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
            reap_completed_ticket(&mut state);
            if let Some(bindless) = bindless.as_mut()
                && let Err(error) = bindless.tables.catch_up(state.table_pins)
            {
                latch_failure(
                    &mut state,
                    format!("bindless table catch-up failed: {error}"),
                );
            }
            membership_dirty |= release_retired(
                &mut state,
                &mut displaced,
                &mut wake_room,
                bindless.as_mut(),
                &mut residency_budget,
                &mut membership_edits,
            );
            out.extend(state.events.drain(..));
            more = !state.release_candidates.is_empty()
                || !state.completed_pending.is_empty()
                || bindless
                    .as_ref()
                    .is_some_and(|item| item.tables.has_dirty());
        }
        if membership_dirty && let Some(bindless) = bindless.as_mut() {
            bindless.commit();
        }
        {
            let mut state = self.state.borrow_mut();
            state.residency_changes_last_pass = membership_edits;
            state.residency_commits_last_pass = usize::from(membership_dirty);
        }
        drop(displaced);
        if wake_room {
            self.wake_room();
        }
        more
    }

    /// A **non-destructive** look for a GPU failure (a store batch that completed with an error, a frame whose
    /// completion carried one): it submits nothing, polls no upload, reaps no ticket and releases no texture, so it
    /// is safe while the end is not yet proven. The failure is latched like the ordinary pump's and returned once;
    /// the ordinary pump never reports it again.
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
        let frame_failure = state
            .pending
            .as_ref()
            .filter(|ticket| ticket.completion.is_done())
            .and_then(|ticket| ticket.completion.status().err());
        if let Some(message) = frame_failure {
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

    /// Whether the store's submitted upload batch has completed and is still held, unpolled (tests only: a phase
    /// handshake; it takes and releases nothing).
    #[doc(hidden)]
    #[must_use]
    pub fn completion_arrived_unpolled(&self) -> bool {
        self.owner
            .borrow()
            .as_ref()
            .is_some_and(TextureOwner::completion_arrived_unpolled)
    }

    /// Whether the store holds a prepared, closed envelope not yet submitted (tests only: a phase handshake; it
    /// takes and releases nothing).
    #[doc(hidden)]
    #[must_use]
    pub fn envelope_ready_unsubmitted(&self) -> bool {
        self.owner
            .borrow()
            .as_ref()
            .is_some_and(TextureOwner::envelope_ready_unsubmitted)
    }

    /// Whether committed revisions wait for a frame boundary (the presenter then wants a frame, so adoption does
    /// not wait for an unrelated redraw).
    #[must_use]
    pub fn has_committed(&self) -> bool {
        !self.state.borrow().committed.is_empty()
    }

    /// Whether bounded release work is waiting (a retired revision whose last pin just went): the presenter then runs
    /// another ordinary pass instead of sleeping, so a released hold's revision is freed, and its key's retirement
    /// reported, without waiting for an unrelated wake.
    #[must_use]
    pub fn has_release_work(&self) -> bool {
        !self.state.borrow().release_candidates.is_empty()
    }

    /// Marks `key` as the current frame's compatibility key (one slot: a later call replaces it, `None` clears it), so a
    /// failed upload of it is also reported as [`LedgerEvent::CompatUploadFailed`] by the next [`pump`](Self::pump). This
    /// affects failure reporting only; it does not change active mappings or table contents.
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
    /// nothing. A Building ticket's held-revision lookup is checked against the current hold on every call, not cached:
    /// replacing or releasing the hold can make a repeated lookup return `None`. It cannot return a wrong slot because the
    /// lookup requires the exact currently held key and revision.
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
                .filter(|item| item.texture.is_some())
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
    /// ticket is Building. A revision that was only kept alive by the hold is then released by the ordinary bounded release
    /// work. Repeated held-revision lookups on an open ticket then return `None`, since they are validated against the current
    /// hold; table-version pins still prevent a stale or wrong table slot from being returned.
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

    /// One owner pass that adopts committed revisions in commit order, up to both supplied and configured caps. Adoption
    /// alone is deferred while a ticket is Building; the other public mapping-mutating entry points act immediately as
    /// documented. Adoption changes active mappings and publishes table entries together.
    /// This invocation has its own 1,024 membership-change budget and at most one residency commit. While a frame ticket is
    /// Building, committed revisions remain pending and no active mapping or table publication changes; adoption resumes after
    /// the ticket's resolution closes at submit preparation or an explicit ending.
    #[expect(
        clippy::too_many_lines,
        reason = "publication, revision adoption, table catch-up and retirement share one ordered owner pass"
    )]
    pub fn adopt(&self, limit: usize, out: &mut Vec<(K, u64)>) {
        let mut displaced = Vec::new();
        let mut wake_room = false;
        let mut bindless = self.bindless.borrow_mut();
        let mut membership_dirty = false;
        let mut membership_edits = 0;
        let mut residency_budget = bindless
            .as_ref()
            .map(|_| MembershipBudget::new(self.residency_budget_per_pass));
        {
            let mut state = self.state.borrow_mut();
            let cap = limit.min(self.limits.max_adoptions_per_frame);
            let mut adopted = 0;
            let resolution_open = state.building;
            if let Some(bindless) = bindless.as_mut()
                && let Err(error) = bindless.tables.catch_up(state.table_pins)
            {
                latch_failure(
                    &mut state,
                    format!("bindless table catch-up failed: {error}"),
                );
            }
            let needs_publication = !resolution_open && !state.committed.is_empty();
            let published = bindless.as_mut().is_none_or(|item| {
                item.tables
                    .publish_if_ready(state.table_pins, needs_publication)
            });
            let mut remaining = state.committed.len();
            while published && !resolution_open && adopted < cap && remaining > 0 {
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
                    if let Some(bindless) = bindless.as_ref()
                        && next.native_slot.is_none_or(|slot| {
                            bindless.tables.contents(bindless.tables.current(), slot)
                                != next.texture.as_ref().map(PreparedTexture::resource_id)
                        })
                    {
                        state.committed.push_front(id);
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
                    state.extra_peak = state.extra_peak.max(state.extra_charges);
                    if transfer < 0 {
                        wake_room = true;
                    }
                }
                out.push((id.key, id.revision));
                adopted += 1;
            }
            state.adoptions_last = adopted;
            state.adoptions_peak = state.adoptions_peak.max(adopted);
            if adopted != 0 {
                let now = crate::metrics::now_ns();
                state.adoptions_total = state.adoptions_total.saturating_add(adopted as u64);
                if state.first_adopted_ns == 0 {
                    state.first_adopted_ns = now;
                }
                state.last_adopted_ns = now;
            }
            reap_completed_ticket(&mut state);
            if let Some(bindless) = bindless.as_mut()
                && let Err(error) = bindless.tables.catch_up(state.table_pins)
            {
                latch_failure(
                    &mut state,
                    format!("bindless table catch-up failed: {error}"),
                );
            }
            membership_dirty |= release_retired(
                &mut state,
                &mut displaced,
                &mut wake_room,
                bindless.as_mut(),
                &mut residency_budget,
                &mut membership_edits,
            );
        }
        if membership_dirty && let Some(bindless) = bindless.as_mut() {
            bindless.commit();
        }
        {
            let mut state = self.state.borrow_mut();
            state.residency_changes_last_pass = membership_edits;
            state.residency_commits_last_pass = usize::from(membership_dirty);
        }
        drop(displaced);
        if wake_room {
            self.wake_room();
        }
    }

    /// Opens the single building frame ticket.
    pub fn begin_frame(&self) -> Result<FrameTicket<'_, K>, LedgerError> {
        let table_version = self
            .bindless
            .borrow()
            .as_ref()
            .map(|bindless| bindless.tables.current());
        let mut state = self.state.borrow_mut();
        state.frames_begun = true;
        if state.ending || state.failure_latched {
            return Err(LedgerError::Ended);
        }
        reap_completed_ticket(&mut state);
        if state.ending || state.failure_latched {
            return Err(LedgerError::Ended);
        }
        if state.building || state.submitting {
            return Err(LedgerError::TicketBusy);
        }
        state.building = true;
        if let Some(version) = table_version {
            state.table_pins[version] += 1;
        }
        let spare_resolved = state.spare_resolved.take().unwrap_or_default();
        let spare_pinned = state.spare_pinned.take().unwrap_or_default();
        Ok(FrameTicket {
            ledger: self,
            pins: RefCell::new(Vec::new()),
            pinned: RefCell::new(spare_pinned),
            resolved: RefCell::new(spare_resolved),
            completion: RefCell::new(None),
            table_version,
            phase: Cell::new(TicketPhase::Building),
        })
    }

    /// Switches on one deliberate defect of the ticket-map reuse (the negative controls of its checks; built only with the
    /// `spare-faults` feature).
    #[cfg(feature = "spare-faults")]
    #[doc(hidden)]
    pub fn test_spare_fault(&self, fault: SpareFault) {
        let mut state = self.state.borrow_mut();
        match fault {
            SpareFault::SkipClear => state.faults.skip_clear = true,
            SpareFault::KeepOversized => state.faults.keep_oversized = true,
            SpareFault::ExplicitOnly => state.faults.explicit_only = true,
            SpareFault::AlwaysApply => state.faults.always_apply = true,
        }
    }

    /// Sets how many entries a ticket's map may have held for the ledger to keep it (empty, with its capacity) for the next ticket;
    /// 0, the default, keeps none. The scene sets it to the most distinct keys one frame resolves.
    ///
    /// It can be set **once, and only before the first [`begin_frame`](Self::begin_frame)** (whether or not that call succeeded), so
    /// the limit never changes while a ticket or a kept map can exist and the capacity bound it gives holds for the ledger's life.
    pub fn set_spare_limit(&self, entries: usize) -> Result<(), LedgerError> {
        let mut state = self.state.borrow_mut();
        if !faults(&state).always_apply
            && (state.spare_limit_set || state.frames_begun || state.building || state.submitting)
        {
            return Err(LedgerError::SpareLimitFixed);
        }
        state.spare_limit = entries;
        state.spare_limit_set = true;
        Ok(())
    }

    /// Proves worker, uploads and frame outcomes against one absolute deadline.
    pub fn prove(&self, deadline: Instant) -> LedgerProof {
        self.state.borrow_mut().ending = true;
        {
            let state = self.state.borrow();
            let reason = if state.building || state.submitting {
                Some("frame ticket is building or submitting")
            } else if !state.uncertain.is_empty() {
                Some("frame outcome is uncertain")
            } else {
                None
            };
            if let Some(reason) = reason {
                return LedgerProof {
                    ledger_id: self.id,
                    store: None,
                    complete: false,
                    reason: Some(reason.to_owned()),
                };
            }
        }
        loop {
            let pending = self
                .state
                .borrow()
                .pending
                .as_ref()
                .is_some_and(|ticket| !ticket.completion.is_done());
            if !pending {
                let mut state = self.state.borrow_mut();
                reap_completed_ticket(&mut state);
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                return LedgerProof {
                    ledger_id: self.id,
                    store: None,
                    complete: false,
                    reason: Some("frame token remained outstanding".to_owned()),
                };
            }
            std::thread::sleep(
                deadline
                    .saturating_duration_since(now)
                    .min(Duration::from_millis(1)),
            );
        }
        let store = self
            .owner
            .borrow_mut()
            .as_mut()
            .map(|owner| owner.prove(deadline));
        let Some(store) = store else {
            return LedgerProof {
                ledger_id: self.id,
                store: None,
                complete: false,
                reason: Some("upload owner already released".to_owned()),
            };
        };
        let complete = store.is_complete();
        let reason = if store.is_complete() {
            None
        } else {
            Some("upload worker or token remained outstanding".to_owned())
        };
        LedgerProof {
            ledger_id: self.id,
            store: Some(store),
            complete,
            reason,
        }
    }

    /// Releases revisions first and the upload store second, only with this ledger's complete proof.
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
        {
            let mut state = self.state.borrow_mut();
            let mut bindless = self.bindless.borrow_mut();
            for (_, mut entry) in state.keys.drain() {
                for (_, mut revision) in entry.revisions.drain() {
                    if let Some(texture) = revision.texture.take() {
                        if let Some(bindless) = bindless.as_mut() {
                            bindless.residency.remove(texture.metal());
                        }
                        displaced.push(texture);
                    }
                }
            }
            if let Some(bindless) = bindless.as_mut() {
                if let Some(checker) = bindless.checker.take() {
                    bindless.residency.remove(checker);
                }
                bindless.tables.remove_residency(&mut bindless.residency);
                bindless.commit();
            }
            // Completed but not yet admitted textures have no residency membership; drop them with the proven ledger.
            state.completed_pending.clear();
            state.committed.clear();
            state.compat_hold = None;
            state.pending.take();
            state.baseline_charges = 0;
            state.extra_charges = 0;
            state.live_pins = 0;
        }
        // Detach the set only after its removals are committed, and before releasing its textures.
        drop(self.bindless.borrow_mut().take());
        drop(displaced);
        if let Some(owner) = self.owner.borrow_mut().as_mut() {
            owner.release(store);
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

    /// The upload store's counters alone (cumulative uploads and texture creations included): constant work, unlike
    /// [`stats`](Self::stats), which also counts every revision.
    #[must_use]
    pub fn upload_stats(&self) -> TextureStats {
        self.owner
            .borrow()
            .as_ref()
            .map(TextureOwner::stats)
            .unwrap_or_default()
    }

    /// The revisions adopted over the ledger's life and the times (`CLOCK_UPTIME_RAW` ns) of the first and the latest boundary that
    /// adopted any: a light read, for the presenter's timing record.
    #[must_use]
    pub fn adoption_counts(&self) -> (u64, u64, u64) {
        let state = self.state.borrow();
        (
            state.adoptions_total,
            state.first_adopted_ns,
            state.last_adopted_ns,
        )
    }

    /// Returns bounded ledger and upload-store counters.
    #[must_use]
    pub fn stats(&self) -> LedgerStats {
        let ledger_state = self.state.borrow();
        let mut stats = LedgerStats {
            baseline_charges: ledger_state.baseline_charges,
            extra_revisions: ledger_state.extra_charges,
            extra_revisions_peak: ledger_state.extra_peak,
            refused_key_retiring: ledger_state.refused_key_retiring,
            refused_protocol: ledger_state.refused_protocol,
            adoptions_last_boundary: ledger_state.adoptions_last,
            adoptions_peak_boundary: ledger_state.adoptions_peak,
            adoptions_total: ledger_state.adoptions_total,
            first_adopted_ns: ledger_state.first_adopted_ns,
            last_adopted_ns: ledger_state.last_adopted_ns,
            live_pins: ledger_state.live_pins,
            tickets_building: usize::from(ledger_state.building),
            tickets_submitting: usize::from(ledger_state.submitting),
            tickets_submitted_pending: usize::from(ledger_state.pending.is_some()),
            tickets_uncertain: ledger_state.uncertain.len(),
            releases: ledger_state.releases,
            compat_held_revision: ledger_state.compat_hold.map_or(0, |id| id.revision),
            ..LedgerStats::default()
        };
        stats.spare_ticket_entries = ledger_state
            .spare_resolved
            .as_ref()
            .map_or(0, FrameMap::len)
            + ledger_state.spare_pinned.as_ref().map_or(0, FrameSet::len);
        stats.spare_ticket_capacity = ledger_state
            .spare_resolved
            .as_ref()
            .map_or(0, FrameMap::capacity)
            + ledger_state
                .spare_pinned
                .as_ref()
                .map_or(0, FrameSet::capacity);
        stats.residency_changes_last_pass = ledger_state.residency_changes_last_pass;
        stats.residency_commits_last_pass = ledger_state.residency_commits_last_pass;
        for entry in ledger_state.keys.values() {
            for rev in entry.revisions.values() {
                match rev.state {
                    RevisionState::InFlight => stats.in_flight += 1,
                    RevisionState::Prepared => stats.prepared += 1,
                    RevisionState::Committed => stats.committed_waiting_for_boundary += 1,
                    RevisionState::Active => stats.active += 1,
                    RevisionState::Retired => stats.retired += 1,
                }
            }
        }
        if let Some(owner) = self.owner.borrow().as_ref() {
            stats.upload = owner.stats();
        }
        stats
    }

    fn wake_room(&self) {
        if let Some(wake) = &self.wakes.room {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| wake()));
        }
    }
}

impl<K: Copy + Eq + Hash + Send + 'static> Drop for TextureLedger<K> {
    fn drop(&mut self) {
        if self.released.get() {
            return;
        }
        let state = std::mem::take(self.state.get_mut());
        std::mem::forget(state);
        if let Some(owner) = self.owner.get_mut().take() {
            std::mem::forget(owner);
        }
        std::mem::forget(self.bindless.get_mut().take());
    }
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

fn reap_completed_ticket<K: Copy + Eq + Hash>(state: &mut LedgerState<K>) {
    if !state
        .pending
        .as_ref()
        .is_some_and(|p| p.completion.is_done())
    {
        return;
    }
    let Some(pending) = state.pending.take() else {
        return;
    };
    release_table_pin(state, pending.table_version);
    let result = pending.completion.status();
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
    if let Err(message) = result {
        latch_failure(state, message);
    }
}

fn release_table_pin<K>(state: &mut LedgerState<K>, version: Option<usize>) {
    if let Some(version) = version {
        state.table_pins[version] = state.table_pins[version].saturating_sub(1);
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "retirement readiness, bounded table sanitization and release stay in one ordered pass"
)]
fn release_retired<K: Copy + Eq + Hash>(
    state: &mut LedgerState<K>,
    displaced: &mut Vec<PreparedTexture<RevisionKey<K>>>,
    wake_room: &mut bool,
    bindless: Option<&mut BindlessLedger>,
    residency_budget: &mut Option<MembershipBudget>,
    membership_edits: &mut usize,
) -> bool {
    let mut bindless = bindless;
    let mut membership_dirty = false;
    for _ in 0..RELEASES_PER_CALL {
        let Some(id) = state.release_candidates.pop_front() else {
            break;
        };
        let Some((candidate_state, candidate_pins, candidate_slot)) = state
            .keys
            .get(&id.key)
            .and_then(|entry| entry.revisions.get(&id.revision))
            .map(|candidate| (candidate.state, candidate.pins, candidate.native_slot))
        else {
            continue;
        };
        if candidate_state != RevisionState::Retired {
            continue;
        }
        if candidate_pins != 0 {
            // A cached ticket resolution can pin a Retired revision after retirement first queued it.
            // Let the final unpin enqueue it again instead of leaving a stale queued flag behind.
            if let Some(candidate) = state
                .keys
                .get_mut(&id.key)
                .and_then(|entry| entry.revisions.get_mut(&id.revision))
            {
                candidate.release_queued = false;
            }
            continue;
        }
        let safe_to_release = bindless.as_deref().is_none_or(|item| {
            candidate_slot.is_none_or(|slot| {
                item.checker_resource_id.is_none_or(|checker_id| {
                    item.tables.contents(0, slot) == Some(checker_id)
                        && item.tables.contents(1, slot) == Some(checker_id)
                })
            })
        });
        if !safe_to_release {
            if let Some(bindless) = bindless.as_deref_mut()
                && let (Some(slot), Some(checker_id)) =
                    (candidate_slot, bindless.checker_resource_id)
            {
                bindless.tables.sanitize(slot, checker_id);
            }
            state.release_candidates.push_back(id);
            continue;
        }
        if residency_budget
            .as_ref()
            .is_some_and(|budget| budget.remaining() == 0)
        {
            state.release_candidates.push_front(id);
            break;
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
                if let Some(bindless) = bindless.as_deref_mut() {
                    bindless.residency.remove(texture.metal());
                    *membership_edits += 1;
                    if let Some(budget) = residency_budget.as_mut() {
                        let took = budget.take();
                        debug_assert!(took);
                    }
                    if let Some(slot) = removed.native_slot.take() {
                        bindless.tables.clear_staged(slot);
                        bindless.slots.release(slot);
                    }
                    membership_dirty = true;
                }
                displaced.push(texture);
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
        state.releases = state.releases.saturating_add(1);
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
    membership_dirty
}

/// Proof tying a ledger and upload-owner proof together.
pub struct LedgerProof {
    ledger_id: u64,
    store: Option<Proof>,
    complete: bool,
    reason: Option<String>,
}

impl LedgerProof {
    /// Whether every worker, upload and frame token was proven complete.
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
#[expect(
    missing_docs,
    reason = "the outcomes summarize the two-phase cleanup result"
)]
pub enum LedgerCleanup {
    Proven,
    Unproven { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TicketPhase {
    Building,
    Submitting,
    Ended,
}

/// One frame's exact revision pins. Dropping before an explicit end conservatively retains pins.
pub struct FrameTicket<'a, K: Copy + Eq + Hash + Send + 'static> {
    ledger: &'a TextureLedger<K>,
    pins: RefCell<Vec<RevisionId<K>>>,
    /// The revisions in `pins`, for the "already pinned" test: a scan of `pins` per distinct key would make resolving a
    /// frame's textures quadratic in their number (CK-116 found it at 4,096 distinct textures).
    pinned: RefCell<FrameSet<RevisionId<K>>>,
    resolved: RefCell<FrameMap<K, Option<u64>>>,
    completion: RefCell<Option<FrameCompletion>>,
    table_version: Option<usize>,
    phase: Cell<TicketPhase>,
}

#[expect(
    clippy::missing_errors_doc,
    reason = "transition failures are named by LedgerError"
)]
impl<K: Copy + Eq + Hash + Send + 'static> FrameTicket<'_, K> {
    /// The direct resource-ID table and epoch pinned by this frame, when bindless is enabled.
    #[must_use]
    pub fn bindless(&self) -> Option<TableView> {
        let version = self.table_version?;
        self.ledger
            .bindless
            .borrow()
            .as_ref()
            .map(|bindless| bindless.tables.view(version))
    }

    /// Resolves the exact active revision, pinning it at most once per key. The first successful lookup is cached for this
    /// ticket: an explicit later [`TextureLedger::retire_key`] does not change a cached answer, and repeated calls may still
    /// resolve that pinned Retired revision until the ticket ends. The direct revision pin keeps its texture alive. A first
    /// lookup after retirement returns `None`.
    pub fn resolve(&self, key: K) -> Option<FrameTexture<'_>> {
        if self.phase.get() != TicketPhase::Building {
            return None;
        }
        let mut state = self.ledger.state.borrow_mut();
        // One lookup of the key's record serves both the active revision (read once per key and cached) and the revision's own record.
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
        let (native, width, height, native_slot, new_pin) = {
            let revision = entry?.revisions.get_mut(&revision_id)?;
            if revision.state != RevisionState::Active
                && !(cached && revision.state == RevisionState::Retired)
            {
                return None;
            }
            let texture = revision.texture.as_ref()?;
            let native = texture.native_for_frame()?;
            let (width, height) = (texture.width, texture.height);
            let native_slot = revision.native_slot.unwrap_or(0);
            let new_pin = self.pinned.borrow_mut().insert(id);
            if new_pin {
                revision.pins += 1;
                self.pins.borrow_mut().push(id);
            }
            (native, width, height, native_slot, new_pin)
        };
        if new_pin {
            state.live_pins += 1;
        }
        Some(FrameTexture {
            native,
            native_slot,
            width,
            height,
            revision: revision_id,
            _ticket: PhantomData,
        })
    }

    /// Resolves the exact active revision's native slot in this frame's pinned table and the revision's number, **without
    /// pinning the revision** (bindless frames only; `None` otherwise, for an unresolved key and once the ticket left its
    /// building phase).
    ///
    /// The frame's epoch pin alone keeps every image it can sample alive and resident for the frame's whole life: a frame
    /// samples a texture only through a slot of its pinned table version with the checker bound; that version names the
    /// texture's resource ID, and the ledger releases a retired revision only after its slot is sanitised in **both**
    /// versions. A pinned version is
    /// never written (`catch_up` skips a version with pins), so its entry cannot be sanitised and the release cannot happen
    /// until the frame's token is done or its ticket discarded. Unlike [`resolve`](Self::resolve), this method does not pin
    /// the revision itself; the immutable table-version pin is the lifetime guarantee. The active revision is read once per
    /// key and cached, so every instance of a key in one frame shows the same revision. An explicit later
    /// [`TextureLedger::retire_key`] does not change a cached answer; a first lookup after retirement returns `None`.
    pub fn resolve_slot(&self, key: K) -> Option<(u32, u64)> {
        if self.phase.get() != TicketPhase::Building || self.table_version.is_none() {
            return None;
        }
        let state = self.ledger.state.borrow();
        // One lookup of the key's record serves both the active revision (read once per key and cached) and the revision's own record.
        let entry = state.keys.get(&key);
        let (revision_id, cached) = match self.resolved.borrow_mut().entry(key) {
            Entry::Occupied(previous) => (*previous.get(), true),
            Entry::Vacant(slot) => (*slot.insert(entry.and_then(|e| e.active)), false),
        };
        let revision_id = revision_id?;
        let revision = entry?.revisions.get(&revision_id)?;
        if (revision.state != RevisionState::Active
            && !(cached && revision.state == RevisionState::Retired))
            || revision.texture.is_none()
        {
            return None;
        }
        revision.native_slot.map(|slot| (slot, revision_id))
    }

    /// The native slot of one exact **currently held** compatibility revision (see [`TextureLedger::hold_compat`]), whatever
    /// its state, without pinning it (bindless frames only). The hold keeps the revision, and the epoch pin its table entry.
    /// Each call checks the current hold, so replacing or releasing the hold during a Building ticket makes this lookup
    /// return `None`; it never exposes a slot for a revision that is no longer held.
    pub fn resolve_revision_slot(&self, key: K, revision: u64) -> Option<u32> {
        if self.phase.get() != TicketPhase::Building || self.table_version.is_none() {
            return None;
        }
        let id = RevisionId { key, revision };
        let state = self.ledger.state.borrow();
        if state.compat_hold != Some(id) {
            return None;
        }
        let held = state.keys.get(&key)?.revisions.get(&revision)?;
        held.texture.as_ref()?;
        held.native_slot
    }

    /// Resolves one exact **currently held** revision (see [`TextureLedger::hold_compat`]) whatever its state: it may be
    /// retired or superseded, but the hold keeps it alive. Pins it for this ticket like [`resolve`](Self::resolve). Each call
    /// checks the current hold rather than caching it; replacing or releasing that hold during a Building ticket makes a
    /// repeated lookup return `None`.
    pub fn resolve_revision(&self, key: K, revision: u64) -> Option<FrameTexture<'_>> {
        if self.phase.get() != TicketPhase::Building {
            return None;
        }
        let id = RevisionId { key, revision };
        let mut state = self.ledger.state.borrow_mut();
        if state.compat_hold != Some(id) {
            return None;
        }
        let (native, width, height, native_slot, new_pin) = {
            let held = state.keys.get_mut(&key)?.revisions.get_mut(&revision)?;
            let texture = held.texture.as_ref()?;
            let native = texture.native_for_frame()?;
            let (width, height) = (texture.width, texture.height);
            let native_slot = held.native_slot.unwrap_or(0);
            let new_pin = self.pinned.borrow_mut().insert(id);
            if new_pin {
                held.pins += 1;
                self.pins.borrow_mut().push(id);
            }
            (native, width, height, native_slot, new_pin)
        };
        if new_pin {
            state.live_pins += 1;
        }
        Some(FrameTexture {
            native,
            native_slot,
            width,
            height,
            revision,
            _ticket: PhantomData,
        })
    }

    /// Attaches a completion token before the caller commits the command buffer.
    pub fn prepare_submit(&self, command: &mut FrameCommandBuffer) -> Result<(), LedgerError> {
        if self.phase.get() != TicketPhase::Building {
            return Err(LedgerError::InvalidState {
                state: RevisionState::Active,
                operation: "prepare_submit",
            });
        }
        {
            let mut state = self.ledger.state.borrow_mut();
            reap_completed_ticket(&mut state);
            if state.failure_latched {
                return Err(LedgerError::Ended);
            }
            if state.pending.is_some() {
                return Err(LedgerError::TicketBusy);
            }
        }
        let completion = command
            .prepare()
            .map_err(|e| LedgerError::Native(e.to_string()))?;
        let mut state = self.ledger.state.borrow_mut();
        state.building = false;
        state.submitting = true;
        *self.completion.borrow_mut() = Some(completion);
        self.phase.set(TicketPhase::Submitting);
        Ok(())
    }

    /// The presenter's form of [`prepare_submit`](Self::prepare_submit): the command buffer is the presenter's own
    /// and is committed natively, so this only prepares the completion token and returns the native token the
    /// presenter's completion handler will complete before it commits. Call
    /// [`mark_committed`](Self::mark_committed) after the commit returned.
    pub(crate) fn prepare_native(&self) -> Result<*mut std::ffi::c_void, LedgerError> {
        if self.phase.get() != TicketPhase::Building {
            return Err(LedgerError::InvalidState {
                state: RevisionState::Active,
                operation: "prepare_native",
            });
        }
        {
            let mut state = self.ledger.state.borrow_mut();
            reap_completed_ticket(&mut state);
            if state.failure_latched {
                return Err(LedgerError::Ended);
            }
            if state.pending.is_some() {
                return Err(LedgerError::TicketBusy);
            }
        }
        let (completion, token) =
            new_command_token().map_err(|error| LedgerError::Native(error.to_string()))?;
        let mut state = self.ledger.state.borrow_mut();
        state.building = false;
        state.submitting = true;
        *self.completion.borrow_mut() = Some(completion);
        self.phase.set(TicketPhase::Submitting);
        Ok(token)
    }

    /// Records that the presenter's commit of the prepared command buffer returned.
    pub(crate) fn mark_committed(&self) {
        if let Some(completion) = self.completion.borrow().as_ref() {
            completion.mark_committed();
        }
    }

    /// Finishes a committed frame; its pins remain until `pump` observes the token.
    pub fn submitted(self) -> Result<(), LedgerError> {
        if self.phase.get() != TicketPhase::Submitting {
            return Err(LedgerError::InvalidState {
                state: RevisionState::Active,
                operation: "submitted",
            });
        }
        let completion = self
            .completion
            .borrow_mut()
            .take()
            .ok_or(LedgerError::TicketBusy)?;
        if !completion.is_committed() {
            *self.completion.borrow_mut() = Some(completion);
            return Err(LedgerError::Native(
                "submitted before command commit".to_owned(),
            ));
        }
        let pins = std::mem::take(&mut *self.pins.borrow_mut());
        let table_version = self.table_version;
        let mut state = self.ledger.state.borrow_mut();
        state.submitting = false;
        state.pending = Some(PendingTicket {
            pins,
            table_version,
            completion,
        });
        self.phase.set(TicketPhase::Ended);
        Ok(())
    }

    /// Explicitly ends a building ticket that never submitted GPU work.
    pub fn discarded(self) -> Result<(), LedgerError> {
        if self.phase.get() != TicketPhase::Building {
            return Err(LedgerError::InvalidState {
                state: RevisionState::Active,
                operation: "discarded",
            });
        }
        self.end_pins();
        self.phase.set(TicketPhase::Ended);
        Ok(())
    }

    /// Ends a prepared ticket only if its command buffer was not committed.
    pub fn not_committed(self) -> Result<(), LedgerError> {
        if self.phase.get() != TicketPhase::Submitting {
            return Err(LedgerError::InvalidState {
                state: RevisionState::Active,
                operation: "not_committed",
            });
        }
        if self
            .completion
            .borrow()
            .as_ref()
            .is_some_and(FrameCompletion::is_committed)
        {
            return Err(LedgerError::Native(
                "not_committed after command commit".to_owned(),
            ));
        }
        self.end_pins();
        self.phase.set(TicketPhase::Ended);
        Ok(())
    }

    fn end_pins(&self) {
        let pins = std::mem::take(&mut *self.pins.borrow_mut());
        let mut state = self.ledger.state.borrow_mut();
        release_pins(&mut state, pins);
        release_table_pin(&mut state, self.table_version);
        state.building = false;
        state.submitting = false;
    }
}

impl<K: Copy + Eq + Hash + Send + 'static> FrameTicket<'_, K> {
    /// Gives the ticket's `resolved` map and `pinned` set back to the ledger for the next ticket: each cleared, and kept only if
    /// the entries it held (counted before clearing) are within the ledger's `spare_limit`, so a kept map's capacity never exceeds
    /// what the standard map allocates for that many entries. A larger map is dropped and the next ticket starts empty.
    ///
    /// Only the allocation is reused: a cleared map holds nothing of this ticket. Every ending reaches it through `Drop`.
    fn return_maps(&mut self) {
        let mut resolved = std::mem::take(self.resolved.get_mut());
        let mut pinned = std::mem::take(self.pinned.get_mut());
        // A ledger state that is borrowed (a drop during a panic inside the ledger) simply forgoes the reuse.
        let Ok(mut state) = self.ledger.state.try_borrow_mut() else {
            return;
        };
        let limit = state.spare_limit;
        let defects = faults(&state);
        let clear = !defects.skip_clear;
        let any_size = defects.keep_oversized;
        if any_size || resolved.len() <= limit {
            if clear {
                resolved.clear();
            }
            state.spare_resolved = Some(resolved);
        }
        if any_size || pinned.len() <= limit {
            if clear {
                pinned.clear();
            }
            state.spare_pinned = Some(pinned);
        }
    }
}

impl<K: Copy + Eq + Hash + Send + 'static> Drop for FrameTicket<'_, K> {
    fn drop(&mut self) {
        let explicit_only = self.phase.get() != TicketPhase::Ended
            && self
                .ledger
                .state
                .try_borrow()
                .is_ok_and(|state| faults(&state).explicit_only);
        if !explicit_only {
            self.return_maps();
        }
        if self.phase.get() == TicketPhase::Ended {
            return;
        }
        let pins = std::mem::take(&mut *self.pins.borrow_mut());
        let completion = self.completion.borrow_mut().take();
        let table_version = self.table_version;
        let mut state = self.ledger.state.borrow_mut();
        state.building = false;
        state.submitting = false;
        state.uncertain.push(UncertainTicket {
            _pins: pins,
            _table_version: table_version,
            _completion: completion,
        });
        latch_failure(
            &mut state,
            "frame ticket dropped without an explicit outcome".to_owned(),
        );
        self.phase.set(TicketPhase::Ended);
    }
}

/// One failed upload, two observations, classified here before any consumer sees either: the ordinary
/// [`LedgerEvent::UploadFailed`] (the service's status), and [`LedgerEvent::CompatUploadFailed`] first when `id.key` is the marked
/// compatibility key. Every way an offered revision can fail (the store's per-item errors, the bindless slots running out) goes
/// through here.
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

/// Frame-scoped native reference to one exact active revision.
///
/// A resolved texture cannot outlive its ticket:
///
/// ```compile_fail
/// use ck_gpu_metal::{FrameTexture, TextureLedger};
/// fn escape<'a>(ledger: &'a TextureLedger<u32>) -> FrameTexture<'a> {
///     let ticket = ledger.begin_frame().unwrap();
///     ticket.resolve(1).unwrap()
/// }
/// ```
pub struct FrameTexture<'a> {
    native: NativeTexture,
    native_slot: u32,
    width: u32,
    height: u32,
    revision: u64,
    _ticket: PhantomData<&'a ()>,
}

impl FrameTexture<'_> {
    /// The revision this texture is (for the hold of the last compatibility image drawn).
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// The Metal texture the frame binds (an `id<MTLTexture>`, not retained); valid while the ticket pins the revision.
    pub(crate) fn metal(&self) -> *mut std::ffi::c_void {
        self.native.metal()
    }

    /// Creates a test-only blit reader that verifies this revision's pixels and completion pin.
    /// Its optional delay defers publication on a global queue and never blocks Metal's completion thread.
    #[doc(hidden)]
    ///
    /// # Errors
    ///
    /// Returns a native resource creation failure.
    pub fn test_reader(
        &self,
        delay: Duration,
        error_completion: bool,
    ) -> Result<FrameCommandBuffer, Error> {
        FrameCommandBuffer::new(
            &self.native,
            self.width,
            self.height,
            delay,
            error_completion,
        )
    }

    /// The native resource-ID table index for this revision (slot zero is reserved for the checker).
    #[must_use]
    pub fn native_slot(&self) -> u32 {
        self.native_slot
    }
}
