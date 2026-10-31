//! The Vulkan sink and its conversion (feature `vulkan`): everything of a `Gpu2d` backend over `ck-gpu-vulkan` except the
//! presenter. [`VulkanSinkSide`] makes the sink, authenticates it and holds the once-only refusal; the application's
//! backend (milestone 4.5, issue 9) owns the presenter and starts it with the slot this side hands back.
//!
//! This module holds **no Vulkan object, makes no Vulkan call and has no `unsafe`**: it only turns a [`Snapshot`] into
//! the GPU crate's plain `VulkanScene` and publishes it to a `VulkanQuadSlot`. It does not depend on a window crate. It
//! is written to the same contracts as the Metal sink (`metal.rs`, CK-92 and CK-94) but shares no code with it.
//!
//! - **The conversion.** A snapshot becomes a `VulkanScene`: the same generation (with or without a quad; "no quad" stays
//!   "no quad"); the quad's image as a `PixelSource` (a newtype over the `Arc<Image>`, so no pixels are copied, and one
//!   wrapper per image, so the presenter's upload-by-identity sees one source however often the quad moves or changes
//!   filter); the destination and source rectangles field for field; the filter. `VulkanQuad::new` validates it.
//! - **A stable source identity.** The presenter recognises its resident texture by the *outer* `Arc<dyn PixelSource>`
//!   allocation and the layout, so the sink reuses the same wrapper for the same image: one cache entry of **weak**
//!   references (the image it last wrapped, and that wrapper). When a snapshot's image is the same `Arc<Image>` (by
//!   pointer) and the wrapper still upgrades, that wrapper is published; otherwise a new one is made. The cache holds
//!   nothing strongly, so it adds nothing to the residency bound.
//! - **Nothing is freed, logged, blocked on or panicked inside `publish`** (it runs under the bridge's lock). It
//!   converts, publishes the scene to the slot (which wakes only the render thread) and hands the snapshot back as the
//!   value not kept. A conversion the GPU crate refuses is not expected: placement (`Quad::place`) already treats any
//!   result the GPU types would refuse as not placed. This is the defence in depth: the sink then publishes "no quad"
//!   with that generation, records the reason in the once-only refusal latch and calls the loop's wake (the notifier's:
//!   it only sets a flag and posts a native wake). The loop's next pass calls the backend's `take_report` on the main
//!   thread, which returns [`VulkanSinkSide::take_report`]'s refusal once as a `Failed` report, before anything the
//!   presenter has. The tests force a refusal through a test-only conversion seam.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use ck_gpu_vulkan::{
    DestinationRect, PixelSource, SourceRect, VulkanFilter, VulkanQuad, VulkanQuadSlot, VulkanScene,
};
use ck_window::PresenterReport;

use crate::{Filter, Image, QuadSink, Snapshot};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An image as the GPU crate's pixel source: the same bytes, shared.
#[derive(Debug)]
pub(crate) struct ImagePixels(pub(crate) Arc<Image>);

impl PixelSource for ImagePixels {
    fn pixels(&self) -> &[u8] {
        self.0.pixels()
    }
}

/// The refusals the sink recorded, for the next report: the **once-only refusal latch**.
///
/// The first reason and "it was reported" are **one state under one mutex**: [`take`](Self::take) consumes the reason
/// and marks the refusal reported inside a single critical section, so no [`record`](Self::record) can observe a state
/// between the two. The run's first reason is returned exactly once and every other reason, before or after that
/// `take`, is rejected.
#[derive(Debug, Default)]
pub(crate) struct Refusals {
    state: Mutex<RefusalState>,
    /// Test-only scheduling seam, run once at the consumption boundary of [`take`](Self::take).
    #[cfg(test)]
    boundary: Boundary,
}

/// What [`Refusals`] holds.
#[derive(Debug, Default)]
struct RefusalState {
    /// The first reason, until it is taken.
    first: Option<String>,
    /// The first reason was taken: nothing is recorded any more.
    reported: bool,
}

/// The test-only hook of [`Refusals`], run at the boundary between consuming the first reason and marking the refusal
/// reported.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Boundary(Mutex<Option<Box<dyn FnOnce() + Send>>>);

#[cfg(test)]
impl std::fmt::Debug for Boundary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Boundary")
    }
}

impl Refusals {
    /// Records a refusal (the first one is kept; one report per run is enough to fail it).
    pub(crate) fn record(&self, message: String) {
        let mut state = lock(&self.state);
        if state.first.is_none() && !state.reported {
            state.first = Some(message);
        }
    }

    /// Makes `hook` run once, at the point of the next [`take`](Self::take) after it has consumed the first reason
    /// and marked the refusal reported. For the tests only: it is not part of any API.
    #[cfg(test)]
    pub(crate) fn at_boundary(&self, hook: impl FnOnce() + Send + 'static) {
        *lock(&self.boundary.0) = Some(Box::new(hook));
    }

    /// The refusal to report, once.
    pub(crate) fn take(&self) -> Option<String> {
        // One critical section: consuming the first reason and marking the refusal reported.
        let taken = {
            let mut state = lock(&self.state);
            let taken = state.first.take();
            if taken.is_some() {
                state.reported = true;
            }
            taken
        };
        // The boundary of a split latch's race (one that released its guard between consuming the reason and
        // marking the refusal reported): the test seam sits here, where a `record` from another thread can run to
        // completion and must be rejected.
        #[cfg(test)]
        if let Some(hook) = lock(&self.boundary.0).take() {
            hook();
        }
        taken
    }
}

/// The scene a snapshot becomes, and the pixel wrapper it used (kept to be reused for the same image).
pub(crate) type Converted = (VulkanScene, Option<Arc<ImagePixels>>);

fn vulkan_filter(filter: Filter) -> VulkanFilter {
    match filter {
        Filter::Nearest => VulkanFilter::Nearest,
        Filter::Linear => VulkanFilter::Linear,
    }
}

/// Turns a snapshot into a scene. `reuse` is the wrapper of the same image, if the previous snapshot had it.
///
/// # Errors
///
/// The GPU crate's reason, if it refuses the quad.
fn convert(snapshot: &Snapshot, reuse: Option<Arc<ImagePixels>>) -> Result<Converted, String> {
    let Some(placed) = &snapshot.quad else {
        return Ok((VulkanScene::new(snapshot.generation, None), None));
    };
    let wrapper = reuse.unwrap_or_else(|| Arc::new(ImagePixels(Arc::clone(&placed.image))));
    let destination = DestinationRect {
        left: placed.destination.left,
        top: placed.destination.top,
        right: placed.destination.right,
        bottom: placed.destination.bottom,
    };
    let source = SourceRect {
        left: placed.source.left,
        top: placed.source.top,
        right: placed.source.right,
        bottom: placed.source.bottom,
    };
    let pixels: Arc<dyn PixelSource> = Arc::clone(&wrapper) as Arc<dyn PixelSource>;
    let quad = VulkanQuad::new(
        pixels,
        placed.image.width(),
        placed.image.height(),
        destination,
        source,
        vulkan_filter(snapshot.filter),
    )
    .map_err(|error| format!("the Vulkan backend refused a quad: {error}"))?;
    Ok((
        VulkanScene::new(snapshot.generation, Some(quad)),
        Some(wrapper),
    ))
}

/// What the sink remembers between snapshots: **weak** references to the image it last wrapped and to that wrapper.
/// It holds nothing strongly, so it adds nothing to the residency bound; the GPU slot's current scene holds the wrapper
/// while that scene is current, so the wrapper upgrades exactly when the same image is still published, and the same
/// wrapper (the same outer `Arc<dyn PixelSource>` allocation, which is what the presenter recognises its resident
/// texture by) goes out again.
#[derive(Debug)]
pub(crate) struct Conversion {
    cache: Option<(Weak<Image>, Weak<ImagePixels>)>,
    converter: Converter,
}

/// What turns a snapshot into a scene: [`convert`], except in the tests that need the GPU crate to refuse a quad.
pub(crate) type Converter = fn(&Snapshot, Option<Arc<ImagePixels>>) -> Result<Converted, String>;

impl Default for Conversion {
    fn default() -> Self {
        Self {
            cache: None,
            converter: convert,
        }
    }
}

/// What publishing one snapshot decided.
pub(crate) struct Step {
    /// The scene for the GPU slot.
    pub(crate) scene: VulkanScene,
    /// Why the GPU crate refused the quad, if it did (the scene then has no quad).
    pub(crate) refusal: Option<String>,
}

impl Conversion {
    /// The wrapper cached for exactly this image, if it is still alive.
    fn cached(&self, image: &Arc<Image>) -> Option<Arc<ImagePixels>> {
        let (cached_image, cached_wrapper) = self.cache.as_ref()?;
        std::ptr::addr_eq(cached_image.as_ptr(), Arc::as_ptr(image))
            .then(|| cached_wrapper.upgrade())
            .flatten()
    }

    /// Takes the next snapshot. It never panics, logs or blocks, and drops no image: it holds none.
    pub(crate) fn next(&mut self, snapshot: &Snapshot) -> Step {
        let reuse = snapshot
            .quad
            .as_ref()
            .and_then(|placed| self.cached(&placed.image));
        let (scene, wrapper, refusal) = match (self.converter)(snapshot, reuse) {
            Ok((scene, wrapper)) => (scene, wrapper, None),
            Err(message) => (
                VulkanScene::new(snapshot.generation, None),
                None,
                Some(message),
            ),
        };
        self.cache = match (&snapshot.quad, wrapper) {
            (Some(placed), Some(wrapper)) => {
                Some((Arc::downgrade(&placed.image), Arc::downgrade(&wrapper)))
            }
            _ => None,
        };
        Step { scene, refusal }
    }
}

/// The sink over a `VulkanQuadSlot`.
pub(crate) struct VulkanSink {
    slot: Arc<VulkanQuadSlot>,
    refusals: Arc<Refusals>,
    wake: Arc<dyn Fn() + Send + Sync>,
    conversion: Mutex<Conversion>,
}

impl std::fmt::Debug for VulkanSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VulkanSink")
    }
}

impl QuadSink for VulkanSink {
    /// Converts and publishes. A refusal is recorded and the loop's wake is called (it only sets a flag and posts a
    /// native wake), so an idle loop calls `take_report` on its main thread, which reports it. Nothing is logged,
    /// blocked on or freed here.
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        let step = lock(&self.conversion).next(&snapshot);
        let refused = step.refusal.is_some();
        if let Some(message) = step.refusal {
            self.refusals.record(message);
        }
        self.slot.publish(step.scene);
        if refused {
            (self.wake)();
        }
        // Nothing of the snapshot is kept (the cache is weak), so it goes back to the caller as the value not kept,
        // to be dropped after the bridge's lock.
        Some(snapshot)
    }
}

#[cfg(test)]
impl VulkanSink {
    /// A sink over a fresh slot, for the tests that need none of the side.
    pub(crate) fn for_test() -> Arc<Self> {
        Arc::new(Self {
            slot: VulkanQuadSlot::new(),
            refusals: Arc::new(Refusals::default()),
            wake: Arc::new(|| {}),
            conversion: Mutex::new(Conversion::default()),
        })
    }
}

/// The sink side of the Vulkan backend: the slot, the sink [`new_sink`](Self::new_sink) made, the recorded refusals and
/// the loop's wake. It is everything of the backend except the presenter, and it holds no presenter, no window handle
/// and no GPU object. The application's `Gpu2d` backend owns one: its `new_sink` returns this side's, its `start` passes
/// the slot [`accept`](Self::accept) gives to `VulkanPresenter::start_2d`, and its `take_report` returns
/// [`take_report`](Self::take_report) before anything the presenter has.
pub struct VulkanSinkSide {
    wake: Arc<dyn Fn() + Send + Sync>,
    slot: Option<Arc<VulkanQuadSlot>>,
    made: Option<Weak<VulkanSink>>,
    refusals: Arc<Refusals>,
    converter: Converter,
}

impl std::fmt::Debug for VulkanSinkSide {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VulkanSinkSide")
    }
}

impl VulkanSinkSide {
    /// A side whose refusals call `wake`: the loop's wake (`ck_window`'s notifier wake), which only sets a flag and
    /// posts a native wake. It must be short and must not block.
    #[must_use]
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::with_converter(wake, convert)
    }

    /// With `converter` in place of the real conversion: the test seam that makes the GPU crate's refusal happen on
    /// demand, since placement no longer hands it a quad it would refuse.
    pub(crate) fn with_converter(wake: Arc<dyn Fn() + Send + Sync>, converter: Converter) -> Self {
        Self {
            wake,
            slot: None,
            made: None,
            refusals: Arc::new(Refusals::default()),
            converter,
        }
    }

    /// A new `VulkanQuadSlot` and the sink over it; the sink replaces any earlier one as "the one this side made".
    pub fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        let slot = VulkanQuadSlot::new();
        let sink = Arc::new(VulkanSink {
            slot: Arc::clone(&slot),
            refusals: Arc::clone(&self.refusals),
            wake: Arc::clone(&self.wake),
            conversion: Mutex::new(Conversion {
                cache: None,
                converter: self.converter,
            }),
        });
        self.slot = Some(slot);
        self.made = Some(Arc::downgrade(&sink));
        sink
    }

    /// The slot behind `sink`, if `sink` is the one [`new_sink`](Self::new_sink) made.
    ///
    /// # Errors
    ///
    /// Why `sink` is refused: it is not the one this side made (a presenter is started with its own sink or not at
    /// all), or no sink was made.
    pub fn accept(&self, sink: &Arc<dyn QuadSink>) -> Result<Arc<VulkanQuadSlot>, String> {
        if !self.made(sink) {
            return Err("the sink is not the one this backend made".to_owned());
        }
        self.slot
            .clone()
            .ok_or_else(|| "the backend was started without a sink".to_owned())
    }

    /// Whether `sink` is the one [`new_sink`](Self::new_sink) made.
    pub(crate) fn made(&self, sink: &Arc<dyn QuadSink>) -> bool {
        self.made
            .as_ref()
            .is_some_and(|made| std::ptr::addr_eq(made.as_ptr(), Arc::as_ptr(sink)))
    }

    /// The report a recorded refusal becomes, once: `Failed` with its reason. The backend returns it before anything
    /// the presenter has.
    #[must_use]
    pub fn take_report(&self) -> Option<PresenterReport> {
        self.refusals
            .take()
            .map(|message| PresenterReport::Failed { message })
    }
}
