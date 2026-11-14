//! The backend: `ck-window`'s [`Backend`] on `AppKit`.
//!
//! Production window creation requests application and key-window activation. Test workers that enable
//! `ck-window`'s test-support feature can disable those requests before creating a backend; that exception
//! is process-local and unavailable in normal builds.

use std::ffi::{CStr, c_char, c_void};
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ck_input::{Key, KeyStates};
use ck_log::Level;
use ck_window::{
    Backend, CreatedWindow, NativeEvent, NativeWake, PixelSize, ScreenSnapshot, Wait, WindowDesc,
};

use crate::ffi::{self, RawEvents};
use crate::wake::CocoaWake;

/// The close source of `native/window.h`. (Escape is no longer one: it is the `close` binding of the input mapper.)
const CLOSE_BUTTON: i32 = 1;

/// How long a record may wait for the logger: the main thread never waits on it longer.
const LOG_WAIT: Duration = Duration::from_millis(100);

/// The error text when the native layer left no NUL inside the buffer it was given.
const UNTERMINATED: &str = "native error message was not NUL-terminated";

/// Reads the native error message from the buffer the native call filled. The read stops at the first
/// NUL and never goes past the buffer: a buffer with no NUL gets the fixed fallback text.
fn message(buffer: &[u8]) -> String {
    match CStr::from_bytes_until_nul(buffer) {
        Ok(text) => text.to_string_lossy().into_owned(),
        Err(_) => UNTERMINATED.to_owned(),
    }
}

/// What one [`pump`](Backend::pump) handed over: the key and focus entries in recorded order (at most the pump's
/// cap), at most one close request, one size change (the latest) and one needs-display, with the counts
/// behind them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PumpReport {
    /// The normalised key and focus entries, in the order they were recorded: [`NativeEvent::Key`],
    /// [`NativeEvent::KeyResync`] (after an overflow, in place of all ordered detail), [`NativeEvent::FocusLost`]
    /// and [`NativeEvent::FocusGained`].
    pub input: Vec<NativeEvent>,
    /// The close request, if any: [`NativeEvent::CloseButton`].
    pub close: Option<NativeEvent>,
    /// The latest content size in points and drawable size in pixels, when it changed: one for a live resize, however long, and
    /// one for each zoom, full-screen transition or change of backing scale that is not part of one.
    pub size: Option<((u32, u32), PixelSize)>,
    /// When a smaller screen lowered the content minimum: the points asked for and the points used.
    pub min_clamped: Option<((u32, u32), (u32, u32))>,
    /// Whether the window needs to be drawn again.
    pub needs_display: bool,
    /// Whether the window was minimised during this pump.
    pub minimized: bool,
    /// Whether the window was restored during this pump.
    pub restored: bool,
    /// Further close-type presses absorbed behind the first (saturating).
    pub absorbed_close_presses: u32,
    /// The events the pump took off the native queue, this backend's own wake events included.
    pub native_events: u64,
    /// The key transitions discarded into the loss latches since the backend was made (a diagnostic; nothing is
    /// logged per key).
    pub overflow_total: u64,
}

/// The native handles of a created window, for the presenter.
#[derive(Debug)]
pub struct CocoaHandles {
    layer: NonNull<c_void>,
}

impl CocoaHandles {
    /// The window's `CAMetalLayer` (the content view's backing layer). It is valid until
    /// [`destroy_window`](Backend::destroy_window) and is not retained for the caller: whoever turns it
    /// into a drawing surface does so on the main thread, within that lifetime.
    #[must_use]
    pub fn layer(&self) -> *mut c_void {
        self.layer.as_ptr()
    }
}

/// The native macOS window backend. It owns the application's one window and must stay on the process's
/// main thread: it is neither `Send` nor `Sync`. The one thing that crosses threads is the wake target
/// [`wake_target`](Backend::wake_target) returns.
#[derive(Debug)]
pub struct CocoaBackend {
    handle: NonNull<c_void>,
    window: bool,
    wake: Option<Arc<CocoaWake>>,
    /// Neither `Send` nor `Sync`: every call stays on the thread that created the backend.
    _main_thread: PhantomData<*const ()>,
}

impl CocoaBackend {
    /// Creates the backend: the shared application with the regular activation policy and no main menu
    /// (so Cmd-Q and every other menu key equivalent does nothing), launched once for the process, as
    /// `[NSApp run]` would. No window exists yet.
    ///
    /// # Errors
    ///
    /// An error when not called on the process's main thread (checked before any `AppKit` object is
    /// touched), or when the application cannot be set up.
    pub fn new() -> Result<Self, String> {
        // SAFETY: a plain function returning a constant; it touches no state.
        let native_size = unsafe { ffi::ck_win_events_size() };
        if native_size != size_of::<RawEvents>() {
            return Err(format!(
                "the native event record is {native_size} bytes, the Rust one {}",
                size_of::<RawEvents>()
            ));
        }
        let mut error = [0_u8; 512];
        // SAFETY: `error` is valid for `error.len()` bytes for this synchronous call, and the native
        // side writes at most that many, NUL-terminated. The thread check happens before any AppKit use.
        let handle =
            unsafe { ffi::ck_win_create(error.as_mut_ptr().cast::<c_char>(), error.len()) };
        let handle = NonNull::new(handle).ok_or_else(|| message(&error))?;
        // The wake target is taken here, on the main thread, and held until `terminate`.
        let Some(wake) = CocoaWake::new(handle) else {
            // SAFETY: the handle was just created and nothing else holds it; this consumes it, once.
            unsafe { ffi::ck_win_free(handle.as_ptr()) };
            return Err("the native layer returned no wake target".to_owned());
        };
        Ok(Self {
            handle,
            window: false,
            wake: Some(Arc::new(wake)),
            _main_thread: PhantomData,
        })
    }

    /// One pump (see [`Backend::pump`]), returning what it handed over together with the counts behind
    /// it. The absorbed close presses are also logged once as a `debug` record in category
    /// `window.cocoa`.
    pub fn pump_with_report(&mut self, wait: Wait, cap: usize) -> PumpReport {
        let raw = self.pump_raw(wait, cap);
        note_absorbed(raw.close_absorbed);
        note_min_clamp(min_clamp_of_raw(&raw));
        PumpReport {
            input: input_events(&raw),
            close: match raw.close_source {
                CLOSE_BUTTON => Some(NativeEvent::CloseButton),
                _ => None,
            },
            size: size_of_raw(&raw),
            min_clamped: min_clamp_of_raw(&raw),
            needs_display: raw.needs_display != 0,
            minimized: raw.minimized != 0,
            restored: raw.restored != 0,
            absorbed_close_presses: raw.close_absorbed,
            native_events: raw.native_events,
            overflow_total: raw.overflow_total,
        }
    }

    fn pump_raw(&mut self, wait: Wait, cap: usize) -> RawEvents {
        let mut events = RawEvents::default();
        // SAFETY: the handle is live (it is freed only when this value drops), this runs on the main
        // thread (the backend is not Send), and `events` is valid for the call.
        unsafe {
            ffi::ck_win_pump(
                self.handle.as_ptr(),
                i32::from(wait == Wait::UntilEvent),
                cap,
                &raw mut events,
            );
        }
        events
    }

    fn pump_raw_until(&mut self, deadline: Instant, cap: usize) -> RawEvents {
        let mut events = RawEvents::default();
        let timeout = deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f64();
        // SAFETY: the handle is live and this runs on the main thread. `events` is valid for the call;
        // the native side converts this fresh monotonic remainder to an absolute NSDate for this wait.
        unsafe {
            ffi::ck_win_pump_until(self.handle.as_ptr(), cap, timeout, &raw mut events);
        }
        events
    }
}

/// The size change of one pump, if it had one: the content size in points and the drawable's size in pixels, read together.
fn size_of_raw(raw: &RawEvents) -> Option<((u32, u32), PixelSize)> {
    (raw.size_changed != 0).then_some((
        (raw.point_width, raw.point_height),
        PixelSize {
            width: raw.pixel_width,
            height: raw.pixel_height,
        },
    ))
}

/// The content minimum a smaller screen lowered during one pump: the points asked for and the points used.
fn min_clamp_of_raw(raw: &RawEvents) -> Option<((u32, u32), (u32, u32))> {
    (raw.min_clamped != 0).then_some((
        (raw.min_requested_width, raw.min_requested_height),
        (raw.min_used_width, raw.min_used_height),
    ))
}

/// Logs the one line for a content minimum a smaller screen lowered, through `log_within`.
fn note_min_clamp(clamp: Option<((u32, u32), (u32, u32))>) {
    if let Some((requested, used)) = clamp {
        let at = Instant::now() + LOG_WAIT;
        let _ = ck_log::log_within!(
            Level::Info,
            at,
            "window.cocoa",
            "the minimum of {}x{} points does not fit the screen: using {}x{}",
            requested.0,
            requested.1,
            used.0,
            used.1
        );
    }
}

/// The key and focus entries of one pump as events, in order. An entry the native layer numbers outside the key set
/// (it cannot) is dropped.
fn input_events(raw: &RawEvents) -> Vec<NativeEvent> {
    let count = usize::try_from(raw.out_count).map_or(0, |count| count.min(raw.out.len()));
    let mut events: Vec<NativeEvent> = raw.out[..count]
        .iter()
        .filter_map(|entry| match entry.kind {
            ffi::OUT_KEY => Key::ALL
                .get(usize::from(entry.key))
                .map(|&key| NativeEvent::Key {
                    key,
                    down: entry.down != 0,
                    repeat: entry.repeat != 0,
                }),
            ffi::OUT_FOCUS_LOST => Some(NativeEvent::FocusLost),
            ffi::OUT_FOCUS_GAINED => Some(NativeEvent::FocusGained),
            ffi::OUT_RESYNC => Some(NativeEvent::KeyResync(KeyStates::from_words(
                raw.resync.held,
                raw.resync.pressed,
                raw.resync.released,
            ))),
            _ => None,
        })
        .collect();
    let typed_count =
        usize::try_from(raw.typed_count).map_or(0, |count| count.min(raw.typed.len()));
    events.extend(
        raw.typed[..typed_count]
            .iter()
            .copied()
            .filter_map(typed_event),
    );
    if raw.typed_lost != 0 {
        events.push(NativeEvent::TypedLost);
    }
    let pointer_count = usize::try_from(raw.pointer_count)
        .unwrap_or(0)
        .min(raw.pointers.len());
    for pointer in &raw.pointers[..pointer_count] {
        let position = ck_input::PointerPosition {
            x: pointer.x,
            y: pointer.y,
        };
        let event = match pointer.kind {
            ffi::POINTER_DOWN => Some(ck_input::PointerEvent::Down(position)),
            ffi::POINTER_MOVE => Some(ck_input::PointerEvent::Move(position)),
            ffi::POINTER_UP => Some(ck_input::PointerEvent::Up(position)),
            ffi::POINTER_BUTTON_DOWN => match pointer.button {
                ffi::POINTER_BUTTON_RIGHT => Some(ck_input::PointerEvent::ButtonDown {
                    button: ck_input::PointerButton::Right,
                    position,
                }),
                ffi::POINTER_BUTTON_MIDDLE => Some(ck_input::PointerEvent::ButtonDown {
                    button: ck_input::PointerButton::Middle,
                    position,
                }),
                _ => None,
            },
            ffi::POINTER_BUTTON_UP => match pointer.button {
                ffi::POINTER_BUTTON_RIGHT => Some(ck_input::PointerEvent::ButtonUp {
                    button: ck_input::PointerButton::Right,
                    position,
                }),
                ffi::POINTER_BUTTON_MIDDLE => Some(ck_input::PointerEvent::ButtonUp {
                    button: ck_input::PointerButton::Middle,
                    position,
                }),
                _ => None,
            },
            ffi::POINTER_SCROLL => Some(ck_input::PointerEvent::Scroll {
                position,
                amount: pointer.amount,
                unit: if pointer.unit == 1 {
                    ck_input::ScrollUnit::Points
                } else {
                    ck_input::ScrollUnit::Lines
                },
            }),
            _ => None,
        };
        if let Some(event) = event {
            events.push(NativeEvent::Pointer(event));
        }
    }
    if raw.pointer_overflow != 0 {
        events.push(NativeEvent::PointerOverflow {
            position: ck_input::PointerPosition {
                x: raw.pointer_x,
                y: raw.pointer_y,
            },
            held: raw.pointer_held != 0,
        });
    }
    events
}

/// One typed entry of a pump as an event: a committed character (a Unicode scalar value) or one of the five edit commands. An entry
/// the native layer numbers outside these (it cannot) is dropped.
fn typed_event(entry: ffi::RawTyped) -> Option<NativeEvent> {
    use ck_input::TypedEntry;
    let typed = match entry.kind {
        ffi::TYPED_CHAR => TypedEntry::Char(char::from_u32(entry.value)?),
        ffi::TYPED_BACKSPACE => TypedEntry::Backspace,
        ffi::TYPED_ENTER => TypedEntry::Enter,
        ffi::TYPED_TAB => TypedEntry::Tab,
        ffi::TYPED_LEFT => TypedEntry::Left,
        ffi::TYPED_RIGHT => TypedEntry::Right,
        _ => return None,
    };
    Some(NativeEvent::Typed(typed))
}

/// Records one `debug` line for the close presses a pump absorbed, through `log_within`.
fn note_absorbed(count: u32) {
    if count > 0 {
        let at = Instant::now() + LOG_WAIT;
        let _ = ck_log::log_within!(
            Level::Debug,
            at,
            "window.cocoa",
            "{count} further close presses were absorbed before the loop took the first"
        );
    }
}

impl Backend for CocoaBackend {
    type Handles = CocoaHandles;

    fn create_window(&mut self, desc: &WindowDesc) -> Result<CreatedWindow<CocoaHandles>, String> {
        if self.window {
            return Err("a window already exists".to_owned());
        }
        let mut error = [0_u8; 512];
        let mut layer: *mut c_void = std::ptr::null_mut();
        let (mut point_width, mut point_height) = (0_u32, 0_u32);
        let (mut pixel_width, mut pixel_height) = (0_u32, 0_u32);
        let content_min = desc.min.unwrap_or((0, 0));
        let pixel_min = desc
            .pixel_request
            .and_then(|request| request.min)
            .unwrap_or((0, 0));
        let clear = desc.clear;
        // SAFETY: the handle is live and this runs on the main thread. The title bytes are valid for
        // `len` bytes for this synchronous call. The out pointers and the error buffer are valid for
        // the call, and the native side writes at most `error.len()` bytes into the buffer.
        let result = unsafe {
            ffi::ck_win_create_window(
                self.handle.as_ptr(),
                desc.title.as_ptr().cast::<c_char>(),
                desc.title.len(),
                desc.width,
                desc.height,
                i32::from(ck_window::window_activation_allowed()),
                i32::from(!ck_window::window_mouse_input_allowed()),
                i32::from(desc.resizable),
                content_min.0,
                content_min.1,
                pixel_min.0,
                pixel_min.1,
                clear.as_ptr(),
                &raw mut layer,
                &raw mut point_width,
                &raw mut point_height,
                &raw mut pixel_width,
                &raw mut pixel_height,
                error.as_mut_ptr().cast::<c_char>(),
                error.len(),
            )
        };
        if result != 0 {
            return Err(message(&error));
        }
        let Some(layer) = NonNull::new(layer) else {
            // No window may be left behind when `Err` is returned.
            self.destroy_window();
            return Err("the native layer returned no CAMetalLayer".to_owned());
        };
        self.window = true;
        Ok(CreatedWindow {
            handles: CocoaHandles { layer },
            pixel_size: PixelSize {
                width: pixel_width,
                height: pixel_height,
            },
        })
    }

    fn resize_window(&mut self, points: (u32, u32)) {
        if self.window {
            // SAFETY: the backend handle owns the live window and this method runs on the main thread.
            unsafe { ffi::ck_win_resize(self.handle.as_ptr(), points.0, points.1) };
        }
    }

    fn main_screen(&mut self) -> ScreenSnapshot {
        let mut backing = 1.0_f64;
        let (mut visible_width, mut visible_height) = (0_u32, 0_u32);
        // SAFETY: the handle is live and this runs on the main thread (the backend is not Send). The out pointers are valid for
        // this synchronous call.
        unsafe {
            ffi::ck_win_main_screen(
                self.handle.as_ptr(),
                &raw mut backing,
                &raw mut visible_width,
                &raw mut visible_height,
            );
        }
        ScreenSnapshot {
            backing,
            visible_points: (visible_width, visible_height),
        }
    }

    fn wake_target(&self) -> Arc<dyn NativeWake> {
        // After `terminate` there is no target: a wake then reaches nothing, as the contract says.
        self.wake
            .clone()
            .map_or_else(|| Arc::new(NoWake) as Arc<dyn NativeWake>, |wake| wake)
    }

    fn pump(&mut self, events: &mut dyn FnMut(NativeEvent), wait: Wait, cap: usize) {
        let report = self.pump_with_report(wait, cap);
        // The key and focus entries in recorded order, then at most one close, one size and one needs-display;
        // callbacks only record.
        for event in report.input {
            events(event);
        }
        if let Some(close) = report.close {
            events(close);
        }
        if let Some((points, pixels)) = report.size {
            events(NativeEvent::SizeChanged { points, pixels });
        }
        if report.needs_display {
            events(NativeEvent::NeedsDisplay);
        }
        if report.minimized {
            events(NativeEvent::Minimized);
        }
        if report.restored {
            events(NativeEvent::Restored);
        }
    }

    fn supports_deadline(&self) -> bool {
        true
    }

    fn pump_until(&mut self, events: &mut dyn FnMut(NativeEvent), deadline: Instant, cap: usize) {
        let raw = self.pump_raw_until(deadline, cap);
        note_absorbed(raw.close_absorbed);
        note_min_clamp(min_clamp_of_raw(&raw));
        for event in input_events(&raw) {
            events(event);
        }
        if raw.close_source == CLOSE_BUTTON {
            events(NativeEvent::CloseButton);
        }
        if let Some((points, pixels)) = size_of_raw(&raw) {
            events(NativeEvent::SizeChanged { points, pixels });
        }
        if raw.needs_display != 0 {
            events(NativeEvent::NeedsDisplay);
        }
        if raw.minimized != 0 {
            events(NativeEvent::Minimized);
        }
        if raw.restored != 0 {
            events(NativeEvent::Restored);
        }
    }

    fn set_text_input(&mut self, enabled: bool) {
        // SAFETY: the handle is live and this runs on the main thread, outside a pump.
        unsafe { ffi::ck_win_set_text_input(self.handle.as_ptr(), i32::from(enabled)) };
    }

    fn destroy_window(&mut self) {
        // SAFETY: the handle is live and this runs on the main thread.
        unsafe { ffi::ck_win_destroy_window(self.handle.as_ptr()) };
        self.window = false;
    }

    fn terminate(&mut self) {
        // SAFETY: the handle is live and this runs on the main thread; the native call is idempotent.
        unsafe { ffi::ck_win_terminate(self.handle.as_ptr()) };
        self.window = false;
        // The backend's own reference to the wake target goes here, after the loop's successful disarm.
        self.wake = None;
    }
}

impl Drop for CocoaBackend {
    fn drop(&mut self) {
        // SAFETY: this consumes the handle, once, on the main thread (the backend is not Send). The native
        // side terminates first, which is idempotent.
        unsafe { ffi::ck_win_free(self.handle.as_ptr()) };
    }
}

/// The wake target of a backend that has none: it wakes nothing.
struct NoWake;

impl NativeWake for NoWake {
    fn wake(&self) {}
}

#[cfg(test)]
mod pointer_tests {
    use super::*;

    #[test]
    fn raw_pointer_kinds_and_button_numbers_map_to_only_supported_events() {
        let mut raw = RawEvents::default();
        let position = ck_input::PointerPosition { x: 12, y: 34 };
        raw.pointer_count = 8;
        raw.pointers[..8].copy_from_slice(&[
            ffi::RawPointer {
                kind: ffi::POINTER_DOWN,
                x: 12,
                y: 34,
                ..ffi::RawPointer::default()
            },
            ffi::RawPointer {
                kind: ffi::POINTER_BUTTON_DOWN,
                button: ffi::POINTER_BUTTON_MIDDLE,
                x: 12,
                y: 34,
                ..ffi::RawPointer::default()
            },
            ffi::RawPointer {
                kind: ffi::POINTER_BUTTON_UP,
                button: ffi::POINTER_BUTTON_MIDDLE,
                x: 12,
                y: 34,
                ..ffi::RawPointer::default()
            },
            ffi::RawPointer {
                kind: ffi::POINTER_BUTTON_DOWN,
                button: ffi::POINTER_BUTTON_RIGHT,
                x: 12,
                y: 34,
                ..ffi::RawPointer::default()
            },
            ffi::RawPointer {
                kind: ffi::POINTER_UP,
                x: 12,
                y: 34,
                ..ffi::RawPointer::default()
            },
            ffi::RawPointer {
                kind: ffi::POINTER_BUTTON_DOWN,
                button: 0,
                x: 12,
                y: 34,
                ..ffi::RawPointer::default()
            },
            ffi::RawPointer {
                kind: ffi::POINTER_BUTTON_UP,
                button: 3,
                x: 12,
                y: 34,
                ..ffi::RawPointer::default()
            },
            ffi::RawPointer {
                kind: 6,
                button: ffi::POINTER_BUTTON_RIGHT,
                x: 12,
                y: 34,
                ..ffi::RawPointer::default()
            },
        ]);

        let pointers: Vec<_> = input_events(&raw)
            .into_iter()
            .filter_map(|event| match event {
                NativeEvent::Pointer(pointer) => Some(pointer),
                _ => None,
            })
            .collect();
        assert_eq!(
            pointers,
            [
                ck_input::PointerEvent::Down(position),
                ck_input::PointerEvent::ButtonDown {
                    button: ck_input::PointerButton::Middle,
                    position
                },
                ck_input::PointerEvent::ButtonUp {
                    button: ck_input::PointerButton::Middle,
                    position
                },
                ck_input::PointerEvent::ButtonDown {
                    button: ck_input::PointerButton::Right,
                    position
                },
                ck_input::PointerEvent::Up(position),
            ]
        );
    }

    #[test]
    fn native_events_size_matches_the_rust_abi() {
        // SAFETY: this returns the native compile-time size of the same header-defined struct.
        assert_eq!(unsafe { ffi::ck_win_events_size() }, size_of::<RawEvents>());
    }
}
