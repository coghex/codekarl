//! Immutable, ordered pointer hit regions published with a scene.

use std::fmt;

use ck_input::POINTER_FRACTION;
use ck_window::PointerPosition;

use crate::group::{GroupError, PointRect};

/// The maximum rectangles registered under one key.
pub const HIT_RECTS_PER_KEY: usize = 16;
/// The maximum rectangles in a published scene.
pub const HIT_RECTS_PER_SCENE: usize = 4096;

/// One half-open hit rectangle, quantized in the same fixed-point units as pointer positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HitRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl HitRect {
    /// Converts a point-space rectangle to pointer fixed-point edges.
    ///
    /// # Errors
    ///
    /// Returns [`HitError::EdgeOutOfRange`] when a scaled edge is not a finite `i32`, and
    /// [`HitError::Empty`] when rounding leaves no positive extent.
    pub fn from_points(x: f64, y: f64, width: f64, height: f64) -> Result<Self, HitError> {
        let left = edge(x, "left")?;
        let top = edge(y, "top")?;
        // The contract adds in f64 before scaling the far edge.
        let right = edge(x + width, "right")?;
        let bottom = edge(y + height, "bottom")?;
        if right <= left || bottom <= top {
            return Err(HitError::Empty);
        }
        Ok(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    /// The rectangle of these fixed-point edges, or `None` if it has no extent.
    fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Option<Self> {
        (right > left && bottom > top).then_some(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    /// The fixed-point edges of a point rectangle, by the rule of [`HitRect::from_points`] (the far edge is added in `f64` before it
    /// is scaled), with no check for extent. Total: a value is clamped to `i32`, which the point domain never reaches.
    fn quantized(rect: PointRect) -> (i32, i32, i32, i32) {
        (
            quantize(rect.x),
            quantize(rect.y),
            quantize(rect.x + rect.width),
            quantize(rect.y + rect.height),
        )
    }

    /// The part of this rectangle inside `other`, or `None` if nothing is left.
    fn intersect(self, other: Self) -> Option<Self> {
        Self::from_edges(
            self.left.max(other.left),
            self.top.max(other.top),
            self.right.min(other.right),
            self.bottom.min(other.bottom),
        )
    }

    /// Whether `position` lies inside these half-open fixed-point edges.
    #[must_use]
    pub fn contains(self, position: PointerPosition) -> bool {
        self.left <= position.x
            && position.x < self.right
            && self.top <= position.y
            && position.y < self.bottom
    }
}

/// `value` points in pointer units (×256, rounded), clamped to `i32` so that the conversion has no failure path.
fn quantize(value: f64) -> i32 {
    let scaled = (value * f64::from(POINTER_FRACTION))
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX));
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value was clamped into i32's range just before"
    )]
    {
        scaled as i32
    }
}

fn edge(value: f64, name: &'static str) -> Result<i32, HitError> {
    let scaled = (value * f64::from(POINTER_FRACTION)).round();
    if !scaled.is_finite() {
        return Err(HitError::EdgeOutOfRange { edge: name });
    }
    if scaled < f64::from(i32::MIN) || scaled > f64::from(i32::MAX) {
        return Err(HitError::EdgeOutOfRange { edge: name });
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the rounded fixed-point edge was range-checked against i32"
    )]
    Ok(scaled as i32)
}

/// Why a hit-region update is invalid.
#[derive(Clone, Debug, PartialEq)]
pub enum HitError {
    /// A key does not follow `[a-z0-9_]{1,48}`.
    BadKey,
    /// The key's rectangle list is empty or has more than 16 entries.
    BadRectCount {
        /// Number of rectangles supplied.
        count: usize,
    },
    /// A scaled edge is non-finite or outside the pointer's i32 range.
    EdgeOutOfRange {
        /// The edge that could not be quantized.
        edge: &'static str,
    },
    /// Rounding leaves no integer extent.
    Empty,
    /// The update would exceed the scene's 4096-rectangle limit.
    SceneFull {
        /// Resulting scene rectangle count.
        count: usize,
    },
    /// The key already belongs to another group: a key keeps the group it was first set in.
    GroupMismatch,
    /// The group is not usable, or a rectangle's numbers or edges are not valid (finite, a positive size, inside the point domain as
    /// set and as translated by the group's offset).
    Group(GroupError),
}

impl fmt::Display for HitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadKey => formatter.write_str("key must match [a-z0-9_]{1,48}"),
            Self::BadRectCount { count } => write!(
                formatter,
                "rectangles has {count} entries; expected 1 to {HIT_RECTS_PER_KEY}"
            ),
            Self::EdgeOutOfRange { edge } => {
                write!(
                    formatter,
                    "{edge} edge is not a finite i32 pointer coordinate"
                )
            }
            Self::Empty => formatter.write_str("rectangle is empty after pointer rounding"),
            Self::SceneFull { count } => write!(
                formatter,
                "scene would have {count} hit rectangles; limit is {HIT_RECTS_PER_SCENE}"
            ),
            Self::GroupMismatch => formatter.write_str(
                "the key belongs to another group; a key keeps the group it was first set in",
            ),
            Self::Group(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for HitError {}

/// A flattened rectangle with its owning key; later entries are hit first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitRegion {
    key: String,
    rect: HitRect,
}

impl HitRegion {
    /// The Lua key that owns pointer events in this rectangle.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The fixed-point rectangle.
    #[must_use]
    pub fn rect(&self) -> HitRect {
        self.rect
    }
}

/// An immutable flat list of hit regions, published with one scene callback.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HitRegions {
    regions: Vec<HitRegion>,
}

impl HitRegions {
    /// The regions in painter-independent hit order, topmost last.
    #[must_use]
    pub fn regions(&self) -> &[HitRegion] {
        &self.regions
    }

    /// The topmost key containing `position`.
    #[must_use]
    pub fn key_at(&self, position: PointerPosition) -> Option<&str> {
        self.regions
            .iter()
            .rev()
            .find(|region| region.rect.contains(position))
            .map(HitRegion::key)
    }

    /// Whether the ordered region list still contains `key`.
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.regions.iter().any(|region| region.key == key)
    }
}

/// The hit keys of one group, in the order they were first set (later on top).
#[derive(Clone, Debug, Default)]
pub(crate) struct GroupHits {
    keys: Vec<(String, Vec<PointRect>)>,
}

/// Whether `key` follows `[a-z0-9_]{1,48}`.
pub(crate) fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 48
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

impl GroupHits {
    /// How many rectangles the keys hold, as set.
    pub(crate) fn rect_count(&self) -> usize {
        self.keys.iter().map(|(_, rects)| rects.len()).sum()
    }

    /// The rectangles of `key`, if it is set here.
    pub(crate) fn rects(&self, key: &str) -> Option<&[PointRect]> {
        self.keys
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, rects)| rects.as_slice())
    }

    /// Replaces `key`'s rectangles in place, or adds it on top.
    pub(crate) fn replace(&mut self, key: &str, rects: Vec<PointRect>) {
        if let Some((_, current)) = self.keys.iter_mut().find(|(existing, _)| existing == key) {
            *current = rects;
        } else {
            self.keys.push((key.to_owned(), rects));
        }
    }

    /// Removes `key`; the number of rectangles it held (0 if it was not set).
    pub(crate) fn remove(&mut self, key: &str) -> usize {
        self.keys
            .iter()
            .position(|(existing, _)| existing == key)
            .map_or(0, |index| self.keys.remove(index).1.len())
    }

    /// The names of the keys, in order.
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.keys.iter().map(|(key, _)| key.as_str())
    }

    /// `Ok` when every edge of every rectangle, translated by `(dx, dy)`, lies in the point domain.
    pub(crate) fn check_offset(&self, dx: f64, dy: f64) -> Result<(), GroupError> {
        for (key, rects) in &self.keys {
            for rect in rects {
                rect.translated(dx, dy)
                    .check_edges()
                    .map_err(|error| match error {
                        GroupError::OutsideDomain { field, .. } => GroupError::HitOutsideDomain {
                            key: key.clone(),
                            edge: field,
                        },
                        other => other,
                    })?;
            }
        }
        Ok(())
    }

    /// Appends this group's regions: each rectangle translated by `(dx, dy)` in `f64` points, converted to pointer units, intersected in
    /// those units with `clip` (converted the same way); an empty result is dropped.
    pub(crate) fn publish_into(
        &self,
        regions: &mut Vec<HitRegion>,
        dx: f64,
        dy: f64,
        clip: Option<PointRect>,
    ) {
        // `None`: no clip. `Some(None)`: a clip with no extent in pointer units, which clips everything away.
        let clip = clip.map(|clip| {
            let (left, top, right, bottom) = HitRect::quantized(clip);
            HitRect::from_edges(left, top, right, bottom)
        });
        for (key, rects) in &self.keys {
            for rect in rects {
                let (left, top, right, bottom) = HitRect::quantized(rect.translated(dx, dy));
                let Some(mut placed) = HitRect::from_edges(left, top, right, bottom) else {
                    continue;
                };
                match clip {
                    None => {}
                    Some(None) => continue,
                    Some(Some(clip)) => {
                        let Some(inside) = placed.intersect(clip) else {
                            continue;
                        };
                        placed = inside;
                    }
                }
                regions.push(HitRegion {
                    key: key.clone(),
                    rect: placed,
                });
            }
        }
    }
}

impl HitRegions {
    pub(crate) fn from_regions(regions: Vec<HitRegion>) -> Self {
        Self { regions }
    }

    /// Whether `position` lies inside one of `key`'s published rectangles.
    #[must_use]
    pub fn key_contains(&self, key: &str, position: PointerPosition) -> bool {
        self.regions
            .iter()
            .any(|region| region.key == key && region.rect.contains(position))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> HitRect {
        HitRect::from_points(x, y, width, height).expect("valid rectangle")
    }

    #[test]
    fn pointer_and_hit_edges_share_rounding_and_half_open_containment() {
        let hit = rect(0.001, 2.0, 4.0, 3.0);
        assert!(hit.contains(PointerPosition { x: 0, y: 512 }));
        assert!(hit.contains(PointerPosition { x: 1, y: 512 }));
        assert!(!hit.contains(PointerPosition { x: 1024, y: 512 }));
        assert!(!hit.contains(PointerPosition { x: 0, y: 1280 }));
        assert!(HitRect::from_points(f64::NAN, 0.0, 1.0, 1.0).is_err());
        assert!(HitRect::from_points(f64::MAX, 0.0, 1.0, 1.0).is_err());
        assert_eq!(
            HitRect::from_points(0.0, 0.0, 0.001, 1.0),
            Err(HitError::Empty)
        );
    }
}
