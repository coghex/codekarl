//! Drawing a scene frame: the instanced pipeline, the instance buffer and the recording of one frame's render pass
//! (milestone 5.5, issue 2a). The swap-chain frames of the presenter and the off-screen readback record through the
//! same functions, so what a test reads back is what the window draws.
//!
//! - **The pipeline** has one instance-rate vertex binding (a 32-byte [`InstanceRecord`]: destination and source) and
//!   draws six vertices of a triangle list (two triangles) per instance: a strip would be four, but `MoltenVK` warns that
//!   Metal cannot disable primitive restart for one, and the log must stay empty. `firstInstance` selects a batch's first instance, which core
//!   Vulkan allows on direct draws. The pipeline layout is the texture ledger's (set 0 a sampler, set 1 a revision's
//!   image) with the drawable size as a push constant. Blending is the legacy quad's: premultiplied, no conversion.
//! - **The instance buffer** is one host-visible buffer for `max_quads` instances, allocated through the context (so
//!   it counts against `maxMemoryAllocationCount`) and charged to `binding_geometry_bytes` at its queried allocation
//!   size. One buffer is enough because at most one frame is in flight: the presenter checks that the previous
//!   submission is proven before it writes.
//! - **Samplers.** A batch's effective sampler is the nearest set for the checker and the frame filter's set
//!   otherwise. Set 0 is bound before the first batch and **rebound whenever the effective sampler differs from the
//!   previous batch's**, in both directions. Rebinding writes no descriptor.

use std::sync::Arc;

use ash::vk;

use crate::batch::{Batch, INSTANCE_STRIDE};
use crate::context::VulkanContext;
use crate::descriptors::DRAWABLE_PUSH_BYTES;
use crate::quad::VulkanFilter;
use crate::{Error, error::vk_error};

const VERTEX_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/scene.vert.spv"));
const FRAGMENT_SPIRV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/scene.frag.spv"));

fn module(device: &ash::Device, bytes: &[u8]) -> Result<vk::ShaderModule, Error> {
    // A SPIR-V binary is a sequence of little-endian words; the bytes `include_bytes` gives are not aligned for `u32`.
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect();
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    // SAFETY: the device is live and `info` (with the code it points to) is valid for the call.
    unsafe { device.create_shader_module(&info, None) }
        .map_err(|code| vk_error("vkCreateShaderModule", code))
}

/// The instanced scene pipeline, made against one render pass and the ledger's pipeline layout.
pub(crate) struct ScenePipeline {
    pipeline: vk::Pipeline,
}

impl ScenePipeline {
    /// Creates the pipeline for `render_pass`.
    pub(crate) fn create(
        device: &ash::Device,
        render_pass: vk::RenderPass,
        layout: vk::PipelineLayout,
    ) -> Result<Self, Error> {
        let vertex = module(device, VERTEX_SPIRV)?;
        let fragment = match module(device, FRAGMENT_SPIRV) {
            Ok(fragment) => fragment,
            Err(error) => {
                // SAFETY: the module was just created and nothing uses it.
                unsafe { device.destroy_shader_module(vertex, None) };
                return Err(error);
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
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(u32::try_from(INSTANCE_STRIDE).unwrap_or(32))
            .input_rate(vk::VertexInputRate::INSTANCE)];
        let attributes = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .binding(0)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .binding(0)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(16),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attributes);
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
        // SAFETY: the device is live; `info` and everything it points to are valid for the call.
        let created =
            unsafe { device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) };
        // SAFETY: the pipeline copied what it needs from the modules, or was never created.
        unsafe { device.destroy_shader_module(fragment, None) };
        // SAFETY: as above.
        unsafe { device.destroy_shader_module(vertex, None) };
        match created {
            Ok(pipelines) => Ok(Self {
                pipeline: pipelines[0],
            }),
            Err((_, code)) => Err(vk_error("vkCreateGraphicsPipelines", code)),
        }
    }

    /// Destroys the pipeline. Only when nothing submitted uses it.
    pub(crate) fn destroy(&mut self, device: &ash::Device) {
        // SAFETY: the caller's contract: nothing submitted uses it; a null handle is a no-op.
        unsafe { device.destroy_pipeline(self.pipeline, None) };
        self.pipeline = vk::Pipeline::null();
    }
}

/// A host-visible buffer for the instances of one frame, persistently mapped.
pub(crate) struct InstanceBuffer {
    context: Arc<VulkanContext>,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    ptr: *mut u8,
    /// Instances it holds.
    pub(crate) capacity: usize,
    /// The bytes the driver allocated: what is charged to `binding_geometry_bytes`.
    pub(crate) allocated: u64,
    coherent: bool,
}

// SAFETY: the pointer maps device memory that only the render thread writes, and only while the GPU does not read
// the buffer (the presenter checks the previous frame's proof first); the handles are plain values.
unsafe impl Send for InstanceBuffer {}

impl InstanceBuffer {
    /// Creates the buffer for `capacity` instances. The bytes are checked arithmetic; a buffer whose queried
    /// allocation does not fit `budget` is refused, naming both numbers, **before** the memory is allocated.
    pub(crate) fn create(
        context: &Arc<VulkanContext>,
        capacity: usize,
        budget: u64,
    ) -> Result<Self, Error> {
        let bytes = capacity
            .checked_mul(INSTANCE_STRIDE)
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or_else(|| Error("the instance buffer's byte count overflows".to_owned()))?;
        let device = context.device();
        let info = vk::BufferCreateInfo::default()
            .size(bytes.max(1))
            .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the device is live and `info` is valid for the call.
        let buffer = unsafe { device.create_buffer(&info, None) }
            .map_err(|code| vk_error("vkCreateBuffer(instances)", code))?;
        // SAFETY: the buffer was just created on this device.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let destroy = |buffer| {
            // SAFETY: the buffer is unbound and unused.
            unsafe { device.destroy_buffer(buffer, None) };
        };
        if requirements.size > budget {
            destroy(buffer);
            return Err(Error(format!(
                "the instance buffer for {capacity} quads takes {} bytes, over the {budget} bytes of binding_geometry_bytes",
                requirements.size
            )));
        }
        let Some(memory_type) = context.memory_type(
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE,
            vk::MemoryPropertyFlags::HOST_COHERENT,
        ) else {
            destroy(buffer);
            return Err(Error(
                "no host-visible memory type for the instance buffer".to_owned(),
            ));
        };
        let memory = match context.allocate_memory(requirements.size, memory_type, None) {
            Ok(memory) => memory,
            Err(error) => {
                destroy(buffer);
                return Err(Error(format!(
                    "the instance buffer could not be allocated: {error:?}"
                )));
            }
        };
        // SAFETY: the memory is unbound and large enough for the buffer.
        if let Err(code) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
            destroy(buffer);
            context.free_memory(memory);
            return Err(vk_error("vkBindBufferMemory(instances)", code));
        }
        // SAFETY: the memory is host-visible and not mapped.
        let mapped =
            unsafe { device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) };
        let ptr = match mapped {
            Ok(ptr) => ptr.cast::<u8>(),
            Err(code) => {
                destroy(buffer);
                context.free_memory(memory);
                return Err(vk_error("vkMapMemory(instances)", code));
            }
        };
        Ok(Self {
            context: Arc::clone(context),
            buffer,
            memory,
            ptr,
            capacity,
            allocated: requirements.size,
            coherent: context.is_coherent(memory_type),
        })
    }

    /// The buffer handle, for the vertex binding.
    pub(crate) fn handle(&self) -> vk::Buffer {
        self.buffer
    }

    /// Writes instance `slot`. The caller guarantees the GPU is not reading the buffer.
    pub(crate) fn write(&self, slot: usize, record: crate::batch::InstanceRecord) {
        if slot >= self.capacity {
            return;
        }
        let bytes = record.to_bytes();
        // SAFETY: `slot` is below the capacity the allocation was made for, so the offset stays inside it.
        let target = unsafe { self.ptr.add(slot * INSTANCE_STRIDE) };
        // SAFETY: the whole record lies inside the mapped allocation (`slot < capacity`), the source is a local array,
        // and the range is written by this thread only while the GPU does not read the buffer.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, INSTANCE_STRIDE) };
    }

    /// Makes the first `count` instances visible to the device when the memory is not host-coherent.
    ///
    /// # Errors
    ///
    /// The Vulkan failure.
    pub(crate) fn flush(&self, count: usize) -> Result<(), Error> {
        if self.coherent || count == 0 {
            return Ok(());
        }
        let atom = self.context.limits().non_coherent_atom_size;
        let bytes = u64::try_from(count.saturating_mul(INSTANCE_STRIDE)).unwrap_or(u64::MAX);
        let size = crate::memory::align_up(bytes, atom)
            .unwrap_or(vk::WHOLE_SIZE)
            .min(self.allocated);
        let range = vk::MappedMemoryRange::default()
            .memory(self.memory)
            .offset(0)
            .size(if size >= self.allocated {
                vk::WHOLE_SIZE
            } else {
                size
            });
        // SAFETY: the memory is mapped by this buffer; the range starts at zero and its size is a multiple of the
        // atom size or reaches the end of the allocation.
        unsafe { self.context.device().flush_mapped_memory_ranges(&[range]) }
            .map_err(|code| vk_error("vkFlushMappedMemoryRanges(instances)", code))
    }

    /// Destroys the buffer and frees its memory. Only when nothing submitted reads it.
    pub(crate) fn destroy(&mut self) {
        let device = self.context.device();
        // SAFETY: the memory is mapped by this buffer and nothing submitted uses the buffer (the caller's contract).
        unsafe { device.unmap_memory(self.memory) };
        // SAFETY: as above.
        unsafe { device.destroy_buffer(self.buffer, None) };
        self.context.free_memory(self.memory);
        self.buffer = vk::Buffer::null();
    }
}

/// What a frame draws: the batches and the sets they bind.
pub(crate) struct SceneDraw<'a> {
    pub(crate) pipeline: &'a ScenePipeline,
    pub(crate) layout: vk::PipelineLayout,
    pub(crate) instances: vk::Buffer,
    pub(crate) batches: &'a [Batch],
    /// The checker's permanent set (set 1).
    pub(crate) checker: vk::DescriptorSet,
    /// The sampler sets: nearest, then linear (set 0).
    pub(crate) samplers: [vk::DescriptorSet; 2],
    /// The frame's filter, for every batch but the checker's.
    pub(crate) filter: VulkanFilter,
}

fn sampler_index(filter: VulkanFilter) -> usize {
    match filter {
        VulkanFilter::Nearest => 0,
        VulkanFilter::Linear => 1,
    }
}

/// The sampler each batch binds, in order, and whether set 0 is rebound before it: the first batch binds, and every
/// batch whose effective sampler differs from the previous batch's rebinds, in both directions.
pub(crate) fn sampler_plan(batches: &[Batch], filter: VulkanFilter) -> Vec<(usize, bool)> {
    let mut previous = None;
    batches
        .iter()
        .map(|batch| {
            let effective = if batch.is_checker() {
                0
            } else {
                sampler_index(filter)
            };
            let rebind = previous != Some(effective);
            previous = Some(effective);
            (effective, rebind)
        })
        .collect()
}

/// Records the draws inside a begun render pass the pipeline is compatible with: the dynamic viewport and scissor over
/// the target, the pipeline, the instance buffer, the drawable size, and for each batch the sampler (set 0, rebound
/// when it changes), the batch's set (set 1) and one instanced draw.
pub(crate) fn record_scene(
    device: &ash::Device,
    cmd: vk::CommandBuffer,
    draw: &SceneDraw<'_>,
    target: (u32, u32),
) {
    if draw.batches.is_empty() {
        return;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a target side is at most 8192, which f32 holds exactly"
    )]
    let (width, height) = (target.0 as f32, target.1 as f32);
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width,
        height,
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
    let mut constants = [0_u8; DRAWABLE_PUSH_BYTES as usize];
    constants[..4].copy_from_slice(&width.to_ne_bytes());
    constants[4..].copy_from_slice(&height.to_ne_bytes());
    // SAFETY: the command buffer is recording inside a render pass the pipeline is compatible with.
    unsafe {
        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, draw.pipeline.pipeline);
    }
    // SAFETY: as above; the pipeline declares dynamic viewport and scissor.
    unsafe { device.cmd_set_viewport(cmd, 0, &[viewport]) };
    // SAFETY: as above.
    unsafe { device.cmd_set_scissor(cmd, 0, &[scissor]) };
    // SAFETY: the buffer holds the instances (written, and visible to the device) and is a vertex buffer.
    unsafe { device.cmd_bind_vertex_buffers(cmd, 0, &[draw.instances], &[0]) };
    // SAFETY: the layout's push constant range covers these bytes for the vertex stage.
    unsafe {
        device.cmd_push_constants(
            cmd,
            draw.layout,
            vk::ShaderStageFlags::VERTEX,
            0,
            &constants,
        );
    }
    for (batch, (sampler, rebind)) in draw
        .batches
        .iter()
        .zip(sampler_plan(draw.batches, draw.filter))
    {
        if rebind {
            // SAFETY: the sampler sets were written at the ledger's start and are never written again.
            unsafe {
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    draw.layout,
                    0,
                    &[draw.samplers[sampler]],
                    &[],
                );
            }
        }
        let set = if batch.is_checker() {
            draw.checker
        } else {
            batch.set
        };
        // SAFETY: the set is a written set of the layout's second set layout, kept alive by the frame's ticket (a
        // revision's) or permanent (the checker's).
        unsafe {
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                draw.layout,
                1,
                &[set],
                &[],
            );
        }
        // SAFETY: the pipeline, the vertex buffer, the push constants and both sets are bound.
        unsafe { device.cmd_draw(cmd, 6, batch.count, 0, batch.first) };
    }
}

#[cfg(test)]
mod tests {
    use ash::vk;

    use super::{VulkanFilter, sampler_plan};
    use crate::batch::Batch;

    fn batch(set: u64, first: u32) -> Batch {
        use ash::vk::Handle as _;
        Batch {
            set: vk::DescriptorSet::from_raw(set),
            first,
            count: 1,
        }
    }

    #[test]
    fn the_sampler_is_bound_first_and_rebound_whenever_it_changes_in_both_directions() {
        // A resident, the checker (set null), a resident, a resident, the checker: in a linear frame the effective
        // samplers are linear, nearest, linear, linear, nearest.
        let batches = [
            batch(10, 0),
            batch(0, 1),
            batch(11, 2),
            batch(12, 3),
            batch(0, 4),
        ];
        let plan = sampler_plan(&batches, VulkanFilter::Linear);
        assert_eq!(
            plan,
            vec![(1, true), (0, true), (1, true), (1, false), (0, true)],
            "bound first; nearest after linear; linear again after the checker (the switch back); unchanged between residents"
        );
        // In a nearest frame the checker's sampler is the frame's, so nothing is ever rebound.
        let plan = sampler_plan(&batches, VulkanFilter::Nearest);
        assert_eq!(
            plan.iter().filter(|(_, rebind)| *rebind).count(),
            1,
            "{plan:?}"
        );
        assert!(plan.iter().all(|(sampler, _)| *sampler == 0));
        assert!(sampler_plan(&[], VulkanFilter::Linear).is_empty());
    }

    /// Negative control: a plan that forgets the switch back to the frame's filter after the checker draws the
    /// resident batch after it with nearest.
    #[test]
    fn negative_control_omitting_the_switch_back_after_the_checker_samples_the_resident_with_nearest()
     {
        let batches = [batch(10, 0), batch(0, 1), batch(11, 2)];
        let correct = sampler_plan(&batches, VulkanFilter::Linear);
        // The faulty plan rebinds only on a change *to nearest*.
        let mut faulty = Vec::new();
        let mut current = None;
        for (sampler, _) in &correct {
            let rebind = *sampler == 0 && current != Some(0) || current.is_none();
            if rebind {
                current = Some(*sampler);
            }
            faulty.push(current.expect("bound"));
        }
        let effective: Vec<usize> = correct.iter().map(|(sampler, _)| *sampler).collect();
        assert_eq!(effective, vec![1, 0, 1]);
        assert_ne!(
            faulty, effective,
            "the faulty plan keeps nearest for the last resident"
        );
    }
}
