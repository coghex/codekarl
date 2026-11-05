//! The GPU-written map image (milestone 6, slice 1): plain data the renderer hands the presenter. The drawing is
//! [`gpu_map`](crate::gpu_map)'s, and the bookkeeping of what is resident [`map_residency`](crate::map_residency)'s.
//!
//! A frame may carry a [`VulkanMap`]: a loaded map ([`VulkanMapInput`]: two packed RGBA8 grids of the crop's size and a palette), the
//! camera's top-left world pixel and view size, and where the enlarged map lands. The presenter then:
//!
//! 1. makes the input resident when its generation is new (three `R8G8B8A8_UNORM` images read only by exact integer reads
//!    (`texelFetch`): grid A, grid B and the palette as a 256 x 1 image), uploading them in the frame's command buffer and retiring the
//!    images they replace under the ledger's proof of the last submit that used them;
//! 2. makes a map target resident when the view's size is new (an `R8G8B8A8_UNORM` image the size of the view, a colour attachment and
//!    a sampled image), retiring the old one the same way;
//! 3. records the **map pass** in the frame's command buffer, before the window pass: one triangle over the target with the slice's
//!    minimal colour rule — map pixel `(x, y)` shows the palette colour of grid A's terrain index (the R byte) at cell
//!    `(left + x - i0, top + y - j0)`, one cell per map pixel, and palette entry 0 outside the crop;
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

/// The most colours a palette holds.
pub const PALETTE_MAX: usize = 256;
/// The longest side of a crop or a view, in cells or map pixels.
pub const MAP_SIDE_MAX: u32 = 16_384;

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
    /// A palette with no colour or more than [`PALETTE_MAX`].
    Palette(usize),
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
            Self::Palette(count) => write!(
                formatter,
                "the map palette has {count} colours; it needs from 1 to {PALETTE_MAX}"
            ),
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

/// A loaded map as the presenter reads it: immutable, shared, identified by its generation.
#[derive(Debug)]
pub struct VulkanMapInput {
    generation: u64,
    origin: [i32; 2],
    size: [u32; 2],
    grid_a: Arc<[u8]>,
    grid_b: Arc<[u8]>,
    palette: Arc<[[u8; 4]]>,
}

impl VulkanMapInput {
    /// An input of `generation`: the crop's global origin cell, its size in cells, the two RGBA8 grids (`size.0 * size.1 * 4` bytes
    /// each) and the palette (1 to [`PALETTE_MAX`] colours). The renderer validates the layout; the presenter checks only what it
    /// needs to size its images.
    ///
    /// # Errors
    ///
    /// A [`VulkanMapError`] for a size, a grid length or the palette's length.
    pub fn new(
        generation: u64,
        origin: [i32; 2],
        size: [u32; 2],
        grid_a: Arc<[u8]>,
        grid_b: Arc<[u8]>,
        palette: Arc<[[u8; 4]]>,
    ) -> Result<Self, VulkanMapError> {
        if !side_ok(size[0]) || !side_ok(size[1]) {
            return Err(VulkanMapError::Size);
        }
        if palette.is_empty() || palette.len() > PALETTE_MAX {
            return Err(VulkanMapError::Palette(palette.len()));
        }
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
            palette,
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

    /// The palette as a 256 x 1 RGBA8 image: its colours, then zeros.
    pub(crate) fn palette_image(&self) -> Vec<u8> {
        let mut bytes = vec![0_u8; PALETTE_MAX * 4];
        for (slot, colour) in bytes.chunks_exact_mut(4).zip(self.palette.iter()) {
            slot.copy_from_slice(colour);
        }
        bytes
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
            .field("top_left", &self.top_left)
            .field("view", &self.view)
            .field("destination", &self.destination)
            .field("source", &self.source)
            .field("refusals", &self.refusals.is_some())
            .finish()
    }
}

impl VulkanMap {
    /// A map frame: `input` seen from world pixel `top_left` through a `view` of map pixels, drawn enlarged at `destination` (whole
    /// drawable pixels) from `source` (map pixels, a non-empty part of the view: the partial right or bottom map pixel is cropped,
    /// never stretched).
    ///
    /// # Errors
    ///
    /// A [`VulkanMapError`] for the view's size, a destination with no area, or a source outside the view.
    pub fn new(
        input: Arc<VulkanMapInput>,
        top_left: [i32; 2],
        view: [u32; 2],
        destination: DestinationRect,
        source: SourceRect,
    ) -> Result<Self, VulkanMapError> {
        if !side_ok(view[0]) || !side_ok(view[1]) {
            return Err(VulkanMapError::Size);
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

    pub(crate) fn destination(&self) -> DestinationRect {
        self.destination
    }

    pub(crate) fn source(&self) -> SourceRect {
        self.source
    }
}
