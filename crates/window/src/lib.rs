//! The window core: a generic, platform-free main loop for one fixed-size window.
//!
//! `ck-window` depends on `ck-log` and `ck-input` (the portable key mapper) only. It knows nothing of Lua, `ck-runtime`, `AppKit`, GLFW, Metal or
//! Vulkan: backends implement [`Backend`] and [`Presenter`], and the executable adapts the UI VM to
//! [`UiLink`]. It contains no `unsafe` code and no platform code.
//!
//! # Threads
//!
//! The **main thread** runs [`run`] and owns the backend (the window and the native library) and the
//! window's description. A **render thread** (behind [`Presenter`]) owns the GPU. The **UI VM's thread**
//! writes requests ([`RequestWriter`]) and receives events ([`UiLink`]). They talk only through messages,
//! and nothing waits for another thread's window work.
//!
//! # The rules (design §4.2 and §4.3)
//!
//! - **Requests are state, not a queue.** The [`Requests`] slot holds the latest colour, a one-shot `open`
//!   and a one-shot `close`. A thousand `set_clear` calls cost one redraw with the last colour. The lock
//!   is held only to move a small value in or out. After the loop ended, writes return at once, change
//!   nothing visible and are counted ([`Requests::discarded`]).
//! - **Events have fixed bounds.** At most one [`UiEvent::CloseRequested`] is outstanding and
//!   [`UiEvent::Opened`] is sent once, so at most two events are ever pending. Posting never waits: while
//!   the link is `Full` the event waits in the loop and is posted again after the UI VM's next result
//!   wake. An event refused with `Full` does not keep the loop polling (the link frees a slot only when
//!   the UI VM produces a result, and that result's wake ends the loop's wait), so the main thread does
//!   not spin while the UI VM is busy.
//! - **Work per pass.** A pass is one return of the backend's pump. A capped backend stops after
//!   [`LoopSettings::native_events`] events; a draining backend (GLFW) may exceed the cap, since it
//!   processes every pending event in one call. After the pump returns, the loop's own work before the next
//!   quit check is bounded: one read of the request slot, at most [`LoopSettings::items`] presenter reports
//!   and at most that many UI answers. Callbacks only record, in constant work.
//! - **Quit observed versus teardown entered.** The quit flag is checked after the pump returns and between
//!   the following steps. Teardown, window destruction and library termination happen only in the outer
//!   loop, after the pump returned, never from inside a callback.
//! - **The notifier never loses a wake.** [`Notifier::wake`] sets a pending flag first. The loop sleeps
//!   (`Wait::UntilEvent`) only when the previous pass saw the presenter reports and the UI answers
//!   *exhausted* (not merely capped), no event waits to be offered (one refused with `Full` does not count)
//!   and no wake is pending; otherwise it polls,
//!   so a backlog left by a cap drains without another wake. [`NativeWake`] must be sticky, which closes the
//!   gap between the pending check and the wait.
//!
//! # The close protocol (design §6)
//!
//! A close button press or a rising edge of the `close` action (Escape by default; the loop's `ck-input` mapper turns
//! normalised [`NativeEvent::Key`] events into actions) produces one [`UiEvent::CloseRequested`] unless one is
//! already outstanding (later ones are absorbed, and counted in one `debug window:` record per pass). The
//! UI answers [`Answer::Accept`], [`Answer::Decline`], [`Answer::NotRegistered`] (which means accept) or
//! [`Answer::Failed`]. Accept closes the window, Decline leaves it open (and the request is no longer
//! outstanding), Failed ends the loop with [`LoopEnd::CallbackFailed`]. A `close` from the slot closes
//! without asking. Closing the only window quits the loop in milestone 3, but closing (teardown of the
//! presenter and window) and quitting (the loop returning) are separate steps.
//!
//! # Setup, passes and teardown
//!
//! [`run`] takes the slot's first batch as a whole: no `open` gives [`LoopEnd::NoWindow`], and `open` with
//! `close` gives [`LoopEnd::Closed`], both ending the slot and touching no backend; otherwise the window is
//! created with the open description and the batch's latest colour, the notifier is armed, and the presenter
//! started. A failure at any step goes to teardown with the failure as the end. Teardown, never from inside
//! `pump`: end the slot; for a started presenter, request stop and poll for its end until the deadline; only
//! when it is `Proven` (or was never started) disarm the notifier, and only when that succeeds destroy the
//! window and terminate. Otherwise **nothing is destroyed**, nothing is dropped, and the end is
//! [`LoopEnd::Unproven`].
//!
//! **Presenter failures during and after the end.** A presenter may report [`PresenterReport::Failed`]
//! and still prove its resources released, and a failure may be queued before a close or quit is seen (the
//! passes stop reading reports at once), or published while the presenter stops. So teardown, after
//! the wait for the presenter and before the notifier is disarmed or anything is destroyed, takes the
//! presenter's reports again, without waiting. Once the presenter was observed finished
//! ([`Presenter::poll_end`] returned `Some`) its thread has ended and its report queue is finite, so
//! teardown takes **every** report, behind any number of first-frame reports and whatever the per-pass
//! caps ([`LoopSettings::items`] bounds a pass, not this): a failure published before that
//! call returned is never missed. The only bound is a large fixed one against a malformed presenter that
//! never stops reporting; reaching it is itself reported as a failure (never a silent discard). At a
//! missed deadline only what is already queued is taken, still without waiting. Each failure
//! found is reported **exactly once**: a clean end ([`LoopEnd::Closed`] or [`LoopEnd::Quit`]) becomes
//! [`LoopEnd::PresenterFailed`] with the first, and every other one, and all of them next to an end that
//! is already a failure (which stays the end, the earlier failure keeping its place), get one `error`
//! record in category `window`, `the presenter failed: ...`. An `Unproven` end stays `Unproven`: the
//! failures found are recorded the same way and the teardown deadlines, the order and the no-release rule
//! are unchanged. `run` holds the backend and the presenter in `ManuallyDrop`, so no `Drop`
//! releases them: a panic unwinding through `run` releases nothing.
//!
//! Records go through `log_within` with a 100 ms wait, in category `window`: `info window opened`
//! (when the presenter reports its first frame was queued, which is not proof that it is visible),
//! `info window closed`, and the `debug` record of absorbed close requests.

mod backend;
mod desc;
mod link;
mod main_loop;
mod metrics;
mod notifier;
mod requests;
mod settings;

// The input types the window core's API is made of (`NativeEvent::Key`, `LoopSettings::with_bindings`, and the
// snapshot an `UpdateEvent` carries), so a caller needs no dependency of its own on `ck-input`.
pub use backend::{Backend, CreatedWindow, NativeEvent, PixelSize, Wait};
pub use ck_input::{
    ActionSnapshot, Bindings, BindingsError, Edge, InputSnapshot, Key, KeyStates, Transition,
};
pub use desc::{InvalidColour, MAX_DIMENSION, OpenError, WindowDesc};
pub use link::{
    Answer, CleanupResult, CloseSource, PostError, Presenter, PresenterReport, UiAnswer, UiEvent,
    UiLink, UpdateEvent, WindowMetrics,
};
pub use main_loop::{Clock, LoopEnd, run, run_with_clock};
pub use metrics::{
    EndError, LoopEndGuard, LoopMetrics, LoopReading, LoopWindowStats, MetricsWindow, RequestError,
    Tracked, UptimeNs, Windows,
};
pub use notifier::{ArmRefused, DisarmTimeout, NativeWake, Notifier, NotifierState};
pub use requests::{RequestChanges, RequestReader, RequestWriter, Requests};
pub use settings::{LoopSettings, SettingsError, UpdateControl, UpdateSettings};
