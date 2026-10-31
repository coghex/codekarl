//! The wake target: the one object that crosses threads.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ck_window::NativeWake;

use crate::ffi;

/// GLFW's `glfwPostEmptyEvent`, callable from any thread between `glfwInit` and `glfwTerminate` (and only
/// then). It posts an event that stays queued, so a `pump(UntilEvent)` entered after the wake returns at
/// once: the wake is sticky.
///
/// `live` is true from a successful `glfwInit` until just before `glfwTerminate`; a wake while it is false
/// is a counted no-op. The notifier already stops every wake at disarm and waits for the ones in flight
/// before the backend terminates, so `live` guards the direct calls the notifier never makes (a retained
/// target used after teardown), not a race the notifier leaves open.
#[derive(Debug, Default)]
pub(crate) struct GlfwWake {
    live: AtomicBool,
    posted: AtomicU64,
    ignored: AtomicU64,
}

impl GlfwWake {
    /// Called by the backend, on the main thread, around `glfwInit` / `glfwTerminate`.
    pub(crate) fn set_live(&self, live: bool) {
        self.live.store(live, Ordering::SeqCst);
    }

    /// Wakes posted to GLFW so far.
    pub(crate) fn posted(&self) -> u64 {
        self.posted.load(Ordering::SeqCst)
    }

    /// Wakes made while GLFW was not initialised, which did nothing.
    pub(crate) fn ignored(&self) -> u64 {
        self.ignored.load(Ordering::SeqCst)
    }
}

impl NativeWake for GlfwWake {
    fn wake(&self) {
        if self.live.load(Ordering::SeqCst) {
            self.posted.fetch_add(1, Ordering::SeqCst);
            // SAFETY: `glfwPostEmptyEvent` may be called from any thread once GLFW is initialised and
            // until it terminates; `live` is set only in that window, and the loop's notifier disarms
            // (waiting for calls in flight) before the backend terminates. The call does not block.
            unsafe { ffi::glfwPostEmptyEvent() };
        } else {
            self.ignored.fetch_add(1, Ordering::SeqCst);
        }
    }
}
