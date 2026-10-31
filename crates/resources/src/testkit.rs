//! Helpers shared by the unit tests of the file sources and the loader pool: scratch directories, PNG files, latches
//! that hold a thread at a point of the protocols, and a fake backend (tests only).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::assets::Assets;
use crate::limits::ResourceLimits;
use crate::lockdebug;
use crate::pixels::PixelRevision;
use crate::registry::TextureKey;
use crate::service::{
    CommitOutcome, Mode, ResourceService, ServiceWakes, Stage, StatusState, TextureStatus,
};

/// How long a test waits for something observable before it fails.
pub(crate) const BOUND: Duration = Duration::from_secs(20);

/// Waits until `condition` holds, failing the test after [`BOUND`].
pub(crate) fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < BOUND, "gave up waiting for {what}");
        thread::yield_now();
    }
}

/// Waits until `condition` holds like [`wait_until`], and when it gives up says what `describe()` returns at that moment: for a
/// wait whose condition has several halves, which half was false (the state a failure leaves is lost once the test panics).
pub(crate) fn wait_until_described(
    what: &str,
    mut condition: impl FnMut() -> bool,
    describe: impl Fn() -> String,
) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < BOUND,
            "gave up waiting for {what}: {}",
            describe()
        );
        thread::yield_now();
    }
}

/// A scratch directory, removed afterwards.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ck-resources-files-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).expect("a scratch directory");
        Self(path.canonicalize().expect("a canonical scratch directory"))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A PNG of `width` x `height` pixels of one RGBA colour (cached: encoding a large image is the slow part).
pub(crate) fn png(width: u32, height: u32, colour: [u8; 4]) -> Arc<Vec<u8>> {
    type Key = (u32, u32, [u8; 4]);
    static CACHE: OnceLock<Mutex<HashMap<Key, Arc<Vec<u8>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(found) = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&(width, height, colour))
    {
        return Arc::clone(found);
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = ::png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(::png::ColorType::Rgba);
        encoder.set_depth(::png::BitDepth::Eight);
        encoder.set_compression(::png::Compression::Fast);
        let mut writer = encoder.write_header().expect("a PNG header");
        let row: Vec<u8> = colour
            .iter()
            .copied()
            .cycle()
            .take(width as usize * 4)
            .collect();
        let mut stream = writer.stream_writer().expect("a PNG stream");
        for _ in 0..height {
            std::io::Write::write_all(&mut stream, &row).expect("a row");
        }
        stream.finish().expect("the PNG is complete");
    }
    let made = Arc::new(bytes);
    cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert((width, height, colour), Arc::clone(&made));
    made
}

/// Writes a PNG called `name` below `dir`.
pub(crate) fn write_png(dir: &Path, name: &str, width: u32, height: u32, colour: [u8; 4]) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("a directory");
    }
    fs::write(path, &**png(width, height, colour)).expect("a file");
}

/// Limits for file tests: `workers` loader threads and `cpu` bytes of CPU budget (at least 64 MiB).
pub(crate) fn limits(workers: u32, cpu: u64) -> ResourceLimits {
    ResourceLimits {
        max_textures: 512,
        max_pending_jobs: 512,
        decode_workers: workers,
        cpu_pixel_bytes: cpu,
        ..ResourceLimits::DEFAULT
    }
}

/// 64 MiB: the smallest CPU budget a service accepts.
pub(crate) const MIB64: u64 = 64 * 1024 * 1024;

/// A service reading files below `root`.
pub(crate) fn file_service(root: &Path, mode: Mode, limits: ResourceLimits) -> ResourceService {
    ResourceService::with_assets(
        limits,
        mode,
        ServiceWakes::default(),
        Some(Assets::new(root)),
    )
    .expect("a service")
}

/// Takes the next job, runs it on this thread and completes it.
pub(crate) fn step(service: &ResourceService) -> Option<(TextureKey, u64)> {
    let job = service.take_job()?;
    let named = (job.key(), job.revision());
    service.complete(job.run());
    Some(named)
}

pub(crate) fn status(service: &ResourceService, key: TextureKey) -> TextureStatus {
    service.status(key).expect("a status")
}

/// Waits until the resource's status satisfies `condition`.
pub(crate) fn wait_for_status(
    service: &ResourceService,
    key: TextureKey,
    what: &str,
    mut condition: impl FnMut(&TextureStatus) -> bool,
) -> TextureStatus {
    let mut last = None;
    wait_until(what, || {
        let now = status(service, key);
        let done = condition(&now);
        last = Some(now);
        done
    });
    last.expect("a status was read")
}

pub(crate) fn state(service: &ResourceService, key: TextureKey) -> StatusState {
    status(service, key).state
}

pub(crate) fn reserved(service: &ResourceService) -> [u64; 3] {
    let stats = service.stats();
    [
        stats.jobs.reserved,
        stats.cpu_pixels.reserved,
        stats.metadata.reserved,
    ]
}

/// A one-shot latch: threads wait on it (bounded) until it is opened.
#[derive(Clone, Default)]
pub(crate) struct Latch(Arc<(Mutex<bool>, Condvar)>);

impl Latch {
    pub(crate) fn open(&self) {
        *self.0.0.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.0.1.notify_all();
    }

    /// Waits until the latch is open, failing the test after [`BOUND`].
    pub(crate) fn wait(&self) {
        let started = Instant::now();
        let mut open = self.0.0.lock().unwrap_or_else(PoisonError::into_inner);
        while !*open {
            let left = BOUND.saturating_sub(started.elapsed());
            assert!(!left.is_zero(), "a latch was never opened");
            open = self
                .0
                .1
                .wait_timeout(open, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for Latch {
    fn drop(&mut self) {
        // A test that failed halfway must not leave a thread held.
        if Arc::strong_count(&self.0) == 1 {
            self.open();
        }
    }
}

type Held = (Stage, Option<TextureKey>, Latch);
type Log = Arc<Mutex<Vec<(Stage, Option<TextureKey>)>>>;
type Action = Arc<dyn Fn(Option<TextureKey>) + Send + Sync>;
type Actions = Arc<Mutex<Vec<(Stage, Option<TextureKey>, Action)>>>;

/// A script for the service's test hook: it logs every point reached and holds a thread at the points a test chose.
#[derive(Clone, Default)]
pub(crate) struct Script {
    log: Log,
    holds: Arc<Mutex<Vec<Held>>>,
    /// Runs at a point, before any hold (to inject a return of bytes, a panic, ...).
    actions: Actions,
}

impl Script {
    pub(crate) fn install(&self, service: &ResourceService) {
        let (log, holds, actions) = (
            Arc::clone(&self.log),
            Arc::clone(&self.holds),
            Arc::clone(&self.actions),
        );
        service.set_hook(Some(Arc::new(move |stage, key| {
            log.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((stage, key));
            let matching = |k: &Option<TextureKey>| k.is_none() || *k == key;
            let action: Vec<_> = actions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .filter(|(s, k, _)| *s == stage && matching(k))
                .map(|(_, _, action)| Arc::clone(action))
                .collect();
            for action in action {
                action(key);
            }
            let latch = holds
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .find(|(s, k, _)| *s == stage && matching(k))
                .map(|(_, _, latch)| latch.clone());
            if let Some(latch) = latch {
                latch.wait();
            }
        })));
    }

    /// Holds every thread that reaches `stage` for `key` (any key if `None`) until the returned latch is opened.
    pub(crate) fn hold(&self, stage: Stage, key: Option<TextureKey>) -> Latch {
        let latch = Latch::default();
        // The newest hold for a point is the one that applies.
        self.holds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(0, (stage, key, latch.clone()));
        latch
    }

    /// Opens every latch (a test that ends, or fails, must not leave a thread held).
    pub(crate) fn open_all(&self) {
        for (_, _, latch) in self
            .holds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            latch.open();
        }
    }

    /// Runs `action` every time `stage` is reached for `key` (any key if `None`), before any hold.
    pub(crate) fn on(
        &self,
        stage: Stage,
        key: Option<TextureKey>,
        action: impl Fn(Option<TextureKey>) + Send + Sync + 'static,
    ) {
        self.actions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((stage, key, Arc::new(action)));
    }

    /// How many times `stage` was reached for `key` (any key if `None`).
    pub(crate) fn count(&self, stage: Stage, key: Option<TextureKey>) -> usize {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(s, k)| *s == stage && (key.is_none() || *k == key))
            .count()
    }

    /// Waits until `stage` was reached `times` times for `key`.
    pub(crate) fn wait_for(&self, stage: Stage, key: Option<TextureKey>, times: usize) {
        wait_until(&format!("{stage:?} x{times}"), || {
            self.count(stage, key) >= times
        });
    }
}

/// The fake backend: takes what is waiting, commits and adopts it, and keeps the `Arc`s (the CPU bytes stay in transit)
/// until it is told to release them.
#[derive(Default)]
pub(crate) struct Backend {
    kept: Vec<Arc<PixelRevision>>,
}

impl Backend {
    /// Takes every waiting upload, commits and adopts it. Returns how many.
    pub(crate) fn take_all(&mut self, service: &ResourceService) -> usize {
        let mut uploads = Vec::new();
        service.take_uploads(&mut uploads, usize::MAX);
        let count = uploads.len();
        for upload in uploads {
            if service.try_commit(upload.key, upload.revision) == CommitOutcome::Committed {
                service.adopted(upload.key, upload.revision);
            }
            self.kept.push(upload.pixels);
        }
        count
    }

    /// Drops the revision of `key` it holds (returning its CPU bytes), if any.
    pub(crate) fn release(&mut self, key: TextureKey) -> usize {
        let before = self.kept.len();
        self.kept.retain(|revision| revision.key() != key);
        before - self.kept.len()
    }

    /// Drops everything it holds.
    pub(crate) fn release_all(&mut self) {
        self.kept.clear();
    }

    /// The `index`th revision it took (in the order taken).
    pub(crate) fn revision(&self, index: usize) -> &Arc<PixelRevision> {
        &self.kept[index]
    }
}

/// Finishes a test whose service was driven by hand or by a pool that has ended: closes, runs what is left, retires
/// everything through the fake backend, and asserts the budgets are back at zero and that no value was dropped with a
/// lock held.
pub(crate) fn teardown(service: &ResourceService) {
    service.close();
    for _ in 0..8 {
        while step(service).is_some() {}
        let mut keys = Vec::new();
        service.take_retirements(&mut keys, usize::MAX);
        for key in keys {
            service.retired(key);
        }
        let _ = service.maintain(usize::MAX);
    }
    assert_eq!(reserved(service), [0, 0, 0], "{:?}", service.stats());
    assert_eq!(
        lockdebug::violations(),
        0,
        "a value was dropped with a lock held"
    );
}
