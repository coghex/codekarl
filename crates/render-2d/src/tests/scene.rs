//! Ordered persistent scenes, immutable publications and retained texture pins.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use ck_resources::{
    CpuPixels, HandleError, Mode, ResourceLimits, ResourceService, ResourceState, ServiceWakes,
    TextureKey, TextureRef, TokenKind,
};

use super::support::{LatestSink, RecordingSink, desc, image, pixels};
use crate::{
    Filter, Metrics, Phase, Quad, QuadError, QuadKey, QuadSink, SceneBridge, SceneLimits,
    SceneLimitsError, Snapshot, SourceRect,
};

fn service() -> ResourceService {
    ResourceService::new(ResourceLimits::DEFAULT, Mode::Headless).expect("headless resources")
}

fn texture(service: &ResourceService) -> (TextureKey, TextureRef<ResourceState>) {
    let reservation = service.reserve_pixels(1, 1).expect("one pixel reservation");
    let pixels = CpuPixels::premultiplied(reservation, vec![0; 4]).expect("RGBA8 length");
    let key = service
        .create_from_pixels(pixels)
        .unwrap_or_else(|_| panic!("texture admitted"));
    let retained = service.retain(key).expect("public texture remains live");
    (key, retained)
}

fn attach(bridge: &SceneBridge, sink: &Arc<RecordingSink>) {
    let sink: Arc<dyn QuadSink> = Arc::clone(sink) as Arc<dyn QuadSink>;
    bridge
        .attach(&desc(100, 80), pixels(200, 160), sink)
        .expect("first attach");
}

fn latest_snapshot(sink: &RecordingSink) -> Snapshot {
    sink.last().expect("snapshot published")
}

#[test]
fn quad_tokens_round_trip_and_reject_texture_tokens() {
    let service = service();
    let (texture_key, retained) = texture(&service);
    let bridge = SceneBridge::new(Filter::Nearest);
    let mut writer = bridge.take_writer().unwrap();
    let key = writer.create_quad(retained, 1.0, 2.0, 3.0, 4.0).unwrap();
    assert_eq!(QuadKey::from_token(&key.to_string()).unwrap(), key);
    assert_eq!(key.token().kind, TokenKind::Quad);
    assert_eq!(
        QuadKey::from_token(&texture_key.to_string()),
        Err(HandleError::WrongKind {
            expected: TokenKind::Quad,
            found: TokenKind::Texture,
        })
    );
}

#[test]
fn slots_reuse_oldest_first_and_stale_or_foreign_keys_are_refused() {
    let service = service();
    let bridge = SceneBridge::with_limits(Filter::Nearest, SceneLimits { max_quads: 4 }).unwrap();
    let other = SceneBridge::new(Filter::Nearest);
    let mut writer = bridge.take_writer().unwrap();
    let foreign = other
        .take_writer()
        .unwrap()
        .create_quad(texture(&service).1, 0.0, 0.0, 1.0, 1.0)
        .unwrap();
    let a = writer
        .create_quad(texture(&service).1, 0.0, 0.0, 1.0, 1.0)
        .unwrap();
    let b = writer
        .create_quad(texture(&service).1, 0.0, 0.0, 1.0, 1.0)
        .unwrap();
    writer.destroy_quad(b).unwrap();
    writer.destroy_quad(a).unwrap();
    assert!(matches!(
        writer.set_position(foreign, 1.0, 1.0),
        Err(QuadError::Foreign(_))
    ));
    assert!(matches!(
        writer.set_position(b, 1.0, 1.0),
        Err(QuadError::Stale(_))
    ));
    let first = writer
        .create_quad(texture(&service).1, 0.0, 0.0, 1.0, 1.0)
        .unwrap();
    let second = writer
        .create_quad(texture(&service).1, 0.0, 0.0, 1.0, 1.0)
        .unwrap();
    assert_eq!(
        (first.slot(), first.generation()),
        (b.slot(), b.generation() + 1)
    );
    assert_eq!(
        (second.slot(), second.generation()),
        (a.slot(), a.generation() + 1)
    );
}

#[test]
fn exhausted_generation_slots_are_never_reused() {
    let service = service();
    let bridge = SceneBridge::with_limits(Filter::Nearest, SceneLimits { max_quads: 1 }).unwrap();
    let mut writer = bridge.take_writer().unwrap();
    let first = writer
        .create_quad(texture(&service).1, 0.0, 0.0, 1.0, 1.0)
        .unwrap();
    let exhausted = writer.force_generation(first, u64::MAX);
    writer.destroy_quad(exhausted).unwrap();
    let next = writer
        .create_quad(texture(&service).1, 0.0, 0.0, 1.0, 1.0)
        .unwrap();
    assert_ne!(next.slot(), exhausted.slot());
    assert_eq!(next.generation(), 1);
}

#[test]
fn capacity_and_geometry_refusals_leave_scene_unchanged() {
    assert_eq!(SceneLimits::default().max_quads, 131_072);
    assert!(SceneLimits { max_quads: 1 }.validate().is_ok());
    assert!(
        SceneLimits {
            max_quads: 1_048_576
        }
        .validate()
        .is_ok()
    );
    let invalid = SceneLimits { max_quads: 0 }.validate().unwrap_err();
    assert_eq!((invalid.field, invalid.value), ("max_quads", 0));
    assert!(matches!(
        SceneBridge::with_limits(
            Filter::Nearest,
            SceneLimits {
                max_quads: 1_048_577
            }
        ),
        Err(SceneLimitsError {
            field: "max_quads",
            ..
        })
    ));

    let service = service();
    let bridge = SceneBridge::with_limits(Filter::Nearest, SceneLimits { max_quads: 1 }).unwrap();
    let sink = RecordingSink::new();
    attach(&bridge, &sink);
    let mut writer = bridge.take_writer().unwrap();
    let key = writer
        .create_quad(texture(&service).1, 3.0, 4.0, 5.0, 6.0)
        .unwrap();
    assert!(matches!(
        writer.create_quad(texture(&service).1, 0.0, 0.0, 1.0, 1.0),
        Err(QuadError::Full { capacity: 1 })
    ));
    assert!(matches!(
        writer.set_position(key, f64::NAN, 2.0),
        Err(QuadError::NotFinite { field: "x" })
    ));
    assert!(matches!(
        writer.set_rect(key, 8.0, 9.0, 0.0, 10.0),
        Err(QuadError::BadSize { field: "width", .. })
    ));
    assert_eq!(writer.quad_count(), 1);
    assert!(writer.publish());
    let frame = latest_snapshot(&sink).scene.unwrap();
    assert!((frame.quads()[0].x - 3.0).abs() < f64::EPSILON);
    assert_eq!(
        (frame.quads()[0].width, frame.quads()[0].height),
        (5.0, 6.0)
    );
}

#[test]
fn edits_coalesce_until_publish_and_noop_publish_keeps_generation() {
    let service = service();
    let bridge = SceneBridge::new(Filter::Nearest);
    let sink = RecordingSink::new();
    attach(&bridge, &sink);
    let mut writer = bridge.take_writer().unwrap();
    let key = writer
        .create_quad(texture(&service).1, 1.0, 2.0, 3.0, 4.0)
        .unwrap();
    writer.set_position(key, 5.0, 6.0).unwrap();
    writer.set_rect(key, 7.0, 8.0, 9.0, 10.0).unwrap();
    assert_eq!(sink.count(), 1, "edits do not publish");
    assert!(writer.publish());
    let first = latest_snapshot(&sink);
    assert_eq!(first.generation, 2);
    assert_eq!(first.scene.as_ref().unwrap().number(), 1);
    assert!((first.scene.as_ref().unwrap().quads()[0].x - 7.0).abs() < f64::EPSILON);
    assert!(!writer.publish());
    assert_eq!(sink.count(), 2, "unchanged publication is skipped");
    assert_eq!(bridge.counts().published, 1);

    let transient = writer
        .create_quad(texture(&service).1, 0.0, 0.0, 1.0, 1.0)
        .unwrap();
    writer.destroy_quad(transient).unwrap();
    assert!(
        writer.publish(),
        "create then destroy remains a scene change"
    );
    assert!(latest_snapshot(&sink).scene.unwrap().quads().len() == 1);
}

#[test]
fn painter_order_survives_destroy_edits_and_legacy_writes() {
    let service = service();
    let bridge = SceneBridge::new(Filter::Linear);
    let sink = RecordingSink::new();
    attach(&bridge, &sink);
    let mut writer = bridge.take_writer().unwrap();
    let (a_texture, a_ref) = texture(&service);
    let (b_texture, b_ref) = texture(&service);
    let (c_texture, c_ref) = texture(&service);
    let a = writer.create_quad(a_ref, 0.0, 0.0, 2.0, 2.0).unwrap();
    let b = writer.create_quad(b_ref, 1.0, 1.0, 2.0, 2.0).unwrap();
    let _c = writer.create_quad(c_ref, 2.0, 2.0, 2.0, 2.0).unwrap();
    writer.destroy_quad(b).unwrap();
    let (d_texture, d_ref) = texture(&service);
    let _d = writer.create_quad(d_ref, 3.0, 3.0, 2.0, 2.0).unwrap();
    writer.set_position(a, 9.0, 9.0).unwrap();
    assert!(writer.publish());
    let frame = latest_snapshot(&sink).scene.unwrap();
    assert_eq!(
        frame
            .quads()
            .iter()
            .map(|quad| quad.texture.key())
            .collect::<Vec<_>>(),
        vec![a_texture, c_texture, d_texture],
    );
    assert!(
        !frame
            .quads()
            .iter()
            .any(|quad| quad.texture.key() == b_texture)
    );
    assert_eq!(
        frame.quads().iter().map(|quad| quad.x).collect::<Vec<_>>(),
        vec![9.0, 2.0, 3.0]
    );
    writer.set_quad(Quad::new(image(4, 4), 0.0, 0.0, Some(4.0), Some(4.0)).unwrap());
    let with_legacy = latest_snapshot(&sink);
    assert!(with_legacy.quad.is_some());
    assert!(Arc::ptr_eq(with_legacy.scene.as_ref().unwrap(), &frame));
    writer.clear_quad();
    let without_legacy = latest_snapshot(&sink);
    assert!(without_legacy.quad.is_none());
    assert!(Arc::ptr_eq(without_legacy.scene.as_ref().unwrap(), &frame));
}

#[test]
fn latest_and_consumer_frames_pin_texture_until_the_last_snapshot_is_dropped() {
    let bridge = SceneBridge::new(Filter::Nearest);
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed_wake = Arc::clone(&observed);
    let bridge_weak = Arc::downgrade(&bridge);
    let wakes = ServiceWakes {
        retirements: Some(Arc::new(move || {
            let held = bridge_weak
                .upgrade()
                .is_some_and(|bridge| bridge.lock_is_held());
            observed_wake
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(held);
        })),
        ..ServiceWakes::default()
    };
    let service = ResourceService::with_wakes(ResourceLimits::DEFAULT, Mode::Headless, wakes)
        .expect("headless resources with retirement observation");
    let sink = LatestSink::new();
    let sink_trait: Arc<dyn QuadSink> = Arc::clone(&sink) as Arc<dyn QuadSink>;
    bridge
        .attach(&desc(20, 20), pixels(20, 20), sink_trait)
        .unwrap();
    let mut writer = bridge.take_writer().unwrap();
    let (key, retained) = texture(&service);
    let quad = writer.create_quad(retained, 0.0, 0.0, 1.0, 1.0).unwrap();
    assert!(writer.publish());
    let consumer = sink.latest().unwrap();
    service.release(key).unwrap();
    writer.destroy_quad(quad).unwrap();
    assert!(writer.publish());
    assert_eq!(
        service.stats().registry.retiring,
        0,
        "consumer snapshot still pins the texture"
    );
    assert_eq!(consumer.scene.as_ref().unwrap().quads().len(), 1);
    drop(consumer);
    assert_eq!(service.stats().registry.retiring, 1);
    assert_eq!(
        *observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![false],
        "dropping the last frame pin wakes retirement after the bridge lock is released"
    );
    bridge.detach();
    assert_eq!(service.maintain(8).finished, 1);
}

#[test]
fn frame_texture_drops_on_displacement_and_detach_are_outside_the_lock() {
    let bridge = SceneBridge::new(Filter::Nearest);
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed_wake = Arc::clone(&observed);
    let bridge_weak = Arc::downgrade(&bridge);
    let wakes = ServiceWakes {
        retirements: Some(Arc::new(move || {
            let held = bridge_weak
                .upgrade()
                .is_some_and(|bridge| bridge.lock_is_held());
            observed_wake
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(held);
        })),
        ..ServiceWakes::default()
    };
    let service = ResourceService::with_wakes(ResourceLimits::DEFAULT, Mode::Headless, wakes)
        .expect("headless resources with retirement observation");
    let sink = LatestSink::new();
    let sink_trait: Arc<dyn QuadSink> = Arc::clone(&sink) as Arc<dyn QuadSink>;
    bridge
        .attach(&desc(20, 20), pixels(20, 20), sink_trait)
        .unwrap();
    let mut writer = bridge.take_writer().unwrap();

    let (key, retained) = texture(&service);
    let quad = writer.create_quad(retained, 0.0, 0.0, 1.0, 1.0).unwrap();
    assert!(writer.publish());
    service.release(key).unwrap();
    writer.destroy_quad(quad).unwrap();
    assert!(
        writer.publish(),
        "replacing the last frame drops its texture pin"
    );
    assert_eq!(service.stats().registry.retiring, 1);
    assert_eq!(
        *observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![false],
        "displaced frame reclamation wakes only after the bridge lock is released"
    );
    assert_eq!(service.maintain(8).finished, 1);

    let (key, retained) = texture(&service);
    let quad = writer.create_quad(retained, 0.0, 0.0, 1.0, 1.0).unwrap();
    assert!(writer.publish());
    service.release(key).unwrap();
    writer.destroy_quad(quad).unwrap();
    bridge.detach();
    assert_eq!(service.stats().registry.retiring, 1);
    assert_eq!(
        *observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![false, false],
        "detach releases the latest frame after the bridge lock is released"
    );
}

#[test]
fn a_key_without_a_retained_texture_reference_does_not_pin_the_resource() {
    let service = service();
    let (key, retained) = texture(&service);
    drop(retained);
    service.release(key).unwrap();
    assert_eq!(
        service.stats().registry.retiring,
        1,
        "negative control: a key is not a TextureRef"
    );
    assert_eq!(service.maintain(8).finished, 1);
}

#[test]
fn a_unique_displaced_frame_recycles_its_cleared_vector() {
    let service = service();
    let bridge = SceneBridge::new(Filter::Nearest);
    let sink = LatestSink::new();
    let sink_trait: Arc<dyn QuadSink> = Arc::clone(&sink) as Arc<dyn QuadSink>;
    bridge
        .attach(&desc(10, 10), pixels(10, 10), sink_trait)
        .unwrap();
    let mut writer = bridge.take_writer().unwrap();
    let key = writer
        .create_quad(texture(&service).1, 0.0, 0.0, 1.0, 1.0)
        .unwrap();
    assert!(writer.publish());
    writer.set_position(key, 1.0, 1.0).unwrap();
    assert!(writer.publish());
    let capacity = writer.spare_capacity();
    assert!(capacity >= 1);
    writer.set_position(key, 2.0, 2.0).unwrap();
    assert!(writer.publish());
    assert_eq!(
        writer.spare_capacity(),
        capacity,
        "the spare allocation is reused"
    );
}

#[test]
fn frame_churn_stays_within_live_frame_bounds_while_consumer_holds_a_snapshot() {
    let service = service();
    let bridge = SceneBridge::new(Filter::Nearest);
    let sink = LatestSink::new();
    let sink_trait: Arc<dyn QuadSink> = Arc::clone(&sink) as Arc<dyn QuadSink>;
    bridge
        .attach(&desc(10, 10), pixels(10, 10), sink_trait)
        .unwrap();
    let observed_max = Arc::new(AtomicUsize::new(0));
    let max_probe = Arc::clone(&observed_max);
    let bridge_probe = Arc::clone(&bridge);
    let mut writer = bridge.take_writer().unwrap();
    let key = writer
        .create_quad(texture(&service).1, 0.0, 0.0, 1.0, 1.0)
        .unwrap();
    writer.set_build_probe(move || {
        max_probe.fetch_max(bridge_probe.live_frames(), Ordering::SeqCst);
    });
    assert!(writer.publish());
    let mut consumer = Some(sink.latest().unwrap());

    for update in 0..256 {
        writer
            .set_position(key, f64::from(update), f64::from(update))
            .unwrap();
        assert!(writer.publish());
        assert!(
            bridge.live_frames() <= 3,
            "at rest the bridge, consumer and spare must stay within three frames"
        );
        if update % 7 == 0 {
            drop(consumer.take());
        } else if update % 7 == 1 {
            consumer = Some(sink.latest().unwrap());
        }
    }
    assert!(
        observed_max.load(Ordering::SeqCst) <= 4,
        "publication may temporarily keep at most four live frames"
    );
    drop(consumer);
    bridge.detach();
    assert_eq!(bridge.live_frames(), 0);
}

#[test]
fn build_runs_outside_the_lock_and_a_metrics_race_keeps_snapshot_coherent() {
    let service = service();
    let bridge = SceneBridge::new(Filter::Nearest);
    let sink = RecordingSink::new();
    attach(&bridge, &sink);
    let (entered_tx, entered_rx) = mpsc::channel();
    let gate = Arc::new(Barrier::new(2));
    let bridge_for_probe = Arc::clone(&bridge);
    let gate_for_probe = Arc::clone(&gate);
    let mut writer = bridge.take_writer().unwrap();
    let _key = writer
        .create_quad(texture(&service).1, 0.0, 0.0, 2.0, 2.0)
        .unwrap();
    writer.set_build_probe(move || {
        entered_tx.send(bridge_for_probe.lock_is_held()).unwrap();
        gate_for_probe.wait();
    });
    let publishing = thread::spawn(move || {
        let published = writer.publish();
        (writer, published)
    });
    assert!(!entered_rx.recv_timeout(Duration::from_secs(2)).unwrap());
    bridge.set_pixel_size(pixels(300, 240));
    gate.wait();
    let (_writer, published) = publishing.join().unwrap();
    assert!(published);
    let snapshots = sink.snapshots();
    assert_eq!(snapshots.last().unwrap().metrics.pixels, pixels(300, 240));
    assert_eq!(
        snapshots
            .last()
            .unwrap()
            .scene
            .as_ref()
            .unwrap()
            .quads()
            .len(),
        1
    );
    assert!(
        snapshots
            .windows(2)
            .all(|pair| pair[0].generation < pair[1].generation)
    );
}

#[test]
fn publication_before_attach_is_in_the_first_snapshot_and_detach_discards_without_building() {
    let service = service();
    let bridge = SceneBridge::new(Filter::Nearest);
    let mut writer = bridge.take_writer().unwrap();
    let key = writer
        .create_quad(texture(&service).1, 0.0, 0.0, 2.0, 2.0)
        .unwrap();
    assert!(writer.publish());
    let sink = RecordingSink::new();
    attach(&bridge, &sink);
    let first = latest_snapshot(&sink);
    assert_eq!(first.scene.unwrap().quads().len(), 1);
    bridge.detach();
    let builds = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&builds);
    writer.set_build_probe(move || {
        calls.fetch_add(1, Ordering::SeqCst);
    });
    writer.set_position(key, 1.0, 1.0).unwrap();
    let before = sink.count();
    assert!(!writer.publish());
    assert_eq!(
        builds.load(Ordering::SeqCst),
        0,
        "ended phase is checked before O(quads) build"
    );
    assert_eq!(
        sink.count(),
        before,
        "nothing is published after detach returns"
    );
    assert_eq!(bridge.phase(), Phase::Ended);
    assert_eq!(bridge.counts().publication_discarded, 1);
}

#[test]
fn detach_during_build_discards_the_frame_after_the_end() {
    let service = service();
    let bridge = SceneBridge::new(Filter::Nearest);
    let sink = RecordingSink::new();
    attach(&bridge, &sink);
    let (entered_tx, entered_rx) = mpsc::channel();
    let gate = Arc::new(Barrier::new(2));
    let gate_probe = Arc::clone(&gate);
    let mut writer = bridge.take_writer().unwrap();
    writer
        .create_quad(texture(&service).1, 0.0, 0.0, 1.0, 1.0)
        .unwrap();
    writer.set_build_probe(move || {
        entered_tx.send(()).unwrap();
        gate_probe.wait();
    });
    let publishing = thread::spawn(move || writer.publish());
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    bridge.detach();
    let after_detach = sink.count();
    gate.wait();
    assert!(!publishing.join().unwrap());
    assert_eq!(sink.count(), after_detach);
    assert_eq!(bridge.counts().publication_discarded, 1);
}

#[test]
fn frame_placement_matches_legacy_destination_and_normalized_source() {
    let service = service();
    let (_, texture) = texture(&service);
    let cases = [
        (
            0.5,
            1.25,
            8.0,
            4.0,
            Metrics {
                points_width: 10,
                points_height: 8,
                pixels: pixels(20, 12),
            },
        ),
        (
            -2.0,
            1.0,
            6.0,
            4.0,
            Metrics {
                points_width: 10,
                points_height: 8,
                pixels: pixels(20, 12),
            },
        ),
        (
            3.0,
            -1.0,
            4.0,
            5.0,
            Metrics {
                points_width: 10,
                points_height: 8,
                pixels: pixels(20, 12),
            },
        ),
        (
            50.0,
            50.0,
            3.0,
            3.0,
            Metrics {
                points_width: 10,
                points_height: 8,
                pixels: pixels(20, 12),
            },
        ),
    ];
    for (x, y, width, height, metrics) in cases {
        let legacy = Quad::new(image(8, 4), x, y, Some(width), Some(height)).unwrap();
        let frame = crate::FrameQuad {
            texture: texture.clone(),
            x,
            y,
            width,
            height,
        };
        let old_place = legacy.place(&metrics);
        let frame_place = frame.place(&metrics);
        assert_eq!(old_place.is_some(), frame_place.is_some());
        if let (Some(old), Some(new)) = (old_place, frame_place) {
            assert_eq!(old.destination, new.destination);
            let expected = SourceRect {
                left: old.source.left / 8.0,
                top: old.source.top / 4.0,
                right: old.source.right / 8.0,
                bottom: old.source.bottom / 4.0,
            };
            assert_eq!(new.source, expected);
        }
    }
}

#[test]
fn place_is_placement_plus_the_texture_for_every_geometry_case() {
    let service = service();
    let (_key, texture) = texture(&service);
    let scale_one = Metrics {
        points_width: 10,
        points_height: 8,
        pixels: pixels(10, 8),
    };
    let scale_two = Metrics {
        points_width: 10,
        points_height: 8,
        pixels: pixels(20, 16),
    };
    // (x, y, width, height): on screen; clipped at the left, the top, the right and the bottom (the source trimmed); wholly off
    // screen; and without extent in pixels.
    let rects = [
        (2.0, 2.0, 4.0, 3.0),
        (-2.0, 1.0, 6.0, 4.0),
        (3.0, -1.0, 4.0, 5.0),
        (8.0, 1.0, 6.0, 4.0),
        (3.0, 6.0, 4.0, 5.0),
        (50.0, 50.0, 3.0, 3.0),
        (4.0, 4.0, 0.0, 3.0),
        (4.0, 4.0, 3.0, 0.0),
        (0.5, 1.25, 8.0, 4.0),
    ];
    let mut placed_cases = 0;
    for metrics in [scale_one, scale_two] {
        for (x, y, width, height) in rects {
            let frame = crate::FrameQuad {
                texture: texture.clone(),
                x,
                y,
                width,
                height,
            };
            let placement = frame.placement(&metrics);
            let placed = frame.place(&metrics);
            assert_eq!(
                placement.is_some(),
                placed.is_some(),
                "both place or neither, at {x} {y} {width} {height}"
            );
            if let (Some(placement), Some(placed)) = (placement, placed) {
                assert_eq!(placement.destination, placed.destination);
                assert_eq!(placement.source, placed.source);
                assert_eq!(placed.texture.key(), texture.key());
                placed_cases += 1;
            }
        }
    }
    assert!(placed_cases >= 10, "the cases include placed quads");
}
