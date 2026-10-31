//! Bounded measurement windows for the window loop (milestone 5, issue 10b-1; design `docs/scene-design.md` §11.3).
//!
//! The scale acceptance measures one **common** window on both owners of a metric: the presenter's render thread (frame intervals,
//! GPU time, presentation) and this loop (updates delivered and missed). The requester defines the window once, as `[start, end)` on
//! one clock (`CLOCK_UPTIME_RAW` nanoseconds), and sends the same definition to both; each owner admits a sample only by **its own
//! timestamp** inside the window, so whatever order and timing the two adoptions have, both cover exactly the same interval.
//!
//! `ck-window` knows nothing of the GPU and `ck-gpu-metal` nothing of the window loop, so [`Windows`] is a deliberate small copy of
//! `ck-gpu-metal`'s: the same admission rules, the same refusals, tested the same way (the application checks the two agree). This
//! crate contains no `unsafe` code and no platform code, so the clock is given: [`LoopMetrics::new`] takes a function returning
//! `CLOCK_UPTIME_RAW` nanoseconds.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use crate::notifier::Notifier;

/// A measurement window as the requester defines it, once, for both owners: `[start, end)` on the instrumentation's clock. `end`
/// is open until [`Windows::end`] sets it (the application's run end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricsWindow {
    /// A per-run counter, never reused.
    pub id: u32,
    /// The first nanosecond in the window.
    pub start: u64,
    /// The first nanosecond after the window, `None` while open.
    pub end: Option<u64>,
}

impl MetricsWindow {
    /// Whether `ns` lies in `[start, end)`.
    #[must_use]
    pub fn contains(&self, ns: u64) -> bool {
        ns >= self.start && self.end.is_none_or(|end| ns < end)
    }
}

/// Why a window request was refused (nothing changed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    /// The request starts before the current window's end, or the current window has no end yet.
    Overlaps,
    /// An earlier request has not been adopted yet: the owner keeps one pending request, and a second would erase it.
    Pending,
    /// The id is not greater than every earlier id (ids are never reused), or the counter ran out.
    IdNotNew,
    /// The window's end is not after its start.
    EmptyWindow,
}

/// What `end` could not do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndError {
    /// No window with that id is the current one.
    UnknownWindow,
    /// The window already has an end.
    AlreadyEnded,
    /// The end is not after the window's start.
    BeforeStart,
}

/// How many timestamps of a range [`Windows::admit_range`] put in each window the owner keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeAdmission {
    /// In the current window.
    pub current: u64,
    /// In the last ended window.
    pub last_ended: u64,
}

/// How many of `first + k * step`, `k` in `0..count`, lie in `[lo, hi)` (`hi` open when `None`), and the latest of them. Exact
/// whatever `count` is: integer arithmetic on the range, in `u128`.
fn count_in(first: u64, step: u64, count: u64, lo: u64, hi: Option<u64>) -> (u64, u64) {
    if count == 0 {
        return (0, 0);
    }
    let (first, step, count) = (
        u128::from(first),
        u128::from(step.max(1)),
        u128::from(count),
    );
    let k_from = u128::from(lo).saturating_sub(first).div_ceil(step);
    let k_to = hi
        .map_or(count, |hi| {
            u128::from(hi).saturating_sub(first).div_ceil(step)
        })
        .min(count);
    if k_to <= k_from {
        return (0, 0);
    }
    let latest = first + (k_to - 1) * step;
    (
        u64::try_from(k_to - k_from).unwrap_or(u64::MAX),
        u64::try_from(latest).unwrap_or(u64::MAX),
    )
}

/// One window with the statistics an owner gathered for it.
#[derive(Debug, Clone)]
pub struct Tracked<S> {
    /// The window.
    pub window: MetricsWindow,
    /// When the owner adopted it (its own boundary), `None` until then.
    pub adopted_at: Option<u64>,
    /// Adopted after `start`: samples between `start` and the adoption could not be admitted, so the window is **not** a pass.
    pub late_adopted: bool,
    /// Samples (by their own timestamp) that fell outside every window the owner knew.
    pub outside: u64,
    /// Results that belonged to a window older than the last ended one, which the owner no longer keeps.
    pub dropped_late: u64,
    /// The latest timestamp admitted to the window (zero: none yet).
    pub last_admitted_ns: u64,
    /// The end was set when a sample at or after it had already been admitted: the window holds work past its end. Never expected
    /// (the end is set while the owner admits nothing); it is a flag so that it cannot pass unnoticed.
    pub end_overrun: bool,
    /// The owner's statistics for this window.
    pub stats: S,
}

impl<S: Default> Tracked<S> {
    fn new(window: MetricsWindow) -> Self {
        Self {
            window,
            adopted_at: None,
            late_adopted: false,
            outside: 0,
            dropped_late: 0,
            last_admitted_ns: 0,
            end_overrun: false,
            stats: S::default(),
        }
    }
}

/// An owner's windows: the pending request, the current window and the last ended one (two kept), with admission by the sample's own
/// timestamp.
#[derive(Debug)]
pub struct Windows<S> {
    pending: Option<MetricsWindow>,
    current: Option<Tracked<S>>,
    last_ended: Option<Tracked<S>>,
    last_id: Option<u32>,
    /// Results older than anything kept (counted when no window exists to hold the count).
    dropped_late: u64,
    /// Samples outside every window when none is current.
    outside: u64,
}

impl<S: Default> Default for Windows<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Default> Windows<S> {
    /// Windows with nothing requested.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: None,
            current: None,
            last_ended: None,
            last_id: None,
            dropped_late: 0,
            outside: 0,
        }
    }

    /// Takes a request. It is adopted at the owner's next [`boundary`](Self::boundary).
    ///
    /// # Errors
    ///
    /// A request that overlaps the current window (its start is before the current window's end, or the current has no end), whose
    /// id is not new, or whose end is not after its start, is refused and changes nothing.
    pub fn request(&mut self, window: MetricsWindow) -> Result<(), RequestError> {
        if window.end.is_some_and(|end| end <= window.start) {
            return Err(RequestError::EmptyWindow);
        }
        if self.last_id.is_some_and(|last| window.id <= last) || self.last_id == Some(u32::MAX) {
            return Err(RequestError::IdNotNew);
        }
        // One request waits at a time: a second would silently erase the first, which was acknowledged. Refused, nothing changes.
        if self.pending.is_some() {
            return Err(RequestError::Pending);
        }
        let blocked = |tracked: &Tracked<S>| match tracked.window.end {
            None => true,
            Some(end) => window.start < end,
        };
        if self.current.as_ref().is_some_and(blocked) {
            return Err(RequestError::Overlaps);
        }
        self.pending = Some(window);
        self.last_id = Some(window.id);
        Ok(())
    }

    /// The owner's boundary at `now`: a pending request is adopted (its adoption time is `now`, and it is late if `now` is after its
    /// `start`), and the current window is retired to `last_ended` once `now` has passed its end.
    pub fn boundary(&mut self, now: u64) {
        if let Some(current) = &self.current
            && current.window.end.is_some_and(|end| now >= end)
            && self.pending.is_some()
        {
            self.last_ended = self.current.take();
        }
        if self.current.is_none()
            && let Some(window) = self.pending.take()
        {
            let mut tracked = Tracked::new(window);
            tracked.adopted_at = Some(now);
            tracked.late_adopted = now > window.start;
            self.current = Some(tracked);
        }
    }

    /// Sets the end of the current (or pending) window `id`.
    ///
    /// # Errors
    ///
    /// The window is unknown, already ended, or `end` is not after its start.
    pub fn end(&mut self, id: u32, end: u64) -> Result<(), EndError> {
        if let Some(pending) = self.pending.as_mut().filter(|window| window.id == id) {
            if pending.end.is_some() {
                return Err(EndError::AlreadyEnded);
            }
            if end <= pending.start {
                return Err(EndError::BeforeStart);
            }
            pending.end = Some(end);
            return Ok(());
        }
        let Some(tracked) = self
            .current
            .as_mut()
            .filter(|tracked| tracked.window.id == id)
        else {
            return Err(EndError::UnknownWindow);
        };
        if tracked.window.end.is_some() {
            return Err(EndError::AlreadyEnded);
        }
        if end <= tracked.window.start {
            return Err(EndError::BeforeStart);
        }
        tracked.window.end = Some(end);
        // A sample at or after the end already admitted means the end reached this owner after work past it: flagged.
        tracked.end_overrun = tracked.last_admitted_ns >= end;
        Ok(())
    }

    /// The window a sample timestamped `ns` belongs to, with its statistics: the current window, else the last ended one (a late
    /// result is attributed to its own window), else `None` after counting it as outside (inside the span the owner knows) or dropped
    /// late (before the last ended window began).
    pub fn admit(&mut self, ns: u64) -> Option<&mut Tracked<S>> {
        if self
            .current
            .as_ref()
            .is_some_and(|tracked| tracked.window.contains(ns))
        {
            let tracked = self.current.as_mut()?;
            tracked.last_admitted_ns = tracked.last_admitted_ns.max(ns);
            return Some(tracked);
        }
        if self
            .last_ended
            .as_ref()
            .is_some_and(|tracked| tracked.window.contains(ns))
        {
            let tracked = self.last_ended.as_mut()?;
            tracked.last_admitted_ns = tracked.last_admitted_ns.max(ns);
            return Some(tracked);
        }
        let known_older = self
            .last_ended
            .as_ref()
            .is_some_and(|tracked| ns < tracked.window.start);
        if known_older {
            self.dropped_late = self.dropped_late.saturating_add(1);
            if let Some(tracked) = self.current.as_mut() {
                tracked.dropped_late = tracked.dropped_late.saturating_add(1);
            }
        } else if let Some(tracked) = self.current.as_mut() {
            tracked.outside = tracked.outside.saturating_add(1);
        } else {
            self.outside = self.outside.saturating_add(1);
        }
        None
    }

    /// The window that contains `ns` among the ones the owner keeps (the current one, else the last ended one), without counting
    /// anything as outside. For a result whose own timestamps, and not a frame's commit, decide its window.
    pub fn window_at(&mut self, ns: u64) -> Option<&mut Tracked<S>> {
        if self
            .current
            .as_ref()
            .is_some_and(|tracked| tracked.window.contains(ns))
        {
            return self.current.as_mut();
        }
        self.last_ended
            .as_mut()
            .filter(|tracked| tracked.window.contains(ns))
    }

    /// The current window, mutably.
    pub fn current_mut(&mut self) -> Option<&mut Tracked<S>> {
        self.current.as_mut()
    }

    /// The last ended window, mutably.
    pub fn last_ended_mut(&mut self) -> Option<&mut Tracked<S>> {
        self.last_ended.as_mut()
    }

    /// Admits the `count` timestamps `first`, `first + step`, `first + 2 * step`, ... by their own values, in bounded work whatever
    /// `count` is (arithmetic on the ranges, not a loop over the samples): how many fall in the current window and in the last ended
    /// one is returned for the caller to add to their statistics, and the rest are counted exactly as [`admit`](Self::admit) counts
    /// a single sample, outside or dropped late.
    pub fn admit_range(&mut self, first: u64, step: u64, count: u64) -> RangeAdmission {
        let bounds = |tracked: &Tracked<S>| (tracked.window.start, tracked.window.end);
        let in_current = self
            .current
            .as_ref()
            .map(bounds)
            .map_or((0, 0), |(lo, hi)| count_in(first, step, count, lo, hi));
        let in_last = self
            .last_ended
            .as_ref()
            .map(bounds)
            .map_or((0, 0), |(lo, hi)| count_in(first, step, count, lo, hi));
        let older = self.last_ended.as_ref().map_or(0, |tracked| {
            count_in(first, step, count, 0, Some(tracked.window.start)).0
        });
        let outside = count
            .saturating_sub(in_current.0)
            .saturating_sub(in_last.0)
            .saturating_sub(older);
        if let Some(tracked) = self.current.as_mut() {
            if in_current.0 != 0 {
                tracked.last_admitted_ns = tracked.last_admitted_ns.max(in_current.1);
            }
            tracked.dropped_late = tracked.dropped_late.saturating_add(older);
            tracked.outside = tracked.outside.saturating_add(outside);
        } else {
            self.outside = self.outside.saturating_add(outside);
        }
        if let Some(tracked) = self.last_ended.as_mut()
            && in_last.0 != 0
        {
            tracked.last_admitted_ns = tracked.last_admitted_ns.max(in_last.1);
        }
        self.dropped_late = self.dropped_late.saturating_add(older);
        RangeAdmission {
            current: in_current.0,
            last_ended: in_last.0,
        }
    }

    /// A request is waiting for the owner's next boundary.
    #[must_use]
    pub const fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// The current window.
    #[must_use]
    pub fn current(&self) -> Option<&Tracked<S>> {
        self.current.as_ref()
    }

    /// The last ended window.
    #[must_use]
    pub fn last_ended(&self) -> Option<&Tracked<S>> {
        self.last_ended.as_ref()
    }

    /// The window `id` if the owner still keeps it (the current or the last ended one).
    #[must_use]
    pub fn get(&self, id: u32) -> Option<&Tracked<S>> {
        self.current
            .iter()
            .chain(self.last_ended.iter())
            .find(|tracked| tracked.window.id == id)
    }

    /// The adoption time of window `id`, once adopted.
    #[must_use]
    pub fn adopted(&self, id: u32) -> Option<u64> {
        self.get(id).and_then(|tracked| tracked.adopted_at)
    }

    /// Results counted as dropped late with no window to hold them.
    #[must_use]
    pub fn dropped_late(&self) -> u64 {
        self.dropped_late
    }

    /// Samples counted as outside while no window was current.
    #[must_use]
    pub fn outside_without_window(&self) -> u64 {
        self.outside
    }
}

// ---------------------------------------------------------------------------------------------
// The loop's metrics
// ---------------------------------------------------------------------------------------------

/// What the window loop gathers for one window: updates delivered (by the time they were posted) and opportunities missed (by the
/// time they fell due).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoopWindowStats {
    /// Updates posted to the UI in the window.
    pub delivered: u64,
    /// Opportunities that fell due in the window while an update was outstanding and were coalesced into a later one.
    pub missed: u64,
}

impl LoopWindowStats {
    /// Whether a counter reached its maximum: the figures are then lower bounds.
    #[must_use]
    pub const fn saturated(&self) -> bool {
        self.delivered == u64::MAX || self.missed == u64::MAX
    }
}

/// The function that returns the instrumentation's clock, `CLOCK_UPTIME_RAW` nanoseconds.
pub type UptimeNs = Arc<dyn Fn() -> u64 + Send + Sync>;

struct LoopMetricsState {
    windows: Windows<LoopWindowStats>,
    /// Every delivered update and missed opportunity of the run, whatever the windows.
    totals: LoopWindowStats,
    /// The loop's progress fence: it has examined the scheduler (captured every opportunity due, counted the ones it coalesced)
    /// through this instant, in window nanoseconds. Zero until the first pass.
    accounted_through: u64,
    /// An `Instant` of the loop's clock and the instrumentation's clock read together, the first time the loop adopted: the pair that
    /// turns the scheduler's `Instant`s into window nanoseconds.
    base: Option<(Instant, u64)>,
    notifier: Weak<Notifier>,
}

struct LoopMetricsInner {
    state: Mutex<LoopMetricsState>,
    changed: Condvar,
    clock_ns: UptimeNs,
}

/// A thread-safe handle on the window loop's measurement windows: the requester (any thread) asks for a window, the loop adopts it
/// at the start of its next pass and counts delivered updates and missed opportunities into it by their own timestamps.
#[derive(Clone)]
pub struct LoopMetrics(Arc<LoopMetricsInner>);

impl std::fmt::Debug for LoopMetrics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LoopMetrics")
    }
}

/// The loop's admission lock held for setting the end of a window ([`LoopMetrics::lock_end`]).
pub struct LoopEndGuard<'a> {
    state: MutexGuard<'a, LoopMetricsState>,
    changed: &'a Condvar,
}

impl LoopEndGuard<'_> {
    /// Sets the end of window `id`, once, while nothing is being admitted.
    ///
    /// # Errors
    ///
    /// The window is unknown, already ended, or `end` is not after its start.
    pub fn end(&mut self, id: u32, end: u64) -> Result<(), EndError> {
        self.state.windows.end(id, end)
    }
}

impl Drop for LoopEndGuard<'_> {
    fn drop(&mut self) {
        self.changed.notify_all();
    }
}

/// A window of the loop as read: its statistics and whether the loop had passed its end.
#[derive(Debug, Clone)]
pub struct LoopReading {
    /// The window with the loop's statistics for it.
    pub window: Tracked<LoopWindowStats>,
    /// The loop had passed the window's end on the clock: the reading is final.
    pub complete: bool,
}

impl LoopMetrics {
    /// Metrics over the instrumentation's clock `clock_ns`.
    #[must_use]
    pub fn new(clock_ns: UptimeNs) -> Self {
        Self(Arc::new(LoopMetricsInner {
            state: Mutex::new(LoopMetricsState {
                windows: Windows::new(),
                totals: LoopWindowStats::default(),
                accounted_through: 0,
                base: None,
                notifier: Weak::new(),
            }),
            changed: Condvar::new(),
            clock_ns,
        }))
    }

    fn lock(&self) -> MutexGuard<'_, LoopMetricsState> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn bind(&self, notifier: &Arc<Notifier>) {
        self.lock().notifier = Arc::downgrade(notifier);
    }

    /// Requests the common window; the loop adopts it at the start of its next pass (this wakes it).
    ///
    /// # Errors
    ///
    /// A request that overlaps the current window, whose id is not new or whose end is not after its start is refused and changes
    /// nothing.
    pub fn request(&self, window: MetricsWindow) -> Result<(), RequestError> {
        let notifier = {
            let mut state = self.lock();
            state.windows.request(window)?;
            state.notifier.upgrade()
        };
        if let Some(notifier) = notifier {
            notifier.wake();
        }
        Ok(())
    }

    /// Sets the end of window `id`, once.
    ///
    /// # Errors
    ///
    /// The window is unknown, already ended, or `end` is not after its start.
    pub fn end(&self, id: u32, end: u64) -> Result<(), EndError> {
        self.lock().windows.end(id, end)?;
        self.0.changed.notify_all();
        Ok(())
    }

    /// Holds the loop's admission lock, so that an end can be set while no update or opportunity is being admitted: with the other
    /// owner's lock also held, one timestamp read under both is, for both, the instant before which everything admitted lies and at
    /// or after which nothing was. Hold it briefly; the loop's passes wait for it.
    #[must_use]
    pub fn lock_end(&self) -> LoopEndGuard<'_> {
        LoopEndGuard {
            state: self.lock(),
            changed: &self.0.changed,
        }
    }

    /// When the loop adopted window `id`, once it has.
    #[must_use]
    pub fn adopted(&self, id: u32) -> Option<u64> {
        self.lock().windows.adopted(id)
    }

    /// Every update delivered and opportunity missed over the loop's life, whatever the windows.
    #[must_use]
    pub fn totals(&self) -> LoopWindowStats {
        self.lock().totals
    }

    /// A copy of window `id` as it stands, without waiting.
    #[must_use]
    pub fn snapshot(&self, id: u32) -> Option<LoopReading> {
        let state = self.lock();
        state.windows.get(id).map(|window| LoopReading {
            window: window.clone(),
            complete: false,
        })
    }

    /// Window `id` once the loop has passed its end on the clock **and has accounted for everything due through it** (the loop's own
    /// progress fence, [`accounted`](Self::accounted)), waiting at most `within` for that.
    #[must_use]
    pub fn read(&self, id: u32, within: Duration) -> Option<LoopReading> {
        let deadline = Instant::now() + within;
        let mut state = self.lock();
        loop {
            let end = state.windows.get(id)?.window.end;
            let passed =
                end.is_some_and(|end| (self.0.clock_ns)() >= end && state.accounted_through >= end);
            let now = Instant::now();
            if passed || now >= deadline {
                let window = state.windows.get(id)?.clone();
                return Some(LoopReading {
                    window,
                    complete: passed,
                });
            }
            let slice = (deadline - now).min(Duration::from_millis(5));
            state = self
                .0
                .changed
                .wait_timeout(state, slice)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The loop's boundary at `now` (its own clock): the instant maps to window nanoseconds, a pending request is adopted. The loop
    /// calls it at the start of a pass; it is public (and hidden) only so a test can place an adoption deterministically.
    #[doc(hidden)]
    pub fn boundary(&self, now: Instant) {
        let mut state = self.lock();
        let ns = Self::to_ns(&mut state, &self.0.clock_ns, now);
        state.windows.boundary(ns);
        drop(state);
        self.0.changed.notify_all();
    }

    /// An update was posted at `at`. The loop calls it when it posts one; it is public (and hidden) only so a test can place it
    /// deterministically.
    #[doc(hidden)]
    pub fn delivered(&self, at: Instant) {
        let mut state = self.lock();
        let ns = Self::to_ns(&mut state, &self.0.clock_ns, at);
        state.totals.delivered = state.totals.delivered.saturating_add(1);
        if let Some(tracked) = state.windows.admit(ns) {
            tracked.stats.delivered = tracked.stats.delivered.saturating_add(1);
        }
    }

    /// The loop has examined the update scheduler through `now`: every opportunity due by then was captured and the coalesced ones
    /// counted. A window ending at or before `now` is final once this has been called (and the clock has passed the end). The loop
    /// calls it after each scheduler refresh and once more when it begins its teardown; it is public (and hidden) only so a test can
    /// place it deterministically.
    #[doc(hidden)]
    pub fn accounted(&self, now: Instant) {
        let mut state = self.lock();
        let ns = Self::to_ns(&mut state, &self.0.clock_ns, now);
        state.accounted_through = state.accounted_through.max(ns);
        drop(state);
        self.0.changed.notify_all();
    }

    /// `count` opportunities that fell due at `first_due`, `first_due + period`, ... were coalesced: each is admitted by its own due
    /// time. Exact in bounded work whatever `count` is (the ranges are intersected with the windows by arithmetic, see
    /// [`Windows::admit_range`]). The loop calls it when it coalesces; it is public (and hidden) only so a test can place it
    /// deterministically.
    #[doc(hidden)]
    pub fn missed(&self, first_due: Instant, period: Duration, count: u128) {
        if count == 0 {
            return;
        }
        let mut state = self.lock();
        let first = Self::to_ns(&mut state, &self.0.clock_ns, first_due);
        let step = u64::try_from(period.as_nanos()).unwrap_or(u64::MAX);
        let count = u64::try_from(count).unwrap_or(u64::MAX);
        state.totals.missed = state.totals.missed.saturating_add(count);
        let admitted = state.windows.admit_range(first, step, count);
        if let Some(tracked) = state.windows.current_mut() {
            tracked.stats.missed = tracked.stats.missed.saturating_add(admitted.current);
        }
        if let Some(tracked) = state.windows.last_ended_mut() {
            tracked.stats.missed = tracked.stats.missed.saturating_add(admitted.last_ended);
        }
    }

    /// The loop's `Instant` as window nanoseconds, from the pair fixed at the first adoption.
    fn to_ns(state: &mut LoopMetricsState, clock_ns: &UptimeNs, instant: Instant) -> u64 {
        let (base_instant, base_ns) = *state.base.get_or_insert_with(|| (instant, clock_ns()));
        if instant >= base_instant {
            base_ns.saturating_add(
                u64::try_from(instant.duration_since(base_instant).as_nanos()).unwrap_or(u64::MAX),
            )
        } else {
            base_ns.saturating_sub(
                u64::try_from(base_instant.duration_since(instant).as_nanos()).unwrap_or(u64::MAX),
            )
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    // --- windows ---

    fn window(id: u32, start: u64, end: Option<u64>) -> MetricsWindow {
        MetricsWindow { id, start, end }
    }

    #[test]
    fn a_sample_enters_the_window_only_by_its_own_timestamp() {
        let mut windows: Windows<u64> = Windows::new();
        windows
            .request(window(1, 1_000, Some(2_000)))
            .expect("a first request");
        windows.boundary(500);
        assert!(!windows.current().expect("adopted").late_adopted);
        for ns in [999, 2_000, 5_000] {
            assert!(windows.admit(ns).is_none(), "{ns} is outside");
        }
        for ns in [1_000, 1_500, 1_999] {
            assert!(windows.admit(ns).is_some(), "{ns} is inside");
        }
        assert_eq!(windows.current().expect("adopted").outside, 3);
    }

    #[test]
    fn samples_between_the_two_adoptions_but_before_start_are_in_neither_owner() {
        // Owner A adopts at 100, owner B at 400; start is 500. A sample at 300 (between the adoptions, before start) is outside both:
        // admission is by timestamp, not by who had adopted. Negative control: adoption-time admission would take it into A.
        let mut a: Windows<u64> = Windows::new();
        let mut b: Windows<u64> = Windows::new();
        let common = window(1, 500, Some(900));
        a.request(common).unwrap();
        b.request(common).unwrap();
        a.boundary(100);
        b.boundary(400);
        assert!(a.admit(300).is_none() && b.admit(300).is_none());
        assert!(a.admit(500).is_some() && b.admit(500).is_some());
        assert!(a.admit(899).is_some() && b.admit(899).is_some());
        assert!(a.admit(900).is_none() && b.admit(900).is_none());
        assert_eq!(
            (
                a.current().unwrap().adopted_at,
                b.current().unwrap().adopted_at
            ),
            (Some(100), Some(400))
        );
    }

    #[test]
    fn adopting_after_start_marks_the_window_late_adopted() {
        let mut windows: Windows<u64> = Windows::new();
        windows.request(window(1, 1_000, Some(3_000))).unwrap();
        windows.boundary(1_001);
        let tracked = windows.current().expect("adopted late");
        assert!(tracked.late_adopted);
        assert_eq!(tracked.adopted_at, Some(1_001));
        // Adopting exactly at start is not late.
        let mut on_time: Windows<u64> = Windows::new();
        on_time.request(window(1, 1_000, Some(3_000))).unwrap();
        on_time.boundary(1_000);
        assert!(!on_time.current().unwrap().late_adopted);
    }

    #[test]
    fn an_end_set_after_work_past_it_was_admitted_flags_the_overrun() {
        let mut windows: Windows<u64> = Windows::new();
        windows.request(window(1, 100, None)).unwrap();
        windows.boundary(50);
        assert!(windows.admit(150).is_some());
        assert!(windows.admit(260).is_some());
        windows.end(1, 200).expect("set the end once");
        let tracked = windows.current().unwrap();
        assert_eq!(tracked.last_admitted_ns, 260);
        assert!(
            tracked.end_overrun,
            "260 ns was admitted before the end of 200 ns was known"
        );
        // An end that no admitted sample passed is not flagged.
        let mut clean: Windows<u64> = Windows::new();
        clean.request(window(1, 100, None)).unwrap();
        clean.boundary(50);
        assert!(clean.admit(150).is_some());
        clean.end(1, 200).expect("set the end once");
        assert!(!clean.current().unwrap().end_overrun);
        assert_eq!(clean.current().unwrap().last_admitted_ns, 150);
    }

    #[test]
    fn an_open_window_takes_everything_from_start_until_its_end_is_set() {
        let mut windows: Windows<u64> = Windows::new();
        windows.request(window(7, 100, None)).unwrap();
        windows.boundary(50);
        assert!(windows.admit(99).is_none());
        assert!(windows.admit(10_000_000).is_some(), "open end");
        windows.end(7, 200).expect("set the end once");
        assert!(windows.admit(200).is_none() && windows.admit(199).is_some());
        assert_eq!(windows.end(7, 300), Err(EndError::AlreadyEnded));
        assert_eq!(windows.end(8, 300), Err(EndError::UnknownWindow));
    }

    #[test]
    fn an_overlapping_request_is_refused_and_changes_nothing_and_ids_are_never_reused() {
        let mut windows: Windows<u64> = Windows::new();
        windows.request(window(1, 1_000, Some(2_000))).unwrap();
        windows.boundary(0);
        // Starts before the current window's end: refused.
        assert_eq!(
            windows.request(window(2, 1_999, Some(3_000))),
            Err(RequestError::Overlaps)
        );
        // A refused request neither took the id nor replaced anything.
        assert_eq!(windows.current().unwrap().window.id, 1);
        windows
            .request(window(2, 2_000, Some(3_000)))
            .expect("starts at the previous end: not overlapping");
        assert_eq!(
            windows.request(window(2, 5_000, Some(6_000))),
            Err(RequestError::IdNotNew),
            "an id is never reused"
        );
        assert_eq!(
            windows.request(window(1, 5_000, Some(6_000))),
            Err(RequestError::IdNotNew)
        );
        // A window with no end yet blocks any later request.
        let mut open: Windows<u64> = Windows::new();
        open.request(window(1, 100, None)).unwrap();
        open.boundary(0);
        assert_eq!(
            open.request(window(2, 10_000, Some(20_000))),
            Err(RequestError::Overlaps)
        );
        // An empty window is refused.
        assert_eq!(
            Windows::<u64>::new().request(window(1, 5, Some(5))),
            Err(RequestError::EmptyWindow)
        );
    }

    #[test]
    fn the_id_counter_running_out_refuses_further_requests() {
        let mut windows: Windows<u64> = Windows::new();
        windows.request(window(u32::MAX, 10, Some(20))).unwrap();
        windows.boundary(0);
        assert_eq!(
            windows.request(window(u32::MAX, 30, Some(40))),
            Err(RequestError::IdNotNew)
        );
    }

    #[test]
    fn a_late_result_is_attributed_to_its_own_window_and_an_older_one_is_dropped_late() {
        let mut windows: Windows<u64> = Windows::new();
        windows.request(window(1, 1_000, Some(2_000))).unwrap();
        windows.boundary(0);
        windows.request(window(2, 2_000, Some(3_000))).unwrap();
        windows.boundary(2_500); // window 1 ended; window 2 adopted
        assert_eq!(windows.current().unwrap().window.id, 2);
        assert_eq!(windows.last_ended().unwrap().window.id, 1);
        // A result stamped inside window 1 that arrives now belongs to window 1, not to the current window.
        let late = windows.admit(1_500).expect("window 1 is still kept");
        assert_eq!(late.window.id, 1);
        // A third window retires window 1: a result stamped inside it is now older than anything kept.
        windows.request(window(3, 4_000, Some(5_000))).unwrap();
        windows.boundary(3_500);
        assert_eq!(windows.last_ended().unwrap().window.id, 2);
        assert!(
            windows.admit(1_500).is_none(),
            "older than the last ended window"
        );
        assert_eq!(windows.dropped_late(), 1);
        assert_eq!(windows.current().unwrap().dropped_late, 1);
    }

    #[test]
    fn a_second_request_while_one_is_unadopted_is_refused_and_the_first_is_kept() {
        let mut windows: Windows<u64> = Windows::new();
        windows.request(window(1, 100, Some(200))).unwrap();
        // A disjoint later request, before any boundary: refused, nothing changes, the id is not consumed.
        assert_eq!(
            windows.request(window(2, 200, Some(300))),
            Err(RequestError::Pending)
        );
        assert!(windows.has_pending());
        windows.boundary(50);
        assert_eq!(
            windows.current().unwrap().window.id,
            1,
            "the first request was adopted"
        );
        // Identity state is unchanged: id 2 is still new, and is accepted once nothing is waiting and the window is clear.
        windows
            .request(window(2, 200, Some(300)))
            .expect("accepted after the adoption");
        windows.boundary(250);
        assert_eq!(windows.current().unwrap().window.id, 2);
        assert_eq!(windows.last_ended().unwrap().window.id, 1);
    }

    #[test]
    fn a_range_of_timestamps_is_admitted_exactly_in_bounded_work() {
        // 5,000 timestamps 0 .. 4,999: the measured window holds exactly the 100 of 4,500 .. 4,599, wherever the prefix lies.
        let mut windows: Windows<u64> = Windows::new();
        windows.request(window(1, 4_500, Some(4_600))).unwrap();
        windows.boundary(0);
        let admitted = windows.admit_range(0, 1, 5_000);
        assert_eq!((admitted.current, admitted.last_ended), (100, 0));
        let tracked = windows.current().unwrap();
        assert_eq!(tracked.outside, 4_900);
        assert_eq!(tracked.last_admitted_ns, 4_599);
        // Whatever the count, the work does not grow: a count of u64::MAX returns at once, exactly (the same 100 lie in the window).
        let huge = windows.admit_range(0, 1, u64::MAX);
        assert_eq!(huge.current, 100);
    }

    #[test]
    fn a_range_agrees_with_admitting_every_timestamp_one_by_one() {
        // Two retained windows and an older one dropped: the arithmetic equals the loop over `admit`, for many ranges.
        let build = || {
            let mut windows: Windows<u64> = Windows::new();
            windows.request(window(1, 100, Some(200))).unwrap();
            windows.boundary(0);
            windows.request(window(2, 300, Some(400))).unwrap();
            windows.boundary(250);
            windows
        };
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            seed >> 33
        };
        for _ in 0..300 {
            let (first, step, count) = (next() % 450, 1 + next() % 40, next() % 60);
            let mut counted = build();
            let admitted = counted.admit_range(first, step, count);
            let mut one_by_one = build();
            let (mut current, mut last) = (0_u64, 0_u64);
            for k in 0..count {
                let ns = first + k * step;
                if let Some(tracked) = one_by_one.admit(ns) {
                    if tracked.window.id == 2 {
                        current += 1;
                    } else {
                        last += 1;
                    }
                }
            }
            assert_eq!(
                (admitted.current, admitted.last_ended),
                (current, last),
                "{first} {step} {count}"
            );
            assert_eq!(
                counted.dropped_late(),
                one_by_one.dropped_late(),
                "{first} {step} {count}"
            );
            assert_eq!(
                counted.current().unwrap().outside,
                one_by_one.current().unwrap().outside,
                "{first} {step} {count}"
            );
            assert_eq!(
                counted.current().unwrap().dropped_late,
                one_by_one.current().unwrap().dropped_late,
                "{first} {step} {count}"
            );
        }
    }

    #[test]
    fn a_sample_with_no_current_window_is_outside_not_lost() {
        let mut windows: Windows<u64> = Windows::new();
        assert!(windows.admit(5).is_none());
        assert_eq!(windows.outside_without_window(), 1);
    }

    // --- the loop's metrics ---

    fn clock(now: &Arc<Mutex<u64>>) -> UptimeNs {
        let now = Arc::clone(now);
        Arc::new(move || *now.lock().unwrap())
    }

    #[test]
    fn delivered_updates_are_admitted_by_the_time_they_were_posted() {
        // The loop's Instant `base` maps to the clock's 1_000_000 ns; the window is [1_000_500, 1_001_500).
        let uptime = Arc::new(Mutex::new(1_000_000_u64));
        let metrics = LoopMetrics::new(clock(&uptime));
        let base = Instant::now();
        metrics
            .request(MetricsWindow {
                id: 1,
                start: 1_000_500,
                end: Some(1_001_500),
            })
            .expect("request");
        assert_eq!(
            metrics.adopted(1),
            None,
            "not adopted before the loop's boundary"
        );
        metrics.boundary(base);
        assert_eq!(
            metrics.adopted(1),
            Some(1_000_000),
            "adopted at the loop's boundary"
        );
        // Posted 0, 100, 600, 1400 and 1600 ns after the base: only 600 and 1400 are inside.
        for offset in [0_u64, 100, 600, 1_400, 1_600] {
            metrics.delivered(base + Duration::from_nanos(offset));
        }
        let reading = metrics.snapshot(1).expect("kept");
        assert_eq!(reading.window.stats.delivered, 2);
        assert_eq!(reading.window.outside, 3);
        assert_eq!(
            metrics.totals().delivered,
            5,
            "the whole run counts the outside ones too"
        );
    }

    #[test]
    fn missed_opportunities_are_admitted_by_their_own_due_times_and_a_long_stall_is_bounded() {
        let uptime = Arc::new(Mutex::new(0_u64));
        let metrics = LoopMetrics::new(clock(&uptime));
        let base = Instant::now();
        metrics
            .request(MetricsWindow {
                id: 1,
                start: 3_000,
                end: Some(6_000),
            })
            .expect("request");
        metrics.boundary(base);
        // Ten opportunities due at 1000, 2000, ..., 10000 ns, coalesced: only those due in [3000, 6000) are missed in the window.
        metrics.missed(
            base + Duration::from_nanos(1_000),
            Duration::from_nanos(1_000),
            10,
        );
        let window = metrics.snapshot(1).expect("kept").window;
        assert_eq!(window.stats.missed, 3, "due at 3000, 4000 and 5000");
        assert_eq!(window.outside, 7);
    }

    #[test]
    fn the_loop_counters_flag_their_own_saturation() {
        assert!(!LoopWindowStats::default().saturated());
        assert!(
            LoopWindowStats {
                delivered: u64::MAX,
                missed: 0
            }
            .saturated()
        );
        assert!(
            LoopWindowStats {
                delivered: 0,
                missed: u64::MAX
            }
            .saturated()
        );
    }

    #[test]
    fn a_long_stall_counts_exactly_the_due_times_inside_the_window_wherever_the_prefix_lies() {
        // 5,000 opportunities due at 0, 1, ..., 4,999 ns coalesced at once; the window holds exactly 4,500 .. 4,599 (100 of them).
        // A loop that admitted only a bounded prefix one by one would count none of them.
        let uptime = Arc::new(Mutex::new(0_u64));
        let metrics = LoopMetrics::new(clock(&uptime));
        let base = Instant::now();
        metrics
            .request(MetricsWindow {
                id: 1,
                start: 4_500,
                end: Some(4_600),
            })
            .expect("request");
        metrics.boundary(base);
        metrics.missed(base, Duration::from_nanos(1), 5_000);
        let window = metrics.snapshot(1).expect("kept").window;
        assert_eq!(window.stats.missed, 100);
        assert_eq!(window.outside, 4_900);
        assert_eq!(metrics.totals().missed, 5_000);
        // A count beyond anything a loop could produce is still exact and immediate.
        metrics.missed(base, Duration::from_nanos(1), u128::MAX);
        assert_eq!(metrics.snapshot(1).expect("kept").window.stats.missed, 200);
    }

    #[test]
    fn the_loop_adopts_a_request_only_at_its_boundary_and_read_waits_for_the_end() {
        let uptime = Arc::new(Mutex::new(5_000_u64));
        let metrics = LoopMetrics::new(clock(&uptime));
        let base = Instant::now();
        metrics
            .request(MetricsWindow {
                id: 1,
                start: 6_000,
                end: None,
            })
            .expect("request");
        assert_eq!(
            metrics.adopted(1),
            None,
            "not adopted before the loop's boundary"
        );
        metrics.boundary(base);
        assert_eq!(metrics.adopted(1), Some(5_000));
        assert!(!metrics.snapshot(1).expect("kept").window.late_adopted);
        metrics.end(1, 7_000).expect("the end, once");
        // The clock is at 5000: not passed, so the read times out incomplete.
        let early = metrics.read(1, Duration::from_millis(20)).expect("kept");
        assert!(!early.complete);
        *uptime.lock().unwrap() = 7_000;
        // The clock has passed the end, but the loop has not examined its schedule through it: not final.
        let waiting = metrics.read(1, Duration::from_millis(20)).expect("kept");
        assert!(
            !waiting.complete,
            "the loop's progress fence has not reached the end"
        );
        metrics.accounted(base + Duration::from_nanos(1_900));
        let still = metrics.read(1, Duration::from_millis(20)).expect("kept");
        assert!(
            !still.complete,
            "accounted through 6,900 ns, short of the end at 7,000 ns"
        );
        metrics.accounted(base + Duration::from_nanos(2_000));
        let done = metrics.read(1, Duration::from_millis(20)).expect("kept");
        assert!(done.complete);
        assert_eq!(
            (done.window.window.start, done.window.window.end),
            (6_000, Some(7_000))
        );
    }
}
