use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use png::{BitDepth, ColorType};

use crate::{Surface, TerrainKind, load_map};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn copy() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("ck-map-fixture-{}-{id}", std::process::id()));
        fs::create_dir(&path).expect("create temp fixture directory");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/valid");
        for name in [
            "map.toml",
            "terrain.toml",
            "rivers.toml",
            "terrain.png",
            "fertility.png",
            "elevation.png",
            "climate_temperature.png",
            "climate_half_range.png",
            "climate_sea_nearness.png",
            "climate_gdd.png",
            "climate_rainfall.png",
            "climate_glacier.png",
        ] {
            fs::copy(source.join(name), path.join(name)).expect("copy valid fixture");
        }
        Self(path)
    }

    fn text(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name)).expect("read fixture text")
    }

    fn replace(&self, name: &str, before: &str, after: &str) {
        let source = self.text(name);
        assert!(
            source.contains(before),
            "fixture text did not contain {before:?}"
        );
        fs::write(self.0.join(name), source.replacen(before, after, 1))
            .expect("write fixture text");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn valid_fixture_loads_climate_fertility_and_preserves_packed_bytes() {
    let fixture = Fixture::copy();
    let loaded = load_map(&fixture.0).expect("valid fixture loads");
    assert_eq!(loaded.data.crop.origin.x, 23_882);
    assert_eq!(loaded.data.crop.origin.y, 23_882);
    assert_eq!((loaded.data.crop.width, loaded.data.crop.height), (4, 4));
    assert_eq!(loaded.data.elevations[0], 10_000);
    assert_eq!(loaded.data.elevations[5], 11_000);
    assert_eq!(loaded.data.elevations[6], 12_000);
    assert_eq!(loaded.data.elevations[10], 500);
    assert_eq!(loaded.data.fertility[6], 496);
    assert_eq!(loaded.data.fertility[10], 500);
    assert_eq!(loaded.data.climate_temperature_tenths, [100; 16]);
    assert_eq!(loaded.data.climate_gdd, [1000; 16]);
    assert_eq!(loaded.data.climate_rainfall_mm, [1000; 16]);
    assert_eq!(
        loaded.data.cells,
        [
            Surface::Sea { depth: 1_000 },
            Surface::Sea { depth: 1_000 },
            Surface::Sea { depth: 1_000 },
            Surface::Sea { depth: 1_000 },
            Surface::Sea { depth: 1_000 },
            Surface::Lake,
            Surface::Land {
                terrain: 0,
                usable: true
            },
            Surface::Sea { depth: 1_000 },
            Surface::Sea { depth: 1_000 },
            Surface::Sea { depth: 1_000 },
            Surface::Land {
                terrain: 1,
                usable: true
            },
            Surface::Sea { depth: 1_000 },
            Surface::Sea { depth: 1_000 },
            Surface::Sea { depth: 1_000 },
            Surface::Sea { depth: 1_000 },
            Surface::Sea { depth: 1_000 },
        ]
    );
    assert_eq!(
        loaded
            .terrain_types
            .iter()
            .map(|terrain| terrain.kind)
            .collect::<Vec<_>>(),
        [
            TerrainKind::Sea,
            TerrainKind::Lake,
            TerrainKind::Land,
            TerrainKind::Mountain,
        ]
    );
    // Grid A: the terrain index, fertility ÷ 4 (255 for a glacier) and the hillside strength + 128.
    assert_eq!(loaded.grid_a.pixels[6], [2, 124, 128, 255]);
    // The mountain below a 1000 m neighbour to its north faces away from the light: the strongest shadow, -15.
    assert_eq!(loaded.grid_a.pixels[10], [3, 125, 128 - 15, 255]);
    for (pixel, strength) in loaded.grid_a.pixels.iter().zip(&loaded.data.hillside) {
        assert_eq!(i16::from(pixel[2]), i16::from(*strength) + 128);
    }
    assert_eq!(loaded.grid_b.pixels[0], [0x27, 0x10, 0, 255]);
    assert_eq!(loaded.grid_b.pixels[5], [0x2a, 0xf8, 0, 255]);
    assert_eq!(loaded.grid_b.pixels[6], [0x2e, 0xe0, 0, 255]);
    assert_eq!(loaded.grid_b.pixels[10], [0x01, 0xf4, 0, 255]);
    assert_eq!(loaded.rivers.len(), 1);
    assert_eq!(loaded.rivers[0].points.len(), 2);
    assert!(loaded.river_network.is_none());
}

#[test]
fn format_two_loads_a_validated_network_and_preserves_legacy_separately() {
    let fixture = Fixture::copy();
    write_png(
        &fixture.0.join("terrain.png"),
        ColorType::Rgb,
        BitDepth::Eight,
        &[0, 0, 255].repeat(16),
        None,
    );
    write_png(
        &fixture.0.join("fertility.png"),
        ColorType::Grayscale,
        BitDepth::Eight,
        &[0; 16],
        None,
    );
    fs::write(
        fixture.0.join("rivers.toml"),
        r#"format = 2

[tiers]
show_from_step = [0, 15, 30]

[[river]]
id = "main"
name = "Main River"
tier = 1

[[stretch]]
id = "main-course"
river = "main"
kind = "river"
width_m = [500.0, 900.0]
ends = "sea"
points = [[0.0, 0.0], [0.0, 0.001]]
note = "accepted and dropped"
"#,
    )
    .expect("write format-2 network");
    let loaded = load_map(&fixture.0).expect("format-2 network loads");
    assert!(loaded.rivers.is_empty());
    let network = loaded.river_network.expect("network is present");
    assert_eq!(network.show_from_step, [0, 15, 30]);
    assert_eq!(network.rivers[0].name, "Main River");
    assert_eq!(network.stretches[0].points.len(), 2);
    assert!(network.stretches[0].width_first > 0);
    assert!(network.stretches[0].width_last > network.stretches[0].width_first);
}

fn format_two_fixture(contents: &str) -> Fixture {
    let fixture = Fixture::copy();
    write_png(
        &fixture.0.join("terrain.png"),
        ColorType::Rgb,
        BitDepth::Eight,
        &[0, 0, 255].repeat(16),
        None,
    );
    write_png(
        &fixture.0.join("fertility.png"),
        ColorType::Grayscale,
        BitDepth::Eight,
        &[0; 16],
        None,
    );
    fs::write(fixture.0.join("rivers.toml"), contents).expect("write format-2 network");
    fixture
}

fn format_two_fixture_with_terrain(contents: &str, first_pixel: [u8; 3]) -> Fixture {
    let fixture = format_two_fixture(contents);
    let mut terrain = [0, 0, 255].repeat(16);
    terrain[6 * 3..6 * 3 + 3].copy_from_slice(&first_pixel);
    write_png(
        &fixture.0.join("terrain.png"),
        ColorType::Rgb,
        BitDepth::Eight,
        &terrain,
        None,
    );
    fixture
}

const VALID_NETWORK: &str = r#"format = 2
[tiers]
show_from_step = [0, 15, 30]
[[river]]
id = "main"
name = "Main River"
tier = 1
[[stretch]]
id = "course"
river = "main"
kind = "river"
width_m = [500.0, 900.0]
ends = "sea"
points = [[0.0, 0.0], [0.0, 0.001]]
note = "accepted and dropped"
"#;

fn replace_network_assignment(source: &str, replacement: &str) -> String {
    let key = replacement.split_once(" = ").expect("assignment key").0;
    let mut changed = false;
    let lines = source
        .lines()
        .map(|line| {
            if !changed && line.starts_with(&format!("{key} = ")) {
                changed = true;
                replacement.to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if changed {
        lines
    } else {
        format!("{lines}\n{replacement}\n")
    }
}

#[test]
fn format_two_rejections_name_the_stretch_id_key_and_problem() {
    let cases = [
        (
            "kind = \"canal\"",
            "stretch[0] (course).kind",
            "must be river",
        ),
        (
            "width_m = [500.0]",
            "stretch[0] (course).width_m",
            "requires two finite",
        ),
        (
            "width_m = [0.0, 900.0]",
            "stretch[0] (course).width_m",
            "requires two finite",
        ),
        (
            "width_m = [-1.0, 900.0]",
            "stretch[0] (course).width_m",
            "requires two finite",
        ),
        (
            "width_m = [nan, 900.0]",
            "stretch[0] (course).width_m",
            "requires two finite",
        ),
        (
            "width_m = [500.0, 10001.0]",
            "stretch[0] (course).width_m",
            "requires two finite",
        ),
        (
            "points = [[0.0, 0.0]]",
            "stretch[0] (course).points",
            "at least two",
        ),
        (
            "points = [[0.0, 0.0], [0.0, 40.0]]",
            "stretch[0] (course).points[1]",
            "outside crop",
        ),
        (
            "points = [[0.0, 0.0], [0.0, 0.0]]",
            "stretch[0] (course).points[1]",
            "same fixed point",
        ),
        (
            "ends = \"absent\"",
            "stretch[0] (course).ends",
            "names no stretch",
        ),
        ("ends = \"course\"", "stretch[0] (course).ends", "itself"),
        ("mystery = 7", "rivers.toml", "unknown field"),
    ];
    for (replacement, location, problem) in cases {
        let fixture = format_two_fixture(&replace_network_assignment(VALID_NETWORK, replacement));
        assert_error(&fixture, location, problem);
    }
    let bad_river = format!(
        "{VALID_NETWORK}\n[[stretch]]\nid = \"bad\"\nriver = \"unknown\"\nkind = \"river\"\nwidth_m = [1.0, 2.0]\nends = \"sea\"\npoints = [[0.0, 0.0], [0.0, 0.001]]\n"
    );
    let fixture = format_two_fixture(&bad_river);
    assert_error(&fixture, "stretch[1] (bad).river", "unknown river");
}

#[test]
fn format_two_river_and_tier_keys_are_checked() {
    let cases = [
        ("id = \"Main\"", "river[0].id", "lowercase"),
        (
            &format!("id = \"{}\"", "a".repeat(49)),
            "river[0].id",
            "1..=48",
        ),
        ("id = \"sea\"", "river[0].id", "not sea"),
        ("tier = 0", "river[0].tier", "1, 2 or 3"),
        ("tier = 4", "river[0].tier", "1, 2 or 3"),
        (
            "show_from_step = [0, 75]",
            "tiers.show_from_step",
            "three nondecreasing",
        ),
        (
            "show_from_step = [0, 76, 76]",
            "tiers.show_from_step",
            "three nondecreasing",
        ),
        (
            "show_from_step = [0, 30, 15]",
            "tiers.show_from_step",
            "three nondecreasing",
        ),
    ];
    for (replacement, location, problem) in cases {
        let fixture = format_two_fixture(&replace_network_assignment(VALID_NETWORK, replacement));
        assert_error(&fixture, location, problem);
    }
    let fixture = format_two_fixture(&VALID_NETWORK.replace("tier = 1", "tier = 1\nextra = 1"));
    assert_error(&fixture, "in `river`", "unknown field `extra`");
    let fixture = format_two_fixture(
        &VALID_NETWORK.replace("note = \"accepted and dropped\"", "mystery = 1"),
    );
    assert_error(&fixture, "in `stretch`", "unknown field `mystery`");
    let fixture = format_two_fixture(&format!(
        "{VALID_NETWORK}\n[[river]]\nid = \"main\"\nname = \"Other\"\ntier = 1\n"
    ));
    assert_error(&fixture, "river[1].id", "duplicate river id");
    let fixture = format_two_fixture(&format!(
        "{VALID_NETWORK}\n[[river]]\nid = \"other\"\nname = \"mAIN rIVER\"\ntier = 1\n\n[[stretch]]\nid = \"other-course\"\nriver = \"other\"\nkind = \"river\"\nwidth_m = [1.0, 2.0]\nends = \"sea\"\npoints = [[0.0, 0.0], [0.0, 0.001]]\n"
    ));
    assert_error(&fixture, "river[1].name", "unique ignoring case");
    let fixture = format_two_fixture(&format!(
        "{VALID_NETWORK}\n[[stretch]]\nid = \"course\"\nriver = \"main\"\nkind = \"river\"\nwidth_m = [1.0, 2.0]\nends = \"sea\"\npoints = [[0.0, 0.0], [0.0, 0.001]]\n"
    ));
    assert_error(&fixture, "stretch[1] (course).id", "duplicate stretch id");
    for invalid in [
        "id = \"Course\"".to_owned(),
        format!("id = \"{}\"", "a".repeat(49)),
        "id = \"sea\"".to_owned(),
    ] {
        let fixture = format_two_fixture(&VALID_NETWORK.replace("id = \"course\"", &invalid));
        assert_error(&fixture, "stretch[0] (", "must be 1..=48");
    }
    let fixture = format_two_fixture(&format!(
        "{VALID_NETWORK}\n[[river]]\nid = \"empty\"\nname = \"Empty\"\ntier = 1\n"
    ));
    assert_error(&fixture, "river[1].id", "river has no stretch");
}

#[test]
fn format_two_network_joins_cycles_and_confluences_are_checked() {
    let cases = [
        (
            VALID_NETWORK.replace("ends = \"sea\"", "ends = \"missing\""),
            "stretch[0] (course).ends",
            "names no stretch",
        ),
        (
            VALID_NETWORK.replace("ends = \"sea\"", "ends = \"down\"")
                + "\n[[stretch]]\nid = \"down\"\nriver = \"main\"\nkind = \"river\"\nwidth_m = [500.0, 900.0]\nends = \"sea\"\npoints = [[0.0, 0.0011], [0.0, 0.0015]]\n",
            "stretch[0] (course).ends",
            "join must equal",
        ),
        (
            VALID_NETWORK
                .replace("ends = \"sea\"", "ends = \"down\"")
                .replace(
                    "points = [[0.0, 0.0], [0.0, 0.001]]",
                    "points = [[0.0, 0.0], [0.0, 0.002]]",
                )
                + "\n[[stretch]]\nid = \"down\"\nriver = \"main\"\nkind = \"river\"\nwidth_m = [500.0, 900.0]\nends = \"sea\"\npoints = [[0.0, 0.001], [0.0, 0.003]]\n",
            "stretch[0] (course).ends",
            "join must equal",
        ),
        (
            VALID_NETWORK.replace("ends = \"sea\"", "ends = \"down\"")
                + "\n[[stretch]]\nid = \"down\"\nriver = \"main\"\nkind = \"river\"\nwidth_m = [500.0, 900.0]\nends = \"course\"\npoints = [[0.0, 0.001], [0.0, 0.0]]\n",
            "stretch[0] (course).ends",
            "cycle",
        ),
        (
            VALID_NETWORK.replace("ends = \"sea\"", "ends = \"down\"")
                + "\n[[stretch]]\nid = \"down\"\nriver = \"main\"\nkind = \"river\"\nwidth_m = [500.0, 900.0]\nends = \"third\"\npoints = [[0.0, 0.001], [0.0, 0.002]]\n\n[[stretch]]\nid = \"third\"\nriver = \"main\"\nkind = \"river\"\nwidth_m = [500.0, 900.0]\nends = \"course\"\npoints = [[0.0, 0.002], [0.0, 0.0]]\n",
            "stretch[0] (course).ends",
            "cycle",
        ),
    ];
    for (text, location, detail) in cases {
        let fixture = format_two_fixture(&text);
        assert_error(&fixture, location, detail);
    }

    let confluence = VALID_NETWORK.replace("ends = \"sea\"", "ends = \"down\"")
        + "\n[[stretch]]\nid = \"down\"\nriver = \"main\"\nkind = \"river\"\nwidth_m = [500.0, 900.0]\nends = \"sea\"\npoints = [[0.0, 0.001], [0.0, 0.0]]\n\n[[stretch]]\nid = \"branch\"\nriver = \"main\"\nkind = \"river\"\nwidth_m = [500.0, 900.0]\nends = \"down\"\npoints = [[0.0, 0.0015], [0.0, 0.001]]\n";
    let fixture = format_two_fixture(&confluence);
    let network = load_map(&fixture.0)
        .expect("two upstream stretches may join one downstream")
        .river_network
        .unwrap();
    assert!(
        network
            .stretches
            .iter()
            .all(|stretch| stretch.drawn_last >= stretch.drawn_first)
    );
}

#[test]
fn format_two_sea_ends_and_lough_midpoints_are_validated_by_surface() {
    for colour in [[0, 255, 0], [0, 255, 255]] {
        let fixture = format_two_fixture_with_terrain(VALID_NETWORK, colour);
        assert_error(&fixture, "stretch[0] (course).ends", "open sea");
    }
    let near_land = VALID_NETWORK.replace("[0.0, 0.001]", "[-0.01, 0.01]");
    let fixture = format_two_fixture_with_terrain(&near_land, [0, 255, 0]);
    assert_error(&fixture, "stretch[0] (course).ends", "open sea");

    let lough = VALID_NETWORK.replace("kind = \"river\"", "kind = \"lough\"");
    let fixture = format_two_fixture(&lough);
    assert_error(
        &fixture,
        "stretch[0] (course).kind",
        "midpoint in a lake cell",
    );
}

#[test]
fn format_two_widths_use_their_projected_ground_row_and_round_as_specified() {
    let fixture = format_two_fixture(VALID_NETWORK);
    let map = fs::read_to_string(fixture.0.join("map.toml")).expect("read map metadata");
    fs::write(
        fixture.0.join("map.toml"),
        map.replace("origin = [23882, 23882]", "origin = [23882, 14700]"),
    )
    .expect("move test crop to the specified row");
    let coordinates = |x: i64, y: i64| {
        let half_world = std::f64::consts::PI * 6_378_137.0;
        let x_metres =
            f64::from(i32::try_from(x).expect("x fixed coordinate fits")) * 839.0 / 32_768.0;
        let y_metres =
            f64::from(i32::try_from(y).expect("y fixed coordinate fits")) * 839.0 / 32_768.0;
        let longitude = (x_metres - half_world) / 6_378_137.0 * 180.0 / std::f64::consts::PI;
        let latitude = (2.0 * ((half_world - y_metres) / 6_378_137.0).exp().atan()
            - std::f64::consts::FRAC_PI_2)
            * 180.0
            / std::f64::consts::PI;
        (latitude, longitude)
    };
    let a = coordinates(23_882 * 32_768 + 6_000, 14_700 * 32_768 + 6_000);
    let b = coordinates(23_882 * 32_768 + 14_000, 14_700 * 32_768 + 6_000);
    let text = VALID_NETWORK
        .replace(
            "width_m = [500.0, 900.0]",
            "width_m = [460.3387756347656, 500.0]",
        )
        .replace(
            "points = [[0.0, 0.0], [0.0, 0.001]]",
            &format!(
                "points = [[{:.15}, {:.15}], [{:.15}, {:.15}]]",
                a.0, a.1, b.0, b.1
            ),
        );
    fs::write(fixture.0.join("rivers.toml"), text).expect("write measured widths");
    let loaded = load_map(&fixture.0).expect("width samples load in crop");
    let stretch = &loaded.river_network.expect("network").stretches[0];
    assert_eq!(stretch.width_first, 32_768);
    assert_eq!(stretch.width_last, 35_591);
}

fn large_lough_fixture(include_lake: bool) -> (Fixture, String) {
    let fixture = Fixture::copy();
    let map = fs::read_to_string(fixture.0.join("map.toml")).expect("read map metadata");
    fs::write(
        fixture.0.join("map.toml"),
        map.replace("width = 4\nheight = 4", "width = 12\nheight = 12"),
    )
    .expect("grow map crop");
    let mut terrain = Vec::new();
    let mut fertility = Vec::new();
    for y in 0..12 {
        for x in 0..12 {
            let sea = x == 0 || y == 0 || x == 11 || y == 11 || (x <= 3 && y <= 3);
            let lake = include_lake && (x, y) == (8, 8);
            terrain.extend_from_slice(if sea {
                &[0, 0, 255]
            } else if lake {
                &[0, 255, 255]
            } else {
                &[0, 255, 0]
            });
            fertility.push(if sea || lake { 0 } else { 125 });
        }
    }
    write_png_sized(
        &fixture.0.join("terrain.png"),
        12,
        12,
        ColorType::Rgb,
        BitDepth::Eight,
        &terrain,
        None,
    );
    write_png_sized(
        &fixture.0.join("fertility.png"),
        12,
        12,
        ColorType::Grayscale,
        BitDepth::Eight,
        &fertility,
        None,
    );
    for (name, sample) in [
        ("climate_temperature.png", 1_100_u16),
        ("climate_half_range.png", 40),
        ("climate_sea_nearness.png", 500),
        ("climate_gdd.png", 1_000),
        ("climate_rainfall.png", 1_000),
        ("elevation.png", 11_000),
    ] {
        let values = [sample; 144]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect::<Vec<_>>();
        write_png_sized(
            &fixture.0.join(name),
            12,
            12,
            ColorType::Grayscale,
            BitDepth::Sixteen,
            &values,
            None,
        );
    }
    write_png_sized(
        &fixture.0.join("climate_glacier.png"),
        12,
        12,
        ColorType::Grayscale,
        BitDepth::Eight,
        &[0; 144],
        None,
    );
    let coordinate = |x: i32, y: i32| {
        let half_world = std::f64::consts::PI * 6_378_137.0;
        let fx = f64::from((23_882 + x) * 32_768 + 16_384);
        let fy = f64::from((23_882 + y) * 32_768 + 16_384);
        let longitude =
            ((fx * 839.0 / 32_768.0) - half_world) / 6_378_137.0 * 180.0 / std::f64::consts::PI;
        let y_metres = fy * 839.0 / 32_768.0;
        let latitude = (2.0 * ((half_world - y_metres) / 6_378_137.0).exp().atan()
            - std::f64::consts::FRAC_PI_2)
            * 180.0
            / std::f64::consts::PI;
        (latitude, longitude)
    };
    let a = coordinate(7, 8);
    let join = coordinate(9, 8);
    let end = coordinate(1, 1);
    let text = format!(
        "format = 2\n[tiers]\nshow_from_step = [0, 15, 30]\n[[river]]\nid = \"main\"\nname = \"Main\"\ntier = 1\n\n[[stretch]]\nid = \"lough-course\"\nriver = \"main\"\nkind = \"lough\"\nwidth_m = [500.0, 500.0]\nends = \"down\"\npoints = [[{:.15}, {:.15}], [{:.15}, {:.15}]]\n\n[[stretch]]\nid = \"down\"\nriver = \"main\"\nkind = \"river\"\nwidth_m = [500.0, 500.0]\nends = \"sea\"\npoints = [[{:.15}, {:.15}], [{:.15}, {:.15}]]\n",
        a.0, a.1, join.0, join.1, join.0, join.1, end.0, end.1
    );
    fs::write(fixture.0.join("rivers.toml"), &text).expect("write lough river file");
    (fixture, text)
}

#[test]
fn lough_stretch_requires_and_accepts_a_midpoint_lake_cell() {
    let (fixture, _) = large_lough_fixture(false);
    assert_error(
        &fixture,
        "stretch[0] (lough-course).kind",
        "midpoint in a lake cell",
    );
    let (fixture, _) = large_lough_fixture(true);
    let loaded = load_map(&fixture.0).expect("midpoint lake validates the lough stretch");
    assert_eq!(
        loaded.river_network.expect("network").stretches[0].kind,
        crate::RiverStretchKind::Lough
    );
}

#[test]
fn unsupported_river_format_names_the_format_key() {
    let fixture = Fixture::copy();
    fs::write(fixture.0.join("rivers.toml"), "format = 3\n").expect("write unsupported river file");
    assert_error(&fixture, "rivers.toml format", "unsupported version 3");
}

#[test]
fn fertility_zero_and_four_drive_land_usability_and_grid_as_green_channel() {
    let fixture = Fixture::copy();
    let mut samples = vec![0; 16];
    samples[6] = 1;
    write_png(
        &fixture.0.join("fertility.png"),
        ColorType::Grayscale,
        BitDepth::Eight,
        &samples,
        None,
    );
    let loaded = load_map(&fixture.0).expect("four per-thousand fertility loads");
    assert_eq!(loaded.data.fertility[6], 4);
    assert!(matches!(
        loaded.data.cells[6],
        Surface::Land { usable: true, .. }
    ));
    assert_eq!(
        loaded.grid_a.pixels[6][1], 1,
        "grid A's green channel is the stored fertility (fertility ÷ 4)"
    );
}

#[test]
fn every_computed_layer_requires_the_crop_dimensions() {
    for (filename, colour, depth, bytes) in [
        (
            "fertility.png",
            ColorType::Grayscale,
            BitDepth::Eight,
            vec![0; 12],
        ),
        (
            "climate_temperature.png",
            ColorType::Grayscale,
            BitDepth::Sixteen,
            vec![0; 24],
        ),
        (
            "climate_gdd.png",
            ColorType::Grayscale,
            BitDepth::Sixteen,
            vec![0; 24],
        ),
        (
            "climate_rainfall.png",
            ColorType::Grayscale,
            BitDepth::Sixteen,
            vec![0; 24],
        ),
        (
            "climate_glacier.png",
            ColorType::Grayscale,
            BitDepth::Eight,
            vec![0; 12],
        ),
    ] {
        let fixture = Fixture::copy();
        write_png_sized(&fixture.0.join(filename), 3, 4, colour, depth, &bytes, None);
        assert_error(&fixture, &format!("{filename} dimensions"), "expected 4x4");
    }
}

#[test]
fn a_missing_file_named_as_a_computed_layer_is_rejected() {
    let fixture = Fixture::copy();
    fs::remove_file(fixture.0.join("climate_rainfall.png"))
        .expect("remove the named climate layer");
    assert_error(&fixture, "climate_rainfall.png", "No such file");
}

#[test]
fn fertility_bounds_and_water_consistency_are_named() {
    let fixture = Fixture::copy();
    let mut values = vec![0; 16];
    values[0] = 251;
    write_png(
        &fixture.0.join("fertility.png"),
        ColorType::Grayscale,
        BitDepth::Eight,
        &values,
        None,
    );
    assert_error(&fixture, "fertility.png pixel (0, 0)", "exceeds 250");

    let fixture = Fixture::copy();
    let mut values = vec![0; 16];
    values[0] = 1;
    write_png(
        &fixture.0.join("fertility.png"),
        ColorType::Grayscale,
        BitDepth::Eight,
        &values,
        None,
    );
    assert_error(
        &fixture,
        "fertility.png pixel (0, 0)",
        "water cell must have zero fertility",
    );

    let mut values = [0_u16; 16];
    values[0] = 399;
    let fixture = Fixture::copy();
    write_png(
        &fixture.0.join("climate_temperature.png"),
        ColorType::Grayscale,
        BitDepth::Sixteen,
        &values
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect::<Vec<_>>(),
        None,
    );
    assert_error(
        &fixture,
        "climate_temperature.png pixel (0, 0)",
        "outside 400..=1500",
    );
}

#[test]
fn climate_gdd_and_rainfall_bounds_are_named() {
    let fixture = Fixture::copy();
    let mut values = [0_u16; 16];
    values[0] = 12_001;
    let bytes = values
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect::<Vec<_>>();
    write_png(
        &fixture.0.join("climate_gdd.png"),
        ColorType::Grayscale,
        BitDepth::Sixteen,
        &bytes,
        None,
    );
    assert_error(&fixture, "climate_gdd.png pixel (0, 0)", "exceeds 12000");

    let fixture = Fixture::copy();
    let mut values = [0_u16; 16];
    values[0] = 15_001;
    let bytes = values
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect::<Vec<_>>();
    write_png(
        &fixture.0.join("climate_rainfall.png"),
        ColorType::Grayscale,
        BitDepth::Sixteen,
        &bytes,
        None,
    );
    assert_error(
        &fixture,
        "climate_rainfall.png pixel (0, 0)",
        "exceeds 15000",
    );
}

#[test]
fn glacier_flags_and_fertility_consistency_are_named() {
    let fixture = Fixture::copy();
    let mut values = [0; 16];
    values[0] = 7;
    write_png(
        &fixture.0.join("climate_glacier.png"),
        ColorType::Grayscale,
        BitDepth::Eight,
        &values,
        None,
    );
    assert_error(
        &fixture,
        "climate_glacier.png pixel (0, 0)",
        "must be 0 or 255",
    );

    let fixture = Fixture::copy();
    let mut values = [0; 16];
    values[0] = 255;
    write_png(
        &fixture.0.join("climate_glacier.png"),
        ColorType::Grayscale,
        BitDepth::Eight,
        &values,
        None,
    );
    assert_error(
        &fixture,
        "climate_glacier.png pixel (0, 0)",
        "water cell cannot be a glacier",
    );

    let fixture = Fixture::copy();
    let mut values = [0; 16];
    values[6] = 255;
    write_png(
        &fixture.0.join("climate_glacier.png"),
        ColorType::Grayscale,
        BitDepth::Eight,
        &values,
        None,
    );
    assert_error(
        &fixture,
        "fertility.png pixel (2, 1)",
        "glacier cell must have zero fertility",
    );
}

#[test]
fn indexed_and_rgb_key_layers_are_equivalent_and_rgba_must_be_opaque() {
    let rgb = Fixture::copy();
    let expected = load_map(&rgb.0).expect("RGB fixture loads");
    write_png(
        &rgb.0.join("terrain.png"),
        ColorType::Indexed,
        BitDepth::Eight,
        &[0, 0, 0, 0, 0, 1, 2, 0, 0, 0, 3, 0, 0, 0, 0, 0],
        Some(&[0, 0, 255, 0, 255, 255, 0, 255, 0, 255, 0, 255]),
    );
    let indexed = load_map(&rgb.0).expect("indexed fixture loads");
    assert_eq!(indexed, expected);

    let rgba = Fixture::copy();
    write_png(
        &rgba.0.join("terrain.png"),
        ColorType::Rgba,
        BitDepth::Eight,
        &[
            0, 0, 255, 255, 0, 255, 255, 255, 0, 255, 0, 255, 255, 0, 255, 254,
        ],
        None,
    );
    assert_error(&rgba, "terrain.png pixel (3, 0)", "alpha must be 255");
}

#[test]
fn low_bit_indexed_key_image_names_the_conversion() {
    let fixture = Fixture::copy();
    write_png(
        &fixture.0.join("terrain.png"),
        ColorType::Indexed,
        BitDepth::Four,
        &[0, 1],
        Some(&[0, 0, 255, 0, 255, 255, 0, 255, 0, 255, 0, 255]),
    );
    assert_error(
        &fixture,
        "terrain.png",
        "indexed key image is 4-bit; save as 8-bit",
    );
}

#[test]
fn colour_metadata_chunks_do_not_change_stored_samples() {
    let plain = Fixture::copy();
    let expected = load_map(&plain.0).expect("plain fixture loads");
    let managed = Fixture::copy();
    for name in [
        "terrain.png",
        "fertility.png",
        "elevation.png",
        "climate_temperature.png",
        "climate_gdd.png",
        "climate_rainfall.png",
        "climate_glacier.png",
    ] {
        let path = managed.0.join(name);
        let bytes = fs::read(&path).expect("read png");
        let mut bytes = insert_chunk(
            bytes,
            *b"iCCP",
            &[
                b'p', 0, 0, 120, 156, 75, 41, 205, 205, 173, 84, 240, 116, 118, 86, 40, 40, 202,
                79, 203, 204, 73, 5, 0, 55, 22, 6, 45,
            ],
        );
        bytes = insert_chunk(bytes, *b"gAMA", &[0, 1, 134, 160]);
        bytes = insert_chunk(bytes, *b"sRGB", &[0]);
        bytes = insert_chunk(
            bytes,
            *b"cHRM",
            &[
                0, 0, 0x7a, 0x26, 0, 0, 0x80, 0x84, 0, 0, 0xfa, 0x00, 0, 0, 0x80, 0xe8, 0, 0, 0x75,
                0x30, 0, 0, 0xea, 0x60, 0, 0, 0x3a, 0x98, 0, 0, 0x17, 0x70,
            ],
        );
        fs::write(path, bytes).expect("write color-managed png");
    }
    assert_eq!(
        load_map(&managed.0).expect("color-managed fixture loads"),
        expected
    );
}

#[test]
fn absent_later_milestone_file_is_ignored() {
    let fixture = Fixture::copy();
    fixture.replace(
        "map.toml",
        "rivers = \"rivers.toml\"",
        "rivers = \"rivers.toml\"\nrulers = \"rulers.png\"",
    );
    let loaded = load_map(&fixture.0).expect("base fixture loads");
    assert_eq!(
        load_map(&fixture.0).expect("optional later file is ignored"),
        loaded
    );
}

#[test]
fn names_projection_cell_crop_and_image_sizes_are_validated() {
    let fixture = Fixture::copy();
    fixture.replace("map.toml", "spherical_mercator", "plate_carree");
    assert_error(
        &fixture,
        "map.toml projection",
        "expected spherical_mercator",
    );

    let fixture = Fixture::copy();
    fixture.replace("map.toml", "839.0", "838.0");
    assert_error(&fixture, "map.toml cell_metres", "expected 839");

    let fixture = Fixture::copy();
    fixture.replace("map.toml", "width = 4", "width = 0");
    assert_error(&fixture, "map.toml crop", "nonzero");

    let fixture = Fixture::copy();
    fixture.replace("map.toml", "width = 4", "width = 8193");
    assert_error(&fixture, "map.toml crop", "limit");

    let fixture = Fixture::copy();
    fixture.replace(
        "map.toml",
        "origin = [23882, 23882]",
        "origin = [-1, 23882]",
    );
    assert_error(&fixture, "map.toml crop.origin", "nonnegative");

    let fixture = Fixture::copy();
    fixture.replace("map.toml", "width = 4", "width = 5");
    assert_error(&fixture, "terrain.png dimensions", "expected 5x4");
}

#[test]
fn key_pixels_and_key_image_formats_are_validated() {
    let fixture = Fixture::copy();
    write_png(
        &fixture.0.join("terrain.png"),
        ColorType::Rgb,
        BitDepth::Eight,
        &[1, 2, 3, 0, 255, 255, 0, 255, 0, 255, 0, 255],
        None,
    );
    assert_error(&fixture, "terrain.png pixel (0, 0)", "unknown terrain key");

    let fixture = Fixture::copy();
    write_png(
        &fixture.0.join("fertility.png"),
        ColorType::Rgb,
        BitDepth::Eight,
        &[1, 2, 3, 255, 0, 0, 0, 255, 0, 255, 0, 255],
        None,
    );
    assert_error(&fixture, "fertility.png", "8-bit grayscale");

    let fixture = Fixture::copy();
    write_png(
        &fixture.0.join("terrain.png"),
        ColorType::Grayscale,
        BitDepth::Eight,
        &[0, 1, 2, 3],
        None,
    );
    assert_error(&fixture, "terrain.png", "8-bit RGB, RGBA, or indexed");

    let fixture = Fixture::copy();
    write_png(
        &fixture.0.join("elevation.png"),
        ColorType::Grayscale,
        BitDepth::Eight,
        &[0, 1, 2, 3],
        None,
    );
    assert_error(&fixture, "elevation.png", "16-bit grayscale");
}

#[test]
fn terrain_table_invariants_are_validated_at_the_offending_entry() {
    let fixture = Fixture::copy();
    fixture.replace("terrain.toml", "key = \"00ffff\"", "key = \"0000ff\"");
    assert_error(
        &fixture,
        "terrain.toml terrain[1].key",
        "duplicate key colour",
    );

    let fixture = Fixture::copy();
    fixture.replace(
        "terrain.toml",
        "shallow_threshold = 20",
        "shallow_threshold = 0",
    );
    assert_error(
        &fixture,
        "terrain.toml shallow_threshold",
        "greater than zero",
    );

    // The keys of the earlier look are removed, and a file that still has one is refused by name (milestone 9, slice 10a).
    for key in [
        "relief_low = 0",
        "relief_high = 100",
        "mix = 30",
        "cap = \"abcdef\"",
        "high_ground = \"fedcba\"",
        "cap_low = 1000",
        "cap_high = 2000",
    ] {
        let fixture = Fixture::copy();
        fixture.replace(
            "terrain.toml",
            "name = \"plains\"",
            &format!("{key}\nname = \"plains\""),
        );
        let name = key.split(' ').next().expect("a key");
        assert_error(&fixture, "terrain.toml", &format!("unknown field `{name}`"));
    }
    let fixture = Fixture::copy();
    fixture.replace(
        "terrain.toml",
        "shallow_threshold = 20",
        "shallow_threshold = 20\ndisplay_usable_pm = 500",
    );
    assert_error(
        &fixture,
        "terrain.toml",
        "unknown field `display_usable_pm`",
    );

    // The new keys are required and checked: dark_lit, the ice, and the light and curve constant.
    let fixture = Fixture::copy();
    fixture.replace("terrain.toml", "dark_lit = \"556677\"\n", "");
    assert_error(
        &fixture,
        "terrain.toml terrain[2].dark_lit",
        "required field is missing",
    );
    let fixture = Fixture::copy();
    fixture.replace("terrain.toml", "light = [-3827, -9239]", "light = [0, 0]");
    assert_error(&fixture, "terrain.toml look.light", "nonzero vector");
    let fixture = Fixture::copy();
    fixture.replace(
        "terrain.toml",
        "light = [-3827, -9239]",
        "light = [-10001, 0]",
    );
    assert_error(&fixture, "terrain.toml look.light", "within -10000..=10000");
    let fixture = Fixture::copy();
    fixture.replace("terrain.toml", "d0 = [1, 80]", "d0 = [0, 80]");
    assert_error(&fixture, "terrain.toml look.d0", "positive");
    let fixture = Fixture::copy();
    fixture.replace("terrain.toml", "colour = \"eeeeee\"", "colour = \"zz\"");
    assert_error(
        &fixture,
        "terrain.toml ice.colour",
        "six hexadecimal digits",
    );

    let fixture = Fixture::copy();
    fixture.replace("terrain.toml", "kind = \"lake\"", "kind = \"sea\"");
    assert_error(&fixture, "terrain.toml terrain", "exactly one sea");

    let fixture = Fixture::copy();
    fixture.replace(
        "terrain.toml",
        "name = \"lake\"\nkind = \"lake\"\nkey = \"00ffff\"",
        "name = \"lake\"\nkind = \"land\"\nkey = \"00ffff\"\nsolid = \"112233\"\nsecond = \"223344\"\ndark = \"334455\"\ndark_second = \"445566\"\ndark_lit = \"556677\"",
    );
    assert_error(&fixture, "terrain.toml terrain", "exactly one lake");

    let fixture = Fixture::copy();
    fixture.replace("terrain.toml", "key = \"00ffff\"", "key = \"00fffg\"");
    assert_error(
        &fixture,
        "terrain.toml terrain[1].key",
        "six hexadecimal digits",
    );

    let fixture = Fixture::copy();
    fixture.replace("terrain.toml", "second = \"223344\"\n", "");
    assert_error(
        &fixture,
        "terrain.toml terrain[2].second",
        "required field is missing",
    );
}

#[test]
fn terrain_type_limit_and_river_crop_membership_are_validated() {
    let fixture = Fixture::copy();
    let mut source = fixture.text("terrain.toml");
    for index in 4..257 {
        source.push_str("\n\n[[terrain]]\nname = \"extra");
        source.push_str(&index.to_string());
        source.push_str("\"\nkind = \"land\"\nkey = \"aabbcc\"\nsolid = \"112233\"\nsecond = \"223344\"\ndark = \"334455\"\ndark_second = \"445566\"\ndark_lit = \"556677\"\n");
    }
    fs::write(fixture.0.join("terrain.toml"), source).expect("write large table");
    assert_error(&fixture, "terrain.toml terrain", "maximum is 256");

    let fixture = Fixture::copy();
    fixture.replace("rivers.toml", "longitude = 0.005", "longitude = 40.0");
    assert_error(&fixture, "rivers.toml river[0].points[1]", "outside crop");
}

fn assert_error(fixture: &Fixture, file_and_entry: &str, detail: &str) {
    let error = load_map(&fixture.0)
        .expect_err("fixture should be rejected")
        .to_string();
    assert!(
        error.contains(file_and_entry),
        "expected {file_and_entry:?} in {error:?}"
    );
    assert!(error.contains(detail), "expected {detail:?} in {error:?}");
}

fn write_png(
    path: &Path,
    colour: ColorType,
    depth: BitDepth,
    bytes: &[u8],
    palette: Option<&[u8]>,
) {
    let expected = match (colour, depth) {
        (ColorType::Rgb, BitDepth::Eight) => 4 * 4 * 3,
        (ColorType::Rgba, BitDepth::Eight) => 4 * 4 * 4,
        (
            ColorType::Grayscale | ColorType::Indexed,
            BitDepth::Eight | BitDepth::Four | BitDepth::Two | BitDepth::One,
        ) => {
            let bits = match depth {
                BitDepth::Eight => 8,
                BitDepth::Four => 4,
                BitDepth::Two => 2,
                BitDepth::One => 1,
                BitDepth::Sixteen => unreachable!(),
            };
            (16_usize * bits).div_ceil(8)
        }
        _ => bytes.len(),
    };
    let expanded = (0..expected)
        .map(|index| bytes[index % bytes.len()])
        .collect::<Vec<_>>();
    write_png_sized(path, 4, 4, colour, depth, &expanded, palette);
}

fn write_png_sized(
    path: &Path,
    width: u32,
    height: u32,
    colour: ColorType,
    depth: BitDepth,
    bytes: &[u8],
    palette: Option<&[u8]>,
) {
    let file = fs::File::create(path).expect("create png");
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(colour);
    encoder.set_depth(depth);
    if let Some(palette) = palette {
        encoder.set_palette(palette);
    }
    let mut writer = encoder.write_header().expect("write png header");
    writer.write_image_data(bytes).expect("write png data");
}

fn insert_chunk(mut png: Vec<u8>, name: [u8; 4], data: &[u8]) -> Vec<u8> {
    let mut offset = 8;
    while offset < png.len() {
        let length =
            u32::from_be_bytes(png[offset..offset + 4].try_into().expect("chunk length")) as usize;
        let chunk_name = &png[offset + 4..offset + 8];
        if chunk_name == b"IDAT" {
            let mut chunk = Vec::with_capacity(data.len() + 12);
            chunk.extend_from_slice(&u32::try_from(data.len()).expect("chunk fits").to_be_bytes());
            chunk.extend_from_slice(&name);
            chunk.extend_from_slice(data);
            let mut crc = 0xffff_ffff_u32;
            for byte in name.iter().chain(data.iter()) {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xedb8_8320 & (0_u32.wrapping_sub(crc & 1)));
                }
            }
            chunk.extend_from_slice(&(!crc).to_be_bytes());
            png.splice(offset..offset, chunk);
            return png;
        }
        offset += 12 + length;
    }
    panic!("PNG did not contain IDAT");
}

#[test]
fn weather_layers_and_phase_are_required_and_bounded() {
    let fixture = Fixture::copy();
    let loaded = load_map(&fixture.0).unwrap();
    assert_eq!(loaded.data.climate_half_range, [40; 16]);
    assert_eq!(loaded.data.climate_sea_nearness, [500; 16]);
    assert_eq!(loaded.data.climate_phase, 650);
    for (name, sample, limit) in [
        ("climate_half_range.png", 301_u16, "300"),
        ("climate_sea_nearness.png", 1001, "1000"),
    ] {
        let fixture = Fixture::copy();
        let bytes = [sample; 16]
            .iter()
            .flat_map(|n| n.to_be_bytes())
            .collect::<Vec<_>>();
        write_png_sized(
            &fixture.0.join(name),
            4,
            4,
            ColorType::Grayscale,
            BitDepth::Sixteen,
            &bytes,
            None,
        );
        assert_error(&fixture, name, limit);
    }
    for key in ["climate_half_range", "climate_sea_nearness"] {
        let fixture = Fixture::copy();
        let text = fixture.text("map.toml");
        let line = format!("{key} = \"{key}.png\"\n");
        fs::write(fixture.0.join("map.toml"), text.replace(&line, "")).unwrap();
        assert_error(&fixture, "map.toml", key);
    }
    for replacement in [
        "warmest_month_phase_hundredths = 1200",
        "warmest_month_phase_hundredths = 6.5",
        "",
    ] {
        let fixture = Fixture::copy();
        fixture.replace(
            "map.toml",
            "warmest_month_phase_hundredths = 650",
            replacement,
        );
        assert_error(&fixture, "map.toml", "warmest_month_phase_hundredths");
    }
    for name in ["climate_half_range.png", "climate_sea_nearness.png"] {
        let fixture = Fixture::copy();
        write_png_sized(
            &fixture.0.join(name),
            3,
            4,
            ColorType::Grayscale,
            BitDepth::Sixteen,
            &[0; 24],
            None,
        );
        assert_error(&fixture, name, "expected 4x4");
    }
}
