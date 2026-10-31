//! How the harness turns the presenter's windows into the evidence: histograms to percentiles, readings to records, the static gate's
//! verdict, the process footprint, and the pieces that every phase shares.

use ck_render_2d::{FrameStats, Histogram, MetricsReading, Percentile, SceneStats};
use ck_resources::{BudgetStats, ServiceStats};

use crate::json::J;

/// The static gate: p95 of the CPU total and of the GPU time under 8 ms.
pub(crate) const GATE_P95_NS: u64 = 8_000_000;
/// Warm-up, never sampled.
pub(crate) const WARM_UP_NS: u64 = 5_000_000_000;
/// The sample window.
pub(crate) const SAMPLE_NS: u64 = 30_000_000_000;
/// The frozen minimum qualifying samples of the static phases (60 Hz would give 1,800 and 1,800).
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
        ("excluded_intervals", J::U(stats.excluded_intervals)),
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

/// A reading with its window's own bookkeeping.
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
        ("complete", J::Bool(reading.complete)),
        ("outstanding", J::U(reading.outstanding)),
        ("timing_lost", J::U(reading.timing_lost)),
        ("evidence_lost", J::Bool(reading.evidence_lost)),
        ("closing_pending", J::Bool(reading.closing_pending)),
        ("stats", frame_stats(&window.stats)),
    ])
}

/// What the presenter's counters and charges were.
pub(crate) fn scene_stats(stats: &SceneStats) -> J {
    let ready_span = stats.last_ready_ns.saturating_sub(stats.first_ready_ns);
    J::o([
        ("frames", J::U(stats.frames)),
        ("last_instances", J::U(stats.last.instances)),
        ("last_batches", J::U(stats.last.batches)),
        ("last_texture_binds", J::U(stats.last.texture_binds)),
        ("last_resolved", J::U(stats.last.resolved)),
        ("last_checker", J::U(stats.last.checker)),
        ("last_skipped", J::U(stats.last.skipped)),
        ("last_dropped", J::U(stats.last.dropped)),
        ("last_slot_skipped", J::U(stats.last.slot_skipped)),
        ("last_pinned", J::U(stats.last.pinned)),
        ("peak_instances", J::U(stats.peak.instances)),
        ("peak_pinned", J::U(stats.peak.pinned)),
        ("instance_buffer_bytes", J::U(stats.instance_buffer_bytes)),
        (
            "instance_buffer_allocated",
            J::U(stats.instance_buffer_allocated),
        ),
        ("binding_geometry_bytes", J::U(stats.binding_geometry_bytes)),
        ("textures_created", J::U(stats.textures_created)),
        ("textures_ready", J::U(stats.textures_ready)),
        ("uploads_submitted", J::U(stats.uploads_submitted)),
        ("uploads_completed", J::U(stats.uploads_completed)),
        (
            "upload_texture_bytes_peak",
            J::U(stats.upload_texture_bytes_peak),
        ),
        (
            "upload_staging_bytes_peak",
            J::U(stats.upload_staging_bytes_peak),
        ),
        ("checker_texture_bytes", J::U(stats.checker_texture_bytes)),
        ("checker_staging_bytes", J::U(stats.checker_staging_bytes)),
        ("residency_set_bytes", J::U(stats.residency_set_bytes)),
        ("bindless_table_bytes", J::U(stats.bindless_table_bytes)),
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
            rate(stats.uploads_completed, ready_span),
        ),
        (
            "textures_ready_per_completed_upload_batch",
            ratio(stats.textures_ready, stats.uploads_completed),
        ),
        // The presenter's counters hold the residency set's size only; no add, remove or commit count is exposed to the harness.
        ("residency_changes", J::Null),
        (
            "residency_changes_note",
            J::s(
                "unavailable and waived by the owner (CK-130 review 1, F2/F4, relayed by ck-manager): the presenter exposes the residency set's size (residency_set_bytes), not a count of its edits; the part rests on the CK-113/CK-115 component evidence, and counter-level debugging is for external tools",
            ),
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

/// One gate input: whether `value` is under the gate, with the value.
fn under(value: Option<u64>) -> (bool, J) {
    (
        value.is_some_and(|value| value < GATE_P95_NS),
        value.map_or(J::Null, J::U),
    )
}

/// The static gate's verdict on a reading (S-bindless): CPU total p95 and GPU p95 under 8 ms with the frozen minimum sample counts,
/// a complete reading and a window that was not adopted late. A reading short of the counts is **insufficient**, never a pass.
#[must_use]
pub(crate) fn static_verdict(reading: &MetricsReading) -> (bool, J) {
    let stats = &reading.window.stats;
    let mut reasons: Vec<J> = Vec::new();
    let mut insufficient = false;
    let mut note = |why: &str, insufficient_for: bool| {
        reasons.push(J::s(why));
        insufficient |= insufficient_for;
    };
    if !reading.complete {
        note("the reading was not complete at the bound", true);
    }
    if reading.window.late_adopted {
        note("the window was adopted late", true);
    }
    if reading.evidence_lost || reading.timing_lost != 0 {
        note("timing evidence was lost", true);
    }
    if stats.cpu_total.count() < MIN_CPU_SAMPLES {
        note("too few CPU-total samples", true);
    }
    if stats.gpu.count() < MIN_GPU_SAMPLES {
        note("too few GPU samples", true);
    }
    if stats.presentation_interval.count() < MIN_INTERVALS {
        note("too few presentation intervals", true);
    }
    let (cpu_ok, cpu) = under(p95_ns(&stats.cpu_total));
    let (gpu_ok, gpu) = under(p95_ns(&stats.gpu));
    if !cpu_ok {
        note("CPU-total p95 is not under 8 ms", false);
    }
    if !gpu_ok {
        note("GPU p95 is not under 8 ms", false);
    }
    let passed = reasons.is_empty();
    let verdict = J::o([
        ("passed", J::Bool(passed)),
        ("insufficient", J::Bool(insufficient)),
        ("cpu_total_p95_upper_bound_ns", cpu),
        ("gpu_p95_upper_bound_ns", gpu),
        ("gate_ns", J::U(GATE_P95_NS)),
        ("reasons", J::A(reasons)),
    ]);
    (passed, verdict)
}
