//! Images, decoded from PNG.

use std::fmt;
use std::io::{BufReader, Read};
use std::rc::Rc;

use png::{Decoder, DecodingError, Limits};

use crate::budget::DecodeLimits;
use crate::chunks::{Drain, Seen, Signature, Stream};
use crate::convert::Converter;

/// An image: its size and its pixels as premultiplied RGBA8, rows top first.
///
/// An image is only made by decoding ([`Image::decode`]), and is shared behind an `Arc` where more than one
/// place holds it.
pub struct Image {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    /// Called when the image is dropped (feature `test-support` only).
    #[cfg(feature = "test-support")]
    observer: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl fmt::Debug for Image {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Image")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "test-support")]
impl Drop for Image {
    fn drop(&mut self) {
        if let Some(observer) = &self.observer {
            observer();
        }
    }
}

/// Why something exceeded a budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TooLarge {
    /// The encoded input is longer than the cap.
    File {
        /// The cap, in bytes.
        limit: u64,
    },
    /// The header's width or height is over the limit.
    Dimensions {
        /// The width.
        width: u32,
        /// The height.
        height: u32,
        /// The limit for either.
        limit: u32,
    },
    /// The decoded image would be over the cap.
    Decoded {
        /// The decoded size, in bytes of RGBA8.
        bytes: u64,
        /// The cap, in bytes.
        limit: usize,
    },
    /// The `png` crate needed more than its own allowance.
    DecoderMemory {
        /// The allowance, in bytes.
        limit: usize,
    },
}

/// Why a file could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// It does not start with the PNG signature.
    NotPng,
    /// It ends before the image does.
    Truncated,
    /// It is over one of the budgets.
    TooLarge(TooLarge),
    /// It uses something this decoder does not.
    Unsupported(String),
    /// The decoder refused it (a damaged or invalid file, or a read that failed).
    Decoder(String),
}

impl fmt::Display for TooLarge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File { limit } => write!(formatter, "the file is over {limit} bytes"),
            Self::Dimensions {
                width,
                height,
                limit,
            } => {
                write!(
                    formatter,
                    "{width} × {height} pixels is over {limit} on a side"
                )
            }
            Self::Decoded { bytes, limit } => {
                write!(
                    formatter,
                    "the decoded image would take {bytes} bytes, over {limit}"
                )
            }
            Self::DecoderMemory { limit } => {
                write!(formatter, "the decoder needs more than its {limit} bytes")
            }
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPng => formatter.write_str("not a PNG file"),
            Self::Truncated => formatter.write_str("the PNG file is truncated"),
            Self::TooLarge(reason) => write!(formatter, "too large: {reason}"),
            Self::Unsupported(what) => write!(formatter, "unsupported: {what}"),
            Self::Decoder(what) => write!(formatter, "the PNG decoder failed: {what}"),
        }
    }
}

impl std::error::Error for TooLarge {}
impl std::error::Error for DecodeError {}

/// How a decode failed, before the stream's observations classify it.
enum Failure {
    Ours(DecodeError),
    Png(DecodingError),
}

impl From<DecodeError> for Failure {
    fn from(error: DecodeError) -> Self {
        Self::Ours(error)
    }
}

impl From<DecodingError> for Failure {
    fn from(error: DecodingError) -> Self {
        Self::Png(error)
    }
}

/// Why a decode stopped: it failed, or the caller's reservation was refused.
enum Stop<E> {
    Failed(Failure),
    Refused(E),
}

impl<E> From<Failure> for Stop<E> {
    fn from(failure: Failure) -> Self {
        Self::Failed(failure)
    }
}

impl<E> From<DecodeError> for Stop<E> {
    fn from(error: DecodeError) -> Self {
        Self::Failed(Failure::Ours(error))
    }
}

impl<E> From<DecodingError> for Stop<E> {
    fn from(error: DecodingError) -> Self {
        Self::Failed(Failure::Png(error))
    }
}

/// Why a decode with a reservation produced no image: the file did not decode, or `reserve` refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReserveError<E> {
    /// The decoder's own error (nothing image-sized was allocated if it is a header or budget error).
    Decode(DecodeError),
    /// The reservation was refused; decoding stopped there and nothing image-sized was allocated.
    Refused(E),
}

impl<E: fmt::Display> fmt::Display for ReserveError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => error.fmt(formatter),
            Self::Refused(error) => error.fmt(formatter),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for ReserveError<E> {}

/// The decoder messages that name something this decoder does not support (checked by tests, so a changed
/// wording in the `png` crate is a test failure and not a silent reclassification).
const UNSUPPORTED_PREFIXES: [&str; 4] = [
    "Unknown compression method",
    "Unknown filter method",
    "Unknown interlace method",
    "Unrecognized critical chunk",
];

fn classify(failure: Failure, seen: &Seen, limits: &DecodeLimits) -> DecodeError {
    if seen.over_cap.get() {
        return DecodeError::TooLarge(TooLarge::File {
            limit: limits.encoded_bytes,
        });
    }
    match seen.signature.get() {
        Signature::Invalid => return DecodeError::NotPng,
        Signature::CutShort => return DecodeError::Truncated,
        Signature::Unknown | Signature::Valid => {}
    }
    match failure {
        Failure::Ours(error) => error,
        Failure::Png(DecodingError::LimitsExceeded) => {
            DecodeError::TooLarge(TooLarge::DecoderMemory {
                limit: limits.decoder_internal,
            })
        }
        Failure::Png(error) => {
            let message = error.to_string();
            let cut_short = seen.ended.get()
                && matches!(error, DecodingError::Format(_) | DecodingError::IoError(_));
            if cut_short {
                DecodeError::Truncated
            } else if UNSUPPORTED_PREFIXES
                .iter()
                .any(|prefix| message.starts_with(prefix))
            {
                DecodeError::Unsupported(message)
            } else {
                DecodeError::Decoder(message)
            }
        }
    }
}

/// An Adam7 pass: where its first pixel is and the steps between its pixels and rows.
struct Pass {
    x0: usize,
    y0: usize,
    dx: usize,
    dy: usize,
}

const ADAM7: [Pass; 7] = [
    Pass {
        x0: 0,
        y0: 0,
        dx: 8,
        dy: 8,
    },
    Pass {
        x0: 4,
        y0: 0,
        dx: 8,
        dy: 8,
    },
    Pass {
        x0: 0,
        y0: 4,
        dx: 4,
        dy: 8,
    },
    Pass {
        x0: 2,
        y0: 0,
        dx: 4,
        dy: 4,
    },
    Pass {
        x0: 0,
        y0: 2,
        dx: 2,
        dy: 4,
    },
    Pass {
        x0: 1,
        y0: 0,
        dx: 2,
        dy: 2,
    },
    Pass {
        x0: 0,
        y0: 1,
        dx: 1,
        dy: 2,
    },
];

/// The pixels or rows of one axis of a pass over `size`.
fn steps(size: usize, start: usize, step: usize) -> usize {
    if size > start {
        (size - start).div_ceil(step)
    } else {
        0
    }
}

fn decode_stream<R: Read, Q, E>(
    stream: Stream<R>,
    drain: &Drain<R>,
    limits: &DecodeLimits,
    reserve: impl FnOnce(u64) -> Result<Q, E>,
) -> Result<(Image, Q), Stop<E>> {
    let buffered = BufReader::with_capacity(limits.read_buffer.max(1), stream);
    let mut decoder = Decoder::new_with_limits(
        buffered,
        Limits {
            bytes: limits.decoder_internal,
        },
    );
    let header = decoder.read_header_info()?;
    let (width, height, interlaced) = (header.width, header.height, header.interlaced);

    // The header decides before anything image-sized is allocated.
    if width == 0 || height == 0 {
        return Err(DecodeError::Decoder("the image has no pixels".to_owned()).into());
    }
    if width > limits.side || height > limits.side {
        return Err(DecodeError::TooLarge(TooLarge::Dimensions {
            width,
            height,
            limit: limits.side,
        })
        .into());
    }
    let full_size = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(DecodeError::TooLarge(TooLarge::Decoded {
            bytes: u64::MAX,
            limit: limits.decoded_bytes,
        }))?;
    let size = usize::try_from(full_size)
        .ok()
        .filter(|size| *size <= limits.decoded_bytes)
        .ok_or(DecodeError::TooLarge(TooLarge::Decoded {
            bytes: full_size,
            limit: limits.decoded_bytes,
        }))?;

    let mut reader = decoder.read_info()?;
    let converter = Converter::new(reader.info())?;
    let row_bytes =
        reader
            .output_line_size(width)
            .ok_or(DecodeError::TooLarge(TooLarge::DecoderMemory {
                limit: limits.decoder_internal,
            }))?;

    // The caller's reservation of the decoded size, after every header check and before the one image-sized
    // allocation: a refusal stops the decode here with nothing image-sized allocated.
    let reservation = reserve(full_size).map_err(Stop::Refused)?;

    // The one image-sized allocation: an allocation failure is an error, not an abort.
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(size).map_err(|_| {
        DecodeError::TooLarge(TooLarge::Decoded {
            bytes: full_size,
            limit: limits.decoded_bytes,
        })
    })?;
    pixels.resize(size, 0);
    // The one raw row (at most 8192 × 8 bytes).
    let mut row = vec![0_u8; row_bytes];

    let (columns, rows) = (width as usize, height as usize);
    let plain = [Pass {
        x0: 0,
        y0: 0,
        dx: 1,
        dy: 1,
    }];
    let passes: &[Pass] = if interlaced { &ADAM7 } else { &plain };
    for pass in passes {
        let (in_row, in_pass) = (
            steps(columns, pass.x0, pass.dx),
            steps(rows, pass.y0, pass.dy),
        );
        if in_row == 0 || in_pass == 0 {
            continue;
        }
        let used = (in_row * converter.bits_per_pixel()).div_ceil(8);
        for line in 0..in_pass {
            if reader.read_row(&mut row)?.is_none() {
                return Err(DecodeError::Truncated.into());
            }
            let y = pass.y0 + line * pass.dy;
            converter.row(&row[..used], in_row, |index, converted| {
                let at = (y * columns + pass.x0 + index * pass.dx) * 4;
                pixels[at..at + 4].copy_from_slice(&converted);
            })?;
        }
    }
    // The rest of the image data and its checksum, then the end chunk; then whatever follows, which is read
    // (and held to the encoded cap) but not looked at.
    reader.finish()?;
    drain.run().map_err(DecodingError::IoError)?;
    Ok((
        Image {
            width,
            height,
            pixels,
            #[cfg(feature = "test-support")]
            observer: None,
        },
        reservation,
    ))
}

impl Image {
    /// Decodes a PNG from a reader, under the shipped budgets ([`DecodeLimits::SHIPPED`]).
    ///
    /// The input is streamed through one small buffer and never held whole, and each decoded row is converted
    /// straight into the final buffer. See the crate documentation for the conversion rules.
    ///
    /// # Errors
    ///
    /// [`DecodeError`], naming the reason.
    pub fn decode<R: Read>(reader: R) -> Result<Self, DecodeError> {
        Self::decode_with(reader, &DecodeLimits::SHIPPED)
    }

    /// Decodes a PNG under the given budgets (the shipped ones are [`DecodeLimits::SHIPPED`]).
    ///
    /// # Errors
    ///
    /// [`DecodeError`], naming the reason.
    pub fn decode_with<R: Read>(reader: R, limits: &DecodeLimits) -> Result<Self, DecodeError> {
        match Self::decode_reserving(reader, limits, |_| Ok::<(), std::convert::Infallible>(())) {
            Ok((image, ())) => Ok(image),
            Err(ReserveError::Decode(error)) => Err(error),
            Err(ReserveError::Refused(never)) => match never {},
        }
    }

    /// Decodes a PNG under the given budgets, asking `reserve` for the decoded size first.
    ///
    /// It is [`decode_with`](Self::decode_with) in every other respect (the same checks in the same order, the same
    /// conversion, the same errors), except that after the header checks (the dimensions, and the decoded size in
    /// checked arithmetic) and **before** the one image-sized allocation it calls `reserve(width × height × 4)`. If
    /// `reserve` refuses, decoding stops with that refusal and nothing image-sized was allocated. On success the
    /// image comes back with whatever `reserve` returned, so a budget reservation travels with the pixels it paid
    /// for.
    ///
    /// # Errors
    ///
    /// [`ReserveError::Decode`] for the errors of [`decode_with`](Self::decode_with); [`ReserveError::Refused`] with
    /// `reserve`'s refusal.
    pub fn decode_reserving<R: Read, Q, E>(
        reader: R,
        limits: &DecodeLimits,
        reserve: impl FnOnce(u64) -> Result<Q, E>,
    ) -> Result<(Self, Q), ReserveError<E>> {
        let (stream, seen, drain): (_, Rc<Seen>, _) = Stream::new(reader, limits.encoded_bytes);
        decode_stream(stream, &drain, limits, reserve).map_err(|stop| match stop {
            Stop::Failed(failure) => ReserveError::Decode(classify(failure, &seen, limits)),
            Stop::Refused(refusal) => ReserveError::Refused(refusal),
        })
    }

    /// The width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixels: premultiplied RGBA8, rows top first, `width × height × 4` bytes.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// An image from parts the caller has validated: `pixels` is `width × height × 4` bytes of premultiplied RGBA8.
    pub(crate) fn from_validated(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            pixels,
            #[cfg(feature = "test-support")]
            observer: None,
        }
    }

    /// An image from raw parts, with no validation. It exists for tests (feature `test-support`) and is not part
    /// of the API.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    #[must_use]
    pub fn from_parts(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            pixels,
            observer: None,
        }
    }

    /// An image whose drop calls `observer`. It exists for tests (feature `test-support`) and is not part of the
    /// API.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    #[must_use]
    pub fn observed(
        width: u32,
        height: u32,
        observer: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let pixels = vec![255; width as usize * height as usize * 4];
        Self {
            width,
            height,
            pixels,
            observer: Some(observer),
        }
    }
}
