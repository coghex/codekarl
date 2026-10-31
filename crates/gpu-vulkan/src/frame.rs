//! What a scene presenter is given each frame (milestone 5.5, issue 2a; the Metal types of the same names are the contract): plain data and a latest-value slot, like the
//! legacy quad's ([`VulkanQuadSlot`](crate::VulkanQuadSlot)).
//!
//! - [`VulkanInstance`] is one quad of the scene: the logical key of its texture, the destination in whole drawable
//!   pixels and the visible part of the texture as **fractions of it** (so a reload with new dimensions samples the
//!   whole new image).
//! - [`VulkanSceneSource`] emits a scene's visible quads in painter order. It is called **on the render thread at frame
//!   build**, so placement in pixels never runs under the publisher's lock.
//! - [`VulkanFrame`] is what is published: the generation, the filter for the whole frame, the legacy quad (under every
//!   scene quad) and the scene source.
//! - [`VulkanFrameSlot`] is the latest-value slot: [`publish`](VulkanFrameSlot::publish) swaps the frame in under a lock
//!   held only for the swap, drops the replaced frame after the lock is released, and wakes the render thread.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::quad::{DestinationRect, VulkanFilter, VulkanQuad};
use crate::slot::SlotWake;

/// What a quad is for. A scene's own quads are [`Scene`](Self::Scene); the legacy compatibility quad that the renderer's
/// `render.quad` keeps is [`Compat`](Self::Compat): the presenter draws it only with a **resolved** texture (never the
/// checker), holds the first frame that would show it until its image is there and keeps the last image it drew on screen
/// until a newer one resolves (milestone 5, issue 5d-2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InstanceRole {
    /// An ordinary scene quad: a texture that has nothing shown yet draws the checker.
    #[default]
    Scene,
    /// The frame's one compatibility quad, emitted first.
    Compat,
}

/// One quad of a scene, as a source emits it.
///
/// The presenter checks each instance as it arrives (a finite source inside 0 to 1 and not empty, a destination not
/// empty) and skips an invalid one, counting it; it never panics on a source's output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VulkanInstance<K> {
    /// The logical texture the quad shows.
    pub key: K,
    /// The destination in whole drawable pixels from the top-left corner; `right` greater than `left`, `bottom`
    /// greater than `top`.
    pub destination: DestinationRect,
    /// The visible part of the texture as fractions of it, from its top-left corner: `[left, top, right, bottom]`, each
    /// finite and from 0.0 to 1.0, `right` greater than `left` and `bottom` greater than `top`.
    pub source: [f32; 4],
    /// What the quad is for: [`InstanceRole::Scene`] for every quad but the compatibility quad.
    pub role: InstanceRole,
}

impl<K> VulkanInstance<K> {
    /// Whether the instance may be drawn.
    pub(crate) fn is_valid(&self) -> bool {
        let [left, top, right, bottom] = self.source;
        let inside = |value: f32| value.is_finite() && (0.0..=1.0).contains(&value);
        self.destination.right > self.destination.left
            && self.destination.bottom > self.destination.top
            && [left, top, right, bottom].into_iter().all(inside)
            && right > left
            && bottom > top
    }
}

/// Where a frame's scene quads come from, implemented by the renderer.
///
/// [`place`](Self::place) is called **on the render thread at frame build** and emits the scene's visible quads in
/// painter order (earlier is below). The publisher (a renderer's bridge, under its lock) therefore only exchanges an
/// `Arc`.
pub trait VulkanSceneSource<K>: Send + Sync {
    /// Emits the visible quads in painter order. `width` and `height` are the drawable's current size in pixels,
    /// **informational for a source that carries its own metrics**: such a source (one built from a snapshot, whose
    /// metrics and quads are one coherent pair) places with its own and may ignore the arguments, so a frame's scene
    /// and its legacy quad are always placed with the same metrics. The shader's viewport is always the actual drawable
    /// size; a frame built before a resize is drawn with its own pixel positions into the new drawable until the
    /// resize's own snapshot arrives.
    fn place(&self, width: u32, height: u32, out: &mut dyn FnMut(VulkanInstance<K>));
}

/// What is published to a [`VulkanFrameSlot`].
pub struct VulkanFrame<K> {
    generation: u64,
    filter: VulkanFilter,
    quad: Option<VulkanQuad>,
    scene: Option<Arc<dyn VulkanSceneSource<K>>>,
    /// The key of the frame's compatibility quad, as plain data **outside** the placed instances: a compatibility quad
    /// that is clipped away and places to nothing still has its role known to the presenter (it classifies an upload failure
    /// of this key whatever was placed).
    compat_key: Option<K>,
}

impl<K: Clone> Clone for VulkanFrame<K> {
    fn clone(&self) -> Self {
        Self {
            generation: self.generation,
            filter: self.filter,
            quad: self.quad.clone(),
            scene: self.scene.clone(),
            compat_key: self.compat_key.clone(),
        }
    }
}

impl<K> fmt::Debug for VulkanFrame<K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VulkanFrame")
            .field("generation", &self.generation)
            .field("filter", &self.filter)
            .field("quad", &self.quad.is_some())
            .field("scene", &self.scene.is_some())
            .field("compat", &self.compat_key.is_some())
            .finish()
    }
}

impl<K> VulkanFrame<K> {
    /// A frame of this `generation`: `filter` samples every scene quad (the checker is always nearest), `quad` is the
    /// legacy quad drawn first (below the scene), `scene` the scene source.
    #[must_use]
    pub fn new(
        generation: u64,
        filter: VulkanFilter,
        quad: Option<VulkanQuad>,
        scene: Option<Arc<dyn VulkanSceneSource<K>>>,
    ) -> Self {
        Self {
            generation,
            filter,
            quad,
            scene,
            compat_key: None,
        }
    }

    /// This frame with the key of its compatibility quad (`None`: no compatibility quad). Filled by the renderer's sink from
    /// the frame it publishes, in O(1).
    #[must_use]
    pub fn with_compat_key(mut self, key: Option<K>) -> Self {
        self.compat_key = key;
        self
    }

    /// The key of the frame's compatibility quad, whether or not it placed anything.
    #[must_use]
    pub fn compat_key(&self) -> Option<&K> {
        self.compat_key.as_ref()
    }

    /// The generation of the change that made this frame.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The filter of the scene's quads.
    #[must_use]
    pub fn filter(&self) -> VulkanFilter {
        self.filter
    }

    /// The legacy quad, if the frame has one.
    #[must_use]
    pub fn quad(&self) -> Option<&VulkanQuad> {
        self.quad.as_ref()
    }

    /// The scene source, if the frame has one.
    #[must_use]
    pub fn scene(&self) -> Option<&Arc<dyn VulkanSceneSource<K>>> {
        self.scene.as_ref()
    }
}

struct FrameState<K> {
    frame: VulkanFrame<K>,
    wake: Option<SlotWake>,
}

/// The latest-value slot a renderer publishes frames to and the scene presenter's render thread reads.
///
/// It can be created and published to **before** the presenter starts; the presenter registers its wake at start and
/// its render thread reads the slot before its first frame. A new slot holds the frame of generation 0 with nothing.
pub struct VulkanFrameSlot<K> {
    state: Mutex<FrameState<K>>,
}

impl<K> fmt::Debug for VulkanFrameSlot<K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VulkanFrameSlot")
    }
}

impl<K: Clone> VulkanFrameSlot<K> {
    /// A slot holding the frame of generation 0 with no quad and no scene.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FrameState {
                frame: VulkanFrame::new(0, VulkanFilter::Nearest, None, None),
                wake: None,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, FrameState<K>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the frame and wakes the render thread.
    ///
    /// The lock is held only to swap the frame in; the replaced frame (and with it the last reference to a source or an
    /// image) is dropped after the lock is released; nothing waits for a frame, an upload or a stalled thread.
    pub fn publish(&self, frame: VulkanFrame<K>) {
        let (replaced, wake) = {
            let mut state = self.lock();
            (
                std::mem::replace(&mut state.frame, frame),
                state.wake.clone(),
            )
        };
        drop(replaced);
        if let Some(wake) = wake {
            wake();
        }
    }

    /// Registers the presenter's wake (one presenter at a time).
    pub(crate) fn register_wake(&self, wake: SlotWake) {
        let replaced = self.lock().wake.replace(wake);
        drop(replaced);
    }

    /// Forgets the wake (the presenter ended).
    pub(crate) fn clear_wake(&self) {
        let replaced = self.lock().wake.take();
        drop(replaced);
    }

    /// The latest frame, cloned (the source and the quad's image are shared, not copied).
    pub(crate) fn latest(&self) -> VulkanFrame<K> {
        self.lock().frame.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(source: [f32; 4], right: i32, bottom: i32) -> VulkanInstance<u32> {
        VulkanInstance {
            key: 1,
            destination: DestinationRect {
                left: 0,
                top: 0,
                right,
                bottom,
            },
            source,
            role: InstanceRole::Scene,
        }
    }

    #[test]
    fn an_instance_is_valid_only_with_a_finite_non_empty_source_inside_0_to_1_and_a_non_empty_destination()
     {
        assert!(instance([0.0, 0.0, 1.0, 1.0], 4, 4).is_valid());
        assert!(instance([0.25, 0.5, 0.75, 1.0], 1, 1).is_valid());
        for (source, right, bottom, why) in [
            ([f32::NAN, 0.0, 1.0, 1.0], 4, 4, "NaN"),
            ([0.0, 0.0, f32::INFINITY, 1.0], 4, 4, "infinite"),
            ([-0.1, 0.0, 1.0, 1.0], 4, 4, "below 0"),
            ([0.0, 0.0, 1.0001, 1.0], 4, 4, "above 1"),
            ([0.5, 0.0, 0.5, 1.0], 4, 4, "empty width"),
            ([0.0, 0.9, 1.0, 0.1], 4, 4, "inside out"),
            ([0.0, 0.0, 1.0, 1.0], 0, 4, "empty destination width"),
            ([0.0, 0.0, 1.0, 1.0], 4, 0, "empty destination height"),
            ([0.0, 0.0, 1.0, 1.0], -3, 4, "inside-out destination"),
        ] {
            assert!(!instance(source, right, bottom).is_valid(), "{why}");
        }
    }

    #[test]
    fn a_new_slot_holds_generation_zero_and_publishing_replaces_it() {
        let slot = VulkanFrameSlot::<u32>::new();
        let first = slot.latest();
        assert_eq!(first.generation(), 0);
        assert!(first.quad().is_none() && first.scene().is_none());
        slot.publish(VulkanFrame::new(5, VulkanFilter::Linear, None, None));
        let latest = slot.latest();
        assert_eq!(
            (latest.generation(), latest.filter()),
            (5, VulkanFilter::Linear)
        );
    }
}
