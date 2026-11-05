//! The map lane (milestone 6, slice 1), windowless: the input's and the camera's validation, where the enlarged map lands, and the
//! bridge's publication rules (a stale input never overwrites a newer one, a camera is for one metrics generation only, nothing is
//! published to a detached bridge, and with no map input the frame is the frame without one).

use std::sync::Arc;

use ck_window::PixelSize;

use super::support::{RecordingSink, desc, pixels};
use crate::map::{MapCamera, MapError, MapInput, MapPublication, SIDE_MAX, place_map};
use crate::{Filter, Metrics, PixelRect, QuadSink, SceneBridge, SourceRect};

/// A grid of `width x height` cells, every one `cell`.
fn grid(width: u32, height: u32, cell: [u8; 4]) -> Vec<u8> {
    cell.repeat(usize::try_from(width * height).unwrap())
}

fn palette(count: usize) -> Vec<[u8; 4]> {
    (0..count)
        .map(|index| [u8::try_from(index % 256).unwrap(), 0, 0, 255])
        .collect()
}

/// A valid input of `width x height` cells: terrain 1 everywhere, elevation zero.
fn input(generation: u64, width: u32, height: u32) -> Arc<MapInput> {
    Arc::new(
        MapInput::new(
            generation,
            [100, 200],
            [width, height],
            grid(width, height, [1, 1, 0, 255]),
            grid(width, height, [42, 24, 0, 255]),
            palette(3),
        )
        .expect("a valid input"),
    )
}

fn camera(view: [u32; 2], metrics_generation: u64) -> Arc<MapCamera> {
    Arc::new(MapCamera::new([5, -6], view, Vec::new(), metrics_generation).expect("a valid camera"))
}

fn metrics(points: (u32, u32), pixel_size: PixelSize) -> Metrics {
    Metrics {
        points_width: points.0,
        points_height: points.1,
        pixels: pixel_size,
    }
}

fn attached() -> (Arc<SceneBridge>, Arc<RecordingSink>) {
    let bridge = SceneBridge::new(Filter::Nearest);
    let sink = RecordingSink::new();
    bridge
        .attach(
            &desc(100, 60),
            pixels(100, 60),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .expect("attached");
    (bridge, sink)
}

#[test]
fn an_input_is_checked_against_the_packed_layout_and_names_the_first_problem() {
    let ok = |a: Vec<u8>, b: Vec<u8>, palette: Vec<[u8; 4]>| {
        MapInput::new(1, [0, 0], [2, 1], a, b, palette)
    };
    let a = grid(2, 1, [1, 1, 0, 255]);
    let b = grid(2, 1, [0, 0, 0, 255]);
    assert!(ok(a.clone(), b.clone(), palette(3)).is_ok());

    // The sizes.
    let size = |width, height| {
        MapInput::new(
            1,
            [0, 0],
            [width, height],
            Vec::new(),
            Vec::new(),
            palette(1),
        )
    };
    for (width, height) in [(0, 1), (1, 0), (SIDE_MAX + 1, 1), (1, SIDE_MAX + 1)] {
        assert_eq!(
            size(width, height).expect_err("refused"),
            MapError::Size {
                what: "crop",
                width,
                height
            }
        );
    }
    // The palette.
    assert_eq!(
        ok(a.clone(), b.clone(), Vec::new()).expect_err("refused"),
        MapError::Palette { count: 0 }
    );
    assert_eq!(
        ok(a.clone(), b.clone(), palette(257)).expect_err("refused"),
        MapError::Palette { count: 257 }
    );
    assert!(ok(a.clone(), b.clone(), palette(256)).is_ok());
    // The lengths.
    assert_eq!(
        ok(vec![0; 4], b.clone(), palette(3)).expect_err("refused"),
        MapError::GridLength {
            grid: 'A',
            expected: 8,
            actual: 4
        }
    );
    assert_eq!(
        ok(a.clone(), vec![0; 9], palette(3)).expect_err("refused"),
        MapError::GridLength {
            grid: 'B',
            expected: 8,
            actual: 9
        }
    );
    // Grid A: alpha, the reserved bits of G, B, a terrain index the palette lacks (cell (1, 0) is the second one).
    for (cell, problem) in [
        ([1, 1, 0, 254], "alpha is not 255"),
        ([1, 3, 0, 255], "a bit of G other than bit 0 is set"),
        ([1, 1, 7, 255], "B is not zero"),
    ] {
        let mut bad = a.clone();
        bad[4..8].copy_from_slice(&cell);
        assert_eq!(
            ok(bad, b.clone(), palette(3)).expect_err("refused"),
            MapError::GridCell {
                grid: 'A',
                x: 1,
                y: 0,
                problem
            }
        );
    }
    let mut bad = a.clone();
    bad[0] = 3;
    assert_eq!(
        ok(bad, b.clone(), palette(3)).expect_err("refused"),
        MapError::TerrainIndex {
            x: 0,
            y: 0,
            index: 3,
            palette: 3
        }
    );
    // Grid B: alpha and B (its G carries the low byte of the elevation, so every value is allowed).
    for (cell, problem) in [
        ([0, 0, 0, 0], "alpha is not 255"),
        ([0, 0, 1, 255], "B is not zero"),
    ] {
        let mut bad = b.clone();
        bad[0..4].copy_from_slice(&cell);
        assert_eq!(
            ok(a.clone(), bad, palette(3)).expect_err("refused"),
            MapError::GridCell {
                grid: 'B',
                x: 0,
                y: 0,
                problem
            }
        );
    }
    let mut free = b;
    free[0..4].copy_from_slice(&[255, 255, 0, 255]);
    assert!(ok(a, free, palette(3)).is_ok());
}

#[test]
fn a_camera_is_checked_and_its_rivers_are_bounded_by_the_view() {
    assert!(MapCamera::new([0, 0], [3, 2], vec![[2, 1]], 1).is_ok());
    assert_eq!(
        MapCamera::new([0, 0], [0, 2], Vec::new(), 1).expect_err("refused"),
        MapError::Size {
            what: "view",
            width: 0,
            height: 2
        }
    );
    assert_eq!(
        MapCamera::new([0, 0], [3, 2], vec![[0, 0]; 7], 1).expect_err("refused"),
        MapError::TooManyRivers { limit: 6, count: 7 }
    );
    assert_eq!(
        MapCamera::new([0, 0], [3, 2], vec![[0, 0], [3, 0]], 1).expect_err("refused"),
        MapError::RiverOutsideView {
            position: 1,
            x: 3,
            y: 0
        }
    );
    let held = MapCamera::new([-7, 9], [3, 2], Vec::new(), 4).unwrap();
    assert_eq!(
        (held.top_left(), held.view(), held.metrics_generation()),
        ([-7, 9], [3, 2], 4)
    );
}

#[test]
fn the_enlarged_map_is_view_times_four_points_through_the_points_to_pixels_rule() {
    // Scale 1: a pixel per point, four per map pixel.
    let at_one = place_map([3, 2], &metrics((100, 60), pixels(100, 60))).expect("placed");
    assert_eq!(
        at_one.destination,
        PixelRect {
            left: 0,
            top: 0,
            right: 12,
            bottom: 8
        }
    );
    assert_eq!(
        at_one.source,
        SourceRect {
            left: 0.0,
            top: 0.0,
            right: 3.0,
            bottom: 2.0
        }
    );
    // Scale 2: eight drawable pixels per map pixel.
    let at_two = place_map([3, 2], &metrics((100, 60), pixels(200, 120))).expect("placed");
    assert_eq!(
        at_two.destination,
        PixelRect {
            left: 0,
            top: 0,
            right: 24,
            bottom: 16
        }
    );
    assert_eq!((at_two.source.right, at_two.source.bottom), (3.0, 2.0));
    // A window of 10 x 6 points shows two and a half map pixels across and one and a half down: the partial pixel is cropped, not
    // stretched, so the destination is clipped and the source trimmed in proportion.
    let cropped = place_map([3, 2], &metrics((10, 6), pixels(10, 6))).expect("placed");
    assert_eq!(
        cropped.destination,
        PixelRect {
            left: 0,
            top: 0,
            right: 10,
            bottom: 6
        }
    );
    assert_eq!((cropped.source.right, cropped.source.bottom), (2.5, 1.5));
    // A fractional scale rounds each edge from its position in points.
    let fractional = place_map([3, 2], &metrics((100, 60), pixels(150, 90))).expect("placed");
    assert_eq!(
        fractional.destination,
        PixelRect {
            left: 0,
            top: 0,
            right: 18,
            bottom: 12
        }
    );
    // A drawable with no pixels shows nothing.
    assert!(place_map([3, 2], &metrics((100, 60), pixels(0, 0))).is_none());
}

#[test]
fn with_no_map_input_the_snapshots_carry_no_map_even_when_a_camera_was_published() {
    let (bridge, sink) = attached();
    assert_eq!(bridge.metrics_generation(), 1);
    assert_eq!(
        bridge.publish_camera(camera([3, 2], 1)),
        MapPublication::Published
    );
    bridge.set_pixel_size(pixels(100, 60));
    for snapshot in sink.snapshots() {
        assert!(snapshot.map.is_none(), "generation {}", snapshot.generation);
    }
}

#[test]
fn an_input_and_a_camera_for_the_current_metrics_make_a_map_with_its_placement() {
    let (bridge, sink) = attached();
    let loaded = input(1, 7, 5);
    assert_eq!(
        bridge.publish_map(Arc::clone(&loaded)),
        MapPublication::Published
    );
    // The input alone is not a map: a camera is needed to place it.
    assert!(sink.last().expect("published").map.is_none());
    let view = camera([5, 3], bridge.metrics_generation());
    assert_eq!(
        bridge.publish_camera(Arc::clone(&view)),
        MapPublication::Published
    );
    let snapshot = sink.last().expect("published");
    let map = snapshot.map.expect("a map");
    assert!(Arc::ptr_eq(&map.input, &loaded));
    assert!(Arc::ptr_eq(&map.camera, &view));
    let placement = map.placement.expect("placed");
    assert_eq!(placement.destination.right, 20);
    assert_eq!(placement.destination.bottom, 12);
    // The generations only grow, in publication order.
    let generations = sink.generations();
    assert!(generations.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn a_stale_input_never_overwrites_a_newer_one_and_nothing_is_published_for_it() {
    let (bridge, sink) = attached();
    assert_eq!(
        bridge.publish_map(input(5, 4, 4)),
        MapPublication::Published
    );
    bridge.publish_camera(camera([2, 2], 1));
    let before = sink.count();
    for generation in [5, 4, 0] {
        let stale = input(generation, 9, 9);
        assert_eq!(
            bridge.publish_map(Arc::clone(&stale)),
            MapPublication::Stale,
            "generation {generation} is not newer than 5"
        );
        // The refused input was dropped by the call: only the caller holds it.
        assert_eq!(Arc::strong_count(&stale), 1);
    }
    assert_eq!(sink.count(), before, "a refusal publishes nothing");
    let map = sink.last().expect("published").map.expect("a map");
    assert_eq!(map.input.generation(), 5);
    assert_eq!(map.input.size(), [4, 4]);
    // A newer one replaces it.
    assert_eq!(
        bridge.publish_map(input(6, 8, 8)),
        MapPublication::Published
    );
    let map = sink.last().expect("published").map.expect("a map");
    assert_eq!((map.input.generation(), map.input.size()), (6, [8, 8]));
}

#[test]
fn a_camera_is_for_one_metrics_generation_and_a_resize_retires_it() {
    let (bridge, sink) = attached();
    bridge.publish_map(input(1, 4, 4));
    let first = bridge.metrics_generation();
    assert_eq!(
        bridge.publish_camera(camera([2, 2], first)),
        MapPublication::Published
    );
    assert!(sink.last().expect("published").map.is_some());
    // A resize: the bridge has new metrics, and the snapshot made with them cannot pair them with the old view.
    bridge.set_pixel_size(pixels(200, 120));
    assert_eq!(bridge.metrics_generation(), first + 1);
    assert!(
        sink.last().expect("published").map.is_none(),
        "the old camera was not computed for the new metrics"
    );
    // A camera computed for the old metrics is refused; one for the new metrics is drawn.
    let before = sink.count();
    assert_eq!(
        bridge.publish_camera(camera([2, 2], first)),
        MapPublication::Stale
    );
    assert_eq!(sink.count(), before);
    assert_eq!(
        bridge.publish_camera(camera([3, 2], first + 1)),
        MapPublication::Published
    );
    let map = sink.last().expect("published").map.expect("a map");
    assert_eq!(map.camera.view(), [3, 2]);
    // The placement uses the snapshot's own metrics (scale 2 here).
    assert_eq!(map.placement.expect("placed").destination.right, 24);
}

#[test]
fn nothing_is_published_to_a_detached_bridge_and_the_sink_is_left_without_the_map() {
    let (bridge, sink) = attached();
    bridge.publish_map(input(1, 4, 4));
    bridge.publish_camera(camera([2, 2], bridge.metrics_generation()));
    assert!(sink.last().expect("published").map.is_some());
    bridge.detach();
    // The detach replaced the consumer's latest snapshot, so an idle sink does not pin the map.
    assert!(sink.last().expect("published").map.is_none());
    let after = sink.count();
    let late = input(9, 4, 4);
    assert_eq!(
        bridge.publish_map(Arc::clone(&late)),
        MapPublication::Detached
    );
    assert_eq!(
        bridge.publish_camera(camera([2, 2], bridge.metrics_generation())),
        MapPublication::Detached
    );
    assert_eq!(sink.count(), after, "a detached bridge publishes nothing");
    assert_eq!(Arc::strong_count(&late), 1, "and keeps nothing");
}

#[test]
fn an_input_published_before_the_sink_attaches_goes_out_at_attach_without_a_camera() {
    let bridge = SceneBridge::new(Filter::Nearest);
    assert_eq!(bridge.metrics_generation(), 0);
    assert_eq!(
        bridge.publish_map(input(1, 4, 4)),
        MapPublication::Published
    );
    // A stale rule applies before the attach too.
    assert_eq!(bridge.publish_map(input(1, 9, 9)), MapPublication::Stale);
    let sink = RecordingSink::new();
    bridge
        .attach(
            &desc(100, 60),
            pixels(100, 60),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .expect("attached");
    // The attach's snapshot has the metrics of generation 1 and no camera for them: no map yet.
    assert_eq!(sink.count(), 1);
    assert!(sink.last().expect("published").map.is_none());
    assert_eq!(
        bridge.publish_camera(camera([2, 2], 1)),
        MapPublication::Published
    );
    assert_eq!(
        sink.last()
            .expect("published")
            .map
            .expect("a map")
            .input
            .generation(),
        1
    );
}

#[test]
fn a_palette_colour_that_is_not_opaque_is_refused_naming_it() {
    let mut colours = palette(3);
    colours[2][3] = 200;
    assert_eq!(
        MapInput::new(
            1,
            [0, 0],
            [2, 1],
            grid(2, 1, [1, 0, 0, 255]),
            grid(2, 1, [0, 0, 0, 255]),
            colours
        )
        .expect_err("refused"),
        MapError::PaletteAlpha {
            index: 2,
            alpha: 200
        }
    );
}

#[test]
fn a_refusal_is_reported_once_per_episode_and_a_success_ends_the_episode() {
    let (bridge, sink) = attached();
    bridge.publish_map(input(1, 4, 4));
    bridge.publish_camera(camera([2, 2], bridge.metrics_generation()));
    let refusals = Arc::clone(&sink.last().expect("published").map.expect("a map").refusals);
    assert_eq!(bridge.take_map_refusal(), None, "nothing was refused");
    // The presenter asks again every frame: the first refusal of the episode is the one message.
    refusals.refused("the target could not be allocated");
    refusals.refused("again");
    refusals.refused("and again");
    assert_eq!(
        bridge.take_map_refusal().as_deref(),
        Some("the target could not be allocated")
    );
    assert_eq!(bridge.take_map_refusal(), None, "read once");
    refusals.refused("still refused");
    assert_eq!(bridge.take_map_refusal(), None, "the same episode");
    // A success ends the episode: the next refusal is a new message.
    refusals.cleared();
    refusals.refused("a new episode");
    assert_eq!(bridge.take_map_refusal().as_deref(), Some("a new episode"));
    // A success also drops a message nobody read.
    refusals.cleared();
    refusals.refused("unread");
    refusals.cleared();
    assert_eq!(bridge.take_map_refusal(), None);
}

/// A snapshot with a map, from a bridge with a sink attached, and that bridge: what a backend's conversion starts from.
fn snapshot_with_a_map() -> (Arc<SceneBridge>, crate::Snapshot) {
    let (bridge, sink) = attached();
    bridge.publish_map(input(1, 4, 4));
    bridge.publish_camera(camera([2, 2], bridge.metrics_generation()));
    let snapshot = sink.last().expect("published");
    assert!(snapshot.map.is_some());
    (bridge, snapshot)
}

#[cfg(all(feature = "metal", target_os = "macos"))]
#[test]
fn the_metal_conversion_gives_the_presenter_a_sink_that_reaches_the_publisher() {
    let (bridge, snapshot) = snapshot_with_a_map();
    let map = crate::metal::metal_map(&snapshot)
        .expect("converted")
        .expect("a map");
    let sink = map.refusals().expect("the conversion attached a sink");
    sink.refused("the target could not be allocated");
    sink.refused("again");
    assert_eq!(
        bridge.take_map_refusal().as_deref(),
        Some("the target could not be allocated"),
        "one message for the episode"
    );
    assert_eq!(bridge.take_map_refusal(), None);
    sink.refused("and again");
    assert_eq!(bridge.take_map_refusal(), None, "the same episode");
    sink.cleared();
    sink.refused("a new episode");
    assert_eq!(bridge.take_map_refusal().as_deref(), Some("a new episode"));
}

#[cfg(feature = "vulkan")]
#[test]
fn the_vulkan_conversion_gives_the_presenter_a_sink_that_reaches_the_publisher() {
    let (bridge, snapshot) = snapshot_with_a_map();
    let map = crate::vulkan::vulkan_map(&snapshot)
        .expect("converted")
        .expect("a map");
    let sink = map.refusals().expect("the conversion attached a sink");
    sink.refused("the target could not be allocated");
    sink.refused("again");
    assert_eq!(
        bridge.take_map_refusal().as_deref(),
        Some("the target could not be allocated"),
        "one message for the episode"
    );
    assert_eq!(bridge.take_map_refusal(), None);
    sink.refused("and again");
    assert_eq!(bridge.take_map_refusal(), None, "the same episode");
    sink.cleared();
    sink.refused("a new episode");
    assert_eq!(bridge.take_map_refusal().as_deref(), Some("a new episode"));
}
