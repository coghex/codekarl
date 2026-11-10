//! The scene bridge: a compatibility quad plus persistent scene frames, written from the UI VM's thread.
//!
//! One small lock covers the compatibility quad, latest immutable scene frame, [`Metrics`], [`Filter`], a
//! generation, the attached [`QuadSink`] and the phase. Scene edits stay in the one writer; publication builds
//! their frame outside this lock and exchanges its `Arc` inside it. Every bridge change goes through the lock:
//!
//! - a scene write ([`SceneWriter::set_quad`], [`SceneWriter::clear_quad`]) from the UI VM's thread;
//! - [`SceneBridge::attach`] from the renderer's `start`, which publishes the current scene under the same
//!   lock, so a write made between `start` and attach cannot be stranded;
//! - [`SceneBridge::set_metrics`], a metrics update (points and pixels together);
//! - [`SceneBridge::detach`], which removes the sink and ends the bridge.
//!
//! **The map lane** (milestone 6, slice 1) sits beside the scene: [`SceneBridge::publish_map`] stores the loaded map once and
//! [`SceneBridge::publish_camera`] the view, each under the same lock and published to the sink with the same rules. An input
//! that is not newer than the stored one, and a camera computed for other metrics than the bridge's
//! ([`SceneBridge::metrics_generation`]), are refused as stale and change nothing; a detached bridge refuses both. The snapshot
//! carries a map only while the input, a camera for its own metrics and a placement exist, so with no map input the frame is
//! exactly the frame without one.
//!
//! A change that alters either lane or the metrics increments the generation and, when attached, publishes the
//! snapshot to the sink **while the lock is held**. Snapshots therefore reach the sink in generation order, and
//! a publication made with old metrics can never overwrite a newer one.
//!
//! **One producer.** The bridge hands out exactly one [`SceneWriter`] ([`SceneBridge::take_writer`]; taking it
//! twice is an error). The writer is `Send` but neither `Sync` nor `Clone`, so all scene writes come from one
//! thread at a time: the UI VM's thread, the only writer in the design.
//!
//! **Nothing is freed under the lock, and nothing outlives the write.** A quad (and so an image) that a write
//! replaces or removes, the snapshot the sink hands back for the one it replaced, a write that arrives after
//! the end, and a detached sink are all dropped after the lock is released, before the call returns. Nothing
//! decodes or allocates image data under the lock. With one writer that bounds the decoded images the renderer
//! owns at once: the scene, the sink's latest snapshot, the one the render thread holds while it uploads (a
//! backend drops its CPU reference once the upload is done), and the writer's own, which is either the image
//! it is decoding or the one it swapped out and is about to drop. That is at most four.
//!
//! **Writes after the end.** After [`SceneBridge::detach`] the phase is [`Phase::Ended`]. A scene write is
//! still accepted and counted, changes nothing, publishes nothing and is also counted as discarded, so the
//! run's `debug` record can show it. The scene survives until the bridge is dropped.

use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use ck_resources::{RegistryId, ResourceState, TextureRef};
use ck_window::{PixelSize, WindowDesc};

use crate::filter::Filter;
use crate::group::{GroupError, GroupKey, PointRect};
use crate::hit::{HitError, HitRegions};
use crate::map::{
    MapCamera, MapError, MapFrame, MapInput, MapMode, MapPublication, MapRefusals, place_map,
};
use crate::quad::{Metrics, Quad, QuadError, Snapshot};
use crate::scene::{EditScene, FrameQuad, QuadKey, SceneFrame, SceneLimits, SceneLimitsError};

/// Where the backend receives snapshots: its latest-value slot.
///
/// A sink holds plain data and no GPU object, so one kept alive after teardown can touch nothing on the
/// GPU. The bridge calls [`publish`](Self::publish) with its lock held, so an implementation:
///
/// - never waits (not for a frame, an upload or a stalled render thread): it swaps the value in and wakes the
///   consumer;
/// - never calls back into the bridge;
/// - never frees an image inside `publish`: it hands back the snapshot it replaced, if there was one, and the
///   bridge drops it after releasing its lock. An image buffer must not be freed while the lock is held, and a
///   replaced snapshot kept for later would let images pile up behind a stalled consumer.
pub trait QuadSink: Send + Sync {
    /// Replaces the sink's value with `snapshot`, returning the value it replaced.
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot>;
}

/// Where the bridge is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No sink yet: writes change the scene and publish nothing.
    Before,
    /// A sink is attached: every change is published.
    Attached,
    /// The sink was detached: writes are discarded.
    Ended,
}

/// How many scene writes were made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WriteCounts {
    /// Every scene write, whatever became of it.
    pub accepted: u64,
    /// The writes that arrived after the end (a subset of `accepted`).
    pub discarded: u64,
    /// Scene-frame publications accepted before or while attached.
    pub published: u64,
    /// Scene-frame publication attempts after the bridge ended, including calls racing detach.
    pub publication_discarded: u64,
}

/// Why a sink could not be attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachRefused {
    /// The phase the bridge was in; a sink attaches only in [`Phase::Before`].
    pub phase: Phase,
}

impl std::fmt::Display for AttachRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the scene bridge cannot attach in phase {:?}",
            self.phase
        )
    }
}

impl std::error::Error for AttachRefused {}

struct State {
    scene: Option<Quad>,
    frame: Option<Arc<SceneFrame>>,
    hit_regions: Option<Arc<HitRegions>>,
    map: Option<Arc<MapInput>>,
    camera: Option<Arc<MapCamera>>,
    mode: Option<Arc<MapMode>>,
    /// What the presenter reports of refusing to make a map image (shared with every snapshot's map).
    map_refusals: Arc<MapRefusals>,
    /// Counts the metrics the bridge has had (attach and every change of the window's metrics); a camera is for exactly one of them.
    metrics_generation: u64,
    metrics: Option<Metrics>,
    /// The configured UI scale limit as `k` (2, 3 or 4), if there is one: the UI scale of a window opened in pixels is the cap of its drawable
    /// or this, whichever is lower.
    ui_scale_limit: Option<u32>,
    /// The window was opened in screen pixels (its description carries a pixel request): its UI scene is in UI units.
    ui_units: bool,
    filter: Filter,
    generation: u64,
    sink: Option<Arc<dyn QuadSink>>,
    phase: Phase,
    counts: WriteCounts,
    writer_taken: bool,
}

/// What a change displaced: dropped by the caller after the lock is released.
#[derive(Default)]
#[must_use = "the displaced values are to be dropped after the lock is released"]
struct Displaced {
    quad: Option<Quad>,
    snapshot: Option<Snapshot>,
    sink: Option<Arc<dyn QuadSink>>,
    frame: Option<Arc<SceneFrame>>,
    hit_regions: Option<Arc<HitRegions>>,
    map: Option<Arc<MapInput>>,
    camera: Option<Arc<MapCamera>>,
    mode: Option<Arc<MapMode>>,
}

impl State {
    /// `metrics` with the UI scale this window's drawable allows: `Some` for a window opened in pixels, `None` for one opened in points.
    fn with_ui_scale(&self, metrics: Metrics) -> Metrics {
        Metrics {
            ui_scale_k: self
                .ui_units
                .then(|| crate::ui_scale::scale_k(metrics.pixels, self.ui_scale_limit)),
            ..metrics
        }
    }

    /// Publishes the current snapshot, when a sink is attached, and returns the one the sink replaced. Called
    /// with the lock held.
    fn publish(&self) -> Option<Snapshot> {
        let (Some(sink), Some(metrics)) = (&self.sink, &self.metrics) else {
            return None;
        };
        let quad = self.scene.as_ref().and_then(|quad| quad.place(metrics));
        sink.publish(Snapshot {
            map: self.map_frame(metrics),
            generation: self.generation,
            filter: self.filter,
            requested: self.scene.is_some(),
            quad,
            scene: self.frame.as_ref().map(Arc::clone),
            metrics: *metrics,
        })
    }

    /// The stored mode, when it is the elevation mode or has a table that fits `input`'s owner grid (a newer map with other places leaves an older table unused).
    fn mode_for(&self, input: &MapInput) -> Option<Arc<MapMode>> {
        let mode = self.mode.as_ref()?;
        if mode.is_elevation() {
            return Some(Arc::clone(mode));
        }
        let values = mode.values()?;
        let owners = input.owners()?;
        (values.len() == owners.places()).then(|| Arc::clone(mode))
    }

    /// The map lane's contribution to a snapshot made with `metrics`: the input and a camera computed for these very metrics, placed.
    /// A camera computed for earlier metrics (a resize since) contributes nothing until the matching one is published, so a frame never
    /// pairs a view size with metrics it was not computed for.
    fn map_frame(&self, metrics: &Metrics) -> Option<MapFrame> {
        let input = self.map.as_ref()?;
        let camera = self.camera.as_ref()?;
        if camera.metrics_generation() != self.metrics_generation {
            return None;
        }
        Some(MapFrame {
            input: Arc::clone(input),
            camera: Arc::clone(camera),
            placement: place_map(camera.view(), metrics),
            mode: self.mode_for(input),
            refusals: Arc::clone(&self.map_refusals),
        })
    }
}

/// The scene bridge of one run.
pub struct SceneBridge {
    state: Mutex<State>,
    registry: RegistryId,
    limits: SceneLimits,
    #[cfg(test)]
    live_frames: Arc<AtomicUsize>,
}

impl std::fmt::Debug for SceneBridge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.lock();
        formatter
            .debug_struct("SceneBridge")
            .field("phase", &state.phase)
            .field("generation", &state.generation)
            .field("counts", &state.counts)
            .finish_non_exhaustive()
    }
}

impl SceneBridge {
    /// A bridge in [`Phase::Before`] with no quad, sampling with `filter`.
    ///
    /// # Panics
    ///
    /// Panics only if the process-wide registry identity space is exhausted.
    #[must_use]
    pub fn new(filter: Filter) -> Arc<Self> {
        Self::with_limits(filter, SceneLimits::DEFAULT).expect("the default scene limits are valid")
    }

    /// A bridge with a caller-selected maximum number of live quads.
    ///
    /// # Errors
    ///
    /// Returns a [`SceneLimitsError`] naming `max_quads` when it is outside 1..=1,048,576.
    ///
    /// # Panics
    ///
    /// Panics only if the process-wide registry identity space is exhausted.
    pub fn with_limits(filter: Filter, limits: SceneLimits) -> Result<Arc<Self>, SceneLimitsError> {
        let limits = limits.validate()?;
        let registry =
            RegistryId::allocate().expect("the process-wide scene identity counter is available");
        #[cfg(test)]
        let live_frames = Arc::new(AtomicUsize::new(0));
        Ok(Arc::new(Self {
            registry,
            limits,
            #[cfg(test)]
            live_frames,
            state: Mutex::new(State {
                scene: None,
                frame: None,
                hit_regions: None,
                map: None,
                camera: None,
                mode: None,
                map_refusals: Arc::new(MapRefusals::default()),
                metrics_generation: 0,
                metrics: None,
                ui_scale_limit: None,
                ui_units: false,
                filter,
                generation: 0,
                sink: None,
                phase: Phase::Before,
                counts: WriteCounts::default(),
                writer_taken: false,
            }),
        }))
    }

    /// The configured scene capacity.
    #[must_use]
    pub fn limits(&self) -> SceneLimits {
        self.limits
    }

    /// The state, even after a panic elsewhere left the lock poisoned: every change leaves it consistent.
    fn lock(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// The one writer, for the UI VM's thread.
    ///
    /// # Errors
    ///
    /// [`WriterTaken`] if it was taken before: a bridge has exactly one producer.
    pub fn take_writer(self: &Arc<Self>) -> Result<SceneWriter, WriterTaken> {
        let mut state = self.lock();
        if state.writer_taken {
            return Err(WriterTaken);
        }
        state.writer_taken = true;
        Ok(SceneWriter {
            bridge: Arc::clone(self),
            edits: EditScene::new(self.registry, self.limits.max_quads, self.limits.max_groups),
            compat: RefCell::new(None),
            dirty: Cell::new(false),
            frame_number: 0,
            spare: Vec::new(),
            #[cfg(test)]
            frame_counter: Arc::clone(&self.live_frames),
            #[cfg(test)]
            build_probe: None,
            #[cfg(test)]
            frame_drop_hook: None,
            not_sync: PhantomData,
        })
    }

    /// Clones the latest immutable hit regions, if the scene has been published.
    #[must_use]
    pub fn hit_regions(&self) -> Option<Arc<HitRegions>> {
        self.lock().hit_regions.as_ref().map(Arc::clone)
    }

    /// Attaches the sink and publishes the current scene to it.
    ///
    /// # Errors
    ///
    /// [`AttachRefused`] unless the bridge is still in [`Phase::Before`]; nothing changes.
    pub fn attach(
        &self,
        desc: &WindowDesc,
        pixels: PixelSize,
        sink: Arc<dyn QuadSink>,
    ) -> Result<(), AttachRefused> {
        // Declared before the guard, so that on every path, an unwinding one included, it is dropped after it.
        let mut displaced = Displaced::default();
        {
            let mut state = self.lock();
            if state.phase != Phase::Before {
                return Err(AttachRefused { phase: state.phase });
            }
            state.ui_units = desc.pixel_request.is_some();
            state.metrics = Some(state.with_ui_scale(Metrics::new(desc, pixels)));
            state.metrics_generation += 1;
            state.sink = Some(sink);
            state.phase = Phase::Attached;
            state.generation += 1;
            displaced.snapshot = state.publish();
        }
        drop(displaced);
        Ok(())
    }

    /// Replaces the window's metrics, its size in points and the drawable's size in pixels together, counts one new metrics
    /// generation and publishes the scene converted with them. Nothing happens unless a sink is attached.
    pub fn set_metrics(&self, points: (u32, u32), pixels: PixelSize) {
        let mut displaced = Displaced::default();
        {
            let mut state = self.lock();
            if state.phase != Phase::Attached {
                return;
            }
            if let Some(metrics) = state.metrics {
                let replaced = Metrics {
                    points_width: points.0,
                    points_height: points.1,
                    pixels,
                    ..metrics
                };
                state.metrics = Some(state.with_ui_scale(replaced));
                state.metrics_generation += 1;
            }
            state.generation += 1;
            displaced.snapshot = state.publish();
        }
        drop(displaced);
    }

    /// The UI scale as `k` for a drawable of `pixels`, when the window was opened in screen pixels (its UI scene is in UI units); `None` for a
    /// window opened in points, whose scene is in points.
    #[must_use]
    pub fn ui_scale(&self, pixels: PixelSize) -> Option<u32> {
        let state = self.lock();
        state
            .ui_units
            .then(|| crate::ui_scale::scale_k(pixels, state.ui_scale_limit))
    }

    /// Limits the UI scale of a window opened in screen pixels to `k` (2, 3 or 4: the configured scale 1, 1.5 or 2), or lifts the limit. It
    /// is read at startup and applies to the metrics set after it.
    pub fn set_ui_scale_limit(&self, limit_k: Option<u32>) {
        self.lock().ui_scale_limit = limit_k;
    }

    /// Removes the sink and ends the bridge. Nothing is published afterwards, and it is idempotent.
    pub fn detach(&self) {
        let mut displaced = Displaced::default();
        {
            let mut state = self.lock();
            if state.phase != Phase::Ended {
                state.generation += 1;
            }
            displaced.frame = state.frame.take();
            displaced.hit_regions = state.hit_regions.take();
            displaced.map = state.map.take();
            displaced.camera = state.camera.take();
            displaced.mode = state.mode.take();
            // Replace the consumer's latest snapshot before detaching it, so an idle sink cannot pin the scene frame or the map.
            if state.phase == Phase::Attached
                && (displaced.frame.is_some() || displaced.map.is_some())
            {
                displaced.snapshot = state.publish();
            }
            state.phase = Phase::Ended;
            // The sink may hold images: it goes after the lock does.
            displaced.sink = state.sink.take();
        }
        drop(displaced);
    }

    /// How many metrics the bridge has had: it grows by one at [`attach`](Self::attach) and at every
    /// [`set_metrics`](Self::set_metrics). A [`MapCamera`] is computed for one of them.
    #[must_use]
    pub fn metrics_generation(&self) -> u64 {
        self.lock().metrics_generation
    }

    /// A refusal of the presenter to make a map image (the grids or the target), once per episode: `None` when there is none to
    /// report. It is not a failure of the run: the presenter drew the frame without the map and tries again on its next frame; a
    /// later success ends the episode and the next refusal is a new message.
    pub fn take_map_refusal(&self) -> Option<String> {
        let refusals = Arc::clone(&self.lock().map_refusals);
        refusals.take()
    }

    /// Publishes the loaded map (once per loaded map): stored, and sent to the sink when one is attached.
    ///
    /// [`MapPublication::Stale`] when `input` is not newer than the stored one (its generation is not larger), and
    /// [`MapPublication::Detached`] after [`detach`](Self::detach); neither changes anything. Before a sink is attached the input is
    /// stored and goes out at [`attach`](Self::attach), like a scene write.
    pub fn publish_map(&self, input: Arc<MapInput>) -> MapPublication {
        // What the publication displaces, or is itself when refused, is dropped after the lock is released.
        let mut displaced = Displaced::default();
        let outcome = {
            let mut state = self.lock();
            if state.phase == Phase::Ended {
                displaced.map = Some(input);
                MapPublication::Detached
            } else if state
                .map
                .as_ref()
                .is_some_and(|held| input.generation() <= held.generation())
            {
                displaced.map = Some(input);
                MapPublication::Stale
            } else {
                displaced.map = state.map.replace(input);
                state.generation += 1;
                displaced.snapshot = state.publish();
                MapPublication::Published
            }
        };
        drop(displaced);
        outcome
    }

    /// Publishes the camera: stored, and sent to the sink when one is attached.
    ///
    /// [`MapPublication::Stale`] when the camera was computed for other metrics than the bridge's now
    /// ([`metrics_generation`](Self::metrics_generation)), and [`MapPublication::Detached`] after [`detach`](Self::detach); neither
    /// changes anything.
    pub fn publish_camera(&self, camera: Arc<MapCamera>) -> MapPublication {
        let mut displaced = Displaced::default();
        let outcome = {
            let mut state = self.lock();
            if state.phase == Phase::Ended {
                displaced.camera = Some(camera);
                MapPublication::Detached
            } else if camera.metrics_generation() != state.metrics_generation {
                displaced.camera = Some(camera);
                MapPublication::Stale
            } else {
                displaced.camera = state.camera.replace(camera);
                state.generation += 1;
                displaced.snapshot = state.publish();
                MapPublication::Published
            }
        };
        drop(displaced);
        outcome
    }

    /// Publishes the map mode (the values each place shows, or off): stored, and sent to the sink when one is attached.
    ///
    /// # Errors
    ///
    /// With a map published, [`MapError::ModeWithoutOwners`] for a table when the map has no owner grid and
    /// [`MapError::ModeTableLength`] when the table's length is not the map's place count. A refused mode changes nothing: the lane
    /// keeps the mode it accepted before. (A table published before the map is held, and carried only while it fits the map's places.)
    ///
    /// # Returns
    ///
    /// [`MapPublication::Stale`] when `mode` is not newer than the stored one (its revision is not larger), and
    /// [`MapPublication::Detached`] after [`detach`](Self::detach); neither changes anything.
    pub fn publish_mode(&self, mode: Arc<MapMode>) -> Result<MapPublication, MapError> {
        let mut displaced = Displaced::default();
        let outcome = {
            let mut state = self.lock();
            if let (Some(values), Some(input)) = (mode.values(), state.map.as_ref()) {
                match input.owners() {
                    None => return Err(MapError::ModeWithoutOwners),
                    Some(owners) if owners.places() != values.len() => {
                        return Err(MapError::ModeTableLength {
                            expected: owners.places(),
                            actual: values.len(),
                        });
                    }
                    Some(_) => {}
                }
            }
            if state.phase == Phase::Ended {
                displaced.mode = Some(mode);
                MapPublication::Detached
            } else if state
                .mode
                .as_ref()
                .is_some_and(|held| mode.revision() <= held.revision())
            {
                displaced.mode = Some(mode);
                MapPublication::Stale
            } else {
                displaced.mode = state.mode.replace(mode);
                state.generation += 1;
                displaced.snapshot = state.publish();
                MapPublication::Published
            }
        };
        drop(displaced);
        Ok(outcome)
    }

    /// Stages the initial camera before the first presenter attach. The caller computes it from the accepted window description;
    /// its metrics generation must be the one the first `attach` will create. This lets the first rendered frame contain the complete
    /// map instead of briefly showing an empty map lane.
    ///
    /// Returns [`MapPublication::Stale`] once a presenter has attached, when metrics already exist, or when the camera does not name
    /// exactly the next metrics generation. A detached bridge returns [`MapPublication::Detached`].
    pub fn stage_initial_camera(&self, camera: Arc<MapCamera>) -> MapPublication {
        let mut displaced = Displaced::default();
        let outcome = {
            let mut state = self.lock();
            if state.phase == Phase::Ended {
                displaced.camera = Some(camera);
                MapPublication::Detached
            } else if state.sink.is_some()
                || state.metrics.is_some()
                || camera.metrics_generation() != state.metrics_generation.saturating_add(1)
            {
                displaced.camera = Some(camera);
                MapPublication::Stale
            } else {
                displaced.camera = state.camera.replace(camera);
                state.generation = state.generation.saturating_add(1);
                MapPublication::Published
            }
        };
        drop(displaced);
        outcome
    }

    /// Where the bridge is.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.lock().phase
    }

    /// How many scene writes were made and how many were discarded, for the run's `debug` record.
    #[must_use]
    pub fn counts(&self) -> WriteCounts {
        self.lock().counts
    }

    fn write(&self, quad: Option<Quad>) {
        // Whatever the write displaces, or is itself when it is discarded, leaves the lock to be dropped. The
        // binding is declared before the guard and filled before the sink is called: if the sink panics, the
        // unwind drops the guard first and this second, so even then nothing is freed under the lock.
        let mut displaced = Displaced::default();
        {
            let mut state = self.lock();
            state.counts.accepted += 1;
            if state.phase == Phase::Ended {
                state.counts.discarded += 1;
                displaced.quad = quad;
            } else if quad.is_some() || state.scene.is_some() {
                state.generation += 1;
                displaced.quad = std::mem::replace(&mut state.scene, quad);
                displaced.snapshot = state.publish();
            }
        }
        drop(displaced);
    }
}

#[cfg(test)]
impl SceneBridge {
    /// Whether another holder has the lock right now (tests only: a drop probe).
    pub(crate) fn lock_is_held(&self) -> bool {
        matches!(
            self.state.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        )
    }

    /// Runs `body` with the lock held (tests only).
    pub(crate) fn with_lock_held<R>(&self, body: impl FnOnce() -> R) -> R {
        let _guard = self.lock();
        body()
    }

    /// Whether a panic poisoned the lock (tests only).
    pub(crate) fn is_poisoned(&self) -> bool {
        self.state.is_poisoned()
    }

    /// Number of live immutable frames for this bridge (tests only).
    pub(crate) fn live_frames(&self) -> usize {
        self.live_frames.load(Ordering::SeqCst)
    }
}

/// Why the writer cannot be taken: it already was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriterTaken;

impl std::fmt::Display for WriterTaken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the scene bridge's one writer was already taken")
    }
}

impl std::error::Error for WriterTaken {}

/// How the UI VM's thread writes the scene. It never waits for the render thread.
///
/// There is one per bridge ([`SceneBridge::take_writer`]). It can be moved to the thread that writes, but it
/// cannot be shared between threads or copied, so scene writes never overlap:
///
/// ```
/// fn moved<T: Send>() {}
/// moved::<ck_render_2d::SceneWriter>();
/// ```
///
/// ```compile_fail,E0277
/// fn shared<T: Sync>() {}
/// shared::<ck_render_2d::SceneWriter>();
/// ```
///
/// ```compile_fail,E0277
/// fn copied<T: Clone>() {}
/// copied::<ck_render_2d::SceneWriter>();
/// ```
#[derive(Debug)]
pub struct SceneWriter {
    bridge: Arc<SceneBridge>,
    edits: EditScene,
    compat: RefCell<Option<FrameQuad>>,
    dirty: Cell<bool>,
    frame_number: u64,
    spare: Vec<FrameQuad>,
    #[cfg(test)]
    frame_counter: Arc<AtomicUsize>,
    #[cfg(test)]
    build_probe: Option<BuildProbe>,
    #[cfg(test)]
    frame_drop_hook: Option<crate::scene::FrameDropHook>,
    /// `Cell` is `Send` but not `Sync`, which makes the writer the same.
    not_sync: PhantomData<Cell<()>>,
}

impl SceneWriter {
    /// Replaces or removes one ordered pointer hit key in the next scene publication.
    ///
    /// `group` is the group the key belongs to, or the base group when `None`; a key keeps the group it was first set in, and
    /// removing it needs no group (a group that is named must be the key's own). The rectangles are in window points: each must be
    /// finite, have a positive size and keep an extent in pointer units, and every edge, as set and as translated by the group's
    /// current offset, must lie in the point domain ([`crate::POINT_DOMAIN`]). Nothing changes on a refusal.
    ///
    /// # Errors
    ///
    /// Returns a [`HitError`] if the key, the group, a rectangle, the rectangle count or the resulting scene cap is invalid.
    pub fn set_hit(
        &mut self,
        key: &str,
        rects: Option<Vec<PointRect>>,
        group: Option<GroupKey>,
    ) -> Result<(), HitError> {
        if self.edits.set_hit(key, rects, group)? {
            self.dirty.set(true);
        }
        Ok(())
    }

    /// Sets the one quad, replacing any earlier one.
    pub fn set_quad(&self, quad: Quad) {
        self.bridge.write(Some(quad));
    }

    /// Removes the quad. With none, it changes nothing (and is counted like any write).
    pub fn clear_quad(&self) {
        self.bridge.write(None);
    }

    /// Sets the one compatibility quad below every persistent scene quad, replacing the previous compatibility entry.
    ///
    /// # Errors
    ///
    /// Returns `QuadError` when the rectangle is invalid. The existing compatibility entry is unchanged on error.
    pub fn set_compat(
        &self,
        texture: TextureRef<ResourceState>,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<(), QuadError> {
        crate::quad::validate_rect(x, y, width, height)?;
        let replacement = FrameQuad {
            texture,
            x,
            y,
            width,
            height,
            clip: None,
            tint: crate::Tint::WHITE,
        };
        let displaced;
        {
            let mut state = self.bridge.lock();
            state.counts.accepted += 1;
            if state.phase == Phase::Ended {
                state.counts.discarded += 1;
                displaced = Some(replacement);
            } else {
                displaced = self.compat.replace(Some(replacement));
                self.dirty.set(true);
            }
        }
        drop(displaced);
        Ok(())
    }

    /// Removes only the compatibility quad. Persistent frame quads keep their order and contents.
    pub fn clear_compat(&self) {
        let displaced;
        {
            let mut state = self.bridge.lock();
            state.counts.accepted += 1;
            if state.phase == Phase::Ended {
                state.counts.discarded += 1;
                displaced = None;
            } else {
                displaced = self.compat.replace(None);
                if displaced.is_some() {
                    self.dirty.set(true);
                }
            }
        }
        drop(displaced);
    }

    /// Whether the bridge has ended: a write is then discarded and counted. A caller that does slow work before it writes
    /// (the legacy lane's synchronous decode) reads this to skip admitting what would be discarded; the writer checks again
    /// when it installs.
    #[must_use]
    pub fn is_ended(&self) -> bool {
        self.bridge.phase() == Phase::Ended
    }

    /// Counts a legacy call that was validated and discarded after the bridge or service ended.
    pub(crate) fn count_discarded_compat_write(&self) {
        let mut state = self.bridge.lock();
        state.counts.accepted += 1;
        state.counts.discarded += 1;
    }

    /// Creates a persistent quad on top of all currently live quads.
    ///
    /// # Errors
    ///
    /// Returns `QuadError` if geometry is invalid, the scene is full, or the texture reference is invalid.
    pub fn create_quad(
        &mut self,
        texture: TextureRef<ResourceState>,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<QuadKey, QuadError> {
        let key = self.edits.create(texture, x, y, width, height, None)?;
        self.dirty.set(true);
        Ok(key)
    }

    /// Creates a persistent quad on top of the quads of `group` (the base group when `None`), in that group's own order.
    ///
    /// # Errors
    ///
    /// Returns `QuadError` if geometry is invalid, the group is not usable or the scene is full.
    pub fn create_quad_in(
        &mut self,
        group: Option<GroupKey>,
        texture: TextureRef<ResourceState>,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<QuadKey, QuadError> {
        let key = self.edits.create(texture, x, y, width, height, group)?;
        self.dirty.set(true);
        Ok(key)
    }

    /// Creates an empty, visible group with offset (0, 0) and no clip, on top of all groups.
    ///
    /// # Errors
    ///
    /// [`GroupError::Full`] when the scene already has `max_groups` groups.
    pub fn create_group(&mut self) -> Result<GroupKey, GroupError> {
        let key = self.edits.create_group()?;
        self.dirty.set(true);
        Ok(key)
    }

    /// Puts the group on top of all groups.
    ///
    /// # Errors
    ///
    /// [`GroupError`] for a stale or foreign key.
    pub fn raise_group(&mut self, key: GroupKey) -> Result<(), GroupError> {
        self.edits.raise_group(key)?;
        self.dirty.set(true);
        Ok(())
    }

    /// Puts the group just above the base group.
    ///
    /// # Errors
    ///
    /// [`GroupError`] for a stale or foreign key.
    pub fn lower_group(&mut self, key: GroupKey) -> Result<(), GroupError> {
        self.edits.lower_group(key)?;
        self.dirty.set(true);
        Ok(())
    }

    /// Puts the listed groups on top in that order (the last topmost), as one edit.
    ///
    /// # Errors
    ///
    /// [`GroupError`] if the list is empty, names a group twice or names a group that is not live; nothing changes.
    pub fn raise_groups(&mut self, keys: &[GroupKey]) -> Result<(), GroupError> {
        self.edits.raise_groups(keys)?;
        self.dirty.set(true);
        Ok(())
    }

    /// Shows or hides a group. A hidden group's quads keep their order, offset and clip but are left out of the frame.
    ///
    /// # Errors
    ///
    /// [`GroupError`] for a stale or foreign key.
    pub fn set_group_visible(&mut self, key: GroupKey, visible: bool) -> Result<(), GroupError> {
        self.edits.set_group_visible(key, visible)?;
        self.dirty.set(true);
        Ok(())
    }

    /// Moves a group by a whole number of points: every quad of the group is translated by it when a frame is built.
    ///
    /// # Errors
    ///
    /// [`GroupError`] for a stale key, an offset that is not finite, whole and inside the point domain, or an offset that would move
    /// a hit rectangle of the group outside it.
    pub fn set_group_offset(&mut self, key: GroupKey, x: f64, y: f64) -> Result<(), GroupError> {
        self.edits.set_group_offset(key, x, y)?;
        self.dirty.set(true);
        Ok(())
    }

    /// Sets or removes a group's clip, a rectangle in absolute window points (the offset does not move it).
    ///
    /// # Errors
    ///
    /// [`GroupError`] for a stale key or a rectangle that is not finite, positive in size and inside the point domain.
    pub fn set_group_clip(
        &mut self,
        key: GroupKey,
        clip: Option<PointRect>,
    ) -> Result<(), GroupError> {
        self.edits.set_group_clip(key, clip)?;
        self.dirty.set(true);
        Ok(())
    }

    /// Destroys a group, all its quads (their texture references are dropped on this writer thread) and all its hit keys.
    ///
    /// # Errors
    ///
    /// [`GroupError`] for a stale or foreign key.
    pub fn destroy_group(&mut self, key: GroupKey) -> Result<(), GroupError> {
        let removed = self.edits.destroy_group(key)?;
        self.dirty.set(true);
        drop(removed);
        Ok(())
    }

    /// The number of live groups, the base group not counted.
    #[must_use]
    pub fn group_count(&self) -> u32 {
        self.edits.group_count()
    }

    /// Moves a live quad without changing its painter order or texture.
    ///
    /// # Errors
    ///
    /// Returns `QuadError` for an invalid key or non-finite coordinates.
    pub fn set_position(&mut self, key: QuadKey, x: f64, y: f64) -> Result<(), QuadError> {
        self.edits.set_position(key, x, y)?;
        self.dirty.set(true);
        Ok(())
    }

    /// Sets a live quad's premultiplied RGBA8 tint without changing its order, texture or geometry.
    ///
    /// # Errors
    ///
    /// Returns `QuadError` for a foreign or stale key.
    pub fn set_tint(&mut self, key: QuadKey, tint: crate::Tint) -> Result<(), QuadError> {
        self.edits.set_tint(key, tint)?;
        self.dirty.set(true);
        Ok(())
    }

    /// Changes a live quad's rectangle atomically without changing its order or texture.
    ///
    /// # Errors
    ///
    /// Returns `QuadError` for an invalid key or invalid geometry.
    pub fn set_rect(
        &mut self,
        key: QuadKey,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<(), QuadError> {
        self.edits.set_rect(key, x, y, width, height)?;
        self.dirty.set(true);
        Ok(())
    }

    /// Destroys a live quad and drops its editable texture reference on this writer thread.
    ///
    /// # Errors
    ///
    /// Returns `QuadError` if the key is foreign or stale.
    pub fn destroy_quad(&mut self, key: QuadKey) -> Result<(), QuadError> {
        let removed = self.edits.destroy(key)?;
        self.dirty.set(true);
        drop(removed);
        Ok(())
    }

    /// The number of live editable quads.
    #[must_use]
    pub fn quad_count(&self) -> u32 {
        self.edits.len()
    }

    /// Publishes one immutable frame when the scene changed; returns false when unchanged or ended.
    pub fn publish(&mut self) -> bool {
        if !self.dirty.get() {
            return false;
        }
        {
            let mut state = self.bridge.lock();
            if state.phase == Phase::Ended {
                state.counts.publication_discarded += 1;
                return false;
            }
        }

        self.edits.build(&mut self.spare);
        let hit_regions = Arc::new(self.edits.publish_hits());
        let next_number = self.frame_number.saturating_add(1);
        let frame = Arc::new(SceneFrame::new(
            next_number,
            self.compat.borrow().clone(),
            std::mem::take(&mut self.spare),
            #[cfg(test)]
            Arc::clone(&self.frame_counter),
            #[cfg(test)]
            self.frame_drop_hook.clone(),
        ));
        #[cfg(test)]
        if let Some(probe) = &self.build_probe {
            (probe.0)();
        }
        let mut displaced = Displaced::default();
        let accepted = {
            let mut state = self.bridge.lock();
            if state.phase == Phase::Ended {
                state.counts.publication_discarded += 1;
                displaced.frame = Some(frame);
                false
            } else {
                displaced.frame = state.frame.replace(Arc::clone(&frame));
                displaced.hit_regions = state.hit_regions.replace(hit_regions);
                state.generation += 1;
                state.counts.published += 1;
                displaced.snapshot = state.publish();
                true
            }
        };

        // Drop replaced snapshots and frames after releasing the bridge lock. If this was the last frame holder,
        // reclaim its vector for the next build; clearing it drops texture pins on the writer thread.
        drop(displaced.snapshot.take());
        drop(displaced.hit_regions.take());
        if let Some(old) = displaced.frame.take() {
            match Arc::try_unwrap(old) {
                Ok(old) => {
                    let (compat, mut quads) = old.into_parts();
                    drop(compat);
                    quads.clear();
                    self.spare = quads;
                }
                Err(shared) => drop(shared),
            }
        }
        if accepted {
            self.frame_number = next_number;
            self.dirty.set(false);
        }
        accepted
    }
}

#[cfg(test)]
#[derive(Clone)]
struct BuildProbe(Arc<dyn Fn() + Send + Sync>);

#[cfg(test)]
impl std::fmt::Debug for BuildProbe {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BuildProbe(..)")
    }
}

#[cfg(test)]
impl SceneWriter {
    pub(crate) fn set_build_probe(&mut self, probe: impl Fn() + Send + Sync + 'static) {
        self.build_probe = Some(BuildProbe(Arc::new(probe)));
    }

    /// Runs `hook` whenever a frame this writer builds is dropped (tests only).
    pub(crate) fn set_frame_drop_hook(&mut self, hook: impl Fn() + Send + Sync + 'static) {
        self.frame_drop_hook = Some(crate::scene::FrameDropHook(Arc::new(hook)));
    }

    pub(crate) fn spare_capacity(&self) -> usize {
        self.spare.capacity()
    }

    pub(crate) fn force_generation(&mut self, key: QuadKey, generation: u64) -> QuadKey {
        self.edits.force_generation(key.slot(), generation)
    }
}
