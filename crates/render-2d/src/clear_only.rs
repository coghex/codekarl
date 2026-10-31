//! The clear-only backend: any [`Presenter`] as a [`Gpu2d`] that draws no quad.
//!
//! [`ClearOnly`] forwards every presenter call unchanged to the presenter it wraps, so the wrapped presenter's
//! reports, end and cleanup semantics are exactly its own. It is the `ck-render-2d` side of a backend that does
//! not draw textured quads yet (milestone 4's Vulkan path wraps the application's accepted adapter): its sink
//! accepts every snapshot and draws nothing, and a run that sets a quad is told so once.
//!
//! **The warning.** The first snapshot whose scene *requests* a quad (`Snapshot::requested`: also a quad that is
//! fully clipped or has no pixel extent) sets a sticky flag and then calls the loop's wake; nothing is logged inside
//! `publish`, which runs under the scene bridge's lock. The wake only sets a flag and posts a native wake (the
//! notifier's contract), and the notifier's pending flag makes it sticky, so a publication just before the loop
//! arms its wait is not lost. The loop's next pass calls `take_report` on the main thread, where the backend
//! (like any [`Gpu2d`] method) logs exactly one `warn render: textured quads on <name> arrive in milestone 4.5`
//! record per run, through `try_log`, which never waits (an ending run gives a refused warning up; a refused warning
//! is otherwise tried again at the next call). The backend name is a constructor argument,
//! so the record names the path that was configured.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ck_log::{Admission, Level};
use ck_window::{CleanupResult, PixelSize, Presenter, PresenterReport, WindowDesc};

use crate::{Gpu2d, QuadSink, Snapshot};

/// What the sink tells the backend: a quad was requested.
#[derive(Debug, Default)]
struct QuadSeen(AtomicBool);

/// The sink of [`ClearOnly`]: accepts every snapshot and draws nothing.
struct DiscardingSink {
    seen: Arc<QuadSeen>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for DiscardingSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("DiscardingSink")
    }
}

impl QuadSink for DiscardingSink {
    /// Notes that a quad was requested (and wakes the loop, the first time), and hands the snapshot straight
    /// back as the value it did not keep, so the bridge drops it after releasing its lock. It never logs,
    /// blocks or panics; the wake is the notifier's, which only sets a flag and posts a native wake.
    fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
        if snapshot.requested && !self.seen.0.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
        Some(snapshot)
    }
}

/// A presenter as a backend that draws no quad.
pub struct ClearOnly<P> {
    presenter: P,
    name: String,
    wake: Arc<dyn Fn() + Send + Sync>,
    seen: Arc<QuadSeen>,
    warned: bool,
}

impl<P: Presenter> ClearOnly<P> {
    /// Wraps `presenter`. `backend_name` (for example `glfw-vulkan`) is the name the warning gives, and `wake`
    /// is the loop's wake (the notifier's `wake`, as the application's adapters pass), called when the sink has
    /// something for the main thread to log.
    #[must_use]
    pub fn new(
        presenter: P,
        backend_name: impl Into<String>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            presenter,
            name: backend_name.into(),
            wake,
            seen: Arc::new(QuadSeen::default()),
            warned: false,
        }
    }

    /// The wrapped presenter.
    #[must_use]
    pub fn presenter(&self) -> &P {
        &self.presenter
    }

    /// The generation of the last frame drawn with a scene: always `None`, as nothing is drawn from one.
    #[must_use]
    pub fn drawn_generation(&self) -> Option<u64> {
        None
    }

    /// Logs the warning, once per run, if a quad was requested since the last look; says whether it tried.
    /// Called first thing by every [`Gpu2d`] method, all of which run on the main thread.
    ///
    /// **It never waits:** the record goes through `try_log`, which takes or drops it at once (a drop is counted
    /// by the logger). Off the end paths a dropped warning is tried again at the next call; on the end paths
    /// (`closing`: the stop request and the end polls) it is given up, as the run is over. A [`Gpu2d`]
    /// method's own deadline is therefore never spent on the logger.
    fn announce(&mut self, closing: bool) -> bool {
        if self.warned || !self.seen.0.load(Ordering::Acquire) {
            return false;
        }
        let admission = ck_log::try_log!(
            Level::Warn,
            "render",
            "textured quads on {} arrive in milestone 4.5",
            self.name
        );
        if admission != Admission::Dropped || closing {
            self.warned = true;
        }
        true
    }
}

impl<P: Presenter> Gpu2d for ClearOnly<P> {
    type Handles = P::Handles;

    fn new_sink(&mut self) -> Arc<dyn QuadSink> {
        self.announce(false);
        Arc::new(DiscardingSink {
            seen: Arc::clone(&self.seen),
            wake: Arc::clone(&self.wake),
        })
    }

    fn start(
        &mut self,
        handles: Self::Handles,
        desc: &WindowDesc,
        pixel_size: PixelSize,
        _sink: Arc<dyn QuadSink>,
    ) -> Result<(), String> {
        self.announce(false);
        self.presenter.start(handles, desc, pixel_size)
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        self.announce(false);
        self.presenter.set_clear(rgb);
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.announce(false);
        self.presenter.set_pixel_size(width, height);
    }

    fn request_redraw(&mut self) {
        self.announce(false);
        self.presenter.request_redraw();
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        self.announce(false);
        self.presenter.take_report()
    }

    fn request_stop(&mut self) {
        self.announce(true);
        self.presenter.request_stop();
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        let started = Instant::now();
        // A warning still pending costs a moment: that moment comes out of the caller's budget.
        let within = if self.announce(true) {
            within.saturating_sub(started.elapsed())
        } else {
            within
        };
        self.presenter.poll_end(within)
    }
}
