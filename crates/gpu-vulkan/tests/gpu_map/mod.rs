//! The GPU-written map image (milestone 6, slice 1), off screen: the presenter's own map pass and composition, read back under the
//! validation layer (with synchronization validation) with no window. A submodule of `gpu.rs`, so the group that runs that target
//! runs these.
//!
//! The fixture is a 7 x 5 crop at global origin cell (100, 200) with four terrains laid out irregularly (so a mirrored or transposed
//! read fails), seen through a 5 x 3 view (an odd size). The expected pixels come from the slice's minimal colour rule written out
//! here in plain integers: map pixel `(x, y)` shows palette entry `terrain(left + x - i0, top + y - j0)`, entry 0 outside the crop.
//! Any validation message fails a case, hazards between the map pass and the window pass included.
//!
//! The window readback is `B8G8R8A8_UNORM` (a texel `(r, g, b, a)` reads back as `[b, g, r, a]`); the map target is `R8G8B8A8_UNORM`.

use std::sync::Arc;

use ck_gpu_vulkan::{
    DestinationRect, PixelSource, SourceRect, VulkanFilter, VulkanMap, VulkanMapError,
    VulkanMapInput, VulkanQuad, load_loader, readback_map, readback_quad,
};

use super::{assert_log_clean, destroy, validated_with};

const ORIGIN: [i32; 2] = [100, 200];
const CROP: [u32; 2] = [7, 5];

/// Grid A's terrain index per cell, rows running south. Irregular on purpose: three or more indices, no symmetry.
const TERRAIN: [[u8; 7]; 5] = [
    [1, 1, 2, 3, 1, 2, 2],
    [3, 1, 1, 2, 2, 3, 1],
    [2, 3, 1, 1, 1, 2, 3],
    [1, 2, 2, 3, 2, 1, 1],
    [3, 3, 1, 2, 1, 1, 2],
];

/// The palette: entry 0 is what outside the crop shows.
const PALETTE: [[u8; 4]; 4] = [
    [10, 20, 30, 255],
    [200, 50, 50, 255],
    [50, 200, 50, 255],
    [50, 50, 200, 255],
];

struct Bytes(Vec<u8>);

impl PixelSource for Bytes {
    fn pixels(&self) -> &[u8] {
        &self.0
    }
}

fn input_with(generation: u64) -> Arc<VulkanMapInput> {
    let mut grid_a = Vec::new();
    let mut grid_b = Vec::new();
    for (y, row) in TERRAIN.iter().enumerate() {
        for (x, terrain) in row.iter().enumerate() {
            // G bit 0 (usable) varies, so a read of the wrong channel fails; grid B holds an elevation nobody reads in this slice.
            grid_a.extend_from_slice(&[*terrain, u8::from((x + y) % 2 == 0), 0, 255]);
            grid_b.extend_from_slice(&[0x2A, u8::try_from(x * 16 + y).expect("small"), 0, 255]);
        }
    }
    Arc::new(
        VulkanMapInput::new(
            generation,
            ORIGIN,
            CROP,
            grid_a.into(),
            grid_b.into(),
            PALETTE.to_vec().into(),
        )
        .expect("a valid input"),
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
    let (width, height) = (view[0] * 4 * scale, view[1] * 4 * scale);
    VulkanMap::new(
        Arc::clone(input),
        top_left,
        view,
        rect(0, 0, signed(width), signed(height)),
        source(0.0, 0.0, float(view[0]), float(view[1])),
    )
    .expect("a valid map")
}

/// The minimal rule: the RGBA colour of map pixel `(x, y)` of a view at `top_left`.
fn expected_pixel(top_left: [i32; 2], x: u32, y: u32) -> [u8; 4] {
    let cell_x = i64::from(top_left[0]) + i64::from(x) - i64::from(ORIGIN[0]);
    let cell_y = i64::from(top_left[1]) + i64::from(y) - i64::from(ORIGIN[1]);
    let inside =
        (0..i64::from(CROP[0])).contains(&cell_x) && (0..i64::from(CROP[1])).contains(&cell_y);
    if inside {
        let terrain = TERRAIN[usize::try_from(cell_y).expect("inside")]
            [usize::try_from(cell_x).expect("inside")];
        PALETTE[usize::from(terrain)]
    } else {
        PALETTE[0]
    }
}

/// The map target the rule gives, RGBA8 rows top first.
fn expected_target(top_left: [i32; 2], view: [u32; 2]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for y in 0..view[1] {
        for x in 0..view[0] {
            bytes.extend_from_slice(&expected_pixel(top_left, x, y));
        }
    }
    bytes
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

/// The map target equals the minimal rule byte for byte: for a camera inside the crop, one overlapping its right and bottom edges
/// (where the pixels beyond show entry 0), one overlapping its left and top edges, and one wholly outside; and a pan away and back
/// gives identical bytes.
#[test]
fn the_map_target_equals_the_minimal_rule_and_a_pan_away_and_back_gives_identical_bytes() {
    let (_loader, instance, log) = rig!();
    let input = input_with(1);
    let view = [5, 3];
    let inside = [ORIGIN[0] + 1, ORIGIN[1] + 1];
    let right_bottom = [ORIGIN[0] + 4, ORIGIN[1] + 3];
    let left_top = [ORIGIN[0] - 2, ORIGIN[1] - 1];
    let outside = [ORIGIN[0] + 50, ORIGIN[1] - 40];
    let mut seen: Vec<Vec<u8>> = Vec::new();
    for top_left in [inside, right_bottom, left_top, outside, inside] {
        let map = map_of(&input, top_left, view, 1);
        let drawn =
            readback_map(&instance, 20, 12, CLEAR, Some(&map), None).expect("the readback works");
        let target = drawn.target.expect("a map target");
        assert_eq!(
            target,
            expected_target(top_left, view),
            "the target at {top_left:?} equals the rule"
        );
        seen.push(target);
    }
    assert_ne!(seen[0], seen[1]);
    assert_ne!(seen[1], seen[2]);
    assert_eq!(seen[0], seen[4], "away and back gives identical bytes");
    let edge = expected_target(right_bottom, view);
    assert_eq!(
        &edge[(2 * 5 + 4) * 4..(2 * 5 + 4) * 4 + 4],
        &PALETTE[0],
        "(4, 2) is cell (8, 5): outside"
    );
    assert_eq!(
        &edge[0..4],
        &PALETTE[usize::from(TERRAIN[3][4])],
        "(0, 0) is cell (4, 3)"
    );
    assert!(seen[3].chunks_exact(4).all(|pixel| pixel == PALETTE[0]));
    assert_log_clean(&log);
    assert_eq!(instance.retained_readbacks(), 0);
    destroy(instance);
    assert_log_clean(&log);
}

/// A view that is not the crop's shape (5 x 3 over 7 x 5), one larger than the crop and a one-pixel view read the same cells.
#[test]
fn views_of_other_shapes_read_the_same_cells() {
    let (_loader, instance, log) = rig!();
    let input = input_with(1);
    for view in [[1, 1], [5, 3], [9, 7], [7, 5], [2, 9]] {
        let top_left = [ORIGIN[0] - 1, ORIGIN[1] - 1];
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
    let top_left = [ORIGIN[0] + 2, ORIGIN[1] + 1];
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
    let top_left = ORIGIN;
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
    let grid: Arc<[u8]> = vec![0; 7 * 5 * 4].into();
    let palette: Arc<[[u8; 4]]> = PALETTE.to_vec().into();
    let make = |size: [u32; 2], grid_b: &Arc<[u8]>, palette: &Arc<[[u8; 4]]>| {
        VulkanMapInput::new(
            1,
            ORIGIN,
            size,
            Arc::clone(&grid),
            Arc::clone(grid_b),
            Arc::clone(palette),
        )
    };
    assert!(make(CROP, &grid, &palette).is_ok());
    assert_eq!(
        make([0, 5], &grid, &palette).expect_err("refused"),
        VulkanMapError::Size
    );
    assert_eq!(
        make([8, 5], &grid, &palette).expect_err("refused"),
        VulkanMapError::GridLength {
            expected: 160,
            actual: 140
        }
    );
    let short: Arc<[u8]> = vec![0; 8].into();
    assert_eq!(
        make(CROP, &short, &palette).expect_err("refused"),
        VulkanMapError::GridLength {
            expected: 140,
            actual: 8
        }
    );
    let none: Arc<[[u8; 4]]> = Vec::new().into();
    assert_eq!(
        make(CROP, &grid, &none).expect_err("refused"),
        VulkanMapError::Palette(0)
    );
    let many: Arc<[[u8; 4]]> = vec![[0; 4]; 257].into();
    assert_eq!(
        make(CROP, &grid, &many).expect_err("refused"),
        VulkanMapError::Palette(257)
    );

    let input = input_with(1);
    let map = |view: [u32; 2], destination: DestinationRect, source: SourceRect| {
        VulkanMap::new(Arc::clone(&input), [0, 0], view, destination, source)
    };
    let whole = source(0.0, 0.0, 5.0, 3.0);
    assert!(map([5, 3], rect(0, 0, 20, 12), whole).is_ok());
    assert_eq!(
        map([0, 3], rect(0, 0, 20, 12), whole).expect_err("refused"),
        VulkanMapError::Size
    );
    assert_eq!(
        map([5, 3], rect(0, 0, 0, 12), whole).expect_err("refused"),
        VulkanMapError::Destination
    );
    for bad in [
        source(0.0, 0.0, 5.5, 3.0),
        source(-1.0, 0.0, 5.0, 3.0),
        source(2.0, 0.0, 2.0, 3.0),
        source(0.0, 0.0, f32::NAN, 3.0),
    ] {
        assert_eq!(
            map([5, 3], rect(0, 0, 20, 12), bad).expect_err("refused"),
            VulkanMapError::Source,
            "{bad:?}"
        );
    }
}
