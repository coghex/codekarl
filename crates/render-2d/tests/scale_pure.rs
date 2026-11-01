//! The pure parts of the milestone 5 scale acceptance (CK-130): the frozen scene's geometry, the identity code and the
//! fixtures' bytes. No window, no GPU, no file system (`tests/scale/layout.rs` is shared with the harness, `tests/scale.rs`).

#![expect(
    dead_code,
    reason = "the layout module is shared with the harness, which uses what these tests do not"
)]

#[path = "vulkan_scale/diagnostic.rs"]
mod diagnostic;
#[path = "scale/layout.rs"]
mod layout;
// The gate's arithmetic reads the presenter's own types, which exist with the Metal feature on macOS (the group's
// all-features run has it).
#[cfg(all(feature = "metal", target_os = "macos"))]
#[path = "scale/evidence.rs"]
mod evidence;
#[cfg(all(feature = "metal", target_os = "macos"))]
#[path = "scale/json.rs"]
mod json;

use ck_resources::Image;
use layout::{
    COLUMNS, DRAWABLE_HEIGHT, DRAWABLE_WIDTH, ROWS, SCENE_QUADS, SET_A, SET_A_SIDE, VIEW_SIDE,
    VIEWS, code_bits, decode_bits, identity_pixels, identity_png, png_of, scene_quad, set_b_bytes,
    set_b_count, set_b_side, view_quad,
};

/// CK-168's fixed matrix uses only the reviewed A-D cells; C's 65,536-quad bindless cell is A's shared cell.
#[test]
fn ck168_matrix_cells_match_the_spec() {
    let cells: Vec<_> = diagnostic::cells().collect();
    assert_eq!(cells.len(), 13, "C's 65,536-quad case shares A");
    for count in [1, 256, 4_096, 32_768] {
        for mode in ["bindless", "per-revision"] {
            let cell = diagnostic::parse(&format!("a-{mode}-{count}")).expect("listed A cell");
            assert_eq!(cell.live_textures, count);
            assert_eq!(cell.sampled_textures, count);
            assert_eq!(cell.quads, 65_536);
            assert_eq!(cell.ordinary, mode == "per-revision");
            assert_eq!(cell.bindless_slots, 131_072);
        }
    }
    for (name, sampled) in [("b-bindless-1", 1), ("b-bindless-256", 256)] {
        let cell = diagnostic::parse(name).expect("listed B cell");
        assert_eq!(
            (cell.live_textures, cell.sampled_textures, cell.quads),
            (32_768, sampled, 65_536)
        );
        assert!(!cell.ordinary);
    }
    let c = diagnostic::parse("c-bindless-32768").expect("listed C cell");
    let shared = diagnostic::parse("a-bindless-32768").expect("A's shared C cell");
    assert_eq!(
        (c.live_textures, c.sampled_textures, c.quads),
        (32_768, 32_768, 32_768)
    );
    assert_eq!(shared.quads, 65_536);
    assert_eq!(c.bindless_slots, shared.bindless_slots);
    for (name, slots) in [("d-bindless-33794", 33_794), ("d-bindless-131072", 131_072)] {
        let cell = diagnostic::parse(name).expect("listed D cell");
        assert_eq!(
            (cell.live_textures, cell.sampled_textures, cell.quads),
            (32_768, 32_768, 65_536)
        );
        assert_eq!(cell.bindless_slots, slots);
        assert_eq!(cell.max_textures, 32_769);
    }
    assert_eq!(
        diagnostic::parse("d-bindless-33794").unwrap().max_textures,
        diagnostic::parse("d-bindless-131072").unwrap().max_textures
    );
    assert!(diagnostic::parse("a-bindless-64").is_none());
}

/// The CK-168 texture mapping uses every sample uniformly and keeps CK-130's positions intact.
#[test]
fn ck168_texture_mapping_is_uniform_and_preserves_geometry() {
    for (quads, sampled) in [
        (65_536, 1),
        (65_536, 256),
        (65_536, 4_096),
        (65_536, 32_768),
        (32_768, 32_768),
    ] {
        let mut counts = vec![0_u32; sampled as usize];
        for index in 0..quads {
            let placed = diagnostic::placement(index, sampled).expect("valid diagnostic mapping");
            counts[placed.texture as usize] += 1;
        }
        assert!(
            counts.iter().all(|count| *count == quads / sampled),
            "{sampled} textures were not sampled evenly by {quads} quads"
        );
    }
    let base = scene_quad(32_769, true);
    let changed = diagnostic::placement(32_769, 32_768).expect("second-pass mapping");
    assert_eq!(base.texture, 0, "CK-130's i&!1 mapping is uneven");
    assert_eq!(changed.texture, 1);
    assert_eq!(
        (base.x, base.y, base.width, base.height),
        (changed.x, changed.y, changed.width, changed.height)
    );
    assert!(diagnostic::placement(65_536, 32_768).is_none());
    assert!(diagnostic::placement(0, 0).is_none());
    assert!(diagnostic::placement(0, 32_769).is_none());
}

/// Every one of the 65,536 quads lies inside the 1920 x 1080 drawable, and every pass-1 quad overlaps a pass-0 quad
/// (r2, F1: the offset turns inward at the last column and row).
#[test]
fn every_quad_lies_inside_the_drawable_and_every_pass_one_quad_overlaps_pass_zero() {
    let mut outside = Vec::new();
    for index in 0..SCENE_QUADS {
        let quad = scene_quad(index, true);
        if !quad.inside_drawable() {
            outside.push(index);
        }
        if index >= SET_A {
            let i = index - SET_A;
            // Its own cell's pass-0 quad is the nearest; the neighbours it straddles are checked by the cell grid.
            assert!(
                quad.overlaps(&scene_quad(i, true)),
                "pass-1 quad {i} does not overlap the pass-0 quad of its cell"
            );
            let (column, row) = layout::cell_of(i);
            let neighbour_column = if column == COLUMNS - 1 {
                column - 1
            } else {
                column + 1
            };
            let neighbour_row = if row == ROWS - 1 { row - 1 } else { row + 1 };
            for neighbour in [
                neighbour_row * COLUMNS + column,
                row * COLUMNS + neighbour_column,
                neighbour_row * COLUMNS + neighbour_column,
            ] {
                assert!(
                    quad.overlaps(&scene_quad(neighbour, true)),
                    "pass-1 quad {i} does not overlap pass-0 neighbour {neighbour}"
                );
            }
        }
    }
    assert!(
        outside.is_empty(),
        "{} quads partly outside: {:?}",
        outside.len(),
        &outside[..outside.len().min(8)]
    );
    // The extreme pass-1 quad of the spec: x 1908.75 .. 1916.25 and y 1067.34375 .. 1075.78125.
    let last = scene_quad(SCENE_QUADS - 1, true);
    assert_eq!(
        (last.x, last.x + last.width, last.y, last.y + last.height),
        (1908.75, 1916.25, 1067.34375, 1075.78125)
    );
}

/// The negative control of the geometry check: r1's uniform right/down offset puts exactly 383 quads (the last column and the last
/// row, 128 + 256 - 1) partly outside, which the check above would have caught.
#[test]
fn the_uniform_offset_of_the_first_draft_puts_383_quads_outside() {
    let outside = (0..SCENE_QUADS)
        .filter(|index| !scene_quad(*index, false).inside_drawable())
        .count();
    assert_eq!(outside, 383);
}

/// Pass 0 tiles the drawable exactly: 256 x 128 cells of 7.5 x 8.4375 pixels, each texture once, adjacent textures distinct.
#[test]
fn pass_zero_tiles_the_drawable_with_every_texture_once() {
    let first = scene_quad(0, true);
    let last = scene_quad(SET_A - 1, true);
    assert_eq!((first.x, first.y), (0.0, 0.0));
    assert_eq!(
        (last.x + last.width, last.y + last.height),
        (f64::from(DRAWABLE_WIDTH), f64::from(DRAWABLE_HEIGHT))
    );
    let mut seen = vec![false; SET_A as usize];
    for index in 0..SET_A {
        let quad = scene_quad(index, true);
        assert_eq!(quad.texture, index);
        assert!(!std::mem::replace(&mut seen[quad.texture as usize], true));
    }
    // Pass 1 shows the textures in runs of two (the lowest bit cleared).
    for index in SET_A..SCENE_QUADS {
        let (a, b) = (scene_quad(index, true), scene_quad(index ^ 1, true));
        assert_eq!(
            a.texture,
            b.texture,
            "quads {index} and {} share a texture",
            index ^ 1
        );
    }
}

/// The 8 verification views show every one of the 32,768 textures once, inside the drawable, in a single non-overlapping pass.
#[test]
fn the_verification_views_show_every_texture_once_without_overlap() {
    let mut seen = vec![false; SET_A as usize];
    for view in 0..VIEWS {
        let mut cells = Vec::new();
        for index in 0..VIEW_SIDE * VIEW_SIDE {
            let quad = view_quad(view, index);
            assert!(quad.inside_drawable());
            assert!(!std::mem::replace(&mut seen[quad.texture as usize], true));
            cells.push(quad);
        }
        // A grid of equal cells that are laid edge to edge: neighbours touch and never overlap.
        for index in 0..VIEW_SIDE * VIEW_SIDE - 1 {
            let (a, b) = (&cells[index as usize], &cells[index as usize + 1]);
            assert!(
                !a.overlaps(b),
                "view {view}: cells {index} and {} overlap",
                index + 1
            );
        }
        let last = &cells[cells.len() - 1];
        assert_eq!(
            (last.x + last.width, last.y + last.height),
            (f64::from(DRAWABLE_WIDTH), f64::from(DRAWABLE_HEIGHT))
        );
    }
    assert!(
        seen.iter().all(|shown| *shown),
        "every texture is in some view"
    );
}

/// What the harness does with a readback: the cell centres of a texture's 4 x 4 grid, thresholded, decoded.
fn decoded_from_pixels(pixels: &[u8], side: u32) -> Option<u32> {
    let cell = side / 4;
    let mut bits = [false; 16];
    for (index, bit) in (0_u32..16).zip(bits.iter_mut()) {
        let (cx, cy) = (index % 4, index / 4);
        let (x, y) = (cx * cell + cell / 2, cy * cell + cell / 2);
        let at = ((y * side + x) * 4) as usize;
        *bit = pixels[at] > 127;
    }
    decode_bits(&bits)
}

/// Every one of the 32,768 identities decodes back to itself from the pixels of its own texture, at every side of the two sets.
#[test]
fn every_identity_decodes_to_itself_at_every_side() {
    for side in [SET_A_SIDE, 64, 256] {
        for k in 0..SET_A {
            // 256 is covered at a stride (the full set would be 8 GiB of pixels); the small sides cover them all.
            if side == 256 && k % 509 != 0 {
                continue;
            }
            assert_eq!(
                decoded_from_pixels(&identity_pixels(k, side), side),
                Some(k),
                "texture {k} at {side}"
            );
        }
    }
}

/// The parity bit makes the number of ones odd: any single flipped cell is rejected, so a mis-sampled cell cannot pass as another
/// texture (the negative control of the decoder).
#[test]
fn a_flipped_cell_fails_the_parity() {
    for k in [0, 1, 0x5555, 0x7FFF, 12_345, 32_767] {
        let bits = code_bits(k);
        assert_eq!(decode_bits(&bits), Some(k));
        for flipped in 0..16 {
            let mut wrong = bits;
            wrong[flipped] = !wrong[flipped];
            assert_eq!(
                decode_bits(&wrong),
                None,
                "texture {k}, cell {flipped} flipped"
            );
        }
    }
}

/// The marks of the recipe: black 0, white 1, the top-left 2 x 2 pixels half-transparent red, 128 MiB for set A, about 338 MiB for B.
#[test]
fn the_fixtures_have_the_frozen_pixels_and_sizes() {
    let pixels = identity_pixels(0, SET_A_SIDE);
    // Texture 0 has bit 15 (the parity) set and all else black; the red mark covers x, y in 0..2 of cell 0.
    assert_eq!(&pixels[0..4], &[255, 0, 0, 128]);
    assert_eq!(&pixels[4..8], &[255, 0, 0, 128]);
    let at = |x: u32, y: u32| ((y * SET_A_SIDE + x) * 4) as usize;
    assert_eq!(&pixels[at(2, 2)..at(2, 2) + 4], &[0, 0, 0, 255]);
    assert_eq!(&pixels[at(28, 28)..at(28, 28) + 4], &[255, 255, 255, 255]);
    assert_eq!(
        u64::from(SET_A) * u64::from(SET_A_SIDE).pow(2) * 4,
        128 * 1024 * 1024
    );
    assert_eq!(set_b_count(), 248);
    assert_eq!(set_b_bytes(), 354_418_688);
    assert_eq!(
        (
            set_b_side(0),
            set_b_side(127),
            set_b_side(128),
            set_b_side(191)
        ),
        (64, 64, 256, 256)
    );
    assert_eq!(
        (
            set_b_side(192),
            set_b_side(239),
            set_b_side(240),
            set_b_side(247)
        ),
        (1024, 1024, 2048, 2048)
    );
}

/// The fixture bytes are deterministic, and the application's own decoder reads them as the pixels they encode (premultiplied).
#[test]
fn the_pngs_are_deterministic_and_decode_to_their_pixels() {
    for (k, side) in [(0, 32), (12_345, 32), (32_767, 32), (7, 64), (5, 256)] {
        let (a, b) = (identity_png(k, side), identity_png(k, side));
        assert_eq!(
            a, b,
            "texture {k} at {side}: the same bytes for the same index"
        );
        let image = Image::decode(a.as_slice()).expect("a PNG the decoder accepts");
        assert_eq!((image.width(), image.height()), (side, side));
        // The decoder premultiplies: the half-transparent red of the 2 x 2 mark is (128, 0, 0, 128), everything else is opaque and
        // unchanged.
        let mut expected = identity_pixels(k, side);
        for (y, x) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            let at = ((y * side + x) * 4) as usize;
            expected[at..at + 4].copy_from_slice(&[128, 0, 0, 128]);
        }
        assert!(
            image.pixels() == expected.as_slice(),
            "texture {k} at {side}: the decoded pixels"
        );
    }
    // A distinct index is a distinct file; an image larger than one stored block (65,535 bytes) round-trips too.
    assert_ne!(identity_png(1, 32), identity_png(2, 32));
    let big = png_of(&identity_pixels(9, 256), 256);
    assert!(big.len() > 65_535);
    assert_eq!(
        Image::decode(big.as_slice())
            .expect("a multi-block PNG")
            .width(),
        256
    );
}

/// The static gate's verdict and the evidence's JSON, on readings built by hand: no window, no GPU.
#[cfg(all(feature = "metal", target_os = "macos"))]
mod gate {
    use ck_render_2d::{FrameStats, Histogram, MetricsReading, MetricsWindow, Tracked};

    use super::{evidence, json::J};

    const MS: u64 = 1_000_000;

    /// A reading of `frames` frames whose CPU total, GPU and presentation interval samples are given by the three functions.
    fn reading(
        frames: u64,
        cpu: impl Fn(u64) -> u64,
        gpu: impl Fn(u64) -> u64,
        interval: impl Fn(u64) -> u64,
        complete: bool,
    ) -> MetricsReading {
        let mut stats = FrameStats::default();
        for n in 0..frames {
            stats.cpu_total.record(cpu(n));
            stats.gpu.record(gpu(n));
            stats.presentation_interval.record(interval(n));
        }
        stats.frames = frames;
        MetricsReading {
            window: Tracked {
                window: MetricsWindow {
                    id: 1,
                    start: 100,
                    end: Some(200),
                },
                adopted_at: Some(50),
                late_adopted: false,
                outside: 0,
                dropped_late: 0,
                last_admitted_ns: 150,
                end_overrun: false,
                stats,
            },
            outstanding: 0,
            complete,
            timing_lost: 0,
            evidence_lost: false,
            closing_pending: false,
        }
    }

    fn steady(frames: u64) -> MetricsReading {
        reading(frames, |_| 3 * MS, |_| 4 * MS, |_| 16 * MS, true)
    }

    fn field<'a>(verdict: &'a J, name: &str) -> &'a J {
        verdict
            .get(name)
            .unwrap_or_else(|| panic!("{name} in {}", verdict.render()))
    }

    #[test]
    fn a_steady_reading_with_the_minimum_samples_passes() {
        let (passed, verdict) = evidence::static_verdict(&steady(1_790));
        assert!(passed, "{}", verdict.render());
        assert_eq!(field(&verdict, "insufficient"), &J::Bool(false));
        // The exact minimum counts pass (1,500 CPU and GPU samples, and 1,400 intervals).
        let mut minimum = steady(1_500);
        minimum.window.stats.presentation_interval = Histogram::default();
        for _ in 0..1_400 {
            minimum.window.stats.presentation_interval.record(16 * MS);
        }
        assert!(evidence::static_verdict(&minimum).0);
    }

    /// Too few samples is insufficient, never a pass, for each of the three counts in turn (the negative control of the gate).
    #[test]
    fn too_few_samples_are_insufficient_and_never_a_pass() {
        for frames in [0, 1, 299, 1_499] {
            let (passed, verdict) = evidence::static_verdict(&steady(frames));
            assert!(!passed, "{frames} frames: {}", verdict.render());
            assert_eq!(
                field(&verdict, "insufficient"),
                &J::Bool(true),
                "{frames} frames"
            );
        }
        // Enough CPU and GPU samples but too few presentation intervals.
        let few_intervals = {
            let mut r = steady(1_800);
            r.window.stats.presentation_interval = Histogram::default();
            for _ in 0..1_399 {
                r.window.stats.presentation_interval.record(16 * MS);
            }
            r
        };
        let (passed, verdict) = evidence::static_verdict(&few_intervals);
        assert!(
            !passed && field(&verdict, "insufficient") == &J::Bool(true),
            "{}",
            verdict.render()
        );
    }

    #[test]
    fn a_late_adopted_incomplete_or_lossy_window_is_insufficient() {
        for mutate in [
            (|r: &mut MetricsReading| r.window.late_adopted = true) as fn(&mut MetricsReading),
            |r| r.complete = false,
            |r| r.evidence_lost = true,
            |r| r.timing_lost = 1,
        ] {
            let mut r = steady(1_800);
            mutate(&mut r);
            let (passed, verdict) = evidence::static_verdict(&r);
            assert!(
                !passed && field(&verdict, "insufficient") == &J::Bool(true),
                "{}",
                verdict.render()
            );
        }
    }

    /// Both percentiles are gated, each on its own: a slow CPU total or a slow GPU time fails, and is not "insufficient".
    #[test]
    fn a_p95_over_8_ms_fails_for_the_cpu_total_and_the_gpu_alike() {
        let slow = |n: u64| {
            if n.is_multiple_of(10) {
                12 * MS
            } else {
                3 * MS
            }
        };
        let cpu = reading(1_800, slow, |_| 4 * MS, |_| 16 * MS, true);
        let gpu = reading(1_800, |_| 3 * MS, slow, |_| 16 * MS, true);
        for (name, r) in [("cpu", &cpu), ("gpu", &gpu)] {
            let (passed, verdict) = evidence::static_verdict(r);
            assert!(!passed, "{name}: {}", verdict.render());
            assert_eq!(field(&verdict, "insufficient"), &J::Bool(false), "{name}");
        }
        // 4 % of the frames slow is under the 5 % that p95 excludes: it passes.
        let rare = reading(
            1_800,
            |n| if n % 25 == 0 { 12 * MS } else { 3 * MS },
            |_| 4 * MS,
            |_| 16 * MS,
            true,
        );
        assert!(evidence::static_verdict(&rare).0);
    }

    /// A p95 beyond the histogram's range is no value that can pass.
    #[test]
    fn an_out_of_range_p95_never_passes() {
        let r = reading(
            1_800,
            |n| if n % 10 == 0 { 500 * MS } else { 3 * MS },
            |_| 4 * MS,
            |_| 16 * MS,
            true,
        );
        let (passed, verdict) = evidence::static_verdict(&r);
        assert!(!passed, "{}", verdict.render());
        assert_eq!(field(&verdict, "cpu_total_p95_upper_bound_ns"), &J::Null);
    }

    #[test]
    fn the_evidence_json_escapes_and_orders_what_it_is_given() {
        let mut record = J::o([("a", J::U(1)), ("b", J::s("x\"y\n"))]);
        record.push(
            "c",
            J::A(vec![J::Bool(true), J::Null, J::F(1.5), J::F(f64::NAN)]),
        );
        assert_eq!(
            record.render(),
            "{\"a\":1,\"b\":\"x\\\"y\\n\",\"c\":[true,null,1.5,null]}"
        );
        assert_eq!(record.get("a"), Some(&J::U(1)));
        assert_eq!(record.get("missing"), None);
    }
}
