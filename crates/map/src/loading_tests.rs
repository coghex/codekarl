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
