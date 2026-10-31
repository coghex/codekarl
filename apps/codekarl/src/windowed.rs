//! The windowed run (design §4.1, §7): the run closure the runtime calls between start-up and the stop
//! phase, generic over the window backend and the presenter so that the failure-boundary tests can run
//! the very same code with fakes.
//!
//! [`run_window`] hands `ck_window::run` the backend (created lazily, see below), the presenter, the UI
//! link ([`CodekarlUi`]), the request slot's reader, the notifier and the quit flag, then maps the
//! [`LoopEnd`] to the runtime's [`RunEnd`]:
//!
//! | `LoopEnd` | `RunEnd` |
//! | --- | --- |
//! | `NoWindow`, `Closed` | `Finished` |
//! | `Quit` | `Quit` |
//! | `CallbackFailed` | `Failed(message)` |
//! | `Failed` | `Failed(message)` (the UI VM's own failure text when the link saw one) |
//! | `PresenterFailed` | `Failed(message)`, also after a clean close or quit (the loop took a late report); a UI VM failure the link saw is appended to it |
//! | `Unproven` | `Failed(..)` **and** the run is marked incomplete ([`RunNotes`]): the runtime still runs its
//! normal, bounded stop phase, then the executable ends through the immediate exit with status 3 |
//!
//! **The backend is created lazily**, on the main thread, in `Backend::create_window`: nothing native
//! exists unless the UI asked for a window. [`ResourceLatch`] is set immediately before that first native
//! object, and cleared only when `ck_window::run` returns an end other than `Unproven`; see
//! [`crate::lifecycle`] for the panic hook's rule.
//!
//! **Late callback failures** (design §3.5): a callback may fail after the loop ended (for example
//! `on_opened` calling `window.close()` and then raising). Before the stop phase, [`drain_late`] keeps
//! taking answers, in slices and quit-aware, for at most [`DRAIN`] while posted calls are outstanding; a
//! `Failed` answer becomes `RunEnd::Failed` even after `Closed`, and a call still unanswered at the bound
//! gets one `warn window: ui callback <name> still running at shutdown` record (a diagnostic only). After
//! the stop phase the executable takes every remaining result without waiting (`main`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ck_log::Level;
use ck_lua::Extension;
use ck_runtime::{RunContext, RunEnd};
use ck_window::{
    Answer, Backend, CreatedWindow, LoopEnd, LoopSettings, NativeEvent, NativeWake, Notifier,
    Presenter, RequestReader, UiLink, Wait,
};

use crate::lifecycle::{ResourceLatch, emit};
use crate::ui_link::CodekarlUi;

/// The longest the run closure waits, after the loop ended, for posted calls that are still running.
pub(crate) const DRAIN: Duration = Duration::from_secs(1);
/// The slice of the drain's polls.
const DRAIN_SLICE: Duration = Duration::from_millis(5);

/// What the run tells the executable besides the [`RunEnd`] it returns to the runtime.
#[derive(Debug, Default)]
pub(crate) struct RunNotes {
    /// The window teardown was `Unproven`: the run is incomplete (status 3).
    pub(crate) unproven: Cell<bool>,
    /// Resource loaders or retained texture references outlived the configured stop deadline.
    pub(crate) resources_incomplete: Cell<bool>,
    /// Owners that may still hold `TextureRef`s when the resource service drains.
    pub(crate) resource_holders: RefCell<Vec<String>>,
    /// The presenter's last numbers, copied here when its end was observed and before its backend was destroyed (the
    /// final resource record reads them after the resource service reconciles). `None` when no presenter ran.
    pub(crate) gpu: RefCell<Option<GpuFigures>>,
    /// The same for a `glfw-vulkan` run (the Vulkan presenter's figures; `gpu` stays empty there).
    pub(crate) vulkan_gpu: RefCell<Option<VulkanFigures>>,
}

/// The GPU-side numbers of the final resource record: plain values copied from the presenter's last snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GpuFigures {
    /// Textures the upload store created over the run (cumulative).
    pub(crate) textures_created: u64,
    /// The entry count of the bindless tables the presenter allocated (`[render] bindless_texture_slots`).
    pub(crate) bindless_slots: u32,
    /// The most texture bytes the upload store ever had charged at once (the images' textures; not the checker's).
    pub(crate) upload_texture_bytes_peak: u64,
    /// The most staging bytes the upload store ever had charged at once (the upload leases; not the checker's).
    pub(crate) upload_staging_bytes_peak: u64,
    /// The checker texture's measured bytes.
    pub(crate) checker_texture_bytes: u64,
    /// The checker's staging buffer's measured bytes.
    pub(crate) checker_staging_bytes: u64,
    /// The instance buffer's measured allocation.
    pub(crate) instance_buffer_bytes: u64,
    /// The most checker instances any one committed frame drew.
    pub(crate) checker_instances_peak: u64,
    /// Frames committed.
    pub(crate) frames: u64,
}

/// The Vulkan presenter's numbers for the same record: the figures it reports, and none of the Metal-only ones above (the
/// record omits those rather than writing 0). The native path's [`GpuFigures`] is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VulkanFigures {
    /// Images the presenter created for uploads over the run (the checker included).
    pub(crate) textures_created: u64,
    /// The most image memory ever reserved at once.
    pub(crate) texture_bytes_peak: u64,
    /// The most staging memory ever allocated at once.
    pub(crate) staging_bytes_peak: u64,
    /// The bytes the driver allocated for the checker.
    pub(crate) checker_bytes: u64,
    /// Frames queued for presentation.
    pub(crate) frames: u64,
}

impl RunNotes {
    pub(crate) fn retain_resource_holder(&self, holder: &str) {
        let mut holders = self.resource_holders.borrow_mut();
        if !holders.iter().any(|present| present == holder) {
            holders.push(holder.to_owned());
        }
    }
}

/// The run closure type of `Runtime::with_run`.
pub(crate) type RunClosure = Box<dyn FnOnce(&RunContext) -> RunEnd>;

/// Everything the windowed run needs besides its backend and presenter.
pub(crate) struct RunEnv {
    /// The UI link (shares the UI VM's handle with the runtime's adapter).
    pub(crate) ui: CodekarlUi,
    /// The request slot's reader.
    pub(crate) reader: RequestReader,
    /// The notifier every producer wakes.
    pub(crate) notifier: Arc<Notifier>,
    /// The quit flag the signal thread sets.
    pub(crate) quit: Arc<AtomicBool>,
    /// The loop's caps and teardown deadlines.
    pub(crate) settings: LoopSettings,
    /// The run's measurement window (`diagnostics.metrics_mark`); `Some` in a native-metal run.
    #[cfg_attr(
        not(all(feature = "metal", target_os = "macos")),
        expect(dead_code, reason = "only the native Metal backend measures")
    )]
    pub(crate) metrics: Option<Arc<crate::metrics::MetricsHub>>,
    /// What the run tells the executable.
    pub(crate) notes: Rc<RunNotes>,
    /// The run's scene bridge: the renderer of the backend the run chose attaches to it.
    pub(crate) bridge: Arc<ck_render_2d::SceneBridge>,
    /// Late-bound presenter wake used by resource uploads and retirements (a windowed backend connects it).
    pub(crate) wake_relay: ck_render_2d::WakeRelay,
    /// The per-run resource service (absent only in a test that runs without one).
    pub(crate) resources: Option<ck_resources::ResourceService>,
    /// Parsed service and backend caps, carried to the windowed backend.
    pub(crate) resource_config: crate::config::ResourceConfig,
    /// `[render] max_quads`: the instance buffer's room on a windowed backend.
    pub(crate) max_quads: usize,
    /// `[render] bindless_texture_slots`: the entry count of the native backend's bindless tables.
    #[cfg_attr(
        not(all(feature = "metal", target_os = "macos")),
        expect(
            dead_code,
            reason = "only the native Metal backend allocates the bindless tables"
        )
    )]
    pub(crate) bindless_texture_slots: u32,
}

/// What `execute` builds before it knows the run closure: the window extension and the run environment.
pub(crate) struct Setup {
    /// The backend the run chose (the run closure of a windowed run is that backend's).
    pub(crate) backend: crate::mode::Backend,
    /// The `window` extension of the UI VM (a test may add host functions to it).
    pub(crate) extension: Extension,
    /// The run environment.
    pub(crate) env: RunEnv,
}

/// What `execute` runs with: the (possibly extended) extension and the run closure, if any.
pub(crate) struct Plan {
    /// The extension the UI VM is created with.
    pub(crate) extension: Extension,
    /// The run closure, or none.
    pub(crate) run: Option<RunClosure>,
}

/// A wake target for a backend that was never created: it does nothing.
struct NoWake;

impl NativeWake for NoWake {
    fn wake(&self) {}
}

/// A backend that creates the real one on the main thread, in `create_window`, with the latch set first.
struct Lazy<'a, B, F> {
    make: Option<F>,
    backend: Option<B>,
    latch: &'a ResourceLatch,
    /// The run configured a paced update source, which needs a backend that can wait until a deadline. The loop asks
    /// before the backend exists, so this is the answer until it does; the backend's own answer replaces it.
    deadline: bool,
}

impl<B, F> Backend for Lazy<'_, B, F>
where
    B: Backend,
    F: FnOnce() -> Result<B, String>,
{
    type Handles = B::Handles;

    fn create_window(
        &mut self,
        desc: &ck_window::WindowDesc,
    ) -> Result<CreatedWindow<B::Handles>, String> {
        let Some(make) = self.make.take() else {
            return Err("the window backend was already created".to_owned());
        };
        // Immediately before the first native object exists.
        self.latch.set();
        let backend = self.backend.insert(make()?);
        backend.create_window(desc)
    }

    fn wake_target(&self) -> Arc<dyn NativeWake> {
        self.backend
            .as_ref()
            .map_or_else(|| Arc::new(NoWake) as Arc<dyn NativeWake>, B::wake_target)
    }

    fn pump(&mut self, events: &mut dyn FnMut(NativeEvent), wait: Wait, cap: usize) {
        if let Some(backend) = &mut self.backend {
            backend.pump(events, wait, cap);
        }
    }

    fn supports_deadline(&self) -> bool {
        self.backend
            .as_ref()
            .map_or(self.deadline, Backend::supports_deadline)
    }

    fn pump_until(&mut self, events: &mut dyn FnMut(NativeEvent), deadline: Instant, cap: usize) {
        if let Some(backend) = &mut self.backend {
            backend.pump_until(events, deadline, cap);
        }
    }

    fn destroy_window(&mut self) {
        if let Some(backend) = &mut self.backend {
            backend.destroy_window();
        }
    }

    fn terminate(&mut self) {
        if let Some(backend) = &mut self.backend {
            backend.terminate();
        }
    }
}

/// The runtime's end for the loop's, noting an `Unproven` teardown.
fn run_end(end: LoopEnd, ui: &CodekarlUi, notes: &RunNotes) -> RunEnd {
    match end {
        LoopEnd::NoWindow | LoopEnd::Closed => RunEnd::Finished,
        LoopEnd::Quit => RunEnd::Quit,
        LoopEnd::CallbackFailed { message } => RunEnd::Failed(message),
        // The presenter's failure is the run's, whatever else the link saw: a UI failure is added to the
        // same record, so neither hides the other.
        LoopEnd::PresenterFailed { message } => RunEnd::Failed(match ui.failure() {
            Some(failure) => format!("{message} (the ui VM also failed: {failure})"),
            None => message,
        }),
        LoopEnd::Failed { message } => RunEnd::Failed(
            // The loop only knows that the UI VM ended; the link has the reason.
            ui.failure()
                .map_or(message, |failure| format!("the ui VM failed: {failure}")),
        ),
        LoopEnd::Unproven { reason } => {
            notes.unproven.set(true);
            notes.retain_resource_holder("render thread (unproven presenter end)");
            RunEnd::Failed(format!("the window teardown is unproven: {reason}"))
        }
    }
}

/// A late callback failure that the run's own end does not carry: one `error` record in category `window`
/// with its message and traceback (the failure was taken from the link, so it is logged exactly once).
pub(crate) fn log_late_failure(message: &str) {
    emit(Level::Error, "window", format_args!("{message}"));
}

/// After the loop ended, **whatever its end**: keeps taking answers while posted calls are outstanding, for
/// at most `drain`, in slices, and not at all once a quit is requested (`quit`, the very flag the runtime's
/// `RunContext` reads). Every `Failed` answer is returned, in order; a call still running at the bound gets
/// one `warn` record.
fn drain_late(quit: &AtomicBool, ui: &CodekarlUi, drain: Duration) -> Vec<String> {
    let mut link = ui.clone();
    let mut failures = Vec::new();
    let until = Instant::now()
        .checked_add(drain)
        .unwrap_or_else(Instant::now);
    loop {
        while let Some(answer) = link.take_answer() {
            if let Answer::Failed { message } = answer.answer {
                failures.push(message);
            }
        }
        if ui.outstanding() == 0 || quit.load(Ordering::SeqCst) || link.is_disconnected() {
            return failures;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            for name in ui.outstanding_names() {
                emit(
                    Level::Warn,
                    "window",
                    format_args!("ui callback {name} still running at shutdown"),
                );
            }
            return failures;
        }
        std::thread::sleep(left.min(DRAIN_SLICE));
    }
}

/// The windowed run of a backend whose teardown always releases what it destroyed (the native path).
#[cfg(any(test, all(feature = "metal", target_os = "macos")))]
pub(crate) fn run_window<B, P, F>(
    env: &RunEnv,
    make_backend: F,
    presenter: P,
    latch: &ResourceLatch,
    drain: Duration,
) -> RunEnd
where
    B: Backend,
    P: Presenter<Handles = B::Handles>,
    F: FnOnce() -> Result<B, String>,
{
    run_window_keeping(env, make_backend, presenter, latch, drain, &|| false)
}

/// The windowed run: `ck_window::run` with a lazily created backend, then the end mapping and the drain
/// of late callback failures. `drain` is the drain bound ([`DRAIN`] in the executable).
///
/// `kept` is for a backend that can keep its objects after a teardown that returned normally (the Vulkan
/// path's refused destroy; [`run_window`] passes `&|| false`): it says, once the loop has returned and **before** the latch is
/// decided or anything else runs, whether objects were kept. If so the latch stays set continuously (the
/// late-callback drain included, so a panic there still exits 3) and the run is unproven.
pub(crate) fn run_window_keeping<B, P, F>(
    env: &RunEnv,
    make_backend: F,
    presenter: P,
    latch: &ResourceLatch,
    drain: Duration,
    kept: &dyn Fn() -> bool,
) -> RunEnd
where
    B: Backend,
    P: Presenter<Handles = B::Handles>,
    F: FnOnce() -> Result<B, String>,
{
    let backend = Lazy {
        make: Some(make_backend),
        backend: None,
        latch,
        // The update source is configured for the native-metal loop only, whose backend waits until deadlines.
        deadline: env.settings.updates().is_some(),
    };
    let end = ck_window::run(
        backend,
        presenter,
        env.ui.clone(),
        &env.reader,
        &env.notifier,
        &env.quit,
        &env.settings,
    );
    // The run phase is over: the bridge ends here, before anything else runs and before the late-callback
    // drain below, so that a write that comes later (a decode still running, `shutdown()`) is discarded and
    // counted. The renderer detaches it too when it is asked to stop, but not on the paths that never start
    // a presenter (no window, an open and a close in the first batch, a quit before the start, a failed
    // native creation). Detaching again is harmless.
    env.bridge.detach();
    // Proven (or nothing was ever created): the latch is cleared. `Unproven` keeps it set, and so does a
    // teardown that kept objects although it returned normally.
    if kept() {
        latch.set();
        env.notes.unproven.set(true);
    } else if !matches!(end, LoopEnd::Unproven { .. }) {
        latch.clear();
    }
    let ended = run_end(end, &env.ui, &env.notes);
    // The drain runs after every loop return with calls outstanding. The run's own failure stays the
    // run's failure; a late callback failure becomes the failure of an otherwise good run, and is logged
    // by itself next to any other failure, so an earlier unrelated failure never hides its traceback.
    let mut late = drain_late(&env.quit, &env.ui, drain).into_iter();
    match ended {
        RunEnd::Finished | RunEnd::Quit => match late.next() {
            Some(first) => {
                late.for_each(|message| log_late_failure(&message));
                RunEnd::Failed(first)
            }
            None => ended,
        },
        RunEnd::Failed(_) => {
            late.for_each(|message| log_late_failure(&message));
            ended
        }
    }
}

/// The end of a windowed run that failed before any window or native object existed (the Metal device context could not be
/// made, or a run had no resource service): the request slot is ended and the bridge detached, as the loop's own ends do,
/// and the run fails with `message`. No window was opened, so the UI is never told `Opened`; its `shutdown()` and the
/// normal teardown still run.
pub(crate) fn fail_before_window(env: &RunEnv, message: String) -> RunEnd {
    env.reader.end();
    env.bridge.detach();
    RunEnd::Failed(message)
}

/// The headless run closure: no window, no backend. The request slot is ended where the run phase would
/// have started, so later writes return at once and the run finishes.
pub(crate) fn headless_run(reader: RequestReader) -> RunClosure {
    Box::new(move |_| {
        reader.end();
        RunEnd::Finished
    })
}

#[cfg(test)]
pub(crate) mod render_tests;
#[cfg(test)]
mod tests;
