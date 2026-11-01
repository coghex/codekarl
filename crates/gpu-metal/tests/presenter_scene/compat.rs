//! The compatibility quad's first-frame gate and the held replacement image on a real window (CK-119, milestone 5, issue
//! 5d-2): the frame that would first show the legacy quad waits for its image (never the checker, never a busy retry), a
//! later replacement keeps the last image drawn on screen until the new one resolves, a failed compatibility upload is the
//! presenter's one `Failed`, and new asynchronous resources are never gated. The cases use the CK-114 test driver of the
//! parent module, whose "service verdicts" a case gives late (`defer_commit` and `commit_now`): a revision that is prepared
//! but not committed is a pending image. Each case names its negative control.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use ck_gpu_metal::{
    CapturedFrame, DestinationRect, Gate, InstanceRole, MetalFilter, MetalFrame, MetalInstance,
    MetalPresenter, MetalSceneSource, TextureTestHooks, TicketEnding, WaitPause,
};

use super::{
    BINDLESS_DEFAULT, CleanupResult, Duration, PresenterReport, Rig, Scene, TestHooks, end, limits,
    limits_adopting, lock, reports_until, until,
};

const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
/// The clear colour of the rig's presenters, [0.1, 0.2, 0.3], as bytes.
const CLEAR: [u8; 4] = [26, 51, 77, 255];
/// An estimate no texture budget accepts: an upload forced to it fails as a per-item resource error.
const TOO_BIG: u64 = 1 << 50;

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> DestinationRect {
    DestinationRect {
        left,
        top,
        right,
        bottom,
    }
}

fn scene_instance(key: u32, destination: DestinationRect) -> MetalInstance<u32> {
    MetalInstance {
        key,
        destination,
        source: WHOLE,
        role: InstanceRole::Scene,
    }
}

fn compat_instance(key: u32, destination: DestinationRect) -> MetalInstance<u32> {
    MetalInstance {
        key,
        destination,
        source: WHOLE,
        role: InstanceRole::Compat,
    }
}

/// A source that emits a fixed list (the compatibility instance first, when there is one).
struct Listed(Vec<MetalInstance<u32>>);

impl MetalSceneSource<u32> for Listed {
    fn place(&self, _width: u32, _height: u32, out: &mut dyn FnMut(MetalInstance<u32>)) {
        for instance in &self.0 {
            out(*instance);
        }
    }
}

/// Publishes a frame whose compatibility quad is `compat` (key and rectangle) and whose scene quads are `others`.
fn publish(
    scene: &Scene,
    generation: u64,
    compat: Option<(u32, DestinationRect)>,
    others: Vec<MetalInstance<u32>>,
) {
    let mut instances: Vec<MetalInstance<u32>> = compat
        .map(|(key, destination)| compat_instance(key, destination))
        .into_iter()
        .collect();
    instances.extend(others);
    scene.frames.publish(
        MetalFrame::new(
            generation,
            MetalFilter::Nearest,
            None,
            Some(Arc::new(Listed(instances))),
        )
        .with_compat_key(compat.map(|(key, _)| key)),
    );
}

/// Publishes a frame whose compatibility quad `key` places to nothing (clipped away): the key is on the frame, no instance.
fn publish_clipped(scene: &Scene, generation: u64, key: u32) {
    scene.frames.publish(
        MetalFrame::new(
            generation,
            MetalFilter::Nearest,
            None,
            Some(Arc::new(Listed(Vec::new()))),
        )
        .with_compat_key(Some(key)),
    );
}

/// An 8 x 8 image of one colour for the driver to offer for `key` at revision 1.
fn offer(scene: &Scene, key: u32, color: [u8; 4]) {
    let mut state = scene.driver();
    state.offers.push_back((key, 1, 8, 8));
    state.colors.push((key, 1, color));
}

/// The RGBA pixel at (x, y) of a captured (BGRA) frame.
fn px(frame: &CapturedFrame, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * frame.width + x) * 4) as usize;
    [
        frame.pixels[offset + 2],
        frame.pixels[offset + 1],
        frame.pixels[offset],
        frame.pixels[offset + 3],
    ]
}

fn capturing(hooks: &mut TestHooks) {
    hooks.capture_frames = true;
}

/// Waits (pumping) for one more captured frame than `before` and returns the latest.
fn next_capture(rig: &mut Rig, scene: &Scene, before: i64) -> CapturedFrame {
    until(rig, "a captured frame", || {
        scene.probes.observer.captures() > before
    });
    scene
        .probes
        .observer
        .captured_frame()
        .expect("a captured frame")
}

/// Publishes a frame (compatibility quad `compat`, scene quads `others`), waits for the presenter to have drawn exactly that
/// generation and for its capture, and returns the latest captured frame. Only for frames that are drawn (not held back).
fn show(
    rig: &mut Rig,
    scene: &Scene,
    presenter: &MetalPresenter,
    generation: u64,
    compat: Option<(u32, DestinationRect)>,
    others: Vec<MetalInstance<u32>>,
) -> CapturedFrame {
    publish(scene, generation, compat, others);
    until(rig, "the published generation to be drawn", || {
        presenter.drawn_generation() == Some(generation)
    });
    // The capture of a submitted frame arrives with its completion.
    rig.pump_for(Duration::from_millis(150));
    scene
        .probes
        .observer
        .captured_frame()
        .expect("a captured frame")
}

/// Waits until the driver has been told the revision is prepared (uploaded, not yet committed).
fn prepared(rig: &mut Rig, scene: &Scene, key: u32) {
    until(rig, "the upload to be prepared", || {
        scene.driver().prepared.contains(&(key, 1))
    });
}

/// The service's late verdict: commit `key`'s revision 1 at the next pass, and wake the render thread for it.
fn commit(scene: &Scene, presenter: &MetalPresenter, key: u32) {
    scene.driver().commit_now.push((key, 1));
    presenter.texture_wake()();
}

fn discarded(scene: &Scene) -> usize {
    scene
        .tickets()
        .iter()
        .filter(|ending| matches!(ending, TicketEnding::Discarded))
        .count()
}

fn failures(reports: &[PresenterReport]) -> usize {
    reports
        .iter()
        .filter(|report| matches!(report, PresenterReport::Failed { .. }))
        .count()
}

// ---------------------------------------------------------------------------------------------
// The initial gate
// ---------------------------------------------------------------------------------------------

/// A compatibility quad set before the window opens, its image pending: no frame is committed (the committed-frame counter
/// stays 0), no checker is ever drawn for it, the render thread and the main thread keep running, and the first committed
/// frame shows its image. While held after recording but before drain, one stats snapshot already includes the completed gate hold
/// with that frame. Negative control: without the gate the first frame shows the checker (magenta) for it.
pub(crate) fn scene_compat_gate_holds_the_first_frame_until_the_image_resolves() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    let before_drain = Arc::new(AtomicI32::new(1));
    scene.driver().defer_commit.push(1);
    offer(&scene, 1, RED);
    publish(&scene, 7, Some((1, rect(0, 0, 32, 32))), Vec::new());
    let mut presenter = scene.start(&rig, limits(), None, |hooks| {
        hooks.capture_frames = true;
        hooks.hold_before_drain = Some(Arc::clone(&before_drain));
    });
    until(&mut rig, "the first attempt to be held", || {
        presenter.stats().gate_holds >= 1
    });
    prepared(&mut rig, &scene, 1);
    // The window is pumped (the main thread is not blocked) while the image is pending.
    rig.pump_for(Duration::from_millis(400));
    assert_eq!(
        scene.probes.frames(),
        0,
        "no frame while the image is pending"
    );
    assert_eq!(scene.probes.observer.committed(), 0);
    assert_eq!(
        scene.probes.observer.captures(),
        0,
        "no checker was ever drawn"
    );
    assert_eq!(presenter.drawn_generation(), None);
    assert!(presenter.take_report().is_none(), "no FirstFrameQueued yet");
    assert!(presenter.stats().gate_holds >= 1);
    // The commit alone lets the frame through.
    commit(&scene, &presenter, 1);
    until(&mut rig, "the committed frame held before drain", || {
        before_drain.load(Ordering::SeqCst) == -1
    });
    assert_eq!(
        scene.probes.observer.first_frame_hook_report_visible(),
        None,
        "the hold is before the drawn-frame hook"
    );
    let stats = presenter.stats();
    before_drain.store(0, Ordering::SeqCst);
    until(&mut rig, "the drawn-frame hook", || {
        scene.probes.observer.first_frame_hook_report_visible() == Some(true)
    });
    assert_eq!(
        scene.probes.observer.first_frame_hook_report_visible(),
        Some(true),
        "FirstFrameQueued is visible when the first drawn-frame hook advances after the held stats snapshot"
    );
    assert_eq!(
        presenter.take_report(),
        Some(PresenterReport::FirstFrameQueued)
    );
    assert_eq!(
        (
            stats.last.instances,
            stats.last.resolved,
            stats.last.checker
        ),
        (1, 1, 0),
        "the compatibility quad was drawn with its image, never the checker"
    );
    assert_eq!(
        stats.frames, 1,
        "the held snapshot includes the committed frame: {stats:?}"
    );
    assert!(
        stats.gate_longest_hold_micros > 0,
        "the same held snapshot includes the frame's compatibility hold: {stats:?}"
    );
    assert_eq!(presenter.drawn_generation(), Some(7));
    let frame = next_capture(&mut rig, &scene, 0);
    assert_eq!(px(&frame, 16, 16), RED);
    assert_ne!(px(&frame, 16, 16), MAGENTA);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A stop while the first frame is held ends `Proven` promptly, with no frame committed. Negative control: a retry loop
/// that never checks the stop would leave the case timing out.
pub(crate) fn scene_compat_stop_while_the_gate_holds_ends_proven() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().defer_commit.push(1);
    offer(&scene, 1, RED);
    publish(&scene, 1, Some((1, rect(0, 0, 32, 32))), Vec::new());
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    until(&mut rig, "the first attempt to be held", || {
        presenter.stats().gate_holds >= 1
    });
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    assert_eq!(scene.probes.frames(), 0);
    assert_eq!(scene.probes.observer.committed(), 0);
}

/// The blocked state waits on its condition and is not busy: after one held attempt the ticket counter stops moving while
/// the gating key has no committed revision (a pending frame-quad texture, its commit and a same-key frame do not unblock
/// it), its commit alone resumes it, and a frame that clears the compatibility quad, or replaces it, unblocks it.
/// Negative control: keeping the frame merely wanted shows the counter climbing while blocked.
pub(crate) fn scene_compat_blocked_state_waits_and_is_not_busy() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    {
        let mut state = scene.driver();
        state.defer_commit.extend([1, 5]);
    }
    offer(&scene, 1, RED);
    offer(&scene, 5, GREEN);
    publish(
        &scene,
        1,
        Some((1, rect(0, 0, 32, 32))),
        vec![scene_instance(5, rect(64, 0, 96, 32))],
    );
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    until(&mut rig, "the first attempt to be held", || {
        presenter.stats().gate_holds >= 1
    });
    prepared(&mut rig, &scene, 1);
    prepared(&mut rig, &scene, 5);
    // The entry handshake: one held attempt, then the counters stop moving while nothing changes.
    rig.pump_for(Duration::from_millis(200));
    let held = (discarded(&scene), presenter.stats().gate_holds);
    rig.pump_for(Duration::from_millis(500));
    assert_eq!(
        (discarded(&scene), presenter.stats().gate_holds),
        held,
        "no frame is built or ticketed while the gating image is pending"
    );
    // An unrelated frame-quad texture committing does not unblock it.
    commit(&scene, &presenter, 5);
    rig.pump_for(Duration::from_millis(400));
    assert_eq!(
        (discarded(&scene), presenter.stats().gate_holds),
        held,
        "an unrelated commit does not wake the gate"
    );
    assert_eq!(scene.probes.frames(), 0);
    // The gating commit resumes it: the next frame draws the image (and adopts the unrelated texture on the way).
    commit(&scene, &presenter, 1);
    until(&mut rig, "the frame after the commit", || {
        scene.probes.frames() >= 1
    });
    let frame = next_capture(&mut rig, &scene, 0);
    assert_eq!(px(&frame, 16, 16), RED);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);

    // A frame that clears the compatibility quad unblocks it; so does one that replaces it with a resolved image.
    for replacement in [false, true] {
        let mut rig = Rig::open();
        let scene = Scene::new();
        scene.driver().defer_commit.push(1);
        offer(&scene, 1, RED);
        offer(&scene, 2, BLUE);
        publish(&scene, 1, Some((1, rect(0, 0, 32, 32))), Vec::new());
        let mut presenter = scene.start(&rig, limits(), None, capturing);
        until(&mut rig, "the first attempt to be held", || {
            presenter.stats().gate_holds >= 1
        });
        rig.pump_for(Duration::from_millis(200));
        assert_eq!(scene.probes.frames(), 0);
        if replacement {
            publish(&scene, 2, Some((2, rect(40, 40, 72, 72))), Vec::new());
        } else {
            publish(&scene, 2, None, Vec::new());
        }
        until(&mut rig, "the frame after the new publication", || {
            scene.probes.frames() >= 1
        });
        let frame = next_capture(&mut rig, &scene, 0);
        if replacement {
            assert_eq!(px(&frame, 56, 56), BLUE, "the replacement's image");
        } else {
            assert_eq!(px(&frame, 16, 16), CLEAR, "no compatibility quad any more");
        }
        assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    }
}

/// A publication that lands after the render thread read the gate and before it decides to wait is not lost: the thread is
/// stopped at exactly that point (the first gated attempt done, nothing left to do), one frame that clears the compat quad, or
/// replaces it with a resolved image, is published, and the thread then goes on **with no other wake** (no second publication,
/// no texture completion, no redraw). Negative control: deciding to wait on the gate read before the pause (the stale
/// predicate) sleeps with the new frame unseen and the generation never draws.
pub(crate) fn scene_compat_a_publication_before_the_wait_is_not_lost() {
    for replacement in [false, true] {
        let mut rig = Rig::open();
        let scene = Scene::new();
        scene.driver().defer_commit.push(1);
        offer(&scene, 1, RED);
        offer(&scene, 2, BLUE);
        publish(&scene, 1, Some((1, rect(0, 0, 32, 32))), Vec::new());
        let pause = Arc::new(WaitPause::new());
        let mut presenter = scene.start(&rig, limits(), None, |hooks| {
            hooks.capture_frames = true;
            hooks.wait_pause = Some(Arc::clone(&pause));
        });
        until(&mut rig, "the first attempt to be held", || {
            presenter.stats().gate_holds >= 1
        });
        prepared(&mut rig, &scene, 1);
        prepared(&mut rig, &scene, 2);
        // The thread goes round once more with nothing changed and stops where it would decide to wait.
        pause.arm();
        presenter.texture_wake()();
        until(&mut rig, "the thread to reach the wait decision", || {
            pause.arrived()
        });
        assert_eq!(scene.probes.frames(), 0);
        if replacement {
            publish(&scene, 2, Some((2, rect(40, 40, 72, 72))), Vec::new());
        } else {
            publish(&scene, 2, None, Vec::new());
        }
        pause.release();
        until(&mut rig, "the published frame to be drawn", || {
            presenter.drawn_generation() == Some(2)
        });
        rig.pump_for(Duration::from_millis(150));
        let frame = scene
            .probes
            .observer
            .captured_frame()
            .expect("a captured frame");
        if replacement {
            assert_eq!(px(&frame, 56, 56), BLUE);
        } else {
            assert_eq!(px(&frame, 16, 16), CLEAR);
        }
        assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    }
}

/// The gating revision committed **last**, behind older unrelated commits, with one adoption per boundary: bounded frame
/// boundaries adopt through the backlog (every unsuccessful retry's ticket is discarded), the first committed frame then
/// shows the image, and the thread idles. A separate half: with the gating upload genuinely pending, committing the others
/// leaves the frame-build counters still behind the idle handshake. Negative control: treating the commit as a one-shot
/// unblock strands the frame after the first unrelated adoption.
pub(crate) fn scene_compat_committed_behind_older_commits_is_adopted_in_bounded_boundaries() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().adopt_limit = 1;
    scene.driver().defer_commit.extend([1, 5, 6, 7]);
    for (key, color) in [(1, RED), (5, GREEN), (6, GREEN), (7, GREEN)] {
        offer(&scene, key, color);
    }
    publish(
        &scene,
        1,
        Some((1, rect(0, 0, 32, 32))),
        vec![
            scene_instance(5, rect(64, 0, 96, 32)),
            scene_instance(6, rect(96, 0, 128, 32)),
            scene_instance(7, rect(128, 0, 160, 32)),
        ],
    );
    let mut presenter = scene.start(&rig, limits_adopting(1), None, capturing);
    until(&mut rig, "the first attempt to be held", || {
        presenter.stats().gate_holds >= 1
    });
    for key in [1, 5, 6, 7] {
        prepared(&mut rig, &scene, key);
    }
    rig.pump_for(Duration::from_millis(200));
    let before = discarded(&scene);
    // The gating revision is committed last, in the same pass as the three older ones; its commit is the final wake.
    {
        let mut state = scene.driver();
        state.commit_now.extend([(5, 1), (6, 1), (7, 1), (1, 1)]);
    }
    presenter.texture_wake()();
    until(&mut rig, "the first committed frame", || {
        scene.probes.frames() >= 1
    });
    let frame = next_capture(&mut rig, &scene, 0);
    assert_eq!(
        px(&frame, 16, 16),
        RED,
        "the first committed frame shows the image"
    );
    assert_eq!(
        discarded(&scene) - before,
        3,
        "one discarded ticket per older adoption: 5, then 6, then 7, then the gating revision"
    );
    rig.pump_for(Duration::from_millis(300));
    let settled = (scene.tickets().len(), scene.probes.frames());
    rig.pump_for(Duration::from_millis(400));
    assert_eq!(
        (scene.tickets().len(), scene.probes.frames()),
        settled,
        "the thread idles"
    );
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);

    // Genuinely pending: the others commit, the gating image does not.
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().adopt_limit = 1;
    scene.driver().defer_commit.extend([1, 5, 6]);
    for (key, color) in [(1, RED), (5, GREEN), (6, GREEN)] {
        offer(&scene, key, color);
    }
    publish(
        &scene,
        1,
        Some((1, rect(0, 0, 32, 32))),
        vec![
            scene_instance(5, rect(64, 0, 96, 32)),
            scene_instance(6, rect(96, 0, 128, 32)),
        ],
    );
    let mut presenter = scene.start(&rig, limits_adopting(1), None, capturing);
    until(&mut rig, "the first attempt to be held", || {
        presenter.stats().gate_holds >= 1
    });
    for key in [1, 5, 6] {
        prepared(&mut rig, &scene, key);
    }
    {
        let mut state = scene.driver();
        state.commit_now.extend([(5, 1), (6, 1)]);
    }
    presenter.texture_wake()();
    rig.pump_for(Duration::from_millis(300));
    let idle = (discarded(&scene), presenter.stats().gate_holds);
    rig.pump_for(Duration::from_millis(500));
    assert_eq!(
        (discarded(&scene), presenter.stats().gate_holds),
        idle,
        "behind the idle handshake the counters stop"
    );
    assert_eq!(scene.probes.frames(), 0);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// Frame quads whose textures are pending draw the checker at once; a frame is held back only when the compatibility image
/// does not resolve and none was drawn yet; with no compatibility quad (or one that places to nothing) nothing is ever held
/// back. Negative control: gating every unresolved instance would hold the first frame here.
pub(crate) fn scene_compat_never_gates_new_asynchronous_resources() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().defer_commit.push(5);
    offer(&scene, 5, GREEN);
    publish(&scene, 1, None, vec![scene_instance(5, rect(0, 0, 32, 32))]);
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    let frame = next_capture(&mut rig, &scene, 0);
    assert_eq!(
        px(&frame, 4, 4),
        MAGENTA,
        "an unresolved frame quad is the checker"
    );
    assert_eq!(presenter.stats().gate_holds, 0);
    // A compatibility quad that places to nothing (its key is on the frame, no instance) holds nothing back.
    publish_clipped(&scene, 2, 9);
    until(&mut rig, "the clipped frame to be drawn", || {
        presenter.drawn_generation() == Some(2)
    });
    rig.pump_for(Duration::from_millis(150));
    let frame = scene
        .probes
        .observer
        .captured_frame()
        .expect("a captured frame");
    assert_eq!(px(&frame, 4, 4), CLEAR);
    assert_eq!(presenter.stats().gate_holds, 0);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

// ---------------------------------------------------------------------------------------------
// The held image
// ---------------------------------------------------------------------------------------------

/// After the first image is shown, a replacement whose image is pending: frames keep showing the old image at the old
/// rectangle, frame quads moved meanwhile are drawn at their new places, the old key's retirement waits for the hold, and
/// once the replacement resolves the next frame shows it at the new rectangle and the old key's `KeyRetired` comes.
/// Negative control: without the hold, the old key's retirement releases the image and a frame draws the checker or nothing.
#[expect(
    clippy::too_many_lines,
    reason = "one replacement's whole life is asserted in order"
)]
pub(crate) fn scene_compat_keeps_the_old_image_while_a_replacement_is_pending() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().defer_commit.push(2);
    offer(&scene, 1, RED);
    offer(&scene, 3, GREEN);
    offer(&scene, 2, BLUE);
    publish(
        &scene,
        1,
        Some((1, rect(0, 0, 32, 32))),
        vec![scene_instance(3, rect(96, 0, 128, 32))],
    );
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    // Both images adopted (the first frames may still show the frame quad as the checker), then a frame that shows them.
    until(&mut rig, "both images adopted", || {
        let adopted: Vec<(u32, u64)> = scene.driver().adopted.iter().flatten().copied().collect();
        adopted.contains(&(1, 1)) && adopted.contains(&(3, 1))
    });
    let frame = show(
        &mut rig,
        &scene,
        &presenter,
        10,
        Some((1, rect(0, 0, 32, 32))),
        vec![scene_instance(3, rect(96, 0, 128, 32))],
    );
    assert_eq!((px(&frame, 16, 16), px(&frame, 112, 16)), (RED, GREEN));
    prepared(&mut rig, &scene, 2);
    // The replacement is published with a new rectangle while its image is pending; the frame quad moves.
    let frame = show(
        &mut rig,
        &scene,
        &presenter,
        11,
        Some((2, rect(40, 40, 72, 72))),
        vec![scene_instance(3, rect(128, 0, 160, 32))],
    );
    assert_eq!(
        px(&frame, 16, 16),
        RED,
        "the old image stays, at the old rectangle"
    );
    assert_eq!(
        px(&frame, 56, 56),
        CLEAR,
        "nothing at the new rectangle yet"
    );
    assert_eq!(
        px(&frame, 144, 16),
        GREEN,
        "the frame quad is drawn at its new place"
    );
    assert_eq!(px(&frame, 112, 16), CLEAR);
    let stats = presenter.stats();
    assert!(stats.held_compat_frames >= 1, "{stats:?}");
    assert_ne!(stats.held_compat_revision, 0, "{stats:?}");
    // The writer retires the old key (CK-118): the hold keeps it, and its `KeyRetired` waits.
    scene.driver().retire.push(1);
    presenter.texture_wake()();
    let frame = show(
        &mut rig,
        &scene,
        &presenter,
        12,
        Some((2, rect(40, 40, 72, 72))),
        vec![scene_instance(3, rect(128, 0, 160, 32))],
    );
    rig.pump_for(Duration::from_millis(300));
    assert_eq!(
        px(&frame, 16, 16),
        RED,
        "the retired key's image is still drawn"
    );
    assert!(
        !scene.driver().retired.contains(&1),
        "the old key is not reported retired while it is held"
    );
    // The replacement resolves.
    commit(&scene, &presenter, 2);
    // Frames are asked for until one shows the replacement (the commit makes one wanted by itself; this only bounds the wait).
    let mut generation = 13_u64;
    let frame = loop {
        publish(
            &scene,
            generation,
            Some((2, rect(40, 40, 72, 72))),
            vec![scene_instance(3, rect(128, 0, 160, 32))],
        );
        generation += 1;
        rig.pump_for(Duration::from_millis(50));
        let latest = scene
            .probes
            .observer
            .captured_frame()
            .expect("a captured frame");
        if px(&latest, 56, 56) == BLUE {
            break latest;
        }
        assert!(generation < 140, "the replacement was never drawn");
    };
    assert_eq!(
        px(&frame, 56, 56),
        BLUE,
        "the new image at the new rectangle"
    );
    assert_eq!(px(&frame, 16, 16), CLEAR, "the old image is gone");
    until(&mut rig, "the old key's retirement", || {
        scene.driver().retired.contains(&1)
    });
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// No rollback: A drawn, B published (pending) and then drawn, C published pending: every later frame draws B until C
/// resolves, never A; replacements published while nothing resolves leave the renderer with the newest image it drew; the
/// hold moves to each newer image drawn and at most one old revision is held. Negative control: a presenter that picked the
/// "previous" image from the published frame could fall back to A.
pub(crate) fn scene_compat_never_goes_back_to_an_older_image() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().defer_commit.extend([2, 3, 4, 5]);
    for (key, color) in [(1, RED), (2, GREEN), (3, BLUE), (4, MAGENTA), (5, MAGENTA)] {
        offer(&scene, key, color);
    }
    let at = rect(0, 0, 32, 32);
    publish(&scene, 1, Some((1, at)), Vec::new());
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    assert_eq!(px(&next_capture(&mut rig, &scene, 0), 16, 16), RED);
    for key in [2, 3, 4, 5] {
        prepared(&mut rig, &scene, key);
    }
    let first_hold = presenter.stats().held_compat_revision;
    assert_ne!(first_hold, 0);
    // B published, pending: A stays. Then B resolves and is drawn: from now on B is what is kept.
    let frame = show(&mut rig, &scene, &presenter, 2, Some((2, at)), Vec::new());
    assert_eq!(px(&frame, 16, 16), RED);
    commit(&scene, &presenter, 2);
    let mut generation = 20_u64;
    loop {
        publish(&scene, generation, Some((2, at)), Vec::new());
        generation += 1;
        rig.pump_for(Duration::from_millis(50));
        if scene
            .probes
            .observer
            .captured_frame()
            .is_some_and(|frame| px(&frame, 16, 16) == GREEN)
        {
            break;
        }
        assert!(generation < 120, "B was never drawn");
    }
    // C, then D and E, published while nothing resolves: B (the newest drawn) stays, never A.
    for (generation, key) in [(200, 3), (201, 4), (202, 5)] {
        let frame = show(
            &mut rig,
            &scene,
            &presenter,
            generation,
            Some((key, at)),
            Vec::new(),
        );
        assert_eq!(
            px(&frame, 16, 16),
            GREEN,
            "generation {generation} draws B, never A"
        );
    }
    rig.pump_for(Duration::from_millis(200));
    assert_eq!(
        px(
            &scene.probes.observer.captured_frame().expect("a frame"),
            16,
            16
        ),
        GREEN
    );
    // At most one old revision is held: A was released when B was drawn.
    scene.driver().retire.push(1);
    presenter.texture_wake()();
    until(&mut rig, "A's retirement (its hold moved on to B)", || {
        scene.driver().retired.contains(&1)
    });
    assert_ne!(presenter.stats().held_compat_revision, 0);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A `clear_compat` (a frame with no compatibility quad) during a pending replacement removes the compatibility quad from
/// the next frame and releases the hold. Negative control: keeping the hold keeps the old key from ever being reported
/// retired.
pub(crate) fn scene_compat_clear_releases_the_hold() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().defer_commit.push(2);
    offer(&scene, 1, RED);
    offer(&scene, 2, BLUE);
    let at = rect(0, 0, 32, 32);
    publish(&scene, 1, Some((1, at)), Vec::new());
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    prepared(&mut rig, &scene, 2);
    let frame = show(&mut rig, &scene, &presenter, 2, Some((2, at)), Vec::new());
    assert_eq!(px(&frame, 16, 16), RED);
    scene.driver().retire.push(1);
    presenter.texture_wake()();
    rig.pump_for(Duration::from_millis(200));
    assert!(!scene.driver().retired.contains(&1), "held");
    // Cleared: the next frame has no compatibility quad, and the hold goes.
    let frame = show(&mut rig, &scene, &presenter, 3, None, Vec::new());
    assert_eq!(px(&frame, 16, 16), CLEAR);
    until(&mut rig, "the hold to be released", || {
        presenter.stats().held_compat_revision == 0
    });
    until(&mut rig, "the old key's retirement", || {
        scene.driver().retired.contains(&1)
    });
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// A pending replacement whose quad is clipped away (placed to nothing) is still the frame's compatibility entry: until it
/// resolves the last image drawn stays on screen at its own rectangle (the key is not reported retired either), whether the
/// frame places it or not; once it resolves, its own placement rules, which is to draw nothing, and the hold goes. Negative
/// control: treating the clipped entry as no entry (or asking the held image only for a placed one) makes the old image
/// vanish while the replacement is still pending.
pub(crate) fn scene_compat_clipped_replacement_keeps_the_hold() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().defer_commit.push(2);
    offer(&scene, 1, RED);
    offer(&scene, 2, BLUE);
    let at = rect(0, 0, 32, 32);
    publish(&scene, 1, Some((1, at)), Vec::new());
    let mut presenter = scene.start(&rig, limits(), None, capturing);
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    prepared(&mut rig, &scene, 2);
    let drawn_at = |rig: &mut Rig, generation: u64| {
        until(rig, "the frame to be drawn", || {
            presenter.drawn_generation() == Some(generation)
        });
        rig.pump_for(Duration::from_millis(150));
        scene
            .probes
            .observer
            .captured_frame()
            .expect("a captured frame")
    };
    publish_clipped(&scene, 2, 2);
    let frame = drawn_at(&mut rig, 2);
    assert_eq!(
        px(&frame, 16, 16),
        RED,
        "the old image stays while the clipped replacement is pending"
    );
    assert_ne!(presenter.stats().held_compat_revision, 0, "the hold stays");
    scene.driver().retire.push(1);
    presenter.texture_wake()();
    rig.pump_for(Duration::from_millis(300));
    assert!(
        !scene.driver().retired.contains(&1),
        "the old key is still held by the compatibility hold"
    );
    // Placed, still pending: the old image at its old rectangle, nothing at the new one.
    let frame = show(
        &mut rig,
        &scene,
        &presenter,
        3,
        Some((2, rect(40, 40, 72, 72))),
        Vec::new(),
    );
    assert_eq!((px(&frame, 16, 16), px(&frame, 56, 56)), (RED, CLEAR));
    // Clipped again, still pending: the old image is still there.
    publish_clipped(&scene, 4, 2);
    let frame = drawn_at(&mut rig, 4);
    assert_eq!(px(&frame, 16, 16), RED);
    // The replacement resolves while it is clipped: its placement is nothing, so nothing is drawn, and the hold goes.
    commit(&scene, &presenter, 2);
    until(&mut rig, "the old key's retirement", || {
        scene.driver().retired.contains(&1)
    });
    rig.pump_for(Duration::from_millis(150));
    let frame = scene
        .probes
        .observer
        .captured_frame()
        .expect("a captured frame");
    assert_eq!(
        px(&frame, 16, 16),
        CLEAR,
        "the resolved clipped quad draws nothing"
    );
    assert_eq!(presenter.stats().held_compat_revision, 0);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

// ---------------------------------------------------------------------------------------------
// Failure
// ---------------------------------------------------------------------------------------------

/// A compatibility upload that fails as a resource error is the presenter's one `Failed` report (the existing lifecycle),
/// with no hang, while the driver still sees the ordinary `UploadFailed` (the service's status); a frame-quad upload that
/// fails does not touch the gate. Negative control: ignoring the failure leaves the gate waiting for ever.
pub(crate) fn scene_compat_upload_failure_is_one_failed_and_never_a_hang() {
    let mut rig = Rig::open();
    let scene = Scene::new();
    offer(&scene, 1, RED);
    publish(&scene, 1, Some((1, rect(0, 0, 32, 32))), Vec::new());
    let hooks = TextureTestHooks {
        force_estimate_at: Some((1, TOO_BIG)),
        ..TextureTestHooks::default()
    };
    let mut presenter = scene.start(&rig, limits(), Some(hooks), capturing);
    let mut reports = Vec::new();
    assert!(
        reports_until(&mut rig, &presenter, &mut reports, 1),
        "no report"
    );
    rig.pump_for(Duration::from_millis(300));
    while let Some(report) = presenter.take_report() {
        reports.push(report);
    }
    assert_eq!(failures(&reports), 1, "exactly one Failed: {reports:?}");
    assert!(
        reports.iter().any(|report| matches!(report, PresenterReport::Failed { message } if message.contains("legacy quad"))),
        "{reports:?}"
    );
    assert_eq!(scene.probes.frames(), 0);
    {
        let state = scene.driver();
        assert!(
            state.upload_failed.iter().any(|(key, ..)| *key == 1),
            "the driver (so the service's status) still sees the failure"
        );
        assert_eq!(
            state.compat_failures_seen, 0,
            "no driver takes the presenter's observation"
        );
    }
    assert_eq!(presenter.stats().compat_upload_failures, 1);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);

    // A frame quad's failure does not touch the gate: no Failed, and the compatibility image still gets through.
    let mut rig = Rig::open();
    let scene = Scene::new();
    scene.driver().defer_commit.push(1);
    offer(&scene, 3, GREEN);
    offer(&scene, 1, RED);
    publish(
        &scene,
        1,
        Some((1, rect(0, 0, 32, 32))),
        vec![scene_instance(3, rect(64, 0, 96, 32))],
    );
    let hooks = TextureTestHooks {
        force_estimate_at: Some((1, TOO_BIG)),
        ..TextureTestHooks::default()
    };
    let mut presenter = scene.start(&rig, limits(), Some(hooks), capturing);
    until(&mut rig, "the first attempt to be held", || {
        presenter.stats().gate_holds >= 1
    });
    prepared(&mut rig, &scene, 1);
    rig.pump_for(Duration::from_millis(300));
    assert!(
        presenter.take_report().is_none(),
        "an unrelated failure is not the presenter's"
    );
    assert!(
        scene
            .driver()
            .upload_failed
            .iter()
            .any(|(key, ..)| *key == 3)
    );
    commit(&scene, &presenter, 1);
    until(&mut rig, "the first frame", || scene.probes.frames() >= 1);
    let frame = next_capture(&mut rig, &scene, 0);
    assert_eq!(px(&frame, 16, 16), RED);
    assert_eq!(
        px(&frame, 68, 4),
        MAGENTA,
        "the failed frame quad is the checker"
    );
    assert_eq!(presenter.stats().compat_upload_failures, 0);
    assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
}

/// The presenter marks the compatibility key from the latest published frame before every pump, so a failure of it is the
/// presenter's one `Failed` whether or not a frame carrying the key was built yet, and also when the compatibility quad
/// places to nothing (the ledger's table of marked, unmarked, other and cleared keys is `verify_ledger_compat_hold`). The
/// frame carrying the key is wanted but held at the presenter's gate while the upload fails, so the failure is waiting
/// when its first pump runs. Then the compatibility cleared or replaced before a late failure: no `Failed`, only the
/// driver's `UploadFailed`. Negative control: marking the key only when a frame is built (not by the pump) finds no `Failed` in the first half.
#[expect(
    clippy::too_many_lines,
    reason = "the two halves of the classification share their setup"
)]
pub(crate) fn scene_compat_failure_is_classified_before_any_frame_was_built() {
    // The frame carrying the key is published before the presenter starts. Hold the image's own completion so its failed
    // upload stays unfinished independently of the checker's completion queue; the checker is then let through and the
    // first frame, now wanted, is held at the presenter's gate while the image's failure completes. The held frame's
    // boundary pass is the first pump to see the failure, with no frame source ever called.
    for clipped in [false, true] {
        let mut rig = Rig::open();
        let mut scene = Scene::new();
        let checker = Arc::new(AtomicI32::new(0));
        let image_completion = Arc::new(AtomicI32::new(0));
        scene.checker_hooks = Some(TextureTestHooks {
            completion_gate: Some(Arc::clone(&checker)),
            ..TextureTestHooks::default()
        });
        let gate = Arc::new(Gate::new());
        offer(&scene, 1, RED);
        if clipped {
            publish_clipped(&scene, 1, 1);
        } else {
            publish(&scene, 1, Some((1, rect(0, 0, 32, 32))), Vec::new());
        }
        let hooks = TextureTestHooks {
            force_estimate_at: Some((1, TOO_BIG)),
            completion_gate: Some(Arc::clone(&image_completion)),
            ..TextureTestHooks::default()
        };
        let mut presenter = scene.start(&rig, limits(), Some(hooks), |hooks| {
            hooks.capture_frames = true;
            hooks.gate = Some(Arc::clone(&gate));
        });
        presenter.texture_wake()();
        until(&mut rig, "the image to be offered", || {
            scene.driver().offers.is_empty()
        });
        rig.pump_for(Duration::from_millis(300));
        assert!(presenter.take_report().is_none());
        // The checker is let through: the first frame is wanted and held at the gate; the failure completes meanwhile.
        checker.store(1, Ordering::SeqCst);
        until(&mut rig, "the checker to be resident", || {
            presenter.stats().checker_resident
        });
        image_completion.store(1, Ordering::SeqCst);
        rig.pump_for(Duration::from_millis(400));
        assert!(
            presenter.take_report().is_none(),
            "nothing runs at the gate"
        );
        assert_eq!(scene.probes.observer.frame_steps(), 0);
        gate.open(1);
        let mut reports = Vec::new();
        assert!(
            reports_until(&mut rig, &presenter, &mut reports, 1),
            "clipped {clipped}: no report"
        );
        rig.pump_for(Duration::from_millis(200));
        while let Some(report) = presenter.take_report() {
            reports.push(report);
        }
        assert_eq!(failures(&reports), 1, "clipped {clipped}: {reports:?}");
        assert_eq!(presenter.stats().compat_upload_failures, 1);
        assert_eq!(
            scene.probes.observer.frame_steps(),
            0,
            "no frame was built: the failure was classified by the pump before any frame source call"
        );
        assert!(
            scene
                .driver()
                .upload_failed
                .iter()
                .any(|(key, ..)| *key == 1),
            "the driver still sees the ordinary failure"
        );
        assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    }
    // Cleared, or replaced, before the late failure: no Failed; the status error remains.
    for replaced in [false, true] {
        let mut rig = Rig::open();
        let scene = Scene::new();
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        offer(&scene, 1, RED);
        offer(&scene, 2, BLUE);
        publish(&scene, 1, Some((1, rect(0, 0, 32, 32))), Vec::new());
        let hooks = TextureTestHooks {
            block_worker: Some(Arc::clone(&gate)),
            force_estimate_at: Some((1, TOO_BIG)),
            ..TextureTestHooks::default()
        };
        let mut presenter = scene.start(&rig, limits(), Some(hooks), capturing);
        until(&mut rig, "the first attempt to be held", || {
            presenter.stats().gate_holds >= 1
        });
        // The compatibility changes before the failure can arrive.
        if replaced {
            publish(&scene, 2, Some((2, rect(0, 0, 32, 32))), Vec::new());
        } else {
            publish(&scene, 2, None, Vec::new());
        }
        until(&mut rig, "the frame after the change", || {
            presenter.drawn_generation() == Some(2) || replaced
        });
        *lock(&gate.0) = true;
        gate.1.notify_all();
        until(&mut rig, "the late failure to reach the driver", || {
            scene
                .driver()
                .upload_failed
                .iter()
                .any(|(key, ..)| *key == 1)
        });
        rig.pump_for(Duration::from_millis(300));
        let mut reports = Vec::new();
        while let Some(report) = presenter.take_report() {
            reports.push(report);
        }
        assert_eq!(
            failures(&reports),
            0,
            "replaced {replaced}: a key no longer marked is only an ordinary UploadFailed: {reports:?}"
        );
        assert_eq!(presenter.stats().compat_upload_failures, 0);
        assert_eq!(end(&mut rig, &mut presenter), CleanupResult::Proven);
    }
}

pub(crate) const CASES: [(&str, fn()); 11] = [
    (
        "scene_compat_gate_holds_the_first_frame_until_the_image_resolves",
        scene_compat_gate_holds_the_first_frame_until_the_image_resolves,
    ),
    (
        "scene_compat_a_publication_before_the_wait_is_not_lost",
        scene_compat_a_publication_before_the_wait_is_not_lost,
    ),
    (
        "scene_compat_stop_while_the_gate_holds_ends_proven",
        scene_compat_stop_while_the_gate_holds_ends_proven,
    ),
    (
        "scene_compat_blocked_state_waits_and_is_not_busy",
        scene_compat_blocked_state_waits_and_is_not_busy,
    ),
    (
        "scene_compat_committed_behind_older_commits_is_adopted_in_bounded_boundaries",
        scene_compat_committed_behind_older_commits_is_adopted_in_bounded_boundaries,
    ),
    (
        "scene_compat_never_gates_new_asynchronous_resources",
        scene_compat_never_gates_new_asynchronous_resources,
    ),
    (
        "scene_compat_keeps_the_old_image_while_a_replacement_is_pending",
        scene_compat_keeps_the_old_image_while_a_replacement_is_pending,
    ),
    (
        "scene_compat_never_goes_back_to_an_older_image",
        scene_compat_never_goes_back_to_an_older_image,
    ),
    (
        "scene_compat_clear_releases_the_hold",
        scene_compat_clear_releases_the_hold,
    ),
    (
        "scene_compat_clipped_replacement_keeps_the_hold",
        scene_compat_clipped_replacement_keeps_the_hold,
    ),
    (
        "scene_compat_upload_failure_is_one_failed_and_never_a_hang",
        scene_compat_upload_failure_is_one_failed_and_never_a_hang,
    ),
];

pub(crate) const MORE: [(&str, fn()); 1] = [(
    "scene_compat_failure_is_classified_before_any_frame_was_built",
    scene_compat_failure_is_classified_before_any_frame_was_built,
)];

/// The compat cases again, on the bindless path (CK-128): the same case run with `Scene::new` giving the scene a table, so the
/// compatibility quad's gate, its held image and its failures are checked through the slot resolvers (`DrawSlot`) and the
/// epoch pin, which is what the production path draws. Each is its original case with the flag set; the negative controls are
/// the originals'.
pub(crate) fn scene_bindless_compat_gate_holds_the_first_frame_until_the_image_resolves() {
    BINDLESS_DEFAULT.store(true, Ordering::SeqCst);
    scene_compat_gate_holds_the_first_frame_until_the_image_resolves();
}

pub(crate) fn scene_bindless_compat_stop_while_the_gate_holds_ends_proven() {
    BINDLESS_DEFAULT.store(true, Ordering::SeqCst);
    scene_compat_stop_while_the_gate_holds_ends_proven();
}

pub(crate) fn scene_bindless_compat_blocked_state_waits_and_is_not_busy() {
    BINDLESS_DEFAULT.store(true, Ordering::SeqCst);
    scene_compat_blocked_state_waits_and_is_not_busy();
}

pub(crate) fn scene_bindless_compat_a_publication_before_the_wait_is_not_lost() {
    BINDLESS_DEFAULT.store(true, Ordering::SeqCst);
    scene_compat_a_publication_before_the_wait_is_not_lost();
}

pub(crate) fn scene_bindless_compat_committed_behind_older_commits_is_adopted_in_bounded_boundaries()
 {
    BINDLESS_DEFAULT.store(true, Ordering::SeqCst);
    scene_compat_committed_behind_older_commits_is_adopted_in_bounded_boundaries();
}

pub(crate) fn scene_bindless_compat_never_gates_new_asynchronous_resources() {
    BINDLESS_DEFAULT.store(true, Ordering::SeqCst);
    scene_compat_never_gates_new_asynchronous_resources();
}

pub(crate) fn scene_bindless_compat_keeps_the_old_image_while_a_replacement_is_pending() {
    BINDLESS_DEFAULT.store(true, Ordering::SeqCst);
    scene_compat_keeps_the_old_image_while_a_replacement_is_pending();
}

pub(crate) fn scene_bindless_compat_never_goes_back_to_an_older_image() {
    BINDLESS_DEFAULT.store(true, Ordering::SeqCst);
    scene_compat_never_goes_back_to_an_older_image();
}

pub(crate) fn scene_bindless_compat_clear_releases_the_hold() {
    BINDLESS_DEFAULT.store(true, Ordering::SeqCst);
    scene_compat_clear_releases_the_hold();
}

pub(crate) fn scene_bindless_compat_clipped_replacement_keeps_the_hold() {
    BINDLESS_DEFAULT.store(true, Ordering::SeqCst);
    scene_compat_clipped_replacement_keeps_the_hold();
}

pub(crate) fn scene_bindless_compat_upload_failure_is_one_failed_and_never_a_hang() {
    BINDLESS_DEFAULT.store(true, Ordering::SeqCst);
    scene_compat_upload_failure_is_one_failed_and_never_a_hang();
}

pub(crate) fn scene_bindless_compat_failure_is_classified_before_any_frame_was_built() {
    BINDLESS_DEFAULT.store(true, Ordering::SeqCst);
    scene_compat_failure_is_classified_before_any_frame_was_built();
}

pub(crate) const BINDLESS: [(&str, fn()); 12] = [
    (
        "scene_bindless_compat_gate_holds_the_first_frame_until_the_image_resolves",
        scene_bindless_compat_gate_holds_the_first_frame_until_the_image_resolves,
    ),
    (
        "scene_bindless_compat_stop_while_the_gate_holds_ends_proven",
        scene_bindless_compat_stop_while_the_gate_holds_ends_proven,
    ),
    (
        "scene_bindless_compat_blocked_state_waits_and_is_not_busy",
        scene_bindless_compat_blocked_state_waits_and_is_not_busy,
    ),
    (
        "scene_bindless_compat_a_publication_before_the_wait_is_not_lost",
        scene_bindless_compat_a_publication_before_the_wait_is_not_lost,
    ),
    (
        "scene_bindless_compat_committed_behind_older_commits_is_adopted_in_bounded_boundaries",
        scene_bindless_compat_committed_behind_older_commits_is_adopted_in_bounded_boundaries,
    ),
    (
        "scene_bindless_compat_never_gates_new_asynchronous_resources",
        scene_bindless_compat_never_gates_new_asynchronous_resources,
    ),
    (
        "scene_bindless_compat_keeps_the_old_image_while_a_replacement_is_pending",
        scene_bindless_compat_keeps_the_old_image_while_a_replacement_is_pending,
    ),
    (
        "scene_bindless_compat_never_goes_back_to_an_older_image",
        scene_bindless_compat_never_goes_back_to_an_older_image,
    ),
    (
        "scene_bindless_compat_clear_releases_the_hold",
        scene_bindless_compat_clear_releases_the_hold,
    ),
    (
        "scene_bindless_compat_clipped_replacement_keeps_the_hold",
        scene_bindless_compat_clipped_replacement_keeps_the_hold,
    ),
    (
        "scene_bindless_compat_upload_failure_is_one_failed_and_never_a_hang",
        scene_bindless_compat_upload_failure_is_one_failed_and_never_a_hang,
    ),
    (
        "scene_bindless_compat_failure_is_classified_before_any_frame_was_built",
        scene_bindless_compat_failure_is_classified_before_any_frame_was_built,
    ),
];
