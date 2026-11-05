//! The GPU-written map image (milestone 6, slice 1): plain data the renderer hands the presenter, and the off-screen readback of the
//! presenter's own map pass.
//!
//! A frame may carry a [`MetalMap`]: a loaded map ([`MetalMapInput`]: two packed RGBA8 grids of the crop's size and a palette), the
//! camera's top-left world pixel and view size, and where the enlarged map lands. The presenter then:
//!
//! 1. makes the input resident when its generation is new (three images read only by exact integer reads: grid A, grid B and the
//!    palette as a 256 x 1 image), retiring the images it replaces under the proof of the last command buffer that used them;
//! 2. makes a map target resident when the view's size is new (an RGBA8 image the size of the view, a colour attachment and a sampled
//!    image), retiring the old one the same way;
//! 3. encodes the **map pass** in the frame's command buffer, before the window pass: one triangle over the target with the slice's
//!    minimal colour rule — map pixel `(x, y)` shows the palette colour of grid A's terrain index (the R byte) at cell
//!    `(left + x - i0, top + y - j0)`, one cell per map pixel, and palette entry 0 outside the crop;
//! 4. draws the enlarged map in the window pass between the clear and the legacy quad: the target, nearest-sampled, at the frame's
//!    destination.
//!
//! The target is a hazard-tracked resource on the presenter's one queue, so Metal orders the map pass's writes before the window pass's
//! reads, and the next frame's writes after this frame's reads, with no CPU wait. A frame with a zero-size drawable encodes no map pass
//! and keeps the resident target for the next visible frame. The grid layout is documented in `ck-render-2d` (`map` module).

use std::ffi::c_char;
use std::sync::Arc;

use crate::quad::{DestinationRect, MetalQuad, SourceRect};
use crate::{Error, message};

/// The most colours a palette holds.
pub const PALETTE_MAX: usize = 256;
/// The longest side of a crop or a view, in cells or map pixels.
pub const MAP_SIDE_MAX: u32 = 16_384;

/// Why a map was refused before it reached the GPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetalMapError {
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

impl std::fmt::Display for MetalMapError {
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

impl std::error::Error for MetalMapError {}

fn side_ok(side: u32) -> bool {
    (1..=MAP_SIDE_MAX).contains(&side)
}

/// A loaded map as the presenter reads it: immutable, shared, identified by its generation.
#[derive(Debug)]
pub struct MetalMapInput {
    generation: u64,
    origin: [i32; 2],
    size: [u32; 2],
    grid_a: Arc<[u8]>,
    grid_b: Arc<[u8]>,
    palette: Arc<[[u8; 4]]>,
}

impl MetalMapInput {
    /// An input of `generation`: the crop's global origin cell, its size in cells, the two RGBA8 grids (`size.0 * size.1 * 4` bytes
    /// each) and the palette (1 to [`PALETTE_MAX`] colours). The renderer validates the layout; the presenter checks only what it
    /// needs to size its images.
    ///
    /// # Errors
    ///
    /// A [`MetalMapError`] for a size, a grid length or the palette's length.
    pub fn new(
        generation: u64,
        origin: [i32; 2],
        size: [u32; 2],
        grid_a: Arc<[u8]>,
        grid_b: Arc<[u8]>,
        palette: Arc<[[u8; 4]]>,
    ) -> Result<Self, MetalMapError> {
        if !side_ok(size[0]) || !side_ok(size[1]) {
            return Err(MetalMapError::Size);
        }
        if palette.is_empty() || palette.len() > PALETTE_MAX {
            return Err(MetalMapError::Palette(palette.len()));
        }
        let expected = usize::try_from(u64::from(size[0]) * u64::from(size[1]) * 4)
            .map_err(|_| MetalMapError::Size)?;
        for grid in [&grid_a, &grid_b] {
            if grid.len() != expected {
                return Err(MetalMapError::GridLength {
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
pub struct MetalMap {
    input: Arc<MetalMapInput>,
    top_left: [i32; 2],
    view: [u32; 2],
    destination: DestinationRect,
    source: SourceRect,
    refusals: Option<Arc<dyn MapRefusalSink>>,
}

impl std::fmt::Debug for MetalMap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalMap")
            .field("input", &self.input)
            .field("top_left", &self.top_left)
            .field("view", &self.view)
            .field("destination", &self.destination)
            .field("source", &self.source)
            .field("refusals", &self.refusals.is_some())
            .finish()
    }
}

impl MetalMap {
    /// A map frame: `input` seen from world pixel `top_left` through a `view` of map pixels, drawn enlarged at `destination` (whole
    /// drawable pixels) from `source` (map pixels, a non-empty part of the view: the partial right or bottom map pixel is cropped,
    /// never stretched).
    ///
    /// # Errors
    ///
    /// A [`MetalMapError`] for the view's size, a destination with no area, or a source outside the view.
    pub fn new(
        input: Arc<MetalMapInput>,
        top_left: [i32; 2],
        view: [u32; 2],
        destination: DestinationRect,
        source: SourceRect,
    ) -> Result<Self, MetalMapError> {
        if !side_ok(view[0]) || !side_ok(view[1]) {
            return Err(MetalMapError::Size);
        }
        if destination.right <= destination.left || destination.bottom <= destination.top {
            return Err(MetalMapError::Destination);
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
            return Err(MetalMapError::Source);
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
}

/// The map of one frame (`CKPrMap`). The pointers borrow the [`MetalMap`] it was made from, which the caller keeps alive for the call.
#[repr(C)]
pub(crate) struct NativeMap {
    present: i32,
    generation: u64,
    origin: [i32; 2],
    crop: [u32; 2],
    top_left: [i32; 2],
    view: [u32; 2],
    destination: [i32; 4],
    source: [f32; 4],
    grid_a: *const u8,
    grid_b: *const u8,
    palette: *const u8,
    palette_count: u32,
}

impl NativeMap {
    /// No map.
    pub(crate) const NONE: Self = Self {
        present: 0,
        generation: 0,
        origin: [0; 2],
        crop: [0; 2],
        top_left: [0; 2],
        view: [0; 2],
        destination: [0; 4],
        source: [0.0; 4],
        grid_a: std::ptr::null(),
        grid_b: std::ptr::null(),
        palette: std::ptr::null(),
        palette_count: 0,
    };

    /// The native description of `map`; `with_grids` also names the input's images' bytes (the first frame of a generation).
    pub(crate) fn of(map: &MetalMap, with_grids: bool) -> Self {
        let input = &map.input;
        let (grid_a, grid_b, palette) = if with_grids {
            (
                input.grid_a.as_ptr(),
                input.grid_b.as_ptr(),
                input.palette.as_ptr().cast::<u8>(),
            )
        } else {
            (std::ptr::null(), std::ptr::null(), std::ptr::null())
        };
        Self {
            present: 1,
            generation: input.generation,
            origin: input.origin,
            crop: input.size,
            top_left: map.top_left,
            view: map.view,
            destination: [
                map.destination.left,
                map.destination.top,
                map.destination.right,
                map.destination.bottom,
            ],
            source: [
                map.source.left,
                map.source.top,
                map.source.right,
                map.source.bottom,
            ],
            grid_a,
            grid_b,
            palette,
            palette_count: u32::try_from(input.palette.len()).unwrap_or(0),
        }
    }
}

unsafe extern "C" {
    fn ck_pr_readback_map(
        width: u32,
        height: u32,
        rgb: *const f32,
        library: *const u8,
        library_length: usize,
        map: *const NativeMap,
        quad: *const crate::presenter::NativeQuad,
        quad_image: *const u8,
        quad_image_length: usize,
        target_pixels: *mut u8,
        target_length: usize,
        pixels: *mut u8,
        length: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
}

/// What an off-screen map frame drew ([`readback_map`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapReadback {
    /// The window pass: `width x height` BGRA8 pixels, rows top first.
    pub window: Vec<u8>,
    /// The map target the map pass wrote: `view.0 x view.1` RGBA8 pixels, rows top first. `None` when the frame had no map.
    pub target: Option<Vec<u8>>,
}

/// Renders a frame off screen **through the presenter's own map pass and composition** — the pipelines, the images, the encoding —
/// and reads back both the map target and the window. `map` `None` is the frame without a map (the window pass alone). `quad` is an
/// optional legacy quad drawn over the map. Used by the readback tests and by nothing else.
///
/// # Errors
///
/// The native layer's message when there is no device or the work fails.
pub fn readback_map(
    width: u32,
    height: u32,
    rgb: [f32; 3],
    map: Option<&MetalMap>,
    quad: Option<&MetalQuad>,
) -> Result<MapReadback, Error> {
    let length = (width as usize) * (height as usize) * 4;
    let mut window = vec![0_u8; length];
    let mut target = map.map(|map| {
        let [view_width, view_height] = map.view;
        vec![0_u8; (view_width as usize) * (view_height as usize) * 4]
    });
    let mut error = [0_u8; 512];
    let native = map.map_or(NativeMap::NONE, |map| NativeMap::of(map, true));
    let native_quad = quad.map_or(
        crate::presenter::NativeQuad::NONE,
        crate::presenter::NativeQuad::of,
    );
    let image = quad.map_or(&[][..], |quad| quad.pixels().pixels());
    let (target_pointer, target_length) =
        target.as_mut().map_or((std::ptr::null_mut(), 0), |bytes| {
            (bytes.as_mut_ptr(), bytes.len())
        });
    // SAFETY: `rgb`, the library bytes, `native` (whose pointers borrow `map`, alive for the call), `native_quad`, `image`, both
    // pixel buffers (of the stated lengths) and `error` are valid for the synchronous call.
    let status = unsafe {
        ck_pr_readback_map(
            width,
            height,
            rgb.as_ptr(),
            crate::presenter::QUAD_LIBRARY.as_ptr(),
            crate::presenter::QUAD_LIBRARY.len(),
            &raw const native,
            &raw const native_quad,
            image.as_ptr(),
            image.len(),
            target_pointer,
            target_length,
            window.as_mut_ptr(),
            window.len(),
            error.as_mut_ptr().cast::<c_char>(),
            error.len(),
        )
    };
    if status == 0 {
        Ok(MapReadback { window, target })
    } else {
        Err(message(&error))
    }
}
