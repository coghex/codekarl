//! Ireland's shipped map data (`assets/map/ireland/`, milestone 6, slice 5) loads and validates with `ck-map`'s own loader, so the data
//! cannot drift out of the format whoever edits its images and tables by hand.

use std::path::Path;

use ck_map::{ScenarioStart, TerrainKind, load_map, load_scenario};

#[test]
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "independently reproduce the specified f64 cosine scale table for its pinned hash"
)]
fn the_shipped_map_of_ireland_loads_and_validates() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/map/ireland");
    let map = load_map(&directory).unwrap_or_else(|error| panic!("{error}"));

    // The owner's terrain types are all there, as the kinds the renderer reads.
    let names = [
        "sea", "lake", "pasture", "forest", "bog", "marsh", "mountain",
    ];
    assert_eq!(map.terrain_types.len(), names.len());
    for kind in [
        TerrainKind::Sea,
        TerrainKind::Lake,
        TerrainKind::Land,
        TerrainKind::Mountain,
    ] {
        assert!(
            map.terrain_types.iter().any(|terrain| terrain.kind == kind),
            "no terrain of kind {kind:?}"
        );
    }
    // It is a map of land and sea, with rivers, and its grids are the crop's.
    let cells = usize::try_from(map.data.crop.width * map.data.crop.height).expect("fits");
    assert_eq!(map.grid_a.pixels.len(), cells);
    assert_eq!(map.grid_b.pixels.len(), cells);
    let land = map
        .grid_a
        .pixels
        .iter()
        .filter(|pixel| {
            map.terrain_types[usize::from(pixel[0])]
                .style_index
                .is_some()
        })
        .count();
    assert!(
        land > cells / 10 && land < cells / 2,
        "{land} land cells of {cells}"
    );
    assert!(!map.rivers.is_empty());

    // Independently pin the f64 cosine scale table for Ireland's crop.
    let crop = map.data.crop;
    let mut hash = 14_695_981_039_346_656_037_u64;
    for row in 0..crop.height {
        let global_y = i64::from(crop.origin.y) + i64::from(row);
        let y_metres = (global_y as f64 + 0.5) * 839.0;
        let radius = 6_378_137.0_f64;
        let half_world = std::f64::consts::PI * radius;
        let latitude =
            2.0 * ((half_world - y_metres) / radius).exp().atan() - std::f64::consts::FRAC_PI_2;
        let scale = (latitude.cos() * 65_536.0).round().clamp(1.0, 65_536.0) as u32;
        for byte in scale.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(1_099_511_628_211);
        }
    }
    assert_eq!(hash, 1_751_616_413_225_014_021);
}

#[test]
fn ireland_768_scenario_resolves_its_map_and_loads_all_locations() {
    let directory =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenarios/ireland-768");
    let scenario = load_scenario(&directory).unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(scenario.id, "ireland_768");
    assert_eq!(
        scenario.start,
        ScenarioStart {
            year: 768,
            month: 10,
            day: 9,
            hour: 0,
        }
    );
    assert_eq!(scenario.locations.len(), 94);
    assert_eq!(scenario.location_kinds.kinds.len(), 5);
    for id in ["iniscealtra", "inisfallen"] {
        assert!(
            scenario.locations.iter().any(|location| location.id == id),
            "missing island monastery {id}"
        );
    }
    assert!(!scenario.map.rivers.is_empty());
}
