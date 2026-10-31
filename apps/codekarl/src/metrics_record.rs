//! The text of the run's one `metrics` record (milestone 5, issue 10b-1; `docs/scene-design.md` §11.3).
//!
//! One `debug` record in category `metrics`, written when the presenter's end is observed. It carries the whole run's totals
//! and, when the script marked a window, the measured window beside them. Every duration is in microseconds, every time in
//! `CLOCK_UPTIME_RAW` nanoseconds. A percentile is the **upper bound** of its histogram bucket, so it is never below the true value and
//! above it by less than the bucket width, which is printed in parentheses (`p95=1350us(+25us)`); a percentile in the overflow
//! (200 ms or more) prints as out of range with the exact maximum, and a window with no samples prints `none`, never zero.

use std::fmt::Write as _;

use ck_render_2d::{FrameStats, Histogram, MetricsReading, Percentile, SceneStats};
use ck_resources::LoaderStats;
use ck_window::{LoopReading, LoopWindowStats};

use crate::metrics::HubReport;

/// What the record is made of.
pub(crate) struct Figures {
    /// The whole run's presenter figures.
    pub(crate) totals: FrameStats,
    /// The presenter's counters and charges at its end.
    pub(crate) scene: SceneStats,
    /// The resource service's loader counters at the same moment.
    pub(crate) loaders: LoaderStats,
    /// Timing results the native ring had no room for (never expected).
    pub(crate) timing_lost: u64,
    /// The window loop's totals over the run.
    pub(crate) loop_totals: LoopWindowStats,
    /// What the mark and the end left in the hub.
    pub(crate) hub: HubReport,
    /// The presenter's reading of the measured window, once marked.
    pub(crate) presenter_window: Option<MetricsReading>,
    /// The window loop's reading of the same window.
    pub(crate) loop_window: Option<LoopReading>,
}

fn micros(ns: u64) -> u64 {
    ns / 1_000
}

fn percentile(histogram: &Histogram, permille: u32) -> String {
    match histogram.percentile(permille) {
        Percentile::NoValue => "none".to_owned(),
        Percentile::Value {
            upper_bound_ns,
            error_ns,
        } => format!("{}us(+{}us)", micros(upper_bound_ns), micros(error_ns)),
        Percentile::OutOfRange { at_most_ns } => {
            format!("out-of-range(max {}us)", micros(at_most_ns))
        }
    }
}

/// `name[n=.. p50=.. p95=.. p99=.. max=..]`, with the overflow count and a saturation flag when they matter.
fn histogram(name: &str, histogram: &Histogram) -> String {
    let max = histogram
        .max_ns()
        .map_or_else(|| "none".to_owned(), |ns| format!("{}us", micros(ns)));
    let mut text = format!(
        "{name}[n={} p50={} p95={} p99={} max={max}",
        histogram.count(),
        percentile(histogram, 500),
        percentile(histogram, 950),
        percentile(histogram, 990),
    );
    if histogram.overflow() != 0 {
        let _ = write!(text, " overflow={}", histogram.overflow());
    }
    if histogram.saturated() {
        text.push_str(" saturated");
    }
    text.push(']');
    text
}

/// One set of presenter figures under `label`.
fn frames(label: &str, stats: &FrameStats) -> String {
    let mut text = format!(
        "{label}: frames={} skipped={} invalid_marks={} invalid_gpu={} presented={} not_presented={} excluded_intervals={} outstanding={} draw_calls_max={} draw_calls_sum={} instances_max={} instances_sum={} texture_binds={} checker_instances={}",
        stats.frames,
        stats.skipped,
        stats.invalid_marks,
        stats.invalid_gpu,
        stats.presented,
        stats.not_presented,
        stats.excluded_intervals,
        stats.outstanding,
        stats.draw_calls_max,
        stats.draw_calls_sum,
        stats.instances_max,
        stats.instances_sum,
        stats.texture_binds,
        stats.checker_instances,
    );
    for (name, part) in [
        ("cpu_total", &stats.cpu_total),
        ("build", &stats.build),
        ("encode", &stats.encode),
        ("ledger", &stats.ledger),
        ("acquire", &stats.acquire),
        ("gpu", &stats.gpu),
        ("presentation_interval", &stats.presentation_interval),
    ] {
        text.push(' ');
        text.push_str(&histogram(name, part));
    }
    if stats.saturated() {
        text.push_str(" counters_saturated");
    }
    text
}

fn time(ns: Option<u64>) -> String {
    ns.map_or_else(|| "none".to_owned(), |ns| ns.to_string())
}

/// `count` per second over `span_ns` in units of `per`, three decimals, from integers.
fn rate_per(count: u64, per: u64, span_ns: u64) -> String {
    if span_ns == 0 {
        return "none".to_owned();
    }
    let milli = u128::from(count) * 1_000_000_000_000 / (u128::from(span_ns) * u128::from(per));
    format!("{}.{:03}", milli / 1_000, milli % 1_000)
}

/// The upload pipeline, each figure from its own event source and labelled for what it counts:
/// - `files_decoded` / `decoded_bytes` / `queue_depth_peak`: the resource service's loaders (file sources only; an image made from
///   memory decodes no file);
/// - `batches_submitted` / `batches_completed` / `textures_created`: the presenter's upload store (`textures_created` counts native
///   textures, a CPU-source image or a repeated revision included, so it is **not** a file count);
/// - `textures_ready` and the ready times: the render thread's adoptions, the moment a revision became shown to the frames;
/// - the span runs from the script's first accepted `texture.load` to the latest ready, and the rates divide the file, byte and
///   batch counts by it.
fn uploads(scene: &SceneStats, loaders: &LoaderStats, first_load_ns: u64) -> String {
    let mut text = format!(
        "uploads: files_decoded={} decoded_bytes={} queue_depth_peak={} batches_submitted={} batches_completed={} textures_created(native)={} textures_ready={} texture_peak={}B staging_peak={}B",
        loaders.files_decoded,
        loaders.decoded_bytes,
        loaders.queue_depth_peak,
        scene.uploads_submitted,
        scene.uploads_completed,
        scene.textures_created,
        scene.textures_ready,
        scene.upload_texture_bytes_peak,
        scene.upload_staging_bytes_peak,
    );
    if loaders.files_decoded == u64::MAX || loaders.decoded_bytes == u64::MAX {
        text.push_str(" counters_saturated");
    }
    let span = (first_load_ns != 0 && scene.last_ready_ns >= first_load_ns)
        .then(|| scene.last_ready_ns - first_load_ns);
    match span {
        Some(span) => {
            let _ = write!(
                text,
                " first_load={first_load_ns} first_ready={} last_ready={} time_to_last_ready={}us files_per_s={} decoded_mib_per_s={} uploads_per_s={}",
                scene.first_ready_ns,
                scene.last_ready_ns,
                micros(span),
                rate_per(loaders.files_decoded, 1, span),
                rate_per(loaders.decoded_bytes, 1 << 20, span),
                rate_per(scene.uploads_completed, 1, span),
            );
        }
        None => text.push_str(
            " time_to_last_ready=none files_per_s=none decoded_mib_per_s=none uploads_per_s=none",
        ),
    }
    text
}

fn updates(stats: &LoopWindowStats) -> String {
    format!(
        "updates delivered={} missed={}{}",
        stats.delivered,
        stats.missed,
        if stats.saturated() {
            " counters_saturated"
        } else {
            ""
        }
    )
}

/// The record's text.
pub(crate) fn summary(figures: &Figures) -> String {
    let mut text = frames("run", &figures.totals);
    let _ = write!(
        text,
        "; run {}; timing_lost={}; {}",
        updates(&figures.loop_totals),
        figures.timing_lost,
        uploads(&figures.scene, &figures.loaders, figures.hub.first_load_ns),
    );
    match &figures.hub.window {
        None => text.push_str("; window: none (no metrics_mark)"),
        Some(window) => {
            let _ = write!(
                text,
                "; window id={} start={} end={} duration={}us marks_after_the_first={}",
                window.id,
                window.start,
                time(window.end),
                window.end.map_or_else(
                    || "none".to_owned(),
                    |end| format!("{}", micros(end.saturating_sub(window.start)))
                ),
                figures.hub.later_marks,
            );
            if let Some(refused) = &figures.hub.loop_refused {
                let _ = write!(text, " loop_refused={refused}");
            }
            if let Some(refused) = &figures.hub.presenter_refused {
                let _ = write!(text, " presenter_refused={refused}");
            }
            match &figures.presenter_window {
                None => text.push_str("; presenter window: not received"),
                Some(reading) => {
                    let _ = write!(
                        text,
                        "; presenter window adopted={} late_adopted={} end_overrun={} outside={} dropped_late={} complete={} outstanding={}; {}",
                        time(reading.window.adopted_at),
                        reading.window.late_adopted,
                        reading.window.end_overrun,
                        reading.window.outside,
                        reading.window.dropped_late,
                        reading.complete,
                        reading.outstanding,
                        frames("window", &reading.window.stats),
                    );
                    if reading.evidence_lost {
                        text.push_str(" evidence_lost");
                    }
                    if reading.closing_pending {
                        text.push_str(" closing_pending");
                    }
                }
            }
            match &figures.loop_window {
                None if figures.hub.loop_attached => {
                    text.push_str("; loop window: not adopted");
                }
                None => text.push_str("; loop window: no update source"),
                Some(reading) => {
                    let _ = write!(
                        text,
                        "; loop window adopted={} late_adopted={} end_overrun={} outside={} dropped_late={} complete={} window {}",
                        time(reading.window.adopted_at),
                        reading.window.late_adopted,
                        reading.window.end_overrun,
                        reading.window.outside,
                        reading.window.dropped_late,
                        reading.complete,
                        updates(&reading.window.stats),
                    );
                }
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use ck_render_2d::{FrameIntervals, MetricsWindow};

    use super::*;

    fn stats(frames: u64) -> FrameStats {
        let mut stats = FrameStats::default();
        for index in 0..frames {
            stats.record_intervals(FrameIntervals {
                ledger: 10_000,
                build: 100_000 + index * 1_000,
                acquire: 500_000,
                encode: 200_000,
                cpu_total: 310_000 + index * 1_000,
            });
            stats.count_frame(7, 3, 2, 1);
        }
        stats
    }

    fn figures() -> Figures {
        Figures {
            totals: stats(4),
            scene: SceneStats {
                textures_created: 6,
                uploads_submitted: 3,
                uploads_completed: 3,
                textures_ready: 5,
                first_ready_ns: 1_500_000_000,
                last_ready_ns: 3_000_000_000,
                ..SceneStats::default()
            },
            // Five files decoded (10 MiB), one texture made natively from memory or twice from one file: 6 native creations.
            loaders: LoaderStats {
                files_decoded: 5,
                decoded_bytes: 10 * 1_048_576,
                queue_depth_peak: 4,
                ..LoaderStats::default()
            },
            timing_lost: 0,
            loop_totals: LoopWindowStats {
                delivered: 9,
                missed: 2,
            },
            hub: HubReport {
                window: None,
                later_marks: 0,
                first_load_ns: 1_000_000_000,
                loop_attached: true,
                loop_refused: None,
                presenter_refused: None,
            },
            presenter_window: None,
            loop_window: None,
        }
    }

    #[test]
    fn a_run_without_a_mark_reports_the_totals_only() {
        let text = summary(&figures());
        assert!(text.starts_with("run: frames=4 skipped=0"), "{text}");
        assert!(text.contains("draw_calls_max=3"), "{text}");
        assert!(text.contains("run updates delivered=9 missed=2"), "{text}");
        assert!(text.contains("window: none (no metrics_mark)"), "{text}");
        assert!(!text.contains("presenter window"), "{text}");
        // Five files (10 MiB) and three completed batches over the 2 s from the first load to the last ready: 2.500 files/s, 5.000
        // MiB/s, 1.500 uploads/s. The six native creations are labelled as such and are not the file count.
        assert!(
            text.contains("files_decoded=5 decoded_bytes=10485760 queue_depth_peak=4 batches_submitted=3 batches_completed=3 textures_created(native)=6 textures_ready=5"),
            "{text}"
        );
        assert!(
            text.contains(
                "time_to_last_ready=2000000us files_per_s=2.500 decoded_mib_per_s=5.000 uploads_per_s=1.500"
            ),
            "{text}"
        );
        assert_eq!(text.lines().count(), 1, "one line, one record");
    }

    #[test]
    fn a_run_that_never_loaded_a_texture_reports_the_rates_as_none_not_zero() {
        let mut figures = figures();
        figures.hub.first_load_ns = 0;
        figures.loaders = LoaderStats::default();
        figures.scene.textures_ready = 0;
        figures.scene.first_ready_ns = 0;
        figures.scene.last_ready_ns = 0;
        let text = summary(&figures);
        assert!(
            text.contains("files_decoded=0 decoded_bytes=0 queue_depth_peak=0"),
            "{text}"
        );
        assert!(
            text.contains(
                "time_to_last_ready=none files_per_s=none decoded_mib_per_s=none uploads_per_s=none"
            ),
            "{text}"
        );
    }

    #[test]
    fn every_histogram_prints_its_error_and_an_empty_one_prints_none() {
        let mut figures = figures();
        figures.totals.gpu = Histogram::new();
        let text = summary(&figures);
        assert!(
            text.contains("gpu[n=0 p50=none p95=none p99=none max=none]"),
            "{text}"
        );
        // Every encode sample is exactly 200 us: the bucket's upper bound is 225 us, within its 25 us width, never below the value.
        assert!(
            text.contains(
                "encode[n=4 p50=225us(+25us) p95=225us(+25us) p99=225us(+25us) max=200us]"
            ),
            "{text}"
        );
        for name in [
            "cpu_total[",
            "build[",
            "encode[",
            "ledger[",
            "acquire[",
            "gpu[",
            "presentation_interval[",
        ] {
            assert!(text.contains(name), "{name} missing: {text}");
        }
    }

    #[test]
    fn a_marked_window_reports_its_common_start_end_adoptions_and_both_owners_figures() {
        let mut figures = figures();
        let window = MetricsWindow {
            id: 1,
            start: 5_250_000_000,
            end: Some(9_250_000_000),
        };
        figures.hub.window = Some(ck_window::MetricsWindow {
            id: 1,
            start: window.start,
            end: window.end,
        });
        figures.hub.later_marks = 2;
        figures.presenter_window = Some(MetricsReading {
            window: ck_render_2d::Tracked {
                window,
                adopted_at: Some(5_100_000_000),
                late_adopted: false,
                outside: 3,
                dropped_late: 0,
                last_admitted_ns: 9_200_000_000,
                end_overrun: false,
                stats: stats(2),
            },
            outstanding: 0,
            complete: true,
            timing_lost: 0,
            evidence_lost: false,
            closing_pending: false,
        });
        figures.loop_window = Some(LoopReading {
            window: ck_window::Tracked {
                window: ck_window::MetricsWindow {
                    id: 1,
                    start: window.start,
                    end: window.end,
                },
                adopted_at: Some(5_050_000_000),
                late_adopted: false,
                outside: 0,
                dropped_late: 0,
                last_admitted_ns: 9_100_000_000,
                end_overrun: false,
                stats: LoopWindowStats {
                    delivered: 4,
                    missed: 1,
                },
            },
            complete: true,
        });
        let text = summary(&figures);
        assert!(
            text.contains("window id=1 start=5250000000 end=9250000000 duration=4000000us marks_after_the_first=2"),
            "{text}"
        );
        assert!(
            text.contains(
                "presenter window adopted=5100000000 late_adopted=false end_overrun=false outside=3"
            ),
            "{text}"
        );
        assert!(
            text.contains("loop window adopted=5050000000 late_adopted=false end_overrun=false"),
            "{text}"
        );
        assert!(
            text.contains("window updates delivered=4 missed=1"),
            "{text}"
        );
        assert!(text.contains("window: frames=2"), "{text}");
    }
}
