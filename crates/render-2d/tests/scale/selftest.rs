//! The harness's own self-test (a few seconds of one real window): the fixed cadence of the static phases and the way their windows admit
//! samples, judged against marks the harness records itself.
//!
//! - **Cadence:** over a 5 s window the static phase's frame count is 300 +- 2 % at 60 Hz with identical geometry each frame.
//! - **Insufficient, never a pass:** a phase whose republication stopped has too few samples and the static verdict says so.
//! - **Window edges:** the window's frame count is exactly the number of frames whose commit mark lies in `[start, end)`, so no
//!   sample is timed before `start` or at or after `end`.
//! - **Delayed handlers (CK-129's hooks):** with the presentation results of a short window held back past `end` (within the
//!   presenter's 16-result hold and the recorder's 16-entry pending bound) and the shown times replaced by times a little after each
//!   commit, an interval whose two presented timestamps are inside is admitted, and one whose second timestamp is at or after `end`
//!   is excluded, as two separate counts. The test also records WHEN each result was applied and requires every in-window result to
//!   have been applied after `end` (a case that applied them earlier fails), and the final reading to be complete with no
//!   evidence lost.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use ck_gpu_metal::{FrameMarks, MarksProbe, TestHooks, TimingProbe};
use ck_render_2d::{MetricsWindow, now_ns};

use crate::evidence;
use crate::json::J;
use crate::rig::{Cadence, PERIOD_NS, Scene};

/// The synthetic delay between a frame's commit and its (replaced) shown time.
const SHOWN_AFTER: u64 = 2_000_000;
/// The delayed-handler window: ticks of the 60 Hz schedule between the hold going on and the window's start, in the window, and
/// from its end to the release.
const GAP_TICKS: u64 = 2;
/// See [`GAP_TICKS`]: the window itself.
const WINDOW_TICKS: u64 = 8;
/// See [`GAP_TICKS`]: the tail after the end, still held.
const TAIL_TICKS: u64 = 2;
/// How many presentation results the presenter's hold keeps (and the recorder's pending bound): the frames committed while the hold is
/// on must stay under it.
const HOLD_CAPACITY: usize = 16;

/// What the self-test records of the presenter's hooks.
#[derive(Clone, Default)]
pub(crate) struct Recorded {
    /// The commit mark of every frame.
    marks: Arc<Mutex<Vec<u64>>>,
    /// Holds presentation results back while non-zero.
    pub(crate) hold: Arc<AtomicI32>,
    /// The synthetic shown times are on once this is set.
    pub(crate) synthesize: Arc<AtomicI32>,
    /// Every replaced presentation result, in the order it was applied.
    shown: Arc<Mutex<Vec<Applied>>>,
}

/// One presentation result the hook replaced: the frame's commit mark, the shown time it was given and when it was applied.
#[derive(Clone, Copy)]
struct Applied {
    t5: u64,
    shown: u64,
    at_ns: u64,
}

impl Recorded {
    /// The hooks that record the marks, hold presentations and replace their shown times.
    pub(crate) fn hooks(&self) -> TestHooks {
        let marks = Arc::clone(&self.marks);
        let (shown, synthesize) = (Arc::clone(&self.shown), Arc::clone(&self.synthesize));
        TestHooks {
            frame_marks: Some(MarksProbe(Arc::new(move |marks_of: &FrameMarks| {
                marks
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(marks_of.t5);
            }))),
            hold_presentations: Some(Arc::clone(&self.hold)),
            timing_override: Some(TimingProbe(Arc::new(move |kind, t5, a, b| {
                if kind != 2 || synthesize.load(Ordering::SeqCst) == 0 {
                    return (a, b);
                }
                let at = t5 + SHOWN_AFTER;
                shown
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(Applied {
                        t5,
                        shown: at,
                        at_ns: now_ns(),
                    });
                (at, b)
            }))),
            ..TestHooks::default()
        }
    }

    fn applied(&self) -> Vec<Applied> {
        self.shown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn marks(&self) -> Vec<u64> {
        self.marks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// A fixed-cadence republication of one unchanged quad until `until` (a new generation each tick), returning the ticks that published.
fn republish(scene: &mut Scene, quad: ck_render_2d::QuadKey, until: u64, publish: bool) -> u64 {
    let mut cadence = Cadence::starting_at(now_ns());
    let mut published = 0;
    while cadence.tick() < until {
        if publish {
            let _ = scene.writer.set_position(quad, 100.0, 100.0);
            published += u64::from(scene.writer.publish());
        }
    }
    published
}

/// The self-test phase.
#[expect(
    clippy::too_many_lines,
    reason = "one phase is one sequential protocol whose steps read best in order"
)]
pub(crate) fn run(scene: &mut Scene, recorded: &Recorded) -> Result<J, String> {
    scene.wait_open(Duration::from_secs(60))?;
    // One unchanged quad over a loaded texture.
    let reservation = scene
        .service
        .reserve_pixels(8, 8)
        .map_err(|error| format!("{error:?}"))?;
    let pixels = ck_resources::CpuPixels::premultiplied(reservation, [200, 20, 20, 255].repeat(64))
        .map_err(|error| format!("{error:?}"))?;
    let texture = scene
        .service
        .create_from_pixels(pixels)
        .map_err(|_| "the texture was refused".to_owned())?;
    let reference = scene
        .service
        .retain(texture)
        .map_err(|error| format!("{error:?}"))?;
    let quad = scene
        .writer
        .create_quad(reference, 100.0, 100.0, 40.0, 40.0)
        .map_err(|error| format!("{error:?}"))?;
    let _ = scene.writer.publish();
    // Let the texture land and the first frames draw before anything is measured.
    republish(scene, quad, now_ns() + 1_000_000_000, true);

    // ---- 1. The cadence, and the window's edges (no hooks, real presentation). -------------------------------------------------
    let start = now_ns() + 300_000_000;
    let end = start + 5_000_000_000;
    scene.request_window(MetricsWindow {
        id: 1,
        start,
        end: Some(end),
    })?;
    let published = republish(scene, quad, end + 4 * PERIOD_NS, true);
    let reading = scene
        .read_window(1, Duration::from_secs(2))
        .ok_or("no reading for the cadence window")?;
    let in_window = recorded
        .marks()
        .iter()
        .filter(|t5| (start..end).contains(t5))
        .count() as u64;
    let frames = reading.window.stats.frames;
    let cadence_ok = (294..=306).contains(&frames);
    let edges_ok = frames == in_window
        && reading.window.stats.cpu_total.count() == frames
        && reading.window.last_admitted_ns < end
        && reading.window.last_admitted_ns >= start;
    let cadence = J::o([
        ("frames_in_window", J::U(frames)),
        ("expected", J::A(vec![J::U(294), J::U(306)])),
        ("republications", J::U(published)),
        ("marks_in_window", J::U(in_window)),
        ("last_admitted_ns", J::U(reading.window.last_admitted_ns)),
        ("window", evidence::reading(&reading)),
    ]);

    // ---- 2. A stopped republication is insufficient, never a pass. ------------------------------------------------------------
    let start2 = now_ns() + 300_000_000;
    let end2 = start2 + 2_000_000_000;
    scene.request_window(MetricsWindow {
        id: 2,
        start: start2,
        end: Some(end2),
    })?;
    republish(scene, quad, end2 + 4 * PERIOD_NS, false);
    let stopped = scene
        .read_window(2, Duration::from_secs(2))
        .ok_or("no reading for the stopped window")?;
    let (stopped_passed, stopped_verdict) = evidence::static_verdict(&stopped);
    let insufficient = matches!(&stopped_verdict, J::O(fields) if fields.iter().any(|(name, value)| name == "insufficient" && *value == J::Bool(true)));
    let stopped_ok = !stopped_passed && insufficient && stopped.window.stats.frames < 20;

    // ---- 3. A delayed handler: results held past the end, the shown times a little after each commit. -----------------------------
    // The presenter keeps back only the first 16 presentation results taken while the hold is on (later ones are applied at once), and
    // the recorder tracks 16 pending presentations; so the hold goes on just before the window and the frames committed from then
    // until the results are let go (the gap, the window and the tail) stay well under 16. The count is checked, not assumed.
    recorded.synthesize.store(1, Ordering::SeqCst);
    recorded
        .shown
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    let marks_before = recorded.marks().len();
    let start3 = now_ns() + GAP_TICKS * PERIOD_NS;
    let end3 = start3 + WINDOW_TICKS * PERIOD_NS;
    scene.request_window(MetricsWindow {
        id: 3,
        start: start3,
        end: Some(end3),
    })?;
    recorded.hold.store(1, Ordering::SeqCst);
    republish(scene, quad, end3 + TAIL_TICKS * PERIOD_NS, true);
    // Every result so far is still held: none was applied. This is read after the end and before the release.
    let applied_while_held = recorded
        .applied()
        .iter()
        .filter(|record| record.at_ns <= end3)
        .count();
    let held_at_end = scene.read_window(3, Duration::from_millis(200));
    let frames_held = recorded.marks().len().saturating_sub(marks_before);
    let release_at = now_ns();
    recorded.hold.store(0, Ordering::SeqCst);
    // A few frames follow to carry the released results in.
    republish(scene, quad, now_ns() + 6 * PERIOD_NS, true);
    let delayed = scene
        .read_window(3, Duration::from_secs(2))
        .ok_or("no reading for the delayed window")?;
    recorded.synthesize.store(0, Ordering::SeqCst);
    // The shown times the hook gave, in the order the results were applied; the expected counts come from them, and they are checked
    // against the commit marks (the same offset) so a changed hook cannot change both sides.
    let applied = recorded.applied();
    let marks = recorded.marks();
    let by_marks: Vec<u64> = marks[marks_before.min(marks.len())..]
        .iter()
        .map(|t5| t5 + SHOWN_AFTER)
        .collect();
    let shown_times: Vec<u64> = applied.iter().map(|record| record.shown).collect();
    let in_commit_order = applied.windows(2).all(|pair| pair[0].t5 < pair[1].t5);
    let inside = |at: u64| (start3..end3).contains(&at);
    let in_window: Vec<&Applied> = applied
        .iter()
        .filter(|record| inside(record.shown))
        .collect();
    let in_window_late = in_window
        .iter()
        .filter(|record| record.at_ns > end3)
        .count();
    let admitted_expected = shown_times
        .windows(2)
        .filter(|p| inside(p[0]) && inside(p[1]))
        .count() as u64;
    let crossing_start_expected = shown_times
        .windows(2)
        .filter(|p| !inside(p[0]) && inside(p[1]))
        .count() as u64;
    let crossing_end_expected = shown_times
        .windows(2)
        .filter(|p| inside(p[0]) && !inside(p[1]))
        .count() as u64;
    let crossing_expected = crossing_start_expected + crossing_end_expected;
    // The witnesses, each separate: the hold held (capacity respected, nothing applied before the end), every in-window result was
    // applied after the end (so none was delivered early), the early reading was not final for that reason alone, the end-crossing
    // interval is excluded, and the final reading is complete with nothing lost.
    let hold_respected = frames_held < HOLD_CAPACITY && applied_while_held == 0;
    let late_witness = in_window.len() >= 5 && in_window_late == in_window.len();
    let early_not_final = held_at_end
        .as_ref()
        .is_none_or(|early| !early.complete && !early.evidence_lost);
    // The tail frames after the release may not all be applied when the hook is switched off, so the applied ones must be a prefix.
    let marks_agree = shown_times.len() <= by_marks.len()
        && shown_times[..] == by_marks[..shown_times.len()]
        && in_commit_order;
    let final_ok = delayed.complete && !delayed.evidence_lost && delayed.timing_lost == 0;
    let counts_ok = delayed.window.stats.presentation_interval.count() == admitted_expected
        && delayed.window.stats.excluded_intervals == crossing_expected
        && admitted_expected >= 4
        && crossing_start_expected >= 1
        && crossing_end_expected >= 1;
    let delayed_ok =
        hold_respected && late_witness && early_not_final && marks_agree && final_ok && counts_ok;
    let delayed_record = J::o([
        (
            "admitted",
            J::U(delayed.window.stats.presentation_interval.count()),
        ),
        ("admitted_expected", J::U(admitted_expected)),
        ("excluded", J::U(delayed.window.stats.excluded_intervals)),
        ("excluded_expected", J::U(crossing_expected)),
        ("excluded_expected_at_start", J::U(crossing_start_expected)),
        ("excluded_expected_at_end", J::U(crossing_end_expected)),
        ("frames_while_held", J::U(frames_held as u64)),
        ("hold_capacity", J::U(HOLD_CAPACITY as u64)),
        (
            "applied_before_end_while_held",
            J::U(applied_while_held as u64),
        ),
        ("in_window_results", J::U(in_window.len() as u64)),
        (
            "in_window_results_applied_after_end",
            J::U(in_window_late as u64),
        ),
        (
            "released_after_end_by_ns",
            J::U(release_at.saturating_sub(end3)),
        ),
        (
            "shown_times_match_commit_marks_in_order",
            J::Bool(marks_agree),
        ),
        ("hold_respected", J::Bool(hold_respected)),
        ("late_delivery_witnessed", J::Bool(late_witness)),
        (
            "read_at_end_complete",
            held_at_end
                .as_ref()
                .map_or(J::Null, |early| J::Bool(early.complete)),
        ),
        (
            "read_at_end_evidence_lost",
            held_at_end
                .as_ref()
                .map_or(J::Null, |early| J::Bool(early.evidence_lost)),
        ),
        ("early_reading_not_final", J::Bool(early_not_final)),
        ("final_complete_without_loss", J::Bool(final_ok)),
        ("window", evidence::reading(&delayed)),
    ]);

    let _ = scene.writer.destroy_quad(quad);
    let _ = scene.writer.publish();
    let _ = scene.service.release(texture);
    thread::sleep(Duration::from_millis(50));
    let passed = cadence_ok && edges_ok && stopped_ok && delayed_ok;
    Ok(J::o([
        ("phase", J::s("selftest")),
        ("cadence", cadence),
        ("cadence_ok", J::Bool(cadence_ok)),
        ("edges_ok", J::Bool(edges_ok)),
        ("stopped_verdict", stopped_verdict),
        ("stopped_ok", J::Bool(stopped_ok)),
        ("delayed_handler", delayed_record),
        ("delayed_ok", J::Bool(delayed_ok)),
        ("passed", J::Bool(passed)),
    ]))
}
