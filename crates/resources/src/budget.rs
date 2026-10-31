//! The decoder's budgets, in one place.
//!
//! These bound data, work and memory, not elapsed time (`docs/render-design.md` §4). Every shipped
//! number is a constant here; [`DecodeLimits`] carries them as a value so a test can inject small ones where the
//! real ones would need huge fixtures.
//!
//! | Budget | Constant | Enforced |
//! | --- | --- | --- |
//! | Encoded input | [`MAX_ENCODED_BYTES`] | Bytes read from the reader, one byte more is an error. |
//! | Dimensions | [`MAX_SIDE`] | From the header, before any image-sized allocation. |
//! | Decoded image | [`MAX_DECODED_BYTES`] | Checked arithmetic from the header, before allocating. |
//! | Caller-owned intermediate | one raw row (at most `8192 × 8` bytes) plus [`READ_BUFFER_BYTES`] | By construction. |
//! | Decoder-internal | [`DECODER_INTERNAL_BYTES`] | Through the `png` crate's own limits. |

/// The largest encoded file, in bytes: 64 MiB.
pub const MAX_ENCODED_BYTES: u64 = 64 * 1024 * 1024;

/// The largest width or height of an image, in pixels.
pub const MAX_SIDE: u32 = 8192;

/// The largest decoded image, in bytes of RGBA8: 64 MiB (for example 4096 × 4096, or 8192 × 2048).
pub const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;

/// The size of the one fixed read buffer the encoded file is streamed through: 64 KiB.
pub const READ_BUFFER_BYTES: usize = 64 * 1024;

/// What the `png` crate may allocate for itself (decompression state, palette, row history): 16 MiB.
///
/// The crate documents its limit as best effort, and it does not count the caller's buffers, so this
/// bounds the library's own allocations only. It is not a whole-process ceiling.
pub const DECODER_INTERNAL_BYTES: usize = 16 * 1024 * 1024;

/// The budgets of one decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeLimits {
    /// The most encoded bytes read.
    pub encoded_bytes: u64,
    /// The largest width or height, in pixels.
    pub side: u32,
    /// The largest decoded image, in bytes of RGBA8.
    pub decoded_bytes: usize,
    /// The size of the read buffer.
    pub read_buffer: usize,
    /// What the `png` crate may allocate for itself.
    pub decoder_internal: usize,
}

impl DecodeLimits {
    /// The shipped budgets.
    pub const SHIPPED: Self = Self {
        encoded_bytes: MAX_ENCODED_BYTES,
        side: MAX_SIDE,
        decoded_bytes: MAX_DECODED_BYTES,
        read_buffer: READ_BUFFER_BYTES,
        decoder_internal: DECODER_INTERNAL_BYTES,
    };
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self::SHIPPED
    }
}
