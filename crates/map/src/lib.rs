//! Backend-neutral map coordinates, camera math, reference colours and river coverage.
//!
//! The colour oracle is a backend-neutral integer reference for crop, jitter,
//! elevation, terrain, coast and river rules.

#![deny(unsafe_code)]

use std::collections::BTreeSet;

/// Projected metres in one map cell.
pub const CELL_METRES: f64 = 839.0;
/// Fixed-point units in one map cell.
pub const FIXED_PER_CELL: i64 = 32_768;
/// Spherical Mercator radius in metres.
pub const MERCATOR_RADIUS_METRES: f64 = 6_378_137.0;
/// Maximum absolute latitude supported by spherical Mercator.
pub const MAX_MERCATOR_LATITUDE: f64 = 85.051_128_779_806_6;

/// Frozen map-pixel sizes in fixed-point cell units for steps 0 through 75.
pub const STEP_SIZE: [i32; 76] = [
    196_608, 183_442, 171_157, 159_695, 149_001, 139_023, 129_713, 121_026, 112_922, 105_360,
    98_304, 91_721, 85_579, 79_848, 74_501, 69_511, 64_856, 60_513, 56_461, 52_680, 49_152, 45_860,
    42_789, 39_924, 37_250, 34_756, 32_428, 30_257, 28_230, 26_340, 24_576, 22_930, 21_395, 19_962,
    18_625, 17_378, 16_214, 15_128, 14_115, 13_170, 12_288, 11_465, 10_697, 9981, 9313, 8689, 8107,
    7564, 7058, 6585, 6144, 5733, 5349, 4990, 4656, 4344, 4054, 3782, 3529, 3292, 3072, 2866, 2674,
    2495, 2328, 2172, 2027, 1891, 1764, 1646, 1536, 1433, 1337, 1248, 1164, 1086,
];

/// A signed global cell coordinate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cell {
    /// Global cell column, increasing east.
    pub x: i32,
    /// Global cell row, increasing south.
    pub y: i32,
}

/// A signed global coordinate in 1/32768-cell units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FixedPoint {
    /// Global fixed-point east coordinate.
    pub x: i32,
    /// Global fixed-point south coordinate.
    pub y: i32,
}

/// Convert latitude and longitude to the fixed-point projected world.
///
/// Longitude wraps at the antimeridian. Latitude is clamped to the finite
/// spherical-Mercator world. Conversion floors, so an exact cell boundary belongs
/// to the cell east or south of that boundary.
#[must_use]
pub fn project(latitude_degrees: f64, longitude_degrees: f64) -> Option<FixedPoint> {
    if !latitude_degrees.is_finite() || !longitude_degrees.is_finite() {
        return None;
    }
    let latitude = latitude_degrees.clamp(-MAX_MERCATOR_LATITUDE, MAX_MERCATOR_LATITUDE);
    let longitude = (longitude_degrees + 180.0).rem_euclid(360.0) - 180.0;
    let half_world = std::f64::consts::PI * MERCATOR_RADIUS_METRES;
    let longitude_radians = longitude.to_radians();
    let latitude_radians = latitude.to_radians();
    let x_metres = MERCATOR_RADIUS_METRES * longitude_radians + half_world;
    let y_metres = half_world
        - MERCATOR_RADIUS_METRES
            * (std::f64::consts::FRAC_PI_4 + latitude_radians / 2.0)
                .tan()
                .ln();
    let scale = 32_768.0 / CELL_METRES;
    let x = (x_metres * scale).floor();
    let y = (y_metres * scale).floor();
    let x = format!("{x:.0}").parse::<i32>().ok()?;
    let y = format!("{y:.0}").parse::<i32>().ok()?;
    Some(FixedPoint { x, y })
}

/// Convert a fixed-point world position to the cell containing it.
#[must_use]
pub fn cell_of(position: FixedPoint) -> Cell {
    Cell {
        x: i32::try_from(i64::from(position.x).div_euclid(FIXED_PER_CELL)).unwrap_or_default(),
        y: i32::try_from(i64::from(position.y).div_euclid(FIXED_PER_CELL)).unwrap_or_default(),
    }
}

/// A global cell crop with a row-major data extent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crop {
    /// Global top-left cell.
    pub origin: Cell,
    /// Number of columns; must be nonzero.
    pub width: u32,
    /// Number of rows; must be nonzero.
    pub height: u32,
}

impl Crop {
    /// Whether this crop contains a global cell.
    #[must_use]
    pub fn contains(self, cell: Cell) -> bool {
        let dx = i64::from(cell.x) - i64::from(self.origin.x);
        let dy = i64::from(cell.y) - i64::from(self.origin.y);
        dx >= 0 && dy >= 0 && dx < i64::from(self.width) && dy < i64::from(self.height)
    }

    fn has_fixed_bounds(self) -> bool {
        if self.width == 0 || self.height == 0 {
            return false;
        }
        let minimum = i64::from(i32::MIN);
        let maximum = i64::from(i32::MAX);
        let left = i64::from(self.origin.x) * FIXED_PER_CELL;
        let top = i64::from(self.origin.y) * FIXED_PER_CELL;
        let right = (i64::from(self.origin.x) + i64::from(self.width)) * FIXED_PER_CELL;
        let bottom = (i64::from(self.origin.y) + i64::from(self.height)) * FIXED_PER_CELL;
        left >= minimum && top >= minimum && right <= maximum && bottom <= maximum
    }

    fn clamp_fixed(self, point: FixedPoint) -> FixedPoint {
        let min_x = i64::from(self.origin.x) * FIXED_PER_CELL;
        let min_y = i64::from(self.origin.y) * FIXED_PER_CELL;
        let max_x = (i64::from(self.origin.x) + i64::from(self.width)) * FIXED_PER_CELL;
        let max_y = (i64::from(self.origin.y) + i64::from(self.height)) * FIXED_PER_CELL;
        FixedPoint {
            x: i32::try_from(i64::from(point.x).clamp(min_x, max_x)).unwrap_or_default(),
            y: i32::try_from(i64::from(point.y).clamp(min_y, max_y)).unwrap_or_default(),
        }
    }
}

/// Integer map-pixel coordinate at a particular zoom step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorldPixel {
    /// Global pixel column.
    pub x: i32,
    /// Global pixel row.
    pub y: i32,
}

/// Number of map pixels visible on each axis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewSize {
    /// Visible map-pixel columns.
    pub width: u32,
    /// Visible map-pixel rows.
    pub height: u32,
}

impl ViewSize {
    /// Convert logical window points to map pixels, rounding each axis upward.
    #[must_use]
    pub const fn from_points(width: u32, height: u32) -> Self {
        Self {
            width: width.div_ceil(4),
            height: height.div_ceil(4),
        }
    }
}

/// Exact camera state; the visible top-left is always derived from this centre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Camera {
    /// Zoom step, 0 through 75.
    pub step: u8,
    /// Fixed-point world centre, clamped to the crop when constructed or panned.
    pub center: FixedPoint,
    /// Current top-left-anchored viewport size.
    pub view: ViewSize,
}

impl Camera {
    /// Construct a camera and clamp its centre to the crop extent.
    #[must_use]
    pub fn new(step: u8, center: FixedPoint, view: ViewSize, crop: Crop) -> Option<Self> {
        if usize::from(step) >= STEP_SIZE.len() || !crop.has_fixed_bounds() {
            return None;
        }
        Some(Self {
            step,
            center: crop.clamp_fixed(center),
            view,
        })
    }

    /// World-pixel top-left, using mathematical floor for negative world coordinates.
    #[must_use]
    pub fn top_left(self) -> WorldPixel {
        let p = i64::from(STEP_SIZE[usize::from(self.step)]);
        let x = i64::from(self.center.x).div_euclid(p) - i64::from(self.view.width / 2);
        let y = i64::from(self.center.y).div_euclid(p) - i64::from(self.view.height / 2);
        WorldPixel {
            x: clamp_i32(x),
            y: clamp_i32(y),
        }
    }

    /// Pan by an integral number of current-step world pixels.
    pub fn pan(&mut self, dx: i32, dy: i32, crop: Crop) {
        let p = i64::from(STEP_SIZE[usize::from(self.step)]);
        self.center = crop.clamp_fixed(FixedPoint {
            x: clamp_i32(i64::from(self.center.x) + i64::from(dx) * p),
            y: clamp_i32(i64::from(self.center.y) + i64::from(dy) * p),
        });
    }

    /// Change zoom while preserving the exact fixed-point centre.
    pub fn set_step(&mut self, step: u8) -> bool {
        if usize::from(step) >= STEP_SIZE.len() {
            return false;
        }
        self.step = step;
        true
    }

    /// Resize without moving the exact centre.
    pub fn resize(&mut self, view: ViewSize) {
        self.view = view;
    }

    /// Quantized displayed centre, which differs from the exact centre by less than one pixel.
    #[must_use]
    pub fn displayed_center(self) -> FixedPoint {
        let p = i64::from(STEP_SIZE[usize::from(self.step)]);
        let x = i64::from(self.center.x).div_euclid(p) * p + p / 2;
        let y = i64::from(self.center.y).div_euclid(p) * p + p / 2;
        FixedPoint {
            x: clamp_i32(x),
            y: clamp_i32(y),
        }
    }
}

fn clamp_i32(value: i64) -> i32 {
    i32::try_from(value.clamp(i64::from(i32::MIN), i64::from(i32::MAX))).unwrap_or_default()
}

/// RGBA8 bytes for the reference colour output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba8(pub [u8; 4]);

impl Rgba8 {
    /// Construct opaque RGB bytes.
    #[must_use]
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self([r, g, b, 255])
    }
}

/// Terrain palette and elevation thresholds used by the colour oracle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainStyle {
    /// Solid usable-land colour.
    pub solid: Rgba8,
    /// Second usable-land dither tone.
    pub second: Rgba8,
    /// Dark unusable-land colour.
    pub dark: Rgba8,
    /// Second unusable-land dither tone.
    pub dark_second: Rgba8,
    /// Usable mountain cap colour.
    pub cap: Rgba8,
    /// Unusable mountain high-ground colour.
    pub high_ground: Rgba8,
    /// Lower elevation used for the 16-level relief ramp.
    pub relief_low: i32,
    /// Upper elevation used for the 16-level relief ramp; greater than `relief_low`.
    pub relief_high: i32,
    /// Usable-land second-tone percentage, 0 through 100.
    pub mix_percent: u8,
    /// Lower cap elevation threshold.
    pub cap_low: i32,
    /// Upper cap elevation threshold; greater than `cap_low`.
    pub cap_high: i32,
    /// Whether this style is the mountain terrain.
    pub mountain: bool,
}

/// Surface classification and water depth of one crop cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Sea depth in the same units as the shallow threshold.
    Sea {
        /// Depth sample.
        depth: u16,
    },
    /// Lake water.
    Lake,
    /// Land with terrain index and usable flag.
    Land {
        /// Terrain palette index.
        terrain: u16,
        /// Whether the cell is available for development or extraction.
        usable: bool,
    },
}

/// Palette and data grids used by the reference colour function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapData {
    /// Global crop origin and extent.
    pub crop: Crop,
    /// Row-major cells, exactly `crop.width * crop.height` entries.
    pub cells: Vec<Surface>,
    /// Row-major encoded elevations (`metres + 11_000`) for every crop cell.
    pub elevations: Vec<u16>,
    /// Terrain palettes indexed by each land cell's terrain id.
    pub terrains: Vec<TerrainStyle>,
    /// Sea base tone.
    pub sea: Rgba8,
    /// Shallow-sea tone.
    pub shallow_sea: Rgba8,
    /// Lake tone.
    pub lake: Rgba8,
    /// River overlay tone.
    pub river: Rgba8,
    /// Depth strictly below this value may dither to shallow sea.
    pub shallow_threshold: u16,
}

impl MapData {
    /// Validate grid dimensions and palette ranges.
    ///
    /// # Errors
    /// Returns an error if the crop, grid, palette thresholds or terrain indices are invalid.
    pub fn validate(&self) -> Result<(), &'static str> {
        let expected = usize::try_from(self.crop.width)
            .ok()
            .and_then(|w| {
                usize::try_from(self.crop.height)
                    .ok()
                    .and_then(|h| w.checked_mul(h))
            })
            .ok_or("crop size overflows address space")?;
        if !self.crop.has_fixed_bounds() {
            return Err("crop fixed-point bounds exceed world-coordinate range");
        }
        if self.crop.origin.x < 0 || self.crop.origin.y < 0 {
            return Err("map crop must lie in the nonnegative projected world");
        }
        if self.cells.len() != expected {
            return Err("cell count does not match nonempty crop dimensions");
        }
        if self.elevations.len() != expected {
            return Err("elevation count does not match crop dimensions");
        }
        if self.shallow_threshold == 0 {
            return Err("shallow threshold must be nonzero");
        }
        for terrain in &self.terrains {
            if terrain.relief_high <= terrain.relief_low
                || terrain.cap_high <= terrain.cap_low
                || terrain.mix_percent > 100
            {
                return Err("invalid terrain thresholds");
            }
        }
        if self.cells.iter().any(|cell| matches!(cell, Surface::Land { terrain, .. } if usize::from(*terrain) >= self.terrains.len())) {
            return Err("cell references missing terrain style");
        }
        Ok(())
    }

    fn surface_at_point(&self, position: FixedPoint) -> Option<Surface> {
        let cell = cell_of(position);
        let Some(index) = self.cell_index(cell) else {
            return Some(Surface::Sea { depth: u16::MAX });
        };
        self.cells.get(index).copied()
    }

    fn elevation_at_cell(&self, x: u32, y: u32) -> Option<u16> {
        if x >= self.crop.width || y >= self.crop.height {
            return None;
        }
        let index = usize::try_from(y)
            .ok()?
            .checked_mul(usize::try_from(self.crop.width).ok()?)?
            .checked_add(usize::try_from(x).ok()?)?;
        self.elevations.get(index).copied()
    }

    fn elevation_at_point(&self, point: FixedPoint) -> Option<i32> {
        let x = u32::try_from(i64::from(point.x) - i64::from(self.crop.origin.x) * FIXED_PER_CELL)
            .ok()?;
        let y = u32::try_from(i64::from(point.y) - i64::from(self.crop.origin.y) * FIXED_PER_CELL)
            .ok()?;
        let gx = x.saturating_sub(u32::try_from(FIXED_PER_CELL / 2).ok()?);
        let gy = y.saturating_sub(u32::try_from(FIXED_PER_CELL / 2).ok()?);
        let x0 = (gx >> 15).min(self.crop.width - 1);
        let y0 = (gy >> 15).min(self.crop.height - 1);
        let fx = (gx & 32_767) >> 3;
        let fy = (gy & 32_767) >> 3;
        let x1 = (x0 + 1).min(self.crop.width - 1);
        let y1 = (y0 + 1).min(self.crop.height - 1);
        let top = lerp12(
            u32::from(self.elevation_at_cell(x0, y0)?),
            u32::from(self.elevation_at_cell(x1, y0)?),
            fx,
        );
        let bottom = lerp12(
            u32::from(self.elevation_at_cell(x0, y1)?),
            u32::from(self.elevation_at_cell(x1, y1)?),
            fx,
        );
        i32::try_from(lerp12(top, bottom, fy))
            .ok()?
            .checked_sub(11_000)
    }

    fn cell_index(&self, cell: Cell) -> Option<usize> {
        if !self.crop.contains(cell) {
            return None;
        }
        let x = usize::try_from(i64::from(cell.x) - i64::from(self.crop.origin.x)).ok()?;
        let y = usize::try_from(i64::from(cell.y) - i64::from(self.crop.origin.y)).ok()?;
        y.checked_mul(usize::try_from(self.crop.width).ok()?)?
            .checked_add(x)
    }
}

/// Sample point at the centre of a world pixel, if it fits the fixed-point range.
#[must_use]
pub fn sample_point(pixel: WorldPixel, step: u8) -> Option<FixedPoint> {
    let p = i64::from(*STEP_SIZE.get(usize::from(step))?);
    let half = p / 2;
    let x = i64::from(pixel.x).checked_mul(p)?.checked_add(half)?;
    let y = i64::from(pixel.y).checked_mul(p)?.checked_add(half)?;
    Some(FixedPoint {
        x: i32::try_from(x).ok()?,
        y: i32::try_from(y).ok()?,
    })
}

/// Standard 4×4 Bayer ordered-dither threshold indexed by global world pixels.
#[must_use]
pub fn bayer(pixel: WorldPixel) -> u8 {
    const MATRIX: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    MATRIX[pixel.y.rem_euclid(4) as usize][pixel.x.rem_euclid(4) as usize]
}

/// Return the reference colour for one world pixel.
#[must_use]
pub fn reference_colour(
    map: &MapData,
    step: u8,
    pixel: WorldPixel,
    rivers: &BTreeSet<WorldPixel>,
) -> Option<Rgba8> {
    if usize::from(step) >= STEP_SIZE.len() {
        return None;
    }
    if map.shallow_threshold == 0 {
        return None;
    }
    let b = u32::from(bayer(pixel));
    let unmoved = sample_point(pixel, step)?;
    if !inside_crop(map.crop, unmoved) {
        return Some(map.sea);
    }
    let moved = jittered_point(unmoved, step)?;
    if !inside_crop(map.crop, moved) {
        return Some(map.sea);
    }
    let surface = map.surface_at_point(moved)?;
    let mut colour = match surface {
        Surface::Sea { depth } => {
            let t = u32::from(map.shallow_threshold);
            let d = u32::from(depth);
            if d < t && b < 8 * (t - d) / t {
                map.shallow_sea
            } else {
                map.sea
            }
        }
        Surface::Lake => map.lake,
        Surface::Land { terrain, usable } => {
            let style = *map.terrains.get(usize::from(terrain))?;
            if style.relief_high <= style.relief_low
                || style.cap_high <= style.cap_low
                || style.mix_percent > 100
            {
                return None;
            }
            let elevation = map.elevation_at_point(moved)?;
            let level = ramp(elevation, style.relief_low, style.relief_high);
            let mut base = if usable {
                if b < (level * u32::from(style.mix_percent)) / 100 {
                    style.second
                } else {
                    style.solid
                }
            } else if b < level / 2 {
                style.dark_second
            } else {
                style.dark
            };
            if style.mountain && b < ramp(elevation, style.cap_low, style.cap_high) {
                base = if usable { style.cap } else { style.high_ground };
            }
            base
        }
    };
    if matches!(surface, Surface::Land { .. })
        && has_water_neighbour(map, step, pixel)
        && let Surface::Land { terrain, .. } = surface
    {
        colour = map.terrains.get(usize::from(terrain))?.solid;
    }
    if rivers.contains(&pixel) {
        colour = map.river;
    }
    Some(colour)
}

fn ramp(value: i32, low: i32, high: i32) -> u32 {
    if value <= low {
        0
    } else if value >= high {
        16
    } else {
        u32::try_from((i64::from(value) - i64::from(low)) * 16 / (i64::from(high) - i64::from(low)))
            .unwrap_or_default()
    }
}

fn has_water_neighbour(map: &MapData, step: u8, pixel: WorldPixel) -> bool {
    [(-1, 0), (1, 0), (0, -1), (0, 1)]
        .into_iter()
        .any(|(dx, dy)| {
            let x = i64::from(pixel.x) + dx;
            let y = i64::from(pixel.y) + dy;
            let Ok(x) = i32::try_from(x) else {
                return false;
            };
            let Ok(y) = i32::try_from(y) else {
                return false;
            };
            matches!(
                surface_for_pixel(map, step, WorldPixel { x, y }),
                Some(Surface::Sea { .. } | Surface::Lake)
            )
        })
}

fn surface_for_pixel(map: &MapData, step: u8, pixel: WorldPixel) -> Option<Surface> {
    let unmoved = sample_point(pixel, step)?;
    if !inside_crop(map.crop, unmoved) {
        return Some(Surface::Sea { depth: u16::MAX });
    }
    let moved = jittered_point(unmoved, step)?;
    if !inside_crop(map.crop, moved) {
        return Some(Surface::Sea { depth: u16::MAX });
    }
    map.surface_at_point(moved)
}

fn inside_crop(crop: Crop, point: FixedPoint) -> bool {
    let x = i64::from(point.x);
    let y = i64::from(point.y);
    let left = i64::from(crop.origin.x) * FIXED_PER_CELL;
    let top = i64::from(crop.origin.y) * FIXED_PER_CELL;
    let right = (i64::from(crop.origin.x) + i64::from(crop.width)) * FIXED_PER_CELL;
    let bottom = (i64::from(crop.origin.y) + i64::from(crop.height)) * FIXED_PER_CELL;
    x >= 0 && y >= 0 && x >= left && y >= top && x < right && y < bottom
}

const JITTER_OCTAVES: [(u8, u32, u32, u32, u32); 3] = [
    (14, 98_304, 29_491, 1, 2),
    (34, 22_938, 9_175, 3, 4),
    (53, 5_898, 2_294, 5, 6),
];

fn lowbias32(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

fn corner_hash(ix: u32, iy: u32, seed: u32) -> u32 {
    lowbias32(
        ix.wrapping_mul(0x8da6_b343)
            ^ iy.wrapping_mul(0xd816_3841)
            ^ seed.wrapping_mul(0xcb1a_b31f),
    )
}

fn smooth12(t: u32) -> u32 {
    let a = (t * t) >> 12;
    (a * (12_288 - 2 * t)) >> 12
}

fn lerp12(a: u32, b: u32, fraction: u32) -> u32 {
    (a * (4096 - fraction) + b * fraction) >> 12
}

fn noise12(x: u32, y: u32, lattice: u32, seed: u32) -> u32 {
    let ix = x / lattice;
    let iy = y / lattice;
    let fx = x % lattice;
    let fy = y % lattice;
    let tx = smooth12(fx * 4096 / lattice);
    let ty = smooth12(fy * 4096 / lattice);
    let top = lerp12(
        corner_hash(ix, iy, seed) >> 20,
        corner_hash(ix + 1, iy, seed) >> 20,
        tx,
    );
    let bottom = lerp12(
        corner_hash(ix, iy + 1, seed) >> 20,
        corner_hash(ix + 1, iy + 1, seed) >> 20,
        tx,
    );
    lerp12(top, bottom, ty)
}

fn noise_offset(x: u32, y: u32, lattice: u32, amplitude: u32, seed: u32) -> i32 {
    let noise = noise12(x, y, lattice, seed);
    i32::try_from((noise * 2 * amplitude) >> 12).unwrap_or_default()
        - i32::try_from(amplitude).unwrap_or_default()
}

fn jittered_point(point: FixedPoint, step: u8) -> Option<FixedPoint> {
    let x = u32::try_from(point.x).ok()?;
    let y = u32::try_from(point.y).ok()?;
    let mut dx = 0_i32;
    let mut dy = 0_i32;
    for (first_step, lattice, amplitude, x_seed, y_seed) in JITTER_OCTAVES {
        if step >= first_step {
            dx += noise_offset(x, y, lattice, amplitude, x_seed);
            dy += noise_offset(x, y, lattice, amplitude, y_seed);
        }
    }
    Some(FixedPoint {
        x: i32::try_from(i64::from(point.x) + i64::from(dx)).ok()?,
        y: i32::try_from(i64::from(point.y) + i64::from(dy)).ok()?,
    })
}

/// One authored river polyline in fixed-point world coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiverLine {
    /// Ordered vertices; each consecutive pair is joined with integer Bresenham.
    pub points: Vec<FixedPoint>,
}

/// Deduplicated sorted river-pixel coverage for a zoom step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RiverCoverage {
    pixels: BTreeSet<WorldPixel>,
}

impl RiverCoverage {
    /// Prepare all lines once for the given zoom step.
    #[must_use]
    pub fn prepare(lines: &[RiverLine], step: u8) -> Option<Self> {
        let p = i64::from(*STEP_SIZE.get(usize::from(step))?);
        let mut pixels = BTreeSet::new();
        for line in lines {
            let mut points = line.points.iter().map(|point| fixed_to_pixel(*point, p));
            let Some(mut previous) = points.next() else {
                continue;
            };
            pixels.insert(previous);
            for next in points {
                bresenham(previous, next, &mut pixels)?;
                previous = next;
            }
        }
        Some(Self { pixels })
    }

    /// All prepared pixels in sorted order.
    #[must_use]
    pub fn pixels(&self) -> &BTreeSet<WorldPixel> {
        &self.pixels
    }

    /// Select river pixels intersecting a view; output cannot exceed its area.
    #[must_use]
    pub fn select(&self, top_left: WorldPixel, view: ViewSize) -> Vec<WorldPixel> {
        let right = i64::from(top_left.x) + i64::from(view.width);
        let bottom = i64::from(top_left.y) + i64::from(view.height);
        self.pixels
            .iter()
            .copied()
            .filter(|pixel| {
                let x = i64::from(pixel.x);
                let y = i64::from(pixel.y);
                x >= i64::from(top_left.x) && x < right && y >= i64::from(top_left.y) && y < bottom
            })
            .collect()
    }
}

fn fixed_to_pixel(point: FixedPoint, pixel_size: i64) -> WorldPixel {
    WorldPixel {
        x: clamp_i32(i64::from(point.x).div_euclid(pixel_size)),
        y: clamp_i32(i64::from(point.y).div_euclid(pixel_size)),
    }
}

fn bresenham(start: WorldPixel, end: WorldPixel, pixels: &mut BTreeSet<WorldPixel>) -> Option<()> {
    let (mut x, mut y) = (i64::from(start.x), i64::from(start.y));
    let (end_x, end_y) = (i64::from(end.x), i64::from(end.y));
    let dx = (end_x - x).abs();
    let sx = if x < end_x { 1 } else { -1 };
    let dy = -(end_y - y).abs();
    let sy = if y < end_y { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        pixels.insert(WorldPixel {
            x: i32::try_from(x).ok()?,
            y: i32::try_from(y).ok()?,
        });
        if x == end_x && y == end_y {
            break;
        }
        let doubled = 2 * error;
        if doubled >= dy {
            error += dy;
            x += sx;
        }
        if doubled <= dx {
            error += dx;
            y += sy;
        }
    }
    Some(())
}

#[cfg(test)]
mod tests;
