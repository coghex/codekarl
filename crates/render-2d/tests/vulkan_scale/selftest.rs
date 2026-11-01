//! The harness's own self-test (a few seconds of one real window): the fixed cadence of the static phases and the way their windows
//! admit samples.
//!
//! - **Cadence:** over a 5 s window the static phase's frame count is 300 +- 2 % at 60 Hz with identical geometry each frame, the
//!   window was adopted before its start, and every sample it holds lies in `[start, end)` (its latest admitted timestamp).
//! - **Insufficient, never a pass:** a phase whose republication stopped has too few samples and the reference reading says so (it is
//!   data, not a pass or a fail: nothing here is gated on time).
//! - **The end set afterwards:** a window opened without an end and ended while frames keep coming (the way the load phase ends its
//!   own window) holds exactly the frames up to that end: none at or after it, and the count is the number of periods within it.
//!
//! The Metal self-test's delayed-handler case rested on Metal's presentation hook; on Vulkan the late-feedback and window-end
//! behaviours are covered by Part B's own tests of the recorder (`ck-gpu-vulkan`, `metrics.rs`, `render/scene/timing.rs`), against
//! supplied feedback, with no real-driver timing assumption.

use std::thread;
use std::time::Duration;

use ck_render_2d::QuadKey;
use ck_render_2d::vulkan_timing::MetricsWindow;

use crate::evidence;
use crate::json::J;
use crate::rig::{Cadence, PERIOD_NS, Scene, now_ns};

/// A fixed-cadence republication of one unchanged quad until `until` (a new generation each tick), returning the ticks that published.
fn republish(scene: &mut Scene, quad: QuadKey, until: u64, publish: bool) -> u64 {
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

/// Whether `record` has an array field `name` with at least one entry.
fn has_entries(record: &J, name: &str) -> bool {
    matches!(record.get(name), Some(J::A(entries)) if !entries.is_empty())
}

/// The self-test phase.
#[expect(
    clippy::too_many_lines,
    reason = "one phase is one sequential protocol whose steps read best in order"
)]
pub(crate) fn run(scene: &mut Scene) -> Result<J, String> {
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

    // ---- 1. The cadence, and the window's edges (real presentation). -----------------------------------------------------------
    let start = now_ns() + 300_000_000;
    let end = start + 5_000_000_000;
    scene.request_window(MetricsWindow {
        id: 1,
        start,
        end: Some(end),
    })?;
    let mut adopted = None;
    let mut cadence_clock = Cadence::starting_at(now_ns());
    let mut published = 0_u64;
    loop {
        let now = cadence_clock.tick();
        if adopted.is_none() {
            adopted = scene.adopted(1);
        }
        let _ = scene.writer.set_position(quad, 100.0, 100.0);
        published += u64::from(scene.writer.publish());
        if now >= end + 4 * PERIOD_NS {
            break;
        }
    }
    let reading = scene
        .read_window(1, Duration::from_secs(2))
        .ok_or("no reading for the cadence window")?;
    let frames = reading.window.stats.frames;
    let cadence_ok = (294..=306).contains(&frames);
    let adopted_before = adopted.is_some_and(|at| at < start);
    let edges_ok = adopted_before
        && !reading.window.late_adopted
        && reading.window.dropped_late == 0
        && !reading.window.end_overrun
        && reading.window.stats.cpu_total.count() == frames
        && reading.window.last_admitted_ns < end
        && reading.window.last_admitted_ns >= start
        && reading.complete;
    let cadence = J::o([
        ("frames_in_window", J::U(frames)),
        ("expected", J::A(vec![J::U(294), J::U(306)])),
        ("republications", J::U(published)),
        ("adopted_before_start", J::Bool(adopted_before)),
        ("last_admitted_ns", J::U(reading.window.last_admitted_ns)),
        ("window_start_ns", J::U(start)),
        ("window_end_ns", J::U(end)),
        ("window", evidence::reading(&reading)),
        ("schedule_late_ticks", J::U(cadence_clock.late)),
        ("schedule_worst_late_ns", J::U(cadence_clock.worst_late_ns)),
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
    let stopped_reading = evidence::reference_reading(&stopped);
    let stopped_ok = has_entries(&stopped_reading, "insufficient_data")
        && stopped.window.stats.frames < 20
        && stopped_reading.get("gated") == Some(&J::Bool(false));

    // ---- 3. The end set afterwards: the window holds exactly the frames up to it. -------------------------------------------
    let start3 = now_ns() + 300_000_000;
    scene.request_window(MetricsWindow {
        id: 3,
        start: start3,
        end: None,
    })?;
    republish(scene, quad, start3 + 2_000_000_000, true);
    let end3 = now_ns() + 3 * PERIOD_NS;
    scene.end_window(3, end3)?;
    republish(scene, quad, end3 + 6 * PERIOD_NS, true);
    let ended = scene
        .read_window(3, Duration::from_secs(2))
        .ok_or("no reading for the window ended afterwards")?;
    // The window is the periods between its start and its end: 60 Hz gives `(end - start) / period` frames, +- 2 for the two edges.
    let expected = (end3 - start3) / PERIOD_NS;
    let frames3 = ended.window.stats.frames;
    let after_end_ok = frames3 + 2 >= expected
        && frames3 <= expected + 2
        && ended.window.last_admitted_ns < end3
        && ended.window.last_admitted_ns >= start3
        && !ended.window.end_overrun
        && !ended.window.late_adopted
        && ended.complete;
    let after_end = J::o([
        ("frames_in_window", J::U(frames3)),
        ("expected_periods", J::U(expected)),
        ("tolerance_frames", J::U(2)),
        ("last_admitted_ns", J::U(ended.window.last_admitted_ns)),
        ("window_start_ns", J::U(start3)),
        ("window_end_ns", J::U(end3)),
        ("window", evidence::reading(&ended)),
    ]);

    let _ = scene.writer.destroy_quad(quad);
    let _ = scene.writer.publish();
    let _ = scene.service.release(texture);
    thread::sleep(Duration::from_millis(50));
    let passed = cadence_ok && edges_ok && stopped_ok && after_end_ok;
    Ok(J::o([
        ("phase", J::s("selftest")),
        ("cadence", cadence),
        ("cadence_ok", J::Bool(cadence_ok)),
        ("edges_ok", J::Bool(edges_ok)),
        ("stopped_reading", stopped_reading),
        ("stopped_frames", J::U(stopped.window.stats.frames)),
        ("stopped_ok", J::Bool(stopped_ok)),
        ("end_set_afterwards", after_end),
        ("end_set_afterwards_ok", J::Bool(after_end_ok)),
        ("passed", J::Bool(passed)),
    ]))
}
