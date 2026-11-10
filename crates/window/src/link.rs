//! The UI link and the presenter: the loop's views of the UI VM and of the render thread.

use std::time::Duration;

use crate::backend::PixelSize;
use crate::desc::WindowDesc;

/// What caused a close request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseSource {
    /// The window's close button.
    Button,
    /// The Escape key.
    Key,
}

/// An event delivered to the UI VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    /// The first frame was queued for presentation. Sent once.
    Opened,
    /// A close was requested. At most one is outstanding.
    CloseRequested {
        /// What caused it.
        source: CloseSource,
    },
    /// One paced UI update with the immutable input state captured when it was posted.
    Update(UpdateEvent),
}

/// Window geometry carried by an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowMetrics {
    /// Window dimensions in points.
    pub points: (u32, u32),
    /// Drawable dimensions in pixels.
    pub pixels: PixelSize,
}

/// One admitted UI update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateEvent {
    /// Monotonic sequence, starting at one for each opened window.
    pub sequence: u64,
    /// Time since the previous successfully posted update; zero for the first update after activation.
    pub elapsed: Duration,
    /// Bounded simulation delta.
    pub dt: Duration,
    /// Window geometry at posting time.
    pub metrics: WindowMetrics,
}

/// One input tick: the input state taken on the loop's own schedule, independent of the UI VM (design `ui-design.md` §3.12).
///
/// A tick runs when the input cursor of the update grid has a due opportunity, however many fell due since the last one, and
/// whether or not an update is outstanding, the link refused a post as full, a callback is registered or a close request is
/// outstanding. The link consumes the snapshot; the loop acknowledges the mapper only after [`UiLink::input`] returned `Ok`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputTick {
    /// Time since the previous tick, capped by `[frame] max_delta_seconds`; zero for the first tick after activation.
    pub dt: Duration,
    /// Window geometry at the tick.
    pub metrics: WindowMetrics,
    /// The input snapshot, taken at the tick.
    pub input: ck_input::InputSnapshot,
}

/// The UI's answer to an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The callback returned normally (including any return values).
    Returned,
    /// Close the window.
    Accept,
    /// Keep the window open.
    Decline,
    /// No callback is registered, which means accept.
    NotRegistered,
    /// The callback failed; the message carries its name and traceback.
    Failed {
        /// The failure.
        message: String,
    },
}

/// An answer, paired with the event it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiAnswer {
    /// The event this answers.
    pub event: UiEvent,
    /// The answer. For [`UiEvent::Opened`] only [`Answer::Failed`] matters.
    pub answer: Answer,
}

/// Why an event could not be posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostError {
    /// The UI VM has no room now; the event waits and is posted again on a later pass.
    Full,
    /// The UI VM has ended.
    Disconnected,
}

/// The loop's view of the UI VM. `codekarl` adapts `ck-lua`'s posted calls to it. Nothing here waits.
pub trait UiLink {
    /// Posts an event without waiting.
    ///
    /// # Errors
    ///
    /// [`PostError::Full`] while the VM cannot take it now, [`PostError::Disconnected`] once it ended.
    fn post(&mut self, event: UiEvent) -> Result<(), PostError>;

    /// Consumes one input tick on the main thread; never waits and never calls the UI VM's Lua.
    ///
    /// The default consumes nothing, for links that need no input outside their posted events (tests, headless fakes).
    ///
    /// # Errors
    ///
    /// The message ends the run as a failed callback does ([`LoopEnd::CallbackFailed`](crate::LoopEnd::CallbackFailed)); the
    /// mapper is then not acknowledged.
    fn input(&mut self, tick: &InputTick) -> Result<(), String> {
        let _ = tick;
        Ok(())
    }

    /// The next answer, if one has arrived; never waits.
    fn take_answer(&mut self) -> Option<UiAnswer>;

    /// Whether the UI VM's thread has ended.
    fn is_disconnected(&self) -> bool;
}

/// What the render thread reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenterReport {
    /// The first frame was queued for presentation (not proof that it is visible).
    FirstFrameQueued,
    /// Setup or drawing failed.
    Failed {
        /// The failure.
        message: String,
    },
}

/// The result of the render thread's cleanup, once it was observed finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupResult {
    /// Every wait proved the objects unused and every object was destroyed.
    Proven,
    /// A panic, a missing or failed cleanup result, or an unproven GPU wait.
    Unproven {
        /// Why.
        reason: String,
    },
}

/// The loop's view of the render thread; backends implement it (issues 6 and 9).
pub trait Presenter {
    /// The native handles a backend's `create_window` hands over.
    type Handles;

    /// Starts the render thread and returns at once.
    ///
    /// # Errors
    ///
    /// `Err` means **no thread was spawned and no GPU object exists**. Once a thread is spawned `start`
    /// returns `Ok`, and any later failure (including one during the thread's own setup) is reported
    /// through [`take_report`](Self::take_report) as [`PresenterReport::Failed`].
    fn start(
        &mut self,
        handles: Self::Handles,
        desc: &WindowDesc,
        pixel_size: PixelSize,
    ) -> Result<(), String>;

    /// Sets the clear colour. Never waits (a latest-value slot).
    fn set_clear(&mut self, rgb: [f32; 3]);

    /// Sets the window's size: the content size in points and the drawable's size in pixels, together. Called once for each size
    /// the loop learned, before the pass's input tick and update that carry it. Never waits.
    fn set_size(&mut self, points: (u32, u32), pixels: PixelSize);

    /// Asks for a redraw. Never waits.
    fn request_redraw(&mut self);

    /// The next report, if there is one; never waits.
    ///
    /// Reports are queued in the order the render thread publishes them, and a report is queued before
    /// the thread's end can be observed: every report the thread published is returned by `take_report`
    /// no later than after a [`poll_end`](Self::poll_end) call returned `Some`, **including** a
    /// [`PresenterReport::Failed`] published while the thread stopped or in that very call. Once
    /// `poll_end` has returned `Some` the thread has ended, so `take_report` yields a **finite** sequence
    /// that ends in `None`; the loop then reads it to its end (see the crate documentation), however many
    /// reports it holds, so a presenter must not discard a report because a stop was requested. A presenter
    /// that never stops reporting is malformed: the loop reads a large fixed number of reports and reports
    /// that as a failure. Taking a report never releases anything.
    fn take_report(&mut self) -> Option<PresenterReport>;

    /// Asks the render thread to stop. Never waits. The thread may publish [`PresenterReport::Failed`]
    /// while it stops and still end [`CleanupResult::Proven`]: a failed operation and released resources
    /// are separate facts, and the first is not lost because the second holds.
    fn request_stop(&mut self);

    /// Waits at most `within` for the render thread to end. `Some` means it was **observed finished**,
    /// and its result is final: every report it ever published is already available to
    /// [`take_report`](Self::take_report), so the loop reads the queue to its end after the first `Some`,
    /// without waiting for more. `None` after the teardown deadline means
    /// the end was never observed, and the loop then takes only what is already queued.
    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult>;
}
