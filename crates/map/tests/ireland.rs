//! Ireland's shipped map data (`assets/map/ireland/`, milestone 6, slice 5) loads and validates with `ck-map`'s own loader, so the data
//! cannot drift out of the format whoever edits its images and tables by hand.

use std::path::Path;

use ck_map::{TerrainKind, load_map};

#[test]
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
}
