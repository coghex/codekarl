//! What a scene presenter is given each frame (milestone 5, issue 5b-1): plain data and a latest-value slot, like the
//! legacy quad's ([`MetalQuadSlot`](crate::MetalQuadSlot)).
//!
//! - [`MetalInstance`] is one quad of the scene: the logical key of its texture, the destination in whole drawable
//!   pixels and the visible part of the texture as **fractions of it** (so a reload with new dimensions samples the
//!   whole new image).
//! - [`MetalSceneSource`] emits a scene's visible quads in painter order. It is called **on the render thread at frame
//!   build**, so placement in pixels never runs under the publisher's lock.
//! - [`MetalFrame`] is what is published: the generation, the filter for the whole frame, the legacy quad (under every
//!   scene quad) and the scene source.
//! - [`MetalFrameSlot`] is the latest-value slot: [`publish`](MetalFrameSlot::publish) swaps the frame in under a lock
//!   held only for the swap, drops the replaced frame after the lock is released, and wakes the render thread.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::quad::{DestinationRect, MetalFilter, MetalQuad, SlotWake};

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
pub struct MetalInstance<K> {
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

impl<K> MetalInstance<K> {
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
pub trait MetalSceneSource<K>: Send + Sync {
    /// Emits the visible quads in painter order. `width` and `height` are the drawable's current size in pixels,
    /// **informational for a source that carries its own metrics**: such a source (one built from a snapshot, whose
    /// metrics and quads are one coherent pair) places with its own and may ignore the arguments, so a frame's scene
    /// and its legacy quad are always placed with the same metrics. The shader's viewport is always the actual drawable
    /// size; a frame built before a resize is drawn with its own pixel positions into the new drawable until the
    /// resize's own snapshot arrives.
    fn place(&self, width: u32, height: u32, out: &mut dyn FnMut(MetalInstance<K>));
}

/// What is published to a [`MetalFrameSlot`].
pub struct MetalFrame<K> {
    generation: u64,
    filter: MetalFilter,
    quad: Option<MetalQuad>,
    scene: Option<Arc<dyn MetalSceneSource<K>>>,
    /// The key of the frame's compatibility quad, as plain data **outside** the placed instances: a compatibility quad
    /// that is clipped away and places to nothing still has its role known to the presenter (it classifies an upload failure
    /// of this key whatever was placed).
    compat_key: Option<K>,
}

impl<K: Clone> Clone for MetalFrame<K> {
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

impl<K> fmt::Debug for MetalFrame<K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MetalFrame")
            .field("generation", &self.generation)
            .field("filter", &self.filter)
            .field("quad", &self.quad.is_some())
            .field("scene", &self.scene.is_some())
            .field("compat", &self.compat_key.is_some())
            .finish()
    }
}

impl<K> MetalFrame<K> {
    /// A frame of this `generation`: `filter` samples every scene quad (the checker is always nearest), `quad` is the
    /// legacy quad drawn first (below the scene), `scene` the scene source.
    #[must_use]
    pub fn new(
        generation: u64,
        filter: MetalFilter,
        quad: Option<MetalQuad>,
        scene: Option<Arc<dyn MetalSceneSource<K>>>,
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
    pub fn filter(&self) -> MetalFilter {
        self.filter
    }

    /// The legacy quad, if the frame has one.
    #[must_use]
    pub fn quad(&self) -> Option<&MetalQuad> {
        self.quad.as_ref()
    }

    /// The scene source, if the frame has one.
    #[must_use]
    pub fn scene(&self) -> Option<&Arc<dyn MetalSceneSource<K>>> {
        self.scene.as_ref()
    }
}

/// The scene presenter's counters and charges, read with [`MetalPresenter::stats`](crate::MetalPresenter::stats).
///
/// The ordinary-binding baseline of milestone 5 (§11.3): per committed frame, the latest value and the peak, never a log.
/// The checker's bytes are the presenter's **baseline**, reported here and nowhere else: they are not in the store's
/// `texture_bytes`, its staging leases or prepared window, or any ledger charge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SceneStats {
    /// Scene frames committed.
    pub frames: u64,
    /// The counters of the last committed frame.
    pub last: crate::batch::FrameCounters,
    /// The largest of each counter over the committed frames.
    pub peak: crate::batch::FrameCounters,
    /// Instance slots written into the instance buffer, over every attempt (a frame not committed included).
    pub instances_written: u64,
    /// Frame attempts that found the previous command buffer still outstanding and wrote nothing into the buffer.
    pub buffer_busy: u64,
    /// The instance buffer's logical size (`max_quads` instances at the instance stride).
    pub instance_buffer_bytes: u64,
    /// The instance buffer's measured `allocatedSize`, which is what `binding_geometry_bytes` is charged.
    pub instance_buffer_allocated: u64,
    /// The `binding_geometry_bytes` budget the presenter was started with.
    pub binding_geometry_bytes: u64,
    /// Whether the checker is resident (no frame is committed before it is).
    pub checker_resident: bool,
    /// Textures the upload store created over the presenter's life (cumulative: it never decreases, whatever is retired).
    /// Published with each texture pass, so a snapshot taken after a frame includes the creations that frame's revisions
    /// needed. Zero for a presenter without a scene.
    pub textures_created: u64,
    /// Upload batches the store submitted to the GPU over the presenter's life (cumulative).
    pub uploads_submitted: u64,
    /// Upload batches whose completion the store observed (cumulative).
    pub uploads_completed: u64,
    /// The most texture bytes the upload store ever had charged at once (its own high-water mark, `TextureStats::texture_bytes_peak`:
    /// the images' textures, **not** the checker baseline below, which is charged to the presenter). Published with each texture
    /// pass (a failing and the ending one too) and once more before the store is released or forgotten; it never decreases. As of the
    /// presenter's last look at the store: a worker an `Unproven` end left running may charge more afterwards (a lower bound there). Zero before the first upload and for a presenter without a scene.
    pub upload_texture_bytes_peak: u64,
    /// The most staging bytes the upload store ever had charged at once (`TextureStats::staging_bytes_peak`, the upload leases,
    /// **not** the checker's staging buffer below). Cumulative high-water; zero before the first upload.
    pub upload_staging_bytes_peak: u64,
    /// The checker texture's measured bytes (presenter baseline); zero until it is resident.
    pub checker_texture_bytes: u64,
    /// The checker's staging buffer's measured bytes (presenter baseline); zero until it is resident.
    pub checker_staging_bytes: u64,
    /// Frame attempts held back by the initial gate: the compatibility quad's image was not there and no compatibility image
    /// had been drawn yet, so the frame was not drawn (its ticket discarded).
    pub gate_holds: u64,
    /// The longest the presenter stayed gated, in microseconds: from the first held attempt to the frame that drew.
    pub gate_longest_hold_micros: u64,
    /// Committed frames that drew the **held** compatibility image because the current one had not resolved (a replacement).
    pub held_compat_frames: u64,
    /// The revision of the compatibility image held now (0: none held). Only one old revision is ever held.
    pub held_compat_revision: u64,
    /// Compatibility-quad uploads that failed and became the presenter's `Failed` report.
    pub compat_upload_failures: u64,
    /// Native residency-set allocation, separate from texture allocation charges.
    pub residency_set_bytes: u64,
    /// Measured allocation for both resource-ID tables: 16 native bytes per texture slot before allocator alignment.
    /// A frame pins one existing table version and copies no table bytes.
    pub bindless_table_bytes: u64,
    /// The entry count of the bindless tables the presenter actually allocated (each of the two versions), so the setting's
    /// propagation is observable: it is `[render] bindless_texture_slots` for a presenter started with bindless limits, and
    /// zero for a scene without them (the hidden ordinary option).
    pub bindless_slots: u32,
    /// Revisions the render thread adopted over the presenter's life: the moments textures became **ready** (shown) to the frames.
    pub textures_ready: u64,
    /// When the first adopting frame boundary ran (`CLOCK_UPTIME_RAW` nanoseconds; 0: none yet).
    pub first_ready_ns: u64,
    /// When the latest adopting frame boundary ran (same clock; 0: none yet).
    pub last_ready_ns: u64,
}

struct FrameState<K> {
    frame: MetalFrame<K>,
    wake: Option<SlotWake>,
}

/// The latest-value slot a renderer publishes frames to and the scene presenter's render thread reads.
///
/// It can be created and published to **before** the presenter starts; the presenter registers its wake at start and
/// its render thread reads the slot before its first frame. A new slot holds the frame of generation 0 with nothing.
pub struct MetalFrameSlot<K> {
    state: Mutex<FrameState<K>>,
}

impl<K> fmt::Debug for MetalFrameSlot<K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MetalFrameSlot")
    }
}

impl<K: Clone> MetalFrameSlot<K> {
    /// A slot holding the frame of generation 0 with no quad and no scene.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FrameState {
                frame: MetalFrame::new(0, MetalFilter::Nearest, None, None),
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
    pub fn publish(&self, frame: MetalFrame<K>) {
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
    pub(crate) fn latest(&self) -> MetalFrame<K> {
        self.lock().frame.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(source: [f32; 4], right: i32, bottom: i32) -> MetalInstance<u32> {
        MetalInstance {
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
        let slot = MetalFrameSlot::<u32>::new();
        let first = slot.latest();
        assert_eq!(first.generation(), 0);
        assert!(first.quad().is_none() && first.scene().is_none());
        slot.publish(MetalFrame::new(5, MetalFilter::Linear, None, None));
        let latest = slot.latest();
        assert_eq!(
            (latest.generation(), latest.filter()),
            (5, MetalFilter::Linear)
        );
    }
}
