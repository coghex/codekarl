//! The quad-slot presenter (`start_2d`, the resource-free `SceneBridge` -> `VulkanSinkSide` path) draws the GPU-written map too
//! (milestone 6, slice 1), so the backends stay interchangeable through the same public API: the first frame, a refusal that is told
//! and retried, and a resize whose old target is retired. A submodule of `window_quad`, so its cases run in the same workers, with
//! the validation layer (any message fails the case), the keyboard filter and the owner-typing run.

use ck_gpu_vulkan::{MapRefusalSink, VulkanMap, VulkanScene};

use super::{Arc, Case, Harness, Ordering, finish_proven};
use crate::window_scene::map::{Told, bgra, corner, expected_pixel, input_with, map_of};

impl Harness {
    /// Publishes a scene (generation `generation`) that carries `map` and no quad.
    fn publish_map(&self, generation: u64, map: VulkanMap) {
        self.slot
            .publish(VulkanScene::new(generation, None).with_map(Some(map)));
    }
}

/// A map published before the presenter starts is in the first frame: the window shows the enlarged map over the clear.
fn quad_slot_map_in_the_first_frame_is_drawn_enlarged() {
    let mut harness = Harness::new();
    let input = input_with(1);
    let top_left = {
        let [x, y] = corner();
        [x - 1, y]
    };
    harness.publish_map(7, map_of(&input, top_left, [5, 3]));
    let mut presenter = harness.start(harness.hooks());
    harness.wait_generation(&presenter, 7);
    let capture = harness.capture_of(7);
    for y in 0..12 {
        for x in 0..20 {
            assert_eq!(
                bgra(&capture, x, y),
                expected_pixel(top_left, x / 4, y / 4),
                "({x}, {y})"
            );
        }
    }
    let stats = harness.probes.observer.map_stats();
    assert_eq!(
        (
            stats.inputs_created,
            stats.targets_created,
            stats.last_target
        ),
        (1, 1, (5, 3))
    );
    assert!(stats.passes >= 1, "{stats:?}");
    finish_proven(harness, &mut presenter);
}

/// A refused allocation is told once and retried on the next frame (the frame is drawn without the map meanwhile), and a resize
/// replaces the target and retires the old one after its proof, on the quad-slot path as on the scene path.
fn quad_slot_a_refusal_is_retried_and_a_resize_retires_the_old_target() {
    let mut harness = Harness::new();
    let told = Arc::new(Told::default());
    let input = input_with(1);
    let with_refusals =
        |map: VulkanMap| map.with_refusals(Arc::clone(&told) as Arc<dyn MapRefusalSink>);
    harness.publish_map(1, with_refusals(map_of(&input, corner(), [5, 3])));
    let mut hooks = harness.hooks();
    hooks.map_allocation_failures = 1;
    let mut presenter = harness.start(hooks);
    harness.wait_generation(&presenter, 1);
    // Nothing is republished: the second frame of generation 1 is the renderer's own retry of the refused map.
    let observer = Arc::clone(&harness.probes.observer);
    harness.rig.wait("the retry to draw the map", || {
        told.cleared.load(Ordering::SeqCst) == 1
    });
    assert_eq!(told.refused.lock().expect("the messages").len(), 1);
    // A frame's capture arrives once its submit is proven: wait for both frames of generation 1.
    let sink = Arc::clone(&harness.captures);
    let of_generation = move || -> Vec<_> {
        sink.lock()
            .expect("the captures")
            .iter()
            .filter(|frame| frame.generation == Some(1))
            .cloned()
            .collect()
    };
    harness
        .rig
        .wait("both frames of generation 1", || of_generation().len() >= 2);
    let frames = of_generation();
    assert_eq!(
        bgra(&frames[0], 2, 2),
        [26, 51, 77, 255],
        "the first frame had no map"
    );
    assert_eq!(
        bgra(&frames[1], 2, 2),
        expected_pixel(corner(), 0, 0),
        "the retry draws the map"
    );
    assert_eq!(
        told.cleared.load(Ordering::SeqCst),
        1,
        "the success is told once"
    );
    // The view grows: a new target, and the old one is retired and destroyed after its proof.
    harness.publish_map(3, with_refusals(map_of(&input, corner(), [6, 4])));
    harness.wait_generation(&presenter, 3);
    harness.rig.wait("the old target's release", || {
        let stats = observer.map_stats();
        stats.targets_created == 2 && stats.retired_released >= 1
    });
    let stats = observer.map_stats();
    assert_eq!(
        (stats.retired, stats.targets_alive, stats.last_target),
        (1, 1, (6, 4))
    );
    finish_proven(harness, &mut presenter);
}

/// The quad-slot presenter's map cases.
pub(crate) const CASES: [(&str, Case); 2] = [
    (
        "quad_slot_map_in_the_first_frame_is_drawn_enlarged",
        quad_slot_map_in_the_first_frame_is_drawn_enlarged,
    ),
    (
        "quad_slot_a_refusal_is_retried_and_a_resize_retires_the_old_target",
        quad_slot_a_refusal_is_retried_and_a_resize_retires_the_old_target,
    ),
];
