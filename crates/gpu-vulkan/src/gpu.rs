//! The render thread's device-level Vulkan objects and the one-call wrappers over them.
//!
//! Nothing here has a `Drop`: `ash` handles are plain values, so a thread that ends by unwinding releases
//! nothing, and only the explicit [`Gpu::destroy_all`] (after the ledger proved every operation finished)
//! destroys. Every method makes one Vulkan call (or a short fixed sequence of them), each in its own
//! `unsafe` block with a `SAFETY` comment.

use std::ffi::{CStr, c_char};
use std::mem::ManuallyDrop;
use std::sync::Arc;

use ash::vk;
use ash::{ext, google, khr};

use crate::capture::Capture;
use crate::draw::QuadParams;
use crate::gpu_quad::QuadObjects;
use crate::hooks::{Event, Observer, SurfaceExtent};
use crate::instance::InstanceInner;
use crate::loader::Loader;

/// The device extensions window presentation needs; a missing one is named.
const REQUIRED_DEVICE_EXTENSIONS: [&CStr; 2] =
    [khr::swapchain::NAME, ext::swapchain_maintenance1::NAME];

/// A physical device picked for the surface.
pub(crate) struct Selection {
    pub(crate) physical: vk::PhysicalDevice,
    pub(crate) family: u32,
    /// The device advertises `VK_KHR_portability_subset`, which must then be enabled.
    pub(crate) portability_subset: bool,
}

fn extension_names(properties: &[vk::ExtensionProperties]) -> Vec<String> {
    properties
        .iter()
        .filter_map(|property| property.extension_name_as_c_str().ok())
        .map(|name| name.to_string_lossy().into_owned())
        .collect()
}

/// Picks a physical device with a graphics queue that can present to `surface`, `VK_KHR_swapchain` and
/// `VK_EXT_swapchain_maintenance1` with its feature enabled. `hide` makes one extension read as absent (a
/// test hook).
///
/// # Errors
///
/// A message naming what was missing, for the first device that came closest.
pub(crate) fn select(
    inner: &InstanceInner,
    surface: vk::SurfaceKHR,
    hide: Option<&str>,
) -> Result<Selection, String> {
    let instance = &inner.instance;
    // SAFETY: the instance is live (the caller holds a clone of `inner`) and the call takes no pointer.
    let devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|code| format!("vkEnumeratePhysicalDevices failed: {code:?}"))?;
    if devices.is_empty() {
        return Err("no Vulkan physical device is available".to_owned());
    }
    let mut first_reason: Option<String> = None;
    let mut best: Option<(u32, Selection)> = None;
    for physical in devices {
        // SAFETY: `physical` came from this instance.
        let properties = unsafe { instance.get_physical_device_properties(physical) };
        let name = properties
            .device_name_as_c_str()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        // SAFETY: `physical` came from this instance.
        let advertised = unsafe { instance.enumerate_device_extension_properties(physical) }
            .map(|list| extension_names(&list))
            .unwrap_or_default();
        let has = |wanted: &CStr| {
            let text = wanted.to_string_lossy();
            hide != Some(&*text) && advertised.iter().any(|have| *have == *text)
        };
        let mut reason = None;
        for wanted in REQUIRED_DEVICE_EXTENSIONS {
            if !has(wanted) {
                reason = Some(format!(
                    "{name} does not advertise the device extension {}",
                    wanted.to_string_lossy()
                ));
                break;
            }
        }
        if reason.is_none() {
            let mut maintenance = vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default();
            let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut maintenance);
            // SAFETY: `physical` came from this instance and `features` is valid for the call.
            unsafe { instance.get_physical_device_features2(physical, &mut features) };
            if maintenance.swapchain_maintenance1 == vk::FALSE {
                reason = Some(format!(
                    "{name} does not support the swapchainMaintenance1 feature of VK_EXT_swapchain_maintenance1"
                ));
            }
        }
        let mut family = None;
        if reason.is_none() {
            // SAFETY: `physical` came from this instance.
            let families =
                unsafe { instance.get_physical_device_queue_family_properties(physical) };
            for (index, properties) in families.iter().enumerate() {
                let index = u32::try_from(index).unwrap_or(u32::MAX);
                if !properties.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                    continue;
                }
                // SAFETY: `physical` and `surface` belong to this instance (the caller's contract).
                let supported = unsafe {
                    inner
                        .surface
                        .get_physical_device_surface_support(physical, index, surface)
                }
                .unwrap_or(false);
                if supported {
                    family = Some(index);
                    break;
                }
            }
            if family.is_none() {
                reason = Some(format!(
                    "{name} has no graphics queue family that can present to the surface"
                ));
            }
        }
        match (reason, family) {
            (None, Some(family)) => {
                let rank = match properties.device_type {
                    vk::PhysicalDeviceType::DISCRETE_GPU => 3,
                    vk::PhysicalDeviceType::INTEGRATED_GPU => 2,
                    _ => 1,
                };
                let chosen = Selection {
                    physical,
                    family,
                    portability_subset: has(khr::portability_subset::NAME),
                };
                if best.as_ref().is_none_or(|(held, _)| rank > *held) {
                    best = Some((rank, chosen));
                }
            }
            (Some(reason), _) => {
                first_reason.get_or_insert(reason);
            }
            (None, None) => {}
        }
    }
    best.map(|(_, selection)| selection).ok_or_else(|| {
        first_reason.unwrap_or_else(|| "no suitable Vulkan physical device".to_owned())
    })
}

/// A swap chain with its image views and framebuffers.
pub(crate) struct Chain {
    pub(crate) id: u64,
    pub(crate) handle: vk::SwapchainKHR,
    /// The size the window asked for when this was created (the extent may be clamped).
    pub(crate) requested: (u32, u32),
    pub(crate) extent: vk::Extent2D,
    /// The surface transform it was created with.
    pub(crate) transform: vk::SurfaceTransformFlagsKHR,
    pub(crate) images: Vec<vk::Image>,
    pub(crate) views: Vec<vk::ImageView>,
    pub(crate) framebuffers: Vec<vk::Framebuffer>,
}

/// Everything the render thread creates on the device.
pub(crate) struct Gpu {
    /// This thread's hold on the instance. In a `ManuallyDrop`, so a thread that unwinds keeps the
    /// instance alive; [`Gpu::release_holders`] drops it at a proven end only.
    pub(crate) inner: ManuallyDrop<Arc<InstanceInner>>,
    /// This thread's own clone of the loader, kept the same way.
    loader: ManuallyDrop<Loader>,
    pub(crate) surface: vk::SurfaceKHR,
    pub(crate) selection: Selection,
    pub(crate) device: ash::Device,
    pub(crate) swapchain: khr::swapchain::Device,
    pub(crate) maintenance: ext::swapchain_maintenance1::Device,
    pub(crate) queue: vk::Queue,
    pub(crate) pool: Option<vk::CommandPool>,
    pub(crate) cmd: Option<vk::CommandBuffer>,
    pub(crate) render_pass: Option<vk::RenderPass>,
    pub(crate) format: vk::SurfaceFormatKHR,
    pub(crate) semaphores: Vec<vk::Semaphore>,
    pub(crate) fences: Vec<vk::Fence>,
    pub(crate) current: Option<Chain>,
    pub(crate) retired: Option<Chain>,
    pub(crate) observer: Option<Arc<Observer>>,
    /// A test's surface `currentExtent`, used in place of the real one (see `TestHooks::surface_extent`).
    pub(crate) extent_override: Option<SurfaceExtent>,
    /// The quad pipeline and textures (nothing is created unless the presenter has a slot and the format is
    /// supported).
    pub(crate) quad: QuadObjects,
    /// The device's memory types, for the quad's allocations.
    pub(crate) memory: vk::PhysicalDeviceMemoryProperties,
    /// Every format the surface offered (before a test restricted them), set when the render pass is created.
    pub(crate) offered_formats: Vec<vk::Format>,
    /// A test asked for copies of the drawn frames: the swap chain's images get `TRANSFER_SRC` usage.
    pub(crate) capture_usage: bool,
    /// The buffer a drawn frame is copied into, for the test.
    pub(crate) capture: Option<Capture>,
    /// The device is this thread's own (`start`, `start_2d`): [`Gpu::destroy_all`] destroys it. A scene presenter draws
    /// on its context's device, which the context destroys when its last `Arc` goes, after a proven end.
    owns_device: bool,
    /// The scene presenter's two timestamp queries for each of two frames (nothing is created without valid timestamp bits).
    timestamps: Option<vk::QueryPool>,
    /// `VK_GOOGLE_display_timing`'s device functions, when the context enabled it.
    pub(crate) display: Option<google::display_timing::Device>,
}

/// What a scene frame's commands record besides the drawing: a test's host-event hold, and the first of the frame's two timestamp
/// queries (none: the device has no valid timestamp bits).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FrameExtras {
    pub(crate) hold: Option<vk::Event>,
    pub(crate) stamps: Option<u32>,
}

/// A Vulkan error from one of the wrappers, with the call that failed.
pub(crate) type Failure = (&'static str, vk::Result);

/// The raw handles of the fixed pools.
pub(crate) struct Pools {
    pub(crate) acquire: Vec<u64>,
    /// Render-finished semaphore and present fence pairs.
    pub(crate) slots: Vec<(u64, u64)>,
    pub(crate) fences: Vec<u64>,
}

impl Gpu {
    /// Creates the device and its queue.
    pub(crate) fn create_device(
        inner: Arc<InstanceInner>,
        loader: Loader,
        surface: vk::SurfaceKHR,
        selection: Selection,
        observer: Option<Arc<Observer>>,
    ) -> Result<Self, Failure> {
        let priorities = [1.0_f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(selection.family)
            .queue_priorities(&priorities)];
        let mut names: Vec<*const c_char> = REQUIRED_DEVICE_EXTENSIONS
            .iter()
            .map(|name| name.as_ptr())
            .collect();
        if selection.portability_subset {
            names.push(khr::portability_subset::NAME.as_ptr());
        }
        let mut maintenance_features =
            vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default()
                .swapchain_maintenance1(true);
        let info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_info)
            .enabled_extension_names(&names)
            .push_next(&mut maintenance_features);
        // SAFETY: the instance is live; `info` and everything it points to live until the call returns.
        let device = unsafe {
            inner
                .instance
                .create_device(selection.physical, &info, None)
        }
        .map_err(|code| ("vkCreateDevice", code))?;
        // SAFETY: the family index was requested in the create info.
        let queue = unsafe { device.get_device_queue(selection.family, 0) };
        // SAFETY: `selection.physical` came from this instance.
        let memory = unsafe {
            inner
                .instance
                .get_physical_device_memory_properties(selection.physical)
        };
        let swapchain = khr::swapchain::Device::new(&inner.instance, &device);
        let maintenance = ext::swapchain_maintenance1::Device::new(&inner.instance, &device);
        Ok(Self {
            inner: ManuallyDrop::new(inner),
            loader: ManuallyDrop::new(loader),
            surface,
            selection,
            device,
            swapchain,
            maintenance,
            queue,
            pool: None,
            cmd: None,
            render_pass: None,
            format: vk::SurfaceFormatKHR::default(),
            semaphores: Vec::new(),
            fences: Vec::new(),
            current: None,
            retired: None,
            observer,
            extent_override: None,
            quad: QuadObjects::default(),
            memory,
            offered_formats: Vec::new(),
            capture_usage: false,
            capture: None,
            owns_device: true,
            timestamps: None,
            display: None,
        })
    }

    /// The thread's objects on the device of `context`, which was made for this surface (M5.5-1). The device, the
    /// instance and the loader belong to the context: this holds clones of them (in `ManuallyDrop`, like the thread's
    /// other holders) and [`Gpu::destroy_all`] does not destroy the device. Every queue operation goes through the
    /// `RenderQueue`, so the `queue` handle here is null and never used.
    pub(crate) fn on_context(
        context: &crate::context::VulkanContext,
        surface: vk::SurfaceKHR,
        observer: Option<Arc<Observer>>,
    ) -> Self {
        let (inner, loader) = context.holders();
        let device = context.device().clone();
        let swapchain = khr::swapchain::Device::new(&inner.instance, &device);
        let maintenance = ext::swapchain_maintenance1::Device::new(&inner.instance, &device);
        let inner_instance = inner.instance.clone();
        let device_for_display = device.clone();
        Self {
            inner: ManuallyDrop::new(inner),
            loader: ManuallyDrop::new(loader),
            surface,
            selection: context.selection(),
            device,
            swapchain,
            maintenance,
            queue: vk::Queue::null(),
            pool: None,
            cmd: None,
            render_pass: None,
            format: vk::SurfaceFormatKHR::default(),
            semaphores: Vec::new(),
            fences: Vec::new(),
            current: None,
            retired: None,
            observer,
            extent_override: None,
            quad: QuadObjects::default(),
            memory: context.memory_properties(),
            offered_formats: Vec::new(),
            capture_usage: false,
            capture: None,
            owns_device: false,
            timestamps: None,
            display: context
                .timing_caps()
                .display_timing
                .then(|| google::display_timing::Device::new(&inner_instance, &device_for_display)),
        }
    }

    /// The command pool and its one command buffer.
    pub(crate) fn create_commands(&mut self) -> Result<(), Failure> {
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(self.selection.family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: the device is live and `pool_info` is valid for the call.
        let pool = unsafe { self.device.create_command_pool(&pool_info, None) }
            .map_err(|code| ("vkCreateCommandPool", code))?;
        self.pool = Some(pool);
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool is live and `allocate` is valid for the call.
        let buffers = unsafe { self.device.allocate_command_buffers(&allocate) }
            .map_err(|code| ("vkAllocateCommandBuffers", code))?;
        self.cmd = buffers.first().copied();
        Ok(())
    }

    /// The surface's capabilities (with a test's `currentExtent` in place of the real one, when set).
    pub(crate) fn capabilities(&self) -> Result<vk::SurfaceCapabilitiesKHR, Failure> {
        // SAFETY: the physical device and the surface belong to the instance.
        let mut caps = unsafe {
            self.inner
                .surface
                .get_physical_device_surface_capabilities(self.selection.physical, self.surface)
        }
        .map_err(|code| ("vkGetPhysicalDeviceSurfaceCapabilitiesKHR", code))?;
        if let Some(over) = &self.extent_override
            && let Some((width, height)) = *over
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            caps.current_extent = vk::Extent2D { width, height };
        }
        Ok(caps)
    }

    /// The extent a swap chain is created with now, **normalised**: the surface's `currentExtent` when it is
    /// defined (not the `0xFFFF_FFFF` sentinel), the window's pixel size otherwise, either way clamped to the
    /// surface's minimum and maximum. `None` when a dimension is then zero (the surface is not drawable:
    /// rendering is suspended and no swap chain is created). Creation and the comparison that decides
    /// whether a suboptimal result needs a rebuild use this one function, so they cannot disagree.
    pub(crate) fn target_extent(
        caps: &vk::SurfaceCapabilitiesKHR,
        size: (u32, u32),
    ) -> Option<vk::Extent2D> {
        let wanted = if caps.current_extent.width == u32::MAX {
            size
        } else {
            (caps.current_extent.width, caps.current_extent.height)
        };
        if wanted.0 == 0 || wanted.1 == 0 {
            // A zero dimension is a surface that is not drawable (a minimised window); the minimum limit must
            // not lift it into a 1 x 1 swap chain.
            return None;
        }
        let width = wanted
            .0
            .max(caps.min_image_extent.width)
            .min(caps.max_image_extent.width);
        let height = wanted
            .1
            .max(caps.min_image_extent.height)
            .min(caps.max_image_extent.height);
        (width > 0 && height > 0).then_some(vk::Extent2D { width, height })
    }

    /// Chooses the surface format (BGRA8 UNORM, then BGRA8 sRGB, then the first) and creates the render
    /// pass that clears it (load op clear, final layout present).
    ///
    /// `only` restricts the formats the surface is seen to offer to those in the list (a test: a subset of what
    /// the real surface offers, never an invented format); the offered formats are kept in
    /// [`Gpu::offered_formats`].
    pub(crate) fn create_render_pass(
        &mut self,
        only: Option<&[vk::Format]>,
    ) -> Result<(), Failure> {
        // SAFETY: the physical device and the surface belong to the instance.
        let offered = unsafe {
            self.inner
                .surface
                .get_physical_device_surface_formats(self.selection.physical, self.surface)
        }
        .map_err(|code| ("vkGetPhysicalDeviceSurfaceFormatsKHR", code))?;
        self.offered_formats = offered.iter().map(|format| format.format).collect();
        let formats: Vec<vk::SurfaceFormatKHR> = offered
            .into_iter()
            .filter(|format| only.is_none_or(|list| list.contains(&format.format)))
            .collect();
        let pick = |wanted: vk::Format| {
            formats.iter().copied().find(|format| {
                format.format == wanted && format.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
            })
        };
        let Some(format) = pick(vk::Format::B8G8R8A8_UNORM)
            .or_else(|| pick(vk::Format::B8G8R8A8_SRGB))
            .or_else(|| formats.first().copied())
        else {
            return Err((
                "vkGetPhysicalDeviceSurfaceFormatsKHR",
                vk::Result::ERROR_FORMAT_NOT_SUPPORTED,
            ));
        };
        self.format = format;
        let attachments = [vk::AttachmentDescription::default()
            .format(format.format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];
        let color = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpasses = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color)];
        let dependencies = [vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)];
        let info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        // SAFETY: the device is live and `info` and what it points to are valid for the call.
        let pass = unsafe { self.device.create_render_pass(&info, None) }
            .map_err(|code| ("vkCreateRenderPass", code))?;
        self.render_pass = Some(pass);
        Ok(())
    }

    /// Creates the semaphores and fences of the fixed pools; returns their raw handles.
    pub(crate) fn create_pools(
        &mut self,
        acquire: usize,
        slots: usize,
        fences: usize,
    ) -> Result<Pools, Failure> {
        use ash::vk::Handle as _;
        let mut acquire_pool = Vec::new();
        for _ in 0..acquire {
            acquire_pool.push(self.new_semaphore()?.as_raw());
        }
        let mut slot_pool = Vec::new();
        for _ in 0..slots {
            let semaphore = self.new_semaphore()?.as_raw();
            let fence = self.new_fence()?.as_raw();
            slot_pool.push((semaphore, fence));
        }
        let mut fence_pool = Vec::new();
        for _ in 0..fences {
            fence_pool.push(self.new_fence()?.as_raw());
        }
        Ok(Pools {
            acquire: acquire_pool,
            slots: slot_pool,
            fences: fence_pool,
        })
    }

    fn new_semaphore(&mut self) -> Result<vk::Semaphore, Failure> {
        let info = vk::SemaphoreCreateInfo::default();
        // SAFETY: the device is live and `info` is valid for the call.
        let semaphore = unsafe { self.device.create_semaphore(&info, None) }
            .map_err(|code| ("vkCreateSemaphore", code))?;
        self.semaphores.push(semaphore);
        Ok(semaphore)
    }

    fn new_fence(&mut self) -> Result<vk::Fence, Failure> {
        let info = vk::FenceCreateInfo::default();
        // SAFETY: the device is live and `info` is valid for the call.
        let fence = unsafe { self.device.create_fence(&info, None) }
            .map_err(|code| ("vkCreateFence", code))?;
        self.fences.push(fence);
        Ok(fence)
    }

    /// Creates a swap chain (with `old` as `oldSwapchain`), its views and framebuffers. On a failure after
    /// the swap chain exists, the partial objects are destroyed at once: nothing was ever submitted to them.
    pub(crate) fn create_chain(
        &mut self,
        id: u64,
        size: (u32, u32),
        old: vk::SwapchainKHR,
        image_count: u32,
    ) -> Result<Chain, Failure> {
        let caps = self.capabilities()?;
        let Some(extent) = Self::target_extent(&caps, size) else {
            // The surface became undrawable (a zero extent) since the step looked: not a swap chain.
            return Err((
                "vkCreateSwapchainKHR",
                vk::Result::ERROR_INITIALIZATION_FAILED,
            ));
        };
        let mut count = image_count.max(caps.min_image_count);
        if caps.max_image_count != 0 {
            count = count.min(caps.max_image_count);
        }
        let composite = [
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            vk::CompositeAlphaFlagsKHR::INHERIT,
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
        ]
        .into_iter()
        .find(|flag| caps.supported_composite_alpha.contains(*flag))
        .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE);
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface)
            .min_image_count(count)
            .image_format(self.format.format)
            .image_color_space(self.format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(if self.capture_usage {
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC
            } else {
                vk::ImageUsageFlags::COLOR_ATTACHMENT
            })
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            .composite_alpha(composite)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true)
            .old_swapchain(old);
        // SAFETY: the device and the surface are live; `info` is valid for the call and `old` is null or a
        // swap chain of this surface that the ledger has retired.
        let handle = unsafe { self.swapchain.create_swapchain(&info, None) }
            .map_err(|code| ("vkCreateSwapchainKHR", code))?;
        let mut chain = Chain {
            id,
            handle,
            requested: size,
            extent,
            transform: caps.current_transform,
            images: Vec::new(),
            views: Vec::new(),
            framebuffers: Vec::new(),
        };
        match self.populate(&mut chain) {
            Ok(()) => Ok(chain),
            Err(failure) => {
                self.destroy_chain(&chain);
                Err(failure)
            }
        }
    }

    fn populate(&self, chain: &mut Chain) -> Result<(), Failure> {
        // SAFETY: the swap chain was just created on this device.
        let images = unsafe { self.swapchain.get_swapchain_images(chain.handle) }
            .map_err(|code| ("vkGetSwapchainImagesKHR", code))?;
        let Some(render_pass) = self.render_pass else {
            return Err((
                "vkCreateFramebuffer",
                vk::Result::ERROR_INITIALIZATION_FAILED,
            ));
        };
        chain.images.clone_from(&images);
        for image in images {
            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(self.format.format)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                );
            // SAFETY: the device is live, the image belongs to the new swap chain and `view_info` is valid.
            let view = unsafe { self.device.create_image_view(&view_info, None) }
                .map_err(|code| ("vkCreateImageView", code))?;
            chain.views.push(view);
            let attachments = [view];
            let framebuffer_info = vk::FramebufferCreateInfo::default()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(chain.extent.width)
                .height(chain.extent.height)
                .layers(1);
            // SAFETY: the device is live and `framebuffer_info` is valid for the call.
            let framebuffer = unsafe { self.device.create_framebuffer(&framebuffer_info, None) }
                .map_err(|code| ("vkCreateFramebuffer", code))?;
            chain.framebuffers.push(framebuffer);
        }
        Ok(())
    }

    /// Destroys a swap chain's framebuffers and image views (never used by a pending operation, by the
    /// caller's contract).
    fn destroy_children(&self, chain: &Chain) {
        if let Some(observer) = &self.observer {
            observer.push(Event::ChainChildrenDestroyed(chain.id));
        }
        for framebuffer in &chain.framebuffers {
            // SAFETY: proven unused by the caller's contract; created on this device.
            unsafe { self.device.destroy_framebuffer(*framebuffer, None) };
        }
        for view in &chain.views {
            // SAFETY: proven unused by the caller's contract; created on this device.
            unsafe { self.device.destroy_image_view(*view, None) };
        }
    }

    fn destroy_swapchain_only(&self, chain: &Chain) {
        // SAFETY: proven unused by the caller's contract; created on this device.
        unsafe { self.swapchain.destroy_swapchain(chain.handle, None) };
        if let Some(observer) = &self.observer {
            observer.push(Event::ChainDestroyed(chain.id));
        }
    }

    /// Destroys a swap chain with its framebuffers and views. Only after every operation that used it was
    /// proven finished (or when it was never used).
    pub(crate) fn destroy_chain(&self, chain: &Chain) {
        self.destroy_children(chain);
        self.destroy_swapchain_only(chain);
    }

    /// `vkAcquireNextImageKHR` with a finite timeout.
    pub(crate) fn acquire(
        &self,
        chain: &Chain,
        semaphore: vk::Semaphore,
        timeout_ns: u64,
    ) -> Result<(u32, bool), vk::Result> {
        // SAFETY: the swap chain and the semaphore belong to this device; the semaphore has no pending
        // signal (the ledger gave it out free).
        unsafe {
            self.swapchain.acquire_next_image(
                chain.handle,
                timeout_ns,
                semaphore,
                vk::Fence::null(),
            )
        }
    }

    /// Records the clear of `index` to `rgb`, and the quad when there is one: `quad` is its draw parameters and
    /// whether the resident texture's upload is recorded first (before the render pass).
    pub(crate) fn record(
        &self,
        chain: &Chain,
        index: u32,
        rgb: [f32; 3],
        quad: Option<(QuadParams, bool)>,
    ) -> Result<(), Failure> {
        let (Some(cmd), Some(render_pass)) = (self.cmd, self.render_pass) else {
            return Err((
                "vkBeginCommandBuffer",
                vk::Result::ERROR_INITIALIZATION_FAILED,
            ));
        };
        let Some(framebuffer) = chain.framebuffers.get(index as usize).copied() else {
            return Err(("vkCmdBeginRenderPass", vk::Result::ERROR_OUT_OF_DATE_KHR));
        };
        // SAFETY: the command buffer is not in use: the previous frame's submit fence was seen signalled.
        unsafe {
            self.device
                .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())
        }
        .map_err(|code| ("vkResetCommandBuffer", code))?;
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: the command buffer is in the initial state after the reset.
        unsafe { self.device.begin_command_buffer(cmd, &begin) }
            .map_err(|code| ("vkBeginCommandBuffer", code))?;
        if let Some((_, true)) = quad {
            self.record_quad_upload(cmd);
        }
        let clear = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [rgb[0], rgb[1], rgb[2], 1.0],
            },
        }];
        let pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: chain.extent,
            })
            .clear_values(&clear);
        // SAFETY: the command buffer is recording; the render pass and framebuffer are compatible.
        unsafe {
            self.device
                .cmd_begin_render_pass(cmd, &pass_info, vk::SubpassContents::INLINE);
        }
        if let Some((params, _)) = quad {
            self.record_quad_draw(cmd, &params, (chain.extent.width, chain.extent.height));
        }
        // SAFETY: the render pass was begun on this command buffer.
        unsafe { self.device.cmd_end_render_pass(cmd) };
        if self.captures(chain.extent)
            && let Some(image) = chain.images.get(index as usize).copied()
        {
            self.record_capture(cmd, image);
        }
        // SAFETY: the command buffer is recording.
        unsafe { self.device.end_command_buffer(cmd) }.map_err(|code| ("vkEndCommandBuffer", code))
    }

    /// Records a scene frame into the command buffer: the optional held-frame wait, the clear, and the batches `draw`
    /// names (none: the clear alone). The drawing is [`record_scene`](crate::scene_draw::record_scene)'s, the same as the
    /// off-screen readback records.
    pub(crate) fn record_scene_frame(
        &self,
        chain: &Chain,
        index: u32,
        rgb: [f32; 3],
        quad: Option<(QuadParams, bool)>,
        draw: Option<&crate::scene_draw::SceneDraw<'_>>,
        extras: FrameExtras,
    ) -> Result<(), Failure> {
        let FrameExtras { hold, stamps } = extras;
        let (Some(cmd), Some(render_pass)) = (self.cmd, self.render_pass) else {
            return Err((
                "vkBeginCommandBuffer",
                vk::Result::ERROR_INITIALIZATION_FAILED,
            ));
        };
        let Some(framebuffer) = chain.framebuffers.get(index as usize).copied() else {
            return Err(("vkCmdBeginRenderPass", vk::Result::ERROR_OUT_OF_DATE_KHR));
        };
        // SAFETY: the command buffer is not in use: the previous frame's proof is recorded (the thread checks it before
        // it builds a frame).
        unsafe {
            self.device
                .reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())
        }
        .map_err(|code| ("vkResetCommandBuffer", code))?;
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: the command buffer is in the initial state after the reset.
        unsafe { self.device.begin_command_buffer(cmd, &begin) }
            .map_err(|code| ("vkBeginCommandBuffer", code))?;
        // The frame's two timestamps (when the device has valid bits): reset, then the start at the top of the pipe.
        let stamped = self.timestamps.zip(stamps);
        if let Some((pool, first)) = stamped {
            // SAFETY: the command buffer is recording outside a render pass; the two queries are not in use: the previous
            // frame's proof is recorded and its results were read before this frame was built.
            unsafe { self.device.cmd_reset_query_pool(cmd, pool, first, 2) };
            // SAFETY: as above; the queries were just reset.
            unsafe {
                self.device.cmd_write_timestamp(
                    cmd,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    pool,
                    first,
                );
            }
        }
        if let Some(event) = hold {
            // SAFETY: the command buffer is recording outside a render pass; the event belongs to this device.
            unsafe {
                self.device.cmd_wait_events(
                    cmd,
                    &[event],
                    vk::PipelineStageFlags::HOST,
                    vk::PipelineStageFlags::VERTEX_SHADER
                        | vk::PipelineStageFlags::FRAGMENT_SHADER
                        | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                    &[],
                    &[],
                    &[],
                );
            }
        }
        if let Some((_, true)) = quad {
            self.record_quad_upload(cmd);
        }
        let clear = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [rgb[0], rgb[1], rgb[2], 1.0],
            },
        }];
        let pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(render_pass)
            .framebuffer(framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: chain.extent,
            })
            .clear_values(&clear);
        // SAFETY: the command buffer is recording; the render pass and framebuffer are compatible.
        unsafe {
            self.device
                .cmd_begin_render_pass(cmd, &pass_info, vk::SubpassContents::INLINE);
        }
        // The legacy quad first (below every scene quad), by today's path.
        if let Some((params, _)) = quad {
            self.record_quad_draw(cmd, &params, (chain.extent.width, chain.extent.height));
        }
        if let Some(draw) = draw {
            crate::scene_draw::record_scene(
                &self.device,
                cmd,
                draw,
                (chain.extent.width, chain.extent.height),
            );
        }
        // SAFETY: the render pass was begun on this command buffer.
        unsafe { self.device.cmd_end_render_pass(cmd) };
        if self.captures(chain.extent)
            && let Some(image) = chain.images.get(index as usize).copied()
        {
            self.record_capture(cmd, image);
        }
        if let Some((pool, first)) = stamped {
            // SAFETY: the command buffer is recording outside a render pass; the query was reset above.
            unsafe {
                self.device.cmd_write_timestamp(
                    cmd,
                    vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                    pool,
                    first + 1,
                );
            }
        }
        // SAFETY: the command buffer is recording.
        unsafe { self.device.end_command_buffer(cmd) }.map_err(|code| ("vkEndCommandBuffer", code))
    }

    /// Creates the timestamp query pool (two queries for each of two frames) when the queue family has valid timestamp bits.
    pub(crate) fn create_timestamps(&mut self, valid_bits: u32) -> Result<(), Failure> {
        if valid_bits == 0 || self.timestamps.is_some() {
            return Ok(());
        }
        let info = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(4);
        // SAFETY: the device is live and `info` is valid for the call.
        let pool = unsafe { self.device.create_query_pool(&info, None) }
            .map_err(|code| ("vkCreateQueryPool", code))?;
        self.timestamps = Some(pool);
        Ok(())
    }

    /// Whether frames record timestamps.
    pub(crate) fn has_timestamps(&self) -> bool {
        self.timestamps.is_some()
    }

    /// The two timestamps of the frame whose queries start at `first`, 64-bit and without waiting: `None` while they are not
    /// available (the caller reads them only after the frame's recorded proof).
    pub(crate) fn timestamp_results(&self, first: u32) -> Option<[u64; 2]> {
        let pool = self.timestamps?;
        let mut data = [0_u64; 2];
        // SAFETY: the pool belongs to this device, the two queries were written by a submitted frame, and `data` holds two
        // 64-bit results.
        let result = unsafe {
            self.device.get_query_pool_results(
                pool,
                first,
                &mut data,
                vk::QueryResultFlags::TYPE_64,
            )
        };
        result.ok().map(|()| data)
    }

    /// One bounded batch of `vkGetPastPresentationTimingGOOGLE` into `out`: how many records it wrote, and whether more remain
    /// (`VK_INCOMPLETE`). Never waits.
    pub(crate) fn past_presentation_timings(
        &self,
        chain: &Chain,
        out: &mut [vk::PastPresentationTimingGOOGLE],
    ) -> Result<(usize, bool), vk::Result> {
        let Some(display) = &self.display else {
            return Ok((0, false));
        };
        let mut count = u32::try_from(out.len()).unwrap_or(u32::MAX);
        // SAFETY: the device and the swap chain are live, `out` holds `count` records, and the extension's functions were loaded
        // only when it was enabled.
        let result = unsafe {
            (display.fp().get_past_presentation_timing_google)(
                display.device(),
                chain.handle,
                std::ptr::addr_of_mut!(count),
                out.as_mut_ptr(),
            )
        };
        let written = usize::try_from(count).unwrap_or(0).min(out.len());
        match result {
            vk::Result::SUCCESS => Ok((written, false)),
            vk::Result::INCOMPLETE => Ok((written, true)),
            other => Err(other),
        }
    }

    /// Submits the frame's commands: waits on `acquire`, signals `render_finished`, with `fence`.
    pub(crate) fn submit(
        &self,
        acquire: vk::Semaphore,
        render_finished: vk::Semaphore,
        fence: vk::Fence,
    ) -> Result<(), vk::Result> {
        let Some(cmd) = self.cmd else {
            return Err(vk::Result::ERROR_INITIALIZATION_FAILED);
        };
        let waits = [acquire];
        let stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let commands = [cmd];
        let signals = [render_finished];
        let info = vk::SubmitInfo::default()
            .wait_semaphores(&waits)
            .wait_dst_stage_mask(&stages)
            .command_buffers(&commands)
            .signal_semaphores(&signals);
        // SAFETY: everything belongs to this device; the fence is unsignalled and not in use; the
        // semaphores are in the states the ledger tracks (acquire pending a signal, render free).
        unsafe { self.device.queue_submit(self.queue, &[info], fence) }
    }

    /// An empty submit that waits on `semaphore` to consume its pending signal, with `fence`.
    pub(crate) fn drain(
        &self,
        semaphore: vk::Semaphore,
        fence: vk::Fence,
    ) -> Result<(), vk::Result> {
        let waits = [semaphore];
        let stages = [vk::PipelineStageFlags::ALL_COMMANDS];
        let info = vk::SubmitInfo::default()
            .wait_semaphores(&waits)
            .wait_dst_stage_mask(&stages);
        // SAFETY: the semaphore has a pending signal (the ledger says so) and the fence is unused.
        unsafe { self.device.queue_submit(self.queue, &[info], fence) }
    }

    /// `vkQueuePresentKHR` with a present fence. Returns whether the result was suboptimal.
    pub(crate) fn present(
        &self,
        chain: &Chain,
        index: u32,
        render_finished: vk::Semaphore,
        present_fence: vk::Fence,
    ) -> Result<bool, vk::Result> {
        let waits = [render_finished];
        let swapchains = [chain.handle];
        let indices = [index];
        let fences = [present_fence];
        let mut fence_info = vk::SwapchainPresentFenceInfoEXT::default().fences(&fences);
        let info = vk::PresentInfoKHR::default()
            .wait_semaphores(&waits)
            .swapchains(&swapchains)
            .image_indices(&indices)
            .push_next(&mut fence_info);
        // SAFETY: the swap chain, image index, semaphore and fence are in the states the ledger tracks.
        unsafe { self.swapchain.queue_present(self.queue, &info) }
    }

    /// [`present`](Self::present) through the render queue: the same call, with the queue's exclusive access.
    pub(crate) fn present_on(
        &self,
        queue: &mut crate::context::RenderQueue,
        chain: &Chain,
        index: u32,
        render_finished: vk::Semaphore,
        present_fence: vk::Fence,
        present_id: Option<u32>,
    ) -> Result<bool, vk::Result> {
        let waits = [render_finished];
        let swapchains = [chain.handle];
        let indices = [index];
        let fences = [present_fence];
        let mut fence_info = vk::SwapchainPresentFenceInfoEXT::default().fences(&fences);
        // The frame's number as the presentation timing feedback's `presentID`, when the extension is enabled.
        let times = [vk::PresentTimeGOOGLE::default()
            .present_id(present_id.unwrap_or(0))
            .desired_present_time(0)];
        let mut times_info = vk::PresentTimesInfoGOOGLE::default().times(&times);
        let mut info = vk::PresentInfoKHR::default()
            .wait_semaphores(&waits)
            .swapchains(&swapchains)
            .image_indices(&indices)
            .push_next(&mut fence_info);
        if present_id.is_some() && self.display.is_some() {
            info = info.push_next(&mut times_info);
        }
        queue.present(&self.swapchain, &info)
    }

    /// `vkReleaseSwapchainImagesEXT` for one acquired image that will not be presented.
    pub(crate) fn release_image(&self, chain: &Chain, index: u32) -> Result<(), vk::Result> {
        let indices = [index];
        let info = vk::ReleaseSwapchainImagesInfoEXT::default()
            .swapchain(chain.handle)
            .image_indices(&indices);
        // SAFETY: the image was acquired from this swap chain and not presented.
        unsafe { self.maintenance.release_swapchain_images(&info) }
    }

    /// Whether `fence` is signalled (never waits).
    pub(crate) fn fence_status(&self, fence: vk::Fence) -> Result<bool, vk::Result> {
        // SAFETY: the fence belongs to this device.
        unsafe { self.device.get_fence_status(fence) }
    }

    /// Resets a fence that was seen signalled.
    pub(crate) fn reset_fence(&self, fence: vk::Fence) -> Result<(), vk::Result> {
        // SAFETY: the fence belongs to this device and is signalled; no operation uses it.
        unsafe { self.device.reset_fences(&[fence]) }
    }

    /// Waits at most `timeout_ns` for any of `fences` (`vkWaitForFences`, never `vkDeviceWaitIdle`).
    pub(crate) fn wait_any(&self, fences: &[vk::Fence], timeout_ns: u64) -> Result<(), vk::Result> {
        if fences.is_empty() {
            return Ok(());
        }
        // SAFETY: the fences belong to this device.
        unsafe { self.device.wait_for_fences(fences, false, timeout_ns) }
    }

    fn note(&self, class: &'static str) {
        if let Some(observer) = &self.observer {
            observer.push(Event::Destroyed(class));
        }
    }

    /// Destroys everything this thread created, in the order of design §7. Only after the ledger proved
    /// every operation finished, or when nothing was ever submitted (a partial setup).
    pub(crate) fn destroy_all(&mut self) {
        if let Some(pool) = self.pool.take() {
            // SAFETY: proven unused; destroying the pool frees its command buffer.
            unsafe { self.device.destroy_command_pool(pool, None) };
            self.cmd = None;
        }
        if let Some(pool) = self.timestamps.take() {
            // SAFETY: proven unused: every frame that wrote it is proven.
            unsafe { self.device.destroy_query_pool(pool, None) };
        }
        self.note("command buffers and pool");
        for fence in std::mem::take(&mut self.fences) {
            // SAFETY: proven unused (every fence was seen signalled or never submitted).
            unsafe { self.device.destroy_fence(fence, None) };
        }
        for semaphore in std::mem::take(&mut self.semaphores) {
            // SAFETY: proven unused (every pending signal and wait is consumed).
            unsafe { self.device.destroy_semaphore(semaphore, None) };
        }
        self.note("fences and semaphores");
        // After everything that used them (the command buffer, the fences), before the render pass the pipeline
        // was made against and the device. Always visited: whatever a partial creation left (a sampler alone
        // included) is destroyed, and the class is noted only when something existed.
        let quad_present = self.quad.any();
        self.destroy_quad();
        if quad_present {
            self.note("quad objects");
        }
        self.destroy_capture();
        let retired = self.retired.take();
        let current = self.current.take();
        for chain in retired.iter().chain(current.iter()) {
            self.destroy_children(chain);
        }
        self.note("image views and framebuffers");
        if let Some(pass) = self.render_pass.take() {
            // SAFETY: no framebuffer or command buffer uses it any more.
            unsafe { self.device.destroy_render_pass(pass, None) };
        }
        self.note("render pass");
        for chain in retired.iter().chain(current.iter()) {
            self.destroy_swapchain_only(chain);
        }
        self.note("swap chains");
        if self.owns_device {
            // SAFETY: every object created on the device is destroyed.
            unsafe { self.device.destroy_device(None) };
            self.note("device");
        }
    }
}

impl Gpu {
    /// Drops this thread's holds on the instance and the loader. Only at a proven end, after
    /// [`Gpu::destroy_all`]; a retained or unwound thread never calls it.
    pub(crate) fn release_holders(&mut self) {
        // SAFETY: called once, at the proven end, and the fields are not used afterwards.
        unsafe { ManuallyDrop::drop(&mut self.inner) };
        // SAFETY: as above.
        unsafe { ManuallyDrop::drop(&mut self.loader) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(current: (u32, u32), min: (u32, u32), max: (u32, u32)) -> vk::SurfaceCapabilitiesKHR {
        vk::SurfaceCapabilitiesKHR {
            current_extent: vk::Extent2D {
                width: current.0,
                height: current.1,
            },
            min_image_extent: vk::Extent2D {
                width: min.0,
                height: min.1,
            },
            max_image_extent: vk::Extent2D {
                width: max.0,
                height: max.1,
            },
            ..vk::SurfaceCapabilitiesKHR::default()
        }
    }

    fn extent(width: u32, height: u32) -> vk::Extent2D {
        vk::Extent2D { width, height }
    }

    #[test]
    fn a_defined_current_extent_is_used_and_clamped_to_the_limits() {
        let limits = ((1, 1), (4096, 4096));
        assert_eq!(
            Gpu::target_extent(&caps((800, 600), limits.0, limits.1), (1, 1)),
            Some(extent(800, 600))
        );
        assert_eq!(
            Gpu::target_extent(&caps((100_000, 600), limits.0, limits.1), (1, 1)),
            Some(extent(4096, 600)),
            "the current extent is clamped to the maximum, as the size is"
        );
        assert_eq!(
            Gpu::target_extent(&caps((800, 600), (1024, 1), limits.1), (1, 1)),
            Some(extent(1024, 600)),
            "and to the minimum"
        );
    }

    #[test]
    fn an_undefined_current_extent_takes_the_window_size_clamped() {
        let undefined = (u32::MAX, u32::MAX);
        assert_eq!(
            Gpu::target_extent(&caps(undefined, (1, 1), (4096, 4096)), (640, 480)),
            Some(extent(640, 480))
        );
        assert_eq!(
            Gpu::target_extent(&caps(undefined, (1, 1), (4096, 4096)), (9000, 2)),
            Some(extent(4096, 2))
        );
    }

    #[test]
    fn a_zero_dimension_is_not_lifted_by_the_minimum_limit() {
        assert_eq!(
            Gpu::target_extent(&caps((0, 0), (1, 1), (4096, 4096)), (800, 600)),
            None,
            "a minimised surface (current extent 0 x 0) is not drawable although its minimum is 1 x 1"
        );
        assert_eq!(
            Gpu::target_extent(&caps((u32::MAX, u32::MAX), (1, 1), (4096, 4096)), (0, 600)),
            None
        );
    }

    #[test]
    fn a_zero_dimension_is_not_drawable() {
        assert_eq!(
            Gpu::target_extent(&caps((0, 600), (0, 0), (4096, 4096)), (800, 600)),
            None
        );
        assert_eq!(
            Gpu::target_extent(&caps((800, 0), (0, 0), (4096, 4096)), (800, 600)),
            None
        );
        assert_eq!(
            Gpu::target_extent(&caps((u32::MAX, u32::MAX), (0, 0), (4096, 4096)), (0, 600)),
            None
        );
        assert_eq!(
            Gpu::target_extent(&caps((u32::MAX, u32::MAX), (0, 0), (0, 0)), (800, 600)),
            None,
            "a surface whose limits are zero is not drawable either"
        );
    }
}
