//! The GLFW window backend, tested through real window-system dispatch, as a supervisor and a worker from
//! this one executable (`harness = false`, so each case's result appears in plain `cargo test` output).
//!
//! GLFW's Cocoa window needs the process's main thread, so every case runs in a **worker**: this
//! executable started again with `--worker <case>`. The **supervisor** (the shared, bounded supervisor of
//! `apps/codekarl/tests/supervisor`, through `#[path]`) owns each worker from the instant it is spawned,
//! gives it 30 s, and on expiry kills and reaps it; every kill, reap and pipe read is bounded, a read error
//! is a failure that names the stream, and each reader's real end is observed within its grace. A worker
//! fails (nothing is skipped) without a desktop session, GLFW or the Vulkan loader.
//!
//! **The owner may be typing while this runs, Escape included.** Before its first window the worker
//! installs a test-only filter (a local event monitor, inside `-[NSApplication sendEvent:]`, before GLFW
//! sees the event) that drops every keyboard event without the tests' mark, and every case runs twice, once
//! on a quiet keyboard and once with a thread typing without stopping. The tests' own synthetic events
//! carry the mark; the assertions count only those. The filter and the typist live in this test's own
//! native archive (`tests/native`), which only the test executable links; a supervisor check proves the
//! library's rlib has none of it.

#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::env;
use std::ffi::{CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use ck_window::{Backend, NativeEvent, NativeWake, Notifier, PixelSize, Wait, WindowDesc};
use ck_window_glfw::{GlfwBackend, GlfwHandles, PumpReport};

type IconifyCallback = extern "C" fn(window: *mut c_void, iconified: i32);
static ORIGINAL_ICONIFY_CALLBACK: OnceLock<IconifyCallback> = OnceLock::new();
static RESTORE_CALLBACK_ACK: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn glfwCreateWindow(
        width: i32,
        height: i32,
        title: *const c_char,
        monitor: *mut c_void,
        share: *mut c_void,
    ) -> *mut c_void;
    fn glfwDestroyWindow(window: *mut c_void);
    fn glfwIconifyWindow(window: *mut c_void);
    fn glfwRestoreWindow(window: *mut c_void);
    fn glfwFocusWindow(window: *mut c_void);
    fn glfwPollEvents();
    fn glfwWaitEvents();
    fn glfwSetWindowIconifyCallback(
        window: *mut c_void,
        callback: Option<IconifyCallback>,
    ) -> Option<IconifyCallback>;
}

extern "C" fn acknowledge_restore_callback(window: *mut c_void, iconified: i32) {
    if let Some(original) = ORIGINAL_ICONIFY_CALLBACK.get() {
        original(window, iconified);
    }
    if iconified == 0 {
        RESTORE_CALLBACK_ACK.store(true, Ordering::SeqCst);
    }
}

#[path = "../../window/tests/support/activation_watch.rs"]
mod activation_watch;
#[path = "../../../apps/codekarl/tests/supervisor/mod.rs"]
#[expect(
    dead_code,
    reason = "the shared supervisor serves other tests; this one uses its spawn, finish and cleanup"
)]
mod supervisor;
mod support;

use supervisor::Supervised;

const WORKER_FLAG: &str = "--worker";
/// `1` makes a worker run with the owner typing without stopping.
const TYPING_VARIABLE: &str = "CK_GLFW_TEST_TYPING";
/// The longest a worker may run.
const DEADLINE: Duration = Duration::from_secs(30);
/// The longest any wait for something that must happen.
const BOUND: Duration = Duration::from_secs(10);
/// CK-165's restore must end the backend wait inside this bound, well before its far deadline.
const RESTORE_WAKE_BOUND: Duration = Duration::from_secs(1);
/// The window every case opens: 400 by 300 points.
const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;
const FOCUS_CASES: [&str; 2] = ["focus_and_minimize", "the_window_becomes_active_by_itself"];

/// A backend with the Vulkan loader the tests loaded by its absolute path, as the program does.
fn new_backend() -> GlfwBackend {
    new_backend_watched().0
}

/// The same, with the flag that says whether the loader's keep-alive has been released.
fn new_backend_watched() -> (GlfwBackend, Arc<AtomicBool>) {
    let (loader, released) = support::loader_for_glfw();
    (
        GlfwBackend::new(Some(loader)).expect("GLFW starts on the main thread"),
        released,
    )
}

/// A size in points times a scale factor, in pixels.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the sizes are a few hundred points times a scale of 1 or 2: the cast is exact"
)]
fn pixels(points: u32, scale: f64) -> u32 {
    (f64::from(points) * scale).round() as u32
}

fn desc() -> WindowDesc {
    WindowDesc {
        title: "glfw test".to_owned(),
        width: WIDTH,
        height: HEIGHT,
        clear: [0.1, 0.2, 0.3],
    }
}

// ---------------------------------------------------------------------------------------------
// The rig
// ---------------------------------------------------------------------------------------------

/// A backend with its window open and key, torn down in the loop's order on every path.
struct Rig {
    backend: GlfwBackend,
    handles: Option<GlfwHandles>,
    pixel_size: PixelSize,
}

impl Rig {
    fn open() -> Self {
        let mut backend = new_backend();
        let created = backend
            .create_window(&desc())
            .expect("the window is created");
        let mut rig = Self {
            backend,
            handles: Some(created.handles),
            pixel_size: created.pixel_size,
        };
        let give_up = Instant::now() + BOUND;
        while !support::info().visible {
            assert!(
                Instant::now() < give_up,
                "the window never became visible: is there a desktop session? {:?}",
                support::info()
            );
            rig.pump(Wait::Poll);
            thread::sleep(Duration::from_millis(5));
        }
        // GLFW may report the activation caused by opening this window on the next pump.
        // Keep that setup event out of each case's own assertions.
        let _ = rig.pump_for(Duration::from_millis(100));
        rig
    }

    fn handles(&self) -> &GlfwHandles {
        self.handles.as_ref().expect("the window is open")
    }

    fn window(&self) -> *mut c_void {
        self.handles().window()
    }

    fn pump(&mut self, wait: Wait) -> PumpReport {
        self.backend.pump_with_report(wait, 16)
    }

    /// Polls until `found` accepts a report, for at most `BOUND`.
    fn pump_until(&mut self, mut found: impl FnMut(&PumpReport) -> bool) -> Option<PumpReport> {
        let give_up = Instant::now() + BOUND;
        while Instant::now() < give_up {
            let report = self.pump(Wait::Poll);
            if found(&report) {
                return Some(report);
            }
            thread::sleep(Duration::from_millis(2));
        }
        None
    }

    /// Polls for `span` and returns every report that handed something over.
    fn pump_for(&mut self, span: Duration) -> Vec<PumpReport> {
        let until = Instant::now() + span;
        let mut seen = Vec::new();
        while Instant::now() < until {
            let report = self.pump(Wait::Poll);
            if !quiet(&report) {
                seen.push(report);
            }
            thread::sleep(Duration::from_millis(2));
        }
        seen
    }

    fn close_requests(reports: &[PumpReport]) -> Vec<NativeEvent> {
        reports.iter().filter_map(|report| report.close).collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // The loop's order: the window, then the library.
        self.handles = None;
        self.backend.destroy_window();
        self.backend.terminate();
    }
}

/// A report that handed over no close request and no size. A window refresh (needs-display) may come at
/// any time, from the window system alone, so the cases never count on its absence.
fn quiet(report: &PumpReport) -> bool {
    report.input.is_empty()
        && report.close.is_none()
        && report.pixel_size.is_none()
        && !report.minimized
        && !report.restored
        && report.absorbed_close_presses == 0
}

/// Whether `returns` empty returns of the blocking pump within `elapsed` is a busy loop. System events end a
/// blocking wait one at a time: the window being ordered or activated gives a handful, and a pointer moving
/// over the window gives one per pointer event (about 120 a second). A busy loop returns at CPU speed,
/// thousands of times a second. So a fixed slack plus one return per 2 ms (500 a second, four times the
/// pointer's rate) passes the first two and fails the third.
fn too_many_returns(returns: usize, elapsed: Duration) -> bool {
    let allowed = 16 + usize::try_from(elapsed.as_millis() / 2).unwrap_or(usize::MAX / 2);
    returns > allowed
}

/// What one wait for the test's own wake took.
#[derive(Debug)]
struct WakeWait {
    /// From the call until the wake was observed.
    waited: Duration,
    /// How many times the blocking pump returned empty before that.
    early_returns: usize,
}

/// Blocks in `pump(Wait::UntilEvent)` until the wake posted from another thread `delay` after the call is
/// observed (the backend's posted-wake count moved), the way the loop pumps. `glfwWaitEvents` ends at **any**
/// event the window system delivers to the application (a pointer event as the window is ordered under the
/// cursor, the application being activated or deactivated by another process, ...), so a return with nothing
/// to hand over before the wake is legitimate: each such return is counted, not failed, and only a count
/// that `too_many_returns` calls a busy loop fails. Every report must still be empty ("nothing else was asked
/// for"). `disturbance` is an optional test-only thread body run `at` after the call.
fn wait_for_the_wake(
    rig: &mut Rig,
    target: &Arc<dyn NativeWake>,
    delay: Duration,
    disturbance: Option<(Duration, Box<dyn FnOnce() + Send>)>,
) -> Result<WakeWait, String> {
    let before = rig.backend.wake_counts().posted;
    let waker = Arc::clone(target);
    let helper = thread::spawn(move || {
        thread::sleep(delay);
        waker.wake();
    });
    let disturber = disturbance.map(|(at, body)| {
        thread::spawn(move || {
            thread::sleep(at);
            body();
        })
    });
    let outcome = bounded_wait(|| {
        let report = rig.pump(Wait::UntilEvent);
        (report, rig.backend.wake_counts().posted != before)
    });
    helper.join().expect("the waker ended");
    if let Some(disturber) = disturber {
        disturber.join().expect("the disturber ended");
    }
    outcome
}

/// The loop of `wait_for_the_wake`, over any step (one blocking pump and whether the wake has arrived): steps
/// while each return is empty and the wake has not arrived, counting the empty returns, and fails on a report
/// with something in it, on a busy loop and on a wake that never comes.
fn bounded_wait(mut step: impl FnMut() -> (PumpReport, bool)) -> Result<WakeWait, String> {
    let started = Instant::now();
    let mut early_returns = 0;
    loop {
        let (report, woken) = step();
        if !quiet(&report) {
            return Err(format!("nothing else was asked for: {report:?}"));
        }
        if woken {
            return Ok(WakeWait {
                waited: started.elapsed(),
                early_returns,
            });
        }
        early_returns += 1;
        if too_many_returns(early_returns, started.elapsed()) {
            return Err(format!(
                "{early_returns} empty returns before the wake: a busy loop, not system events"
            ));
        }
        if started.elapsed() > BOUND {
            return Err("the wake never arrived".to_owned());
        }
    }
}

/// The wait the test used to make: exactly one blocking pump, which must not return before the wake. Returns
/// how long that pump took, with the wake (and `disturbance`) posted from other threads as above.
fn one_blocking_pump(
    rig: &mut Rig,
    target: &Arc<dyn NativeWake>,
    delay: Duration,
    disturbance: Option<(Duration, Box<dyn FnOnce() + Send>)>,
) -> Duration {
    let waker = Arc::clone(target);
    let helper = thread::spawn(move || {
        thread::sleep(delay);
        waker.wake();
    });
    let disturber = disturbance.map(|(at, body)| {
        thread::spawn(move || {
            thread::sleep(at);
            body();
        })
    });
    let started = Instant::now();
    assert!(quiet(&rig.pump(Wait::UntilEvent)));
    let waited = started.elapsed();
    helper.join().expect("the waker ended");
    if let Some(disturber) = disturber {
        disturber.join().expect("the disturber ended");
    }
    // The wake it did not wait for is still queued: take it, so the next wait starts clean.
    rig.pump(Wait::Poll);
    waited
}

fn marked_escape() {
    assert!(support::post_key(support::ESCAPE, 0, false, "\u{1b}", true));
}

fn marked_escape_up() {
    assert!(support::post_keyboard_event(1, support::ESCAPE, 0, true));
}

/// A press and its release, both marked.
fn marked_escape_tap() {
    marked_escape();
    marked_escape_up();
}

fn marked_key_tap(key_code: u16, characters: &str) {
    assert!(support::post_key(key_code, 0, false, characters, true));
    assert!(support::post_keyboard_event(1, key_code, 0, true));
}

fn key_events(report: &PumpReport) -> Vec<NativeEvent> {
    report
        .input
        .iter()
        .filter(|event| matches!(event, NativeEvent::Key { .. } | NativeEvent::KeyResync(_)))
        .copied()
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------------------------

/// 1. A fixed-size window with the description's title and size; its handles; a clean teardown.
fn create_and_destroy() {
    let mut backend = new_backend();
    assert!(
        GlfwBackend::glfw_version() >= (3, 4, 0),
        "{:?}",
        GlfwBackend::glfw_version()
    );
    assert!(!support::info().has_window);
    let created = backend
        .create_window(&desc())
        .expect("the window is created");
    assert!(
        support::proc_addr_calls() > 0,
        "GLFW used the loader function it was given"
    );
    let info = support::info();
    assert!(info.has_window && info.visible);
    assert_eq!(info.title, "glfw test");
    assert_eq!(info.content, (f64::from(WIDTH), f64::from(HEIGHT)));
    assert!(!info.resizable, "the window is fixed-size");
    let scale = info.window_scale;
    assert_eq!(
        created.pixel_size,
        PixelSize {
            width: pixels(WIDTH, scale),
            height: pixels(HEIGHT, scale)
        },
        "the pixel size is the size in points times the scale"
    );
    assert!(!created.handles.window().is_null());
    assert!(
        backend.create_window(&desc()).is_err(),
        "one window at a time"
    );
    backend.destroy_window();
    assert_eq!(support::info().window_count, 0, "no window is left behind");
    backend.destroy_window(); // idempotent
    backend.terminate();
    backend.terminate(); // idempotent
    assert_eq!(backend.refused_terminations(), 0);
}

/// 2. Off the main thread is an error, before any GLFW call, whatever the thread is called (unnamed,
///    named `main`, named otherwise), and a backend can follow.
fn off_the_main_thread() {
    let calls = support::proc_addr_calls();
    for name in [None, Some("main"), Some("not-main")] {
        let builder = match name {
            Some(name) => thread::Builder::new().name(name.to_owned()),
            None => thread::Builder::new(),
        };
        let (loader, released) = support::loader_for_glfw();
        let elsewhere = builder
            .spawn(move || GlfwBackend::new(Some(loader)).err())
            .expect("a thread")
            .join()
            .expect("the thread ended");
        let message = elsewhere.expect("creating a backend off the main thread is an error");
        assert!(message.contains("main thread"), "{name:?}: {message}");
        assert!(
            released.load(Ordering::SeqCst),
            "{name:?}: the refused loader is released"
        );
        assert_eq!(
            support::proc_addr_calls(),
            calls,
            "{name:?}: GLFW was not touched"
        );
    }
    // Nothing was left initialised: the main thread can create one.
    drop(new_backend());
}

/// 3. One backend at a time; terminate refuses while a window exists, and a backend can follow a
///    terminated one.
fn one_backend_at_a_time_and_terminate_refuses_with_a_window() {
    let mut backend = new_backend();
    let second = GlfwBackend::new(None).expect_err("a second backend is refused");
    assert!(second.contains("already exists"), "{second}");
    backend
        .create_window(&desc())
        .expect("the window is created");
    backend.terminate();
    assert_eq!(backend.refused_terminations(), 1);
    assert!(support::info().has_window, "the window is still there");
    backend.destroy_window();
    backend.terminate();
    assert_eq!(backend.refused_terminations(), 1);
    drop(backend);
    let mut again = new_backend();
    again.create_window(&desc()).expect("and its window");
}

/// 4. The close button: one request with the right source, the flag reset, the window alive.
fn close_button() {
    let mut rig = Rig::open();
    for _ in 0..2 {
        support::perform_close();
        let report = rig
            .pump_until(|report| report.close.is_some())
            .expect("a close request");
        assert_eq!(report.close, Some(NativeEvent::CloseButton));
        assert_eq!(report.absorbed_close_presses, 0);
        assert!(
            !support::should_close(rig.window()),
            "the close flag was reset"
        );
        let info = support::info();
        assert!(
            info.visible && info.window_count == 1,
            "the window stays until teardown"
        );
        let more = rig.pump_for(Duration::from_millis(150));
        assert!(
            Rig::close_requests(&more).is_empty(),
            "exactly one request: {more:?}"
        );
    }
}

/// 5. Escape is recorded as a normal key; the default close binding is applied by the loop.
fn escape() {
    let mut rig = Rig::open();
    marked_escape_tap();
    let report = rig
        .pump_until(|report| !report.input.is_empty())
        .expect("an Escape transition");
    assert_eq!(
        key_events(&report),
        [
            NativeEvent::Key {
                key: ck_input::Key::Escape,
                down: true,
                repeat: false
            },
            NativeEvent::Key {
                key: ck_input::Key::Escape,
                down: false,
                repeat: false
            },
        ]
    );
    assert_eq!(
        report.close, None,
        "the backend does not hardcode Escape as close"
    );
    assert!(
        !support::should_close(rig.window()),
        "Escape never sets the flag"
    );
    assert!(support::info().visible);
    // A key-up alone asks for nothing.
    marked_escape_up();
    assert!(Rig::close_requests(&rig.pump_for(Duration::from_millis(150))).is_empty());
}

/// 6. Key transitions are preserved; duplicate state changes are dropped and held repeats are marked.
fn presses_coalesce() {
    let mut rig = Rig::open();
    // Three taps (press, release) remain three ordered key pairs.
    for _ in 0..3 {
        marked_escape_tap();
    }
    let report = rig
        .pump_until(|report| !report.input.is_empty())
        .expect("the Escape taps are recorded");
    assert_eq!(key_events(&report).len(), 6);
    assert!(key_events(&report).iter().all(|event| matches!(
        event,
        NativeEvent::Key {
            key: ck_input::Key::Escape,
            ..
        }
    )));
    // The first key-down presses; an explicit repeat below verifies GLFW's repeat flag.
    assert!(support::post_key(support::ESCAPE, 0, false, "\u{1b}", true));
    let held = rig
        .pump_until(|report| !report.input.is_empty())
        .expect("the held Escape is recorded");
    assert_eq!(
        key_events(&held).len(),
        1,
        "the initial key-down is delivered once"
    );
    assert!(support::post_key(support::ESCAPE, 0, true, "\u{1b}", true));
    let repeated = rig
        .pump_until(|report| !report.input.is_empty())
        .expect("a repeat is recorded while held");
    assert_eq!(
        key_events(&repeated),
        [NativeEvent::Key {
            key: ck_input::Key::Escape,
            down: true,
            repeat: true
        }]
    );
    marked_escape_up();
    rig.pump_for(Duration::from_millis(100));
    // The button and Escape are independent: each retains its own event shape.
    support::perform_close();
    marked_escape_tap();
    let both = rig.pump(Wait::Poll);
    assert_eq!(both.close, Some(NativeEvent::CloseButton));
    assert_eq!(key_events(&both).len(), 2);
}

/// 7. The pump drains every native event in one call and hands over every recorded key transition.
fn the_pump_drains_everything_and_hands_over_one() {
    let mut rig = Rig::open();
    for _ in 0..100 {
        marked_escape_tap();
    }
    let first = rig.pump(Wait::Poll);
    assert_eq!(
        key_events(&first).len(),
        200,
        "all transitions drain in one call"
    );
    assert_eq!(first.close, None);
    assert!(quiet(&rig.pump(Wait::Poll)), "nothing is left");
    // Through the `Backend` interface: the GLFW drain preserves every transition even with cap = 1.
    for _ in 0..10 {
        marked_escape_tap();
    }
    let mut handed = Vec::new();
    rig.backend
        .pump(&mut |event| handed.push(event), Wait::Poll, 1);
    assert_eq!(
        handed
            .into_iter()
            .filter(|event| matches!(event, NativeEvent::Key { .. } | NativeEvent::KeyResync(_)))
            .collect::<Vec<_>>(),
        (0..10)
            .flat_map(|_| [
                NativeEvent::Key {
                    key: ck_input::Key::Escape,
                    down: true,
                    repeat: false
                },
                NativeEvent::Key {
                    key: ck_input::Key::Escape,
                    down: false,
                    repeat: false
                },
            ])
            .collect::<Vec<_>>(),
        "the cap bounds neither callback recording nor the GLFW drain"
    );
}

/// 7a. Representative keys from each mapping family arrive normalized, and unknown keys are dropped.
fn normalized_keys() {
    let mut rig = Rig::open();
    let cases = [
        (0, "a", ck_input::Key::KeyA),     // letter
        (18, "1", ck_input::Key::Digit1),  // digit
        (122, "", ck_input::Key::F1),      // function
        (126, "", ck_input::Key::ArrowUp), // arrow
        (49, " ", ck_input::Key::Space),   // control/punctuation
        (115, "", ck_input::Key::Home),    // navigation
        (27, "-", ck_input::Key::Minus),   // punctuation
    ];
    for (code, text, key) in cases {
        marked_key_tap(code, text);
        let report = rig
            .pump_until(|report| {
                report.input.iter().any(|event| {
                    matches!(event,
                NativeEvent::Key { key: actual, .. } if *actual == key)
                })
            })
            .unwrap_or_else(|| panic!("GLFW did not report {key:?}"));
        let events = key_events(&report);
        assert_eq!(
            events,
            [
                NativeEvent::Key {
                    key,
                    down: true,
                    repeat: false
                },
                NativeEvent::Key {
                    key,
                    down: false,
                    repeat: false
                },
            ],
            "mapping for {key:?}"
        );
    }
    // Carbon keycode 0xFFFF has no GLFW key mapping. Follow it with a known key so an empty report
    // cannot make this check pass without dispatching either event.
    assert!(support::post_key(u16::MAX, 0, false, "", true));
    marked_key_tap(0, "a");
    let report = rig
        .pump_until(|report| !key_events(report).is_empty())
        .expect("the known key after the unmapped key is delivered");
    assert!(
        key_events(&report).iter().all(|event| matches!(
            event,
            NativeEvent::Key {
                key: ck_input::Key::KeyA,
                ..
            }
        )),
        "unknown token is dropped and only the known key arrives: {report:?}"
    );
    assert_eq!(
        key_events(&report),
        [
            NativeEvent::Key {
                key: ck_input::Key::KeyA,
                down: true,
                repeat: false
            },
            NativeEvent::Key {
                key: ck_input::Key::KeyA,
                down: false,
                repeat: false
            },
        ]
    );
}

/// 7b. Modifier callbacks preserve left/right identity and held-state order for every modifier family.
fn modifier_sides_and_order() {
    let mut rig = Rig::open();
    let cases = [
        (
            56,
            60,
            1 << 17,
            ck_input::Key::ShiftLeft,
            ck_input::Key::ShiftRight,
        ),
        (
            59,
            62,
            1 << 18,
            ck_input::Key::ControlLeft,
            ck_input::Key::ControlRight,
        ),
        (
            58,
            61,
            1 << 19,
            ck_input::Key::AltLeft,
            ck_input::Key::AltRight,
        ),
        (
            55,
            54,
            1 << 20,
            ck_input::Key::MetaLeft,
            ck_input::Key::MetaRight,
        ),
    ];
    for (left_code, right_code, flag, left, right) in cases {
        for right_first in [false, true] {
            let (first_code, first_key, second_code, second_key) = if right_first {
                (right_code, right, left_code, left)
            } else {
                (left_code, left, right_code, right)
            };
            assert!(support::post_keyboard_event(2, first_code, flag, true));
            assert!(support::post_keyboard_event(2, second_code, flag, true));
            assert!(support::post_keyboard_event(2, first_code, flag, true));
            assert!(support::post_keyboard_event(2, second_code, 0, true));
            let report = rig
                .pump_until(|report| report.input.len() >= 4)
                .expect("modifier side transitions");
            assert_eq!(
                key_events(&report),
                [
                    NativeEvent::Key {
                        key: first_key,
                        down: true,
                        repeat: false
                    },
                    NativeEvent::Key {
                        key: second_key,
                        down: true,
                        repeat: false
                    },
                    NativeEvent::Key {
                        key: first_key,
                        down: false,
                        repeat: false
                    },
                    NativeEvent::Key {
                        key: second_key,
                        down: false,
                        repeat: false
                    },
                ],
                "modifier order {first_key:?}, {second_key:?}"
            );
        }
    }
}

/// 7c. Focus and iconify callbacks report the latest native state.
fn focus_and_minimize() {
    let mut rig = Rig::open();
    support::activate();
    let window = rig.window();
    let title = CString::new("glfw focus-stealing test window").expect("static title has no NUL");
    // SAFETY: GLFW is initialized on its required main thread; the second window is a test-only
    // focus target and has no Rust backend handle or shared context.
    let other = unsafe {
        glfwCreateWindow(
            200,
            150,
            title.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert!(
        !other.is_null(),
        "a second GLFW window is created as a focus target"
    );
    let lost = rig
        .pump_until(|report| report.input.contains(&NativeEvent::FocusLost))
        .expect("creating a focused second GLFW window reports focus loss");
    assert!(lost.input.contains(&NativeEvent::FocusLost));
    // SAFETY: the live Rig owns this GLFW window and the test runs on its main thread.
    unsafe { glfwFocusWindow(window) };
    let gained = rig
        .pump_until(|report| report.input.contains(&NativeEvent::FocusGained))
        .expect("showing and focusing reports focus gain");
    assert!(gained.input.contains(&NativeEvent::FocusGained));
    // SAFETY: `other` is the live test window created above and GLFW is still initialized.
    unsafe { glfwDestroyWindow(other) };

    // SAFETY: the live Rig owns this GLFW window and the test runs on its main thread.
    unsafe { glfwIconifyWindow(window) };
    let minimized = rig
        .pump_until(|report| report.minimized)
        .expect("iconify callback reports minimized");
    assert!(minimized.minimized && !minimized.restored);
    // SAFETY: the live Rig owns this GLFW window and the test runs on its main thread.
    unsafe { glfwRestoreWindow(window) };
    let restored = rig
        .pump_until(|report| report.restored)
        .expect("restore callback reports restored");
    assert!(restored.restored && !restored.minimized);
}

/// 7d. A native restore wakes `Backend::pump_until` without a test-posted event or another GLFW call.
fn restore_wakes_the_backend_blocking_wait() {
    let mut rig = Rig::open();
    let window = rig.window();
    let focus_before = support::info().focus;
    assert!(
        !focus_before.key,
        "the test window is not key before restore"
    );

    // Consume the minimization completely so the backend wait starts with no pending state or queued wake.
    // SAFETY: `window` belongs to the live rig and this case runs on GLFW's main thread.
    unsafe { glfwIconifyWindow(window) };
    let minimized = rig
        .pump_until(|report| report.minimized)
        .expect("the minimization callback is consumed before the blocking wait");
    assert!(minimized.minimized && !minimized.restored);
    assert!(
        quiet(&rig.pump(Wait::Poll)),
        "no minimized state remains pending"
    );

    RESTORE_CALLBACK_ACK.store(false, Ordering::SeqCst);
    // SAFETY: `window` is live, and the iconify callback is installed on GLFW's main thread.
    let original =
        unsafe { glfwSetWindowIconifyCallback(window, Some(acknowledge_restore_callback)) }
            .expect("the backend's iconify callback is installed");
    ORIGINAL_ICONIFY_CALLBACK
        .set(original)
        .unwrap_or_else(|_| panic!("the restore observer is installed once per worker process"));

    // The scheduled main-queue block only calls NSWindow deminiaturize:. It does not post an event or call GLFW.
    let started = Instant::now();
    assert!(support::schedule_restore(Duration::from_millis(250)));
    let deadline = started + BOUND;
    let mut events = Vec::new();
    while Instant::now() < deadline && !events.contains(&NativeEvent::Restored) {
        rig.backend
            .pump_until(&mut |event| events.push(event), deadline, 16);
    }
    let elapsed = started.elapsed();
    let callback_acknowledged = RESTORE_CALLBACK_ACK.load(Ordering::SeqCst);
    println!(
        "restore wait observation: callback_acknowledged={callback_acknowledged}, elapsed={elapsed:?}, events={events:?}"
    );
    assert!(
        events.contains(&NativeEvent::Restored),
        "the backend's own blocking wait hands over Restored before its far deadline; elapsed={elapsed:?}, events={events:?}"
    );
    assert!(
        callback_acknowledged,
        "the GLFW iconify callback itself acknowledged the restore"
    );

    let expect_no_wake = env::var_os("CK_GLFW_EXPECT_NO_RESTORE_WAKE").is_some();
    if expect_no_wake {
        assert!(
            elapsed >= RESTORE_WAKE_BOUND,
            "without the callback wake the acknowledged restore must not end the blocking wait within its observation bound: {elapsed:?}"
        );
    } else {
        assert!(
            elapsed < RESTORE_WAKE_BOUND,
            "the restore callback's wake ends the backend wait within the observation bound: {elapsed:?}"
        );
    }

    let focus_after = support::info().focus;
    assert_eq!(
        focus_after.key, focus_before.key,
        "restore does not make the window key"
    );
    assert_eq!(
        focus_after.active, focus_before.active,
        "restore does not activate the application"
    );
}

/// 7d. One absolute deadline bounds repeated real waits and the two spec-required negative controls.
fn deadline_wait_is_bounded() {
    let mut rig = Rig::open();
    rig.pump_for(Duration::from_millis(400));
    let started = Instant::now();
    let deadline = started + Duration::from_millis(300);
    let mut returns = 0;
    while Instant::now() < deadline {
        rig.backend.pump_until(&mut |_| {}, deadline, 16);
        returns += 1;
    }
    let elapsed = started.elapsed();
    assert!(
        !too_many_returns(returns, elapsed),
        "busy polling: {returns} returns in {elapsed:?}"
    );
    assert!(
        elapsed <= Duration::from_millis(500),
        "deadline + 200 ms bound: {elapsed:?}"
    );
    eprintln!("deadline witness: {returns} returns in {elapsed:?}; limit 500 ms");

    // Negative control 1: a polling implementation returns at CPU speed and violates the existing bound.
    let started = Instant::now();
    let mut polls = 0;
    while started.elapsed() < Duration::from_millis(300) {
        // SAFETY: GLFW is initialized by the live Rig, on its required main thread.
        unsafe { glfwPollEvents() };
        polls += 1;
    }
    assert!(
        too_many_returns(polls, started.elapsed()),
        "PollEvents control unexpectedly met the bound: {polls}"
    );
    eprintln!(
        "negative control glfwPollEvents: {polls} returns in {:?}; correctly exceeds bound",
        started.elapsed()
    );

    // Negative control 2: the unbounded wait cannot honor the absolute deadline. A later GLFW wake releases
    // it only after the deadline plus the allowed slack.
    if env::var(TYPING_VARIABLE).as_deref() != Ok("1") {
        let waker: Arc<dyn NativeWake> = rig.backend.wake_target();
        let helper = thread::spawn(move || {
            thread::sleep(Duration::from_millis(600));
            waker.wake();
        });
        let started = Instant::now();
        let deadline = started + Duration::from_millis(300);
        while Instant::now() < deadline {
            // SAFETY: GLFW is initialized by the live Rig, on its required main thread.
            unsafe { glfwWaitEvents() };
        }
        let elapsed = started.elapsed();
        helper.join().expect("negative control event posted");
        assert!(
            elapsed > Duration::from_millis(500),
            "WaitEvents control unexpectedly met deadline: {elapsed:?}"
        );
        eprintln!(
            "negative control glfwWaitEvents: returned after {elapsed:?}, past deadline + 200 ms"
        );
    }
}

/// 8. Both waits, with pending state and wake races.
fn wait_modes() {
    let mut rig = Rig::open();
    support::trace_start();
    // Poll never waits.
    let started = Instant::now();
    assert!(quiet(&rig.pump(Wait::Poll)));
    assert!(started.elapsed() < Duration::from_millis(500));
    let target: Arc<dyn NativeWake> = rig.backend.wake_target();
    // A wake before a blocking pump is sticky: the pump returns at once with nothing.
    target.wake();
    let started = Instant::now();
    assert!(quiet(&rig.pump(Wait::UntilEvent)));
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    assert!(rig.backend.wake_counts().posted >= 1);
    // State already pending (recorded by the delegate outside the pump): a blocking pump does not block.
    support::perform_close();
    let started = Instant::now();
    let report = rig.pump(Wait::UntilEvent);
    assert_eq!(report.close, Some(NativeEvent::CloseButton));
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    // A blocking pump is ended by a wake from another thread. (The owner's typing, dropped by the filter,
    // still ends a native wait early: the pump is then called again, as the loop does.)
    let before = rig.backend.wake_counts().posted;
    let waker = Arc::clone(&target);
    let helper = thread::spawn(move || {
        thread::sleep(Duration::from_millis(250));
        waker.wake();
    });
    let started = Instant::now();
    let give_up = started + BOUND;
    while rig.backend.wake_counts().posted == before {
        assert!(Instant::now() < give_up, "the wake never arrived");
        assert!(
            quiet(&rig.pump(Wait::UntilEvent)),
            "nothing else was asked for"
        );
    }
    // The wake ended the wait (it was consumed by the pump that returned); the helper has finished.
    helper.join().expect("the waker ended");
    assert!(
        started.elapsed() >= Duration::from_millis(150),
        "the wake came at 250 ms"
    );
    if env::var(TYPING_VARIABLE).as_deref() != Ok("1") {
        // On a quiet keyboard the wait ends at the wake, give or take system events: the pump keeps blocking
        // while its returns are empty and the wake has not arrived (see `wait_for_the_wake`).
        support::trace_mark("the quiet wait begins");
        let wait = wait_for_the_wake(&mut rig, &target, Duration::from_millis(250), None)
            .unwrap_or_else(|why| panic!("{why}\n{}", support::trace_take()));
        assert!(
            wait.waited >= Duration::from_millis(150),
            "the wake was observed too early: {:?} after {} empty returns\n{}",
            wait.waited,
            wait.early_returns,
            support::trace_take()
        );
        assert!(wait.waited < BOUND, "{:?}", wait.waited);
    }
    // ... and by a window-system event from another thread.
    let helper = thread::spawn(|| {
        thread::sleep(Duration::from_millis(250));
        marked_escape_tap();
    });
    let started = Instant::now();
    let give_up = started + BOUND;
    let report = loop {
        assert!(Instant::now() < give_up, "the Escape never arrived");
        let report = rig.pump(Wait::UntilEvent);
        if !report.input.is_empty() {
            break report;
        }
    };
    helper.join().expect("the poster ended");
    assert_eq!(
        report.input[0],
        NativeEvent::Key {
            key: ck_input::Key::Escape,
            down: true,
            repeat: false
        }
    );
    assert!(started.elapsed() >= Duration::from_millis(150));
}

/// 8a. A wake posted before `pump_until` is sticky and ends the far-deadline wait immediately.
fn pump_until_honors_a_preposted_wake() {
    let mut rig = Rig::open();
    let target = rig.backend.wake_target();
    target.wake();
    let mut events = Vec::new();
    let started = Instant::now();
    rig.backend.pump_until(
        &mut |event| events.push(event),
        started + Duration::from_secs(5),
        16,
    );
    assert!(
        events.is_empty(),
        "a wake alone has no native event: {events:?}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(1000),
        "the preposted wake ends pump_until well before its deadline: {:?}",
        started.elapsed()
    );
}

/// 8b. A callback's already-recorded state makes `pump_until` drain without waiting for the wake.
fn pump_until_hands_over_previously_recorded_state() {
    let mut rig = Rig::open();
    support::perform_close();
    let mut events = Vec::new();
    let started = Instant::now();
    rig.backend.pump_until(
        &mut |event| events.push(event),
        started + Duration::from_secs(5),
        16,
    );
    assert_eq!(events, [NativeEvent::CloseButton]);
    assert!(
        started.elapsed() < Duration::from_millis(1000),
        "pending state ends pump_until well before its deadline: {:?}",
        started.elapsed()
    );
}

/// 9. Wakes through the notifier: counted while armed, none after disarm; a retained target after
///    teardown is a counted no-op.
fn wake_lifetime() {
    let mut rig = Rig::open();
    let notifier = Notifier::new();
    let target = rig.backend.wake_target();
    notifier
        .arm(Arc::clone(&target))
        .expect("the notifier arms");
    notifier.wake();
    notifier.wake();
    let armed = rig.backend.wake_counts();
    assert_eq!(armed.posted, 2);
    assert_eq!(armed.ignored, 0);
    notifier
        .disarm(Duration::from_secs(2))
        .expect("no wake is in flight");
    notifier.wake();
    assert_eq!(
        rig.backend.wake_counts(),
        armed,
        "a wake after disarm reaches nothing"
    );
    // The loop's teardown: the window, then the library. The test holds `target` past it, which is outside
    // the contract, and the wake is a counted no-op that reaches GLFW not at all.
    let posted = rig.backend.wake_counts().posted;
    rig.backend.destroy_window();
    rig.backend.terminate();
    target.wake();
    let after = rig.backend.wake_counts();
    assert_eq!((after.posted, after.ignored), (posted, 1), "{after:?}");
}

/// 10. The pixel size, and a forced content-scale change.
fn pixel_size_and_scale() {
    let mut rig = Rig::open();
    let native = support::info().window_scale;
    assert_eq!(
        rig.pixel_size.width,
        pixels(WIDTH, native),
        "the initial pixel size"
    );
    let forced = if native > 1.5 { 1.0 } else { 2.0 };
    support::set_scale(forced);
    support::backing_changed();
    let report = rig
        .pump_until(|report| report.pixel_size.is_some())
        .expect("a pixel size change is reported");
    let size = report.pixel_size.expect("a pixel size");
    assert_eq!(size.width, pixels(WIDTH, forced), "{size:?}");
    assert_eq!(size.height, pixels(HEIGHT, forced), "{size:?}");
    assert!(
        rig.pump_for(Duration::from_millis(150))
            .iter()
            .all(|r| r.pixel_size.is_none())
    );
    support::set_scale(0.0);
}

/// 11. No menu bar: no key equivalent anywhere, and Cmd-Q changes nothing.
fn no_menu_bar_and_cmd_q() {
    let mut rig = Rig::open();
    let info = support::info();
    assert_eq!(
        info.menu_key_equivalents, 0,
        "no menu item has a key equivalent"
    );
    assert!(
        support::post_key(support::KEY_Q, support::COMMAND, false, "q", true),
        "Cmd-Q is posted"
    );
    let seen = rig.pump_for(Duration::from_millis(300));
    assert!(
        Rig::close_requests(&seen).is_empty(),
        "Cmd-Q asks for nothing: {seen:?}"
    );
    let after = support::info();
    assert!(
        after.has_window && after.visible,
        "the process and its window are alive"
    );
}

/// 12. The instance extensions GLFW requires, copied and deduplicated.
fn required_extensions() {
    let rig = Rig::open();
    let names = rig.handles().required_instance_extensions();
    assert!(
        names.iter().any(|name| name == "VK_KHR_surface"),
        "{names:?}"
    );
    assert!(names.len() >= 2, "{names:?}");
    let mut unique = names.to_vec();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "no duplicates: {names:?}");
}

/// 13. A surface from a real `VkInstance` created with only the required extensions and portability
///     enumeration, destroyed before the instance and the window.
fn surface_creation() {
    let rig = Rig::open();
    let loader = support::loader();
    let mut names: Vec<CString> = rig
        .handles()
        .required_instance_extensions()
        .iter()
        .map(|name| CString::new(name.as_str()).expect("no NUL"))
        .collect();
    names.push(CString::new("VK_KHR_portability_enumeration").expect("no NUL"));
    let pointers: Vec<_> = names.iter().map(|name| name.as_ptr()).collect();
    let info = support::InstanceInfo {
        s_type: support::INSTANCE_CREATE_INFO,
        next: std::ptr::null(),
        flags: support::ENUMERATE_PORTABILITY,
        application_info: std::ptr::null(),
        layer_count: 0,
        layers: std::ptr::null(),
        extension_count: u32::try_from(pointers.len()).expect("a few names"),
        extensions: pointers.as_ptr(),
    };
    let mut instance: *mut c_void = std::ptr::null_mut();
    // SAFETY: `info` and the extension names it points to live until the call returns, and `instance` is
    // valid for the call.
    let result =
        unsafe { (loader.create_instance)(&raw const info, std::ptr::null(), &raw mut instance) };
    assert_eq!(result, 0, "vkCreateInstance returned {result}");
    // SAFETY: the instance was created with the extensions GLFW requires and is alive; the window is
    // alive; this is the main thread.
    let surface = unsafe { rig.handles().create_surface(instance) };
    let surface = surface.expect("the surface is created");
    assert_ne!(surface, 0);
    // The GPU side's order: the surface, the instance, then (the rig's drop) the window.
    // SAFETY: the surface belongs to the instance and is destroyed once, before it.
    unsafe { (loader.destroy_surface)(instance, surface, std::ptr::null()) };
    // SAFETY: the instance is destroyed once, after its surface.
    unsafe { (loader.destroy_instance)(instance, std::ptr::null()) };
}

/// 14. The owner's typing is dropped: unmarked keyboard events, Escape among them, change nothing.
fn typing_is_dropped() {
    let mut rig = Rig::open();
    let before = support::info().filtered_keys;
    for _ in 0..5 {
        assert!(support::post_key(
            support::ESCAPE,
            0,
            false,
            "\u{1b}",
            false
        ));
        assert!(support::post_keyboard_event(1, support::ESCAPE, 0, false));
        assert!(support::post_key(
            support::KEY_Q,
            support::COMMAND,
            false,
            "q",
            false
        ));
    }
    let seen = rig.pump_for(Duration::from_millis(300));
    assert!(
        Rig::close_requests(&seen).is_empty(),
        "the owner's typing asked for nothing: {seen:?}"
    );
    assert!(
        seen.iter().all(|report| key_events(report).is_empty()),
        "the owner's typing delivered no Key or KeyResync: {seen:?}"
    );
    assert!(
        support::info().filtered_keys >= before + 10,
        "the filter dropped the unmarked events"
    );
    // The tests' own events still get through.
    marked_escape_tap();
    let report = rig
        .pump_until(|report| !report.input.is_empty())
        .expect("the marked Escape is delivered");
    assert_eq!(
        report.input[0],
        NativeEvent::Key {
            key: ck_input::Key::Escape,
            down: true,
            repeat: false
        }
    );
}

/// 15. Given no loader, GLFW searches by the loader's leaf name, and a failure is reported by name: found
///     here exactly when `dlopen` finds the loader by that name.
fn no_loader_given_reports_a_failure_by_name() {
    let mut backend = GlfwBackend::new(None).expect("GLFW starts");
    let created = backend.create_window(&desc());
    if support::leaf_name_is_found() {
        created.expect("the loader is found by its leaf name");
        backend.destroy_window();
    } else {
        let message = created.expect_err("GLFW cannot find the loader by its leaf name");
        assert!(message.contains("Vulkan loader"), "{message}");
        assert!(message.contains("vkGetInstanceProcAddr"), "{message}");
        assert!(!support::info().has_window, "no window is left behind");
    }
    // A backend given the loader afterwards finds it (the earlier choice is not sticky).
    backend.terminate();
    drop(backend);
    let mut with = new_backend();
    with.create_window(&desc())
        .expect("a window with the loader given");
}

/// 16. The loader's keep-alive is held until after termination, and GLFW is given NULL before it goes: the
///     pointer GLFW kept is never used by a later backend, a refused terminate keeps the keep-alive, and a
///     backend dropped without an explicit teardown releases it after the reset.
fn loader_lifetime() {
    let (mut backend, released) = new_backend_watched();
    backend.create_window(&desc()).expect("a window");
    assert!(
        support::proc_addr_calls() > 0,
        "GLFW used the loader function it was given"
    );
    assert!(
        !released.load(Ordering::SeqCst),
        "held while the window lives"
    );
    backend.terminate(); // refused: the window still exists
    assert!(
        !released.load(Ordering::SeqCst),
        "a refused terminate keeps the keep-alive"
    );
    backend.destroy_window();
    assert!(
        !released.load(Ordering::SeqCst),
        "held until glfwTerminate has run"
    );
    backend.terminate();
    assert!(
        released.load(Ordering::SeqCst),
        "released after glfwTerminate and the NULL reset"
    );
    // GLFW was told NULL before the release: a later backend without a loader never reaches the old function.
    let calls = support::proc_addr_calls();
    let mut none = GlfwBackend::new(None).expect("a backend with no loader");
    none.create_window(&desc())
        .expect("the loader is found by name once its image is loaded");
    assert_eq!(
        support::proc_addr_calls(),
        calls,
        "the earlier pointer is gone"
    );
    none.destroy_window();
    none.terminate();
    drop(none);
    // A replaced loader: the next backend's function is used, and the first keep-alive stays released.
    let before = support::proc_addr_calls();
    let (mut next, next_released) = new_backend_watched();
    next.create_window(&desc()).expect("a window");
    assert!(
        support::proc_addr_calls() > before,
        "the replacement is used"
    );
    assert!(released.load(Ordering::SeqCst) && !next_released.load(Ordering::SeqCst));
    // Dropped without an explicit teardown: the window, the library, then the keep-alive.
    drop(next);
    assert!(
        next_released.load(Ordering::SeqCst),
        "released when the backend is dropped"
    );
}

/// 17. The backend sets the regular activation policy itself (GLFW does that only with its menu bar, which
///     the backend leaves out), once: a second backend in the process finds it in place and asks `AppKit` for
///     nothing. The policy is read through the test support, not the library.
fn activation_policy_is_regular() {
    support::watch_policy(false);
    assert_eq!(
        support::info().activation_policy,
        -1,
        "no application object exists before GLFW starts"
    );
    let first = new_backend();
    assert_eq!(
        support::info().activation_policy,
        support::POLICY_REGULAR,
        "the first backend leaves the process a regular application"
    );
    let calls = support::policy_calls();
    assert_eq!(calls, 1, "the policy was set once");
    drop(first);
    let second = new_backend();
    assert_eq!(support::info().activation_policy, support::POLICY_REGULAR);
    assert_eq!(
        support::policy_calls(),
        calls,
        "a policy already in place is left alone"
    );
    assert_eq!(support::info().menu_key_equivalents, 0, "still no menu bar");
    drop(second);
}

/// 18. A refused policy is an error that leaves the process as the other post-init failures do: GLFW
///     terminated, the loader reset and released, the backend slot free for the next one.
fn a_refused_policy_is_an_error() {
    support::watch_policy(true);
    let (loader, released) = support::loader_for_glfw();
    let message = GlfwBackend::new(Some(loader)).expect_err("a refused policy fails `new`");
    assert_eq!(message, "the regular activation policy could not be set");
    assert_eq!(support::policy_calls(), 1, "the set was tried once");
    assert!(
        released.load(Ordering::SeqCst),
        "the loader was reset and released"
    );
    assert!(!support::glfw_is_initialised(), "GLFW was terminated");
    assert_eq!(support::info().window_count, 0);
    // The refusal over, a backend can follow and gets its window.
    support::watch_policy(false);
    let mut backend = new_backend();
    assert!(support::glfw_is_initialised());
    assert_eq!(support::info().activation_policy, support::POLICY_REGULAR);
    assert_eq!(support::policy_calls(), 2);
    backend
        .create_window(&desc())
        .expect("a window after the refusal");
}

/// 19. The window comes to the front and takes keys by itself: the application becomes active and the
///     window key, with `ck_glfw_test_activate` never called. This is what the missing policy broke.
fn the_window_becomes_active_by_itself() {
    // On macOS 14 and later the backend's own request is `activate`, which GLFW never calls: it is made once
    // by `create_window` and not before.
    let watching = support::watch_activate();
    let mut backend = new_backend();
    assert_eq!(
        support::activate_calls(),
        0,
        "nothing asked before the window"
    );
    backend
        .create_window(&desc())
        .expect("the window is created");
    if watching {
        assert_eq!(
            support::activate_calls(),
            1,
            "the backend asked for activation when it created the window"
        );
    }
    let give_up = Instant::now() + Duration::from_secs(5);
    loop {
        backend.pump_with_report(Wait::Poll, 16);
        let info = support::info();
        if info.visible && info.focus.active && info.focus.key {
            break;
        }
        assert!(
            Instant::now() < give_up,
            "the application did not become active with a key window within 5 s: {info:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }
    // Keys reach it: a marked Escape is delivered without any activation by the test.
    marked_escape_tap();
    let give_up = Instant::now() + BOUND;
    loop {
        let report = backend.pump_with_report(Wait::Poll, 16);
        if report.input.iter().any(|event| {
            matches!(
                event,
                NativeEvent::Key {
                    key: ck_input::Key::Escape,
                    down: true,
                    ..
                }
            )
        }) {
            break;
        }
        assert!(Instant::now() < give_up, "the Escape never arrived");
        thread::sleep(Duration::from_millis(2));
    }
    backend.destroy_window();
    backend.terminate();
}

/// 20. The Rust binding of the native info structure has the layout the native side was compiled with (size,
///     alignment and every field's offset), checked before any call that writes through it. Opens no window.
fn the_info_binding_matches_the_native_layout() {
    if let Err(message) = support::check_layout() {
        panic!("{message}");
    }
}

/// 21. A system event ends a blocking wait early, and the bounded wait takes it in its stride: a mouse-moved event
///     (what the window system sends as a window is ordered or activated under the pointer) is posted 100 ms
///     into a wait for a wake due at 250 ms. The old assertion (one blocking pump lasting at least 150 ms)
///     fails; the new one passes with the early return counted, also for a pointer moving all the while
///     (an event every 8 ms); a pump that returns empty at CPU speed is a busy loop and fails the bound.
fn wait_survives_a_system_event() {
    let mut rig = Rig::open();
    let target = rig.backend.wake_target();
    // Let the opening of the window settle, so nothing else is in flight.
    rig.pump_for(Duration::from_millis(700));
    let number = support::window_number();
    let disturbance = || -> Option<(Duration, Box<dyn FnOnce() + Send>)> {
        Some((
            Duration::from_millis(100),
            Box::new(move || support::post_mouse_moved(number)),
        ))
    };
    // The old assertion: one pump, at least 150 ms. The posted event ends it long before.
    let one = one_blocking_pump(&mut rig, &target, Duration::from_millis(250), disturbance());
    assert!(
        one < Duration::from_millis(150),
        "the old single-pump wait ended at the posted event, not at the wake: {one:?}"
    );
    rig.pump_for(Duration::from_millis(100));
    // The new one: the same disturbance, the wake still observed after the total delay, the early return counted.
    let wait = wait_for_the_wake(&mut rig, &target, Duration::from_millis(250), disturbance())
        .expect("the bounded wait passes");
    assert!(
        wait.waited >= Duration::from_millis(150),
        "{:?}",
        wait.waited
    );
    assert!(
        wait.early_returns >= 1,
        "the posted event ended one pump: {}",
        wait.early_returns
    );
    assert!(
        !too_many_returns(wait.early_returns, wait.waited),
        "{}",
        wait.early_returns
    );
    rig.pump_for(Duration::from_millis(100));
    // A pointer moving over the window (an event every 8 ms, 120 a second, for the whole wait) is not a busy loop.
    let pointer: Box<dyn FnOnce() + Send> = Box::new(move || {
        let until = Instant::now() + Duration::from_millis(240);
        while Instant::now() < until {
            support::post_mouse_moved(number);
            thread::sleep(Duration::from_millis(8));
        }
    });
    let moving = wait_for_the_wake(
        &mut rig,
        &target,
        Duration::from_millis(250),
        Some((Duration::ZERO, pointer)),
    )
    .expect("a moving pointer is not a busy loop");
    assert!(
        moving.waited >= Duration::from_millis(150),
        "{:?}",
        moving.waited
    );
    assert!(
        moving.early_returns >= 10,
        "the pointer events ended pumps: {}",
        moving.early_returns
    );
    rig.pump_for(Duration::from_millis(100));
    // A pump that returns empty at CPU speed (a busy loop) fails the bound, however long it lasts. (Events cannot
    // make one: AppKit coalesces pointer events to the display's rate, about 120 a second, which the bound allows.)
    let began = Instant::now();
    let busy = bounded_wait(|| {
        (
            PumpReport::default(),
            began.elapsed() > Duration::from_millis(300),
        )
    })
    .expect_err("a pump that returns at once, again and again, is a busy loop");
    assert!(busy.contains("busy loop"), "{busy}");
    // The bound itself: slack and a pointer's rate pass, a CPU-speed loop does not.
    assert!(!too_many_returns(3, Duration::from_millis(250)));
    assert!(!too_many_returns(30, Duration::from_millis(250)));
    assert!(!too_many_returns(120, Duration::from_secs(1)));
    assert!(too_many_returns(17, Duration::ZERO));
    assert!(too_many_returns(2_000, Duration::from_millis(10)));
}

type Case = fn();

const CASES: [(&str, Case); 28] = [
    ("create_and_destroy", create_and_destroy),
    ("off_the_main_thread", off_the_main_thread),
    (
        "one_backend_at_a_time_and_terminate_refuses_with_a_window",
        one_backend_at_a_time_and_terminate_refuses_with_a_window,
    ),
    ("close_button", close_button),
    ("escape", escape),
    ("presses_coalesce", presses_coalesce),
    (
        "the_pump_drains_everything_and_hands_over_one",
        the_pump_drains_everything_and_hands_over_one,
    ),
    ("normalized_keys", normalized_keys),
    ("modifier_sides_and_order", modifier_sides_and_order),
    ("focus_and_minimize", focus_and_minimize),
    (
        "restore_wakes_the_backend_blocking_wait",
        restore_wakes_the_backend_blocking_wait,
    ),
    ("deadline_wait_is_bounded", deadline_wait_is_bounded),
    ("wait_modes", wait_modes),
    (
        "pump_until_honors_a_preposted_wake",
        pump_until_honors_a_preposted_wake,
    ),
    (
        "pump_until_hands_over_previously_recorded_state",
        pump_until_hands_over_previously_recorded_state,
    ),
    ("wake_lifetime", wake_lifetime),
    ("pixel_size_and_scale", pixel_size_and_scale),
    ("no_menu_bar_and_cmd_q", no_menu_bar_and_cmd_q),
    ("required_extensions", required_extensions),
    ("surface_creation", surface_creation),
    ("typing_is_dropped", typing_is_dropped),
    (
        "no_loader_given_reports_a_failure_by_name",
        no_loader_given_reports_a_failure_by_name,
    ),
    ("loader_lifetime", loader_lifetime),
    ("activation_policy_is_regular", activation_policy_is_regular),
    ("a_refused_policy_is_an_error", a_refused_policy_is_an_error),
    (
        "the_window_becomes_active_by_itself",
        the_window_becomes_active_by_itself,
    ),
    (
        "the_info_binding_matches_the_native_layout",
        the_info_binding_matches_the_native_layout,
    ),
    ("wait_survives_a_system_event", wait_survives_a_system_event),
];

/// A worker that never ends, for the supervisor's own check.
fn hang() {
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

fn worker(name: &str) -> ExitCode {
    let case = CASES
        .iter()
        .find(|(case, _)| *case == name)
        .map(|(_, case)| *case)
        .or_else(|| (name == "hang").then_some(hang as Case));
    let Some(case) = case else {
        eprintln!("worker: no case called {name}");
        return ExitCode::from(1);
    };
    if !FOCUS_CASES.contains(&name) {
        ck_window::test_support::open_windows_without_activation();
        activation_watch::install(false);
    }
    // The filter is in before the first window and stays for the whole life of the worker. This
    // policy case specifically verifies the app does not exist before GLFW starts, and posts no keys.
    support::install_input_filter();
    if env::var(TYPING_VARIABLE).as_deref() == Ok("1") {
        support::start_typist();
    }
    match catch_unwind(AssertUnwindSafe(case)) {
        Ok(()) => {
            if !FOCUS_CASES.contains(&name) {
                activation_watch::assert_clean();
            }
            ExitCode::SUCCESS
        }
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

/// Runs `case` in a worker under the supervisor; `Ok` when it exited 0.
fn run_case(case: &str, typing: bool, deadline: Duration) -> Result<(), String> {
    let mut command = Command::new(env::current_exe().map_err(|error| error.to_string())?);
    command.arg(WORKER_FLAG).arg(case);
    if typing {
        command.env(TYPING_VARIABLE, "1");
    } else {
        command.env_remove(TYPING_VARIABLE);
    }
    let supervised = Supervised::spawn(command, deadline)?;
    let seen = supervised.finish()?;
    if seen.code == Some(0) {
        Ok(())
    } else {
        Err(format!(
            "{case} ended with {:?}\n{}",
            seen.code, seen.stdout
        ))
    }
}

/// A worker that hangs is killed and reaped at its deadline, and the case fails with that said.
fn check_a_hung_worker_is_killed() -> Result<(), String> {
    let started = Instant::now();
    match run_case("hang", false, Duration::from_millis(500)) {
        Err(message) if message.contains("deadline") || message.contains("did not finish") => {
            if started.elapsed() > Duration::from_secs(20) {
                return Err(format!(
                    "it took {:?} to kill the worker",
                    started.elapsed()
                ));
            }
            Ok(())
        }
        other => Err(format!(
            "a hung worker must fail at its deadline, got {other:?}"
        )),
    }
}

/// The library's rlib holds none of the test support, and does hold the backend and its own native
/// archive (so the check looks at the right archive).
fn check_test_support_is_not_in_the_library() -> Result<(), String> {
    let deps: PathBuf = env::current_exe()
        .map_err(|error| error.to_string())?
        .parent()
        .ok_or("the test executable has no directory")?
        .to_path_buf();
    let mut rlibs: Vec<PathBuf> = std::fs::read_dir(&deps)
        .map_err(|error| format!("{}: {error}", deps.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "rlib")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("libck_window_glfw-"))
        })
        .collect();
    rlibs.sort();
    let rlib = rlibs
        .pop()
        .ok_or("the library's rlib was not found next to the test")?;
    let mut command = Command::new("/usr/bin/nm");
    command.arg("-g").arg(&rlib);
    let seen = Supervised::spawn(command, Duration::from_secs(30))?.finish()?;
    if seen.code != Some(0) {
        return Err(format!("nm failed ({:?}): {}", seen.code, seen.stdout));
    }
    if !seen.stdout.contains("GlfwBackend") {
        return Err("the library's rlib does not hold the backend".to_owned());
    }
    // The library's own native archive (the activation calls) is in the rlib, defined, so the scan below
    // covers it as well.
    for defined in [
        "T _ck_glfw_ensure_regular_policy",
        "T _ck_glfw_activate_window",
    ] {
        if !seen.stdout.contains(defined) {
            return Err(format!(
                "the library's rlib does not define its native {defined:?}"
            ));
        }
    }
    for forbidden in ["ck_glfw_test_", "CKGlfwTest"] {
        if seen.stdout.contains(forbidden) {
            return Err(format!(
                "the library's rlib contains test support: {forbidden}"
            ));
        }
    }
    Ok(())
}

type SupervisorCheck = fn() -> Result<(), String>;

fn supervisor() -> ExitCode {
    let mut failures = BTreeMap::new();
    let checks: [(&str, SupervisorCheck); 4] = [
        (
            "the layout check rejects mismatched info bindings",
            support::check_the_comparison_rejects_mismatched_bindings,
        ),
        (
            "nothing else binds the native info structure",
            support::check_nothing_else_binds_the_native_info,
        ),
        (
            "a hung worker is killed and reaped",
            check_a_hung_worker_is_killed,
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
            let result = run_case(name, typing, DEADLINE);
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
        _ => {
            assert!(
                FOCUS_CASES
                    .iter()
                    .all(|name| CASES.iter().any(|(case, _)| case == name)),
                "every named focus case must exist"
            );
            supervisor()
        }
    }
}
