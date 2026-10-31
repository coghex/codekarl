//! Test support for `window_glfw.rs`: safe wrappers over `tests/native/testing.m`, the raw GLFW and Vulkan
//! entry points the cases call directly, and a timed pump helper.
//!
//! None of this is part of the library. The native half is built into its own archive that only the test
//! executable links (`build.rs`), and it acts on the process's one GLFW window by finding it through the
//! application's window list, so the library exports no handle, hook or control for it.

use std::ffi::{CStr, CString, c_char, c_int, c_void};

use ck_window_glfw::{GetInstanceProcAddr, VulkanLoaderForGlfw};

mod info_abi;

unsafe extern "C" {
    fn ck_glfw_test_activate();
    fn ck_glfw_test_perform_close();
    fn ck_glfw_test_window_number() -> i64;
    fn ck_glfw_test_post_mouse_moved(window_number: i64);
    fn ck_glfw_test_post_key(
        key_code: u16,
        modifier_flags: u64,
        repeat: i32,
        characters: *const c_char,
        marked: i32,
    ) -> i32;
    fn ck_glfw_test_post_keyboard_event(
        kind: i32,
        key_code: u16,
        modifier_flags: u64,
        marked: i32,
    ) -> i32;
    fn ck_glfw_test_install_input_filter();
    fn ck_glfw_test_start_typist();
    fn ck_glfw_test_set_scale(scale: f64);
    fn ck_glfw_test_trace_start();
    fn ck_glfw_test_trace_mark(text: *const c_char);
    fn ck_glfw_test_trace_take(out: *mut c_char, capacity: usize) -> usize;
    fn ck_glfw_test_backing_changed();
    fn ck_glfw_test_watch_policy(refuse: i32);
    fn ck_glfw_test_policy_calls() -> u64;
    fn ck_glfw_test_watch_activate() -> i32;
    fn ck_glfw_test_activate_calls() -> u64;
    fn glfwGetPrimaryMonitor() -> *mut c_void;
    fn glfwWindowShouldClose(window: *mut c_void) -> c_int;
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
    fn dlclose(handle: *mut c_void) -> c_int;
}

/// The Escape and `q` key codes, and the Command modifier flag.
pub(crate) const ESCAPE: u16 = 53;
pub(crate) const KEY_Q: u16 = 12;
pub(crate) const COMMAND: u64 = 1 << 20;

/// What the native layer sees of the window.
#[derive(Debug, Clone)]
pub(crate) struct Info {
    pub(crate) has_window: bool,
    pub(crate) title: String,
    pub(crate) content: (f64, f64),
    pub(crate) resizable: bool,
    pub(crate) visible: bool,
    pub(crate) window_scale: f64,
    pub(crate) menu_key_equivalents: u32,
    pub(crate) window_count: u64,
    pub(crate) filtered_keys: u64,
    /// `NSApplicationActivationPolicy` of the shared application, or -1 before it exists.
    pub(crate) activation_policy: i32,
    pub(crate) focus: Focus,
}

/// Whether the application is the active one and whether the window is its key window.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Focus {
    pub(crate) active: bool,
    pub(crate) key: bool,
}

/// `NSApplicationActivationPolicyRegular`.
pub(crate) const POLICY_REGULAR: i32 = 0;

/// The layout of the info structure against the native side's, which needs no window.
pub(crate) use info_abi::{
    check_layout, check_nothing_else_binds_the_native_info,
    check_the_comparison_rejects_mismatched_bindings,
};

/// The window's state, read through `AppKit`. Main thread.
pub(crate) fn info() -> Info {
    let raw = info_abi::read();
    let title = CStr::from_bytes_until_nul(&raw.title)
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_default();
    Info {
        has_window: raw.has_window != 0,
        title,
        content: (raw.content_width, raw.content_height),
        resizable: raw.resizable != 0,
        visible: raw.visible != 0,
        window_scale: raw.window_scale,
        menu_key_equivalents: raw.menu_key_equivalents,
        window_count: raw.window_count,
        filtered_keys: raw.filtered_keys,
        activation_policy: raw.activation_policy,
        focus: Focus {
            active: raw.is_active != 0,
            key: raw.is_key != 0,
        },
    }
}

/// Activates the application and makes the window key. Main thread.
pub(crate) fn activate() {
    // SAFETY: a plain AppKit call on the main thread.
    unsafe { ck_glfw_test_activate() };
}

/// What the close button does: the window delegate's `windowShouldClose:`. Main thread.
pub(crate) fn perform_close() {
    // SAFETY: a plain AppKit call on the main thread.
    unsafe { ck_glfw_test_perform_close() };
}

/// Posts a key-down made from a `CGEvent`, marked as the tests' own or not (the owner's typing). Any
/// thread. Returns whether the event could be made.
pub(crate) fn post_key(
    key_code: u16,
    flags: u64,
    repeat: bool,
    characters: &str,
    marked: bool,
) -> bool {
    let characters = CString::new(characters).expect("no NUL in the characters");
    // SAFETY: the characters are NUL-terminated and valid for the call; `NSApp postEvent:` is documented
    // as callable from any thread.
    unsafe {
        ck_glfw_test_post_key(
            key_code,
            flags,
            i32::from(repeat),
            characters.as_ptr(),
            i32::from(marked),
        ) != 0
    }
}

/// Posts a key-up (`kind` 1) or a modifier change (`kind` 2). Any thread.
pub(crate) fn post_keyboard_event(kind: i32, key_code: u16, flags: u64, marked: bool) -> bool {
    // SAFETY: as for `post_key`.
    unsafe { ck_glfw_test_post_keyboard_event(kind, key_code, flags, i32::from(marked)) != 0 }
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

/// Starts the trace of what the application receives (events and notifications, with times), or restarts
/// its clock. Main thread.
pub(crate) fn trace_start() {
    // SAFETY: registers an event monitor and a notification observer once, on the main thread.
    unsafe { ck_glfw_test_trace_start() };
}

/// The window's number, for [`post_mouse_moved`]. Main thread.
pub(crate) fn window_number() -> i64 {
    // SAFETY: a plain AppKit call on the main thread.
    unsafe { ck_glfw_test_window_number() }
}

/// Posts a mouse-moved event for the window: a system event, such as the window system sends when a window
/// is ordered or activated under the pointer, which ends a blocking wait with nothing to hand over. Any
/// thread.
pub(crate) fn post_mouse_moved(window_number: i64) {
    // SAFETY: `NSApp postEvent:` is documented as callable from any thread.
    unsafe { ck_glfw_test_post_mouse_moved(window_number) };
}

/// Adds a marker line to the trace. Any thread.
pub(crate) fn trace_mark(text: &str) {
    let text = CString::new(text).expect("no NUL in a marker");
    // SAFETY: the text is NUL-terminated and valid for the call.
    unsafe { ck_glfw_test_trace_mark(text.as_ptr()) };
}

/// The trace so far, one line per entry; reading clears it. Any thread.
pub(crate) fn trace_take() -> String {
    let mut buffer = vec![0_u8; 64 * 1024];
    // SAFETY: the buffer is valid for `capacity` bytes, which the native call does not exceed.
    let written =
        unsafe { ck_glfw_test_trace_take(buffer.as_mut_ptr().cast::<c_char>(), buffer.len()) };
    String::from_utf8_lossy(&buffer[..written.min(buffer.len())]).into_owned()
}

/// Uses `scale` in place of the window's backing scale factor (0 restores it).
pub(crate) fn set_scale(scale: f64) {
    // SAFETY: a plain AppKit method replacement and an atomic store, on the main thread.
    unsafe { ck_glfw_test_set_scale(scale) };
}

/// Makes `AppKit` tell the window's views that the backing properties changed. Main thread.
pub(crate) fn backing_changed() {
    // SAFETY: a plain AppKit call on the main thread.
    unsafe { ck_glfw_test_backing_changed() };
}

/// Counts the calls to `-[NSApplication setActivationPolicy:]` and, while `refuse`, makes each return NO. Main
/// thread, before the backend is created.
pub(crate) fn watch_policy(refuse: bool) {
    // SAFETY: a plain Objective-C runtime method replacement (once) and an atomic store, on the main thread.
    unsafe { ck_glfw_test_watch_policy(i32::from(refuse)) };
}

/// How many times `setActivationPolicy:` was called since [`watch_policy`] first ran.
pub(crate) fn policy_calls() -> u64 {
    // SAFETY: an atomic load.
    unsafe { ck_glfw_test_policy_calls() }
}

/// Counts the calls to `-[NSApplication activate]`, which GLFW does not make itself (it uses
/// `activateIgnoringOtherApps:`), so a call is the backend's own request. `false` when this system has no
/// such selector (before macOS 14). Main thread.
pub(crate) fn watch_activate() -> bool {
    // SAFETY: a plain Objective-C runtime method replacement (once), on the main thread.
    unsafe { ck_glfw_test_watch_activate() != 0 }
}

/// How many times `activate` was called since [`watch_activate`] first ran.
pub(crate) fn activate_calls() -> u64 {
    // SAFETY: an atomic load.
    unsafe { ck_glfw_test_activate_calls() }
}

/// Whether GLFW is initialised, read through the one GLFW function that answers NULL when it is not (a
/// primary monitor exists whenever a display does). Main thread.
pub(crate) fn glfw_is_initialised() -> bool {
    // SAFETY: `glfwGetPrimaryMonitor` may be called on the main thread whether or not GLFW is initialised
    // (before `glfwInit` it reports an error, which the library's error callback records, and returns NULL).
    !unsafe { glfwGetPrimaryMonitor() }.is_null()
}

/// GLFW's close flag for the window `handle` (a `GLFWwindow` pointer). Main thread.
pub(crate) fn should_close(window: *mut c_void) -> bool {
    // SAFETY: the pointer is the live window of the backend under test and this is the main thread.
    unsafe { glfwWindowShouldClose(window) != 0 }
}

// ---------------------------------------------------------------------------------------------
// The Vulkan loader, through dlopen
// ---------------------------------------------------------------------------------------------

/// `RTLD_NOW`.
const RTLD_NOW: c_int = 2;

/// `vkCreateInstance`, `vkDestroyInstance` and `vkDestroySurfaceKHR` as the loader exports them.
type CreateInstance =
    unsafe extern "C" fn(*const InstanceInfo, *const c_void, *mut *mut c_void) -> c_int;
type DestroyInstance = unsafe extern "C" fn(*mut c_void, *const c_void);
type DestroySurface = unsafe extern "C" fn(*mut c_void, u64, *const c_void);

/// The loader's entry points the cases use.
pub(crate) struct Loader {
    pub(crate) get_instance_proc_addr: GetInstanceProcAddr,
    pub(crate) create_instance: CreateInstance,
    pub(crate) destroy_instance: DestroyInstance,
    pub(crate) destroy_surface: DestroySurface,
}

/// `VkInstanceCreateInfo`.
#[repr(C)]
pub(crate) struct InstanceInfo {
    pub(crate) s_type: i32,
    pub(crate) next: *const c_void,
    pub(crate) flags: u32,
    pub(crate) application_info: *const c_void,
    pub(crate) layer_count: u32,
    pub(crate) layers: *const *const c_char,
    pub(crate) extension_count: u32,
    pub(crate) extensions: *const *const c_char,
}

/// `VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO` and `VK_INSTANCE_CREATE_ENUMERATE_PORTABILITY_BIT_KHR`.
pub(crate) const INSTANCE_CREATE_INFO: i32 = 1;
pub(crate) const ENUMERATE_PORTABILITY: u32 = 1;

fn symbol(handle: *mut c_void, name: &str) -> Result<*mut c_void, String> {
    let name = CString::new(name).expect("no NUL in a symbol name");
    // SAFETY: the handle is a live `dlopen` handle and the name is NUL-terminated.
    let found = unsafe { dlsym(handle, name.as_ptr()) };
    if found.is_null() {
        Err(format!("the Vulkan loader has no {name:?}"))
    } else {
        Ok(found)
    }
}

/// Where the Vulkan loader is installed (the directories `tools/ck doctor` searches), by absolute path.
const LOADER_PATHS: [&str; 2] = [
    "/usr/local/lib/libvulkan.1.dylib",
    "/opt/homebrew/lib/libvulkan.1.dylib",
];

/// Whether `dlopen` finds the loader by its leaf name `libvulkan.1.dylib`, which is how GLFW looks when it is
/// given no loader of its own.
pub(crate) fn leaf_name_is_found() -> bool {
    let name = CString::new("libvulkan.1.dylib").expect("no NUL in a library name");
    // SAFETY: the name is NUL-terminated and valid for the call.
    !unsafe { dlopen(name.as_ptr(), RTLD_NOW) }.is_null()
}

/// Opens the Vulkan loader by the absolute path the doctor reports. It is never closed.
pub(crate) fn open_loader() -> Result<Loader, String> {
    let path = LOADER_PATHS
        .iter()
        .find(|path| std::path::Path::new(path).exists())
        .ok_or_else(|| {
            format!(
                "no Vulkan loader in {}; run `tools/ck doctor`",
                LOADER_PATHS.join(" or ")
            )
        })?;
    let c_path = CString::new(*path).expect("no NUL in a path");
    // SAFETY: the path is NUL-terminated and valid for the call.
    let handle = unsafe { dlopen(c_path.as_ptr(), RTLD_NOW) };
    if handle.is_null() {
        // SAFETY: `dlerror` returns a NUL-terminated message or null, valid until the next dl call.
        let why = unsafe { dlerror() };
        let text = if why.is_null() {
            "unknown error".to_owned()
        } else {
            // SAFETY: non-null and NUL-terminated, read before the next dl call.
            unsafe { CStr::from_ptr(why) }
                .to_string_lossy()
                .into_owned()
        };
        return Err(format!("{path} could not be opened: {text}"));
    }
    let proc_addr = symbol(handle, "vkGetInstanceProcAddr")?;
    let create = symbol(handle, "vkCreateInstance")?;
    let destroy = symbol(handle, "vkDestroyInstance")?;
    let surface = symbol(handle, "vkDestroySurfaceKHR")?;
    // SAFETY: the symbol is the loader's `vkGetInstanceProcAddr`, whose C signature `GetInstanceProcAddr`
    // describes.
    let get_instance_proc_addr =
        unsafe { std::mem::transmute::<*mut c_void, GetInstanceProcAddr>(proc_addr) };
    // SAFETY: the symbol is the loader's `vkCreateInstance`, described by `CreateInstance`.
    let create_instance = unsafe { std::mem::transmute::<*mut c_void, CreateInstance>(create) };
    // SAFETY: the symbol is the loader's `vkDestroyInstance`, described by `DestroyInstance`.
    let destroy_instance = unsafe { std::mem::transmute::<*mut c_void, DestroyInstance>(destroy) };
    // SAFETY: the symbol is the loader's `vkDestroySurfaceKHR`, described by `DestroySurface`.
    let destroy_surface = unsafe { std::mem::transmute::<*mut c_void, DestroySurface>(surface) };
    Ok(Loader {
        get_instance_proc_addr,
        create_instance,
        destroy_instance,
        destroy_surface,
    })
}

/// The process's one loader, opened on first use; a failure to open it fails the case.
pub(crate) fn loader() -> &'static Loader {
    static LOADER: std::sync::OnceLock<Loader> = std::sync::OnceLock::new();
    LOADER.get_or_init(|| open_loader().expect("the Vulkan loader opens"))
}

static PROC_ADDR_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The loader's `vkGetInstanceProcAddr`, counted: it forwards to the real one. The backend is given this
/// function, and the cases require the count to move, which proves GLFW used the function it was given and
/// did not find the loader itself (the loader image is already loaded, so GLFW's own search would succeed).
pub(crate) unsafe extern "C" fn counting_proc_addr(
    instance: *mut c_void,
    name: *const c_char,
) -> *mut c_void {
    PROC_ADDR_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    // SAFETY: GLFW calls this with the arguments the C function takes, and the function is the live
    // loader's own.
    unsafe { (loader().get_instance_proc_addr)(instance, name) }
}

/// How many times GLFW called [`counting_proc_addr`].
pub(crate) fn proc_addr_calls() -> u64 {
    PROC_ADDR_CALLS.load(std::sync::atomic::Ordering::SeqCst)
}

/// What keeps the loader's library loaded for one backend: its own `dlopen` handle (by the absolute path),
/// closed when the backend lets go of it. `dropped` says whether that has happened.
struct Keepalive {
    handle: *mut c_void,
    dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

// SAFETY: the handle is an opaque `dlopen` handle; `dlclose` may run on any thread, and nothing else
// touches it.
unsafe impl Send for Keepalive {}

impl Drop for Keepalive {
    fn drop(&mut self) {
        // SAFETY: the handle came from `dlopen` and is closed once, here.
        unsafe { dlclose(self.handle) };
        self.dropped
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A loader value for the backend: the counting `vkGetInstanceProcAddr` and a keep-alive that owns a fresh
/// `dlopen` handle of the loader (opened by the absolute path). The flag is set when the keep-alive is
/// released.
pub(crate) fn loader_for_glfw() -> (
    VulkanLoaderForGlfw,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let path = LOADER_PATHS
        .iter()
        .find(|path| std::path::Path::new(path).exists())
        .expect("the Vulkan loader is installed");
    let c_path = CString::new(*path).expect("no NUL in a path");
    // SAFETY: the path is NUL-terminated and valid for the call.
    let handle = unsafe { dlopen(c_path.as_ptr(), RTLD_NOW) };
    assert!(!handle.is_null(), "the Vulkan loader opens");
    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let keepalive = Keepalive {
        handle,
        dropped: std::sync::Arc::clone(&dropped),
    };
    // SAFETY: `counting_proc_addr` forwards to the loader's own `vkGetInstanceProcAddr` (the process's
    // loader, opened by `loader` and never closed), and the keep-alive holds the loader loaded.
    let value = unsafe { VulkanLoaderForGlfw::new(counting_proc_addr, Box::new(keepalive)) };
    (value, dropped)
}
