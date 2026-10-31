//! The textured quad, off screen (milestone 4.5, issue 6): the quad type's boundaries, the pixels the shared
//! drawing code produces, the device checks and the readback's ownership rules, all under the validation layer
//! with no window. A submodule of `gpu.rs`, so the group that runs that target runs these.
//!
//! The readback returns `B8G8R8A8_UNORM` bytes, rows top first: a texel `(r, g, b, a)` reads back as
//! `[b, g, r, a]`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ck_gpu_vulkan::{
    DestinationRect, Instance, Loader, MAX_IMAGE_BYTES, MAX_IMAGE_SIDE, MAX_TARGET_SIDE,
    PixelSource, ReadbackHooks, SourceRect, VulkanFilter, VulkanQuad, VulkanQuadError, VulkanScene,
    destroy_surface_and_instance, load_loader, readback_quad, readback_quad_with_test_hooks, vk,
};

use super::{assert_log_clean, destroy, validated};

/// An image's bytes.
struct Bytes(Vec<u8>);

impl PixelSource for Bytes {
    fn pixels(&self) -> &[u8] {
        &self.0
    }
}

/// An RGBA8 image of `width` × `height` whose texel at (x, y) is `texel(x, y)`.
fn image(width: u32, height: u32, texel: impl Fn(u32, u32) -> [u8; 4]) -> Arc<Bytes> {
    let mut bytes = Vec::new();
    for y in 0..height {
        for x in 0..width {
            bytes.extend_from_slice(&texel(x, y));
        }
    }
    Arc::new(Bytes(bytes))
}

fn whole(width: u32, height: u32) -> (DestinationRect, SourceRect) {
    (
        DestinationRect {
            left: 0,
            top: 0,
            right: i32::try_from(width).expect("a small width"),
            bottom: i32::try_from(height).expect("a small height"),
        },
        SourceRect {
            left: 0.0,
            top: 0.0,
            right: f32::from(u16::try_from(width).expect("a small width")),
            bottom: f32::from(u16::try_from(height).expect("a small height")),
        },
    )
}

/// A quad of `pixels` (the `width` × `height` image) drawn into `destination` from `source`.
fn quad(
    pixels: Arc<Bytes>,
    (width, height): (u32, u32),
    destination: DestinationRect,
    source: SourceRect,
    filter: VulkanFilter,
) -> VulkanQuad {
    VulkanQuad::new(pixels, width, height, destination, source, filter).expect("a valid quad")
}

/// The pixel at (x, y) of a `width`-wide BGRA readback, as `[b, g, r, a]`.
fn at(bytes: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * width + x) * 4) as usize;
    [
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]
}

/// A clear colour as the bytes a `B8G8R8A8_UNORM` target stores: `[b, g, r, 255]`, rounded to nearest.
fn clear_bytes(rgb: [f32; 3]) -> [u8; 4] {
    let byte = |value: f32| -> u8 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a value in 0..=1 times 255 and rounded is in 0..=255"
        )]
        let rounded = (value * 255.0).round() as u8;
        rounded
    };
    [byte(rgb[2]), byte(rgb[1]), byte(rgb[0]), 255]
}

/// `value` rounded to the nearest byte (the values here are between 0 and 255).
fn byte(value: f64) -> u8 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is between 0 and 255 and was rounded"
    )]
    let rounded = value.round().clamp(0.0, 255.0) as u8;
    rounded
}

/// Every channel of `got` within `tolerance` of `wanted`.
fn near(got: [u8; 4], wanted: [u8; 4], tolerance: u8) -> bool {
    got.iter()
        .zip(wanted)
        .all(|(got, wanted)| got.abs_diff(wanted) <= tolerance)
}

/// The Vulkan loader, a validated instance, and the log: every case starts here.
macro_rules! rig {
    () => {{
        let loader = load_loader().expect("the Vulkan loader loads");
        let instance = validated(&loader);
        let log = instance.validation_log();
        (loader, instance, log)
    }};
}

// ---------------------------------------------------------------------------------------------
// The quad type: no device
// ---------------------------------------------------------------------------------------------

fn quad_of(width: u32, height: u32, bytes: usize) -> Result<VulkanQuad, VulkanQuadError> {
    let (destination, source) = whole(width.min(64), height.min(64));
    // The source is inside the image whatever its size (the whole image is at least one pixel).
    let source = SourceRect {
        right: source
            .right
            .min(f32::from(u16::try_from(width.min(8192)).expect("a side"))),
        bottom: source
            .bottom
            .min(f32::from(u16::try_from(height.min(8192)).expect("a side"))),
        ..source
    };
    VulkanQuad::new(
        Arc::new(Bytes(vec![0; bytes])),
        width,
        height,
        destination,
        source,
        VulkanFilter::Nearest,
    )
}

#[test]
fn the_constructor_accepts_8192_and_refuses_8193_and_exactly_64_mib_is_the_limit() {
    // 8192 wide (one row) is accepted; 8193 is refused with the numbers.
    assert!(quad_of(MAX_IMAGE_SIDE, 1, MAX_IMAGE_SIDE as usize * 4).is_ok());
    assert!(quad_of(1, MAX_IMAGE_SIDE, MAX_IMAGE_SIDE as usize * 4).is_ok());
    let error = quad_of(MAX_IMAGE_SIDE + 1, 1, (MAX_IMAGE_SIDE as usize + 1) * 4)
        .expect_err("8193 is too wide");
    assert_eq!(
        error,
        VulkanQuadError::ImageSize {
            width: 8193,
            height: 1
        }
    );
    assert!(error.to_string().contains("8193 x 1"), "{error}");
    assert!(error.to_string().contains("8192"), "{error}");
    assert!(matches!(
        quad_of(0, 4, 0),
        Err(VulkanQuadError::ImageSize { .. })
    ));
    assert!(matches!(
        quad_of(4, 0, 0),
        Err(VulkanQuadError::ImageSize { .. })
    ));
    // Exactly 64 MiB: 4096 x 4096 RGBA8 is accepted; one row more is refused **before** the source's bytes are
    // looked at (the pixel source here is a few bytes, which would otherwise be a length error).
    assert_eq!(u64::from(4096_u32) * 4096 * 4, MAX_IMAGE_BYTES);
    let big = 4096_usize * 4096 * 4;
    assert!(quad_of(4096, 4096, big).is_ok());
    let error = quad_of(4096, 4097, 4).expect_err("one row more is over 64 MiB");
    assert_eq!(
        error,
        VulkanQuadError::ImageTooLarge {
            bytes: 4096 * 4097 * 4
        }
    );
    assert!(
        error
            .to_string()
            .contains(&(4096_u64 * 4097 * 4).to_string()),
        "{error}"
    );
}

#[test]
fn a_wrong_byte_length_and_a_bad_rectangle_are_refused_with_their_reasons() {
    let (destination, source) = whole(2, 2);
    let make = |bytes: usize, destination: DestinationRect, source: SourceRect| {
        VulkanQuad::new(
            Arc::new(Bytes(vec![0; bytes])),
            2,
            2,
            destination,
            source,
            VulkanFilter::Linear,
        )
    };
    assert!(make(16, destination, source).is_ok());
    assert_eq!(
        make(15, destination, source).expect_err("short"),
        VulkanQuadError::PixelLength {
            expected: 16,
            actual: 15
        }
    );
    assert_eq!(
        make(17, destination, source).expect_err("long"),
        VulkanQuadError::PixelLength {
            expected: 16,
            actual: 17
        }
    );
    for (left, top, right, bottom) in [(0, 0, 0, 2), (0, 0, 2, 0), (2, 0, 0, 2), (0, 2, 2, 0)] {
        let empty_or_inverted = DestinationRect {
            left,
            top,
            right,
            bottom,
        };
        assert_eq!(
            make(16, empty_or_inverted, source).expect_err("an empty destination"),
            VulkanQuadError::Destination,
            "{empty_or_inverted:?}"
        );
    }
    for (left, top, right, bottom) in [
        (0.0, 0.0, 0.0, 2.0),
        (0.0, 0.0, 2.0, 0.0),
        (1.0, 0.0, 0.5, 2.0),
        (-0.5, 0.0, 2.0, 2.0),
        (0.0, 0.0, 2.5, 2.0),
        (0.0, 0.0, 2.0, 2.5),
        (f32::NAN, 0.0, 2.0, 2.0),
        (0.0, 0.0, f32::INFINITY, 2.0),
    ] {
        let wrong = SourceRect {
            left,
            top,
            right,
            bottom,
        };
        assert_eq!(
            make(16, destination, wrong).expect_err("a bad source rectangle"),
            VulkanQuadError::Source,
            "{wrong:?}"
        );
    }
    // The scene carries its generation with or without a quad.
    let scene = VulkanScene::new(7, make(16, destination, source).ok());
    assert_eq!(scene.generation(), 7);
    assert!(scene.quad().is_some());
    assert!(VulkanScene::new(8, None).quad().is_none());
}

// ---------------------------------------------------------------------------------------------
// The pixels
// ---------------------------------------------------------------------------------------------

#[test]
fn no_quad_gives_exactly_the_clear() {
    let (_loader, instance, log) = rig!();
    let clear = [0.2, 0.4, 0.6];
    let bytes = readback_quad(&instance, 8, 8, clear, None).expect("the readback works");
    assert_eq!(bytes.len(), 8 * 8 * 4);
    for y in 0..8 {
        for x in 0..8 {
            assert!(
                near(at(&bytes, 8, x, y), clear_bytes(clear), 1),
                "({x}, {y}): {:?}",
                at(&bytes, 8, x, y)
            );
        }
    }
    assert_log_clean(&log);
    assert_eq!(instance.retained_readbacks(), 0);
    destroy(instance);
    assert_log_clean(&log);
}

#[test]
fn nearest_sampling_draws_a_2x2_checker_at_8x8_as_exact_4x4_blocks() {
    let (_loader, instance, log) = rig!();
    // Four opaque colours: the texels are exact in an 8-bit target, so every pixel must match exactly.
    let colours = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 0, 255],
    ];
    let pixels = image(2, 2, |x, y| colours[(y * 2 + x) as usize]);
    let (destination, source) = whole(8, 8);
    let drawn = quad(
        pixels,
        (2, 2),
        destination,
        SourceRect {
            right: 2.0,
            bottom: 2.0,
            ..source
        },
        VulkanFilter::Nearest,
    );
    let bytes =
        readback_quad(&instance, 8, 8, [0.1, 0.1, 0.1], Some(&drawn)).expect("the readback works");
    for y in 0..8_u32 {
        for x in 0..8_u32 {
            let [r, g, b, a] = colours[((y / 4) * 2 + x / 4) as usize];
            assert_eq!(at(&bytes, 8, x, y), [b, g, r, a], "pixel ({x}, {y})");
        }
    }
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

#[test]
fn half_transparent_texels_blend_over_the_clear_by_the_premultiplied_rule() {
    let (_loader, instance, log) = rig!();
    // Premultiplied texels (a colour never exceeds its alpha). The rule is
    // `result = src + dst * (255 - a) / 255` per channel, with the GPU's own rounding: the blend is done in
    // floating point on the stored (rounded) destination and rounded to nearest on the way out, so the result
    // is within one unit of the exact rule's nearest integer. That is the stated tolerance.
    let texels = [
        [100, 50, 0, 128],
        [128, 128, 128, 128],
        [0, 0, 0, 0],
        [255, 255, 255, 255],
    ];
    let pixels = image(4, 1, |x, _| texels[x as usize]);
    let destination = DestinationRect {
        left: 0,
        top: 0,
        right: 4,
        bottom: 1,
    };
    let source = SourceRect {
        left: 0.0,
        top: 0.0,
        right: 4.0,
        bottom: 1.0,
    };
    let drawn = quad(pixels, (4, 1), destination, source, VulkanFilter::Nearest);
    let clear = [0.2, 0.4, 0.6];
    let bytes = readback_quad(&instance, 4, 1, clear, Some(&drawn)).expect("the readback works");
    let [clear_b, clear_g, clear_r, _] = clear_bytes(clear);
    let dst = [clear_r, clear_g, clear_b];
    for (x, texel) in texels.iter().enumerate() {
        let alpha = u32::from(texel[3]);
        let expected = |channel: usize| -> u8 {
            byte(
                f64::from(texel[channel])
                    + f64::from(dst[channel]) * f64::from(255 - alpha) / 255.0,
            )
        };
        // Alpha: `src.a + dst.a * (255 - a) / 255` with dst.a = 255.
        let expected_alpha = byte(f64::from(texel[3]) + 255.0 * f64::from(255 - alpha) / 255.0);
        let wanted = [expected(2), expected(1), expected(0), expected_alpha];
        let got = at(&bytes, 4, u32::try_from(x).expect("a small index"), 0);
        assert!(
            near(got, wanted, 1),
            "texel {x} {texel:?} over {dst:?}: got {got:?}, wanted {wanted:?}"
        );
    }
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

#[test]
fn linear_sampling_differs_from_nearest_at_the_block_edges_and_stays_within_a_tolerance() {
    let (_loader, instance, log) = rig!();
    // A black column and a white column, each drawn at four times their size: 2x1 -> 8x1.
    let pixels = image(2, 1, |x, _| {
        if x == 0 {
            [0, 0, 0, 255]
        } else {
            [255, 255, 255, 255]
        }
    });
    let destination = DestinationRect {
        left: 0,
        top: 0,
        right: 8,
        bottom: 1,
    };
    let source = SourceRect {
        left: 0.0,
        top: 0.0,
        right: 2.0,
        bottom: 1.0,
    };
    let nearest = quad(
        Arc::clone(&pixels),
        (2, 1),
        destination,
        source,
        VulkanFilter::Nearest,
    );
    let linear = quad(pixels, (2, 1), destination, source, VulkanFilter::Linear);
    let flat = readback_quad(&instance, 8, 1, [0.0; 3], Some(&nearest)).expect("nearest");
    let smooth = readback_quad(&instance, 8, 1, [0.0; 3], Some(&linear)).expect("linear");
    let red = |bytes: &[u8], x: u32| i32::from(at(bytes, 8, x, 0)[2]);
    // Nearest: the two blocks, exactly.
    for x in 0..8 {
        assert_eq!(
            red(&flat, x),
            if x < 4 { 0 } else { 255 },
            "nearest, pixel {x}"
        );
    }
    // Linear: pixel centres fall at texel coordinate (x + 0.5) / 4, and a texel's centre is at +0.5, so the
    // ramp runs between the two texels over x = 2..=5 with weights 1/8, 3/8, 5/8, 7/8 and clamps outside it.
    // The sampler's weight has 8 fractional bits, so each value is within 3 of the exact interpolation.
    let expected = [
        0.0,
        0.0,
        255.0 / 8.0,
        255.0 * 3.0 / 8.0,
        255.0 * 5.0 / 8.0,
        255.0 * 7.0 / 8.0,
        255.0,
        255.0,
    ];
    for (x, wanted) in expected.iter().enumerate() {
        let got = red(&smooth, u32::try_from(x).expect("a small index"));
        assert!(
            (f64::from(got) - wanted).abs() <= 3.0,
            "linear, pixel {x}: got {got}, wanted about {wanted}"
        );
    }
    // And linear is not nearest at the block edge: pixels 2 and 3 are lighter than black, 4 and 5 darker than white.
    assert!(red(&smooth, 3) > red(&flat, 3) + 40, "pixel 3");
    assert!(red(&smooth, 4) < red(&flat, 4) - 40, "pixel 4");
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

#[test]
fn a_clipped_source_rectangle_draws_exactly_the_trimmed_image_unstretched() {
    let (_loader, instance, log) = rig!();
    // A 4x4 image of sixteen distinct opaque texels; the inner 2x2 is drawn, one image pixel per target
    // pixel (the source and the destination are the same size), into the middle of a 6x6 target.
    let texel = |x: u32, y: u32| -> [u8; 4] {
        [
            u8::try_from(40 * x + 10).expect("a byte"),
            u8::try_from(40 * y + 10).expect("a byte"),
            200,
            255,
        ]
    };
    let pixels = image(4, 4, texel);
    let destination = DestinationRect {
        left: 2,
        top: 2,
        right: 4,
        bottom: 4,
    };
    let source = SourceRect {
        left: 1.0,
        top: 1.0,
        right: 3.0,
        bottom: 3.0,
    };
    let drawn = quad(pixels, (4, 4), destination, source, VulkanFilter::Nearest);
    let clear = [0.0, 0.0, 0.0];
    let bytes = readback_quad(&instance, 6, 6, clear, Some(&drawn)).expect("the readback works");
    for y in 0..6_u32 {
        for x in 0..6_u32 {
            let got = at(&bytes, 6, x, y);
            if (2..4).contains(&x) && (2..4).contains(&y) {
                let [r, g, b, a] = texel(x - 1, y - 1);
                assert_eq!(
                    got,
                    [b, g, r, a],
                    "pixel ({x}, {y}) is image texel ({}, {})",
                    x - 1,
                    y - 1
                );
            } else {
                assert_eq!(got, [0, 0, 0, 255], "pixel ({x}, {y}) is outside the quad");
            }
        }
    }
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

// ---------------------------------------------------------------------------------------------
// The device checks
// ---------------------------------------------------------------------------------------------

static LIMITED: AtomicUsize = AtomicUsize::new(0);
static UNLIMITED: AtomicUsize = AtomicUsize::new(0);

#[test]
fn a_lowered_device_limit_gives_the_named_error_and_creates_no_quad_object() {
    let (_loader, instance, log) = rig!();
    let pixels = image(2, 2, |_, _| [255, 255, 255, 255]);
    let (destination, source) = whole(2, 2);
    let drawn = quad(pixels, (2, 2), destination, source, VulkanFilter::Nearest);
    let error = readback_quad_with_test_hooks(
        &instance,
        1,
        1,
        [0.0; 3],
        Some(&drawn),
        ReadbackHooks {
            max_image_dimension: Some(1),
            quad_objects: Some(&LIMITED),
            ..ReadbackHooks::default()
        },
    )
    .expect_err("the image does not fit the lowered limit");
    assert!(
        error.message().contains("2 x 2"),
        "names the image's size: {error}"
    );
    assert!(
        error.message().contains("maxImageDimension2D of 1"),
        "names the limit: {error}"
    );
    assert_eq!(
        LIMITED.load(Ordering::SeqCst),
        0,
        "no object was created for the quad"
    );
    assert_eq!(
        instance.retained_readbacks(),
        0,
        "a refused quad retains nothing"
    );
    // The same quad with the device's own limit draws, and creates objects (so the count means something).
    let bytes = readback_quad_with_test_hooks(
        &instance,
        4,
        4,
        [0.0; 3],
        Some(&drawn),
        ReadbackHooks {
            quad_objects: Some(&UNLIMITED),
            ..ReadbackHooks::default()
        },
    )
    .expect("the quad draws with the device's own limit");
    assert_eq!(bytes.len(), 4 * 4 * 4);
    assert!(
        UNLIMITED.load(Ordering::SeqCst) > 0,
        "the control creates quad objects"
    );
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

/// A readback that is refused leaves the instance as it found it: nothing retained, the instance reference and the
/// loader clone released.
fn assert_refusal_is_clean(instance: &Instance, loader: &Loader, base: (usize, usize), what: &str) {
    assert_eq!(
        instance.retained_readbacks(),
        0,
        "{what}: nothing is retained"
    );
    assert_eq!(
        instance.holders(),
        base.0,
        "{what}: the instance reference is released"
    );
    assert_eq!(
        loader.clones(),
        base.1,
        "{what}: the loader clone is released"
    );
}

#[test]
fn a_target_that_is_not_1_to_4096_a_side_is_refused_before_any_vulkan_call_with_or_without_a_quad()
{
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base = (instance.holders(), loader.clones());
    let drawn = small_quad();
    for quad in [None, Some(&drawn)] {
        for (width, height) in [(0, 4), (4, 0), (4097, 4), (4, 4097), (8192, 8192)] {
            let error = readback_quad(&instance, width, height, [0.0; 3], quad)
                .expect_err("an invalid target");
            assert!(
                error.message().contains(&format!("{width} x {height}")),
                "{error}"
            );
            assert!(error.message().contains("4096"), "{error}");
            assert_refusal_is_clean(&instance, &loader, base, &format!("{width} x {height}"));
        }
    }
    assert_log_clean(&log);
    destroy(instance);
}

#[test]
fn a_4096_by_4096_target_is_exactly_64_mib_and_is_accepted() {
    let (_loader, instance, log) = rig!();
    assert_eq!(
        u64::from(MAX_TARGET_SIDE) * u64::from(MAX_TARGET_SIDE) * 4,
        MAX_IMAGE_BYTES
    );
    let clear = [0.2, 0.4, 0.6];
    let bytes = readback_quad(&instance, MAX_TARGET_SIDE, MAX_TARGET_SIDE, clear, None)
        .expect("the largest target works");
    assert_eq!(bytes.len() as u64, MAX_IMAGE_BYTES);
    assert!(near(
        at(&bytes, MAX_TARGET_SIDE, 0, 0),
        clear_bytes(clear),
        1
    ));
    let last = MAX_TARGET_SIDE - 1;
    assert!(near(
        at(&bytes, MAX_TARGET_SIDE, last, last),
        clear_bytes(clear),
        1
    ));
    drop(bytes);
    assert_log_clean(&log);
    assert_eq!(instance.retained_readbacks(), 0);
    destroy(instance);
    assert_log_clean(&log);
}

static IMAGE_LIMITED_TARGET: AtomicUsize = AtomicUsize::new(0);
static FRAMEBUFFER_LIMITED_TARGET: AtomicUsize = AtomicUsize::new(0);
static FRAMEBUFFER_LIMITED_QUAD: AtomicUsize = AtomicUsize::new(0);
static CONTROL_TARGET: AtomicUsize = AtomicUsize::new(0);

#[test]
fn a_lowered_image_or_framebuffer_limit_refuses_the_target_creating_no_target_render_pass_or_framebuffer()
 {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base = (instance.holders(), loader.clones());
    let drawn = small_quad();
    let cases: [(&str, ReadbackHooks, Option<&VulkanQuad>, &str, &AtomicUsize); 3] = [
        (
            "image limit, no quad",
            ReadbackHooks {
                max_image_dimension: Some(2),
                target_objects: Some(&IMAGE_LIMITED_TARGET),
                ..ReadbackHooks::default()
            },
            None,
            "maxImageDimension2D of 2",
            &IMAGE_LIMITED_TARGET,
        ),
        (
            "framebuffer limit, no quad",
            ReadbackHooks {
                max_framebuffer_dimension: Some(2),
                target_objects: Some(&FRAMEBUFFER_LIMITED_TARGET),
                ..ReadbackHooks::default()
            },
            None,
            "maxFramebufferWidth of 2",
            &FRAMEBUFFER_LIMITED_TARGET,
        ),
        (
            "framebuffer limit, with a quad",
            ReadbackHooks {
                max_framebuffer_dimension: Some(2),
                target_objects: Some(&FRAMEBUFFER_LIMITED_QUAD),
                ..ReadbackHooks::default()
            },
            Some(&drawn),
            "maxFramebufferWidth of 2",
            &FRAMEBUFFER_LIMITED_QUAD,
        ),
    ];
    for (what, hooks, quad, named, created) in cases {
        let error = readback_quad_with_test_hooks(&instance, 4, 4, [0.0; 3], quad, hooks)
            .expect_err("the target does not fit the lowered limit");
        assert!(
            error.message().contains("4 x 4"),
            "{what}: names the target: {error}"
        );
        assert!(
            error.message().contains(named),
            "{what}: names the limit: {error}"
        );
        assert_eq!(
            created.load(Ordering::SeqCst),
            0,
            "{what}: no target, render pass or framebuffer"
        );
        assert_refusal_is_clean(&instance, &loader, base, what);
    }
    // The control: the same target with the device's own limits draws, and creates its objects.
    let bytes = readback_quad_with_test_hooks(
        &instance,
        4,
        4,
        [0.0; 3],
        None,
        ReadbackHooks {
            target_objects: Some(&CONTROL_TARGET),
            ..ReadbackHooks::default()
        },
    )
    .expect("the target draws with the device's own limits");
    assert_eq!(bytes.len(), 4 * 4 * 4);
    assert_eq!(
        CONTROL_TARGET.load(Ordering::SeqCst),
        4,
        "the render pass, the image, its view and the framebuffer"
    );
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

#[test]
fn each_side_of_the_target_is_checked_against_the_image_limit_on_its_own() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base = (instance.holders(), loader.clones());
    // A target that is too wide only, and one that is too tall only.
    for (width, height) in [(4, 1), (1, 4)] {
        let error = readback_quad_with_test_hooks(
            &instance,
            width,
            height,
            [0.0; 3],
            None,
            ReadbackHooks {
                max_image_dimension: Some(2),
                ..ReadbackHooks::default()
            },
        )
        .expect_err("one side is over the lowered image limit");
        assert!(
            error.message().contains(&format!("{width} x {height}")),
            "{error}"
        );
        assert!(
            error.message().contains("maxImageDimension2D of 2"),
            "{error}"
        );
        assert_refusal_is_clean(&instance, &loader, base, "one side over the image limit");
    }
    assert_log_clean(&log);
    destroy(instance);
}

// ---------------------------------------------------------------------------------------------
// A pixel source that answers differently on each call (it is a safe trait)
// ---------------------------------------------------------------------------------------------

/// A safe `PixelSource` whose first answer (the one `VulkanQuad::new` checks) is `first` and whose every later one is
/// `later`: nothing in the trait forbids it, so the upload must not trust what the constructor saw.
struct Shifting {
    calls: AtomicUsize,
    first: Vec<u8>,
    later: Vec<u8>,
}

impl PixelSource for Shifting {
    fn pixels(&self) -> &[u8] {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            &self.first
        } else {
            &self.later
        }
    }
}

static GROWN: AtomicUsize = AtomicUsize::new(0);
static SHRUNK: AtomicUsize = AtomicUsize::new(0);
static EMPTIED: AtomicUsize = AtomicUsize::new(0);
static STEADY: AtomicUsize = AtomicUsize::new(0);
static NO_TARGET: AtomicUsize = AtomicUsize::new(0);

#[test]
fn an_upload_uses_one_checked_slice_and_a_source_that_changes_size_is_a_clean_named_error() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base = (instance.holders(), loader.clones());
    let first = vec![200_u8; 16];
    // A 2 x 2 image: 16 bytes. The constructor sees 16; the upload is offered 20, 12, none, or 16 again.
    let cases: [(&str, Vec<u8>, &AtomicUsize, Option<&str>); 4] = [
        ("grown", vec![9; 20], &GROWN, Some("returned 20 bytes")),
        ("shrunk", vec![9; 12], &SHRUNK, Some("returned 12 bytes")),
        ("emptied", Vec::new(), &EMPTIED, Some("returned 0 bytes")),
        ("steady", vec![200; 16], &STEADY, None),
    ];
    for (what, later, created, refused) in cases {
        let source = Arc::new(Shifting {
            calls: AtomicUsize::new(0),
            first: first.clone(),
            later,
        });
        let (destination, whole_source) = whole(2, 2);
        let drawn = VulkanQuad::new(
            Arc::clone(&source) as Arc<dyn PixelSource>,
            2,
            2,
            destination,
            whole_source,
            VulkanFilter::Nearest,
        )
        .expect("a valid quad");
        assert_eq!(
            source.calls.load(Ordering::SeqCst),
            1,
            "{what}: the constructor asked once"
        );
        let result = readback_quad_with_test_hooks(
            &instance,
            2,
            2,
            [0.0; 3],
            Some(&drawn),
            ReadbackHooks {
                quad_objects: Some(created),
                target_objects: Some(&NO_TARGET),
                ..ReadbackHooks::default()
            },
        );
        assert_eq!(
            source.calls.load(Ordering::SeqCst),
            2,
            "{what}: the readback asked once, and trusted nothing else"
        );
        if let Some(named) = refused {
            let error = result.expect_err("a source that changed size is refused");
            assert!(error.message().contains(named), "{what}: {error}");
            assert!(error.message().contains("at upload"), "{what}: {error}");
            assert!(error.message().contains("needs 16"), "{what}: {error}");
            assert_eq!(
                created.load(Ordering::SeqCst),
                0,
                "{what}: no quad object was created"
            );
            assert_eq!(
                NO_TARGET.load(Ordering::SeqCst),
                0,
                "{what}: no target object either"
            );
            assert_refusal_is_clean(&instance, &loader, base, what);
        } else {
            let bytes = result.expect("a source that stays the same size draws");
            assert_eq!(bytes.len(), 2 * 2 * 4);
            assert!(
                created.load(Ordering::SeqCst) > 0,
                "{what}: the quad's objects existed"
            );
            assert_eq!(
                at(&bytes, 2, 0, 0),
                [200, 200, 200, 255],
                "{what}: the checked slice is what was drawn"
            );
        }
    }
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

/// A safe `PixelSource` that answers once (the constructor's check) and panics on every later call: the upload's.
struct Panicking {
    calls: AtomicUsize,
    first: Vec<u8>,
}

impl PixelSource for Panicking {
    fn pixels(&self) -> &[u8] {
        assert!(
            self.calls.fetch_add(1, Ordering::SeqCst) == 0,
            "the pixel source panicked on purpose"
        );
        &self.first
    }
}

/// As [`Panicking`], but the panic's payload is a value whose own `Drop` panics.
struct PanickingWithABomb {
    calls: AtomicUsize,
    first: Vec<u8>,
}

impl PixelSource for PanickingWithABomb {
    fn pixels(&self) -> &[u8] {
        if self.calls.fetch_add(1, Ordering::SeqCst) != 0 {
            std::panic::panic_any(super::Bomb(&UPLOAD_BOMBS));
        }
        &self.first
    }
}

static UPLOAD_BOMBS: AtomicUsize = AtomicUsize::new(0);
static PANICKED_QUAD: AtomicUsize = AtomicUsize::new(0);
static PANICKED_TARGET: AtomicUsize = AtomicUsize::new(0);
static BOMB_QUAD: AtomicUsize = AtomicUsize::new(0);
static BOMB_TARGET: AtomicUsize = AtomicUsize::new(0);

/// The probe from CK-95's review 3, as a regression: a pixel source panics at upload with a payload whose `Drop`
/// panics. The panic is caught at the readback boundary, the payload disposed of without unwinding, and the result
/// is the clean error with the holds released.
#[test]
fn a_pixel_source_that_panics_with_a_dropping_payload_is_a_clean_error_that_releases_the_holds() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let watch = loader.watch();
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base = (instance.holders(), loader.clones());
    let source = Arc::new(PanickingWithABomb {
        calls: AtomicUsize::new(0),
        first: vec![200; 16],
    });
    let (destination, whole_source) = whole(2, 2);
    let drawn = VulkanQuad::new(
        Arc::clone(&source) as Arc<dyn PixelSource>,
        2,
        2,
        destination,
        whole_source,
        VulkanFilter::Nearest,
    )
    .expect("a valid quad");
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        readback_quad_with_test_hooks(
            &instance,
            2,
            2,
            [0.0; 3],
            Some(&drawn),
            ReadbackHooks {
                quad_objects: Some(&BOMB_QUAD),
                target_objects: Some(&BOMB_TARGET),
                ..ReadbackHooks::default()
            },
        )
    }));
    let Ok(result) = outcome else {
        panic!(
            "the payload's drop panic escaped the readback (retained readbacks: {})",
            instance.retained_readbacks()
        );
    };
    let error = result.expect_err("a panicking source is an error");
    assert!(
        error.message().contains("unwound before its submit")
            && error
                .message()
                .contains("a panic with a non-string payload"),
        "{error}"
    );
    assert_eq!(UPLOAD_BOMBS.load(Ordering::SeqCst), 1, "disposed once");
    assert_eq!(counted(&BOMB_QUAD), 0, "no quad object was created");
    assert_eq!(counted(&BOMB_TARGET), 0, "no target object was created");
    assert_refusal_is_clean(&instance, &loader, base, "a panicking source");
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
    drop(loader);
    assert!(watch.released(), "nothing keeps the library loaded");
}

#[test]
fn a_pixel_source_that_panics_at_upload_is_a_clean_error_that_releases_the_holds() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let watch = loader.watch();
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base = (instance.holders(), loader.clones());
    let source = Arc::new(Panicking {
        calls: AtomicUsize::new(0),
        first: vec![200; 16],
    });
    let (destination, whole_source) = whole(2, 2);
    let drawn = VulkanQuad::new(
        Arc::clone(&source) as Arc<dyn PixelSource>,
        2,
        2,
        destination,
        whole_source,
        VulkanFilter::Nearest,
    )
    .expect("a valid quad");
    // The caller-supplied call is made inside the readback; its panic must not escape the readback.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        readback_quad_with_test_hooks(
            &instance,
            2,
            2,
            [0.0; 3],
            Some(&drawn),
            ReadbackHooks {
                quad_objects: Some(&PANICKED_QUAD),
                target_objects: Some(&PANICKED_TARGET),
                ..ReadbackHooks::default()
            },
        )
    }));
    let Ok(result) = outcome else {
        panic!(
            "the pixel source's panic escaped the readback (retained readbacks: {})",
            instance.retained_readbacks()
        );
    };
    let error = result.expect_err("a panicking source is an error");
    assert!(
        error.message().contains("unwound before its submit"),
        "{error}"
    );
    assert!(
        error
            .message()
            .contains("the pixel source panicked on purpose"),
        "{error}"
    );
    assert_eq!(counted(&PANICKED_QUAD), 0, "no quad object was created");
    assert_eq!(counted(&PANICKED_TARGET), 0, "no target object was created");
    assert_refusal_is_clean(&instance, &loader, base, "a panicking source");
    assert_eq!(
        source.calls.load(Ordering::SeqCst),
        2,
        "the constructor's call and the one upload call"
    );
    assert_log_clean(&log);
    // The instance is destroyable: nothing retains it.
    destroy(instance);
    assert_log_clean(&log);
    drop(loader);
    assert!(watch.released(), "nothing keeps the library loaded");
}

// ---------------------------------------------------------------------------------------------
// Capability refusals the real device never makes: synthetic feature overrides
// ---------------------------------------------------------------------------------------------

static SOURCE_CAPABILITY: AtomicUsize = AtomicUsize::new(0);
static TARGET_CAPABILITY: AtomicUsize = AtomicUsize::new(0);
static TARGET_OBJECTS_CAPABILITY: AtomicUsize = AtomicUsize::new(0);

/// All the features the quad's source format needs, and the target's.
const SOURCE_NEEDS: vk::FormatFeatureFlags = vk::FormatFeatureFlags::from_raw(
    vk::FormatFeatureFlags::SAMPLED_IMAGE.as_raw()
        | vk::FormatFeatureFlags::TRANSFER_DST.as_raw()
        | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR.as_raw(),
);
const TARGET_NEEDS: vk::FormatFeatureFlags = vk::FormatFeatureFlags::from_raw(
    vk::FormatFeatureFlags::COLOR_ATTACHMENT.as_raw()
        | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND.as_raw()
        | vk::FormatFeatureFlags::TRANSFER_SRC.as_raw(),
);

fn counted(created: &'static AtomicUsize) -> usize {
    created.load(Ordering::SeqCst)
}

#[test]
fn a_missing_source_format_capability_refuses_the_quad_naming_it_and_creates_nothing() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base = (instance.holders(), loader.clones());
    let linear = small_quad();
    let (destination, source) = whole(2, 2);
    let nearest = quad(
        image(2, 2, |_, _| [255, 255, 255, 255]),
        (2, 2),
        destination,
        source,
        VulkanFilter::Nearest,
    );
    // The source format: sampling, transfer destination, and linear filtering (for a linear quad only).
    for (missing, name, drawn) in [
        (
            vk::FormatFeatureFlags::SAMPLED_IMAGE,
            "SAMPLED_IMAGE",
            &nearest,
        ),
        (
            vk::FormatFeatureFlags::TRANSFER_DST,
            "TRANSFER_DST",
            &nearest,
        ),
        (
            vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR,
            "SAMPLED_IMAGE_FILTER_LINEAR",
            &linear,
        ),
    ] {
        let before = counted(&SOURCE_CAPABILITY);
        let error = readback_quad_with_test_hooks(
            &instance,
            4,
            4,
            [0.0; 3],
            Some(drawn),
            ReadbackHooks {
                source_features: Some(SOURCE_NEEDS & !missing),
                quad_objects: Some(&SOURCE_CAPABILITY),
                target_objects: Some(&TARGET_OBJECTS_CAPABILITY),
                ..ReadbackHooks::default()
            },
        )
        .expect_err("a missing source capability is refused");
        assert!(error.message().contains(name), "{name}: {error}");
        assert!(
            error.message().contains("R8G8B8A8_UNORM"),
            "{name}: {error}"
        );
        assert_eq!(
            counted(&SOURCE_CAPABILITY),
            before,
            "{name}: no quad object was created"
        );
        assert_eq!(
            counted(&TARGET_OBJECTS_CAPABILITY),
            0,
            "{name}: no target object either"
        );
        assert_refusal_is_clean(&instance, &loader, base, name);
    }
    // Linear filtering is needed only by a linear quad: a nearest quad without it is drawn.
    let bytes = readback_quad_with_test_hooks(
        &instance,
        4,
        4,
        [0.0; 3],
        Some(&nearest),
        ReadbackHooks {
            source_features: Some(
                SOURCE_NEEDS & !vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR,
            ),
            ..ReadbackHooks::default()
        },
    )
    .expect("a nearest quad does not need linear filtering");
    assert_eq!(bytes.len(), 4 * 4 * 4);
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

#[test]
fn a_missing_target_format_capability_refuses_the_target_naming_it_and_creates_nothing() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base = (instance.holders(), loader.clones());
    let (destination, source) = whole(2, 2);
    let nearest = quad(
        image(2, 2, |_, _| [255, 255, 255, 255]),
        (2, 2),
        destination,
        source,
        VulkanFilter::Nearest,
    );
    // The target format: colour attachment, blending and transfer source, with a quad and without.
    for (missing, name) in [
        (vk::FormatFeatureFlags::COLOR_ATTACHMENT, "COLOR_ATTACHMENT"),
        (
            vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND,
            "COLOR_ATTACHMENT_BLEND",
        ),
        (vk::FormatFeatureFlags::TRANSFER_SRC, "TRANSFER_SRC"),
    ] {
        for drawn in [None, Some(&nearest)] {
            let error = readback_quad_with_test_hooks(
                &instance,
                4,
                4,
                [0.0; 3],
                drawn,
                ReadbackHooks {
                    target_features: Some(TARGET_NEEDS & !missing),
                    quad_objects: Some(&TARGET_CAPABILITY),
                    target_objects: Some(&TARGET_OBJECTS_CAPABILITY),
                    ..ReadbackHooks::default()
                },
            )
            .expect_err("a missing target capability is refused");
            assert!(error.message().contains(name), "{name}: {error}");
            assert!(
                error.message().contains("B8G8R8A8_UNORM"),
                "{name}: {error}"
            );
            assert_eq!(
                counted(&TARGET_CAPABILITY),
                0,
                "{name}: no quad object was created"
            );
            assert_eq!(
                counted(&TARGET_OBJECTS_CAPABILITY),
                0,
                "{name}: no target object was created"
            );
            assert_refusal_is_clean(&instance, &loader, base, name);
        }
    }
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

// ---------------------------------------------------------------------------------------------
// Ownership: the rules of `readback_clear`, with every new object
// ---------------------------------------------------------------------------------------------

fn small_quad() -> VulkanQuad {
    let pixels = image(2, 2, |x, y| {
        [
            u8::try_from(100 * x).expect("a byte"),
            u8::try_from(100 * y).expect("a byte"),
            50,
            255,
        ]
    });
    let (destination, source) = whole(4, 4);
    quad(
        pixels,
        (2, 2),
        destination,
        SourceRect {
            right: 2.0,
            bottom: 2.0,
            ..source
        },
        VulkanFilter::Linear,
    )
}

/// An unproven quad readback keeps its device, **every object** (the quad's pipeline, layouts, pool, samplers,
/// texture and staging buffer among them), the instance reference and the loader clone: nothing is destroyed on
/// a guess, so the layer, asked to destroy the instance, would complain only if something was; here the instance
/// is never destroyed (the retained readback refuses it).
fn unproven_keeps_everything(name: &str, hooks: ReadbackHooks, created: &'static AtomicUsize) {
    let loader = load_loader().expect("the Vulkan loader loads");
    let watch = loader.watch();
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base_clones = loader.clones();
    let base_holders = instance.holders();
    let drawn = small_quad();
    let error = readback_quad_with_test_hooks(
        &instance,
        4,
        4,
        [0.3, 0.3, 0.3],
        Some(&drawn),
        ReadbackHooks {
            quad_objects: Some(created),
            ..hooks
        },
    )
    .expect_err("an unproven readback is an error");
    assert!(error.message().contains("unproven"), "{name}: {error}");
    assert!(
        created.load(Ordering::SeqCst) >= 8,
        "{name}: the quad's objects existed ({})",
        created.load(Ordering::SeqCst)
    );
    assert_eq!(instance.retained_readbacks(), 1, "{name}");
    assert_eq!(
        instance.holders(),
        base_holders + 1,
        "{name}: the instance reference is kept"
    );
    assert_eq!(
        loader.clones(),
        base_clones + 1,
        "{name}: the loader clone is kept"
    );
    assert_log_clean(&log);
    // SAFETY: there is no surface (null); the call is refused and destroys nothing.
    let refused = unsafe { destroy_surface_and_instance(instance, vk::SurfaceKHR::null(), None) }
        .expect_err("the retained readback keeps the instance");
    assert!(
        refused.error.message().contains("readback"),
        "{name}: {refused}"
    );
    assert_log_clean(&log);
    drop(refused.instance);
    drop(loader);
    assert!(
        !watch.released(),
        "{name}: the retained readback keeps the library loaded"
    );
}

static TIMEOUT: AtomicUsize = AtomicUsize::new(0);
static LOST_SUBMIT: AtomicUsize = AtomicUsize::new(0);
static LOST_WAIT: AtomicUsize = AtomicUsize::new(0);
static UNWOUND: AtomicUsize = AtomicUsize::new(0);

#[test]
fn a_quad_readback_whose_fence_wait_times_out_keeps_everything() {
    unproven_keeps_everything(
        "wait timeout",
        ReadbackHooks {
            wait: Some(vk::Result::TIMEOUT),
            ..ReadbackHooks::default()
        },
        &TIMEOUT,
    );
}

#[test]
fn a_quad_readback_whose_submit_loses_the_device_keeps_everything() {
    unproven_keeps_everything(
        "submit device lost",
        ReadbackHooks {
            submit: Some(vk::Result::ERROR_DEVICE_LOST),
            ..ReadbackHooks::default()
        },
        &LOST_SUBMIT,
    );
}

#[test]
fn a_quad_readback_whose_fence_wait_loses_the_device_keeps_everything() {
    unproven_keeps_everything(
        "wait device lost",
        ReadbackHooks {
            wait: Some(vk::Result::ERROR_DEVICE_LOST),
            ..ReadbackHooks::default()
        },
        &LOST_WAIT,
    );
}

#[test]
fn a_quad_readback_that_unwinds_after_its_submit_is_an_error_that_keeps_everything() {
    unproven_keeps_everything(
        "panic after submit",
        ReadbackHooks {
            panic_after_submit: true,
            ..ReadbackHooks::default()
        },
        &UNWOUND,
    );
}

static BEFORE_SUBMIT: AtomicUsize = AtomicUsize::new(0);
static NOTHING_QUEUED: AtomicUsize = AtomicUsize::new(0);

/// A failure before any submit, or a submit that queued nothing, destroys **everything** the quad readback made and
/// releases the instance reference and the loader clone: the instance can then be destroyed, and the layer, which
/// reports every object still alive at that moment, says nothing.
fn clean_failure_destroys_everything(
    name: &str,
    hooks: ReadbackHooks,
    created: &'static AtomicUsize,
) {
    let loader = load_loader().expect("the Vulkan loader loads");
    let watch = loader.watch();
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base_clones = loader.clones();
    let base_holders = instance.holders();
    let drawn = small_quad();
    let error = readback_quad_with_test_hooks(
        &instance,
        4,
        4,
        [0.3, 0.3, 0.3],
        Some(&drawn),
        ReadbackHooks {
            quad_objects: Some(created),
            ..hooks
        },
    )
    .expect_err("a clean failure is an error");
    assert!(!error.message().contains("unproven"), "{name}: {error}");
    assert!(
        created.load(Ordering::SeqCst) >= 8,
        "{name}: the quad's objects existed"
    );
    assert_eq!(
        instance.retained_readbacks(),
        0,
        "{name}: nothing is retained"
    );
    assert_eq!(
        instance.holders(),
        base_holders,
        "{name}: the instance reference is released"
    );
    assert_eq!(
        loader.clones(),
        base_clones,
        "{name}: the loader clone is released"
    );
    assert_log_clean(&log);
    destroy(instance);
    // Everything was destroyed before the instance: the layer reports nothing leaked.
    assert_log_clean(&log);
    drop(loader);
    assert!(watch.released(), "{name}: nothing keeps the library loaded");
}

#[test]
fn a_quad_readback_that_unwinds_before_its_submit_destroys_everything() {
    clean_failure_destroys_everything(
        "panic before submit",
        ReadbackHooks {
            panic_before_submit: true,
            ..ReadbackHooks::default()
        },
        &BEFORE_SUBMIT,
    );
}

#[test]
fn a_quad_readback_whose_submit_queued_nothing_destroys_everything() {
    clean_failure_destroys_everything(
        "submit out of memory",
        ReadbackHooks {
            submit: Some(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY),
            ..ReadbackHooks::default()
        },
        &NOTHING_QUEUED,
    );
}
