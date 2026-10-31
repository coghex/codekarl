//! The Vulkan loader, loaded by an explicit absolute path and owned behind a reference count.
//!
//! **The ownership contract** (shared with `ck-window-glfw` and the executable): a [`Loader`] owns the
//! dynamically loaded Vulkan library. It is `Clone` and cheap to clone; the library is unloaded only when
//! the last clone is dropped. Every Vulkan consumer holds its own clone for as long as it can call the
//! library: each [`Instance`](crate::Instance), each [`VulkanPresenter`](crate::VulkanPresenter) handle
//! **and its render thread**, and [`readback_clear`](crate::readback_clear) for its duration. A consumer
//! kept alive on purpose (an `Unproven` teardown, a missed deadline, a panic) keeps its clone, so the
//! library is never unloaded under retained objects, and no ordinary `Drop` or unwind unloads it while such
//! a consumer exists. [`Loader::get_instance_proc_addr`] returns the raw `vkGetInstanceProcAddr`; copying
//! that pointer does not carry ownership, and it is valid only while a clone of that `Loader` lives.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ash::vk;

use crate::Error;

/// The loader's file name on this platform.
#[cfg(target_os = "macos")]
const LOADER_FILE: &str = "libvulkan.1.dylib";
/// The loader's file name on this platform.
#[cfg(not(target_os = "macos"))]
const LOADER_FILE: &str = "libvulkan.so.1";

/// The raw `vkGetInstanceProcAddr` of a loader.
pub type GetInstanceProcAddr =
    unsafe extern "system" fn(vk::Instance, *const std::ffi::c_char) -> vk::PFN_vkVoidFunction;

struct LoaderInner {
    /// `None` only while the last clone is being dropped.
    entry: Option<ash::Entry>,
    path: Option<PathBuf>,
    /// Set after the library was released: the last clone is gone.
    released: Arc<AtomicBool>,
}

impl Drop for LoaderInner {
    fn drop(&mut self) {
        // The library is unloaded first (the `Entry` holds the only reference to it), then the flag says so.
        drop(self.entry.take());
        self.released.store(true, Ordering::SeqCst);
    }
}

/// The loaded Vulkan loader, shared by reference count (see the module documentation).
#[derive(Clone)]
pub struct Loader {
    inner: Arc<LoaderInner>,
}

impl std::fmt::Debug for Loader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Loader")
            .field("path", &self.inner.path)
            .field("clones", &Arc::strong_count(&self.inner))
            .finish()
    }
}

/// Reports whether a [`Loader`]'s library has been released, for the tests: set once the last clone was
/// dropped, after the library was unloaded.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct LoaderWatch(Arc<AtomicBool>);

impl LoaderWatch {
    /// Whether the last clone of the loader has been dropped.
    #[must_use]
    pub fn released(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

impl Loader {
    fn from_entry(entry: ash::Entry, path: Option<PathBuf>) -> Self {
        Self {
            inner: Arc::new(LoaderInner {
                entry: Some(entry),
                path,
                released: Arc::new(AtomicBool::new(false)),
            }),
        }
    }

    pub(crate) fn entry(&self) -> &ash::Entry {
        // The entry is taken only by the last drop, which no live clone can observe.
        self.inner
            .entry
            .as_ref()
            .unwrap_or_else(|| unreachable!("a live Loader has its entry"))
    }

    /// The raw `vkGetInstanceProcAddr`, for `glfwInitVulkanLoader` (through `ck-window-glfw`'s
    /// `VulkanLoaderForGlfw`).
    ///
    /// Copying the pointer does not carry ownership: it is valid only while a clone of this `Loader`
    /// lives, so whoever hands it on must also hand on a clone (or another owner of the library) that
    /// keeps the library loaded.
    #[must_use]
    pub fn get_instance_proc_addr(&self) -> GetInstanceProcAddr {
        self.entry().static_fn().get_instance_proc_addr
    }

    /// The absolute path the library was loaded from, or `None` when the platform's default search found it.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.inner.path.as_deref()
    }

    /// How many clones of this loader exist right now.
    #[must_use]
    pub fn clones(&self) -> usize {
        Arc::strong_count(&self.inner)
    }

    /// A watch on this loader's release, for the tests.
    #[doc(hidden)]
    #[must_use]
    pub fn watch(&self) -> LoaderWatch {
        LoaderWatch(Arc::clone(&self.inner.released))
    }

    /// The highest instance version the loader reports (1.0 when it cannot say).
    ///
    /// # Errors
    ///
    /// The loader's error when the query fails.
    pub fn instance_version(&self) -> Result<u32, Error> {
        // SAFETY: the entry is live (this loader holds it) and the call takes no pointer from us.
        let version = unsafe { self.entry().try_enumerate_instance_version() }
            .map_err(|code| crate::error::vk_error("vkEnumerateInstanceVersion", code))?;
        Ok(version.unwrap_or(vk::API_VERSION_1_0))
    }
}

/// The directories searched for the loader, in order: `$VULKAN_SDK/lib` when the variable is set, then
/// `/usr/local/lib`, then `/opt/homebrew/lib`.
#[must_use]
pub fn search_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(sdk) = std::env::var_os("VULKAN_SDK").filter(|value| !value.is_empty()) {
        directories.push(Path::new(&sdk).join("lib"));
    }
    directories.push(PathBuf::from("/usr/local/lib"));
    directories.push(PathBuf::from("/opt/homebrew/lib"));
    directories
}

/// Loads the Vulkan loader: each directory of [`search_directories`] is tried for `libvulkan.1.dylib`
/// (`libvulkan.so.1` off macOS), by absolute path, and then the platform's default search runs. It never
/// relies on `DYLD_*` or any other dynamic-loader variable.
///
/// # Errors
///
/// An [`Error`] listing every place searched when nothing loaded.
pub fn load_loader() -> Result<Loader, Error> {
    load_loader_from(&search_directories(), true)
}

/// [`load_loader`] over the given directories, with the platform's default search last when
/// `default_search` is set. The tests use it with directories of their own.
///
/// # Errors
///
/// An [`Error`] listing every place searched when nothing loaded.
pub fn load_loader_from(directories: &[PathBuf], default_search: bool) -> Result<Loader, Error> {
    let mut tried = Vec::new();
    for directory in directories {
        let candidate = directory.join(LOADER_FILE);
        if !candidate.exists() {
            tried.push(format!("{} (not there)", candidate.display()));
            continue;
        }
        // SAFETY: loading a shared library runs its initialisers; this is the Vulkan loader the
        // installation put at an absolute path, which is the same trust `tools/ck doctor` checks.
        match unsafe { ash::Entry::load_from(OsString::from(candidate.as_os_str())) } {
            Ok(entry) => return Ok(Loader::from_entry(entry, Some(candidate))),
            Err(error) => tried.push(format!("{} ({error})", candidate.display())),
        }
    }
    if default_search {
        // SAFETY: as above, for whatever the platform's default search finds under the loader's names.
        match unsafe { ash::Entry::load() } {
            Ok(entry) => return Ok(Loader::from_entry(entry, None)),
            Err(error) => tried.push(format!("the platform's default search ({error})")),
        }
    }
    Err(Error(format!(
        "no Vulkan loader ({LOADER_FILE}) could be loaded; searched: {}",
        tried.join("; ")
    )))
}
