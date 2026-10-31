//! Shared helpers: images, windows, sinks and gates.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use ck_window::{PixelSize, WindowDesc};

use crate::{Image, Quad, QuadSink, Snapshot};

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(super) fn image(width: u32, height: u32) -> Arc<Image> {
    Arc::new(Image::from_parts(
        width,
        height,
        vec![255; width as usize * height as usize * 4],
    ))
}

pub(super) fn quad_at(image: &Arc<Image>, x: f64, y: f64, width: f64, height: f64) -> Quad {
    Quad::new(Arc::clone(image), x, y, Some(width), Some(height)).unwrap()
}

pub(super) fn desc(width: u32, height: u32) -> WindowDesc {
    WindowDesc {
        title: "test".to_owned(),
        width,
        height,
        clear: [0.0; 3],
    }
}

pub(super) fn pixels(width: u32, height: u32) -> PixelSize {
    PixelSize { width, height }
}

/// Counts live images: an image made by [`LiveImages::make`] is live until it is dropped.
#[derive(Clone, Default)]
pub(super) struct LiveImages {
    live: Arc<AtomicUsize>,
    /// What each drop saw (see `LiveImages::on_drop`).
    dropped: Arc<Mutex<Vec<bool>>>,
}

impl LiveImages {
    pub(super) fn live(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    /// An image whose drop decrements the live count and calls `probe`, recording its answer.
    pub(super) fn make(
        &self,
        width: u32,
        height: u32,
        probe: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Arc<Image> {
        self.live.fetch_add(1, Ordering::SeqCst);
        let (live, dropped) = (Arc::clone(&self.live), Arc::clone(&self.dropped));
        Arc::new(Image::observed(
            width,
            height,
            Arc::new(move || {
                live.fetch_sub(1, Ordering::SeqCst);
                lock(&dropped).push(probe());
            }),
        ))
    }

    /// The answers of the probes, one per image dropped so far.
    pub(super) fn probes(&self) -> Vec<bool> {
        lock(&self.dropped).clone()
    }
}

/// A one-shot gate: `wait` returns once `open` was called.
#[derive(Default)]
pub(super) struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    pub(super) fn open(&self) {
        *lock(&self.open) = true;
        self.changed.notify_all();
    }

    pub(super) fn wait(&self) {
        let mut open = lock(&self.open);
        while !*open {
            open = self
                .changed
                .wait(open)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

/// A sink that records every snapshot, and calls a hook with each one before recording it. The hook runs with
/// the bridge lock held, as a real sink does, so a test can hold the lock at an exact point by blocking in it.
pub(super) struct RecordingSink {
    published: Mutex<Vec<Snapshot>>,
    hook: Box<dyn Fn(&Snapshot) + Send + Sync>,
}

impl RecordingSink {
    pub(super) fn new() -> Arc<Self> {
        Self::with_hook(|_| {})
    }

    pub(super) fn with_hook(hook: impl Fn(&Snapshot) + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            published: Mutex::new(Vec::new()),
            hook: Box::new(hook),
        })
    }

    pub(super) fn count(&self) -> usize {
        lock(&self.published).len()
    }

    pub(super) fn generations(&self) -> Vec<u64> {
        lock(&self.published)
            .iter()
            .map(|snapshot| snapshot.generation)
            .collect()
    }

    pub(super) fn snapshots(&self) -> Vec<Snapshot> {
        lock(&self.published).clone()
    }

    pub(super) fn last(&self) -> Option<Snapshot> {
        lock(&self.published).last().cloned()
    }
}

impl QuadSink for RecordingSink {
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        (self.hook)(&snapshot);
        lock(&self.published).push(snapshot);
        None
    }
}

/// A latest-value sink as a backend has one: it swaps the value in and hands the one it replaced back, so
/// nothing is freed inside `publish`, and keeps a history of what it was given.
#[derive(Default)]
pub(super) struct LatestSink {
    latest: Mutex<Option<Snapshot>>,
    /// Every snapshot ever published, as (generation, whether it holds a quad).
    history: Mutex<Vec<(u64, bool)>>,
    /// While set, `publish` panics before it takes the snapshot.
    fault: Option<Arc<AtomicBool>>,
}

impl LatestSink {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A sink that panics in `publish` whenever `fault` is set.
    pub(super) fn failing(fault: &Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            fault: Some(Arc::clone(fault)),
            ..Self::default()
        })
    }

    pub(super) fn latest(&self) -> Option<Snapshot> {
        lock(&self.latest).clone()
    }

    /// The generation of each snapshot published to this sink, in order, and whether it held a quad.
    pub(super) fn history(&self) -> Vec<(u64, bool)> {
        lock(&self.history).clone()
    }

    /// Takes the latest value, leaving none (a render thread drawing a frame).
    pub(super) fn take(&self) -> Option<Snapshot> {
        lock(&self.latest).take()
    }
}

impl QuadSink for LatestSink {
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        if let Some(fault) = &self.fault {
            assert!(!fault.load(Ordering::SeqCst), "a sink that panics");
        }
        lock(&self.history).push((snapshot.generation, snapshot.quad.is_some()));
        lock(&self.latest).replace(snapshot)
    }
}

/// A sink that breaks the contract: it drops the snapshot it replaces inside `publish`, so it keeps no
/// reference of its own past the next write. With it, the bridge's own references are the last ones standing,
/// which isolates what the bridge itself drops, and when.
#[derive(Default)]
pub(super) struct EagerSink {
    latest: Mutex<Option<Snapshot>>,
}

impl EagerSink {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl QuadSink for EagerSink {
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        *lock(&self.latest) = Some(snapshot);
        None
    }
}

/// How long a test waits for something that must happen. Reaching it is a failure (a hang), never part of an
/// ordering.
pub(super) const PATIENCE: Duration = Duration::from_secs(30);

/// Waits until `condition` holds, failing after [`PATIENCE`].
pub(super) fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < PATIENCE, "gave up waiting for {what}");
        thread::yield_now();
    }
}

/// Answers whether this bridge's lock is held right now. It holds the bridge weakly, so an image that the
/// bridge owns can carry it without a cycle.
pub(super) fn lock_held_by(
    bridge: &Arc<crate::SceneBridge>,
) -> impl Fn() -> bool + Clone + Send + Sync + 'static {
    let weak = Arc::downgrade(bridge);
    move || weak.upgrade().is_some_and(|bridge| bridge.lock_is_held())
}
