//! The GPU-written map image (milestone 6, slices 1 and 6), off screen: the presenter's own map pass and composition, read back under
//! the validation layer (with synchronization validation) with no window. A submodule of `gpu.rs`, so the group that runs that target
//! runs these.
//!
//! The fixture is `ck-map`'s slice 6 fixture (`crates/map/tests/fixture/mod.rs`, included below): a 24 x 16 crop that exercises every
//! rule of `docs/map-design.md` section 9.4 (rules 1 to 6 and 8). Every expected pixel is `ck_map::reference_colour`'s, the reference
//! the map pass must match byte for byte; slice 1's minimal rule is gone. Any validation message fails a case, hazards between the map
//! pass and the window pass included.
//!
//! The window readback is `B8G8R8A8_UNORM` (a texel `(r, g, b, a)` reads back as `[b, g, r, a]`); the map target is `R8G8B8A8_UNORM`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use ck_gpu_vulkan::{
    DestinationRect, PixelSource, SourceRect, VulkanFilter, VulkanMap, VulkanMapError,
    VulkanMapInput, VulkanMapRules, VulkanQuad, VulkanTerrainKind, VulkanTerrainRule, load_loader,
    readback_map, readback_quad,
};
use ck_map::{
    FIXED_PER_CELL, ModeInputs, PreparedLocations, PreparedRivers, RiverLine, STEP_SIZE, ViewSize,
    WorldPixel, fixture, reference_colour_with_locations, stockpile_value, yield_values,
};

use super::{assert_log_clean, destroy, validated_with};

const ORIGIN: [i32; 2] = fixture::ORIGIN;
const CROP: [u32; 2] = fixture::SIZE;

/// The step of the cases that do not choose one: a tenth of a cell per map pixel, so the cases see all three octaves of the sub-cell
/// detail and about ten map pixels to a cell.
const STEP: u8 = 60;

struct Bytes(Vec<u8>);

impl PixelSource for Bytes {
    fn pixels(&self) -> &[u8] {
        &self.0
    }
}

fn the_fixture() -> &'static fixture::Fixture {
    static FIXTURE: OnceLock<fixture::Fixture> = OnceLock::new();
    FIXTURE.get_or_init(fixture::fixture)
}

/// The world pixel of the crop's top-left corner at [`STEP`].
fn corner() -> [i32; 2] {
    let pixel = fixture::Fixture::crop_top_left(STEP);
    [pixel.x, pixel.y]
}

/// The world pixel `(dx, dy)` map pixels from the crop's top-left corner at [`STEP`].
fn at(dx: i32, dy: i32) -> [i32; 2] {
    let [x, y] = corner();
    [x + dx, y + dy]
}

fn kind_of(kind: fixture::Kind) -> VulkanTerrainKind {
    match kind {
        fixture::Kind::Land => VulkanTerrainKind::Land,
        fixture::Kind::Sea => VulkanTerrainKind::Sea,
        fixture::Kind::Lake => VulkanTerrainKind::Lake,
        fixture::Kind::Mountain => VulkanTerrainKind::Mountain,
    }
}

/// The fixture's rule table as the presenter takes it.
fn rules_of(fixture: &fixture::Fixture) -> VulkanMapRules {
    VulkanMapRules {
        terrains: fixture
            .terrains
            .iter()
            .map(|terrain| VulkanTerrainRule {
                kind: kind_of(terrain.kind),
                solid: terrain.style.solid.0,
                second: terrain.style.second.0,
                dark: terrain.style.dark.0,
                // Vulkan is compile-only until the milestone's catch-up (CK-239): its map pass still has the earlier look's rule fields,
                // filled from the new tones just so that this builds. These cases are not run, and their fixtures are regenerated then.
                dark_second: terrain.style.dark_lit.0,
                cap: terrain.style.solid.0,
                high_ground: terrain.style.dark.0,
                relief: [0, 1],
                mix_percent: 0,
                cap_heights: [0, 1],
            })
            .collect(),
        sea: fixture.data.sea.0,
        shallow_sea: fixture.data.shallow_sea.0,
        lake: fixture.data.lake.0,
        river: fixture.data.river.0,
        overlay_palette: Vec::new(),
        shallow_threshold: fixture.data.shallow_threshold,
        steps: STEP_SIZE,
    }
}

fn input_of(fixture: &fixture::Fixture, generation: u64) -> Arc<VulkanMapInput> {
    Arc::new(
        VulkanMapInput::new(
            generation,
            ORIGIN,
            CROP,
            fixture.grid_a.clone().into(),
            fixture.grid_b.clone().into(),
            &rules_of(fixture),
        )
        .expect("a valid input"),
    )
}

fn input_with(generation: u64) -> Arc<VulkanMapInput> {
    input_of(the_fixture(), generation)
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> DestinationRect {
    DestinationRect {
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

fn float(value: u32) -> f32 {
    f32::from(u16::try_from(value).expect("a small number"))
}

fn signed(value: u32) -> i32 {
    i32::try_from(value).expect("a small number")
}

/// The map of `input` seen from `top_left` through a `view`, enlarged by `scale` into the top-left of the window.
fn map_of(
    input: &Arc<VulkanMapInput>,
    top_left: [i32; 2],
    view: [u32; 2],
    scale: u32,
) -> VulkanMap {
    map_at(input, STEP, top_left, view, scale)
}

/// [`map_of`] at a chosen zoom step.
fn map_at(
    input: &Arc<VulkanMapInput>,
    step: u8,
    top_left: [i32; 2],
    view: [u32; 2],
    scale: u32,
) -> VulkanMap {
    let (width, height) = (view[0] * 4 * scale, view[1] * 4 * scale);
    VulkanMap::new(
        Arc::clone(input),
        step,
        top_left,
        view,
        rect(0, 0, signed(width), signed(height)),
        source(0.0, 0.0, float(view[0]), float(view[1])),
    )
    .expect("a valid map")
}

/// The map target the reference gives, RGBA8 rows top first.
fn expected_target(top_left: [i32; 2], view: [u32; 2]) -> Vec<u8> {
    the_fixture().reference_image(STEP, top_left, view)
}

fn fixture_rivers(step: u8, top_left: [i32; 2], view: [u32; 2]) -> Vec<[u32; 2]> {
    let point = |x: i32, y: i32| ck_map::FixedPoint {
        x: i32::try_from((i64::from(ORIGIN[0]) + i64::from(x)) * FIXED_PER_CELL)
            .expect("fixture x fits"),
        y: i32::try_from((i64::from(ORIGIN[1]) + i64::from(y)) * FIXED_PER_CELL)
            .expect("fixture y fits"),
    };
    let lines = vec![
        RiverLine {
            points: vec![point(-2, 5), point(12, 5)],
        },
        RiverLine {
            points: vec![point(3, 1), point(20, 14)],
        },
        RiverLine {
            points: vec![point(17, 1), point(18, 15)],
        },
    ];
    for (name, line) in ["horizontal", "diagonal", "steep"].into_iter().zip(&lines) {
        assert!(
            !PreparedRivers::new(vec![line.clone()])
                .select_view(
                    step,
                    WorldPixel {
                        x: top_left[0],
                        y: top_left[1],
                    },
                    ViewSize {
                        width: view[0],
                        height: view[1],
                    },
                )
                .expect("fixture step has river coverage")
                .is_empty(),
            "the {name} fixture river is visible at step {step}"
        );
    }
    PreparedRivers::new(lines)
        .select_view(
            step,
            WorldPixel {
                x: top_left[0],
                y: top_left[1],
            },
            ViewSize {
                width: view[0],
                height: view[1],
            },
        )
        .expect("fixture step has river coverage")
}

/// The pixel at (x, y) of a BGRA8 readback, as RGBA.
fn bgra_at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let offset = usize::try_from((y * width + x) * 4).expect("small");
    [
        pixels[offset + 2],
        pixels[offset + 1],
        pixels[offset],
        pixels[offset + 3],
    ]
}

const CLEAR: [f32; 3] = [0.25, 0.5, 0.75];

/// A clear colour as the bytes a `B8G8R8A8_UNORM` target stores, as RGBA.
fn clear_rgba(rgb: [f32; 3]) -> [u8; 4] {
    let byte = |value: f32| -> u8 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a value in 0..=1 times 255 and rounded is in 0..=255"
        )]
        let rounded = (value * 255.0).round() as u8;
        rounded
    };
    [byte(rgb[0]), byte(rgb[1]), byte(rgb[2]), 255]
}

/// The Vulkan loader, an instance with validation **and synchronization validation**, and the log: every case starts here.
macro_rules! rig {
    () => {{
        let loader = load_loader().expect("the Vulkan loader loads");
        let instance = validated_with(&loader, true);
        let log = instance.validation_log();
        (loader, instance, log)
    }};
}

/// The map target equals the reference colour function byte for byte on every scenario of the fixture: steps coarser than a cell and
/// finer (the three octaves of sub-cell detail), views overlapping the crop's edge and lying wholly outside it, odd view sizes, the
/// coast, the lake, the shelf, the mountains with their caps and high ground. The last scenario is a view again after one far away: a
/// pan away and back, with identical bytes.
#[test]
fn the_map_target_equals_the_reference_and_a_pan_away_and_back_gives_identical_bytes() {
    let (_loader, instance, log) = rig!();
    let fixture = the_fixture();
    let input = input_with(1);
    let mut first_visit: Option<Vec<u8>> = None;
    for scenario in fixture::Fixture::scenarios() {
        let map = map_at(&input, scenario.step, scenario.top_left, scenario.view, 1);
        let drawn = readback_map(
            &instance,
            scenario.view[0] * 4,
            scenario.view[1] * 4,
            CLEAR,
            Some(&map),
            None,
        )
        .expect("the readback works");
        let target = drawn.target.expect("a map target");
        let expected = fixture.reference_image(scenario.step, scenario.top_left, scenario.view);
        let wrong = target
            .chunks_exact(4)
            .zip(expected.chunks_exact(4))
            .enumerate()
            .filter(|(_, (got, want))| got != want)
            .map(|(index, (got, want))| {
                (
                    index % scenario.view[0] as usize,
                    index / scenario.view[0] as usize,
                    got,
                    want,
                )
            })
            .take(5)
            .collect::<Vec<_>>();
        assert!(
            wrong.is_empty(),
            "{}: the target differs from the reference at (x, y, got, want) {wrong:?}",
            scenario.name
        );
        assert_eq!(target, expected, "{}", scenario.name);
        if scenario.name.starts_with("half a cell") {
            first_visit = Some(target);
        } else if scenario.name.starts_with("back from far away") {
            assert_eq!(
                Some(target),
                first_visit,
                "away and back gives identical bytes"
            );
        }
    }
    assert_log_clean(&log);
    assert_eq!(instance.retained_readbacks(), 0);
    destroy(instance);
    assert_log_clean(&log);
}

/// Prepared river pixels replace the reference byte-for-byte on two steps; the left view edge cuts a river line.
#[test]
fn prepared_rivers_overlay_the_map_reference_at_two_steps() {
    let (_loader, instance, log) = rig!();
    let fixture = the_fixture();
    let input = input_with(1);
    for (step, offset, view) in [(60, 2, [260, 180]), (36, 1, [52, 37])] {
        let corner = fixture::Fixture::crop_top_left(step);
        let top_left = [corner.x - offset, corner.y - offset];
        let rivers = fixture_rivers(step, top_left, view);
        assert!(!rivers.is_empty(), "step {step} has visible river pixels");
        let (mut sea, mut land, mut lake) = (false, false, false);
        for [x, y] in &rivers {
            let colour = fixture.reference(
                step,
                WorldPixel {
                    x: top_left[0] + signed(*x),
                    y: top_left[1] + signed(*y),
                },
            );
            sea |= colour == fixture.data.sea.0 || colour == fixture.data.shallow_sea.0;
            lake |= colour == fixture.data.lake.0;
            land |= colour != fixture.data.sea.0
                && colour != fixture.data.shallow_sea.0
                && colour != fixture.data.lake.0;
        }
        assert!(
            sea && land && lake,
            "rivers cross sea/coast/land and lake at step {step}"
        );
        assert!(
            rivers.iter().any(|pixel| pixel[0] == 0),
            "the view edge cuts a river"
        );
        let map = VulkanMap::new_with_rivers(
            Arc::clone(&input),
            step,
            top_left,
            view,
            rect(0, 0, signed(view[0] * 4), signed(view[1] * 4)),
            source(0.0, 0.0, float(view[0]), float(view[1])),
            rivers.clone(),
        )
        .expect("a valid river map");
        let drawn = readback_map(&instance, view[0] * 4, view[1] * 4, CLEAR, Some(&map), None)
            .expect("the river readback works");
        let mut expected = fixture.reference_image(step, top_left, view);
        for [x, y] in rivers {
            let offset = usize::try_from((y * view[0] + x) * 4).expect("small fixture");
            expected[offset..offset + 4].copy_from_slice(&fixture.data.river.0);
        }
        assert_eq!(drawn.target.expect("a map target"), expected, "step {step}");
    }
    assert_log_clean(&log);
    assert_eq!(instance.retained_readbacks(), 0);
    destroy(instance);
    assert_log_clean(&log);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "keep the two-step readback, pan control and empty-list control together"
)]
fn prepared_locations_overlay_the_map_reference_at_marker_and_house_steps() {
    let (_loader, instance, log) = rig!();
    let fixture = the_fixture();
    let scenario = fixture::location_scenario();
    let palette = scenario
        .location_kinds
        .overlay_palette()
        .into_iter()
        .map(|colour| colour.0)
        .collect::<Vec<_>>();
    let mut prepared = PreparedLocations::new(
        scenario.locations.clone(),
        scenario.location_kinds.clone(),
        scenario.map.data.crop,
    );
    let mut rules = rules_of(fixture);
    rules.overlay_palette.clone_from(&palette);
    let input = Arc::new(
        VulkanMapInput::new(
            2,
            ORIGIN,
            CROP,
            fixture.grid_a.clone().into(),
            fixture.grid_b.clone().into(),
            &rules,
        )
        .expect("location palette is carried by the map input"),
    );

    for (step, offset, view) in [(15, 1, [35, 25]), (46, 2, [99, 67])] {
        let crop_corner = fixture::Fixture::crop_top_left(step);
        let top_left = [crop_corner.x - offset, crop_corner.y - offset];
        let mut rivers = fixture_rivers(step, top_left, view)
            .into_iter()
            .filter(|pixel| fixture_pixel_inside_crop(step, top_left, *pixel))
            .collect::<Vec<_>>();
        let locations = prepared
            .select_view(
                step,
                WorldPixel {
                    x: top_left[0],
                    y: top_left[1],
                },
                ViewSize {
                    width: view[0],
                    height: view[1],
                },
            )
            .expect("fixture scenario step is supported")
            .into_iter()
            .map(|([x, y], index)| [x, y, u32::from(index)])
            .collect::<Vec<_>>();
        if step == 46 {
            let [x, y, _] = locations
                .iter()
                .find(|[_, _, index]| (6..9).contains(index))
                .copied()
                .expect("fixture scenario has a visible house pixel at step 46");
            let house = WorldPixel {
                x: top_left[0] + signed(x),
                y: top_left[1] + signed(y),
            };
            let mut crossing = PreparedRivers::new(vec![
                fixture::river_through_pixel(house, step)
                    .expect("fixture house river fits fixed-point coordinates"),
            ]);
            rivers.extend(
                crossing
                    .select_view(
                        step,
                        WorldPixel {
                            x: top_left[0],
                            y: top_left[1],
                        },
                        ViewSize {
                            width: view[0],
                            height: view[1],
                        },
                    )
                    .expect("fixture house river step is supported")
                    .into_iter()
                    .filter(|pixel| fixture_pixel_inside_crop(step, top_left, *pixel)),
            );
        }
        rivers.sort_unstable();
        rivers.dedup();
        let river_pixels = rivers
            .iter()
            .map(|[x, y]| WorldPixel {
                x: top_left[0] + signed(*x),
                y: top_left[1] + signed(*y),
            })
            .collect::<BTreeSet<_>>();
        let has_visible_location_colour = locations.iter().any(|[x, y, index]| {
            !river_pixels.contains(&WorldPixel {
                x: top_left[0] + signed(*x),
                y: top_left[1] + signed(*y),
            }) && *index < u32::try_from(palette.len()).expect("small palette")
        });
        let has_visible_palette_range = |range: std::ops::Range<u32>| {
            locations.iter().any(|[x, y, index]| {
                range.contains(index)
                    && !river_pixels.contains(&WorldPixel {
                        x: top_left[0] + signed(*x),
                        y: top_left[1] + signed(*y),
                    })
            })
        };
        let has_river_overlap = |range: std::ops::Range<u32>| {
            locations.iter().any(|[x, y, index]| {
                range.contains(index)
                    && river_pixels.contains(&WorldPixel {
                        x: top_left[0] + signed(*x),
                        y: top_left[1] + signed(*y),
                    })
            })
        };
        assert!(
            has_visible_location_colour,
            "prepared marker, house or building pixels remain visible off the river"
        );
        if step == 15 {
            assert!(
                has_visible_palette_range(0..4),
                "step 15 has a visible marker pixel"
            );
            assert!(
                has_river_overlap(0..4),
                "the river crosses a marker at step 15"
            );
        } else {
            assert!(
                has_visible_palette_range(4..6),
                "step 46 has a visible building pixel"
            );
            assert!(
                has_visible_palette_range(6..9),
                "step 46 has a visible house pixel"
            );
            assert!(
                has_river_overlap(6..9),
                "the river crosses a house at step 46"
            );
        }
        let colour_by_pixel = locations
            .iter()
            .map(|[x, y, index]| {
                (
                    WorldPixel {
                        x: top_left[0] + signed(*x),
                        y: top_left[1] + signed(*y),
                    },
                    ck_map::Rgba8(
                        palette[usize::try_from(*index).expect("literal palette index fits")],
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let expected = (0..view[1])
            .flat_map(|y| (0..view[0]).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                reference_colour_with_locations(
                    &fixture.data,
                    step,
                    WorldPixel {
                        x: top_left[0] + signed(x),
                        y: top_left[1] + signed(y),
                    },
                    &river_pixels,
                    &colour_by_pixel,
                )
                .expect("reference colour")
                .0
            })
            .collect::<Vec<_>>();
        let map = VulkanMap::new_with_overlays(
            Arc::clone(&input),
            step,
            top_left,
            view,
            rect(0, 0, signed(view[0] * 4), signed(view[1] * 4)),
            source(0.0, 0.0, float(view[0]), float(view[1])),
            rivers.clone(),
            locations,
        )
        .expect("valid overlay map");
        let drawn = readback_map(&instance, view[0] * 4, view[1] * 4, CLEAR, Some(&map), None)
            .expect("overlay readback works");
        let target = drawn.target.expect("map target");
        assert_eq!(target, expected, "location/rivers reference at step {step}");

        let far = VulkanMap::new_with_rivers(
            Arc::clone(&input),
            step,
            [top_left[0] + 5_000, top_left[1] + 5_000],
            view,
            rect(0, 0, signed(view[0] * 4), signed(view[1] * 4)),
            source(0.0, 0.0, float(view[0]), float(view[1])),
            Vec::new(),
        )
        .expect("far-away pan is a valid camera");
        let _ = readback_map(&instance, view[0] * 4, view[1] * 4, CLEAR, Some(&far), None)
            .expect("far-away pan readback works");
        let back = readback_map(&instance, view[0] * 4, view[1] * 4, CLEAR, Some(&map), None)
            .expect("return pan readback works");
        assert_eq!(back.target.expect("returned map target"), target);

        let empty = VulkanMap::new_with_overlays(
            Arc::clone(&input),
            step,
            top_left,
            view,
            rect(0, 0, signed(view[0] * 4), signed(view[1] * 4)),
            source(0.0, 0.0, float(view[0]), float(view[1])),
            rivers,
            Vec::new(),
        )
        .expect("empty locations accepted");
        let baseline = readback_map(
            &instance,
            view[0] * 4,
            view[1] * 4,
            CLEAR,
            Some(&empty),
            None,
        )
        .expect("empty control readback works");
        let expected_without_locations = (0..view[1])
            .flat_map(|y| (0..view[0]).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                reference_colour_with_locations(
                    &fixture.data,
                    step,
                    WorldPixel {
                        x: top_left[0] + signed(x),
                        y: top_left[1] + signed(y),
                    },
                    &river_pixels,
                    &BTreeMap::new(),
                )
                .expect("reference colour")
                .0
            })
            .collect::<Vec<_>>();
        assert_eq!(
            baseline.target.expect("baseline map target"),
            expected_without_locations,
            "no locations preserves the existing readback"
        );
    }
    assert_log_clean(&log);
    assert_eq!(instance.retained_readbacks(), 0);
    destroy(instance);
    assert_log_clean(&log);
}

fn fixture_pixel_inside_crop(step: u8, top_left: [i32; 2], pixel: [u32; 2]) -> bool {
    let crop = the_fixture().data.crop;
    let size = i64::from(STEP_SIZE[usize::from(step)]);
    let centre = |world: i32| i64::from(world) * size + size / 2;
    let left = i64::from(crop.origin.x) * FIXED_PER_CELL;
    let top = i64::from(crop.origin.y) * FIXED_PER_CELL;
    let right = (i64::from(crop.origin.x) + i64::from(crop.width)) * FIXED_PER_CELL;
    let bottom = (i64::from(crop.origin.y) + i64::from(crop.height)) * FIXED_PER_CELL;
    let x = centre(top_left[0] + signed(pixel[0]));
    let y = centre(top_left[1] + signed(pixel[1]));
    x >= left && x < right && y >= top && y < bottom
}

/// Negative control: with one rule table entry deliberately wrong (pasture's second tone another colour) the readback changes in
/// exactly the pixels that show that tone, and in no other.
#[test]
fn a_wrong_rule_table_entry_changes_exactly_the_pixels_it_colours() {
    let (_loader, instance, log) = rig!();
    let fixture = the_fixture();
    let wrong = fixture.with_other_pasture_second_tone();
    let input = input_of(&wrong, 1);
    let second = fixture.terrains[2].style.second.0;
    let mut changed = 0;
    for scenario in fixture::Fixture::scenarios() {
        let map = map_at(&input, scenario.step, scenario.top_left, scenario.view, 1);
        let drawn = readback_map(
            &instance,
            scenario.view[0] * 4,
            scenario.view[1] * 4,
            CLEAR,
            Some(&map),
            None,
        )
        .expect("the readback works");
        let got = drawn.target.expect("a map target");
        let want = fixture.reference_image(scenario.step, scenario.top_left, scenario.view);
        for (index, (got, want)) in got.chunks_exact(4).zip(want.chunks_exact(4)).enumerate() {
            assert_eq!(
                got != want,
                want == second,
                "{}: pixel {index} (reference {want:?}, drawn {got:?})",
                scenario.name
            );
            changed += usize::from(got != want);
        }
    }
    assert!(changed > 0, "the wrong entry changed something");
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

/// A view that is not the crop's shape (5 x 3 over 7 x 5), one larger than the crop and a one-pixel view read the same cells.
#[test]
fn views_of_other_shapes_read_the_same_cells() {
    let (_loader, instance, log) = rig!();
    let input = input_with(1);
    for view in [[1, 1], [5, 3], [9, 7], [7, 5], [2, 9]] {
        let top_left = at(-1, -1);
        let map = map_of(&input, top_left, view, 1);
        let drawn = readback_map(&instance, view[0] * 4, view[1] * 4, CLEAR, Some(&map), None)
            .expect("the readback works");
        assert_eq!(
            drawn.target.expect("a map target"),
            expected_target(top_left, view),
            "view {view:?}"
        );
    }
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

/// The enlarged map: at scales 1 and 2 every drawable pixel equals the map pixel whose point rectangle contains its centre. A window
/// smaller than `view x 4` points crops the partial right and bottom map pixel, never stretches it.
#[test]
fn the_enlarged_map_is_the_map_pixel_whose_point_rectangle_holds_each_pixels_centre() {
    let (_loader, instance, log) = rig!();
    let input = input_with(1);
    let view = [5, 3];
    let top_left = at(2, 1);
    let target = expected_target(top_left, view);
    let map_pixel = |x: u32, y: u32| -> [u8; 4] {
        let offset = usize::try_from((y * view[0] + x) * 4).expect("small");
        [
            target[offset],
            target[offset + 1],
            target[offset + 2],
            target[offset + 3],
        ]
    };
    for scale in [1_u32, 2] {
        let (width, height) = (view[0] * 4 * scale, view[1] * 4 * scale);
        let map = map_of(&input, top_left, view, scale);
        let drawn = readback_map(&instance, width, height, CLEAR, Some(&map), None)
            .expect("the readback works");
        for py in 0..height {
            for px in 0..width {
                let (mx, my) = (px / (4 * scale), py / (4 * scale));
                assert_eq!(
                    bgra_at(&drawn.window, width, px, py),
                    map_pixel(mx, my),
                    "scale {scale}: drawable pixel ({px}, {py}) is map pixel ({mx}, {my})"
                );
            }
        }
    }
    // A window of 18 x 10 pixels (scale 1) is 4.5 x 2.5 map pixels: the destination is clipped and the source trimmed in proportion.
    let cropped = VulkanMap::new(
        Arc::clone(&input),
        STEP,
        top_left,
        view,
        rect(0, 0, 18, 10),
        source(0.0, 0.0, 4.5, 2.5),
    )
    .expect("a valid map");
    let drawn =
        readback_map(&instance, 24, 16, CLEAR, Some(&cropped), None).expect("the readback works");
    let clear = clear_rgba(CLEAR);
    for py in 0..16 {
        for px in 0..24 {
            let expected = if px < 18 && py < 10 {
                map_pixel(px / 4, py / 4)
            } else {
                clear
            };
            assert_eq!(
                bgra_at(&drawn.window, 24, px, py),
                expected,
                "cropped: ({px}, {py})"
            );
        }
    }
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

/// The map is drawn between the clear and the legacy quad: a quad over part of the enlarged map hides it there.
#[test]
fn the_map_is_below_the_legacy_quad() {
    let (_loader, instance, log) = rig!();
    let input = input_with(1);
    let view = [5, 3];
    let top_left = corner();
    let map = map_of(&input, top_left, view, 1);
    let picture = Arc::new(Bytes([255, 255, 0, 255].repeat(4)));
    let quad = VulkanQuad::new(
        picture,
        2,
        2,
        rect(4, 4, 12, 8),
        source(0.0, 0.0, 2.0, 2.0),
        VulkanFilter::Nearest,
    )
    .expect("a valid quad");
    let drawn = readback_map(&instance, 20, 12, CLEAR, Some(&map), Some(&quad))
        .expect("the readback works");
    let target = expected_target(top_left, view);
    for py in 0..12 {
        for px in 0..20 {
            let expected = if (4..12).contains(&px) && (4..8).contains(&py) {
                [255, 255, 0, 255]
            } else {
                let offset = usize::try_from(((py / 4) * view[0] + px / 4) * 4).expect("small");
                [
                    target[offset],
                    target[offset + 1],
                    target[offset + 2],
                    target[offset + 3],
                ]
            };
            assert_eq!(bgra_at(&drawn.window, 20, px, py), expected, "({px}, {py})");
        }
    }
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

/// Negative control: with no map the frame is exactly the frame without one, whatever else it draws.
#[test]
fn with_no_map_the_frame_is_the_scene_only_result() {
    let (_loader, instance, log) = rig!();
    let drawn = readback_map(&instance, 16, 8, CLEAR, None, None).expect("the readback works");
    assert!(drawn.target.is_none());
    assert_eq!(
        drawn.window,
        readback_quad(&instance, 16, 8, CLEAR, None).expect("the readback works"),
        "the clear alone"
    );
    let picture = Arc::new(Bytes([9, 90, 190, 255, 1, 2, 3, 255].repeat(2)));
    let quad = VulkanQuad::new(
        picture,
        2,
        2,
        rect(2, 1, 10, 7),
        source(0.0, 0.0, 2.0, 2.0),
        VulkanFilter::Nearest,
    )
    .expect("a valid quad");
    let with_quad =
        readback_map(&instance, 16, 8, CLEAR, None, Some(&quad)).expect("the readback works");
    assert_eq!(
        with_quad.window,
        readback_quad(&instance, 16, 8, CLEAR, Some(&quad)).expect("the readback works"),
        "the scene-only result"
    );
    assert_log_clean(&log);
    assert_eq!(instance.retained_readbacks(), 0);
    destroy(instance);
    assert_log_clean(&log);
}

/// A map the GPU would be handed in a shape it cannot read never reaches it.
#[test]
fn the_types_refuse_what_the_gpu_cannot_read() {
    let fixture = the_fixture();
    let grid: Arc<[u8]> = vec![0; 24 * 16 * 4].into();
    let rules = rules_of(fixture);
    let make = |origin: [i32; 2], size: [u32; 2], grid_b: &Arc<[u8]>, rules: &VulkanMapRules| {
        VulkanMapInput::new(
            1,
            origin,
            size,
            Arc::clone(&grid),
            Arc::clone(grid_b),
            rules,
        )
    };
    assert!(make(ORIGIN, CROP, &grid, &rules).is_ok());
    assert_eq!(
        make(ORIGIN, [0, 16], &grid, &rules).expect_err("refused"),
        VulkanMapError::Size
    );
    assert_eq!(
        make(ORIGIN, [25, 16], &grid, &rules).expect_err("refused"),
        VulkanMapError::GridLength {
            expected: 25 * 16 * 4,
            actual: 24 * 16 * 4
        }
    );
    let short: Arc<[u8]> = vec![0; 8].into();
    assert_eq!(
        make(ORIGIN, CROP, &short, &rules).expect_err("refused"),
        VulkanMapError::GridLength {
            expected: 24 * 16 * 4,
            actual: 8
        }
    );
    // The crop must lie in the non-negative world and leave room for a map pixel past its far edge.
    for origin in [[-1, 0], [0, -1], [i32::MAX / 32_768, 0], [0, 65_500]] {
        assert_eq!(
            make(origin, CROP, &grid, &rules).expect_err("refused"),
            VulkanMapError::Crop,
            "{origin:?}"
        );
    }
    // The rule table must be one the shader can read.
    let rules_error = |change: &dyn Fn(&mut VulkanMapRules)| {
        let mut broken = rules.clone();
        change(&mut broken);
        match make(ORIGIN, CROP, &grid, &broken).expect_err("refused") {
            VulkanMapError::Rules(reason) => reason,
            other => panic!("not a rules error: {other:?}"),
        }
    };
    assert!(rules_error(&|rules| rules.terrains.clear()).contains("0 terrain types"));
    assert!(
        rules_error(&|rules| rules.terrains = vec![rules.terrains[0]; 257]).contains("257 terrain")
    );
    assert!(rules_error(&|rules| rules.shallow_threshold = 0).contains("threshold"));
    assert!(rules_error(&|rules| rules.sea[3] = 254).contains("sea colour"));
    assert!(rules_error(&|rules| rules.river[3] = 0).contains("river colour"));
    assert!(rules_error(&|rules| rules.terrains[2].solid[3] = 0).contains("terrain 2"));
    assert!(rules_error(&|rules| rules.terrains[2].mix_percent = 101).contains("101 percent"));
    assert!(rules_error(&|rules| rules.terrains[2].relief = [10, 10]).contains("must rise"));
    assert!(rules_error(&|rules| rules.terrains[2].relief = [-11_001, 5]).contains("out of range"));
    assert!(rules_error(&|rules| rules.terrains[6].cap_heights = [900, 500]).contains("must rise"));
    assert!(rules_error(&|rules| rules.terrains[6].cap[3] = 0).contains("terrain 6"));
    assert!(rules_error(&|rules| rules.steps[5] = 0).contains("step 5"));
    // Water ignores the fields it does not read: an unusable style on the sea is not a fault.
    let mut sloppy = rules.clone();
    sloppy.terrains[0].relief = [9, 1];
    assert!(make(ORIGIN, CROP, &grid, &sloppy).is_ok());
}

/// A step, a view, a destination or a source the pass cannot draw never reaches it.
#[test]
fn the_camera_types_refuse_what_the_pass_cannot_draw() {
    let input = input_with(1);
    let map = |step: u8,
               top_left: [i32; 2],
               view: [u32; 2],
               destination: DestinationRect,
               source: SourceRect| {
        VulkanMap::new(
            Arc::clone(&input),
            step,
            top_left,
            view,
            destination,
            source,
        )
    };
    let whole = source(0.0, 0.0, 5.0, 3.0);
    assert!(map(STEP, [0, 0], [5, 3], rect(0, 0, 20, 12), whole).is_ok());
    assert_eq!(
        map(STEP, [0, 0], [0, 3], rect(0, 0, 20, 12), whole).expect_err("refused"),
        VulkanMapError::Size
    );
    assert_eq!(
        map(76, [0, 0], [5, 3], rect(0, 0, 20, 12), whole).expect_err("refused"),
        VulkanMapError::Step(76)
    );
    // A camera whose sample points would not fit 32 bits is refused; at the coarsest step a pixel is 6 cells.
    assert_eq!(
        map(
            0,
            [i32::MAX / 100_000, 0],
            [5, 3],
            rect(0, 0, 20, 12),
            whole
        )
        .expect_err("refused"),
        VulkanMapError::Camera
    );
    assert_eq!(
        map(
            0,
            [0, i32::MIN / 100_000],
            [5, 3],
            rect(0, 0, 20, 12),
            whole
        )
        .expect_err("refused"),
        VulkanMapError::Camera
    );
    assert_eq!(
        map(STEP, [0, 0], [5, 3], rect(0, 0, 0, 12), whole).expect_err("refused"),
        VulkanMapError::Destination
    );
    for bad in [
        source(0.0, 0.0, 5.5, 3.0),
        source(-1.0, 0.0, 5.0, 3.0),
        source(2.0, 0.0, 2.0, 3.0),
        source(0.0, 0.0, f32::NAN, 3.0),
    ] {
        assert_eq!(
            map(STEP, [0, 0], [5, 3], rect(0, 0, 20, 12), bad).expect_err("refused"),
            VulkanMapError::Source,
            "{bad:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The map modes (milestone 9, slice 8)
// ---------------------------------------------------------------------------------------------

/// The fixture's owner grid as the GPU takes it: RGBA8, R and G the high and low bytes of each cell's place, B zero, A 255.
fn owner_image(owners: &[u16]) -> Arc<[u8]> {
    owners
        .iter()
        .flat_map(|index| {
            let [high, low] = index.to_be_bytes();
            [high, low, 0, 255]
        })
        .collect::<Vec<u8>>()
        .into()
}

/// An input with the fixture's owner grid over its four places.
fn moded_input(generation: u64) -> Arc<VulkanMapInput> {
    let fixture = the_fixture();
    Arc::new(
        VulkanMapInput::new(
            generation,
            ORIGIN,
            CROP,
            fixture.grid_a.clone().into(),
            fixture.grid_b.clone().into(),
            &rules_of(fixture),
        )
        .expect("a valid input")
        .with_owners(
            owner_image(&fixture::mode_owners()),
            fixture::MODE_PLACE_COUNT,
        )
        .expect("a valid owner grid"),
    )
}

/// The map of `input` at `step`, showing `values` as revision `revision`.
fn moded_map(
    input: &Arc<VulkanMapInput>,
    scenario: &fixture::Scenario,
    mode: Option<(u64, &[u16])>,
) -> VulkanMap {
    let map = map_at(input, scenario.step, scenario.top_left, scenario.view, 1);
    match mode {
        Some((revision, values)) => map
            .with_mode(revision, values.to_vec().into())
            .expect("a table that fits the places"),
        None => map,
    }
}

/// The map target the reference gives under `values`.
fn moded_reference(scenario: &fixture::Scenario, values: &[u16]) -> Vec<u8> {
    let owners = fixture::mode_owners();
    the_fixture().reference_image_with_mode(
        scenario.step,
        scenario.top_left,
        scenario.view,
        &ModeInputs {
            owners: &owners,
            values,
        },
    )
}

fn drawn_target(
    instance: &ck_gpu_vulkan::Instance,
    scenario: &fixture::Scenario,
    map: &VulkanMap,
) -> Vec<u8> {
    readback_map(
        instance,
        scenario.view[0] * 4,
        scenario.view[1] * 4,
        CLEAR,
        Some(map),
        None,
    )
    .expect("the readback works")
    .target
    .expect("a map target")
}

/// The two views the mode cases draw: one and a half cells per map pixel (the coarse zoom, blocks of the blend) and a tenth of a cell
/// (the fine zoom, all three octaves of sub-cell detail over the blend).
fn mode_scenarios() -> Vec<fixture::Scenario> {
    fixture::Fixture::scenarios()
        .into_iter()
        .filter(|scenario| {
            matches!(
                scenario.name,
                "one and a half cells per pixel" | "a tenth of a cell, inside"
            )
        })
        .collect()
}

/// In each mode, at two zoom steps, the map target equals the reference byte for byte; after a revision of the values (a new table) it
/// equals the reference for the new values, and differs from the old; and with the mode off it equals the plain map the earlier
/// slices drew. The stockpile table comes from stored volumes against capacities (the fourth place has none, so it is uncoloured), the
/// yield table from projected yields relative to each other.
#[test]
fn the_map_modes_equal_the_reference_at_two_zoom_steps_after_a_revision_and_off_is_the_plain_map() {
    let (_loader, instance, log) = rig!();
    let input = moded_input(1);
    let stockpile = vec![
        stockpile_value(0, 1000),
        stockpile_value(500, 1000),
        stockpile_value(900, 900),
        stockpile_value(5, 0),
    ];
    assert_eq!(stockpile, [0, 500, 1000, 0xFFFF]);
    let yields = yield_values(&[Some(40), Some(100), Some(70), None]);
    assert_eq!(yields, [0, 1000, 500, 0xFFFF]);
    let revised = fixture::mode_values(1);
    let scenarios = mode_scenarios();
    assert_eq!(scenarios.len(), 2, "two zoom steps");
    for scenario in &scenarios {
        let plain = the_fixture().reference_image(scenario.step, scenario.top_left, scenario.view);
        let mut seen = Vec::new();
        for (name, revision, values) in [
            ("stockpile", 1, &stockpile),
            ("yield", 2, &yields),
            ("a revision", 3, &revised),
        ] {
            let map = moded_map(&input, scenario, Some((revision, values)));
            let got = drawn_target(&instance, scenario, &map);
            let want = moded_reference(scenario, values);
            let wrong = got
                .chunks_exact(4)
                .zip(want.chunks_exact(4))
                .enumerate()
                .filter(|(_, (got, want))| got != want)
                .map(|(index, (got, want))| {
                    (
                        index % scenario.view[0] as usize,
                        index / scenario.view[0] as usize,
                        got,
                        want,
                    )
                })
                .take(5)
                .collect::<Vec<_>>();
            assert!(
                wrong.is_empty(),
                "{} / {name}: differs from the reference at (x, y, got, want) {wrong:?}",
                scenario.name
            );
            assert_ne!(
                got, plain,
                "{} / {name}: the mode changed the map",
                scenario.name
            );
            seen.push(got);
        }
        assert!(
            seen[0] != seen[1] && seen[1] != seen[2] && seen[0] != seen[2],
            "{}: each table draws its own image",
            scenario.name
        );
        // Off: the same input and view, no mode, is the plain map.
        let map = moded_map(&input, scenario, None);
        assert_eq!(
            drawn_target(&instance, scenario, &map),
            plain,
            "{}: off",
            scenario.name
        );
    }
    assert_log_clean(&log);
    assert_eq!(instance.retained_readbacks(), 0);
    destroy(instance);
    assert_log_clean(&log);
}

/// An input with an owner grid and no mode, and a map whose input has no owner grid, draw the plain map; a table that is not one value
/// per place never reaches the presenter.
#[test]
fn an_input_without_a_mode_draws_the_plain_map_and_a_wrong_table_is_refused() {
    let (_loader, instance, log) = rig!();
    let scenario = mode_scenarios().remove(0);
    let plain = the_fixture().reference_image(scenario.step, scenario.top_left, scenario.view);
    for input in [moded_input(1), input_with(2)] {
        let map = moded_map(&input, &scenario, None);
        assert_eq!(drawn_target(&instance, &scenario, &map), plain);
    }
    let input = moded_input(3);
    for length in [0, 1, 3, 5] {
        let map = map_at(&input, scenario.step, scenario.top_left, scenario.view, 1);
        assert_eq!(
            map.with_mode(1, vec![0; length].into())
                .expect_err("refused"),
            VulkanMapError::ModeTable {
                expected: fixture::MODE_PLACE_COUNT,
                actual: length
            }
        );
    }
    let no_owners = input_with(4);
    let map = map_at(
        &no_owners,
        scenario.step,
        scenario.top_left,
        scenario.view,
        1,
    );
    assert_eq!(
        map.with_mode(1, vec![0; 4].into()).expect_err("refused"),
        VulkanMapError::ModeTable {
            expected: 0,
            actual: 4
        }
    );
    let fixture = the_fixture();
    let short: Arc<[u8]> = vec![255; 8].into();
    assert_eq!(
        VulkanMapInput::new(
            1,
            ORIGIN,
            CROP,
            fixture.grid_a.clone().into(),
            fixture.grid_b.clone().into(),
            &rules_of(fixture),
        )
        .expect("valid")
        .with_owners(short, 4)
        .expect_err("refused"),
        VulkanMapError::OwnerGridLength {
            expected: 24 * 16 * 4,
            actual: 8
        }
    );
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

/// A map with no places has an empty mode table, and a frame that shows it is the plain map: nothing is owned, so nothing is read.
#[test]
fn a_mode_of_a_map_with_no_places_draws_the_plain_map() {
    let (_loader, instance, log) = rig!();
    let fixture = the_fixture();
    let input = Arc::new(
        VulkanMapInput::new(
            1,
            ORIGIN,
            CROP,
            fixture.grid_a.clone().into(),
            fixture.grid_b.clone().into(),
            &rules_of(fixture),
        )
        .expect("a valid input")
        .with_owners(owner_image(&vec![0xFFFF; 24 * 16]), 0)
        .expect("an owner grid of no places"),
    );
    for scenario in &mode_scenarios() {
        let plain = fixture.reference_image(scenario.step, scenario.top_left, scenario.view);
        let map = moded_map(&input, scenario, Some((1, &[])));
        assert_eq!(
            drawn_target(&instance, scenario, &map),
            plain,
            "{}",
            scenario.name
        );
    }
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}
