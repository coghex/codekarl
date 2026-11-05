//! The GPU-written map image (milestone 6, slices 1 and 6): plain data the renderer hands the presenter. The drawing is
//! [`gpu_map`](crate::gpu_map)'s, and the bookkeeping of what is resident [`map_residency`](crate::map_residency)'s.
//!
//! A frame may carry a [`VulkanMap`]: a loaded map ([`VulkanMapInput`]: two packed RGBA8 grids of the crop's size and the rule table
//! ([`VulkanMapRules`])), the camera's zoom step, top-left world pixel and view size, and where the enlarged map lands. The presenter
//! then:
//!
//! 1. makes the input resident when its generation is new (three `R8G8B8A8_UNORM` images read only by exact integer reads
//!    (`texelFetch`): grid A, grid B and the rules as one [`RULES_WIDTH`] x [`RULES_HEIGHT`] image), uploading them in the frame's
//!    command buffer and retiring the images they replace under the ledger's proof of the last submit that used them;
//! 2. makes a map target resident when the view's size is new (an `R8G8B8A8_UNORM` image the size of the view, a colour attachment and
//!    a sampled image), retiring the old one the same way;
//! 3. records the **map pass** in the frame's command buffer, before the window pass: one triangle over the target with the full
//!    colour rules of `docs/map-design.md` section 9.4 (rules 1 to 6 and 8) at the camera's step, in integer arithmetic, exactly as the
//!    reference colour function of `ck-map` computes them (`map.frag`'s own comment gives the layout of the rules image);
//! 4. records the enlarged map in the window pass between the clear and the legacy quad: the target, nearest-sampled, at the frame's
//!    destination, by the legacy quad's own pipeline (so on a surface format the quad does not support, the map quad is skipped with
//!    that rule's single notice).
//!
//! **Ordering** is explicit, in the same command buffer and with no CPU wait: one image barrier before the map pass orders the previous
//! submission's shader reads of the target before this pass's colour writes (the target's old contents are discarded: the pass writes
//! every pixel), and one between the passes orders the colour writes before the window pass's shader reads, with the layout change to
//! shader-read-only. A frame with a zero-size drawable records no map pass and keeps the resident target for the next visible frame.
//!
//! The grid layout is documented in `ck-render-2d` (`map` module).

use std::sync::Arc;

use crate::quad::{DestinationRect, SourceRect};

/// The most terrain types a rule table holds: grid A's terrain index is a byte.
pub const TERRAIN_MAX: usize = 256;
/// The number of zoom steps (0 through 75) and so of entries in the step table.
pub const STEP_COUNT: usize = 76;
/// The longest side of a crop or a view, in cells or map pixels.
pub const MAP_SIDE_MAX: u32 = 16_384;
/// The rules image's width in texels (`map.frag` documents the layout).
pub const RULES_WIDTH: u32 = 80;
/// The rules image's height in texels: one row per terrain index, the globals row and the step-table row.
pub const RULES_HEIGHT: u32 = 258;
/// Fixed-point units in one cell.
const FIXED_PER_CELL: i64 = 32_768;
/// What the largest crop edge may leave below `i32::MAX`: room for one map pixel and the sub-cell detail past the edge.
const CROP_MARGIN: i64 = 1 << 20;
/// The offset that makes an elevation in metres an unsigned 16-bit value in the files and the rules image.
const ELEVATION_OFFSET: i32 = 11_000;

/// Why a map was refused before it reached the GPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VulkanMapError {
    /// A crop or a view with a zero or too long side.
    Size,
    /// A grid whose length is not `width * height * 4`.
    GridLength {
        /// The length the crop needs.
        expected: usize,
        /// The length given.
        actual: usize,
    },
    /// A rule table that cannot be packed: the reason names the entry.
    Rules(String),
    /// A crop that does not lie in the non-negative projected world, or that leaves no room for a map pixel past its edge.
    Crop,
    /// A zoom step past the last one.
    Step(u8),
    /// A camera whose sample points do not fit the 32-bit fixed-point world.
    Camera,
    /// A destination with no area.
    Destination,
    /// A source that is not finite, not inside the view, or empty.
    Source,
}

impl std::fmt::Display for VulkanMapError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Size => write!(
                formatter,
                "a map side must be from 1 to {MAP_SIDE_MAX} cells or pixels"
            ),
            Self::GridLength { expected, actual } => write!(
                formatter,
                "a map grid has {actual} bytes; the crop needs {expected}"
            ),
            Self::Rules(reason) => write!(formatter, "the map rules are unusable: {reason}"),
            Self::Crop => formatter.write_str(
                "the map crop must lie in the non-negative world with room for a map pixel past its far edge",
            ),
            Self::Step(step) => write!(
                formatter,
                "zoom step {step} is past the last ({})",
                STEP_COUNT - 1
            ),
            Self::Camera => formatter.write_str("the camera's sample points do not fit the 32-bit fixed-point world"),
            Self::Destination => formatter.write_str("the map's destination has no area"),
            Self::Source => {
                formatter.write_str("the map's source is not a non-empty part of the view")
            }
        }
    }
}

impl std::error::Error for VulkanMapError {}

fn side_ok(side: u32) -> bool {
    (1..=MAP_SIDE_MAX).contains(&side)
}

/// What a terrain index is: the sea, a lake, ordinary land or a mountain (the rule table's sea, lake and mountain flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VulkanTerrainKind {
    /// Ordinary land.
    Land,
    /// The sea: drawn in the sea colour, or the shallow-sea colour where the depth is under the threshold.
    Sea,
    /// A lake: drawn in the lake colour.
    Lake,
    /// A mountain: land that also shows snow caps (usable) or high ground (unusable).
    Mountain,
}

/// One terrain type's rules (`docs/map-design.md` section 9.4). A water kind ignores every other field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VulkanTerrainRule {
    /// Water, ordinary land or a mountain.
    pub kind: VulkanTerrainKind,
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
    /// The elevations, in metres, between which the relief ramp rises from level 0 to 16.
    pub relief: [i32; 2],
    /// How much of the relief level the usable second tone shows, in percent (0 to 100).
    pub mix_percent: u8,
    /// The elevations, in metres, between which a mountain's cap ramp rises from level 0 to 16.
    pub cap_heights: [i32; 2],
}

/// The rule table a map input carries: one rule per terrain index (grid A's red byte), the water colours, the shallow-sea threshold,
/// the river colour (carried; rivers are drawn from slice 8) and the step table. The GPU uses this table and no copy of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VulkanMapRules {
    /// One rule per terrain index, 1 to [`TERRAIN_MAX`].
    pub terrains: Vec<VulkanTerrainRule>,
    /// The sea.
    pub sea: [u8; 4],
    /// The shallow sea.
    pub shallow_sea: [u8; 4],
    /// The lakes.
    pub lake: [u8; 4],
    /// The rivers.
    pub river: [u8; 4],
    /// A sea depth, in metres, under which the shallow sea dithers in; at least 1.
    pub shallow_threshold: u16,
    /// The map pixel size at each zoom step, in 1/32768 cell (`ck-map`'s frozen table).
    pub steps: [i32; STEP_COUNT],
}

fn rules_error(reason: impl Into<String>) -> VulkanMapError {
    VulkanMapError::Rules(reason.into())
}

fn opaque(colour: [u8; 4], what: &str, row: Option<usize>) -> Result<[u8; 4], VulkanMapError> {
    if colour[3] == 255 {
        return Ok(colour);
    }
    Err(rules_error(match row {
        Some(row) => format!("terrain {row}: the {what} colour is not opaque"),
        None => format!("the {what} colour is not opaque"),
    }))
}

/// A height range as two encoded 16-bit values (`metres + 11000`), high byte first.
fn height_pair(heights: [i32; 2], what: &str, row: usize) -> Result<[u8; 4], VulkanMapError> {
    let encode = |metres: i32| {
        u16::try_from(metres.checked_add(ELEVATION_OFFSET).unwrap_or(-1))
            .map_err(|_| rules_error(format!("terrain {row}: a {what} height is out of range")))
    };
    if heights[1] <= heights[0] {
        return Err(rules_error(format!(
            "terrain {row}: the {what} range must rise"
        )));
    }
    let (low, high) = (
        encode(heights[0])?.to_be_bytes(),
        encode(heights[1])?.to_be_bytes(),
    );
    Ok([low[0], low[1], high[0], high[1]])
}

impl VulkanMapRules {
    /// The rules as the rules image the shader reads (`RULES_WIDTH x RULES_HEIGHT` RGBA8 texels, rows top first).
    fn image(&self) -> Result<Vec<u8>, VulkanMapError> {
        if self.terrains.is_empty() || self.terrains.len() > TERRAIN_MAX {
            return Err(rules_error(format!(
                "{} terrain types; it needs from 1 to {TERRAIN_MAX}",
                self.terrains.len()
            )));
        }
        if self.shallow_threshold == 0 {
            return Err(rules_error("the shallow-sea threshold must be at least 1"));
        }
        let mut image = vec![0_u8; (RULES_WIDTH * RULES_HEIGHT * 4) as usize];
        let mut put = |column: usize, row: usize, texel: [u8; 4]| {
            let at = (row * RULES_WIDTH as usize + column) * 4;
            image[at..at + 4].copy_from_slice(&texel);
        };
        for (row, terrain) in self.terrains.iter().enumerate() {
            let land = matches!(
                terrain.kind,
                VulkanTerrainKind::Land | VulkanTerrainKind::Mountain
            );
            let kind = match terrain.kind {
                VulkanTerrainKind::Land => 0,
                VulkanTerrainKind::Sea => 1,
                VulkanTerrainKind::Lake => 2,
                VulkanTerrainKind::Mountain => 3,
            };
            if land {
                if terrain.mix_percent > 100 {
                    return Err(rules_error(format!(
                        "terrain {row}: the mix is {} percent",
                        terrain.mix_percent
                    )));
                }
                for (column, what, colour) in [
                    (0, "solid", terrain.solid),
                    (1, "second", terrain.second),
                    (2, "dark", terrain.dark),
                    (3, "dark second", terrain.dark_second),
                ] {
                    put(column, row, opaque(colour, what, Some(row))?);
                }
                put(6, row, height_pair(terrain.relief, "relief", row)?);
                put(8, row, [terrain.mix_percent, kind, 0, 255]);
                if terrain.kind == VulkanTerrainKind::Mountain {
                    put(4, row, opaque(terrain.cap, "cap", Some(row))?);
                    put(
                        5,
                        row,
                        opaque(terrain.high_ground, "high ground", Some(row))?,
                    );
                    put(7, row, height_pair(terrain.cap_heights, "cap", row)?);
                }
            } else {
                put(8, row, [0, kind, 0, 255]);
            }
        }
        for (column, what, colour) in [
            (0, "sea", self.sea),
            (1, "shallow sea", self.shallow_sea),
            (2, "lake", self.lake),
            (3, "river", self.river),
        ] {
            put(column, TERRAIN_MAX, opaque(colour, what, None)?);
        }
        let threshold = self.shallow_threshold.to_be_bytes();
        put(4, TERRAIN_MAX, [threshold[0], threshold[1], 0, 255]);
        for (step, &size) in self.steps.iter().enumerate() {
            if !(1..1 << 24).contains(&size) {
                return Err(rules_error(format!(
                    "step {step}: the size {size} is out of range"
                )));
            }
            let bytes = size.to_be_bytes();
            put(step, TERRAIN_MAX + 1, [bytes[1], bytes[2], bytes[3], 255]);
        }
        Ok(image)
    }
}

/// A loaded map as the presenter reads it: immutable, shared, identified by its generation.
#[derive(Debug)]
pub struct VulkanMapInput {
    generation: u64,
    origin: [i32; 2],
    size: [u32; 2],
    grid_a: Arc<[u8]>,
    grid_b: Arc<[u8]>,
    rules: Arc<[u8]>,
    steps: [i32; STEP_COUNT],
}

impl VulkanMapInput {
    /// An input of `generation`: the crop's global origin cell, its size in cells, the two RGBA8 grids (`size.0 * size.1 * 4` bytes
    /// each) and the rule table. The renderer validates the layout; the presenter checks only what it needs to size and pack its
    /// images, and that the shader's 32-bit fixed-point arithmetic holds.
    ///
    /// # Errors
    ///
    /// A [`VulkanMapError`] for a size, a grid length, a crop outside the usable world, or a rule table that cannot be packed.
    pub fn new(
        generation: u64,
        origin: [i32; 2],
        size: [u32; 2],
        grid_a: Arc<[u8]>,
        grid_b: Arc<[u8]>,
        rules: &VulkanMapRules,
    ) -> Result<Self, VulkanMapError> {
        if !side_ok(size[0]) || !side_ok(size[1]) {
            return Err(VulkanMapError::Size);
        }
        let far = |axis: usize| (i64::from(origin[axis]) + i64::from(size[axis])) * FIXED_PER_CELL;
        if origin[0] < 0
            || origin[1] < 0
            || far(0) > i64::from(i32::MAX) - CROP_MARGIN
            || far(1) > i64::from(i32::MAX) - CROP_MARGIN
        {
            return Err(VulkanMapError::Crop);
        }
        let image = rules.image()?;
        let expected = usize::try_from(u64::from(size[0]) * u64::from(size[1]) * 4)
            .map_err(|_| VulkanMapError::Size)?;
        for grid in [&grid_a, &grid_b] {
            if grid.len() != expected {
                return Err(VulkanMapError::GridLength {
                    expected,
                    actual: grid.len(),
                });
            }
        }
        Ok(Self {
            generation,
            origin,
            size,
            grid_a,
            grid_b,
            rules: image.into(),
            steps: rules.steps,
        })
    }

    /// The generation that identifies the input: the presenter uploads a generation once.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The crop's global origin cell.
    pub(crate) fn origin(&self) -> [i32; 2] {
        self.origin
    }

    /// The crop's size in cells.
    pub(crate) fn size(&self) -> [u32; 2] {
        self.size
    }

    pub(crate) fn grid_a(&self) -> &[u8] {
        &self.grid_a
    }

    pub(crate) fn grid_b(&self) -> &[u8] {
        &self.grid_b
    }

    /// The rules image: `RULES_WIDTH x RULES_HEIGHT` RGBA8 texels, rows top first.
    pub(crate) fn rules_image(&self) -> &[u8] {
        &self.rules
    }
}

/// What an off-screen map frame drew ([`readback_map`](crate::readback_map)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapReadback {
    /// The window pass: `width x height` BGRA8 pixels, rows top first.
    pub window: Vec<u8>,
    /// The map target the map pass wrote: `view.0 x view.1` RGBA8 pixels, rows top first. `None` when the frame had no map.
    pub target: Option<Vec<u8>>,
}

/// Where the presenter says it refused to make a map image, and that it later could (the renderer's side keeps the episode: one
/// message per refusal episode). Not a failure of the run: the frame is drawn without the map and the next frame asks again.
pub trait MapRefusalSink: Send + Sync {
    /// An attempt to make the map's grid images or target was refused; `message` names what could not be allocated.
    fn refused(&self, message: &str);
    /// An attempt succeeded after a refusal.
    fn cleared(&self);
}

/// What a frame carries for the map: the input, the camera and where the enlarged map lands.
#[derive(Clone)]
pub struct VulkanMap {
    input: Arc<VulkanMapInput>,
    step: u8,
    top_left: [i32; 2],
    view: [u32; 2],
    destination: DestinationRect,
    source: SourceRect,
    refusals: Option<Arc<dyn MapRefusalSink>>,
}

impl std::fmt::Debug for VulkanMap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VulkanMap")
            .field("input", &self.input)
            .field("step", &self.step)
            .field("top_left", &self.top_left)
            .field("view", &self.view)
            .field("destination", &self.destination)
            .field("source", &self.source)
            .field("refusals", &self.refusals.is_some())
            .finish()
    }
}

impl VulkanMap {
    /// A map frame: `input` seen at zoom `step` from world pixel `top_left` through a `view` of map pixels, drawn enlarged at
    /// `destination` (whole drawable pixels) from `source` (map pixels, a non-empty part of the view: the partial right or bottom map
    /// pixel is cropped, never stretched).
    ///
    /// # Errors
    ///
    /// A [`VulkanMapError`] for the step, the view's size, a camera whose sample points do not fit the 32-bit fixed-point world, a
    /// destination with no area, or a source outside the view.
    pub fn new(
        input: Arc<VulkanMapInput>,
        step: u8,
        top_left: [i32; 2],
        view: [u32; 2],
        destination: DestinationRect,
        source: SourceRect,
    ) -> Result<Self, VulkanMapError> {
        if !side_ok(view[0]) || !side_ok(view[1]) {
            return Err(VulkanMapError::Size);
        }
        let Some(&size) = input.steps.get(usize::from(step)) else {
            return Err(VulkanMapError::Step(step));
        };
        // Every sample point the pass computes (the view and the pixels next to it) must fit, or the shader's 32-bit arithmetic wraps.
        let fits = |axis: usize| {
            let (low, high) = (
                i64::from(top_left[axis]) - 1,
                i64::from(top_left[axis]) + i64::from(view[axis]),
            );
            let size = i64::from(size);
            low * size >= i64::from(i32::MIN) && high * size + size / 2 <= i64::from(i32::MAX)
        };
        if !fits(0) || !fits(1) {
            return Err(VulkanMapError::Camera);
        }
        if destination.right <= destination.left || destination.bottom <= destination.top {
            return Err(VulkanMapError::Destination);
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a view side is at most 16384, exactly representable as f32"
        )]
        let (width, height) = (view[0] as f32, view[1] as f32);
        let finite = [source.left, source.top, source.right, source.bottom]
            .iter()
            .all(|value| value.is_finite());
        if !(finite
            && source.left >= 0.0
            && source.top >= 0.0
            && source.right <= width
            && source.bottom <= height
            && source.right > source.left
            && source.bottom > source.top)
        {
            return Err(VulkanMapError::Source);
        }
        Ok(Self {
            input,
            step,
            top_left,
            view,
            destination,
            source,
            refusals: None,
        })
    }

    /// This map with the place the presenter tells a refused allocation to.
    #[must_use]
    pub fn with_refusals(mut self, refusals: Arc<dyn MapRefusalSink>) -> Self {
        self.refusals = Some(refusals);
        self
    }

    /// Where a refused allocation is told, if the renderer gave a place.
    #[must_use]
    pub fn refusals(&self) -> Option<&Arc<dyn MapRefusalSink>> {
        self.refusals.as_ref()
    }

    /// The input's generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.input.generation
    }

    /// The view's size in map pixels: the target's size.
    #[must_use]
    pub fn view(&self) -> [u32; 2] {
        self.view
    }

    pub(crate) fn input(&self) -> &Arc<VulkanMapInput> {
        &self.input
    }

    pub(crate) fn top_left(&self) -> [i32; 2] {
        self.top_left
    }

    pub(crate) fn step(&self) -> u8 {
        self.step
    }

    pub(crate) fn destination(&self) -> DestinationRect {
        self.destination
    }

    pub(crate) fn source(&self) -> SourceRect {
        self.source
    }
}
