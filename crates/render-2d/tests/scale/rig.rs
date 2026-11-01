//! The worker's rig: one real Cocoa window on the main thread, the renderer over a `MetalBackend::with_resources` backend, and a
//! **scene thread** that runs one phase against it (loads, publishes at a fixed cadence, opens the measurement windows).
//!
//! `AppKit` needs the process's main thread, so the window loop (`ck_window::run`) owns it. The presenter belongs to the loop's thread
//! (the backend is moved into the renderer), so the scene thread reaches the metrics through a **mailbox** that the backend wrapper
//! serves on the loop thread at its next pass (`Control`): one command at a time, a reply, never an unbounded wait.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ck_gpu_metal::{MetalContext, Observer, TestHooks};
use ck_render_2d::{
    BackendTestHooks, Filter, FrameStats, Gpu2d, MetalBackend, MetricsReading, MetricsWindow,
    QuadSink, Renderer, ResourceBackendLimits, SceneBridge, SceneStats, SceneWriter, WakeRelay,
    now_ns,
};
use ck_resources::{Assets, LoaderPool, Mode, ResourceLimits, ResourceService, ServiceWakes};
use ck_window::{
    Answer, CleanupResult, LoopEnd, LoopSettings, Notifier, PixelSize, PostError, PresenterReport,
    RequestWriter, Requests, UiAnswer, UiEvent, UiLink, WindowDesc, run,
};
use ck_window_cocoa::{CocoaBackend, CocoaHandles};

use crate::evidence;
use crate::json::J;

unsafe extern "C" {
    fn ck_r2_t_install_input_filter();
    fn ck_r2_t_dropped_keys() -> u64;
}

/// The cadence of the static phases and of every phase that publishes on a schedule: 60 Hz.
pub(crate) const PERIOD_NS: u64 = 16_666_667;
/// How long a mailbox call may take beyond what it asks for.
const CALL_SLACK: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------------------------
// The mailbox to the presenter
// ---------------------------------------------------------------------------------------------

/// What the scene thread asks of the presenter (served on the loop thread).
pub(crate) enum Command {
    Request(MetricsWindow),
    Adopted(u32),
    End(u32, u64),
    Read(u32, Duration),
    Stats,
}

/// The answer to one [`Command`].
pub(crate) enum Reply {
    Refused(String),
    Done,
    Adopted(Option<u64>),
    Reading(Option<Box<MetricsReading>>),
    Stats(Option<Box<SceneStats>>),
}

#[derive(Default)]
struct Slot {
    command: Option<Command>,
    reply: Option<Reply>,
    /// The loop ended: nothing will be served any more.
    ended: bool,
}

/// The mailbox: one command in flight.
#[derive(Default)]
pub(crate) struct Control {
    slot: Mutex<Slot>,
    changed: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Control {
    /// Posts `command`, wakes the loop and waits (at most `within` plus a slack) for the answer; `None` when the loop ended or did
    /// not answer.
    pub(crate) fn call(
        &self,
        notifier: &Notifier,
        command: Command,
        within: Duration,
    ) -> Option<Reply> {
        let limit = Instant::now() + within + CALL_SLACK;
        let mut slot = lock(&self.slot);
        while (slot.command.is_some() || slot.reply.is_some()) && !slot.ended {
            let now = Instant::now();
            if now >= limit {
                return None;
            }
            slot = self
                .changed
                .wait_timeout(slot, limit - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        if slot.ended {
            return None;
        }
        slot.command = Some(command);
        drop(slot);
        notifier.wake();
        let mut slot = lock(&self.slot);
        loop {
            if let Some(reply) = slot.reply.take() {
                self.changed.notify_all();
                return Some(reply);
            }
            let now = Instant::now();
            if slot.ended || now >= limit {
                slot.command = None;
                return None;
            }
            slot = self
                .changed
                .wait_timeout(slot, limit - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Answers the pending command, if any (the loop thread, which owns the presenter).
    fn serve(&self, backend: &MetalBackend) {
        let Some(command) = lock(&self.slot).command.take() else {
            return;
        };
        let reply = match command {
            Command::Request(window) => match backend.request_metrics(window) {
                None => Reply::Refused("the presenter is not started".to_owned()),
                Some(Ok(())) => Reply::Done,
                Some(Err(error)) => Reply::Refused(format!("{error:?}")),
            },
            Command::Adopted(id) => Reply::Adopted(backend.metrics_adopted(id)),
            Command::End(id, end) => match backend.end_metrics(id, end) {
                None => Reply::Refused("the presenter is not started".to_owned()),
                Some(Ok(())) => Reply::Done,
                Some(Err(error)) => Reply::Refused(format!("{error:?}")),
            },
            Command::Read(id, within) => {
                Reply::Reading(backend.read_metrics(id, within).map(Box::new))
            }
            Command::Stats => Reply::Stats(backend.scene_stats().map(Box::new)),
        };
        lock(&self.slot).reply = Some(reply);
        self.changed.notify_all();
    }

    /// The loop ended: every waiter gives up.
    fn end(&self) {
        lock(&self.slot).ended = true;
        self.changed.notify_all();
    }
}

// ---------------------------------------------------------------------------------------------
// The backend wrapper
// ---------------------------------------------------------------------------------------------

/// What the loop thread saw, for the scene thread and the final record.
#[derive(Default)]
pub(crate) struct Seen {
    /// The drawable size in pixels, packed `width << 32 | height` (0: none yet).
    pub(crate) pixels: AtomicU64,
    /// The first frame was queued.
    pub(crate) first_frame: AtomicBool,
    /// A presenter failure, once.
    pub(crate) failure: Mutex<Option<String>>,
    /// The presenter's last numbers, taken when its end was observed.
    pub(crate) last_stats: Mutex<Option<SceneStats>>,
    /// The whole run's frame figures, taken at the same moment.
    pub(crate) last_totals: Mutex<Option<FrameStats>>,
}

impl Seen {
    /// The drawable size, once known.
    pub(crate) fn pixel_size(&self) -> Option<(u32, u32)> {
        match self.pixels.load(Ordering::Acquire) {
            0 => None,
            packed => Some((
                (packed >> 32) as u32,
                u32::try_from(packed & 0xFFFF_FFFF).unwrap_or(0),
            )),
        }
    }

    fn set_pixels(&self, width: u32, height: u32) {
        self.pixels.store(
            (u64::from(width) << 32) | u64::from(height),
            Ordering::Release,
        );
    }
}

/// [`MetalBackend`] that serves the mailbox on the loop thread and keeps what the loop saw. Everything else is a forward.
struct Wrapped {
    inner: MetalBackend,
    control: Arc<Control>,
    seen: Arc<Seen>,
}

impl Gpu2d for Wrapped {
    type Handles = CocoaHandles;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        self.inner.new_sink()
    }

    fn start(
        &mut self,
        handles: CocoaHandles,
        desc: &WindowDesc,
        pixel_size: PixelSize,
        sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        self.seen.set_pixels(pixel_size.width, pixel_size.height);
        self.inner.start(handles, desc, pixel_size, sink)
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        self.inner.set_clear(rgb);
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.seen.set_pixels(width, height);
        self.inner.set_pixel_size(width, height);
    }

    fn request_redraw(&mut self) {
        self.inner.request_redraw();
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        self.control.serve(&self.inner);
        let report = self.inner.take_report();
        match &report {
            Some(PresenterReport::FirstFrameQueued) => {
                self.seen.first_frame.store(true, Ordering::Release);
            }
            Some(PresenterReport::Failed { message }) => {
                *lock(&self.seen.failure) = Some(message.clone());
            }
            None => {}
        }
        report
    }

    fn request_stop(&mut self) {
        self.inner.request_stop();
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        self.control.serve(&self.inner);
        let end = self.inner.poll_end(within);
        if end.is_some() {
            *lock(&self.seen.last_stats) = self.inner.scene_stats();
            *lock(&self.seen.last_totals) = self.inner.metrics_totals();
            self.control.end();
        }
        end
    }
}

// ---------------------------------------------------------------------------------------------
// The UI link
// ---------------------------------------------------------------------------------------------

/// The scripted UI: it notes that the window opened and declines every close request (the owner's keys never get this far; the
/// filter drops them before any window sees them).
struct Ui {
    opened: Arc<AtomicBool>,
    close_requests: Arc<AtomicUsize>,
    answers: Vec<UiAnswer>,
}

impl UiLink for Ui {
    fn post(&mut self, event: UiEvent) -> Result<(), PostError> {
        match event {
            UiEvent::Opened => self.opened.store(true, Ordering::Release),
            UiEvent::CloseRequested { .. } => {
                self.close_requests.fetch_add(1, Ordering::SeqCst);
                self.answers.push(UiAnswer {
                    event,
                    answer: Answer::Decline,
                });
            }
            UiEvent::Update(update) => panic!("unexpected UI update in a scale worker: {update:?}"),
        }
        Ok(())
    }

    fn take_answer(&mut self) -> Option<UiAnswer> {
        self.answers.pop()
    }

    fn is_disconnected(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------------------------
// The scene thread's handle
// ---------------------------------------------------------------------------------------------

/// What a phase works with, on the scene thread.
pub(crate) struct Scene {
    pub(crate) service: ResourceService,
    pub(crate) writer: SceneWriter,
    pub(crate) requests: RequestWriter,
    pub(crate) control: Arc<Control>,
    pub(crate) notifier: Arc<Notifier>,
    pub(crate) observer: Arc<Observer>,
    pub(crate) seen: Arc<Seen>,
    pub(crate) opened: Arc<AtomicBool>,
}

/// A fixed-cadence schedule on the uptime clock: never faster than the period, a late tick taken once and not caught up.
pub(crate) struct Cadence {
    next: u64,
    /// Ticks taken.
    pub(crate) ticks: u64,
    /// Ticks that were late by more than half a period.
    pub(crate) late: u64,
    /// The latest a tick has been, in nanoseconds.
    pub(crate) worst_late_ns: u64,
}

impl Cadence {
    /// A schedule whose first tick is due at `first`.
    pub(crate) fn starting_at(first: u64) -> Self {
        Self {
            next: first,
            ticks: 0,
            late: 0,
            worst_late_ns: 0,
        }
    }

    /// Waits for the next deadline and returns the time it was taken at.
    pub(crate) fn tick(&mut self) -> u64 {
        let due = self.next;
        let now = now_ns();
        if due > now {
            thread::sleep(Duration::from_nanos(due - now));
        }
        let taken = now_ns();
        let late = taken.saturating_sub(due);
        self.ticks += 1;
        self.worst_late_ns = self.worst_late_ns.max(late);
        if late > PERIOD_NS / 2 {
            self.late += 1;
        }
        // The next deadline is the next whole period after the one just taken: a tick that ran late is not made up.
        let periods = late / PERIOD_NS + 1;
        self.next = due + periods * PERIOD_NS;
        taken
    }
}

impl Scene {
    /// Waits for the window to open and report its drawable size, within `bound`.
    pub(crate) fn wait_open(&self, bound: Duration) -> Result<(u32, u32), String> {
        let limit = Instant::now() + bound;
        loop {
            if let Some(message) = lock(&self.seen.failure).clone() {
                return Err(format!("the presenter failed: {message}"));
            }
            if self.opened.load(Ordering::Acquire)
                && self.seen.first_frame.load(Ordering::Acquire)
                && let Some(size) = self.seen.pixel_size()
            {
                return Ok(size);
            }
            if Instant::now() >= limit {
                return Err("the window did not open in time".to_owned());
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// The presenter failed (once), if it did.
    pub(crate) fn failure(&self) -> Option<String> {
        lock(&self.seen.failure).clone()
    }

    fn call(&self, command: Command, within: Duration) -> Option<Reply> {
        self.control.call(&self.notifier, command, within)
    }

    /// Files `window` with the presenter.
    pub(crate) fn request_window(&self, window: MetricsWindow) -> Result<(), String> {
        match self.call(Command::Request(window), Duration::ZERO) {
            Some(Reply::Done) => Ok(()),
            Some(Reply::Refused(why)) => Err(why),
            _ => Err("the presenter did not answer the window request".to_owned()),
        }
    }

    /// Sets the end of window `id`.
    pub(crate) fn end_window(&self, id: u32, end: u64) -> Result<(), String> {
        match self.call(Command::End(id, end), Duration::ZERO) {
            Some(Reply::Done) => Ok(()),
            Some(Reply::Refused(why)) => Err(why),
            _ => Err("the presenter did not answer the window end".to_owned()),
        }
    }

    /// When the presenter adopted window `id`.
    pub(crate) fn adopted(&self, id: u32) -> Option<u64> {
        match self.call(Command::Adopted(id), Duration::ZERO) {
            Some(Reply::Adopted(at)) => at,
            _ => None,
        }
    }

    /// Window `id` once its work is consumed, waiting at most `within`.
    pub(crate) fn read_window(&self, id: u32, within: Duration) -> Option<MetricsReading> {
        match self.call(Command::Read(id, within), within) {
            Some(Reply::Reading(reading)) => reading.map(|reading| *reading),
            _ => None,
        }
    }

    /// The presenter's counters and charges.
    pub(crate) fn stats(&self) -> Option<SceneStats> {
        match self.call(Command::Stats, Duration::ZERO) {
            Some(Reply::Stats(stats)) => stats.map(|stats| *stats),
            _ => None,
        }
    }

    /// Ends the run: the window closes and the loop's teardown proves the presenter's end.
    pub(crate) fn close(&self) {
        self.requests.close();
        self.notifier.wake();
    }
}

// ---------------------------------------------------------------------------------------------
// The rig
// ---------------------------------------------------------------------------------------------

/// One worker's configuration.
pub(crate) struct Config {
    pub(crate) title: &'static str,
    /// The window's size in points.
    pub(crate) points: (u32, u32),
    pub(crate) backend: ResourceBackendLimits,
    pub(crate) service: ResourceLimits,
    /// The hidden ordinary per-texture path (`false`: the bindless table).
    pub(crate) ordinary: bool,
    /// Every frame is also rendered off screen and kept (the verification views).
    pub(crate) capture: bool,
    /// The presenter's other test hooks (the observer and the capture switch are the rig's own).
    pub(crate) presenter_hooks: TestHooks,
    pub(crate) assets: Option<PathBuf>,
    pub(crate) scene_max_quads: u32,
    /// The whole-process deadline.
    pub(crate) deadline: Duration,
}

/// The facts of the end of a worker, for the final record.
pub(crate) struct Ended {
    pub(crate) loop_end: LoopEnd,
    pub(crate) problems: Vec<String>,
    pub(crate) stats: Option<SceneStats>,
    pub(crate) totals: Option<FrameStats>,
    pub(crate) resources: J,
}

/// A hang ends the process, so the supervisor reports it.
fn watchdog(deadline: Duration) {
    thread::spawn(move || {
        thread::sleep(deadline.saturating_sub(Duration::from_secs(10)));
        eprintln!("worker: the window loop did not finish in time");
        std::process::exit(2);
    });
}

/// A stimulus that wakes the loop: frames produce no report, so an idle loop would otherwise sleep.
fn ticker(notifier: &Arc<Notifier>) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
    let running = Arc::new(AtomicBool::new(true));
    let handle = {
        let (notifier, running) = (Arc::clone(notifier), Arc::clone(&running));
        thread::spawn(move || {
            while running.load(Ordering::Acquire) {
                notifier.wake();
                thread::sleep(Duration::from_millis(16));
            }
        })
    };
    (running, handle)
}

/// Runs one phase: opens the window, runs `phase` on the scene thread, closes, tears down in the application's order and returns
/// what the phase produced with the end's own checks.
///
/// # Panics
///
/// When the machine has no Metal device or no desktop session (reported, not hidden).
#[expect(
    clippy::too_many_lines,
    reason = "one phase is one sequential protocol whose steps read best in order"
)]
pub(crate) fn run_phase<F>(config: Config, phase: F) -> (Result<J, String>, Ended)
where
    F: FnOnce(&mut Scene) -> Result<J, String> + Send + 'static,
{
    ck_window::test_support::open_windows_without_activation();
    crate::activation_watch::install(false);
    watchdog(config.deadline);
    let notifier = Notifier::new();
    let requests = Requests::new();
    let writer = requests.writer(Arc::clone(&notifier));
    let reader = requests.reader();
    writer
        .open(WindowDesc {
            title: config.title.to_owned(),
            width: config.points.0,
            height: config.points.1,
            clear: [0.1, 0.2, 0.3],
        })
        .expect("a valid description");

    let relay = WakeRelay::default();
    let wakes = ServiceWakes {
        jobs: None,
        uploads: Some(relay.wake()),
        retirements: Some(relay.wake()),
    };
    let service = ResourceService::with_assets(
        config.service,
        Mode::Backend,
        wakes,
        config.assets.map(Assets::new),
    )
    .expect("a backend-mode service");
    let bridge = SceneBridge::with_limits(
        Filter::Nearest,
        ck_render_2d::SceneLimits {
            max_quads: config.scene_max_quads,
        },
    )
    .expect("valid scene limits");
    let scene_writer = bridge.take_writer().expect("the one writer");
    let pool = LoaderPool::start(service.clone()).expect("the loader pool starts");

    let context = MetalContext::new().expect("a Metal device");
    let observer = Arc::new(Observer::new());
    let presenter_hooks = TestHooks {
        observer: Some(Arc::clone(&observer)),
        capture_frames: config.capture,
        ..config.presenter_hooks
    };
    let wake = {
        let notifier = Arc::clone(&notifier);
        Arc::new(move || notifier.wake()) as Arc<dyn Fn() + Send + Sync>
    };
    // SAFETY: `ck_window::run` below keeps the window (and so its layer) until the presenter's end is proven, which is the
    // contract of `MetalBackend::with_resources`; the backend is used as nothing but that presenter.
    let backend = unsafe {
        MetalBackend::with_resources_and_hooks(
            wake,
            service.clone(),
            relay.clone(),
            Arc::clone(&context),
            config.backend,
            BackendTestHooks {
                presenter: presenter_hooks,
                checker: None,
                textures: None,
                before_exchange: None,
                ordinary_baseline: config.ordinary,
            },
        )
    };
    let control = Arc::new(Control::default());
    let seen = Arc::new(Seen::default());
    let renderer = Renderer::new(
        Wrapped {
            inner: backend,
            control: Arc::clone(&control),
            seen: Arc::clone(&seen),
        },
        Arc::clone(&bridge),
    );
    let opened = Arc::new(AtomicBool::new(false));
    let close_requests = Arc::new(AtomicUsize::new(0));
    let ui = Ui {
        opened: Arc::clone(&opened),
        close_requests: Arc::clone(&close_requests),
        answers: Vec::new(),
    };
    let mut scene = Scene {
        service: service.clone(),
        writer: scene_writer,
        requests: writer,
        control: Arc::clone(&control),
        notifier: Arc::clone(&notifier),
        observer: Arc::clone(&observer),
        seen: Arc::clone(&seen),
        opened,
    };
    let scene_thread = thread::Builder::new()
        .name("scale-scene".to_owned())
        .spawn(move || {
            let result = phase(&mut scene);
            // The phase is over, whatever it came to: the window closes and the loop's teardown proves the presenter's end. The
            // scene's quads (and their texture references) go with `scene`, on this thread, before the loop's end is awaited.
            scene.close();
            drop(scene);
            result
        })
        .expect("the scene thread starts");

    let cocoa = match CocoaBackend::new() {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("worker: no window backend (a desktop session is required): {error}");
            std::process::exit(1);
        }
    };
    // SAFETY: main thread; installs one local event monitor, once, before the first window.
    unsafe { ck_r2_t_install_input_filter() };
    let (running, wakeups) = ticker(&notifier);
    let quit = AtomicBool::new(false);
    let loop_end = run(
        cocoa,
        renderer,
        ui,
        &reader,
        &notifier,
        &quit,
        &LoopSettings::default(),
    );
    running.store(false, Ordering::Release);
    let _ = wakeups.join();
    control.end();
    let result = scene_thread
        .join()
        .unwrap_or_else(|_| Err("the scene thread panicked".to_owned()));

    let mut problems = Vec::new();
    if !matches!(loop_end, LoopEnd::Closed) {
        problems.push(format!("the window loop ended {loop_end:?}, not Closed"));
    }
    if close_requests.load(Ordering::SeqCst) != 0 {
        problems.push(format!(
            "{} close request(s) reached the UI (the keyboard filter should drop them)",
            close_requests.load(Ordering::SeqCst)
        ));
    }
    // SAFETY: reads an atomic counter.
    let dropped_keys = unsafe { ck_r2_t_dropped_keys() };
    // The end: every CPU holder is gone before a resource count is claimed zero. The scene writer and the backend's slot went with
    // the loop and the scene thread; the bridge is detached; what is left is this function's own handles.
    drop(bridge);
    let mut pool = pool;
    let shutdown = pool.shutdown(Duration::from_secs(20));
    if !shutdown.unfinished.is_empty() {
        problems.push(format!("loader threads did not finish: {shutdown:?}"));
    }
    drop(pool);
    let _ = service.finish_teardown();
    let stats = service.stats();
    if stats.jobs.reserved != 0 || stats.cpu_pixels.reserved != 0 || stats.metadata.reserved != 0 {
        problems.push(format!("a budget is not zero after the end: {stats:?}"));
    }
    if stats.registry.live != 0 || stats.registry.retiring != 0 {
        problems.push(format!(
            "a texture is still live after the end: {:?}",
            stats.registry
        ));
    }
    if stats.loaders.referenced_at_teardown != 0 {
        problems.push(format!(
            "{} resource(s) were still referenced at teardown",
            stats.loaders.referenced_at_teardown
        ));
    }
    let resources = J::o([
        ("live_at_end", J::U(u64::from(stats.registry.live))),
        ("retiring_at_end", J::U(u64::from(stats.registry.retiring))),
        ("cpu_pixel_bytes_peak", J::U(stats.cpu_pixels.peak)),
        ("loader_files_decoded", J::U(stats.loaders.files_decoded)),
        ("loader_decoded_bytes", J::U(stats.loaders.decoded_bytes)),
        ("service", evidence::service_stats(&stats)),
        ("keys_dropped_by_the_filter", J::U(dropped_keys)),
    ]);
    let ended = Ended {
        loop_end,
        problems,
        stats: lock(&seen.last_stats).take(),
        totals: lock(&seen.last_totals).take(),
        resources,
    };
    (result, ended)
}

/// Makes the process exit with the right status after printing the worker's result line.
pub(crate) fn finish(result: &Result<J, String>, ended: &Ended) -> ExitCode {
    let mut problems = ended.problems.clone();
    if let Err(error) = result {
        problems.push(format!("the phase failed: {error}"));
    }
    let mut record = match result {
        Ok(json) => json.clone(),
        Err(_) => J::o([]),
    };
    record.push(
        "end",
        J::o([
            ("loop_end", J::s(format!("{:?}", ended.loop_end))),
            ("resources", ended.resources.clone()),
            (
                "last_scene_stats",
                ended.stats.as_ref().map_or(J::Null, evidence::scene_stats),
            ),
            (
                "last_window_totals",
                ended.totals.as_ref().map_or(J::Null, evidence::frame_stats),
            ),
            (
                "problems",
                J::A(problems.iter().cloned().map(J::S).collect()),
            ),
        ]),
    );
    println!("SCALE_RESULT {}", record.render());
    let passed = problems.is_empty() && record.get("passed") == Some(&J::Bool(true));
    println!("SCALE_STATUS passed={passed}");
    if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        for problem in &problems {
            eprintln!("worker: {problem}");
        }
        ExitCode::FAILURE
    }
}
