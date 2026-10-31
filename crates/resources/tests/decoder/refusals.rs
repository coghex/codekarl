//! Files the decoder refuses, and the reason it names.

use ck_resources::{DecodeError, Image, TooLarge};

use crate::fixtures::fixture;
use crate::png::{self, GREY, PALETTE, RGBA};

fn decode(bytes: &[u8]) -> Result<Image, DecodeError> {
    Image::decode(bytes)
}

#[test]
fn something_that_is_not_a_png_is_refused_as_such() {
    assert_eq!(decode(b"").unwrap_err(), DecodeError::NotPng);
    assert_eq!(
        decode(b"GIF89a-not-a-png-at-all").unwrap_err(),
        DecodeError::NotPng
    );
    assert_eq!(decode(&[0; 100]).unwrap_err(), DecodeError::NotPng);
    // Seven right bytes and then a wrong one.
    let mut almost = png::SIGNATURE.to_vec();
    almost[7] = 0x0B;
    almost.extend_from_slice(&fixture("rgba8")[8..]);
    assert_eq!(decode(&almost).unwrap_err(), DecodeError::NotPng);
    // A text file is not a PNG, however long.
    assert_eq!(
        decode(&b"hello world\n".repeat(10_000)).unwrap_err(),
        DecodeError::NotPng
    );
}

#[test]
fn a_truncated_file_is_refused_as_truncated_wherever_it_is_cut() {
    for name in ["rgba8", "grey1-adam7", "palette4-trns"] {
        let whole = fixture(name);
        // Inside the signature, inside the header, inside the later chunks, and just before the image ends.
        for length in [
            1,
            4,
            7,
            8,
            12,
            20,
            30,
            whole.len() / 2,
            whole.len() - 20,
            whole.len() - 13,
        ] {
            let cut = &whole[..length];
            assert_eq!(
                decode(cut).unwrap_err(),
                DecodeError::Truncated,
                "{name} cut at {length} of {}",
                whole.len()
            );
        }
        decode(&whole).unwrap();
    }
}

#[test]
fn a_header_of_zero_width_or_height_is_refused() {
    for (width, height) in [(0, 10), (10, 0), (0, 0)] {
        let error = decode(&png::header_only(width, height, 8, RGBA)).unwrap_err();
        assert!(
            matches!(error, DecodeError::Decoder(_)),
            "{width}×{height}: {error}"
        );
    }
}

#[test]
fn a_header_over_the_side_limit_is_refused_naming_the_dimensions() {
    let error = decode(&png::header_only(8193, 1, 8, GREY)).unwrap_err();
    assert_eq!(
        error,
        DecodeError::TooLarge(TooLarge::Dimensions {
            width: 8193,
            height: 1,
            limit: 8192
        })
    );
    let error = decode(&png::header_only(1, 8193, 8, GREY)).unwrap_err();
    assert_eq!(
        error,
        DecodeError::TooLarge(TooLarge::Dimensions {
            width: 1,
            height: 8193,
            limit: 8192
        })
    );
    let error = decode(&png::header_only(u32::MAX, u32::MAX, 16, RGBA)).unwrap_err();
    assert!(
        matches!(error, DecodeError::TooLarge(TooLarge::Dimensions { .. })),
        "{error}"
    );
    assert!(error.to_string().contains("8192"), "{error}");
}

#[test]
fn what_the_decoder_does_not_support_is_named_unsupported() {
    let header = |compression: u8, filter: u8, interlace: u8| {
        let mut data = Vec::new();
        data.extend_from_slice(&1_u32.to_be_bytes());
        data.extend_from_slice(&1_u32.to_be_bytes());
        data.extend_from_slice(&[8, GREY, compression, filter, interlace]);
        png::chunk(*b"IHDR", &data)
    };
    let scanlines = [0, 7];
    for (what, header) in [
        ("compression", header(1, 0, 0)),
        ("filter", header(0, 1, 0)),
        ("interlace", header(0, 0, 2)),
    ] {
        let error = decode(&png::file(&header, &[], &scanlines)).unwrap_err();
        assert!(
            matches!(error, DecodeError::Unsupported(_)),
            "{what}: {error}"
        );
    }
    // A critical chunk this decoder does not know.
    let good = png::ihdr(1, 1, 8, GREY, 0);
    let error = decode(&png::file(&good, &[png::chunk(*b"AbCd", b"x")], &scanlines)).unwrap_err();
    assert!(matches!(error, DecodeError::Unsupported(_)), "{error}");
    // The same chunk with the ancillary bit set is skipped.
    assert!(decode(&png::file(&good, &[png::chunk(*b"abCd", b"x")], &scanlines)).is_ok());
}

#[test]
fn a_damaged_file_is_a_decoder_error() {
    let mut whole = fixture("rgba8");
    // Flip a byte inside the image data: the checksum no longer matches.
    let at = whole.len() - 20;
    whole[at] ^= 0xFF;
    assert!(matches!(
        decode(&whole).unwrap_err(),
        DecodeError::Decoder(_)
    ));
    // A palette image with no palette.
    let header = png::ihdr(2, 1, 8, PALETTE, 0);
    let error = decode(&png::file(&header, &[], &[0, 0, 1])).unwrap_err();
    assert!(matches!(error, DecodeError::Decoder(_)), "{error}");
    // An index past the end of the palette.
    let palette = png::chunk(*b"PLTE", &[1, 2, 3, 4, 5, 6]);
    let error = decode(&png::file(&header, &[palette], &[0, 0, 2])).unwrap_err();
    match error {
        DecodeError::Decoder(message) => assert!(message.contains("palette index 2"), "{message}"),
        other => panic!("{other}"),
    }
}

#[test]
fn an_image_with_less_data_than_its_header_says_is_refused() {
    // The header says 4 rows of 4 grey pixels; the data holds two.
    let header = png::ihdr(4, 4, 8, GREY, 0);
    let error = decode(&png::file(&header, &[], &[0; 10])).unwrap_err();
    assert!(
        matches!(error, DecodeError::Decoder(_) | DecodeError::Truncated),
        "{error}"
    );
}

#[test]
fn errors_name_their_reason_in_words() {
    assert_eq!(DecodeError::NotPng.to_string(), "not a PNG file");
    assert_eq!(
        DecodeError::Truncated.to_string(),
        "the PNG file is truncated"
    );
    let long = DecodeError::TooLarge(TooLarge::File { limit: 67_108_864 }).to_string();
    assert!(long.contains("67108864"), "{long}");
}
