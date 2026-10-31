//! The backend trait: what `ck-window` needs from a windowing library, implemented on the main thread.

use std::sync::Arc;
use std::time::Instant;

use crate::desc::WindowDesc;
use crate::notifier::NativeWake;

/// A native event a backend reports from `pump`. Callbacks only record these: the loop never tears
/// anything down from inside the pump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeEvent {
    /// The close button was pressed.
    CloseButton,
    /// Escape was pressed (including key repeat). Only the GLFW backend still reports it, until milestone 5.5 gives
    /// it normalised keys: the native macOS backend reports [`Key`](Self::Key) and the close action is a binding.
    EscapePressed,
    /// A physical key changed state, normalised on the main thread. `repeat` is the system's auto-repeat flag. The
    /// loop feeds these to its `ck-input` mapper in the order they arrive.
    Key {
        /// The physical key.
        key: ck_input::Key,
        /// Whether the key went down (`false`: up).
        down: bool,
        /// Whether this is an auto-repeat of a held key.
        repeat: bool,
    },
    /// The native layer's bounded record of key transitions overflowed: no ordered detail of the span is delivered,
    /// only the final state of every key and the taps lost (`docs/scene-design.md` §10.2). Applied in one step
    /// ([`ck_input::Mapper::resync`]).
    KeyResync(ck_input::KeyStates),
    /// The window lost key focus: every held key is released.
    FocusLost,
    /// The window gained key focus.
    FocusGained,
    /// The window was minimised.
    Minimized,
    /// The window was restored from minimisation.
    Restored,
    /// The drawable's size in pixels changed (for example, a scale-factor change).
    PixelSizeChanged {
        /// The new width in pixels.
        width: u32,
        /// The new height in pixels.
        height: u32,
    },
    /// The window needs to be drawn again.
    NeedsDisplay,
}

/// Whether `pump` may wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Process what is pending and return; do not wait.
    Poll,
    /// Wait for an event or a wake, returning at once if a wake is already queued (the wake is sticky,
    /// see [`NativeWake`]).
    UntilEvent,
}

/// A size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelSize {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// What a successful `create_window` hands back.
#[derive(Debug)]
pub struct CreatedWindow<H> {
    /// The native handles the presenter needs. The loop passes them on without looking at them.
    pub handles: H,
    /// The drawable's initial size in pixels.
    pub pixel_size: PixelSize,
}

/// A windowing library, driven by `ck-window` on the main thread. It need not be `Send`.
pub trait Backend {
    /// The native handles of a created window, for the presenter.
    type Handles;

    /// Creates the fixed-size, non-resizable window.
    ///
    /// # Errors
    ///
    /// `Err` means **no window exists**: the backend has cleaned up its own partial objects before
    /// returning, and only [`terminate`](Self::terminate) may still be needed.
    fn create_window(&mut self, desc: &WindowDesc) -> Result<CreatedWindow<Self::Handles>, String>;

    /// The object the notifier arms with: valid from after `create_window` until `destroy_window`.
    fn wake_target(&self) -> Arc<dyn NativeWake>;

    /// Processes native events, calling `events` for each. A **capped** backend (the native path) stops
    /// after `cap` events and returns; a **draining** backend (GLFW) may exceed `cap`, since it processes
    /// every pending event in one call. Callbacks only record. The loop never calls
    /// [`destroy_window`](Self::destroy_window) or [`terminate`](Self::terminate) from inside `events`.
    fn pump(&mut self, events: &mut dyn FnMut(NativeEvent), wait: Wait, cap: usize);

    /// Whether this backend can wait until a monotonic deadline without polling.
    fn supports_deadline(&self) -> bool {
        false
    }

    /// Processes events and returns at or before `deadline`, unless an event or wake arrives first.
    /// Called only when [`supports_deadline`](Self::supports_deadline) is true.
    fn pump_until(
        &mut self,
        _events: &mut dyn FnMut(NativeEvent),
        _deadline: Instant,
        _cap: usize,
    ) {
        debug_assert!(
            false,
            "pump_until called on a backend without deadline support"
        );
    }

    /// Destroys the window. Called only by the loop's teardown, outside `pump`.
    fn destroy_window(&mut self);

    /// Terminates the library. Idempotent, and safe when no window was ever created. Called only by the
    /// loop's teardown, outside `pump`.
    fn terminate(&mut self);
}
