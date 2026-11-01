//! Bounded frame-timing instrumentation on Vulkan (milestone 5.5, issue 6, part B; design `docs/scene-design.md` §11.3).
//!
//! **A deliberate copy** of the platform-free parts of `ck-gpu-metal`'s `metrics.rs` (CK-129 r3), as `ck-window` already keeps a copy
//! of the window book: GPU crates may not depend on each other or on a window crate (§3), and moving the Metal types into a shared
//! crate would change files that select the five Metal window groups. The follow-up that removes the duplication is named in
//! CK-162's Constraints. The histogram, `Percentile`, the window book ([`Windows`]) and their tests are the Metal copy's, unchanged;
//! the application's agreement test compares the readings of all three copies. What differs is stated where it differs:
//!
//! - **The clock** is the caller's: the presenter takes it as a function (the application's, so one window is one interval on one
//!   clock) and this module reads no clock, so `ck-gpu-vulkan` stays free of platform code. Every figure is in nanoseconds.
//! - **The CPU marks follow the Vulkan frame's real order** (CK-157's frame step acquires the swap-chain image before it builds the
//!   instances; production is not reordered for the instrumentation): `t0` the latest frame taken from the slot, `t1` the driver's
//!   pass and any adoption returned, `t2` `vkAcquireNextImageKHR` called, `t3` it returned, `t4` the frame built (recording about to
//!   begin), `t5` `vkQueuePresentKHR` returned. **This is a stated difference from Metal**, whose built mark comes before its
//!   acquisition; the marks and their interval algebra ([`FrameMarks`]) are Vulkan's own.
//! - **GPU time** is two timestamp queries per frame, read after the frame's recorded proof ([`gpu_ns`]); where the queue family has
//!   no valid timestamp bits the reading says so and holds no zero sample.
//! - **Presentation intervals** come from `VK_GOOGLE_display_timing`, optional, through a bounded pending protocol ([`Recorder`]),
//!   admitted by the `t5` of the later frame of two consecutive resolved presents (an interval is a difference, so no conversion
//!   between the WSI's time base and the caller's clock is needed).

// ---------------------------------------------------------------------------------------------
// The histogram (the Metal copy's)
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
    /// **The CPU total** of each qualifying frame, one sample per frame: `(t2 - t0) + (t5 - t3)`, everything from the slot take to the
    /// hand-off to the presentation engine except the acquisition. CK-130's CPU figure is this histogram's p95, never a sum of the
    /// components'.
    pub cpu_total: Histogram,
    /// `t1 - t0`: the texture driver's pass and any boundary adoption.
    pub ledger: Histogram,
    /// `t4 - t3`: the frame built (resolution and instance writes), after the acquisition.
    pub build: Histogram,
    /// `(t2 - t1) + (t5 - t4)`: setup before the acquisition and everything after the build up to `vkQueuePresentKHR`'s return
    /// (recording, submission and the present call).
    pub encode: Histogram,
    /// `t3 - t2`: `vkAcquireNextImageKHR`.
    pub acquire: Histogram,
    /// The two timestamp queries of a frame's command buffer, in nanoseconds.
    pub gpu: Histogram,
    /// The difference between the actual present times of two consecutive presents.
    pub presentation_interval: Histogram,
    /// Frames handed to the presentation engine with valid marks.
    pub frames: u64,
    /// Frames that presented nothing (no image, an acquisition timeout, a skipped or abandoned frame).
    pub skipped: u64,
    /// Frames with a missing or decreasing mark.
    pub invalid_marks: u64,
    /// GPU times that were zero, wrapped or inverted.
    pub invalid_gpu: u64,
    /// Presents whose timing resolved with an actual present time.
    pub presented: u64,
    /// Presents whose timing resolved with no present time (never shown).
    pub not_presented: u64,
    /// Presents whose timing never resolved within the protocol's bounds (final: no interval needing them is sampled).
    pub presentation_expired: u64,
    /// Instances drawn: the sum over frames.
    pub instances_sum: u64,
    /// The most instances one frame drew.
    pub instances_max: u64,
    /// Scene draw calls: the sum over frames (one per bindless frame).
    pub draw_calls_sum: u64,
    /// The most scene draw calls in one frame.
    pub draw_calls_max: u64,
    /// Table binds (one per bindless frame), summed.
    pub texture_binds: u64,
    /// Checker instances, summed.
    pub checker_instances: u64,
    /// Frames submitted in the window whose GPU result had not yet been read when the snapshot was taken.
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

    /// Counts a frame that presented nothing.
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
                self.presentation_expired,
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
// One frame's marks (Vulkan's own algebra)
// ---------------------------------------------------------------------------------------------

/// The six marks of one frame step, in the caller's clock nanoseconds, in the Vulkan frame's real order: the latest frame taken from
/// the slot (`t0`), the texture driver's pass and any boundary adoption returned (`t1`), `vkAcquireNextImageKHR` called (`t2`), it
/// returned (`t3`), the frame built and recording about to begin (`t4`), and `vkQueuePresentKHR` returned (`t5`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameMarks {
    /// The latest frame taken from the slot.
    pub t0: u64,
    /// The driver's pass and any adoption returned.
    pub t1: u64,
    /// `vkAcquireNextImageKHR` called.
    pub t2: u64,
    /// `vkAcquireNextImageKHR` returned.
    pub t3: u64,
    /// The frame built (resolution and instance writes); recording about to begin.
    pub t4: u64,
    /// `vkQueuePresentKHR` returned: the frame has been handed off.
    pub t5: u64,
}

/// The contiguous intervals of a frame, from valid marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameIntervals {
    /// `t1 - t0`.
    pub ledger: u64,
    /// `t4 - t3`.
    pub build: u64,
    /// `t3 - t2`.
    pub acquire: u64,
    /// `(t2 - t1) + (t5 - t4)`.
    pub encode: u64,
    /// `ledger + build + encode` = `(t2 - t0) + (t5 - t3)`: **everything except the acquisition**.
    pub cpu_total: u64,
}

impl FrameMarks {
    /// The intervals of this frame, `None` when a mark is missing (zero) or the marks decrease (the frame is then counted invalid). A
    /// sequence in Metal's order, with the frame built before the acquisition, decreases at the build mark and is invalid.
    #[must_use]
    pub fn intervals(&self) -> Option<FrameIntervals> {
        let marks = [self.t0, self.t1, self.t2, self.t3, self.t4, self.t5];
        if marks.contains(&0) || marks.windows(2).any(|pair| pair[0] > pair[1]) {
            return None;
        }
        let ledger = self.t1 - self.t0;
        let acquire = self.t3 - self.t2;
        let build = self.t4 - self.t3;
        let encode = (self.t2 - self.t1) + (self.t5 - self.t4);
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

    /// Records a frame's GPU time: a sample, or an invalid pair counted and not sampled.
    pub fn record_gpu(&mut self, nanoseconds: Option<u64>) {
        match nanoseconds {
            Some(nanoseconds) => self.gpu.record(nanoseconds),
            None => Self::add(&mut self.invalid_gpu, 1),
        }
    }
}

/// A frame's GPU time from its two timestamp queries (`start`, `end` in ticks), `valid_bits` the queue family's
/// `timestampValidBits` (1 to 64) and `period` the device's `timestampPeriod` (nanoseconds per tick): **ticks first, then
/// nanoseconds.** The delta in ticks is `(end - start) mod 2^b` (plain subtraction when `b = 64`); the bits above `b` are undefined
/// and masked. A delta of zero, a delta of `2^(b-1)` or more (an end before its start, including the inverted pair), or `b = 64`
/// with `end < start` is **invalid** (`None`: counted, not sampled). Otherwise `round(delta × period)` nanoseconds, computed in
/// `f64` and saturated to `u64`, so a non-unit period is exact to rounding. `valid_bits = 0` has no timestamps at all: the caller
/// reports "unavailable" and never calls this.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "ticks times a period far below 2^53 nanoseconds in any real frame; the saturating cast is the stated rule"
)]
pub fn gpu_ns(start: u64, end: u64, valid_bits: u32, period: f32) -> Option<u64> {
    if valid_bits == 0 || !period.is_finite() || period <= 0.0 {
        return None;
    }
    let delta = if valid_bits >= 64 {
        end.checked_sub(start)?
    } else {
        let mask = (1_u64 << valid_bits) - 1;
        (end & mask).wrapping_sub(start & mask) & mask
    };
    let half = 1_u64 << (valid_bits.min(64) - 1);
    if delta == 0 || delta >= half {
        return None;
    }
    let nanoseconds = (delta as f64 * f64::from(period)).round();
    Some(if nanoseconds >= u64::MAX as f64 {
        u64::MAX
    } else {
        nanoseconds as u64
    })
}

// ---------------------------------------------------------------------------------------------
// The recorder
// ---------------------------------------------------------------------------------------------

/// The pending presentation records the recorder keeps: the ring of 16.
pub(crate) const PENDING_PRESENTATIONS: usize = 16;
/// A pending record expires when this many **later** presents have resolved without it.
const EXPIRES_AFTER_LATER_RESOLVED: u32 = 8;
/// A pending record expires this long after its own present fence was seen signalled.
const EXPIRES_AFTER_FENCE_NS: u64 = 1_000_000_000;
/// The resolved presents remembered for pairing: a pair is formed by whichever of its two resolves second.
const REMEMBERED_RESOLVED: usize = 16;

/// A present whose timing feedback has not arrived: its `presentID` (the frame's number), its hand-off mark `t5` (what the result is
/// settled by), how many later presents have resolved, and when its present fence was seen signalled (zero: not yet).
#[derive(Clone, Copy, Default)]
struct PendingPresent {
    id: u32,
    t5: u64,
    later_resolved: u32,
    fence_seen: u64,
}

/// A present whose timing resolved: kept so that a neighbour that resolves later forms its interval.
#[derive(Clone, Copy, Default)]
struct ResolvedPresent {
    id: u32,
    t5: u64,
    actual: u64,
}

/// The presenter's measurement windows and whole-run totals, with the rules that put a finished frame, a timestamp result or a
/// presentation feedback record in them. It lives behind the presenter's one lock and is written by the render thread only; it holds
/// fixed storage (two windows, each seven histograms, two rings of 16), so recording allocates nothing and is O(1).
///
/// **The presentation protocol (bounded).** Feedback is asynchronous: the presentation engine returns timings when it has them and
/// keeps only a bounded history. Each present with timing adds a pending record to a ring of [`PENDING_PRESENTATIONS`]; a record
/// leaves the ring only by being **resolved** (its actual present time read) or **expired**: 8 later presents resolved, or 1 s after
/// its own present fence was seen signalled, or the ring is full and a new record must enter (the oldest expires). An expired record
/// is final: its frame counts as presentation expired in its window and neither interval that needs it is sampled. An interval is
/// formed only from two **consecutive** `presentID`s both resolved (a gap forms none), the later one's time minus the earlier one's,
/// and is admitted by the `t5` of its later frame. A window's reading stays not final while any pending record handed off before its
/// end is unresolved and unexpired, so a temporarily empty answer never finalises a window.
#[derive(Default)]
pub(crate) struct Recorder {
    /// The pending, current and last ended windows.
    pub(crate) windows: Windows<FrameStats>,
    /// The whole run's figures, every sample whatever its window (the end-of-run record's totals).
    pub(crate) totals: FrameStats,
    /// Why GPU time is unavailable (the queue family has no valid timestamp bits), if it is.
    pub(crate) gpu_unavailable: Option<&'static str>,
    /// Why presentation intervals are unavailable (the extension is not advertised), if they are.
    pub(crate) presentation_unavailable: Option<&'static str>,
    /// The first mark of the frame step in flight, from when the render thread started it until it has recorded it: the owner's
    /// progress fence.
    in_flight_since: Option<u64>,
    pending: [PendingPresent; PENDING_PRESENTATIONS],
    pending_len: usize,
    resolved: [ResolvedPresent; REMEMBERED_RESOLVED],
    resolved_next: usize,
    resolved_len: usize,
    /// The render thread has ended: no frame will be presented or resolved again.
    stopped: bool,
}

impl Recorder {
    /// A recorder that says why GPU time or presentation intervals are unavailable, if they are.
    #[cfg(test)]
    pub(crate) fn new(
        gpu_unavailable: Option<&'static str>,
        presentation_unavailable: Option<&'static str>,
    ) -> Self {
        Self {
            gpu_unavailable,
            presentation_unavailable,
            ..Self::default()
        }
    }

    /// The render thread starts a frame step whose first mark is `t0`.
    pub(crate) fn begin_step(&mut self, t0: u64) {
        self.in_flight_since = Some(t0);
    }

    /// The render thread has recorded the step (or found nothing to record).
    pub(crate) fn end_step(&mut self) {
        self.in_flight_since = None;
    }

    /// Whether this owner has accounted for everything that can still change a window ending at `end`: no step that began before it
    /// is unrecorded, and no present handed off before it still has its feedback to come (unresolved and unexpired: it resolves or
    /// expires within a second of its fence). A present handed off at or after `end` cannot matter.
    pub(crate) fn accounted_through(&self, end: u64) -> bool {
        self.in_flight_since.is_none_or(|since| since >= end)
            && !self.pending[..self.pending_len]
                .iter()
                .any(|pending| pending.t5 < end)
    }

    /// Whether any present is still pending (idle polling continues while one is).
    pub(crate) fn has_pending(&self) -> bool {
        self.pending_len != 0
    }

    /// A frame handed off with valid or invalid marks. With valid marks its intervals (the CPU total once, as that frame's own sum)
    /// and counts, and one result still to come when GPU time is available; with invalid marks it is counted and not sampled. A
    /// frame presented with timing (`present_id`) enters the pending ring. Admitted by the frame's hand-off mark `t5`.
    pub(crate) fn drawn(
        &mut self,
        marks: &FrameMarks,
        present_id: Option<u32>,
        counters: &crate::batch::FrameCounters,
        table_binds: u64,
    ) {
        let owed_gpu = u64::from(self.gpu_unavailable.is_none() && marks.t5 != 0);
        let Some(intervals) = marks.intervals() else {
            self.totals.count_invalid_marks();
            let at = if marks.t5 != 0 { marks.t5 } else { marks.t0 };
            if let Some(tracked) = self.windows.admit(at) {
                tracked.stats.count_invalid_marks();
                tracked.stats.outstanding = tracked.stats.outstanding.saturating_add(owed_gpu);
            }
            if marks.t5 != 0
                && let Some(id) = present_id
            {
                self.owe_presentation(id, marks.t5);
            }
            return;
        };
        let (instances, calls, binds, checker) = (
            counters.instances,
            counters.batches,
            table_binds,
            counters.checker,
        );
        self.totals.record_intervals(intervals);
        self.totals.count_frame(instances, calls, binds, checker);
        if let Some(tracked) = self.windows.admit(marks.t5) {
            tracked.stats.record_intervals(intervals);
            tracked.stats.count_frame(instances, calls, binds, checker);
            tracked.stats.outstanding = tracked.stats.outstanding.saturating_add(owed_gpu);
        }
        if let Some(id) = present_id {
            self.owe_presentation(id, marks.t5);
        }
    }

    /// A frame step that presented nothing (skipped or abandoned): counted, admitted by its first mark, never sampled.
    pub(crate) fn not_drawn(&mut self, t0: u64) {
        self.totals.count_skipped();
        if let Some(tracked) = self.windows.admit(t0) {
            tracked.stats.count_skipped();
        }
    }

    /// A frame's GPU time (`None`: invalid, counted and not sampled), belonging to the frame handed off at `t5`.
    pub(crate) fn gpu(&mut self, t5: u64, nanoseconds: Option<u64>) {
        self.totals.record_gpu(nanoseconds);
        if let Some(tracked) = self.windows.admit(t5) {
            tracked.stats.record_gpu(nanoseconds);
            tracked.stats.outstanding = tracked.stats.outstanding.saturating_sub(1);
        }
    }

    /// The present with `presentID` `id`, handed off at `t5`, was presented with timing: its feedback is to come.
    fn owe_presentation(&mut self, id: u32, t5: u64) {
        if self.pending_len == PENDING_PRESENTATIONS {
            // The ring is full: the oldest (the first, the ring being in arrival order) expires to make room.
            self.expire_at(0);
        }
        self.pending[self.pending_len] = PendingPresent {
            id,
            t5,
            later_resolved: 0,
            fence_seen: 0,
        };
        self.pending_len += 1;
    }

    /// The present fence of the frame whose `presentID` is `id` was seen signalled at `now`: its record expires a second later.
    pub(crate) fn fence_seen(&mut self, id: u32, now: u64) {
        if let Some(pending) = self.pending[..self.pending_len]
            .iter_mut()
            .find(|pending| pending.id == id)
        {
            pending.fence_seen = pending.fence_seen.max(now.max(1));
        }
    }

    /// Expires the records whose fence was seen a second ago or more.
    pub(crate) fn expire_due(&mut self, now: u64) {
        let mut index = 0;
        while index < self.pending_len {
            let pending = self.pending[index];
            if pending.fence_seen != 0
                && now.saturating_sub(pending.fence_seen) >= EXPIRES_AFTER_FENCE_NS
            {
                self.expire_at(index);
            } else {
                index += 1;
            }
        }
    }

    /// Removes the pending record at `index` (keeping arrival order) as expired: final, counted in its frame's window.
    fn expire_at(&mut self, index: usize) {
        let expired = self.pending[index];
        self.pending.copy_within(index + 1..self.pending_len, index);
        self.pending_len -= 1;
        FrameStats::add(&mut self.totals.presentation_expired, 1);
        if let Some(tracked) = self.windows.admit(expired.t5) {
            FrameStats::add(&mut tracked.stats.presentation_expired, 1);
        }
    }

    /// The feedback for the present `id` arrived: its actual present time on the presentation engine's own clock (zero: never shown). A record that is not pending (it expired, or was never owed) is ignored. The present is counted presented or not
    /// in its frame's window; with a resolved neighbour on either side, the interval to it is sampled and admitted by the `t5` of the
    /// later of the two frames.
    pub(crate) fn presentation_resolved(&mut self, id: u32, actual_ns: u64) {
        let Some(index) = self.pending[..self.pending_len]
            .iter()
            .position(|pending| pending.id == id)
        else {
            return;
        };
        let resolved = self.pending[index];
        self.pending.copy_within(index + 1..self.pending_len, index);
        self.pending_len -= 1;
        let (presented, not_presented) = if actual_ns == 0 { (0, 1) } else { (1, 0) };
        FrameStats::add(&mut self.totals.presented, presented);
        FrameStats::add(&mut self.totals.not_presented, not_presented);
        if let Some(tracked) = self.windows.admit(resolved.t5) {
            FrameStats::add(&mut tracked.stats.presented, presented);
            FrameStats::add(&mut tracked.stats.not_presented, not_presented);
        }
        if actual_ns != 0 {
            for neighbour in [id.wrapping_sub(1), id.wrapping_add(1)] {
                if let Some(other) = self.remembered(neighbour) {
                    let (earlier, later) = if neighbour == id.wrapping_sub(1) {
                        (other, resolved_present(resolved, actual_ns))
                    } else {
                        (resolved_present(resolved, actual_ns), other)
                    };
                    if later.actual >= earlier.actual {
                        let interval = later.actual - earlier.actual;
                        self.totals.presentation_interval.record(interval);
                        if let Some(tracked) = self.windows.admit(later.t5) {
                            tracked.stats.presentation_interval.record(interval);
                        }
                    }
                }
            }
            self.remember(resolved_present(resolved, actual_ns));
        }
        // Every older pending record has a later present resolved: the eighth expires it.
        let mut index = 0;
        while index < self.pending_len {
            if wrapping_older(self.pending[index].id, id) {
                self.pending[index].later_resolved += 1;
                if self.pending[index].later_resolved >= EXPIRES_AFTER_LATER_RESOLVED {
                    self.expire_at(index);
                    continue;
                }
            }
            index += 1;
        }
    }

    /// The resolved present `id`, if it is among the remembered.
    fn remembered(&self, id: u32) -> Option<ResolvedPresent> {
        self.resolved[..self.resolved_len]
            .iter()
            .copied()
            .find(|present| present.id == id)
    }

    fn remember(&mut self, present: ResolvedPresent) {
        self.resolved[self.resolved_next] = present;
        self.resolved_next = (self.resolved_next + 1) % REMEMBERED_RESOLVED;
        self.resolved_len = (self.resolved_len + 1).min(REMEMBERED_RESOLVED);
    }

    /// The render thread has ended: nothing will be presented or resolved again. The pending records that remain will never
    /// resolve: they expire now, final and counted.
    pub(crate) fn stop(&mut self) {
        self.stopped = true;
        while self.pending_len != 0 {
            self.expire_at(0);
        }
    }

    /// Whether the thread has ended.
    #[cfg(test)]
    pub(crate) fn stopped(&self) -> bool {
        self.stopped
    }
}

fn resolved_present(pending: PendingPresent, actual: u64) -> ResolvedPresent {
    ResolvedPresent {
        id: pending.id,
        t5: pending.t5,
        actual,
    }
}

/// Whether present `a` was made before present `b` (the ids are a wrapping counter of presents in order).
fn wrapping_older(a: u32, b: u32) -> bool {
    a != b && b.wrapping_sub(a) < u32::MAX / 2
}

// ---------------------------------------------------------------------------------------------
// The presenter's shared state
// ---------------------------------------------------------------------------------------------

/// The clock the caller gives the presenter: nanoseconds, monotonic, never zero. The application passes its own, so one window is
/// one interval on one clock for every owner.
pub type MetricsClock = std::sync::Arc<dyn Fn() -> u64 + Send + Sync>;

/// A clock for a presenter started without one: nanoseconds since the first read in the process, plus one (never zero).
pub(crate) fn default_clock() -> MetricsClock {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    std::sync::Arc::new(|| {
        let started = *START.get_or_init(std::time::Instant::now);
        u64::try_from(started.elapsed().as_nanos())
            .unwrap_or(u64::MAX)
            .saturating_add(1)
    })
}

/// What the presenter shares with its handle for metrics: the recorder under its one lock, the condition that says it changed, the
/// clock, and the flag that says a window was requested and not yet adopted.
pub(crate) struct MetricsShared {
    pub(crate) recorder: std::sync::Mutex<Recorder>,
    pub(crate) changed: std::sync::Condvar,
    pub(crate) clock: MetricsClock,
    /// A window was requested: the render thread adopts it at its next boundary.
    pub(crate) check: std::sync::atomic::AtomicBool,
}

impl MetricsShared {
    pub(crate) fn new(clock: Option<MetricsClock>) -> Self {
        Self {
            recorder: std::sync::Mutex::new(Recorder::default()),
            changed: std::sync::Condvar::new(),
            clock: clock.unwrap_or_else(default_clock),
            check: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The recorder, locked (a poisoned lock is read through: the recorder is plain data).
    pub(crate) fn recorder(&self) -> std::sync::MutexGuard<'_, Recorder> {
        self.recorder
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn now(&self) -> u64 {
        (self.clock)()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::FrameCounters;

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
            |stats| stats.presentation_expired = u64::MAX,
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

    // --- windows (the Metal copy's tests, unchanged) ---

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

    // --- the marks (Vulkan's order) ---

    /// Marks of a frame whose first mark is `t0`: ledger 10 us, setup before the acquire 5 us, acquire `hold` ns, build 20 us,
    /// encode after the build 15 us.
    fn frame_marks(t0: u64, hold: u64) -> FrameMarks {
        let t1 = t0 + 10_000;
        let t2 = t1 + 5_000;
        let t3 = t2 + hold;
        let t4 = t3 + 20_000;
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

    #[test]
    fn the_six_marks_in_the_vulkan_order_give_contiguous_intervals_and_the_total_excludes_the_acquire_exactly()
     {
        let marks = frame_marks(1_000_000, 123_456);
        let intervals = marks.intervals().expect("valid");
        assert_eq!(intervals.ledger, 10_000, "t1 - t0");
        assert_eq!(intervals.acquire, 123_456, "t3 - t2");
        assert_eq!(intervals.build, 20_000, "t4 - t3");
        assert_eq!(intervals.encode, 5_000 + 15_000, "(t2 - t1) + (t5 - t4)");
        assert_eq!(
            intervals.cpu_total,
            (marks.t2 - marks.t0) + (marks.t5 - marks.t3),
            "everything from the slot take to the hand-off except the acquire"
        );
        assert_eq!(
            intervals.ledger + intervals.build + intervals.encode + intervals.acquire,
            marks.t5 - marks.t0,
            "the components and the acquire sum to t5 - t0 exactly"
        );
        assert_eq!(intervals.cpu_total + intervals.acquire, marks.t5 - marks.t0);
    }

    #[test]
    fn a_missing_or_decreasing_mark_is_invalid_and_so_is_metals_order() {
        let valid = frame_marks(1_000, 100);
        assert!(valid.intervals().is_some());
        for index in 0..6 {
            let mut marks = [valid.t0, valid.t1, valid.t2, valid.t3, valid.t4, valid.t5];
            marks[index] = 0;
            let missing = FrameMarks {
                t0: marks[0],
                t1: marks[1],
                t2: marks[2],
                t3: marks[3],
                t4: marks[4],
                t5: marks[5],
            };
            assert!(missing.intervals().is_none(), "mark {index} missing");
        }
        // Metal's order: the frame is built (a mark) before the acquisition. Supplied here as the build mark before the acquire
        // returned, it decreases and is invalid, counted and not sampled.
        let metals = FrameMarks {
            t0: 1_000,
            t1: 2_000,
            t2: 3_000,
            t3: 9_000,
            t4: 4_000,
            t5: 10_000,
        };
        assert!(metals.intervals().is_none());
        let mut recorder = Recorder::default();
        recorder
            .windows
            .request(MetricsWindow {
                id: 1,
                start: 0,
                end: None,
            })
            .expect("requested");
        recorder.windows.boundary(0);
        recorder.drawn(&metals, None, &FrameCounters::default(), 1);
        assert_eq!(recorder.totals.invalid_marks, 1);
        assert_eq!(recorder.totals.frames, 0, "not sampled");
        assert_eq!(recorder.totals.cpu_total.count(), 0);
    }

    #[test]
    fn the_total_is_the_p95_of_per_frame_sums_not_the_sum_of_component_p95s() {
        // Anti-correlated components: in each frame one component is large and the other small, so the per-frame total is
        // constant while each component's p95 is large.
        let mut stats = FrameStats::default();
        for frame in 0..100_u64 {
            let (ledger, build) = if frame % 2 == 0 {
                (1_000_000, 100_000)
            } else {
                (100_000, 1_000_000)
            };
            let t0 = 10_000_000 * (frame + 1);
            let marks = FrameMarks {
                t0,
                t1: t0 + ledger,
                t2: t0 + ledger,
                t3: t0 + ledger,
                t4: t0 + ledger + build,
                t5: t0 + ledger + build,
            };
            stats.record_intervals(marks.intervals().expect("valid"));
        }
        let p95 = |histogram: &Histogram| value(histogram, 950);
        let total = p95(&stats.cpu_total);
        assert!(
            total < p95(&stats.ledger) + p95(&stats.build),
            "the total's p95 {total} is below the sum of the components' p95s"
        );
    }

    // --- GPU time ---

    #[test]
    fn a_supplied_tick_pair_is_converted_ticks_first_and_invalid_pairs_are_counted_not_sampled() {
        // 64 valid bits, a unit period: the plain difference.
        assert_eq!(gpu_ns(100, 600, 64, 1.0), Some(500));
        // An inverted pair and a zero delta are invalid, with 64 bits and with fewer.
        assert_eq!(gpu_ns(600, 100, 64, 1.0), None);
        assert_eq!(gpu_ns(77, 77, 64, 1.0), None);
        assert_eq!(
            gpu_ns(600, 100, 36, 1.0),
            None,
            "an end 'before' its start is a delta of half the range or more"
        );
        assert_eq!(gpu_ns(5, 5, 36, 1.0), None);
        // 36 valid bits: the counter wrapped between the two reads; the delta is taken modulo 2^36 and the undefined bits above
        // are masked.
        let wrap = 1_u64 << 36;
        assert_eq!(gpu_ns(wrap - 10, 15, 36, 1.0), Some(25), "wrapped");
        assert_eq!(
            gpu_ns((7 << 40) | (wrap - 10), (3 << 44) | 15, 36, 1.0),
            Some(25),
            "high bits masked"
        );
        // A delta of 2^(b-1) or more is invalid; one tick less is valid.
        assert_eq!(gpu_ns(0, 1 << 35, 36, 1.0), None);
        assert_eq!(gpu_ns(0, (1 << 35) - 1, 36, 1.0), Some((1 << 35) - 1));
        // A non-unit period scales exactly to rounding: 52.08 ns per tick.
        assert_eq!(gpu_ns(0, 1_000, 64, 52.08), Some(52_080));
        assert_eq!(gpu_ns(0, 3, 64, 52.08), Some(156));
        // No valid bits at all: no sample (the caller reports unavailable instead).
        assert_eq!(gpu_ns(0, 10, 0, 1.0), None);
        // Saturated, not wrapped.
        assert_eq!(gpu_ns(0, u64::MAX / 2, 64, 1.0e10), Some(u64::MAX));
    }

    // --- the recorder ---

    fn counters() -> FrameCounters {
        FrameCounters {
            instances: 100,
            batches: 1,
            checker: 3,
            ..FrameCounters::default()
        }
    }

    fn recorder_with(start: u64, end: u64) -> Recorder {
        let mut recorder = Recorder::default();
        recorder
            .windows
            .request(MetricsWindow {
                id: 1,
                start,
                end: Some(end),
            })
            .expect("requested");
        recorder.windows.boundary(start.saturating_sub(1));
        recorder
    }

    fn current(recorder: &Recorder) -> FrameStats {
        recorder.windows.get(1).expect("the window").stats.clone()
    }

    #[test]
    fn a_handed_off_frame_gives_components_that_sum_to_the_total_without_the_acquire() {
        let mut recorder = recorder_with(1_000, 100_000_000);
        let marks = frame_marks(10_000, 700_000);
        recorder.drawn(&marks, None, &counters(), 1);
        let stats = current(&recorder);
        assert_eq!(stats.frames, 1);
        assert_eq!(stats.cpu_total.count(), 1);
        assert_eq!(stats.instances_sum, 100);
        assert_eq!(stats.draw_calls_sum, 1);
        assert_eq!(stats.texture_binds, 1);
        assert_eq!(stats.checker_instances, 3);
        assert_eq!(stats.outstanding, 1, "its GPU time is still to come");
        recorder.gpu(marks.t5, Some(2_000_000));
        let stats = current(&recorder);
        assert_eq!(stats.gpu.count(), 1);
        assert_eq!(stats.outstanding, 0);
        // A frame whose GPU time is invalid is counted and not sampled.
        recorder.drawn(&frame_marks(20_000, 100), None, &counters(), 1);
        recorder.gpu(frame_marks(20_000, 100).t5, None);
        let stats = current(&recorder);
        assert_eq!((stats.gpu.count(), stats.invalid_gpu), (1, 1));
    }

    #[test]
    fn without_valid_timestamp_bits_no_gpu_sample_is_owed_and_none_is_zero() {
        let mut recorder = Recorder::new(Some("timestampValidBits 0"), None);
        recorder
            .windows
            .request(MetricsWindow {
                id: 1,
                start: 0,
                end: None,
            })
            .expect("requested");
        recorder.windows.boundary(0);
        recorder.drawn(&frame_marks(10_000, 100), None, &counters(), 1);
        let stats = current(&recorder);
        assert_eq!((stats.frames, stats.outstanding), (1, 0), "nothing is owed");
        assert_eq!(
            (stats.gpu.count(), stats.invalid_gpu),
            (0, 0),
            "and no zero sample exists"
        );
        assert_eq!(recorder.gpu_unavailable, Some("timestampValidBits 0"));
    }

    #[test]
    fn skipped_frames_are_counted_and_not_sampled() {
        let mut recorder = recorder_with(1_000, 100_000_000);
        recorder.not_drawn(5_000);
        recorder.not_drawn(6_000);
        assert_eq!(recorder.totals.skipped, 2);
        assert_eq!(current(&recorder).skipped, 2);
        assert_eq!(current(&recorder).cpu_total.count(), 0);
    }

    // --- presentation feedback (deterministic, supplied) ---

    /// A recorder with presents `ids` handed off at `t5 = 10_000 * (index + 1)` and owing their feedback, the first window open
    /// from 0 to `end`.
    fn presenting(ids: &[u32], end: u64) -> Recorder {
        let mut recorder = recorder_with(1, end);
        for (index, &id) in ids.iter().enumerate() {
            let mut marks = frame_marks(10_000 * (index as u64 + 1), 100);
            marks.t5 = marks.t4 + 15_000;
            // Shift the whole frame so t5 is exactly the stated value.
            let shift = 10_000 * (index as u64 + 1) + 200_000 - marks.t5;
            let marks = FrameMarks {
                t0: marks.t0 + shift,
                t1: marks.t1 + shift,
                t2: marks.t2 + shift,
                t3: marks.t3 + shift,
                t4: marks.t4 + shift,
                t5: marks.t5 + shift,
            };
            recorder.drawn(&marks, Some(id), &counters(), 1);
        }
        recorder
    }

    #[test]
    fn a_temporarily_empty_answer_leaves_the_record_pending_and_the_window_not_final() {
        let mut recorder = presenting(&[1, 2], 10_000_000);
        // Nothing has been reported yet: the window is not accounted for, however far the clock is past its end.
        assert!(!recorder.accounted_through(10_000_000));
        recorder.presentation_resolved(1, 5_000_000);
        assert!(
            !recorder.accounted_through(10_000_000),
            "the second is still pending"
        );
        // The next poll resolves it: the interval is admitted by the later frame's t5.
        recorder.presentation_resolved(2, 21_000_000);
        assert!(recorder.accounted_through(10_000_000));
        let stats = current(&recorder);
        assert_eq!(stats.presentation_interval.count(), 1);
        assert_eq!(stats.presented, 2);
        assert_eq!(recorder.totals.presentation_interval.count(), 1);
        let interval = value(&stats.presentation_interval, 500);
        assert!(
            (16_000_000..=16_000_000 + 250_000).contains(&interval),
            "the interval is the later actual time minus the earlier (16 ms), on its bucket's bound: {interval}"
        );
    }

    #[test]
    fn an_interval_is_admitted_by_the_later_frames_hand_off_not_by_its_present_times() {
        // Present 1 is handed off inside the window; present 2 after its end. The interval belongs to the later frame, so it is
        // outside this window.
        let mut recorder = recorder_with(1, 220_000);
        let early = FrameMarks {
            t0: 100_000,
            t1: 100_100,
            t2: 100_200,
            t3: 100_300,
            t4: 100_400,
            t5: 200_000,
        };
        let late = FrameMarks {
            t0: 300_000,
            t1: 300_100,
            t2: 300_200,
            t3: 300_300,
            t4: 300_400,
            t5: 400_000,
        };
        recorder.drawn(&early, Some(1), &counters(), 1);
        recorder.drawn(&late, Some(2), &counters(), 1);
        recorder.presentation_resolved(1, 7_000);
        recorder.presentation_resolved(2, 17_000);
        assert_eq!(current(&recorder).presentation_interval.count(), 0);
        assert_eq!(
            recorder.totals.presentation_interval.count(),
            1,
            "the run's totals have it"
        );
    }

    #[test]
    fn a_permanently_missing_record_expires_after_eight_later_resolved_presents_and_the_window_completes()
     {
        let ids: Vec<u32> = (1..=10).collect();
        let mut recorder = presenting(&ids, 10_000_000);
        // Present 1 never reports; 2 to 9 resolve (eight later ones).
        for id in 2..=9_u32 {
            recorder.presentation_resolved(id, 1_000_000 * u64::from(id));
            if id < 9 {
                assert_eq!(
                    current(&recorder).presentation_expired,
                    0,
                    "after present {id}"
                );
            }
        }
        let stats = current(&recorder);
        assert_eq!(
            stats.presentation_expired, 1,
            "present 1 expired after eight later presents resolved"
        );
        assert_eq!(stats.presented, 8);
        // The pair (1, 2) formed no interval: 2 gave one with 3, and so on, but none with 1.
        assert_eq!(stats.presentation_interval.count(), 7);
        // Present 10 is still pending: the window is not final until it resolves.
        assert!(!recorder.accounted_through(10_000_000));
        recorder.presentation_resolved(10, 11_000_000);
        assert!(recorder.accounted_through(10_000_000));
    }

    #[test]
    fn a_record_whose_fence_was_seen_a_second_ago_expires_with_an_idle_renderer() {
        let mut recorder = presenting(&[1], 10_000_000);
        recorder.fence_seen(1, 5_000_000_000);
        recorder.expire_due(5_999_999_999);
        assert!(recorder.has_pending(), "not yet a second");
        assert!(!recorder.accounted_through(10_000_000));
        recorder.expire_due(6_000_000_000);
        assert!(!recorder.has_pending());
        assert_eq!(current(&recorder).presentation_expired, 1);
        assert!(
            recorder.accounted_through(10_000_000),
            "the window now completes"
        );
        // A record whose fence was never seen does not expire by time.
        let mut waiting = presenting(&[1], 10_000_000);
        waiting.expire_due(u64::MAX);
        assert!(waiting.has_pending());
    }

    #[test]
    fn a_seventeenth_pending_record_expires_the_oldest_counted() {
        let ids: Vec<u32> = (1..=17).collect();
        let recorder = presenting(&ids, 10_000_000);
        assert_eq!(recorder.pending_len, PENDING_PRESENTATIONS);
        assert_eq!(recorder.totals.presentation_expired, 1);
        assert_eq!(current(&recorder).presentation_expired, 1);
        assert!(
            recorder.pending[..recorder.pending_len]
                .iter()
                .all(|pending| pending.id != 1),
            "it was the oldest"
        );
    }

    #[test]
    fn a_gap_in_present_ids_forms_no_interval_and_a_late_arrival_after_expiry_forms_none() {
        let mut gap = presenting(&[1, 3], 10_000_000);
        gap.presentation_resolved(1, 1_000_000);
        gap.presentation_resolved(3, 2_000_000);
        assert_eq!(
            current(&gap).presentation_interval.count(),
            0,
            "1 and 3 are not consecutive"
        );
        // 2 resolves after 3 expired?  Present 4 pairs with 3 even if 4 resolves first.
        let mut out_of_order = presenting(&[5, 6], 10_000_000);
        out_of_order.presentation_resolved(6, 9_000_000);
        out_of_order.presentation_resolved(5, 4_000_000);
        assert_eq!(
            current(&out_of_order).presentation_interval.count(),
            1,
            "formed by whichever resolves second"
        );
        // A partner that expired forms none: 8 later resolutions expire 7; then 7's late feedback is ignored.
        let ids: Vec<u32> = (7..=16).collect();
        let mut late = presenting(&ids, 10_000_000);
        for id in 9..=16_u32 {
            late.presentation_resolved(id, 1_000_000 * u64::from(id));
        }
        assert_eq!(current(&late).presentation_expired, 2, "7 and 8 expired");
        let before = current(&late).presentation_interval.count();
        late.presentation_resolved(8, 8_000_000);
        assert_eq!(
            current(&late).presentation_interval.count(),
            before,
            "a late arrival forms no interval"
        );
        assert_eq!(current(&late).presented, 8, "and is not counted presented");
    }

    #[test]
    fn a_not_presented_record_is_counted_and_forms_no_interval() {
        let mut recorder = presenting(&[1, 2, 3], 10_000_000);
        recorder.presentation_resolved(1, 1_000_000);
        recorder.presentation_resolved(2, 0);
        recorder.presentation_resolved(3, 3_000_000);
        let stats = current(&recorder);
        assert_eq!((stats.presented, stats.not_presented), (2, 1));
        assert_eq!(stats.presentation_interval.count(), 0);
    }

    #[test]
    fn the_end_of_the_thread_expires_whatever_never_resolved() {
        let mut recorder = presenting(&[1, 2], 10_000_000);
        recorder.stop();
        assert!(recorder.stopped());
        assert!(!recorder.has_pending());
        assert_eq!(recorder.totals.presentation_expired, 2);
        assert!(recorder.accounted_through(10_000_000));
    }

    #[test]
    fn the_progress_fence_holds_while_a_step_that_began_before_the_end_is_unrecorded() {
        let mut recorder = recorder_with(1_000, 100_000);
        assert!(recorder.accounted_through(100_000));
        recorder.begin_step(50_000);
        assert!(!recorder.accounted_through(100_000));
        assert!(
            recorder.accounted_through(40_000),
            "a step that began at or after the end cannot matter"
        );
        recorder.end_step();
        assert!(recorder.accounted_through(100_000));
    }

    /// The ring of pending presents stays within its bound however many frames are recorded.
    #[test]
    fn recording_many_frames_and_results_leaves_every_ring_within_its_bound() {
        let mut recorder = recorder_with(1, u64::MAX / 2);
        for frame in 0..1_000_u32 {
            let t0 = 10_000 * (u64::from(frame) + 1);
            let marks = frame_marks(t0, 100);
            recorder.drawn(&marks, Some(frame), &counters(), 1);
            recorder.gpu(marks.t5, Some(1_000));
            if frame % 3 != 0 {
                recorder.presentation_resolved(frame, t0 * 1_000);
            }
            assert!(recorder.pending_len <= PENDING_PRESENTATIONS);
        }
        assert_eq!(recorder.totals.frames, 1_000);
    }

    mod allocation {
        use std::alloc::{GlobalAlloc, Layout, System};
        use std::cell::Cell;

        use super::*;

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

        /// No allocation per frame: after the warm-up the recorder takes frames, GPU results and presentation feedback (resolved and
        /// expired) in fixed storage.
        #[test]
        fn recording_many_frames_and_results_allocates_nothing_after_warm_up() {
            let mut recorder = recorder_with(0, u64::MAX);
            // Warm-up: the first frame, GPU result and presentation (the lock, the windows and the first window's storage exist).
            recorder.drawn(&frame_marks(1_000, 1_000), Some(0), &counters(), 1);
            recorder.gpu(frame_marks(1_000, 1_000).t5, Some(10));
            recorder.presentation_resolved(0, 5_000);
            let before = ALLOCATIONS.with(Cell::get);
            for frame in 1..10_000_u32 {
                let t0 = 100_000 + u64::from(frame) * 20_000;
                let marks = frame_marks(t0, 1_000 + u64::from(frame) % 7);
                recorder.drawn(&marks, Some(frame), &counters(), 1);
                recorder.gpu(marks.t5, Some(100 + u64::from(frame) % 13));
                // Most resolve; every seventh never does and expires through the bounded rules.
                if frame % 7 != 0 {
                    recorder.presentation_resolved(frame, 5_000 + u64::from(frame) * 16_667);
                }
                recorder.fence_seen(frame, marks.t5);
                recorder.expire_due(marks.t5 + 2_000_000_000);
                if frame % 100 == 0 {
                    recorder.not_drawn(t0);
                }
            }
            let after = ALLOCATIONS.with(Cell::get);
            assert_eq!(after - before, 0, "recording 10,000 frames allocated");
            assert_eq!(recorder.totals.frames, 10_000);
            assert!(recorder.totals.presentation_expired > 0);
        }
    }
}
