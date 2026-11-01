//! The scene presenter's frame timing on a real window (milestone 5.5, issue 6, part B): the common measurement window on the render
//! thread, the timestamp queries read after a frame's proof, and `VK_GOOGLE_display_timing`'s feedback (optional, never a
//! requirement). A submodule of `window_scene`; no case sleeps to synchronise: a hold is the host-event hold, a wait is on an
//! observable fact, and every window is placed on the harness's clock.

use ck_gpu_vulkan::{MetricsWindow, TestFeatures};

use super::{
    BOUND, CleanupResult, Duration, Event, Harness, Instant, Rig, count, destroy_event, event,
    quad, set_event,
};

/// How far ahead of `now` a requested window starts: both owners adopt at their next boundary, before it opens.
const LEAD: u64 = 40_000_000;

/// Publishes changing frames for `span`, pumping the window, so the presenter hands frames off steadily.
fn run_frames(harness: &mut Harness, span: Duration) {
    let until = Instant::now() + span;
    let mut step = 0_i32;
    while Instant::now() < until {
        step += 1;
        harness.publish(vec![quad(1, 10 + step % 40, 10, 100)]);
        harness.rig.pump();
        std::thread::sleep(Duration::from_millis(8));
    }
}

/// A frame is handed off in a window placed ahead of it: admitted by its own hand-off mark; samples before the start and at or after
/// the end are counted outside; the window was adopted before it opened, so it is not late; the components and the acquire cover
/// the hand-off exactly (their counts equal the frames); a request that overlaps the window is refused.
pub(crate) fn a_window_admits_frames_by_their_own_handoff_mark() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start();
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    let start = harness.now() + LEAD;
    presenter
        .request_metrics(MetricsWindow {
            id: 1,
            start,
            end: None,
        })
        .expect("requested");
    assert!(
        presenter
            .request_metrics(MetricsWindow {
                id: 2,
                start: start + 1,
                end: None
            })
            .is_err(),
        "a second request while one is unadopted or current overlaps"
    );
    run_frames(&mut harness, Duration::from_millis(500));
    let end = harness.now();
    presenter.end_metrics(1, end).expect("ended once");
    let reading = presenter
        .read_metrics(1, BOUND)
        .expect("the presenter keeps the window");
    assert!(reading.complete, "{reading:?}");
    let window = &reading.window;
    assert!(!window.late_adopted, "adopted before it opened: {window:?}");
    assert!(
        window.adopted_at.is_some_and(|at| at <= start),
        "{window:?}"
    );
    let stats = &window.stats;
    assert!(stats.frames >= 5, "frames flowed in the window: {stats:?}");
    assert_eq!(stats.cpu_total.count(), stats.frames, "{stats:?}");
    assert_eq!(stats.acquire.count(), stats.frames);
    assert_eq!(stats.build.count(), stats.frames);
    assert_eq!(stats.encode.count(), stats.frames);
    assert_eq!(stats.ledger.count(), stats.frames);
    assert_eq!(stats.draw_calls_max, 1, "one draw per bindless frame");
    assert_eq!(
        stats.texture_binds, stats.frames,
        "one table bind per frame"
    );
    assert!(
        window.outside > 0,
        "the frames before the start and after the end are outside: {window:?}"
    );
    let totals = presenter.metrics_totals();
    assert!(
        totals.frames > stats.frames,
        "the run's totals hold every frame, the window's only its own"
    );
    harness.finish(&mut presenter);
}

/// A window whose start has already passed when the presenter adopts it is marked late-adopted, and the window stays what was asked
/// for.
pub(crate) fn a_window_adopted_after_its_start_is_marked_late() {
    let mut harness = Harness::new();
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start();
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    let start = harness.now().saturating_sub(1_000_000);
    presenter
        .request_metrics(MetricsWindow {
            id: 1,
            start,
            end: None,
        })
        .expect("requested");
    harness
        .rig
        .wait("the adoption", || presenter.metrics_adopted(1).is_some());
    let reading = presenter.metrics_snapshot(1).expect("kept");
    assert!(reading.window.late_adopted, "{:?}", reading.window);
    assert_eq!(reading.window.window.start, start);
    harness.finish(&mut presenter);
}

/// A real frame gives one GPU sample after its recorded proof (no assertion on its size), where the device has timestamp bits.
pub(crate) fn a_real_frame_gives_a_gpu_sample_after_its_proof() {
    let mut harness = Harness::new();
    let caps = harness.context.timing_caps();
    assert!(
        caps.timestamp_valid_bits > 0,
        "this device has timestamps: {caps:?}"
    );
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start();
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    harness.rig.wait("a GPU sample", || {
        presenter.metrics_totals().gpu.count() >= 1
    });
    let totals = presenter.metrics_totals();
    assert_eq!(totals.invalid_gpu, 0, "{totals:?}");
    assert_eq!(presenter.metrics_unavailable().0, None);
    harness.finish(&mut presenter);
}

/// With timestamps forced off the reading says GPU time is unavailable and holds no zero sample, and nothing is owed.
pub(crate) fn with_timestamps_forced_off_gpu_time_is_unavailable_never_zero() {
    let mut harness = Harness::with_features(TestFeatures {
        events: true,
        without_timestamps: true,
        ..TestFeatures::default()
    });
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start();
    run_frames(&mut harness, Duration::from_millis(300));
    let totals = presenter.metrics_totals();
    assert!(totals.frames >= 3, "{totals:?}");
    assert_eq!(
        (totals.gpu.count(), totals.invalid_gpu),
        (0, 0),
        "no zero sample"
    );
    assert_eq!(totals.outstanding, 0);
    assert!(
        presenter
            .metrics_unavailable()
            .0
            .is_some_and(|reason| reason.contains("timestampValidBits 0")),
        "{:?}",
        presenter.metrics_unavailable()
    );
    harness.finish(&mut presenter);
}

/// With display timing advertised, consecutive presents give intervals; its absence only makes them unavailable (never a refusal).
pub(crate) fn display_timing_gives_intervals_between_consecutive_presents() {
    let mut harness = Harness::new();
    let advertised = harness.context.timing_caps().display_timing;
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start();
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    if advertised {
        let give_up = Instant::now() + BOUND;
        let mut step = 0_i32;
        while presenter.metrics_totals().presentation_interval.count() < 3 {
            assert!(
                Instant::now() < give_up,
                "no presentation interval: {:?}",
                presenter.metrics_totals()
            );
            step += 1;
            harness.publish(vec![quad(1, 10 + step % 40, 10, 100)]);
            harness.rig.pump();
            std::thread::sleep(Duration::from_millis(8));
        }
        assert_eq!(presenter.metrics_unavailable().1, None);
        let totals = presenter.metrics_totals();
        assert!(totals.presented >= 3, "{totals:?}");
    } else {
        assert!(
            presenter
                .metrics_unavailable()
                .1
                .is_some_and(|reason| reason.contains("VK_GOOGLE_display_timing")),
            "{:?}",
            presenter.metrics_unavailable()
        );
    }
    harness.finish(&mut presenter);
}

/// With the extension not enabled (the capability override), the reading says presentation is unavailable and nothing is pending:
/// a window completes at once, whatever the frames.
pub(crate) fn with_display_timing_off_presentation_is_unavailable_and_nothing_is_pending() {
    let mut harness = Harness::with_features(TestFeatures {
        events: true,
        without_display_timing: true,
        ..TestFeatures::default()
    });
    harness.publish(vec![quad(1, 10, 10, 100)]);
    let mut presenter = harness.start();
    harness.wait_stats(&presenter, "the first frame", |stats| stats.frames >= 1);
    let start = harness.now() + LEAD;
    presenter
        .request_metrics(MetricsWindow {
            id: 1,
            start,
            end: None,
        })
        .expect("requested");
    run_frames(&mut harness, Duration::from_millis(300));
    presenter.end_metrics(1, harness.now()).expect("ended");
    let reading = presenter.read_metrics(1, BOUND).expect("kept");
    assert!(reading.complete, "{reading:?}");
    assert!(
        reading
            .presentation_unavailable
            .is_some_and(|reason| reason.contains("VK_GOOGLE_display_timing")),
        "{reading:?}"
    );
    let totals = presenter.metrics_totals();
    assert_eq!(
        (
            totals.presentation_interval.count(),
            totals.presented,
            totals.not_presented,
            totals.presentation_expired
        ),
        (0, 0, 0, 0),
        "nothing is sampled, counted or expired without the extension"
    );
    harness.finish(&mut presenter);
}

/// A frame handed off before the window's end whose GPU result arrives after it is admitted to the window, and `read_metrics` waits
/// for it within its bound: held behind a host event, the reading is not complete; released, it completes with the frame's sample.
pub(crate) fn a_gpu_result_after_the_end_is_admitted_and_the_read_waits_for_it() {
    let mut harness = Harness::new();
    let held = event(&harness);
    let mut presenter = harness.start_with(Harness::limits(), None, harness.rig.pixel, |hooks| {
        hooks.scene.frame_event = Some(held);
    });
    let start = harness.now() + LEAD;
    presenter
        .request_metrics(MetricsWindow {
            id: 1,
            start,
            end: None,
        })
        .expect("requested");
    harness
        .rig
        .wait("the adoption", || presenter.metrics_adopted(1).is_some());
    let clock = std::sync::Arc::clone(&harness.clock);
    harness.rig.wait("the window to open", || clock() > start);
    harness.publish(vec![quad(1, 10, 10, 100)]);
    harness.rig.wait("the held frame to be handed off", || {
        count(&harness.probes.events(), |e| *e == Event::Submit) >= 1
            && presenter.metrics_totals().frames >= 1
    });
    presenter
        .end_metrics(1, harness.now())
        .expect("the window ends while the frame is held");
    let pending = presenter
        .read_metrics(1, Duration::from_millis(150))
        .expect("kept");
    assert!(
        !pending.complete,
        "the GPU result is outstanding: {pending:?}"
    );
    assert_eq!(pending.outstanding, 1);
    assert_eq!(pending.window.stats.frames, 1, "the frame is in the window");
    set_event(&harness, held);
    let done = presenter.read_metrics(1, BOUND).expect("kept");
    assert!(done.complete, "{done:?}");
    assert_eq!(
        done.window.stats.gpu.count(),
        1,
        "admitted by the frame's own hand-off"
    );
    presenter.request_stop();
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    destroy_event(&harness, held);
    drop(presenter);
    let Harness { context, rig, .. } = harness;
    drop(context);
    rig.close(&ended);
}
