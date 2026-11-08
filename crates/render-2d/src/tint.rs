//! Per-quad premultiplied RGBA8 tint.

/// A premultiplied RGBA8 multiplier for a sampled texture texel.
///
/// Values are stored as `[red, green, blue, alpha]`. `from_straight` converts straight-alpha
/// channels using integer rounding, so the result is stable across backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tint([u8; 4]);

impl Tint {
    /// No tint: preserve the sampled premultiplied texel.
    pub const WHITE: Self = Self([255; 4]);

    /// Makes a tint from already-premultiplied RGBA8 channels.
    #[must_use]
    pub const fn premultiplied(rgba: [u8; 4]) -> Self {
        Self(rgba)
    }

    /// Premultiplies straight RGBA8 channels, rounding each product to nearest (ties upward).
    #[must_use]
    pub const fn from_straight(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self([premultiply(r, a), premultiply(g, a), premultiply(b, a), a])
    }

    /// The four premultiplied channels in RGBA order.
    #[must_use]
    pub const fn rgba(self) -> [u8; 4] {
        self.0
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the rounded premultiplication quotient cannot exceed the input channel"
)]
const fn premultiply(channel: u8, alpha: u8) -> u8 {
    ((channel as u16 * alpha as u16 + 127) / 255) as u8
}

impl Default for Tint {
    fn default() -> Self {
        Self::WHITE
    }
}
