//! The plain quad types the Vulkan drawing takes: no Vulkan object, no device, no dependency on a renderer
//! crate (design §7.1).
//!
//! - [`PixelSource`] is where the pixels come from: premultiplied RGBA8, rows top first. The GPU side copies
//!   them into a staging buffer once, and then drops its reference to the source.
//! - [`VulkanQuad`] is a quad checked **without a device**: the source, its size, the destination rectangle in
//!   whole target pixels, the part of the image to draw and the filter. [`VulkanQuad::new`] refuses what no
//!   device could draw, naming the reason, so an invalid quad never reaches the GPU. What depends on the device
//!   (its `maxImageDimension2D`, the format's capabilities) is checked later, on the thread that has one.
//! - [`VulkanScene`] is what a renderer publishes: a generation, and a quad or none. The generation travels
//!   with or without a quad.

use std::fmt;
use std::sync::Arc;

/// The largest width or height of a quad's image, in pixels.
pub const MAX_IMAGE_SIDE: u32 = 8192;

/// The most bytes of RGBA8 one quad texture may take: 64 MiB (for example 4096 × 4096).
pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

/// Where a quad's pixels come from. An implementation owns the image (typically behind an `Arc`, shared with
/// whoever decoded it), so handing the drawing a source copies no pixels.
pub trait PixelSource: Send + Sync {
    /// The pixels: premultiplied RGBA8, rows top first, exactly `width × height × 4` bytes for the quad this
    /// source belongs to.
    fn pixels(&self) -> &[u8];
}

/// How a quad's image is sampled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VulkanFilter {
    /// Each destination pixel takes the nearest image pixel.
    Nearest,
    /// Each destination pixel blends the four nearest image pixels.
    Linear,
}

/// A rectangle in whole target pixels, from the target's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DestinationRect {
    /// The left edge.
    pub left: i32,
    /// The top edge.
    pub top: i32,
    /// The right edge (exclusive); greater than `left`.
    pub right: i32,
    /// The bottom edge (exclusive); greater than `top`.
    pub bottom: i32,
}

/// The part of the image to draw, in image pixels from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceRect {
    /// The left edge.
    pub left: f32,
    /// The top edge.
    pub top: f32,
    /// The right edge; greater than `left`.
    pub right: f32,
    /// The bottom edge; greater than `top`.
    pub bottom: f32,
}

/// Why a quad is not valid. An invalid quad never reaches the GPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VulkanQuadError {
    /// A width or height is zero or over [`MAX_IMAGE_SIDE`].
    ImageSize {
        /// The width given.
        width: u32,
        /// The height given.
        height: u32,
    },
    /// The image would take more than [`MAX_IMAGE_BYTES`] as RGBA8.
    ImageTooLarge {
        /// The bytes `width × height × 4` come to.
        bytes: u64,
    },
    /// The source did not hold exactly `width × height × 4` bytes.
    PixelLength {
        /// The bytes the size needs.
        expected: usize,
        /// The bytes the source held.
        actual: usize,
    },
    /// The destination rectangle is empty or inside out.
    Destination,
    /// The source rectangle is not finite, not inside the image, or empty.
    Source,
}

impl fmt::Display for VulkanQuadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImageSize { width, height } => write!(
                formatter,
                "the image is {width} x {height} pixels; each side must be from 1 to {MAX_IMAGE_SIDE}"
            ),
            Self::ImageTooLarge { bytes } => write!(
                formatter,
                "the image would take {bytes} bytes as RGBA8, over the {MAX_IMAGE_BYTES} a quad texture may take"
            ),
            Self::PixelLength { expected, actual } => {
                write!(formatter, "the pixel source holds {actual} bytes, not {expected}")
            }
            Self::Destination => formatter.write_str(
                "the destination rectangle needs right greater than left and bottom greater than top",
            ),
            Self::Source => formatter.write_str(
                "the source rectangle must be finite, non-empty and inside the image",
            ),
        }
    }
}

impl std::error::Error for VulkanQuadError {}

/// A validated quad: an image and where, and how, to draw it.
#[derive(Clone)]
pub struct VulkanQuad {
    pixels: Arc<dyn PixelSource>,
    width: u32,
    height: u32,
    destination: DestinationRect,
    source: SourceRect,
    filter: VulkanFilter,
}

impl fmt::Debug for VulkanQuad {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VulkanQuad")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("destination", &self.destination)
            .field("source", &self.source)
            .field("filter", &self.filter)
            .finish_non_exhaustive()
    }
}

impl VulkanQuad {
    /// A quad of the `width × height` image `pixels` holds.
    ///
    /// # Errors
    ///
    /// [`VulkanQuadError`] unless:
    /// - each side is from 1 to [`MAX_IMAGE_SIDE`];
    /// - the image takes at most [`MAX_IMAGE_BYTES`] as RGBA8 (checked before the source's bytes are looked at);
    /// - `pixels` holds exactly `width × height × 4` bytes;
    /// - `destination` has right greater than left and bottom greater than top;
    /// - `source` is finite, non-empty and inside the image.
    pub fn new(
        pixels: Arc<dyn PixelSource>,
        width: u32,
        height: u32,
        destination: DestinationRect,
        source: SourceRect,
        filter: VulkanFilter,
    ) -> Result<Self, VulkanQuadError> {
        if !(1..=MAX_IMAGE_SIDE).contains(&width) || !(1..=MAX_IMAGE_SIDE).contains(&height) {
            return Err(VulkanQuadError::ImageSize { width, height });
        }
        // Checked arithmetic in u64, so nothing overflows on any target: at most 2^13 × 2^13 × 4 = 2^28.
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(VulkanQuadError::ImageTooLarge { bytes: u64::MAX })?;
        if bytes > MAX_IMAGE_BYTES {
            return Err(VulkanQuadError::ImageTooLarge { bytes });
        }
        let expected = (width as usize) * (height as usize) * 4;
        let actual = pixels.pixels().len();
        if actual != expected {
            return Err(VulkanQuadError::PixelLength { expected, actual });
        }
        if destination.right <= destination.left || destination.bottom <= destination.top {
            return Err(VulkanQuadError::Destination);
        }
        let (image_width, image_height) = (to_f32(width), to_f32(height));
        let finite = [source.left, source.top, source.right, source.bottom]
            .iter()
            .all(|value| value.is_finite());
        let inside = source.left >= 0.0
            && source.top >= 0.0
            && source.right <= image_width
            && source.bottom <= image_height;
        if !finite || !inside || source.right <= source.left || source.bottom <= source.top {
            return Err(VulkanQuadError::Source);
        }
        Ok(Self {
            pixels,
            width,
            height,
            destination,
            source,
            filter,
        })
    }

    /// The image's width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The image's height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Where the quad goes, in whole target pixels.
    #[must_use]
    pub fn destination(&self) -> DestinationRect {
        self.destination
    }

    /// The part of the image to draw.
    #[must_use]
    pub fn source(&self) -> SourceRect {
        self.source
    }

    /// How the image is sampled.
    #[must_use]
    pub fn filter(&self) -> VulkanFilter {
        self.filter
    }

    /// The pixel source (its allocation, with the width and height, is the image's identity).
    #[must_use]
    pub fn pixels(&self) -> &Arc<dyn PixelSource> {
        &self.pixels
    }
}

/// A side of at most 8192 is exactly representable as `f32`.
#[expect(
    clippy::cast_precision_loss,
    reason = "the value is at most 8192, which f32 holds exactly"
)]
fn to_f32(side: u32) -> f32 {
    side as f32
}

/// What a renderer publishes: the generation of the change that made it, and a quad or none. The generation
/// travels with or without a quad, so a scene that shows nothing is told apart from the one before it.
#[derive(Debug, Clone)]
pub struct VulkanScene {
    generation: u64,
    quad: Option<VulkanQuad>,
}

impl VulkanScene {
    /// A scene of this `generation` showing `quad`, or nothing.
    #[must_use]
    pub fn new(generation: u64, quad: Option<VulkanQuad>) -> Self {
        Self { generation, quad }
    }

    /// The generation of the change that made this scene.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The quad, if the scene shows one.
    #[must_use]
    pub fn quad(&self) -> Option<&VulkanQuad> {
        self.quad.as_ref()
    }
}
