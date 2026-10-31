//! `Image::decode_reserving`: the decoded size is reserved after the header checks and before the image-sized allocation.
//!
//! What it allocates is measured in the `allocation` test binary; here the order and the contents of the calls are checked
//! through the public API.

use ck_resources::{DecodeError, DecodeLimits, Image, ReserveError, TooLarge};

use crate::fixtures::{decode, fixture};
use crate::png::{self, GREY};

/// The names of the committed fixtures used here: grey, palette and 16-bit colour, plain and interlaced, odd sizes.
const NAMES: [&str; 9] = [
    "rgba8",
    "grey1",
    "grey1-9x3",
    "palette4-trns",
    "rgb8-adam7",
    "greyalpha8-adam7",
    "rgb8-5x3",
    "rgba16",
    "grey1-adam7",
];

/// Decodes with a reservation that records what it was asked and grants it.
fn reserving(
    bytes: &[u8],
    asked: &mut Vec<u64>,
) -> Result<(Image, u64), ReserveError<&'static str>> {
    Image::decode_reserving(bytes, &DecodeLimits::SHIPPED, |size| {
        asked.push(size);
        Ok::<u64, &'static str>(size)
    })
}

/// The reservation is asked for exactly `width × height × 4`, once, and the image comes back with what it returned and
/// the same pixels as `decode`.
#[test]
fn the_reservation_is_asked_for_exactly_the_decoded_size_once() {
    for name in NAMES {
        let plain = decode(name);
        let mut asked = Vec::new();
        let (image, granted) = reserving(&fixture(name), &mut asked).unwrap_or_else(|error| {
            panic!("{name}: {error}");
        });
        let expected = u64::from(plain.width()) * u64::from(plain.height()) * 4;
        assert_eq!(asked, vec![expected], "{name}");
        assert_eq!(
            granted, expected,
            "{name}: the reservation travels with the image"
        );
        assert_eq!(
            (image.width(), image.height(), image.pixels()),
            (plain.width(), plain.height(), plain.pixels()),
            "{name}: the same image as decode"
        );
    }
}

/// A refusal stops the decode with that refusal, for every kind of file.
#[test]
fn a_refused_reservation_stops_the_decode_with_that_refusal() {
    for name in NAMES {
        let mut asked = Vec::new();
        let refused =
            Image::decode_reserving(fixture(name).as_slice(), &DecodeLimits::SHIPPED, |size| {
                asked.push(size);
                Err::<(), &'static str>("no room")
            });
        match refused {
            Err(ReserveError::Refused("no room")) => {}
            other => panic!("{name}: {other:?}"),
        }
        assert_eq!(asked.len(), 1, "{name}: asked once");
    }
}

/// The reservation is asked **before** the pixel data is read: a file cut inside its first image-data chunk is refused
/// by the reservation when that refuses, and only a granted reservation goes on to find the file truncated.
#[test]
fn the_reservation_comes_before_the_pixel_data_is_read() {
    let whole = fixture("rgba8");
    let idat = whole
        .windows(4)
        .position(|window| window == b"IDAT")
        .expect("an image-data chunk");
    let cut = &whole[..idat + 5];
    assert_eq!(
        Image::decode_with(cut, &DecodeLimits::SHIPPED).unwrap_err(),
        DecodeError::Truncated,
        "decode alone finds the cut file truncated"
    );
    let mut asked = Vec::new();
    assert!(matches!(
        reserving(cut, &mut asked),
        Err(ReserveError::Decode(DecodeError::Truncated))
    ));
    assert_eq!(
        asked.len(),
        1,
        "a granted reservation reads on and finds it truncated"
    );
    assert!(matches!(
        Image::decode_reserving(cut, &DecodeLimits::SHIPPED, |_| Err::<(), _>("refused")),
        Err(ReserveError::Refused("refused"))
    ));
}

/// The header checks come first: a header over a budget is refused as `decode` refuses it, and the reservation is never
/// asked.
#[test]
fn a_header_over_a_budget_is_refused_before_any_reservation_is_asked() {
    for (bytes, what) in [
        (png::header_only(8193, 1, 8, GREY), "too wide"),
        (
            png::header_only(4097, 4096, 8, GREY),
            "over the decoded cap",
        ),
    ] {
        let mut asked = Vec::new();
        let error = reserving(&bytes, &mut asked).unwrap_err();
        assert!(
            matches!(error, ReserveError::Decode(DecodeError::TooLarge(_))),
            "{what}: {error:?}"
        );
        assert!(asked.is_empty(), "{what}: the reservation was asked");
        assert_eq!(
            Image::decode(bytes.as_slice()).unwrap_err(),
            match error {
                ReserveError::Decode(error) => error,
                ReserveError::Refused(never) => panic!("{never}"),
            },
            "{what}: the same error as decode"
        );
    }
    // Injected small budgets are honoured in the same order.
    let small = DecodeLimits {
        decoded_bytes: 8,
        ..DecodeLimits::SHIPPED
    };
    let mut asked = Vec::new();
    let error = Image::decode_reserving(fixture("rgba8").as_slice(), &small, |size| {
        asked.push(size);
        Ok::<(), ()>(())
    })
    .unwrap_err();
    assert!(matches!(
        error,
        ReserveError::Decode(DecodeError::TooLarge(TooLarge::Decoded { .. }))
    ));
    assert!(asked.is_empty());
}

/// `decode` and `decode_with` are the reserving decode with a reservation that always succeeds: the same images and the
/// same errors on every fixture.
#[test]
fn decode_and_decode_with_behave_exactly_as_before() {
    for name in NAMES {
        let bytes = fixture(name);
        let plain = Image::decode(bytes.as_slice()).expect("a decode");
        let with = Image::decode_with(bytes.as_slice(), &DecodeLimits::SHIPPED).expect("a decode");
        assert_eq!(plain.pixels(), with.pixels(), "{name}");
        // A cut file is the same error with or without a reservation.
        let cut = &bytes[..bytes.len() / 2];
        let mut asked = Vec::new();
        let reserved = match reserving(cut, &mut asked) {
            Err(ReserveError::Decode(error)) => error,
            other => panic!("{name}: {other:?}"),
        };
        assert_eq!(Image::decode(cut).unwrap_err(), reserved, "{name}");
    }
}
