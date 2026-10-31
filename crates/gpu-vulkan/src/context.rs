//! The shared device context and the render queue (design §6.1, the Vulkan proposal).
//!
//! One [`VulkanContext`] owns the logical device. It offers only device-level calls Vulkan allows from any
//! thread (create and destroy objects, allocate memory), so the upload worker, the render owner and, later,
//! the presenter share it behind an `Arc`. **The queue is not on the context.** [`VulkanContext::take_queue`]
//! hands out, once, the [`RenderQueue`]: every `vkQueueSubmit` goes through `&mut RenderQueue`, which makes
//! Vulkan's external synchronisation of the queue a rule of the types. The worker never holds one.
//!
//! The device is destroyed when the last `Arc` goes, which happens only after a proven end: an unproven end
//! keeps its `Arc`s, so the device, the instance and the loader are retained with everything made from them.
//!
//! **Allocation count.** Every `vkAllocateMemory` the crate makes goes through [`VulkanContext::allocate_memory`],
//! which counts it against `maxMemoryAllocationCount` (one counter, here). Swap-chain images are not
//! application allocations and do not count.

use std::ffi::{CStr, c_char};
use std::sync::{Arc, Mutex, PoisonError};

use ash::vk;
use ash::{ext, khr};

use crate::error::vk_error;
use crate::gpu::{self, Selection};
use crate::instance::InstanceInner;
use crate::loader::Loader;
use crate::memory::SourceError;
use crate::{Error, Instance};

/// The only image format the store creates.
pub(crate) const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;

/// The device extensions the presenter needs (so a context made with a surface can serve it).
const PRESENTER_EXTENSIONS: [&CStr; 2] = [khr::swapchain::NAME, ext::swapchain_maintenance1::NAME];

/// What a test may ask of [`VulkanContext::new_for_tests`]. Not part of the interface.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TestFeatures {
    /// Enable the host-event feature of the portability subset (`events`), which a held frame needs.
    pub events: bool,
}

/// The device limits the texture path reads once.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DeviceLimits {
    pub(crate) max_image_dimension_2d: u32,
    pub(crate) non_coherent_atom_size: u64,
    pub(crate) optimal_buffer_copy_offset_alignment: u64,
}

/// The allocation counter: live `vkAllocateMemory` allocations against the limit.
struct Allocations {
    in_use: u32,
    limit: u32,
}

/// The shared logical device. See the module documentation.
pub struct VulkanContext {
    // Held so the instance and the loader outlive the device; dropped in this order after `Drop` destroys it.
    _instance: Arc<InstanceInner>,
    _loader: Loader,
    family: u32,
    device: ash::Device,
    memory: vk::PhysicalDeviceMemoryProperties,
    limits: DeviceLimits,
    queue: Mutex<Option<vk::Queue>>,
    allocations: Mutex<Allocations>,
}

impl std::fmt::Debug for VulkanContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VulkanContext")
            .field("family", &self.family)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        // SAFETY: the last `Arc` is gone, so nothing made from the device is in use: a proven end destroyed
        // every object first, and an unproven one never drops its `Arc`s.
        unsafe { self.device.destroy_device(None) };
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl VulkanContext {
    /// Picks a physical device with a graphics queue family, creates the device and checks what the texture
    /// path needs of it.
    ///
    /// With a `surface`, the family must also present to it and the presenter's device extensions are
    /// required and enabled, so the presenter can use the same device. `VK_KHR_portability_subset` is enabled
    /// when the device advertises it.
    ///
    /// # Errors
    ///
    /// An [`Error`] naming what the device lacks (a version, a format capability, a limit) with the value found
    /// and the requirement, or the Vulkan error.
    pub fn new(instance: &Instance, surface: Option<vk::SurfaceKHR>) -> Result<Arc<Self>, Error> {
        Self::build(instance, surface, TestFeatures::default())
    }

    /// [`new`](Self::new) with the test features. Not part of the interface: the one reviewed seam for the
    /// host-event hold. On a portability-subset device it enables `events` when asked and refuses when the
    /// device does not report it; elsewhere events are core and nothing extra is enabled.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new), and "portability subset does not support events".
    #[doc(hidden)]
    pub fn new_for_tests(
        instance: &Instance,
        surface: Option<vk::SurfaceKHR>,
        features: TestFeatures,
    ) -> Result<Arc<Self>, Error> {
        Self::build(instance, surface, features)
    }

    fn build(
        instance: &Instance,
        surface: Option<vk::SurfaceKHR>,
        features: TestFeatures,
    ) -> Result<Arc<Self>, Error> {
        let inner = Arc::clone(&instance.inner);
        let selection = match surface {
            Some(surface) => gpu::select(&inner, surface, None).map_err(Error)?,
            None => select_headless(&inner)?,
        };
        let vulkan = &inner.instance;
        // SAFETY: `selection.physical` came from this instance.
        let properties = unsafe { vulkan.get_physical_device_properties(selection.physical) };
        let name = properties
            .device_name_as_c_str()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if properties.api_version < vk::API_VERSION_1_1 {
            return Err(Error(format!(
                "{name} supports Vulkan {}.{}; the texture store needs 1.1 (vkGetImageMemoryRequirements2)",
                vk::api_version_major(properties.api_version),
                vk::api_version_minor(properties.api_version)
            )));
        }
        check_format(vulkan, selection.physical, &name)?;
        let limits = check_limits(&properties.limits, &name)?;
        let events = events_feature(vulkan, &selection, features, &name)?;

        let priorities = [1.0_f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(selection.family)
            .queue_priorities(&priorities)];
        let mut names: Vec<*const c_char> = Vec::new();
        if surface.is_some() {
            names.extend(PRESENTER_EXTENSIONS.iter().map(|name| name.as_ptr()));
        }
        if selection.portability_subset {
            names.push(khr::portability_subset::NAME.as_ptr());
        }
        let mut maintenance = vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default()
            .swapchain_maintenance1(true);
        let mut subset = vk::PhysicalDevicePortabilitySubsetFeaturesKHR::default().events(true);
        let mut info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_info)
            .enabled_extension_names(&names);
        if surface.is_some() {
            info = info.push_next(&mut maintenance);
        }
        if events {
            info = info.push_next(&mut subset);
        }
        // SAFETY: the instance is live; `info` and everything it points to live until the call returns.
        let device = unsafe { vulkan.create_device(selection.physical, &info, None) }
            .map_err(|code| vk_error("vkCreateDevice", code))?;
        // SAFETY: the family index was requested in the create info.
        let queue = unsafe { device.get_device_queue(selection.family, 0) };
        // SAFETY: `selection.physical` came from this instance.
        let memory = unsafe { vulkan.get_physical_device_memory_properties(selection.physical) };
        let allocation_limit = properties.limits.max_memory_allocation_count;
        Ok(Arc::new(Self {
            _loader: instance.loader().clone(),
            _instance: inner,
            family: selection.family,
            device,
            memory,
            limits,
            queue: Mutex::new(Some(queue)),
            allocations: Mutex::new(Allocations {
                in_use: 0,
                limit: allocation_limit,
            }),
        }))
    }

    /// The render queue, handed out once.
    ///
    /// # Errors
    ///
    /// An [`Error`] when it was already taken.
    pub fn take_queue(self: &Arc<Self>) -> Result<RenderQueue, Error> {
        let queue = lock(&self.queue)
            .take()
            .ok_or_else(|| Error("the render queue was already taken".to_owned()))?;
        Ok(RenderQueue {
            context: Arc::clone(self),
            queue,
            injected: None,
            _not_sync: std::marker::PhantomData,
        })
    }

    /// The logical device, for the tests' own commands.
    #[doc(hidden)]
    #[must_use]
    pub fn device(&self) -> &ash::Device {
        &self.device
    }

    /// The queue family the device was created with.
    #[doc(hidden)]
    #[must_use]
    pub fn family(&self) -> u32 {
        self.family
    }

    /// The device's memory types, for the tests' own buffers.
    #[doc(hidden)]
    pub fn memory_properties(&self) -> vk::PhysicalDeviceMemoryProperties {
        self.memory
    }

    /// The live `vkAllocateMemory` allocations the crate made and the limit they count against.
    #[doc(hidden)]
    #[must_use]
    pub fn allocation_count(&self) -> (u32, u32) {
        let allocations = lock(&self.allocations);
        (allocations.in_use, allocations.limit)
    }

    /// Lowers the allocation-count limit (never raises it past the device's): the reviewed test seam for the
    /// count reason.
    #[doc(hidden)]
    pub fn lower_allocation_limit(&self, limit: u32) {
        let mut allocations = lock(&self.allocations);
        allocations.limit = allocations.limit.min(limit);
    }

    pub(crate) fn limits(&self) -> &DeviceLimits {
        &self.limits
    }

    /// A memory type allowed by `bits` that has `required`, preferring one that also has `preferred`.
    pub(crate) fn memory_type(
        &self,
        bits: u32,
        required: vk::MemoryPropertyFlags,
        preferred: vk::MemoryPropertyFlags,
    ) -> Option<u32> {
        pick_memory_type(&self.memory, bits, required, preferred)
    }

    /// Whether a memory type is host-coherent.
    pub(crate) fn is_coherent(&self, memory_type: u32) -> bool {
        usize::try_from(memory_type)
            .ok()
            .and_then(|index| self.memory.memory_types.get(index))
            .is_some_and(|kind| {
                kind.property_flags
                    .contains(vk::MemoryPropertyFlags::HOST_COHERENT)
            })
    }

    /// One counted `vkAllocateMemory`. `dedicated` names the image that gets the whole allocation.
    pub(crate) fn allocate_memory(
        &self,
        size: u64,
        memory_type: u32,
        dedicated: Option<vk::Image>,
    ) -> Result<vk::DeviceMemory, SourceError> {
        {
            let mut allocations = lock(&self.allocations);
            if allocations.in_use >= allocations.limit {
                return Err(SourceError::Count {
                    in_use: allocations.in_use,
                    limit: allocations.limit,
                });
            }
            allocations.in_use += 1;
        }
        let mut dedicated_info = vk::MemoryDedicatedAllocateInfo::default();
        let mut info = vk::MemoryAllocateInfo::default()
            .allocation_size(size)
            .memory_type_index(memory_type);
        if let Some(image) = dedicated {
            dedicated_info = dedicated_info.image(image);
            info = info.push_next(&mut dedicated_info);
        }
        // SAFETY: the device is live and `info` (with the dedicated chain) is valid for the call.
        match unsafe { self.device.allocate_memory(&info, None) } {
            Ok(memory) => Ok(memory),
            Err(code) => {
                lock(&self.allocations).in_use -= 1;
                Err(SourceError::Device(format!(
                    "vkAllocateMemory({size} bytes, type {memory_type}) failed: {code:?}"
                )))
            }
        }
    }

    /// Frees an allocation `allocate_memory` made.
    pub(crate) fn free_memory(&self, memory: vk::DeviceMemory) {
        // SAFETY: the memory belongs to this device and nothing uses it (the caller's contract: its images and
        // buffers are destroyed, or it was never bound).
        unsafe { self.device.free_memory(memory, None) };
        let mut allocations = lock(&self.allocations);
        allocations.in_use = allocations.in_use.saturating_sub(1);
    }
}

/// The one queue, owned by the render owner: neither `Sync` nor `Clone`, so every `vkQueueSubmit` has
/// exclusive access by construction. It is `Send` (the owner may move between threads, never share).
///
/// ```compile_fail
/// fn needs_sync<T: Sync>() {}
/// needs_sync::<ck_gpu_vulkan::RenderQueue>();
/// ```
///
/// ```compile_fail
/// fn needs_clone<T: Clone>() {}
/// needs_clone::<ck_gpu_vulkan::RenderQueue>();
/// ```
pub struct RenderQueue {
    context: Arc<VulkanContext>,
    queue: vk::Queue,
    /// A test's injected result for the next submit (see [`inject_submit_error`](Self::inject_submit_error)).
    injected: Option<vk::Result>,
    _not_sync: std::marker::PhantomData<std::cell::Cell<()>>,
}

impl std::fmt::Debug for RenderQueue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RenderQueue")
            .finish_non_exhaustive()
    }
}

impl RenderQueue {
    /// The context this queue belongs to.
    #[must_use]
    pub fn context(&self) -> &Arc<VulkanContext> {
        &self.context
    }

    /// Makes the next submit fail with `result` without calling Vulkan (`None` clears it). The reviewed test
    /// seam for a submit that fails: out of memory is never-submitted, device loss is uncertain.
    #[doc(hidden)]
    pub fn inject_submit_error(&mut self, result: Option<vk::Result>) {
        self.injected = result;
    }

    /// `vkQueueSubmit` of one command buffer with `fence`, for the tests' own commands.
    ///
    /// # Errors
    ///
    /// The Vulkan result.
    #[doc(hidden)]
    pub fn submit_commands(
        &mut self,
        buffer: vk::CommandBuffer,
        fence: vk::Fence,
    ) -> Result<(), vk::Result> {
        let buffers = [buffer];
        self.submit(&vk::SubmitInfo::default().command_buffers(&buffers), fence)
    }

    /// `vkQueueSubmit` of one batch with `fence`.
    pub(crate) fn submit(
        &mut self,
        info: &vk::SubmitInfo<'_>,
        fence: vk::Fence,
    ) -> Result<(), vk::Result> {
        if let Some(result) = self.injected.take() {
            return Err(result);
        }
        // SAFETY: `&mut self` is exclusive access to the queue; the fence belongs to this device and is
        // unsignalled; `info` and what it points to are valid for the call.
        unsafe {
            self.context
                .device
                .queue_submit(self.queue, std::slice::from_ref(info), fence)
        }
    }
}

/// A memory type allowed by `bits` that has `required`, preferring one that also has `preferred`.
fn pick_memory_type(
    memory: &vk::PhysicalDeviceMemoryProperties,
    bits: u32,
    required: vk::MemoryPropertyFlags,
    preferred: vk::MemoryPropertyFlags,
) -> Option<u32> {
    let count = usize::try_from(memory.memory_type_count).ok()?;
    let allowed = |index: usize| {
        bits & (1 << index) != 0 && memory.memory_types[index].property_flags.contains(required)
    };
    (0..count)
        .find(|&index| {
            allowed(index)
                && memory.memory_types[index]
                    .property_flags
                    .contains(preferred)
        })
        .or_else(|| (0..count).find(|&index| allowed(index)))
        .and_then(|index| u32::try_from(index).ok())
}

/// A physical device with a graphics queue family (no surface), the best kind first.
fn select_headless(inner: &InstanceInner) -> Result<Selection, Error> {
    let instance = &inner.instance;
    // SAFETY: the instance is live and the call takes no pointer.
    let devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|code| vk_error("vkEnumeratePhysicalDevices", code))?;
    let mut best: Option<(u32, Selection)> = None;
    for physical in devices {
        // SAFETY: `physical` came from this instance.
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        let Some(family) = families
            .iter()
            .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        else {
            continue;
        };
        // SAFETY: `physical` came from this instance.
        let properties = unsafe { instance.get_physical_device_properties(physical) };
        // SAFETY: `physical` came from this instance.
        let subset = unsafe { instance.enumerate_device_extension_properties(physical) }
            .unwrap_or_default()
            .iter()
            .any(|property| {
                property
                    .extension_name_as_c_str()
                    .is_ok_and(|name| name == khr::portability_subset::NAME)
            });
        let rank = match properties.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 3,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
            _ => 1,
        };
        let selection = Selection {
            physical,
            family: u32::try_from(family).unwrap_or(0),
            portability_subset: subset,
        };
        if best.as_ref().is_none_or(|(held, _)| rank > *held) {
            best = Some((rank, selection));
        }
    }
    best.map(|(_, selection)| selection)
        .ok_or_else(|| Error("no Vulkan physical device has a graphics queue family".to_owned()))
}

/// `R8G8B8A8_UNORM` with optimal tiling must be sampled, a transfer destination and a transfer source.
fn check_format(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    name: &str,
) -> Result<(), Error> {
    // SAFETY: `physical` came from this instance.
    let properties = unsafe { instance.get_physical_device_format_properties(physical, FORMAT) };
    let have = properties.optimal_tiling_features;
    let mut missing = Vec::new();
    for (feature, label) in [
        (vk::FormatFeatureFlags::SAMPLED_IMAGE, "sampling"),
        (vk::FormatFeatureFlags::TRANSFER_DST, "transfer destination"),
        (vk::FormatFeatureFlags::TRANSFER_SRC, "transfer source"),
    ] {
        if !have.contains(feature) {
            missing.push(label);
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Error(format!(
            "{name} cannot use R8G8B8A8_UNORM with optimal tiling for {} (features {have:?})",
            missing.join(", ")
        )))
    }
}

/// The descriptor limits the ordinary stage needs, and the limits the store reads.
fn check_limits(limits: &vk::PhysicalDeviceLimits, name: &str) -> Result<DeviceLimits, Error> {
    for (label, value, needed) in [
        (
            "maxBoundDescriptorSets",
            limits.max_bound_descriptor_sets,
            2,
        ),
        (
            "maxPerStageDescriptorSamplers",
            limits.max_per_stage_descriptor_samplers,
            1,
        ),
        (
            "maxPerStageDescriptorSampledImages",
            limits.max_per_stage_descriptor_sampled_images,
            1,
        ),
        (
            "maxMemoryAllocationCount",
            limits.max_memory_allocation_count,
            1,
        ),
    ] {
        if value < needed {
            return Err(Error(format!(
                "{name} reports {label} = {value}; the texture store needs at least {needed}"
            )));
        }
    }
    Ok(DeviceLimits {
        max_image_dimension_2d: limits.max_image_dimension2_d,
        non_coherent_atom_size: limits.non_coherent_atom_size.max(1),
        optimal_buffer_copy_offset_alignment: limits.optimal_buffer_copy_offset_alignment.max(1),
    })
}

/// Whether to enable the portability subset's `events` feature: only when a test asked, only on a device with
/// the subset, and a refusal when the device does not report it.
fn events_feature(
    instance: &ash::Instance,
    selection: &Selection,
    features: TestFeatures,
    name: &str,
) -> Result<bool, Error> {
    if !features.events || !selection.portability_subset {
        return Ok(false);
    }
    let mut subset = vk::PhysicalDevicePortabilitySubsetFeaturesKHR::default();
    let mut query = vk::PhysicalDeviceFeatures2::default().push_next(&mut subset);
    // SAFETY: `selection.physical` came from this instance and `query` is valid for the call.
    unsafe { instance.get_physical_device_features2(selection.physical, &mut query) };
    if subset.events == vk::TRUE {
        Ok(true)
    } else {
        Err(Error(format!(
            "{name}: portability subset does not support events"
        )))
    }
}

#[cfg(test)]
mod tests {
    use ash::vk;

    use super::{RenderQueue, VulkanContext, check_limits, pick_memory_type};

    fn memory(types: &[vk::MemoryPropertyFlags]) -> vk::PhysicalDeviceMemoryProperties {
        let mut properties = vk::PhysicalDeviceMemoryProperties {
            memory_type_count: u32::try_from(types.len()).expect("a few types"),
            ..Default::default()
        };
        for (index, flags) in types.iter().enumerate() {
            properties.memory_types[index].property_flags = *flags;
        }
        properties
    }

    #[test]
    fn a_memory_type_prefers_the_wanted_flag_among_the_allowed_types() {
        let local = vk::MemoryPropertyFlags::DEVICE_LOCAL;
        let visible = vk::MemoryPropertyFlags::HOST_VISIBLE;
        let coherent = vk::MemoryPropertyFlags::HOST_COHERENT;
        let properties = memory(&[visible, local, visible | coherent]);
        let none = vk::MemoryPropertyFlags::empty();
        // Images: any allowed type, device-local first.
        assert_eq!(pick_memory_type(&properties, 0b111, none, local), Some(1));
        // The preference is dropped when no allowed type has it.
        assert_eq!(pick_memory_type(&properties, 0b101, none, local), Some(0));
        // Staging: host-visible is required, coherent preferred.
        assert_eq!(
            pick_memory_type(&properties, 0b111, visible, coherent),
            Some(2)
        );
        assert_eq!(
            pick_memory_type(&properties, 0b011, visible, coherent),
            Some(0)
        );
        // Nothing allowed has the required flag.
        assert_eq!(
            pick_memory_type(&properties, 0b010, visible, coherent),
            None
        );
        assert_eq!(pick_memory_type(&properties, 0, none, none), None);
    }

    #[test]
    fn a_device_limit_below_the_requirement_is_refused_naming_value_and_requirement() {
        let good = vk::PhysicalDeviceLimits {
            max_bound_descriptor_sets: 4,
            max_per_stage_descriptor_samplers: 16,
            max_per_stage_descriptor_sampled_images: 16,
            max_memory_allocation_count: 4096,
            max_image_dimension2_d: 8192,
            non_coherent_atom_size: 64,
            optimal_buffer_copy_offset_alignment: 1,
            ..Default::default()
        };
        let limits = check_limits(&good, "gpu").expect("a device that meets every limit");
        assert_eq!(limits.max_image_dimension_2d, 8192);
        assert_eq!(limits.non_coherent_atom_size, 64);
        // The alignments are read as at least one, so a zero never reaches a division.
        let zero = vk::PhysicalDeviceLimits {
            non_coherent_atom_size: 0,
            optimal_buffer_copy_offset_alignment: 0,
            ..good
        };
        let limits = check_limits(&zero, "gpu").expect("zero alignments read as one");
        assert_eq!(
            (
                limits.non_coherent_atom_size,
                limits.optimal_buffer_copy_offset_alignment
            ),
            (1, 1)
        );
        let cases = [
            (
                vk::PhysicalDeviceLimits {
                    max_bound_descriptor_sets: 1,
                    ..good
                },
                "maxBoundDescriptorSets = 1; the texture store needs at least 2",
            ),
            (
                vk::PhysicalDeviceLimits {
                    max_per_stage_descriptor_samplers: 0,
                    ..good
                },
                "maxPerStageDescriptorSamplers = 0; the texture store needs at least 1",
            ),
            (
                vk::PhysicalDeviceLimits {
                    max_per_stage_descriptor_sampled_images: 0,
                    ..good
                },
                "maxPerStageDescriptorSampledImages = 0",
            ),
            (
                vk::PhysicalDeviceLimits {
                    max_memory_allocation_count: 0,
                    ..good
                },
                "maxMemoryAllocationCount = 0",
            ),
        ];
        for (limits, expected) in cases {
            let message = check_limits(&limits, "gpu")
                .expect_err("below the requirement")
                .to_string();
            assert!(message.contains(expected), "{message} names {expected}");
        }
    }

    /// The context is shared across threads; the queue moves between them but is never shared.
    #[test]
    fn the_context_is_send_and_sync_and_the_queue_is_send() {
        fn send<T: Send>() {}
        fn sync<T: Sync>() {}
        send::<VulkanContext>();
        sync::<VulkanContext>();
        send::<RenderQueue>();
    }
}
