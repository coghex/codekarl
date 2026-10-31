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

/// The one validation message the log tolerates: `MoltenVK`'s `vkGetDescriptorSetLayoutSupport` answers no for any layout of
/// 1,024 (1.2.11) or 1,212 (1.4.2) descriptors or more, whatever its update-after-bind limits say, so creating the bindless
/// table layout after that answer is reported by the layer with this VUID
/// ([KhronosGroup/MoltenVK#2573](https://github.com/KhronosGroup/MoltenVK/issues/2573), open). The owner chose to gate on the
/// update-after-bind limits and create the layout anyway (2026-10-31, "Siblings' way"). It is tolerated only inside the table
/// layout's own creation call, after a "no" (see [`ValidationLog::layout_scope`]); never as a layer setting or a filter.
pub(crate) const TOLERATED_VUID: &str = "VUID-vkCreateDescriptorSetLayout-support-09582";

/// The most entries the log keeps; later ones are counted and dropped.
pub(crate) const LOG_ENTRIES: usize = 256;

/// One message the debug messenger recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// `[severity kinds] id: message`.
    pub text: String,
    /// The message was recorded inside the table layout's creation call after a "no" and is the one tolerated VUID. The
    /// callback decides this when the message arrives; nothing re-derives it.
    pub tolerated: bool,
    /// The message is the loader's own notice, made while the instance was being created, that the environment adds the
    /// validation layer (`is_loader_layer_notice`). It is not a validation message; the callback decides this too.
    pub loader_notice: bool,
}

/// Entries moved out of a log by [`ValidationLogHandle::drain`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogDrain {
    /// The entries, oldest first.
    pub entries: Vec<LogEntry>,
    /// Entries dropped beyond the log's bound (256 entries) since the last drain.
    pub dropped: u64,
}

#[derive(Debug, Default)]
struct LogState {
    entries: Vec<LogEntry>,
    dropped: u64,
}

/// The loader's message id for its own notices.
const LOADER_MESSAGE_ID: &str = "Loader Message";

/// Whether a message is the loader's notice that an environment variable adds a layer: when the loader enables the validation
/// layer from `VK_INSTANCE_LAYERS` or `VK_LOADER_LAYERS_ENABLE` it tells the debug messenger chained into the instance create
/// info so, at `WARNING` severity, although it is neither a validation message nor a problem. Only that text, only from the
/// loader's own id, and only while the instance is being created (see [`ValidationLog::creating`]).
pub(crate) fn is_loader_layer_notice(id: &str, message: &str) -> bool {
    id == LOADER_MESSAGE_ID
        && ["VK_INSTANCE_LAYERS", "VK_LOADER_LAYERS_ENABLE"]
            .iter()
            .any(|variable| {
                message.starts_with(&format!("env var '{variable}' defined and adding layers"))
            })
}

/// What the validation layer reported (warnings and errors), read by the tests: any message that is not tolerated fails a
/// test. Bounded at [`LOG_ENTRIES`] entries and a count of the entries beyond them.
#[derive(Debug, Default)]
pub(crate) struct ValidationLog {
    state: Mutex<LogState>,
    /// The instance is being created: set before `vkCreateInstance`, cleared when the instance exists. The loader's notices
    /// about the environment's layers arrive in this window and in no other.
    creating: std::sync::atomic::AtomicBool,
}

/// The table layout's creation call in progress on this thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Scope {
    /// The address of the log (the instance) the call belongs to.
    log: usize,
    /// The support query had answered no.
    answered_no: bool,
}

thread_local! {
    /// The calling thread's table-layout scope. The debug messenger runs synchronously on the thread that made the call
    /// (Vulkan's callback rule), so only that thread's messages inside the guard are tolerated.
    static SCOPE: std::cell::Cell<Option<Scope>> = const { std::cell::Cell::new(None) };
}

/// A guard marking the table layout's `vkCreateDescriptorSetLayout` call on the calling thread; cleared when dropped, on
/// success, on error and on unwinding.
#[doc(hidden)]
#[must_use = "the scope lasts as long as the guard"]
pub struct LayoutScope {
    previous: Option<Scope>,
    // The guard belongs to its thread's scope; it must not move to another one.
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for LayoutScope {
    fn drop(&mut self) {
        SCOPE.with(|scope| scope.set(self.previous));
    }
}

impl ValidationLog {
    /// Marks the table layout's creation call on this thread, for this log only (see [`TOLERATED_VUID`]).
    pub(crate) fn layout_scope(&self, answered_no: bool) -> LayoutScope {
        let mine = Scope {
            log: std::ptr::from_ref(self) as usize,
            answered_no,
        };
        let previous = SCOPE.with(|scope| scope.replace(Some(mine)));
        LayoutScope {
            previous,
            _not_send: std::marker::PhantomData,
        }
    }

    /// Whether a message with `id` arriving now is the tolerated one: its id is [`TOLERATED_VUID`], this thread is inside
    /// this log's table layout scope, and the support query had answered no.
    fn tolerates(&self, id: &str) -> bool {
        id == TOLERATED_VUID
            && SCOPE.with(std::cell::Cell::get)
                == Some(Scope {
                    log: std::ptr::from_ref(self) as usize,
                    answered_no: true,
                })
    }

    fn record(&self, text: String, tolerated: bool, loader_notice: bool) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.entries.len() >= LOG_ENTRIES {
            state.dropped = state.dropped.saturating_add(1);
        } else {
            state.entries.push(LogEntry {
                text,
                tolerated,
                loader_notice,
            });
        }
    }

    /// Whether `id` and `message` are the loader's notice about the environment's layers, arriving during instance creation.
    fn loader_notice(&self, id: &str, message: &str) -> bool {
        self.creating.load(Ordering::SeqCst) && is_loader_layer_notice(id, message)
    }

    /// Every message that fails a clean check: the entries that are not tolerated, then one line for the entries dropped.
    pub(crate) fn snapshot(&self) -> Vec<String> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut messages: Vec<String> = state
            .entries
            .iter()
            .filter(|entry| !entry.tolerated && !entry.loader_notice)
            .map(|entry| entry.text.clone())
            .collect();
        if state.dropped != 0 {
            messages.push(format!(
                "{} further validation messages were dropped beyond {LOG_ENTRIES}",
                state.dropped
            ));
        }
        messages
    }

    /// The tolerated messages, oldest first.
    pub(crate) fn tolerated(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .iter()
            .filter(|entry| entry.tolerated)
            .map(|entry| entry.text.clone())
            .collect()
    }

    /// Moves every entry out (each is delivered once) with the count dropped since the last drain.
    pub(crate) fn drain(&self) -> LogDrain {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        LogDrain {
            entries: std::mem::take(&mut state.entries),
            dropped: std::mem::take(&mut state.dropped),
        }
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
    let tolerated = log.tolerates(&id);
    // GENERAL type and WARNING severity only, as observed: a validation-type message, or one of another severity, with the
    // loader's words is not the loader's notice.
    let loader_notice = kinds == vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
        && severity == vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
        && log.loader_notice(&id, &message);
    log.record(
        format!("[{severity:?} {kinds:?}] {id}: {message}"),
        tolerated,
        loader_notice,
    );
    vk::FALSE
}

/// A handle on an instance's validation log (see [`Instance::validation_log`]): the application keeps it apart from the
/// instance, so messages emitted while the instance is destroyed, or after a failed setup, are still read.
#[derive(Debug, Clone)]
pub struct ValidationLogHandle(Arc<ValidationLog>);

impl ValidationLogHandle {
    /// Every message that fails a clean check: each recorded message that is not tolerated, and one line when entries were
    /// dropped beyond the bound.
    #[must_use]
    pub fn messages(&self) -> Vec<String> {
        self.0.snapshot()
    }

    /// The tolerated messages recorded so far (the table layout's one, after a "no").
    #[must_use]
    pub fn tolerated(&self) -> Vec<String> {
        self.0.tolerated()
    }

    /// Moves every recorded entry out, once, with the number dropped beyond the bound since the last drain.
    #[must_use]
    pub fn drain(&self) -> LogDrain {
        self.0.drain()
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
    /// Also turn on synchronization validation (`VK_EXT_validation_features`, from the validation layer), which
    /// reports missing barriers and hazards between commands. Instance creation fails when it is unavailable,
    /// and when `validation` is off.
    pub synchronization: bool,
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
        self.submit_test_message("", text)
    }

    /// [`submit_test_warning`](Self::submit_test_warning) with a message id (a VUID), so a test can prove which message the
    /// log tolerates. Returns whether the instance has a messenger.
    #[doc(hidden)]
    #[must_use]
    pub fn submit_test_message(&self, id: &str, text: &str) -> bool {
        let Some(debug) = &self.inner.debug else {
            return false;
        };
        let (Ok(message), Ok(id)) = (CString::new(text), CString::new(id)) else {
            return false;
        };
        let data = vk::DebugUtilsMessengerCallbackDataEXT::default()
            .message(&message)
            .message_id_name(&id);
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

    /// Marks the table layout's creation call on this thread, as the ledger does (see [`ValidationLog::layout_scope`]); here so
    /// a test can prove the tolerance with messages it sends itself.
    #[doc(hidden)]
    pub fn bindless_layout_scope(&self, answered_no: bool) -> LayoutScope {
        self.inner.log.layout_scope(answered_no)
    }

    /// A handle on the validation log that outlives the instance: the messages recorded while the instance was destroyed
    /// are still readable. The log is empty unless the crate's messenger was installed (validation was asked for, or the
    /// loader's environment enables the Khronos layer).
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
    // This entry has no way to hand the log back; the application uses [`create_instance_draining`], which returns every entry.
    create_inner(loader, required_extensions, options, hooks).map_err(|failure| failure.error)
}

/// What a failed or partial instance creation leaves: the error, and the messages the instance's log recorded until then. A creation
/// that fails returns no [`Instance`], hence no [`ValidationLogHandle`]; this keeps every entry, none summarised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceFailure {
    /// What failed.
    pub error: Error,
    /// The log's entries (each once) and the count dropped beyond its bound; empty when the failure came before the log existed.
    pub log: LogDrain,
}

/// Creates an instance as [`create_instance`] does, but a failure returns the log's entries along with the error
/// ([`InstanceFailure`]), so the caller can write every one as the record it is.
///
/// # Errors
///
/// An [`InstanceFailure`] naming the missing layer or extension, or the Vulkan error, with whatever the log recorded.
pub fn create_instance_draining(
    loader: &Loader,
    required_extensions: &[String],
) -> Result<Instance, InstanceFailure> {
    create_inner(
        loader,
        required_extensions,
        InstanceOptions::default(),
        InstanceTestHooks::default(),
    )
}

fn create_inner(
    loader: &Loader,
    required_extensions: &[String],
    options: InstanceOptions,
    hooks: InstanceTestHooks,
) -> Result<Instance, InstanceFailure> {
    let early = |error: Error| InstanceFailure {
        error,
        log: LogDrain::default(),
    };
    let entry = loader.entry();
    let plan = plan_instance(loader, required_extensions, options, hooks).map_err(early)?;
    let names: Vec<CString> = plan
        .extensions
        .iter()
        .map(|name| {
            CString::new(name.as_str()).map_err(|_| Error(format!("bad extension name {name:?}")))
        })
        .collect::<Result<_, _>>()
        .map_err(early)?;
    let name_pointers: Vec<*const std::ffi::c_char> =
        names.iter().map(|name| name.as_ptr()).collect();
    let layer_pointers: Vec<*const std::ffi::c_char> =
        plan.layers.iter().map(|name| name.as_ptr()).collect();
    let application = vk::ApplicationInfo::default()
        .application_name(c"codekarl")
        .api_version(vk::API_VERSION_1_1);
    let log = Arc::new(ValidationLog::default());
    // The loader's notices about the environment's layers arrive while the instance is created (and the persistent messenger is
    // installed): the log tolerates exactly those, and only until the instance exists.
    log.creating.store(true, Ordering::SeqCst);
    // The log's storage outlives the instance: the raw pointer is released after `vkDestroyInstance`, or
    // never for an instance that is kept.
    let log_pointer = plan.messenger.then(|| Arc::into_raw(Arc::clone(&log)));
    let mut create_messenger = log_pointer.map(messenger_info);
    let enabled_features = [vk::ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION];
    let mut validation_features =
        vk::ValidationFeaturesEXT::default().enabled_validation_features(&enabled_features);
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
    if options.synchronization {
        info = info.push_next(&mut validation_features);
    }
    // SAFETY: `info` and everything it points to (names, layers, application, the messenger info) live until
    // the call returns; the callback's user data outlives the instance.
    let created = unsafe { entry.create_instance(&info, None) };
    let instance = match created {
        Ok(instance) => instance,
        Err(code) => {
            let failure = InstanceFailure {
                error: vk_error("vkCreateInstance", code),
                log: log.drain(),
            };
            release_log(log_pointer);
            return Err(failure);
        }
    };
    let surface = khr::surface::Instance::new(entry, &instance);
    let debug = match log_pointer {
        Some(pointer) => match install_messenger(entry, &instance, pointer) {
            Ok(debug) => Some(debug),
            Err(error) => {
                // SAFETY: the instance was just created and nothing else uses it.
                unsafe { instance.destroy_instance(None) };
                // What the creation and the destruction said is in the log, which is not returned as a handle: it goes with
                // the error, entry by entry.
                let failure = InstanceFailure {
                    error,
                    log: log.drain(),
                };
                release_log(log_pointer);
                return Err(failure);
            }
        },
        None => None,
    };
    log.creating.store(false, Ordering::SeqCst);
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

/// What the loader's documented environment asks of the layers (read only to mirror what the loader will do; not an
/// application setting).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LayerEnvironment {
    /// `VK_INSTANCE_LAYERS`: layer names separated by the platform's path-list separator.
    pub(crate) instance_layers: Option<String>,
    /// `VK_LOADER_LAYERS_ENABLE`: comma-separated globs of layer names.
    pub(crate) loader_layers_enable: Option<String>,
}

impl LayerEnvironment {
    pub(crate) fn from_process() -> Self {
        let read = |name| std::env::var(name).ok().filter(|value| !value.is_empty());
        Self {
            instance_layers: read("VK_INSTANCE_LAYERS"),
            loader_layers_enable: read("VK_LOADER_LAYERS_ENABLE"),
        }
    }

    fn is_empty(&self) -> bool {
        self.instance_layers.is_none() && self.loader_layers_enable.is_none()
    }
}

/// Whether one filter of `VK_LOADER_LAYERS_ENABLE` matches the layer `name`, as the loader decides it
/// (`loader_environment.c`, `determine_filter_type` and `check_name_matches_filter_environment_var`, Vulkan-Loader 1.3.296):
/// everything is compared without regard to case; a filter that starts and ends with `*` matches a substring, one that starts
/// with `*` a suffix, one that ends with `*` a prefix, and any other filter the whole name. Only that first and last `*` is a
/// wildcard: a `?` and an inner `*` are literal characters. The loader runs the enable variable through the same matcher as the
/// disable variable, whose special values `~all~`, `*` and `**` therefore match every layer, while `~implicit~`, `~explicit~`
/// and any other `~` token match none here. The loader trims nothing, so a space belongs to the filter.
fn enable_filter_matches(filter: &str, name: &str) -> bool {
    let filter = filter.to_lowercase();
    if filter == "~all~" || filter == "*" || filter == "**" {
        return true;
    }
    if filter.is_empty() || filter.starts_with('~') {
        return false;
    }
    let name = name.to_lowercase();
    match (filter.strip_prefix('*'), filter.strip_suffix('*')) {
        (Some(rest), Some(_)) => {
            let inner = rest.strip_suffix('*').unwrap_or(rest);
            !inner.is_empty() && name.contains(inner)
        }
        (Some(suffix), None) => name.ends_with(suffix),
        (None, Some(prefix)) => name.starts_with(prefix),
        (None, None) => name == filter,
    }
}

/// Whether a validation layer will be active in an instance created with `explicit` validation (the options) in this
/// environment, with `available` the installed layers' names: explicitly; or because `VK_INSTANCE_LAYERS` names
/// `VK_LAYER_KHRONOS_validation`, or a `VK_LOADER_LAYERS_ENABLE` glob matches it, **and** that layer is installed (the
/// loader loads nothing it cannot find). The crate's messenger is installed exactly then, so production without layers is
/// unchanged: Vulkan lets the loader and drivers call a messenger even with no layer, which is why it is not always on.
pub(crate) fn validation_active(
    explicit: bool,
    environment: &LayerEnvironment,
    available: &[String],
) -> bool {
    if explicit {
        return true;
    }
    let name = VALIDATION_LAYER.to_string_lossy();
    if !available.iter().any(|layer| layer.as_str() == name) {
        return false;
    }
    let separator = if cfg!(windows) { ';' } else { ':' };
    let named = environment
        .instance_layers
        .as_deref()
        .is_some_and(|list| list.split(separator).any(|layer| layer == name));
    let matched = environment
        .loader_layers_enable
        .as_deref()
        .is_some_and(|list| {
            list.split(',')
                .any(|filter| enable_filter_matches(filter, &name))
        });
    named || matched
}

/// Whether the loader's environment enables the validation layer (the application's tests do), so the messenger records what
/// it says. Nothing is asked of the loader when neither variable is set.
///
/// # Errors
///
/// The layer is enabled but `VK_EXT_debug_utils` is not offered, or the layers cannot be listed.
fn messenger_from_environment(
    entry: &ash::Entry,
    have: &BTreeSet<String>,
    debug_utils: &str,
) -> Result<bool, Error> {
    let environment = LayerEnvironment::from_process();
    if environment.is_empty() {
        return Ok(false);
    }
    // SAFETY: the entry is live and the call takes no pointer from us.
    let installed = unsafe { entry.enumerate_instance_layer_properties() }
        .map_err(|code| vk_error("vkEnumerateInstanceLayerProperties", code))?;
    let names: Vec<String> = installed
        .iter()
        .filter_map(|layer| layer.layer_name_as_c_str().ok())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    if !validation_active(false, &environment, &names) {
        return Ok(false);
    }
    if !have.contains(debug_utils) {
        return Err(Error(format!(
            "the Vulkan instance extension {debug_utils} (validation messages, for the layer the environment enables) is not available"
        )));
    }
    Ok(true)
}

/// `VK_EXT_validation_features` (synchronization validation), which the validation layer itself offers; an error naming it when it
/// does not.
fn synchronization_extension(entry: &ash::Entry) -> Result<String, Error> {
    // SAFETY: the entry is live and the call takes no pointer from us.
    let offered = unsafe { entry.enumerate_instance_extension_properties(Some(VALIDATION_LAYER)) }
        .map_err(|code| vk_error("vkEnumerateInstanceExtensionProperties(layer)", code))?;
    let features = vk::EXT_VALIDATION_FEATURES_NAME;
    if !offered.iter().any(|property| {
        property
            .extension_name_as_c_str()
            .is_ok_and(|name| name == features)
    }) {
        return Err(Error(format!(
            "the Vulkan extension {} (synchronization validation) is not offered by {}",
            features.to_string_lossy(),
            VALIDATION_LAYER.to_string_lossy()
        )));
    }
    Ok(features.to_string_lossy().into_owned())
}

/// What an instance will be created with.
struct Plan {
    extensions: Vec<String>,
    layers: Vec<&'static CStr>,
    portability: bool,
    /// The crate's debug messenger is installed: validation was asked for, or the environment enables the layer.
    messenger: bool,
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
    let debug_utils = ext::debug_utils::NAME.to_string_lossy().into_owned();
    let mut messenger = options.validation;
    if !options.validation && messenger_from_environment(entry, &have, &debug_utils)? {
        wanted.push(debug_utils.clone());
        messenger = true;
    }
    if options.validation {
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
        if options.synchronization {
            wanted.push(synchronization_extension(entry)?);
        }
    } else if options.synchronization {
        return Err(Error(
            "synchronization validation needs validation to be on".to_owned(),
        ));
    }
    wanted.sort();
    wanted.dedup();
    Ok(Plan {
        extensions: wanted,
        layers,
        portability,
        messenger,
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

#[cfg(test)]
mod tests {
    use std::ffi::{CString, c_void};
    use std::sync::Arc;

    use ash::vk;

    use super::{
        LOG_ENTRIES, LayerEnvironment, TOLERATED_VUID, ValidationLog, enable_filter_matches,
        on_message, validation_active,
    };

    const LAYER: &str = "VK_LAYER_KHRONOS_validation";

    fn layers(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn environment(instance_layers: Option<&str>, enable: Option<&str>) -> LayerEnvironment {
        LayerEnvironment {
            instance_layers: instance_layers.map(str::to_owned),
            loader_layers_enable: enable.map(str::to_owned),
        }
    }

    /// `VK_LOADER_LAYERS_ENABLE` as the loader reads it: case-insensitive; whole name, prefix, suffix and substring only; a `?`
    /// or an inner `*` is a literal character; `~all~`, `*` and `**` match every layer; nothing is trimmed.
    #[test]
    fn the_enable_filter_matches_as_the_loader_does() {
        for (filter, matches) in [
            (LAYER, true),
            ("vk_layer_khronos_validation", true),
            ("VK_Layer_KHRONOS_Validation", true),
            ("VK_LAYER_KHRONOS_*", true),
            ("vk_layer_khronos_*", true),
            ("Vk_LaYeR_*", true),
            ("*validation", true),
            ("*VALIDATION", true),
            ("*khronos*", true),
            ("*KHRONOS_valid*", true),
            ("vk_layer_khronos_validation*", true),
            // Not a match: another name, a longer prefix, a different substring.
            ("vk_layer_khronos_validation_extra*", false),
            ("VK_LAYER_LUNARG_*", false),
            ("*lunarg*", false),
            ("*validations", false),
            ("VK_LAYER_KHRONOS_validatio", false),
            // A `?` is a literal character, not a wildcard.
            ("VK_LAYER_KHRONOS_validatio?", false),
            ("?K_LAYER_KHRONOS_validation", false),
            // An inner `*` is a literal character too; only the first and the last count.
            ("VK_LAYER_*_validation", false),
            ("VK_*validation", false),
            ("*KHRONOS*validation*", false),
            // The loader's matcher treats `~all~`, `*` and `**` as "every layer" for the enable variable too; the other
            // `~` tokens match none here; an empty filter matches nothing.
            ("*", true),
            ("**", true),
            ("~all~", true),
            ("~ALL~", true),
            ("~implicit~", false),
            ("~explicit~", false),
            ("~anything~", false),
            ("", false),
            // Nothing is trimmed: a space belongs to the filter.
            (" VK_LAYER_KHRONOS_validation", false),
            ("VK_LAYER_KHRONOS_validation ", false),
        ] {
            assert_eq!(enable_filter_matches(filter, LAYER), matches, "{filter:?}");
        }
    }

    /// The messenger is installed only when a validation layer will be active (design: production without layers is unchanged).
    #[test]
    fn validation_is_active_for_the_option_or_for_the_environment_that_names_the_installed_layer() {
        let none = LayerEnvironment::default();
        let installed = layers(&[LAYER, "VK_LAYER_LUNARG_api_dump"]);
        // The option, whatever the environment and the installed layers.
        assert!(validation_active(true, &none, &[]));
        // Nothing asked.
        assert!(!validation_active(false, &none, &installed));
        // The loader's list names it, alone or among others.
        assert!(validation_active(
            false,
            &environment(Some(LAYER), None),
            &installed
        ));
        assert!(validation_active(
            false,
            &environment(
                Some("VK_LAYER_LUNARG_api_dump:VK_LAYER_KHRONOS_validation"),
                None
            ),
            &installed
        ));
        // The loader's enable filters match it: any case, in a comma list, with nothing trimmed.
        assert!(validation_active(
            false,
            &environment(None, Some("VK_LAYER_KHRONOS_*")),
            &installed
        ));
        assert!(validation_active(
            false,
            &environment(None, Some("vk_layer_khronos_*")),
            &installed
        ));
        assert!(validation_active(
            false,
            &environment(None, Some("VK_LAYER_LUNARG_*,*validation")),
            &installed
        ));
        // A space after the comma belongs to the second filter, which then matches nothing.
        assert!(!validation_active(
            false,
            &environment(None, Some("VK_LAYER_LUNARG_*, *validation")),
            &installed
        ));
        // The loader's special values enable every layer, the installed validation layer included.
        for special in ["*", "**", "~all~", "~All~"] {
            assert!(
                validation_active(false, &environment(None, Some(special)), &installed),
                "{special}"
            );
            assert!(
                !validation_active(false, &environment(None, Some(special)), &[]),
                "{special}: not installed"
            );
        }
        assert!(!validation_active(
            false,
            &environment(None, Some("~implicit~,~explicit~")),
            &installed
        ));
        // A literal `?` and an inner `*` are not wildcards: the loader enables nothing, so no messenger is installed.
        assert!(!validation_active(
            false,
            &environment(None, Some("VK_LAYER_KHRONOS_validatio?")),
            &installed
        ));
        assert!(!validation_active(
            false,
            &environment(None, Some("VK_LAYER_*_validation")),
            &installed
        ));
        // Another layer's name, or a glob that does not match, installs nothing.
        assert!(!validation_active(
            false,
            &environment(Some("VK_LAYER_LUNARG_api_dump"), None),
            &installed
        ));
        assert!(!validation_active(
            false,
            &environment(None, Some("VK_LAYER_LUNARG_*")),
            &installed
        ));
        assert!(!validation_active(
            false,
            &environment(Some("VK_LAYER_KHRONOS_validation_extra"), None),
            &installed
        ));
        // Named, but not installed: the loader will not load it, so nothing is installed. (Recorded mutation: dropping the
        // availability check installs a messenger for a layer the loader will not load.)
        assert!(!validation_active(
            false,
            &environment(Some(LAYER), Some("*khronos*")),
            &layers(&["VK_LAYER_LUNARG_api_dump"])
        ));
        assert!(!validation_active(
            false,
            &environment(Some(LAYER), None),
            &[]
        ));
    }

    /// Sends one message to `log` through the real callback, on this thread, as the loader would.
    fn send(log: &Arc<ValidationLog>, id: &str, text: &str) {
        send_as(log, id, text, vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION);
    }

    /// [`send_as`] with the observed severity of the loader's notice.
    fn send_notice(
        log: &Arc<ValidationLog>,
        id: &str,
        text: &str,
        kinds: vk::DebugUtilsMessageTypeFlagsEXT,
    ) {
        send_with(
            log,
            id,
            text,
            kinds,
            vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
        );
    }

    /// [`send`] with a message type.
    fn send_as(
        log: &Arc<ValidationLog>,
        id: &str,
        text: &str,
        kinds: vk::DebugUtilsMessageTypeFlagsEXT,
    ) {
        send_with(
            log,
            id,
            text,
            kinds,
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
        );
    }

    fn send_with(
        log: &Arc<ValidationLog>,
        id: &str,
        text: &str,
        kinds: vk::DebugUtilsMessageTypeFlagsEXT,
        severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    ) {
        let message = CString::new(text).expect("text");
        let id = CString::new(id).expect("id");
        let data = vk::DebugUtilsMessengerCallbackDataEXT::default()
            .message(&message)
            .message_id_name(&id);
        // SAFETY: `data` is valid for the call and the user data is the log, which outlives it.
        unsafe {
            on_message(
                severity,
                kinds,
                &raw const data,
                Arc::as_ptr(log).cast_mut().cast::<c_void>(),
            );
        }
    }

    #[test]
    fn the_one_vuid_is_tolerated_only_inside_the_table_layouts_scope_after_a_no() {
        let log = Arc::new(ValidationLog::default());
        // Inside the scope, after a no: tolerated, and the clean check holds.
        {
            let _scope = log.layout_scope(true);
            send(&log, TOLERATED_VUID, "layout unsupported");
        }
        assert_eq!(log.snapshot(), Vec::<String>::new());
        assert_eq!(log.tolerated().len(), 1);
        assert!(log.tolerated()[0].contains(TOLERATED_VUID));
        // The same VUID outside the scope fails (the guard was cleared when it dropped).
        send(&log, TOLERATED_VUID, "layout unsupported, elsewhere");
        assert_eq!(log.snapshot().len(), 1);
        assert_eq!(log.tolerated().len(), 1, "still the one");
        let log = Arc::new(ValidationLog::default());
        // Another VUID inside the scope fails.
        {
            let _scope = log.layout_scope(true);
            send(
                &log,
                "VUID-vkCreateDescriptorSetLayout-pBindings-09510",
                "something else",
            );
            send(&log, "", "no id at all");
        }
        assert_eq!(log.snapshot().len(), 2);
        assert!(log.tolerated().is_empty());
        // The VUID inside the scope after a "supported" answer fails.
        let log = Arc::new(ValidationLog::default());
        {
            let _scope = log.layout_scope(false);
            send(&log, TOLERATED_VUID, "layout unsupported");
        }
        assert_eq!(log.snapshot().len(), 1);
        assert!(log.tolerated().is_empty());
    }

    #[test]
    fn the_loaders_notice_about_the_environments_layers_is_tolerated_only_while_the_instance_is_created()
     {
        use std::sync::atomic::Ordering;
        const GENERAL: vk::DebugUtilsMessageTypeFlagsEXT =
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL;
        const VALIDATION: vk::DebugUtilsMessageTypeFlagsEXT =
            vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION;
        let notice = "env var 'VK_INSTANCE_LAYERS' defined and adding layers \"VK_LAYER_KHRONOS_validation\"";
        let log = Arc::new(ValidationLog::default());
        log.creating.store(true, Ordering::SeqCst);
        send_notice(&log, "Loader Message", notice, GENERAL);
        send_notice(
            &log,
            "Loader Message",
            "env var 'VK_LOADER_LAYERS_ENABLE' defined and adding layers \"VK_LAYER_KHRONOS_validation\"",
            GENERAL,
        );
        assert_eq!(
            log.snapshot(),
            Vec::<String>::new(),
            "both notices, while creating"
        );
        assert!(
            log.drain()
                .entries
                .iter()
                .all(|entry| entry.loader_notice && !entry.tolerated)
        );
        // Every other message during creation still fails: another id, another text, another type, another severity, the
        // layout VUID.
        send_notice(&log, "VUID-other", notice, GENERAL);
        send_notice(
            &log,
            "Loader Message",
            "something else the loader says",
            GENERAL,
        );
        send_notice(
            &log,
            "Loader Message",
            "env var 'PATH' defined and adding layers x",
            GENERAL,
        );
        send_notice(&log, "Loader Message", notice, VALIDATION);
        send_notice(&log, "Loader Message", notice, GENERAL | VALIDATION);
        send_as(&log, "Loader Message", notice, GENERAL);
        send(&log, TOLERATED_VUID, "layout");
        assert_eq!(log.snapshot().len(), 7);
        // After creation the same notice fails, and so does it for a log whose instance was never being created.
        let after = Arc::new(ValidationLog::default());
        send_notice(&after, "Loader Message", notice, GENERAL);
        assert_eq!(
            after.snapshot().len(),
            1,
            "not while the instance is created"
        );
        log.creating.store(false, Ordering::SeqCst);
        send_notice(&log, "Loader Message", notice, GENERAL);
        assert_eq!(log.snapshot().len(), 8, "the window closed");
    }

    #[test]
    fn the_scope_belongs_to_its_log_its_thread_and_its_call() {
        let mine = Arc::new(ValidationLog::default());
        let other = Arc::new(ValidationLog::default());
        {
            let _scope = mine.layout_scope(true);
            // Another instance's log is not in scope.
            send(&other, TOLERATED_VUID, "another instance");
            // Another thread is not in scope: its messages are not this call's.
            let from_thread = Arc::clone(&mine);
            std::thread::spawn(move || send(&from_thread, TOLERATED_VUID, "another thread"))
                .join()
                .expect("the thread");
            // A nested scope restores the outer one when it ends, and a scope that says "supported" tolerates nothing.
            {
                let _inner = mine.layout_scope(false);
                send(&mine, TOLERATED_VUID, "inner, supported");
            }
            send(&mine, TOLERATED_VUID, "outer again");
        }
        assert_eq!(other.snapshot().len(), 1, "the other log's message fails");
        assert_eq!(
            mine.snapshot().len(),
            2,
            "the other thread's and the inner one's fail"
        );
        assert_eq!(mine.tolerated().len(), 1, "only the outer scope's own call");
        assert!(mine.tolerated()[0].contains("outer again"));
        // An unwinding call clears the scope too.
        let unwound = Arc::clone(&mine);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _scope = unwound.layout_scope(true);
            panic!("the call failed");
        }));
        assert!(caught.is_err());
        send(&mine, TOLERATED_VUID, "after the unwind");
        assert_eq!(mine.tolerated().len(), 1, "no scope is left behind");
    }

    #[test]
    fn the_log_keeps_256_entries_counts_the_rest_and_drains_each_entry_once() {
        let log = Arc::new(ValidationLog::default());
        {
            let _scope = log.layout_scope(true);
            send(&log, TOLERATED_VUID, "the tolerated one");
        }
        for index in 0..300 {
            send(&log, "VUID-other", &format!("message {index}"));
        }
        let messages = log.snapshot();
        // 255 failing entries fit beside the tolerated one; the other 45 are dropped and counted, and the count fails the check.
        assert_eq!(
            messages.len(),
            LOG_ENTRIES - 1 + 1,
            "{} entries",
            messages.len()
        );
        assert_eq!(
            messages.last().map(String::as_str),
            Some("45 further validation messages were dropped beyond 256")
        );
        let drained = log.drain();
        assert_eq!(drained.entries.len(), LOG_ENTRIES);
        assert_eq!(drained.dropped, 45);
        assert!(drained.entries[0].tolerated && !drained.entries[1].tolerated);
        assert!(drained.entries.iter().all(|entry| !entry.loader_notice));
        let again = log.drain();
        assert!(
            again.entries.is_empty() && again.dropped == 0,
            "each entry is delivered once"
        );
        assert!(log.snapshot().is_empty() && log.tolerated().is_empty());
    }
}
