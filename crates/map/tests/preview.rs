//! End-to-end contract tests for the `map-preview` command.

use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use ck_map::{
    PreparedMarkers, RiverCoverage, STEP_SIZE, WorldPixel, load_map, load_scenario, project,
    reference_colour,
};
use png::{BitDepth, ColorType};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ck-map-preview-{label}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ScenarioFixture {
    _directory: TempDir,
    scenario: PathBuf,
}

impl ScenarioFixture {
    fn path(&self) -> &Path {
        &self.scenario
    }
}

fn fixture() -> TempDir {
    let directory = TempDir::new("fixture");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-data/valid");
    for name in [
        "map.toml",
        "terrain.toml",
        "rivers.toml",
        "terrain.png",
        "usable.png",
        "elevation.png",
    ] {
        fs::copy(source.join(name), directory.path().join(name)).expect("copy map fixture");
    }
    directory
}

fn scenario_fixture() -> ScenarioFixture {
    let directory = TempDir::new("scenario");
    let scenario = directory.path().join("scenarios/preview_test");
    let map = directory.path().join("map/fixture");
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
    fs::write(scenario.join("scenario.toml"), "id = \"preview_test\"\nmap = \"fixture\"\nlocations = \"locations.toml\"\n[start]\nyear = 768\nmonth = 10\nday = 9\nhour = 0\n").expect("write scenario");
    fs::write(map.join("location_kinds.toml"), "[[kind]]\nid = \"town\"\nsizes = [1, 1]\ncatchment_m = [1000]\nglyph = [\".#.\", \"#d#\", \".#.\"]\nlight = \"#d7c89b\"\ndark = \"#66583c\"\n").expect("write kinds");
    let (latitude, longitude) = lat_lon_for_cell(23_883, 23_883);
    fs::write(scenario.join("locations.toml"), format!("[[location]]\nid = \"test_town\"\nname = \"Test Town\"\nlat = {latitude}\nlon = {longitude}\nkind = \"town\"\nsize = 1\nbuildings = []\nsource = \"fixture\"\nconfidence = \"medium\"\n")).expect("write location");
    ScenarioFixture {
        _directory: directory,
        scenario,
    }
}

fn lat_lon_for_cell(x: i32, y: i32) -> (f64, f64) {
    let radius = 6_378_137.0_f64;
    let half = std::f64::consts::PI * radius;
    let x_metres = (f64::from(x) + 0.5) * 839.0;
    let y_metres = (f64::from(y) + 0.5) * 839.0;
    (
        ((2.0 * ((half - y_metres) / radius).exp().atan() - std::f64::consts::FRAC_PI_2)
            .to_degrees()),
        ((x_metres - half) / radius).to_degrees(),
    )
}

fn run_preview(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_map-preview"))
        .args(args)
        .output()
        .expect("run map-preview")
}

#[test]
fn rendered_png_matches_the_reference_for_every_pixel_and_html_lists_steps() {
    let input = fixture();
    let output = TempDir::new("rendered");
    let result = run_preview(&[
        input.path().to_str().expect("fixture path"),
        output.path().to_str().expect("output path"),
        "--steps",
        "0,75",
        "--view",
        "0,0",
        "80x60",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let map = load_map(input.path()).expect("fixture loads");
    let html = fs::read_to_string(output.path().join("index.html")).expect("read HTML");
    assert_eq!(html.matches("<section>").count(), 2);
    assert!(html.contains("Step 0"));
    assert!(html.contains("Step 75"));
    assert!(html.contains("image-rendering: pixelated"));

    for step in [0_u8, 75] {
        let name = format!("step-{step}.png");
        let (width, height, bytes) = decode_png(&output.path().join(&name));
        assert_eq!((width, height), (80, 60));
        let center = project(0.0, 0.0).expect("finite fixture centre");
        let pixel_size = i64::from(STEP_SIZE[usize::from(step)]);
        let top_left = WorldPixel {
            x: i32::try_from(i64::from(center.x).div_euclid(pixel_size) - 40).expect("x range"),
            y: i32::try_from(i64::from(center.y).div_euclid(pixel_size) - 30).expect("y range"),
        };
        let rivers = RiverCoverage::prepare(&map.rivers, step).expect("prepare river pixels");
        let mut expected = Vec::with_capacity(bytes.len());
        for y in 0..height {
            for x in 0..width {
                let world = WorldPixel {
                    x: top_left.x + i32::try_from(x).expect("x coordinate"),
                    y: top_left.y + i32::try_from(y).expect("y coordinate"),
                };
                let colour = reference_colour(&map.data, step, world, rivers.pixels())
                    .expect("reference colour");
                expected.extend_from_slice(&colour.0);
            }
        }
        assert_eq!(
            bytes, expected,
            "step {step} output differs from the reference"
        );
    }
}

#[test]
fn loader_error_is_forwarded_and_no_output_is_written() {
    let input = fixture();
    let source = fs::read_to_string(input.path().join("terrain.toml")).expect("read terrain");
    fs::write(
        input.path().join("terrain.toml"),
        source.replace("shallow_threshold = 20", "shallow_threshold = 0"),
    )
    .expect("invalidate fixture");
    let output = TempDir::new("invalid-output");
    let expected = load_map(input.path())
        .expect_err("invalid map is rejected")
        .to_string();
    let result = run_preview(&[
        input.path().to_str().expect("fixture path"),
        output.path().to_str().expect("output path"),
        "--steps",
        "0",
    ]);
    assert!(!result.status.success());
    assert_eq!(String::from_utf8_lossy(&result.stderr).trim(), expected);
    assert_eq!(fs::read_dir(output.path()).expect("read output").count(), 0);
}

#[test]
fn scenario_preview_writes_markers_catchments_and_draws_rivers_last() {
    let input = scenario_fixture();
    let output = TempDir::new("scenario-preview");
    let (latitude, longitude) = lat_lon_for_cell(23_883, 23_883);
    let result = run_preview(&[
        output.path().to_str().expect("output path"),
        "--scenario",
        input.path().to_str().expect("scenario path"),
        "--steps",
        "14,15",
        "--catchments",
        "--view",
        &format!("{latitude},{longitude}"),
        "16x16",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.path().join("catchments.png").is_file());
    let catchments = decode_png(&output.path().join("catchments.png"));
    assert_eq!((catchments.0, catchments.1), (2, 2));

    let scenario = load_scenario(input.path()).expect("scenario fixture loads");
    let mut markers =
        PreparedMarkers::new(scenario.locations.clone(), scenario.location_kinds.clone());
    assert!(markers.pixels(14).expect("valid step").is_empty());
    let marker_pixels = markers.pixels(15).expect("valid step").to_vec();
    let rivers = RiverCoverage::prepare(&scenario.map.rivers, 15).expect("river pixels");
    let overlap = marker_pixels
        .iter()
        .find(|marker| rivers.pixels().contains(&marker.pixel))
        .expect("fixture marker overlaps the river");
    let view_center = project(latitude, longitude).expect("projected view centre");
    let top_left = WorldPixel {
        x: i32::try_from(i64::from(view_center.x).div_euclid(i64::from(STEP_SIZE[15])) - 8)
            .expect("view x"),
        y: i32::try_from(i64::from(view_center.y).div_euclid(i64::from(STEP_SIZE[15])) - 8)
            .expect("view y"),
    };
    let x = u32::try_from(i64::from(overlap.pixel.x) - i64::from(top_left.x))
        .expect("visible overlap x");
    let y = u32::try_from(i64::from(overlap.pixel.y) - i64::from(top_left.y))
        .expect("visible overlap y");
    let (_, _, bytes) = decode_png(&output.path().join("step-15.png"));
    let index = (usize::try_from(y).unwrap() * 16 + usize::try_from(x).unwrap()) * 4;
    assert_eq!(
        &bytes[index..index + 4],
        &scenario.map.data.river.0,
        "river pixels are drawn after marker pixels"
    );
}

#[test]
fn whole_crop_step_75_refuses_oversize_before_writing_but_view_succeeds() {
    let input = large_fixture(300, 300);
    let rejected = TempDir::new("oversize-output");
    let result = run_preview(&[
        input.path().to_str().expect("fixture path"),
        rejected.path().to_str().expect("output path"),
        "--steps",
        "75",
    ]);
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("requested image"), "{error}");
    assert!(
        error.contains("allowed size is at most 8192x8192"),
        "{error}"
    );
    assert!(error.contains("provide --view"), "{error}");
    assert_eq!(
        fs::read_dir(rejected.path())
            .expect("read rejected output")
            .count(),
        0
    );

    let accepted = TempDir::new("view-output");
    let result = run_preview(&[
        input.path().to_str().expect("fixture path"),
        accepted.path().to_str().expect("output path"),
        "--steps",
        "75",
        "--view",
        "0,0",
        "32x24",
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(decode_png(&accepted.path().join("step-75.png")).0, 32);
    assert!(accepted.path().join("index.html").is_file());
}

fn large_fixture(width: u32, height: u32) -> TempDir {
    let directory = fixture();
    let mut map = fs::read_to_string(directory.path().join("map.toml")).expect("read map");
    map = map.replace(
        "width = 2\nheight = 2",
        &format!("width = {width}\nheight = {height}"),
    );
    fs::write(directory.path().join("map.toml"), map).expect("grow crop metadata");

    let pixel_count =
        usize::try_from(width).expect("width") * usize::try_from(height).expect("height");
    let mut terrain = Vec::with_capacity(pixel_count * 3);
    let mut usable = Vec::with_capacity(pixel_count * 3);
    for _ in 0..pixel_count {
        terrain.extend_from_slice(&[0, 255, 0]);
        usable.extend_from_slice(&[0, 255, 0]);
    }
    write_png(
        &directory.path().join("terrain.png"),
        width,
        height,
        ColorType::Rgb,
        BitDepth::Eight,
        &terrain,
    );
    write_png(
        &directory.path().join("usable.png"),
        width,
        height,
        ColorType::Rgb,
        BitDepth::Eight,
        &usable,
    );
    let mut elevations = Vec::with_capacity(pixel_count * 2);
    for _ in 0..pixel_count {
        elevations.extend_from_slice(&21_000_u16.to_be_bytes());
    }
    write_png(
        &directory.path().join("elevation.png"),
        width,
        height,
        ColorType::Grayscale,
        BitDepth::Sixteen,
        &elevations,
    );
    directory
}

fn write_png(
    path: &Path,
    width: u32,
    height: u32,
    colour: ColorType,
    depth: BitDepth,
    bytes: &[u8],
) {
    let file = fs::File::create(path).expect("create fixture PNG");
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(colour);
    encoder.set_depth(depth);
    encoder
        .write_header()
        .expect("write fixture PNG header")
        .write_image_data(bytes)
        .expect("write fixture PNG data");
}

fn decode_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let file = fs::File::open(path).expect("open output PNG");
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder.read_info().expect("read output PNG header");
    let mut bytes = vec![0; reader.output_buffer_size().expect("PNG output size")];
    let frame = reader.next_frame(&mut bytes).expect("decode output PNG");
    bytes.truncate(frame.buffer_size());
    assert_eq!(frame.color_type, ColorType::Rgba);
    assert_eq!(frame.bit_depth, BitDepth::Eight);
    (frame.width, frame.height, bytes)
}
