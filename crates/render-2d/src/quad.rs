//! A quad in window points, and its pixel snapshot.
//!
//! A [`Quad`] is an image and a rectangle in window points, measured from the top-left corner. The bridge
//! turns it into a [`Snapshot`] (plain data in pixels) with [`Metrics`]: the window's size in points and its
//! current size in pixels.
//!
//! **Points to pixels.** Each axis has its own scale, the pixel size over the point size, so fractional and
//! unequal scales are allowed. Each edge lands at the rounded product of its coordinate in points and its
//! axis's scale (halves round away from zero). The rectangle is then clipped to the window, and the source
//! rectangle, in image pixels, is trimmed in proportion to what was cut, so the visible part is never
//! stretched. A quad entirely outside the window, or with no extent in pixels, is "no quad".
//!
//! **Large coordinates.** An edge in pixels is clamped to ±2^30 before it becomes an `i32`, so the destination
//! is always representable and the source, a proportion of the image's side, is always finite. Every quad is at
//! most [`MAX_DIMENSION`](ck_window::MAX_DIMENSION) points across, so a quad that large coordinates push far
//! outside the window stays outside after clamping (both edges clamp to the same side), and one with no extent
//! left is "no quad".

use std::fmt;
use std::sync::Arc;

use ck_resources::HandleError;
use ck_window::{MAX_DIMENSION, PixelSize, WindowDesc};

use crate::Image;
use crate::filter::Filter;
use crate::scene::{QuadKey, SceneFrame};

/// Edges, in pixels, are clamped to this magnitude before they become `i32`.
const EDGE_LIMIT: f64 = 1_073_741_824.0;

/// Why a quad's numbers are not valid.
#[derive(Debug, Clone, PartialEq)]
pub enum QuadError {
    /// A number is NaN or infinite.
    NotFinite {
        /// Which one.
        field: &'static str,
    },
    /// A size is not greater than 0 and at most [`MAX_DIMENSION`].
    BadSize {
        /// Which one.
        field: &'static str,
        /// What it was.
        value: f64,
    },
    /// A malformed token or a token of the wrong kind.
    Handle(HandleError),
    /// A key belongs to a different bridge.
    Foreign(QuadKey),
    /// A key is stale, destroyed, or its slot generation is exhausted.
    Stale(QuadKey),
    /// The scene has reached its configured live-quad limit.
    Full {
        /// The configured capacity.
        capacity: u32,
    },
}

impl fmt::Display for QuadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFinite { field } => write!(formatter, "{field} must be a finite number"),
            Self::BadSize { field, value } => {
                write!(
                    formatter,
                    "{field} {value} is not greater than 0 and at most {MAX_DIMENSION}"
                )
            }
            Self::Handle(error) => error.fmt(formatter),
            Self::Foreign(key) => write!(formatter, "quad {key} belongs to another scene"),
            Self::Stale(key) => write!(formatter, "quad {key} is no longer live"),
            Self::Full { capacity } => {
                write!(formatter, "all {capacity} scene quad slots are in use")
            }
        }
    }
}

impl std::error::Error for QuadError {}

/// An image and a rectangle in window points.
#[derive(Debug, Clone)]
pub struct Quad {
    image: Arc<Image>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Quad {
    /// A quad of `image` with its top-left corner at (`x`, `y`) points. `width` and `height` default to the
    /// image's size in pixels, read as points.
    ///
    /// # Errors
    ///
    /// [`QuadError`] unless every number is finite and each size is greater than 0 and at most
    /// [`MAX_DIMENSION`].
    pub fn new(
        image: Arc<Image>,
        x: f64,
        y: f64,
        width: Option<f64>,
        height: Option<f64>,
    ) -> Result<Self, QuadError> {
        let width = width.unwrap_or_else(|| f64::from(image.width()));
        let height = height.unwrap_or_else(|| f64::from(image.height()));
        validate_rect(x, y, width, height)?;
        Ok(Self {
            image,
            x,
            y,
            width,
            height,
        })
    }

    /// The image.
    #[must_use]
    pub fn image(&self) -> &Arc<Image> {
        &self.image
    }

    /// The left edge, in points.
    #[must_use]
    pub fn x(&self) -> f64 {
        self.x
    }

    /// The top edge, in points.
    #[must_use]
    pub fn y(&self) -> f64 {
        self.y
    }

    /// The width, in points.
    #[must_use]
    pub fn width(&self) -> f64 {
        self.width
    }

    /// The height, in points.
    #[must_use]
    pub fn height(&self) -> f64 {
        self.height
    }

    /// Where the quad lands in pixels with these metrics, or `None` if nothing of it shows.
    #[must_use]
    pub fn place(&self, metrics: &Metrics) -> Option<PlacedQuad> {
        let (left, right) = edges(
            self.x,
            self.width,
            metrics.points_width,
            metrics.pixels.width,
        );
        let (top, bottom) = edges(
            self.y,
            self.height,
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
        let (source_left, source_right) =
            trimmed(self.image.width(), left, right, visible_left, visible_right);
        let (source_top, source_bottom) = trimmed(
            self.image.height(),
            top,
            bottom,
            visible_top,
            visible_bottom,
        );
        let source = SourceRect {
            left: source_left,
            top: source_top,
            right: source_right,
            bottom: source_bottom,
        };
        // A quad a GPU type would refuse is not placed: the same answer as for a quad that is outside the window.
        if !drawable(&source, self.image.width(), self.image.height()) {
            return None;
        }
        Some(PlacedQuad {
            image: Arc::clone(&self.image),
            destination: PixelRect {
                left: visible_left,
                top: visible_top,
                right: visible_right,
                bottom: visible_bottom,
            },
            source,
        })
    }
}

/// The rule every quad rectangle must satisfy, the one [`SceneWriter`](crate::SceneWriter) applies to
/// `create_quad`, `set_rect`, `set_position` (as a one-point rectangle) and the compatibility quad: `x`, `y`, `width` and
/// `height` are finite, and `width` and `height` are greater than 0 and at most [`MAX_DIMENSION`]. A caller that can refuse
/// a bad rectangle before it does anything else (a script host, before it checks the backend or retains a texture) uses
/// this one function instead of a copy of the rule.
///
/// # Errors
///
/// [`QuadError::NotFinite`] naming the first non-finite number, or [`QuadError::BadSize`] naming the first bad size.
pub fn validate_rect(x: f64, y: f64, width: f64, height: f64) -> Result<(), QuadError> {
    for (field, value) in [("x", x), ("y", y), ("width", width), ("height", height)] {
        if !value.is_finite() {
            return Err(QuadError::NotFinite { field });
        }
    }
    for (field, value) in [("width", width), ("height", height)] {
        if !(value > 0.0 && value <= f64::from(MAX_DIMENSION)) {
            return Err(QuadError::BadSize { field, value });
        }
    }
    Ok(())
}

/// Whether `source` is a span a GPU can sample: finite, inside the `width x height` image, and non-empty
/// *as `f32`* (an extreme scale can make two distinct `f64` edges round to one `f32`, which no backend can
/// draw). The destination is already known to be non-empty here.
fn drawable(source: &SourceRect, width: u32, height: u32) -> bool {
    let finite = [source.left, source.top, source.right, source.bottom]
        .iter()
        .all(|value| value.is_finite());
    finite
        && source.left >= 0.0
        && source.top >= 0.0
        && source.right <= to_f32(f64::from(width))
        && source.bottom <= to_f32(f64::from(height))
        && source.right > source.left
        && source.bottom > source.top
}

/// A rounded edge in pixels, clamped to a representable value.
fn pixel_edge(points: f64, scale: f64) -> i32 {
    to_i32((points * scale).round().clamp(-EDGE_LIMIT, EDGE_LIMIT))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the value was clamped to ±2^30 just before, so it is an integer well inside i32"
)]
fn to_i32(value: f64) -> i32 {
    value as i32
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a source coordinate is at most the image's side (8192), exactly representable as f32"
)]
fn to_f32(value: f64) -> f32 {
    value as f32
}

/// The two edges of one axis in pixels: the start and the end, each the rounded product of its coordinate in
/// points and the axis's scale.
pub(crate) fn edges(start: f64, extent: f64, points: u32, pixels: u32) -> (i32, i32) {
    let scale = f64::from(pixels) / f64::from(points.max(1));
    (pixel_edge(start, scale), pixel_edge(start + extent, scale))
}

/// The source span, in image pixels, of the visible part of an axis: the image's `size` spread over the
/// whole `start..end` span in pixels, trimmed by what the window clipped at each end.
pub(crate) fn trimmed(
    size: u32,
    start: i32,
    end: i32,
    visible_start: i32,
    visible_end: i32,
) -> (f32, f32) {
    let size = f64::from(size);
    let span = f64::from(end) - f64::from(start);
    let from = (f64::from(visible_start) - f64::from(start)) / span * size;
    let to = size - (f64::from(end) - f64::from(visible_end)) / span * size;
    (to_f32(from), to_f32(to))
}

/// The window's size in points and in pixels: what a quad needs to be placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    /// The window's width in points.
    pub points_width: u32,
    /// The window's height in points.
    pub points_height: u32,
    /// The drawable's size in pixels.
    pub pixels: PixelSize,
}

impl Metrics {
    /// The metrics of a window of this description whose drawable is `pixels`.
    #[must_use]
    pub fn new(desc: &WindowDesc, pixels: PixelSize) -> Self {
        Self {
            points_width: desc.width,
            points_height: desc.height,
            pixels,
        }
    }
}

/// A rectangle in whole pixels: `left..right` by `top..bottom`, from the window's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    /// The left edge.
    pub left: i32,
    /// The top edge.
    pub top: i32,
    /// The right edge (exclusive).
    pub right: i32,
    /// The bottom edge (exclusive).
    pub bottom: i32,
}

/// A source rectangle. Legacy [`Quad::place`] uses image pixels; persistent frame placement uses normalized
/// fractions of the active texture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceRect {
    /// The left edge.
    pub left: f32,
    /// The top edge.
    pub top: f32,
    /// The right edge.
    pub right: f32,
    /// The bottom edge.
    pub bottom: f32,
}

/// A quad in pixels: the visible part of the image and where it goes.
#[derive(Debug, Clone)]
pub struct PlacedQuad {
    /// The image, shared.
    pub image: Arc<Image>,
    /// Where it is drawn, after clipping to the window.
    pub destination: PixelRect,
    /// The part of the image drawn there, trimmed in proportion to the clipping.
    pub source: SourceRect,
}

/// What the bridge publishes: the legacy quad in pixels plus an immutable frame in points.
///
/// Backends draw [`Snapshot::quad`] below every quad in [`Snapshot::scene`]. Legacy writes never reorder the frame.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The generation of the change that produced it. It only grows.
    pub generation: u64,
    /// How the image is sampled.
    pub filter: Filter,
    /// Whether the scene holds a quad, placed or not: false after `clear_quad` or before any quad was set,
    /// true for a quad that is fully clipped or has no extent in pixels (then `quad` is `None`). It is the
    /// scene's intent, for a backend that has to tell "nothing was asked for" from "something was asked for and
    /// does not show".
    pub requested: bool,
    /// The quad as placed, or `None` for none (or none showing).
    pub quad: Option<PlacedQuad>,
    /// The latest ordered scene frame, or `None` before its first publication.
    pub scene: Option<Arc<SceneFrame>>,
    /// The metrics in force when this snapshot was made. Frame consumers use these metrics for placement.
    pub metrics: Metrics,
}
