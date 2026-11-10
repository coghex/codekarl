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
