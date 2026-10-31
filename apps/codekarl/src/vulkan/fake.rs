//! The fake stack of the `glfw-vulkan` path's tests (test builds only): every creation and release is recorded in
//! one ordered log, the quad side keeps every scene it is given (and can be told to refuse), and the presenter's
//! drawn generation and format notice are scripted. The same [`Run`](super::Run), backend and adapter the
//! production run uses run over it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use ck_render_2d::{QuadSink, Snapshot};
use ck_window::{CleanupResult, Notifier, PixelSize, Presenter, PresenterReport, WindowDesc};

use super::{Ended, QuadSide, Refused, Running, Stack};
use crate::fakes::{BackendScript, FakeBackend, FakePresenter, PresenterScript, Recorder};

/// How a wake regression is driven: the presenter's report, or the quad side's refusal, is raised from another
/// thread while the loop sleeps, and only the wake the adapter was given can wake the loop.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WakeCase {
    /// A `Failed` report, queued on the fake presenter and announced by the wake the stack's `start` was given.
    Report,
    /// A refusal recorded by the fake side, announced by the wake the side was built with.
    Refusal,
}

/// What the fake stack does.
#[derive(Clone, Default)]
pub(super) struct Script {
    pub(super) loader_error: Option<String>,
    pub(super) glfw_error: Option<String>,
    pub(super) window_error: Option<String>,
    pub(super) instance_error: Option<String>,
    pub(super) surface_error: Option<String>,
    /// The destroy helper refuses: whether the surface was destroyed before the refusal.
    pub(super) refuse: Option<bool>,
    /// After a refusal a thread panics 300 ms later, while the run is in the late-callback drain (the
    /// fixture's `on_opened` callback is held, so the drain lasts its whole bound).
    pub(super) panic_in_drain: bool,
    pub(super) presenter: PresenterScript,
    /// What the presenter says its drawn generation is.
    pub(super) drawn_generation: Option<u64>,
    /// A format notice the presenter has from the start.
    pub(super) notice: Option<String>,
    /// A format notice that becomes available inside the first `poll_end` call, before it answers.
    pub(super) notice_at_end: Option<String>,
    /// The fake side refuses every quad it is given.
    pub(super) refuse_quads: bool,
    /// A wake regression (see [`WakeCase`]).
    pub(super) wake_case: Option<WakeCase>,
}

/// The ordered log of everything the fake stack did.
pub(super) type Log = Recorder;

/// The run's loader; releasing it is logged.
pub(super) struct FakeLoader(Log);

impl Drop for FakeLoader {
    fn drop(&mut self) {
        (self.0)("loader dropped");
    }
}

/// The instance; releasing it (after the helper's `Ok`, or a drop) is logged.
pub(super) struct FakeInstance(Log);

impl Drop for FakeInstance {
    fn drop(&mut self) {
        (self.0)("instance dropped");
    }
}

/// A scene as the fake side recorded it: generation, whether a quad was requested, and the placed quad's
/// destination, source and filter name.
pub(super) type Scene = (u64, bool, Option<([i32; 4], [f32; 4], String)>);

/// The fake slot: the scenes the sink was given, in order.
#[derive(Default)]
pub(super) struct FakeSlot {
    pub(super) scenes: Mutex<Vec<Scene>>,
}

/// The once-only refusal latch of the fake side, as the real one is: the first reason, reported once.
#[derive(Default)]
struct Latch {
    state: Mutex<(Option<String>, bool)>,
}

impl Latch {
    fn record(&self, message: String) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.0.is_none() && !state.1 {
            state.0 = Some(message);
        }
    }

    fn take(&self) -> Option<String> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let taken = state.0.take();
        if taken.is_some() {
            state.1 = true;
        }
        taken
    }
}

/// What raises a refusal: records it and calls the wake the side was built with.
#[derive(Clone)]
pub(super) struct Refuser {
    latch: Arc<Latch>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Refuser {
    pub(super) fn refuse(&self, message: &str) {
        self.latch.record(message.to_owned());
        (self.wake)();
    }
}

struct FakeSink {
    slot: Arc<FakeSlot>,
    refuser: Refuser,
    refuse_quads: bool,
}

impl QuadSink for FakeSink {
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        let shape: Scene = (
            snapshot.generation,
            snapshot.requested,
            snapshot.quad.as_ref().map(|placed| {
                let (d, s) = (placed.destination, placed.source);
                (
                    [d.left, d.top, d.right, d.bottom],
                    [s.left, s.top, s.right, s.bottom],
                    snapshot.filter.to_string(),
                )
            }),
        );
        let refuse = self.refuse_quads && snapshot.quad.is_some();
        self.slot
            .scenes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(shape);
        if refuse {
            self.refuser.refuse("the fake side refused a quad");
        }
        Some(snapshot)
    }
}

/// The fake quad side: it makes the sink, authenticates it and keeps the refusal.
pub(super) struct FakeSide {
    scenes: Arc<FakeSlot>,
    refuser: Refuser,
    refuse_quads: bool,
    made: Option<Weak<FakeSink>>,
}

impl QuadSide for FakeSide {
    type Slot = Arc<FakeSlot>;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        let sink = Arc::new(FakeSink {
            slot: Arc::clone(&self.scenes),
            refuser: self.refuser.clone(),
            refuse_quads: self.refuse_quads,
        });
        self.made = Some(Arc::downgrade(&sink));
        sink
    }

    fn accept(&self, sink: &Arc<dyn QuadSink>) -> Result<Arc<FakeSlot>, String> {
        let made = self
            .made
            .as_ref()
            .is_some_and(|made| std::ptr::addr_eq(made.as_ptr(), Arc::as_ptr(sink)));
        if made {
            Ok(Arc::clone(&self.scenes))
        } else {
            Err("the sink is not the one this backend made".to_owned())
        }
    }

    fn take_report(&self) -> Option<PresenterReport> {
        self.refuser
            .latch
            .take()
            .map(|message| PresenterReport::Failed { message })
    }
}

/// The started presenter handle; releasing it is logged by `FakePresenter` itself.
pub(super) struct FakeStarted {
    log: Log,
    presenter: FakePresenter,
    drawn: Option<u64>,
    notice: Option<String>,
    notice_at_end: Option<String>,
    /// Reports raised from another thread (the wake regression), served before the presenter's.
    extra: Arc<Mutex<VecDeque<PresenterReport>>>,
}

impl Running for FakeStarted {
    fn set_clear(&mut self, rgb: [f32; 3]) {
        self.presenter.set_clear(rgb);
    }
    fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.presenter.set_pixel_size(width, height);
    }
    fn request_redraw(&mut self) {
        self.presenter.request_redraw();
    }
    fn take_report(&mut self) -> Option<PresenterReport> {
        let extra = self
            .extra
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front();
        extra.or_else(|| self.presenter.take_report())
    }
    fn request_stop(&mut self) {
        self.presenter.request_stop();
    }
    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        if let Some(notice) = self.notice_at_end.take() {
            self.notice = Some(notice);
        }
        self.presenter.poll_end(within)
    }
    fn drawn_generation(&self) -> Option<u64> {
        self.drawn
    }
    fn take_notice(&mut self) -> Option<String> {
        let notice = self.notice.take();
        if notice.is_some() {
            (self.log)("notice taken");
        }
        notice
    }
}

/// Microseconds since the first wake regression raised something, read once by the child that measures how long
/// the loop took to notice (0 until it happens).
static RAISED_AT: AtomicU64 = AtomicU64::new(0);

/// When the wake regression raised its report or refusal, as microseconds on [`clock`]; 0 if it did not.
pub(super) fn raised_at() -> u64 {
    RAISED_AT.load(Ordering::SeqCst)
}

/// Microseconds on a process-wide clock.
pub(super) fn clock() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let micros = START.get_or_init(Instant::now).elapsed().as_micros();
    u64::try_from(micros).unwrap_or(u64::MAX).max(1)
}

/// How long after the window opened the wake regression raises its report or refusal (the loop is asleep by then).
const RAISE_AFTER: Duration = Duration::from_millis(300);
/// The longest the fake GLFW's pump sleeps for a wake: long, so a wake that never comes costs this much.
const SLEEP_BOUND: Duration = Duration::from_secs(10);

pub(super) struct FakeStack {
    script: Script,
    log: Log,
    notifier: Arc<Notifier>,
    /// Every scene the fake side's sinks were given.
    pub(super) slot: Arc<FakeSlot>,
    /// The refuser of the side `quad_side` built, for the wake regression's thread.
    refuser: Arc<Mutex<Option<Refuser>>>,
}

impl FakeStack {
    pub(super) fn new(script: Script, log: Log, notifier: Arc<Notifier>) -> Self {
        Self {
            script,
            log,
            notifier,
            slot: Arc::new(FakeSlot::default()),
            refuser: Arc::new(Mutex::new(None)),
        }
    }
}

impl Stack for FakeStack {
    type Handles = ();
    type Glfw = FakeBackend;
    type Loader = FakeLoader;
    type Instance = FakeInstance;
    type Surface = u64;
    type Started = FakeStarted;
    type Slot = Arc<FakeSlot>;
    type Side = FakeSide;

    fn load_loader(&self) -> Result<FakeLoader, String> {
        (self.log)("load loader");
        match &self.script.loader_error {
            Some(error) => Err(error.clone()),
            None => Ok(FakeLoader(Arc::clone(&self.log))),
        }
    }

    fn glfw(&self, _loader: &FakeLoader) -> Result<FakeBackend, String> {
        (self.log)("create glfw");
        match &self.script.glfw_error {
            Some(error) => Err(error.clone()),
            None => Ok(FakeBackend::new(
                BackendScript {
                    create_error: self.script.window_error.clone(),
                    wait_bound: if self.script.wake_case.is_some() {
                        SLEEP_BOUND
                    } else {
                        BackendScript::default().wait_bound
                    },
                    ..BackendScript::default()
                },
                Arc::clone(&self.log),
            )),
        }
    }

    fn create_instance(&self, _loader: &FakeLoader, (): &()) -> Result<FakeInstance, String> {
        (self.log)("create instance");
        match &self.script.instance_error {
            Some(error) => Err(error.clone()),
            None => Ok(FakeInstance(Arc::clone(&self.log))),
        }
    }

    fn create_surface(&self, (): &(), _instance: &FakeInstance) -> Result<u64, String> {
        (self.log)("create surface");
        match &self.script.surface_error {
            Some(error) => Err(error.clone()),
            None => Ok(7),
        }
    }

    fn quad_side(&self, wake: Arc<dyn Fn() + Send + Sync>) -> FakeSide {
        let refuser = Refuser {
            latch: Arc::new(Latch::default()),
            wake,
        };
        *self.refuser.lock().unwrap_or_else(PoisonError::into_inner) = Some(refuser.clone());
        FakeSide {
            scenes: Arc::clone(&self.slot),
            refuser,
            refuse_quads: self.script.refuse_quads,
            made: None,
        }
    }

    fn start(
        &self,
        _instance: &FakeInstance,
        _surface: u64,
        size: PixelSize,
        clear: [f32; 3],
        wake: Arc<dyn Fn() + Send + Sync>,
        slot: Arc<FakeSlot>,
    ) -> Result<FakeStarted, String> {
        // Only the slot behind the sink the side made reaches the presenter.
        assert!(
            Arc::ptr_eq(&slot, &self.slot),
            "the presenter was given a slot the side did not make"
        );
        (self.log)("presenter start with the side's slot");
        // A wake regression's presenter has a notifier of its own: its own publications wake nothing the loop
        // sleeps on, so only the wake the adapter was given can.
        let notifier = if self.script.wake_case.is_some() {
            Notifier::new()
        } else {
            Arc::clone(&self.notifier)
        };
        let mut presenter = FakePresenter::new(
            notifier,
            self.script.presenter.clone(),
            Arc::clone(&self.log),
        );
        let desc = WindowDesc {
            title: String::new(),
            width: size.width,
            height: size.height,
            clear,
        };
        Presenter::start(&mut presenter, (), &desc, size)?;
        let extra = Arc::new(Mutex::new(VecDeque::new()));
        match self.script.wake_case {
            Some(WakeCase::Report) => {
                let (extra, wake) = (Arc::clone(&extra), Arc::clone(&wake));
                std::thread::spawn(move || {
                    std::thread::sleep(RAISE_AFTER);
                    extra
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push_back(PresenterReport::Failed {
                            message: "probe: a report raised while the loop sleeps".to_owned(),
                        });
                    RAISED_AT.store(clock(), Ordering::SeqCst);
                    wake();
                });
            }
            Some(WakeCase::Refusal) => {
                let refuser = self
                    .refuser
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                std::thread::spawn(move || {
                    std::thread::sleep(RAISE_AFTER);
                    if let Some(refuser) = refuser {
                        RAISED_AT.store(clock(), Ordering::SeqCst);
                        refuser.refuse("probe: the fake side refused a quad while the loop sleeps");
                    }
                });
            }
            None => {}
        }
        Ok(FakeStarted {
            log: Arc::clone(&self.log),
            presenter,
            drawn: self.script.drawn_generation,
            notice: self.script.notice.clone(),
            notice_at_end: self.script.notice_at_end.clone(),
            extra,
        })
    }

    fn destroy(
        &self,
        instance: FakeInstance,
        surface: Option<u64>,
        ended: Ended,
    ) -> Result<(), Refused<FakeInstance>> {
        (self.log)(&format!(
            "destroy helper surface={} ended={ended:?}",
            surface.map_or_else(|| "none".to_owned(), |surface| surface.to_string())
        ));
        if let Some(surface_destroyed) = self.script.refuse {
            if surface_destroyed {
                (self.log)("surface destroyed");
            }
            if self.script.panic_in_drain {
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_millis(300));
                    panic!("probe: a panic during the late-callback drain");
                });
            }
            return Err(Refused {
                instance,
                surface_destroyed,
                cause: "the helper refused".to_owned(),
            });
        }
        if surface.is_some() {
            (self.log)("surface destroyed");
        }
        (self.log)("instance destroyed");
        drop(instance);
        Ok(())
    }
}
