//! The minimal GLFW binding: only the functions this crate calls, and the constants it needs.
//!
//! Nothing here is safe to call on its own; every call is made from `backend.rs` or `wake.rs`, each with
//! its `SAFETY` argument. The declarations follow `GLFW/glfw3.h` (3.4, unchanged in 3.5). No other GLFW
//! crate is used and nothing is generated.

use std::ffi::{c_char, c_int, c_void};

/// An opaque `GLFWwindow`.
#[repr(C)]
pub(crate) struct GlfwWindow {
    _private: [u8; 0],
}

pub(crate) const FALSE: c_int = 0;
pub(crate) const TRUE: c_int = 1;
/// `GLFW_CLIENT_API` and `GLFW_NO_API`: the window is for Vulkan, so GLFW creates no OpenGL context.
pub(crate) const CLIENT_API: c_int = 0x0002_2001;
pub(crate) const NO_API: c_int = 0;
/// `GLFW_RESIZABLE`.
pub(crate) const RESIZABLE: c_int = 0x0002_0003;
/// `GLFW_COCOA_MENUBAR`: an init hint (macOS only; it is not set on other platforms).
pub(crate) const COCOA_MENUBAR: c_int = 0x0005_1002;
/// `GLFW_PRESS`, `GLFW_REPEAT` (key actions).
pub(crate) const PRESS: c_int = 1;
pub(crate) const REPEAT: c_int = 2;
/// `GLFW_KEY_ESCAPE`.
pub(crate) const KEY_ESCAPE: c_int = 256;
/// The `VkResult` of success.
pub(crate) const VK_SUCCESS: c_int = 0;

pub(crate) type ErrorCallback = extern "C" fn(error: c_int, description: *const c_char);
pub(crate) type KeyCallback =
    extern "C" fn(window: *mut GlfwWindow, key: c_int, scancode: c_int, action: c_int, mods: c_int);
pub(crate) type CloseCallback = extern "C" fn(window: *mut GlfwWindow);
pub(crate) type SizeCallback = extern "C" fn(window: *mut GlfwWindow, width: c_int, height: c_int);
pub(crate) type ScaleCallback = extern "C" fn(window: *mut GlfwWindow, x: f32, y: f32);
pub(crate) type RefreshCallback = extern "C" fn(window: *mut GlfwWindow);

unsafe extern "C" {
    pub(crate) fn glfwInit() -> c_int;
    pub(crate) fn glfwTerminate();
    pub(crate) fn glfwInitHint(hint: c_int, value: c_int);
    pub(crate) fn glfwGetVersion(major: *mut c_int, minor: *mut c_int, revision: *mut c_int);
    pub(crate) fn glfwInitVulkanLoader(loader: Option<crate::loader::GetInstanceProcAddr>);
    pub(crate) fn glfwSetErrorCallback(callback: Option<ErrorCallback>) -> Option<ErrorCallback>;
    pub(crate) fn glfwWindowHint(hint: c_int, value: c_int);
    pub(crate) fn glfwCreateWindow(
        width: c_int,
        height: c_int,
        title: *const c_char,
        monitor: *mut c_void,
        share: *mut GlfwWindow,
    ) -> *mut GlfwWindow;
    pub(crate) fn glfwDestroyWindow(window: *mut GlfwWindow);
    pub(crate) fn glfwGetFramebufferSize(
        window: *mut GlfwWindow,
        width: *mut c_int,
        height: *mut c_int,
    );
    pub(crate) fn glfwWindowShouldClose(window: *mut GlfwWindow) -> c_int;
    pub(crate) fn glfwSetWindowShouldClose(window: *mut GlfwWindow, value: c_int);
    pub(crate) fn glfwSetWindowUserPointer(window: *mut GlfwWindow, pointer: *mut c_void);
    pub(crate) fn glfwGetWindowUserPointer(window: *mut GlfwWindow) -> *mut c_void;
    pub(crate) fn glfwSetKeyCallback(
        window: *mut GlfwWindow,
        callback: Option<KeyCallback>,
    ) -> Option<KeyCallback>;
    pub(crate) fn glfwSetWindowCloseCallback(
        window: *mut GlfwWindow,
        callback: Option<CloseCallback>,
    ) -> Option<CloseCallback>;
    pub(crate) fn glfwSetFramebufferSizeCallback(
        window: *mut GlfwWindow,
        callback: Option<SizeCallback>,
    ) -> Option<SizeCallback>;
    pub(crate) fn glfwSetWindowContentScaleCallback(
        window: *mut GlfwWindow,
        callback: Option<ScaleCallback>,
    ) -> Option<ScaleCallback>;
    pub(crate) fn glfwSetWindowRefreshCallback(
        window: *mut GlfwWindow,
        callback: Option<RefreshCallback>,
    ) -> Option<RefreshCallback>;
    pub(crate) fn glfwPollEvents();
    pub(crate) fn glfwWaitEvents();
    pub(crate) fn glfwPostEmptyEvent();
    pub(crate) fn glfwVulkanSupported() -> c_int;
    pub(crate) fn glfwGetRequiredInstanceExtensions(count: *mut u32) -> *const *const c_char;
    /// The window's `NSWindow` (macOS only); NULL when GLFW is not initialised or the window is not a Cocoa one.
    #[cfg(target_os = "macos")]
    pub(crate) fn glfwGetCocoaWindow(window: *mut GlfwWindow) -> *mut c_void;
    pub(crate) fn glfwCreateWindowSurface(
        instance: *mut c_void,
        window: *mut GlfwWindow,
        allocator: *const c_void,
        surface: *mut u64,
    ) -> c_int;
}
