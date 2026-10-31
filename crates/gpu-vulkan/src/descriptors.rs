//! The ordinary stage's descriptors: one set per native revision, recycled after its proven release
//! (`runtime-m55/specs/m55-descriptor-choice.md`).
//!
//! - **Layouts.** Set 0 has one `SAMPLER` binding and set 1 one `SAMPLED_IMAGE` binding, both for the fragment
//!   stage; the pipeline layout names the two. Core Vulkan 1.0: no extension and no feature bit.
//! - **Sampler sets.** Two sets, nearest and linear, written once at start. A filter change rewrites nothing.
//! - **Revision sets.** Each revision owns one set-1 set naming its image view in `SHADER_READ_ONLY_OPTIMAL`.
//!   The pool has a fixed size and **no free flag**: sets are allocated lazily up to the cap, never freed one by
//!   one, and recycled through a free list, so there is no fragmentation and no reliance on free-path
//!   guarantees. A set goes back on the list only when its revision is released, and a set is never written while
//!   a recorded or submitted command buffer may still bind it.
//! - **The limit.** `ordinary_sets = max_textures + max_extra_revisions + 1`, checked. A pool the driver refuses
//!   to create is a start failure naming the counts; capacity is never lowered silently. A free list that is
//!   empty with the count at the cap is a refusal for that revision, naming in-use and configured counts.

use std::sync::Arc;

use ash::vk;

use crate::Error;
use crate::context::VulkanContext;
use crate::error::vk_error;

/// The push constants of the ordinary stage's pipeline layout: the drawable's width and height as `f32`.
pub(crate) const DRAWABLE_PUSH_BYTES: u32 = 8;

/// The books of a pool that allocates up to a cap and recycles: pure, so its rules are tested without a device.
#[derive(Debug)]
pub(crate) struct SetBook<S: Copy> {
    free: Vec<S>,
    allocated: usize,
    in_use: usize,
    cap: usize,
}

impl<S: Copy> SetBook<S> {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            free: Vec::new(),
            allocated: 0,
            in_use: 0,
            cap,
        }
    }

    /// A recycled set, if one is free.
    pub(crate) fn take_free(&mut self) -> Option<S> {
        let set = self.free.pop()?;
        self.in_use += 1;
        Some(set)
    }

    /// Whether the pool may allocate one more set.
    pub(crate) fn may_allocate(&self) -> bool {
        self.allocated < self.cap
    }

    /// A newly allocated set is now in use.
    pub(crate) fn allocated_one(&mut self) {
        self.allocated += 1;
        self.in_use += 1;
    }

    /// A released revision's set goes back on the free list.
    pub(crate) fn recycle(&mut self, set: S) {
        self.in_use = self.in_use.saturating_sub(1);
        self.free.push(set);
    }

    /// Why no set is available.
    pub(crate) fn exhausted(&self) -> String {
        format!(
            "descriptor sets exhausted ({} in use, {} configured)",
            self.in_use, self.cap
        )
    }

    pub(crate) fn cap(&self) -> usize {
        self.cap
    }

    pub(crate) fn in_use(&self) -> usize {
        self.in_use
    }

    #[cfg(test)]
    pub(crate) fn free_count(&self) -> usize {
        self.free.len()
    }
}

/// The descriptor objects of one ledger.
pub(crate) struct Descriptors {
    context: Arc<VulkanContext>,
    sampler_layout: vk::DescriptorSetLayout,
    image_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    sampler_pool: vk::DescriptorPool,
    image_pool: vk::DescriptorPool,
    samplers: [vk::Sampler; 2],
    sampler_sets: [vk::DescriptorSet; 2],
    book: SetBook<vk::DescriptorSet>,
}

impl Descriptors {
    /// Creates the layouts, the pipeline layout, the samplers, both pools and the two sampler sets (written),
    /// or destroys what exists and says which step failed, naming the counts where they matter.
    pub(crate) fn create(
        context: &Arc<VulkanContext>,
        ordinary_sets: usize,
    ) -> Result<Self, Error> {
        let mut this = Self {
            context: Arc::clone(context),
            sampler_layout: vk::DescriptorSetLayout::null(),
            image_layout: vk::DescriptorSetLayout::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            sampler_pool: vk::DescriptorPool::null(),
            image_pool: vk::DescriptorPool::null(),
            samplers: [vk::Sampler::null(); 2],
            sampler_sets: [vk::DescriptorSet::null(); 2],
            book: SetBook::new(ordinary_sets),
        };
        match this.build(ordinary_sets) {
            Ok(()) => Ok(this),
            Err(error) => {
                this.destroy();
                Err(error)
            }
        }
    }

    fn build(&mut self, ordinary_sets: usize) -> Result<(), Error> {
        let device = self.context.device();
        let sets = u32::try_from(ordinary_sets).map_err(|_| {
            Error(format!(
                "{ordinary_sets} descriptor sets do not fit a descriptor pool"
            ))
        })?;
        let layout = |descriptor_type| {
            let binding = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(descriptor_type)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&binding);
            // SAFETY: the device is live and `info` (with its binding) is valid for the call.
            unsafe { device.create_descriptor_set_layout(&info, None) }
                .map_err(|code| vk_error("vkCreateDescriptorSetLayout", code))
        };
        self.sampler_layout = layout(vk::DescriptorType::SAMPLER)?;
        self.image_layout = layout(vk::DescriptorType::SAMPLED_IMAGE)?;
        let layouts = [self.sampler_layout, self.image_layout];
        // The ordinary stage's vertex shader takes the drawable size (two floats) as a push constant.
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(DRAWABLE_PUSH_BYTES)];
        let info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&layouts)
            .push_constant_ranges(&ranges);
        // SAFETY: the device is live and the layouts exist.
        self.pipeline_layout = unsafe { device.create_pipeline_layout(&info, None) }
            .map_err(|code| vk_error("vkCreatePipelineLayout", code))?;
        for (index, filter) in [vk::Filter::NEAREST, vk::Filter::LINEAR]
            .into_iter()
            .enumerate()
        {
            let info = vk::SamplerCreateInfo::default()
                .mag_filter(filter)
                .min_filter(filter)
                .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
            // SAFETY: the device is live and `info` is valid for the call.
            self.samplers[index] = unsafe { device.create_sampler(&info, None) }
                .map_err(|code| vk_error("vkCreateSampler", code))?;
        }
        let pool = |count: u32, descriptor_type| {
            let sizes = [vk::DescriptorPoolSize::default()
                .ty(descriptor_type)
                .descriptor_count(count)];
            let info = vk::DescriptorPoolCreateInfo::default()
                .max_sets(count)
                .pool_sizes(&sizes);
            // SAFETY: the device is live and `info` (with its sizes) is valid for the call. No free flag:
            // sets are never freed one by one.
            unsafe { device.create_descriptor_pool(&info, None) }.map_err(|code| {
                Error(format!(
                    "the descriptor pool for {count} {descriptor_type:?} sets was refused: {code:?}"
                ))
            })
        };
        self.sampler_pool = pool(2, vk::DescriptorType::SAMPLER)?;
        // One set more than the ordinary revisions can ever need: the checker's permanent one.
        self.image_pool = pool(
            sets.checked_add(1).ok_or_else(|| {
                Error(format!(
                    "{ordinary_sets} descriptor sets and the checker's do not fit a pool"
                ))
            })?,
            vk::DescriptorType::SAMPLED_IMAGE,
        )?;
        let both = [self.sampler_layout; 2];
        let allocate = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.sampler_pool)
            .set_layouts(&both);
        // SAFETY: the pool has room for exactly these two sets of this layout.
        let allocated = unsafe { device.allocate_descriptor_sets(&allocate) }
            .map_err(|code| vk_error("vkAllocateDescriptorSets(samplers)", code))?;
        self.sampler_sets = [allocated[0], allocated[1]];
        let infos = [
            vk::DescriptorImageInfo::default().sampler(self.samplers[0]),
            vk::DescriptorImageInfo::default().sampler(self.samplers[1]),
        ];
        let writes = [0, 1].map(|index| {
            vk::WriteDescriptorSet::default()
                .dst_set(self.sampler_sets[index])
                .dst_binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .image_info(std::slice::from_ref(&infos[index]))
        });
        // SAFETY: the sets are unused and the samplers exist; the writes are valid for the call.
        unsafe { device.update_descriptor_sets(&writes, &[]) };
        Ok(())
    }

    pub(crate) fn pipeline_layout(&self) -> vk::PipelineLayout {
        self.pipeline_layout
    }

    pub(crate) fn sampler_sets(&self) -> [vk::DescriptorSet; 2] {
        self.sampler_sets
    }

    /// A set for a revision: a recycled one, else a new one up to the cap.
    ///
    /// # Errors
    ///
    /// The reason, naming in-use and configured counts, when none is available.
    pub(crate) fn acquire(&mut self) -> Result<vk::DescriptorSet, String> {
        if let Some(set) = self.book.take_free() {
            return Ok(set);
        }
        if !self.book.may_allocate() {
            return Err(self.book.exhausted());
        }
        let layouts = [self.image_layout];
        let info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.image_pool)
            .set_layouts(&layouts);
        // SAFETY: the pool has room (the book counts allocations against its `maxSets`).
        match unsafe { self.context.device().allocate_descriptor_sets(&info) } {
            Ok(sets) => {
                self.book.allocated_one();
                Ok(sets[0])
            }
            Err(code) => Err(format!(
                "vkAllocateDescriptorSets failed: {code:?} ({} in use, {} configured)",
                self.book.in_use(),
                self.book.cap()
            )),
        }
    }

    /// Writes each set to name its view in `SHADER_READ_ONLY_OPTIMAL`, in one call. The caller guarantees no
    /// recorded or submitted command buffer binds any of these sets.
    pub(crate) fn write(&self, writes: &[(vk::DescriptorSet, vk::ImageView)]) {
        if writes.is_empty() {
            return;
        }
        let infos: Vec<vk::DescriptorImageInfo> = writes
            .iter()
            .map(|&(_, view)| {
                vk::DescriptorImageInfo::default()
                    .image_view(view)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            })
            .collect();
        let updates: Vec<vk::WriteDescriptorSet<'_>> = writes
            .iter()
            .zip(&infos)
            .map(|(&(set, _), info)| {
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(std::slice::from_ref(info))
            })
            .collect();
        // SAFETY: the sets belong to this pool, the views exist, and (the caller's contract) no command buffer
        // that binds these sets is recorded or pending.
        unsafe { self.context.device().update_descriptor_sets(&updates, &[]) };
    }

    /// The checker's permanent set: the pool's one set beyond the ordinary revisions', written to name `view`. It is
    /// never recycled.
    pub(crate) fn checker_set(&mut self, view: vk::ImageView) -> Result<vk::DescriptorSet, String> {
        let layouts = [self.image_layout];
        let info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.image_pool)
            .set_layouts(&layouts);
        // SAFETY: the pool has room for this one more set (its `maxSets` is the ordinary count plus one).
        let set = unsafe { self.context.device().allocate_descriptor_sets(&info) }
            .map_err(|code| format!("vkAllocateDescriptorSets failed: {code:?}"))?[0];
        self.write(&[(set, view)]);
        Ok(set)
    }

    /// A released revision's set returns to the free list.
    pub(crate) fn recycle(&mut self, set: vk::DescriptorSet) {
        self.book.recycle(set);
    }

    /// Destroys every object, pools first (their sets go with them). Only after the ledger's proven end, or
    /// when nothing was ever recorded.
    pub(crate) fn destroy(&mut self) {
        let device = self.context.device();
        // SAFETY: nothing recorded or pending uses the pools, samplers and layouts (the caller's contract); null
        // handles are a no-op.
        unsafe { device.destroy_descriptor_pool(self.image_pool, None) };
        // SAFETY: as above.
        unsafe { device.destroy_descriptor_pool(self.sampler_pool, None) };
        for sampler in self.samplers {
            // SAFETY: as above.
            unsafe { device.destroy_sampler(sampler, None) };
        }
        // SAFETY: as above.
        unsafe { device.destroy_pipeline_layout(self.pipeline_layout, None) };
        // SAFETY: as above.
        unsafe { device.destroy_descriptor_set_layout(self.image_layout, None) };
        // SAFETY: as above.
        unsafe { device.destroy_descriptor_set_layout(self.sampler_layout, None) };
        *self = Self {
            context: Arc::clone(&self.context),
            sampler_layout: vk::DescriptorSetLayout::null(),
            image_layout: vk::DescriptorSetLayout::null(),
            pipeline_layout: vk::PipelineLayout::null(),
            sampler_pool: vk::DescriptorPool::null(),
            image_pool: vk::DescriptorPool::null(),
            samplers: [vk::Sampler::null(); 2],
            sampler_sets: [vk::DescriptorSet::null(); 2],
            book: SetBook::new(0),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::SetBook;

    #[test]
    fn sets_are_allocated_up_to_the_cap_and_recycled_after_release() {
        let mut book = SetBook::<u32>::new(2);
        assert_eq!(book.take_free(), None, "nothing to recycle yet");
        assert!(book.may_allocate());
        book.allocated_one();
        book.allocated_one();
        assert!(!book.may_allocate(), "the cap is reached");
        assert_eq!(book.in_use(), 2);
        assert_eq!(
            book.exhausted(),
            "descriptor sets exhausted (2 in use, 2 configured)"
        );
        book.recycle(7);
        assert_eq!((book.in_use(), book.free_count()), (1, 1));
        assert_eq!(
            book.take_free(),
            Some(7),
            "the released set is the next one"
        );
        assert_eq!(book.in_use(), 2);
        assert!(!book.may_allocate(), "recycling never raises the cap");
    }
}
