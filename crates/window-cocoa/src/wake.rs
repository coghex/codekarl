//! The native wake target: the one object that crosses threads.

use std::ffi::c_void;
use std::ptr::NonNull;

use ck_window::NativeWake;

use crate::ffi;

/// A retained reference to the application's wake target (`CKWakeBox`): a strong reference to the
/// application object and a flag that coalesces wake events while one is queued.
pub(crate) struct CocoaWake {
    handle: NonNull<c_void>,
}

impl std::fmt::Debug for CocoaWake {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CocoaWake")
    }
}

impl CocoaWake {
    /// Takes one retained wake reference from the backend whose native handle is `backend`. Main thread.
    pub(crate) fn new(backend: NonNull<c_void>) -> Option<Self> {
        // SAFETY: `backend` is a live backend handle and this runs on the main thread (the backend
        // is neither Send nor Sync). The call returns one retained reference, which this value owns.
        let handle = unsafe { ffi::ck_win_wake_target(backend.as_ptr()) };
        NonNull::new(handle).map(|handle| Self { handle })
    }
}

// SAFETY: the pointer is a retained reference to an Objective-C object whose state never changes after
// creation (the application it holds) except for two atomics, and whose only operation here is
// `postEvent:atStart:` on that application, which Apple documents as callable from any thread. A wake
// creates one event and posts it inside its own autorelease pool, and touches no window, view or layer.
unsafe impl Send for CocoaWake {}
// SAFETY: as for `Send`: `wake` takes `&self` and only performs that documented, thread-safe post.
unsafe impl Sync for CocoaWake {}

impl NativeWake for CocoaWake {
    fn wake(&self) {
        // SAFETY: the reference is retained for as long as this value lives, so the wake target is valid
        // for the call; the native side posts one event and returns without blocking.
        unsafe { ffi::ck_win_wake(self.handle.as_ptr()) };
    }
}

impl Drop for CocoaWake {
    fn drop(&mut self) {
        // SAFETY: this consumes the one retained reference taken in `new`, once. Releasing it only
        // lowers a reference count.
        unsafe { ffi::ck_win_wake_release(self.handle.as_ptr()) };
    }
}
