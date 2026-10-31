//! The scene presenter's compatibility lane on a real window (milestone 5.5, issue 2a, part B): the initial gate, the held
//! image, a compatibility upload failure as the presenter's `Failed`, and the legacy quad drawn first by today's path.
//! A submodule of `window_scene`, so its cases run in the same workers.

use ck_gpu_vulkan::{
    CompatKind, DestinationRect, FrameCapture, FrameCaptures, InstanceRole, PixelSource,
    SourceRect, TextureTestHooks, VulkanFilter, VulkanFrame, VulkanInstance, VulkanQuad,
};

use super::{
    Arc, Case, Duration, Harness, Mutex, PresenterReport, Quads, Solid, failed_messages, quad,
    reports, vk,
};

fn compat_instance(key: u32, left: i32, top: i32, side: i32) -> VulkanInstance<u32> {
    VulkanInstance {
        role: InstanceRole::Compat,
        ..quad(key, left, top, side)
    }
}

impl Harness {
    /// Publishes a frame whose compatibility key is `key`.
    fn publish_compat(&mut self, key: u32, instances: Vec<VulkanInstance<u32>>) {
        self.generation += 1;
        self.frames.publish(
            VulkanFrame::new(
                self.generation,
                VulkanFilter::Nearest,
                None,
                Some(Arc::new(Quads(instances))),
            )
            .with_compat_key(Some(key)),
        );
    }

    /// What the frames built so far did with the compatibility quad, in order.
    fn compat_kinds(&self) -> Vec<CompatKind> {
        self.builds().iter().map(|build| build.compat).collect()
    }
}

/// The gate holds the first frame that would show the compatibility quad until its image resolves: the first attempt is
/// held back (once), the want is parked with no repeated attempt, and the frame runs when the image is committed, with the
/// image drawn.
///
/// Negative control: the idle rule's unit test shows that a want parked on a gate is not runnable; here the attempts stay at
/// one while the gate is closed.
fn the_gate_holds_the_first_frame_until_the_compat_image_resolves() {
    let mut harness = Harness::new();
    harness.publish_compat(5, vec![compat_instance(5, 10, 10, 50)]);
    let mut presenter = harness.start();
    harness.wait_for("the first attempt to be held back", |harness| {
        harness.compat_kinds() == vec![CompatKind::Gated]
    });
    let attempts = harness.waits.attempts();
    harness.rig.idle(Duration::from_millis(100));
    assert_eq!(
        harness.waits.attempts(),
        attempts,
        "a held want is not attempted again"
    );
    assert_eq!(presenter.scene_stats().frames, 0, "nothing was drawn");
    harness.offer(5, 4, [0, 255, 0, 255]);
    (presenter.texture_wake())();
    harness.wait_stats(&presenter, "the frame once the image is there", |stats| {
        stats.frames >= 1
    });
    assert_eq!(
        harness.compat_kinds(),
        vec![CompatKind::Gated, CompatKind::Drew],
        "held once, then drawn with the compatibility image"
    );
    harness.finish(&mut presenter);
}

/// The last compatibility image drawn stays on screen until a newer one resolves: after key 5's image is drawn, a frame whose
/// compatibility key is 6 (nothing there yet) draws key 5's held image, and once key 6's image is adopted it draws that.
fn the_held_compat_image_stays_until_a_newer_one_resolves() {
    let mut harness = Harness::new();
    harness.publish_compat(5, vec![compat_instance(5, 10, 10, 50)]);
    let mut presenter = harness.start();
    harness.wait_for("the first attempt to be held back", |harness| {
        harness.compat_kinds() == vec![CompatKind::Gated]
    });
    harness.offer(5, 4, [0, 255, 0, 255]);
    (presenter.texture_wake())();
    harness.wait_stats(&presenter, "key 5 drawn", |stats| stats.frames >= 1);
    // A newer compatibility image is pending: the held one is drawn in its place.
    harness.publish_compat(6, vec![compat_instance(6, 10, 10, 50)]);
    harness.wait_stats(&presenter, "the frame with the held image", |stats| {
        stats.frames >= 2
    });
    harness.offer(6, 4, [0, 0, 255, 255]);
    (presenter.texture_wake())();
    harness.wait_for("key 6 drawn", |harness| {
        harness.compat_kinds().last() == Some(&CompatKind::Drew) && harness.builds().len() >= 3
    });
    let kinds = harness.compat_kinds();
    assert_eq!(
        kinds[..3],
        [CompatKind::Gated, CompatKind::Drew, CompatKind::DrewHeld]
    );
    assert_eq!(kinds.last(), Some(&CompatKind::Drew), "{kinds:?}");
    harness.finish(&mut presenter);
}

/// An upload failure of the compatibility key is the presenter's own `Failed`, once, naming the legacy quad's image.
///
/// Negative control: `a_compatibility_upload_failure_is_reported_before_the_ordinary_one_and_only_for_the_marked_key` shows that
/// an unmarked key's failure is not reported this way.
fn a_compat_upload_failure_is_one_failed() {
    let mut harness = Harness::new();
    harness.publish_compat(5, vec![compat_instance(5, 10, 10, 50)]);
    // The second image allocation fails: the checker's is the first.
    let mut presenter = harness.start_with(
        Harness::limits(),
        Some(TextureTestHooks {
            fail_allocation_at: Some(2),
            ..TextureTestHooks::default()
        }),
        harness.rig.pixel,
        |_| {},
    );
    harness.wait_for("the first attempt to be held back", |harness| {
        harness.compat_kinds() == vec![CompatKind::Gated]
    });
    // 1100 x 1100 has an allocation of its own (over the dedicated-allocation threshold): the injected failure is its.
    harness.offer(5, 1100, [0, 255, 0, 255]);
    (presenter.texture_wake())();
    let mut seen = Vec::new();
    harness.rig.wait("the failure", || {
        seen.extend(reports(&presenter));
        !failed_messages(&seen).is_empty()
    });
    harness.rig.idle(Duration::from_millis(50));
    seen.extend(reports(&presenter));
    let failed = failed_messages(&seen);
    assert_eq!(failed.len(), 1, "one Failed: {seen:?}");
    assert!(
        failed[0].contains("legacy quad's image upload failed"),
        "{}",
        failed[0]
    );
    assert!(
        !seen.contains(&PresenterReport::FirstFrameQueued),
        "nothing was drawn"
    );
    harness.finish(&mut presenter);
}

// ---- the legacy quad ----

fn legacy() -> VulkanQuad {
    // 2 x 2: opaque red, opaque green, half-alpha blue, transparent.
    let pixels: Arc<dyn PixelSource> = Arc::new(Solid(
        [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 128, 128],
            [0, 0, 0, 0],
        ]
        .concat(),
    ));
    VulkanQuad::new(
        pixels,
        2,
        2,
        DestinationRect {
            left: 40,
            top: 30,
            right: 60,
            bottom: 50,
        },
        SourceRect {
            left: 0.0,
            top: 0.0,
            right: 2.0,
            bottom: 2.0,
        },
        VulkanFilter::Nearest,
    )
    .expect("a valid quad")
}

fn bgra(capture: &FrameCapture, x: u32, y: u32) -> [u8; 4] {
    assert_eq!(capture.format, vk::Format::B8G8R8A8_UNORM);
    let at = ((y * capture.size.0 + x) * 4) as usize;
    [
        capture.bytes[at + 2],
        capture.bytes[at + 1],
        capture.bytes[at],
        capture.bytes[at + 3],
    ]
}

fn captured(harness: &mut Harness, captures: &FrameCaptures, generation: u64) -> FrameCapture {
    let sink = Arc::clone(captures);
    harness.rig.wait("the capture", || {
        sink.lock()
            .expect("the captures")
            .iter()
            .any(|frame| frame.generation == Some(generation))
    });
    let all = captures.lock().expect("the captures");
    all.iter()
        .rev()
        .find(|frame| frame.generation == Some(generation))
        .cloned()
        .expect("the capture is there")
}

/// The legacy quad of a frame is drawn first, by today's path, under every scene quad: a frame carrying only the quad shows its
/// four blocks over the clear; the next frame adds a scene quad (the checker, for a pending key) over the quad's top-left
/// block and that block is the checker's while the other blocks stay the legacy quad's.
///
/// Negative control: the first capture's top-left block is the quad's opaque red, so the second capture's magenta there is the
/// scene quad drawn over it, not the quad.
fn the_legacy_quad_is_drawn_first_under_the_scene_quads() {
    let mut harness = Harness::new();
    let captures: FrameCaptures = Arc::new(Mutex::new(Vec::new()));
    harness.generation += 1;
    harness.frames.publish(VulkanFrame::new(
        harness.generation,
        VulkanFilter::Nearest,
        Some(legacy()),
        Some(Arc::new(Quads(Vec::new()))),
    ));
    let sink = Arc::clone(&captures);
    let mut presenter =
        harness.start_with(Harness::limits(), None, harness.rig.pixel, move |hooks| {
            hooks.capture_frames = Some(sink);
        });
    let first_generation = harness.generation;
    let first = captured(&mut harness, &captures, first_generation);
    assert_eq!(bgra(&first, 45, 35), [255, 0, 0, 255], "opaque red");
    assert_eq!(bgra(&first, 55, 35), [0, 255, 0, 255], "opaque green");
    assert_eq!(
        bgra(&first, 10, 10),
        [26, 51, 77, 255],
        "the clear outside the quad"
    );
    // The same legacy quad, now with a scene quad over its top-left block (key 9 has nothing shown: the checker).
    harness.generation += 1;
    harness.frames.publish(VulkanFrame::new(
        harness.generation,
        VulkanFilter::Nearest,
        Some(legacy()),
        Some(Arc::new(Quads(vec![quad(9, 40, 30, 8)]))),
    ));
    let second_generation = harness.generation;
    let second = captured(&mut harness, &captures, second_generation);
    assert_eq!(
        bgra(&second, 42, 32),
        [255, 0, 255, 255],
        "the scene quad (the checker) over the legacy quad"
    );
    assert_eq!(
        bgra(&second, 55, 35),
        [0, 255, 0, 255],
        "the legacy quad elsewhere"
    );
    harness.finish(&mut presenter);
}

/// A frame carrying a compatibility key draws that quad through the ledger, not the legacy quad: the legacy quad of such a
/// frame is not drawn (the clear shows where it would be).
fn a_frame_with_a_compat_key_does_not_draw_the_legacy_quad() {
    let mut harness = Harness::new();
    let captures: FrameCaptures = Arc::new(Mutex::new(Vec::new()));
    harness.generation += 1;
    // The compatibility image is pending, so the frame is gated and nothing is drawn; the next frame, without a compatibility
    // key, draws the quad. First the keyed frame is published with a resident image so it draws.
    harness.offer(5, 4, [0, 255, 0, 255]);
    harness.frames.publish(
        VulkanFrame::new(
            harness.generation,
            VulkanFilter::Nearest,
            Some(legacy()),
            Some(Arc::new(Quads(vec![VulkanInstance {
                role: InstanceRole::Compat,
                ..quad(5, 100, 100, 20)
            }]))),
        )
        .with_compat_key(Some(5)),
    );
    let sink = Arc::clone(&captures);
    let mut presenter =
        harness.start_with(Harness::limits(), None, harness.rig.pixel, move |hooks| {
            hooks.capture_frames = Some(sink);
        });
    let generation = harness.generation;
    let frame = captured(&mut harness, &captures, generation);
    assert_eq!(
        bgra(&frame, 110, 110),
        [0, 255, 0, 255],
        "the compatibility image"
    );
    assert_eq!(
        bgra(&frame, 45, 35),
        [26, 51, 77, 255],
        "the frame's legacy quad is not drawn: the clear shows where it would be"
    );
    harness.finish(&mut presenter);
}

/// The compatibility lane's cases.
pub(crate) const CASES: [(&str, Case); 5] = [
    (
        "the_gate_holds_the_first_frame_until_the_compat_image_resolves",
        the_gate_holds_the_first_frame_until_the_compat_image_resolves,
    ),
    (
        "the_held_compat_image_stays_until_a_newer_one_resolves",
        the_held_compat_image_stays_until_a_newer_one_resolves,
    ),
    (
        "a_compat_upload_failure_is_one_failed",
        a_compat_upload_failure_is_one_failed,
    ),
    (
        "the_legacy_quad_is_drawn_first_under_the_scene_quads",
        the_legacy_quad_is_drawn_first_under_the_scene_quads,
    ),
    (
        "a_frame_with_a_compat_key_does_not_draw_the_legacy_quad",
        a_frame_with_a_compat_key_does_not_draw_the_legacy_quad,
    ),
];
