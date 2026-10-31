//! The budget ledger: counters changed only by compare-and-swap, and the owned reservations that return their amount
//! exactly once (`docs/scene-design.md` §7.2).

use std::fmt;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The three budgets of the resource service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetKind {
    /// Pending jobs (`max_pending_jobs`).
    JobSlots,
    /// CPU pixel bytes (`cpu_pixel_bytes`).
    CpuPixelBytes,
    /// Metadata bytes (`metadata_bytes`).
    MetadataBytes,
}

impl fmt::Display for BudgetKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::JobSlots => "job slots",
            Self::CpuPixelBytes => "CPU pixel bytes",
            Self::MetadataBytes => "metadata bytes",
        })
    }
}

/// One budget's numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetStats {
    /// The most that may be reserved.
    pub limit: u64,
    /// Reserved now.
    pub reserved: u64,
    /// The most that has been reserved at once.
    pub peak: u64,
    /// How many reservations were refused.
    pub refusals: u64,
}

/// Identifies one ledger for the life of the process: reservations are tied to the ledger that granted them, and equal
/// limits or equal counters never make a reservation of another ledger acceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LedgerId(u64);

static NEXT_LEDGER: AtomicU64 = AtomicU64::new(1);

/// Ledger identities are used up; the counter never wraps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerIdsExhausted;

impl fmt::Display for LedgerIdsExhausted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the process has issued every ledger identity")
    }
}

impl std::error::Error for LedgerIdsExhausted {}

impl LedgerId {
    pub(crate) fn allocate() -> Result<Self, LedgerIdsExhausted> {
        Self::allocate_from(&NEXT_LEDGER)
    }

    /// `allocate` over a counter the caller owns (a unit test starts one at `u64::MAX`).
    pub(crate) fn allocate_from(counter: &AtomicU64) -> Result<Self, LedgerIdsExhausted> {
        counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |next| {
                next.checked_add(1)
            })
            .map(Self)
            .map_err(|_| LedgerIdsExhausted)
    }

    /// The identity as a number.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

struct Budget {
    limit: u64,
    reserved: AtomicU64,
    peak: AtomicU64,
    refusals: AtomicU64,
}

impl Budget {
    fn new(limit: u64) -> Self {
        Self {
            limit,
            reserved: AtomicU64::new(0),
            peak: AtomicU64::new(0),
            refusals: AtomicU64::new(0),
        }
    }

    /// Reserves `amount` by compare-and-swap; on refusal nothing changes but the refusal count, and the amount still
    /// available is returned.
    fn try_reserve(&self, amount: u64) -> Result<(), u64> {
        let mut current = self.reserved.load(Ordering::SeqCst);
        loop {
            let available = self.limit.saturating_sub(current);
            let Some(next) = current
                .checked_add(amount)
                .filter(|next| *next <= self.limit)
            else {
                self.refusals.fetch_add(1, Ordering::SeqCst);
                return Err(available);
            };
            match self.reserved.compare_exchange_weak(
                current,
                next,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    self.peak.fetch_max(next, Ordering::SeqCst);
                    return Ok(());
                }
                Err(seen) => current = seen,
            }
        }
    }

    fn stats(&self) -> BudgetStats {
        BudgetStats {
            limit: self.limit,
            reserved: self.reserved.load(Ordering::SeqCst),
            peak: self.peak.load(Ordering::SeqCst),
            refusals: self.refusals.load(Ordering::SeqCst),
        }
    }
}

/// The CPU budget's capacity-change signal (`docs/scene-design.md` §7.3): what lets a file job whose reservation was
/// refused wait for bytes that return without its help, and be told when they do, with no timer and no lost wake.
///
/// - `epoch` counts the capacity changes: the returns of CPU bytes, and the end of a revision's time in transit. It is
///   incremented **after** the change is visible (the bytes are back in the budget; the in-transit count is down).
/// - `in_transit` counts the bytes of live [`PixelRevision`](crate::PixelRevision)s: finished pixels that wait for the
///   backend, are held by it, or are about to be dropped. They return without any loader's help. It is decremented
///   after the revision's bytes have returned, so while it is non-zero some bytes may still be on their way, **and the
///   decrement is itself a capacity change** (it advances the epoch and wakes, like a return): a loader that judged a
///   refusal deferrable because bytes were still counted in transit, when they had in fact just returned, is told when
///   the count catches up, and settles (the capacity error, if nothing is in transit and nothing returned since).
/// - `deferral` is set while a file job may be waiting for a return. A returner that sees it set calls the notifier
///   (which takes the service lock briefly and wakes the loaders); one that does not skips the lock.
///
/// The orderings are all sequentially consistent. A deferring loader (1) reads the epoch before its reservation
/// attempt, (2) sets the flag after the attempt was refused, (3) sets it again under the service lock as it records the
/// deferral and (4) under that lock compares the epoch. A returner (a) returns the bytes, (b) increments the epoch,
/// (c) reads the flag. If (b) is before (1)'s read the attempt saw the bytes; if (b) is before (4)'s read the
/// comparison sees a changed epoch; otherwise (c) is after (3), which is under the lock the notifier also takes, so
/// the returner sees the flag and wakes the loaders. The flag is cleared only under the service lock, only when no
/// deferral is recorded, and (3) sets it again, so a clear between (2) and (3) loses nothing.
pub(crate) struct Capacity {
    epoch: AtomicU64,
    in_transit: AtomicU64,
    deferral: AtomicBool,
    notifier: OnceLock<Box<dyn Fn() + Send + Sync>>,
    /// Called at the start of a revision's last drop step, after its bytes returned and before the in-transit count
    /// goes down (tests only: holds the gap).
    #[cfg(test)]
    transit_pause: std::sync::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Capacity {
    fn new() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            in_transit: AtomicU64::new(0),
            deferral: AtomicBool::new(false),
            notifier: OnceLock::new(),
            #[cfg(test)]
            transit_pause: std::sync::Mutex::new(None),
        }
    }

    /// Installs (or removes) the hook that holds the gap between a revision's return of bytes and its decrement of
    /// the in-transit count (tests only).
    #[cfg(test)]
    pub(crate) fn set_transit_pause(&self, hook: Option<Arc<dyn Fn() + Send + Sync>>) {
        *self
            .transit_pause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = hook;
    }

    /// How many returns of CPU bytes there have been.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// The bytes of live revisions.
    pub(crate) fn in_transit(&self) -> u64 {
        self.in_transit.load(Ordering::SeqCst)
    }

    /// Step (2) and (3): a file job may be waiting for a return.
    pub(crate) fn raise_deferral(&self) {
        self.deferral.store(true, Ordering::SeqCst);
    }

    /// No file job is recorded as waiting (only under the service lock).
    pub(crate) fn clear_deferral(&self) {
        self.deferral.store(false, Ordering::SeqCst);
    }

    /// Whether the flag is set.
    #[cfg(test)]
    pub(crate) fn deferral(&self) -> bool {
        self.deferral.load(Ordering::SeqCst)
    }

    /// Installs what a returner calls when it sees the flag. Once.
    pub(crate) fn set_notifier(&self, notifier: Box<dyn Fn() + Send + Sync>) {
        let _ = self.notifier.set(notifier);
    }

    /// Steps (b) and (c) of a return of CPU bytes (the bytes are already back).
    fn returned(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        if self.deferral.load(Ordering::SeqCst)
            && let Some(notifier) = self.notifier.get()
        {
            notifier();
        }
    }
}

/// The bytes of one live revision, counted in [`Capacity::in_transit`] until it is dropped.
pub(crate) struct Transit {
    ledger: Arc<Ledger>,
    bytes: u64,
}

impl Drop for Transit {
    fn drop(&mut self) {
        #[cfg(test)]
        {
            let hook = self
                .ledger
                .capacity
                .transit_pause
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(hook) = hook {
                hook();
            }
        }
        self.ledger
            .capacity
            .in_transit
            .fetch_sub(self.bytes, Ordering::SeqCst);
        // The end of a revision's time in transit is a capacity change of its own (see `Capacity`): the bytes
        // returned before it, so no later event would otherwise tell a loader that counted them as still coming.
        self.ledger.capacity.returned();
    }
}

/// A refused reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shortfall {
    pub(crate) requested: u64,
    pub(crate) available: u64,
}

/// The three budgets of one service, shared by its clones.
pub(crate) struct Ledger {
    id: LedgerId,
    jobs: Budget,
    cpu: Budget,
    metadata: Budget,
    /// The CPU budget's capacity-change signal.
    pub(crate) capacity: Capacity,
}

impl Ledger {
    pub(crate) fn new(
        id: Result<LedgerId, LedgerIdsExhausted>,
        jobs: u64,
        cpu: u64,
        metadata: u64,
    ) -> Result<Arc<Self>, LedgerIdsExhausted> {
        Ok(Arc::new(Self {
            id: id?,
            jobs: Budget::new(jobs),
            cpu: Budget::new(cpu),
            metadata: Budget::new(metadata),
            capacity: Capacity::new(),
        }))
    }

    pub(crate) fn id(&self) -> LedgerId {
        self.id
    }

    fn budget(&self, kind: BudgetKind) -> &Budget {
        match kind {
            BudgetKind::JobSlots => &self.jobs,
            BudgetKind::CpuPixelBytes => &self.cpu,
            BudgetKind::MetadataBytes => &self.metadata,
        }
    }

    /// Reserves `amount` of `kind`: an owned [`Reservation`] that returns it when dropped, or a refusal that changed
    /// nothing.
    pub(crate) fn reserve(
        self: &Arc<Self>,
        kind: BudgetKind,
        amount: u64,
    ) -> Result<Reservation, Shortfall> {
        self.budget(kind)
            .try_reserve(amount)
            .map_err(|available| Shortfall {
                requested: amount,
                available,
            })?;
        Ok(Reservation {
            ledger: Arc::clone(self),
            kind,
            amount,
        })
    }

    pub(crate) fn stats(&self, kind: BudgetKind) -> BudgetStats {
        self.budget(kind).stats()
    }
}

/// An amount reserved from one budget of one ledger. It is an owned value: it moves with the work it pays for and is
/// never copied, and dropping it returns its amount exactly once.
pub struct Reservation {
    ledger: Arc<Ledger>,
    kind: BudgetKind,
    amount: u64,
}

impl Reservation {
    /// Which budget it was taken from.
    #[must_use]
    pub fn kind(&self) -> BudgetKind {
        self.kind
    }

    /// How much it holds.
    #[must_use]
    pub fn amount(&self) -> u64 {
        self.amount
    }

    /// The ledger that granted it.
    #[must_use]
    pub fn ledger_id(&self) -> LedgerId {
        self.ledger.id
    }

    /// Whether `ledger` granted it.
    pub(crate) fn belongs_to(&self, ledger: &Ledger) -> bool {
        self.ledger.id == ledger.id
    }

    /// Counts this reservation's bytes as in transit until the returned value is dropped (declare it **after** the
    /// reservation in a struct, so it drops after the bytes are back).
    pub(crate) fn in_transit(&self) -> Transit {
        self.ledger
            .capacity
            .in_transit
            .fetch_add(self.amount, Ordering::SeqCst);
        Transit {
            ledger: Arc::clone(&self.ledger),
            bytes: self.amount,
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.ledger
            .budget(self.kind)
            .reserved
            .fetch_sub(self.amount, Ordering::SeqCst);
        if self.kind == BudgetKind::CpuPixelBytes {
            // After the bytes are back: the capacity-change event. Never under a lock of this crate (the notifier
            // may take the service lock).
            #[cfg(test)]
            crate::lockdebug::dropped_value();
            self.ledger.capacity.returned();
        }
    }
}

impl fmt::Debug for Reservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Reservation")
            .field("kind", &self.kind)
            .field("amount", &self.amount)
            .field("ledger", &self.ledger.id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger(jobs: u64, cpu: u64, metadata: u64) -> Arc<Ledger> {
        Ledger::new(LedgerId::allocate(), jobs, cpu, metadata).expect("a ledger")
    }

    #[test]
    fn a_reservation_is_returned_exactly_once_per_budget() {
        let ledger = ledger(2, 100, 1000);
        for (kind, amount) in [
            (BudgetKind::JobSlots, 2),
            (BudgetKind::CpuPixelBytes, 100),
            (BudgetKind::MetadataBytes, 640),
        ] {
            let reservation = ledger.reserve(kind, amount).expect("fits");
            assert_eq!((reservation.kind(), reservation.amount()), (kind, amount));
            assert_eq!(ledger.stats(kind).reserved, amount);
            drop(reservation);
            let stats = ledger.stats(kind);
            assert_eq!((stats.reserved, stats.peak), (0, amount), "{kind}");
        }
    }

    #[test]
    fn a_refused_reservation_changes_nothing_and_counts_a_refusal() {
        let ledger = ledger(2, 100, 1000);
        let held = ledger.reserve(BudgetKind::CpuPixelBytes, 60).expect("fits");
        let refused = ledger
            .reserve(BudgetKind::CpuPixelBytes, 41)
            .expect_err("over the limit");
        assert_eq!(
            refused,
            Shortfall {
                requested: 41,
                available: 40
            }
        );
        let stats = ledger.stats(BudgetKind::CpuPixelBytes);
        assert_eq!((stats.reserved, stats.peak, stats.refusals), (60, 60, 1));
        // Exactly the limit is allowed; the amount that overflows u64 is refused, not wrapped.
        let exact = ledger
            .reserve(BudgetKind::CpuPixelBytes, 40)
            .expect("exactly fits");
        assert!(ledger.reserve(BudgetKind::CpuPixelBytes, u64::MAX).is_err());
        assert_eq!(ledger.stats(BudgetKind::CpuPixelBytes).refusals, 2);
        drop((held, exact));
        assert_eq!(ledger.stats(BudgetKind::CpuPixelBytes).reserved, 0);
        assert_eq!(ledger.stats(BudgetKind::CpuPixelBytes).peak, 100);
    }

    #[test]
    fn a_reservation_knows_its_ledger_and_equal_ledgers_are_still_foreign() {
        let (a, b) = (ledger(1, 10, 10), ledger(1, 10, 10));
        assert_ne!(a.id(), b.id());
        let from_a = a.reserve(BudgetKind::JobSlots, 1).expect("fits");
        assert!(from_a.belongs_to(&a));
        assert!(!from_a.belongs_to(&b), "equal limits do not make it b's");
        assert_eq!(from_a.ledger_id(), a.id());
    }

    #[test]
    fn a_reservation_outlives_its_service_and_returns_to_its_own_ledger() {
        let ledger = ledger(1, 10, 10);
        let reservation = ledger.reserve(BudgetKind::MetadataBytes, 5).expect("fits");
        let weak = Arc::downgrade(&ledger);
        drop(ledger);
        assert!(
            weak.upgrade().is_some(),
            "the reservation keeps the ledger alive"
        );
        drop(reservation);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn ledger_identities_do_not_wrap() {
        let counter = AtomicU64::new(u64::MAX - 1);
        assert_eq!(
            LedgerId::allocate_from(&counter),
            Ok(LedgerId(u64::MAX - 1))
        );
        assert_eq!(LedgerId::allocate_from(&counter), Err(LedgerIdsExhausted));
        assert_eq!(LedgerId::allocate_from(&counter), Err(LedgerIdsExhausted));
        assert_eq!(counter.load(Ordering::SeqCst), u64::MAX);
        let first = LedgerId::allocate().expect("an identity");
        assert!(first.get() >= 1);
    }

    #[test]
    fn concurrent_reservations_never_exceed_the_limit() {
        let ledger = ledger(8, 0, 0);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..2000 {
                        if let Ok(reservation) = ledger.reserve(BudgetKind::JobSlots, 1) {
                            assert!(ledger.stats(BudgetKind::JobSlots).reserved <= 8);
                            drop(reservation);
                        }
                    }
                });
            }
        });
        let stats = ledger.stats(BudgetKind::JobSlots);
        assert_eq!((stats.reserved, stats.limit), (0, 8));
        assert!(stats.peak <= 8);
    }
}
