//! The latest-value slot a renderer publishes scenes to and the presenter's render thread reads (design §4.2,
//! §7.2), and the format notice.
//!
//! The slot holds no Vulkan object. It can be created, and published to, before a presenter starts; the
//! presenter registers its wake when it starts and its render thread reads the slot before its first frame, so
//! a scene published first is in the first frame. A publish wakes the render thread only, never the main loop.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ash::vk;

use crate::quad::VulkanScene;

/// The callback that asks the render thread for a frame.
pub(crate) type SlotWake = Arc<dyn Fn() + Send + Sync>;

struct SlotState {
    scene: VulkanScene,
    wake: Option<SlotWake>,
}

/// The latest-value slot: the scene a renderer last published. Shared by `Arc`, `Send + Sync`.
///
/// A new slot holds the scene of generation 0 with no quad.
pub struct VulkanQuadSlot {
    state: Mutex<SlotState>,
}

impl fmt::Debug for VulkanQuadSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VulkanQuadSlot")
    }
}

impl VulkanQuadSlot {
    /// A slot holding the scene of generation 0 with no quad.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(SlotState {
                scene: VulkanScene::new(0, None),
                wake: None,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the scene and, once a presenter has registered, marks a frame wanted and wakes its render
    /// thread (never the main loop).
    ///
    /// The lock is held only to swap the scene in; the replaced scene is dropped after it is released (so
    /// freeing the last reference to an image never happens under it). Nothing waits for a frame, an upload or
    /// a stalled thread. After the presenter ended, or was dropped, this only stores the value.
    pub fn publish(&self, scene: VulkanScene) {
        let (replaced, wake) = {
            let mut state = self.lock();
            (
                std::mem::replace(&mut state.scene, scene),
                state.wake.clone(),
            )
        };
        // After the lock: the replaced scene may be the last holder of an image, and the wake takes the
        // presenter's own lock.
        drop(replaced);
        if let Some(wake) = wake {
            wake();
        }
    }

    /// Registers the presenter's wake (one presenter at a time; a later registration replaces an earlier one).
    pub(crate) fn register_wake(&self, wake: SlotWake) {
        let replaced = self.lock().wake.replace(wake);
        drop(replaced);
    }

    /// Forgets the wake (the presenter ended).
    pub(crate) fn clear_wake(&self) {
        let replaced = self.lock().wake.take();
        drop(replaced);
    }

    /// The latest scene, cloned (a quad's pixel source is shared, not copied).
    pub(crate) fn latest(&self) -> VulkanScene {
        self.lock().scene.clone()
    }
}

/// Why the quad is not drawn: the surface's colour format is not one the textured drawing is exact on
/// (`B8G8R8A8_UNORM` or `R8G8B8A8_UNORM`, design §7.3). The window keeps clearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuadSkipped {
    /// The surface format the presenter chose.
    pub format: vk::Format,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;
    use crate::quad::{DestinationRect, PixelSource, SourceRect, VulkanFilter, VulkanQuad};

    struct Source {
        bytes: Vec<u8>,
        dropped: Arc<AtomicBool>,
        slot_was_locked_at_drop: Arc<AtomicBool>,
        slot: Mutex<Option<Arc<VulkanQuadSlot>>>,
    }

    impl PixelSource for Source {
        fn pixels(&self) -> &[u8] {
            &self.bytes
        }
    }

    impl Drop for Source {
        fn drop(&mut self) {
            if let Some(slot) = self.slot.lock().ok().and_then(|slot| slot.clone()) {
                // The replaced scene is dropped after the publish released its lock: the lock is free here.
                self.slot_was_locked_at_drop
                    .store(slot.state.try_lock().is_err(), Ordering::SeqCst);
            }
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    fn scene_of(source: Arc<dyn PixelSource>, generation: u64) -> VulkanScene {
        let quad = VulkanQuad::new(
            source,
            1,
            1,
            DestinationRect {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
            },
            SourceRect {
                left: 0.0,
                top: 0.0,
                right: 1.0,
                bottom: 1.0,
            },
            VulkanFilter::Nearest,
        )
        .expect("a valid quad");
        VulkanScene::new(generation, Some(quad))
    }

    #[test]
    fn a_new_slot_holds_generation_zero_with_no_quad() {
        let scene = VulkanQuadSlot::new().latest();
        assert_eq!(scene.generation(), 0);
        assert!(scene.quad().is_none());
    }

    #[test]
    fn a_publish_replaces_the_value_and_wakes_only_once_a_presenter_registered() {
        let slot = VulkanQuadSlot::new();
        let wakes = Arc::new(AtomicUsize::new(0));
        slot.publish(VulkanScene::new(1, None));
        assert_eq!(slot.latest().generation(), 1, "stored before any presenter");
        let counted = Arc::clone(&wakes);
        slot.register_wake(Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(wakes.load(Ordering::SeqCst), 0, "registering is not a wake");
        slot.publish(VulkanScene::new(2, None));
        assert_eq!(
            (slot.latest().generation(), wakes.load(Ordering::SeqCst)),
            (2, 1)
        );
        slot.clear_wake();
        slot.publish(VulkanScene::new(3, None));
        assert_eq!(
            (slot.latest().generation(), wakes.load(Ordering::SeqCst)),
            (3, 1),
            "after the presenter ended a publish only stores the value"
        );
    }

    #[test]
    fn the_replaced_scene_is_dropped_after_the_lock_is_released() {
        let slot = VulkanQuadSlot::new();
        let dropped = Arc::new(AtomicBool::new(false));
        let locked = Arc::new(AtomicBool::new(true));
        let source = Arc::new(Source {
            bytes: vec![255; 4],
            dropped: Arc::clone(&dropped),
            slot_was_locked_at_drop: Arc::clone(&locked),
            slot: Mutex::new(Some(Arc::clone(&slot))),
        });
        slot.publish(scene_of(Arc::clone(&source) as Arc<dyn PixelSource>, 1));
        drop(source);
        assert!(!dropped.load(Ordering::SeqCst), "the slot holds the source");
        slot.publish(VulkanScene::new(2, None));
        assert!(
            dropped.load(Ordering::SeqCst),
            "the replaced scene was dropped"
        );
        assert!(
            !locked.load(Ordering::SeqCst),
            "the replaced scene was dropped under the slot's lock"
        );
    }
}
