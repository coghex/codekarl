//! The native window backend, tested through real `AppKit` dispatch, as a supervisor and a worker from
//! this one executable (`harness = false`, so each case's result appears in plain `cargo test` output).
//!
//! `AppKit` needs the process's main thread, so every case runs in a **worker**: this executable started
//! again with `--worker <case>`, in its own process group, with Apple's Main Thread Checker
//! (`libMainThreadChecker.dylib` from the selected Xcode) inserted from the first instant and
//! `MTC_CRASH_ON_REPORT=1`. The worker first checks that the checker is really loaded, then runs the
//! case on its main thread. The **supervisor** owns each worker from the instant it is spawned, gives it
//! 30 s, and on expiry kills its process group, reaps it and fails the case; every kill, reap and pipe
//! wait is bounded. A worker fails (nothing is skipped) without a desktop session, without the checker
//! library, with any checker report on stderr, and with any failed assertion.
//!
//! Two supervisor fixtures prove the machinery: a hanging worker is killed and reaped at its deadline,
//! and a worker that calls an `AppKit` method off the main thread is reported by the checker (so a
//! silent checker is evidence of nothing being wrong, not of nothing being loaded).
//!
//! The cases open real windows on the desktop. Each worker lives for one case, a window's life is a few
//! hundred milliseconds, and the window is destroyed on every path (a guard does it, also when an
//! assertion fails).

use std::collections::BTreeMap;
use std::env;
use std::ffi::{CStr, OsString, c_char};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ck_input::Key;
use ck_window::{Backend, NativeEvent, NativeWake, Notifier, Wait, WindowDesc};
use ck_window_cocoa::CocoaBackend;

mod support;
use support::Pumped;

#[path = "keys/mod.rs"]
mod keys;

const WORKER_FLAG: &str = "--worker";
/// `1` makes a worker run with the owner typing without stopping (see `typing`).
const TYPING_VARIABLE: &str = "CK_WINDOW_COCOA_TYPING";
/// The cap of a draining pump, and the number of events that means the queue may hold more.
const DRAIN_CAP: usize = 64;
const DRAIN_CAP_EVENTS: u64 = 64;
/// The longest a worker may run.
const DEADLINE: Duration = Duration::from_secs(30);
/// How long the supervisor waits for output pipes, for an emptied group and for each reap attempt.
const PIPE_GRACE: Duration = Duration::from_secs(2);
const REAP_GRACE: Duration = Duration::from_secs(1);
/// What the Main Thread Checker prints for a violation.
const CHECKER_REPORT: &str = "Main Thread Checker";

// ---------------------------------------------------------------------------------------------
// Fixtures for the worker
// ---------------------------------------------------------------------------------------------

unsafe extern "C" {
    fn _dyld_image_count() -> u32;
    fn _dyld_get_image_name(index: u32) -> *const c_char;
}

/// Whether `libMainThreadChecker.dylib` is among the images loaded into this process.
fn checker_loaded() -> bool {
    // SAFETY: both functions are dyld's documented image enumeration; they are called on the main
    // thread before any other thread exists.
    let count = unsafe { _dyld_image_count() };
    (0..count).any(|index| {
        // SAFETY: `index` is below the image count; the returned string is NUL-terminated and valid
        // for the process's life (or null, which is handled).
        let name = unsafe { _dyld_get_image_name(index) };
        // SAFETY: a non-null pointer from dyld is a valid C string.
        !name.is_null()
            && unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .ends_with("libMainThreadChecker.dylib")
    })
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn desc(width: u32, height: u32) -> WindowDesc {
    WindowDesc {
        title: "Codekarl window test".to_owned(),
        width,
        height,
        clear: [0.0, 0.0, 0.0],
    }
}

/// Whether this worker runs with the owner typing without stopping (`CK_WINDOW_COCOA_TYPING=1`).
fn typing() -> bool {
    env::var_os(TYPING_VARIABLE).is_some_and(|value| value == "1")
}

/// Creates a backend the way every case does: the counting application first, then the backend, then the
/// test-only input filter (before the first window, for the worker's whole life) and, in the typing run,
/// the typist.
fn new_backend() -> CocoaBackend {
    support::install_application();
    let backend = CocoaBackend::new().expect("the backend is created on the main thread");
    support::install_input_filter();
    if typing() {
        support::start_typist();
    }
    backend
}

/// A backend with a window, destroyed (and the backend terminated) on every path, also a failed
/// assertion's unwinding.
struct Fixture {
    backend: CocoaBackend,
    pixels: (u32, u32),
}

impl Fixture {
    fn open(width: u32, height: u32) -> Self {
        let mut backend = new_backend();
        let created = backend
            .create_window(&desc(width, height))
            .expect("the window is created (a desktop session is required)");
        assert!(!created.handles.layer().is_null(), "the layer pointer");
        let mut fixture = Self {
            backend,
            pixels: (created.pixel_size.width, created.pixel_size.height),
        };
        fixture.settle();
        if typing() {
            // The owner is typing for real: the filter has been dropping it since before the window.
            assert!(
                support::filtered_keys() > 0,
                "the typist is not typing, so this run proves nothing about typing"
            );
        }
        fixture
    }

    /// Takes the events the window's showing produces (activation, occlusion, backing properties) until
    /// no semantic state has arrived for a few polls, and leaves the pending state empty. Keyboard
    /// events (the owner typing) are not state and do not count.
    fn settle(&mut self) {
        let end = Instant::now() + Duration::from_secs(10);
        let mut quiet = 0;
        while quiet < 4 {
            assert!(Instant::now() < end, "the window never settled");
            if self.drain().is_empty() {
                quiet += 1;
            } else {
                quiet = 0;
            }
            thread::sleep(Duration::from_millis(15));
        }
        let _ = support::discard_wake_events();
    }

    /// One pump, with what it handed over.
    fn pump(&mut self, wait: Wait, cap: usize) -> Pumped {
        self.backend.pump_with_report(wait, cap).into()
    }

    /// Pumps without waiting until the native queue was left empty (a pump that took fewer events than
    /// its cap), folding what the pumps handed over into one result. The owner typing may have queued any
    /// number of keyboard events ahead of a test's own probe; they are taken and dropped on the way.
    fn drain(&mut self) -> Pumped {
        let mut total = Pumped::default();
        for _ in 0..1000 {
            let pumped = self.pump(Wait::Poll, DRAIN_CAP);
            let taken = pumped.native_events;
            total.merge(pumped);
            if taken < DRAIN_CAP_EVENTS {
                // What a pump's own dispatch recorded is handed over by the next pump (the boundary the spec sets):
                // one more pump delivers it.
                total.merge(self.pump(Wait::Poll, DRAIN_CAP));
                break;
            }
        }
        total
    }

    /// The events the `Backend` interface delivers: pumps without waiting until `wanted` have arrived
    /// (the owner typing may have queued keyboard events ahead of them), then one more to show nothing
    /// follows them.
    fn interface_events(&mut self, wanted: usize) -> Vec<NativeEvent> {
        let mut seen = Vec::new();
        let end = Instant::now() + Duration::from_secs(5);
        while seen.len() < wanted {
            assert!(
                Instant::now() < end,
                "only {seen:?} of {wanted} events arrived"
            );
            self.backend
                .pump(&mut |event| seen.push(event), Wait::Poll, DRAIN_CAP);
        }
        for _ in 0..3 {
            self.backend
                .pump(&mut |event| seen.push(event), Wait::Poll, DRAIN_CAP);
        }
        seen
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.backend.destroy_window();
        self.backend.terminate();
    }
}

// ---------------------------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------------------------

/// 1. Create and destroy.
fn create_and_destroy() {
    let before = support::ns_window_count();
    let mut backend = new_backend();
    let info = support::info();
    assert!(!info.has_window, "{info:?}");
    assert_eq!(
        info.menu.key_equivalents, 0,
        "no menu key equivalent exists, so Cmd-Q matches nothing: {info:?}"
    );
    let created = backend.create_window(&desc(320, 200)).expect("the window");
    let info = support::info();
    assert!(info.has_window);
    assert_eq!(info.title, "Codekarl window test");
    assert!(close(info.content_size.0, 320.0) && close(info.content_size.1, 200.0));
    assert!(info.style.titled && info.style.closable && info.style.miniaturizable);
    assert!(!info.behaviour.resizable, "the window is not resizable");
    assert!(
        info.layer.is_metal && info.layer.is_backing_layer,
        "{info:?}"
    );
    assert!(close(info.layer.scale, info.window_scale), "{info:?}");
    assert!(info.window_scale >= 1.0);
    assert!(info.behaviour.has_delegate && info.focus.first_responder_is_view && info.visible);
    assert!(!info.behaviour.released_when_closed);
    assert_eq!(info.menu.key_equivalents, 0, "{info:?}");
    assert_eq!(support::ns_window_count(), before + 1);
    let wide = (320.0 * info.window_scale).round();
    let high = (200.0 * info.window_scale).round();
    assert_eq!(
        (
            f64::from(created.pixel_size.width),
            f64::from(created.pixel_size.height)
        ),
        (wide, high),
        "pixels are points times the scale factor"
    );
    assert!(!created.handles.layer().is_null());
    assert!(
        backend.create_window(&desc(100, 100)).is_err(),
        "one window only"
    );
    assert_eq!(support::ns_window_count(), before + 1);

    backend.destroy_window();
    assert!(!support::info().has_window);
    assert_eq!(support::ns_window_count(), before, "no window is left");
    backend.terminate();
    backend.terminate();
    backend.destroy_window();

    // A window can be created again after destroy, and terminate is safe with no window.
    let mut again = CocoaBackend::new().expect("a second backend");
    assert_eq!(
        support::app_counters().finish_launching_calls,
        1,
        "finishLaunching runs once for the process, however many backends are created"
    );
    again.terminate();
    again.terminate();
    assert!(again.create_window(&desc(10, 10)).is_err(), "terminated");
    assert_eq!(support::ns_window_count(), before);
}

/// 2. Off the main thread.
fn off_the_main_thread() {
    assert!(!support::app_exists(), "no application object yet");
    let refused = thread::spawn(|| CocoaBackend::new().err())
        .join()
        .expect("the thread ended");
    let message = refused.expect("creating the backend off the main thread is an error");
    assert!(message.contains("main thread"), "{message}");
    assert!(
        !support::app_exists(),
        "no AppKit object was touched off the main thread"
    );
    let _backend = CocoaBackend::new().expect("the main thread can");
    assert!(support::app_exists());
}

/// 3. The close button.
fn close_button() {
    let mut fixture = Fixture::open(300, 200);
    support::perform_close();
    let info = support::info();
    assert!(
        info.has_window && info.visible,
        "the window never closes by itself"
    );
    let pumped = fixture.drain();
    assert_eq!(pumped.close_source, 1, "one CloseButton: {pumped:?}");
    assert_eq!(pumped.close_absorbed, 0);
    assert_eq!(fixture.drain().close_source, 0, "taken once");
    // Through the `Backend` interface the same press is one `CloseButton` event.
    support::perform_close();
    let seen = fixture.interface_events(1);
    assert_eq!(seen, [NativeEvent::CloseButton]);
    assert!(support::info().visible);
}

const ESCAPE: u16 = 53;
const COMMAND: u64 = 1 << 20;

/// 4. Keys, through posted events and `sendEvent:`.
///
/// Escape is a key like any other for this backend (it is the `close` binding of the input mapper, not a close source
/// here); repeats keep their flag; keys the table knows are normalised and every key is consumed (no beep).
fn escape_through_dispatch() {
    let mut fixture = Fixture::open(300, 200);
    let seen0 = support::key_downs_seen();
    assert!(support::post_key(ESCAPE, 0, false, "\u{1b}"));
    let pumped = fixture.drain();
    assert_eq!(
        pumped.close_source, 0,
        "the Cocoa backend no longer reports Escape as a close: {pumped:?}"
    );
    assert_eq!(pumped.keys(), [(Key::Escape, true, false)], "{pumped:?}");
    assert_eq!(
        support::key_downs_seen(),
        seen0 + 1,
        "the event reached the first responder through sendEvent:"
    );

    // A press and its repeat in one pass: two events, the second flagged as the repeat.
    assert!(support::post_key(ESCAPE, 0, false, "\u{1b}"));
    assert!(support::post_key(ESCAPE, 0, true, "\u{1b}"));
    let pumped = fixture.drain();
    assert_eq!(
        pumped.keys(),
        [(Key::Escape, true, false), (Key::Escape, true, true)],
        "{pumped:?}"
    );
    // The key goes up (a marked key-up).
    assert!(support::post_keyboard_event(
        support::KeyboardEvent::KeyUp,
        ESCAPE,
        0
    ));
    assert_eq!(fixture.drain().keys(), [(Key::Escape, false, false)]);

    // Other keys are recorded as themselves, and none reaches the beep path.
    let beeps0 = support::no_responder_count();
    let seen = support::key_downs_seen();
    assert!(support::post_key(0, 0, false, "a"));
    assert!(support::post_key(36, 0, false, "\r"));
    let pumped = fixture.drain();
    assert_eq!(
        pumped.keys(),
        [(Key::KeyA, true, false), (Key::Enter, true, false)],
        "{pumped:?}"
    );
    assert_eq!(
        (pumped.close_source, pumped.pixel_size, pumped.needs_display),
        (0, None, false),
        "no other state is recorded: {pumped:?}"
    );
    assert_eq!(support::key_downs_seen(), seen + 2);
    assert_eq!(support::no_responder_count(), beeps0, "no beep");

    // The marker works: with no first responder the same key reaches the beep path.
    support::set_first_responder(false);
    assert!(support::post_key(0, 0, false, "a"));
    let _ = fixture.drain();
    assert_eq!(
        support::no_responder_count(),
        beeps0 + 1,
        "the responder-chain marker sees an unhandled key"
    );
    support::set_first_responder(true);
}

/// Real typing (unmarked key events, Escape among them) is dropped before window and responder dispatch.
fn real_input_is_dropped() {
    let mut fixture = Fixture::open(300, 200);
    let dropped0 = support::filtered_keys();
    let seen0 = support::key_downs_seen();
    let beeps0 = support::no_responder_count();
    let sent = support::markers_dispatched();
    assert!(support::post_unmarked_key(ESCAPE, 0, false, "\u{1b}"));
    assert!(support::post_unmarked_key(ESCAPE, 0, true, "\u{1b}"));
    assert!(support::post_unmarked_key(0, 0, false, "a"));
    assert!(support::post_unmarked_key(12, COMMAND, false, "q"));
    // Key-ups and modifier changes are keyboard events too.
    assert!(support::post_unmarked_keyboard_event(
        support::KeyboardEvent::KeyUp,
        ESCAPE,
        0
    ));
    assert!(support::post_unmarked_keyboard_event(
        support::KeyboardEvent::ModifierChange,
        55,
        COMMAND
    ));
    // And the owner types one more key that the test does not count (nor may its result depend on it).
    assert!(support::post_unmarked_key(11, 0, false, "b"));
    // A test event behind them shows the pump dispatched past the keys.
    support::post_marker(21, false);
    let pumped = fixture.drain();
    assert_eq!(
        support::markers_dispatched(),
        sent + 1,
        "the pump dispatched everything queued"
    );
    assert_eq!(
        (pumped.close_source, pumped.close_absorbed),
        (0, 0),
        "typing, Escape included, records nothing: {pumped:?}"
    );
    assert!(
        pumped.keys().is_empty(),
        "no Key event reaches the loop from the owner's typing: {pumped:?}"
    );
    assert!(!support::native_keys().any_held(), "and nothing is held");
    // At least these six: any further typing by the owner is dropped as well and is not counted on.
    assert!(
        support::filtered_keys() - dropped0 >= 6,
        "the filter dropped the six (key-downs, a key-up, a modifier change), inside the pump's dispatch"
    );
    assert_eq!(
        support::key_downs_seen(),
        seen0,
        "none reached the content view"
    );
    assert_eq!(support::no_responder_count(), beeps0, "no beep");
    assert!(support::info().visible);

    // The test's own, marked Escape is not filtered and still records, as a key.
    assert!(support::post_key(ESCAPE, 0, false, "\u{1b}"));
    let pumped = fixture.drain();
    assert_eq!(pumped.keys(), [(Key::Escape, true, false)], "{pumped:?}");
    assert_eq!(
        support::key_downs_seen(),
        seen0 + 1,
        "the marked Escape passed the filter and reached the content view"
    );
}

/// 5. Cmd-Q, through the same dispatch.
fn cmd_q_through_dispatch() {
    let mut fixture = Fixture::open(300, 200);
    let menu = support::info().menu;
    assert_eq!(
        (menu.items, menu.key_equivalents),
        (menu.items, 0),
        "AppKit's own placeholder menu has no key equivalent: {menu:?}"
    );
    assert!(support::post_key(12, COMMAND, false, "q"));
    // A marker behind it shows the pump dispatched past the key.
    let sent = support::markers_dispatched();
    support::post_marker(1, false);
    let pumped = fixture.drain();
    assert_eq!(
        support::markers_dispatched(),
        sent + 1,
        "the key and the marker behind it were dispatched"
    );
    assert_eq!(
        (pumped.close_source, pumped.pixel_size, pumped.needs_display),
        (0, None, false),
        "Cmd-Q requests no close and changes nothing: {pumped:?}"
    );
    assert_eq!(
        pumped.keys(),
        [(Key::KeyQ, true, false)],
        "it is a key like any other: the window has no menu, so no key equivalent takes it: {pumped:?}"
    );
    let info = support::info();
    assert!(info.has_window && info.visible, "the window stays open");
    // The process is still running: more events are served.
    assert!(support::post_key(ESCAPE, 0, false, "\u{1b}"));
    assert_eq!(fixture.drain().keys(), [(Key::Escape, true, false)]);
}

/// The marker id whose dispatch calls `updateWindows` (`CK_TEST_UPDATE_MARKER` in the test support).
const UPDATE_MARKER: i64 = 0x5550_4454;

/// 6. The cap counts raw native events.
fn cap() {
    let mut fixture = Fixture::open(300, 200);
    let start = support::markers_dispatched();
    for id in 0..8 {
        support::post_marker(id, true);
    }
    for expected in [3, 3, 2] {
        let before = support::markers_dispatched();
        let _ = fixture.pump(Wait::Poll, 3);
        assert_eq!(
            support::markers_dispatched() - before,
            expected,
            "one pump dispatches exactly `cap` of the identifiable events"
        );
    }
    assert_eq!(support::markers_dispatched() - start, 8);

    // The backend's own wake events are consumed, never dispatched, and count toward the cap.
    let wake = fixture.backend.wake_target();
    for id in 0..3 {
        support::post_marker(100 + id, true);
    }
    wake.wake();
    let before = support::markers_dispatched();
    let _ = fixture.pump(Wait::Poll, 2);
    assert_eq!(support::markers_dispatched() - before, 2);
    if !typing() {
        // With nothing else queued the wake is the event behind the markers: the second pump's two
        // events are the last marker and the wake. (While the owner types, keyboard events can sit
        // ahead of it, so this ordering is only claimed for the quiet run.)
        let _ = fixture.pump(Wait::Poll, 2);
        assert_eq!(
            support::markers_dispatched() - before,
            3,
            "the last marker and the wake used this pump's two events"
        );
    }
    let _ = fixture.drain();
    assert_eq!(support::markers_dispatched() - before, 3);
    assert_eq!(
        support::discard_wake_events(),
        0,
        "the wake event was consumed by the pumps"
    );
    assert_eq!(
        support::app_counters().wake_events_dispatched,
        0,
        "the pump's own wake events are consumed, never dispatched"
    );
    // updateWindows runs once per pump, after its dispatch. Only the pump's own calls are counted:
    // AppKit also calls it while dispatching events the owner's focus changes cause.
    let updates = support::app_counters().pump_update_windows_calls;
    let _ = fixture.pump(Wait::Poll, 1);
    let _ = fixture.pump(Wait::Poll, 5);
    assert_eq!(
        support::app_counters().pump_update_windows_calls,
        updates + 2
    );
    // A call made while the pump dispatches (as AppKit's own are) is not the pump's either: the
    // marker below calls updateWindows from its dispatch, with the pump further up the stack.
    support::post_marker(UPDATE_MARKER, false);
    let before = support::app_counters();
    let _ = fixture.pump(Wait::Poll, 1);
    let after = support::app_counters();
    assert_eq!(after.markers_dispatched, before.markers_dispatched + 1);
    assert!(after.update_windows_calls >= before.update_windows_calls + 2);
    assert_eq!(
        after.pump_update_windows_calls,
        before.pump_update_windows_calls + 1,
        "only the pump's own call after its dispatch is the pump's"
    );
    // A call from elsewhere counts in the total but is not attributed to the pump.
    let before = support::app_counters();
    support::drain_events();
    let after = support::app_counters();
    assert!(after.update_windows_calls > before.update_windows_calls);
    assert_eq!(
        after.pump_update_windows_calls, before.pump_update_windows_calls,
        "an updateWindows call outside the pump is not the pump's"
    );
}

/// 7a. A wake from another thread ends a pump that is already waiting.
fn wake_ends_a_waiting_pump() {
    let mut fixture = Fixture::open(300, 200);
    let wake = fixture.backend.wake_target();
    let woke_at = Arc::new(Mutex::new(None::<Instant>));
    let thread_woke = Arc::clone(&woke_at);
    let producer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        *thread_woke.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
        wake.wake();
    });
    // A late test event ends the wait if the wake is ever lost, so the failure is an assertion.
    support::post_marker_after(Duration::from_secs(6), 7);
    let marker0 = support::markers_dispatched();
    let posts0 = support::wake_posts();
    let end = Instant::now() + Duration::from_secs(5);
    // `wake_posts` rises before the event is posted, so a pump that returns for the wake sees it; a
    // pump that returns for an incidental event first does not, and the loop waits again.
    let returned = loop {
        let _ = fixture.pump(Wait::UntilEvent, 64);
        if support::wake_posts() > posts0 {
            break Instant::now();
        }
        assert!(
            Instant::now() < end,
            "the waiting pump never returned for the wake"
        );
    };
    producer.join().expect("the producer ended");
    let woke_time = woke_at
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .expect("the producer woke");
    let latency = returned.saturating_duration_since(woke_time);
    assert!(latency < Duration::from_secs(1), "woke after {latency:?}");
    assert_eq!(
        support::markers_dispatched(),
        marker0,
        "the pump returned for the wake, not for the late marker"
    );
}

/// 7b. A wake made before the pump starts is sticky.
fn wake_before_the_pump_is_sticky() {
    let mut fixture = Fixture::open(300, 200);
    let wake = fixture.backend.wake_target();
    let posts = support::wake_posts();
    let sender = thread::spawn(move || {
        // Many wakes while one event is queued: one event, however many producers.
        for _ in 0..50 {
            wake.wake();
        }
    });
    sender.join().expect("the producer ended");
    assert_eq!(
        support::wake_posts(),
        posts + 1,
        "wakes coalesce while one wake event is queued"
    );
    support::post_marker_after(Duration::from_secs(6), 8);
    let marker0 = support::markers_dispatched();
    let started = Instant::now();
    let _ = fixture.pump(Wait::UntilEvent, 64);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the queued wake ended the wait at once ({:?})",
        started.elapsed()
    );
    assert_eq!(support::markers_dispatched(), marker0);
    let _ = fixture.drain();
    assert_eq!(
        support::discard_wake_events(),
        0,
        "the pumps consumed the wake event"
    );
    assert_eq!(support::app_counters().wake_events_dispatched, 0);
}

/// 7d. A coalescing flag left set by a consumer that is not the pump does not swallow later wakes.
fn a_stale_flag_does_not_swallow_later_wakes() {
    let mut fixture = Fixture::open(300, 200);
    let wake = fixture.backend.wake_target();
    wake.wake();
    // Another consumer takes the event (a nested loop would), leaving the flag set.
    support::drain_events();
    let _ = fixture.drain();
    let posts = support::wake_posts();
    wake.wake();
    assert_eq!(
        support::wake_posts(),
        posts + 1,
        "a wake after a pump posts a new event"
    );
    support::post_marker_after(Duration::from_secs(4), 11);
    let marker0 = support::markers_dispatched();
    let started = Instant::now();
    let _ = fixture.pump(Wait::UntilEvent, 64);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the wake ended the wait ({:?})",
        started.elapsed()
    );
    assert_eq!(support::markers_dispatched(), marker0);
}

/// Forwards to the real target and counts the calls that reach it.
struct Counting {
    inner: Arc<dyn NativeWake>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl NativeWake for Counting {
    fn wake(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.wake();
    }
}

/// 7c. After the notifier is disarmed, wakes reach no native target.
fn disarmed_notifier_reaches_nothing() {
    let mut fixture = Fixture::open(300, 200);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let target: Arc<dyn NativeWake> = Arc::new(Counting {
        inner: fixture.backend.wake_target(),
        calls: Arc::clone(&calls),
    });
    let notifier = Notifier::new();
    notifier.wake();
    assert_eq!(calls.load(Ordering::SeqCst), 0, "inactive: no native call");
    notifier.arm(target).expect("arms");
    let posts = support::wake_posts();
    notifier.wake();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(support::wake_posts(), posts + 1);
    notifier.disarm(Duration::from_secs(1)).expect("disarms");
    for _ in 0..5 {
        notifier.wake();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1, "disarmed: no native call");
    assert_eq!(support::wake_posts(), posts + 1);
    assert!(notifier.take_pending(), "a wake is still never lost");

    // A target held past the backend's teardown is released without touching a freed object.
    let held = fixture.backend.wake_target();
    fixture.backend.destroy_window();
    fixture.backend.terminate();
    drop(held);
}

/// 8. Constant pending state, at most three events per pump.
fn bounded_pending_state() {
    let mut fixture = Fixture::open(320, 200);
    let initial = fixture.pixels;
    support::burst(3000, 1000, 1500);
    let pumped = fixture.drain();
    assert_eq!(
        pumped.close_source, 1,
        "the first source in arrival order: {pumped:?}"
    );
    assert_eq!(pumped.close_absorbed, 2999, "{pumped:?}");
    // The 1000th change used the scale 1 + (999 % 7 + 1) / 4 = 2.5.
    assert_eq!(pumped.pixel_size, Some((800, 500)), "the latest size");
    assert!(pumped.needs_display);
    assert!(fixture.drain().is_empty(), "cleared");

    // Through the interface: at most one of each, in this order.
    support::burst(5000, 5, 5000);
    let seen = fixture.interface_events(3);
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert_eq!(seen[0], NativeEvent::CloseButton);
    assert!(
        matches!(seen[1], NativeEvent::PixelSizeChanged { .. }),
        "{seen:?}"
    );
    assert_eq!(seen[2], NativeEvent::NeedsDisplay);

    // The count of absorbed presses saturates instead of overflowing.
    support::burst(1, 0, 0);
    support::set_absorbed(u32::MAX - 1);
    support::burst(3, 0, 0);
    let pumped = fixture.drain();
    assert_eq!((pumped.close_source, pumped.close_absorbed), (1, u32::MAX));

    support::set_scale(0.0);
    support::backing_changed();
    let pumped = fixture.drain();
    assert_eq!(
        pumped.pixel_size,
        Some(initial),
        "back to the window's own scale"
    );
}

/// 9a. With pending state recorded and no native event queued, a wait returns at once.
fn pending_state_does_not_sleep() {
    let mut fixture = Fixture::open(300, 200);
    // Pending state through the real recording path; its wake event and every incidental event are
    // taken off the queue, so only the pending-state branch can end the wait.
    support::burst(1, 0, 0);
    assert!(
        support::discard_wake_events() >= 1,
        "the callback posted a wake, which the test removes to test the pending-state branch"
    );
    for _ in 0..3 {
        support::drain_events();
        thread::sleep(Duration::from_millis(30));
    }
    let _ = support::discard_wake_events();
    support::post_marker_after(Duration::from_secs(4), 9);
    let marker0 = support::markers_dispatched();
    let started = Instant::now();
    let pumped = fixture.pump(Wait::UntilEvent, 64);
    assert_eq!(pumped.close_source, 1, "{pumped:?}");
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "a pump with pending state does not sleep ({:?})",
        started.elapsed()
    );
    assert_eq!(support::markers_dispatched(), marker0);
}

/// 9b. Work recorded while the pump waits ends the wait through one coalesced wake.
fn work_recorded_during_the_wait_wakes_once() {
    let mut fixture = Fixture::open(300, 200);
    let posts = support::wake_posts();
    support::schedule_close_button_records(Duration::from_millis(400), 20, 50);
    support::post_marker_after(Duration::from_secs(6), 10);
    let marker0 = support::markers_dispatched();
    let started = Instant::now();
    let end = started + Duration::from_secs(5);
    let pumped = loop {
        let pumped = fixture.pump(Wait::UntilEvent, 64);
        if pumped.close_source != 0 {
            break pumped;
        }
        assert!(
            Instant::now() < end,
            "the recorded work never ended the wait"
        );
    };
    assert_eq!(pumped.close_source, 1);
    assert_eq!(
        pumped.close_absorbed, 999,
        "every callback recorded: {pumped:?}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "the wait ended with the work ({:?})",
        started.elapsed()
    );
    assert!(
        support::recorded_during_wait() > 0,
        "the callbacks ran while the pump was waiting"
    );
    assert_eq!(
        support::wake_posts() - posts,
        1,
        "one wake event, however many callbacks"
    );
    assert_eq!(support::markers_dispatched(), marker0);
    // The wake the callbacks posted was consumed in that same pump, so it no longer coalesces later
    // wakes: a producer's wake right after queues a new event and ends the next wait.
    let posts = support::wake_posts();
    fixture.backend.wake_target().wake();
    assert_eq!(support::wake_posts(), posts + 1);
    support::post_marker_after(Duration::from_secs(4), 12);
    let started = Instant::now();
    let _ = fixture.pump(Wait::UntilEvent, 64);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the later wake ended the wait ({:?})",
        started.elapsed()
    );
    assert_eq!(support::markers_dispatched(), marker0);
    assert_eq!(
        support::discard_wake_events(),
        0,
        "the pumps consumed every wake event"
    );
}

/// 10. A backing-properties change records the newly computed pixel size.
fn pixel_size() {
    let mut fixture = Fixture::open(320, 200);
    let initial = fixture.pixels;
    let scale = support::info().window_scale;
    // A whole-number factor that differs from the window's own, so the pixel size is exact.
    let factor: u32 = if scale >= 2.0 { 1 } else { 2 };
    let other = f64::from(factor);
    support::set_scale(other);
    support::backing_changed();
    let pumped = fixture.drain();
    let wanted = (320 * factor, 200 * factor);
    assert_eq!(pumped.pixel_size, Some(wanted), "{pumped:?}");
    assert_ne!(wanted, initial, "the new size differs from the old one");
    assert!(close(support::info().layer.scale, other));

    support::backing_changed();
    assert_eq!(fixture.drain().pixel_size, None, "no change, no event");

    support::set_scale(0.0);
    support::backing_changed();
    assert_eq!(fixture.drain().pixel_size, Some(initial));
    assert!(close(support::info().layer.scale, scale));
}

/// 11. A window that needs drawing again records needs-display.
fn needs_display() {
    let mut fixture = Fixture::open(300, 200);
    assert!(!fixture.drain().needs_display, "settled");
    support::deminiaturized();
    let seen = fixture.interface_events(1);
    assert_eq!(seen, [NativeEvent::NeedsDisplay, NativeEvent::Restored]);
    assert!(!fixture.drain().needs_display, "handed over once");
}

/// Deadline waits return at the requested time, wake early, and hand over the same native state as `pump`.
fn deadline_wait_and_minimize_restore() {
    let mut fixture = Fixture::open(300, 200);
    fixture.settle();

    let start = Instant::now();
    let deadline = start + Duration::from_millis(80);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        fixture
            .backend
            .pump_until(&mut |event| seen.push(event), deadline, DRAIN_CAP);
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(45),
        "wait returned too early: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(800),
        "wait exceeded its bound: {elapsed:?}"
    );
    assert!(
        seen.is_empty(),
        "settled deadline wait recorded no work: {seen:?}"
    );

    support::miniaturized();
    seen.clear();
    fixture.backend.pump_until(
        &mut |event| seen.push(event),
        Instant::now() + Duration::from_secs(2),
        DRAIN_CAP,
    );
    assert_eq!(seen, [NativeEvent::Minimized]);

    support::deminiaturized();
    seen.clear();
    fixture.backend.pump_until(
        &mut |event| seen.push(event),
        Instant::now() + Duration::from_secs(2),
        DRAIN_CAP,
    );
    assert!(
        seen.contains(&NativeEvent::Restored),
        "restore is reported: {seen:?}"
    );
    assert!(
        seen.contains(&NativeEvent::NeedsDisplay),
        "restore redraw is retained: {seen:?}"
    );

    // A wake interrupts a long deadline without a polling loop.
    fixture.backend.wake_target().wake();
    let start = Instant::now();
    seen.clear();
    fixture.backend.pump_until(
        &mut |event| seen.push(event),
        start + Duration::from_secs(5),
        DRAIN_CAP,
    );
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "wake did not interrupt the deadline wait"
    );
}

/// The fixtures' workers.
fn checker_control() {
    let _fixture = Fixture::open(200, 120);
    support::violate_main_thread();
}

fn hang() {
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

type Case = fn();

const CASES: [(&str, Case); 38] = [
    ("create_and_destroy", create_and_destroy),
    ("off_the_main_thread", off_the_main_thread),
    ("close_button", close_button),
    ("escape_through_dispatch", escape_through_dispatch),
    ("real_input_is_dropped", real_input_is_dropped),
    ("cmd_q_through_dispatch", cmd_q_through_dispatch),
    ("cap", cap),
    ("wake_ends_a_waiting_pump", wake_ends_a_waiting_pump),
    (
        "wake_before_the_pump_is_sticky",
        wake_before_the_pump_is_sticky,
    ),
    (
        "a_stale_flag_does_not_swallow_later_wakes",
        a_stale_flag_does_not_swallow_later_wakes,
    ),
    (
        "disarmed_notifier_reaches_nothing",
        disarmed_notifier_reaches_nothing,
    ),
    ("bounded_pending_state", bounded_pending_state),
    ("pending_state_does_not_sleep", pending_state_does_not_sleep),
    (
        "work_recorded_during_the_wait_wakes_once",
        work_recorded_during_the_wait_wakes_once,
    ),
    ("pixel_size", pixel_size),
    ("needs_display", needs_display),
    (
        "deadline_wait_and_minimize_restore",
        deadline_wait_and_minimize_restore,
    ),
    (
        "key_table_matches_ck_input",
        keys::key_table_matches_ck_input,
    ),
    (
        "every_table_key_produces_its_key",
        keys::every_table_key_produces_its_key,
    ),
    (
        "unmapped_codes_produce_nothing_and_no_beep",
        keys::unmapped_codes_produce_nothing_and_no_beep,
    ),
    ("modifiers_are_side_aware", keys::modifiers_are_side_aware),
    (
        "a_mixed_side_chord_stays_held",
        keys::a_mixed_side_chord_stays_held,
    ),
    (
        "modifiers_without_side_bits_toggle_and_release_with_the_family",
        keys::modifiers_without_side_bits_toggle_and_release_with_the_family,
    ),
    (
        "focus_events_and_the_reset",
        keys::focus_events_and_the_reset,
    ),
    (
        "bounded_recording_resyncs_to_the_native_state",
        keys::bounded_recording_resyncs_to_the_native_state,
    ),
    (
        "a_tap_in_the_discarded_span_is_latched_and_escape_still_closes",
        keys::a_tap_in_the_discarded_span_is_latched_and_escape_still_closes,
    ),
    (
        "schedule_a_a_stale_modifier_is_not_replayed",
        keys::schedule_a_a_stale_modifier_is_not_replayed,
    ),
    (
        "schedule_b_an_unknown_order_invents_no_chord",
        keys::schedule_b_an_unknown_order_invents_no_chord,
    ),
    (
        "a_prefixed_close_binding_closes_only_through_a_chord_held_now",
        keys::a_prefixed_close_binding_closes_only_through_a_chord_held_now,
    ),
    (
        "every_transition_is_delivered_or_latched_exactly_once",
        keys::every_transition_is_delivered_or_latched_exactly_once,
    ),
    (
        "focus_loss_with_a_pending_overflow_delivers_no_resync",
        keys::focus_loss_with_a_pending_overflow_delivers_no_resync,
    ),
    (
        "recovery_over_capped_pumps",
        keys::recovery_over_capped_pumps,
    ),
    ("typing_reaches_no_window", keys::typing_reaches_no_window),
    (
        "recorded_keys_and_focus_do_not_let_a_pump_sleep",
        keys::recorded_keys_and_focus_do_not_let_a_pump_sleep,
    ),
    (
        "discarded_repeats_are_no_presses",
        keys::discarded_repeats_are_no_presses,
    ),
    (
        "input_recorded_while_a_pump_dispatches_waits_for_the_next_pump",
        keys::input_recorded_while_a_pump_dispatches_waits_for_the_next_pump,
    ),
    (
        "a_focus_gain_keeps_its_place_among_the_keys_recorded_around_it",
        keys::a_focus_gain_keeps_its_place_among_the_keys_recorded_around_it,
    ),
    (
        "a_focus_gain_and_a_resync_keep_the_order_of_their_first_records",
        keys::a_focus_gain_and_a_resync_keep_the_order_of_their_first_records,
    ),
];

/// Cases only the supervisor's fixtures start.
const FIXTURE_CASES: [(&str, Case); 2] = [("checker_control", checker_control), ("hang", hang)];

fn worker(name: &str) -> ExitCode {
    if !checker_loaded() {
        eprintln!("worker: libMainThreadChecker.dylib is not loaded; the checker must be inserted");
        return ExitCode::from(2);
    }
    let Some((_, case)) = CASES
        .iter()
        .chain(FIXTURE_CASES.iter())
        .find(|(case, _)| *case == name)
    else {
        eprintln!("worker: no case called {name}");
        return ExitCode::from(1);
    };
    match catch_unwind(AssertUnwindSafe(case)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(payload) => {
            let text = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_owned())
                })
                .unwrap_or_else(|| "a panic with a non-string payload".to_owned());
            eprintln!("worker: {name} failed: {text}");
            ExitCode::from(1)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The supervisor
// ---------------------------------------------------------------------------------------------

/// How long a helper program of the supervisor (`kill`, `ps`, `xcode-select`, `nm`) may run before it is
/// killed. After the kill it is reaped within `REAP_GRACE`, and its output within `PIPE_GRACE`.
const HELPER_BUDGET: Duration = Duration::from_secs(2);

/// The helper programs the supervisor uses. The fixtures replace them with stalled or failing ones.
#[derive(Debug, Clone)]
struct Tools {
    kill: PathBuf,
    shell: PathBuf,
    ps: PathBuf,
    xcode_select: PathBuf,
}

impl Tools {
    fn system() -> Self {
        Self {
            kill: PathBuf::from("/bin/kill"),
            shell: PathBuf::from("/bin/sh"),
            ps: PathBuf::from("/bin/ps"),
            xcode_select: PathBuf::from("/usr/bin/xcode-select"),
        }
    }
}

/// What reading a stream to its end came to: all of its text, or the error that ended the read. Bytes read
/// before an error are not kept: a prefix that looks clean is no evidence of what the stream held.
type Drained = Result<String, String>;

/// Reads `stream` to its end. An error is a failure that says how much had been read before it.
fn read_all<R: Read>(stream: Option<R>) -> Drained {
    let mut bytes = Vec::new();
    if let Some(mut stream) = stream
        && let Err(error) = stream.read_to_end(&mut bytes)
    {
        return Err(format!("{error} (after {} bytes)", bytes.len()));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Reads `stream` to its end on a thread of its own, and sends the result: a read error is sent as such.
fn drain<R: Read + Send + 'static>(stream: Option<R>) -> std::io::Result<mpsc::Receiver<Drained>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new().spawn(move || {
        let _ = sender.send(read_all(stream));
    })?;
    Ok(receiver)
}

/// Waits at most `grace` for the child to be reaped; `Ok(false)` if it is still running.
fn reaped_within(child: &mut Child, grace: Duration) -> std::io::Result<bool> {
    let expiry = Instant::now() + grace;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        if Instant::now() >= expiry {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Why a helper gave no answer. The kinds matter to callers: a helper that ran and exited unsuccessfully
/// (`pgrep` found no match, `kill -0` found no process) is an answer for some of them; one whose output
/// could not be read, or that did not finish, is never an answer.
#[derive(Debug, PartialEq, Eq)]
enum HelperError {
    /// It ran to its end and exited unsuccessfully (`code` is `None` when a signal ended it).
    Exit {
        name: String,
        status: String,
        code: Option<i32>,
    },
    /// Its output could not be read to the end (whatever its status said).
    Read { name: String, error: String },
    /// It could not be run, waited for, ended or drained in time.
    Other(String),
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exit { name, status, .. } => write!(formatter, "{name} exited with {status}"),
            Self::Read { name, error } => {
                write!(formatter, "{name}: its output could not be read: {error}")
            }
            Self::Other(text) => formatter.write_str(text),
        }
    }
}

impl From<HelperError> for String {
    fn from(error: HelperError) -> Self {
        error.to_string()
    }
}

/// What a finished helper came to, from its status and what reading its output gave. A failed read comes
/// first: with the output unread, neither a zero status nor an exit status of 1 means what it usually does.
fn helper_result(name: &str, status: ExitStatus, output: Drained) -> Result<String, HelperError> {
    let text = output.map_err(|error| HelperError::Read {
        name: name.to_owned(),
        error,
    })?;
    if status.success() {
        Ok(text)
    } else {
        Err(HelperError::Exit {
            name: name.to_owned(),
            status: status.to_string(),
            code: status.code(),
        })
    }
}

/// A helper process while the supervisor owns it, from the instant it is spawned: if it is not settled
/// when this drops (an error, a panic), the helper is killed and reaped, waiting at most `REAP_GRACE`.
struct Helper {
    child: Child,
    settled: bool,
}

impl Drop for Helper {
    fn drop(&mut self) {
        if !self.settled {
            let _ = self.child.kill();
            let _ = reaped_within(&mut self.child, REAP_GRACE);
        }
    }
}

/// Runs `program` and returns what it wrote to stdout. The helper is owned from the instant it is spawned,
/// and nothing about it can take longer than `budget` plus `REAP_GRACE` plus `PIPE_GRACE`: at the deadline
/// it is killed and reaped (or the failure to reap is the error), and its output is waited for only so
/// long. A helper that fails, times out or cannot be reaped is an error, never an empty answer.
fn run_helper(program: &Path, args: &[&str], budget: Duration) -> Result<String, HelperError> {
    let name = program.display().to_string();
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let child = command
        .spawn()
        .map_err(|error| HelperError::Other(format!("{name} could not run: {error}")))?;
    let mut helper = Helper {
        child,
        settled: false,
    };
    let output = drain(helper.child.stdout.take()).map_err(|error| {
        HelperError::Other(format!("{name}: output could not be read: {error}"))
    })?;
    let expiry = Instant::now() + budget;
    let status = loop {
        match helper.child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < expiry => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = helper.child.kill();
                return match reaped_within(&mut helper.child, REAP_GRACE) {
                    Ok(true) => {
                        helper.settled = true;
                        Err(HelperError::Other(format!(
                            "{name} did not finish within {budget:?}; it was killed"
                        )))
                    }
                    _ => Err(HelperError::Other(format!(
                        "{name} did not finish within {budget:?} and could not be reaped"
                    ))),
                };
            }
            Err(error) => {
                return Err(HelperError::Other(format!(
                    "{name} could not be waited for: {error}"
                )));
            }
        }
    };
    helper.settled = true;
    // A timeout here is a descendant holding the pipe open; a read error arrives as the answer.
    let output = output
        .recv_timeout(PIPE_GRACE)
        .map_err(|_| HelperError::Other(format!("{name}: its output pipe stayed open")))?;
    helper_result(&name, status, output)
}

/// Kills every process in the group (the worker is its leader): `kill`, then the shell's own. Both are
/// bounded helpers.
fn kill_group(tools: &Tools, group: u32) -> Result<(), String> {
    let target = format!("-{group}");
    let first = run_helper(&tools.kill, &["-KILL", "--", &target], HELPER_BUDGET);
    let Err(first) = first else {
        return Ok(());
    };
    let group_text = group.to_string();
    run_helper(
        &tools.shell,
        &["-c", "kill -KILL -- \"-$0\"", &group_text],
        HELPER_BUDGET,
    )
    .map(drop)
    .map_err(|second| format!("{first}; then {second}"))
}

/// The ids of the processes in the group. Failing to list them is an error: not being able to look is
/// not evidence that the group is empty.
fn group_members(tools: &Tools, group: u32) -> Result<Vec<u32>, String> {
    members_of(
        run_helper(&tools.ps, &["-axo", "pid=,pgid="], HELPER_BUDGET),
        group,
    )
}

/// [`group_members`] from what the listing helper gave: a listing that was not read to its end, whatever
/// its prefix shows, is an error and never an empty or a complete group.
fn members_of(listing: Result<String, HelperError>, group: u32) -> Result<Vec<u32>, String> {
    let listing = listing?;
    Ok(listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace().map(str::parse::<u32>);
            match (fields.next(), fields.next()) {
                (Some(Ok(member)), Some(Ok(leader))) if leader == group => Some(member),
                _ => None,
            }
        })
        .collect())
}

/// The worker while the supervisor owns it, from the instant it is spawned. If the supervisor returns
/// for any reason with the worker not reaped (an error, a panic), dropping this kills the group and the
/// worker and reaps it, waiting at most `REAP_GRACE` per attempt.
struct Owned {
    child: Child,
    group: u32,
    tools: Tools,
    settled: bool,
    /// What the most recent group kill said.
    group_kill: Result<(), String>,
}

impl Owned {
    fn end(&mut self) -> Result<(), String> {
        let group = kill_group(&self.tools, self.group);
        self.group_kill.clone_from(&group);
        let direct = match self.child.kill() {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
            Err(error) => Err(error.to_string()),
        };
        match reaped_within(&mut self.child, REAP_GRACE) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!(
                "worker {} is still running after the kill attempts (group kill {group:?}, direct kill {direct:?})",
                self.group
            )),
            Err(error) => Err(format!(
                "worker {} could not be waited for: {error}",
                self.group
            )),
        }
    }

    fn end_with_retry(&mut self) -> Result<(), String> {
        let result = self.end().or_else(|_| self.end());
        if result.is_ok() {
            self.settled = true;
        }
        result
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.settled
            && let Err(message) = self.end_with_retry()
        {
            eprintln!("window_cocoa: cleanup failed: {message}");
        }
    }
}

/// How a supervised worker ended.
#[derive(Debug)]
enum Outcome {
    Exited {
        code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    /// The deadline passed; the worker's group was killed and is empty, and the worker is reaped.
    TimedOut,
    /// The deadline passed (or a descendant held the pipes) and the group could not be shown to be
    /// ended: it could not be killed, listed or emptied.
    Unterminated(String),
    /// The worker exited but a diagnostic stream could not be read to its end: whatever the status and the
    /// bytes read say, the case did not produce complete evidence.
    ReadFailed(String),
}

/// The outcome of a worker that exited, from what reading its two streams gave. A read error in either is
/// a failure naming the stream, even for a zero status and streams that look clean.
fn exited(code: Option<i32>, stdout: Drained, stderr: Drained) -> Outcome {
    match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => Outcome::Exited {
            code,
            stdout,
            stderr,
        },
        (stdout, stderr) => Outcome::ReadFailed(
            [("stdout", stdout), ("stderr", stderr)]
                .into_iter()
                .filter_map(|(stream, read)| {
                    read.err()
                        .map(|error| format!("the worker's {stream} could not be read: {error}"))
                })
                .collect::<Vec<_>>()
                .join("; "),
        ),
    }
}

/// The Xcode developer directory: `DEVELOPER_DIR` when it is set, else what `xcode-select -p` says (a
/// bounded helper).
fn developer_dir(variable: Option<OsString>, tools: &Tools) -> Result<PathBuf, String> {
    match variable {
        Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
        _ => developer_path(run_helper(&tools.xcode_select, &["-p"], HELPER_BUDGET)),
    }
}

/// The developer directory `xcode-select -p` printed; output that was not read to its end is an error.
fn developer_path(printed: Result<String, HelperError>) -> Result<PathBuf, String> {
    Ok(PathBuf::from(printed?.trim()))
}

/// The Main Thread Checker library of the selected Xcode.
fn checker_library(tools: &Tools) -> Result<PathBuf, String> {
    let developer = developer_dir(env::var_os("DEVELOPER_DIR"), tools)?;
    let library = developer.join("usr/lib/libMainThreadChecker.dylib");
    if library.is_file() {
        Ok(library)
    } else {
        Err(format!(
            "the Main Thread Checker library {} is missing; the full check needs the selected Xcode's",
            library.display()
        ))
    }
}

/// How a worker is started.
#[derive(Debug, Clone, Copy)]
struct Run {
    /// How long it may run.
    deadline: Duration,
    /// Whether the checker crashes the worker on a report (`MTC_CRASH_ON_REPORT`).
    crash_on_report: bool,
    /// Whether the owner is typing without stopping while it runs.
    typing: bool,
}

/// Starts this executable as the worker for `case`, in its own process group, with the checker inserted,
/// and waits at most `run.deadline`. Every path after the spawn ends the worker, and every wait is bounded.
fn supervise(case: &str, run: Run, tools: &Tools) -> Result<Outcome, String> {
    let library = checker_library(tools)?;
    let mut command = Command::new(env::current_exe().map_err(|error| error.to_string())?);
    command
        .arg(WORKER_FLAG)
        .arg(case)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("DYLD_INSERT_LIBRARIES", &library);
    if run.crash_on_report {
        command.env("MTC_CRASH_ON_REPORT", "1");
    } else {
        command.env_remove("MTC_CRASH_ON_REPORT");
    }
    if run.typing {
        command.env(TYPING_VARIABLE, "1");
    } else {
        command.env_remove(TYPING_VARIABLE);
    }
    let child = command
        .spawn()
        .map_err(|error| format!("the worker could not be started: {error}"))?;
    let group = child.id();
    let mut owned = Owned {
        child,
        group,
        tools: tools.clone(),
        settled: false,
        group_kill: Ok(()),
    };
    let stdout = drain(owned.child.stdout.take()).map_err(|error| error.to_string())?;
    let stderr = drain(owned.child.stderr.take()).map_err(|error| error.to_string())?;
    let expiry = Instant::now() + run.deadline;
    loop {
        if let Some(status) = owned.child.try_wait().map_err(|error| error.to_string())? {
            owned.settled = true;
            return Ok(finish_worker(status.code(), &stdout, &stderr, tools, group));
        }
        if Instant::now() >= expiry {
            return Ok(end_after_deadline(&mut owned));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// A worker that exited: its two streams are waited for at most `PIPE_GRACE` each, and what they gave
/// decides the outcome. The pipes close when every member of the group is gone, so a descendant that kept
/// them open must not hang the supervisor (the group is killed and that is reported). A stream that could
/// not be read is a failure of its own, with the group killed as well, and it is reported with its stream
/// and its error even when the other stream did not finish.
fn finish_worker(
    code: Option<i32>,
    stdout: &mpsc::Receiver<Drained>,
    stderr: &mpsc::Receiver<Drained>,
    tools: &Tools,
    group: u32,
) -> Outcome {
    // Each stream's result is kept on its own: a sibling that times out or disconnects must not hide a
    // read error already received.
    let (stdout, stderr) = (
        stdout.recv_timeout(PIPE_GRACE).ok(),
        stderr.recv_timeout(PIPE_GRACE).ok(),
    );
    let mut errors = Vec::new();
    for (stream, read) in [("stdout", &stdout), ("stderr", &stderr)] {
        if let Some(Err(error)) = read {
            errors.push(format!("the worker's {stream} could not be read: {error}"));
        }
    }
    if errors.is_empty() {
        if let (Some(stdout), Some(stderr)) = (stdout, stderr) {
            return exited(code, stdout, stderr);
        }
        let killed = kill_group(tools, group);
        return Outcome::Unterminated(format!(
            "a descendant of the worker kept its output pipes open (kill result {killed:?})"
        ));
    }
    if stdout.is_none() || stderr.is_none() {
        errors.push(
            "its other stream did not finish within the pipe grace, or its reader ended without a result"
                .to_owned(),
        );
    }
    // The streams may be held by a survivor that made them fail: end the group as well.
    let killed = kill_group(tools, group);
    Outcome::ReadFailed(format!("{} (kill result {killed:?})", errors.join("; ")))
}

/// Ends the group after a deadline: kill it, reap the worker, then insist that the group is shown to be
/// empty. A group that cannot be killed or listed is reported as not ended.
fn end_after_deadline(owned: &mut Owned) -> Outcome {
    let group = owned.group;
    if let Err(survivor) = owned.end_with_retry() {
        owned.settled = true;
        return Outcome::Unterminated(survivor);
    }
    if let Err(killed) = owned.group_kill.clone() {
        return Outcome::Unterminated(format!("group {group} could not be killed: {killed}"));
    }
    let expiry = Instant::now() + PIPE_GRACE;
    loop {
        match group_members(&owned.tools, group) {
            Err(error) => {
                return Outcome::Unterminated(format!(
                    "group {group} could not be listed: {error}"
                ));
            }
            Ok(members) if members.is_empty() => return Outcome::TimedOut,
            Ok(members) if Instant::now() >= expiry => {
                return Outcome::Unterminated(format!("group {group} not emptied: {members:?}"));
            }
            Ok(_) => thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// `Ok` when a case's worker passed: it exited 0 and the checker reported nothing.
fn judge(case: &str, outcome: &Outcome) -> Result<(), String> {
    match outcome {
        Outcome::TimedOut => Err(format!(
            "{case} timed out after {} s and the worker's process group was killed",
            DEADLINE.as_secs()
        )),
        Outcome::Unterminated(detail) => Err(format!(
            "{case} did not finish and its process group could not be ended: {detail}"
        )),
        Outcome::ReadFailed(detail) => Err(format!(
            "{case}: its diagnostics are incomplete, so the case cannot pass: {detail}"
        )),
        Outcome::Exited {
            code,
            stdout,
            stderr,
        } => {
            if stderr.contains(CHECKER_REPORT) {
                return Err(format!(
                    "{case}: the Main Thread Checker reported:\n{stderr}"
                ));
            }
            if *code == Some(0) {
                Ok(())
            } else {
                Err(format!(
                    "{case}: the worker ended with {code:?}; stdout {stdout:?}, stderr {stderr:?}"
                ))
            }
        }
    }
}

fn run_case(name: &str, typing: bool) -> Result<(), String> {
    let run = Run {
        deadline: DEADLINE,
        crash_on_report: true,
        typing,
    };
    judge(name, &supervise(name, run, &Tools::system())?)
}

// -- The supervisor's own checks ----------------------------------------------------------------

/// A directory of fixture helper scripts, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = env::temp_dir().join(format!(
            "ck-window-cocoa-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self(path)
    }

    /// An executable script with this body.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.0.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("a helper script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("an executable helper script");
        path
    }

    /// A helper that records its pid in `pids/<name>` and then stalls (the `exec` leaves no child behind).
    fn stalled(&self, name: &str) -> (PathBuf, PathBuf) {
        let pid_file = self.0.join(format!("{name}.pid"));
        let script = self.script(
            name,
            &format!("echo $$ > '{}'\nexec /bin/sleep 60", pid_file.display()),
        );
        (script, pid_file)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether the process with this pid (recorded by a stalled helper) is still running.
fn running(pid_file: &Path) -> Result<bool, String> {
    let text = std::fs::read_to_string(pid_file)
        .map_err(|error| format!("the helper never recorded its pid: {error}"))?;
    let pid = text.trim();
    // `kill -0` exits 0 only for a process that exists.
    pid_verdict(
        run_helper(Path::new("/bin/kill"), &["-0", pid], HELPER_BUDGET),
        pid,
    )
}

/// Whether process `pid` exists, from `kill -0`: success is yes and its own unsuccessful exit is no;
/// anything else (a read error, a stall, a failure to run) cannot tell and is an error.
fn pid_verdict(result: Result<String, HelperError>, pid: &str) -> Result<bool, String> {
    match result {
        Ok(_) => Ok(true),
        Err(HelperError::Exit { .. }) => Ok(false),
        Err(error) => Err(format!(
            "could not tell whether process {pid} still runs: {error}"
        )),
    }
}

/// A hanging worker is killed and reaped at its deadline, and leaves no process behind.
fn check_a_hung_worker_is_killed() -> Result<(), String> {
    let run = Run {
        deadline: Duration::from_secs(2),
        crash_on_report: true,
        typing: false,
    };
    match supervise("hang", run, &Tools::system())? {
        Outcome::TimedOut => Ok(()),
        other => Err(format!(
            "a hung worker should time out cleanly, got {other:?}"
        )),
    }
}

/// The checker is live in the worker: an `AppKit` call off the main thread is reported, and with
/// crash-on-report the worker does not end successfully.
fn check_the_checker_reports_a_violation() -> Result<(), String> {
    let mut run = Run {
        deadline: DEADLINE,
        crash_on_report: false,
        typing: false,
    };
    let stderr = match supervise("checker_control", run, &Tools::system())? {
        Outcome::Exited { stderr, .. } => stderr,
        Outcome::ReadFailed(detail) => {
            return Err(format!(
                "the control worker's diagnostics could not be read: {detail}"
            ));
        }
        other => return Err(format!("the control worker did not exit: {other:?}")),
    };
    if !stderr.contains(CHECKER_REPORT) {
        return Err(format!(
            "the checker reported nothing for a call off the main thread; stderr {stderr:?}"
        ));
    }
    run.crash_on_report = true;
    match supervise("checker_control", run, &Tools::system())? {
        Outcome::Exited { code: Some(0), .. } => {
            Err("with crash-on-report the control worker still ended successfully".to_owned())
        }
        Outcome::Exited { .. } => Ok(()),
        Outcome::ReadFailed(detail) => Err(format!(
            "the crashing control worker's diagnostics could not be read: {detail}"
        )),
        other => Err(format!(
            "the crashing control worker did not exit: {other:?}"
        )),
    }
}

/// The longest the faulty-helper checks may take: the worker's deadline, a helper's budget, its reap and
/// pipe waits, and a margin.
const FAULT_BOUND: Duration = Duration::from_secs(14);

/// A stalled or failing `ps` after a hung worker's deadline is a bounded, reported failure (not an empty
/// group), within `FAULT_BOUND`, and leaves neither the worker nor the helper running.
fn check_a_faulty_process_list_is_bounded_and_fails_closed() -> Result<(), String> {
    let scratch = Scratch::new();
    let (stalled, pid_file) = scratch.stalled("stalled-ps");
    let failing = scratch.script("failing-ps", "exit 1");
    for (label, ps) in [("stalled", stalled), ("failing", failing)] {
        let tools = Tools {
            ps,
            ..Tools::system()
        };
        let run = Run {
            deadline: Duration::from_secs(2),
            crash_on_report: true,
            typing: false,
        };
        let started = Instant::now();
        let outcome = supervise("hang", run, &tools)?;
        let took = started.elapsed();
        match outcome {
            Outcome::Unterminated(detail) if detail.contains("could not be listed") => {}
            other => {
                return Err(format!(
                    "a {label} process list must be a reported failure, got {other:?}"
                ));
            }
        }
        if took > FAULT_BOUND {
            return Err(format!(
                "a {label} process list took {took:?}, over {FAULT_BOUND:?}"
            ));
        }
    }
    if running(&pid_file)? {
        return Err("the stalled process-list helper is still running".to_owned());
    }
    // The worker itself was ended: nothing of that run is left in a group the system tools can see.
    Ok(())
}

/// A stalled `kill` helper (and a failing shell fallback) cannot stall the supervisor either.
fn check_a_stalled_kill_helper_is_bounded() -> Result<(), String> {
    let scratch = Scratch::new();
    let (stalled, pid_file) = scratch.stalled("stalled-kill");
    let failing = scratch.script("failing-shell", "exit 1");
    let tools = Tools {
        kill: stalled,
        shell: failing,
        ..Tools::system()
    };
    let run = Run {
        deadline: Duration::from_secs(2),
        crash_on_report: true,
        typing: false,
    };
    let started = Instant::now();
    let outcome = supervise("hang", run, &tools)?;
    let took = started.elapsed();
    if !matches!(&outcome, Outcome::Unterminated(detail) if detail.contains("could not be killed"))
    {
        return Err(format!(
            "a stalled kill helper must be reported, got {outcome:?}"
        ));
    }
    if took > FAULT_BOUND * 2 {
        return Err(format!("a stalled kill helper took {took:?}"));
    }
    if running(&pid_file)? {
        return Err("the stalled kill helper is still running".to_owned());
    }
    Ok(())
}

/// A stalled `xcode-select` is a bounded error, and its helper is gone.
fn check_a_stalled_xcode_select_is_bounded() -> Result<(), String> {
    let scratch = Scratch::new();
    let (stalled, pid_file) = scratch.stalled("stalled-xcode-select");
    let tools = Tools {
        xcode_select: stalled,
        ..Tools::system()
    };
    let started = Instant::now();
    let result = developer_dir(None, &tools);
    let took = started.elapsed();
    if result.is_ok() {
        return Err("a stalled xcode-select must be an error".to_owned());
    }
    if took > HELPER_BUDGET + REAP_GRACE + PIPE_GRACE {
        return Err(format!("a stalled xcode-select took {took:?}"));
    }
    if running(&pid_file)? {
        return Err("the stalled xcode-select is still running".to_owned());
    }
    Ok(())
}

/// A reader that yields `prefix` and then fails (at once when the prefix is empty): what a pipe that breaks
/// mid-read does, which no real helper can be made to do on demand.
struct FailingRead {
    prefix: &'static [u8],
}

impl Read for FailingRead {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.prefix.is_empty() {
            return Err(std::io::Error::other("injected read failure"));
        }
        let taken = self.prefix.len().min(buffer.len());
        buffer[..taken].copy_from_slice(&self.prefix[..taken]);
        self.prefix = &self.prefix[taken..];
        Ok(taken)
    }
}

/// What reading a stream that fails after `prefix` gives.
fn failing(prefix: &'static [u8]) -> Drained {
    read_all(Some(FailingRead { prefix }))
}

/// The wait status of a process that exited with `code`.
fn exit_status(code: i32) -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(code << 8)
}

/// The two shapes of a read failure: before any byte, and after a prefix that looks clean.
const FAILURE_SHAPES: [&[u8]; 2] = [b"", b"1 1\n2 2\n"];

/// A read failure on either stream of a worker fails its case, even for a zero status and streams that
/// look clean, and the failure names the stream and the error.
fn check_a_read_failure_fails_a_worker() -> Result<(), String> {
    for prefix in FAILURE_SHAPES {
        for (stream, outcome) in [
            (
                "stdout",
                exited(Some(0), failing(prefix), Ok(String::new())),
            ),
            (
                "stderr",
                exited(Some(0), Ok(String::new()), failing(prefix)),
            ),
        ] {
            let Outcome::ReadFailed(detail) = &outcome else {
                return Err(format!(
                    "a failed {stream} read after {} bytes gave {outcome:?}",
                    prefix.len()
                ));
            };
            if !detail.contains(stream) || !detail.contains("injected read failure") {
                return Err(format!(
                    "the failure does not name {stream} and the error: {detail}"
                ));
            }
            match judge("control", &outcome) {
                Err(message)
                    if message.contains(stream) && message.contains("injected read failure") => {}
                other => {
                    return Err(format!(
                        "a failed {stream} read must fail the case and say so, got {other:?}"
                    ));
                }
            }
        }
    }
    let both = exited(Some(0), failing(b""), failing(b"x\n"));
    match &both {
        Outcome::ReadFailed(detail) if detail.contains("stdout") && detail.contains("stderr") => {}
        other => {
            return Err(format!(
                "two failed reads must both be named, got {other:?}"
            ));
        }
    }
    // The real reader thread hands the failure over as a failure, with how much it had read.
    let received = drain(Some(FailingRead { prefix: b"abc" }))
        .map_err(|error| error.to_string())?
        .recv_timeout(PIPE_GRACE)
        .map_err(|error| format!("the reader sent nothing: {error}"))?;
    match received {
        Err(error)
            if error.contains("injected read failure") && error.contains("after 3 bytes") => {}
        other => {
            return Err(format!(
                "the reader thread must send the failure, got {other:?}"
            ));
        }
    }
    // Controls: clean streams still pass, and a clean checker report is still the checker's.
    judge(
        "control",
        &exited(Some(0), Ok("fine\n".to_owned()), Ok(String::new())),
    )?;
    match judge(
        "control",
        &exited(
            Some(0),
            Ok(String::new()),
            Ok(format!("{CHECKER_REPORT}: a violation")),
        ),
    ) {
        Err(message) if message.contains(CHECKER_REPORT) => Ok(()),
        other => Err(format!(
            "a checker report must still fail the case, got {other:?}"
        )),
    }
}

/// The step `supervise` takes once the worker has exited, with a zero status and one stream that fails.
fn check_the_exit_step_fails_on_a_read_failure() -> Result<(), String> {
    // The step `supervise` takes when the worker has exited, with a zero status. The kill helpers do
    // nothing, so no real process group is touched.
    let scratch = Scratch::new();
    let marker = scratch.0.join("kill-called");
    let harmless = scratch.script("kill-nothing", &format!("touch '{}'", marker.display()));
    let tools = Tools {
        kill: harmless.clone(),
        shell: harmless,
        ..Tools::system()
    };
    let group = 4_000_000;
    for prefix in FAILURE_SHAPES {
        let bad = || drain(Some(FailingRead { prefix })).map_err(|error| error.to_string());
        let clean = || drain(Some(&b"fine\n"[..])).map_err(|error| error.to_string());
        for (stream, outcome) in [
            (
                "stdout",
                finish_worker(Some(0), &bad()?, &clean()?, &tools, group),
            ),
            (
                "stderr",
                finish_worker(Some(0), &clean()?, &bad()?, &tools, group),
            ),
        ] {
            match &outcome {
                Outcome::ReadFailed(detail)
                    if detail.contains(stream) && detail.contains("injected read failure") => {}
                other => {
                    return Err(format!(
                        "a worker that exited 0 with a failed {stream} read gave {other:?}"
                    ));
                }
            }
        }
    }
    if !marker.exists() {
        return Err("a read failure did not end the worker's group as well".to_owned());
    }
    std::fs::remove_file(&marker).map_err(|error| error.to_string())?;
    let clean = |text: &'static [u8]| drain(Some(text)).map_err(|error| error.to_string());
    let outcome = finish_worker(Some(0), &clean(b"out\n")?, &clean(b"")?, &tools, group);
    match outcome {
        Outcome::Exited {
            code: Some(0),
            stdout,
            ..
        } if stdout == "out\n" => {}
        other => return Err(format!("a worker with clean streams gave {other:?}")),
    }
    if marker.exists() {
        return Err("a worker with clean streams had its group killed".to_owned());
    }
    Ok(())
}

/// A worker that exited with one stream failing to be read and the other unable to finish (held open past
/// the pipe grace, or its reader gone): the read error is still reported, with its stream and error text.
fn check_a_read_error_survives_a_sibling_that_cannot_finish() -> Result<(), String> {
    let scratch = Scratch::new();
    let harmless = scratch.script("kill-nothing", "exit 0");
    let tools = Tools {
        kill: harmless.clone(),
        shell: harmless,
        ..Tools::system()
    };
    let group = 4_000_000;
    for bad_is_stdout in [true, false] {
        for prefix in FAILURE_SHAPES {
            for held_open in [false, true] {
                let bad = drain(Some(FailingRead { prefix })).map_err(|error| error.to_string())?;
                let (sender, sibling) = mpsc::channel::<Drained>();
                // Held open: the sender lives through the call and sends nothing. Disconnected: it is gone.
                let kept = held_open.then_some(sender);
                let (stdout, stderr) = if bad_is_stdout {
                    (&bad, &sibling)
                } else {
                    (&sibling, &bad)
                };
                let stream = if bad_is_stdout { "stdout" } else { "stderr" };
                let outcome = finish_worker(Some(0), stdout, stderr, &tools, group);
                drop(kept);
                let after = format!("after {} bytes", prefix.len());
                match &outcome {
                    Outcome::ReadFailed(detail)
                        if detail.contains(&format!("the worker's {stream} could not be read"))
                            && detail.contains("injected read failure")
                            && detail.contains(&after)
                            && detail.contains("did not finish") => {}
                    other => {
                        return Err(format!(
                            "a failed {stream} read ({after}) with its sibling {} gave {other:?}",
                            if held_open {
                                "held open"
                            } else {
                                "disconnected"
                            }
                        ));
                    }
                }
                match judge("control", &outcome) {
                    Err(message)
                        if message.contains(stream)
                            && message.contains("injected read failure") => {}
                    other => {
                        return Err(format!(
                            "the case must fail naming the {stream} read error, got {other:?}"
                        ));
                    }
                }
            }
        }
    }
    // Control: with no read error, a sibling held open is still the pipes-held-open failure.
    let (_sender, open) = mpsc::channel::<Drained>();
    let clean = drain(Some(&b"fine\n"[..])).map_err(|error| error.to_string())?;
    match finish_worker(Some(0), &clean, &open, &tools, group) {
        Outcome::Unterminated(detail) if detail.contains("kept its output pipes open") => Ok(()),
        other => Err(format!("a held-open stream alone gave {other:?}")),
    }
}

/// A read failure never makes a helper's answer: a listing that would look empty or complete if truncated,
/// a developer directory, a symbol list and `kill -0`'s "no process" are all errors.
fn check_a_read_failure_is_never_a_helper_answer() -> Result<(), String> {
    for prefix in FAILURE_SHAPES {
        let shape = format!("after {} bytes", prefix.len());
        let result = helper_result("ps", exit_status(0), failing(prefix));
        match &result {
            Err(HelperError::Read { name, error })
                if name == "ps" && error.contains("injected read failure") => {}
            other => return Err(format!("a ps read failure {shape} gave {other:?}")),
        }
        let text = result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        if !text.contains("ps") || !text.contains("injected read failure") {
            return Err(format!(
                "the reported error does not name the helper and the error: {text}"
            ));
        }
        if let Ok(members) = members_of(result, 7) {
            return Err(format!(
                "a partial listing {shape} gave the group {members:?}"
            ));
        }
        let developer = developer_path(helper_result(
            "xcode-select",
            exit_status(0),
            failing(prefix),
        ));
        if developer.is_ok() {
            return Err(format!(
                "a partial developer directory {shape} was taken: {developer:?}"
            ));
        }
        let symbols = helper_result("nm", exit_status(0), failing(prefix));
        if symbols.is_ok() {
            return Err(format!(
                "a partial symbol list {shape} was taken: {symbols:?}"
            ));
        }
        for code in [0, 1] {
            let kill = pid_verdict(
                helper_result("kill", exit_status(code), failing(prefix)),
                "1",
            );
            if kill.is_ok() {
                return Err(format!(
                    "kill -0 exit {code} with a read failure {shape} gave {kill:?}"
                ));
            }
        }
    }
    Ok(())
}

/// The positive controls: a helper read to its end, including an empty listing and the exit statuses that
/// mean something, is still an answer, through the real helper runner as well.
fn check_clean_helper_output_is_still_an_answer() -> Result<(), String> {
    let listing = |text: &str, group| {
        members_of(
            helper_result("ps", exit_status(0), Ok(text.to_owned())),
            group,
        )
    };
    if listing("1 7\n2 7\n3 9\n", 7) != Ok(vec![1, 2]) {
        return Err("a complete listing did not give its members".to_owned());
    }
    if listing("", 7) != Ok(Vec::new()) {
        return Err("a complete empty listing is an empty group".to_owned());
    }
    let kill = |code| {
        pid_verdict(
            helper_result("kill", exit_status(code), Ok(String::new())),
            "1",
        )
    };
    if kill(0) != Ok(true) || kill(1) != Ok(false) {
        return Err("kill -0's complete answers changed".to_owned());
    }
    let printed = developer_path(helper_result(
        "xcode-select",
        exit_status(0),
        Ok("/Applications/Xcode.app/Contents/Developer\n".to_owned()),
    ));
    if printed != Ok(PathBuf::from("/Applications/Xcode.app/Contents/Developer")) {
        return Err(format!(
            "a complete developer directory changed: {printed:?}"
        ));
    }
    let scratch = Scratch::new();
    let speaks = scratch.script("speaks", "printf '4 7\\n'");
    match run_helper(&speaks, &[], HELPER_BUDGET) {
        Ok(text) if text == "4 7\n" => {}
        other => return Err(format!("a helper that prints and exits 0 gave {other:?}")),
    }
    let silent = scratch.script("silent-exit-1", "exit 1");
    match run_helper(&silent, &[], HELPER_BUDGET) {
        Err(HelperError::Exit { code: Some(1), .. }) => Ok(()),
        other => Err(format!("a helper that exits 1 gave {other:?}")),
    }
}

/// The library carries none of the test support: its archive has the backend's symbols and no
/// `ck_win_test_` function or test class, and the test support's archive has them (so the check looks at
/// the right things).
fn check_test_support_is_not_in_the_library() -> Result<(), String> {
    let nm = Path::new("/usr/bin/nm");
    let library = run_helper(nm, &["-g", env!("CK_WINDOW_COCOA_ARCHIVE")], HELPER_BUDGET)?;
    let testing = run_helper(
        nm,
        &["-g", env!("CK_WINDOW_COCOA_TESTING_ARCHIVE")],
        HELPER_BUDGET,
    )?;
    if !library.contains("_ck_win_pump") {
        return Err("the library archive does not have the backend's symbols".to_owned());
    }
    for forbidden in ["ck_win_test_", "CKTest"] {
        if library.contains(forbidden) {
            return Err(format!(
                "the library archive contains test support: {forbidden}"
            ));
        }
    }
    if !testing.contains("_ck_win_test_install_input_filter")
        || !testing.contains("CKTestApplication")
    {
        return Err("the test support archive lacks the filter or the test classes".to_owned());
    }
    if testing.contains("_ck_win_pump") {
        return Err("the test support archive repeats the backend".to_owned());
    }
    Ok(())
}

/// A check of the supervisor's own machinery.
type SupervisorCheck = fn() -> Result<(), String>;

fn supervisor() -> ExitCode {
    if let Err(message) = checker_library(&Tools::system()) {
        eprintln!("window_cocoa: {message}");
        return ExitCode::FAILURE;
    }
    let mut failures = BTreeMap::new();
    let checks: [(&str, SupervisorCheck); 11] = [
        (
            "a hung worker is killed and reaped",
            check_a_hung_worker_is_killed,
        ),
        (
            "the checker reports a violation off the main thread",
            check_the_checker_reports_a_violation,
        ),
        (
            "a faulty process list is bounded and fails closed",
            check_a_faulty_process_list_is_bounded_and_fails_closed,
        ),
        (
            "a stalled kill helper is bounded",
            check_a_stalled_kill_helper_is_bounded,
        ),
        (
            "a stalled xcode-select is bounded",
            check_a_stalled_xcode_select_is_bounded,
        ),
        (
            "a read failure fails a worker whatever its status and bytes",
            check_a_read_failure_fails_a_worker,
        ),
        (
            "the exit step fails on a read failure whatever the status",
            check_the_exit_step_fails_on_a_read_failure,
        ),
        (
            "a read error survives a sibling stream that cannot finish",
            check_a_read_error_survives_a_sibling_that_cannot_finish,
        ),
        (
            "a read failure is never a complete helper answer",
            check_a_read_failure_is_never_a_helper_answer,
        ),
        (
            "a helper that reads cleanly is still an answer",
            check_clean_helper_output_is_still_an_answer,
        ),
        (
            "the test support is not in the library",
            check_test_support_is_not_in_the_library,
        ),
    ];
    for (name, check) in checks {
        let result = check();
        println!(
            "test supervisor: {name} ... {}",
            if result.is_ok() { "ok" } else { "FAILED" }
        );
        if let Err(message) = result {
            failures.insert(name.to_owned(), message);
        }
    }
    // Every case runs on a quiet keyboard, and again while the owner types without stopping.
    for typing in [false, true] {
        for (name, _) in CASES {
            let label = if typing {
                format!("{name} (owner typing)")
            } else {
                name.to_owned()
            };
            let started = Instant::now();
            let result = run_case(name, typing);
            println!(
                "test {label} ... {} ({:.1} s)",
                if result.is_ok() { "ok" } else { "FAILED" },
                started.elapsed().as_secs_f64()
            );
            if let Err(message) = result {
                failures.insert(label, message);
            }
        }
    }
    for (name, message) in &failures {
        println!("---- {name} ----\n{message}");
    }
    let total = checks.len() + 2 * CASES.len();
    println!(
        "test result: {}. {} passed; {} failed",
        if failures.is_empty() { "ok" } else { "FAILED" },
        total - failures.len(),
        failures.len()
    );
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [flag, case] if flag == WORKER_FLAG => worker(case),
        // Cargo may pass its own arguments (a filter, `--nocapture`); the cases always all run.
        _ => supervisor(),
    }
}
