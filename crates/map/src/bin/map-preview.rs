//! Render exact reference-colour previews of authored maps.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use ck_map::{
    CELL_METRES, FIXED_PER_CELL, FixedPoint, LoadedMap, PreparedMarkers, Rgba8, RiverCoverage,
    STEP_SIZE, Scenario, Surface, ViewSize, WorldPixel, load_map, load_scenario, project,
    reference_colour,
};
use png::{BitDepth, ColorType};

const MAX_IMAGE_DIMENSION: u32 = 8192;

#[derive(Clone, Copy)]
struct View {
    center: FixedPoint,
    size: ViewSize,
}

struct RenderPlan {
    step: u8,
    top_left: WorldPixel,
    size: ViewSize,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    let scenario = args
        .scenario_directory
        .as_ref()
        .map(load_scenario)
        .transpose()?;
    let loaded_map = args.map_directory.as_ref().map(load_map).transpose()?;
    let map = scenario
        .as_ref()
        .map(|scenario| &scenario.map)
        .or(loaded_map.as_ref())
        .expect("one map source was parsed");
    let plans = make_plans(map, &args.steps, args.view)?;

    fs::create_dir_all(&args.output_directory)?;
    let mut markers = scenario.as_ref().and_then(|scenario| {
        (!scenario.location_kinds.kinds.is_empty()).then(|| {
            PreparedMarkers::new(scenario.locations.clone(), scenario.location_kinds.clone())
        })
    });
    let mut entries = Vec::with_capacity(plans.len());
    for plan in plans {
        let name = format!("step-{}.png", plan.step);
        if let (Some(scenario), Some(markers)) = (&scenario, &mut markers) {
            render_scenario_png(scenario, markers, &args.output_directory.join(&name), &plan)?;
        } else {
            render_png(map, &args.output_directory.join(&name), &plan)?;
        }
        let metres_per_pixel = f64::from(STEP_SIZE[usize::from(plan.step)]) * CELL_METRES
            / f64::from(i32::try_from(FIXED_PER_CELL)?);
        entries.push(format!(
            "<section><h2>Step {} — {:.3} metres per map pixel</h2><img src=\"{}\" alt=\"Map preview at step {}\" width=\"{}\" height=\"{}\"></section>",
            plan.step,
            metres_per_pixel,
            name,
            plan.step,
            plan.size.width * 4,
            plan.size.height * 4
        ));
    }
    if args.catchments {
        let scenario = scenario
            .as_ref()
            .ok_or("--catchments requires --scenario")?;
        render_catchments_png(scenario, &args.output_directory.join("catchments.png"))?;
    }
    let html = format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>Map preview</title><style>img {{ image-rendering: pixelated; }}</style></head><body>\n{}\n</body></html>\n",
        entries.join("\n")
    );
    fs::write(args.output_directory.join("index.html"), html)?;
    Ok(())
}

struct Arguments {
    map_directory: Option<PathBuf>,
    scenario_directory: Option<PathBuf>,
    output_directory: PathBuf,
    steps: Vec<u8>,
    view: Option<View>,
    catchments: bool,
}

fn parse_args() -> Result<Arguments, Box<dyn Error>> {
    let mut positional = Vec::new();
    let mut steps = None;
    let mut view = None;
    let mut scenario = None;
    let mut catchments = false;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--steps" => {
                let value = args
                    .next()
                    .ok_or("--steps requires a comma-separated list")?;
                if steps.is_some() {
                    return Err("--steps may be supplied only once".into());
                }
                steps = Some(parse_steps(&value)?);
            }
            "--view" => {
                let center = args.next().ok_or("--view requires LAT,LON WIDTHxHEIGHT")?;
                let size = args.next().ok_or("--view requires LAT,LON WIDTHxHEIGHT")?;
                if view.is_some() {
                    return Err("--view may be supplied only once".into());
                }
                view = Some(parse_view(&center, &size)?);
            }
            "--scenario" => {
                if scenario.is_some() {
                    return Err("--scenario may be supplied only once".into());
                }
                scenario = Some(PathBuf::from(
                    args.next().ok_or("--scenario requires a directory")?,
                ));
            }
            "--catchments" => {
                if catchments {
                    return Err("--catchments may be supplied only once".into());
                }
                catchments = true;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown option {value:?}; {}", usage()).into());
            }
            _ => positional.push(PathBuf::from(argument)),
        }
    }
    let (map_directory, output_directory) = if let Some(scenario_directory) = &scenario {
        if positional.len() != 1 {
            return Err(usage().into());
        }
        let output_directory = positional.remove(0);
        if catchments && scenario_directory.as_os_str().is_empty() {
            return Err("--scenario directory must not be empty".into());
        }
        (None, output_directory)
    } else {
        if catchments {
            return Err("--catchments requires --scenario".into());
        }
        if positional.len() != 2 {
            return Err(usage().into());
        }
        (Some(positional.remove(0)), positional.remove(0))
    };
    let steps = steps.ok_or("--steps is required")?;
    Ok(Arguments {
        map_directory,
        scenario_directory: scenario,
        output_directory,
        steps,
        view,
        catchments,
    })
}

fn usage() -> &'static str {
    "usage: map-preview MAP_DIR OUTPUT_DIR --steps N[,N...] [--view LAT,LON WIDTHxHEIGHT] | map-preview OUTPUT_DIR --scenario DIR --steps N[,N...] [--catchments] [--view LAT,LON WIDTHxHEIGHT]"
}

fn parse_steps(value: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut steps = Vec::new();
    for part in value.split(',') {
        let step = part.parse::<u8>()?;
        if usize::from(step) >= STEP_SIZE.len() {
            return Err(format!("step {step} is outside the supported range 0..=75").into());
        }
        if steps.contains(&step) {
            return Err(format!("step {step} was requested more than once").into());
        }
        steps.push(step);
    }
    if steps.is_empty() {
        return Err("--steps must contain at least one step".into());
    }
    Ok(steps)
}

fn parse_view(center: &str, dimensions: &str) -> Result<View, Box<dyn Error>> {
    let (latitude, longitude) = center
        .split_once(',')
        .ok_or("view centre must be LAT,LON")?;
    let latitude = latitude.parse::<f64>()?;
    let longitude = longitude.parse::<f64>()?;
    let center = project(latitude, longitude).ok_or("view centre must be finite")?;
    let (width, height) = dimensions
        .split_once('x')
        .ok_or("view size must be WIDTHxHEIGHT map pixels")?;
    let size = ViewSize {
        width: width.parse()?,
        height: height.parse()?,
    };
    if size.width == 0 || size.height == 0 {
        return Err("view width and height must be greater than zero".into());
    }
    Ok(View { center, size })
}

fn make_plans(
    map: &LoadedMap,
    steps: &[u8],
    view: Option<View>,
) -> Result<Vec<RenderPlan>, Box<dyn Error>> {
    let mut plans = Vec::with_capacity(steps.len());
    for &step in steps {
        let size = view.map_or_else(|| whole_crop_size(map, step), |view| Ok(view.size))?;
        if size.width > MAX_IMAGE_DIMENSION || size.height > MAX_IMAGE_DIMENSION {
            return Err(format!(
                "step {step} requested image {}x{} map pixels; allowed size is at most {}x{}; provide --view LAT,LON WIDTHxHEIGHT to choose a smaller view",
                size.width, size.height, MAX_IMAGE_DIMENSION, MAX_IMAGE_DIMENSION
            )
            .into());
        }
        let top_left = if let Some(view) = view {
            view_top_left(view, step)?
        } else {
            crop_top_left(map, step)?
        };
        plans.push(RenderPlan {
            step,
            top_left,
            size,
        });
    }
    Ok(plans)
}

fn whole_crop_size(map: &LoadedMap, step: u8) -> Result<ViewSize, Box<dyn Error>> {
    let crop = map.data.crop;
    let pixel_size = i64::from(STEP_SIZE[usize::from(step)]);
    let left = i64::from(crop.origin.x) * FIXED_PER_CELL;
    let top = i64::from(crop.origin.y) * FIXED_PER_CELL;
    let right = (i64::from(crop.origin.x) + i64::from(crop.width)) * FIXED_PER_CELL;
    let bottom = (i64::from(crop.origin.y) + i64::from(crop.height)) * FIXED_PER_CELL;
    let min_x = left.div_euclid(pixel_size);
    let min_y = top.div_euclid(pixel_size);
    let max_x = div_ceil(right, pixel_size);
    let max_y = div_ceil(bottom, pixel_size);
    Ok(ViewSize {
        width: u32::try_from(max_x - min_x)?,
        height: u32::try_from(max_y - min_y)?,
    })
}

fn crop_top_left(map: &LoadedMap, step: u8) -> Result<WorldPixel, Box<dyn Error>> {
    let crop = map.data.crop;
    let pixel_size = i64::from(STEP_SIZE[usize::from(step)]);
    Ok(WorldPixel {
        x: i32::try_from((i64::from(crop.origin.x) * FIXED_PER_CELL).div_euclid(pixel_size))?,
        y: i32::try_from((i64::from(crop.origin.y) * FIXED_PER_CELL).div_euclid(pixel_size))?,
    })
}

fn div_ceil(value: i64, divisor: i64) -> i64 {
    value.div_euclid(divisor) + i64::from(value.rem_euclid(divisor) != 0)
}

fn view_top_left(view: View, step: u8) -> Result<WorldPixel, Box<dyn Error>> {
    let pixel_size = i64::from(STEP_SIZE[usize::from(step)]);
    let x = i64::from(view.center.x).div_euclid(pixel_size) - i64::from(view.size.width / 2);
    let y = i64::from(view.center.y).div_euclid(pixel_size) - i64::from(view.size.height / 2);
    Ok(WorldPixel {
        x: i32::try_from(x)?,
        y: i32::try_from(y)?,
    })
}

fn render_png(map: &LoadedMap, path: &Path, plan: &RenderPlan) -> Result<(), Box<dyn Error>> {
    let pixel_count = usize::try_from(plan.size.width)?
        .checked_mul(usize::try_from(plan.size.height)?)
        .ok_or("image pixel count overflows address space")?;
    let byte_count = pixel_count
        .checked_mul(4)
        .ok_or("image byte count overflows address space")?;
    let rivers = RiverCoverage::prepare(&map.rivers, plan.step)
        .ok_or("could not prepare river pixels for step")?;
    let mut pixels = Vec::with_capacity(byte_count);
    for y in 0..plan.size.height {
        for x in 0..plan.size.width {
            let world = WorldPixel {
                x: plan.top_left.x + i32::try_from(x)?,
                y: plan.top_left.y + i32::try_from(y)?,
            };
            let colour = reference_colour(&map.data, plan.step, world, rivers.pixels())
                .ok_or("reference colour could not be computed")?;
            pixels.extend_from_slice(&colour.0);
        }
    }
    let file = File::create(path)?;
    let mut encoder = png::Encoder::new(file, plan.size.width, plan.size.height);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    Ok(())
}

fn render_scenario_png(
    scenario: &Scenario,
    markers: &mut PreparedMarkers,
    path: &Path,
    plan: &RenderPlan,
) -> Result<(), Box<dyn Error>> {
    let pixel_count = usize::try_from(plan.size.width)?
        .checked_mul(usize::try_from(plan.size.height)?)
        .ok_or("image pixel count overflows address space")?;
    let mut marker_colours = vec![None; pixel_count];
    for (position, colour) in markers
        .select_view(plan.step, plan.top_left, plan.size)
        .ok_or("marker step is outside the supported range")?
    {
        let index = usize::try_from(position[1])?
            .checked_mul(usize::try_from(plan.size.width)?)
            .and_then(|row| row.checked_add(usize::try_from(position[0]).ok()?))
            .ok_or("marker pixel index overflows address space")?;
        marker_colours[index] = Some(colour);
    }
    let rivers = RiverCoverage::prepare(&scenario.map.rivers, plan.step)
        .ok_or("could not prepare river pixels for step")?;
    let river_pixels = rivers.pixels();
    let no_rivers = BTreeSet::new();
    let mut pixels = Vec::with_capacity(
        pixel_count
            .checked_mul(4)
            .ok_or("image byte count overflows address space")?,
    );
    for y in 0..plan.size.height {
        for x in 0..plan.size.width {
            let index =
                usize::try_from(y)? * usize::try_from(plan.size.width)? + usize::try_from(x)?;
            let world = WorldPixel {
                x: plan.top_left.x + i32::try_from(x)?,
                y: plan.top_left.y + i32::try_from(y)?,
            };
            let base = reference_colour(&scenario.map.data, plan.step, world, &no_rivers)
                .ok_or("reference colour could not be computed")?;
            let colour = if river_pixels.contains(&world) {
                scenario.map.data.river
            } else {
                marker_colours[index].unwrap_or(base)
            };
            pixels.extend_from_slice(&colour.0);
        }
    }
    write_rgba_png(path, plan.size.width, plan.size.height, &pixels)
}

fn render_catchments_png(scenario: &Scenario, path: &Path) -> Result<(), Box<dyn Error>> {
    let crop = scenario.map.data.crop;
    let count = usize::try_from(crop.width)?
        .checked_mul(usize::try_from(crop.height)?)
        .ok_or("catchment image pixel count overflows address space")?;
    if scenario.catchment_owners.len() != count {
        return Err("catchment owner count does not match the crop".into());
    }
    let mut pixels = Vec::with_capacity(
        count
            .checked_mul(4)
            .ok_or("catchment image byte count overflows address space")?,
    );
    for (index, owner) in scenario.catchment_owners.iter().enumerate() {
        let colour = if let Some(owner) = owner {
            catchment_colour(*owner)
        } else {
            match scenario.map.data.cells[index] {
                Surface::Sea { .. } => crate_black(),
                Surface::Lake | Surface::Land { .. } => Rgba8::opaque(24, 19, 17),
            }
        };
        pixels.extend_from_slice(&colour.0);
    }
    write_rgba_png(path, crop.width, crop.height, &pixels)
}

fn catchment_colour(owner: usize) -> Rgba8 {
    let index = u64::try_from(owner).unwrap_or_default();
    let channel = |multiplier: u64, offset: u64| {
        u8::try_from(48 + index.wrapping_mul(multiplier).wrapping_add(offset) % 192)
            .unwrap_or_default()
    };
    Rgba8::opaque(channel(73, 31), channel(151, 79), channel(199, 113))
}

const fn crate_black() -> Rgba8 {
    Rgba8::opaque(0, 0, 0)
}

fn write_rgba_png(
    path: &Path,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<(), Box<dyn Error>> {
    let file = File::create(path)?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{div_ceil, parse_steps};

    #[test]
    fn steps_are_bounded_and_unique() {
        assert_eq!(parse_steps("0,75").expect("valid steps"), [0, 75]);
        assert!(parse_steps("76").is_err());
        assert!(parse_steps("2,2").is_err());
    }

    #[test]
    fn ceil_division_works_for_negative_and_positive_coordinates() {
        assert_eq!(div_ceil(10, 4), 3);
        assert_eq!(div_ceil(8, 4), 2);
        assert_eq!(div_ceil(-9, 4), -2);
        assert_eq!(div_ceil(-8, 4), -2);
    }
}
