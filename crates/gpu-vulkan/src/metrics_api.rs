//! The scene presenter's frame-timing interface (milestone 5.5, issue 6, part B): `VulkanPresenter`'s methods for the common
//! measurement window, with `ck-gpu-metal`'s meanings and bounds (CK-129 r3): a requested window is adopted by the render thread at
//! its next boundary; a sample enters it only by its **own timestamp**; a reading is final once the render thread has passed the
//! window's end and nothing it owes is outstanding. The clock is the caller's ([`MetricsClock`](crate::MetricsClock)), so the
//! window is one interval on one clock for every owner.

use std::sync::{Condvar, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::metrics::{EndError, FrameStats, MetricsWindow, Recorder, RequestError, Tracked};
use crate::presenter::VulkanPresenter;
use crate::shared::lock;

/// How long [`VulkanPresenter::read_metrics`] waits between looks while a window's end has not passed on the clock.
const METRICS_WAIT_SLICE: Duration = Duration::from_millis(5);

/// A copy of one measurement window of the presenter, as [`VulkanPresenter::metrics_snapshot`] and
/// [`VulkanPresenter::read_metrics`] return it.
#[derive(Debug, Clone)]
pub struct MetricsReading {
    /// The window, its adoption time, whether it was adopted late, the samples counted outside it and the results dropped late, with
    /// the presenter's statistics for it.
    pub window: Tracked<FrameStats>,
    /// Frames handed off in the window whose GPU time was still outstanding when the reading was taken.
    pub outstanding: u64,
    /// This owner had passed the window's end and had nothing outstanding (no GPU result, no pending present feedback): the
    /// reading is the window's final one.
    pub complete: bool,
    /// Why GPU time is unavailable on this device, if it is (`timestampValidBits 0`). Not a zero and not a failure.
    pub gpu_unavailable: Option<&'static str>,
    /// Why presentation intervals are unavailable, if they are (`VK_GOOGLE_display_timing` not advertised).
    pub presentation_unavailable: Option<&'static str>,
}

/// The presenter's admission lock held for setting the end of a window ([`VulkanPresenter::lock_metrics_end`]). With the window
/// loop's lock also held, one timestamp read under both is the instant before which everything either owner admitted lies, and at or
/// after which nothing was: both owners then cover exactly the same `[start, end)`.
pub struct MetricsEndGuard<'a> {
    metrics: MutexGuard<'a, Recorder>,
    changed: &'a Condvar,
}

impl MetricsEndGuard<'_> {
    /// Sets the end of window `id`, once, while nothing is being admitted.
    ///
    /// # Errors
    ///
    /// The window is unknown, already ended, or `end` is not after its start.
    pub fn end(&mut self, id: u32, end: u64) -> Result<(), EndError> {
        self.metrics.windows.end(id, end)
    }
}

impl Drop for MetricsEndGuard<'_> {
    fn drop(&mut self) {
        self.changed.notify_all();
    }
}

impl MetricsReading {
    fn of(window: Tracked<FrameStats>, complete: bool, recorder: &Recorder) -> Self {
        Self {
            outstanding: window.stats.outstanding,
            window,
            complete,
            gpu_unavailable: recorder.gpu_unavailable,
            presentation_unavailable: recorder.presentation_unavailable,
        }
    }
}

impl VulkanPresenter {
    /// Requests the common measurement window `window` (`[start, end)` on the caller's clock, the same definition sent to every
    /// owner). The render thread adopts it at its next boundary (before its next frame step, or when this request wakes it) and
    /// publishes its adoption time ([`metrics_adopted`](Self::metrics_adopted)); a sample enters the window only by its **own
    /// timestamp** in `[start, end)`, whatever the adoption times.
    ///
    /// # Errors
    ///
    /// A request that overlaps the current window, whose id is not new or whose end is not after its start is refused and changes
    /// nothing.
    pub fn request_metrics(&self, window: MetricsWindow) -> Result<(), RequestError> {
        self.shared.metrics.recorder().windows.request(window)?;
        self.shared
            .metrics
            .check
            .store(true, std::sync::atomic::Ordering::Release);
        // The slot's mutex is taken for the notify, so a thread deciding to wait either sees the flag or is woken.
        drop(lock(&self.shared.slot));
        self.shared.signal.notify_all();
        Ok(())
    }

    /// Sets the end of window `id`, once (the application's run end).
    ///
    /// # Errors
    ///
    /// The window is unknown, already ended, or `end` is not after its start.
    pub fn end_metrics(&self, id: u32, end: u64) -> Result<(), EndError> {
        self.shared.metrics.recorder().windows.end(id, end)?;
        self.shared.metrics.changed.notify_all();
        Ok(())
    }

    /// When the render thread adopted window `id`, once it has.
    #[must_use]
    pub fn metrics_adopted(&self, id: u32) -> Option<u64> {
        self.shared.metrics.recorder().windows.adopted(id)
    }

    /// Holds the presenter's admission lock, so that an end can be set while the render thread admits nothing (see
    /// [`MetricsEndGuard`]). Hold it briefly; the render thread's recording waits for it.
    #[must_use]
    pub fn lock_metrics_end(&self) -> MetricsEndGuard<'_> {
        MetricsEndGuard {
            metrics: self.shared.metrics.recorder(),
            changed: &self.shared.metrics.changed,
        }
    }

    /// Why GPU time and why presentation intervals are unavailable on this device (a source it does not offer, said once for the run;
    /// never a zero): `(gpu, presentation)`.
    #[must_use]
    pub fn metrics_unavailable(&self) -> (Option<&'static str>, Option<&'static str>) {
        let recorder = self.shared.metrics.recorder();
        (recorder.gpu_unavailable, recorder.presentation_unavailable)
    }

    /// The whole run's figures so far, every sample whatever its window.
    #[must_use]
    pub fn metrics_totals(&self) -> FrameStats {
        self.shared.metrics.recorder().totals.clone()
    }

    /// A copy of window `id` as it stands, without waiting. `None` for a window the presenter does not keep (it keeps the current and
    /// the last ended one).
    #[must_use]
    pub fn metrics_snapshot(&self, id: u32) -> Option<MetricsReading> {
        let recorder = self.shared.metrics.recorder();
        let window = recorder.windows.get(id)?.clone();
        Some(MetricsReading::of(window, false, &recorder))
    }

    /// Window `id` once this owner has passed its end and the window's **outstanding** work (frames handed off in it whose GPU time
    /// is not yet read, and presents handed off before its end whose feedback is neither resolved nor expired) is zero, waiting at
    /// most `within` for that. Every pending present resolves or expires within a second of its fence, so a window completes within
    /// that bound; a reading that is not `complete` says what was still outstanding at the bound.
    #[must_use]
    pub fn read_metrics(&self, id: u32, within: Duration) -> Option<MetricsReading> {
        let deadline = Instant::now() + within;
        let metrics = &self.shared.metrics;
        let mut recorder = metrics.recorder();
        loop {
            let (passed, outstanding) = {
                let tracked = recorder.windows.get(id)?;
                (
                    // Past the end on the clock AND this owner has recorded every step that began before it AND every present
                    // handed off before it has resolved or expired.
                    tracked
                        .window
                        .end
                        .is_some_and(|end| metrics.now() >= end && recorder.accounted_through(end)),
                    tracked.stats.outstanding,
                )
            };
            let complete = passed && outstanding == 0;
            let now = Instant::now();
            if complete || now >= deadline {
                let window = recorder.windows.get(id)?.clone();
                return Some(MetricsReading::of(window, complete, &recorder));
            }
            // The end is a point on the clock and nothing signals it passing, so the wait is in slices.
            let slice = (deadline - now).min(METRICS_WAIT_SLICE);
            recorder = metrics
                .changed
                .wait_timeout(recorder, slice)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}
