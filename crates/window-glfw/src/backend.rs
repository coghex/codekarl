//! The backend: `ck-window`'s [`Backend`] on GLFW.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ck_log::Level;
use ck_window::{Backend, CreatedWindow, NativeEvent, NativeWake, PixelSize, Wait, WindowDesc};

use crate::activation;
use crate::ffi::{self, GlfwWindow};
use crate::loader::{Glfw, Installed, VulkanLoaderForGlfw};
use crate::pending::{Pending, effective_wait, key_from_glfw, timeout_seconds};
use crate::wake::GlfwWake;

/// How long a record may wait for the logger: the main thread never waits on it longer.
const LOG_WAIT: Duration = Duration::from_millis(100);

/// The oldest GLFW this crate accepts (3.4); any newer 3.x is accepted too.
const MINIMUM: (c_int, c_int) = (3, 4);

/// Whether a backend exists in this process: GLFW is initialised once, by one backend at a time.
static LIVE: AtomicBool = AtomicBool::new(false);

/// The latest error GLFW reported through its error callback, which only records.
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

extern "C" fn on_error(code: c_int, description: *const c_char) {
    let text = if description.is_null() {
        String::from("no description")
    } else {
        // SAFETY: GLFW passes a NUL-terminated string that is valid for the duration of the callback.
        unsafe { CStr::from_ptr(description) }
            .to_string_lossy()
            .into_owned()
    };
    *LAST_ERROR.lock().unwrap_or_else(PoisonError::into_inner) =
        Some(format!("GLFW error {code:#010X}: {text}"));
}

/// Takes the error GLFW last reported, if any.
fn take_error() -> Option<String> {
    LAST_ERROR
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
}

/// The text of the last GLFW error, or `fallback` when GLFW reported none.
fn error_or(fallback: &str) -> String {
    take_error().unwrap_or_else(|| fallback.to_owned())
}

/// Whether a thread id and a process id name the main thread: on Linux the main thread is the one whose
/// thread id is the process id. A pure function, so its logic is tested on every platform.
#[cfg(any(target_os = "linux", test))]
const fn is_the_process_thread(thread_id: i64, process_id: i64) -> bool {
    thread_id == process_id
}

/// Whether the calling thread is the process's main thread, by identity and not by name: a thread
/// built with the name `main` is not it, and no thread can designate itself.
///
/// On macOS this is `pthread_main_np()`, the call GLFW's own Cocoa code relies on; on Linux it is
/// `gettid() == getpid()`. On any other target there is no supported answer and
/// [`on_main_thread`] refuses before this is asked.
#[cfg(target_os = "macos")]
fn is_main_thread() -> bool {
    unsafe extern "C" {
        fn pthread_main_np() -> c_int;
    }
    // SAFETY: `pthread_main_np` takes no argument, touches no memory of ours and may be called from any
    // thread.
    unsafe { pthread_main_np() != 0 }
}

#[cfg(target_os = "linux")]
fn is_main_thread() -> bool {
    unsafe extern "C" {
        fn gettid() -> c_int;
        fn getpid() -> c_int;
    }
    // SAFETY: neither call takes an argument or touches memory of ours, and both may be called from any
    // thread.
    let (thread, process) = unsafe { (gettid(), getpid()) };
    is_the_process_thread(i64::from(thread), i64::from(process))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn is_main_thread() -> bool {
    false
}

/// Whether this target is one whose main thread can be told (macOS now, Linux later).
const SUPPORTED_PLATFORM: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// Refuses an unsupported platform and any thread that is not the process's main thread, before any GLFW
/// call.
fn on_main_thread() -> Result<(), String> {
    if !SUPPORTED_PLATFORM {
        return Err(format!(
            "unsupported platform: GLFW windows are supported on macOS and Linux only, not {}",
            std::env::consts::OS
        ));
    }
    if is_main_thread() {
        Ok(())
    } else {
        let thread = std::thread::current();
        Err(format!(
            "GLFW windows belong to the main thread; this is thread {:?}",
            thread.name().unwrap_or("<unnamed>")
        ))
    }
}

/// The callback state behind a window's user pointer.
#[derive(Debug)]
struct CallbackState {
    pending: Pending,
    wake: Arc<GlfwWake>,
}

/// Gets the callback state behind a live window's user pointer.
///
/// # Safety
///
/// `window` is the window of a live [`GlfwBackend`] and the call is made by GLFW, on the main thread,
/// from inside one of the backend's own `glfwPollEvents` / `glfwWaitEvents` / window calls.
unsafe fn callback_state_of<'a>(window: *mut GlfwWindow) -> Option<&'a CallbackState> {
    // SAFETY: by the contract above, the user pointer is the address of the backend's boxed callback
    // state, which outlives the window (the backend clears the pointer and destroys the window before it
    // drops the box) and is only ever used from the main thread.
    let pointer = unsafe { ffi::glfwGetWindowUserPointer(window) };
    // SAFETY: as above; a null pointer (no state attached) gives `None`.
    unsafe { pointer.cast::<CallbackState>().as_ref() }
}

extern "C" fn on_close(window: *mut GlfwWindow) {
    // SAFETY: called by GLFW on the main thread for a window of a live backend (see `callback_state_of`).
    if let Some(state) = unsafe { callback_state_of(window) } {
        state.pending.record_close();
    }
}

extern "C" fn on_key(
    window: *mut GlfwWindow,
    key: c_int,
    _scancode: c_int,
    action: c_int,
    _mods: c_int,
) {
    let Some(key) = key_from_glfw(key) else {
        return;
    };
    // SAFETY: called by GLFW on the main thread for a window of a live backend (see `callback_state_of`).
    if let Some(state) = unsafe { callback_state_of(window) } {
        match action {
            ffi::PRESS => state.pending.record_key(key, true, false),
            ffi::RELEASE => state.pending.record_key(key, false, false),
            ffi::REPEAT => state.pending.record_key(key, true, true),
            _ => {}
        }
    }
}

extern "C" fn on_focus(window: *mut GlfwWindow, focused: c_int) {
    // SAFETY: called by GLFW on the main thread for a window of a live backend (see `callback_state_of`).
    if let Some(state) = unsafe { callback_state_of(window) } {
        if focused == ffi::TRUE {
            state.pending.record_focus_gained();
        } else {
            state.pending.record_focus_lost();
        }
    }
}

extern "C" fn on_iconify(window: *mut GlfwWindow, iconified: c_int) {
    // SAFETY: called by GLFW on the main thread for a window of a live backend (see `callback_state_of`).
    if let Some(state) = unsafe { callback_state_of(window) } {
        state.pending.record_minimized(iconified == ffi::TRUE);
        if iconified == ffi::FALSE {
            // A restore can arrive while the application is asleep in GLFW's event wait. The wake is
            // sticky, so the loop re-evaluates its minimise gate after this callback returns.
            state.wake.wake();
        }
    }
}

extern "C" fn on_size(window: *mut GlfwWindow, width: c_int, height: c_int) {
    // SAFETY: called by GLFW on the main thread for a window of a live backend (see `callback_state_of`).
    if let Some(state) = unsafe { callback_state_of(window) } {
        state.pending.record_size(
            u32::try_from(width).unwrap_or(0),
            u32::try_from(height).unwrap_or(0),
        );
    }
}

extern "C" fn on_scale(window: *mut GlfwWindow, _x: f32, _y: f32) {
    // SAFETY: called by GLFW on the main thread for a window of a live backend (see `callback_state_of`).
    if let Some(state) = unsafe { callback_state_of(window) } {
        state.pending.record_scale_changed();
    }
}

extern "C" fn on_refresh(window: *mut GlfwWindow) {
    // SAFETY: called by GLFW on the main thread for a window of a live backend (see `callback_state_of`).
    if let Some(state) = unsafe { callback_state_of(window) } {
        state.pending.record_display();
    }
}

/// What one [`pump`](Backend::pump) handed over: ordered key/focus events, the latest iconify state,
/// at most one close request and the latest size/redraw state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PumpReport {
    /// Normalised key and focus events, in their recorded order.
    pub input: Vec<NativeEvent>,
    /// The close request, if any: [`NativeEvent::CloseButton`].
    pub close: Option<NativeEvent>,
    /// The latest pixel size, when it changed. A zero size (a minimised window) is reported as such.
    pub pixel_size: Option<PixelSize>,
    /// Whether the window needs to be drawn again.
    pub needs_display: bool,
    /// Whether the window was minimised during this pump.
    pub minimized: bool,
    /// Whether the window was restored during this pump.
    pub restored: bool,
    /// Further close-type presses absorbed behind the first (saturating).
    pub absorbed_close_presses: u32,
}

/// How many wakes reached GLFW and how many found it not initialised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeCounts {
    /// Wakes posted with `glfwPostEmptyEvent`.
    pub posted: u64,
    /// Wakes made while GLFW was not initialised, which did nothing.
    pub ignored: u64,
}

/// The native handles of a created window, for the Vulkan side.
#[derive(Debug)]
pub struct GlfwHandles {
    window: NonNull<GlfwWindow>,
    extensions: Vec<String>,
}

impl GlfwHandles {
    /// The `GLFWwindow` pointer. It is valid until [`destroy_window`](Backend::destroy_window) and is
    /// used on the main thread only.
    #[must_use]
    pub fn window(&self) -> *mut c_void {
        self.window.as_ptr().cast::<c_void>()
    }

    /// The instance extensions GLFW requires for a surface on this window, copied while GLFW was
    /// initialised and without duplicates.
    #[must_use]
    pub fn required_instance_extensions(&self) -> &[String] {
        &self.extensions
    }

    /// Creates the window's `VkSurfaceKHR` on `instance` with `glfwCreateWindowSurface` and returns the
    /// surface handle. The Vulkan side owns the surface, and destroys it, and the instance, **before**
    /// [`destroy_window`](Backend::destroy_window).
    ///
    /// # Errors
    ///
    /// An error when not called on the main thread, or the `VkResult` and GLFW's message when surface
    /// creation fails.
    ///
    /// # Safety
    ///
    /// `instance` must be a live `VkInstance` created with at least the extensions in
    /// [`required_instance_extensions`](Self::required_instance_extensions), and this window must not
    /// have been destroyed (the handles must not outlive the backend's window).
    pub unsafe fn create_surface(&self, instance: *mut c_void) -> Result<u64, String> {
        on_main_thread()?;
        let mut surface = 0_u64;
        // SAFETY: by this function's contract the instance is live and the window is alive; this runs on
        // the main thread; `surface` is valid for the call; no allocator is passed.
        let result = unsafe {
            ffi::glfwCreateWindowSurface(
                instance,
                self.window.as_ptr(),
                std::ptr::null(),
                &raw mut surface,
            )
        };
        if result == ffi::VK_SUCCESS {
            Ok(surface)
        } else {
            Err(format!(
                "glfwCreateWindowSurface failed with VkResult {result}: {}",
                error_or("no GLFW error")
            ))
        }
    }
}

/// Leaves the process as it was before `glfwInit` after a failure that comes after it: GLFW terminated (no
/// window exists yet), the loader reset to NULL and then released, and the backend slot free again.
fn abandon(installed: Installed<Glfw>) {
    // SAFETY: GLFW was just initialised on this thread and no window exists.
    unsafe { ffi::glfwTerminate() };
    LIVE.store(false, Ordering::SeqCst);
    installed.release();
}

/// The GLFW window backend. It owns the application's one window and must stay on the process's main
/// thread: it is neither `Send` nor `Sync`. The one thing that crosses threads is the wake target
/// [`wake_target`](Backend::wake_target) returns.
#[derive(Debug)]
pub struct GlfwBackend {
    /// The callbacks' recorded state and the wake they may post; its address is the window's user pointer.
    callback: Box<CallbackState>,
    window: Option<NonNull<GlfwWindow>>,
    wake: Arc<GlfwWake>,
    initialised: bool,
    /// The Vulkan loader GLFW was given, held until after `glfwTerminate` and the NULL reset.
    loader: Option<Installed<Glfw>>,
    refused: u64,
    /// Neither `Send` nor `Sync`: every call stays on the thread that created the backend.
    _main_thread: PhantomData<*const ()>,
}

impl GlfwBackend {
    /// Initialises GLFW: the error callback is installed first, then the Vulkan loader the caller gives
    /// (or NULL when none) is handed to `glfwInitVulkanLoader`, then the `GLFW_COCOA_MENUBAR` init hint is
    /// set false (no menu bar, so Cmd-Q does nothing) and `glfwInit` runs. No window exists yet.
    ///
    /// **On macOS the process becomes a regular application here.** GLFW gives an unbundled process the
    /// regular activation policy only together with its menu bar, so with the menu bar off the backend sets
    /// the policy itself after `glfwInit` (unless it is already regular, as for a second backend in the
    /// process): without it the process has no Dock icon, is not in Cmd-Tab and cannot become the active
    /// application, and its window stays behind whatever is in front. A refused policy fails `new` with
    /// `the regular activation policy could not be set`, leaving the process as a failed `glfwInit` does.
    ///
    /// The backend holds the loader value until after [`terminate`](Backend::terminate) has run
    /// `glfwTerminate` **and** set GLFW's loader back to NULL (GLFW keeps the setting across
    /// `glfwTerminate`); a failed `glfwInit` resets to NULL before the value is released. A backend that is
    /// retained (never dropped, after an unproven end) keeps the value with it.
    ///
    /// With `None`, GLFW searches for the Vulkan loader itself (by its leaf name); if it is not found,
    /// [`create_window`](Backend::create_window) says so.
    ///
    /// # Errors
    ///
    /// An error when not called on the process's main thread (checked before any GLFW call), when
    /// another backend is alive, when `glfwInit` fails, when the library is older than 3.4, or (macOS) when
    /// the regular activation policy cannot be set. After a failure past `glfwInit` GLFW is terminated, the
    /// loader is reset to NULL and then released, and a later backend can be created.
    pub fn new(vulkan_loader: Option<VulkanLoaderForGlfw>) -> Result<Self, String> {
        on_main_thread()?;
        if LIVE.swap(true, Ordering::SeqCst) {
            return Err("a GLFW backend already exists in this process".to_owned());
        }
        // SAFETY: the error callback may be set before `glfwInit`, and on the main thread.
        unsafe { ffi::glfwSetErrorCallback(Some(on_error)) };
        take_error();
        // Before glfwInit: the loader's pointer or NULL. From here every way out resets to NULL first.
        let installed = Installed::before_init(Glfw, vulkan_loader);
        if cfg!(target_os = "macos") {
            // SAFETY: init hints are set before `glfwInit`, on the main thread.
            unsafe { ffi::glfwInitHint(ffi::COCOA_MENUBAR, ffi::FALSE) };
        }
        // SAFETY: called on the main thread, once, with no other GLFW state in this process.
        if unsafe { ffi::glfwInit() } != ffi::TRUE {
            LIVE.store(false, Ordering::SeqCst);
            installed.release();
            return Err(error_or("glfwInit failed"));
        }
        let (mut major, mut minor, mut revision) = (0, 0, 0);
        // SAFETY: GLFW is initialised; the pointers are valid for the call.
        unsafe { ffi::glfwGetVersion(&raw mut major, &raw mut minor, &raw mut revision) };
        if (major, minor) < MINIMUM {
            abandon(installed);
            return Err(format!(
                "GLFW 3.4 or newer 3.x is required, found {major}.{minor}.{revision}"
            ));
        }
        // GLFW makes an unbundled process a regular application only together with its menu bar, which the
        // hint above turned off: without the regular policy the process cannot become the active application.
        if let Err(message) = activation::ensure_regular_policy() {
            abandon(installed);
            return Err(message);
        }
        let wake = Arc::new(GlfwWake::default());
        wake.set_live(true);
        let callback = Box::new(CallbackState {
            pending: Pending::default(),
            wake: Arc::clone(&wake),
        });
        Ok(Self {
            callback,
            window: None,
            wake,
            initialised: true,
            loader: Some(installed),
            refused: 0,
            _main_thread: PhantomData,
        })
    }

    /// The GLFW version in use, as `(major, minor, revision)`.
    #[must_use]
    pub fn glfw_version() -> (i32, i32, i32) {
        let (mut major, mut minor, mut revision) = (0, 0, 0);
        // SAFETY: `glfwGetVersion` may be called before initialisation, from any thread; the pointers
        // are valid for the call.
        unsafe { ffi::glfwGetVersion(&raw mut major, &raw mut minor, &raw mut revision) };
        (major, minor, revision)
    }

    /// The wakes that reached GLFW and the ones that found it not initialised.
    #[must_use]
    pub fn wake_counts(&self) -> WakeCounts {
        WakeCounts {
            posted: self.wake.posted(),
            ignored: self.wake.ignored(),
        }
    }

    /// How many times [`terminate`](Backend::terminate) was called with a window still alive and so did
    /// nothing.
    #[must_use]
    pub fn refused_terminations(&self) -> u64 {
        self.refused
    }

    /// One pump (see [`Backend::pump`]), returning what it handed over together with the count behind the
    /// close. The absorbed close presses are also logged once as a `debug` record in category
    /// `window.glfw`. `cap` bounds nothing here: GLFW drains every pending event inside one call, and the
    /// hand-over is at most one of each kind.
    pub fn pump_with_report(&mut self, wait: Wait, _cap: usize) -> PumpReport {
        if !self.initialised {
            return PumpReport::default();
        }
        // A blocking pump never blocks with state already waiting.
        let wait = effective_wait(wait, self.callback.pending.is_pending());
        match wait {
            // SAFETY: GLFW is initialised and this runs on the main thread (the backend is not Send).
            Wait::Poll => unsafe { ffi::glfwPollEvents() },
            // SAFETY: as above; a wake from any thread ends the wait.
            Wait::UntilEvent => unsafe { ffi::glfwWaitEvents() },
        }
        self.report_after_dispatch()
    }

    fn report_after_dispatch(&mut self) -> PumpReport {
        // GLFW has returned: outside every callback, so GLFW may be called again.
        if let Some(window) = self.window {
            // SAFETY: the window is live (destroyed only by `destroy_window`), on the main thread.
            if unsafe { ffi::glfwWindowShouldClose(window.as_ptr()) } == ffi::TRUE {
                // SAFETY: as above; GLFW permits changing the flag after its close callback.
                unsafe { ffi::glfwSetWindowShouldClose(window.as_ptr(), ffi::FALSE) };
            }
        }
        let taken = self.callback.pending.take();
        let size = match (taken.query_size, self.window) {
            (true, Some(window)) => Some(framebuffer_size(window)),
            _ => taken
                .size
                .map(|(width, height)| PixelSize { width, height }),
        };
        note_absorbed(taken.absorbed);
        PumpReport {
            input: taken.input,
            close: taken.close.map(|_| NativeEvent::CloseButton),
            pixel_size: size,
            needs_display: taken.display,
            minimized: taken.minimized == Some(true),
            restored: taken.minimized == Some(false),
            absorbed_close_presses: taken.absorbed,
        }
    }
}

/// The window's framebuffer size in pixels.
fn framebuffer_size(window: NonNull<GlfwWindow>) -> PixelSize {
    let (mut width, mut height) = (0, 0);
    // SAFETY: the window is live and this runs on the main thread; the pointers are valid for the call.
    unsafe { ffi::glfwGetFramebufferSize(window.as_ptr(), &raw mut width, &raw mut height) };
    PixelSize {
        width: u32::try_from(width).unwrap_or(0),
        height: u32::try_from(height).unwrap_or(0),
    }
}

/// Records one `debug` line for the close presses a pump absorbed, through `log_within`.
fn note_absorbed(count: u32) {
    if count > 0 {
        let at = Instant::now() + LOG_WAIT;
        let _ = ck_log::log_within!(
            Level::Debug,
            at,
            "window.glfw",
            "{count} further close presses were absorbed before the loop took the first"
        );
    }
}

/// The extension names GLFW requires, copied (deduplicated, in order) while GLFW is initialised.
fn required_extensions() -> Result<Vec<String>, String> {
    let mut count = 0_u32;
    // SAFETY: GLFW is initialised and this runs on the main thread; `count` is valid for the call.
    let names = unsafe { ffi::glfwGetRequiredInstanceExtensions(&raw mut count) };
    if names.is_null() {
        return Err(error_or("GLFW found no Vulkan surface extensions"));
    }
    let mut copied: Vec<&str> = Vec::new();
    for index in 0..count as usize {
        // SAFETY: GLFW returned an array of `count` pointers, valid until GLFW terminates; `index` is
        // below `count`.
        let entry = unsafe { *names.wrapping_add(index) };
        // SAFETY: each entry is a NUL-terminated name, copied below before anything else is called.
        let name = unsafe { CStr::from_ptr(entry) };
        copied.push(
            name.to_str()
                .map_err(|_| "a required Vulkan extension name is not UTF-8".to_owned())?,
        );
    }
    Ok(deduplicated(&copied))
}

/// The names without repeats, in the order they first appear.
fn deduplicated(names: &[&str]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for name in names {
        if !found.iter().any(|known| known == name) {
            found.push((*name).to_owned());
        }
    }
    found
}

impl Backend for GlfwBackend {
    type Handles = GlfwHandles;

    /// Creates the fixed-size Vulkan window. **On macOS it then asks to come to the front**: the application
    /// is activated and the window made key and ordered front (as `ck-window-cocoa` does), so it takes keys.
    /// The system may decline an activation request; the backend does not wait for it, retry it or fail on it.
    fn create_window(&mut self, desc: &WindowDesc) -> Result<CreatedWindow<GlfwHandles>, String> {
        on_main_thread()?;
        if !self.initialised {
            return Err("GLFW has been terminated".to_owned());
        }
        if self.window.is_some() {
            return Err("a window already exists".to_owned());
        }
        let title = CString::new(desc.title.as_str())
            .map_err(|_| "the window title contains a NUL byte".to_owned())?;
        let width = c_int::try_from(desc.width).map_err(|_| "the window is too wide".to_owned())?;
        let height =
            c_int::try_from(desc.height).map_err(|_| "the window is too tall".to_owned())?;
        // SAFETY: GLFW is initialised and this runs on the main thread.
        if unsafe { ffi::glfwVulkanSupported() } != ffi::TRUE {
            return Err(
                "GLFW found no Vulkan loader: pass a VulkanLoaderForGlfw to \
                 GlfwBackend::new (`tools/ck doctor` reports where the loader is)"
                    .to_owned(),
            );
        }
        take_error();
        // SAFETY: window hints are set after `glfwInit` and before the window is created, on the main
        // thread. No OpenGL context ...
        unsafe { ffi::glfwWindowHint(ffi::CLIENT_API, ffi::NO_API) };
        // SAFETY: as above: ... and a fixed-size window.
        unsafe { ffi::glfwWindowHint(ffi::RESIZABLE, ffi::FALSE) };
        let activation_allowed = ck_window::window_activation_allowed();
        if !activation_allowed {
            // SAFETY: GLFW hints are set on the main thread before creation. These keep the
            // test-only non-focus window from requesting focus when it is shown.
            unsafe { ffi::glfwWindowHint(ffi::FOCUSED, ffi::FALSE) };
            // SAFETY: as above; suppress focus-on-show as well as initial focus.
            unsafe { ffi::glfwWindowHint(ffi::FOCUS_ON_SHOW, ffi::FALSE) };
        }
        // SAFETY: the title is NUL-terminated and valid for the call; no monitor and no shared window.
        let created = unsafe {
            ffi::glfwCreateWindow(
                width,
                height,
                title.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        let window = NonNull::new(created).ok_or_else(|| error_or("glfwCreateWindow failed"))?;
        let extensions = match required_extensions() {
            Ok(extensions) => extensions,
            Err(message) => {
                // No window may be left behind when `Err` is returned.
                // SAFETY: the window was just created on this thread and nothing else holds it.
                unsafe { ffi::glfwDestroyWindow(window.as_ptr()) };
                return Err(message);
            }
        };
        self.callback.pending.take();
        let state: *const CallbackState = std::ptr::from_ref(&*self.callback);
        // SAFETY: the window is live and this is the main thread. The user pointer is the boxed callback
        // state, which outlives the window; callbacks record state and the restore callback may post its
        // thread-safe sticky wake.
        unsafe {
            ffi::glfwSetWindowUserPointer(window.as_ptr(), state.cast_mut().cast::<c_void>());
        };
        // SAFETY: as above; the key callback only records.
        unsafe { ffi::glfwSetKeyCallback(window.as_ptr(), Some(on_key)) };
        // SAFETY: as above; the close callback only records.
        unsafe { ffi::glfwSetWindowCloseCallback(window.as_ptr(), Some(on_close)) };
        // SAFETY: as above; the framebuffer-size callback only records.
        unsafe { ffi::glfwSetFramebufferSizeCallback(window.as_ptr(), Some(on_size)) };
        // SAFETY: as above; the content-scale callback only records.
        unsafe { ffi::glfwSetWindowContentScaleCallback(window.as_ptr(), Some(on_scale)) };
        // SAFETY: as above; the refresh callback only records.
        unsafe { ffi::glfwSetWindowRefreshCallback(window.as_ptr(), Some(on_refresh)) };
        // SAFETY: as above; the focus callback only records.
        unsafe { ffi::glfwSetWindowFocusCallback(window.as_ptr(), Some(on_focus)) };
        // SAFETY: as above; the iconify callback records the state and posts the sticky restore wake.
        unsafe { ffi::glfwSetWindowIconifyCallback(window.as_ptr(), Some(on_iconify)) };
        let pixel_size = framebuffer_size(window);
        self.window = Some(window);
        // A request the system may decline: not waited for, retried or failed on.
        if activation_allowed && let Err(reason) = activation::request_activation(window) {
            let at = Instant::now() + LOG_WAIT;
            let _ = ck_log::log_within!(
                Level::Warn,
                at,
                "window.glfw",
                "the window could not ask to come to the front: {reason}"
            );
        }
        Ok(CreatedWindow {
            handles: GlfwHandles { window, extensions },
            pixel_size,
        })
    }

    fn wake_target(&self) -> Arc<dyn NativeWake> {
        Arc::clone(&self.wake) as Arc<dyn NativeWake>
    }

    fn pump(&mut self, events: &mut dyn FnMut(NativeEvent), wait: Wait, cap: usize) {
        let report = self.pump_with_report(wait, cap);
        for event in report.input {
            events(event);
        }
        if report.minimized {
            events(NativeEvent::Minimized);
        }
        if report.restored {
            events(NativeEvent::Restored);
        }
        // At most one close, one size and one needs-display, in that order; callbacks only recorded.
        if let Some(close) = report.close {
            events(close);
        }
        if let Some(PixelSize { width, height }) = report.pixel_size {
            events(NativeEvent::PixelSizeChanged { width, height });
        }
        if report.needs_display {
            events(NativeEvent::NeedsDisplay);
        }
    }

    fn supports_deadline(&self) -> bool {
        true
    }

    fn pump_until(&mut self, events: &mut dyn FnMut(NativeEvent), deadline: Instant, _cap: usize) {
        if !self.initialised {
            return;
        }
        let wait = effective_wait(Wait::UntilEvent, self.callback.pending.is_pending());
        match (wait, timeout_seconds(deadline, Instant::now())) {
            (Wait::Poll, _) | (_, None) => {
                // SAFETY: GLFW is initialised and this runs on the main thread (the backend is not Send).
                unsafe { ffi::glfwPollEvents() };
            }
            (Wait::UntilEvent, Some(timeout)) => {
                // SAFETY: GLFW is initialised and this runs on the main thread; timeout is finite and positive.
                unsafe { ffi::glfwWaitEventsTimeout(timeout) };
            }
        }
        let report = self.report_after_dispatch();
        for event in report.input {
            events(event);
        }
        if report.minimized {
            events(NativeEvent::Minimized);
        }
        if report.restored {
            events(NativeEvent::Restored);
        }
        if let Some(close) = report.close {
            events(close);
        }
        if let Some(PixelSize { width, height }) = report.pixel_size {
            events(NativeEvent::PixelSizeChanged { width, height });
        }
        if report.needs_display {
            events(NativeEvent::NeedsDisplay);
        }
    }

    fn destroy_window(&mut self) {
        if let Some(window) = self.window.take() {
            // SAFETY: the window is live and this runs on the main thread, outside every callback. The
            // user pointer is cleared first so no callback can reach the state after this.
            unsafe { ffi::glfwSetWindowUserPointer(window.as_ptr(), std::ptr::null_mut()) };
            // SAFETY: as above; the window is destroyed once and not used again.
            unsafe { ffi::glfwDestroyWindow(window.as_ptr()) };
            self.callback.pending.take();
        }
    }

    fn terminate(&mut self) {
        if self.window.is_some() {
            self.refused += 1;
            let at = Instant::now() + LOG_WAIT;
            let _ = ck_log::log_within!(
                Level::Error,
                at,
                "window.glfw",
                "terminate refused: a window still exists (destroy it first)"
            );
            return;
        }
        if self.initialised {
            // Wakes stop first: a wake after this finds GLFW not initialised and does nothing.
            self.wake.set_live(false);
            // SAFETY: GLFW is initialised, no window exists and this is the main thread, outside every
            // callback; the loop's notifier was disarmed (no wake in flight) before this.
            unsafe { ffi::glfwTerminate() };
            self.initialised = false;
            // GLFW keeps its loader setting across termination: NULL it, and only then release the value.
            if let Some(installed) = self.loader.take() {
                installed.release();
            }
            LIVE.store(false, Ordering::SeqCst);
        }
    }
}

impl Drop for GlfwBackend {
    fn drop(&mut self) {
        self.destroy_window();
        self.terminate();
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use super::*;

    #[test]
    fn extension_names_are_deduplicated_keeping_their_first_order() {
        assert_eq!(
            deduplicated(&["VK_KHR_surface", "VK_EXT_metal_surface", "VK_KHR_surface"]),
            ["VK_KHR_surface", "VK_EXT_metal_surface"]
        );
        assert!(deduplicated(&[]).is_empty());
    }

    #[test]
    fn a_thread_that_is_not_the_main_thread_is_refused_before_any_glfw_call() {
        // The test harness runs this on a thread named after the test, not `main`.
        let message = on_main_thread().expect_err("not the main thread");
        assert!(message.contains("main thread"), "{message}");
        let refused = GlfwBackend::new(None).expect_err("a backend off the main thread");
        assert!(refused.contains("main thread"), "{refused}");
        assert!(
            !LIVE.load(Ordering::SeqCst),
            "a refused backend leaves no state behind"
        );
    }

    #[test]
    fn the_linux_predicate_accepts_only_the_thread_whose_id_is_the_process_id() {
        assert!(is_the_process_thread(4242, 4242));
        assert!(
            !is_the_process_thread(4243, 4242),
            "a worker has its own thread id"
        );
        assert!(!is_the_process_thread(4241, 4242));
        assert!(!is_the_process_thread(0, 4242));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_supported_platform_reports_no_unsupported_error() {
        assert!(SUPPORTED_PLATFORM);
        let refused = on_main_thread().expect_err("a test thread is not the main thread");
        assert!(!refused.contains("unsupported platform"), "{refused}");
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    #[test]
    fn any_other_platform_is_refused_as_unsupported_before_any_glfw_call() {
        let refused = GlfwBackend::new(None).expect_err("an unsupported platform");
        assert!(refused.contains("unsupported platform"), "{refused}");
        assert!(!LIVE.load(Ordering::SeqCst));
    }

    #[test]
    fn an_unnamed_worker_and_one_named_main_are_both_refused_with_no_state_changed() {
        use std::sync::atomic::AtomicBool;

        struct Released(Arc<AtomicBool>);
        impl Drop for Released {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        unsafe extern "C" fn never_called(_: *mut c_void, _: *const c_char) -> *mut c_void {
            std::ptr::null_mut()
        }

        for name in [None, Some("main")] {
            let builder = match name {
                Some(name) => std::thread::Builder::new().name(name.to_owned()),
                None => std::thread::Builder::new(),
            };
            let released = Arc::new(AtomicBool::new(false));
            let watched = Arc::clone(&released);
            let outcome = builder
                .spawn(move || {
                    // SAFETY: the function is never called: the backend refuses before any GLFW call.
                    let loader = unsafe {
                        VulkanLoaderForGlfw::new(never_called, Box::new(Released(watched)))
                    };
                    (
                        is_main_thread(),
                        GlfwBackend::new(Some(loader)).err(),
                        LIVE.load(Ordering::SeqCst),
                    )
                })
                .expect("a thread")
                .join()
                .expect("the thread ended");
            let (is_main, refused, live) = outcome;
            assert!(!is_main, "{name:?} is not the main thread");
            let refused = refused.expect("refused");
            assert!(refused.contains("main thread"), "{name:?}: {refused}");
            assert!(!live, "{name:?}: the single-backend flag was not taken");
            assert!(
                released.load(Ordering::SeqCst),
                "{name:?}: the refused loader value was released at once"
            );
            assert!(!LIVE.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn glfw_errors_are_recorded_with_their_code_and_returned_once() {
        take_error();
        let text = CString::new("the window could not be made").expect("no NUL");
        on_error(0x0001_0003, text.as_ptr());
        assert_eq!(
            error_or("none"),
            "GLFW error 0x00010003: the window could not be made"
        );
        assert_eq!(error_or("none"), "none", "an error is returned once");
        on_error(7, std::ptr::null());
        assert_eq!(error_or("none"), "GLFW error 0x00000007: no description");
    }
}
