//! The off-screen scene readback (milestone 5.5, issue 2a): one scene frame drawn into an `R8G8B8A8_UNORM` image through
//! the presenter's own frame building and drawing code, then read back. It is used by the tests and by nothing else.
//!
//! The frame is built through a ticket of the caller's [`TextureLedger`](crate::TextureLedger) (the batches, the pins,
//! the sets), written into an instance buffer, recorded by [`record_scene`](crate::scene_draw::record_scene) and
//! submitted with [`FrameTicket::submit`](crate::FrameTicket::submit), so the frame's proof is the ledger's own
//! `frame_proven`. What the test reads is what the window draws, minus the swap chain.
//!
//! The frame is drawn in phases so a test can hold it: [`SceneReadback::prepare`] opens the ticket (which pins the table
//! version in the bindless mode), builds and records, optionally behind a host event; [`SceneReadback::submit`] submits it; and
//! [`SceneReadback::finish`] pumps the ledger until its proof and reads the pixels. [`readback_scene`] does all three. The slot
//! probe ([`SceneContent::Slots`]) draws one cell per listed slot of a named table version through the same pipeline and
//! `record_scene`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ash::vk;

use crate::batch::{Batch, Binding, FrameCounters, InstanceLayout, InstanceRecord};
use crate::context::{RenderQueue, VulkanContext};
use crate::error::vk_error;
use crate::frame::VulkanFrame;
use crate::quad::VulkanFilter;
use crate::revisions::{FrameSubmission, FrameTicket, TextureLedger};
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

/// What a prepared scene frame draws.
#[doc(hidden)]
pub enum SceneContent<'f, K> {
    /// A scene frame, built through its ticket exactly as the presenter builds it. `held` is the compatibility image the
    /// presenter kept on screen from an earlier frame (the ledger's current `compat_hold`).
    Frame {
        /// The frame.
        frame: &'f VulkanFrame<K>,
        /// The held compatibility image.
        held: Option<HeldCompat<K>>,
    },
    /// One [`PROBE_CELL`]-pixel cell per listed slot of the bindless table `version`, in reading order, drawn through a ticket
    /// that pins that version (bindless mode only).
    Slots {
        /// The table version to bind.
        version: usize,
        /// The slots, one cell each.
        slots: &'f [u32],
    },
}

/// The side of a probe cell, in pixels: the whole 64 x 64 image of a slot is drawn into it with nearest sampling.
pub const PROBE_CELL: u32 = 8;

/// One off-screen scene frame in phases. See the module documentation.
#[doc(hidden)]
pub struct SceneReadback<'a, K: Copy + Eq + std::hash::Hash + Send + 'static> {
    context: Arc<VulkanContext>,
    objects: Objects,
    ticket: Option<FrameTicket<'a, K>>,
    cmd: vk::CommandBuffer,
    counters: FrameCounters,
    size: (u32, u32),
    host_type: u32,
    submission: Option<FrameSubmission>,
    /// The frame may have reached the GPU and its completion is not proven: everything is retained.
    uncertain: bool,
    finished: bool,
}

impl<K: Copy + Eq + std::hash::Hash + Send + 'static> Drop for SceneReadback<'_, K> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if self.uncertain || self.submission.is_some() {
            // The frame may still run: the objects and the device owner stay for good.
            std::mem::forget(std::mem::take(&mut self.objects));
            std::mem::forget(Arc::clone(&self.context));
            std::mem::forget(self.ticket.take());
        } else {
            if let Some(ticket) = self.ticket.take() {
                let _ = ticket.not_submitted();
            }
            self.objects.destroy(&self.context);
        }
    }
}

impl<'a, K: Copy + Eq + std::hash::Hash + Send + 'static> SceneReadback<'a, K> {
    /// Creates the target, opens the frame's ticket, builds the instances and records the commands, **without submitting**.
    /// With `hold`, the commands first wait for that host event (set from the test, never by a sleep), so the frame stays
    /// pending after it is submitted. `max_quads` bounds the instance buffer.
    ///
    /// # Errors
    ///
    /// A size outside `1..=MAX_TARGET_SIDE`, a checker needed and not resident, a frame held back by the initial gate, a probe
    /// that does not fit, or the Vulkan failure. Nothing was submitted: everything made is destroyed.
    pub fn prepare(
        context: &Arc<VulkanContext>,
        ledger: &'a TextureLedger<K>,
        size: (u32, u32),
        clear: [f32; 3],
        what: &SceneContent<'_, K>,
        max_quads: usize,
        hold: Option<vk::Event>,
    ) -> Result<Self, Error> {
        let (width, height) = size;
        if width == 0 || height == 0 || width > MAX_TARGET_SIDE || height > MAX_TARGET_SIDE {
            return Err(Error(format!(
                "the target is {width} x {height}; each side must be 1 to {MAX_TARGET_SIDE}"
            )));
        }
        let mut this = Self {
            context: Arc::clone(context),
            objects: Objects::default(),
            ticket: None,
            cmd: vk::CommandBuffer::null(),
            counters: FrameCounters::default(),
            size,
            host_type: 0,
            submission: None,
            uncertain: false,
            finished: false,
        };
        // Drop (below, on error) destroys what exists: nothing was submitted.
        this.build(ledger, clear, what, max_quads, hold)?;
        Ok(this)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one fixed sequence: the target, the pipeline, the instances, the commands"
    )]
    fn build(
        &mut self,
        ledger: &'a TextureLedger<K>,
        clear: [f32; 3],
        what: &SceneContent<'_, K>,
        max_quads: usize,
        hold: Option<vk::Event>,
    ) -> Result<(), Error> {
        let owner = Arc::clone(&self.context);
        let device = owner.device();
        let (width, height) = self.size;
        let bytes = u64::from(width) * u64::from(height) * 4;
        let objects = &mut self.objects;
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
            .map_err(|code| vk_error("vkCreateImage(target)", code))?;
        // SAFETY: the image was just created on this device.
        let requirements = unsafe { device.get_image_memory_requirements(objects.image) };
        let memory_type = owner
            .memory_type(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::empty(),
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )
            .ok_or_else(|| Error("no memory type for the target".to_owned()))?;
        let memory = owner
            .allocate_memory(requirements.size, memory_type, None)
            .map_err(|error| Error(format!("the target could not be allocated: {error:?}")))?;
        objects.image_memory = Some(memory);
        // SAFETY: the memory is unbound and large enough for the image.
        unsafe { device.bind_image_memory(objects.image, memory, 0) }
            .map_err(|code| vk_error("vkBindImageMemory(target)", code))?;
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
            .map_err(|code| vk_error("vkCreateImageView(target)", code))?;
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
            .map_err(|code| vk_error("vkCreateRenderPass(target)", code))?;
        let views = [objects.view];
        let framebuffer_info = vk::FramebufferCreateInfo::default()
            .render_pass(objects.render_pass)
            .attachments(&views)
            .width(width)
            .height(height)
            .layers(1);
        // SAFETY: the device is live; `framebuffer_info` is valid for the call.
        objects.framebuffer = unsafe { device.create_framebuffer(&framebuffer_info, None) }
            .map_err(|code| vk_error("vkCreateFramebuffer(target)", code))?;
        // The pixels' destination buffer.
        let buffer_info = vk::BufferCreateInfo::default()
            .size(bytes)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the device is live; `buffer_info` is valid for the call.
        objects.buffer = unsafe { device.create_buffer(&buffer_info, None) }
            .map_err(|code| vk_error("vkCreateBuffer(readback)", code))?;
        // SAFETY: the buffer was just created on this device.
        let buffer_requirements = unsafe { device.get_buffer_memory_requirements(objects.buffer) };
        self.host_type = owner
            .memory_type(
                buffer_requirements.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE,
                vk::MemoryPropertyFlags::HOST_COHERENT,
            )
            .ok_or_else(|| Error("no host-visible memory type for the readback".to_owned()))?;
        let buffer_memory = owner
            .allocate_memory(buffer_requirements.size, self.host_type, None)
            .map_err(|error| Error(format!("the readback could not be allocated: {error:?}")))?;
        objects.buffer_memory = Some(buffer_memory);
        // SAFETY: the memory is unbound and large enough for the buffer.
        unsafe { device.bind_buffer_memory(objects.buffer, buffer_memory, 0) }
            .map_err(|code| vk_error("vkBindBufferMemory(readback)", code))?;
        // The pipeline and the instance buffer: the presenter's own.
        let instance_layout = if ledger.bindless_slots() == 0 {
            InstanceLayout::PER_REVISION
        } else {
            InstanceLayout::bindless(
                owner
                    .bindless_caps()
                    .limits
                    .min_vertex_input_binding_stride_alignment,
            )
            .ok_or_else(|| Error("the bindless instance stride overflows".to_owned()))?
        };
        objects.pipeline = Some(ScenePipeline::create(
            device,
            objects.render_pass,
            ledger.pipeline_layout(),
            ledger.bindless_slots() != 0,
            instance_layout,
        )?);
        objects.instances = Some(InstanceBuffer::create(
            &owner,
            instance_layout,
            max_quads,
            u64::MAX,
        )?);
        let pool_info = vk::CommandPoolCreateInfo::default().queue_family_index(owner.family());
        // SAFETY: the device is live.
        objects.pool = unsafe { device.create_command_pool(&pool_info, None) }
            .map_err(|code| vk_error("vkCreateCommandPool(readback)", code))?;
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(objects.pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool is live.
        self.cmd = unsafe { device.allocate_command_buffers(&allocate) }
            .map_err(|code| vk_error("vkAllocateCommandBuffers(readback)", code))?[0];

        // ---- the frame: a ticket, the instances, the commands ----
        let ticket = match what {
            SceneContent::Frame { .. } => ledger.begin_frame(),
            SceneContent::Slots { version, .. } => ledger.test_begin_frame_on(*version),
        }
        .map_err(|error| Error(format!("no frame ticket: {error:?}")))?;
        let buffer = objects
            .instances
            .as_ref()
            .ok_or_else(|| Error("no instance buffer".to_owned()))?;
        let mut scratch = Scratch::default();
        let mut probe_batches: Vec<Batch> = Vec::new();
        let (counters, filter) = match what {
            SceneContent::Frame { frame, held } => {
                let built = build_frame(
                    &ticket,
                    frame,
                    self.size,
                    buffer,
                    max_quads,
                    &mut scratch,
                    *held,
                )
                .map(|(built, _)| built);
                if built.as_ref().is_some_and(|built| built.gated) {
                    let _ = ticket.not_submitted();
                    return Err(Error(
                        "the frame is held back by the initial gate: the compatibility quad has no image"
                            .to_owned(),
                    ));
                }
                (
                    built
                        .as_ref()
                        .map_or_else(FrameCounters::default, |built| built.counters),
                    built
                        .as_ref()
                        .map_or(VulkanFilter::Nearest, |built| built.filter),
                )
            }
            SceneContent::Slots { slots, .. } => {
                let cell_px = PROBE_CELL as usize;
                let columns = (width as usize / cell_px).max(1);
                let rows = height as usize / cell_px;
                if slots.len() > columns * rows || slots.len() > max_quads {
                    let _ = ticket.not_submitted();
                    return Err(Error(format!(
                        "{} slots do not fit {columns} x {rows} cells and {max_quads} quads",
                        slots.len()
                    )));
                }
                let pixel = |value: usize| f32::from(u16::try_from(value).unwrap_or(u16::MAX));
                for (index, &slot) in slots.iter().enumerate() {
                    let (left, top) = (index % columns * cell_px, index / columns * cell_px);
                    buffer.write(
                        index,
                        InstanceRecord {
                            destination: [
                                pixel(left),
                                pixel(top),
                                pixel(left + cell_px),
                                pixel(top + cell_px),
                            ],
                            source: [0.0, 0.0, 1.0, 1.0],
                            slot,
                        },
                    );
                }
                // One draw: each instance names its own slot.
                probe_batches.push(Batch {
                    binding: Binding::Table,
                    first: 0,
                    count: u32::try_from(slots.len()).unwrap_or(u32::MAX),
                });
                (
                    FrameCounters {
                        instances: slots.len() as u64,
                        batches: 1,
                        ..FrameCounters::default()
                    },
                    VulkanFilter::Nearest,
                )
            }
        };
        let batches: &[Batch] = if probe_batches.is_empty() {
            scratch.batches()
        } else {
            &probe_batches
        };
        if batches.iter().any(Batch::is_checker)
            && ledger.bindless_slots() == 0
            && ledger.checker_set().is_none()
        {
            let _ = ticket.not_submitted();
            return Err(Error(
                "the frame draws the checker, which is not resident".to_owned(),
            ));
        }
        self.counters = counters;
        buffer.flush(usize::try_from(counters.instances).unwrap_or(usize::MAX))?;
        let begin = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: the command buffer is in the initial state.
        unsafe { device.begin_command_buffer(self.cmd, &begin) }
            .map_err(|code| vk_error("vkBeginCommandBuffer(readback)", code))?;
        if let Some(event) = hold {
            // The frame waits for the host to set the event: held pending, never by a sleep.
            // SAFETY: the command buffer is recording outside a render pass; the event belongs to this device.
            unsafe {
                device.cmd_wait_events(
                    self.cmd,
                    &[event],
                    vk::PipelineStageFlags::HOST,
                    vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::TRANSFER,
                    &[],
                    &[],
                    &[],
                );
            }
        }
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
        unsafe { device.cmd_begin_render_pass(self.cmd, &pass_begin, vk::SubpassContents::INLINE) };
        if let Some(pipeline) = &objects.pipeline {
            let draw = SceneDraw {
                pipeline,
                layout: ledger.pipeline_layout(),
                instances: buffer.handle(),
                batches,
                checker: ledger.checker_set().unwrap_or_else(vk::DescriptorSet::null),
                table: ticket.table().unwrap_or_else(vk::DescriptorSet::null),
                samplers: ledger.sampler_sets(),
                filter,
            };
            record_scene(device, self.cmd, &draw, self.size);
        }
        // SAFETY: the render pass was begun on this command buffer.
        unsafe { device.cmd_end_render_pass(self.cmd) };
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
                self.cmd,
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
                self.cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[],
                &to_host,
                &[],
            );
        }
        // SAFETY: the command buffer is recording.
        unsafe { device.end_command_buffer(self.cmd) }
            .map_err(|code| vk_error("vkEndCommandBuffer(readback)", code))?;
        self.ticket = Some(ticket);
        Ok(())
    }

    /// The frame's ticket, while it is open (before [`submit`](Self::submit)): the table version it pinned, and what it
    /// resolved.
    #[must_use]
    pub fn ticket(&self) -> Option<&FrameTicket<'a, K>> {
        self.ticket.as_ref()
    }

    /// What building the frame counted.
    #[must_use]
    pub fn counters(&self) -> FrameCounters {
        self.counters
    }

    /// Submits the frame through its ticket. Its proof is the ledger's own `frame_proven`.
    ///
    /// # Errors
    ///
    /// The ticket was already submitted, or the submit failed (the frame is then retained: its outcome is uncertain).
    pub fn submit(&mut self, queue: &mut RenderQueue) -> Result<FrameSubmission, Error> {
        let ticket = self
            .ticket
            .take()
            .ok_or_else(|| Error("the frame was already submitted".to_owned()))?;
        match ticket.submit(queue, self.cmd, &[], &[]) {
            Ok(submission) => {
                self.submission = Some(submission);
                drop(ticket);
                Ok(submission)
            }
            Err(error) => {
                self.uncertain = true;
                std::mem::forget(ticket);
                Err(Error(format!(
                    "the frame could not be submitted: {error:?}"
                )))
            }
        }
    }

    /// Whether the frame's proof is recorded (after [`submit`](Self::submit)).
    #[must_use]
    pub fn is_proven(&self, ledger: &TextureLedger<K>) -> bool {
        self.submission
            .is_some_and(|submission| ledger.frame_proven(submission))
    }

    /// Pumps `ledger` until the frame's proof, then reads the pixels back (`size.0 * size.1 * 4` bytes, rows top first, RGBA)
    /// and destroys what the readback made.
    ///
    /// # Errors
    ///
    /// The frame was not submitted, its proof did not arrive in time (everything is retained for the rest of the process), or
    /// the Vulkan failure.
    pub fn finish(
        mut self,
        queue: &mut RenderQueue,
        ledger: &TextureLedger<K>,
    ) -> Result<(Vec<u8>, FrameCounters), Error> {
        let submission = self
            .submission
            .ok_or_else(|| Error("the frame was not submitted".to_owned()))?;
        let started = Instant::now();
        let mut events = Vec::new();
        while !ledger.frame_proven(submission) {
            ledger.pump(queue, &mut events);
            if started.elapsed() > PATIENCE {
                return Err(Error(
                    "the frame's proof did not arrive; everything is retained".to_owned(),
                ));
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        let context = Arc::clone(&self.context);
        let device = context.device();
        let (width, height) = self.size;
        let length = usize::try_from(u64::from(width) * u64::from(height) * 4)
            .map_err(|_| Error("the target is too large".to_owned()))?;
        let memory = self
            .objects
            .buffer_memory
            .ok_or_else(|| Error("no readback memory".to_owned()))?;
        // SAFETY: the memory is host-visible, unmapped and not in use by a pending operation (the frame is proven).
        let mapped =
            unsafe { device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) }
                .map_err(|code| vk_error("vkMapMemory(readback)", code))?;
        if !context.is_coherent(self.host_type) {
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
                self.objects.destroy(&context);
                self.finished = true;
                return Err(vk_error("vkInvalidateMappedMemoryRanges(readback)", code));
            }
        }
        let mut pixels = vec![0_u8; length];
        // SAFETY: the mapping covers the buffer's `length` bytes, the copy's source and destination are distinct.
        unsafe { std::ptr::copy_nonoverlapping(mapped.cast::<u8>(), pixels.as_mut_ptr(), length) };
        // SAFETY: the memory was mapped just above.
        unsafe { device.unmap_memory(memory) };
        self.objects.destroy(&context);
        self.finished = true;
        Ok((pixels, self.counters))
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
    reason = "readback_scene's arguments plus the held image"
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
    let mut readback = SceneReadback::prepare(
        context,
        ledger,
        size,
        clear,
        &SceneContent::Frame { frame, held },
        max_quads,
        None,
    )?;
    readback.submit(queue)?;
    readback.finish(queue, ledger)
}

/// The probe: the 8 x 8 cells of `slots` of table `version`, as `PROBE_CELL * PROBE_CELL * 4` bytes each, in the order listed.
/// Up to 4,096 slots per call, drawn into a 512 x 512 target through the production pipeline and `record_scene`, with a ticket
/// that pins `version` like a frame's.
///
/// # Errors
///
/// As [`SceneReadback::prepare`] and [`SceneReadback::finish`].
#[doc(hidden)]
pub fn readback_slots<K: Copy + Eq + std::hash::Hash + Send + 'static>(
    context: &Arc<VulkanContext>,
    queue: &mut RenderQueue,
    ledger: &TextureLedger<K>,
    version: usize,
    slots: &[u32],
) -> Result<Vec<Vec<u8>>, Error> {
    let side = 512_u32;
    let mut readback = SceneReadback::prepare(
        context,
        ledger,
        (side, side),
        [0.0, 0.0, 0.0],
        &SceneContent::Slots { version, slots },
        4_096,
        None,
    )?;
    readback.submit(queue)?;
    let (pixels, _) = readback.finish(queue, ledger)?;
    let columns = (side / PROBE_CELL) as usize;
    let cell = PROBE_CELL as usize;
    Ok((0..slots.len())
        .map(|index| {
            let (column, row) = (index % columns, index / columns);
            let mut bytes = Vec::with_capacity(cell * cell * 4);
            for y in 0..cell {
                let start = ((row * cell + y) * side as usize + column * cell) * 4;
                bytes.extend_from_slice(&pixels[start..start + cell * 4]);
            }
            bytes
        })
        .collect())
}
