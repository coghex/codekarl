//! Backdrop fixture and the shipped world data.
use ck_map::load_backdrop;
use std::path::PathBuf;

#[test]
fn large_fixture_loads_and_answers_block_samples() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/backdrop");
    let backdrop = load_backdrop(&path).expect("valid fixture");
    assert_eq!(
        (
            backdrop.block_cells(),
            backdrop.columns(),
            backdrop.first_row(),
            backdrop.rows()
        ),
        (4096, 12, 2, 7)
    );
    assert_eq!(backdrop.block_of(5 * 4096, 3 * 4096), Some((5, 3)));
    assert_eq!(backdrop.block_of(0, 4096), None);
    assert_eq!(backdrop.east(11), Some(0));
    let sample = backdrop.sample(5, 3, 0).expect("sea sample");
    assert!(!sample.land);
    assert_eq!(sample.pressure_pa, 100_558);
    assert_eq!(sample.sea_temperature_tenths, Some(93));
    assert!(backdrop.sample(7, 4, 0).expect("land").land);
    assert_eq!(
        backdrop
            .sample(7, 4, 0)
            .expect("sample")
            .sea_temperature_tenths,
        None
    );
    assert!(backdrop.sample(5, 3, 12).is_none());
}

#[test]
fn shipped_data_matches_worked_normals() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/map/backdrop");
    let backdrop = load_backdrop(&path).expect("shipped backdrop");
    assert_eq!(
        (backdrop.columns(), backdrop.first_row(), backdrop.rows()),
        (747, 132, 398)
    );
    for (column, row, expected) in [
        (
            351,
            246,
            [
                101_148, 101_451, 101_394, 101_456, 101_602, 101_649, 101_627, 101_590, 101_544,
                101_253, 101_270, 101_329,
            ],
        ),
        (
            356,
            242,
            [
                101_154, 101_414, 101_377, 101_461, 101_576, 101_590, 101_542, 101_509, 101_529,
                101_229, 101_244, 101_295,
            ],
        ),
        (
            327,
            198,
            [
                100_038, 100_426, 100_947, 101_231, 101_654, 101_318, 101_127, 101_102, 100_797,
                100_696, 100_581, 100_144,
            ],
        ),
        (
            319,
            288,
            [
                102_099, 102_009, 102_022, 102_085, 102_193, 102_385, 102_407, 102_241, 102_085,
                101_916, 101_856, 101_999,
            ],
        ),
    ] {
        for (month, pressure) in expected.into_iter().enumerate() {
            assert_eq!(
                backdrop
                    .sample(column, row, u32::try_from(month).expect("month index fits"))
                    .expect("sample")
                    .pressure_pa,
                pressure
            );
        }
    }
    let birr = [35, 36, 43, 68, 91, 124, 138, 135, 116, 83, 56, 42];
    for (month, air) in birr.into_iter().enumerate() {
        let sample = backdrop
            .sample(356, 242, u32::try_from(month).expect("month index fits"))
            .expect("sample");
        assert!(sample.land);
        assert_eq!(sample.air_temperature_tenths, air);
        assert_eq!(sample.sea_temperature_tenths, None);
    }
    assert_eq!(
        backdrop
            .sample(360, 244, 1)
            .expect("sample")
            .sea_temperature_tenths,
        Some(83)
    );
    assert_eq!(backdrop.east(746), Some(0));
    assert_eq!(backdrop.block_of(0, 131 * 64), None);
    assert_eq!(backdrop.block_of(0, 530 * 64), None);
}

#[test]
fn rejects_invalid_manifests_and_images_with_file_names() {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/backdrop");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("ck-backdrop-{}-{stamp}", std::process::id()));
    fs::create_dir(&dir).expect("create fixture");
    let names = [
        "backdrop.toml",
        "land.png",
        "pressure.png",
        "air_temperature.png",
        "sea_temperature.png",
    ];
    for name in names {
        fs::copy(fixture.join(name), dir.join(name)).expect("copy fixture");
    }
    let original = fs::read_to_string(dir.join("backdrop.toml")).expect("manifest");
    for (old, new) in [
        ("format = 1", "format = 2"),
        ("spherical_mercator", "bad_projection"),
        ("cell_metres = 839.0", "cell_metres = 840.0"),
        ("block_cells = 4096", "block_cells = 24"),
        ("columns = 12", "columns = 11"),
        ("rows = 7", "rows = 0"),
        ("first_row = 2", "first_row = 9"),
    ] {
        fs::write(dir.join("backdrop.toml"), original.replace(old, new)).expect("write manifest");
        let error = load_backdrop(&dir)
            .err()
            .expect("invalid manifest")
            .to_string();
        assert!(error.contains("backdrop.toml"), "{error}");
    }
    fs::write(dir.join("backdrop.toml"), original).expect("restore manifest");
    for name in [
        "land.png",
        "pressure.png",
        "air_temperature.png",
        "sea_temperature.png",
    ] {
        fs::write(dir.join(name), b"invalid PNG").expect("corrupt image");
        let error = load_backdrop(&dir)
            .err()
            .expect("invalid image")
            .to_string();
        assert!(error.contains(name), "{error}");
        fs::copy(fixture.join(name), dir.join(name)).expect("restore image");
    }
    gray(&dir.join("land.png"), 1, 1, png::BitDepth::Eight, &[0]);
    let error = load_backdrop(&dir)
        .err()
        .expect("wrong dimensions")
        .to_string();
    assert!(
        error.contains("land.png") && error.contains("dimensions"),
        "{error}"
    );
    fs::remove_dir_all(dir).expect("remove fixture");
}

fn gray(path: &std::path::Path, width: u32, height: u32, depth: png::BitDepth, bytes: &[u8]) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).expect("create png"));
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(depth);
    encoder
        .write_header()
        .expect("header")
        .write_image_data(bytes)
        .expect("pixels");
}
fn words(value: u16) -> Vec<u8> {
    (0..12 * 84).flat_map(|_| value.to_be_bytes()).collect()
}

#[test]
fn rejects_out_of_range_pixel_samples() {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("ck-backdrop-values-{}-{stamp}", std::process::id()));
    fs::create_dir(&dir).expect("create fixture");
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/backdrop");
    fs::copy(fixture.join("backdrop.toml"), dir.join("backdrop.toml")).expect("manifest");
    gray(&dir.join("land.png"), 12, 7, png::BitDepth::Eight, &[0; 84]);
    gray(
        &dir.join("pressure.png"),
        12,
        84,
        png::BitDepth::Sixteen,
        &words(41_174),
    );
    gray(
        &dir.join("air_temperature.png"),
        12,
        84,
        png::BitDepth::Sixteen,
        &words(1035),
    );
    gray(
        &dir.join("sea_temperature.png"),
        12,
        84,
        png::BitDepth::Sixteen,
        &words(1000),
    );
    assert!(load_backdrop(&dir).is_ok());
    for (file, values) in [
        ("pressure.png", vec![29_999, 47_001]),
        ("air_temperature.png", vec![299, 1501]),
        ("sea_temperature.png", vec![0, 969, 1401]),
    ] {
        let normal = match file {
            "pressure.png" => 41_174,
            "air_temperature.png" => 1035,
            _ => 1000,
        };
        for value in values {
            gray(
                &dir.join(file),
                12,
                84,
                png::BitDepth::Sixteen,
                &words(value),
            );
            let error = load_backdrop(&dir)
                .err()
                .expect("invalid sample")
                .to_string();
            assert!(error.contains(file), "{error}");
        }
        gray(
            &dir.join(file),
            12,
            84,
            png::BitDepth::Sixteen,
            &words(normal),
        );
    }
    gray(&dir.join("land.png"), 12, 7, png::BitDepth::Eight, &[1; 84]);
    assert!(
        load_backdrop(&dir)
            .err()
            .expect("bad land")
            .to_string()
            .contains("land.png")
    );
    gray(
        &dir.join("land.png"),
        12,
        7,
        png::BitDepth::Eight,
        &[255; 84],
    );
    assert!(
        load_backdrop(&dir)
            .err()
            .expect("sea on land")
            .to_string()
            .contains("sea_temperature.png")
    );
    fs::remove_dir_all(dir).expect("remove fixture");
}

#[test]
fn last_row_may_extend_past_projection_edge() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/backdrop8192");
    let backdrop = load_backdrop(&path).expect("projection-edge fixture");
    assert_eq!(
        (
            backdrop.block_cells(),
            backdrop.columns(),
            backdrop.first_row(),
            backdrop.rows()
        ),
        (8192, 6, 0, 6)
    );
    assert_eq!(backdrop.block_of(0, 47_765), Some((0, 5)));
    assert_eq!(backdrop.block_of(0, 47_766), None);
}

#[test]
fn shipped_sea_and_second_air_row_match_the_worked_numbers() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/map/backdrop");
    let backdrop = load_backdrop(&path).expect("shipped backdrop");
    let sea_rows = [
        (
            348,
            246,
            [107, 104, 104, 109, 118, 136, 151, 157, 151, 137, 122, 114],
        ),
        (
            478,
            277,
            [92, 82, 86, 110, 156, 207, 242, 249, 225, 186, 145, 112],
        ),
        (
            319,
            288,
            [164, 156, 157, 163, 174, 194, 218, 230, 226, 209, 190, 174],
        ),
    ];
    for (c, r, months) in sea_rows {
        for (month, expected) in months.into_iter().enumerate() {
            let sample = backdrop
                .sample(c, r, u32::try_from(month).expect("month"))
                .expect("sea");
            assert!(!sample.land);
            assert_eq!(sample.sea_temperature_tenths, Some(expected));
        }
    }
    for (month, expected) in [40, 40, 45, 67, 91, 122, 137, 135, 119, 89, 63, 49]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            backdrop
                .sample(359, 238, u32::try_from(month).expect("month"))
                .expect("armagh")
                .air_temperature_tenths,
            expected
        );
    }
}

#[test]
fn worked_places_have_their_expected_blocks_and_land_classes() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/map/backdrop");
    let backdrop = load_backdrop(&path).expect("shipped backdrop");
    for (lat, lon, column, row, land) in [
        (53.0956, -7.9101, 356, 242, true),
        (54.3489, -6.6545, 359, 238, true),
        (52.0, -12.0, 348, 246, false),
        (51.94, -10.24, 351, 246, false),
        (54.6, -6.4, 359, 237, true),
        (54.2, -4.5, 363, 238, false),
        (62.0, -6.9, 358, 208, false),
        (64.1, -21.9, 327, 198, false),
        (52.5, 5.5, 384, 244, true),
        (31.5, 35.5, 446, 304, true),
        (42.0, 51.0, 478, 277, false),
        (-1.0, 33.0, 441, 375, true),
        (51.4779, 0.0, 373, 248, true),
        (0.0, 0.0, 373, 373, false),
        (0.0, 179.99, 746, 373, false),
        (0.0, -180.0, 0, 373, false),
    ] {
        let cell = ck_map::cell_of(ck_map::project(lat, lon).expect("project"));
        let block = backdrop.block_of(
            u32::try_from(cell.x).expect("x"),
            u32::try_from(cell.y).expect("y"),
        );
        assert_eq!(block, Some((column, row)));
        assert_eq!(backdrop.sample(column, row, 0).expect("covered").land, land);
    }
}

#[test]
fn large_fixture_matches_all_worked_land_and_monthly_values() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/backdrop");
    let backdrop = load_backdrop(&path).expect("valid fixture");
    let counts: [[u8; 12]; 7] = [
        [17, 30, 34, 22, 54, 5, 18, 21, 47, 56, 37, 17],
        [15, 44, 58, 35, 7, 8, 49, 64, 64, 64, 45, 15],
        [0, 7, 58, 28, 1, 24, 40, 58, 64, 56, 10, 0],
        [1, 0, 8, 28, 7, 29, 62, 25, 16, 24, 3, 0],
        [0, 0, 0, 36, 29, 0, 42, 6, 0, 20, 33, 0],
        [0, 0, 0, 18, 0, 0, 0, 0, 1, 0, 6, 3],
        [0, 0, 0, 4, 0, 0, 1, 10, 10, 15, 11, 0],
    ];
    for (row, columns) in counts.iter().enumerate() {
        for (column, count) in columns.iter().enumerate() {
            let sample = backdrop
                .sample(
                    u32::try_from(column).expect("column"),
                    u32::try_from(row).expect("row") + 2,
                    0,
                )
                .expect("block");
            assert_eq!(sample.land, *count >= 32);
        }
    }
    let pressure = [
        100_558, 100_904, 101_084, 101_335, 101_568, 101_390, 101_285, 101_204, 101_164, 100_895,
        100_847, 100_688,
    ];
    let sea = [93, 90, 90, 93, 101, 113, 126, 133, 127, 116, 105, 99];
    for month in 0..12 {
        let sample = backdrop
            .sample(5, 3, u32::try_from(month).expect("month"))
            .expect("sea");
        assert_eq!(sample.pressure_pa, pressure[month]);
        assert_eq!(sample.sea_temperature_tenths, Some(sea[month]));
    }
}
