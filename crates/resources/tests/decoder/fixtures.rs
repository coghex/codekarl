//! Committed fixtures (written by `tests/fixtures/generate.py`) decode to exactly the pixels it computes.

use std::fs;
use std::path::PathBuf;

use ck_resources::Image;

use crate::png;

pub(crate) fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub(crate) fn fixture(name: &str) -> Vec<u8> {
    fs::read(directory().join(format!("{name}.png")))
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

pub(crate) fn decode(name: &str) -> Image {
    Image::decode(fixture(name).as_slice()).unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// `name width height hex` for every fixture.
fn manifest() -> Vec<(String, u32, u32, Vec<u8>)> {
    let text = fs::read_to_string(directory().join("expected.txt")).expect("expected.txt");
    text.lines()
        .map(|line| {
            let words: Vec<&str> = line.split(' ').collect();
            assert_eq!(words.len(), 4, "{line}");
            let bytes = (0..words[3].len())
                .step_by(2)
                .map(|at| u8::from_str_radix(&words[3][at..at + 2], 16).expect("hex"))
                .collect();
            (
                words[0].to_owned(),
                words[1].parse().expect("width"),
                words[2].parse().expect("height"),
                bytes,
            )
        })
        .collect()
}

#[test]
fn every_fixture_decodes_to_the_expected_premultiplied_bytes() {
    let manifest = manifest();
    for (name, width, height, expected) in &manifest {
        let image = decode(name);
        assert_eq!((image.width(), image.height()), (*width, *height), "{name}");
        assert_eq!(image.pixels(), expected.as_slice(), "{name}");
    }
}

#[test]
fn the_manifest_covers_every_fixture_file_and_every_colour_type_depth_and_transparency_form() {
    let manifest = manifest();
    let listed: Vec<&str> = manifest.iter().map(|(name, ..)| name.as_str()).collect();
    let mut files: Vec<String> = fs::read_dir(directory())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter_map(|name| name.strip_suffix(".png").map(str::to_owned))
        .collect();
    files.sort();
    let mut sorted = listed.clone();
    sorted.sort_unstable();
    assert_eq!(
        files, sorted,
        "a fixture is missing from expected.txt, or the reverse"
    );
    for required in [
        "grey1",
        "grey2",
        "grey4",
        "grey8",
        "grey16",
        "greyalpha8",
        "greyalpha16",
        "rgb8",
        "rgb16",
        "rgba8",
        "rgba16",
        "palette1",
        "palette2",
        "palette4",
        "palette8",
        "palette1-trns",
        "palette2-trns",
        "palette4-trns",
        "palette8-trns",
        "grey1-trns",
        "grey2-trns",
        "grey4-trns",
        "grey8-trns",
        "grey16-trns",
        "rgb8-trns",
        "rgb16-trns",
        "grey1-9x3",
        "grey1-adam7",
        "grey4-trns-adam7",
        "palette2-trns-adam7",
        "rgb8-5x3",
        "rgb8-adam7",
        "rgba16-adam7",
        "greyalpha8-adam7",
        "rgba8-metadata",
    ] {
        assert!(listed.contains(&required), "no fixture {required}");
    }
}

#[test]
fn premultiplication_rounds_as_c_times_a_plus_127_over_255() {
    // (200, 100, 50) at alpha 255, 128, 1 and 0.
    let image = decode("rgba8");
    assert_eq!(
        image.pixels(),
        [
            [200, 100, 50, 255], // unchanged: (c × 255 + 127) / 255 = c
            [100, 50, 25, 128], // 25727/255 = 100.9, 12927/255 = 50.7, 6527/255 = 25.6: each rounds down
            [1, 0, 0, 1],       // 327/255 = 1.28, 227/255 = 0.89, 177/255 = 0.69
            [0, 0, 0, 0],       // fully transparent: all zeros
        ]
        .concat()
    );
}

#[test]
fn a_16_bit_transparent_sample_is_compared_at_full_depth() {
    // The key is 0x1234, which reduces to 18. 0x1250 differs from it in the low byte only, and reduces to 18
    // as well (and has the same high byte), so reducing both first, by rounding or by dropping the low byte,
    // would call it transparent: it is opaque. 0x12FF has the same high byte too, and reduces to 19.
    let image = decode("grey16-trns");
    assert_eq!(
        image.pixels(),
        [
            [0, 0, 0, 0],      // 0x1234: the key
            [18, 18, 18, 255], // 0x1250: not the key, and 18 at 8 bits like the key
            [19, 19, 19, 255], // 0x12FF: same high byte, not the key
            [52, 52, 52, 255], // 0x3412: same bytes swapped, not the key
            [0, 0, 0, 255],    // 0x0000: opaque black
            [0, 0, 0, 0],      // 0x1234 again
        ]
        .concat()
    );
    // The same for an RGB key: 0x9AFF differs from 0x9ABC only in its low byte (both are 154 at 8 bits).
    let image = decode("rgb16-trns");
    assert_eq!(
        image.pixels(),
        [
            [0, 0, 0, 0],
            [18, 86, 154, 255],
            [18, 86, 153, 255],
            [0, 0, 0, 0]
        ]
        .concat()
    );
}

#[test]
fn an_8_bit_transparent_sample_and_a_palette_alpha_are_applied() {
    assert_eq!(
        decode("rgb8-trns").pixels(),
        [
            [0, 0, 0, 0],
            [10, 20, 31, 255],
            [255, 0, 0, 255],
            [0, 0, 0, 0]
        ]
        .concat()
    );
    // palette2-trns: the entries are red, green, blue, yellow; alphas are 0, 128, 51, then opaque (indexes
    // (i × 3 + 1) % 4 for i in 0..10 = 1 0 3 2 1 0 3 2 1 0).
    let image = decode("palette2-trns");
    let pixel = |index: usize| image.pixels()[index * 4..index * 4 + 4].to_vec();
    assert_eq!(
        pixel(0),
        vec![0, 128, 0, 128],
        "green at alpha 128 premultiplies to (0, 128, 0)"
    );
    assert_eq!(pixel(1), vec![0, 0, 0, 0], "red at alpha 0 is all zeros");
    assert_eq!(
        pixel(2),
        vec![255, 255, 0, 255],
        "yellow is past the tRNS: opaque"
    );
    assert_eq!(
        pixel(3),
        vec![0, 0, 51, 51],
        "blue at alpha 51: (255 × 51 + 127) / 255 = 51"
    );
}

#[test]
fn an_interlaced_image_equals_the_same_image_stored_in_order() {
    // 9 × 3 and 5 × 3 leave several Adam7 passes empty, and 1-bit rows are packed per pass.
    for (plain, interlaced) in [("grey1-9x3", "grey1-adam7"), ("rgb8-5x3", "rgb8-adam7")] {
        let (plain, interlaced) = (decode(plain), decode(interlaced));
        assert_eq!(
            (plain.width(), plain.height()),
            (interlaced.width(), interlaced.height())
        );
        assert_eq!(plain.pixels(), interlaced.pixels());
    }
}

/// A reader that hands out at most `step` bytes per read, so every boundary of the chunk filter is crossed.
struct Drip<'a> {
    bytes: &'a [u8],
    step: usize,
}

impl std::io::Read for Drip<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.step.min(buffer.len()).min(self.bytes.len());
        buffer[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}

#[test]
fn the_pixels_do_not_depend_on_how_the_input_is_cut_into_reads() {
    for (name, _, _, expected) in &manifest() {
        let bytes = fixture(name);
        for step in [1, 2, 3, 5, 7, 8, 9, 13, 64, 4096] {
            let image = Image::decode(Drip {
                bytes: &bytes,
                step,
            })
            .unwrap_or_else(|error| panic!("{name} at {step}: {error}"));
            assert_eq!(
                image.pixels(),
                expected.as_slice(),
                "{name} read {step} bytes at a time"
            );
        }
    }
}

#[test]
fn trailing_bytes_after_the_end_chunk_are_read_but_do_not_matter() {
    let mut bytes = fixture("rgba8");
    let plain = Image::decode(bytes.as_slice()).unwrap();
    bytes.extend_from_slice(b"trailing garbage, not a chunk at all");
    let with_trailer = Image::decode(bytes.as_slice()).unwrap();
    assert_eq!(plain.pixels(), with_trailer.pixels());
}

#[test]
fn an_animated_png_decodes_as_its_default_image() {
    // The animation chunks are dropped before the decoder sees them, so the frames after the default image
    // (here with a different size and different pixels) change nothing.
    let frame_control = |sequence: u32, width: u32, height: u32| {
        let mut data = sequence.to_be_bytes().to_vec();
        data.extend_from_slice(
            &[width.to_be_bytes(), height.to_be_bytes(), [0; 4], [0; 4]].concat(),
        );
        data.extend_from_slice(&[0, 1, 0, 0, 0, 1, 0, 0, 0]);
        data
    };
    let mut bytes = png::SIGNATURE.to_vec();
    bytes.extend_from_slice(&png::ihdr(2, 1, 8, png::GREY, 0));
    bytes.extend_from_slice(&png::chunk(*b"acTL", &[0, 0, 0, 2, 0, 0, 0, 0]));
    bytes.extend_from_slice(&png::chunk(*b"fcTL", &frame_control(0, 2, 1)));
    bytes.extend_from_slice(&png::chunk(*b"IDAT", &png::stored_zlib(&[0, 7, 9])));
    bytes.extend_from_slice(&png::chunk(*b"fcTL", &frame_control(1, 1, 1)));
    let mut frame = 2_u32.to_be_bytes().to_vec();
    frame.extend_from_slice(&png::stored_zlib(&[0, 200]));
    bytes.extend_from_slice(&png::chunk(*b"fdAT", &frame));
    bytes.extend_from_slice(&png::chunk(*b"IEND", &[]));
    let image = Image::decode(bytes.as_slice()).unwrap();
    assert_eq!(image.pixels(), [7, 7, 7, 255, 9, 9, 9, 255]);
}
