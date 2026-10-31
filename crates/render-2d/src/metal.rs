//! The Metal backend (feature `metal`, macOS): [`MetalBackend`] is a [`Gpu2d`] over `ck-gpu-metal`'s
//! `MetalPresenter`, with its textured quad.
//!
//! - **Construction is `unsafe`**: [`MetalBackend::new`] takes the contract that the backend is driven only as the
//!   presenter of a driver that keeps the window's layer alive until `poll_end` returns `Proven` (as
//!   `ck_window::run` does). Its `start` calls `MetalPresenter::start_2d` under that contract. The constructors'
//!   contracts and the presenter start calls (`start_2d`, `start_scene` and the hooked start of the tests) are this
//!   module's only `unsafe` sites.
//! - **The sink.** [`Gpu2d::new_sink`] makes a `MetalQuadSlot` and returns a sink over it; `start` passes the same
//!   slot to `start_2d` and refuses any other sink. The sink turns each [`Snapshot`] into a `MetalScene`: the same
//!   generation; the quad's image as a `PixelSource` (a newtype over the `Arc<Image>`, so no pixels are copied, and
//!   one wrapper per image, so the GPU crate's upload-by-identity sees one source however often the quad moves);
//!   the destination and source rectangles; the filter. "No quad" stays "no quad".
//! - **`MetalBackend::with_resources`** (milestone 5, issue 5b-2) draws the bridge's published scene frames through the
//!   resource service: it starts the presenter with `start_scene` over a `MetalFrameSlot<TextureKey>` and CK-115's
//!   `MetalResidentDriver`, and connects the service's [`WakeRelay`] to the presenter's `texture_wake()` only once the
//!   presenter exists (a wake before that stays pending in the relay) and disconnects it on every path that ends the
//!   backend. Its sink converts each snapshot in O(1) into a `MetalFrame` (the legacy quad exactly as above, the scene as a
//!   [`SceneFrameSource`](crate::metal_scene::SceneFrameSource) over the snapshot's `Arc<SceneFrame>` and metrics); the
//!   quads are placed on the render thread at frame build. The sink keeps the snapshot it last published and returns the
//!   previous one, so no scene frame is ever freed under the bridge's lock.
//! - **A stable source identity.** The presenter recognises its resident texture by the *outer*
//!   `Arc<dyn PixelSource>`'s allocation and the layout, so the sink reuses the same wrapper for the same image: it
//!   keeps one cache entry of **weak** references (the image it last wrapped, and that wrapper). When a snapshot's
//!   image is the same `Arc<Image>` (by pointer) and the wrapper still upgrades, that wrapper is published;
//!   otherwise a new one is made. The cache holds nothing strongly, so it adds nothing to the residency bound.
//! - **Nothing is freed, logged, blocked on or panicked inside `publish`** (it runs under the bridge's lock). A
//!   conversion the GPU crate refuses is not expected: placement (`Quad::place`) already treats any result the GPU
//!   types would refuse (an empty destination, a source span that is empty as `f32`) as not placed. This is the
//!   defence in depth: if one happens, the sink publishes "no quad" with that generation, records the refusal and
//!   calls the loop's wake (the notifier's: it only sets a flag and posts a native wake, and its pending flag
//!   makes it sticky). The loop's next pass calls [`Gpu2d::take_report`] on the main thread, which returns the
//!   refusal once as a `Failed` report. The tests force it through a test-only conversion seam.

use std::ffi::c_void;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use ck_gpu_metal::{
    BindlessLimits, CleanupResult as GpuCleanup, DestinationRect, EndError, FrameStats,
    LedgerLimits, MetalContext, MetalFilter, MetalFrame, MetalFrameSlot, MetalPresenter, MetalQuad,
    MetalQuadSlot, MetalScene, MetalSceneSource, MetricsEndGuard, MetricsReading, MetricsWindow,
    PixelSource, PresenterReport as GpuReport, RequestError, SceneLimits as GpuSceneLimits,
    SceneSetup, SceneStats, SourceRect, TestHooks as PresenterTestHooks, TextureLimits,
    TextureTestHooks,
};
use ck_resources::{ResourceService, TextureKey};
use ck_window::{CleanupResult, PixelSize, PresenterReport, WindowDesc};
use ck_window_cocoa::CocoaHandles;

use crate::metal_resident::MetalResidentDriver;
use crate::metal_scene::SceneFrameSource;
use crate::{Filter, Gpu2d, Image, QuadSink, Snapshot, WakeRelay};

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

/// The refusals the sink recorded, for the next report.
///
/// The first reason and "it was reported" are **one state under one mutex**: [`take`](Self::take) consumes the
/// reason and marks the refusal reported inside a single critical section, so no [`record`](Self::record) can
/// observe a state between the two. The run's first reason is returned exactly once and every other reason, before
/// or after that `take`, is rejected.
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

/// The test-only hook of [`Refusals`], run between consuming the first reason and marking the refusal reported.
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

    /// Makes `hook` run once, at the point of the next [`take`](Self::take) between consuming the first reason
    /// and marking the refusal reported. For the tests only: it is not part of any API.
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
        // The boundary of the accepted code's race (it released its guard between consuming the reason and
        // marking the refusal reported): the test seam sits here, after the critical section, where a `record`
        // from another thread can run to completion and must be rejected.
        #[cfg(test)]
        if let Some(hook) = lock(&self.boundary.0).take() {
            hook();
        }
        taken
    }
}

/// The scene a snapshot becomes, and the pixel wrapper it used (kept to be reused for the same image).
pub(crate) type Converted = (MetalScene, Option<Arc<ImagePixels>>);

fn metal_filter(filter: Filter) -> MetalFilter {
    match filter {
        Filter::Nearest => MetalFilter::Nearest,
        Filter::Linear => MetalFilter::Linear,
    }
}

/// Turns a snapshot into a scene. `reuse` is the wrapper of the same image, if the previous snapshot had it.
///
/// # Errors
///
/// The GPU crate's reason, if it refuses the quad.
fn convert(snapshot: &Snapshot, reuse: Option<Arc<ImagePixels>>) -> Result<Converted, String> {
    let Some(placed) = &snapshot.quad else {
        return Ok((MetalScene::new(snapshot.generation, None), None));
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
    let quad = MetalQuad::new(
        pixels,
        placed.image.width(),
        placed.image.height(),
        destination,
        source,
        metal_filter(snapshot.filter),
    )
    .map_err(|error| format!("the Metal backend refused a quad: {error}"))?;
    Ok((
        MetalScene::new(snapshot.generation, Some(quad)),
        Some(wrapper),
    ))
}

/// What the sink remembers between snapshots: **weak** references to the image it last wrapped and to that
/// wrapper. It holds nothing strongly, so it adds nothing to the residency bound; the GPU slot's current scene
/// holds the wrapper while that scene is current, so the wrapper upgrades exactly when the same image is still
/// published, and the same wrapper (the same outer `Arc<dyn PixelSource>` allocation, which is what the
/// presenter recognises its resident texture by) goes out again.
#[derive(Debug)]
pub(crate) struct Conversion {
    cache: Option<(Weak<Image>, Weak<ImagePixels>)>,
    converter: Converter,
}

/// What turns a snapshot into a scene: [`convert`], except in the tests that need the GPU crate to refuse a quad.
pub(crate) type Converter = fn(&Snapshot, Option<Arc<ImagePixels>>) -> Result<Converted, String>;

/// The real conversion (tests build sinks with it).
#[cfg(test)]
pub(crate) fn real_converter() -> Converter {
    convert
}

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
    pub(crate) scene: MetalScene,
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
                MetalScene::new(snapshot.generation, None),
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

/// The sink over a `MetalQuadSlot`.
pub(crate) struct MetalSink {
    slot: Arc<MetalQuadSlot>,
    refusals: Arc<Refusals>,
    wake: Arc<dyn Fn() + Send + Sync>,
    conversion: Mutex<Conversion>,
}

impl std::fmt::Debug for MetalSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MetalSink")
    }
}

impl MetalSink {
    fn over(
        slot: Arc<MetalQuadSlot>,
        refusals: Arc<Refusals>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self {
            slot,
            refusals,
            wake,
            conversion: Mutex::new(Conversion::default()),
        })
    }
}

impl QuadSink for MetalSink {
    /// Converts and publishes. A refusal is recorded and the loop's wake is called (it only sets a flag and posts
    /// a native wake), so an idle loop calls `take_report` on its main thread, which reports it. Nothing is
    /// logged, blocked on or freed here.
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
        // Nothing of the snapshot is kept (the cache is weak), so it goes back to the caller as the value
        // not kept, to be dropped after the bridge's lock.
        Some(snapshot)
    }
}

/// The sink of a `with_resources` backend, over a `MetalFrameSlot`.
pub(crate) struct SceneSink {
    slot: Arc<MetalFrameSlot<TextureKey>>,
    refusals: Arc<Refusals>,
    wake: Arc<dyn Fn() + Send + Sync>,
    conversion: Mutex<Conversion>,
    /// The snapshot last published, kept so that the one it replaces goes back to the bridge to be dropped after its lock.
    last: Mutex<Option<Snapshot>>,
    /// Called after a frame is built and before it is exchanged into the slot (tests only: the interval in which the
    /// renderer may still draw the previous frame).
    before_exchange: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Counts the placements the scene sources made (tests only).
    #[cfg(test)]
    placements: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

impl std::fmt::Debug for SceneSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SceneSink")
    }
}

impl SceneSink {
    pub(crate) fn over(
        slot: Arc<MetalFrameSlot<TextureKey>>,
        refusals: Arc<Refusals>,
        wake: Arc<dyn Fn() + Send + Sync>,
        converter: Converter,
        before_exchange: Option<Arc<dyn Fn() + Send + Sync>>,
        #[cfg(test)] placements: Option<Arc<std::sync::atomic::AtomicUsize>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            slot,
            refusals,
            wake,
            conversion: Mutex::new(Conversion {
                cache: None,
                converter,
            }),
            last: Mutex::new(None),
            before_exchange,
            #[cfg(test)]
            placements,
        })
    }

    /// The frame a snapshot becomes: its generation (that of `scene`), filter and converted legacy quad, and its scene as a
    /// shared source. O(1).
    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "only the tests' placement counter is read from the sink"
        )
    )]
    pub(crate) fn frame_for(
        &self,
        snapshot: &Snapshot,
        scene: &MetalScene,
    ) -> MetalFrame<TextureKey> {
        MetalFrame::new(
            scene.generation(),
            metal_filter(snapshot.filter),
            scene.quad().cloned(),
            Self::source_of(
                snapshot,
                #[cfg(test)]
                self.placements.as_ref(),
            ),
        )
        // The compatibility quad's key as plain data, in O(1), whether or not it places anything.
        .with_compat_key(
            snapshot
                .scene
                .as_ref()
                .and_then(|frame| frame.compat())
                .map(|quad| quad.texture.key()),
        )
    }

    /// The scene source of `snapshot`: its frame and the metrics it was made with, shared, not placed.
    fn source_of(
        snapshot: &Snapshot,
        #[cfg(test)] placements: Option<&Arc<std::sync::atomic::AtomicUsize>>,
    ) -> Option<Arc<dyn MetalSceneSource<TextureKey>>> {
        let frame = Arc::clone(snapshot.scene.as_ref()?);
        #[cfg(test)]
        if let Some(counter) = placements {
            return Some(Arc::new(SceneFrameSource::counting(
                frame,
                snapshot.metrics,
                Arc::clone(counter),
            )));
        }
        Some(Arc::new(SceneFrameSource::new(frame, snapshot.metrics)))
    }
}

impl QuadSink for SceneSink {
    /// Converts in O(1) and publishes: the generation, the filter, the legacy quad converted as `MetalSink` does, and
    /// the scene as a shared source (no quad is placed, no texture resolved, nothing allocated in proportion to the
    /// scene). The snapshot is kept; the previous one is returned, so the bridge drops it after releasing its lock and
    /// the slot's drop of the replaced frame is never the last owner of a `SceneFrame`.
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        let step = lock(&self.conversion).next(&snapshot);
        let refused = step.refusal.is_some();
        if let Some(message) = step.refusal {
            self.refusals.record(message);
        }
        let frame = self.frame_for(&snapshot, &step.scene);
        drop(step.scene);
        if let Some(hook) = &self.before_exchange {
            hook();
        }
        self.slot.publish(frame);
        let previous = lock(&self.last).replace(snapshot);
        if refused {
            (self.wake)();
        }
        previous
    }
}

/// A sink over a caller's slot, with the same conversion as [`MetalBackend`]'s own: for the tests that start a
/// presenter themselves to watch uploads. Not part of the interface.
#[doc(hidden)]
#[must_use]
pub fn sink_over_slot(
    slot: Arc<MetalQuadSlot>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> Arc<dyn QuadSink> {
    MetalSink::over(slot, Arc::new(Refusals::default()), wake)
}

#[cfg(test)]
impl MetalSink {
    /// A sink over a fresh slot, for the tests that need none of the backend.
    pub(crate) fn for_test() -> Arc<Self> {
        Self::over(
            MetalQuadSlot::new(),
            Arc::new(Refusals::default()),
            Arc::new(|| {}),
        )
    }
}

/// The sink side of the backend: the slot, the sink `new_sink` made, the recorded refusals and the wake. It is
/// everything of [`MetalBackend`] except the presenter, so the tests can run it under a sleeping loop with fake
/// handles (and a forced conversion) without a window or a layer.
pub(crate) struct SinkSide {
    wake: Arc<dyn Fn() + Send + Sync>,
    slot: Option<Arc<MetalQuadSlot>>,
    /// The frame slot of a `with_resources` backend (its sinks are scene sinks).
    frames: Option<Arc<MetalFrameSlot<TextureKey>>>,
    /// Whether `new_sink` makes scene sinks (a `with_resources` backend).
    scenes: bool,
    made: Option<Weak<dyn QuadSink>>,
    refusals: Arc<Refusals>,
    converter: Converter,
    /// The scene sinks' pause between building a frame and exchanging it (tests only).
    before_exchange: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Counts the placements of the scene sources the sink made (tests only).
    #[cfg(test)]
    placements: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

impl SinkSide {
    fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::with_converter(wake, convert)
    }

    /// With `converter` in place of the real conversion: the test seam that makes the GPU crate's refusal happen
    /// on demand, since placement no longer hands it a quad it would refuse.
    pub(crate) fn with_converter(wake: Arc<dyn Fn() + Send + Sync>, converter: Converter) -> Self {
        Self {
            wake,
            slot: None,
            frames: None,
            scenes: false,
            made: None,
            refusals: Arc::new(Refusals::default()),
            converter,
            before_exchange: None,
            #[cfg(test)]
            placements: None,
        }
    }

    /// The side of a `with_resources` backend: its sinks publish frames to a `MetalFrameSlot`.
    fn for_scenes(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::for_scenes_with_converter(wake, convert)
    }

    /// The same with `converter` in place of the real conversion (the tests' refusal seam).
    pub(crate) fn for_scenes_with_converter(
        wake: Arc<dyn Fn() + Send + Sync>,
        converter: Converter,
    ) -> Self {
        Self {
            scenes: true,
            ..Self::with_converter(wake, converter)
        }
    }

    /// The side of a `with_resources` backend with the real conversion, for the tests.
    #[cfg(test)]
    pub(crate) fn for_scenes_for_test(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::for_scenes(wake)
    }

    /// A new frame slot and the scene sink over it.
    fn new_scene_sink(&mut self) -> Arc<dyn QuadSink> {
        let slot = MetalFrameSlot::new();
        let sink = SceneSink::over(
            Arc::clone(&slot),
            Arc::clone(&self.refusals),
            Arc::clone(&self.wake),
            self.converter,
            self.before_exchange.clone(),
            #[cfg(test)]
            self.placements.clone(),
        );
        self.frames = Some(slot);
        let made: Weak<dyn QuadSink> = Arc::downgrade(&sink) as Weak<dyn QuadSink>;
        self.made = Some(made);
        sink
    }

    /// The frame slot behind `sink`, if `sink` is the one [`new_sink`](Self::new_sink) made.
    ///
    /// # Errors
    ///
    /// The reason it is refused: the sink is not the one this backend made, or none was made.
    pub(crate) fn accept_frames(
        &self,
        sink: &Arc<dyn QuadSink>,
    ) -> Result<Arc<MetalFrameSlot<TextureKey>>, String> {
        if !self.made(sink) {
            return Err("the sink is not the one this backend made".to_owned());
        }
        self.frames
            .clone()
            .ok_or_else(|| "the backend was started without a sink".to_owned())
    }

    /// A new slot and the sink over it; the sink replaces any earlier one as "the one this side made".
    pub(crate) fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        if self.scenes {
            return self.new_scene_sink();
        }
        let slot = MetalQuadSlot::new();
        let sink = Arc::new(MetalSink {
            slot: Arc::clone(&slot),
            refusals: Arc::clone(&self.refusals),
            wake: Arc::clone(&self.wake),
            conversion: Mutex::new(Conversion {
                cache: None,
                converter: self.converter,
            }),
        });
        self.slot = Some(slot);
        let made: Weak<dyn QuadSink> = Arc::downgrade(&sink) as Weak<dyn QuadSink>;
        self.made = Some(made);
        sink
    }

    /// The slot behind `sink`, if `sink` is the one [`new_sink`](Self::new_sink) made; an error otherwise (a
    /// presenter is started with its own sink or not at all).
    pub(crate) fn accept(&self, sink: &Arc<dyn QuadSink>) -> Result<Arc<MetalQuadSlot>, String> {
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

    /// The report a recorded refusal becomes, once: `Failed` with its reason. `take_report` returns it before
    /// anything the presenter has.
    pub(crate) fn report(&self) -> Option<PresenterReport> {
        self.refusals
            .take()
            .map(|message| PresenterReport::Failed { message })
    }
}

/// The limits of a [`MetalBackend::with_resources`] backend: the texture store's (CK-112) and the ledger's (CK-113), the
/// most quads one frame draws, and the budget the scene's instance buffer is charged to (CK-116).
#[derive(Debug, Clone, Copy)]
pub struct ResourceBackendLimits {
    /// The upload store's limits.
    pub texture: TextureLimits,
    /// The revision ledger's limits (`max_adoptions_per_frame` is also the resident driver's adoption limit).
    pub ledger: LedgerLimits,
    /// The most quads one frame draws (`[render] max_quads`): the instance buffer's room.
    pub max_quads: usize,
    /// The budget the instance buffer's measured allocation must fit (`[render] binding_geometry_bytes`).
    pub binding_geometry_bytes: u64,
    /// The bindless table (`[render] bindless_texture_slots` with the `[resources]` capacities it must cover): the scene is
    /// drawn through it, one draw. There is no way to leave it out here; the ordinary per-texture path is the hidden
    /// `BackendTestHooks::ordinary_baseline`.
    pub bindless: BindlessLimits,
}

/// What a `with_resources` backend needs besides its sink side.
struct Resources {
    service: ResourceService,
    relay: WakeRelay,
    context: Arc<MetalContext>,
    limits: ResourceBackendLimits,
    hooks: Option<BackendTestHooks>,
}

/// The fault switches of a `with_resources` backend's presenter, for the tests: the presenter's own hooks (its observer,
/// its counters and logs) and those of the checker's upload. Not part of the interface.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct BackendTestHooks {
    /// The presenter's hooks; its `scene` is set by the backend.
    pub presenter: PresenterTestHooks,
    /// The checker upload's fault switches.
    pub checker: Option<TextureTestHooks>,
    /// The upload store's fault switches (the images of the scene's textures and of the compatibility quad).
    pub textures: Option<TextureTestHooks>,
    /// Called by the scene sink after it built a frame and before it exchanged it into the frame slot, under the bridge's
    /// lock (the producer's pause: the renderer keeps drawing the frame already in the slot meanwhile).
    pub before_exchange: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Draws the scene by CK-116's ordinary per-texture batches instead of through the bindless table: the baseline M5-10b
    /// measures the same content against, and the pre-bindless cases. Reachable only through these hooks (the application never
    /// names them; the hook-isolation guard rejects the name there).
    pub ordinary_baseline: bool,
}

/// The presenter's frame-timing types, which the crate re-exports (`crate::timing` names them for the application).
pub(crate) mod timing {
    pub use ck_gpu_metal::{
        EndError, FrameIntervals, FrameStats, Histogram, MetricsEndGuard, MetricsReading,
        MetricsWindow, Percentile, RequestError, SceneStats, Tracked, Windows, now_ns,
    };
}

/// `Gpu2d` over the Metal presenter.
pub struct MetalBackend {
    side: SinkSide,
    presenter: Option<MetalPresenter>,
    /// Present for a [`with_resources`](Self::with_resources) backend.
    resources: Option<Resources>,
}

impl std::fmt::Debug for MetalBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MetalBackend")
    }
}

impl MetalBackend {
    /// A backend whose render thread calls `wake` after each report and after its cleanup result (it must be
    /// short and must not block; it is `ck_window`'s notifier wake).
    ///
    /// # Safety
    ///
    /// The backend must be driven only as the presenter of a driver that keeps the window's `CAMetalLayer` alive
    /// until [`Gpu2d::poll_end`] has returned `Proven` (or the process ends), as `ck_window::run` does: `start`
    /// hands the layer from the window's handles to `MetalPresenter::start_2d` under exactly that contract.
    #[must_use]
    pub unsafe fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            side: SinkSide::new(wake),
            presenter: None,
            resources: None,
        }
    }

    /// A backend that draws the bridge's published scene frames through the resource service.
    ///
    /// `service` is a `Backend`-mode service and `relay` the [`WakeRelay`] it was built with. `start` starts the presenter
    /// with `start_scene` on `context` (a frame slot, CK-115's `MetalResidentDriver` over `service`, `limits`), and only
    /// once the presenter exists connects `relay` to its `texture_wake()`; the relay is disconnected on every path that ends
    /// the backend (a refused or failed `start`, and the end). [`MetalBackend::new`] is unchanged and keeps drawing only the
    /// legacy quad.
    ///
    /// # Safety
    ///
    /// The same contract as [`MetalBackend::new`]: the backend must be driven only as the presenter of a driver that keeps
    /// the window's `CAMetalLayer` alive until [`Gpu2d::poll_end`] has returned `Proven` (or the process ends); `start`
    /// hands that layer to `MetalPresenter::start_scene` under exactly that contract.
    #[must_use]
    pub unsafe fn with_resources(
        wake: Arc<dyn Fn() + Send + Sync>,
        service: ResourceService,
        relay: WakeRelay,
        context: Arc<MetalContext>,
        limits: ResourceBackendLimits,
    ) -> Self {
        Self {
            side: SinkSide::for_scenes(wake),
            presenter: None,
            resources: Some(Resources {
                service,
                relay,
                context,
                limits,
                hooks: None,
            }),
        }
    }

    /// [`with_resources`](Self::with_resources) whose presenter is started with test hooks (the checker's fault switches and
    /// the presenter's observer). Not part of the interface.
    ///
    /// # Safety
    ///
    /// The same contract as [`with_resources`](Self::with_resources).
    #[doc(hidden)]
    #[must_use]
    pub unsafe fn with_resources_and_hooks(
        wake: Arc<dyn Fn() + Send + Sync>,
        service: ResourceService,
        relay: WakeRelay,
        context: Arc<MetalContext>,
        limits: ResourceBackendLimits,
        hooks: BackendTestHooks,
    ) -> Self {
        // SAFETY: the contract is this function's own.
        let mut backend = unsafe { Self::with_resources(wake, service, relay, context, limits) };
        backend
            .side
            .before_exchange
            .clone_from(&hooks.before_exchange);
        if let Some(resources) = &mut backend.resources {
            resources.hooks = Some(hooks);
        }
        backend
    }

    /// The presenter's counters and charges once it is started (a `with_resources` backend draws a scene): per committed
    /// frame the latest value and the peak, the instance buffer against its budget, the checker's baseline bytes.
    #[must_use]
    pub fn scene_stats(&self) -> Option<SceneStats> {
        self.presenter.as_ref().map(MetalPresenter::stats)
    }

    /// The measurement windows of the presenter (milestone 5, issue 10b-1): these pass `MetalPresenter`'s own through, so the
    /// application defines one window and sends it to the presenter and the window loop. Each returns `None` before the presenter
    /// is started (nothing is held back or retried here: the caller asks again once the window is open).
    ///
    /// Requests `window` from the presenter's render thread, which adopts it at its next boundary.
    #[must_use]
    pub fn request_metrics(&self, window: MetricsWindow) -> Option<Result<(), RequestError>> {
        self.presenter
            .as_ref()
            .map(|presenter| presenter.request_metrics(window))
    }

    /// Sets the end of window `id` (once).
    #[must_use]
    pub fn end_metrics(&self, id: u32, end: u64) -> Option<Result<(), EndError>> {
        self.presenter
            .as_ref()
            .map(|presenter| presenter.end_metrics(id, end))
    }

    /// The presenter's admission lock, for setting the end of a window while nothing is admitted (see [`MetricsEndGuard`]).
    #[must_use]
    pub fn lock_metrics_end(&self) -> Option<MetricsEndGuard<'_>> {
        self.presenter
            .as_ref()
            .map(MetalPresenter::lock_metrics_end)
    }

    /// When the render thread adopted window `id`.
    #[must_use]
    pub fn metrics_adopted(&self, id: u32) -> Option<u64> {
        self.presenter
            .as_ref()
            .and_then(|presenter| presenter.metrics_adopted(id))
    }

    /// A copy of window `id` as it stands, without waiting.
    #[must_use]
    pub fn metrics_snapshot(&self, id: u32) -> Option<MetricsReading> {
        self.presenter
            .as_ref()
            .and_then(|presenter| presenter.metrics_snapshot(id))
    }

    /// Window `id` once its work is consumed, waiting at most `within`.
    #[must_use]
    pub fn read_metrics(&self, id: u32, within: Duration) -> Option<MetricsReading> {
        self.presenter
            .as_ref()
            .and_then(|presenter| presenter.read_metrics(id, within))
    }

    /// The whole run's figures so far.
    #[must_use]
    pub fn metrics_totals(&self) -> Option<FrameStats> {
        self.presenter.as_ref().map(MetalPresenter::metrics_totals)
    }

    /// Disconnects the service's wake relay from the presenter (a `with_resources` backend; idempotent).
    fn disconnect_relay(&self) {
        if let Some(resources) = &self.resources {
            resources.relay.disconnect();
        }
    }

    /// Starts the presenter of a `with_resources` backend on `layer`.
    fn start_scene_on(
        &mut self,
        layer: *mut c_void,
        desc: &WindowDesc,
        pixel_size: PixelSize,
        sink: &Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        let frames = self.side.accept_frames(sink)?;
        let resources = self
            .resources
            .as_ref()
            .ok_or_else(|| "the backend has no resource service".to_owned())?;
        let limits = resources.limits;
        let scene = GpuSceneLimits {
            max_quads: limits.max_quads,
            binding_geometry_bytes: limits.binding_geometry_bytes,
        };
        let driver = MetalResidentDriver::new(
            resources.service.clone(),
            limits.ledger.max_adoptions_per_frame,
        );
        let context = Arc::clone(&resources.context);
        let wake = Arc::clone(&self.side.wake);
        let started = match &resources.hooks {
            None => {
                // SAFETY: `layer` is the live `CAMetalLayer` of the window the driver holds, which by this type's
                // constructor contract outlives the presenter (it is destroyed only after a `Proven` end), as
                // `MetalPresenter::start_scene` requires.
                unsafe {
                    MetalPresenter::start_scene_bindless(
                        layer,
                        (pixel_size.width, pixel_size.height),
                        desc.clear,
                        wake,
                        frames,
                        context,
                        limits.texture,
                        limits.ledger,
                        limits.bindless,
                        scene,
                        driver,
                    )
                }
            }
            Some(hooks) => {
                let mut presenter_hooks = hooks.presenter.clone();
                presenter_hooks.scene = Some(SceneSetup::new_with_bindless_limits(
                    context,
                    limits.texture,
                    limits.ledger,
                    (!hooks.ordinary_baseline).then_some(limits.bindless),
                    scene,
                    driver,
                    frames,
                    hooks.textures.clone(),
                    hooks.checker.clone(),
                ));
                // SAFETY: the same contract as `start_scene` above.
                unsafe {
                    MetalPresenter::start_with_test_hooks(
                        layer,
                        (pixel_size.width, pixel_size.height),
                        desc.clear,
                        wake,
                        presenter_hooks,
                    )
                }
            }
        };
        let presenter = started.map_err(|error| error.to_string())?;
        // Only now, with the presenter alive, can its weak texture wake be obtained: a wake before this point stayed
        // pending in the relay and is delivered once by `connect`.
        resources.relay.connect(presenter.texture_wake());
        self.presenter = Some(presenter);
        Ok(())
    }

    /// `start` on a raw layer pointer: the refused or failed start leaves the relay disconnected. Not part of the
    /// interface: the tests start a backend with a null layer, which the presenter refuses before it touches anything.
    ///
    /// # Safety
    ///
    /// `layer` must be null or the live `CAMetalLayer` of the window the driver holds until the presenter's end is proven,
    /// as for `MetalPresenter::start_scene`.
    #[doc(hidden)]
    pub unsafe fn start_on_layer(
        &mut self,
        layer: *mut c_void,
        desc: &WindowDesc,
        pixel_size: PixelSize,
        sink: &Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        if self.resources.is_some() {
            let result = self.start_scene_on(layer, desc, pixel_size, sink);
            if result.is_err() {
                self.disconnect_relay();
            }
            return result;
        }
        let slot = self.side.accept(sink)?;
        // SAFETY: `layer` is the live `CAMetalLayer` of the window the driver holds, which by this type's
        // constructor contract outlives the presenter (it is destroyed only after a `Proven` end), as
        // `MetalPresenter::start_2d` requires.
        let started = unsafe {
            MetalPresenter::start_2d(
                layer,
                (pixel_size.width, pixel_size.height),
                desc.clear,
                Arc::clone(&self.side.wake),
                slot,
            )
        };
        self.presenter = Some(started.map_err(|error| error.to_string())?);
        Ok(())
    }

    /// The generation of the last frame committed (with a quad or without), once the presenter is started: `None`
    /// before the first frame.
    #[must_use]
    pub fn drawn_generation(&self) -> Option<u64> {
        self.presenter
            .as_ref()
            .and_then(MetalPresenter::drawn_generation)
    }

    /// Whether `sink` is the one [`Gpu2d::new_sink`] made.
    #[cfg(test)]
    pub(crate) fn made(&self, sink: &Arc<dyn QuadSink>) -> bool {
        self.side.made(sink)
    }

    /// The slot behind `sink`, or why it is refused.
    #[cfg(test)]
    pub(crate) fn accept(&self, sink: &Arc<dyn QuadSink>) -> Result<Arc<MetalQuadSlot>, String> {
        self.side.accept(sink)
    }
}

impl Gpu2d for MetalBackend {
    type Handles = CocoaHandles;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        self.side.new_sink()
    }

    fn start(
        &mut self,
        handles: CocoaHandles,
        desc: &WindowDesc,
        pixel_size: PixelSize,
        sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        // SAFETY: `handles.layer()` is the live `CAMetalLayer` of the window the driver holds, which by this type's
        // constructor contract outlives the presenter (it is destroyed only after a `Proven` end).
        unsafe { self.start_on_layer(handles.layer(), desc, pixel_size, &sink) }
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        if let Some(presenter) = &self.presenter {
            presenter.set_clear(rgb);
        }
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        if let Some(presenter) = &self.presenter {
            presenter.set_pixel_size(width, height);
        }
    }

    fn request_redraw(&mut self) {
        if let Some(presenter) = &self.presenter {
            presenter.request_redraw();
        }
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        if let Some(report) = self.side.report() {
            return Some(report);
        }
        self.presenter
            .as_ref()
            .and_then(MetalPresenter::take_report)
            .map(|report| match report {
                GpuReport::FirstFrameQueued => PresenterReport::FirstFrameQueued,
                GpuReport::Failed { message } => PresenterReport::Failed { message },
            })
    }

    fn request_stop(&mut self) {
        if let Some(presenter) = &self.presenter {
            presenter.request_stop();
        }
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        // A presenter that was never started has nothing to prove.
        let Some(presenter) = &mut self.presenter else {
            return Some(CleanupResult::Proven);
        };
        let ended = presenter.poll_end(within).map(|result| match result {
            GpuCleanup::Proven => CleanupResult::Proven,
            GpuCleanup::Unproven { reason } => CleanupResult::Unproven { reason },
        });
        if ended.is_some() {
            // The presenter is over: no service wake reaches it any more.
            self.disconnect_relay();
        }
        ended
    }
}

impl Drop for MetalBackend {
    fn drop(&mut self) {
        // However the backend ends, the service's wakes stop being delivered to a presenter it owned.
        self.disconnect_relay();
    }
}
