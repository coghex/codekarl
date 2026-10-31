//! The textured quad on the presenter (milestone 4.5, issue 7): the cases that need a real GLFW window and surface.
//! A submodule of `window.rs`, so its cases run in the same workers, each in its own process, under the validation
//! layer (any message fails the case), with the keyboard filter and the owner-typing run.
//!
//! The surface path's pixels are read through the presenter's frame capture (`TestHooks::capture_frames`): an
//! off-screen readback alone is not proof of what the swap chain showed.

use ck_gpu_vulkan::{
    DestinationRect, FrameCapture, FrameCaptures, PipelineFault, PixelSource, QuadSkipped,
    QuadStats, SourceRect, VulkanFilter, VulkanQuad, VulkanQuadSlot, VulkanScene,
};

use super::{
    Arc, AtomicBool, AtomicUsize, BOUND, Case, CleanupResult, Duration, Event, Gate, Mutex,
    Ordering, PresenterReport, Probes, Rig, TestHooks, VulkanPresenter, check_ledger_order, count,
    reports, vk,
};

// ---------------------------------------------------------------------------------------------
// The tools
// ---------------------------------------------------------------------------------------------

/// A pixel source that counts its reads and its drop.
struct Image {
    bytes: Vec<u8>,
    reads: AtomicUsize,
    drops: Arc<AtomicUsize>,
}

impl PixelSource for Image {
    fn pixels(&self) -> &[u8] {
        self.reads.fetch_add(1, Ordering::SeqCst);
        &self.bytes
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

fn image(width: u32, height: u32, texel: impl Fn(u32, u32) -> [u8; 4]) -> Arc<Image> {
    let mut bytes = Vec::new();
    for y in 0..height {
        for x in 0..width {
            bytes.extend_from_slice(&texel(x, y));
        }
    }
    Arc::new(Image {
        bytes,
        reads: AtomicUsize::new(0),
        drops: Arc::new(AtomicUsize::new(0)),
    })
}

/// Premultiplied texels with known channels: opaque red, opaque green, half-alpha blue, fully transparent.
fn checker() -> Arc<Image> {
    image(2, 2, |x, y| match (x, y) {
        (0, 0) => [255, 0, 0, 255],
        (1, 0) => [0, 255, 0, 255],
        (0, 1) => [0, 0, 128, 128],
        _ => [0, 0, 0, 0],
    })
}

/// A solid image of one texel colour (a different allocation each call, so a different image).
fn solid(width: u32, height: u32, texel: [u8; 4]) -> Arc<Image> {
    image(width, height, |_, _| texel)
}

/// Where every quad of these cases goes: 20 by 20 target pixels at (40, 30).
const DESTINATION: DestinationRect = DestinationRect {
    left: 40,
    top: 30,
    right: 60,
    bottom: 50,
};

fn quad(image: &Arc<Image>, width: u32, height: u32, filter: VulkanFilter) -> VulkanQuad {
    quad_at(image, width, height, DESTINATION, filter)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "image sides in these cases are tiny"
)]
fn quad_at(
    image: &Arc<Image>,
    width: u32,
    height: u32,
    destination: DestinationRect,
    filter: VulkanFilter,
) -> VulkanQuad {
    VulkanQuad::new(
        Arc::clone(image) as Arc<dyn PixelSource>,
        width,
        height,
        destination,
        SourceRect {
            left: 0.0,
            top: 0.0,
            right: width as f32,
            bottom: height as f32,
        },
        filter,
    )
    .expect("a valid quad")
}

fn scene(generation: u64, quad: Option<VulkanQuad>) -> VulkanScene {
    VulkanScene::new(generation, quad)
}

/// A window, a validated instance, a quad slot and the places the presenter reports to.
struct Harness {
    rig: Rig,
    probes: Probes,
    slot: Arc<VulkanQuadSlot>,
    captures: FrameCaptures,
}

impl Harness {
    fn new() -> Self {
        Self {
            rig: Rig::open(),
            probes: Probes::new(),
            slot: VulkanQuadSlot::new(),
            captures: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Hooks that report into the probes, start with the slot and copy every drawn frame.
    fn hooks(&self) -> TestHooks {
        let mut hooks = self.probes.hooks();
        hooks.quad_slot = Some(Arc::clone(&self.slot));
        hooks.capture_frames = Some(Arc::clone(&self.captures));
        hooks
    }

    fn start(&self, hooks: TestHooks) -> VulkanPresenter {
        self.rig.start(&self.probes, hooks)
    }

    fn publish(&self, generation: u64, quad: Option<VulkanQuad>) {
        self.slot.publish(scene(generation, quad));
    }

    fn wait_generation(&mut self, presenter: &VulkanPresenter, generation: u64) {
        self.rig
            .wait(&format!("generation {generation} to be drawn"), || {
                presenter.drawn_generation() == Some(generation)
            });
    }

    fn wait_event(&mut self, what: &str, wanted: impl Fn(&Event) -> bool) {
        let probes = &self.probes;
        self.rig
            .wait(what, || count(&probes.events(), &wanted) >= 1);
    }

    /// The captured frame that drew `generation` (waits for its submit to be proven).
    fn capture_of(&mut self, generation: u64) -> FrameCapture {
        let captures = Arc::clone(&self.captures);
        self.rig
            .wait(&format!("the capture of generation {generation}"), || {
                captures
                    .lock()
                    .expect("the captures")
                    .iter()
                    .any(|frame| frame.generation == Some(generation))
            });
        let all = self.captures.lock().expect("the captures");
        all.iter()
            .rev()
            .find(|frame| frame.generation == Some(generation))
            .cloned()
            .expect("the capture is there")
    }

    fn stats(&self) -> QuadStats {
        self.probes.observer.quad_stats()
    }

    fn events(&self) -> Vec<Event> {
        self.probes.events()
    }
}

/// The pixel at (`x`, `y`) of a capture in `B8G8R8A8_UNORM`, as `[r, g, b, a]`.
fn pixel(capture: &FrameCapture, x: u32, y: u32) -> [u8; 4] {
    assert_eq!(
        capture.format,
        vk::Format::B8G8R8A8_UNORM,
        "this check reads B8G8R8A8_UNORM"
    );
    let offset = ((y * capture.size.0 + x) * 4) as usize;
    let bytes = &capture.bytes[offset..offset + 4];
    [bytes[2], bytes[1], bytes[0], bytes[3]]
}

fn near(actual: [u8; 4], wanted: [u8; 4], tolerance: u8, what: &str) {
    for (got, want) in actual.iter().zip(wanted) {
        assert!(
            got.abs_diff(want) <= tolerance,
            "{what}: {actual:?} against {wanted:?} (±{tolerance})"
        );
    }
}

/// The clear colour the rig starts with (0.1, 0.2, 0.3) as stored in `B8G8R8A8_UNORM`.
const CLEAR: [u8; 4] = [26, 51, 77, 255];

/// The checker's four blocks, each 10 by 10 target pixels, over the known clear: opaque red and green replace it,
/// half-alpha blue adds to half of it (premultiplied: `src + dst × (1 − 128/255)`), transparent leaves it.
fn check_the_checker(capture: &FrameCapture) {
    let at = |x, y| pixel(capture, x, y);
    near(at(45, 35), [255, 0, 0, 255], 1, "opaque red");
    near(at(55, 35), [0, 255, 0, 255], 1, "opaque green");
    near(
        at(45, 45),
        [13, 25, 166, 255],
        2,
        "half-alpha blue over the clear",
    );
    near(at(55, 45), CLEAR, 1, "transparent leaves the clear");
    near(at(10, 10), CLEAR, 1, "outside the quad");
    near(at(39, 35), CLEAR, 1, "just left of the quad");
    near(at(60, 35), CLEAR, 1, "just right of the quad");
}

fn position(events: &[Event], wanted: impl Fn(&Event) -> bool) -> Option<usize> {
    events.iter().position(wanted)
}

fn is_destroyed(event: &Event) -> bool {
    matches!(
        event,
        Event::QuadTextureDestroyed { .. } | Event::QuadStagingDestroyed { .. }
    )
}

/// Every quad object the observer counted is destroyed, and the books balance.
fn assert_balanced(stats: &QuadStats) {
    assert_eq!(stats.textures_alive, 0, "{stats:?}");
    assert_eq!(stats.staging_alive, 0, "{stats:?}");
    assert_eq!(stats.textures_created, stats.textures_released, "{stats:?}");
    assert_eq!(stats.staging_created, stats.staging_released, "{stats:?}");
}

/// The teardown destroyed the quad objects after the fences and semaphores (what uses them) and before the render
/// pass and the device.
fn check_quad_teardown_order(events: &[Event]) {
    let classes: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::Destroyed(class) => Some(*class),
            _ => None,
        })
        .collect();
    assert_eq!(
        classes,
        [
            "command buffers and pool",
            "fences and semaphores",
            "quad objects",
            "image views and framebuffers",
            "render pass",
            "swap chains",
            "device",
        ],
        "the teardown order: {events:?}"
    );
}

/// Ends the presenter: proven, the books balance and the order holds. Closes the window.
fn finish_proven(mut harness: Harness, presenter: &mut VulkanPresenter) {
    let ended = Rig::end(presenter);
    assert_eq!(ended, CleanupResult::Proven);
    let events = harness.events();
    check_ledger_order(&events);
    if harness.stats().textures_created > 0
        || count(&events, |e| *e == Event::QuadPipelineCreated) > 0
    {
        check_quad_teardown_order(&events);
    }
    assert_balanced(&harness.stats());
    harness.rig.idle(Duration::from_millis(1));
    harness.rig.close(&ended);
}

// ---------------------------------------------------------------------------------------------
// The slot, the first frame and what is drawn
// ---------------------------------------------------------------------------------------------

/// A quad published before the presenter starts is in the first frame, observed by its generation and its pixels.
fn quad_published_before_start_is_in_the_first_frame() {
    let mut harness = Harness::new();
    let source = checker();
    harness.publish(7, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut presenter = harness.start(harness.hooks());
    harness.wait_generation(&presenter, 7);
    let capture = harness.capture_of(7);
    check_the_checker(&capture);
    let first = harness.captures.lock().expect("captures")[0].clone();
    assert_eq!(first.generation, Some(7), "the first frame already had it");
    let events = harness.events();
    let created = position(&events, |e| matches!(e, Event::QuadTextureCreated { .. }));
    let submit = position(&events, |e| *e == Event::Submit);
    assert!(created < submit && created.is_some(), "{events:?}");
    assert_eq!(reports(&presenter), vec![PresenterReport::FirstFrameQueued]);
    finish_proven(harness, &mut presenter);
}

/// A later scene with no quad: the next queued frame is the clear only, and the generation is that scene's.
fn a_scene_with_no_quad_draws_only_the_clear() {
    let mut harness = Harness::new();
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut presenter = harness.start(harness.hooks());
    harness.wait_generation(&presenter, 1);
    harness.publish(2, None);
    harness.wait_generation(&presenter, 2);
    let capture = harness.capture_of(2);
    near(
        pixel(&capture, 45, 35),
        CLEAR,
        1,
        "the quad's place is cleared",
    );
    near(
        pixel(&capture, 55, 45),
        CLEAR,
        1,
        "the quad's place is cleared",
    );
    let events = harness.events();
    assert_eq!(count(&events, |e| *e == Event::QuadTextureRetired), 1);
    finish_proven(harness, &mut presenter);
}

/// Publishing while a frame is stalled replaces the value and allocates nothing until a frame is drawn.
fn publishing_during_a_stalled_frame_allocates_nothing() {
    let mut harness = Harness::new();
    let gate = Arc::new(Gate::new());
    let mut hooks = harness.hooks();
    hooks.gate = Some(Arc::clone(&gate));
    let mut presenter = harness.start(hooks);
    gate.open(1);
    harness.wait_generation(&presenter, 0);
    while gate.take_for_test() {}
    let held = harness.probes.frames();
    let images: Vec<Arc<Image>> = (1..=5_u8).map(|n| solid(2, 2, [n, 0, 0, 255])).collect();
    for (generation, source) in (1_u64..).zip(&images) {
        harness.publish(generation, Some(quad(source, 2, 2, VulkanFilter::Nearest)));
    }
    harness.rig.idle(Duration::from_millis(200));
    assert_eq!(
        harness.probes.frames(),
        held,
        "the stalled thread drew nothing"
    );
    let stalled = harness.stats();
    assert_eq!(
        (
            stalled.textures_created,
            stalled.staging_created,
            stalled.allocations.len()
        ),
        (0, 0, 0),
        "nothing was allocated while the frames were stalled: {stalled:?}"
    );
    gate.open(1);
    harness.wait_generation(&presenter, 5);
    let drawn = harness.stats();
    assert_eq!(
        (drawn.textures_created, drawn.staging_created, drawn.uploads),
        (1, 1, 1),
        "only the latest value was uploaded: {drawn:?}"
    );
    drop(images);
    gate.open(100);
    finish_proven(harness, &mut presenter);
}

/// Replacing the image again and again never has more than two textures and one staging buffer alive.
fn replacing_the_image_repeatedly_keeps_the_cap() {
    let mut harness = Harness::new();
    let mut presenter = harness.start(harness.hooks());
    harness.wait_generation(&presenter, 0);
    let mut keep = Vec::new();
    for generation in 1..=6_u8 {
        let source = solid(3, 2, [generation, generation, 0, 255]);
        harness.publish(
            u64::from(generation),
            Some(quad(&source, 3, 2, VulkanFilter::Nearest)),
        );
        keep.push(source);
        harness.wait_generation(&presenter, u64::from(generation));
    }
    let stats = harness.stats();
    assert_eq!(stats.textures_created, 6, "{stats:?}");
    assert!(stats.textures_max_alive <= 2, "{stats:?}");
    assert!(stats.staging_max_alive <= 1, "{stats:?}");
    assert_eq!(stats.uploads, 6, "{stats:?}");
    // A queued present is not a proof yet: the older textures are reaped as the proofs arrive.
    let probes = &harness.probes;
    harness.rig.wait("the retired texture to go", || {
        probes.observer.quad_stats().textures_alive <= 1
    });
    let settled = harness.stats();
    assert_eq!(settled.textures_released, 5, "{settled:?}");
    drop(keep);
    finish_proven(harness, &mut presenter);
}

/// A retired texture is destroyed only after the ledger has the proof of the last submit that drew it: it is kept
/// while fences read as never signalled and goes after the proof, in the events' order.
fn a_retired_texture_is_destroyed_only_after_its_last_submits_proof() {
    let mut harness = Harness::new();
    let never = Arc::new(AtomicBool::new(true));
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut hooks = harness.hooks();
    hooks.fences_never_signal = Some(Arc::clone(&never));
    let mut presenter = harness.start(hooks);
    harness.wait_generation(&presenter, 1);
    harness.publish(2, None);
    harness.wait_event("the texture to be retired", |e| {
        *e == Event::QuadTextureRetired
    });
    harness.rig.idle(Duration::from_millis(200));
    let events = harness.events();
    assert_eq!(
        count(&events, is_destroyed),
        0,
        "the last submit is not proven, so nothing is destroyed: {events:?}"
    );
    assert_eq!(harness.stats().textures_released, 0);
    never.store(false, Ordering::SeqCst);
    harness.wait_event("the retired texture to be destroyed", |e| {
        *e == Event::QuadTextureDestroyed { teardown: false }
    });
    let events = harness.events();
    let proof = position(&events, |e| {
        matches!(
            e,
            Event::FenceSignalled {
                proof: "submit",
                ..
            }
        )
    })
    .expect("the submit's proof");
    let destroyed = position(&events, |e| {
        *e == Event::QuadTextureDestroyed { teardown: false }
    })
    .expect("the destruction");
    assert!(proof < destroyed, "destroyed before the proof: {events:?}");
    harness.wait_generation(&presenter, 2);
    drop(source);
    finish_proven(harness, &mut presenter);
}

/// A move and a filter change draw again without uploading, so the pixel source is not read again.
fn a_move_and_a_filter_change_do_not_upload() {
    let mut harness = Harness::new();
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut presenter = harness.start(harness.hooks());
    harness.wait_generation(&presenter, 1);
    let reads = source.reads.load(Ordering::SeqCst);
    let moved = DestinationRect {
        left: 100,
        top: 80,
        right: 140,
        bottom: 120,
    };
    harness.publish(
        2,
        Some(quad_at(&source, 2, 2, moved, VulkanFilter::Nearest)),
    );
    harness.wait_generation(&presenter, 2);
    let capture = harness.capture_of(2);
    near(
        pixel(&capture, 110, 90),
        [255, 0, 0, 255],
        1,
        "red at the new place",
    );
    near(
        pixel(&capture, 45, 35),
        CLEAR,
        1,
        "nothing at the old place",
    );
    harness.publish(3, Some(quad_at(&source, 2, 2, moved, VulkanFilter::Linear)));
    harness.wait_generation(&presenter, 3);
    let stats = harness.stats();
    assert_eq!((stats.textures_created, stats.uploads), (1, 1), "{stats:?}");
    assert_eq!(
        stats.allocations.len(),
        2,
        "one texture and one staging buffer: {stats:?}"
    );
    // The quads' own checks read the source, one read each; the render thread read it once, for the upload.
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        reads + 2,
        "no more reads"
    );
    drop(source);
    finish_proven(harness, &mut presenter);
}

/// The render thread holds no pixel source after its upload: replacing the slot's value frees the last reference.
fn the_render_thread_holds_no_pixel_source_after_its_upload() {
    let mut harness = Harness::new();
    let source = checker();
    let drops = Arc::clone(&source.drops);
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut presenter = harness.start(harness.hooks());
    harness.wait_generation(&presenter, 1);
    drop(source);
    assert_eq!(drops.load(Ordering::SeqCst), 0, "the slot still holds it");
    harness.publish(2, None);
    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "the replaced value held the last reference: the render thread had none"
    );
    harness.wait_generation(&presenter, 2);
    finish_proven(harness, &mut presenter);
}

// ---------------------------------------------------------------------------------------------
// Ends
// ---------------------------------------------------------------------------------------------

/// A proven end releases every quad object, after what uses them and before the render pass and the device.
fn a_proven_end_releases_every_quad_object() {
    let mut harness = Harness::new();
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut presenter = harness.start(harness.hooks());
    harness.wait_generation(&presenter, 1);
    let before = harness.stats();
    assert_eq!(before.textures_alive, 1, "{before:?}");
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    let events = harness.events();
    assert_eq!(
        count(&events, |e| *e
            == Event::QuadTextureDestroyed { teardown: true }),
        1,
        "{events:?}"
    );
    check_ledger_order(&events);
    check_quad_teardown_order(&events);
    assert_balanced(&harness.stats());
    drop(source);
    harness.rig.close(&ended);
}

/// Fences that never signal: an unproven end releases no quad object (a resident texture, a retired one and a
/// staging buffer are all live).
fn an_unproven_end_releases_no_quad_object() {
    let mut harness = Harness::new();
    let never = Arc::new(AtomicBool::new(true));
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut hooks = harness.hooks();
    hooks.fences_never_signal = Some(Arc::clone(&never));
    hooks.stop_budget = Some(Duration::from_millis(300));
    let mut presenter = harness.start(hooks);
    harness.wait_generation(&presenter, 1);
    harness.publish(2, None);
    harness.wait_event("the texture to be retired", |e| {
        *e == Event::QuadTextureRetired
    });
    let ended = Rig::end(&mut presenter);
    assert!(matches!(ended, CleanupResult::Unproven { .. }), "{ended:?}");
    let events = harness.events();
    super::check_nothing_destroyed(&events);
    assert_eq!(count(&events, is_destroyed), 0, "{events:?}");
    let stats = harness.stats();
    assert_eq!(
        (stats.textures_released, stats.staging_released),
        (0, 0),
        "{stats:?}"
    );
    assert_eq!(
        (stats.textures_alive, stats.staging_alive),
        (1, 1),
        "{stats:?}"
    );
    drop(source);
    harness.rig.retire();
}

/// A stop requested while a texture is retired destroys it only in a proven teardown, never opportunistically.
fn a_stop_while_a_texture_is_retired_destroys_it_only_in_the_teardown() {
    let mut harness = Harness::new();
    let never = Arc::new(AtomicBool::new(true));
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut hooks = harness.hooks();
    hooks.fences_never_signal = Some(Arc::clone(&never));
    let mut presenter = harness.start(hooks);
    harness.wait_generation(&presenter, 1);
    harness.publish(2, None);
    harness.wait_event("the texture to be retired", |e| {
        *e == Event::QuadTextureRetired
    });
    // The stop first; the proofs arrive after it.
    presenter.request_stop();
    harness.rig.idle(Duration::from_millis(100));
    never.store(false, Ordering::SeqCst);
    let ended = presenter
        .poll_end(BOUND)
        .expect("the presenter's end was observed");
    assert_eq!(ended, CleanupResult::Proven);
    let events = harness.events();
    assert_eq!(
        count(&events, |e| *e
            == Event::QuadTextureDestroyed { teardown: false }),
        0,
        "nothing was destroyed outside the teardown once the stop was requested: {events:?}"
    );
    assert_eq!(
        count(&events, |e| *e
            == Event::QuadTextureDestroyed { teardown: true }),
        1,
        "{events:?}"
    );
    check_ledger_order(&events);
    assert_balanced(&harness.stats());
    drop(source);
    harness.rig.close(&ended);
}

// ---------------------------------------------------------------------------------------------
// Failures
// ---------------------------------------------------------------------------------------------

/// A fault in creating the quad's objects: one `Failed` naming `message`, nothing more drawn, the partial objects
/// destroyed on the proven end.
fn a_creation_fault_is_one_failed_and_ends_proven(
    configure: impl FnOnce(&mut TestHooks),
    message: &str,
    expected: (u64, u64),
) {
    let mut harness = Harness::new();
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut hooks = harness.hooks();
    configure(&mut hooks);
    let mut presenter = harness.start(hooks);
    let mut seen = Vec::new();
    harness.rig.wait("a Failed report", || {
        seen.extend(reports(&presenter));
        seen.iter()
            .any(|report| matches!(report, PresenterReport::Failed { .. }))
    });
    harness.rig.idle(Duration::from_millis(150));
    seen.extend(reports(&presenter));
    let failures: Vec<&str> = seen
        .iter()
        .filter_map(|report| match report {
            PresenterReport::Failed { message } => Some(message.as_str()),
            PresenterReport::FirstFrameQueued => None,
        })
        .collect();
    assert_eq!(failures.len(), 1, "exactly one Failed: {seen:?}");
    assert!(
        failures[0].contains(message),
        "{failures:?} lacks {message:?}"
    );
    let events = harness.events();
    assert_eq!(
        count(&events, |e| matches!(e, Event::QuadTextureCreated { .. })),
        0,
        "no further quad object was made: {events:?}"
    );
    let stats = harness.stats();
    assert_eq!(
        (stats.textures_created, stats.staging_created),
        expected,
        "{stats:?}"
    );
    assert_eq!(presenter.drawn_generation(), None, "nothing was drawn");
    drop(source);
    finish_proven(harness, &mut presenter);
}

fn a_forced_texture_allocation_failure_is_one_failed() {
    a_creation_fault_is_one_failed_and_ends_proven(
        |hooks| hooks.quad_faults.texture_allocation = true,
        "bytes for the quad's texture failed",
        (1, 0),
    );
}

fn a_forced_pipeline_failure_is_one_failed() {
    a_creation_fault_is_one_failed_and_ends_proven(
        |hooks| hooks.quad_faults.pipeline = Some(PipelineFault::Pipeline),
        "vkCreateGraphicsPipelines",
        (0, 0),
    );
}

fn a_lowered_device_limit_is_one_failed_naming_it() {
    a_creation_fault_is_one_failed_and_ends_proven(
        |hooks| hooks.quad_image_limit = Some(1),
        "over this device's maxImageDimension2D of 1",
        (0, 0),
    );
}

/// The second sampler's creation fails with the first sampler the only quad object that exists: one `Failed`, and
/// the proven end still destroys that sampler (the validation layer reports a sampler left alive at the device's
/// destruction, so the case fails on it).
fn a_failure_after_only_a_sampler_exists_is_cleaned_up() {
    a_creation_fault_is_one_failed_and_ends_proven(
        |hooks| hooks.quad_faults.pipeline = Some(PipelineFault::SecondSampler),
        "vkCreateSampler",
        (0, 0),
    );
}

/// A non-device-loss terminal failure (the surface is lost at the first present) with a staging buffer whose upload
/// proof arrives only afterwards: nothing is released while the presenter idles failed, and the proven teardown
/// destroys everything.
fn a_terminal_failure_reaps_no_quad_object_before_a_proven_teardown() {
    let mut harness = Harness::new();
    let never = Arc::new(AtomicBool::new(true));
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut hooks = harness.hooks();
    hooks.fences_never_signal = Some(Arc::clone(&never));
    hooks.present = vec![Some(vk::Result::ERROR_SURFACE_LOST_KHR)];
    let mut presenter = harness.start(hooks);
    let mut seen = Vec::new();
    harness.rig.wait("a Failed report", || {
        seen.extend(reports(&presenter));
        seen.iter()
            .any(|report| matches!(report, PresenterReport::Failed { .. }))
    });
    // The upload's submit is proven only now: the staging buffer is reapable, but rendering has failed.
    never.store(false, Ordering::SeqCst);
    harness.wait_event("the submit's proof", |e| {
        matches!(
            e,
            Event::FenceSignalled {
                proof: "submit",
                ..
            }
        )
    });
    harness.rig.idle(Duration::from_millis(400));
    let events = harness.events();
    assert_eq!(
        count(&events, is_destroyed),
        0,
        "a quad object was destroyed after the terminal failure, before the teardown: {events:?}"
    );
    let idle = harness.stats();
    assert_eq!(
        (idle.textures_released, idle.staging_released),
        (0, 0),
        "{idle:?}"
    );
    assert_eq!(
        (idle.textures_alive, idle.staging_alive),
        (1, 1),
        "{idle:?}"
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    let events = harness.events();
    assert_eq!(
        count(&events, |e| *e
            == Event::QuadStagingDestroyed { teardown: false }),
        0,
        "{events:?}"
    );
    assert_eq!(
        count(&events, |e| *e
            == Event::QuadTextureDestroyed { teardown: true }),
        1,
        "{events:?}"
    );
    assert_balanced(&harness.stats());
    drop(source);
    harness.rig.close(&ended);
}

/// The staging buffer's memory allocation fails after the buffer exists: one `Failed`, and the partial objects are
/// destroyed on the proven end (created equals released).
fn a_partial_staging_failure_is_one_failed_and_everything_is_destroyed() {
    a_creation_fault_is_one_failed_and_ends_proven(
        |hooks| hooks.quad_faults.staging_allocation = true,
        "bytes for the quad's staging buffer failed",
        (1, 1),
    );
}

/// A device loss with a resident texture and its staging buffer live: `Unproven`, and no quad object is released.
fn a_device_loss_with_quad_objects_live_releases_none() {
    let mut harness = Harness::new();
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut hooks = harness.hooks();
    hooks.present = vec![Some(vk::Result::ERROR_DEVICE_LOST)];
    let mut presenter = harness.start(hooks);
    let mut seen = Vec::new();
    harness.rig.wait("a Failed report", || {
        seen.extend(reports(&presenter));
        seen.iter()
            .any(|report| matches!(report, PresenterReport::Failed { .. }))
    });
    // Long enough for the idle service to run: with a lost device it must not reap what the proofs would allow.
    harness.rig.idle(Duration::from_millis(250));
    let before = harness.stats();
    assert_eq!(
        (before.textures_alive, before.staging_alive),
        (1, 1),
        "{before:?}"
    );
    let ended = Rig::end(&mut presenter);
    assert!(matches!(ended, CleanupResult::Unproven { .. }), "{ended:?}");
    let events = harness.events();
    super::check_nothing_destroyed(&events);
    assert_eq!(count(&events, is_destroyed), 0, "{events:?}");
    let after = harness.stats();
    assert_eq!(
        (after.textures_released, after.staging_released),
        (before.textures_released, before.staging_released),
        "no quad object was released by the loss: {after:?}"
    );
    drop(source);
    harness.rig.retire();
}

/// A panic after a submit with a quad drawn: `Unproven`, and no quad object is released.
fn a_panic_after_submit_with_a_quad_drawn_releases_none() {
    let mut harness = Harness::new();
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut hooks = harness.hooks();
    hooks.panic_after_frames = Some(1);
    let mut presenter = harness.start(hooks);
    // The panic follows the first queued frame; the stop comes after it.
    harness
        .rig
        .wait("the first frame", || harness.probes.frames() >= 1);
    let ended = Rig::end(&mut presenter);
    let CleanupResult::Unproven { reason } = &ended else {
        panic!("a panic after the submit must give Unproven, got {ended:?}");
    };
    assert!(reason.contains("panicked"), "{reason}");
    let events = harness.events();
    super::check_nothing_destroyed(&events);
    assert_eq!(count(&events, is_destroyed), 0, "{events:?}");
    let stats = harness.stats();
    assert_eq!(
        (stats.textures_released, stats.staging_released),
        (0, 0),
        "{stats:?}"
    );
    assert!(stats.textures_alive >= 1, "{stats:?}");
    drop(source);
    harness.rig.retire();
}

// ---------------------------------------------------------------------------------------------
// Upload states
// ---------------------------------------------------------------------------------------------

/// A frame abandoned after its upload was recorded: the pixel source is already released, the texture is not
/// uploaded and `drawn_generation` is unchanged; the next frame draws the quad from the kept staging bytes
/// without asking the source again.
fn a_frame_abandoned_after_recording_is_uploaded_by_the_next_from_the_kept_bytes() {
    let mut harness = Harness::new();
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let mut hooks = harness.hooks();
    hooks.abandon_after_record = 1;
    // One frame step through the gate: the abandoned one. The next waits at the gate, so what is checked below is
    // the state right after the abandon, whatever the thread would do next.
    let gate = Arc::new(Gate::new());
    hooks.gate = Some(Arc::clone(&gate));
    gate.open(1);
    let mut presenter = harness.start(hooks);
    harness.wait_event("the abandon", |e| *e == Event::ReleaseImages);
    assert_eq!(
        Arc::strong_count(&source),
        2,
        "the test and the slot hold the source; the render thread holds none"
    );
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        2,
        "the quad's check and one upload read"
    );
    assert_eq!(presenter.drawn_generation(), None, "no frame was queued");
    assert_eq!(harness.stats().uploads, 0, "the upload was not submitted");
    let events = harness.events();
    assert_eq!(
        count(&events, |e| *e == Event::QuadUploadRecorded),
        1,
        "{events:?}"
    );
    gate.open(1);
    harness.wait_generation(&presenter, 1);
    let capture = harness.capture_of(1);
    check_the_checker(&capture);
    let stats = harness.stats();
    assert_eq!(
        (stats.uploads, stats.textures_created, stats.staging_created),
        (1, 1, 1),
        "{stats:?}"
    );
    assert_eq!(
        source.reads.load(Ordering::SeqCst),
        2,
        "the second upload came from the kept staging buffer"
    );
    let events = harness.events();
    assert_eq!(
        count(&events, |e| *e == Event::QuadUploadRecorded),
        2,
        "{events:?}"
    );
    drop(source);
    finish_proven(harness, &mut presenter);
}

// ---------------------------------------------------------------------------------------------
// The surface formats
// ---------------------------------------------------------------------------------------------

/// Starts a presenter that sees only `wanted` of the formats the real surface offers, and waits for its choice. The
/// case fails, naming what the surface offered, if none of `wanted` is among them.
fn start_with_formats(
    harness: &mut Harness,
    wanted: &[vk::Format],
    configure: impl FnOnce(&mut TestHooks),
) -> (VulkanPresenter, vk::Format) {
    let mut hooks = harness.hooks();
    hooks.forced_surface_formats = Some(wanted.to_vec());
    configure(&mut hooks);
    let presenter = harness.start(hooks);
    harness.wait_event("the format choice", |e| {
        matches!(e, Event::Formats { .. }) || matches!(e, Event::Failed(_))
    });
    let events = harness.events();
    let Some(Event::Formats { offered, chosen }) = events
        .iter()
        .find(|e| matches!(e, Event::Formats { .. }))
        .cloned()
    else {
        panic!("the surface offers none of {wanted:?} (failed set-up): {events:?}");
    };
    assert!(
        wanted.contains(&chosen),
        "the surface offers none of {wanted:?}; it offered {offered:?}"
    );
    (presenter, chosen)
}

/// The preferred `UNORM` format draws the quad: known-channel alpha results on the surface path.
fn the_preferred_unorm_format_draws_the_quad() {
    let mut harness = Harness::new();
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let (mut presenter, chosen) =
        start_with_formats(&mut harness, &[vk::Format::B8G8R8A8_UNORM], |_| {});
    assert_eq!(chosen, vk::Format::B8G8R8A8_UNORM);
    harness.wait_generation(&presenter, 1);
    let capture = harness.capture_of(1);
    assert_eq!(capture.format, vk::Format::B8G8R8A8_UNORM);
    check_the_checker(&capture);
    assert!(
        presenter.take_quad_notice().is_none(),
        "no notice for a supported format"
    );
    drop(source);
    finish_proven(harness, &mut presenter);
}

/// A format the quad cannot be drawn on: one notice, the plain clear, no quad object, and the wake ran after the
/// notice. `uniform`: the capture shows the same colour everywhere (the quad's place included).
fn a_format_without_the_quad_gives_one_notice_and_the_plain_clear(wanted: &[vk::Format]) {
    let mut harness = Harness::new();
    let hold = Arc::new(AtomicBool::new(true));
    let source = checker();
    harness.publish(1, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    let (mut presenter, chosen) = start_with_formats(&mut harness, wanted, |hooks| {
        hooks.hold_acquire = Some(Arc::clone(&hold));
    });
    // The frame is held before its acquire, so no report has been made: the only wake so far is the notice's.
    let wakes = Arc::clone(&harness.probes.wakes);
    harness
        .rig
        .wait("the notice's wake", || wakes.load(Ordering::SeqCst) >= 1);
    assert_eq!(harness.probes.frames(), 0, "no frame was queued yet");
    assert_eq!(
        presenter.take_quad_notice(),
        Some(QuadSkipped { format: chosen }),
        "the notice names the surface format"
    );
    assert_eq!(presenter.take_quad_notice(), None, "taken once");
    hold.store(false, Ordering::SeqCst);
    harness.wait_generation(&presenter, 1);
    harness.publish(2, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    harness.wait_generation(&presenter, 2);
    assert_eq!(presenter.take_quad_notice(), None, "once per run");
    let capture = harness.capture_of(2);
    let bytes_per_pixel = capture.bytes.len() / (capture.size.0 * capture.size.1) as usize;
    let at = |x: u32, y: u32| {
        let offset = (y * capture.size.0 + x) as usize * bytes_per_pixel;
        capture.bytes[offset..offset + bytes_per_pixel].to_vec()
    };
    assert_eq!(
        at(45, 35),
        at(10, 10),
        "the quad's place shows the plain clear"
    );
    assert_eq!(
        at(55, 45),
        at(10, 10),
        "the quad's place shows the plain clear"
    );
    let events = harness.events();
    assert_eq!(
        count(&events, |e| matches!(e, Event::QuadSkipped(_))),
        1,
        "{events:?}"
    );
    assert_eq!(
        count(&events, |e| matches!(e, Event::QuadTextureCreated { .. })),
        0,
        "{events:?}"
    );
    assert_eq!(
        count(&events, |e| *e == Event::QuadPipelineCreated),
        0,
        "an unsupported format creates no quad object, so it cannot fail set-up: {events:?}"
    );
    assert_eq!(harness.stats().textures_created, 0);
    drop(source);
    finish_proven(harness, &mut presenter);
}

fn an_srgb_only_subset_gives_one_notice_and_the_plain_clear() {
    a_format_without_the_quad_gives_one_notice_and_the_plain_clear(&[vk::Format::B8G8R8A8_SRGB]);
}

fn an_offered_format_outside_the_supported_pair_gives_one_notice_and_the_plain_clear() {
    a_format_without_the_quad_gives_one_notice_and_the_plain_clear(&[
        vk::Format::A2B10G10R10_UNORM_PACK32,
        vk::Format::R16G16B16A16_SFLOAT,
    ]);
}

// ---------------------------------------------------------------------------------------------
// Start, with and without a slot; the slot after the end
// ---------------------------------------------------------------------------------------------

/// `start` without a slot behaves exactly as today: clear only, no quad object, no generation, no notice.
fn start_without_a_slot_is_clear_only() {
    let mut harness = Harness::new();
    let mut presenter = harness.start(harness.probes.hooks());
    harness
        .rig
        .wait("the first frame", || harness.probes.frames() >= 1);
    assert_eq!(presenter.drawn_generation(), None);
    assert_eq!(presenter.take_quad_notice(), None);
    let events = harness.events();
    assert_eq!(
        count(&events, |e| matches!(
            e,
            Event::QuadPipelineCreated
                | Event::QuadTextureCreated { .. }
                | Event::QuadSkipped(_)
                | Event::Formats { .. }
        )),
        0,
        "{events:?}"
    );
    finish_proven(harness, &mut presenter);
}

/// A publish after the presenter ended only stores the value: no wake, no GPU object, no generation.
fn a_publish_after_the_end_only_stores_the_value() {
    let mut harness = Harness::new();
    let mut presenter = harness.start(harness.hooks());
    harness.wait_generation(&presenter, 0);
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    let wakes = harness.probes.wakes.load(Ordering::SeqCst);
    let source = checker();
    harness.publish(9, Some(quad(&source, 2, 2, VulkanFilter::Nearest)));
    drop(presenter);
    harness.publish(10, None);
    assert_eq!(
        harness.probes.wakes.load(Ordering::SeqCst),
        wakes,
        "nothing woke"
    );
    assert_eq!(harness.stats().textures_created, 0);
    drop(source);
    harness.rig.close(&ended);
}

pub(super) const CASES: [(&str, Case); 23] = [
    (
        "quad_published_before_start_is_in_the_first_frame",
        quad_published_before_start_is_in_the_first_frame,
    ),
    (
        "a_scene_with_no_quad_draws_only_the_clear",
        a_scene_with_no_quad_draws_only_the_clear,
    ),
    (
        "publishing_during_a_stalled_frame_allocates_nothing",
        publishing_during_a_stalled_frame_allocates_nothing,
    ),
    (
        "replacing_the_image_repeatedly_keeps_the_cap",
        replacing_the_image_repeatedly_keeps_the_cap,
    ),
    (
        "a_retired_texture_is_destroyed_only_after_its_last_submits_proof",
        a_retired_texture_is_destroyed_only_after_its_last_submits_proof,
    ),
    (
        "a_move_and_a_filter_change_do_not_upload",
        a_move_and_a_filter_change_do_not_upload,
    ),
    (
        "the_render_thread_holds_no_pixel_source_after_its_upload",
        the_render_thread_holds_no_pixel_source_after_its_upload,
    ),
    (
        "a_proven_end_releases_every_quad_object",
        a_proven_end_releases_every_quad_object,
    ),
    (
        "an_unproven_end_releases_no_quad_object",
        an_unproven_end_releases_no_quad_object,
    ),
    (
        "a_stop_while_a_texture_is_retired_destroys_it_only_in_the_teardown",
        a_stop_while_a_texture_is_retired_destroys_it_only_in_the_teardown,
    ),
    (
        "a_forced_texture_allocation_failure_is_one_failed",
        a_forced_texture_allocation_failure_is_one_failed,
    ),
    (
        "a_forced_pipeline_failure_is_one_failed",
        a_forced_pipeline_failure_is_one_failed,
    ),
    (
        "a_lowered_device_limit_is_one_failed_naming_it",
        a_lowered_device_limit_is_one_failed_naming_it,
    ),
    (
        "a_failure_after_only_a_sampler_exists_is_cleaned_up",
        a_failure_after_only_a_sampler_exists_is_cleaned_up,
    ),
    (
        "a_terminal_failure_reaps_no_quad_object_before_a_proven_teardown",
        a_terminal_failure_reaps_no_quad_object_before_a_proven_teardown,
    ),
    (
        "a_partial_staging_failure_is_one_failed_and_everything_is_destroyed",
        a_partial_staging_failure_is_one_failed_and_everything_is_destroyed,
    ),
    (
        "a_device_loss_with_quad_objects_live_releases_none",
        a_device_loss_with_quad_objects_live_releases_none,
    ),
    (
        "a_panic_after_submit_with_a_quad_drawn_releases_none",
        a_panic_after_submit_with_a_quad_drawn_releases_none,
    ),
    (
        "a_frame_abandoned_after_recording_is_uploaded_by_the_next_from_the_kept_bytes",
        a_frame_abandoned_after_recording_is_uploaded_by_the_next_from_the_kept_bytes,
    ),
    (
        "the_preferred_unorm_format_draws_the_quad",
        the_preferred_unorm_format_draws_the_quad,
    ),
    (
        "an_srgb_only_subset_gives_one_notice_and_the_plain_clear",
        an_srgb_only_subset_gives_one_notice_and_the_plain_clear,
    ),
    (
        "an_offered_format_outside_the_supported_pair_gives_one_notice_and_the_plain_clear",
        an_offered_format_outside_the_supported_pair_gives_one_notice_and_the_plain_clear,
    ),
    (
        "start_without_a_slot_is_clear_only",
        start_without_a_slot_is_clear_only,
    ),
];

/// The case that runs after the others in the list (the array above is a fixed size).
pub(super) const LAST: [(&str, Case); 1] = [(
    "a_publish_after_the_end_only_stores_the_value",
    a_publish_after_the_end_only_stores_the_value,
)];
