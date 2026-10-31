//! The decoder's budgets: each constant, and what each refuses or allows. The allocation they bound is measured
//! in the `allocation` test binary.

use ck_resources::budget::{
    DECODER_INTERNAL_BYTES, MAX_DECODED_BYTES, MAX_ENCODED_BYTES, MAX_SIDE, READ_BUFFER_BYTES,
};
use ck_resources::{DecodeError, DecodeLimits, Image, TooLarge};

use crate::fixtures::fixture;
use crate::png::{self, Counting, GREY, Part, Parts, RGBA};

const MIB: usize = 1024 * 1024;

#[test]
fn the_shipped_numbers_are_the_documented_ones() {
    assert_eq!(MAX_ENCODED_BYTES, 64 * MIB as u64);
    assert_eq!(MAX_SIDE, 8192);
    assert_eq!(MAX_DECODED_BYTES, 64 * MIB);
    assert_eq!(READ_BUFFER_BYTES, 64 * 1024);
    assert_eq!(DECODER_INTERNAL_BYTES, 16 * MIB);
    assert_eq!(
        DecodeLimits::SHIPPED,
        DecodeLimits {
            encoded_bytes: MAX_ENCODED_BYTES,
            side: MAX_SIDE,
            decoded_bytes: MAX_DECODED_BYTES,
            read_buffer: READ_BUFFER_BYTES,
            decoder_internal: DECODER_INTERNAL_BYTES,
        }
    );
    assert_eq!(DecodeLimits::default(), DecodeLimits::SHIPPED);
    // 8192 x 2048 and 4096 x 4096 RGBA8 are exactly the decoded cap.
    assert_eq!(8192 * 2048 * 4, MAX_DECODED_BYTES);
    assert_eq!(4096 * 4096 * 4, MAX_DECODED_BYTES);
}

fn limits() -> DecodeLimits {
    DecodeLimits::SHIPPED
}

#[test]
fn the_encoded_cap_allows_exactly_that_many_bytes_and_refuses_one_more() {
    let whole = fixture("rgba16-adam7");
    let length = whole.len() as u64;
    let at_cap = DecodeLimits {
        encoded_bytes: length,
        ..limits()
    };
    assert!(Image::decode_with(whole.as_slice(), &at_cap).is_ok());
    let under = DecodeLimits {
        encoded_bytes: length - 1,
        ..limits()
    };
    let error = Image::decode_with(whole.as_slice(), &under).unwrap_err();
    assert_eq!(
        error,
        DecodeError::TooLarge(TooLarge::File { limit: length - 1 })
    );
    // One byte of trailing data past the cap is read, and is the error.
    let mut trailing = whole.clone();
    trailing.push(0);
    let error = Image::decode_with(trailing.as_slice(), &at_cap).unwrap_err();
    assert_eq!(
        error,
        DecodeError::TooLarge(TooLarge::File { limit: length })
    );
}

#[test]
fn a_file_over_the_shipped_encoded_cap_is_refused_whatever_it_holds() {
    // A 1 x 1 image with a 70 MiB text chunk: the metadata is not what is refused, the length is.
    let mut parts = vec![Part::Bytes(png::header_only(1, 1, 8, GREY))];
    parts.extend(Parts::giant_chunk(*b"tEXt", 70 * MIB as u64));
    parts.push(Part::Bytes(
        [
            png::chunk(*b"IDAT", &png::stored_zlib(&[0, 7])),
            png::chunk(*b"IEND", &[]),
        ]
        .concat(),
    ));
    let mut source = Counting {
        inner: Parts::new(parts),
        read: 0,
    };
    let error = Image::decode(&mut source).unwrap_err();
    assert_eq!(
        error,
        DecodeError::TooLarge(TooLarge::File {
            limit: MAX_ENCODED_BYTES
        })
    );
    assert_eq!(
        source.read,
        MAX_ENCODED_BYTES + 1,
        "it stopped at the one byte past the cap"
    );
}

#[test]
fn a_dimension_over_the_limit_is_refused_by_the_header_alone() {
    let bytes = png::header_only(8193, 1, 8, GREY);
    let mut source = Counting {
        inner: bytes.as_slice(),
        read: 0,
    };
    let error = Image::decode(&mut source).unwrap_err();
    assert!(
        matches!(
            error,
            DecodeError::TooLarge(TooLarge::Dimensions { width: 8193, .. })
        ),
        "{error}"
    );
    assert_eq!(
        source.read,
        bytes.len() as u64,
        "nothing past the header was wanted"
    );
}

#[test]
fn a_decoded_size_over_the_cap_is_refused() {
    // 4097 x 4096 RGBA8 is one row over 64 MiB.
    let bytes = png::header_only(4097, 4096, 8, GREY);
    assert_eq!(
        Image::decode(bytes.as_slice()).unwrap_err(),
        DecodeError::TooLarge(TooLarge::Decoded {
            bytes: 4097 * 4096 * 4,
            limit: MAX_DECODED_BYTES
        })
    );
    let bytes = png::header_only(8192, 2049, 16, RGBA);
    let error = Image::decode(bytes.as_slice()).unwrap_err();
    assert!(
        matches!(error, DecodeError::TooLarge(TooLarge::Decoded { .. })),
        "{error}"
    );
}

#[test]
fn the_side_limit_allows_8192_and_the_injected_limits_are_honoured() {
    let wide = png::file(
        &png::ihdr(8192, 1, 8, GREY, 0),
        &[],
        &[vec![0], vec![9; 8192]].concat(),
    );
    assert_eq!(Image::decode(wide.as_slice()).unwrap().width(), 8192);
    let tall = png::file(&png::ihdr(1, 8192, 8, GREY, 0), &[], &[0, 9].repeat(8192));
    assert_eq!(Image::decode(tall.as_slice()).unwrap().height(), 8192);
    // A small side limit, a small decoded cap.
    let small = fixture("grey8"); // 4 x 1
    let side = DecodeLimits {
        side: 3,
        ..limits()
    };
    assert_eq!(
        Image::decode_with(small.as_slice(), &side).unwrap_err(),
        DecodeError::TooLarge(TooLarge::Dimensions {
            width: 4,
            height: 1,
            limit: 3
        })
    );
    let cap = DecodeLimits {
        decoded_bytes: 15,
        ..limits()
    };
    assert_eq!(
        Image::decode_with(small.as_slice(), &cap).unwrap_err(),
        DecodeError::TooLarge(TooLarge::Decoded {
            bytes: 16,
            limit: 15
        })
    );
    let exact = DecodeLimits {
        decoded_bytes: 16,
        ..limits()
    };
    assert!(Image::decode_with(small.as_slice(), &exact).is_ok());
}

#[test]
fn the_decoders_own_allowance_is_passed_to_the_png_crate() {
    // A 1000-pixel RGBA8 row is 4001 bytes: the decoder cannot work in 1000.
    let bytes = png::file(
        &png::ihdr(1000, 2, 8, RGBA, 0),
        &[],
        &[vec![0_u8; 1], vec![9; 4000], vec![0], vec![9; 4000]].concat(),
    );
    let tight = DecodeLimits {
        decoder_internal: 1000,
        ..limits()
    };
    assert_eq!(
        Image::decode_with(bytes.as_slice(), &tight).unwrap_err(),
        DecodeError::TooLarge(TooLarge::DecoderMemory { limit: 1000 })
    );
    assert!(Image::decode(bytes.as_slice()).is_ok());
}
