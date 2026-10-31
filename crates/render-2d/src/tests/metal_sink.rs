//! The Metal backend's sink and conversion, without a window: what a snapshot becomes for the GPU crate, what is
//! kept and handed back, and what happens to a quad the GPU crate refuses.

use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use ck_gpu_metal::MetalFilter;
use ck_window::{CleanupResult, PresenterReport};

use super::support::{LiveImages, image, pixels};
use crate::metal::{Conversion, MetalSink, Refusals};
use crate::{
    Filter, Gpu2d, MetalBackend, Metrics, PixelRect, PlacedQuad, QuadSink, Snapshot, SourceRect,
};

fn snapshot(
    generation: u64,
    filter: Filter,
    picture: Option<&Arc<crate::Image>>,
    destination: PixelRect,
    source: SourceRect,
) -> Snapshot {
    Snapshot {
        generation,
        filter,
        requested: picture.is_some(),
        quad: picture.map(|picture| PlacedQuad {
            image: Arc::clone(picture),
            destination,
            source,
        }),
        scene: None,
        metrics: Metrics {
            points_width: 8,
            points_height: 4,
            pixels: pixels(8, 4),
        },
    }
}

const WHOLE: PixelRect = PixelRect {
    left: 0,
    top: 0,
    right: 8,
    bottom: 4,
};

const ALL: SourceRect = SourceRect {
    left: 0.0,
    top: 0.0,
    right: 8.0,
    bottom: 4.0,
};

#[expect(
    unsafe_code,
    reason = "the constructor's contract is about the layer `start` is given, and this backend is never started"
)]
fn unsafe_backend(wake: Arc<dyn Fn() + Send + Sync>) -> MetalBackend {
    // SAFETY: this backend is never started, so it never touches a layer; the contract is about the layer that
    // `start` is given.
    unsafe { MetalBackend::new(wake) }
}

fn backend() -> MetalBackend {
    unsafe_backend(Arc::new(|| {}))
}

#[test]
fn a_snapshot_with_a_quad_becomes_the_scene_with_the_same_fields_and_generation() {
    let picture = image(8, 4);
    let destination = PixelRect {
        left: 10,
        top: 20,
        right: 30,
        bottom: 40,
    };
    let source = SourceRect {
        left: 1.0,
        top: 0.5,
        right: 7.0,
        bottom: 3.5,
    };
    for (filter, expected) in [
        (Filter::Nearest, MetalFilter::Nearest),
        (Filter::Linear, MetalFilter::Linear),
    ] {
        let mut conversion = Conversion::default();
        let step = conversion.next(&snapshot(7, filter, Some(&picture), destination, source));
        assert!(step.refusal.is_none());
        assert_eq!(step.scene.generation(), 7);
        let quad = step.scene.quad().expect("a quad");
        assert_eq!((quad.width(), quad.height()), (8, 4));
        assert_eq!(quad.filter(), expected);
        let (d, s) = (quad.destination(), quad.source());
        assert_eq!((d.left, d.top, d.right, d.bottom), (10, 20, 30, 40));
        assert_eq!((s.left, s.top, s.right, s.bottom), (1.0, 0.5, 7.0, 3.5));
    }
}

#[test]
fn no_quad_stays_no_quad_with_its_generation() {
    let mut conversion = Conversion::default();
    let step = conversion.next(&snapshot(9, Filter::Nearest, None, WHOLE, ALL));
    assert_eq!(step.scene.generation(), 9);
    assert!(step.scene.quad().is_none());
    assert!(step.refusal.is_none());
}

#[test]
fn the_pixel_source_shares_the_image_and_copies_nothing() {
    let picture = image(8, 4);
    let before = Arc::strong_count(&picture);
    let mut conversion = Conversion::default();
    let step = conversion.next(&snapshot(1, Filter::Nearest, Some(&picture), WHOLE, ALL));
    let quad = step.scene.quad().expect("a quad");
    assert_eq!(
        quad.pixels().pixels().as_ptr(),
        picture.pixels().as_ptr(),
        "the same bytes, not a copy"
    );
    // The scene's wrapper, and the snapshot kept to be handed back, are two more holders of the one image.
    assert!(Arc::strong_count(&picture) > before);
}

#[test]
fn one_image_is_one_pixel_source_however_often_the_quad_moves_and_a_new_image_is_a_new_one() {
    let (first, second) = (image(8, 4), image(8, 4));
    let mut conversion = Conversion::default();
    let a = conversion.next(&snapshot(1, Filter::Nearest, Some(&first), WHOLE, ALL));
    let moved = PixelRect {
        left: 2,
        top: 2,
        right: 12,
        bottom: 12,
    };
    let b = conversion.next(&snapshot(2, Filter::Linear, Some(&first), moved, ALL));
    let c = conversion.next(&snapshot(3, Filter::Linear, Some(&second), moved, ALL));
    let source_of =
        |step: &crate::metal::Step| Arc::as_ptr(step.scene.quad().expect("a quad").pixels());
    assert!(
        std::ptr::addr_eq(source_of(&a), source_of(&b)),
        "the GPU crate uploads by this identity: moving the quad must not change it"
    );
    assert!(!std::ptr::addr_eq(source_of(&b), source_of(&c)));
    // After a scene with no quad the wrapper is not kept alive.
    let none = conversion.next(&snapshot(4, Filter::Nearest, None, WHOLE, ALL));
    assert!(none.scene.quad().is_none());
    let again = conversion.next(&snapshot(5, Filter::Nearest, Some(&second), WHOLE, ALL));
    assert!(!std::ptr::addr_eq(source_of(&c), source_of(&again)));
}

#[test]
fn the_cache_holds_no_strong_reference_to_the_image_or_its_pixel_source() {
    let live = LiveImages::default();
    let picture = live.make(8, 4, || false);
    let mut conversion = Conversion::default();
    let step = conversion.next(&snapshot(1, Filter::Nearest, Some(&picture), WHOLE, ALL));
    drop(picture);
    assert_eq!(live.live(), 1, "the scene's pixel source keeps it alive");
    drop(step);
    assert_eq!(live.live(), 0, "the cache kept nothing");
    // A wrapper that is gone is not revived: the next snapshot of a new image makes a new one.
    let again = live.make(8, 4, || false);
    let step = conversion.next(&snapshot(2, Filter::Nearest, Some(&again), WHOLE, ALL));
    assert!(step.scene.quad().is_some());
}

#[test]
fn a_refused_quad_is_no_quad_with_its_generation_and_a_reason() {
    let picture = image(8, 4);
    let inside_out = PixelRect {
        left: 5,
        top: 0,
        right: 5,
        bottom: 4,
    };
    let mut conversion = Conversion::default();
    let step = conversion.next(&snapshot(
        11,
        Filter::Nearest,
        Some(&picture),
        inside_out,
        ALL,
    ));
    assert_eq!(step.scene.generation(), 11);
    assert!(step.scene.quad().is_none());
    let reason = step.refusal.expect("a refusal");
    assert!(reason.contains("refused"), "{reason}");
    // The next, valid snapshot is converted normally.
    let next = conversion.next(&snapshot(12, Filter::Nearest, Some(&picture), WHOLE, ALL));
    assert!(next.refusal.is_none());
    assert!(next.scene.quad().is_some());
}

#[test]
fn a_refusal_is_one_failed_report_and_never_a_panic_inside_publish() {
    let mut backend = backend();
    let sink = backend.new_sink();
    let picture = image(8, 4);
    let bad = PixelRect {
        left: 3,
        top: 3,
        right: 1,
        bottom: 1,
    };
    // `publish` runs under the bridge's lock: it returns normally, handing the snapshot back.
    assert!(
        sink.publish(snapshot(1, Filter::Nearest, Some(&picture), bad, ALL))
            .is_some()
    );
    assert!(
        sink.publish(snapshot(2, Filter::Nearest, Some(&picture), bad, ALL))
            .is_some()
    );
    match backend.take_report() {
        Some(PresenterReport::Failed { message }) => {
            assert!(message.contains("refused"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(backend.take_report(), None, "reported once");
    // A presenter that was never started has nothing to prove.
    assert_eq!(
        backend.poll_end(std::time::Duration::ZERO),
        Some(CleanupResult::Proven)
    );
    assert_eq!(backend.drawn_generation(), None);
}

#[test]
fn publish_hands_the_snapshot_back_so_nothing_is_freed_inside_it() {
    let live = LiveImages::default();
    let sink = MetalSink::for_test();
    let a = live.make(8, 4, || false);
    let b = live.make(8, 4, || false);
    let first = sink.publish(snapshot(1, Filter::Nearest, Some(&a), WHOLE, ALL));
    drop(a);
    let second = sink.publish(snapshot(2, Filter::Nearest, Some(&b), WHOLE, ALL));
    drop(b);
    assert_eq!(
        live.live(),
        2,
        "both snapshots came back, so neither image was freed inside publish"
    );
    drop(first);
    drop(second);
    assert_eq!(
        live.live(),
        1,
        "the slot's current scene keeps only the latest"
    );
}

#[test]
fn a_refusal_wakes_the_loop_once_per_refusal_and_a_good_snapshot_does_not() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let mut backend = unsafe_backend(Arc::new(move || {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    let sink = backend.new_sink();
    let picture = image(8, 4);
    let bad = PixelRect {
        left: 3,
        top: 3,
        right: 1,
        bottom: 1,
    };
    let _ = sink.publish(snapshot(1, Filter::Nearest, Some(&picture), WHOLE, ALL));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let _ = sink.publish(snapshot(2, Filter::Nearest, Some(&picture), bad, ALL));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn start_takes_only_the_sink_new_sink_made() {
    let mut backend = backend();
    assert!(
        !backend.made(&(MetalSink::for_test() as Arc<dyn QuadSink>)),
        "none made yet"
    );
    let first = backend.new_sink();
    assert!(backend.made(&first));
    let foreign = MetalSink::for_test() as Arc<dyn QuadSink>;
    assert!(!backend.made(&foreign));
    let second = backend.new_sink();
    assert!(backend.made(&second));
    assert!(
        !backend.made(&first),
        "a later new_sink replaces the earlier one"
    );
    // What `start` does with them: its own sink gives the slot, any other is refused by name.
    assert!(backend.accept(&second).is_ok());
    assert_eq!(
        backend.accept(&first).expect_err("a stale sink"),
        "the sink is not the one this backend made"
    );
    assert!(backend.accept(&foreign).is_err());
    let unstarted = self::backend();
    assert!(
        unstarted
            .accept(&(MetalSink::for_test() as Arc<dyn QuadSink>))
            .is_err(),
        "a backend that made no sink accepts none"
    );
    assert_eq!(pixels(1, 1).width, 1);
}

#[test]
fn the_refusals_keep_the_first_and_report_it_once() {
    let refusals = Refusals::default();
    assert_eq!(refusals.take(), None);
    refusals.record("first".to_owned());
    refusals.record("second".to_owned());
    assert_eq!(refusals.take().as_deref(), Some("first"));
    refusals.record("third".to_owned());
    assert_eq!(refusals.take(), None, "one report per run");
}

/// The longest the seam waits for the racing `record` to finish (it always does; the bound is only a guard against
/// a hang).
const RACE_BOUND: std::time::Duration = std::time::Duration::from_secs(10);

/// CK-94: a second refusal recorded from another thread **at the consumption boundary of `take`** (the point where
/// the accepted code had consumed the first reason and released its guard but not yet marked the refusal reported)
/// must be rejected: the run reports its first refusal once and nothing after it. The seam is a handshake, not a
/// timing: from inside the boundary the test starts the racing `record` on another thread and waits for it to
/// finish, then lets `take` go on. With the accepted code the racing record stores its reason and the next `take`
/// reports it; with consumption and "reported" one synchronized operation the record is rejected.
#[test]
fn a_record_racing_the_consumption_of_the_first_refusal_is_rejected_and_the_report_is_once() {
    let refusals = Arc::new(Refusals::default());
    refusals.record("first".to_owned());
    let racer: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));
    {
        let (refusals, racer) = (Arc::clone(&refusals), Arc::clone(&racer));
        refusals.clone().at_boundary(move || {
            let (finished, done) = mpsc::channel();
            let handle = thread::spawn(move || {
                refusals.record("second".to_owned());
                let _ = finished.send(());
            });
            done.recv_timeout(RACE_BOUND)
                .expect("the racing record finished");
            *racer.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
        });
    }
    let first = refusals.take();
    if let Some(handle) = racer.lock().unwrap_or_else(PoisonError::into_inner).take() {
        handle.join().expect("the racing record's thread ended");
    }
    assert_eq!(
        first.as_deref(),
        Some("first"),
        "the first reason is kept and returned"
    );
    for _ in 0..3 {
        assert_eq!(
            refusals.take(),
            None,
            "nothing is reported after the first, ever"
        );
    }
    refusals.record("third".to_owned());
    assert_eq!(refusals.take(), None, "and a later record is rejected too");
}
