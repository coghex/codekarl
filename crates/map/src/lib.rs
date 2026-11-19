//! Backend-neutral map coordinates, camera math, reference colours and river coverage.
//!
//! The colour oracle is a backend-neutral integer reference for crop, jitter,
//! elevation, terrain, coast and river rules.

#![deny(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

#[cfg(feature = "test-support")]
pub mod fixture;
mod loading;
pub mod look;
mod mode;
mod scenario;

pub use loading::{
    LoadError, LoadedMap, LoadedTerrain, MAX_CROP_DIMENSION, PackedGrid, TerrainKind, load_map,
};

/// A format-2 river network, projected and validated at load time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiverNetwork {
    /// First zoom step at which each tier is visible (tiers are 1-based).
    pub show_from_step: [u8; 3],
    /// Named rivers in file order.
    pub rivers: Vec<RiverRiver>,
    /// Directed upstream stretches in file order.
    pub stretches: Vec<RiverStretch>,
}

/// A named river and its display tier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiverRiver {
    /// Stable river id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Visibility tier, 1 through 3.
    pub tier: u8,
}

/// The surveyed kind of a network stretch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiverStretchKind {
    /// Inland river course.
    River,
    /// Tidal course.
    Tidal,
    /// Course surveyed through a lough.
    Lough,
}

/// A validated stretch with widths in fixed map units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiverStretch {
    /// Stable stretch id.
    pub id: String,
    /// Index into [`RiverNetwork::rivers`].
    pub river: usize,
    /// Surveyed stretch kind.
    pub kind: RiverStretchKind,
    /// Ordered projected points, upstream to downstream.
    pub points: Vec<FixedPoint>,
    /// Authored width at the first point, in fixed map units.
    pub width_first: i64,
    /// Authored width at the last point, in fixed map units.
    pub width_last: i64,
    /// Index of the downstream stretch, or `None` for sea.
    pub ends: Option<usize>,
    /// Width after applying the nonshrinking confluence rule.
    pub drawn_first: i64,
    /// Width after applying the nonshrinking confluence rule.
    pub drawn_last: i64,
}

/// Line and dark-water pixels prepared for one network tile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RiverCoverageParts {
    /// One-pixel blue river line.
    pub line: BTreeSet<WorldPixel>,
    /// Wide dark-water portion.
    pub water: BTreeSet<WorldPixel>,
    /// Land pixels beside river water, mapped to their terrain-style index.
    pub banks: BTreeMap<WorldPixel, u16>,
}

/// The two bounded draw lists for a format-2 map view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RiverNetworkView {
    /// River-line pixels in view coordinates.
    pub rivers: Vec<[u32; 2]>,
    /// Indexed bank, location and river-water pixels in view coordinates.
    pub indexed: Vec<[u32; 3]>,
}
pub use mode::{
    ELEVATION_TOP_METRES, MODE_UNCOLOURED, MODE_UNOWNED, MODE_VALUE_MAX, ModeInputs,
    elevation_grey, mode_colour, mode_value_at_cell, stockpile_value, yield_values,
};
pub use scenario::{
    BuildingKind, Confidence, HouseStyle, Location, LocationKind, LocationKinds, LocationPixel,
    MarkerPixel, PreparedLocations, PreparedMarkers, Scenario, ScenarioStart, ground_scale_for_row,
    load_scenario,
};

/// The offset of the stored elevations: a cell's `elevations` entry is its height in metres plus this.
const ELEVATION_OFFSET: i32 = 11_000;
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

/// A land terrain's four tones (`docs/map-design.md` §12): full colour for fertile land, the dark colour for barren land, and the
/// two tones the hillside dither brings in on the shadow and the lit side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainStyle {
    /// Fertile land: full colour, and the coast outline's.
    pub solid: Rgba8,
    /// A bright pixel on the shadow side of a hillside.
    pub second: Rgba8,
    /// Barren land: the dark colour.
    pub dark: Rgba8,
    /// A dark pixel on the lit side of a hillside: `dark + 2 × (dark_second − dark)`, written out in the data.
    pub dark_lit: Rgba8,
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
    /// Fertility in per-thousand units, one entry per crop cell.
    pub fertility: Vec<u16>,
    /// Mean annual temperature in tenths of a degree Celsius.
    pub climate_temperature_tenths: Vec<i16>,
    /// Growing degree-days above 5 °C, rounded to whole degree-days.
    pub climate_gdd: Vec<u16>,
    /// Annual rainfall in whole millimetres.
    pub climate_rainfall_mm: Vec<u16>,
    /// Whether each crop cell is glaciated by the climate model.
    pub glacier: Vec<bool>,
    /// The hillside strength of every crop cell, −15 through +15 (positive faces the light; zero for water): see [`look`].
    pub hillside: Vec<i8>,
    /// The glacier colour.
    pub ice: Rgba8,
    /// The glacier's second tone, on the shadow side.
    pub ice_second: Rgba8,
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
        if self.fertility.len() != expected
            || self.climate_temperature_tenths.len() != expected
            || self.climate_gdd.len() != expected
            || self.climate_rainfall_mm.len() != expected
            || self.glacier.len() != expected
            || self.hillside.len() != expected
        {
            return Err("climate and fertility counts do not match crop dimensions");
        }
        if self.fertility.iter().any(|value| *value > 1000)
            || self
                .climate_temperature_tenths
                .iter()
                .any(|value| !(-600..=500).contains(value))
            || self.climate_gdd.iter().any(|value| *value > 12_000)
            || self.climate_rainfall_mm.iter().any(|value| *value > 15_000)
        {
            return Err("climate or fertility value is outside supported bounds");
        }
        if self
            .hillside
            .iter()
            .any(|strength| !(-15..=15).contains(strength))
        {
            return Err("hillside strength is outside -15..=15");
        }
        if self.shallow_threshold == 0 {
            return Err("shallow threshold must be nonzero");
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
    reference_colour_with_locations(map, step, pixel, rivers, &BTreeMap::new())
}

/// Reference colour with prepared location pixels keyed by global world pixel.
/// Rivers retain their final-draw precedence over locations and terrain.
#[must_use]
pub fn reference_colour_with_locations(
    map: &MapData,
    step: u8,
    pixel: WorldPixel,
    rivers: &BTreeSet<WorldPixel>,
    locations: &BTreeMap<WorldPixel, Rgba8>,
) -> Option<Rgba8> {
    reference_colour_with_mode(map, step, pixel, rivers, locations, None)
}

/// Reference colour with separate format-2 river line and dark-water pixels.
#[must_use]
pub fn reference_colour_with_river_network(
    map: &MapData,
    step: u8,
    pixel: WorldPixel,
    line: &BTreeSet<WorldPixel>,
    water: &BTreeSet<WorldPixel>,
    locations: &BTreeMap<WorldPixel, Rgba8>,
    mode: Option<&ModeInputs<'_>>,
) -> Option<Rgba8> {
    reference_colour_with_river_network_look(
        map,
        step,
        pixel,
        line,
        water,
        locations,
        mode.map_or(Look::Terrain, Look::Values),
    )
}

/// Reference colour for a format-2 river network in elevation mode.
#[must_use]
pub fn reference_colour_with_river_network_elevation(
    map: &MapData,
    step: u8,
    pixel: WorldPixel,
    line: &BTreeSet<WorldPixel>,
    water: &BTreeSet<WorldPixel>,
    locations: &BTreeMap<WorldPixel, Rgba8>,
) -> Option<Rgba8> {
    reference_colour_with_river_network_look(
        map,
        step,
        pixel,
        line,
        water,
        locations,
        Look::Elevation,
    )
}

fn reference_colour_with_river_network_look(
    map: &MapData,
    step: u8,
    pixel: WorldPixel,
    line: &BTreeSet<WorldPixel>,
    water: &BTreeSet<WorldPixel>,
    locations: &BTreeMap<WorldPixel, Rgba8>,
    look: Look<'_>,
) -> Option<Rgba8> {
    if usize::from(step) >= STEP_SIZE.len() || map.shallow_threshold == 0 {
        return None;
    }
    if line.contains(&pixel) {
        return Some(map.river);
    }
    if water.contains(&pixel) {
        return Some(map.lake);
    }
    if let Some(colour) = locations.get(&pixel) {
        return Some(*colour);
    }
    let surface = surface_for_pixel(map, step, pixel)?;
    if let Surface::Land { terrain, .. } = surface
        && [(-1, 0), (1, 0), (0, -1), (0, 1)]
            .into_iter()
            .any(|(dx, dy)| {
                i32::try_from(i64::from(pixel.x) + dx)
                    .ok()
                    .zip(i32::try_from(i64::from(pixel.y) + dy).ok())
                    .is_some_and(|(x, y)| water.contains(&WorldPixel { x, y }))
            })
    {
        return Some(map.terrains.get(usize::from(terrain))?.solid);
    }
    reference_colour_with_look(map, step, pixel, &BTreeSet::new(), locations, look)
}

fn river_width_at(first: i64, last: i64, position: i64, total: i64) -> Option<i64> {
    i64::try_from(
        i128::from(first) + (i128::from(last - first) * i128::from(position)) / i128::from(total),
    )
    .ok()
}

/// Apply the downstream nonshrinking rule to a validated acyclic network.
pub(crate) fn apply_confluence_widths(stretches: &mut [RiverStretch]) {
    let mut pending = vec![0_usize; stretches.len()];
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); stretches.len()];
    let mut incoming_width = vec![0_i64; stretches.len()];
    for (upstream, stretch) in stretches.iter().enumerate() {
        if let Some(downstream) = stretch.ends {
            pending[downstream] += 1;
            outgoing[upstream].push(downstream);
        }
    }
    let mut ready: std::collections::VecDeque<usize> = pending
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count == 0).then_some(index))
        .collect();
    while let Some(index) = ready.pop_front() {
        stretches[index].drawn_first = stretches[index].width_first.max(incoming_width[index]);
        stretches[index].drawn_last = stretches[index]
            .width_last
            .max(stretches[index].drawn_first);
        for &downstream in &outgoing[index] {
            incoming_width[downstream] =
                incoming_width[downstream].max(stretches[index].drawn_last);
            pending[downstream] -= 1;
            if pending[downstream] == 0 {
                ready.push_back(downstream);
            }
        }
    }
}

/// The reference colour with a map mode (`docs/economy-design.md` section 3.22): rivers and locations first, as without a mode; then,
/// when `mode` is given and the cell under the pixel's (moved) sample point has a counted neighbour in its 5 x 5 box, the mode's colour
/// ([`mode_value_at_cell`], [`mode_colour`]); otherwise the terrain colour. With `mode` of `None` this is
/// [`reference_colour_with_locations`] exactly.
#[must_use]
pub fn reference_colour_with_mode(
    map: &MapData,
    step: u8,
    pixel: WorldPixel,
    rivers: &BTreeSet<WorldPixel>,
    locations: &BTreeMap<WorldPixel, Rgba8>,
    mode: Option<&ModeInputs<'_>>,
) -> Option<Rgba8> {
    reference_colour_with_look(
        map,
        step,
        pixel,
        rivers,
        locations,
        mode.map_or(Look::Terrain, Look::Values),
    )
}

/// The reference colour in the elevation mode (milestone 9, slice 10b): every land pixel that is not on the coast outline is the grey
/// of its cell's elevation ([`elevation_grey`]); sea, lake, the coast outline, rivers and locations are as without a mode.
#[must_use]
pub fn reference_colour_with_elevation(
    map: &MapData,
    step: u8,
    pixel: WorldPixel,
    rivers: &BTreeSet<WorldPixel>,
    locations: &BTreeMap<WorldPixel, Rgba8>,
) -> Option<Rgba8> {
    reference_colour_with_look(map, step, pixel, rivers, locations, Look::Elevation)
}

/// How a land pixel is coloured.
#[derive(Clone, Copy)]
enum Look<'a> {
    /// The base look of `look::land_colour`.
    Terrain,
    /// A per-place value mode (stockpile, yield).
    Values(&'a ModeInputs<'a>),
    /// The elevation mode: black to white by the square root of the height.
    Elevation,
}

fn reference_colour_with_look(
    map: &MapData,
    step: u8,
    pixel: WorldPixel,
    rivers: &BTreeSet<WorldPixel>,
    locations: &BTreeMap<WorldPixel, Rgba8>,
    look: Look<'_>,
) -> Option<Rgba8> {
    if usize::from(step) >= STEP_SIZE.len() {
        return None;
    }
    if map.shallow_threshold == 0 {
        return None;
    }
    if rivers.contains(&pixel) {
        return Some(map.river);
    }
    if let Some(&location_colour) = locations.get(&pixel) {
        return Some(location_colour);
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
    if let Look::Values(mode) = look
        && let Some(value) = mode::mode_value_at_point(map, mode, moved)
    {
        return Some(mode_colour(value));
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
        Surface::Land { terrain, .. } => {
            let cell = cell_of(moved);
            let Some(index) = map.cell_index(cell) else {
                return Some(map.sea);
            };
            let style = map.terrains.get(usize::from(terrain))?;
            if matches!(look, Look::Elevation) {
                let metres = i32::from(*map.elevations.get(index)?) - ELEVATION_OFFSET;
                elevation_grey(metres)
            } else {
                look::land_colour(
                    style,
                    (map.ice, map.ice_second),
                    *map.fertility.get(index)?,
                    *map.glacier.get(index)?,
                    *map.hillside.get(index)?,
                    pixel,
                )
            }
        }
    };
    if matches!(surface, Surface::Land { .. })
        && has_water_neighbour(map, step, pixel)
        && let Surface::Land { terrain, .. } = surface
    {
        colour = map.terrains.get(usize::from(terrain))?.solid;
    }
    Some(colour)
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

/// Lazily prepared river coverage for one immutable map.
///
/// The application owns one cache for the loaded map. A zoom step is rasterized
/// the first time it is shown and reused for subsequent pans at that step. A new
/// loaded map gets a new cache, so coverage cannot outlive its source lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreparedRivers {
    lines: Vec<RiverLine>,
    steps: Vec<Option<RiverCoverage>>,
    network: Option<PreparedRiverNetwork>,
}

impl PreparedRivers {
    /// Create an empty per-map cache from the map's projected river lines.
    #[must_use]
    pub fn new(lines: Vec<RiverLine>) -> Self {
        Self {
            lines,
            steps: vec![None; STEP_SIZE.len()],
            network: None,
        }
    }

    /// Create the format-appropriate river selector for a loaded map.
    #[must_use]
    pub fn for_map(loaded: &LoadedMap) -> Self {
        let mut prepared = Self::new(loaded.rivers.clone());
        if let Some(network) = &loaded.river_network {
            prepared.network = Some(PreparedRiverNetwork::new(&loaded.data, network));
        }
        prepared
    }

    /// Return the retained coverage for `step`, preparing it once on first use.
    ///
    /// Invalid steps return `None` and do not alter the cache.
    pub fn coverage(&mut self, step: u8) -> Option<&RiverCoverage> {
        let slot = self.steps.get_mut(usize::from(step))?;
        if slot.is_none() {
            *slot = RiverCoverage::prepare(&self.lines, step);
        }
        slot.as_ref()
    }

    /// Select the visible pixels in view coordinates without rebuilding the
    /// step's coverage. Output is bounded by the view's area.
    pub fn select_view(
        &mut self,
        step: u8,
        top_left: WorldPixel,
        view: ViewSize,
    ) -> Option<Vec<[u32; 2]>> {
        let coverage = self.coverage(step)?;
        let mut selected = Vec::new();
        for pixel in coverage.select(top_left, view) {
            selected.push([
                u32::try_from(i64::from(pixel.x) - i64::from(top_left.x)).ok()?,
                u32::try_from(i64::from(pixel.y) - i64::from(top_left.y)).ok()?,
            ]);
        }
        Some(selected)
    }

    /// Select the line and indexed overlays for a view, using the network path when available.
    pub fn select_map_view(
        &mut self,
        step: u8,
        top_left: WorldPixel,
        view: ViewSize,
        locations: &[[u32; 3]],
        location_palette_len: usize,
    ) -> Option<RiverNetworkView> {
        if let Some(network) = &mut self.network {
            network.select_map_view(step, top_left, view, locations, location_palette_len)
        } else {
            let rivers = self.select_view(step, top_left, view)?;
            Some(RiverNetworkView {
                rivers,
                indexed: locations.to_vec(),
            })
        }
    }
}

/// Tile-backed view of a format-2 network. Only requested 128-pixel tiles are rasterized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedRiverNetwork {
    map: MapData,
    network: RiverNetwork,
    tiles: BTreeMap<(u8, i32, i32), RiverCoverageParts>,
    current_step: Option<u8>,
    step_index: Option<BTreeMap<(i32, i32), Vec<usize>>>,
    bank_tiles: BTreeSet<(u8, i32, i32)>,
}

impl PreparedRiverNetwork {
    /// Prepare the lazy tile cache for one loaded map.
    #[must_use]
    pub fn new(map: &MapData, network: &RiverNetwork) -> Self {
        Self {
            map: map.clone(),
            network: network.clone(),
            tiles: BTreeMap::new(),
            current_step: None,
            step_index: None,
            bank_tiles: BTreeSet::new(),
        }
    }

    /// Select the network's line and water pixels in view-relative coordinates.
    #[expect(
        clippy::too_many_lines,
        reason = "keep view tile acquisition, bank preparation and clipping in one cache operation"
    )]
    pub fn select_view(
        &mut self,
        step: u8,
        top_left: WorldPixel,
        view: ViewSize,
    ) -> Option<RiverCoverageParts> {
        if self.current_step != Some(step) {
            self.tiles.clear();
            self.bank_tiles.clear();
            self.step_index = None;
            self.current_step = Some(step);
        }
        let pixel_size = i64::from(*STEP_SIZE.get(usize::from(step))?);
        if self.step_index.is_none() {
            self.step_index = Some(self.build_step_index(pixel_size)?);
        }
        let mut selected = RiverCoverageParts::default();
        let right = i64::from(top_left.x).checked_add(i64::from(view.width))?;
        let bottom = i64::from(top_left.y).checked_add(i64::from(view.height))?;
        if view.width == 0 || view.height == 0 {
            return Some(selected);
        }
        let tx0 = i64::from(top_left.x).div_euclid(128);
        let ty0 = i64::from(top_left.y).div_euclid(128);
        let tx1 = (right - 1).div_euclid(128);
        let ty1 = (bottom - 1).div_euclid(128);
        // Banks can depend on water anywhere in one of the eight neighbouring
        // tiles. Prepare that neighbourhood before caching this tile's banks.
        for ty in ty0 - 1..=ty1 + 1 {
            for tx in tx0 - 1..=tx1 + 1 {
                let tx = i32::try_from(tx).ok()?;
                let ty = i32::try_from(ty).ok()?;
                if !self.tiles.contains_key(&(step, tx, ty)) {
                    let candidates = self
                        .step_index
                        .as_ref()?
                        .get(&(tx, ty))
                        .map_or(&[][..], Vec::as_slice);
                    let tile = self.prepare_tile(step, pixel_size, tx, ty, candidates)?;
                    self.tiles.insert((step, tx, ty), tile);
                }
            }
        }
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let tx = i32::try_from(tx).ok()?;
                let ty = i32::try_from(ty).ok()?;
                self.prepare_banks(step, tx, ty)?;
                let tile = self.tiles.get(&(step, tx, ty))?;
                for pixel in &tile.line {
                    if i64::from(pixel.x) >= i64::from(top_left.x)
                        && i64::from(pixel.x) < right
                        && i64::from(pixel.y) >= i64::from(top_left.y)
                        && i64::from(pixel.y) < bottom
                    {
                        selected.line.insert(*pixel);
                    }
                }
                for pixel in &tile.water {
                    if i64::from(pixel.x) >= i64::from(top_left.x)
                        && i64::from(pixel.x) < right
                        && i64::from(pixel.y) >= i64::from(top_left.y)
                        && i64::from(pixel.y) < bottom
                    {
                        selected.water.insert(*pixel);
                    }
                }
                for (pixel, style) in &tile.banks {
                    if i64::from(pixel.x) >= i64::from(top_left.x)
                        && i64::from(pixel.x) < right
                        && i64::from(pixel.y) >= i64::from(top_left.y)
                        && i64::from(pixel.y) < bottom
                    {
                        selected.banks.insert(*pixel, *style);
                    }
                }
            }
        }
        Some(RiverCoverageParts {
            line: selected
                .line
                .into_iter()
                .map(|p| WorldPixel {
                    x: i32::try_from(i64::from(p.x) - i64::from(top_left.x))
                        .ok()
                        .unwrap_or_default(),
                    y: i32::try_from(i64::from(p.y) - i64::from(top_left.y))
                        .ok()
                        .unwrap_or_default(),
                })
                .collect(),
            water: selected
                .water
                .into_iter()
                .map(|p| WorldPixel {
                    x: i32::try_from(i64::from(p.x) - i64::from(top_left.x))
                        .ok()
                        .unwrap_or_default(),
                    y: i32::try_from(i64::from(p.y) - i64::from(top_left.y))
                        .ok()
                        .unwrap_or_default(),
                })
                .collect(),
            banks: selected
                .banks
                .into_iter()
                .map(|(p, style)| {
                    (
                        WorldPixel {
                            x: i32::try_from(i64::from(p.x) - i64::from(top_left.x))
                                .ok()
                                .unwrap_or_default(),
                            y: i32::try_from(i64::from(p.y) - i64::from(top_left.y))
                                .ok()
                                .unwrap_or_default(),
                        },
                        style,
                    )
                })
                .collect(),
        })
    }

    /// Compose the network and already-selected location pixels through the existing map lists.
    pub fn select_map_view(
        &mut self,
        step: u8,
        top_left: WorldPixel,
        view: ViewSize,
        locations: &[[u32; 3]],
        location_palette_len: usize,
    ) -> Option<RiverNetworkView> {
        let parts = self.select_view(step, top_left, view)?;
        let lake_index = u32::try_from(location_palette_len).ok()?;
        let terrain_offset = lake_index.checked_add(1)?;
        let mut indexed = BTreeMap::new();
        let mut location_by_pixel = BTreeMap::new();
        for &[x, y, index] in locations {
            if x < view.width && y < view.height {
                location_by_pixel.insert((x, y), index);
            }
        }
        for (pixel, style) in parts.banks {
            let x = u32::try_from(pixel.x).ok()?;
            let y = u32::try_from(pixel.y).ok()?;
            if x >= view.width || y >= view.height || location_by_pixel.contains_key(&(x, y)) {
                continue;
            }
            indexed.insert((x, y), terrain_offset.checked_add(u32::from(style))?);
        }
        let water: BTreeSet<_> = parts
            .water
            .iter()
            .filter_map(|pixel| Some((u32::try_from(pixel.x).ok()?, u32::try_from(pixel.y).ok()?)))
            .collect();
        for &[x, y, index] in locations {
            if x < view.width && y < view.height && !water.contains(&(x, y)) {
                indexed.insert((x, y), index);
            }
        }
        for pixel in parts.water {
            let x = u32::try_from(pixel.x).ok()?;
            let y = u32::try_from(pixel.y).ok()?;
            indexed.insert((x, y), lake_index);
        }
        Some(RiverNetworkView {
            rivers: parts
                .line
                .into_iter()
                .map(|pixel| Some([u32::try_from(pixel.x).ok()?, u32::try_from(pixel.y).ok()?]))
                .collect::<Option<Vec<_>>>()?,
            indexed: indexed
                .into_iter()
                .map(|((x, y), index)| [x, y, index])
                .collect(),
        })
    }

    fn prepare_banks(&mut self, step: u8, tx: i32, ty: i32) -> Option<()> {
        if self.bank_tiles.contains(&(step, tx, ty)) {
            return Some(());
        }
        let left = i64::from(tx).checked_mul(128)?;
        let top = i64::from(ty).checked_mul(128)?;
        let mut candidates = BTreeSet::new();
        for ny in ty.saturating_sub(1)..=ty.saturating_add(1) {
            for nx in tx.saturating_sub(1)..=tx.saturating_add(1) {
                if let Some(tile) = self.tiles.get(&(step, nx, ny)) {
                    for water in &tile.water {
                        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                            let x = i64::from(water.x) + dx;
                            let y = i64::from(water.y) + dy;
                            if x >= left && x < left + 128 && y >= top && y < top + 128 {
                                candidates.insert(WorldPixel {
                                    x: i32::try_from(x).ok()?,
                                    y: i32::try_from(y).ok()?,
                                });
                            }
                        }
                    }
                }
            }
        }
        let tile = self.tiles.get(&(step, tx, ty))?;
        let covered: BTreeSet<_> = tile.line.union(&tile.water).copied().collect();
        let mut banks = BTreeMap::new();
        for pixel in candidates {
            if covered.contains(&pixel) {
                continue;
            }
            if let Surface::Land { terrain, .. } = surface_for_pixel(&self.map, step, pixel)? {
                banks.insert(pixel, terrain);
            }
        }
        self.tiles.get_mut(&(step, tx, ty))?.banks = banks;
        self.bank_tiles.insert((step, tx, ty));
        Some(())
    }

    fn build_step_index(&self, p: i64) -> Option<BTreeMap<(i32, i32), Vec<usize>>> {
        let mut index: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
        for (stretch_index, stretch) in self.network.stretches.iter().enumerate() {
            if stretch.points.is_empty() {
                continue;
            }
            let radius = stretch.drawn_first.max(stretch.drawn_last).max(1) / 2 + 2;
            let min_x = i64::from(stretch.points.iter().map(|pt| pt.x).min()?) - radius;
            let max_x = i64::from(stretch.points.iter().map(|pt| pt.x).max()?) + radius;
            let min_y = i64::from(stretch.points.iter().map(|pt| pt.y).min()?) - radius;
            let max_y = i64::from(stretch.points.iter().map(|pt| pt.y).max()?) + radius;
            let tx0 = (min_x.div_euclid(p)).div_euclid(128);
            let tx1 = (max_x.div_euclid(p)).div_euclid(128);
            let ty0 = (min_y.div_euclid(p)).div_euclid(128);
            let ty1 = (max_y.div_euclid(p)).div_euclid(128);
            for ty in ty0..=ty1 {
                for tx in tx0..=tx1 {
                    index
                        .entry((i32::try_from(tx).ok()?, i32::try_from(ty).ok()?))
                        .or_default()
                        .push(stretch_index);
                }
            }
        }
        for entries in index.values_mut() {
            entries.sort_unstable();
            entries.dedup();
        }
        Some(index)
    }

    #[expect(
        clippy::too_many_lines,
        clippy::many_single_char_names,
        reason = "keep the normative integer distance cases beside tile clipping and land masking"
    )]
    fn prepare_tile(
        &self,
        step: u8,
        p: i64,
        tx: i32,
        ty: i32,
        candidates: &[usize],
    ) -> Option<RiverCoverageParts> {
        let mut result = RiverCoverageParts::default();
        let tile_left = i64::from(tx).checked_mul(128)?;
        let tile_top = i64::from(ty).checked_mul(128)?;
        let tile_right = tile_left + 128;
        let tile_bottom = tile_top + 128;
        for &stretch_index in candidates {
            let stretch = self.network.stretches.get(stretch_index)?;
            let tier = self.network.rivers.get(stretch.river)?.tier;
            if step < self.network.show_from_step[usize::from(tier - 1)] {
                continue;
            }
            let mut positions = vec![0_i64];
            for pair in stretch.points.windows(2) {
                let length = (i64::from(pair[1].x) - i64::from(pair[0].x)).abs()
                    + (i64::from(pair[1].y) - i64::from(pair[0].y)).abs();
                positions.push(positions.last()?.checked_add(length)?);
            }
            let total = *positions.last()?;
            if total <= 0 {
                continue;
            }
            let first = stretch.drawn_first;
            let last = stretch.drawn_last;
            let width_at = |m: i64| river_width_at(first, last, m, total);
            let threshold = (3 * p + 1) / 2;
            let turn = if last < threshold {
                None
            } else if first >= threshold {
                Some(0)
            } else {
                Some(((threshold - first) * total + (last - first) - 1) / (last - first))
            };
            let max_width = last.max(first).max(1);
            let mut narrow = BTreeSet::new();
            let mut wide = BTreeSet::new();
            if let Some(turn) = turn {
                for (i, pair) in stretch.points.windows(2).enumerate() {
                    let a = pair[0];
                    let b = pair[1];
                    let ma = positions[i];
                    let mb = positions[i + 1];
                    if ma >= turn {
                        continue;
                    }
                    let end = if mb > turn {
                        let dx = i64::from(b.x) - i64::from(a.x);
                        let dy = i64::from(b.y) - i64::from(a.y);
                        let den = mb - ma;
                        FixedPoint {
                            x: i32::try_from(
                                i128::from(a.x)
                                    + (i128::from(dx) * i128::from(turn - ma))
                                        .div_euclid(i128::from(den)),
                            )
                            .ok()?,
                            y: i32::try_from(
                                i128::from(a.y)
                                    + (i128::from(dy) * i128::from(turn - ma))
                                        .div_euclid(i128::from(den)),
                            )
                            .ok()?,
                        }
                    } else {
                        b
                    };
                    bresenham(fixed_to_pixel(a, p), fixed_to_pixel(end, p), &mut narrow)?;
                }
            } else {
                for pair in stretch.points.windows(2) {
                    bresenham(
                        fixed_to_pixel(pair[0], p),
                        fixed_to_pixel(pair[1], p),
                        &mut narrow,
                    )?;
                }
            }
            for (i, pair) in stretch.points.windows(2).enumerate() {
                let a = pair[0];
                let b = pair[1];
                let ax = i64::from(a.x);
                let ay = i64::from(a.y);
                let bx = i64::from(b.x);
                let by = i64::from(b.y);
                let radius = max_width / 2 + 2;
                let min_x = (ax.min(bx) - radius).div_euclid(p).max(tile_left);
                let max_x = (ax.max(bx) + radius).div_euclid(p).min(tile_right - 1);
                let min_y = (ay.min(by) - radius).div_euclid(p).max(tile_top);
                let max_y = (ay.max(by) + radius).div_euclid(p).min(tile_bottom - 1);
                if min_x > max_x || min_y > max_y {
                    continue;
                }
                let dx = bx - ax;
                let dy = by - ay;
                let len = i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy);
                let ma = positions[i];
                let mb = positions[i + 1];
                for y in min_y..=max_y {
                    for x in min_x..=max_x {
                        let pixel = WorldPixel {
                            x: i32::try_from(x).ok()?,
                            y: i32::try_from(y).ok()?,
                        };
                        if !matches!(
                            surface_for_pixel(&self.map, step, pixel)?,
                            Surface::Land { .. }
                        ) {
                            continue;
                        }
                        let c = sample_point(pixel, step)?;
                        let acx = i64::from(c.x) - ax;
                        let acy = i64::from(c.y) - ay;
                        let d = i128::from(acx) * i128::from(dx) + i128::from(acy) * i128::from(dy);
                        let Some(turn) = turn else {
                            continue;
                        };
                        let (m, inside) = if d <= 0 {
                            (
                                ma,
                                i == 0
                                    && turn == 0
                                    && 4 * (i128::from(acx) * i128::from(acx)
                                        + i128::from(acy) * i128::from(acy))
                                        <= i128::from(width_at(ma)?) * i128::from(width_at(ma)?),
                            )
                        } else if d >= len {
                            (
                                mb,
                                mb >= turn
                                    && 4 * (i128::from(i64::from(c.x) - bx).pow(2)
                                        + i128::from(i64::from(c.y) - by).pow(2))
                                        <= i128::from(width_at(mb)?).pow(2),
                            )
                        } else {
                            let m = ma + i64::try_from(i128::from(mb - ma) * d / len).ok()?;
                            let cross =
                                i128::from(acx) * i128::from(dy) - i128::from(acy) * i128::from(dx);
                            (
                                m,
                                m >= turn
                                    && 4 * cross * cross <= i128::from(width_at(m)?).pow(2) * len,
                            )
                        };
                        if inside && m >= turn {
                            wide.insert(pixel);
                        }
                    }
                }
            }
            for pixel in narrow {
                if tile_has(pixel, tile_left, tile_top)
                    && !wide.contains(&pixel)
                    && matches!(
                        surface_for_pixel(&self.map, step, pixel)?,
                        Surface::Land { .. }
                    )
                {
                    result.line.insert(pixel);
                }
            }
            result.water.extend(wide);
        }
        // Water from any stretch masks every narrow line offered in the tile.
        result.line.retain(|pixel| !result.water.contains(pixel));
        Some(result)
    }
}

fn tile_has(pixel: WorldPixel, left: i64, top: i64) -> bool {
    i64::from(pixel.x) >= left
        && i64::from(pixel.x) < left + 128
        && i64::from(pixel.y) >= top
        && i64::from(pixel.y) < top + 128
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
mod look_tests;
#[cfg(test)]
mod mode_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod loading_tests;
