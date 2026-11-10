//! Persistent, ordered quads and immutable scene frames.
//!
//! The editable scene belongs to the single [`SceneWriter`]. Edits touch one slot and the order links; publication
//! alone walks the live scene. It builds a frame before taking the bridge lock, then exchanges one `Arc` while the
//! lock is held. A frame pins its textures independently of the public tokens.

use std::collections::{HashMap, VecDeque};
use std::fmt;
#[cfg(test)]
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use ck_resources::{HandleError, RegistryId, ResourceState, TextureRef, Token, TokenKind};

use crate::Tint;
use crate::group::{GroupError, GroupKey, PointRect, in_domain};
use crate::hit::{
    GroupHits, HIT_RECTS_PER_KEY, HIT_RECTS_PER_SCENE, HitError, HitRect, HitRegions, valid_key,
};
use crate::quad::{Metrics, PixelRect, QuadError, SourceRect, edges_at, trimmed, validate_rect};

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

/// The capacities of one scene: live quads and live groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneLimits {
    /// Maximum live quads, in every group together. Valid from 1 through 1,048,576.
    pub max_quads: u32,
    /// Maximum live groups, the base group not counted. Valid from 1 through 65,536.
    pub max_groups: u32,
}

impl SceneLimits {
    /// The default scene capacity.
    pub const DEFAULT: Self = Self {
        max_quads: 131_072,
        max_groups: 4_096,
    };
    const MAX: u32 = 1_048_576;
    const MAX_GROUPS: u32 = 65_536;

    /// Validates the configured capacities.
    ///
    /// # Errors
    ///
    /// Returns a [`SceneLimitsError`] naming `max_quads` when it is outside 1..=1,048,576, or `max_groups` when it is outside
    /// 1..=65,536.
    pub fn validate(self) -> Result<Self, SceneLimitsError> {
        if !(1..=Self::MAX).contains(&self.max_quads) {
            return Err(SceneLimitsError {
                field: "max_quads",
                value: u64::from(self.max_quads),
                min: 1,
                max: u64::from(Self::MAX),
            });
        }
        if !(1..=Self::MAX_GROUPS).contains(&self.max_groups) {
            return Err(SceneLimitsError {
                field: "max_groups",
                value: u64::from(self.max_groups),
                min: 1,
                max: u64::from(Self::MAX_GROUPS),
            });
        }
        Ok(self)
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
    /// The clip of the quad's group, in absolute window points (the group's offset does not move it), or `None`. The visible part of
    /// the quad is its destination intersected with the window and with this rectangle, both in pixels.
    pub clip: Option<PointRect>,
    /// Premultiplied per-quad tint, RGBA8.
    pub tint: Tint,
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
        let (scale_x, scale_y) = metrics.scene_scale();
        let (left, right) = edges_at(self.x, self.width, scale_x);
        let (top, bottom) = edges_at(self.y, self.height, scale_y);
        if right == left || bottom == top {
            return None;
        }
        let window_width = i32::try_from(metrics.pixels.width).unwrap_or(i32::MAX);
        let window_height = i32::try_from(metrics.pixels.height).unwrap_or(i32::MAX);
        let (mut visible_left, mut visible_right) = (left.max(0), right.min(window_width));
        let (mut visible_top, mut visible_bottom) = (top.max(0), bottom.min(window_height));
        if let Some(clip) = &self.clip {
            // The clip's pixel edges are rounded by the same rule as the quad's.
            let (clip_left, clip_right) = edges_at(clip.x, clip.width, scale_x);
            let (clip_top, clip_bottom) = edges_at(clip.y, clip.height, scale_y);
            visible_left = visible_left.max(clip_left);
            visible_right = visible_right.min(clip_right);
            visible_top = visible_top.max(clip_top);
            visible_bottom = visible_bottom.min(clip_bottom);
        }
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
    group_capacity: u32,
    slots: Vec<Slot>,
    free: VecDeque<u32>,
    live: u32,
    /// The group table. Index 0 is the base group: always live, always at the bottom, with no key.
    groups: Vec<GroupSlot>,
    group_free: VecDeque<u32>,
    /// The live group indices in painter order, bottom first; `order[0]` is the base group.
    order: Vec<u32>,
    live_groups: u32,
    /// The group each hit key was first set in.
    hit_owner: HashMap<String, u32>,
    /// Hit rectangles in the scene, as set.
    hit_count: usize,
}

#[derive(Debug)]
struct Slot {
    generation: u64,
    quad: Option<EditableQuad>,
    previous: Option<u32>,
    next: Option<u32>,
    /// The group the quad is in (an index into the group table).
    group: u32,
}

#[derive(Debug)]
struct GroupSlot {
    generation: u64,
    live: bool,
    visible: bool,
    offset: (f64, f64),
    clip: Option<PointRect>,
    /// The group's own quad order, bottom first.
    head: Option<u32>,
    tail: Option<u32>,
    hits: GroupHits,
}

impl GroupSlot {
    fn fresh(generation: u64) -> Self {
        Self {
            generation,
            live: true,
            visible: true,
            offset: (0.0, 0.0),
            clip: None,
            head: None,
            tail: None,
            hits: GroupHits::default(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct EditableQuad {
    texture: TextureRef<ResourceState>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    tint: Tint,
}

/// One quad of a frame being built, with what its group applies to it.
pub(crate) struct VisibleQuad<'a> {
    quad: &'a EditableQuad,
    offset: (f64, f64),
    clip: Option<PointRect>,
}

/// The quads that go into a frame, in painter order: group by group, bottom first, skipping hidden groups; within a group, in the
/// group's own creation order.
pub(crate) struct VisibleQuads<'a> {
    scene: &'a EditScene,
    next_group: usize,
    group: usize,
    current: Option<u32>,
}

impl<'a> Iterator for VisibleQuads<'a> {
    type Item = VisibleQuad<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(index) = self.current {
                let slot = &self.scene.slots[usize::try_from(index).unwrap_or(usize::MAX)];
                self.current = slot.next;
                let group = &self.scene.groups[self.group];
                return Some(VisibleQuad {
                    quad: slot.quad.as_ref().expect("order links name live slots"),
                    offset: group.offset,
                    clip: group.clip,
                });
            }
            let index =
                usize::try_from(*self.scene.order.get(self.next_group)?).unwrap_or(usize::MAX);
            self.next_group += 1;
            let group = &self.scene.groups[index];
            if group.visible {
                self.group = index;
                self.current = group.head;
            }
        }
    }
}

impl EditScene {
    pub(crate) fn new(registry: RegistryId, capacity: u32, group_capacity: u32) -> Self {
        Self {
            registry,
            capacity,
            group_capacity,
            slots: Vec::new(),
            free: VecDeque::new(),
            live: 0,
            groups: vec![GroupSlot::fresh(1)],
            group_free: VecDeque::new(),
            order: vec![0],
            live_groups: 0,
            hit_owner: HashMap::new(),
            hit_count: 0,
        }
    }

    pub(crate) fn len(&self) -> u32 {
        self.live
    }

    /// The live groups, the base group not counted.
    pub(crate) fn group_count(&self) -> u32 {
        self.live_groups
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

    /// The index of a live group named by `key`. The base group has no key, so slot 0 is never live to a caller.
    fn validate_group(&self, key: GroupKey) -> Result<usize, GroupError> {
        if key.registry() != self.registry {
            return Err(GroupError::Foreign(key));
        }
        let index = usize::try_from(key.slot()).unwrap_or(usize::MAX);
        match self.groups.get(index) {
            Some(group) if index != 0 && group.live && group.generation == key.generation() => {
                Ok(index)
            }
            _ => Err(GroupError::Stale(key)),
        }
    }

    pub(crate) fn create(
        &mut self,
        texture: TextureRef<ResourceState>,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        group: Option<GroupKey>,
    ) -> Result<QuadKey, QuadError> {
        validate_rect(x, y, width, height)?;
        let group = match group {
            Some(key) => self.validate_group(key).map_err(QuadError::Group)?,
            None => 0,
        };
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
                group: 0,
            });
            self.slots.len() - 1
        };
        let slot_id = u32::try_from(index).expect("scene capacity fits the key slot");
        let previous = self.groups[group].tail;
        self.slots[index].quad = Some(EditableQuad {
            texture,
            x,
            y,
            width,
            height,
            tint: Tint::WHITE,
        });
        self.slots[index].previous = previous;
        self.slots[index].next = None;
        self.slots[index].group = u32::try_from(group).expect("the group table fits u32");
        if let Some(previous) = previous {
            self.slots[usize::try_from(previous).unwrap_or(usize::MAX)].next = Some(slot_id);
        } else {
            self.groups[group].head = Some(slot_id);
        }
        self.groups[group].tail = Some(slot_id);
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

    pub(crate) fn set_tint(&mut self, key: QuadKey, tint: Tint) -> Result<(), QuadError> {
        let index = self.validate_key(key)?;
        self.slots[index]
            .quad
            .as_mut()
            .expect("validated live quad")
            .tint = tint;
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

    /// Takes the quad in slot `index` out of its group's order and frees the slot.
    fn remove_quad(&mut self, index: usize, slot_id: u32) -> EditableQuad {
        let group = usize::try_from(self.slots[index].group).unwrap_or(usize::MAX);
        let previous = self.slots[index].previous;
        let next = self.slots[index].next;
        if let Some(previous) = previous {
            self.slots[usize::try_from(previous).unwrap_or(usize::MAX)].next = next;
        } else {
            self.groups[group].head = next;
        }
        if let Some(next) = next {
            self.slots[usize::try_from(next).unwrap_or(usize::MAX)].previous = previous;
        } else {
            self.groups[group].tail = previous;
        }
        let slot = &mut self.slots[index];
        slot.previous = None;
        slot.next = None;
        let removed = slot.quad.take().expect("a live quad");
        self.live -= 1;
        if slot.generation != u64::MAX {
            slot.generation += 1;
            self.free.push_back(slot_id);
        }
        removed
    }

    pub(crate) fn destroy(&mut self, key: QuadKey) -> Result<EditableQuad, QuadError> {
        let index = self.validate_key(key)?;
        Ok(self.remove_quad(index, key.slot))
    }

    // ----- groups -----

    /// Creates an empty, visible group with offset (0, 0) and no clip, on top of all groups.
    pub(crate) fn create_group(&mut self) -> Result<GroupKey, GroupError> {
        if self.live_groups >= self.group_capacity {
            return Err(GroupError::Full {
                capacity: self.group_capacity,
            });
        }
        let index = if let Some(index) = self.group_free.pop_front() {
            let index = usize::try_from(index).unwrap_or(usize::MAX);
            let generation = self.groups[index].generation;
            self.groups[index] = GroupSlot::fresh(generation);
            index
        } else {
            self.groups.try_reserve(1).map_err(|_| GroupError::Full {
                capacity: self.group_capacity,
            })?;
            self.groups.push(GroupSlot::fresh(1));
            self.groups.len() - 1
        };
        let slot = u32::try_from(index).expect("the group capacity fits the key slot");
        self.order.push(slot);
        self.live_groups += 1;
        Ok(GroupKey::new(
            self.registry,
            slot,
            self.groups[index].generation,
        ))
    }

    fn position_of(&self, index: usize) -> usize {
        self.order
            .iter()
            .position(|&entry| usize::try_from(entry).unwrap_or(usize::MAX) == index)
            .expect("a live group is in the order")
    }

    fn slot_of(index: usize) -> u32 {
        u32::try_from(index).expect("the group table fits u32")
    }

    pub(crate) fn raise_group(&mut self, key: GroupKey) -> Result<(), GroupError> {
        let index = self.validate_group(key)?;
        let position = self.position_of(index);
        let entry = self.order.remove(position);
        self.order.push(entry);
        Ok(())
    }

    /// Puts the group just above the base group.
    pub(crate) fn lower_group(&mut self, key: GroupKey) -> Result<(), GroupError> {
        let index = self.validate_group(key)?;
        let position = self.position_of(index);
        let entry = self.order.remove(position);
        self.order.insert(1, entry);
        Ok(())
    }

    /// Puts the listed groups on top in that order (the last topmost), as one edit. Refused, changing nothing, if the list is
    /// empty, names a group twice or names a group that is not live.
    pub(crate) fn raise_groups(&mut self, keys: &[GroupKey]) -> Result<(), GroupError> {
        if keys.is_empty() {
            return Err(GroupError::BadList { reason: "is empty" });
        }
        let mut indices = Vec::with_capacity(keys.len());
        for key in keys {
            indices.push(self.validate_group(*key)?);
        }
        let mut seen = indices.clone();
        seen.sort_unstable();
        if seen.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(GroupError::BadList {
                reason: "names a group twice",
            });
        }
        self.order
            .retain(|&entry| !indices.contains(&usize::try_from(entry).unwrap_or(usize::MAX)));
        self.order.extend(indices.into_iter().map(Self::slot_of));
        Ok(())
    }

    pub(crate) fn set_group_visible(
        &mut self,
        key: GroupKey,
        visible: bool,
    ) -> Result<(), GroupError> {
        let index = self.validate_group(key)?;
        self.groups[index].visible = visible;
        Ok(())
    }

    /// Sets the group's offset, in whole points inside the point domain. Refused, changing nothing, if a hit rectangle of the group
    /// would leave the domain under it.
    pub(crate) fn set_group_offset(
        &mut self,
        key: GroupKey,
        x: f64,
        y: f64,
    ) -> Result<(), GroupError> {
        let index = self.validate_group(key)?;
        for (field, value) in [("x", x), ("y", y)] {
            if !value.is_finite() {
                return Err(GroupError::NotFinite { field });
            }
            if value.fract() != 0.0 {
                return Err(GroupError::NotIntegral { field });
            }
            in_domain(field, value)?;
        }
        self.groups[index].hits.check_offset(x, y)?;
        self.groups[index].offset = (x, y);
        Ok(())
    }

    /// Sets or removes the group's clip, in absolute window points.
    pub(crate) fn set_group_clip(
        &mut self,
        key: GroupKey,
        clip: Option<PointRect>,
    ) -> Result<(), GroupError> {
        let index = self.validate_group(key)?;
        if let Some(clip) = clip {
            clip.check()?;
        }
        self.groups[index].clip = clip;
        Ok(())
    }

    /// Destroys the group, its quads (returned, to be dropped by the caller) and its hit keys.
    pub(crate) fn destroy_group(&mut self, key: GroupKey) -> Result<Vec<EditableQuad>, GroupError> {
        let index = self.validate_group(key)?;
        let mut removed = Vec::new();
        while let Some(head) = self.groups[index].head {
            removed.push(self.remove_quad(usize::try_from(head).unwrap_or(usize::MAX), head));
        }
        self.hit_count -= self.groups[index].hits.rect_count();
        for name in self.groups[index].hits.names() {
            self.hit_owner.remove(name);
        }
        let generation = self.groups[index].generation;
        self.groups[index] = GroupSlot::fresh(generation);
        self.groups[index].live = false;
        if generation != u64::MAX {
            self.groups[index].generation = generation + 1;
            self.group_free.push_back(key.slot());
        }
        let position = self.position_of(index);
        self.order.remove(position);
        self.live_groups -= 1;
        Ok(removed)
    }

    // ----- hit regions -----

    /// Replaces or removes one hit key. `group` is the group the key belongs to (the base group if none); a key keeps the group it was
    /// first set in. Everything is validated before anything changes; `Ok(true)` when something changed.
    pub(crate) fn set_hit(
        &mut self,
        key: &str,
        rects: Option<Vec<PointRect>>,
        group: Option<GroupKey>,
    ) -> Result<bool, HitError> {
        if !valid_key(key) {
            return Err(HitError::BadKey);
        }
        let named = match group {
            Some(group) => Some(self.validate_group(group).map_err(HitError::Group)?),
            None => None,
        };
        let owner = self.hit_owner.get(key).copied();
        let owner = owner.map(|owner| usize::try_from(owner).unwrap_or(usize::MAX));
        let Some(rects) = rects else {
            // Removing needs no group; a group that is named must be the key's own.
            let Some(owner) = owner else {
                return Ok(false);
            };
            if named.is_some_and(|named| named != owner) {
                return Err(HitError::GroupMismatch);
            }
            self.hit_count -= self.groups[owner].hits.remove(key);
            self.hit_owner.remove(key);
            return Ok(true);
        };
        let target = named.unwrap_or(0);
        if owner.is_some_and(|owner| owner != target) {
            return Err(HitError::GroupMismatch);
        }
        if !(1..=HIT_RECTS_PER_KEY).contains(&rects.len()) {
            return Err(HitError::BadRectCount { count: rects.len() });
        }
        let (dx, dy) = self.groups[target].offset;
        for rect in &rects {
            rect.check().map_err(HitError::Group)?;
            // The rectangle must keep an extent in pointer units as set, as before groups existed.
            HitRect::from_points(rect.x, rect.y, rect.width, rect.height)?;
            rect.translated(dx, dy)
                .check_edges()
                .map_err(HitError::Group)?;
        }
        let prior = self.groups[target]
            .hits
            .rects(key)
            .map_or(0, <[PointRect]>::len);
        let count = self.hit_count - prior + rects.len();
        if count > HIT_RECTS_PER_SCENE {
            return Err(HitError::SceneFull { count });
        }
        self.hit_count = count;
        self.groups[target].hits.replace(key, rects);
        self.hit_owner.insert(key.to_owned(), Self::slot_of(target));
        Ok(true)
    }

    /// The hit regions to publish: group by group in painter order, a hidden group publishing none.
    pub(crate) fn publish_hits(&self) -> HitRegions {
        let mut regions = Vec::with_capacity(self.hit_count);
        for &index in &self.order {
            let group = &self.groups[usize::try_from(index).unwrap_or(usize::MAX)];
            if group.visible {
                group
                    .hits
                    .publish_into(&mut regions, group.offset.0, group.offset.1, group.clip);
            }
        }
        HitRegions::from_regions(regions)
    }

    // ----- frames -----

    /// The quads of the next frame, in painter order.
    pub(crate) fn visible_quads(&self) -> VisibleQuads<'_> {
        VisibleQuads {
            scene: self,
            next_group: 0,
            group: 0,
            current: None,
        }
    }

    pub(crate) fn build(&self, target: &mut Vec<FrameQuad>) {
        self.build_from(self.visible_quads(), target);
    }

    /// Builds the frame from `quads` (the walk of [`EditScene::visible_quads`], or a test's wrapper around it): each quad is
    /// translated by its group's offset and keeps its group's clip.
    pub(crate) fn build_from<'a>(
        &self,
        quads: impl Iterator<Item = VisibleQuad<'a>>,
        target: &mut Vec<FrameQuad>,
    ) {
        target.clear();
        target.reserve(usize::try_from(self.live).unwrap_or(usize::MAX));
        for visible in quads {
            target.push(FrameQuad {
                texture: visible.quad.texture.clone(),
                x: visible.quad.x + visible.offset.0,
                y: visible.quad.y + visible.offset.1,
                width: visible.quad.width,
                height: visible.quad.height,
                clip: visible.clip,
                tint: visible.quad.tint,
            });
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
