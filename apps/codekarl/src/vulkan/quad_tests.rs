//! The quad on the `glfw-vulkan` path, over the fake stack and without a window or a GPU: what a script's
//! `render.quad` becomes at the quad side, the sink check, the refusal and its priority, the format notice and its
//! final drain, the first-frame record and the end paths. The same `Run`, backend and adapter the production run
//! uses run over [`FakeStack`]; the notice and first-frame records are read from a process logger installed for
//! these tests (see [`super::logging`]).

use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ck_render_2d::{Gpu2d, QuadSink, Renderer};
use ck_runtime::RunEnd;
use ck_window::{Backend, CleanupResult, Presenter, PresenterReport};

use super::fake::{FakeStack, Scene, Script};
use super::logging::{hold_sink_and_fill_the_queue, scenario};
use super::tests::{at, desc, events, parts, run_over, start_on};
use super::{VulkanAdapter, VulkanBackend};
use crate::fakes::{End, PresenterScript};
use crate::lifecycle::ResourceLatch;
use crate::testing::{Extras, Harness, WINDOW};
use crate::windowed::render_tests::{Recording, SCRIPT, Shape, assets, snapshots_of};
use crate::windowed::{self, DRAIN};

/// What a run over the fake stack left behind.
struct Outcome {
    end: RunEnd,
    scenes: Vec<Scene>,
    log: Vec<String>,
    harness: Harness,
}

/// Runs `body` (after `window.open`) through `run_window_keeping` with the production backend and adapter over a
/// fake stack with `script`, with an assets directory holding `small.png` (8 x 4).
fn run_script(script: Script, body: &str) -> Outcome {
    let dir = assets();
    let harness = Harness::new(
        &format!("{WINDOW}\n{body}"),
        Extras {
            assets: Some(dir.0.clone()),
            ..Extras::default()
        },
    );
    let (log, recorded) = super::tests::vector_log();
    let stack = FakeStack::new(script, log, Arc::clone(&harness.notifier));
    let slot = Arc::clone(&stack.slot);
    let notifier = Arc::clone(&harness.notifier);
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || notifier.wake());
    let run = super::fake::new_run(stack, Arc::clone(&wake));
    let presenter = Renderer::new(
        VulkanAdapter::new(Rc::clone(&run), wake),
        Arc::clone(&harness.bridge),
    );
    let (opener, kept) = (Rc::clone(&run), Rc::clone(&run));
    let latch = ResourceLatch::new();
    let end = windowed::run_window_keeping(
        &harness.env(),
        move || VulkanBackend::open(opener),
        presenter,
        &latch,
        DRAIN,
        &|| kept.retained().is_some(),
    );
    let scenes = slot
        .scenes
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    drop(dir);
    Outcome {
        end,
        scenes,
        log: events(&recorded),
        harness,
    }
}

const OPEN_AND_CLOSE: &str = "window.on_opened(function() window.close() end)";

#[test]
fn a_scripts_quad_arrives_at_the_fake_side_with_its_generation_rectangles_and_filter() {
    let outcome = run_script(
        Script::default(),
        &format!("render.quad{{image='small.png', x=10, y=5}}\n{OPEN_AND_CLOSE}"),
    );
    assert_eq!(outcome.end, RunEnd::Finished);
    assert!(
        outcome
            .log
            .contains(&"presenter start with the side's slot".to_owned()),
        "{:?}",
        outcome.log
    );
    let (generation, requested, placed) = outcome.scenes.first().expect("a scene").clone();
    assert!(generation >= 1 && requested);
    // At (10, 5) points, at the fake window's scale of 2: 8 x 4 image pixels become 16 x 8 device pixels.
    assert_eq!(
        placed,
        Some((
            [20, 10, 36, 18],
            [0.0, 0.0, 8.0, 4.0],
            ck_render_2d::Filter::Nearest.to_string()
        ))
    );
    let counts = outcome.harness.bridge.counts();
    assert_eq!((counts.accepted, counts.discarded), (1, 0), "{counts:?}");
}

#[test]
fn clear_quad_arrives_as_no_quad_with_a_newer_generation() {
    let outcome = run_script(
        Script::default(),
        "render.quad{image='small.png', x=10, y=5}\n\
         window.on_opened(function() render.clear_quad() window.close() end)",
    );
    assert_eq!(outcome.end, RunEnd::Finished);
    let first = outcome.scenes.first().expect("the quad's scene").clone();
    let last = outcome.scenes.last().expect("a last scene").clone();
    assert!(first.2.is_some(), "{:?}", outcome.scenes);
    assert_eq!(last.2, None, "clear_quad is no quad: {:?}", outcome.scenes);
    assert!(!last.1, "and nothing is requested");
    assert!(last.0 > first.0, "with its own generation");
}

/// What a Metal-shaped scene (`Shape`) and a Vulkan-path scene both come to.
fn plain(scene: &Scene) -> Shape {
    (
        scene.0,
        scene.1,
        scene
            .2
            .as_ref()
            .map(|(destination, source, _)| (*destination, *source)),
    )
}

#[test]
fn the_same_script_gives_the_same_snapshots_to_a_metal_shaped_backend_and_to_the_vulkan_path() {
    let (metal, metal_end) = snapshots_of(|harness, seen| {
        Renderer::new(
            Recording {
                inner: crate::fakes::FakeGpu::default(),
                seen,
            },
            Arc::clone(&harness.bridge),
        )
    });
    // The very script `snapshots_of` runs, through the Vulkan path.
    let vulkan = run_script(Script::default(), SCRIPT);
    assert_eq!(metal_end, RunEnd::Finished);
    assert_eq!(vulkan.end, RunEnd::Finished);
    assert!(!metal.is_empty(), "the bridge published to the backend");
    let vulkan_shapes: Vec<Shape> = vulkan.scenes.iter().map(plain).collect();
    assert_eq!(metal, vulkan_shapes, "the same script, the same snapshots");
}

#[test]
fn start_refuses_a_sink_the_side_did_not_make_and_starts_nothing() {
    let (run, log) = run_over(Script::default());
    let (backend, mut adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    // A sink of another run's side.
    let (other_run, _other_log) = run_over(Script::default());
    let (_other_backend, mut other_adapter) = parts(&other_run);
    let foreign: Arc<dyn QuadSink> = other_adapter.new_sink();
    let own = adapter.new_sink();
    let refused = Gpu2d::start(&mut adapter, (), &desc(), created.pixel_size, foreign);
    assert_eq!(
        refused.expect_err("a foreign sink"),
        "the sink is not the one this backend made"
    );
    assert!(
        run.state.borrow().presenter.is_none(),
        "nothing was started"
    );
    let log = events(&log);
    assert!(
        !log.iter().any(|event| event.starts_with("presenter start")),
        "{log:?}"
    );
    // The sink it made starts it.
    assert!(Gpu2d::start(&mut adapter, (), &desc(), created.pixel_size, own).is_ok());
}

#[test]
fn a_sink_made_before_a_later_one_is_stale_and_is_refused() {
    let (run, _log) = run_over(Script::default());
    let (backend, mut adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    let stale = adapter.new_sink();
    let _current = adapter.new_sink();
    assert!(Gpu2d::start(&mut adapter, (), &desc(), created.pixel_size, stale).is_err());
}

#[test]
fn a_refusal_from_the_fake_side_is_one_failed_taken_before_the_presenters_reports() {
    let outcome = run_script(
        Script {
            refuse_quads: true,
            presenter: PresenterScript {
                fail: Some("scripted presenter failure".to_owned()),
                ..PresenterScript::default()
            },
            ..Script::default()
        },
        "render.quad{image='small.png', x=10, y=5}\nwindow.on_opened(function() end)",
    );
    let RunEnd::Failed(message) = &outcome.end else {
        panic!("the refusal ends the run: {:?}", outcome.end);
    };
    assert!(
        message.contains("the fake side refused a quad"),
        "the side's refusal is first: {message}"
    );
    assert!(
        !message.contains("scripted presenter failure"),
        "and the presenter's failure came after it: {message}"
    );
}

#[test]
fn a_refusal_is_reported_once_by_the_adapter() {
    // The adapter alone: the side's refusal first, once, then the presenter's reports as they are.
    let (run, _log) = run_over(Script {
        presenter: PresenterScript {
            fail: Some("presenter failure".to_owned()),
            ..PresenterScript::default()
        },
        ..Script::default()
    });
    let (backend, mut adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    start_on(&mut adapter, &created).expect("started");
    assert_eq!(
        adapter.take_report(),
        Some(PresenterReport::FirstFrameQueued)
    );
    assert_eq!(
        adapter.take_report(),
        Some(PresenterReport::Failed {
            message: "presenter failure".to_owned()
        })
    );
    assert_eq!(adapter.take_report(), None);
}

#[test]
fn a_failed_start_detaches_the_bridge() {
    let (run, _log) = run_over(Script {
        presenter: PresenterScript {
            start_error: Some("no device".to_owned()),
            ..PresenterScript::default()
        },
        ..Script::default()
    });
    let harness = Harness::new(WINDOW, Extras::default());
    let (backend, adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    let mut renderer = Renderer::new(adapter, Arc::clone(&harness.bridge));
    let failure = Presenter::start(&mut renderer, (), &desc(), created.pixel_size)
        .expect_err("the presenter does not start");
    assert!(failure.contains("no device"), "{failure}");
    assert_eq!(
        harness.bridge.phase(),
        ck_render_2d::Phase::Ended,
        "a failed start detaches the bridge"
    );
}

// ---------------------------------------------------------------------------------------------
// The format notice and the first-frame record (read from the tests' logger)
// ---------------------------------------------------------------------------------------------

const NOTICE: &str =
    "textured quad skipped: the glfw-vulkan surface format is R16G16B16A16_SFLOAT, not UNORM";

/// How many records end with `text`.
fn count(records: &[String], text: &str) -> usize {
    records
        .iter()
        .filter(|record| record.ends_with(text))
        .count()
}

/// A started adapter over a fake stack with `script`, its log, and the pieces it needs to stay alive.
struct Started {
    adapter: VulkanAdapter<FakeStack>,
    _backend: VulkanBackend<FakeStack>,
    log: Arc<Mutex<Vec<String>>>,
}

fn started(script: Script) -> Started {
    let (run, log) = run_over(script);
    let (backend, mut adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    start_on(&mut adapter, &created).expect("started");
    Started {
        adapter,
        _backend: backend,
        log,
    }
}

fn with_notice() -> Script {
    Script {
        notice: Some("R16G16B16A16_SFLOAT".to_owned()),
        ..Script::default()
    }
}

#[test]
fn the_notice_is_logged_once_with_its_exact_text() {
    let scenario = scenario();
    let mut run = started(with_notice());
    let _ = run.adapter.take_report();
    run.adapter.set_clear([0.5, 0.5, 0.5]);
    run.adapter.request_redraw();
    let _ = run.adapter.take_report();
    let records = scenario.records();
    assert_eq!(count(&records, NOTICE), 1, "{records:?}");
    assert!(
        records
            .iter()
            .any(|record| record.contains("WARN") && record.contains("render: textured quad")),
        "a warn record in category render: {records:?}"
    );
}

#[test]
fn no_notice_means_no_record() {
    let scenario = scenario();
    let mut run = started(Script::default());
    let _ = run.adapter.take_report();
    let records = scenario.records();
    assert_eq!(count(&records, "not UNORM"), 0, "{records:?}");
}

/// The adapter stopped and polled to its end, as the loop does.
fn end_of(run: &mut Started, within: Duration) -> Option<CleanupResult> {
    run.adapter.request_stop();
    run.adapter.poll_end(within)
}

#[test]
fn a_notice_that_becomes_available_during_poll_end_is_logged_before_the_handle_is_released_proven()
{
    let scenario = scenario();
    let mut run = started(Script {
        notice_at_end: Some("R16G16B16A16_SFLOAT".to_owned()),
        ..Script::default()
    });
    assert_eq!(
        end_of(&mut run, Duration::from_millis(50)),
        Some(CleanupResult::Proven)
    );
    let records = scenario.records();
    assert_eq!(count(&records, NOTICE), 1, "{records:?}");
    let log = events(&run.log);
    assert!(
        at(&log, "notice taken") < at(&log, "presenter dropped"),
        "the notice was taken before the handle was released: {log:?}"
    );
}

#[test]
fn a_notice_that_becomes_available_during_poll_end_is_logged_before_the_handle_is_retained_unproven()
 {
    let scenario = scenario();
    let mut run = started(Script {
        notice_at_end: Some("R16G16B16A16_SFLOAT".to_owned()),
        presenter: PresenterScript {
            end: End::Unproven("no cleanup result".to_owned()),
            ..PresenterScript::default()
        },
        ..Script::default()
    });
    assert!(matches!(
        end_of(&mut run, Duration::from_millis(50)),
        Some(CleanupResult::Unproven { .. })
    ));
    let records = scenario.records();
    assert_eq!(count(&records, NOTICE), 1, "{records:?}");
    let log = events(&run.log);
    assert!(log.contains(&"notice taken".to_owned()), "{log:?}");
    assert!(
        !log.contains(&"presenter dropped".to_owned()),
        "an unproven end keeps the handle: {log:?}"
    );
}

#[test]
fn a_saturated_logger_refuses_the_record_which_is_retried_while_the_run_is_active() {
    let scenario = scenario();
    let mut run = started(with_notice());
    {
        let _hold = hold_sink_and_fill_the_queue(&scenario);
        let started = Instant::now();
        run.adapter.set_clear([0.1, 0.1, 0.1]);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the logger never makes a call wait"
        );
        assert!(
            run.adapter.notice.is_some(),
            "the refused record is kept to be tried again"
        );
    }
    // The sink is open again: the next call logs it, once.
    run.adapter.request_redraw();
    let mut tries = 0;
    while run.adapter.notice.is_some() && tries < 200 {
        run.adapter.request_redraw();
        std::thread::sleep(Duration::from_millis(5));
        tries += 1;
    }
    assert!(run.adapter.notice.is_none(), "it was logged in the end");
    let records = scenario.records();
    assert_eq!(count(&records, NOTICE), 1, "{records:?}");
}

#[test]
fn a_saturated_logger_makes_an_end_path_give_the_record_up_without_waiting() {
    let scenario = scenario();
    let mut run = started(with_notice());
    let began = Instant::now();
    {
        let _hold = hold_sink_and_fill_the_queue(&scenario);
        assert_eq!(
            end_of(&mut run, Duration::from_millis(200)),
            Some(CleanupResult::Proven)
        );
        assert!(
            run.adapter.notice.is_none(),
            "an end path gives a refused record up"
        );
    }
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "nothing waited for the logger"
    );
    let records = scenario.records();
    assert_eq!(
        count(&records, NOTICE),
        0,
        "the record was dropped, not delayed: {records:?}"
    );
}

#[test]
fn the_first_frame_record_names_the_drawn_generation_once() {
    let scenario = scenario();
    let mut run = started(Script {
        drawn_generation: Some(7),
        ..Script::default()
    });
    assert_eq!(
        run.adapter.take_report(),
        Some(PresenterReport::FirstFrameQueued)
    );
    let _ = run.adapter.take_report();
    let _ = run.adapter.take_report();
    let records = scenario.records();
    assert_eq!(
        count(&records, "render: first frame drawn, generation 7"),
        1,
        "{records:?}"
    );
    assert!(
        records
            .iter()
            .any(|record| record.contains("DEBUG") && record.contains("first frame drawn")),
        "a debug record: {records:?}"
    );
}

#[test]
fn a_presenter_without_a_drawn_generation_writes_no_first_frame_record() {
    let scenario = scenario();
    let mut run = started(Script::default());
    assert_eq!(
        run.adapter.take_report(),
        Some(PresenterReport::FirstFrameQueued)
    );
    let _ = scenario.records();
    assert_eq!(
        count(&scenario.records(), "first frame drawn, generation 7"),
        0
    );
}
