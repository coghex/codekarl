//! The scene bridge: a compatibility quad plus persistent scene frames, written from the UI VM's thread.
//!
//! One small lock covers the compatibility quad, latest immutable scene frame, [`Metrics`], [`Filter`], a
//! generation, the attached [`QuadSink`] and the phase. Scene edits stay in the one writer; publication builds
//! their frame outside this lock and exchanges its `Arc` inside it. Every bridge change goes through the lock:
//!
//! - a scene write ([`SceneWriter::set_quad`], [`SceneWriter::clear_quad`]) from the UI VM's thread;
//! - [`SceneBridge::attach`] from the renderer's `start`, which publishes the current scene under the same
//!   lock, so a write made between `start` and attach cannot be stranded;
//! - [`SceneBridge::set_pixel_size`], a metrics update;
//! - [`SceneBridge::detach`], which removes the sink and ends the bridge.
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
    metrics: Option<Metrics>,
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
}

impl State {
    /// Publishes the current snapshot, when a sink is attached, and returns the one the sink replaced. Called
    /// with the lock held.
    fn publish(&self) -> Option<Snapshot> {
        let (Some(sink), Some(metrics)) = (&self.sink, &self.metrics) else {
            return None;
        };
        let quad = self.scene.as_ref().and_then(|quad| quad.place(metrics));
        sink.publish(Snapshot {
            generation: self.generation,
            filter: self.filter,
            requested: self.scene.is_some(),
            quad,
            scene: self.frame.as_ref().map(Arc::clone),
            metrics: *metrics,
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
                metrics: None,
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
            edits: EditScene::new(self.registry, self.limits.max_quads),
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
            state.metrics = Some(Metrics::new(desc, pixels));
            state.sink = Some(sink);
            state.phase = Phase::Attached;
            state.generation += 1;
            displaced.snapshot = state.publish();
        }
        drop(displaced);
        Ok(())
    }

    /// Updates the drawable's size in pixels and publishes the scene converted with it. Nothing happens
    /// unless a sink is attached.
    pub fn set_pixel_size(&self, pixels: PixelSize) {
        let mut displaced = Displaced::default();
        {
            let mut state = self.lock();
            if state.phase != Phase::Attached {
                return;
            }
            if let Some(metrics) = &mut state.metrics {
                metrics.pixels = pixels;
            }
            state.generation += 1;
            displaced.snapshot = state.publish();
        }
        drop(displaced);
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
            // Replace the consumer's latest snapshot before detaching it, so an idle sink cannot pin the scene frame.
            if state.phase == Phase::Attached && displaced.frame.is_some() {
                displaced.snapshot = state.publish();
            }
            state.phase = Phase::Ended;
            // The sink may hold images: it goes after the lock does.
            displaced.sink = state.sink.take();
        }
        drop(displaced);
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
        let key = self.edits.create(texture, x, y, width, height)?;
        self.dirty.set(true);
        Ok(key)
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
                state.generation += 1;
                state.counts.published += 1;
                displaced.snapshot = state.publish();
                true
            }
        };

        // Drop replaced snapshots and frames after releasing the bridge lock. If this was the last frame holder,
        // reclaim its vector for the next build; clearing it drops texture pins on the writer thread.
        drop(displaced.snapshot.take());
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
