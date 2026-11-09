//! The map modes' reference rule (`docs/economy-design.md` section 3.22): vectors for the blend, the colour ramp and the values.

use std::collections::{BTreeMap, BTreeSet};

use super::*;

const OFF: u16 = MODE_UNOWNED;

fn all_land(width: u32, height: u32) -> MapData {
    let count = (width * height) as usize;
    MapData {
        crop: Crop {
            origin: Cell { x: 10, y: 20 },
            width,
            height,
        },
        cells: vec![
            Surface::Land {
                terrain: 0,
                usable: true
            };
            count
        ],
        elevations: vec![11_004; count],
        fertility: vec![1000; count],
        climate_temperature_tenths: vec![100; count],
        climate_gdd: vec![1000; count],
        climate_rainfall_mm: vec![1000; count],
        glacier: vec![false; count],
        display_usable: vec![true; count],
        display_usable_pm: 500,
        terrains: vec![TerrainStyle {
            solid: Rgba8::opaque(1, 2, 3),
            second: Rgba8::opaque(4, 5, 6),
            dark: Rgba8::opaque(7, 8, 9),
            dark_second: Rgba8::opaque(10, 11, 12),
            cap: Rgba8::opaque(13, 14, 15),
            high_ground: Rgba8::opaque(16, 17, 18),
            relief_low: 0,
            relief_high: 16,
            mix_percent: 0,
            cap_low: 8,
            cap_high: 24,
            mountain: false,
        }],
        sea: Rgba8::opaque(20, 21, 22),
        shallow_sea: Rgba8::opaque(23, 24, 25),
        lake: Rgba8::opaque(26, 27, 28),
        river: Rgba8::opaque(29, 30, 31),
        shallow_threshold: 100,
    }
}

/// The value at crop cell `(x, y)` of a crop `width` wide.
fn value(width: u32, height: u32, owners: &[u16], values: &[u16], x: u32, y: u32) -> Option<u16> {
    let crop = Crop {
        origin: Cell { x: 0, y: 0 },
        width,
        height,
    };
    mode_value_at_cell(crop, &ModeInputs { owners, values }, x, y)
}

/// The step the pixel tests use: below the first jitter step, so a pixel's sample point is its centre.
const STEP: u8 = 13;

/// Every world pixel of `STEP` whose sample point is in the crop of `map`, with the local column and row of the cell it samples.
fn sampled(map: &MapData) -> Vec<(WorldPixel, u32, u32)> {
    let size = i64::from(STEP_SIZE[usize::from(STEP)]);
    let left = i64::from(map.crop.origin.x) * FIXED_PER_CELL / size - 1;
    let top = i64::from(map.crop.origin.y) * FIXED_PER_CELL / size - 1;
    let span = |cells: u32| i64::from(cells) * FIXED_PER_CELL / size + 3;
    let mut found = Vec::new();
    for j in top..top + span(map.crop.height) {
        for i in left..left + span(map.crop.width) {
            let pixel = WorldPixel {
                x: i32::try_from(i).unwrap(),
                y: i32::try_from(j).unwrap(),
            };
            let cell = cell_of(sample_point(pixel, STEP).unwrap());
            if map.crop.contains(cell) {
                let x = u32::try_from(cell.x - map.crop.origin.x).unwrap();
                let y = u32::try_from(cell.y - map.crop.origin.y).unwrap();
                found.push((pixel, x, y));
            }
        }
    }
    found
}

#[test]
fn the_colour_ramp_runs_red_yellow_green_in_integers() {
    assert_eq!(mode_colour(0), Rgba8::opaque(255, 0, 0));
    assert_eq!(mode_colour(500), Rgba8::opaque(255, 255, 0));
    assert_eq!(mode_colour(1000), Rgba8::opaque(0, 255, 0));
    assert_eq!(mode_colour(250), Rgba8::opaque(255, 127, 0));
    assert_eq!(mode_colour(750), Rgba8::opaque(127, 255, 0));
    assert_eq!(mode_colour(1), Rgba8::opaque(255, 0, 0), "floored");
    assert_eq!(mode_colour(501), Rgba8::opaque(254, 255, 0));
    assert_eq!(mode_colour(999), Rgba8::opaque(0, 255, 0));
    assert_eq!(
        mode_colour(1200),
        mode_colour(1000),
        "above the ramp is green"
    );
}

#[test]
fn the_blend_at_a_two_catchment_border_is_the_floored_mean_of_the_counted_cells() {
    // Eight columns by five rows: columns 0 to 3 belong to place 0 (value 0), columns 4 to 7 to place 1 (value 1000).
    let owners: Vec<u16> = (0..40).map(|cell| u16::from(cell % 8 >= 4)).collect();
    let values = [0, 1000];
    let at = |x, y| value(8, 5, &owners, &values, x, y);
    assert_eq!(at(0, 2), Some(0), "far from the border: place 0 alone");
    assert_eq!(at(7, 2), Some(1000), "far from the border: place 1 alone");
    assert_eq!(
        at(3, 2),
        Some(400),
        "10 of the 25 cells of the box are place 1"
    );
    assert_eq!(at(4, 2), Some(600), "15 of the 25");
    assert_eq!(at(2, 2), Some(200), "5 of the 25");
    assert_eq!(
        at(3, 0),
        Some(400),
        "a box that runs off the crop counts only the cells inside"
    );
    // The mean is floored: three cells of value 10 and two of 11 give 10.
    let owners = [0, 0, 0, 1, 1];
    assert_eq!(value(5, 1, &owners, &[10, 11], 2, 0), Some(10));
}

#[test]
fn an_isolated_owned_cell_colours_its_box_and_no_further() {
    let mut owners = vec![OFF; 81];
    owners[4 * 9 + 4] = 0;
    for y in 0..9_u32 {
        for x in 0..9_u32 {
            let expected = (x.abs_diff(4) <= 2 && y.abs_diff(4) <= 2).then_some(700);
            assert_eq!(value(9, 9, &owners, &[700], x, y), expected, "cell {x},{y}");
        }
    }
}

#[test]
fn a_place_with_no_capacity_is_not_counted_and_is_not_a_zero() {
    // Place 0 is uncoloured, place 1 is 500: a cell that sees both reads 500, not 250.
    let owners = [0, 0, 1, 1, 1];
    let values = [MODE_UNCOLOURED, 500];
    assert_eq!(value(5, 1, &owners, &values, 1, 0), Some(500));
    assert_eq!(
        value(5, 1, &owners, &values, 0, 0),
        Some(500),
        "place 1 starts two cells away"
    );
    // Every owner uncoloured: no value, the terrain colour stays.
    assert_eq!(
        value(5, 1, &owners, &[MODE_UNCOLOURED, MODE_UNCOLOURED], 2, 0),
        None
    );
    // Red (0) is a value, not the absence of one.
    assert_eq!(value(5, 1, &owners, &[0, 0], 2, 0), Some(0));
}

#[test]
fn the_stockpile_value_is_thousandths_of_capacity() {
    assert_eq!(stockpile_value(0, 100), 0);
    assert_eq!(stockpile_value(50, 100), 500);
    assert_eq!(stockpile_value(100, 100), 1000);
    assert_eq!(stockpile_value(1, 3), 333, "floored");
    assert_eq!(stockpile_value(150, 100), 1000, "never above full");
    assert_eq!(
        stockpile_value(0, 0),
        MODE_UNCOLOURED,
        "no capacity: uncoloured"
    );
    assert_eq!(stockpile_value(7, 0), MODE_UNCOLOURED);
    assert_eq!(stockpile_value(u64::MAX, u64::MAX), 1000);
}

#[test]
fn the_yield_value_is_relative_across_the_coloured_places() {
    let values = yield_values(&[Some(10), Some(30), None, Some(20), Some(10)]);
    assert_eq!(values, [0, 1000, MODE_UNCOLOURED, 500, 0]);
    // The minimum and maximum are of the coloured places only: the uncoloured one does not set them.
    assert_eq!(
        yield_values(&[Some(5), None, Some(8)]),
        [0, MODE_UNCOLOURED, 1000]
    );
    assert_eq!(yield_values(&[Some(0), Some(2), Some(1)]), [0, 1000, 500]);
    assert_eq!(
        yield_values(&[Some(0), Some(3), Some(1)]),
        [0, 1000, 333],
        "floored"
    );
}

#[test]
fn equal_minimum_and_maximum_yield_puts_every_place_at_the_middle() {
    assert_eq!(yield_values(&[Some(7), Some(7), Some(7)]), [500, 500, 500]);
    assert_eq!(yield_values(&[Some(0)]), [500]);
    assert_eq!(yield_values(&[Some(4), None]), [500, MODE_UNCOLOURED]);
    assert_eq!(
        yield_values(&[None, None]),
        [MODE_UNCOLOURED, MODE_UNCOLOURED]
    );
    assert_eq!(yield_values(&[]), Vec::<u16>::new());
}

#[test]
fn the_reference_colour_shows_the_blend_and_keeps_the_terrain_where_nothing_is_counted() {
    // Twenty-one by twenty-one cells, one owned cell in the middle with value 1000 (green).
    let map = all_land(21, 21);
    let mut owners = vec![OFF; 441];
    owners[10 * 21 + 10] = 0;
    let values = [1000_u16];
    let mode = ModeInputs {
        owners: &owners,
        values: &values,
    };
    let (no_rivers, no_locations) = (BTreeSet::new(), BTreeMap::new());
    let (mut shown, mut kept) = (0, 0);
    for (pixel, x, y) in sampled(&map) {
        let plain = reference_colour_with_locations(&map, STEP, pixel, &no_rivers, &no_locations);
        let with =
            reference_colour_with_mode(&map, STEP, pixel, &no_rivers, &no_locations, Some(&mode));
        assert_eq!(
            reference_colour_with_mode(&map, STEP, pixel, &no_rivers, &no_locations, None),
            plain,
            "no mode is the terrain reference"
        );
        if x.abs_diff(10) <= 2 && y.abs_diff(10) <= 2 {
            assert_eq!(
                with,
                Some(Rgba8::opaque(0, 255, 0)),
                "cell {x},{y} is in the box"
            );
            shown += 1;
        } else {
            assert_eq!(
                with, plain,
                "cell {x},{y} is outside the box and keeps its terrain colour"
            );
            kept += 1;
        }
    }
    assert!(
        shown > 0 && kept > 0,
        "the fixture samples both kinds of cell: {shown} shown, {kept} kept"
    );
}

#[test]
fn rivers_and_locations_draw_over_the_mode_and_the_outside_stays_sea() {
    let map = all_land(21, 21);
    let owners = vec![0_u16; 441];
    let values = [0_u16];
    let mode = ModeInputs {
        owners: &owners,
        values: &values,
    };
    let pixel = sampled(&map)[0].0;
    assert_eq!(
        reference_colour_with_mode(
            &map,
            STEP,
            pixel,
            &BTreeSet::new(),
            &BTreeMap::new(),
            Some(&mode)
        ),
        Some(Rgba8::opaque(255, 0, 0)),
        "the whole crop is owned at value 0: red"
    );
    assert_eq!(
        reference_colour_with_mode(
            &map,
            STEP,
            pixel,
            &BTreeSet::from([pixel]),
            &BTreeMap::new(),
            Some(&mode)
        ),
        Some(map.river)
    );
    let marker = Rgba8::opaque(90, 91, 92);
    assert_eq!(
        reference_colour_with_mode(
            &map,
            STEP,
            pixel,
            &BTreeSet::new(),
            &BTreeMap::from([(pixel, marker)]),
            Some(&mode)
        ),
        Some(marker)
    );
    assert_eq!(
        reference_colour_with_mode(
            &map,
            STEP,
            WorldPixel { x: 0, y: 0 },
            &BTreeSet::new(),
            &BTreeMap::new(),
            Some(&mode)
        ),
        Some(map.sea),
        "a pixel outside the crop is sea, whatever the mode"
    );
}

#[test]
fn the_mode_covers_water_cells_too() {
    let mut map = all_land(21, 21);
    let lake: Vec<usize> = (0..441).filter(|index| index % 21 >= 10).collect();
    for &index in &lake {
        map.cells[index] = Surface::Lake;
    }
    let owners = vec![0_u16; 441];
    let values = [1000_u16];
    let mode = ModeInputs {
        owners: &owners,
        values: &values,
    };
    let mut seen = 0;
    for (pixel, x, _) in sampled(&map) {
        let with = reference_colour_with_mode(
            &map,
            STEP,
            pixel,
            &BTreeSet::new(),
            &BTreeMap::new(),
            Some(&mode),
        );
        assert_eq!(with, Some(Rgba8::opaque(0, 255, 0)));
        seen += usize::from(x >= 10);
    }
    assert!(seen > 0, "the lake half is sampled");
}
