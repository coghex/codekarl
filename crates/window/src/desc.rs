//! The window description, and the errors of describing one.

use std::error::Error;
use std::fmt;

/// The largest width or height, in points, of a window.
pub const MAX_DIMENSION: u32 = 16_384;

/// The unit a window's size and minimum are given in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SizeUnit {
    /// Points: the unit every caller used before resizable windows.
    #[default]
    Points,
    /// Screen pixels, resolved to points once, against the main screen, before the window is requested
    /// ([`WindowDesc::resolve`]).
    Pixels,
}

/// The size a window was asked for in pixels, as requested (before any clamp to a small screen), kept on a resolved description:
/// the backend recomputes the minimum from it when the backing scale changes, and the UI scale reads it to know the window was opened
/// in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRequest {
    /// The opening width and height in pixels.
    pub size: (u32, u32),
    /// The minimum width and height in pixels, if there is one.
    pub min: Option<(u32, u32)>,
}

/// The main screen as the main thread saw it once, before the UI VM started: what resolves a size in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenSnapshot {
    /// Pixels per point.
    pub backing: f64,
    /// The width and height of the screen's visible area in points.
    pub visible_points: (u32, u32),
}

impl ScreenSnapshot {
    /// The screen of a run with no window system: one pixel per point and no limit on the visible size.
    #[must_use]
    pub const fn unlimited() -> Self {
        Self {
            backing: 1.0,
            visible_points: (u32::MAX, u32::MAX),
        }
    }
}

/// What a window is: a title, a size, whether it can be resized, an optional minimum, and the colour it is cleared to.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowDesc {
    /// The title. Any string.
    pub title: String,
    /// The width in [`unit`](Self::unit), from 1 to [`MAX_DIMENSION`].
    pub width: u32,
    /// The height in [`unit`](Self::unit), from 1 to [`MAX_DIMENSION`].
    pub height: u32,
    /// The clear colour, red, green and blue. Components must be finite; finite values are clamped to
    /// 0..=1.
    pub clear: [f32; 3],
    /// What `width`, `height` and `min` are in. A description handed to a backend, a presenter or the request slot is in points: pixels
    /// are resolved first ([`WindowDesc::resolve`]) and the resolved description says [`SizeUnit::Points`].
    pub unit: SizeUnit,
    /// Whether the user can resize the window, zoom it and make it full screen.
    pub resizable: bool,
    /// The smallest size, width and height in [`unit`](Self::unit), of a resizable window.
    pub min: Option<(u32, u32)>,
    /// The size as requested in pixels, kept by [`resolve`](Self::resolve); `None` for a request in points.
    pub pixel_request: Option<PixelRequest>,
}

/// Why a window could not be described or opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// The description is not valid; nothing was recorded.
    InvalidDesc {
        /// What is wrong with it.
        reason: String,
    },
    /// There is one window and it was already requested.
    AlreadyOpened,
}

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDesc { reason } => {
                write!(formatter, "invalid window description: {reason}")
            }
            Self::AlreadyOpened => formatter.write_str("the window was already opened"),
        }
    }
}

impl Error for OpenError {}

/// A colour with a component that is not finite (NaN or infinite); nothing was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidColour {
    /// Which component (0 red, 1 green, 2 blue).
    pub component: usize,
}

impl fmt::Display for InvalidColour {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "colour component {} is not finite",
            self.component
        )
    }
}

impl Error for InvalidColour {}

/// The colour with every component clamped to 0..=1; the first non-finite component is an error.
pub(crate) fn clamp_colour(rgb: [f32; 3]) -> Result<[f32; 3], InvalidColour> {
    for (component, value) in rgb.iter().enumerate() {
        if !value.is_finite() {
            return Err(InvalidColour { component });
        }
    }
    Ok(rgb.map(|value| value.clamp(0.0, 1.0)))
}

impl Default for WindowDesc {
    /// A description of no size (which validation refuses), black, in points, fixed and with no minimum: the base of a literal with
    /// struct-update syntax, so that a caller names only what it sets.
    fn default() -> Self {
        Self {
            title: String::new(),
            width: 0,
            height: 0,
            clear: [0.0; 3],
            unit: SizeUnit::Points,
            resizable: false,
            min: None,
            pixel_request: None,
        }
    }
}

/// A description resolved against a screen, and the lines to log for what the screen's size forced.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// The description in points, with `pixel_request` set when it was asked for in pixels.
    pub desc: WindowDesc,
    /// One line for each clamp to a smaller screen: the sizes requested and the sizes used. Empty when the screen was large enough.
    pub clamps: Vec<String>,
}

fn invalid(reason: String) -> OpenError {
    OpenError::InvalidDesc { reason }
}

/// `⌈pixels ÷ backing⌉`, never below 1 and never above [`MAX_DIMENSION`].
fn pixels_to_points(pixels: u32, backing: f64) -> u32 {
    let backing = if backing.is_finite() && backing > 0.0 {
        backing
    } else {
        1.0
    };
    let points = (f64::from(pixels) / backing).ceil();
    // `points` is finite and not negative; the float-to-integer conversion saturates.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a finite ceiling of a non-negative quotient; the conversion saturates"
    )]
    let points = points as u32;
    points.clamp(1, MAX_DIMENSION)
}

impl WindowDesc {
    /// The sizes and the minimum checked in the description's own unit, and the colour clamped. Does not look at `unit`.
    fn checked(mut self) -> Result<Self, OpenError> {
        for (name, value) in [("width", self.width), ("height", self.height)] {
            if !(1..=MAX_DIMENSION).contains(&value) {
                return Err(invalid(format!(
                    "{name} {value} is not from 1 to {MAX_DIMENSION}"
                )));
            }
        }
        if let Some((min_width, min_height)) = self.min {
            if !self.resizable {
                return Err(invalid(
                    "min_width and min_height need a resizable window".to_owned(),
                ));
            }
            for (name, value, size) in [
                ("min_width", min_width, self.width),
                ("min_height", min_height, self.height),
            ] {
                if !(1..=MAX_DIMENSION).contains(&value) {
                    return Err(invalid(format!(
                        "{name} {value} is not from 1 to {MAX_DIMENSION}"
                    )));
                }
                if value > size {
                    return Err(invalid(format!(
                        "{name} {value} is larger than the opening size {size}"
                    )));
                }
            }
        }
        self.clear = clamp_colour(self.clear).map_err(|error| invalid(error.to_string()))?;
        Ok(self)
    }

    /// The description checked and normalised for the request slot: dimensions in range, a minimum only on a resizable window and
    /// no larger than the opening size, colour finite and clamped. A description in pixels is refused: it was not resolved.
    pub(crate) fn validated(self) -> Result<Self, OpenError> {
        if self.unit != SizeUnit::Points {
            return Err(invalid(
                "unit pixels was not resolved to points before the window was requested".to_owned(),
            ));
        }
        self.checked()
    }

    /// The description in points: checked in its own unit, then, for [`SizeUnit::Pixels`], each size converted once with
    /// `points = ⌈pixels ÷ backing⌉`, and the opening size clamped to the screen's visible size (a minimum larger than the clamped
    /// size is lowered to it; a clamp never raises a size). The result is in points and, for a request in pixels, carries the sizes as
    /// requested in [`pixel_request`](Self::pixel_request). A request in points is checked and kept as it is: it is not clamped.
    ///
    /// # Errors
    ///
    /// [`OpenError::InvalidDesc`], naming the field, when the description is not valid in its own unit.
    pub fn resolve(self, screen: &ScreenSnapshot) -> Result<Resolved, OpenError> {
        let mut desc = self.checked()?;
        let mut clamps = Vec::new();
        if desc.unit == SizeUnit::Points {
            return Ok(Resolved { desc, clamps });
        }
        let request = PixelRequest {
            size: (desc.width, desc.height),
            min: desc.min,
        };
        let convert = |pixels: u32| pixels_to_points(pixels, screen.backing);
        let (visible_width, visible_height) = screen.visible_points;
        let (width, height) = (convert(desc.width), convert(desc.height));
        let (used_width, used_height) = (width.min(visible_width), height.min(visible_height));
        if (used_width, used_height) != (width, height) {
            clamps.push(format!(
                "the window of {width}x{height} points does not fit the screen's {visible_width}x{visible_height} visible points: opening at {used_width}x{used_height}"
            ));
        }
        desc.width = used_width;
        desc.height = used_height;
        desc.min = desc.min.map(|(min_width, min_height)| {
            let (min_width, min_height) = (convert(min_width), convert(min_height));
            let used = (min_width.min(used_width), min_height.min(used_height));
            if used != (min_width, min_height) {
                clamps.push(format!(
                    "the minimum of {min_width}x{min_height} points is larger than the window: using {}x{}",
                    used.0, used.1
                ));
            }
            used
        });
        desc.unit = SizeUnit::Points;
        desc.pixel_request = Some(request);
        Ok(Resolved { desc, clamps })
    }
}
