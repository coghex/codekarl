//! The phases of the milestone 5.5 scale run on Vulkan (CK-162, part C; CK-130's phases), each run by one worker on the scene thread of
//! `rig`. **Correctness passes or fails; timings are measured and reported, never gated** (owner, 2026-10-30): a figure is printed
//! beside the §11.3 reference and a reading short of its sample counts is reported as insufficient data.
//!
//! Every phase first loads what it draws (with a foreground quad moving at 60 Hz so that frames keep flowing and textures keep being
//! adopted), then does its work on a fixed 60 Hz deadline schedule, and its measurement window is the common one of CK-129 r3: opened
//! at `T0 + 5 s` (warm-up, never sampled) and ended at `start + 30 s`, adopted before `start`, read after `end` within 2 s.

use std::time::{Duration, Instant};

use ck_gpu_vulkan::FrameCapture;
use ck_render_2d::QuadKey;
use ck_render_2d::vulkan_timing::{MetricsReading, MetricsWindow};
use ck_resources::{AdmitError, CpuPixels, ResourceService, StatusState, TextureKey};

use crate::diagnostic::Cell;
use crate::evidence::{self, SAMPLE_NS, WARM_UP_NS};
use crate::json::J;
use crate::layout;
use crate::rig::{Cadence, Committed, PERIOD_NS, Scene, now_ns};

/// How long a worker may wait for a window to open.
const OPEN: Duration = Duration::from_secs(60);
/// K2's reloads: one more than the 64 extra revisions the backend may hold.
const K2_RELOADS: usize = 65;
/// The presenter counts a frame after its native step has committed and returned, so the count read at a fence can lag by the one
/// frame the (single, sequential) render thread has committed but not yet counted: a frame is taken as committed after a fence only
/// from this many frames above the count read.
const FENCE_MARGIN_FRAMES: u64 = 1;
/// K2 polls the statuses and takes the captures made since the last poll this many times per 60 Hz tick.
const K2_POLLS_PER_TICK: u32 = 4;
/// The bytes of one block of the Vulkan image pool (`crates/gpu-vulkan/src/memory.rs`, `BLOCK_SIZE`). `texture_bytes` is reserved in whole
/// blocks (and in dedicated allocations at their queried size), and the checker, uploaded when the presenter starts, holds a range of the
/// first block for the presenter's whole life, so one block is reserved from the start. Metal counts per image, which is why CK-130's
/// 64 MiB ceiling does not carry over (CK-162 part C, scale run 2).
pub(crate) const POOL_BLOCK_BYTES: u64 = 64 * 1024 * 1024;
/// Phase K's `texture_bytes`: the pool's first block plus room for four of K3's 16 MiB images. A recorded departure from CK-130's 64 MiB,
/// which is exactly one block and so leaves no room for any K3 image.
pub(crate) const CAPS_TEXTURE_BYTES: u64 = POOL_BLOCK_BYTES + 4 * K3_IMAGE_BYTES;
/// What `caps-min` runs with: CK-130's ceiling, one block.
pub(crate) const CAPS_MIN_TEXTURE_BYTES: u64 = POOL_BLOCK_BYTES;
/// K3's requests: the eight 2048 x 2048 images of set B.
pub(crate) const K3_IMAGES: u32 = 8;
/// One of them in bytes (RGBA8).
pub(crate) const K3_IMAGE_BYTES: u64 = 2048 * 2048 * 4;
/// How many decoded K2 frames are kept (their commit ordinals) for the coverage verdict: far more than the seconds K2 lasts at 60 Hz need; past
/// it the evidence is not retained and K2 fails by saying so.
const K2_MAX_FRAMES: usize = 4096;
/// How long a capture may wait for the presenter's record of its committed frame before it is called unidentified (the record is made by
/// another thread, so it can lag the capture a little).
const IDENTIFY_BOUND: Duration = Duration::from_secs(2);
/// How long all of a set may take to become ready.
const READY_BOUND: Duration = Duration::from_secs(300);
/// §11.3's reference for the warm load rate: 1,024 loads per second. Not a gate on Vulkan.
const LOAD_REFERENCE_PER_SECOND: f64 = 1024.0;
/// CK-130's accepted warm rate on Metal (`runtime-m5/ck130-worker-result.md`): 7,692 loads per second, the parity figure.
const METAL_WARM_LOADS_PER_SECOND: f64 = 7_692.0;
/// The read after a window's end waits at most this long (the recipe's `within: 2 s`).
const READ_WITHIN: Duration = Duration::from_secs(2);
/// Creations per update of the load phases.
const PER_UPDATE: usize = 128;
/// The points a drawable of `pixels` pixels is when the screen's backing scale is `scale`.
fn points(pixels: u32, scale: u32) -> u32 {
    pixels / scale
}

/// The window's size in points (960 x 540 at the scale 2 of the owner's M3 Max).
#[must_use]
pub(crate) fn window_points() -> (u32, u32) {
    (
        points(layout::DRAWABLE_WIDTH, 2),
        points(layout::DRAWABLE_HEIGHT, 2),
    )
}

/// Checks that the drawable is the recipe's 1920 x 1080 and returns the backing scale it implies.
fn check_drawable(scene: &Scene) -> Result<(u32, f64), String> {
    let (width, height) = scene.wait_open(OPEN)?;
    if (width, height) != (layout::DRAWABLE_WIDTH, layout::DRAWABLE_HEIGHT) {
        return Err(format!(
            "the drawable is {width} x {height}, not {} x {} (the recipe needs the screen's backing scale to be 2)",
            layout::DRAWABLE_WIDTH,
            layout::DRAWABLE_HEIGHT
        ));
    }
    let (points_width, _) = window_points();
    Ok((width, f64::from(width) / f64::from(points_width)))
}

/// The scale of drawable pixels to points of this run (the quads are placed in points).
#[derive(Clone, Copy)]
struct Units(f64);

impl Units {
    fn point(self, pixels: f64) -> f64 {
        pixels / self.0
    }
}

// ---------------------------------------------------------------------------------------------
// The foreground quad
// ---------------------------------------------------------------------------------------------

/// The textures the foreground quad adds to the store (one 8 x 8 image).
const FOREGROUND_TEXTURES: u64 = 1;

/// The images the store has created for uploads once set A is loaded under the foreground quad: set A's, the foreground's and, when it is
/// resident, the checker (the Vulkan presenter counts the checker in `textures_created`; the Metal presenter's count does not, so the
/// Metal harness's expectation without it does not hold here).
pub(crate) fn expected_textures_created(checker_resident: bool) -> u64 {
    u64::from(layout::SET_A) + FOREGROUND_TEXTURES + u64::from(checker_resident)
}

/// A small quad that moves along a scripted path (a Lissajous curve), published every tick: it keeps frames flowing while textures
/// load, so adoption never waits for a quad that does not exist.
struct Foreground {
    key: QuadKey,
    texture: TextureKey,
    units: Units,
    step: u64,
}

impl Foreground {
    fn new(scene: &mut Scene, units: Units) -> Result<Self, String> {
        let reservation = scene
            .service
            .reserve_pixels(8, 8)
            .map_err(|error| format!("the foreground pixels: {error:?}"))?;
        let pixels = CpuPixels::premultiplied(reservation, [20, 200, 20, 255].repeat(64))
            .map_err(|error| format!("the foreground pixels: {error:?}"))?;
        let texture = scene
            .service
            .create_from_pixels(pixels)
            .map_err(|_| "the foreground texture was refused".to_owned())?;
        Self::over(scene, units, texture)
    }

    /// A foreground over a texture the phase already loaded.
    fn over(scene: &mut Scene, units: Units, texture: TextureKey) -> Result<Self, String> {
        let reference = scene
            .service
            .retain(texture)
            .map_err(|error| format!("the foreground reference: {error:?}"))?;
        let key = scene
            .writer
            .create_quad(reference, 40.0, 40.0, 40.0, 40.0)
            .map_err(|error| format!("the foreground quad: {error:?}"))?;
        Ok(Self {
            key,
            texture,
            units,
            step: 0,
        })
    }

    /// One scripted move, published.
    fn tick(&mut self, scene: &mut Scene) {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a small step number becomes an angle"
        )]
        let angle = self.step as f64 * 0.05;
        self.step += 1;
        let (width, height) = (
            self.units.point(f64::from(layout::DRAWABLE_WIDTH)),
            self.units.point(f64::from(layout::DRAWABLE_HEIGHT)),
        );
        let x = width / 2.0 + (width / 2.0 - 60.0) * (angle * 1.0).sin();
        let y = height / 2.0 + (height / 2.0 - 60.0) * (angle * 1.3).cos();
        // A finite position of a live key: it cannot fail.
        let _ = scene.writer.set_position(self.key, x, y);
        let _ = scene.writer.publish();
    }

    /// Takes the quad out of the scene (its reference goes with it) and publishes.
    fn remove(self, scene: &mut Scene) {
        let _ = scene.writer.destroy_quad(self.key);
        let _ = scene.writer.publish();
        let _ = scene.service.release(self.texture);
    }
}

// ---------------------------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------------------------

/// What a load run measured by the service's and the presenter's own clocks.
struct Loaded {
    keys: Vec<TextureKey>,
    /// The clock just before the first `load`: the ledger's own clock, the one every adoption time below is stamped on (read through
    /// `ck_gpu_vulkan::ledger_clock_ns`, which is also the harness's clock), so the load's span is one subtraction on one clock.
    first_load_ns: u64,
    /// The presenter's first and last adoption boundary (the first is the foreground texture's, adopted before the load began).
    first_ready_ns: u64,
    last_ready_ns: u64,
    /// Loads the service refused, by reason.
    refused: Vec<(String, u64)>,
}

fn count(refused: &mut Vec<(String, u64)>, reason: String) {
    match refused.iter_mut().find(|(name, _)| *name == reason) {
        Some((_, n)) => *n += 1,
        None => refused.push((reason, 1)),
    }
}

/// A short name for the reason of an admission refusal.
fn reason(error: &AdmitError) -> String {
    match error {
        AdmitError::RegistryFull => "texture count (registry full)".to_owned(),
        AdmitError::JobsFull => "job slots".to_owned(),
        AdmitError::Capacity { budget, .. } => format!("budget {budget:?}"),
        other => format!("{other:?}"),
    }
}

/// K3's verdict: at least one of the big images is ready and at least one is refused, every refusal has its message, and every message
/// is the backend's texture-bytes reason. A refusal with no text, or for another reason (the allocation-count limit, a native device
/// failure, another budget), is not a texture-bytes refusal.
pub(crate) fn k3_verdict(ready: usize, failed: usize, messages: &[String]) -> bool {
    ready >= 1
        && failed >= 1
        && messages.len() == failed
        && messages
            .iter()
            .all(|message| message.contains(TEXTURE_BYTES_REASON))
}

/// The reason the Vulkan backend gives a texture the texture-bytes budget refuses (`crates/gpu-vulkan/src/textures.rs`).
const TEXTURE_BYTES_REASON: &str = "texture_bytes budget:";

/// Loads `names` at `per_update` creations per tick of a 60 Hz schedule (the foreground moving) and waits until every admitted one is
/// ready or `READY_BOUND` passes. A refusal is counted, not an error (the caps phase expects some).
fn load_all(
    scene: &mut Scene,
    names: &[String],
    per_update: usize,
    foreground: &mut Foreground,
    expect_all: bool,
) -> Result<Loaded, String> {
    let mut cadence = Cadence::starting_at(now_ns());
    let before = scene.stats().map_or(0, |stats| stats.textures_ready);
    let first_load_ns = now_ns();
    let mut keys = Vec::with_capacity(names.len());
    let mut refused = Vec::new();
    let mut next = 0;
    let limit = Instant::now() + READY_BOUND;
    loop {
        if let Some(message) = scene.failure() {
            return Err(format!("the presenter failed while loading: {message}"));
        }
        let end = (next + per_update).min(names.len());
        for name in &names[next..end] {
            match scene.service.load(name) {
                Ok(key) => keys.push(key),
                Err(error) => count(&mut refused, reason(&error)),
            }
        }
        next = end;
        foreground.tick(scene);
        cadence.tick();
        if next == names.len() {
            let (ready, failed) = ready_and_failed(&scene.service, &keys);
            if failed != 0 {
                return Err(format!("{failed} of the loaded textures failed"));
            }
            if ready == keys.len() {
                break;
            }
        }
        if Instant::now() >= limit {
            let (ready, _) = ready_and_failed(&scene.service, &keys);
            return Err(format!(
                "only {ready} of {} textures were ready after {READY_BOUND:?}",
                keys.len()
            ));
        }
    }
    if expect_all && keys.len() != names.len() {
        return Err(format!(
            "{} of {} loads were refused: {refused:?}",
            names.len() - keys.len(),
            names.len()
        ));
    }
    // The service says ready once the upload completed; the presenter counts the revision at its next boundary, a frame later at most.
    let settle = Instant::now() + Duration::from_secs(10);
    let stats = loop {
        let stats = scene
            .stats()
            .ok_or("the presenter gave no stats after the load")?;
        if stats.textures_ready >= before + keys.len() as u64 {
            break stats;
        }
        if Instant::now() >= settle {
            return Err(format!(
                "the presenter adopted {} revisions for {} loads",
                stats.textures_ready - before,
                keys.len()
            ));
        }
        foreground.tick(scene);
        cadence.tick();
    };
    Ok(Loaded {
        keys,
        first_load_ns,
        first_ready_ns: stats.first_ready_ns,
        last_ready_ns: stats.last_ready_ns,
        refused,
    })
}

/// The number of `keys` that are ready and the number that failed.
fn ready_and_failed(service: &ResourceService, keys: &[TextureKey]) -> (usize, usize) {
    let (mut ready, mut failed) = (0, 0);
    for key in keys {
        match service.status(*key).map(|status| status.state) {
            Ok(StatusState::Ready) => ready += 1,
            Ok(StatusState::Failed) => failed += 1,
            _ => {}
        }
    }
    (ready, failed)
}

/// Texture names of set A, `0 .. count`.
fn set_a(count: u32) -> Vec<String> {
    (0..count).map(layout::set_a_name).collect()
}

/// A record of a load run: the service's and the presenter's own timestamps and the rate between them.
fn load_record(loaded: &Loaded, label: &str) -> J {
    let span_ns = loaded.last_ready_ns.saturating_sub(loaded.first_load_ns);
    #[expect(
        clippy::cast_precision_loss,
        reason = "counts and spans far below 2^53 become a rate"
    )]
    let rate = if span_ns == 0 {
        0.0
    } else {
        loaded.keys.len() as f64 / (span_ns as f64 / 1e9)
    };
    J::o([
        ("label", J::s(label)),
        ("loaded", J::U(loaded.keys.len() as u64)),
        ("first_load_ns", J::U(loaded.first_load_ns)),
        ("first_ready_ns", J::U(loaded.first_ready_ns)),
        ("last_ready_ns", J::U(loaded.last_ready_ns)),
        ("first_load_to_last_ready_ns", J::U(span_ns)),
        ("loads_per_second", J::F(rate)),
        (
            "refused",
            J::A(
                loaded
                    .refused
                    .iter()
                    .map(|(reason, n)| {
                        J::o([("reason", J::s(reason.clone())), ("count", J::U(*n))])
                    })
                    .collect(),
            ),
        ),
    ])
}

// ---------------------------------------------------------------------------------------------
// The scene
// ---------------------------------------------------------------------------------------------

/// The full scene: 65,536 quads over `keys` (set A in order), created and published once.
fn build_full_scene(
    scene: &mut Scene,
    keys: &[TextureKey],
    units: Units,
) -> Result<Vec<QuadKey>, String> {
    let mut quads = Vec::with_capacity(layout::SCENE_QUADS as usize);
    for index in 0..layout::SCENE_QUADS {
        let placed = layout::scene_quad(index, true);
        let reference = scene
            .service
            .retain(keys[placed.texture as usize])
            .map_err(|error| format!("quad {index}: {error:?}"))?;
        quads.push(
            scene
                .writer
                .create_quad(
                    reference,
                    units.point(placed.x),
                    units.point(placed.y),
                    units.point(placed.width),
                    units.point(placed.height),
                )
                .map_err(|error| format!("quad {index}: {error:?}"))?,
        );
    }
    Ok(quads)
}

/// Builds a CK-168 scene using a uniform texture-index mapping and CK-130's fixed geometry.
fn build_diagnostic_scene(
    scene: &mut Scene,
    keys: &[TextureKey],
    cell: Cell,
    units: Units,
) -> Result<Vec<QuadKey>, String> {
    let mut quads = Vec::with_capacity(cell.quads as usize);
    for index in 0..cell.quads {
        let placed = crate::diagnostic::placement(index, cell.sampled_textures)
            .ok_or_else(|| format!("diagnostic quad {index} has no valid texture mapping"))?;
        let reference = scene
            .service
            .retain(keys[placed.texture as usize])
            .map_err(|error| format!("diagnostic quad {index}: {error:?}"))?;
        quads.push(
            scene
                .writer
                .create_quad(
                    reference,
                    units.point(placed.x),
                    units.point(placed.y),
                    units.point(placed.width),
                    units.point(placed.height),
                )
                .map_err(|error| format!("diagnostic quad {index}: {error:?}"))?,
        );
    }
    Ok(quads)
}

/// One CK-168 matrix cell: all textures become ready before the uniform sample mapping is drawn and measured.
#[expect(
    clippy::too_many_lines,
    reason = "one diagnostic cell is a single ordered load, draw, measure and evidence protocol"
)]
pub(crate) fn diagnostic_scene(scene: &mut Scene, cell: Cell) -> Result<J, String> {
    let (_, scale) = check_drawable(scene)?;
    let units = Units(scale);
    let mut foreground = Foreground::new(scene, units)?;
    let loaded = load_all(
        scene,
        &set_a(cell.live_textures),
        PER_UPDATE,
        &mut foreground,
        true,
    )?;
    foreground.remove(scene);
    let quads = build_diagnostic_scene(scene, &loaded.keys, cell, units)?;
    let _ = scene.writer.publish();
    wait_drawn(scene, u64::from(cell.quads), Duration::from_secs(60))?;
    let first = crate::diagnostic::placement(0, cell.sampled_textures)
        .ok_or("diagnostic mapping for the first quad was invalid")?;
    let republish = Republish {
        key: quads[0],
        at: (units.point(first.x), units.point(first.y)),
    };
    let measured = measure(scene, 1, |scene, _| republish.tick(scene))?;
    let stats = scene.stats().ok_or("no presenter stats")?;
    let notice = scene.binding_notice();
    let expected_slots = if cell.ordinary {
        0
    } else {
        cell.bindless_slots
    };
    let mut reasons = Vec::new();
    if loaded.keys.len() != cell.live_textures as usize {
        reasons.push(format!(
            "loaded {} textures, expected {}",
            loaded.keys.len(),
            cell.live_textures
        ));
    }
    if stats.last.instances != u64::from(cell.quads)
        || stats.last.resolved != u64::from(cell.quads)
        || stats.last.checker != 0
    {
        reasons.push(format!(
            "last frame has {} instances, {} resolved and {} checker; expected {} resolved with none checker",
            stats.last.instances, stats.last.resolved, stats.last.checker, cell.quads
        ));
    }
    if stats.bindless_slots != expected_slots {
        reasons.push(format!(
            "presenter has {} bindless slots, expected {expected_slots}",
            stats.bindless_slots
        ));
    }
    if !cell.ordinary && !matches!(notice.as_ref(), Some(notice) if !notice.fallback) {
        reasons.push("bindless mode did not report a non-fallback binding notice".to_owned());
    }
    if !cell.ordinary && stats.last.batches != 1 {
        reasons.push(format!(
            "bindless frame has {} batches, expected one",
            stats.last.batches
        ));
    }
    let passed = reasons.is_empty();
    let max_textures = u64::try_from(cell.max_textures)
        .map_err(|error| format!("max_textures does not fit the evidence integer: {error}"))?;
    let mut record = J::o([
        ("phase", J::s("diagnostic")),
        ("cell", J::s(cell.name)),
        (
            "configuration",
            J::o([
                ("live_textures", J::U(u64::from(cell.live_textures))),
                ("sampled_textures", J::U(u64::from(cell.sampled_textures))),
                ("quads", J::U(u64::from(cell.quads))),
                (
                    "mode",
                    J::s(if cell.ordinary {
                        "per-revision"
                    } else {
                        "bindless"
                    }),
                ),
                ("bindless_slots", J::U(u64::from(cell.bindless_slots))),
                ("max_textures", J::U(max_textures)),
                ("max_extra_revisions", J::U(1_024)),
                (
                    "checker",
                    J::o([
                        ("resident", J::Bool(stats.checker_resident)),
                        ("bytes", J::U(stats.checker_bytes)),
                    ]),
                ),
                (
                    "foreground",
                    J::o([
                        ("textures", J::U(FOREGROUND_TEXTURES)),
                        ("removed_before_measurement", J::Bool(true)),
                    ]),
                ),
            ]),
        ),
        (
            "mode",
            J::o([
                ("bindless", J::Bool(!cell.ordinary)),
                ("bindless_slots", J::U(u64::from(stats.bindless_slots))),
                (
                    "binding_notice",
                    notice.as_ref().map_or(J::Null, |notice| {
                        J::o([
                            ("fallback", J::Bool(notice.fallback)),
                            ("text", J::s(notice.text.clone())),
                        ])
                    }),
                ),
                ("draw_calls_last_frame", J::U(stats.last.batches)),
                ("instances_last_frame", J::U(stats.last.instances)),
                ("checker_last_frame", J::U(stats.last.checker)),
            ]),
        ),
        ("load", load_record(&loaded, "CK-168 set A")),
        ("window", measured_record(&measured)),
        (
            "reference",
            measured
                .reading
                .as_ref()
                .map_or(J::Null, evidence::reference_reading),
        ),
        (
            "correctness",
            J::o([
                ("passed", J::Bool(passed)),
                ("reasons", J::A(reasons.into_iter().map(J::s).collect())),
                (
                    "distinct_sampled_textures",
                    J::U(u64::from(cell.sampled_textures)),
                ),
                ("checker", J::U(stats.last.checker)),
            ]),
        ),
        ("memory", memory(scene)),
    ]);
    record.push("gated", J::Bool(true));
    record.push("passed", J::Bool(passed));
    for quad in quads {
        let _ = scene.writer.destroy_quad(quad);
    }
    let _ = scene.writer.publish();
    for key in &loaded.keys {
        let _ = scene.service.release(*key);
    }
    Ok(record)
}

/// Waits until the newest frame the presenter has **queued for presentation** (its own record of a committed frame, never a build that
/// was abandoned or held back) has `instances` instances, all resolved to a shown revision (none the checker), and returns that frame.
/// The caller changes the scene to a count different from the one before it, so the newest committed frame is the new scene's.
fn wait_drawn(scene: &Scene, instances: u64, bound: Duration) -> Result<Committed, String> {
    let limit = Instant::now() + bound;
    loop {
        if let Some(stats) = scene.stats()
            && let Some(committed) = stats.committed
            && stats.last.instances == instances
            && stats.last.resolved == instances
            && stats.last.checker == 0
        {
            return Ok(committed);
        }
        if let Some(message) = scene.failure() {
            return Err(format!("the presenter failed: {message}"));
        }
        if Instant::now() >= limit {
            return Err(format!(
                "no committed frame of {instances} resolved instances in {bound:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------------------------

/// What a window run produced.
struct Measured {
    reading: Option<MetricsReading>,
    cadence: Cadence,
    adopted_before_start: bool,
    republished: u64,
}

/// Opens the common window (`T0 + 5 s` to `+ 30 s`) and runs `work` on every tick of the 60 Hz schedule until the window's end, then
/// reads it.
fn measure(
    scene: &mut Scene,
    id: u32,
    mut work: impl FnMut(&mut Scene, u64) -> bool,
) -> Result<Measured, String> {
    let t0 = now_ns();
    let start = t0 + WARM_UP_NS;
    let end = start + SAMPLE_NS;
    scene.request_window(MetricsWindow {
        id,
        start,
        end: Some(end),
    })?;
    let mut cadence = Cadence::starting_at(t0);
    let mut adopted = None;
    let mut republished = 0;
    loop {
        let now = cadence.tick();
        if adopted.is_none() {
            adopted = scene.adopted(id);
        }
        if work(scene, now) {
            republished += 1;
        }
        if now >= end + 3 * PERIOD_NS {
            break;
        }
        if let Some(message) = scene.failure() {
            return Err(format!("the presenter failed during the window: {message}"));
        }
    }
    let adopted_before_start = adopted.is_some_and(|at| at < start);
    let reading = scene.read_window(id, READ_WITHIN);
    Ok(Measured {
        reading,
        cadence,
        adopted_before_start,
        republished,
    })
}

fn measured_record(measured: &Measured) -> J {
    J::o([
        (
            "reading",
            measured.reading.as_ref().map_or(J::Null, evidence::reading),
        ),
        (
            "adopted_before_start",
            J::Bool(measured.adopted_before_start),
        ),
        ("republications", J::U(measured.republished)),
        (
            "schedule",
            J::o([
                ("ticks", J::U(measured.cadence.ticks)),
                ("late_ticks", J::U(measured.cadence.late)),
                ("worst_late_ns", J::U(measured.cadence.worst_late_ns)),
                ("period_ns", J::U(PERIOD_NS)),
            ]),
        ),
    ])
}

/// The run's memory and the presenter's charges at the end of a phase.
fn memory(scene: &Scene) -> J {
    let mut record = J::o([(
        "phys_footprint_bytes",
        evidence::footprint_bytes().map_or(J::Null, J::U),
    )]);
    if let Some(stats) = scene.stats() {
        record.push(
            "presenter",
            evidence::scene_stats(&stats.scene, scene.allocations()),
        );
    }
    record.push("service", evidence::service_stats(&scene.service.stats()));
    record
}

/// A republication of the identical scene, a new generation each time: the first quad is set to the position it has.
struct Republish {
    key: QuadKey,
    at: (f64, f64),
}

impl Republish {
    fn tick(&self, scene: &mut Scene) -> bool {
        let _ = scene.writer.set_position(self.key, self.at.0, self.at.1);
        scene.writer.publish()
    }
}

// ---------------------------------------------------------------------------------------------
// Phase: the verification views
// ---------------------------------------------------------------------------------------------

/// K2's cell `index`: the first verification view's cell of that number, which shows texture `index`.
fn k2_cell(index: usize) -> layout::Placed {
    layout::view_quad(0, u32::try_from(index).unwrap_or(u32::MAX))
}

/// The number the 16 code cells of `cell` show in `frame` (four bytes a pixel, white bright in every channel, so BGRA and RGBA alike), read at their centres, or `None` when the code is not valid.
fn decode_cell(frame: &FrameCapture, cell: &layout::Placed) -> Option<u32> {
    let stride = frame.size.0 as usize * 4;
    let mut bits = [false; 16];
    for (c, bit) in (0_u8..16).zip(bits.iter_mut()) {
        let (cx, cy) = (c % 4, c / 4);
        let x = cell.x + (f64::from(cx) + 0.5) * cell.width / 4.0;
        let y = cell.y + (f64::from(cy) + 0.5) * cell.height / 4.0;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a centre inside the 1920 x 1080 drawable"
        )]
        let (px, py) = (x as usize, y as usize);
        let at = py * stride + px * 4;
        // BGRA: a white code cell is bright in every channel.
        *bit = frame.bytes[at] > 127 && frame.bytes[at + 1] > 127 && frame.bytes[at + 2] > 127;
    }
    layout::decode_bits(&bits)
}

/// Decodes `frame` as a verification view: for each of its 4,096 cells the 16 code cells at their centres, and compares the
/// decoded number with the texture the cell should show. Returns `(correct, wrong)` and the first few mismatches.
fn decode_view(frame: &FrameCapture, view: u32) -> (u32, u32, Vec<String>) {
    let (mut correct, mut wrong) = (0, 0);
    let mut examples = Vec::new();
    for index in 0..layout::VIEW_SIDE * layout::VIEW_SIDE {
        let cell = layout::view_quad(view, index);
        let decoded = decode_cell(frame, &cell);
        if decoded == Some(cell.texture) {
            correct += 1;
        } else {
            wrong += 1;
            if examples.len() < 10 {
                examples.push(format!(
                    "view {view} cell {index}: texture {} decoded as {decoded:?}",
                    cell.texture,
                ));
            }
        }
    }
    (correct, wrong, examples)
}

/// A capture the decoder can read: four bytes a pixel, the whole drawable, the rows complete.
fn readable(frame: &FrameCapture) -> Result<(), String> {
    let (width, height) = frame.size;
    if frame.bytes.len() != width as usize * height as usize * 4 {
        return Err(format!(
            "a capture of {width} x {height} in {:?} holds {} bytes, not four a pixel",
            frame.format,
            frame.bytes.len()
        ));
    }
    Ok(())
}

/// Waits for the capture of the committed frame `drawn`: the one whose submission number it carries (a later or earlier frame of the
/// same scene is not it), and returns it.
fn capture_of(
    scene: &Scene,
    drawn: &Committed,
    bound: Duration,
) -> Result<std::sync::Arc<FrameCapture>, String> {
    let limit = Instant::now() + bound;
    loop {
        let (found, newest) = scene.capture_with_submission(drawn.submission);
        if let Some(frame) = found {
            readable(&frame)?;
            return Ok(frame);
        }
        if newest.is_some_and(|newest| newest > drawn.submission) {
            return Err(format!(
                "the capture of submission {} was not kept: the newest is {} (the harness keeps the last 8)",
                drawn.submission,
                newest.unwrap_or(0)
            ));
        }
        if Instant::now() >= limit {
            return Err(format!(
                "no capture of submission {} in {bound:?}",
                drawn.submission
            ));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A captured frame decoded in the K2 cells.
struct Decoded {
    /// The 0-based place of the frame in commit order.
    index: u64,
    /// Committed after every reload was requested (by index, with the fence margin).
    after_requests: bool,
    /// Whether cell `i` shows texture `i`.
    rights: Vec<bool>,
}

impl Decoded {
    fn wrong(&self) -> u64 {
        self.rights.iter().filter(|right| !**right).count() as u64
    }
}

/// What the K2 frames cover, for [`coverage_problem`].
pub(crate) struct Window<'a> {
    /// Captures that were gone before they were read (more than the last 8 arrived between two reads).
    pub(crate) lost: u64,
    /// Captures that could not be read as a frame.
    pub(crate) unreadable: u64,
    /// Captures with no committed record of their frame, after the bound.
    pub(crate) unidentified: u64,
    /// The commit ordinals (0-based) of the decoded frames, in the order taken.
    pub(crate) ordinals: &'a [u64],
    /// More frames were decoded than are retained.
    pub(crate) overflowed: bool,
    /// The commit ordinal of the baseline frame (the last one decoded before the window began).
    pub(crate) baseline_index: u64,
    /// The first and last frames the claim needs: the first committed after the requests (with the fence margin) and the end frame.
    pub(crate) first_required: u64,
    pub(crate) last_required: u64,
}

/// Why the decoded frames do not prove the transition, if they do not: the window must be complete (no capture lost, unreadable or
/// unidentified, the retained frames consecutive in commit order), must begin after the baseline frame (no earlier scene), and must
/// reach both ends of the interval the claim names.
pub(crate) fn coverage_problem(window: &Window<'_>) -> Option<String> {
    if window.overflowed {
        return Some(format!(
            "more than {K2_MAX_FRAMES} frames were decoded and the evidence is not retained"
        ));
    }
    if window.lost != 0 {
        return Some(format!(
            "{} captures were gone before they were read (more than 8 arrived between two reads)",
            window.lost
        ));
    }
    if window.unreadable != 0 {
        return Some(format!("{} captures could not be read", window.unreadable));
    }
    if window.unidentified != 0 {
        return Some(format!(
            "{} captures had no committed record of their frame",
            window.unidentified
        ));
    }
    let (Some(&first), Some(&last)) = (window.ordinals.first(), window.ordinals.last()) else {
        return Some("no frame was decoded".to_owned());
    };
    if first <= window.baseline_index {
        return Some(format!(
            "the window begins at frame {first}, not after the baseline frame {}: an earlier scene is in it",
            window.baseline_index
        ));
    }
    for pair in window.ordinals.windows(2) {
        if pair[1] != pair[0] + 1 {
            return Some(if pair[1] > pair[0] {
                format!(
                    "frames {} to {} have no decoded capture",
                    pair[0] + 1,
                    pair[1] - 1
                )
            } else {
                format!("frame {} came after frame {}", pair[1], pair[0])
            });
        }
    }
    if first > window.first_required {
        return Some(format!(
            "the first frame the claim needs, {}, was not decoded: the window begins at {first}",
            window.first_required
        ));
    }
    if last < window.last_required {
        return Some(format!(
            "the last frame the claim needs, {}, was not decoded: the window ends at {last}",
            window.last_required
        ));
    }
    None
}

/// K2's verdict: no refusal, no regression, no wrong cell in any decoded frame, every revision shown at the end in a frame decoded
/// after, and a complete window ([`coverage_problem`]).
pub(crate) fn k2_verdict(
    reload_refused: u64,
    shown_regressed: u64,
    wrong_cells: u64,
    end_wrong: u64,
    all_shown: bool,
    coverage: Option<&str>,
) -> bool {
    reload_refused == 0
        && shown_regressed == 0
        && wrong_cells == 0
        && end_wrong == 0
        && all_shown
        && coverage.is_none()
}

/// K2's frames: every capture from a cursor taken before the reloads is taken once, in order, identified by its submission number and
/// decoded in all the cells, so that no frame of the transition is looked at only when a poll happens to fall on it. A capture is
/// consumed only when it has been read as a frame and identified (or the bound for its record has passed, which is a defect counted
/// and failing K2), and the numbers of the captures that were gone before they were read are counted.
struct TransitionFrames {
    /// The first index in commit order that counts as committed after the requests.
    first_after: u64,
    /// The sequence number of the next capture to take.
    next: u64,
    /// Since when the capture at `next` has had no committed record.
    pending_since: Option<Instant>,
    /// The commit ordinals decoded, in order.
    ordinals: Vec<u64>,
    overflowed: bool,
    lost: u64,
    unreadable: u64,
    unidentified: u64,
    decoded: u64,
    after_requests: u64,
    /// Cells that did not show their own identity, over every decoded frame.
    wrong_cells: u64,
    /// The first decoded frame committed after the requests: its index and wrong cells.
    first_after_requests: Option<(u64, u64)>,
}

impl TransitionFrames {
    /// `cursor` is [`Scene::captures_cursor`], taken after the baseline frame was decoded and before the reloads were requested.
    fn new(first_after: u64, cursor: u64) -> Self {
        Self {
            first_after,
            next: cursor,
            pending_since: None,
            ordinals: Vec::new(),
            overflowed: false,
            lost: 0,
            unreadable: 0,
            unidentified: 0,
            decoded: 0,
            after_requests: 0,
            wrong_cells: 0,
            first_after_requests: None,
        }
    }

    /// Takes the captures made since the last call, oldest first, and decodes the identified ones in `cells` cells.
    fn take(&mut self, scene: &Scene, cells: usize) -> Vec<Decoded> {
        let mut taken = Vec::new();
        let (lost, kept) = scene.captures_since(self.next);
        self.lost += lost;
        self.next += lost;
        for (number, frame) in kept {
            if readable(&frame).is_err() {
                self.unreadable += 1;
                self.next = number + 1;
                self.pending_since = None;
                continue;
            }
            let Some(index) = scene
                .committed_ordinal_of(frame.submission)
                .and_then(|ordinal| ordinal.checked_sub(1))
            else {
                // The record of the committed frame is made by another thread and can lag: wait for it, then call it a defect.
                let since = *self.pending_since.get_or_insert_with(Instant::now);
                if since.elapsed() < IDENTIFY_BOUND {
                    break;
                }
                self.unidentified += 1;
                self.next = number + 1;
                self.pending_since = None;
                continue;
            };
            self.pending_since = None;
            self.next = number + 1;
            let rights: Vec<bool> = (0..cells)
                .map(|index| {
                    let cell = k2_cell(index);
                    decode_cell(&frame, &cell) == Some(cell.texture)
                })
                .collect();
            let decoded = Decoded {
                index,
                after_requests: index >= self.first_after,
                rights,
            };
            self.decoded += 1;
            self.wrong_cells += decoded.wrong();
            if self.ordinals.len() < K2_MAX_FRAMES {
                self.ordinals.push(index);
            } else {
                self.overflowed = true;
            }
            if decoded.after_requests {
                self.after_requests += 1;
                if self.first_after_requests.is_none() {
                    self.first_after_requests = Some((index, decoded.wrong()));
                }
            }
            taken.push(decoded);
        }
        taken
    }

    /// The coverage problem, if any, for the window that ends at `last_required`.
    fn coverage(&self, baseline_index: u64, last_required: u64) -> Option<String> {
        coverage_problem(&Window {
            lost: self.lost,
            unreadable: self.unreadable,
            unidentified: self.unidentified,
            ordinals: &self.ordinals,
            overflowed: self.overflowed,
            baseline_index,
            first_required: self.first_after,
            last_required,
        })
    }
}

/// The correctness pass (the validation layer on): all 32,768 textures sampled at the right place through 8 views, the distinct-identity
/// count, and the full scene's counters.
#[expect(
    clippy::too_many_lines,
    reason = "one phase is one sequential protocol whose steps read best in order"
)]
pub(crate) fn verify(scene: &mut Scene) -> Result<J, String> {
    let (_, scale) = check_drawable(scene)?;
    let units = Units(scale);
    let mut foreground = Foreground::new(scene, units)?;
    let names = set_a(layout::SET_A);
    let loaded = load_all(scene, &names, PER_UPDATE, &mut foreground, true)?;
    foreground.remove(scene);
    // The scene is empty before the first view: a frame of no quads is drawn, so that every later wait is for the view's own frame.
    wait_drawn(scene, 0, Duration::from_secs(60))?;

    let mut views = Vec::new();
    let (mut correct_total, mut wrong_total) = (0_u32, 0_u32);
    for view in 0..layout::VIEWS {
        // The view's quads, over the live textures.
        let mut quads = Vec::with_capacity(4096);
        for index in 0..layout::VIEW_SIDE * layout::VIEW_SIDE {
            let placed = layout::view_quad(view, index);
            let reference = scene
                .service
                .retain(loaded.keys[placed.texture as usize])
                .map_err(|error| format!("view {view}: {error:?}"))?;
            quads.push(
                scene
                    .writer
                    .create_quad(
                        reference,
                        units.point(placed.x),
                        units.point(placed.y),
                        units.point(placed.width),
                        units.point(placed.height),
                    )
                    .map_err(|error| format!("view {view}: {error:?}"))?,
            );
        }
        let _ = scene.writer.publish();
        let drawn = wait_drawn(scene, 4096, Duration::from_secs(60))?;
        let frame = capture_of(scene, &drawn, Duration::from_secs(60))?;
        // The captured frame is of the view's own scene: 4,096 instances, none the checker.
        let stats = scene.stats().ok_or("no presenter stats")?;
        if frame.size != (layout::DRAWABLE_WIDTH, layout::DRAWABLE_HEIGHT) {
            return Err(format!("a capture of {} x {}", frame.size.0, frame.size.1));
        }
        let (correct, wrong, examples) = decode_view(&frame, view);
        correct_total += correct;
        wrong_total += wrong;
        views.push(J::o([
            ("view", J::U(u64::from(view))),
            ("frame_ordinal", J::U(drawn.ordinal)),
            ("frame_generation", drawn.generation.map_or(J::Null, J::U)),
            ("frame_submission", J::U(drawn.submission)),
            ("correct", J::U(u64::from(correct))),
            ("wrong", J::U(u64::from(wrong))),
            ("instances", J::U(stats.last.instances)),
            ("resolved", J::U(stats.last.resolved)),
            ("checker", J::U(stats.last.checker)),
            ("examples", J::A(examples.into_iter().map(J::S).collect())),
        ]));
        for quad in quads {
            let _ = scene.writer.destroy_quad(quad);
        }
        let _ = scene.writer.publish();
        wait_drawn(scene, 0, Duration::from_secs(60))?;
    }

    // The full scene: 65,536 quads, painter order and overlap; the counters say how many distinct revisions it pinned.
    let quads = build_full_scene(scene, &loaded.keys, units)?;
    let _ = scene.writer.publish();
    wait_drawn(
        scene,
        u64::from(layout::SCENE_QUADS),
        Duration::from_secs(60),
    )?;
    let full = scene.stats().ok_or("no presenter stats")?;
    // The store created set A's 32,768 textures, the foreground's one and the checker (the bindless path pins its table, not each
    // revision, so the frame's `pinned` is 0 here and is only reported). Each term is recorded, so a failure names the term.
    let created_expected = expected_textures_created(full.checker_resident);
    let terms = [
        ("no_checker_instance", full.last.checker == 0),
        (
            "all_resolved",
            full.last.resolved == u64::from(layout::SCENE_QUADS),
        ),
        ("none_skipped", full.last.skipped == 0),
        ("none_dropped", full.last.dropped == 0),
        (
            "textures_created_as_expected",
            full.textures_created == created_expected,
        ),
    ];
    let distinct_ok = terms.iter().all(|(_, held)| *held);
    let terms_record = J::o(terms.map(|(name, held)| (name, J::Bool(held))));
    let last_frame = J::o([
        ("instances", J::U(full.last.instances)),
        ("resolved", J::U(full.last.resolved)),
        ("checker", J::U(full.last.checker)),
        ("skipped", J::U(full.last.skipped)),
        ("dropped", J::U(full.last.dropped)),
        ("draw_calls", J::U(full.last.batches)),
        ("textures_created", J::U(full.textures_created)),
        ("textures_created_expected", J::U(created_expected)),
        ("checker_resident", J::Bool(full.checker_resident)),
    ]);
    for quad in quads {
        let _ = scene.writer.destroy_quad(quad);
    }
    let _ = scene.writer.publish();
    for key in &loaded.keys {
        let _ = scene.service.release(*key);
    }

    let mut record = J::o([
        ("phase", J::s("verify")),
        (
            "drawable",
            J::A(vec![
                J::U(u64::from(layout::DRAWABLE_WIDTH)),
                J::U(u64::from(layout::DRAWABLE_HEIGHT)),
            ]),
        ),
        ("backing_scale", J::F(scale)),
        ("load", load_record(&loaded, "set A")),
        ("views", J::A(views)),
        ("decoded_correct", J::U(u64::from(correct_total))),
        ("decoded_wrong", J::U(u64::from(wrong_total))),
        (
            "full_scene",
            evidence::scene_stats(&full.scene, scene.allocations()),
        ),
        ("full_scene_last_frame", last_frame),
        ("distinct_identities_terms", terms_record),
        ("distinct_identities_ok", J::Bool(distinct_ok)),
        (
            "distinct_identities_method",
            J::s(
                "the full scene's last frame drew 65,536 resolved instances with no checker and no skipped or dropped instance, the store created exactly 32,768 textures for set A plus the foreground's one and the checker (the Vulkan count includes it), and all 32,768 identity codes decoded in their own cells through the 8 views (a proxy: the presenter exposes no count of the table's non-checker resource IDs)",
            ),
        ),
    ]);
    let passed = correct_total == layout::SET_A && wrong_total == 0 && distinct_ok;
    record.push("passed", J::Bool(passed));
    if passed {
        Ok(record)
    } else {
        Err(format!("the verification failed: {}", record.render()))
    }
}

// ---------------------------------------------------------------------------------------------
// Phase: the load rates
// ---------------------------------------------------------------------------------------------

/// L-cold and L-warm: set A twice, 128 creations per update at 60 Hz, a foreground quad moving. Reported by the service's own
/// timestamps. The warm run's rate is printed beside the reference of 1,024 loads per second and CK-130's Metal figure; it is **not a
/// gate on Vulkan**. Correctness is that both loads complete (a load that fails or does not finish returns an error).
pub(crate) fn loads(scene: &mut Scene) -> Result<J, String> {
    let (_, scale) = check_drawable(scene)?;
    let mut foreground = Foreground::new(scene, Units(scale))?;
    let names = set_a(layout::SET_A);
    // The window of this phase is the work itself (the load is shorter than the 5 s warm-up): from the first load to the last
    // ready, ended afterwards.
    let t0 = now_ns();
    scene.request_window(MetricsWindow {
        id: 1,
        start: t0 + 250_000_000,
        end: None,
    })?;
    let cold = load_all(scene, &names, PER_UPDATE, &mut foreground, true)?;
    // The registry holds 65,536 textures: two live copies of set A (and the foreground) would not fit, so the cold set is released and
    // retired before the same files are loaded again.
    for key in &cold.keys {
        let _ = scene.service.release(*key);
    }
    let drain_start = Instant::now();
    let mut cadence = Cadence::starting_at(now_ns());
    let drained = loop {
        let stats = scene.service.stats();
        if stats.registry.live + stats.registry.retiring <= 1 {
            break true;
        }
        if drain_start.elapsed() >= Duration::from_secs(60) {
            break false;
        }
        foreground.tick(scene);
        cadence.tick();
    };
    if !drained {
        return Err("the cold set did not retire within 60 s".to_owned());
    }
    let drain_seconds = drain_start.elapsed().as_secs_f64();
    let warm = load_all(scene, &names, PER_UPDATE, &mut foreground, true)?;
    let end = now_ns() + 250_000_000;
    scene.end_window(1, end)?;
    let mut cadence = Cadence::starting_at(now_ns());
    while now_ns() < end + PERIOD_NS {
        foreground.tick(scene);
        cadence.tick();
    }
    let reading = scene.read_window(1, READ_WITHIN);
    let warm_rate = {
        let span = warm.last_ready_ns.saturating_sub(warm.first_load_ns);
        #[expect(clippy::cast_precision_loss, reason = "counts far below 2^53")]
        if span == 0 {
            0.0
        } else {
            warm.keys.len() as f64 / (span as f64 / 1e9)
        }
    };
    let reference_met = warm_rate >= LOAD_REFERENCE_PER_SECOND;
    let mut record = J::o([
        ("phase", J::s("loads")),
        (
            "cold",
            load_record(
                &cold,
                "L-cold (first load of set A into a fresh directory; no cache purge)",
            ),
        ),
        (
            "warm",
            load_record(&warm, "L-warm (the same immediately after, files cached)"),
        ),
        (
            "window",
            reading.as_ref().map_or(J::Null, evidence::reading),
        ),
        ("cold_set_retired_before_warm_seconds", J::F(drain_seconds)),
        ("warm_loads_per_second", J::F(warm_rate)),
        (
            "warm_loads_per_second_reference",
            J::o([
                ("gated", J::Bool(false)),
                (
                    "note",
                    J::s(
                        "the reference printed beside the figure; not a gate on Vulkan (owner, 2026-10-30)",
                    ),
                ),
                ("reference_per_second", J::F(LOAD_REFERENCE_PER_SECOND)),
                ("at_or_above_reference", J::Bool(reference_met)),
                ("metal_ck130_per_second", J::F(METAL_WARM_LOADS_PER_SECOND)),
                (
                    "ratio_to_metal",
                    J::F(warm_rate / METAL_WARM_LOADS_PER_SECOND),
                ),
            ]),
        ),
        ("memory", memory(scene)),
    ]);
    // Both loads came back complete (an incomplete one returned an error above): correctness holds whatever the rate.
    record.push("passed", J::Bool(true));
    for key in &warm.keys {
        let _ = scene.service.release(*key);
    }
    foreground.remove(scene);
    Ok(record)
}

// ---------------------------------------------------------------------------------------------
// Phase: the static scene (S-ordinary and S-bindless)
// ---------------------------------------------------------------------------------------------

/// S-ordinary or S-bindless: the full scene, static, republished (the identical scene, a new generation) at 60 Hz for the warm-up and
/// the sample window.
#[expect(
    clippy::too_many_lines,
    reason = "one phase is one sequential protocol whose steps read best in order"
)]
pub(crate) fn static_scene(scene: &mut Scene, label: &'static str) -> Result<J, String> {
    let (_, scale) = check_drawable(scene)?;
    let units = Units(scale);
    let mut foreground = Foreground::new(scene, units)?;
    let loaded = load_all(
        scene,
        &set_a(layout::SET_A),
        PER_UPDATE,
        &mut foreground,
        true,
    )?;
    foreground.remove(scene);
    let quads = build_full_scene(scene, &loaded.keys, units)?;
    let _ = scene.writer.publish();
    wait_drawn(
        scene,
        u64::from(layout::SCENE_QUADS),
        Duration::from_secs(60),
    )?;
    let first = layout::scene_quad(0, true);
    let republish = Republish {
        key: quads[0],
        at: (units.point(first.x), units.point(first.y)),
    };
    let measured = measure(scene, 1, |scene, _| republish.tick(scene))?;
    // The mode and the draw, from the frame the presenter built last (the scene is static, so every frame in the window is this one).
    let stats = scene.stats().ok_or("no presenter stats")?;
    let bindless = label == "static-bindless";
    let notice = scene.binding_notice();
    let mut reasons: Vec<J> = Vec::new();
    if stats.last.instances != u64::from(layout::SCENE_QUADS)
        || stats.last.resolved != u64::from(layout::SCENE_QUADS)
        || stats.last.checker != 0
    {
        reasons.push(J::s(format!(
            "the last frame drew {} instances, {} resolved, {} checker; {} resolved and no checker were required",
            stats.last.instances,
            stats.last.resolved,
            stats.last.checker,
            layout::SCENE_QUADS
        )));
    }
    if bindless {
        if stats.bindless_slots != 131_072 {
            reasons.push(J::s(format!(
                "the bindless table has {} slots, not 131,072 (the ordinary mode or a fallback ran)",
                stats.bindless_slots
            )));
        }
        match &notice {
            Some(notice) if !notice.fallback => {}
            Some(notice) => reasons.push(J::s(format!(
                "the binding notice is the fallback one: {}",
                notice.text
            ))),
            None => reasons.push(J::s("no binding notice was reported".to_owned())),
        }
        if stats.last.batches != 1 {
            reasons.push(J::s(format!(
                "{} draw calls for the frame, not one",
                stats.last.batches
            )));
        }
    } else if stats.bindless_slots != 0 {
        reasons.push(J::s(format!(
            "the per-revision baseline ran with a bindless table of {} slots",
            stats.bindless_slots
        )));
    }
    let passed = reasons.is_empty();
    let reference = measured
        .reading
        .as_ref()
        .map_or(J::Null, evidence::reference_reading);
    let mut record = J::o([
        ("phase", J::s(label)),
        ("load", load_record(&loaded, "set A")),
        ("window", measured_record(&measured)),
        ("reference", reference),
        (
            "mode",
            J::o([
                ("bindless", J::Bool(bindless)),
                ("bindless_slots", J::U(u64::from(stats.bindless_slots))),
                (
                    "binding_notice",
                    notice.as_ref().map_or(J::Null, |notice| {
                        J::o([
                            ("fallback", J::Bool(notice.fallback)),
                            ("text", J::s(notice.text.clone())),
                        ])
                    }),
                ),
                ("draw_calls_last_frame", J::U(stats.last.batches)),
                ("instances_last_frame", J::U(stats.last.instances)),
                ("checker_last_frame", J::U(stats.last.checker)),
            ]),
        ),
        (
            "correctness",
            J::o([("passed", J::Bool(passed)), ("reasons", J::A(reasons))]),
        ),
        ("memory", memory(scene)),
    ]);
    // Timings are reported, not gated; both runs gate on their correctness (the mode they ran in and the draw they made).
    record.push("gated", J::Bool(false));
    record.push("passed", J::Bool(passed));
    for quad in quads {
        let _ = scene.writer.destroy_quad(quad);
    }
    let _ = scene.writer.publish();
    for key in &loaded.keys {
        let _ = scene.service.release(*key);
    }
    Ok(record)
}

// ---------------------------------------------------------------------------------------------
// Phase: churn
// ---------------------------------------------------------------------------------------------

/// Textures reloaded and textures released and recreated per update.
const CHURN: usize = 64;

/// C: for 30 s (after the warm-up) every update reloads 64 textures and releases and recreates 64 textures with their quads, with
/// frames in flight. Correctness is asserted: nothing fails, every texture that was created is retired at the end.
#[expect(
    clippy::too_many_lines,
    reason = "one phase is one sequential protocol whose steps read best in order"
)]
pub(crate) fn churn(scene: &mut Scene) -> Result<J, String> {
    let (_, scale) = check_drawable(scene)?;
    let units = Units(scale);
    let mut foreground = Foreground::new(scene, units)?;
    let names = set_a(layout::SET_A);
    let loaded = load_all(scene, &names, PER_UPDATE, &mut foreground, true)?;
    foreground.remove(scene);
    let mut keys = loaded.keys.clone();
    let mut quads = build_full_scene(scene, &keys, units)?;
    let _ = scene.writer.publish();
    wait_drawn(
        scene,
        u64::from(layout::SCENE_QUADS),
        Duration::from_secs(60),
    )?;

    let (mut reload_at, mut recreate_at) = (0_usize, 0_usize);
    let (mut reloads, mut recreated, mut refused) = (0_u64, 0_u64, 0_u64);
    let measured = measure(scene, 1, |scene, _| {
        for _ in 0..CHURN {
            // Reload: any texture, round-robin; a refusal (extra revisions busy) is counted.
            let key = keys[reload_at % keys.len()];
            reload_at += 1;
            match scene.service.reload(key) {
                Ok(_) => reloads += 1,
                Err(_) => refused += 1,
            }
        }
        for _ in 0..CHURN {
            // Release and recreate an odd texture (it has only its pass-0 quad): the quad and the public token go, a new load and
            // a new quad take their place.
            let t = (2 * (recreate_at % (keys.len() / 2)) + 1) % keys.len();
            recreate_at += 1;
            let placed = layout::scene_quad(u32::try_from(t).unwrap_or(0), true);
            let _ = scene.writer.destroy_quad(quads[t]);
            let _ = scene.service.release(keys[t]);
            match scene
                .service
                .load(&layout::set_a_name(u32::try_from(t).unwrap_or(0)))
            {
                Ok(key) => {
                    keys[t] = key;
                    if let Ok(reference) = scene.service.retain(key)
                        && let Ok(quad) = scene.writer.create_quad(
                            reference,
                            units.point(placed.x),
                            units.point(placed.y),
                            units.point(placed.width),
                            units.point(placed.height),
                        )
                    {
                        quads[t] = quad;
                        recreated += 1;
                    }
                }
                Err(_) => refused += 1,
            }
        }
        scene.writer.publish()
    })?;
    // The churn stops; everything still in flight lands.
    let deadline = Instant::now() + READY_BOUND;
    let (mut ready, mut failed);
    loop {
        (ready, failed) = ready_and_failed(&scene.service, &keys);
        if ready + failed == keys.len() || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let created = scene.stats().map_or(0, |stats| stats.textures_created);
    let mut record = J::o([
        ("phase", J::s("churn")),
        (
            "per_update",
            J::o([
                ("reloads", J::U(CHURN as u64)),
                ("recreations", J::U(CHURN as u64)),
            ]),
        ),
        ("reloads", J::U(reloads)),
        ("recreated", J::U(recreated)),
        ("refused", J::U(refused)),
        ("ready_at_end", J::U(ready as u64)),
        ("failed_at_end", J::U(failed as u64)),
        ("textures_created", J::U(created)),
        ("window", measured_record(&measured)),
        ("memory", memory(scene)),
    ]);
    for quad in quads {
        let _ = scene.writer.destroy_quad(quad);
    }
    let _ = scene.writer.publish();
    for key in &keys {
        let _ = scene.service.release(*key);
    }
    // Every created texture is retired: nothing live or retiring remains once the backend has processed the retirements.
    let deadline = Instant::now() + Duration::from_secs(60);
    let retired = loop {
        let stats = scene.service.stats();
        if stats.registry.live == 0 && stats.registry.retiring == 0 {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    record.push("all_retired", J::Bool(retired));
    let passed = failed == 0 && ready == keys.len() && retired;
    record.push("passed", J::Bool(passed));
    if passed {
        Ok(record)
    } else {
        Err(format!(
            "the churn run failed its correctness checks: {}",
            record.render()
        ))
    }
}

// ---------------------------------------------------------------------------------------------
// Phase: heterogeneous sizes
// ---------------------------------------------------------------------------------------------

/// H: set B (248 images, about 338 MiB decoded) drawn in a 16 x 16 grid, republished at 60 Hz; reported (memory and frame times).
pub(crate) fn heterogeneous(scene: &mut Scene) -> Result<J, String> {
    let (_, scale) = check_drawable(scene)?;
    let units = Units(scale);
    let mut foreground = Foreground::new(scene, units)?;
    let names: Vec<String> = (0..layout::set_b_count()).map(layout::set_b_name).collect();
    let loaded = load_all(scene, &names, 16, &mut foreground, true)?;
    foreground.remove(scene);
    let (cell_w, cell_h) = (
        f64::from(layout::DRAWABLE_WIDTH) / 16.0,
        f64::from(layout::DRAWABLE_HEIGHT) / 16.0,
    );
    let mut quads = Vec::new();
    for (n, key) in loaded.keys.iter().enumerate() {
        let (column, row) = (n % 16, n / 16);
        let reference = scene
            .service
            .retain(*key)
            .map_err(|error| format!("set B image {n}: {error:?}"))?;
        #[expect(clippy::cast_precision_loss, reason = "a grid index")]
        quads.push(
            scene
                .writer
                .create_quad(
                    reference,
                    units.point(column as f64 * cell_w),
                    units.point(row as f64 * cell_h),
                    units.point(cell_w),
                    units.point(cell_h),
                )
                .map_err(|error| format!("set B image {n}: {error:?}"))?,
        );
    }
    let _ = scene.writer.publish();
    wait_drawn(scene, loaded.keys.len() as u64, Duration::from_secs(60))?;
    let republish = Republish {
        key: quads[0],
        at: (0.0, 0.0),
    };
    let measured = measure(scene, 1, |scene, _| republish.tick(scene))?;
    let record = J::o([
        ("phase", J::s("heterogeneous")),
        ("images", J::U(loaded.keys.len() as u64)),
        ("decoded_bytes", J::U(layout::set_b_bytes())),
        ("load", load_record(&loaded, "set B")),
        ("window", measured_record(&measured)),
        ("memory", memory(scene)),
        ("passed", J::Bool(true)),
    ]);
    for quad in quads {
        let _ = scene.writer.destroy_quad(quad);
    }
    let _ = scene.writer.publish();
    for key in &loaded.keys {
        let _ = scene.service.release(*key);
    }
    Ok(record)
}

// ---------------------------------------------------------------------------------------------
// Phase: caps at small sizes
// ---------------------------------------------------------------------------------------------

/// Waits until nothing is live or retiring (the backend processed every retirement), within `bound`.
fn wait_retired(scene: &Scene, bound: Duration) -> bool {
    let limit = Instant::now() + bound;
    loop {
        let stats = scene.service.stats();
        if stats.registry.live == 0 && stats.registry.retiring == 0 {
            return true;
        }
        if Instant::now() >= limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// What `caps-min` expects of its one request: refused, not ready, by the texture-bytes budget with the pool already at the ceiling
/// (`limit` of `limit` reserved).
pub(crate) fn caps_min_verdict(
    ready: usize,
    failed: usize,
    messages: &[String],
    limit: u64,
) -> bool {
    ready == 0
        && failed == 1
        && messages.len() == 1
        && messages[0].contains(TEXTURE_BYTES_REASON)
        && messages[0].contains(&format!("({limit} of {limit} reserved)"))
}

/// The start-up reservation (CK-162 part C r4): on a fresh presenter at CK-130's 64 MiB, before anything else was loaded, one 16 MiB image
/// of set B is requested. If the pool's ceiling is already reached at that point, the reservation that refuses K3 in `caps` is there
/// without any K1 or K2 history: the presenter's start-up state (the checker's block). That does **not** show that no range was also
/// left behind in the `caps` process (a checker-only block and a checker-plus-leak block reserve the same total), and nothing here claims it.
/// **Reported, not gated**: the job states what the one request did.
pub(crate) fn caps_min(scene: &mut Scene) -> Result<J, String> {
    check_drawable(scene)?;
    let mut cadence = Cadence::starting_at(now_ns());
    let key = scene
        .service
        .load(&layout::set_b_name(layout::set_b_count() - 1))
        .map_err(|error| format!("caps-min: the request was refused at admission: {error:?}"))?;
    let limit = Instant::now() + Duration::from_secs(60);
    let (mut ready, mut failed);
    loop {
        cadence.tick();
        (ready, failed) = ready_and_failed(&scene.service, std::slice::from_ref(&key));
        if ready + failed == 1 || Instant::now() >= limit {
            break;
        }
    }
    let mut messages = Vec::new();
    if let Ok(status) = scene.service.status(key)
        && status.state == StatusState::Failed
        && let Some(error) = status.error
    {
        messages.push(error.message().to_owned());
    }
    let _ = scene.service.release(key);
    let passed = caps_min_verdict(ready, failed, &messages, CAPS_MIN_TEXTURE_BYTES);
    let mut record = J::o([
        ("phase", J::s("caps-min")),
        ("texture_bytes_limit", J::U(CAPS_MIN_TEXTURE_BYTES)),
        ("requested", J::U(1)),
        ("ready", J::U(ready as u64)),
        ("refused", J::U(failed as u64)),
        ("messages", J::A(messages.into_iter().map(J::S).collect())),
        (
            "expectation",
            J::s(
                "refused by the texture-bytes budget with the pool at its ceiling (`<limit> of <limit> reserved`) before anything else was loaded",
            ),
        ),
        ("memory", memory(scene)),
    ]);
    record.push("passed", J::Bool(passed));
    Ok(record)
}

/// K1 to K3, with the small limits the worker was started with (`max_textures` 1,024, 64 extra revisions, 1,089 slots, `texture_bytes`
/// [`CAPS_TEXTURE_BYTES`]).
#[expect(
    clippy::too_many_lines,
    reason = "one phase is one sequential protocol whose steps read best in order"
)]
pub(crate) fn caps(scene: &mut Scene) -> Result<J, String> {
    let (_, scale) = check_drawable(scene)?;
    let units = Units(scale);
    // K1: 1,100 distinct set A loads: 1,024 admitted and 76 refused by the texture count. The foreground draws the first loaded
    // texture (the registry has no room for another).
    let names = set_a(1100);
    let mut cadence = Cadence::starting_at(now_ns());
    let mut keys = Vec::new();
    let mut refused: Vec<(String, u64)> = Vec::new();
    for name in &names {
        match scene.service.load(name) {
            Ok(key) => keys.push(key),
            Err(error) => count(&mut refused, reason(&error)),
        }
    }
    let mut foreground = Foreground::over(scene, units, keys[0])?;
    let limit = Instant::now() + READY_BOUND;
    loop {
        foreground.tick(scene);
        cadence.tick();
        let (ready, failed) = ready_and_failed(&scene.service, &keys);
        if failed != 0 {
            return Err(format!("K1: {failed} admitted loads failed"));
        }
        if ready == keys.len() {
            break;
        }
        if Instant::now() >= limit {
            return Err(format!("K1: only {ready} of {} ready", keys.len()));
        }
    }
    let k1 = J::o([
        ("requested", J::U(names.len() as u64)),
        ("admitted", J::U(keys.len() as u64)),
        (
            "refused",
            J::A(
                refused
                    .iter()
                    .map(|(reason, n)| {
                        J::o([("reason", J::s(reason.clone())), ("count", J::U(*n))])
                    })
                    .collect(),
            ),
        ),
    ]);
    let k1_ok = keys.len() == 1024
        && refused.len() == 1
        && refused[0].1 == 76
        && refused[0].0.starts_with("texture count");
    // K2: 65 reloads of distinct shown textures in one update: the backend holds at most 64 extra revisions; the one with no room waits
    // in the resource service with its shown revision unchanged; all 65 become shown; no refusal and no failure. What this harness can
    // observe: the 65 textures are drawn in identity cells (cell i shows texture i), every frame is captured, and **every frame
    // of a complete window is decoded in all 65 cells**: the window runs from a cursor taken after the baseline frame and before the reloads
    // to the end frame, no capture may be lost, unreadable or unidentified, the commit ordinals must be consecutive and reach both ends of the
    // interval the claim names, and the first frame committed after the requests is singled out. A
    // texture whose newest requested revision is not yet shown must still show its own identity (the old image stays up until the new
    // one is admitted: never the checker, never another image); the claim is "no frame across the transition shows a wrong image". Whether
    // a waiting texture was seen, and the number of such waiting revisions per tick, are recorded as data: the reloads can all be shown
    // within one frame of the requests (run 2: 65 shown within 13 frames), so no waiting state need be visible. The extra-revision count
    // and peak are not exposed by the presenter and so are not claimed.
    let sample: Vec<TextureKey> = keys.iter().take(K2_RELOADS).copied().collect();
    let mut cells = Vec::with_capacity(sample.len());
    for (index, key) in sample.iter().enumerate() {
        let placed = k2_cell(index);
        let reference = scene
            .service
            .retain(*key)
            .map_err(|error| format!("K2 cell {index}: {error:?}"))?;
        cells.push(
            scene
                .writer
                .create_quad(
                    reference,
                    units.point(placed.x),
                    units.point(placed.y),
                    units.point(placed.width),
                    units.point(placed.height),
                )
                .map_err(|error| format!("K2 cell {index}: {error:?}"))?,
        );
    }
    let _ = scene.writer.publish();
    // The foreground quad is drawn too (it was created first and is below the cells).
    let drawn = wait_drawn(scene, cells.len() as u64 + 1, Duration::from_secs(60))?;
    let baseline_frame = capture_of(scene, &drawn, Duration::from_secs(60))?;
    let baseline_wrong = (0..sample.len())
        .filter(|index| {
            let cell = k2_cell(*index);
            decode_cell(&baseline_frame, &cell) != Some(cell.texture)
        })
        .count();
    if baseline_wrong != 0 {
        return Err(format!(
            "K2: {baseline_wrong} of {} cells did not show their own identity before the reloads",
            sample.len()
        ));
    }
    // The window of the transition begins here: the baseline frame is decoded and the reloads are not yet requested, so a capture from
    // the cursor on belongs to the cell scene and to this frame or a later one.
    let baseline_index = drawn
        .ordinal
        .checked_sub(1)
        .ok_or("K2: the baseline frame has no ordinal")?;
    let cursor = scene.captures_cursor();
    let shown_before: Vec<u64> = sample
        .iter()
        .map(|key| {
            scene
                .service
                .status(*key)
                .ok()
                .and_then(|s| s.shown)
                .map_or(0, |s| s.revision)
        })
        .collect();
    let mut reload_refused = 0_u64;
    for key in &sample {
        if scene.service.reload(*key).is_err() {
            reload_refused += 1;
        }
    }
    // The frames committed so far: a capture of a frame whose index is at least this plus the fence margin was committed after every reload was requested.
    let frames_at_reload = scene
        .stats()
        .ok_or("K2: no presenter stats at the requests, so no frame fence")?
        .frames;
    let first_index_after_requests = frames_at_reload
        .checked_add(FENCE_MARGIN_FRAMES)
        .ok_or("K2: the frame fence at the requests overflowed")?;
    let (mut waiting_peak, mut shown_regressed) = (0_usize, 0_u64);
    let (mut ticks_with_waiting, mut waiting_cells_checked, mut waiting_cells_wrong) =
        (0_u64, 0_u64, 0_u64);
    let mut waiting_series: Vec<J> = Vec::new();
    let mut polls = 0_u64;
    let mut bound_sequences: Vec<J> = Vec::new();
    let mut frames = TransitionFrames::new(first_index_after_requests, cursor);
    let limit = Instant::now() + READY_BOUND;
    let all_shown = 'poll: loop {
        cadence.tick();
        foreground.tick(scene);
        // Several polls between ticks, so that the short wait of the reloaded textures is not missed.
        for _ in 0..K2_POLLS_PER_TICK {
            polls += 1;
            // Every frame captured since the last poll is decoded, in every cell, whether or not a texture is seen waiting. The
            // captures are read BEFORE the statuses: a texture still waiting in the status read that follows was waiting when those
            // frames were committed (a shown revision only advances). The frames that count for a waiting texture are those
            // committed after the reloads were requested (their index in commit order is at least the number of frames committed then).
            let fresh = frames.take(scene, sample.len());
            let after: Vec<&Decoded> = fresh.iter().filter(|frame| frame.after_requests).collect();
            let mut shown_new = 0;
            let mut waiting = 0;
            let mut waiting_checked_here = 0_u64;
            for (index, key) in sample.iter().enumerate() {
                let Ok(status) = scene.service.status(*key) else {
                    continue;
                };
                let revision = status.shown.map_or(0, |s| s.revision);
                let is_waiting = revision <= shown_before[index] && status.requested > revision;
                if revision > shown_before[index] {
                    shown_new += 1;
                } else if is_waiting {
                    waiting += 1;
                    // The shown revision of a waiting texture must be the one it had (a smaller number is a regression).
                    if revision < shown_before[index] {
                        shown_regressed += 1;
                    }
                    for frame in &after {
                        waiting_cells_checked += 1;
                        waiting_checked_here += 1;
                        if !frame.rights[index] {
                            waiting_cells_wrong += 1;
                        }
                    }
                }
                if status.state == StatusState::Failed {
                    return Err("K2: a reload failed".to_owned());
                }
            }
            if waiting_checked_here != 0
                && bound_sequences.len() < 200
                && let Some(frame) = after.last()
            {
                bound_sequences.push(J::o([
                    ("frame_index", J::U(frame.index)),
                    ("waiting_cells_decoded", J::U(waiting_checked_here)),
                ]));
            }
            if waiting != 0 {
                ticks_with_waiting += 1;
            }
            if waiting_series.len() < 600 {
                waiting_series.push(J::U(waiting as u64));
            }
            waiting_peak = waiting_peak.max(waiting);
            if shown_new == sample.len() {
                break 'poll true;
            }
            if Instant::now() >= limit {
                break 'poll false;
            }
            std::thread::sleep(Duration::from_millis(3));
        }
    };
    // Once every revision is shown, a frame committed after that is decoded: the cells still show their identities (the newest
    // revision is the same image). The frames committed when every revision was seen shown are counted, then frames are published
    // until a capture above that count exists, so the decoded frame cannot be one drawn before the all-shown read. Every frame captured
    // on the way is decoded too.
    let frames_at_all_shown = scene
        .stats()
        .ok_or("K2: no presenter stats after every revision was shown, so no frame fence")?
        .frames;
    let first_index_after_all_shown = frames_at_all_shown
        .checked_add(FENCE_MARGIN_FRAMES)
        .ok_or("K2: the frame fence after every revision was shown overflowed")?;
    let end_limit = Instant::now() + Duration::from_secs(60);
    let (end_index, end_wrong) = loop {
        cadence.tick();
        foreground.tick(scene);
        let fresh = frames.take(scene, sample.len());
        if let Some(end) = fresh
            .iter()
            .find(|frame| frame.index >= first_index_after_all_shown)
        {
            break (end.index, end.wrong());
        }
        if Instant::now() >= end_limit {
            return Err(format!(
                "K2: no capture of a frame from index {first_index_after_all_shown} on after every revision was shown"
            ));
        }
    };
    for cell in cells {
        let _ = scene.writer.destroy_quad(cell);
    }
    let _ = scene.writer.publish();
    let waiting_cell_seen = waiting_cells_checked >= 1;
    let any_cell_wrong = frames.wrong_cells;
    let coverage = frames.coverage(baseline_index, end_index);
    let k2 = J::o([
        ("reloads_requested", J::U(sample.len() as u64)),
        ("reload_refusals", J::U(reload_refused)),
        ("polls", J::U(polls)),
        ("peak_not_yet_shown", J::U(waiting_peak as u64)),
        ("ticks_with_a_waiting_revision", J::U(ticks_with_waiting)),
        ("waiting_revisions_per_tick", J::A(waiting_series)),
        (
            "shown_revision_regressed_while_waiting",
            J::U(shown_regressed),
        ),
        ("frames_committed_at_the_requests", J::U(frames_at_reload)),
        ("fence_margin_frames", J::U(FENCE_MARGIN_FRAMES)),
        (
            "first_capture_index_counted_after_the_requests",
            J::U(first_index_after_requests),
        ),
        ("frames_decoded", J::U(frames.decoded)),
        (
            "frames_decoded_committed_after_the_requests",
            J::U(frames.after_requests),
        ),
        (
            "capture_coverage",
            J::o([
                ("baseline_index", J::U(baseline_index)),
                ("first_required_index", J::U(first_index_after_requests)),
                ("last_required_index", J::U(end_index)),
                (
                    "first_decoded_index",
                    frames.ordinals.first().map_or(J::Null, |i| J::U(*i)),
                ),
                (
                    "last_decoded_index",
                    frames.ordinals.last().map_or(J::Null, |i| J::U(*i)),
                ),
                ("frames_retained", J::U(frames.ordinals.len() as u64)),
                ("captures_lost_before_read", J::U(frames.lost)),
                ("captures_unreadable", J::U(frames.unreadable)),
                ("captures_unidentified", J::U(frames.unidentified)),
                ("overflowed", J::Bool(frames.overflowed)),
                ("problem", coverage.clone().map_or(J::Null, J::S)),
            ]),
        ),
        (
            "first_frame_committed_after_the_requests",
            frames
                .first_after_requests
                .map_or(J::Null, |(index, wrong)| {
                    J::o([
                        ("frame_index", J::U(index)),
                        ("cells_decoded", J::U(sample.len() as u64)),
                        ("cells_wrong", J::U(wrong)),
                    ])
                }),
        ),
        (
            "waiting_cell_observations_by_capture",
            J::A(bound_sequences),
        ),
        (
            "frames_committed_when_all_were_shown",
            J::U(frames_at_all_shown),
        ),
        (
            "first_capture_index_accepted_after_all_shown",
            J::U(first_index_after_all_shown),
        ),
        ("end_capture_frame_index", J::U(end_index)),
        ("waiting_cells_checked", J::U(waiting_cells_checked)),
        ("waiting_cells_wrong", J::U(waiting_cells_wrong)),
        ("cells_wrong_in_any_decoded_frame", J::U(any_cell_wrong)),
        ("cells_wrong_before", J::U(baseline_wrong as u64)),
        ("cells_wrong_after_all_shown", J::U(end_wrong)),
        (
            "waiting_cell_seen_in_a_frame_committed_after_the_requests",
            J::Bool(waiting_cell_seen),
        ),
        ("all_shown_at_end", J::Bool(all_shown)),
        ("service", evidence::service_stats(&scene.service.stats())),
    ]);
    let k2_ok = k2_verdict(
        reload_refused,
        shown_regressed,
        any_cell_wrong,
        end_wrong,
        all_shown,
        coverage.as_deref(),
    ) && waiting_cells_wrong == 0;
    foreground.remove(scene);
    for key in &keys {
        let _ = scene.service.release(*key);
    }
    let retired = wait_retired(scene, Duration::from_secs(60));

    // K3: set B's 2048 x 2048 images are loaded until the texture bytes refuse one. The ceiling is the pool's first block (held from the
    // start by the checker, and by the textures of K1 and K2 while they live) plus four of these images, so some are admitted and the
    // rest refused whatever K1 and K2 left behind (CK-130 used 64 MiB, which on Vulkan is the first block alone).
    let mut big = Vec::new();
    let mut failed_texts = Vec::new();
    let mut failed_strings: Vec<String> = Vec::new();
    let first_big = layout::set_b_count() - K3_IMAGES;
    let mut cadence = Cadence::starting_at(now_ns());
    for n in first_big..layout::set_b_count() {
        if let Ok(key) = scene.service.load(&layout::set_b_name(n)) {
            big.push(key);
        }
    }
    let limit = Instant::now() + Duration::from_secs(120);
    let (mut ready, mut failed);
    loop {
        cadence.tick();
        (ready, failed) = ready_and_failed(&scene.service, &big);
        if ready + failed == big.len() || Instant::now() >= limit {
            break;
        }
    }
    for key in &big {
        if let Ok(status) = scene.service.status(*key)
            && status.state == StatusState::Failed
            && let Some(error) = status.error
        {
            failed_texts.push(J::s(error.message().to_owned()));
            failed_strings.push(error.message().to_owned());
        }
    }
    let k3 = J::o([
        ("texture_bytes_limit", J::U(CAPS_TEXTURE_BYTES)),
        ("pool_block_bytes", J::U(POOL_BLOCK_BYTES)),
        ("image_bytes", J::U(K3_IMAGE_BYTES)),
        ("requested", J::U(big.len() as u64)),
        ("ready", J::U(ready as u64)),
        ("refused_by_texture_bytes", J::U(failed as u64)),
        ("messages", J::A(failed_texts.clone())),
    ]);
    // Every failure carries the backend's own texture-bytes reason (`texture_bytes budget: ...`, which the allocation-count and native
    // device failures do not), and each one has its message: a failure without text, or for another reason, is not a texture-bytes
    // refusal.
    let k3_ok = k3_verdict(ready, failed, &failed_strings);
    for key in &big {
        let _ = scene.service.release(*key);
    }
    let passed = k1_ok && k2_ok && retired && k3_ok;
    let mut record = J::o([
        ("phase", J::s("caps")),
        ("k1_texture_count", k1),
        ("k1_ok", J::Bool(k1_ok)),
        ("k2_extra_revisions", k2),
        ("k2_ok", J::Bool(k2_ok)),
        (
            "k2_note",
            J::s(
                "NOT PROVEN HERE, waived by the owner (CK-130 review 1, F2, relayed by ck-manager; the part rests on the CK-113/CK-115 component evidence): the backend's extra-revision count and peak, and the reservation that holds a no-room revision, are not exposed by the presenter, so the <= 64 bound and the identification of which revision had no room are not claimed. Claimed: no refusal or failure, no regression of a shown revision, and no frame across the transition shows a wrong image: the window from a cursor taken after the baseline frame and before the reloads to the end frame is COMPLETE (no capture lost before it was read, none unreadable or without its committed record, the decoded frames consecutive in commit order, beginning after the baseline frame and covering the first frame committed after the requests through the end frame, or K2 fails by naming the gap), and every frame in it is decoded in all 65 cells; and the end state in a frame committed after every revision was seen shown. Reported as data, not claimed: how many cells were seen waiting in frames committed after the requests (it can be none: the reloads can be shown within a frame), the waiting count per tick, and the service's budget peaks.",
            ),
        ),
        ("k3_texture_bytes", k3),
        ("k3_ok", J::Bool(k3_ok)),
        ("retired_between", J::Bool(retired)),
        ("memory", memory(scene)),
    ]);
    record.push("passed", J::Bool(passed));
    Ok(record)
}
