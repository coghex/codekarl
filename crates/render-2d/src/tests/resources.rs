//! The move to `ck-resources` (CK-105): every path this crate had still resolves to the same item.

use crate::budget;

/// Each re-exported type is the very type `ck-resources` defines: this does not compile if one is replaced by a
/// local copy.
#[test]
fn the_moved_types_are_ck_resourcess_own() {
    let _: fn(ck_resources::Image) -> crate::Image = |image| image;
    let _: fn(ck_resources::DecodeError) -> crate::DecodeError = |error| error;
    let _: fn(ck_resources::TooLarge) -> crate::TooLarge = |too_large| too_large;
    let _: fn(ck_resources::DecodeLimits) -> crate::DecodeLimits = |limits| limits;
    let _: fn(ck_resources::DecodeLimits) -> budget::DecodeLimits = |limits| limits;
    let _: fn(ck_resources::Assets) -> crate::Assets = |assets| assets;
    let _: fn(ck_resources::AssetsError) -> crate::AssetsError = |error| error;
}

#[test]
fn the_decode_budgets_are_ck_resourcess_own() {
    assert_eq!(
        budget::MAX_ENCODED_BYTES,
        ck_resources::budget::MAX_ENCODED_BYTES
    );
    assert_eq!(budget::MAX_SIDE, ck_resources::budget::MAX_SIDE);
    assert_eq!(
        budget::MAX_DECODED_BYTES,
        ck_resources::budget::MAX_DECODED_BYTES
    );
    assert_eq!(
        budget::READ_BUFFER_BYTES,
        ck_resources::budget::READ_BUFFER_BYTES
    );
    assert_eq!(
        budget::DECODER_INTERNAL_BYTES,
        ck_resources::budget::DECODER_INTERNAL_BYTES
    );
    assert_eq!(
        budget::DecodeLimits::SHIPPED,
        ck_resources::DecodeLimits::SHIPPED
    );
}

/// The renderer's own limit stays here (it moved with nothing).
#[test]
fn the_renderer_keeps_its_image_limit() {
    assert_eq!(budget::MAX_RENDERER_IMAGES, 3);
}

/// The real-window test reads this crate's fixture; the decoder's tests read `ck-resources`'s copy. They are one
/// file, so the window test decodes what the decoder tests pin.
#[test]
fn the_window_fixture_is_the_resources_fixture() {
    let window = include_bytes!("../../tests/fixtures/rgba8.png");
    let resources = include_bytes!("../../../resources/tests/fixtures/rgba8.png");
    assert_eq!(&window[..], &resources[..]);
}
