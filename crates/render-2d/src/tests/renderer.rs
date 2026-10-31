//! The renderer over a fake backend.

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ck_window::{CleanupResult, Presenter, PresenterReport};

use super::fake::{Event, FakeGpu, Mode};
use super::support::{
    LiveImages, PATIENCE, desc, image, lock, lock_held_by, pixels, quad_at, wait_until,
};
use crate::budget::MAX_RENDERER_IMAGES;
use crate::{Filter, Phase, Renderer, SceneBridge, SceneWriter, WriteCounts};

fn renderer(mode: Mode) -> (Renderer<FakeGpu>, Arc<SceneBridge>, SceneWriter) {
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    (
        Renderer::new(FakeGpu::new(mode, &bridge), Arc::clone(&bridge)),
        bridge,
        writer,
    )
}

/// Stops the renderer and waits for its backend to report the end.
fn finish(renderer: &mut Renderer<FakeGpu>) {
    renderer.request_stop();
    wait_until("the end", || renderer.poll_end(Duration::ZERO).is_some());
}

#[test]
fn the_sink_exists_and_holds_the_initial_scene_before_the_backends_thread_could_draw() {
    for (mode, drew_quad) in [(Mode::DrawsQuad, true), (Mode::ClearOnly, false)] {
        let (mut renderer, _bridge, writer) = renderer(mode);
        let picture = image(20, 20);
        writer.set_quad(quad_at(&picture, 10.0, 10.0, 20.0, 20.0));
        renderer
            .start((), &desc(100, 100), pixels(100, 100))
            .unwrap();
        let state = Arc::clone(&renderer.gpu().state);
        // `start` found the sink made and already holding the scene, before it spawned anything. The write was
        // generation 1, the attach 2.
        assert_eq!(
            state.events(),
            vec![
                Event::NewSink,
                Event::Start {
                    held: Some(2),
                    quad: true,
                    size: pixels(100, 100)
                }
            ]
        );
        // The thread draws once released, and its first frame has the scene written before the window opened.
        assert!(state.frames().is_empty());
        state.go.open();
        finish(&mut renderer);
        let frames = state.frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(
            (frames[0].generation, frames[0].drew_quad),
            (2, drew_quad),
            "{mode:?}"
        );
        assert_eq!(
            frames[0].destination.map(|rect| (rect.left, rect.right)),
            Some((10, 30))
        );
        assert_eq!(
            renderer.take_report(),
            Some(PresenterReport::FirstFrameQueued)
        );
    }
}

#[test]
fn a_clear_only_backend_and_a_drawing_one_see_the_same_snapshots() {
    let picture = image(8, 8);
    let mut seen = Vec::new();
    for mode in [Mode::DrawsQuad, Mode::ClearOnly] {
        let (mut renderer, _bridge, writer) = renderer(mode);
        renderer
            .start((), &desc(100, 100), pixels(100, 100))
            .unwrap();
        writer.set_quad(quad_at(&picture, 4.0, 4.0, 8.0, 8.0));
        renderer.set_pixel_size(200, 200);
        let state = Arc::clone(&renderer.gpu().state);
        state.go.open();
        finish(&mut renderer);
        seen.push((state.sink_history(), state.frames()));
    }
    // Both were given the same snapshots, in the same order...
    assert_eq!(seen[0].0, seen[1].0);
    assert_eq!(seen[0].0, vec![(1, false), (2, true), (3, true)]);
    // ...and only the drawing one draws the quad.
    assert_eq!(seen[0].1.len(), 1);
    assert_eq!(seen[1].1.len(), 1);
    assert_eq!(seen[0].1[0].generation, seen[1].1[0].generation);
    assert_eq!(seen[0].1[0].destination, seen[1].1[0].destination);
    assert!(seen[0].1[0].drew_quad);
    assert!(!seen[1].1[0].drew_quad);
}

#[test]
fn a_start_failure_detaches_the_bridge() {
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    let mut renderer = Renderer::new(FakeGpu::failing("no device", &bridge), Arc::clone(&bridge));
    let error = renderer
        .start((), &desc(100, 100), pixels(100, 100))
        .unwrap_err();
    assert_eq!(error, "no device");
    assert_eq!(bridge.phase(), Phase::Ended);
    let state = &renderer.gpu().state;
    let published = state.sink().latest().unwrap().generation;
    writer.set_quad(quad_at(&image(4, 4), 0.0, 0.0, 4.0, 4.0));
    assert_eq!(
        state.sink().latest().unwrap().generation,
        published,
        "nothing is published after the failure"
    );
    assert_eq!(
        bridge.counts(),
        WriteCounts {
            accepted: 1,
            discarded: 1,
            ..WriteCounts::default()
        }
    );
}

#[test]
fn the_first_stop_request_detaches_before_forwarding_and_later_ones_are_harmless() {
    let (mut renderer, bridge, writer) = renderer(Mode::DrawsQuad);
    renderer
        .start((), &desc(100, 100), pixels(100, 100))
        .unwrap();
    assert_eq!(bridge.phase(), Phase::Attached);
    renderer.request_stop();
    renderer.request_stop();
    let stops: Vec<Event> = renderer
        .gpu()
        .state
        .events()
        .into_iter()
        .filter(|event| matches!(event, Event::RequestStop { .. }))
        .collect();
    assert_eq!(
        stops,
        vec![
            Event::RequestStop {
                phase: Phase::Ended
            };
            2
        ],
        "the bridge had ended when the backend heard"
    );
    let published = renderer.gpu().state.sink().latest().unwrap().generation;
    writer.set_quad(quad_at(&image(4, 4), 0.0, 0.0, 4.0, 4.0));
    assert_eq!(
        renderer.gpu().state.sink().latest().unwrap().generation,
        published
    );
    assert_eq!(bridge.counts().discarded, 1);
}

#[test]
fn a_pixel_size_change_republishes_before_it_is_forwarded() {
    let (mut renderer, _bridge, writer) = renderer(Mode::DrawsQuad);
    writer.set_quad(quad_at(&image(10, 10), 10.0, 10.0, 10.0, 10.0));
    renderer
        .start((), &desc(100, 100), pixels(100, 100))
        .unwrap();
    let sink = renderer.gpu().state.sink();
    assert_eq!(sink.latest().unwrap().quad.unwrap().destination.left, 10);
    renderer.set_pixel_size(200, 200);
    let republished = sink.latest().unwrap();
    assert_eq!(republished.generation, 3);
    assert_eq!(republished.quad.unwrap().destination.left, 20);
    // When the backend heard of the change, the sink already held the new conversion.
    assert!(
        renderer
            .gpu()
            .state
            .events()
            .contains(&Event::SetPixelSize {
                width: 200,
                height: 200,
                held: Some(3)
            })
    );
}

#[test]
fn everything_else_is_forwarded_unchanged() {
    let (mut renderer, _bridge, _writer) = renderer(Mode::DrawsQuad);
    renderer
        .start((), &desc(100, 100), pixels(100, 100))
        .unwrap();
    renderer.set_clear([0.25, 0.5, 0.75]);
    renderer.request_redraw();
    lock(&renderer.gpu().state.reports).push_back(PresenterReport::Failed {
        message: "lost".to_owned(),
    });
    let failure = PresenterReport::Failed {
        message: "lost".to_owned(),
    };
    assert_eq!(renderer.take_report(), Some(failure));
    assert_eq!(renderer.take_report(), None);
    let state = Arc::clone(&renderer.gpu().state);
    assert_eq!(
        renderer.poll_end(Duration::from_millis(7)),
        None,
        "the end is the backend's to report"
    );
    state.go.open();
    renderer.request_stop();
    let end = loop {
        if let Some(end) = renderer.poll_end(Duration::from_millis(9)) {
            break end;
        }
        thread::yield_now();
    };
    assert_eq!(end, CleanupResult::Proven);
    let events = state.events();
    assert!(events.contains(&Event::SetClear([0.25, 0.5, 0.75])));
    assert!(events.contains(&Event::RequestRedraw));
    assert!(events.contains(&Event::PollEnd(Duration::from_millis(7))));
    assert!(events.contains(&Event::PollEnd(Duration::from_millis(9))));
    let pixel_sizes = events
        .iter()
        .filter(|event| matches!(event, Event::SetPixelSize { .. }))
        .count();
    assert_eq!(pixel_sizes, 0, "nothing the caller did not ask for");
}

#[test]
fn a_second_start_is_refused_without_starting_the_backend_again() {
    let (mut renderer, _bridge, _writer) = renderer(Mode::DrawsQuad);
    renderer
        .start((), &desc(100, 100), pixels(100, 100))
        .unwrap();
    let error = renderer
        .start((), &desc(100, 100), pixels(100, 100))
        .unwrap_err();
    assert!(error.contains("cannot attach"), "{error}");
    let starts = renderer
        .gpu()
        .state
        .events()
        .iter()
        .filter(|event| matches!(event, Event::Start { .. }))
        .count();
    assert_eq!(starts, 1);
}

#[test]
fn the_fake_records_each_published_snapshot_and_its_generation() {
    let (mut renderer, _bridge, writer) = renderer(Mode::DrawsQuad);
    let picture = image(10, 10);
    writer.set_quad(quad_at(&picture, 0.0, 0.0, 10.0, 10.0)); // generation 1, before the sink exists
    renderer
        .start((), &desc(100, 100), pixels(100, 100))
        .unwrap(); // 2
    writer.set_quad(quad_at(&picture, 5.0, 5.0, 10.0, 10.0)); // 3
    renderer.set_pixel_size(200, 200); // 4
    writer.clear_quad(); // 5
    writer.clear_quad(); // changes nothing, publishes nothing
    renderer.request_stop();
    writer.set_quad(quad_at(&picture, 0.0, 0.0, 10.0, 10.0)); // discarded
    assert_eq!(
        renderer.gpu().state.sink_history(),
        vec![(2, true), (3, true), (4, true), (5, false)]
    );
}

#[test]
fn a_stalled_render_thread_does_not_hold_the_producer_up() {
    let bridge = SceneBridge::new(Filter::Nearest);
    let mut renderer = Renderer::new(
        FakeGpu::stalled(Mode::DrawsQuad, &bridge),
        Arc::clone(&bridge),
    );
    renderer
        .start((), &desc(100, 100), pixels(100, 100))
        .unwrap();
    let state = Arc::clone(&renderer.gpu().state);
    // The thread draws its first frame and then stalls "uploading".
    state.go.open();
    wait_until("the first frame", || !state.frames().is_empty());
    let (done, finished) = mpsc::channel();
    let writer = bridge.take_writer().unwrap();
    let producer = thread::spawn(move || {
        let picture = image(10, 10);
        for step in 0..3000 {
            writer.set_quad(quad_at(&picture, f64::from(step % 80), 0.0, 10.0, 10.0));
        }
        done.send(()).unwrap();
    });
    finished
        .recv_timeout(PATIENCE)
        .expect("the producer waited for the stalled render thread");
    producer.join().unwrap();
    assert_eq!(
        state.sink().latest().unwrap().generation,
        3001,
        "the attach, then 3000 writes"
    );
    state.upload.open();
    finish(&mut renderer);
}

#[test]
fn replacing_the_scene_while_the_fake_uploads_keeps_at_most_four_images_and_frees_none_under_the_lock()
 {
    let bridge = SceneBridge::new(Filter::Nearest);
    let writer = bridge.take_writer().unwrap();
    let mut renderer = Renderer::new(
        FakeGpu::stalled(Mode::DrawsQuad, &bridge),
        Arc::clone(&bridge),
    );
    let (live, held) = (LiveImages::default(), lock_held_by(&bridge));
    let first = live.make(10, 10, held.clone());
    writer.set_quad(quad_at(&first, 0.0, 0.0, 10.0, 10.0));
    drop(first);
    renderer
        .start((), &desc(100, 100), pixels(100, 100))
        .unwrap();
    let state = Arc::clone(&renderer.gpu().state);
    // The render thread takes the snapshot and holds it while it uploads, stalled.
    state.go.open();
    wait_until("the first frame", || !state.frames().is_empty());
    let mut most = live.live();
    for step in 0..300 {
        // The writer's own image, in hand before it is written.
        let picture = live.make(10, 10, held.clone());
        most = most.max(live.live());
        writer.set_quad(quad_at(&picture, f64::from(step % 80), 0.0, 10.0, 10.0));
        most = most.max(live.live());
        drop(picture);
        // What the write displaced is gone when it returns: the scene's image and the uploading one remain.
        assert_eq!(live.live(), 2, "write {step}");
    }
    assert!(
        most <= MAX_RENDERER_IMAGES + 1,
        "{most} decoded images alive at once"
    );
    assert_eq!(most, 3, "the uploading one, the scene's and the writer's");
    state.upload.open();
    finish(&mut renderer);
    state.sink().take();
    drop(renderer);
    drop(writer);
    drop(bridge);
    assert_eq!(live.live(), 0);
    let probes = live.probes();
    assert_eq!(probes.len(), 301);
    assert!(
        probes.iter().all(|held| !held),
        "an image was dropped under the lock"
    );
}
