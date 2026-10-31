//! The fakes. They keep the contracts of the real adapters:
//!
//! - a native wake is **sticky**: it leaves a queued event, so a `pump(UntilEvent)` entered after it
//!   returns at once. The pump blocks (bounded) only when nothing is queued, and says so in the log;
//! - a **capped** pump stops after `cap` scripted events; a **draining** pump delivers every queued event
//!   in one call, however many;
//! - `destroy_window` and `terminate` fail the test if called from inside `pump`;
//! - dropping a fake backend or presenter is logged, so a test can prove nothing was released.
//!
//! All the fakes share one call log (`Log`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ck_window::{
    Answer, Backend, CleanupResult, Clock, CreatedWindow, LoopEnd, LoopSettings, NativeEvent,
    NativeWake, Notifier, PixelSize, PostError, Presenter, PresenterReport, RequestWriter,
    Requests, UiAnswer, UiEvent, UiLink, Wait, WindowDesc, run_with_clock,
};

/// The longest a fake waits for something that must happen, and the longest a test runs.
pub(crate) const BOUND: Duration = Duration::from_secs(10);

pub(crate) type Log = Arc<Mutex<Vec<String>>>;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn note(log: &Log, what: impl Into<String>) {
    lock(log).push(what.into());
}

/// The deadline of a test that drives the loop: generous, because a passing test takes milliseconds.
pub(crate) const DEADLINE: Duration = Duration::from_secs(25);
/// The deadline of a test whose failure mode is a deadlock: a failure must show up quickly.
pub(crate) const SHORT: Duration = Duration::from_secs(5);

/// The environment variable that tells a re-executed test binary which test body to run.
const OWNED: &str = "CK_WINDOW_OWNED_TEST";

/// Runs `body` in an **owned child process**: the test binary re-executes itself for this one test, owned
/// from the moment it is spawned. If the child does not exit within `deadline`, the parent kills and
/// reaps it and fails the test; if the parent fails or panics while waiting, the child is killed and
/// reaped too. So a body that deadlocks or loops without end cannot outlive its test, hang the binary or
/// leave a thread running into the next test, and nothing waits on the code under test.
///
/// In the child (`CK_WINDOW_OWNED_TEST` names this test) the body runs directly.
pub(crate) fn owned(module: &str, name: &str, deadline: Duration, body: impl FnOnce()) {
    let path = format!(
        "{}::{name}",
        module.strip_prefix("window::").unwrap_or(module)
    );
    if std::env::var(OWNED).as_deref() == Ok(path.as_str()) {
        body();
        return;
    }
    let exe = std::env::current_exe().expect("the test binary's path");
    let child = std::process::Command::new(exe)
        .args(["--exact", &path, "--nocapture"])
        .env(OWNED, &path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the test binary re-executes itself");
    let mut child = Owned(Some(child));
    let (out, err) = child.drain();
    let begun = Instant::now();
    let status = loop {
        match child.try_wait() {
            Some(status) => break Some(status),
            None if begun.elapsed() >= deadline => break None,
            None => thread::sleep(Duration::from_millis(2)),
        }
    };
    let Some(status) = status else {
        child.kill_and_reap();
        let (stdout, stderr) = (
            out.join().unwrap_or_default(),
            err.join().unwrap_or_default(),
        );
        panic!(
            "test `{path}` did not finish within {deadline:?}: its process was killed and reaped\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
        );
    };
    let (stdout, stderr) = (
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
    );
    assert!(
        status.success() && stdout.contains("1 passed"),
        "test `{path}` failed in its child process ({status})\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
    );
}

/// Reads a pipe to its end on a thread of its own; the pipe closes when the child is reaped.
fn read_all(mut pipe: impl std::io::Read + Send + 'static) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut text = String::new();
        let _ = pipe.read_to_string(&mut text);
        text
    })
}

/// The child process of an owned test. Dropping it kills and reaps it, on every path.
struct Owned(Option<std::process::Child>);

impl Owned {
    /// Starts a reader thread for each pipe (so a chatty child can never block on a full pipe).
    fn drain(&mut self) -> (thread::JoinHandle<String>, thread::JoinHandle<String>) {
        let child = self.0.as_mut().expect("the child is running");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        (read_all(stdout), read_all(stderr))
    }

    fn try_wait(&mut self) -> Option<std::process::ExitStatus> {
        self.0
            .as_mut()
            .expect("the child is running")
            .try_wait()
            .expect("the child can be waited for")
    }

    fn kill_and_reap(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        self.kill_and_reap();
    }
}

// ---------------------------------------------------------------------------------------------
// The logger and the scenario lock
// ---------------------------------------------------------------------------------------------

struct Sink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The records written so far, as lines, after a bounded flush that must be complete.
fn written() -> &'static Arc<Mutex<Vec<u8>>> {
    static BUFFER: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    BUFFER.get_or_init(|| {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let logger = ck_log::Logger::new(
            Box::new(Sink(Arc::clone(&buffer))),
            ck_log::Settings {
                filter: ck_log::Filter::new(ck_log::Level::Trace),
                timestamps: false,
                colour: false,
            },
        )
        .expect("the writer thread starts");
        ck_log::install(logger).expect("no logger is installed yet in this process");
        buffer
    })
}

/// The scenario lock and a reader of the records the scenario writes (by offset).
pub(crate) struct Scenario {
    _serial: MutexGuard<'static, ()>,
    taken: usize,
}

pub(crate) fn scenario() -> Scenario {
    static SERIAL: Mutex<()> = Mutex::new(());
    let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = written();
    let taken = records_so_far().len();
    Scenario {
        _serial: serial,
        taken,
    }
}

fn records_so_far() -> Vec<String> {
    let logger = ck_log::installed().expect("the logger is installed");
    assert_eq!(
        logger.flush(BOUND),
        ck_log::FlushOutcome::Complete,
        "the logger must write every record"
    );
    let text = String::from_utf8(lock(written()).clone()).expect("log output is UTF-8");
    text.lines().map(str::to_owned).collect()
}

impl Scenario {
    /// The records written since the scenario began.
    pub(crate) fn records(&self) -> Vec<String> {
        records_so_far()[self.taken..].to_vec()
    }
}

// ---------------------------------------------------------------------------------------------
// The sticky native wake
// ---------------------------------------------------------------------------------------------

/// A gate a test opens: native wakes that reach it block until then (bounded).
#[derive(Default)]
pub(crate) struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
    entered: AtomicUsize,
}

impl Gate {
    pub(crate) fn release(&self) {
        *lock(&self.open) = true;
        self.changed.notify_all();
    }

    fn pass(&self) {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let give_up = Instant::now() + BOUND;
        let mut open = lock(&self.open);
        while !*open {
            let left = give_up.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            open = self
                .changed
                .wait_timeout(open, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Waits (bounded) until `n` wakes have reached the gate.
    pub(crate) fn wait_entered(&self, n: usize) {
        let give_up = Instant::now() + BOUND;
        while self.entered.load(Ordering::SeqCst) < n {
            assert!(Instant::now() < give_up, "no native wake reached the gate");
            thread::sleep(Duration::from_millis(1));
        }
    }
}

/// The queue the fake backend sleeps on: scripted events pushed by the test, and the sticky wake.
#[derive(Default)]
pub(crate) struct Native {
    state: Mutex<NativeState>,
    changed: Condvar,
    /// How many native wakes were called.
    pub(crate) wakes: AtomicUsize,
    gate: Mutex<Option<Arc<Gate>>>,
    /// The number of `pump(UntilEvent)` calls that went to sleep, and a signal for each.
    sleeping: Mutex<usize>,
    slept: Condvar,
}

#[derive(Default)]
struct NativeState {
    /// A wake is queued (sticky).
    queued_wake: bool,
    /// Events pushed by a test while the loop runs.
    pushed: VecDeque<NativeEvent>,
}

impl Native {
    /// Makes a wake that reaches the gate block until the gate is released.
    pub(crate) fn gate_wakes(&self, gate: &Arc<Gate>) {
        *lock(&self.gate) = Some(Arc::clone(gate));
    }

    /// Waits (bounded) until the pump has gone to sleep `n` times in all.
    pub(crate) fn wait_sleeping(&self, n: usize) {
        let give_up = Instant::now() + BOUND;
        let mut sleeping = lock(&self.sleeping);
        while *sleeping < n {
            let left = give_up.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the pump never went to sleep");
            sleeping = self
                .slept
                .wait_timeout(sleeping, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl NativeWake for Native {
    fn wake(&self) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
        let gate = lock(&self.gate).clone();
        if let Some(gate) = gate {
            gate.pass();
        }
        lock(&self.state).queued_wake = true;
        self.changed.notify_all();
    }
}

// ---------------------------------------------------------------------------------------------
// The backend
// ---------------------------------------------------------------------------------------------

/// What the backend's pump does at its entry, to let a test act "while the pump holds control".
pub(crate) type PumpHook = Box<dyn FnMut(&PumpCall)>;

/// A `pump` call as the loop made it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PumpCall {
    /// Which pump this is, from 1.
    pub(crate) index: usize,
    pub(crate) wait: Wait,
    pub(crate) cap: usize,
    /// A timed wait's absolute monotonic deadline.
    pub(crate) deadline: Option<Instant>,
}

pub(crate) struct FakeBackend {
    pub(crate) log: Log,
    pub(crate) native: Arc<Native>,
    /// Scripted batches: one is delivered per pump (a capped pump leaves the rest at the front).
    pub(crate) script: VecDeque<Vec<NativeEvent>>,
    /// A draining backend delivers the whole batch whatever the cap.
    pub(crate) draining: bool,
    pub(crate) deadline_supported: bool,
    pub(crate) deadline_hook: Option<Box<dyn FnMut(Instant)>>,
    pub(crate) create: Result<(), String>,
    pub(crate) hook: Option<PumpHook>,
    pub(crate) notifier: Option<Arc<Notifier>>,
    pub(crate) pumps: Arc<Mutex<Vec<PumpCall>>>,
    /// How many events each pump delivered.
    pub(crate) delivered: Arc<Mutex<Vec<usize>>>,
    in_pump: Arc<AtomicBool>,
    /// A `pump(UntilEvent)` that waited the whole bound without any event or wake.
    pub(crate) timed_out: Arc<AtomicBool>,
    pub(crate) created_with: Arc<Mutex<Option<WindowDesc>>>,
}

impl FakeBackend {
    pub(crate) fn new(log: &Log) -> Self {
        Self {
            log: Arc::clone(log),
            native: Arc::new(Native::default()),
            script: VecDeque::new(),
            draining: false,
            deadline_supported: false,
            deadline_hook: None,
            create: Ok(()),
            hook: None,
            notifier: None,
            pumps: Arc::new(Mutex::new(Vec::new())),
            delivered: Arc::new(Mutex::new(Vec::new())),
            in_pump: Arc::new(AtomicBool::new(false)),
            timed_out: Arc::new(AtomicBool::new(false)),
            created_with: Arc::new(Mutex::new(None)),
        }
    }

    fn teardown_call(&self, what: &str) {
        assert!(
            !self.in_pump.load(Ordering::SeqCst),
            "{what} was called from inside pump"
        );
        let state = self.notifier.as_ref().map_or_else(String::new, |notifier| {
            format!(" notifier={:?}", notifier.state())
        });
        note(&self.log, format!("backend.{what}{state}"));
    }
}

impl Drop for FakeBackend {
    fn drop(&mut self) {
        note(&self.log, "drop backend");
    }
}

impl Backend for FakeBackend {
    type Handles = u32;

    fn create_window(&mut self, desc: &WindowDesc) -> Result<CreatedWindow<u32>, String> {
        note(&self.log, "backend.create_window");
        *lock(&self.created_with) = Some(desc.clone());
        match &self.create {
            Ok(()) => Ok(CreatedWindow {
                handles: 7,
                pixel_size: PixelSize {
                    width: desc.width * 2,
                    height: desc.height * 2,
                },
            }),
            Err(message) => Err(message.clone()),
        }
    }

    fn wake_target(&self) -> Arc<dyn NativeWake> {
        note(&self.log, "backend.wake_target");
        Arc::clone(&self.native) as Arc<dyn NativeWake>
    }

    fn pump(&mut self, events: &mut dyn FnMut(NativeEvent), wait: Wait, cap: usize) {
        self.pump_inner(events, wait, cap, None);
    }

    fn supports_deadline(&self) -> bool {
        self.deadline_supported
    }

    fn pump_until(&mut self, events: &mut dyn FnMut(NativeEvent), deadline: Instant, cap: usize) {
        assert!(
            self.deadline_supported,
            "timed wait used without capability"
        );
        if let Some(hook) = &mut self.deadline_hook {
            hook(deadline);
        }
        self.pump_inner(events, Wait::UntilEvent, cap, Some(deadline));
    }

    fn destroy_window(&mut self) {
        self.teardown_call("destroy_window");
    }

    fn terminate(&mut self) {
        self.teardown_call("terminate");
    }
}

impl FakeBackend {
    fn pump_inner(
        &mut self,
        events: &mut dyn FnMut(NativeEvent),
        wait: Wait,
        cap: usize,
        deadline: Option<Instant>,
    ) {
        let index = {
            let mut pumps = lock(&self.pumps);
            let index = pumps.len() + 1;
            pumps.push(PumpCall {
                index,
                wait,
                cap,
                deadline,
            });
            index
        };
        self.in_pump.store(true, Ordering::SeqCst);
        note(&self.log, format!("backend.pump enter #{index} {wait:?}"));
        if let Some(hook) = &mut self.hook {
            hook(&PumpCall {
                index,
                wait,
                cap,
                deadline,
            });
        }
        let mut delivered = 0;
        // Scripted events first: a batch per pump, capped unless the backend drains.
        if let Some(batch) = self.script.pop_front() {
            let take = if self.draining {
                batch.len()
            } else {
                batch.len().min(cap)
            };
            let (now, later) = batch.split_at(take);
            for event in now {
                events(*event);
                delivered += 1;
            }
            if !later.is_empty() {
                self.script.push_front(later.to_vec());
            }
        }
        // Events a test pushed, under the same rule.
        loop {
            let next = {
                let mut state = lock(&self.native.state);
                if !self.draining && delivered >= cap {
                    None
                } else {
                    state.pushed.pop_front()
                }
            };
            let Some(event) = next else { break };
            events(event);
            delivered += 1;
        }
        if wait == Wait::UntilEvent && delivered == 0 {
            if deadline.is_some() {
                let mut state = lock(&self.native.state);
                if state.queued_wake {
                    state.queued_wake = false;
                } else if let Some(event) = state.pushed.pop_front() {
                    drop(state);
                    events(event);
                    delivered += 1;
                }
            } else {
                self.sleep_until_event_or_wake(events, &mut delivered);
            }
        }
        lock(&self.delivered).push(delivered);
        note(&self.log, format!("backend.pump exit #{index}"));
        self.in_pump.store(false, Ordering::SeqCst);
    }

    /// `pump(UntilEvent)` with nothing delivered: returns at once if a wake is queued (sticky), else sleeps
    /// until an event or a wake, for at most [`BOUND`].
    fn sleep_until_event_or_wake(
        &self,
        events: &mut dyn FnMut(NativeEvent),
        delivered: &mut usize,
    ) {
        let give_up = Instant::now() + BOUND;
        let mut announced = false;
        let mut state = lock(&self.native.state);
        loop {
            if state.queued_wake {
                state.queued_wake = false;
                return;
            }
            if let Some(event) = state.pushed.pop_front() {
                drop(state);
                events(event);
                *delivered += 1;
                return;
            }
            if !announced {
                announced = true;
                note(&self.log, "backend.pump sleeping");
                *lock(&self.native.sleeping) += 1;
                self.native.slept.notify_all();
            }
            let left = give_up.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.timed_out.store(true, Ordering::SeqCst);
                return;
            }
            state = self
                .native
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The presenter
// ---------------------------------------------------------------------------------------------

/// How the fake render thread ends.
#[derive(Clone)]
pub(crate) enum End {
    /// `poll_end` says this at once.
    At(CleanupResult),
    /// `poll_end` never observes the thread finished.
    Never,
}

pub(crate) struct FakePresenter {
    pub(crate) log: Log,
    pub(crate) start: Result<(), String>,
    pub(crate) reports: Arc<Mutex<VecDeque<PresenterReport>>>,
    pub(crate) end: End,
    pub(crate) taken: Arc<AtomicUsize>,
    pub(crate) notifier: Option<Arc<Notifier>>,
    /// Called inside `start`, to let a test act at that point.
    pub(crate) on_start: Option<Box<dyn FnOnce()>>,
    /// Called inside `request_stop`, to let a test act at the moment the application's presenter takes its end timestamp.
    pub(crate) on_stop: Option<Box<dyn FnOnce()>>,
    /// Panics in `take_report` the n-th time it is called (from 1).
    pub(crate) panic_on_take: Option<usize>,
    /// Reports the render thread publishes while it stops: queued by `request_stop`.
    pub(crate) at_stop: Vec<PresenterReport>,
    /// Reports it publishes inside the `poll_end` call that first runs, before that call answers (so, for
    /// `End::At`, in the very call that first returns finished).
    pub(crate) at_end: Vec<PresenterReport>,
    /// Once a stop was requested and the queue is empty, `take_report` hands out a first-frame report
    /// every time: a malformed presenter whose queue never ends.
    pub(crate) endless_after_stop: bool,
    pub(crate) started_with: Arc<Mutex<Option<(u32, WindowDesc, PixelSize)>>>,
    stopped: bool,
}

impl FakePresenter {
    pub(crate) fn new(log: &Log) -> Self {
        Self {
            log: Arc::clone(log),
            start: Ok(()),
            reports: Arc::new(Mutex::new(VecDeque::new())),
            end: End::At(CleanupResult::Proven),
            taken: Arc::new(AtomicUsize::new(0)),
            notifier: None,
            on_start: None,
            on_stop: None,
            panic_on_take: None,
            at_stop: Vec::new(),
            at_end: Vec::new(),
            endless_after_stop: false,
            started_with: Arc::new(Mutex::new(None)),
            stopped: false,
        }
    }
}

impl Drop for FakePresenter {
    fn drop(&mut self) {
        note(&self.log, "drop presenter");
    }
}

impl Presenter for FakePresenter {
    type Handles = u32;

    fn start(
        &mut self,
        handles: u32,
        desc: &WindowDesc,
        pixel_size: PixelSize,
    ) -> Result<(), String> {
        note(&self.log, "presenter.start");
        *lock(&self.started_with) = Some((handles, desc.clone(), pixel_size));
        if let Some(on_start) = self.on_start.take() {
            on_start();
        }
        self.start.clone()
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        note(&self.log, format!("presenter.set_clear {rgb:?}"));
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        note(
            &self.log,
            format!("presenter.set_pixel_size {width}x{height}"),
        );
    }

    fn request_redraw(&mut self) {
        note(&self.log, "presenter.request_redraw");
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        let call = self.taken.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(self.panic_on_take != Some(call), "injected presenter panic");
        let mut report = lock(&self.reports).pop_front();
        if report.is_none() && self.endless_after_stop && self.stopped {
            report = Some(PresenterReport::FirstFrameQueued);
        }
        if report.is_none() {
            // An empty look is not a "take"; the counter is of reports handed out.
            self.taken.fetch_sub(1, Ordering::SeqCst);
        } else {
            note(&self.log, "presenter.report");
        }
        report
    }

    fn request_stop(&mut self) {
        note(&self.log, "presenter.request_stop");
        if let Some(act) = self.on_stop.take() {
            act();
        }
        self.stopped = true;
        lock(&self.reports).extend(self.at_stop.drain(..));
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        note(&self.log, "presenter.poll_end");
        lock(&self.reports).extend(self.at_end.drain(..));
        match &self.end {
            End::At(result) => Some(result.clone()),
            End::Never => {
                thread::sleep(within.min(Duration::from_millis(2)));
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The UI link
// ---------------------------------------------------------------------------------------------

/// Shared with the test, so it can read and steer the link while the loop owns it.
pub(crate) struct UiHandle {
    /// Every event accepted by `post`, in order.
    pub(crate) posts: Mutex<Vec<UiEvent>>,
    /// While set, every `post` is answered `Full`: the UI VM is busy and has no slot free. It frees one
    /// when it produces a result, which a test models with [`UiHandle::release_capacity`].
    pub(crate) blocked: AtomicBool,
    pub(crate) post_attempts: AtomicUsize,
    /// Number of update posts that should be refused before capacity is available.
    pub(crate) full_update_posts: AtomicUsize,
    /// Number of close posts that should be refused before capacity is available.
    pub(crate) full_close_posts: AtomicUsize,
    pub(crate) disconnected: AtomicBool,
    pub(crate) disconnect_on_post: AtomicBool,
    pub(crate) answers: Mutex<VecDeque<UiAnswer>>,
    /// Answers to give, in order, to each `CloseRequested` that is posted.
    pub(crate) close_answers: Mutex<VecDeque<Answer>>,
    /// Answers to give, in order, to each paced update that is posted.
    pub(crate) update_answers: Mutex<VecDeque<Answer>>,
    pub(crate) answers_taken: AtomicUsize,
    notifier: Arc<Notifier>,
    log: Log,
}

impl UiHandle {
    pub(crate) fn new(notifier: &Arc<Notifier>, log: &Log) -> Arc<Self> {
        Arc::new(Self {
            posts: Mutex::new(Vec::new()),
            blocked: AtomicBool::new(false),
            post_attempts: AtomicUsize::new(0),
            full_update_posts: AtomicUsize::new(0),
            full_close_posts: AtomicUsize::new(0),
            disconnected: AtomicBool::new(false),
            disconnect_on_post: AtomicBool::new(false),
            answers: Mutex::new(VecDeque::new()),
            close_answers: Mutex::new(VecDeque::new()),
            update_answers: Mutex::new(VecDeque::new()),
            answers_taken: AtomicUsize::new(0),
            notifier: Arc::clone(notifier),
            log: Arc::clone(log),
        })
    }

    /// Queues an answer and wakes the loop, as the UI link's result wake does.
    pub(crate) fn answer(&self, event: UiEvent, answer: Answer) {
        lock(&self.answers).push_back(UiAnswer { event, answer });
        self.notifier.wake();
    }

    /// The UI VM produced a result: a slot is free, and the result wake reaches the notifier.
    pub(crate) fn release_capacity(&self) {
        self.blocked.store(false, Ordering::SeqCst);
        self.notifier.wake();
    }

    pub(crate) fn posted(&self) -> Vec<UiEvent> {
        lock(&self.posts).clone()
    }
}

pub(crate) struct FakeUi(pub(crate) Arc<UiHandle>);

impl UiLink for FakeUi {
    fn post(&mut self, event: UiEvent) -> Result<(), PostError> {
        let ui = &self.0;
        ui.post_attempts.fetch_add(1, Ordering::SeqCst);
        if ui.disconnect_on_post.load(Ordering::SeqCst) {
            return Err(PostError::Disconnected);
        }
        if ui.blocked.load(Ordering::SeqCst) {
            return Err(PostError::Full);
        }
        let scripted_full = match &event {
            UiEvent::CloseRequested { .. } => Some(&ui.full_close_posts),
            UiEvent::Update(_) => Some(&ui.full_update_posts),
            UiEvent::Opened => None,
        };
        if scripted_full.is_some_and(|remaining| {
            remaining
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok()
        }) {
            return Err(PostError::Full);
        }
        lock(&ui.posts).push(event.clone());
        note(&ui.log, format!("ui.post {event:?}"));
        let scripted = match &event {
            UiEvent::CloseRequested { .. } => lock(&ui.close_answers).pop_front(),
            UiEvent::Update(_) => lock(&ui.update_answers).pop_front(),
            UiEvent::Opened => None,
        };
        if let Some(answer) = scripted {
            ui.answer(event, answer);
        }
        Ok(())
    }

    fn take_answer(&mut self) -> Option<UiAnswer> {
        let answer = lock(&self.0.answers).pop_front();
        if answer.is_some() {
            self.0.answers_taken.fetch_add(1, Ordering::SeqCst);
        }
        answer
    }

    fn is_disconnected(&self) -> bool {
        self.0.disconnected.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------------------------
// The rig
// ---------------------------------------------------------------------------------------------

/// A backend, a presenter and a UI link wired to one notifier, one slot and one quit flag.
pub(crate) struct Rig {
    pub(crate) log: Log,
    pub(crate) backend: FakeBackend,
    pub(crate) presenter: FakePresenter,
    pub(crate) ui: Arc<UiHandle>,
    pub(crate) requests: Requests,
    pub(crate) writer: RequestWriter,
    pub(crate) notifier: Arc<Notifier>,
    pub(crate) quit: Arc<AtomicBool>,
    pub(crate) settings: LoopSettings,
}

impl Rig {
    /// A rig whose first request batch opens a 400 by 300 window.
    pub(crate) fn new() -> Self {
        let rig = Self::empty();
        rig.writer.open(desc()).expect("a valid description");
        rig
    }

    /// A rig with nothing requested yet.
    pub(crate) fn empty() -> Self {
        let log: Log = Arc::new(Mutex::new(Vec::new()));
        let notifier = Notifier::new();
        let requests = Requests::new();
        let writer = requests.writer(Arc::clone(&notifier));
        let mut backend = FakeBackend::new(&log);
        backend.notifier = Some(Arc::clone(&notifier));
        let mut presenter = FakePresenter::new(&log);
        presenter.notifier = Some(Arc::clone(&notifier));
        Self {
            ui: UiHandle::new(&notifier, &log),
            log,
            backend,
            presenter,
            requests,
            writer,
            notifier,
            quit: Arc::new(AtomicBool::new(false)),
            settings: LoopSettings::default(),
        }
    }

    /// Makes pump `index` set the quit flag and wake the loop, as the signal thread does.
    pub(crate) fn quit_at_pump(&mut self, index: usize) {
        let quit = Arc::clone(&self.quit);
        let notifier = Arc::clone(&self.notifier);
        self.backend.hook = Some(Box::new(move |call| {
            if call.index == index {
                quit.store(true, Ordering::SeqCst);
                notifier.wake();
            }
        }));
    }

    /// Makes the first pump close the window through the request slot.
    pub(crate) fn close_at_pump(&mut self, index: usize) {
        let writer = self.writer.clone();
        self.backend.hook = Some(Box::new(move |call| {
            if call.index == index {
                writer.close();
            }
        }));
    }
}

/// The 400 by 300 description `Rig::new` opens.
pub(crate) fn desc() -> WindowDesc {
    WindowDesc {
        title: "test".to_owned(),
        width: 400,
        height: 300,
        clear: [0.1, 0.2, 0.3],
    }
}

/// A colour as bit patterns, to compare exactly.
pub(crate) fn bits(rgb: [f32; 3]) -> [u32; 3] {
    rgb.map(f32::to_bits)
}

/// The call log so far.
pub(crate) fn calls(log: &Log) -> Vec<String> {
    lock(log).clone()
}

/// The position of the first log line starting with `prefix`; the test fails if there is none.
pub(crate) fn at(log: &[String], prefix: &str) -> usize {
    log.iter()
        .position(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no log line starts with `{prefix}` in {log:#?}"))
}

/// What a run came to, with the facts a test usually wants.
pub(crate) struct Ran {
    pub(crate) end: LoopEnd,
    pub(crate) log: Vec<String>,
    pub(crate) posts: Vec<UiEvent>,
    pub(crate) waits: Vec<Wait>,
    pub(crate) deadlines: Vec<Option<Instant>>,
    pub(crate) delivered: Vec<usize>,
}

impl Rig {
    /// Runs the loop and collects what happened.
    pub(crate) fn run_collect(self) -> Ran {
        self.run_collect_with_clock(&SystemClockForTests)
    }

    pub(crate) fn run_collect_with_clock<C: Clock>(self, clock: &C) -> Ran {
        let log = Arc::clone(&self.log);
        let ui = Arc::clone(&self.ui);
        let pumps = Arc::clone(&self.backend.pumps);
        let delivered = Arc::clone(&self.backend.delivered);
        let timed_out = Arc::clone(&self.backend.timed_out);
        let reader = self.requests.reader();
        let end = run_with_clock(
            self.backend,
            self.presenter,
            FakeUi(Arc::clone(&self.ui)),
            &reader,
            &self.notifier,
            &self.quit,
            &self.settings,
            clock,
        );
        assert!(
            !timed_out.load(Ordering::SeqCst),
            "a pump(UntilEvent) slept the whole bound: the loop neither polled nor was woken"
        );
        let pump_calls = lock(&pumps).clone();
        let waits = pump_calls.iter().map(|pump| pump.wait).collect();
        let deadlines = pump_calls.iter().map(|pump| pump.deadline).collect();
        Ran {
            end,
            log: calls(&log),
            posts: ui.posted(),
            waits,
            deadlines,
            delivered: lock(&delivered).clone(),
        }
    }
}

struct SystemClockForTests;

impl Clock for SystemClockForTests {
    fn now(&self) -> Instant {
        Instant::now()
    }
}
