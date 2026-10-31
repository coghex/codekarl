//! The state the presenter's handle and its render thread share.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::slot::QuadSkipped;
use crate::unwind;

/// The stop budget: the moment of the first `request_stop` plus this.
pub(crate) const STOP_BUDGET: Duration = Duration::from_millis(1500);

/// What the render thread reports through `VulkanPresenter::take_report`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenterReport {
    /// The first frame was queued for presentation (a present call returned success): not proof that it
    /// is visible. Sent once.
    FirstFrameQueued,
    /// Setup or a frame failed. The thread draws nothing more.
    Failed {
        /// What failed.
        message: String,
    },
}

/// How the presenter's cleanup ended, once the thread has been observed finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupResult {
    /// The thread sent an explicit success after every submit fence, present fence and drain was proven and
    /// every object was destroyed.
    Proven,
    /// Anything else (a panic, a missing result, a fence that did not signal by the stop deadline, a lost
    /// device). Every object the thread owned was kept, and the loader with it.
    Unproven {
        /// Why.
        reason: String,
    },
}

/// The wake callback type.
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

/// The latest-value slot the main thread writes and the render thread reads.
pub(crate) struct Slot {
    pub(crate) clear: [f32; 3],
    /// The window's pixel size; a zero dimension suspends rendering.
    pub(crate) size: (u32, u32),
    /// A frame is wanted: a changed colour or size, a redraw request, or the first frame.
    pub(crate) dirty: bool,
    /// Counts every redraw trigger (a changed colour or size, a redraw request): a paused swap chain
    /// creation retries at once when this has moved.
    pub(crate) epoch: u64,
    pub(crate) stop: bool,
    /// The absolute stop deadline, set once, by the first `request_stop`.
    pub(crate) stop_deadline: Option<Instant>,
}

pub(crate) struct Shared {
    pub(crate) slot: Mutex<Slot>,
    pub(crate) signal: Condvar,
    pub(crate) reports: Mutex<VecDeque<PresenterReport>>,
    /// The cleanup result the render thread sent, if it did.
    pub(crate) result: Mutex<Option<CleanupResult>>,
    pub(crate) wake: Wake,
    pub(crate) stop_budget: Duration,
    /// The generation of the scene the last queued frame drew; meaningful once `drawn_any` is set. Written value
    /// first, flag second.
    pub(crate) drawn_generation: AtomicU64,
    pub(crate) drawn_any: AtomicBool,
    /// The format notice, until the main thread takes it.
    pub(crate) notice: Mutex<Option<QuadSkipped>>,
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    /// Calls the wake. A panic in it is caught and its payload disposed of (even one whose `Drop` panics): it
    /// never changes a result already sent, and this always returns.
    pub(crate) fn wake(&self) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| (self.wake)())) {
            unwind::dispose(payload);
        }
    }

    pub(crate) fn report(&self, report: PresenterReport) {
        lock(&self.reports).push_back(report);
        self.wake();
    }
}

#[cfg(test)]
mod tests {
    use super::{CleanupResult, PresenterReport, Shared, Slot, lock};
    use std::collections::VecDeque;
    use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    /// A panic payload whose `Drop` panics (and counts itself).
    struct Bomb(Arc<AtomicUsize>);

    impl Drop for Bomb {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
            panic!("a payload whose drop panics");
        }
    }

    fn shared(wake: Arc<dyn Fn() + Send + Sync>) -> Shared {
        Shared {
            slot: Mutex::new(Slot {
                clear: [0.0; 3],
                size: (1, 1),
                dirty: false,
                epoch: 0,
                stop: false,
                stop_deadline: None,
            }),
            signal: Condvar::new(),
            reports: Mutex::new(VecDeque::new()),
            result: Mutex::new(Some(CleanupResult::Proven)),
            wake,
            stop_budget: Duration::from_secs(1),
            drawn_generation: AtomicU64::new(0),
            drawn_any: AtomicBool::new(false),
            notice: Mutex::new(None),
        }
    }

    #[test]
    fn a_wake_that_panics_with_a_dropping_payload_is_discarded_and_the_report_is_queued() {
        let drops = Arc::new(AtomicUsize::new(0));
        let from_wake = Arc::clone(&drops);
        let shared = shared(Arc::new(move || panic_any(Bomb(Arc::clone(&from_wake)))));
        // The caller's own boundary: the wake's payload must not reach it.
        let escaped = catch_unwind(AssertUnwindSafe(|| {
            shared.report(PresenterReport::FirstFrameQueued);
        }));
        assert!(escaped.is_ok(), "the wake's payload escaped `report`");
        assert_eq!(
            drops.load(Ordering::SeqCst),
            1,
            "the payload was dropped once"
        );
        assert_eq!(
            lock(&shared.reports).front(),
            Some(&PresenterReport::FirstFrameQueued),
            "the report is queued"
        );
        assert_eq!(
            *lock(&shared.result),
            Some(CleanupResult::Proven),
            "a result already sent is unchanged"
        );
    }

    #[test]
    fn a_wake_that_panics_with_an_ordinary_payload_is_discarded_too() {
        let shared = shared(Arc::new(|| panic!("an ordinary wake panic")));
        let escaped = catch_unwind(AssertUnwindSafe(|| shared.wake()));
        assert!(escaped.is_ok());
        assert_eq!(*lock(&shared.result), Some(CleanupResult::Proven));
    }
}
