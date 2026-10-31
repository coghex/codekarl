//! The raw declarations of `native/window.h`. Every call is made through the wrappers in `backend.rs` and
//! `wake.rs`, each with its `SAFETY` argument.

use std::ffi::{c_char, c_void};

/// The most entries one pump hands over (`CK_WIN_OUT_MAX`).
pub(crate) const OUT_MAX: usize = 258;

/// The kinds of an entry (`CK_WIN_OUT_*`).
pub(crate) const OUT_KEY: u8 = 0;
pub(crate) const OUT_FOCUS_LOST: u8 = 1;
pub(crate) const OUT_FOCUS_GAINED: u8 = 2;
pub(crate) const OUT_RESYNC: u8 = 3;

/// One key or focus entry of a pump (`CKWinOut`).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawOut {
    pub(crate) kind: u8,
    pub(crate) key: u8,
    pub(crate) down: u8,
    pub(crate) repeat: u8,
}

/// The final key states and loss latches after an overflow (`CKWinResync`).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawResync {
    pub(crate) held: [u64; 2],
    pub(crate) pressed: [u64; 2],
    pub(crate) released: [u64; 2],
}

/// What one pump hands over (`CKWinEvents`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawEvents {
    pub(crate) close_source: i32,
    pub(crate) close_absorbed: u32,
    pub(crate) pixel_changed: i32,
    pub(crate) pixel_width: u32,
    pub(crate) pixel_height: u32,
    pub(crate) needs_display: i32,
    pub(crate) minimized: i32,
    pub(crate) restored: i32,
    pub(crate) native_events: u64,
    pub(crate) overflow_total: u64,
    pub(crate) out_count: u32,
    pub(crate) out: [RawOut; OUT_MAX],
    pub(crate) resync: RawResync,
}

impl Default for RawEvents {
    fn default() -> Self {
        Self {
            close_source: 0,
            close_absorbed: 0,
            pixel_changed: 0,
            pixel_width: 0,
            pixel_height: 0,
            needs_display: 0,
            minimized: 0,
            restored: 0,
            native_events: 0,
            overflow_total: 0,
            out_count: 0,
            out: [RawOut::default(); OUT_MAX],
            resync: RawResync::default(),
        }
    }
}

unsafe extern "C" {
    pub(crate) fn ck_win_events_size() -> usize;
    pub(crate) fn ck_win_create(error: *mut c_char, capacity: usize) -> *mut c_void;
    pub(crate) fn ck_win_create_window(
        handle: *mut c_void,
        title: *const c_char,
        title_length: usize,
        width: u32,
        height: u32,
        layer: *mut *mut c_void,
        pixel_width: *mut u32,
        pixel_height: *mut u32,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    pub(crate) fn ck_win_pump(handle: *mut c_void, wait: i32, cap: usize, out: *mut RawEvents);
    pub(crate) fn ck_win_pump_until(
        handle: *mut c_void,
        cap: usize,
        timeout_seconds: f64,
        out: *mut RawEvents,
    );
    pub(crate) fn ck_win_destroy_window(handle: *mut c_void);
    pub(crate) fn ck_win_terminate(handle: *mut c_void);
    pub(crate) fn ck_win_free(handle: *mut c_void);
    pub(crate) fn ck_win_wake_target(handle: *mut c_void) -> *mut c_void;
    pub(crate) fn ck_win_wake(wake: *const c_void);
    pub(crate) fn ck_win_wake_release(wake: *mut c_void);
}
