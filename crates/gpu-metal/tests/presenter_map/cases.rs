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

use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ck_gpu_metal::{
    DestinationRect, MapRefusalSink, MetalFilter, MetalMap, MetalMapError, MetalMapInput,
    MetalMapRules, MetalQuad, MetalQuadSlot, MetalScene, MetalTerrainKind, MetalTerrainRule,
    PixelSource, SourceRect, readback_clear, readback_map, readback_quad,
};
use ck_map::{STEP_SIZE, WorldPixel, fixture};

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
            .map(|terrain| MetalTerrainRule {
                kind: kind_of(terrain.kind),
                solid: terrain.style.solid.0,
                second: terrain.style.second.0,
                dark: terrain.style.dark.0,
                dark_second: terrain.style.dark_second.0,
                cap: terrain.style.cap.0,
                high_ground: terrain.style.high_ground.0,
                relief: [terrain.style.relief_low, terrain.style.relief_high],
                mix_percent: terrain.style.mix_percent,
                cap_heights: [terrain.style.cap_low, terrain.style.cap_high],
            })
            .collect(),
        sea: fixture.data.sea.0,
        shallow_sea: fixture.data.shallow_sea.0,
        lake: fixture.data.lake.0,
        river: fixture.data.river.0,
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

/// Negative control: with one rule table entry deliberately wrong (pasture's second tone another colour) the readback changes in
/// exactly the pixels that show that tone, and in no other.
pub(crate) fn readback_map_negative_control() {
    let fixture = the_fixture();
    let wrong = fixture.with_other_pasture_second_tone();
    let input = input_of(&wrong, 1);
    let second = fixture.terrains[2].style.second.0;
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
                want == second,
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
    publish(&slot, 3, map_of(&input_with(1), corner(), [5, 3], 1));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(3)));
    assert_eq!(observer.map_passes(), 1);
    // And removing it again stops the pass but keeps the resident resources for the next frame with a map.
    slot.publish(MetalScene::new(4, None));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(4)));
    assert_eq!(observer.map_passes(), 1, "no map, no pass");
    assert_eq!(observer.map_targets_alive(), 1, "the target is kept");
    publish(&slot, 5, map_of(&input_with(1), corner(), [5, 3], 1));
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
    await_presented(&mut rig, observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A resize replaces the target and a new input replaces the images; the old ones are retired and let go of only after the proof
/// of the last command buffer that used them, which this case orders by a held completion, not by elapsed time.
pub(crate) fn map_resize_retires_after_the_proof() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let gate = Arc::new(AtomicI32::new(0));
    let hooks = TestHooks {
        frame_completion_gate: Some(Arc::clone(&gate)),
        ..hooks_with(&probes, &slot)
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    let observer = &probes.observer;
    let input = input_with(1);
    publish(&slot, 1, map_of(&input, corner(), [5, 3], 1));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(1)));
    assert_eq!(
        (
            observer.map_targets_created(),
            observer.map_retired(),
            observer.map_targets_alive()
        ),
        (1, 0, 1)
    );
    // The view grows while the first frame's command buffer is still outstanding: the step finds it so, and makes nothing.
    let steps = observer.frame_steps();
    publish(&slot, 2, map_of(&input, corner(), [6, 4], 1));
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
    assert_eq!(observer.map_retired(), 0);
    gate.store(1, Ordering::SeqCst);
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(2)));
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
    publish(&slot, 3, map_of(&input_with(2), corner(), [6, 4], 1));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(3)));
    assert_eq!(observer.map_grids_created(), 2);
    assert_eq!(observer.map_targets_created(), 2, "the view did not change");
    assert_eq!(observer.map_retired(), 2);
    assert_eq!(observer.map_retired_released(), 2);
    assert_eq!(observer.map_grids_alive(), 1);
    await_presented(&mut rig, observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(observer.map_targets_alive(), 0);
    assert_eq!(observer.map_grids_alive(), 0);
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

pub(crate) const CASES: [(&str, fn()); 14] = [
    ("readback_map_full_rules", readback_map_full_rules),
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
