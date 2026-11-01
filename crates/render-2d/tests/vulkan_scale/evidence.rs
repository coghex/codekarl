//! How the Vulkan harness turns the presenter's windows into the evidence (milestone 5.5, issue 6, part C): histograms to percentiles,
//! readings to records, the §11.3 reference (printed beside each figure, **not a gate on Vulkan**), the process footprint, and the pieces
//! that every phase shares.
//!
//! The schema is the Metal harness's wherever the quantity exists on both backends. The Vulkan-only figures are the configured descriptor
//! counts, the live `vkAllocateMemory` count against `maxMemoryAllocationCount`, and presentation expired; the Metal-only fields
//! (`residency_set_bytes`, `bindless_table_bytes`, `residency_changes`) are the string `"not_applicable"`; and a source the device does
//! not offer is the explicit `"unavailable"` with its reason, never a zero.

use ck_render_2d::vulkan_timing::{FrameStats, Histogram, MetricsReading, Percentile, SceneStats};
use ck_resources::{BudgetStats, ServiceStats};

use crate::json::J;

/// §11.3's reference for the p95 of the CPU total and of the GPU time: 8 ms. **Not a gate on Vulkan** (the owner, 2026-10-30): it is
/// printed beside each figure so the report can say how far off a figure is.
pub(crate) const REFERENCE_P95_NS: u64 = 8_000_000;
/// Warm-up, never sampled.
pub(crate) const WARM_UP_NS: u64 = 5_000_000_000;
/// The sample window.
pub(crate) const SAMPLE_NS: u64 = 30_000_000_000;
/// The frozen minimum qualifying samples of the static phases (60 Hz would give 1,800 and 1,800). Short of them a reading is reported as
/// insufficient data, never a pass or a fail.
pub(crate) const MIN_CPU_SAMPLES: u64 = 1_500;
/// See [`MIN_CPU_SAMPLES`].
pub(crate) const MIN_GPU_SAMPLES: u64 = 1_500;
/// See [`MIN_CPU_SAMPLES`]: presentation intervals.
pub(crate) const MIN_INTERVALS: u64 = 1_400;

/// A percentile as a record.
fn percentile(value: Percentile) -> J {
    match value {
        Percentile::NoValue => J::o([("kind", J::s("no_value"))]),
        Percentile::Value {
            upper_bound_ns,
            error_ns,
        } => J::o([
            ("kind", J::s("value")),
            ("upper_bound_ns", J::U(upper_bound_ns)),
            ("error_ns", J::U(error_ns)),
        ]),
        other @ Percentile::OutOfRange { .. } => J::o([
            ("kind", J::s("out_of_range")),
            ("detail", J::s(format!("{other:?}"))),
        ]),
    }
}

/// One histogram: its sample count and the nearest-rank p50, p95, p99 and the exact maximum.
pub(crate) fn histogram(h: &Histogram) -> J {
    J::o([
        ("count", J::U(h.count())),
        ("p50", percentile(h.percentile(500))),
        ("p95", percentile(h.percentile(950))),
        ("p99", percentile(h.percentile(990))),
        ("max_ns", h.max_ns().map_or(J::Null, J::U)),
        ("overflow", J::U(h.overflow())),
        ("saturated", J::Bool(h.saturated())),
    ])
}

/// The p95 upper bound of `h` in nanoseconds, or `None` for no value or an out-of-range rank (which never passes a gate).
pub(crate) fn p95_ns(h: &Histogram) -> Option<u64> {
    match h.percentile(950) {
        Percentile::Value { upper_bound_ns, .. } => Some(upper_bound_ns),
        _ => None,
    }
}

/// Every figure of one window's statistics.
pub(crate) fn frame_stats(stats: &FrameStats) -> J {
    J::o([
        ("frames", J::U(stats.frames)),
        ("skipped", J::U(stats.skipped)),
        ("invalid_marks", J::U(stats.invalid_marks)),
        ("invalid_gpu", J::U(stats.invalid_gpu)),
        ("presented", J::U(stats.presented)),
        ("not_presented", J::U(stats.not_presented)),
        ("presentation_expired", J::U(stats.presentation_expired)),
        ("outstanding", J::U(stats.outstanding)),
        ("instances_sum", J::U(stats.instances_sum)),
        ("instances_max", J::U(stats.instances_max)),
        ("draw_calls_sum", J::U(stats.draw_calls_sum)),
        ("draw_calls_max", J::U(stats.draw_calls_max)),
        ("texture_binds", J::U(stats.texture_binds)),
        ("checker_instances", J::U(stats.checker_instances)),
        ("cpu_total", histogram(&stats.cpu_total)),
        ("cpu_build", histogram(&stats.build)),
        ("cpu_encode", histogram(&stats.encode)),
        ("cpu_ledger", histogram(&stats.ledger)),
        ("acquire", histogram(&stats.acquire)),
        ("gpu", histogram(&stats.gpu)),
        (
            "presentation_interval",
            histogram(&stats.presentation_interval),
        ),
    ])
}

/// A reading with its window's own bookkeeping, and the sources the device does not offer said as such.
pub(crate) fn reading(reading: &MetricsReading) -> J {
    let window = &reading.window;
    J::o([
        ("id", J::U(u64::from(window.window.id))),
        ("start_ns", J::U(window.window.start)),
        ("end_ns", window.window.end.map_or(J::Null, J::U)),
        ("adopted_at_ns", window.adopted_at.map_or(J::Null, J::U)),
        ("late_adopted", J::Bool(window.late_adopted)),
        ("outside", J::U(window.outside)),
        ("dropped_late", J::U(window.dropped_late)),
        ("end_overrun", J::Bool(window.end_overrun)),
        ("last_admitted_ns", J::U(window.last_admitted_ns)),
        ("complete", J::Bool(reading.complete)),
        ("outstanding", J::U(reading.outstanding)),
        ("gpu_time", unavailable(reading.gpu_unavailable)),
        (
            "presentation",
            unavailable(reading.presentation_unavailable),
        ),
        ("stats", frame_stats(&window.stats)),
    ])
}

/// `"available"`, or the explicit `"unavailable"` with the reason (never a zero in its place).
fn unavailable(reason: Option<&str>) -> J {
    reason.map_or_else(
        || J::s("available"),
        |reason| J::o([("status", J::s("unavailable")), ("reason", J::s(reason))]),
    )
}

/// What the presenter's counters and charges were. `allocations` is the context's live `vkAllocateMemory` count and the device's
/// `maxMemoryAllocationCount`, read through the existing accessor.
pub(crate) fn scene_stats(stats: &SceneStats, allocations: Option<(u32, u32)>) -> J {
    let ready_span = stats.last_ready_ns.saturating_sub(stats.first_ready_ns);
    let not_applicable = || J::s("not_applicable");
    J::o([
        ("frames", J::U(stats.frames)),
        ("textures_created", J::U(stats.textures_created)),
        ("textures_ready", J::U(stats.textures_ready)),
        ("uploads_submitted", J::U(stats.upload_batches_submitted)),
        ("uploads_completed", J::U(stats.upload_batches_completed)),
        ("upload_texture_bytes_peak", J::U(stats.texture_bytes_peak)),
        ("upload_staging_bytes_peak", J::U(stats.staging_bytes_peak)),
        ("checker_texture_bytes", J::U(stats.checker_bytes)),
        ("bindless_slots", J::U(u64::from(stats.bindless_slots))),
        ("first_ready_ns", J::U(stats.first_ready_ns)),
        ("last_ready_ns", J::U(stats.last_ready_ns)),
        ("ready_span_ns", J::U(ready_span)),
        (
            "textures_ready_per_second_over_ready_span",
            rate(stats.textures_ready, ready_span),
        ),
        (
            "upload_batches_completed_per_second_over_ready_span",
            rate(stats.upload_batches_completed, ready_span),
        ),
        (
            "textures_ready_per_completed_upload_batch",
            ratio(stats.textures_ready, stats.upload_batches_completed),
        ),
        // Vulkan has no residency sets and no resource-ID table buffer: these Metal figures do not exist here.
        ("residency_set_bytes", not_applicable()),
        ("bindless_table_bytes", not_applicable()),
        ("residency_changes", not_applicable()),
        (
            "device_memory_allocations",
            allocations.map_or(J::Null, |(live, limit)| {
                J::o([
                    ("live_vkAllocateMemory", J::U(u64::from(live))),
                    ("maxMemoryAllocationCount", J::U(u64::from(limit))),
                ])
            }),
        ),
    ])
}

/// `count` per second over `span_ns`, or null when the span is empty (nothing was observed to rate).
fn rate(count: u64, span_ns: u64) -> J {
    if span_ns == 0 {
        return J::Null;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a rate for a report: counts and spans are far below 2^52"
    )]
    J::F(count as f64 * 1e9 / span_ns as f64)
}

/// `count` over `per`, or null when `per` is zero.
fn ratio(count: u64, per: u64) -> J {
    if per == 0 {
        return J::Null;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a ratio for a report: counts are far below 2^52"
    )]
    J::F(count as f64 / per as f64)
}

/// One of the resource service's budgets as a record: its limit, the most reserved at once and the refusals.
fn budget(stats: &BudgetStats) -> J {
    J::o([
        ("limit", J::U(stats.limit)),
        ("reserved_now", J::U(stats.reserved)),
        ("peak", J::U(stats.peak)),
        ("refusals", J::U(stats.refusals)),
    ])
}

/// What the resource service's budgets, registry and loaders had done: peak reservations, the loader queue's peak and the deferrals.
pub(crate) fn service_stats(stats: &ServiceStats) -> J {
    J::o([
        ("jobs", budget(&stats.jobs)),
        ("cpu_pixels", budget(&stats.cpu_pixels)),
        ("metadata", budget(&stats.metadata)),
        (
            "registry",
            J::o([
                ("capacity", J::U(u64::from(stats.registry.capacity))),
                ("live", J::U(u64::from(stats.registry.live))),
                ("retiring", J::U(u64::from(stats.registry.retiring))),
                ("exhausted", J::U(u64::from(stats.registry.exhausted))),
                ("free", J::U(u64::from(stats.registry.free))),
            ]),
        ),
        (
            "loaders",
            J::o([
                ("files_decoded", J::U(stats.loaders.files_decoded)),
                ("decoded_bytes", J::U(stats.loaders.decoded_bytes)),
                ("queue_depth_peak", J::U(stats.loaders.queue_depth_peak)),
                ("deferrals", J::U(stats.loaders.deferrals)),
                ("deferred_now", J::U(stats.loaders.deferred_now as u64)),
                (
                    "reservation_attempts",
                    J::U(stats.loaders.reservation_attempts),
                ),
                ("job_panics", J::U(stats.loaders.job_panics)),
                ("inline_finishes", J::U(stats.loaders.inline_finishes)),
            ]),
        ),
    ])
}

/// The process's `phys_footprint` in bytes (`proc_pid_rusage`, flavour 4), or `None` when the call fails.
#[expect(
    unsafe_code,
    reason = "one libproc call into a buffer sized for rusage_info_v4; the field is read at its documented offset"
)]
#[must_use]
pub(crate) fn footprint_bytes() -> Option<u64> {
    unsafe extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut u8) -> i32;
    }
    // `rusage_info_v4` is 304 bytes; `ri_phys_footprint` follows the uuid (16), the user and system times, the two wakeup counts, the
    // page-ins, the wired and the resident sizes (7 x 8): offset 72.
    let mut buffer = [0_u8; 512];
    let pid = i32::try_from(std::process::id()).ok()?;
    // SAFETY: the buffer is larger than `rusage_info_v4` and lives for the call.
    let status = unsafe { proc_pid_rusage(pid, 4, buffer.as_mut_ptr()) };
    (status == 0).then(|| u64::from_ne_bytes(buffer[72..80].try_into().expect("eight bytes")))
}

/// One reference input: whether `value` is under the §11.3 reference, with the value.
fn under(value: Option<u64>) -> (bool, J) {
    (
        value.is_some_and(|value| value < REFERENCE_P95_NS),
        value.map_or(J::Null, J::U),
    )
}

/// The report's reading of a static phase against §11.3's targets: CPU total p95 and GPU p95 under 8 ms, with the frozen minimum
/// sample counts. **Not a gate on Vulkan**: the verdict never fails a phase (`gated: false`); it says whether the reference was met, and
/// where the data fall short of the counts it says **insufficient**, which is data, not a pass or a fail. A source the device does not
/// offer (no GPU timestamps, no presentation feedback) is `unavailable` for that figure.
#[must_use]
pub(crate) fn reference_reading(reading: &MetricsReading) -> J {
    let stats = &reading.window.stats;
    let mut insufficient: Vec<J> = Vec::new();
    if !reading.complete {
        insufficient.push(J::s("the reading was not complete at the bound"));
    }
    if reading.window.late_adopted {
        insufficient.push(J::s("the window was adopted late"));
    }
    if stats.cpu_total.count() < MIN_CPU_SAMPLES {
        insufficient.push(J::s("too few CPU-total samples"));
    }
    if reading.gpu_unavailable.is_none() && stats.gpu.count() < MIN_GPU_SAMPLES {
        insufficient.push(J::s("too few GPU samples"));
    }
    if reading.presentation_unavailable.is_none()
        && stats.presentation_interval.count() < MIN_INTERVALS
    {
        insufficient.push(J::s("too few presentation intervals"));
    }
    let (cpu_ok, cpu) = under(p95_ns(&stats.cpu_total));
    let (gpu_ok, gpu) = under(p95_ns(&stats.gpu));
    J::o([
        ("gated", J::Bool(false)),
        (
            "note",
            J::s(
                "the §11.3 reference printed beside the figure; not a gate on Vulkan (owner, 2026-10-30)",
            ),
        ),
        ("reference_p95_ns", J::U(REFERENCE_P95_NS)),
        ("cpu_total_p95_upper_bound_ns", cpu),
        ("cpu_total_under_reference", J::Bool(cpu_ok)),
        (
            "gpu_p95_upper_bound_ns",
            if reading.gpu_unavailable.is_some() {
                J::s("unavailable")
            } else {
                gpu
            },
        ),
        (
            "gpu_under_reference",
            if reading.gpu_unavailable.is_some() {
                J::s("unavailable")
            } else {
                J::Bool(gpu_ok)
            },
        ),
        ("insufficient_data", J::A(insufficient)),
    ])
}
