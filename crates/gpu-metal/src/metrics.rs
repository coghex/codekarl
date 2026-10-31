//! Bounded frame-timing instrumentation (milestone 5, issue 10b-1; design `docs/scene-design.md` §11.3).
//!
//! Everything here is plain data and plain Rust, so it is tested without a device: a fixed [`Histogram`] of 512 buckets (no
//! allocation, O(1) recording), nearest-rank percentiles on bucket upper bounds, and the measurement windows an owner keeps
//! ([`Windows`]): one **common** window, defined once by the requester as `[start, end)` on one clock (`CLOCK_UPTIME_RAW`
//! nanoseconds), into which each owner admits a sample only by **its own timestamp**, whatever order the owners adopt the window in.
//!
//! The clock. Every figure is in nanoseconds of `CLOCK_UPTIME_RAW` ([`now_ns`]). `CFTimeInterval` host times (a drawable's
//! `presentedTime`, a command buffer's `GPUStartTime`) are seconds of `mach_absolute_time` scaled by the host time base, which is
//! exactly what `clock_gettime_nsec_np(CLOCK_UPTIME_RAW)` returns, so [`seconds_to_ns`] is a plain scaling (a test checks that a time
//! taken with [`now_ns`] and one taken as seconds from `mach_absolute_time` agree).

use std::ffi::c_int;

use crate::batch::FrameCounters;

unsafe extern "C" {
    fn clock_gettime_nsec_np(clock_id: c_int) -> u64;
}

/// `CLOCK_UPTIME_RAW` on macOS.
const CLOCK_UPTIME_RAW: c_int = 8;

/// The current time on the instrumentation's one clock: `CLOCK_UPTIME_RAW` nanoseconds.
#[must_use]
pub fn now_ns() -> u64 {
    // SAFETY: `clock_gettime_nsec_np` has no preconditions; it reads the system clock.
    unsafe { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) }
}

/// A `CFTimeInterval` (seconds of `mach_absolute_time`, scaled by the host time base) as `CLOCK_UPTIME_RAW` nanoseconds. A
/// non-finite, negative or zero value is `0` ("no time"), which every consumer treats as not recorded.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a finite, positive number of seconds times 1e9 is a nanosecond count far below u64::MAX for any real uptime; the cast saturates"
)]
pub fn seconds_to_ns(seconds: f64) -> u64 {
    if seconds.is_finite() && seconds > 0.0 {
        (seconds * 1.0e9).round() as u64
    } else {
        0
    }
}

// ---------------------------------------------------------------------------------------------
// The histogram
// ---------------------------------------------------------------------------------------------

/// Buckets of 25 microseconds covering 0 to 8 ms.
const FINE: usize = 320;
/// Buckets of 250 microseconds covering 8 to 40 ms.
const MEDIUM: usize = 128;
/// Buckets of 2.5 ms covering 40 to 200 ms.
const COARSE: usize = 64;
/// The number of buckets.
pub const BUCKETS: usize = FINE + MEDIUM + COARSE;
const FINE_WIDTH: u64 = 25_000;
const MEDIUM_WIDTH: u64 = 250_000;
const COARSE_WIDTH: u64 = 2_500_000;
const FINE_END: u64 = FINE as u64 * FINE_WIDTH;
const MEDIUM_END: u64 = FINE_END + MEDIUM as u64 * MEDIUM_WIDTH;
/// The first value that is in no bucket (200 ms): it counts as overflow.
pub const RANGE_END_NS: u64 = MEDIUM_END + COARSE as u64 * COARSE_WIDTH;

/// The `[lower, upper)` bounds of bucket `index` in nanoseconds, and its width.
fn bounds(index: usize) -> (u64, u64) {
    let index = index as u64;
    if index < FINE as u64 {
        (index * FINE_WIDTH, (index + 1) * FINE_WIDTH)
    } else if index < (FINE + MEDIUM) as u64 {
        let local = index - FINE as u64;
        (
            FINE_END + local * MEDIUM_WIDTH,
            FINE_END + (local + 1) * MEDIUM_WIDTH,
        )
    } else {
        let local = index - (FINE + MEDIUM) as u64;
        (
            MEDIUM_END + local * COARSE_WIDTH,
            MEDIUM_END + (local + 1) * COARSE_WIDTH,
        )
    }
}

/// The bucket a value belongs to, `None` for a value in the overflow.
fn bucket_of(ns: u64) -> Option<usize> {
    if ns < FINE_END {
        Some(usize::try_from(ns / FINE_WIDTH).unwrap_or(usize::MAX))
    } else if ns < MEDIUM_END {
        Some(FINE + usize::try_from((ns - FINE_END) / MEDIUM_WIDTH).unwrap_or(usize::MAX))
    } else if ns < RANGE_END_NS {
        Some(
            FINE + MEDIUM + usize::try_from((ns - MEDIUM_END) / COARSE_WIDTH).unwrap_or(usize::MAX),
        )
    } else {
        None
    }
}

/// A percentile read from a [`Histogram`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Percentile {
    /// The window has no samples: there is no value (never zero).
    NoValue,
    /// The nearest-rank percentile on the bucket's upper bound: never below the true value, and above it by less than
    /// `error_ns` (the bucket's width).
    Value {
        /// The reported value, nanoseconds.
        upper_bound_ns: u64,
        /// The bucket's width: the most the report can exceed the true percentile by.
        error_ns: u64,
    },
    /// The rank falls in the overflow (200 ms or more): the true value is out of range, and at most the exact maximum.
    OutOfRange {
        /// The exact maximum of every sample recorded.
        at_most_ns: u64,
    },
}

/// A fixed histogram: 512 buckets (0-8 ms in 25 us, 8-40 ms in 250 us, 40-200 ms in 2.5 ms), an overflow counter and the exact
/// maximum. Recording is O(1) and allocates nothing; the counters saturate and flag it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    counts: [u64; BUCKETS],
    overflow: u64,
    max_ns: u64,
    saturated: bool,
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

impl Histogram {
    /// An empty histogram.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            counts: [0; BUCKETS],
            overflow: 0,
            max_ns: 0,
            saturated: false,
        }
    }

    /// Records one sample.
    pub fn record(&mut self, ns: u64) {
        let slot = match bucket_of(ns) {
            Some(index) => &mut self.counts[index],
            None => &mut self.overflow,
        };
        match slot.checked_add(1) {
            Some(next) => *slot = next,
            None => self.saturated = true,
        }
        self.max_ns = self.max_ns.max(ns);
    }

    /// The number of samples recorded (the overflow included), saturating.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.counts
            .iter()
            .fold(self.overflow, |sum, count| sum.saturating_add(*count))
    }

    /// The samples in the overflow.
    #[must_use]
    pub fn overflow(&self) -> u64 {
        self.overflow
    }

    /// The exact maximum sample, `None` for an empty histogram.
    #[must_use]
    pub fn max_ns(&self) -> Option<u64> {
        (self.count() != 0).then_some(self.max_ns)
    }

    /// Whether any counter saturated: the figures are then lower bounds.
    #[must_use]
    pub fn saturated(&self) -> bool {
        self.saturated
            || self
                .counts
                .iter()
                .try_fold(self.overflow, |sum, count| sum.checked_add(*count))
                .is_none()
    }

    /// The nearest-rank percentile (`permille` of 1000: 500, 950, 990) on bucket upper bounds.
    #[must_use]
    pub fn percentile(&self, permille: u32) -> Percentile {
        let total = self.count();
        if total == 0 {
            return Percentile::NoValue;
        }
        let permille = u128::from(permille.clamp(1, 1000));
        // rank = ceil(total x permille / 1000), at least 1.
        let rank = u64::try_from((u128::from(total) * permille).div_ceil(1000))
            .unwrap_or(u64::MAX)
            .max(1);
        let mut seen = 0_u64;
        for (index, count) in self.counts.iter().enumerate() {
            seen = seen.saturating_add(*count);
            if seen >= rank {
                let (lower, upper) = bounds(index);
                return Percentile::Value {
                    upper_bound_ns: upper,
                    error_ns: upper - lower,
                };
            }
        }
        Percentile::OutOfRange {
            at_most_ns: self.max_ns,
        }
    }

    /// Empties the histogram.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Test support: a histogram whose bucket `index` already holds `count` samples (to reach saturation).
    #[cfg(test)]
    fn preload(&mut self, ns: u64, count: u64) {
        match bucket_of(ns) {
            Some(index) => self.counts[index] = count,
            None => self.overflow = count,
        }
        self.max_ns = self.max_ns.max(ns);
    }
}

// ---------------------------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------------------------

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
// The presenter's statistics for one window
// ---------------------------------------------------------------------------------------------

/// What the presenter's render thread gathers for one window (frame intervals, GPU time, presentation, counts).
#[derive(Debug, Clone, Default)]
pub struct FrameStats {
    /// **The CPU total** of each qualifying frame, one sample per frame: `(t3 - t0) + (t5 - t4)`, everything from the slot take to
    /// `commit`'s return except waiting for the drawable. CK-130's CPU gate is this histogram's p95, never a sum of the components'.
    pub cpu_total: Histogram,
    /// `t1 - t0`: the texture driver's pass and any boundary adoption.
    pub ledger: Histogram,
    /// `t2 - t1`: placement, resolution and the instance writes.
    pub build: Histogram,
    /// `(t3 - t2) + (t5 - t4)`: setup before the acquisition and everything after it up to `commit`'s return.
    pub encode: Histogram,
    /// `t4 - t3`: waiting for the drawable.
    pub acquire: Histogram,
    /// `GPUEndTime - GPUStartTime` of a frame's command buffer.
    pub gpu: Histogram,
    /// The difference between consecutive presented times.
    pub presentation_interval: Histogram,
    /// Frames committed with a drawable and valid marks.
    pub frames: u64,
    /// Frames that committed nothing (no drawable, an acquisition timeout, a skipped frame).
    pub skipped: u64,
    /// Frames with a missing or decreasing mark.
    pub invalid_marks: u64,
    /// GPU times that were inverted or had a zero start.
    pub invalid_gpu: u64,
    /// Drawables that were presented / not presented (a zero `presentedTime`).
    pub presented: u64,
    /// Drawables with a zero `presentedTime`.
    pub not_presented: u64,
    /// Presentation intervals excluded because one of the two presented times lay outside the window.
    pub excluded_intervals: u64,
    /// Instances drawn: the sum over frames and the largest frame.
    pub instances_sum: u64,
    /// The most instances one frame drew.
    pub instances_max: u64,
    /// Scene draw calls: the sum over frames and the most in one frame.
    pub draw_calls_sum: u64,
    /// The most scene draw calls in one frame.
    pub draw_calls_max: u64,
    /// Texture binds (the table counts as one per frame in the bindless path), summed.
    pub texture_binds: u64,
    /// Checker instances, summed.
    pub checker_instances: u64,
    /// Frames committed in the window whose GPU time or presentation was not yet consumed when the snapshot was taken.
    pub outstanding: u64,
}

impl FrameStats {
    /// Adds `delta` to a counter, flagging nothing (a `u64` frame counter does not wrap in a run; the histograms flag theirs).
    fn add(counter: &mut u64, delta: u64) {
        *counter = counter.saturating_add(delta);
    }

    /// Records a frame's draw counts.
    pub fn count_frame(&mut self, instances: u64, draw_calls: u64, binds: u64, checker: u64) {
        Self::add(&mut self.frames, 1);
        Self::add(&mut self.instances_sum, instances);
        self.instances_max = self.instances_max.max(instances);
        Self::add(&mut self.draw_calls_sum, draw_calls);
        self.draw_calls_max = self.draw_calls_max.max(draw_calls);
        Self::add(&mut self.texture_binds, binds);
        Self::add(&mut self.checker_instances, checker);
    }

    /// Counts `n` frames that committed nothing.
    pub fn count_skipped(&mut self) {
        Self::add(&mut self.skipped, 1);
    }

    /// Counts a frame with an invalid mark.
    pub fn count_invalid_marks(&mut self) {
        Self::add(&mut self.invalid_marks, 1);
    }

    /// Whether any counter or histogram of this window saturated.
    #[must_use]
    pub fn saturated(&self) -> bool {
        [
            &self.cpu_total,
            &self.ledger,
            &self.build,
            &self.encode,
            &self.acquire,
            &self.gpu,
            &self.presentation_interval,
        ]
        .iter()
        .any(|histogram| histogram.saturated())
            || [
                self.frames,
                self.skipped,
                self.invalid_marks,
                self.invalid_gpu,
                self.presented,
                self.not_presented,
                self.excluded_intervals,
                self.outstanding,
                self.instances_sum,
                self.draw_calls_sum,
                self.texture_binds,
                self.checker_instances,
            ]
            .contains(&u64::MAX)
    }
}

// ---------------------------------------------------------------------------------------------
// One frame's marks
// ---------------------------------------------------------------------------------------------

/// The six marks of one frame step, all `CLOCK_UPTIME_RAW` nanoseconds, in this order: the latest frame taken from the slot (`t0`),
/// the texture driver's pass and any boundary adoption returned (`t1`), the frame built and the native step entered (`t2`), the
/// native step calling `nextDrawable` (`t3`), `nextDrawable` returned (`t4`), and the command buffer's `commit` returned (`t5`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameMarks {
    /// The latest frame taken from the slot.
    pub t0: u64,
    /// The driver's pass and any adoption returned.
    pub t1: u64,
    /// The frame built; the native step entered.
    pub t2: u64,
    /// The native step calls `nextDrawable`.
    pub t3: u64,
    /// `nextDrawable` returned.
    pub t4: u64,
    /// `commit` returned.
    pub t5: u64,
}

/// The contiguous intervals of a frame, from valid marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameIntervals {
    /// `t1 - t0`.
    pub ledger: u64,
    /// `t2 - t1`.
    pub build: u64,
    /// `t4 - t3`.
    pub acquire: u64,
    /// `(t3 - t2) + (t5 - t4)`.
    pub encode: u64,
    /// `ledger + build + encode` = `(t3 - t0) + (t5 - t4)`: **everything except waiting for the drawable**.
    pub cpu_total: u64,
}

impl FrameMarks {
    /// The intervals of this frame, `None` when a mark is missing (zero) or the marks decrease (the frame is then counted invalid).
    #[must_use]
    pub fn intervals(&self) -> Option<FrameIntervals> {
        let marks = [self.t0, self.t1, self.t2, self.t3, self.t4, self.t5];
        if marks.contains(&0) || marks.windows(2).any(|pair| pair[0] > pair[1]) {
            return None;
        }
        let ledger = self.t1 - self.t0;
        let build = self.t2 - self.t1;
        let acquire = self.t4 - self.t3;
        let encode = (self.t3 - self.t2) + (self.t5 - self.t4);
        Some(FrameIntervals {
            ledger,
            build,
            acquire,
            encode,
            cpu_total: ledger + build + encode,
        })
    }
}

impl FrameStats {
    /// Records one qualifying frame's intervals (the marks were valid): the components and the total, which is that frame's own sum.
    pub fn record_intervals(&mut self, intervals: FrameIntervals) {
        self.ledger.record(intervals.ledger);
        self.build.record(intervals.build);
        self.encode.record(intervals.encode);
        self.acquire.record(intervals.acquire);
        self.cpu_total.record(intervals.cpu_total);
    }

    /// Records a command buffer's GPU time from its two timestamps; an end before the start, or a zero start, is invalid and not
    /// sampled.
    pub fn record_gpu(&mut self, start_ns: u64, end_ns: u64) {
        if start_ns == 0 || end_ns < start_ns {
            Self::add(&mut self.invalid_gpu, 1);
        } else {
            self.gpu.record(end_ns - start_ns);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The recorder
// ---------------------------------------------------------------------------------------------

/// The presenter's measurement windows and whole-run totals, with the rules that put a finished frame or a consumed timing result in
/// them. It lives behind the presenter's one lock and is written by the render thread only; it holds fixed storage (two windows, each
/// seven histograms), so recording allocates nothing and is O(1).
#[derive(Default)]
pub(crate) struct Recorder {
    /// The pending, current and last ended windows.
    pub(crate) windows: Windows<FrameStats>,
    /// The whole run's figures, every sample whatever its window (the end-of-run record's totals).
    pub(crate) totals: FrameStats,
    /// Timing results the native ring had no room for (never expected: the ring holds more than can be outstanding).
    pub(crate) timing_lost: u64,
    /// The previous drawable's presented time (zero until one was shown): presentation intervals are between consecutive presented
    /// times.
    previous_presented: u64,
    /// The first mark of the frame step in flight, from when the render thread started it until it has recorded it: the owner's
    /// progress fence. A step that began before a window's end may still register a frame committed before that end, so the window
    /// is not final while one is in flight.
    in_flight_since: Option<u64>,
    /// The commit marks of committed frames whose presentation result is not yet consumed, whatever window they belong to: a drawable
    /// committed before a window opens can be shown inside it, and the interval it closes is that window's sample. Bounded by the
    /// drawable pool and the one-frame-in-flight rule; `pending_overflow` says the bound was exceeded (the figures are then
    /// insufficient evidence, never final).
    pending_presentations: [PendingPresentation; PENDING_PRESENTATIONS],
    pending_len: usize,
    pending_overflow: bool,
    /// The render thread has ended: no frame will be committed or shown again, so a window's closing interval cannot appear.
    stopped: bool,
}

/// A committed frame whose presentation is to come: its commit mark (`t5`, what the result is settled by) and the earliest instant it
/// can have been shown: the clock read **before** `commit` was called (`t5` is read after it returns and can be late).
#[derive(Clone, Copy, Default)]
struct PendingPresentation {
    t5: u64,
    not_before: u64,
}

/// How many committed frames' presentations the recorder tracks at once.
const PENDING_PRESENTATIONS: usize = 16;

impl Recorder {
    /// The render thread starts a frame step whose first mark is `t0`.
    pub(crate) fn begin_step(&mut self, t0: u64) {
        self.in_flight_since = Some(t0);
    }

    /// The render thread has recorded the step (or found nothing to record).
    pub(crate) fn end_step(&mut self) {
        self.in_flight_since = None;
    }

    /// Whether this owner has accounted for everything that can still change a window ending at `end`: no step that began before it
    /// is unrecorded, and no frame committed before it still has its presentation to come (its drawable can be shown inside the
    /// window whatever its commit time, and the interval it closes belongs to the window). A frame committed at or after `end` is
    /// shown after it and cannot matter.
    pub(crate) fn accounted_through(&self, end: u64) -> bool {
        self.in_flight_since.is_none_or(|since| since >= end)
            && !self.pending_overflow
            && !self.pending_presentations[..self.pending_len]
                .iter()
                .any(|pending| pending.not_before < end)
    }

    /// The tracking bound was exceeded at some point: a reading can never be shown as final after that.
    pub(crate) fn evidence_lost(&self) -> bool {
        self.pending_overflow
    }

    /// A frame was committed (`t5`, with `submitted` read before the commit call): its presentation is to come, and cannot be earlier
    /// than `submitted`, nor, in a clock that is monotonic, than `t5` unless `t5` was read late.
    fn owe_presentation(&mut self, t5: u64, submitted: u64) {
        let not_before = if submitted == 0 {
            t5
        } else {
            submitted.min(t5)
        };
        if self.pending_len < PENDING_PRESENTATIONS {
            self.pending_presentations[self.pending_len] = PendingPresentation { t5, not_before };
            self.pending_len += 1;
        } else {
            self.pending_overflow = true;
        }
    }

    /// The presentation of the frame committed at `t5` was consumed.
    fn settle_presentation(&mut self, t5: u64) {
        if let Some(index) = self.pending_presentations[..self.pending_len]
            .iter()
            .position(|pending| pending.t5 == t5)
        {
            self.pending_len -= 1;
            self.pending_presentations[index] = self.pending_presentations[self.pending_len];
        }
    }

    /// A committed frame step: with valid marks its intervals (the CPU total once, as that frame's own sum) and counts, and two
    /// results still to come (its GPU time and its presentation); with invalid marks it is counted and not sampled. Admitted by the
    /// frame's commit mark `t5`.
    #[cfg(test)]
    pub(crate) fn drawn(&mut self, marks: &FrameMarks, counters: &FrameCounters) {
        self.drawn_at(marks, marks.t5, counters);
    }

    /// [`drawn`](Self::drawn) with the clock read before the commit call (`submitted`, zero if unknown): the earliest the frame's
    /// drawable can have been shown, which finality rests on.
    pub(crate) fn drawn_at(
        &mut self,
        marks: &FrameMarks,
        submitted: u64,
        counters: &FrameCounters,
    ) {
        let Some(intervals) = marks.intervals() else {
            // Counted and not sampled, but the command buffer was committed (when `t5` is set) and its GPU time and presentation will
            // arrive: they are owed to the window of the commit mark, so completion reconciliation does not lose them.
            self.totals.count_invalid_marks();
            if marks.t5 != 0 {
                self.owe_presentation(marks.t5, submitted);
            }
            let at = if marks.t5 != 0 { marks.t5 } else { marks.t0 };
            if let Some(tracked) = self.windows.admit(at) {
                tracked.stats.count_invalid_marks();
                if marks.t5 != 0 {
                    tracked.stats.outstanding = tracked.stats.outstanding.saturating_add(2);
                }
            }
            return;
        };
        let (instances, calls, binds, checker) = (
            counters.instances,
            counters.batches,
            counters.texture_binds,
            counters.checker,
        );
        self.totals.record_intervals(intervals);
        self.totals.count_frame(instances, calls, binds, checker);
        self.owe_presentation(marks.t5, submitted);
        if let Some(tracked) = self.windows.admit(marks.t5) {
            tracked.stats.record_intervals(intervals);
            tracked.stats.count_frame(instances, calls, binds, checker);
            tracked.stats.outstanding = tracked.stats.outstanding.saturating_add(2);
        }
    }

    /// A frame step that committed nothing (skipped or abandoned): counted, admitted by its first mark, never sampled.
    pub(crate) fn not_drawn(&mut self, t0: u64) {
        self.totals.count_skipped();
        if let Some(tracked) = self.windows.admit(t0) {
            tracked.stats.count_skipped();
        }
    }

    /// A command buffer's GPU timestamps, belonging to the frame committed at `t5`.
    pub(crate) fn gpu(&mut self, t5: u64, start_ns: u64, end_ns: u64) {
        self.totals.record_gpu(start_ns, end_ns);
        if let Some(tracked) = self.windows.admit(t5) {
            tracked.stats.record_gpu(start_ns, end_ns);
            tracked.stats.outstanding = tracked.stats.outstanding.saturating_sub(1);
        }
    }

    /// A drawable's presented time (zero: never shown), belonging to the frame committed at `t5`. The frame's own counts (presented or
    /// not, and the result it owed) go to the window of its commit mark. The presentation **interval** from the previous presented
    /// drawable is a pair of timestamps and is attributed by them, not by the commit: for each window the owner keeps, an interval
    /// whose two ends lie inside it is sampled there; one with exactly one end inside crosses the window's start or end and is
    /// counted as excluded there; any other is not that window's. A frame committed before a window whose drawable is shown inside it
    /// therefore still gives the window its interval. A window is final once every frame submitted before its end has had its
    /// presentation consumed ([`accounted_through`](Self::accounted_through)) **and** its closing interval is known
    /// ([`closing_pending`](Self::closing_pending)).
    pub(crate) fn presented(&mut self, t5: u64, presented_ns: u64) {
        self.settle_presentation(t5);
        let previous = self.previous_presented;
        let interval = (presented_ns != 0 && previous != 0 && presented_ns >= previous)
            .then_some((previous, presented_ns));
        if presented_ns == 0 {
            Self::note(&mut self.totals.not_presented);
        } else {
            Self::note(&mut self.totals.presented);
            if let Some((from, to)) = interval {
                self.totals.presentation_interval.record(to - from);
            }
            self.previous_presented = presented_ns;
        }
        if let Some(tracked) = self.windows.admit(t5) {
            tracked.stats.outstanding = tracked.stats.outstanding.saturating_sub(1);
            if presented_ns == 0 {
                Self::note(&mut tracked.stats.not_presented);
            } else {
                Self::note(&mut tracked.stats.presented);
            }
        }
        if let Some((from, to)) = interval {
            if let Some(tracked) = self.windows.current_mut() {
                Self::interval_in(tracked, from, to);
            }
            if let Some(tracked) = self.windows.last_ended_mut() {
                Self::interval_in(tracked, from, to);
            }
        }
    }

    /// One presentation interval against one window: sampled when both ends lie inside; counted as excluded when exactly one does (it
    /// crosses the window's start or its end); not that window's when neither does.
    fn interval_in(tracked: &mut Tracked<FrameStats>, from: u64, to: u64) {
        match (tracked.window.contains(from), tracked.window.contains(to)) {
            (true, true) => tracked.stats.presentation_interval.record(to - from),
            (true, false) | (false, true) => Self::note(&mut tracked.stats.excluded_intervals),
            (false, false) => {}
        }
    }

    /// The render thread has ended: nothing will be committed or shown again.
    pub(crate) fn stop(&mut self) {
        self.stopped = true;
    }

    /// Whether the window's **closing interval** is still to come: its latest shown drawable lies inside it (`previous_presented`), so
    /// the next drawable shown after the end closes an interval that crosses the end, which r3 counts as excluded. Until a drawable
    /// has been shown at or after the end (or the thread has ended), that count can still grow by one, so the reading is not final.
    pub(crate) fn closing_pending(&self, window: &MetricsWindow) -> bool {
        !self.stopped && self.previous_presented != 0 && window.contains(self.previous_presented)
    }

    fn note(counter: &mut u64) {
        *counter = counter.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }

    fn value(histogram: &Histogram, permille: u32) -> u64 {
        match histogram.percentile(permille) {
            Percentile::Value { upper_bound_ns, .. } => upper_bound_ns,
            other => panic!("expected a value, got {other:?}"),
        }
    }

    #[test]
    fn the_buckets_tile_zero_to_two_hundred_milliseconds_with_the_stated_widths() {
        assert_eq!(BUCKETS, 512);
        assert_eq!(RANGE_END_NS, 200_000_000);
        assert_eq!(bounds(0), (0, 25_000));
        assert_eq!(bounds(319), (7_975_000, 8_000_000));
        assert_eq!(bounds(320), (8_000_000, 8_250_000));
        assert_eq!(bounds(447), (39_750_000, 40_000_000));
        assert_eq!(bounds(448), (40_000_000, 42_500_000));
        assert_eq!(bounds(511), (197_500_000, 200_000_000));
        for index in 1..BUCKETS {
            assert_eq!(
                bounds(index).0,
                bounds(index - 1).1,
                "bucket {index} follows"
            );
        }
        // Every boundary value belongs to the bucket it opens; the last nanosecond below 200 ms is in the last bucket.
        for ns in [
            0,
            24_999,
            25_000,
            7_999_999,
            8_000_000,
            39_999_999,
            40_000_000,
            199_999_999,
        ] {
            let index = bucket_of(ns).expect("in range");
            let (lower, upper) = bounds(index);
            assert!(
                lower <= ns && ns < upper,
                "{ns} in bucket {index} [{lower}, {upper})"
            );
        }
        assert_eq!(bucket_of(200_000_000), None);
    }

    #[test]
    fn a_sample_in_the_overflow_is_counted_and_the_maximum_is_exact() {
        let mut histogram = Histogram::new();
        histogram.record(1_000_000);
        histogram.record(200_000_000);
        histogram.record(731_000_123);
        assert_eq!((histogram.count(), histogram.overflow()), (3, 2));
        assert_eq!(
            histogram.max_ns(),
            Some(731_000_123),
            "the exact maximum, not a bucket's bound"
        );
    }

    #[test]
    fn nearest_rank_percentiles_are_never_below_the_true_value_and_within_one_bucket() {
        // 100 samples of 1 ms, 2 ms, ..., 100 ms. The true nearest-rank p95 is the 95th sample, 95 ms.
        let mut histogram = Histogram::new();
        for step in 1..=100_u64 {
            histogram.record(step * 1_000_000);
        }
        for (permille, true_ns) in [
            (500, 50_000_000_u64),
            (950, 95_000_000),
            (990, 99_000_000),
            (1000, 100_000_000),
        ] {
            let Percentile::Value {
                upper_bound_ns,
                error_ns,
            } = histogram.percentile(permille)
            else {
                panic!("p{permille}");
            };
            assert!(
                upper_bound_ns >= true_ns,
                "p{permille}: {upper_bound_ns} is not below {true_ns}"
            );
            assert!(
                upper_bound_ns - true_ns <= error_ns,
                "p{permille}: within one bucket ({error_ns})"
            );
        }
    }

    #[test]
    fn samples_straddling_a_boundary_are_ranked_in_their_own_buckets() {
        let mut histogram = Histogram::new();
        // 90 samples just below 8 ms (last fine bucket) and 10 just above (first medium bucket).
        for _ in 0..90 {
            histogram.record(7_999_999);
        }
        for _ in 0..10 {
            histogram.record(8_000_000);
        }
        assert_eq!(
            value(&histogram, 900),
            8_000_000,
            "the 90th sample is the last in the fine bucket"
        );
        assert_eq!(
            value(&histogram, 910),
            8_250_000,
            "the 91st is the first in the medium bucket"
        );
    }

    #[test]
    fn all_samples_in_one_bucket_give_that_buckets_bound_for_every_percentile() {
        let mut histogram = Histogram::new();
        for _ in 0..1000 {
            histogram.record(3_010_000);
        }
        for permille in [1, 500, 950, 990, 1000] {
            assert_eq!(value(&histogram, permille), 3_025_000, "p{permille}");
        }
    }

    #[test]
    fn a_percentile_in_the_overflow_is_out_of_range_with_the_exact_maximum_never_200_ms() {
        let mut histogram = Histogram::new();
        for _ in 0..90 {
            histogram.record(1_000_000);
        }
        for _ in 0..10 {
            histogram.record(450_000_000);
        }
        assert_eq!(value(&histogram, 900), 1_025_000);
        assert_eq!(
            histogram.percentile(950),
            Percentile::OutOfRange {
                at_most_ns: 450_000_000
            },
            "p95 falls in the overflow"
        );
    }

    #[test]
    fn an_empty_histogram_has_no_value_never_zero() {
        let histogram = Histogram::new();
        assert_eq!(histogram.percentile(950), Percentile::NoValue);
        assert_eq!((histogram.count(), histogram.max_ns()), (0, None));
        let mut zero = Histogram::new();
        zero.record(0);
        assert_eq!(
            value(&zero, 950),
            25_000,
            "a zero sample is a sample, reported at its bucket's bound"
        );
    }

    #[test]
    fn a_saturated_counter_is_flagged_and_reset_empties_everything() {
        let mut histogram = Histogram::new();
        histogram.preload(1_000_000, u64::MAX);
        assert!(!histogram.saturated());
        histogram.record(1_000_000);
        assert!(histogram.saturated(), "the saturating add is flagged");
        assert_eq!(histogram.count(), u64::MAX);
        histogram.reset();
        assert_eq!(
            (histogram.count(), histogram.saturated(), histogram.max_ns()),
            (0, false, None)
        );
    }

    #[test]
    fn a_total_that_overflows_while_every_bucket_is_below_the_maximum_is_flagged() {
        // Two buckets each at u64::MAX / 2 + 1: neither saturated on its own, their sum does.
        let mut histogram = Histogram::new();
        histogram.preload(1_000, u64::MAX / 2 + 1);
        histogram.preload(500_000, u64::MAX / 2 + 1);
        assert_eq!(histogram.count(), u64::MAX, "the total saturates");
        assert!(
            histogram.saturated(),
            "and says so, though no single bucket did"
        );
    }

    #[test]
    fn every_frame_counter_that_can_saturate_is_covered_by_the_flag() {
        let counters: [fn(&mut FrameStats); 8] = [
            |stats| stats.invalid_gpu = u64::MAX,
            |stats| stats.not_presented = u64::MAX,
            |stats| stats.excluded_intervals = u64::MAX,
            |stats| stats.outstanding = u64::MAX,
            |stats| stats.skipped = u64::MAX,
            |stats| stats.invalid_marks = u64::MAX,
            |stats| stats.presented = u64::MAX,
            |stats| stats.frames = u64::MAX,
        ];
        assert!(!FrameStats::default().saturated());
        for (index, saturate) in counters.iter().enumerate() {
            let mut stats = FrameStats::default();
            saturate(&mut stats);
            assert!(stats.saturated(), "counter {index} is not covered");
        }
    }

    #[test]
    fn the_six_marks_give_contiguous_intervals_whose_components_sum_to_the_total_exactly() {
        let marks = FrameMarks {
            t0: 100,
            t1: 130,
            t2: 200,
            t3: 260,
            t4: 1260,
            t5: 1300,
        };
        let intervals = marks.intervals().expect("valid marks");
        assert_eq!(
            (
                intervals.ledger,
                intervals.build,
                intervals.acquire,
                intervals.encode
            ),
            (30, 70, 1000, (260 - 200) + (1300 - 1260))
        );
        assert_eq!(
            intervals.cpu_total,
            intervals.ledger + intervals.build + intervals.encode
        );
        assert_eq!(
            intervals.cpu_total,
            (marks.t3 - marks.t0) + (marks.t5 - marks.t4)
        );
        // Negative control: the acquisition (t4 - t3) is not in the total; adding it would make the total t5 - t0.
        assert_ne!(intervals.cpu_total, marks.t5 - marks.t0);
        assert_eq!(marks.t5 - marks.t0 - intervals.cpu_total, intervals.acquire);
    }

    #[test]
    fn a_missing_or_decreasing_mark_is_invalid() {
        let good = FrameMarks {
            t0: 1,
            t1: 2,
            t2: 3,
            t3: 4,
            t4: 5,
            t5: 6,
        };
        assert!(good.intervals().is_some());
        for broken in [
            FrameMarks { t0: 0, ..good },
            FrameMarks { t5: 0, ..good },
            FrameMarks { t2: 9, ..good },
            FrameMarks { t4: 3, ..good },
        ] {
            assert_eq!(broken.intervals(), None, "{broken:?}");
        }
    }

    #[test]
    fn the_total_is_the_p95_of_per_frame_sums_not_the_sum_of_component_p95s() {
        // Anti-correlated components: a large build pairs with a small encode and the reverse. Every frame's total is 10 ms.
        let mut stats = FrameStats::default();
        for frame in 0..100_u64 {
            let (build, encode) = if frame % 2 == 0 {
                (9_000_000, 1_000_000)
            } else {
                (1_000_000, 9_000_000)
            };
            let t0 = 1_000;
            let marks = FrameMarks {
                t0,
                t1: t0,
                t2: t0 + build,
                t3: t0 + build + encode / 2,
                t4: t0 + build + encode / 2,
                t5: t0 + build + encode,
            };
            stats.record_intervals(marks.intervals().expect("valid"));
        }
        let total = value(&stats.cpu_total, 950);
        let sum_of_parts = value(&stats.build, 950) + value(&stats.encode, 950);
        assert_eq!(
            total, 10_250_000,
            "every frame's total is 10 ms, at its bucket's upper bound"
        );
        assert!(
            total < sum_of_parts,
            "the total's p95 {total} is below the sum of the components' p95s {sum_of_parts}"
        );
        // Negative control: the wrong computation (sum of component p95s) is the larger value, 18.5 ms.
        assert_eq!(sum_of_parts, 18_500_000, "9.25 ms + 9.25 ms");
    }

    #[test]
    fn an_inverted_or_zero_start_gpu_pair_is_counted_invalid_not_sampled() {
        let mut stats = FrameStats::default();
        stats.record_gpu(100, 5_100);
        stats.record_gpu(5_000, 4_000);
        stats.record_gpu(0, 4_000);
        assert_eq!((stats.gpu.count(), stats.invalid_gpu), (1, 2));
        assert_eq!(stats.gpu.max_ns(), Some(5_000));
    }

    #[test]
    #[expect(
        clippy::cast_precision_loss,
        reason = "a mach tick count in a test is far below 2^52, where u64 to f64 is exact"
    )]
    fn the_clock_is_monotonic_and_a_seconds_value_converts_to_the_same_base() {
        let a = now_ns();
        let b = now_ns();
        assert!(a > 0 && b >= a);
        assert_eq!(seconds_to_ns(1.5), 1_500_000_000);
        assert_eq!(seconds_to_ns(0.0), 0);
        assert_eq!(seconds_to_ns(f64::NAN), 0);
        assert_eq!(seconds_to_ns(-3.0), 0);
        // `mach_absolute_time` scaled by the host time base is `CLOCK_UPTIME_RAW`: a mach time taken between two `now_ns` reads, in
        // seconds, lies between them.
        let mut timebase = Timebase { numer: 0, denom: 0 };
        // SAFETY: `timebase` is a valid writable struct for the call.
        assert_eq!(unsafe { mach_timebase_info(&raw mut timebase) }, 0);
        let before = now_ns();
        // SAFETY: no preconditions.
        let ticks = unsafe { mach_absolute_time() };
        let after = now_ns();
        let seconds =
            (ticks as f64 * f64::from(timebase.numer) / f64::from(timebase.denom)) / 1.0e9;
        let ns = seconds_to_ns(seconds);
        let slack = 1_000; // a mach tick rounds; allow a microsecond
        assert!(
            ns + slack >= before && ns <= after + slack,
            "{before} <= {ns} <= {after}"
        );
    }

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

    // --- the recorder ---

    /// Marks of a frame whose first mark is `t0`: ledger 10 us, build 20 us, encode before `nextDrawable` 5 us, acquire `hold`
    /// ns, encode after 15 us.
    fn frame_marks(t0: u64, hold: u64) -> FrameMarks {
        let t1 = t0 + 10_000;
        let t2 = t1 + 20_000;
        let t3 = t2 + 5_000;
        let t4 = t3 + hold;
        let t5 = t4 + 15_000;
        FrameMarks {
            t0,
            t1,
            t2,
            t3,
            t4,
            t5,
        }
    }

    fn counters() -> FrameCounters {
        FrameCounters {
            instances: 9,
            batches: 3,
            texture_binds: 3,
            checker: 2,
            ..FrameCounters::default()
        }
    }

    fn recorder_with(start: u64, end: u64) -> Recorder {
        let mut recorder = Recorder::default();
        recorder
            .windows
            .request(window(1, start, Some(end)))
            .unwrap();
        recorder.windows.boundary(0);
        recorder
    }

    #[test]
    fn a_committed_frame_gives_components_that_sum_to_the_total_without_the_acquisition() {
        let mut recorder = recorder_with(0, u64::MAX);
        let marks = frame_marks(1_000, 700_000);
        recorder.drawn(&marks, &counters());
        let stats = &recorder.windows.current().unwrap().stats;
        let intervals = marks.intervals().expect("valid");
        // ledger 10 + build 20 + encode (5 + 15) = 50 us; the 700 us acquisition is its own interval and not in the total.
        assert_eq!(intervals.cpu_total, 50_000);
        assert_eq!(
            intervals.ledger + intervals.build + intervals.encode,
            intervals.cpu_total
        );
        assert_eq!(intervals.acquire, 700_000);
        assert_eq!(stats.cpu_total.max_ns(), Some(50_000));
        assert_eq!(stats.acquire.max_ns(), Some(700_000));
        assert_eq!(
            (stats.frames, stats.draw_calls_max, stats.instances_max),
            (1, 3, 9)
        );
        assert_eq!(
            stats.outstanding, 2,
            "its GPU time and its presentation are still to come"
        );
    }

    #[test]
    fn a_blocked_acquisition_adds_to_acquire_only_by_the_timestamp_arithmetic() {
        // Two frames with the same work and a 10 ms hold in the second: only the acquire interval differs, by exactly the hold.
        let free = frame_marks(1_000, 100_000).intervals().unwrap();
        let held = frame_marks(1_000, 100_000 + 10_000_000)
            .intervals()
            .unwrap();
        assert_eq!(held.acquire - free.acquire, 10_000_000);
        assert_eq!(held.cpu_total, free.cpu_total);
        assert_eq!(
            (held.ledger, held.build, held.encode),
            (free.ledger, free.build, free.encode)
        );
        // The same through the recorder: the total's histogram holds the one figure twice.
        let mut recorder = recorder_with(0, u64::MAX);
        recorder.drawn(&frame_marks(1_000, 100_000), &counters());
        recorder.drawn(&frame_marks(2_000_000, 10_100_000), &counters());
        let stats = &recorder.windows.current().unwrap().stats;
        assert_eq!(stats.cpu_total.count(), 2);
        assert_eq!(stats.cpu_total.max_ns(), Some(50_000));
        assert_eq!(stats.acquire.max_ns(), Some(10_100_000));
    }

    #[test]
    fn skipped_frames_are_counted_and_not_sampled_and_invalid_marks_likewise() {
        let mut recorder = recorder_with(0, u64::MAX);
        recorder.not_drawn(500);
        recorder.not_drawn(600);
        let mut broken = frame_marks(1_000, 1_000);
        broken.t3 = broken.t2 - 1; // out of order
        recorder.drawn(&broken, &counters());
        let mut zero = frame_marks(1_000, 1_000);
        zero.t4 = 0;
        recorder.drawn(&zero, &counters());
        // No commit mark at all: nothing was committed that the native ring will ever report.
        let mut uncommitted = frame_marks(1_000, 1_000);
        uncommitted.t5 = 0;
        recorder.drawn(&uncommitted, &counters());
        let stats = &recorder.windows.current().unwrap().stats;
        assert_eq!(
            (stats.skipped, stats.invalid_marks, stats.frames),
            (2, 3, 0)
        );
        assert_eq!(stats.cpu_total.count(), 0, "nothing was sampled");
        assert_eq!(
            stats.outstanding, 4,
            "the two invalid-mark frames that were committed still owe their GPU time and presentation; the uncommitted one owes none"
        );
        // Their results arrive and are reconciled by the commit mark, so completion is not lost to the invalid marks.
        for frame in [&broken, &zero] {
            recorder.gpu(frame.t5, 10, 20);
            recorder.presented(frame.t5, 90_000);
        }
        let stats = &recorder.windows.current().unwrap().stats;
        assert_eq!(stats.outstanding, 0);
        assert_eq!(
            (recorder.totals.skipped, recorder.totals.invalid_marks),
            (2, 3)
        );
    }

    #[test]
    fn an_inverted_gpu_pair_is_counted_invalid_and_not_sampled_and_a_valid_one_is() {
        let mut recorder = recorder_with(0, u64::MAX);
        let marks = frame_marks(1_000, 1_000);
        recorder.drawn(&marks, &counters());
        recorder.drawn(&frame_marks(100_000, 1_000), &counters());
        recorder.gpu(marks.t5, 900, 400); // the end before the start
        recorder.gpu(marks.t5 + 99_000, 5_000, 25_000);
        let stats = &recorder.windows.current().unwrap().stats;
        assert_eq!(stats.invalid_gpu, 1);
        assert_eq!(stats.gpu.count(), 1);
        assert_eq!(stats.gpu.max_ns(), Some(20_000));
        assert_eq!(
            stats.outstanding, 2,
            "two frames owed two results each; two were consumed"
        );
    }

    #[test]
    fn presentation_intervals_are_recorded_for_presented_drawables_only_and_never_across_the_window()
     {
        let mut recorder = recorder_with(10_000_000, 40_000_000);
        // Presented times in ms: 8 (before the window), 12, 28, never shown, 30, 45 (after the window). Each belongs to a frame
        // committed at a time of its own (t5), which decides the window it counts in.
        for (frame_t5, presented) in [
            (9_000_000, 8_000_000),
            (13_000_000, 12_000_000),
            (29_000_000, 28_000_000),
            (31_000_000, 0),
            (32_000_000, 30_000_000),
            (41_000_000, 45_000_000),
        ] {
            recorder.presented(frame_t5, presented);
        }
        let stats = &recorder.windows.current().unwrap().stats;
        // Frames committed inside (13, 29, 31, 32 ms): presented 12 and 28 ms, never shown, 30 ms. Intervals by their own ends: 12 - 8
        // crosses the start (excluded), 28 - 12 = 16 ms and 30 - 28 = 2 ms are inside (the unshown drawable did not become the
        // previous one), and 45 - 30 crosses the end (excluded), though its frame was committed after the window.
        assert_eq!(stats.presented, 3);
        assert_eq!(stats.not_presented, 1);
        assert_eq!(stats.presentation_interval.count(), 2);
        assert_eq!(stats.presentation_interval.max_ns(), Some(16_000_000));
        assert_eq!(
            stats.excluded_intervals, 2,
            "12 ms - 8 ms crosses the start and 45 ms - 30 ms crosses the end"
        );
        // The whole run's totals hold every interval whatever the window: 4, 16, 2 and 15 ms.
        assert_eq!(recorder.totals.presentation_interval.count(), 4);
        assert_eq!(
            (recorder.totals.presented, recorder.totals.not_presented),
            (5, 1)
        );
    }

    #[test]
    fn an_interval_inside_the_window_counts_though_its_frame_was_committed_before_the_window() {
        let mut recorder = recorder_with(100, 200);
        // Two drawables shown at 105 and 110, both inside; their frames were committed at 90 and 95, before the window opens.
        recorder.presented(90, 105);
        recorder.presented(95, 110);
        let tracked = recorder.windows.current().unwrap();
        assert_eq!(
            tracked.stats.presentation_interval.count(),
            1,
            "the 5 ns interval has both ends inside"
        );
        assert_eq!(tracked.stats.presentation_interval.max_ns(), Some(5));
        assert_eq!(
            (tracked.stats.presented, tracked.outside),
            (0, 2),
            "the frames themselves are outside: their commit marks decide their own counts"
        );
        // And the other way: a frame committed inside whose two shown times lie before the window gives the window nothing.
        let mut early = recorder_with(100, 200);
        early.presented(150, 20);
        early.presented(160, 30);
        let tracked = early.windows.current().unwrap();
        assert_eq!(tracked.stats.presentation_interval.count(), 0);
        assert_eq!(
            tracked.stats.excluded_intervals, 0,
            "an interval that touches the window nowhere is not its exclusion"
        );
        assert_eq!(tracked.stats.presented, 2);
    }

    #[test]
    fn an_interval_across_two_kept_windows_is_excluded_from_both_and_sampled_in_neither() {
        let mut recorder = recorder_with(100, 200);
        recorder.windows.request(window(2, 200, Some(300))).unwrap();
        recorder.windows.boundary(250);
        recorder.presented(120, 190);
        recorder.presented(260, 210);
        let first = recorder.windows.get(1).unwrap();
        let second = recorder.windows.get(2).unwrap();
        assert_eq!(
            (
                first.stats.presentation_interval.count(),
                first.stats.excluded_intervals
            ),
            (0, 1),
            "190 -> 210 starts inside the first window and ends past it: it crosses its end"
        );
        assert_eq!(
            (
                second.stats.presentation_interval.count(),
                second.stats.excluded_intervals
            ),
            (0, 1)
        );
    }

    #[test]
    fn a_window_is_not_final_while_a_frame_committed_before_it_still_has_its_presentation_to_come()
    {
        // W = [100_000, 200_000). Two frames were committed before it (t5 = 52_000 and 61_000); their drawables are shown at 105_000
        // and 110_000, inside the window. Until those results are consumed the window cannot be final: they add an interval to it.
        let mut recorder = recorder_with(100_000, 200_000);
        let (first, second) = (frame_marks(1_000, 1_000), frame_marks(10_000, 1_000));
        assert!(first.t5 < 100_000 && second.t5 < 100_000);
        recorder.drawn(&first, &counters());
        recorder.drawn(&second, &counters());
        assert_eq!(
            recorder.windows.current().unwrap().stats.outstanding,
            0,
            "no frame of the window owes anything"
        );
        assert!(
            !recorder.accounted_through(200_000),
            "two presentations are to come"
        );
        recorder.presented(first.t5, 105_000);
        assert!(!recorder.accounted_through(200_000), "one is still to come");
        recorder.presented(second.t5, 110_000);
        assert!(
            recorder.accounted_through(200_000),
            "everything committed before the end was consumed"
        );
        let stats = &recorder.windows.current().unwrap().stats;
        assert_eq!(
            stats.presentation_interval.count(),
            1,
            "the 5,000 ns interval between the two shown times"
        );
        assert_eq!(stats.presentation_interval.max_ns(), Some(5_000));
        // A frame committed at or after the end is shown after it and cannot change the window.
        let mut late = frame_marks(250_000, 1_000);
        late.t5 = 300_000;
        recorder.drawn(&late, &counters());
        assert!(recorder.accounted_through(200_000));
    }

    #[test]
    fn a_late_commit_stamp_does_not_hide_a_presentation_that_can_already_have_happened() {
        // W = [100_000, 200_000); a drawable was shown at 150_000. A new command is submitted at 180_000 (the clock read before the
        // commit call) and shown at 190_000, but the render thread is descheduled after the submission and reads its commit mark
        // only at 210_000, after the window's end. The commit mark says "after the end"; the submission says it may already be shown.
        let mut recorder = recorder_with(100_000, 200_000);
        recorder.presented(1, 150_000);
        let late = FrameMarks {
            t0: 170_000,
            t1: 171_000,
            t2: 172_000,
            t3: 173_000,
            t4: 174_000,
            t5: 210_000,
        };
        recorder.drawn_at(&late, 180_000, &counters());
        assert!(
            !recorder.accounted_through(200_000),
            "submitted before the end: its presentation can fall inside the window whatever the commit mark says"
        );
        recorder.presented(late.t5, 190_000);
        assert!(recorder.accounted_through(200_000));
        let stats = &recorder.windows.current().unwrap().stats;
        assert_eq!(
            stats.presentation_interval.count(),
            1,
            "the 150_000 -> 190_000 interval is in the window"
        );
        assert_eq!(stats.presentation_interval.max_ns(), Some(40_000));
        // A frame submitted after the end cannot be shown inside the window.
        let after = FrameMarks {
            t0: 230_000,
            t1: 231_000,
            t2: 232_000,
            t3: 233_000,
            t4: 234_000,
            t5: 240_000,
        };
        recorder.drawn_at(&after, 236_000, &counters());
        assert!(recorder.accounted_through(200_000));
    }

    #[test]
    fn the_closing_interval_keeps_a_window_open_until_a_drawable_is_shown_after_its_end() {
        let mut recorder = recorder_with(100_000, 200_000);
        let window = window(1, 100_000, Some(200_000));
        assert!(
            !recorder.closing_pending(&window),
            "nothing was shown yet: no interval can close"
        );
        recorder.presented(1, 150_000);
        assert!(
            recorder.closing_pending(&window),
            "the latest shown drawable is inside the window"
        );
        recorder.presented(2, 260_000);
        let stats = &recorder.windows.current().unwrap().stats;
        assert_eq!(
            stats.excluded_intervals, 1,
            "150_000 -> 260_000 crosses the end"
        );
        assert!(
            !recorder.closing_pending(&window),
            "a drawable was shown after the end"
        );
        // The thread ending resolves it too: nothing will be shown again.
        let mut idle = recorder_with(100_000, 200_000);
        idle.presented(1, 150_000);
        assert!(idle.closing_pending(&window));
        idle.stop();
        assert!(!idle.closing_pending(&window));
        // A window whose latest shown drawable is before it has no closing interval to wait for.
        let mut before = recorder_with(100_000, 200_000);
        before.presented(1, 50_000);
        assert!(!before.closing_pending(&window));
    }

    #[test]
    fn a_closing_drawable_that_is_never_shown_does_not_resolve_the_closing_interval() {
        let mut recorder = recorder_with(100_000, 200_000);
        let window = window(1, 100_000, Some(200_000));
        recorder.presented(1, 150_000);
        assert!(recorder.closing_pending(&window));
        // A drawable after the end whose presented time is zero (never shown, as when the window is occluded) closes nothing.
        recorder.presented(2, 0);
        assert!(
            recorder.closing_pending(&window),
            "still pending: nothing was shown after the end"
        );
        assert_eq!(
            recorder.windows.current().unwrap().stats.excluded_intervals,
            0
        );
        recorder.stop();
        assert!(!recorder.closing_pending(&window));
    }

    #[test]
    fn exceeding_the_tracking_bound_is_explicit_insufficient_evidence_that_never_clears() {
        let mut recorder = recorder_with(100_000, 200_000);
        let mut committed = Vec::new();
        for index in 0..17_u64 {
            let marks = frame_marks(1_000 + index * 2, 1_000);
            committed.push(marks.t5);
            recorder.drawn(&marks, &counters());
        }
        assert!(recorder.evidence_lost());
        for t5 in committed {
            recorder.presented(t5, 150_000);
        }
        assert!(recorder.evidence_lost());
        assert!(
            !recorder.accounted_through(200_000),
            "the window can no longer be shown as final"
        );
    }

    #[test]
    fn the_progress_fence_holds_while_a_step_that_began_before_the_end_is_unrecorded() {
        let mut recorder = recorder_with(100, 200);
        assert!(recorder.accounted_through(200), "no step in flight");
        recorder.begin_step(150);
        assert!(
            !recorder.accounted_through(200),
            "a step began before the end and is not recorded"
        );
        recorder.end_step();
        assert!(recorder.accounted_through(200));
        recorder.begin_step(250);
        assert!(
            recorder.accounted_through(200),
            "a step that began after the end cannot own work before it"
        );
    }

    #[test]
    fn a_result_arriving_after_the_end_belongs_to_the_frame_that_was_committed_before_it() {
        let mut recorder = recorder_with(1_000, 100_000);
        let inside = frame_marks(2_000, 1_000); // t5 = 2_000 + 51_000 = 53_000, inside
        let after = frame_marks(120_000, 1_000); // committed after the end
        recorder.drawn(&inside, &counters());
        recorder.drawn(&after, &counters());
        recorder.windows.boundary(130_000);
        // The window has ended (the boundary ended it), and the GPU and presentation of the inside frame arrive now.
        recorder.gpu(inside.t5, 10, 30);
        recorder.presented(inside.t5, 90_000);
        let window = recorder.windows.get(1).expect("kept");
        assert_eq!(
            window.stats.frames, 1,
            "the frame committed after the end is outside"
        );
        assert_eq!(window.outside, 1);
        assert_eq!(
            window.stats.gpu.count(),
            1,
            "admitted to the window though it arrived after the end"
        );
        assert_eq!(window.stats.presented, 1);
        assert_eq!(window.stats.outstanding, 0);
    }

    // --- recording allocates nothing ---

    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    }

    /// Counts the allocations of the calling thread; every other thread's go through unchanged.
    struct Counting;

    // SAFETY: every method forwards to `System` unchanged; the count is a thread-local cell that allocates nothing.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
            // SAFETY: the caller's layout contract is passed on as it came.
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            // SAFETY: the pointer and layout are the ones `alloc` gave, passed on as they came.
            unsafe { System.dealloc(pointer, layout) }
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
            // SAFETY: the arguments are the ones the caller guaranteed, passed on as they came.
            unsafe { System.realloc(pointer, layout, size) }
        }
    }

    #[global_allocator]
    static COUNTING: Counting = Counting;

    #[test]
    fn recording_many_frames_and_results_allocates_nothing_after_warm_up() {
        let mut recorder = recorder_with(0, u64::MAX);
        // Warm-up: the first frame, GPU result and presentation (the lock, the windows and the first window's storage exist already).
        recorder.drawn(&frame_marks(1_000, 1_000), &counters());
        recorder.gpu(1_000, 10, 20);
        recorder.presented(1_000, 5_000);
        let before = ALLOCATIONS.with(Cell::get);
        for frame in 0..10_000_u64 {
            let t0 = 100_000 + frame * 20_000;
            let marks = frame_marks(t0, 1_000 + frame % 7);
            recorder.drawn(&marks, &counters());
            recorder.gpu(marks.t5, 100, 100 + frame % 13);
            recorder.presented(marks.t5, 5_000 + (frame + 1) * 16_667);
            if frame % 100 == 0 {
                recorder.not_drawn(t0);
            }
        }
        let after = ALLOCATIONS.with(Cell::get);
        assert_eq!(after - before, 0, "recording 10,000 frames allocated");
        assert_eq!(recorder.totals.frames, 10_001);
    }
}
