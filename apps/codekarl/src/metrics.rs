//! The run's measurement window and the `diagnostics` extension of the UI VM (milestone 5, issue 10b-1).
//!
//! `diagnostics.metrics_mark()` defines the run's **one** measured window: `start` is now plus [`MARK_LEAD_NS`], the end is open. The
//! call never waits: it takes the clock, files the window in the [`MetricsHub`] (a lock held for a few assignments) and asks the window
//! loop's measurement windows ([`LoopMetrics::request`]) to adopt it, which wakes the loop. The loop's thread then hands the same
//! window to the presenter at its next pass ([`MetricsHub::take_forward`]), because the presenter belongs to that thread. A second call
//! changes nothing and is counted. When the window loop begins its teardown it takes **one** timestamp ([`MetricsHub::finish`]) and
//! sends it to both owners as the window's `end`, so both cover exactly `[start, end)` whatever their adoption times.
//!
//! A run that never marks measures nothing; the end-of-run `metrics` record then holds the whole-run totals only.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use ck_lua::Extension;
use ck_window::{LoopMetrics, MetricsWindow, UptimeNs};

/// How long after the mark the window starts: both owners adopt at their next boundary, before the window opens.
pub(crate) const MARK_LEAD_NS: u64 = 250_000_000;

/// The window of a run is always this id: ids are a per-run counter and a run has one window.
pub(crate) const WINDOW_ID: u32 = 1;

/// The hub's state, under one lock held only to read or assign small values.
#[derive(Debug, Default)]
struct State {
    /// The run's measured window, once marked.
    window: Option<MetricsWindow>,
    /// The window has not yet been handed to the presenter.
    forward: bool,
    /// Calls to `metrics_mark` after the first (or after the run ended).
    later_marks: u64,
    /// The loop's teardown timestamp.
    end: Option<u64>,
    /// When the script first asked for a texture to load (the start of the upload pipeline's span), zero before.
    first_load_ns: u64,
    /// The window loop's owner refused the request or the end (the text names why).
    loop_refused: Option<String>,
    /// The presenter refused the request or the end.
    presenter_refused: Option<String>,
}

/// What `metrics_mark` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Marked {
    /// This call started the run's window.
    Started(MetricsWindow),
    /// An earlier call had started it, or the run was ending: counted, nothing changed.
    Counted,
}

/// The run's measurement window, shared by the UI VM (the mark), the window loop's thread (forwarding it, the end) and the summary.
pub(crate) struct MetricsHub {
    loop_metrics: LoopMetrics,
    clock: UptimeNs,
    /// The loop's measurement windows are attached to an update source (without one nothing is counted there).
    loop_attached: bool,
    state: Mutex<State>,
}

impl std::fmt::Debug for MetricsHub {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MetricsHub")
    }
}

impl MetricsHub {
    /// A hub over `clock` (`CLOCK_UPTIME_RAW` nanoseconds); `loop_attached` says the window loop counts updates.
    pub(crate) fn new(clock: UptimeNs, loop_attached: bool) -> Self {
        Self {
            loop_metrics: LoopMetrics::new(Arc::clone(&clock)),
            clock,
            loop_attached,
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The loop's measurement windows (given to the update settings).
    pub(crate) fn loop_metrics(&self) -> &LoopMetrics {
        &self.loop_metrics
    }

    /// What a `texture.load` tells the hub: the instant to stamp a request with, read before it is submitted, and the notice that
    /// the request stamped at an instant was accepted. The earliest accepted stamp is when the upload pipeline's span begins; a
    /// request that was refused never defines it.
    pub(crate) fn load_notes(self: &Arc<Self>) -> crate::texture_api::LoadNotes {
        let (clock, noting) = (Arc::clone(self), Arc::clone(self));
        crate::texture_api::LoadNotes {
            before: Arc::new(move || clock.now()),
            accepted: Arc::new(move |at| noting.note_load_at(at)),
        }
    }

    /// An accepted `texture.load` stamped at `at`: the earliest such stamp is the start of the upload pipeline's span.
    fn note_load_at(&self, at: u64) {
        let mut state = self.lock();
        if state.first_load_ns == 0 || at < state.first_load_ns {
            state.first_load_ns = at;
        }
    }

    /// The clock.
    pub(crate) fn now(&self) -> u64 {
        (self.clock)()
    }

    /// The clock itself, to give the presenter (so the window is one interval on one clock for every owner).
    pub(crate) fn clock(&self) -> UptimeNs {
        Arc::clone(&self.clock)
    }

    /// `diagnostics.metrics_mark()`: the first call starts the window and never waits.
    pub(crate) fn mark(&self) -> Marked {
        let now = self.now();
        let window = {
            let mut state = self.lock();
            if state.window.is_some() || state.end.is_some() {
                state.later_marks = state.later_marks.saturating_add(1);
                return Marked::Counted;
            }
            let window = MetricsWindow {
                id: WINDOW_ID,
                start: now.saturating_add(MARK_LEAD_NS),
                end: None,
            };
            state.window = Some(window);
            state.forward = true;
            window
        };
        if self.loop_attached
            && let Err(error) = self.loop_metrics.request(window)
        {
            self.lock().loop_refused = Some(format!("{error:?}"));
        }
        Marked::Started(window)
    }

    /// The window to hand to the presenter, once: the loop's thread asks at each pass, and gives it back with
    /// [`unforwarded`](Self::unforwarded) when the presenter is not started yet.
    pub(crate) fn take_forward(&self) -> Option<MetricsWindow> {
        let mut state = self.lock();
        if state.forward {
            state.forward = false;
            state.window
        } else {
            None
        }
    }

    /// The presenter refused the window or its end (the text names why); the summary says so.
    pub(crate) fn presenter_refused(&self, why: String) {
        self.lock().presenter_refused = Some(why);
    }

    /// The presenter was not ready: the window stays to be forwarded.
    pub(crate) fn unforwarded(&self) {
        self.lock().forward = true;
    }

    /// The loop's teardown: takes the one end timestamp and sets it as the window's end on both owners, `render_end` being the
    /// presenter's. The timestamp is read **while both owners' admission locks are held** (the presenter's, by the caller, whose
    /// `render_end` sets its end; the loop's, here), so neither owner admitted a sample at or after it and both then cover exactly
    /// `[start, end)`. Returns the window with that end. Once: a later call returns `None`. `None` too when no window was marked.
    pub(crate) fn finish(
        &self,
        render_end: &mut dyn FnMut(u32, u64) -> Result<(), String>,
    ) -> Option<MetricsWindow> {
        let mut window = {
            let state = self.lock();
            if state.end.is_some() {
                return None;
            }
            state.window?
        };
        // The loop's admission lock, then the one timestamp, then both ends, then the lock goes.
        let mut loop_guard = self.loop_attached.then(|| self.loop_metrics.lock_end());
        let end = self.now();
        window.end = Some(end);
        self.lock().end = Some(end);
        let loop_result = loop_guard
            .as_mut()
            .map(|guard| guard.end(window.id, end))
            .transpose();
        let render_result = render_end(window.id, end);
        drop(loop_guard);
        let mut state = self.lock();
        if let Err(error) = loop_result {
            state.loop_refused = Some(format!("{error:?}"));
        }
        if let Err(error) = render_result {
            state.presenter_refused = Some(error);
        }
        Some(window)
    }

    /// What the summary needs of the hub.
    pub(crate) fn report(&self) -> HubReport {
        let state = self.lock();
        HubReport {
            window: state.window.map(|window| MetricsWindow {
                end: state.end,
                ..window
            }),
            later_marks: state.later_marks,
            first_load_ns: state.first_load_ns,
            loop_attached: self.loop_attached,
            loop_refused: state.loop_refused.clone(),
            presenter_refused: state.presenter_refused.clone(),
        }
    }
}

/// What the hub knows at the end of the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HubReport {
    /// The marked window, with the teardown timestamp as its end once the loop began its teardown.
    pub(crate) window: Option<MetricsWindow>,
    /// Calls after the first.
    pub(crate) later_marks: u64,
    /// When the script first asked for a texture to load (zero: never).
    pub(crate) first_load_ns: u64,
    /// The window loop counts updates.
    pub(crate) loop_attached: bool,
    /// The window loop's owner refused the window or its end.
    pub(crate) loop_refused: Option<String>,
    /// The presenter refused the window or its end.
    pub(crate) presenter_refused: Option<String>,
}

/// How long the summary waits for the owners to consume work still in flight at the run's end.
pub(crate) const SUMMARY_WAIT: Duration = Duration::from_millis(200);

/// The clock the run's measurement windows use: `CLOCK_UPTIME_RAW` nanoseconds, the Metal presenter's clock, so a window defined here
/// means the same instant to the presenter and the loop. Without Metal there is no presenter and the clock is the process's own
/// monotonic one.
#[cfg(all(feature = "metal", target_os = "macos"))]
pub(crate) fn uptime_clock() -> UptimeNs {
    Arc::new(ck_render_2d::now_ns)
}

/// See above.
#[cfg(not(all(feature = "metal", target_os = "macos")))]
pub(crate) fn uptime_clock() -> UptimeNs {
    let origin = std::time::Instant::now();
    Arc::new(move || u64::try_from(origin.elapsed().as_nanos()).unwrap_or(u64::MAX))
}

/// The `diagnostics` extension. `hub` is `Some` in a windowed run (native-metal or `glfw-vulkan`: the mark opens the window); `None` is
/// a headless run, where the function exists and does nothing (there is nothing to measure).
pub(crate) fn extension(hub: Option<Arc<MetricsHub>>) -> Extension {
    let function: ck_lua::HostFunction = Arc::new(move |arguments| {
        if !arguments.is_empty() {
            return Err("diagnostics.metrics_mark takes no arguments".to_owned());
        }
        if let Some(hub) = &hub {
            let _ = hub.mark();
        }
        Ok(Vec::new())
    });
    Extension::new("diagnostics").function("metrics_mark", function)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn hub(attached: bool) -> (Arc<MetricsHub>, Arc<AtomicU64>) {
        let now = Arc::new(AtomicU64::new(10_000_000_000));
        let clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst)) as UptimeNs
        };
        (Arc::new(MetricsHub::new(clock, attached)), now)
    }

    #[test]
    fn the_first_mark_starts_a_window_a_lead_after_the_call_and_later_ones_only_count() {
        let (hub, now) = hub(false);
        let Marked::Started(window) = hub.mark() else {
            panic!("the first call starts the window");
        };
        assert_eq!(
            window,
            MetricsWindow {
                id: WINDOW_ID,
                start: 10_000_000_000 + MARK_LEAD_NS,
                end: None
            }
        );
        now.store(11_000_000_000, Ordering::SeqCst);
        assert_eq!(hub.mark(), Marked::Counted);
        assert_eq!(hub.mark(), Marked::Counted);
        let report = hub.report();
        assert_eq!(report.window, Some(window), "later calls change nothing");
        assert_eq!(report.later_marks, 2);
    }

    #[test]
    fn the_window_is_forwarded_once_and_comes_back_when_the_presenter_is_not_ready() {
        let (hub, _) = hub(false);
        assert_eq!(hub.take_forward(), None, "nothing marked");
        let Marked::Started(window) = hub.mark() else {
            panic!("started");
        };
        assert_eq!(hub.take_forward(), Some(window));
        assert_eq!(hub.take_forward(), None, "once");
        hub.unforwarded();
        assert_eq!(hub.take_forward(), Some(window), "given back");
    }

    #[test]
    fn the_teardown_takes_one_timestamp_for_both_owners_and_a_late_mark_is_only_counted() {
        let (hub, now) = hub(true);
        let Marked::Started(window) = hub.mark() else {
            panic!("started");
        };
        now.store(12_000_000_000, Ordering::SeqCst);
        let ended = hub.finish(&mut |_, _| Ok(())).expect("the window ends");
        assert_eq!(ended.end, Some(12_000_000_000));
        assert_eq!(ended.start, window.start);
        now.store(13_000_000_000, Ordering::SeqCst);
        assert_eq!(
            hub.finish(&mut |_, _| Ok(())),
            None,
            "the end is taken once"
        );
        assert_eq!(
            hub.mark(),
            Marked::Counted,
            "a mark after the end is counted"
        );
        let report = hub.report();
        assert_eq!(report.window, Some(ended));
        assert_eq!(report.later_marks, 1);
        // The loop's side took the same definition (its end included) and adopts it at its next pass, which this test does not run.
        assert_eq!(hub.loop_metrics().adopted(WINDOW_ID), None);
        assert_eq!(report.loop_refused, None);
    }

    #[test]
    fn a_run_that_never_marks_has_no_window_and_finishing_changes_nothing() {
        let (hub, _) = hub(true);
        assert_eq!(hub.finish(&mut |_, _| Ok(())), None);
        let report = hub.report();
        assert_eq!(report.window, None);
        assert_eq!(report.later_marks, 0);
    }

    #[test]
    fn a_window_that_ends_before_it_starts_is_refused_by_the_loop_and_said_so() {
        let (hub, now) = hub(true);
        let Marked::Started(_) = hub.mark() else {
            panic!("started");
        };
        // The run ends 10 ms after the mark: before the 250 ms lead is over.
        now.store(10_010_000_000, Ordering::SeqCst);
        let ended = hub.finish(&mut |_, _| Ok(())).expect("the hub ends it");
        assert!(ended.end.expect("end") < ended.start);
        assert!(
            hub.report().loop_refused.is_some(),
            "the loop's side refused an empty window"
        );
    }

    // --- the Lua function ---

    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    use ck_lua::{ConfigValue, VmOwner, VmSettings};

    /// Runs `script` as the UI VM's main file with the `diagnostics` extension; a script that raises fails the start.
    fn run_script(extension: Extension, script: &str) {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "codekarl-metrics-mark-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).expect("a scratch directory");
        let entry = dir.join("main.lua");
        fs::write(&entry, script).expect("a script");
        let mut settings = VmSettings::new("test", entry, ConfigValue::Table(BTreeMap::new()));
        settings.extensions = vec![extension];
        let mut vm = VmOwner::spawn(settings).expect("the VM spawns");
        vm.wait_ready(Duration::from_secs(5)).expect("ready");
        let started = vm.start(Duration::from_secs(5));
        let _ = vm.stop(Duration::from_secs(5));
        let _ = fs::remove_dir_all(&dir);
        started.expect("the script ran to its end");
    }

    #[test]
    fn a_lua_script_that_marks_twice_starts_one_window_and_the_second_call_is_counted() {
        let (hub, now) = hub(true);
        let began = Instant::now();
        now.store(20_000_000_000, Ordering::SeqCst);
        run_script(
            extension(Some(Arc::clone(&hub))),
            "diagnostics.metrics_mark()\ndiagnostics.metrics_mark()\n\
             local ok, err = pcall(diagnostics.metrics_mark, 1)\n\
             if ok then error('arguments were accepted') end\n\
             if not string.find(tostring(err), 'takes no arguments', 1, true) then error(tostring(err)) end\n",
        );
        let report = hub.report();
        assert_eq!(
            report.window,
            Some(MetricsWindow {
                id: WINDOW_ID,
                start: 20_000_000_000 + MARK_LEAD_NS,
                end: None
            }),
            "the window starts a lead after the first call"
        );
        assert_eq!(
            report.later_marks, 1,
            "the second call changed nothing and was counted"
        );
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "the calls never waited for anything"
        );
    }

    #[test]
    fn a_script_that_never_marks_leaves_no_window() {
        let (hub, _) = hub(true);
        run_script(extension(Some(Arc::clone(&hub))), "local x = 1\n");
        assert_eq!(hub.report().window, None);
    }

    #[test]
    fn a_headless_run_accepts_the_call_and_does_nothing() {
        run_script(extension(None), "diagnostics.metrics_mark()\n");
    }
}
