//! The GLFW window backend: GLFW behind `ck-window`'s [`Backend`](ck_window::Backend), and the raw
//! handles the Vulkan side needs.
//!
//! The binding is this crate's own and minimal (`ffi.rs`: the dozens of GLFW functions it calls, no
//! other GLFW crate). It links GLFW (3.4 or any newer 3.x) from the Homebrew keg `tools/bootstrap`
//! installs; see `build.rs` for the one rule. The crate depends on no GPU crate and on `ash` not at all:
//! the window and the surface function cross the boundary as raw pointers and integers, through the one
//! documented `unsafe` constructor on the consuming side.
//!
//! # The Vulkan loader
//!
//! GLFW needs the Vulkan loader for `glfwGetRequiredInstanceExtensions` and surface creation, and by
//! default it opens `libvulkan.1.dylib` by that leaf name, which on macOS only finds the loader where the
//! dynamic linker's search path says (not in `/usr/local/lib`, where the Vulkan SDK installs it). So the
//! program does not depend on a search path: [`GlfwBackend::new`] takes an optional [`VulkanLoaderForGlfw`], the
//! loader's `vkGetInstanceProcAddr` (a plain function-pointer type, [`GetInstanceProcAddr`]; no `ash` and no GPU
//! crate) together with the value that keeps the loader's library loaded, built by one `unsafe` constructor.
//! The backend hands the pointer, or NULL when none is given, to `glfwInitVulkanLoader` before `glfwInit`, so
//! GLFW never searches. GLFW keeps that setting across `glfwTerminate`, so the backend sets NULL again, and
//! only then releases the keep-alive, after `glfwTerminate` and when `glfwInit` fails; a retained backend (an
//! unproven end) keeps the keep-alive with it. Whoever owns the Vulkan side loads the loader by an absolute
//! path (the one `tools/ck doctor` reports); with `None` GLFW's own search applies, and a failure to find the
//! loader is reported by name.
//!
//! # The menu bar and the regular application (macOS)
//!
//! [`GlfwBackend::new`] sets the `GLFW_COCOA_MENUBAR` init hint false, so there is no menu bar, no key
//! equivalent and Cmd-Q does nothing. GLFW ties a second effect to the same hint: **only with the menu bar
//! does `glfwInit` give an unbundled process the regular activation policy** (GLFW's "make us a proper UI
//! application"). The `codekarl` binary is unbundled, so without more the process keeps a non-regular
//! policy: no Dock icon, not in Cmd-Tab, and it cannot become the active application, so its window is drawn
//! but stays behind the terminal and never receives key events. The backend therefore sets the regular
//! policy itself after `glfwInit` (leaving it when it is already regular) and, when the window is created,
//! asks for the application to be activated and the window to be key and in front. Those two `AppKit` calls
//! are a small Objective-C source in `native/`, built into the library by `build.rs` (its own archive,
//! separate from the test support's); an activation request may be declined by the system and is not waited
//! for.
//!
//! # The main thread
//!
//! GLFW's window and event functions are main-thread functions. [`GlfwBackend`] is neither `Send` nor
//! `Sync` and [`GlfwBackend::new`] refuses to run on any thread that is not the process's main thread
//! (Rust names it `main`). The one thing that crosses threads is the wake target
//! [`wake_target`](ck_window::Backend::wake_target).
//!
//! # Events: callbacks only record
//!
//! GLFW delivers events by callback from inside `glfwPollEvents` / `glfwWaitEvents`, and those drain every
//! pending native event in one call (design §4.2): the accepted [`Backend`](ck_window::Backend) contract
//! permits that, and nothing here claims to bound it. Every callback only **records** into a fixed-size
//! [`Pending`](crate) state in constant work and never calls GLFW: the close callback, the key callback
//! (Escape, action press; a repeat only while no request is pending), the framebuffer-size, content-scale
//! and refresh callbacks. After GLFW returns, the pump (on the main thread, outside every callback) does
//! the work the records need: it resets the close flag (`glfwSetWindowShouldClose(false)`, which GLFW
//! permits after its close callback), queries the framebuffer size when the scale changed, and hands over
//! **at most one close, one pixel size and one needs-display**, the first close source and the latest of
//! the others. `cap` bounds that hand-over, not GLFW's draining. Further close presses are counted
//! (saturating) and logged once per pump as a `debug` record in category `window.glfw`.
//!
//! **No sleeping with pending state.** A `pump(UntilEvent)` that starts with state already waiting (from
//! the callbacks of an earlier GLFW call) does not wait: it polls and hands the state over. The wake is
//! GLFW's `glfwPostEmptyEvent`, which stays queued, so `glfwWaitEvents` entered after a wake returns at
//! once; the pump consumes its own wake events and hands over only what the callbacks recorded.
//!
//! # Closing and teardown
//!
//! The close flag GLFW sets before the close callback is reset in the pump, so the window never closes by
//! itself and stays until the loop's teardown. Teardown is `destroy_window` then `terminate`, only
//! outside callbacks and after the notifier is disarmed (design §4.3, §7). **The surface and the instance
//! belong to the GPU side and are destroyed before `destroy_window`**; this crate documents that order
//! and [`terminate`](ck_window::Backend::terminate) refuses to terminate while a window exists (it
//! records an `error` and returns, counted in [`GlfwBackend::refused_terminations`]).
//!
//! # Tests while the owner types
//!
//! The tests open real windows that may take focus while the owner types, Escape included, and give the
//! same result however much is typed. **None of the test support is part of this library**: it lives in
//! `tests/` and its native part is built into a separate archive that only the test executable links; a
//! test checks the library's rlib for its symbols.

mod activation;
mod backend;
mod ffi;
mod loader;
mod pending;
mod wake;

pub use backend::{GlfwBackend, GlfwHandles, PumpReport, WakeCounts};
pub use loader::{GetInstanceProcAddr, VulkanLoaderForGlfw};
