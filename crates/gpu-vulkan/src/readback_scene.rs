//! The off-screen scene readback (milestone 5.5, issue 2a): one scene frame drawn into an `R8G8B8A8_UNORM` image through
//! the presenter's own frame building and drawing code, then read back. It is used by the tests and by nothing else.
//!
//! The frame is built through a ticket of the caller's [`TextureLedger`](crate::TextureLedger) (the batches, the pins,
//! the sets), written into an instance buffer, recorded by [`record_scene`](crate::scene_draw::record_scene) and
//! submitted with [`FrameTicket::submit`](crate::FrameTicket::submit), so the frame's proof is the ledger's own
//! `frame_proven`. What the test reads is what the window draws, minus the swap chain.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ash::vk;

use crate::batch::FrameCounters;
use crate::context::{RenderQueue, VulkanContext};
use crate::error::vk_error;
use crate::frame::VulkanFrame;
use crate::revisions::TextureLedger;
use crate::scene::{HeldCompat, Scratch, build_frame};
use crate::scene_draw::{InstanceBuffer, SceneDraw, ScenePipeline, record_scene};
use crate::{Error, MAX_TARGET_SIDE};

/// How long the readback waits for the GPU before it reports the outcome as uncertain.
const PATIENCE: Duration = Duration::from_secs(60);

/// The objects of one readback, destroyed together once the frame is proven finished.
#[derive(Default)]
struct Objects {
    image: vk::Image,
    image_memory: Option<vk::DeviceMemory>,
    view: vk::ImageView,
    render_pass: vk::RenderPass,
    framebuffer: vk::Framebuffer,
    pool: vk::CommandPool,
    buffer: vk::Buffer,
    buffer_memory: Option<vk::DeviceMemory>,
    pipeline: Option<ScenePipeline>,
    instances: Option<InstanceBuffer>,
}

impl Objects {
    /// Destroys what exists. Only when nothing submitted uses it (the frame is proven finished, or none was submitted).
    fn destroy(&mut self, context: &VulkanContext) {
        let device = context.device();
        if let Some(mut pipeline) = self.pipeline.take() {
            pipeline.destroy(device);
        }
        // SAFETY: the pool was created on this device and nothing submitted uses it (null: a no-op).
        unsafe { device.destroy_command_pool(self.pool, None) };
        // SAFETY: as above for the framebuffer.
        unsafe { device.destroy_framebuffer(self.framebuffer, None) };
        // SAFETY: as above for the render pass.
        unsafe { device.destroy_render_pass(self.render_pass, None) };
        // SAFETY: as above for the view.
        unsafe { device.destroy_image_view(self.view, None) };
        // SAFETY: as above for the image.
        unsafe { device.destroy_image(self.image, None) };
        // SAFETY: as above for the buffer.
        unsafe { device.destroy_buffer(self.buffer, None) };
        if let Some(memory) = self.image_memory.take() {
            context.free_memory(memory);
        }
        if let Some(memory) = self.buffer_memory.take() {
            context.free_memory(memory);
        }
        if let Some(mut instances) = self.instances.take() {
            instances.destroy();
        }
    }
}

/// Draws `frame` off screen at `size` over `clear` and reads the pixels back (`size.0 * size.1 * 4` bytes, rows top first,
/// RGBA), with what building the frame counted. `ledger` has pumped and adopted what the frame's keys need, and its checker
/// is resident when the frame draws it. `max_quads` bounds the instance buffer.
///
/// # Errors
///
/// A size outside `1..=MAX_TARGET_SIDE`, a checker needed and not resident, the Vulkan failure, or an outcome that stayed
/// uncertain (the frame's proof did not arrive in time: everything is then retained for the rest of the process).
#[doc(hidden)]
pub fn readback_scene<K: Copy + Eq + std::hash::Hash + Send + 'static>(
    context: &Arc<VulkanContext>,
    queue: &mut RenderQueue,
    ledger: &TextureLedger<K>,
    size: (u32, u32),
    clear: [f32; 3],
    frame: &VulkanFrame<K>,
    max_quads: usize,
) -> Result<(Vec<u8>, FrameCounters), Error> {
    readback_scene_with_held(context, queue, ledger, size, clear, frame, max_quads, None)
}

/// [`readback_scene`] with the compatibility image the presenter kept on screen from an earlier frame (`held`, which must be
/// the ledger's current `compat_hold`): drawn, at the rectangle it was drawn at, while the frame's own compatibility image is
/// pending. A frame the initial gate holds back (nothing to draw for the compatibility quad and nothing held) is an error.
///
/// # Errors
///
/// As [`readback_scene`], and a gated frame.
#[doc(hidden)]
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "readback_scene's arguments plus the held image, and one fixed sequence of creation, recording, proof and cleanup"
)]
pub fn readback_scene_with_held<K: Copy + Eq + std::hash::Hash + Send + 'static>(
    context: &Arc<VulkanContext>,
    queue: &mut RenderQueue,
    ledger: &TextureLedger<K>,
    size: (u32, u32),
    clear: [f32; 3],
    frame: &VulkanFrame<K>,
    max_quads: usize,
    held: Option<HeldCompat<K>>,
) -> Result<(Vec<u8>, FrameCounters), Error> {
    let (width, height) = size;
    if width == 0 || height == 0 || width > MAX_TARGET_SIDE || height > MAX_TARGET_SIDE {
        return Err(Error(format!(
            "the target is {width} x {height}; each side must be 1 to {MAX_TARGET_SIDE}"
        )));
    }
    let device = context.device();
    let bytes = u64::from(width) * u64::from(height) * 4;
    let mut objects = Objects::default();
    let result = (|| -> Result<(Vec<u8>, FrameCounters), (Error, bool)> {
        let early = |error: Error| (error, false);
        // The target image, its view, the render pass and the framebuffer.
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_UNORM)
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
        // SAFETY: the device is live and `image_info` is valid for the call.
        objects.image = unsafe { device.create_image(&image_info, None) }
            .map_err(|code| early(vk_error("vkCreateImage(target)", code)))?;
        // SAFETY: the image was just created on this device.
        let requirements = unsafe { device.get_image_memory_requirements(objects.image) };
        let memory_type = context
            .memory_type(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or_else(|| early(Error("no memory type for the target".to_owned())))?;
        let memory = context
            .allocate_memory(requirements.size, memory_type, None)
            .map_err(|error| {
                early(Error(format!(
                    "the target could not be allocated: {error:?}"
                )))
            })?;
        objects.image_memory = Some(memory);
        // SAFETY: the memory is unbound and large enough for the image.
        unsafe { device.bind_image_memory(objects.image, memory, 0) }
            .map_err(|code| early(vk_error("vkBindImageMemory(target)", code)))?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(objects.image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::R8G8B8A8_UNORM)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .level_count(1)
                    .layer_count(1),
            );
        // SAFETY: the image is bound; `view_info` is valid for the call.
        objects.view = unsafe { device.create_image_view(&view_info, None) }
            .map_err(|code| early(vk_error("vkCreateImageView(target)", code)))?;
        let attachments = [vk::AttachmentDescription::default()
            .format(vk::Format::R8G8B8A8_UNORM)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)];
        let color = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpasses = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color)];
        let dependencies = [vk::SubpassDependency::default()
            .src_subpass(0)
            .dst_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::TRANSFER)
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ)];
        let pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        // SAFETY: the device is live; `pass_info` and what it points to are valid for the call.
        objects.render_pass = unsafe { device.create_render_pass(&pass_info, None) }
            .map_err(|code| early(vk_error("vkCreateRenderPass(target)", code)))?;
        let views = [objects.view];
        let framebuffer_info = vk::FramebufferCreateInfo::default()
            .render_pass(objects.render_pass)
            .attachments(&views)
            .width(width)
            .height(height)
            .layers(1);
        // SAFETY: the device is live; `framebuffer_info` is valid for the call.
        objects.framebuffer = unsafe { device.create_framebuffer(&framebuffer_info, None) }
            .map_err(|code| early(vk_error("vkCreateFramebuffer(target)", code)))?;
        // The pixels' destination buffer.
        let buffer_info = vk::BufferCreateInfo::default()
            .size(bytes)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the device is live; `buffer_info` is valid for the call.
        objects.buffer = unsafe { device.create_buffer(&buffer_info, None) }
            .map_err(|code| early(vk_error("vkCreateBuffer(readback)", code)))?;
        // SAFETY: the buffer was just created on this device.
        let buffer_requirements = unsafe { device.get_buffer_memory_requirements(objects.buffer) };
        let host_type = context
            .memory_type(
                buffer_requirements.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE,
                vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or_else(|| {
                early(Error(
                    "no host-visible memory type for the readback".to_owned(),
                ))
            })?;
        let buffer_memory = context
            .allocate_memory(buffer_requirements.size, host_type, None)
            .map_err(|error| {
                early(Error(format!(
                    "the readback could not be allocated: {error:?}"
                )))
            })?;
        objects.buffer_memory = Some(buffer_memory);
        // SAFETY: the memory is unbound and large enough for the buffer.
        unsafe { device.bind_buffer_memory(objects.buffer, buffer_memory, 0) }
            .map_err(|code| early(vk_error("vkBindBufferMemory(readback)", code)))?;
        // The pipeline and the instance buffer: the presenter's own.
        objects.pipeline = Some(
            ScenePipeline::create(device, objects.render_pass, ledger.pipeline_layout())
                .map_err(early)?,
        );
        objects.instances =
            Some(InstanceBuffer::create(context, max_quads, u64::MAX).map_err(early)?);
        let pool_info = vk::CommandPoolCreateInfo::default().queue_family_index(context.family());
        // SAFETY: the device is live.
        objects.pool = unsafe { device.create_command_pool(&pool_info, None) }
            .map_err(|code| early(vk_error("vkCreateCommandPool(readback)", code)))?;
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(objects.pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool is live.
        let cmd = unsafe { device.allocate_command_buffers(&allocate) }
            .map_err(|code| early(vk_error("vkAllocateCommandBuffers(readback)", code)))?[0];

        // ---- the frame: a ticket, the instances, the commands ----
        let ticket = ledger
            .begin_frame()
            .map_err(|error| early(Error(format!("no frame ticket: {error:?}"))))?;
        let mut scratch = Scratch::default();
        let buffer = objects
            .instances
            .as_ref()
            .ok_or_else(|| early(Error("no instance buffer".to_owned())))?;
        let built = build_frame(&ticket, frame, size, buffer, max_quads, &mut scratch, held)
            .map(|(built, _)| built);
        if built.as_ref().is_some_and(|built| built.gated) {
            let _ = ticket.not_submitted();
            return Err(early(Error(
                "the frame is held back by the initial gate: the compatibility quad has no image"
                    .to_owned(),
            )));
        }
        let counters = built
            .as_ref()
            .map_or_else(FrameCounters::default, |built| built.counters);
        let batches = scratch.batches();
        if batches.iter().any(crate::batch::Batch::is_checker) && ledger.checker_set().is_none() {
            let _ = ticket.not_submitted();
            return Err(early(Error(
                "the frame draws the checker, which is not resident".to_owned(),
            )));
        }
        buffer
            .flush(usize::try_from(counters.instances).unwrap_or(usize::MAX))
            .map_err(early)?;
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: the command buffer is in the initial state.
        unsafe { device.begin_command_buffer(cmd, &begin) }
            .map_err(|code| early(vk_error("vkBeginCommandBuffer(readback)", code)))?;
        let clear_value = [vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [clear[0], clear[1], clear[2], 1.0],
            },
        }];
        let pass_begin = vk::RenderPassBeginInfo::default()
            .render_pass(objects.render_pass)
            .framebuffer(objects.framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D::default(),
                extent: vk::Extent2D { width, height },
            })
            .clear_values(&clear_value);
        // SAFETY: the command buffer is recording; the render pass and framebuffer are compatible.
        unsafe { device.cmd_begin_render_pass(cmd, &pass_begin, vk::SubpassContents::INLINE) };
        if let (Some(built), Some(pipeline)) = (&built, &objects.pipeline) {
            let draw = SceneDraw {
                pipeline,
                layout: ledger.pipeline_layout(),
                instances: buffer.handle(),
                batches,
                checker: ledger.checker_set().unwrap_or_else(vk::DescriptorSet::null),
                samplers: ledger.sampler_sets(),
                filter: built.filter,
            };
            record_scene(device, cmd, &draw, size);
        }
        // SAFETY: the render pass was begun on this command buffer.
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
        // SAFETY: the image is in `TRANSFER_SRC_OPTIMAL` (the render pass's final layout) and the buffer is a destination.
        unsafe {
            device.cmd_copy_image_to_buffer(
                cmd,
                objects.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                objects.buffer,
                &[region],
            );
        }
        let to_host = [vk::BufferMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(objects.buffer)
            .offset(0)
            .size(vk::WHOLE_SIZE)];
        // SAFETY: the command buffer is recording outside a render pass.
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[],
                &to_host,
                &[],
            );
        }
        // SAFETY: the command buffer is recording.
        unsafe { device.end_command_buffer(cmd) }
            .map_err(|code| early(vk_error("vkEndCommandBuffer(readback)", code)))?;
        let submission = match ticket.submit(queue, cmd, &[], &[]) {
            Ok(submission) => submission,
            Err(error) => {
                // A submit that failed with out of memory left the ticket building; any other outcome may have reached the
                // GPU and the ticket keeps its pins.
                return Err((
                    Error(format!("the frame could not be submitted: {error:?}")),
                    true,
                ));
            }
        };
        drop(ticket);
        // ---- the proof: the ledger's latched record ----
        let started = Instant::now();
        let mut events = Vec::new();
        while !ledger.frame_proven(submission) {
            ledger.pump(queue, &mut events);
            if started.elapsed() > PATIENCE {
                return Err((
                    Error("the frame's proof did not arrive; everything is retained".to_owned()),
                    true,
                ));
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        // ---- the pixels ----
        let memory = objects
            .buffer_memory
            .ok_or_else(|| early(Error("no readback memory".to_owned())))?;
        let length = usize::try_from(bytes)
            .map_err(|_| early(Error("the target is too large".to_owned())))?;
        // SAFETY: the memory is host-visible, unmapped and not in use by a pending operation (the frame is proven).
        let mapped =
            unsafe { device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) }
                .map_err(|code| (vk_error("vkMapMemory(readback)", code), true))?;
        if !context.is_coherent(host_type) {
            // A host-visible type that is not coherent: the device's writes are made visible to the host by an invalidate,
            // not by the proof alone (the ordinary Linux portability contract).
            let range = vk::MappedMemoryRange::default()
                .memory(memory)
                .offset(0)
                .size(vk::WHOLE_SIZE);
            // SAFETY: the memory is mapped by this readback and the range covers all of it.
            if let Err(code) = unsafe { device.invalidate_mapped_memory_ranges(&[range]) } {
                // SAFETY: the memory was mapped just above and nothing read it.
                unsafe { device.unmap_memory(memory) };
                return Err(early(vk_error(
                    "vkInvalidateMappedMemoryRanges(readback)",
                    code,
                )));
            }
        }
        let mut pixels = vec![0_u8; length];
        // SAFETY: the mapping covers the buffer's `length` bytes, the copy's source and destination are distinct.
        unsafe { std::ptr::copy_nonoverlapping(mapped.cast::<u8>(), pixels.as_mut_ptr(), length) };
        // SAFETY: the memory was mapped just above.
        unsafe { device.unmap_memory(memory) };
        Ok((pixels, counters))
    })();
    match result {
        Ok(read) => {
            objects.destroy(context);
            Ok(read)
        }
        Err((error, uncertain)) => {
            if uncertain {
                // The frame may still run: the objects and the device owner stay for good.
                std::mem::forget(objects);
                std::mem::forget(Arc::clone(context));
            } else {
                objects.destroy(context);
            }
            Err(error)
        }
    }
}
