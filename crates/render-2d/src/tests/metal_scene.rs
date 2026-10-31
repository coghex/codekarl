//! The Metal backend's scene side without a window or a device: what a published snapshot becomes (in O(1)), where the
//! scene's quads are placed (on the render thread, with the snapshot's own metrics), and that no scene frame is ever freed
//! under the bridge's lock.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ck_gpu_metal::{MetalFrameSlot, MetalInstance};
use ck_resources::{
    CpuPixels, Mode, ResourceLimits, ResourceService, ResourceState, TextureKey, TextureRef,
};

use super::support::{RecordingSink, desc, image, lock, lock_held_by, pixels, quad_at};
use crate::metal::{Converted, ImagePixels, Refusals, SceneSink, SinkSide};
use crate::metal_scene::SceneFrameSource;
use crate::{Filter, Gpu2d, Metrics, QuadSink, SceneBridge, Snapshot};

fn service() -> ResourceService {
    ResourceService::new(ResourceLimits::DEFAULT, Mode::Backend).expect("a backend-mode service")
}

fn texture(service: &ResourceService) -> (TextureKey, TextureRef<ResourceState>) {
    let reservation = service.reserve_pixels(1, 1).expect("one pixel");
    let pixels = CpuPixels::premultiplied(reservation, vec![0; 4]).expect("RGBA8 length");
    let key = service
        .create_from_pixels(pixels)
        .unwrap_or_else(|_| panic!("a texture admitted"));
    let retained = service.retain(key).expect("the public texture is live");
    (key, retained)
}

fn bits(values: [f32; 4]) -> [u32; 4] {
    values.map(f32::to_bits)
}

/// What `place` emitted, as plain values: key, destination, source bits.
type Emitted = Vec<(TextureKey, [i32; 4], [u32; 4])>;

fn emitted(source: &SceneFrameSource, size: (u32, u32)) -> Emitted {
    use ck_gpu_metal::MetalSceneSource;
    let mut out = Vec::new();
    source.place(size.0, size.1, &mut |instance: MetalInstance<
        TextureKey,
    >| {
        let d = instance.destination;
        out.push((
            instance.key,
            [d.left, d.top, d.right, d.bottom],
            bits(instance.source),
        ));
    });
    out
}

/// What `FrameQuad::place` gives for the same frame and metrics, with each quad's key.
fn expected(snapshot: &Snapshot, metrics: &Metrics) -> Emitted {
    let frame = snapshot.scene.as_ref().expect("a published frame");
    frame
        .quads()
        .iter()
        .filter_map(|quad| {
            quad.place(metrics).map(|placed| {
                let d = placed.destination;
                let s = placed.source;
                (
                    placed.texture.key(),
                    [d.left, d.top, d.right, d.bottom],
                    bits([s.left, s.top, s.right, s.bottom]),
                )
            })
        })
        .collect()
}

/// A bridge with a recording sink attached (window 200 x 160 points, a 400 x 320 drawable) and its writer.
fn scene_bridge() -> (
    Arc<SceneBridge>,
    crate::SceneWriter,
    Arc<RecordingSink>,
    ResourceService,
) {
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().expect("the one writer");
    let sink = RecordingSink::new();
    bridge
        .attach(
            &desc(200, 160),
            pixels(400, 320),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .expect("a first attach");
    (bridge, writer, sink, service())
}

#[test]
fn placement_gives_what_frame_quad_place_gives_in_painter_order_and_omits_the_unplaced() {
    let (_bridge, mut writer, sink, service) = scene_bridge();
    let keys: Vec<(TextureKey, TextureRef<ResourceState>)> =
        (0..4).map(|_| texture(&service)).collect();
    // On screen, partly off screen (clipped, its source trimmed), wholly off screen (placed to nothing) and on screen.
    let rects = [
        (10.0, 10.0, 40.0, 30.0),
        (-20.0, 100.0, 60.0, 80.0),
        (500.0, 500.0, 30.0, 30.0),
        (150.0, 20.0, 30.0, 30.0),
    ];
    for ((_, reference), (x, y, w, h)) in keys.iter().zip(rects) {
        writer
            .create_quad(reference.clone(), x, y, w, h)
            .expect("a quad");
    }
    assert!(writer.publish());
    let snapshot = sink.last().expect("a snapshot");
    let source = SceneFrameSource::new(
        Arc::clone(snapshot.scene.as_ref().expect("a frame")),
        snapshot.metrics,
    );
    let got = emitted(&source, (400, 320));
    let want = expected(&snapshot, &snapshot.metrics);
    assert_eq!(got, want, "exactly CK-110's placement, in painter order");
    assert_eq!(got.len(), 3, "the quad placed to nothing is omitted");
    let order: Vec<TextureKey> = got.iter().map(|g| g.0).collect();
    assert_eq!(
        order,
        [keys[0].0, keys[1].0, keys[3].0],
        "painter order, each with its key"
    );
}

#[test]
fn placement_takes_no_texture_reference() {
    use ck_gpu_metal::MetalSceneSource;
    let (_bridge, mut writer, sink, service) = scene_bridge();
    let (_, reference) = texture(&service);
    for column in 0..8_u32 {
        writer
            .create_quad(reference.clone(), f64::from(column) * 10.0, 10.0, 8.0, 8.0)
            .expect("a quad");
    }
    assert!(writer.publish());
    let snapshot = sink.last().expect("a snapshot");
    let frame = Arc::clone(snapshot.scene.as_ref().expect("a frame"));
    let source = SceneFrameSource::new(Arc::clone(&frame), snapshot.metrics);
    let before = reference.reference_count();

    let mut inside = Vec::new();
    source.place(400, 320, &mut |_: MetalInstance<TextureKey>| {
        inside.push(reference.reference_count());
    });
    assert_eq!(inside.len(), 8, "eight instances were emitted");
    assert!(
        inside.iter().all(|count| *count == before),
        "no instance holds a reference of its own: {inside:?} against {before}"
    );
    assert_eq!(reference.reference_count(), before, "and none is left over");

    // Negative control: placing through `FrameQuad::place`, as `SceneFrameSource::place` did, holds one more while the placed
    // quad lives, which is exactly what the check above would fail on.
    let held = frame
        .quads()
        .iter()
        .map(|quad| {
            let placed = quad.place(&snapshot.metrics).expect("placed");
            let count = reference.reference_count();
            drop(placed);
            count
        })
        .collect::<Vec<_>>();
    assert!(held.iter().all(|count| *count == before + 1), "{held:?}");
    assert_eq!(reference.reference_count(), before);
}

#[test]
fn the_size_the_presenter_passes_does_not_change_the_destinations() {
    // Negative control: placing with the size argument (a metrics made from it) gives other destinations.
    let (_bridge, mut writer, sink, service) = scene_bridge();
    let (_, reference) = texture(&service);
    writer
        .create_quad(reference, 10.0, 10.0, 40.0, 30.0)
        .expect("a quad");
    assert!(writer.publish());
    let snapshot = sink.last().expect("a snapshot");
    let source = SceneFrameSource::new(
        Arc::clone(snapshot.scene.as_ref().expect("a frame")),
        snapshot.metrics,
    );
    let own = emitted(&source, (400, 320));
    assert_eq!(emitted(&source, (1600, 1200)), own);
    assert_eq!(emitted(&source, (1, 1)), own);
    let resized = Metrics {
        pixels: pixels(1600, 1200),
        ..snapshot.metrics
    };
    assert_ne!(
        expected(&snapshot, &resized),
        own,
        "a placement that followed the argument would move the quad"
    );
}

#[test]
fn the_scene_and_the_legacy_quad_of_one_snapshot_agree_through_a_resize() {
    let (bridge, mut writer, sink, service) = scene_bridge();
    let (_, reference) = texture(&service);
    let picture = image(16, 16);
    // The same point rectangle for the legacy quad and the frame quad.
    let rect = (20.0, 30.0, 60.0, 40.0);
    writer.set_quad(quad_at(&picture, rect.0, rect.1, rect.2, rect.3));
    writer
        .create_quad(reference, rect.0, rect.1, rect.2, rect.3)
        .expect("a quad");
    assert!(writer.publish());
    let destination = |snapshot: &Snapshot| {
        let d = snapshot
            .quad
            .as_ref()
            .expect("the legacy quad is placed")
            .destination;
        [d.left, d.top, d.right, d.bottom]
    };
    let before = sink.last().expect("a snapshot");
    let source = SceneFrameSource::new(
        Arc::clone(before.scene.as_ref().expect("a frame")),
        before.metrics,
    );
    // The drawable doubled since this snapshot was made: `place` is still given the new size and ignores it.
    let placed = emitted(&source, (800, 640));
    assert_eq!(placed.len(), 1);
    assert_eq!(
        placed[0].1,
        destination(&before),
        "equal to the legacy quad's pixel destination"
    );
    // Negative control: placing with the size argument makes the two disagree.
    let by_argument = expected(
        &before,
        &Metrics {
            pixels: pixels(800, 640),
            ..before.metrics
        },
    );
    assert_ne!(by_argument[0].1, destination(&before));
    // The resize's own snapshot gives both the new, equal destinations.
    bridge.set_pixel_size(pixels(800, 640));
    let after = sink.last().expect("the resize's snapshot");
    let source = SceneFrameSource::new(
        Arc::clone(after.scene.as_ref().expect("a frame")),
        after.metrics,
    );
    let placed = emitted(&source, (800, 640));
    assert_eq!(placed[0].1, destination(&after));
    assert_ne!(destination(&after), destination(&before));
}

/// A sink over a fresh frame slot with the real conversion.
fn scene_sink(counter: &Arc<AtomicUsize>) -> Arc<SceneSink> {
    sink_with(Some(Arc::clone(counter)))
}

/// A sink as the backend builds it: no placement counter.
fn plain_scene_sink() -> Arc<SceneSink> {
    sink_with(None)
}

fn sink_with(placements: Option<Arc<AtomicUsize>>) -> Arc<SceneSink> {
    SceneSink::over(
        MetalFrameSlot::new(),
        Arc::new(Refusals::default()),
        Arc::new(|| {}),
        crate::metal::real_converter(),
        None,
        placements,
    )
}

#[test]
fn publishing_a_ten_thousand_quad_frame_places_nothing_and_shares_the_frame() {
    let (_bridge, mut writer, sink, service) = scene_bridge();
    let (_, reference) = texture(&service);
    for index in 0..10_000_u32 {
        let (x, y) = (f64::from(index % 100), f64::from(index / 100));
        writer
            .create_quad(reference.clone(), x, y, 1.0, 1.0)
            .expect("a quad");
    }
    assert!(writer.publish());
    let snapshot = sink.last().expect("a snapshot");
    let shared = snapshot.scene.clone().expect("a frame");
    let before = Arc::strong_count(&shared);
    let counter = Arc::new(AtomicUsize::new(0));
    let scene_sink = scene_sink(&counter);
    let returned = scene_sink.publish(snapshot.clone());
    assert_eq!(
        returned.map(|s| s.generation),
        None,
        "the first publication displaces nothing"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "no placement under the lock"
    );
    // The frame is shared, not copied: the sink's source and its kept snapshot are two more owners of the same allocation.
    assert_eq!(
        Arc::strong_count(&shared),
        before + 3 - 1,
        "snapshot clone (dropped), source, kept snapshot"
    );
    // Placement runs where the presenter calls it, once, for every quad.
    let frame = scene_sink.frame_for(
        &snapshot,
        &ck_gpu_metal::MetalScene::new(snapshot.generation, None),
    );
    let mut count = 0_usize;
    frame
        .scene()
        .expect("a scene source")
        .place(400, 320, &mut |_| count += 1);
    assert_eq!(count, 10_000);
    assert_eq!(counter.load(Ordering::SeqCst), 1, "placed once, on demand");
}

/// What one snapshot becomes when a sink makes its frame, and how that frame places.
fn check_frame(
    scene_sink: &SceneSink,
    snapshot: &Snapshot,
    filter: ck_gpu_metal::MetalFilter,
    key: TextureKey,
) {
    let mut conversion = crate::metal::Conversion::default();
    let step = conversion.next(snapshot);
    let frame = scene_sink.frame_for(snapshot, &step.scene);
    assert_eq!(frame.generation(), snapshot.generation);
    assert_eq!(frame.filter(), filter);
    let quad = frame.quad().expect("the legacy quad is converted as today");
    let d = quad.destination();
    let legacy = snapshot
        .quad
        .as_ref()
        .expect("a placed legacy quad")
        .destination;
    assert_eq!(
        [d.left, d.top, d.right, d.bottom],
        [legacy.left, legacy.top, legacy.right, legacy.bottom]
    );
    // The scene is placed by the snapshot's own metrics, whatever size is asked.
    let mut placed = Vec::new();
    frame
        .scene()
        .expect("a scene source")
        .place(7, 7, &mut |instance: MetalInstance<TextureKey>| {
            placed.push((
                instance.key,
                [
                    instance.destination.left,
                    instance.destination.top,
                    instance.destination.right,
                    instance.destination.bottom,
                ],
                bits(instance.source),
            ));
        });
    assert_eq!(placed, expected(snapshot, &snapshot.metrics));
    assert_eq!(placed[0].0, key);
    assert_eq!(
        placed[0].1,
        [legacy.left, legacy.top, legacy.right, legacy.bottom]
    );
}

#[test]
fn a_snapshot_becomes_a_frame_with_its_generation_filter_legacy_quad_and_the_scene_placed_by_its_own_metrics()
 {
    let (bridge, mut writer, sink, service) = scene_bridge();
    let (key, reference) = texture(&service);
    let picture = image(16, 16);
    writer.set_quad(quad_at(&picture, 20.0, 30.0, 60.0, 40.0));
    writer
        .create_quad(reference, 20.0, 30.0, 60.0, 40.0)
        .expect("a quad");
    assert!(writer.publish());
    let counter = Arc::new(AtomicUsize::new(0));
    // Both ways a sink is built (the counting one only exists in tests): the frame is the same.
    let counting = scene_sink(&counter);
    let plain = plain_scene_sink();
    let check = |snapshot: &Snapshot, filter: ck_gpu_metal::MetalFilter| {
        for scene_sink in [&counting, &plain] {
            check_frame(scene_sink, snapshot, filter, key);
        }
    };
    check(
        &sink.last().expect("a snapshot"),
        ck_gpu_metal::MetalFilter::Nearest,
    );
    // After a resize the new snapshot's metrics decide, for the legacy quad and the scene alike.
    bridge.set_pixel_size(pixels(800, 640));
    check(
        &sink.last().expect("the resize's snapshot"),
        ck_gpu_metal::MetalFilter::Nearest,
    );
    // The filter follows the snapshot.
    let mut linear = sink.last().expect("a snapshot");
    linear.filter = Filter::Linear;
    check(&linear, ck_gpu_metal::MetalFilter::Linear);
    // No scene: no source.
    let mut bare = sink.last().expect("a snapshot");
    bare.scene = None;
    let frame = plain.frame_for(&bare, &ck_gpu_metal::MetalScene::new(bare.generation, None));
    assert!(frame.scene().is_none() && frame.quad().is_none());
}

/// A frame drop observer: records whether the bridge lock was held at each drop.
fn observe_frame_drops(
    writer: &mut crate::SceneWriter,
    bridge: &Arc<SceneBridge>,
) -> Arc<Mutex<Vec<bool>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (held, record) = (lock_held_by(bridge), Arc::clone(&seen));
    writer.set_frame_drop_hook(move || lock(&record).push(held()));
    seen
}

fn real_scene_backend_sink() -> (SinkSide, Arc<dyn QuadSink>) {
    let mut side = SinkSide::for_scenes_for_test(Arc::new(|| {}));
    let sink = side.new_sink();
    (side, sink)
}

#[test]
fn a_scene_frame_is_never_dropped_while_the_bridge_lock_is_held() {
    let bridge = SceneBridge::new(Filter::Nearest);
    let mut writer = bridge.take_writer().expect("the one writer");
    let drops = observe_frame_drops(&mut writer, &bridge);
    let (_side, sink) = real_scene_backend_sink();
    bridge
        .attach(&desc(200, 160), pixels(400, 320), sink)
        .expect("a first attach");
    let service = service();
    let (_, reference) = texture(&service);
    let picture = image(8, 8);
    // Publications, a legacy write that republishes the same frame, and a destroyed quad.
    let first = writer
        .create_quad(reference.clone(), 1.0, 1.0, 10.0, 10.0)
        .expect("a quad");
    assert!(writer.publish());
    writer
        .create_quad(reference.clone(), 20.0, 1.0, 10.0, 10.0)
        .expect("a quad");
    assert!(writer.publish());
    writer.set_quad(quad_at(&picture, 5.0, 5.0, 20.0, 20.0));
    writer.destroy_quad(first).expect("destroyed");
    assert!(writer.publish());
    bridge.set_pixel_size(pixels(800, 640));
    // Detach: the sink goes after the lock does.
    bridge.detach();
    drop(writer);
    let seen = lock(&drops).clone();
    assert!(!seen.is_empty(), "frames were dropped and observed");
    assert!(
        seen.iter().all(|held| !held),
        "a frame was dropped under the lock: {seen:?}"
    );
}

/// A conversion that refuses every legacy quad, as the GPU crate would an empty source span.
fn refuse(snapshot: &Snapshot, _reuse: Option<Arc<ImagePixels>>) -> Result<Converted, String> {
    if snapshot.quad.is_some() {
        Err("the Metal backend refused a quad: forced by the test".to_owned())
    } else {
        Ok((
            ck_gpu_metal::MetalScene::new(snapshot.generation, None),
            None,
        ))
    }
}

#[test]
fn a_refused_legacy_conversion_publishes_no_quad_and_still_frees_no_frame_under_the_lock() {
    let bridge = SceneBridge::new(Filter::Nearest);
    let mut writer = bridge.take_writer().expect("the one writer");
    let drops = observe_frame_drops(&mut writer, &bridge);
    let mut side = SinkSide::for_scenes_with_converter(Arc::new(|| {}), refuse);
    let sink = side.new_sink();
    bridge
        .attach(&desc(200, 160), pixels(400, 320), sink)
        .expect("a first attach");
    let service = service();
    let (_, reference) = texture(&service);
    let picture = image(8, 8);
    writer
        .create_quad(reference.clone(), 1.0, 1.0, 10.0, 10.0)
        .expect("a quad");
    assert!(writer.publish());
    writer.set_quad(quad_at(&picture, 5.0, 5.0, 20.0, 20.0));
    writer
        .create_quad(reference, 30.0, 1.0, 10.0, 10.0)
        .expect("a quad");
    assert!(writer.publish());
    assert!(
        side.report().is_some(),
        "the refusal is recorded and reported once"
    );
    bridge.detach();
    drop(writer);
    let seen = lock(&drops).clone();
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|held| !held), "{seen:?}");
}

/// The wrong pattern, for the control: it drops the snapshot it replaces inside `publish`.
struct EagerScene(Arc<SceneSink>);

impl QuadSink for EagerScene {
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        drop(self.0.publish(snapshot));
        None
    }
}

#[test]
fn the_probe_sees_a_scene_frame_dropped_under_the_lock_when_the_sink_drops_its_previous_snapshot() {
    // Negative control. Two snapshots with distinct frames are made, then held only by the test; published while the lock
    // is held, the real sink hands the first back (dropped outside the lock afterwards) and the eager one frees it inside.
    for eager in [false, true] {
        let bridge = SceneBridge::new(Filter::Nearest);
        let mut writer = bridge.take_writer().expect("the one writer");
        let recording = RecordingSink::new();
        bridge
            .attach(
                &desc(200, 160),
                pixels(400, 320),
                Arc::clone(&recording) as Arc<dyn QuadSink>,
            )
            .expect("a first attach");
        let service = service();
        let (_, reference) = texture(&service);
        let drops = observe_frame_drops(&mut writer, &bridge);
        writer
            .create_quad(reference.clone(), 1.0, 1.0, 10.0, 10.0)
            .expect("a quad");
        assert!(writer.publish());
        writer
            .create_quad(reference, 20.0, 1.0, 10.0, 10.0)
            .expect("a quad");
        assert!(writer.publish());
        let snapshots: Vec<Snapshot> = recording
            .snapshots()
            .into_iter()
            .filter(|s| s.scene.is_some())
            .collect();
        assert_eq!(snapshots.len(), 2);
        drop(recording);
        bridge.detach();
        let counter = Arc::new(AtomicUsize::new(0));
        let real = scene_sink(&counter);
        let sink: Arc<dyn QuadSink> = if eager {
            Arc::new(EagerScene(Arc::clone(&real)))
        } else {
            Arc::clone(&real) as Arc<dyn QuadSink>
        };
        let mut snapshots = snapshots.into_iter();
        let (first, second) = (
            snapshots.next().expect("first"),
            snapshots.next().expect("second"),
        );
        // The test's own references to the first frame go; the sink's are the last, so only the sink can free it.
        lock(&drops).clear();
        let handed_back = bridge.with_lock_held(|| {
            sink.publish(first);
            sink.publish(second)
        });
        let under_lock = lock(&drops).clone();
        drop(handed_back);
        let all = lock(&drops).clone();
        if eager {
            assert!(
                under_lock.contains(&true),
                "the eager sink freed a frame under the lock: {under_lock:?}"
            );
        } else {
            assert!(
                under_lock.is_empty(),
                "the real sink freed nothing under the lock: {under_lock:?}"
            );
            assert!(
                all.iter().all(|held| !held),
                "and what it handed back went outside it: {all:?}"
            );
        }
    }
}

#[test]
fn the_backend_is_built_without_a_window_and_new_still_draws_only_the_legacy_quad() {
    // `MetalBackend::new` keeps its quad-slot sink: the sink it makes is the one it accepts.
    #[expect(
        unsafe_code,
        reason = "never started: the contract is about the layer `start` is given"
    )]
    // SAFETY: this backend is never started, so it never touches a layer.
    let mut backend = unsafe { crate::MetalBackend::new(Arc::new(|| {})) };
    let sink = backend.new_sink();
    assert!(backend.made(&sink));
    assert!(backend.accept(&sink).is_ok());
    let other = scene_sink(&Arc::new(AtomicUsize::new(0))) as Arc<dyn QuadSink>;
    assert!(!backend.made(&other));
}

/// What `place` emitted with each instance's role: key, role.
fn roles(source: &SceneFrameSource) -> Vec<(TextureKey, ck_gpu_metal::InstanceRole)> {
    use ck_gpu_metal::MetalSceneSource;
    let mut out = Vec::new();
    source.place(400, 320, &mut |instance: MetalInstance<TextureKey>| {
        out.push((instance.key, instance.role));
    });
    out
}

/// The compatibility key a sink's frame carries for `snapshot`.
fn compat_key_of(sink: &SceneSink, snapshot: &Snapshot) -> Option<TextureKey> {
    let mut conversion = crate::metal::Conversion::default();
    let step = conversion.next(snapshot);
    sink.frame_for(snapshot, &step.scene).compat_key().copied()
}

#[test]
fn the_compat_quad_is_emitted_first_with_its_role_and_its_key_rides_the_frame_even_when_it_places_to_nothing()
 {
    use ck_gpu_metal::InstanceRole::{Compat, Scene};
    let (_bridge, mut writer, sink, service) = scene_bridge();
    let (compat_key, compat_ref) = texture(&service);
    let (first, first_ref) = texture(&service);
    let (second, second_ref) = texture(&service);
    // The scene quads are written first and the compatibility quad last: the order of emission does not follow the order
    // of the writes.
    writer
        .create_quad(first_ref, 10.0, 10.0, 20.0, 20.0)
        .expect("a quad");
    writer
        .create_quad(second_ref, 60.0, 10.0, 20.0, 20.0)
        .expect("a quad");
    writer
        .set_compat(compat_ref.clone(), 5.0, 5.0, 100.0, 100.0)
        .expect("a compatibility quad");
    assert!(writer.publish());
    let snapshot = sink.last().expect("a snapshot");
    let source = SceneFrameSource::new(
        Arc::clone(snapshot.scene.as_ref().expect("a frame")),
        snapshot.metrics,
    );
    assert_eq!(
        roles(&source),
        [(compat_key, Compat), (first, Scene), (second, Scene)],
        "the compatibility quad first, below every scene quad, with its role"
    );
    let plain = plain_scene_sink();
    assert_eq!(compat_key_of(&plain, &snapshot), Some(compat_key));

    // Placed to nothing (wholly off screen): not emitted, yet the key still reaches the presenter.
    writer
        .set_compat(compat_ref, 900.0, 900.0, 20.0, 20.0)
        .expect("a compatibility quad");
    assert!(writer.publish());
    let snapshot = sink.last().expect("a snapshot");
    let source = SceneFrameSource::new(
        Arc::clone(snapshot.scene.as_ref().expect("a frame")),
        snapshot.metrics,
    );
    assert_eq!(roles(&source), [(first, Scene), (second, Scene)]);
    assert_eq!(
        compat_key_of(&plain, &snapshot),
        Some(compat_key),
        "a clipped compatibility quad is still the frame's compatibility key"
    );

    // Cleared: no compatibility key.
    writer.clear_compat();
    assert!(writer.publish());
    let snapshot = sink.last().expect("a snapshot");
    assert_eq!(compat_key_of(&plain, &snapshot), None);
    let mut bare = snapshot;
    bare.scene = None;
    assert_eq!(
        compat_key_of(&plain, &bare),
        None,
        "no scene, no compatibility key"
    );
}

#[test]
fn the_pre_exchange_hook_runs_once_per_publication_under_the_bridges_lock() {
    // Its order against the exchange is proven by the real-window scenario (the renderer draws the previous frame while the
    // hook holds the producer). Negative control: a sink that never calls it leaves the count at zero.
    let service = service();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let hooked = SceneSink::over(
        MetalFrameSlot::new(),
        Arc::new(Refusals::default()),
        Arc::new(|| {}),
        crate::metal::real_converter(),
        Some(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })),
        None,
    );
    let bridge = SceneBridge::new(Filter::Nearest);
    let mut writer = bridge.take_writer().expect("the one writer");
    bridge
        .attach(
            &desc(200, 160),
            pixels(400, 320),
            Arc::clone(&hooked) as Arc<dyn QuadSink>,
        )
        .expect("a first attach");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the attach publishes once");
    let (_, reference) = texture(&service);
    writer
        .create_quad(reference, 10.0, 10.0, 20.0, 20.0)
        .expect("a quad");
    assert!(writer.publish());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
