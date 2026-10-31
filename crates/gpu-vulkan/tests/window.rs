//! The Vulkan presenter, tested with real GLFW windows, as a supervisor and a worker from this one
//! executable (`harness = false`, so each case's result appears in plain `cargo test` output).
//!
//! GLFW's Cocoa window needs the process's main thread, so every case runs in a **worker**: this
//! executable started again with `--worker <case>`. The **supervisor** (the shared, bounded supervisor of
//! `apps/codekarl/tests/supervisor`, through `#[path]`) owns each worker from the instant it is spawned,
//! gives it 60 s, and on expiry kills and reaps it; every kill, reap and pipe read is bounded, a read error
//! is a failure that names the stream, and each reader's real end is observed within its grace. A worker
//! fails (nothing is skipped) without a desktop session, GLFW, the Vulkan loader or the validation layer.
//!
//! **The owner may be typing while this runs, Escape included.** Before its first window the worker
//! installs `ck-window-glfw`'s test-only filter (a local event monitor that drops every keyboard event
//! without the tests' mark), and every case runs twice, once on a quiet keyboard and once with a thread
//! typing without stopping. The filter and the typist are linked into this test executable only.
//!
//! **Validation is on in every case, and any validation message fails it.** The windows come from
//! `ck-window-glfw` (a dev-dependency); the library under test has no window dependency.

#![cfg(target_os = "macos")]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ck_gpu_vulkan::vk::Handle as _;
use ck_gpu_vulkan::{
    CleanupResult, Event, Gate, Instance, InstanceOptions, Loader, Observer, Pause,
    PresenterReport, ReadbackHooks, SurfaceExtent, TestHooks, ValidationLogHandle, VulkanPresenter,
    create_instance_with, destroy_surface_and_instance, load_loader,
    readback_clear_with_test_hooks, vk,
};
use ck_window::{Backend, Wait, WindowDesc};
use ck_window_glfw::{GlfwBackend, GlfwHandles, VulkanLoaderForGlfw};

#[path = "../../../apps/codekarl/tests/supervisor/mod.rs"]
#[expect(
    dead_code,
    reason = "the shared supervisor serves other tests; this one uses its spawn, finish and cleanup"
)]
mod supervisor;
mod support;
mod window_quad;
mod window_scene;

use supervisor::Supervised;

const WORKER_FLAG: &str = "--worker";
/// `1` makes a worker run with the owner typing without stopping.
const TYPING_VARIABLE: &str = "CK_VULKAN_TEST_TYPING";
/// `1` makes a worker send one warning through its instance's own messenger right after the instance is
/// created: every case must then fail, proving that validation reaches the case's final assertion.
const INJECT_VARIABLE: &str = "CK_VULKAN_TEST_INJECT_VALIDATION";
/// `1` makes the real-surface readback case send a warning right after the surface was destroyed: the
/// assertion placed there must then fail.
const INJECT_AFTER_DESTROY_VARIABLE: &str = "CK_VULKAN_TEST_INJECT_AFTER_DESTROY";
/// The longest a worker may run.
const DEADLINE: Duration = Duration::from_secs(60);
/// The longest any wait for something that must happen.
const BOUND: Duration = Duration::from_secs(10);
/// The window every case opens: 400 by 300 points.
const DESC_WIDTH: u32 = 400;
const DESC_HEIGHT: u32 = 300;

fn desc() -> WindowDesc {
    WindowDesc {
        title: "vulkan test".to_owned(),
        width: DESC_WIDTH,
        height: DESC_HEIGHT,
        clear: [0.1, 0.2, 0.3],
    }
}

// ---------------------------------------------------------------------------------------------
// The rig
// ---------------------------------------------------------------------------------------------

/// A GLFW window with a validated instance and a surface, torn down in the design's order.
struct Rig {
    backend: GlfwBackend,
    handles: GlfwHandles,
    instance: Instance,
    surface: vk::SurfaceKHR,
    loader: Loader,
    log: ValidationLogHandle,
    pixel: (u32, u32),
}

/// What the cases watch of a presenter.
struct Probes {
    observer: Arc<Observer>,
    frames: Arc<AtomicU64>,
    clear_log: Arc<Mutex<Vec<[f32; 3]>>>,
    wakes: Arc<AtomicU64>,
}

impl Probes {
    fn new() -> Self {
        Self {
            observer: Arc::new(Observer::new()),
            frames: Arc::new(AtomicU64::new(0)),
            clear_log: Arc::new(Mutex::new(Vec::new())),
            wakes: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Hooks that report into these probes.
    fn hooks(&self) -> TestHooks {
        TestHooks {
            observer: Some(Arc::clone(&self.observer)),
            frames: Some(Arc::clone(&self.frames)),
            clear_log: Some(Arc::clone(&self.clear_log)),
            ..TestHooks::default()
        }
    }

    fn frames(&self) -> u64 {
        self.frames.load(Ordering::SeqCst)
    }

    /// Every submit and every present that queued has had its fence proven (seen in the events), and the
    /// ledger holds nothing pending. (The ledger's snapshot alone can be stale: it reads zero before the first
    /// frame's operations are recorded in it.)
    fn proofs_in(&self) -> bool {
        let events = self.events();
        let submits = count(&events, |e| matches!(e, Event::Submit));
        let queued = count(&events, |e| {
            matches!(
                e,
                Event::Present { result, .. }
                    if !matches!(
                        *result,
                        vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY
                    )
            )
        });
        let submit_proofs = count(&events, |e| {
            matches!(
                e,
                Event::FenceSignalled {
                    proof: "submit",
                    ..
                }
            )
        });
        let present_proofs = count(&events, |e| {
            matches!(
                e,
                Event::FenceSignalled {
                    proof: "present",
                    ..
                }
            )
        });
        let ledger = self.observer.ledger();
        submits == submit_proofs
            && queued == present_proofs
            && ledger.pending_submits + ledger.pending_presents + ledger.pending_drains == 0
    }

    fn events(&self) -> Vec<Event> {
        self.observer.events()
    }
}

impl Rig {
    fn open() -> Self {
        Self::open_with(false)
    }

    /// [`Rig::open`] with synchronization validation when asked: the scene cases run with it.
    fn open_with(synchronization: bool) -> Self {
        let loader = load_loader().expect("the Vulkan loader loads by its absolute path");
        // SAFETY: `ck-gpu-vulkan`'s `GetInstanceProcAddr` and `ck-window-glfw`'s have the same C signature
        // (a pointer-sized handle, a C string, a function pointer back).
        let proc_addr = unsafe {
            std::mem::transmute::<
                ck_gpu_vulkan::GetInstanceProcAddr,
                ck_window_glfw::GetInstanceProcAddr,
            >(loader.get_instance_proc_addr())
        };
        // SAFETY: `proc_addr` is this loader's `vkGetInstanceProcAddr`, and the keep-alive is a clone of
        // that loader, which keeps the library loaded for as long as the value lives.
        let for_glfw = unsafe { VulkanLoaderForGlfw::new(proc_addr, Box::new(loader.clone())) };
        let mut backend = GlfwBackend::new(Some(for_glfw)).expect("GLFW starts on the main thread");
        let created = backend
            .create_window(&desc())
            .expect("the window is created");
        support::activate();
        let handles = created.handles;
        let instance = create_instance_with(
            &loader,
            handles.required_instance_extensions(),
            InstanceOptions {
                validation: true,
                synchronization,
            },
        )
        .expect("a validated instance");
        let raw = usize::try_from(instance.raw_handle()).expect("a pointer-sized handle");
        // SAFETY: the instance handle is live, and this runs on the main thread.
        let surface =
            unsafe { handles.create_surface(raw as *mut c_void) }.expect("the surface is created");
        let log = instance.validation_log();
        if env::var(INJECT_VARIABLE).as_deref() == Ok("1") {
            assert!(
                instance.submit_test_warning("an injected validation message"),
                "the instance has a messenger"
            );
        }
        let mut rig = Self {
            backend,
            handles,
            instance,
            surface: vk::SurfaceKHR::from_raw(surface),
            loader,
            log,
            pixel: (created.pixel_size.width, created.pixel_size.height),
        };
        let give_up = Instant::now() + BOUND;
        while !support::info().visible {
            assert!(
                Instant::now() < give_up,
                "the window never became visible: is there a desktop session? {:?}",
                support::info()
            );
            rig.pump();
            thread::sleep(Duration::from_millis(5));
        }
        rig
    }

    fn pump(&mut self) {
        self.backend.pump(&mut |_| {}, Wait::Poll, 100);
    }

    /// Pumps the window's events until `condition` holds, or fails after the bound.
    fn wait(&mut self, what: &str, mut condition: impl FnMut() -> bool) {
        let give_up = Instant::now() + BOUND;
        while !condition() {
            assert!(Instant::now() < give_up, "timed out waiting for {what}");
            self.pump();
            thread::sleep(Duration::from_millis(3));
        }
    }

    /// [`Rig::wait`] for a condition on the rig itself.
    fn wait_rig(&mut self, what: &str, condition: impl Fn(&Self) -> bool) {
        let give_up = Instant::now() + BOUND;
        while !condition(self) {
            assert!(Instant::now() < give_up, "timed out waiting for {what}");
            self.pump();
            thread::sleep(Duration::from_millis(3));
        }
    }

    /// Makes fences read as never signalled and draws one frame, so that its submit and present fences stay
    /// outstanding in the ledger.
    fn leave_fences_outstanding(
        &mut self,
        presenter: &VulkanPresenter,
        probes: &Probes,
        never: &AtomicBool,
        colour: [f32; 3],
    ) {
        // Everything earlier is proven first: a submit fence left unseen would hold the next frame back.
        self.wait("the earlier frames' fences to be proven", || {
            probes.proofs_in()
        });
        never.store(true, Ordering::SeqCst);
        let target = probes.frames() + 1;
        presenter.set_clear(colour);
        self.wait("a frame whose fences stay outstanding", || {
            probes.frames() >= target && probes.observer.ledger().pending_presents >= 1
        });
    }

    /// Pumps for `span` (the window stays serviced while a case checks that something does *not* happen).
    fn idle(&mut self, span: Duration) {
        let until = Instant::now() + span;
        while Instant::now() < until {
            self.pump();
            thread::sleep(Duration::from_millis(3));
        }
    }

    fn start(&self, probes: &Probes, hooks: TestHooks) -> VulkanPresenter {
        self.start_sized(probes, hooks, self.pixel)
    }

    fn start_sized(&self, probes: &Probes, hooks: TestHooks, size: (u32, u32)) -> VulkanPresenter {
        let wakes = Arc::clone(&probes.wakes);
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            wakes.fetch_add(1, Ordering::SeqCst);
        });
        // SAFETY: the surface was created on this instance for a window that outlives the presenter's
        // proven end (the rig destroys it after), and nothing else uses the surface meanwhile.
        unsafe {
            VulkanPresenter::start_with_test_hooks(
                &self.instance,
                self.surface,
                size,
                [0.1, 0.2, 0.3],
                wake,
                hooks,
            )
        }
        .expect("the presenter starts")
    }

    /// Stops the presenter and waits for its end.
    fn end(presenter: &mut VulkanPresenter) -> CleanupResult {
        presenter.request_stop();
        presenter
            .poll_end(BOUND)
            .expect("the presenter's end was observed")
    }

    /// Asserts that validation said nothing so far, whatever the case's end (an unproven case destroys
    /// nothing, so this is where it is checked).
    fn assert_clean(&self) {
        assert_eq!(
            self.log.messages(),
            Vec::<String>::new(),
            "validation reported something"
        );
    }

    /// The end of a case that retains its objects (an unproven end): validation is asserted, then the
    /// window, GLFW and the Vulkan objects are left to the process exit.
    fn retire(self) {
        self.assert_clean();
        std::mem::forget(self);
    }

    /// Destroys the surface and the instance (refused unless proven), the window and GLFW; asserts that
    /// validation said nothing in the whole life of the instance.
    fn close(self, ended: &CleanupResult) {
        let Self {
            mut backend,
            handles,
            instance,
            surface,
            loader,
            log,
            ..
        } = self;
        assert_eq!(
            log.messages(),
            Vec::<String>::new(),
            "validation reported something"
        );
        // SAFETY: the surface belongs to this instance, nothing uses it any more (the presenter ended
        // `Proven`), and the window is still alive.
        let destroyed = unsafe { destroy_surface_and_instance(instance, surface, Some(ended)) };
        if let Err(refused) = destroyed {
            panic!("the surface and instance were not destroyed: {refused}");
        }
        assert_eq!(
            log.messages(),
            Vec::<String>::new(),
            "validation reported something at destruction"
        );
        drop(handles);
        backend.destroy_window();
        backend.terminate();
        drop(backend);
        drop(loader);
    }
}

fn reports(presenter: &VulkanPresenter) -> Vec<PresenterReport> {
    std::iter::from_fn(|| presenter.take_report()).collect()
}

fn failed_messages(list: &[PresenterReport]) -> Vec<&str> {
    list.iter()
        .filter_map(|report| match report {
            PresenterReport::Failed { message } => Some(message.as_str()),
            PresenterReport::FirstFrameQueued => None,
        })
        .collect()
}

/// A different colour for each `n`.
#[expect(
    clippy::cast_precision_loss,
    reason = "n is a few dozen: the conversion is exact"
)]
fn colour(n: u64) -> [f32; 3] {
    [
        (n % 7) as f32 / 7.0,
        (n % 5) as f32 / 5.0,
        (n % 3) as f32 / 3.0,
    ]
}

// ---------------------------------------------------------------------------------------------
// What the observer's events must show
// ---------------------------------------------------------------------------------------------

/// The destroy order of design §7.
const DESTROY_ORDER: [&str; 6] = [
    "command buffers and pool",
    "fences and semaphores",
    "image views and framebuffers",
    "render pass",
    "swap chains",
    "device",
];

/// Checks, for a proven end: every swap chain is destroyed only after all its present fences signalled and
/// every present that queued had one; at most one swap chain is retired at a time; the teardown destroys in
/// design §7 order and only after the last operation was proven.
fn check_ledger_order(events: &[Event]) {
    check_ledger_order_with(events, &DESTROY_ORDER);
}

/// [`check_ledger_order`] against another teardown order (the scene presenter's device belongs to its context, so its
/// teardown has no `device` class).
fn check_ledger_order_with(events: &[Event], order: &[&str]) {
    let mut retired: BTreeSet<u64> = BTreeSet::new();
    let mut presents: BTreeMap<u64, usize> = BTreeMap::new();
    let mut signalled: BTreeMap<u64, usize> = BTreeMap::new();
    let mut destroyed: BTreeSet<u64> = BTreeSet::new();
    for event in events {
        match event {
            Event::ChainRetired(id) => {
                retired.insert(*id);
                assert!(
                    retired.len() <= 1,
                    "two swap chains retired at once: {events:?}"
                );
            }
            Event::ChainDestroyed(id) => {
                retired.remove(id);
                destroyed.insert(*id);
                assert_eq!(
                    presents.get(id).copied().unwrap_or(0),
                    signalled.get(id).copied().unwrap_or(0),
                    "swap chain {id} destroyed before all its present fences signalled: {events:?}"
                );
            }
            Event::Present { chain, result } => {
                if !matches!(
                    *result,
                    vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY
                ) {
                    *presents.entry(*chain).or_default() += 1;
                }
            }
            Event::FenceSignalled {
                proof: "present",
                chain: Some(chain),
            } => {
                *signalled.entry(*chain).or_default() += 1;
                assert!(
                    !destroyed.contains(chain),
                    "a fence of destroyed swap chain {chain} signalled"
                );
            }
            _ => {}
        }
    }
    let teardown: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::Destroyed(class) => Some(*class),
            _ => None,
        })
        .filter(|class| *class != "quad objects")
        .collect();
    assert_eq!(teardown, order, "the teardown order: {events:?}");
    let first_destroy = events
        .iter()
        .position(|event| matches!(event, Event::Destroyed(_)))
        .expect("a teardown");
    assert!(
        events[first_destroy..].iter().all(|event| !matches!(
            event,
            Event::Acquire { .. }
                | Event::Submit
                | Event::Present { .. }
                | Event::Drain { .. }
                | Event::FenceSignalled { .. }
        )),
        "an operation or proof after the destruction began: {events:?}"
    );
    for (chain, count) in &presents {
        assert_eq!(
            Some(count),
            signalled.get(chain),
            "swap chain {chain}: every queued present's fence was proven: {events:?}"
        );
    }
}

/// Nothing was destroyed (an unproven end): no teardown event of any class, and no swap chain, image view
/// or framebuffer destroy, which includes a retired swap chain.
fn check_nothing_destroyed(events: &[Event]) {
    assert!(
        !events.iter().any(|event| matches!(
            event,
            Event::Destroyed(_) | Event::ChainDestroyed(_) | Event::ChainChildrenDestroyed(_)
        )),
        "an unproven end destroyed something: {events:?}"
    );
}

fn count(events: &[Event], wanted: impl Fn(&Event) -> bool) -> usize {
    events.iter().filter(|event| wanted(event)).count()
}

// ---------------------------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------------------------

/// 3. `FirstFrameQueued` once, and `wake` called; the device is a portability one and the instance says so
///    (`VK_KHR_portability_subset` enabled), under validation.
fn first_frame_and_wake() {
    let mut rig = Rig::open();
    assert!(
        rig.instance.is_portability(),
        "MoltenVK is a portability implementation"
    );
    let probes = Probes::new();
    let mut presenter = rig.start(&probes, probes.hooks());
    rig.wait("the first frame", || probes.frames() >= 1);
    let first = reports(&presenter);
    assert_eq!(
        first,
        vec![PresenterReport::FirstFrameQueued],
        "one first-frame report"
    );
    assert!(
        probes.wakes.load(Ordering::SeqCst) >= 1,
        "the wake was called"
    );
    // More frames never repeat it.
    presenter.set_clear([0.9, 0.2, 0.2]);
    rig.wait("a second frame", || probes.frames() >= 2);
    rig.idle(Duration::from_millis(200));
    assert_eq!(reports(&presenter), vec![], "no second first-frame report");
    let events = probes.events();
    let order: Vec<&Event> = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                Event::Acquire { .. } | Event::Submit | Event::Present { .. }
            )
        })
        .take(3)
        .collect();
    assert!(
        matches!(
            order.as_slice(),
            [
                Event::Acquire { .. },
                Event::Submit,
                Event::Present {
                    result: vk::Result::SUCCESS,
                    ..
                }
            ]
        ),
        "acquire, submit, present: {events:?}"
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    check_ledger_order(&probes.events());
    rig.close(&ended);
}

/// 4. Redraw on demand: one colour change gives one frame, a burst gives one frame with the last colour,
///    no change gives none.
fn redraw_on_demand() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let gate = Arc::new(Gate::new());
    let mut hooks = probes.hooks();
    hooks.gate = Some(Arc::clone(&gate));
    let mut presenter = rig.start(&probes, hooks);
    gate.open(1);
    rig.wait("the first frame", || probes.frames() == 1);
    // No change: no frame, for a good while.
    presenter.set_clear([0.1, 0.2, 0.3]);
    presenter.set_pixel_size(rig.pixel.0, rig.pixel.1);
    gate.open(5);
    rig.idle(Duration::from_millis(300));
    assert_eq!(
        probes.frames(),
        1,
        "setting the same colour and size draws nothing"
    );
    // One change: one frame.
    presenter.set_clear([0.7, 0.1, 0.1]);
    rig.wait("the frame for one change", || probes.frames() == 2);
    rig.idle(Duration::from_millis(200));
    assert_eq!(probes.frames(), 2, "one colour change gave one frame");
    // A burst while the gate is closed: one frame, with the last colour.
    let held = probes.frames();
    // Drain the remaining permits first so the burst below really waits at the gate.
    while gate.take_for_test() {}
    for n in 0..50 {
        presenter.set_clear(colour(n + 1));
    }
    let last = colour(50);
    presenter.set_clear(last);
    rig.idle(Duration::from_millis(200));
    assert_eq!(probes.frames(), held, "the burst waits at the gate");
    gate.open(1);
    rig.wait("the burst's frame", || probes.frames() == held + 1);
    rig.idle(Duration::from_millis(200));
    assert_eq!(
        probes.frames(),
        held + 1,
        "a burst of 51 changes gave one frame"
    );
    assert_eq!(
        probes.clear_log.lock().expect("the log").last().copied(),
        Some(last),
        "the last colour wins"
    );
    // An explicit redraw gives one more.
    gate.open(5);
    presenter.request_redraw();
    rig.wait("the redraw", || probes.frames() == held + 2);
    rig.idle(Duration::from_millis(200));
    assert_eq!(probes.frames(), held + 2);
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    check_ledger_order(&probes.events());
    rig.close(&ended);
}

/// 5. Present lifetime under validation: many presents, several swap chain rebuilds forced through the
///    seam, and a pixel-size change while a swap chain is retired (coalesced; one rebuild after the retired
///    one is destroyed); then a teardown.
fn present_lifetime() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let never = Arc::new(AtomicBool::new(false));
    let mut hooks = probes.hooks();
    hooks.fences_never_signal = Some(Arc::clone(&never));
    hooks.present = vec![
        None,
        None,
        Some(vk::Result::ERROR_OUT_OF_DATE_KHR),
        None,
        Some(vk::Result::SUBOPTIMAL_KHR),
    ];
    hooks.acquire = vec![
        None,
        None,
        None,
        None,
        None,
        Some(vk::Result::ERROR_OUT_OF_DATE_KHR),
    ];
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    for n in 1..=30_u64 {
        presenter.set_clear(colour(n));
        let target = probes.frames() + 1;
        rig.wait("a frame", || probes.frames() >= target);
    }
    assert!(
        probes.observer.chains_created() >= 3,
        "the initial swap chain and the rebuilds forced by an out-of-date present and acquire: {:?}",
        probes.events()
    );
    // A pixel-size change while a swap chain is retired. Fences read as never signalled, so the frame
    // below leaves a present fence outstanding, and the resize retires the current swap chain with it.
    rig.leave_fences_outstanding(&presenter, &probes, &never, [0.5, 0.5, 0.5]);
    let before = probes.observer.chains_created();
    let (width, height) = rig.pixel;
    presenter.set_pixel_size(width / 2, height / 2);
    rig.wait("the rebuild that retires the current swap chain", || {
        probes.observer.chains_created() == before + 1
    });
    rig.wait("a retired swap chain", || {
        probes.observer.ledger().has_retired
    });
    presenter.set_pixel_size(width / 2 + 10, height / 2 + 10);
    presenter.set_pixel_size(width / 2 + 20, height / 2 + 20);
    rig.idle(Duration::from_millis(300));
    assert_eq!(
        probes.observer.chains_created(),
        before + 1,
        "while one swap chain is retired, size changes only coalesce: {:?}",
        probes.events()
    );
    never.store(false, Ordering::SeqCst);
    rig.wait(
        "the one rebuild after the retired swap chain is destroyed",
        || probes.observer.chains_created() == before + 2,
    );
    rig.wait("a frame at the final size", || {
        probes.events().iter().any(|event| matches!(event, Event::ChainCreated { size, .. } if *size == (width / 2 + 20, height / 2 + 20)))
    });
    let sizes: Vec<(u32, u32)> = probes
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::ChainCreated { size, .. } => Some(size),
            _ => None,
        })
        .collect();
    assert!(
        !sizes.contains(&(width / 2 + 10, height / 2 + 10)),
        "the middle size was coalesced away: {sizes:?}"
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    check_ledger_order(&probes.events());
    rig.close(&ended);
}

/// What one transition case injects and expects.
struct Transition {
    name: &'static str,
    hooks: fn(&mut TestHooks),
    /// How many `Failed` reports.
    failures: usize,
    /// The events that must have happened.
    expect: fn(&[Event]),
}

fn fatal_expect(events: &[Event]) {
    assert!(
        events.iter().any(|event| matches!(event, Event::Failed(_))),
        "{events:?}"
    );
}

/// 6. Every ledger transition of §2.5 injected through the seam: the ledger's state after each, and that
///    nothing is destroyed before its proving fence.
#[expect(
    clippy::too_many_lines,
    reason = "a table: one entry for each transition of the ledger, with what it must show"
)]
fn transitions() -> Vec<Transition> {
    vec![
        Transition {
            name: "acquire out of date",
            hooks: |h| h.acquire = vec![Some(vk::Result::ERROR_OUT_OF_DATE_KHR)],
            failures: 0,
            expect: |events| {
                assert_eq!(
                    count(events, |e| matches!(
                        e,
                        Event::AcquireError(vk::Result::ERROR_OUT_OF_DATE_KHR)
                    )),
                    1
                );
                assert!(
                    count(events, |e| matches!(e, Event::ChainRetired(_))) >= 1,
                    "{events:?}"
                );
                assert!(
                    count(events, |e| matches!(e, Event::ChainCreated { .. })) >= 2,
                    "{events:?}"
                );
            },
        },
        Transition {
            name: "acquire suboptimal",
            hooks: |h| h.acquire = vec![Some(vk::Result::SUBOPTIMAL_KHR)],
            failures: 0,
            expect: |events| {
                assert!(
                    matches!(
                        events
                            .iter()
                            .find(|e| matches!(e, Event::Acquire { .. } | Event::AcquireError(_))),
                        Some(Event::Acquire { .. })
                    ),
                    "a suboptimal acquire is a success: {events:?}"
                );
                assert_eq!(
                    count(events, |e| matches!(e, Event::ChainRetired(_))),
                    0,
                    "the surface's configuration is the swap chain's: no rebuild: {events:?}"
                );
            },
        },
        Transition {
            name: "acquire timeout and not ready",
            hooks: |h| h.acquire = vec![Some(vk::Result::TIMEOUT), Some(vk::Result::NOT_READY)],
            failures: 0,
            expect: |events| {
                assert_eq!(
                    count(events, |e| matches!(
                        e,
                        Event::AcquireError(vk::Result::TIMEOUT | vk::Result::NOT_READY)
                    )),
                    2
                );
                assert_eq!(
                    count(events, |e| matches!(e, Event::ChainRetired(_))),
                    0,
                    "no rebuild: {events:?}"
                );
            },
        },
        Transition {
            name: "acquire surface lost",
            hooks: |h| h.acquire = vec![Some(vk::Result::ERROR_SURFACE_LOST_KHR)],
            failures: 1,
            expect: fatal_expect,
        },
        Transition {
            name: "acquire out of memory",
            hooks: |h| h.acquire = vec![Some(vk::Result::ERROR_OUT_OF_HOST_MEMORY)],
            failures: 1,
            expect: fatal_expect,
        },
        Transition {
            name: "frame abandoned after acquire",
            // More frames than the swap chain has images: an image that was not released would never come
            // back, and the acquire would time out for good.
            hooks: |h| h.abandon_frames = 4,
            failures: 0,
            expect: |events| {
                assert_eq!(
                    count(events, |e| matches!(e, Event::ReleaseImages)),
                    4,
                    "{events:?}"
                );
                assert_eq!(
                    count(events, |e| matches!(e, Event::Drain { what: "acquire" })),
                    4,
                    "{events:?}"
                );
                assert_eq!(
                    count(events, |e| matches!(
                        e,
                        Event::FenceSignalled {
                            proof: "acquire-drain",
                            ..
                        }
                    )),
                    4,
                    "{events:?}"
                );
            },
        },
        Transition {
            name: "submit out of memory abandons the frame",
            hooks: |h| h.submit = vec![Some(vk::Result::ERROR_OUT_OF_HOST_MEMORY)],
            failures: 1,
            expect: |events| {
                assert_eq!(
                    count(events, |e| matches!(e, Event::ReleaseImages)),
                    1,
                    "{events:?}"
                );
                assert_eq!(
                    count(events, |e| matches!(e, Event::Drain { what: "acquire" })),
                    1,
                    "{events:?}"
                );
            },
        },
        Transition {
            name: "present suboptimal",
            hooks: |h| h.present = vec![Some(vk::Result::SUBOPTIMAL_KHR)],
            failures: 0,
            expect: |events| {
                assert_eq!(
                    count(events, |e| matches!(
                        e,
                        Event::Present {
                            result: vk::Result::SUBOPTIMAL_KHR,
                            ..
                        }
                    )),
                    1
                );
                assert_eq!(
                    count(events, |e| matches!(e, Event::ChainRetired(_))),
                    0,
                    "the surface's configuration is the swap chain's: no rebuild: {events:?}"
                );
            },
        },
        Transition {
            name: "present out of date",
            hooks: |h| h.present = vec![Some(vk::Result::ERROR_OUT_OF_DATE_KHR)],
            failures: 0,
            expect: |events| {
                assert_eq!(
                    count(events, |e| matches!(
                        e,
                        Event::Present {
                            result: vk::Result::ERROR_OUT_OF_DATE_KHR,
                            ..
                        }
                    )),
                    1
                );
                assert!(
                    count(events, |e| matches!(e, Event::ChainRetired(_))) >= 1,
                    "{events:?}"
                );
            },
        },
        Transition {
            name: "present surface lost",
            hooks: |h| h.present = vec![Some(vk::Result::ERROR_SURFACE_LOST_KHR)],
            failures: 1,
            expect: |events| {
                assert_eq!(
                    count(events, |e| matches!(
                        e,
                        Event::Present {
                            result: vk::Result::ERROR_SURFACE_LOST_KHR,
                            ..
                        }
                    )),
                    1
                );
                assert_eq!(
                    count(events, |e| matches!(
                        e,
                        Event::FenceSignalled {
                            proof: "present",
                            ..
                        }
                    )),
                    1,
                    "its queued present fence was proven: {events:?}"
                );
            },
        },
        Transition {
            name: "present out of host memory",
            hooks: |h| h.present = vec![Some(vk::Result::ERROR_OUT_OF_HOST_MEMORY)],
            failures: 1,
            expect: present_oom,
        },
        Transition {
            name: "present out of device memory",
            hooks: |h| h.present = vec![Some(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)],
            failures: 1,
            expect: present_oom,
        },
        Transition {
            name: "swap chain creation fails once",
            hooks: |h| h.swapchain = vec![None, Some(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)],
            failures: 0,
            expect: |events| {
                // The first creation is real; a later rebuild (forced below) fails once and is retried.
                assert_eq!(
                    count(events, |e| matches!(e, Event::CreationFailed(_))),
                    1,
                    "{events:?}"
                );
                assert!(
                    count(events, |e| matches!(e, Event::ChainCreated { .. })) >= 2,
                    "{events:?}"
                );
            },
        },
    ]
}

/// Runs one injected transition in a rig of its own and checks what it must show.
fn run_transition(transition: &Transition) {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut hooks = probes.hooks();
    (transition.hooks)(&mut hooks);
    let forced_rebuild = transition.name.starts_with("swap chain creation");
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame or the failure", || {
        probes.frames() >= 1
            || probes
                .events()
                .iter()
                .any(|e| matches!(e, Event::Failed(_)))
    });
    if forced_rebuild {
        // A real size change makes the failing creation happen.
        let (width, height) = rig.pixel;
        presenter.set_pixel_size(width - 20, height - 20);
        rig.wait("the rebuild after the failed creation", || {
            probes.events().iter().any(|e| matches!(e, Event::ChainCreated { size, .. } if *size == (width - 20, height - 20)))
        });
    } else if transition.failures == 0 {
        // Give recoveries time to finish: another colour, and a frame drawn after the transition.
        presenter.set_clear([0.8, 0.8, 0.1]);
        let target = probes.frames() + 1;
        rig.wait("a frame after the transition", || probes.frames() >= target);
    }
    rig.idle(Duration::from_millis(250));
    let list = reports(&presenter);
    assert_eq!(
        failed_messages(&list).len(),
        transition.failures,
        "{}: failure reports {list:?}",
        transition.name
    );
    if transition.failures > 0 {
        // After a failure nothing more is drawn.
        let drawn = probes.frames();
        presenter.set_clear([0.0, 1.0, 0.0]);
        rig.idle(Duration::from_millis(250));
        assert_eq!(
            probes.frames(),
            drawn,
            "{}: drew after Failed",
            transition.name
        );
    }
    (transition.expect)(&probes.events());
    // A frame counts, and the first one is reported, only when its present call queued it (success or
    // suboptimal); an out-of-date present is neither.
    let queued = count(&probes.events(), |e| {
        matches!(
            e,
            Event::Present {
                result: vk::Result::SUCCESS | vk::Result::SUBOPTIMAL_KHR,
                ..
            }
        )
    });
    assert_eq!(
        usize::try_from(probes.frames()).expect("a small count"),
        queued,
        "{}: frames are presents that queued",
        transition.name
    );
    assert_eq!(
        list.iter()
            .filter(|report| **report == PresenterReport::FirstFrameQueued)
            .count(),
        usize::from(queued > 0),
        "{}: the first frame is reported once, after a present that queued: {list:?}",
        transition.name
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(
        ended,
        CleanupResult::Proven,
        "{}: {ended:?}",
        transition.name
    );
    check_ledger_order(&probes.events());
    let snapshot = probes.observer.ledger();
    assert!(
        snapshot.pending_submits + snapshot.pending_presents + snapshot.pending_drains == 0
            && !snapshot.image_acquired,
        "{}: the ledger ended clean: {snapshot:?}",
        transition.name
    );
    rig.close(&ended);
}

/// 6. Every ledger transition of §2.5 injected through the seam: the ledger's state after each, and that
///    nothing is destroyed before its proving fence.
fn ledger_transitions() {
    for transition in transitions() {
        run_transition(&transition);
    }
}

/// A present that failed with out-of-memory queued nothing: no fence is waited on for it, and its
/// render-finished semaphore is drained by a waiting submit.
fn present_oom(events: &[Event]) {
    let oom = events
        .iter()
        .position(|e| {
            matches!(
                e,
                Event::Present {
                    result: vk::Result::ERROR_OUT_OF_HOST_MEMORY
                        | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY,
                    ..
                }
            )
        })
        .expect("the injected present");
    assert!(
        matches!(
            events.get(oom + 1),
            Some(Event::Drain {
                what: "render-finished"
            })
        ),
        "the render-finished semaphore is drained right after: {events:?}"
    );
    assert_eq!(
        count(events, |e| matches!(
            e,
            Event::FenceSignalled {
                proof: "render-drain",
                ..
            }
        )),
        1,
        "{events:?}"
    );
    // The failed present's own present fence was never submitted, so it is never waited on: every present
    // fence proof belongs to a present that queued.
    let queued = count(
        events,
        |e| matches!(e, Event::Present { result, .. } if !matches!(*result, vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)),
    );
    assert_eq!(
        count(events, |e| matches!(
            e,
            Event::FenceSignalled {
                proof: "present",
                ..
            }
        )),
        queued,
        "{events:?}"
    );
}

/// 5b. The pools are fixed: with the presentation engine seeming never to finish (present fences held), the
///     present slots run out, the next frame is skipped with the redraw kept pending, and a fence that
///     signals frees an entry and the pending redraw is drawn without a new request.
fn an_empty_pool_skips_the_frame_and_keeps_the_redraw_pending() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let held = Arc::new(AtomicBool::new(true));
    let mut hooks = probes.hooks();
    hooks.present_fences_held = Some(Arc::clone(&held));
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    // Frames go on until the slots are all outstanding.
    for n in 1..=40_u64 {
        if probes.observer.ledger().free_slots == 0 {
            break;
        }
        presenter.set_clear(colour(n));
        let target = probes.frames() + 1;
        rig.wait("a frame while slots are free", || probes.frames() >= target);
        rig.wait("its submit fence to be proven", || {
            probes.observer.ledger().pending_submits == 0
        });
    }
    let full = probes.observer.ledger();
    assert_eq!(
        full.free_slots, 0,
        "every present slot is outstanding: {full:?}"
    );
    assert!(full.pending_presents >= 2, "{full:?}");
    let drawn = probes.frames();
    presenter.set_clear([0.05, 0.95, 0.5]);
    rig.idle(Duration::from_millis(300));
    assert_eq!(probes.frames(), drawn, "an empty pool skips the frame");
    assert!(
        probes.observer.chains_created() == 1,
        "and rebuilds nothing"
    );
    held.store(false, Ordering::SeqCst);
    rig.wait("the pending redraw, with no new request", || {
        probes.frames() > drawn
    });
    assert_eq!(
        probes.clear_log.lock().expect("the log").last().copied(),
        Some([0.05, 0.95, 0.5]),
        "the redraw that stayed pending drew the colour that was waiting"
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven, "{ended:?}");
    check_ledger_order(&probes.events());
    rig.close(&ended);
}

/// 5c. A suboptimal result at the configuration the swap chain already has does not rebuild (the frame
///     still counts as presented), however often it comes.
fn a_suboptimal_result_at_the_current_configuration_does_not_rebuild() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut hooks = probes.hooks();
    hooks.present = vec![
        Some(vk::Result::SUBOPTIMAL_KHR),
        Some(vk::Result::SUBOPTIMAL_KHR),
        Some(vk::Result::SUBOPTIMAL_KHR),
    ];
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    for n in 1..=4_u64 {
        presenter.set_clear(colour(n));
        let target = probes.frames() + 1;
        rig.wait("a frame", || probes.frames() >= target);
    }
    let events = probes.events();
    assert_eq!(
        count(&events, |e| matches!(
            e,
            Event::Present {
                result: vk::Result::SUBOPTIMAL_KHR,
                ..
            }
        )),
        3,
        "{events:?}"
    );
    assert_eq!(
        count(&events, |e| matches!(e, Event::ChainRetired(_))),
        0,
        "three suboptimal results at the swap chain's own configuration rebuild nothing: {events:?}"
    );
    assert_eq!(
        probes.frames(),
        5,
        "every one of them was a presented frame"
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven, "{ended:?}");
    check_ledger_order(&probes.events());
    rig.close(&ended);
}

/// 5d. After the surface's configuration changed, a suboptimal result rebuilds once, and the new swap chain
///     has the new configuration, so the next suboptimal result does not.
fn a_suboptimal_result_after_the_surface_changed_rebuilds_once() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let extent: SurfaceExtent = Arc::new(Mutex::new(None));
    let mut hooks = probes.hooks();
    hooks.surface_extent = Some(Arc::clone(&extent));
    hooks.present = vec![
        None,
        Some(vk::Result::SUBOPTIMAL_KHR),
        None,
        Some(vk::Result::SUBOPTIMAL_KHR),
    ];
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    // The surface now reports a different extent (a new configuration), and the next present says
    // suboptimal.
    let (width, height) = rig.pixel;
    *extent.lock().expect("the extent") = Some((width - 20, height - 20));
    presenter.set_clear(colour(1));
    rig.wait("the rebuild", || probes.observer.chains_created() == 2);
    rig.wait("the frame after the rebuild", || probes.frames() >= 3);
    // At the new configuration a suboptimal result rebuilds nothing.
    presenter.set_clear(colour(2));
    rig.wait("a frame with a suboptimal result", || probes.frames() >= 4);
    rig.idle(Duration::from_millis(200));
    let events = probes.events();
    assert_eq!(
        count(&events, |e| matches!(e, Event::ChainRetired(_))),
        1,
        "one rebuild, for the change: {events:?}"
    );
    assert_eq!(probes.observer.chains_created(), 2, "{events:?}");
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven, "{ended:?}");
    check_ledger_order(&probes.events());
    rig.close(&ended);
}

/// 5e. The comparison uses the same normalised, limit-clamped extent the swap chain is created with: a
///     surface whose `currentExtent` is beyond the limits makes a swap chain at the limit, and a suboptimal
///     result at that configuration does not rebuild.
fn a_suboptimal_result_compares_the_clamped_extent() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let extent: SurfaceExtent = Arc::new(Mutex::new(None));
    let mut hooks = probes.hooks();
    hooks.surface_extent = Some(Arc::clone(&extent));
    hooks.present = vec![
        None,
        Some(vk::Result::SUBOPTIMAL_KHR),
        None,
        Some(vk::Result::SUBOPTIMAL_KHR),
    ];
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    // A width far beyond any limit: the swap chain is made at the maximum width.
    *extent.lock().expect("the extent") = Some((1_000_000, 600));
    presenter.set_clear(colour(1));
    rig.wait("the rebuild", || probes.observer.chains_created() == 2);
    rig.wait("the frame after the rebuild", || probes.frames() >= 3);
    presenter.set_clear(colour(2));
    rig.wait("a frame with a suboptimal result", || probes.frames() >= 4);
    rig.idle(Duration::from_millis(200));
    let events = probes.events();
    assert_eq!(
        count(&events, |e| matches!(e, Event::ChainRetired(_))),
        1,
        "the second suboptimal result is at the clamped configuration the swap chain has: {events:?}"
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven, "{ended:?}");
    check_ledger_order(&probes.events());
    rig.close(&ended);
}

/// 6b. Failed swap chain creations are retried with a backoff (50 ms, doubling), never fail the presenter by
///     count, and a later success resumes rendering.
fn failed_creations_are_retried_with_a_backoff_and_never_fail_by_count() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut hooks = probes.hooks();
    hooks.swapchain = vec![
        None,
        Some(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY),
        Some(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY),
        Some(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY),
        Some(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY),
        None,
    ];
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    rig.wait("the first frame's fences to be proven", || {
        probes.proofs_in()
    });
    let (width, height) = rig.pixel;
    presenter.set_pixel_size(width - 20, height - 20);
    rig.wait("the creation that finally succeeds", || {
        probes.observer.chains_created() == 2
    });
    let events = probes.events();
    let times = probes.observer.event_times();
    let failures: Vec<Instant> = events
        .iter()
        .zip(&times)
        .filter(|(event, _)| matches!(event, Event::CreationFailed(_)))
        .map(|(_, time)| *time)
        .collect();
    assert_eq!(failures.len(), 4, "{events:?}");
    for (index, wait) in [47_u64, 95, 190].into_iter().enumerate() {
        let gap = failures[index + 1].duration_since(failures[index]);
        assert!(
            gap >= Duration::from_millis(wait),
            "retry {} came {gap:?} after the failure: the backoff is 50 ms doubling ({wait} ms at least)",
            index + 1
        );
    }
    assert_eq!(
        reports(&presenter).len(),
        1,
        "only the first-frame report: no Failed by count"
    );
    // A later success resumes rendering.
    let target = probes.frames() + 1;
    presenter.set_clear(colour(9));
    rig.wait("a frame after the recovery", || probes.frames() >= target);
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven, "{ended:?}");
    check_ledger_order(&probes.events());
    rig.close(&ended);
}

/// 6c. A redraw trigger retries a failed creation at once, without waiting for the backoff.
fn a_redraw_trigger_retries_a_failed_creation_at_once() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut hooks = probes.hooks();
    hooks.swapchain = vec![None, Some(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY), None];
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    rig.wait("the first frame's fences to be proven", || {
        probes.proofs_in()
    });
    let (width, height) = rig.pixel;
    presenter.set_pixel_size(width - 20, height - 20);
    rig.wait("the failed creation", || {
        probes
            .events()
            .iter()
            .any(|e| matches!(e, Event::CreationFailed(_)))
    });
    presenter.request_redraw();
    rig.wait("the retry", || probes.observer.chains_created() == 2);
    let events = probes.events();
    let times = probes.observer.event_times();
    let at = |wanted: fn(&Event) -> bool| {
        events
            .iter()
            .zip(&times)
            .find(|(event, _)| wanted(event))
            .map(|(_, time)| *time)
            .expect("the event")
    };
    let failed = at(|e| matches!(e, Event::CreationFailed(_)));
    let created = events
        .iter()
        .zip(&times)
        .filter(|(event, _)| matches!(event, Event::ChainCreated { .. }))
        .nth(1)
        .map(|(_, time)| *time)
        .expect("the second swap chain");
    assert!(
        created.duration_since(failed) < Duration::from_millis(40),
        "a redraw request retried the creation before the 50 ms backoff: {:?}",
        created.duration_since(failed)
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven, "{ended:?}");
    rig.close(&ended);
}

/// 6e. A stop cancels the backoff: the thread does not sleep through the wait before it tears down.
fn a_stop_during_a_creation_backoff_ends_at_once() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut hooks = probes.hooks();
    hooks.swapchain = std::iter::once(None)
        .chain(std::iter::repeat_n(
            Some(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY),
            10,
        ))
        .collect();
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    rig.wait("the first frame's fences to be proven", || {
        probes.proofs_in()
    });
    let (width, height) = rig.pixel;
    presenter.set_pixel_size(width - 20, height - 20);
    // After the fourth failure the thread waits 400 ms before the next attempt.
    rig.wait("the fourth failed creation", || {
        count(&probes.events(), |e| matches!(e, Event::CreationFailed(_))) >= 4
    });
    let started = Instant::now();
    presenter.request_stop();
    let ended = presenter.poll_end(BOUND).expect("observed finished");
    let took = started.elapsed();
    assert_eq!(ended, CleanupResult::Proven, "{ended:?}");
    assert!(
        took < Duration::from_millis(250),
        "the stop ended the 400 ms backoff at once: {took:?}"
    );
    check_ledger_order(&probes.events());
    rig.close(&ended);
}

/// 6f. A capability query that fails follows the failure and proof rules: a lost surface is one `Failed` and
///     a proven teardown; a lost device is one `Failed` and an unproven one (see the device-loss cases).
fn a_failed_capability_query_is_one_failed() {
    let mut rig = Rig::open();
    // After a suboptimal result (the first present, injected).
    let probes = Probes::new();
    let mut hooks = probes.hooks();
    hooks.present = vec![Some(vk::Result::SUBOPTIMAL_KHR)];
    hooks.capability_query = vec![None, Some(vk::Result::ERROR_SURFACE_LOST_KHR)];
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the failure", || {
        probes
            .events()
            .iter()
            .any(|e| matches!(e, Event::Failed(_)))
    });
    rig.idle(Duration::from_millis(200));
    let list = reports(&presenter);
    assert_eq!(
        failed_messages(&list).len(),
        1,
        "after a suboptimal result: {list:?}"
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven, "{ended:?}");
    check_ledger_order(&probes.events());
    // Before a rebuild (the resize).
    let probes = Probes::new();
    let mut hooks = probes.hooks();
    hooks.capability_query = vec![None, Some(vk::Result::ERROR_SURFACE_LOST_KHR)];
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    let (width, height) = rig.pixel;
    presenter.set_pixel_size(width - 20, height - 20);
    rig.wait("the failure", || {
        probes
            .events()
            .iter()
            .any(|e| matches!(e, Event::Failed(_)))
    });
    let list = reports(&presenter);
    assert_eq!(
        failed_messages(&list).len(),
        1,
        "before a rebuild: {list:?}"
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven, "{ended:?}");
    check_ledger_order(&probes.events());
    rig.close(&ended);
}

/// 6g. A surface whose limited extent has a zero dimension is not drawable: rendering is suspended and no
///     swap chain is created for it, however the window's own size changes; it resumes with the extent.
fn a_surface_with_a_zero_extent_suspends_rendering() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let extent: SurfaceExtent = Arc::new(Mutex::new(None));
    let mut hooks = probes.hooks();
    hooks.surface_extent = Some(Arc::clone(&extent));
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    let chains = probes.observer.chains_created();
    let frames = probes.frames();
    *extent.lock().expect("the extent") = Some((0, 0));
    let (width, height) = rig.pixel;
    presenter.set_pixel_size(width - 20, height - 20);
    presenter.set_clear(colour(3));
    rig.idle(Duration::from_millis(400));
    assert_eq!(
        probes.observer.chains_created(),
        chains,
        "no swap chain for a zero extent"
    );
    assert_eq!(probes.frames(), frames, "nothing drawn while suspended");
    assert_eq!(
        reports(&presenter),
        vec![PresenterReport::FirstFrameQueued],
        "suspended is not a failure"
    );
    *extent.lock().expect("the extent") = None;
    presenter.request_redraw();
    rig.wait("rendering resumes", || probes.frames() > frames);
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven, "{ended:?}");
    check_ledger_order(&probes.events());
    rig.close(&ended);
}

/// 9h. Retirement is not teardown, for a stop that wakes the idle wait: the stop arrives after the thread's wake
///     and before it services the fences (held there by a scheduling pause), with a swap chain retired and
///     proven and the current swap chain's proofs unproven. Nothing is destroyed, the retired swap chain
///     included: a stop already requested decides it, not a flag set later inside the teardown.
fn a_stop_before_the_idle_service_keeps_the_retired_swap_chain() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let never = Arc::new(AtomicBool::new(false));
    let pause = Arc::new(Pause::new());
    let mut hooks = probes.hooks();
    hooks.fences_never_signal = Some(Arc::clone(&never));
    hooks.pause_before_idle_service = Some(Arc::clone(&pause));
    hooks.stop_budget = Some(Duration::from_millis(600));
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    rig.wait("the first frame's fences to be proven", || {
        probes.proofs_in()
    });
    // The rebuild retires the idle (proven) swap chain 1 and the frame on swap chain 2 leaves proofs that
    // never come; the thread then waits to reach its next idle service.
    never.store(true, Ordering::SeqCst);
    pause.hold();
    let (width, height) = rig.pixel;
    presenter.set_pixel_size(width - 20, height - 20);
    rig.wait("the frame on the new swap chain", || {
        probes.observer.chains_created() == 2 && probes.frames() >= 2
    });
    rig.wait("the thread at its idle service", || pause.reached());
    // The stop arrives now: after the wake, before the service.
    presenter.request_stop();
    pause.release();
    let ended = presenter.poll_end(BOUND).expect("observed finished");
    assert!(matches!(ended, CleanupResult::Unproven { .. }), "{ended:?}");
    let events = probes.events();
    assert_eq!(
        count(&events, |e| matches!(e, Event::ChainRetired(_))),
        1,
        "{events:?}"
    );
    check_nothing_destroyed(&events);
    rig.retire();
}

/// An unproven off-screen readback keeps the instance, but the surface is destroyed and the outcome says so,
/// so nobody destroys it twice.
fn a_retained_readback_keeps_the_instance_and_not_the_surface() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut presenter = rig.start(&probes, probes.hooks());
    rig.wait("the first frame", || probes.frames() >= 1);
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    drop(presenter);
    let error = readback_clear_with_test_hooks(
        &rig.instance,
        [0.1, 0.2, 0.3],
        ReadbackHooks {
            wait: Some(vk::Result::TIMEOUT),
            ..ReadbackHooks::default()
        },
    )
    .expect_err("an unproven readback");
    assert!(error.message().contains("unproven"), "{error}");
    rig.assert_clean();
    let log = rig.log.clone();
    let Rig {
        backend,
        handles,
        instance,
        surface,
        loader,
        ..
    } = rig;
    // SAFETY: the presenter ended `Proven`, nothing else uses the surface and the window is alive.
    let refused = unsafe { destroy_surface_and_instance(instance, surface, Some(&ended)) }
        .expect_err("the retained readback keeps the instance");
    if env::var(INJECT_AFTER_DESTROY_VARIABLE).as_deref() == Ok("1") {
        assert!(
            refused
                .instance
                .submit_test_warning("a message after the surface's destruction")
        );
    }
    // Validation is asserted after the surface's destruction too, with the instance still retained.
    assert_eq!(
        log.messages(),
        Vec::<String>::new(),
        "validation reported something after the surface was destroyed"
    );
    assert!(
        refused.surface_destroyed,
        "the surface is already destroyed: {refused}"
    );
    assert!(refused.error.message().contains("readback"), "{refused}");
    drop(refused.instance);
    drop(loader);
    // The window is left to the process exit with the retained objects.
    std::mem::forget(handles);
    std::mem::forget(backend);
}

/// 6d. A creation that fails because the surface is lost, or its native window is in use, cannot recover:
///     one `Failed`, then the teardown the ledger allows.
fn an_unrecoverable_creation_failure_is_one_failed() {
    let mut rig = Rig::open();
    for code in [
        vk::Result::ERROR_SURFACE_LOST_KHR,
        vk::Result::ERROR_NATIVE_WINDOW_IN_USE_KHR,
    ] {
        let probes = Probes::new();
        let mut hooks = probes.hooks();
        hooks.swapchain = vec![None, Some(code)];
        let mut presenter = rig.start(&probes, hooks);
        rig.wait("the first frame", || probes.frames() >= 1);
        let (width, height) = rig.pixel;
        presenter.set_pixel_size(width - 20, height - 20);
        rig.wait("the failure", || {
            probes
                .events()
                .iter()
                .any(|e| matches!(e, Event::Failed(_)))
        });
        rig.idle(Duration::from_millis(300));
        let list = reports(&presenter);
        assert_eq!(failed_messages(&list).len(), 1, "{code:?}: {list:?}");
        assert_eq!(
            count(&probes.events(), |e| matches!(e, Event::CreationFailed(_))),
            1,
            "{code:?}: no retry of an unrecoverable failure"
        );
        let ended = Rig::end(&mut presenter);
        assert_eq!(ended, CleanupResult::Proven, "{code:?}: {ended:?}");
        check_ledger_order(&probes.events());
    }
    rig.close(&CleanupResult::Proven);
}

/// 7. Zero size: a zero pixel size suspends rendering and creates no swap chain; a non-zero size resumes.
fn zero_size() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut presenter = rig.start_sized(&probes, probes.hooks(), (0, 0));
    rig.idle(Duration::from_millis(400));
    assert_eq!(
        probes.observer.chains_created(),
        0,
        "no zero-size swap chain: {:?}",
        probes.events()
    );
    assert_eq!(probes.frames(), 0);
    assert_eq!(reports(&presenter), vec![], "suspended is not a failure");
    let (width, height) = rig.pixel;
    presenter.set_pixel_size(width, height);
    rig.wait("rendering resumes", || probes.frames() >= 1);
    assert_eq!(probes.observer.chains_created(), 1);
    // Minimised again: one dimension zero suspends, whatever else changes.
    let chains = probes.observer.chains_created();
    let frames = probes.frames();
    presenter.set_pixel_size(0, height);
    presenter.set_clear([0.9, 0.9, 0.1]);
    presenter.request_redraw();
    rig.idle(Duration::from_millis(400));
    assert_eq!(
        probes.observer.chains_created(),
        chains,
        "no swap chain for a zero width"
    );
    assert_eq!(probes.frames(), frames, "nothing drawn while suspended");
    presenter.set_pixel_size(width, 0);
    rig.idle(Duration::from_millis(200));
    assert_eq!(
        probes.observer.chains_created(),
        chains,
        "no swap chain for a zero height"
    );
    // Restored to the size the swap chain already has: no rebuild, and the pending colour is drawn.
    presenter.set_pixel_size(width, height);
    rig.wait("rendering resumes at the old size", || {
        probes.frames() > frames
    });
    assert_eq!(
        probes.observer.chains_created(),
        chains,
        "the swap chain was kept while suspended"
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    check_ledger_order(&probes.events());
    // A presenter that ends while suspended with no swap chain at all is proven too.
    let probes_two = Probes::new();
    let mut idle = rig.start_sized(&probes_two, probes_two.hooks(), (0, 0));
    rig.idle(Duration::from_millis(200));
    let ended_two = Rig::end(&mut idle);
    assert_eq!(ended_two, CleanupResult::Proven);
    assert_eq!(probes_two.observer.chains_created(), 0);
    rig.close(&ended_two);
}

/// 2. A missing required extension is a `Failed` naming it, then `Proven` (nothing was created).
fn missing_extension() {
    let mut rig = Rig::open();
    for name in ["VK_EXT_swapchain_maintenance1", "VK_KHR_swapchain"] {
        let probes = Probes::new();
        let mut hooks = probes.hooks();
        hooks.hide_extension = Some(name.to_owned());
        let mut presenter = rig.start(&probes, hooks);
        rig.wait("the failure", || {
            probes
                .events()
                .iter()
                .any(|e| matches!(e, Event::Failed(_)))
        });
        let list = reports(&presenter);
        let messages = failed_messages(&list);
        assert_eq!(messages.len(), 1, "{list:?}");
        assert!(
            messages[0].contains(name),
            "the message names {name}: {}",
            messages[0]
        );
        let ended = Rig::end(&mut presenter);
        assert_eq!(
            ended,
            CleanupResult::Proven,
            "nothing was created that cannot be proven: {ended:?}"
        );
        assert_eq!(
            probes.events().len(),
            1,
            "only the failure: {:?}",
            probes.events()
        );
        assert_eq!(probes.frames(), 0);
    }
    rig.close(&CleanupResult::Proven);
}

/// 8. Loader lifetime: the library is released only after every instance, presenter (thread included) and
///    readback using it is gone; no dynamic-loader variable is involved.
fn loader_lifetime() {
    assert!(
        env::vars().all(|(name, _)| !name.starts_with("DYLD_") && name != "LD_LIBRARY_PATH"),
        "no dynamic-loader variable is set"
    );
    let mut rig = Rig::open();
    let watch = rig.loader.watch();
    let path = rig
        .loader
        .path()
        .expect("loaded by an explicit path")
        .to_owned();
    assert!(path.is_absolute(), "{}", path.display());
    let probes = Probes::new();
    let base_clones = rig.loader.clones();
    let mut presenter = rig.start(&probes, probes.hooks());
    rig.wait("the first frame", || probes.frames() >= 1);
    // The handle and the thread (two holds: its own and the device's) each hold a clone, on top of the
    // test's, the window side's and the instance's.
    assert_eq!(rig.loader.clones(), base_clones + 3);
    assert!(!watch.released());
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    // The thread's clones went at the proven end; the handle's own is still held.
    assert!(
        !watch.released(),
        "the handle, the instance and GLFW still hold the loader"
    );
    drop(presenter);
    assert_eq!(
        rig.loader.clones(),
        base_clones,
        "the thread released both of its holds at the proven end, the handle its own on drop"
    );
    assert_eq!(
        rig.instance.holders(),
        1,
        "and the thread's holds on the instance"
    );
    assert!(!watch.released(), "the instance and GLFW still hold it");
    // The rig closes: surface, instance, window, GLFW (its keep-alive is released after the NULL reset),
    // and the rig's own clone last.
    let loader = rig.loader.clone();
    rig.close(&ended);
    assert!(!watch.released(), "this test's own clone is the last");
    assert_eq!(loader.clones(), 1);
    drop(loader);
    assert!(watch.released(), "the last drop unloads the library");
}

/// 9a. `Unproven` with nothing destroyed for a forced fence timeout, and the loader stays loaded.
fn unproven_fence_timeout() {
    let mut rig = Rig::open();
    let watch = rig.loader.watch();
    let probes = Probes::new();
    let never = Arc::new(AtomicBool::new(false));
    let mut hooks = probes.hooks();
    hooks.fences_never_signal = Some(Arc::clone(&never));
    hooks.stop_budget = Some(Duration::from_millis(600));
    let mut presenter = rig.start(&probes, hooks);
    never.store(true, Ordering::SeqCst);
    rig.wait("the first frame", || probes.frames() >= 1);
    let started = Instant::now();
    presenter.request_stop();
    let ended = presenter.poll_end(BOUND).expect("observed finished");
    let took = started.elapsed();
    let CleanupResult::Unproven { reason } = &ended else {
        panic!("a fence that never signals must give Unproven, got {ended:?}");
    };
    assert!(reason.contains("fence"), "{reason}");
    assert!(
        took >= Duration::from_millis(550) && took < Duration::from_millis(3000),
        "{took:?}"
    );
    check_nothing_destroyed(&probes.events());
    rig.assert_clean();
    // The instance and the loader are retained with the thread.
    let Rig {
        backend,
        instance,
        surface,
        loader,
        ..
    } = rig;
    // SAFETY: the surface belongs to the instance; the call is refused because the end was not proven.
    let refused = unsafe { destroy_surface_and_instance(instance, surface, Some(&ended)) }
        .expect_err("the surface and instance are kept");
    assert!(refused.error.message().contains("not proven"), "{refused}");
    assert!(
        !refused.surface_destroyed,
        "an unproven end destroys not even the surface"
    );
    drop(presenter);
    drop(refused.instance);
    drop(loader);
    assert!(
        !watch.released(),
        "the retained render thread keeps the library loaded"
    );
    std::mem::forget(backend);
}

/// 9b. `Unproven` with nothing destroyed for a forced device loss, from the acquire, the submit or the
///     present; one `Failed`.
fn device_lost_in(name: &str, inject: fn(&mut TestHooks), resize: bool) {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut set = probes.hooks();
    inject(&mut set);
    let mut presenter = rig.start(&probes, set);
    rig.wait("the first frame", || probes.frames() >= 1);
    if resize {
        let (width, height) = rig.pixel;
        presenter.set_pixel_size(width - 20, height - 20);
    } else {
        presenter.set_clear([0.9, 0.9, 0.9]);
    }
    rig.wait("the failure", || {
        probes
            .events()
            .iter()
            .any(|e| matches!(e, Event::Failed(_)))
    });
    rig.idle(Duration::from_millis(200));
    let list = reports(&presenter);
    assert_eq!(failed_messages(&list).len(), 1, "{name}: {list:?}");
    let ended = Rig::end(&mut presenter);
    let CleanupResult::Unproven { reason } = &ended else {
        panic!("{name}: device loss must give Unproven, got {ended:?}");
    };
    assert!(reason.contains("lost"), "{name}: {reason}");
    check_nothing_destroyed(&probes.events());
    // Keep everything: the window and GLFW are left to the process exit.
    rig.retire();
}

fn unproven_device_lost_in_acquire() {
    device_lost_in(
        "acquire",
        |h| h.acquire = vec![None, Some(vk::Result::ERROR_DEVICE_LOST)],
        false,
    );
}

fn unproven_device_lost_in_submit() {
    device_lost_in(
        "submit",
        |h| h.submit = vec![None, Some(vk::Result::ERROR_DEVICE_LOST)],
        false,
    );
}

fn unproven_device_lost_in_present() {
    device_lost_in(
        "present",
        |h| h.present = vec![None, Some(vk::Result::ERROR_DEVICE_LOST)],
        false,
    );
}

fn unproven_device_lost_in_a_capability_query() {
    device_lost_in(
        "capability query",
        |h| {
            h.present = vec![Some(vk::Result::SUBOPTIMAL_KHR)];
            h.capability_query = vec![None, Some(vk::Result::ERROR_DEVICE_LOST)];
        },
        false,
    );
}

fn unproven_device_lost_in_creation() {
    device_lost_in(
        "creation",
        |h| h.swapchain = vec![None, Some(vk::Result::ERROR_DEVICE_LOST)],
        true,
    );
}

/// 9f. Retirement is not teardown: a swap chain retired by a rebuild is destroyed while the rendering is
///     healthy, but after a device loss nothing is destroyed any more, the retired swap chain included.
fn device_loss_with_a_swap_chain_retired() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut hooks = probes.hooks();
    // The first acquire is the first frame; the second is in the step that rebuilds after the resize.
    hooks.acquire = vec![None, Some(vk::Result::ERROR_DEVICE_LOST)];
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    rig.wait("the first frame's fences to be proven", || {
        probes.proofs_in()
    });
    let (width, height) = rig.pixel;
    presenter.set_pixel_size(width - 20, height - 20);
    rig.wait("the failure", || {
        probes
            .events()
            .iter()
            .any(|e| matches!(e, Event::Failed(_)))
    });
    // Long enough for the thread to look at the retired swap chain's (signalled) fences many times.
    rig.idle(Duration::from_millis(400));
    let events = probes.events();
    assert_eq!(
        count(&events, |e| matches!(e, Event::ChainRetired(_))),
        1,
        "{events:?}"
    );
    let ended = Rig::end(&mut presenter);
    assert!(matches!(ended, CleanupResult::Unproven { .. }), "{ended:?}");
    check_nothing_destroyed(&probes.events());
    rig.retire();
}

/// 9g. Retirement is not teardown, in the teardown: with a swap chain retired and its fences proven, a
///     teardown whose current swap chain's proof times out destroys nothing, the retired swap chain
///     included.
fn teardown_timeout_with_a_swap_chain_retired() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let hold = Arc::new(AtomicBool::new(false));
    let never = Arc::new(AtomicBool::new(false));
    let mut hooks = probes.hooks();
    hooks.hold_acquire = Some(Arc::clone(&hold));
    hooks.fences_never_signal = Some(Arc::clone(&never));
    hooks.stop_budget = Some(Duration::from_millis(700));
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    rig.wait("the first frame's fences to be proven", || {
        probes.proofs_in()
    });
    // The rebuild retires the idle swap chain and the thread blocks before its next acquire...
    hold.store(true, Ordering::SeqCst);
    let (width, height) = rig.pixel;
    presenter.set_pixel_size(width - 20, height - 20);
    rig.wait("the rebuild", || probes.observer.chains_created() == 2);
    // ...the stop arrives, and what the blocked frame leaves behind is never proven.
    never.store(true, Ordering::SeqCst);
    presenter.request_stop();
    rig.idle(Duration::from_millis(100));
    hold.store(false, Ordering::SeqCst);
    let ended = presenter.poll_end(BOUND).expect("observed finished");
    assert!(matches!(ended, CleanupResult::Unproven { .. }), "{ended:?}");
    let events = probes.events();
    assert_eq!(
        count(&events, |e| matches!(e, Event::ChainRetired(_))),
        1,
        "{events:?}"
    );
    check_nothing_destroyed(&events);
    rig.retire();
}

/// 9c. `Unproven` with nothing destroyed for a stop-time panic; the loader stays loaded.
fn unproven_stop_panic() {
    let mut rig = Rig::open();
    let watch = rig.loader.watch();
    let probes = Probes::new();
    let mut hooks = probes.hooks();
    hooks.panic_at_stop = true;
    let base_clones = rig.loader.clones();
    let base_holders = rig.instance.holders();
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    let ended = Rig::end(&mut presenter);
    let CleanupResult::Unproven { reason } = &ended else {
        panic!("a stop-time panic must give Unproven, got {ended:?}");
    };
    assert!(reason.contains("panicked"), "{reason}");
    check_nothing_destroyed(&probes.events());
    drop(presenter);
    // The unwound thread kept both of its holds on the instance and both on the loader (its own and the
    // device's): nothing was released by the unwind.
    assert_eq!(
        rig.instance.holders(),
        base_holders + 2,
        "the unwound thread kept its holds on the instance"
    );
    assert_eq!(
        rig.loader.clones(),
        base_clones + 2,
        "the unwound thread kept its holds on the loader"
    );
    rig.assert_clean();
    let Rig {
        backend,
        instance,
        loader,
        ..
    } = rig;
    drop(instance);
    drop(loader);
    assert!(
        !watch.released(),
        "the panicked thread kept the library loaded"
    );
    std::mem::forget(backend);
}

/// A panic payload whose `Drop` panics. It notes how many wakes the presenter had made when it was dropped,
/// so the case can tell that the render thread disposed of it before it published its result and made its
/// last wake.
struct Bomb;

static BOMB_WAKES: Mutex<Option<Arc<AtomicU64>>> = Mutex::new(None);
static WAKES_AT_DROP: AtomicU64 = AtomicU64::new(u64::MAX);
static BOMB_DROPS: AtomicUsize = AtomicUsize::new(0);

impl Drop for Bomb {
    fn drop(&mut self) {
        if let Some(wakes) = BOMB_WAKES.lock().ok().and_then(|slot| slot.clone()) {
            WAKES_AT_DROP.store(wakes.load(Ordering::SeqCst), Ordering::SeqCst);
        }
        BOMB_DROPS.fetch_add(1, Ordering::SeqCst);
        panic!("a payload whose drop panics");
    }
}

fn raise_bomb() -> ! {
    std::panic::panic_any(Bomb)
}

/// 9d. A render thread panic whose payload's own `Drop` panics is the same `Unproven` with nothing destroyed and
/// the same holds kept. The payload is disposed of inside the thread's boundary, before the result is stored
/// and the last wake made (so the thread never panics a second time on its way out), and the result is observed.
fn unproven_render_panic_with_a_dropping_payload() {
    let mut rig = Rig::open();
    let watch = rig.loader.watch();
    let probes = Probes::new();
    *BOMB_WAKES.lock().expect("the wake slot") = Some(Arc::clone(&probes.wakes));
    let mut hooks = probes.hooks();
    hooks.panic_after_frames = Some(1);
    hooks.forced_panic_payload = Some(raise_bomb);
    let base_clones = rig.loader.clones();
    let base_holders = rig.instance.holders();
    let mut presenter = rig.start(&probes, hooks);
    // The panic follows the first presented frame; the stop comes after it, so the thread cannot end first.
    rig.wait("the first frame", || probes.frames() >= 1);
    let ended = Rig::end(&mut presenter);
    let CleanupResult::Unproven { reason } = &ended else {
        panic!("a render thread panic must give Unproven, got {ended:?}");
    };
    assert!(reason.contains("panicked"), "{reason}");
    assert_eq!(
        BOMB_DROPS.load(Ordering::SeqCst),
        1,
        "the payload was disposed of once"
    );
    let wakes = probes.wakes.load(Ordering::SeqCst);
    assert!(
        WAKES_AT_DROP.load(Ordering::SeqCst) < wakes,
        "the payload was dropped after the last wake ({} of {wakes}): not before the result was published",
        WAKES_AT_DROP.load(Ordering::SeqCst)
    );
    check_nothing_destroyed(&probes.events());
    drop(presenter);
    assert_eq!(
        rig.instance.holders(),
        base_holders + 2,
        "the unwound thread kept its holds on the instance"
    );
    assert_eq!(
        rig.loader.clones(),
        base_clones + 2,
        "the unwound thread kept its holds on the loader"
    );
    rig.assert_clean();
    let Rig {
        backend,
        instance,
        loader,
        ..
    } = rig;
    drop(instance);
    drop(loader);
    assert!(
        !watch.released(),
        "the panicked thread kept the library loaded"
    );
    std::mem::forget(backend);
}

/// A presenter blocked before an acquire when the stop arrives, whose fences then read as never signalled
/// (so its teardown can only wait out the deadline): how long from the first `request_stop` to its end.
/// `again` is a second `request_stop` that many milliseconds after the first; the blocked acquire is
/// released `release` milliseconds after the first.
fn blocked_stop(budget: u64, again: Option<u64>, release: u64) -> Duration {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let hold = Arc::new(AtomicBool::new(false));
    let never = Arc::new(AtomicBool::new(false));
    let mut hooks = probes.hooks();
    hooks.hold_acquire = Some(Arc::clone(&hold));
    hooks.fences_never_signal = Some(Arc::clone(&never));
    hooks.stop_budget = Some(Duration::from_millis(budget));
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    rig.wait("the first frame's fences to be proven", || {
        probes.proofs_in()
    });
    // The thread blocks before its next acquire...
    hold.store(true, Ordering::SeqCst);
    presenter.set_clear([0.3, 0.6, 0.9]);
    rig.idle(Duration::from_millis(150));
    // ...and what that frame leaves behind is never proven.
    never.store(true, Ordering::SeqCst);
    let started = Instant::now();
    presenter.request_stop();
    if let Some(after) = again {
        rig.idle(Duration::from_millis(after));
        presenter.request_stop();
    }
    while started.elapsed() < Duration::from_millis(release) {
        rig.pump();
        thread::sleep(Duration::from_millis(3));
    }
    hold.store(false, Ordering::SeqCst);
    let ended = presenter.poll_end(BOUND).expect("observed finished");
    let took = started.elapsed();
    assert!(matches!(ended, CleanupResult::Unproven { .. }), "{ended:?}");
    check_nothing_destroyed(&probes.events());
    rig.retire();
    took
}

/// 9d. One absolute stop deadline: a repeated `request_stop` does not extend it, even while the thread is
///     blocked and has not started its teardown.
fn stop_deadline_ignores_a_repeated_request() {
    let took = blocked_stop(1500, Some(400), 800);
    assert!(
        took >= Duration::from_millis(1400) && took < Duration::from_millis(1750),
        "one deadline of 1500 ms from the first request, not from the second: {took:?}"
    );
}

/// 9e. An acquire that was blocked when the stop arrived spends the same budget: the teardown starts late
///     and has only what is left.
fn stop_deadline_includes_a_blocked_acquire() {
    let took = blocked_stop(1500, None, 800);
    assert!(
        took >= Duration::from_millis(1400) && took < Duration::from_millis(1750),
        "800 ms blocked plus the teardown stay inside one 1500 ms budget, not 800 + 1500: {took:?}"
    );
}

/// The surface and instance cannot be destroyed while the render thread still holds the instance, however
/// the caller describes the presenter's end, and can once the end is proven.
fn destroy_refuses_while_the_thread_runs() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut presenter = rig.start(&probes, probes.hooks());
    rig.wait("the first frame", || probes.frames() >= 1);
    let Rig {
        backend,
        handles,
        instance,
        surface,
        loader,
        log,
        pixel,
    } = rig;
    // "No presenter was ever started" is untrue here, and the instance is still held by the thread.
    // SAFETY: the surface belongs to the instance; the call is refused and destroys nothing.
    let refused = unsafe { destroy_surface_and_instance(instance, surface, None) }
        .expect_err("refused while the thread holds the instance");
    assert!(refused.error.message().contains("still holds"), "{refused}");
    assert!(
        !refused.surface_destroyed,
        "the surface is not destroyed under a running thread"
    );
    // SAFETY: as above.
    let refused = unsafe {
        destroy_surface_and_instance(refused.instance, surface, Some(&CleanupResult::Proven))
    }
    .expect_err("a claimed proof does not release a thread that is still running");
    assert!(refused.error.message().contains("still holds"), "{refused}");
    assert!(!refused.surface_destroyed);
    let instance = refused.instance;
    let rig = Rig {
        backend,
        handles,
        instance,
        surface,
        loader,
        log,
        pixel,
    };
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    rig.close(&ended);
}

/// 10. A forced unrecoverable error gives one `Failed`, nothing more drawn, then the teardown the ledger
///     allows (here `Proven`).
fn unrecoverable_failure() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let mut hooks = probes.hooks();
    hooks.present = vec![None, Some(vk::Result::ERROR_SURFACE_LOST_KHR)];
    let mut presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    presenter.set_clear([0.9, 0.2, 0.2]);
    rig.wait("the failure", || {
        probes
            .events()
            .iter()
            .any(|e| matches!(e, Event::Failed(_)))
    });
    for n in 0..5 {
        presenter.set_clear(colour(n + 10));
        presenter.request_redraw();
    }
    rig.idle(Duration::from_millis(300));
    let list = reports(&presenter);
    assert_eq!(
        failed_messages(&list).len(),
        1,
        "exactly one Failed: {list:?}"
    );
    assert_eq!(
        probes.frames(),
        1,
        "the failed frame and every later one drew nothing"
    );
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven, "{ended:?}");
    check_ledger_order(&probes.events());
    rig.close(&ended);
}

/// 11. Dropping the handle never blocks, even with a blocked acquire; late work touches only the thread's
///     own state and the thread ends by itself, proven.
fn handle_drop_never_blocks() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let hold = Arc::new(AtomicBool::new(false));
    let mut hooks = probes.hooks();
    hooks.hold_acquire = Some(Arc::clone(&hold));
    let presenter = rig.start(&probes, hooks);
    rig.wait("the first frame", || probes.frames() >= 1);
    hold.store(true, Ordering::SeqCst);
    presenter.set_clear([0.4, 0.4, 0.9]);
    rig.idle(Duration::from_millis(100));
    let started = Instant::now();
    drop(presenter);
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "the drop blocked: {:?}",
        started.elapsed()
    );
    // The thread is still blocked in its hold; the instance cannot be destroyed while it holds it.
    assert!(
        rig.instance.holders() > 1,
        "the running thread holds the instance"
    );
    hold.store(false, Ordering::SeqCst);
    rig.wait("the thread's own teardown", || {
        probes
            .events()
            .iter()
            .any(|e| matches!(e, Event::Destroyed("device")))
    });
    rig.wait_rig("the thread's hold on the instance to go", |rig| {
        rig.instance.holders() == 1
    });
    check_ledger_order(&probes.events());
    rig.close(&CleanupResult::Proven);
}

// ---------------------------------------------------------------------------------------------
// The harness
// ---------------------------------------------------------------------------------------------

type Case = fn();

const CASES: [(&str, Case); 33] = [
    ("first_frame_and_wake", first_frame_and_wake),
    ("redraw_on_demand", redraw_on_demand),
    ("present_lifetime", present_lifetime),
    (
        "an_empty_pool_skips_the_frame_and_keeps_the_redraw_pending",
        an_empty_pool_skips_the_frame_and_keeps_the_redraw_pending,
    ),
    (
        "a_suboptimal_result_at_the_current_configuration_does_not_rebuild",
        a_suboptimal_result_at_the_current_configuration_does_not_rebuild,
    ),
    (
        "a_suboptimal_result_after_the_surface_changed_rebuilds_once",
        a_suboptimal_result_after_the_surface_changed_rebuilds_once,
    ),
    (
        "a_suboptimal_result_compares_the_clamped_extent",
        a_suboptimal_result_compares_the_clamped_extent,
    ),
    (
        "failed_creations_are_retried_with_a_backoff_and_never_fail_by_count",
        failed_creations_are_retried_with_a_backoff_and_never_fail_by_count,
    ),
    (
        "a_redraw_trigger_retries_a_failed_creation_at_once",
        a_redraw_trigger_retries_a_failed_creation_at_once,
    ),
    (
        "an_unrecoverable_creation_failure_is_one_failed",
        an_unrecoverable_creation_failure_is_one_failed,
    ),
    ("ledger_transitions", ledger_transitions),
    ("zero_size", zero_size),
    ("missing_extension", missing_extension),
    ("loader_lifetime", loader_lifetime),
    ("unproven_fence_timeout", unproven_fence_timeout),
    (
        "unproven_device_lost_in_acquire",
        unproven_device_lost_in_acquire,
    ),
    (
        "unproven_device_lost_in_submit",
        unproven_device_lost_in_submit,
    ),
    (
        "unproven_device_lost_in_present",
        unproven_device_lost_in_present,
    ),
    (
        "unproven_device_lost_in_creation",
        unproven_device_lost_in_creation,
    ),
    (
        "unproven_device_lost_in_a_capability_query",
        unproven_device_lost_in_a_capability_query,
    ),
    (
        "a_stop_during_a_creation_backoff_ends_at_once",
        a_stop_during_a_creation_backoff_ends_at_once,
    ),
    (
        "a_failed_capability_query_is_one_failed",
        a_failed_capability_query_is_one_failed,
    ),
    (
        "a_surface_with_a_zero_extent_suspends_rendering",
        a_surface_with_a_zero_extent_suspends_rendering,
    ),
    (
        "a_stop_before_the_idle_service_keeps_the_retired_swap_chain",
        a_stop_before_the_idle_service_keeps_the_retired_swap_chain,
    ),
    (
        "a_retained_readback_keeps_the_instance_and_not_the_surface",
        a_retained_readback_keeps_the_instance_and_not_the_surface,
    ),
    (
        "device_loss_with_a_swap_chain_retired",
        device_loss_with_a_swap_chain_retired,
    ),
    (
        "teardown_timeout_with_a_swap_chain_retired",
        teardown_timeout_with_a_swap_chain_retired,
    ),
    ("unproven_stop_panic", unproven_stop_panic),
    (
        "unproven_render_panic_with_a_dropping_payload",
        unproven_render_panic_with_a_dropping_payload,
    ),
    (
        "destroy_refuses_while_the_thread_runs",
        destroy_refuses_while_the_thread_runs,
    ),
    (
        "stop_deadline_ignores_a_repeated_request",
        stop_deadline_ignores_a_repeated_request,
    ),
    (
        "stop_deadline_includes_a_blocked_acquire",
        stop_deadline_includes_a_blocked_acquire,
    ),
    ("unrecoverable_failure", unrecoverable_failure),
];

/// The twelfth case is separate only because the array above is fixed at twelve; see `EXTRA`.
const EXTRA: [(&str, Case); 2] = [
    ("handle_drop_never_blocks", handle_drop_never_blocks),
    (
        "the_info_binding_matches_the_native_layout",
        the_info_binding_matches_the_native_layout,
    ),
];

/// The tests' Rust binding of the native info structure has the layout the native side was compiled with
/// (size, alignment and every field's offset), checked before any call that writes through it. Opens no
/// window.
fn the_info_binding_matches_the_native_layout() {
    if let Err(message) = support::check_layout() {
        panic!("{message}");
    }
}

fn hang() {
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

fn all_cases() -> Vec<(&'static str, Case)> {
    CASES
        .iter()
        .chain(EXTRA.iter())
        .chain(window_quad::CASES.iter())
        .chain(window_quad::LAST.iter())
        .chain(window_scene::CASES.iter())
        .chain(window_scene::compat::CASES.iter())
        .copied()
        .collect()
}

fn worker(name: &str) -> ExitCode {
    let cases = all_cases();
    let case = cases
        .iter()
        .find(|(case, _)| *case == name)
        .map(|(_, case)| *case)
        .or_else(|| (name == "hang").then_some(hang as Case));
    let Some(case) = case else {
        eprintln!("worker: no case called {name}");
        return ExitCode::from(1);
    };
    // The filter is in before the first window and stays for the whole life of the worker.
    support::install_input_filter();
    if env::var(TYPING_VARIABLE).as_deref() == Ok("1") {
        support::start_typist();
    }
    match catch_unwind(AssertUnwindSafe(case)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(payload) => {
            let text = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_owned())
                })
                .unwrap_or_else(|| "a panic with a non-string payload".to_owned());
            eprintln!("worker: {name} failed: {text}");
            ExitCode::from(1)
        }
    }
}

/// Runs `case` in a worker under the supervisor; `Ok` when it exited 0.
fn run_case(case: &str, typing: bool, deadline: Duration) -> Result<(), String> {
    run_case_with(case, typing, deadline, &[])
}

/// [`run_case`] with extra environment variables for the worker.
fn run_case_with(
    case: &str,
    typing: bool,
    deadline: Duration,
    extra: &[(&str, &str)],
) -> Result<(), String> {
    let mut command = Command::new(env::current_exe().map_err(|error| error.to_string())?);
    command.arg(WORKER_FLAG).arg(case);
    for (name, value) in extra {
        command.env(name, value);
    }
    if typing {
        command.env(TYPING_VARIABLE, "1");
    } else {
        command.env_remove(TYPING_VARIABLE);
    }
    // Cargo sets DYLD_FALLBACK_LIBRARY_PATH for the test run. The worker must find the Vulkan loader by its
    // absolute path alone, so no dynamic-loader variable reaches it.
    for (name, _) in env::vars() {
        if name.starts_with("DYLD_") || name == "LD_LIBRARY_PATH" {
            command.env_remove(name);
        }
    }
    let supervised = Supervised::spawn(command, deadline)?;
    let seen = supervised.finish()?;
    if seen.code == Some(0) {
        Ok(())
    } else {
        Err(format!(
            "{case} ended with {:?}\n{}",
            seen.code, seen.stdout
        ))
    }
}

/// A worker that hangs is killed and reaped at its deadline, and the case fails with that said.
fn check_a_hung_worker_is_killed() -> Result<(), String> {
    let started = Instant::now();
    match run_case("hang", false, Duration::from_millis(500)) {
        Err(message) if message.contains("deadline") || message.contains("did not finish") => {
            if started.elapsed() > Duration::from_secs(20) {
                return Err(format!(
                    "it took {:?} to kill the worker",
                    started.elapsed()
                ));
            }
            Ok(())
        }
        other => Err(format!(
            "a hung worker must fail at its deadline, got {other:?}"
        )),
    }
}

/// Validation reaches the final assertion of every kind of case: a warning injected through the instance's
/// own messenger fails a case that ends proven, one that ends unproven (nothing destroyed), and one that
/// loses the device.
fn check_validation_reaches_every_kind_of_case() -> Result<(), String> {
    // A message right after the surface's destruction, in the case that destroys it with the instance retained.
    match run_case_with(
        "a_retained_readback_keeps_the_instance_and_not_the_surface",
        false,
        DEADLINE,
        &[(INJECT_AFTER_DESTROY_VARIABLE, "1")],
    ) {
        Err(message) if message.contains("after the surface was destroyed") => {}
        other => {
            return Err(format!(
                "a message after the surface's destruction must fail its case, got {other:?}"
            ));
        }
    }
    for case in [
        "first_frame_and_wake",
        "unproven_fence_timeout",
        "unproven_device_lost_in_acquire",
        "device_loss_with_a_swap_chain_retired",
        "teardown_timeout_with_a_swap_chain_retired",
        "a_stop_before_the_idle_service_keeps_the_retired_swap_chain",
        "a_retained_readback_keeps_the_instance_and_not_the_surface",
    ] {
        match run_case_with(case, false, DEADLINE, &[(INJECT_VARIABLE, "1")]) {
            Err(message) if message.contains("validation reported") => {}
            other => {
                return Err(format!(
                    "{case}: an injected validation message must fail the case, got {other:?}"
                ));
            }
        }
    }
    Ok(())
}

type SupervisorCheck = fn() -> Result<(), String>;

fn supervisor() -> ExitCode {
    let mut failures = BTreeMap::new();
    let checks: [(&str, SupervisorCheck); 4] = [
        (
            "the layout check rejects mismatched info bindings",
            support::check_the_comparison_rejects_mismatched_bindings,
        ),
        (
            "nothing else binds the native info structure",
            support::check_nothing_else_binds_the_native_info,
        ),
        (
            "a hung worker is killed and reaped",
            check_a_hung_worker_is_killed,
        ),
        (
            "validation reaches every kind of case",
            check_validation_reaches_every_kind_of_case,
        ),
    ];
    for (name, check) in checks {
        let result = check();
        println!(
            "test supervisor: {name} ... {}",
            if result.is_ok() { "ok" } else { "FAILED" }
        );
        if let Err(message) = result {
            failures.insert(name.to_owned(), message);
        }
    }
    let cases = all_cases();
    // Every case runs on a quiet keyboard, and again while the owner types without stopping.
    for typing in [false, true] {
        for (name, _) in &cases {
            let label = if typing {
                format!("{name} (owner typing)")
            } else {
                (*name).to_owned()
            };
            let started = Instant::now();
            let result = run_case(name, typing, DEADLINE);
            println!(
                "test {label} ... {} ({:.1} s)",
                if result.is_ok() { "ok" } else { "FAILED" },
                started.elapsed().as_secs_f64()
            );
            if let Err(message) = result {
                failures.insert(label, message);
            }
        }
    }
    for (name, message) in &failures {
        println!("---- {name} ----\n{message}");
    }
    let total = checks.len() + 2 * cases.len();
    println!(
        "test result: {}. {} passed; {} failed",
        if failures.is_empty() { "ok" } else { "FAILED" },
        total - failures.len(),
        failures.len()
    );
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [flag, case] if flag == WORKER_FLAG => worker(case),
        // Cargo may pass its own arguments (a filter, `--nocapture`); the cases always all run.
        _ => supervisor(),
    }
}
