//! The sampling filter.

use std::fmt;
use std::str::FromStr;

/// How a quad's image is sampled when it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    /// Each destination pixel takes the nearest image pixel.
    #[default]
    Nearest,
    /// Each destination pixel blends the four nearest image pixels.
    Linear,
}

impl Filter {
    /// The names a filter is parsed from, in the order they are listed in an error.
    pub const NAMES: [&'static str; 2] = ["nearest", "linear"];

    /// This filter's name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Nearest => "nearest",
            Self::Linear => "linear",
        }
    }
}

/// A filter name that is neither `nearest` nor `linear`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownFilter {
    /// What was given.
    pub given: String,
}

impl fmt::Display for UnknownFilter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown filter {:?}; the accepted values are \"nearest\" and \"linear\"",
            self.given
        )
    }
}

impl std::error::Error for UnknownFilter {}

impl FromStr for Filter {
    type Err = UnknownFilter;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "nearest" => Ok(Self::Nearest),
            "linear" => Ok(Self::Linear),
            _ => Err(UnknownFilter {
                given: text.to_owned(),
            }),
        }
    }
}

impl fmt::Display for Filter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}
