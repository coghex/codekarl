//! The scene presenter's GPU-written map image on a real window (milestone 6, slices 1 and 6): the first frame with a map, and the
//! resize and the new input that replace the target and the images, retired and let go of only after the ledger's proof of the last
//! submit that used them. A submodule of `window_scene`, so its cases run in the same workers (synchronization validation on: a
//! missing barrier between the map pass and the window pass fails the case).
//!
//! No case sleeps to synchronise: the hold is a `VkEvent` the frame waits on, and a wait is on an observable fact (the presenter's
//! map counters, the ledger's proofs). The map is `ck-map`'s slice 6 fixture (`crates/map/tests/fixture/mod.rs`, included below) and
//! every expected pixel is `ck_map::reference_colour`'s.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use ck_gpu_vulkan::{
    DestinationRect, Event, FrameCapture, FrameCaptures, MapRefusalSink, SourceRect, VulkanFilter,
    VulkanFrame, VulkanMap, VulkanMapInput, VulkanMapRules, VulkanTerrainKind, VulkanTerrainRule,
};
use ck_map::{STEP_SIZE, WorldPixel, fixture};

use super::{
    Arc, Case, CleanupResult, Harness, Mutex, Quads, Rig, count, destroy_event, event, set_event,
    vk,
};

pub(crate) const ORIGIN: [i32; 2] = fixture::ORIGIN;
pub(crate) const CROP: [u32; 2] = fixture::SIZE;

/// The step of these cases: a tenth of a cell per map pixel, with all three octaves of the sub-cell detail.
pub(crate) const STEP: u8 = 60;

fn the_fixture() -> &'static fixture::Fixture {
    static FIXTURE: OnceLock<fixture::Fixture> = OnceLock::new();
    FIXTURE.get_or_init(fixture::fixture)
}

/// The world pixel of the crop's top-left corner at [`STEP`].
pub(crate) fn corner() -> [i32; 2] {
    let pixel = fixture::Fixture::crop_top_left(STEP);
    [pixel.x, pixel.y]
}

fn kind_of(kind: fixture::Kind) -> VulkanTerrainKind {
    match kind {
        fixture::Kind::Land => VulkanTerrainKind::Land,
        fixture::Kind::Sea => VulkanTerrainKind::Sea,
        fixture::Kind::Lake => VulkanTerrainKind::Lake,
        fixture::Kind::Mountain => VulkanTerrainKind::Mountain,
    }
}

/// The fixture's rule table as the presenter takes it.
fn rules_of(fixture: &fixture::Fixture) -> VulkanMapRules {
    VulkanMapRules {
        terrains: fixture
            .terrains
            .iter()
            .map(|terrain| VulkanTerrainRule {
                kind: kind_of(terrain.kind),
                solid: terrain.style.solid.0,
                second: terrain.style.second.0,
                dark: terrain.style.dark.0,
                // Vulkan is compile-only until the milestone's catch-up (CK-239): its map pass still has the earlier look's rule fields,
                // filled from the new tones just so that this builds. These cases are not run, and their fixtures are regenerated then.
                dark_second: terrain.style.dark_lit.0,
                cap: terrain.style.solid.0,
                high_ground: terrain.style.dark.0,
                relief: [0, 1],
                mix_percent: 0,
                cap_heights: [0, 1],
            })
            .collect(),
        sea: fixture.data.sea.0,
        shallow_sea: fixture.data.shallow_sea.0,
        lake: fixture.data.lake.0,
        river: fixture.data.river.0,
        overlay_palette: Vec::new(),
        shallow_threshold: fixture.data.shallow_threshold,
        steps: STEP_SIZE,
    }
}

pub(crate) fn input_with(generation: u64) -> Arc<VulkanMapInput> {
    let fixture = the_fixture();
    Arc::new(
        VulkanMapInput::new(
            generation,
            ORIGIN,
            CROP,
            fixture.grid_a.clone().into(),
            fixture.grid_b.clone().into(),
            &rules_of(fixture),
        )
        .expect("a valid input"),
    )
}

/// An input with the fixture's owner grid over its four places (the map modes' tables are one value per place).
pub(crate) fn moded_input(generation: u64) -> Arc<VulkanMapInput> {
    let fixture = the_fixture();
    let owners: Vec<u8> = fixture::mode_owners()
        .iter()
        .flat_map(|index| {
            let [high, low] = index.to_be_bytes();
            [high, low, 0, 255]
        })
        .collect();
    Arc::new(
        VulkanMapInput::new(
            generation,
            ORIGIN,
            CROP,
            fixture.grid_a.clone().into(),
            fixture.grid_b.clone().into(),
            &rules_of(fixture),
        )
        .expect("a valid input")
        .with_owners(owners.into(), fixture::MODE_PLACE_COUNT)
        .expect("a valid owner grid"),
    )
}

/// [`map_of`] showing the map mode `mode` (a revision and one value per place), or the plain map.
pub(crate) fn moded_map(
    input: &Arc<VulkanMapInput>,
    top_left: [i32; 2],
    view: [u32; 2],
    mode: Option<(u64, &[u16])>,
) -> VulkanMap {
    let map = map_of(input, top_left, view);
    match mode {
        Some((revision, values)) => map
            .with_mode(revision, values.to_vec().into())
            .expect("a table that fits the places"),
        None => map,
    }
}

/// The reference's RGBA colour of map pixel `(x, y)` of a view at `top_left` under the map mode `values` (the plain map for `None`).
pub(crate) fn moded_pixel(top_left: [i32; 2], x: u32, y: u32, values: Option<&[u16]>) -> [u8; 4] {
    let Some(values) = values else {
        return expected_pixel(top_left, x, y);
    };
    let owners = fixture::mode_owners();
    ck_map::reference_colour_with_mode(
        &the_fixture().data,
        STEP,
        WorldPixel {
            x: top_left[0] + i32::try_from(x).expect("small"),
            y: top_left[1] + i32::try_from(y).expect("small"),
        },
        &std::collections::BTreeSet::new(),
        &std::collections::BTreeMap::new(),
        Some(&ck_map::ModeInputs {
            owners: &owners,
            values,
        }),
    )
    .expect("the pixel's sample point fits")
    .0
}

/// The map of `input` seen from `top_left` through a `view`, enlarged four times into the window's top-left corner.
pub(crate) fn map_of(input: &Arc<VulkanMapInput>, top_left: [i32; 2], view: [u32; 2]) -> VulkanMap {
    let side = |value: u32| i32::try_from(value * 4).expect("small");
    let float = |value: u32| f32::from(u16::try_from(value).expect("small"));
    VulkanMap::new(
        Arc::clone(input),
        STEP,
        top_left,
        view,
        DestinationRect {
            left: 0,
            top: 0,
            right: side(view[0]),
            bottom: side(view[1]),
        },
        SourceRect {
            left: 0.0,
            top: 0.0,
            right: float(view[0]),
            bottom: float(view[1]),
        },
    )
    .expect("a valid map")
}

/// The reference's RGBA colour of map pixel `(x, y)` of a view at `top_left`, at [`STEP`].
pub(crate) fn expected_pixel(top_left: [i32; 2], x: u32, y: u32) -> [u8; 4] {
    the_fixture().reference(
        STEP,
        WorldPixel {
            x: top_left[0] + i32::try_from(x).expect("small"),
            y: top_left[1] + i32::try_from(y).expect("small"),
        },
    )
}

pub(crate) fn bgra(capture: &FrameCapture, x: u32, y: u32) -> [u8; 4] {
    assert_eq!(capture.format, vk::Format::B8G8R8A8_UNORM);
    let at = ((y * capture.size.0 + x) * 4) as usize;
    [
        capture.bytes[at + 2],
        capture.bytes[at + 1],
        capture.bytes[at],
        capture.bytes[at + 3],
    ]
}

fn captured(harness: &mut Harness, captures: &FrameCaptures, generation: u64) -> FrameCapture {
    let sink = Arc::clone(captures);
    harness.rig.wait("the capture", || {
        sink.lock()
            .expect("the captures")
            .iter()
            .any(|frame| frame.generation == Some(generation))
    });
    let all = captures.lock().expect("the captures");
    all.iter()
        .rev()
        .find(|frame| frame.generation == Some(generation))
        .cloned()
        .expect("the capture is there")
}

impl Harness {
    /// Publishes a frame with no map (a withdrawn one), and returns its generation.
    fn publish_without_map(&mut self) -> u64 {
        self.generation += 1;
        self.frames.publish(VulkanFrame::new(
            self.generation,
            VulkanFilter::Nearest,
            None,
            Some(Arc::new(Quads(Vec::new()))),
        ));
        self.generation
    }

    /// Publishes a frame (the next generation) that carries `map` and no scene quad.
    fn publish_map(&mut self, map: VulkanMap) {
        self.generation += 1;
        self.frames.publish(
            VulkanFrame::new(
                self.generation,
                VulkanFilter::Nearest,
                None,
                Some(Arc::new(Quads(Vec::new()))),
            )
            .with_map(Some(map)),
        );
    }

    /// Publishes a map frame with no separate scene source, as the app does for the map-only path.
    fn publish_map_only(&mut self, map: VulkanMap) {
        self.generation += 1;
        self.frames.publish(
            VulkanFrame::new(self.generation, VulkanFilter::Nearest, None, None)
                .with_map(Some(map)),
        );
    }
}

/// The first frame has the map: the presenter makes the input and the target resident, records the map pass, and its window is
/// the enlarged map over the clear.
fn a_map_in_the_first_frame_is_drawn_enlarged() {
    let mut harness = Harness::new();
    let captures: FrameCaptures = Arc::new(Mutex::new(Vec::new()));
    let input = input_with(1);
    let top_left = {
        let [x, y] = corner();
        [x - 1, y]
    };
    harness.publish_map_only(map_of(&input, top_left, [5, 3]));
    let generation = harness.generation;
    let sink = Arc::clone(&captures);
    let mut presenter =
        harness.start_with(Harness::limits(), None, harness.rig.pixel, move |hooks| {
            hooks.capture_frames = Some(sink);
        });
    let frame = captured(&mut harness, &captures, generation);
    harness.rig.wait("the map frame's metrics hand-off", || {
        presenter.metrics_totals().frames >= 1
    });
    let metrics = presenter.metrics_totals();
    assert_eq!(
        metrics.frames, 1,
        "the map frame is a drawn frame: {metrics:?}"
    );
    assert_eq!(
        metrics.skipped, 0,
        "the map frame was not skipped: {metrics:?}"
    );
    for y in 0..12 {
        for x in 0..20 {
            assert_eq!(
                bgra(&frame, x, y),
                expected_pixel(top_left, x / 4, y / 4),
                "({x}, {y})"
            );
        }
    }
    let stats = harness.probes.observer.map_stats();
    assert_eq!(
        (
            stats.inputs_created,
            stats.targets_created,
            stats.last_target
        ),
        (1, 1, (5, 3))
    );
    assert!(stats.passes >= 1, "{stats:?}");
    assert_eq!(stats.uploads, 1, "the input is uploaded once");
    harness.finish(&mut presenter);
}

/// A resize replaces the target and a new input replaces the images; the old ones are retired and destroyed only after the ledger's
/// proof of the last submit that used them. The order is established by a held frame, not by elapsed time: with the first frame
/// held, the resize waits (nothing new is made), and once it is proven the resize is drawn and the retired target is destroyed.
fn a_resize_retires_the_old_target_after_its_proof() {
    let mut harness = Harness::new();
    let held = event(&harness);
    let input = input_with(1);
    harness.publish_map(map_of(&input, corner(), [5, 3]));
    let mut presenter = harness.start_with(Harness::limits(), None, harness.rig.pixel, |hooks| {
        hooks.scene.frame_event = Some(held);
    });
    harness.rig.wait("the held frame to be submitted", || {
        count(&harness.probes.events(), |e| *e == Event::Submit) >= 1
    });
    let observer = Arc::clone(&harness.probes.observer);
    let first = observer.map_stats();
    assert_eq!((first.targets_created, first.retired), (1, 0));
    // The view grows while the first frame is unproven: the step waits, and makes nothing.
    harness.publish_map(map_of(&input, corner(), [6, 4]));
    presenter.request_redraw();
    harness.rig.idle(std::time::Duration::from_millis(150));
    let held_stats = observer.map_stats();
    assert_eq!(
        (
            held_stats.targets_created,
            held_stats.retired,
            held_stats.retired_released
        ),
        (1, 0, 0),
        "no second target while the first is in use"
    );
    assert_eq!(harness.builds().len(), 1);
    set_event(&harness, held);
    harness.wait_for("the resized frame", |harness| harness.builds().len() >= 2);
    harness.rig.wait("the old target's proof and release", || {
        let stats = observer.map_stats();
        stats.targets_created == 2 && stats.retired_released >= 1
    });
    let resized = observer.map_stats();
    assert_eq!(resized.last_target, (6, 4));
    assert_eq!(resized.retired, 1, "the old target was retired");
    assert_eq!(resized.targets_alive, 1, "and destroyed after its proof");
    // A new input (another generation) replaces the images the same way.
    harness.publish_map(map_of(&input_with(2), corner(), [6, 4]));
    harness.rig.wait("the new input", || {
        let stats = observer.map_stats();
        stats.inputs_created == 2 && stats.retired_released >= 2
    });
    let replaced = observer.map_stats();
    assert_eq!(replaced.targets_created, 2, "the view did not change");
    assert_eq!(replaced.retired, 2);
    assert_eq!(replaced.inputs_alive, 1);
    assert_eq!(replaced.uploads, 2, "each input is uploaded once");
    let ended = Rig::end(&mut presenter);
    assert_eq!(ended, CleanupResult::Proven);
    let ended_stats = observer.map_stats();
    assert_eq!(
        (ended_stats.targets_alive, ended_stats.inputs_alive),
        (0, 0),
        "the proven end destroyed what the render thread still held"
    );
    destroy_event(&harness, held);
    drop(presenter);
    let Harness { context, rig, .. } = harness;
    drop(context);
    rig.close(&ended);
}

/// What the presenter held of the map's targets at an instant: targets made, targets alive, objects retired, retired objects let go of.
pub(crate) type Residency = [u64; 4];

/// What the presenter told the publisher: the messages of its refusals and how often an attempt succeeded after one, and, when a case
/// watches the presenter, what it held at the instant of each call (the calls come from the render thread, in the step itself).
#[derive(Default)]
pub(crate) struct Told {
    pub(crate) refused: Mutex<Vec<String>>,
    pub(crate) cleared: AtomicUsize,
    reader: Mutex<Option<Box<dyn Fn() -> Residency + Send>>>,
    /// What the presenter held when each refusal was told.
    pub(crate) at_refused: Mutex<Vec<Residency>>,
    /// What the presenter held when each success after a refusal was told.
    pub(crate) at_cleared: Mutex<Vec<Residency>>,
}

impl Told {
    /// Reads the presenter's residency with `reader` inside each call from now on.
    pub(crate) fn watch(&self, reader: impl Fn() -> Residency + Send + 'static) {
        *self.reader.lock().expect("the reader") = Some(Box::new(reader));
    }

    fn held(&self) -> Option<Residency> {
        self.reader
            .lock()
            .expect("the reader")
            .as_ref()
            .map(|read| read())
    }
}

impl MapRefusalSink for Told {
    fn refused(&self, message: &str) {
        if let Some(held) = self.held() {
            self.at_refused.lock().expect("the snapshots").push(held);
        }
        self.refused
            .lock()
            .expect("the messages")
            .push(message.to_owned());
    }

    fn cleared(&self) {
        if let Some(held) = self.held() {
            self.at_cleared.lock().expect("the snapshots").push(held);
        }
        self.cleared.fetch_add(1, Ordering::SeqCst);
    }
}

/// Makes `told` read the targets of `observer`'s presenter.
pub(crate) fn watch_targets(told: &Told, observer: &Arc<ck_gpu_vulkan::Observer>) {
    let observer = Arc::clone(observer);
    told.watch(move || {
        let stats = observer.map_stats();
        [
            stats.targets_created,
            stats.targets_alive,
            stats.retired,
            stats.retired_released,
        ]
    });
}

/// Every captured frame of `generation`, in order, once there are `count` of them.
fn captures_of(
    harness: &mut Harness,
    captures: &FrameCaptures,
    generation: u64,
    count: usize,
) -> Vec<FrameCapture> {
    let sink = Arc::clone(captures);
    harness.rig.wait("the captures", || {
        sink.lock()
            .expect("the captures")
            .iter()
            .filter(|frame| frame.generation == Some(generation))
            .count()
            >= count
    });
    let all = captures.lock().expect("the captures");
    all.iter()
        .filter(|frame| frame.generation == Some(generation))
        .cloned()
        .collect()
}

/// A refused allocation is not a failure of the run: the frame is drawn without the map (never with a stale target), the publisher
/// is told, and the renderer keeps a redraw wanted by itself, so the next normally paced frame asks again with nothing republished;
/// the success is told too, and the idle resumes. The refusal is injected (the device is "out of memory" for the first attempt), so
/// the case is deterministic.
fn a_refused_allocation_is_told_retried_and_cleared() {
    let mut harness = Harness::new();
    let captures: FrameCaptures = Arc::new(Mutex::new(Vec::new()));
    let told = Arc::new(Told::default());
    let input = input_with(1);
    watch_targets(&told, &harness.probes.observer);
    let with_refusals =
        |map: VulkanMap| map.with_refusals(Arc::clone(&told) as Arc<dyn MapRefusalSink>);
    harness.publish_map(with_refusals(map_of(&input, corner(), [5, 3])));
    let generation = harness.generation;
    let sink = Arc::clone(&captures);
    let mut presenter =
        harness.start_with(Harness::limits(), None, harness.rig.pixel, move |hooks| {
            hooks.capture_frames = Some(sink);
            hooks.map_allocation_failures = 1;
        });
    // Nothing is republished: the second frame of this generation is the renderer's own retry.
    let frames = captures_of(&mut harness, &captures, generation, 2);
    assert_eq!(
        bgra(&frames[0], 2, 2),
        [26, 51, 77, 255],
        "the clear, no map"
    );
    assert_eq!(
        bgra(&frames[1], 2, 2),
        expected_pixel(corner(), 0, 0),
        "the map is drawn by the retry"
    );
    assert_eq!(told.refused.lock().expect("the messages").len(), 1);
    assert_eq!(
        told.cleared.load(Ordering::SeqCst),
        1,
        "the success is told once"
    );
    assert_eq!(
        *told.at_refused.lock().expect("the snapshots"),
        [[0, 0, 0, 0]],
        "at the refusal nothing of the map was made"
    );
    let observer = Arc::clone(&harness.probes.observer);
    let stats = observer.map_stats();
    assert_eq!((stats.inputs_created, stats.targets_created), (1, 1));
    // The idle resumes: no more frames.
    harness.rig.idle(std::time::Duration::from_millis(150));
    assert_eq!(
        harness.drawn().len(),
        2,
        "one refused frame and one retry, no loop"
    );
    harness.finish(&mut presenter);
}

/// A resize whose new target is refused while the old target is resident: the frame is drawn **without** the map (never with the old,
/// differently sized target), the old target stays resident and is not retired, the renderer retries by itself, and the retry
/// replaces the old target (retired, then destroyed after its proof).
/// A refusal ends its retry when the map is withdrawn: a frame with no map is drawn once and the renderer goes idle (the redraw want
/// belongs to the last refused attempt, not to the open episode), while the refusal keeps repeating as long as the map is wanted.
fn a_withdrawn_map_ends_the_retry() {
    let mut harness = Harness::new();
    let captures: FrameCaptures = Arc::new(Mutex::new(Vec::new()));
    let told = Arc::new(Told::default());
    let input = input_with(1);
    let with_refusals =
        |map: VulkanMap| map.with_refusals(Arc::clone(&told) as Arc<dyn MapRefusalSink>);
    harness.publish_map(with_refusals(map_of(&input, corner(), [5, 3])));
    let first = harness.generation;
    let sink = Arc::clone(&captures);
    let mut presenter =
        harness.start_with(Harness::limits(), None, harness.rig.pixel, move |hooks| {
            hooks.capture_frames = Some(sink);
            // Every attempt is refused for the whole case.
            hooks.map_allocation_failures = u32::MAX;
        });
    // The renderer keeps retrying by itself while the map is wanted: more frames of the same generation, all without the map.
    let retries = captures_of(&mut harness, &captures, first, 3);
    assert!(
        retries
            .iter()
            .all(|frame| bgra(frame, 2, 2) == [26, 51, 77, 255])
    );
    // The presenter tells every refused attempt (the publisher's side keeps one message per episode) and no success.
    assert!(told.refused.lock().expect("the messages").len() >= 3);
    assert_eq!(told.cleared.load(Ordering::SeqCst), 0);
    // The map is withdrawn: a frame without it is drawn, and then nothing more is drawn.
    let plain = harness.publish_without_map();
    let _ = captures_of(&mut harness, &captures, plain, 1);
    let drawn = captures.lock().expect("the captures").len();
    harness.rig.idle(std::time::Duration::from_millis(250));
    assert_eq!(
        captures.lock().expect("the captures").len(),
        drawn,
        "no frame is drawn after the withdrawn map's own: the retry ended"
    );
    harness.finish(&mut presenter);
}

fn a_refused_resize_draws_without_the_map_and_recovers() {
    let mut harness = Harness::new();
    let captures: FrameCaptures = Arc::new(Mutex::new(Vec::new()));
    let told = Arc::new(Told::default());
    let input = input_with(1);
    watch_targets(&told, &harness.probes.observer);
    let with_refusals =
        |map: VulkanMap| map.with_refusals(Arc::clone(&told) as Arc<dyn MapRefusalSink>);
    harness.publish_map(with_refusals(map_of(&input, corner(), [5, 3])));
    let first = harness.generation;
    let sink = Arc::clone(&captures);
    let mut presenter =
        harness.start_with(Harness::limits(), None, harness.rig.pixel, move |hooks| {
            hooks.capture_frames = Some(sink);
            // The first creation goes through; the second (the resize's target) is refused once.
            hooks.map_allocation_skip = 1;
            hooks.map_allocation_failures = 1;
        });
    let shown = captures_of(&mut harness, &captures, first, 1);
    assert_eq!(
        bgra(&shown[0], 2, 2),
        expected_pixel(corner(), 0, 0),
        "the first map"
    );
    let observer = Arc::clone(&harness.probes.observer);
    assert_eq!(observer.map_stats().targets_created, 1);
    // The view grows: the new target is refused.
    harness.publish_map(with_refusals(map_of(&input, corner(), [6, 4])));
    let second = harness.generation;
    let frames = captures_of(&mut harness, &captures, second, 2);
    assert_eq!(
        bgra(&frames[0], 2, 2),
        [26, 51, 77, 255],
        "the refused frame shows no map: not the old target"
    );
    assert_eq!(
        bgra(&frames[1], 2, 2),
        expected_pixel(corner(), 0, 0),
        "the retry draws the new map"
    );
    assert_eq!(told.refused.lock().expect("the messages").len(), 1);
    assert!(
        told.refused.lock().expect("the messages")[0].contains("target"),
        "{:?}",
        told.refused.lock().expect("the messages")
    );
    assert_eq!(told.cleared.load(Ordering::SeqCst), 1);
    // The state at the instant of the refusal, read inside the refused step itself: the old target is the only one, resident and not
    // retired. The state at the instant of the success: the new target is made and the old one retired (once) by the retry.
    assert_eq!(
        *told.at_refused.lock().expect("the snapshots"),
        [[1, 1, 0, 0]],
        "at the refusal: one target made, one alive, nothing retired or released"
    );
    let at_cleared = told.at_cleared.lock().expect("the snapshots").clone();
    assert_eq!(at_cleared.len(), 1);
    assert_eq!(
        (at_cleared[0][0], at_cleared[0][2]),
        (2, 1),
        "at the success: the new target is made and the old one retired, by the retry"
    );
    harness.rig.wait("the old target's release", || {
        let stats = observer.map_stats();
        stats.targets_created == 2 && stats.retired_released >= 1
    });
    let stats = observer.map_stats();
    assert_eq!(
        (stats.retired, stats.targets_alive, stats.last_target),
        (1, 1, (6, 4)),
        "the old target was retired once, by the retry, and destroyed after its proof"
    );
    harness.rig.idle(std::time::Duration::from_millis(150));
    assert_eq!(
        harness.drawn().len(),
        3,
        "the first frame, the refused one and the retry"
    );
    harness.finish(&mut presenter);
}

/// The scene presenter's map cases.
pub(crate) const CASES: [(&str, Case); 5] = [
    (
        "a_map_in_the_first_frame_is_drawn_enlarged",
        a_map_in_the_first_frame_is_drawn_enlarged,
    ),
    (
        "a_resize_retires_the_old_target_after_its_proof",
        a_resize_retires_the_old_target_after_its_proof,
    ),
    (
        "a_refused_allocation_is_told_retried_and_cleared",
        a_refused_allocation_is_told_retried_and_cleared,
    ),
    (
        "a_refused_resize_draws_without_the_map_and_recovers",
        a_refused_resize_draws_without_the_map_and_recovers,
    ),
    (
        "a_withdrawn_map_ends_the_retry",
        a_withdrawn_map_ends_the_retry,
    ),
];
