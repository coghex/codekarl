//! The scene presenter (CK-114): one device context shared with the texture store, the revision ledger run on the
//! render thread by a texture driver, every frame a ledger ticket, and one proven teardown. These are cases of the
//! presenter test target (see `tests/presenter.rs`: a worker per case, under Metal validation and the Main Thread
//! Checker, twice, once with the owner typing). Faults come only through `start_with_test_hooks` and the store's own
//! test hooks; the test driver below stands in for the resource service of milestone 5-5c.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Instant;

#[path = "instanced.rs"]
pub(crate) mod instanced;

#[path = "compat.rs"]
pub(crate) mod compat;

use ck_gpu_metal::{
    BindlessLimits, Boundary, LedgerEvent, LedgerLimits, LedgerStats, MetalContext, MetalFrameSlot,
    PassOutcome, PixelSource, SceneLimits, SceneSetup, TextureDriver, TextureLedger, TextureLimits,
    TextureTestHooks, TicketEnding,
};

use super::{
    BOUND, CleanupResult, Duration, Gate, MetalPresenter, NeverComplete, PresenterReport, Probes,
    Rig, TestHooks, await_presented, end, reports_until,
};

// ---------------------------------------------------------------------------------------------
// The test driver
// ---------------------------------------------------------------------------------------------

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An RGBA8 image of one grey, `width` x `height`.
struct Grey(Vec<u8>);

impl PixelSource for Grey {
    fn pixels(&self) -> &[u8] {
        &self.0
    }
}

fn grey(width: u32, height: u32) -> Arc<dyn PixelSource> {
    let bytes = usize::try_from(width * height * 4).expect("a small image");
    Arc::new(Grey(vec![0x80; bytes]))
}

/// An RGBA8 image of one colour, `width` x `height`.
fn solid(width: u32, height: u32, pixel: [u8; 4]) -> Arc<dyn PixelSource> {
    let bytes = usize::try_from(width * height).expect("a small image");
    Arc::new(Grey(pixel.repeat(bytes)))
}

/// What the test asks of the driver, and what the driver saw.
#[derive(Default)]
struct DriverState {
    // --- commands
    /// Images to offer when there is room: (key, revision, width, height).
    offers: VecDeque<(u32, u64, u32, u32)>,
    /// The colour of an offered image, by (key, revision); the default is the grey of the older cases.
    colors: Vec<(u32, u64, [u8; 4])>,
    /// The exact pixels of an offered image, by (key, revision): (width, height, RGBA8 bytes); they replace the offer's
    /// own size and colour.
    pictures: Vec<(u32, u64, u32, u32, Vec<u8>)>,
    /// Keys to retire at the next pass.
    retire: Vec<u32>,
    /// Commit a prepared revision at once (the service's verdict).
    commit: bool,
    /// Keys whose prepared revisions are **not** committed on their `Prepared` event: the service has not given its verdict
    /// yet (CK-119's "pending" compatibility image). They are committed later through `commit_now`.
    defer_commit: Vec<u32>,
    /// Prepared revisions to commit at the next pass, in this order (the service's late verdicts).
    commit_now: Vec<(u32, u64)>,
    /// How many committed revisions one frame boundary adopts at most.
    adopt_limit: usize,
    /// The most `KeyRetired` events relayed in one pass.
    relay_limit: usize,
    // --- observations
    /// Every pass: its boundary and the number of events it was handed.
    passes: Vec<(Boundary, usize)>,
    /// What each frame boundary adopted.
    adopted: Vec<Vec<(u32, u64)>>,
    prepared: Vec<(u32, u64)>,
    upload_failed: Vec<(u32, u64, String)>,
    retired: Vec<u32>,
    /// `GpuFailure` events that reached the driver (the presenter must take them itself: none).
    gpu_failures: usize,
    /// Compat-upload-failure observations that reached the driver: the presenter takes them first, so always 0.
    compat_failures_seen: usize,
    /// The ledger's counters at the last pass.
    stats: Option<LedgerStats>,
    /// The most live pins any pass saw.
    max_live_pins: usize,
    /// The most pending submitted tickets and building tickets the driver ever saw at a pass.
    max_submitted_pending: usize,
    /// The ledger's counters at the `Ending` pass, and whether an offer was still accepted there.
    ending_stats: Option<LedgerStats>,
    ending_offer: Option<bool>,
    /// A pass found room zero while offers waited.
    blocked_on_room: usize,
    /// Keys the driver retires inside the `Ending` pass.
    retire_at_ending: Vec<u32>,
    /// The `Ending` pass makes no offer.
    skip_ending_offer: bool,
    /// The size (width, height) of the image the `Ending` pass offers; one pixel when unset.
    ending_offer_size: Option<(u32, u32)>,
    /// The driver panics when it is handed an `UploadFailed` event.
    panic_on_upload_failed: bool,
    /// After a pass that made an offer, the driver (tests only) waits until the store has a **closed** envelope
    /// waiting to be submitted, says so, and holds the render thread there until released.
    hold_after_offer: Option<Arc<Hold>>,
    /// After a pass that finds an upload batch submitted, the driver waits until its **completion has arrived**
    /// (and the batch is still unpolled), says so, and holds the render thread there until released.
    hold_after_submit: Option<Arc<Hold>>,
}

/// A latch the test driver waits on after an offer: `entered` says it is waiting, `release` lets it go.
#[derive(Default)]
struct Hold {
    /// The driver is in its pass and waiting for the phase it was asked to wait for.
    entered: AtomicBool,
    /// That phase is reached (a closed envelope waits to be submitted / a submitted batch has completed unpolled):
    /// the driver, and with it the render thread, is held right there until released.
    ready: AtomicBool,
    release: (Mutex<bool>, Condvar),
}

impl Hold {
    fn open(&self) {
        *lock(&self.release.0) = true;
        self.release.1.notify_all();
    }
}

/// Tests only: holds the render thread in the driver's pass. Waits (bounded, a handshake and not a guess of a delay)
/// for the phase the case asked for - a closed envelope waiting to be submitted, or a submitted batch whose
/// completion has arrived unpolled - says so through `hold.ready`, and then holds until the case releases it.
fn hold_here(hold: &Hold, ledger: &TextureLedger<u32>, after_submit: bool) {
    hold.entered.store(true, Ordering::SeqCst);
    let limit = Instant::now() + BOUND;
    while Instant::now() < limit {
        let reached = if after_submit {
            ledger.completion_arrived_unpolled()
        } else {
            ledger.envelope_ready_unsubmitted()
        };
        if reached {
            hold.ready.store(true, Ordering::SeqCst);
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let mut released = lock(&hold.release.0);
    while !*released && Instant::now() < limit {
        released = hold
            .release
            .1
            .wait_timeout(released, Duration::from_millis(20))
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

/// Offers the images the test asked for while the ledger has room (a driver pass's offering step); whether any was offered.
fn offer_waiting(state: &mut DriverState, ledger: &mut TextureLedger<u32>) -> bool {
    let mut offered = false;
    while let Some(&(key, revision, offered_width, offered_height)) = state.offers.front() {
        let picture = state
            .pictures
            .iter()
            .find(|(k, r, ..)| (*k, *r) == (key, revision))
            .map(|(_, _, w, h, bytes)| (*w, *h, bytes.clone()));
        let (width, height) = picture
            .as_ref()
            .map_or((offered_width, offered_height), |(w, h, _)| (*w, *h));
        if ledger.room().min(ledger.extra_room()) == 0 {
            state.blocked_on_room += 1;
            break;
        }
        let image: Arc<dyn PixelSource> = match picture {
            Some((_, _, bytes)) => Arc::new(Grey(bytes)),
            None => state
                .colors
                .iter()
                .find(|(k, r, _)| (*k, *r) == (key, revision))
                .map_or_else(|| grey(width, height), |(_, _, c)| solid(width, height, *c)),
        };
        ledger
            .offer(key, revision, width, height, image)
            .expect("an offer within the reserved room");
        state.offers.pop_front();
        offered = true;
    }
    offered
}

struct TestDriver {
    state: Arc<Mutex<DriverState>>,
    backlog: VecDeque<u32>,
}

impl TestDriver {
    fn new(state: &Arc<Mutex<DriverState>>) -> Self {
        {
            let mut inner = lock(state);
            if inner.adopt_limit == 0 {
                inner.adopt_limit = usize::MAX;
            }
            if inner.relay_limit == 0 {
                inner.relay_limit = 256;
            }
            inner.commit = true;
        }
        Self {
            state: Arc::clone(state),
            backlog: VecDeque::new(),
        }
    }
}

impl TextureDriver<u32> for TestDriver {
    fn pass(
        &mut self,
        ledger: &mut TextureLedger<u32>,
        boundary: Boundary,
        events: Vec<LedgerEvent<u32>>,
    ) -> PassOutcome {
        let mut state = lock(&self.state);
        state.passes.push((boundary, events.len()));
        let mut more = false;
        for event in events {
            match event {
                LedgerEvent::Prepared { key, revision } => {
                    state.prepared.push((key, revision));
                    if state.commit && !state.defer_commit.contains(&key) {
                        ledger
                            .commit(key, revision)
                            .expect("a prepared revision commits");
                    }
                }
                LedgerEvent::UploadFailed {
                    key,
                    revision,
                    reason,
                } => {
                    state.upload_failed.push((key, revision, reason));
                    assert!(
                        !state.panic_on_upload_failed,
                        "the driver panics on an upload failure"
                    );
                }
                LedgerEvent::KeyRetired { key } => self.backlog.push_back(key),
                LedgerEvent::GpuFailure { .. } => state.gpu_failures += 1,
                // The presenter takes this observation before the driver runs: a driver never sees it.
                LedgerEvent::CompatUploadFailed { .. } => state.compat_failures_seen += 1,
            }
        }
        // Relay at most `relay_limit` retirements per pass; what is left is runnable work.
        for _ in 0..state.relay_limit {
            match self.backlog.pop_front() {
                Some(key) => state.retired.push(key),
                None => break,
            }
        }
        more |= !self.backlog.is_empty();
        // Retire the keys the test asked for. Their events arrive with the next pump, which is runnable work.
        let retire: Vec<u32> = state.retire.drain(..).collect();
        if !retire.is_empty() {
            more = true;
        }
        for key in retire {
            ledger.retire_key(key);
        }
        // The service's late verdicts, in the order the case gave them.
        for (key, revision) in std::mem::take(&mut state.commit_now) {
            ledger
                .commit(key, revision)
                .expect("a prepared revision commits");
        }
        // Offer images while there is room; work blocked on room is not runnable.
        let mut offered = offer_waiting(&mut state, ledger);
        if boundary == Boundary::Frame {
            let mut adopted = Vec::new();
            ledger.adopt(state.adopt_limit, &mut adopted);
            state.adopted.push(adopted);
        }
        let counters = ledger.stats();
        state.max_live_pins = state.max_live_pins.max(counters.live_pins);
        state.max_submitted_pending = state
            .max_submitted_pending
            .max(counters.tickets_submitted_pending);
        if boundary == Boundary::Ending {
            for key in std::mem::take(&mut state.retire_at_ending) {
                ledger.retire_key(key);
            }
            // The store is still running here: an offer is still accepted.
            let accepted = !state.skip_ending_offer && ledger.room() > 0 && {
                let (width, height) = state.ending_offer_size.unwrap_or((1, 1));
                ledger
                    .offer(u32::MAX, 1, width, height, grey(width, height))
                    .is_ok()
            };
            state.ending_offer = Some(accepted);
            // An offer made here is an offer like the others: a case's `hold_after_offer` holds this pass until the store has
            // closed its envelope (the charge was made), before the stop proves and cancels the waiting work.
            offered |= accepted;
            state.ending_stats = Some(counters.clone());
        }
        state.stats = Some(counters);
        let submitted = state
            .stats
            .as_ref()
            .is_some_and(|counters| counters.upload.batches_submitted > 0);
        let hold = if offered {
            state.hold_after_offer.take().map(|hold| (hold, false))
        } else if submitted {
            state.hold_after_submit.take().map(|hold| (hold, true))
        } else {
            None
        };
        drop(state);
        if let Some((hold, after_submit)) = hold {
            hold_here(&hold, ledger, after_submit);
        }
        PassOutcome { more }
    }
}

// ---------------------------------------------------------------------------------------------
// A scene presenter and what a case watches of it
// ---------------------------------------------------------------------------------------------

struct Scene {
    context: Arc<MetalContext>,
    state: Arc<Mutex<DriverState>>,
    tickets: Arc<Mutex<Vec<TicketEnding>>>,
    releases: Arc<Mutex<Vec<&'static str>>>,
    probes: Probes,
    /// The slot the renderer publishes frames to (CK-116).
    frames: Arc<MetalFrameSlot<u32>>,
    /// The bounds of the instance buffer.
    bounds: SceneLimits,
    /// The fault switches of the checker's own upload.
    checker_hooks: Option<TextureTestHooks>,
    /// Optional bindless limits for a scene's queue-attached residency set.
    bindless_limits: Option<BindlessLimits>,
}

/// Whether `Scene::new` gives the scene the bindless path (set by the wrapper cases that re-run an existing case through the
/// table; a case runs in a worker process of its own, so the flag never leaks into another).
pub(crate) static BINDLESS_DEFAULT: AtomicBool = AtomicBool::new(false);

impl Scene {
    fn new() -> Self {
        Self {
            context: MetalContext::new().expect("a Metal device"),
            state: Arc::new(Mutex::new(DriverState::default())),
            tickets: Arc::new(Mutex::new(Vec::new())),
            releases: Arc::new(Mutex::new(Vec::new())),
            probes: Probes::new(),
            frames: MetalFrameSlot::<u32>::new(),
            bounds: scene_limits(),
            checker_hooks: None,
            // A wrapper case of the bindless variants (CK-128) sets the flag in its own worker process: the scene then draws
            // through the table, whatever the original case builds with `Scene::new()`.
            bindless_limits: BINDLESS_DEFAULT
                .load(Ordering::SeqCst)
                .then_some(BindlessLimits {
                    texture_slots: 64,
                    max_textures: 16,
                    max_extra_revisions: 16,
                }),
        }
    }

    fn start(
        &self,
        rig: &Rig,
        ledger: LedgerLimits,
        store: Option<TextureTestHooks>,
        edit: impl FnOnce(&mut TestHooks),
    ) -> MetalPresenter {
        self.start_with_limits(rig, TextureLimits::default(), ledger, store, edit)
    }

    fn start_with_limits(
        &self,
        rig: &Rig,
        textures: TextureLimits,
        ledger: LedgerLimits,
        store: Option<TextureTestHooks>,
        edit: impl FnOnce(&mut TestHooks),
    ) -> MetalPresenter {
        let mut hooks = self.probes.hooks();
        hooks.ticket_log = Some(Arc::clone(&self.tickets));
        hooks.release_log = Some(Arc::clone(&self.releases));
        hooks.scene = Some(SceneSetup::new_with_bindless_limits(
            Arc::clone(&self.context),
            textures,
            ledger,
            self.bindless_limits,
            self.bounds,
            TestDriver::new(&self.state),
            Arc::clone(&self.frames),
            store,
            self.checker_hooks.clone(),
        ));
        edit(&mut hooks);
        self.probes.start_ok(rig, hooks)
    }

    fn driver(&self) -> MutexGuard<'_, DriverState> {
        lock(&self.state)
    }

    fn passes(&self) -> usize {
        self.driver().passes.len()
    }

    fn count(&self, boundary: Boundary) -> usize {
        self.driver()
            .passes
            .iter()
            .filter(|(seen, _)| *seen == boundary)
            .count()
    }

    fn tickets(&self) -> Vec<TicketEnding> {
        lock(&self.tickets).clone()
    }
}

/// The instance buffer's bounds for the cases that do not draw a scene: room for a thousand quads in a roomy budget.
fn scene_limits() -> SceneLimits {
    SceneLimits {
        max_quads: 1024,
        binding_geometry_bytes: 128 * 1024 * 1024,
    }
}

fn limits() -> LedgerLimits {
    LedgerLimits {
        max_extra_revisions: 16,
        max_adoptions_per_frame: 256,
    }
}

fn limits_adopting(per_frame: usize) -> LedgerLimits {
    LedgerLimits {
        max_extra_revisions: 16,
        max_adoptions_per_frame: per_frame,
    }
}

/// A texture store hook set that holds the worker until the returned gate is opened.
fn blocked_worker() -> (TextureTestHooks, Arc<(Mutex<bool>, Condvar)>) {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    (
        TextureTestHooks {
            block_worker: Some(Arc::clone(&gate)),
            ..TextureTestHooks::default()
        },
        gate,
    )
}

fn open(gate: &Arc<(Mutex<bool>, Condvar)>) {
    *lock(&gate.0) = true;
    gate.1.notify_all();
}

/// Waits (pumping the window) until `done`.
fn until(rig: &mut Rig, what: &str, done: impl FnMut() -> bool) {
    assert!(rig.pump_until(BOUND, done), "gave up waiting for {what}");
}

/// The value must stay put for `within` (pumping the window).
fn stays<T: PartialEq + std::fmt::Debug>(
    rig: &mut Rig,
    what: &str,
    within: Duration,
    mut value: impl FnMut() -> T,
) {
    let first = value();
    rig.pump_for(within);
    assert_eq!(value(), first, "{what} moved");
}

// ---------------------------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------------------------

/// A scene presenter and its store use one device (a native identity check), the presenter's frames still draw the
/// quad of a `start_2d` presenter, and on a proven end the context is released once, last.
fn scene_shares_the_context() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    let witness = MetalContext::new().expect("a second context, to read the live count");
    let mut presenter = scene.start(&rig, limits(), None, |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    assert_ne!(scene.probes.observer.device_identity(), 0);
    assert_eq!(
        scene.probes.observer.device_identity(),
        scene.context.device_identity(),
        "the presenter took its device from the shared context"
    );
    assert_ne!(scene.probes.observer.queue_identity(), 0);
    assert_eq!(
        scene.probes.observer.queue_identity(),
        scene.context.queue_identity(),
        "and its command queue: one device context, not a second queue on the same device"
    );
    assert!(
        Arc::strong_count(&scene.context) > 1,
        "the presenter and the store hold the context"
    );
    assert_eq!(scene.context.native_live_objects().contexts, 2);
    await_presented(&mut rig, &scene.probes.observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(
        Arc::strong_count(&scene.context),
        1,
        "every share of the context but the caller's was released"
    );
    assert_eq!(
        *lock(&scene.releases),
        ["store_and_ledger", "presenter", "context"],
        "the store and ledger first, the presenter's holders next, the context last"
    );
    assert!(scene.probes.observer.holders_released() > 0);
    assert_eq!(scene.probes.observer.layer_retains_outstanding(), 0);
    assert_eq!(
        scene.context.native_live_objects().contexts,
        2,
        "the caller's context and the witness are all that is left"
    );
    drop(witness);
}

/// Textures offered by the driver are uploaded, reported to it, committed by it and adopted at frame boundaries (at
/// most the limit per boundary); the store's wakes and `texture_wake()` wake an idle render thread; texture work
/// with nothing committed draws no frame.
fn scene_driver_wakes_and_adoption() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().adopt_limit = 1;
    let presenter = scene.start(&rig, limits_adopting(1), None, |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    rig.pump_for(Duration::from_millis(100));

    // texture_wake(): an idle render thread (no frame wanted) gets an ordinary pass; nothing is committed, so no
    // frame follows.
    let wake = presenter.texture_wake();
    let frames = scene.probes.frames();
    let passes = scene.count(Boundary::Pass);
    wake();
    until(&mut rig, "a pass after texture_wake", || {
        scene.count(Boundary::Pass) > passes
    });
    rig.pump_for(Duration::from_millis(200));
    assert_eq!(
        scene.probes.frames(),
        frames,
        "texture work with nothing committed draws no frame"
    );

    // Three images offered by the driver: uploaded, reported, committed, adopted one per frame boundary.
    {
        let mut state = scene.driver();
        state
            .offers
            .extend([(1, 1, 8, 8), (2, 1, 8, 8), (3, 1, 8, 8)]);
    }
    wake();
    until(&mut rig, "all three adopted", || {
        scene.driver().adopted.iter().map(Vec::len).sum::<usize>() >= 3
    });
    let state = scene.driver();
    assert_eq!(state.prepared.len(), 3, "reported to the driver once each");
    let adopted: Vec<_> = state
        .adopted
        .iter()
        .filter(|list| !list.is_empty())
        .collect();
    assert_eq!(adopted.len(), 3, "one adoption per boundary: {adopted:?}");
    assert!(adopted.iter().all(|list| list.len() == 1));
    assert_eq!(
        adopted.iter().map(|list| list[0].0).collect::<Vec<_>>(),
        [1, 2, 3],
        "in commit order"
    );
    drop(state);
    until(&mut rig, "the third wanted frame", || {
        scene.probes.frames() >= frames + 3
    });
    assert!(
        scene.probes.frames() >= frames + 3,
        "three wanted frames were drawn: {} then {}",
        frames,
        scene.probes.frames()
    );
    stays(&mut rig, "the frames", Duration::from_millis(400), || {
        scene.probes.frames()
    });
    let mut presenter = presenter;
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(scene.driver().gpu_failures, 0);
}

/// A wanted frame is not delayed beyond one pass: with the frame gated and texture work pending, the frame's own
/// boundary pass serves the work (no ordinary pass runs first), and nothing runs while the gate is closed.
fn scene_wanted_frame_comes_first() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    let gate = Arc::new(Gate::new());
    let mut presenter = scene.start(&rig, limits(), None, |hooks| {
        hooks.gate = Some(Arc::clone(&gate));
    });
    // The first frame is wanted and held at the gate.
    rig.pump_for(Duration::from_millis(200));
    assert_eq!(scene.probes.frames(), 0);
    presenter.texture_wake()();
    rig.pump_for(Duration::from_millis(200));
    assert_eq!(
        scene.passes(),
        0,
        "nothing runs while the gate holds the frame"
    );
    gate.open(1);
    until(&mut rig, "the frame", || scene.probes.frames() >= 1);
    {
        let state = scene.driver();
        assert_eq!(
            state.passes.first().map(|(boundary, _)| *boundary),
            Some(Boundary::Frame),
            "the frame's boundary pass came first: {:?}",
            state.passes
        );
    }
    rig.pump_for(Duration::from_millis(200));
    assert_eq!(
        scene.count(Boundary::Pass),
        0,
        "the boundary pass served the texture work: no ordinary pass followed"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A committed revision is adopted at a frame the rule itself makes wanted (no other redraw request), and with a
/// limit of one three committed revisions take three frames, then the thread idles.
fn scene_redraw_for_adoption() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().adopt_limit = 1;
    let mut presenter = scene.start(&rig, limits_adopting(1), None, |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    rig.pump_for(Duration::from_millis(150));
    let before = scene.probes.frames();
    scene.driver().offers.push_back((7, 1, 8, 8));
    presenter.texture_wake()();
    until(&mut rig, "the adoption", || {
        scene.driver().adopted.iter().any(|list| list == &[(7, 1)])
    });
    rig.pump_for(Duration::from_millis(300));
    assert_eq!(
        scene.probes.frames(),
        before + 1,
        "the rule made exactly one frame wanted"
    );
    stays(&mut rig, "the frames", Duration::from_millis(300), || {
        scene.probes.frames()
    });

    // Three committed, one adopted per boundary: three frames, all adopted, then idle.
    let before = scene.probes.frames();
    {
        let mut state = scene.driver();
        state
            .offers
            .extend([(8, 1, 8, 8), (9, 1, 8, 8), (10, 1, 8, 8)]);
    }
    presenter.texture_wake()();
    until(&mut rig, "three more adoptions", || {
        scene.driver().adopted.iter().map(Vec::len).sum::<usize>() >= 4
    });
    rig.pump_for(Duration::from_millis(300));
    assert_eq!(
        scene.probes.frames(),
        before + 3,
        "three committed revisions, one adoption per boundary: three frames"
    );
    stays(&mut rig, "the frames", Duration::from_millis(300), || {
        scene.probes.frames()
    });
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// Every frame result ends its ticket as specified, never dropped: committed -> submitted; prepared but not
/// committed -> `not_committed`; abandoned before preparing (or busy, skipped, a nil object) -> discarded.
#[expect(
    clippy::too_many_lines,
    reason = "the table of frame results and the checks on each read best together"
)]
fn scene_tickets_follow_frames() {
    type Injection = fn(&mut TestHooks);
    let cases: [(&str, Injection, TicketEnding, bool); 9] = [
        (
            "nil command buffer",
            |hooks| hooks.nil_command_buffer_frames = 1,
            TicketEnding::Discarded,
            true,
        ),
        (
            "nil render pass",
            |hooks| hooks.nil_render_pass_frames = 1,
            TicketEnding::Discarded,
            true,
        ),
        (
            "nil encoder",
            |hooks| hooks.nil_encoder_frames = 1,
            TicketEnding::Discarded,
            true,
        ),
        (
            "partial encoding",
            |hooks| hooks.partial_encoding_frames = 1,
            TicketEnding::NotCommitted,
            true,
        ),
        (
            "abandoned after the token was prepared",
            |hooks| hooks.abandon_after_prepare_frames = 1,
            TicketEnding::NotCommitted,
            true,
        ),
        (
            "abandoned after acquire",
            |hooks| hooks.abandon_frames = 1,
            TicketEnding::Discarded,
            false,
        ),
        (
            "skipped",
            |hooks| hooks.skipped_frames = 1,
            TicketEnding::Discarded,
            false,
        ),
        ("drawn", |_| {}, TicketEnding::Submitted, false),
        (
            "error completion",
            |hooks| hooks.error_completion_frames = 1,
            TicketEnding::Submitted,
            true,
        ),
    ];
    let mut rig = Rig::open();
    for (name, inject, first, fails) in cases {
        let scene = Scene::new();
        let mut presenter = scene.start(&rig, limits(), None, inject);
        let mut reports = Vec::new();
        if fails {
            until(&mut rig, name, || {
                while let Some(report) = presenter.take_report() {
                    reports.push(report);
                }
                reports
                    .iter()
                    .any(|report| matches!(report, PresenterReport::Failed { .. }))
            });
        } else {
            until(&mut rig, name, || scene.probes.frames() >= 1);
        }
        rig.pump_for(Duration::from_millis(150));
        assert_eq!(
            end(&mut rig, &mut presenter),
            CleanupResult::Proven,
            "{name}"
        );
        let tickets = scene.tickets();
        assert_eq!(tickets.first(), Some(&first), "{name}: {tickets:?}");
        assert!(
            tickets
                .iter()
                .all(|ending| !matches!(ending, TicketEnding::Refused(_))),
            "{name}: a ledger refusal (TicketBusy must never fire): {tickets:?}"
        );
        let state = scene.driver();
        let ending = state.ending_stats.as_ref().expect("the Ending pass");
        assert_eq!(ending.tickets_uncertain, 0, "{name}: no uncertain ticket");
        assert_eq!(ending.tickets_building, 0, "{name}");
        assert_eq!(ending.tickets_submitting, 0, "{name}");
        // The presenter and the ledger agree on the frames committed.
        let submitted = i64::try_from(
            tickets
                .iter()
                .filter(|ending| **ending == TicketEnding::Submitted)
                .count(),
        )
        .expect("a small count");
        assert_eq!(
            submitted,
            scene.probes.observer.committed(),
            "{name}: one submitted ticket per committed command buffer"
        );
    }
}

/// A busy frame (the previous command buffer has not completed) ends its ticket `discarded`, and a delayed
/// completion delays the ticket's end: its token is the frame's command buffer's.
fn scene_tickets_busy_and_delayed() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    let mut presenter = scene.start(&rig, limits(), None, |hooks| {
        hooks.completion_delay = Duration::from_millis(600);
    });
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    // Redraws while the first completion is delayed: busy steps.
    for step in 0..4_u8 {
        presenter.set_clear([f32::from(step) / 4.0, 0.2, 0.2]);
        presenter.request_redraw();
        rig.pump_for(Duration::from_millis(30));
    }
    let tickets = scene.tickets();
    assert_eq!(tickets[0], TicketEnding::Submitted);
    assert!(
        tickets.len() > 1
            && tickets[1..]
                .iter()
                .all(|ending| *ending == TicketEnding::Discarded),
        "busy steps discard their tickets: {tickets:?}"
    );
    assert_eq!(
        scene.probes.frames(),
        1,
        "the first completion is delayed: no second frame is committed"
    );
    assert_eq!(
        scene.driver().max_submitted_pending,
        1,
        "the ticket of the first frame waited for its (delayed) completion token"
    );
    until(
        &mut rig,
        "the delayed completion and the next frame",
        || scene.probes.frames() >= 2,
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert!(
        scene
            .tickets()
            .iter()
            .all(|ending| !matches!(ending, TicketEnding::Refused(_))),
        "TicketBusy never fires"
    );
}

/// An upload batch completing with an error becomes one presenter `Failed` report; the driver never sees it; nothing
/// more is drawn; the end is still `Proven` once every token arrived.
fn scene_gpu_failure() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    let store = TextureTestHooks {
        error_completion: true,
        ..TextureTestHooks::default()
    };
    let mut presenter = scene.start(&rig, limits(), Some(store), |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    scene.driver().offers.push_back((1, 1, 8, 8));
    presenter.texture_wake()();
    let mut reports = Vec::new();
    assert!(
        reports_until(&mut rig, &presenter, &mut reports, 2),
        "{reports:?}"
    );
    assert_eq!(
        reports
            .iter()
            .filter(|report| matches!(report, PresenterReport::Failed { .. }))
            .count(),
        1,
        "{reports:?}"
    );
    let frames = scene.probes.frames();
    presenter.request_redraw();
    rig.pump_for(Duration::from_millis(300));
    assert_eq!(scene.probes.frames(), frames, "nothing more is drawn");
    // Exactly one Failed, even though the failure is also seen by later looks.
    while let Some(report) = presenter.take_report() {
        reports.push(report);
    }
    assert_eq!(
        reports
            .iter()
            .filter(|report| matches!(report, PresenterReport::Failed { .. }))
            .count(),
        1,
        "{reports:?}"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    let state = scene.driver();
    assert_eq!(state.gpu_failures, 0, "the driver never sees a GpuFailure");
    assert_eq!(
        state.upload_failed.len(),
        1,
        "the failed upload itself is relayed"
    );
}

/// The combined proof (r3, F1): a presentation token delayed past the deadline while the store and ledger are
/// complete leaves **everything** unreleased; the inverse likewise; a success releases each graph once, in order.
fn scene_combined_proof() {
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let budget = Duration::from_millis(400);

    // (a) Only a drawable presentation token is lost: the store and ledger are complete, the presenter is not.
    let scene = Scene::new();
    let mut presenter = scene.start(&rig, limits(), None, |hooks| {
        hooks.never_complete = NeverComplete::Drawables;
        hooks.stop_budget = Some(budget);
    });
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    scene.driver().offers.push_back((1, 1, 8, 8));
    presenter.texture_wake()();
    until(&mut rig, "the upload adopted", || {
        scene.driver().adopted.iter().any(|list| !list.is_empty())
    });
    let live = scene.context.native_live_objects();
    assert!(live.textures > 0, "an uploaded texture exists");
    let result = end(&mut rig, &mut presenter);
    let CleanupResult::Unproven { reason } = &result else {
        panic!("{result:?}");
    };
    assert!(
        reason.contains("drawable") && !reason.contains("texture store or ledger incomplete"),
        "the presenter's tokens are what is outstanding: {reason}"
    );
    let after = scene.context.native_live_objects();
    assert!(
        after.textures >= live.textures,
        "the store's textures were not released: the presenter's tokens were incomplete ({} then {})",
        live.textures,
        after.textures
    );
    assert!(
        Arc::strong_count(&scene.context) > 1,
        "the context is retained"
    );
    assert_eq!(scene.probes.observer.holders_released(), 0);
    assert_eq!(scene.probes.observer.layer_retains_outstanding(), 1);
    assert!(lock(&scene.releases).is_empty(), "nothing was released");

    // (b) Only the upload side is missing (the worker is held): the presenter's tokens are complete.
    let scene = Scene::new();
    let (store, gate) = blocked_worker();
    let mut presenter = scene.start(&rig, limits(), Some(store), |hooks| {
        hooks.stop_budget = Some(budget);
    });
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    let result = end(&mut rig, &mut presenter);
    let CleanupResult::Unproven { reason } = &result else {
        panic!("{result:?}");
    };
    assert!(
        reason.contains("texture store or ledger incomplete"),
        "the store is what is outstanding, named as such: {reason}"
    );
    assert_eq!(scene.probes.observer.holders_released(), 0);
    assert_eq!(scene.probes.observer.layer_retains_outstanding(), 1);
    assert!(Arc::strong_count(&scene.context) > 1);
    assert!(lock(&scene.releases).is_empty());
    open(&gate);

    // (c) Both complete: each graph is released once, in order.
    let kept = scene.context.native_live_objects();
    let scene = Scene::new();
    let mut presenter = scene.start(&rig, limits(), None, |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    scene.driver().offers.push_back((1, 1, 8, 8));
    presenter.texture_wake()();
    until(&mut rig, "the upload adopted", || {
        scene.driver().adopted.iter().any(|list| !list.is_empty())
    });
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(
        *lock(&scene.releases),
        ["store_and_ledger", "presenter", "context"]
    );
    assert_eq!(Arc::strong_count(&scene.context), 1);
    // The live counters are the process's: what the unproven ends above kept stays; this scene added nothing.
    let live = scene.context.native_live_objects();
    assert_eq!(
        (live.textures, live.batches, live.frame_readers),
        (kept.textures, kept.batches, kept.frame_readers),
        "this scene's textures, batches and readers were released"
    );
    assert_eq!(scene.probes.observer.layer_retains_outstanding(), 0);
}

/// The teardown only *observes* until the combined proof is complete (r3, review 1 F1): with a presentation token
/// outstanding, a texture retired by the driver's `Ending` pass and an upload batch completing during the teardown are
/// both still held when the end is `Unproven`, and with everything complete they are released once, after both
/// proofs. The ordinary pump would have dropped the retired texture and the completed batch's staging at once.
fn scene_teardown_observes_without_destroying() {
    let mut rig = Rig::open();
    rig.keep_alive = true;
    retired_texture_survives_an_unproven_end(&mut rig);
    completed_batch_survives_an_unproven_end(&mut rig);
    everything_is_released_once_after_both_proofs(&mut rig);
}

/// (a) A texture retired at `Ending`, a presentation token lost: `Unproven`, the retired texture still held.
fn retired_texture_survives_an_unproven_end(rig: &mut Rig) {
    let budget = Duration::from_millis(700);
    let scene = Scene::new();
    let mut presenter = scene.start(rig, limits(), None, |hooks| {
        hooks.never_complete = NeverComplete::Drawables;
        hooks.stop_budget = Some(budget);
    });
    until(rig, "the first frame", || scene.probes.frames() >= 1);
    scene.driver().offers.push_back((1, 1, 8, 8));
    presenter.texture_wake()();
    until(rig, "the texture adopted", || {
        scene.driver().adopted.iter().any(|list| !list.is_empty())
    });
    {
        let mut state = scene.driver();
        state.retire_at_ending = vec![1];
        state.skip_ending_offer = true;
    }
    let live = scene.context.native_live_objects();
    assert!(live.textures >= 1, "the adopted texture exists");
    let result = end(rig, &mut presenter);
    assert!(
        matches!(result, CleanupResult::Unproven { .. }),
        "{result:?}"
    );
    let after = scene.context.native_live_objects();
    assert_eq!(
        after.textures, live.textures,
        "the retired texture was not dropped before the proof was complete"
    );
    assert!(lock(&scene.releases).is_empty());
    assert!(Arc::strong_count(&scene.context) > 1);
    assert_eq!(scene.probes.observer.holders_released(), 0);
    {
        let state = scene.driver();
        assert_eq!(
            state.passes.last(),
            Some(&(Boundary::Ending, 0)),
            "the Ending pass is handed no events"
        );
    }
}

/// (b) An upload whose completion has arrived but has not been polled when the stop comes, a presentation token
/// lost: `Unproven`, and its batch is still held. The batch's completion is gated: it cannot arrive until the case
/// opens the gate, so the pump that submitted it cannot poll it however long the render thread is paused between the
/// two; the case opens the gate while the render thread is held after that pump, waits for the store to say the
/// completion has arrived and is still unpolled, and only then requests the stop.
fn completed_batch_survives_an_unproven_end(rig: &mut Rig) {
    let scene = Scene::new();
    // The batch's completion cannot arrive until this case opens the gate: the submitting pump's own poll therefore
    // cannot consume it, however long the render thread is paused between its submit and its poll.
    let gate = Arc::new(AtomicI32::new(0));
    // The owner is also stalled for longer than the batch needs between its submit and its poll: the unfavourable
    // schedule. Only the gate keeps the batch unpolled.
    let store = TextureTestHooks {
        completion_gate: Some(Arc::clone(&gate)),
        submit_pause: Duration::from_millis(300),
        ..TextureTestHooks::default()
    };
    let mut presenter = scene.start(rig, limits(), Some(store), |hooks| {
        hooks.never_complete = NeverComplete::Drawables;
        hooks.stop_budget = Some(Duration::from_millis(900));
    });
    until(rig, "the first frame", || scene.probes.frames() >= 1);
    let hold = Arc::new(Hold::default());
    {
        let mut state = scene.driver();
        state.skip_ending_offer = true;
        state.hold_after_submit = Some(Arc::clone(&hold));
        state.offers.push_back((2, 1, 8, 8));
    }
    presenter.texture_wake()();
    // The render thread reaches the hold after a pump that submitted the batch (and could not poll it: the gate is
    // shut); only then does the completion get leave to arrive, while the render thread stays held.
    until(
        rig,
        "the render thread held with the batch submitted",
        || hold.entered.load(Ordering::SeqCst),
    );
    gate.store(1, Ordering::SeqCst);
    until(rig, "the completion arrived, unpolled", || {
        hold.ready.load(Ordering::SeqCst)
    });
    let live = scene.context.native_live_objects();
    assert!(
        live.batches >= 1,
        "the completed batch is still held: {live:?}"
    );
    presenter.request_stop();
    hold.open();
    let mut result = None;
    until(rig, "the end", || {
        result = presenter.poll_end(Duration::ZERO);
        result.is_some()
    });
    assert!(
        matches!(result, Some(CleanupResult::Unproven { .. })),
        "{result:?}"
    );
    let after = scene.context.native_live_objects();
    assert!(
        after.batches >= live.batches && after.textures >= live.textures,
        "the completed batch and its textures were not dropped: {live:?} then {after:?}"
    );
}

/// (c) Everything complete: the retired texture and the batch are released once, after both proofs.
fn everything_is_released_once_after_both_proofs(rig: &mut Rig) {
    let probe = MetalContext::new().expect("a context, to read the process's live objects");
    let baseline = probe.native_live_objects();
    let scene = Scene::new();
    let store = TextureTestHooks {
        completion_delay: Duration::from_millis(150),
        ..TextureTestHooks::default()
    };
    let mut presenter = scene.start(rig, limits(), Some(store), |_| {});
    until(rig, "the first frame", || scene.probes.frames() >= 1);
    {
        let mut state = scene.driver();
        state.skip_ending_offer = true;
        state.offers.push_back((3, 1, 8, 8));
    }
    presenter.texture_wake()();
    until(rig, "the texture adopted", || {
        scene.driver().adopted.iter().any(|list| !list.is_empty())
    });
    scene.driver().retire_at_ending = vec![3];
    assert_eq!(end(rig, &mut presenter), CleanupResult::Proven);
    let live = scene.context.native_live_objects();
    assert_eq!(
        (live.textures, live.batches, live.frame_readers),
        (baseline.textures, baseline.batches, baseline.frame_readers),
        "released after both proofs: nothing of this scene is left"
    );
}

/// The teardown submits no new upload (review 1, F1): a batch prepared and closed when the stop arrives is not
/// committed to the GPU by the `Ending` pass or the proof loop - the ordinary pump would submit it.
fn scene_ending_submits_nothing() {
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let scene = Scene::new();
    let mut presenter = scene.start(&rig, limits(), None, |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    let hold = Arc::new(Hold::default());
    {
        let mut state = scene.driver();
        state.hold_after_offer = Some(Arc::clone(&hold));
        state.skip_ending_offer = true;
        state.offers.push_back((1, 1, 8, 8));
    }
    presenter.texture_wake()();
    // The render thread is held in its pass until the worker has prepared the batch and **closed** the envelope
    // (the store says so: a handshake, not a count of native objects); only then does the stop arrive.
    until(
        &mut rig,
        "a closed envelope waiting to be submitted",
        || hold.ready.load(Ordering::SeqCst),
    );
    presenter.request_stop();
    hold.open();
    let mut result = None;
    until(&mut rig, "the end", || {
        result = presenter.poll_end(Duration::ZERO);
        result.is_some()
    });
    assert_eq!(result, Some(CleanupResult::Proven));
    let state = scene.driver();
    let ending = state.ending_stats.as_ref().expect("the Ending pass");
    assert_eq!(
        ending.upload.batches_submitted, 0,
        "nothing was submitted to the GPU once the stop had arrived: {ending:?}"
    );
}

/// A failure the presenter has observed is published before the driver runs (review 1 F2): an upload error gives a
/// `GpuFailure` and an `UploadFailed`; the driver panics handling the latter; the `Failed` report is there anyway, and
/// the end is `Unproven` with the graph kept.
fn scene_failure_is_published_before_the_driver_runs() {
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let scene = Scene::new();
    let store = TextureTestHooks {
        error_completion: true,
        ..TextureTestHooks::default()
    };
    let mut presenter = scene.start(&rig, limits(), Some(store), |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    {
        let mut state = scene.driver();
        state.panic_on_upload_failed = true;
        state.offers.push_back((1, 1, 8, 8));
    }
    presenter.texture_wake()();
    // The panicking driver ends the render thread by itself (no stop is requested): the end is Unproven.
    let mut result = None;
    until(&mut rig, "the end", || {
        result = presenter.poll_end(Duration::ZERO);
        result.is_some()
    });
    assert!(
        matches!(result, Some(CleanupResult::Unproven { .. })),
        "{result:?}"
    );
    let mut reports = Vec::new();
    while let Some(report) = presenter.take_report() {
        reports.push(report);
    }
    assert_eq!(
        reports
            .iter()
            .filter(|report| matches!(report, PresenterReport::Failed { .. }))
            .count(),
        1,
        "the observed GPU failure was published before the driver panicked: {reports:?}"
    );
    assert_eq!(scene.probes.observer.holders_released(), 0);
    assert!(Arc::strong_count(&scene.context) > 1);
}

/// A setup that fails after the store started (r3, F1): `Failed`, then, with the worker finishing promptly,
/// everything is released and the end is `Proven`; with the worker stuck, everything is retained and `Unproven`; a
/// failure before the store started keeps today's immediate release (no store ever ran).
fn scene_setup_rollback() {
    let mut rig = Rig::open();
    rig.keep_alive = true;

    // The worker finishes promptly.
    let scene = Scene::new();
    let mut presenter = scene.start(&rig, limits(), None, |hooks| {
        // The quad slot is what makes the pipeline be built.
        hooks.quads = Some(ck_gpu_metal::MetalQuadSlot::new());
        hooks.fail_pipeline = true;
    });
    let mut reports = Vec::new();
    assert!(
        reports_until(&mut rig, &presenter, &mut reports, 1),
        "Failed"
    );
    assert!(matches!(reports[0], PresenterReport::Failed { .. }));
    let result = end(&mut rig, &mut presenter);
    assert_eq!(result, CleanupResult::Proven, "{result:?}");
    assert_eq!(Arc::strong_count(&scene.context), 1);
    assert_eq!(scene.probes.observer.layer_retains_outstanding(), 0);
    assert_eq!(
        *lock(&scene.releases),
        ["store_and_ledger", "presenter", "context"]
    );
    assert_eq!(scene.probes.frames(), 0);

    // The worker is stuck: the store cannot be proven by the deadline.
    let scene = Scene::new();
    let (store, gate) = blocked_worker();
    let mut presenter = scene.start(&rig, limits(), Some(store), |hooks| {
        hooks.quads = Some(ck_gpu_metal::MetalQuadSlot::new());
        hooks.fail_pipeline = true;
        hooks.stop_budget = Some(Duration::from_millis(300));
    });
    let mut reports = Vec::new();
    assert!(
        reports_until(&mut rig, &presenter, &mut reports, 1),
        "Failed"
    );
    let result = end(&mut rig, &mut presenter);
    assert!(
        matches!(result, CleanupResult::Unproven { .. }),
        "{result:?}"
    );
    assert!(
        Arc::strong_count(&scene.context) > 1,
        "the context was kept"
    );
    assert_eq!(scene.probes.observer.layer_retains_outstanding(), 1);
    assert_eq!(scene.probes.observer.holders_released(), 0);
    assert!(lock(&scene.releases).is_empty());
    open(&gate);

    // A failure before the store started: released at once, and no store ever ran.
    let scene = Scene::new();
    let mut presenter = scene.start(&rig, limits(), None, |hooks| hooks.fail_setup = true);
    let mut reports = Vec::new();
    assert!(
        reports_until(&mut rig, &presenter, &mut reports, 1),
        "Failed"
    );
    let result = end(&mut rig, &mut presenter);
    assert_eq!(result, CleanupResult::Proven);
    assert_eq!(scene.passes(), 0, "the driver never ran");
    assert_eq!(Arc::strong_count(&scene.context), 1);
    assert!(lock(&scene.releases).is_empty());
}

/// With 1,000 retirements arriving in one wake and a driver relaying at most 256 per pass, every key is relayed
/// with no further wake, then the thread idles; a pass blocked on room reports no `more` and the presenter sleeps
/// until the room wake.
fn scene_backlog_and_room() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    let mut presenter = scene.start(&rig, limits(), None, |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    rig.pump_for(Duration::from_millis(100));
    scene.driver().retire = (0..1_000).collect();
    presenter.texture_wake()();
    until(&mut rig, "every key relayed", || {
        scene.driver().retired.len() >= 1_000
    });
    {
        let state = scene.driver();
        let mut keys = state.retired.clone();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 1_000, "every key once");
    }
    let passes = scene.passes();
    assert!(passes >= 5, "256 per pass needs several passes: {passes}");
    stays(&mut rig, "the passes", Duration::from_millis(400), || {
        scene.passes()
    });
    assert_eq!(scene.probes.frames(), 1, "retirements make no frame wanted");
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);

    // Blocked on room: one prepared slot, its upload held by a late completion.
    let scene = Scene::new();
    let store = TextureTestHooks {
        completion_delay: Duration::from_millis(700),
        ..TextureTestHooks::default()
    };
    let textures = TextureLimits {
        max_prepared_images: 2,
        max_upload_images_per_batch: 1,
        ..TextureLimits::default()
    };
    let mut presenter = scene.start_with_limits(&rig, textures, limits(), Some(store), |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    scene
        .driver()
        .offers
        .extend([(1, 1, 8, 8), (2, 1, 8, 8), (3, 1, 8, 8)]);
    presenter.texture_wake()();
    until(&mut rig, "the pass that finds no room", || {
        scene.driver().blocked_on_room > 0
    });
    let blocked = scene.passes();
    rig.pump_for(Duration::from_millis(300));
    assert!(
        scene.passes() <= blocked + 3,
        "a pass blocked on room reports no more: no spinning ({} passes then {})",
        blocked,
        scene.passes()
    );
    until(&mut rig, "the second offer, after the room wake", || {
        scene.driver().offers.is_empty()
    });
    until(&mut rig, "all prepared", || {
        scene.driver().prepared.len() >= 3
    });
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A pass blocked on the ledger's replacement room is resumed by the room wake alone: with room for one extra
/// revision, the third revision of a key waits until the adoption of the second releases the first, and nothing but
/// the room wake brings the presenter back to offer it.
fn scene_room_wake() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    let mut presenter = scene.start(
        &rig,
        LedgerLimits {
            max_extra_revisions: 1,
            max_adoptions_per_frame: 256,
        },
        None,
        |_| {},
    );
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    scene.driver().offers.push_back((1, 1, 8, 8));
    presenter.texture_wake()();
    until(&mut rig, "the baseline adopted", || {
        scene.driver().adopted.iter().any(|list| list == &[(1, 1)])
    });
    // Revision 2 takes the one extra charge; revision 3 finds none.
    scene.driver().offers.extend([(1, 2, 8, 8), (1, 3, 8, 8)]);
    presenter.texture_wake()();
    until(&mut rig, "a pass that finds no replacement room", || {
        scene.driver().blocked_on_room > 0
    });
    // The adoption of revision 2 retires revision 1: its extra charge is released, and the ledger's room wake says so.
    // Nothing else wakes the thread (no frame is wanted, no upload is pending): only that wake offers revision 3.
    until(&mut rig, "revision 3 adopted", || {
        scene.driver().adopted.iter().any(|list| list == &[(1, 3)])
    });
    assert_eq!(scene.driver().prepared.len(), 3);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// Teardown: `Proven` with an idle store, and with an upload in flight that completes before the deadline;
/// `Unproven` (everything retained) when an upload's completion, the worker or a frame token is held past the
/// deadline; one deadline across the stages; the driver's `Ending` pass comes before the store stops.
fn scene_teardown() {
    let mut rig = Rig::open();
    rig.keep_alive = true;

    // Idle store: Proven; the Ending pass is the last pass and the store still accepted an offer there.
    let scene = Scene::new();
    let mut presenter = scene.start(&rig, limits(), None, |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    await_presented(&mut rig, &scene.probes.observer);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    {
        let state = scene.driver();
        assert_eq!(
            state.passes.last().map(|(boundary, _)| *boundary),
            Some(Boundary::Ending),
            "{:?}",
            state.passes
        );
        assert_eq!(
            state
                .passes
                .iter()
                .filter(|(boundary, _)| *boundary == Boundary::Ending)
                .count(),
            1
        );
        assert_eq!(
            state.ending_offer,
            Some(true),
            "the Ending pass reached the driver before the store stopped"
        );
    }

    // An upload in flight that completes before the deadline: Proven.
    let scene = Scene::new();
    let store = TextureTestHooks {
        completion_delay: Duration::from_millis(250),
        ..TextureTestHooks::default()
    };
    let mut presenter = scene.start(&rig, limits(), Some(store), |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    scene.driver().offers.push_back((1, 1, 8, 8));
    presenter.texture_wake()();
    until(&mut rig, "the upload submitted", || {
        scene
            .driver()
            .stats
            .as_ref()
            .is_some_and(|stats| stats.upload.batches_submitted > 0)
    });
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// Teardown, the unproven side: an upload's completion, the worker or a frame token held past the deadline leaves
/// everything retained (the layer retain included), and the end comes at the one deadline.
fn scene_teardown_unproven() {
    let mut rig = Rig::open();
    rig.keep_alive = true;

    // An upload completion held past the deadline: Unproven, everything retained (including the layer retain).
    let scene = Scene::new();
    let store = TextureTestHooks {
        completion_delay: Duration::from_secs(3),
        ..TextureTestHooks::default()
    };
    let mut presenter = scene.start(&rig, limits(), Some(store), |hooks| {
        hooks.stop_budget = Some(Duration::from_millis(400));
    });
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    scene.driver().offers.push_back((1, 1, 8, 8));
    presenter.texture_wake()();
    until(&mut rig, "the upload submitted", || {
        scene
            .driver()
            .stats
            .as_ref()
            .is_some_and(|stats| stats.upload.batches_submitted > 0)
    });
    let started = Instant::now();
    presenter.request_stop();
    let result = loop {
        if let Some(result) = presenter.poll_end(Duration::ZERO) {
            break result;
        }
        rig.pump_once();
        assert!(started.elapsed() < BOUND, "never ended");
    };
    assert!(
        matches!(result, CleanupResult::Unproven { .. }),
        "{result:?}"
    );
    assert_eq!(scene.probes.observer.layer_retains_outstanding(), 1);
    assert_eq!(scene.probes.observer.holders_released(), 0);
    assert!(Arc::strong_count(&scene.context) > 1);

    // The worker held past the deadline: Unproven.
    let scene = Scene::new();
    let (store, gate) = blocked_worker();
    let mut presenter = scene.start(&rig, limits(), Some(store), |hooks| {
        hooks.stop_budget = Some(Duration::from_millis(300));
    });
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    let result = end(&mut rig, &mut presenter);
    assert!(
        matches!(result, CleanupResult::Unproven { .. }),
        "{result:?}"
    );
    assert_eq!(scene.probes.observer.layer_retains_outstanding(), 1);
    open(&gate);

    // A frame token held past the deadline: Unproven.
    let scene = Scene::new();
    let mut presenter = scene.start(&rig, limits(), None, |hooks| {
        hooks.never_complete = NeverComplete::Commands;
        hooks.stop_budget = Some(Duration::from_millis(300));
    });
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    let result = end(&mut rig, &mut presenter);
    assert!(
        matches!(result, CleanupResult::Unproven { .. }),
        "{result:?}"
    );
    assert_eq!(scene.probes.observer.layer_retains_outstanding(), 1);
    assert_eq!(scene.probes.observer.holders_released(), 0);

    // One deadline across the stages: with both sides held, the end comes at the budget, not at twice it.
    let scene = Scene::new();
    let (store, gate) = blocked_worker();
    let budget = Duration::from_millis(500);
    let mut presenter = scene.start(&rig, limits(), Some(store), |hooks| {
        hooks.never_complete = NeverComplete::Commands;
        hooks.stop_budget = Some(budget);
    });
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    let started = Instant::now();
    let result = end(&mut rig, &mut presenter);
    let took = started.elapsed();
    assert!(
        matches!(result, CleanupResult::Unproven { .. }),
        "{result:?}"
    );
    assert!(
        took < budget + Duration::from_millis(900),
        "one deadline, never restarted per stage: {took:?}"
    );
    open(&gate);
}

/// A store failure, a latched ledger failure or a texture driver that panics: the existing rules hold (a panic on
/// the render thread keeps everything and ends `Unproven`).
fn scene_driver_panic() {
    struct Panicking(Arc<AtomicBool>);
    impl TextureDriver<u32> for Panicking {
        fn pass(
            &mut self,
            _ledger: &mut TextureLedger<u32>,
            boundary: Boundary,
            _events: Vec<LedgerEvent<u32>>,
        ) -> PassOutcome {
            if boundary == Boundary::Frame {
                self.0.store(true, Ordering::SeqCst);
                panic!("a driver pass panicked");
            }
            PassOutcome::default()
        }
    }
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let context = MetalContext::new().expect("a device");
    let probes = Probes::new();
    let seen = Arc::new(AtomicBool::new(false));
    let mut hooks = probes.hooks();
    hooks.scene = Some(SceneSetup::new(
        Arc::clone(&context),
        TextureLimits::default(),
        limits(),
        scene_limits(),
        Panicking(Arc::clone(&seen)),
        MetalFrameSlot::<u32>::new(),
        None,
        None,
    ));
    let mut presenter = probes.start_ok(&rig, hooks);
    until(&mut rig, "the panicking pass", || {
        seen.load(Ordering::SeqCst)
    });
    let result = end(&mut rig, &mut presenter);
    assert!(
        matches!(result, CleanupResult::Unproven { .. }),
        "{result:?}"
    );
    assert_eq!(probes.observer.holders_released(), 0);
    assert!(Arc::strong_count(&context) > 1, "the context was kept");
}

/// `start_scene_bindless` itself (the public entry): the same layer contract, a quad slot, and the frames are the scene's.
fn scene_start_scene() {
    let mut rig = Rig::open();
    let context = MetalContext::new().expect("a device");
    let state = Arc::new(Mutex::new(DriverState::default()));
    let slot = MetalFrameSlot::<u32>::new();
    let wakes = Arc::new(AtomicUsize::new(0));
    let wake = {
        let wakes = Arc::clone(&wakes);
        Arc::new(move || {
            wakes.fetch_add(1, Ordering::SeqCst);
        })
    };
    // SAFETY: `rig.layer` is the live `CAMetalLayer` of the rig's window, which outlives the presenter (the case
    // waits for its proven end).
    let mut presenter = unsafe {
        MetalPresenter::start_scene_bindless(
            rig.layer,
            rig.pixels,
            [0.1, 0.2, 0.3],
            wake,
            slot,
            Arc::clone(&context),
            TextureLimits::default(),
            limits(),
            BindlessLimits {
                texture_slots: 64,
                max_textures: 16,
                max_extra_revisions: 16,
            },
            scene_limits(),
            TestDriver::new(&state),
        )
    }
    .expect("start_scene_bindless");
    until(&mut rig, "a boundary pass", || {
        lock(&state)
            .passes
            .iter()
            .any(|(boundary, _)| *boundary == Boundary::Frame)
    });
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(Arc::strong_count(&context), 1);
}

/// The scene stats count uploads and texture creations cumulatively: images offered by the driver raise `textures_created`
/// and the submitted and completed upload counts, frames drawn with no new revision move none of them, a later image raises
/// them again, and none ever decreases. (The movement scenarios of `ck-render-2d` read these to show that moving quads
/// uploads nothing.)
fn scene_stats_count_uploads_and_creations_cumulatively() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().adopt_limit = 8;
    let mut presenter = scene.start(&rig, limits_adopting(8), None, |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    let counters = |presenter: &MetalPresenter| {
        let stats = presenter.stats();
        (
            stats.textures_created,
            stats.uploads_submitted,
            stats.uploads_completed,
        )
    };
    let start = counters(&presenter);

    // Three images: three textures created, at least one batch submitted, every submitted batch completed.
    {
        let mut state = scene.driver();
        state
            .offers
            .extend([(1, 1, 8, 8), (2, 1, 8, 8), (3, 1, 8, 8)]);
    }
    presenter.texture_wake()();
    until(&mut rig, "all three adopted", || {
        scene.driver().adopted.iter().map(Vec::len).sum::<usize>() >= 3
    });
    let frames = scene.probes.frames();
    presenter.request_redraw();
    until(&mut rig, "a frame after the adoptions", || {
        scene.probes.frames() > frames
    });
    let after = counters(&presenter);
    assert_eq!(
        after.0,
        start.0 + 3,
        "three textures were created: {start:?} then {after:?}"
    );
    assert!(
        after.1 > start.1,
        "an upload batch was submitted: {start:?} then {after:?}"
    );
    assert_eq!(
        after.1, after.2,
        "every submitted batch completed: {after:?}"
    );

    // Frames with no new revision move none of the counters.
    for _ in 0..8 {
        let frames = scene.probes.frames();
        presenter.request_redraw();
        until(&mut rig, "a frame with no new revision", || {
            scene.probes.frames() > frames
        });
    }
    assert_eq!(counters(&presenter), after, "frames alone upload nothing");

    // Another revision of a key already shown: counted again, and nothing went back down.
    scene.driver().offers.push_back((1, 2, 8, 8));
    presenter.texture_wake()();
    until(&mut rig, "the new revision adopted", || {
        scene.driver().adopted.iter().map(Vec::len).sum::<usize>() >= 4
    });
    let frames = scene.probes.frames();
    presenter.request_redraw();
    until(&mut rig, "a frame after the new revision", || {
        scene.probes.frames() > frames
    });
    let later = counters(&presenter);
    assert_eq!(
        later.0,
        after.0 + 1,
        "one more texture was created: {after:?} then {later:?}"
    );
    assert!(
        later.1 > after.1 && later.2 >= after.2,
        "{after:?} then {later:?}"
    );
    assert_eq!(later.1, later.2);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// The scene stats carry the upload store's **own** byte high-water marks, which are not the presenter's checker baselines: with no
/// upload yet both peaks are zero while the checker's baseline bytes are not; an 8 x 8 image raises both to at least its pixel
/// bytes; a larger image raises them again; and later frames, and retiring the images, never lower them. Negative control: the
/// peaks equal to the checker baseline (a reading of the wrong counter) or a peak that fell would fail.
fn scene_stats_carry_the_upload_stores_byte_peaks() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().adopt_limit = 8;
    let mut presenter = scene.start(&rig, limits_adopting(8), None, |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    let none = presenter.stats();
    assert!(
        none.checker_texture_bytes > 0 && none.checker_staging_bytes > 0,
        "the checker has baseline bytes: {none:?}"
    );
    assert_eq!(
        (
            none.upload_texture_bytes_peak,
            none.upload_staging_bytes_peak
        ),
        (0, 0),
        "no upload yet, so the store's peaks are zero whatever the checker's baseline is: {none:?}"
    );
    let adopted =
        |count: usize| scene.driver().adopted.iter().map(Vec::len).sum::<usize>() >= count;
    let frame_after = |rig: &mut Rig, presenter: &MetalPresenter| {
        let frames = scene.probes.frames();
        presenter.request_redraw();
        until(rig, "a frame after the adoption", || {
            scene.probes.frames() > frames
        });
    };
    scene.driver().offers.push_back((1, 1, 8, 8));
    presenter.texture_wake()();
    until(&mut rig, "the small image adopted", || adopted(1));
    frame_after(&mut rig, &presenter);
    let small = presenter.stats();
    assert!(
        small.upload_texture_bytes_peak >= 8 * 8 * 4,
        "the store charged at least the image's pixel bytes: {small:?}"
    );
    assert!(small.upload_staging_bytes_peak >= 8 * 8 * 4, "{small:?}");
    assert_ne!(
        small.upload_texture_bytes_peak, small.checker_texture_bytes,
        "the peak is the store's, not the checker baseline: {small:?}"
    );
    // (The staging peak of one small image is one staging block, which happens to equal the checker's 16 KiB staging buffer, so
    // only the texture peak is told apart from the checker's baseline by value here. The staging peak is only required to be at
    // least the pixel bytes and never to fall; it is not shown to differ from the checker's baseline.)
    scene.driver().offers.push_back((2, 1, 32, 32));
    presenter.texture_wake()();
    until(&mut rig, "the large image adopted", || adopted(2));
    frame_after(&mut rig, &presenter);
    let large = presenter.stats();
    assert!(
        large.upload_texture_bytes_peak > small.upload_texture_bytes_peak,
        "a larger image raises the texture peak: {small:?} then {large:?}"
    );
    assert!(
        large.upload_staging_bytes_peak >= small.upload_staging_bytes_peak,
        "and never lowers the staging peak: {small:?} then {large:?}"
    );
    // Frames alone and retiring the images never lower a high-water mark.
    scene.driver().retire.extend([1, 2]);
    presenter.texture_wake()();
    for _ in 0..4 {
        frame_after(&mut rig, &presenter);
    }
    let later = presenter.stats();
    assert_eq!(
        (
            later.upload_texture_bytes_peak,
            later.upload_staging_bytes_peak
        ),
        (
            large.upload_texture_bytes_peak,
            large.upload_staging_bytes_peak
        ),
        "high-water marks never decrease: {large:?} then {later:?}"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// What the store charged **in the `Ending` pass** is in the final scene stats (review 2 F2, review 3): an 8 x 8 image is shown first
/// (so the peaks are not zero), then the `Ending` pass offers a 64 x 64 one and is held, through the driver's `hold_after_offer`
/// handshake, until the store has closed its envelope for it; so the charge is made inside the pass whatever the schedule, before
/// the stop's proof cancels waiting work. After the `Proven` end the stats' texture peak is above the one published before the stop.
/// What it binds: the `Ending` pass publishes (before this change that pass returned before publishing). What it does **not** bind,
/// an untested limit: the last look before the release, for a charge that lands between the `Ending` pass and the release (the worker
/// leaves on `stopping` before taking waiting work and the store has no hook to land one there), which is published by construction.
fn scene_stats_hold_the_peaks_charged_in_the_ending_pass() {
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let scene = Scene::new();
    scene.driver().adopt_limit = 8;
    let mut presenter = scene.start(&rig, limits_adopting(8), None, |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    scene.driver().offers.push_back((1, 1, 8, 8));
    presenter.texture_wake()();
    until(&mut rig, "the small image adopted", || {
        scene.driver().adopted.iter().map(Vec::len).sum::<usize>() >= 1
    });
    let frames = scene.probes.frames();
    presenter.request_redraw();
    until(&mut rig, "a frame after the adoption", || {
        scene.probes.frames() > frames
    });
    let before = presenter.stats();
    assert!(
        before.upload_texture_bytes_peak >= 8 * 8 * 4,
        "the small image's peak was published: {before:?}"
    );
    // The Ending-pass offer is held until the store has closed its envelope for it: the charge is made **inside** the Ending pass
    // (a handshake, not a schedule), before the stop's proof cancels waiting work (`prove` fails every job still waiting, and the
    // worker leaves on `stopping` before taking one).
    let hold = Arc::new(Hold::default());
    {
        let mut state = scene.driver();
        state.ending_offer_size = Some((64, 64));
        state.hold_after_offer = Some(Arc::clone(&hold));
    }
    presenter.request_stop();
    until(
        &mut rig,
        "a closed envelope for the Ending-pass offer",
        || hold.ready.load(Ordering::SeqCst),
    );
    hold.open();
    let mut result = None;
    until(&mut rig, "the end", || {
        result = presenter.poll_end(Duration::ZERO);
        result.is_some()
    });
    assert_eq!(result, Some(CleanupResult::Proven));
    let after = presenter.stats();
    assert_eq!(
        scene.driver().ending_offer,
        Some(true),
        "the Ending pass offered the large image"
    );
    // The offer was made inside the Ending pass, after that pass's own counters were taken and with no pump after it: only the
    // last look before the store was released can have published what the store charged for it while it was proven.
    assert!(
        after.upload_texture_bytes_peak > before.upload_texture_bytes_peak,
        "the final stats hold the texture charge made in the Ending pass: {before:?} then {after:?}"
    );
}

/// The figures stay available around a failure (review 2 F2, review 3): with the upload completion made to fail, a 64 x 64 image is
/// offered and **no stop is requested**; once the `Failed` report is out, the stats show the image's texture and staging charges while
/// the presenter is still running. **What this does not bind:** that the pass which found the failure published them. A successful
/// pass between the allocation and the completion error can already have published the same peaks, so this case can pass with the old
/// early return; it is not causal evidence for the failing-pass publication, which is by construction (the publication sits above the
/// early return). The case then ends the presenter.
fn scene_stats_stay_available_after_an_upload_failure() {
    let mut rig = Rig::open();
    rig.keep_alive = true;
    let scene = Scene::new();
    let store = TextureTestHooks {
        error_completion: true,
        ..TextureTestHooks::default()
    };
    let mut presenter = scene.start(&rig, limits(), Some(store), |_| {});
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    scene.driver().offers.push_back((1, 1, 64, 64));
    presenter.texture_wake()();
    let mut reports = Vec::new();
    until(&mut rig, "the upload failure is reported", || {
        while let Some(report) = presenter.take_report() {
            reports.push(report);
        }
        reports
            .iter()
            .any(|report| matches!(report, PresenterReport::Failed { .. }))
    });
    until(
        &mut rig,
        "the failing pass published the store's peaks",
        || {
            let stats = presenter.stats();
            stats.upload_texture_bytes_peak >= 64 * 64 * 4
                && stats.upload_staging_bytes_peak >= 64 * 64 * 4
        },
    );
    let _ = end(&mut rig, &mut presenter);
}

pub(crate) const CASES: [(&str, fn()); 21] = [
    ("scene_shares_the_context", scene_shares_the_context),
    (
        "scene_driver_wakes_and_adoption",
        scene_driver_wakes_and_adoption,
    ),
    (
        "scene_stats_count_uploads_and_creations_cumulatively",
        scene_stats_count_uploads_and_creations_cumulatively,
    ),
    (
        "scene_stats_carry_the_upload_stores_byte_peaks",
        scene_stats_carry_the_upload_stores_byte_peaks,
    ),
    (
        "scene_stats_hold_the_peaks_charged_in_the_ending_pass",
        scene_stats_hold_the_peaks_charged_in_the_ending_pass,
    ),
    (
        "scene_stats_stay_available_after_an_upload_failure",
        scene_stats_stay_available_after_an_upload_failure,
    ),
    (
        "scene_wanted_frame_comes_first",
        scene_wanted_frame_comes_first,
    ),
    ("scene_redraw_for_adoption", scene_redraw_for_adoption),
    ("scene_tickets_follow_frames", scene_tickets_follow_frames),
    (
        "scene_tickets_busy_and_delayed",
        scene_tickets_busy_and_delayed,
    ),
    ("scene_gpu_failure", scene_gpu_failure),
    ("scene_combined_proof", scene_combined_proof),
    (
        "scene_teardown_observes_without_destroying",
        scene_teardown_observes_without_destroying,
    ),
    ("scene_ending_submits_nothing", scene_ending_submits_nothing),
    (
        "scene_failure_is_published_before_the_driver_runs",
        scene_failure_is_published_before_the_driver_runs,
    ),
    ("scene_setup_rollback", scene_setup_rollback),
    ("scene_backlog_and_room", scene_backlog_and_room),
    ("scene_room_wake", scene_room_wake),
    ("scene_teardown", scene_teardown),
    ("scene_teardown_unproven", scene_teardown_unproven),
    ("scene_driver_panic", scene_driver_panic),
];

pub(crate) const MORE: [(&str, fn()); 1] = [("scene_start_scene", scene_start_scene)];
