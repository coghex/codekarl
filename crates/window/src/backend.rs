//! The backend trait: what `ck-window` needs from a windowing library, implemented on the main thread.

use std::sync::Arc;
use std::time::Instant;

use crate::desc::{ScreenSnapshot, WindowDesc};
use crate::notifier::NativeWake;

/// A native event a backend reports from `pump`. Callbacks only record these: the loop never tears
/// anything down from inside the pump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeEvent {
    /// The close button was pressed.
    CloseButton,
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
    /// One ordered mouse event; pointer detail has its own bounded ring in `ck-input`.
    Pointer(ck_input::PointerEvent),
    /// Native pointer detail overflowed; the held state and latest position remain exact.
    PointerOverflow {
        /// Latest pointer position in the fixed 1/256-point representation.
        position: ck_input::PointerPosition,
        /// Whether the left button remains held.
        held: bool,
    },
    /// One typed entry (CK-314): a committed character or an edit command, in the order typed. The loop gathers it only while text
    /// input is on.
    Typed(ck_input::TypedEntry),
    /// The backend's own bounded store of typed entries overflowed and dropped the later ones.
    TypedLost,
    /// The window lost key focus: every held key is released.
    FocusLost,
    /// The window gained key focus.
    FocusGained,
    /// The window was minimised.
    Minimized,
    /// The window was restored from minimisation.
    Restored,
    /// The window's size changed: the end of a live resize, a zoom, a full-screen transition or a change of the screen's scale. It
    /// carries the content size in points and the drawable's size in pixels, read together, so that the loop, the presenter, the map
    /// and the UI always see one coherent size. A backend reports no size during a live resize, only at its end.
    SizeChanged {
        /// The content size in points: width and height.
        points: (u32, u32),
        /// The drawable's size in pixels.
        pixels: PixelSize,
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

    /// The main screen's scale and visible size, read once on the main thread after the backend exists and before any thread
    /// resolves a size in pixels against it ([`WindowDesc::resolve`]). Main thread only.
    fn main_screen(&mut self) -> ScreenSnapshot;

    /// Creates the window: fixed size unless the description is `resizable`, in which case it can be resized, zoomed and made full
    /// screen, and never goes below the description's minimum. Production backends may request application or key-window
    /// activation as part of the normal open path; test workers can suppress those requests through
    /// `ck-window`'s test-support-only activation switch.
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

    /// Turns the typed-entry route on or off (CK-314), on the main thread and outside `pump`. Either way it starts from nothing: text
    /// being composed and entries not yet reported are discarded. A backend with no typed route (the default) does nothing.
    fn set_text_input(&mut self, _enabled: bool) {}

    /// Destroys the window. Called only by the loop's teardown, outside `pump`.
    fn destroy_window(&mut self);

    /// Terminates the library. Idempotent, and safe when no window was ever created. Called only by the
    /// loop's teardown, outside `pump`.
    fn terminate(&mut self);
}
