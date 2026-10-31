//! Backend-neutral adapter between [`ck_resources::ResourceService`] and a revision ledger.

use std::sync::{Arc, Mutex, PoisonError};

use ck_resources::{CommitOutcome, ResourceService, TextureKey, UploadRequest};

const RETIREMENTS_PER_PASS: usize = 256;
const EVENTS_PER_PASS: usize = 1_024;

/// The kind of pass the backend requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentBoundary {
    /// Ordinary texture work.
    Ordinary,
    /// Immediately before a frame is built.
    Frame,
    /// Backend teardown.
    Ending,
}

/// Whether an upload was refused by the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidentRefusal {
    /// Its key is retiring, so dropping the request is expected.
    KeyRetiring,
    /// The ledger violated its reserved-room contract.
    Protocol(String),
}

/// A backend-neutral event delivered from a revision ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    missing_docs,
    reason = "event payload fields are named by the service/ledger contract"
)]
pub enum ResidentEvent {
    /// A prepared image revision is ready for service compare-and-commit.
    Prepared { key: TextureKey, revision: u64 },
    /// The backend failed to upload an image.
    UploadFailed {
        key: TextureKey,
        revision: u64,
        reason: String,
    },
    /// All revisions for a retiring key are released.
    KeyRetired { key: TextureKey },
    /// A backend-wide failure, reported by its presenter.
    GpuFailure { message: String },
}

/// Whether another immediately runnable pass is required.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ResidentPassOutcome {
    /// Runnable retirement or event work remains.
    pub more: bool,
}

/// Operations supplied by a backend's revision ledger.
pub trait LedgerOps {
    /// Available baseline upload slots.
    fn room(&self) -> usize;
    /// Available replacement-revision slots.
    fn extra_room(&self) -> usize;
    /// Offers one service upload without copying its pixels.
    ///
    /// # Errors
    ///
    /// Returns a key-retiring drop or a protocol violation from the ledger.
    fn offer(&mut self, request: UploadRequest) -> Result<(), ResidentRefusal>;
    /// Takes the next event produced by the ledger's owner step.
    fn next_event(&mut self) -> Option<ResidentEvent>;
    /// Whether an event remains after the per-pass event bound.
    fn has_events(&self) -> bool;
    /// Commits a service-approved prepared revision.
    fn commit(&mut self, key: TextureKey, revision: u64);
    /// Rejects a service-rejected prepared revision.
    fn reject(&mut self, key: TextureKey, revision: u64);
    /// Adopts committed revisions in order, bounded by `limit`.
    fn adopt(&mut self, limit: usize, out: &mut Vec<(TextureKey, u64)>);
    /// Starts retirement of every revision for `key`.
    fn retire_key(&mut self, key: TextureKey);
}

/// Aggregate adapter counters.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ResidentStats {
    /// Number of passes run.
    pub passes: u64,
    /// Uploads offered to the ledger.
    pub uploads_offered: u64,
    /// Uploads dropped because their key was retiring.
    pub uploads_dropped_retiring: u64,
    /// Protocol refusals that were surfaced to the service.
    pub protocol_errors: u64,
    /// Prepared revisions committed by the service.
    pub commits: u64,
    /// Prepared revisions rejected by the service.
    pub rejects: u64,
    /// Revisions adopted at frame boundaries.
    pub adoptions: u64,
    /// Key retirements relayed to the ledger.
    pub retirements: u64,
}

/// Moves service work through a backend ledger on the render thread.
pub struct ResidentAdapter {
    service: ResourceService,
    max_adoptions_per_frame: usize,
    stopped: bool,
    deferred_event: Option<ResidentEvent>,
    deferred_retirement: Option<TextureKey>,
    retirements: Vec<TextureKey>,
    uploads: Vec<UploadRequest>,
    adoptions: Vec<(TextureKey, u64)>,
    stats: ResidentStats,
}

impl ResidentAdapter {
    /// Creates an adapter with the ledger's configured per-frame adoption cap.
    #[must_use]
    pub fn new(service: ResourceService, max_adoptions_per_frame: usize) -> Self {
        Self {
            service,
            max_adoptions_per_frame,
            stopped: false,
            deferred_event: None,
            deferred_retirement: None,
            retirements: Vec::new(),
            uploads: Vec::new(),
            adoptions: Vec::new(),
            stats: ResidentStats::default(),
        }
    }

    /// The adapter's aggregate counters.
    #[must_use]
    pub const fn stats(&self) -> ResidentStats {
        self.stats
    }

    /// Runs one non-blocking service/ledger pass.
    pub fn pass(
        &mut self,
        ledger: &mut impl LedgerOps,
        boundary: ResidentBoundary,
    ) -> ResidentPassOutcome {
        self.stats.passes = self.stats.passes.saturating_add(1);
        if boundary == ResidentBoundary::Ending && !self.stopped {
            self.service.detach_backend();
            self.stopped = true;
        }
        if self.stopped {
            self.process_events(ledger, true);
            return ResidentPassOutcome { more: false };
        }

        let mut more = false;
        let mut caused_pump_work = false;
        self.retirements.clear();
        if let Some(key) = self.deferred_retirement.take() {
            self.retirements.push(key);
        }
        let room = RETIREMENTS_PER_PASS.saturating_sub(self.retirements.len());
        self.service.take_retirements(&mut self.retirements, room);
        for &key in &self.retirements {
            ledger.retire_key(key);
            caused_pump_work = true;
        }
        self.stats.retirements = self
            .stats
            .retirements
            .saturating_add(self.retirements.len() as u64);
        if self.retirements.len() == RETIREMENTS_PER_PASS {
            self.retirements.clear();
            if self.service.take_retirements(&mut self.retirements, 1) != 0 {
                self.deferred_retirement = self.retirements.pop();
                more = true;
            }
        }

        caused_pump_work |= self.process_events(ledger, false);
        more |= self.has_events(ledger);

        self.uploads.clear();
        let upload_limit = ledger.room().min(ledger.extra_room());
        self.service.take_uploads(&mut self.uploads, upload_limit);
        for request in self.uploads.drain(..) {
            let key = request.key;
            let revision = request.revision;
            match ledger.offer(request) {
                Ok(()) => {
                    self.stats.uploads_offered = self.stats.uploads_offered.saturating_add(1);
                }
                Err(ResidentRefusal::KeyRetiring) => {
                    self.stats.uploads_dropped_retiring =
                        self.stats.uploads_dropped_retiring.saturating_add(1);
                }
                Err(ResidentRefusal::Protocol(reason)) => {
                    self.service.upload_failed(
                        key,
                        revision,
                        &format!("the backend refused an admitted upload: {reason}"),
                    );
                    self.stats.protocol_errors = self.stats.protocol_errors.saturating_add(1);
                }
            }
        }

        if boundary == ResidentBoundary::Frame {
            self.adoptions.clear();
            ledger.adopt(self.max_adoptions_per_frame, &mut self.adoptions);
            for &(key, revision) in &self.adoptions {
                self.service.adopted(key, revision);
            }
            self.stats.adoptions = self
                .stats
                .adoptions
                .saturating_add(self.adoptions.len() as u64);
        }
        more |= caused_pump_work;
        ResidentPassOutcome { more }
    }

    fn process_events(&mut self, ledger: &mut impl LedgerOps, ending: bool) -> bool {
        let event_limit = if ending { usize::MAX } else { EVENTS_PER_PASS };
        let mut caused_pump_work = false;
        for _ in 0..event_limit {
            let event = self.deferred_event.take().or_else(|| ledger.next_event());
            let Some(event) = event else { break };
            match event {
                ResidentEvent::Prepared { key, revision } => {
                    if ending || self.service.try_commit(key, revision) == CommitOutcome::Rejected {
                        ledger.reject(key, revision);
                        caused_pump_work |= !ending;
                        self.stats.rejects = self.stats.rejects.saturating_add(1);
                    } else {
                        ledger.commit(key, revision);
                        // A commit can supersede the old active revision; the next owner pump must release it.
                        caused_pump_work = true;
                        self.stats.commits = self.stats.commits.saturating_add(1);
                    }
                }
                ResidentEvent::UploadFailed {
                    key,
                    revision,
                    reason,
                } => {
                    self.service.upload_failed(key, revision, &reason);
                }
                ResidentEvent::KeyRetired { key } => self.service.retired(key),
                ResidentEvent::GpuFailure { .. } => {}
            }
        }
        if !ending && self.deferred_event.is_none() {
            self.deferred_event = ledger.next_event();
        }
        caused_pump_work
    }

    fn has_events(&self, ledger: &impl LedgerOps) -> bool {
        self.deferred_event.is_some() || ledger.has_events()
    }
}

#[derive(Default)]
struct RelayState {
    connected: Option<Arc<dyn Fn() + Send + Sync>>,
    pending: bool,
}

/// Late-bound, short-lock wake relay for service work and a backend presenter.
#[derive(Clone, Default)]
pub struct WakeRelay {
    state: Arc<Mutex<RelayState>>,
}

impl WakeRelay {
    /// A service wake that can be installed before the presenter exists.
    #[must_use]
    pub fn wake(&self) -> Arc<dyn Fn() + Send + Sync> {
        let relay = self.clone();
        Arc::new(move || relay.fire())
    }

    /// Connects a backend wake, delivering one sticky wake if work arrived earlier.
    pub fn connect(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        let callback = {
            let mut state = self.lock();
            state.connected = Some(wake);
            if std::mem::replace(&mut state.pending, false) {
                state.connected.as_ref().map(Arc::clone)
            } else {
                None
            }
        };
        if let Some(callback) = callback {
            callback();
        }
    }

    /// Disconnects future wake delivery. A callback already copied by a concurrent wake can still run once.
    pub fn disconnect(&self) {
        let mut state = self.lock();
        state.connected = None;
        state.pending = false;
    }

    fn fire(&self) {
        let callback = {
            let mut state = self.lock();
            if let Some(wake) = &state.connected {
                Some(Arc::clone(wake))
            } else {
                state.pending = true;
                None
            }
        };
        if let Some(callback) = callback {
            callback();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RelayState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
