use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{
    BuildingKind, Confidence, HouseStyle, Location, LocationKind, LocationKinds, PreparedLocations,
    ScenarioStart, compute_catchments, ground_scales, load_scenario, prepare_locations,
    prepare_markers,
};
use crate::{Cell, Crop, MapData, Rgba8, STEP_SIZE, Surface, TerrainStyle, WorldPixel};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    scenario: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("ck-map-scenario-{}-{id}", std::process::id()));
        let scenario = root.join("scenarios/test_768");
        let map = root.join("map/fixture");
        fs::create_dir_all(&scenario).expect("create scenario directory");
        fs::create_dir_all(&map).expect("create shared map directory");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/valid");
        for name in [
            "map.toml",
            "terrain.toml",
            "rivers.toml",
            "terrain.png",
            "usable.png",
            "elevation.png",
        ] {
            fs::copy(source.join(name), map.join(name)).expect("copy map fixture");
        }
        fs::write(
            scenario.join("scenario.toml"),
            "id = \"test_768\"
map = \"fixture\"
locations = \"locations.toml\"
[start]
year = 768
month = 10
day = 9
hour = 0
",
        )
        .expect("write scenario");
        fs::write(
            map.join("location_kinds.toml"),
            "[[kind]]
id = \"town\"
sizes = [1, 2]
catchment_m = [839, 2000]
houses = { count = [2, 3], radius_cells_x256 = [256, 512] }
glyph = [\".#.\", \"#d#\", \".#.\"]
light = \"#d7c89b\"
dark = \"#66583c\"
[[building]]
id = \"church\"
glyph = [\"#\"]
light = \"#ffffff\"
dark = \"#000000\"
[house]
roof = \"#a9563b\"
light = \"#ece3cb\"
dark = \"#2a2119\"
",
        )
        .expect("write kinds");
        let land = lat_lon_for_cell(23_882, 23_883);
        let mountain = lat_lon_for_cell(23_883, 23_883);
        fs::write(scenario.join("locations.toml"), format!("[[location]]\nid = \"town_a\"\nname = \"Test Place\"\nlat = {}\nlon = {}\nkind = \"town\"\nsize = 1\nbuildings = [\"church\"]\nsource = \"fixture\"\nconfidence = \"high\"\n\n[[location]]\nid = \"town_b\"\nname = \"Mountain Place\"\nlat = {}\nlon = {}\nkind = \"town\"\nsize = 2\nbuildings = []\nsource = \"fixture\"\nconfidence = \"medium\"\n", land.0, land.1, mountain.0, mountain.1)).expect("write locations");
        Self { root, scenario }
    }

    fn path(&self) -> &Path {
        &self.scenario
    }
    fn root(&self) -> &Path {
        &self.root
    }
    fn map_path(&self, name: &str) -> PathBuf {
        self.root.join("map").join(name)
    }
    fn copy_map_as(&self, name: &str) {
        let destination = self.map_path(name);
        fs::create_dir_all(&destination).expect("create named map directory");
        for file in [
            "map.toml",
            "terrain.toml",
            "rivers.toml",
            "terrain.png",
            "usable.png",
            "elevation.png",
            "location_kinds.toml",
        ] {
            fs::copy(self.map_path("fixture").join(file), destination.join(file))
                .expect("copy named map fixture");
        }
    }
    fn text(&self, path: &str) -> String {
        fs::read_to_string(self.scenario.join(path)).expect("read fixture text")
    }
    fn write(&self, path: &str, text: &str) {
        fs::write(self.scenario.join(path), text).expect("write fixture text");
    }
    fn map_text(&self, path: &str) -> String {
        fs::read_to_string(self.map_path("fixture").join(path)).expect("read map fixture text")
    }
    fn write_map(&self, path: &str, text: &str) {
        fs::write(self.map_path("fixture").join(path), text).expect("write map fixture text");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn lat_lon_for_cell(x: i32, y: i32) -> (f64, f64) {
    let radius = 6_378_137.0_f64;
    let half = std::f64::consts::PI * radius;
    let x_metres = (f64::from(x) + 0.5) * 839.0;
    let y_metres = (f64::from(y) + 0.5) * 839.0;
    let longitude = ((x_metres - half) / radius).to_degrees();
    let latitude = 2.0 * ((half - y_metres) / radius).exp().atan() - std::f64::consts::FRAC_PI_2;
    (latitude.to_degrees(), longitude)
}

#[test]
fn scenario_loads_map_locations_styles_and_catchment_owners() {
    let fixture = Fixture::new();
    let scenario = load_scenario(fixture.path()).expect("scenario loads");
    assert_eq!(scenario.id, "test_768");
    assert_eq!(
        scenario.start,
        ScenarioStart {
            year: 768,
            month: 10,
            day: 9,
            hour: 0
        }
    );
    assert_eq!(scenario.locations.len(), 2);
    assert_eq!(scenario.location_kinds.kinds[0].id, "town");
    assert_eq!(scenario.locations[0].confidence, Confidence::High);
    assert_eq!(scenario.locations[0].building_indices, [0]);
    assert_eq!(scenario.catchment_owners.len(), 4);
    assert_eq!(scenario.catchment_owners[2], Some(0));
    assert_eq!(scenario.catchment_owners[3], Some(1));
}

#[test]
fn unknown_fields_and_invalid_scenario_dates_are_source_qualified() {
    let fixture = Fixture::new();
    let mut scenario = fixture.text("scenario.toml");
    scenario.push_str("\nunknown = 1\n");
    fixture.write("scenario.toml", &scenario);
    assert!(
        load_scenario(fixture.path())
            .expect_err("unknown scenario key refused")
            .to_string()
            .contains("scenario.toml")
    );

    let fixture = Fixture::new();
    let scenario = fixture
        .text("scenario.toml")
        .replace("month = 10", "month = 13");
    fixture.write("scenario.toml", &scenario);
    let error = load_scenario(fixture.path())
        .expect_err("bad month refused")
        .to_string();
    assert!(error.contains("scenario.toml start.month"), "{error}");

    for (field, value) in [("year", "10000"), ("day", "0"), ("hour", "24")] {
        let fixture = Fixture::new();
        let scenario = fixture.text("scenario.toml").replace(
            &format!(
                "{field} = {}",
                match field {
                    "year" => "768",
                    "day" => "9",
                    _ => "0",
                }
            ),
            &format!("{field} = {value}"),
        );
        fixture.write("scenario.toml", &scenario);
        let error = load_scenario(fixture.path())
            .expect_err("out-of-range start field refused")
            .to_string();
        assert!(
            error.contains(&format!("scenario.toml start.{field}")),
            "{error}"
        );
    }
}

#[test]
fn scenario_map_name_rule_rejects_invalid_names_and_accepts_48_characters() {
    for name in ["../map", "map/fixture", ".", "", "Fixture"] {
        let fixture = Fixture::new();
        fixture.write(
            "scenario.toml",
            &fixture
                .text("scenario.toml")
                .replace("map = \"fixture\"", &format!("map = {name:?}")),
        );
        let error = load_scenario(fixture.path())
            .expect_err("invalid map name refused")
            .to_string();
        assert_eq!(
            error,
            "scenario.toml scenario.map: must be a map name ([a-z0-9_]{1,48})"
        );
    }

    let fixture = Fixture::new();
    let too_long = "a".repeat(49);
    fixture.write(
        "scenario.toml",
        &fixture
            .text("scenario.toml")
            .replace("map = \"fixture\"", &format!("map = {too_long:?}")),
    );
    assert_eq!(
        load_scenario(fixture.path())
            .expect_err("49-character map name refused")
            .to_string(),
        "scenario.toml scenario.map: must be a map name ([a-z0-9_]{1,48})"
    );

    let fixture = Fixture::new();
    let maximum = "a".repeat(48);
    fixture.copy_map_as(&maximum);
    fixture.write(
        "scenario.toml",
        &fixture
            .text("scenario.toml")
            .replace("map = \"fixture\"", &format!("map = {maximum:?}")),
    );
    load_scenario(fixture.path()).expect("48-character map name is accepted");
}

#[test]
fn scenario_layout_and_missing_map_are_refused_before_loading() {
    let fixture = Fixture::new();
    let misplaced = fixture.root().join("not-scenarios/test_768");
    fs::create_dir_all(&misplaced).expect("create misplaced scenario directory");
    fs::copy(
        fixture.path().join("scenario.toml"),
        misplaced.join("scenario.toml"),
    )
    .expect("copy scenario file");
    fs::copy(
        fixture.path().join("locations.toml"),
        misplaced.join("locations.toml"),
    )
    .expect("copy locations file");
    let error = load_scenario(&misplaced)
        .expect_err("scenario outside the scenarios directory refused")
        .to_string();
    assert!(error.contains("scenario.toml scenario.map"), "{error}");
    assert!(error.contains("<root>/scenarios/<directory>"), "{error}");

    let fixture = Fixture::new();
    let missing = fixture.map_path("missing");
    fixture.write(
        "scenario.toml",
        &fixture
            .text("scenario.toml")
            .replace("map = \"fixture\"", "map = \"missing\""),
    );
    let error = load_scenario(fixture.path())
        .expect_err("missing map refused")
        .to_string();
    assert!(error.contains("scenario.toml scenario.map"), "{error}");
    assert!(error.contains("missing"), "{error}");
    assert!(error.contains(&missing.display().to_string()), "{error}");
    assert!(
        !error.contains("map.toml"),
        "load_map must not run: {error}"
    );
}

#[test]
fn scenarios_can_share_one_named_map() {
    let fixture = Fixture::new();
    let second = fixture.root().join("scenarios/another");
    fs::create_dir_all(&second).expect("create second scenario");
    fs::write(
        second.join("scenario.toml"),
        fixture
            .text("scenario.toml")
            .replace("test_768", "another_768"),
    )
    .expect("write second scenario");
    fs::copy(
        fixture.path().join("locations.toml"),
        second.join("locations.toml"),
    )
    .expect("copy second locations");

    let first_loaded = load_scenario(fixture.path()).expect("first scenario loads");
    let second_loaded = load_scenario(second).expect("second scenario loads");
    assert_eq!(first_loaded.id, "test_768");
    assert_eq!(second_loaded.id, "another_768");
    assert_eq!(first_loaded.map.data.crop, second_loaded.map.data.crop);
    assert_eq!(first_loaded.location_kinds, second_loaded.location_kinds);
}

#[cfg(unix)]
#[test]
fn scenario_symlinks_keep_the_lexical_assets_root() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let target = fixture.root().join("external/scenario");
    fs::create_dir_all(&target).expect("create external scenario target");
    fs::copy(
        fixture.path().join("scenario.toml"),
        target.join("scenario.toml"),
    )
    .expect("copy scenario file");
    fs::copy(
        fixture.path().join("locations.toml"),
        target.join("locations.toml"),
    )
    .expect("copy locations file");
    let link = fixture.root().join("scenarios/linked");
    symlink(&target, &link).expect("link scenario under lexical scenarios directory");

    let scenario = load_scenario(link).expect("the lexical root supplies the shared map");
    assert_eq!(scenario.id, "test_768");
    assert_eq!(scenario.location_kinds.kinds[0].id, "town");
}

#[test]
fn location_refusals_name_the_file_and_entry() {
    let cases = [
        ("id = \"town_a\"", "id = \"Bad-ID\"", "id must match"),
        ("name = \"Test Place\"", "name = \" \"", "name"),
        ("kind = \"town\"", "kind = \"unknown\"", "unknown kind"),
        (
            "buildings = [\"church\"]",
            "buildings = [\"missing\"]",
            "unknown building",
        ),
        (
            "buildings = [\"church\"]",
            "buildings = [\"church\", \"church\"]",
            "repeated building",
        ),
        (
            "confidence = \"high\"",
            "confidence = \"certain\"",
            "confidence",
        ),
        ("size = 1", "size = 9", "size"),
        ("source = \"fixture\"", "source = \" \"", "source"),
    ];
    for (old, new, message) in cases {
        let fixture = Fixture::new();
        fixture.write(
            "locations.toml",
            &fixture.text("locations.toml").replacen(old, new, 1),
        );
        let error = load_scenario(fixture.path())
            .expect_err("invalid location refused")
            .to_string();
        assert!(error.contains("locations.toml location["), "{error}");
        assert!(error.contains(message), "{error}");
    }
}

#[test]
fn duplicate_unknown_key_and_bad_radius_locations_are_refused() {
    let fixture = Fixture::new();
    fixture.write(
        "locations.toml",
        &fixture
            .text("locations.toml")
            .replace("id = \"town_b\"", "id = \"town_a\""),
    );
    assert!(
        load_scenario(fixture.path())
            .expect_err("duplicate id refused")
            .to_string()
            .contains("duplicate location id")
    );
    let fixture = Fixture::new();
    fixture.write(
        "locations.toml",
        &fixture.text("locations.toml").replace(
            "confidence = \"high\"",
            "confidence = \"high\"\nunknown = 1",
        ),
    );
    assert!(
        load_scenario(fixture.path())
            .expect_err("unknown location field refused")
            .to_string()
            .contains("locations.toml")
    );
    let fixture = Fixture::new();
    fixture.write(
        "locations.toml",
        &fixture.text("locations.toml").replace(
            "source = \"fixture\"",
            "source = \"fixture\"\ncatchment_m = 50001",
        ),
    );
    assert!(
        load_scenario(fixture.path())
            .expect_err("large override refused")
            .to_string()
            .contains("catchment_m")
    );
}

#[test]
fn location_placement_refuses_crop_sea_lake_and_duplicate_cells() {
    let land = lat_lon_for_cell(23_882, 23_883);
    let sea = lat_lon_for_cell(23_882, 23_882);
    let fixture = Fixture::new();
    let mut locations = fixture.text("locations.toml");
    locations = locations.replace(
        &format!("lat = {}\nlon = {}", land.0, land.1),
        &format!("lat = {}\nlon = {}", sea.0, sea.1),
    );
    fixture.write("locations.toml", &locations);
    assert!(
        load_scenario(fixture.path())
            .expect_err("sea centre refused")
            .to_string()
            .contains("sea")
    );

    let fixture = Fixture::new();
    let lake = lat_lon_for_cell(23_883, 23_882);
    let mut locations = fixture.text("locations.toml");
    locations = locations.replace(
        &format!("lat = {}\nlon = {}", land.0, land.1),
        &format!("lat = {}\nlon = {}", lake.0, lake.1),
    );
    fixture.write("locations.toml", &locations);
    assert!(
        load_scenario(fixture.path())
            .expect_err("lake centre refused")
            .to_string()
            .contains("lake")
    );

    let fixture = Fixture::new();
    let second = lat_lon_for_cell(23_882, 23_883);
    let mut locations = fixture.text("locations.toml");
    let target = lat_lon_for_cell(23_883, 23_883);
    locations = locations.replace(
        &format!("lat = {}\nlon = {}", target.0, target.1),
        &format!("lat = {}\nlon = {}", second.0, second.1),
    );
    fixture.write("locations.toml", &locations);
    assert!(
        load_scenario(fixture.path())
            .expect_err("duplicate cell refused")
            .to_string()
            .contains("shares cell")
    );

    let fixture = Fixture::new();
    let outside = lat_lon_for_cell(23_881, 23_883);
    fixture.write(
        "locations.toml",
        &fixture.text("locations.toml").replace(
            &format!("lat = {}\nlon = {}", land.0, land.1),
            &format!("lat = {}\nlon = {}", outside.0, outside.1),
        ),
    );
    let error = load_scenario(fixture.path())
        .expect_err("a point one cell beyond the crop is refused")
        .to_string();
    assert!(error.contains("locations.toml location[town_a]"), "{error}");
    assert!(error.contains("outside crop"), "{error}");
}

#[test]
fn kind_table_validates_ids_sizes_radii_glyphs_colours_and_unknown_keys() {
    let cases = [
        ("sizes = [1, 2]", "sizes = [2, 1]", "sizes"),
        (
            "catchment_m = [839, 2000]",
            "catchment_m = [839]",
            "exactly 2",
        ),
        (
            "catchment_m = [839, 2000]",
            "catchment_m = [0, 2000]",
            "1..=50000",
        ),
        (
            "glyph = [\".#.\", \"#d#\", \".#.\"]",
            "glyph = [\".#.\", \"##\"]",
            "rectangular",
        ),
        (
            "glyph = [\".#.\", \"#d#\", \".#.\"]",
            "glyph = [\".d.\"]",
            "at least one #",
        ),
        ("light = \"#d7c89b\"", "light = \"d7c89b\"", "#rrggbb"),
        ("id = \"town\"", "id = \"bad-id\"", "id must match"),
    ];
    for (old, new, message) in cases {
        let fixture = Fixture::new();
        fixture.write_map(
            "location_kinds.toml",
            &fixture
                .map_text("location_kinds.toml")
                .replacen(old, new, 1),
        );
        let error = load_scenario(fixture.path())
            .expect_err("invalid kind table refused")
            .to_string();
        assert!(error.contains("location_kinds.toml"), "{error}");
        assert!(error.contains(message), "{error}");
    }
}

#[test]
fn kind_table_refuses_duplicate_ids_and_all_size_radius_and_glyph_limits() {
    let cases = [
        ("sizes = [1, 2]", "sizes = [-1, 2]", "sizes"),
        ("sizes = [1, 2]", "sizes = [0, 2]", "sizes"),
        ("sizes = [1, 2]", "sizes = [1, 17]", "sizes"),
        (
            "catchment_m = [839, 2000]",
            "catchment_m = [839, 2000, 3000]",
            "exactly 2",
        ),
        (
            "catchment_m = [839, 2000]",
            "catchment_m = [839, 50001]",
            "1..=50000",
        ),
        (
            "glyph = [\".#.\", \"#d#\", \".#.\"]",
            "glyph = []",
            "1..=9 rows",
        ),
        (
            "glyph = [\".#.\", \"#d#\", \".#.\"]",
            "glyph = [\"##########\"]",
            "1..=9 characters",
        ),
        (
            "glyph = [\".#.\", \"#d#\", \".#.\"]",
            "glyph = [\"xxx\"]",
            "characters must be #, d or .",
        ),
        ("dark = \"#66583c\"", "dark = \"#GG583c\"", "#rrggbb"),
    ];
    for (old, new, message) in cases {
        let fixture = Fixture::new();
        fixture.write_map(
            "location_kinds.toml",
            &fixture
                .map_text("location_kinds.toml")
                .replacen(old, new, 1),
        );
        let error = load_scenario(fixture.path())
            .expect_err("invalid table entry refused")
            .to_string();
        assert!(error.contains("location_kinds.toml"), "{error}");
        assert!(error.contains(message), "{error}");
    }

    let fixture = Fixture::new();
    fixture.write_map(
        "location_kinds.toml",
        &fixture.map_text("location_kinds.toml").replace(
            "[[building]]",
            "[[kind]]\nid = \"town\"\nsizes = [1, 1]\ncatchment_m = [839]\nhouses = { count = [0], radius_cells_x256 = [0] }\nglyph = [\"#\"]\nlight = \"#ffffff\"\ndark = \"#000000\"\n\n[[building]]",
        ),
    );
    assert!(
        load_scenario(fixture.path())
            .expect_err("duplicate kind id refused")
            .to_string()
            .contains("duplicate kind id")
    );

    let fixture = Fixture::new();
    fixture.write_map(
        "location_kinds.toml",
        &fixture.map_text("location_kinds.toml").replace(
            "light = \"#ffffff\"\ndark = \"#000000\"",
            "light = \"#ffffff\"\ndark = \"#000000\"\n\n[[building]]\nid = \"church\"\nglyph = [\"#\"]\nlight = \"#ffffff\"\ndark = \"#000000\"",
        ),
    );
    assert!(
        load_scenario(fixture.path())
            .expect_err("duplicate building id refused")
            .to_string()
            .contains("duplicate building id")
    );
}

#[test]
fn house_rules_validate_lengths_and_inclusive_limits() {
    for (old, new, message) in [
        ("count = [2, 3]", "count = [2]", "houses.count"),
        ("count = [2, 3]", "count = [-1, 3]", "houses.count[0]"),
        ("count = [2, 3]", "count = [2, 201]", "houses.count[1]"),
        (
            "radius_cells_x256 = [256, 512]",
            "radius_cells_x256 = [256]",
            "houses.radius_cells_x256",
        ),
        (
            "radius_cells_x256 = [256, 512]",
            "radius_cells_x256 = [-1, 512]",
            "houses.radius_cells_x256[0]",
        ),
        (
            "radius_cells_x256 = [256, 512]",
            "radius_cells_x256 = [256, 2561]",
            "houses.radius_cells_x256[1]",
        ),
    ] {
        let fixture = Fixture::new();
        fixture.write_map(
            "location_kinds.toml",
            &fixture.map_text("location_kinds.toml").replace(old, new),
        );
        let error = load_scenario(fixture.path())
            .expect_err("invalid house rule refused")
            .to_string();
        assert!(error.contains(message), "{error}");
    }
}

#[test]
fn ten_row_glyph_refusal_names_its_table_entry() {
    let fixture = Fixture::new();
    let ten_rows = "glyph = [\"#\", \"#\", \"#\", \"#\", \"#\", \"#\", \"#\", \"#\", \"#\", \"#\"]";
    fixture.write_map(
        "location_kinds.toml",
        &fixture.map_text("location_kinds.toml").replacen(
            "glyph = [\".#.\", \"#d#\", \".#.\"]",
            ten_rows,
            1,
        ),
    );
    let error = load_scenario(fixture.path())
        .expect_err("ten glyph rows are refused")
        .to_string();
    assert!(
        error.contains("location_kinds.toml kind[town].glyph"),
        "{error}"
    );
    assert!(error.contains("1..=9 rows"), "{error}");
}

#[test]
fn locations_require_a_kinds_table() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.map_path("fixture").join("location_kinds.toml")).expect("remove kinds");
    let error = load_scenario(fixture.path())
        .expect_err("locations without a kinds table refused")
        .to_string();
    assert!(error.contains("location_kinds.toml kind"), "{error}");
}

fn map_data(origin: Cell, width: u32, height: u32) -> MapData {
    let count = usize::try_from(width * height).expect("small test crop");
    MapData {
        crop: Crop {
            origin,
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
        elevations: vec![11_000; count],
        terrains: vec![TerrainStyle {
            solid: Rgba8::opaque(1, 2, 3),
            second: Rgba8::opaque(1, 2, 3),
            dark: Rgba8::opaque(1, 2, 3),
            dark_second: Rgba8::opaque(1, 2, 3),
            cap: Rgba8::opaque(1, 2, 3),
            high_ground: Rgba8::opaque(1, 2, 3),
            relief_low: 0,
            relief_high: 1,
            mix_percent: 0,
            cap_low: 0,
            cap_high: 1,
            mountain: false,
        }],
        sea: Rgba8::opaque(0, 0, 0),
        shallow_sea: Rgba8::opaque(0, 0, 0),
        lake: Rgba8::opaque(0, 0, 0),
        river: Rgba8::opaque(0, 0, 0),
        shallow_threshold: 1,
    }
}

fn location(id: &str, cell: Cell, radius: Option<u32>) -> Location {
    Location {
        id: id.to_owned(),
        name: id.to_owned(),
        cell,
        kind_index: 0,
        size: 1,
        building_indices: vec![],
        catchment_m: radius,
        source: "test".to_owned(),
        confidence: Confidence::High,
    }
}

fn kind(radius: u32) -> LocationKinds {
    LocationKinds {
        kinds: vec![LocationKind {
            id: "town".into(),
            min_size: 1,
            max_size: 1,
            catchment_m: vec![radius],
            house_count: vec![2],
            house_radius_cells_x256: vec![256],
            glyph: vec![vec!['#']],
            light: Rgba8::opaque(255, 0, 0),
            dark: Rgba8::opaque(0, 0, 255),
        }],
        buildings: vec![BuildingKind {
            id: "church".into(),
            glyph: vec![vec!['#']],
            light: Rgba8::opaque(255, 255, 255),
            dark: Rgba8::opaque(0, 0, 0),
        }],
        house: HouseStyle::default(),
    }
}

#[test]
fn catchments_match_independent_brute_force_and_include_exact_radius_boundary() {
    let mut data = map_data(
        Cell {
            x: 23_881,
            y: 23_881,
        },
        5,
        5,
    );
    data.cells[11] = Surface::Sea { depth: 1 };
    data.cells[13] = Surface::Lake;
    let place = location(
        "center",
        Cell {
            x: 23_883,
            y: 23_883,
        },
        Some(839),
    );
    let kinds = kind(1);
    let actual = compute_catchments(&data, std::slice::from_ref(&place), &kinds)
        .expect("catchment computes");
    let s = i128::from(ground_scales(data.crop)[2]);
    assert_eq!(s, 65_536);
    let mut expected = vec![None; 25];
    for y in 0..5_i64 {
        for x in 0..5_i64 {
            let di = i128::from(x - 2);
            let dj = i128::from(y - 2);
            let dx = 839_i128 * di;
            let dy = 839_i128 * dj;
            let q = (dx * dx + dy * dy) * s * s;
            if q <= 839_i128 * 839 * (1_i128 << 32) {
                expected[usize::try_from(y * 5 + x).unwrap()] = Some(0);
            }
        }
    }
    assert_eq!(actual, expected);
    assert_eq!(
        actual[2 * 5 + 3],
        Some(0),
        "east cell is exactly on the inclusive radius"
    );
    assert_eq!(actual[3 * 5 + 3], None, "diagonal is beyond the radius");
    assert_eq!(actual[2 * 5 + 1], Some(0), "sea is included in catchments");
    assert_eq!(
        actual[2 * 5 + 3],
        Some(0),
        "lakes are included in catchments"
    );
}

#[test]
fn unequal_row_overlap_uses_each_locations_scale_and_file_order_breaks_exact_ties() {
    let crop = Crop {
        origin: Cell { x: 100, y: 14_979 },
        width: 3,
        height: 3,
    };
    let data = map_data(crop.origin, crop.width, crop.height);
    let scales = ground_scales(crop);
    assert_eq!(scales, [37_071, 37_075, 37_079]);
    let north = location("north", Cell { x: 100, y: 14_979 }, Some(50_000));
    let south = location("south", Cell { x: 102, y: 14_981 }, Some(50_000));
    let north_q = (839_i128 * 839 + 839_i128 * 839) * 37_071_i128 * 37_071;
    let south_q = (839_i128 * 839 + 839_i128 * 839) * 37_079_i128 * 37_079;
    assert_eq!(north_q, 1_934_739_596_799_522_i128);
    assert_eq!(south_q, 1_935_574_728_673_922_i128);
    assert!(
        north_q < south_q,
        "each location's row scale changes the winner"
    );
    let owners = compute_catchments(&data, &[south, north], &kind(50_000))
        .expect("overlap computes with south first in file order");
    assert_eq!(
        owners[4],
        Some(1),
        "north has lower q at its own row scale, despite being later in file order"
    );

    let tie_crop = Crop {
        origin: Cell { x: 100, y: 14_980 },
        width: 3,
        height: 1,
    };
    let tie_data = map_data(tie_crop.origin, tie_crop.width, tie_crop.height);
    let first = location("first", Cell { x: 100, y: 14_980 }, Some(50_000));
    let second = location("second", Cell { x: 102, y: 14_980 }, Some(50_000));
    let tied =
        compute_catchments(&tie_data, &[first, second], &kind(50_000)).expect("tie computes");
    assert_eq!(tied[1], Some(0), "equal q stays with the first file entry");
}

#[test]
fn largest_radius_is_bounded_without_overflow() {
    let data = map_data(Cell { x: 0, y: 14_979 }, 4, 4);
    assert_eq!(ground_scales(data.crop)[0], 37_071);
    let location = location("polar", Cell { x: 1, y: 14_980 }, Some(50_000));
    let owners =
        compute_catchments(&data, &[location], &kind(50_000)).expect("maximum radius fits i128");
    assert!(owners.iter().any(Option::is_some));
}

#[test]
fn markers_obey_zoom_center_outline_overlap_and_view_selection() {
    let glyph = |rows: &[&str]| rows.iter().map(|row| row.chars().collect()).collect();
    let kinds = LocationKinds {
        kinds: vec![
            LocationKind {
                id: "town".into(),
                min_size: 1,
                max_size: 1,
                catchment_m: vec![1],
                house_count: vec![0],
                house_radius_cells_x256: vec![0],
                glyph: glyph(&[".#.", "#d#", ".#."]),
                light: Rgba8::opaque(255, 0, 0),
                dark: Rgba8::opaque(0, 0, 255),
            },
            LocationKind {
                id: "later_town".into(),
                min_size: 1,
                max_size: 1,
                catchment_m: vec![1],
                house_count: vec![0],
                house_radius_cells_x256: vec![0],
                glyph: glyph(&[".#.", "#d#", ".#."]),
                light: Rgba8::opaque(0, 255, 0),
                dark: Rgba8::opaque(255, 255, 0),
            },
        ],
        buildings: vec![],
        house: HouseStyle::default(),
    };
    let place = location("town", Cell { x: 10, y: 10 }, None);
    assert!(prepare_markers(std::slice::from_ref(&place), &kinds, 14).is_empty());
    let step = prepare_markers(std::slice::from_ref(&place), &kinds, 15);
    let size = i64::from(STEP_SIZE[15]);
    let center = WorldPixel {
        x: i32::try_from(
            (10_i64 * crate::FIXED_PER_CELL + crate::FIXED_PER_CELL / 2).div_euclid(size),
        )
        .unwrap(),
        y: i32::try_from(
            (10_i64 * crate::FIXED_PER_CELL + crate::FIXED_PER_CELL / 2).div_euclid(size),
        )
        .unwrap(),
    };
    let at = |x, y| {
        step.iter()
            .find(|pixel| pixel.pixel == WorldPixel { x, y })
            .map(|pixel| pixel.colour)
    };
    assert_eq!(at(center.x, center.y), Some(Rgba8::opaque(0, 0, 255)));
    assert_eq!(
        at(center.x - 1, center.y - 1),
        Some(Rgba8::opaque(0, 0, 255)),
        "outline sits on four-neighbours of # pixels"
    );
    assert!(at(center.x - 1, center.y).is_some());
    let overlapping = Location {
        id: "later".into(),
        kind_index: 1,
        ..place.clone()
    };
    let overlap = prepare_markers(&[place, overlapping], &kinds, 15);
    assert_eq!(
        overlap
            .iter()
            .find(|pixel| pixel.pixel == center)
            .map(|pixel| pixel.colour),
        Some(Rgba8::opaque(255, 255, 0)),
        "the overlapping center uses the later marker's distinct dark colour"
    );
    let selected =
        super::PreparedMarkers::new(vec![location("town", Cell { x: 10, y: 10 }, None)], kinds)
            .select_view(
                15,
                WorldPixel {
                    x: center.x - 2,
                    y: center.y - 2,
                },
                crate::ViewSize {
                    width: 5,
                    height: 5,
                },
            )
            .expect("valid step");
    assert_eq!(selected.len(), step.len());
    assert!(
        super::PreparedMarkers::default()
            .select_view(76, WorldPixel::default(), crate::ViewSize::default())
            .is_none()
    );
}

#[test]
fn even_sized_marker_places_its_lower_right_cell_on_the_location_pixel() {
    let kinds = LocationKinds {
        kinds: vec![LocationKind {
            id: "town".into(),
            min_size: 1,
            max_size: 1,
            catchment_m: vec![1],
            house_count: vec![0],
            house_radius_cells_x256: vec![0],
            glyph: vec![vec!['#', '.'], vec!['.', '#']],
            light: Rgba8::opaque(200, 100, 20),
            dark: Rgba8::opaque(10, 20, 30),
        }],
        buildings: vec![],
        house: HouseStyle::default(),
    };
    let center = location("town", Cell { x: 10, y: 20 }, None);
    let size = i64::from(STEP_SIZE[15]);
    let center_pixel = WorldPixel {
        x: i32::try_from(
            (10_i64 * crate::FIXED_PER_CELL + crate::FIXED_PER_CELL / 2).div_euclid(size),
        )
        .unwrap(),
        y: i32::try_from(
            (20_i64 * crate::FIXED_PER_CELL + crate::FIXED_PER_CELL / 2).div_euclid(size),
        )
        .unwrap(),
    };
    let pixels = prepare_markers(&[center], &kinds, 15);
    assert!(
        pixels
            .iter()
            .any(|pixel| pixel.pixel == center_pixel && pixel.colour == kinds.kinds[0].light)
    );
}

#[test]
fn location_placement_vectors_and_zoom_thresholds_are_frozen() {
    let cell = Cell { x: 10, y: 20 };
    let place = location("town_a", cell, None);
    let kinds = kind(1);
    let offsets = [
        (-13_395, 17_278),
        (-13_155, 17_329),
        (-28_615, -12_419),
        (14_123, 17_126),
    ];
    for (index, expected) in offsets.into_iter().enumerate() {
        assert_eq!(
            super::accepted_offset(
                place.id.as_bytes(),
                u32::try_from(index).unwrap(),
                32_768,
                0x484f_5553,
                0x484f_5554,
            ),
            Some(expected),
            "house {index} frozen fixed-point offset"
        );
    }
    assert_eq!(
        super::accepted_offset(place.id.as_bytes(), 0, 32_768, 0x424c_4447, 0x424c_4448,),
        Some((22_452, 13_771)),
        "building 0 frozen offset"
    );
    assert_eq!(
        offsets.map(|(x, y)| super::location_pixel(cell, x, y, i64::from(STEP_SIZE[46]))),
        [
            WorldPixel { x: 40, y: 84 },
            WorldPixel { x: 40, y: 84 },
            WorldPixel { x: 38, y: 81 },
            WorldPixel { x: 44, y: 84 },
        ],
        "house anchors are the world pixels containing their fixed-point positions"
    );

    let mut marker_only = kinds.clone();
    marker_only.kinds[0].house_count = vec![1];
    marker_only.kinds[0].house_radius_cells_x256 = vec![0];
    marker_only.kinds[0].glyph = vec![vec!['#']];
    marker_only.buildings.clear();
    let at_14 = prepare_locations(std::slice::from_ref(&place), &marker_only, crop(), 14);
    let at_15 = prepare_locations(std::slice::from_ref(&place), &marker_only, crop(), 15);
    let at_45 = prepare_locations(std::slice::from_ref(&place), &marker_only, crop(), 45);
    assert!(at_14.is_empty());
    assert_eq!(
        at_15.len(),
        5,
        "the marker and outline first appear at step 15"
    );
    assert_eq!(at_45.len(), 5, "houses remain hidden through step 45");
    assert!(
        at_45.iter().all(|pixel| pixel.palette_index < 2),
        "nonzero house counts still produce markers only at step 45"
    );
    let at_46 = prepare_locations(std::slice::from_ref(&place), &marker_only, crop(), 46);
    assert_eq!(
        at_46.len(),
        6,
        "the 2x2 house appears at step 46 below the marker outline"
    );
    assert!(
        at_46.iter().any(|pixel| pixel.palette_index >= 2),
        "step 46 includes a house pixel"
    );
}

#[test]
fn location_readback_fixture_produces_marker_building_and_house_pixels() {
    let scenario = crate::fixture::location_scenario();
    let marker = prepare_locations(
        &scenario.locations,
        &scenario.location_kinds,
        scenario.map.data.crop,
        15,
    );
    assert!(marker.iter().any(|pixel| pixel.palette_index < 4));

    let step46 = prepare_locations(
        &scenario.locations,
        &scenario.location_kinds,
        scenario.map.data.crop,
        46,
    );
    assert!(
        step46
            .iter()
            .any(|pixel| (4..6).contains(&pixel.palette_index)),
        "the building has prepared pixels: {step46:?}"
    );
    assert!(
        step46
            .iter()
            .any(|pixel| (6..9).contains(&pixel.palette_index)),
        "the fixture has prepared house pixels: {step46:?}"
    );
}

#[test]
fn location_readback_fixture_cpu_reference_crosses_a_prepared_house_pixel() {
    use std::collections::{BTreeMap, BTreeSet};

    let scenario = crate::fixture::location_scenario();
    let step = 46;
    let crop_corner = crate::fixture::Fixture::crop_top_left(step);
    let top_left = WorldPixel {
        x: crop_corner.x - 2,
        y: crop_corner.y - 2,
    };
    let view = crate::ViewSize {
        width: 99,
        height: 67,
    };
    let mut prepared = PreparedLocations::new(
        scenario.locations.clone(),
        scenario.location_kinds.clone(),
        scenario.map.data.crop,
    );
    let locations = prepared
        .select_view(step, top_left, view)
        .expect("fixture scenario step is supported");
    let world_pixel = |[x, y]: [u32; 2]| WorldPixel {
        x: top_left.x + i32::try_from(x).expect("fixture x fits"),
        y: top_left.y + i32::try_from(y).expect("fixture y fits"),
    };
    let house = locations
        .iter()
        .find(|(_, index)| (6..9).contains(index))
        .copied()
        .expect("step 46 has a prepared house pixel");
    let house_world = world_pixel(house.0);
    let mut rivers = crate::PreparedRivers::new(vec![
        crate::fixture::river_through_pixel(house_world, step)
            .expect("fixture river fits fixed-point coordinates"),
    ]);
    let river_pixels = rivers
        .select_view(step, top_left, view)
        .expect("fixture scenario step is supported")
        .into_iter()
        .map(world_pixel)
        .collect::<BTreeSet<_>>();
    assert!(
        river_pixels.contains(&house_world),
        "the river crosses a prepared house pixel"
    );

    let palette = scenario
        .location_kinds
        .overlay_palette()
        .into_iter()
        .collect::<Vec<_>>();
    let location_colours = locations
        .iter()
        .map(|([x, y], index)| (world_pixel([*x, *y]), palette[usize::from(*index)]))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        crate::reference_colour_with_locations(
            &scenario.map.data,
            step,
            house_world,
            &river_pixels,
            &location_colours,
        ),
        Some(scenario.map.data.river),
        "the CPU reference draws the river over the house pixel"
    );
    let visible_house = locations
        .iter()
        .find(|([x, y], index)| {
            (6..9).contains(index) && !river_pixels.contains(&world_pixel([*x, *y]))
        })
        .unwrap_or_else(|| {
            panic!("another prepared house pixel remains visible beside the river; locations={locations:?}, rivers={river_pixels:?}")
        });
    assert_eq!(
        crate::reference_colour_with_locations(
            &scenario.map.data,
            step,
            world_pixel(visible_house.0),
            &river_pixels,
            &location_colours,
        ),
        Some(palette[usize::from(visible_house.1)]),
        "the CPU reference keeps an uncovered house pixel visible"
    );
}

#[test]
fn prepared_locations_later_overlapping_marker_replaces_the_earlier_location() {
    let mut kinds = kind(1);
    kinds.kinds[0].house_count = vec![0];
    kinds.kinds[0].glyph = vec![vec!['#']];
    let mut later_kind = kinds.kinds[0].clone();
    later_kind.id = "later_town".into();
    later_kind.glyph = vec![vec!['d']];
    later_kind.light = Rgba8::opaque(0, 255, 0);
    later_kind.dark = Rgba8::opaque(255, 255, 0);
    kinds.kinds.push(later_kind);

    let first = location("first", Cell { x: 10, y: 20 }, None);
    let later = Location {
        id: "later".into(),
        kind_index: 1,
        ..first.clone()
    };
    let center = super::location_pixel(first.cell, 0, 0, i64::from(STEP_SIZE[15]));
    let pixels = prepare_locations(&[first, later], &kinds, crop(), 15);
    assert_eq!(
        pixels
            .iter()
            .find(|pixel| pixel.pixel == center)
            .map(|pixel| pixel.palette_index),
        Some(3),
        "the later location's marker colour replaces the earlier marker in the same layer"
    );
}

#[test]
fn location_kinds_refuse_an_overlay_palette_over_256_colours() {
    let fixture = Fixture::new();
    let mut table = String::new();
    for index in 0..127 {
        let id = if index == 0 {
            "town".to_owned()
        } else {
            format!("kind_{index}")
        };
        writeln!(
            table,
            "[[kind]]\nid = \"{id}\"\nsizes = [1, 1]\ncatchment_m = [839]\nhouses = {{ count = [0], radius_cells_x256 = [0] }}\nglyph = [\"#\"]\nlight = \"#ffffff\"\ndark = \"#000000\"\n"
        )
        .expect("write fixture kind");
    }
    table.push_str(
        "[[building]]\nid = \"church\"\nglyph = [\"#\"]\nlight = \"#ffffff\"\ndark = \"#000000\"\n\n[house]\nroof = \"#a9563b\"\nlight = \"#ece3cb\"\ndark = \"#2a2119\"\n",
    );
    fixture.write_map("location_kinds.toml", &table);

    let error = load_scenario(fixture.path())
        .expect_err("259-colour overlay palette is refused")
        .to_string();
    assert!(error.contains("overlay_palette"), "{error}");
    assert!(error.contains("more than 256 colours"), "{error}");
}

fn crop() -> Crop {
    Crop {
        origin: Cell { x: -100, y: -100 },
        width: 300,
        height: 300,
    }
}

#[test]
fn locations_apply_house_marker_building_precedence_and_cache_view_selection() {
    let cell = Cell { x: 10, y: 20 };
    let mut place = location("town_a", cell, None);
    place.building_indices = vec![0];
    let mut kinds = kind(1);
    kinds.kinds[0].house_count = vec![1];
    kinds.kinds[0].house_radius_cells_x256 = vec![0];
    kinds.kinds[0].glyph = vec![vec!['#']];
    kinds.buildings[0].glyph = vec![vec!['#']];
    let center = super::location_pixel(cell, 0, 0, i64::from(STEP_SIZE[46]));
    let pixels = prepare_locations(&[place.clone()], &kinds, crop(), 46);
    let center_pixel = pixels
        .iter()
        .find(|pixel| pixel.pixel == center)
        .expect("building glyph covers the marker at its anchor");
    assert_eq!(center_pixel.palette_index, 2, "building is above marker");
    assert!(
        pixels.iter().any(|pixel| {
            pixel.pixel
                == WorldPixel {
                    x: center.x + 1,
                    y: center.y,
                }
                && pixel.palette_index == 3
        }),
        "the building outline is drawn over the house and marker outlines"
    );
    assert!(
        pixels.iter().any(|pixel| {
            pixel.pixel
                == WorldPixel {
                    x: center.x + 1,
                    y: center.y + 1,
                }
                && pixel.palette_index == 6
        }),
        "the diagonal house pixel remains visible outside the marker outline"
    );

    let mut house_kinds = kind(1);
    house_kinds.kinds[0].house_count = vec![1];
    house_kinds.kinds[0].glyph = vec![vec!['#']];
    let house_only = location("town_a", cell, None);
    let house_pixels = prepare_locations(&[house_only], &house_kinds, crop(), 46);
    assert!(
        house_pixels.iter().any(|pixel| {
            pixel.pixel == WorldPixel { x: 40, y: 84 } && pixel.palette_index == 4
        })
    );
    assert!(
        house_pixels.iter().any(|pixel| {
            pixel.pixel == WorldPixel { x: 40, y: 85 } && pixel.palette_index == 5
        })
    );

    let mut cached = PreparedLocations::new(vec![place], kinds, crop());
    let first_ptr = cached.pixels(46).unwrap().as_ptr();
    let second_ptr = cached.pixels(46).unwrap().as_ptr();
    assert_eq!(
        first_ptr, second_ptr,
        "a step is rasterized once and cached"
    );
    let view = cached
        .select_view(
            46,
            WorldPixel {
                x: center.x - 1,
                y: center.y - 1,
            },
            crate::ViewSize {
                width: 3,
                height: 3,
            },
        )
        .expect("valid step");
    assert!(view.contains(&([1, 1], 2)));
    assert!(
        cached
            .select_view(76, WorldPixel::default(), crate::ViewSize::default())
            .is_none()
    );
}

#[test]
fn building_fallback_and_house_skip_have_a_test_only_attempt_seam() {
    let house = super::accepted_offset_from_attempts(2_304, |_| Some((2_304, 2_304)));
    assert_eq!(
        house, None,
        "all eight out-of-disc house attempts are skipped"
    );

    let building = super::building_offset_from_attempts(2_304, |attempt| {
        Some(if attempt == 0 {
            (-4_026, -4_253)
        } else {
            (2_304, 2_304)
        })
    });
    assert_eq!(
        building,
        Some((-2_013, -2_126)),
        "the building uses the first offsets halved toward zero"
    );
    assert_eq!(super::building_offset(b"zero-radius", 0, 0), Some((0, 0)));
}
