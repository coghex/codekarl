//! CPU pixel sources and immutable pixel revisions (`docs/scene-design.md` §5.1).
//!
//! A Rust producer reserves its pixel bytes *before* it allocates the buffer ([`ResourceService::reserve_pixels`]), so
//! the allocation is inside the engine's bound, then builds [`CpuPixels`] with one of two named constructors, so nothing
//! can premultiply twice. The per-pixel work runs in the source's job, never on the caller's thread.

use std::fmt;
use std::sync::Arc;

use crate::convert::premultiply;
use crate::image::Image;
use crate::ledger::{Ledger, Reservation, Transit};
use crate::registry::TextureKey;

/// A reservation of the CPU pixel bytes of one image, made before its buffer is allocated.
pub struct PixelReservation {
    reservation: Reservation,
    width: u32,
    height: u32,
}

impl PixelReservation {
    pub(crate) fn new(reservation: Reservation, width: u32, height: u32) -> Self {
        Self {
            reservation,
            width,
            height,
        }
    }

    /// The reserved image's width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The reserved image's height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The buffer length this reservation is for: `width × height × 4`.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        usize::try_from(self.reservation.amount()).unwrap_or(usize::MAX)
    }

    /// The underlying reservation.
    #[must_use]
    pub fn reservation(&self) -> &Reservation {
        &self.reservation
    }
}

impl fmt::Debug for PixelReservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PixelReservation")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// A byte buffer; in tests its drop reports whether a lock of this crate was held.
struct Bytes(Vec<u8>);

#[cfg(test)]
impl Drop for Bytes {
    fn drop(&mut self) {
        crate::lockdebug::dropped_value();
    }
}

/// Whether the bytes still need premultiplying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Alpha {
    Premultiplied,
    Straight,
}

/// An RGBA8 buffer, rows top first, `width × height × 4` bytes, paid for by a [`PixelReservation`]. Taking the `Vec` by
/// value freezes it: no GPU-facing data aliases caller memory.
pub struct CpuPixels {
    reservation: PixelReservation,
    bytes: Bytes,
    alpha: Alpha,
}

/// Why a buffer was not accepted: its length is not what the reservation is for. The reservation and the buffer are
/// handed back.
pub struct CpuPixelsError {
    /// The reservation, unchanged.
    pub reservation: PixelReservation,
    /// The buffer, unchanged.
    pub bytes: Vec<u8>,
    /// The length it needed.
    pub expected: usize,
}

impl fmt::Debug for CpuPixelsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CpuPixelsError")
            .field("len", &self.bytes.len())
            .field("expected", &self.expected)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for CpuPixelsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "a {}x{} image needs {} bytes of RGBA8, not {}",
            self.reservation.width,
            self.reservation.height,
            self.expected,
            self.bytes.len()
        )
    }
}

impl std::error::Error for CpuPixelsError {}

impl CpuPixels {
    fn new(
        reservation: PixelReservation,
        bytes: Vec<u8>,
        alpha: Alpha,
    ) -> Result<Self, CpuPixelsError> {
        let expected = reservation.byte_len();
        if bytes.len() != expected {
            return Err(CpuPixelsError {
                reservation,
                bytes,
                expected,
            });
        }
        Ok(Self {
            reservation,
            bytes: Bytes(bytes),
            alpha,
        })
    }

    /// Premultiplied RGBA8: the job checks that no colour exceeds its alpha and fails the request, naming the first
    /// such pixel, if one does.
    ///
    /// # Errors
    ///
    /// [`CpuPixelsError`] (with the reservation and the buffer) when `bytes.len()` is not `width × height × 4`.
    pub fn premultiplied(
        reservation: PixelReservation,
        bytes: Vec<u8>,
    ) -> Result<Self, CpuPixelsError> {
        Self::new(reservation, bytes, Alpha::Premultiplied)
    }

    /// Straight-alpha RGBA8: the job premultiplies it with the decoder's rule (`(c × a + 127) / 255`; a fully
    /// transparent pixel becomes all zeros).
    ///
    /// # Errors
    ///
    /// As [`premultiplied`](Self::premultiplied).
    pub fn straight_alpha(
        reservation: PixelReservation,
        bytes: Vec<u8>,
    ) -> Result<Self, CpuPixelsError> {
        Self::new(reservation, bytes, Alpha::Straight)
    }

    /// The width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.reservation.width
    }

    /// The height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.reservation.height
    }

    /// Whether `ledger` granted this source's reservation (an O(1) check).
    pub(crate) fn belongs_to(&self, ledger: &Ledger) -> bool {
        self.reservation.reservation.belongs_to(ledger)
    }

    /// The per-pixel work: checks or premultiplies the buffer and makes the immutable revision, which keeps the CPU
    /// reservation.
    pub(crate) fn into_revision(
        self,
        key: TextureKey,
        revision: u64,
    ) -> Result<Arc<PixelRevision>, String> {
        let Self {
            reservation,
            mut bytes,
            alpha,
        } = self;
        let width = reservation.width;
        let mut pixels = std::mem::take(&mut bytes.0);
        match alpha {
            Alpha::Premultiplied => {
                if let Some(index) = first_over_alpha(&pixels) {
                    let x = index % width as usize;
                    let y = index / width as usize;
                    return Err(format!(
                        "premultiplied pixel ({x}, {y}) has a colour above its alpha"
                    ));
                }
            }
            Alpha::Straight => premultiply_in_place(&mut pixels),
        }
        let height = reservation.height;
        Ok(PixelRevision::new(
            key,
            revision,
            Image::from_validated(width, height, pixels),
            reservation,
        ))
    }
}

impl fmt::Debug for CpuPixels {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CpuPixels")
            .field("width", &self.reservation.width)
            .field("height", &self.reservation.height)
            .field("alpha", &self.alpha)
            .finish_non_exhaustive()
    }
}

/// The index (in pixels) of the first pixel with a colour above its alpha.
fn first_over_alpha(pixels: &[u8]) -> Option<usize> {
    pixels
        .chunks_exact(4)
        .position(|pixel| pixel[0] > pixel[3] || pixel[1] > pixel[3] || pixel[2] > pixel[3])
}

/// Premultiplies straight-alpha RGBA8 with the decoder's rule.
fn premultiply_in_place(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = pixel[3];
        if alpha == 0 {
            pixel.fill(0);
        } else {
            for channel in &mut pixel[..3] {
                *channel = premultiply(*channel, alpha);
            }
        }
    }
}

/// One immutable revision of a texture's pixels, premultiplied. It is shared as `Arc<PixelRevision>`; its CPU
/// reservation returns when the last holder drops it.
pub struct PixelRevision {
    key: TextureKey,
    revision: u64,
    image: Image,
    reservation: PixelReservation,
    /// Counts these bytes as in transit (see `Capacity`) until the revision is dropped. **Last**, so it drops after
    /// the pixels are freed and the reservation has returned its bytes.
    _transit: Transit,
}

#[cfg(test)]
impl Drop for PixelRevision {
    fn drop(&mut self) {
        crate::lockdebug::dropped_value();
    }
}

impl PixelRevision {
    /// A revision of `image`, paid for by `reservation`.
    fn new(
        key: TextureKey,
        revision: u64,
        image: Image,
        reservation: PixelReservation,
    ) -> Arc<Self> {
        let transit = reservation.reservation.in_transit();
        Arc::new(Self {
            key,
            revision,
            image,
            reservation,
            _transit: transit,
        })
    }

    /// A revision of a decoded `image`, paid for by `reservation` (the decoded size, reserved before the decoder
    /// allocated the pixels).
    pub(crate) fn from_decoded(
        key: TextureKey,
        revision: u64,
        image: Image,
        reservation: Reservation,
    ) -> Arc<Self> {
        let (width, height) = (image.width(), image.height());
        Self::new(
            key,
            revision,
            image,
            PixelReservation::new(reservation, width, height),
        )
    }

    /// The texture this revision belongs to.
    #[must_use]
    pub fn key(&self) -> TextureKey {
        self.key
    }

    /// Its revision number.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.image.width()
    }

    /// The height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.image.height()
    }

    /// The image: premultiplied RGBA8, rows top first.
    #[must_use]
    pub fn image(&self) -> &Image {
        &self.image
    }

    /// The pixels: premultiplied RGBA8, rows top first.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        self.image.pixels()
    }

    /// The CPU reservation this revision holds.
    #[must_use]
    pub fn reservation(&self) -> &PixelReservation {
        &self.reservation
    }
}

impl fmt::Debug for PixelRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PixelRevision")
            .field("key", &self.key)
            .field("revision", &self.revision)
            .field("width", &self.image.width())
            .field("height", &self.image.height())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_alpha_is_premultiplied_with_the_decoders_rule() {
        // (c * a + 127) / 255, and a fully transparent pixel is all zeros.
        let mut pixels = vec![
            255, 128, 1, 255, // opaque: unchanged
            255, 255, 255, 0, // transparent: all zeros
            255, 255, 255, 128, // 128
            200, 100, 50, 127, // rounding at 127
            1, 2, 3, 1, // tiny alpha
        ];
        premultiply_in_place(&mut pixels);
        let expected = |c: u32, a: u32| u8::try_from((c * a + 127) / 255).unwrap();
        assert_eq!(&pixels[0..4], &[255, 128, 1, 255]);
        assert_eq!(&pixels[4..8], &[0, 0, 0, 0]);
        assert_eq!(&pixels[8..12], &[128, 128, 128, 128]);
        assert_eq!(
            &pixels[12..16],
            &[
                expected(200, 127),
                expected(100, 127),
                expected(50, 127),
                127
            ]
        );
        assert_eq!(
            &pixels[16..20],
            &[expected(1, 1), expected(2, 1), expected(3, 1), 1]
        );
    }

    #[test]
    fn the_first_pixel_with_a_colour_above_its_alpha_is_found() {
        let mut pixels = vec![
            10, 10, 10, 10, 0, 0, 0, 0, 5, 6, 7, 20, 3, 9, 3, 5, 0, 0, 0, 0,
        ];
        assert_eq!(first_over_alpha(&pixels), Some(3));
        pixels[13] = 5;
        assert_eq!(first_over_alpha(&pixels), None);
        pixels[8] = 21;
        assert_eq!(first_over_alpha(&pixels), Some(2));
    }

    #[test]
    fn each_colour_channel_is_checked_against_the_alpha_on_its_own() {
        for channel in 0..3 {
            let mut pixel = [4, 4, 4, 5];
            assert_eq!(first_over_alpha(&pixel), None);
            pixel[channel] = 6;
            assert_eq!(first_over_alpha(&pixel), Some(0), "channel {channel}");
        }
    }
}
