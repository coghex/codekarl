//! Off-screen readback: an image cleared to a known colour on a device from an instance, copied to a
//! host-visible buffer and read back, with no surface or window. Public, used by tests and nothing else.

use std::cell::Cell;
use std::ffi::c_char;
use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use ash::khr;
use ash::vk;

use crate::error::vk_error;
use crate::instance::InstanceInner;
use crate::unwind;
use crate::{Error, Instance, Loader};

/// How long the readback waits for its one submit.
const WAIT_NS: u64 = 5_000_000_000;
/// The image is `SIZE` x `SIZE` texels.
const SIZE: u32 = 4;

/// What the readback created, so an early exit destroys exactly that.
struct Objects<'a> {
    device: &'a ash::Device,
    pool: Option<vk::CommandPool>,
    fence: Option<vk::Fence>,
    image: Option<vk::Image>,
    image_memory: Option<vk::DeviceMemory>,
    buffer: Option<vk::Buffer>,
    buffer_memory: Option<vk::DeviceMemory>,
}

impl Objects<'_> {
    /// Destroys what exists. Only after the submit was proven finished, or when nothing was submitted.
    fn destroy(&mut self) {
        if let Some(pool) = self.pool.take() {
            // SAFETY: nothing submitted uses the pool any more (proven, or never submitted).
            unsafe { self.device.destroy_command_pool(pool, None) };
        }
        if let Some(fence) = self.fence.take() {
            // SAFETY: the fence was signalled or never submitted.
            unsafe { self.device.destroy_fence(fence, None) };
        }
        if let Some(buffer) = self.buffer.take() {
            // SAFETY: nothing uses the buffer any more.
            unsafe { self.device.destroy_buffer(buffer, None) };
        }
        if let Some(memory) = self.buffer_memory.take() {
            // SAFETY: the buffer bound to it is destroyed.
            unsafe { self.device.free_memory(memory, None) };
        }
        if let Some(image) = self.image.take() {
            // SAFETY: nothing uses the image any more.
            unsafe { self.device.destroy_image(image, None) };
        }
        if let Some(memory) = self.image_memory.take() {
            // SAFETY: the image bound to it is destroyed.
            unsafe { self.device.free_memory(memory, None) };
        }
    }
}

pub(crate) fn memory_type(
    properties: &vk::PhysicalDeviceMemoryProperties,
    bits: u32,
    wanted: vk::MemoryPropertyFlags,
) -> Option<u32> {
    (0..properties.memory_type_count).find(|index| {
        bits & (1 << index) != 0
            && properties.memory_types[*index as usize]
                .property_flags
                .contains(wanted)
    })
}

/// Faults a test injects into the readback, through [`readback_clear_with_test_hooks`] only.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadbackHooks {
    /// The result of the submit (the real submit is not made). Out of host or device memory queued nothing;
    /// anything else (a lost device) leaves the completion unproven.
    pub submit: Option<vk::Result>,
    /// The result of the fence wait, after a real submit (the real wait is not made). Any error leaves the
    /// completion unproven.
    pub wait: Option<vk::Result>,
    /// Panic right after the submit: an unwind with the completion unproven.
    pub panic_after_submit: bool,
    /// Panic before the submit: an unwind with nothing in flight.
    pub panic_before_submit: bool,
    /// What the forced panics (`panic_after_submit`, `panic_before_submit`) raise, in place of a plain message:
    /// a function that panics with any payload, such as one whose `Drop` panics.
    pub forced_panic_payload: Option<fn() -> !>,
    /// The `maxImageDimension2D` the quad's device checks see, in place of the device's own
    /// ([`readback_quad_with_test_hooks`](crate::readback_quad_with_test_hooks) only).
    pub max_image_dimension: Option<u32>,
    /// Counts every quad object whose creation begins (a partial creation counts)
    /// ([`readback_quad_with_test_hooks`](crate::readback_quad_with_test_hooks) only).
    pub quad_objects: Option<&'static std::sync::atomic::AtomicUsize>,
    /// The `maxFramebufferWidth` and `maxFramebufferHeight` the readback target's device checks see, in place of
    /// the device's own ([`readback_quad_with_test_hooks`](crate::readback_quad_with_test_hooks) only).
    pub max_framebuffer_dimension: Option<u32>,
    /// Counts every readback target object whose creation begins: the render pass, the target image, its view and
    /// the framebuffer ([`readback_quad_with_test_hooks`](crate::readback_quad_with_test_hooks) only).
    pub target_objects: Option<&'static std::sync::atomic::AtomicUsize>,
    /// The `R8G8B8A8_UNORM` optimal-tiling features the quad's device checks see, in place of the device's own
    /// ([`readback_quad_with_test_hooks`](crate::readback_quad_with_test_hooks) only).
    pub source_features: Option<vk::FormatFeatureFlags>,
    /// The `B8G8R8A8_UNORM` optimal-tiling features the target's device checks see, in place of the device's own
    /// ([`readback_quad_with_test_hooks`](crate::readback_quad_with_test_hooks) only).
    pub target_features: Option<vk::FormatFeatureFlags>,
}

/// How a readback ended.
enum Outcome {
    /// The pixel, with the submit proven finished.
    Done([u8; 4]),
    /// An error with nothing in flight: everything may be destroyed.
    Clean(Error),
    /// An error with the completion unproven: nothing is destroyed on a guess.
    Unproven(Error),
}

/// Clears an off-screen image to `rgb`, copies it to a host-visible buffer and returns the first texel as
/// `[r, g, b, a]` (alpha is 255).
///
/// **Ownership.** The readback holds an owning reference to the instance (the instance cannot be destroyed
/// while it lives) and its own [`Loader`] clone. A failure before any submit, or a submit that queued
/// nothing (out of host or device memory), cleans up and returns an `Error`. When completion is unproven
/// (the fence wait times out or fails, `VK_ERROR_DEVICE_LOST` from the submit or the wait, or an unwind after
/// the submit) it returns an `Error` naming the unproven state and **keeps** its device, its objects, the
/// instance reference and the loader clone for the rest of the process: no `Drop` or unwind releases them,
/// and [`destroy_surface_and_instance`](crate::destroy_surface_and_instance) does not destroy an instance
/// that a retained readback references.
///
/// # Errors
///
/// An [`Error`] naming the Vulkan call that failed, or that no device could do the work, or that the
/// completion is unproven.
pub fn readback_clear(instance: &Instance, rgb: [f32; 3]) -> Result<[u8; 4], Error> {
    readback_clear_with_test_hooks(instance, rgb, ReadbackHooks::default())
}

/// [`readback_clear`] with faults injected by the tests. Not part of the interface.
///
/// # Errors
///
/// As [`readback_clear`].
#[doc(hidden)]
pub fn readback_clear_with_test_hooks(
    instance: &Instance,
    rgb: [f32; 3],
    hooks: ReadbackHooks,
) -> Result<[u8; 4], Error> {
    // The holds sit in `ManuallyDrop`: only an explicit clean or proven exit releases them, never an unwind.
    let held: ManuallyDrop<(Arc<InstanceInner>, Loader)> =
        ManuallyDrop::new((Arc::clone(&instance.inner), instance.inner.loader.clone()));
    // Counted from the start, and uncounted only by a clean or proven exit: an unwind leaves it counted.
    instance
        .inner
        .retained_readbacks
        .fetch_add(1, Ordering::SeqCst);
    let (outcome, device) = run(&instance.inner, rgb, hooks);
    match outcome {
        Outcome::Done(pixel) => {
            release(instance, held, device);
            Ok(pixel)
        }
        Outcome::Clean(error) => {
            release(instance, held, device);
            Err(error)
        }
        // Everything stays: the device, its objects, the instance reference and the loader clone.
        Outcome::Unproven(error) => Err(error),
    }
}

/// Destroys the device (its objects are destroyed already) and drops the holds, then uncounts the readback.
pub(crate) fn release(
    instance: &Instance,
    mut held: ManuallyDrop<(Arc<InstanceInner>, Loader)>,
    device: Option<ash::Device>,
) {
    if let Some(device) = device {
        // SAFETY: every object created on the device is destroyed and no submit is in flight.
        unsafe { device.destroy_device(None) };
    }
    // SAFETY: the readback is over and nothing uses the holds any more; they are dropped once.
    unsafe { ManuallyDrop::drop(&mut held) };
    instance
        .inner
        .retained_readbacks
        .fetch_sub(1, Ordering::SeqCst);
}

/// A device picked and created for the readback.
pub(crate) struct Opened {
    pub(crate) device: ash::Device,
    pub(crate) physical: vk::PhysicalDevice,
    pub(crate) family: u32,
}

/// Picks a physical device with a graphics queue and creates a device on it (with
/// `VK_KHR_portability_subset` where it is advertised).
pub(crate) fn open_device(vk_instance: &ash::Instance) -> Result<Opened, Error> {
    // SAFETY: the instance is live (the caller holds it) and the call takes no pointer from us.
    let devices = unsafe { vk_instance.enumerate_physical_devices() }
        .map_err(|code| vk_error("vkEnumeratePhysicalDevices", code))?;
    let mut chosen = None;
    for physical in devices {
        // SAFETY: `physical` came from this instance.
        let families = unsafe { vk_instance.get_physical_device_queue_family_properties(physical) };
        if let Some(family) = families
            .iter()
            .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        {
            chosen = Some((physical, u32::try_from(family).unwrap_or(0)));
            break;
        }
    }
    let Some((physical, family)) = chosen else {
        return Err(Error("no Vulkan device has a graphics queue".to_owned()));
    };
    // SAFETY: `physical` came from this instance.
    let advertised = unsafe { vk_instance.enumerate_device_extension_properties(physical) }
        .map_err(|code| vk_error("vkEnumerateDeviceExtensionProperties", code))?;
    let subset = advertised.iter().any(|property| {
        property
            .extension_name_as_c_str()
            .is_ok_and(|name| name == khr::portability_subset::NAME)
    });
    let priorities = [1.0_f32];
    let queue_info = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(family)
        .queue_priorities(&priorities)];
    let mut names: Vec<*const c_char> = Vec::new();
    if subset {
        names.push(khr::portability_subset::NAME.as_ptr());
    }
    let device_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_info)
        .enabled_extension_names(&names);
    // SAFETY: the instance is live; `device_info` and what it points to live for the call.
    let device = unsafe { vk_instance.create_device(physical, &device_info, None) }
        .map_err(|code| vk_error("vkCreateDevice", code))?;
    Ok(Opened {
        device,
        physical,
        family,
    })
}

/// What `read` works with.
struct Job<'a> {
    instance: &'a ash::Instance,
    opened: &'a Opened,
    rgb: [f32; 3],
    hooks: ReadbackHooks,
    /// Set from the moment the submit is attempted.
    submitted: &'a Cell<bool>,
}

/// Picks a device, creates it and runs the work. The device is returned when the caller is to destroy it
/// (after the objects, which this has destroyed), and `None` when it is kept (or never created).
fn run(
    inner: &InstanceInner,
    rgb: [f32; 3],
    hooks: ReadbackHooks,
) -> (Outcome, Option<ash::Device>) {
    let opened = match open_device(&inner.instance) {
        Ok(opened) => opened,
        Err(error) => return (Outcome::Clean(error), None),
    };
    let mut objects = Objects {
        device: &opened.device,
        pool: None,
        fence: None,
        image: None,
        image_memory: None,
        buffer: None,
        buffer_memory: None,
    };
    // An unwind is caught here, at the readback boundary, and becomes an `Error`: after the submit the
    // completion is unproven (the owners are kept), before it nothing is in flight.
    let submitted = Cell::new(false);
    let job = Job {
        instance: &inner.instance,
        opened: &opened,
        rgb,
        hooks,
        submitted: &submitted,
    };
    let unwound = catch_unwind(AssertUnwindSafe(|| read(&job, &mut objects)));
    let outcome = match unwound {
        Ok(outcome) => outcome,
        Err(payload) => {
            // The text first, then the payload is disposed of: its own `Drop` may panic, and that must not
            // unwind out of this boundary.
            let text = unwind::message(&*payload);
            unwind::dispose(payload);
            if submitted.get() {
                Outcome::Unproven(Error(format!(
                    "the readback unwound after its submit ({text}); the completion is unproven, so the \
                     device, its objects, the instance and the loader were kept"
                )))
            } else {
                Outcome::Clean(Error(format!(
                    "the readback unwound before its submit ({text})"
                )))
            }
        }
    };
    if matches!(outcome, Outcome::Unproven(_)) {
        // Nothing is destroyed on a guess: the objects and the device stay (they have no `Drop`).
        return (outcome, None);
    }
    objects.destroy();
    (outcome, Some(opened.device))
}

/// A Vulkan failure: the call and its code.
pub(crate) type Failure = (&'static str, vk::Result);

fn early((call, code): Failure) -> Outcome {
    Outcome::Clean(vk_error(call, code))
}

/// The work: builds the objects, submits and waits.
fn read(job: &Job<'_>, objects: &mut Objects<'_>) -> Outcome {
    let device = &job.opened.device;
    let (rgb, hooks, submitted) = (job.rgb, job.hooks, job.submitted);
    let work = match build(
        job.instance,
        device,
        job.opened.physical,
        job.opened.family,
        objects,
    ) {
        Ok(work) => work,
        Err(failure) => return early(failure),
    };
    if let Err(failure) = record(&work, rgb) {
        return early(failure);
    }
    let commands = [work.cmd];
    let submit = vk::SubmitInfo::default().command_buffers(&commands);
    if hooks.panic_before_submit {
        unwind::forced_panic(
            hooks.forced_panic_payload,
            "a forced panic before the submit",
        );
    }
    // Counted as submitted from here: a submit that unwinds is as unproven as one that returned.
    submitted.set(true);
    let queued = match hooks.submit {
        Some(code) => Err(code),
        // SAFETY: everything belongs to this device; the fence is unsignalled and unused.
        None => unsafe { device.queue_submit(work.queue, &[submit], work.fence) },
    };
    if let Err(code) = queued {
        return match code {
            // Queued nothing: nothing is in flight.
            vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY => {
                early(("vkQueueSubmit", code))
            }
            // A lost device (or anything else the spec gives no queue effect for): unproven.
            other => Outcome::Unproven(Error(format!(
                "vkQueueSubmit failed: {other:?}; the completion is unproven, so the device, its objects, \
                 the instance and the loader were kept"
            ))),
        };
    }
    if hooks.panic_after_submit {
        unwind::forced_panic(
            hooks.forced_panic_payload,
            "a forced panic after the submit",
        );
    }
    let waited = match hooks.wait {
        Some(code) => Err(code),
        // SAFETY: the fence belongs to this device. The wait is bounded; `vkDeviceWaitIdle` is never used.
        None => unsafe { device.wait_for_fences(&[work.fence], true, WAIT_NS) },
    };
    if let Err(code) = waited {
        // In flight and unproven: nothing is destroyed on a guess.
        return Outcome::Unproven(Error(format!(
            "vkWaitForFences failed: {code:?}; the completion is unproven, so the device, its objects, \
             the instance and the loader were kept"
        )));
    }
    // SAFETY: the memory is host visible and coherent; the submit is proven finished.
    let mapped = match unsafe {
        device.map_memory(
            work.buffer_memory,
            0,
            vk::WHOLE_SIZE,
            vk::MemoryMapFlags::empty(),
        )
    } {
        Ok(mapped) => mapped,
        Err(code) => return early(("vkMapMemory", code)),
    };
    // SAFETY: the mapping is at least four bytes, written by the finished copy.
    let pixel = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), 4) };
    let result = [pixel[0], pixel[1], pixel[2], pixel[3]];
    // SAFETY: the memory is mapped by this function.
    unsafe { device.unmap_memory(work.buffer_memory) };
    Outcome::Done(result)
}

/// The device-level state the readback works with.
struct Work<'a> {
    device: &'a ash::Device,
    cmd: vk::CommandBuffer,
    queue: vk::Queue,
    image: vk::Image,
    buffer: vk::Buffer,
    buffer_memory: vk::DeviceMemory,
    fence: vk::Fence,
}

/// Allocates memory for a resource with `requirements`, preferring `wanted` properties.
pub(crate) fn allocate(
    device: &ash::Device,
    properties: &vk::PhysicalDeviceMemoryProperties,
    requirements: vk::MemoryRequirements,
    wanted: vk::MemoryPropertyFlags,
    any_will_do: bool,
) -> Result<vk::DeviceMemory, Failure> {
    let kind = memory_type(properties, requirements.memory_type_bits, wanted)
        .or_else(|| {
            if any_will_do {
                memory_type(
                    properties,
                    requirements.memory_type_bits,
                    vk::MemoryPropertyFlags::empty(),
                )
            } else {
                None
            }
        })
        .ok_or(("vkAllocateMemory", vk::Result::ERROR_FEATURE_NOT_PRESENT))?;
    let allocation = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(kind);
    // SAFETY: the device is live and `allocation` is valid for the call.
    unsafe { device.allocate_memory(&allocation, None) }.map_err(|code| ("vkAllocateMemory", code))
}

/// Creates the command pool and buffer, the image, the host-visible buffer and the fence, recording each
/// in `objects` as it exists.
fn build<'a>(
    instance: &ash::Instance,
    device: &'a ash::Device,
    physical: vk::PhysicalDevice,
    family: u32,
    objects: &mut Objects<'_>,
) -> Result<Work<'a>, Failure> {
    // SAFETY: `physical` came from this instance.
    let memory_properties = unsafe { instance.get_physical_device_memory_properties(physical) };
    // SAFETY: the family was requested in the device create info.
    let queue = unsafe { device.get_device_queue(family, 0) };
    let pool_info = vk::CommandPoolCreateInfo::default().queue_family_index(family);
    // SAFETY: the device is live and `pool_info` is valid for the call.
    let pool = unsafe { device.create_command_pool(&pool_info, None) }
        .map_err(|code| ("vkCreateCommandPool", code))?;
    objects.pool = Some(pool);
    let allocate_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: the pool is live and `allocate_info` is valid for the call.
    let cmd = unsafe { device.allocate_command_buffers(&allocate_info) }
        .map_err(|code| ("vkAllocateCommandBuffers", code))?[0];
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::R8G8B8A8_UNORM)
        .extent(vk::Extent3D {
            width: SIZE,
            height: SIZE,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    // SAFETY: the device is live and `image_info` is valid for the call.
    let image = unsafe { device.create_image(&image_info, None) }
        .map_err(|code| ("vkCreateImage", code))?;
    objects.image = Some(image);
    // SAFETY: the image belongs to the device.
    let image_requirements = unsafe { device.get_image_memory_requirements(image) };
    let image_memory = allocate(
        device,
        &memory_properties,
        image_requirements,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
        true,
    )?;
    objects.image_memory = Some(image_memory);
    // SAFETY: the memory is fresh and large enough for the image.
    unsafe { device.bind_image_memory(image, image_memory, 0) }
        .map_err(|code| ("vkBindImageMemory", code))?;
    let buffer_info = vk::BufferCreateInfo::default()
        .size(u64::from(SIZE * SIZE * 4))
        .usage(vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: the device is live and `buffer_info` is valid for the call.
    let buffer = unsafe { device.create_buffer(&buffer_info, None) }
        .map_err(|code| ("vkCreateBuffer", code))?;
    objects.buffer = Some(buffer);
    // SAFETY: the buffer belongs to the device.
    let buffer_requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    let buffer_memory = allocate(
        device,
        &memory_properties,
        buffer_requirements,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        false,
    )?;
    objects.buffer_memory = Some(buffer_memory);
    // SAFETY: the memory is fresh and large enough for the buffer.
    unsafe { device.bind_buffer_memory(buffer, buffer_memory, 0) }
        .map_err(|code| ("vkBindBufferMemory", code))?;
    let fence_info = vk::FenceCreateInfo::default();
    // SAFETY: the device is live and `fence_info` is valid for the call.
    let fence = unsafe { device.create_fence(&fence_info, None) }
        .map_err(|code| ("vkCreateFence", code))?;
    objects.fence = Some(fence);
    Ok(Work {
        device,
        cmd,
        queue,
        image,
        buffer,
        buffer_memory,
        fence,
    })
}

fn barrier(
    image: vk::Image,
    from: vk::ImageLayout,
    to: vk::ImageLayout,
) -> vk::ImageMemoryBarrier<'static> {
    let (source, destination) = if from == vk::ImageLayout::UNDEFINED {
        (vk::AccessFlags::empty(), vk::AccessFlags::TRANSFER_WRITE)
    } else {
        (
            vk::AccessFlags::TRANSFER_WRITE,
            vk::AccessFlags::TRANSFER_READ,
        )
    };
    vk::ImageMemoryBarrier::default()
        .src_access_mask(source)
        .dst_access_mask(destination)
        .old_layout(from)
        .new_layout(to)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1),
        )
}

/// Records: transition, clear, transition, copy to the buffer, make the copy visible to the host.
fn record(work: &Work<'_>, rgb: [f32; 3]) -> Result<(), Failure> {
    let device = work.device;
    let cmd = work.cmd;
    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1);
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the command buffer is in the initial state.
    unsafe { device.begin_command_buffer(cmd, &begin) }
        .map_err(|code| ("vkBeginCommandBuffer", code))?;
    let to_destination = barrier(
        work.image,
        vk::ImageLayout::UNDEFINED,
        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
    );
    // SAFETY: the command buffer is recording; the barrier names this device's image.
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_destination],
        );
    }
    let clear = vk::ClearColorValue {
        float32: [rgb[0], rgb[1], rgb[2], 1.0],
    };
    // SAFETY: the command buffer is recording and the image is in `TRANSFER_DST_OPTIMAL`.
    unsafe {
        device.cmd_clear_color_image(
            cmd,
            work.image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &clear,
            &[range],
        );
    }
    let to_source = barrier(
        work.image,
        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
    );
    // SAFETY: the command buffer is recording; the barrier names this device's image.
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_source],
        );
    }
    let region = vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width: SIZE,
            height: SIZE,
            depth: 1,
        });
    // SAFETY: the command buffer is recording; the image is in `TRANSFER_SRC_OPTIMAL`.
    unsafe {
        device.cmd_copy_image_to_buffer(
            cmd,
            work.image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            work.buffer,
            &[region],
        );
    }
    let to_host = vk::BufferMemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .dst_access_mask(vk::AccessFlags::HOST_READ)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .buffer(work.buffer)
        .offset(0)
        .size(vk::WHOLE_SIZE);
    // SAFETY: the command buffer is recording; the barrier names this device's buffer.
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::HOST,
            vk::DependencyFlags::empty(),
            &[],
            &[to_host],
            &[],
        );
    }
    // SAFETY: the command buffer is recording.
    unsafe { device.end_command_buffer(cmd) }.map_err(|code| ("vkEndCommandBuffer", code))
}
