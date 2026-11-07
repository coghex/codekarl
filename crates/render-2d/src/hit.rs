//! Immutable, ordered pointer hit regions published with a scene.

use std::fmt;

use ck_input::POINTER_FRACTION;
use ck_window::PointerPosition;

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

    /// Whether `position` lies inside these half-open fixed-point edges.
    #[must_use]
    pub fn contains(self, position: PointerPosition) -> bool {
        self.left <= position.x
            && position.x < self.right
            && self.top <= position.y
            && position.y < self.bottom
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
#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, Default)]
pub(crate) struct HitEdits {
    keys: Vec<(String, Vec<HitRect>)>,
    count: usize,
}

impl HitEdits {
    pub(crate) fn set(&mut self, key: &str, rects: Option<Vec<HitRect>>) -> Result<bool, HitError> {
        if key.is_empty()
            || key.len() > 48
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(HitError::BadKey);
        }

        let current = self.keys.iter().position(|(existing, _)| existing == key);
        let Some(rects) = rects else {
            let Some(index) = current else {
                return Ok(false);
            };
            let (_, removed) = self.keys.remove(index);
            self.count -= removed.len();
            return Ok(true);
        };

        if !(1..=HIT_RECTS_PER_KEY).contains(&rects.len()) {
            return Err(HitError::BadRectCount { count: rects.len() });
        }
        let prior_count = current.map_or(0, |index| self.keys[index].1.len());
        let count = self.count - prior_count + rects.len();
        if count > HIT_RECTS_PER_SCENE {
            return Err(HitError::SceneFull { count });
        }

        self.count = count;
        if let Some(index) = current {
            self.keys[index].1 = rects;
        } else {
            self.keys.push((key.to_owned(), rects));
        }
        Ok(true)
    }

    pub(crate) fn publish(&self) -> HitRegions {
        HitRegions {
            regions: self
                .keys
                .iter()
                .flat_map(|(key, rects)| {
                    rects.iter().copied().map(|rect| HitRegion {
                        key: key.clone(),
                        rect,
                    })
                })
                .collect(),
        }
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

    #[test]
    fn keys_replace_in_place_and_remove_then_set_moves_to_the_top() {
        let mut edits = HitEdits::default();
        edits
            .set("lower", Some(vec![rect(0.0, 0.0, 10.0, 10.0)]))
            .unwrap();
        edits
            .set("upper", Some(vec![rect(0.0, 0.0, 10.0, 10.0)]))
            .unwrap();
        edits
            .set("lower", Some(vec![rect(0.0, 0.0, 10.0, 10.0)]))
            .unwrap();
        let position = PointerPosition { x: 1, y: 1 };
        assert_eq!(edits.publish().key_at(position), Some("upper"));
        edits.set("lower", None).unwrap();
        edits
            .set("lower", Some(vec![rect(0.0, 0.0, 10.0, 10.0)]))
            .unwrap();
        assert_eq!(edits.publish().key_at(position), Some("lower"));
    }

    #[test]
    fn refused_updates_leave_the_order_and_rectangles_unchanged() {
        let mut edits = HitEdits::default();
        edits
            .set("panel", Some(vec![rect(0.0, 0.0, 10.0, 10.0)]))
            .unwrap();
        let before = edits.publish();
        assert!(
            edits
                .set("Bad", Some(vec![rect(20.0, 20.0, 1.0, 1.0)]))
                .is_err()
        );
        assert!(edits.set("panel", Some(Vec::new())).is_err());
        assert!(
            edits
                .set(
                    "panel",
                    Some(vec![rect(0.0, 0.0, 1.0, 1.0); HIT_RECTS_PER_KEY + 1])
                )
                .is_err()
        );
        assert_eq!(edits.publish(), before);
    }

    #[test]
    fn the_scene_cap_is_atomic_and_replacements_make_room() {
        let mut edits = HitEdits::default();
        let rectangles = vec![rect(0.0, 0.0, 1.0, 1.0); HIT_RECTS_PER_KEY];
        for index in 0..HIT_RECTS_PER_SCENE / HIT_RECTS_PER_KEY {
            edits
                .set(&format!("key_{index:03}"), Some(rectangles.clone()))
                .unwrap();
        }
        let before = edits.publish();
        assert!(matches!(
            edits.set("extra", Some(vec![rect(0.0, 0.0, 1.0, 1.0)])),
            Err(HitError::SceneFull { .. })
        ));
        assert_eq!(edits.publish(), before);
        edits.set("key_000", None).unwrap();
        edits
            .set("extra", Some(vec![rect(0.0, 0.0, 1.0, 1.0)]))
            .unwrap();
        assert_eq!(
            edits.publish().regions().len(),
            HIT_RECTS_PER_SCENE - HIT_RECTS_PER_KEY + 1
        );
    }
}
