//! The UI scale of a window opened in screen pixels (milestone 9, slice 12c): which of 1, 1.5 and 2 UI units' worth of pixels a UI unit
//! is drawn at, from the drawable's size.
//!
//! The scale `s` is 1, 1.5 or 2; `k = 2s` (2, 3 or 4) is the number of screen pixels per UI pixel, a UI pixel being two UI units. The cap is
//! the largest `s` with the drawable at least `1280·s` by `800·s` pixels (below 1280 by 800 it is 1), and the scale used is the cap or
//! the configured limit when that is lower. Everything here is integer arithmetic on `k`, so there is one rule for the drawing, the view
//! size the UI sees and the conversion of pointer positions.

use ck_window::{PixelSize, PointerPosition};

/// `k` for a scale of 1: two pixels per UI pixel.
pub const K_ONE: u32 = 2;
/// `k` for a scale of 1.5.
pub const K_ONE_AND_A_HALF: u32 = 3;
/// `k` for a scale of 2.
pub const K_TWO: u32 = 4;

/// The `k` of a configured scale (`1`, `1.5` or `2`), or `None` for any other value.
#[must_use]
pub fn k_of_scale(scale: f64) -> Option<u32> {
    // The three accepted values are exact in binary floating point, so an exact comparison is the right one.
    #[expect(clippy::float_cmp, reason = "1, 1.5 and 2 are exactly representable")]
    let k = if scale == 1.0 {
        Some(K_ONE)
    } else if scale == 1.5 {
        Some(K_ONE_AND_A_HALF)
    } else if scale == 2.0 {
        Some(K_TWO)
    } else {
        None
    };
    k
}

/// The largest `k` the drawable allows: 4 from 2560 by 1600 pixels, 3 from 1920 by 1200, otherwise 2.
#[must_use]
pub const fn cap_k(pixels: PixelSize) -> u32 {
    if pixels.width >= 2560 && pixels.height >= 1600 {
        K_TWO
    } else if pixels.width >= 1920 && pixels.height >= 1200 {
        K_ONE_AND_A_HALF
    } else {
        K_ONE
    }
}

/// The `k` in use: the cap for `pixels`, or `limit_k` when it is set and lower.
#[must_use]
pub fn scale_k(pixels: PixelSize, limit_k: Option<u32>) -> u32 {
    let cap = cap_k(pixels);
    limit_k.map_or(cap, |limit| limit.min(cap))
}

/// The view the UI sees, in UI units, for a drawable of `pixels` at `k`: `⌊pixels ÷ s⌋` per axis, which is `⌊2·pixels ÷ k⌋`.
#[must_use]
pub fn view_units(pixels: PixelSize, k: u32) -> (u32, u32) {
    let k = u64::from(k.max(1));
    let unit = |pixels: u32| u32::try_from(u64::from(pixels) * 2 / k).unwrap_or(u32::MAX);
    (unit(pixels.width), unit(pixels.height))
}

/// A position of one axis, in 1/256 of a point, as 1/256 of a UI unit: `⌊p × px × 2 ÷ (pt × k)⌋` (toward minus infinity, so negative and
/// fractional positions stay ordered), computed in `i64` and saturated to `i32`. `px` and `pt` are the drawable's and the window's
/// extent on that axis, from the same metrics.
#[must_use]
pub fn position_units(position: i32, pixels: u32, points: u32, k: u32) -> i32 {
    let numerator = i64::from(position) * i64::from(pixels) * 2;
    let denominator = i64::from(points.max(1)) * i64::from(k.max(1));
    saturate(numerator.div_euclid(denominator))
}

/// A point-scroll amount of one axis, in 1/256 of a point, as 1/256 of a UI unit: `trunc(a × px × 2 ÷ (pt × k))`, toward zero, so equal
/// scrolls up and down stay equal.
#[must_use]
pub fn amount_units(amount: i32, pixels: u32, points: u32, k: u32) -> i32 {
    let numerator = i64::from(amount) * i64::from(pixels) * 2;
    let denominator = i64::from(points.max(1)) * i64::from(k.max(1));
    saturate(numerator / denominator)
}

fn saturate(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX })
}

/// The conversion of window points to UI units for one set of metrics at one scale: the one rule for pointer positions, hit tests,
/// point-scroll amounts and the positions the map hands the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitMap {
    /// The drawable's size in pixels.
    pub pixels: PixelSize,
    /// The window's content size in points, from the same metrics.
    pub points: (u32, u32),
    /// The UI scale as `k`.
    pub k: u32,
}

impl UnitMap {
    /// A pointer position (1/256 point, top-left origin) as the UI sees it (1/256 unit).
    #[must_use]
    pub fn position(&self, position: PointerPosition) -> PointerPosition {
        PointerPosition {
            x: position_units(position.x, self.pixels.width, self.points.0, self.k),
            y: position_units(position.y, self.pixels.height, self.points.1, self.k),
        }
    }

    /// A point-scroll amount (1/256 point; scrolling is vertical) as the UI sees it (1/256 unit).
    #[must_use]
    pub fn scroll_amount(&self, amount: i32) -> i32 {
        amount_units(amount, self.pixels.height, self.points.1, self.k)
    }

    /// A whole-point coordinate on the horizontal axis (the map's hover) as 1/256 of a UI unit, by the same rule as a pointer position.
    #[must_use]
    pub fn x_fixed(&self, points: i64) -> i32 {
        position_units(fixed(points), self.pixels.width, self.points.0, self.k)
    }

    /// A whole-point coordinate on the vertical axis (the map's hover) as 1/256 of a UI unit, by the same rule as a pointer position.
    #[must_use]
    pub fn y_fixed(&self, points: i64) -> i32 {
        position_units(fixed(points), self.pixels.height, self.points.1, self.k)
    }
}

/// A whole-point coordinate in 1/256 of a point, saturated to `i32` like every position.
fn fixed(points: i64) -> i32 {
    saturate(points.saturating_mul(256))
}
