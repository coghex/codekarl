//! The Vulkan objects of the map pass and their recording (milestone 6, slice 1). The data is [`map`](crate::map)'s, the bookkeeping of what
//! is resident [`map_residency`](crate::map_residency)'s; this module creates, records and destroys.
//!
//! - **[`MapPipeline`]**: the map pass's render pass (one `R8G8B8A8_UNORM` colour attachment, kept in `COLOR_ATTACHMENT_OPTIMAL` by the
//!   pass; the layouts change by the explicit barriers below), its descriptor set layout (three sampled images: grid A, grid B, the
//!   palette), a pool, the pipeline layout (a 24-byte push constant for the fragment stage) and the pipeline (the full-target triangle,
//!   dynamic viewport and scissor, no blending).
//! - **[`MapInputObjects`]**: grid A, grid B and the palette (a 256 x 1 image) as `R8G8B8A8_UNORM` optimal-tiling images (`SAMPLED |
//!   TRANSFER_DST`), their memory, views, one descriptor set naming them, and one host-visible staging buffer holding all three. The
//!   upload is recorded in the frame's command buffer before the map pass and counts as done only once the frame is submitted.
//! - **[`MapTargetObjects`]**: the map target, an `R8G8B8A8_UNORM` image of the view's size (`COLOR_ATTACHMENT | SAMPLED`), its view and
//!   framebuffer, and the descriptor set (of the legacy quad's own set layout, one combined image sampler with the nearest sampler) that
//!   the window pass draws it through.
//! - **The recording.** [`Gpu::record_map_pass`] runs outside any render pass, before the window pass: the input's upload when it is
//!   pending, **one barrier** that orders the earlier shader reads of the target (the previous submission's window pass) before this
//!   pass's colour writes and discards its old contents (every pixel is written), the pass, and **one barrier** that orders the colour
//!   writes before the window pass's fragment-shader reads, with the layout change to shader-read-only. [`Gpu::record_map_quad`] runs in
//!   the window pass, after the clear and before the legacy quad, through the quad's pipeline. Nothing waits on the CPU.
//!
//! Every object is created by one function and destroyed by one, each keeping what exists in the struct as it goes, so a failure in
//! the middle destroys exactly what was made, and a caller that must keep everything (an unproven completion) does not destroy.

use std::io::Cursor;

use ash::vk;

use crate::Error;
use crate::draw::{QuadParams, QuadPipeline, color_range, record_draw_with_set};
use crate::gpu::{Failure, Gpu};
use crate::hooks::QuadFaults;
use crate::map::{MAP_SIDE_MAX, VulkanMap, VulkanMapInput};
use crate::quad::{DestinationRect, SourceRect, VulkanFilter};
use crate::readback::allocate;

const VERTEX_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/map.vert.spv"));
const FRAGMENT_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/map.frag.spv"));

/// The format of every map image, the target included.
pub(crate) const MAP_FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;
/// The push constants: `origin`, `crop` and `top_left`, each two `i32`.
const PUSH_BYTES: u32 = 24;
/// The palette image's width: one pixel per palette entry.
const PALETTE_WIDTH: u32 = 256;
/// The descriptor sets the pool holds: an input and a retired input (three images each), a target and a retired target (the window
/// pass's combined image sampler each).
const POOL_SETS: u32 = 4;

/// What a frame's map pass records. Plain data: it names no object, so it is carried from the frame's preparation to its recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MapRecord {
    /// The input's upload is recorded in this frame (it was never submitted).
    pub(crate) upload: bool,
    /// The push constants: `origin`, `crop` and `top_left`.
    pub(crate) constants: [i32; 6],
    /// The view, which is the target's size, in map pixels.
    pub(crate) view: [u32; 2],
    /// Where the enlarged map lands, in drawable pixels.
    pub(crate) destination: DestinationRect,
    /// The part of the target drawn there, in map pixels.
    pub(crate) source: SourceRect,
}

impl MapRecord {
    /// The record for a frame of `map`.
    pub(crate) fn of(map: &VulkanMap, upload: bool) -> Self {
        let input = map.input();
        let [origin_x, origin_y] = input.origin();
        let [crop_width, crop_height] = input.size();
        let [left, top] = map.top_left();
        Self {
            upload,
            constants: [
                origin_x,
                origin_y,
                i32::try_from(crop_width).unwrap_or(i32::MAX),
                i32::try_from(crop_height).unwrap_or(i32::MAX),
                left,
                top,
            ],
            view: map.view(),
            destination: map.destination(),
            source: map.source(),
        }
    }

    /// The quad the window pass draws: the target, nearest, whole view-sized image.
    fn quad(&self) -> QuadParams {
        QuadParams {
            width: self.view[0],
            height: self.view[1],
            destination: self.destination,
            source: self.source,
            filter: VulkanFilter::Nearest,
        }
    }
}

fn module(device: &ash::Device, bytes: &[u8]) -> Result<vk::ShaderModule, Failure> {
    let words = ash::util::read_spv(&mut Cursor::new(bytes)).map_err(|_| {
        (
            "the SPIR-V is not whole words",
            vk::Result::ERROR_INITIALIZATION_FAILED,
        )
    })?;
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    // SAFETY: the device is live and `info` (with the words it points to) is valid for the call.
    unsafe { device.create_shader_module(&info, None) }
        .map_err(|code| ("vkCreateShaderModule", code))
}

/// The map pass's render pass, descriptor layout and pool, pipeline layout and pipeline. Fields are `None` until created.
#[derive(Default)]
pub(crate) struct MapPipeline {
    pub(crate) pass: Option<vk::RenderPass>,
    pub(crate) set_layout: Option<vk::DescriptorSetLayout>,
    pub(crate) pool: Option<vk::DescriptorPool>,
    pub(crate) layout: Option<vk::PipelineLayout>,
    pub(crate) pipeline: Option<vk::Pipeline>,
}

impl MapPipeline {
    /// Whether any object of the pipeline exists: what a partial creation leaves.
    pub(crate) fn exists(&self) -> bool {
        self.pass.is_some()
            || self.set_layout.is_some()
            || self.pool.is_some()
            || self.layout.is_some()
            || self.pipeline.is_some()
    }

    /// Creates everything, recording each object as it exists.
    pub(crate) fn create(&mut self, device: &ash::Device) -> Result<(), Failure> {
        let attachments = [vk::AttachmentDescription::default()
            .format(MAP_FORMAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let references = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpasses = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&references)];
        let pass_info = vk::RenderPassCreateInfo::default()
            .attachments(&attachments)
            .subpasses(&subpasses);
        // SAFETY: the device is live and `pass_info` (with what it points to) is valid for the call.
        self.pass = Some(
            unsafe { device.create_render_pass(&pass_info, None) }
                .map_err(|code| ("vkCreateRenderPass", code))?,
        );
        let bindings: Vec<vk::DescriptorSetLayoutBinding<'_>> = (0..3)
            .map(|binding| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            })
            .collect();
        let set_layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: the device is live and `set_layout_info` (with its bindings) is valid for the call.
        let set_layout = unsafe { device.create_descriptor_set_layout(&set_layout_info, None) }
            .map_err(|code| ("vkCreateDescriptorSetLayout", code))?;
        self.set_layout = Some(set_layout);
        let sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(6),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(2),
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
            .max_sets(POOL_SETS)
            .pool_sizes(&sizes);
        // SAFETY: the device is live and `pool_info` (with its sizes) is valid for the call.
        self.pool = Some(
            unsafe { device.create_descriptor_pool(&pool_info, None) }
                .map_err(|code| ("vkCreateDescriptorPool", code))?,
        );
        let layouts = [set_layout];
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(PUSH_BYTES)];
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&layouts)
            .push_constant_ranges(&ranges);
        // SAFETY: the device is live and `layout_info` (with what it points to) is valid for the call.
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .map_err(|code| ("vkCreatePipelineLayout", code))?;
        self.layout = Some(layout);
        self.create_pipeline(device, layout)
    }

    fn create_pipeline(
        &mut self,
        device: &ash::Device,
        layout: vk::PipelineLayout,
    ) -> Result<(), Failure> {
        let Some(pass) = self.pass else {
            return Err((
                "vkCreateGraphicsPipelines",
                vk::Result::ERROR_INITIALIZATION_FAILED,
            ));
        };
        let vertex = module(device, VERTEX_SPIRV)?;
        let fragment = match module(device, FRAGMENT_SPIRV) {
            Ok(fragment) => fragment,
            Err(failure) => {
                // SAFETY: the module was just created and nothing uses it.
                unsafe { device.destroy_shader_module(vertex, None) };
                return Err(failure);
            }
        };
        let entry = c"main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vertex)
                .name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(fragment)
                .name(entry),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(false)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .render_pass(pass)
            .subpass(0);
        // SAFETY: the device is live and `info` (with everything it points to) is valid for the call.
        let created =
            unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) };
        // A pipeline does not keep its modules.
        // SAFETY: the modules were created above and the call has returned.
        unsafe { device.destroy_shader_module(vertex, None) };
        // SAFETY: as above.
        unsafe { device.destroy_shader_module(fragment, None) };
        let pipelines = created.map_err(|(_, code)| ("vkCreateGraphicsPipelines", code))?;
        self.pipeline = pipelines.first().copied();
        Ok(())
    }

    /// Destroys what exists, in reverse order of creation. Only when nothing submitted uses any of it.
    pub(crate) fn destroy(&mut self, device: &ash::Device) {
        if let Some(pipeline) = self.pipeline.take() {
            // SAFETY: nothing submitted uses the pipeline.
            unsafe { device.destroy_pipeline(pipeline, None) };
        }
        if let Some(layout) = self.layout.take() {
            // SAFETY: the pipeline using it is destroyed.
            unsafe { device.destroy_pipeline_layout(layout, None) };
        }
        if let Some(pool) = self.pool.take() {
            // SAFETY: no submitted frame uses a set of the pool; destroying it frees them.
            unsafe { device.destroy_descriptor_pool(pool, None) };
        }
        if let Some(set_layout) = self.set_layout.take() {
            // SAFETY: nothing uses the layout.
            unsafe { device.destroy_descriptor_set_layout(set_layout, None) };
        }
        if let Some(pass) = self.pass.take() {
            // SAFETY: the pipeline and the framebuffers made against it are destroyed.
            unsafe { device.destroy_render_pass(pass, None) };
        }
    }
}

/// An image with its memory and view. Fields are `None` until created.
#[derive(Default)]
struct MapImage {
    image: Option<vk::Image>,
    memory: Option<vk::DeviceMemory>,
    view: Option<vk::ImageView>,
}

impl MapImage {
    /// Creates the image of `extent` with `usage`, its memory (device-local preferred) and its view. `fail_allocation` forces the
    /// memory allocation to fail, after the image exists (a test).
    fn create(
        &mut self,
        device: &ash::Device,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        extent: (u32, u32),
        usage: vk::ImageUsageFlags,
        fail_allocation: bool,
    ) -> Result<(), Failure> {
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(MAP_FORMAT)
            .extent(vk::Extent3D {
                width: extent.0,
                height: extent.1,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: the device is live and `info` is valid for the call.
        let image =
            unsafe { device.create_image(&info, None) }.map_err(|code| ("vkCreateImage", code))?;
        self.image = Some(image);
        if fail_allocation {
            return Err(("vkAllocateMemory", vk::Result::ERROR_OUT_OF_DEVICE_MEMORY));
        }
        // SAFETY: the image belongs to the device.
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let memory = allocate(
            device,
            memory_properties,
            requirements,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
            true,
        )?;
        self.memory = Some(memory);
        // SAFETY: the memory is fresh and at least the size the image requires.
        unsafe { device.bind_image_memory(image, memory, 0) }
            .map_err(|code| ("vkBindImageMemory", code))?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(MAP_FORMAT)
            .subresource_range(color_range());
        // SAFETY: the image is bound to memory and `view_info` is valid for the call.
        self.view = Some(
            unsafe { device.create_image_view(&view_info, None) }
                .map_err(|code| ("vkCreateImageView", code))?,
        );
        Ok(())
    }

    fn destroy(&mut self, device: &ash::Device) {
        if let Some(view) = self.view.take() {
            // SAFETY: nothing uses the view any more.
            unsafe { device.destroy_image_view(view, None) };
        }
        if let Some(image) = self.image.take() {
            // SAFETY: nothing uses the image any more.
            unsafe { device.destroy_image(image, None) };
        }
        if let Some(memory) = self.memory.take() {
            // SAFETY: the image bound to it is destroyed.
            unsafe { device.free_memory(memory, None) };
        }
    }
}

/// A map input on the device: grid A, grid B and the palette, their descriptor set, and the one staging buffer of their upload.
#[derive(Default)]
pub(crate) struct MapInputObjects {
    images: [MapImage; 3],
    set: Option<vk::DescriptorSet>,
    staging: Option<vk::Buffer>,
    staging_memory: Option<vk::DeviceMemory>,
    /// The three images' sizes, and the byte offset of each in the staging buffer.
    extents: [(u32, u32); 3],
    offsets: [u64; 3],
    /// The input's generation (informational).
    pub(crate) generation: u64,
}

impl MapInputObjects {
    /// Creates the three images, their descriptor set and the staging buffer filled with the three images' bytes.
    pub(crate) fn create(
        &mut self,
        device: &ash::Device,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        pipeline: &MapPipeline,
        input: &VulkanMapInput,
        faults: QuadFaults,
    ) -> Result<(), Failure> {
        self.generation = input.generation();
        let [width, height] = input.size();
        let palette = input.palette_image();
        let parts: [&[u8]; 3] = [input.grid_a(), input.grid_b(), &palette];
        self.extents = [(width, height), (width, height), (PALETTE_WIDTH, 1)];
        let mut total = 0_u64;
        for (offset, part) in self.offsets.iter_mut().zip(parts) {
            *offset = total;
            total += part.len() as u64;
        }
        for (image, extent) in self.images.iter_mut().zip(self.extents) {
            image.create(
                device,
                memory_properties,
                extent,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
                faults.texture_allocation,
            )?;
        }
        self.allocate_set(device, pipeline)?;
        self.create_staging(device, memory_properties, &parts, total, faults)
    }

    fn allocate_set(
        &mut self,
        device: &ash::Device,
        pipeline: &MapPipeline,
    ) -> Result<(), Failure> {
        let (Some(pool), Some(set_layout)) = (pipeline.pool, pipeline.set_layout) else {
            return Err((
                "vkAllocateDescriptorSets",
                vk::Result::ERROR_INITIALIZATION_FAILED,
            ));
        };
        let layouts = [set_layout];
        let info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        // SAFETY: the pool and the layout are live and `info` is valid for the call.
        let set = unsafe { device.allocate_descriptor_sets(&info) }
            .map_err(|code| ("vkAllocateDescriptorSets", code))?[0];
        self.set = Some(set);
        let infos: Vec<[vk::DescriptorImageInfo; 1]> = self
            .images
            .iter()
            .map(|image| {
                [vk::DescriptorImageInfo::default()
                    .image_view(image.view.unwrap_or_default())
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
            })
            .collect();
        let writes: Vec<vk::WriteDescriptorSet<'_>> = infos
            .iter()
            .enumerate()
            .map(|(binding, info)| {
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(u32::try_from(binding).unwrap_or(0))
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(info)
            })
            .collect();
        // SAFETY: the set is fresh (no submitted work uses it) and `writes` names live image views.
        unsafe { device.update_descriptor_sets(&writes, &[]) };
        Ok(())
    }

    fn create_staging(
        &mut self,
        device: &ash::Device,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        parts: &[&[u8]; 3],
        total: u64,
        faults: QuadFaults,
    ) -> Result<(), Failure> {
        let info = vk::BufferCreateInfo::default()
            .size(total)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the device is live and `info` is valid for the call.
        let buffer = unsafe { device.create_buffer(&info, None) }
            .map_err(|code| ("vkCreateBuffer", code))?;
        self.staging = Some(buffer);
        if faults.staging_allocation {
            return Err(("vkAllocateMemory", vk::Result::ERROR_OUT_OF_DEVICE_MEMORY));
        }
        // SAFETY: the buffer belongs to the device.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let memory = allocate(
            device,
            memory_properties,
            requirements,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            false,
        )?;
        self.staging_memory = Some(memory);
        // SAFETY: the memory is fresh and at least the size the buffer requires.
        unsafe { device.bind_buffer_memory(buffer, memory, 0) }
            .map_err(|code| ("vkBindBufferMemory", code))?;
        // SAFETY: the memory is host visible and coherent, not mapped, and at least `total` bytes long.
        let mapped = unsafe { device.map_memory(memory, 0, total, vk::MemoryMapFlags::empty()) }
            .map_err(|code| ("vkMapMemory", code))?;
        for (part, offset) in parts.iter().zip(self.offsets) {
            let offset = usize::try_from(offset).unwrap_or(0);
            // SAFETY: `mapped` points at `total` writable bytes and each part lies inside them.
            let destination = unsafe { mapped.cast::<u8>().add(offset) };
            // SAFETY: `destination` has `part.len()` writable bytes that nothing else touches.
            unsafe { std::ptr::copy_nonoverlapping(part.as_ptr(), destination, part.len()) };
        }
        // SAFETY: the memory was mapped just above.
        unsafe { device.unmap_memory(memory) };
        Ok(())
    }

    /// Records the upload of the three images: barriers to transfer destination, the buffer-to-image copies and barriers to shader
    /// read. Before the map pass, outside any render pass, in the same command buffer.
    fn record_upload(&self, device: &ash::Device, cmd: vk::CommandBuffer) {
        let Some(staging) = self.staging else {
            return;
        };
        let barrier = |image: vk::Image,
                       (old, new): (vk::ImageLayout, vk::ImageLayout),
                       (from, to): (vk::AccessFlags, vk::AccessFlags)| {
            vk::ImageMemoryBarrier::default()
                .src_access_mask(from)
                .dst_access_mask(to)
                .old_layout(old)
                .new_layout(new)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(color_range())
        };
        let images: Vec<vk::Image> = self.images.iter().filter_map(|image| image.image).collect();
        let to_destination: Vec<_> = images
            .iter()
            .map(|image| {
                barrier(
                    *image,
                    (
                        vk::ImageLayout::UNDEFINED,
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    ),
                    (vk::AccessFlags::empty(), vk::AccessFlags::TRANSFER_WRITE),
                )
            })
            .collect();
        // SAFETY: the command buffer is recording outside a render pass; the barriers name this device's images.
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &to_destination,
            );
        }
        for ((image, extent), offset) in images.iter().zip(self.extents).zip(self.offsets) {
            let region = vk::BufferImageCopy::default()
                .buffer_offset(offset)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width: extent.0,
                    height: extent.1,
                    depth: 1,
                });
            // SAFETY: the command buffer is recording; the image is in `TRANSFER_DST_OPTIMAL` and the buffer holds the pixels.
            unsafe {
                device.cmd_copy_buffer_to_image(
                    cmd,
                    staging,
                    *image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
            }
        }
        let to_shader: Vec<_> = images
            .iter()
            .map(|image| {
                barrier(
                    *image,
                    (
                        vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    ),
                    (
                        vk::AccessFlags::TRANSFER_WRITE,
                        vk::AccessFlags::SHADER_READ,
                    ),
                )
            })
            .collect();
        // SAFETY: the command buffer is recording; the barriers name this device's images.
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &to_shader,
            );
        }
    }

    /// Destroys the staging buffer and its memory: once the upload frame's submit is proven (or was never made).
    pub(crate) fn destroy_staging(&mut self, device: &ash::Device) {
        if let Some(buffer) = self.staging.take() {
            // SAFETY: the upload that read the buffer is proven finished (or never submitted).
            unsafe { device.destroy_buffer(buffer, None) };
        }
        if let Some(memory) = self.staging_memory.take() {
            // SAFETY: the buffer bound to it is destroyed.
            unsafe { device.free_memory(memory, None) };
        }
    }

    /// Destroys what exists: the staging buffer, the set (returned to `pool`), the views, the images and their memory. Only when no
    /// submitted frame uses any of it.
    pub(crate) fn destroy(&mut self, device: &ash::Device, pool: Option<vk::DescriptorPool>) {
        self.destroy_staging(device);
        if let (Some(set), Some(pool)) = (self.set.take(), pool) {
            // SAFETY: no submitted frame uses the set. A failure to free it is not an error: the pool's destruction frees it.
            let _ = unsafe { device.free_descriptor_sets(pool, &[set]) };
        }
        for image in &mut self.images {
            image.destroy(device);
        }
    }
}

/// The map target on the device: the image, its framebuffer for the map pass and the descriptor set the window pass draws it through.
#[derive(Default)]
pub(crate) struct MapTargetObjects {
    image: MapImage,
    framebuffer: Option<vk::Framebuffer>,
    set: Option<vk::DescriptorSet>,
    /// The target's size in map pixels.
    pub(crate) size: [u32; 2],
}

impl MapTargetObjects {
    /// Creates the target of `size`, its framebuffer against the map pass and its drawing set from the quad pipeline's set layout.
    pub(crate) fn create(
        &mut self,
        device: &ash::Device,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        pipeline: &MapPipeline,
        quad: &QuadPipeline,
        size: [u32; 2],
        (faults, extra_usage): (QuadFaults, vk::ImageUsageFlags),
    ) -> Result<(), Failure> {
        self.size = size;
        self.image.create(
            device,
            memory_properties,
            (size[0], size[1]),
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED | extra_usage,
            faults.texture_allocation,
        )?;
        let (Some(pass), Some(view), Some(pool), Some(set_layout)) = (
            pipeline.pass,
            self.image.view,
            pipeline.pool,
            quad.set_layout,
        ) else {
            return Err((
                "vkCreateFramebuffer",
                vk::Result::ERROR_INITIALIZATION_FAILED,
            ));
        };
        let views = [view];
        let framebuffer_info = vk::FramebufferCreateInfo::default()
            .render_pass(pass)
            .attachments(&views)
            .width(size[0])
            .height(size[1])
            .layers(1);
        // SAFETY: the render pass and the view are live and `framebuffer_info` is valid for the call.
        self.framebuffer = Some(
            unsafe { device.create_framebuffer(&framebuffer_info, None) }
                .map_err(|code| ("vkCreateFramebuffer", code))?,
        );
        let layouts = [set_layout];
        let info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        // SAFETY: the pool and the layout are live and `info` is valid for the call.
        let set = unsafe { device.allocate_descriptor_sets(&info) }
            .map_err(|code| ("vkAllocateDescriptorSets", code))?[0];
        self.set = Some(set);
        let images = [vk::DescriptorImageInfo::default()
            .sampler(quad.sampler(VulkanFilter::Nearest))
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&images);
        // SAFETY: the set is fresh and `write` names live objects.
        unsafe { device.update_descriptor_sets(&[write], &[]) };
        Ok(())
    }

    /// Destroys what exists. Only when no submitted frame uses any of it.
    pub(crate) fn destroy(&mut self, device: &ash::Device, pool: Option<vk::DescriptorPool>) {
        if let (Some(set), Some(pool)) = (self.set.take(), pool) {
            // SAFETY: no submitted frame uses the set; a failure to free it is not an error (the pool's destruction frees it).
            let _ = unsafe { device.free_descriptor_sets(pool, &[set]) };
        }
        if let Some(framebuffer) = self.framebuffer.take() {
            // SAFETY: nothing uses the framebuffer any more.
            unsafe { device.destroy_framebuffer(framebuffer, None) };
        }
        self.image.destroy(device);
    }

    pub(crate) fn image(&self) -> Option<vk::Image> {
        self.image.image
    }
}

/// What the render thread created for the map: the pipeline, the resident input and target, and the retired ones that wait for their
/// proofs. Bookkept by [`MapResidency`](crate::map_residency::MapResidency); these are the objects behind that bookkeeping.
#[derive(Default)]
pub(crate) struct MapObjects {
    pub(crate) pipeline: MapPipeline,
    pub(crate) input: Option<MapInputObjects>,
    pub(crate) target: Option<MapTargetObjects>,
    pub(crate) retired_input: Option<MapInputObjects>,
    pub(crate) retired_target: Option<MapTargetObjects>,
}

impl MapObjects {
    /// Whether any map object exists.
    pub(crate) fn any(&self) -> bool {
        self.pipeline.exists()
            || self.input.is_some()
            || self.target.is_some()
            || self.retired_input.is_some()
            || self.retired_target.is_some()
    }
}

/// The device's checks for a map, run **before any object is created for it**: the crop and the view fit `maxImageDimension2D` (or
/// the limit a test lowers it to), and `R8G8B8A8_UNORM` supports sampling, transfer destination and colour attachment with optimal
/// tiling. An error names the size and the limit, or the missing capability.
pub(crate) fn device_checks(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    map: &VulkanMap,
    max_dimension: Option<u32>,
) -> Result<(), Error> {
    // SAFETY: `physical` came from this instance.
    let properties = unsafe { instance.get_physical_device_properties(physical) };
    let limit = max_dimension
        .unwrap_or(properties.limits.max_image_dimension2_d)
        .min(MAP_SIDE_MAX);
    for (what, [width, height]) in [("crop", map.input().size()), ("view", map.view())] {
        if width > limit || height > limit {
            return Err(Error(format!(
                "the map's {what} is {width} x {height}, over this device's maxImageDimension2D of {limit}"
            )));
        }
    }
    // SAFETY: `physical` came from this instance.
    let features = unsafe {
        instance
            .get_physical_device_format_properties(physical, MAP_FORMAT)
            .optimal_tiling_features
    };
    let wanted = vk::FormatFeatureFlags::SAMPLED_IMAGE
        | vk::FormatFeatureFlags::TRANSFER_DST
        | vk::FormatFeatureFlags::COLOR_ATTACHMENT;
    if !features.contains(wanted) {
        let missing = wanted & !features;
        return Err(Error(format!(
            "this device's R8G8B8A8_UNORM does not support {missing:?} with optimal tiling, which the map needs"
        )));
    }
    Ok(())
}

/// Records the map pass into a command buffer, **before** the window pass and outside any render pass: the input's upload when
/// `record.upload`, the barrier that orders the earlier shader reads of the target before the pass's colour writes (the old contents
/// are discarded: the triangle writes every pixel), the pass, and the barrier that orders its colour writes before the window pass's
/// fragment-shader reads, with the layout change to shader-read-only. The presenter's frames and the off-screen readback record
/// through this one function.
pub(crate) fn record_map_pass(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    objects: (&MapPipeline, &MapInputObjects, &MapTargetObjects),
    record: &MapRecord,
) {
    let (pipeline, input, target) = objects;
    let (Some(graphics), Some(layout), Some(pass)) =
        (pipeline.pipeline, pipeline.layout, pipeline.pass)
    else {
        return;
    };
    let (Some(image), Some(framebuffer), Some(set)) =
        (target.image(), target.framebuffer, input.set)
    else {
        return;
    };
    if record.upload {
        input.record_upload(device, cmd);
    }
    let to_attachment = vk::ImageMemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::empty())
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
        .old_layout(vk::ImageLayout::UNDEFINED)
        .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(color_range());
    // SAFETY: the command buffer is recording outside a render pass; the barrier names this device's image. The source stage is
    // the fragment shader of the earlier submission's window pass, whose reads of the target must finish before the colour writes.
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_attachment],
        );
    }
    let area = vk::Rect2D {
        offset: vk::Offset2D::default(),
        extent: vk::Extent2D {
            width: record.view[0],
            height: record.view[1],
        },
    };
    let begin = vk::RenderPassBeginInfo::default()
        .render_pass(pass)
        .framebuffer(framebuffer)
        .render_area(area);
    #[expect(
        clippy::cast_precision_loss,
        reason = "a view side is at most 16384, which f32 holds exactly"
    )]
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: record.view[0] as f32,
        height: record.view[1] as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    let mut constants = [0_u8; PUSH_BYTES as usize];
    for (chunk, value) in constants.chunks_exact_mut(4).zip(record.constants) {
        chunk.copy_from_slice(&value.to_ne_bytes());
    }
    // SAFETY: the command buffer is recording outside a render pass; the render pass and framebuffer are compatible.
    unsafe { device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE) };
    // SAFETY: inside the render pass just begun; the pipeline is compatible with it.
    unsafe { device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, graphics) };
    // SAFETY: the pipeline declares dynamic viewport and scissor.
    unsafe { device.cmd_set_viewport(cmd, 0, &[viewport]) };
    // SAFETY: as above.
    unsafe { device.cmd_set_scissor(cmd, 0, &[area]) };
    // SAFETY: the set was allocated from the pipeline's layout and written.
    unsafe {
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::GRAPHICS,
            layout,
            0,
            &[set],
            &[],
        );
    }
    // SAFETY: the push constant range covers these 24 bytes for the fragment stage.
    unsafe {
        device.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::FRAGMENT, 0, &constants);
    }
    // SAFETY: the pipeline, descriptor set and push constants are bound.
    unsafe { device.cmd_draw(cmd, 3, 1, 0, 0) };
    // SAFETY: the render pass was begun on this command buffer.
    unsafe { device.cmd_end_render_pass(cmd) };
    let to_shader = vk::ImageMemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
        .dst_access_mask(vk::AccessFlags::SHADER_READ)
        .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
        .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(color_range());
    // SAFETY: the command buffer is recording outside a render pass; the barrier names this device's image.
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[to_shader],
        );
    }
}

impl Gpu {
    /// Creates the map pipeline against nothing the swap chain owns (the map pass has its own render pass).
    pub(crate) fn create_map_pipeline(&mut self) -> Result<(), Failure> {
        self.map.pipeline.create(&self.device)
    }

    /// Destroys every map object, the retired and resident ones first and the pipeline after: the teardown's, once the end is proven.
    pub(crate) fn destroy_map(&mut self) {
        let mut objects = std::mem::take(&mut self.map);
        let pool = objects.pipeline.pool;
        for mut input in [objects.retired_input.take(), objects.input.take()]
            .into_iter()
            .flatten()
        {
            input.destroy(&self.device, pool);
            if let Some(observer) = &self.observer {
                observer.map_input_released(false);
            }
        }
        for mut target in [objects.retired_target.take(), objects.target.take()]
            .into_iter()
            .flatten()
        {
            target.destroy(&self.device, pool);
            if let Some(observer) = &self.observer {
                observer.map_target_released(false);
            }
        }
        objects.pipeline.destroy(&self.device);
    }

    /// Records the map pass into the frame's command buffer (see [`record_map_pass`]).
    pub(crate) fn record_map_pass(&self, cmd: vk::CommandBuffer, record: &MapRecord) {
        if let (Some(input), Some(target)) = (&self.map.input, &self.map.target) {
            record_map_pass(
                &self.device,
                cmd,
                (&self.map.pipeline, input, target),
                record,
            );
        }
    }

    /// Records the enlarged map into the window pass (see [`record_map_quad`]).
    pub(crate) fn record_map_quad(
        &self,
        cmd: vk::CommandBuffer,
        record: &MapRecord,
        target: (u32, u32),
    ) {
        if let Some(objects) = &self.map.target {
            record_map_quad(
                &self.device,
                cmd,
                &self.quad.pipeline,
                objects,
                record,
                target,
            );
        }
    }
}

/// Records the enlarged map into the window pass, after the clear and before the legacy quad: the map target drawn by the quad's own
/// pipeline with the nearest sampler. The presenter's frames and the off-screen readback record through this one function.
pub(crate) fn record_map_quad(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    quad: &QuadPipeline,
    target: &MapTargetObjects,
    record: &MapRecord,
    extent: (u32, u32),
) {
    if let Some(set) = target.set {
        record_draw_with_set(device, cmd, quad, set, &record.quad(), extent);
    }
}
