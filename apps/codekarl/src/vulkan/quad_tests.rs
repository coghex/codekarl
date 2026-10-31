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
use super::{BindingNote, TOLERATED_MARKER};
use super::{VulkanAdapter, VulkanBackend};
use crate::fakes::{End, PresenterScript};
use crate::lifecycle::ResourceLatch;
use crate::testing::{Extras, Harness, WINDOW};
use crate::windowed::render_tests::{Recording, SCRIPT, Shape, assets, snapshots_of};
use crate::windowed::{self, DRAIN};

/// The output rule `vulkan_app.rs` applies to a real window's output, here over the records these runs wrote.
#[path = "../../tests/validation_rule/mod.rs"]
mod validation_rule;

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

// ---------------------------------------------------------------------------------------------
// The binding notice and the validation log (milestone 5.5, issue 5)
// ---------------------------------------------------------------------------------------------

fn binding(fallback: bool, text: &str) -> Script {
    Script {
        binding: Some(BindingNote {
            fallback,
            text: text.to_owned(),
        }),
        ..Script::default()
    }
}

/// The records that contain `text`.
fn containing<'a>(records: &'a [String], text: &str) -> Vec<&'a String> {
    records
        .iter()
        .filter(|record| record.contains(text))
        .collect()
}

/// Several calls of the adapter, as the loop makes them.
fn some_passes(run: &mut Started) {
    for _ in 0..3 {
        let _ = run.adapter.take_report();
        run.adapter.set_clear([0.2, 0.2, 0.2]);
        run.adapter.request_redraw();
    }
}

#[test]
fn a_fallback_notice_is_exactly_one_warn_record_with_the_diagnostic_and_the_run_goes_on() {
    let scenario = scenario();
    let diagnostic =
        "bindless unavailable on Test GPU (TestDriver 1.0): maxBoundDescriptorSets 1 < 2 requested";
    let mut run = started(binding(true, diagnostic));
    some_passes(&mut run);
    let records = scenario.records();
    let notices = containing(&records, "drawing through per-revision descriptor sets");
    assert_eq!(notices.len(), 1, "exactly one record: {records:?}");
    assert!(
        notices[0].contains("WARN")
            && notices[0].ends_with(&format!(
                "render: bindless unavailable ({diagnostic}); drawing through per-revision descriptor sets"
            )),
        "a warn in category render with the diagnostic verbatim: {}",
        notices[0]
    );
    some_passes(&mut run);
    assert_eq!(
        containing(&scenario.records(), "per-revision descriptor sets").len(),
        1,
        "a later pass writes nothing more"
    );
    assert_eq!(
        end_of(&mut run, Duration::from_secs(5)),
        Some(CleanupResult::Proven),
        "the run ends with its normal status"
    );
}

#[test]
fn a_bindless_notice_is_exactly_one_info_record_with_the_support_answer() {
    for text in [
        "bindless table of 131072 slots (vkGetDescriptorSetLayoutSupport: supported)",
        "bindless table of 131072 slots (vkGetDescriptorSetLayoutSupport: not supported although every update-after-bind limit passes; MoltenVK #2573, proceeding)",
    ] {
        let scenario = scenario();
        let mut run = started(binding(false, text));
        some_passes(&mut run);
        some_passes(&mut run);
        let records = scenario.records();
        let notices = containing(&records, "bindless table of 131072 slots");
        assert_eq!(notices.len(), 1, "exactly one record: {records:?}");
        assert!(
            notices[0].contains("INFO") && notices[0].ends_with(&format!("render: {text}")),
            "an info in category render with the text verbatim: {}",
            notices[0]
        );
        assert_eq!(
            end_of(&mut run, Duration::from_secs(5)),
            Some(CleanupResult::Proven)
        );
    }
}

const TOLERATED_ENTRY: &str =
    "[ERROR VALIDATION] VUID-vkCreateDescriptorSetLayout-support-09582: layout unsupported";
const OTHER_ENTRY: &str = "[ERROR VALIDATION] VUID-vkCmdDraw-None-02699: something else";
const LATE_ENTRY: &str =
    "[ERROR VALIDATION] VUID-vkDestroyInstance-instance-00629: from the destruction";

#[test]
fn the_markers_are_the_ones_the_output_rule_accepts() {
    assert_eq!(validation_rule::MARKER, TOLERATED_MARKER);
    assert_eq!(validation_rule::LOADER_MARKER, super::LOADER_NOTICE_MARKER);
}

/// The output rule over supplied outputs, here too (the same function `vulkan_app.rs` calls), so its recorded mutations run without
/// a window.
#[test]
fn the_output_rule_binds_each_record_to_its_marker_and_fixed_fields() {
    validation_rule::supplied_outputs_behave();
}

#[test]
fn the_tolerated_entry_is_one_marked_info_record_every_other_is_a_warn_record_and_each_is_drained_once()
 {
    let scenario = scenario();
    let mut run = started(Script {
        validation_now: vec![
            (TOLERATED_ENTRY.to_owned(), true),
            (OTHER_ENTRY.to_owned(), false),
        ],
        validation_dropped: 44,
        ..Script::default()
    });
    some_passes(&mut run);
    some_passes(&mut run);
    let records = scenario.records();
    let marked = containing(&records, TOLERATED_MARKER);
    assert_eq!(marked.len(), 1, "{records:?}");
    assert!(
        marked[0].contains("INFO") && marked[0].contains("render:"),
        "{}",
        marked[0]
    );
    assert!(
        marked[0].ends_with(&format!("{TOLERATED_MARKER}: {TOLERATED_ENTRY}")),
        "{}",
        marked[0]
    );
    let others = containing(
        &records,
        "vulkan validation: [ERROR VALIDATION] VUID-vkCmdDraw",
    );
    assert_eq!(others.len(), 1, "{records:?}");
    assert!(others[0].contains("WARN"), "{}", others[0]);
    let dropped = containing(&records, "44 further messages were dropped");
    assert_eq!(
        dropped.len(),
        1,
        "the dropped count is written once: {records:?}"
    );
    // The output rule: the marked record alone passes; the warn and the dropped count fail it.
    let marked_only: Vec<&str> = marked.iter().map(|record| record.as_str()).collect();
    assert!(validation_rule::check(&marked_only.join("\n")).is_ok());
    assert!(validation_rule::check(&records.join("\n")).is_err());
    assert_eq!(
        end_of(&mut run, Duration::from_secs(5)),
        Some(CleanupResult::Proven)
    );
    assert_eq!(
        containing(&scenario.records(), "vulkan validation").len(),
        3,
        "a drain at the end writes nothing more: the three records"
    );
}

/// Many validation messages are pending while the logger is stalled: the end drain costs one bounded wait for the whole batch (the
/// control path's deadline, taken once), not one wait per record, and the run still ends `Proven`. Negative control: waiting per
/// record would take 50 x 100 ms.
#[test]
fn a_stalled_logger_costs_the_end_drain_one_bounded_wait_not_one_per_record() {
    let scenario = scenario();
    let _hold = hold_sink_and_fill_the_queue(&scenario);
    let mut run = started(Script {
        validation_now: (0..50)
            .map(|index| (format!("[ERROR VALIDATION] VUID-x: message {index}"), false))
            .collect(),
        ..Script::default()
    });
    let began = Instant::now();
    let result = end_of(&mut run, Duration::from_secs(5));
    let took = began.elapsed();
    assert_eq!(result, Some(CleanupResult::Proven));
    assert!(
        took < Duration::from_millis(1500),
        "the whole drain is bounded by one deadline, not 50 of them: {took:?}"
    );
}

/// The end drains come out of the caller's own budget (`poll_end`'s `within`), never on top of it: with none left (`within` zero) and
/// the sink shut, the drain adds no wait at all (the control path's 100 ms would otherwise be added).
#[test]
fn the_end_drain_waits_no_longer_than_the_callers_remaining_budget() {
    let scenario = scenario();
    let _hold = hold_sink_and_fill_the_queue(&scenario);
    let mut run = started(Script {
        validation_now: (0..5)
            .map(|index| (format!("[ERROR VALIDATION] VUID-x: message {index}"), false))
            .collect(),
        ..Script::default()
    });
    let began = Instant::now();
    let result = end_of(&mut run, Duration::ZERO);
    let took = began.elapsed();
    assert_eq!(result, Some(CleanupResult::Proven));
    assert!(
        took < Duration::from_millis(60),
        "no budget left, so no wait on top of it: {took:?}"
    );
}

/// A refused destruction retains the instance, so a late callback can still add entries between the drain after the destruction and
/// the retention's own: that second closing drain is under the same caller budget, not a fresh wait after it.
#[test]
fn a_refused_destruction_retains_without_a_second_wait_beyond_the_callers_budget() {
    let scenario = scenario();
    let _hold = hold_sink_and_fill_the_queue(&scenario);
    let mut run = started(Script {
        refuse: Some(false),
        validation_late: (0..3)
            .map(|index| {
                (
                    format!("[ERROR VALIDATION] VUID-late: message {index}"),
                    false,
                )
            })
            .collect(),
        ..Script::default()
    });
    let began = Instant::now();
    let result = end_of(&mut run, Duration::ZERO);
    let took = began.elapsed();
    assert!(
        matches!(&result, Some(CleanupResult::Unproven { reason }) if reason.contains("the instance could not be destroyed")),
        "the destruction was refused and everything retained: {result:?}"
    );
    assert!(
        took < Duration::from_millis(60),
        "both closing drains come out of the caller's budget: {took:?}"
    );
}

/// The loop's teardown has no caller budget: its destruction drain and, on a refusal, the retention's share one control-path deadline
/// (one wait of about 100 ms), not one each.
#[test]
fn the_loops_teardown_shares_one_deadline_between_the_destruction_drain_and_the_retention() {
    let scenario = scenario();
    let _hold = hold_sink_and_fill_the_queue(&scenario);
    let (run, _log) = run_over(Script {
        refuse: Some(false),
        validation_now: (0..3)
            .map(|index| (format!("[ERROR VALIDATION] VUID-x: message {index}"), false))
            .collect(),
        validation_late: (0..3)
            .map(|index| {
                (
                    format!("[ERROR VALIDATION] VUID-late: message {index}"),
                    false,
                )
            })
            .collect(),
        ..Script::default()
    });
    let (backend, _adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    backend.create_window(&desc()).expect("a window");
    let began = Instant::now();
    backend.destroy_window();
    let took = began.elapsed();
    assert!(run.retained().is_some(), "the refusal retained everything");
    assert!(
        took < Duration::from_millis(160),
        "one shared deadline, not one wait for each drain: {took:?}"
    );
}

#[test]
fn the_loaders_env_layer_notice_is_one_info_record_with_its_own_marker_and_passes_the_output_rule()
{
    let scenario = scenario();
    let notice = "[WARNING GENERAL] Loader Message: env var 'VK_INSTANCE_LAYERS' defined and adding layers \"VK_LAYER_KHRONOS_validation\"";
    let mut run = started(Script {
        validation_now: vec![(notice.to_owned(), false), (notice.to_owned(), false)],
        ..Script::default()
    });
    some_passes(&mut run);
    let records = scenario.records();
    let loader = containing(&records, super::LOADER_NOTICE_MARKER);
    assert_eq!(
        loader.len(),
        2,
        "one record per message, both marked: {records:?}"
    );
    assert!(
        loader.iter().all(|record| record.contains("INFO")),
        "{loader:?}"
    );
    assert!(
        containing(&records, "vulkan validation:").is_empty(),
        "and none is a validation record: {records:?}"
    );
    assert!(
        validation_rule::check(&records.join("\n")).is_ok(),
        "the output rule accepts them"
    );
    // The same words as an unmarked validation record fail the rule.
    assert!(validation_rule::check(&format!("WARN  render: vulkan validation: {notice}")).is_err());
    assert_eq!(
        end_of(&mut run, Duration::from_secs(5)),
        Some(CleanupResult::Proven)
    );
}

#[test]
fn a_message_the_destruction_appends_after_the_instance_is_consumed_is_written_by_the_final_drain()
{
    let scenario = scenario();
    let mut run = started(Script {
        validation_on_destroy: vec![(LATE_ENTRY.to_owned(), false)],
        ..Script::default()
    });
    some_passes(&mut run);
    assert!(containing(&scenario.records(), "VUID-vkDestroyInstance").is_empty());
    assert_eq!(
        end_of(&mut run, Duration::from_secs(5)),
        Some(CleanupResult::Proven)
    );
    let records = scenario.records();
    let late = containing(&records, "VUID-vkDestroyInstance-instance-00629");
    assert_eq!(
        late.len(),
        1,
        "written once, after the destruction: {records:?}"
    );
    assert!(late[0].contains("WARN"), "{}", late[0]);
    // The same output rule fails on it, because it is unmarked.
    assert!(validation_rule::check(&records.join("\n")).is_err());
}

#[test]
fn a_failed_start_whose_cleanup_appends_a_message_still_writes_it() {
    let scenario = scenario();
    let (run, _log) = run_over(Script {
        presenter: PresenterScript {
            start_error: Some("no device".to_owned()),
            ..PresenterScript::default()
        },
        validation_on_destroy: vec![(LATE_ENTRY.to_owned(), false)],
        ..Script::default()
    });
    let (backend, mut adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    assert!(start_on(&mut adapter, &created).is_err());
    backend.destroy_window();
    backend.terminate();
    let records = scenario.records();
    assert_eq!(
        containing(&records, "VUID-vkDestroyInstance-instance-00629").len(),
        1,
        "the cleanup's message is written: {records:?}"
    );
    assert!(validation_rule::check(&records.join("\n")).is_err());
}

/// A creation that fails returns no instance and so no log handle: what its log recorded still becomes records, every entry, one each,
/// classified like any other (none summarised, none lost).
#[test]
fn a_failed_instance_creation_has_every_entry_of_its_log_written_as_a_classified_record() {
    let scenario = scenario();
    let mut entries: Vec<(String, bool)> = (0..10)
        .map(|index| {
            (
                format!("[ERROR VALIDATION] VUID-creation: message {index}"),
                false,
            )
        })
        .collect();
    entries.push((TOLERATED_ENTRY.to_owned(), true));
    let (run, _log) = run_over(Script {
        instance_error: Some("vkCreateInstance failed".to_owned()),
        validation_on_failed_creation: entries,
        ..Script::default()
    });
    let (backend, _adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    let error = backend
        .create_window(&desc())
        .expect_err("the instance fails");
    assert!(
        error.contains("cannot create the Vulkan instance"),
        "{error}"
    );
    backend.terminate();
    let records = scenario.records();
    assert_eq!(
        containing(
            &records,
            "vulkan validation: [ERROR VALIDATION] VUID-creation"
        )
        .len(),
        10,
        "all ten, one record each: {records:?}"
    );
    assert_eq!(
        containing(&records, TOLERATED_MARKER).len(),
        1,
        "and the tolerated one marked"
    );
    assert!(validation_rule::check(&records.join("\n")).is_err());
}

#[test]
fn an_instance_destroyed_before_the_loop_begins_has_its_messages_written_after_its_destruction() {
    let scenario = scenario();
    let (run, log) = run_over(Script {
        surface_error: Some("no surface".to_owned()),
        validation_on_destroy: vec![(LATE_ENTRY.to_owned(), false)],
        ..Script::default()
    });
    let (backend, _adapter) = parts(&run);
    let mut backend = backend.expect("opened");
    assert!(backend.create_window(&desc()).is_err());
    backend.terminate();
    let records = scenario.records();
    assert_eq!(
        containing(&records, "VUID-vkDestroyInstance-instance-00629").len(),
        1,
        "{records:?}"
    );
    // The drain came after the destruction: the helper's log line precedes the last drain's.
    let log = events(&log);
    let destroyed = at(&log, "instance destroyed");
    assert!(
        log.iter()
            .enumerate()
            .any(|(index, event)| event == "validation drain" && index > destroyed),
        "a drain after the destruction: {log:?}"
    );
}
