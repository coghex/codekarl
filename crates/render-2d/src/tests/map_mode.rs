//! The map modes' lane (milestone 9, slice 8), windowless: the owner grid's and the mode table's validation, each refusal by name, and
//! the bridge's rules for publishing a mode (a refused mode changes nothing, a stale revision never overwrites a newer one, a table is
//! carried only while it fits the map's places, and off is the frame without a mode).

use std::sync::Arc;

use super::map::{attached, camera, grid, input, rules};
use crate::map::{
    MODE_UNCOLOURED, MODE_UNOWNED, MODE_VALUE_MAX, MapError, MapInput, MapMode, MapPublication,
};

/// A `width x height` input at `generation` with an owner grid whose cells are `owners` (row-major) over `places` places.
fn owned(
    generation: u64,
    width: u32,
    height: u32,
    owners: Vec<u16>,
    places: usize,
) -> Arc<MapInput> {
    let plain = MapInput::new(
        generation,
        [100, 200],
        [width, height],
        grid(width, height, [1, 1, 0, 255]),
        grid(width, height, [42, 24, 0, 255]),
        rules(3),
    )
    .expect("a valid input");
    Arc::new(
        plain
            .with_owners(owners, places)
            .expect("a valid owner grid"),
    )
}

/// A 4 x 2 map over three places: places 0, 1 and 2 own two cells each, and the last two are nobody's.
fn three_places(generation: u64) -> Arc<MapInput> {
    owned(
        generation,
        4,
        2,
        vec![0, 0, 1, 1, 2, 2, MODE_UNOWNED, MODE_UNOWNED],
        3,
    )
}

fn plain(width: u32, height: u32) -> MapInput {
    MapInput::new(
        1,
        [0, 0],
        [width, height],
        grid(width, height, [1, 1, 0, 255]),
        grid(width, height, [0, 0, 0, 255]),
        rules(3),
    )
    .expect("a valid input")
}

#[test]
fn the_owner_grid_must_be_the_crops_size() {
    for length in [0, 5, 7, 9] {
        assert_eq!(
            plain(4, 2)
                .with_owners(vec![0; length], 1)
                .expect_err("refused"),
            MapError::OwnerGridLength {
                expected: 8,
                actual: length
            }
        );
    }
    assert!(plain(4, 2).with_owners(vec![0; 8], 1).is_ok());
}

#[test]
fn each_owner_index_is_below_the_place_count_or_the_unowned_marker() {
    let mut owners = vec![0, 1, 2, 0, 1, 2, 0, MODE_UNOWNED];
    assert!(plain(4, 2).with_owners(owners.clone(), 3).is_ok());
    // The first bad cell is named by its column and row; an index equal to the count is already one too many.
    owners[5] = 3;
    owners[6] = 9;
    assert_eq!(
        plain(4, 2)
            .with_owners(owners.clone(), 3)
            .expect_err("refused"),
        MapError::OwnerIndex {
            x: 1,
            y: 1,
            index: 3,
            places: 3
        }
    );
    // The same grid is fine once there are enough places for it, and any index is refused with none.
    assert!(plain(4, 2).with_owners(owners, 10).is_ok());
    assert_eq!(
        plain(2, 1)
            .with_owners(vec![0, MODE_UNOWNED], 0)
            .expect_err("refused"),
        MapError::OwnerIndex {
            x: 0,
            y: 0,
            index: 0,
            places: 0
        }
    );
    assert!(plain(2, 1).with_owners(vec![MODE_UNOWNED; 2], 0).is_ok());
}

#[test]
fn the_place_count_never_meets_the_unowned_marker() {
    let largest = usize::from(MODE_UNOWNED) - 1;
    assert!(plain(1, 1).with_owners(vec![MODE_UNOWNED], largest).is_ok());
    for places in [largest + 1, largest + 2, usize::MAX] {
        assert_eq!(
            plain(1, 1)
                .with_owners(vec![MODE_UNOWNED], places)
                .expect_err("refused"),
            MapError::OwnerPlaces { places }
        );
    }
    // The place count is checked before the grid is read.
    assert_eq!(
        plain(1, 1)
            .with_owners(Vec::new(), usize::MAX)
            .expect_err("refused"),
        MapError::OwnerPlaces { places: usize::MAX }
    );
}

#[test]
fn the_owner_grid_is_packed_for_the_gpu_as_high_and_low_bytes() {
    let input = plain(3, 1)
        .with_owners(vec![0x0102, 0, MODE_UNOWNED], 0x0200)
        .expect("valid");
    let owners = input.owners().expect("an owner grid");
    assert_eq!(owners.places(), 0x0200);
    assert_eq!(&**owners.indices(), [0x0102, 0, MODE_UNOWNED]);
    assert_eq!(
        &**owners.image(),
        [1, 2, 0, 255, 0, 0, 0, 255, 255, 255, 0, 255],
        "R the high byte, G the low byte, B zero, A 255"
    );
    assert!(
        plain(3, 1).owners().is_none(),
        "a map has no owner grid until it is given one"
    );
}

#[test]
fn a_mode_value_is_on_the_ramp_or_uncoloured() {
    assert!(MapMode::new(1, vec![0, 500, MODE_VALUE_MAX, MODE_UNCOLOURED]).is_ok());
    assert!(MapMode::new(1, Vec::new()).is_ok());
    // The first bad value is named by its place.
    for bad in [MODE_VALUE_MAX + 1, 1001, 40_000, 0xFFFE] {
        assert_eq!(
            MapMode::new(1, vec![0, 1000, bad, 0xFFFF]).expect_err("refused"),
            MapError::ModeValue {
                place: 2,
                value: bad
            }
        );
    }
    let off = MapMode::off(4);
    assert_eq!((off.revision(), off.values().is_none()), (4, true));
}

#[test]
fn a_table_that_is_not_the_place_count_is_refused_and_changes_nothing() {
    let (bridge, sink) = attached();
    bridge.publish_map(three_places(1));
    bridge.publish_camera(camera([2, 2], bridge.metrics_generation()));
    let good = Arc::new(MapMode::new(1, vec![0, 500, 1000]).expect("values"));
    assert_eq!(
        bridge.publish_mode(Arc::clone(&good)),
        Ok(MapPublication::Published)
    );
    let before = sink.count();
    for length in [0, 1, 2, 4, 40] {
        let wrong = Arc::new(MapMode::new(2, vec![7; length]).expect("values"));
        assert_eq!(
            bridge.publish_mode(wrong),
            Err(MapError::ModeTableLength {
                expected: 3,
                actual: length
            })
        );
    }
    assert_eq!(sink.count(), before, "a refusal publishes nothing");
    let mode = sink
        .last()
        .expect("published")
        .map
        .expect("a map")
        .mode
        .expect("a mode");
    assert!(
        Arc::ptr_eq(&mode, &good),
        "the frame keeps the previous accepted mode"
    );
}

#[test]
fn a_table_for_a_map_with_no_owner_grid_is_refused() {
    let (bridge, sink) = attached();
    bridge.publish_map(input(1, 4, 4));
    bridge.publish_camera(camera([2, 2], bridge.metrics_generation()));
    let before = sink.count();
    assert_eq!(
        bridge.publish_mode(Arc::new(MapMode::new(1, vec![1, 2]).expect("values"))),
        Err(MapError::ModeWithoutOwners)
    );
    assert_eq!(sink.count(), before);
    // Off needs no owner grid: it is the frame without a mode.
    assert_eq!(
        bridge.publish_mode(Arc::new(MapMode::off(1))),
        Ok(MapPublication::Published)
    );
    assert!(
        sink.last()
            .expect("published")
            .map
            .expect("a map")
            .mode
            .is_none()
    );
}

#[test]
fn a_mode_is_published_with_a_revision_and_a_stale_one_never_overwrites_a_newer_one() {
    let (bridge, sink) = attached();
    bridge.publish_map(three_places(1));
    bridge.publish_camera(camera([2, 2], bridge.metrics_generation()));
    assert!(
        sink.last()
            .expect("published")
            .map
            .expect("a map")
            .mode
            .is_none(),
        "no mode until one is published"
    );
    let first = Arc::new(MapMode::new(5, vec![0, 500, 1000]).expect("values"));
    assert_eq!(
        bridge.publish_mode(Arc::clone(&first)),
        Ok(MapPublication::Published)
    );
    let before = sink.count();
    for revision in [5, 4, 0] {
        let stale = Arc::new(MapMode::new(revision, vec![1, 1, 1]).expect("values"));
        assert_eq!(
            bridge.publish_mode(Arc::clone(&stale)),
            Ok(MapPublication::Stale),
            "revision {revision}"
        );
        assert_eq!(
            Arc::strong_count(&stale),
            1,
            "the refused mode was dropped by the call"
        );
    }
    assert_eq!(sink.count(), before, "a stale mode publishes nothing");
    let newer = Arc::new(MapMode::new(6, vec![9, 9, 9]).expect("values"));
    assert_eq!(
        bridge.publish_mode(Arc::clone(&newer)),
        Ok(MapPublication::Published)
    );
    let mode = sink
        .last()
        .expect("published")
        .map
        .expect("a map")
        .mode
        .expect("a mode");
    assert_eq!(
        (mode.revision(), mode.values().map(|v| v.to_vec())),
        (6, Some(vec![9, 9, 9]))
    );
    // Off is a newer revision with no table: the next frame has no mode.
    assert_eq!(
        bridge.publish_mode(Arc::new(MapMode::off(7))),
        Ok(MapPublication::Published)
    );
    assert!(
        sink.last()
            .expect("published")
            .map
            .expect("a map")
            .mode
            .is_none()
    );
}

#[test]
fn a_table_is_carried_only_while_it_fits_the_maps_places() {
    let (bridge, sink) = attached();
    // The table comes first (no map yet): it is held, and the map that arrives is shown with it when it fits.
    let table = Arc::new(MapMode::new(1, vec![0, 500, 1000]).expect("values"));
    assert_eq!(
        bridge.publish_mode(Arc::clone(&table)),
        Ok(MapPublication::Published)
    );
    bridge.publish_map(three_places(1));
    bridge.publish_camera(camera([2, 2], bridge.metrics_generation()));
    let mode = sink
        .last()
        .expect("published")
        .map
        .expect("a map")
        .mode
        .expect("a mode");
    assert!(Arc::ptr_eq(&mode, &table));
    // A newer map with other places leaves the older table unused, rather than reading past it.
    bridge.publish_map(owned(2, 4, 2, vec![0; 8], 1));
    let map = sink.last().expect("published").map.expect("a map");
    assert!(map.mode.is_none());
    // A table for the new map's places is carried again.
    let fitting = Arc::new(MapMode::new(2, vec![250]).expect("values"));
    assert_eq!(
        bridge.publish_mode(Arc::clone(&fitting)),
        Ok(MapPublication::Published)
    );
    assert!(Arc::ptr_eq(
        &sink
            .last()
            .expect("published")
            .map
            .expect("a map")
            .mode
            .expect("a mode"),
        &fitting
    ));
}

#[test]
fn nothing_is_published_for_a_mode_to_a_detached_bridge() {
    let (bridge, sink) = attached();
    bridge.publish_map(three_places(1));
    bridge.publish_camera(camera([2, 2], bridge.metrics_generation()));
    bridge
        .publish_mode(Arc::new(MapMode::new(1, vec![0, 0, 0]).expect("values")))
        .expect("accepted");
    bridge.detach();
    let after = sink.count();
    let late = Arc::new(MapMode::new(9, vec![0, 0, 0]).expect("values"));
    assert_eq!(
        bridge.publish_mode(Arc::clone(&late)),
        Ok(MapPublication::Detached)
    );
    assert_eq!(sink.count(), after);
    assert_eq!(Arc::strong_count(&late), 1, "and keeps nothing");
}
