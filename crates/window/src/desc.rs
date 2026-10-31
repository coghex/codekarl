//! The window description, and the errors of describing one.

use std::error::Error;
use std::fmt;

/// The largest width or height, in points, of a window.
pub const MAX_DIMENSION: u32 = 16_384;

/// What a window is: a title, a fixed size in points and the colour it is cleared to.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowDesc {
    /// The title. Any string.
    pub title: String,
    /// The width in points, from 1 to [`MAX_DIMENSION`].
    pub width: u32,
    /// The height in points, from 1 to [`MAX_DIMENSION`].
    pub height: u32,
    /// The clear colour, red, green and blue. Components must be finite; finite values are clamped to
    /// 0..=1.
    pub clear: [f32; 3],
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

impl WindowDesc {
    /// The description checked and normalised: dimensions in range, colour finite and clamped.
    pub(crate) fn validated(mut self) -> Result<Self, OpenError> {
        for (name, value) in [("width", self.width), ("height", self.height)] {
            if !(1..=MAX_DIMENSION).contains(&value) {
                return Err(OpenError::InvalidDesc {
                    reason: format!("{name} {value} is not from 1 to {MAX_DIMENSION}"),
                });
            }
        }
        self.clear = clamp_colour(self.clear).map_err(|error| OpenError::InvalidDesc {
            reason: error.to_string(),
        })?;
        Ok(self)
    }
}
