//! The textured quad's drawing: one internal module, so the off-screen readback tests exactly what the presenter
//! will draw (design §7.1).
//!
//! - **[`QuadPipeline`]**: the quad shaders (compiled to SPIR-V by `build.rs`), two triangles (six
//!   vertices) made from push constants (no vertex buffer), dynamic viewport and scissor, the premultiplied blend
//!   `ONE, ONE_MINUS_SRC_ALPHA` for colour and alpha, its descriptor set layout (one combined image sampler) and
//!   pipeline layout, a fixed descriptor pool, and one sampler per filter (clamp to edge, no mipmaps, no
//!   anisotropy). It is created against a render pass whose colour attachment is `B8G8R8A8_UNORM` or
//!   `R8G8B8A8_UNORM` (design §7.3).
//! - **[`QuadTexture`]**: an `R8G8B8A8_UNORM` optimal-tiling image (`SAMPLED | TRANSFER_DST`) with its memory
//!   (allocated at `VkMemoryRequirements::size`, device-local preferred), its view, one descriptor set from the
//!   pool and a host-visible staging buffer at the payload size, filled with the source's bytes at creation.
//!   [`record_upload`](QuadTexture::record_upload) records the copy into the command buffer, before the render
//!   pass: a barrier to transfer destination, the buffer-to-image copy, a barrier to shader read.
//! - **[`record_draw`]**: inside the render pass, after the clear: bind, push constants, draw. Texture
//!   coordinates are the source rectangle over the image size; the destination converts to the target's normalised
//!   device coordinates from its top-left pixel origin (Vulkan's +y points down the framebuffer, so nothing flips).
//! - **[`device_checks`]**: run once the device is known and **before any object is created for a quad**: the
//!   image fits `maxImageDimension2D`, and `R8G8B8A8_UNORM` supports sampling and transfer destination with
//!   optimal tiling (and linear filtering for a linear quad).
//!
//! Every object is created by one function (`create`) and destroyed by one (`destroy`), each of which keeps
//! what exists in the struct as it goes, so a failure in the middle destroys exactly what was made, and a caller
//! that must keep everything (an unproven completion) simply does not call `destroy`.

use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};

use ash::vk;

use crate::Error;
use crate::hooks::{PipelineFault, QuadFaults};
use crate::quad::{DestinationRect, SourceRect, VulkanFilter, VulkanQuad};
use crate::readback::{Failure, ReadbackHooks, allocate};

/// The vertex shader's SPIR-V, compiled by `build.rs`.
const VERTEX_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/quad.vert.spv"));
/// The fragment shader's SPIR-V, compiled by `build.rs`.
const FRAGMENT_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/quad.frag.spv"));

/// How many descriptor sets the fixed pool holds: the resident texture and the retired one.
const POOL_SETS: u32 = 2;
/// The push constants: the destination in normalised device coordinates and the texture rectangle, each
/// `(left, top, right, bottom)` as four `f32`.
const PUSH_BYTES: u32 = 32;

/// Counts every object whose creation begins, for the tests (a partial creation counts).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Tally(pub(crate) Option<&'static AtomicUsize>);

impl Tally {
    /// One more quad object is about to be created.
    pub(crate) fn note(self) {
        if let Some(count) = self.0 {
            count.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// What drawing a quad needs of it, without the pixel source: the render thread drops its reference to the source
/// once the bytes are copied, and every later frame draws from these.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct QuadParams {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) destination: DestinationRect,
    pub(crate) source: SourceRect,
    pub(crate) filter: VulkanFilter,
}

impl QuadParams {
    pub(crate) fn of(quad: &VulkanQuad) -> Self {
        Self {
            width: quad.width(),
            height: quad.height(),
            destination: quad.destination(),
            source: quad.source(),
            filter: quad.filter(),
        }
    }
}

/// The shader module of `bytes`, which are SPIR-V words.
fn module(device: &ash::Device, bytes: &[u8], tally: Tally) -> Result<vk::ShaderModule, Failure> {
    let words = ash::util::read_spv(&mut Cursor::new(bytes)).map_err(|_| {
        (
            "the SPIR-V is not whole words",
            vk::Result::ERROR_INITIALIZATION_FAILED,
        )
    })?;
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    tally.note();
    // SAFETY: the device is live and `info` (with the words it points to) is valid for the call.
    unsafe { device.create_shader_module(&info, None) }
        .map_err(|code| ("vkCreateShaderModule", code))
}

/// The pipeline, its layouts, the pool and the samplers. Fields are `None` until created.
#[derive(Default)]
pub(crate) struct QuadPipeline {
    pub(crate) pipeline: Option<vk::Pipeline>,
    pub(crate) layout: Option<vk::PipelineLayout>,
    pub(crate) set_layout: Option<vk::DescriptorSetLayout>,
    pub(crate) pool: Option<vk::DescriptorPool>,
    nearest: Option<vk::Sampler>,
    linear: Option<vk::Sampler>,
}

impl QuadPipeline {
    /// Creates everything against `render_pass`, recording each object as it exists.
    pub(crate) fn create(
        &mut self,
        device: &ash::Device,
        render_pass: vk::RenderPass,
        tally: Tally,
    ) -> Result<(), Failure> {
        self.create_with_fault(device, render_pass, tally, None)
    }

    /// Whether any object of the pipeline exists, a sampler alone included: what a partial creation leaves.
    pub(crate) fn exists(&self) -> bool {
        self.pipeline.is_some()
            || self.layout.is_some()
            || self.set_layout.is_some()
            || self.pool.is_some()
            || self.nearest.is_some()
            || self.linear.is_some()
    }

    /// [`create`](Self::create), failing where `fault` says (a test's forced failure: everything before it exists
    /// and is destroyed by the caller's cleanup).
    pub(crate) fn create_with_fault(
        &mut self,
        device: &ash::Device,
        render_pass: vk::RenderPass,
        tally: Tally,
        fault: Option<PipelineFault>,
    ) -> Result<(), Failure> {
        for (slot, filter) in [
            (&mut self.nearest, vk::Filter::NEAREST),
            (&mut self.linear, vk::Filter::LINEAR),
        ] {
            if fault == Some(PipelineFault::SecondSampler) && filter == vk::Filter::LINEAR {
                return Err(("vkCreateSampler", vk::Result::ERROR_OUT_OF_HOST_MEMORY));
            }
            let info = vk::SamplerCreateInfo::default()
                .mag_filter(filter)
                .min_filter(filter)
                .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .anisotropy_enable(false)
                .compare_enable(false)
                .min_lod(0.0)
                .max_lod(0.0)
                .unnormalized_coordinates(false);
            tally.note();
            // SAFETY: the device is live and `info` is valid for the call.
            *slot = Some(
                unsafe { device.create_sampler(&info, None) }
                    .map_err(|code| ("vkCreateSampler", code))?,
            );
        }
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let set_layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        tally.note();
        // SAFETY: the device is live and `set_layout_info` (with its bindings) is valid for the call.
        let set_layout = unsafe { device.create_descriptor_set_layout(&set_layout_info, None) }
            .map_err(|code| ("vkCreateDescriptorSetLayout", code))?;
        self.set_layout = Some(set_layout);
        let sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(POOL_SETS)];
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
            .max_sets(POOL_SETS)
            .pool_sizes(&sizes);
        tally.note();
        // SAFETY: the device is live and `pool_info` (with its sizes) is valid for the call.
        self.pool = Some(
            unsafe { device.create_descriptor_pool(&pool_info, None) }
                .map_err(|code| ("vkCreateDescriptorPool", code))?,
        );
        let layouts = [set_layout];
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(PUSH_BYTES)];
        let layout_info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&layouts)
            .push_constant_ranges(&ranges);
        tally.note();
        // SAFETY: the device is live and `layout_info` (with what it points to) is valid for the call.
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None) }
            .map_err(|code| ("vkCreatePipelineLayout", code))?;
        self.layout = Some(layout);
        if fault == Some(PipelineFault::Pipeline) {
            return Err((
                "vkCreateGraphicsPipelines",
                vk::Result::ERROR_INITIALIZATION_FAILED,
            ));
        }
        self.create_pipeline(device, render_pass, layout, tally)
    }

    /// The graphics pipeline, with its two shader modules (destroyed again at once: a pipeline does not keep them).
    fn create_pipeline(
        &mut self,
        device: &ash::Device,
        render_pass: vk::RenderPass,
        layout: vk::PipelineLayout,
        tally: Tally,
    ) -> Result<(), Failure> {
        let vertex = module(device, VERTEX_SPIRV, tally)?;
        let fragment = match module(device, FRAGMENT_SPIRV, tally) {
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
        // Premultiplied alpha: the source is already multiplied by its alpha, for colour and for alpha.
        let attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
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
            .render_pass(render_pass)
            .subpass(0);
        tally.note();
        // SAFETY: the device is live; `info` and everything it points to are valid for the call.
        let created =
            unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) };
        // SAFETY: the pipeline copied what it needs from the module, or was never created.
        unsafe { device.destroy_shader_module(fragment, None) };
        // SAFETY: as above.
        unsafe { device.destroy_shader_module(vertex, None) };
        match created {
            Ok(pipelines) => {
                self.pipeline = pipelines.first().copied();
                Ok(())
            }
            Err((_, code)) => Err(("vkCreateGraphicsPipelines", code)),
        }
    }

    /// The sampler for `filter`. Panics if the pipeline was not created: the callers draw only after `create`.
    pub(crate) fn sampler(&self, filter: VulkanFilter) -> vk::Sampler {
        let sampler = match filter {
            VulkanFilter::Nearest => self.nearest,
            VulkanFilter::Linear => self.linear,
        };
        sampler.expect("the samplers exist once the pipeline was created")
    }

    /// Destroys what exists, in reverse order of creation. Only when nothing submitted uses any of it.
    pub(crate) fn destroy(&mut self, device: &ash::Device) {
        if let Some(pipeline) = self.pipeline.take() {
            // SAFETY: nothing submitted uses the pipeline any more.
            unsafe { device.destroy_pipeline(pipeline, None) };
        }
        if let Some(layout) = self.layout.take() {
            // SAFETY: the pipeline that used the layout is destroyed.
            unsafe { device.destroy_pipeline_layout(layout, None) };
        }
        if let Some(pool) = self.pool.take() {
            // SAFETY: every set from the pool is free or no longer used.
            unsafe { device.destroy_descriptor_pool(pool, None) };
        }
        if let Some(set_layout) = self.set_layout.take() {
            // SAFETY: the pipeline layout and the pool that used it are destroyed.
            unsafe { device.destroy_descriptor_set_layout(set_layout, None) };
        }
        for sampler in [self.linear.take(), self.nearest.take()]
            .into_iter()
            .flatten()
        {
            // SAFETY: no set that names the sampler is in use.
            unsafe { device.destroy_sampler(sampler, None) };
        }
    }
}

/// A quad's image, its memory, view, descriptor set and staging buffer. Fields are `None` until created.
#[derive(Default)]
pub(crate) struct QuadTexture {
    image: Option<vk::Image>,
    memory: Option<vk::DeviceMemory>,
    view: Option<vk::ImageView>,
    set: Option<vk::DescriptorSet>,
    staging: Option<vk::Buffer>,
    staging_memory: Option<vk::DeviceMemory>,
    width: u32,
    height: u32,
    /// The filter the descriptor set currently names.
    filter: Option<VulkanFilter>,
    /// A test's forced failures of the image's and the staging buffer's memory allocations.
    faults: QuadFaults,
    /// The texture's image exists (creation began: a partial creation counts).
    pub(crate) texture_began: bool,
    /// The staging buffer exists (creation began: a partial creation counts).
    pub(crate) staging_began: bool,
    /// The image's `VkMemoryRequirements::size`, once queried.
    pub(crate) texture_bytes: Option<u64>,
    /// The staging buffer's `VkMemoryRequirements::size`, once queried.
    pub(crate) staging_bytes: Option<u64>,
}

impl QuadTexture {
    /// A texture whose creation fails as a test asks: the image's memory allocation, or the staging buffer's.
    pub(crate) fn with_faults(faults: QuadFaults) -> Self {
        Self {
            faults,
            ..Self::default()
        }
    }

    /// Creates the image, its memory, view and descriptor set (bound to `quad`'s filter) and the staging
    /// buffer, and fills the staging buffer with `bytes`: the slice [`upload_bytes`] validated for this quad (the
    /// pixel source is not asked again). `quad` must have passed [`device_checks`].
    pub(crate) fn create(
        &mut self,
        device: &ash::Device,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        pipeline: &QuadPipeline,
        quad: &VulkanQuad,
        bytes: &[u8],
        tally: Tally,
    ) -> Result<(), Failure> {
        let (width, height) = (quad.width(), quad.height());
        self.width = width;
        self.height = height;
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
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        tally.note();
        // SAFETY: the device is live and `image_info` is valid for the call.
        let image = unsafe { device.create_image(&image_info, None) }
            .map_err(|code| ("vkCreateImage", code))?;
        self.image = Some(image);
        // Counted from the first handle that exists: a partial creation counts, a call that made nothing does not.
        self.texture_began = true;
        // SAFETY: the image belongs to the device.
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        self.texture_bytes = Some(requirements.size);
        if self.faults.texture_allocation {
            return Err(("vkAllocateMemory", vk::Result::ERROR_OUT_OF_DEVICE_MEMORY));
        }
        tally.note();
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
            .format(vk::Format::R8G8B8A8_UNORM)
            .subresource_range(color_range());
        tally.note();
        // SAFETY: the image is bound to memory and `view_info` is valid for the call.
        self.view = Some(
            unsafe { device.create_image_view(&view_info, None) }
                .map_err(|code| ("vkCreateImageView", code))?,
        );
        self.allocate_set(device, pipeline, quad.filter(), tally)?;
        self.create_staging(device, memory_properties, bytes, tally)
    }

    /// One descriptor set from the pool, naming the view and the sampler of `filter`.
    fn allocate_set(
        &mut self,
        device: &ash::Device,
        pipeline: &QuadPipeline,
        filter: VulkanFilter,
        tally: Tally,
    ) -> Result<(), Failure> {
        let (Some(pool), Some(set_layout), Some(view)) =
            (pipeline.pool, pipeline.set_layout, self.view)
        else {
            return Err((
                "vkAllocateDescriptorSets",
                vk::Result::ERROR_INITIALIZATION_FAILED,
            ));
        };
        let layouts = [set_layout];
        let info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool)
            .set_layouts(&layouts);
        tally.note();
        // SAFETY: the pool and the layout are live and `info` is valid for the call.
        let set = unsafe { device.allocate_descriptor_sets(&info) }
            .map_err(|code| ("vkAllocateDescriptorSets", code))?[0];
        self.set = Some(set);
        Self::write_set(device, pipeline, filter, view, set);
        self.filter = Some(filter);
        Ok(())
    }

    /// Points `set` at `view` and the sampler of `filter`. Only while no submitted frame uses the set.
    fn write_set(
        device: &ash::Device,
        pipeline: &QuadPipeline,
        filter: VulkanFilter,
        view: vk::ImageView,
        set: vk::DescriptorSet,
    ) {
        let images = [vk::DescriptorImageInfo::default()
            .sampler(pipeline.sampler(filter))
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&images);
        // SAFETY: the set is not in use by any submitted work (a fresh set, or the caller's rule), and `write`
        // names objects that are live.
        unsafe { device.update_descriptor_sets(&[write], &[]) };
    }

    /// The host-visible staging buffer at the payload size, filled with the source's bytes.
    fn create_staging(
        &mut self,
        device: &ash::Device,
        memory_properties: &vk::PhysicalDeviceMemoryProperties,
        bytes: &[u8],
        tally: Tally,
    ) -> Result<(), Failure> {
        let info = vk::BufferCreateInfo::default()
            .size(bytes.len() as u64)
            .usage(vk::BufferUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        tally.note();
        // SAFETY: the device is live and `info` is valid for the call.
        let buffer = unsafe { device.create_buffer(&info, None) }
            .map_err(|code| ("vkCreateBuffer", code))?;
        self.staging = Some(buffer);
        self.staging_began = true;
        // SAFETY: the buffer belongs to the device.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        self.staging_bytes = Some(requirements.size);
        if self.faults.staging_allocation {
            return Err(("vkAllocateMemory", vk::Result::ERROR_OUT_OF_DEVICE_MEMORY));
        }
        tally.note();
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
        // SAFETY: the memory is host visible and coherent, not mapped, and at least `bytes.len()` long.
        let mapped = unsafe {
            device.map_memory(memory, 0, bytes.len() as u64, vk::MemoryMapFlags::empty())
        }
        .map_err(|code| ("vkMapMemory", code))?;
        // SAFETY: `mapped` points at `bytes.len()` writable bytes that nothing else touches.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>(), bytes.len()) };
        // SAFETY: the memory was mapped just above.
        unsafe { device.unmap_memory(memory) };
        Ok(())
    }

    /// The filter the descriptor set currently names.
    pub(crate) fn filter(&self) -> Option<VulkanFilter> {
        self.filter
    }

    /// Whether any part of the staging buffer exists.
    pub(crate) fn has_staging(&self) -> bool {
        self.staging.is_some() || self.staging_memory.is_some()
    }

    /// Points the descriptor set at the sampler of `filter` (when the quad moves and refilters without a new
    /// image, nothing is uploaded). Only while no submitted frame uses the set.
    pub(crate) fn refilter(
        &mut self,
        device: &ash::Device,
        pipeline: &QuadPipeline,
        filter: VulkanFilter,
    ) {
        if let (Some(view), Some(set)) = (self.view, self.set) {
            Self::write_set(device, pipeline, filter, view, set);
            self.filter = Some(filter);
        }
    }

    /// Records the upload: a barrier to transfer destination, the buffer-to-image copy and a barrier to shader
    /// read. Before the render pass, in the same command buffer as the draw.
    pub(crate) fn record_upload(&self, device: &ash::Device, cmd: vk::CommandBuffer) {
        let (Some(image), Some(staging)) = (self.image, self.staging) else {
            return;
        };
        let to_destination = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(color_range());
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
        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width: self.width,
                height: self.height,
                depth: 1,
            });
        // SAFETY: the command buffer is recording; the image is in `TRANSFER_DST_OPTIMAL`, the buffer holds
        // the pixels.
        unsafe {
            device.cmd_copy_buffer_to_image(
                cmd,
                staging,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
        }
        let to_shader = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(color_range());
        // SAFETY: the command buffer is recording; the barrier names this device's image.
        unsafe {
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_shader],
            );
        }
    }

    /// Destroys the staging buffer and its memory: once the upload frame's submit fence has signalled.
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

    /// Destroys what exists: the staging buffer, the set (returned to `pool`), the view, the image and its memory.
    /// Only when no submitted frame uses any of it.
    pub(crate) fn destroy(&mut self, device: &ash::Device, pool: Option<vk::DescriptorPool>) {
        if let Some(buffer) = self.staging.take() {
            // SAFETY: nothing uses the buffer any more.
            unsafe { device.destroy_buffer(buffer, None) };
        }
        if let Some(memory) = self.staging_memory.take() {
            // SAFETY: the buffer bound to it is destroyed.
            unsafe { device.free_memory(memory, None) };
        }
        if let (Some(set), Some(pool)) = (self.set.take(), pool) {
            // SAFETY: no submitted frame uses the set. A failure to free it is not an error: the pool's own
            // destruction frees it.
            let _ = unsafe { device.free_descriptor_sets(pool, &[set]) };
        }
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

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

/// The bytes to upload for `quad`, taken from its pixel source **once** and checked against the quad's own image
/// size: exactly `width × height × 4`. `PixelSource` is a safe trait, and a safe implementation may answer
/// differently on each call, so the answer [`VulkanQuad::new`] saw proves nothing about this one. Only this
/// borrow is used for the staging buffer's size and its contents, and the copy region records the quad's
/// dimensions, so the buffer always covers every location the copy addresses. A mismatch is a named error, and
/// nothing has been created yet.
pub(crate) fn upload_bytes(quad: &VulkanQuad) -> Result<&[u8], Error> {
    let bytes = quad.pixels().pixels();
    let expected = u64::from(quad.width())
        .checked_mul(u64::from(quad.height()))
        .and_then(|pixels| pixels.checked_mul(4));
    if expected.and_then(|expected| usize::try_from(expected).ok()) == Some(bytes.len()) {
        Ok(bytes)
    } else {
        Err(Error(format!(
            "the pixel source returned {} bytes at upload, but the quad's {} x {} image needs {} (a source must answer the same on every call)",
            bytes.len(),
            quad.width(),
            quad.height(),
            expected.map_or_else(
                || "more than fits".to_owned(),
                |expected| expected.to_string()
            )
        )))
    }
}

/// The two rectangles the vertex shader takes, as eight `f32`: the destination in normalised device
/// coordinates of a `target_width` × `target_height` target (its top-left pixel is `(-1, -1)`) and the source
/// rectangle over the image size.
#[expect(
    clippy::cast_precision_loss,
    reason = "pixel coordinates and sizes are far below 2^24, which f32 holds exactly"
)]
fn rectangles(quad: &QuadParams, target_width: u32, target_height: u32) -> [f32; 8] {
    let (width, height) = (target_width as f32, target_height as f32);
    let destination = quad.destination;
    let source = quad.source;
    let (image_width, image_height) = (quad.width as f32, quad.height as f32);
    [
        2.0 * destination.left as f32 / width - 1.0,
        2.0 * destination.top as f32 / height - 1.0,
        2.0 * destination.right as f32 / width - 1.0,
        2.0 * destination.bottom as f32 / height - 1.0,
        source.left / image_width,
        source.top / image_height,
        source.right / image_width,
        source.bottom / image_height,
    ]
}

/// Records the draw, inside the render pass and after its clear: dynamic viewport and scissor over the whole
/// target, the pipeline, the texture's descriptor set, the push constants and six vertices.
pub(crate) fn record_draw(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    pipeline: &QuadPipeline,
    texture: &QuadTexture,
    quad: &QuadParams,
    target: (u32, u32),
) {
    let (Some(graphics), Some(layout), Some(set)) =
        (pipeline.pipeline, pipeline.layout, texture.set)
    else {
        return;
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "a target side is at most 8192, which f32 holds exactly"
    )]
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: target.0 as f32,
        height: target.1 as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    let scissor = vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent: vk::Extent2D {
            width: target.0,
            height: target.1,
        },
    };
    let constants = rectangles(quad, target.0, target.1);
    let mut bytes = [0_u8; PUSH_BYTES as usize];
    for (chunk, value) in bytes.chunks_exact_mut(4).zip(constants) {
        chunk.copy_from_slice(&value.to_ne_bytes());
    }
    // SAFETY: the command buffer is recording inside a render pass the pipeline is compatible with.
    unsafe { device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, graphics) };
    // SAFETY: as above; the pipeline declares dynamic viewport and scissor.
    unsafe { device.cmd_set_viewport(cmd, 0, &[viewport]) };
    // SAFETY: as above.
    unsafe { device.cmd_set_scissor(cmd, 0, &[scissor]) };
    // SAFETY: the set was allocated from the pool of this pipeline's layout and written.
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
    // SAFETY: the push constant range of the layout covers these 32 bytes for the vertex stage.
    unsafe {
        device.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::VERTEX, 0, &bytes);
    }
    // SAFETY: the pipeline, descriptor set and push constants are bound.
    unsafe { device.cmd_draw(cmd, 6, 1, 0, 0) };
}

/// The device's checks for `quad` (design §7.2.1), run **before any object is created for it**: the image fits
/// `max_dimension` (the device's `maxImageDimension2D`, or the limit a test hook lowers it to), and
/// `R8G8B8A8_UNORM` supports sampling and transfer destination with optimal tiling (and linear filtering when
/// the quad asks for it). An error names the image's size and the limit, or the missing capability.
pub(crate) fn device_checks(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    quad: &VulkanQuad,
    hooks: ReadbackHooks,
) -> Result<(), Error> {
    let max_dimension = hooks.max_image_dimension;
    // SAFETY: `physical` came from this instance.
    let properties = unsafe { instance.get_physical_device_properties(physical) };
    let limit = max_dimension.unwrap_or(properties.limits.max_image_dimension2_d);
    if quad.width() > limit || quad.height() > limit {
        return Err(Error(format!(
            "the quad's image is {} x {} pixels, over this device's maxImageDimension2D of {limit}",
            quad.width(),
            quad.height()
        )));
    }
    // SAFETY: `physical` came from this instance.
    let device_features = unsafe {
        instance
            .get_physical_device_format_properties(physical, vk::Format::R8G8B8A8_UNORM)
            .optimal_tiling_features
    };
    let features = hooks.source_features.unwrap_or(device_features);
    let mut wanted = vk::FormatFeatureFlags::SAMPLED_IMAGE | vk::FormatFeatureFlags::TRANSFER_DST;
    if quad.filter() == VulkanFilter::Linear {
        wanted |= vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR;
    }
    if !features.contains(wanted) {
        let missing = wanted & !features;
        return Err(Error(format!(
            "this device's R8G8B8A8_UNORM does not support {missing:?} with optimal tiling, which the quad needs"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::quad::{DestinationRect, PixelSource, SourceRect};

    struct Bytes(Vec<u8>);

    impl PixelSource for Bytes {
        fn pixels(&self) -> &[u8] {
            &self.0
        }
    }

    /// Every value within a hair of its expectation (these are exact in `f32`; the tolerance only satisfies the
    /// float-comparison lint).
    fn assert_close(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len());
        for (got, wanted) in actual.iter().zip(expected) {
            assert!(
                (got - wanted).abs() < 1e-6,
                "{actual:?} against {expected:?}"
            );
        }
    }

    fn quad(destination: DestinationRect, source: SourceRect) -> VulkanQuad {
        VulkanQuad::new(
            Arc::new(Bytes(vec![0; 4 * 2 * 2])),
            2,
            2,
            destination,
            source,
            VulkanFilter::Nearest,
        )
        .expect("a valid quad")
    }

    #[test]
    fn the_push_constants_are_the_destination_in_ndc_and_the_source_over_the_image_size() {
        let quad = quad(
            DestinationRect {
                left: 0,
                top: 0,
                right: 8,
                bottom: 4,
            },
            SourceRect {
                left: 0.0,
                top: 0.0,
                right: 2.0,
                bottom: 1.0,
            },
        );
        // An 8 x 4 target, the whole of it: from (-1, -1) at the top left to (1, 1) at the bottom right.
        assert_close(
            &rectangles(&QuadParams::of(&quad), 8, 4),
            &[-1.0, -1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.5],
        );
        // The same quad drawn into the right half of a 16 x 4 target.
        let moved = VulkanQuad::new(
            Arc::clone(quad.pixels()),
            2,
            2,
            DestinationRect {
                left: 8,
                top: 0,
                right: 16,
                bottom: 4,
            },
            quad.source(),
            VulkanFilter::Linear,
        )
        .expect("a valid quad");
        assert_close(
            &rectangles(&QuadParams::of(&moved), 16, 4)[..4],
            &[0.0, -1.0, 1.0, 1.0],
        );
    }
}
