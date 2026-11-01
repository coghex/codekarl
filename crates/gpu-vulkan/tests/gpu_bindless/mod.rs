//! The bindless device gate on the real device (milestone 5.5, issue 5): the features the context enables at creation, the
//! capability report it keeps, the limits the gate reads, the support query's answer for the exact table layout (logged,
//! never a gate row) and the one validation message the log tolerates. A submodule of `gpu.rs`, so the group that runs that
//! target runs these.
//!
//! Every case runs under `VK_LAYER_KHRONOS_validation` with synchronization validation and requires an empty log from the
//! creation of the instance to its destruction, **except** the one tolerated message
//! (`VUID-vkCreateDescriptorSetLayout-support-09582`, inside the table layout's own creation call, after a "no"), which the
//! log records separately. The update-after-bind binding flag is in the layouts these cases create, so a feature the context
//! failed to enable would be reported by validation here.

use ck_gpu_vulkan::{InstanceOptions, create_instance_with, load_loader};

use super::gpu_textures::Kit;
use super::{assert_log_clean, destroy};

mod ledger;

/// The final stage's table size: two versions of 131,072 sampled images.
const FINAL_SLOTS: u32 = 131_072;

/// The one message the log tolerates, and nothing else.
const TOLERATED: &str = "VUID-vkCreateDescriptorSetLayout-support-09582";

#[test]
fn the_gate_passes_the_final_scale_on_this_device_and_prints_what_it_read() {
    let kit = Kit::new(false);
    let report = kit.context.bindless_report(FINAL_SLOTS);
    // The device evidence the result file carries: the report as the gate read it, and the support answer.
    println!("bindless capability report: {:#?}", report.caps);
    println!(
        "vkGetDescriptorSetLayoutSupport(table of {FINAL_SLOTS}): {:?}",
        report.layout_supported
    );
    println!("gate refusal at {FINAL_SLOTS}: {:?}", report.refusal);
    let caps = &report.caps;
    assert!(caps.extension, "VK_EXT_descriptor_indexing is enabled");
    assert!(
        caps.dynamic_indexing
            && caps.runtime_descriptor_array
            && caps.non_uniform_indexing
            && caps.sampled_image_update_after_bind,
        "the four features are reported (and so enabled): {caps:?}"
    );
    assert!(!caps.device.is_empty(), "the device name is context");
    let limits = &caps.limits;
    assert!(limits.max_per_stage_descriptor_update_after_bind_sampled_images >= FINAL_SLOTS);
    assert!(limits.max_descriptor_set_update_after_bind_sampled_images >= FINAL_SLOTS);
    assert!(limits.max_per_stage_update_after_bind_resources >= FINAL_SLOTS + 2);
    assert!(
        u64::from(limits.max_update_after_bind_descriptors_in_all_pools)
            >= 2 * u64::from(FINAL_SLOTS),
        "both versions can be allocated at once"
    );
    assert!(limits.max_bound_descriptor_sets >= 2);
    assert_eq!(
        kit.context.bindless_caps(),
        caps,
        "the report is the one the context kept at creation; nothing queries the device again"
    );
    assert_eq!(
        report.refusal, None,
        "every row passes at {FINAL_SLOTS}; the support answer decides nothing"
    );
    assert!(
        report.layout_supported.is_some(),
        "the extension and the features are enabled, so the query was asked"
    );
    // A small table passes too.
    let small = kit.context.bindless_report(1_000);
    assert_eq!(small.refusal, None);
    assert_eq!(
        small.layout_supported,
        Some(true),
        "a small layout is supported everywhere"
    );
    kit.finish();
}

/// A real refusal, with no seam: one slot more than the device's own per-stage update-after-bind sampled-image limit.
#[test]
fn one_slot_past_the_devices_own_limit_is_refused_naming_that_row_with_both_values() {
    let kit = Kit::new(false);
    let limit = kit
        .context
        .bindless_caps()
        .limits
        .max_per_stage_descriptor_update_after_bind_sampled_images;
    let requested = limit
        .checked_add(1)
        .expect("a device limit below u32::MAX, so one more is a request");
    let report = kit.context.bindless_report(requested);
    let refusal = report.refusal.expect("the gate refuses the request");
    println!("refusal: {refusal}");
    assert!(
        refusal.contains(&format!(
            "maxPerStageDescriptorUpdateAfterBindSampledImages {limit} < {requested} requested"
        )),
        "{refusal}"
    );
    assert!(
        refusal.starts_with(&format!("bindless unavailable on {}", report.caps.device)),
        "{refusal}"
    );
    assert!(
        !refusal.contains("vkGetDescriptorSetLayoutSupport"),
        "the support query is not a row: {refusal}"
    );
    // The same device passes at the limit itself, when the other rows allow it: the refusal is the one past it.
    if let Some(refusal) = kit.context.bindless_report(limit).refusal {
        assert!(
            !refusal.contains("maxPerStageDescriptorUpdateAfterBindSampledImages"),
            "{refusal}"
        );
    }
    kit.finish();
}

/// The table layout is created as the ledger creates it, at the final size and at a small one. Where the device answers no
/// (this Mac: `MoltenVK` #2573) the layer may report the one VUID, which the log tolerates; the clean check at the end holds
/// whichever it does. Where the device answers yes nothing is reported at all.
#[test]
fn the_table_layout_is_created_and_the_one_message_is_tolerated_only_after_a_no() {
    let kit = Kit::new(false);
    for slots in [1_000, FINAL_SLOTS] {
        let answer = kit
            .context
            .create_table_layout_for_tests(slots)
            .expect("the layout is created")
            .expect("the extension and the features are enabled, so the query was asked");
        let tolerated = kit.log.tolerated();
        println!(
            "table of {slots}: support answer {answer}; tolerated messages so far: {tolerated:#?}"
        );
        assert_eq!(
            kit.log.messages(),
            Vec::<String>::new(),
            "nothing fails the clean check"
        );
        if slots == 1_000 {
            assert!(answer, "a small layout is supported everywhere");
            assert!(
                tolerated.is_empty(),
                "nothing is reported for a layout the device supports"
            );
        } else if answer {
            assert!(
                tolerated.is_empty(),
                "the device supports the final layout: nothing is reported"
            );
        } else {
            // After a no the layer may report the VUID for the creation (a layer that tracks the query), and then the log
            // records exactly that one message as tolerated; or it reports nothing. The Khronos layer 1.3.296 on this Mac
            // reports nothing: the VUID is about a layout that exceeds the *reported* limits, and this one does not. Either
            // way the clean check holds, and the count is part of the device evidence.
            assert!(
                tolerated.len() <= 1,
                "at most the one message: {tolerated:#?}"
            );
            if let Some(message) = tolerated.first() {
                assert!(message.contains(TOLERATED), "{message}");
            }
            println!(
                "the layer reported the tolerated VUID {} time(s) for the table of {slots}",
                tolerated.len()
            );
        }
    }
    kit.finish();
}

/// The tolerance on a real instance, through the log's own test message path (`submit_test_message` goes through the
/// instance's debug messenger, as the layer's messages do).
#[test]
fn the_tolerance_holds_on_a_real_instance_and_nowhere_else() {
    super::hold_layer_gate();
    let loader = load_loader().expect("the Vulkan loader loads");
    let options = InstanceOptions {
        validation: true,
        synchronization: false,
    };
    let instance = create_instance_with(&loader, &[], options).expect("a validated instance");
    let log = instance.validation_log();
    // Inside the scope after a no: tolerated.
    {
        let _scope = instance.bindless_layout_scope(true);
        assert!(instance.submit_test_message(TOLERATED, "inside, after a no"));
    }
    assert_eq!(log.tolerated().len(), 1);
    assert_eq!(log.messages(), Vec::<String>::new());
    // The same VUID outside the scope, another VUID inside it and the VUID after a "supported" answer all fail.
    assert!(instance.submit_test_message(TOLERATED, "outside the scope"));
    assert_eq!(log.messages().len(), 1);
    {
        let _scope = instance.bindless_layout_scope(true);
        assert!(instance.submit_test_message("VUID-something-else", "inside, another message"));
    }
    assert_eq!(log.messages().len(), 2);
    {
        let _scope = instance.bindless_layout_scope(false);
        assert!(instance.submit_test_message(TOLERATED, "inside, after a supported"));
    }
    assert_eq!(log.messages().len(), 3);
    assert_eq!(log.tolerated().len(), 1, "still the one");
    // Delivery: each entry is drained once, the dropped count is zero, and the tolerated flag travels with the entry.
    let drained = log.drain();
    assert_eq!(drained.dropped, 0);
    assert_eq!(
        drained
            .entries
            .iter()
            .filter(|entry| entry.tolerated)
            .count(),
        1
    );
    assert_eq!(drained.entries.len(), 4);
    assert!(log.drain().entries.is_empty());
    destroy(instance);
    assert_log_clean(&log);
    drop(loader);
}
