//! The GPU-written map image (milestone 6, slices 1 and 6), tested as cases of the presenter test target (see `tests/presenter.rs` for
//! how a case runs: in a worker, under Metal validation and the Main Thread Checker, twice, once with the owner typing).
//!
//! The `readback_map_*` cases open no window: they draw through the presenter's own map pass and composition into off-screen textures
//! and read both the map target and the window back. The `map_*` cases start a presenter with a quad slot on a real layer and watch it
//! through the observer's counters and its captured frames.
//!
//! The fixture is `ck-map`'s slice 6 fixture (`crates/map/tests/fixture/mod.rs`, included below): a 24 x 16 crop that exercises every
//! rule of `docs/map-design.md` section 9.4 (rules 1 to 6 and 8). Every expected pixel is `ck_map::reference_colour`'s, the
//! reference the map pass must match byte for byte; slice 1's minimal rule is gone.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ck_gpu_metal::{
    DestinationRect, MapRefusalSink, MetalFilter, MetalMap, MetalMapError, MetalMapInput,
    MetalMapRules, MetalQuad, MetalQuadSlot, MetalScene, MetalTerrainKind, MetalTerrainRule,
    PixelSource, SourceRect, readback_clear, readback_map, readback_quad,
};
use ck_map::{
    FIXED_PER_CELL, ModeInputs, PreparedLocations, PreparedRivers, RiverCoverage, RiverLine,
    STEP_SIZE, ViewSize, WorldPixel, fixture, reference_colour_with_locations, stockpile_value,
    yield_values,
};

use super::{BOUND, CleanupResult, Duration, Probes, Rig, TestHooks, await_presented, end};

// ---------------------------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------------------------

const ORIGIN: [i32; 2] = fixture::ORIGIN;
const CROP: [u32; 2] = fixture::SIZE;

/// The step of the cases that do not choose one: a tenth of a cell per map pixel, so the cases see all three octaves of the sub-cell
/// detail and about ten map pixels to a cell.
const STEP: u8 = 60;

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

fn kind_of(kind: fixture::Kind) -> MetalTerrainKind {
    match kind {
        fixture::Kind::Land => MetalTerrainKind::Land,
        fixture::Kind::Sea => MetalTerrainKind::Sea,
        fixture::Kind::Lake => MetalTerrainKind::Lake,
        fixture::Kind::Mountain => MetalTerrainKind::Mountain,
    }
}

/// The fixture's rule table as the presenter takes it.
fn rules_of(fixture: &fixture::Fixture) -> MetalMapRules {
    MetalMapRules {
        terrains: fixture
            .terrains
            .iter()
            .map(|terrain| {
                let shades = ck_map::look::TerrainShades::of(&terrain.style);
                MetalTerrainRule {
                    kind: kind_of(terrain.kind),
                    tone: shades.tone.map(|tone| tone.0),
                    shadow: shades.shadow.map(|tone| tone.0),
                    lit: shades.lit.map(|tone| tone.0),
                }
            })
            .collect(),
        sea: fixture.data.sea.0,
        shallow_sea: fixture.data.shallow_sea.0,
        lake: fixture.data.lake.0,
        river: fixture.data.river.0,
        ice: fixture.data.ice.0,
        ice_second: fixture.data.ice_second.0,
        overlay_palette: Vec::new(),
        shallow_threshold: fixture.data.shallow_threshold,
        steps: STEP_SIZE,
    }
}

fn input_of(fixture: &fixture::Fixture, generation: u64) -> Arc<MetalMapInput> {
    Arc::new(
        MetalMapInput::new(
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

fn input_with(generation: u64) -> Arc<MetalMapInput> {
    input_of(the_fixture(), generation)
}

fn input_with_palette(generation: u64, colour: [u8; 4]) -> Arc<MetalMapInput> {
    let fixture = the_fixture();
    let mut rules = rules_of(fixture);
    rules.overlay_palette = vec![colour];
    Arc::new(
        MetalMapInput::new(
            generation,
            ORIGIN,
            CROP,
            fixture.grid_a.clone().into(),
            fixture.grid_b.clone().into(),
            &rules,
        )
        .expect("a valid one-colour overlay input"),
    )
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

/// A value from 0 to 255, rounded, as a byte (the clear colour as the 8-bit values the target holds).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255 and rounded just before"
)]
fn byte(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

fn float(value: u32) -> f32 {
    f32::from(u16::try_from(value).expect("a small number"))
}

fn signed(value: u32) -> i32 {
    i32::try_from(value).expect("a small number")
}

/// The map of `input` seen from `top_left` through a `view`, enlarged by `scale` into the top-left of the drawable: the whole
/// `view x 4 x scale` rectangle.
fn map_of(input: &Arc<MetalMapInput>, top_left: [i32; 2], view: [u32; 2], scale: u32) -> MetalMap {
    map_at(input, STEP, top_left, view, scale)
}

fn map_with_palette(input: &Arc<MetalMapInput>, top_left: [i32; 2], view: [u32; 2]) -> MetalMap {
    MetalMap::new_with_overlays(
        Arc::clone(input),
        STEP,
        top_left,
        view,
        rect(0, 0, signed(view[0] * 4), signed(view[1] * 4)),
        source(0.0, 0.0, float(view[0]), float(view[1])),
        Vec::new(),
        vec![[0, 0, 0]],
    )
    .expect("a valid map with one visible palette pixel")
}

/// [`map_of`] at a chosen zoom step.
fn map_at(
    input: &Arc<MetalMapInput>,
    step: u8,
    top_left: [i32; 2],
    view: [u32; 2],
    scale: u32,
) -> MetalMap {
    let (width, height) = (view[0] * 4 * scale, view[1] * 4 * scale);
    MetalMap::new(
        Arc::clone(input),
        step,
        top_left,
        view,
        rect(0, 0, signed(width), signed(height)),
        source(0.0, 0.0, float(view[0]), float(view[1])),
    )
    .expect("a valid map")
}

/// The reference's RGBA colour of map pixel `(x, y)` of a view at `top_left`, at [`STEP`].
fn expected_pixel(top_left: [i32; 2], x: u32, y: u32) -> [u8; 4] {
    the_fixture().reference(
        STEP,
        WorldPixel {
            x: top_left[0] + signed(x),
            y: top_left[1] + signed(y),
        },
    )
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

/// The map target the reference gives, RGBA8 rows top first.
fn expected_target(top_left: [i32; 2], view: [u32; 2]) -> Vec<u8> {
    the_fixture().reference_image(STEP, top_left, view)
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

/// The clear colour the presenter of these cases draws (the rig starts it at (0.1, 0.2, 0.3)) as the 8-bit RGBA the drawable holds.
const PRESENTER_CLEAR: [u8; 4] = [26, 51, 77, 255];

// ---------------------------------------------------------------------------------------------
// Off-screen: the production map pass and composition
// ---------------------------------------------------------------------------------------------

/// The map target equals the reference colour function byte for byte on every scenario of the fixture: steps coarser than a cell and
/// finer (the three octaves of sub-cell detail), views overlapping the crop's edge and lying wholly outside it, odd view sizes, the
/// coast, the lake, the shelf, the mountains with their caps and high ground. The last scenario is a view again after one far away: a
/// pan away and back, with identical bytes.
pub(crate) fn readback_map_full_rules() {
    let fixture = the_fixture();
    let input = input_with(1);
    let mut first_visit: Option<Vec<u8>> = None;
    for scenario in fixture::Fixture::scenarios() {
        let map = map_at(&input, scenario.step, scenario.top_left, scenario.view, 1);
        let drawn = readback_map(
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
}

/// Prepared river pixels replace the reference byte-for-byte on two steps; the left view edge cuts a river line.
pub(crate) fn readback_map_rivers_overlay() {
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
        let map = MetalMap::new_with_rivers(
            Arc::clone(&input),
            step,
            top_left,
            view,
            rect(0, 0, signed(view[0] * 4), signed(view[1] * 4)),
            source(0.0, 0.0, float(view[0]), float(view[1])),
            rivers.clone(),
        )
        .expect("a valid river map");
        let drawn = readback_map(view[0] * 4, view[1] * 4, CLEAR, Some(&map), None)
            .expect("the river readback works");
        let mut expected = fixture.reference_image(step, top_left, view);
        for [x, y] in rivers {
            let offset = usize::try_from((y * view[0] + x) * 4).expect("small fixture");
            expected[offset..offset + 4].copy_from_slice(&fixture.data.river.0);
        }
        assert_eq!(drawn.target.expect("a map target"), expected, "step {step}");
    }
}

/// Palette-indexed locations are drawn between terrain and rivers, and an empty location list preserves the original image.
#[expect(
    clippy::too_many_lines,
    reason = "keep the two-step readback, pan control and empty-list control together"
)]
pub(crate) fn readback_map_location_overlays() {
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
        MetalMapInput::new(
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
        // A stable second capture pans away and back using the same selected pixels.
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
                let world = WorldPixel {
                    x: top_left[0] + signed(x),
                    y: top_left[1] + signed(y),
                };
                reference_colour_with_locations(
                    &fixture.data,
                    step,
                    world,
                    &river_pixels,
                    &colour_by_pixel,
                )
                .expect("reference colour")
                .0
            })
            .collect::<Vec<_>>();
        let with_overlays = MetalMap::new_with_overlays(
            Arc::clone(&input),
            step,
            top_left,
            view,
            rect(0, 0, signed(view[0] * 4), signed(view[1] * 4)),
            source(0.0, 0.0, float(view[0]), float(view[1])),
            rivers.clone(),
            locations.clone(),
        )
        .expect("valid overlay map");
        let drawn = readback_map(view[0] * 4, view[1] * 4, CLEAR, Some(&with_overlays), None)
            .expect("the overlay readback works");
        let target = drawn.target.expect("map target");
        assert_eq!(target, expected, "location/rivers reference at step {step}");
        let far = MetalMap::new_with_rivers(
            Arc::clone(&input),
            step,
            [top_left[0] + 5_000, top_left[1] + 5_000],
            view,
            rect(0, 0, signed(view[0] * 4), signed(view[1] * 4)),
            source(0.0, 0.0, float(view[0]), float(view[1])),
            Vec::new(),
        )
        .expect("far-away pan is a valid camera");
        let _ = readback_map(view[0] * 4, view[1] * 4, CLEAR, Some(&far), None)
            .expect("the far-away pan readback works");
        let back = readback_map(view[0] * 4, view[1] * 4, CLEAR, Some(&with_overlays), None)
            .expect("the return pan readback works");
        assert_eq!(back.target.expect("returned map target"), target);
        let empty = MetalMap::new_with_overlays(
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
        let baseline = readback_map(view[0] * 4, view[1] * 4, CLEAR, Some(&empty), None)
            .expect("the empty control readback works");
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

/// Encode a local river fixture at two zoom steps through Metal's offscreen map pass. Both maps share one input generation, just as
/// the resident-grid zoom path does, while each step carries its newly selected river vector.
pub(crate) fn readback_map_ireland_rivers_after_zoom() {
    // Keep this crash regression independent of the shipped map assets: the GPU-metal crate owns
    // its small input and river line, while the live rule table still supplies the river colour.
    let (input, crop, river, river_center, river_colour) = metal_zoom_fixture();
    let counts = [
        readback_zoom_step(&input, &crop, &river, river_colour, 56, river_center),
        readback_zoom_step(&input, &crop, &river, river_colour, 55, river_center),
    ];
    assert_ne!(
        counts[0], counts[1],
        "the zoom change selects a new river buffer size"
    );
}

fn metal_zoom_fixture() -> (
    Arc<MetalMapInput>,
    ck_map::Crop,
    RiverLine,
    ck_map::FixedPoint,
    [u8; 4],
) {
    const ORIGIN: [i32; 2] = [24_100, 14_700];
    const SIZE: [u32; 2] = [24, 16];
    const CELLS: usize = SIZE[0] as usize * SIZE[1] as usize;
    const FIXED_PER_CELL: i32 = 32_768;
    let river_colour = [0x3a, 0x78, 0xc2, 0xff];
    let rules = MetalMapRules {
        terrains: vec![MetalTerrainRule {
            kind: MetalTerrainKind::Land,
            tone: [
                [0x10, 0x20, 0x10, 0xff],
                [0x10, 0x20, 0x10, 0xff],
                [0x30, 0x60, 0x20, 0xff],
                [0x30, 0x60, 0x20, 0xff],
            ],
            shadow: [[0x30, 0x60, 0x20, 0xff]; 3],
            lit: [[0x10, 0x20, 0x10, 0xff]; 3],
        }],
        sea: [0x05, 0x08, 0x0e, 0xff],
        shallow_sea: [0x0b, 0x14, 0x22, 0xff],
        lake: [0x0b, 0x14, 0x22, 0xff],
        river: river_colour,
        ice: [0xe8, 0xe9, 0xed, 0xff],
        ice_second: [0xb9, 0xbc, 0xc6, 0xff],
        overlay_palette: Vec::new(),
        shallow_threshold: 40,
        steps: STEP_SIZE,
    };
    let mut grid_a = Vec::with_capacity(CELLS * 4);
    let mut grid_b = Vec::with_capacity(CELLS * 4);
    for _ in 0..CELLS {
        grid_a.extend_from_slice(&[0, 1, 128, 255]);
        grid_b.extend_from_slice(&[0x2a, 0xf8, 0, 255]); // 11,000 metres encoded as metres + 11,000.
    }
    let input = Arc::new(
        MetalMapInput::new(184, ORIGIN, SIZE, grid_a.into(), grid_b.into(), &rules)
            .expect("the local zoom fixture has valid grids and rules"),
    );
    let crop = ck_map::Crop {
        origin: ck_map::Cell {
            x: ORIGIN[0],
            y: ORIGIN[1],
        },
        width: SIZE[0],
        height: SIZE[1],
    };
    let fixed = |x: i32, y: i32| ck_map::FixedPoint {
        x: x * FIXED_PER_CELL,
        y: y * FIXED_PER_CELL,
    };
    let river = RiverLine {
        points: vec![
            fixed(ORIGIN[0] + 2, ORIGIN[1] + 8),
            fixed(ORIGIN[0] + 22, ORIGIN[1] + 8),
        ],
    };
    let river_center = fixed(ORIGIN[0] + 12, ORIGIN[1] + 8);
    (input, crop, river, river_center, river_colour)
}

fn readback_zoom_step(
    input: &Arc<MetalMapInput>,
    crop: &ck_map::Crop,
    river: &RiverLine,
    river_colour: [u8; 4],
    step: u8,
    river_center: ck_map::FixedPoint,
) -> usize {
    let view = [320_u32, 240_u32];
    let view_size = ViewSize {
        width: view[0],
        height: view[1],
    };
    let coverage =
        RiverCoverage::prepare(std::slice::from_ref(river), step).expect("valid zoom step");
    assert!(
        !coverage.pixels().is_empty(),
        "the local fixture has river coverage at step {step}"
    );
    let top_left = ck_map::Camera::new(step, river_center, view_size, *crop)
        .expect("camera fits the local zoom fixture")
        .top_left();
    let rivers = coverage
        .select(top_left, view_size)
        .into_iter()
        .map(|pixel| {
            [
                u32::try_from(i64::from(pixel.x) - i64::from(top_left.x))
                    .expect("river x is visible"),
                u32::try_from(i64::from(pixel.y) - i64::from(top_left.y))
                    .expect("river y is visible"),
            ]
        })
        .collect::<Vec<_>>();
    assert!(
        !rivers.is_empty(),
        "the zoomed fixture view includes rivers at step {step}"
    );
    let map = MetalMap::new_with_rivers(
        Arc::clone(input),
        step,
        [top_left.x, top_left.y],
        view,
        rect(0, 0, 320, 240),
        source(0.0, 0.0, 320.0, 240.0),
        rivers.clone(),
    )
    .expect("the zoomed fixture map is valid");
    let drawn = readback_map(320, 240, CLEAR, Some(&map), None)
        .expect("Metal encodes the full river buffer at this zoom");
    let target = drawn.target.expect("the map pass writes its target");
    for [x, y] in &rivers {
        let offset = usize::try_from((y * view[0] + x) * 4).expect("view pixel fits");
        assert_eq!(
            &target[offset..offset + 4],
            &river_colour,
            "the new step's river pixel is drawn at ({x}, {y})"
        );
    }
    rivers.len()
}

/// Negative control: with one rule table entry deliberately wrong (pasture's second tone another colour) the readback changes in
/// exactly the pixels that show a tone made from it (the three shadow-side tones), and in no other.
pub(crate) fn readback_map_negative_control() {
    let fixture = the_fixture();
    let wrong = fixture.with_other_pasture_second_tone();
    let input = input_of(&wrong, 1);
    let shadow = ck_map::look::TerrainShades::of(&fixture.terrains[2].style)
        .shadow
        .map(|tone| tone.0);
    let mut changed = 0;
    for scenario in fixture::Fixture::scenarios() {
        let map = map_at(&input, scenario.step, scenario.top_left, scenario.view, 1);
        let drawn = readback_map(
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
                shadow.iter().any(|tone| want == tone),
                "{}: pixel {index} (reference {want:?}, drawn {got:?})",
                scenario.name
            );
            changed += usize::from(got != want);
        }
    }
    assert!(changed > 0, "the wrong entry changed something");
}

/// A view that is not the crop's shape (5 x 3 over 7 x 5) and one larger than the crop (9 x 7) read the same cells.
pub(crate) fn readback_map_view_sizes() {
    let input = input_with(1);
    for view in [[1, 1], [5, 3], [9, 7], [7, 5], [2, 9]] {
        let top_left = at(-1, -1);
        let map = map_of(&input, top_left, view, 1);
        let drawn = readback_map(view[0] * 4, view[1] * 4, CLEAR, Some(&map), None)
            .expect("the readback works");
        assert_eq!(
            drawn.target.expect("a map target"),
            expected_target(top_left, view),
            "view {view:?}"
        );
    }
}

/// The enlarged map: at scales 1 and 2 every drawable pixel equals the map pixel whose point rectangle contains its centre. A
/// window smaller than `view x 4` points crops the partial right and bottom map pixel, never stretches it.
pub(crate) fn readback_map_enlarged() {
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
        let drawn =
            readback_map(width, height, CLEAR, Some(&map), None).expect("the readback works");
        for py in 0..height {
            for px in 0..width {
                // The centre of the pixel, in points: (p + 1/2) / scale; the map pixel is four points wide.
                let (mx, my) = (px / (4 * scale), py / (4 * scale));
                assert_eq!(
                    bgra_at(&drawn.window, width, px, py),
                    map_pixel(mx, my),
                    "scale {scale}: drawable pixel ({px}, {py}) is map pixel ({mx}, {my})"
                );
            }
        }
    }
    // A window of 18 x 10 pixels (scale 1) is 4.5 x 2.5 map pixels: the destination is clipped to the window and the source
    // trimmed in proportion, so every pixel still lies in the map pixel that contains it.
    let cropped = MetalMap::new(
        Arc::clone(&input),
        STEP,
        top_left,
        view,
        rect(0, 0, 18, 10),
        source(0.0, 0.0, 4.5, 2.5),
    )
    .expect("a valid map");
    let drawn = readback_map(24, 16, CLEAR, Some(&cropped), None).expect("the readback works");
    let clear = [
        byte(CLEAR[0] * 255.0),
        byte(CLEAR[1] * 255.0),
        byte(CLEAR[2] * 255.0),
        255,
    ];
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
}

/// The map is drawn between the clear and the legacy quad: a quad over part of the enlarged map hides it there, and the map shows
/// everywhere else.
pub(crate) fn readback_map_below_the_legacy_quad() {
    struct Solid(Vec<u8>);
    impl PixelSource for Solid {
        fn pixels(&self) -> &[u8] {
            &self.0
        }
    }
    let input = input_with(1);
    let view = [5, 3];
    let top_left = corner();
    let map = map_of(&input, top_left, view, 1);
    let picture: Arc<dyn PixelSource> = Arc::new(Solid([255, 255, 0, 255].repeat(4)));
    let quad = MetalQuad::new(
        picture,
        2,
        2,
        rect(4, 4, 12, 8),
        source(0.0, 0.0, 2.0, 2.0),
        MetalFilter::Nearest,
    )
    .expect("a valid quad");
    let drawn = readback_map(20, 12, CLEAR, Some(&map), Some(&quad)).expect("the readback works");
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
}

/// Negative control: with no map the frame is exactly the frame without one, whatever else it draws.
pub(crate) fn readback_map_none_is_the_frame_without_a_map() {
    struct Solid(Vec<u8>);
    impl PixelSource for Solid {
        fn pixels(&self) -> &[u8] {
            &self.0
        }
    }
    let drawn = readback_map(16, 8, CLEAR, None, None).expect("the readback works");
    assert!(drawn.target.is_none());
    assert_eq!(
        drawn.window,
        readback_clear(16, 8, CLEAR).expect("the readback works"),
        "the clear alone"
    );
    let picture: Arc<dyn PixelSource> = Arc::new(Solid([9, 90, 190, 255, 1, 2, 3, 255].repeat(2)));
    let quad = MetalQuad::new(
        picture,
        2,
        2,
        rect(2, 1, 10, 7),
        source(0.0, 0.0, 2.0, 2.0),
        MetalFilter::Nearest,
    )
    .expect("a valid quad");
    let with_quad = readback_map(16, 8, CLEAR, None, Some(&quad)).expect("the readback works");
    assert_eq!(
        with_quad.window,
        readback_quad(16, 8, CLEAR, Some(&quad)).expect("the readback works"),
        "the scene-only result"
    );
}

/// What the lane's refusals are: a map the GPU would be handed in a shape it cannot read never reaches it.
pub(crate) fn readback_map_refusals() {
    let fixture = the_fixture();
    let grid: Arc<[u8]> = vec![0; 24 * 16 * 4].into();
    let rules = rules_of(fixture);
    let make = |origin: [i32; 2], size: [u32; 2], grid_b: &Arc<[u8]>, rules: &MetalMapRules| {
        MetalMapInput::new(
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
        MetalMapError::Size
    );
    assert_eq!(
        make(ORIGIN, [25, 16], &grid, &rules).expect_err("refused"),
        MetalMapError::GridLength {
            expected: 25 * 16 * 4,
            actual: 24 * 16 * 4
        }
    );
    let short: Arc<[u8]> = vec![0; 8].into();
    assert_eq!(
        make(ORIGIN, CROP, &short, &rules).expect_err("refused"),
        MetalMapError::GridLength {
            expected: 24 * 16 * 4,
            actual: 8
        }
    );
    // The crop must lie in the non-negative world and leave room for a map pixel past its far edge.
    for origin in [[-1, 0], [0, -1], [i32::MAX / 32_768, 0], [0, 65_500]] {
        assert_eq!(
            make(origin, CROP, &grid, &rules).expect_err("refused"),
            MetalMapError::Crop,
            "{origin:?}"
        );
    }
    // The rule table must be one the shader can read.
    let rules_error = |change: &dyn Fn(&mut MetalMapRules)| {
        let mut broken = rules.clone();
        change(&mut broken);
        match make(ORIGIN, CROP, &grid, &broken).expect_err("refused") {
            MetalMapError::Rules(reason) => reason,
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
    assert!(rules_error(&|rules| rules.terrains[2].tone[3][3] = 0).contains("terrain 2"));
    assert!(rules_error(&|rules| rules.terrains[2].lit[0][3] = 0).contains("terrain 2"));
    assert!(rules_error(&|rules| rules.terrains[6].shadow[2][3] = 0).contains("terrain 6"));
    assert!(rules_error(&|rules| rules.ice[3] = 0).contains("ice colour"));
    assert!(rules_error(&|rules| rules.ice_second[3] = 0).contains("ice second colour"));
    assert!(rules_error(&|rules| rules.steps[5] = 0).contains("step 5"));
    // Water ignores the fields it does not read: a style with no opaque colours on the sea is not a fault.
    let mut sloppy = rules.clone();
    sloppy.terrains[0].tone[3] = [0; 4];
    assert!(make(ORIGIN, CROP, &grid, &sloppy).is_ok());
}

/// What the camera's refusals are: a step, a view, a destination or a source the pass cannot draw never reaches it.
pub(crate) fn readback_map_camera_refusals() {
    let input = input_with(1);
    let map = |step: u8,
               top_left: [i32; 2],
               view: [u32; 2],
               destination: DestinationRect,
               source: SourceRect| {
        MetalMap::new(
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
        MetalMapError::Size
    );
    assert_eq!(
        map(76, [0, 0], [5, 3], rect(0, 0, 20, 12), whole).expect_err("refused"),
        MetalMapError::Step(76)
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
        MetalMapError::Camera
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
        MetalMapError::Camera
    );
    assert_eq!(
        map(STEP, [0, 0], [5, 3], rect(0, 0, 0, 12), whole).expect_err("refused"),
        MetalMapError::Destination
    );
    for bad in [
        source(0.0, 0.0, 5.5, 3.0),
        source(-1.0, 0.0, 5.0, 3.0),
        source(2.0, 0.0, 2.0, 3.0),
        source(0.0, 0.0, f32::NAN, 3.0),
    ] {
        assert_eq!(
            map(STEP, [0, 0], [5, 3], rect(0, 0, 20, 12), bad).expect_err("refused"),
            MetalMapError::Source,
            "{bad:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// On a real layer: the presenter's own frames
// ---------------------------------------------------------------------------------------------

/// A slot, and the hooks of a presenter started with it that keeps the frames it draws.
fn hooks_with(probes: &Probes, slot: &Arc<MetalQuadSlot>) -> TestHooks {
    TestHooks {
        quads: Some(Arc::clone(slot)),
        capture_frames: true,
        ..probes.hooks()
    }
}

fn publish(slot: &MetalQuadSlot, generation: u64, map: MetalMap) {
    slot.publish(MetalScene::new(generation, None).with_map(Some(map)));
}

/// The first frame has the map: the presenter makes the input and the target resident, runs the map pass, and its window is the
/// enlarged map over the clear.
pub(crate) fn map_in_the_first_frame() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let input = input_with(1);
    let top_left = at(-1, 0);
    publish(&slot, 7, map_of(&input, top_left, [5, 3], 1));
    let mut presenter = probes.start_ok(&rig, hooks_with(&probes, &slot));
    assert!(
        rig.pump_until(BOUND, || presenter.drawn_generation() == Some(7)),
        "the first frame has the map"
    );
    let observer = &probes.observer;
    assert_eq!(probes.frames(), 1, "it was the first and only frame");
    assert_eq!(observer.map_passes(), 1);
    assert_eq!(observer.map_grids_created(), 1);
    assert_eq!(observer.map_targets_created(), 1);
    assert_eq!(observer.map_last_target_size(), (5, 3));
    // What the presenter drew, captured by the same encoding: the enlarged map in the drawable's top-left corner.
    assert!(rig.pump_until(BOUND, || observer.captures() >= 1));
    let captured = observer.captured_frame().expect("a captured frame");
    let target = expected_target(top_left, [5, 3]);
    for py in 0..12 {
        for px in 0..20 {
            let offset = usize::try_from(((py / 4) * 5 + px / 4) * 4).expect("small");
            assert_eq!(
                bgra_at(&captured.pixels, captured.width, px, py),
                [
                    target[offset],
                    target[offset + 1],
                    target[offset + 2],
                    target[offset + 3]
                ],
                "({px}, {py})"
            );
        }
    }
    await_presented(&mut rig, observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(observer.map_targets_alive(), 0);
    assert_eq!(observer.map_grids_alive(), 0);
}

/// A frame with no map is the frame without one: no input, no target, no pass; publishing a map later starts them.
pub(crate) fn map_absent_then_present() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let mut presenter = probes.start_ok(&rig, hooks_with(&probes, &slot));
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let observer = &probes.observer;
    assert_eq!(
        (
            observer.map_passes(),
            observer.map_grids_created(),
            observer.map_targets_created()
        ),
        (0, 0, 0),
        "no map, no map resource"
    );
    let input = input_with_palette(1, [210, 20, 30, 255]);
    publish(&slot, 3, map_with_palette(&input, corner(), [5, 3]));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(3)));
    assert_eq!(observer.map_passes(), 1);
    // And removing it again stops the pass but keeps the resident resources for the next frame with a map.
    slot.publish(MetalScene::new(4, None));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(4)));
    assert_eq!(observer.map_passes(), 1, "no map, no pass");
    assert_eq!(observer.map_targets_alive(), 1, "the target is kept");
    publish(&slot, 5, map_with_palette(&input, at(1, 0), [5, 3]));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(5)));
    assert_eq!(observer.map_passes(), 2);
    assert_eq!(
        observer.map_grids_created(),
        1,
        "the same generation is not uploaded again"
    );
    assert_eq!(
        observer.map_targets_created(),
        1,
        "and the same size is not reallocated"
    );
    assert_eq!(
        observer.map_palettes_created(),
        1,
        "the input palette is reused across frames"
    );
    assert_eq!(observer.map_palettes_alive(), 1);
    await_presented(&mut rig, observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A resize replaces the target and a new input replaces the images; the old ones are retired and let go of only after the proof
/// of the last command buffer that used them, which this case orders by a held completion, not by elapsed time.
#[expect(
    clippy::too_many_lines,
    reason = "keep generation replacement and its completion-gated palette witness together"
)]
pub(crate) fn map_resize_retires_after_the_proof() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let gate = Arc::new(AtomicI32::new(0));
    let hooks = TestHooks {
        frame_completion_gate: Some(Arc::clone(&gate)),
        capture_frames: true,
        ..hooks_with(&probes, &slot)
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    let observer = &probes.observer;
    let input = input_with_palette(1, [210, 20, 30, 255]);
    publish(&slot, 1, map_with_palette(&input, corner(), [5, 3]));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(1)));
    assert_eq!(
        (
            observer.map_targets_created(),
            observer.map_retired(),
            observer.map_targets_alive()
        ),
        (1, 0, 1)
    );
    assert_eq!(observer.map_palettes_created(), 1);
    assert_eq!(observer.map_palettes_alive(), 1);
    // The view grows while the first frame's command buffer is still outstanding: the step finds it so, and makes nothing.
    let steps = observer.frame_steps();
    publish(&slot, 2, map_with_palette(&input, corner(), [6, 4]));
    assert!(
        rig.pump_until(BOUND, || observer.frame_steps() > steps),
        "a step ran while the completion was held"
    );
    assert_eq!(
        observer.completion_handlers_run(),
        0,
        "the first frame's completion is held"
    );
    assert_eq!(
        observer.map_targets_created(),
        1,
        "no second target while the first is in use"
    );
    assert_eq!(
        observer.map_palettes_created(),
        1,
        "no new palette while the old frame is in flight"
    );
    assert_eq!(
        observer.map_palettes_alive(),
        1,
        "the old palette remains held"
    );
    assert_eq!(observer.map_retired(), 0);
    gate.store(1, Ordering::SeqCst);
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(2)));
    assert!(rig.pump_until(BOUND, || observer.captures() >= 2));
    assert_eq!(
        first_map_pixel(
            &observer
                .captured_frame_at(0)
                .expect("first captured generation")
        ),
        [210, 20, 30, 255],
        "the first generation binds its resident palette"
    );
    assert_eq!(observer.map_targets_created(), 2);
    assert_eq!(observer.map_last_target_size(), (6, 4));
    assert_eq!(observer.map_retired(), 1, "the old target was retired");
    assert_eq!(
        observer.map_retired_released(),
        1,
        "and let go of, its command buffer's completion having arrived"
    );
    assert_eq!(observer.map_targets_alive(), 1);
    assert_eq!(observer.map_released_early(), 0);
    // A new input (another generation) replaces the images the same way.
    let replacement = input_with_palette(2, [30, 40, 220, 255]);
    let captures_before_replacement = observer.captures();
    publish(&slot, 3, map_with_palette(&replacement, corner(), [6, 4]));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(3)));
    assert!(rig.pump_until(BOUND, || observer.captures() > captures_before_replacement));
    assert_eq!(
        first_map_pixel(
            &observer
                .captured_frame()
                .expect("replacement palette capture")
        ),
        [30, 40, 220, 255],
        "the replacement generation binds its changed palette"
    );
    assert_eq!(observer.map_grids_created(), 2);
    assert_eq!(
        observer.map_palettes_created(),
        2,
        "a changed generation uploads its changed palette"
    );
    assert_eq!(
        observer.map_palettes_released(),
        1,
        "the old palette waited for its completion proof"
    );
    assert_eq!(observer.map_palettes_alive(), 1);
    assert_eq!(observer.map_targets_created(), 2, "the view did not change");
    assert_eq!(observer.map_retired(), 2);
    assert_eq!(observer.map_retired_released(), 2);
    assert_eq!(observer.map_grids_alive(), 1);
    await_presented(&mut rig, observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(observer.map_targets_alive(), 0);
    assert_eq!(observer.map_grids_alive(), 0);
    assert_eq!(observer.map_palettes_alive(), 0);
    assert_eq!(observer.map_released_early(), 0);
}

/// What the presenter held of the map's targets at an instant: targets made, targets alive, objects retired, retired objects let go of.
type Residency = [i64; 4];

/// What the presenter told the publisher: the messages of its refusals and how often an attempt succeeded after one, and, when a case
/// watches the presenter, what it held at the instant of each call (the calls come from the render thread, after the native step).
#[derive(Default)]
struct Told {
    refused: Mutex<Vec<String>>,
    cleared: AtomicUsize,
    reader: Mutex<Option<Box<dyn Fn() -> Residency + Send>>>,
    /// What the presenter held when each refusal was told.
    at_refused: Mutex<Vec<Residency>>,
    /// What the presenter held when each success after a refusal was told.
    at_cleared: Mutex<Vec<Residency>>,
}

impl Told {
    /// Reads the presenter's residency from `observer` inside each call from now on.
    fn watch(&self, observer: &Arc<ck_gpu_metal::Observer>) {
        let observer = Arc::clone(observer);
        *self.reader.lock().expect("the reader") = Some(Box::new(move || {
            [
                observer.map_targets_created(),
                observer.map_targets_alive(),
                observer.map_retired(),
                observer.map_retired_released(),
            ]
        }));
    }

    fn held(&self) -> Option<Residency> {
        self.reader
            .lock()
            .expect("the reader")
            .as_ref()
            .map(|read| read())
    }
}

impl MapRefusalSink for Told {
    fn refused(&self, message: &str) {
        if let Some(held) = self.held() {
            self.at_refused.lock().expect("the snapshots").push(held);
        }
        self.refused
            .lock()
            .expect("the messages")
            .push(message.to_owned());
    }

    fn cleared(&self) {
        if let Some(held) = self.held() {
            self.at_cleared.lock().expect("the snapshots").push(held);
        }
        self.cleared.fetch_add(1, Ordering::SeqCst);
    }
}

/// The pixel at (1, 1) of a captured frame (BGRA8), as RGBA: inside the enlarged map's first map pixel.
fn first_map_pixel(frame: &ck_gpu_metal::CapturedFrame) -> [u8; 4] {
    bgra_at(&frame.pixels, frame.width, 1, 1)
}

/// A refused allocation is not a failure of the run: the frame is drawn without the map (never with a stale target), the publisher
/// is told, and the renderer keeps a redraw wanted by itself, so the next normally paced frame asks again with nothing republished;
/// the success is told too, and the idle resumes.
pub(crate) fn map_a_refused_allocation_is_told_retried_and_cleared() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let told = Arc::new(Told::default());
    let hooks = TestHooks {
        // The first attempt to make the map's images is refused.
        fail_map_allocations: 1,
        ..hooks_with(&probes, &slot)
    };
    let input = input_with(1);
    let with_refusals =
        |map: MetalMap| map.with_refusals(Arc::clone(&told) as Arc<dyn MapRefusalSink>);
    publish(&slot, 1, with_refusals(map_of(&input, corner(), [5, 3], 1)));
    // Watching starts before the presenter does: the first frame may already be refused when the start returns.
    told.watch(&probes.observer);
    let mut presenter = probes.start_ok(&rig, hooks);
    let observer = &probes.observer;
    // Nothing is republished: the retry frame is the renderer's own.
    assert!(
        rig.pump_until(BOUND, || told.cleared.load(Ordering::SeqCst) == 1),
        "the renderer retried by itself and the success was told"
    );
    assert_eq!(told.refused.lock().expect("the messages").len(), 1);
    assert!(
        told.refused.lock().expect("the messages")[0].contains("grid A"),
        "{:?}",
        told.refused.lock().expect("the messages")
    );
    assert_eq!(
        *told.at_refused.lock().expect("the snapshots"),
        [[0, 0, 0, 0]],
        "at the refusal nothing of the map was made"
    );
    assert_eq!(
        (
            observer.map_passes(),
            observer.map_grids_created(),
            observer.map_targets_created()
        ),
        (1, 1, 1),
        "the refused frame made and drew nothing of the map; the retry made it once"
    );
    // The first frame was drawn without the map, the second with it.
    assert!(rig.pump_until(BOUND, || observer.captures() >= 2));
    let without = observer.captured_frame_at(0).expect("the first frame");
    let with = observer.captured_frame_at(1).expect("the retry frame");
    assert_eq!(
        first_map_pixel(&without),
        PRESENTER_CLEAR,
        "no map, the clear"
    );
    assert_eq!(
        first_map_pixel(&with),
        expected_pixel(corner(), 0, 0),
        "the map"
    );
    // The idle resumes: no more frames, and the success is not told again.
    rig.pump_for(Duration::from_millis(150));
    assert_eq!(
        probes.frames(),
        2,
        "one refused frame and one retry, no loop"
    );
    assert_eq!(told.cleared.load(Ordering::SeqCst), 1);
    await_presented(&mut rig, observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A palette refusal is reached at generation admission, preserves the current generation and retries without another publication.
pub(crate) fn map_overlay_palette_refusal_retries_without_replacing_the_old_generation() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let told = Arc::new(Told::default());
    let hooks = TestHooks {
        fail_map_palette_allocations: 1,
        ..hooks_with(&probes, &slot)
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    let observer = &probes.observer;
    let first = input_with(1);
    publish(&slot, 1, map_of(&first, corner(), [5, 3], 1));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(1)));
    assert_eq!(observer.map_grids_created(), 1);

    let second = input_with_palette(2, [30, 40, 220, 255]);
    let map = map_with_palette(&second, corner(), [5, 3])
        .with_refusals(Arc::clone(&told) as Arc<dyn MapRefusalSink>);
    told.watch(observer);
    publish(&slot, 2, map);
    assert!(rig.pump_until(BOUND, || told.cleared.load(Ordering::SeqCst) == 1));
    assert_eq!(told.refused.lock().expect("the messages").len(), 1);
    assert!(
        told.refused.lock().expect("the messages")[0].contains("overlay-palette"),
        "{:?}",
        told.refused.lock().expect("the messages")
    );
    assert_eq!(
        *told.at_refused.lock().expect("the snapshots"),
        [[1, 1, 0, 0]],
        "the prior generation remains resident when palette allocation is refused"
    );
    assert_eq!(
        observer.map_grids_created(),
        2,
        "the retry admits the new generation"
    );
    assert_eq!(observer.map_palettes_created(), 1);
    assert_eq!(observer.map_palettes_alive(), 1);
    await_presented(&mut rig, observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(observer.map_palettes_alive(), 0);
}

/// A resize whose new target is refused while the old target is resident: the frame is drawn **without** the map (never with the old,
/// differently sized target), the old target stays resident and is not retired, the renderer retries by itself, and the retry replaces
/// the old target (retired, then let go of after its proof).
/// A refusal ends its retry when the map is withdrawn: a frame with no map is drawn once and the renderer goes idle, while the
/// refusal keeps repeating as long as the map is wanted.
pub(crate) fn map_a_withdrawn_map_ends_the_retry() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let told = Arc::new(Told::default());
    let hooks = TestHooks {
        // Every attempt is refused for the whole case.
        fail_map_allocations: u32::MAX,
        ..hooks_with(&probes, &slot)
    };
    let input = input_with(1);
    let map = map_of(&input, corner(), [5, 3], 1)
        .with_refusals(Arc::clone(&told) as Arc<dyn MapRefusalSink>);
    publish(&slot, 1, map);
    let mut presenter = probes.start_ok(&rig, hooks);
    let observer = &probes.observer;
    // The renderer keeps retrying by itself while the map is wanted.
    assert!(
        rig.pump_until(BOUND, || probes.frames() >= 3),
        "the renderer retried by itself"
    );
    // The presenter tells every refused attempt (the publisher's side keeps one message per episode) and no success.
    assert!(told.refused.lock().expect("the messages").len() >= 3);
    assert_eq!(told.cleared.load(Ordering::SeqCst), 0);
    // The map is withdrawn: a frame without it is drawn, and then nothing more is drawn.
    slot.publish(MetalScene::new(2, None));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(2)));
    rig.pump_for(Duration::from_millis(100));
    let drawn = probes.frames();
    rig.pump_for(Duration::from_millis(250));
    assert_eq!(
        probes.frames(),
        drawn,
        "no frame is drawn after the withdrawn map's own: the retry ended"
    );
    await_presented(&mut rig, observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

pub(crate) fn map_a_refused_resize_draws_without_the_map_and_recovers() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let told = Arc::new(Told::default());
    let hooks = TestHooks {
        // The first attempt (the first map) goes through; the second (the resize's target) is refused once.
        skip_map_allocations: 1,
        fail_map_allocations: 1,
        ..hooks_with(&probes, &slot)
    };
    let input = input_with(1);
    let with_refusals =
        |map: MetalMap| map.with_refusals(Arc::clone(&told) as Arc<dyn MapRefusalSink>);
    publish(&slot, 1, with_refusals(map_of(&input, corner(), [5, 3], 1)));
    // Watching starts before the presenter does: the first frame may already be refused when the start returns.
    told.watch(&probes.observer);
    let mut presenter = probes.start_ok(&rig, hooks);
    let observer = &probes.observer;
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(1)));
    assert_eq!(observer.map_targets_created(), 1);
    // The first frame is presented, hence its command buffer has completed: the next step is not busy and reaches the allocation.
    await_presented(&mut rig, observer);
    publish(&slot, 2, with_refusals(map_of(&input, corner(), [6, 4], 1)));
    assert!(
        rig.pump_until(BOUND, || told.cleared.load(Ordering::SeqCst) == 1),
        "the refusal was told and the renderer's own retry succeeded"
    );
    assert_eq!(told.refused.lock().expect("the messages").len(), 1);
    assert!(
        told.refused.lock().expect("the messages")[0].contains("target"),
        "{:?}",
        told.refused.lock().expect("the messages")
    );
    // The state at the instant of the refusal, read after the refused step itself: the old target is the only one, resident and not
    // retired. The state at the instant of the success: the new target is made and the old one retired (once) by the retry.
    assert_eq!(
        *told.at_refused.lock().expect("the snapshots"),
        [[1, 1, 0, 0]],
        "at the refusal: one target made, one alive, nothing retired or released"
    );
    let at_cleared = told.at_cleared.lock().expect("the snapshots").clone();
    assert_eq!(at_cleared.len(), 1);
    assert_eq!(
        (at_cleared[0][0], at_cleared[0][2]),
        (2, 1),
        "at the success: the new target is made and the old one retired, by the retry"
    );
    assert!(rig.pump_until(BOUND, || observer.captures() >= 3));
    let first = observer.captured_frame_at(0).expect("the first frame");
    let refused = observer.captured_frame_at(1).expect("the refused frame");
    let retried = observer.captured_frame_at(2).expect("the retry frame");
    assert_eq!(
        first_map_pixel(&first),
        expected_pixel(corner(), 0, 0),
        "the first map"
    );
    assert_eq!(
        first_map_pixel(&refused),
        PRESENTER_CLEAR,
        "the refused frame shows no map: not the old target"
    );
    assert_eq!(
        first_map_pixel(&retried),
        expected_pixel(corner(), 0, 0),
        "the new map"
    );
    assert_eq!(
        (
            observer.map_targets_created(),
            observer.map_last_target_size()
        ),
        (2, (6, 4)),
        "one new target, made by the retry"
    );
    assert_eq!(
        observer.map_retired(),
        1,
        "the old target was retired once, by the retry"
    );
    assert_eq!(
        observer.map_passes(),
        2,
        "the refused frame recorded no map pass"
    );
    rig.pump_for(Duration::from_millis(150));
    assert_eq!(
        probes.frames(),
        3,
        "one refused frame and one retry, no loop"
    );
    await_presented(&mut rig, observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(observer.map_targets_alive(), 0);
}

/// A retired target held by its token (as if a command buffer still used it) is not released and keeps the next replacement from
/// being made; the teardown waits for it, and nothing is released early.
pub(crate) fn map_retired_target_waits_for_its_token() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let hooks = TestHooks {
        // A resource retired now is held as if a command buffer still used it, for 500 ms.
        retired_token_delay: Duration::from_millis(500),
        ..hooks_with(&probes, &slot)
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    let observer = &probes.observer;
    let input = input_with(1);
    publish(&slot, 1, map_of(&input, corner(), [5, 3], 1));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(1)));
    publish(&slot, 2, map_of(&input, corner(), [6, 4], 1));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(2)));
    assert_eq!(
        observer.map_targets_alive(),
        2,
        "the retired target waits for its token"
    );
    assert_eq!(observer.map_retired_released(), 0);
    // Another resize arrives while the retired target waits: it must not be made yet.
    publish(&slot, 3, map_of(&input, corner(), [7, 5], 1));
    assert!(rig.pump_until(BOUND, || observer.map_admission_waits() >= 1));
    assert_eq!(
        observer.map_targets_created(),
        2,
        "no third target while a retired one waits"
    );
    // Once the token arrives, the retired target is let go of and the frame is drawn.
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(3)));
    assert_eq!(observer.map_targets_created(), 3);
    assert_eq!(
        observer.map_targets_max(),
        2,
        "never more than the current target and one retired"
    );
    await_presented(&mut rig, observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(
        observer.map_released_early(),
        0,
        "the teardown waited for the token"
    );
    assert_eq!(observer.map_targets_alive(), 0);
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
fn moded_input(generation: u64) -> Arc<MetalMapInput> {
    let fixture = the_fixture();
    Arc::new(
        MetalMapInput::new(
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

/// The map of `input` for `scenario`, showing `mode` (a revision and one value per place) when given.
fn moded_map(
    input: &Arc<MetalMapInput>,
    scenario: &fixture::Scenario,
    mode: Option<(u64, &[u16])>,
) -> MetalMap {
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

fn drawn_target(scenario: &fixture::Scenario, map: &MetalMap) -> Vec<u8> {
    readback_map(
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

/// The two views the mode cases draw: one and a half cells per map pixel (the coarse zoom) and a tenth of a cell (the fine zoom, all
/// three octaves of sub-cell detail over the blend).
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
pub(crate) fn readback_map_modes() {
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
            let got = drawn_target(scenario, &map);
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
            drawn_target(scenario, &map),
            plain,
            "{}: off",
            scenario.name
        );
    }
}

/// An input with an owner grid and no mode, and an input with no owner grid, draw the plain map; a table that is not one value per
/// place never reaches the presenter.
pub(crate) fn readback_map_mode_refusals() {
    let scenario = mode_scenarios().remove(0);
    let plain = the_fixture().reference_image(scenario.step, scenario.top_left, scenario.view);
    for input in [moded_input(1), input_with(2)] {
        let map = moded_map(&input, &scenario, None);
        assert_eq!(drawn_target(&scenario, &map), plain);
    }
    let input = moded_input(3);
    for length in [0, 1, 3, 5] {
        let map = map_at(&input, scenario.step, scenario.top_left, scenario.view, 1);
        assert_eq!(
            map.with_mode(1, vec![0; length].into())
                .expect_err("refused"),
            MetalMapError::ModeTable {
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
        MetalMapError::ModeTable {
            expected: 0,
            actual: 4
        }
    );
    let fixture = the_fixture();
    let short: Arc<[u8]> = vec![255; 8].into();
    assert_eq!(
        MetalMapInput::new(
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
        MetalMapError::OwnerGridLength {
            expected: 24 * 16 * 4,
            actual: 8
        }
    );
}

/// A map with no places has an empty mode table, and a frame that shows it is the plain map: nothing is owned, so nothing is read.
pub(crate) fn readback_map_mode_with_no_places() {
    let fixture = the_fixture();
    let input = Arc::new(
        MetalMapInput::new(
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
        let plain = the_fixture().reference_image(scenario.step, scenario.top_left, scenario.view);
        let map = moded_map(&input, scenario, Some((1, &[])));
        assert_eq!(drawn_target(scenario, &map), plain, "{}", scenario.name);
    }
}

/// What one frame of the live presenter shows.
#[derive(Clone, Copy)]
enum Shown<'a> {
    /// A value table (one value per place), of the frame's revision.
    Values(&'a [u16]),
    /// The elevation mode, of the frame's revision: it has no table.
    Elevation,
    /// No mode: the plain map.
    Off,
}

/// Through a live presenter, which keeps its resident input across frames: a mode's table is replaced by each new revision (the old one
/// retired under the proof of its last command buffer), turning the mode off draws the plain map, and turning it on again draws the
/// table of its own revision. The elevation mode, which has no table, goes between them: values, then elevation, then off, then
/// elevation, then values again (the resident table still usable), and each captured frame is compared, pixel for pixel, with the
/// reference for what is then showing; the input is uploaded once and the target made once.
pub(crate) fn map_modes_replace_the_resident_table_and_off_draws_the_plain_map() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let corner20 = fixture::Fixture::crop_top_left(20);
    let scenario = fixture::Scenario {
        name: "retained",
        step: 20,
        top_left: [corner20.x - 2, corner20.y - 2],
        view: [16, 12],
    };
    let input = moded_input(1);
    let first = fixture::mode_values(0);
    let second = fixture::mode_values(1);
    // What each frame shows. Its revision is its position in the sequence, so that every frame is newer than the one before.
    let frames = [
        Shown::Values(&first),
        Shown::Values(&second),
        Shown::Elevation,
        Shown::Off,
        Shown::Elevation,
        Shown::Values(&first),
        Shown::Off,
        Shown::Values(&second),
    ];
    let plain = the_fixture().reference_image(scenario.step, scenario.top_left, scenario.view);
    let elevation = the_fixture().reference_image_with_elevation(
        scenario.step,
        scenario.top_left,
        scenario.view,
    );
    let expected_for = |shown: Shown| match shown {
        Shown::Values(values) => moded_reference(&scenario, values),
        Shown::Elevation => elevation.clone(),
        Shown::Off => plain.clone(),
    };
    let map_for = |index: usize, shown: Shown| {
        let revision = u64::try_from(index + 1).expect("small");
        match shown {
            Shown::Values(values) => moded_map(&input, &scenario, Some((revision, values))),
            Shown::Elevation => moded_map(&input, &scenario, None).with_elevation(revision),
            Shown::Off => moded_map(&input, &scenario, None),
        }
    };
    // The first map is published before the presenter starts, so that the first frame the presenter draws and captures is the one with
    // the map (as `map_in_the_first_frame`), never a frame before any publication.
    publish(&slot, 1, map_for(0, frames[0]));
    let mut presenter = probes.start_ok(&rig, hooks_with(&probes, &slot));
    let observer = &probes.observer;
    for (index, shown) in frames.iter().enumerate() {
        let expected = expected_for(*shown);
        let generation = u64::try_from(index + 1).expect("small");
        // A capture arrives once its command buffer completed: the frame of this generation is the next one after those so far.
        let captured_before = if index == 0 { 0 } else { observer.captures() };
        if index > 0 {
            publish(&slot, generation, map_for(index, *shown));
        }
        assert!(
            rig.pump_until(BOUND, || presenter.drawn_generation() == Some(generation)
                && observer.captures() > captured_before),
            "frame {index} is drawn and captured"
        );
        let captured = observer.captured_frame().expect("a captured frame");
        for py in 0..48 {
            for px in 0..64 {
                let offset = usize::try_from(((py / 4) * 16 + px / 4) * 4).expect("small");
                assert_eq!(
                    bgra_at(&captured.pixels, captured.width, px, py),
                    [
                        expected[offset],
                        expected[offset + 1],
                        expected[offset + 2],
                        expected[offset + 3]
                    ],
                    "frame {index}: drawable pixel ({px}, {py})"
                );
            }
        }
    }
    // The frames differ where they should: the two tables, the elevation and the plain map are four different images, so a frame that
    // kept showing its predecessor would fail above.
    let images = [
        expected_for(frames[0]),
        expected_for(frames[1]),
        elevation.clone(),
        plain.clone(),
    ];
    for (a, left) in images.iter().enumerate() {
        for (b, right) in images.iter().enumerate().skip(a + 1) {
            assert_ne!(left, right, "images {a} and {b} differ");
        }
    }
    assert_eq!(
        observer.map_grids_created(),
        1,
        "the input is uploaded once"
    );
    assert_eq!(
        observer.map_targets_created(),
        1,
        "and the target made once"
    );
    await_presented(&mut rig, observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// The look's patterns (the blue-noise tile and the hillside dither) are pinned to the world pixel: after a camera pan, every world pixel
/// that both views show has the same colour, at two zoom steps (the fertility ramp, the slopes of both signs and the glacier patch all
/// show), and each view equals the reference.
pub(crate) fn readback_map_patterns_stay_pinned_to_the_world_after_a_pan() {
    let fixture = the_fixture();
    let input = input_with(1);
    for (step, shift) in [(36_u8, [13_usize, 7_usize]), (60, [37, 21])] {
        let corner = fixture::Fixture::crop_top_left(step);
        let view = [96_u32, 72_u32];
        let first_top_left = [corner.x - 2, corner.y - 2];
        let second_top_left = [
            first_top_left[0] + i32::try_from(shift[0]).expect("small"),
            first_top_left[1] + i32::try_from(shift[1]).expect("small"),
        ];
        let draw = |top_left: [i32; 2]| {
            let map = map_at(&input, step, top_left, view, 1);
            readback_map(view[0] * 4, view[1] * 4, CLEAR, Some(&map), None)
                .expect("the readback works")
                .target
                .expect("a map target")
        };
        let (first, second) = (draw(first_top_left), draw(second_top_left));
        assert_eq!(
            first,
            fixture.reference_image(step, first_top_left, view),
            "step {step}, first view"
        );
        assert_eq!(
            second,
            fixture.reference_image(step, second_top_left, view),
            "step {step}, second view"
        );
        let (width, height) = (view[0] as usize, view[1] as usize);
        let mut compared = 0;
        for y in 0..height - shift[1] {
            for x in 0..width - shift[0] {
                let (fx, fy) = (x + shift[0], y + shift[1]);
                assert_eq!(
                    first[(fy * width + fx) * 4..(fy * width + fx) * 4 + 4],
                    second[(y * width + x) * 4..(y * width + x) * 4 + 4],
                    "step {step}: world pixel ({}, {}) differs between the two views",
                    first_top_left[0] + i32::try_from(fx).expect("small"),
                    first_top_left[1] + i32::try_from(fy).expect("small"),
                );
                compared += 1;
            }
        }
        assert!(compared > 1000, "the views overlap widely");
        // The two views are not one image shifted by a whole tile: the pattern is a property of the world, not of the view.
        assert_ne!(first, second);
    }
}

/// The elevation mode (milestone 9, slice 10b): at two zoom steps the map target equals the reference byte for byte, on an input with
/// the owner grid and on one without (the mode reads the map's own elevations); the mode, a value table and off follow one another on
/// the same input, each drawing its own reference; and the elevation draws the greys of many heights, over land only.
pub(crate) fn readback_map_elevation_mode() {
    let with_owners = moded_input(1);
    let without = input_with(2);
    let stockpile = vec![stockpile_value(0, 1000), 500, 1000, 0xFFFF];
    for scenario in &mode_scenarios() {
        let plain = the_fixture().reference_image(scenario.step, scenario.top_left, scenario.view);
        let want = the_fixture().reference_image_with_elevation(
            scenario.step,
            scenario.top_left,
            scenario.view,
        );
        assert_ne!(want, plain, "{}: the mode changes the map", scenario.name);
        let greys: BTreeSet<u8> = want
            .chunks_exact(4)
            .filter(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2])
            .map(|pixel| pixel[0])
            .collect();
        assert!(
            greys.len() >= 8,
            "{}: the fixture's heights draw many greys, saw {greys:?}",
            scenario.name
        );
        for (name, input) in [("with an owner grid", &with_owners), ("without", &without)] {
            let got = drawn_target(
                scenario,
                &moded_map(input, scenario, None).with_elevation(1),
            );
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
        }
        // One input through the modes in turn: a value table, the elevation, off, the elevation again, the table again.
        let draw = |map: MetalMap| drawn_target(scenario, &map);
        let table = moded_reference(scenario, &stockpile);
        assert_eq!(
            draw(moded_map(&with_owners, scenario, Some((10, &stockpile)))),
            table
        );
        assert_eq!(
            draw(moded_map(&with_owners, scenario, None).with_elevation(11)),
            want
        );
        assert_eq!(draw(moded_map(&with_owners, scenario, None)), plain);
        assert_eq!(
            draw(moded_map(&with_owners, scenario, None).with_elevation(12)),
            want
        );
        assert_eq!(
            draw(moded_map(&with_owners, scenario, Some((13, &stockpile)))),
            table
        );
    }
}

pub(crate) const CASES: [(&str, fn()); 24] = [
    ("readback_map_full_rules", readback_map_full_rules),
    ("readback_map_rivers_overlay", readback_map_rivers_overlay),
    (
        "readback_map_location_overlays",
        readback_map_location_overlays,
    ),
    (
        "readback_map_ireland_rivers_after_zoom",
        readback_map_ireland_rivers_after_zoom,
    ),
    (
        "readback_map_negative_control",
        readback_map_negative_control,
    ),
    ("readback_map_camera_refusals", readback_map_camera_refusals),
    ("readback_map_view_sizes", readback_map_view_sizes),
    ("readback_map_enlarged", readback_map_enlarged),
    (
        "readback_map_below_the_legacy_quad",
        readback_map_below_the_legacy_quad,
    ),
    (
        "readback_map_none_is_the_frame_without_a_map",
        readback_map_none_is_the_frame_without_a_map,
    ),
    ("readback_map_refusals", readback_map_refusals),
    ("readback_map_modes", readback_map_modes),
    ("readback_map_mode_refusals", readback_map_mode_refusals),
    ("readback_map_elevation_mode", readback_map_elevation_mode),
    (
        "readback_map_patterns_stay_pinned_to_the_world_after_a_pan",
        readback_map_patterns_stay_pinned_to_the_world_after_a_pan,
    ),
    (
        "readback_map_mode_with_no_places",
        readback_map_mode_with_no_places,
    ),
    (
        "map_modes_replace_the_resident_table_and_off_draws_the_plain_map",
        map_modes_replace_the_resident_table_and_off_draws_the_plain_map,
    ),
    ("map_in_the_first_frame", map_in_the_first_frame),
    ("map_absent_then_present", map_absent_then_present),
    (
        "map_resize_retires_after_the_proof",
        map_resize_retires_after_the_proof,
    ),
    (
        "map_a_refused_allocation_is_told_retried_and_cleared",
        map_a_refused_allocation_is_told_retried_and_cleared,
    ),
    (
        "map_overlay_palette_refusal_retries_without_replacing_the_old_generation",
        map_overlay_palette_refusal_retries_without_replacing_the_old_generation,
    ),
    (
        "map_a_refused_resize_draws_without_the_map_and_recovers",
        map_a_refused_resize_draws_without_the_map_and_recovers,
    ),
    (
        "map_a_withdrawn_map_ends_the_retry",
        map_a_withdrawn_map_ends_the_retry,
    ),
];

pub(crate) const MORE: [(&str, fn()); 1] = [(
    "map_retired_target_waits_for_its_token",
    map_retired_target_waits_for_its_token,
)];
