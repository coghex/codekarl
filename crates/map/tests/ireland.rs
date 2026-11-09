//! Ireland's shipped map data (`assets/map/ireland/`, milestone 6, slice 5) loads and validates with `ck-map`'s own loader, so the data
//! cannot drift out of the format whoever edits its images and tables by hand.

use std::fs;
use std::path::Path;

use ck_map::{ScenarioStart, Surface, TerrainKind, cell_of, load_map, load_scenario, project};

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

#[test]
fn ireland_fertility_climate_glaciers_and_owner_pins_match_ck234() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/map/ireland");
    let map = load_map(&directory).unwrap_or_else(|error| panic!("{error}"));
    let land = map
        .data
        .cells
        .iter()
        .enumerate()
        .filter(|(_, cell)| matches!(cell, Surface::Land { .. }))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert!(!land.is_empty());
    let fertile = land
        .iter()
        .filter(|index| map.data.fertility[**index] > 0)
        .count();
    let display_fertile = land
        .iter()
        .filter(|index| map.data.fertility[**index] >= map.data.display_usable_pm)
        .count();
    assert!(
        fertile * 100 >= land.len() * 95,
        "{fertile}/{} land cells have fertility",
        land.len()
    );
    assert!(
        (30..=55).contains(&(display_fertile * 100 / land.len())),
        "{display_fertile}/{} cells meet the display threshold",
        land.len()
    );
    assert!(
        !map.data.glacier.iter().any(|value| *value),
        "Ireland should have no glacier cells"
    );

    let cell_index = |latitude: f64, longitude: f64| {
        let cell = cell_of(project(latitude, longitude).expect("place projects"));
        let x =
            u32::try_from(cell.x - map.data.crop.origin.x).expect("place is east of crop origin");
        let y =
            u32::try_from(cell.y - map.data.crop.origin.y).expect("place is south of crop origin");
        assert!(
            x < map.data.crop.width && y < map.data.crop.height,
            "place is inside the crop"
        );
        usize::try_from(y * map.data.crop.width + x).expect("index fits")
    };
    for (name, latitude, longitude) in [("Emly", 52.465, -8.349), ("Tara", 53.578, -6.611)] {
        let index = cell_index(latitude, longitude);
        assert!(
            map.data.fertility[index] >= 500,
            "{name} fertility is {}‰",
            map.data.fertility[index]
        );
    }
    for (name, latitude, longitude, minimum, maximum) in [
        ("Valentia", 51.94, -10.24, 100, 118),
        ("Dublin", 53.35, -6.26, 85, 105),
    ] {
        let index = cell_index(latitude, longitude);
        let temperature = map.data.climate_temperature_tenths[index];
        assert!(
            (minimum..=maximum).contains(&temperature),
            "{name} mean temperature is {temperature} tenths °C"
        );
    }
    let dublin = cell_index(53.35, -6.26);
    assert!(map.data.climate_rainfall_mm[dublin] < 900);

    let northwest = cell_of(project(55.0, -10.5).expect("northwest projects"));
    let southeast = cell_of(project(52.9, -8.5).expect("southeast projects"));
    let mut wet_upland = false;
    for global_y in northwest.y..=southeast.y {
        for global_x in northwest.x..=southeast.x {
            let x = u32::try_from(global_x - map.data.crop.origin.x).expect("in crop");
            let y = u32::try_from(global_y - map.data.crop.origin.y).expect("in crop");
            let index = usize::try_from(y * map.data.crop.width + x).expect("index fits");
            if matches!(map.data.cells[index], Surface::Land { .. })
                && map.data.climate_rainfall_mm[index] > 1800
            {
                wet_upland = true;
                break;
            }
        }
        if wet_upland {
            break;
        }
    }
    assert!(
        wet_upland,
        "Mayo/Galway uplands need a cell above 1,800 mm annual rainfall"
    );
    let edits =
        fs::read_to_string(directory.join("edits.toml")).expect("read signed terrain edits");
    assert!(edits.contains("glendalough-laragh-avonmore-pasture"));
    assert!(edits.contains("rahan-clodiagh-dry-ground-pasture"));
}
