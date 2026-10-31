//! The instanced scene (CK-116), off screen: the `readback_scene_*` cases draw a clear, an optional legacy quad and a
//! list of instances through the presenter's own instance layout, batching, pipeline, samplers and encoding
//! (`readback_scene`) and read the pixels back. They open no window (the presenter target's `gpu` set); the windowed
//! cases of the same issue are in `presenter_scene/instanced.rs`. Each names its negative control.

use ck_gpu_metal::{
    CHECKER_CELL, CHECKER_SIDE, DestinationRect, MetalFilter, MetalQuad, PixelSource,
    ReadbackImage, ReadbackInstance, ReadbackTexture, checker_pixels, readback_scene,
};

use std::sync::Arc;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> DestinationRect {
    DestinationRect {
        left,
        top,
        right,
        bottom,
    }
}

const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// A small count as a float.
fn float(value: u32) -> f32 {
    f32::from(u16::try_from(value).expect("a small number"))
}

/// A small float rounded down, as an integer.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the values are a few texels, nowhere near the limits of i32"
)]
fn floor(value: f32) -> i32 {
    value.floor() as i32
}

/// A value from 0 to 255, rounded, as a byte.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255 and rounded just before"
)]
fn byte(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// The pixel at (x, y) of a BGRA8 readback, as RGBA.
fn at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * width + x) * 4) as usize;
    [
        pixels[offset + 2],
        pixels[offset + 1],
        pixels[offset],
        pixels[offset + 3],
    ]
}

/// The clear colour as the 8-bit values the target holds.
fn clear_bytes(rgb: [f32; 3]) -> [u8; 3] {
    rgb.map(|channel| byte(channel * 255.0))
}

/// An image and its pixels.
struct Picture {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

fn picture(width: u32, height: u32, pixels: &[[u8; 4]]) -> Picture {
    assert_eq!(pixels.len(), (width * height) as usize);
    Picture {
        width,
        height,
        bytes: pixels.concat(),
    }
}

fn solid(width: u32, height: u32, pixel: [u8; 4]) -> Picture {
    picture(width, height, &vec![pixel; (width * height) as usize])
}

fn images(pictures: &[Picture]) -> Vec<ReadbackImage<'_>> {
    pictures
        .iter()
        .map(|picture| ReadbackImage {
            width: picture.width,
            height: picture.height,
            pixels: &picture.bytes,
        })
        .collect()
}

fn instance(
    texture: ReadbackTexture,
    destination: DestinationRect,
    source: [f32; 4],
) -> ReadbackInstance {
    ReadbackInstance {
        texture,
        destination,
        source,
    }
}

fn of(index: usize, destination: DestinationRect) -> ReadbackInstance {
    instance(ReadbackTexture::Image(index), destination, WHOLE)
}

/// `source over destination` with premultiplied alpha, per channel, in floats: `src + dst * (255 - a) / 255`.
fn over(source: [u8; 4], destination: [f32; 3]) -> [f32; 3] {
    let alpha = f32::from(source[3]);
    [0, 1, 2]
        .map(|channel| f32::from(source[channel]) + destination[channel] * (255.0 - alpha) / 255.0)
}

/// Each channel within `tolerance` of the float expectation. The GPU's blend rounds in fixed point: one step per blend
/// the pixel went through is allowed, which is the tolerance the legacy quad's pixel tests state too.
fn near(seen: [u8; 4], expected: [f32; 3], tolerance: u8, what: &str) {
    for channel in 0..3 {
        assert!(
            seen[channel].abs_diff(byte(expected[channel])) <= tolerance,
            "{what}: channel {channel} is {} against {:.1}",
            seen[channel],
            expected[channel]
        );
    }
    assert_eq!(seen[3], 255, "{what}: the target stays opaque");
}

fn target(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    at(pixels, width, x, y)
}

// ---------------------------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------------------------

/// Three overlapping half-transparent instances give, at each overlap, exactly the premultiplied result of drawing them
/// in order, and swapping two changes the result accordingly. Negative control: drawing in reverse order (or sorted by
/// texture) fails the first loop; ignoring the swap fails the second.
pub(crate) fn readback_scene_painter_order() {
    let clear = [0.25, 0.5, 0.75];
    let dst = clear_bytes(clear).map(f32::from);
    // Premultiplied: red 50%, green 25%, blue 75%.
    let a = [128, 0, 0, 128];
    let b = [0, 64, 0, 64];
    let c = [0, 0, 192, 192];
    let pictures = [solid(1, 1, a), solid(1, 1, b), solid(1, 1, c)];
    // A covers x 0..6, B covers x 2..8 and C covers x 4..8, all rows 0..4: A only, A+B, A+B+C and B+C and C regions.
    let rects = [rect(0, 0, 6, 4), rect(2, 0, 8, 4), rect(4, 0, 8, 4)];
    let draw = |order: [usize; 3]| {
        let list: Vec<ReadbackInstance> =
            order.iter().map(|&index| of(index, rects[index])).collect();
        readback_scene(
            8,
            4,
            clear,
            None,
            MetalFilter::Nearest,
            &images(&pictures),
            &list,
        )
        .expect("the readback works")
    };
    // The covering of column x (0..8) in painter order `order`.
    let expect = |order: [usize; 3], x: u32| {
        let mut value = dst;
        let mut steps = 0_u8;
        for index in order {
            if (rects[index].left..rects[index].right).contains(&i32::try_from(x).expect("small")) {
                value = over([a, b, c][index], value);
                steps += 1;
            }
        }
        (value, steps)
    };
    let forward = draw([0, 1, 2]);
    for x in 0..8 {
        let (value, steps) = expect([0, 1, 2], x);
        near(
            target(&forward, 8, x, 1),
            value,
            steps.max(1),
            &format!("A B C, column {x}"),
        );
    }
    // Swapped: C first, then B, then A. At the triple overlap (x 4..6) the result differs from the forward order.
    let backward = draw([2, 1, 0]);
    for x in 0..8 {
        let (value, steps) = expect([2, 1, 0], x);
        near(
            target(&backward, 8, x, 1),
            value,
            steps.max(1),
            &format!("C B A, column {x}"),
        );
    }
    assert_ne!(
        target(&forward, 8, 5, 1),
        target(&backward, 8, 5, 1),
        "the triple overlap depends on the order"
    );
    // Columns covered by a single instance do not (columns 0 and 1 are A's alone).
    assert_eq!(target(&forward, 8, 0, 1), target(&backward, 8, 0, 1));
    assert_eq!(target(&forward, 8, 1, 1), target(&backward, 8, 1, 1));
}

/// A legacy quad and a scene instance over the same pixels: the scene instance is on top (the legacy quad is below every
/// scene quad). Negative control: drawing the legacy quad after the scene fails both checks.
pub(crate) fn readback_scene_legacy_quad_is_below() {
    let clear = [0.0; 3];
    let legacy_pixels: Arc<dyn PixelSource> = Arc::new(Bytes(RED.repeat(4)));
    let legacy = MetalQuad::new(
        legacy_pixels,
        2,
        2,
        rect(0, 0, 8, 8),
        ck_gpu_metal::SourceRect {
            left: 0.0,
            top: 0.0,
            right: 2.0,
            bottom: 2.0,
        },
        MetalFilter::Nearest,
    )
    .expect("a valid legacy quad");
    // Opaque blue over the legacy red in the middle, half-transparent green over it elsewhere.
    let pictures = [solid(1, 1, BLUE), solid(1, 1, [0, 128, 0, 128])];
    let list = [of(0, rect(2, 2, 6, 6)), of(1, rect(0, 0, 3, 3))];
    let out = readback_scene(
        8,
        8,
        clear,
        Some(&legacy),
        MetalFilter::Nearest,
        &images(&pictures),
        &list,
    )
    .expect("the readback works");
    assert_eq!(
        target(&out, 8, 4, 4),
        BLUE,
        "the opaque scene quad covers the legacy quad"
    );
    assert_eq!(
        target(&out, 8, 7, 7),
        RED,
        "the legacy quad shows where nothing covers it"
    );
    // Half-transparent green over the legacy red: green + red * (255 - 128) / 255.
    near(
        target(&out, 8, 0, 0),
        over([0, 128, 0, 128], [255.0, 0.0, 0.0]),
        1,
        "the translucent scene quad over the legacy quad",
    );
    // Overlap of the blue quad (drawn first) and the green one (second): green over blue.
    near(
        target(&out, 8, 2, 2),
        over([0, 128, 0, 128], [0.0, 0.0, 255.0]),
        1,
        "the second scene quad over the first",
    );
}

struct Bytes(Vec<u8>);

impl PixelSource for Bytes {
    fn pixels(&self) -> &[u8] {
        &self.0
    }
}

/// Instances A, A, B, A draw the right pixels (three batches in the presenter's own batching). Negative control:
/// sorting by texture would still give these pixels, so this case also overlaps the neighbours; with the overlap, a
/// sorted draw (A A A, B) paints the second A over B and fails the last assertion.
pub(crate) fn readback_scene_batching_keeps_painter_order() {
    let pictures = [solid(1, 1, RED), solid(1, 1, BLUE)];
    // Four 4-pixel cells; the third (B) overlaps the fourth (A) at x 10..12 and A, drawn after it, must win there.
    let list = [
        of(0, rect(0, 0, 4, 4)),
        of(0, rect(4, 0, 8, 4)),
        of(1, rect(8, 0, 12, 4)),
        of(0, rect(10, 0, 14, 4)),
    ];
    let out = readback_scene(
        16,
        4,
        [0.0; 3],
        None,
        MetalFilter::Nearest,
        &images(&pictures),
        &list,
    )
    .expect("the readback works");
    assert_eq!(target(&out, 16, 1, 1), RED);
    assert_eq!(target(&out, 16, 5, 1), RED);
    assert_eq!(target(&out, 16, 8, 1), BLUE);
    assert_eq!(target(&out, 16, 9, 1), BLUE);
    assert_eq!(target(&out, 16, 10, 1), RED, "the later A is on top of B");
    assert_eq!(target(&out, 16, 13, 1), RED);
    assert_eq!(target(&out, 16, 15, 1), [0, 0, 0, 255]);
}

/// Nearest and linear over the same 2x2 texture scaled up: exact blocks for nearest, interpolated within a stated
/// tolerance for linear. Negative control: a frame that ignores its filter fails one of the two.
pub(crate) fn readback_scene_filters() {
    let pictures = [picture(2, 2, &[BLACK, WHITE, WHITE, BLACK])];
    let list = [of(0, rect(0, 0, 8, 8))];
    let hard = readback_scene(
        8,
        8,
        [0.0; 3],
        None,
        MetalFilter::Nearest,
        &images(&pictures),
        &list,
    )
    .expect("the readback works");
    let soft = readback_scene(
        8,
        8,
        [0.0; 3],
        None,
        MetalFilter::Linear,
        &images(&pictures),
        &list,
    )
    .expect("the readback works");
    for y in 0..8 {
        for x in 0..8 {
            let expected = if (x / 4 + y / 4) % 2 == 0 {
                BLACK
            } else {
                WHITE
            };
            assert_eq!(
                target(&hard, 8, x, y),
                expected,
                "nearest, pixel ({x}, {y})"
            );
        }
    }
    // Linear: the bilinear weights of the pixel's centre in texel units, clamped at the edges; tolerance 3 of 255 (the
    // GPU interpolates with a fixed-point fraction).
    let texel = |x: i32, y: i32| -> f32 {
        if (x.clamp(0, 1) + y.clamp(0, 1)) % 2 == 0 {
            0.0
        } else {
            255.0
        }
    };
    let mut differing = 0;
    for y in 0..8_u32 {
        for x in 0..8_u32 {
            let u = (float(x) + 0.5) / 8.0 * 2.0 - 0.5;
            let v = (float(y) + 0.5) / 8.0 * 2.0 - 0.5;
            let (x0, y0) = (floor(u), floor(v));
            let (fx, fy) = (u - u.floor(), v - v.floor());
            let expected = texel(x0, y0) * (1.0 - fx) * (1.0 - fy)
                + texel(x0 + 1, y0) * fx * (1.0 - fy)
                + texel(x0, y0 + 1) * (1.0 - fx) * fy
                + texel(x0 + 1, y0 + 1) * fx * fy;
            let seen = f32::from(target(&soft, 8, x, y)[0]);
            assert!(
                (seen - expected).abs() <= 3.0,
                "linear ({x}, {y}): {seen} against {expected}"
            );
            if target(&soft, 8, x, y) != target(&hard, 8, x, y) {
                differing += 1;
            }
        }
    }
    assert!(
        differing >= 16,
        "linear and nearest differ in {differing} pixels"
    );
}

/// The checker is always nearest, even in a linear frame. Negative control: sampling it with the frame's linear sampler
/// blends its cell edges at a scale that puts texel centres between pixel centres, and fails the second size.
pub(crate) fn readback_scene_checker_stays_sharp_under_a_linear_frame() {
    // 64 x 64 at 1:1 has cells exactly 8 pixels wide, and stretched
    // to 100 x 100 (a scale that puts texel centres between pixel centres) every pixel is still exactly one of the two
    // colours - a linear sampler would blend them along every cell edge.
    for side in [64_u32, 100] {
        let side_signed = i32::try_from(side).expect("small");
        let sharp = readback_scene(
            side,
            side,
            [0.0; 3],
            None,
            MetalFilter::Linear,
            &[],
            &[instance(
                ReadbackTexture::Checker,
                rect(0, 0, side_signed, side_signed),
                WHOLE,
            )],
        )
        .expect("the readback works");
        for y in 0..side {
            for x in 0..side {
                let seen = target(&sharp, side, x, y);
                assert!(
                    seen == MAGENTA || seen == BLACK,
                    "checker under a linear frame at {side}, ({x}, {y}): {seen:?}"
                );
                if side == 64 {
                    let expected = if (x / 8 + y / 8) % 2 == 0 {
                        MAGENTA
                    } else {
                        BLACK
                    };
                    assert_eq!(seen, expected, "({x}, {y})");
                }
            }
        }
    }
}

/// A clipped instance samples exactly the trimmed part of the image; the same instance with a texture of different
/// dimensions samples the whole new image over the same destination. Negative control: reading the source as image
/// pixels instead of fractions fails the first check; scaling by the old dimensions fails the second.
pub(crate) fn readback_scene_normalised_source() {
    // A 4 x 4 picture whose pixel (x, y) is (10 + 40x, 10 + 40y, 200, 255).
    let pixels: Vec<[u8; 4]> = (0_u8..16)
        .map(|index| [10 + 40 * (index % 4), 10 + 40 * (index / 4), 200, 255])
        .collect();
    let pictures = [picture(4, 4, &pixels)];
    let pixel = |x: u32, y: u32| pixels[(y * 4 + x) as usize];
    // The middle 2x2 (fractions 0.25..0.75), drawn 1:1 into the middle of a 4x4 target.
    let list = [instance(
        ReadbackTexture::Image(0),
        rect(1, 1, 3, 3),
        [0.25, 0.25, 0.75, 0.75],
    )];
    let out = readback_scene(
        4,
        4,
        [0.0; 3],
        None,
        MetalFilter::Nearest,
        &images(&pictures),
        &list,
    )
    .expect("the readback works");
    for y in 0..4 {
        for x in 0..4 {
            let expected = if (1..3).contains(&x) && (1..3).contains(&y) {
                pixel(x, y)
            } else {
                BLACK
            };
            assert_eq!(target(&out, 4, x, y), expected, "trimmed, pixel ({x}, {y})");
        }
    }
    // The same fractions on a texture of other dimensions: a 2 x 2 reload of the same key (red, green / blue, white)
    // sampled over the whole 0..1 draws the whole new image over the same 4 x 4 destination.
    let reload = [picture(2, 2, &[RED, GREEN, BLUE, WHITE])];
    let list = [of(0, rect(0, 0, 4, 4))];
    let out = readback_scene(
        4,
        4,
        [0.0; 3],
        None,
        MetalFilter::Nearest,
        &images(&reload),
        &list,
    )
    .expect("the readback works");
    let expected = [RED, GREEN, BLUE, WHITE];
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(
                target(&out, 4, x, y),
                expected[((y / 2) * 2 + x / 2) as usize],
                "reloaded, ({x}, {y})"
            );
        }
    }
    // And a 4 x 4 whose quadrants are the same four colours draws the same pixels: dimensions do not matter.
    let quadrants: Vec<[u8; 4]> = (0..16)
        .map(|index: u32| {
            expected[usize::try_from((index / 8) * 2 + (index % 4) / 2).expect("small")]
        })
        .collect();
    let larger = [picture(4, 4, &quadrants)];
    let again = readback_scene(
        4,
        4,
        [0.0; 3],
        None,
        MetalFilter::Nearest,
        &images(&larger),
        &list,
    )
    .expect("the readback works");
    assert_eq!(
        out, again,
        "a texture of different dimensions samples the whole new image"
    );
}

/// An unresolved key draws 8 x 8-pixel magenta and black cells, the top-left magenta, over its whole destination,
/// whatever source the instance carried. Negative control: swapping the two colours or the cell size fails.
pub(crate) fn readback_scene_checker() {
    let pixels = checker_pixels();
    assert_eq!(pixels.len(), (CHECKER_SIDE * CHECKER_SIDE * 4) as usize);
    assert_eq!(CHECKER_CELL, 8);
    // The texture itself: the top-left cell magenta, its right and lower neighbours black, opaque everywhere.
    assert_eq!(&pixels[0..4], &MAGENTA);
    assert_eq!(&pixels[(8 * 4)..(8 * 4 + 4)], &BLACK);
    assert_eq!(
        &pixels[((8 * CHECKER_SIDE) * 4) as usize..((8 * CHECKER_SIDE) * 4 + 4) as usize],
        &BLACK
    );
    assert!(pixels.chunks(4).all(|texel| texel[3] == 255));
    // Drawn 1:1 at 64 x 64 and stretched to 128 x 128 (cells of 16 pixels), with a source that must be ignored.
    for (side, cell) in [(64_u32, 8_u32), (128, 16)] {
        let side_signed = i32::try_from(side).expect("small");
        let out = readback_scene(
            side,
            side,
            [0.0; 3],
            None,
            MetalFilter::Nearest,
            &[],
            &[instance(
                ReadbackTexture::Checker,
                rect(0, 0, side_signed, side_signed),
                [0.2, 0.2, 0.3, 0.3],
            )],
        )
        .expect("the readback works");
        for y in 0..side {
            for x in 0..side {
                let expected = if (x / cell + y / cell) % 2 == 0 {
                    MAGENTA
                } else {
                    BLACK
                };
                assert_eq!(
                    target(&out, side, x, y),
                    expected,
                    "checker at {side}, ({x}, {y})"
                );
            }
        }
    }
}

/// An invalid instance (a source that is not finite or outside 0 to 1, an empty source or destination) is skipped, not
/// drawn and not a panic; the instances after it are drawn. Negative control: drawing the invalid ones paints their
/// cell and fails.
pub(crate) fn readback_scene_invalid_instances_are_skipped() {
    let pictures = [solid(1, 1, RED)];
    let nan = instance(
        ReadbackTexture::Image(0),
        rect(0, 0, 2, 2),
        [0.0, 0.0, f32::NAN, 1.0],
    );
    let outside = instance(
        ReadbackTexture::Image(0),
        rect(2, 0, 4, 2),
        [0.0, 0.0, 1.5, 1.0],
    );
    let empty_source = instance(
        ReadbackTexture::Image(0),
        rect(4, 0, 6, 2),
        [0.5, 0.0, 0.5, 1.0],
    );
    let empty_destination = instance(ReadbackTexture::Image(0), rect(6, 0, 6, 2), WHOLE);
    let valid = of(0, rect(6, 2, 8, 4));
    let out = readback_scene(
        8,
        4,
        [0.0; 3],
        None,
        MetalFilter::Nearest,
        &images(&pictures),
        &[nan, outside, empty_source, empty_destination, valid],
    )
    .expect("invalid instances never fail the readback");
    for (x, y) in [
        (0, 0),
        (2, 0),
        (4, 0),
        (6, 0),
        (1, 1),
        (3, 1),
        (5, 1),
        (7, 1),
    ] {
        assert_eq!(target(&out, 8, x, y), BLACK, "({x}, {y}) is untouched");
    }
    assert_eq!(
        target(&out, 8, 7, 3),
        RED,
        "the valid instance after them is drawn"
    );
}

pub(crate) const CASES: [(&str, fn()); 8] = [
    ("readback_scene_painter_order", readback_scene_painter_order),
    (
        "readback_scene_legacy_quad_is_below",
        readback_scene_legacy_quad_is_below,
    ),
    (
        "readback_scene_batching_keeps_painter_order",
        readback_scene_batching_keeps_painter_order,
    ),
    ("readback_scene_filters", readback_scene_filters),
    (
        "readback_scene_checker_stays_sharp_under_a_linear_frame",
        readback_scene_checker_stays_sharp_under_a_linear_frame,
    ),
    (
        "readback_scene_normalised_source",
        readback_scene_normalised_source,
    ),
    ("readback_scene_checker", readback_scene_checker),
    (
        "readback_scene_invalid_instances_are_skipped",
        readback_scene_invalid_instances_are_skipped,
    ),
];
