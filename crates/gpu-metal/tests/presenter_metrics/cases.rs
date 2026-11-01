//! The presenter's frame-timing windows (CK-129, milestone 5 issue 10b-1), on a real window.
//!
//! Each case starts a presenter on the rig's layer, asks it for one measurement window, draws frames inside it and compares what the
//! window recorded with what the test saw itself: the frames' own six marks (`TestHooks::frame_marks`), the commit mark that decides
//! which window a frame belongs to, and timing results the test replaced through `TestHooks::timing_override`. Nothing here asserts
//! how long anything takes: the assertions are exact relations between timestamps taken in the same run (see
//! `tests/presenter.rs` for how a case runs: in a worker, under Metal validation, twice, once with the owner typing).

use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ck_gpu_metal::{
    FrameMarks, MarksProbe, MetricsReading, MetricsWindow, Percentile, RequestError, TimingProbe,
    now_ns,
};

use super::{
    BOUND, CleanupResult, Duration, MetalPresenter, Probes, Rig, TestHooks, ck_t_rig_ever_occluded,
    ck_t_rig_occlusion_state, ck_t_watch_rig_occlusion, end,
};

#[path = "settle.rs"]
mod settle;
use settle::{Facts, Settled, Window};

const MS: u64 = 1_000_000;

/// The marks of every committed frame, in order.
type Marks = Arc<Mutex<Vec<FrameMarks>>>;

fn marks_hooks(probes: &Probes) -> (TestHooks, Marks) {
    let marks: Marks = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&marks);
    let hooks = TestHooks {
        frame_marks: Some(MarksProbe(Arc::new(move |frame| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(*frame);
        }))),
        ..probes.hooks()
    };
    (hooks, marks)
}

fn marks_of(marks: &Marks) -> Vec<FrameMarks> {
    marks.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Requests window 1, `[now + lead, open)`, and pumps until the render thread adopted it and the clock reached its start.
fn open_window(rig: &mut Rig, presenter: &MetalPresenter, lead: u64) -> MetricsWindow {
    let window = MetricsWindow {
        id: 1,
        start: now_ns() + lead * MS,
        end: None,
    };
    presenter
        .request_metrics(window)
        .expect("the request is accepted");
    assert!(
        rig.pump_until(BOUND, || presenter.metrics_adopted(1).is_some()),
        "the render thread never adopted the window"
    );
    assert!(rig.pump_until(BOUND, || now_ns() >= window.start));
    window
}

/// Draws `count` more frames, one at a time (one frame in flight), each with a colour of its own.
fn draw(rig: &mut Rig, presenter: &MetalPresenter, probes: &Probes, count: u64) {
    for step in 0..count {
        let committed = probes.frames();
        #[expect(
            clippy::cast_precision_loss,
            reason = "a small step number becomes a colour component"
        )]
        let shade = (step % 16) as f32 / 16.0;
        presenter.set_clear([shade, 0.4, 0.4]);
        presenter.request_redraw();
        assert!(
            rig.pump_until(BOUND, || probes.frames() > committed),
            "frame {step} was never committed"
        );
    }
}

/// Installs the watch that latches, for the rest of the worker, whether the rig window is ever seen without the visible bit (CK-171). An
/// occluded window shows its drawables with a `presentedTime` of zero, which closes nothing, so a reading whose closing drawable was
/// never shown cannot become final. The latch is set by `NSWindowDidChangeOcclusionStateNotification` (and by the state at the time the
/// watch is installed), not by polling, so a window covered for any length of time after this call is retained. Idempotent. Main thread.
fn watch_occlusion() {
    // SAFETY: main thread (the worker's); registers one block observer on the rig's window, which may be absent.
    unsafe { ck_t_watch_rig_occlusion() };
}

fn ever_occluded() -> bool {
    // SAFETY: reads an atomic flag.
    unsafe { ck_t_rig_ever_occluded() != 0 }
}

/// The window's `occlusionState` now, for a failure message (-1: no window).
fn occlusion_now() -> i64 {
    // SAFETY: main thread; reads one property of the rig's window, which may be absent.
    unsafe { ck_t_rig_occlusion_state() }
}

/// Pumps until the window's reading is final, which is the product's own condition (`read_metrics(..).complete`: past the end, every
/// step recorded, every pre-end presentation consumed, nothing outstanding, the closing interval known). The one thing it cannot wait
/// for is the closing interval of an occluded window, whose closing drawable has a `presentedTime` of zero: when the window has ever
/// been seen occluded and the reading is not final only because that interval is open (`closing_pending`, `outstanding == 0`), the
/// case **ends the presenter** (`Proven`; the teardown proof accounts for every frame and the end resolves the interval) and takes the
/// reading kept for the window after the end, which is final and exact; the flag says so, and the presenter is then already ended
/// (use [`stop_settled`]). The poll's reading is never used as that reading, because it cannot show the owner's own progress
/// (`accounted_through`). A window never seen occluded keeps the full assertion and fails at the bound.
fn settle(rig: &mut Rig, presenter: &mut MetalPresenter) -> (MetricsReading, bool) {
    let mut reading = None;
    let mut outcome = None;
    let settled = rig.pump_until(BOUND, || {
        reading = presenter.read_metrics(1, Duration::ZERO);
        outcome = reading.as_ref().and_then(|reading| {
            match settle::settled(&Facts {
                complete: reading.complete,
                closing_pending: reading.closing_pending,
                outstanding: reading.outstanding,
                window: if ever_occluded() {
                    Window::SeenOccluded
                } else {
                    Window::AlwaysVisible
                },
            }) {
                Settled::Final => Some(false),
                Settled::Occluded => Some(true),
                Settled::Waiting => None,
            }
        });
        outcome.is_some()
    });
    assert!(
        settled,
        "never final: {} | occlusion={} ever_occluded={}",
        describe(reading.as_ref(), presenter),
        occlusion_now(),
        ever_occluded()
    );
    if outcome == Some(true) {
        assert_eq!(end(rig, presenter), CleanupResult::Proven);
        let ended = presenter.read_metrics(1, Duration::ZERO).expect("kept");
        assert!(
            ended.complete && !ended.closing_pending,
            "the presenter's end resolves the closing interval: {ended:?}"
        );
        return (ended, true);
    }
    (reading.expect("a reading"), false)
}

/// The few figures of a reading that say why it is not final.
fn describe(reading: Option<&MetricsReading>, presenter: &MetalPresenter) -> String {
    let totals = presenter.metrics_totals();
    reading.map_or_else(
        || "no reading".to_owned(),
        |reading| {
            let stats = &reading.window.stats;
            format!(
                "complete={} closing_pending={} outstanding={} frames={} gpu={}+{} invalid presented={} not_presented={} end={:?} adopted={:?} outside={} | run: frames={} gpu={} presented={}",
                reading.complete,
                reading.closing_pending,
                reading.outstanding,
                stats.frames,
                stats.gpu.count(),
                stats.invalid_gpu,
                stats.presented,
                stats.not_presented,
                reading.window.window.end,
                reading.window.adopted_at,
                reading.window.outside,
                totals.frames,
                totals.gpu.count() + totals.invalid_gpu,
                totals.presented + totals.not_presented,
            )
        },
    )
}

/// Ends window 1 at the clock now, draws one more frame (a drawable shown after the end closes the window's last interval) and pumps
/// until the presenter's reading is final, or the window is occluded and the presenter was ended to resolve it (see [`settle`]; the
/// flag says which).
fn finish(
    rig: &mut Rig,
    presenter: &mut MetalPresenter,
    probes: &Probes,
) -> (u64, MetricsReading, bool) {
    watch_occlusion();
    let ended = now_ns();
    presenter
        .end_metrics(1, ended)
        .expect("the end is accepted once");
    draw(rig, presenter, probes, 1);
    let (reading, occluded) = settle(rig, presenter);
    (ended, reading, occluded)
}

fn stop(rig: &mut Rig, mut presenter: MetalPresenter) {
    assert_eq!(end(rig, &mut presenter), CleanupResult::Proven);
}

/// [`stop`] for a case whose reading came from [`settle`] or [`finish`]: when that ended the presenter already (the occluded outcome),
/// there is nothing more to end.
fn stop_settled(rig: &mut Rig, mut presenter: MetalPresenter, ended_already: bool) {
    if !ended_already {
        assert_eq!(end(rig, &mut presenter), CleanupResult::Proven);
    }
}

/// The frames whose commit mark lies in `[start, end)`.
fn inside(marks: &[FrameMarks], window: &MetricsWindow, ended: u64) -> Vec<FrameMarks> {
    marks
        .iter()
        .copied()
        .filter(|frame| frame.t5 >= window.start && frame.t5 < ended)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------------------------

/// The six marks are taken in order on every committed frame, the components sum to the total exactly, and the window holds exactly
/// the frames whose commit mark lies inside it, each with its GPU time and its presentation accounted for.
pub(super) fn metrics_marks_are_ordered_and_the_window_holds_exactly_its_frames() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let (hooks, marks) = marks_hooks(&probes);
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let window = open_window(&mut rig, &presenter, 200);
    // A second request while the first window is still open is refused and changes nothing.
    assert_eq!(
        presenter.request_metrics(MetricsWindow {
            id: 2,
            start: now_ns() + 50 * MS,
            end: None
        }),
        Err(RequestError::Overlaps)
    );
    draw(&mut rig, &presenter, &probes, 20);
    let (ended, reading, occluded) = finish(&mut rig, &mut presenter, &probes);
    let marks = marks_of(&marks);
    let mine = inside(&marks, &window, ended);
    assert!(!mine.is_empty());
    for frame in &marks {
        let ordered = [frame.t0, frame.t1, frame.t2, frame.t3, frame.t4, frame.t5];
        assert!(
            ordered.iter().all(|mark| *mark != 0)
                && ordered.windows(2).all(|pair| pair[0] <= pair[1]),
            "the marks are in order: {frame:?}"
        );
        let intervals = frame.intervals().expect("valid marks give intervals");
        assert_eq!(
            intervals.ledger + intervals.build + intervals.encode,
            intervals.cpu_total,
            "the components sum to the total exactly"
        );
        assert_eq!(
            intervals.cpu_total,
            (frame.t3 - frame.t0) + (frame.t5 - frame.t4),
            "the total excludes the acquisition and nothing else"
        );
        assert_eq!(
            intervals.cpu_total + intervals.acquire,
            frame.t5 - frame.t0,
            "the total and the acquisition are the whole frame"
        );
    }
    let stats = &reading.window.stats;
    assert_eq!(
        stats.frames,
        mine.len() as u64,
        "exactly the frames committed inside the window"
    );
    assert_eq!(stats.cpu_total.count(), stats.frames);
    assert_eq!(stats.acquire.count(), stats.frames);
    assert_eq!((stats.invalid_marks, stats.skipped), (0, 0));
    assert_eq!(
        stats.gpu.count() + stats.invalid_gpu,
        stats.frames,
        "every GPU result is accounted for"
    );
    assert_eq!(
        stats.presented + stats.not_presented,
        stats.frames,
        "every drawable is accounted for"
    );
    assert_eq!(reading.outstanding, 0);
    // Final: as the poll found it, or (an occluded window's closing interval cannot close) after the presenter's end.
    assert!(reading.complete && !reading.closing_pending, "{reading:?}");
    assert!(matches!(
        stats.cpu_total.percentile(950),
        Percentile::Value { .. }
    ));
    assert_eq!(
        reading.window.window.end,
        Some(ended),
        "the reported duration is end - start"
    );
    assert!(!reading.window.late_adopted);
    assert_eq!(reading.timing_lost, 0);
    // The whole run holds at least the window's frames.
    assert!(presenter.metrics_totals().frames >= stats.frames);
    stop_settled(&mut rig, presenter, occluded);
}

/// Frames that committed nothing are counted and never sampled (the whole run's totals, which the first, start-up steps belong to).
pub(super) fn metrics_skipped_frames_are_counted_and_not_sampled() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let (mut hooks, _marks) = marks_hooks(&probes);
    hooks.skipped_frames = 3;
    let presenter = probes.start_ok(&rig, hooks);
    // The first three steps skip; keep asking until three were counted and a frame was drawn after them.
    assert!(rig.pump_until(BOUND, || {
        presenter.request_redraw();
        let totals = presenter.metrics_totals();
        totals.skipped >= 3 && totals.frames >= 1
    }));
    let totals = presenter.metrics_totals();
    assert_eq!(totals.skipped, 3, "exactly the frames the hook skipped");
    assert_eq!(
        totals.cpu_total.count(),
        totals.frames,
        "a skipped frame has no interval"
    );
    assert_eq!(totals.acquire.count(), totals.frames);
    assert_eq!(totals.invalid_marks, 0);
    stop(&mut rig, presenter);
}

/// A blocked `nextDrawable` adds to the acquire interval only: within one frame, the total plus the acquisition is the whole frame, and
/// the acquisition is at least as long as the hold. Two runs are never compared.
pub(super) fn metrics_a_blocked_acquisition_adds_to_acquire_only() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let (mut hooks, marks) = marks_hooks(&probes);
    let hold = Arc::new(AtomicI32::new(0));
    hooks.hold_in_acquire = Some(Arc::clone(&hold));
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let window = open_window(&mut rig, &presenter, 100);
    draw(&mut rig, &presenter, &probes, 3);
    // The next frame is held inside its acquisition for a while.
    let before = marks_of(&marks).len();
    hold.store(1, Ordering::SeqCst);
    let committed = probes.frames();
    presenter.set_clear([0.9, 0.2, 0.2]);
    presenter.request_redraw();
    rig.pump_for(Duration::from_millis(300));
    assert_eq!(
        probes.frames(),
        committed,
        "the held frame is still in its acquisition"
    );
    let released = now_ns();
    hold.store(0, Ordering::SeqCst);
    assert!(rig.pump_until(BOUND, || probes.frames() > committed));
    let (ended, reading, occluded) = finish(&mut rig, &mut presenter, &probes);
    let blocked = marks_of(&marks)[before];
    let intervals = blocked.intervals().expect("valid marks");
    assert!(
        intervals.acquire >= released - blocked.t3,
        "the acquisition lasted until the hold was released: {intervals:?} released at {released}"
    );
    assert!(
        released - blocked.t3 > intervals.cpu_total,
        "the hold outlasted the whole CPU total, so the case exercises the exclusion: {intervals:?}"
    );
    assert_eq!(
        intervals.cpu_total + intervals.acquire,
        blocked.t5 - blocked.t0,
        "the total excludes exactly the acquisition"
    );
    let stats = &reading.window.stats;
    assert!(stats.acquire.max_ns().expect("sampled") >= intervals.acquire);
    assert!(
        stats.cpu_total.max_ns().expect("sampled") < intervals.acquire,
        "no frame's CPU total includes the hold"
    );
    assert_eq!(
        inside(&marks_of(&marks), &window, ended).len() as u64,
        stats.frames
    );
    stop_settled(&mut rig, presenter, occluded);
}

/// The timestamp rules: an inverted GPU pair is invalid and not sampled; a drawable that was never shown is not presented and makes
/// no interval; a presentation interval is recorded only between shown drawables inside the window, and the one that crosses the
/// window's start or its end is counted as excluded.
pub(super) fn metrics_gpu_and_presentation_follow_their_rules() {
    const FRAMES: u64 = 12;
    const STEP: u64 = 16 * MS;
    let mut rig = Rig::open();
    let probes = Probes::new();
    let start = now_ns() + 300 * MS;
    let base = start + MS;
    let seen = Arc::new(AtomicU64::new(0));
    let (mut hooks, _marks) = marks_hooks(&probes);
    // The results of frames committed inside the window are replaced; frame `j`'s GPU pair is inverted for j = 0, and its drawable
    // is shown at base + j * 16 ms unless j % 3 == 2 (never shown). Results before the window are untouched.
    let (gpu_seen, shown_seen) = (Arc::new(AtomicU64::new(0)), Arc::clone(&seen));
    let gpu_counter = Arc::clone(&gpu_seen);
    hooks.timing_override = Some(TimingProbe(Arc::new(move |kind, t5, a, b| {
        if t5 < start {
            // Before the window: a drawable shown just before it opens, so the first interval inside crosses the start.
            return if kind == 2 {
                (start - 5 * MS, b)
            } else {
                (a, b)
            };
        }
        // The frame drawn after the window's end (index FRAMES) closes the last interval; its GPU result stays real.
        if kind == 1 {
            let j = gpu_counter.fetch_add(1, Ordering::SeqCst);
            if j == 0 { (1_000, 500) } else { (a, b) }
        } else {
            let j = shown_seen.fetch_add(1, Ordering::SeqCst);
            if j >= FRAMES {
                // The frame drawn after the end: shown long after it, whatever the window's visibility (a real `presentedTime` is
                // zero when the window is occluded, and a zero closes nothing).
                (base + 100_000 * MS, b)
            } else if j % 3 == 2 {
                (0, b)
            } else {
                (base + j * STEP, b)
            }
        }
    })));
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let window = MetricsWindow {
        id: 1,
        start,
        end: None,
    };
    presenter.request_metrics(window).expect("accepted");
    assert!(rig.pump_until(BOUND, || presenter.metrics_adopted(1).is_some()));
    assert!(rig.pump_until(BOUND, || now_ns() >= start));
    draw(&mut rig, &presenter, &probes, FRAMES);
    // The window must outlast every replaced presentation time.
    assert!(rig.pump_until(BOUND, || now_ns() > base + (FRAMES + 1) * STEP));
    let (_, reading, occluded) = finish(&mut rig, &mut presenter, &probes);
    let stats = &reading.window.stats;
    assert_eq!(stats.frames, FRAMES);
    let unshown = FRAMES / 3;
    assert_eq!(stats.invalid_gpu, 1, "the inverted pair is counted invalid");
    assert_eq!(stats.gpu.count(), FRAMES - 1, "and not sampled");
    assert_eq!(stats.not_presented, unshown);
    assert_eq!(stats.presented, FRAMES - unshown);
    // Intervals between shown drawables only: the first shown drawable's predecessor is a real one from before the window, so that
    // interval crosses the start and is excluded; the rest are recorded, and the longest spans one unshown drawable.
    assert_eq!(stats.presentation_interval.count(), stats.presented - 1);
    assert_eq!(
        stats.excluded_intervals, 2,
        "one interval crosses the start (from the real drawable before the window) and one the end (to the drawable shown after it)"
    );
    assert_eq!(stats.presentation_interval.max_ns(), Some(2 * STEP));
    assert_eq!(reading.outstanding, 0);
    stop_settled(&mut rig, presenter, occluded);
}

/// An adoption delayed past `start` is late-adopted, and nothing is adopted while it is held.
pub(super) fn metrics_an_adoption_delayed_past_start_is_late_adopted() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let hold = Arc::new(AtomicI32::new(1));
    let hooks = TestHooks {
        hold_metrics_adoption: Some(Arc::clone(&hold)),
        ..probes.hooks()
    };
    let presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let window = MetricsWindow {
        id: 1,
        start: now_ns() + 100 * MS,
        end: None,
    };
    presenter.request_metrics(window).expect("accepted");
    // Held: the clock passes the start and nothing is adopted.
    assert!(rig.pump_until(BOUND, || now_ns() > window.start + 100 * MS));
    assert_eq!(presenter.metrics_adopted(1), None, "the adoption is held");
    hold.store(0, Ordering::SeqCst);
    assert!(rig.pump_until(BOUND, || presenter.metrics_adopted(1).is_some()));
    let adopted = presenter.metrics_adopted(1).expect("adopted");
    assert!(adopted > window.start, "adopted after the window opened");
    let reading = presenter.metrics_snapshot(1).expect("kept");
    assert!(
        reading.window.late_adopted,
        "the window is flagged, not silently short"
    );
    stop(&mut rig, presenter);
}

/// A frame committed before the end whose GPU time and presentation arrive after the end is admitted to its window, and `read_metrics`
/// waits for it: the reading is not final while the results are outstanding, and exact once they arrive.
pub(super) fn metrics_results_arriving_after_the_end_belong_to_their_frame() {
    results_after_the_end(false);
}

/// The same, with the thread never polling for completion errors: the only look at the tokens is the one that decides whether it may
/// sleep, so the results that look consumes reach the window only if the thread moves them in before it sleeps. (Found by a
/// full-check failure, about 1 run in 75: the sleep decision consumed the GPU result, said nothing was outstanding, and the thread
/// slept on it until the next wake.)
pub(super) fn metrics_results_consumed_by_the_sleep_decision_reach_the_window() {
    results_after_the_end(true);
}

fn results_after_the_end(never_poll: bool) {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let gate = Arc::new(AtomicI32::new(1));
    let (mut hooks, marks) = marks_hooks(&probes);
    hooks.frame_completion_gate = Some(Arc::clone(&gate));
    if never_poll {
        hooks.error_poll = super::ErrorPoll::Never;
    }
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    // The start-up frame's handlers have all run, so the gate only touches the next frame.
    assert!(rig.pump_until(BOUND, || { probes.observer.completion_handlers_run() >= 1 }));
    let window = open_window(&mut rig, &presenter, 100);
    gate.store(0, Ordering::SeqCst);
    let committed = probes.frames();
    presenter.set_clear([0.3, 0.7, 0.2]);
    presenter.request_redraw();
    assert!(rig.pump_until(BOUND, || probes.frames() > committed));
    let ended = now_ns();
    presenter.end_metrics(1, ended).expect("accepted");
    // The frame is committed but its completion is held: the reading cannot be final.
    rig.pump_for(Duration::from_millis(100));
    let early = presenter.read_metrics(1, Duration::ZERO).expect("kept");
    assert!(!early.complete, "a result is still outstanding");
    assert!(early.outstanding >= 1);
    gate.store(1, Ordering::SeqCst);
    // A frame drawn after the end is shown after it: the drawable that closes the window's last interval.
    watch_occlusion();
    presenter.set_clear([0.6, 0.1, 0.1]);
    presenter.request_redraw();
    let (reading, occluded) = settle(&mut rig, &mut presenter);
    let mine = inside(&marks_of(&marks), &window, ended);
    let stats = &reading.window.stats;
    assert_eq!(stats.frames, mine.len() as u64);
    assert_eq!(stats.frames, 1, "the one frame committed before the end");
    assert_eq!(
        stats.gpu.count() + stats.invalid_gpu,
        1,
        "its GPU time arrived after the end and counts"
    );
    assert_eq!(stats.presented + stats.not_presented, 1);
    assert_eq!(reading.outstanding, 0);
    stop_settled(&mut rig, presenter, occluded);
}

/// The results of the last frames arrive through the teardown: stopping right after the last frame, the presenter's proof waits for
/// every token and the recorder accounts for every frame's GPU time and presentation before the presenter ends.
pub(super) fn metrics_the_last_frames_results_arrive_through_the_teardown() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let presenter = probes.start_ok(&rig, probes.hooks());
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    draw(&mut rig, &presenter, &probes, 5);
    // No pause: the stop finds the last frame's results possibly still on their way.
    let mut presenter = presenter;
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    let totals = presenter.metrics_totals();
    assert!(totals.frames >= 6);
    assert_eq!(
        totals.gpu.count() + totals.invalid_gpu,
        totals.frames,
        "every frame's GPU result was consumed before the end"
    );
    assert_eq!(
        totals.presented + totals.not_presented,
        totals.frames,
        "every frame's drawable result was consumed before the end"
    );
}

/// The owner's progress fence (CK-129 review F2): a frame committed before the window's end whose step has not yet been recorded keeps
/// the window open, however long the clock has been past the end; once the thread records it, the count is exact.
pub(super) fn metrics_a_committed_frame_not_yet_registered_keeps_the_window_open() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let (mut hooks, marks) = marks_hooks(&probes);
    let pause = Arc::new(AtomicI32::new(0));
    hooks.hold_before_record = Some(Arc::clone(&pause));
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let window = open_window(&mut rig, &presenter, 100);
    draw(&mut rig, &presenter, &probes, 2);
    // Everything the first two frames owe has arrived and been consumed, so that the only thing that can keep the window open below
    // is the step in flight (and not an earlier frame's result the paused thread could not consume).
    assert!(
        rig.pump_until(BOUND, || presenter
            .metrics_snapshot(1)
            .is_some_and(|reading| reading.outstanding == 0)),
        "the first two frames' results were never consumed"
    );
    // The next frame commits and the thread stops before it records it.
    pause.store(1, Ordering::SeqCst);
    let committed = probes.observer.committed();
    presenter.set_clear([0.8, 0.8, 0.2]);
    presenter.request_redraw();
    assert!(rig.pump_until(BOUND, || probes.observer.committed() > committed));
    let ended = now_ns();
    presenter.end_metrics(1, ended).expect("accepted");
    rig.pump_for(Duration::from_millis(60));
    // Past the end on the clock, nothing outstanding that is recorded, and still not final: the step is in flight.
    let early = presenter.read_metrics(1, Duration::ZERO).expect("kept");
    assert!(
        !early.complete,
        "a step that began before the end is unrecorded: {early:?}"
    );
    assert_eq!(
        early.window.stats.frames, 2,
        "the held frame is not registered yet"
    );
    pause.store(0, Ordering::SeqCst);
    // A frame drawn after the end is shown after it: the drawable that closes the window's last interval.
    watch_occlusion();
    presenter.set_clear([0.1, 0.6, 0.1]);
    presenter.request_redraw();
    let (reading, occluded) = settle(&mut rig, &mut presenter);
    assert_eq!(
        reading.window.stats.frames, 3,
        "the held frame was committed before the end"
    );
    assert_eq!(
        inside(&marks_of(&marks), &window, ended).len() as u64,
        reading.window.stats.frames
    );
    assert_eq!(reading.outstanding, 0);
    stop_settled(&mut rig, presenter, occluded);
}

/// A window is not final while work committed **before** it still has its presentation to come (CK-129 review 2, F1). Three drawables
/// are committed before the window opens (the start-up frame and two more) and their presentation results are kept back; their shown
/// times are replaced by times inside the window (1, 6 and 11 ms after its start). At the first read, after the window's end, no
/// frame of the window owes anything and no step is in flight, yet the reading must not be final; once the results are released the
/// window holds the two 5 ms intervals between them, exactly.
pub(super) fn metrics_a_window_waits_for_the_presentations_of_work_committed_before_it() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let start = now_ns() + 700 * MS;
    let pre_start_results = Arc::new(AtomicU64::new(0));
    let hold = Arc::new(AtomicI32::new(1));
    let (mut hooks, _marks) = marks_hooks(&probes);
    hooks.hold_presentations = Some(Arc::clone(&hold));
    let seen_before_start = Arc::clone(&pre_start_results);
    hooks.timing_override = Some(TimingProbe(Arc::new(move |kind, t5, a, b| {
        if kind == 2 && t5 < start {
            let index = seen_before_start.fetch_add(1, Ordering::SeqCst);
            (start + MS + index * 5 * MS, b)
        } else if kind == 2 {
            // A frame committed after the window opened is the redraw after its end: shown long after it, whatever the window's
            // visibility (a real `presentedTime` is zero when the window is occluded, and a zero closes nothing).
            (start + 100_000 * MS, b)
        } else {
            (a, b)
        }
    })));
    let presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    presenter
        .request_metrics(MetricsWindow {
            id: 1,
            start,
            end: None,
        })
        .expect("accepted");
    assert!(rig.pump_until(BOUND, || presenter.metrics_adopted(1).is_some()));
    draw(&mut rig, &presenter, &probes, 2);
    assert!(
        now_ns() < start,
        "the three frames must be committed before the window opens"
    );
    assert!(rig.pump_until(BOUND, || now_ns() > start + 30 * MS));
    let ended = now_ns();
    presenter.end_metrics(1, ended).expect("accepted");
    rig.pump_for(Duration::from_millis(60));
    let first = presenter.read_metrics(1, Duration::ZERO).expect("kept");
    assert_eq!(
        first.window.stats.frames, 0,
        "every frame was committed before the window"
    );
    assert_eq!(first.outstanding, 0, "and the window itself owes nothing");
    assert!(
        !first.complete,
        "three presentations committed before the window are still to come: {first:?}"
    );
    assert_eq!(first.window.stats.presentation_interval.count(), 0);
    // Release them; the thread records them at its next look, which a redraw (a frame after the end, outside the window) brings.
    hold.store(0, Ordering::SeqCst);
    presenter.set_clear([0.5, 0.5, 0.5]);
    presenter.request_redraw();
    let mut reading = None;
    let final_ = rig.pump_until(BOUND, || {
        reading = presenter.read_metrics(1, Duration::ZERO);
        reading.as_ref().is_some_and(|reading| reading.complete)
    });
    assert!(
        final_,
        "never final: {}",
        describe(reading.as_ref(), &presenter)
    );
    let reading = reading.expect("a reading");
    let stats = &reading.window.stats;
    let before = pre_start_results.load(Ordering::SeqCst);
    assert!(
        before >= 3,
        "the start-up frame and the two drawn ones: {before}"
    );
    assert_eq!(
        stats.presentation_interval.count(),
        before - 1,
        "one 5 ms interval between each consecutive pair of the frames committed before the window"
    );
    assert_eq!(stats.presentation_interval.max_ns(), Some(5 * MS));
    assert_eq!(
        stats.excluded_intervals, 1,
        "none crosses the start; the one from the last of them to the drawable shown after the end crosses the end"
    );
    assert_eq!((stats.frames, stats.presented), (0, 0));
    assert!(!reading.evidence_lost);
    stop(&mut rig, presenter);
}

/// A render thread descheduled right after a submission reads its commit mark late, after the window's end, while the drawable is
/// already shown inside the window (CK-129 review 3, F2). The native hook holds the thread between `commit` and the mark; the
/// frame's presentation happens meanwhile and is kept back from the recorder. The interval it closes (from the last drawable shown
/// before it) lies inside the window and must be in the final reading, exactly as the shown times say, whatever the commit mark.
#[expect(
    clippy::too_many_lines,
    reason = "one scenario: the schedule, the two holds, the end, the late mark and the exact comparison read best together"
)]
pub(super) fn metrics_a_late_commit_stamp_does_not_hide_a_presentation_inside_the_window() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let start = now_ns() + 300 * MS;
    let shown: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let after_commit = Arc::new(AtomicI32::new(0));
    let held = Arc::new(AtomicI32::new(0));
    let ended_at = Arc::new(AtomicU64::new(0));
    let in_window = Arc::new(AtomicU64::new(0));
    let after_end = Arc::new(AtomicU64::new(0));
    let (mut hooks, marks) = marks_hooks(&probes);
    hooks.hold_after_commit = Some(Arc::clone(&after_commit));
    hooks.hold_presentations = Some(Arc::clone(&held));
    // The shown times are replaced, so that the case does not depend on the window being visible: the start-up frame is shown 5 ms
    // before the window opens, each frame committed inside it 10 ms, 20 ... after its start; once the window has ended, the first
    // frame committed (the held one, whose commit mark is late) is shown 5 ms BEFORE the end and the next one 5 ms after it.
    let (record, ended_seen, inside, outside) = (
        Arc::clone(&shown),
        Arc::clone(&ended_at),
        Arc::clone(&in_window),
        Arc::clone(&after_end),
    );
    hooks.timing_override = Some(TimingProbe(Arc::new(move |kind, t5, a, b| {
        if kind != 2 {
            return (a, b);
        }
        let ended = ended_seen.load(Ordering::SeqCst);
        let at = if t5 < start {
            start - 5 * MS
        } else if ended == 0 {
            start + 10 * MS * (inside.fetch_add(1, Ordering::SeqCst) + 1)
        } else if outside.fetch_add(1, Ordering::SeqCst) == 0 {
            ended - 5 * MS
        } else {
            ended + 5 * MS
        };
        record
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(at);
        (at, b)
    })));
    let presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    presenter
        .request_metrics(MetricsWindow {
            id: 1,
            start,
            end: None,
        })
        .expect("accepted");
    assert!(rig.pump_until(BOUND, || presenter.metrics_adopted(1).is_some()));
    assert!(rig.pump_until(BOUND, || now_ns() >= start));
    draw(&mut rig, &presenter, &probes, 2);
    // The shown times of the start-up frame and of the two frames drawn inside the window are fixed before anything else moves:
    // `draw` waits for the commits only, and the override reads the window's end when a result is APPLIED, so a result still
    // unconsumed when the holds are armed (or the end is set) would be given a time around the end instead. The render thread
    // polls its outstanding results while it waits, so the three pass through the override without another frame.
    assert!(
        rig.pump_until(BOUND, || shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
            >= 3),
        "the three earlier presentations never passed through the override: {:?}",
        shown.lock().unwrap_or_else(PoisonError::into_inner)
    );
    // Frame X: its commit is submitted, then the thread is held before it reads its commit mark; its presentation is kept back from
    // the recorder.
    held.store(1, Ordering::SeqCst);
    after_commit.store(1, Ordering::SeqCst);
    let presented_before = probes.observer.presented_handlers_run();
    presenter.set_clear([0.9, 0.9, 0.1]);
    presenter.request_redraw();
    // The frame is submitted while the thread is held (the observer counts the presented handler, not the thread's own bookkeeping,
    // which waits behind the hold).
    assert!(
        rig.pump_until(BOUND, || probes.observer.presented_handlers_run()
            > presented_before)
    );
    rig.pump_for(Duration::from_millis(30));
    let ended = now_ns();
    assert!(
        ended > start + 30 * MS,
        "the window must outlast the frames inside it"
    );
    presenter.end_metrics(1, ended).expect("accepted");
    ended_at.store(ended, Ordering::SeqCst);
    // The thread reads its commit mark now: after the window's end, though the frame was submitted before it.
    let frames = probes.frames();
    after_commit.store(0, Ordering::SeqCst);
    assert!(rig.pump_until(BOUND, || probes.frames() > frames));
    let x = *marks_of(&marks).last().expect("the held frame");
    assert!(
        x.t5 >= ended,
        "the commit mark is late: {x:?} ended {ended}"
    );
    // Let its presentation, and a closing frame's, be recorded.
    held.store(0, Ordering::SeqCst);
    draw(&mut rig, &presenter, &probes, 1);
    let mut reading = None;
    let final_ = rig.pump_until(BOUND, || {
        reading = presenter.read_metrics(1, Duration::ZERO);
        reading.as_ref().is_some_and(|reading| reading.complete)
    });
    assert!(
        final_,
        "never final: {}",
        describe(reading.as_ref(), &presenter)
    );
    let reading = reading.expect("a reading");
    let shown = shown.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let inside = |at: u64| at >= start && at < ended;
    let expected = shown
        .windows(2)
        .filter(|pair| inside(pair[0]) && inside(pair[1]))
        .count() as u64;
    let crossing = shown
        .windows(2)
        .filter(|pair| inside(pair[0]) != inside(pair[1]))
        .count() as u64;
    assert!(
        expected >= 1 && crossing >= 2,
        "the schedule has start- and end-crossing intervals and inside ones: {shown:?}"
    );
    let stats = &reading.window.stats;
    assert_eq!(
        stats.presentation_interval.count(),
        expected,
        "every interval between two drawables shown inside the window, the one closed by the held frame (committed late) among them: {shown:?}"
    );
    assert_eq!(
        stats.excluded_intervals, crossing,
        "and every one that crosses its start or end"
    );
    stop(&mut rig, presenter);
}

/// The step is not over until the results of the frame it recorded are in the windows (CK-129 review 3): held between the recording
/// and the drain, a frame committed before the window's end keeps the reading incomplete; once the thread drains, it is exact.
pub(super) fn metrics_a_recorded_frame_keeps_the_window_open_until_its_results_are_drained() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let (mut hooks, marks) = marks_hooks(&probes);
    let before_drain = Arc::new(AtomicI32::new(0));
    hooks.hold_before_drain = Some(Arc::clone(&before_drain));
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    let window = open_window(&mut rig, &presenter, 100);
    draw(&mut rig, &presenter, &probes, 1);
    before_drain.store(1, Ordering::SeqCst);
    let committed = probes.observer.committed();
    presenter.set_clear([0.2, 0.6, 0.9]);
    presenter.request_redraw();
    assert!(rig.pump_until(BOUND, || probes.observer.committed() > committed));
    rig.pump_for(Duration::from_millis(80));
    let ended = now_ns();
    presenter.end_metrics(1, ended).expect("accepted");
    rig.pump_for(Duration::from_millis(60));
    let held = presenter.read_metrics(1, Duration::ZERO).expect("kept");
    assert!(
        !held.complete,
        "a frame committed before the end is recorded but its results are not in the windows yet: {held:?}"
    );
    before_drain.store(0, Ordering::SeqCst);
    watch_occlusion();
    presenter.set_clear([0.9, 0.6, 0.2]);
    presenter.request_redraw();
    let (reading, occluded) = settle(&mut rig, &mut presenter);
    assert_eq!(reading.outstanding, 0);
    assert_eq!(
        reading.window.stats.frames,
        inside(&marks_of(&marks), &window, ended).len() as u64
    );
    stop_settled(&mut rig, presenter, occluded);
}

/// The control for the closing rule (CK-129 review 4): a closing drawable that is never shown (a real `presentedTime` of zero, as
/// when the window is occluded) closes nothing, so the reading stays not final and says why; the presenter ending resolves it.
pub(super) fn metrics_a_zero_closing_presentation_leaves_the_reading_not_final() {
    let mut rig = Rig::open();
    let probes = Probes::new();
    let start = now_ns() + 300 * MS;
    let ended_at = Arc::new(AtomicU64::new(0));
    let inside = Arc::new(AtomicU64::new(0));
    let (mut hooks, _marks) = marks_hooks(&probes);
    let (ended_seen, counter) = (Arc::clone(&ended_at), Arc::clone(&inside));
    hooks.timing_override = Some(TimingProbe(Arc::new(move |kind, t5, a, b| {
        if kind != 2 {
            return (a, b);
        }
        if t5 < start {
            (start - 5 * MS, b)
        } else if ended_seen.load(Ordering::SeqCst) == 0 {
            (
                start + 10 * MS * (counter.fetch_add(1, Ordering::SeqCst) + 1),
                b,
            )
        } else {
            // The frame drawn after the end is never shown.
            (0, b)
        }
    })));
    let mut presenter = probes.start_ok(&rig, hooks);
    assert!(rig.pump_until(BOUND, || probes.frames() >= 1));
    presenter
        .request_metrics(MetricsWindow {
            id: 1,
            start,
            end: None,
        })
        .expect("accepted");
    assert!(rig.pump_until(BOUND, || presenter.metrics_adopted(1).is_some()));
    assert!(rig.pump_until(BOUND, || now_ns() >= start));
    draw(&mut rig, &presenter, &probes, 2);
    assert!(rig.pump_until(BOUND, || now_ns() > start + 50 * MS));
    let ended = now_ns();
    presenter.end_metrics(1, ended).expect("accepted");
    ended_at.store(ended, Ordering::SeqCst);
    draw(&mut rig, &presenter, &probes, 1);
    // Everything the window owes is consumed, yet nothing was shown after its end: the closing interval is unknown.
    rig.pump_for(Duration::from_millis(300));
    let open = presenter.read_metrics(1, Duration::ZERO).expect("kept");
    assert_eq!(open.outstanding, 0);
    assert!(open.closing_pending, "{open:?}");
    assert!(
        !open.complete,
        "a zero closing presentation closes nothing: {open:?}"
    );
    // The presenter ending resolves it: nothing will be shown again.
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    let ended_reading = presenter.read_metrics(1, Duration::ZERO).expect("kept");
    assert!(
        ended_reading.complete && !ended_reading.closing_pending,
        "{ended_reading:?}"
    );
}

/// The cases of this module.
pub(super) const CASES: [(&str, super::Case); 5] = [
    (
        "metrics_marks_are_ordered_and_the_window_holds_exactly_its_frames",
        metrics_marks_are_ordered_and_the_window_holds_exactly_its_frames,
    ),
    (
        "metrics_skipped_frames_are_counted_and_not_sampled",
        metrics_skipped_frames_are_counted_and_not_sampled,
    ),
    (
        "metrics_a_blocked_acquisition_adds_to_acquire_only",
        metrics_a_blocked_acquisition_adds_to_acquire_only,
    ),
    (
        "metrics_gpu_and_presentation_follow_their_rules",
        metrics_gpu_and_presentation_follow_their_rules,
    ),
    (
        "metrics_an_adoption_delayed_past_start_is_late_adopted",
        metrics_an_adoption_delayed_past_start_is_late_adopted,
    ),
];

/// Cases besides the table (they run in both modes too).
pub(super) const MORE: [(&str, super::Case); 8] = [
    (
        "metrics_a_zero_closing_presentation_leaves_the_reading_not_final",
        metrics_a_zero_closing_presentation_leaves_the_reading_not_final,
    ),
    (
        "metrics_a_recorded_frame_keeps_the_window_open_until_its_results_are_drained",
        metrics_a_recorded_frame_keeps_the_window_open_until_its_results_are_drained,
    ),
    (
        "metrics_a_late_commit_stamp_does_not_hide_a_presentation_inside_the_window",
        metrics_a_late_commit_stamp_does_not_hide_a_presentation_inside_the_window,
    ),
    (
        "metrics_a_window_waits_for_the_presentations_of_work_committed_before_it",
        metrics_a_window_waits_for_the_presentations_of_work_committed_before_it,
    ),
    (
        "metrics_results_consumed_by_the_sleep_decision_reach_the_window",
        metrics_results_consumed_by_the_sleep_decision_reach_the_window,
    ),
    (
        "metrics_a_committed_frame_not_yet_registered_keeps_the_window_open",
        metrics_a_committed_frame_not_yet_registered_keeps_the_window_open,
    ),
    (
        "metrics_results_arriving_after_the_end_belong_to_their_frame",
        metrics_results_arriving_after_the_end_belong_to_their_frame,
    ),
    (
        "metrics_the_last_frames_results_arrive_through_the_teardown",
        metrics_the_last_frames_results_arrive_through_the_teardown,
    ),
];
