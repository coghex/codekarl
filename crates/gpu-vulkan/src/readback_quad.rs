//! Off-screen readback with a quad: the clear, then the quad if any, through the shared drawing code, on a
//! `B8G8R8A8_UNORM` target, copied to a host-visible buffer and returned. Public, used by tests and nothing else.
//!
//! It keeps [`readback_clear`](crate::readback_clear)'s ownership rules exactly: it holds an owning reference to
//! the instance and its own loader clone in `ManuallyDrop` (only an explicit clean or proven exit releases them,
//! never an unwind); a failure before any submit, or a submit that queued nothing, destroys what exists and
//! returns an error; an unproven completion (the fence wait times out or fails, a lost device, an unwind after the
//! submit) returns an error naming it and **keeps** the device, every object (the pipeline, layouts, pool,
//! samplers, texture, staging buffer, target and the rest), the instance reference and the loader clone for the
//! rest of the process.

use std::cell::Cell;
use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use ash::vk;

use crate::draw::{
    QuadParams, QuadPipeline, QuadTexture, Tally, device_checks, record_draw, upload_bytes,
};
use crate::error::vk_error;
use crate::instance::InstanceInner;
use crate::quad::{MAX_IMAGE_BYTES, VulkanQuad};
use crate::readback::{Failure, Opened, ReadbackHooks, allocate, open_device, release};
use crate::unwind;
use crate::{Error, Instance, Loader};

/// The largest width or height of the readback target, in pixels. Output is at most 64 MiB (4096 × 4096 × 4).
pub const MAX_TARGET_SIDE: u32 = 4096;

/// How long the readback waits for its one submit.
const WAIT_NS: u64 = 5_000_000_000;
/// The target's format: one of the two the quad drawing supports (design §7.3).
const TARGET_FORMAT: vk::Format = vk::Format::B8G8R8A8_UNORM;

/// What the readback created, so an early exit destroys exactly that. Fields are `None` until created.
struct Objects<'a> {
    device: &'a ash::Device,
    pool: Option<vk::CommandPool>,
    fence: Option<vk::Fence>,
    render_pass: Option<vk::RenderPass>,
    target: Option<vk::Image>,
    target_memory: Option<vk::DeviceMemory>,
    target_view: Option<vk::ImageView>,
    framebuffer: Option<vk::Framebuffer>,
    buffer: Option<vk::Buffer>,
    buffer_memory: Option<vk::DeviceMemory>,
    pipeline: QuadPipeline,
    texture: QuadTexture,
}

impl Objects<'_> {
    /// Destroys what exists, in reverse order of creation. Only after the submit was proven finished, or when
    /// nothing was submitted.
    fn destroy(&mut self) {
        let device = self.device;
        if let Some(pool) = self.pool.take() {
            // SAFETY: nothing submitted uses the pool (or its command buffer) any more.
            unsafe { device.destroy_command_pool(pool, None) };
        }
        if let Some(fence) = self.fence.take() {
            // SAFETY: the fence was signalled or never submitted.
            unsafe { device.destroy_fence(fence, None) };
        }
        if let Some(framebuffer) = self.framebuffer.take() {
            // SAFETY: nothing submitted uses the framebuffer any more.
            unsafe { device.destroy_framebuffer(framebuffer, None) };
        }
        if let Some(buffer) = self.buffer.take() {
            // SAFETY: nothing uses the buffer any more.
            unsafe { device.destroy_buffer(buffer, None) };
        }
        if let Some(memory) = self.buffer_memory.take() {
            // SAFETY: the buffer bound to it is destroyed.
            unsafe { device.free_memory(memory, None) };
        }
        let descriptor_pool = self.pipeline.pool;
        self.texture.destroy(device, descriptor_pool);
        self.pipeline.destroy(device);
        if let Some(view) = self.target_view.take() {
            // SAFETY: nothing uses the view any more.
            unsafe { device.destroy_image_view(view, None) };
        }
        if let Some(image) = self.target.take() {
            // SAFETY: nothing uses the image any more.
            unsafe { device.destroy_image(image, None) };
        }
        if let Some(memory) = self.target_memory.take() {
            // SAFETY: the image bound to it is destroyed.
            unsafe { device.free_memory(memory, None) };
        }
        if let Some(render_pass) = self.render_pass.take() {
            // SAFETY: the pipeline and framebuffer that used the pass are destroyed.
            unsafe { device.destroy_render_pass(render_pass, None) };
        }
    }
}

/// How a readback ended.
enum Outcome {
    /// The bytes, with the submit proven finished.
    Done(Vec<u8>),
    /// An error with nothing in flight: everything may be destroyed.
    Clean(Error),
    /// An error with the completion unproven: nothing is destroyed on a guess.
    Unproven(Error),
}

/// Renders a `width` × `height` `B8G8R8A8_UNORM` target off screen: the clear `clear` (alpha one), then `quad`
/// if any, through the shared drawing code, and returns the target's bytes (BGRA, rows top first, exactly
/// `width × height × 4`).
///
/// **Ownership.** As [`readback_clear`](crate::readback_clear): an owning reference to the instance and its own
/// loader clone. A failure before any submit, or a submit that queued nothing, cleans up and returns an `Error`.
/// When the completion is unproven it returns an `Error` naming that and **keeps** the device, every object, the
/// instance reference and the loader clone for the rest of the process.
///
/// # Errors
///
/// An [`Error`] naming the Vulkan call that failed, the device check the quad failed (the image's size and the
/// device's limit, or the missing capability), an invalid target size, or that the completion is unproven.
pub fn readback_quad(
    instance: &Instance,
    width: u32,
    height: u32,
    clear: [f32; 3],
    quad: Option<&VulkanQuad>,
) -> Result<Vec<u8>, Error> {
    readback_quad_with_test_hooks(
        instance,
        width,
        height,
        clear,
        quad,
        ReadbackHooks::default(),
    )
}

/// [`readback_quad`] with faults injected by the tests. Not part of the interface.
///
/// # Errors
///
/// As [`readback_quad`].
#[doc(hidden)]
pub fn readback_quad_with_test_hooks(
    instance: &Instance,
    width: u32,
    height: u32,
    clear: [f32; 3],
    quad: Option<&VulkanQuad>,
    hooks: ReadbackHooks,
) -> Result<Vec<u8>, Error> {
    // Before any Vulkan call or large allocation: the target's own bounds (not the quad's source limits).
    let length = output_length(width, height, MAX_TARGET_SIDE, MAX_IMAGE_BYTES)?;
    // The holds sit in `ManuallyDrop`: only an explicit clean or proven exit releases them, never an unwind.
    let held: ManuallyDrop<(Arc<InstanceInner>, Loader)> =
        ManuallyDrop::new((Arc::clone(&instance.inner), instance.inner.loader.clone()));
    // Counted from the start, and uncounted only by a clean or proven exit: an unwind leaves it counted.
    instance
        .inner
        .retained_readbacks
        .fetch_add(1, Ordering::SeqCst);
    let job = Job {
        target: (width, height),
        length,
        clear,
        quad,
        hooks,
    };
    let (outcome, device) = run(&instance.inner, &job);
    match outcome {
        Outcome::Done(bytes) => {
            release(instance, held, device);
            Ok(bytes)
        }
        Outcome::Clean(error) => {
            release(instance, held, device);
            Err(error)
        }
        // Everything stays: the device, its objects, the instance reference and the loader clone.
        Outcome::Unproven(error) => Err(error),
    }
}

/// The bytes of a `width` × `height` BGRA output, or the refusal: each side from 1 to `max_side` and the output
/// (`width × height × 4`, in checked arithmetic with fallible conversions) at most `max_bytes`. The refusal names
/// the numbers. No Vulkan call is involved.
fn output_length(width: u32, height: u32, max_side: u32, max_bytes: u64) -> Result<usize, Error> {
    if !(1..=max_side).contains(&width) || !(1..=max_side).contains(&height) {
        return Err(Error(format!(
            "the readback target is {width} x {height} pixels; each side must be from 1 to {max_side}"
        )));
    }
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .filter(|bytes| *bytes <= max_bytes)
        .ok_or_else(|| {
            Error(format!(
                "the readback output of {width} x {height} pixels is over the {max_bytes} bytes it may take"
            ))
        })?;
    usize::try_from(bytes).map_err(|_| {
        Error(format!(
            "the readback output of {bytes} bytes does not fit this platform's address space"
        ))
    })
}

/// The device's checks for the target (design §7.2.1, r2), run after the device is selected and **before the render
/// pass, the target image, its view or the framebuffer is created**: the target fits `maxImageDimension2D`,
/// `maxFramebufferWidth` and `maxFramebufferHeight` (or the limits a test hook lowers them to), and
/// `B8G8R8A8_UNORM` supports colour attachment with blending and transfer source with optimal tiling. A refusal
/// names the limit or the missing capability. With a quad or without.
fn target_checks(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    (width, height): (u32, u32),
    hooks: ReadbackHooks,
) -> Result<(), Error> {
    // SAFETY: `physical` came from this instance.
    let limits = unsafe { instance.get_physical_device_properties(physical) }.limits;
    let image = hooks
        .max_image_dimension
        .unwrap_or(limits.max_image_dimension2_d);
    let (frame_width, frame_height) = match hooks.max_framebuffer_dimension {
        Some(limit) => (limit, limit),
        None => (limits.max_framebuffer_width, limits.max_framebuffer_height),
    };
    for (what, size, limit) in [
        ("maxImageDimension2D", width, image),
        ("maxImageDimension2D", height, image),
        ("maxFramebufferWidth", width, frame_width),
        ("maxFramebufferHeight", height, frame_height),
    ] {
        if size > limit {
            return Err(Error(format!(
                "the readback target is {width} x {height} pixels, over this device's {what} of {limit}"
            )));
        }
    }
    // SAFETY: `physical` came from this instance.
    let device_features = unsafe {
        instance
            .get_physical_device_format_properties(physical, TARGET_FORMAT)
            .optimal_tiling_features
    };
    let features = hooks.target_features.unwrap_or(device_features);
    let wanted = vk::FormatFeatureFlags::COLOR_ATTACHMENT
        | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND
        | vk::FormatFeatureFlags::TRANSFER_SRC;
    if !features.contains(wanted) {
        let missing = wanted & !features;
        return Err(Error(format!(
            "this device's B8G8R8A8_UNORM does not support {missing:?} with optimal tiling, which the readback target needs"
        )));
    }
    Ok(())
}

/// What the readback is asked to do.
struct Job<'a> {
    target: (u32, u32),
    /// The output's length in bytes, checked by [`output_length`] before anything was created.
    length: usize,
    clear: [f32; 3],
    quad: Option<&'a VulkanQuad>,
    hooks: ReadbackHooks,
}

/// Picks a device, creates it and runs the work. The device is returned when the caller is to destroy it
/// (after the objects, which this has destroyed), and `None` when it is kept (or never created).
fn run(inner: &InstanceInner, job: &Job<'_>) -> (Outcome, Option<ash::Device>) {
    let opened = match open_device(&inner.instance) {
        Ok(opened) => opened,
        Err(error) => return (Outcome::Clean(error), None),
    };
    // The device checks come first: **no object is created for a target or a quad the device cannot take.**
    if let Err(error) = target_checks(&inner.instance, opened.physical, job.target, job.hooks) {
        return (Outcome::Clean(error), Some(opened.device));
    }
    if let Some(quad) = job.quad
        && let Err(error) = device_checks(&inner.instance, opened.physical, quad, job.hooks)
    {
        return (Outcome::Clean(error), Some(opened.device));
    }
    let mut objects = Objects {
        device: &opened.device,
        pool: None,
        fence: None,
        render_pass: None,
        target: None,
        target_memory: None,
        target_view: None,
        framebuffer: None,
        buffer: None,
        buffer_memory: None,
        pipeline: QuadPipeline::default(),
        texture: QuadTexture::default(),
    };
    // An unwind is caught here, at the readback boundary, and becomes an `Error`: after the submit the
    // completion is unproven (the owners are kept), before it nothing is in flight.
    let submitted = Cell::new(false);
    let unwound = catch_unwind(AssertUnwindSafe(|| {
        read(&inner.instance, &opened, job, &mut objects, &submitted)
    }));
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

fn early((call, code): Failure) -> Outcome {
    Outcome::Clean(vk_error(call, code))
}

/// The work: builds the objects, records, submits and waits.
fn read(
    instance: &ash::Instance,
    opened: &Opened,
    job: &Job<'_>,
    objects: &mut Objects<'_>,
    submitted: &Cell<bool>,
) -> Outcome {
    // The upload's bytes, taken from the pixel source once and checked against the image's size, before anything
    // is created: they are the only bytes the staging buffer is sized from and filled with. `PixelSource` is a
    // safe trait, so this call is caller-supplied code: it runs here, inside the readback's unwind boundary, where
    // a panic before the submit becomes a clean error with the device and the holds released (nothing exists yet
    // for it to leave behind).
    let upload = match job.quad.map(upload_bytes).transpose() {
        Ok(upload) => upload,
        Err(error) => return Outcome::Clean(error),
    };
    let device = &opened.device;
    let hooks = job.hooks;
    let tally = Tally(hooks.quad_objects);
    let work = match build(
        instance,
        opened,
        (job, upload),
        objects,
        (tally, Tally(hooks.target_objects)),
    ) {
        Ok(work) => work,
        Err(failure) => return early(failure),
    };
    if let Err(failure) = record(device, &work, objects, job) {
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
    let length = job.length;
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
    // SAFETY: the mapping is at least `length` bytes, written by the finished copy.
    let bytes = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), length) }.to_vec();
    // SAFETY: the memory is mapped by this function.
    unsafe { device.unmap_memory(work.buffer_memory) };
    Outcome::Done(bytes)
}

/// The device-level state the readback works with.
struct Work {
    cmd: vk::CommandBuffer,
    queue: vk::Queue,
    fence: vk::Fence,
    buffer: vk::Buffer,
    buffer_memory: vk::DeviceMemory,
    render_pass: vk::RenderPass,
    framebuffer: vk::Framebuffer,
    target: vk::Image,
}

/// Creates the render pass, the target and its view and framebuffer, the quad's pipeline and texture (when
/// there is a quad), the host-visible buffer, the command pool and buffer and the fence, recording each in
/// `objects` as it exists.
fn build(
    instance: &ash::Instance,
    opened: &Opened,
    (job, upload): (&Job<'_>, Option<&[u8]>),
    objects: &mut Objects<'_>,
    (tally, target_tally): (Tally, Tally),
) -> Result<Work, Failure> {
    let device = &opened.device;
    let (width, height) = job.target;
    // SAFETY: `physical` came from this instance.
    let memory_properties =
        unsafe { instance.get_physical_device_memory_properties(opened.physical) };
    // SAFETY: the family was requested in the device create info.
    let queue = unsafe { device.get_device_queue(opened.family, 0) };
    target_tally.note();
    let render_pass = create_render_pass(device)?;
    objects.render_pass = Some(render_pass);
    let (target, framebuffer) = create_target(
        device,
        &memory_properties,
        job.target,
        objects,
        target_tally,
    )?;
    if let (Some(quad), Some(bytes)) = (job.quad, upload) {
        objects.pipeline.create(device, render_pass, tally)?;
        objects.texture.create(
            device,
            &memory_properties,
            &objects.pipeline,
            quad,
            bytes,
            tally,
        )?;
    }
    let length = u64::from(width) * u64::from(height) * 4;
    let buffer_info = vk::BufferCreateInfo::default()
        .size(length)
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
    // SAFETY: the memory is fresh and at least the size the buffer requires.
    unsafe { device.bind_buffer_memory(buffer, buffer_memory, 0) }
        .map_err(|code| ("vkBindBufferMemory", code))?;
    let pool_info = vk::CommandPoolCreateInfo::default().queue_family_index(opened.family);
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
    let fence_info = vk::FenceCreateInfo::default();
    // SAFETY: the device is live and `fence_info` is valid for the call.
    let fence = unsafe { device.create_fence(&fence_info, None) }
        .map_err(|code| ("vkCreateFence", code))?;
    objects.fence = Some(fence);
    Ok(Work {
        cmd,
        queue,
        fence,
        buffer,
        buffer_memory,
        render_pass,
        framebuffer,
        target,
    })
}

/// Creates the target image, its memory and view, and the framebuffer over it, recording each in `objects` as it
/// exists. Returns the image and the framebuffer.
fn create_target(
    device: &ash::Device,
    memory_properties: &vk::PhysicalDeviceMemoryProperties,
    (width, height): (u32, u32),
    objects: &mut Objects<'_>,
    tally: Tally,
) -> Result<(vk::Image, vk::Framebuffer), Failure> {
    let render_pass = objects.render_pass.ok_or((
        "vkCreateFramebuffer",
        vk::Result::ERROR_INITIALIZATION_FAILED,
    ))?;
    let target_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(TARGET_FORMAT)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    tally.note();
    // SAFETY: the device is live and `target_info` is valid for the call.
    let target = unsafe { device.create_image(&target_info, None) }
        .map_err(|code| ("vkCreateImage", code))?;
    objects.target = Some(target);
    // SAFETY: the image belongs to the device.
    let requirements = unsafe { device.get_image_memory_requirements(target) };
    let target_memory = allocate(
        device,
        memory_properties,
        requirements,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
        true,
    )?;
    objects.target_memory = Some(target_memory);
    // SAFETY: the memory is fresh and at least the size the image requires.
    unsafe { device.bind_image_memory(target, target_memory, 0) }
        .map_err(|code| ("vkBindImageMemory", code))?;
    let view_info = vk::ImageViewCreateInfo::default()
        .image(target)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(TARGET_FORMAT)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1),
        );
    tally.note();
    // SAFETY: the image is bound to memory and `view_info` is valid for the call.
    let view = unsafe { device.create_image_view(&view_info, None) }
        .map_err(|code| ("vkCreateImageView", code))?;
    objects.target_view = Some(view);
    let attachments = [view];
    let framebuffer_info = vk::FramebufferCreateInfo::default()
        .render_pass(render_pass)
        .attachments(&attachments)
        .width(width)
        .height(height)
        .layers(1);
    tally.note();
    // SAFETY: the render pass and view are live and `framebuffer_info` is valid for the call.
    let framebuffer = unsafe { device.create_framebuffer(&framebuffer_info, None) }
        .map_err(|code| ("vkCreateFramebuffer", code))?;
    objects.framebuffer = Some(framebuffer);
    Ok((target, framebuffer))
}

/// A render pass with one colour attachment: cleared on load, stored, ending in `TRANSFER_SRC_OPTIMAL`, with
/// dependencies that order the attachment's writes before the copy out.
fn create_render_pass(device: &ash::Device) -> Result<vk::RenderPass, Failure> {
    let attachments = [vk::AttachmentDescription::default()
        .format(TARGET_FORMAT)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)];
    let references = [vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
    let subpasses = [vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&references)];
    let dependencies = [
        // The upload's transfer writes, and the layout change, before the attachment is written.
        vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::TRANSFER)
            .dst_stage_mask(
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                    | vk::PipelineStageFlags::FRAGMENT_SHADER,
            )
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::SHADER_READ,
            ),
        // The attachment's writes before the copy to the host-visible buffer.
        vk::SubpassDependency::default()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::TRANSFER)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ),
    ];
    let info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(&subpasses)
        .dependencies(&dependencies);
    // SAFETY: the device is live and `info` (with what it points to) is valid for the call.
    unsafe { device.create_render_pass(&info, None) }.map_err(|code| ("vkCreateRenderPass", code))
}

/// Records: the upload (when there is a quad), the render pass (the clear, then the draw), the copy of the target
/// to the host-visible buffer and the barrier that makes it visible to the host.
fn record(
    device: &ash::Device,
    work: &Work,
    objects: &Objects<'_>,
    job: &Job<'_>,
) -> Result<(), Failure> {
    let cmd = work.cmd;
    let (width, height) = job.target;
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the command buffer is in the initial state.
    unsafe { device.begin_command_buffer(cmd, &begin) }
        .map_err(|code| ("vkBeginCommandBuffer", code))?;
    if job.quad.is_some() {
        objects.texture.record_upload(device, cmd);
    }
    let clear_values = [vk::ClearValue {
        color: vk::ClearColorValue {
            float32: [job.clear[0], job.clear[1], job.clear[2], 1.0],
        },
    }];
    let pass_info = vk::RenderPassBeginInfo::default()
        .render_pass(work.render_pass)
        .framebuffer(work.framebuffer)
        .render_area(vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: vk::Extent2D { width, height },
        })
        .clear_values(&clear_values);
    // SAFETY: the command buffer is recording; the pass and framebuffer are live and compatible.
    unsafe { device.cmd_begin_render_pass(cmd, &pass_info, vk::SubpassContents::INLINE) };
    if let Some(quad) = job.quad {
        record_draw(
            device,
            cmd,
            &objects.pipeline,
            &objects.texture,
            &QuadParams::of(quad),
            job.target,
        );
    }
    // SAFETY: the render pass is active on this command buffer.
    unsafe { device.cmd_end_render_pass(cmd) };
    let region = vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        });
    // SAFETY: the command buffer is recording; the pass left the target in `TRANSFER_SRC_OPTIMAL`.
    unsafe {
        device.cmd_copy_image_to_buffer(
            cmd,
            work.target,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_length_is_refused_before_any_vulkan_call_naming_the_numbers() {
        // 4096 × 4096 × 4 is exactly 64 MiB and accepted; one more is refused by the side bound.
        assert_eq!(
            output_length(4096, 4096, MAX_TARGET_SIDE, MAX_IMAGE_BYTES).expect("exactly 64 MiB"),
            64 * 1024 * 1024
        );
        for (width, height) in [(0, 4), (4, 0), (4097, 4), (4, 4097), (0, 0)] {
            let error = output_length(width, height, MAX_TARGET_SIDE, MAX_IMAGE_BYTES)
                .expect_err("outside 1..=4096");
            assert!(
                error.message().contains(&format!("{width} x {height}")),
                "{error}"
            );
            assert!(error.message().contains("4096"), "{error}");
        }
        // The byte bound is its own check: with a lower bound, an output over it is refused with the numbers,
        // even when both sides are within the side bound.
        let error = output_length(100, 100, MAX_TARGET_SIDE, 39_999).expect_err("over the bytes");
        assert!(error.message().contains("100 x 100"), "{error}");
        assert!(error.message().contains("39999"), "{error}");
        assert_eq!(
            output_length(100, 100, MAX_TARGET_SIDE, 40_000).expect("exactly the bytes"),
            40_000
        );
        // Checked arithmetic: sides whose product overflows are refused, never wrapped (a side bound of
        // `u32::MAX` lets them through to the byte check).
        let error = output_length(u32::MAX, u32::MAX, u32::MAX, u64::MAX).expect_err("overflow");
        assert!(error.message().contains("over the"), "{error}");
    }
}
