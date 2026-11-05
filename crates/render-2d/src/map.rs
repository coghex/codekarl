//! The map lane (milestone 6, slice 1): the inputs the application publishes beside the scene so that the GPU owners can write the
//! map image, and where that image lands in the window.
//!
//! Two immutable inputs, both plain data (no GPU object, no `ck-map` type):
//!
//! - [`MapInput`], published once per loaded map: the crop's global origin cell and size, the two packed grids, a palette of up to 256
//!   colours and a generation number;
//! - [`MapCamera`]: the top-left world pixel, the view size in map pixels, the river pixels (empty in this slice) and the metrics
//!   generation it was computed for.
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
//! # The minimal colour rule of slice 1
//!
//! Map pixel `(x, y)` of the view shows the palette colour of grid A's terrain index at cell
//! `(left + x - i0, top + y - j0)`, where `(left, top)` is the camera's top-left world pixel and `(i0, j0)` the crop's origin: one cell
//! per map pixel. A cell outside the crop shows palette entry 0. The camera's river list is carried and not drawn (slice 8).
//!
//! # Where the map image lands
//!
//! The view is `view.0 x view.1` map pixels; the enlarged map is **one quad of `(view x 4)` window points** at the window's top-left,
//! placed by the points-to-pixels rule the legacy quad uses (each edge lands at the rounded product of its position in points and the
//! axis scale). A partial map pixel at the right or bottom edge is cropped, never stretched ([`place_map`]).
//!
//! # Publication
//!
//! [`SceneBridge::publish_map`](crate::SceneBridge::publish_map) and
//! [`SceneBridge::publish_camera`](crate::SceneBridge::publish_camera) follow the scene lane's rules: under the bridge's lock, in generation
//! order, nothing freed under the lock, a stale input never overwrites a newer one, nothing published to a detached bridge, and with no map
//! input the snapshot carries no map, so the frame is exactly the frame without a map.

use std::sync::Arc;

use crate::quad::{Metrics, PixelRect, SourceRect, drawable, edges, trimmed};

/// The most colours a palette holds.
pub const PALETTE_MAX: usize = 256;

/// The longest side of a crop or a view: a size every GPU image supports.
pub const SIDE_MAX: u32 = 16_384;

/// How many window points one map pixel covers on each axis.
pub const POINTS_PER_PIXEL: u32 = 4;

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
    /// A palette with no colour or more than [`PALETTE_MAX`].
    Palette {
        /// How many colours were given.
        count: usize,
    },
    /// A palette colour whose alpha is not 255: the map target is opaque, and the enlarged map is blended as premultiplied.
    PaletteAlpha {
        /// The colour's index in the palette.
        index: usize,
        /// Its alpha.
        alpha: u8,
    },
    /// A terrain index that the palette does not have.
    TerrainIndex {
        /// The cell's column within the crop.
        x: u32,
        /// The cell's row within the crop.
        y: u32,
        /// The index.
        index: u8,
        /// The palette's length.
        palette: usize,
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
}

impl std::fmt::Display for MapError {
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
            Self::Palette { count } => write!(
                formatter,
                "the palette has {count} colours; it needs from 1 to {PALETTE_MAX}"
            ),
            Self::PaletteAlpha { index, alpha } => write!(
                formatter,
                "palette colour {index} has alpha {alpha}; every colour must be opaque (255)"
            ),
            Self::TerrainIndex {
                x,
                y,
                index,
                palette,
            } => write!(
                formatter,
                "grid A cell ({x}, {y}) has terrain index {index}; the palette has {palette} colours"
            ),
            Self::RiverOutsideView { position, x, y } => write!(
                formatter,
                "river pixel {position} ({x}, {y}) is outside the view"
            ),
            Self::TooManyRivers { limit, count } => write!(
                formatter,
                "{count} river pixels; the view holds at most {limit}"
            ),
        }
    }
}

impl std::error::Error for MapError {}

fn side_ok(side: u32) -> bool {
    (1..=SIDE_MAX).contains(&side)
}

/// A loaded map as the GPU reads it: immutable, published once, replaced as a whole by a newer generation.
#[derive(Debug)]
pub struct MapInput {
    generation: u64,
    origin: [i32; 2],
    size: [u32; 2],
    grid_a: Arc<[u8]>,
    grid_b: Arc<[u8]>,
    palette: Arc<[[u8; 4]]>,
}

impl MapInput {
    /// A map input, checked against the layout in the module documentation.
    ///
    /// `origin` is the crop's global origin cell `(i0, j0)`; `size` its width and height in cells; `grid_a` and `grid_b` the packed
    /// grids; `palette` up to [`PALETTE_MAX`] RGBA8 colours; `generation` the number that orders inputs (a newer input has a larger one).
    ///
    /// # Errors
    ///
    /// The first [`MapError`] found: a size, a grid length, a cell that breaks the layout, the palette's length, or a terrain index the
    /// palette lacks.
    pub fn new(
        generation: u64,
        origin: [i32; 2],
        size: [u32; 2],
        grid_a: Vec<u8>,
        grid_b: Vec<u8>,
        palette: Vec<[u8; 4]>,
    ) -> Result<Self, MapError> {
        let [width, height] = size;
        if !side_ok(width) || !side_ok(height) {
            return Err(MapError::Size {
                what: "crop",
                width,
                height,
            });
        }
        if palette.is_empty() || palette.len() > PALETTE_MAX {
            return Err(MapError::Palette {
                count: palette.len(),
            });
        }
        if let Some((index, colour)) = palette
            .iter()
            .enumerate()
            .find(|(_, colour)| colour[3] != 255)
        {
            return Err(MapError::PaletteAlpha {
                index,
                alpha: colour[3],
            });
        }
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
            if usize::from(pixel[0]) >= palette.len() {
                return Err(MapError::TerrainIndex {
                    x,
                    y,
                    index: pixel[0],
                    palette: palette.len(),
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
            palette: palette.into(),
        })
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

    /// The palette, shared.
    #[must_use]
    pub fn palette(&self) -> &Arc<[[u8; 4]]> {
        &self.palette
    }
}

/// The camera as the map pass reads it: immutable, one per change of the view.
#[derive(Debug)]
pub struct MapCamera {
    top_left: [i32; 2],
    view: [u32; 2],
    rivers: Arc<[[u32; 2]]>,
    metrics_generation: u64,
}

impl MapCamera {
    /// A camera at world pixel `top_left` showing a `view` of map pixels, computed for the bridge's metrics of
    /// `metrics_generation` ([`SceneBridge::metrics_generation`](crate::SceneBridge::metrics_generation)).
    ///
    /// `rivers` are the visible river pixels in view coordinates; the list is empty in slice 1 and bounded by the view's pixel count.
    ///
    /// # Errors
    ///
    /// [`MapError::Size`] for a view with a zero or too long side, [`MapError::TooManyRivers`], or [`MapError::RiverOutsideView`].
    pub fn new(
        top_left: [i32; 2],
        view: [u32; 2],
        rivers: Vec<[u32; 2]>,
        metrics_generation: u64,
    ) -> Result<Self, MapError> {
        let [width, height] = view;
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
        Ok(Self {
            top_left,
            view,
            rivers: rivers.into(),
            metrics_generation,
        })
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

    /// The visible river pixels in view coordinates (not drawn in slice 1).
    #[must_use]
    pub fn rivers(&self) -> &Arc<[[u32; 2]]> {
        &self.rivers
    }

    /// The metrics generation this camera was computed for.
    #[must_use]
    pub fn metrics_generation(&self) -> u64 {
        self.metrics_generation
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
