//! Test support for `window.rs`: the keyboard filter's entry points (`ck-window-glfw`'s test-only native
//! code, linked into the `window` test executable only) and small helpers.

// The one binding of the shared native info structure and its layout check, included from `ck-window-glfw`'s
// test support (this crate reads that crate's test files by path, as `build.rs` does for the native source).
#[path = "../../../window-glfw/tests/support/info_abi.rs"]
mod info_abi;

pub(crate) use info_abi::{
    check_layout, check_nothing_else_binds_the_native_info,
    check_the_comparison_rejects_mismatched_bindings,
};

unsafe extern "C" {
    fn ck_glfw_test_activate();
    fn ck_glfw_test_install_input_filter();
    fn ck_glfw_test_start_typist();
}

/// What the native layer sees of the window.
#[derive(Debug, Clone)]
pub(crate) struct Info {
    pub(crate) visible: bool,
}

/// The window's state, read through `AppKit`. Main thread.
pub(crate) fn info() -> Info {
    let raw = info_abi::read();
    Info {
        visible: raw.visible != 0,
    }
}

/// Activates the application and makes the window key. Main thread.
pub(crate) fn activate() {
    // SAFETY: a plain AppKit call on the main thread.
    unsafe { ck_glfw_test_activate() };
}

/// Installs the filter that drops every unmarked keyboard event. Main thread, before the first window.
pub(crate) fn install_input_filter() {
    // SAFETY: a plain AppKit call on the main thread; idempotent.
    unsafe { ck_glfw_test_install_input_filter() };
}

/// Starts the thread that types without stopping (unmarked events of every keyboard kind).
pub(crate) fn start_typist() {
    // SAFETY: starts a thread that only posts events, which AppKit allows from any thread.
    unsafe { ck_glfw_test_start_typist() };
}
