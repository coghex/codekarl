//! The UI scale of a window opened in screen pixels (milestone 9, slice 12c): the cap from the drawable, the configured limit, the one
//! conversion of pointer positions, scroll amounts and hit tests, and the drawing of the UI scene at 1, 1.5 or 2 pixels a unit.

use std::sync::Arc;

use ck_resources::{
    CpuPixels, Mode, ResourceLimits, ResourceService, ResourceState, TextureKey, TextureRef,
};
use ck_window::{PixelRequest, PixelSize, PointerPosition, WindowDesc};

use super::support::{RecordingSink, desc, image, pixels};
use crate::ui_scale::{self, UnitMap, cap_k, k_of_scale, scale_k, view_units};
use crate::{Filter, Metrics, PointRect, Quad, QuadSink, SceneBridge};

fn size(width: u32, height: u32) -> PixelSize {
    PixelSize { width, height }
}

#[test]
fn the_cap_is_the_largest_scale_the_drawable_allows() {
    // k = 2 is the scale 1, 3 is 1.5 and 4 is 2.
    for (width, height, k) in [
        (1279, 800, 2),
        (1280, 800, 2),
        (1919, 1200, 2),
        (1920, 1199, 2),
        (1920, 1200, 3),
        (2559, 1600, 3),
        (2560, 1599, 3),
        (2560, 1600, 4),
        (3456, 2232, 4),
        (100, 100, 2),
    ] {
        assert_eq!(cap_k(size(width, height)), k, "{width}x{height}");
        assert_eq!(scale_k(size(width, height), None), k, "{width}x{height}");
    }
    // The configured scale limits it and never raises it.
    assert_eq!(scale_k(size(3456, 2232), Some(2)), 2, "[ui] scale = 1");
    assert_eq!(scale_k(size(3456, 2232), Some(3)), 3, "[ui] scale = 1.5");
    assert_eq!(
        scale_k(size(1280, 800), Some(4)),
        2,
        "[ui] scale = 2 at the least size"
    );
    assert_eq!(scale_k(size(1920, 1200), Some(4)), 3);
    assert_eq!(k_of_scale(1.0), Some(2));
    assert_eq!(k_of_scale(1.5), Some(3));
    assert_eq!(k_of_scale(2.0), Some(4));
    for bad in [0.0, 1.25, 3.0, -1.0, f64::NAN] {
        assert_eq!(k_of_scale(bad), None, "{bad}");
    }
}

#[test]
fn the_view_the_ui_sees_is_the_drawable_over_the_scale() {
    assert_eq!(view_units(size(1280, 800), 2), (1280, 800));
    assert_eq!(view_units(size(1920, 1200), 3), (1280, 800));
    assert_eq!(view_units(size(3456, 2232), 4), (1728, 1116));
    // Floored: 1282 pixels at scale 1.5 would be 854 units and a third.
    assert_eq!(view_units(size(1281, 801), 3), (854, 534));
}

fn fixed(units: f64) -> i32 {
    #[expect(clippy::cast_possible_truncation, reason = "test values are small")]
    let value = (units * 256.0) as i32;
    value
}

fn at(x: f64, y: f64) -> PointerPosition {
    PointerPosition {
        x: fixed(x),
        y: fixed(y),
    }
}

/// The metrics of a display at backing 2: the drawable and the points it is half of.
fn map(pixels: (u32, u32), k: u32) -> UnitMap {
    UnitMap {
        pixels: size(pixels.0, pixels.1),
        points: (pixels.0 / 2, pixels.1 / 2),
        k,
    }
}

#[test]
fn positions_are_converted_by_one_rule_and_keep_the_fixed_precision() {
    // Backing 2, scale 2 (3456x2232 pixels, 1728x1116 points): a unit is a point.
    assert_eq!(
        map((3456, 2232), 4).position(at(100.0, 50.0)),
        at(100.0, 50.0)
    );
    // Scale 1 (1280x800 pixels, 640x400 points): a unit is half a point, so positions double.
    let one = map((1280, 800), 2);
    assert_eq!(one.position(at(100.0, 50.0)), at(200.0, 100.0));
    assert_eq!(one.position(at(-0.5, 0.0)), at(-1.0, 0.0));
    // Scale 1.5 (1920x1200 pixels, 960x600 points): positions are 4/3 of the points, in 1/256 units, floored.
    let one_and_a_half = map((1920, 1200), 3);
    assert_eq!(
        one_and_a_half.position(at(100.0, 50.0)),
        PointerPosition { x: 34133, y: 17066 },
        "133.33203125 and 66.6640625"
    );
    assert_eq!(
        one_and_a_half.position(at(-1.0, 0.0)),
        PointerPosition { x: -342, y: 0 },
        "toward minus infinity: -1.3359375"
    );
}

#[test]
fn point_scroll_amounts_round_toward_zero_so_equal_scrolls_stay_equal() {
    let one_and_a_half = map((1920, 1200), 3);
    assert_eq!(
        one_and_a_half.scroll_amount(15 * 256),
        20 * 256,
        "15 points are 20 units"
    );
    assert_eq!(
        one_and_a_half.scroll_amount(-256),
        -341,
        "-1.33203125 units"
    );
    assert_eq!(one_and_a_half.scroll_amount(256), 341);
    // A saturating end: the result never wraps.
    assert_eq!(ui_scale::amount_units(i32::MAX, 3456, 1, 2), i32::MAX);
    assert_eq!(ui_scale::position_units(i32::MIN, 3456, 1, 2), i32::MIN);
}

#[test]
fn a_hit_region_in_units_contains_the_converted_position_and_is_half_open() {
    let bridge = SceneBridge::new(Filter::Nearest);
    let mut writer = bridge.take_writer().expect("the writer");
    // A region at units (130, 60) sized 4 by 7: x 130..134, y 60..67.
    writer
        .set_hit(
            "probe",
            Some(vec![PointRect::new(130.0, 60.0, 4.0, 7.0)]),
            None,
        )
        .expect("a valid hit");
    assert!(writer.publish());
    let regions = bridge.hit_regions().expect("regions");
    let one_and_a_half = map((1920, 1200), 3);
    // Point (100, 50) is unit (133.33, 66.66): inside. Point (100.5, 50) is unit 134.0 exactly: on the half-open right edge, outside.
    assert_eq!(
        regions.key_at(one_and_a_half.position(at(100.0, 50.0))),
        Some("probe")
    );
    assert_eq!(
        regions.key_at(one_and_a_half.position(at(100.5, 50.0))),
        None
    );
}

fn quad_texture(service: &ResourceService) -> TextureRef<ResourceState> {
    let reservation = service.reserve_pixels(1, 1).expect("a pixel reservation");
    let pixels = CpuPixels::premultiplied(reservation, vec![0; 4]).expect("RGBA8");
    let key: TextureKey = service
        .create_from_pixels(pixels)
        .unwrap_or_else(|_| panic!("texture admitted"));
    service.retain(key).expect("live")
}

/// The window of a pixels request of 1280x800 whose drawable is `pixels`.
fn pixels_window(width: u32, height: u32) -> WindowDesc {
    WindowDesc {
        pixel_request: Some(PixelRequest {
            size: (1280, 800),
            min: Some((1280, 800)),
        }),
        ..desc(width / 2, height / 2)
    }
}

/// Where a quad at unit (10, 4) sized 20 by 2 is drawn in a window opened in pixels with a drawable of `drawable`.
fn drawn_at(drawable: (u32, u32), limit: Option<u32>) -> (i32, i32, i32, i32) {
    let service = ResourceService::new(ResourceLimits::DEFAULT, Mode::Headless).expect("service");
    let bridge = SceneBridge::new(Filter::Nearest);
    bridge.set_ui_scale_limit(limit);
    let sink = RecordingSink::new();
    bridge
        .attach(
            &pixels_window(drawable.0, drawable.1),
            pixels(drawable.0, drawable.1),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .expect("attached");
    let mut writer = bridge.take_writer().expect("the writer");
    writer
        .create_quad(quad_texture(&service), 10.0, 4.0, 20.0, 2.0)
        .expect("a quad");
    assert!(writer.publish());
    let snapshot = sink.last().expect("a snapshot");
    let placed = snapshot.scene.expect("a scene").quads()[0]
        .place(&snapshot.metrics)
        .expect("the quad shows");
    let d = placed.destination;
    (d.left, d.top, d.right, d.bottom)
}

#[test]
fn a_ui_quad_is_drawn_at_the_scale_the_window_allows() {
    // The scale 1: 1280x800 pixels. 1.5: 1920x1200. 2: 2560x1600 and more.
    assert_eq!(drawn_at((1280, 800), None), (10, 4, 30, 6));
    assert_eq!(drawn_at((1920, 1200), None), (15, 6, 45, 9));
    assert_eq!(drawn_at((2560, 1600), None), (20, 8, 60, 12));
    assert_eq!(drawn_at((3456, 2232), None), (20, 8, 60, 12));
    // A configured limit lowers it.
    assert_eq!(drawn_at((3456, 2232), Some(2)), (10, 4, 30, 6));
    assert_eq!(drawn_at((3456, 2232), Some(3)), (15, 6, 45, 9));
}

#[test]
fn a_window_opened_in_points_keeps_the_points_to_pixels_ratio_whatever_the_limit() {
    let service = ResourceService::new(ResourceLimits::DEFAULT, Mode::Headless).expect("service");
    let bridge = SceneBridge::new(Filter::Nearest);
    bridge.set_ui_scale_limit(Some(4));
    let sink = RecordingSink::new();
    // 400x320 points at 2 pixels a point, no pixel request: the scene is in points, drawn at the ratio.
    bridge
        .attach(
            &desc(400, 320),
            pixels(800, 640),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .expect("attached");
    assert_eq!(bridge.ui_scale(pixels(3456, 2232)), None);
    let mut writer = bridge.take_writer().expect("the writer");
    writer
        .create_quad(quad_texture(&service), 10.0, 4.0, 20.0, 2.0)
        .expect("a quad");
    assert!(writer.publish());
    let snapshot = sink.last().expect("a snapshot");
    assert_eq!(snapshot.metrics.ui_scale_k, None);
    let d = snapshot.scene.expect("a scene").quads()[0]
        .place(&snapshot.metrics)
        .expect("shows")
        .destination;
    assert_eq!((d.left, d.top, d.right, d.bottom), (20, 8, 60, 12));
}

#[test]
fn a_resize_across_a_threshold_changes_the_scale_at_the_next_metrics() {
    let bridge = SceneBridge::new(Filter::Nearest);
    let sink = RecordingSink::new();
    bridge
        .attach(
            &pixels_window(1280, 800),
            pixels(1280, 800),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .expect("attached");
    assert_eq!(sink.last().expect("a snapshot").metrics.ui_scale_k, Some(2));
    bridge.set_metrics((960, 600), pixels(1920, 1200));
    assert_eq!(sink.last().expect("a snapshot").metrics.ui_scale_k, Some(3));
    bridge.set_metrics((1280, 800), pixels(2560, 1600));
    assert_eq!(sink.last().expect("a snapshot").metrics.ui_scale_k, Some(4));
    bridge.set_metrics((640, 400), pixels(1280, 800));
    assert_eq!(sink.last().expect("a snapshot").metrics.ui_scale_k, Some(2));
}

#[test]
fn the_compatibility_quad_is_placed_at_the_ui_scale_in_a_pixels_window_and_at_the_ratio_in_points()
{
    // The legacy `render.set_quad` lane is part of the UI scene: a quad at unit (10, 4) sized 20 by 2.
    let picture = image(1, 1);
    let quad = Quad::new(Arc::clone(&picture), 10.0, 4.0, Some(20.0), Some(2.0)).expect("a quad");
    let placed = |metrics: &Metrics| {
        let d = quad.place(metrics).expect("it shows").destination;
        (d.left, d.top, d.right, d.bottom)
    };
    for (drawable, k, want) in [
        ((1280, 800), 2, (10, 4, 30, 6)),
        ((1920, 1200), 3, (15, 6, 45, 9)),
        ((2560, 1600), 4, (20, 8, 60, 12)),
    ] {
        let metrics = Metrics {
            points_width: drawable.0 / 2,
            points_height: drawable.1 / 2,
            pixels: size(drawable.0, drawable.1),
            ui_scale_k: Some(k),
        };
        assert_eq!(placed(&metrics), want, "k = {k}");
    }
    // A window opened in points keeps the points-to-pixels ratio.
    let points = Metrics {
        points_width: 400,
        points_height: 320,
        pixels: size(800, 640),
        ui_scale_k: None,
    };
    assert_eq!(placed(&points), (20, 8, 60, 12));
}
