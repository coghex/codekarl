//! The clear-only backend over a recording fake presenter.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ck_window::{CleanupResult, PixelSize, Presenter, PresenterReport, WindowDesc};

use super::idle::{BOUND, IdleLoop};
use super::logging::{Scenario, hold_sink_and_fill_the_queue, scenario};
use super::support::{LiveImages, desc, image, lock, lock_held_by, pixels, quad_at};
use crate::{
    ClearOnly, Filter, Gpu2d, Metrics, PixelRect, PlacedQuad, Renderer, SceneBridge, Snapshot,
    SourceRect,
};

/// A wake that counts its calls.
fn counting_wake() -> (Arc<dyn Fn() + Send + Sync>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    (
        Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }),
        calls,
    )
}

/// A presenter that records each call, and answers from a script.
struct Fake {
    calls: Arc<Mutex<Vec<String>>>,
    reports: VecDeque<PresenterReport>,
    end: Option<CleanupResult>,
    start_fails: Option<String>,
}

impl Fake {
    fn new() -> (Self, Arc<Mutex<Vec<String>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                calls: Arc::clone(&calls),
                reports: VecDeque::new(),
                end: None,
                start_fails: None,
            },
            calls,
        )
    }

    fn note(&self, call: String) {
        lock(&self.calls).push(call);
    }
}

impl Presenter for Fake {
    type Handles = u32;

    fn start(
        &mut self,
        handles: u32,
        desc: &WindowDesc,
        pixel_size: PixelSize,
    ) -> Result<(), String> {
        self.note(format!(
            "start {handles} {} {}x{} {:?} {}x{}",
            desc.title, desc.width, desc.height, desc.clear, pixel_size.width, pixel_size.height
        ));
        self.start_fails.clone().map_or(Ok(()), Err)
    }

    fn set_clear(&mut self, rgb: [f32; 3]) {
        self.note(format!("set_clear {rgb:?}"));
    }

    fn set_pixel_size(&mut self, width: u32, height: u32) {
        self.note(format!("set_pixel_size {width}x{height}"));
    }

    fn request_redraw(&mut self) {
        self.note("request_redraw".to_owned());
    }

    fn take_report(&mut self) -> Option<PresenterReport> {
        self.note("take_report".to_owned());
        self.reports.pop_front()
    }

    fn request_stop(&mut self) {
        self.note("request_stop".to_owned());
    }

    fn poll_end(&mut self, within: Duration) -> Option<CleanupResult> {
        self.note(format!("poll_end {within:?}"));
        self.end.clone()
    }
}

fn snapshot(generation: u64, with_quad: bool) -> Snapshot {
    Snapshot {
        generation,
        filter: Filter::Nearest,
        requested: with_quad,
        quad: with_quad.then(|| PlacedQuad {
            image: image(8, 8),
            destination: PixelRect {
                left: 0,
                top: 0,
                right: 8,
                bottom: 8,
            },
            source: SourceRect {
                left: 0.0,
                top: 0.0,
                right: 8.0,
                bottom: 8.0,
            },
        }),
        scene: None,
        metrics: Metrics {
            points_width: 8,
            points_height: 8,
            pixels: pixels(8, 8),
        },
    }
}

/// The records of the render category, from a scenario.
fn render_records(records: &[String]) -> Vec<&String> {
    records
        .iter()
        .filter(|line| line.contains("render:"))
        .collect()
}

const WARNING: &str = "render: textured quads on glfw-vulkan arrive in milestone 4.5";

#[test]
fn every_presenter_call_is_forwarded_unchanged() {
    let _scenario = scenario();
    let (mut fake, calls) = Fake::new();
    fake.reports.extend([
        PresenterReport::Failed {
            message: "lost".to_owned(),
        },
        PresenterReport::FirstFrameQueued,
    ]);
    fake.end = Some(CleanupResult::Unproven {
        reason: "a token".to_owned(),
    });
    let mut backend = ClearOnly::new(fake, "glfw-vulkan", counting_wake().0);
    let sink = backend.new_sink();
    backend
        .start(7, &desc(100, 80), pixels(200, 160), sink)
        .unwrap();
    backend.set_clear([0.25, 0.5, 0.75]);
    backend.set_pixel_size(300, 240);
    backend.request_redraw();
    assert_eq!(
        backend.take_report(),
        Some(PresenterReport::Failed {
            message: "lost".to_owned()
        })
    );
    assert_eq!(
        backend.take_report(),
        Some(PresenterReport::FirstFrameQueued)
    );
    assert_eq!(backend.take_report(), None);
    backend.request_stop();
    assert_eq!(
        backend.poll_end(Duration::from_millis(7)),
        Some(CleanupResult::Unproven {
            reason: "a token".to_owned()
        })
    );
    assert_eq!(
        *lock(&calls),
        vec![
            "start 7 test 100x80 [0.0, 0.0, 0.0] 200x160".to_owned(),
            "set_clear [0.25, 0.5, 0.75]".to_owned(),
            "set_pixel_size 300x240".to_owned(),
            "request_redraw".to_owned(),
            "take_report".to_owned(),
            "take_report".to_owned(),
            "take_report".to_owned(),
            "request_stop".to_owned(),
            "poll_end 7ms".to_owned(),
        ]
    );
    assert_eq!(backend.drawn_generation(), None);
    assert_eq!(
        backend.presenter().reports.len(),
        0,
        "the wrapped presenter is reachable"
    );
}

#[test]
fn a_start_failure_comes_through_as_the_presenters_own() {
    let _scenario = scenario();
    let (mut fake, _calls) = Fake::new();
    fake.start_fails = Some("no device".to_owned());
    let mut backend = ClearOnly::new(fake, "glfw-vulkan", counting_wake().0);
    let sink = backend.new_sink();
    assert_eq!(
        backend.start(1, &desc(10, 10), pixels(10, 10), sink),
        Err("no device".to_owned())
    );
}

#[test]
fn a_run_that_sets_a_quad_logs_one_warning_naming_the_path_and_no_more() {
    let scenario = scenario();
    let (fake, calls) = Fake::new();
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    let mut renderer = Renderer::new(
        ClearOnly::new(fake, "glfw-vulkan", counting_wake().0),
        Arc::clone(&bridge),
    );
    let picture = image(10, 10);
    writer.set_quad(quad_at(&picture, 0.0, 0.0, 10.0, 10.0)); // before the window
    renderer
        .start(7, &desc(100, 100), pixels(100, 100))
        .unwrap();
    assert_eq!(render_records(&scenario.records()).len(), 1, "one at start");
    writer.set_quad(quad_at(&picture, 5.0, 5.0, 10.0, 10.0));
    writer.clear_quad();
    renderer.set_pixel_size(200, 200);
    renderer.request_redraw();
    renderer.request_stop();
    let records = scenario.records();
    let found = render_records(&records);
    assert_eq!(found.len(), 1, "{records:?}");
    assert!(found[0].contains("WARN"), "{found:?}");
    assert!(found[0].ends_with(WARNING), "{found:?}");
    assert!(lock(&calls).iter().any(|call| call.starts_with("start 7")));
}

#[test]
fn a_run_that_never_sets_a_quad_logs_nothing() {
    let scenario = scenario();
    let (fake, _calls) = Fake::new();
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    let mut renderer = Renderer::new(
        ClearOnly::new(fake, "glfw-vulkan", counting_wake().0),
        Arc::clone(&bridge),
    );
    renderer
        .start(7, &desc(100, 100), pixels(100, 100))
        .unwrap();
    writer.clear_quad();
    renderer.set_pixel_size(200, 200);
    renderer.request_stop();
    assert_eq!(render_records(&scenario.records()), Vec::<&String>::new());
}

#[test]
fn nothing_is_logged_inside_publish_and_the_next_main_thread_call_logs_it() {
    let scenario = scenario();
    let (fake, _calls) = Fake::new();
    let mut backend = ClearOnly::new(fake, "glfw-vulkan", counting_wake().0);
    let sink = backend.new_sink();
    // `publish` runs under the bridge's lock: it must only note the quad.
    let handed_back = sink.publish(snapshot(5, true));
    assert_eq!(
        handed_back.map(|snapshot| snapshot.generation),
        Some(5),
        "the snapshot it did not keep is returned for the caller to drop"
    );
    assert!(sink.publish(snapshot(6, false)).is_some());
    assert_eq!(
        render_records(&scenario.records()).len(),
        0,
        "nothing in publish"
    );
    let _ = backend.take_report();
    let _ = backend.take_report();
    assert_eq!(
        render_records(&scenario.records()).len(),
        1,
        "once, on the main thread"
    );
}

#[test]
fn the_backend_name_is_the_constructors_and_each_backend_warns_once() {
    let scenario = scenario();
    for name in ["glfw-vulkan", "other-path"] {
        let (fake, _calls) = Fake::new();
        let mut backend = ClearOnly::new(fake, name, counting_wake().0);
        let sink = backend.new_sink();
        let _ = sink.publish(snapshot(1, true));
        backend.request_redraw();
        backend.request_redraw();
    }
    let records = scenario.records();
    let found = render_records(&records);
    assert_eq!(found.len(), 2, "{records:?}");
    assert!(found[0].ends_with("textured quads on glfw-vulkan arrive in milestone 4.5"));
    assert!(found[1].ends_with("textured quads on other-path arrive in milestone 4.5"));
}

#[test]
fn the_sink_hands_every_snapshot_back_so_the_bridge_drops_the_images_outside_its_lock() {
    let _scenario = scenario();
    let (fake, _calls) = Fake::new();
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    let mut renderer = Renderer::new(
        ClearOnly::new(fake, "glfw-vulkan", counting_wake().0),
        Arc::clone(&bridge),
    );
    renderer
        .start(7, &desc(100, 100), pixels(100, 100))
        .unwrap();
    let (live, held) = (LiveImages::default(), lock_held_by(&bridge));
    for step in 0..5 {
        let picture = live.make(10, 10, held.clone());
        writer.set_quad(quad_at(&picture, f64::from(step), 0.0, 10.0, 10.0));
        drop(picture);
        assert_eq!(
            live.live(),
            1,
            "write {step}: only the scene's image remains"
        );
    }
    writer.clear_quad();
    assert_eq!(live.live(), 0);
    let probes = live.probes();
    assert_eq!(probes.len(), 5);
    assert!(
        probes.iter().all(|held| !held),
        "an image was dropped under the lock"
    );
}

#[test]
fn the_sink_wakes_the_loop_once_when_a_quad_is_first_requested_and_never_logs() {
    let scenario = scenario();
    let (fake, _calls) = Fake::new();
    let (wake, calls) = counting_wake();
    let mut backend = ClearOnly::new(fake, "glfw-vulkan", wake);
    let sink = backend.new_sink();
    let _ = sink.publish(snapshot(1, false));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no quad was requested");
    let _ = sink.publish(snapshot(2, true));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the first request wakes the loop"
    );
    let _ = sink.publish(snapshot(3, true));
    let _ = sink.publish(snapshot(4, false));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the flag is sticky: once");
    assert_eq!(
        render_records(&scenario.records()).len(),
        0,
        "the sink never logs"
    );
}

/// A run over the real bridge: `before` happens before the window opens, `after` once it is open. Returns the
/// number of warnings the run logged once it was stopped.
fn warnings_of(
    before: impl FnOnce(&crate::SceneWriter, &Arc<crate::Image>),
    after: impl FnOnce(&crate::SceneWriter, &Arc<crate::Image>),
) -> usize {
    let scenario = scenario();
    let (fake, _calls) = Fake::new();
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    let mut renderer = Renderer::new(
        ClearOnly::new(fake, "glfw-vulkan", counting_wake().0),
        Arc::clone(&bridge),
    );
    let picture = image(10, 10);
    before(&writer, &picture);
    renderer
        .start(7, &desc(100, 100), pixels(100, 100))
        .unwrap();
    after(&writer, &picture);
    let _ = renderer.take_report();
    renderer.request_stop();
    render_records(&scenario.records()).len()
}

#[test]
fn a_quad_that_is_fully_clipped_or_has_no_pixel_extent_is_still_a_requested_quad() {
    // Fully clipped at attach: entirely right of a 100-point window.
    assert_eq!(
        warnings_of(
            |writer, picture| writer.set_quad(quad_at(picture, 500.0, 10.0, 10.0, 10.0)),
            |_, _| {}
        ),
        1,
        "clipped away at attach"
    );
    // Fully clipped, set later.
    assert_eq!(
        warnings_of(
            |_, _| {},
            |writer, picture| writer.set_quad(quad_at(picture, -50.0, -50.0, 10.0, 10.0))
        ),
        1,
        "clipped away later"
    );
    // No extent in pixels: 0.2 points at scale 1.
    assert_eq!(
        warnings_of(
            |_, _| {},
            |writer, picture| writer.set_quad(quad_at(picture, 10.0, 10.0, 0.2, 10.0))
        ),
        1,
        "no pixel extent"
    );
    // Repeated writes, clipped or not, give the one warning.
    assert_eq!(
        warnings_of(
            |writer, picture| writer.set_quad(quad_at(picture, 500.0, 10.0, 10.0, 10.0)),
            |writer, picture| {
                writer.set_quad(quad_at(picture, 10.0, 10.0, 10.0, 10.0));
                writer.set_quad(quad_at(picture, -50.0, -50.0, 10.0, 10.0));
                writer.clear_quad();
                writer.set_quad(quad_at(picture, 20.0, 20.0, 10.0, 10.0));
            }
        ),
        1,
        "repeated writes"
    );
    // A run that only ever clears requests no quad.
    assert_eq!(
        warnings_of(
            |writer, _| writer.clear_quad(),
            |writer, _| writer.clear_quad()
        ),
        0,
        "never requested"
    );
}

/// Waits (bounded) until a render record appears.
fn wait_for_a_warning(scenario: &Scenario) -> Vec<String> {
    let give_up = Instant::now() + BOUND;
    loop {
        let records = scenario.records();
        if !render_records(&records).is_empty() {
            return records;
        }
        assert!(
            Instant::now() < give_up,
            "no warning was logged: {records:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_quad_published_while_the_loop_sleeps_gets_its_warning_with_no_input_event_redraw_or_answer() {
    let scenario = scenario();
    let (mut fake, _calls) = Fake::new();
    fake.end = Some(CleanupResult::Proven);
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    let idle = IdleLoop::start(
        &bridge,
        move |wake| ClearOnly::new(fake, "glfw-vulkan", wake),
        None,
    );
    idle.native.wait_sleeping(1);
    assert_eq!(render_records(&scenario.records()).len(), 0, "nothing yet");
    writer.set_quad(quad_at(&image(10, 10), 10.0, 10.0, 10.0, 10.0));
    let records = wait_for_a_warning(&scenario);
    assert!(
        render_records(&records)[0].ends_with(WARNING),
        "{records:?}"
    );
    assert_eq!(idle.stop(), ck_window::LoopEnd::Quit);
    assert_eq!(render_records(&scenario.records()).len(), 1, "only one");
}

#[test]
fn a_quad_published_just_before_the_wait_arms_is_not_lost() {
    let scenario = scenario();
    let (mut fake, _calls) = Fake::new();
    fake.end = Some(CleanupResult::Proven);
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    let picture = image(10, 10);
    // The first sleeping pump publishes at its very entry, before it looks at the native wake: the wake the
    // sink makes is already queued (sticky) when the wait would arm.
    let mut held = Some(writer);
    let hook: Box<dyn FnMut() + Send> = Box::new(move || {
        if let Some(writer) = held.take() {
            writer.set_quad(quad_at(&picture, 10.0, 10.0, 10.0, 10.0));
        }
    });
    let idle = IdleLoop::start(
        &bridge,
        move |wake| ClearOnly::new(fake, "glfw-vulkan", wake),
        Some(hook),
    );
    let records = wait_for_a_warning(&scenario);
    assert!(
        render_records(&records)[0].ends_with(WARNING),
        "{records:?}"
    );
    idle.native.wait_sleeping(1);
    assert_eq!(idle.stop(), ck_window::LoopEnd::Quit);
    assert_eq!(render_records(&scenario.records()).len(), 1, "only one");
}

/// A backend over a fake whose end is proven, with a requested quad already published (the warning pending).
fn pending_warning() -> (ClearOnly<Fake>, Arc<Mutex<Vec<String>>>) {
    let (mut fake, calls) = Fake::new();
    fake.end = Some(CleanupResult::Proven);
    let mut backend = ClearOnly::new(fake, "glfw-vulkan", counting_wake().0);
    let sink = backend.new_sink();
    let _ = sink.publish(snapshot(1, true));
    (backend, calls)
}

/// More than the logger's wait of old (100 ms), well under what a stalled logger would cost.
const NOT_WAITING: Duration = Duration::from_millis(50);

#[test]
fn a_pending_warning_behind_a_full_queue_never_spends_the_end_budget() {
    let scenario = scenario();
    let hold = hold_sink_and_fill_the_queue(&scenario);
    for within in [Duration::ZERO, Duration::from_millis(20)] {
        let (mut backend, calls) = pending_warning();
        let started = Instant::now();
        let end = backend.poll_end(within);
        assert!(
            started.elapsed() < NOT_WAITING,
            "poll_end({within:?}) waited {:?} on the logger",
            started.elapsed()
        );
        assert_eq!(
            end,
            Some(CleanupResult::Proven),
            "the wrapped result is unchanged"
        );
        let forwarded = lock(&calls).last().cloned().expect("a forwarded call");
        assert!(forwarded.starts_with("poll_end "), "{forwarded}");
        if within.is_zero() {
            assert_eq!(forwarded, "poll_end 0ns", "a zero budget stays zero");
        }
    }
    let (mut backend, _calls) = pending_warning();
    let started = Instant::now();
    backend.request_stop();
    assert!(
        started.elapsed() < NOT_WAITING,
        "request_stop waited on the logger"
    );
    drop(hold);
}

#[test]
fn a_refused_warning_is_tried_again_off_the_end_paths_and_given_up_at_the_end() {
    let scenario = scenario();
    let hold = hold_sink_and_fill_the_queue(&scenario);
    let (mut kept, _) = pending_warning();
    let (mut ended, _) = pending_warning();
    let started = Instant::now();
    let _ = kept.take_report();
    assert!(
        started.elapsed() < NOT_WAITING,
        "take_report waited on the logger"
    );
    ended.request_stop();
    drop(hold);
    let after = scenario.records();
    assert!(
        render_records(&after).is_empty(),
        "both were refused and nothing was queued: {after:?}"
    );
    // The logger takes records again: the one that was only tried off the end paths is logged, once.
    let _ = kept.take_report();
    let _ = kept.take_report();
    // The one given up at the end is not.
    let _ = ended.take_report();
    let _ = ended.poll_end(Duration::ZERO);
    assert_eq!(
        render_records(&scenario.records()).len(),
        1,
        "one warning for the run that was still going, none for the one that ended"
    );
}
