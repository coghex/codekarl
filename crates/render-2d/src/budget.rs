//! The budgets: the renderer's own limit, and the decoder's, which are `ck-resources`'s.
//!
//! The decode budgets ([`MAX_ENCODED_BYTES`], [`MAX_SIDE`], [`MAX_DECODED_BYTES`], [`READ_BUFFER_BYTES`],
//! [`DECODER_INTERNAL_BYTES`] and [`DecodeLimits`]) are defined in `ck_resources::budget` and re-exported here, so
//! their paths through this crate are unchanged.

pub use ck_resources::budget::{
    DECODER_INTERNAL_BYTES, DecodeLimits, MAX_DECODED_BYTES, MAX_ENCODED_BYTES, MAX_SIDE,
    READ_BUFFER_BYTES,
};

/// The most decoded images that `ck-render-2d` and a backend keep alive at once: the bridge's scene, the
/// sink's latest unconsumed snapshot and the one snapshot the render thread holds while it uploads.
pub const MAX_RENDERER_IMAGES: usize = 3;
