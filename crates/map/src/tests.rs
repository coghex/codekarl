use super::*;

fn fixture(cells: Vec<Surface>) -> MapData {
    MapData {
        crop: Crop {
            origin: Cell { x: 10, y: 20 },
            width: 3,
            height: 3,
        },
        cells,
        elevations: vec![11_004; 9],
        fertility: vec![500; 9],
        climate_temperature_tenths: vec![100; 9],
        climate_gdd: vec![1000; 9],
        climate_rainfall_mm: vec![1000; 9],
        glacier: vec![false; 9],
        hillside: vec![0; 9],
        ice: Rgba8::opaque(200, 200, 200),
        ice_second: Rgba8::opaque(150, 150, 150),
        terrains: vec![TerrainStyle {
            solid: Rgba8::opaque(1, 2, 3),
            second: Rgba8::opaque(4, 5, 6),
            dark: Rgba8::opaque(7, 8, 9),
            dark_lit: Rgba8::opaque(10, 11, 12),
        }],
        sea: Rgba8::opaque(20, 21, 22),
        shallow_sea: Rgba8::opaque(23, 24, 25),
        lake: Rgba8::opaque(26, 27, 28),
        river: Rgba8::opaque(29, 30, 31),
        shallow_threshold: 100,
    }
}

fn wide_river_map() -> MapData {
    let mut map = fixture(vec![
        Surface::Land {
            terrain: 0,
            usable: true,
        };
        9
    ]);
    let crop = Crop {
        origin: Cell { x: 0, y: 0 },
        width: 300,
        height: 300,
    };
    let cells = usize::try_from(crop.width * crop.height).unwrap();
    map.crop = crop;
    map.cells = vec![
        Surface::Land {
            terrain: 0,
            usable: true,
        };
        cells
    ];
    map.elevations = vec![11_004; cells];
    map.fertility = vec![500; cells];
    map.climate_temperature_tenths = vec![100; cells];
    map.climate_gdd = vec![1000; cells];
    map.climate_rainfall_mm = vec![1000; cells];
    map.glacier = vec![false; cells];
    map.hillside = vec![0; cells];
    map
}

fn cross_tile_river() -> RiverNetwork {
    let pixel_size = STEP_SIZE[53];
    let point = |x: i32, y: i32| FixedPoint {
        x: i32::try_from(i64::from(x) * i64::from(pixel_size)).unwrap(),
        y: i32::try_from(i64::from(y) * i64::from(pixel_size)).unwrap(),
    };
    RiverNetwork {
        show_from_step: [0; 3],
        rivers: vec![RiverRiver {
            id: "cross-tile".into(),
            name: "Cross tile".into(),
            tier: 1,
        }],
        stretches: vec![
            RiverStretch {
                id: "wide-course".into(),
                river: 0,
                kind: RiverStretchKind::River,
                points: vec![point(110, 129), point(146, 129)],
                width_first: 12_000,
                width_last: 12_000,
                ends: None,
                drawn_first: 12_000,
                drawn_last: 12_000,
            },
            RiverStretch {
                id: "narrow-course".into(),
                river: 0,
                kind: RiverStretchKind::River,
                points: vec![point(110, 64), point(146, 64)],
                width_first: 100,
                width_last: 100,
                ends: None,
                drawn_first: 100,
                drawn_last: 100,
            },
        ],
    }
}

fn encoded_elevation_map() -> MapData {
    let mut map = fixture(vec![
        Surface::Land {
            terrain: 0,
            usable: true,
        };
        4
    ]);
    map.crop = Crop {
        origin: Cell { x: 0, y: 0 },
        width: 2,
        height: 2,
    };
    map.elevations = vec![11_000, 11_100, 11_400, 12_000];
    map.fertility = vec![500; 4];
    map.climate_temperature_tenths = vec![100; 4];
    map.climate_gdd = vec![1000; 4];
    map.climate_rainfall_mm = vec![1000; 4];
    map.glacier = vec![false; 4];
    map.hillside = vec![0; 4];
    map
}

fn all_land() -> Vec<Surface> {
    vec![
        Surface::Land {
            terrain: 0,
            usable: true
        };
        9
    ]
}

fn pixels_for_cell(cell: Cell, step: u8) -> Vec<WorldPixel> {
    let p = i64::from(STEP_SIZE[usize::from(step)]);
    let approximate_x = i64::from(cell.x) * FIXED_PER_CELL / p;
    let approximate_y = i64::from(cell.y) * FIXED_PER_CELL / p;
    let mut pixels = Vec::new();
    for y in (approximate_y - 8)..=(approximate_y + 8) {
        for x in (approximate_x - 8)..=(approximate_x + 8) {
            let Ok(x) = i32::try_from(x) else { continue };
            let Ok(y) = i32::try_from(y) else { continue };
            let pixel = WorldPixel { x, y };
            if sample_point(pixel, step).is_some_and(|unmoved| {
                inside_crop(
                    Crop {
                        origin: Cell { x: 10, y: 20 },
                        width: 3,
                        height: 3,
                    },
                    unmoved,
                ) && jittered_point(unmoved, step).is_some_and(|moved| cell_of(moved) == cell)
            }) {
                pixels.push(pixel);
            }
        }
    }
    pixels
}

#[test]
fn projection_known_points_wrap_and_clamp() {
    let origin = project(0.0, -180.0).expect("valid origin");
    assert_eq!(cell_of(origin), Cell { x: 0, y: 23_882 });
    let east = project(0.0, 0.0).expect("valid equator");
    assert_eq!(
        cell_of(east),
        Cell {
            x: 23_882,
            y: 23_882
        }
    );
    assert_eq!(project(0.0, 180.0), project(0.0, -180.0));
    assert_eq!(project(90.0, 0.0), project(MAX_MERCATOR_LATITUDE, 0.0));
    assert!(project(f64::NAN, 0.0).is_none());
}

#[test]
fn fixed_point_boundaries_use_the_east_and_south_cell() {
    assert_eq!(cell_of(FixedPoint { x: -1, y: -1 }), Cell { x: -1, y: -1 });
    assert_eq!(cell_of(FixedPoint { x: 0, y: 0 }), Cell { x: 0, y: 0 });
    assert_eq!(
        cell_of(FixedPoint {
            x: 32_767,
            y: 32_767
        }),
        Cell { x: 0, y: 0 }
    );
    assert_eq!(
        cell_of(FixedPoint {
            x: 32_768,
            y: 32_768
        }),
        Cell { x: 1, y: 1 }
    );
}

#[test]
fn crop_maps_image_edges_to_global_cells() {
    let crop = Crop {
        origin: Cell { x: -3, y: 7 },
        width: 2,
        height: 4,
    };
    assert!(crop.contains(Cell { x: -3, y: 7 }));
    assert!(crop.contains(Cell { x: -2, y: 10 }));
    assert!(!crop.contains(Cell { x: -4, y: 7 }));
    assert!(!crop.contains(Cell { x: -1, y: 7 }));
    assert!(!crop.contains(Cell { x: -3, y: 11 }));
}

#[test]
fn frozen_steps_match_the_f64_formula() {
    for (step, actual) in STEP_SIZE.iter().enumerate() {
        let expected = format!(
            "{:.0}",
            32_768.0
                * 6.0
                * 2_f64.powf(-f64::from(u32::try_from(step).expect("step index fits")) / 10.0)
        )
        .parse::<i32>()
        .expect("step size fits");
        assert_eq!(*actual, expected, "step {step}");
    }
    assert_eq!(STEP_SIZE.len(), 76);
}

#[test]
fn world_extremes_and_sample_points_fit_i32() {
    let edge = project(MAX_MERCATOR_LATITUDE, 179.999_999).expect("edge fits");
    assert!(edge.x > 0 && edge.y >= 0);
    assert!(i64::from(edge.x) < i64::from(i32::MAX));
    assert!(sample_point(WorldPixel { x: 0, y: 0 }, 0).is_some());
    assert!(
        sample_point(
            WorldPixel {
                x: i32::MAX,
                y: i32::MAX
            },
            0
        )
        .is_none()
    );
}

#[test]
fn camera_pan_zoom_resize_and_quantization_preserve_exact_center() {
    let crop = Crop {
        origin: Cell { x: -100, y: -100 },
        width: 200,
        height: 200,
    };
    let center = FixedPoint { x: 0, y: 0 };
    let mut camera = Camera::new(
        30,
        center,
        ViewSize {
            width: 9,
            height: 7,
        },
        crop,
    )
    .expect("valid camera");
    let start = camera.top_left();
    camera.pan(3, -2, crop);
    camera.pan(-3, 2, crop);
    assert_eq!(camera.top_left(), start);
    for step in [31, 42, 2, 75, 0, 30] {
        assert!(camera.set_step(step));
    }
    assert_eq!(camera.center, center);
    assert_eq!(camera.top_left(), start);
    for step in 0..=75 {
        camera.set_step(step);
        let shown = camera.displayed_center();
        let p = i64::from(STEP_SIZE[usize::from(step)]);
        assert!((i64::from(shown.x) - i64::from(center.x)).abs() < p);
        assert!((i64::from(shown.y) - i64::from(center.y)).abs() < p);
    }
    camera.resize(ViewSize {
        width: 10,
        height: 8,
    });
    assert_eq!(camera.center, center);
    assert_eq!(
        ViewSize::from_points(9, 13),
        ViewSize {
            width: 3,
            height: 4
        }
    );
}

#[test]
fn camera_clamps_center_to_crop_and_rejects_bad_step() {
    let crop = Crop {
        origin: Cell { x: 2, y: 3 },
        width: 4,
        height: 5,
    };
    let mut camera = Camera::new(
        0,
        FixedPoint { x: -1, y: i32::MAX },
        ViewSize::default(),
        crop,
    )
    .expect("valid camera");
    assert_eq!(
        camera.center,
        FixedPoint {
            x: 2 * 32_768,
            y: 8 * 32_768
        }
    );
    assert!(!camera.set_step(76));
    assert!(Camera::new(76, FixedPoint::default(), ViewSize::default(), crop).is_none());
}

#[test]
fn view_size_uses_the_same_points_at_each_display_scale() {
    let size = ViewSize::from_points(801, 601);
    assert_eq!(
        size,
        ViewSize {
            width: 201,
            height: 151
        }
    );
    for _scale in [1.0_f32, 1.5, 2.0] {
        assert_eq!(ViewSize::from_points(801, 601), size);
    }
}

#[test]
fn validation_rejects_bad_grid_and_style_inputs() {
    let mut map = fixture(all_land());
    map.cells.pop();
    assert!(map.validate().is_err());
    let mut map = fixture(all_land());
    map.hillside[0] = 16;
    assert!(map.validate().is_err(), "a hillside strength beyond 15");
    let mut map = fixture(all_land());
    map.hillside.pop();
    assert!(map.validate().is_err(), "a hillside strength missing");
    let mut map = fixture(all_land());
    map.cells[0] = Surface::Land {
        terrain: 3,
        usable: true,
    };
    assert!(map.validate().is_err());
    let mut map = fixture(all_land());
    map.crop.origin.x = i32::MAX;
    assert!(map.validate().is_err());
    let mut map = fixture(all_land());
    map.elevations.pop();
    assert!(map.validate().is_err());
    let mut map = fixture(all_land());
    map.crop.origin.x = -1;
    assert!(map.validate().is_err());
}

/// The map's centre cell (11, 21) set to `fertility_pm`, glacier flag and hillside `strength`.
fn centre(fertility_pm: u16, glacier: bool, strength: i8) -> MapData {
    let mut map = fixture(all_land());
    map.fertility[4] = fertility_pm;
    map.glacier[4] = glacier;
    map.hillside[4] = strength;
    map
}

/// The first pixel sampling the centre cell at step 50 for which `wanted(blue16 value, bayer value)` holds.
fn centre_pixel(wanted: impl Fn(u16, i16) -> bool) -> WorldPixel {
    pixels_for_cell(Cell { x: 11, y: 21 }, 50)
        .into_iter()
        .find(|pixel| {
            let blue = u16::from(
                look::BLUE16[pixel.y.rem_euclid(16) as usize][pixel.x.rem_euclid(16) as usize],
            );
            let ordered = i16::from(
                look::BAYER4_TRANSPOSED[pixel.y.rem_euclid(4) as usize]
                    [pixel.x.rem_euclid(4) as usize],
            );
            wanted(blue, ordered)
        })
        .expect("the cell has a pixel of the wanted pattern values")
}

fn colour_at(map: &MapData, pixel: WorldPixel) -> Rgba8 {
    assert!(!has_water_neighbour(map, 50, pixel), "an interior pixel");
    reference_colour(map, 50, pixel, &BTreeSet::new()).expect("a colour")
}

#[test]
fn a_fertile_pixel_is_solid_and_a_barren_one_dark_on_level_ground() {
    let style = fixture(all_land()).terrains[0];
    // Full fertility (stored 250) is 256 levels: every pixel of the blue-noise tile is below it.
    let full = centre(1000, false, 0);
    let any = centre_pixel(|_, _| true);
    assert_eq!(colour_at(&full, any), style.solid);
    // No fertility is no level: none is.
    let barren = centre(0, false, 0);
    assert_eq!(colour_at(&barren, any), style.dark);
    // In between, the shade is three times the level with the blue-noise value deciding the remainder: stored 100 (400 per mille)
    // is level 25, so `v` is 75, `k` 0, and a pixel whose blue value is below 75 takes shade 1 and the others stay dark.
    let middle = centre(400, false, 0);
    let shades = look::TerrainShades::of(&style);
    assert_eq!(look::FERTILITY_LEVELS[100], 25);
    assert_eq!(
        colour_at(&middle, centre_pixel(|blue, _| blue < 75)),
        shades.tone[1]
    );
    assert_eq!(
        colour_at(&middle, centre_pixel(|blue, _| blue >= 75)),
        style.dark
    );
    // The strength's sign alone changes nothing without a pattern hit: the wrong side leaves the pixel as it was.
    assert_eq!(
        colour_at(&centre(1000, false, 5), any),
        style.solid,
        "lit side, bright"
    );
    assert_eq!(
        colour_at(&centre(0, false, -5), any),
        style.dark,
        "shadow side, dark"
    );
}

#[test]
fn the_hillside_dither_takes_the_second_tone_on_bright_shadow_and_the_lit_tone_on_dark_light() {
    let style = fixture(all_land()).terrains[0];
    // A bright pixel on the shadow side (negative strength) takes `second` where the Bayer value is below |strength|.
    let shadow = centre(1000, false, -8);
    assert_eq!(
        colour_at(&shadow, centre_pixel(|_, bayer| bayer < 8)),
        style.second
    );
    assert_eq!(
        colour_at(&shadow, centre_pixel(|_, bayer| bayer >= 8)),
        style.solid
    );
    // The threshold is the strength itself: at -1 only Bayer value 0 qualifies.
    let faint = centre(1000, false, -1);
    assert_eq!(
        colour_at(&faint, centre_pixel(|_, bayer| bayer == 0)),
        style.second
    );
    assert_eq!(
        colour_at(&faint, centre_pixel(|_, bayer| bayer == 1)),
        style.solid
    );
    // A dark pixel on the lit side (positive strength) takes `dark_lit` under the same test.
    let lit = centre(0, false, 8);
    assert_eq!(
        colour_at(&lit, centre_pixel(|_, bayer| bayer < 8)),
        style.dark_lit
    );
    assert_eq!(
        colour_at(&lit, centre_pixel(|_, bayer| bayer >= 8)),
        style.dark
    );
    // At +15 and -15, the strongest, all but Bayer value 15 qualify.
    assert_eq!(
        colour_at(&centre(0, false, 15), centre_pixel(|_, bayer| bayer == 14)),
        style.dark_lit
    );
    assert_eq!(
        colour_at(&centre(0, false, 15), centre_pixel(|_, bayer| bayer == 15)),
        style.dark
    );
    assert_eq!(
        colour_at(
            &centre(1000, false, -15),
            centre_pixel(|_, bayer| bayer == 14)
        ),
        style.second
    );
}

#[test]
fn a_glacier_is_ice_with_its_second_tone_on_the_shadow_side_whatever_the_fertility() {
    let map = centre(1000, true, 0);
    let any = centre_pixel(|_, _| true);
    assert_eq!(
        colour_at(&map, any),
        map.ice,
        "the fertility pattern is skipped"
    );
    let shadow = centre(0, true, -8);
    assert_eq!(
        colour_at(&shadow, centre_pixel(|_, bayer| bayer < 8)),
        shadow.ice_second
    );
    assert_eq!(
        colour_at(&shadow, centre_pixel(|_, bayer| bayer >= 8)),
        shadow.ice
    );
    let lit = centre(0, true, 8);
    assert_eq!(
        colour_at(&lit, centre_pixel(|_, bayer| bayer < 8)),
        lit.ice,
        "no second tone on the lit side"
    );
}

#[test]
fn the_patterns_are_pinned_to_the_world_pixel_and_so_to_negative_pixels_too() {
    let style = fixture(all_land()).terrains[0];
    let map = centre(400, false, 0);
    // The same world pixel has the same colour whatever the view, and the tile repeats every 16 pixels.
    let pixel = centre_pixel(|blue, _| blue < 25);
    let again = WorldPixel {
        x: pixel.x + 16,
        y: pixel.y + 32,
    };
    let blue = |pixel: WorldPixel| {
        look::BLUE16[pixel.y.rem_euclid(16) as usize][pixel.x.rem_euclid(16) as usize]
    };
    assert_eq!(blue(pixel), blue(again));
    assert_eq!(
        look::land_colour(&style, (map.ice, map.ice_second), 400, false, 0, pixel),
        look::land_colour(&style, (map.ice, map.ice_second), 400, false, 0, again)
    );
    // Negative world pixels wrap to the other end of the tile.
    assert_eq!(blue(WorldPixel { x: -1, y: -1 }), look::BLUE16[15][15]);
}

#[test]
fn the_coast_outline_water_and_the_outside_of_the_crop_are_unchanged_by_the_look() {
    let mut map = centre(0, false, 8);
    map.cells[1] = Surface::Lake;
    let coast = pixels_for_cell(Cell { x: 11, y: 21 }, 50)
        .into_iter()
        .find(|pixel| has_water_neighbour(&map, 50, *pixel))
        .expect("a land pixel touches the lake");
    assert_eq!(
        reference_colour(&map, 50, coast, &BTreeSet::new()),
        Some(map.terrains[0].solid),
        "the outline is the terrain's solid colour, whatever the fertility and hillside"
    );
    let lake = pixels_for_cell(Cell { x: 11, y: 20 }, 50)[0];
    assert_eq!(
        reference_colour(&map, 50, lake, &BTreeSet::new()),
        Some(map.lake)
    );
    assert_eq!(
        reference_colour(&map, 50, WorldPixel { x: 0, y: 0 }, &BTreeSet::new()),
        Some(map.sea)
    );
}

#[test]
fn colour_rule_thresholds_and_river_overlay_are_integer_and_ordered() {
    let mut map = fixture(all_land());
    map.cells[4] = Surface::Sea { depth: 50 };
    let pixel = pixels_for_cell(Cell { x: 11, y: 21 }, 50)
        .into_iter()
        .find(|pixel| bayer(*pixel) < 4)
        .expect("pixel meets the shallow dither threshold");
    assert_eq!(
        reference_colour(&map, 50, pixel, &BTreeSet::new()),
        Some(map.shallow_sea)
    );
    let dry_dither = pixels_for_cell(Cell { x: 11, y: 21 }, 50)
        .into_iter()
        .find(|candidate| bayer(*candidate) >= 4)
        .expect("pixel falls outside the shallow dither threshold");
    assert_eq!(
        reference_colour(&map, 50, dry_dither, &BTreeSet::new()),
        Some(map.sea)
    );
    map.cells[4] = Surface::Sea { depth: 100 };
    assert_eq!(
        reference_colour(&map, 50, pixel, &BTreeSet::new()),
        Some(map.sea)
    );
    let river = BTreeSet::from([pixel]);
    assert_eq!(reference_colour(&map, 50, pixel, &river), Some(map.river));
}

#[test]
fn bayer_is_pinned_for_negative_world_pixels() {
    assert_eq!(bayer(WorldPixel { x: -1, y: -1 }), 5);
    assert_eq!(bayer(WorldPixel { x: 0, y: 0 }), 0);
    assert_eq!(bayer(WorldPixel { x: 1, y: 0 }), 8);
}

#[test]
fn frozen_jitter_hash_noise_and_activation_vectors_match_r4() {
    assert_eq!(lowbias32(0), 0x0000_0000);
    assert_eq!(lowbias32(1), 0x6889_90c0);
    assert_eq!(lowbias32(0xdead_beef), 0xe628_c683);
    assert_eq!(corner_hash(0, 0, 1), 0x6d87_54e2);
    assert_eq!(corner_hash(5, 7, 3), 0x81b2_2d4b);
    assert_eq!(noise12(0, 0, 98_304, 1), 1752);
    assert_eq!(noise12(49_152, 16_384, 98_304, 1), 1388);
    assert_eq!(noise12(1_234_567, 7_654_321, 98_304, 1), 3497);

    let point = FixedPoint {
        x: 1_234_567,
        y: 7_654_321,
    };
    assert_eq!(jittered_point(point, 13), Some(point));
    assert_eq!(
        jittered_point(point, 14),
        Some(FixedPoint {
            x: 1_255_432,
            y: 7_677_879
        })
    );
    assert_eq!(jittered_point(point, 33), jittered_point(point, 14));
    assert_eq!(
        jittered_point(point, 34),
        Some(FixedPoint {
            x: 1_256_574,
            y: 7_676_691
        })
    );
    assert_eq!(jittered_point(point, 52), jittered_point(point, 34));
    assert_eq!(
        jittered_point(point, 75),
        Some(FixedPoint {
            x: 1_255_615,
            y: 7_676_959
        })
    );
    assert_eq!(
        jittered_point(point, 75),
        Some(FixedPoint {
            x: 1_255_615,
            y: 7_676_959
        })
    );
    assert_ne!(jittered_point(point, 53), jittered_point(point, 52));
    assert!(u64::from(u16::MAX) * 4096 < u64::from(u32::MAX));
    assert!(
        u64::from(4095_u16) * 2 * 29_491
            < u64::try_from(i32::MAX).expect("positive i32 maximum fits u64")
    );
}

#[test]
fn crop_is_checked_before_jitter_and_outside_neighbours_are_sea() {
    let mut map = encoded_elevation_map();
    let outside = WorldPixel { x: -1, y: 0 };
    assert_eq!(
        reference_colour(&map, 14, outside, &BTreeSet::new()),
        Some(map.sea)
    );
    let river = BTreeSet::from([outside]);
    assert_eq!(
        reference_colour(&map, 14, outside, &river),
        Some(map.river),
        "rivers remain the final overlay even beyond the crop"
    );
    let outside_above = WorldPixel { x: 0, y: -1 };
    assert_eq!(
        reference_colour(&map, 14, outside_above, &BTreeSet::new()),
        Some(map.sea)
    );
    let moved_outside = WorldPixel { x: 0, y: 2 };
    assert_eq!(
        reference_colour(&map, 34, moved_outside, &BTreeSet::new()),
        Some(map.sea)
    );

    let edge = WorldPixel { x: 0, y: 0 };
    map.elevations.fill(11_004);
    assert_eq!(
        reference_colour(&map, 13, edge, &BTreeSet::new()),
        Some(map.terrains[0].solid)
    );
}

#[test]
fn prepared_location_overlay_precedes_jitter_crop_rejection_but_not_rivers() {
    use std::collections::BTreeMap;

    let map = encoded_elevation_map();
    let moved_outside = WorldPixel { x: 0, y: 2 };
    let location_colour = Rgba8::opaque(201, 101, 51);
    let locations = BTreeMap::from([(moved_outside, location_colour)]);

    assert_eq!(
        reference_colour_with_locations(&map, 34, moved_outside, &BTreeSet::new(), &locations,),
        Some(location_colour),
        "a prepared location survives terrain jitter moving its sample outside the crop"
    );

    let rivers = BTreeSet::from([moved_outside]);
    assert_eq!(
        reference_colour_with_locations(&map, 34, moved_outside, &rivers, &locations),
        Some(map.river),
        "river colour remains above a prepared location overlay"
    );
}

fn fp_pixel(x: i32, y: i32, step: u8) -> FixedPoint {
    let p = STEP_SIZE[usize::from(step)];
    FixedPoint { x: x * p, y: y * p }
}

#[test]
fn rivers_cover_horizontal_vertical_diagonal_and_steep_segments() {
    for (end, expected_len) in [((3, 0), 4), ((0, 3), 4), ((3, 3), 4), ((1, 4), 5)] {
        let line = RiverLine {
            points: vec![fp_pixel(0, 0, 50), fp_pixel(end.0, end.1, 50)],
        };
        let coverage = RiverCoverage::prepare(&[line], 50).expect("valid step");
        assert_eq!(coverage.pixels().len(), expected_len);
        assert!(
            coverage
                .pixels()
                .contains(&WorldPixel { x: end.0, y: end.1 })
        );
    }
}

#[test]
fn network_tiles_keep_tapered_line_and_water_separate_and_are_view_bounded() {
    let map = fixture(all_land());
    let start = FixedPoint {
        x: 10 * i32::try_from(FIXED_PER_CELL).expect("cell scale fits i32") + 20_000,
        y: 21 * i32::try_from(FIXED_PER_CELL).expect("cell scale fits i32"),
    };
    let end = FixedPoint {
        x: 10 * i32::try_from(FIXED_PER_CELL).expect("cell scale fits i32") + 75_000,
        y: 21 * i32::try_from(FIXED_PER_CELL).expect("cell scale fits i32"),
    };
    let network = RiverNetwork {
        show_from_step: [0, 0, 0],
        rivers: vec![RiverRiver {
            id: "main".into(),
            name: "Main".into(),
            tier: 1,
        }],
        stretches: vec![RiverStretch {
            id: "course".into(),
            river: 0,
            kind: RiverStretchKind::River,
            points: vec![start, end],
            width_first: 2_000,
            width_last: 12_000,
            ends: None,
            drawn_first: 2_000,
            drawn_last: 12_000,
        }],
    };
    let top = WorldPixel {
        x: fixed_to_pixel(start, i64::from(STEP_SIZE[53])).x - 1,
        y: fixed_to_pixel(start, i64::from(STEP_SIZE[53])).y - 2,
    };
    let view = ViewSize {
        width: 24,
        height: 16,
    };
    let mut prepared = PreparedRiverNetwork::new(&map, &network);
    let first = prepared
        .select_view(53, top, view)
        .expect("valid network tile");
    let second = prepared
        .select_view(53, top, view)
        .expect("cached network tile");
    assert_eq!(first, second);
    assert!(
        !first.line.is_empty(),
        "the narrow upstream portion remains a blue line"
    );
    assert!(
        !first.water.is_empty(),
        "the downstream portion widens into river water"
    );
    assert!(
        first
            .line
            .iter()
            .chain(&first.water)
            .all(|p| p.x >= 0 && p.y >= 0 && p.x < 24 && p.y < 16)
    );
}

#[test]
fn format_two_river_water_adds_a_bank_before_value_mode_colours() {
    let map = fixture(all_land());
    let step = 50_u8;
    let pixel = fixed_to_pixel(
        FixedPoint {
            x: 11 * i32::try_from(FIXED_PER_CELL).expect("cell scale fits i32"),
            y: 21 * i32::try_from(FIXED_PER_CELL).expect("cell scale fits i32"),
        },
        i64::from(STEP_SIZE[usize::from(step)]),
    );
    let water = BTreeSet::from([WorldPixel {
        x: pixel.x + 1,
        y: pixel.y,
    }]);
    assert_eq!(
        reference_colour_with_river_network(
            &map,
            step,
            pixel,
            &BTreeSet::new(),
            &water,
            &BTreeMap::new(),
            None
        ),
        Some(map.terrains[0].solid)
    );
    assert_eq!(
        reference_colour_with_river_network(
            &map,
            step,
            WorldPixel {
                x: pixel.x + 1,
                y: pixel.y
            },
            &BTreeSet::new(),
            &water,
            &BTreeMap::new(),
            None
        ),
        Some(map.lake)
    );
}

#[test]
fn a_narrow_tributary_line_is_removed_under_another_stretches_water() {
    let map = fixture(all_land());
    let center = |x: i32, y: i32| FixedPoint {
        x: x * 32_768 + 16_384,
        y: y * 32_768 + 16_384,
    };
    let network = RiverNetwork {
        show_from_step: [0, 0, 0],
        rivers: vec![
            RiverRiver {
                id: "main".into(),
                name: "Main".into(),
                tier: 1,
            },
            RiverRiver {
                id: "branch".into(),
                name: "Branch".into(),
                tier: 1,
            },
        ],
        stretches: vec![
            RiverStretch {
                id: "main-course".into(),
                river: 0,
                kind: RiverStretchKind::River,
                points: vec![center(10, 21), center(12, 21)],
                width_first: 8_000,
                width_last: 8_000,
                ends: None,
                drawn_first: 8_000,
                drawn_last: 8_000,
            },
            RiverStretch {
                id: "tributary".into(),
                river: 1,
                kind: RiverStretchKind::River,
                points: vec![center(10, 20), center(10, 21)],
                width_first: 1_000,
                width_last: 1_000,
                ends: Some(0),
                drawn_first: 1_000,
                drawn_last: 1_000,
            },
        ],
    };
    let mut prepared = PreparedRiverNetwork::new(&map, &network);
    let anchor = fixed_to_pixel(center(10, 21), i64::from(STEP_SIZE[53]));
    let parts = prepared
        .select_view(
            53,
            WorldPixel {
                x: anchor.x - 8,
                y: anchor.y - 8,
            },
            ViewSize {
                width: 24,
                height: 24,
            },
        )
        .expect("confluence raster");
    assert!(!parts.water.is_empty());
    assert!(
        parts.line.is_disjoint(&parts.water),
        "all river water hides every narrow line"
    );
}

#[test]
fn format_two_elevation_keeps_line_water_and_bank_colours() {
    let map = encoded_elevation_map();
    let pixel = fixed_to_pixel(
        FixedPoint {
            x: 16_384,
            y: 16_384,
        },
        i64::from(STEP_SIZE[50]),
    );
    let line = BTreeSet::from([pixel]);
    let water = BTreeSet::from([WorldPixel {
        x: pixel.x + 2,
        y: pixel.y + 2,
    }]);
    let bank = WorldPixel {
        x: pixel.x + 2,
        y: pixel.y + 1,
    };
    let empty = BTreeMap::new();
    assert_eq!(
        reference_colour_with_river_network_elevation(&map, 50, pixel, &line, &water, &empty),
        Some(map.river)
    );
    let water_pixel = *water.first().expect("water pixel");
    assert_eq!(
        reference_colour_with_river_network_elevation(&map, 50, water_pixel, &line, &water, &empty),
        Some(map.lake)
    );
    assert_eq!(
        reference_colour_with_river_network_elevation(&map, 50, bank, &line, &water, &empty),
        Some(map.terrains[0].solid)
    );
    let interior = fixed_to_pixel(
        FixedPoint {
            x: 49_152,
            y: 49_152,
        },
        i64::from(STEP_SIZE[50]),
    );
    let expected = reference_colour_with_elevation(&map, 50, interior, &BTreeSet::new(), &empty);
    assert_ne!(
        expected,
        Some(map.terrains[0].solid),
        "interior elevation is distinguishable"
    );
    assert_eq!(
        reference_colour_with_river_network_elevation(
            &map,
            50,
            interior,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &empty
        ),
        expected
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "assert all format-2 colour precedence rules against one prepared test map"
)]
fn network_colours_keep_river_location_bank_value_and_empty_set_ordering() {
    let mut map = fixture(all_land());
    map.terrains[0].solid = Rgba8::opaque(43, 51, 59);
    map.terrains.push(TerrainStyle {
        solid: Rgba8::opaque(71, 83, 97),
        second: Rgba8::opaque(1, 1, 1),
        dark: Rgba8::opaque(2, 2, 2),
        dark_lit: Rgba8::opaque(3, 3, 3),
    });
    map.cells[4] = Surface::Land {
        terrain: 1,
        usable: true,
    };
    let step = 50;
    let pixel = pixels_for_cell(Cell { x: 11, y: 21 }, step)[4];
    let line = BTreeSet::from([pixel]);
    let water_pixel = pixels_for_cell(Cell { x: 11, y: 21 }, step)[0];
    let water = BTreeSet::from([water_pixel]);
    let bank = [(-1, 0), (1, 0), (0, -1), (0, 1)]
        .into_iter()
        .map(|(dx, dy)| WorldPixel {
            x: water_pixel.x + dx,
            y: water_pixel.y + dy,
        })
        .find(|candidate| {
            matches!(
                surface_for_pixel(&map, step, *candidate),
                Some(Surface::Land { terrain: 1, .. })
            )
        })
        .expect("water neighbour is land in the forest terrain cell");
    let location_colour = Rgba8::opaque(201, 101, 51);
    let locations = BTreeMap::from([
        (pixel, location_colour),
        (water_pixel, location_colour),
        (bank, location_colour),
    ]);
    assert_eq!(
        reference_colour_with_river_network(&map, step, pixel, &line, &water, &locations, None),
        Some(map.river)
    );
    assert_eq!(
        reference_colour_with_river_network(
            &map,
            step,
            water_pixel,
            &line,
            &water,
            &locations,
            None
        ),
        Some(map.lake)
    );
    let pasture_pixel = pixels_for_cell(Cell { x: 10, y: 21 }, step)[4];
    assert_eq!(
        reference_colour_with_river_network(
            &map,
            step,
            pasture_pixel,
            &BTreeSet::from([pasture_pixel]),
            &BTreeSet::new(),
            &BTreeMap::new(),
            None
        ),
        Some(map.river)
    );
    assert_eq!(
        reference_colour_with_river_network(
            &map,
            step,
            pasture_pixel,
            &BTreeSet::new(),
            &BTreeSet::from([pasture_pixel]),
            &BTreeMap::new(),
            None
        ),
        Some(map.lake)
    );
    assert_eq!(
        reference_colour_with_river_network(&map, step, bank, &line, &water, &locations, None),
        Some(location_colour)
    );
    assert_eq!(
        reference_colour_with_river_network(
            &map,
            step,
            bank,
            &line,
            &water,
            &BTreeMap::new(),
            None
        ),
        Some(map.terrains[1].solid)
    );
    assert!(
        !has_water_neighbour(&map, step, bank),
        "the river bank sample is inland from sea and lake"
    );
    let line_neighbour = WorldPixel {
        x: pixel.x + 1,
        y: pixel.y,
    };
    assert_ne!(
        reference_colour_with_river_network(
            &map,
            step,
            line_neighbour,
            &line,
            &BTreeSet::new(),
            &BTreeMap::new(),
            None
        ),
        Some(map.river)
    );

    let owners = vec![0_u16; 9];
    let values = [0_u16];
    let mode = ModeInputs {
        owners: &owners,
        values: &values,
    };
    assert_eq!(
        reference_colour_with_river_network(
            &map,
            step,
            bank,
            &BTreeSet::new(),
            &water,
            &BTreeMap::new(),
            Some(&mode)
        ),
        Some(map.terrains[1].solid)
    );
    let mut coast_map = map.clone();
    coast_map.cells[1] = Surface::Sea { depth: 100 };
    let coastal = pixels_for_cell(Cell { x: 11, y: 21 }, step)
        .into_iter()
        .find(|p| has_water_neighbour(&coast_map, step, *p))
        .expect("coastal land pixel");
    assert_eq!(
        reference_colour_with_river_network(
            &coast_map,
            step,
            coastal,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeMap::new(),
            Some(&mode)
        ),
        Some(mode_colour(0))
    );

    let empty = BTreeSet::new();
    assert_eq!(
        reference_colour_with_river_network(
            &map,
            step,
            pixel,
            &empty,
            &empty,
            &BTreeMap::new(),
            None
        ),
        reference_colour_with_locations(&map, step, pixel, &empty, &BTreeMap::new()),
    );
    assert_eq!(
        reference_colour_with_river_network(
            &map,
            step,
            pixel,
            &empty,
            &empty,
            &BTreeMap::new(),
            Some(&mode)
        ),
        reference_colour_with_mode(&map, step, pixel, &empty, &BTreeMap::new(), Some(&mode)),
    );
}

#[test]
fn fixture_network_uses_confluence_widths_and_crosses_the_wide_threshold() {
    let fixture = fixture::fixture();
    let network = &fixture.river_network;
    let main = network
        .stretches
        .iter()
        .find(|s| s.id == "main-down")
        .expect("main downstream");
    assert_eq!(main.drawn_first, 600);
    assert!(main.drawn_last > main.drawn_first);
    let mut narrow = PreparedRiverNetwork::new(&fixture.data, network);
    let mut wide = PreparedRiverNetwork::new(&fixture.data, network);
    let view = ViewSize {
        width: 80,
        height: 80,
    };
    let anchor_zero = fixed_to_pixel(main.points[0], i64::from(STEP_SIZE[0]));
    let top_zero = WorldPixel {
        x: anchor_zero.x - 4,
        y: anchor_zero.y - 4,
    };
    let anchor_75 = fixed_to_pixel(main.points[0], i64::from(STEP_SIZE[75]));
    let top_75 = WorldPixel {
        x: anchor_75.x - 20,
        y: anchor_75.y - 20,
    };
    let at_zero = narrow
        .select_view(0, top_zero, view)
        .expect("coarse fixture step");
    let at_75 = wide
        .select_view(75, top_75, view)
        .expect("close fixture step");
    assert!(!at_zero.line.is_empty());
    assert!(at_zero.water.is_empty());
    assert!(!at_75.water.is_empty());
}

#[test]
fn confluence_widths_propagate_and_interpolate_at_both_ends() {
    let point = FixedPoint { x: 1, y: 2 };
    let stretch = |id: &str, first, last, ends| RiverStretch {
        id: id.into(),
        river: 0,
        kind: RiverStretchKind::River,
        points: vec![
            point,
            FixedPoint {
                x: point.x + 1,
                y: point.y,
            },
        ],
        width_first: first,
        width_last: last,
        ends,
        drawn_first: first,
        drawn_last: last,
    };
    let mut chain = vec![
        stretch("up", 40, 30, Some(1)),
        stretch("mid", 30, 30, Some(2)),
        stretch("down", 20, 60, None),
    ];
    apply_confluence_widths(&mut chain);
    assert_eq!((chain[0].drawn_first, chain[0].drawn_last), (40, 40));
    assert_eq!((chain[1].drawn_first, chain[1].drawn_last), (40, 40));
    assert_eq!((chain[2].drawn_first, chain[2].drawn_last), (40, 60));
    assert_eq!(
        river_width_at(chain[2].drawn_first, chain[2].drawn_last, 0, 1),
        Some(40)
    );
    assert_eq!(
        river_width_at(chain[2].drawn_first, chain[2].drawn_last, 1, 1),
        Some(60)
    );

    let mut tributary = vec![
        stretch("large-branch", 30, 80, Some(1)),
        stretch("main", 40, 50, None),
    ];
    apply_confluence_widths(&mut tributary);
    assert_eq!(
        (tributary[1].drawn_first, tributary[1].drawn_last),
        (80, 80)
    );
    assert_eq!(
        river_width_at(10, 50, 1, 3),
        Some(23),
        "partial position floors interpolation"
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "compare one prepared network across tile, pan and step transitions"
)]
fn network_views_match_across_tiles_negative_starts_pans_and_step_changes() {
    let map = fixture(all_land());
    let center = |x: i32, y: i32| FixedPoint {
        x: x * 32_768 + 16_384,
        y: y * 32_768 + 16_384,
    };
    let network = RiverNetwork {
        show_from_step: [0, 0, 0],
        rivers: vec![RiverRiver {
            id: "main".into(),
            name: "Main".into(),
            tier: 1,
        }],
        stretches: vec![RiverStretch {
            id: "cross".into(),
            river: 0,
            kind: RiverStretchKind::River,
            points: vec![center(10, 21), center(12, 21)],
            width_first: 500,
            width_last: 9_000,
            ends: None,
            drawn_first: 500,
            drawn_last: 9_000,
        }],
    };
    let step = 75;
    let view = ViewSize {
        width: 60,
        height: 40,
    };
    let top = WorldPixel { x: 350, y: 620 };
    let mut prepared = PreparedRiverNetwork::new(&map, &network);
    let cross_tile = prepared
        .select_view(step, top, view)
        .expect("cross-tile view");
    assert!(
        !cross_tile.water.is_empty(),
        "the requested view includes the wide part"
    );
    let full = prepared
        .select_view(
            step,
            WorldPixel { x: 250, y: 600 },
            ViewSize {
                width: 220,
                height: 80,
            },
        )
        .expect("expanded view");
    let crop = |set: &BTreeSet<WorldPixel>, origin: WorldPixel| {
        set.iter()
            .filter_map(|p| {
                let absolute = WorldPixel {
                    x: p.x + origin.x,
                    y: p.y + origin.y,
                };
                ((absolute.x >= top.x && absolute.x < top.x + 60)
                    && (absolute.y >= top.y && absolute.y < top.y + 40))
                    .then_some(WorldPixel {
                        x: absolute.x - top.x,
                        y: absolute.y - top.y,
                    })
            })
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        cross_tile.line,
        crop(&full.line, WorldPixel { x: 250, y: 600 })
    );
    assert_eq!(
        cross_tile.water,
        crop(&full.water, WorldPixel { x: 250, y: 600 })
    );
    let away = prepared
        .select_view(
            step,
            WorldPixel { x: -20, y: -20 },
            ViewSize {
                width: 10,
                height: 10,
            },
        )
        .expect("pan away");
    assert!(away.line.is_empty());
    assert_eq!(
        prepared.select_view(step, top, view).expect("pan back"),
        cross_tile
    );
    let _ = prepared.select_view(74, top, view).expect("step change");
    assert_eq!(
        prepared.select_view(step, top, view).expect("step back"),
        cross_tile
    );
    let negative = prepared
        .select_view(
            step,
            WorldPixel { x: -10, y: -10 },
            ViewSize {
                width: 1_000,
                height: 1_000,
            },
        )
        .expect("negative world start");
    assert!(!negative.line.is_empty() || !negative.water.is_empty());
}

#[test]
fn network_tiers_are_hidden_before_their_step_and_present_at_it() {
    let map = fixture(all_land());
    let point = FixedPoint {
        x: 11 * 32_768 + 16_384,
        y: 21 * 32_768 + 16_384,
    };
    let network = |tier| RiverNetwork {
        show_from_step: [0, 15, 30],
        rivers: vec![RiverRiver {
            id: "tiered".into(),
            name: "Tiered".into(),
            tier,
        }],
        stretches: vec![RiverStretch {
            id: "course".into(),
            river: 0,
            kind: RiverStretchKind::River,
            points: vec![
                point,
                FixedPoint {
                    x: point.x + 50_000,
                    y: point.y,
                },
            ],
            width_first: 500,
            width_last: 500,
            ends: None,
            drawn_first: 500,
            drawn_last: 500,
        }],
    };
    let view = ViewSize {
        width: 30,
        height: 20,
    };
    let top = fixed_to_pixel(point, i64::from(STEP_SIZE[15]));
    let tier2_network = network(2);
    let mut tier2_prepared = PreparedRiverNetwork::new(&map, &tier2_network);
    assert!(
        tier2_prepared
            .select_view(
                14,
                WorldPixel {
                    x: top.x - 2,
                    y: top.y - 2
                },
                view
            )
            .unwrap()
            .line
            .is_empty()
    );
    let tier2 = tier2_prepared
        .select_view(
            15,
            WorldPixel {
                x: top.x - 2,
                y: top.y - 2,
            },
            view,
        )
        .unwrap();
    assert!(!tier2.line.is_empty());
    let top = fixed_to_pixel(point, i64::from(STEP_SIZE[30]));
    let tier3_network = network(3);
    let mut tier3_prepared = PreparedRiverNetwork::new(&map, &tier3_network);
    assert!(
        tier3_prepared
            .select_view(
                29,
                WorldPixel {
                    x: top.x - 2,
                    y: top.y - 2
                },
                view
            )
            .unwrap()
            .line
            .is_empty()
    );
    assert!(
        !tier3_prepared
            .select_view(
                30,
                WorldPixel {
                    x: top.x - 2,
                    y: top.y - 2
                },
                view
            )
            .unwrap()
            .line
            .is_empty()
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "compare all three reference looks and pixel collisions for the same fixture view"
)]
fn network_draw_lists_reproduce_the_reference_and_cache_cross_tile_banks() {
    let scenario = fixture::location_scenario();
    let loaded = &scenario.map;
    let network = loaded.river_network.as_ref().expect("format-2 network");
    let mut locations = PreparedLocations::new(
        scenario.locations.clone(),
        scenario.location_kinds.clone(),
        loaded.data.crop,
    );
    let location_palette = scenario.location_kinds.overlay_palette();
    let mut palette = location_palette.clone();
    palette.push(loaded.data.lake);
    palette.extend(loaded.data.terrains.iter().map(|terrain| terrain.solid));
    let mut prepared = PreparedRiverNetwork::new(&loaded.data, network);

    for step in [15_u8, 53] {
        let crop = loaded.data.crop;
        let origin = WorldPixel {
            x: fixed_to_pixel(
                FixedPoint {
                    x: crop.origin.x * 32_768,
                    y: crop.origin.y * 32_768,
                },
                i64::from(STEP_SIZE[usize::from(step)]),
            )
            .x - 4,
            y: fixed_to_pixel(
                FixedPoint {
                    x: crop.origin.x * 32_768,
                    y: crop.origin.y * 32_768,
                },
                i64::from(STEP_SIZE[usize::from(step)]),
            )
            .y - 4,
        };
        let view = ViewSize {
            width: 136,
            height: 80,
        };
        let locations_view = locations
            .select_view(step, origin, view)
            .expect("location view")
            .into_iter()
            .map(|([x, y], index)| [x, y, u32::from(index)])
            .collect::<Vec<_>>();
        let draw = prepared
            .select_map_view(step, origin, view, &locations_view, location_palette.len())
            .expect("network draw lists");
        let parts = prepared
            .select_view(step, origin, view)
            .expect("cached network tiles");
        if step == 53 {
            assert!(!parts.banks.is_empty(), "wide rivers have banks");
            let line = draw.rivers[0];
            let water = parts
                .water
                .iter()
                .next()
                .map(|pixel| {
                    [
                        u32::try_from(pixel.x).unwrap(),
                        u32::try_from(pixel.y).unwrap(),
                    ]
                })
                .expect("wide water pixel");
            let bank = parts
                .banks
                .keys()
                .next()
                .map(|pixel| {
                    [
                        u32::try_from(pixel.x).unwrap(),
                        u32::try_from(pixel.y).unwrap(),
                    ]
                })
                .expect("bank pixel");
            let collisions = [line, water, bank].map(|[x, y]| [x, y, 0]);
            let composed = prepared
                .select_map_view(step, origin, view, &collisions, location_palette.len())
                .expect("collision overlays compose");
            assert!(composed.indexed.contains(&[line[0], line[1], 0]));
            assert!(composed.indexed.contains(&[
                water[0],
                water[1],
                u32::try_from(location_palette.len()).unwrap()
            ]));
            assert!(composed.indexed.contains(&[bank[0], bank[1], 0]));
        }
        assert!(draw.indexed.len() <= (view.width * view.height) as usize);

        let halo_top_left = WorldPixel {
            x: origin.x - 1,
            y: origin.y - 1,
        };
        let halo_view = ViewSize {
            width: view.width + 2,
            height: view.height + 2,
        };
        let halo_draw = prepared
            .select_map_view(step, halo_top_left, halo_view, &[], location_palette.len())
            .expect("halo reference lines");
        let halo_parts = prepared
            .select_view(step, halo_top_left, halo_view)
            .expect("halo reference water");
        let lines: BTreeSet<_> = halo_draw
            .rivers
            .iter()
            .map(|[x, y]| WorldPixel {
                x: halo_top_left.x + i32::try_from(*x).unwrap(),
                y: halo_top_left.y + i32::try_from(*y).unwrap(),
            })
            .collect();
        let water: BTreeSet<_> = halo_parts
            .water
            .iter()
            .map(|pixel| WorldPixel {
                x: halo_top_left.x + pixel.x,
                y: halo_top_left.y + pixel.y,
            })
            .collect();
        let location_colours: BTreeMap<_, _> = locations_view
            .iter()
            .map(|[x, y, index]| {
                (
                    WorldPixel {
                        x: origin.x + i32::try_from(*x).unwrap(),
                        y: origin.y + i32::try_from(*y).unwrap(),
                    },
                    location_palette[*index as usize],
                )
            })
            .collect();
        let indexed: BTreeMap<_, _> = draw
            .indexed
            .iter()
            .map(|[x, y, index]| ((*x, *y), palette[*index as usize]))
            .collect();
        let owners = vec![0; (loaded.data.crop.width * loaded.data.crop.height) as usize];
        let values = [650];
        let mode = ModeInputs {
            owners: &owners,
            values: &values,
        };
        for look in [0, 1, 2] {
            for y in 0..view.height {
                for x in 0..view.width {
                    let world = WorldPixel {
                        x: origin.x + i32::try_from(x).unwrap(),
                        y: origin.y + i32::try_from(y).unwrap(),
                    };
                    let expected = match look {
                        0 => reference_colour_with_river_network(
                            &loaded.data,
                            step,
                            world,
                            &lines,
                            &water,
                            &location_colours,
                            None,
                        ),
                        1 => reference_colour_with_river_network_elevation(
                            &loaded.data,
                            step,
                            world,
                            &lines,
                            &water,
                            &location_colours,
                        ),
                        _ => reference_colour_with_river_network(
                            &loaded.data,
                            step,
                            world,
                            &lines,
                            &water,
                            &location_colours,
                            Some(&mode),
                        ),
                    }
                    .expect("reference colour");
                    let drawn = if lines.contains(&world) {
                        loaded.data.river
                    } else if let Some(colour) = indexed.get(&(x, y)) {
                        *colour
                    } else {
                        match look {
                            0 => reference_colour_with_locations(
                                &loaded.data,
                                step,
                                world,
                                &BTreeSet::new(),
                                &BTreeMap::new(),
                            ),
                            1 => reference_colour_with_elevation(
                                &loaded.data,
                                step,
                                world,
                                &BTreeSet::new(),
                                &BTreeMap::new(),
                            ),
                            _ => reference_colour_with_mode(
                                &loaded.data,
                                step,
                                world,
                                &BTreeSet::new(),
                                &BTreeMap::new(),
                                Some(&mode),
                            ),
                        }
                        .expect("base reference colour")
                    };
                    assert_eq!(drawn, expected, "look {look}, step {step} at ({x},{y})");
                }
            }
        }
    }
}

#[test]
fn banks_across_tile_edges_are_cached_once_independent_of_neighbour_order() {
    let map = wide_river_map();
    let network = cross_tile_river();
    let view = ViewSize {
        width: 8,
        height: 8,
    };
    let bank_view = WorldPixel { x: 124, y: 120 };
    let water_view = WorldPixel { x: 124, y: 128 };

    let mut target_first = PreparedRiverNetwork::new(&map, &network);
    let first = target_first
        .select_view(53, bank_view, view)
        .expect("target tile prepares first");
    assert!(target_first.bank_tiles.contains(&(53, 0, 0)));
    let first_banks = first.banks.clone();
    let neighbour_water = target_first
        .select_view(53, water_view, view)
        .expect("neighbour water is available");
    assert!(first_banks.iter().any(|(bank, _)| {
        neighbour_water.water.iter().any(|water| {
            let bank_world = WorldPixel {
                x: bank_view.x + bank.x,
                y: bank_view.y + bank.y,
            };
            let water_world = WorldPixel {
                x: water_view.x + water.x,
                y: water_view.y + water.y,
            };
            bank_world.x == water_world.x
                && (i64::from(water_world.y) - i64::from(bank_world.y)).abs() == 1
                && bank_world.y.div_euclid(128) != water_world.y.div_euclid(128)
        })
    }));

    let mut neighbour_first = PreparedRiverNetwork::new(&map, &network);
    neighbour_first
        .select_view(53, water_view, view)
        .expect("neighbour tile prepares first");
    let second = neighbour_first
        .select_view(53, bank_view, view)
        .expect("target tile prepares after its neighbour");
    assert_eq!(second.banks, first_banks);
    assert!(neighbour_first.bank_tiles.contains(&(53, 0, 0)));
}

#[test]
fn negative_start_network_view_matches_reference() {
    let map = wide_river_map();
    let network = cross_tile_river();
    let mut prepared = PreparedRiverNetwork::new(&map, &network);
    let top_left = WorldPixel { x: -8, y: -8 };
    let view = ViewSize {
        width: 160,
        height: 160,
    };
    let draw = prepared
        .select_map_view(53, top_left, view, &[], 0)
        .expect("negative-start view");
    assert!(!draw.rivers.is_empty());
    let lines: BTreeSet<_> = draw
        .rivers
        .iter()
        .map(|[x, y]| WorldPixel {
            x: top_left.x + i32::try_from(*x).unwrap(),
            y: top_left.y + i32::try_from(*y).unwrap(),
        })
        .collect();
    let halo_top_left = WorldPixel {
        x: top_left.x - 1,
        y: top_left.y - 1,
    };
    let halo_view = ViewSize {
        width: view.width + 2,
        height: view.height + 2,
    };
    let halo_draw = prepared
        .select_map_view(53, halo_top_left, halo_view, &[], 0)
        .expect("one-pixel halo reference selection");
    let halo_parts = prepared
        .select_view(53, halo_top_left, halo_view)
        .expect("one-pixel halo water selection");
    let reference_lines: BTreeSet<_> = halo_draw
        .rivers
        .iter()
        .map(|[x, y]| WorldPixel {
            x: halo_top_left.x + i32::try_from(*x).unwrap(),
            y: halo_top_left.y + i32::try_from(*y).unwrap(),
        })
        .collect();
    let reference_water: BTreeSet<_> = halo_parts
        .water
        .iter()
        .map(|pixel| WorldPixel {
            x: halo_top_left.x + pixel.x,
            y: halo_top_left.y + pixel.y,
        })
        .collect();
    let indexed: BTreeMap<_, _> = draw
        .indexed
        .iter()
        .map(|[x, y, index]| ((*x, *y), *index))
        .collect();
    for y in 0..view.height {
        for x in 0..view.width {
            let world = WorldPixel {
                x: top_left.x + i32::try_from(x).unwrap(),
                y: top_left.y + i32::try_from(y).unwrap(),
            };
            let expected = reference_colour_with_river_network(
                &map,
                53,
                world,
                &reference_lines,
                &reference_water,
                &BTreeMap::new(),
                None,
            )
            .unwrap();
            let drawn = if lines.contains(&world) {
                map.river
            } else if let Some(index) = indexed.get(&(x, y)) {
                [map.lake, map.terrains[0].solid][*index as usize]
            } else {
                reference_colour_with_locations(&map, 53, world, &BTreeSet::new(), &BTreeMap::new())
                    .unwrap()
            };
            assert_eq!(drawn, expected, "negative-start pixel ({x},{y})");
        }
    }
}

#[test]
fn all_water_view_fills_exact_indexed_bound() {
    let map = wide_river_map();
    let network = cross_tile_river();
    let mut prepared = PreparedRiverNetwork::new(&map, &network);
    let top_left = WorldPixel { x: 100, y: 120 };
    let view = ViewSize {
        width: 64,
        height: 32,
    };
    let parts = prepared
        .select_view(53, top_left, view)
        .expect("coverage around the wide course");
    let water_top_left = parts
        .water
        .iter()
        .find(|pixel| {
            [(1, 0), (0, 1), (1, 1)].into_iter().all(|(dx, dy)| {
                parts.water.contains(&WorldPixel {
                    x: pixel.x + dx,
                    y: pixel.y + dy,
                })
            })
        })
        .map(|pixel| WorldPixel {
            x: top_left.x + pixel.x,
            y: top_left.y + pixel.y,
        })
        .expect("fixture has a 2x2 all-water patch");
    let all_water = prepared
        .select_map_view(
            53,
            water_top_left,
            ViewSize {
                width: 2,
                height: 2,
            },
            &[],
            0,
        )
        .expect("2x2 all-water view");
    assert!(all_water.rivers.is_empty());
    assert_eq!(all_water.indexed.len(), 4);
    assert!(all_water.indexed.iter().all(|pixel| pixel[2] == 0));
}

#[test]
fn constant_width_at_the_threshold_is_water_and_one_unit_under_is_line() {
    let map = fixture(all_land());
    let point = FixedPoint {
        x: 10 * 32_768 + 16_384,
        y: 21 * 32_768 + 16_384,
    };
    let network = |width| RiverNetwork {
        show_from_step: [0, 0, 0],
        rivers: vec![RiverRiver {
            id: "main".into(),
            name: "Main".into(),
            tier: 1,
        }],
        stretches: vec![RiverStretch {
            id: "course".into(),
            river: 0,
            kind: RiverStretchKind::River,
            points: vec![
                point,
                FixedPoint {
                    x: point.x + 120_000,
                    y: point.y,
                },
            ],
            width_first: width,
            width_last: width,
            ends: None,
            drawn_first: width,
            drawn_last: width,
        }],
    };
    let top = fixed_to_pixel(point, i64::from(STEP_SIZE[53]));
    let origin = WorldPixel {
        x: top.x - 4,
        y: top.y - 4,
    };
    let view = ViewSize {
        width: 32,
        height: 16,
    };
    let narrow_network = network(7_484);
    let mut narrow = PreparedRiverNetwork::new(&map, &narrow_network);
    let under = narrow.select_view(53, origin, view).unwrap();
    assert!(!under.line.is_empty());
    assert!(under.water.is_empty());
    let wide_network = network(7_485);
    let mut wide = PreparedRiverNetwork::new(&map, &wide_network);
    let at = wide.select_view(53, origin, view).unwrap();
    assert!(!at.water.is_empty());
    assert!(at.line.is_empty());
}

#[test]
fn each_segment_distance_case_includes_the_integer_boundary_and_excludes_one_more_unit() {
    let map = fixture(all_land());
    let pixel = pixels_for_cell(Cell { x: 11, y: 21 }, 53)[10];
    let c = sample_point(pixel, 53).expect("sample centre");
    let observe = |points: Vec<FixedPoint>| {
        let network = RiverNetwork {
            show_from_step: [0, 0, 0],
            rivers: vec![RiverRiver {
                id: "main".into(),
                name: "Main".into(),
                tier: 1,
            }],
            stretches: vec![RiverStretch {
                id: "course".into(),
                river: 0,
                kind: RiverStretchKind::River,
                points,
                width_first: 7_485,
                width_last: 7_485,
                ends: None,
                drawn_first: 7_485,
                drawn_last: 7_485,
            }],
        };
        let mut prepared = PreparedRiverNetwork::new(&map, &network);
        let parts = prepared
            .select_view(
                53,
                WorldPixel {
                    x: pixel.x - 1,
                    y: pixel.y - 1,
                },
                ViewSize {
                    width: 3,
                    height: 3,
                },
            )
            .unwrap();
        parts.water.contains(&WorldPixel { x: 1, y: 1 })
    };
    let segment_case = |distance: i32, case: u8| {
        let (a, b) = match case {
            0 => (
                FixedPoint {
                    x: c.x - 20_000,
                    y: c.y + distance,
                },
                FixedPoint {
                    x: c.x + 20_000,
                    y: c.y + distance,
                },
            ),
            1 => (
                FixedPoint {
                    x: c.x - distance - 10_000,
                    y: c.y,
                },
                FixedPoint {
                    x: c.x - distance,
                    y: c.y,
                },
            ),
            _ => (
                FixedPoint {
                    x: c.x + distance,
                    y: c.y,
                },
                FixedPoint {
                    x: c.x + distance + 10_000,
                    y: c.y,
                },
            ),
        };
        observe(vec![a, b])
    };
    for case in 0..3 {
        assert!(
            segment_case(3_742, case),
            "inside boundary for distance case {case}"
        );
        assert!(
            !segment_case(3_743, case),
            "outside boundary for distance case {case}"
        );
    }
}

#[test]
fn nearest_points_just_before_and_after_the_turn_split_line_from_water() {
    let map = fixture(all_land());
    let before = pixels_for_cell(Cell { x: 11, y: 21 }, 53)
        .into_iter()
        .find(|pixel| {
            matches!(
                surface_for_pixel(&map, 53, *pixel),
                Some(Surface::Land { .. })
            ) && matches!(
                surface_for_pixel(
                    &map,
                    53,
                    WorldPixel {
                        x: pixel.x + 1,
                        y: pixel.y
                    }
                ),
                Some(Surface::Land { .. })
            )
        })
        .expect("adjacent sample pair in crop");
    let c = sample_point(before, 53).unwrap();
    let next = WorldPixel {
        x: before.x + 1,
        y: before.y,
    };
    let second_sample_x = sample_point(next, 53).unwrap().x;
    let total = 100_000_i64;
    let turn = (5_485 * total + 9_999) / 10_000;
    let a = FixedPoint {
        x: i32::try_from(i64::from(c.x) + i64::from(second_sample_x - c.x) / 2 - turn).unwrap(),
        y: c.y,
    };
    let b = FixedPoint {
        x: a.x + i32::try_from(total).unwrap(),
        y: a.y,
    };
    let network = RiverNetwork {
        show_from_step: [0, 0, 0],
        rivers: vec![RiverRiver {
            id: "main".into(),
            name: "Main".into(),
            tier: 1,
        }],
        stretches: vec![RiverStretch {
            id: "taper".into(),
            river: 0,
            kind: RiverStretchKind::River,
            points: vec![a, b],
            width_first: 2_000,
            width_last: 12_000,
            ends: None,
            drawn_first: 2_000,
            drawn_last: 12_000,
        }],
    };
    let origin = WorldPixel {
        x: before.x - 1,
        y: before.y - 1,
    };
    let mut prepared = PreparedRiverNetwork::new(&map, &network);
    let parts = prepared
        .select_view(
            53,
            origin,
            ViewSize {
                width: 4,
                height: 3,
            },
        )
        .unwrap();
    assert!(
        !parts.water.contains(&WorldPixel { x: 1, y: 1 }),
        "nearest point just above m* is not water"
    );
    assert!(
        parts.water.contains(&WorldPixel { x: 2, y: 1 }),
        "nearest point just below the downstream side of m* is water"
    );
}

#[test]
fn taper_places_the_line_cut_at_the_turn_and_widens_water_downstream() {
    let mut map = fixture(all_land());
    map.crop.width = 16;
    map.cells = vec![
        Surface::Land {
            terrain: 0,
            usable: true
        };
        16 * 3
    ];
    map.elevations = vec![11_004; 16 * 3];
    map.fertility = vec![500; 16 * 3];
    map.climate_temperature_tenths = vec![100; 16 * 3];
    map.climate_gdd = vec![1000; 16 * 3];
    map.climate_rainfall_mm = vec![1000; 16 * 3];
    map.glacier = vec![false; 16 * 3];
    map.hillside = vec![0; 16 * 3];
    let a = FixedPoint {
        x: 10 * 32_768 + 16_384,
        y: 21 * 32_768 + 16_384,
    };
    let b = FixedPoint {
        x: a.x + 5 * 32_768,
        y: a.y,
    };
    let total = i64::from(b.x - a.x);
    let turn = (5_485 * total + 27_999) / 28_000;
    let x_cut = FixedPoint {
        x: a.x + i32::try_from(turn).unwrap(),
        y: a.y,
    };
    let network = RiverNetwork {
        show_from_step: [0, 0, 0],
        rivers: vec![RiverRiver {
            id: "main".into(),
            name: "Main".into(),
            tier: 1,
        }],
        stretches: vec![RiverStretch {
            id: "taper".into(),
            river: 0,
            kind: RiverStretchKind::River,
            points: vec![a, b],
            width_first: 2_000,
            width_last: 30_000,
            ends: None,
            drawn_first: 2_000,
            drawn_last: 30_000,
        }],
    };
    let p = i64::from(STEP_SIZE[53]);
    let target = fixed_to_pixel(x_cut, p);
    let origin = WorldPixel {
        x: fixed_to_pixel(a, p).x - 2,
        y: fixed_to_pixel(a, p).y - 3,
    };
    let view = ViewSize {
        width: 48,
        height: 12,
    };
    let mut prepared = PreparedRiverNetwork::new(&map, &network);
    let parts = prepared.select_view(53, origin, view).unwrap();
    let local_cut = WorldPixel {
        x: target.x - origin.x,
        y: target.y - origin.y,
    };
    assert!(
        parts.line.contains(&local_cut) || parts.water.contains(&local_cut),
        "the line/water transition reaches X's pixel"
    );
    let turn_x = target.x - origin.x + 3;
    let end_x = fixed_to_pixel(b, p).x - origin.x - 2;
    let rows_at = |x| parts.water.iter().filter(|pixel| pixel.x == x).count();
    assert!(
        rows_at(end_x) > rows_at(turn_x),
        "water is wider at the end than beside the turning position: end={}, turn={}",
        rows_at(end_x),
        rows_at(turn_x)
    );
}

#[test]
fn river_segments_stop_at_lake_shores_with_and_without_shore_detail() {
    let mut map = fixture(all_land());
    map.cells[4] = Surface::Lake;
    let a = FixedPoint {
        x: 10 * 32_768 + 16_384,
        y: 21 * 32_768 + 16_384,
    };
    let b = FixedPoint {
        x: 12 * 32_768 + 16_384,
        y: a.y,
    };
    let network = RiverNetwork {
        show_from_step: [0, 0, 0],
        rivers: vec![RiverRiver {
            id: "main".into(),
            name: "Main".into(),
            tier: 1,
        }],
        stretches: vec![RiverStretch {
            id: "lake-crossing".into(),
            river: 0,
            kind: RiverStretchKind::River,
            points: vec![a, b],
            width_first: 10_000,
            width_last: 10_000,
            ends: None,
            drawn_first: 10_000,
            drawn_last: 10_000,
        }],
    };
    for step in [13_u8, 53] {
        let p = i64::from(STEP_SIZE[usize::from(step)]);
        let start = fixed_to_pixel(a, p);
        let top = WorldPixel {
            x: start.x - 4,
            y: start.y - 6,
        };
        let view = ViewSize {
            width: 32,
            height: 16,
        };
        let mut prepared = PreparedRiverNetwork::new(&map, &network);
        let parts = prepared.select_view(step, top, view).unwrap();
        let pixels = parts
            .line
            .union(&parts.water)
            .copied()
            .collect::<BTreeSet<_>>();
        assert!(!pixels.is_empty(), "coverage at step {step}");
        let lake_pixel_x = fixed_to_pixel(
            FixedPoint {
                x: 11 * 32_768 + 16_384,
                y: a.y,
            },
            p,
        )
        .x;
        let mut left_shore = false;
        let mut right_shore = false;
        for relative in &pixels {
            let world = WorldPixel {
                x: relative.x + top.x,
                y: relative.y + top.y,
            };
            let sample = jittered_point(sample_point(world, step).unwrap(), step).unwrap();
            assert_ne!(cell_of(sample), Cell { x: 11, y: 21 });
            assert!(matches!(
                surface_for_pixel(&map, step, world),
                Some(Surface::Land { .. })
            ));
            left_shore |= world.x < lake_pixel_x;
            right_shore |= world.x > lake_pixel_x;
        }
        if step >= 53 {
            assert!(
                left_shore && right_shore,
                "the river meets both lake shores at step {step}"
            );
        } else {
            assert!(
                !pixels.is_empty(),
                "the coarse line remains on land without shore detail"
            );
        }
    }
}

#[test]
fn bend_outer_corner_is_filled_once_by_the_segment_ending_at_the_bend() {
    let map = fixture(all_land());
    let pixel = pixels_for_cell(Cell { x: 11, y: 21 }, 75)[10];
    let c = sample_point(pixel, 75).unwrap();
    let bend = FixedPoint {
        x: c.x - 2_700,
        y: c.y + 2_700,
    };
    let a = FixedPoint {
        x: bend.x - 20_000,
        y: bend.y,
    };
    let end = FixedPoint {
        x: bend.x,
        y: bend.y + 20_000,
    };
    let network = |points| RiverNetwork {
        show_from_step: [0, 0, 0],
        rivers: vec![RiverRiver {
            id: "main".into(),
            name: "Main".into(),
            tier: 1,
        }],
        stretches: vec![RiverStretch {
            id: "bend".into(),
            river: 0,
            kind: RiverStretchKind::River,
            points,
            width_first: 8_000,
            width_last: 8_000,
            ends: None,
            drawn_first: 8_000,
            drawn_last: 8_000,
        }],
    };
    let origin = WorldPixel {
        x: pixel.x - 1,
        y: pixel.y - 1,
    };
    let view = ViewSize {
        width: 3,
        height: 3,
    };
    let bent_network = network(vec![a, bend, end]);
    let mut corner_raster = PreparedRiverNetwork::new(&map, &bent_network);
    let coverage = corner_raster.select_view(75, origin, view).unwrap();
    assert!(coverage.water.contains(&WorldPixel { x: 1, y: 1 }));
    let first_segment_network = network(vec![a, bend]);
    let mut first_segment = PreparedRiverNetwork::new(&map, &first_segment_network);
    let single = first_segment.select_view(75, origin, view).unwrap();
    assert!(single.water.contains(&WorldPixel { x: 1, y: 1 }));
}

#[test]
fn diagonal_taper_keeps_the_review_witness_pixel_when_a_collinear_point_is_added() {
    let mut map = fixture(vec![
        Surface::Land {
            terrain: 0,
            usable: true
        };
        24 * 16
    ]);
    map.crop = Crop {
        origin: Cell {
            x: 24_100,
            y: 14_700,
        },
        width: 24,
        height: 16,
    };
    let a = FixedPoint {
        x: 790_108_008,
        y: 481_991_278,
    };
    let b = FixedPoint {
        x: 790_128_024,
        y: 482_001_286,
    };
    let s = FixedPoint {
        x: 790_124_008,
        y: 481_999_278,
    };
    let make = |points| RiverNetwork {
        show_from_step: [0, 0, 0],
        rivers: vec![RiverRiver {
            id: "main".into(),
            name: "Main".into(),
            tier: 1,
        }],
        stretches: vec![RiverStretch {
            id: "course".into(),
            river: 0,
            kind: RiverStretchKind::River,
            points,
            width_first: 2_000,
            width_last: 12_000,
            ends: None,
            drawn_first: 2_000,
            drawn_last: 12_000,
        }],
    };
    let top = WorldPixel {
        x: 158_335,
        y: 96_590,
    };
    let view = ViewSize {
        width: 12,
        height: 12,
    };
    let plain_network = make(vec![a, b]);
    let mut plain = PreparedRiverNetwork::new(&map, &plain_network);
    let original = plain
        .select_view(53, top, view)
        .expect("original diagonal raster");
    let subdivided_network = make(vec![a, s, b]);
    let mut subdivided = PreparedRiverNetwork::new(&map, &subdivided_network);
    let with_point = subdivided
        .select_view(53, top, view)
        .expect("subdivided diagonal raster");
    assert_eq!(original, with_point);
    for points in [
        vec![
            a,
            FixedPoint {
                x: 790_118_016,
                y: 481_996_282,
            },
            b,
        ],
        vec![
            a,
            FixedPoint {
                x: 790_116_008,
                y: 481_995_278,
            },
            FixedPoint {
                x: 790_124_008,
                y: 481_999_278,
            },
            b,
        ],
    ] {
        let network = make(points);
        let mut candidate = PreparedRiverNetwork::new(&map, &network);
        assert_eq!(
            original,
            candidate
                .select_view(53, top, view)
                .expect("point-subdivided raster")
        );
    }
    assert!(
        original.water.contains(&WorldPixel { x: 5, y: 3 }),
        "the translated review sample remains dark river water"
    );
}

#[test]
fn river_lines_deduplicate_crossings_and_view_selection_is_bounded() {
    let lines = [
        RiverLine {
            points: vec![fp_pixel(0, 2, 50), fp_pixel(4, 2, 50)],
        },
        RiverLine {
            points: vec![fp_pixel(2, 0, 50), fp_pixel(2, 4, 50)],
        },
    ];
    let coverage = RiverCoverage::prepare(&lines, 50).expect("valid step");
    assert_eq!(coverage.pixels().len(), 9);
    let visible = coverage.select(
        WorldPixel { x: 2, y: 1 },
        ViewSize {
            width: 2,
            height: 3,
        },
    );
    assert_eq!(visible.len(), 4);
    assert!(
        visible
            .iter()
            .all(|p| (2..4).contains(&p.x) && (1..4).contains(&p.y))
    );
    assert!(visible.contains(&WorldPixel { x: 2, y: 1 }));
    assert!(visible.contains(&WorldPixel { x: 3, y: 2 }));
    assert!(!visible.contains(&WorldPixel { x: 4, y: 2 }));
    assert!(visible.len() <= 2 * 3);
    assert!(RiverCoverage::prepare(&[], 76).is_none());
}

#[test]
fn prepared_rivers_reuse_step_coverage_across_pans_and_rebuild_on_step_change() {
    let lines = vec![RiverLine {
        points: vec![fp_pixel(0, 2, 50), fp_pixel(8, 2, 50)],
    }];
    let mut prepared = PreparedRivers::new(lines);
    let first = prepared.coverage(50).expect("coverage at step 50") as *const _;
    let first_view = prepared
        .select_view(
            50,
            WorldPixel { x: 1, y: 1 },
            ViewSize {
                width: 4,
                height: 3,
            },
        )
        .expect("first view");
    assert_eq!(first_view, vec![[0, 1], [1, 1], [2, 1], [3, 1]]);
    let panned_view = prepared
        .select_view(
            50,
            WorldPixel { x: 3, y: 1 },
            ViewSize {
                width: 4,
                height: 3,
            },
        )
        .expect("panned view");
    assert_eq!(panned_view, vec![[0, 1], [1, 1], [2, 1], [3, 1]]);
    let reused = prepared.coverage(50).expect("retained coverage") as *const _;
    assert_eq!(first, reused, "panning must not rasterize the lines again");

    let other_step = prepared.coverage(51).expect("coverage at next step") as *const _;
    assert_ne!(first, other_step, "each zoom step has its own coverage");
    assert!(prepared.coverage(76).is_none());
}

#[test]
fn great_circle_distance_is_the_haversine_on_the_projections_sphere_in_whole_metres() {
    let at = |latitude: f64, longitude: f64| project(latitude, longitude).expect("projectable");
    // One degree of longitude on the equator is R x pi / 180 = 111,319.49 m; the positions are fixed-point (2.5 cm), so the whole
    // metres may differ by one from the exact value.
    for (to_latitude, to_longitude) in [(0.0, 1.0), (1.0, 0.0)] {
        let metres =
            great_circle_metres(at(0.0, 0.0), at(to_latitude, to_longitude)).expect("a distance");
        assert!((111_319..=111_320).contains(&metres), "{metres}");
    }
    // The same point, and symmetry.
    assert_eq!(great_circle_metres(at(53.3, -6.3), at(53.3, -6.3)), Some(0));
    let (dublin, london) = (at(53.3498, -6.2603), at(51.5074, -0.1278));
    let there = great_circle_metres(dublin, london).expect("a distance");
    assert_eq!(great_circle_metres(london, dublin), Some(there));
    // About 463 km on this sphere (the radius of the projection, not the Earth's mean).
    assert!((462_000..=465_000).contains(&there), "{there}");
}
