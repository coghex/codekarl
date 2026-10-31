//! The scene presenter on a real window (milestone 5.5, issue 2a): the checker and the first frame, adoption redraws,
//! the idle rule, the present ledger on the texture ledger's frame proofs, the instance buffer, the format notice and the
//! proven and unproven ends. A submodule of `window.rs`, so its cases run in the same workers, each in its own process,
//! with **synchronization validation** on (any validation message fails the case), with the keyboard filter and the
//! owner-typing run.
//!
//! No case sleeps to synchronise: a hold is a test hook (a store gate, a pause point, a `VkEvent` a frame waits on, held
//! present fences) and a wait is on an observable fact.

use std::collections::VecDeque;

use ck_gpu_vulkan::{
    Boundary, BuildRecord, DestinationRect, InstanceRole, LedgerEvent, LedgerLimits, PassOutcome,
    Pause, PixelSource, QuadSkipped, SceneLimits, SceneSetup, SceneStats, StallGate, TestFeatures,
    TextureDriver, TextureLedger, TextureLimits, TextureTestHooks, TicketEnding, VulkanContext,
    VulkanFilter, VulkanFrame, VulkanFrameSlot, VulkanInstance, VulkanSceneSource, WaitKind,
    WaitObserver,
};

use super::{
    Arc, AtomicBool, BOUND, Case, CleanupResult, Duration, Event, Instant, Mutex, Ordering,
    PresenterReport, Probes, Rig, TestHooks, VulkanPresenter, check_ledger_order_with,
    check_nothing_destroyed, count, failed_messages, reports, vk,
};

pub(crate) mod compat;

/// The scene a case publishes: its instances in painter order.
struct Quads(Vec<VulkanInstance<u32>>);

impl VulkanSceneSource<u32> for Quads {
    fn place(&self, _width: u32, _height: u32, out: &mut dyn FnMut(VulkanInstance<u32>)) {
        for instance in &self.0 {
            out(*instance);
        }
    }
}

/// A solid image.
struct Solid(Vec<u8>);

impl PixelSource for Solid {
    fn pixels(&self) -> &[u8] {
        &self.0
    }
}

fn solid(width: u32, height: u32, texel: [u8; 4]) -> Arc<Solid> {
    Arc::new(Solid(texel.repeat(width as usize * height as usize)))
}

const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn quad(key: u32, left: i32, top: i32, side: i32) -> VulkanInstance<u32> {
    VulkanInstance {
        key,
        destination: DestinationRect {
            left,
            top,
            right: left + side,
            bottom: top + side,
        },
        source: WHOLE,
        role: InstanceRole::Scene,
    }
}

/// One image the driver is to offer.
struct Offer {
    key: u32,
    revision: u64,
    width: u32,
    height: u32,
    pixels: Arc<Solid>,
}

/// What the fake driver does and records. Shared with the case.
#[derive(Default)]
struct DriverState {
    /// Images to offer when the window has room.
    offers: VecDeque<Offer>,
    /// The ledger's events, in order.
    events: Vec<LedgerEvent<u32>>,
    /// The boundary of each pass.
    boundaries: Vec<Boundary>,
    /// What was adopted, in order.
    adopted: Vec<(u32, u64)>,
    /// Passes left that report `more = true`.
    more: usize,
    /// The number of waits the thread had entered at each pass.
    waits_at_pass: Vec<usize>,
    /// For each image the driver offered to the ledger: its key and the number of waits the thread had entered at the pass
    /// that offered it (the thread cannot enter a wait during a pass, so the next wait is strictly newer than this count).
    offered_at: Vec<(u32, usize)>,
    waits: Option<Arc<WaitObserver>>,
}

struct Driver(Arc<Mutex<DriverState>>);

impl TextureDriver<u32> for Driver {
    fn pass(
        &mut self,
        ledger: &mut TextureLedger<u32>,
        boundary: Boundary,
        events: Vec<LedgerEvent<u32>>,
    ) -> PassOutcome {
        let mut state = self.0.lock().expect("the driver state");
        state.boundaries.push(boundary);
        let waited = state.waits.as_ref().map_or(0, |waits| waits.waits());
        state.waits_at_pass.push(waited);
        for event in events {
            if let LedgerEvent::Prepared { key, revision } = &event {
                ledger.commit(*key, *revision).expect("commit");
            }
            state.events.push(event);
        }
        while ledger.room() > 0 && ledger.extra_room() > 0 {
            let Some(offer) = state.offers.pop_front() else {
                break;
            };
            state.offered_at.push((offer.key, waited));
            ledger
                .offer(
                    offer.key,
                    offer.revision,
                    offer.width,
                    offer.height,
                    offer.pixels as Arc<dyn PixelSource>,
                )
                .expect("the offer is admitted");
        }
        if boundary == Boundary::Frame {
            let mut adopted = Vec::new();
            ledger.adopt(256, &mut adopted);
            state.adopted.extend(adopted);
        }
        let more = if state.more > 0 {
            state.more -= 1;
            true
        } else {
            false
        };
        PassOutcome { more }
    }
}

/// A window, a context made for its surface, a frame slot and the places the presenter reports to.
struct Harness {
    rig: Rig,
    probes: Probes,
    context: Arc<VulkanContext>,
    frames: Arc<VulkanFrameSlot<u32>>,
    waits: Arc<WaitObserver>,
    builds: Arc<Mutex<Vec<BuildRecord>>>,
    tickets: Arc<Mutex<Vec<TicketEnding>>>,
    driver: Arc<Mutex<DriverState>>,
    generation: u64,
}

impl Harness {
    fn new() -> Self {
        let rig = Rig::open_with(true);
        let context = VulkanContext::new_for_tests(
            &rig.instance,
            Some(rig.surface),
            TestFeatures { events: true },
        )
        .expect("a context made for the surface, with host events");
        let waits = Arc::new(WaitObserver::new());
        let driver = Arc::new(Mutex::new(DriverState {
            waits: Some(Arc::clone(&waits)),
            ..DriverState::default()
        }));
        Self {
            rig,
            probes: Probes::new(),
            context,
            frames: VulkanFrameSlot::new(),
            waits,
            builds: Arc::new(Mutex::new(Vec::new())),
            tickets: Arc::new(Mutex::new(Vec::new())),
            driver,
            generation: 0,
        }
    }

    fn limits() -> SceneLimits {
        SceneLimits {
            max_quads: 256,
            binding_geometry_bytes: 1 << 20,
        }
    }

    fn start(&self) -> VulkanPresenter {
        self.start_with(Self::limits(), None, self.rig.pixel, |_: &mut TestHooks| {})
    }

    fn start_with(
        &self,
        limits: SceneLimits,
        texture_hooks: Option<TextureTestHooks>,
        size: (u32, u32),
        edit: impl FnOnce(&mut TestHooks),
    ) -> VulkanPresenter {
        let mut hooks = self.probes.hooks();
        hooks.scene.waits = Some(Arc::clone(&self.waits));
        hooks.scene.builds = Some(Arc::clone(&self.builds));
        hooks.scene.tickets = Some(Arc::clone(&self.tickets));
        edit(&mut hooks);
        let setup = SceneSetup::new(
            Arc::clone(&self.context),
            TextureLimits::default(),
            LedgerLimits::default(),
            64,
            limits,
            Driver(Arc::clone(&self.driver)),
            Arc::clone(&self.frames),
            texture_hooks,
        );
        let wakes = Arc::clone(&self.probes.wakes);
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            wakes.fetch_add(1, Ordering::SeqCst);
        });
        // SAFETY: the surface was created on this instance for a window that outlives the presenter's proven end (the
        // rig destroys it after), the context was made for it, and nothing else uses the surface meanwhile.
        unsafe {
            VulkanPresenter::start_scene_with_test_hooks(
                self.rig.surface,
                size,
                [0.1, 0.2, 0.3],
                wake,
                &setup,
                hooks,
            )
        }
        .expect("the scene presenter starts")
    }

    /// Publishes a frame of `instances` (the next generation).
    fn publish(&mut self, instances: Vec<VulkanInstance<u32>>) {
        self.generation += 1;
        self.frames.publish(VulkanFrame::new(
            self.generation,
            VulkanFilter::Nearest,
            None,
            Some(Arc::new(Quads(instances))),
        ));
    }

    fn offer(&self, key: u32, side: u32, texel: [u8; 4]) {
        self.driver
            .lock()
            .expect("the driver state")
            .offers
            .push_back(Offer {
                key,
                revision: 1,
                width: side,
                height: side,
                pixels: solid(side, side, texel),
            });
    }

    fn builds(&self) -> Vec<BuildRecord> {
        self.builds.lock().expect("the builds").clone()
    }

    fn state<T>(&self, read: impl FnOnce(&DriverState) -> T) -> T {
        read(&self.driver.lock().expect("the driver state"))
    }

    /// Waits for `condition` on the presenter's stats.
    fn wait_stats(
        &mut self,
        presenter: &VulkanPresenter,
        what: &str,
        condition: impl Fn(&SceneStats) -> bool,
    ) {
        self.rig.wait(what, || condition(&presenter.scene_stats()));
    }

    /// Waits for `condition` on the harness itself (its driver and build records), pumping the window.
    fn wait_for(&mut self, what: &str, condition: impl Fn(&Self) -> bool) {
        let give_up = Instant::now() + BOUND;
        while !condition(self) {
            assert!(Instant::now() < give_up, "timed out waiting for {what}");
            self.rig.pump();
            std::thread::sleep(Duration::from_millis(3));
        }
    }

    /// Ends the presenter proven; the teardown order and the books hold; the context goes before the instance. Closes the
    /// window.
    fn finish(mut self, presenter: &mut VulkanPresenter) {
        let ended = Rig::end(presenter);
        assert_eq!(ended, CleanupResult::Proven, "the scene presenter's end");
        let events = self.probes.events();
        check_ledger_order_with(
            &events,
            &[
                "command buffers and pool",
                "fences and semaphores",
                "image views and framebuffers",
                "render pass",
                "swap chains",
            ],
        );
        assert!(
            self.tickets
                .lock()
                .expect("the tickets")
                .iter()
                .all(|ending| matches!(ending, TicketEnding::Submitted | TicketEnding::Discarded)),
            "every ticket ended as the frame did"
        );
        assert_eq!(
            Arc::strong_count(&self.context),
            1,
            "the proven end released every hold on the context"
        );
        drop(self.context);
        self.rig.idle(Duration::from_millis(1));
        self.rig.close(&ended);
    }
}

/// An event the held frames wait on.
fn event(harness: &Harness) -> vk::Event {
    // SAFETY: the event belongs to the case's own device and is destroyed before the context goes.
    unsafe {
        harness
            .context
            .device()
            .create_event(&vk::EventCreateInfo::default(), None)
    }
    .expect("the host-event feature is enabled")
}

fn set_event(harness: &Harness, event: vk::Event) {
    // SAFETY: the event belongs to this device.
    unsafe { harness.context.device().set_event(event) }.expect("set the event");
}

fn destroy_event(harness: &Harness, event: vk::Event) {
    // SAFETY: every frame that waited on the event is proven finished.
    unsafe { harness.context.device().destroy_event(event, None) };
}

/// A store gate that is closed, and the flag the worker sets when it is waiting at it.
fn closed_gate() -> (StallGate, Arc<AtomicBool>) {
    (
        Arc::new((Mutex::new(false), std::sync::Condvar::new())),
        Arc::new(AtomicBool::new(false)),
    )
}

fn open_gate(gate: &StallGate) {
    let (mutex, changed) = &**gate;
    *mutex.lock().expect("the gate") = true;
    changed.notify_all();
}

fn texture_hooks(gate: &StallGate, reached: &Arc<AtomicBool>) -> TextureTestHooks {
    TextureTestHooks {
        stall_gate: Some(Arc::clone(gate)),
        stall_reached: Some(Arc::clone(reached)),
        ..TextureTestHooks::default()
    }
}

fn wait_flag(harness: &mut Harness, what: &str, flag: &AtomicBool) {
    harness.rig.wait(what, || flag.load(Ordering::SeqCst));
}

// ---------------------------------------------------------------------------------------------
// 4. The checker and the first frame
// ---------------------------------------------------------------------------------------------

/// A scene published before the start is in the first frame, which shows the checker for a key with nothing shown: its
/// instance is drawn as the checker (the build counted it), `FirstFrameQueued` is reported once, and the end is proven.
fn the_first_frame_shows_the_checker_for_a_pending_key() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start();
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    let builds = harness.builds();
    assert_eq!(builds[0].counters.instances, 1);
    assert_eq!(
        builds[0].counters.checker, 1,
        "the pending key drew the checker"
    );
    assert_eq!(builds[0].counters.resolved, 0);
    assert!(builds.iter().all(|build| build.previous_proven));
    let stats = presenter.scene_stats();
    assert!(
        stats.checker_resident && stats.checker_bytes > 0,
        "{stats:?}"
    );
    assert_eq!(reports(&presenter), vec![PresenterReport::FirstFrameQueued]);
    harness.finish(&mut presenter);
}

/// With the checker's upload held (the store's gate), no frame is committed and none is attempted; releasing the gate
/// alone makes the first frame run, showing the checker.
///
/// Negative control: the idle rule's unit test (`render/scene.rs`) shows that treating "wanted with the previous
/// submission proven" as runnable attempts frames with the checker pending.
fn a_held_checker_parks_the_first_frame_and_its_release_alone_runs_it() {
    let mut harness = Harness::new();
    let (gate, reached) = closed_gate();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(
        Harness::limits(),
        Some(texture_hooks(&gate, &reached)),
        harness.rig.pixel,
        |_| {},
    );
    wait_flag(&mut harness, "the worker at the gate", &reached);
    // A wait is entered (the thread has nothing to run): look at it, not at a clock.
    let waits = Arc::clone(&harness.waits);
    harness
        .rig
        .wait("the thread to wait", || waits.waits() >= 1);
    let attempts = harness.waits.attempts();
    harness.rig.idle(Duration::from_millis(100));
    assert_eq!(presenter.scene_stats().frames, 0, "no frame is committed");
    assert_eq!(harness.waits.attempts(), attempts, "none was attempted");
    assert_eq!(attempts, 0, "not even once");
    assert_eq!(
        harness.waits.entries().last(),
        Some(&WaitKind::Unbounded),
        "nothing is outstanding while the worker holds the checker: a wait without a bound"
    );
    open_gate(&gate);
    harness.wait_stats(&presenter, "the first frame after the release", |stats| {
        stats.frames >= 1
    });
    assert!(
        harness.builds()[0].counters.checker >= 1,
        "the checker is shown"
    );
    harness.finish(&mut presenter);
}

/// An injected checker failure is one `Failed` report, and nothing is drawn.
fn an_injected_checker_failure_is_one_failed_and_draws_nothing() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(
        Harness::limits(),
        Some(TextureTestHooks {
            fail_baseline: true,
            ..TextureTestHooks::default()
        }),
        harness.rig.pixel,
        |_| {},
    );
    let mut seen = Vec::new();
    harness.rig.wait("the failure", || {
        seen.extend(reports(&presenter));
        !failed_messages(&seen).is_empty()
    });
    harness.rig.idle(Duration::from_millis(50));
    seen.extend(reports(&presenter));
    let failed = failed_messages(&seen);
    assert_eq!(failed.len(), 1, "one Failed: {seen:?}");
    assert!(failed[0].contains("checker"), "{}", failed[0]);
    assert_eq!(presenter.scene_stats().frames, 0, "nothing was drawn");
    assert!(harness.builds().is_empty());
    harness.finish(&mut presenter);
}

/// A stop while the checker's upload is held ends proven once the upload finishes inside the budget.
fn a_stop_during_the_checker_upload_is_proven() {
    let mut harness = Harness::new();
    let (gate, reached) = closed_gate();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(
        Harness::limits(),
        Some(texture_hooks(&gate, &reached)),
        harness.rig.pixel,
        |_| {},
    );
    wait_flag(&mut harness, "the worker at the gate", &reached);
    presenter.request_stop();
    open_gate(&gate);
    let ended = presenter
        .poll_end(BOUND)
        .expect("the presenter's end was observed");
    assert_eq!(ended, CleanupResult::Proven);
    drop(presenter);
    assert_eq!(Arc::strong_count(&harness.context), 1);
    let Harness { context, rig, .. } = harness;
    drop(context);
    rig.close(&ended);
}

// ---------------------------------------------------------------------------------------------
// 5. Redraw on adoption
// ---------------------------------------------------------------------------------------------

/// With no new publication, an adopted revision appears in a later frame: the driver is asked for a pass once, offers the
/// image, commits what is prepared, and the committed revision wants a frame whose boundary adopts it.
fn an_adopted_revision_appears_without_a_publication() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start();
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    let generation = harness.generation;
    harness.offer(1, 4, [255, 0, 0, 255]);
    (presenter.texture_wake())();
    harness.rig.wait("a frame that resolves the key", || {
        harness
            .builds
            .lock()
            .expect("the builds")
            .iter()
            .any(|build| build.counters.resolved >= 1)
    });
    assert_eq!(harness.generation, generation, "nothing was published");
    assert_eq!(harness.state(|state| state.adopted.clone()), vec![(1, 1)]);
    let stats = presenter.scene_stats();
    assert!(
        stats.textures_ready >= 1 && stats.first_ready_ns != 0,
        "{stats:?}"
    );
    harness.finish(&mut presenter);
}

// ---------------------------------------------------------------------------------------------
// 6. The idle rule
// ---------------------------------------------------------------------------------------------

/// An upload that completes while no frame is wanted is observed, and its `Prepared` reaches the driver with no external
/// wake after the one that asked for the pass.
fn an_upload_completing_while_idle_reaches_the_driver_without_a_wake() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start();
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    harness.offer(5, 4, [0, 255, 0, 255]);
    (presenter.texture_wake())();
    harness.wait_for("the Prepared event at the driver", |harness| {
        harness.state(|state| {
            state
                .events
                .iter()
                .any(|event| matches!(event, LedgerEvent::Prepared { key: 5, .. }))
        })
    });
    harness.finish(&mut presenter);
}

/// No wait is entered between the passes of a capped sequence: the driver reports `more = true` for five passes, and the
/// wait count it sees is the same at each of them.
///
/// Negative control: an unconditional 1 ms wait whenever a fence is outstanding would enter a wait between passes; the
/// unit test of `idle_deadline` shows the real rule bounds only an idle thread.
fn a_capped_pass_enters_no_wait() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start();
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    // Settled: everything proven, the thread waiting without a bound.
    harness
        .rig
        .wait("everything proven", || harness.probes.proofs_in());
    let waits = Arc::clone(&harness.waits);
    harness.rig.wait("a wait without a bound", || {
        waits.entries().last() == Some(&WaitKind::Unbounded)
    });
    let before = harness.state(|state| state.waits_at_pass.len());
    harness.driver.lock().expect("the driver state").more = 5;
    (presenter.texture_wake())();
    harness.wait_for("the five capped passes", |harness| {
        harness.state(|state| state.waits_at_pass.len() >= before + 5)
    });
    let counts = harness.state(|state| state.waits_at_pass[before..before + 5].to_vec());
    assert!(
        counts.iter().all(|seen| *seen == counts[0]),
        "no wait was entered between the capped passes: {counts:?}"
    );
    harness.finish(&mut presenter);
}

/// A completion already signalled at the idle decision makes the thread loop, not wait: the worker holds an offered image at
/// the store's gate, the thread is held at its idle decision once the upload has been submitted, the device is idled from
/// outside (the upload completes), and after the release the driver sees `Prepared` in a pass that entered no wait.
///
/// Negative control: before the release the upload is submitted and not yet seen (the driver has not been told), so the
/// completion really is "already signalled but unobserved" when the decision is made.
fn a_completion_signalled_at_the_idle_decision_enters_no_wait() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let pause = Arc::new(Pause::new());
    let (gate, reached) = closed_gate();
    open_gate(&gate);
    let mut presenter = harness.start_with(
        Harness::limits(),
        Some(texture_hooks(&gate, &reached)),
        harness.rig.pixel,
        |hooks| {
            hooks.scene.pause_before_wait = Some(Arc::clone(&pause));
        },
    );
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    harness
        .rig
        .wait("everything proven", || harness.probes.proofs_in());
    // Close the gate again and hand the worker an image: it takes it and stops at the gate.
    {
        let (mutex, _) = &*gate;
        *mutex.lock().expect("the gate") = false;
    }
    reached.store(false, Ordering::SeqCst);
    harness.offer(7, 4, [0, 0, 255, 255]);
    (presenter.texture_wake())();
    wait_flag(&mut harness, "the worker at the gate", &reached);
    // The pass that offered key 7 is the one the driver recorded; the thread cannot enter a wait during a pass, so a wait
    // entered after that count is strictly newer than the pass. The last wait being unbounded then says the thread offered the
    // image, found nothing outstanding (the worker holds the batch at its gate) and is waiting **past** its pause point, so
    // the pause below can only be reached by a decision that follows the worker's release.
    harness.wait_for("the pass that offered key 7", |harness| {
        harness.state(|state| state.offered_at.iter().any(|(key, _)| *key == 7))
    });
    let entered_before = harness.state(|state| {
        state
            .offered_at
            .iter()
            .find(|(key, _)| *key == 7)
            .map(|(_, waited)| *waited)
            .expect("the offering pass was recorded")
    });
    let waits = Arc::clone(&harness.waits);
    harness
        .rig
        .wait("a wait without a bound after the offering pass", || {
            waits.waits() > entered_before && waits.entries().last() == Some(&WaitKind::Unbounded)
        });
    // Hold the thread at its next idle decision, then let the worker go: it closes the batch and wakes the thread, whose pass
    // submits it and arrives at the decision.
    pause.hold();
    open_gate(&gate);
    harness
        .rig
        .wait("the thread at its idle decision", || pause.reached());
    assert!(
        presenter.scene_stats().upload_batches_submitted >= 2,
        "the offered image's batch was submitted (the checker's was the first): {:?}",
        presenter.scene_stats()
    );
    assert!(
        harness.state(|state| !state
            .events
            .iter()
            .any(|event| matches!(event, LedgerEvent::Prepared { key: 7, .. }))),
        "its completion has not been seen"
    );
    let entered = harness.waits.waits();
    let passes = harness.state(|state| state.waits_at_pass.len());
    // SAFETY: the render thread is held at its pause point and uses no queue meanwhile; this is the only caller.
    unsafe { harness.context.device().device_wait_idle() }.expect("the device idles");
    pause.release();
    harness.wait_for("the Prepared event at the driver", |harness| {
        harness.state(|state| {
            state
                .events
                .iter()
                .any(|event| matches!(event, LedgerEvent::Prepared { key: 7, .. }))
        })
    });
    let first_after = harness.state(|state| state.waits_at_pass[passes]);
    assert_eq!(
        first_after, entered,
        "the completion was found by the last look at the decision, not waited for"
    );
    harness.finish(&mut presenter);
}

/// Only unsignalled GPU work left: a frame held by a `VkEvent`, with nothing else to do, enters bounded waits (the 1 ms
/// look) and never a wait without a bound; once the event is set the frame is proven and the thread settles into a wait
/// without a bound.
fn unsignalled_gpu_work_waits_with_a_bound() {
    let mut harness = Harness::new();
    let held = event(&harness);
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(Harness::limits(), None, harness.rig.pixel, |hooks| {
        hooks.scene.frame_event = Some(held);
    });
    harness.rig.wait("the held frame to be submitted", || {
        count(&harness.probes.events(), |e| *e == Event::Submit) >= 1
    });
    let submitted_at = harness
        .probes
        .observer
        .event_times()
        .into_iter()
        .zip(harness.probes.events())
        .find(|(_, event)| *event == Event::Submit)
        .map(|(time, _)| time)
        .expect("the submit");
    let waits = Arc::clone(&harness.waits);
    harness.rig.wait("bounded waits after the submit", || {
        waits
            .entries()
            .into_iter()
            .zip(waits.times())
            .filter(|(kind, time)| *time > submitted_at && *kind == WaitKind::GpuPoll)
            .count()
            >= 3
    });
    let after: Vec<WaitKind> = harness
        .waits
        .entries()
        .into_iter()
        .zip(harness.waits.times())
        .filter(|(_, time)| *time > submitted_at)
        .map(|(kind, _)| kind)
        .collect();
    assert!(
        after.iter().all(|kind| *kind == WaitKind::GpuPoll),
        "only bounded waits while the frame is unsignalled: {after:?}"
    );
    set_event(&harness, held);
    harness
        .rig
        .wait("everything proven", || harness.probes.proofs_in());
    let waits = Arc::clone(&harness.waits);
    harness.rig.wait("a wait without a bound", || {
        waits.entries().last() == Some(&WaitKind::Unbounded)
    });
    // Settled: no wait is entered while nothing happens.
    let entered = harness.waits.waits();
    harness.rig.idle(Duration::from_millis(100));
    assert_eq!(
        harness.waits.waits(),
        entered,
        "an unbounded wait is not re-entered"
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    destroy_event(&harness, held);
    drop(presenter);
    let Harness { context, rig, .. } = harness;
    drop(context);
    rig.close(&ended);
}

/// A wake posted between the last check and the wait is not lost: the thread is held between the unlocked look and the
/// locked recheck, a frame is published, and the release is followed by the frame, with no other wake.
fn a_wake_between_the_recheck_and_the_wait_is_not_lost() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let pause = Arc::new(Pause::new());
    let mut presenter = harness.start_with(Harness::limits(), None, harness.rig.pixel, |hooks| {
        hooks.scene.pause_before_wait = Some(Arc::clone(&pause));
    });
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    harness
        .rig
        .wait("everything proven", || harness.probes.proofs_in());
    pause.hold();
    // Poke the thread out of its current wait so that it reaches the pause point afresh.
    presenter.request_redraw();
    harness
        .rig
        .wait("a frame", || presenter.scene_stats().frames >= 2);
    harness
        .rig
        .wait("the thread at the pause point", || pause.reached());
    let frames = presenter.scene_stats().frames;
    harness.publish(vec![quad(1, 20, 20, 100)]);
    pause.release();
    harness.wait_stats(
        &presenter,
        "the frame published while the thread was held",
        |stats| stats.frames > frames,
    );
    harness.finish(&mut presenter);
}

/// A drawable of 0 x 0 parks the want: the thread waits and makes no attempt, and resumes once it is given a size.
fn a_zero_size_parks_the_want_until_a_size_arrives() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(Harness::limits(), None, (0, 0), |_| {});
    harness.wait_stats(&presenter, "the checker", |stats| stats.checker_resident);
    let waits = Arc::clone(&harness.waits);
    harness.rig.wait("a wait without a bound", || {
        waits.entries().last() == Some(&WaitKind::Unbounded)
    });
    harness.rig.idle(Duration::from_millis(100));
    assert_eq!(
        harness.waits.attempts(),
        0,
        "no frame was attempted at 0 x 0"
    );
    assert_eq!(presenter.scene_stats().frames, 0);
    let (width, height) = harness.rig.pixel;
    presenter.set_pixel_size(width, height);
    harness.wait_stats(&presenter, "the frame once it has a size", |stats| {
        stats.frames >= 1
    });
    harness.finish(&mut presenter);
}

/// With the present fences held the pools run out: the want is parked (no repeated attempt, a bounded wait for the
/// outstanding fences), and the frame runs once the present fences prove.
fn exhausted_present_capacity_parks_the_want_until_the_present_fences_prove() {
    let mut harness = Harness::new();
    let held = Arc::new(AtomicBool::new(true));
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(Harness::limits(), None, harness.rig.pixel, |hooks| {
        hooks.present_fences_held = Some(Arc::clone(&held));
    });
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    // Frames until the present slots are used up.
    let observer = Arc::clone(&harness.probes.observer);
    while observer.ledger().free_slots > 0 {
        let target = presenter.scene_stats().frames + 1;
        presenter.request_redraw();
        harness.wait_stats(&presenter, "another frame", |stats| stats.frames >= target);
        harness.rig.wait("the frame's proof", || {
            observer.ledger().pending_submits == 0
        });
    }
    harness.rig.wait("the thread to wait", || {
        harness.waits.entries().last() == Some(&WaitKind::GpuPoll)
    });
    presenter.request_redraw();
    harness.rig.idle(Duration::from_millis(50));
    let attempts = harness.waits.attempts();
    let frames = presenter.scene_stats().frames;
    harness.rig.idle(Duration::from_millis(100));
    assert_eq!(
        harness.waits.attempts(),
        attempts,
        "a want with no present capacity is not attempted again"
    );
    assert_eq!(presenter.scene_stats().frames, frames);
    assert!(
        harness
            .waits
            .entries()
            .iter()
            .rev()
            .take(5)
            .all(|kind| *kind == WaitKind::GpuPoll),
        "the outstanding present fences bound the wait"
    );
    held.store(false, Ordering::SeqCst);
    harness.wait_stats(&presenter, "the frame once the fences prove", |stats| {
        stats.frames > frames
    });
    harness.finish(&mut presenter);
}

/// A recoverable swap-chain creation failure with no outstanding work: the thread waits until `creation_retry_at`, makes no
/// attempt before it, and the retry succeeds and rendering resumes with no input from the case.
///
/// Negative control: the idle rule's unit test shows that a calculation without the retry deadline waits without a bound
/// here, which this case would observe as a missing retry.
fn a_parked_rebuild_waits_for_its_retry_deadline() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(Harness::limits(), None, harness.rig.pixel, |hooks| {
        // The first creation is real, the second (the rebuild) fails recoverably, the rest are real.
        hooks.swapchain = vec![None, Some(vk::Result::ERROR_INITIALIZATION_FAILED)];
    });
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    harness
        .rig
        .wait("everything proven", || harness.probes.proofs_in());
    let (width, height) = harness.rig.pixel;
    presenter.set_pixel_size(width + 8, height + 8);
    let observer = Arc::clone(&harness.probes.observer);
    harness.rig.wait("the failed creation", || {
        observer
            .events()
            .iter()
            .any(|event| matches!(event, Event::CreationFailed(_)))
    });
    // From here the case does nothing to the presenter: the retry is the thread's own.
    harness.rig.wait("the second swap chain", || {
        count(&observer.events(), |e| {
            matches!(e, Event::ChainCreated { .. })
        }) >= 2
    });
    harness.wait_stats(&presenter, "rendering to resume", |stats| stats.frames >= 2);
    let events = observer.events();
    let times = observer.event_times();
    let failed_at = events
        .iter()
        .zip(&times)
        .find(|(event, _)| matches!(event, Event::CreationFailed(_)))
        .map(|(_, time)| *time)
        .expect("the failed creation");
    let created_at = events
        .iter()
        .zip(&times)
        .filter(|(event, _)| matches!(event, Event::ChainCreated { .. }))
        .nth(1)
        .map(|(_, time)| *time)
        .expect("the retry");
    assert!(
        created_at.duration_since(failed_at) >= Duration::from_millis(50),
        "no creation attempt before the 50 ms backoff: {:?}",
        created_at.duration_since(failed_at)
    );
    let between: Vec<WaitKind> = harness
        .waits
        .entries()
        .into_iter()
        .zip(harness.waits.times())
        .filter(|(_, time)| *time > failed_at && *time < created_at)
        .map(|(kind, _)| kind)
        .collect();
    assert!(
        between.contains(&WaitKind::RetryDeadline),
        "a wait bounded by the retry deadline was entered: {between:?}"
    );
    assert!(
        !between.contains(&WaitKind::Unbounded),
        "no wait without a bound while a retry is pending: {between:?}"
    );
    harness.finish(&mut presenter);
}

// ---------------------------------------------------------------------------------------------
// 7. Frame proofs
// ---------------------------------------------------------------------------------------------

/// 600 scene frames with presents leave no acquire semaphore and no present slot stuck: the pools are whole again once the
/// proofs are in, and every ticket ended.
fn six_hundred_frames_leave_no_semaphore_or_slot_stuck() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start();
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    harness
        .rig
        .wait("everything proven", || harness.probes.proofs_in());
    let whole = harness.probes.observer.ledger();
    for _ in 0..600 {
        let target = presenter.scene_stats().frames + 1;
        presenter.request_redraw();
        harness.wait_stats(&presenter, "the next frame", |stats| stats.frames >= target);
    }
    let observer = Arc::clone(&harness.probes.observer);
    harness.rig.wait("every proof", || {
        let ledger = observer.ledger();
        harness.probes.proofs_in()
            && ledger.free_acquire == whole.free_acquire
            && ledger.free_slots == whole.free_slots
            && ledger.free_fences == whole.free_fences
    });
    assert!(
        presenter.scene_stats().frames >= 601,
        "{:?}",
        presenter.scene_stats()
    );
    let tickets = harness.tickets.lock().expect("the tickets").len();
    assert!(tickets >= 601, "{tickets} tickets");
    harness.finish(&mut presenter);
}

// ---------------------------------------------------------------------------------------------
// 9. The instance buffer
// ---------------------------------------------------------------------------------------------

/// `binding_geometry_bytes` below the buffer's size refuses the setup with one `Failed` that names both numbers; the end is
/// proven (nothing was submitted).
fn a_budget_below_the_instance_buffer_refuses_setup_naming_both() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(
        SceneLimits {
            max_quads: 1_000,
            binding_geometry_bytes: 1,
        },
        None,
        harness.rig.pixel,
        |_| {},
    );
    let mut seen = Vec::new();
    harness.rig.wait("the failure", || {
        seen.extend(reports(&presenter));
        !failed_messages(&seen).is_empty()
    });
    let failed = failed_messages(&seen);
    assert_eq!(failed.len(), 1, "{seen:?}");
    assert!(failed[0].contains("1000 quads"), "{}", failed[0]);
    assert!(failed[0].contains("over the 1 bytes"), "{}", failed[0]);
    assert!(
        failed[0].contains("binding_geometry_bytes"),
        "{}",
        failed[0]
    );
    let ended = presenter
        .poll_end(BOUND)
        .or_else(|| {
            presenter.request_stop();
            presenter.poll_end(BOUND)
        })
        .expect("the end");
    assert_eq!(ended, CleanupResult::Proven);
    drop(presenter);
    let Harness { context, rig, .. } = harness;
    drop(context);
    rig.close(&ended);
}

/// A frame is never written while the previous submission is unproven: a frame held by a `VkEvent`, then a publication; no
/// build (and so no write) happens until the event is set, and every build saw the previous submission proven. When the
/// event is set the pools are whole again once the proofs are in (the frame's acquire semaphore and present slot return
/// through the texture ledger's latched proof).
fn no_instance_is_written_while_the_previous_frame_is_unproven() {
    let mut harness = Harness::new();
    let held = event(&harness);
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(Harness::limits(), None, harness.rig.pixel, |hooks| {
        hooks.scene.frame_event = Some(held);
    });
    harness
        .rig
        .wait("the held frame to be built and submitted", || {
            count(&harness.probes.events(), |e| *e == Event::Submit) >= 1
        });
    assert_eq!(harness.builds().len(), 1);
    harness.publish(vec![quad(1, 30, 30, 100)]);
    presenter.request_redraw();
    harness.rig.idle(Duration::from_millis(150));
    assert_eq!(
        harness.builds().len(),
        1,
        "no instance was written while the previous frame was unproven"
    );
    set_event(&harness, held);
    harness.wait_for("the second build", |harness| harness.builds().len() >= 2);
    assert!(
        harness.builds().iter().all(|build| build.previous_proven),
        "every build saw the previous submission proven"
    );
    let observer = Arc::clone(&harness.probes.observer);
    harness.rig.wait("every proof", || {
        let ledger = observer.ledger();
        harness.probes.proofs_in() && ledger.free_acquire == ledger.free_slots
    });
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    destroy_event(&harness, held);
    drop(presenter);
    let Harness { context, rig, .. } = harness;
    drop(context);
    rig.close(&ended);
}

// ---------------------------------------------------------------------------------------------
// 10. Formats
// ---------------------------------------------------------------------------------------------

/// A non-UNORM swap chain clears, records one `QuadSkipped` notice, and draws no scene.
fn a_non_unorm_swap_chain_clears_notices_once_and_draws_no_scene() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(Harness::limits(), None, harness.rig.pixel, |hooks| {
        hooks.forced_surface_formats = Some(vec![vk::Format::B8G8R8A8_SRGB]);
    });
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    harness.rig.idle(Duration::from_millis(50));
    assert_eq!(
        presenter.take_quad_notice(),
        Some(QuadSkipped {
            format: vk::Format::B8G8R8A8_SRGB
        })
    );
    assert_eq!(
        presenter.take_quad_notice(),
        None,
        "the notice is given once"
    );
    assert!(harness.builds().is_empty(), "no scene was built");
    let events = harness.probes.events();
    assert_eq!(
        count(&events, |e| matches!(e, Event::QuadSkipped(_))),
        1,
        "{events:?}"
    );
    harness.finish(&mut presenter);
}

// ---------------------------------------------------------------------------------------------
// 11. The end
// ---------------------------------------------------------------------------------------------

/// A proven stop with a scene drawn and an offered image held by the store's worker: the worker has the image (it is at the
/// gate), the stop is requested, and the worker is released; the end is proven inside the budget.
fn a_stop_with_a_scene_drawn_and_an_offered_image_held_by_the_worker_is_proven() {
    let mut harness = Harness::new();
    let (gate, reached) = closed_gate();
    open_gate(&gate);
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(
        Harness::limits(),
        Some(texture_hooks(&gate, &reached)),
        harness.rig.pixel,
        |_| {},
    );
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    {
        let (mutex, _) = &*gate;
        *mutex.lock().expect("the gate") = false;
    }
    reached.store(false, Ordering::SeqCst);
    harness.offer(2, 1_024, [9, 9, 9, 255]);
    (presenter.texture_wake())();
    // The acknowledgement that the offer reached the ledger and the store: the worker took the image.
    wait_flag(
        &mut harness,
        "the worker holding the offered image",
        &reached,
    );
    presenter.request_stop();
    open_gate(&gate);
    harness.finish(&mut presenter);
}

/// Test 11: a proven stop with a scene drawn and an upload **genuinely outstanding**: a scene frame held by its `VkEvent` is
/// ahead of the offered image's upload batch in the queue, so the batch is submitted and not complete (the presenter's own
/// counts say `submitted > completed`) when the stop is requested; the event is then set, and the end must be proven inside
/// the deadline.
fn a_stop_with_an_upload_genuinely_outstanding_behind_a_held_frame_is_proven() {
    let mut harness = Harness::new();
    let held = event(&harness);
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(Harness::limits(), None, harness.rig.pixel, |hooks| {
        hooks.scene.frame_event = Some(held);
    });
    harness.rig.wait("the held frame to be submitted", || {
        count(&harness.probes.events(), |e| *e == Event::Submit) >= 1
    });
    harness.offer(2, 1_024, [9, 9, 9, 255]);
    (presenter.texture_wake())();
    harness.wait_stats(
        &presenter,
        "the offered image's batch to be submitted and not complete",
        |stats| {
            stats.upload_batches_submitted >= 2
                && stats.upload_batches_submitted > stats.upload_batches_completed
        },
    );
    presenter.request_stop();
    set_event(&harness, held);
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    destroy_event(&harness, held);
    drop(presenter);
    let Harness { context, rig, .. } = harness;
    drop(context);
    rig.close(&ended);
}

/// A proven stop right after an upload was submitted: the image is offered and the case waits for the presenter's own count
/// of submitted upload batches to include it (the checker's was the first) before it requests the stop. (The batch may be
/// complete by then; what is guaranteed is that it was submitted before the stop, which the count says.)
fn a_stop_after_an_upload_was_submitted_is_proven() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start();
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    harness.offer(2, 1_024, [9, 9, 9, 255]);
    (presenter.texture_wake())();
    harness.wait_stats(
        &presenter,
        "the offered image's batch to be submitted",
        |stats| stats.upload_batches_submitted >= 2,
    );
    presenter.request_stop();
    harness.finish(&mut presenter);
}

/// A frame held past the deadline gives `Unproven` with everything retained: the context, the store and ledger and the
/// swap chain are kept, nothing is destroyed, and validation said nothing.
fn a_held_frame_past_the_deadline_is_unproven_and_retains_everything() {
    let mut harness = Harness::new();
    let held = event(&harness);
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start_with(Harness::limits(), None, harness.rig.pixel, |hooks| {
        hooks.scene.frame_event = Some(held);
        hooks.stop_budget = Some(Duration::from_millis(300));
    });
    harness.rig.wait("the held frame to be submitted", || {
        count(&harness.probes.events(), |e| *e == Event::Submit) >= 1
    });
    let ended = Rig::end(&mut presenter);
    let CleanupResult::Unproven { reason } = &ended else {
        panic!("a frame the GPU never finished cannot be proven: {ended:?}");
    };
    assert!(
        reason.contains("stop deadline") && reason.contains("kept"),
        "{reason}"
    );
    check_nothing_destroyed(&harness.probes.events());
    // Let the GPU finish so the leaked objects are not left waiting on the host.
    set_event(&harness, held);
    harness.rig.retire();
}

/// The cases this module adds to the window worker.
pub(super) const CASES: [(&str, Case); 21] = [
    (
        "the_first_frame_shows_the_checker_for_a_pending_key",
        the_first_frame_shows_the_checker_for_a_pending_key,
    ),
    (
        "a_held_checker_parks_the_first_frame_and_its_release_alone_runs_it",
        a_held_checker_parks_the_first_frame_and_its_release_alone_runs_it,
    ),
    (
        "an_injected_checker_failure_is_one_failed_and_draws_nothing",
        an_injected_checker_failure_is_one_failed_and_draws_nothing,
    ),
    (
        "a_stop_during_the_checker_upload_is_proven",
        a_stop_during_the_checker_upload_is_proven,
    ),
    (
        "an_adopted_revision_appears_without_a_publication",
        an_adopted_revision_appears_without_a_publication,
    ),
    (
        "an_upload_completing_while_idle_reaches_the_driver_without_a_wake",
        an_upload_completing_while_idle_reaches_the_driver_without_a_wake,
    ),
    ("a_capped_pass_enters_no_wait", a_capped_pass_enters_no_wait),
    (
        "a_completion_signalled_at_the_idle_decision_enters_no_wait",
        a_completion_signalled_at_the_idle_decision_enters_no_wait,
    ),
    (
        "unsignalled_gpu_work_waits_with_a_bound",
        unsignalled_gpu_work_waits_with_a_bound,
    ),
    (
        "a_wake_between_the_recheck_and_the_wait_is_not_lost",
        a_wake_between_the_recheck_and_the_wait_is_not_lost,
    ),
    (
        "a_zero_size_parks_the_want_until_a_size_arrives",
        a_zero_size_parks_the_want_until_a_size_arrives,
    ),
    (
        "exhausted_present_capacity_parks_the_want_until_the_present_fences_prove",
        exhausted_present_capacity_parks_the_want_until_the_present_fences_prove,
    ),
    (
        "a_parked_rebuild_waits_for_its_retry_deadline",
        a_parked_rebuild_waits_for_its_retry_deadline,
    ),
    (
        "six_hundred_frames_leave_no_semaphore_or_slot_stuck",
        six_hundred_frames_leave_no_semaphore_or_slot_stuck,
    ),
    (
        "a_budget_below_the_instance_buffer_refuses_setup_naming_both",
        a_budget_below_the_instance_buffer_refuses_setup_naming_both,
    ),
    (
        "no_instance_is_written_while_the_previous_frame_is_unproven",
        no_instance_is_written_while_the_previous_frame_is_unproven,
    ),
    (
        "a_non_unorm_swap_chain_clears_notices_once_and_draws_no_scene",
        a_non_unorm_swap_chain_clears_notices_once_and_draws_no_scene,
    ),
    (
        "a_stop_with_a_scene_drawn_and_an_offered_image_held_by_the_worker_is_proven",
        a_stop_with_a_scene_drawn_and_an_offered_image_held_by_the_worker_is_proven,
    ),
    (
        "a_stop_with_an_upload_genuinely_outstanding_behind_a_held_frame_is_proven",
        a_stop_with_an_upload_genuinely_outstanding_behind_a_held_frame_is_proven,
    ),
    (
        "a_stop_after_an_upload_was_submitted_is_proven",
        a_stop_after_an_upload_was_submitted_is_proven,
    ),
    (
        "a_held_frame_past_the_deadline_is_unproven_and_retains_everything",
        a_held_frame_past_the_deadline_is_unproven_and_retains_everything,
    ),
];
