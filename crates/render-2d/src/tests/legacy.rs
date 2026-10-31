//! Synchronous legacy admission and its independent compatibility lane.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ck_resources::{
    Assets, CpuPixels, LegacyAdmitError, LegacyBudget, Mode, ResourceLimits, ResourceService,
    ServiceWakes, StatusState,
};

use super::support::{LatestSink, desc, pixels};
use crate::{Filter, LegacyError, LegacyLane, Phase, QuadSink, SceneBridge};

const GOOD: &[u8] = include_bytes!("fixtures/rgba8.png");

fn assets() -> (PathBuf, Assets) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "ck-render2d-legacy-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    fs::create_dir(&root).expect("unique assets directory");
    fs::write(root.join("good.png"), GOOD).expect("valid PNG fixture");
    fs::write(root.join("broken.png"), b"not a PNG").expect("broken fixture");
    let assets = Assets::new(&root);
    (root, assets)
}

fn backend(assets: Option<Assets>, wakes: ServiceWakes, limits: ResourceLimits) -> ResourceService {
    ResourceService::with_assets(limits, Mode::Backend, wakes, assets).expect("backend resources")
}

fn frame(writer: &mut crate::SceneWriter, sink: &LatestSink) -> Arc<crate::SceneFrame> {
    assert!(writer.publish());
    sink.latest()
        .and_then(|snapshot| snapshot.scene)
        .expect("published scene frame")
}

#[test]
fn compat_is_separate_ordered_below_persistent_quads_and_publication_coalesces() {
    let (root, assets) = assets();
    let service = backend(
        Some(Assets::new(&root)),
        ServiceWakes::default(),
        ResourceLimits::DEFAULT,
    );
    let lane = LegacyLane::new(service.clone(), Some(assets));
    let bridge = SceneBridge::new(Filter::Nearest);
    let sink = LatestSink::new();
    let sink_trait: Arc<dyn QuadSink> = sink.clone();
    bridge
        .attach(&desc(100, 80), pixels(100, 80), sink_trait)
        .unwrap();
    let mut writer = bridge.take_writer().unwrap();

    let reservation = service.reserve_pixels(1, 1).unwrap();
    let pixels = CpuPixels::premultiplied(reservation, vec![0; 4]).unwrap();
    let persistent_key = service.create_from_pixels(pixels).unwrap();
    let persistent = service.retain(persistent_key).unwrap();
    service.release(persistent_key).unwrap();
    writer.create_quad(persistent, 1.0, 2.0, 3.0, 4.0).unwrap();

    lane.quad(&writer, "good.png", 5.0, 6.0, None, None)
        .unwrap();
    let first = frame(&mut writer, &sink);
    let first_compat = first.compat().unwrap();
    let first_key = first_compat.texture.key();
    assert_eq!((first_compat.width, first_compat.height), (2.0, 2.0));
    assert_eq!(first.quads().len(), 1);
    assert!(
        service.check(first_key).is_err(),
        "the public key was released after retain"
    );
    assert_eq!(
        service.status_of(&first_compat.texture).state,
        StatusState::Uploading
    );

    lane.quad(&writer, "good.png", 10.0, 11.0, Some(20.0), Some(21.0))
        .unwrap();
    lane.quad(&writer, "good.png", 30.0, 31.0, Some(40.0), Some(41.0))
        .unwrap();
    let second = frame(&mut writer, &sink);
    assert_eq!(
        second.quads().len(),
        1,
        "compat replacement must not reorder the scene"
    );
    let compat = second.compat().unwrap();
    assert_eq!(
        (compat.x, compat.y, compat.width, compat.height),
        (30.0, 31.0, 40.0, 41.0)
    );
    assert_ne!(compat.texture.key(), first_key);
    assert!(
        first.compat().is_some(),
        "an older immutable frame pins its own texture"
    );

    lane.clear(&writer);
    let cleared = frame(&mut writer, &sink);
    assert!(cleared.compat().is_none());
    assert_eq!(
        cleared.quads().len(),
        1,
        "clearing compat preserves persistent quads"
    );
    drop(sink.take());
    drop((cleared, second, first, writer));
    bridge.detach();
    service.close();
    service.detach_backend();
    let _ = service.finish_teardown();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_decode_geometry_and_shared_budget_failures_preserve_compat() {
    let (root, assets) = assets();
    let mut limits = ResourceLimits::DEFAULT;
    limits.max_textures = 2;
    let service = backend(Some(Assets::new(&root)), ServiceWakes::default(), limits);
    let lane = LegacyLane::new(service.clone(), Some(assets));
    let bridge = SceneBridge::new(Filter::Nearest);
    let sink = LatestSink::new();
    let sink_trait: Arc<dyn QuadSink> = sink.clone();
    bridge
        .attach(&desc(100, 80), pixels(100, 80), sink_trait)
        .unwrap();
    let mut writer = bridge.take_writer().unwrap();
    lane.quad(&writer, "good.png", 2.0, 3.0, None, None)
        .unwrap();
    let original = frame(&mut writer, &sink);
    let original_key = original.compat().unwrap().texture.key();
    assert!(matches!(
        lane.quad(&writer, "missing.png", 0.0, 0.0, None, None),
        Err(LegacyError::Assets(_))
    ));
    assert!(matches!(
        lane.quad(&writer, "broken.png", 0.0, 0.0, None, None),
        Err(LegacyError::Assets(_))
    ));
    assert!(matches!(
        lane.quad(&writer, "good.png", 0.0, 0.0, Some(f64::NAN), None),
        Err(LegacyError::Geometry(_))
    ));
    let _ = service.maintain(usize::MAX);
    let mut retirements = Vec::new();
    service.take_retirements(&mut retirements, usize::MAX);
    for key in retirements {
        service.retired(key);
    }
    let _ = service.maintain(usize::MAX);
    let held_pixels = service.reserve_pixels(1, 1).unwrap();
    let held_pixels = CpuPixels::premultiplied(held_pixels, vec![0; 4]).unwrap();
    let held_key = service.create_from_pixels(held_pixels).unwrap();
    let before = service.stats();
    assert!(matches!(
        lane.quad(&writer, "good.png", 0.0, 0.0, None, None),
        Err(LegacyError::Admission(LegacyAdmitError::Budget {
            budget: LegacyBudget::RegistrySlots,
            ..
        }))
    ));
    assert!(
        !writer.publish(),
        "failed legacy calls do not dirty the scene"
    );
    let current = sink.latest().unwrap().scene.unwrap();
    assert_eq!(current.compat().unwrap().texture.key(), original_key);
    assert_eq!(
        service.stats().cpu_pixels.reserved,
        before.cpu_pixels.reserved
    );
    assert_eq!(service.stats().metadata.reserved, before.metadata.reserved);
    service.release(held_key).unwrap();
    drop(sink.take());
    drop((current, original, writer));
    bridge.detach();
    service.close();
    service.detach_backend();
    let _ = service.finish_teardown();
    fs::remove_dir_all(root).unwrap();
}

/// Other resources consume shared CPU capacity; refusal leaves the compatibility image untouched. Negative control:
/// release the competing resource, then the same valid image is admitted and replaces it.
#[test]
fn aggregate_cpu_budget_refusal_preserves_previous_compat() {
    let (root, assets) = assets();
    let mut limits = ResourceLimits::DEFAULT;
    limits.cpu_pixel_bytes = 64 * 1024 * 1024 + 24;
    let service = backend(Some(Assets::new(&root)), ServiceWakes::default(), limits);
    let lane = LegacyLane::new(service.clone(), Some(assets));
    let bridge = SceneBridge::new(Filter::Nearest);
    let sink = LatestSink::new();
    let sink_trait: Arc<dyn QuadSink> = sink.clone();
    bridge
        .attach(&desc(100, 80), pixels(100, 80), sink_trait)
        .unwrap();
    let mut writer = bridge.take_writer().unwrap();

    lane.quad(&writer, "good.png", 2.0, 3.0, None, None)
        .unwrap();
    let original = frame(&mut writer, &sink);
    let original_key = original.compat().unwrap().texture.key();

    let held_pixels = service.reserve_pixels(4096, 4096).unwrap();
    let held_bytes = vec![0; held_pixels.byte_len()];
    let held_pixels = CpuPixels::premultiplied(held_pixels, held_bytes).unwrap();
    let held_key = service.create_from_pixels(held_pixels).unwrap();
    assert_eq!(service.stats().cpu_pixels.reserved, 64 * 1024 * 1024 + 16);
    assert!(matches!(
        lane.quad(&writer, "good.png", 8.0, 9.0, None, None),
        Err(LegacyError::Admission(LegacyAdmitError::Budget {
            budget: LegacyBudget::CpuPixelBytes,
            requested: 16,
            available: 8,
        }))
    ));
    assert!(
        !writer.publish(),
        "the refused replacement does not dirty the scene"
    );
    let current = sink.latest().unwrap().scene.unwrap();
    assert_eq!(current.compat().unwrap().texture.key(), original_key);
    assert_eq!(service.stats().cpu_pixels.reserved, 64 * 1024 * 1024 + 16);

    service.release(held_key).unwrap();
    let _ = service.maintain(usize::MAX);
    let mut retirements = Vec::new();
    service.take_retirements(&mut retirements, usize::MAX);
    for key in retirements {
        service.retired(key);
    }
    let _ = service.maintain(usize::MAX);
    lane.quad(&writer, "good.png", 8.0, 9.0, None, None)
        .expect("the same image fits after the competing resource retires");
    let replaced = frame(&mut writer, &sink);
    assert_ne!(replaced.compat().unwrap().texture.key(), original_key);

    drop(sink.take());
    drop((replaced, current, original, writer));
    bridge.detach();
    service.close();
    service.detach_backend();
    let _ = service.finish_teardown();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn bridge_end_during_admission_discards_and_releases_the_new_texture() {
    let (root, assets) = assets();
    let bridge = SceneBridge::new(Filter::Nearest);
    let bridge_for_wake = Arc::clone(&bridge);
    let wakes = ServiceWakes {
        uploads: Some(Arc::new(move || bridge_for_wake.detach())),
        ..ServiceWakes::default()
    };
    let service = backend(Some(Assets::new(&root)), wakes, ResourceLimits::DEFAULT);
    let lane = LegacyLane::new(service.clone(), Some(assets));
    let mut writer = bridge.take_writer().unwrap();

    lane.quad(&writer, "good.png", 0.0, 0.0, None, None)
        .unwrap();
    assert_eq!(bridge.phase(), Phase::Ended);
    assert_eq!(bridge.counts().discarded, 1);
    assert!(!writer.publish());
    assert_eq!(bridge.counts().publication_discarded, 0);
    service.close();
    service.detach_backend();
    let _ = service.maintain(usize::MAX);
    assert_eq!(service.stats().cpu_pixels.reserved, 0);
    assert_eq!(service.stats().metadata.reserved, 0);
    let _ = service.finish_teardown();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ended_bridge_and_service_keep_legacy_validation_and_discard_semantics() {
    let (root, _) = assets();
    let service = backend(
        Some(Assets::new(&root)),
        ServiceWakes::default(),
        ResourceLimits::DEFAULT,
    );
    let lane = LegacyLane::new(service.clone(), Some(Assets::new(&root)));
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    bridge.detach();
    let before = service.stats();
    assert!(matches!(
        lane.quad(&writer, "broken.png", 0.0, 0.0, None, None),
        Err(LegacyError::Assets(_))
    ));
    assert!(matches!(
        lane.quad(&writer, "good.png", 0.0, 0.0, Some(f64::NAN), None),
        Err(LegacyError::Geometry(_))
    ));
    lane.quad(&writer, "good.png", 0.0, 0.0, None, None)
        .unwrap();
    lane.clear(&writer);
    assert_eq!(bridge.counts().discarded, 2);
    assert_eq!(
        service.stats().cpu_pixels.reserved,
        before.cpu_pixels.reserved
    );

    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    service.close();
    let before_closed = service.stats();
    assert!(matches!(
        lane.quad(&writer, "broken.png", 0.0, 0.0, None, None),
        Err(LegacyError::Assets(_))
    ));
    lane.quad(&writer, "good.png", 0.0, 0.0, None, None)
        .unwrap();
    assert_eq!(
        bridge.counts().discarded,
        1,
        "service end routes a valid call through legacy discard semantics"
    );
    assert_eq!(
        service.stats().cpu_pixels.reserved,
        before_closed.cpu_pixels.reserved
    );
    drop(writer);
    service.detach_backend();
    let _ = service.finish_teardown();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn decoded_admission_rejects_headless_mode() {
    let service = ResourceService::new(ResourceLimits::DEFAULT, Mode::Headless).unwrap();
    assert!(matches!(
        service.create_decoded("good.png"),
        Err(LegacyAdmitError::Mode(Mode::Headless))
    ));
}

/// `admit` needs no writer and `install` is the short second half: `quad` is exactly the two, an admission dropped instead
/// of installed releases its texture, and a bridge that ends between the halves discards and counts the write.
#[test]
fn admit_and_install_are_the_two_halves_of_quad() {
    let (root, _) = assets();
    let service = backend(
        Some(Assets::new(&root)),
        ServiceWakes::default(),
        ResourceLimits::DEFAULT,
    );
    let lane = LegacyLane::new(service.clone(), Some(Assets::new(&root)));
    let bridge = SceneBridge::new(Filter::Nearest);
    let sink = LatestSink::new();
    bridge
        .attach(
            &desc(100, 80),
            pixels(100, 80),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .unwrap();
    let mut writer = bridge.take_writer().unwrap();

    // The two halves install what `quad` installs, with no writer needed to admit.
    let admission = lane
        .admit(false, "good.png", 3.0, 4.0, None, None)
        .expect("admitted without a writer");
    assert_eq!(service.stats().registry.live, 1, "the texture is retained");
    lane.install(&writer, admission).unwrap();
    let halves = frame(&mut writer, &sink);
    let compat = halves.compat().expect("the compatibility quad");
    let (x, y, width, height) = (compat.x, compat.y, compat.width, compat.height);
    lane.quad(&writer, "good.png", 3.0, 4.0, None, None)
        .unwrap();
    let whole = frame(&mut writer, &sink);
    let compat = whole.compat().expect("the compatibility quad");
    assert!(
        (x, y, width, height).eq(&(compat.x, compat.y, compat.width, compat.height)),
        "the halves and quad install the same rectangle"
    );
    drop((halves, whole));

    // Failures are admit's and change nothing: the old quad stays, no resource stays.
    let live = service.stats().registry.live;
    assert!(matches!(
        lane.admit(false, "broken.png", 0.0, 0.0, None, None),
        Err(LegacyError::Assets(_))
    ));
    assert!(matches!(
        lane.admit(false, "good.png", 0.0, 0.0, Some(0.0), None),
        Err(LegacyError::Geometry(_))
    ));
    assert_eq!(
        service.stats().registry.live,
        live,
        "a refused admit keeps nothing"
    );

    // An admission dropped instead of installed releases its texture.
    let admission = lane.admit(false, "good.png", 0.0, 0.0, None, None).unwrap();
    drop(admission);
    let _ = service.maintain(usize::MAX);
    assert_eq!(
        service.stats().registry.live,
        live,
        "the dropped admission released it"
    );

    // `ended`: the image is decoded and validated (the legacy errors), nothing is admitted, install counts the discard.
    let before = (service.stats().registry, bridge.counts().discarded);
    assert!(matches!(
        lane.admit(true, "broken.png", 0.0, 0.0, None, None),
        Err(LegacyError::Assets(_))
    ));
    assert!(matches!(
        lane.admit(true, "good.png", 0.0, 0.0, Some(f64::NAN), None),
        Err(LegacyError::Geometry(_))
    ));
    let admission = lane.admit(true, "good.png", 0.0, 0.0, None, None).unwrap();
    assert_eq!(
        service.stats().registry,
        before.0,
        "an ended admit admits nothing"
    );
    lane.install(&writer, admission).unwrap();
    assert_eq!(bridge.counts().discarded, before.1 + 1);

    // The bridge ends between the halves: install discards, counts and releases the texture.
    let admission = lane.admit(false, "good.png", 0.0, 0.0, None, None).unwrap();
    bridge.detach();
    let discarded = bridge.counts().discarded;
    lane.install(&writer, admission).unwrap();
    assert_eq!(bridge.counts().discarded, discarded + 1);
    let _ = service.maintain(usize::MAX);
    drop(writer);
    service.close();
    service.detach_backend();
    let _ = service.maintain(usize::MAX);
    let _ = service.finish_teardown();
    assert_eq!(service.stats().loaders.referenced_at_teardown, 0);
    fs::remove_dir_all(root).unwrap();
}

/// `validate_rect` is the writer's own rule, exported: the same numbers pass and fail in both.
#[test]
fn validate_rect_is_the_rule_the_writer_applies() {
    use crate::{QuadError, validate_rect};

    assert!(validate_rect(0.0, 0.0, 1.0, 1.0).is_ok());
    assert!(validate_rect(-5.0, 1e9, 64.0, 64.0).is_ok());
    assert!(matches!(
        validate_rect(f64::NAN, 0.0, 1.0, 1.0),
        Err(QuadError::NotFinite { field: "x" })
    ));
    assert!(matches!(
        validate_rect(0.0, f64::INFINITY, 1.0, 1.0),
        Err(QuadError::NotFinite { field: "y" })
    ));
    assert!(matches!(
        validate_rect(0.0, 0.0, 0.0, 1.0),
        Err(QuadError::BadSize { field: "width", .. })
    ));
    assert!(matches!(
        validate_rect(0.0, 0.0, 1.0, -2.0),
        Err(QuadError::BadSize {
            field: "height",
            ..
        })
    ));
    assert!(matches!(
        validate_rect(0.0, 0.0, 1.0, 1e12),
        Err(QuadError::BadSize {
            field: "height",
            ..
        })
    ));
    // The writer refuses exactly what the function refuses (set_position as a one-point rectangle).
    let bridge = SceneBridge::new(Filter::Nearest);
    let mut writer = bridge.take_writer().unwrap();
    let service = ResourceService::new(ResourceLimits::DEFAULT, Mode::Backend).unwrap();
    let key = service
        .create_from_pixels(pixels_for(&service))
        .unwrap_or_else(|_| panic!("admitted"));
    let texture = service.retain(key).unwrap();
    let quad = writer
        .create_quad(texture.clone(), 0.0, 0.0, 4.0, 4.0)
        .unwrap();
    for (x, y, w, h) in [
        (f64::NAN, 0.0, 1.0, 1.0),
        (0.0, 0.0, 0.0, 1.0),
        (0.0, 0.0, 1.0, 1e12),
    ] {
        let rule = validate_rect(x, y, w, h).unwrap_err();
        assert_eq!(
            writer.set_rect(quad, x, y, w, h).unwrap_err(),
            rule,
            "set_rect refuses with the exported rule's error"
        );
        assert_eq!(
            writer.create_quad(texture.clone(), x, y, w, h).unwrap_err(),
            rule
        );
    }
    drop((writer, texture));
    let _ = service.release(key);
    service.close();
    let _ = service.finish_teardown();
}

fn pixels_for(service: &ResourceService) -> CpuPixels {
    CpuPixels::premultiplied(service.reserve_pixels(2, 2).unwrap(), vec![255; 16]).unwrap()
}
