//! Drawing through the bindless table (CK-128), off screen: the `readback_scene_counted` cases draw a clear and a list of
//! instances through the presenter's own instance layout, pipeline, samplers and encoding on the **bindless** path (one run of
//! instances that select their image by slot in a table of resource IDs) and, for the comparisons, on CK-116's ordinary
//! per-texture path, and read the pixels back. They open no window (the presenter target's `gpu` set), run under Metal API and
//! shader validation (the worker fails on any validation report: "no GPU read beyond the table" is that), and each names its
//! negative control.

use ck_gpu_metal::{
    BindlessLimits, DestinationRect, LedgerLimits, MetalContext, MetalFilter, ReadbackImage,
    ReadbackInstance, ReadbackPath, ReadbackTexture, TextureLedger, TextureLimits,
    TextureTestHooks, TextureWakes, readback_scene_counted,
};

unsafe extern "C" {
    /// The native length in bytes of a table buffer (`tests/native/bindless_probe.m`).
    fn ck_test_bindless_table_length(table: *mut std::ffi::c_void) -> u64;
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
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

/// An image and its pixels.
struct Picture {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

fn solid(width: u32, height: u32, pixel: [u8; 4]) -> Picture {
    Picture {
        width,
        height,
        bytes: pixel.repeat((width * height) as usize),
    }
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

fn of(index: usize, destination: DestinationRect) -> ReadbackInstance {
    ReadbackInstance {
        texture: ReadbackTexture::Image(index),
        destination,
        source: WHOLE,
    }
}

/// The bindless path with a table of `slots` entries, no chunking and nothing forced.
fn bindless(slots: u32) -> ReadbackPath {
    ReadbackPath::Bindless {
        texture_slots: slots,
        chunk: u32::MAX,
        force_out_of_range_at: None,
    }
}

/// Draws `list` on `path` into a `width` x `height` target cleared to `clear`.
fn draw(
    width: u32,
    height: u32,
    clear: [f32; 3],
    filter: MetalFilter,
    pictures: &[Picture],
    list: &[ReadbackInstance],
    path: ReadbackPath,
) -> (Vec<u8>, ck_gpu_metal::FrameCounters) {
    readback_scene_counted(
        width,
        height,
        clear,
        None,
        filter,
        &images(pictures),
        list,
        path,
    )
    .expect("the readback works")
}

// ---------------------------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------------------------

/// 10,000 instances over 5,000 distinct textures draw with **one** scene draw (the frame's counters) and the right pixels at
/// every instance (each shows its own identity-coded colour); the ordinary path draws the same scene with 10,000 draws and
/// identical pixels. Negative control: a bindless path that split a run where the texture changes would count more than
/// one draw; one that wrote a wrong slot would show another instance's colour.
pub(crate) fn bindless_one_draw_for_ten_thousand_instances_over_five_thousand_textures() {
    const TEXTURES: usize = 5_000;
    const COLUMNS: u32 = 100;
    const INSTANCES: usize = 10_000;
    // Image i is a 2 x 2 of the identity colour (i % 50, i / 50 % 50, i / 2500) scaled into the byte range: unique per i.
    let colour = |index: usize| -> [u8; 4] {
        let index = u32::try_from(index).expect("small");
        [
            u8::try_from(20 + (index % 50) * 4).expect("a byte"),
            u8::try_from(20 + (index / 50 % 50) * 4).expect("a byte"),
            u8::try_from(40 + (index / 2_500) * 100).expect("a byte"),
            255,
        ]
    };
    let pictures: Vec<Picture> = (0..TEXTURES)
        .map(|index| solid(2, 2, colour(index)))
        .collect();
    // Instance j sits in cell (j % 100, j / 100), 4 x 4 pixels, and shows image (j * 37) % 5000: neighbours always differ,
    // and every image is drawn twice.
    let image_of = |j: usize| (j * 37) % TEXTURES;
    let list: Vec<ReadbackInstance> = (0..INSTANCES)
        .map(|j| {
            let column = i32::try_from(j % COLUMNS as usize).expect("small") * 4;
            let row = i32::try_from(j / COLUMNS as usize).expect("small") * 4;
            of(image_of(j), rect(column, row, column + 4, row + 4))
        })
        .collect();
    let (pixels, counters) = draw(
        400,
        400,
        [0.0; 3],
        MetalFilter::Nearest,
        &pictures,
        &list,
        bindless(u32::try_from(TEXTURES).expect("small") + 1),
    );
    assert_eq!(
        counters.batches, 1,
        "one scene draw for the whole scene: {counters:?}"
    );
    assert_eq!(
        counters.texture_binds, 1,
        "the table is bound once: {counters:?}"
    );
    assert_eq!(counters.instances, INSTANCES as u64);
    assert_eq!((counters.resolved, counters.checker), (INSTANCES as u64, 0));
    assert_eq!(
        counters.pinned, 0,
        "the bindless path pins no revision: the table epoch alone does"
    );
    assert_eq!(counters.slot_skipped, 0);
    for j in 0..INSTANCES {
        let x = u32::try_from(j % COLUMNS as usize).expect("small") * 4 + 2;
        let y = u32::try_from(j / COLUMNS as usize).expect("small") * 4 + 2;
        assert_eq!(
            at(&pixels, 400, x, y),
            colour(image_of(j)),
            "instance {j} shows image {}",
            image_of(j)
        );
    }
    // The ordinary path: the same scene, one draw per instance (neighbours differ), the same pixels.
    let (ordinary_pixels, ordinary) = draw(
        400,
        400,
        [0.0; 3],
        MetalFilter::Nearest,
        &pictures,
        &list,
        ReadbackPath::Ordinary,
    );
    assert_eq!(
        ordinary.batches, INSTANCES as u64,
        "the ordinary option draws 10,000 times: {ordinary:?}"
    );
    assert_eq!(ordinary.pinned, TEXTURES as u64);
    assert!(
        pixels == ordinary_pixels,
        "the bindless and the ordinary path draw identical pixels"
    );
}

/// The painter-order and alpha cases of CK-116 through the bindless path: three overlapping half-transparent instances, in two
/// orders, give exactly the ordinary path's pixels (and at the triple overlap the premultiplied result of drawing them in
/// order), in **one** draw where the ordinary path draws three. Negative control: drawing the run in another order (or
/// sorting it by slot) changes the triple overlap and fails the equality with the ordinary path and the order dependence.
pub(crate) fn bindless_painter_order_and_blending_match_the_ordinary_path() {
    let clear = [0.25, 0.5, 0.75];
    let pictures = [
        solid(1, 1, [128, 0, 0, 128]),
        solid(1, 1, [0, 64, 0, 64]),
        solid(1, 1, [0, 0, 192, 192]),
    ];
    let rects = [rect(0, 0, 6, 4), rect(2, 0, 8, 4), rect(4, 0, 8, 4)];
    let scene = |order: [usize; 3]| -> Vec<ReadbackInstance> {
        order.iter().map(|&index| of(index, rects[index])).collect()
    };
    let mut triple = Vec::new();
    for order in [[0, 1, 2], [2, 1, 0]] {
        let list = scene(order);
        let (bindless_pixels, counters) = draw(
            8,
            4,
            clear,
            MetalFilter::Nearest,
            &pictures,
            &list,
            bindless(8),
        );
        let (ordinary_pixels, ordinary) = draw(
            8,
            4,
            clear,
            MetalFilter::Nearest,
            &pictures,
            &list,
            ReadbackPath::Ordinary,
        );
        assert_eq!(counters.batches, 1, "one draw: {counters:?}");
        assert_eq!(
            ordinary.batches, 3,
            "the ordinary path draws three: {ordinary:?}"
        );
        assert!(
            bindless_pixels == ordinary_pixels,
            "order {order:?}: the bindless pixels equal the ordinary path's"
        );
        triple.push(at(&bindless_pixels, 8, 5, 1));
    }
    assert_ne!(
        triple[0], triple[1],
        "the triple overlap depends on the painter order"
    );
}

/// Instances whose textures alternate keep their order inside one draw: twenty overlapping translucent instances of two
/// images, alternating, give the ordinary path's pixels from twenty draws. Negative control: grouping the run by texture
/// (all of image 0, then all of image 1) changes the stacking and fails the equality.
pub(crate) fn bindless_alternating_textures_keep_painter_order_in_one_draw() {
    let pictures = [solid(1, 1, [100, 0, 0, 100]), solid(1, 1, [0, 0, 120, 120])];
    let list: Vec<ReadbackInstance> = (0..20_i32)
        .map(|j| {
            of(
                usize::try_from(j % 2).expect("0 or 1"),
                rect(j, 0, j + 6, 4),
            )
        })
        .collect();
    let (bindless_pixels, counters) = draw(
        26,
        4,
        [0.1, 0.2, 0.3],
        MetalFilter::Nearest,
        &pictures,
        &list,
        bindless(3),
    );
    let (ordinary_pixels, ordinary) = draw(
        26,
        4,
        [0.1, 0.2, 0.3],
        MetalFilter::Nearest,
        &pictures,
        &list,
        ReadbackPath::Ordinary,
    );
    assert_eq!(counters.batches, 1, "{counters:?}");
    assert_eq!(ordinary.batches, 20, "{ordinary:?}");
    assert!(
        bindless_pixels == ordinary_pixels,
        "alternating textures keep their order in the one draw"
    );
}

/// Unresolved keys draw the checker, sharp under a linear frame, inside the **same** draw as a linear-sampled image: the
/// checker's cells are exactly the two colours at 64 x 64 (and stretched), while the image beside it is blended by the linear
/// sampler; one draw. Negative control: sampling slot 0 with the frame's linear sampler blends the checker's cell edges.
pub(crate) fn bindless_checker_stays_sharp_under_a_linear_frame_inside_the_same_draw() {
    // A 2 x 2 image stretched to 64 x 64 is blended by a linear sampler (so the frame is really linear for it).
    let pictures = [Picture {
        width: 2,
        height: 2,
        bytes: [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 255, 255],
        ]
        .concat(),
    }];
    for side in [64_u32, 100] {
        let signed = i32::try_from(side).expect("small");
        let list = [
            ReadbackInstance {
                texture: ReadbackTexture::Checker,
                destination: rect(0, 0, signed, signed),
                source: WHOLE,
            },
            of(0, rect(signed, 0, 2 * signed, signed)),
        ];
        let (pixels, counters) = draw(
            2 * side,
            side,
            [0.0; 3],
            MetalFilter::Linear,
            &pictures,
            &list,
            bindless(4),
        );
        assert_eq!(
            counters.batches, 1,
            "the checker is inside the one draw: {counters:?}"
        );
        assert_eq!((counters.checker, counters.resolved), (1, 1));
        for y in 0..side {
            for x in 0..side {
                let seen = at(&pixels, 2 * side, x, y);
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
        // The image beside it is sampled linear: a pixel between texel centres is neither of the four colours.
        let blended = at(&pixels, 2 * side, side + side / 2, side / 2);
        assert!(
            ![
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [255, 255, 255, 255]
            ]
            .contains(&blended),
            "the image is blended by the linear sampler: {blended:?}"
        );
    }
}

/// A forced out-of-range slot is skipped and counted, never read: the second of three instances has its slot forced past the
/// table; it takes no instance and shows nothing (the clear colour remains where it would be), the others draw, one draw,
/// and the validation layer (the worker) reports nothing. Negative control: writing the slot would read beyond the table, which
/// the validation layer reports, and the skipped instance's area would not stay clear.
pub(crate) fn bindless_a_forced_out_of_range_slot_is_skipped_and_counted() {
    let pictures = [
        solid(1, 1, [255, 0, 0, 255]),
        solid(1, 1, [0, 255, 0, 255]),
        solid(1, 1, [0, 0, 255, 255]),
    ];
    let list = [
        of(0, rect(0, 0, 4, 4)),
        of(1, rect(4, 0, 8, 4)),
        of(2, rect(8, 0, 12, 4)),
    ];
    let (pixels, counters) = draw(
        12,
        4,
        [0.0; 3],
        MetalFilter::Nearest,
        &pictures,
        &list,
        ReadbackPath::Bindless {
            texture_slots: 4,
            chunk: u32::MAX,
            force_out_of_range_at: Some(1),
        },
    );
    assert_eq!(counters.slot_skipped, 1, "{counters:?}");
    assert_eq!(
        counters.instances, 2,
        "the skipped one took no instance: {counters:?}"
    );
    assert_eq!(counters.batches, 1, "{counters:?}");
    assert_eq!(at(&pixels, 12, 2, 2), [255, 0, 0, 255]);
    assert_eq!(
        at(&pixels, 12, 6, 2),
        [0, 0, 0, 255],
        "the skipped instance's area stays clear"
    );
    assert_eq!(at(&pixels, 12, 10, 2), [0, 0, 255, 255]);
}

/// With a test-injected small per-draw limit a scene splits into the stated chunks, in order, with the same pixels as the
/// unchunked draw and the ordinary path: ten overlapping instances at a limit of three are four draws (3, 3, 3, 1).
/// Negative control: chunks drawn out of order, or a chunk that lost an instance, change the overlapping pixels.
pub(crate) fn bindless_chunks_with_an_injected_limit_split_in_order_with_the_same_pixels() {
    let pictures = [solid(1, 1, [90, 0, 0, 90]), solid(1, 1, [0, 100, 0, 100])];
    let list: Vec<ReadbackInstance> = (0..10_i32)
        .map(|j| {
            of(
                usize::try_from(j % 2).expect("0 or 1"),
                rect(j, 0, j + 5, 4),
            )
        })
        .collect();
    let draw_with = |chunk: u32| {
        draw(
            16,
            4,
            [0.2, 0.2, 0.2],
            MetalFilter::Nearest,
            &pictures,
            &list,
            ReadbackPath::Bindless {
                texture_slots: 3,
                chunk,
                force_out_of_range_at: None,
            },
        )
    };
    let (whole, one) = draw_with(u32::MAX);
    let (chunked, four) = draw_with(3);
    let (single, ten) = draw_with(1);
    let (ordinary_pixels, _) = draw(
        16,
        4,
        [0.2, 0.2, 0.2],
        MetalFilter::Nearest,
        &pictures,
        &list,
        ReadbackPath::Ordinary,
    );
    assert_eq!(one.batches, 1, "{one:?}");
    assert_eq!(four.batches, 4, "ten instances at three per draw: {four:?}");
    assert_eq!(ten.batches, 10, "{ten:?}");
    assert_eq!((four.instances, ten.instances), (10, 10));
    assert!(
        whole == chunked,
        "chunks of three draw the unchunked pixels"
    );
    assert!(whole == single, "chunks of one draw the unchunked pixels");
    assert!(whole == ordinary_pixels, "and the ordinary path's");
}

/// The configured slot count reaches the tables the ledger allocates, off screen: a bindless ledger started with 17 slots (with
/// `max_textures` 8 and `max_extra_revisions` 8, the rule's exact minimum) has two table buffers, each **exactly** 17 entries of
/// 8 bytes (136 bytes) long as the native buffer reports it (not its page-rounded allocation), each view names 17 entries, the
/// two buffers are distinct, and the CPU mirror has a 17th entry (slot 16) and no 18th; the same with 4,096 slots shows 32,768
/// bytes. No logical or byte-budget refusal is counted as native exhaustion. Negative control: a hard-coded count would show the
/// same length for both, and a length rounded to a page would not be 136.
pub(crate) fn readback_bindless_each_table_buffer_has_exactly_the_configured_entries() {
    let context = MetalContext::new().expect("a Metal device");
    for slots in [17_u32, 4096] {
        let ledger = TextureLedger::<u32>::start_bindless(
            std::sync::Arc::clone(&context),
            TextureLimits::default(),
            LedgerLimits::default(),
            BindlessLimits {
                texture_slots: slots,
                max_textures: 8,
                max_extra_revisions: 8,
            },
            128 * 1024 * 1024,
            0,
            TextureWakes::default(),
            TextureTestHooks::default(),
        )
        .expect("a bindless ledger");
        let views = ledger
            .test_table_views()
            .expect("a bindless ledger has tables");
        assert_ne!(
            views[0].buffer, views[1].buffer,
            "the two versions are two buffers"
        );
        for (version, view) in views.iter().enumerate() {
            assert_eq!(view.slots, slots, "version {version} names {slots} entries");
            // SAFETY: the buffer is the live table of `ledger`, which outlives this call.
            let length = unsafe { ck_test_bindless_table_length(view.buffer) };
            assert_eq!(
                length,
                u64::from(slots) * 8,
                "version {version}'s buffer is exactly {slots} entries of 8 bytes"
            );
        }
        let last = slots - 1;
        assert!(
            ledger.test_bindless_entries(last).is_some(),
            "entry {last} exists"
        );
        assert!(
            ledger.test_bindless_entries(slots).is_none(),
            "entry {slots} does not"
        );
    }
}

/// The cases of this module.
pub(crate) const CASES: [(&str, fn()); 7] = [
    (
        "readback_bindless_one_draw_for_ten_thousand_instances_over_five_thousand_textures",
        bindless_one_draw_for_ten_thousand_instances_over_five_thousand_textures,
    ),
    (
        "readback_bindless_painter_order_and_blending_match_the_ordinary_path",
        bindless_painter_order_and_blending_match_the_ordinary_path,
    ),
    (
        "readback_bindless_alternating_textures_keep_painter_order_in_one_draw",
        bindless_alternating_textures_keep_painter_order_in_one_draw,
    ),
    (
        "readback_bindless_checker_stays_sharp_under_a_linear_frame_inside_the_same_draw",
        bindless_checker_stays_sharp_under_a_linear_frame_inside_the_same_draw,
    ),
    (
        "readback_bindless_a_forced_out_of_range_slot_is_skipped_and_counted",
        bindless_a_forced_out_of_range_slot_is_skipped_and_counted,
    ),
    (
        "readback_bindless_chunks_with_an_injected_limit_split_in_order_with_the_same_pixels",
        bindless_chunks_with_an_injected_limit_split_in_order_with_the_same_pixels,
    ),
    (
        "readback_bindless_each_table_buffer_has_exactly_the_configured_entries",
        readback_bindless_each_table_buffer_has_exactly_the_configured_entries,
    ),
];
