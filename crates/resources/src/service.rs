//! The resource service: a state machine with no threads of its own (`docs/scene-design.md` §4.3, §4.4, §5.1–5.3,
//! §6.1–6.2, §7.1–7.3, §8.3). The [`LoaderPool`](crate::LoaderPool) supplies the threads that run it; the service is
//! equally driven by hand (the stepped methods below), which is how most of its tests run it.
//!
//! # What it does
//!
//! Callers admit requests cheaply and transactionally ([`ResourceService::create_from_pixels`],
//! [`ResourceService::replace_pixels`]); a stepped executor turns each into an immutable [`PixelRevision`]
//! ([`ResourceService::take_job`], [`Job::run`], [`ResourceService::complete`]); a backend pulls finished revisions,
//! commits the current one by compare-and-commit and acknowledges that frames use it ([`ResourceService::take_uploads`],
//! [`ResourceService::try_commit`], [`ResourceService::adopted`]); and a retiring resource is cleaned up by
//! [`ResourceService::maintain`] and the backend's [`ResourceService::retired`], with no call from the code that dropped
//! its last reference.
//!
//! # Three revisions
//!
//! Each resource has a **requested** revision (the latest admitted), a **committed** one (the latest the backend
//! committed; frames may already use it) and a **shown** one (the latest the backend acknowledged; in a headless run,
//! the latest finished). `shown <= committed <= requested` whenever each exists. Readiness and the shown size refer
//! only to the shown revision: a commit never manufactures an adoption, and a failure never removes or advances one.
//!
//! # Locks
//!
//! One short service lock; the registry's lock is taken only inside it or alone, never the other way round. The
//! decision to accept a result, a commit or an adoption is made under the *registry* lock
//! ([`TextureRegistry::with_entry_lifecycle`]), nested inside the service lock, so a last-reference drop (which needs the
//! registry lock to make the entry retiring) falls either before the decision or after it: the decision is the
//! linearisation point. No pixel buffer, [`CpuPixels`], [`Job`], [`PixelRevision`] or resource state is dropped while
//! either lock is held, and no callback runs under either: values to drop are collected and dropped after the locks are
//! released, and the wakes are called after them. **Nothing waits** except the one method a loader thread sleeps in
//! ([`ResourceService::wait_for_work`]); every caller-side and backend-side method is non-blocking.
//!
//! # File sources and the CPU budget
//!
//! [`ResourceService::load`] admits a PNG file by name with no file-system call; the file is opened and decoded by a
//! job, with the decoded size reserved from the CPU budget before the decoder allocates it. A reservation refused
//! while finished pixels are still in transit (waiting for the backend, held by it) is not a failure: the job waits in
//! the **deferred queue** for those bytes to return, and is retried one at a time, in the order the jobs were taken,
//! when the capacity epoch has changed (`Capacity` in the ledger describes the handshake that loses no return). A
//! refusal with nothing in transit is a capacity error.

use std::collections::VecDeque;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

use crate::assets::{Assets, AssetsError, OpenReserveError};
use crate::budget::{DecodeLimits, MAX_DECODED_BYTES, MAX_SIDE};
use crate::ledger::{
    BudgetKind, BudgetStats, Ledger, LedgerId, LedgerIdsExhausted, Reservation, Shortfall,
};
use crate::limits::{LimitsError, MAX_ERROR_BYTES, MAX_PATH_BYTES, ResourceLimits};
use crate::pixels::{CpuPixels, PixelReservation, PixelRevision};
use crate::queue::IndexedQueue;
use crate::registry::{
    CreateError, HandleError, Lifecycle, NewError, RegistryStats, TextureKey, TextureRef,
    TextureRegistry,
};
use crate::unwind;

/// A wake: called after the locks are released, when a queue goes from empty to non-empty. A wake is not a delivery
/// (it may be spurious, and none is lost because the queues hold the work). It must be short and must not block.
pub type ServiceWake = Arc<dyn Fn() + Send + Sync>;

/// The optional wakes of a service.
#[derive(Clone, Default)]
pub struct ServiceWakes {
    /// A queued request is ready to be taken by [`ResourceService::take_job`].
    pub jobs: Option<ServiceWake>,
    /// A finished revision is ready for [`ResourceService::take_uploads`].
    pub uploads: Option<ServiceWake>,
    /// Retirement work is pending: the registry reported a retiring resource (so [`ResourceService::maintain`] has
    /// work), or the backend has something to retire ([`ResourceService::take_retirements`]).
    pub retirements: Option<ServiceWake>,
}

impl fmt::Debug for ServiceWakes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceWakes")
            .field("jobs", &self.jobs.is_some())
            .field("uploads", &self.uploads.is_some())
            .field("retirements", &self.retirements.is_some())
            .finish()
    }
}

/// Whether there is a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// No backend: a finished job makes its revision shown at once with the state `cpu_ready`, and the pixels are
    /// dropped.
    Headless,
    /// A backend pulls finished revisions and acknowledges adoption.
    Backend,
}

/// Rounds up to a multiple of 64.
const fn round_up_64(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(64) * 64
}

/// The metadata charged for one resource: the size of its record, of its key and of the queue entries that can name
/// it (a job, an upload and a retirement queue entry, and one finishing entry), rounded up to 64 bytes.
pub(crate) const RESOURCE_METADATA_BYTES: u64 =
    round_up_64(size_of::<ResourceState>() + 4 * size_of::<TextureKey>());

/// The metadata charged for one job (a queued or executing request): the size of its request, its output and its
/// reservations, rounded up to 64 bytes.
pub(crate) const JOB_METADATA_BYTES: u64 =
    round_up_64(size_of::<Request>() + size_of::<JobOutput>() + size_of::<Ticket>());

/// How many queued resources one [`ResourceService::take_job`] looks at. A queued resource whose retirement the
/// registry has begun (its last reference dropped, on a thread that never calls the service) can only be passed over,
/// and [`ResourceService::maintain`] removes it; the budget keeps one step's work independent of how many there are.
const JOB_VISITS: usize = 8;

/// How many queued resources one [`ResourceService::take_uploads`] may pass over, beyond the `limit` it was asked to
/// move: the same bound, for the same resources.
const UPLOAD_SKIPS: usize = 8;

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// Why a service could not be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceError {
    /// A limit is outside its range.
    Limits(LimitsError),
    /// The registry could not be made.
    Registry(NewError),
    /// Every ledger identity has been issued.
    Ledger(LedgerIdsExhausted),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limits(error) => error.fmt(formatter),
            Self::Registry(error) => error.fmt(formatter),
            Self::Ledger(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ServiceError {}

/// Why a request was refused. Each message is bounded and names the reason; a refusal changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmitError {
    /// The image is larger than the decoder's per-image limits.
    TooLarge {
        /// The requested width.
        width: u32,
        /// The requested height.
        height: u32,
    },
    /// A budget (CPU pixel bytes or metadata bytes) cannot cover the request.
    Capacity {
        /// Which budget.
        budget: BudgetKind,
        /// How much was asked.
        requested: u64,
        /// How much was available.
        available: u64,
    },
    /// No registry slot is free.
    RegistryFull,
    /// No job slot is free.
    JobsFull,
    /// The key cannot be used (stale, foreign, wrong kind, closed).
    Handle(HandleError),
    /// The resource's revision counter is at `u64::MAX`; it never wraps.
    RevisionsExhausted,
    /// The service has ended.
    Ended,
    /// The pixels' reservation was granted by another service.
    ForeignReservation,
    /// No assets root is configured, so no file can be loaded.
    NoAssets,
    /// The name is longer than [`MAX_PATH_BYTES`] bytes.
    PathTooLong {
        /// Its length in bytes.
        bytes: usize,
    },
    /// The name breaks the rules for names ([`Assets::check_name`]): the message is that error's, unchanged.
    Name(String),
    /// A reload of a texture that was not loaded from a file (CPU sources use `replace_pixels`).
    NotAFileSource,
    /// A replacement of the pixels of a texture that was loaded from a file (those use `reload`).
    NotACpuSource,
}

/// Which shared resource limit refused a synchronously decoded legacy image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyBudget {
    /// Decoded CPU pixels.
    CpuPixelBytes,
    /// Live texture registry slots.
    RegistrySlots,
    /// Resource metadata.
    MetadataBytes,
}

impl fmt::Display for LegacyBudget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CpuPixelBytes => "CPU pixel bytes",
            Self::RegistrySlots => "registry slots",
            Self::MetadataBytes => "metadata bytes",
        })
    }
}

/// The result of synchronously decoding and admitting a legacy image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decoded {
    /// The resource's public key. The caller may retain an internal reference, then release this ownership.
    pub key: TextureKey,
    /// The decoded image width.
    pub width: u32,
    /// The decoded image height.
    pub height: u32,
}

/// Why a synchronous legacy image could not be admitted.
#[derive(Debug)]
pub enum LegacyAdmitError {
    /// Opening or decoding failed with the usual assets error and message.
    Assets(AssetsError),
    /// A shared CPU, registry-slot or metadata budget could not cover the admission.
    Budget {
        /// The shared budget that refused.
        budget: LegacyBudget,
        /// The requested amount.
        requested: u64,
        /// The amount available.
        available: u64,
    },
    /// The service ended before the resource could be admitted.
    Ended,
    /// This synchronous admission is not available in the named mode.
    Mode(Mode),
    /// The service has no configured assets root.
    NoAssets,
}

impl fmt::Display for LegacyAdmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Assets(error) => error.fmt(formatter),
            Self::Budget {
                budget,
                requested,
                available,
            } => write!(
                formatter,
                "not enough shared {budget}: {requested} needed, {available} available"
            ),
            Self::Ended => formatter.write_str("the resource service has ended"),
            Self::Mode(mode) => write!(
                formatter,
                "synchronous decoded-image admission is unavailable in {mode:?} mode"
            ),
            Self::NoAssets => formatter.write_str("no assets directory is configured"),
        }
    }
}

impl std::error::Error for LegacyAdmitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Assets(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for AdmitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { width, height } => write!(
                formatter,
                "a {width}x{height} image exceeds the per-image limits ({MAX_SIDE} pixels a side, {MAX_DECODED_BYTES} bytes)"
            ),
            Self::Capacity {
                budget,
                requested,
                available,
            } => write!(
                formatter,
                "not enough {budget}: {requested} needed, {available} available"
            ),
            Self::RegistryFull => formatter.write_str("every texture slot is in use"),
            Self::JobsFull => formatter.write_str("every job slot is in use"),
            Self::Handle(error) => error.fmt(formatter),
            Self::RevisionsExhausted => {
                formatter.write_str("the texture has used every revision number")
            }
            Self::Ended => formatter.write_str("the resource service has ended"),
            Self::ForeignReservation => {
                formatter.write_str("the pixels' reservation belongs to another resource service")
            }
            Self::NoAssets => formatter.write_str("no assets directory is configured"),
            Self::PathTooLong { bytes } => write!(
                formatter,
                "the image name is {bytes} bytes long; at most {MAX_PATH_BYTES} are allowed"
            ),
            Self::Name(message) => formatter.write_str(message),
            Self::NotAFileSource => formatter
                .write_str("the texture was not loaded from a file; replace its pixels instead"),
            Self::NotACpuSource => {
                formatter.write_str("the texture was loaded from a file; reload it instead")
            }
        }
    }
}

impl std::error::Error for AdmitError {}

/// A refused admission: the reason, and the caller's pixels (with their own reservation) handed back.
pub struct Refused<P> {
    /// Why.
    pub error: AdmitError,
    /// What the caller passed in, unchanged.
    pub pixels: P,
}

impl<P> fmt::Debug for Refused<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Refused").field(&self.error).finish()
    }
}

impl<P> fmt::Display for Refused<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl<P> std::error::Error for Refused<P> {}

fn refused(error: AdmitError, pixels: CpuPixels) -> Refused<CpuPixels> {
    Refused { error, pixels }
}

/// A refusal that hands back the pixels still waiting in `source`: they are taken out of it only when a call
/// succeeds, so on every refusal they are still there.
fn give_back(error: AdmitError, source: &mut Option<CpuPixels>) -> Refused<CpuPixels> {
    match source.take() {
        Some(pixels) => Refused { error, pixels },
        None => unreachable!("the pixels leave `source` only when the admission succeeds"),
    }
}

// ---------------------------------------------------------------------------------------------
// Per-resource state
// ---------------------------------------------------------------------------------------------

/// A job slot and its metadata charge: they pay for a request from its admission until its job completes.
struct Ticket {
    _slot: Reservation,
    /// Marks the point between the two returns (tests only: the order of the fields is the order of the drops).
    #[cfg(test)]
    _between: SlotReturned,
    _metadata: Reservation,
}

/// Reports that the ticket's slot has been returned and its metadata charge has not (tests only).
#[cfg(test)]
struct SlotReturned(Weak<Shared>);

#[cfg(test)]
impl Drop for SlotReturned {
    fn drop(&mut self) {
        if let Some(shared) = self.0.upgrade() {
            shared.hook_call(Stage::SlotReturned, None);
        }
    }
}

/// Where a request's pixels come from.
enum Source {
    /// A Rust buffer, already reserved and allocated by the caller.
    Pixels(CpuPixels),
    /// The resource's file (its name is in [`ResourceState`]): opened and decoded by the job.
    File,
}

/// A request waiting for a job: the next revision's source.
struct Request {
    revision: u64,
    source: Source,
    ticket: Ticket,
    /// The place in admission order the request holds: file and CPU requests wait in separate queues, and the older
    /// head goes first. A request replaced in place keeps its predecessor's.
    stamp: u64,
}

/// The file a resource is loaded from: its name, charged to metadata while the resource lives.
struct FileSource {
    name: Arc<str>,
    _charge: Reservation,
}

/// A revision with its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sized {
    revision: u64,
    width: u32,
    height: u32,
}

/// What kind of failure an error records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorKind {
    Job,
    Upload,
}

impl ErrorKind {
    /// The text stored when the charge for the real one is refused: no allocation.
    fn fixed(self) -> &'static str {
        match self {
            Self::Job => "the job failed (the error text was not stored: no metadata budget)",
            Self::Upload => "the upload failed (the error text was not stored: no metadata budget)",
        }
    }
}

enum Text {
    Stored {
        text: Arc<str>,
        _charge: Reservation,
    },
    Fixed(&'static str),
}

/// The last error of a resource, with the revision that failed.
struct StoredError {
    revision: u64,
    text: Text,
}

/// The per-resource state a [`TextureRegistry`] entry holds. Opaque: read it through [`TextureStatus`].
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts about one resource: headless, and three of its retirement"
)]
pub struct ResourceState {
    headless: bool,
    requested: u64,
    queued: Option<Request>,
    executing: Option<u64>,
    waiting: Option<Arc<PixelRevision>>,
    taken: Option<Sized>,
    committed: Option<Sized>,
    shown: Option<Sized>,
    error: Option<StoredError>,
    backend_retired: bool,
    discard: bool,
    /// Set while the key is in the service's finishing queue (retirement started, not finished).
    in_finishing: bool,
    /// The file the resource is loaded from, if it is a file resource.
    file: Option<FileSource>,
    _charge: Reservation,
}

#[cfg(test)]
impl Drop for ResourceState {
    fn drop(&mut self) {
        crate::lockdebug::dropped_value();
    }
}

impl ResourceState {
    fn new(charge: Reservation, headless: bool) -> Self {
        Self {
            headless,
            requested: 1,
            queued: None,
            executing: None,
            waiting: None,
            taken: None,
            committed: None,
            shown: None,
            error: None,
            backend_retired: false,
            discard: false,
            in_finishing: false,
            file: None,
            _charge: charge,
        }
    }

    fn status(&self) -> TextureStatus {
        let error = self.error.as_ref().map(|error| ResourceError {
            revision: error.revision,
            text: match &error.text {
                Text::Stored { text, .. } => ErrorText::Stored(Arc::clone(text)),
                Text::Fixed(text) => ErrorText::Fixed(text),
            },
        });
        let state = if let Some(shown) = self.shown {
            let _ = shown;
            if self.headless {
                StatusState::CpuReady
            } else {
                StatusState::Ready
            }
        } else if self.committed.is_some() {
            StatusState::Uploading
        } else if self
            .error
            .as_ref()
            .is_some_and(|e| e.revision == self.requested)
        {
            StatusState::Failed
        } else if self.executing.is_some() {
            StatusState::Decoding
        } else if self.queued.is_some() {
            StatusState::Pending
        } else if self.waiting.is_some() || self.taken.is_some() {
            StatusState::Uploading
        } else {
            // Nothing is queued, executing or with the backend: the request has no work in flight.
            StatusState::Pending
        };
        TextureStatus {
            state,
            requested: self.requested,
            committed: self.committed.map(|c| c.revision),
            shown: self.shown.map(|s| ShownRevision {
                revision: s.revision,
                width: s.width,
                height: s.height,
            }),
            error,
        }
    }

    /// Records a failure of the current revision: truncated at a UTF-8 boundary, charged to metadata while stored; if
    /// the charge is refused, a fixed text is stored instead. The older error moves to `discards`.
    fn record_error(
        &mut self,
        ledger: &Arc<Ledger>,
        revision: u64,
        kind: ErrorKind,
        message: String,
        discards: &mut Discards,
    ) {
        let text = truncate(&message, MAX_ERROR_BYTES);
        let stored = match ledger.reserve(BudgetKind::MetadataBytes, text.len() as u64) {
            Ok(charge) => Text::Stored {
                text: Arc::from(text),
                _charge: charge,
            },
            Err(_) => Text::Fixed(kind.fixed()),
        };
        discards.strings.push(message);
        if let Some(old) = self.error.replace(StoredError {
            revision,
            text: stored,
        }) {
            discards.errors.push(old);
        }
    }

    /// Makes `revision` the shown one; an error older than it is cleared (moved to `discards`).
    fn show(&mut self, revision: Sized, discards: &mut Discards) {
        self.shown = Some(revision);
        if self
            .error
            .as_ref()
            .is_some_and(|error| error.revision < revision.revision)
            && let Some(old) = self.error.take()
        {
            discards.errors.push(old);
        }
    }
}

/// `text` cut at a UTF-8 boundary to at most `max` bytes.
fn truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Everything a call wants dropped once the locks are released.
#[derive(Default)]
struct Discards {
    requests: Vec<Request>,
    pixels: Vec<CpuPixels>,
    revisions: Vec<Arc<PixelRevision>>,
    states: Vec<ResourceState>,
    errors: Vec<StoredError>,
    strings: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// Public value types
// ---------------------------------------------------------------------------------------------

/// A resource's state, in the precedence order of the design: a shown revision makes it `Ready` (`CpuReady` headless)
/// even while a replacement is in flight, committed or failed; otherwise a committed revision awaiting adoption is
/// `Uploading`; otherwise `Failed` if the latest request failed, else `Pending` (queued), `Decoding` (executing) or
/// `Uploading` (waiting for upload, or taken).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusState {
    /// Queued, not yet taken by a job.
    Pending,
    /// A job is executing.
    Decoding,
    /// Waiting for upload, taken by the backend, or committed and awaiting adoption.
    Uploading,
    /// Headless: a revision finished and is shown.
    CpuReady,
    /// A revision was adopted by the backend.
    Ready,
    /// The latest request failed and nothing is shown.
    Failed,
}

/// The shown revision and its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShownRevision {
    /// The revision.
    pub revision: u64,
    /// Its width in pixels.
    pub width: u32,
    /// Its height in pixels.
    pub height: u32,
}

/// An error text: stored (charged to metadata while stored) or a fixed message used when the charge was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorText {
    /// The stored text.
    Stored(Arc<str>),
    /// The fixed message for the kind of failure.
    Fixed(&'static str),
}

/// The last error, with the revision that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceError {
    /// The revision whose job or upload failed.
    pub revision: u64,
    /// What went wrong (at most [`MAX_ERROR_BYTES`] bytes).
    pub text: ErrorText,
}

impl ResourceError {
    /// The text.
    #[must_use]
    pub fn message(&self) -> &str {
        match &self.text {
            ErrorText::Stored(text) => text,
            ErrorText::Fixed(text) => text,
        }
    }
}

/// A snapshot of a resource (never a wait).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureStatus {
    /// The state.
    pub state: StatusState,
    /// The latest admitted revision.
    pub requested: u64,
    /// The latest revision the backend committed, if any.
    pub committed: Option<u64>,
    /// The latest adopted revision with its size, if any.
    pub shown: Option<ShownRevision>,
    /// The last error, if any.
    pub error: Option<ResourceError>,
}

/// The service's numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceStats {
    /// Job slots.
    pub jobs: BudgetStats,
    /// CPU pixel bytes.
    pub cpu_pixels: BudgetStats,
    /// Metadata bytes.
    pub metadata: BudgetStats,
    /// The registry's counters.
    pub registry: RegistryStats,
    /// What the file sources and the loader threads have done.
    pub loaders: LoaderStats,
}

/// What the file sources and the loader threads have done.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoaderStats {
    /// Jobs whose work panicked: each was caught and its request failed.
    pub job_panics: u64,
    /// Loader threads that ended by a failure of their loop outside any job.
    pub loop_failures: u64,
    /// File reservation attempts: the first of every file job and every retry.
    pub reservation_attempts: u64,
    /// Times a file job joined the deferred queue, a return after a refused retry included.
    pub deferrals: u64,
    /// File jobs in the deferred queue now.
    pub deferred_now: usize,
    /// File jobs whose decode finished over the service's life (read-only diagnostics: no behaviour depends on it).
    pub files_decoded: u64,
    /// The decoded pixel bytes of those files.
    pub decoded_bytes: u64,
    /// The most requests that waited in the service's request queues at once.
    pub queue_depth_peak: u64,
    /// Retirements finished inline, by a late completion or a late backend proof after the pool's shutdown.
    pub inline_finishes: u64,
    /// Inline retirements the registry refused (cannot happen by construction).
    pub inline_refusals: u64,
    /// Resources still referenced when `finish_teardown` ran.
    pub referenced_at_teardown: u64,
}

/// What `maintain` did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Maintained {
    /// Retiring resources it started to clean up.
    pub started: usize,
    /// Resources whose retirement it finished (their slots are free at the next generation).
    pub finished: usize,
}

/// A finished revision waiting for the backend.
#[derive(Debug)]
pub struct UploadRequest {
    /// The texture.
    pub key: TextureKey,
    /// The revision.
    pub revision: u64,
    /// The pixels; dropping this `Arc` when the backend no longer needs the CPU bytes returns their reservation.
    pub pixels: Arc<PixelRevision>,
}

/// The answer of a compare-and-commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitOutcome {
    /// The revision is now the committed one; future frames may use it.
    Committed,
    /// Superseded, retiring, closed or detached: the backend retires its prepared native revision.
    Rejected,
}

/// Why a job failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFailure {
    /// What went wrong.
    pub message: String,
}

/// What a job runs.
enum JobWork {
    /// The CPU work of a Rust buffer (the checks or the premultiplication).
    Pixels(CpuPixels),
    /// Opening and decoding the resource's file.
    File { name: Arc<str> },
}

/// A request handed to an executor: it names its key and revision and owns its source and its job slot. **It holds the
/// [`TextureKey`], never a [`TextureRef`]**: a job holding a reference would keep the entry live, so releasing the last
/// public token and destroying the last quad would no longer begin retirement and cancel the job.
pub struct Job {
    key: TextureKey,
    revision: u64,
    work: JobWork,
    ticket: Ticket,
    /// The take sequence of a file job (zero for a CPU job): the order file jobs were taken in.
    seq: u64,
    /// Whether this is the retry of a deferred file job.
    retry: bool,
    shared: Arc<Shared>,
}

impl Job {
    /// The texture.
    #[must_use]
    pub fn key(&self) -> TextureKey {
        self.key
    }

    /// The revision.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The job's work, with no lock held: the checks or the premultiplication of a Rust buffer, or, for a file, opening
    /// and decoding it under a CPU reservation made before the decoder allocates the image (a reservation refused
    /// while finished pixels are in transit makes the output a *deferral*, which [`ResourceService::complete`] turns
    /// into a wait for those bytes). A file job takes the service lock only for one brief moment, between the
    /// decoder's header checks and its allocation, and only when it is the retry of a deferred job.
    ///
    /// A panic in the work is caught here: the request fails with a bounded message naming it, its reservations
    /// return as the work unwinds, the payload is disposed of without unwinding again, and the caller gets an output
    /// like any other.
    #[must_use]
    pub fn run(self) -> JobOutput {
        let Self {
            key,
            revision,
            work,
            ticket,
            seq,
            retry,
            shared,
        } = self;
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            #[cfg(test)]
            shared.at(Stage::Run, key);
            match work {
                JobWork::Pixels(pixels) => match pixels.into_revision(key, revision) {
                    Ok(revision) => JobResult::Done(revision),
                    Err(message) => JobResult::Failed(JobFailure { message }),
                },
                JobWork::File { name } => load_file(&shared, &name, key, revision, retry),
            }
        }));
        let result = match outcome {
            Ok(result) => result,
            Err(payload) => {
                let text = unwind::message(&payload);
                unwind::dispose(payload);
                shared.counters.job_panics.fetch_add(1, Ordering::SeqCst);
                JobResult::Failed(JobFailure {
                    message: format!("the loader panicked: {text}"),
                })
            }
        };
        #[cfg(test)]
        shared.at(Stage::Ran, key);
        JobOutput {
            key,
            revision,
            ticket,
            seq,
            retry,
            result,
        }
    }
}

impl fmt::Debug for Job {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Job")
            .field("key", &self.key)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

/// A refused CPU reservation of a file job.
struct Refusal {
    requested: u64,
    available: u64,
    /// The capacity epoch read before the attempt.
    epoch: u64,
    /// Whether finished pixels were in transit, so the bytes may return without any loader's help.
    deferrable: bool,
}

/// Adds `by` to a diagnostics counter, stopping at the maximum instead of wrapping (a counter at the maximum is a lower bound).
fn bump(counter: &AtomicU64, by: u64) {
    let _ = counter.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
        Some(value.saturating_add(by))
    });
}

/// A file job's work: the reservation is made between the decoder's header checks and its allocation (`§7.2`).
fn load_file(
    shared: &Arc<Shared>,
    name: &str,
    key: TextureKey,
    revision: u64,
    retry: bool,
) -> JobResult {
    let Some(assets) = shared.assets.as_ref() else {
        return JobResult::Failed(JobFailure {
            message: AdmitError::NoAssets.to_string(),
        });
    };
    let ledger = &shared.ledger;
    // A test may hold this job before it opens its path (feature `test-support`; no such check exists without it).
    #[cfg(feature = "test-support")]
    {
        let gate = shared
            .job_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(gate) = gate {
            gate.pass(name);
        }
    }
    let opened = assets.open_reserving(name, &DecodeLimits::SHIPPED, |bytes| {
        #[cfg(test)]
        shared.at(Stage::BeforeAttempt, key);
        shared.counters.attempts.fetch_add(1, Ordering::SeqCst);
        // (1) The epoch is read before the attempt.
        let epoch = ledger.capacity.epoch();
        match ledger.reserve(BudgetKind::CpuPixelBytes, bytes) {
            Ok(reservation) => {
                if retry {
                    shared.retry_reserved(key, revision);
                }
                #[cfg(test)]
                shared.at(Stage::Reserved, key);
                Ok(reservation)
            }
            Err(short) => {
                #[cfg(test)]
                shared.at(Stage::Refused, key);
                // Bytes in transit return without any loader's help. So do bytes that returned since the attempt
                // (the epoch moved): the refusal may already be stale, and a retry will settle it.
                let deferrable =
                    ledger.capacity.in_transit() > 0 || ledger.capacity.epoch() != epoch;
                #[cfg(test)]
                shared.at(Stage::Checked, key);
                if deferrable {
                    // (2) A return from here on must notify.
                    ledger.capacity.raise_deferral();
                    #[cfg(test)]
                    shared.at(Stage::FlagRaised, key);
                }
                Err(Refusal {
                    requested: short.requested,
                    available: short.available,
                    epoch,
                    deferrable,
                })
            }
        }
    });
    match opened {
        Ok((image, reservation)) => {
            // Read-only diagnostics: one more file decoded, and its decoded bytes.
            bump(&shared.counters.files_decoded, 1);
            bump(&shared.counters.decoded_bytes, image.pixels().len() as u64);
            JobResult::Done(PixelRevision::from_decoded(
                key,
                revision,
                image,
                reservation,
            ))
        }
        Err(OpenReserveError::Assets(error)) => JobResult::Failed(JobFailure {
            message: error.to_string(),
        }),
        Err(OpenReserveError::Refused(refusal)) if refusal.deferrable => JobResult::Deferred {
            epoch: refusal.epoch,
        },
        Err(OpenReserveError::Refused(refusal)) => JobResult::Failed(JobFailure {
            message: AdmitError::Capacity {
                budget: BudgetKind::CpuPixelBytes,
                requested: refusal.requested,
                available: refusal.available,
            }
            .to_string(),
        }),
    }
}

/// How a job ended.
enum JobResult {
    /// It produced the revision.
    Done(Arc<PixelRevision>),
    /// It failed.
    Failed(JobFailure),
    /// A file job's reservation was refused while finished pixels were in transit: it waits for them to return.
    Deferred {
        /// The capacity epoch read before the refused attempt.
        epoch: u64,
    },
}

/// What a job produced, to hand to [`ResourceService::complete`].
pub struct JobOutput {
    key: TextureKey,
    revision: u64,
    ticket: Ticket,
    seq: u64,
    retry: bool,
    result: JobResult,
}

impl JobOutput {
    /// The texture.
    #[must_use]
    pub fn key(&self) -> TextureKey {
        self.key
    }

    /// The revision.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether the job succeeded.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        matches!(self.result, JobResult::Done(_))
    }

    /// Whether the job waits for CPU bytes to return rather than having ended.
    #[must_use]
    pub fn is_deferred(&self) -> bool {
        matches!(self.result, JobResult::Deferred { .. })
    }
}

impl fmt::Debug for JobOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JobOutput")
            .field("key", &self.key)
            .field("revision", &self.revision)
            .field("ok", &self.is_ok())
            .field("deferred", &self.is_deferred())
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------------------------

/// The service lock's guard. In tests it also records, per thread, that the lock is held (`lockdebug`).
struct Guard<'a, T> {
    guard: MutexGuard<'a, T>,
    #[cfg(test)]
    _held: crate::lockdebug::Held,
}

impl<T> Deref for Guard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T> Guard<'_, T> {
    /// Waits on `condvar`, releasing the lock while it waits (in tests the thread does not count as holding it).
    fn wait(self, condvar: &Condvar) -> Self {
        let Guard {
            guard,
            #[cfg(test)]
                _held: held,
        } = self;
        #[cfg(test)]
        drop(held);
        let guard = condvar.wait(guard).unwrap_or_else(PoisonError::into_inner);
        Guard {
            guard,
            #[cfg(test)]
            _held: crate::lockdebug::Held::new(),
        }
    }
}

/// A file job whose reservation was refused while finished pixels were in transit. Its request, with the job slot and
/// the job's metadata, waits in the resource's `queued` field; this is its place in the deferred queue.
struct Deferred {
    key: TextureKey,
    revision: u64,
    /// The take sequence the job received when a loader took it: the order of the deferred queue.
    seq: u64,
    /// The capacity epoch read before the refused attempt.
    epoch: u64,
}

/// The one deferred job being retried: its outcome is not decided yet.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Retry {
    key: TextureKey,
    revision: u64,
}

/// What the service lock protects: the queues, the flags.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent flags of one state under one lock"
)]
struct Core {
    closed: bool,
    detached: bool,
    /// Resources with a queued CPU request and no executing job, oldest first. Each is in it once, and a resource
    /// whose request is dropped leaves it at once, so it holds only live work and `take_job` never scans stale
    /// entries.
    jobs: IndexedQueue,
    /// The same for file requests. They are taken only while the deferred queue is empty and no retry is in flight.
    file_jobs: IndexedQueue,
    /// File jobs waiting for CPU bytes to return, in take order (lowest sequence first). At most one per loader.
    deferred: Vec<Deferred>,
    /// The deferred job being retried, if any: file admission stays closed until its outcome is decided.
    retrying: Option<Retry>,
    /// The front of the deferred queue is eligible whatever the epoch (a retry was just decided, with bytes possibly
    /// left): set when a retry's reservation succeeds or fails for good, cleared when the front is taken.
    front_now: bool,
    /// The next take sequence of a file job.
    next_seq: u64,
    /// The next admission stamp (see [`Request::stamp`]).
    next_stamp: u64,
    /// The most requests that waited in the two request queues at once (read-only diagnostics, updated under the lock the push
    /// already holds).
    queue_peak: usize,
    /// The loader threads are asked to end.
    loaders_stop: bool,
    /// Resources with a revision waiting for upload, oldest first. A superseded or dropped revision leaves the queue
    /// when it is dropped; a replacement takes its place at the back when its own job completes.
    uploads: IndexedQueue,
    /// Retiring resources the backend must retire.
    backend_retire: VecDeque<TextureKey>,
    /// Retiring resources whose clean-up has started and whose retirement is not finished (the *pending-finish set*:
    /// drained from the registry's FIFO, but blocked by a job still executing or a backend proof awaited). Each is in
    /// it once; a resource leaves it in O(1) when its retirement is finished inline.
    finishing: IndexedQueue,
    /// How many finishing resources `maintain` has found blocked since the last event that could unblock one (a
    /// backend proof, a discarded completion, a detach, a new retirement). When it reaches the number of finishing
    /// resources a whole scan has found only blocked work, and `maintain` stops asking to be called again.
    blocked_streak: usize,
    /// Queued resources examined by `take_job` and `take_uploads` so far (tests only).
    #[cfg(test)]
    visited: usize,
}

impl Core {
    /// Queues a CPU request and notes the depth of the request queues.
    fn push_job(&mut self, key: TextureKey) {
        self.jobs.push_back(key);
        self.queue_peak = self.queue_peak.max(self.jobs.len() + self.file_jobs.len());
    }

    /// Queues a file request and notes the depth of the request queues.
    fn push_file_job(&mut self, key: TextureKey) {
        self.file_jobs.push_back(key);
        self.queue_peak = self.queue_peak.max(self.jobs.len() + self.file_jobs.len());
    }

    /// Whether the deferred queue's front may be retried now: nothing is being retried and the front's epoch is not the
    /// current one (or a retry was just decided).
    fn front_eligible(&self, epoch: u64) -> bool {
        self.retrying.is_none()
            && self
                .deferred
                .first()
                .is_some_and(|front| self.front_now || front.epoch != epoch)
    }

    /// Whether a new file job may be taken from the ordinary queue.
    fn file_admission_open(&self) -> bool {
        self.deferred.is_empty() && self.retrying.is_none()
    }

    /// The next admission stamp.
    fn stamp(&mut self) -> u64 {
        let stamp = self.next_stamp;
        self.next_stamp += 1;
        stamp
    }

    /// Called wherever the deferred queue may have emptied: with no deferred job and no retry in flight the deferral flag
    /// is cleared (returners stop taking the service lock) and "eligible at once" lapses. Only under the service lock; a
    /// loader that raised the flag meanwhile raises it again as it records its deferral.
    fn settle(&mut self, ledger: &Ledger) {
        if self.deferred.is_empty() {
            self.front_now = false;
            if self.retrying.is_none() {
                ledger.capacity.clear_deferral();
            }
        }
    }

    /// Takes `key` out of the deferred queue; whether it was there.
    fn undefer(&mut self, key: TextureKey) -> bool {
        let before = self.deferred.len();
        self.deferred.retain(|entry| entry.key != key);
        self.deferred.len() != before
    }

    /// Whether `key` is in a queue of ready requests.
    fn is_queued(&self, key: TextureKey) -> bool {
        self.jobs.contains(key.slot()) || self.file_jobs.contains(key.slot())
    }
}

/// Retirement work flagged by the registry's wake, and the wake hooks.
struct Notify {
    retirement_pending: AtomicBool,
    wakes: ServiceWakes,
    /// The shared state the loader threads sleep in (set once, right after it exists).
    shared: OnceLock<Weak<Shared>>,
}

impl Notify {
    fn retirement_work(&self) {
        self.retirement_pending.store(true, Ordering::SeqCst);
        if let Some(wake) = &self.wakes.retirements {
            wake();
        }
        self.wake_loaders();
    }

    /// Wakes the loader threads (a no-op once the service is gone).
    fn wake_loaders(&self) {
        if let Some(shared) = self.shared.get().and_then(Weak::upgrade) {
            shared.wake_loaders();
        }
    }
}

/// The numbers the loaders and the service keep outside the lock.
#[derive(Default)]
struct Counters {
    /// Jobs whose work panicked (caught, the request failed).
    job_panics: AtomicU64,
    /// Loader threads that ended by a failure of their loop outside any job.
    loop_failures: AtomicU64,
    /// File reservation attempts (the first and every retry).
    attempts: AtomicU64,
    /// File jobs that joined the deferred queue (the first deferral and every return to it).
    deferrals: AtomicU64,
    /// Inline retirements the registry refused (cannot happen by construction).
    finish_refused: AtomicU64,
    /// Retirements finished inline, by a late completion or a late backend proof.
    inline_finished: AtomicU64,
    /// Entries still referenced when `finish_teardown` ran.
    referenced_at_teardown: AtomicU64,
    /// File jobs whose decode finished (read-only diagnostics).
    files_decoded: AtomicU64,
    /// The decoded pixel bytes of those files.
    decoded_bytes: AtomicU64,
}

/// What the tests install to watch or hold the file job and loader protocols (tests only).
#[cfg(test)]
pub(crate) type Hook = Arc<dyn Fn(Stage, Option<TextureKey>) + Send + Sync>;

/// A point in the file job and loader protocols a test can hold or fail (tests only).
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// The start of a job's work, inside its panic boundary.
    Run,
    /// The end of a job's work, after its panic boundary (the output is made).
    Ran,
    /// A file job, before it reads the capacity epoch and makes its reservation attempt.
    BeforeAttempt,
    /// A file job's reservation was granted (and a retry's mark cleared); the decoder has not allocated yet.
    Reserved,
    /// A file job's attempt was refused; nothing has been looked at yet.
    Refused,
    /// The refusal was judged (deferrable or not); the deferral flag is not yet set.
    Checked,
    /// The flag is set; the output has not reached `complete` yet.
    FlagRaised,
    /// `complete` has recorded a deferral (the locks are released).
    Recorded,
    /// A loader thread is at the top of its loop.
    LoopTop,
    /// `complete` finished a retirement inline; its discards and the job's slot are not yet dropped.
    Finished,
    /// `complete` has published the job's result and left its locks; the discards and the job's ticket are not yet
    /// dropped (the request may already read as ready while its slot and bytes are still charged).
    Published,
    /// A job's ticket has returned its slot; its metadata charge is not yet returned.
    SlotReturned,
    /// `complete` is done: the discards and the ticket are dropped and the owed wakes fired.
    Completed,
}

struct Shared {
    mode: Mode,
    limits: ResourceLimits,
    registry: TextureRegistry<ResourceState>,
    ledger: Arc<Ledger>,
    notify: Arc<Notify>,
    core: Mutex<Core>,
    /// Where the loader threads sleep: paired with `core`, whose state is the predicate they wait for.
    wake: Condvar,
    /// The configured assets root, if any.
    assets: Option<Arc<Assets>>,
    counters: Counters,
    /// Called inside an acceptance's critical section (tests only).
    #[cfg(test)]
    gate: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// How many loaders are inside `wait_for_work`'s wait (tests only).
    #[cfg(test)]
    waiting: std::sync::atomic::AtomicUsize,
    /// Called at the points of the file job and loader protocols (tests only).
    #[cfg(test)]
    hook: Mutex<Option<Hook>>,
    /// The hold a test of a crate built on the service puts on named file jobs (feature `test-support` only).
    #[cfg(feature = "test-support")]
    job_gate: Mutex<Option<Arc<crate::jobgate::JobGate>>>,
}

impl Shared {
    fn lock(&self) -> Guard<'_, Core> {
        let guard = self.core.lock().unwrap_or_else(PoisonError::into_inner);
        Guard {
            guard,
            #[cfg(test)]
            _held: crate::lockdebug::Held::new(),
        }
    }

    /// Wakes every sleeping loader after the state they wait on changed. The lock is taken and released first, so a
    /// loader that has checked its predicate but not yet begun to wait (it holds the lock until it does) cannot miss
    /// the notification. Never called with a lock of this crate held.
    fn wake_loaders(&self) {
        #[cfg(test)]
        crate::lockdebug::must_be_free();
        drop(self.lock());
        self.wake.notify_all();
    }

    /// A retry's reservation succeeded: its outcome is decided, so file admission may reopen and the next deferred
    /// job is eligible at once for one attempt.
    fn retry_reserved(&self, key: TextureKey, revision: u64) {
        {
            let mut core = self.lock();
            if core.retrying == Some(Retry { key, revision }) {
                core.retrying = None;
                core.front_now = !core.deferred.is_empty();
                core.settle(&self.ledger);
            }
        }
        self.wake.notify_all();
    }

    #[cfg(test)]
    fn at(&self, stage: Stage, key: TextureKey) {
        self.hook_call(stage, Some(key));
    }

    #[cfg(test)]
    fn hook_call(&self, stage: Stage, key: Option<TextureKey>) {
        let hook = self
            .hook
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(hook) = hook {
            hook(stage, key);
        }
    }

    #[cfg(test)]
    fn gate(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        self.gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Which wakes to call once the locks are released.
#[derive(Default, Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one independent flag per wake"
)]
struct Owed {
    jobs: bool,
    uploads: bool,
    /// Retirement work is pending: set the flag and call the wake.
    retirements: bool,
    /// The backend's retirement queue went non-empty: call the wake (the flag is not set).
    backend: bool,
    /// What the loader threads wait for changed: wake them.
    loaders: bool,
}

/// The resource service: a cloneable handle on shared state (clones share one budget ledger, so their reservations are
/// interchangeable).
#[derive(Clone)]
pub struct ResourceService {
    shared: Arc<Shared>,
}

impl fmt::Debug for ResourceService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResourceService")
            .field("mode", &self.shared.mode)
            .field("ledger", &self.shared.ledger.id())
            .finish_non_exhaustive()
    }
}

impl ResourceService {
    /// A service with `limits` and no wakes.
    ///
    /// # Errors
    ///
    /// [`ServiceError`]: a limit outside its range, or an identity that cannot be issued.
    pub fn new(limits: ResourceLimits, mode: Mode) -> Result<Self, ServiceError> {
        Self::with_wakes(limits, mode, ServiceWakes::default())
    }

    /// A service with `limits` and the given wakes.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn with_wakes(
        limits: ResourceLimits,
        mode: Mode,
        wakes: ServiceWakes,
    ) -> Result<Self, ServiceError> {
        Self::with_assets(limits, mode, wakes, None)
    }

    /// A service with `limits`, the given wakes and, if there is one, the configured assets root its file sources
    /// ([`load`](Self::load), [`reload`](Self::reload)) open files below. Nothing of the root is looked at yet: it is
    /// resolved by the first job that opens a file, on a loader thread.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn with_assets(
        limits: ResourceLimits,
        mode: Mode,
        wakes: ServiceWakes,
        assets: Option<Assets>,
    ) -> Result<Self, ServiceError> {
        Self::build(limits, mode, wakes, assets, LedgerId::allocate())
    }

    pub(crate) fn build(
        limits: ResourceLimits,
        mode: Mode,
        wakes: ServiceWakes,
        assets: Option<Assets>,
        ledger_id: Result<LedgerId, LedgerIdsExhausted>,
    ) -> Result<Self, ServiceError> {
        limits.validate().map_err(ServiceError::Limits)?;
        let ledger = Ledger::new(
            ledger_id,
            u64::from(limits.max_pending_jobs),
            limits.cpu_pixel_bytes,
            limits.metadata_bytes,
        )
        .map_err(ServiceError::Ledger)?;
        let notify = Arc::new(Notify {
            retirement_pending: AtomicBool::new(false),
            wakes,
            shared: OnceLock::new(),
        });
        let wake: crate::registry::RetiringWake = {
            let notify = Arc::clone(&notify);
            Arc::new(move || notify.retirement_work())
        };
        let registry = TextureRegistry::new(limits.max_textures, Some(wake))
            .map_err(ServiceError::Registry)?;
        let id = registry.id();
        let shared = Arc::new(Shared {
            mode,
            limits,
            registry,
            ledger,
            notify: Arc::clone(&notify),
            core: Mutex::new(Core {
                closed: false,
                detached: false,
                jobs: IndexedQueue::new(id),
                file_jobs: IndexedQueue::new(id),
                deferred: Vec::new(),
                retrying: None,
                front_now: false,
                next_seq: 1,
                next_stamp: 0,
                queue_peak: 0,
                loaders_stop: false,
                uploads: IndexedQueue::new(id),
                backend_retire: VecDeque::new(),
                finishing: IndexedQueue::new(id),
                blocked_streak: 0,
                #[cfg(test)]
                visited: 0,
            }),
            wake: Condvar::new(),
            assets: assets.map(Arc::new),
            counters: Counters::default(),
            #[cfg(test)]
            gate: Mutex::new(None),
            #[cfg(test)]
            waiting: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            hook: Mutex::new(None),
            #[cfg(feature = "test-support")]
            job_gate: Mutex::new(None),
        });
        // The loaders sleep in `shared`; the registry's retiring wake and a return of CPU bytes wake them through
        // a weak reference, so neither keeps the service alive.
        let _ = notify.shared.set(Arc::downgrade(&shared));
        let weak = Arc::downgrade(&shared);
        shared.ledger.capacity.set_notifier(Box::new(move || {
            if let Some(shared) = weak.upgrade() {
                shared.wake_loaders();
            }
        }));
        Ok(Self { shared })
    }

    /// The limits.
    #[must_use]
    pub fn limits(&self) -> ResourceLimits {
        self.shared.limits
    }

    /// The mode.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.shared.mode
    }

    /// The identity of this service's budget ledger (shared by its clones).
    #[must_use]
    pub fn ledger_id(&self) -> LedgerId {
        self.shared.ledger.id()
    }

    /// The budgets' numbers and the registry's.
    #[must_use]
    pub fn stats(&self) -> ServiceStats {
        let counters = &self.shared.counters;
        // One look at the queues, the lock released before the figures are assembled.
        let (deferred_now, queue_depth_peak) = {
            let core = self.shared.lock();
            (core.deferred.len(), core.queue_peak as u64)
        };
        ServiceStats {
            jobs: self.shared.ledger.stats(BudgetKind::JobSlots),
            cpu_pixels: self.shared.ledger.stats(BudgetKind::CpuPixelBytes),
            metadata: self.shared.ledger.stats(BudgetKind::MetadataBytes),
            registry: self.shared.registry.stats(),
            loaders: LoaderStats {
                job_panics: counters.job_panics.load(Ordering::SeqCst),
                loop_failures: counters.loop_failures.load(Ordering::SeqCst),
                reservation_attempts: counters.attempts.load(Ordering::SeqCst),
                deferrals: counters.deferrals.load(Ordering::SeqCst),
                deferred_now,
                files_decoded: counters.files_decoded.load(Ordering::SeqCst),
                decoded_bytes: counters.decoded_bytes.load(Ordering::SeqCst),
                queue_depth_peak,
                inline_finishes: counters.inline_finished.load(Ordering::SeqCst),
                inline_refusals: counters.finish_refused.load(Ordering::SeqCst),
                referenced_at_teardown: counters.referenced_at_teardown.load(Ordering::SeqCst),
            },
        }
    }

    /// Gives the service a hold on named file jobs (feature `test-support`: for the tests of the crates built on the
    /// service; there is no such call, and no check in the job, without the feature).
    #[cfg(feature = "test-support")]
    pub fn set_job_gate(&self, gate: Arc<crate::jobgate::JobGate>) {
        *self
            .shared
            .job_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(gate);
    }

    fn fire(&self, owed: Owed) {
        let wakes = &self.shared.notify.wakes;
        if owed.jobs
            && let Some(wake) = &wakes.jobs
        {
            wake();
        }
        if owed.uploads
            && let Some(wake) = &wakes.uploads
        {
            wake();
        }
        if owed.retirements {
            self.shared.notify.retirement_work();
        } else if owed.backend
            && let Some(wake) = &wakes.retirements
        {
            wake();
        }
        if owed.loaders {
            self.shared.wake_loaders();
        }
    }

    /// A job slot and its metadata charge, or the refusal.
    fn ticket(&self) -> Result<Ticket, AdmitError> {
        let ledger = &self.shared.ledger;
        let slot = ledger
            .reserve(BudgetKind::JobSlots, 1)
            .map_err(|_| AdmitError::JobsFull)?;
        let metadata = ledger
            .reserve(BudgetKind::MetadataBytes, JOB_METADATA_BYTES)
            .map_err(|short| AdmitError::Capacity {
                budget: BudgetKind::MetadataBytes,
                requested: short.requested,
                available: short.available,
            })?;
        Ok(Ticket {
            _slot: slot,
            #[cfg(test)]
            _between: SlotReturned(Arc::downgrade(&self.shared)),
            _metadata: metadata,
        })
    }

    // -----------------------------------------------------------------------------------------
    // Caller side
    // -----------------------------------------------------------------------------------------

    /// Reserves the CPU pixel bytes of a `width` x `height` image, before the caller allocates its buffer (so the
    /// allocation is inside the engine's bound). There is no way to submit pixels without such a reservation.
    ///
    /// # Errors
    ///
    /// [`AdmitError::TooLarge`] for a side outside 1 to 8192 or more than 64 MiB of RGBA8 (checked before any
    /// reservation), [`AdmitError::Capacity`] when the CPU budget cannot cover it.
    pub fn reserve_pixels(&self, width: u32, height: u32) -> Result<PixelReservation, AdmitError> {
        let too_large = AdmitError::TooLarge { width, height };
        if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
            return Err(too_large);
        }
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .filter(|bytes| *bytes <= MAX_DECODED_BYTES as u64)
            .ok_or(too_large)?;
        let reservation = self
            .shared
            .ledger
            .reserve(BudgetKind::CpuPixelBytes, bytes)
            .map_err(|short| AdmitError::Capacity {
                budget: BudgetKind::CpuPixelBytes,
                requested: short.requested,
                available: short.available,
            })?;
        Ok(PixelReservation::new(reservation, width, height))
    }

    /// Synchronously decodes and admits a PNG as revision 1, reserving its CPU bytes before the decoder allocates them.
    /// Unlike [`load`](Self::load), this compatibility operation creates no loader job: the finished revision waits
    /// directly for the backend's upload step.
    ///
    /// # Errors
    ///
    /// Returns the original assets error, a shared budget refusal, `Ended`, `NoAssets`, or `Mode(Headless)`. On every
    /// refusal all reservations and decoded pixels are returned and no registry entry remains.
    pub fn create_decoded(&self, name: &str) -> Result<Decoded, LegacyAdmitError> {
        if self.shared.mode != Mode::Backend {
            return Err(LegacyAdmitError::Mode(self.shared.mode));
        }
        let assets = self
            .shared
            .assets
            .as_ref()
            .ok_or(LegacyAdmitError::NoAssets)?;
        {
            let core = self.shared.lock();
            if core.closed {
                return Err(LegacyAdmitError::Ended);
            }
        }

        let opened = assets.open_reserving(name, &DecodeLimits::SHIPPED, |bytes| {
            self.shared.counters.attempts.fetch_add(1, Ordering::SeqCst);
            match self.shared.ledger.reserve(BudgetKind::CpuPixelBytes, bytes) {
                Ok(reservation) => {
                    #[cfg(test)]
                    self.shared.hook_call(Stage::Reserved, None);
                    Ok(reservation)
                }
                Err(short) => Err(short),
            }
        });
        let (image, reservation) = match opened {
            Ok(decoded) => decoded,
            Err(OpenReserveError::Assets(error)) => {
                return Err(LegacyAdmitError::Assets(error));
            }
            Err(OpenReserveError::Refused(short)) => {
                return Err(LegacyAdmitError::Budget {
                    budget: LegacyBudget::CpuPixelBytes,
                    requested: short.requested,
                    available: short.available,
                });
            }
        };
        let (width, height) = (image.width(), image.height());
        let mut discards = Discards::default();
        let mut owed = Owed::default();
        let key = {
            let mut core = self.shared.lock();
            if core.closed {
                return Err(LegacyAdmitError::Ended);
            }
            let charge = self
                .shared
                .ledger
                .reserve(BudgetKind::MetadataBytes, RESOURCE_METADATA_BYTES)
                .map_err(|short| LegacyAdmitError::Budget {
                    budget: LegacyBudget::MetadataBytes,
                    requested: short.requested,
                    available: short.available,
                })?;
            let state = ResourceState::new(charge, false);
            let key = match self.shared.registry.create_returning(state) {
                Ok(key) => key,
                Err((error, state)) => {
                    discards.states.push(state);
                    return Err(match error {
                        CreateError::Closed => LegacyAdmitError::Ended,
                        CreateError::Full { .. } | CreateError::OutOfMemory => {
                            LegacyAdmitError::Budget {
                                budget: LegacyBudget::RegistrySlots,
                                requested: 1,
                                available: 0,
                            }
                        }
                    });
                }
            };
            let revision = PixelRevision::from_decoded(key, 1, image, reservation);
            let _ = self.shared.registry.with_entry_value(key, |state| {
                state.waiting = Some(revision);
            });
            owed.uploads = core.uploads.is_empty();
            core.uploads.push_back(key);
            key
        };
        drop(discards);
        self.fire(owed);
        Ok(Decoded { key, width, height })
    }

    /// Admits a new resource whose first request, revision 1, is `pixels`. Admission is transactional: a registry slot,
    /// the resource's metadata charge and one job slot (with its metadata charge) are all reserved, or none is.
    ///
    /// # Errors
    ///
    /// [`Refused`] with the reason and the pixels (and their reservation) handed back; nothing changed.
    pub fn create_from_pixels(&self, pixels: CpuPixels) -> Result<TextureKey, Refused<CpuPixels>> {
        if !pixels.belongs_to(&self.shared.ledger) {
            return Err(refused(AdmitError::ForeignReservation, pixels));
        }
        let mut discards = Discards::default();
        let mut owed = Owed::default();
        let key = {
            let mut core = self.shared.lock();
            if core.closed {
                return Err(refused(AdmitError::Ended, pixels));
            }
            let ticket = match self.ticket() {
                Ok(ticket) => ticket,
                Err(error) => return Err(refused(error, pixels)),
            };
            let charge = match self
                .shared
                .ledger
                .reserve(BudgetKind::MetadataBytes, RESOURCE_METADATA_BYTES)
            {
                Ok(charge) => charge,
                Err(short) => {
                    return Err(refused(
                        AdmitError::Capacity {
                            budget: BudgetKind::MetadataBytes,
                            requested: short.requested,
                            available: short.available,
                        },
                        pixels,
                    ));
                }
            };
            let state = ResourceState::new(charge, self.shared.mode == Mode::Headless);
            let key = match self.shared.registry.create_returning(state) {
                Ok(key) => key,
                Err((error, state)) => {
                    // The state comes back to be dropped after the locks are released.
                    discards.states.push(state);
                    let error = match error {
                        CreateError::Closed => AdmitError::Ended,
                        CreateError::Full { .. } | CreateError::OutOfMemory => {
                            AdmitError::RegistryFull
                        }
                    };
                    return Err(refused(error, pixels));
                }
            };
            let stamp = core.stamp();
            let mut request = Some(Request {
                revision: 1,
                source: Source::Pixels(pixels),
                ticket,
                stamp,
            });
            let _ = self
                .shared
                .registry
                .with_entry_value(key, |state| state.queued = request.take());
            if let Some(request) = request {
                discards.requests.push(request);
            }
            owed.jobs = core.jobs.is_empty() && core.file_jobs.is_empty();
            owed.loaders = true;
            core.push_job(key);
            key
        };
        drop(discards);
        self.fire(owed);
        Ok(key)
    }

    /// Admits the next revision of an existing resource. It needs live public ownership. A queued request is replaced in
    /// place (its pixels and CPU reservation are dropped, and its job slot passes to the new request, so this succeeds
    /// even when the job budget is full); otherwise it takes a new job slot. Every older revision is superseded: an
    /// executing job's result will be discarded, a revision waiting for upload is dropped at once, and a revision the
    /// backend has taken will be rejected at commit. A resource loaded from a file is [`reload`](Self::reload)ed
    /// instead (`NotACpuSource`).
    ///
    /// # Errors
    ///
    /// [`Refused`] with the reason and the pixels handed back; nothing changed.
    pub fn replace_pixels(
        &self,
        key: TextureKey,
        pixels: CpuPixels,
    ) -> Result<u64, Refused<CpuPixels>> {
        if !pixels.belongs_to(&self.shared.ledger) {
            return Err(refused(AdmitError::ForeignReservation, pixels));
        }
        let mut discards = Discards::default();
        let mut owed = Owed::default();
        let mut source = Some(pixels);
        let revision = {
            let mut core = self.shared.lock();
            if core.closed {
                return Err(give_back(AdmitError::Ended, &mut source));
            }
            let fresh_stamp = core.stamp();
            let outcome = self
                .shared
                .registry
                .with_entry_lifecycle(key, |life, state| {
                    if !matches!(life, Lifecycle::Live { public: true, .. }) {
                        return Err(AdmitError::Handle(HandleError::Stale(key)));
                    }
                    if state.file.is_some() {
                        return Err(AdmitError::NotACpuSource);
                    }
                    let Some(revision) = state.requested.checked_add(1) else {
                        return Err(AdmitError::RevisionsExhausted);
                    };
                    let (ticket, stamp) = match state.queued.take() {
                        // Replaced in place: the old job slot passes to the new request, which keeps its place.
                        Some(old) => take_over(old, &mut discards),
                        None => (self.ticket()?, fresh_stamp),
                    };
                    let Some(pixels) = source.take() else {
                        return Err(AdmitError::Ended);
                    };
                    state.queued = Some(Request {
                        revision,
                        source: Source::Pixels(pixels),
                        ticket,
                        stamp,
                    });
                    state.requested = revision;
                    // A revision waiting for upload is superseded: dropped at once, and its position in the upload
                    // queue goes with it (a replacement is new work, queued when its own job completes).
                    let superseded = state.waiting.take().map(|waiting| {
                        discards.revisions.push(waiting);
                    });
                    Ok((revision, state.executing.is_none(), superseded.is_some()))
                });
            match outcome {
                Ok(Ok((revision, runnable, superseded))) => {
                    if superseded {
                        core.uploads.remove(key.slot());
                    }
                    requeue(
                        &mut core,
                        &self.shared.ledger,
                        key,
                        false,
                        runnable,
                        &mut owed,
                    );
                    revision
                }
                Ok(Err(error)) => return Err(give_back(error, &mut source)),
                Err(error) => return Err(give_back(AdmitError::Handle(error), &mut source)),
            }
        };
        drop(discards);
        self.fire(owed);
        Ok(revision)
    }

    /// Admits a new resource loaded from the PNG file `name` below the configured assets root; its first request,
    /// revision 1, is a file job. **On the caller's thread this makes no file-system call** (`docs/scene-design.md`
    /// §5.2): it checks the name's length and rules and then admits transactionally, as
    /// [`create_from_pixels`](Self::create_from_pixels) does (a registry slot, the resource's metadata charge plus the
    /// name's length, one job slot with its metadata charge). The file is opened and decoded by a job; its failures
    /// arrive asynchronously in [`status`](Self::status). Each call makes a new resource, even for a name already
    /// loaded (there is no implicit deduplication).
    ///
    /// # Errors
    ///
    /// [`AdmitError::NoAssets`] without a configured root, [`AdmitError::PathTooLong`] over [`MAX_PATH_BYTES`] bytes
    /// (checked first, so a refused name in a message is bounded), [`AdmitError::Name`] for a name [`Assets::check_name`]
    /// refuses, and the capacity, `Ended` and handle errors of any admission. Nothing changes on a refusal.
    pub fn load(&self, name: &str) -> Result<TextureKey, AdmitError> {
        if self.shared.assets.is_none() {
            return Err(AdmitError::NoAssets);
        }
        if name.len() > MAX_PATH_BYTES {
            return Err(AdmitError::PathTooLong { bytes: name.len() });
        }
        Assets::check_name(name).map_err(|error| AdmitError::Name(error.to_string()))?;
        // Allocated before the lock.
        let name: Arc<str> = Arc::from(name);
        let mut discards = Discards::default();
        let mut owed = Owed::default();
        let key = {
            let mut core = self.shared.lock();
            if core.closed {
                return Err(AdmitError::Ended);
            }
            let ticket = self.ticket()?;
            let ledger = &self.shared.ledger;
            let charge = ledger
                .reserve(BudgetKind::MetadataBytes, RESOURCE_METADATA_BYTES)
                .map_err(|short| capacity(BudgetKind::MetadataBytes, short))?;
            let name_charge = ledger
                .reserve(BudgetKind::MetadataBytes, name.len() as u64)
                .map_err(|short| capacity(BudgetKind::MetadataBytes, short))?;
            let mut state = ResourceState::new(charge, self.shared.mode == Mode::Headless);
            state.file = Some(FileSource {
                name,
                _charge: name_charge,
            });
            let key = match self.shared.registry.create_returning(state) {
                Ok(key) => key,
                Err((error, state)) => {
                    // The state comes back to be dropped after the locks are released.
                    discards.states.push(state);
                    return Err(match error {
                        CreateError::Closed => AdmitError::Ended,
                        CreateError::Full { .. } | CreateError::OutOfMemory => {
                            AdmitError::RegistryFull
                        }
                    });
                }
            };
            let stamp = core.stamp();
            let mut request = Some(Request {
                revision: 1,
                source: Source::File,
                ticket,
                stamp,
            });
            let _ = self
                .shared
                .registry
                .with_entry_value(key, |state| state.queued = request.take());
            if let Some(request) = request {
                discards.requests.push(request);
            }
            owed.jobs = core.jobs.is_empty() && core.file_jobs.is_empty();
            owed.loaders = true;
            core.push_file_job(key);
            key
        };
        drop(discards);
        self.fire(owed);
        Ok(key)
    }

    /// Admits the next revision of a **file** resource: the same name, decoded again by a new job, with the coalescing
    /// of [`replace_pixels`](Self::replace_pixels) (a queued request, a deferred one included, is replaced in place and
    /// keeps its job slot). It needs live public ownership. A failed reload changes neither the committed nor the shown
    /// revision (`docs/scene-design.md` §5.3): the old content stays, with the error beside it. There is no file watching.
    ///
    /// # Errors
    ///
    /// [`AdmitError::NotAFileSource`] for a resource made from a Rust buffer, and the handle, capacity and `Ended`
    /// errors of any admission. Nothing changes on a refusal.
    pub fn reload(&self, key: TextureKey) -> Result<u64, AdmitError> {
        let mut discards = Discards::default();
        let mut owed = Owed::default();
        let revision = {
            let mut core = self.shared.lock();
            if core.closed {
                return Err(AdmitError::Ended);
            }
            let fresh_stamp = core.stamp();
            let outcome = self
                .shared
                .registry
                .with_entry_lifecycle(key, |life, state| {
                    if !matches!(life, Lifecycle::Live { public: true, .. }) {
                        return Err(AdmitError::Handle(HandleError::Stale(key)));
                    }
                    if state.file.is_none() {
                        return Err(AdmitError::NotAFileSource);
                    }
                    let Some(revision) = state.requested.checked_add(1) else {
                        return Err(AdmitError::RevisionsExhausted);
                    };
                    let (ticket, stamp) = match state.queued.take() {
                        Some(old) => take_over(old, &mut discards),
                        None => (self.ticket()?, fresh_stamp),
                    };
                    state.queued = Some(Request {
                        revision,
                        source: Source::File,
                        ticket,
                        stamp,
                    });
                    state.requested = revision;
                    let superseded = state.waiting.take().map(|waiting| {
                        discards.revisions.push(waiting);
                    });
                    Ok((revision, state.executing.is_none(), superseded.is_some()))
                });
            match outcome {
                Ok(Ok((revision, runnable, superseded))) => {
                    if superseded {
                        core.uploads.remove(key.slot());
                    }
                    requeue(
                        &mut core,
                        &self.shared.ledger,
                        key,
                        true,
                        runnable,
                        &mut owed,
                    );
                    revision
                }
                Ok(Err(error)) => return Err(error),
                Err(error) => return Err(AdmitError::Handle(error)),
            }
        };
        drop(discards);
        self.fire(owed);
        Ok(revision)
    }
}

/// The capacity refusal of `budget`.
fn capacity(budget: BudgetKind, short: Shortfall) -> AdmitError {
    AdmitError::Capacity {
        budget,
        requested: short.requested,
        available: short.available,
    }
}

/// Whether the decision recorded no deferral (a decision that never came counts as none).
fn verdict_not_deferred(verdict: Option<&Verdict>) -> bool {
    verdict.is_none_or(|verdict| verdict.deferral.is_none())
}

/// A queued request replaced in place: its source is dropped later (outside the locks), and its job slot and place are
/// handed to the new request.
fn take_over(old: Request, discards: &mut Discards) -> (Ticket, u64) {
    let Request {
        source,
        ticket,
        stamp,
        ..
    } = old;
    if let Source::Pixels(pixels) = source {
        discards.pixels.push(pixels);
    }
    (ticket, stamp)
}

/// After a request was (re)queued on `key`'s resource: a deferred request it replaces leaves the deferred queue (and
/// file admission reopens if that empties it); a resource with no job executing goes into its queue unless it is in one
/// already (a replaced request keeps its place); the loaders are woken.
fn requeue(
    core: &mut Core,
    ledger: &Ledger,
    key: TextureKey,
    file: bool,
    runnable: bool,
    owed: &mut Owed,
) {
    if core.undefer(key) {
        core.settle(ledger);
    }
    if runnable && !core.is_queued(key) {
        owed.jobs = core.jobs.is_empty() && core.file_jobs.is_empty();
        if file {
            core.push_file_job(key);
        } else {
            core.push_job(key);
        }
    }
    owed.loaders = true;
}

/// Where a finished job's output lands.
#[derive(Clone, Copy)]
struct Landing {
    revision: u64,
    /// Whether the result is accepted: a live resource, the service open, the revision still the requested one.
    accept: bool,
    /// The admission stamp a deferred request takes.
    deferred_stamp: u64,
}

/// What recording a job's result did.
struct Recorded {
    /// A finished revision now waits for upload.
    upload: bool,
    /// The job was deferred: the epoch read before its refused attempt.
    deferral: Option<u64>,
}

/// Records a finished job's output in the resource under the registry lock (the acceptance decision): shown at once
/// (headless), waiting for upload, an error, or a deferral that puts the request back with its job slot; a result that is
/// not accepted goes to `discards` to be dropped outside the locks.
fn record_result(
    state: &mut ResourceState,
    landing: Landing,
    ledger: &Arc<Ledger>,
    (result, ticket): (&mut Option<JobResult>, &mut Option<Ticket>),
    discards: &mut Discards,
) -> Recorded {
    let Landing {
        revision,
        accept,
        deferred_stamp,
    } = landing;
    let mut recorded = Recorded {
        upload: false,
        deferral: None,
    };
    match result.take() {
        Some(JobResult::Done(pixels)) if accept && state.headless => {
            state.show(
                Sized {
                    revision,
                    width: pixels.width(),
                    height: pixels.height(),
                },
                discards,
            );
            discards.revisions.push(pixels);
        }
        Some(JobResult::Done(pixels)) if accept => {
            if let Some(old) = state.waiting.replace(pixels) {
                discards.revisions.push(old);
            }
            recorded.upload = true;
        }
        Some(JobResult::Failed(failure)) if accept => {
            state.record_error(ledger, revision, ErrorKind::Job, failure.message, discards);
        }
        Some(JobResult::Deferred { epoch }) if accept => {
            // The request goes back into the resource with its job slot and metadata; nothing image-sized is held.
            if let Some(ticket) = ticket.take() {
                state.queued = Some(Request {
                    revision,
                    source: Source::File,
                    ticket,
                    stamp: deferred_stamp,
                });
                recorded.deferral = Some(epoch);
            }
        }
        Some(JobResult::Done(pixels)) => discards.revisions.push(pixels),
        Some(JobResult::Failed(failure)) => discards.strings.push(failure.message),
        Some(JobResult::Deferred { .. }) | None => {}
    }
    recorded
}

/// How a job ended, as `complete` hands it to the bookkeeping under the service lock.
#[derive(Clone, Copy)]
struct Ended {
    key: TextureKey,
    revision: u64,
    seq: u64,
    retry: bool,
}

/// What a decision made under the registry lock tells the service to do next.
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts about one decision"
)]
struct Verdict {
    /// A finished revision was queued for upload.
    upload: bool,
    /// The resource has a queued successor that may now be taken.
    successor: bool,
    /// The successor (or the resource) is a file resource.
    file: bool,
    /// The resource is retiring and its executing job has ended: retirement may proceed.
    retiring: bool,
    /// The retirement can be finished inline: the service is closed, the resource was drained from the registry's
    /// queue and nothing is left to wait for.
    finish: bool,
    /// The job was deferred (the capacity epoch read before its refused attempt): its request waits in the resource.
    deferral: Option<u64>,
}

/// What a loader thread should do next.
pub(crate) enum Work {
    /// Retirement work is pending: run [`ResourceService::maintain`].
    Retire,
    /// A job to run and complete.
    Job(Job),
    /// The pool is ending: leave the loop.
    Stop,
}

impl ResourceService {
    /// Passes through to the registry: whether `key` has live public ownership.
    ///
    /// # Errors
    ///
    /// [`HandleError`] as for [`TextureRegistry::check`].
    pub fn check(&self, key: TextureKey) -> Result<(), HandleError> {
        self.shared.registry.check(key)
    }

    /// Passes through to the registry: a retained reference to the resource of `key` (it needs live public ownership).
    ///
    /// # Errors
    ///
    /// [`HandleError`] as for [`TextureRegistry::retain`].
    pub fn retain(&self, key: TextureKey) -> Result<TextureRef<ResourceState>, HandleError> {
        self.shared.registry.retain(key)
    }

    /// Passes through to the registry: ends public ownership of `key`. When no reference remains the resource becomes
    /// retiring and the retirement work is flagged; [`maintain`](Self::maintain) does the rest.
    ///
    /// # Errors
    ///
    /// [`HandleError`] as for [`TextureRegistry::release`].
    pub fn release(&self, key: TextureKey) -> Result<(), HandleError> {
        self.shared.registry.release(key)
    }

    /// A snapshot of the resource of `key`, whatever its ownership; it never waits.
    ///
    /// # Errors
    ///
    /// [`HandleError::Foreign`], or [`HandleError::Stale`] once the resource's retirement has finished.
    pub fn status(&self, key: TextureKey) -> Result<TextureStatus, HandleError> {
        self.shared
            .registry
            .with_entry_value(key, |state| state.status())
    }

    /// A snapshot of the resource a retained reference holds; it never waits.
    #[must_use]
    pub fn status_of(&self, reference: &TextureRef<ResourceState>) -> TextureStatus {
        reference.with_value(|state| state.status())
    }

    // -----------------------------------------------------------------------------------------
    // The stepped executor
    // -----------------------------------------------------------------------------------------

    /// Hands out the oldest queued request as a [`Job`]: it becomes executing. Jobs come out FIFO across resources and
    /// never two at once for one resource.
    ///
    /// File jobs are taken in order too, but only while the deferred queue is empty and no retry is in flight; a CPU
    /// request may go ahead of a file job that has to wait (its bytes were reserved at admission). When the deferred
    /// queue's front is eligible (the capacity epoch changed since its refused attempt) it is handed out first, alone,
    /// as a retry.
    ///
    /// The step is bounded: it looks at no more than eight queued resources, the ones whose retirement the
    /// registry has begun and [`maintain`](Self::maintain) has not yet cleaned up being the only ones it can pass
    /// over. If that budget ends before a job is found while work is still queued, the jobs wake is called again, so
    /// the work is not lost and the caller is asked to come back.
    #[must_use]
    pub fn take_job(&self) -> Option<Job> {
        let mut core = self.shared.lock();
        let (job, more) = self.take_locked(&mut core);
        drop(core);
        if more {
            self.fire(Owed {
                jobs: true,
                ..Owed::default()
            });
        }
        job
    }

    /// `take_job` under the service lock. Also says whether the step's budget ended with eligible work still queued.
    fn take_locked(&self, core: &mut Core) -> (Option<Job>, bool) {
        let epoch = self.shared.ledger.capacity.epoch();
        let mut job = None;
        let mut visits = 0;
        // A deferred job whose bytes may have returned goes first, alone: the front, by take order.
        while job.is_none() && visits < JOB_VISITS && core.front_eligible(epoch) {
            visits += 1;
            let entry = core.deferred.remove(0);
            let taken = self
                .shared
                .registry
                .with_entry_lifecycle(entry.key, |life, state| {
                    if life == Lifecycle::Retiring || state.discard || state.executing.is_some() {
                        return None;
                    }
                    let request = state.queued.take()?;
                    state.executing = Some(request.revision);
                    Some(self.job_of(entry.key, request, state, entry.seq, true))
                })
                .ok()
                .flatten();
            if taken.is_some() {
                core.retrying = Some(Retry {
                    key: entry.key,
                    revision: entry.revision,
                });
                core.front_now = false;
            }
            // Otherwise the entry is retiring: `maintain` takes its request and returns the job slot.
            job = taken;
        }
        // Then the older head of the two ordinary queues. A file head waits while file admission is closed.
        while job.is_none() && visits < JOB_VISITS {
            let cpu = core.jobs.front();
            let file = if core.file_admission_open() {
                core.file_jobs.front()
            } else {
                None
            };
            let (key, is_file) = match (cpu, file) {
                (None, None) => break,
                (Some(key), None) => (key, false),
                (None, Some(key)) => (key, true),
                (Some(cpu), Some(file)) => {
                    if self.stamp_of(cpu) <= self.stamp_of(file) {
                        (cpu, false)
                    } else {
                        (file, true)
                    }
                }
            };
            if is_file {
                core.file_jobs.pop_front();
            } else {
                core.jobs.pop_front();
            }
            visits += 1;
            let seq = core.next_seq;
            job = self
                .shared
                .registry
                .with_entry_lifecycle(key, |life, state| {
                    if life == Lifecycle::Retiring || state.executing.is_some() || state.discard {
                        return None;
                    }
                    let request = state.queued.take()?;
                    state.executing = Some(request.revision);
                    Some(self.job_of(key, request, state, seq, false))
                })
                .ok()
                .flatten();
            if job.is_some() && is_file {
                core.next_seq += 1;
            }
        }
        core.settle(&self.shared.ledger);
        #[cfg(test)]
        {
            core.visited += visits;
        }
        let more = job.is_none()
            && (!core.jobs.is_empty()
                || (core.file_admission_open() && !core.file_jobs.is_empty())
                || core.front_eligible(epoch));
        (job, more)
    }

    /// The admission stamp of `key`'s queued request (zero for a queued entry that has none: it is passed over first).
    fn stamp_of(&self, key: TextureKey) -> u64 {
        self.shared
            .registry
            .with_entry_value(key, |state| {
                state.queued.as_ref().map(|request| request.stamp)
            })
            .ok()
            .flatten()
            .unwrap_or(0)
    }

    /// The job of `request` on `key`'s resource.
    fn job_of(
        &self,
        key: TextureKey,
        request: Request,
        state: &ResourceState,
        seq: u64,
        retry: bool,
    ) -> Job {
        let Request {
            revision,
            source,
            ticket,
            ..
        } = request;
        let (work, seq) = match source {
            Source::Pixels(pixels) => (JobWork::Pixels(pixels), 0),
            Source::File => (
                JobWork::File {
                    name: state
                        .file
                        .as_ref()
                        .map_or_else(|| Arc::from(""), |file| Arc::clone(&file.name)),
                },
                seq,
            ),
        };
        Job {
            key,
            revision,
            work,
            ticket,
            seq,
            retry,
            shared: Arc::clone(&self.shared),
        }
    }

    /// **For the loader threads only** (it blocks): the next thing a loader should do, sleeping until there is
    /// something. The runnable predicate is evaluated under the service lock, and every notifier changes the state it
    /// reads under that lock or takes it before notifying, so no wake is lost and nothing polls or sleeps on a timer:
    ///
    /// - the loaders are told to stop: [`Work::Stop`];
    /// - retirement work is pending: [`Work::Retire`];
    /// - a job can be taken (see [`take_job`](Self::take_job): a CPU request; the deferred queue's front when the
    ///   capacity epoch changed since its refused attempt and no retry is in flight; a file request when the deferred
    ///   queue is empty and no retry is in flight): [`Work::Job`].
    ///
    /// A spurious or unrelated wake re-evaluates the predicate and never retries the front while the epoch is
    /// unchanged. Every other method of the service is non-blocking.
    pub(crate) fn wait_for_work(&self) -> Work {
        let mut core = self.shared.lock();
        loop {
            if core.loaders_stop {
                return Work::Stop;
            }
            if self.shared.notify.retirement_pending.load(Ordering::SeqCst) {
                return Work::Retire;
            }
            let (job, more) = self.take_locked(&mut core);
            if let Some(job) = job {
                return Work::Job(job);
            }
            if !more {
                // Counted while the lock is still held: a returner that wants to wake this loader needs the lock.
                #[cfg(test)]
                self.shared.waiting.fetch_add(1, Ordering::SeqCst);
                core = core.wait(&self.shared.wake);
                #[cfg(test)]
                self.shared.waiting.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    /// Asks the loader threads to end (their `wait_for_work` returns [`Work::Stop`]) and wakes them.
    pub(crate) fn stop_loaders(&self) {
        self.shared.lock().loaders_stop = true;
        self.shared.wake.notify_all();
    }

    /// Lets loader threads run again (a pool being started; a stop that was only a failed start's clean-up).
    pub(crate) fn allow_loaders(&self) {
        self.shared.lock().loaders_stop = false;
    }

    /// Records that a loader thread ended by a failure of its loop outside any job.
    pub(crate) fn count_loop_failure(&self) {
        self.shared
            .counters
            .loop_failures
            .fetch_add(1, Ordering::SeqCst);
    }

    /// Records a job's output. A result that is superseded, or whose resource is retiring or whose service is closed or
    /// detached, is **discarded**: its pixels and reservations are dropped outside the locks and no content, readiness or
    /// error changes. Either way the job slot and the job's metadata return, the resource's executing-job bookkeeping is
    /// cleared, and a queued successor can be taken (or the retirement proceed).
    ///
    /// A file job's *deferral* (its reservation was refused while finished pixels were in transit) is an acceptance
    /// decision like this one, made under the registry lock: if the resource is retiring (its last reference dropped,
    /// not yet seen by [`maintain`](Self::maintain)) or the request is superseded, the job is discarded and its job
    /// slot returns; otherwise it joins the deferred queue, keeping its job slot and metadata and holding nothing
    /// image-sized.
    ///
    /// **After [`close`](Self::close)** (the pool's shutdown) a late completion may only clear its job's executing
    /// bookkeeping and return its job slot, job metadata and CPU bytes; it never queues an upload, changes a status,
    /// shows content or reopens admission. If it clears the last executing job of a retiring resource that
    /// `maintain` (or [`finish_teardown`](Self::finish_teardown)) had already drained and that waits for no backend
    /// proof, it finishes the retirement itself, inline; a resource not yet drained is left to the next drain.
    pub fn complete(&self, output: JobOutput) {
        let JobOutput {
            key,
            revision,
            ticket,
            seq,
            retry,
            result,
        } = output;
        let mut discards = Discards::default();
        let mut owed = Owed::default();
        let mut ticket = Some(ticket);
        let mut result = Some(result);
        let (finish, recorded) = {
            let mut core = self.shared.lock();
            let allowed = !core.closed && !core.detached;
            let inline = core.closed;
            let no_backend = self.shared.mode == Mode::Headless || core.detached;
            let deferred_stamp = if matches!(result, Some(JobResult::Deferred { .. })) {
                core.stamp()
            } else {
                0
            };
            let ledger = &self.shared.ledger;
            #[cfg(test)]
            let gate = self.shared.gate();
            let verdict = self
                .shared
                .registry
                .with_entry_lifecycle(key, |life, state| {
                    #[cfg(test)]
                    if let Some(gate) = &gate {
                        gate();
                    }
                    let cleared = state.executing == Some(revision);
                    if cleared {
                        state.executing = None;
                    }
                    let live = matches!(life, Lifecycle::Live { .. });
                    let accept = live && allowed && !state.discard && state.requested == revision;
                    let mut verdict = Verdict::default();
                    let recorded = record_result(
                        state,
                        Landing {
                            revision,
                            accept,
                            deferred_stamp,
                        },
                        ledger,
                        (&mut result, &mut ticket),
                        &mut discards,
                    );
                    verdict.upload = recorded.upload;
                    verdict.deferral = recorded.deferral;
                    verdict.file = state.file.is_some();
                    verdict.successor = cleared
                        && live
                        && !state.discard
                        && state.queued.is_some()
                        && verdict.deferral.is_none();
                    verdict.retiring = cleared && !live;
                    verdict.finish = verdict.retiring
                        && inline
                        && state.in_finishing
                        && state.executing.is_none()
                        && (state.backend_retired || no_backend);
                    if verdict.finish {
                        state.in_finishing = false;
                    }
                    verdict
                });
            // An entry that is gone (it cannot be, while a job executes) leaves nothing to record.
            let ended = Ended {
                key,
                revision,
                seq,
                retry,
            };
            self.after_decision(&mut core, ended, verdict.as_ref().ok(), &mut owed)
        };
        match result.take() {
            Some(JobResult::Done(pixels)) => discards.revisions.push(pixels),
            Some(JobResult::Failed(failure)) => discards.strings.push(failure.message),
            Some(JobResult::Deferred { .. }) | None => {}
        }
        if let Some(key) = finish {
            self.finish_inline(key, &mut discards);
            #[cfg(test)]
            self.shared.at(Stage::Finished, key);
        }
        #[cfg(test)]
        self.shared.at(Stage::Published, key);
        drop(discards);
        drop(ticket);
        self.fire(owed);
        #[cfg(test)]
        if recorded {
            self.shared.at(Stage::Recorded, key);
        }
        let _ = recorded;
        #[cfg(test)]
        self.shared.at(Stage::Completed, key);
    }

    /// What `complete` does under the service lock once the decision under the registry lock is made: the queues, the
    /// deferred queue, the retry's mark. Returns the key to finish inline, if any, and whether a deferral was recorded.
    fn after_decision(
        &self,
        core: &mut Core,
        ended: Ended,
        decided: Option<&Verdict>,
        owed: &mut Owed,
    ) -> (Option<TextureKey>, bool) {
        let Ended {
            key,
            revision,
            seq,
            retry,
        } = ended;
        let ledger = &self.shared.ledger;
        let (mut finish, mut recorded) = (None, false);
        if let Some(verdict) = decided {
            if verdict.successor && !core.is_queued(key) {
                owed.jobs = core.jobs.is_empty() && core.file_jobs.is_empty();
                if verdict.file {
                    core.push_file_job(key);
                } else {
                    core.push_job(key);
                }
                owed.loaders = true;
            }
            if verdict.upload {
                // The finished revision takes its own place in upload order: the back of the queue, whatever
                // position an earlier, superseded revision of this resource had (it left the queue when it was
                // dropped).
                owed.uploads = core.uploads.is_empty();
                core.uploads.push_back(key);
            }
            if verdict.retiring {
                // A retiring resource's job ended: its retirement may be finishable now.
                core.blocked_streak = 0;
            }
            owed.retirements = verdict.retiring && !verdict.finish;
            if let Some(epoch) = verdict.deferral {
                // (3) The deferral is recorded under the service lock, in take order, and the flag is raised
                // again: a clear between the attempt and here loses nothing. (4) The comparison of the epoch
                // with the recorded one is the eligibility test itself, so a return at any point since the
                // attempt makes the front eligible.
                let at = core.deferred.partition_point(|entry| entry.seq < seq);
                core.deferred.insert(
                    at,
                    Deferred {
                        key,
                        revision,
                        seq,
                        epoch,
                    },
                );
                ledger.capacity.raise_deferral();
                self.shared
                    .counters
                    .deferrals
                    .fetch_add(1, Ordering::SeqCst);
                owed.loaders = true;
                recorded = true;
            }
            if verdict.finish {
                core.finishing.remove(key.slot());
                finish = Some(key);
            }
        }
        if retry && core.retrying == Some(Retry { key, revision }) {
            // The retry ended without having reserved (refused again, superseded, failed, discarded): its outcome
            // is decided. After a refusal that returned it to the queue it is the front again, not eligible until
            // the epoch changes; otherwise the next front is eligible at once.
            core.retrying = None;
            core.front_now = verdict_not_deferred(decided) && !core.deferred.is_empty();
            owed.loaders = true;
        }
        core.settle(ledger);
        (finish, recorded)
    }

    /// Finishes the retirement of `key` (drained, nothing left to wait for) on the calling thread, after the locks; the
    /// state is dropped later with `discards`. A refusal cannot happen by construction; if one does it is counted and
    /// asserted in debug builds, never ignored silently.
    fn finish_inline(&self, key: TextureKey, discards: &mut Discards) {
        match self.shared.registry.finish_retirement(key) {
            Ok(state) => {
                self.shared
                    .counters
                    .inline_finished
                    .fetch_add(1, Ordering::SeqCst);
                discards.states.push(state);
            }
            Err(error) => {
                self.shared
                    .counters
                    .finish_refused
                    .fetch_add(1, Ordering::SeqCst);
                debug_assert!(false, "an inline retirement was refused: {error}");
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // The backend side
    // -----------------------------------------------------------------------------------------

    /// Moves at most `limit` revisions that finished their job and wait for upload to the backend, oldest first. The
    /// backend drops each `Arc<PixelRevision>` when it no longer needs the CPU bytes, which returns them.
    ///
    /// A step is bounded: it looks at no more than `limit` plus a fixed number of queued resources (the ones whose
    /// retirement has begun and [`maintain`](Self::maintain) has not yet cleaned up can only be passed over). A
    /// return smaller than `limit` therefore does not mean the queue is empty; when the budget ended with work
    /// still queued the uploads wake is called again.
    pub fn take_uploads(&self, out: &mut Vec<UploadRequest>, limit: usize) -> usize {
        // Reserved before the lock, so nothing below allocates under it.
        out.reserve(
            limit.min(usize::try_from(self.shared.limits.max_textures).unwrap_or(usize::MAX)),
        );
        let mut core = self.shared.lock();
        if core.detached {
            return 0;
        }
        // The step looks at no more than `limit` plus a fixed number of resources, whatever is queued behind them.
        let visit_budget = limit.saturating_add(UPLOAD_SKIPS);
        let mut moved = 0;
        let mut visits = 0;
        while moved < limit && visits < visit_budget {
            let Some(key) = core.uploads.pop_front() else {
                break;
            };
            visits += 1;
            let taken = self
                .shared
                .registry
                .with_entry_lifecycle(key, |life, state| {
                    if life == Lifecycle::Retiring {
                        return None;
                    }
                    let pixels = state.waiting.take()?;
                    state.taken = Some(Sized {
                        revision: pixels.revision(),
                        width: pixels.width(),
                        height: pixels.height(),
                    });
                    Some(UploadRequest {
                        key,
                        revision: pixels.revision(),
                        pixels,
                    })
                });
            if let Ok(Some(upload)) = taken {
                out.push(upload);
                moved += 1;
            }
        }
        #[cfg(test)]
        {
            core.visited += visits;
        }
        // The budget ended with work still queued: ask to be called again, so nothing is lost.
        let again = moved < limit && !core.uploads.is_empty();
        drop(core);
        if again {
            self.fire(Owed {
                uploads: true,
                ..Owed::default()
            });
        }
        moved
    }

    /// The compare-and-commit: commits `revision` only if it is the resource's current requested revision, the backend
    /// took it, and the resource is neither retiring (decided under the registry lock) nor closed nor detached. The
    /// committed revision may be used by future frames; it is shown only when [`adopted`](Self::adopted). Otherwise
    /// `Rejected`, and the backend retires its prepared native revision. The ordering is the lock's: a request admitted
    /// before the commit makes the candidate superseded; one admitted after it is a later replacement.
    #[must_use]
    pub fn try_commit(&self, key: TextureKey, revision: u64) -> CommitOutcome {
        let core = self.shared.lock();
        if core.closed || core.detached {
            return CommitOutcome::Rejected;
        }
        #[cfg(test)]
        let gate = self.shared.gate();
        let outcome = self
            .shared
            .registry
            .with_entry_lifecycle(key, |life, state| {
                #[cfg(test)]
                if let Some(gate) = &gate {
                    gate();
                }
                let taken = state.taken.filter(|taken| taken.revision == revision);
                if taken.is_some() {
                    state.taken = None;
                }
                match taken {
                    Some(taken) if life != Lifecycle::Retiring && state.requested == revision => {
                        state.committed = Some(taken);
                        CommitOutcome::Committed
                    }
                    _ => CommitOutcome::Rejected,
                }
            })
            .unwrap_or(CommitOutcome::Rejected);
        drop(core);
        outcome
    }

    /// The backend's acknowledgement that frames now use the committed revision: only then does it become the shown
    /// revision and the resource `ready` for it. An acknowledgement for anything but the current committed revision is
    /// ignored. An error older than the shown revision is cleared.
    pub fn adopted(&self, key: TextureKey, revision: u64) {
        let mut discards = Discards::default();
        {
            let core = self.shared.lock();
            #[cfg(test)]
            let gate = self.shared.gate();
            let _ = self
                .shared
                .registry
                .with_entry_lifecycle(key, |life, state| {
                    #[cfg(test)]
                    if let Some(gate) = &gate {
                        gate();
                    }
                    if life == Lifecycle::Retiring {
                        return;
                    }
                    if let Some(committed) = state.committed
                        && committed.revision == revision
                        && state.shown.is_none_or(|shown| shown.revision < revision)
                    {
                        state.show(committed, &mut discards);
                    }
                });
            drop(core);
        }
        drop(discards);
    }

    /// A resource-level upload failure (for example an over-budget texture). For the current requested revision it is
    /// recorded like a failed job: the committed and shown revisions are unchanged. For any other revision it is
    /// ignored. GPU failures that are not resource errors never come here.
    pub fn upload_failed(&self, key: TextureKey, revision: u64, message: &str) {
        let mut discards = Discards::default();
        {
            let core = self.shared.lock();
            let ledger = &self.shared.ledger;
            let message = message.to_owned();
            let _ = self
                .shared
                .registry
                .with_entry_lifecycle(key, |life, state| {
                    if life == Lifecycle::Retiring || state.requested != revision {
                        discards.strings.push(message);
                        return;
                    }
                    if state.taken.is_some_and(|taken| taken.revision == revision) {
                        state.taken = None;
                    }
                    state.record_error(ledger, revision, ErrorKind::Upload, message, &mut discards);
                });
            drop(core);
        }
        drop(discards);
    }

    /// Moves at most `limit` retiring resources the backend must retire (it calls [`retired`](Self::retired) for each
    /// once no native revision of that key remains in use) into `out`, oldest first.
    pub fn take_retirements(&self, out: &mut Vec<TextureKey>, limit: usize) -> usize {
        out.reserve(
            limit.min(usize::try_from(self.shared.limits.max_textures).unwrap_or(usize::MAX)),
        );
        let mut core = self.shared.lock();
        let mut moved = 0;
        while moved < limit {
            let Some(key) = core.backend_retire.pop_front() else {
                break;
            };
            out.push(key);
            moved += 1;
        }
        moved
    }

    /// The backend's proof that no native revision of the retiring `key` remains in use. After [`close`](Self::close)
    /// (the pool's shutdown) a proof that is the last thing a drained retiring resource waited for finishes its
    /// retirement inline; a resource not yet drained is left to the next drain.
    pub fn retired(&self, key: TextureKey) {
        let mut discards = Discards::default();
        let mut finish = false;
        let known = {
            let mut core = self.shared.lock();
            let inline = core.closed;
            let known = self
                .shared
                .registry
                .with_entry_lifecycle(key, |life, state| {
                    if life == Lifecycle::Retiring {
                        state.backend_retired = true;
                        finish = inline && state.in_finishing && state.executing.is_none();
                        if finish {
                            state.in_finishing = false;
                        }
                        true
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
            if known {
                // This proof may unblock a finishing resource that `maintain` has already found blocked.
                core.blocked_streak = 0;
            }
            if finish {
                core.finishing.remove(key.slot());
            }
            known
        };
        if finish {
            self.finish_inline(key, &mut discards);
        }
        drop(discards);
        if known && !finish {
            self.fire(Owed {
                retirements: true,
                ..Owed::default()
            });
        }
    }

    /// The backend handoff closes (shutdown closes it before stopping the GPU). Afterwards no upload is offered,
    /// revisions waiting for upload are dropped, any later [`try_commit`](Self::try_commit) is rejected, job results are
    /// discarded, and retirement no longer waits for [`retired`](Self::retired). A headless service has no backend: this
    /// does nothing.
    pub fn detach_backend(&self) {
        if self.shared.mode == Mode::Headless {
            return;
        }
        let mut discards = Discards::default();
        let owed;
        {
            let mut core = self.shared.lock();
            if core.detached {
                return;
            }
            core.detached = true;
            core.blocked_streak = 0;
            while let Some(key) = core.uploads.pop_front() {
                let _ = self.shared.registry.with_entry_value(key, |state| {
                    if let Some(waiting) = state.waiting.take() {
                        discards.revisions.push(waiting);
                    }
                });
            }
            core.backend_retire.clear();
            owed = Owed {
                retirements: !core.finishing.is_empty(),
                ..Owed::default()
            };
        }
        drop(discards);
        self.fire(owed);
    }

    // -----------------------------------------------------------------------------------------
    // Retirement, close
    // -----------------------------------------------------------------------------------------

    /// Whether retirement work has been flagged (by the registry's retiring wake, a discarded completion, `retired` or
    /// `detach_backend`) since the last [`maintain`](Self::maintain).
    #[must_use]
    pub fn retirement_pending(&self) -> bool {
        self.shared.notify.retirement_pending.load(Ordering::SeqCst)
    }

    /// Processes retirement work, at most `limit` resources per step: for each newly retiring resource the queued
    /// request is dropped (pixels and reservations returned outside the locks), a revision waiting for upload is
    /// dropped, an executing job is marked discard-only, and in `Backend` mode with a backend attached the key is
    /// queued for [`take_retirements`](Self::take_retirements); then every retiring resource with no executing job
    /// and (`retired` received, or no backend) has its retirement finished: its slot is reusable at the next
    /// generation and its state is dropped outside every lock, returning its metadata charge. **No caller-side call is
    /// needed after the last reference drops** for this to be reachable.
    #[must_use]
    pub fn maintain(&self, limit: usize) -> Maintained {
        let cap = limit.min(usize::try_from(self.shared.limits.max_textures).unwrap_or(usize::MAX));
        let mut fresh: Vec<TextureKey> = Vec::with_capacity(cap);
        let mut ready: Vec<TextureKey> = Vec::with_capacity(cap);
        let mut discards = Discards::default();
        let mut owed = Owed::default();
        let mut done = Maintained::default();
        {
            let mut core = self.shared.lock();
            // Cleared before the registry is read: a wake after this point sets it again.
            self.shared
                .notify
                .retirement_pending
                .store(false, Ordering::SeqCst);
            self.shared.registry.drain_retiring(&mut fresh, limit);
            let more = fresh.len() >= limit && limit > 0;
            let backend = self.shared.mode == Mode::Backend && !core.detached;
            if !fresh.is_empty() {
                // New retirements: any blocked finishing resource may now be examined again.
                core.blocked_streak = 0;
            }
            for key in fresh.drain(..) {
                let _ = self.shared.registry.with_entry_lifecycle(key, |_, state| {
                    if let Some(request) = state.queued.take() {
                        discards.requests.push(request);
                    }
                    if let Some(waiting) = state.waiting.take() {
                        discards.revisions.push(waiting);
                    }
                    state.discard = true;
                    // A proof that arrived before this drain (the backend retiring on its own knowledge, after the
                    // pool's shutdown) is kept, not erased: the key is finished by this drain, not asked about again.
                    state.backend_retired = state.backend_retired || !backend;
                    state.in_finishing = true;
                });
                if backend {
                    owed.backend |= core.backend_retire.is_empty();
                    core.backend_retire.push_back(key);
                }
                // A retiring resource has no queued request and no waiting revision any more: it leaves the queues
                // at once (O(1) each), so they hold only live work.
                core.jobs.remove(key.slot());
                core.file_jobs.remove(key.slot());
                core.uploads.remove(key.slot());
                // A deferred job of a retiring resource is dropped wherever it is in the deferred queue (its request
                // was taken above); file admission reopens if that empties the queue.
                if core.undefer(key) {
                    owed.loaders = true;
                }
                core.finishing.push_back(key);
                done.started += 1;
            }
            let no_backend = self.shared.mode == Mode::Headless || core.detached;
            for _ in 0..core.finishing.len().min(limit) {
                let Some(key) = core.finishing.pop_front() else {
                    break;
                };
                let finishable = self.shared.registry.with_entry_lifecycle(key, |_, state| {
                    let finishable =
                        state.executing.is_none() && (state.backend_retired || no_backend);
                    if finishable {
                        state.in_finishing = false;
                    }
                    finishable
                });
                match finishable {
                    Ok(true) => ready.push(key),
                    Ok(false) => {
                        core.finishing.push_back(key);
                        core.blocked_streak += 1;
                    }
                    Err(_) => {}
                }
            }
            // The step is bounded, so work may be left unexamined: fresh retirements the registry still holds, or
            // finishing resources behind the ones this step looked at. Ask to be called again unless a whole scan
            // has found only blocked resources (then only an event that unblocks one asks again).
            let unexamined = core.blocked_streak < core.finishing.len();
            if limit > 0 && (more || unexamined) {
                owed.retirements = true;
            }
            core.settle(&self.shared.ledger);
        }
        for key in ready {
            if let Ok(state) = self.shared.registry.finish_retirement(key) {
                discards.states.push(state);
                done.finished += 1;
            }
        }
        drop(discards);
        self.fire(owed);
        done
    }

    /// Whether [`close`](Self::close) has run: a read of the closed flag, never waiting on anything but the service's own
    /// short lock. A caller that checks it and then admits can still lose a race with a `close` that lands in between; the
    /// admission then fails with [`AdmitError::Ended`] by itself.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.lock().closed
    }

    /// Ends the run: closes the registry (public ownership ends everywhere), refuses every later admission with
    /// [`AdmitError::Ended`], drops queued requests and revisions waiting for upload (returning their reservations), and
    /// marks executing jobs discard-only. Retirement, [`maintain`](Self::maintain), the backend side and
    /// [`detach_backend`](Self::detach_backend) keep working so teardown can finish. After every resource has retired
    /// and every job and `Arc<PixelRevision>` is gone, all three budgets are back to zero reserved.
    ///
    /// **Its bound.** `close` is the one caller-side method that is not O(1): it costs O(capacity) for the registry
    /// close plus O(Q) for the Q queued requests and waiting revisions. Under the locks it only flips flags and *moves*
    /// those items out; after releasing them it drops them on the caller's thread, so their CPU, job and metadata
    /// reservations have returned when `close` returns, and the caller's thread pays for freeing the dropped buffers. No
    /// wall-clock bound is claimed. Executing jobs' reservations return when their [`complete`](Self::complete) discards
    /// them; revisions the backend holds return when it drops its `Arc`s; resource states return through `maintain`.
    pub fn close(&self) {
        let mut discards = Discards::default();
        {
            let mut core = self.shared.lock();
            if core.closed {
                return;
            }
            core.closed = true;
            core.jobs.clear();
            core.file_jobs.clear();
            core.uploads.clear();
            // Deferred requests are in their resources' `queued` field, taken below with every other.
            core.deferred.clear();
            core.settle(&self.shared.ledger);
            self.shared.registry.for_each_entry(|_, _, state| {
                if let Some(request) = state.queued.take() {
                    discards.requests.push(request);
                }
                if let Some(waiting) = state.waiting.take() {
                    discards.revisions.push(waiting);
                }
            });
        }
        // Alone, after the service lock: the registry's retiring wake runs with no lock of ours held.
        self.shared.registry.close();
        drop(discards);
    }

    /// Ends the retirement of everything, once, **after** the loader pool's shutdown, after the backend is detached
    /// ([`detach_backend`](Self::detach_backend), so retirement waits for no backend) and after every holder of
    /// `TextureRef`s (the scene, its frames, the VMs) is gone. On the caller's thread it drains every retiring entry from
    /// the registry's queue, finishes each one with no executing job and no backend awaited (its state is dropped after
    /// the locks), and leaves the rest in the pending-finish set, in O(capacity) (stated, like [`close`](Self::close);
    /// it is [`maintain`](Self::maintain) with no limit). A key in the pending-finish set finishes inline, at its job's
    /// late [`complete`](Self::complete) or its backend's late [`retired`](Self::retired).
    ///
    /// An entry whose last `TextureRef` is dropped only **after** this call is outside the contract (the composition
    /// must not let that happen): [`ServiceStats::loaders`] reports how many entries were still referenced when it ran,
    /// so the case is visible and not silent.
    #[must_use]
    pub fn finish_teardown(&self) -> Maintained {
        let done = self.maintain(usize::MAX);
        self.shared.counters.referenced_at_teardown.store(
            u64::from(self.shared.registry.stats().live),
            Ordering::SeqCst,
        );
        done
    }
}

#[cfg(test)]
mod deferral_tests;
#[cfg(test)]
mod file_tests;
#[cfg(test)]
mod teardown_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
impl ResourceService {
    /// Runs `gate` inside the critical section of every acceptance (`complete`, `try_commit`, `adopted`), under the
    /// registry lock (tests only).
    pub(crate) fn set_gate(&self, gate: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self
            .shared
            .gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = gate;
    }

    /// The lengths of the job, upload, backend-retirement and finishing queues (tests only).
    pub(crate) fn queue_lengths(&self) -> [usize; 4] {
        let core = self.shared.lock();
        [
            core.jobs.len(),
            core.uploads.len(),
            core.backend_retire.len(),
            core.finishing.len(),
        ]
    }

    /// The queued keys of the job queue and the upload queue, oldest first (tests only).
    pub(crate) fn queued_keys(&self) -> (Vec<TextureKey>, Vec<TextureKey>) {
        let core = self.shared.lock();
        (core.jobs.keys(), core.uploads.keys())
    }

    /// The work the queues have done so far, as `(queued resources examined by take_job and take_uploads, nodes the two
    /// queues have read or written)` (tests only: a deterministic measure, instead of a time).
    pub(crate) fn work_done(&self) -> (usize, usize) {
        let core = self.shared.lock();
        (core.visited, core.jobs.touches() + core.uploads.touches())
    }

    /// Installs the hook called at the points of the file job and loader protocols (tests only).
    pub(crate) fn set_hook(&self, hook: Option<Hook>) {
        *self
            .shared
            .hook
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = hook;
    }

    /// A loader thread is at the top of its loop (tests only).
    pub(crate) fn at_loop_top(&self) {
        self.shared.hook_call(Stage::LoopTop, None);
    }

    /// The deferred queue, front first, as `(key, revision, take sequence, epoch)` (tests only).
    pub(crate) fn deferred(&self) -> Vec<(TextureKey, u64, u64, u64)> {
        let core = self.shared.lock();
        core.deferred
            .iter()
            .map(|entry| (entry.key, entry.revision, entry.seq, entry.epoch))
            .collect()
    }

    /// Whether a retry is in flight, and the deferral flag (tests only).
    pub(crate) fn retry_state(&self) -> (bool, bool) {
        let core = self.shared.lock();
        (
            core.retrying.is_some(),
            self.shared.ledger.capacity.deferral(),
        )
    }

    /// Puts a deferral record in the deferred queue by take sequence (tests only: the state rules, on the state).
    pub(crate) fn plant_deferral(&self, key: TextureKey, revision: u64, seq: u64, epoch: u64) {
        let mut core = self.shared.lock();
        let at = core.deferred.partition_point(|entry| entry.seq < seq);
        core.deferred.insert(
            at,
            Deferred {
                key,
                revision,
                seq,
                epoch,
            },
        );
    }

    /// Marks `key`'s `revision` as the retry in flight (tests only).
    pub(crate) fn plant_retry(&self, key: TextureKey, revision: u64) {
        self.shared.lock().retrying = Some(Retry { key, revision });
    }

    /// A retry's reservation succeeded (tests only: the call a file job makes between its header checks and its
    /// allocation).
    pub(crate) fn retry_reserved(&self, key: TextureKey, revision: u64) {
        self.shared.retry_reserved(key, revision);
    }

    /// Whether the deferred queue's front may be retried now (tests only).
    pub(crate) fn front_eligible_now(&self) -> bool {
        let epoch = self.shared.ledger.capacity.epoch();
        self.shared.lock().front_eligible(epoch)
    }

    /// How many loaders are waiting for work (tests only).
    pub(crate) fn waiting_loaders(&self) -> usize {
        self.shared.waiting.load(Ordering::SeqCst)
    }

    /// Holds the gap between a revision's return of bytes and its in-transit decrement (tests only).
    pub(crate) fn set_transit_hook(&self, hook: Option<Arc<dyn Fn() + Send + Sync>>) {
        self.shared.ledger.capacity.set_transit_pause(hook);
    }

    /// Wakes the loaders for no reason (tests only: a spurious wake).
    pub(crate) fn poke(&self) {
        self.shared.wake_loaders();
    }

    /// The capacity epoch (tests only).
    pub(crate) fn capacity_epoch(&self) -> u64 {
        self.shared.ledger.capacity.epoch()
    }

    /// Sets the requested revision of a resource (tests only: reaching `u64::MAX` takes 2^64 requests otherwise).
    pub(crate) fn force_requested(&self, key: TextureKey, requested: u64) {
        self.shared
            .registry
            .with_entry_value(key, |state| state.requested = requested)
            .expect("a live resource");
    }

    /// The registry (tests only).
    pub(crate) fn registry(&self) -> &TextureRegistry<ResourceState> {
        &self.shared.registry
    }
}
