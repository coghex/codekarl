//! The run's measurement window on the `glfw-vulkan` path, over the fake stack and without a window or a GPU (milestone 5.5, issue 6,
//! part B): `diagnostics.metrics_mark` opens the one window, the adapter hands it to the presenter, sets its end once under the
//! presenter's admission lock, and writes the one `metrics` record when the presenter's end is observed. The same `Run`, backend and
//! adapter the production run uses run over [`FakeStack`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ck_render_2d::Gpu2d;
use ck_window::{Backend, CleanupResult, UptimeNs};

use super::fake::{FakeStack, Script, new_run_with};
use super::logging::scenario;
use super::tests::{desc, start_on, vector_log};
use super::{VulkanAdapter, VulkanBackend};
use crate::metrics::{Marked, MetricsHub};
use crate::metrics_record::{PresenterFigures, ReadingView, StatsView};
use ck_window::Notifier;

/// A run with a measurement window over a hub on a clock the case moves.
struct Rig {
    adapter: VulkanAdapter<FakeStack>,
    _backend: VulkanBackend<FakeStack>,
    hub: Arc<MetricsHub>,
    now: Arc<AtomicU64>,
    log: Arc<Mutex<Vec<String>>>,
}

/// A hub on a clock at 10 s, and a run over a fake stack with `script` that holds it. The presenter is **not** started yet.
fn unstarted(
    script: Script,
) -> (
    Rig,
    ck_window::CreatedWindow<<FakeStack as super::Stack>::Handles>,
) {
    let now = Arc::new(AtomicU64::new(10_000_000_000));
    let clock: UptimeNs = {
        let now = Arc::clone(&now);
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let hub = Arc::new(MetricsHub::new(clock, true));
    let (log, events) = vector_log();
    let run = new_run_with(
        FakeStack::new(script, log, Notifier::new()),
        Arc::new(|| {}),
        Some(Arc::clone(&hub)),
    );
    let adapter = VulkanAdapter::new(Rc::clone(&run), Arc::new(|| {}));
    let mut backend = VulkanBackend::open(run).expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    (
        Rig {
            adapter,
            _backend: backend,
            hub,
            now,
            log: events,
        },
        created,
    )
}

use std::rc::Rc;

fn events(rig: &Rig) -> Vec<String> {
    rig.log.lock().expect("the log").clone()
}

fn record_figures() -> PresenterFigures {
    PresenterFigures {
        totals: StatsView {
            frames: 7,
            ..StatsView::default()
        },
        window: Some(ReadingView {
            adopted_at: Some(10_100_000_000),
            complete: true,
            stats: StatsView {
                frames: 3,
                ..StatsView::default()
            },
            ..ReadingView::default()
        }),
        gpu_unavailable: Some("timestampValidBits 0".to_owned()),
        presentation_unavailable: Some("VK_GOOGLE_display_timing not advertised".to_owned()),
        ..PresenterFigures::default()
    }
}

#[test]
fn a_window_marked_before_the_presenter_exists_is_handed_over_once_it_does() {
    let (mut rig, created) = unstarted(Script::default());
    let Marked::Started(window) = rig.hub.mark() else {
        panic!("the first mark starts the window");
    };
    // No presenter yet: the pass keeps the window to be forwarded, and asks for nothing.
    let _ = rig.adapter.take_report();
    assert!(
        !events(&rig)
            .iter()
            .any(|event| event.starts_with("metrics window requested")),
        "{:?}",
        events(&rig)
    );
    start_on(&mut rig.adapter, &created).expect("started");
    let _ = rig.adapter.take_report();
    let _ = rig.adapter.take_report();
    let requested: Vec<String> = events(&rig)
        .into_iter()
        .filter(|event| event.starts_with("metrics window requested"))
        .collect();
    assert_eq!(
        requested,
        [format!(
            "metrics window requested id={} start={}",
            window.id, window.start
        )],
        "once, however many passes follow"
    );
}

#[test]
fn the_stop_sets_the_windows_end_once_under_the_admission_lock_before_the_presenter_is_told_to_stop()
 {
    let (mut rig, created) = unstarted(Script::default());
    let Marked::Started(window) = rig.hub.mark() else {
        panic!("started");
    };
    start_on(&mut rig.adapter, &created).expect("started");
    let _ = rig.adapter.take_report();
    rig.now.store(10_800_000_000, Ordering::SeqCst);
    rig.adapter.request_stop();
    rig.adapter.request_stop();
    let log = events(&rig);
    let position = |text: &str| log.iter().position(|event| event.starts_with(text));
    let locked = position("metrics end locked").expect("the end was set under the lock");
    let set = position(&format!("metrics end id={} end=10800000000", window.id))
        .expect("the one end timestamp");
    let unlocked = position("metrics end unlocked").expect("and released");
    let stopped = position("presenter stop").expect("the presenter is told to stop");
    assert!(locked < set && set < unlocked, "{log:?}");
    assert!(
        unlocked < stopped,
        "the end is set before the stop: {log:?}"
    );
    assert_eq!(
        log.iter()
            .filter(|event| event.starts_with("metrics end id="))
            .count(),
        1,
        "the end is the run's one timestamp, whatever the number of stops: {log:?}"
    );
}

#[test]
fn the_end_writes_one_metrics_record_with_the_window_and_the_unavailable_notes() {
    let scenario = scenario();
    let (mut rig, created) = unstarted(Script {
        record: record_figures(),
        ..Script::default()
    });
    let _ = rig.hub.mark();
    start_on(&mut rig.adapter, &created).expect("started");
    let _ = rig.adapter.take_report();
    rig.now.store(10_800_000_000, Ordering::SeqCst);
    rig.adapter.request_stop();
    assert_eq!(
        rig.adapter.poll_end(Duration::from_millis(100)),
        Some(CleanupResult::Proven)
    );
    // A second observation of the end writes nothing more.
    assert_eq!(
        rig.adapter.poll_end(Duration::from_millis(100)),
        Some(CleanupResult::Proven)
    );
    let records = scenario.records();
    let metrics: Vec<&String> = records
        .iter()
        .filter(|record| record.contains("metrics:"))
        .collect();
    assert_eq!(metrics.len(), 1, "one record: {records:?}");
    let text = metrics[0];
    assert!(text.contains("DEBUG"), "{text}");
    assert!(text.contains("run: frames=7"), "{text}");
    assert!(
        text.contains("window id=1 start=10250000000 end=10800000000"),
        "{text}"
    );
    assert!(
        text.contains("presenter window adopted=10100000000"),
        "{text}"
    );
    assert!(text.contains("window: frames=3"), "{text}");
    assert!(
        text.contains("gpu: unavailable (timestampValidBits 0)"),
        "{text}"
    );
    assert!(
        text.contains("presentation: unavailable (VK_GOOGLE_display_timing not advertised)"),
        "{text}"
    );
}

#[test]
fn a_run_without_a_mark_reports_the_totals_only() {
    let scenario = scenario();
    let (mut rig, created) = unstarted(Script {
        record: record_figures(),
        ..Script::default()
    });
    start_on(&mut rig.adapter, &created).expect("started");
    let _ = rig.adapter.take_report();
    rig.adapter.request_stop();
    let _ = rig.adapter.poll_end(Duration::from_millis(100));
    let records = scenario.records();
    let text = records
        .iter()
        .find(|record| record.contains("metrics:"))
        .expect("the record");
    assert!(text.contains("window: none (no metrics_mark)"), "{text}");
    assert!(!text.contains("presenter window"), "{text}");
    assert!(
        !events(&rig)
            .iter()
            .any(|event| event.starts_with("metrics window requested")
                || event.starts_with("metrics end")),
        "nothing is forwarded or ended"
    );
}

#[test]
fn a_presenter_refusal_of_the_window_is_in_the_record() {
    let scenario = scenario();
    let (mut rig, created) = unstarted(Script {
        record: record_figures(),
        metrics_refusal: Some("Overlaps".to_owned()),
        ..Script::default()
    });
    let _ = rig.hub.mark();
    start_on(&mut rig.adapter, &created).expect("started");
    let _ = rig.adapter.take_report();
    rig.adapter.request_stop();
    let _ = rig.adapter.poll_end(Duration::from_millis(100));
    let records = scenario.records();
    let text = records
        .iter()
        .find(|record| record.contains("metrics:"))
        .expect("the record");
    assert!(text.contains("presenter_refused=Overlaps"), "{text}");
}

#[test]
fn a_run_with_no_measurement_window_writes_no_record() {
    let scenario = scenario();
    let (log, _events) = vector_log();
    let run = super::fake::new_run(
        FakeStack::new(Script::default(), log, Notifier::new()),
        Arc::new(|| {}),
    );
    let mut adapter = VulkanAdapter::new(Rc::clone(&run), Arc::new(|| {}));
    let mut backend = VulkanBackend::open(run).expect("opened");
    let created = backend.create_window(&desc()).expect("a window");
    start_on(&mut adapter, &created).expect("started");
    adapter.request_stop();
    let _ = adapter.poll_end(Duration::from_millis(100));
    assert!(
        !scenario
            .records()
            .iter()
            .any(|record| record.contains("metrics:")),
        "a headless-like run has no hub and no record"
    );
}

/// The record's reads come out of the caller's budget (`poll_end`'s `within`), never on top of it: with nothing left they do not
/// wait (the presenter's read gets a zero bound), and with a little left they wait at most that, however large the summary wait is.
#[test]
fn the_records_reads_are_capped_by_what_remains_of_the_callers_budget() {
    // Each run below writes a `metrics` record: the scenario lock keeps them out of the other cases' windows.
    let _scenario = scenario();
    let figures_wait = |within: Duration| -> u64 {
        let (mut rig, created) = unstarted(Script {
            record: record_figures(),
            ..Script::default()
        });
        let _ = rig.hub.mark();
        start_on(&mut rig.adapter, &created).expect("started");
        let _ = rig.adapter.take_report();
        rig.adapter.request_stop();
        let _ = rig.adapter.poll_end(within);
        events(&rig)
            .iter()
            .find_map(|event| event.split("within_ms=").nth(1))
            .and_then(|ms| ms.parse().ok())
            .expect("the presenter's read was asked")
    };
    assert_eq!(
        figures_wait(Duration::ZERO),
        0,
        "nothing left: a read that does not wait"
    );
    assert!(
        figures_wait(Duration::from_millis(40)) <= 40,
        "a little left: at most that"
    );
    assert_eq!(
        figures_wait(Duration::from_secs(10)),
        crate::metrics::SUMMARY_WAIT
            .as_millis()
            .try_into()
            .expect("small"),
        "plenty left: the summary wait itself"
    );
}

/// An unproven end cannot add renewed summary time either: the presenter says its cleanup failed, the caller has nothing left, and the
/// record's reads are still non-waiting (and the end stays unproven, the record still honest).
#[test]
fn an_unproven_end_with_nothing_left_adds_no_summary_wait() {
    let scenario = scenario();
    let (mut rig, created) = unstarted(Script {
        record: record_figures(),
        presenter: crate::fakes::PresenterScript {
            end: crate::fakes::End::Unproven("the stop deadline passed".to_owned()),
            ..crate::fakes::PresenterScript::default()
        },
        ..Script::default()
    });
    let _ = rig.hub.mark();
    start_on(&mut rig.adapter, &created).expect("started");
    let _ = rig.adapter.take_report();
    rig.adapter.request_stop();
    let began = std::time::Instant::now();
    let end = rig.adapter.poll_end(Duration::ZERO);
    assert!(
        matches!(end, Some(CleanupResult::Unproven { .. })),
        "{end:?}"
    );
    assert!(
        began.elapsed() < Duration::from_millis(100),
        "{:?}",
        began.elapsed()
    );
    let asked: Vec<String> = events(&rig)
        .into_iter()
        .filter(|event| event.starts_with("metrics figures"))
        .collect();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].ends_with("within_ms=0"), "{asked:?}");
    assert!(
        scenario
            .records()
            .iter()
            .any(|record| record.contains("metrics:")),
        "the record is still written, with whatever the presenter had"
    );
}
