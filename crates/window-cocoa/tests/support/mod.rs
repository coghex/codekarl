//! Test support for `window_cocoa.rs`: safe wrappers over `tests/native/testing.m`.
//!
//! None of this is part of the library. The native half is built into its own archive that only the test
//! executable links (`build.rs`), and it acts on the process's one backend window by finding it through
//! the application's window list, so the library exports no handle, hook or control for it.

use std::ffi::{CStr, CString, c_char};
use std::time::Duration;

use ck_window::NativeEvent;
use ck_window_cocoa::PumpReport;

#[repr(C)]
struct RawInfo {
    has_window: i32,
    title: [u8; 256],
    content_width: f64,
    content_height: f64,
    titled: i32,
    closable: i32,
    miniaturizable: i32,
    resizable: i32,
    layer_is_metal: i32,
    layer_is_view_backing_layer: i32,
    layer_scale: f64,
    window_scale: f64,
    visible: i32,
    is_key: i32,
    first_responder_is_view: i32,
    has_delegate: i32,
    released_when_closed: i32,
    window_number: i64,
    has_main_menu: i32,
    menu_items: u32,
    menu_key_equivalents: u32,
}

#[repr(C)]
#[derive(Default)]
struct RawNativeKeys {
    held: [u64; 2],
    lost_pressed: [u64; 2],
    lost_released: [u64; 2],
    ring_length: u32,
    overflow: i32,
    focus_lost_pending: i32,
    focus_gained_pending: i32,
    overflow_total: u64,
}

#[repr(C)]
#[derive(Default)]
struct RawCounters {
    markers_dispatched: u64,
    wake_events_dispatched: u64,
    update_windows_calls: u64,
    finish_launching_calls: u64,
    wake_posts: u64,
    no_responder: u64,
    key_downs_seen: u64,
    recorded_in_pump: u64,
    filtered_keys: u64,
    pump_update_windows_calls: u64,
}

unsafe extern "C" {
    fn ck_win_test_install_application();
    fn ck_win_test_app_exists() -> i32;
    fn ck_win_test_ns_window_count() -> u64;
    fn ck_win_test_info(out: *mut RawInfo);
    fn ck_win_test_counters(out: *mut RawCounters);
    fn ck_win_test_set_first_responder(to_view: i32);
    fn ck_win_test_perform_close();
    fn ck_win_test_post_key(
        key_code: u16,
        modifier_flags: u64,
        repeat: i32,
        characters: *const c_char,
        marked: i32,
    ) -> i32;
    fn ck_win_test_post_keyboard_event(
        kind: i32,
        key_code: u16,
        modifier_flags: u64,
        marked: i32,
    ) -> i32;
    fn ck_win_test_install_input_filter();
    fn ck_win_test_start_typist();
    fn ck_win_test_post_marker(id: i64, at_start: i32);
    fn ck_win_test_post_marker_after(delay_seconds: f64, id: i64);
    fn ck_win_test_discard_wake_events() -> u64;
    fn ck_win_test_drain_events();
    fn ck_win_test_set_scale(scale: f64);
    fn ck_win_test_backing_changed();
    fn ck_win_test_deminiaturized();
    fn ck_win_test_miniaturized();
    fn ck_win_test_burst(button_presses: u32, pixel_changes: u32, needs_display: u32);
    fn ck_win_test_record_keys(pairs: u32, key_code: u16) -> i32;
    fn ck_win_test_native_keys(out: *mut RawNativeKeys);
    fn ck_win_test_key_table_count() -> u32;
    fn ck_win_test_key_table(
        position: u32,
        key_code: *mut u16,
        key: *mut u8,
        name: *mut c_char,
        name_capacity: u32,
    ) -> i32;
    fn ck_win_test_resign_key();
    fn ck_win_test_become_key();
    fn ck_win_test_set_absorbed(count: u32);
    fn ck_win_test_schedule_records(delay_seconds: f64, timers: u32, calls: u32, kind: i32);
    fn ck_win_test_post_recording_marker(kind: i32, pairs: u32, key_code: u16);
    fn ck_win_test_violate_main_thread();
}

// Every call below is made on the worker's main thread (or says otherwise), as `AppKit` requires; the
// native functions take no pointers except the ones named in each `SAFETY` comment.

/// Makes the application object a counting subclass. Call before the backend is created.
pub(crate) fn install_application() {
    // SAFETY: main thread; creates the shared application object.
    unsafe { ck_win_test_install_application() };
}

/// Whether the shared application object exists yet (it reads a global; any thread).
#[must_use]
pub(crate) fn app_exists() -> bool {
    // SAFETY: only reads the `NSApp` global.
    unsafe { ck_win_test_app_exists() != 0 }
}

/// How many windows `[NSApp windows]` lists.
#[must_use]
pub(crate) fn ns_window_count() -> u64 {
    // SAFETY: main thread.
    unsafe { ck_win_test_ns_window_count() }
}

/// The style mask's buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Style {
    /// The style mask has the title bar.
    pub(crate) titled: bool,
    /// The style mask has the close button.
    pub(crate) closable: bool,
    /// The style mask has the minimise button.
    pub(crate) miniaturizable: bool,
}

/// How the window behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Behaviour {
    /// The style mask has `NSWindowStyleMaskResizable` (it must not).
    pub(crate) resizable: bool,
    /// The window is released when closed (it must not be).
    pub(crate) released_when_closed: bool,
    /// The window has a delegate.
    pub(crate) has_delegate: bool,
}

/// Keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Focus {
    /// The content view is the first responder.
    pub(crate) first_responder_is_view: bool,
}

/// The content view's layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LayerInfo {
    /// It is a `CAMetalLayer`.
    pub(crate) is_metal: bool,
    /// It is the content view's own backing layer.
    pub(crate) is_backing_layer: bool,
    /// Its `contentsScale`.
    pub(crate) scale: f64,
}

/// The application's main menu: `AppKit` installs an empty placeholder when it finishes launching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MenuInfo {
    /// Items in it, submenus included.
    pub(crate) items: u32,
    /// Items with a key equivalent (there must be none, so Cmd-Q does nothing).
    pub(crate) key_equivalents: u32,
}

/// What the window looks like to `AppKit`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WindowInfo {
    /// Whether the backend has a window.
    pub(crate) has_window: bool,
    /// The window is visible.
    pub(crate) visible: bool,
    /// The window's title.
    pub(crate) title: String,
    /// The content size in points.
    pub(crate) content_size: (f64, f64),
    /// The buttons.
    pub(crate) style: Style,
    /// How it behaves.
    pub(crate) behaviour: Behaviour,
    /// Keyboard focus.
    pub(crate) focus: Focus,
    /// The content view's layer.
    pub(crate) layer: LayerInfo,
    /// The window's backing scale factor.
    pub(crate) window_scale: f64,
    /// The main menu.
    pub(crate) menu: MenuInfo,
}

/// Reads the window's properties.
#[must_use]
pub(crate) fn info() -> WindowInfo {
    let mut raw = RawInfo {
        has_window: 0,
        title: [0; 256],
        content_width: 0.0,
        content_height: 0.0,
        titled: 0,
        closable: 0,
        miniaturizable: 0,
        resizable: 0,
        layer_is_metal: 0,
        layer_is_view_backing_layer: 0,
        layer_scale: 0.0,
        window_scale: 0.0,
        visible: 0,
        is_key: 0,
        first_responder_is_view: 0,
        has_delegate: 0,
        released_when_closed: 0,
        window_number: 0,
        has_main_menu: 0,
        menu_items: 0,
        menu_key_equivalents: 0,
    };
    // SAFETY: main thread; `raw` is valid for the call, which fills the whole structure.
    unsafe { ck_win_test_info(&raw mut raw) };
    let title = CStr::from_bytes_until_nul(&raw.title)
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_default();
    WindowInfo {
        has_window: raw.has_window != 0,
        visible: raw.visible != 0,
        title,
        content_size: (raw.content_width, raw.content_height),
        style: Style {
            titled: raw.titled != 0,
            closable: raw.closable != 0,
            miniaturizable: raw.miniaturizable != 0,
        },
        behaviour: Behaviour {
            resizable: raw.resizable != 0,
            released_when_closed: raw.released_when_closed != 0,
            has_delegate: raw.has_delegate != 0,
        },
        focus: Focus {
            first_responder_is_view: raw.first_responder_is_view != 0,
        },
        layer: LayerInfo {
            is_metal: raw.layer_is_metal != 0,
            is_backing_layer: raw.layer_is_view_backing_layer != 0,
            scale: raw.layer_scale,
        },
        window_scale: raw.window_scale,
        menu: MenuInfo {
            items: raw.menu_items,
            key_equivalents: raw.menu_key_equivalents,
        },
    }
}

/// The application's and the window's counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Counters {
    /// Test events that reached `sendEvent:`.
    pub(crate) markers_dispatched: u64,
    /// Wake events that reached `sendEvent:` (the pump consumes its own, so this stays 0).
    pub(crate) wake_events_dispatched: u64,
    /// `updateWindows` calls, including the ones `AppKit` makes itself.
    pub(crate) update_windows_calls: u64,
    /// `finishLaunching` calls.
    pub(crate) finish_launching_calls: u64,
    /// Wake events posted to the application.
    pub(crate) wake_posts: u64,
    /// Key events that reached the window with no responder to take them (the beep path).
    pub(crate) no_responder: u64,
    /// Key-downs that reached the content view.
    pub(crate) key_downs_seen: u64,
    /// Timer callbacks that ran inside a pump.
    pub(crate) recorded_in_pump: u64,
    /// Unmarked keyboard events the input filter dropped.
    pub(crate) filtered_keys: u64,
    /// `updateWindows` calls made directly by the backend's pump (`AppKit`'s own calls are not counted).
    pub(crate) pump_update_windows_calls: u64,
}

/// Reads the counters.
#[must_use]
pub(crate) fn counters() -> Counters {
    let mut raw = RawCounters::default();
    // SAFETY: `raw` is valid for the call; the native side only reads atomics into it.
    unsafe { ck_win_test_counters(&raw mut raw) };
    Counters {
        markers_dispatched: raw.markers_dispatched,
        wake_events_dispatched: raw.wake_events_dispatched,
        update_windows_calls: raw.update_windows_calls,
        finish_launching_calls: raw.finish_launching_calls,
        wake_posts: raw.wake_posts,
        no_responder: raw.no_responder,
        key_downs_seen: raw.key_downs_seen,
        recorded_in_pump: raw.recorded_in_pump,
        filtered_keys: raw.filtered_keys,
        pump_update_windows_calls: raw.pump_update_windows_calls,
    }
}

/// Makes the content view (or nothing) the window's first responder.
pub(crate) fn set_first_responder(to_view: bool) {
    // SAFETY: main thread.
    unsafe { ck_win_test_set_first_responder(i32::from(to_view)) };
}

/// `[window performClose:nil]`.
pub(crate) fn perform_close() {
    // SAFETY: main thread.
    unsafe { ck_win_test_perform_close() };
}

/// Posts a key-down event to the application, **marked** as the tests' own (it carries
/// `kCGEventSourceUserData`, which real input cannot). `false` when it could not be made.
#[must_use]
pub(crate) fn post_key(key_code: u16, modifier_flags: u64, repeat: bool, characters: &str) -> bool {
    post_key_with(key_code, modifier_flags, repeat, characters, true)
}

/// Posts a key-down event with no mark, as the owner's typing would arrive: the input filter drops it.
#[must_use]
pub(crate) fn post_unmarked_key(
    key_code: u16,
    modifier_flags: u64,
    repeat: bool,
    characters: &str,
) -> bool {
    post_key_with(key_code, modifier_flags, repeat, characters, false)
}

fn post_key_with(
    key_code: u16,
    modifier_flags: u64,
    repeat: bool,
    characters: &str,
    marked: bool,
) -> bool {
    let Ok(characters) = CString::new(characters) else {
        return false;
    };
    // SAFETY: `characters` is a NUL-terminated string valid for the call.
    unsafe {
        ck_win_test_post_key(
            key_code,
            modifier_flags,
            i32::from(repeat),
            characters.as_ptr().cast::<c_char>(),
            i32::from(marked),
        ) != 0
    }
}

/// The other keyboard events the owner's typing produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyboardEvent {
    /// A key released.
    KeyUp,
    /// A modifier key pressed or released.
    ModifierChange,
}

/// Posts a **marked** key-up (`KeyboardEvent::KeyUp`) or modifier change (`ModifierChange`, whose `modifier_flags` carry
/// the family bits and the device-dependent side bits), as the tests' own.
#[must_use]
pub(crate) fn post_keyboard_event(
    event: KeyboardEvent,
    key_code: u16,
    modifier_flags: u64,
) -> bool {
    let kind = match event {
        KeyboardEvent::KeyUp => 1,
        KeyboardEvent::ModifierChange => 2,
    };
    // SAFETY: main thread; no pointers.
    unsafe { ck_win_test_post_keyboard_event(kind, key_code, modifier_flags, 1) != 0 }
}

/// Posts an unmarked key-up or modifier change, as the owner's typing would arrive.
#[must_use]
pub(crate) fn post_unmarked_keyboard_event(
    event: KeyboardEvent,
    key_code: u16,
    modifier_flags: u64,
) -> bool {
    let kind = match event {
        KeyboardEvent::KeyUp => 1,
        KeyboardEvent::ModifierChange => 2,
    };
    // SAFETY: main thread; no pointers.
    unsafe { ck_win_test_post_keyboard_event(kind, key_code, modifier_flags, 0) != 0 }
}

/// Installs, once for the process, the filter that drops every keyboard event without the tests' mark (a
/// local event monitor, which `AppKit` runs inside `sendEvent:`, before the event reaches a window, its
/// key equivalents or `keyDown:`). Call it before the first window.
pub(crate) fn install_input_filter() {
    // SAFETY: main thread; installs one monitor, once.
    unsafe { ck_win_test_install_input_filter() };
}

/// Starts a thread that types without stopping (unmarked key-downs, Escape among them, repeats, key-ups,
/// modifier changes). Call after the filter is installed.
pub(crate) fn start_typist() {
    // SAFETY: main thread; starts one background thread that only posts events.
    unsafe { ck_win_test_start_typist() };
}

/// Posts an identifiable application-defined test event, at the front of the queue or the back.
pub(crate) fn post_marker(id: i64, at_start: bool) {
    // SAFETY: main thread.
    unsafe { ck_win_test_post_marker(id, i32::from(at_start)) };
}

/// Posts a test event from a main-run-loop timer after `delay`, so a blocked pump always ends.
pub(crate) fn post_marker_after(delay: Duration, id: i64) {
    // SAFETY: main thread.
    unsafe { ck_win_test_post_marker_after(delay.as_secs_f64(), id) };
}

/// Removes the backend's own wake events from the queue; returns how many.
#[must_use]
pub(crate) fn discard_wake_events() -> u64 {
    // SAFETY: main thread.
    unsafe { ck_win_test_discard_wake_events() }
}

/// Takes and dispatches every queued event without handing over or clearing the pending state.
pub(crate) fn drain_events() {
    // SAFETY: main thread.
    unsafe { ck_win_test_drain_events() };
}

/// Uses `scale` instead of the window's backing scale factor (0 restores it).
pub(crate) fn set_scale(scale: f64) {
    // SAFETY: main thread.
    unsafe { ck_win_test_set_scale(scale) };
}

/// Makes the window's delegate see a backing-properties change.
pub(crate) fn backing_changed() {
    // SAFETY: main thread.
    unsafe { ck_win_test_backing_changed() };
}

/// Makes the window's delegate see the window come back from the Dock.
pub(crate) fn deminiaturized() {
    // SAFETY: main thread.
    unsafe { ck_win_test_deminiaturized() };
}

/// Makes the window's delegate see the window enter the Dock.
pub(crate) fn miniaturized() {
    // SAFETY: main thread; the helper calls the live window's delegate.
    unsafe { ck_win_test_miniaturized() };
}

/// Calls the recording callbacks in one go, as a burst inside one dispatch.
pub(crate) fn burst(button_presses: u32, pixel_changes: u32, needs_display: u32) {
    // SAFETY: main thread.
    unsafe { ck_win_test_burst(button_presses, pixel_changes, needs_display) };
}

/// Records `pairs` key-down / key-up pairs of the virtual key code straight into the native state (no event is
/// posted): the fast way to overflow the ring. `false` when the code is not in the key table.
#[must_use]
pub(crate) fn record_keys(pairs: u32, key_code: u16) -> bool {
    // SAFETY: main thread; no pointers.
    unsafe { ck_win_test_record_keys(pairs, key_code) != 0 }
}

/// The window resigns key through the notification `AppKit` sends its delegate.
pub(crate) fn resign_key() {
    // SAFETY: main thread.
    unsafe { ck_win_test_resign_key() };
}

/// The window becomes key through the notification `AppKit` sends its delegate.
pub(crate) fn become_key() {
    // SAFETY: main thread.
    unsafe { ck_win_test_become_key() };
}

/// The native key state: what the responder recorded and has not handed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeKeys {
    /// The exact held state of every key.
    pub(crate) held: [u64; 2],
    /// Presses in a discarded transition.
    pub(crate) lost_pressed: [u64; 2],
    /// Releases in a discarded transition.
    pub(crate) lost_released: [u64; 2],
    /// Transitions waiting in the ring.
    pub(crate) ring_length: u32,
    /// The overflow flag.
    pub(crate) overflow: bool,
    /// A focus loss waits to be delivered.
    pub(crate) focus_lost_pending: bool,
    /// A focus gain waits to be delivered.
    pub(crate) focus_gained_pending: bool,
    /// Transitions discarded into the latches, ever.
    pub(crate) overflow_total: u64,
}

impl NativeKeys {
    fn bit(words: &[u64; 2], key: ck_input::Key) -> bool {
        let index = key as usize;
        words[index / 64] & (1_u64 << (index % 64)) != 0
    }

    /// Whether `key` is held natively.
    #[must_use]
    pub(crate) fn is_held(&self, key: ck_input::Key) -> bool {
        Self::bit(&self.held, key)
    }

    /// Whether any key is held natively.
    #[must_use]
    pub(crate) fn any_held(&self) -> bool {
        self.held != [0, 0]
    }

    /// Whether `key` has a press latched.
    #[must_use]
    pub(crate) fn pressed_latched(&self, key: ck_input::Key) -> bool {
        Self::bit(&self.lost_pressed, key)
    }

    /// Whether `key` has a release latched.
    #[must_use]
    pub(crate) fn released_latched(&self, key: ck_input::Key) -> bool {
        Self::bit(&self.lost_released, key)
    }

    /// Whether no latch is set.
    #[must_use]
    pub(crate) fn latches_clear(&self) -> bool {
        self.lost_pressed == [0, 0] && self.lost_released == [0, 0]
    }
}

/// The native key state now.
#[must_use]
pub(crate) fn native_keys() -> NativeKeys {
    let mut raw = RawNativeKeys::default();
    // SAFETY: main thread; `raw` is valid for the call.
    unsafe { ck_win_test_native_keys(&raw mut raw) };
    NativeKeys {
        held: raw.held,
        lost_pressed: raw.lost_pressed,
        lost_released: raw.lost_released,
        ring_length: raw.ring_length,
        overflow: raw.overflow != 0,
        focus_lost_pending: raw.focus_lost_pending != 0,
        focus_gained_pending: raw.focus_gained_pending != 0,
        overflow_total: raw.overflow_total,
    }
}

/// The native key table: `(virtual key code, key number, W3C name)` for every entry.
#[must_use]
pub(crate) fn key_table() -> Vec<(u16, u8, String)> {
    // SAFETY: main thread; no pointers.
    let count = unsafe { ck_win_test_key_table_count() };
    (0..count)
        .map(|position| {
            let (mut code, mut key) = (0_u16, 0_u8);
            let mut name = [0_u8; 64];
            // SAFETY: the out pointers and the name buffer are valid for the call, and the native side writes
            // at most `name.len()` bytes into the buffer, NUL-terminated.
            let found = unsafe {
                ck_win_test_key_table(
                    position,
                    &raw mut code,
                    &raw mut key,
                    name.as_mut_ptr().cast::<c_char>(),
                    u32::try_from(name.len()).unwrap_or(0),
                )
            };
            assert_ne!(found, 0, "table position {position}");
            let text = CStr::from_bytes_until_nul(&name)
                .expect("a NUL-terminated name")
                .to_string_lossy()
                .into_owned();
            (code, key, text)
        })
        .collect()
}

/// Sets the absorbed-press count, to show it saturates.
pub(crate) fn set_absorbed(count: u32) {
    // SAFETY: main thread.
    unsafe { ck_win_test_set_absorbed(count) };
}

/// Schedules `timers` default-mode timers on the main run loop, all due after `delay`; each calls the
/// close-button recording callback `calls` times.
pub(crate) fn schedule_close_button_records(delay: Duration, timers: u32, calls: u32) {
    // SAFETY: main thread.
    unsafe { ck_win_test_schedule_records(delay.as_secs_f64(), timers, calls, 1) };
}

/// What a recording event records when it is dispatched.
#[derive(Clone, Copy)]
pub(crate) enum Scheduled {
    /// `pairs` down/up pairs of the key with this macOS key code.
    KeyPairs(u32, u16),
    FocusLost,
}

/// Posts an event whose dispatch records `what` natively: the record is made while the pump that dispatches it runs.
pub(crate) fn post_recording(what: Scheduled) {
    let (kind, pairs, code) = match what {
        Scheduled::KeyPairs(pairs, code) => (0, pairs, code),
        Scheduled::FocusLost => (1, 0, 0),
    };
    // SAFETY: main thread.
    unsafe { ck_win_test_post_recording_marker(kind, pairs, code) };
}

/// Calls a main-thread-only `AppKit` method from a thread that is not the main thread: the control that
/// shows the Main Thread Checker reports.
pub(crate) fn violate_main_thread() {
    let thread = std::thread::spawn(|| {
        // SAFETY: deliberately not on the main thread, to provoke the Main Thread Checker; the native
        // call only marks the content view dirty, and the caller joins this thread before going on.
        unsafe { ck_win_test_violate_main_thread() };
    });
    let _ = thread.join();
}

/// What one pump handed over, as the tests compare it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Pumped {
    /// 0 none, 1 the close button: the first close source.
    pub(crate) close_source: i32,
    /// Further close presses, saturating.
    pub(crate) close_absorbed: u32,
    /// The latest pixel size, when it changed.
    pub(crate) pixel_size: Option<(u32, u32)>,
    /// Whether a redraw is needed.
    pub(crate) needs_display: bool,
    /// The events taken off the native queue (typing included).
    pub(crate) native_events: u64,
    /// The normalised key and focus entries, in recorded order.
    pub(crate) input: Vec<NativeEvent>,
    /// The transitions discarded into the loss latches so far (the latest pump's value).
    pub(crate) overflow_total: u64,
}

impl From<PumpReport> for Pumped {
    fn from(report: PumpReport) -> Self {
        Self {
            close_source: match report.close {
                Some(NativeEvent::CloseButton) => 1,
                _ => 0,
            },
            close_absorbed: report.absorbed_close_presses,
            pixel_size: report.pixel_size.map(|size| (size.width, size.height)),
            needs_display: report.needs_display,
            native_events: report.native_events,
            input: report.input,
            overflow_total: report.overflow_total,
        }
    }
}

impl Pumped {
    /// Whether nothing was handed over (the raw event count is ignored: typing is not state).
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.close_source == 0
            && self.pixel_size.is_none()
            && !self.needs_display
            && self.input.is_empty()
    }

    /// Folds a later pump into this one: the first close source stays, presses add up, the latest size wins, the
    /// key and focus entries follow in order.
    pub(crate) fn merge(&mut self, later: Self) {
        if self.close_source == 0 {
            self.close_source = later.close_source;
            self.close_absorbed = self.close_absorbed.saturating_add(later.close_absorbed);
        } else {
            let further = later
                .close_absorbed
                .saturating_add(u32::from(later.close_source != 0));
            self.close_absorbed = self.close_absorbed.saturating_add(further);
        }
        if later.pixel_size.is_some() {
            self.pixel_size = later.pixel_size;
        }
        self.needs_display |= later.needs_display;
        self.native_events += later.native_events;
        self.input.extend(later.input);
        self.overflow_total = later.overflow_total;
    }

    /// The key transitions among the entries, in order, as `(key, down, repeat)`.
    #[must_use]
    pub(crate) fn keys(&self) -> Vec<(ck_input::Key, bool, bool)> {
        self.input
            .iter()
            .filter_map(|event| match event {
                NativeEvent::Key { key, down, repeat } => Some((*key, *down, *repeat)),
                _ => None,
            })
            .collect()
    }
}

/// Key events that reached the window with no responder to take them (the beep path).
#[must_use]
pub(crate) fn no_responder_count() -> u64 {
    counters().no_responder
}

/// Unmarked keyboard events the input filter has dropped (the owner's typing is counted too).
#[must_use]
pub(crate) fn filtered_keys() -> u64 {
    counters().filtered_keys
}

/// Test events `sendEvent:` has dispatched so far.
#[must_use]
pub(crate) fn markers_dispatched() -> u64 {
    counters().markers_dispatched
}

/// Wake events posted to the application so far.
#[must_use]
pub(crate) fn wake_posts() -> u64 {
    counters().wake_posts
}

/// The counters of the application object.
#[must_use]
pub(crate) fn app_counters() -> Counters {
    counters()
}

/// Key-downs that reached the content view so far.
#[must_use]
pub(crate) fn key_downs_seen() -> u64 {
    counters().key_downs_seen
}

/// Timer callbacks that ran inside a pump.
#[must_use]
pub(crate) fn recorded_during_wait() -> u64 {
    counters().recorded_in_pump
}
