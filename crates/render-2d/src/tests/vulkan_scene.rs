//! The Vulkan resource-aware scene side, including its caller-owned service wake relay.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ck_gpu_vulkan::{InstanceRole, VulkanFilter};
use ck_resources::{
    CpuPixels, Mode, ResourceLimits, ResourceService, ResourceState, ServiceWakes, TextureKey,
    TextureRef,
};

use super::support::{image, pixels};
use crate::vulkan::{Conversion, Refusals, VulkanSceneSink};
use crate::{
    Filter, FrameQuad, Metrics, PixelRect, PlacedQuad, QuadSink, SceneFrame, Snapshot, SourceRect,
    VulkanResourceLimits, VulkanSinkSide, WakeRelay,
};

fn service_with(relay: &WakeRelay) -> ResourceService {
    ResourceService::with_wakes(
        ResourceLimits::DEFAULT,
        Mode::Backend,
        ServiceWakes {
            uploads: Some(relay.wake()),
            retirements: Some(relay.wake()),
            ..ServiceWakes::default()
        },
    )
    .expect("backend resource service")
}

fn retained(service: &ResourceService) -> (TextureKey, TextureRef<ResourceState>) {
    let reservation = service.reserve_pixels(1, 1).expect("pixel budget");
    let pixels = CpuPixels::premultiplied(reservation, vec![0; 4]).expect("RGBA8 bytes");
    let key = service
        .create_from_pixels(pixels)
        .expect("resource admission");
    (key, service.retain(key).expect("retain live resource"))
}

fn finish_upload(service: &ResourceService) -> TextureKey {
    let reservation = service.reserve_pixels(1, 1).expect("pixel budget");
    let pixels = CpuPixels::premultiplied(reservation, vec![0; 4]).expect("RGBA8 bytes");
    let key = service
        .create_from_pixels(pixels)
        .expect("resource admission");
    let job = service.take_job().expect("CPU job");
    service.complete(job.run());
    key
}

fn sample_snapshot(generation: u64, scene: Option<Arc<SceneFrame>>) -> Snapshot {
    Snapshot {
        generation,
        filter: Filter::Linear,
        requested: false,
        quad: Some(PlacedQuad {
            image: image(4, 4),
            destination: PixelRect {
                left: 1,
                top: 2,
                right: 5,
                bottom: 6,
            },
            source: SourceRect {
                left: 0.0,
                top: 0.0,
                right: 4.0,
                bottom: 4.0,
            },
        }),
        scene,
        metrics: Metrics {
            points_width: 20,
            points_height: 20,
            pixels: pixels(40, 40),
        },
    }
}

fn frame(
    service: &ResourceService,
) -> (
    Arc<SceneFrame>,
    TextureRef<ResourceState>,
    TextureRef<ResourceState>,
) {
    let (_, compat) = retained(service);
    let (_, placed) = retained(service);
    let scene = Arc::new(SceneFrame::new(
        8,
        Some(FrameQuad {
            texture: compat.clone(),
            x: 1.0,
            y: 2.0,
            width: 4.0,
            height: 5.0,
        }),
        vec![FrameQuad {
            texture: placed.clone(),
            x: 8.0,
            y: 9.0,
            width: 3.0,
            height: 2.0,
        }],
        Arc::new(AtomicUsize::new(1)),
        None,
    ));
    (scene, compat, placed)
}

fn sink(
    placements: Arc<AtomicUsize>,
) -> (
    Arc<VulkanSceneSink>,
    Arc<ck_gpu_vulkan::VulkanFrameSlot<TextureKey>>,
) {
    let slot = ck_gpu_vulkan::VulkanFrameSlot::new();
    let sink = VulkanSceneSink::over(
        Arc::clone(&slot),
        Arc::new(Refusals::default()),
        Arc::new(|| {}),
        crate::vulkan::real_converter(),
        Some(placements),
    );
    (sink, slot)
}

#[test]
fn publication_is_o1_returns_the_previous_snapshot_and_placement_uses_snapshot_metrics() {
    let relay = WakeRelay::default();
    let service = service_with(&relay);
    let (scene, _compat, placed) = frame(&service);
    let placements = Arc::new(AtomicUsize::new(0));
    let (sink, _slot) = sink(Arc::clone(&placements));
    let first = sample_snapshot(10, Some(Arc::clone(&scene)));
    assert!(sink.publish(first).is_none(), "nothing was displaced");
    assert_eq!(
        placements.load(Ordering::SeqCst),
        0,
        "publish does no placement"
    );

    let inspect = sample_snapshot(10, Some(Arc::clone(&scene)));
    let source_count = Arc::strong_count(&scene);
    let converted = Conversion::default().next(&inspect);
    let published = sink.frame_for(&inspect, &converted.scene);
    assert_eq!(
        Arc::strong_count(&scene),
        source_count + 1,
        "the frame source shares this SceneFrame Arc"
    );
    assert_eq!(published.generation(), 10);
    assert_eq!(published.filter(), VulkanFilter::Linear);
    let legacy = published.quad().expect("the legacy compatibility quad");
    assert_eq!(
        (legacy.destination().left, legacy.destination().top),
        (1, 2)
    );
    assert_eq!(
        (legacy.destination().right, legacy.destination().bottom),
        (5, 6)
    );
    let source = published.scene().expect("scene source");
    let before = placed.reference_count();
    let mut instances = Vec::new();
    source.place(1, 1, &mut |instance| instances.push(instance));
    assert_eq!(placements.load(Ordering::SeqCst), 1);
    assert_eq!(instances.len(), 2);
    assert_eq!(instances[0].role, InstanceRole::Compat);
    assert_eq!(instances[1].role, InstanceRole::Scene);
    assert_eq!(instances[0].destination.left, 2);
    assert_eq!(instances[1].destination.left, 16);
    assert_eq!(
        placed.reference_count(),
        before,
        "placement clones no TextureRef"
    );

    let second = sample_snapshot(11, Some(Arc::clone(&scene)));
    let displaced = sink
        .publish(second)
        .expect("the prior snapshot is returned");
    assert_eq!(displaced.generation, 10);
}

#[test]
fn clipped_compatibility_quad_keeps_its_key_even_when_no_instance_is_emitted() {
    let relay = WakeRelay::default();
    let service = service_with(&relay);
    let (key, reference) = retained(&service);
    let clipped = Arc::new(SceneFrame::new(
        9,
        Some(FrameQuad {
            texture: reference,
            x: 100.0,
            y: 100.0,
            width: 2.0,
            height: 2.0,
        }),
        Vec::new(),
        Arc::new(AtomicUsize::new(1)),
        None,
    ));
    let placements = Arc::new(AtomicUsize::new(0));
    let (sink, _slot) = sink(Arc::clone(&placements));
    let snapshot = sample_snapshot(12, Some(clipped));
    let converted = Conversion::default().next(&snapshot);
    let published = sink.frame_for(&snapshot, &converted.scene);
    assert_eq!(published.compat_key(), Some(&key));
    let mut instances = Vec::new();
    published
        .scene()
        .expect("scene source")
        .place(40, 40, &mut |instance| instances.push(instance));
    assert!(instances.is_empty());
}

#[test]
fn resource_side_accepts_only_its_scene_sink_and_exposes_the_driver_and_relay() {
    let relay = WakeRelay::default();
    let service = service_with(&relay);
    let mut side = VulkanSinkSide::with_resources(
        Arc::new(|| {}),
        service,
        relay,
        VulkanResourceLimits {
            max_adoptions_per_frame: 17,
        },
    );
    let sink = side.new_sink();
    assert!(side.accept_frames(&sink).is_ok());
    assert_eq!(side.resident_driver().unwrap().stats().passes, 0);
    let foreign = VulkanSinkSide::new(Arc::new(|| {})).new_sink();
    assert!(side.accept_frames(&foreign).is_err());
    let target_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&target_calls);
    side.connect(Arc::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
    }))
    .unwrap();
    side.disconnect().unwrap();
    side.disconnect().unwrap();
    drop(sink);
}

#[test]
fn service_upload_wakes_before_and_after_connect_and_eligible_retirement_reach_the_target() {
    let relay = WakeRelay::default();
    let service = service_with(&relay);
    let target_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&target_calls);
    let mut side = VulkanSinkSide::with_resources(
        Arc::new(|| {}),
        service.clone(),
        relay,
        VulkanResourceLimits {
            max_adoptions_per_frame: 8,
        },
    );
    let _sink = side.new_sink();

    let _before = finish_upload(&service);
    assert_eq!(
        target_calls.load(Ordering::SeqCst),
        0,
        "wake is sticky before connect"
    );
    side.connect(Arc::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
    }))
    .unwrap();
    assert_eq!(
        target_calls.load(Ordering::SeqCst),
        1,
        "pending wake delivered once"
    );

    let mut uploads = Vec::new();
    assert_eq!(
        service.take_uploads(&mut uploads, 8),
        1,
        "drain the pre-connect upload"
    );

    let _first_after = finish_upload(&service);
    assert_eq!(target_calls.load(Ordering::SeqCst), 2);
    uploads.clear();
    assert_eq!(
        service.take_uploads(&mut uploads, 8),
        1,
        "drain first upload"
    );
    let _second_after = finish_upload(&service);
    assert_eq!(
        target_calls.load(Ordering::SeqCst),
        3,
        "empty-to-nonempty wakes again"
    );
    uploads.clear();
    assert_eq!(service.take_uploads(&mut uploads, 8), 1);

    let retiring_key = finish_upload(&service);
    let reference = service
        .retain(retiring_key)
        .expect("retain upload resource");
    assert_eq!(
        target_calls.load(Ordering::SeqCst),
        4,
        "retirement fixture upload wake"
    );
    uploads.clear();
    assert_eq!(service.take_uploads(&mut uploads, 8), 1);
    let before_retirement = target_calls.load(Ordering::SeqCst);
    service
        .release(retiring_key)
        .expect("release the public resource");
    drop(reference);
    assert_eq!(
        service.maintain(8).started,
        1,
        "registry reports eligible retirement"
    );
    let after_retirement = target_calls.load(Ordering::SeqCst);
    assert!(
        after_retirement > before_retirement,
        "eligible retirement reached the connected target"
    );
    side.disconnect().unwrap();
    side.disconnect().unwrap();
    uploads.clear();
    service.take_uploads(&mut uploads, 8);
    let _after_disconnect = finish_upload(&service);
    assert_eq!(
        target_calls.load(Ordering::SeqCst),
        after_retirement,
        "a quiescent disconnect suppresses future target delivery"
    );
}

#[test]
fn dropping_the_resource_side_disconnects_its_caller_owned_relay() {
    let relay = WakeRelay::default();
    let service = service_with(&relay);
    let target_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&target_calls);
    let mut side = VulkanSinkSide::with_resources(
        Arc::new(|| {}),
        service.clone(),
        relay,
        VulkanResourceLimits {
            max_adoptions_per_frame: 8,
        },
    );
    side.new_sink();
    side.connect(Arc::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
    }))
    .unwrap();
    drop(side);
    let _ = finish_upload(&service);
    assert_eq!(target_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_fresh_unrelated_relay_is_a_negative_control_for_the_service_wake() {
    let service_relay = WakeRelay::default();
    let service = service_with(&service_relay);
    let unrelated = WakeRelay::default();
    let mut side = VulkanSinkSide::with_resources(
        Arc::new(|| {}),
        service.clone(),
        unrelated,
        VulkanResourceLimits {
            max_adoptions_per_frame: 8,
        },
    );
    let _sink = side.new_sink();
    let target_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&target_calls);
    side.connect(Arc::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
    }))
    .unwrap();
    let _ = finish_upload(&service);
    assert_eq!(
        target_calls.load(Ordering::SeqCst),
        0,
        "the wrong relay sees no service wake"
    );
}
