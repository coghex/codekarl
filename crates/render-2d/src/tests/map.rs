//! The map lane (milestone 6, slices 1 and 6), windowless: the input's and the camera's validation, where the enlarged map lands, and the
//! bridge's publication rules (a stale input never overwrites a newer one, a camera is for one metrics generation only, nothing is
//! published to a detached bridge, and with no map input the frame is the frame without one).

use std::sync::Arc;

use ck_window::PixelSize;

use super::support::{RecordingSink, desc, pixels};
use crate::map::{
    MapCamera, MapError, MapInput, MapPublication, MapRules, MapTerrainKind, MapTerrainRule,
    SIDE_MAX, STEP_COUNT, TERRAIN_MAX, place_map,
};
use crate::{Filter, Metrics, PixelRect, QuadSink, SceneBridge, SourceRect};

/// A grid of `width x height` cells, every one `cell`.
pub(super) fn grid(width: u32, height: u32, cell: [u8; 4]) -> Vec<u8> {
    cell.repeat(usize::try_from(width * height).unwrap())
}

/// One land type's rules: valid in every field.
fn land() -> MapTerrainRule {
    MapTerrainRule {
        kind: MapTerrainKind::Land,
        tone: [
            [7, 8, 9, 255],
            [4, 5, 6, 255],
            [3, 4, 5, 255],
            [1, 2, 3, 255],
        ],
        shadow: [[10, 11, 12, 255], [13, 14, 15, 255], [16, 17, 18, 255]],
        lit: [[19, 20, 21, 255], [22, 23, 24, 255], [25, 26, 27, 255]],
    }
}

/// A valid rule table of `count` land types (the step table is a plain descending ramp, not `ck-map`'s).
pub(super) fn rules(count: usize) -> MapRules {
    MapRules {
        terrains: vec![land(); count],
        sea: [5, 8, 14, 255],
        shallow_sea: [11, 20, 34, 255],
        lake: [11, 20, 34, 255],
        river: [58, 120, 194, 255],
        ice: [232, 233, 237, 255],
        ice_second: [185, 188, 198, 255],
        overlay_palette: Vec::new(),
        shallow_threshold: 40,
        steps: std::array::from_fn(|step| 196_608 - 2_000 * i32::try_from(step).unwrap()),
    }
}

/// A valid input of `width x height` cells: terrain 1 everywhere, elevation zero.
pub(super) fn input(generation: u64, width: u32, height: u32) -> Arc<MapInput> {
    Arc::new(
        MapInput::new(
            generation,
            [100, 200],
            [width, height],
            grid(width, height, [1, 1, 128, 255]),
            grid(width, height, [42, 24, 0, 255]),
            rules(3),
        )
        .expect("a valid input"),
    )
}

pub(super) fn camera(view: [u32; 2], metrics_generation: u64) -> Arc<MapCamera> {
    Arc::new(
        MapCamera::new(40, [5, -6], view, Vec::new(), metrics_generation).expect("a valid camera"),
    )
}

fn metrics(points: (u32, u32), pixel_size: PixelSize) -> Metrics {
    Metrics {
        points_width: points.0,
        points_height: points.1,
        pixels: pixel_size,
    }
}

pub(super) fn attached() -> (Arc<SceneBridge>, Arc<RecordingSink>) {
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
    let ok =
        |a: Vec<u8>, b: Vec<u8>, rules: MapRules| MapInput::new(1, [0, 0], [2, 1], a, b, rules);
    let a = grid(2, 1, [1, 1, 128, 255]);
    let b = grid(2, 1, [0, 0, 0, 255]);
    assert!(ok(a.clone(), b.clone(), rules(3)).is_ok());

    // The sizes.
    let size =
        |width, height| MapInput::new(1, [0, 0], [width, height], Vec::new(), Vec::new(), rules(1));
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
    // The lengths.
    assert_eq!(
        ok(vec![0; 4], b.clone(), rules(3)).expect_err("refused"),
        MapError::GridLength {
            grid: 'A',
            expected: 8,
            actual: 4
        }
    );
    assert_eq!(
        ok(a.clone(), vec![0; 9], rules(3)).expect_err("refused"),
        MapError::GridLength {
            grid: 'B',
            expected: 8,
            actual: 9
        }
    );
    // Grid A: alpha, G (fertility / 4 up to 250, or 255 for a glacier), B (a strength + 128: 113 to 143), a terrain index the rules lack
    // (cell (1, 0) is the second one).
    for (cell, problem) in [
        ([1, 1, 128, 254], "alpha is not 255"),
        (
            [1, 251, 128, 255],
            "G is neither fertility / 4 (0 to 250) nor the glacier marker 255",
        ),
        (
            [1, 254, 128, 255],
            "G is neither fertility / 4 (0 to 250) nor the glacier marker 255",
        ),
        (
            [1, 1, 0, 255],
            "B is not a hillside strength + 128 (113 to 143)",
        ),
        (
            [1, 1, 112, 255],
            "B is not a hillside strength + 128 (113 to 143)",
        ),
        (
            [1, 1, 144, 255],
            "B is not a hillside strength + 128 (113 to 143)",
        ),
    ] {
        let mut bad = a.clone();
        bad[4..8].copy_from_slice(&cell);
        assert_eq!(
            ok(bad, b.clone(), rules(3)).expect_err("refused"),
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
        ok(bad, b.clone(), rules(3)).expect_err("refused"),
        MapError::TerrainIndex {
            x: 0,
            y: 0,
            index: 3,
            terrains: 3
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
            ok(a.clone(), bad, rules(3)).expect_err("refused"),
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
    assert!(ok(a, free, rules(3)).is_ok());
}

#[test]
fn a_rule_table_is_checked_and_names_the_first_problem() {
    let ok =
        |a: Vec<u8>, b: Vec<u8>, rules: MapRules| MapInput::new(1, [0, 0], [2, 1], a, b, rules);
    let a = grid(2, 1, [1, 1, 128, 255]);
    let b = grid(2, 1, [0, 0, 0, 255]);
    // The rule table: its length, and each entry's own rules.
    let problem = |change: &dyn Fn(&mut MapRules)| {
        let mut broken = rules(3);
        change(&mut broken);
        ok(a.clone(), b.clone(), broken).expect_err("refused")
    };
    let table = |problem| MapError::Rules {
        terrain: None,
        problem,
    };
    let terrain = |terrain, problem| MapError::Rules {
        terrain: Some(terrain),
        problem,
    };
    assert_eq!(
        problem(&|rules| rules.terrains.clear()),
        table("it needs from 1 to 256 terrain types")
    );
    assert_eq!(
        problem(&|rules| rules.terrains = vec![land(); TERRAIN_MAX + 1]),
        table("it needs from 1 to 256 terrain types")
    );
    assert!(ok(a.clone(), b.clone(), rules(TERRAIN_MAX)).is_ok());
    assert_eq!(
        problem(&|rules| rules.shallow_threshold = 0),
        table("the shallow-sea threshold must be at least 1")
    );
    for (change, what) in [
        (
            (&|rules: &mut MapRules| rules.sea[3] = 254) as &dyn Fn(&mut MapRules),
            "the sea colour is not opaque",
        ),
        (
            &|rules| rules.shallow_sea[3] = 0,
            "the shallow-sea colour is not opaque",
        ),
        (&|rules| rules.lake[3] = 1, "the lake colour is not opaque"),
        (
            &|rules| rules.river[3] = 200,
            "the river colour is not opaque",
        ),
        (
            &|rules| rules.steps[STEP_COUNT - 1] = 0,
            "a step size must be from 1 to 2^24 - 1",
        ),
        (
            &|rules| rules.steps[3] = 1 << 24,
            "a step size must be from 1 to 2^24 - 1",
        ),
    ] {
        assert_eq!(problem(change), table(what));
    }
    assert_eq!(
        problem(&|rules| rules.terrains[1].shadow[2][3] = 200),
        terrain(1, "a land colour is not opaque")
    );
    assert_eq!(
        problem(&|rules| rules.terrains[0].lit[0][3] = 0),
        terrain(0, "a land colour is not opaque")
    );
    assert_eq!(
        problem(&|rules| rules.ice[3] = 254),
        table("the ice colour is not opaque")
    );
    assert_eq!(
        problem(&|rules| rules.ice_second[3] = 0),
        table("the ice second tone is not opaque")
    );
}

#[test]
fn the_rules_a_kind_reads_and_the_crop_are_checked() {
    let ok =
        |a: Vec<u8>, b: Vec<u8>, rules: MapRules| MapInput::new(1, [0, 0], [2, 1], a, b, rules);
    let a = grid(2, 1, [1, 1, 128, 255]);
    let b = grid(2, 1, [0, 0, 0, 255]);
    let problem = |change: &dyn Fn(&mut MapRules)| {
        let mut broken = rules(3);
        change(&mut broken);
        ok(a.clone(), b.clone(), broken).expect_err("refused")
    };
    let terrain = |terrain, problem| MapError::Rules {
        terrain: Some(terrain),
        problem,
    };
    // A mountain's tones are read like ordinary land's; water ignores them.
    assert_eq!(
        problem(&|rules| {
            rules.terrains[1].kind = MapTerrainKind::Mountain;
            rules.terrains[1].tone[1][3] = 0;
        }),
        terrain(1, "a land colour is not opaque")
    );
    let mut sloppy = rules(3);
    sloppy.terrains[1].kind = MapTerrainKind::Sea;
    sloppy.terrains[1].tone[3][3] = 0;
    assert!(ok(a.clone(), b.clone(), sloppy).is_ok());
    // The crop must lie in the non-negative world and leave room past its far edge.
    for (origin, size) in [
        ([-1, 0], [2, 1]),
        ([0, -1], [2, 1]),
        ([i32::MAX / 32_768, 0], [2, 1]),
    ] {
        assert_eq!(
            MapInput::new(1, origin, size, a.clone(), b.clone(), rules(3)).expect_err("refused"),
            MapError::Crop { origin, size }
        );
    }
}

#[test]
fn a_camera_is_checked_and_its_rivers_are_bounded_by_the_view() {
    assert!(MapCamera::new(0, [0, 0], [3, 2], vec![[2, 1]], 1).is_ok());
    assert!(MapCamera::new(75, [0, 0], [3, 2], Vec::new(), 1).is_ok());
    assert_eq!(
        MapCamera::new(76, [0, 0], [3, 2], Vec::new(), 1).expect_err("refused"),
        MapError::Step { step: 76 }
    );
    assert_eq!(
        MapCamera::new(0, [0, 0], [0, 2], Vec::new(), 1).expect_err("refused"),
        MapError::Size {
            what: "view",
            width: 0,
            height: 2
        }
    );
    assert_eq!(
        MapCamera::new(0, [0, 0], [3, 2], vec![[0, 0]; 7], 1).expect_err("refused"),
        MapError::TooManyRivers { limit: 6, count: 7 }
    );
    assert_eq!(
        MapCamera::new(0, [0, 0], [3, 2], vec![[0, 0], [3, 0]], 1).expect_err("refused"),
        MapError::RiverOutsideView {
            position: 1,
            x: 3,
            y: 0
        }
    );
    let held = MapCamera::new(33, [-7, 9], [3, 2], Vec::new(), 4).unwrap();
    assert_eq!(
        (
            held.step(),
            held.top_left(),
            held.view(),
            held.metrics_generation()
        ),
        (33, [-7, 9], [3, 2], 4)
    );
}

#[test]
fn location_snapshots_are_bounded_checked_and_use_the_input_palette() {
    assert!(
        MapCamera::new_with_locations(46, [10, -4], [3, 2], vec![[0, 0]], vec![[2, 1, 1]], 2, 7,)
            .is_ok()
    );
    assert_eq!(
        MapCamera::new_with_locations(0, [0, 0], [3, 2], vec![], vec![[0, 0, 0]; 7], 1, 1)
            .expect_err("too many location pixels refused"),
        MapError::TooManyLocations { limit: 6, count: 7 }
    );
    assert_eq!(
        MapCamera::new_with_locations(0, [0, 0], [3, 2], vec![], vec![[3, 0, 0]], 1, 1)
            .expect_err("out of view refused"),
        MapError::LocationOutsideView {
            position: 0,
            x: 3,
            y: 0
        }
    );
    assert_eq!(
        MapCamera::new_with_locations(0, [0, 0], [3, 2], vec![], vec![[1, 1, 1]], 1, 1)
            .expect_err("missing palette index refused"),
        MapError::LocationPaletteIndex {
            position: 0,
            index: 1,
            colours: 1
        }
    );

    let mut map_rules = rules(3);
    map_rules.overlay_palette = vec![[20, 30, 40, 255], [50, 60, 70, 255]];
    let map_input = MapInput::new(
        9,
        [100, 200],
        [2, 1],
        grid(2, 1, [1, 1, 128, 255]),
        grid(2, 1, [0, 0, 0, 255]),
        map_rules,
    )
    .expect("palette is part of a validated map input");
    assert_eq!(
        map_input.rules().overlay_palette,
        [[20, 30, 40, 255], [50, 60, 70, 255]]
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
    bridge.set_metrics((100, 60), pixels(100, 60));
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
    bridge.set_metrics((100, 60), pixels(200, 120));
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
fn a_staged_initial_camera_makes_the_first_attached_map_snapshot_complete() {
    let bridge = SceneBridge::new(Filter::Nearest);
    assert_eq!(
        bridge.publish_map(input(1, 4, 4)),
        MapPublication::Published
    );
    assert_eq!(
        bridge.stage_initial_camera(camera([2, 2], 1)),
        MapPublication::Published
    );
    let sink = RecordingSink::new();
    bridge
        .attach(
            &desc(100, 60),
            pixels(100, 60),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .expect("attached");
    let first = sink.last().expect("first publication");
    let map = first.map.expect("the first frame contains the map camera");
    assert_eq!(map.camera.step(), 40);
    assert_eq!(map.input.generation(), 1);
    assert!(
        map.placement.is_some(),
        "the first camera is bound to attach metrics"
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

#[cfg(all(feature = "metal", target_os = "macos"))]
#[test]
fn the_metal_conversion_shares_one_input_per_generation_and_retains_nothing() {
    let (bridge, snapshot) = snapshot_with_a_map();
    let mut cache = None;
    let first = crate::metal::metal_map_cached(&snapshot, &mut cache)
        .expect("converted")
        .expect("a map");
    let second = crate::metal::metal_map_cached(&snapshot, &mut cache)
        .expect("converted")
        .expect("a map");
    let weak = cache.as_ref().expect("the input is cached").clone();
    assert_eq!(
        weak.strong_count(),
        2,
        "both maps share the one input the cache refers to"
    );
    // A snapshot that shows no map forgets the cache; the maps going away let go of the input: nothing is retained.
    // A snapshot that shows no map forgets the cache, and the maps going away let go of the input: nothing is retained.
    let mut withdrawn = snapshot.clone();
    withdrawn.map = None;
    assert!(
        crate::metal::metal_map_cached(&withdrawn, &mut cache)
            .expect("converted")
            .is_none()
    );
    assert!(cache.is_none(), "a map-free snapshot clears the cache");
    drop((first, second));
    assert_eq!(weak.strong_count(), 0, "the cache held nothing alive");
    drop(bridge);
}

#[cfg(feature = "vulkan")]
#[test]
fn the_vulkan_conversion_shares_one_input_per_generation_and_retains_nothing() {
    let (bridge, snapshot) = snapshot_with_a_map();
    let mut cache = None;
    let first = crate::vulkan::vulkan_map_cached(&snapshot, &mut cache)
        .expect("converted")
        .expect("a map");
    let second = crate::vulkan::vulkan_map_cached(&snapshot, &mut cache)
        .expect("converted")
        .expect("a map");
    let weak = cache.as_ref().expect("the input is cached").clone();
    assert_eq!(
        weak.strong_count(),
        2,
        "both maps share the one input the cache refers to"
    );
    // A snapshot that shows no map forgets the cache, and the maps going away let go of the input: nothing is retained.
    let mut withdrawn = snapshot.clone();
    withdrawn.map = None;
    assert!(
        crate::vulkan::vulkan_map_cached(&withdrawn, &mut cache)
            .expect("converted")
            .is_none()
    );
    assert!(cache.is_none(), "a map-free snapshot clears the cache");
    drop((first, second));
    assert_eq!(weak.strong_count(), 0, "the cache held nothing alive");
    drop(bridge);
}

#[test]
fn a_resize_replaces_the_points_and_pixels_together_once_and_a_map_pixel_stays_four_points() {
    let (bridge, sink) = attached();
    bridge.publish_map(input(1, 40, 40));
    // The window opens at 100x60 points on a 2x display: 200x120 pixels. A view of 25x15 map pixels (four points each) covers it.
    bridge.set_metrics((100, 60), pixels(200, 120));
    let generation = bridge.metrics_generation();
    assert_eq!(generation, 2, "attach, then one change");
    let opening = camera([25, 15], generation);
    assert_eq!(
        bridge.publish_camera(Arc::clone(&opening)),
        MapPublication::Published
    );
    let placed = |sink: &RecordingSink| {
        let snapshot = sink.last().expect("published");
        (
            snapshot.metrics,
            snapshot.map.expect("a map").placement.expect("placed"),
        )
    };
    let (metrics_before, placement_before) = placed(&sink);
    assert_eq!(
        (metrics_before.points_width, metrics_before.points_height),
        (100, 60)
    );
    assert_eq!(
        (
            placement_before.destination.right,
            placement_before.destination.bottom
        ),
        (200, 120),
        "eight drawable pixels a map pixel"
    );
    // A resize at the same scale: 150x90 points, 300x180 pixels, in one call: the generation rises by exactly one, and both sizes
    // are the bridge's.
    bridge.set_metrics((150, 90), pixels(300, 180));
    assert_eq!(bridge.metrics_generation(), generation + 1);
    let resized = sink.last().expect("published");
    assert_eq!(
        (resized.metrics.points_width, resized.metrics.points_height),
        (150, 90)
    );
    assert_eq!(resized.metrics.pixels, pixels(300, 180));
    // The camera for the previous generation is refused as stale, and no map is drawn from it with the new metrics.
    assert_eq!(
        bridge.publish_camera(Arc::clone(&opening)),
        MapPublication::Stale
    );
    assert!(
        resized.map.is_none(),
        "the old camera is never placed with the new metrics"
    );
    // The camera for the new size is adopted, and a map pixel is still eight drawable pixels (four points at 2x).
    let fitted = camera([37, 22], bridge.metrics_generation());
    assert_eq!(bridge.publish_camera(fitted), MapPublication::Published);
    let (metrics_after, placement_after) = placed(&sink);
    assert_eq!(metrics_after.pixels, pixels(300, 180));
    assert_eq!(
        (
            placement_after.destination.right,
            placement_after.destination.bottom
        ),
        (296, 176),
        "37 and 22 map pixels of 8 drawable pixels each"
    );
    // A change of the display's scale alone (the points stay): the camera is stale again, and the map pixel is four drawable pixels.
    bridge.set_metrics((150, 90), pixels(150, 90));
    assert_eq!(bridge.metrics_generation(), generation + 2);
    let at_one = camera([37, 22], bridge.metrics_generation());
    assert_eq!(bridge.publish_camera(at_one), MapPublication::Published);
    let (_, placement_one) = placed(&sink);
    assert_eq!(
        (
            placement_one.destination.right,
            placement_one.destination.bottom
        ),
        (148, 88),
        "four drawable pixels a map pixel at 1x"
    );
}
