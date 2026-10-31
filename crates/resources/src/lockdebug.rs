//! Which locks the current thread holds (tests only).
//!
//! The service and the registry each count, per thread, the locks of this crate that the thread holds. A value whose
//! drop must happen with no lock held (a pixel buffer, a resource state) reports a violation here when it is dropped
//! while the count is not zero; the tests assert there were none.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

thread_local! {
    static HELD: Cell<usize> = const { Cell::new(0) };
}

static VIOLATIONS: AtomicUsize = AtomicUsize::new(0);

/// Marks one lock of this crate as held by the current thread, until it is dropped.
pub(crate) struct Held;

impl Held {
    pub(crate) fn new() -> Self {
        HELD.with(|held| held.set(held.get() + 1));
        Self
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        HELD.with(|held| held.set(held.get() - 1));
    }
}

/// Whether the current thread holds a lock of this crate.
pub(crate) fn held() -> bool {
    HELD.with(|held| held.get() > 0)
}

/// Called by a value's `Drop`: counts a violation if a lock is held.
pub(crate) fn dropped_value() {
    if held() {
        VIOLATIONS.fetch_add(1, Ordering::SeqCst);
    }
}

/// Called where a lock of this crate must not be held (a wake that takes the service lock itself): counts a violation if
/// one is.
pub(crate) fn must_be_free() {
    dropped_value();
}

/// How many values have been dropped with a lock held so far, in the whole test process.
pub(crate) fn violations() -> usize {
    VIOLATIONS.load(Ordering::SeqCst)
}
