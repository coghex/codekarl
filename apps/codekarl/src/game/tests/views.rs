//! The economy view the core publishes (milestone 9, slice 3): when it is built, the revision it carries and what a load does.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::{Harness, World};
use crate::game::controls::Control;
use crate::game::view::{EconomyView, PlaceView};

/// The builder, and the folds it was asked to build from.
type Recording = (
    Box<dyn Fn(&World) -> EconomyView + Send>,
    Arc<Mutex<Vec<u64>>>,
);

/// A view builder that puts the state's hourly fold in a place's `workers`, and notes each fold it was asked to build from.
fn recording_views() -> Recording {
    let built = Arc::new(Mutex::new(Vec::new()));
    let noting = Arc::clone(&built);
    let views = Box::new(move |world: &World| {
        noting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(world.hourly);
        EconomyView {
            places: vec![PlaceView {
                id: "fixture".to_owned(),
                workers: world.hourly,
                ..PlaceView::default()
            }],
        }
    });
    (views, built)
}

fn folds(built: &Mutex<Vec<u64>>) -> Vec<u64> {
    built.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

fn workers_of(snapshot: &crate::game::slot::SimSnapshot) -> u64 {
    snapshot
        .economy
        .as_ref()
        .expect("the snapshot carries a view")
        .place("fixture")
        .expect("the place is in the view")
        .workers
}

#[test]
fn the_first_snapshot_carries_the_starting_view_as_revision_one() {
    let (views, built) = recording_views();
    let harness = Harness::with_views(42, views);
    let published = harness.published();
    assert_eq!(published.len(), 1, "one snapshot at the start");
    assert_eq!(published[0].economy_revision, 1);
    assert_eq!(
        workers_of(&published[0]),
        0,
        "built from the starting state"
    );
    assert_eq!(folds(&built), [0]);
}

#[test]
fn a_core_without_a_view_builder_publishes_revision_zero_and_no_view() {
    let harness = Harness::new(42);
    let snapshot = harness.published().remove(0);
    assert_eq!((snapshot.economy_revision, snapshot.economy), (0, None));
}

#[test]
fn the_view_is_rebuilt_after_each_noon_tick_and_at_no_other_hour() {
    let (views, built) = recording_views();
    let mut harness = Harness::with_views(42, views);
    harness.run_at(1);
    // Hours 0 to 11 of the first day run: no noon yet, so still the starting view.
    for _ in 0..12 {
        harness.wake(Duration::from_secs(1));
    }
    assert_eq!(harness.core.snapshot().economy_revision, 1);
    assert_eq!(folds(&built).len(), 1);
    // The noon tick runs: all of the day's systems have run, and the view is built from what they left.
    harness.wake(Duration::from_secs(1));
    let noon = harness.core.snapshot();
    assert_eq!(noon.date.hour, 13, "the hour that runs next");
    assert_eq!(noon.economy_revision, 2);
    assert_eq!(folds(&built).len(), 2);
    assert_ne!(
        workers_of(&noon),
        0,
        "the hourly fold has moved since the start"
    );
    assert_eq!(workers_of(&noon), folds(&built)[1]);
    // The rest of the day builds nothing, and the next noon builds the third.
    for _ in 0..11 {
        harness.wake(Duration::from_secs(1));
    }
    assert_eq!(harness.core.snapshot().economy_revision, 2);
    for _ in 0..12 {
        harness.wake(Duration::from_secs(1));
    }
    assert_eq!(
        harness.core.snapshot().economy_revision,
        2,
        "hours 0 to 11 build nothing"
    );
    harness.wake(Duration::from_secs(1));
    assert_eq!(harness.core.snapshot().economy_revision, 3);
    // Every revision published rose by one and was published with its view, whatever the hours in between published.
    let mut last = 0;
    for snapshot in harness.published() {
        assert!(snapshot.economy_revision >= last);
        assert!(
            snapshot.economy_revision <= last + 1,
            "revisions never skip"
        );
        last = snapshot.economy_revision;
        assert!(snapshot.economy.is_some());
    }
    assert_eq!(last, 3);
}

#[test]
fn a_load_builds_a_new_revision_from_the_loaded_state_and_revisions_never_go_back() {
    let (views, built) = recording_views();
    let mut harness = Harness::with_views(42, views);
    harness.run_at(1);
    // Past the first noon, so the saved state is not the starting one.
    for _ in 0..13 {
        harness.wake(Duration::from_secs(1));
    }
    assert_eq!(harness.core.snapshot().economy_revision, 2);
    let saved_fold = folds(&built)[1];
    harness.control(Control::Quicksave);
    // Later days run and move the revision on; then the load replaces the state with the saved one.
    for _ in 0..(24 * 2) {
        harness.wake(Duration::from_secs(1));
    }
    let before_load = harness.core.snapshot();
    assert!(before_load.economy_revision >= 4);
    harness.control(Control::Quickload);
    let loaded = harness.core.snapshot();
    assert_eq!(
        loaded.economy_revision,
        before_load.economy_revision + 1,
        "a load is a new view under the next revision"
    );
    // The loaded state is the one saved straight after the first noon tick, which the second view was built from: the load's view
    // is newer and equal in content to it.
    assert_eq!(workers_of(&loaded), saved_fold);
    assert_ne!(workers_of(&loaded), workers_of(&before_load));
    assert_eq!(
        harness.published().last().map(|s| s.economy_revision),
        Some(loaded.economy_revision),
        "the load's view is published"
    );
}
