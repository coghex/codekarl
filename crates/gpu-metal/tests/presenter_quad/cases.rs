//! The textured quad, tested as cases of the presenter test target (see `tests/presenter.rs` for how a case
//! runs: in a worker, under Metal validation and the Main Thread Checker, twice, once with the owner typing).
//!
//! The `readback_quad_*` cases open no window: they draw into an off-screen texture with the presenter's own
//! pipeline and read the pixels back. The `quad_*` cases start a presenter with a quad slot on a real layer
//! and watch it through the observer's counters and the drawn generation.

use ck_gpu_metal::{
    DestinationRect, MetalFilter, MetalQuad, MetalQuadSlot, MetalScene, PixelSource, SourceRect,
    readback_quad,
};

use super::{
    BOUND, CleanupResult, Duration, Gate, Injection, MetalPresenter, NeverComplete,
    PresenterReport, Probes, Rig, TestHooks, end, readback_clear, reports_until,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// A small count as a float.
fn float(value: u32) -> f32 {
    f32::from(u16::try_from(value).expect("a small number"))
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

/// A small float rounded down, as an integer.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the values are a few texels, nowhere near the limits of i32"
)]
fn floor(value: f32) -> i32 {
    value.floor() as i32
}

/// A pixel dimension as the signed number a destination rectangle uses.
fn signed(value: u32) -> i32 {
    i32::try_from(value).expect("a small pixel size")
}

/// An image the tests build: RGBA8 bytes, and a flag set when it is dropped.
struct Image {
    bytes: Vec<u8>,
    dropped: Option<Arc<AtomicBool>>,
}

impl PixelSource for Image {
    fn pixels(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        if let Some(dropped) = &self.dropped {
            dropped.store(true, Ordering::SeqCst);
        }
    }
}

fn image(pixels: &[[u8; 4]]) -> Arc<Image> {
    Arc::new(Image {
        bytes: pixels.concat(),
        dropped: None,
    })
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

/// A quad of `image` (a `width` x `height` picture) drawn whole into `destination`.
fn whole(
    image: &Arc<Image>,
    (width, height): (u32, u32),
    destination: DestinationRect,
    filter: MetalFilter,
) -> MetalQuad {
    let pixels: Arc<dyn PixelSource> = Arc::clone(image) as Arc<dyn PixelSource>;
    MetalQuad::new(
        pixels,
        width,
        height,
        destination,
        source(0.0, 0.0, float(width), float(height)),
        filter,
    )
    .expect("a valid quad")
}

/// One opaque or translucent pixel, premultiplied.
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

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

/// The clear colour as the 8-bit values the target holds (unorm rounding).
fn clear_bytes(rgb: [f32; 3]) -> [u8; 3] {
    rgb.map(|channel| byte(channel * 255.0))
}

// ---------------------------------------------------------------------------------------------
// Readback cases (no window)
// ---------------------------------------------------------------------------------------------

/// Nearest: a 2x2 checker drawn at 8x8 gives exact 4x4 blocks, and the target outside it is the clear.
pub(crate) fn readback_quad_nearest() {
    let picture = image(&[RED, GREEN, BLUE, WHITE]);
    let clear = [0.25, 0.5, 0.75];
    let quad = whole(&picture, (2, 2), rect(0, 0, 8, 8), MetalFilter::Nearest);
    let pixels = readback_quad(8, 8, clear, Some(&quad)).expect("the readback works");
    let expected = [RED, GREEN, BLUE, WHITE];
    for y in 0..8 {
        for x in 0..8 {
            assert_eq!(
                at(&pixels, 8, x, y),
                expected[((y / 4) * 2 + x / 4) as usize],
                "pixel ({x}, {y})"
            );
        }
    }
    // The same picture drawn into the middle of a larger target leaves the rest as the clear.
    let inset = whole(&picture, (2, 2), rect(4, 4, 12, 12), MetalFilter::Nearest);
    let pixels = readback_quad(16, 16, clear, Some(&inset)).expect("the readback works");
    let [red, green, blue] = clear_bytes(clear);
    for y in 0..16 {
        for x in 0..16 {
            let inside = (4..12).contains(&x) && (4..12).contains(&y);
            let expected = if inside {
                [RED, GREEN, BLUE, WHITE][(((y - 4) / 4) * 2 + (x - 4) / 4) as usize]
            } else {
                [red, green, blue, 255]
            };
            assert_eq!(at(&pixels, 16, x, y), expected, "pixel ({x}, {y})");
        }
    }
}

/// Half-transparent pixels over a known clear colour are exact to the premultiplication rule:
/// `result = source + clear * (255 - alpha) / 255`, per channel, rounded to the nearest 8-bit value (the GPU's
/// blend rounds in fixed point, so the test allows one step either way).
pub(crate) fn readback_quad_alpha() {
    let clear = [0.25, 0.5, 0.75];
    let dst = clear_bytes(clear);
    // Premultiplied pixels: red at 50%, green at 25%, fully transparent (all zeros), and opaque blue.
    let pixels_in = [[128, 0, 0, 128], [0, 64, 0, 64], [0, 0, 0, 0], BLUE];
    let picture = image(&pixels_in);
    let quad = whole(&picture, (4, 1), rect(0, 0, 4, 1), MetalFilter::Nearest);
    let out = readback_quad(4, 1, clear, Some(&quad)).expect("the readback works");
    for (x, pixel) in pixels_in.iter().enumerate() {
        let alpha = f32::from(pixel[3]);
        let expected: Vec<u8> = (0..3)
            .map(|channel| {
                let blended =
                    f32::from(pixel[channel]) + f32::from(dst[channel]) * (255.0 - alpha) / 255.0;
                byte(blended)
            })
            .collect();
        let seen = at(&out, 4, u32::try_from(x).expect("a small index"), 0);
        for channel in 0..3 {
            assert!(
                seen[channel].abs_diff(expected[channel]) <= 1,
                "pixel {x} channel {channel}: {} against {}",
                seen[channel],
                expected[channel]
            );
        }
        assert_eq!(seen[3], 255, "the target stays opaque");
    }
    // The fully transparent pixel is exactly the clear.
    assert_eq!(at(&out, 4, 2, 0), [dst[0], dst[1], dst[2], 255]);
}

/// Linear differs from nearest at the block edges and stays within a stated tolerance of the interpolation.
pub(crate) fn readback_quad_linear() {
    // A 2x2 black/white checker drawn at 8x8, sampled linearly with clamp-to-edge.
    let picture = image(&[BLACK, WHITE, WHITE, BLACK]);
    let clear = [0.0; 3];
    let nearest = whole(&picture, (2, 2), rect(0, 0, 8, 8), MetalFilter::Nearest);
    let linear = whole(&picture, (2, 2), rect(0, 0, 8, 8), MetalFilter::Linear);
    let hard = readback_quad(8, 8, clear, Some(&nearest)).expect("the readback works");
    let soft = readback_quad(8, 8, clear, Some(&linear)).expect("the readback works");
    // The expected interpolation: a destination pixel's centre in texel units, bilinear weights, edges
    // clamped. Tolerance: 3 of 255 (the GPU interpolates with a fixed-point fraction of a few bits).
    let texel = |x: i32, y: i32| -> f32 {
        let x = x.clamp(0, 1);
        let y = y.clamp(0, 1);
        if (x + y) % 2 == 0 { 0.0 } else { 255.0 }
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
            let seen = f32::from(at(&soft, 8, x, y)[0]);
            assert!(
                (seen - expected).abs() <= 3.0,
                "pixel ({x}, {y}): {seen} against {expected}"
            );
            if at(&soft, 8, x, y) != at(&hard, 8, x, y) {
                differing += 1;
            }
        }
    }
    // Away from the edges the two agree: the middle of a block is the block's own colour.
    assert_eq!(at(&soft, 8, 1, 1)[0], 0);
    assert_eq!(at(&soft, 8, 6, 1)[0], 255);
    // The blocks' edges differ: the two middle columns and rows, at least.
    assert!(
        differing >= 16,
        "linear and nearest differ in {differing} pixels"
    );
    assert_ne!(at(&soft, 8, 3, 1), at(&hard, 8, 3, 1));
    assert_ne!(at(&soft, 8, 4, 1), at(&hard, 8, 4, 1));
}

/// A clipped source rectangle gives exactly the trimmed image, unstretched.
pub(crate) fn readback_quad_clipped_source() {
    // A 4x4 picture whose pixel (x, y) is (10 + 40x, 10 + 40y, 200, 255).
    let pixels: Vec<[u8; 4]> = (0_u8..16)
        .map(|index| {
            let (x, y) = (index % 4, index / 4);
            [10 + 40 * x, 10 + 40 * y, 200, 255]
        })
        .collect();
    let picture = image(&pixels);
    let clear = [0.0, 0.0, 0.0];
    let pixel = |x: u32, y: u32| pixels[(y * 4 + x) as usize];
    // The middle 2x2 of the picture, drawn 1:1 into the middle of a 4x4 target.
    let pixels_in = Arc::clone(&picture) as Arc<dyn PixelSource>;
    let quad = MetalQuad::new(
        Arc::clone(&pixels_in),
        4,
        4,
        rect(1, 1, 3, 3),
        source(1.0, 1.0, 3.0, 3.0),
        MetalFilter::Nearest,
    )
    .expect("a valid quad");
    let out = readback_quad(4, 4, clear, Some(&quad)).expect("the readback works");
    for y in 0..4 {
        for x in 0..4 {
            let expected = if (1..3).contains(&x) && (1..3).contains(&y) {
                pixel(x, y)
            } else {
                [0, 0, 0, 255]
            };
            assert_eq!(at(&out, 4, x, y), expected, "pixel ({x}, {y})");
        }
    }
    // The right two columns of the picture, drawn at twice the size: each pixel becomes a 2x2 block.
    let right = MetalQuad::new(
        pixels_in,
        4,
        4,
        rect(0, 0, 4, 8),
        source(2.0, 0.0, 4.0, 4.0),
        MetalFilter::Nearest,
    )
    .expect("a valid quad");
    let out = readback_quad(4, 8, clear, Some(&right)).expect("the readback works");
    for y in 0..8 {
        for x in 0..4 {
            assert_eq!(
                at(&out, 4, x, y),
                pixel(2 + x / 2, y / 2),
                "pixel ({x}, {y})"
            );
        }
    }
}

/// No quad gives exactly the clear, the same bytes `readback_clear` reads.
pub(crate) fn readback_quad_none() {
    let clear = [0.2, 0.4, 0.6];
    let with = readback_quad(16, 8, clear, None).expect("the readback works");
    let without = readback_clear(16, 8, clear).expect("the readback works");
    assert_eq!(with, without);
}

/// One pixel source, two valid layouts of the same sixteen bytes (2x2 and 1x4): each draws its own picture,
/// exactly.
pub(crate) fn readback_quad_layouts_of_one_source() {
    let picture = image(&[RED, GREEN, BLUE, WHITE]);
    let clear = [0.0; 3];
    // 2x2: red, green / blue, white, each pixel a 2x2 block of an 4x4 target.
    let square = whole(&picture, (2, 2), rect(0, 0, 4, 4), MetalFilter::Nearest);
    let out = readback_quad(4, 4, clear, Some(&square)).expect("the readback works");
    let expected = [RED, GREEN, BLUE, WHITE];
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(
                at(&out, 4, x, y),
                expected[((y / 2) * 2 + x / 2) as usize],
                "square ({x}, {y})"
            );
        }
    }
    // 1x4: a column, red, green, blue, white from the top, each pixel a row of a 2x4 target.
    let column = whole(&picture, (1, 4), rect(0, 0, 2, 4), MetalFilter::Nearest);
    let out = readback_quad(2, 4, clear, Some(&column)).expect("the readback works");
    for y in 0..4 {
        for x in 0..2 {
            assert_eq!(at(&out, 2, x, y), expected[y as usize], "column ({x}, {y})");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Presenter cases (a real layer)
// ---------------------------------------------------------------------------------------------

/// A slot, and a presenter started with it by the production entry point's path.
fn hooks_with(probes: &Probes, slot: &Arc<MetalQuadSlot>) -> TestHooks {
    TestHooks {
        quads: Some(Arc::clone(slot)),
        ..probes.hooks()
    }
}

/// A quad published before start is in the first frame: its generation is the first one drawn.
pub(crate) fn quad_in_the_first_frame() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let picture = image(&[RED, GREEN, BLUE, WHITE]);
    let (width, height) = (signed(rig.pixels.0), signed(rig.pixels.1));
    slot.publish(MetalScene::new(
        7,
        Some(whole(
            &picture,
            (2, 2),
            rect(0, 0, width / 2, height / 2),
            MetalFilter::Nearest,
        )),
    ));
    let mut presenter = probes.start_ok(&rig, hooks_with(&probes, &slot));
    assert!(
        rig.pump_until(BOUND, || presenter.drawn_generation() == Some(7)),
        "the first frame has the quad: {:?}",
        presenter.drawn_generation()
    );
    assert_eq!(probes.frames(), 1, "it was the first and only frame");
    assert_eq!(probes.observer.quad_draws(), 1);
    assert_eq!(probes.observer.quad_textures_created(), 1);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// Publishing during a gate-stalled frame allocates nothing until a frame is drawn.
pub(crate) fn quad_publish_while_stalled() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let gate = Arc::new(Gate::new());
    let hooks = TestHooks {
        gate: Some(Arc::clone(&gate)),
        ..hooks_with(&probes, &slot)
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    gate.open(1);
    assert!(rig.pump_until(BOUND, || probes.frames() == 1));
    assert_eq!(probes.observer.quad_textures_created(), 0, "no quad yet");
    let picture = image(&[RED, GREEN, BLUE, WHITE]);
    for generation in 1..=5 {
        slot.publish(MetalScene::new(
            generation,
            Some(whole(
                &picture,
                (2, 2),
                rect(0, 0, 4, 4),
                MetalFilter::Nearest,
            )),
        ));
        slot.publish(MetalScene::new(
            generation + 100,
            Some(whole(
                &image(&[BLACK; 4]),
                (2, 2),
                rect(0, 0, 4, 4),
                MetalFilter::Linear,
            )),
        ));
    }
    rig.pump_for(Duration::from_millis(300));
    assert_eq!(probes.frames(), 1, "the gate holds every frame");
    assert_eq!(
        probes.observer.quad_textures_created(),
        0,
        "no texture is allocated while no frame is drawn"
    );
    gate.open(1);
    assert!(rig.pump_until(BOUND, || probes.frames() == 2));
    assert_eq!(
        probes.observer.quad_textures_created(),
        1,
        "one frame, one upload"
    );
    assert_eq!(
        presenter.drawn_generation(),
        Some(105),
        "the latest of ten publishes"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// Replacing the image again and again never has more than two quad textures alive.
pub(crate) fn quad_replacement_bound() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let mut presenter = probes.start_ok(&rig, hooks_with(&probes, &slot));
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    for generation in 1..=12_u64 {
        let picture = image(&[[u8::try_from(generation).expect("a small number"), 0, 0, 255]; 4]);
        slot.publish(MetalScene::new(
            generation,
            Some(whole(
                &picture,
                (2, 2),
                rect(0, 0, 8, 8),
                MetalFilter::Nearest,
            )),
        ));
        assert!(
            rig.pump_until(BOUND, || presenter.drawn_generation() == Some(generation)),
            "generation {generation}"
        );
    }
    let observer = &probes.observer;
    assert_eq!(observer.quad_textures_created(), 12);
    assert_eq!(
        observer.quad_textures_max(),
        2,
        "the replaced one is alive while the new one is made"
    );
    assert_eq!(observer.quad_textures_alive(), 1);
    assert_eq!(observer.quad_retired(), 11);
    assert_eq!(observer.quad_textures_released(), 11);
    assert_eq!(observer.quad_released_early(), 0);
    assert_eq!(
        observer.quad_admission_waits(),
        0,
        "each retired texture's token had arrived"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(observer.quad_textures_alive(), 0);
}

/// A retired texture is released only after its token, and while it waits no further texture is made.
pub(crate) fn quad_retired_texture_waits_for_its_token() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let hooks = TestHooks {
        // A texture retired now is held as if a command buffer still used it, for 600 ms.
        retired_token_delay: Duration::from_millis(600),
        ..hooks_with(&probes, &slot)
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    let publish = |generation: u64, shade: u8| {
        let picture = image(&[[shade, 0, 0, 255]; 4]);
        slot.publish(MetalScene::new(
            generation,
            Some(whole(
                &picture,
                (2, 2),
                rect(0, 0, 8, 8),
                MetalFilter::Nearest,
            )),
        ));
    };
    publish(1, 10);
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(1)));
    publish(2, 20);
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(2)));
    let observer = &probes.observer;
    assert_eq!(observer.quad_textures_created(), 2);
    assert_eq!(
        observer.quad_textures_alive(),
        2,
        "the retired texture waits for its token"
    );
    // A third image arrives while the retired texture still waits: it must not be made yet.
    publish(3, 30);
    rig.pump_for(Duration::from_millis(250));
    assert_eq!(
        observer.quad_textures_created(),
        2,
        "admission holds the new texture back"
    );
    assert_eq!(presenter.drawn_generation(), Some(2));
    assert!(observer.quad_admission_waits() >= 1);
    assert_eq!(observer.quad_textures_released(), 0, "not before the token");
    // The token arrives; the retired texture goes; the third image is drawn.
    assert!(
        rig.pump_until(BOUND, || presenter.drawn_generation() == Some(3)),
        "the frame draws once the token arrives"
    );
    assert_eq!(observer.quad_textures_created(), 3);
    assert_eq!(observer.quad_textures_max(), 2, "never more than two");
    assert_eq!(observer.quad_released_early(), 0);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(observer.quad_textures_alive(), 0);
    assert_eq!(
        observer.quad_textures_released(),
        3,
        "every texture, after its token"
    );
}

/// A change of position or filter draws again without uploading.
pub(crate) fn quad_position_and_filter_do_not_upload() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let mut presenter = probes.start_ok(&rig, hooks_with(&probes, &slot));
    let picture = image(&[RED, GREEN, BLUE, WHITE]);
    let variants = [
        (rect(0, 0, 8, 8), MetalFilter::Nearest),
        (rect(10, 10, 30, 30), MetalFilter::Nearest),
        (rect(10, 10, 30, 30), MetalFilter::Linear),
        (rect(-5, -5, 20, 20), MetalFilter::Linear),
    ];
    for (index, (destination, filter)) in variants.into_iter().enumerate() {
        let generation = index as u64 + 1;
        slot.publish(MetalScene::new(
            generation,
            Some(whole(&picture, (2, 2), destination, filter)),
        ));
        assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(generation)));
    }
    let observer = &probes.observer;
    assert_eq!(observer.quad_textures_created(), 1, "one image, one upload");
    assert_eq!(observer.quad_retired(), 0);
    assert_eq!(observer.quad_draws(), 4);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// The resident texture is the source **and its layout**: the same source described as 2x2 and then as 1x4 is
/// uploaded again, with the right shape; a change of position or filter alone is not.
pub(crate) fn quad_same_source_new_layout_uploads_again() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let mut presenter = probes.start_ok(&rig, hooks_with(&probes, &slot));
    let picture = image(&[RED, GREEN, BLUE, WHITE]);
    let observer = &probes.observer;
    let publish = |generation: u64, layout: (u32, u32), destination: DestinationRect, filter| {
        slot.publish(MetalScene::new(
            generation,
            Some(whole(&picture, layout, destination, filter)),
        ));
    };
    publish(1, (2, 2), rect(0, 0, 8, 8), MetalFilter::Nearest);
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(1)));
    assert_eq!(observer.quad_textures_created(), 1);
    assert_eq!(observer.quad_last_texture_size(), (2, 2));
    // The same Arc, the other layout of the same bytes: a new texture of the new shape.
    publish(2, (1, 4), rect(0, 0, 8, 8), MetalFilter::Nearest);
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(2)));
    assert_eq!(
        observer.quad_textures_created(),
        2,
        "a new layout is a new upload"
    );
    assert_eq!(observer.quad_last_texture_size(), (1, 4));
    // The same layout again, moved and filtered differently: no upload.
    publish(3, (1, 4), rect(2, 2, 10, 10), MetalFilter::Linear);
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(3)));
    assert_eq!(
        observer.quad_textures_created(),
        2,
        "position and filter alone do not upload"
    );
    // And back to the first layout.
    publish(4, (2, 2), rect(0, 0, 8, 8), MetalFilter::Nearest);
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(4)));
    assert_eq!(observer.quad_textures_created(), 3);
    assert_eq!(observer.quad_last_texture_size(), (2, 2));
    assert_eq!(observer.quad_textures_max(), 2);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(observer.quad_textures_alive(), 0);
}

/// A retired texture's token that arrives between the frame step's look at it and its admission decision
/// must not let a third texture in: the decision is the look's own, so the step waits and the next one reaps.
/// (`pause_after_reap_for_token` places the completion deterministically; the case before it is the control
/// without the pause.)
pub(crate) fn quad_admission_is_one_decision() {
    for pausing in [false, true] {
        let mut rig = Rig::open();
        let probes = Probes::new();
        let slot = MetalQuadSlot::new();
        let hooks = TestHooks {
            retired_token_delay: Duration::from_millis(300),
            pause_after_reap_for_token: pausing,
            ..hooks_with(&probes, &slot)
        };
        let mut presenter = probes.start_ok(&rig, hooks);
        for (generation, shade) in [(1, 10), (2, 20), (3, 30), (4, 40)] {
            let picture = image(&[[shade, 0, 0, 255]; 4]);
            slot.publish(MetalScene::new(
                generation,
                Some(whole(
                    &picture,
                    (2, 2),
                    rect(0, 0, 8, 8),
                    MetalFilter::Nearest,
                )),
            ));
            // Published at once, one after the other, while the previous retirement still waits.
            if generation >= 3 {
                continue;
            }
            assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(generation)));
        }
        assert!(
            rig.pump_until(BOUND, || presenter.drawn_generation() == Some(4)),
            "pausing {pausing}"
        );
        let observer = &probes.observer;
        assert!(
            observer.quad_textures_max() <= 2,
            "pausing {pausing}: {}",
            observer.quad_textures_max()
        );
        assert_eq!(observer.quad_released_early(), 0, "pausing {pausing}");
        assert_eq!(
            end(&mut rig, &mut presenter),
            CleanupResult::Proven,
            "pausing {pausing}"
        );
        // Every texture made was released exactly once, and none is counted alive.
        assert_eq!(observer.quad_textures_alive(), 0, "pausing {pausing}");
        assert_eq!(
            observer.quad_textures_released(),
            observer.quad_textures_created(),
            "pausing {pausing}"
        );
        assert_eq!(observer.quad_textures_max(), 2, "pausing {pausing}");
    }
}

/// The render thread keeps no reference to a pixel source once it has uploaded it.
pub(crate) fn quad_source_is_not_held_after_upload() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let mut presenter = probes.start_ok(&rig, hooks_with(&probes, &slot));
    let dropped = Arc::new(AtomicBool::new(false));
    let picture = Arc::new(Image {
        bytes: [RED, GREEN, BLUE, WHITE].concat(),
        dropped: Some(Arc::clone(&dropped)),
    });
    slot.publish(MetalScene::new(
        1,
        Some(whole(
            &picture,
            (2, 2),
            rect(0, 0, 8, 8),
            MetalFilter::Nearest,
        )),
    ));
    drop(picture);
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(1)));
    assert!(
        !dropped.load(Ordering::SeqCst),
        "the slot still holds the quad (and so the source)"
    );
    // Only the slot holds it now: replacing the slot's value must free it, with the texture still resident.
    slot.publish(MetalScene::new(2, None));
    assert!(
        dropped.load(Ordering::SeqCst),
        "the render thread held a reference after its upload"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// Removing the quad retires the texture, and the empty change's generation is the drawn one.
pub(crate) fn quad_removal() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let mut presenter = probes.start_ok(&rig, hooks_with(&probes, &slot));
    assert!(
        rig.pump_until(BOUND, || presenter.drawn_generation() == Some(0)),
        "an empty first frame"
    );
    let picture = image(&[RED, GREEN, BLUE, WHITE]);
    slot.publish(MetalScene::new(
        3,
        Some(whole(
            &picture,
            (2, 2),
            rect(0, 0, 8, 8),
            MetalFilter::Nearest,
        )),
    ));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(3)));
    slot.publish(MetalScene::new(5, None));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(5)));
    let observer = &probes.observer;
    assert_eq!(observer.quad_retired(), 1);
    assert_eq!(
        observer.quad_textures_alive(),
        0,
        "released after its token"
    );
    assert_eq!(observer.quad_draws(), 1);
    // A scene that shows nothing because its quad was clipped away entirely is the same to the presenter: it
    // draws only the clear, and the drawn generation is that scene's own (the generation travels without a
    // quad). A later scene with a quad uploads and draws again.
    slot.publish(MetalScene::new(6, None));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(6)));
    assert_eq!(
        observer.quad_draws(),
        1,
        "no quad in the frame of generation 6"
    );
    slot.publish(MetalScene::new(
        7,
        Some(whole(
            &picture,
            (2, 2),
            rect(0, 0, 8, 8),
            MetalFilter::Nearest,
        )),
    ));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(7)));
    assert_eq!(observer.quad_draws(), 2);
    assert_eq!(
        observer.quad_textures_created(),
        2,
        "the removed image is uploaded again"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A proven end releases every quad holder; an unproven one releases none.
pub(crate) fn quad_teardown() {
    // Proven.
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let picture = image(&[RED, GREEN, BLUE, WHITE]);
    slot.publish(MetalScene::new(
        1,
        Some(whole(
            &picture,
            (2, 2),
            rect(0, 0, 8, 8),
            MetalFilter::Nearest,
        )),
    ));
    let mut presenter = probes.start_ok(&rig, hooks_with(&probes, &slot));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(1)));
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    let observer = &probes.observer;
    assert_eq!(observer.quad_textures_alive(), 0);
    assert_eq!(observer.quad_textures_released(), 1);
    assert_eq!(
        observer.quad_holders_released(),
        4,
        "the texture, the pipeline and the two samplers"
    );
    // After the end, publishing touches nothing.
    slot.publish(MetalScene::new(
        2,
        Some(whole(
            &picture,
            (2, 2),
            rect(0, 0, 8, 8),
            MetalFilter::Linear,
        )),
    ));
    assert_eq!(presenter.drawn_generation(), Some(1));
    assert_eq!(observer.quad_textures_created(), 1);
    drop(presenter);
    drop(rig);

    // Unproven: the command buffers' completions never arrive, and the stop budget passes.
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    slot.publish(MetalScene::new(
        1,
        Some(whole(
            &picture,
            (2, 2),
            rect(0, 0, 8, 8),
            MetalFilter::Nearest,
        )),
    ));
    let hooks = TestHooks {
        never_complete: NeverComplete::Commands,
        stop_budget: Some(Duration::from_millis(200)),
        ..hooks_with(&probes, &slot)
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(1)));
    assert!(matches!(
        end(&mut rig, &mut presenter),
        CleanupResult::Unproven { .. }
    ));
    let observer = &probes.observer;
    assert_eq!(
        observer.quad_textures_released(),
        0,
        "no texture was released"
    );
    assert_eq!(
        observer.quad_holders_released(),
        0,
        "no holder was released"
    );
    assert_eq!(observer.quad_textures_alive(), 1);
    assert_eq!(observer.holders_released(), 0);
}

/// A retired texture still waiting for its token: a proven end waits for the token and releases it after;
/// an unproven end releases nothing, not even a texture whose token arrives during the teardown.
pub(crate) fn quad_teardown_with_a_retired_texture() {
    let picture_of = |shade: u8| image(&[[shade, 0, 0, 255]; 4]);
    let scene = |generation: u64, shade: u8| {
        MetalScene::new(
            generation,
            Some(whole(
                &picture_of(shade),
                (2, 2),
                rect(0, 0, 8, 8),
                MetalFilter::Nearest,
            )),
        )
    };
    // Proven: the stop comes while the first texture, retired by the second image, waits for its token.
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let hooks = TestHooks {
        retired_token_delay: Duration::from_millis(500),
        ..hooks_with(&probes, &slot)
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    slot.publish(scene(1, 10));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(1)));
    slot.publish(scene(2, 20));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(2)));
    let observer = &probes.observer;
    assert_eq!(
        observer.quad_textures_alive(),
        2,
        "the retired texture waits"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(
        observer.quad_released_early(),
        0,
        "the teardown waited for the token"
    );
    assert_eq!(observer.quad_textures_alive(), 0);
    assert_eq!(observer.quad_textures_released(), 2);
    drop(presenter);
    drop(rig);

    // Unproven: the drawables' presented handlers never arrive, so the end cannot be proven; the retired
    // texture's token does arrive, 300 ms in, during the teardown, and nothing is released all the same.
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let hooks = TestHooks {
        retired_token_delay: Duration::from_millis(300),
        never_complete: NeverComplete::Drawables,
        stop_budget: Some(Duration::from_millis(900)),
        ..hooks_with(&probes, &slot)
    };
    let mut presenter = probes.start_ok(&rig, hooks);
    slot.publish(scene(1, 10));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(1)));
    slot.publish(scene(2, 20));
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(2)));
    assert!(matches!(
        end(&mut rig, &mut presenter),
        CleanupResult::Unproven { .. }
    ));
    let observer = &probes.observer;
    assert_eq!(
        observer.quad_textures_released(),
        0,
        "an unproven end releases no texture"
    );
    assert_eq!(observer.quad_holders_released(), 0);
    assert_eq!(observer.quad_textures_alive(), 2);
}

/// A texture that cannot be allocated, a pipeline that cannot be created and a quad that cannot be encoded
/// each give one `Failed`, draw nothing more, and still end proven.
pub(crate) fn quad_failures() {
    let mut rig = Rig::open();
    let picture = image(&[RED, GREEN, BLUE, WHITE]);
    let faults: [(&str, &str, Injection); 3] = [
        ("texture allocation", "2 x 2", |hooks| {
            hooks.fail_texture_allocation_frames = 1;
        }),
        ("pipeline", "pipeline", |hooks| hooks.fail_pipeline = true),
        ("quad encoding", "could not be encoded", |hooks| {
            hooks.fail_quad_encode_frames = 1;
        }),
    ];
    for (name, wanted, inject) in faults {
        let probes = Probes::new();
        let slot = MetalQuadSlot::new();
        slot.publish(MetalScene::new(
            1,
            Some(whole(
                &picture,
                (2, 2),
                rect(0, 0, 8, 8),
                MetalFilter::Nearest,
            )),
        ));
        let mut hooks = hooks_with(&probes, &slot);
        inject(&mut hooks);
        let mut presenter = probes.start_ok(&rig, hooks);
        let mut reports = Vec::new();
        assert!(
            reports_until(&mut rig, &presenter, &mut reports, 1),
            "{name}"
        );
        rig.pump_for(Duration::from_millis(200));
        while let Some(report) = presenter.take_report() {
            reports.push(report);
        }
        assert_eq!(reports.len(), 1, "{name}: one report: {reports:?}");
        match &reports[0] {
            PresenterReport::Failed { message } => assert!(
                message.contains(wanted),
                "{name}: the message names it: {message}"
            ),
            PresenterReport::FirstFrameQueued => panic!("{name}: a first frame was queued"),
        }
        assert_eq!(probes.frames(), 0, "{name}: nothing was drawn");
        assert_eq!(
            end(&mut rig, &mut presenter),
            CleanupResult::Proven,
            "{name}"
        );
        assert_eq!(probes.observer.quad_textures_alive(), 0, "{name}");
    }
}

/// `start` without a slot is the clear-only presenter it always was.
pub(crate) fn quad_start_without_a_slot() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    // SAFETY: `rig.layer` is the live `CAMetalLayer` of the rig's window, which the rig keeps until after the
    // presenter ended `Proven` (checked below).
    let mut presenter =
        unsafe { MetalPresenter::start(rig.layer, rig.pixels, [0.1, 0.2, 0.3], probes.wake()) }
            .expect("the presenter starts");
    let mut reports = Vec::new();
    assert!(reports_until(&mut rig, &presenter, &mut reports, 1));
    assert_eq!(reports, [PresenterReport::FirstFrameQueued]);
    assert_eq!(presenter.drawn_generation(), None, "no slot, no generation");
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// `start_2d` is `start` plus the slot.
pub(crate) fn quad_start_2d() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let slot = MetalQuadSlot::new();
    let picture = image(&[RED, GREEN, BLUE, WHITE]);
    slot.publish(MetalScene::new(
        9,
        Some(whole(
            &picture,
            (2, 2),
            rect(0, 0, 8, 8),
            MetalFilter::Nearest,
        )),
    ));
    // SAFETY: as in `quad_start_without_a_slot`.
    let mut presenter = unsafe {
        MetalPresenter::start_2d(
            rig.layer,
            rig.pixels,
            [0.1, 0.2, 0.3],
            probes.wake(),
            Arc::clone(&slot),
        )
    }
    .expect("the presenter starts");
    assert!(rig.pump_until(BOUND, || presenter.drawn_generation() == Some(9)));
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// The cases, in the order they run.
pub(crate) const CASES: [(&str, fn()); 19] = [
    ("readback_quad_nearest", readback_quad_nearest),
    ("readback_quad_alpha", readback_quad_alpha),
    ("readback_quad_linear", readback_quad_linear),
    ("readback_quad_clipped_source", readback_quad_clipped_source),
    ("readback_quad_none", readback_quad_none),
    (
        "readback_quad_layouts_of_one_source",
        readback_quad_layouts_of_one_source,
    ),
    ("quad_in_the_first_frame", quad_in_the_first_frame),
    ("quad_publish_while_stalled", quad_publish_while_stalled),
    ("quad_replacement_bound", quad_replacement_bound),
    (
        "quad_admission_is_one_decision",
        quad_admission_is_one_decision,
    ),
    (
        "quad_same_source_new_layout_uploads_again",
        quad_same_source_new_layout_uploads_again,
    ),
    (
        "quad_retired_texture_waits_for_its_token",
        quad_retired_texture_waits_for_its_token,
    ),
    (
        "quad_position_and_filter_do_not_upload",
        quad_position_and_filter_do_not_upload,
    ),
    (
        "quad_source_is_not_held_after_upload",
        quad_source_is_not_held_after_upload,
    ),
    ("quad_removal", quad_removal),
    ("quad_teardown", quad_teardown),
    (
        "quad_teardown_with_a_retired_texture",
        quad_teardown_with_a_retired_texture,
    ),
    ("quad_failures", quad_failures),
    ("quad_start_without_a_slot", quad_start_without_a_slot),
];

/// Cases besides the table (kept apart so the table's length stays a plain constant).
pub(crate) const MORE: [(&str, fn()); 1] = [("quad_start_2d", quad_start_2d)];
