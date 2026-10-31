//! The two `AppKit` calls the backend makes itself on macOS: the regular activation policy and the request
//! to come to the front (see `native/activation.m`). On other platforms both do nothing.

use crate::ffi::GlfwWindow;
use std::ptr::NonNull;

/// What `native/activation.h` reports.
#[cfg(target_os = "macos")]
const OK: i32 = 0;
#[cfg(target_os = "macos")]
const OFF_MAIN: i32 = 1;
#[cfg(target_os = "macos")]
const POLICY_REFUSED: i32 = 2;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn ck_glfw_ensure_regular_policy() -> i32;
    fn ck_glfw_activate_window(native_window: *mut std::ffi::c_void) -> i32;
}

/// Gives the process the regular activation policy unless it has it. GLFW does that itself only together
/// with its menu bar, which the backend leaves out, so without this the process cannot become the active
/// application. Call after `glfwInit`, on the main thread.
///
/// # Errors
///
/// `the regular activation policy could not be set` when `AppKit` refuses it.
#[cfg(target_os = "macos")]
pub(crate) fn ensure_regular_policy() -> Result<(), String> {
    // SAFETY: the function takes no argument, checks for the main thread itself and touches only the shared
    // application object, which GLFW's initialisation has created.
    match unsafe { ck_glfw_ensure_regular_policy() } {
        OK => Ok(()),
        POLICY_REFUSED => Err("the regular activation policy could not be set".to_owned()),
        OFF_MAIN => Err("the activation policy belongs to the main thread".to_owned()),
        other => Err(format!("the activation policy call returned {other}")),
    }
}

#[cfg(not(target_os = "macos"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the macOS function can fail; the other platforms share its signature"
)]
pub(crate) fn ensure_regular_policy() -> Result<(), String> {
    Ok(())
}

/// Asks for the application to be activated and the window made key and ordered front. A request the
/// system may decline: it is not waited for, retried or treated as a failure. `Err` says why the request
/// was not even made.
#[cfg(target_os = "macos")]
pub(crate) fn request_activation(window: NonNull<GlfwWindow>) -> Result<(), String> {
    // SAFETY: `window` is a live GLFW window and this runs on the main thread; `glfwGetCocoaWindow` only
    // reads it.
    let native = unsafe { crate::ffi::glfwGetCocoaWindow(window.as_ptr()) };
    // SAFETY: `native` is the window's `NSWindow` (or NULL, which the function reports); the call checks the
    // main thread itself and the window is not destroyed while it runs.
    match unsafe { ck_glfw_activate_window(native) } {
        OK => Ok(()),
        other => Err(format!("the activation request returned status {other}")),
    }
}

#[cfg(not(target_os = "macos"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the macOS function can fail; the other platforms share its signature"
)]
pub(crate) fn request_activation(_window: NonNull<GlfwWindow>) -> Result<(), String> {
    Ok(())
}
