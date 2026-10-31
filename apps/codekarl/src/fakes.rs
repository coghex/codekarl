//! Test doubles for the window backend and the presenter (test builds only; none of this is in the
//! production binary). They model the contracts `ck-window` documents: a sticky wake, a pump that waits
//! only when told to, a presenter whose thread reports through a queue and ends with a scripted result.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ck_render_2d::{Gpu2d, QuadSink, Snapshot};
use ck_window::{
    Backend, CleanupResult, CreatedWindow, NativeEvent, NativeWake, Notifier, PixelSize, Presenter,
    PresenterReport, Wait, WindowDesc,
};

/// How the doubles tell the test what happened (a closure, so a child process can write marker files).
pub(crate) type Recorder = Arc<dyn Fn(&str) + Send + Sync>;

/// A recorder that appends to a vector the test keeps.
pub(crate) fn vector_recorder() -> (Recorder, Arc<Mutex<Vec<String>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    (
        Arc::new(move |event| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(event.to_owned());
        }),
        events,
    )
}

/// The fake native wake: sticky, like `postEvent:` and `glfwPostEmptyEvent`.
pub(crate) struct FakeNative {
    woken: Mutex<bool>,
    changed: Condvar,
    /// How many native wakes were made.
    pub(crate) wakes: Mutex<u64>,
}

impl FakeNative {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            woken: Mutex::new(false),
            changed: Condvar::new(),
            wakes: Mutex::new(0),
        })
    }

    /// Waits at most `bound` for a wake and consumes it; returns at once if one is already queued.
    fn wait(&self, bound: Duration) {
        let give_up = Instant::now() + bound;
        let mut woken = self.woken.lock().unwrap_or_else(PoisonError::into_inner);
        while !*woken {
            let left = give_up.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            woken = self
                .changed
                .wait_timeout(woken, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        *woken = false;
    }
}

impl NativeWake for FakeNative {
    fn wake(&self) {
        *self.wakes.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        *self.woken.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.changed.notify_all();
    }
}

/// How the fake backend behaves.
#[derive(Clone)]
pub(crate) struct BackendScript {
    /// `create_window` fails with this message (the backend cleaned up after itself).
    pub(crate) create_error: Option<String>,
    /// `pump` panics (a panic on the main thread during the run).
    pub(crate) panic_in_pump: bool,
    /// The longest a `pump(UntilEvent)` waits for a wake (a bound, so a missed wake fails a test).
    pub(crate) wait_bound: Duration,
}

impl Default for BackendScript {
    fn default() -> Self {
        Self {
            create_error: None,
            panic_in_pump: false,
            wait_bound: Duration::from_secs(5),
        }
    }
}

/// A window backend that records what the loop does to it.
pub(crate) struct FakeBackend {
    pub(crate) native: Arc<FakeNative>,
    pub(crate) script: BackendScript,
    pub(crate) record: Recorder,
    /// Native events the next pump reports.
    pub(crate) pending: Arc<Mutex<VecDeque<NativeEvent>>>,
    /// A hook run at the start of every pump (for example to cancel a VM at a chosen moment).
    pub(crate) on_pump: Option<Box<dyn FnMut(u32)>>,
    /// A hook run when the window is destroyed.
    pub(crate) on_destroy: Option<Box<dyn FnMut()>>,
    pumps: u32,
}

impl FakeBackend {
    pub(crate) fn new(script: BackendScript, record: Recorder) -> Self {
        Self {
            native: FakeNative::new(),
            script,
            record,
            pending: Arc::new(Mutex::new(VecDeque::new())),
            on_pump: None,
            on_destroy: None,
            pumps: 0,
        }
    }
}

impl Drop for FakeBackend {
    fn drop(&mut self) {
        (self.record)("backend dropped");
    }
}

impl Backend for FakeBackend {
    type Handles = ();

    fn create_window(&mut self, desc: &WindowDesc) -> Result<CreatedWindow<()>, String> {
        (self.record)("create_window");
        if let Some(message) = &self.script.create_error {
            return Err(message.clone());
        }
        Ok(CreatedWindow {
            handles: (),
            pixel_size: PixelSize {
                width: desc.width * 2,
                height: desc.height * 2,
            },
        })
    }

    fn wake_target(&self) -> Arc<dyn NativeWake> {
        Arc::clone(&self.native) as Arc<dyn NativeWake>
    }

    fn pump(&mut self, events: &mut dyn FnMut(NativeEvent), wait: Wait, _cap: usize) {
        self.pumps += 1;
        if let Some(hook) = &mut self.on_pump {
            hook(self.pumps);
        }
        assert!(
            !self.script.panic_in_pump,
            "a forced panic in the main thread's pump"
        );
        if wait == Wait::UntilEvent {
            self.native.wait(self.script.wait_bound);
        }
        let taken: Vec<_> = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        for event in taken {
            events(event);
        }
    }

    /// The fake can wait until a deadline (as the native-metal backend can), so a loop with a paced update source runs.
    fn supports_deadline(&self) -> bool {
        true
    }

    fn pump_until(&mut self, events: &mut dyn FnMut(NativeEvent), deadline: Instant, _cap: usize) {
        self.pumps += 1;
        if let Some(hook) = &mut self.on_pump {
            hook(self.pumps);
        }
        assert!(
            !self.script.panic_in_pump,
            "a forced panic in the main thread's pump"
        );
        // Until the deadline or a wake, and never longer than the script's bound (a missed wake fails a test).
        self.native.wait(
            deadline
                .saturating_duration_since(Instant::now())
                .min(self.script.wait_bound),
        );
        let taken: Vec<_> = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        for event in taken {
            events(event);
        }
    }

    fn destroy_window(&mut self) {
        (self.record)("destroy_window");
        if let Some(hook) = &mut self.on_destroy {
            hook();
        }
    }

    fn terminate(&mut self) {
        (self.record)("terminate");
    }
}

/// How the fake render thread ends.
#[derive(Clone)]
pub(crate) enum End {
    /// An explicit success.
    Proven,
    /// A failed or missing cleanup result.
    Unproven(String),
    /// It never ends (a thread stuck in a native call): `poll_end` waits out its budget and says no.
    Never,
}

/// How the fake presenter behaves.
#[derive(Clone)]
pub(crate) struct PresenterScript {
    /// `start` fails (no thread, no GPU object).
    pub(crate) start_error: Option<String>,
    /// Report `FirstFrameQueued` as soon as it starts.
    pub(crate) first_frame: bool,
    /// Then report a failure.
    pub(crate) fail: Option<String>,
    /// How its end goes.
    pub(crate) end: End,
    /// Failures the render thread publishes while it stops: queued by `request_stop`, in order, and the
    /// end is still the scripted one (a presenter may report `Failed` and prove its cleanup).
    pub(crate) fail_at_stop: Vec<String>,
    /// Failures published inside the `poll_end` call that first runs, before it answers.
    pub(crate) fail_at_end: Vec<String>,
    /// A watcher thread reports `fail` as soon as this file exists (a failure that comes while the UI's
    /// callback is running), waking the notifier.
    pub(crate) fail_when_file: Option<std::path::PathBuf>,
    /// `start` spawns a thread (not a VM owner) that panics at once: the panic hook's rule for any thread
    /// that is not a VM owner, with the stand-in for the real presenter's `wake` callback.
    pub(crate) panic_on_other_thread: bool,
}

impl Default for PresenterScript {
    fn default() -> Self {
        Self {
            start_error: None,
            first_frame: true,
            fail: None,
            end: End::Proven,
            panic_on_other_thread: false,
            fail_when_file: None,
            fail_at_stop: Vec::new(),
            fail_at_end: Vec::new(),
        }
    }
}

/// A presenter that reports through a queue and wakes the notifier after each report, as the real one does.
pub(crate) struct FakePresenter {
    pub(crate) notifier: Arc<Notifier>,
    pub(crate) script: PresenterScript,
    pub(crate) record: Recorder,
    reports: Arc<Mutex<VecDeque<PresenterReport>>>,
    /// Set once `request_stop` was called.
    pub(crate) stopped: Arc<AtomicBool>,
}

impl FakePresenter {
    pub(crate) fn new(notifier: Arc<Notifier>, script: PresenterScript, record: Recorder) -> Self {
        Self {
            notifier,
            script,
            record,
            reports: Arc::new(Mutex::new(VecDeque::new())),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl FakePresenter {
    /// Queues a `Failed` report for each message and wakes the notifier, as the real presenter does.
    fn publish(&self, messages: &[String]) {
        if messages.is_empty() {
            return;
        }
        let mut reports = self.reports.lock().unwrap_or_else(PoisonError::into_inner);
        reports.extend(messages.iter().map(|message| PresenterReport::Failed {
            message: message.clone(),
        }));
        drop(reports);
        self.notifier.wake();
    }
}

impl Drop for FakePresenter {
    fn drop(&mut self) {
        (self.record)("presenter dropped");
    }
}

impl Presenter for FakePresenter {
    type Handles = ();

    fn start(&mut self, (): (), _desc: &WindowDesc, _pixel_size: PixelSize) -> Result<(), String> {
        (self.record)("presenter start");
        if let Some(message) = &self.script.start_error {
            return Err(message.clone());
        }
        let queue = |reports: &Mutex<VecDeque<PresenterReport>>, report| {
            reports
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(report);
        };
        if self.script.first_frame {
            queue(&self.reports, PresenterReport::FirstFrameQueued);
        }
        match (&self.script.fail, &self.script.fail_when_file) {
            (Some(message), None) => queue(
                &self.reports,
                PresenterReport::Failed {
                    message: message.clone(),
                },
            ),
            (Some(message), Some(file)) => {
                let (reports, notifier) = (Arc::clone(&self.reports), Arc::clone(&self.notifier));
                let (message, file) = (message.clone(), file.clone());
                std::thread::Builder::new()
                    .name("fake-presenter-watcher".to_owned())
                    .spawn(move || {
                        let give_up = Instant::now() + Duration::from_secs(30);
                        while !file.exists() && Instant::now() < give_up {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        reports
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push_back(PresenterReport::Failed { message });
                        notifier.wake();
                    })
                    .map_err(|error| error.to_string())?;
            }
            (None, _) => {}
        }
        self.notifier.wake();
        if self.script.panic_on_other_thread {
            let stand_in = std::thread::Builder::new()
                .name("fake-presenter-wake".to_owned())
                .spawn(|| panic!("a forced panic on a thread that is not a VM owner"))
                .map_err(|error| error.to_string())?;
            // The panic hook decides the process's fate before this returns; if it returns, so do we.
            let _ = stand_in.join();
        }
        Ok(())
    }

    fn set_clear(&mut self, _rgb: [f32; 3]) {}

    fn set_pixel_size(&mut self, _width: u32, _height: u32) {}

    fn request_redraw(&mut self) {}

    fn take_report(&mut self) -> Option<PresenterReport> {
        self.reports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
    }

    fn request_stop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        (self.record)("presenter stop");
        self.publish(&self.script.fail_at_stop);
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        let at_end = std::mem::take(&mut self.script.fail_at_end);
        self.publish(&at_end);
        match &self.script.end {
            End::Proven => Some(CleanupResult::Proven),
            End::Unproven(reason) => Some(CleanupResult::Unproven {
                reason: reason.clone(),
            }),
            End::Never => {
                std::thread::sleep(within);
                None
            }
        }
    }
}

/// A Metal-shaped `Gpu2d` over fake handles: its sink keeps the latest value (as the Metal slot does), it
/// reports a first frame and proves its end at once.
#[derive(Default)]
pub(crate) struct FakeGpu {
    latest: Arc<Mutex<Option<Snapshot>>>,
    reported: bool,
    /// Told `snapshot filter <name>` for every snapshot its sink is given (a child process leaves markers).
    record: Option<Recorder>,
}

struct Latest(Arc<Mutex<Option<Snapshot>>>, Option<Recorder>);

impl QuadSink for Latest {
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        if let Some(record) = &self.1 {
            record(&format!("snapshot filter {}", snapshot.filter));
        }
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(snapshot)
    }
}

impl FakeGpu {
    /// A fake that tells `record` about every snapshot its sink is given.
    pub(crate) fn recording(record: Recorder) -> Self {
        Self {
            record: Some(record),
            ..Self::default()
        }
    }
}

impl Gpu2d for FakeGpu {
    type Handles = ();

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        Arc::new(Latest(Arc::clone(&self.latest), self.record.clone()))
    }

    fn start(
        &mut self,
        (): (),
        _desc: &WindowDesc,
        _pixel_size: PixelSize,
        _sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        Ok(())
    }

    fn set_clear(&mut self, _rgb: [f32; 3]) {}

    fn set_pixel_size(&mut self, _width: u32, _height: u32) {}

    fn request_redraw(&mut self) {}

    fn take_report(&mut self) -> Option<PresenterReport> {
        (!std::mem::replace(&mut self.reported, true)).then_some(PresenterReport::FirstFrameQueued)
    }

    fn request_stop(&mut self) {}

    fn poll_end(&mut self, _within: Duration) -> Option<CleanupResult> {
        Some(CleanupResult::Proven)
    }
}
