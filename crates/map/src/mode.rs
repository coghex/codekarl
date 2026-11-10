//! The map modes' reference rule (milestone 9, slice 8; `docs/economy-design.md` section 3.22): the colour of a map cell that shows a
//! per-place value, from red through yellow to green.
//!
//! The inputs are two plain tables. The **index grid** has one `u16` per crop cell, row-major: the index of the place that owns the
//! cell (`Scenario::catchment_owners`), or [`MODE_UNOWNED`] for none. The **value table** has one `u16` per place: a position on the
//! colour ramp, 0 through [`MODE_VALUE_MAX`] (in thousandths), or [`MODE_UNCOLOURED`] for a place with nothing to show (a place with no
//! capacity). Red (0) and uncoloured therefore never collide.
//!
//! **The rule.** A cell's value is the floored integer mean of the values of the cells in its 5 x 5 box (the cell, two cells to each
//! side and two above and below, inside the crop) that are owned by a coloured place. Unowned cells and cells of uncoloured places
//! are not counted. A cell with no counted cell in its box has no value and keeps its terrain colour. The colour runs linearly from
//! red (0) through yellow (500) to green (1000), in integers. The rule applies to every cell, water included.

use crate::{Cell, Crop, FixedPoint, MapData, Rgba8, cell_of};

/// The height, in metres, that is white in the elevation mode: Everest, the top of the map's range.
pub const ELEVATION_TOP_METRES: u32 = 8_848;

/// The largest value on the ramp: green.
pub const MODE_VALUE_MAX: u16 = 1000;
/// A value-table entry for a place that is not coloured.
pub const MODE_UNCOLOURED: u16 = 0xFFFF;
/// An index-grid entry for a cell no place owns.
pub const MODE_UNOWNED: u16 = 0xFFFF;

/// The half-width of the blend box: 2 cells each side, a 5 x 5 box.
const REACH: i64 = 2;

/// The two tables the rule reads (plain borrowed data, validated by the caller: see the map lane).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeInputs<'a> {
    /// One owner index per crop cell, row-major ([`MODE_UNOWNED`] for none).
    pub owners: &'a [u16],
    /// One value per place ([`MODE_UNCOLOURED`] for none).
    pub values: &'a [u16],
}

/// The colour of `value` on the ramp: red `(255, 0, 0)` at 0, yellow `(255, 255, 0)` at 500, green `(0, 255, 0)` at 1000, linear in
/// between with the integer division floored. A value above 1000 is read as 1000.
#[must_use]
pub fn mode_colour(value: u16) -> Rgba8 {
    let value = u32::from(value.min(MODE_VALUE_MAX));
    // Each half of the ramp spans 500 thousandths; 255 * v / 500 is exact at both ends.
    if value <= 500 {
        let green = u8::try_from(value * 255 / 500).unwrap_or(u8::MAX);
        Rgba8::opaque(255, green, 0)
    } else {
        let red = u8::try_from((u32::from(MODE_VALUE_MAX) - value) * 255 / 500).unwrap_or(u8::MAX);
        Rgba8::opaque(red, 255, 0)
    }
}

/// The stockpile value of a place, in thousandths: `stored * 1000 / capacity` by volume, floored, never above 1000. A place with no
/// capacity is [`MODE_UNCOLOURED`].
#[must_use]
pub fn stockpile_value(stored: u64, capacity: u64) -> u16 {
    if capacity == 0 {
        return MODE_UNCOLOURED;
    }
    let value = u128::from(stored) * 1000 / u128::from(capacity);
    u16::try_from(value.min(u128::from(MODE_VALUE_MAX))).unwrap_or(MODE_VALUE_MAX)
}

/// The yield values of every place, in thousandths, from each place's projected annual yield (`None`: the place is not coloured):
/// `(y - min) * 1000 / (max - min)` across the coloured places, floored. When the maximum equals the minimum every coloured place is
/// 500.
#[must_use]
pub fn yield_values(yields: &[Option<u64>]) -> Vec<u16> {
    let coloured = || yields.iter().flatten().copied();
    let (Some(min), Some(max)) = (coloured().min(), coloured().max()) else {
        return vec![MODE_UNCOLOURED; yields.len()];
    };
    yields
        .iter()
        .map(|place| match place {
            None => MODE_UNCOLOURED,
            Some(_) if max == min => 500,
            Some(y) => {
                let value = u128::from(y - min) * 1000 / u128::from(max - min);
                u16::try_from(value).unwrap_or(MODE_VALUE_MAX)
            }
        })
        .collect()
}

/// The blended value of the crop cell at local column `x` and row `y` (`None` when no cell of its 5 x 5 box is counted).
#[must_use]
pub fn mode_value_at_cell(crop: Crop, inputs: &ModeInputs<'_>, x: u32, y: u32) -> Option<u16> {
    let width = i64::from(crop.width);
    let height = i64::from(crop.height);
    let (x, y) = (i64::from(x), i64::from(y));
    let mut sum = 0_u64;
    let mut count = 0_u64;
    for row in (y - REACH).max(0)..=(y + REACH).min(height - 1) {
        for column in (x - REACH).max(0)..=(x + REACH).min(width - 1) {
            let Ok(index) = usize::try_from(row * width + column) else {
                continue;
            };
            let Some(&owner) = inputs.owners.get(index) else {
                continue;
            };
            if owner == MODE_UNOWNED {
                continue;
            }
            let Some(&value) = inputs.values.get(usize::from(owner)) else {
                continue;
            };
            if value == MODE_UNCOLOURED {
                continue;
            }
            sum += u64::from(value.min(MODE_VALUE_MAX));
            count += 1;
        }
    }
    if count == 0 {
        return None;
    }
    u16::try_from(sum / count).ok()
}

/// The blended value under a (moved) sample point inside the crop.
pub(crate) fn mode_value_at_point(
    map: &MapData,
    inputs: &ModeInputs<'_>,
    point: FixedPoint,
) -> Option<u16> {
    let Cell { x, y } = cell_of(point);
    let local_x = u32::try_from(i64::from(x) - i64::from(map.crop.origin.x)).ok()?;
    let local_y = u32::try_from(i64::from(y) - i64::from(map.crop.origin.y)).ok()?;
    if local_x >= map.crop.width || local_y >= map.crop.height {
        return None;
    }
    mode_value_at_cell(map.crop, inputs, local_x, local_y)
}

/// The grey of the elevation mode (milestone 9, slice 10b): `⌊255 × √(max(0, e) ÷ 8,848)⌋` for a cell `metres` above sea level,
/// computed exactly as `isqrt(65,025 × max(0, e) ÷ 8,848)` (integer division, integer square root), clamped to 255, on every channel.
/// A height at or below sea level is black.
#[must_use]
pub fn elevation_grey(metres: i32) -> Rgba8 {
    let height = u64::try_from(metres.max(0)).unwrap_or(0);
    let scaled = 255_u64 * 255 * height / u64::from(ELEVATION_TOP_METRES);
    let grey = u8::try_from(scaled.isqrt().min(255)).unwrap_or(u8::MAX);
    Rgba8::opaque(grey, grey, grey)
}
