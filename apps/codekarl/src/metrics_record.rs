//! The text of the run's one `metrics` record (milestone 5, issue 10b-1; `docs/scene-design.md` §11.3).
//!
//! One `debug` record in category `metrics`, written when the presenter's end is observed. It carries the whole run's totals
//! and, when the script marked a window, the measured window beside them. Every duration is in microseconds, every time in
//! `CLOCK_UPTIME_RAW` nanoseconds. A percentile is the **upper bound** of its histogram bucket, so it is never below the true value and
//! above it by less than the bucket width, which is printed in parentheses (`p95=1350us(+25us)`); a percentile in the overflow
//! (200 ms or more) prints as out of range with the exact maximum, and a window with no samples prints `none`, never zero.
//!
//! **One formatter for both backends** (milestone 5.5, issue 6): the Metal and the Vulkan presenters have each their own statistics
//! types, so [`summary`] reads an app-local plain view of the figures ([`StatsView`], [`ReadingView`], [`UploadFigures`]), which each
//! backend fills from its own types. The text is the same on both; Vulkan adds the notes that say a source is unavailable on the
//! device (`gpu: unavailable (...)`, `presentation: unavailable (...)`), never a zero in their place.

use std::fmt::Write as _;

use ck_resources::LoaderStats;
use ck_window::{LoopReading, LoopWindowStats};

use crate::metrics::HubReport;

/// A percentile as the record prints it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Pct {
    /// No samples.
    #[default]
    None,
    /// The bucket's upper bound and its width.
    Value { upper_bound_ns: u64, error_ns: u64 },
    /// In the overflow, with the exact maximum.
    OutOfRange { at_most_ns: u64 },
}

/// One histogram as the record prints it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HistView {
    pub(crate) count: u64,
    pub(crate) p50: Pct,
    pub(crate) p95: Pct,
    pub(crate) p99: Pct,
    pub(crate) max_ns: Option<u64>,
    pub(crate) overflow: u64,
    pub(crate) saturated: bool,
}

/// One set of presenter figures (the whole run, or the measured window).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StatsView {
    pub(crate) frames: u64,
    pub(crate) skipped: u64,
    pub(crate) invalid_marks: u64,
    pub(crate) invalid_gpu: u64,
    pub(crate) presented: u64,
    pub(crate) not_presented: u64,
    /// Presentation intervals that crossed a window's edge (the Metal presenter; the Vulkan one admits an interval by its later
    /// frame's hand-off and has none to exclude: 0).
    pub(crate) excluded_intervals: u64,
    /// Presents whose timing never resolved (the Vulkan presenter's bounded protocol); `None` on Metal.
    pub(crate) presentation_expired: Option<u64>,
    pub(crate) outstanding: u64,
    pub(crate) draw_calls_max: u64,
    pub(crate) draw_calls_sum: u64,
    pub(crate) instances_max: u64,
    pub(crate) instances_sum: u64,
    pub(crate) texture_binds: u64,
    pub(crate) checker_instances: u64,
    /// `cpu_total, build, encode, ledger, acquire, gpu, presentation_interval`, in this order.
    pub(crate) histograms: [HistView; 7],
    pub(crate) saturated: bool,
}

/// A copy of the presenter's measured window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the window's independent flags, each printed on its own"
)]
pub(crate) struct ReadingView {
    pub(crate) adopted_at: Option<u64>,
    pub(crate) late_adopted: bool,
    pub(crate) end_overrun: bool,
    pub(crate) outside: u64,
    pub(crate) dropped_late: u64,
    pub(crate) stats: StatsView,
    pub(crate) complete: bool,
    pub(crate) outstanding: u64,
    /// Metal's bound on committed frames with an unconsumed presentation was exceeded.
    pub(crate) evidence_lost: bool,
    /// Metal's window still owes its closing interval.
    pub(crate) closing_pending: bool,
}

/// The upload pipeline's presenter-side figures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct UploadFigures {
    pub(crate) uploads_submitted: u64,
    pub(crate) uploads_completed: u64,
    pub(crate) textures_created: u64,
    pub(crate) textures_ready: u64,
    pub(crate) first_ready_ns: u64,
    pub(crate) last_ready_ns: u64,
    pub(crate) upload_texture_bytes_peak: u64,
    pub(crate) upload_staging_bytes_peak: u64,
}

/// What a presenter gives the record: its figures for the whole run and for the measured window, and which sources are
/// unavailable on the device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PresenterFigures {
    pub(crate) totals: StatsView,
    pub(crate) uploads: UploadFigures,
    pub(crate) timing_lost: u64,
    pub(crate) window: Option<ReadingView>,
    /// Why GPU time is unavailable on this device, if it is (Vulkan).
    pub(crate) gpu_unavailable: Option<String>,
    /// Why presentation intervals are unavailable on this device, if they are (Vulkan).
    pub(crate) presentation_unavailable: Option<String>,
}

/// What the record is made of, as plain values: what [`format`] prints, whichever backend's presenter the figures came from.
pub(crate) struct Report {
    /// The presenter's figures.
    pub(crate) presenter: PresenterFigures,
    /// The resource service's loader counters at the same moment.
    pub(crate) loaders: LoaderStats,
    /// The window loop's totals over the run.
    pub(crate) loop_totals: LoopWindowStats,
    /// What the mark and the end left in the hub.
    pub(crate) hub: HubReport,
    /// The window loop's reading of the same window.
    pub(crate) loop_window: Option<LoopReading>,
}

fn micros(ns: u64) -> u64 {
    ns / 1_000
}

fn percentile(value: Pct) -> String {
    match value {
        Pct::None => "none".to_owned(),
        Pct::Value {
            upper_bound_ns,
            error_ns,
        } => format!("{}us(+{}us)", micros(upper_bound_ns), micros(error_ns)),
        Pct::OutOfRange { at_most_ns } => {
            format!("out-of-range(max {}us)", micros(at_most_ns))
        }
    }
}

/// `name[n=.. p50=.. p95=.. p99=.. max=..]`, with the overflow count and a saturation flag when they matter.
fn histogram(name: &str, histogram: &HistView) -> String {
    let max = histogram
        .max_ns
        .map_or_else(|| "none".to_owned(), |ns| format!("{}us", micros(ns)));
    let mut text = format!(
        "{name}[n={} p50={} p95={} p99={} max={max}",
        histogram.count,
        percentile(histogram.p50),
        percentile(histogram.p95),
        percentile(histogram.p99),
    );
    if histogram.overflow != 0 {
        let _ = write!(text, " overflow={}", histogram.overflow);
    }
    if histogram.saturated {
        text.push_str(" saturated");
    }
    text.push(']');
    text
}

/// One set of presenter figures under `label`.
fn frames(label: &str, stats: &StatsView) -> String {
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
    if let Some(expired) = stats.presentation_expired {
        let _ = write!(text, " presentation_expired={expired}");
    }
    for (name, part) in [
        "cpu_total",
        "build",
        "encode",
        "ledger",
        "acquire",
        "gpu",
        "presentation_interval",
    ]
    .into_iter()
    .zip(&stats.histograms)
    {
        text.push(' ');
        text.push_str(&histogram(name, part));
    }
    if stats.saturated {
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
fn uploads(scene: &UploadFigures, loaders: &LoaderStats, first_load_ns: u64) -> String {
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

/// The record's text, from the plain figures.
pub(crate) fn format(figures: &Report) -> String {
    let presenter = &figures.presenter;
    let mut text = frames("run", &presenter.totals);
    let _ = write!(
        text,
        "; run {}; timing_lost={}; {}",
        updates(&figures.loop_totals),
        presenter.timing_lost,
        uploads(
            &presenter.uploads,
            &figures.loaders,
            figures.hub.first_load_ns
        ),
    );
    // A source the device does not offer is said, never printed as a zero.
    if let Some(reason) = &presenter.gpu_unavailable {
        let _ = write!(text, "; gpu: unavailable ({reason})");
    }
    if let Some(reason) = &presenter.presentation_unavailable {
        let _ = write!(text, "; presentation: unavailable ({reason})");
    }
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
            match &presenter.window {
                None => text.push_str("; presenter window: not received"),
                Some(reading) => {
                    let _ = write!(
                        text,
                        "; presenter window adopted={} late_adopted={} end_overrun={} outside={} dropped_late={} complete={} outstanding={}; {}",
                        time(reading.adopted_at),
                        reading.late_adopted,
                        reading.end_overrun,
                        reading.outside,
                        reading.dropped_late,
                        reading.complete,
                        reading.outstanding,
                        frames("window", &reading.stats),
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

/// What the record is made of for a native-metal run: the Metal presenter's own types. (The entry point native-metal runs have always
/// had; it reads them through [`metal_views`] into the plain figures [`format`] prints.)
#[cfg(all(feature = "metal", target_os = "macos"))]
pub(crate) struct Figures {
    /// The whole run's presenter figures.
    pub(crate) totals: ck_render_2d::FrameStats,
    /// The presenter's counters and charges at its end.
    pub(crate) scene: ck_render_2d::SceneStats,
    /// The resource service's loader counters at the same moment.
    pub(crate) loaders: LoaderStats,
    /// Timing results the native ring had no room for (never expected).
    pub(crate) timing_lost: u64,
    /// The window loop's totals over the run.
    pub(crate) loop_totals: LoopWindowStats,
    /// What the mark and the end left in the hub.
    pub(crate) hub: HubReport,
    /// The presenter's reading of the measured window, once marked.
    pub(crate) presenter_window: Option<ck_render_2d::MetricsReading>,
    /// The window loop's reading of the same window.
    pub(crate) loop_window: Option<LoopReading>,
}

/// The record's text for a native-metal run.
#[cfg(all(feature = "metal", target_os = "macos"))]
pub(crate) fn summary(figures: &Figures) -> String {
    format(&Report {
        presenter: PresenterFigures {
            totals: metal_views::stats(&figures.totals),
            uploads: metal_views::uploads(&figures.scene),
            timing_lost: figures.timing_lost,
            window: figures.presenter_window.as_ref().map(metal_views::reading),
            gpu_unavailable: None,
            presentation_unavailable: None,
        },
        loaders: figures.loaders,
        loop_totals: figures.loop_totals,
        hub: figures.hub.clone(),
        loop_window: figures.loop_window.clone(),
    })
}

/// The record's text for a `glfw-vulkan` run: the Vulkan entry point. The presenter's figures arrive as plain values, read from its
/// own types by [`vulkan_views`] (and, when it is the fake of the tests, supplied directly); the text is [`format`]'s, with the
/// notes that say a source is unavailable on the device.
pub(crate) fn vulkan_summary(
    presenter: PresenterFigures,
    loaders: LoaderStats,
    loop_totals: LoopWindowStats,
    hub: HubReport,
    loop_window: Option<LoopReading>,
) -> String {
    format(&Report {
        presenter,
        loaders,
        loop_totals,
        hub,
        loop_window,
    })
}

/// The views of the Metal presenter's types.
#[cfg(all(feature = "metal", target_os = "macos"))]
pub(crate) mod metal_views {
    use ck_render_2d::{FrameStats, Histogram, MetricsReading, Percentile, SceneStats};

    use super::{HistView, Pct, ReadingView, StatsView, UploadFigures};

    fn pct(value: Percentile) -> Pct {
        match value {
            Percentile::NoValue => Pct::None,
            Percentile::Value {
                upper_bound_ns,
                error_ns,
            } => Pct::Value {
                upper_bound_ns,
                error_ns,
            },
            Percentile::OutOfRange { at_most_ns } => Pct::OutOfRange { at_most_ns },
        }
    }

    fn histogram(histogram: &Histogram) -> HistView {
        HistView {
            count: histogram.count(),
            p50: pct(histogram.percentile(500)),
            p95: pct(histogram.percentile(950)),
            p99: pct(histogram.percentile(990)),
            max_ns: histogram.max_ns(),
            overflow: histogram.overflow(),
            saturated: histogram.saturated(),
        }
    }

    /// The statistics of the Metal presenter, as the record prints them.
    pub(crate) fn stats(stats: &FrameStats) -> StatsView {
        StatsView {
            frames: stats.frames,
            skipped: stats.skipped,
            invalid_marks: stats.invalid_marks,
            invalid_gpu: stats.invalid_gpu,
            presented: stats.presented,
            not_presented: stats.not_presented,
            excluded_intervals: stats.excluded_intervals,
            presentation_expired: None,
            outstanding: stats.outstanding,
            draw_calls_max: stats.draw_calls_max,
            draw_calls_sum: stats.draw_calls_sum,
            instances_max: stats.instances_max,
            instances_sum: stats.instances_sum,
            texture_binds: stats.texture_binds,
            checker_instances: stats.checker_instances,
            histograms: [
                histogram(&stats.cpu_total),
                histogram(&stats.build),
                histogram(&stats.encode),
                histogram(&stats.ledger),
                histogram(&stats.acquire),
                histogram(&stats.gpu),
                histogram(&stats.presentation_interval),
            ],
            saturated: stats.saturated(),
        }
    }

    /// The presenter's reading of the measured window.
    pub(crate) fn reading(reading: &MetricsReading) -> ReadingView {
        ReadingView {
            adopted_at: reading.window.adopted_at,
            late_adopted: reading.window.late_adopted,
            end_overrun: reading.window.end_overrun,
            outside: reading.window.outside,
            dropped_late: reading.window.dropped_late,
            stats: stats(&reading.window.stats),
            complete: reading.complete,
            outstanding: reading.outstanding,
            evidence_lost: reading.evidence_lost,
            closing_pending: reading.closing_pending,
        }
    }

    /// The upload store's and ledger's counters.
    pub(crate) fn uploads(stats: &SceneStats) -> UploadFigures {
        UploadFigures {
            uploads_submitted: stats.uploads_submitted,
            uploads_completed: stats.uploads_completed,
            textures_created: stats.textures_created,
            textures_ready: stats.textures_ready,
            first_ready_ns: stats.first_ready_ns,
            last_ready_ns: stats.last_ready_ns,
            upload_texture_bytes_peak: stats.upload_texture_bytes_peak,
            upload_staging_bytes_peak: stats.upload_staging_bytes_peak,
        }
    }
}

/// The views of the Vulkan presenter's types.
#[cfg(feature = "vulkan")]
pub(crate) mod vulkan_views {
    use ck_render_2d::vulkan_timing::{
        FrameStats, Histogram, MetricsReading, Percentile, SceneStats,
    };

    use super::{HistView, Pct, ReadingView, StatsView, UploadFigures};

    fn pct(value: Percentile) -> Pct {
        match value {
            Percentile::NoValue => Pct::None,
            Percentile::Value {
                upper_bound_ns,
                error_ns,
            } => Pct::Value {
                upper_bound_ns,
                error_ns,
            },
            Percentile::OutOfRange { at_most_ns } => Pct::OutOfRange { at_most_ns },
        }
    }

    fn histogram(histogram: &Histogram) -> HistView {
        HistView {
            count: histogram.count(),
            p50: pct(histogram.percentile(500)),
            p95: pct(histogram.percentile(950)),
            p99: pct(histogram.percentile(990)),
            max_ns: histogram.max_ns(),
            overflow: histogram.overflow(),
            saturated: histogram.saturated(),
        }
    }

    /// The statistics of the Vulkan presenter, as the record prints them.
    pub(crate) fn stats(stats: &FrameStats) -> StatsView {
        StatsView {
            frames: stats.frames,
            skipped: stats.skipped,
            invalid_marks: stats.invalid_marks,
            invalid_gpu: stats.invalid_gpu,
            presented: stats.presented,
            not_presented: stats.not_presented,
            excluded_intervals: 0,
            presentation_expired: Some(stats.presentation_expired),
            outstanding: stats.outstanding,
            draw_calls_max: stats.draw_calls_max,
            draw_calls_sum: stats.draw_calls_sum,
            instances_max: stats.instances_max,
            instances_sum: stats.instances_sum,
            texture_binds: stats.texture_binds,
            checker_instances: stats.checker_instances,
            histograms: [
                histogram(&stats.cpu_total),
                histogram(&stats.build),
                histogram(&stats.encode),
                histogram(&stats.ledger),
                histogram(&stats.acquire),
                histogram(&stats.gpu),
                histogram(&stats.presentation_interval),
            ],
            saturated: stats.saturated(),
        }
    }

    /// The presenter's reading of the measured window.
    pub(crate) fn reading(reading: &MetricsReading) -> ReadingView {
        ReadingView {
            adopted_at: reading.window.adopted_at,
            late_adopted: reading.window.late_adopted,
            end_overrun: reading.window.end_overrun,
            outside: reading.window.outside,
            dropped_late: reading.window.dropped_late,
            stats: stats(&reading.window.stats),
            complete: reading.complete,
            outstanding: reading.outstanding,
            evidence_lost: false,
            closing_pending: false,
        }
    }

    /// The Vulkan presenter's figures for the record.
    pub(crate) fn presenter(
        totals: &FrameStats,
        scene: &SceneStats,
        window: Option<&MetricsReading>,
        unavailable: (Option<&str>, Option<&str>),
    ) -> super::PresenterFigures {
        super::PresenterFigures {
            totals: stats(totals),
            uploads: uploads(scene),
            timing_lost: 0,
            window: window.map(reading),
            gpu_unavailable: unavailable.0.map(str::to_owned),
            presentation_unavailable: unavailable.1.map(str::to_owned),
        }
    }

    /// The upload store's and ledger's counters.
    pub(crate) fn uploads(stats: &SceneStats) -> UploadFigures {
        UploadFigures {
            uploads_submitted: stats.upload_batches_submitted,
            uploads_completed: stats.upload_batches_completed,
            textures_created: stats.textures_created,
            textures_ready: stats.textures_ready,
            first_ready_ns: stats.first_ready_ns,
            last_ready_ns: stats.last_ready_ns,
            upload_texture_bytes_peak: stats.texture_bytes_peak,
            upload_staging_bytes_peak: stats.staging_bytes_peak,
        }
    }
}

#[cfg(test)]
mod plain_tests {
    use super::*;

    fn hist(count: u64, upper_bound_ns: u64, error_ns: u64, max_ns: u64) -> HistView {
        let value = Pct::Value {
            upper_bound_ns,
            error_ns,
        };
        HistView {
            count,
            p50: value,
            p95: value,
            p99: value,
            max_ns: Some(max_ns),
            overflow: 0,
            saturated: false,
        }
    }

    fn empty() -> HistView {
        HistView {
            count: 0,
            p50: Pct::None,
            p95: Pct::None,
            p99: Pct::None,
            max_ns: None,
            overflow: 0,
            saturated: false,
        }
    }

    /// `frames` frames of 7 instances in 3 draws, 2 table binds and 1 checker each; every encode sample is exactly 200 us.
    fn stats(frames: u64) -> StatsView {
        StatsView {
            frames,
            skipped: 0,
            invalid_marks: 0,
            invalid_gpu: 0,
            presented: 0,
            not_presented: 0,
            excluded_intervals: 0,
            presentation_expired: None,
            outstanding: 0,
            draw_calls_max: 3,
            draw_calls_sum: 3 * frames,
            instances_max: 7,
            instances_sum: 7 * frames,
            texture_binds: 2 * frames,
            checker_instances: frames,
            histograms: [
                hist(frames, 325_000, 25_000, 313_000),
                hist(frames, 125_000, 25_000, 103_000),
                hist(frames, 225_000, 25_000, 200_000),
                hist(frames, 25_000, 25_000, 10_000),
                hist(frames, 525_000, 25_000, 500_000),
                empty(),
                empty(),
            ],
            saturated: false,
        }
    }

    fn figures() -> Report {
        Report {
            presenter: PresenterFigures {
                totals: stats(4),
                uploads: UploadFigures {
                    textures_created: 6,
                    uploads_submitted: 3,
                    uploads_completed: 3,
                    textures_ready: 5,
                    first_ready_ns: 1_500_000_000,
                    last_ready_ns: 3_000_000_000,
                    ..UploadFigures::default()
                },
                timing_lost: 0,
                window: None,
                gpu_unavailable: None,
                presentation_unavailable: None,
            },
            // Five files decoded (10 MiB), one texture made natively from memory or twice from one file: 6 native creations.
            loaders: LoaderStats {
                files_decoded: 5,
                decoded_bytes: 10 * 1_048_576,
                queue_depth_peak: 4,
                ..LoaderStats::default()
            },
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
            loop_window: None,
        }
    }

    #[test]
    fn a_run_without_a_mark_reports_the_totals_only() {
        let text = format(&figures());
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
        assert!(
            !text.contains("unavailable"),
            "a device with both sources says nothing: {text}"
        );
        assert!(!text.contains("presentation_expired"), "{text}");
    }

    #[test]
    fn a_run_that_never_loaded_a_texture_reports_the_rates_as_none_not_zero() {
        let mut figures = figures();
        figures.hub.first_load_ns = 0;
        figures.loaders = LoaderStats::default();
        figures.presenter.uploads.textures_ready = 0;
        figures.presenter.uploads.first_ready_ns = 0;
        figures.presenter.uploads.last_ready_ns = 0;
        let text = format(&figures);
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
        let text = format(&figures());
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
    fn a_percentile_in_the_overflow_prints_out_of_range_with_the_exact_maximum() {
        let mut figures = figures();
        figures.presenter.totals.histograms[2] = HistView {
            count: 3,
            p50: Pct::Value {
                upper_bound_ns: 225_000,
                error_ns: 25_000,
            },
            p95: Pct::OutOfRange {
                at_most_ns: 312_000_000,
            },
            p99: Pct::OutOfRange {
                at_most_ns: 312_000_000,
            },
            max_ns: Some(312_000_000),
            overflow: 1,
            saturated: false,
        };
        let text = format(&figures);
        assert!(
            text.contains("encode[n=3 p50=225us(+25us) p95=out-of-range(max 312000us) p99=out-of-range(max 312000us) max=312000us overflow=1]"),
            "{text}"
        );
    }

    #[test]
    fn a_source_the_device_does_not_offer_is_said_and_never_a_zero() {
        let mut figures = figures();
        figures.presenter.gpu_unavailable = Some("timestampValidBits 0".to_owned());
        figures.presenter.presentation_unavailable =
            Some("VK_GOOGLE_display_timing not advertised".to_owned());
        figures.presenter.totals.presentation_expired = Some(2);
        let text = format(&figures);
        assert!(
            text.contains("gpu: unavailable (timestampValidBits 0)"),
            "{text}"
        );
        assert!(
            text.contains("presentation: unavailable (VK_GOOGLE_display_timing not advertised)"),
            "{text}"
        );
        assert!(text.contains("presentation_expired=2"), "{text}");
        // The histograms of the unavailable sources are empty, and print none: the note is the statement, not a number.
        assert!(text.contains("gpu[n=0 p50=none"), "{text}");
        assert_eq!(text.lines().count(), 1, "still one line, one record");
    }

    #[test]
    fn a_marked_window_reports_its_common_start_end_adoptions_and_both_owners_figures() {
        let mut figures = figures();
        let window = ck_window::MetricsWindow {
            id: 1,
            start: 5_250_000_000,
            end: Some(9_250_000_000),
        };
        figures.hub.window = Some(window);
        figures.hub.later_marks = 2;
        figures.presenter.window = Some(ReadingView {
            adopted_at: Some(5_100_000_000),
            late_adopted: false,
            end_overrun: false,
            outside: 3,
            dropped_late: 0,
            stats: stats(2),
            complete: true,
            outstanding: 0,
            evidence_lost: false,
            closing_pending: false,
        });
        figures.loop_window = Some(LoopReading {
            window: ck_window::Tracked {
                window,
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
        let text = format(&figures);
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

#[cfg(test)]
#[cfg(all(feature = "metal", target_os = "macos"))]
mod metal_tests {
    use ck_render_2d::{
        FrameIntervals, FrameStats, Histogram, MetricsReading, MetricsWindow, SceneStats,
    };

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
