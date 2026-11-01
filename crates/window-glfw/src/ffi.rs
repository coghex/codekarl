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
/// `GLFW_FOCUSED` and `GLFW_FOCUS_ON_SHOW` from GLFW 3.4's `glfw3.h`.
pub(crate) const FOCUSED: c_int = 0x0002_0001;
pub(crate) const FOCUS_ON_SHOW: c_int = 0x0002_000c;
/// `GLFW_CLIENT_API` and `GLFW_NO_API`: the window is for Vulkan, so GLFW creates no OpenGL context.
pub(crate) const CLIENT_API: c_int = 0x0002_2001;
pub(crate) const NO_API: c_int = 0;
/// `GLFW_RESIZABLE`.
pub(crate) const RESIZABLE: c_int = 0x0002_0003;
/// `GLFW_COCOA_MENUBAR`: an init hint (macOS only; it is not set on other platforms).
pub(crate) const COCOA_MENUBAR: c_int = 0x0005_1002;
/// `GLFW_RELEASE`, `GLFW_PRESS`, `GLFW_REPEAT` (key actions).
pub(crate) const RELEASE: c_int = 0;
pub(crate) const PRESS: c_int = 1;
pub(crate) const REPEAT: c_int = 2;
/// Keyboard token values from GLFW 3.4's `glfw3.h`.
pub(crate) const KEY_UNKNOWN: c_int = -1;
pub(crate) const KEY_SPACE: c_int = 32;
pub(crate) const KEY_APOSTROPHE: c_int = 39;
pub(crate) const KEY_COMMA: c_int = 44;
pub(crate) const KEY_MINUS: c_int = 45;
pub(crate) const KEY_PERIOD: c_int = 46;
pub(crate) const KEY_SLASH: c_int = 47;
pub(crate) const KEY_0: c_int = 48;
pub(crate) const KEY_1: c_int = 49;
pub(crate) const KEY_2: c_int = 50;
pub(crate) const KEY_3: c_int = 51;
pub(crate) const KEY_4: c_int = 52;
pub(crate) const KEY_5: c_int = 53;
pub(crate) const KEY_6: c_int = 54;
pub(crate) const KEY_7: c_int = 55;
pub(crate) const KEY_8: c_int = 56;
pub(crate) const KEY_9: c_int = 57;
pub(crate) const KEY_SEMICOLON: c_int = 59;
pub(crate) const KEY_EQUAL: c_int = 61;
pub(crate) const KEY_A: c_int = 65;
pub(crate) const KEY_B: c_int = 66;
pub(crate) const KEY_C: c_int = 67;
pub(crate) const KEY_D: c_int = 68;
pub(crate) const KEY_E: c_int = 69;
pub(crate) const KEY_F: c_int = 70;
pub(crate) const KEY_G: c_int = 71;
pub(crate) const KEY_H: c_int = 72;
pub(crate) const KEY_I: c_int = 73;
pub(crate) const KEY_J: c_int = 74;
pub(crate) const KEY_K: c_int = 75;
pub(crate) const KEY_L: c_int = 76;
pub(crate) const KEY_M: c_int = 77;
pub(crate) const KEY_N: c_int = 78;
pub(crate) const KEY_O: c_int = 79;
pub(crate) const KEY_P: c_int = 80;
pub(crate) const KEY_Q: c_int = 81;
pub(crate) const KEY_R: c_int = 82;
pub(crate) const KEY_S: c_int = 83;
pub(crate) const KEY_T: c_int = 84;
pub(crate) const KEY_U: c_int = 85;
pub(crate) const KEY_V: c_int = 86;
pub(crate) const KEY_W: c_int = 87;
pub(crate) const KEY_X: c_int = 88;
pub(crate) const KEY_Y: c_int = 89;
pub(crate) const KEY_Z: c_int = 90;
pub(crate) const KEY_LEFT_BRACKET: c_int = 91;
pub(crate) const KEY_BACKSLASH: c_int = 92;
pub(crate) const KEY_RIGHT_BRACKET: c_int = 93;
pub(crate) const KEY_GRAVE_ACCENT: c_int = 96;
pub(crate) const KEY_ESCAPE: c_int = 256;
pub(crate) const KEY_ENTER: c_int = 257;
pub(crate) const KEY_TAB: c_int = 258;
pub(crate) const KEY_BACKSPACE: c_int = 259;
pub(crate) const KEY_DELETE: c_int = 261;
pub(crate) const KEY_RIGHT: c_int = 262;
pub(crate) const KEY_LEFT: c_int = 263;
pub(crate) const KEY_DOWN: c_int = 264;
pub(crate) const KEY_UP: c_int = 265;
pub(crate) const KEY_PAGE_UP: c_int = 266;
pub(crate) const KEY_PAGE_DOWN: c_int = 267;
pub(crate) const KEY_HOME: c_int = 268;
pub(crate) const KEY_END: c_int = 269;
pub(crate) const KEY_F1: c_int = 290;
pub(crate) const KEY_F2: c_int = 291;
pub(crate) const KEY_F3: c_int = 292;
pub(crate) const KEY_F4: c_int = 293;
pub(crate) const KEY_F5: c_int = 294;
pub(crate) const KEY_F6: c_int = 295;
pub(crate) const KEY_F7: c_int = 296;
pub(crate) const KEY_F8: c_int = 297;
pub(crate) const KEY_F9: c_int = 298;
pub(crate) const KEY_F10: c_int = 299;
pub(crate) const KEY_F11: c_int = 300;
pub(crate) const KEY_F12: c_int = 301;
pub(crate) const KEY_LEFT_SHIFT: c_int = 340;
pub(crate) const KEY_LEFT_CONTROL: c_int = 341;
pub(crate) const KEY_LEFT_ALT: c_int = 342;
pub(crate) const KEY_LEFT_SUPER: c_int = 343;
pub(crate) const KEY_RIGHT_SHIFT: c_int = 344;
pub(crate) const KEY_RIGHT_CONTROL: c_int = 345;
pub(crate) const KEY_RIGHT_ALT: c_int = 346;
pub(crate) const KEY_RIGHT_SUPER: c_int = 347;
/// The `VkResult` of success.
pub(crate) const VK_SUCCESS: c_int = 0;

pub(crate) type ErrorCallback = extern "C" fn(error: c_int, description: *const c_char);
pub(crate) type KeyCallback =
    extern "C" fn(window: *mut GlfwWindow, key: c_int, scancode: c_int, action: c_int, mods: c_int);
pub(crate) type CloseCallback = extern "C" fn(window: *mut GlfwWindow);
pub(crate) type SizeCallback = extern "C" fn(window: *mut GlfwWindow, width: c_int, height: c_int);
pub(crate) type ScaleCallback = extern "C" fn(window: *mut GlfwWindow, x: f32, y: f32);
pub(crate) type RefreshCallback = extern "C" fn(window: *mut GlfwWindow);
pub(crate) type FocusCallback = extern "C" fn(window: *mut GlfwWindow, focused: c_int);
pub(crate) type IconifyCallback = extern "C" fn(window: *mut GlfwWindow, iconified: c_int);

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
    pub(crate) fn glfwSetWindowFocusCallback(
        window: *mut GlfwWindow,
        callback: Option<FocusCallback>,
    ) -> Option<FocusCallback>;
    pub(crate) fn glfwSetWindowIconifyCallback(
        window: *mut GlfwWindow,
        callback: Option<IconifyCallback>,
    ) -> Option<IconifyCallback>;
    pub(crate) fn glfwPollEvents();
    pub(crate) fn glfwWaitEvents();
    pub(crate) fn glfwWaitEventsTimeout(timeout: f64);
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
