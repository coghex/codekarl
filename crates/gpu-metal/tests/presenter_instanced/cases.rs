//! The instanced scene (CK-116), off screen: the `readback_scene_*` cases draw a clear, an optional legacy quad and a
//! list of instances through the presenter's own instance layout, batching, pipeline, samplers and encoding
//! (`readback_scene`) and read the pixels back. They open no window (the presenter target's `gpu` set); the windowed
//! cases of the same issue are in `presenter_scene/instanced.rs`. Each names its negative control.

use ck_gpu_metal::{
    CHECKER_CELL, CHECKER_SIDE, DestinationRect, MetalFilter, MetalQuad, PixelSource,
    ReadbackImage, ReadbackInstance, ReadbackPath, ReadbackTexture, checker_pixels, readback_scene,
    readback_scene_counted,
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
        tint: [255; 4],
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

/// Per-instance premultiplied tint reaches the ordinary Metal pipeline, including alpha blending, without splitting a shared-texture run.
pub(crate) fn readback_scene_per_quad_tint_is_exact_and_keeps_one_batch() {
    let white = solid(1, 1, WHITE);
    let list = [
        ReadbackInstance {
            tint: [79, 179, 200, 255],
            ..instance(ReadbackTexture::Image(0), rect(0, 0, 1, 1), WHOLE)
        },
        ReadbackInstance {
            tint: [55, 10, 8, 128],
            ..instance(ReadbackTexture::Image(0), rect(1, 0, 2, 1), WHOLE)
        },
        ReadbackInstance {
            tint: [255, 0, 0, 255],
            ..instance(ReadbackTexture::Image(0), rect(2, 0, 3, 1), WHOLE)
        },
        ReadbackInstance {
            tint: [0, 255, 255, 255],
            ..instance(ReadbackTexture::Image(0), rect(3, 0, 4, 1), WHOLE)
        },
    ];
    let (pixels, counts) = readback_scene_counted(
        4,
        1,
        [0.0; 3],
        None,
        MetalFilter::Nearest,
        &images(&[white]),
        &list,
        ReadbackPath::Ordinary,
    )
    .expect("the readback works");
    assert_eq!(
        counts.batches, 1,
        "tint does not split the same-texture run"
    );
    assert_eq!(at(&pixels, 4, 0, 0), [79, 179, 200, 255]);
    assert_eq!(at(&pixels, 4, 1, 0), [55, 10, 8, 255]);
    assert_eq!(at(&pixels, 4, 2, 0), [255, 0, 0, 255]);
    assert_eq!(at(&pixels, 4, 3, 0), [0, 255, 255, 255]);
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

// ---------------------------------------------------------------------------------------------
// The date readout's pixel font (CK-197)
// ---------------------------------------------------------------------------------------------

/// The shipped font's PNG at `name` under `assets/ui/font`, decoded: its width, height and RGBA8 pixels.
fn font_png(name: &str) -> (u32, u32, Vec<u8>) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/ui/font")
        .join(name);
    let mut reader = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&path).expect("the shipped font image"),
    ))
    .read_info()
    .expect("a PNG");
    let mut bytes = vec![0; reader.output_buffer_size().expect("a size")];
    let info = reader.next_frame(&mut bytes).expect("a frame");
    assert_eq!(info.color_type, png::ColorType::Rgba, "{name}");
    assert_eq!(info.bit_depth, png::BitDepth::Eight, "{name}");
    bytes.truncate(info.buffer_size());
    (info.width, info.height, bytes)
}

/// "9 Oct 768" as the application's `font.layout` lays it out from the committed font (`apps/codekarl` pins the same literals):
/// the character, its x in font pixels and its glyph width. The string is 49 font pixels wide and every glyph 11 tall.
const READOUT: [(char, i32, u32); 7] = [
    ('9', 0, 5),
    ('O', 10, 5),
    ('c', 16, 5),
    ('t', 22, 4),
    ('7', 31, 5),
    ('6', 37, 5),
    ('8', 43, 5),
];

/// The date readout of a fixed string, off screen: a panel one font pixel larger than the text on every side with the glyph quads
/// over it, at two points per font pixel, nearest sampled. Every pixel equals the composite of the font's PNGs computed on the
/// CPU, the text colour is on screen exactly where a set pixel of a glyph lands, and the pixels around the panel are the clear
/// colour. Negative control: drawing the panel after the glyphs fails the first check (it covers them); drawing at one point per
/// font pixel fails the sizes.
#[expect(
    clippy::too_many_lines,
    reason = "the images, the layout, the draw, the CPU composite and the checks read best in one run"
)]
pub(crate) fn readback_scene_pixel_font_readout() {
    const SCALE: i32 = 2;
    const TEXT: [u8; 4] = [0xEC, 0xE3, 0xCB, 255];
    const PANEL: [u8; 4] = [0x2A, 0x21, 0x19, 255];
    let (width, height) = (110_u32, 30_u32);
    let (origin_x, origin_y) = (4_i32, 2_i32);
    let clear = [0.1, 0.2, 0.3];
    let background = {
        let [r, g, b] = clear_bytes(clear);
        [r, g, b, 255]
    };

    // The images: the panel, then one per distinct glyph.
    let mut pictures = vec![{
        let (w, h, bytes) = font_png("panel.png");
        assert_eq!((w, h, bytes.as_slice()), (1, 1, PANEL.as_slice()));
        Picture {
            width: w,
            height: h,
            bytes,
        }
    }];
    let mut glyph_index = std::collections::BTreeMap::new();
    let mut glyphs = Vec::new();
    for (character, x, _) in READOUT {
        let index = *glyph_index.entry(character).or_insert_with(|| {
            let (w, h, bytes) = font_png(&format!("glyphs/U+{:04X}.png", u32::from(character)));
            assert_eq!(h, 11, "{character}");
            pictures.push(Picture {
                width: w,
                height: h,
                bytes: bytes.clone(),
            });
            pictures.len() - 1
        });
        let (w, h) = (pictures[index].width, pictures[index].height);
        let left = origin_x + SCALE + x * SCALE;
        let top = origin_y + SCALE;
        let destination = rect(
            left,
            top,
            left + i32::try_from(w).expect("small") * SCALE,
            top + i32::try_from(h).expect("small") * SCALE,
        );
        glyphs.push((index, destination));
    }
    let panel = rect(
        origin_x,
        origin_y,
        origin_x + (49 + 2) * SCALE,
        origin_y + (11 + 2) * SCALE,
    );

    let draw = |panel_first: bool| {
        let mut list = Vec::new();
        if panel_first {
            list.push(of(0, panel));
        }
        // The glyph images are white; the text colour is the instance's tint.
        list.extend(glyphs.iter().map(|&(index, destination)| ReadbackInstance {
            tint: TEXT,
            ..of(index, destination)
        }));
        if !panel_first {
            list.push(of(0, panel));
        }
        readback_scene(
            width,
            height,
            clear,
            None,
            MetalFilter::Nearest,
            &images(&pictures),
            &list,
        )
        .expect("the readback works")
    };

    // The CPU composite of the same pictures, pixel for pixel.
    let mut expected = vec![background; (width * height) as usize];
    let mut fill = |destination: DestinationRect, picture: &Picture| {
        for y in destination.top..destination.bottom {
            for x in destination.left..destination.right {
                // Nearest: the texel under the middle of the target pixel (the panel is one texel stretched over its rectangle).
                let across = u32::try_from(x - destination.left).expect("inside");
                let down = u32::try_from(y - destination.top).expect("inside");
                let wide = u32::try_from(destination.right - destination.left).expect("a size");
                let high = u32::try_from(destination.bottom - destination.top).expect("a size");
                let px = across * picture.width / wide;
                let py = down * picture.height / high;
                let at_texel = ((py * picture.width + px) * 4) as usize;
                let texel = &picture.bytes[at_texel..at_texel + 4];
                if texel[3] != 0 {
                    expected[(u32::try_from(y).expect("inside") * width
                        + u32::try_from(x).expect("inside"))
                        as usize] = [texel[0], texel[1], texel[2], texel[3]];
                }
            }
        }
    };
    fill(panel, &pictures[0]);
    for &(index, destination) in &glyphs {
        // The CPU composite of a tinted glyph: its opaque white texels are the text colour (the tint), the clear ones stay clear.
        let tinted = Picture {
            width: pictures[index].width,
            height: pictures[index].height,
            bytes: pictures[index]
                .bytes
                .chunks_exact(4)
                .flat_map(|texel| match texel {
                    [255, 255, 255, 255] => TEXT,
                    [_, _, _, 0] => [0; 4],
                    other => panic!("a glyph texel is opaque white or clear: {other:?}"),
                })
                .collect(),
        };
        fill(destination, &tinted);
    }

    let pixels = draw(true);
    for y in 0..height {
        for x in 0..width {
            assert_eq!(
                target(&pixels, width, x, y),
                expected[(y * width + x) as usize],
                "({x}, {y})"
            );
        }
    }
    // The text colour is where the glyphs' set pixels are, four target pixels each: nothing was dropped or covered.
    let set: usize = glyphs
        .iter()
        .map(|&(index, _)| {
            pictures[index]
                .bytes
                .chunks_exact(4)
                .filter(|texel| texel[3] != 0)
                .count()
        })
        .sum();
    let text = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .filter(|&(x, y)| target(&pixels, width, x, y) == TEXT)
        .count();
    assert_eq!(
        text,
        set * 4,
        "the text colour covers exactly the glyphs' set pixels at two points per font pixel"
    );
    // Around the panel the target is the clear colour, and the panel's corner is the panel colour.
    assert_eq!(target(&pixels, width, 0, 0), background);
    assert_eq!(target(&pixels, width, width - 1, height - 1), background);
    assert_eq!(target(&pixels, width, 4, 2), PANEL);
    // Negative control: the panel drawn last covers the text.
    let covered = draw(false);
    assert_eq!(
        target(&covered, width, 6 + 2, 4 + 6),
        PANEL,
        "a panel drawn last hides the glyphs"
    );
    assert_ne!(covered, pixels);
}

pub(crate) const CASES: [(&str, fn()); 10] = [
    ("readback_scene_painter_order", readback_scene_painter_order),
    (
        "readback_scene_per_quad_tint_is_exact_and_keeps_one_batch",
        readback_scene_per_quad_tint_is_exact_and_keeps_one_batch,
    ),
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
    (
        "readback_scene_pixel_font_readout",
        readback_scene_pixel_font_readout,
    ),
];
