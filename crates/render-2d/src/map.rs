//! The map lane (milestone 6, slices 1 and 6): the inputs the application publishes beside the scene so that the GPU owners can write
//! the map image, and where that image lands in the window.
//!
//! Two immutable inputs, both plain data (no GPU object, no `ck-map` type):
//!
//! - [`MapInput`], published once per loaded map: the crop's global origin cell and size, the two packed grids, the rule table
//!   ([`MapRules`]) and a generation number;
//! - [`MapCamera`]: the zoom step, the top-left world pixel, the view size in map pixels, the river pixels (empty until slice 8) and the
//!   metrics generation it was computed for.
//!
//! # The packed grids
//!
//! Both grids are RGBA8 images of the crop's dimensions: `size.0 * size.1 * 4` bytes, rows running south (row 0 is the crop's
//! northernmost row, global cell row `j0`), columns running east (column 0 is global cell column `i0`), and **alpha always 255**.
//! A GPU owner reads them with exact integer reads and never filters them.
//!
//! | Grid | R | G | B |
//! | --- | --- | --- | --- |
//! | A | the terrain index (an index into the palette) | bit 0: the cell is usable; every other bit zero | zero |
//! | B | the high byte of `elevation + 11000` | its low byte | zero |
//!
//! `elevation + 11000` is an unsigned 16-bit value in metres, so a sea depth down to 11 km fits. Slice 3 (the loader) produces this layout
//! and cites this section; slice 6 reads grid B.
//!
//! # The rule table and the colour rules (slice 6)
//!
//! [`MapRules`] carries, **for each terrain index** (grid A's red byte): its kind ([`MapTerrainKind`]: land, sea, lake or mountain —
//! the table's sea, lake and mountain flags), the solid, second, dark and dark-second colours, the relief range and mix percentage, the
//! cap and high-ground colours and the cap heights. It also carries the sea, shallow-sea, lake and river colours, the shallow-sea
//! threshold and the 76-entry step table (`ck-map`'s `STEP_SIZE`, which the GPU reads from here and not from a copy of its own).
//! Elevations are metres, from -11000 to 54535 (so `elevation + 11000` is the 16-bit value of grid B); every colour is opaque.
//!
//! Map pixel `(x, y)` of the view at zoom step `s` is world pixel `(I_left + x, J_top + y)`. Its colour is the one `ck-map`'s
//! reference colour function gives (`docs/map-design.md` section 9.4, rules 1 to 6 and 8, every one in integer arithmetic): the
//! sample point of the pixel's centre, moved by the frozen sub-cell jitter, the cell and elevation under it, the water, usable and
//! unusable tones with their Bayer dither, the snow caps and high ground, and the four-neighbour coast outline. A pixel outside the
//! crop is sea. The camera's river list is carried and not drawn (rule 7 is slice 8's).
//!
//! The crop must lie in the non-negative projected world and leave room for a map pixel and the sub-cell detail past its far edge
//! (`(origin + size) * 32768` at most `i32::MAX - 2^20`), and the camera's sample points must fit 32 bits; both are checked.
//!
//! # Where the map image lands
//!
//! The view is `view.0 x view.1` map pixels; the enlarged map is **one quad of `(view x 4)` window points** at the window's top-left,
//! placed by the points-to-pixels rule the legacy quad uses (each edge lands at the rounded product of its position in points and the
//! axis scale). A partial map pixel at the right or bottom edge is cropped, never stretched ([`place_map`]).
//!
//! # Map modes (milestone 9, slice 8; `docs/economy-design.md` section 3.22)
//!
//! A map may carry an **owner grid** ([`MapInput::with_owners`]): one `u16` per crop cell, row-major, the index of the place that owns the
//! cell or [`MODE_UNOWNED`] (`0xFFFF`) for none, uploaded once per loaded map with its [`MapInput`]. A **mode** ([`MapMode`]) is a table
//! of one `u16` per place: a position on the colour ramp from 0 through [`MODE_VALUE_MAX`] (1000), or [`MODE_UNCOLOURED`] (`0xFFFF`) for a
//! place with nothing to show; red (0) and uncoloured never collide. It is published with a revision, as an immutable value, beside the
//! camera ([`SceneBridge::publish_mode`](crate::SceneBridge::publish_mode)); a mode with no table is **off**, and the frame is then the
//! frame without a mode, byte for byte. `ck-map`'s reference rule (`mode` module) gives the colours: each cell shows the floored mean of
//! the values of the counted cells of its 5 x 5 box, from red through yellow to green, over the terrain and under the rivers and the
//! locations.
//!
//! **Validation at this lane.** The owner grid's length is the crop's `width x height`, each index is below the place count or
//! `0xFFFF`, and the place count is below `0xFFFF`; a mode's table length is the place count and each value is at most 1000 or `0xFFFF`.
//! A violation is refused with a named [`MapError`]; the lane keeps what it accepted before.
//!
//! # Publication
//!
//! [`SceneBridge::publish_map`](crate::SceneBridge::publish_map) and
//! [`SceneBridge::publish_camera`](crate::SceneBridge::publish_camera) follow the scene lane's rules: under the bridge's lock, in generation
//! order, nothing freed under the lock, a stale input never overwrites a newer one, nothing published to a detached bridge, and with no map
//! input the snapshot carries no map, so the frame is exactly the frame without a map.

use std::sync::Arc;

use crate::quad::{Metrics, PixelRect, SourceRect, drawable, edges, trimmed};

/// The most terrain types a rule table holds: grid A's terrain index is a byte.
pub const TERRAIN_MAX: usize = 256;

/// The number of zoom steps (0 through 75) and so of entries in the step table.
pub const STEP_COUNT: usize = 76;

/// Fixed-point units in one cell.
const FIXED_PER_CELL: i64 = 32_768;

/// What the far edge of the crop must leave below `i32::MAX`: room for one map pixel and the sub-cell detail past the edge.
const CROP_MARGIN: i64 = 1 << 20;

/// The lowest and highest elevation in metres a rule can name (`elevation + 11000` is a 16-bit value).
const ELEVATION_RANGE: std::ops::RangeInclusive<i32> = -11_000..=54_535;

/// The longest side of a crop or a view: a size every GPU image supports.
pub const SIDE_MAX: u32 = 16_384;

/// How many window points one map pixel covers on each axis.
pub const POINTS_PER_PIXEL: u32 = 4;

/// An owner-grid entry for a cell no place owns.
pub const MODE_UNOWNED: u16 = 0xFFFF;

/// A mode value for a place that is not coloured (no capacity, say).
pub const MODE_UNCOLOURED: u16 = 0xFFFF;

/// The largest mode value: green.
pub const MODE_VALUE_MAX: u16 = 1000;

/// The width, in texels, of the image a GPU owner makes of a mode table: place `i` is texel `(i % MODE_TABLE_WIDTH, i / MODE_TABLE_WIDTH)`.
pub const MODE_TABLE_WIDTH: u32 = 256;

/// Why a map input or a camera was refused: a named, first problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapError {
    /// A crop or a view with a zero or a too long side.
    Size {
        /// What was sized: `crop` or `view`.
        what: &'static str,
        /// The width.
        width: u32,
        /// The height.
        height: u32,
    },
    /// A grid whose byte length is not `width * height * 4`.
    GridLength {
        /// `A` or `B`.
        grid: char,
        /// The length the crop needs.
        expected: usize,
        /// The length given.
        actual: usize,
    },
    /// A grid cell that breaks the layout: alpha not 255, or a byte that must be zero.
    GridCell {
        /// `A` or `B`.
        grid: char,
        /// The cell's column within the crop.
        x: u32,
        /// The cell's row within the crop.
        y: u32,
        /// What is wrong with it.
        problem: &'static str,
    },
    /// A rule table that breaks its layout: the terrain index it concerns (`None` for the table as a whole) and the problem. Colours
    /// must be opaque (the map target is opaque, and the enlarged map is blended as premultiplied).
    Rules {
        /// The terrain index the problem concerns, or `None` for the table as a whole (its length, the water colours, the step table).
        terrain: Option<usize>,
        /// What is wrong.
        problem: &'static str,
    },
    /// A crop outside the usable world: origin not in the non-negative projected world, or no room past the far edge for a map pixel.
    Crop {
        /// The crop's global origin cell.
        origin: [i32; 2],
        /// Its size in cells.
        size: [u32; 2],
    },
    /// A terrain index that the rule table does not have.
    TerrainIndex {
        /// The cell's column within the crop.
        x: u32,
        /// The cell's row within the crop.
        y: u32,
        /// The index.
        index: u8,
        /// The rule table's length.
        terrains: usize,
    },
    /// A zoom step past the last one.
    Step {
        /// The step.
        step: u8,
    },
    /// A river pixel outside the view.
    RiverOutsideView {
        /// Its position in the list.
        position: usize,
        /// Its column.
        x: u32,
        /// Its row.
        y: u32,
    },
    /// More river pixels than the view has pixels.
    TooManyRivers {
        /// The limit: the view's pixel count.
        limit: u64,
        /// How many were given.
        count: usize,
    },
    /// A location pixel outside the view.
    LocationOutsideView {
        /// Its position in the list.
        position: usize,
        /// Its column.
        x: u32,
        /// Its row.
        y: u32,
    },
    /// More location pixels than the view can contain.
    TooManyLocations {
        /// The view's pixel count.
        limit: u64,
        /// How many were given.
        count: usize,
    },
    /// A location references a palette entry that does not exist.
    LocationPaletteIndex {
        /// Its position in the list.
        position: usize,
        /// The palette index supplied.
        index: u32,
        /// Number of available colours.
        colours: usize,
    },
    /// An owner grid whose length is not the crop's `width x height`.
    OwnerGridLength {
        /// The length the crop needs.
        expected: usize,
        /// The length given.
        actual: usize,
    },
    /// An owner index that is neither below the place count nor `0xFFFF`.
    OwnerIndex {
        /// The cell's column within the crop.
        x: u32,
        /// The cell's row within the crop.
        y: u32,
        /// The index.
        index: u16,
        /// The place count.
        places: usize,
    },
    /// A place count that cannot be indexed without meeting the `0xFFFF` marker.
    OwnerPlaces {
        /// The place count.
        places: usize,
    },
    /// A mode table whose length is not the place count.
    ModeTableLength {
        /// The place count.
        expected: usize,
        /// The table's length.
        actual: usize,
    },
    /// A mode value that is neither at most 1000 nor `0xFFFF`.
    ModeValue {
        /// The place the value belongs to.
        place: usize,
        /// The value.
        value: u16,
    },
    /// A mode table for a map with no owner grid.
    ModeWithoutOwners,
}

impl std::fmt::Display for MapError {
    #[expect(clippy::too_many_lines, reason = "one arm per named refusal")]
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Size {
                what,
                width,
                height,
            } => write!(
                formatter,
                "the {what} is {width} x {height}: each side must be from 1 to {SIDE_MAX}"
            ),
            Self::GridLength {
                grid,
                expected,
                actual,
            } => write!(
                formatter,
                "grid {grid} has {actual} bytes; the crop needs {expected} (width x height x 4)"
            ),
            Self::GridCell {
                grid,
                x,
                y,
                problem,
            } => write!(formatter, "grid {grid} cell ({x}, {y}): {problem}"),
            Self::Rules {
                terrain: Some(terrain),
                problem,
            } => write!(formatter, "the rules of terrain {terrain}: {problem}"),
            Self::Rules {
                terrain: None,
                problem,
            } => write!(formatter, "the map rules: {problem}"),
            Self::Crop { origin, size } => write!(
                formatter,
                "the crop at {origin:?} of {size:?} cells must lie in the non-negative world with room for a map pixel past its far edge"
            ),
            Self::TerrainIndex {
                x,
                y,
                index,
                terrains,
            } => write!(
                formatter,
                "grid A cell ({x}, {y}) has terrain index {index}; the rules have {terrains} terrain types"
            ),
            Self::Step { step } => write!(
                formatter,
                "zoom step {step} is past the last ({})",
                STEP_COUNT - 1
            ),
            Self::RiverOutsideView { position, x, y } => write!(
                formatter,
                "river pixel {position} ({x}, {y}) is outside the view"
            ),
            Self::TooManyRivers { limit, count } => write!(
                formatter,
                "{count} river pixels; the view holds at most {limit}"
            ),
            Self::LocationOutsideView { position, x, y } => write!(
                formatter,
                "location pixel {position} ({x}, {y}) is outside the view"
            ),
            Self::TooManyLocations { limit, count } => write!(
                formatter,
                "{count} location pixels; the view holds at most {limit}"
            ),
            Self::LocationPaletteIndex {
                position,
                index,
                colours,
            } => write!(
                formatter,
                "location pixel {position} uses palette index {index}; the palette has {colours} colours"
            ),
            Self::OwnerGridLength { expected, actual } => write!(
                formatter,
                "the owner grid has {actual} entries; the crop needs {expected} (width x height)"
            ),
            Self::OwnerIndex {
                x,
                y,
                index,
                places,
            } => write!(
                formatter,
                "the owner grid cell ({x}, {y}) names place {index}; there are {places} places and {MODE_UNOWNED:#06x} means none"
            ),
            Self::OwnerPlaces { places } => write!(
                formatter,
                "{places} places cannot be indexed below the {MODE_UNOWNED:#06x} marker"
            ),
            Self::ModeTableLength { expected, actual } => write!(
                formatter,
                "the mode table has {actual} values; the map has {expected} places"
            ),
            Self::ModeValue { place, value } => write!(
                formatter,
                "the mode value of place {place} is {value}; it must be at most {MODE_VALUE_MAX} or {MODE_UNCOLOURED:#06x}"
            ),
            Self::ModeWithoutOwners => {
                formatter.write_str("a mode table needs a map with an owner grid")
            }
        }
    }
}

impl std::error::Error for MapError {}

fn side_ok(side: u32) -> bool {
    (1..=SIDE_MAX).contains(&side)
}

/// What a terrain index is: ordinary land, the sea, a lake or a mountain (the table's sea, lake and mountain flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapTerrainKind {
    /// Ordinary land: the usable and unusable tones with their dither.
    Land,
    /// The sea: the sea colour, or the shallow-sea colour where the depth is under the threshold.
    Sea,
    /// A lake: the lake colour.
    Lake,
    /// A mountain: land that also shows snow caps (usable) or high ground (unusable) by its elevation.
    Mountain,
}

/// One terrain type's rules (`docs/map-design.md` section 9.4). A water kind ignores every field but its kind; ordinary land ignores
/// the cap and high-ground colours and the cap heights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapTerrainRule {
    /// Land, sea, lake or mountain.
    pub kind: MapTerrainKind,
    /// The usable land's colour, and the coast outline's.
    pub solid: [u8; 4],
    /// The usable land's second tone.
    pub second: [u8; 4],
    /// The unusable land's dark colour.
    pub dark: [u8; 4],
    /// The unusable land's dark second tone.
    pub dark_second: [u8; 4],
    /// A usable mountain's snow cap.
    pub cap: [u8; 4],
    /// An unusable mountain's high ground.
    pub high_ground: [u8; 4],
    /// The elevations, in metres, between which the relief ramp rises from level 0 to 16 (the low one below the high one).
    pub relief: [i32; 2],
    /// How much of the relief level the usable second tone shows, in percent (0 to 100).
    pub mix_percent: u8,
    /// The elevations, in metres, between which a mountain's cap ramp rises from level 0 to 16.
    pub cap_heights: [i32; 2],
}

/// The rule table a map input carries (see the module documentation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapRules {
    /// One rule per terrain index, 1 to [`TERRAIN_MAX`].
    pub terrains: Vec<MapTerrainRule>,
    /// The sea.
    pub sea: [u8; 4],
    /// The shallow sea.
    pub shallow_sea: [u8; 4],
    /// The lakes.
    pub lake: [u8; 4],
    /// The rivers (carried; drawn from slice 8).
    pub river: [u8; 4],
    /// Opaque RGBA8 colours for the location overlay, indexed by camera pixels.
    pub overlay_palette: Vec<[u8; 4]>,
    /// A sea depth, in metres, under which the shallow sea dithers in; at least 1.
    pub shallow_threshold: u16,
    /// The map pixel size at each zoom step, in 1/32768 cell (`ck-map`'s frozen `STEP_SIZE`).
    pub steps: [i32; STEP_COUNT],
}

impl MapRules {
    /// The first problem with the table, or `Ok`.
    fn check(&self) -> Result<(), MapError> {
        let table = |problem| MapError::Rules {
            terrain: None,
            problem,
        };
        let terrain = |index, problem| MapError::Rules {
            terrain: Some(index),
            problem,
        };
        if self.terrains.is_empty() || self.terrains.len() > TERRAIN_MAX {
            return Err(table("it needs from 1 to 256 terrain types"));
        }
        if self.shallow_threshold == 0 {
            return Err(table("the shallow-sea threshold must be at least 1"));
        }
        if self.overlay_palette.len() > 256 {
            return Err(table("the overlay palette has more than 256 colours"));
        }
        if self.overlay_palette.iter().any(|colour| colour[3] != 255) {
            return Err(table("an overlay palette colour is not opaque"));
        }
        for (name, colour) in [
            ("the sea colour is not opaque", self.sea),
            ("the shallow-sea colour is not opaque", self.shallow_sea),
            ("the lake colour is not opaque", self.lake),
            ("the river colour is not opaque", self.river),
        ] {
            if colour[3] != 255 {
                return Err(table(name));
            }
        }
        if self.steps.iter().any(|size| !(1..1 << 24).contains(size)) {
            return Err(table("a step size must be from 1 to 2^24 - 1"));
        }
        for (index, rule) in self.terrains.iter().enumerate() {
            let land = matches!(rule.kind, MapTerrainKind::Land | MapTerrainKind::Mountain);
            if !land {
                continue;
            }
            if [rule.solid, rule.second, rule.dark, rule.dark_second]
                .iter()
                .any(|colour| colour[3] != 255)
            {
                return Err(terrain(index, "a land colour is not opaque"));
            }
            if rule.mix_percent > 100 {
                return Err(terrain(index, "the mix is over 100 percent"));
            }
            let ramp_ok = |range: [i32; 2]| {
                range[0] < range[1]
                    && ELEVATION_RANGE.contains(&range[0])
                    && ELEVATION_RANGE.contains(&range[1])
            };
            if !ramp_ok(rule.relief) {
                return Err(terrain(
                    index,
                    "the relief range must rise within -11000 to 54535 metres",
                ));
            }
            if rule.kind == MapTerrainKind::Mountain {
                if rule.cap[3] != 255 || rule.high_ground[3] != 255 {
                    return Err(terrain(
                        index,
                        "the cap or high-ground colour is not opaque",
                    ));
                }
                if !ramp_ok(rule.cap_heights) {
                    return Err(terrain(
                        index,
                        "the cap heights must rise within -11000 to 54535 metres",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// A loaded map as the GPU reads it: immutable, published once, replaced as a whole by a newer generation.
#[derive(Debug)]
pub struct MapInput {
    generation: u64,
    origin: [i32; 2],
    size: [u32; 2],
    grid_a: Arc<[u8]>,
    grid_b: Arc<[u8]>,
    rules: Arc<MapRules>,
    owners: Option<MapOwners>,
}

/// The owner grid of a loaded map and its place count: validated, immutable, shared.
#[derive(Debug, Clone)]
pub struct MapOwners {
    places: usize,
    indices: Arc<[u16]>,
    image: Arc<[u8]>,
}

impl MapOwners {
    /// The number of places the grid indexes.
    #[must_use]
    pub fn places(&self) -> usize {
        self.places
    }

    /// The owner of each crop cell, row-major: a place index below [`places`](Self::places) or [`MODE_UNOWNED`].
    #[must_use]
    pub fn indices(&self) -> &Arc<[u16]> {
        &self.indices
    }

    /// The grid as an RGBA8 image of the crop's size, as the GPU reads it: R the high byte of the index, G its low byte, B zero and
    /// A 255 (so an unowned cell is `255, 255, 0, 255`).
    #[must_use]
    pub fn image(&self) -> &Arc<[u8]> {
        &self.image
    }
}

impl MapInput {
    /// A map input, checked against the layout in the module documentation.
    ///
    /// `origin` is the crop's global origin cell `(i0, j0)`; `size` its width and height in cells; `grid_a` and `grid_b` the packed
    /// grids; `rules` the rule table; `generation` the number that orders inputs (a newer input has a larger one).
    ///
    /// # Errors
    ///
    /// The first [`MapError`] found: a size, a crop outside the usable world, a rule that breaks its layout, a grid length, a cell that
    /// breaks the layout, or a terrain index the rule table lacks.
    pub fn new(
        generation: u64,
        origin: [i32; 2],
        size: [u32; 2],
        grid_a: Vec<u8>,
        grid_b: Vec<u8>,
        rules: MapRules,
    ) -> Result<Self, MapError> {
        let [width, height] = size;
        if !side_ok(width) || !side_ok(height) {
            return Err(MapError::Size {
                what: "crop",
                width,
                height,
            });
        }
        let far = |axis: usize| (i64::from(origin[axis]) + i64::from(size[axis])) * FIXED_PER_CELL;
        if origin[0] < 0
            || origin[1] < 0
            || far(0) > i64::from(i32::MAX) - CROP_MARGIN
            || far(1) > i64::from(i32::MAX) - CROP_MARGIN
        {
            return Err(MapError::Crop { origin, size });
        }
        rules.check()?;
        let expected = usize::try_from(u64::from(width) * u64::from(height) * 4).map_err(|_| {
            MapError::Size {
                what: "crop",
                width,
                height,
            }
        })?;
        for (grid, bytes) in [('A', &grid_a), ('B', &grid_b)] {
            if bytes.len() != expected {
                return Err(MapError::GridLength {
                    grid,
                    expected,
                    actual: bytes.len(),
                });
            }
        }
        let cell = |position: usize| {
            let cell = u32::try_from(position / 4).unwrap_or(u32::MAX);
            (cell % width, cell / width)
        };
        for (position, pixel) in grid_a.chunks_exact(4).enumerate() {
            let problem = if pixel[3] != 255 {
                Some("alpha is not 255")
            } else if pixel[1] & 0xFE != 0 {
                Some("a bit of G other than bit 0 is set")
            } else if pixel[2] != 0 {
                Some("B is not zero")
            } else {
                None
            };
            let (x, y) = cell(position * 4);
            if let Some(problem) = problem {
                return Err(MapError::GridCell {
                    grid: 'A',
                    x,
                    y,
                    problem,
                });
            }
            if usize::from(pixel[0]) >= rules.terrains.len() {
                return Err(MapError::TerrainIndex {
                    x,
                    y,
                    index: pixel[0],
                    terrains: rules.terrains.len(),
                });
            }
        }
        for (position, pixel) in grid_b.chunks_exact(4).enumerate() {
            let problem = if pixel[3] != 255 {
                Some("alpha is not 255")
            } else if pixel[2] != 0 {
                Some("B is not zero")
            } else {
                None
            };
            if let Some(problem) = problem {
                let (x, y) = cell(position * 4);
                return Err(MapError::GridCell {
                    grid: 'B',
                    x,
                    y,
                    problem,
                });
            }
        }
        Ok(Self {
            generation,
            origin,
            size,
            grid_a: grid_a.into(),
            grid_b: grid_b.into(),
            rules: Arc::new(rules),
            owners: None,
        })
    }

    /// This input with the owner grid of its crop: `owners` has one entry per crop cell, row-major (the place index that owns the cell,
    /// or [`MODE_UNOWNED`]), and `places` is the number of places.
    ///
    /// # Errors
    ///
    /// [`MapError::OwnerPlaces`] for a place count that reaches `0xFFFF`, [`MapError::OwnerGridLength`] for a grid that is not
    /// `width x height` entries, and [`MapError::OwnerIndex`] for the first entry that is neither below `places` nor `0xFFFF`.
    pub fn with_owners(mut self, owners: Vec<u16>, places: usize) -> Result<Self, MapError> {
        if places >= usize::from(MODE_UNOWNED) {
            return Err(MapError::OwnerPlaces { places });
        }
        let [width, height] = self.size;
        let expected =
            usize::try_from(u64::from(width) * u64::from(height)).map_err(|_| MapError::Size {
                what: "crop",
                width,
                height,
            })?;
        if owners.len() != expected {
            return Err(MapError::OwnerGridLength {
                expected,
                actual: owners.len(),
            });
        }
        if let Some(position) = owners
            .iter()
            .position(|&index| index != MODE_UNOWNED && usize::from(index) >= places)
        {
            let cell = u32::try_from(position).unwrap_or(u32::MAX);
            return Err(MapError::OwnerIndex {
                x: cell % width,
                y: cell / width,
                index: owners[position],
                places,
            });
        }
        let image: Vec<u8> = owners
            .iter()
            .flat_map(|index| {
                let [high, low] = index.to_be_bytes();
                [high, low, 0, 255]
            })
            .collect();
        self.owners = Some(MapOwners {
            places,
            indices: owners.into(),
            image: image.into(),
        });
        Ok(self)
    }

    /// The owner grid, when the map has one.
    #[must_use]
    pub fn owners(&self) -> Option<&MapOwners> {
        self.owners.as_ref()
    }

    /// The generation that orders inputs.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The crop's global origin cell `(i0, j0)`.
    #[must_use]
    pub fn origin(&self) -> [i32; 2] {
        self.origin
    }

    /// The crop's width and height in cells.
    #[must_use]
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// Grid A (terrain), shared: `size.0 * size.1 * 4` bytes.
    #[must_use]
    pub fn grid_a(&self) -> &Arc<[u8]> {
        &self.grid_a
    }

    /// Grid B (elevation), shared.
    #[must_use]
    pub fn grid_b(&self) -> &Arc<[u8]> {
        &self.grid_b
    }

    /// The rule table, shared.
    #[must_use]
    pub fn rules(&self) -> &Arc<MapRules> {
        &self.rules
    }
}

/// The camera as the map pass reads it: immutable, one per change of the view.
#[derive(Debug)]
pub struct MapCamera {
    step: u8,
    top_left: [i32; 2],
    view: [u32; 2],
    rivers: Arc<[[u32; 2]]>,
    locations: Arc<[[u32; 3]]>,
    metrics_generation: u64,
}

impl MapCamera {
    /// A camera at zoom `step` (0 through 75) and world pixel `top_left` showing a `view` of map pixels, computed for the bridge's
    /// metrics of `metrics_generation` ([`SceneBridge::metrics_generation`](crate::SceneBridge::metrics_generation)).
    ///
    /// `rivers` are the visible river pixels in view coordinates; the list is empty until slice 8 and bounded by the view's pixel
    /// count.
    ///
    /// # Errors
    ///
    /// [`MapError::Step`] for a step past the last, [`MapError::Size`] for a view with a zero or too long side,
    /// [`MapError::TooManyRivers`], or [`MapError::RiverOutsideView`].
    pub fn new(
        step: u8,
        top_left: [i32; 2],
        view: [u32; 2],
        rivers: Vec<[u32; 2]>,
        metrics_generation: u64,
    ) -> Result<Self, MapError> {
        Self::new_with_locations(
            step,
            top_left,
            view,
            rivers,
            Vec::new(),
            0,
            metrics_generation,
        )
    }

    /// Construct a camera with view-relative `[x, y, palette_index]` locations.
    ///
    /// # Errors
    ///
    /// Returns [`MapError::Step`] or [`MapError::Size`] for an invalid step/view, the river errors
    /// documented on [`Self::new`], [`MapError::TooManyLocations`] when the list exceeds the view
    /// pixel count, [`MapError::LocationOutsideView`] for an out-of-view pixel, or
    /// [`MapError::LocationPaletteIndex`] for an unavailable palette index.
    pub fn new_with_locations(
        step: u8,
        top_left: [i32; 2],
        view: [u32; 2],
        rivers: Vec<[u32; 2]>,
        locations: Vec<[u32; 3]>,
        palette_colours: usize,
        metrics_generation: u64,
    ) -> Result<Self, MapError> {
        let [width, height] = view;
        if usize::from(step) >= STEP_COUNT {
            return Err(MapError::Step { step });
        }
        if !side_ok(width) || !side_ok(height) {
            return Err(MapError::Size {
                what: "view",
                width,
                height,
            });
        }
        let limit = u64::from(width) * u64::from(height);
        if rivers.len() as u64 > limit {
            return Err(MapError::TooManyRivers {
                limit,
                count: rivers.len(),
            });
        }
        if let Some((position, pixel)) = rivers
            .iter()
            .enumerate()
            .find(|(_, pixel)| pixel[0] >= width || pixel[1] >= height)
        {
            return Err(MapError::RiverOutsideView {
                position,
                x: pixel[0],
                y: pixel[1],
            });
        }
        if locations.len() as u64 > limit {
            return Err(MapError::TooManyLocations {
                limit,
                count: locations.len(),
            });
        }
        for (position, pixel) in locations.iter().enumerate() {
            if pixel[0] >= width || pixel[1] >= height {
                return Err(MapError::LocationOutsideView {
                    position,
                    x: pixel[0],
                    y: pixel[1],
                });
            }
            let index = pixel[2];
            if usize::try_from(index).unwrap_or(usize::MAX) >= palette_colours {
                return Err(MapError::LocationPaletteIndex {
                    position,
                    index,
                    colours: palette_colours,
                });
            }
        }
        Ok(Self {
            step,
            top_left,
            view,
            rivers: rivers.into(),
            locations: locations.into(),
            metrics_generation,
        })
    }

    /// The zoom step `s`, 0 through 75.
    #[must_use]
    pub fn step(&self) -> u8 {
        self.step
    }

    /// The top-left world pixel `(I_left, J_top)`.
    #[must_use]
    pub fn top_left(&self) -> [i32; 2] {
        self.top_left
    }

    /// The view's width and height in map pixels.
    #[must_use]
    pub fn view(&self) -> [u32; 2] {
        self.view
    }

    /// The visible river pixels in view coordinates (not drawn until slice 8).
    #[must_use]
    pub fn rivers(&self) -> &Arc<[[u32; 2]]> {
        &self.rivers
    }

    /// The visible location pixels in view coordinates and their palette indices.
    #[must_use]
    pub fn locations(&self) -> &Arc<[[u32; 3]]> {
        &self.locations
    }

    /// The metrics generation this camera was computed for.
    #[must_use]
    pub fn metrics_generation(&self) -> u64 {
        self.metrics_generation
    }
}

/// A map mode as the map pass reads it: immutable, one per change of the values or of the mode, ordered by its revision (a newer one has
/// a larger one). With no table it is **off**.
#[derive(Debug)]
pub struct MapMode {
    revision: u64,
    values: Option<Arc<[u16]>>,
}

impl MapMode {
    /// A mode showing `values`, one per place (a position on the ramp from 0 through 1000, or [`MODE_UNCOLOURED`]), at `revision`.
    ///
    /// # Errors
    ///
    /// [`MapError::ModeValue`] for the first value that is neither at most 1000 nor `0xFFFF`. The table's length is checked against
    /// the map's places when it is published ([`SceneBridge::publish_mode`](crate::SceneBridge::publish_mode)).
    pub fn new(revision: u64, values: Vec<u16>) -> Result<Self, MapError> {
        if let Some((place, &value)) = values
            .iter()
            .enumerate()
            .find(|&(_, &value)| value > MODE_VALUE_MAX && value != MODE_UNCOLOURED)
        {
            return Err(MapError::ModeValue { place, value });
        }
        Ok(Self {
            revision,
            values: Some(values.into()),
        })
    }

    /// The mode that shows nothing: the frame without a map mode.
    #[must_use]
    pub fn off(revision: u64) -> Self {
        Self {
            revision,
            values: None,
        }
    }

    /// The revision that orders modes.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The value of each place, or `None` when the mode is off.
    #[must_use]
    pub fn values(&self) -> Option<&Arc<[u16]>> {
        self.values.as_ref()
    }
}

/// Where the enlarged map lands: the destination in whole drawable pixels (clipped to the window) and the part of the map image drawn
/// there, in **map pixels**.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapPlacement {
    /// Where it is drawn, after clipping to the window.
    pub destination: PixelRect,
    /// The part of the `view` image drawn there, trimmed in proportion to the clipping (the partial right or bottom map pixel is
    /// cropped, never stretched).
    pub source: SourceRect,
}

/// Where the enlarged map of a `view` lands with these metrics, or `None` if nothing of it shows: one quad of `view x 4` points at
/// the window's top-left through the accepted points-to-pixels rule.
#[must_use]
pub fn place_map(view: [u32; 2], metrics: &Metrics) -> Option<MapPlacement> {
    let [view_width, view_height] = view;
    let (left, right) = edges(
        0.0,
        f64::from(view_width) * f64::from(POINTS_PER_PIXEL),
        metrics.points_width,
        metrics.pixels.width,
    );
    let (top, bottom) = edges(
        0.0,
        f64::from(view_height) * f64::from(POINTS_PER_PIXEL),
        metrics.points_height,
        metrics.pixels.height,
    );
    if right == left || bottom == top {
        return None;
    }
    let window_width = i32::try_from(metrics.pixels.width).unwrap_or(i32::MAX);
    let window_height = i32::try_from(metrics.pixels.height).unwrap_or(i32::MAX);
    let (visible_left, visible_right) = (left.max(0), right.min(window_width));
    let (visible_top, visible_bottom) = (top.max(0), bottom.min(window_height));
    if visible_left >= visible_right || visible_top >= visible_bottom {
        return None;
    }
    let (source_left, source_right) = trimmed(view_width, left, right, visible_left, visible_right);
    let (source_top, source_bottom) =
        trimmed(view_height, top, bottom, visible_top, visible_bottom);
    let source = SourceRect {
        left: source_left,
        top: source_top,
        right: source_right,
        bottom: source_bottom,
    };
    if !drawable(&source, view_width, view_height) {
        return None;
    }
    Some(MapPlacement {
        destination: PixelRect {
            left: visible_left,
            top: visible_top,
            right: visible_right,
            bottom: visible_bottom,
        },
        source,
    })
}

/// What a snapshot carries for the map: the input, the camera that was computed for the snapshot's own metrics, and where the
/// enlarged map lands. Present only when all three exist; otherwise the snapshot has no map and the frame is the frame without one.
#[derive(Debug, Clone)]
pub struct MapFrame {
    /// The loaded map.
    pub input: Arc<MapInput>,
    /// The camera, computed for the metrics of the snapshot that carries it.
    pub camera: Arc<MapCamera>,
    /// Where the enlarged map lands, or `None` when nothing of it shows (a zero-size window, a view clipped away).
    pub placement: Option<MapPlacement>,
    /// The map mode showing, or `None` for the plain map: only a mode with a table that fits the input's owner grid is carried.
    pub mode: Option<Arc<MapMode>>,
    /// Where a backend's presenter says it refused to make a map image, and that it later could
    /// ([`SceneBridge::take_map_refusal`](crate::SceneBridge::take_map_refusal) reads it).
    pub refusals: Arc<MapRefusals>,
}

/// The publisher's view of the presenter's refusals to make a map image (milestone 6, slice 1): not a failure of the run. A native
/// allocation of the map's grid images or target can be refused (out of device memory, say); the presenter then draws the frame
/// without the map, never with a stale target, and tries again on its next frame, one attempt per frame.
///
/// A refusal is reported **once per episode**: the first refusal of a run of refusals is kept for
/// [`SceneBridge::take_map_refusal`](crate::SceneBridge::take_map_refusal), the others in the same episode are not, and an attempt
/// that succeeds ends the episode (and drops a message nobody read), so the next refusal is a new episode and a new message.
#[derive(Debug, Default)]
pub struct MapRefusals {
    state: std::sync::Mutex<RefusalEpisode>,
}

#[derive(Debug, Default)]
struct RefusalEpisode {
    /// A refusal is in progress: the presenter's attempts keep failing.
    open: bool,
    /// The episode's message, until it is taken.
    message: Option<String>,
}

impl MapRefusals {
    fn state(&self) -> std::sync::MutexGuard<'_, RefusalEpisode> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The presenter's attempt was refused. The first of an episode keeps its message; the rest of the episode keep nothing.
    pub fn refused(&self, message: &str) {
        let mut state = self.state();
        if !state.open {
            state.open = true;
            state.message = Some(message.to_owned());
        }
    }

    /// The presenter's attempt succeeded: the episode is over.
    pub fn cleared(&self) {
        let mut state = self.state();
        state.open = false;
        state.message = None;
    }

    /// The episode's message, once.
    pub fn take(&self) -> Option<String> {
        self.state().message.take()
    }
}

/// What a map-lane publication did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapPublication {
    /// Accepted: it replaced what the lane held and, with a sink attached, went to it.
    Published,
    /// Refused: an input that is not newer than the lane's, or a camera computed for other metrics than the bridge's.
    Stale,
    /// Refused: the bridge was detached.
    Detached,
}
