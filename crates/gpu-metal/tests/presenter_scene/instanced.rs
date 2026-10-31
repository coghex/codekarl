//! The instanced scene on a real window (CK-116): the checker's startup contract, resolution and batching through the
//! frame's ticket, the instance buffer's reuse argument, the budget and the bounds, and the pixels of the frames the
//! presenter really draws (captured off screen by the presenter's own `capture_frames` hook, which encodes the same frame a
//! second time into a shared texture). The cases use the CK-114 test driver of the parent module. Each names its negative
//! control.

use std::sync::atomic::AtomicI32;
use std::sync::{Arc, Mutex};

use ck_gpu_metal::{
    Boundary, CapturedFrame, DestinationRect, LedgerEvent, MetalContext, MetalFilter, MetalFrame,
    MetalFrameSlot, MetalInstance, MetalQuad, MetalSceneSource, PassOutcome, PixelSource,
    SceneLimits, SceneSetup, SourceRect, TextureDriver, TextureLedger, TextureLimits,
    TextureTestHooks,
};

use super::{
    CleanupResult, Duration, PresenterReport, Probes, Rig, Scene, TestHooks, end, limits,
    limits_adopting, lock, reports_until, scene_limits, until,
};
use std::sync::atomic::Ordering;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
/// The clear colour of the rig's presenters, [0.1, 0.2, 0.3], as bytes.
const CLEAR: [u8; 4] = [26, 51, 77, 255];

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> DestinationRect {
    DestinationRect {
        left,
        top,
        right,
        bottom,
    }
}

fn instance(key: u32, destination: DestinationRect) -> MetalInstance<u32> {
    MetalInstance {
        key,
        destination,
        source: WHOLE,
        role: ck_gpu_metal::InstanceRole::Scene,
    }
}

/// A source that emits a fixed list and records the size it was asked to place for.
struct Fixed {
    instances: Vec<MetalInstance<u32>>,
    asked: Arc<Mutex<Vec<(u32, u32)>>>,
}

impl MetalSceneSource<u32> for Fixed {
    fn place(&self, width: u32, height: u32, out: &mut dyn FnMut(MetalInstance<u32>)) {
        lock(&self.asked).push((width, height));
        for instance in &self.instances {
            out(*instance);
        }
    }
}

fn source(instances: Vec<MetalInstance<u32>>) -> Arc<Fixed> {
    Arc::new(Fixed {
        instances,
        asked: Arc::new(Mutex::new(Vec::new())),
    })
}

fn publish(
    scene: &Scene,
    generation: u64,
    quad: Option<MetalQuad>,
    instances: Vec<MetalInstance<u32>>,
) {
    scene.frames.publish(MetalFrame::new(
        generation,
        MetalFilter::Nearest,
        quad,
        Some(source(instances)),
    ));
}

/// A grid of `count` 1 x 1 instances of the keys `key(i)`.
fn grid(count: u32, key: impl Fn(u32) -> u32) -> Vec<MetalInstance<u32>> {
    (0..count)
        .map(|index| {
            let (x, y) = (
                i32::try_from(index % 40).expect("small"),
                i32::try_from(index / 40).expect("small"),
            );
            instance(key(index), rect(x, y, x + 1, y + 1))
        })
        .collect()
}

/// The RGBA pixel at (x, y) of a captured (BGRA) frame.
fn px(frame: &CapturedFrame, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * frame.width + x) * 4) as usize;
    [
        frame.pixels[offset + 2],
        frame.pixels[offset + 1],
        frame.pixels[offset],
        frame.pixels[offset + 3],
    ]
}

/// Waits (pumping) until the observer has `captures` completed captures and returns the latest.
fn captured(rig: &mut Rig, scene: &Scene, captures: i64) -> CapturedFrame {
    until(rig, "the captured frame", || {
        scene.probes.observer.captures() >= captures
    });
    scene
        .probes
        .observer
        .captured_frame()
        .expect("a captured frame")
}

/// Waits (pumping) for the `index`-th (0-based) captured frame and returns it.
fn captured_at(rig: &mut Rig, scene: &Scene, index: u64) -> CapturedFrame {
    let wanted = i64::try_from(index).expect("small") + 1;
    until(rig, "the captured frame", || {
        scene.probes.observer.captures() >= wanted
    });
    scene
        .probes
        .observer
        .captured_frame_at(u32::try_from(index).expect("small"))
        .expect("a kept capture")
}

fn capturing(hooks: &mut TestHooks) {
    hooks.capture_frames = true;
}

/// An 8 x 8 image of one colour for the driver to offer, adopted for `key` at `revision`.
fn offer(scene: &Scene, key: u32, revision: u64, color: [u8; 4]) {
    let mut state = scene.driver();
    state.offers.push_back((key, revision, 8, 8));
    state.colors.push((key, revision, color));
}

// ---------------------------------------------------------------------------------------------
// The checker's startup contract
// ---------------------------------------------------------------------------------------------

/// A bindless scene keeps the checker resident before its first frame and removes it before releasing the texture.
/// Negative control: making the frame eligible before the checker membership commit leaves no first-frame member;
/// releasing the checker before removal reverses the native lifecycle order marks.
pub(crate) fn scene_bindless_checker_residency_lifecycle() {
    let mut rig = Rig::open();
    let mut scene = Scene::new();
    scene.bindless_limits = Some(ck_gpu_metal::BindlessLimits {
        texture_slots: 4,
        max_textures: 2,
        max_extra_revisions: 1,
    });
    let gate = Arc::new(AtomicI32::new(0));
    scene.checker_hooks = Some(TextureTestHooks {
        completion_gate: Some(Arc::clone(&gate)),
        ..TextureTestHooks::default()
    });
    let baseline = scene.context.native_live_objects();
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    rig.pump_for(Duration::from_millis(500));
    let pending = scene.context.native_live_objects();
    assert_eq!(
        scene.probes.frames(),
        0,
        "no frame while checker upload is held"
    );
    assert_eq!(pending.residency_sets, baseline.residency_sets + 1);
    assert_eq!(pending.residency_members, baseline.residency_members + 2);

    gate.store(1, Ordering::SeqCst);
    until(&mut rig, "the first bindless frame", || {
        scene.probes.frames() >= 1
    });
    assert!(presenter.stats().checker_resident);
    assert!(presenter.stats().residency_set_bytes > 0);
    assert!(presenter.stats().bindless_table_bytes >= 2 * 4 * 8);
    let resident = scene.context.native_live_objects();
    assert_eq!(resident.residency_members, baseline.residency_members + 3);

    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    let ended = scene.context.native_live_objects();
    assert_eq!(ended.residency_members, baseline.residency_members);
    assert_eq!(ended.residency_sets, baseline.residency_sets);
    assert!(ended.last_residency_remove_order > baseline.last_residency_remove_order);
    assert!(ended.last_texture_release_order > ended.last_residency_remove_order);
}

/// The window is open and the presenter started, yet **no frame is committed until the checker is resident**: while its
/// upload is held, no frame step is even attempted (the render thread waits on its condition), no report is made; its
/// completion alone, with no other redraw, runs the first frame, and that frame shows the checker for the published
/// unresolved quads. Negative control: letting a clear-only frame through shows a first frame with no checker (the frame
/// count moves while the upload is held).
pub(crate) fn scene_checker_gates_the_first_frame() {
    let mut rig = Rig::open();
    let mut scene = Scene::new();
    scene.bindless_limits = Some(ck_gpu_metal::BindlessLimits {
        texture_slots: 4,
        max_textures: 2,
        max_extra_revisions: 1,
    });
    let gate = Arc::new(AtomicI32::new(0));
    scene.checker_hooks = Some(TextureTestHooks {
        completion_gate: Some(Arc::clone(&gate)),
        ..TextureTestHooks::default()
    });
    let native_baseline = scene.context.native_live_objects();
    // Published before the presenter starts: it is in the first frame.
    publish(
        &scene,
        3,
        None,
        vec![
            instance(5, rect(0, 0, 64, 64)),
            instance(6, rect(64, 0, 128, 64)),
        ],
    );
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    rig.pump_for(Duration::from_millis(500));
    assert_eq!(
        scene.probes.frames(),
        0,
        "no frame while the checker is pending"
    );
    assert_eq!(
        scene.probes.observer.frame_steps(),
        0,
        "not even a frame step was attempted"
    );
    assert_eq!(scene.probes.observer.committed(), 0);
    assert_eq!(presenter.drawn_generation(), None);
    assert!(presenter.take_report().is_none(), "no FirstFrameQueued yet");
    assert!(!presenter.stats().checker_resident);
    assert_eq!(scene.probes.observer.captures(), 0);
    assert_eq!(
        scene.context.native_live_objects().residency_members,
        native_baseline.residency_members + 2
    );
    // The checker's completion alone: nothing else asks for a frame.
    gate.store(1, Ordering::SeqCst);
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    assert_eq!(
        scene.context.native_live_objects().residency_members,
        native_baseline.residency_members + 3,
        "the checker membership was committed before the first frame became eligible",
    );
    assert_eq!(
        presenter.take_report(),
        Some(PresenterReport::FirstFrameQueued)
    );
    let stats = presenter.stats();
    assert!(stats.checker_resident);
    assert_eq!(
        (
            stats.last.instances,
            stats.last.checker,
            stats.last.resolved
        ),
        (2, 2, 0),
        "the first committed frame is the first scene-bearing frame, with checker instances"
    );
    assert_eq!(presenter.drawn_generation(), Some(3));
    let frame = captured(&mut rig, &scene, 1);
    let (width, height) = rig.pixels;
    assert_eq!((frame.width, frame.height), (width, height));
    // Two 64 x 64 checkers side by side: cells of 8 pixels, the top-left cell magenta.
    assert_eq!(px(&frame, 4, 4), MAGENTA);
    assert_eq!(px(&frame, 12, 4), BLACK);
    assert_eq!(px(&frame, 4, 12), BLACK);
    assert_eq!(px(&frame, 12, 12), MAGENTA);
    assert_eq!(px(&frame, 64 + 4, 4), MAGENTA);
    assert_eq!(px(&frame, 64 + 12, 4), BLACK);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A checker upload that fails - at its creation, its commit, or by a completion error - is one `Failed` report, nothing
/// drawn, and a proven end; it can never leave the first frame waiting for ever. Negative control: ignoring a failed
/// upload leaves the first frame waiting (no report is made) and the case times out.
pub(crate) fn scene_checker_upload_failure() {
    type Injection = fn(&mut TextureTestHooks);
    let cases: [(&str, Injection); 4] = [
        ("a completion error", |hooks| hooks.error_completion = true),
        ("a refused commit", |hooks| hooks.fail_commit = true),
        ("no texture", |hooks| {
            hooks.fail_texture_allocation_at = Some(1);
        }),
        ("no staging buffer", |hooks| hooks.nil_staging = true),
    ];
    for (what, inject) in cases {
        let mut rig = Rig::open();
        let mut scene = Scene::new();
        let mut hooks = TextureTestHooks::default();
        inject(&mut hooks);
        scene.checker_hooks = Some(hooks);
        publish(&scene, 1, None, vec![instance(5, rect(0, 0, 64, 64))]);
        let mut presenter = scene.start(&rig, limits(), None, capturing);
        let mut reports = Vec::new();
        assert!(
            reports_until(&mut rig, &presenter, &mut reports, 1),
            "{what}: no report"
        );
        rig.pump_for(Duration::from_millis(300));
        while let Some(report) = presenter.take_report() {
            reports.push(report);
        }
        let failures = reports
            .iter()
            .filter(|report| matches!(report, PresenterReport::Failed { .. }))
            .count();
        assert_eq!(
            failures, 1,
            "{what}: exactly one Failed report: {reports:?}"
        );
        assert!(
            !reports.contains(&PresenterReport::FirstFrameQueued),
            "{what}: {reports:?}"
        );
        assert_eq!(scene.probes.frames(), 0, "{what}: nothing is drawn");
        assert_eq!(scene.probes.observer.committed(), 0, "{what}");
        assert_eq!(
            end(&mut rig, &mut presenter),
            CleanupResult::Proven,
            "{what}"
        );
    }
}

/// A stop while the checker is uploading ends through the combined proof: with the completion released once the teardown
/// has begun it is `Proven`, and with it held past the budget everything is retained (`Unproven`, naming the checker's
/// upload), and in neither case was a frame committed. Negative control: a teardown that ignores the checker's token
/// ends while the completion is still held (the first half's `poll_end` check) and releases the staging buffer and
/// texture under a pending blit (the second half).
pub(crate) fn scene_checker_stop_while_uploading() {
    // The completion is held until the teardown has begun, then released: `Proven`, and no frame was ever committed.
    {
        let mut rig = Rig::open();
        let mut scene = Scene::new();
        let gate = Arc::new(AtomicI32::new(0));
        scene.checker_hooks = Some(TextureTestHooks {
            completion_gate: Some(Arc::clone(&gate)),
            ..TextureTestHooks::default()
        });
        let mut presenter = scene.start(&rig, limits(), None, |hooks| {
            hooks.stop_budget = Some(Duration::from_secs(20));
        });
        presenter.request_stop();
        // The phase under test: the teardown is under way (the driver's last pass ran) with the completion still held.
        until(&mut rig, "the teardown's last pass", || {
            scene.count(Boundary::Ending) >= 1
        });
        // A teardown that ignored the held token would have ended by now (the negative control).
        rig.pump_for(Duration::from_millis(150));
        assert_eq!(
            presenter.poll_end(Duration::ZERO),
            None,
            "the proof must wait for the checker's completion"
        );
        gate.store(1, Ordering::SeqCst);
        assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
        assert_eq!(scene.probes.frames(), 0);
        assert_eq!(scene.probes.observer.committed(), 0);
        assert_eq!(
            *lock(&scene.releases),
            ["store_and_ledger", "presenter", "context"]
        );
    }
    // Held past the budget: retained, never released under a pending blit.
    {
        let mut rig = Rig::open();
        rig.keep_alive = true;
        let mut scene = Scene::new();
        let gate = Arc::new(AtomicI32::new(0));
        scene.checker_hooks = Some(TextureTestHooks {
            completion_gate: Some(Arc::clone(&gate)),
            ..TextureTestHooks::default()
        });
        let mut presenter = scene.start(&rig, limits(), None, |hooks| {
            hooks.stop_budget = Some(Duration::from_millis(300));
        });
        presenter.request_stop();
        let result = end(&mut rig, &mut presenter);
        let CleanupResult::Unproven { reason } = result else {
            panic!("a held checker upload cannot be proven: {result:?}");
        };
        assert!(reason.contains("checker"), "{reason}");
        assert_eq!(scene.probes.frames(), 0);
        assert!(lock(&scene.releases).is_empty(), "nothing was released");
        gate.store(1, Ordering::SeqCst);
    }
}

/// The checker is charged once to the presenter's baseline - reported in `stats()` - and never to the store's texture
/// bytes, staging leases, prepared window or any ledger charge; the instance buffer's charge is its measured allocation.
/// Negative control: charging the checker through the ledger puts bytes into `texture_bytes_measured` and fails.
pub(crate) fn scene_checker_is_a_baseline_charge() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    publish(&scene, 1, None, vec![instance(1, rect(0, 0, 8, 8))]);
    let mut presenter = scene.start(&rig, limits(), None, |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    // A second frame so that a boundary pass has looked at the counters with the checker resident.
    publish(&scene, 2, None, vec![instance(1, rect(0, 0, 8, 8))]);
    until(&mut rig, "the second frame", || scene.probes.frames() >= 2);
    let stats = presenter.stats();
    assert!(stats.checker_resident);
    assert!(stats.checker_texture_bytes >= 64 * 64 * 4, "{stats:?}");
    assert!(stats.checker_staging_bytes >= 64 * 64 * 4, "{stats:?}");
    let ledger = scene
        .driver()
        .stats
        .clone()
        .expect("the driver saw the ledger");
    assert_eq!(ledger.upload.texture_bytes_reserved, 0);
    assert_eq!(ledger.upload.texture_bytes_measured, 0);
    assert_eq!(ledger.upload.live_textures, 0);
    assert_eq!(ledger.upload.staging_bytes, 0);
    assert_eq!(ledger.upload.prepared_images, 0);
    assert_eq!(ledger.upload.batches_submitted, 0);
    assert_eq!(ledger.baseline_charges, 0);
    assert_eq!(ledger.extra_revisions, 0);
    // The instance buffer: its charge is the measured allocation, which holds at least the logical product.
    // The record is 48 bytes (the bindless slot and its padding): 16 more than the ordinary path's first 32.
    assert_eq!(stats.instance_buffer_bytes, 1024 * 48);
    assert!(
        stats.instance_buffer_allocated >= stats.instance_buffer_bytes,
        "{stats:?}"
    );
    assert_eq!(stats.binding_geometry_bytes, 128 * 1024 * 1024);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

// ---------------------------------------------------------------------------------------------
// Resolution, batching and the pixels of real frames
// ---------------------------------------------------------------------------------------------

/// Adopted revisions are drawn with CK-114's redraw rule: with no other redraw a committed revision is adopted and the
/// frame the rule makes wanted shows its pixels (checker before, the image after; a newer revision of the key after
/// that); and with one adoption per boundary, three committed revisions are shown in turn, one per frame. Negative
/// control: drawing the checker for a resolved key, or resolving a stale revision, fails the pixel checks.
pub(crate) fn scene_draws_adopted_revisions() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().adopt_limit = 1;
    // Four 32 x 32 cells in a row, keys 7, 8, 9, 10.
    let cells: Vec<MetalInstance<u32>> = (0..4_i32)
        .map(|i| {
            instance(
                7 + u32::try_from(i).expect("small"),
                rect(32 * i, 0, 32 * (i + 1), 32),
            )
        })
        .collect();
    publish(&scene, 1, None, cells);
    let mut presenter = scene.start(&rig, limits_adopting(1), None, capturing);
    // Frame 1: nothing is shown yet, so every cell is the checker.
    until(&mut rig, "the first frame", || {
        scene.probes.observer.captures() >= 1
    });
    let frame = captured_at(&mut rig, &scene, 0);
    for cell in 0..4 {
        assert_eq!(
            px(&frame, 32 * cell + 4, 4),
            MAGENTA,
            "cell {cell} is the checker"
        );
    }
    rig.pump_for(Duration::from_millis(200));
    let settled = scene.probes.frames();
    // Key 7 is offered and committed: the rule wants a frame for it, and no redraw is asked for.
    offer(&scene, 7, 1, RED);
    presenter.texture_wake()();
    until(&mut rig, "the frame for key 7", || {
        scene.probes.frames() > settled
    });
    rig.pump_for(Duration::from_millis(200));
    assert_eq!(
        scene.probes.frames(),
        settled + 1,
        "exactly the frame the rule made wanted"
    );
    let frame = captured_at(&mut rig, &scene, settled);
    assert_eq!(px(&frame, 4, 4), RED, "the adopted revision of key 7");
    assert_eq!(px(&frame, 32 + 4, 4), MAGENTA);
    // A newer revision of the same key replaces it.
    offer(&scene, 7, 2, BLUE);
    presenter.texture_wake()();
    until(&mut rig, "the frame for revision 2", || {
        scene.probes.frames() > settled + 1
    });
    rig.pump_for(Duration::from_millis(200));
    assert_eq!(scene.probes.frames(), settled + 2);
    let frame = captured_at(&mut rig, &scene, settled + 1);
    assert_eq!(px(&frame, 4, 4), BLUE, "the new revision, not the old one");
    // Three committed revisions, one adoption per boundary: three frames, each showing one more.
    for (key, color) in [(8, GREEN), (9, WHITE), (10, RED)] {
        offer(&scene, key, 1, color);
    }
    presenter.texture_wake()();
    until(&mut rig, "three more frames", || {
        scene.probes.frames() >= settled + 5
    });
    rig.pump_for(Duration::from_millis(300));
    assert_eq!(
        scene.probes.frames(),
        settled + 5,
        "one frame per adopted revision, then idle"
    );
    for (turn, shown) in [(2_u64, 1_usize), (3, 2), (4, 3)] {
        let frame = captured_at(&mut rig, &scene, settled + turn);
        let resolved = (1..4)
            .filter(|&cell| px(&frame, 32 * cell + 4, 4) != MAGENTA)
            .count();
        assert_eq!(
            resolved, shown,
            "frame {turn}: one more adopted revision is shown"
        );
    }
    let frame = captured_at(&mut rig, &scene, settled + 4);
    assert_eq!(
        [
            px(&frame, 4, 4),
            px(&frame, 36, 4),
            px(&frame, 68, 4),
            px(&frame, 100, 4)
        ],
        [BLUE, GREEN, WHITE, RED]
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// Batching and resolution through the real frame: one thousand instances of one shown texture are one instanced draw and
/// one pin; one thousand alternating two shown textures are a thousand draws; the counters say so, and a repeated key is
/// pinned once (the ledger's live pins while the frame is in flight). Negative control: sorting by texture, or pinning per
/// instance, changes the counters.
pub(crate) fn scene_batches_and_pins_once() {
    let mut rig = Rig::open();
    let mut scene = Scene::new();
    scene.bounds = SceneLimits {
        max_quads: 2048,
        binding_geometry_bytes: 128 * 1024 * 1024,
    };
    offer(&scene, 1, 1, RED);
    offer(&scene, 2, 1, BLUE);
    let mut presenter = scene.start(&rig, limits(), None, |hooks| {
        // Frames complete slowly, so a boundary pass sees the previous frame's ticket still pinned.
        hooks.completion_delay = Duration::from_millis(250);
    });
    presenter.texture_wake()();
    until(&mut rig, "both adopted", || {
        scene.driver().adopted.iter().map(Vec::len).sum::<usize>() >= 2
    });
    publish(&scene, 10, None, grid(1000, |_| 1));
    until(&mut rig, "the one-texture frame", || {
        presenter.drawn_generation() == Some(10)
    });
    let one = presenter.stats();
    assert_eq!(
        (
            one.last.instances,
            one.last.batches,
            one.last.texture_binds,
            one.last.resolved,
            one.last.checker
        ),
        (1000, 1, 1, 1000, 0),
        "{one:?}"
    );
    assert_eq!(one.last.pinned, 1, "one key, many instances: one pin");
    publish(&scene, 11, None, grid(1000, |index| 1 + index % 2));
    until(&mut rig, "the alternating frame", || {
        presenter.drawn_generation() == Some(11)
    });
    let alternating = presenter.stats();
    assert_eq!(
        (
            alternating.last.instances,
            alternating.last.batches,
            alternating.last.texture_binds
        ),
        (1000, 1000, 1000),
        "{alternating:?}"
    );
    assert_eq!(alternating.last.pinned, 2);
    // A smaller frame after it: the latest value follows, the peak does not.
    publish(&scene, 12, None, grid(5, |_| 1));
    until(&mut rig, "the small frame", || {
        presenter.drawn_generation() == Some(12)
    });
    let small = presenter.stats();
    assert_eq!(
        (small.last.instances, small.last.batches),
        (5, 1),
        "{small:?}"
    );
    assert_eq!(
        (small.peak.instances, small.peak.batches, small.peak.pinned),
        (1000, 1000, 2),
        "the peak keeps the largest of each counter: {small:?}"
    );
    assert!(small.frames >= 3);
    // The ledger counted one pin per revision, not per instance, at a boundary pass while the frame was in flight.
    assert!(
        scene.driver().max_live_pins <= 2,
        "live pins: {}",
        scene.driver().max_live_pins
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// Moving every instance for many frames leaves the store's upload and texture counters unchanged: only instance data
/// changes. Negative control: re-uploading a texture per frame moves `batches_submitted`.
pub(crate) fn scene_movement_uploads_nothing() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    offer(&scene, 1, 1, RED);
    let mut presenter = scene.start(&rig, limits(), None, |_| {});
    presenter.texture_wake()();
    until(&mut rig, "adopted", || {
        scene.driver().adopted.iter().map(Vec::len).sum::<usize>() >= 1
    });
    publish(&scene, 1, None, grid(200, |_| 1));
    until(&mut rig, "the first frame", || {
        presenter.drawn_generation() == Some(1)
    });
    let before = scene
        .driver()
        .stats
        .clone()
        .expect("a pass saw the ledger")
        .upload;
    for generation in 2..60_i32 {
        let shifted: Vec<MetalInstance<u32>> = (0..200)
            .map(|i| {
                instance(
                    1,
                    rect(
                        generation + i % 40,
                        i / 40,
                        generation + i % 40 + 1,
                        i / 40 + 1,
                    ),
                )
            })
            .collect();
        publish(
            &scene,
            u64::try_from(generation).expect("positive"),
            None,
            shifted,
        );
        until(&mut rig, "a moved frame", || {
            presenter.drawn_generation() == Some(u64::try_from(generation).expect("positive"))
        });
    }
    rig.pump_for(Duration::from_millis(100));
    let after = scene
        .driver()
        .stats
        .clone()
        .expect("a pass saw the ledger")
        .upload;
    assert_eq!(
        after.batches_submitted, before.batches_submitted,
        "no upload"
    );
    assert_eq!(after.batches_prepared, before.batches_prepared);
    assert_eq!(after.live_textures, before.live_textures);
    assert_eq!(after.texture_bytes_measured, before.texture_bytes_measured);
    assert_eq!(
        scene.probes.observer.quad_textures_created(),
        0,
        "and no legacy texture either"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A source that carries its own metrics places with them whatever size the presenter passes (the passed size is the
/// drawable's, informational for such a source), and the shader's viewport is always the actual drawable: a quad placed in
/// drawable pixels fills exactly its share of the capture. Negative control: placing with the passed size, or a viewport
/// taken from the source's metrics, moves the quad.
pub(crate) fn scene_places_with_its_own_metrics() {
    /// Places one checker quad at a position of its own 100 x 100 metrics, ignoring the arguments.
    struct Own {
        asked: Arc<Mutex<Vec<(u32, u32)>>>,
    }

    impl MetalSceneSource<u32> for Own {
        fn place(&self, width: u32, height: u32, out: &mut dyn FnMut(MetalInstance<u32>)) {
            lock(&self.asked).push((width, height));
            out(instance(1, rect(10, 10, 74, 74)));
        }
    }

    let mut rig = Rig::open();
    let scene = Scene::new();
    let asked = Arc::new(Mutex::new(Vec::new()));
    scene.frames.publish(MetalFrame::new(
        1,
        MetalFilter::Nearest,
        None,
        Some(Arc::new(Own {
            asked: Arc::clone(&asked),
        })),
    ));
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    let frame = {
        until(&mut rig, "the frame", || scene.probes.frames() >= 1);
        captured(&mut rig, &scene, 1)
    };
    // The presenter passed the drawable's size; the source ignored it; the quad is where the source's own metrics put it.
    let (width, height) = rig.pixels;
    assert!(
        lock(&asked).iter().all(|size| *size == (width, height)),
        "{:?}",
        lock(&asked)
    );
    assert_eq!(px(&frame, 10 + 4, 10 + 4), MAGENTA);
    assert_eq!(px(&frame, 10 + 12, 10 + 4), BLACK);
    assert_eq!(px(&frame, 9, 9), CLEAR, "outside the quad is the clear");
    assert_eq!(px(&frame, 74, 74), CLEAR);
    // The viewport is the drawable: a quad over the top half of the drawable fills exactly rows 0..height/2.
    let half = i32::try_from(height / 2).expect("small");
    publish(
        &scene,
        2,
        None,
        vec![instance(
            1,
            rect(0, 0, i32::try_from(width).expect("small"), half),
        )],
    );
    until(&mut rig, "the second frame", || {
        presenter.drawn_generation() == Some(2)
    });
    let frame = captured(&mut rig, &scene, 2);
    let row = |y: u32| (0..width).any(|x| px(&frame, x, y) == MAGENTA);
    assert!(
        row(0) && row(height / 2 - 1),
        "the quad reaches the top and the middle"
    );
    assert!(!row(height / 2 + 2), "and stops at half of the drawable");
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// The one instance buffer is written only once the previous frame's command buffer has completed. With the completion
/// held, the next frame step finds the buffer in use, writes nothing into it, and draws after the completion arrives.
/// Negative control: checking after the first write (or not at all) counts the second frame's instances as written while
/// the first frame's completion is still pending.
pub(crate) fn scene_instance_buffer_is_written_only_when_free() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    publish(&scene, 1, None, grid(10, |_| 5));
    // Every committed frame's completion is held until the test opens the gate: the ordering under test is established by
    // the state observed, not by elapsed time.
    let gate = Arc::new(AtomicI32::new(0));
    let mut presenter = {
        let gate = Arc::clone(&gate);
        scene.start(&rig, limits(), None, move |hooks| {
            hooks.frame_completion_gate = Some(gate);
        })
    };
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    let written = presenter.stats().instances_written;
    assert!(written >= 10, "{written}");
    // The second frame is asked for while the first one's completion is held.
    publish(&scene, 2, None, grid(25, |_| 5));
    until(&mut rig, "a busy attempt", || {
        presenter.stats().buffer_busy >= 1
    });
    // Long enough for an unheld completion to have arrived many times over; a held one cannot, however long this takes.
    rig.pump_for(Duration::from_millis(150));
    let during = presenter.stats();
    assert_eq!(scene.probes.frames(), 1, "the second frame has not drawn");
    assert_eq!(
        scene.probes.observer.completion_handlers_run(),
        0,
        "the first frame's completion is still held"
    );
    assert_eq!(
        during.instances_written, written,
        "nothing was written into the buffer while the previous command buffer was outstanding"
    );
    gate.store(1, Ordering::SeqCst);
    until(&mut rig, "the second frame", || {
        presenter.drawn_generation() == Some(2)
    });
    let after = presenter.stats();
    assert_eq!(after.last.instances, 25);
    assert!(after.instances_written >= written + 25);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// Budget and bounds: a `binding_geometry_bytes` too small for `max_quads` instances is refused naming both numbers (as
/// asked for, and as measured after alignment); a source that emits more than `max_quads` draws the first `max_quads` and
/// counts the rest as dropped; an invalid instance is skipped and counted, never a panic. Negative control: an unchecked
/// budget starts the presenter; an unbounded write overruns the buffer (the validation layer aborts).
pub(crate) fn scene_budget_and_bounds() {
    // Too small as asked for: 1000 instances of 48 bytes need 48000 bytes.
    {
        let mut rig = Rig::open();
        let mut scene = Scene::new();
        scene.bounds = SceneLimits {
            max_quads: 1000,
            binding_geometry_bytes: 47_999,
        };
        let mut presenter = scene.start(&rig, limits(), None, |_| {});
        let mut reports = Vec::new();
        assert!(reports_until(&mut rig, &presenter, &mut reports, 1));
        let PresenterReport::Failed { message } = &reports[0] else {
            panic!("the setup must fail: {reports:?}");
        };
        assert!(
            message.contains("48000") && message.contains("47999"),
            "{message}"
        );
        assert_eq!(scene.probes.frames(), 0);
        assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    }
    // Fits as asked for but not once the allocation is measured and aligned (a page is larger than 48000 bytes).
    {
        let mut rig = Rig::open();
        let mut scene = Scene::new();
        scene.bounds = SceneLimits {
            max_quads: 1000,
            binding_geometry_bytes: 48_000,
        };
        let mut presenter = scene.start(&rig, limits(), None, |_| {});
        let mut reports = Vec::new();
        assert!(reports_until(&mut rig, &presenter, &mut reports, 1));
        let PresenterReport::Failed { message } = &reports[0] else {
            panic!("the measured allocation must not fit: {reports:?}");
        };
        assert!(
            message.contains("measured") && message.contains("48000"),
            "{message}"
        );
        assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    }
    // The bounds: 16 quads of room, 40 valid instances and 3 invalid ones.
    {
        let mut rig = Rig::open();
        let mut scene = Scene::new();
        scene.bounds = SceneLimits {
            max_quads: 16,
            binding_geometry_bytes: 128 * 1024 * 1024,
        };
        let mut all = grid(40, |_| 1);
        for slot in [3_usize, 20, 41] {
            let mut bad = instance(1, rect(0, 0, 4, 4));
            bad.source = [0.0, 0.0, f32::INFINITY, 1.0];
            all.insert(slot.min(all.len()), bad);
        }
        publish(&scene, 1, None, all);
        let mut presenter = scene.start(&rig, limits(), None, |_| {});
        until(&mut rig, "the frame", || scene.probes.frames() >= 1);
        let stats = presenter.stats();
        assert_eq!(
            stats.last.instances, 16,
            "the first max_quads in painter order"
        );
        assert_eq!(stats.last.dropped, 24, "the rest are counted");
        assert_eq!(stats.last.skipped, 3, "the invalid ones took no slot");
        assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    }
}

/// A scene presenter with no scene draws exactly what `start_2d` would: the legacy quad over the clear, with no instance
/// and no batch; and with a scene the legacy quad stays below every scene quad. Negative control: drawing the scene
/// before the legacy quad fails the last pixel check.
pub(crate) fn scene_keeps_the_legacy_quad_below() {
    struct Pixels(Vec<u8>);

    impl PixelSource for Pixels {
        fn pixels(&self) -> &[u8] {
            &self.0
        }
    }

    let mut rig = Rig::open();
    let scene = Scene::new();
    let (width, height) = rig.pixels;
    let legacy = || {
        MetalQuad::new(
            Arc::new(Pixels(RED.repeat(4))),
            2,
            2,
            rect(
                0,
                0,
                i32::try_from(width).expect("small"),
                i32::try_from(height).expect("small"),
            ),
            SourceRect {
                left: 0.0,
                top: 0.0,
                right: 2.0,
                bottom: 2.0,
            },
            MetalFilter::Nearest,
        )
        .expect("a valid legacy quad")
    };
    scene.frames.publish(MetalFrame::new(
        1,
        MetalFilter::Nearest,
        Some(legacy()),
        None,
    ));
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    let frame = captured(&mut rig, &scene, 1);
    assert_eq!(px(&frame, 5, 5), RED, "the legacy quad alone");
    assert_eq!(px(&frame, width - 1, height - 1), RED);
    let stats = presenter.stats();
    assert_eq!(
        (stats.last.instances, stats.last.batches),
        (0, 0),
        "no scene, no draw"
    );
    // A scene quad over the same pixels is on top of it.
    scene.frames.publish(MetalFrame::new(
        2,
        MetalFilter::Nearest,
        Some(legacy()),
        Some(source(vec![instance(1, rect(0, 0, 64, 64))])),
    ));
    until(&mut rig, "the second frame", || {
        presenter.drawn_generation() == Some(2)
    });
    let frame = captured(&mut rig, &scene, 2);
    assert_eq!(
        px(&frame, 4, 4),
        MAGENTA,
        "the scene's checker is above the legacy quad"
    );
    assert_eq!(px(&frame, 12, 4), BLACK);
    assert_eq!(px(&frame, 100, 100), RED, "the legacy quad shows elsewhere");
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// The frame's filter reaches the draw: the same shown 2 x 2 texture drawn at four times its size is exact blocks under a
/// nearest frame and interpolated at the block edges under a linear one, while the checker stays sharp in both. Negative
/// control: not forwarding the filter (always nearest), or sampling the checker with the frame's filter, fails.
pub(crate) fn scene_filter_reaches_the_frame() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    // Black / white on the first row, white / black on the second.
    scene.driver().offers.push_back((1, 1, 2, 2));
    scene
        .driver()
        .pictures
        .push((1, 1, 2, 2, [BLACK, WHITE, WHITE, BLACK].concat()));
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    presenter.texture_wake()();
    until(&mut rig, "the texture adopted", || {
        scene.driver().adopted.iter().map(Vec::len).sum::<usize>() >= 1
    });
    // The texture at 8 x 8 and, beside it, the checker (key 9 is never shown) at 64 x 64.
    let list = vec![
        instance(1, rect(0, 0, 8, 8)),
        instance(9, rect(16, 0, 112, 96)),
    ];
    let send = |generation: u64, filter: MetalFilter| {
        scene.frames.publish(MetalFrame::new(
            generation,
            filter,
            None,
            Some(source(list.clone())),
        ));
    };
    send(1, MetalFilter::Nearest);
    until(&mut rig, "the nearest frame", || {
        presenter.drawn_generation() == Some(1)
    });
    let nearest = captured(
        &mut rig,
        &scene,
        scene.probes.frames().try_into().expect("small"),
    );
    send(2, MetalFilter::Linear);
    until(&mut rig, "the linear frame", || {
        presenter.drawn_generation() == Some(2)
    });
    let linear = captured(
        &mut rig,
        &scene,
        scene.probes.frames().try_into().expect("small"),
    );
    for y in 0..8 {
        for x in 0..8 {
            let expected = if (x / 4 + y / 4) % 2 == 0 {
                BLACK
            } else {
                WHITE
            };
            assert_eq!(px(&nearest, x, y), expected, "nearest, ({x}, {y})");
        }
    }
    let differing = (0..8)
        .flat_map(|y| (0..8).map(move |x| (x, y)))
        .filter(|&(x, y)| px(&nearest, x, y) != px(&linear, x, y))
        .count();
    assert!(
        differing >= 16,
        "the linear frame differs from the nearest one in {differing} pixels"
    );
    assert_eq!(
        px(&linear, 1, 1),
        BLACK,
        "inside a block the colour is the block's own"
    );
    // The checker, stretched to one and a half times its size, is sharp in both frames: every pixel is one of its two
    // colours, with no blend along a cell edge.
    for frame in [&nearest, &linear] {
        for y in 0..96 {
            for x in 16..112 {
                let seen = px(frame, x, y);
                assert!(
                    seen == MAGENTA || seen == BLACK,
                    "checker pixel ({x}, {y}): {seen:?}"
                );
            }
        }
        assert_eq!(px(frame, 16 + 2, 4), MAGENTA);
    }
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// An error completion of the checker that arrives while the presenter is stopping (the completion held until the
/// teardown has begun) is still the presenter's one `Failed` report, made before anything is released; the cleanup is
/// proof-correct (`Proven`: the token is done) and no frame was committed. Negative control: a teardown that reads only
/// whether the token is done releases the checker and loses the failure.
pub(crate) fn scene_checker_error_arriving_during_the_proof() {
    let mut rig = Rig::open();
    let mut scene = Scene::new();
    let gate = Arc::new(AtomicI32::new(0));
    scene.checker_hooks = Some(TextureTestHooks {
        completion_gate: Some(Arc::clone(&gate)),
        error_completion: true,
        ..TextureTestHooks::default()
    });
    let mut presenter = scene.start(&rig, limits(), None, |hooks| {
        hooks.stop_budget = Some(Duration::from_secs(20));
    });
    presenter.request_stop();
    until(&mut rig, "the teardown's last pass", || {
        scene.count(Boundary::Ending) >= 1
    });
    assert!(presenter.take_report().is_none(), "nothing failed yet");
    // The error completion arrives now, during the proof.
    gate.store(1, Ordering::SeqCst);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    let mut reports = Vec::new();
    while let Some(report) = presenter.take_report() {
        reports.push(report);
    }
    assert_eq!(reports.len(), 1, "exactly one report: {reports:?}");
    let PresenterReport::Failed { message } = &reports[0] else {
        panic!("the report must be a failure: {reports:?}");
    };
    assert!(message.contains("checker"), "{message}");
    assert_eq!(scene.probes.frames(), 0);
    assert_eq!(
        *lock(&scene.releases),
        ["store_and_ledger", "presenter", "context"]
    );
}

/// A render thread that unwinds while the checker's completion is still pending keeps the checker's graph (the token the
/// native handler will write, the staging buffer and the texture) and ends `Unproven`: the native live-object counts stay
/// up, and releasing the held completion afterwards writes retained memory. Negative control: freeing the checker on
/// unwind drops the counts to zero (and leaves the handler a freed token).
pub(crate) fn scene_unwind_retains_a_pending_checker() {
    /// A driver whose ordinary pass panics (after saying so).
    struct PanicOnPass(Arc<std::sync::atomic::AtomicBool>);

    impl TextureDriver<u32> for PanicOnPass {
        fn pass(
            &mut self,
            _ledger: &mut TextureLedger<u32>,
            boundary: Boundary,
            _events: Vec<LedgerEvent<u32>>,
        ) -> PassOutcome {
            if boundary == Boundary::Pass {
                self.0.store(true, Ordering::SeqCst);
                panic!("an ordinary pass panicked");
            }
            PassOutcome::default()
        }
    }

    let mut rig = Rig::open();
    rig.keep_alive = true;
    let context = MetalContext::new().expect("a device");
    let probes = Probes::new();
    let gate = Arc::new(AtomicI32::new(0));
    let panicked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut hooks = probes.hooks();
    hooks.scene = Some(SceneSetup::new(
        Arc::clone(&context),
        TextureLimits::default(),
        limits(),
        scene_limits(),
        PanicOnPass(Arc::clone(&panicked)),
        MetalFrameSlot::<u32>::new(),
        None,
        Some(TextureTestHooks {
            completion_gate: Some(Arc::clone(&gate)),
            ..TextureTestHooks::default()
        }),
    ));
    let mut presenter = probes.start_ok(&rig, hooks);
    // An ordinary texture pass runs while the checker's completion is held, and the driver panics in it.
    presenter.texture_wake()();
    until(&mut rig, "the panicking pass", || {
        panicked.load(Ordering::SeqCst)
    });
    let result = end(&mut rig, &mut presenter);
    // Unproven because the thread unwound, not because a stop budget ran out: the checker was still held throughout.
    let CleanupResult::Unproven { reason } = result else {
        panic!("a render thread that unwound cannot be proven: {result:?}");
    };
    assert!(reason.contains("panicked"), "{reason}");
    let live = context.native_live_objects();
    assert!(
        live.textures >= 1 && live.batches >= 1,
        "the checker's texture and its upload batch (staging and command buffer) were retained: {live:?}"
    );
    assert!(Arc::strong_count(&context) > 1, "and so was the context");
    // The held completion now arrives and writes the retained token; the graph stays alive to the process end.
    gate.store(1, Ordering::SeqCst);
    rig.pump_for(Duration::from_millis(300));
    let live = context.native_live_objects();
    assert!(live.textures >= 1 && live.batches >= 1, "{live:?}");
}

/// A committed frame of a scene presenter that has no scene source (a clear, or the legacy quad alone) updates the latest
/// counters to zero scene work and counts as a frame, while the peak is kept; an uncommitted attempt changes nothing.
/// Negative control: leaving the previous frame's counters in place reports ten instances for a frame that drew none.
pub(crate) fn scene_without_a_scene_updates_the_latest_counters() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    publish(&scene, 1, None, grid(10, |_| 5));
    let mut presenter = scene.start(&rig, limits(), None, |_| {});
    until(&mut rig, "the frame with a scene", || {
        presenter.drawn_generation() == Some(1)
    });
    let with = presenter.stats();
    assert_eq!((with.last.instances, with.peak.instances), (10, 10));
    // The scene is removed: the frame commits with nothing from the scene.
    scene
        .frames
        .publish(MetalFrame::new(2, MetalFilter::Nearest, None, None));
    until(&mut rig, "the frame without one", || {
        presenter.drawn_generation() == Some(2)
    });
    let without = presenter.stats();
    assert_eq!(without.frames, with.frames + 1, "the frame counts");
    assert_eq!(
        (
            without.last.instances,
            without.last.batches,
            without.last.checker
        ),
        (0, 0, 0),
        "the latest value describes the frame that was drawn: {without:?}"
    );
    assert_eq!(without.peak.instances, 10, "the peak is kept");
    // And back.
    publish(&scene, 3, None, grid(4, |_| 5));
    until(&mut rig, "the frame with a scene again", || {
        presenter.drawn_generation() == Some(3)
    });
    let again = presenter.stats();
    assert_eq!(
        (again.frames, again.last.instances, again.peak.instances),
        (without.frames + 1, 4, 10)
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// The cost of a scene at the target scale (tens of thousands of quads): the presenter's build-and-encode time per frame for
/// the same quads over one shown texture, over thousands of distinct shown textures, and over unresolved keys, printed as
/// evidence, and the counters checked. Nothing here is timed against a limit.
pub(crate) fn scene_scale_cost() {
    // The number of distinct shown textures (4,096 here; `CK_SCALE_KEYS` raises it for a measurement by hand).
    let distinct: u32 = std::env::var("CK_SCALE_KEYS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4096);
    let quads = distinct.clamp(20_000, 32_768);
    let mut rig = Rig::open();
    let mut scene = Scene::new();
    scene.bounds = SceneLimits {
        max_quads: 32_768,
        binding_geometry_bytes: 128 * 1024 * 1024,
    };
    {
        let mut driver = scene.driver();
        for key in 0..distinct {
            driver.offers.push_back((key, 1, 1, 1));
        }
    }
    let mut presenter = scene.start(&rig, limits(), None, |_| {});
    presenter.texture_wake()();
    assert!(
        rig.pump_until(Duration::from_secs(120), || {
            scene.driver().adopted.iter().map(Vec::len).sum::<usize>() >= distinct as usize
        }),
        "every texture adopted"
    );
    let mut generation = 0_u64;
    let mut measure = |what: &str, list: Vec<MetalInstance<u32>>| {
        generation += 1;
        publish(&scene, generation, None, list);
        until(&mut rig, "the frame", || {
            presenter.drawn_generation() == Some(generation)
        });
        let last = presenter.stats().last;
        eprintln!("scale: {what}: {last:?}");
        last
    };
    let one = measure(
        &format!("{quads} quads of one shown texture"),
        grid(quads, |_| 1),
    );
    assert_eq!(
        (one.instances, one.batches, one.pinned),
        (u64::from(quads), 1, 1)
    );
    let many = measure(
        &format!("{quads} quads over {distinct} distinct shown textures"),
        grid(quads, |i| i % distinct),
    );
    assert_eq!(
        (many.instances, many.pinned),
        (u64::from(quads), u64::from(distinct.min(quads)))
    );
    let unresolved = measure(
        &format!("{quads} quads of unresolved keys"),
        grid(quads, |i| 100_000 + i % 4096),
    );
    assert_eq!(
        (unresolved.instances, unresolved.checker, unresolved.pinned),
        (u64::from(quads), u64::from(quads), 0)
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

// ---------------------------------------------------------------------------------------------
// Drawing through the bindless table (CK-128)
// ---------------------------------------------------------------------------------------------

/// A scene whose presenter has the bindless path: a table of `slots` entries covering `textures` logical textures and `extra`
/// extra revisions (and the checker's slot 0).
fn bindless_scene(slots: u32, textures: u32, extra: u32) -> Scene {
    let mut scene = Scene::new();
    scene.bindless_limits = Some(ck_gpu_metal::BindlessLimits {
        texture_slots: slots,
        max_textures: textures,
        max_extra_revisions: extra,
    });
    scene
}

/// A bindless scene frame is one draw whatever its textures: seven instances of three images and an unresolved key alternate
/// in one run (one draw, the table bound once), each shows its own image (the pixels the presenter really drew, captured), the
/// unresolved one the checker, and **no revision is pinned** (the frame's counters and the ledger's live pins stay zero): the
/// table epoch alone holds them. Negative control: a path that bound a texture per batch would count seven draws and pins;
/// one that wrote another instance's slot would show another colour.
pub(crate) fn scene_bindless_draws_one_run_and_pins_no_revision() {
    let mut rig = Rig::open();
    let scene = bindless_scene(32, 8, 8);
    offer(&scene, 1, 1, RED);
    offer(&scene, 2, 1, GREEN);
    offer(&scene, 3, 1, BLUE);
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    presenter.texture_wake()();
    until(&mut rig, "all three adopted", || {
        scene.driver().adopted.iter().map(Vec::len).sum::<usize>() >= 3
    });
    let keys = [1, 2, 3, 9, 1, 2, 3];
    publish(
        &scene,
        10,
        None,
        keys.iter()
            .zip(0_i32..)
            .map(|(&key, cell)| instance(key, rect(cell * 8, 0, cell * 8 + 8, 8)))
            .collect(),
    );
    until(&mut rig, "the frame", || {
        presenter.drawn_generation() == Some(10)
    });
    let stats = presenter.stats();
    assert_eq!(
        (
            stats.last.instances,
            stats.last.batches,
            stats.last.texture_binds,
            stats.last.resolved,
            stats.last.checker,
            stats.last.pinned,
            stats.last.slot_skipped
        ),
        (7, 1, 1, 6, 1, 0, 0),
        "{stats:?}"
    );
    assert_eq!(stats.bindless_slots, 32);
    assert_eq!(
        scene.driver().max_live_pins,
        0,
        "no revision was pinned by any frame"
    );
    // The capture of the frame just drawn is the latest once every committed frame's capture has completed.
    let committed = i64::try_from(scene.probes.frames()).expect("small");
    let frame = captured(&mut rig, &scene, committed);
    for (cell, key) in (0_u32..).zip(keys) {
        let seen = px(&frame, cell * 8 + 4, 4);
        match key {
            1 => assert_eq!(seen, RED, "cell {cell}"),
            2 => assert_eq!(seen, GREEN, "cell {cell}"),
            3 => assert_eq!(seen, BLUE, "cell {cell}"),
            _ => assert!(
                seen == MAGENTA || seen == BLACK,
                "the checker at cell {cell}: {seen:?}"
            ),
        }
    }
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// **The epoch pin alone** (design review 1, Claude finding 4): while one identified frame is outstanding (its completion held),
/// **every texture it drew is replaced** (a new revision of each key is prepared, committed and adopted, so each old revision
/// retires) and none of the old native textures is released before that frame's token is done; the frame's own pixels (its capture)
/// are the OLD textures'; no per-revision pin was ever taken; once the completion arrives the old textures are released and the
/// next frame shows the replacements. Each step waits for the observed transition of the driver or the ledger (the replacements
/// prepared, adopted, the old revisions retired, then a counted number of further passes), never for elapsed time. Negative control
/// (the mutation probe): a frame that did not pin its table epoch lets the version be sanitised and the old textures released while
/// the frame is outstanding.
#[expect(
    clippy::too_many_lines,
    reason = "one ordered scenario: the held frame, the replacements, the observed transitions and the release read best together"
)]
pub(crate) fn scene_bindless_epoch_pin_alone_keeps_replaced_textures_until_the_frame_completes() {
    let mut rig = Rig::open();
    let scene = bindless_scene(32, 8, 8);
    offer(&scene, 1, 1, RED);
    offer(&scene, 2, 1, GREEN);
    // Frames complete normally until the test closes the gate (a closed gate holds the completion of the frame committed
    // next: the ordering under test is established by the state observed, not by elapsed time).
    let gate = Arc::new(AtomicI32::new(1));
    let mut presenter = {
        let gate = Arc::clone(&gate);
        scene.start(&rig, limits(), None, move |hooks| {
            hooks.frame_completion_gate = Some(gate);
            hooks.capture_frames = true;
        })
    };
    presenter.texture_wake()();
    until(&mut rig, "both adopted", || {
        scene.driver().adopted.iter().map(Vec::len).sum::<usize>() >= 2
    });
    let cells = vec![
        instance(1, rect(0, 0, 8, 8)),
        instance(2, rect(8, 0, 16, 8)),
    ];
    publish(&scene, 10, None, cells.clone());
    until(&mut rig, "the first scene frame", || {
        presenter.drawn_generation() == Some(10)
    });
    // Its completion arrived (the gate was open): nothing is in flight now.
    let drawn = scene.probes.frames();
    until(&mut rig, "its completion", || {
        scene.probes.observer.completion_handlers_run() >= i64::try_from(drawn).expect("small")
    });
    // The replacements are prepared BEFORE the frame is held (a held completion handler blocks the queue's later handlers, the
    // uploads' included), but their verdict is withheld (`defer_commit`), so nothing is adopted yet.
    let alive = scene.context.native_live_objects().textures;
    scene.driver().defer_commit.extend([1, 2]);
    offer(&scene, 1, 2, BLUE);
    offer(&scene, 2, 2, WHITE);
    until(&mut rig, "both replacements prepared", || {
        presenter.texture_wake()();
        let state = scene.driver();
        state.prepared.contains(&(1, 2)) && state.prepared.contains(&(2, 2))
    });
    assert_eq!(
        scene.context.native_live_objects().textures,
        alive + 2,
        "the old two and the two replacements exist"
    );
    // The identified frame: committed with its completion held. It is the `held`-th committed frame (0-based `held - 1`).
    gate.store(0, Ordering::SeqCst);
    publish(&scene, 11, None, cells.clone());
    until(&mut rig, "the held frame", || {
        presenter.drawn_generation() == Some(11)
    });
    let held = scene.probes.frames();
    assert_eq!(
        presenter.stats().last.pinned,
        0,
        "no revision pin: the epoch alone"
    );
    // Replace every texture it drew: the service's verdict arrives for both replacements (the late `commit_now`), and they are
    // adopted at a frame boundary, which retires the old revision of each key.
    scene.driver().commit_now.extend([(1, 2), (2, 2)]);
    presenter.texture_wake()();
    // A newer frame asks for a boundary while the held frame is still outstanding: the boundary pass adopts both (the frame
    // itself finds the buffer busy and waits for the completion).
    publish(&scene, 12, None, cells);
    until(&mut rig, "both replacements adopted", || {
        presenter.texture_wake()();
        let state = scene.driver();
        let adopted: Vec<(u32, u64)> = state.adopted.iter().flatten().copied().collect();
        adopted.contains(&(1, 2)) && adopted.contains(&(2, 2))
    });
    until(&mut rig, "both old revisions retired in the ledger", || {
        presenter.texture_wake()();
        scene
            .driver()
            .stats
            .as_ref()
            .is_some_and(|stats| stats.retired >= 2)
    });
    // Release was due on every pass since; three further passes by the driver's own count (not a duration) have looked at it.
    let passes = scene.passes();
    until(&mut rig, "three more passes", || {
        presenter.texture_wake()();
        scene.passes() >= passes + 3
    });
    assert_eq!(
        scene.probes.frames(),
        held,
        "the outstanding frame is still the last committed one"
    );
    assert_eq!(
        scene.context.native_live_objects().textures,
        alive + 2,
        "the two replacements exist beside the old two: no old texture the in-flight frame can sample was released"
    );
    assert_eq!(
        scene.driver().max_live_pins,
        0,
        "no revision was ever pinned"
    );
    // The completion arrives: the old textures are released, and the frame's own pixels are the old textures'.
    gate.store(1, Ordering::SeqCst);
    let frame = captured_at(
        &mut rig,
        &scene,
        u64::from(u32::try_from(held).expect("small")) - 1,
    );
    assert_eq!(
        px(&frame, 4, 4),
        RED,
        "the held frame shows the old first texture"
    );
    assert_eq!(
        px(&frame, 12, 4),
        GREEN,
        "the held frame shows the old second texture"
    );
    until(
        &mut rig,
        "the old textures released after the completion",
        || {
            presenter.texture_wake()();
            scene.context.native_live_objects().textures == alive
        },
    );
    // The next frame (the newer generation, drawn once the buffer is free) shows the replacements.
    until(&mut rig, "the replacements drawn", || {
        presenter.drawn_generation() == Some(12)
    });
    let next = captured_at(
        &mut rig,
        &scene,
        u64::from(u32::try_from(held).expect("small")),
    );
    assert_eq!(px(&next, 4, 4), BLUE);
    assert_eq!(px(&next, 12, 4), WHITE);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A refused capability gate is the presenter's one `Failed` report with the gate's diagnostic, **no fallback drawing** and a
/// proven end: with the device forced to report Tier 1 argument buffers, the report names the missing feature and both values
/// ("argumentBuffersSupport requested Tier 2, available Tier 1"), and no frame is committed even though a scene is published.
/// Negative control: falling back to the ordinary path would commit a frame.
pub(crate) fn scene_bindless_gate_refusal_is_one_failed_report_and_no_fallback() {
    let mut rig = Rig::open();
    let mut scene = bindless_scene(32, 8, 8);
    scene.checker_hooks = Some(TextureTestHooks {
        force_bindless_tier1: true,
        ..TextureTestHooks::default()
    });
    publish(&scene, 1, None, vec![instance(5, rect(0, 0, 64, 64))]);
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    let mut reports = Vec::new();
    assert!(
        reports_until(&mut rig, &presenter, &mut reports, 1),
        "no report"
    );
    rig.pump_for(Duration::from_millis(300));
    while let Some(report) = presenter.take_report() {
        reports.push(report);
    }
    let failures: Vec<&PresenterReport> = reports
        .iter()
        .filter(|report| matches!(report, PresenterReport::Failed { .. }))
        .collect();
    assert_eq!(failures.len(), 1, "exactly one Failed report: {reports:?}");
    let PresenterReport::Failed {
        message: reason, ..
    } = failures[0]
    else {
        unreachable!("filtered");
    };
    assert!(
        reason.contains("bindless capability gate")
            && reason.contains("argumentBuffersSupport requested Tier 2")
            && reason.contains("available Tier 1"),
        "the gate's diagnostic names the feature and both values: {reason}"
    );
    assert!(
        !reports.contains(&PresenterReport::FirstFrameQueued),
        "{reports:?}"
    );
    assert_eq!(scene.probes.frames(), 0, "no fallback drawing");
    assert_eq!(scene.probes.observer.captures(), 0);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// The setting's propagation, off screen of the application: a presenter built with `texture_slots` 17 (with `max_textures` 8
/// and `max_extra_revisions` 8, the rule's exact minimum) allocates tables of exactly that many entries (the snapshot's
/// `bindless_slots`, and the two tables' measured bytes cover 17 entries of 8 bytes each, no more than their page
/// alignment adds); a presenter with 4,096 slots shows 4,096 and strictly more bytes. No case counts a logical or byte-budget
/// refusal as native exhaustion. Negative control: a hard-coded default would show the same count for both.
pub(crate) fn scene_bindless_slot_count_is_the_configured_one() {
    let mut measured = Vec::new();
    for (slots, textures, extra) in [(17_u32, 8_u32, 8_u32), (4096, 8, 8)] {
        let mut rig = Rig::open();
        let scene = bindless_scene(slots, textures, extra);
        let mut presenter = scene.start(&rig, limits(), None, capturing);
        until(&mut rig, "the checker resident", || {
            presenter.stats().checker_resident
        });
        let stats = presenter.stats();
        assert_eq!(stats.bindless_slots, slots, "{stats:?}");
        let entries = u64::from(slots);
        assert!(
            stats.bindless_table_bytes >= entries * 16,
            "both tables hold {slots} entries of 8 bytes: {} bytes",
            stats.bindless_table_bytes
        );
        assert!(
            stats.bindless_table_bytes <= entries * 16 + 2 * 16384,
            "no more than the page alignment adds: {} bytes",
            stats.bindless_table_bytes
        );
        measured.push(stats.bindless_table_bytes);
        assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    }
    assert!(measured[1] > measured[0], "{measured:?}");
}

/// The scene cases of drawing through the bindless table (CK-128).
pub(crate) const BINDLESS: [(&str, fn()); 4] = [
    (
        "scene_bindless_draws_one_run_and_pins_no_revision",
        scene_bindless_draws_one_run_and_pins_no_revision,
    ),
    (
        "scene_bindless_epoch_pin_alone_keeps_replaced_textures_until_the_frame_completes",
        scene_bindless_epoch_pin_alone_keeps_replaced_textures_until_the_frame_completes,
    ),
    (
        "scene_bindless_gate_refusal_is_one_failed_report_and_no_fallback",
        scene_bindless_gate_refusal_is_one_failed_report_and_no_fallback,
    ),
    (
        "scene_bindless_slot_count_is_the_configured_one",
        scene_bindless_slot_count_is_the_configured_one,
    ),
];

pub(crate) const CASES: [(&str, fn()); 14] = [
    (
        "scene_bindless_checker_residency_lifecycle",
        scene_bindless_checker_residency_lifecycle,
    ),
    (
        "scene_checker_gates_the_first_frame",
        scene_checker_gates_the_first_frame,
    ),
    ("scene_checker_upload_failure", scene_checker_upload_failure),
    (
        "scene_checker_stop_while_uploading",
        scene_checker_stop_while_uploading,
    ),
    (
        "scene_checker_is_a_baseline_charge",
        scene_checker_is_a_baseline_charge,
    ),
    (
        "scene_draws_adopted_revisions",
        scene_draws_adopted_revisions,
    ),
    ("scene_batches_and_pins_once", scene_batches_and_pins_once),
    (
        "scene_movement_uploads_nothing",
        scene_movement_uploads_nothing,
    ),
    (
        "scene_places_with_its_own_metrics",
        scene_places_with_its_own_metrics,
    ),
    (
        "scene_instance_buffer_is_written_only_when_free",
        scene_instance_buffer_is_written_only_when_free,
    ),
    ("scene_budget_and_bounds", scene_budget_and_bounds),
    (
        "scene_checker_error_arriving_during_the_proof",
        scene_checker_error_arriving_during_the_proof,
    ),
    (
        "scene_unwind_retains_a_pending_checker",
        scene_unwind_retains_a_pending_checker,
    ),
    (
        "scene_without_a_scene_updates_the_latest_counters",
        scene_without_a_scene_updates_the_latest_counters,
    ),
];

pub(crate) const MORE: [(&str, fn()); 3] = [
    ("scene_scale_cost", scene_scale_cost),
    (
        "scene_keeps_the_legacy_quad_below",
        scene_keeps_the_legacy_quad_below,
    ),
    (
        "scene_filter_reaches_the_frame",
        scene_filter_reaches_the_frame,
    ),
];
