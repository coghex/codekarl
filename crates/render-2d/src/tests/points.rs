//! Points to pixels, and the quad's own validation.

use std::sync::Arc;

use ck_window::MAX_DIMENSION;

use super::support::{desc, image, pixels, quad_at};
use crate::{Metrics, PixelRect, PlacedQuad, Quad, QuadError, SourceRect};

fn metrics(points: (u32, u32), pixel: (u32, u32)) -> Metrics {
    Metrics::new(&desc(points.0, points.1), pixels(pixel.0, pixel.1))
}

fn place(quad: &Quad, points: (u32, u32), pixel: (u32, u32)) -> Option<PlacedQuad> {
    quad.place(&metrics(points, pixel))
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> PixelRect {
    PixelRect {
        left,
        top,
        right,
        bottom,
    }
}

fn source(left: f32, top: f32, right: f32, bottom: f32) -> SourceRect {
    SourceRect {
        left,
        top,
        right,
        bottom,
    }
}

#[test]
fn at_scale_1_the_destination_is_the_rectangle_and_the_source_is_the_whole_image() {
    let picture = image(30, 40);
    let placed = place(
        &quad_at(&picture, 10.0, 20.0, 30.0, 40.0),
        (100, 100),
        (100, 100),
    )
    .unwrap();
    assert_eq!(placed.destination, rect(10, 20, 40, 60));
    assert_eq!(placed.source, source(0.0, 0.0, 30.0, 40.0));
    assert!(Arc::ptr_eq(&placed.image, &picture));
}

#[test]
fn at_scale_2_every_edge_doubles_and_the_source_is_unchanged() {
    let picture = image(30, 40);
    let placed = place(
        &quad_at(&picture, 10.0, 20.0, 30.0, 40.0),
        (100, 100),
        (200, 200),
    )
    .unwrap();
    assert_eq!(placed.destination, rect(20, 40, 80, 120));
    assert_eq!(placed.source, source(0.0, 0.0, 30.0, 40.0));
}

#[test]
fn at_a_fractional_scale_each_edge_is_the_rounded_product_and_halves_round_away_from_zero() {
    let picture = image(10, 10);
    // 1.5 × 1.0 = 1.5 → 2; 1.5 × 4.0 = 6.0 → 6 (so the width is 4 pixels, not round(1.5 × 3.0) = 5 from the left).
    let placed = place(
        &quad_at(&picture, 1.0, 0.0, 3.0, 2.0),
        (100, 100),
        (150, 150),
    )
    .unwrap();
    assert_eq!(placed.destination, rect(2, 0, 6, 3));
    // Edges 0.3 × 1.5 = 0.45 → 0 and 0.3 + 3.0 = 3.3 × 1.5 = 4.95 → 5.
    let placed = place(
        &quad_at(&picture, 0.3, 0.3, 3.0, 3.0),
        (100, 100),
        (150, 150),
    )
    .unwrap();
    assert_eq!(placed.destination, rect(0, 0, 5, 5));
}

#[test]
fn the_two_axes_have_their_own_scales() {
    let picture = image(10, 10);
    // A 100 × 50 point window drawn on 300 × 50 pixels: 3 horizontally, 1 vertically.
    let placed = place(
        &quad_at(&picture, 10.0, 10.0, 10.0, 10.0),
        (100, 50),
        (300, 50),
    )
    .unwrap();
    assert_eq!(placed.destination, rect(30, 10, 60, 20));
    // And 1.5 horizontally, 2.5 vertically.
    let placed = place(
        &quad_at(&picture, 10.0, 10.0, 10.0, 10.0),
        (100, 40),
        (150, 100),
    )
    .unwrap();
    assert_eq!(placed.destination, rect(15, 25, 30, 50));
}

#[test]
fn the_size_defaults_to_the_images_pixel_size_read_as_points() {
    let picture = image(8, 4);
    let quad = Quad::new(Arc::clone(&picture), 5.0, 6.0, None, None).unwrap();
    assert_eq!((quad.width(), quad.height()), (8.0, 4.0));
    assert_eq!(
        place(&quad, (100, 100), (200, 200)).unwrap().destination,
        rect(10, 12, 26, 20)
    );
    // Each size defaults on its own.
    let quad = Quad::new(picture, 0.0, 0.0, Some(20.0), None).unwrap();
    assert_eq!((quad.width(), quad.height()), (20.0, 4.0));
}

#[test]
fn clipping_trims_the_source_in_proportion_on_every_side() {
    let picture = image(40, 40);
    // The window is 100 × 100 points at scale 1. A 40-point quad.
    let left = place(
        &quad_at(&picture, -10.0, 10.0, 40.0, 40.0),
        (100, 100),
        (100, 100),
    )
    .unwrap();
    assert_eq!(left.destination, rect(0, 10, 30, 50));
    assert_eq!(left.source, source(10.0, 0.0, 40.0, 40.0));
    let right = place(
        &quad_at(&picture, 80.0, 10.0, 40.0, 40.0),
        (100, 100),
        (100, 100),
    )
    .unwrap();
    assert_eq!(right.destination, rect(80, 10, 100, 50));
    assert_eq!(right.source, source(0.0, 0.0, 20.0, 40.0));
    let top = place(
        &quad_at(&picture, 10.0, -30.0, 40.0, 40.0),
        (100, 100),
        (100, 100),
    )
    .unwrap();
    assert_eq!(top.destination, rect(10, 0, 50, 10));
    assert_eq!(top.source, source(0.0, 30.0, 40.0, 40.0));
    let bottom = place(
        &quad_at(&picture, 10.0, 90.0, 40.0, 40.0),
        (100, 100),
        (100, 100),
    )
    .unwrap();
    assert_eq!(bottom.destination, rect(10, 90, 50, 100));
    assert_eq!(bottom.source, source(0.0, 0.0, 40.0, 10.0));
    // All four at once: a 200-point quad covers the window and shows its middle.
    let all = place(
        &quad_at(&picture, -50.0, -50.0, 200.0, 200.0),
        (100, 100),
        (100, 100),
    )
    .unwrap();
    assert_eq!(all.destination, rect(0, 0, 100, 100));
    assert_eq!(all.source, source(10.0, 10.0, 30.0, 30.0));
}

#[test]
fn the_source_is_trimmed_by_the_proportion_even_when_the_image_is_not_the_quads_size_or_the_scale_is_not_1()
 {
    // A 20-pixel image stretched over 40 points, at scale 2 (80 pixels), clipped by 20 pixels on the left.
    let picture = image(20, 10);
    let placed = place(
        &quad_at(&picture, -10.0, 0.0, 40.0, 10.0),
        (100, 100),
        (200, 200),
    )
    .unwrap();
    assert_eq!(placed.destination, rect(0, 0, 60, 20));
    assert_eq!(placed.source, source(5.0, 0.0, 20.0, 10.0));
}

#[test]
fn a_quad_entirely_outside_the_window_is_no_quad() {
    let picture = image(40, 40);
    let outside =
        |x: f64, y: f64| place(&quad_at(&picture, x, y, 40.0, 40.0), (100, 100), (100, 100));
    assert!(outside(100.0, 10.0).is_none(), "touching the right edge");
    assert!(outside(200.0, 10.0).is_none());
    assert!(outside(-40.0, 10.0).is_none(), "touching the left edge");
    assert!(outside(-100.0, 10.0).is_none());
    assert!(outside(10.0, 100.0).is_none(), "touching the bottom edge");
    assert!(outside(10.0, -40.0).is_none(), "touching the top edge");
    assert!(outside(-60.0, -60.0).is_none(), "beyond a corner");
    assert!(outside(99.0, 99.0).is_some(), "one point inside");
}

#[test]
fn a_zero_pixel_extent_is_no_quad() {
    let picture = image(10, 10);
    // 0.2 points at scale 1 is 10.0 to 10.2: both edges round to 10.
    assert!(
        place(
            &quad_at(&picture, 10.0, 10.0, 0.2, 10.0),
            (100, 100),
            (100, 100)
        )
        .is_none()
    );
    assert!(
        place(
            &quad_at(&picture, 10.0, 10.0, 10.0, 0.2),
            (100, 100),
            (100, 100)
        )
        .is_none()
    );
    // A drawable with no pixels.
    assert!(place(&quad_at(&picture, 0.0, 0.0, 10.0, 10.0), (100, 100), (0, 0)).is_none());
    // The same quad at scale 2 has an extent.
    assert!(
        place(
            &quad_at(&picture, 10.0, 10.0, 0.4, 10.0),
            (100, 100),
            (200, 200)
        )
        .is_some()
    );
}

#[test]
fn very_large_coordinates_are_clamped_and_the_result_is_always_representable() {
    let picture = image(100, 100);
    let window = (100, 100);
    for coordinate in [
        1e300,
        -1e300,
        f64::MAX,
        f64::MIN,
        1e15,
        -1e15,
        3e9,
        -3e9,
        16_777_216.0,
        -16_777_216.0,
    ] {
        for size in [1.0, 100.0, f64::from(MAX_DIMENSION)] {
            let quad = quad_at(&picture, coordinate, coordinate, size, size);
            // Anywhere a window of at most MAX_DIMENSION points cannot reach is outside, exactly.
            assert!(
                place(&quad, window, (100, 100)).is_none(),
                "{coordinate} × {size}"
            );
            assert!(
                place(&quad, window, (u32::MAX, u32::MAX)).is_none(),
                "{coordinate} × {size}"
            );
        }
    }
    // A big scale makes large edges: they still fit i32, and the source stays finite and inside the image.
    let quad = quad_at(
        &picture,
        -5_000_000.0,
        -5_000_000.0,
        f64::from(MAX_DIMENSION),
        f64::from(MAX_DIMENSION),
    );
    assert!(
        place(&quad, (1, 1), (u32::MAX, u32::MAX)).is_none(),
        "5 million points left of a 1-point window"
    );
    let quad = quad_at(
        &picture,
        0.0,
        0.0,
        f64::from(MAX_DIMENSION),
        f64::from(MAX_DIMENSION),
    );
    let placed = place(&quad, (1, 1), (u32::MAX, u32::MAX)).unwrap();
    // The edges are clamped to ±2^30 pixels before they become i32, inside the window of i32::MAX pixels.
    assert_eq!(placed.destination, rect(0, 0, 1 << 30, 1 << 30));
    for value in [
        placed.source.left,
        placed.source.top,
        placed.source.right,
        placed.source.bottom,
    ] {
        assert!(
            value.is_finite() && (0.0..=100.0).contains(&value),
            "{value}"
        );
    }
}

#[test]
fn every_placement_is_ordered_inside_the_window_and_inside_the_image() {
    let picture = image(37, 23);
    let coordinates = [
        -1e6, -300.0, -50.5, -1.0, -0.4, 0.0, 0.4, 1.0, 33.3, 99.5, 100.0, 150.0, 1e6,
    ];
    let sizes = [0.3, 1.0, 17.5, 100.0, 4000.0, f64::from(MAX_DIMENSION)];
    for (points, pixel) in [
        ((100, 80), (100, 80)),
        ((100, 80), (200, 160)),
        ((100, 80), (150, 40)),
        ((7, 3), (1920, 1080)),
    ] {
        for &x in &coordinates {
            for &y in &coordinates {
                for &width in &sizes {
                    for &height in &sizes {
                        let Some(placed) =
                            place(&quad_at(&picture, x, y, width, height), points, pixel)
                        else {
                            continue;
                        };
                        let (d, s) = (placed.destination, placed.source);
                        let context = format!("{x} {y} {width} {height} {points:?} {pixel:?}");
                        assert!(0 <= d.left && d.left < d.right, "{context}");
                        assert!(0 <= d.top && d.top < d.bottom, "{context}");
                        assert!(i64::from(d.right) <= i64::from(pixel.0), "{context}");
                        assert!(i64::from(d.bottom) <= i64::from(pixel.1), "{context}");
                        assert!(
                            0.0 <= s.left && s.left < s.right && s.right <= 37.0,
                            "{context}: {s:?}"
                        );
                        assert!(
                            0.0 <= s.top && s.top < s.bottom && s.bottom <= 23.0,
                            "{context}: {s:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn a_quad_refuses_numbers_that_are_not_finite_and_sizes_out_of_range() {
    let picture = image(4, 4);
    let new = |x: f64, y: f64, width: Option<f64>, height: Option<f64>| {
        Quad::new(Arc::clone(&picture), x, y, width, height)
    };
    assert!(new(0.0, 0.0, Some(1.0), Some(1.0)).is_ok());
    assert!(
        new(
            0.0,
            0.0,
            Some(f64::from(MAX_DIMENSION)),
            Some(f64::from(MAX_DIMENSION))
        )
        .is_ok()
    );
    assert_eq!(
        new(f64::NAN, 0.0, None, None).unwrap_err(),
        QuadError::NotFinite { field: "x" }
    );
    assert_eq!(
        new(0.0, f64::INFINITY, None, None).unwrap_err(),
        QuadError::NotFinite { field: "y" }
    );
    assert_eq!(
        new(0.0, 0.0, Some(f64::NAN), None).unwrap_err(),
        QuadError::NotFinite { field: "width" }
    );
    assert_eq!(
        new(0.0, 0.0, None, Some(f64::NEG_INFINITY)).unwrap_err(),
        QuadError::NotFinite { field: "height" }
    );
    for bad in [0.0, -1.0, f64::from(MAX_DIMENSION) + 1.0, 1e300] {
        assert_eq!(
            new(0.0, 0.0, Some(bad), None).unwrap_err(),
            QuadError::BadSize {
                field: "width",
                value: bad
            }
        );
        assert_eq!(
            new(0.0, 0.0, None, Some(bad)).unwrap_err(),
            QuadError::BadSize {
                field: "height",
                value: bad
            }
        );
    }
    let message = new(0.0, 0.0, Some(0.0), None).unwrap_err().to_string();
    assert!(
        message.contains("width") && message.contains("16384"),
        "{message}"
    );
}

#[test]
fn a_half_pixel_edge_rounds_away_from_zero_and_not_to_even() {
    let picture = image(10, 10);
    // 3.0 points at scale 1.5 is 4.5 pixels: away from zero is 5 (to even would be 4).
    let placed = place(
        &quad_at(&picture, 3.0, 0.0, 3.0, 2.0),
        (100, 100),
        (150, 150),
    )
    .unwrap();
    assert_eq!((placed.destination.left, placed.destination.right), (5, 9));
    // Left of the window: -3.0 is -4.5 pixels, which rounds to -5 (to even would be -4), and the far edge,
    // 7.0 points = 10.5 pixels, to 11 (to even: 10). Clipped at 0, the source starts 5/16 of the way in.
    let placed = place(
        &quad_at(&picture, -3.0, 0.0, 10.0, 10.0),
        (100, 100),
        (150, 150),
    )
    .unwrap();
    assert_eq!((placed.destination.left, placed.destination.right), (0, 11));
    assert_eq!(placed.source, source(3.125, 0.0, 10.0, 10.0));
}

/// Submission review 1's geometry: an 8192 x 1 image on a 1 x 1 point window whose drawable is 16384 x 1
/// pixels, the quad 16384 points wide and one pixel short of leaving the window on the left. The visible
/// destination is one pixel; its source span, 8191.999969482422..8192, is two distinct `f64` values that round
/// to one `f32`. No GPU type can sample an empty span.
fn extreme_quad() -> (Arc<crate::Image>, Quad) {
    let picture = image(8192, 1);
    let quad = quad_at(&picture, -16384.0 + 1.0 / 16384.0, 0.0, 16384.0, 1.0);
    (picture, quad)
}

#[test]
fn a_quad_whose_source_span_is_empty_as_f32_is_not_placed() {
    let (_picture, quad) = extreme_quad();
    assert!(
        place(&quad, (1, 1), (16384, 1)).is_none(),
        "a source span of one f32 value is not drawable"
    );
}

#[test]
fn the_extreme_quad_is_a_requested_scene_that_is_not_placed_through_the_real_bridge() {
    use super::support::RecordingSink;
    use crate::{Filter, QuadSink, SceneBridge};

    let (_picture, quad) = extreme_quad();
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    writer.set_quad(quad);
    let sink = RecordingSink::new();
    bridge
        .attach(
            &desc(1, 1),
            pixels(16384, 1),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .unwrap();
    let snapshots = sink.snapshots();
    let attached = snapshots.last().expect("the attach published");
    assert!(attached.requested, "the scene holds a quad");
    assert!(attached.quad.is_none(), "but nothing of it can be drawn");
}

#[test]
fn every_placed_quad_is_something_a_gpu_can_draw() {
    // A sweep over scales and offsets near the edges: whatever is placed has finite, ordered, in-image edges.
    let picture = image(64, 48);
    for pixel_width in [1_u32, 3, 64, 100, 1000, 16384] {
        for offset in [-1.0e6, -63.9999, -1.0, 0.0, 0.5, 99.99, 1.0e6] {
            for extent in [0.001, 1.0, 63.0, 64.0, 16000.0] {
                let quad = quad_at(&picture, offset, offset / 2.0, extent, extent);
                let Some(placed) = place(&quad, (100, 100), (pixel_width, 100)) else {
                    continue;
                };
                let source = placed.source;
                assert!(source.left.is_finite() && source.top.is_finite());
                assert!(source.right.is_finite() && source.bottom.is_finite());
                assert!(source.left >= 0.0 && source.top >= 0.0);
                assert!(source.right <= 64.0 && source.bottom <= 48.0);
                assert!(source.right > source.left && source.bottom > source.top);
                let destination = placed.destination;
                assert!(destination.right > destination.left);
                assert!(destination.bottom > destination.top);
            }
        }
    }
}
