//! The instance and the surface helpers, used on the main thread, and the validation log.

use std::collections::BTreeSet;
use std::ffi::{CStr, CString, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ash::vk;
use ash::{ext, khr};

use crate::error::vk_error;
use crate::{CleanupResult, Error, Loader};

/// The validation layer's name.
const VALIDATION_LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

/// What the validation layer reported (warnings and errors), read by the tests: any message fails a test.
#[derive(Debug, Default)]
pub(crate) struct ValidationLog {
    messages: Mutex<Vec<String>>,
}

impl ValidationLog {
    pub(crate) fn snapshot(&self) -> Vec<String> {
        self.messages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The debug messenger callback: records warnings and errors. It never panics and never aborts the call.
unsafe extern "system" fn on_message(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    kinds: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user: *mut c_void,
) -> vk::Bool32 {
    if user.is_null() || data.is_null() {
        return vk::FALSE;
    }
    // SAFETY: `user` is the `Arc<ValidationLog>` raw pointer `create_instance` registered; the instance
    // keeps that `Arc` alive until after the messenger is destroyed (or for the process, if it is kept).
    let log = unsafe { &*user.cast::<ValidationLog>() };
    // SAFETY: the layer passes a valid callback data structure for the duration of the call.
    let data = unsafe { &*data };
    // SAFETY: the strings, when present, are NUL-terminated and valid for the duration of the call.
    let id = unsafe { data.message_id_name_as_c_str() }
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    // SAFETY: as above.
    let message = unsafe { data.message_as_c_str() }
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_default();
    log.messages
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(format!("[{severity:?} {kinds:?}] {id}: {message}"));
    vk::FALSE
}

/// A handle on an instance's validation log (see [`Instance::validation_log`]).
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct ValidationLogHandle(Arc<ValidationLog>);

impl ValidationLogHandle {
    /// Every warning and error recorded so far.
    #[must_use]
    pub fn messages(&self) -> Vec<String> {
        self.0.snapshot()
    }
}

/// The debug messenger and what it needs to stay alive.
///
/// Two messengers report to the same log: the one chained into the instance create info (it reports what
/// happens during `vkCreateInstance` and `vkDestroyInstance`) and the persistent one created right after the
/// instance. Both read `log_pointer`, which is released only **after** `vkDestroyInstance` returned, and
/// never for an instance that is kept.
pub(crate) struct Debug {
    utils: ext::debug_utils::Instance,
    messenger: vk::DebugUtilsMessengerEXT,
    /// The raw `Arc<ValidationLog>` the callbacks read.
    log_pointer: *const ValidationLog,
}

// SAFETY: the messenger handle and the function table are plain data; the log pointer is an `Arc`'s raw
// pointer to a `Sync` value that is read only through shared references.
unsafe impl Send for Debug {}
// SAFETY: as above: nothing here is mutated after creation.
unsafe impl Sync for Debug {}

/// What an instance is made of; shared with a presenter's render thread (which holds a clone while it runs,
/// so a retained thread keeps the instance from being destroyed).
pub(crate) struct InstanceInner {
    /// A clone of the loader: the library stays loaded for as long as this instance exists.
    pub(crate) loader: Loader,
    pub(crate) instance: ash::Instance,
    pub(crate) surface: khr::surface::Instance,
    pub(crate) log: Arc<ValidationLog>,
    pub(crate) debug: Option<Debug>,
    pub(crate) portability: bool,
    pub(crate) extensions: Vec<String>,
    /// Off-screen readbacks whose completion is unproven and which therefore keep this instance for the rest
    /// of the process (each also holds an `Arc` of it).
    pub(crate) retained_readbacks: AtomicUsize,
}

/// Options for [`create_instance_with`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstanceOptions {
    /// Enable `VK_LAYER_KHRONOS_validation` and a debug messenger that records every warning and error
    /// (the tests; an error when the layer or `VK_EXT_debug_utils` is missing).
    pub validation: bool,
}

/// A Vulkan instance, created and destroyed on the main thread. Not `Clone`: it is destroyed once, by
/// [`destroy_surface_and_instance`].
pub struct Instance {
    pub(crate) inner: Arc<InstanceInner>,
}

impl std::fmt::Debug for Instance {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Instance")
            .field("extensions", &self.inner.extensions)
            .field("portability", &self.inner.portability)
            .finish_non_exhaustive()
    }
}

impl Instance {
    /// The raw `VkInstance` handle, for creating the surface (`glfwCreateWindowSurface`).
    #[must_use]
    pub fn raw_handle(&self) -> u64 {
        use ash::vk::Handle as _;
        self.inner.instance.handle().as_raw()
    }

    /// The loader this instance holds a clone of.
    #[must_use]
    pub fn loader(&self) -> &Loader {
        &self.inner.loader
    }

    /// The instance extensions that were enabled.
    #[must_use]
    pub fn enabled_extensions(&self) -> &[String] {
        &self.inner.extensions
    }

    /// Whether the implementation is a portability one (`VK_KHR_portability_enumeration` was enabled).
    #[must_use]
    pub fn is_portability(&self) -> bool {
        self.inner.portability
    }

    /// Every validation warning and error recorded so far (empty unless validation was asked for).
    #[must_use]
    pub fn validation_messages(&self) -> Vec<String> {
        self.inner.log.snapshot()
    }

    /// Sends one warning-severity message through the instance's own debug messenger, so a test can prove
    /// the validation path records what the layer says. Returns whether the instance has a messenger.
    #[doc(hidden)]
    #[must_use]
    pub fn submit_test_warning(&self, text: &str) -> bool {
        let Some(debug) = &self.inner.debug else {
            return false;
        };
        let Ok(message) = CString::new(text) else {
            return false;
        };
        let data = vk::DebugUtilsMessengerCallbackDataEXT::default().message(&message);
        // SAFETY: the messenger belongs to this live instance and `data` is valid for the call.
        unsafe {
            debug.utils.submit_debug_utils_message(
                vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
                vk::DebugUtilsMessageTypeFlagsEXT::GENERAL,
                &data,
            );
        }
        true
    }

    /// A handle on the validation log that outlives the instance, for the tests: the messages recorded while
    /// the instance was destroyed are still readable.
    #[doc(hidden)]
    #[must_use]
    pub fn validation_log(&self) -> ValidationLogHandle {
        ValidationLogHandle(Arc::clone(&self.inner.log))
    }

    /// How many holders the instance has right now: itself, plus one per render thread and one per retained
    /// readback that still holds it.
    #[doc(hidden)]
    #[must_use]
    pub fn holders(&self) -> usize {
        Arc::strong_count(&self.inner)
    }

    /// How many off-screen readbacks are retained with unproven completion (see [`crate::readback_clear`]).
    #[doc(hidden)]
    #[must_use]
    pub fn retained_readbacks(&self) -> usize {
        self.inner.retained_readbacks.load(Ordering::SeqCst)
    }

    /// Creates a logical device on the first physical device with a graphics queue and **leaks it**, so a
    /// test can make `vkDestroyInstance` report a child object that was not destroyed (and prove that the
    /// log records messages from the instance's destruction). Returns whether a device was created.
    #[doc(hidden)]
    #[must_use]
    pub fn leak_a_device_for_tests(&self) -> bool {
        let instance = &self.inner.instance;
        // SAFETY: the instance is live and the call takes no pointer from us.
        let Ok(devices) = (unsafe { instance.enumerate_physical_devices() }) else {
            return false;
        };
        let Some(&physical) = devices.first() else {
            return false;
        };
        // SAFETY: `physical` came from this instance.
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        let Some(family) = families
            .iter()
            .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        else {
            return false;
        };
        let priorities = [1.0_f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(u32::try_from(family).unwrap_or(0))
            .queue_priorities(&priorities)];
        let subset = [khr::portability_subset::NAME.as_ptr()];
        // SAFETY: `physical` came from this instance and the property list is valid for the call.
        let advertised = unsafe { instance.enumerate_device_extension_properties(physical) }
            .unwrap_or_default()
            .iter()
            .any(|property| {
                property
                    .extension_name_as_c_str()
                    .is_ok_and(|name| name == khr::portability_subset::NAME)
            });
        let mut info = vk::DeviceCreateInfo::default().queue_create_infos(&queue_info);
        if advertised {
            info = info.enabled_extension_names(&subset);
        }
        // SAFETY: the instance is live and `info` and what it points to live for the call.
        // The device is deliberately never destroyed.
        unsafe { instance.create_device(physical, &info, None) }.is_ok()
    }
}

/// What a test may change about [`create_instance_with`]. Not part of the interface.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstanceTestHooks {
    /// Do not add the extensions that the helper's own extensions depend on (`VK_KHR_surface`): the instance
    /// is then invalid, and the validation layer reports it while the instance is created.
    pub omit_dependencies: bool,
}

/// Creates an instance without validation: see [`create_instance_with`].
///
/// # Errors
///
/// As [`create_instance_with`].
pub fn create_instance(loader: &Loader, required_extensions: &[String]) -> Result<Instance, Error> {
    create_instance_with(loader, required_extensions, InstanceOptions::default())
}

/// Creates an instance on the main thread with `loader` (the instance holds its own clone): the surface
/// extensions the window side requires, `VK_KHR_portability_enumeration` (with its flag) where the
/// implementation is a portability one, `VK_EXT_surface_maintenance1` and `VK_KHR_get_surface_capabilities2`
/// (the present-fence extensions need them), and, when asked, `VK_LAYER_KHRONOS_validation` with a debug
/// messenger that records every warning and error.
///
/// # Errors
///
/// An [`Error`] naming the missing layer or extension, or the Vulkan error.
pub fn create_instance_with(
    loader: &Loader,
    required_extensions: &[String],
    options: InstanceOptions,
) -> Result<Instance, Error> {
    create_instance_with_test_hooks(
        loader,
        required_extensions,
        options,
        InstanceTestHooks::default(),
    )
}

/// [`create_instance_with`] with test changes. Not part of the interface.
///
/// # Errors
///
/// As [`create_instance_with`].
#[doc(hidden)]
pub fn create_instance_with_test_hooks(
    loader: &Loader,
    required_extensions: &[String],
    options: InstanceOptions,
    hooks: InstanceTestHooks,
) -> Result<Instance, Error> {
    let entry = loader.entry();
    let plan = plan_instance(loader, required_extensions, options, hooks)?;
    let names: Vec<CString> = plan
        .extensions
        .iter()
        .map(|name| {
            CString::new(name.as_str()).map_err(|_| Error(format!("bad extension name {name:?}")))
        })
        .collect::<Result<_, _>>()?;
    let name_pointers: Vec<*const std::ffi::c_char> =
        names.iter().map(|name| name.as_ptr()).collect();
    let layer_pointers: Vec<*const std::ffi::c_char> =
        plan.layers.iter().map(|name| name.as_ptr()).collect();
    let application = vk::ApplicationInfo::default()
        .application_name(c"codekarl")
        .api_version(vk::API_VERSION_1_1);
    let log = Arc::new(ValidationLog::default());
    // The log's storage outlives the instance: the raw pointer is released after `vkDestroyInstance`, or
    // never for an instance that is kept.
    let log_pointer = options.validation.then(|| Arc::into_raw(Arc::clone(&log)));
    let mut create_messenger = log_pointer.map(messenger_info);
    let mut info = vk::InstanceCreateInfo::default()
        .application_info(&application)
        .enabled_extension_names(&name_pointers)
        .enabled_layer_names(&layer_pointers);
    if plan.portability {
        info = info.flags(vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR);
    }
    if let Some(messenger) = create_messenger.as_mut() {
        // Reports what happens during `vkCreateInstance` and `vkDestroyInstance`.
        info = info.push_next(messenger);
    }
    // SAFETY: `info` and everything it points to (names, layers, application, the messenger info) live until
    // the call returns; the callback's user data outlives the instance.
    let created = unsafe { entry.create_instance(&info, None) };
    let instance = match created {
        Ok(instance) => instance,
        Err(code) => {
            release_log(log_pointer);
            return Err(vk_error("vkCreateInstance", code));
        }
    };
    let surface = khr::surface::Instance::new(entry, &instance);
    let debug = match log_pointer {
        Some(pointer) => match install_messenger(entry, &instance, pointer) {
            Ok(debug) => Some(debug),
            Err(error) => {
                // SAFETY: the instance was just created and nothing else uses it.
                unsafe { instance.destroy_instance(None) };
                release_log(log_pointer);
                return Err(error);
            }
        },
        None => None,
    };
    Ok(Instance {
        inner: Arc::new(InstanceInner {
            loader: loader.clone(),
            instance,
            surface,
            log,
            debug,
            portability: plan.portability,
            extensions: plan.extensions,
            retained_readbacks: AtomicUsize::new(0),
        }),
    })
}

/// Releases the raw `Arc` taken for the callbacks; only after the instance is gone (or never created).
fn release_log(pointer: Option<*const ValidationLog>) {
    if let Some(pointer) = pointer {
        // SAFETY: undoes the `Arc::into_raw` of this instance's creation, once; no callback can run any more.
        drop(unsafe { Arc::from_raw(pointer) });
    }
}

/// The messenger create info shared by the one chained into the instance create info and the persistent one:
/// warnings and errors of every kind, reported to the log behind `pointer`.
fn messenger_info(pointer: *const ValidationLog) -> vk::DebugUtilsMessengerCreateInfoEXT<'static> {
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(on_message))
        .user_data(pointer.cast_mut().cast::<c_void>())
}

/// What an instance will be created with.
struct Plan {
    extensions: Vec<String>,
    layers: Vec<&'static CStr>,
    portability: bool,
}

/// Checks the loader offers every extension and layer the instance needs and names the first that is not.
fn plan_instance(
    loader: &Loader,
    required_extensions: &[String],
    options: InstanceOptions,
    hooks: InstanceTestHooks,
) -> Result<Plan, Error> {
    let entry = loader.entry();
    // SAFETY: the entry is live (the loader is borrowed) and the call takes no pointer from us.
    let available = unsafe { entry.enumerate_instance_extension_properties(None) }
        .map_err(|code| vk_error("vkEnumerateInstanceExtensionProperties", code))?;
    let have: BTreeSet<String> = available
        .iter()
        .filter_map(|property| property.extension_name_as_c_str().ok())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    let mut wanted: Vec<String> = Vec::new();
    for name in required_extensions {
        if !have.contains(name) {
            return Err(Error(format!(
                "the Vulkan instance extension {name} (required by the window) is not available"
            )));
        }
        wanted.push(name.clone());
    }
    for name in [
        ext::surface_maintenance1::NAME,
        khr::get_surface_capabilities2::NAME,
    ] {
        let name = name.to_string_lossy().into_owned();
        if !have.contains(&name) {
            return Err(Error(format!(
                "the Vulkan instance extension {name} (present fences) is not available"
            )));
        }
        wanted.push(name);
    }
    // The two extensions above require `VK_KHR_surface`, whatever the caller passes: a headless caller (an
    // empty `required_extensions`, the readback) gets a valid instance, and an implementation without it
    // is an error naming it before `vkCreateInstance`.
    if !hooks.omit_dependencies {
        let surface = khr::surface::NAME.to_string_lossy().into_owned();
        if !have.contains(&surface) {
            return Err(Error(format!(
                "the Vulkan instance extension {surface} (required by VK_EXT_surface_maintenance1 and \
                 VK_KHR_get_surface_capabilities2) is not available"
            )));
        }
        wanted.push(surface);
    }
    let portability_name = khr::portability_enumeration::NAME
        .to_string_lossy()
        .into_owned();
    let portability = have.contains(&portability_name);
    if portability {
        wanted.push(portability_name);
    }
    let mut layers: Vec<&'static CStr> = Vec::new();
    if options.validation {
        let debug_utils = ext::debug_utils::NAME.to_string_lossy().into_owned();
        if !have.contains(&debug_utils) {
            return Err(Error(format!(
                "the Vulkan instance extension {debug_utils} (validation messages) is not available"
            )));
        }
        // SAFETY: the entry is live and the call takes no pointer from us.
        let installed = unsafe { entry.enumerate_instance_layer_properties() }
            .map_err(|code| vk_error("vkEnumerateInstanceLayerProperties", code))?;
        if !installed.iter().any(|layer| {
            layer
                .layer_name_as_c_str()
                .is_ok_and(|name| name == VALIDATION_LAYER)
        }) {
            return Err(Error(format!(
                "the Vulkan layer {} is not installed",
                VALIDATION_LAYER.to_string_lossy()
            )));
        }
        layers.push(VALIDATION_LAYER);
        wanted.push(debug_utils);
    }
    wanted.sort();
    wanted.dedup();
    Ok(Plan {
        extensions: wanted,
        layers,
        portability,
    })
}

/// Creates the persistent debug messenger that records every warning and error into the log behind
/// `log_pointer` (the raw `Arc` the caller took; ownership of it passes to the returned [`Debug`]).
fn install_messenger(
    entry: &ash::Entry,
    instance: &ash::Instance,
    log_pointer: *const ValidationLog,
) -> Result<Debug, Error> {
    let utils = ext::debug_utils::Instance::new(entry, instance);
    let info = messenger_info(log_pointer);
    // SAFETY: the create info is valid for the call; the callback and its user data outlive the messenger
    // and the instance (the data is released only after `vkDestroyInstance`).
    match unsafe { utils.create_debug_utils_messenger(&info, None) } {
        Ok(messenger) => Ok(Debug {
            utils,
            messenger,
            log_pointer,
        }),
        Err(code) => Err(vk_error("vkCreateDebugUtilsMessengerEXT", code)),
    }
}

/// Why [`destroy_surface_and_instance`] kept the instance. The outcome is explicit about the surface: when
/// [`surface_destroyed`](Self::surface_destroyed) is true the surface is **already destroyed** (an unproven
/// readback keeps the instance, but nothing else uses the surface), and the caller must not destroy it
/// again.
#[derive(Debug)]
pub struct DestroyRefused {
    /// The instance, untouched.
    pub instance: Instance,
    /// Whether the surface was destroyed before the refusal.
    pub surface_destroyed: bool,
    /// Why the instance was kept.
    pub error: Error,
}

impl std::fmt::Display for DestroyRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} (surface {})",
            self.error,
            if self.surface_destroyed {
                "destroyed"
            } else {
                "kept"
            }
        )
    }
}

impl std::error::Error for DestroyRefused {}

/// Destroys the surface and then the instance (design §7 order), on the main thread, after the presenter's
/// proven end and before the window is destroyed. It **refuses**, returning the instance untouched and the
/// surface kept, unless `ended` is `Some(&CleanupResult::Proven)` (the presenter reported a proven end) or
/// `None` (no presenter was ever started), and while a render thread still holds the instance.
///
/// An unproven off-screen readback (see [`crate::readback_clear`]) keeps the instance for the rest of the
/// process: then the surface is still destroyed (nothing else uses it) but the instance is returned, and
/// [`DestroyRefused::surface_destroyed`] says so.
///
/// # Errors
///
/// A [`DestroyRefused`]: the instance back, whether the surface was destroyed, and why the instance was kept.
///
/// # Safety
///
/// `surface` must be a surface created on `instance` (or null) that nothing else uses any more, and the
/// window it belongs to must still be alive.
pub unsafe fn destroy_surface_and_instance(
    instance: Instance,
    surface: vk::SurfaceKHR,
    ended: Option<&CleanupResult>,
) -> Result<(), DestroyRefused> {
    let refuse = |instance: Instance, surface_destroyed: bool, message: String| DestroyRefused {
        instance,
        surface_destroyed,
        error: Error(message),
    };
    if let Some(CleanupResult::Unproven { reason }) = ended {
        return Err(refuse(
            instance,
            false,
            format!(
                "the presenter's end was not proven ({reason}); the surface and instance were kept"
            ),
        ));
    }
    let retained = instance.inner.retained_readbacks.load(Ordering::SeqCst);
    // The holders other than the caller's: render threads and retained readbacks (one each).
    let others = Arc::strong_count(&instance.inner).saturating_sub(1);
    if others > retained {
        return Err(refuse(
            instance,
            false,
            "a render thread still holds the instance; the surface and instance were kept"
                .to_owned(),
        ));
    }
    let destroyed = surface != vk::SurfaceKHR::null();
    if destroyed {
        // SAFETY: by this function's contract the surface belongs to this instance and is unused.
        unsafe { instance.inner.surface.destroy_surface(surface, None) };
    }
    if retained > 0 {
        return Err(refuse(
            instance,
            destroyed,
            format!(
                "{retained} off-screen readback(s) with unproven completion still reference the instance; \
                 the instance was kept"
            ),
        ));
    }
    match Arc::try_unwrap(instance.inner) {
        Ok(inner) => {
            let pointer = inner.debug.as_ref().map(|debug| debug.log_pointer);
            if let Some(debug) = &inner.debug {
                // SAFETY: the messenger belongs to this instance, which is still alive.
                unsafe {
                    debug
                        .utils
                        .destroy_debug_utils_messenger(debug.messenger, None);
                }
            }
            // SAFETY: the surface and the messenger are destroyed and nothing else uses the instance. The
            // messenger chained into the create info reports this call too, into a log that is still alive.
            unsafe { inner.instance.destroy_instance(None) };
            release_log(pointer);
            drop(inner);
            Ok(())
        }
        Err(inner) => Err(refuse(
            Instance { inner },
            destroyed,
            "a holder took the instance while it was being destroyed".to_owned(),
        )),
    }
}
