//! The scene bridge. Every race is controlled by a gate or a barrier inside a sink, never by a delay: a sink
//! is called with the bridge lock held, so blocking in it holds the lock at an exact point, and what the other
//! thread then does cannot be reordered.
//!
//! There is one writer per bridge, so a test takes it once and moves it to the thread that writes.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Barrier};
use std::thread::{self, JoinHandle};

use super::support::{
    EagerSink, Gate, LatestSink, LiveImages, PATIENCE, RecordingSink, desc, image, lock_held_by,
    pixels, quad_at,
};
use crate::budget::MAX_RENDERER_IMAGES;
use crate::{Filter, Phase, QuadSink, SceneBridge, SceneWriter, WriteCounts, WriterTaken};

fn bridge() -> Arc<SceneBridge> {
    SceneBridge::new(Filter::Nearest)
}

fn take_writer(bridge: &Arc<SceneBridge>) -> SceneWriter {
    bridge.take_writer().unwrap()
}

fn attach(bridge: &SceneBridge, sink: &Arc<RecordingSink>) {
    let sink: Arc<dyn QuadSink> = Arc::clone(sink) as Arc<dyn QuadSink>;
    bridge
        .attach(&desc(100, 100), pixels(100, 100), sink)
        .unwrap();
}

/// Runs `body` with the writer on another thread, and hands the writer back when it is joined.
fn on_thread(
    writer: SceneWriter,
    body: impl FnOnce(&SceneWriter) + Send + 'static,
) -> JoinHandle<SceneWriter> {
    thread::spawn(move || {
        body(&writer);
        writer
    })
}

/// A sink that stops in `publish` for generation `at`: it says it is inside, then waits for the gate.
fn stopping_at(at: u64) -> (Arc<RecordingSink>, mpsc::Receiver<()>, Arc<Gate>) {
    let (inside, entered) = mpsc::channel();
    let gate = Arc::new(Gate::default());
    let waiting = Arc::clone(&gate);
    let sink = RecordingSink::with_hook(move |snapshot| {
        if snapshot.generation == at {
            inside.send(()).unwrap();
            waiting.wait();
        }
    });
    (sink, entered, gate)
}

fn within(entered: &mpsc::Receiver<()>) {
    match entered.recv_timeout(PATIENCE) {
        Ok(()) => {}
        Err(RecvTimeoutError::Timeout) => panic!("the sink was never entered"),
        Err(RecvTimeoutError::Disconnected) => panic!("the sink's thread ended"),
    }
}

fn moves<T: Send>() {}

// A type is `Sync` (or `Clone`) exactly when the second impl applies as well as the first, which makes the call
// to `check` ambiguous and the test fail to build. (The doctests of `SceneWriter` show the same as compile
// failures.)
trait AmbiguousIfSync<A> {
    fn check() {}
}
impl<T> AmbiguousIfSync<()> for T {}
impl<T: Sync> AmbiguousIfSync<u8> for T {}

trait AmbiguousIfClone<A> {
    fn check() {}
}
impl<T> AmbiguousIfClone<()> for T {}
impl<T: Clone> AmbiguousIfClone<u8> for T {}

#[test]
fn there_is_exactly_one_writer_and_it_can_move_between_threads_but_not_be_shared() {
    let bridge = bridge();
    let first = bridge.take_writer().unwrap();
    assert_eq!(bridge.take_writer().unwrap_err(), WriterTaken);
    assert_eq!(
        bridge.take_writer().unwrap_err(),
        WriterTaken,
        "every later take"
    );
    assert!(WriterTaken.to_string().contains("already taken"));
    // Dropping the writer does not return it: the bridge's one producer is gone for good.
    drop(first);
    assert!(bridge.take_writer().is_err());

    // Send: it moves to the writing thread. Neither Sync nor Clone: these do not build otherwise.
    moves::<SceneWriter>();
    <SceneWriter as AmbiguousIfSync<_>>::check();
    <SceneWriter as AmbiguousIfClone<_>>::check();
}

#[test]
fn a_write_before_attach_is_in_the_first_published_snapshot() {
    let (bridge, picture) = (bridge(), image(20, 20));
    take_writer(&bridge).set_quad(quad_at(&picture, 10.0, 10.0, 20.0, 20.0));
    assert_eq!(bridge.phase(), Phase::Before);
    let sink = RecordingSink::new();
    attach(&bridge, &sink);
    assert_eq!(bridge.phase(), Phase::Attached);
    let first = sink.snapshots();
    assert_eq!(first.len(), 1);
    let placed = first[0].quad.as_ref().unwrap();
    assert!(Arc::ptr_eq(&placed.image, &picture));
    assert_eq!(
        (placed.destination.left, placed.destination.right),
        (10, 30)
    );
    assert_eq!(first[0].filter, Filter::Nearest);
    // Without a quad, the first snapshot is "no quad".
    let (empty, sink) = (self::bridge(), RecordingSink::new());
    attach(&empty, &sink);
    assert!(sink.last().unwrap().quad.is_none());
}

#[test]
fn the_filter_is_carried_in_every_snapshot() {
    let (bridge, sink) = (SceneBridge::new(Filter::Linear), RecordingSink::new());
    attach(&bridge, &sink);
    take_writer(&bridge).set_quad(quad_at(&image(4, 4), 0.0, 0.0, 4.0, 4.0));
    assert!(
        sink.snapshots()
            .iter()
            .all(|snapshot| snapshot.filter == Filter::Linear)
    );
}

#[test]
fn attach_refuses_to_run_twice_or_after_the_end() {
    let (bridge, sink) = (bridge(), RecordingSink::new());
    attach(&bridge, &sink);
    let again: Arc<dyn QuadSink> = RecordingSink::new();
    assert_eq!(
        bridge
            .attach(&desc(10, 10), pixels(10, 10), again)
            .unwrap_err()
            .phase,
        Phase::Attached
    );
    bridge.detach();
    let late: Arc<dyn QuadSink> = RecordingSink::new();
    assert_eq!(
        bridge
            .attach(&desc(10, 10), pixels(10, 10), late)
            .unwrap_err()
            .phase,
        Phase::Ended
    );
}

#[test]
fn every_change_publishes_in_generation_order() {
    let (bridge, sink) = (bridge(), RecordingSink::new());
    let (writer, picture) = (take_writer(&bridge), image(10, 10));
    attach(&bridge, &sink);
    writer.set_quad(quad_at(&picture, 0.0, 0.0, 10.0, 10.0));
    bridge.set_pixel_size(pixels(200, 200));
    writer.set_quad(quad_at(&picture, 5.0, 5.0, 10.0, 10.0));
    writer.clear_quad();
    assert_eq!(sink.generations(), vec![1, 2, 3, 4, 5]);
    assert!(sink.last().unwrap().quad.is_none());
}

#[test]
fn clearing_with_no_quad_changes_nothing_and_is_counted() {
    let (bridge, sink) = (bridge(), RecordingSink::new());
    attach(&bridge, &sink);
    take_writer(&bridge).clear_quad();
    assert_eq!(sink.generations(), vec![1], "nothing published");
    assert_eq!(
        bridge.counts(),
        WriteCounts {
            accepted: 1,
            discarded: 0,
            ..WriteCounts::default()
        }
    );
}

#[test]
fn attach_racing_a_write_strands_nothing_when_the_attach_is_in_progress() {
    // Forced order: the attach is inside its publication, holding the lock, when the write starts.
    let (bridge, picture) = (bridge(), image(20, 20));
    let (sink, entered, gate) = stopping_at(1);
    // Taken first: taking the writer needs the lock the attach is about to hold.
    let writer = take_writer(&bridge);
    let attacher = {
        let (bridge, sink) = (Arc::clone(&bridge), Arc::clone(&sink));
        thread::spawn(move || attach(&bridge, &sink))
    };
    within(&entered);
    let writing = on_thread(writer, move |writer| {
        writer.set_quad(quad_at(&picture, 0.0, 0.0, 20.0, 20.0));
    });
    gate.open();
    attacher.join().unwrap();
    writing.join().unwrap();
    assert_eq!(sink.generations(), vec![1, 2]);
    assert!(
        sink.snapshots()[0].quad.is_none(),
        "the attach published the scene it found"
    );
    assert!(
        sink.last().unwrap().quad.is_some(),
        "the write followed and was not stranded"
    );
}

#[test]
fn attach_racing_a_write_strands_nothing_in_either_order() {
    // Free race, started together by a barrier. Whichever wins, the last snapshot has the write.
    for _ in 0..300 {
        let (bridge, picture, sink) = (bridge(), image(20, 20), RecordingSink::new());
        let start = Arc::new(Barrier::new(3));
        let attacher = {
            let (bridge, sink, start) =
                (Arc::clone(&bridge), Arc::clone(&sink), Arc::clone(&start));
            thread::spawn(move || {
                start.wait();
                attach(&bridge, &sink);
            })
        };
        let writing = {
            let start = Arc::clone(&start);
            on_thread(take_writer(&bridge), move |writer| {
                start.wait();
                writer.set_quad(quad_at(&picture, 0.0, 0.0, 20.0, 20.0));
            })
        };
        start.wait();
        attacher.join().unwrap();
        writing.join().unwrap();
        let generations = sink.generations();
        assert!(
            generations.windows(2).all(|pair| pair[0] < pair[1]),
            "{generations:?}"
        );
        assert!(sink.last().unwrap().quad.is_some(), "{generations:?}");
    }
}

#[test]
fn a_metrics_change_racing_a_write_publishes_last_with_the_current_metrics() {
    // The write is inside its publication when the metrics change starts: the change follows, converting with
    // the new pixel size, and is the last thing published.
    let (bridge, picture) = (bridge(), image(10, 10));
    let (sink, entered, gate) = stopping_at(2);
    attach(&bridge, &sink);
    let writing = on_thread(take_writer(&bridge), move |writer| {
        writer.set_quad(quad_at(&picture, 10.0, 10.0, 10.0, 10.0));
    });
    within(&entered);
    let resizer = {
        let bridge = Arc::clone(&bridge);
        thread::spawn(move || bridge.set_pixel_size(pixels(200, 200)))
    };
    gate.open();
    writing.join().unwrap();
    resizer.join().unwrap();
    let snapshots = sink.snapshots();
    assert_eq!(sink.generations(), vec![1, 2, 3]);
    assert_eq!(
        snapshots[1].quad.as_ref().unwrap().destination.left,
        10,
        "the write, at the old size"
    );
    let last = snapshots[2].quad.as_ref().unwrap();
    assert_eq!(
        (last.destination.left, last.destination.right),
        (20, 40),
        "the change, at the new size"
    );
}

#[test]
fn a_write_racing_a_metrics_change_converts_with_the_new_metrics() {
    // The other order: the metrics change is inside its publication when the write starts.
    let (bridge, picture) = (bridge(), image(10, 10));
    let (sink, entered, gate) = stopping_at(2);
    let writer = take_writer(&bridge);
    attach(&bridge, &sink);
    let resizer = {
        let bridge = Arc::clone(&bridge);
        thread::spawn(move || bridge.set_pixel_size(pixels(200, 200)))
    };
    within(&entered);
    let writing = on_thread(writer, move |writer| {
        writer.set_quad(quad_at(&picture, 10.0, 10.0, 10.0, 10.0));
    });
    gate.open();
    resizer.join().unwrap();
    writing.join().unwrap();
    assert_eq!(sink.generations(), vec![1, 2, 3]);
    let last = sink.last().unwrap().quad.unwrap();
    assert_eq!(
        (last.destination.left, last.destination.right),
        (20, 40),
        "the write, at the new size"
    );
}

#[test]
fn a_metrics_change_and_a_write_started_together_end_with_the_current_metrics() {
    for _ in 0..300 {
        let (bridge, picture, sink) = (bridge(), image(10, 10), RecordingSink::new());
        attach(&bridge, &sink);
        let start = Arc::new(Barrier::new(3));
        let writing = {
            let start = Arc::clone(&start);
            on_thread(take_writer(&bridge), move |writer| {
                start.wait();
                writer.set_quad(quad_at(&picture, 10.0, 10.0, 10.0, 10.0));
            })
        };
        let resizer = {
            let (bridge, start) = (Arc::clone(&bridge), Arc::clone(&start));
            thread::spawn(move || {
                start.wait();
                bridge.set_pixel_size(pixels(200, 200));
            })
        };
        start.wait();
        writing.join().unwrap();
        resizer.join().unwrap();
        let last = sink.last().unwrap().quad.unwrap();
        assert_eq!(
            (last.destination.left, last.destination.right),
            (20, 40),
            "{:?}",
            sink.generations()
        );
        let generations = sink.generations();
        assert!(
            generations.windows(2).all(|pair| pair[0] < pair[1]),
            "{generations:?}"
        );
    }
}

#[test]
fn detach_racing_a_write_publishes_nothing_after_detach_and_the_write_is_counted() {
    // The write is inside its publication when the detach starts: it completes, the detach follows, and
    // everything after is discarded.
    let (bridge, picture) = (bridge(), image(10, 10));
    let (sink, entered, gate) = stopping_at(2);
    attach(&bridge, &sink);
    let again = Arc::clone(&picture);
    let writing = on_thread(take_writer(&bridge), move |writer| {
        writer.set_quad(quad_at(&picture, 0.0, 0.0, 10.0, 10.0));
    });
    within(&entered);
    let detacher = {
        let bridge = Arc::clone(&bridge);
        thread::spawn(move || bridge.detach())
    };
    gate.open();
    let writer = writing.join().unwrap();
    detacher.join().unwrap();
    assert_eq!(sink.generations(), vec![1, 2]);
    assert_eq!(bridge.phase(), Phase::Ended);
    let published = sink.count();
    writer.set_quad(quad_at(&again, 1.0, 1.0, 10.0, 10.0));
    writer.clear_quad();
    bridge.set_pixel_size(pixels(300, 300));
    assert_eq!(
        sink.count(),
        published,
        "nothing is published after the detach"
    );
    assert_eq!(
        bridge.counts(),
        WriteCounts {
            accepted: 3,
            discarded: 2,
            ..WriteCounts::default()
        }
    );
}

#[test]
fn detach_and_a_stream_of_writes_started_together_publish_nothing_after_detach_returns() {
    for _ in 0..200 {
        let (bridge, picture, sink) = (bridge(), image(10, 10), RecordingSink::new());
        attach(&bridge, &sink);
        let start = Arc::new(Barrier::new(2));
        let writing = {
            let start = Arc::clone(&start);
            on_thread(take_writer(&bridge), move |writer| {
                start.wait();
                for step in 0..200 {
                    writer.set_quad(quad_at(&picture, f64::from(step), 0.0, 10.0, 10.0));
                }
            })
        };
        start.wait();
        bridge.detach();
        let at_detach = sink.count();
        writing.join().unwrap();
        assert_eq!(
            sink.count(),
            at_detach,
            "a snapshot was published after detach returned"
        );
        let counts = bridge.counts();
        assert_eq!(counts.accepted, 200);
        assert_eq!(
            counts.discarded,
            200 - (at_detach as u64 - 1),
            "every write is either published or discarded"
        );
    }
}

#[test]
fn a_write_after_the_end_is_discarded_counted_and_changes_nothing() {
    let (bridge, sink, picture) = (bridge(), RecordingSink::new(), image(10, 10));
    let writer = take_writer(&bridge);
    attach(&bridge, &sink);
    writer.set_quad(quad_at(&picture, 0.0, 0.0, 10.0, 10.0));
    bridge.detach();
    bridge.detach();
    assert_eq!(bridge.phase(), Phase::Ended, "detach is idempotent");
    let published = sink.count();
    writer.set_quad(quad_at(&picture, 50.0, 50.0, 10.0, 10.0));
    writer.clear_quad();
    assert_eq!(sink.count(), published);
    assert_eq!(
        bridge.counts(),
        WriteCounts {
            accepted: 3,
            discarded: 2,
            ..WriteCounts::default()
        }
    );
}

#[test]
fn with_a_stalled_consumer_the_producer_keeps_making_progress() {
    // The sink's consumer never takes a value. Each write must still return: a hang is a failure, found by the
    // timeout and not by an assertion about time.
    let (bridge, sink, picture) = (bridge(), LatestSink::new(), image(10, 10));
    bridge
        .attach(
            &desc(100, 100),
            pixels(100, 100),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .unwrap();
    let (done, finished) = mpsc::channel();
    let producer = on_thread(take_writer(&bridge), move |writer| {
        for step in 0..5000 {
            writer.set_quad(quad_at(&picture, f64::from(step % 90), 0.0, 10.0, 10.0));
        }
        done.send(()).unwrap();
    });
    finished
        .recv_timeout(PATIENCE)
        .expect("the producer waited for the consumer");
    producer.join().unwrap();
    assert_eq!(
        sink.latest().unwrap().generation,
        5001,
        "the sink holds the latest of 5000 writes and the attach"
    );
    assert_eq!(sink.history().len(), 5001);
}

#[test]
fn an_image_is_never_dropped_while_the_bridge_lock_is_held() {
    // Single threaded, so a lock found held by a drop can only be held by this thread.
    let (bridge, live, sink) = (bridge(), LiveImages::default(), LatestSink::new());
    bridge
        .attach(
            &desc(100, 100),
            pixels(100, 100),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .unwrap();
    let (writer, held) = (take_writer(&bridge), lock_held_by(&bridge));
    // A scene replaced twice, then cleared: every displaced quad and snapshot is dropped after the lock.
    for _ in 0..3 {
        let picture = live.make(10, 10, held.clone());
        writer.set_quad(quad_at(&picture, 0.0, 0.0, 10.0, 10.0));
    }
    writer.clear_quad();
    // A write after the end: the quad it brings is dropped after the lock too.
    bridge.detach();
    let late = live.make(10, 10, held.clone());
    writer.set_quad(quad_at(&late, 0.0, 0.0, 10.0, 10.0));
    drop(late);
    // A sink that detach removes, still holding a snapshot of the last image (a recording sink keeps every
    // snapshot): the sink goes after the lock does, taking the image with it.
    let (second, second_sink) = (self::bridge(), RecordingSink::new());
    attach(&second, &second_sink);
    let second_writer = take_writer(&second);
    let image_in_sink = live.make(10, 10, lock_held_by(&second));
    second_writer.set_quad(quad_at(&image_in_sink, 0.0, 0.0, 10.0, 10.0));
    drop(image_in_sink);
    second_writer.clear_quad();
    assert_eq!(live.live(), 1, "only the recording sink still has it");
    drop(second_sink);
    second.detach();
    let probes = live.probes();
    assert_eq!(probes.len(), 5, "every image was dropped: {probes:?}");
    assert!(
        probes.iter().all(|held| !held),
        "an image was dropped under the lock: {probes:?}"
    );
    assert_eq!(live.live(), 0);
}

#[test]
fn the_bridges_own_references_are_dropped_after_the_lock_even_when_the_sink_keeps_none() {
    // The eager sink drops the snapshot it replaces inside `publish`, so the bridge's quad is the last
    // reference to each image: when the bridge drops it, it is the bridge that frees the image.
    let (bridge, live) = (bridge(), LiveImages::default());
    bridge
        .attach(
            &desc(100, 100),
            pixels(100, 100),
            EagerSink::new() as Arc<dyn QuadSink>,
        )
        .unwrap();
    let (writer, held) = (take_writer(&bridge), lock_held_by(&bridge));
    for _ in 0..3 {
        let picture = live.make(10, 10, held.clone());
        writer.set_quad(quad_at(&picture, 0.0, 0.0, 10.0, 10.0));
    }
    writer.clear_quad();
    assert_eq!(live.probes(), vec![false; 3], "all three, outside the lock");
    assert_eq!(live.live(), 0, "and before the write returned");
}

#[test]
fn the_probe_sees_a_drop_made_while_the_lock_is_held() {
    // The control: the same probe, with an image dropped inside the lock on purpose.
    let (bridge, live) = (bridge(), LiveImages::default());
    let picture = live.make(10, 10, lock_held_by(&bridge));
    bridge.with_lock_held(|| drop(picture));
    assert_eq!(live.probes(), vec![true]);
}

#[test]
fn what_a_write_displaces_is_dropped_before_the_write_returns() {
    // One image in the scene and the sink's slot; the render thread holds none. A replacing write leaves
    // exactly the new image alive when it returns: the old quad and the snapshot the sink handed back are gone.
    let (bridge, live, sink) = (bridge(), LiveImages::default(), LatestSink::new());
    bridge
        .attach(
            &desc(100, 100),
            pixels(100, 100),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .unwrap();
    let (writer, held) = (take_writer(&bridge), lock_held_by(&bridge));
    for step in 0..10 {
        let picture = live.make(10, 10, held.clone());
        writer.set_quad(quad_at(&picture, f64::from(step), 0.0, 10.0, 10.0));
        drop(picture);
        assert_eq!(live.live(), 1, "write {step}");
    }
    bridge.set_pixel_size(pixels(200, 200));
    assert_eq!(live.live(), 1, "a metrics update displaces a snapshot too");
}

#[test]
fn replacing_the_scene_while_the_sink_holds_and_a_frame_is_mid_upload_keeps_at_most_four_images() {
    let (bridge, live, sink) = (bridge(), LiveImages::default(), LatestSink::new());
    bridge
        .attach(
            &desc(100, 100),
            pixels(100, 100),
            Arc::clone(&sink) as Arc<dyn QuadSink>,
        )
        .unwrap();
    let (writer, held) = (take_writer(&bridge), lock_held_by(&bridge));
    let mut most = 0;
    let mut uploading = None;
    for step in 0..200 {
        // The writer's own image, in hand while it is "decoded" and before it is written.
        let picture = live.make(10, 10, held.clone());
        most = most.max(live.live());
        writer.set_quad(quad_at(&picture, f64::from(step % 80), 0.0, 10.0, 10.0));
        most = most.max(live.live());
        drop(picture);
        if step % 50 == 0 {
            // The render thread takes the latest snapshot and holds it while it uploads: stalled.
            uploading = sink.take();
        }
        most = most.max(live.live());
    }
    assert!(
        most <= MAX_RENDERER_IMAGES + 1,
        "{most} decoded images alive at once"
    );
    assert!(uploading.is_some());
    drop(uploading);
    writer.clear_quad();
    sink.take();
    assert_eq!(live.live(), 0);
}

#[test]
fn a_displaced_image_is_dropped_outside_the_lock_when_the_sink_returns_and_when_it_panics() {
    // The consumer has already uploaded and released its snapshot of the old image, so the bridge's own quad is
    // the last reference to it. Replacing (or clearing) the scene displaces it: with a sink that returns, and
    // with one that panics mid-publication, it must be freed after the lock is released, not while the guard is
    // still alive during the unwind.
    for panics in [false, true] {
        for clearing in [false, true] {
            let (bridge, live, fault) = (
                bridge(),
                LiveImages::default(),
                Arc::new(AtomicBool::new(false)),
            );
            let sink = LatestSink::failing(&fault);
            bridge
                .attach(
                    &desc(100, 100),
                    pixels(100, 100),
                    Arc::clone(&sink) as Arc<dyn QuadSink>,
                )
                .unwrap();
            let (writer, held) = (take_writer(&bridge), lock_held_by(&bridge));
            let old = live.make(10, 10, held.clone());
            writer.set_quad(quad_at(&old, 0.0, 0.0, 10.0, 10.0));
            drop(old);
            drop(sink.take()); // the render thread drew it and released its reference
            fault.store(panics, Ordering::SeqCst);
            let new = live.make(10, 10, held.clone());
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                if clearing {
                    writer.clear_quad();
                } else {
                    writer.set_quad(quad_at(&new, 0.0, 0.0, 10.0, 10.0));
                }
            }));
            let case = format!("panics {panics}, clearing {clearing}");
            assert_eq!(outcome.is_err(), panics, "{case}");
            assert_eq!(bridge.is_poisoned(), panics, "{case}");
            // The old image is gone, and went outside the lock.
            assert_eq!(live.probes(), vec![false], "{case}");
            assert_eq!(
                live.live(),
                1,
                "{case}: only the new image remains (in hand, or the scene's)"
            );
            drop(new);
        }
    }
}

#[test]
fn a_poisoned_lock_does_not_stop_the_bridge() {
    let bridge = bridge();
    let armed = Arc::new(AtomicBool::new(false));
    let panics = {
        let armed = Arc::clone(&armed);
        RecordingSink::with_hook(move |_| {
            assert!(!armed.load(Ordering::SeqCst), "a sink that panics");
        })
    };
    bridge
        .attach(
            &desc(100, 100),
            pixels(100, 100),
            panics as Arc<dyn QuadSink>,
        )
        .unwrap();
    armed.store(true, Ordering::SeqCst);
    let picture = image(10, 10);
    let writing = on_thread(take_writer(&bridge), move |writer| {
        writer.set_quad(quad_at(&picture, 0.0, 0.0, 10.0, 10.0));
    });
    assert!(writing.join().is_err());
    assert!(bridge.is_poisoned());
    armed.store(false, Ordering::SeqCst);
    // The bridge still answers, and ends. (The writer died with its thread; the bridge does not need it.)
    bridge.set_pixel_size(pixels(200, 200));
    bridge.detach();
    assert_eq!(bridge.phase(), Phase::Ended);
}
