//! The plain quad types the Metal presenter draws, and the latest-value slot that carries them.
//!
//! Everything here is plain data: no Metal object, no dependency on a renderer crate. A renderer adapter
//! (issue 4) implements [`PixelSource`] over its own image, builds a [`MetalQuad`] from its snapshot and
//! publishes it to a [`MetalQuadSlot`]; the presenter's render thread reads the slot.
//!
//! - [`PixelSource`] is where the pixels come from: premultiplied RGBA8, rows top first. The render thread
//!   copies them into a texture once, when a quad with a different source arrives, and then **drops its
//!   reference to the source**, so the CPU copy of an uploaded image is not kept alive by the GPU side.
//! - [`MetalQuad`] is a validated quad: the source, its size, the destination rectangle in whole drawable
//!   pixels, the part of the image to draw and the filter. An image is at most [`MAX_IMAGE_BYTES`] (64 MiB).
//! - [`MetalScene`] is what is published: a generation, and a quad or none. The generation travels **with or
//!   without a quad**, so a scene that shows nothing (cleared, or clipped away entirely) is observed with its
//!   own generation.
//! - [`MetalQuadSlot`] is the latest-value slot of the design (§4.2): [`publish`](MetalQuadSlot::publish)
//!   swaps the scene in under a lock held only for the swap, drops the scene it replaced **after** the lock
//!   is released, and wakes the render thread once one is registered. It never waits for a frame, an upload or
//!   a stalled thread, and it holds no GPU object, so publishing after the presenter ended touches nothing on
//!   the GPU.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The largest width or height of a quad's image, in pixels (a Metal 2D texture limit on the GPUs in use).
pub const MAX_IMAGE_SIDE: u32 = 16_384;

/// The most bytes of RGBA8 one quad texture may take: 64 MiB (for example 4096 × 4096). It is the per-texture
/// cap of the presenter's GPU bound (at most two textures are alive), enforced when a quad is made.
pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

/// Where a quad's pixels come from.
///
/// An implementation owns the image (typically behind an `Arc`, shared with the renderer that decoded it), so
/// handing the presenter a source copies no pixels.
pub trait PixelSource: Send + Sync {
    /// The pixels: premultiplied RGBA8, rows top first, exactly `width × height × 4` bytes for the quad this
    /// source belongs to.
    fn pixels(&self) -> &[u8];
}

/// How a quad's image is sampled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetalFilter {
    /// Each destination pixel takes the nearest image pixel.
    Nearest,
    /// Each destination pixel blends the four nearest image pixels.
    Linear,
}

/// A rectangle in whole drawable pixels, from the drawable's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DestinationRect {
    /// The left edge.
    pub left: i32,
    /// The top edge.
    pub top: i32,
    /// The right edge (exclusive); greater than `left`.
    pub right: i32,
    /// The bottom edge (exclusive); greater than `top`.
    pub bottom: i32,
}

/// The part of the image to draw, in image pixels from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceRect {
    /// The left edge.
    pub left: f32,
    /// The top edge.
    pub top: f32,
    /// The right edge.
    pub right: f32,
    /// The bottom edge.
    pub bottom: f32,
}

/// Why a quad is not valid. An invalid quad never reaches the GPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetalQuadError {
    /// A width or height is zero or over [`MAX_IMAGE_SIDE`].
    ImageSize {
        /// The width given.
        width: u32,
        /// The height given.
        height: u32,
    },
    /// The image would take more than [`MAX_IMAGE_BYTES`] as RGBA8.
    ImageTooLarge {
        /// The bytes `width × height × 4` come to.
        bytes: u64,
    },
    /// The source did not hold exactly `width × height × 4` bytes.
    PixelLength {
        /// The bytes the size needs.
        expected: usize,
        /// The bytes the source held.
        actual: usize,
    },
    /// The destination rectangle is empty or inside out.
    Destination,
    /// The source rectangle is not finite, not inside the image, or empty.
    Source,
}

impl fmt::Display for MetalQuadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ImageSize { width, height } => write!(
                formatter,
                "the image is {width} x {height} pixels; each side must be from 1 to {MAX_IMAGE_SIDE}"
            ),
            Self::ImageTooLarge { bytes } => write!(
                formatter,
                "the image would take {bytes} bytes as RGBA8, over the {MAX_IMAGE_BYTES} a quad texture may take"
            ),
            Self::PixelLength { expected, actual } => {
                write!(formatter, "the pixel source holds {actual} bytes, not {expected}")
            }
            Self::Destination => formatter.write_str(
                "the destination rectangle needs right greater than left and bottom greater than top",
            ),
            Self::Source => formatter.write_str(
                "the source rectangle must be finite, non-empty and inside the image",
            ),
        }
    }
}

impl std::error::Error for MetalQuadError {}

/// A validated quad: an image and where, and how, to draw it.
#[derive(Clone)]
pub struct MetalQuad {
    pixels: Arc<dyn PixelSource>,
    width: u32,
    height: u32,
    destination: DestinationRect,
    source: SourceRect,
    filter: MetalFilter,
}

impl fmt::Debug for MetalQuad {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MetalQuad")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("destination", &self.destination)
            .field("source", &self.source)
            .field("filter", &self.filter)
            .finish_non_exhaustive()
    }
}

impl MetalQuad {
    /// A quad of the `width × height` image `pixels` holds.
    ///
    /// # Errors
    ///
    /// [`MetalQuadError`] unless:
    /// - each side is from 1 to [`MAX_IMAGE_SIDE`];
    /// - the image takes at most [`MAX_IMAGE_BYTES`] as RGBA8 (checked before the source is looked at);
    /// - `pixels` holds exactly `width × height × 4` bytes;
    /// - `destination` has right greater than left and bottom greater than top;
    /// - `source` is finite, non-empty and inside the image.
    pub fn new(
        pixels: Arc<dyn PixelSource>,
        width: u32,
        height: u32,
        destination: DestinationRect,
        source: SourceRect,
        filter: MetalFilter,
    ) -> Result<Self, MetalQuadError> {
        if !(1..=MAX_IMAGE_SIDE).contains(&width) || !(1..=MAX_IMAGE_SIDE).contains(&height) {
            return Err(MetalQuadError::ImageSize { width, height });
        }
        // Checked arithmetic, in u64 so that nothing overflows on any target: at most 2^14 × 2^14 × 4 = 2^30.
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(MetalQuadError::ImageTooLarge { bytes: u64::MAX })?;
        if bytes > MAX_IMAGE_BYTES {
            return Err(MetalQuadError::ImageTooLarge { bytes });
        }
        let expected = (width as usize) * (height as usize) * 4;
        let actual = pixels.pixels().len();
        if actual != expected {
            return Err(MetalQuadError::PixelLength { expected, actual });
        }
        if destination.right <= destination.left || destination.bottom <= destination.top {
            return Err(MetalQuadError::Destination);
        }
        let (image_width, image_height) = (to_f32(width), to_f32(height));
        let finite = [source.left, source.top, source.right, source.bottom]
            .iter()
            .all(|value| value.is_finite());
        let inside = source.left >= 0.0
            && source.top >= 0.0
            && source.right <= image_width
            && source.bottom <= image_height;
        if !finite || !inside || source.right <= source.left || source.bottom <= source.top {
            return Err(MetalQuadError::Source);
        }
        Ok(Self {
            pixels,
            width,
            height,
            destination,
            source,
            filter,
        })
    }

    /// The image's width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The image's height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The destination rectangle in drawable pixels.
    #[must_use]
    pub fn destination(&self) -> DestinationRect {
        self.destination
    }

    /// The part of the image drawn, in image pixels.
    #[must_use]
    pub fn source(&self) -> SourceRect {
        self.source
    }

    /// How the image is sampled.
    #[must_use]
    pub fn filter(&self) -> MetalFilter {
        self.filter
    }

    /// The pixel source.
    #[must_use]
    pub fn pixels(&self) -> &Arc<dyn PixelSource> {
        &self.pixels
    }
}

/// A side of at most 16384 is exactly representable as `f32`.
#[expect(
    clippy::cast_precision_loss,
    reason = "the value is at most 16384, which f32 holds exactly"
)]
fn to_f32(side: u32) -> f32 {
    side as f32
}

/// What is published to a slot: the generation of the change that made it, and a quad or none.
///
/// The generation travels with or without a quad, so a scene that shows nothing (cleared, or clipped away
/// entirely) is told apart from the one before it, and a frame drawn from it reports its generation.
#[derive(Debug, Clone)]
pub struct MetalScene {
    generation: u64,
    quad: Option<MetalQuad>,
}

impl MetalScene {
    /// A scene of this `generation` showing `quad`, or nothing.
    #[must_use]
    pub fn new(generation: u64, quad: Option<MetalQuad>) -> Self {
        Self { generation, quad }
    }

    /// The generation of the change that made this scene.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The quad, if the scene shows one.
    #[must_use]
    pub fn quad(&self) -> Option<&MetalQuad> {
        self.quad.as_ref()
    }
}

/// The callback that asks the render thread for a frame.
pub(crate) type SlotWake = Arc<dyn Fn() + Send + Sync>;

struct SlotState {
    scene: MetalScene,
    wake: Option<SlotWake>,
}

/// The latest-value slot a renderer publishes scenes to and the presenter's render thread reads.
///
/// It can be created and published to **before** the presenter starts; the presenter registers its wake at
/// start and its render thread reads the slot before its first frame, so a scene published first is in the
/// first frame. A new slot holds the scene of generation 0 with no quad.
pub struct MetalQuadSlot {
    state: Mutex<SlotState>,
}

impl fmt::Debug for MetalQuadSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MetalQuadSlot")
    }
}

impl MetalQuadSlot {
    /// A slot holding the scene of generation 0 with no quad.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(SlotState {
                scene: MetalScene::new(0, None),
                wake: None,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the scene and wakes the render thread.
    ///
    /// The lock is held only to swap the scene in; the replaced scene is dropped after it is released (so
    /// freeing the last reference to an image never happens under it); nothing waits for a frame, an upload or
    /// a stalled thread.
    pub fn publish(&self, scene: MetalScene) {
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

    /// The latest scene, cloned (a quad's source is shared, not copied).
    pub(crate) fn latest(&self) -> MetalScene {
        self.lock().scene.clone()
    }
}

#[cfg(test)]
impl MetalQuadSlot {
    /// Whether another holder has the slot's lock right now (tests only: a drop probe).
    fn lock_is_held(&self) -> bool {
        matches!(
            self.state.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::*;

    /// A pixel source of opaque white pixels that calls a function when it is dropped.
    struct Source {
        bytes: Vec<u8>,
        on_drop: Option<Box<dyn Fn() + Send + Sync>>,
    }

    impl PixelSource for Source {
        fn pixels(&self) -> &[u8] {
            &self.bytes
        }
    }

    impl Drop for Source {
        fn drop(&mut self) {
            if let Some(on_drop) = &self.on_drop {
                on_drop();
            }
        }
    }

    fn source(width: u32, height: u32) -> Arc<dyn PixelSource> {
        Arc::new(Source {
            bytes: vec![255; (width * height * 4) as usize],
            on_drop: None,
        })
    }

    const WHOLE: DestinationRect = DestinationRect {
        left: 0,
        top: 0,
        right: 4,
        bottom: 4,
    };

    fn rect(left: f32, top: f32, right: f32, bottom: f32) -> SourceRect {
        SourceRect {
            left,
            top,
            right,
            bottom,
        }
    }

    fn quad() -> MetalQuad {
        MetalQuad::new(
            source(2, 2),
            2,
            2,
            WHOLE,
            rect(0.0, 0.0, 2.0, 2.0),
            MetalFilter::Nearest,
        )
        .expect("a valid quad")
    }

    fn scene(generation: u64) -> MetalScene {
        MetalScene::new(generation, Some(quad()))
    }

    /// The generation of the scene the slot holds, and whether it has a quad.
    fn held(slot: &MetalQuadSlot) -> (u64, bool) {
        let latest = slot.latest();
        (latest.generation(), latest.quad().is_some())
    }

    #[test]
    fn a_valid_quad_keeps_what_it_was_given() {
        let made = quad();
        assert_eq!((made.width(), made.height()), (2, 2));
        assert_eq!(made.destination(), WHOLE);
        assert_eq!(made.source(), rect(0.0, 0.0, 2.0, 2.0));
        assert_eq!(made.filter(), MetalFilter::Nearest);
        assert_eq!(made.pixels().pixels().len(), 16);
        assert!(format!("{made:?}").contains("width: 2"));
    }

    #[test]
    fn a_scene_keeps_its_generation_with_or_without_a_quad() {
        let with = scene(7);
        assert_eq!(with.generation(), 7);
        assert!(with.quad().is_some());
        let without = MetalScene::new(8, None);
        assert_eq!(without.generation(), 8);
        assert!(without.quad().is_none());
        assert!(format!("{without:?}").contains("generation: 8"));
    }

    #[test]
    fn a_quad_refuses_a_bad_image_size() {
        for (width, height) in [
            (0, 2),
            (2, 0),
            (MAX_IMAGE_SIDE + 1, 1),
            (1, MAX_IMAGE_SIDE + 1),
        ] {
            // The source length is irrelevant: the size is checked first.
            let error = MetalQuad::new(
                source(1, 1),
                width,
                height,
                WHOLE,
                rect(0.0, 0.0, 1.0, 1.0),
                MetalFilter::Linear,
            )
            .expect_err("a bad size");
            assert_eq!(error, MetalQuadError::ImageSize { width, height });
        }
    }

    #[test]
    fn the_image_cap_is_64_mib_exactly_and_is_checked_before_the_source_is_looked_at() {
        assert_eq!(MAX_IMAGE_BYTES, 64 * 1024 * 1024);
        // Exactly 64 MiB (4096 x 4096) is accepted, and so is the same bytes in another shape.
        for (width, height) in [(4096, 4096), (16_384, 1024), (1024, 16_384)] {
            let pixels: Arc<dyn PixelSource> = Arc::new(Source {
                bytes: vec![0; usize::try_from(MAX_IMAGE_BYTES).expect("64 MiB fits")],
                on_drop: None,
            });
            let made = MetalQuad::new(
                pixels,
                width,
                height,
                WHOLE,
                rect(0.0, 0.0, 1.0, 1.0),
                MetalFilter::Nearest,
            );
            assert!(made.is_ok(), "{width} x {height}: {made:?}");
        }
        // One row more is refused, naming the size, and before the source is looked at (a tiny source is
        // not a length error here): so nothing of it reaches the GPU.
        for (width, height) in [(4096, 4097), (4097, 4096), (16_384, 1025), (16_384, 16_384)] {
            let error = MetalQuad::new(
                source(1, 1),
                width,
                height,
                WHOLE,
                rect(0.0, 0.0, 1.0, 1.0),
                MetalFilter::Nearest,
            )
            .expect_err("over the cap");
            let bytes = u64::from(width) * u64::from(height) * 4;
            assert_eq!(
                error,
                MetalQuadError::ImageTooLarge { bytes },
                "{width} x {height}"
            );
            assert!(error.to_string().contains(&bytes.to_string()));
        }
        // 16385 wide is refused as a size.
        assert!(matches!(
            MetalQuad::new(
                source(1, 1),
                16_385,
                1,
                WHOLE,
                rect(0.0, 0.0, 1.0, 1.0),
                MetalFilter::Nearest
            ),
            Err(MetalQuadError::ImageSize { width: 16_385, .. })
        ));
    }

    #[test]
    fn a_quad_refuses_a_source_of_the_wrong_length() {
        let error = MetalQuad::new(
            source(2, 2),
            2,
            3,
            WHOLE,
            rect(0.0, 0.0, 2.0, 3.0),
            MetalFilter::Nearest,
        )
        .expect_err("16 bytes are not 24");
        assert_eq!(
            error,
            MetalQuadError::PixelLength {
                expected: 24,
                actual: 16
            }
        );
        assert!(error.to_string().contains("16") && error.to_string().contains("24"));
    }

    #[test]
    fn a_quad_refuses_an_empty_or_inside_out_destination() {
        for (left, top, right, bottom) in [(0, 0, 0, 4), (0, 0, 4, 0), (3, 0, 2, 4), (0, 3, 4, 2)] {
            let error = MetalQuad::new(
                source(2, 2),
                2,
                2,
                DestinationRect {
                    left,
                    top,
                    right,
                    bottom,
                },
                rect(0.0, 0.0, 2.0, 2.0),
                MetalFilter::Nearest,
            )
            .expect_err("a bad destination");
            assert_eq!(error, MetalQuadError::Destination);
        }
        // A destination partly or wholly off the drawable is still a valid rectangle.
        let off = DestinationRect {
            left: -10,
            top: -10,
            right: -2,
            bottom: 3,
        };
        assert!(
            MetalQuad::new(
                source(2, 2),
                2,
                2,
                off,
                rect(0.0, 0.0, 2.0, 2.0),
                MetalFilter::Nearest,
            )
            .is_ok()
        );
    }

    #[test]
    fn a_quad_refuses_a_source_rectangle_that_is_not_finite_empty_or_outside_the_image() {
        let bad = [
            rect(f32::NAN, 0.0, 2.0, 2.0),
            rect(0.0, f32::INFINITY, 2.0, 2.0),
            rect(0.0, 0.0, f32::INFINITY, 2.0),
            rect(0.0, 0.0, 2.0, f32::NEG_INFINITY),
            rect(-0.5, 0.0, 2.0, 2.0),
            rect(0.0, -0.5, 2.0, 2.0),
            rect(0.0, 0.0, 2.5, 2.0),
            rect(0.0, 0.0, 2.0, 2.5),
            rect(1.0, 0.0, 1.0, 2.0),
            rect(0.0, 1.0, 2.0, 1.0),
            rect(1.5, 0.0, 1.0, 2.0),
        ];
        for source_rect in bad {
            let error =
                MetalQuad::new(source(2, 2), 2, 2, WHOLE, source_rect, MetalFilter::Nearest)
                    .expect_err("a bad source rectangle");
            assert_eq!(error, MetalQuadError::Source, "{source_rect:?}");
        }
        // A trimmed region inside the image is valid.
        assert!(
            MetalQuad::new(
                source(2, 2),
                2,
                2,
                WHOLE,
                rect(0.5, 0.25, 1.5, 2.0),
                MetalFilter::Linear,
            )
            .is_ok()
        );
    }

    #[test]
    fn a_quad_accepts_the_largest_image_side() {
        let made = MetalQuad::new(
            source(MAX_IMAGE_SIDE, 1),
            MAX_IMAGE_SIDE,
            1,
            WHOLE,
            rect(0.0, 0.0, 16_384.0, 1.0),
            MetalFilter::Nearest,
        );
        assert!(made.is_ok());
    }

    #[test]
    fn the_slot_starts_with_generation_zero_and_no_quad_and_holds_the_latest_scene() {
        let slot = MetalQuadSlot::new();
        assert_eq!(held(&slot), (0, false));
        slot.publish(scene(3));
        assert_eq!(held(&slot), (3, true));
        slot.publish(scene(4));
        assert_eq!(held(&slot), (4, true));
        // A scene without a quad keeps its own generation.
        slot.publish(MetalScene::new(9, None));
        assert_eq!(held(&slot), (9, false));
        slot.publish(MetalScene::new(10, None));
        assert_eq!(held(&slot), (10, false));
    }

    #[test]
    fn a_publish_before_any_wake_is_registered_is_kept_for_the_first_read() {
        let slot = MetalQuadSlot::new();
        slot.publish(scene(11));
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        slot.register_wake(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(
            woken.load(Ordering::SeqCst),
            0,
            "registering a wake wakes nothing"
        );
        assert_eq!(held(&slot), (11, true));
    }

    #[test]
    fn every_publish_after_registration_wakes_once_and_a_cleared_wake_is_not_called() {
        let slot = MetalQuadSlot::new();
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        slot.register_wake(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        slot.publish(scene(1));
        slot.publish(MetalScene::new(2, None));
        slot.publish(MetalScene::new(5, None));
        assert_eq!(woken.load(Ordering::SeqCst), 3);
        slot.clear_wake();
        slot.publish(scene(2));
        assert_eq!(woken.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn the_wake_runs_with_the_slots_lock_released() {
        let slot = MetalQuadSlot::new();
        let seen_held = Arc::new(AtomicBool::new(true));
        let probe = Arc::downgrade(&slot);
        let record = Arc::clone(&seen_held);
        slot.register_wake(Arc::new(move || {
            let held = probe.upgrade().is_some_and(|slot| slot.lock_is_held());
            record.store(held, Ordering::SeqCst);
        }));
        slot.publish(scene(1));
        assert!(!seen_held.load(Ordering::SeqCst));
    }

    #[test]
    fn a_replaced_scene_is_dropped_after_the_lock_is_released() {
        let slot = MetalQuadSlot::new();
        let probe = Arc::downgrade(&slot);
        let drops = Arc::new(Mutex::new(Vec::new()));
        let make = |generation| {
            let (probe, drops) = (probe.clone(), Arc::clone(&drops));
            let pixels: Arc<dyn PixelSource> = Arc::new(Source {
                bytes: vec![0; 16],
                on_drop: Some(Box::new(move || {
                    let held = probe.upgrade().is_some_and(|slot| slot.lock_is_held());
                    drops
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(held);
                })),
            });
            let quad = MetalQuad::new(
                pixels,
                2,
                2,
                WHOLE,
                rect(0.0, 0.0, 2.0, 2.0),
                MetalFilter::Nearest,
            )
            .expect("a valid quad");
            MetalScene::new(generation, Some(quad))
        };
        slot.publish(make(1));
        slot.publish(make(2));
        slot.publish(MetalScene::new(3, None));
        slot.publish(MetalScene::new(4, None));
        let seen = drops.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert_eq!(
            seen,
            vec![false, false],
            "both were dropped, outside the lock"
        );
    }

    #[test]
    fn the_probe_sees_a_drop_made_while_the_lock_is_held() {
        // The control for the test above.
        let slot = MetalQuadSlot::new();
        let probe = Arc::downgrade(&slot);
        let held = Arc::new(AtomicBool::new(false));
        let record = Arc::clone(&held);
        let guard = slot.lock();
        let pixels = Source {
            bytes: Vec::new(),
            on_drop: Some(Box::new(move || {
                record.store(
                    probe.upgrade().is_some_and(|slot| slot.lock_is_held()),
                    Ordering::SeqCst,
                );
            })),
        };
        drop(pixels);
        drop(guard);
        assert!(held.load(Ordering::SeqCst));
    }

    #[test]
    fn publishing_never_waits_for_a_consumer_that_never_reads() {
        // Nothing reads the slot: every publish must still return. A hang is a failure, found by the
        // timeout and not by an assertion about time.
        let slot = MetalQuadSlot::new();
        let (done, finished) = mpsc::channel();
        let publisher = {
            let slot = Arc::clone(&slot);
            thread::spawn(move || {
                for generation in 0..20_000 {
                    slot.publish(scene(generation));
                }
                done.send(()).expect("the test is listening");
            })
        };
        finished
            .recv_timeout(Duration::from_secs(60))
            .expect("a publish waited for a consumer");
        publisher.join().expect("the publisher finishes");
        assert_eq!(held(&slot), (19_999, true));
    }

    #[test]
    fn a_slot_can_be_published_to_from_many_threads_while_it_is_read() {
        let slot = MetalQuadSlot::new();
        let publishers: Vec<_> = (0..4_u64)
            .map(|index| {
                let slot = Arc::clone(&slot);
                thread::spawn(move || {
                    for step in 0..2_000 {
                        slot.publish(scene(index * 10_000 + step));
                    }
                })
            })
            .collect();
        let reader = {
            let slot = Arc::clone(&slot);
            thread::spawn(move || {
                let mut last = 0;
                for _ in 0..2_000 {
                    last = slot.latest().generation();
                }
                last
            })
        };
        for publisher in publishers {
            publisher.join().expect("a publisher finishes");
        }
        reader.join().expect("the reader finishes");
        assert!(slot.latest().quad().is_some());
    }
}
