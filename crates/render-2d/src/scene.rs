//! Persistent, ordered quads and immutable scene frames.
//!
//! The editable scene belongs to the single [`SceneWriter`]. Edits touch one slot and the order links; publication
//! alone walks the live scene. It builds a frame before taking the bridge lock, then exchanges one `Arc` while the
//! lock is held. A frame pins its textures independently of the public tokens.

use std::collections::VecDeque;
use std::fmt;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use ck_resources::{HandleError, RegistryId, ResourceState, TextureRef, Token, TokenKind};

use crate::quad::{Metrics, PixelRect, QuadError, SourceRect, edges, trimmed, validate_rect};

/// A generational handle for one persistent quad in one bridge's scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QuadKey {
    registry: RegistryId,
    slot: u32,
    generation: u64,
}

impl QuadKey {
    fn new(registry: RegistryId, slot: u32, generation: u64) -> Self {
        Self {
            registry,
            slot,
            generation,
        }
    }

    /// The registry this quad belongs to.
    #[must_use]
    pub fn registry(self) -> RegistryId {
        self.registry
    }

    /// The slot within the bridge's scene registry.
    #[must_use]
    pub fn slot(self) -> u32 {
        self.slot
    }

    /// The slot generation.
    #[must_use]
    pub fn generation(self) -> u64 {
        self.generation
    }

    /// Parses a quad token. Texture tokens are refused with [`HandleError::WrongKind`].
    ///
    /// # Errors
    ///
    /// Returns the token parse error for malformed input or `WrongKind` for a non-quad token.
    pub fn from_token(text: &str) -> Result<Self, HandleError> {
        let token: Token = text.parse().map_err(HandleError::Token)?;
        if token.kind != TokenKind::Quad {
            return Err(HandleError::WrongKind {
                expected: TokenKind::Quad,
                found: token.kind,
            });
        }
        Ok(Self::new(token.registry, token.slot, token.generation))
    }

    /// The process-local token for this quad.
    #[must_use]
    pub fn token(self) -> Token {
        Token {
            kind: TokenKind::Quad,
            registry: self.registry,
            slot: self.slot,
            generation: self.generation,
        }
    }
}

impl fmt::Display for QuadKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.token().fmt(f)
    }
}

/// The maximum number of live quads in one scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneLimits {
    /// Maximum live quads. Valid from 1 through 1,048,576.
    pub max_quads: u32,
}

impl SceneLimits {
    /// The default scene capacity.
    pub const DEFAULT: Self = Self { max_quads: 131_072 };
    const MAX: u32 = 1_048_576;

    /// Validates the configured capacity.
    ///
    /// # Errors
    ///
    /// Returns a [`SceneLimitsError`] naming `max_quads` when it is outside 1..=1,048,576.
    pub fn validate(self) -> Result<Self, SceneLimitsError> {
        if (1..=Self::MAX).contains(&self.max_quads) {
            Ok(self)
        } else {
            Err(SceneLimitsError {
                field: "max_quads",
                value: u64::from(self.max_quads),
                min: 1,
                max: u64::from(Self::MAX),
            })
        }
    }
}

impl Default for SceneLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A scene capacity is outside its supported range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneLimitsError {
    /// The invalid field.
    pub field: &'static str,
    /// The supplied value.
    pub value: u64,
    /// The inclusive minimum.
    pub min: u64,
    /// The inclusive maximum.
    pub max: u64,
}

impl fmt::Display for SceneLimitsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "render.{} is {}, outside {} to {}",
            self.field, self.value, self.min, self.max
        )
    }
}
impl std::error::Error for SceneLimitsError {}

/// A quad in immutable painter order. It owns a retained texture reference and a point-space rectangle.
#[derive(Debug, Clone)]
pub struct FrameQuad {
    /// The retained texture reference. It is independent of the public texture token.
    pub texture: TextureRef<ResourceState>,
    /// Left edge in window points.
    pub x: f64,
    /// Top edge in window points.
    pub y: f64,
    /// Width in window points.
    pub width: f64,
    /// Height in window points.
    pub height: f64,
}

/// A placed frame quad. Source coordinates are normalized fractions of the active texture.
#[derive(Debug, Clone)]
pub struct PlacedFrameQuad {
    /// The retained texture reference.
    pub texture: TextureRef<ResourceState>,
    /// Visible destination rectangle in pixels.
    pub destination: PixelRect,
    /// Visible source rectangle as fractions of the texture (each edge is in 0.0..=1.0).
    pub source: SourceRect,
}

/// Where a frame quad lands: the visible destination in pixels and the visible source as fractions of the texture. It holds no texture
/// reference, so computing one takes and drops none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Placement {
    /// Visible destination rectangle in pixels.
    pub(crate) destination: PixelRect,
    /// Visible source rectangle as fractions of the texture.
    pub(crate) source: SourceRect,
}

impl FrameQuad {
    /// Places the quad using the same rounded edges and clipping as [`crate::Quad::place`]. Source coordinates
    /// remain normalized because a later texture revision may have different pixel dimensions.
    #[must_use]
    pub fn place(&self, metrics: &Metrics) -> Option<PlacedFrameQuad> {
        let Placement {
            destination,
            source,
        } = self.placement(metrics)?;
        Some(PlacedFrameQuad {
            texture: self.texture.clone(),
            destination,
            source,
        })
    }

    /// The same placement as [`FrameQuad::place`], without the texture reference: the frame already holds the reference for as long as
    /// the packet needs it, and a clone and drop of it per instance would be atomic traffic on the registry's shared count.
    #[must_use]
    pub(crate) fn placement(&self, metrics: &Metrics) -> Option<Placement> {
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
        let (source_left, source_right) = trimmed(1, left, right, visible_left, visible_right);
        let (source_top, source_bottom) = trimmed(1, top, bottom, visible_top, visible_bottom);
        if !(source_left.is_finite()
            && source_top.is_finite()
            && source_right.is_finite()
            && source_bottom.is_finite()
            && source_left >= 0.0
            && source_top >= 0.0
            && source_right <= 1.0
            && source_bottom <= 1.0
            && source_right > source_left
            && source_bottom > source_top)
        {
            return None;
        }
        Some(Placement {
            destination: PixelRect {
                left: visible_left,
                top: visible_top,
                right: visible_right,
                bottom: visible_bottom,
            },
            source: SourceRect {
                left: source_left,
                top: source_top,
                right: source_right,
                bottom: source_bottom,
            },
        })
    }
}

/// An immutable publication of all live quads, in painter order.
///
/// The compatibility [`crate::Snapshot::quad`] lane is drawn below every quad in this frame. Updating that legacy
/// quad never reorders this frame.
#[derive(Debug)]
pub struct SceneFrame {
    number: u64,
    compat: Option<FrameQuad>,
    quads: Vec<FrameQuad>,
    #[cfg(test)]
    _tracker: FrameTracker,
}

/// What a test's frame observer runs when a `SceneFrame` is dropped (to ask, for instance, whether the bridge lock is held).
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct FrameDropHook(pub(crate) Arc<dyn Fn() + Send + Sync>);

#[cfg(test)]
impl fmt::Debug for FrameDropHook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FrameDropHook(..)")
    }
}

#[cfg(test)]
pub(crate) struct FrameTracker(Arc<AtomicUsize>, Option<FrameDropHook>);

#[cfg(test)]
impl fmt::Debug for FrameTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FrameTracker")
    }
}

#[cfg(test)]
impl FrameTracker {
    fn new(counter: Arc<AtomicUsize>, hook: Option<FrameDropHook>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter, hook)
    }
}

#[cfg(test)]
impl Drop for FrameTracker {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
        if let Some(hook) = &self.1 {
            (hook.0)();
        }
    }
}

impl SceneFrame {
    pub(crate) fn new(
        number: u64,
        compat: Option<FrameQuad>,
        quads: Vec<FrameQuad>,
        #[cfg(test)] frame_counter: Arc<AtomicUsize>,
        #[cfg(test)] drop_hook: Option<FrameDropHook>,
    ) -> Self {
        Self {
            number,
            compat,
            quads,
            #[cfg(test)]
            _tracker: FrameTracker::new(frame_counter, drop_hook),
        }
    }

    /// The monotonically increasing publication number.
    #[must_use]
    pub fn number(&self) -> u64 {
        self.number
    }

    /// The quads, in stable painter order.
    #[must_use]
    pub fn quads(&self) -> &[FrameQuad] {
        &self.quads
    }

    /// The compatibility quad, drawn below every persistent quad in this frame.
    #[must_use]
    pub fn compat(&self) -> Option<&FrameQuad> {
        self.compat.as_ref()
    }

    pub(crate) fn into_parts(self) -> (Option<FrameQuad>, Vec<FrameQuad>) {
        (self.compat, self.quads)
    }
}

#[derive(Debug)]
pub(crate) struct EditScene {
    registry: RegistryId,
    capacity: u32,
    slots: Vec<Slot>,
    order_head: Option<u32>,
    order_tail: Option<u32>,
    free: VecDeque<u32>,
    live: u32,
}

#[derive(Debug)]
struct Slot {
    generation: u64,
    quad: Option<EditableQuad>,
    previous: Option<u32>,
    next: Option<u32>,
}

#[derive(Debug)]
pub(crate) struct EditableQuad {
    texture: TextureRef<ResourceState>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl EditScene {
    pub(crate) fn new(registry: RegistryId, capacity: u32) -> Self {
        Self {
            registry,
            capacity,
            slots: Vec::new(),
            order_head: None,
            order_tail: None,
            free: VecDeque::new(),
            live: 0,
        }
    }

    pub(crate) fn len(&self) -> u32 {
        self.live
    }

    fn validate_key(&self, key: QuadKey) -> Result<usize, QuadError> {
        if key.registry != self.registry {
            return Err(QuadError::Foreign(key));
        }
        let index = usize::try_from(key.slot).unwrap_or(usize::MAX);
        match self.slots.get(index) {
            Some(slot) if slot.generation == key.generation && slot.quad.is_some() => Ok(index),
            _ => Err(QuadError::Stale(key)),
        }
    }

    pub(crate) fn create(
        &mut self,
        texture: TextureRef<ResourceState>,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<QuadKey, QuadError> {
        validate_rect(x, y, width, height)?;
        if self.live >= self.capacity {
            return Err(QuadError::Full {
                capacity: self.capacity,
            });
        }
        let index = if let Some(index) = self.free.pop_front() {
            usize::try_from(index).unwrap_or(usize::MAX)
        } else {
            self.slots.try_reserve(1).map_err(|_| QuadError::Full {
                capacity: self.capacity,
            })?;
            self.slots.push(Slot {
                generation: 1,
                quad: None,
                previous: None,
                next: None,
            });
            self.slots.len() - 1
        };
        let slot_id = u32::try_from(index).expect("scene capacity fits the key slot");
        let previous = self.order_tail;
        self.slots[index].quad = Some(EditableQuad {
            texture,
            x,
            y,
            width,
            height,
        });
        self.slots[index].previous = previous;
        self.slots[index].next = None;
        if let Some(previous) = previous {
            self.slots[usize::try_from(previous).unwrap_or(usize::MAX)].next = Some(slot_id);
        } else {
            self.order_head = Some(slot_id);
        }
        self.order_tail = Some(slot_id);
        self.live += 1;
        Ok(QuadKey::new(
            self.registry,
            slot_id,
            self.slots[index].generation,
        ))
    }

    pub(crate) fn set_position(&mut self, key: QuadKey, x: f64, y: f64) -> Result<(), QuadError> {
        validate_rect(x, y, 1.0, 1.0)?;
        let index = self.validate_key(key)?;
        let quad = self.slots[index]
            .quad
            .as_mut()
            .expect("validated live quad");
        quad.x = x;
        quad.y = y;
        Ok(())
    }

    pub(crate) fn set_rect(
        &mut self,
        key: QuadKey,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<(), QuadError> {
        validate_rect(x, y, width, height)?;
        let index = self.validate_key(key)?;
        let quad = self.slots[index]
            .quad
            .as_mut()
            .expect("validated live quad");
        quad.x = x;
        quad.y = y;
        quad.width = width;
        quad.height = height;
        Ok(())
    }

    pub(crate) fn destroy(&mut self, key: QuadKey) -> Result<EditableQuad, QuadError> {
        let index = self.validate_key(key)?;
        let previous = self.slots[index].previous;
        let next = self.slots[index].next;
        if let Some(previous) = previous {
            self.slots[usize::try_from(previous).unwrap_or(usize::MAX)].next = next;
        } else {
            self.order_head = next;
        }
        if let Some(next) = next {
            self.slots[usize::try_from(next).unwrap_or(usize::MAX)].previous = previous;
        } else {
            self.order_tail = previous;
        }
        let slot = &mut self.slots[index];
        slot.previous = None;
        slot.next = None;
        let removed = slot.quad.take().expect("validated live quad");
        self.live -= 1;
        if slot.generation != u64::MAX {
            slot.generation += 1;
            self.free.push_back(key.slot);
        }
        Ok(removed)
    }

    pub(crate) fn build(&self, target: &mut Vec<FrameQuad>) {
        target.clear();
        target.reserve(usize::try_from(self.live).unwrap_or(usize::MAX));
        let mut current = self.order_head;
        while let Some(index) = current {
            let slot = &self.slots[usize::try_from(index).unwrap_or(usize::MAX)];
            let quad = slot.quad.as_ref().expect("order links name live slots");
            target.push(FrameQuad {
                texture: quad.texture.clone(),
                x: quad.x,
                y: quad.y,
                width: quad.width,
                height: quad.height,
            });
            current = slot.next;
        }
    }
}

#[cfg(test)]
impl EditScene {
    pub(crate) fn force_generation(&mut self, slot: u32, generation: u64) -> QuadKey {
        let entry = &mut self.slots[usize::try_from(slot).unwrap_or(usize::MAX)];
        entry.generation = generation;
        QuadKey::new(self.registry, slot, generation)
    }
}
