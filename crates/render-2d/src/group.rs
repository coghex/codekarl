//! Scene groups (milestone 8, slice 4): the token a script holds for one, the rectangles groups clip and hit with, and the bounded point
//! domain every one of those numbers lies in.
//!
//! A group is an ordered set of quads that is raised, hidden, moved and clipped as one. Its offset and clip are applied when a frame is
//! built, and its hit regions when the regions are published (`docs/ui-design.md` §3.4 and §3.5).

use std::fmt;

use ck_resources::{HandleError, RegistryId, Token, TokenKind};

/// The largest magnitude, in window points, of a group offset, a clip edge and a hit-rectangle edge (as set and as translated by its
/// group's offset): `i32::MAX / 256` rounded down. It is the largest range whose conversion to pointer units (×256) always fits `i32`,
/// so publication of hit regions has no failure path.
pub const POINT_DOMAIN: f64 = 8_388_607.0;

/// A rectangle in window points: the top-left corner and the size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointRect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

impl PointRect {
    /// A rectangle of these numbers (unchecked: the operation that uses it checks).
    #[must_use]
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The rectangle moved by `(dx, dy)`, added in `f64`.
    pub(crate) fn translated(self, dx: f64, dy: f64) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            ..self
        }
    }

    /// Checks that every number is finite, that `width` and `height` are greater than 0 and that all four edges (`x`, `x + width`, `y`,
    /// `y + height`, computed in `f64`) lie in the point domain.
    ///
    /// # Errors
    ///
    /// [`GroupError::NotFinite`], [`GroupError::BadSize`] or [`GroupError::OutsideDomain`], naming the first failing field or edge.
    pub(crate) fn check(self) -> Result<(), GroupError> {
        for (field, value) in [
            ("x", self.x),
            ("y", self.y),
            ("width", self.width),
            ("height", self.height),
        ] {
            if !value.is_finite() {
                return Err(GroupError::NotFinite { field });
            }
        }
        for (field, value) in [("width", self.width), ("height", self.height)] {
            if value <= 0.0 {
                return Err(GroupError::BadSize { field, value });
            }
        }
        self.check_edges()
    }

    /// Every edge lies in the point domain (the numbers are finite).
    pub(crate) fn check_edges(self) -> Result<(), GroupError> {
        for (field, value) in [
            ("left", self.x),
            ("right", self.x + self.width),
            ("top", self.y),
            ("bottom", self.y + self.height),
        ] {
            in_domain(field, value)?;
        }
        Ok(())
    }
}

/// `Ok` when `value` lies in the closed point domain.
pub(crate) fn in_domain(field: &'static str, value: f64) -> Result<(), GroupError> {
    if (-POINT_DOMAIN..=POINT_DOMAIN).contains(&value) {
        Ok(())
    } else {
        Err(GroupError::OutsideDomain { field, value })
    }
}

/// A generational handle for one group in one bridge's scene. The base group has no key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupKey {
    registry: RegistryId,
    slot: u32,
    generation: u64,
}

impl GroupKey {
    pub(crate) fn new(registry: RegistryId, slot: u32, generation: u64) -> Self {
        Self {
            registry,
            slot,
            generation,
        }
    }

    /// The registry this group belongs to.
    #[must_use]
    pub fn registry(self) -> RegistryId {
        self.registry
    }

    /// The slot within the scene's group table.
    #[must_use]
    pub fn slot(self) -> u32 {
        self.slot
    }

    /// The slot generation.
    #[must_use]
    pub fn generation(self) -> u64 {
        self.generation
    }

    /// Parses a group token. A texture or quad token is refused with [`HandleError::WrongKind`].
    ///
    /// # Errors
    ///
    /// The token parse error for malformed input, or `WrongKind` for a token that is not a group token.
    pub fn from_token(text: &str) -> Result<Self, HandleError> {
        let token: Token = text.parse().map_err(HandleError::Token)?;
        if token.kind != TokenKind::Group {
            return Err(HandleError::WrongKind {
                expected: TokenKind::Group,
                found: token.kind,
            });
        }
        Ok(Self::new(token.registry, token.slot, token.generation))
    }

    /// The process-local token for this group.
    #[must_use]
    pub fn token(self) -> Token {
        Token {
            kind: TokenKind::Group,
            registry: self.registry,
            slot: self.slot,
            generation: self.generation,
        }
    }
}

impl fmt::Display for GroupKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.token().fmt(f)
    }
}

/// Why a group operation was refused. A refusal changes nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum GroupError {
    /// A malformed token or a token of the wrong kind.
    Handle(HandleError),
    /// A key belongs to a different bridge.
    Foreign(GroupKey),
    /// A key is stale, destroyed, exhausted, or names the base group (which has no key).
    Stale(GroupKey),
    /// The scene has reached its configured group limit (`max_groups`).
    Full {
        /// The configured capacity.
        capacity: u32,
    },
    /// A number is NaN or infinite.
    NotFinite {
        /// Which one.
        field: &'static str,
    },
    /// An offset is not a whole number of points.
    NotIntegral {
        /// Which one.
        field: &'static str,
    },
    /// A size is not greater than 0.
    BadSize {
        /// Which one.
        field: &'static str,
        /// What it was.
        value: f64,
    },
    /// A number or an edge lies outside the point domain (±8,388,607 points).
    OutsideDomain {
        /// Which one.
        field: &'static str,
        /// What it was.
        value: f64,
    },
    /// The new offset would move an edge of one of the group's hit rectangles outside the point domain.
    HitOutsideDomain {
        /// The hit key.
        key: String,
        /// The edge.
        edge: &'static str,
    },
    /// A `raise_groups` list is empty or names a group twice.
    BadList {
        /// What is wrong with it.
        reason: &'static str,
    },
}

impl fmt::Display for GroupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handle(error) => error.fmt(f),
            Self::Foreign(key) => write!(f, "group {key} belongs to another scene"),
            Self::Stale(key) => write!(f, "group {key} is no longer live"),
            Self::Full { capacity } => {
                write!(f, "all {capacity} scene groups are in use (max_groups)")
            }
            Self::NotFinite { field } => write!(f, "{field} must be a finite number"),
            Self::NotIntegral { field } => write!(f, "{field} must be a whole number of points"),
            Self::BadSize { field, value } => {
                write!(f, "{field} {value} is not greater than 0")
            }
            Self::OutsideDomain { field, value } => {
                write!(f, "{field} {value} is outside ±{POINT_DOMAIN} points")
            }
            Self::HitOutsideDomain { key, edge } => write!(
                f,
                "the offset would move the {edge} edge of hit key `{key}` outside ±{POINT_DOMAIN} points"
            ),
            Self::BadList { reason } => write!(f, "the group list {reason}"),
        }
    }
}

impl std::error::Error for GroupError {}
