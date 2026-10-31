//! The Vulkan loader GLFW is told to use, and the order in which GLFW is told.
//!
//! GLFW keeps the pointer given to `glfwInitVulkanLoader` across `glfwTerminate`, and calls it during
//! `glfwInit`, window creation and surface creation. So the pointer must never outlive the library it
//! points into: [`Installed`] sets it before `glfwInit`, and sets NULL again **before** the keep-alive that
//! holds the library is released (after `glfwTerminate`, or at once when `glfwInit` fails). A backend that
//! is retained (an unproven end) keeps its [`Installed`] and so the library, and no `Drop` runs.
//!
//! The order is separate from GLFW (a [`Setter`]) so a test can record it: the unit tests below run it
//! against a recording setter, for a loader, none, a loader then none, and a replaced loader in one
//! process.

use std::any::Any;
use std::ffi::{c_char, c_void};
use std::fmt;

use crate::ffi;

/// The Vulkan loader's `vkGetInstanceProcAddr`, as a plain function-pointer type: this crate depends on no
/// Vulkan binding. It is `unsafe` to call, as the C function is.
pub type GetInstanceProcAddr = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void;

/// The loader GLFW is to use: the loader's `vkGetInstanceProcAddr` and what keeps its library loaded.
pub struct VulkanLoaderForGlfw {
    pub(crate) proc_addr: GetInstanceProcAddr,
    /// Held until after `glfwTerminate` and the NULL reset; never read.
    pub(crate) _keepalive: Box<dyn Any + Send>,
}

impl VulkanLoaderForGlfw {
    /// Pairs a loader's `vkGetInstanceProcAddr` with the value that keeps that loader loaded.
    ///
    /// # Safety
    ///
    /// `proc_addr` must be the `vkGetInstanceProcAddr` of a Vulkan loader, with the C ABI, and `keepalive`
    /// must keep that loader loaded for as long as the returned value lives (the backend holds the value
    /// until after `glfwTerminate`, and drops it only after it has set GLFW's loader back to NULL).
    #[must_use]
    pub unsafe fn new(proc_addr: GetInstanceProcAddr, keepalive: Box<dyn Any + Send>) -> Self {
        Self {
            proc_addr,
            _keepalive: keepalive,
        }
    }
}

impl fmt::Debug for VulkanLoaderForGlfw {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VulkanLoaderForGlfw")
    }
}

/// Whatever sets GLFW's loader: GLFW itself, or a recorder in a test.
pub(crate) trait Setter {
    /// `glfwInitVulkanLoader(loader)`: the pointer, or NULL for none.
    fn set(&self, loader: Option<GetInstanceProcAddr>);
}

/// GLFW's `glfwInitVulkanLoader`.
pub(crate) struct Glfw;

impl Setter for Glfw {
    fn set(&self, loader: Option<GetInstanceProcAddr>) {
        // SAFETY: `glfwInitVulkanLoader` may be called before `glfwInit` and after `glfwTerminate`, on the
        // main thread, which is where the backend runs; a given pointer is valid for as long as the
        // `Installed` that set it holds its keep-alive (see the module documentation).
        unsafe { ffi::glfwInitVulkanLoader(loader) };
    }
}

/// The loader as installed: set before `glfwInit`, reset to NULL before the keep-alive is released.
pub(crate) struct Installed<S: Setter> {
    setter: S,
    held: Option<VulkanLoaderForGlfw>,
}

impl<S: Setter> Installed<S> {
    /// Sets the loader's pointer, or NULL when none is given, and holds the value. Call before **every**
    /// `glfwInit`.
    pub(crate) fn before_init(setter: S, loader: Option<VulkanLoaderForGlfw>) -> Self {
        setter.set(loader.as_ref().map(|loader| loader.proc_addr));
        Self {
            setter,
            held: loader,
        }
    }

    /// After `glfwTerminate`, or after a failed `glfwInit`: sets NULL, and only then releases the value.
    pub(crate) fn release(mut self) {
        self.reset();
    }

    fn reset(&mut self) {
        self.setter.set(None);
        drop(self.held.take());
    }
}

impl<S: Setter> fmt::Debug for Installed<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Installed")
            .field("held", &self.held.is_some())
            .finish_non_exhaustive()
    }
}

impl<S: Setter> Drop for Installed<S> {
    fn drop(&mut self) {
        // Not reached by a retained backend (nothing is dropped); for any other drop the order still holds.
        if self.held.is_some() {
            self.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, PoisonError};

    use super::*;

    unsafe extern "C" fn first(_: *mut c_void, _: *const c_char) -> *mut c_void {
        std::ptr::null_mut()
    }

    unsafe extern "C" fn second(_: *mut c_void, _: *const c_char) -> *mut c_void {
        std::ptr::null_mut()
    }

    type Log = Arc<Mutex<Vec<String>>>;

    /// Records every call to the setter, and every release of a keep-alive, in one log.
    struct Recorder(Log);

    impl Setter for Recorder {
        fn set(&self, loader: Option<GetInstanceProcAddr>) {
            let text = match loader {
                None => "set(NULL)".to_owned(),
                Some(pointer) if pointer as usize == first as usize => "set(first)".to_owned(),
                Some(pointer) if pointer as usize == second as usize => "set(second)".to_owned(),
                Some(_) => "set(other)".to_owned(),
            };
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(text);
        }
    }

    struct Keepalive(Log, &'static str);

    impl Drop for Keepalive {
        fn drop(&mut self) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("drop({})", self.1));
        }
    }

    fn loader(
        log: &Log,
        proc_addr: GetInstanceProcAddr,
        name: &'static str,
    ) -> VulkanLoaderForGlfw {
        // SAFETY: nothing here calls the function pointers; the keep-alive stands in for the library.
        unsafe { VulkanLoaderForGlfw::new(proc_addr, Box::new(Keepalive(Arc::clone(log), name))) }
    }

    fn events(log: &Log) -> Vec<String> {
        log.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    #[test]
    fn a_loader_is_set_before_init_and_nulled_before_its_keepalive_is_released() {
        let log = Log::default();
        let installed =
            Installed::before_init(Recorder(Arc::clone(&log)), Some(loader(&log, first, "a")));
        assert_eq!(
            events(&log),
            ["set(first)"],
            "set before glfwInit, nothing released"
        );
        installed.release();
        assert_eq!(events(&log), ["set(first)", "set(NULL)", "drop(a)"]);
    }

    #[test]
    fn none_sets_null_before_init_and_again_after_termination() {
        let log = Log::default();
        Installed::before_init(Recorder(Arc::clone(&log)), None).release();
        assert_eq!(events(&log), ["set(NULL)", "set(NULL)"]);
    }

    #[test]
    fn a_loader_then_none_in_one_process_leaves_no_pointer_behind() {
        let log = Log::default();
        Installed::before_init(Recorder(Arc::clone(&log)), Some(loader(&log, first, "a")))
            .release();
        Installed::before_init(Recorder(Arc::clone(&log)), None).release();
        assert_eq!(
            events(&log),
            [
                "set(first)",
                "set(NULL)",
                "drop(a)",
                "set(NULL)",
                "set(NULL)"
            ]
        );
    }

    #[test]
    fn a_replaced_loader_is_nulled_and_released_before_the_next_is_set() {
        let log = Log::default();
        Installed::before_init(Recorder(Arc::clone(&log)), Some(loader(&log, first, "a")))
            .release();
        Installed::before_init(Recorder(Arc::clone(&log)), Some(loader(&log, second, "b")))
            .release();
        assert_eq!(
            events(&log),
            [
                "set(first)",
                "set(NULL)",
                "drop(a)",
                "set(second)",
                "set(NULL)",
                "drop(b)"
            ]
        );
    }

    #[test]
    fn a_failed_init_nulls_before_the_value_is_released() {
        // The backend releases at once when `glfwInit` fails: the same call, the same order.
        let log = Log::default();
        let installed =
            Installed::before_init(Recorder(Arc::clone(&log)), Some(loader(&log, first, "a")));
        installed.release();
        assert_eq!(events(&log), ["set(first)", "set(NULL)", "drop(a)"]);
    }

    #[test]
    fn dropping_an_unreleased_installation_still_nulls_first() {
        let log = Log::default();
        drop(Installed::before_init(
            Recorder(Arc::clone(&log)),
            Some(loader(&log, first, "a")),
        ));
        assert_eq!(events(&log), ["set(first)", "set(NULL)", "drop(a)"]);
    }

    #[test]
    fn a_retained_installation_releases_nothing() {
        let log = Log::default();
        let installed =
            Installed::before_init(Recorder(Arc::clone(&log)), Some(loader(&log, first, "a")));
        std::mem::forget(installed);
        assert_eq!(
            events(&log),
            ["set(first)"],
            "the keep-alive stays with the retained backend"
        );
    }
}
