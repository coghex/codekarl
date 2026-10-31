//! Raw PNG rows to premultiplied RGBA8, explicitly.
//!
//! The `png` crate is used with no output transformation, so it hands over each row as the file stores it:
//! packed samples of the file's own colour type and bit depth, 16-bit samples big-endian. Everything else is
//! done here, which keeps each rule (the transparent sample at full depth, the 16-to-8 reduction, the
//! premultiplication) in one reviewable place:
//!
//! - **Samples** of 1, 2 and 4 bits are scaled exactly to 8 bits (`v × 255 / max`, which is exact); 8-bit
//!   samples are unchanged; 16-bit samples are reduced with the rounding the PNG specification recommends,
//!   `(v × 255 + 32767) / 65535`.
//! - **Transparency.** Grey and RGB images get their alpha from a `tRNS` sample. The sample is compared with
//!   the pixel's *raw* sample at the image's own bit depth, before any reduction, so a 16-bit value that
//!   differs from the key only in its low byte is not transparent. A palette's `tRNS` gives one alpha per
//!   entry; entries past its end are opaque.
//! - **Premultiplication** is `(c × a + 127) / 255` on 8-bit values, and a fully transparent pixel is all
//!   zeros.

use png::{BitDepth, ColorType, Info};

use crate::image::DecodeError;

/// A premultiplied RGBA8 pixel.
pub(crate) type Pixel = [u8; 4];

/// A transparent pixel.
const CLEAR: Pixel = [0; 4];

/// What the conversion needs from the file's header and kept chunks.
pub(crate) struct Converter {
    color: ColorType,
    depth: u8,
    /// A palette image's entries, already premultiplied.
    palette: Vec<Pixel>,
    /// A grey image's transparent sample, at the image's bit depth.
    grey_key: Option<u16>,
    /// An RGB image's transparent sample, at the image's bit depth.
    rgb_key: Option<[u16; 3]>,
}

/// An 8-bit channel premultiplied by an 8-bit alpha.
pub(crate) fn premultiply(channel: u8, alpha: u8) -> u8 {
    let product = u32::from(channel) * u32::from(alpha) + 127;
    // At most (255 × 255 + 127) / 255 = 255.
    u8::try_from(product / 255).unwrap_or(u8::MAX)
}

fn pixel(red: u8, green: u8, blue: u8, alpha: u8) -> Pixel {
    if alpha == 0 {
        return CLEAR;
    }
    [
        premultiply(red, alpha),
        premultiply(green, alpha),
        premultiply(blue, alpha),
        alpha,
    ]
}

/// A sample of 8 bits at most, or 16 bits big-endian, `index` samples into the row.
fn sample(row: &[u8], index: usize, depth: u8) -> u16 {
    match depth {
        16 => u16::from_be_bytes([row[2 * index], row[2 * index + 1]]),
        8 => u16::from(row[index]),
        _ => {
            let per_byte = usize::from(8 / depth);
            let shift = 8 - depth * u8::try_from(index % per_byte + 1).unwrap_or(1);
            (u16::from(row[index / per_byte]) >> shift) & ((1 << depth) - 1)
        }
    }
}

/// A sample reduced to 8 bits: exact for 1, 2, 4 and 8 bits, rounded for 16.
fn to_eight(value: u16, depth: u8) -> u8 {
    let scaled = match depth {
        16 => (u32::from(value) * 255 + 32767) / 65535,
        8 => u32::from(value),
        _ => u32::from(value) * 255 / ((1 << u32::from(depth)) - 1),
    };
    u8::try_from(scaled).unwrap_or(u8::MAX)
}

impl Converter {
    /// The converter for a file, from its header and its kept `PLTE` and `tRNS`.
    pub(crate) fn new(info: &Info<'_>) -> Result<Self, DecodeError> {
        let depth = match info.bit_depth {
            BitDepth::One => 1,
            BitDepth::Two => 2,
            BitDepth::Four => 4,
            BitDepth::Eight => 8,
            BitDepth::Sixteen => 16,
        };
        let transparency = info.trns.as_deref();
        let mut converter = Self {
            color: info.color_type,
            depth,
            palette: Vec::new(),
            grey_key: None,
            rgb_key: None,
        };
        match info.color_type {
            ColorType::Indexed => {
                let entries = info.palette.as_deref().ok_or_else(|| {
                    DecodeError::Decoder("a palette image has no palette".to_owned())
                })?;
                if entries.len() % 3 != 0 {
                    return Err(DecodeError::Decoder(format!(
                        "the palette has {} bytes, which is not a whole number of entries",
                        entries.len()
                    )));
                }
                converter.palette = entries
                    .chunks_exact(3)
                    .enumerate()
                    .map(|(index, rgb)| {
                        let alpha = transparency.and_then(|alphas| alphas.get(index)).copied();
                        pixel(rgb[0], rgb[1], rgb[2], alpha.unwrap_or(u8::MAX))
                    })
                    .collect();
            }
            // The decoder keeps a grey key as one byte below 16 bits and as two at 16.
            ColorType::Grayscale => {
                converter.grey_key = match (transparency, depth) {
                    (Some([high, low, ..]), 16) => Some(u16::from_be_bytes([*high, *low])),
                    (Some([low, ..]), _) => Some(u16::from(*low)),
                    _ => None,
                };
            }
            ColorType::Rgb => {
                converter.rgb_key = match (transparency, depth) {
                    (Some([r1, r0, g1, g0, b1, b0, ..]), 16) => Some([
                        u16::from_be_bytes([*r1, *r0]),
                        u16::from_be_bytes([*g1, *g0]),
                        u16::from_be_bytes([*b1, *b0]),
                    ]),
                    (Some([red, green, blue, ..]), _) => {
                        Some([u16::from(*red), u16::from(*green), u16::from(*blue)])
                    }
                    _ => None,
                };
            }
            ColorType::GrayscaleAlpha | ColorType::Rgba => {}
        }
        Ok(converter)
    }

    /// The bits one pixel takes in a raw row.
    pub(crate) fn bits_per_pixel(&self) -> usize {
        let samples = match self.color {
            ColorType::Grayscale | ColorType::Indexed => 1,
            ColorType::GrayscaleAlpha => 2,
            ColorType::Rgb => 3,
            ColorType::Rgba => 4,
        };
        samples * usize::from(self.depth)
    }

    /// Converts the first `count` pixels of a raw row, handing each to `put` with its index.
    pub(crate) fn row(
        &self,
        raw: &[u8],
        count: usize,
        mut put: impl FnMut(usize, Pixel),
    ) -> Result<(), DecodeError> {
        let depth = self.depth;
        for index in 0..count {
            let converted = match self.color {
                ColorType::Grayscale => {
                    let value = sample(raw, index, depth);
                    if self.grey_key == Some(value) {
                        CLEAR
                    } else {
                        let grey = to_eight(value, depth);
                        [grey, grey, grey, u8::MAX]
                    }
                }
                ColorType::GrayscaleAlpha => {
                    let grey = to_eight(sample(raw, 2 * index, depth), depth);
                    let alpha = to_eight(sample(raw, 2 * index + 1, depth), depth);
                    pixel(grey, grey, grey, alpha)
                }
                ColorType::Rgb => {
                    let raw_sample = [
                        sample(raw, 3 * index, depth),
                        sample(raw, 3 * index + 1, depth),
                        sample(raw, 3 * index + 2, depth),
                    ];
                    if self.rgb_key == Some(raw_sample) {
                        CLEAR
                    } else {
                        let [red, green, blue] = raw_sample.map(|value| to_eight(value, depth));
                        [red, green, blue, u8::MAX]
                    }
                }
                ColorType::Rgba => {
                    let channel =
                        |offset: usize| to_eight(sample(raw, 4 * index + offset, depth), depth);
                    pixel(channel(0), channel(1), channel(2), channel(3))
                }
                ColorType::Indexed => {
                    let entry = usize::from(sample(raw, index, depth));
                    *self.palette.get(entry).ok_or_else(|| {
                        DecodeError::Decoder(format!(
                            "palette index {entry} is outside the palette of {} entries",
                            self.palette.len()
                        ))
                    })?
                }
            };
            put(index, converted);
        }
        Ok(())
    }
}
