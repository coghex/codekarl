//! The bindless device gate (milestone 5.5, issue 5; design §13.3): what the device must offer before the scene path
//! samples through one large sampled-image table, checked on **queried features and limits, never on a driver name**.
//!
//! - **Features** are enabled when the device is created (`context.rs`), because Vulkan allows nothing else: the context
//!   uses `VK_EXT_descriptor_indexing` (the instance requests Vulkan 1.1, so the core 1.2 path is not available) and
//!   enables each of `shaderSampledImageArrayDynamicIndexing`, `runtimeDescriptorArray`,
//!   `shaderSampledImageArrayNonUniformIndexing` and `descriptorBindingSampledImageUpdateAfterBind` that the device
//!   reports. [`BindlessCaps`] keeps what was reported (which is what was enabled) and the limits, once; no later call
//!   queries the device again. `partiallyBound`, the variable descriptor count and `descriptorBindingUpdateUnusedWhilePending`
//!   are neither required nor enabled: every entry is always valid and no entry changes while a pending frame can read it.
//! - **The gate** ([`evaluate`]) is a pure function of the report and the slot count: the extension, the features and the
//!   update-after-bind (and other) limits. It lists **every** failed row with the requested and the available value, in one
//!   bounded line: the rows are fixed, and the device strings are at most 256 bytes each.
//! - **The support query is logged, not gating** (the owner, 2026-10-31, "Siblings' way"). `vkGetDescriptorSetLayoutSupport`
//!   is still called on the exact table layout ([`with_table_layout`] builds it, and the ledger creates the layout from the
//!   same function) and its answer is recorded in [`BindlessReport`]. `MoltenVK` answers no for any layout of 1,024 (1.2.11) or
//!   1,212 (1.4.2) descriptors or more whatever its own update-after-bind limits say
//!   ([KhronosGroup/MoltenVK#2573](https://github.com/KhronosGroup/MoltenVK/issues/2573)), so a no beside passing rows proceeds.
//!   The validation layer then reports `VUID-vkCreateDescriptorSetLayout-support-09582` for the creation, which the validation
//!   log tolerates narrowly (`instance.rs`): [`create_table_layout`] marks that one call.
//! - **The final stage is judged once.** The rows cover the next stage's needs too (its two sampler bindings and its
//!   non-uniform index), so a device is never accepted here and refused one stage later. The last row is the instance stride
//!   (milestone 5.5, issue 6): the bindless record's 36 bytes, padded to the portability subset's queried
//!   `minVertexInputBindingStrideAlignment`, must fit `maxVertexInputBindingStride` (at least 2,048 on every device, so the row is
//!   unreachable in practice, but it is judged here and the owner's logged fallback applies if it ever fails).

use std::ffi::CStr;

use ash::vk;
use ash::{ext, khr};

use crate::Error;
use crate::error::vk_error;

/// The slots of the bindless tables: the caller's number, validated against the logical capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindlessLimits {
    /// Native sampled-image slots in each of the two table versions, slot 0 being the checker's.
    pub texture_slots: u32,
}

impl BindlessLimits {
    /// The GPU crate's own range is `2..=u32::MAX`: the device decides the upper bound through the gate. The application's
    /// configuration keeps its own, smaller range (`2..=1,000,000`).
    pub const MIN_SLOTS: u32 = 2;

    /// Checks the slot count against the logical capacity (design §7.1): `texture_slots >= max_textures +
    /// max_extra_revisions + 1`, the one being the checker's slot, without overflow.
    ///
    /// # Errors
    ///
    /// The refusal names the three numbers.
    pub fn validate(self, max_textures: usize, max_extra_revisions: usize) -> Result<Self, Error> {
        if self.texture_slots < Self::MIN_SLOTS {
            return Err(Error(format!(
                "bindless texture_slots={} is below the minimum {} (the checker's slot and one more)",
                self.texture_slots,
                Self::MIN_SLOTS
            )));
        }
        let required = max_textures
            .checked_add(max_extra_revisions)
            .and_then(|sum| sum.checked_add(1));
        let slots = usize::try_from(self.texture_slots).unwrap_or(usize::MAX);
        match required {
            Some(required) if slots >= required => Ok(self),
            Some(required) => Err(Error(format!(
                "bindless texture_slots={} is below max_textures={max_textures} + max_extra_revisions={max_extra_revisions} + checker=1 (required {required})",
                self.texture_slots
            ))),
            None => Err(Error(format!(
                "bindless capacity overflows: texture_slots={}, max_textures={max_textures}, max_extra_revisions={max_extra_revisions}",
                self.texture_slots
            ))),
        }
    }
}

/// The device limits the gate reads (the descriptor-indexing properties, the core limits and the stride alignment), as `u32` like Vulkan.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BindlessDeviceLimits {
    /// `maxPerStageDescriptorUpdateAfterBindSampledImages`.
    pub max_per_stage_descriptor_update_after_bind_sampled_images: u32,
    /// `maxDescriptorSetUpdateAfterBindSampledImages`.
    pub max_descriptor_set_update_after_bind_sampled_images: u32,
    /// `maxPerStageDescriptorUpdateAfterBindSamplers`.
    pub max_per_stage_descriptor_update_after_bind_samplers: u32,
    /// `maxDescriptorSetUpdateAfterBindSamplers`.
    pub max_descriptor_set_update_after_bind_samplers: u32,
    /// `maxPerStageDescriptorSamplers` (set 0 is not update-after-bind).
    pub max_per_stage_descriptor_samplers: u32,
    /// `maxDescriptorSetSamplers`.
    pub max_descriptor_set_samplers: u32,
    /// `maxPerStageUpdateAfterBindResources`.
    pub max_per_stage_update_after_bind_resources: u32,
    /// `maxUpdateAfterBindDescriptorsInAllPools`.
    pub max_update_after_bind_descriptors_in_all_pools: u32,
    /// `maxBoundDescriptorSets`.
    pub max_bound_descriptor_sets: u32,
    /// `maxVertexInputBindingStride`.
    pub max_vertex_input_binding_stride: u32,
    /// The portability subset's `minVertexInputBindingStrideAlignment`; 1 on a device without the subset (core Vulkan imposes no
    /// stride alignment). 0 is read as 1.
    pub min_vertex_input_binding_stride_alignment: u32,
}

/// What the device reported for the gate, read once when the context was made. The four features are what was reported and
/// therefore what was enabled; the extension is enabled when advertised.
#[doc(hidden)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the four features are reported independently, as the device reports them"
)]
pub struct BindlessCaps {
    /// The device's name (context for the diagnostic only).
    pub device: String,
    /// The driver's name and info (context for the diagnostic only; empty when the device does not report them).
    pub driver: String,
    /// `VK_EXT_descriptor_indexing` is advertised, and enabled.
    pub extension: bool,
    /// Core `shaderSampledImageArrayDynamicIndexing`.
    pub dynamic_indexing: bool,
    /// `runtimeDescriptorArray`.
    pub runtime_descriptor_array: bool,
    /// `shaderSampledImageArrayNonUniformIndexing`.
    pub non_uniform_indexing: bool,
    /// `descriptorBindingSampledImageUpdateAfterBind`.
    pub sampled_image_update_after_bind: bool,
    /// The limits.
    pub limits: BindlessDeviceLimits,
}

impl BindlessCaps {
    /// Whether every feature the table needs is reported (and so enabled).
    #[must_use]
    pub fn features_complete(&self) -> bool {
        self.dynamic_indexing
            && self.runtime_descriptor_array
            && self.non_uniform_indexing
            && self.sampled_image_update_after_bind
    }

    /// Reads the device: the extension list, the features and the properties, through the Vulkan 1.1 `2` entry points.
    pub(crate) fn query(
        instance: &ash::Instance,
        physical: vk::PhysicalDevice,
        properties: &vk::PhysicalDeviceProperties,
    ) -> Result<Self, Error> {
        // SAFETY: `physical` came from this instance.
        let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }
            .map_err(|code| vk_error("vkEnumerateDeviceExtensionProperties", code))?;
        let advertised = |wanted: &CStr| {
            extensions.iter().any(|property| {
                property
                    .extension_name_as_c_str()
                    .is_ok_and(|name| name == wanted)
            })
        };
        let extension = advertised(ext::descriptor_indexing::NAME);
        let driver_properties = advertised(khr::driver_properties::NAME);
        let mut caps = Self {
            device: properties
                .device_name_as_c_str()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            extension,
            ..Self::default()
        };
        caps.limits.max_per_stage_descriptor_samplers =
            properties.limits.max_per_stage_descriptor_samplers;
        caps.limits.max_descriptor_set_samplers = properties.limits.max_descriptor_set_samplers;
        caps.limits.max_bound_descriptor_sets = properties.limits.max_bound_descriptor_sets;
        caps.limits.max_vertex_input_binding_stride =
            properties.limits.max_vertex_input_binding_stride;
        caps.limits.min_vertex_input_binding_stride_alignment = 1;

        let mut indexing = vk::PhysicalDeviceDescriptorIndexingFeatures::default();
        let mut features = vk::PhysicalDeviceFeatures2::default();
        if extension {
            features = features.push_next(&mut indexing);
        }
        // SAFETY: `physical` came from this instance; the chain holds the descriptor-indexing structure only when the
        // device advertises its extension, and `features` is valid for the call.
        unsafe { instance.get_physical_device_features2(physical, &mut features) };
        caps.dynamic_indexing = features
            .features
            .shader_sampled_image_array_dynamic_indexing
            == vk::TRUE;
        if extension {
            caps.runtime_descriptor_array = indexing.runtime_descriptor_array == vk::TRUE;
            caps.non_uniform_indexing =
                indexing.shader_sampled_image_array_non_uniform_indexing == vk::TRUE;
            caps.sampled_image_update_after_bind =
                indexing.descriptor_binding_sampled_image_update_after_bind == vk::TRUE;
        }

        let mut indexing_properties = vk::PhysicalDeviceDescriptorIndexingProperties::default();
        let mut driver = vk::PhysicalDeviceDriverProperties::default();
        let mut subset = vk::PhysicalDevicePortabilitySubsetPropertiesKHR::default();
        let portability_subset = advertised(khr::portability_subset::NAME);
        let mut query = vk::PhysicalDeviceProperties2::default();
        if extension {
            query = query.push_next(&mut indexing_properties);
        }
        if portability_subset {
            query = query.push_next(&mut subset);
        }
        if driver_properties {
            query = query.push_next(&mut driver);
        }
        // SAFETY: `physical` came from this instance; each structure is chained only when the device advertises the
        // extension that defines it, and `query` is valid for the call.
        unsafe { instance.get_physical_device_properties2(physical, &mut query) };
        if extension {
            let limits = &mut caps.limits;
            limits.max_per_stage_descriptor_update_after_bind_sampled_images =
                indexing_properties.max_per_stage_descriptor_update_after_bind_sampled_images;
            limits.max_descriptor_set_update_after_bind_sampled_images =
                indexing_properties.max_descriptor_set_update_after_bind_sampled_images;
            limits.max_per_stage_descriptor_update_after_bind_samplers =
                indexing_properties.max_per_stage_descriptor_update_after_bind_samplers;
            limits.max_descriptor_set_update_after_bind_samplers =
                indexing_properties.max_descriptor_set_update_after_bind_samplers;
            limits.max_per_stage_update_after_bind_resources =
                indexing_properties.max_per_stage_update_after_bind_resources;
            limits.max_update_after_bind_descriptors_in_all_pools =
                indexing_properties.max_update_after_bind_descriptors_in_all_pools;
        }
        if portability_subset {
            caps.limits.min_vertex_input_binding_stride_alignment =
                subset.min_vertex_input_binding_stride_alignment.max(1);
        }
        if driver_properties {
            let name = driver
                .driver_name_as_c_str()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let info = driver
                .driver_info_as_c_str()
                .map(|info| info.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("{name} {info}").trim().clone_into(&mut caps.driver);
        }
        Ok(caps)
    }
}

/// One limit row of the gate: what the device offers against what N slots need.
struct Row {
    name: &'static str,
    available: u32,
    /// `None` when the requirement does not fit a `u32` (an overflowing N: no `u32` limit can meet it).
    needed: Option<u32>,
}

/// The limit rows for `slots` (N), in the order the diagnostic lists them. `N + 2` and `2 * N` are checked.
fn rows(limits: &BindlessDeviceLimits, slots: u32) -> [Row; 10] {
    let row = |name, available, needed| Row {
        name,
        available,
        needed,
    };
    [
        row(
            "maxPerStageDescriptorUpdateAfterBindSampledImages",
            limits.max_per_stage_descriptor_update_after_bind_sampled_images,
            Some(slots),
        ),
        row(
            "maxDescriptorSetUpdateAfterBindSampledImages",
            limits.max_descriptor_set_update_after_bind_sampled_images,
            Some(slots),
        ),
        row(
            "maxPerStageDescriptorUpdateAfterBindSamplers",
            limits.max_per_stage_descriptor_update_after_bind_samplers,
            Some(2),
        ),
        row(
            "maxDescriptorSetUpdateAfterBindSamplers",
            limits.max_descriptor_set_update_after_bind_samplers,
            Some(2),
        ),
        row(
            "maxPerStageDescriptorSamplers",
            limits.max_per_stage_descriptor_samplers,
            Some(2),
        ),
        row(
            "maxDescriptorSetSamplers",
            limits.max_descriptor_set_samplers,
            Some(2),
        ),
        row(
            "maxPerStageUpdateAfterBindResources",
            limits.max_per_stage_update_after_bind_resources,
            slots.checked_add(2),
        ),
        row(
            "maxUpdateAfterBindDescriptorsInAllPools",
            limits.max_update_after_bind_descriptors_in_all_pools,
            slots.checked_mul(2),
        ),
        row(
            "maxBoundDescriptorSets",
            limits.max_bound_descriptor_sets,
            Some(2),
        ),
        row(
            "maxVertexInputBindingStride",
            limits.max_vertex_input_binding_stride,
            crate::batch::InstanceLayout::bindless(
                limits.min_vertex_input_binding_stride_alignment,
            )
            .and_then(|layout| u32::try_from(layout.stride).ok()),
        ),
    ]
}

/// Every failed row of the gate for `slots`, each as text with the requested and the available value; empty when the device
/// passes. Without the extension the limit rows cannot be queried, so the extension alone is named. The support query is not a
/// row (see the module documentation).
#[must_use]
pub(crate) fn evaluate(caps: &BindlessCaps, slots: u32) -> Vec<String> {
    if !caps.extension {
        return vec!["VK_EXT_descriptor_indexing is not available".to_owned()];
    }
    let mut failed = Vec::new();
    for (present, name) in [
        (
            caps.dynamic_indexing,
            "shaderSampledImageArrayDynamicIndexing",
        ),
        (caps.runtime_descriptor_array, "runtimeDescriptorArray"),
        (
            caps.non_uniform_indexing,
            "shaderSampledImageArrayNonUniformIndexing",
        ),
        (
            caps.sampled_image_update_after_bind,
            "descriptorBindingSampledImageUpdateAfterBind",
        ),
    ] {
        if !present {
            failed.push(format!("feature {name} is not supported"));
        }
    }
    for row in rows(&caps.limits, slots) {
        match row.needed {
            Some(needed) if row.available >= needed => {}
            Some(needed) => failed.push(format!(
                "{} {} < {needed} requested",
                row.name, row.available
            )),
            None => failed.push(format!(
                "{} {} < more than {} requested (the requirement for {slots} slots overflows)",
                row.name,
                row.available,
                u32::MAX
            )),
        }
    }
    failed
}

/// The one-line refusal for `failed` rows on the device `caps` describes: the queried names appear as context only.
#[must_use]
pub(crate) fn diagnostic(caps: &BindlessCaps, failed: &[String]) -> String {
    let driver = if caps.driver.is_empty() {
        String::new()
    } else {
        format!(" ({})", caps.driver)
    };
    format!(
        "bindless unavailable on {}{driver}: {}",
        caps.device,
        failed.join("; ")
    )
}

/// Calls `then` with the create info of the table's set layout for `slots`: one `SAMPLED_IMAGE` binding of `slots`
/// descriptors for the fragment stage, flagged `UPDATE_AFTER_BIND`, in a layout flagged `UPDATE_AFTER_BIND_POOL`. No other
/// binding flag: not partially bound, no variable count, not "update unused while pending". The support query and the
/// ledger's layout creation both go through here, so the gate judges the layout that is created.
pub(crate) fn with_table_layout<R>(
    slots: u32,
    then: impl FnOnce(&vk::DescriptorSetLayoutCreateInfo<'_>) -> R,
) -> R {
    let binding = [vk::DescriptorSetLayoutBinding::default()
        .binding(0)
        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
        .descriptor_count(slots)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
    let flags = [vk::DescriptorBindingFlags::UPDATE_AFTER_BIND];
    let mut flags_info =
        vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&flags);
    let info = vk::DescriptorSetLayoutCreateInfo::default()
        .flags(vk::DescriptorSetLayoutCreateFlags::UPDATE_AFTER_BIND_POOL)
        .bindings(&binding)
        .push_next(&mut flags_info);
    then(&info)
}

/// Asks the device whether it supports the table layout for `slots`. The extension and the features must be enabled (the
/// binding-flags chain is the extension's, and the flag needs its feature), which the caller checked.
pub(crate) fn layout_supported(device: &ash::Device, slots: u32) -> bool {
    with_table_layout(slots, |info| {
        let mut support = vk::DescriptorSetLayoutSupport::default();
        // SAFETY: the device is live, the extension and the update-after-bind feature are enabled (the caller's
        // contract), and `info` (with its flags chain) and `support` are valid for the call.
        unsafe { device.get_descriptor_set_layout_support(info, &mut support) };
        support.supported == vk::TRUE
    })
}

/// What the gate found on a device for N slots: the report, the support query's answer and the refusal when there is one.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindlessReport {
    /// What the device reported.
    pub caps: BindlessCaps,
    /// `vkGetDescriptorSetLayoutSupport`'s answer for the exact table layout, or `None` when it could not be asked (the
    /// extension or a feature is missing). Logged, never a gate row: a no beside an empty `refusal` proceeds.
    pub layout_supported: Option<bool>,
    /// The one-line diagnostic listing every failed row; `None` when the device passes.
    pub refusal: Option<String>,
}

impl BindlessReport {
    /// The report for `caps` and `slots` given the support query's answer.
    pub(crate) fn of(caps: &BindlessCaps, slots: u32, layout_supported: Option<bool>) -> Self {
        let failed = evaluate(caps, slots);
        let refusal = (!failed.is_empty()).then(|| diagnostic(caps, &failed));
        Self {
            caps: caps.clone(),
            layout_supported,
            refusal,
        }
    }

    /// Whether the support query said no although every row passed: the `MoltenVK` case, which proceeds.
    #[must_use]
    pub fn proceeds_despite_support_no(&self) -> bool {
        self.refusal.is_none() && self.layout_supported == Some(false)
    }
}

/// Runs the gate on `device` for `slots`: the rows, and the support query when the extension and the features are enabled.
pub(crate) fn run(device: &ash::Device, caps: &BindlessCaps, slots: u32) -> BindlessReport {
    let ask = caps.extension && caps.features_complete();
    BindlessReport::of(caps, slots, ask.then(|| layout_supported(device, slots)))
}

/// Creates the table's set layout for `slots`. `support` is the query's answer for it. The call is marked on the instance's
/// validation log: when the answer was no, the one message `VUID-vkCreateDescriptorSetLayout-support-09582` that the layer
/// reports for this call is recorded as tolerated, and nothing else is (see `instance.rs`). The mark is a guard on the calling
/// thread, set immediately before the call and cleared immediately after it, on success and on error.
pub(crate) fn create_table_layout(
    context: &crate::context::VulkanContext,
    slots: u32,
    support: Option<bool>,
) -> Result<vk::DescriptorSetLayout, Error> {
    let device = context.device();
    let _scope = context.layout_scope(support == Some(false));
    with_table_layout(slots, |info| {
        // SAFETY: the device is live and `info` (with its binding-flags chain) is valid for the call.
        unsafe { device.create_descriptor_set_layout(info, None) }
    })
    .map_err(|code| vk_error("vkCreateDescriptorSetLayout(table)", code))
}

#[cfg(test)]
mod tests {
    use super::{
        BindlessCaps, BindlessDeviceLimits, BindlessLimits, BindlessReport, diagnostic, evaluate,
        rows,
    };

    const N: u32 = 131_072;

    /// A device that meets every row for `N` slots with room to spare.
    fn good() -> BindlessCaps {
        BindlessCaps {
            device: "Test GPU".to_owned(),
            driver: "TestDriver 1.2".to_owned(),
            extension: true,
            dynamic_indexing: true,
            runtime_descriptor_array: true,
            non_uniform_indexing: true,
            sampled_image_update_after_bind: true,
            limits: BindlessDeviceLimits {
                max_per_stage_descriptor_update_after_bind_sampled_images: 1_000_000,
                max_descriptor_set_update_after_bind_sampled_images: 1_000_000,
                max_per_stage_descriptor_update_after_bind_samplers: 500_000,
                max_descriptor_set_update_after_bind_samplers: 500_000,
                max_per_stage_descriptor_samplers: 16,
                max_descriptor_set_samplers: 80,
                max_per_stage_update_after_bind_resources: 1_000_000,
                max_update_after_bind_descriptors_in_all_pools: 1 << 30,
                max_bound_descriptor_sets: 8,
                max_vertex_input_binding_stride: 2048,
                min_vertex_input_binding_stride_alignment: 1,
            },
        }
    }

    #[test]
    fn a_device_with_everything_passes() {
        assert!(evaluate(&good(), N).is_empty());
        assert!(evaluate(&good(), 2).is_empty());
    }

    #[test]
    fn each_missing_feature_is_refused_and_named() {
        type Clear = fn(&mut BindlessCaps);
        let cases: [(&str, Clear); 4] = [
            ("shaderSampledImageArrayDynamicIndexing", |caps| {
                caps.dynamic_indexing = false;
            }),
            ("runtimeDescriptorArray", |caps| {
                caps.runtime_descriptor_array = false;
            }),
            ("shaderSampledImageArrayNonUniformIndexing", |caps| {
                caps.non_uniform_indexing = false;
            }),
            ("descriptorBindingSampledImageUpdateAfterBind", |caps| {
                caps.sampled_image_update_after_bind = false;
            }),
        ];
        for (name, clear) in cases {
            let mut caps = good();
            clear(&mut caps);
            let failed = evaluate(&caps, N);
            assert_eq!(failed.len(), 1, "{name}: {failed:?}");
            assert!(failed[0].contains(name), "{name} is named in {failed:?}");
            assert!(!caps.features_complete());
        }
    }

    #[test]
    fn a_missing_extension_is_refused_naming_the_extension_alone() {
        let mut caps = good();
        caps.extension = false;
        // The limit rows cannot be queried without the extension, so even an all-zero report names only the extension.
        caps.limits = BindlessDeviceLimits::default();
        let failed = evaluate(&caps, N);
        assert_eq!(failed, ["VK_EXT_descriptor_indexing is not available"]);
    }

    /// The limit `rows` lists at `index`, mutably.
    fn limit(limits: &mut BindlessDeviceLimits, index: usize) -> &mut u32 {
        match index {
            0 => &mut limits.max_per_stage_descriptor_update_after_bind_sampled_images,
            1 => &mut limits.max_descriptor_set_update_after_bind_sampled_images,
            2 => &mut limits.max_per_stage_descriptor_update_after_bind_samplers,
            3 => &mut limits.max_descriptor_set_update_after_bind_samplers,
            4 => &mut limits.max_per_stage_descriptor_samplers,
            5 => &mut limits.max_descriptor_set_samplers,
            6 => &mut limits.max_per_stage_update_after_bind_resources,
            7 => &mut limits.max_update_after_bind_descriptors_in_all_pools,
            8 => &mut limits.max_bound_descriptor_sets,
            _ => &mut limits.max_vertex_input_binding_stride,
        }
    }

    #[test]
    fn each_limit_one_below_its_requirement_is_refused_with_both_numbers() {
        let names: Vec<&str> = rows(&good().limits, N).iter().map(|row| row.name).collect();
        assert_eq!(names.len(), 10);
        for (index, name) in names.iter().enumerate() {
            let mut caps = good();
            let needed = rows(&caps.limits, N)[index].needed.expect("fits");
            // Exactly the requirement passes; one below is refused, and by that row alone.
            *limit(&mut caps.limits, index) = needed;
            assert!(
                evaluate(&caps, N).is_empty(),
                "{name} at its requirement {needed}"
            );
            *limit(&mut caps.limits, index) = needed - 1;
            let failed = evaluate(&caps, N);
            assert_eq!(failed.len(), 1, "{name}: {failed:?}");
            assert_eq!(
                failed[0],
                format!("{name} {} < {needed} requested", needed - 1)
            );
        }
    }

    /// The stride row judges the record padded to the portability subset's alignment: 36 bytes (an alignment of 1, this Mac's), 40
    /// (8), 48 (16). A device whose `maxVertexInputBindingStride` is below the padded stride falls back, naming the row.
    #[test]
    fn the_stride_row_judges_the_padded_record() {
        let at = |alignment, max| {
            let mut caps = good();
            caps.limits.min_vertex_input_binding_stride_alignment = alignment;
            caps.limits.max_vertex_input_binding_stride = max;
            evaluate(&caps, N)
        };
        assert!(
            at(1, 36).is_empty() && at(0, 36).is_empty(),
            "no alignment: 36"
        );
        assert!(at(8, 40).is_empty());
        assert_eq!(at(8, 39), ["maxVertexInputBindingStride 39 < 40 requested"]);
        assert!(at(16, 48).is_empty());
        assert_eq!(
            at(16, 47),
            ["maxVertexInputBindingStride 47 < 48 requested"]
        );
        assert!(at(4, 36).is_empty(), "36 is already a multiple of 4");
    }

    #[test]
    fn the_example_diagnostic_names_the_row_the_device_and_the_driver() {
        let mut caps = good();
        caps.limits.max_per_stage_update_after_bind_resources = 1_000_000;
        caps.device = "Apple M3 Max".to_owned();
        caps.driver = "MoltenVK 1.2.11".to_owned();
        let failed = evaluate(&caps, 1_000_000);
        assert_eq!(
            diagnostic(&caps, &failed),
            "bindless unavailable on Apple M3 Max (MoltenVK 1.2.11): maxPerStageUpdateAfterBindResources 1000000 < 1000002 requested"
        );
        caps.driver.clear();
        assert!(
            diagnostic(&caps, &failed).starts_with("bindless unavailable on Apple M3 Max: "),
            "no parenthesis without driver strings"
        );
    }

    /// The owner's "Siblings' way" (2026-10-31): the support query's answer is recorded, and gates nothing.
    #[test]
    fn a_false_support_answer_with_every_row_passing_is_accepted_and_recorded() {
        let no = BindlessReport::of(&good(), N, Some(false));
        assert_eq!(no.refusal, None, "every row passes: bindless proceeds");
        assert_eq!(no.layout_supported, Some(false), "the answer is recorded");
        assert!(no.proceeds_despite_support_no());
        let yes = BindlessReport::of(&good(), N, Some(true));
        assert_eq!(
            (yes.refusal.as_deref(), yes.layout_supported),
            (None, Some(true))
        );
        assert!(!yes.proceeds_despite_support_no());
        let unasked = BindlessReport::of(&good(), N, None);
        assert_eq!(
            (unasked.refusal.as_deref(), unasked.layout_supported),
            (None, None)
        );
    }

    #[test]
    fn a_false_support_answer_beside_a_failing_row_falls_back_naming_only_the_failing_row() {
        let mut caps = good();
        caps.limits.max_bound_descriptor_sets = 1;
        let report = BindlessReport::of(&caps, N, Some(false));
        let refusal = report.refusal.clone().expect("a row fails");
        assert!(
            refusal.contains("maxBoundDescriptorSets 1 < 2 requested"),
            "{refusal}"
        );
        assert!(
            !refusal.contains("vkGetDescriptorSetLayoutSupport"),
            "{refusal}"
        );
        assert!(!refusal.contains("; "), "one row only: {refusal}");
        assert!(!report.proceeds_despite_support_no());
    }

    #[test]
    fn several_failures_are_all_listed_and_the_text_stays_bounded() {
        let mut caps = BindlessCaps {
            device: "d".repeat(256),
            driver: "v".repeat(513),
            extension: true,
            ..BindlessCaps::default()
        };
        caps.limits.max_bound_descriptor_sets = 1;
        let failed = evaluate(&caps, N);
        // Four features and ten rows fail.
        assert_eq!(failed.len(), 4 + 10, "{failed:?}");
        let text = diagnostic(&caps, &failed);
        for name in ["maxBoundDescriptorSets", "runtimeDescriptorArray"] {
            assert!(text.contains(name), "{name} in {text}");
        }
        assert!(text.len() < 2048, "bounded: {} bytes", text.len());
    }

    #[test]
    fn the_derived_requirements_are_checked_so_an_overflowing_slot_count_is_refused_not_wrapped() {
        let mut caps = good();
        caps.limits.max_per_stage_update_after_bind_resources = u32::MAX;
        caps.limits.max_update_after_bind_descriptors_in_all_pools = u32::MAX;
        caps.limits
            .max_per_stage_descriptor_update_after_bind_sampled_images = u32::MAX;
        caps.limits
            .max_descriptor_set_update_after_bind_sampled_images = u32::MAX;
        // N + 2 wraps to 1 and 2 * N to a small number for these N; a wrapped requirement would be met.
        for slots in [u32::MAX, u32::MAX - 1, (1 << 31) + 1] {
            let failed = evaluate(&caps, slots);
            let text = failed.join("\n");
            assert!(
                text.contains("maxUpdateAfterBindDescriptorsInAllPools"),
                "2 x {slots} overflows and is refused: {text}"
            );
            if slots >= u32::MAX - 1 {
                assert!(
                    text.contains("maxPerStageUpdateAfterBindResources"),
                    "{slots} + 2 overflows and is refused: {text}"
                );
            }
        }
        // A slot count that fits both stays accepted by a device that offers it.
        assert!(evaluate(&caps, 1 << 30).is_empty());
    }

    /// Both sets' descriptors are in the fragment stage at once, so a million slots need a million and two resources: the
    /// resources row alone refuses a device that offers a million. (Its recorded mutation deletes the row.)
    #[test]
    fn a_million_slots_on_a_million_resources_fail_the_resources_row_alone() {
        let failed = evaluate(&good(), 1_000_000);
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(failed[0].starts_with("maxPerStageUpdateAfterBindResources 1000000 < 1000002"));
    }

    #[test]
    fn the_slot_count_is_checked_against_the_logical_capacity_without_overflow() {
        let limits = |texture_slots| BindlessLimits { texture_slots };
        assert!(limits(2).validate(1, 0).is_ok());
        assert!(limits(17).validate(8, 8).is_ok());
        assert!(limits(u32::MAX).validate(8, 8).is_ok());
        let error = limits(16)
            .validate(8, 8)
            .expect_err("one short")
            .to_string();
        for text in [
            "texture_slots=16",
            "max_textures=8",
            "max_extra_revisions=8",
            "required 17",
        ] {
            assert!(error.contains(text), "{text} in {error}");
        }
        assert!(
            limits(1)
                .validate(0, 0)
                .expect_err("below the range")
                .to_string()
                .contains("minimum 2")
        );
        let error = limits(u32::MAX)
            .validate(usize::MAX, 1)
            .expect_err("an overflowing sum")
            .to_string();
        assert!(error.contains("overflows"), "{error}");
        let error = limits(u32::MAX)
            .validate(usize::MAX - 1, 1)
            .expect_err("the sum fits but the +1 does not")
            .to_string();
        assert!(error.contains("overflows"), "{error}");
        // A requirement larger than any u32 slot count is a refusal with its number, not a wrap.
        if usize::BITS > 32 {
            let error = limits(u32::MAX)
                .validate(1 << 33, 0)
                .expect_err("larger")
                .to_string();
            assert!(error.contains("required"), "{error}");
        }
    }
}
