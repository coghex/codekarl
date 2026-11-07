//! Scenario, location, catchment and marker loading for the map preview.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::{
    Cell, Crop, LoadError, LoadedMap, Rgba8, STEP_SIZE, Surface, ViewSize, WorldPixel, cell_of,
    project,
};

const MAX_CATCHMENT_METRES: i64 = 50_000;
const METRES_PER_CELL: i128 = 839;
const SCALE: i128 = 65_536;

/// Validated marker styles shared by every scenario on a map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocationKinds {
    /// Location marker kinds, in source-file order.
    pub kinds: Vec<LocationKind>,
    /// Validated notable-building styles, reserved for later slices.
    pub buildings: Vec<BuildingKind>,
}

/// One location kind's allowed sizes, catchment radii and marker appearance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocationKind {
    /// Stable lower-case identifier.
    pub id: String,
    /// Inclusive minimum size.
    pub min_size: u8,
    /// Inclusive maximum size.
    pub max_size: u8,
    /// Default radius for each size, in order from `min_size`.
    pub catchment_m: Vec<u32>,
    /// Marker rows from top to bottom.
    pub glyph: Vec<Vec<char>>,
    /// Colour for `#` pixels.
    pub light: Rgba8,
    /// Colour for `d` pixels and outlines.
    pub dark: Rgba8,
}

/// One notable-building glyph and its colours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildingKind {
    /// Stable lower-case identifier.
    pub id: String,
    /// Marker rows from top to bottom.
    pub glyph: Vec<Vec<char>>,
    /// Colour for `#` pixels.
    pub light: Rgba8,
    /// Colour for `d` pixels.
    pub dark: Rgba8,
}

/// Scenario start date; Julian validity is deliberately left to the simulation layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScenarioStart {
    /// Calendar year, 1 through 9999.
    pub year: u16,
    /// Calendar month, 1 through 12.
    pub month: u8,
    /// Calendar day, 1 through 31.
    pub day: u8,
    /// Hour, 0 through 23.
    pub hour: u8,
}

/// A validated scenario location and its projected crop cell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// Stable scenario-local identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Projected map cell containing the authored point.
    pub cell: Cell,
    /// Index into [`LocationKinds::kinds`].
    pub kind_index: usize,
    /// Authored size.
    pub size: u8,
    /// Indices into [`LocationKinds::buildings`].
    pub building_indices: Vec<usize>,
    /// Explicit catchment radius, if supplied.
    pub catchment_m: Option<u32>,
    /// Source note.
    pub source: String,
    /// Confidence label.
    pub confidence: Confidence,
}

/// Confidence in one authored location.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence {
    /// Strong evidence.
    High,
    /// Moderate evidence.
    Medium,
    /// Tentative placement.
    Low,
}

/// Loaded scenario, map, locations and row-major catchment owner indices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scenario {
    /// Stable scenario id.
    pub id: String,
    /// Start date.
    pub start: ScenarioStart,
    /// Loaded map.
    pub map: LoadedMap,
    /// Validated marker styles shared by this map's scenarios.
    pub location_kinds: LocationKinds,
    /// Locations in authored file order.
    pub locations: Vec<Location>,
    /// Per-crop-cell owning location index, or `None`.
    pub catchment_owners: Vec<Option<usize>>,
}

/// One visible marker pixel and its opaque display colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkerPixel {
    /// Global map-pixel position.
    pub pixel: WorldPixel,
    /// Marker colour.
    pub colour: Rgba8,
}

/// Lazily prepared marker pixels, parallel to the river coverage cache.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreparedMarkers {
    locations: Vec<Location>,
    kinds: LocationKinds,
    steps: Vec<Option<Vec<MarkerPixel>>>,
}

impl PreparedMarkers {
    /// Build a per-scenario cache; each zoom step is rasterized at most once.
    #[must_use]
    pub fn new(locations: Vec<Location>, kinds: LocationKinds) -> Self {
        Self {
            locations,
            kinds,
            steps: vec![None; STEP_SIZE.len()],
        }
    }

    /// Return prepared global pixels for a step, or `None` for an invalid step.
    pub fn pixels(&mut self, step: u8) -> Option<&[MarkerPixel]> {
        let slot = self.steps.get_mut(usize::from(step))?;
        if slot.is_none() {
            *slot = Some(prepare_markers(&self.locations, &self.kinds, step));
        }
        slot.as_deref()
    }

    /// Select visible marker pixels as view-relative coordinates.
    pub fn select_view(
        &mut self,
        step: u8,
        top_left: WorldPixel,
        view: ViewSize,
    ) -> Option<Vec<([u32; 2], Rgba8)>> {
        let right = i64::from(top_left.x) + i64::from(view.width);
        let bottom = i64::from(top_left.y) + i64::from(view.height);
        Some(
            self.pixels(step)?
                .iter()
                .filter_map(|marker| {
                    let x = i64::from(marker.pixel.x);
                    let y = i64::from(marker.pixel.y);
                    if x < i64::from(top_left.x)
                        || x >= right
                        || y < i64::from(top_left.y)
                        || y >= bottom
                    {
                        return None;
                    }
                    Some((
                        [
                            u32::try_from(x - i64::from(top_left.x)).ok()?,
                            u32::try_from(y - i64::from(top_left.y)).ok()?,
                        ],
                        marker.colour,
                    ))
                })
                .collect(),
        )
    }
}

/// Load and validate a scenario directory, its map, location table and catchments.
///
/// # Errors
/// Returns a source-qualified `LoadError` for malformed files or placements.
pub fn load_scenario(directory: impl AsRef<Path>) -> Result<Scenario, LoadError> {
    let directory = directory.as_ref();
    let scenario_file: ScenarioFile = read_scenario_toml(directory, "scenario.toml")?;
    validate_id(&scenario_file.id, "scenario.toml", "scenario.id")?;
    validate_map_name(&scenario_file.map)?;
    validate_relative_path(
        &scenario_file.locations,
        "scenario.toml",
        "scenario.locations",
    )?;
    let start = validate_start(&scenario_file.start)?;
    let map_directory = scenario_map_directory(directory, &scenario_file.map)?;
    let map = crate::load_map(&map_directory)?;
    let locations_file: LocationsFile = read_scenario_toml(directory, &scenario_file.locations)?;
    let kinds = load_location_kinds(&map_directory)?;
    if kinds.is_none() && !locations_file.location.is_empty() {
        return Err(LoadError::at(
            "location_kinds.toml",
            "kind",
            "required when a scenario has locations",
        ));
    }
    let location_kinds = kinds.unwrap_or_default();
    let locations = validate_locations(
        &locations_file,
        &scenario_file.locations,
        &map,
        &location_kinds,
    )?;
    let catchment_owners = compute_catchments(&map.data, &locations, &location_kinds)?;
    Ok(Scenario {
        id: scenario_file.id,
        start,
        map,
        location_kinds,
        locations,
        catchment_owners,
    })
}

fn validate_map_name(value: &str) -> Result<(), LoadError> {
    if value.is_empty()
        || value.len() > 48
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(LoadError::at(
            "scenario.toml",
            "scenario.map",
            "must be a map name ([a-z0-9_]{1,48})",
        ));
    }
    Ok(())
}

fn scenario_map_directory(directory: &Path, map_name: &str) -> Result<PathBuf, LoadError> {
    let absolute = std::path::absolute(directory).map_err(|error| {
        LoadError::at(
            "scenario.toml",
            "scenario.map",
            format!("could not make scenario directory absolute: {error}"),
        )
    })?;
    let Some(scenarios_directory) = absolute.parent() else {
        return Err(scenario_layout_error());
    };
    let Some(assets_root) = scenarios_directory.parent() else {
        return Err(scenario_layout_error());
    };
    if scenarios_directory
        .file_name()
        .and_then(|name| name.to_str())
        != Some("scenarios")
    {
        return Err(scenario_layout_error());
    }

    let map_directory = assets_root.join("map").join(map_name);
    if !map_directory.is_dir() {
        return Err(LoadError::at(
            "scenario.toml",
            "scenario.map",
            format!(
                "map {map_name:?} was not found at {}",
                map_directory.display()
            ),
        ));
    }
    Ok(map_directory)
}

fn scenario_layout_error() -> LoadError {
    LoadError::at(
        "scenario.toml",
        "scenario.map",
        "scenario directory must be <root>/scenarios/<directory> so maps can be resolved under <root>/map/<name>",
    )
}

pub(crate) fn load_location_kinds(
    map_directory: &Path,
) -> Result<Option<LocationKinds>, LoadError> {
    let path = map_directory.join("location_kinds.toml");
    if !path.exists() {
        return Ok(None);
    }
    let source =
        fs::read_to_string(&path).map_err(|error| LoadError::new("location_kinds.toml", error))?;
    let file: LocationKindsFile =
        toml::from_str(&source).map_err(|error| LoadError::new("location_kinds.toml", error))?;
    validate_location_kinds(&file).map(Some)
}

fn validate_location_kinds(file: &LocationKindsFile) -> Result<LocationKinds, LoadError> {
    let mut kind_ids = BTreeSet::new();
    let mut kinds = Vec::with_capacity(file.kind.len());
    for (index, entry) in file.kind.iter().enumerate() {
        let path = format!("kind[{}]", entry.id);
        validate_id(&entry.id, "location_kinds.toml", &path)?;
        if !kind_ids.insert(entry.id.as_str()) {
            return Err(LoadError::at(
                "location_kinds.toml",
                &path,
                "duplicate kind id",
            ));
        }
        if entry.sizes.len() != 2 {
            return Err(LoadError::at(
                "location_kinds.toml",
                format!("{path}.sizes"),
                "must contain exactly [min, max]",
            ));
        }
        let min = entry.sizes[0];
        let max = entry.sizes[1];
        if min < 1 || max > 16 || min > max {
            return Err(LoadError::at(
                "location_kinds.toml",
                format!("{path}.sizes"),
                "must satisfy 1 <= min <= max <= 16",
            ));
        }
        let expected = usize::try_from(max - min + 1).unwrap_or_default();
        if entry.catchment_m.len() != expected {
            return Err(LoadError::at(
                "location_kinds.toml",
                format!("{path}.catchment_m"),
                format!("must contain exactly {expected} radii"),
            ));
        }
        let radii = validate_radii(&entry.catchment_m, "location_kinds.toml", &path)?;
        let glyph = parse_glyph(&entry.glyph, "location_kinds.toml", &path)?;
        kinds.push(LocationKind {
            id: entry.id.clone(),
            min_size: u8::try_from(min).unwrap_or_default(),
            max_size: u8::try_from(max).unwrap_or_default(),
            catchment_m: radii,
            glyph,
            light: parse_rgb(&entry.light, "location_kinds.toml", &path, "light")?,
            dark: parse_rgb(&entry.dark, "location_kinds.toml", &path, "dark")?,
        });
        let _ = index;
    }
    let mut building_ids = BTreeSet::new();
    let mut buildings = Vec::with_capacity(file.building.len());
    for entry in &file.building {
        let path = format!("building[{}]", entry.id);
        validate_id(&entry.id, "location_kinds.toml", &path)?;
        if !building_ids.insert(entry.id.as_str()) {
            return Err(LoadError::at(
                "location_kinds.toml",
                &path,
                "duplicate building id",
            ));
        }
        buildings.push(BuildingKind {
            id: entry.id.clone(),
            glyph: parse_glyph(&entry.glyph, "location_kinds.toml", &path)?,
            light: parse_rgb(&entry.light, "location_kinds.toml", &path, "light")?,
            dark: parse_rgb(&entry.dark, "location_kinds.toml", &path, "dark")?,
        });
    }
    Ok(LocationKinds { kinds, buildings })
}

fn validate_radii(values: &[i64], file: &str, entry: &str) -> Result<Vec<u32>, LoadError> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            if !(1..=MAX_CATCHMENT_METRES).contains(value) {
                return Err(LoadError::at(
                    file,
                    format!("{entry}.catchment_m[{index}]"),
                    "must be in 1..=50000 metres",
                ));
            }
            Ok(u32::try_from(*value).unwrap_or_default())
        })
        .collect()
}

fn validate_start(start: &StartFile) -> Result<ScenarioStart, LoadError> {
    if !(1..=9999).contains(&start.year) {
        return Err(LoadError::at(
            "scenario.toml",
            "start.year",
            "must be in 1..=9999",
        ));
    }
    if !(1..=12).contains(&start.month) {
        return Err(LoadError::at(
            "scenario.toml",
            "start.month",
            "must be in 1..=12",
        ));
    }
    if !(1..=31).contains(&start.day) {
        return Err(LoadError::at(
            "scenario.toml",
            "start.day",
            "must be in 1..=31",
        ));
    }
    if !(0..=23).contains(&start.hour) {
        return Err(LoadError::at(
            "scenario.toml",
            "start.hour",
            "must be in 0..=23",
        ));
    }
    Ok(ScenarioStart {
        year: u16::try_from(start.year).unwrap_or_default(),
        month: u8::try_from(start.month).unwrap_or_default(),
        day: u8::try_from(start.day).unwrap_or_default(),
        hour: u8::try_from(start.hour).unwrap_or_default(),
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "keep location validation stages together before catchment computation"
)]
fn validate_locations(
    file: &LocationsFile,
    filename: &str,
    map: &LoadedMap,
    kinds: &LocationKinds,
) -> Result<Vec<Location>, LoadError> {
    let mut ids = BTreeSet::new();
    // Validate every authored row before resolving cross-file identifiers or
    // computing any catchment index.
    for entry in &file.location {
        let path = format!("location[{}]", entry.id);
        validate_id(&entry.id, filename, &path)?;
        if !ids.insert(entry.id.as_str()) {
            return Err(LoadError::at(filename, &path, "duplicate location id"));
        }
        if entry.name.trim().is_empty() || entry.name.chars().count() > 64 {
            return Err(LoadError::at(
                filename,
                format!("{path}.name"),
                "must be non-empty and at most 64 characters",
            ));
        }
        if entry.source.trim().is_empty() {
            return Err(LoadError::at(
                filename,
                format!("{path}.source"),
                "must not be empty",
            ));
        }
        if !matches!(entry.confidence.as_str(), "high" | "medium" | "low") {
            return Err(LoadError::at(
                filename,
                format!("{path}.confidence"),
                "must be high, medium or low",
            ));
        }
        if project(entry.lat, entry.lon).is_none() {
            return Err(LoadError::at(
                filename,
                &path,
                "latitude and longitude must be finite and projectable",
            ));
        }
        if let Some(radius) = entry.catchment_m
            && !(1..=MAX_CATCHMENT_METRES).contains(&radius)
        {
            return Err(LoadError::at(
                filename,
                format!("{path}.catchment_m"),
                "must be in 1..=50000 metres",
            ));
        }
        let mut seen_buildings = BTreeSet::new();
        for building in &entry.buildings {
            if !seen_buildings.insert(building.as_str()) {
                return Err(LoadError::at(
                    filename,
                    format!("{path}.buildings"),
                    format!("repeated building {building:?}"),
                ));
            }
        }
    }
    let mut locations = Vec::with_capacity(file.location.len());
    for entry in &file.location {
        let path = format!("location[{}]", entry.id);
        let confidence = match entry.confidence.as_str() {
            "high" => Confidence::High,
            "medium" => Confidence::Medium,
            "low" => Confidence::Low,
            _ => unreachable!("confidence validated above"),
        };
        let position = project(entry.lat, entry.lon).expect("projectability validated above");
        let cell = cell_of(position);
        let kind_index = kinds
            .kinds
            .iter()
            .position(|kind| kind.id == entry.kind)
            .ok_or_else(|| {
                LoadError::at(
                    filename,
                    format!("{path}.kind"),
                    format!("unknown kind {:?}", entry.kind),
                )
            })?;
        let kind = &kinds.kinds[kind_index];
        if entry.size < i64::from(kind.min_size) || entry.size > i64::from(kind.max_size) {
            return Err(LoadError::at(
                filename,
                format!("{path}.size"),
                format!(
                    "must be in {}..={} for kind {:?}",
                    kind.min_size, kind.max_size, kind.id
                ),
            ));
        }
        let catchment_m = entry
            .catchment_m
            .map(|radius| u32::try_from(radius).expect("radius range validated above"));
        let mut building_indices = Vec::with_capacity(entry.buildings.len());
        for building in &entry.buildings {
            let index = kinds
                .buildings
                .iter()
                .position(|candidate| candidate.id == *building)
                .ok_or_else(|| {
                    LoadError::at(
                        filename,
                        format!("{path}.buildings"),
                        format!("unknown building {building:?}"),
                    )
                })?;
            building_indices.push(index);
        }
        locations.push(Location {
            id: entry.id.clone(),
            name: entry.name.clone(),
            cell,
            kind_index,
            size: u8::try_from(entry.size).unwrap_or_default(),
            building_indices,
            catchment_m,
            source: entry.source.clone(),
            confidence,
        });
    }
    let mut occupied = BTreeMap::new();
    for (index, location) in locations.iter().enumerate() {
        if !map.data.crop.contains(location.cell) {
            return Err(LoadError::at(
                filename,
                format!("location[{}]", location.id),
                format!(
                    "projects outside crop at cell ({}, {})",
                    location.cell.x, location.cell.y
                ),
            ));
        }
        let cell_index =
            local_index(map.data.crop, location.cell).expect("crop membership checked");
        match map.data.cells[cell_index] {
            Surface::Land { .. } => {}
            Surface::Sea { .. } => {
                return Err(LoadError::at(
                    filename,
                    format!("location[{}]", location.id),
                    "projects onto sea",
                ));
            }
            Surface::Lake => {
                return Err(LoadError::at(
                    filename,
                    format!("location[{}]", location.id),
                    "projects onto lake",
                ));
            }
        }
        if let Some(previous) = occupied.insert(location.cell, index) {
            return Err(LoadError::at(
                filename,
                format!("location[{}]", location.id),
                format!("shares cell with location[{}]", locations[previous].id),
            ));
        }
    }
    Ok(locations)
}

fn compute_catchments(
    map: &crate::MapData,
    locations: &[Location],
    kinds: &LocationKinds,
) -> Result<Vec<Option<usize>>, LoadError> {
    let count = usize::try_from(map.crop.width)
        .ok()
        .and_then(|width| {
            usize::try_from(map.crop.height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| {
            LoadError::at(
                "locations.toml",
                "catchment",
                "crop size overflows address space",
            )
        })?;
    let mut owners = vec![None; count];
    let mut best_q: Vec<Option<i128>> = vec![None; count];
    let row_scales = ground_scales(map.crop);
    for (location_index, location) in locations.iter().enumerate() {
        let kind = &kinds.kinds[location.kind_index];
        let radius = i128::from(
            location
                .catchment_m
                .unwrap_or_else(|| kind.catchment_m[usize::from(location.size - kind.min_size)]),
        );
        let local_y = i64::from(location.cell.y) - i64::from(map.crop.origin.y);
        let s = i128::from(
            row_scales[usize::try_from(local_y).map_err(|_| {
                LoadError::at("locations.toml", &location.id, "row is outside crop")
            })?],
        );
        let extent = ceil_nonnegative_i64(
            i64::try_from(radius * SCALE).unwrap_or_default(),
            i64::try_from(METRES_PER_CELL * s).unwrap_or_default(),
        );
        let center_x = i64::from(location.cell.x) - i64::from(map.crop.origin.x);
        let center_y = local_y;
        let x0 = (center_x - extent).max(0);
        let x1 = (center_x + extent).min(i64::from(map.crop.width) - 1);
        let y0 = (center_y - extent).max(0);
        let y1 = (center_y + extent).min(i64::from(map.crop.height) - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let di = i128::from(x - center_x);
                let dj = i128::from(y - center_y);
                let dx = METRES_PER_CELL * di;
                let dy = METRES_PER_CELL * dj;
                let q = (dx * dx + dy * dy) * s * s;
                if q > radius * radius * (1_i128 << 32) {
                    continue;
                }
                let index = usize::try_from(y).unwrap_or_default()
                    * usize::try_from(map.crop.width).unwrap_or_default()
                    + usize::try_from(x).unwrap_or_default();
                if best_q[index].is_none_or(|best| q < best) {
                    best_q[index] = Some(q);
                    owners[index] = Some(location_index);
                }
            }
        }
    }
    Ok(owners)
}

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the spec requires one f64 latitude cosine per bounded map row, rounded and clamped to 1..=65536"
)]
fn ground_scales(crop: Crop) -> Vec<u32> {
    (0..crop.height)
        .map(|row| {
            let global_y = i64::from(crop.origin.y) + i64::from(row);
            let y_metres = (global_y as f64 + 0.5) * 839.0;
            let half_world = std::f64::consts::PI * 6_378_137.0;
            let latitude = 2.0 * ((half_world - y_metres) / 6_378_137.0).exp().atan()
                - std::f64::consts::FRAC_PI_2;
            (latitude.cos() * 65_536.0).round().clamp(1.0, 65_536.0) as u32
        })
        .collect()
}

fn ceil_nonnegative_i64(numerator: i64, denominator: i64) -> i64 {
    debug_assert!(numerator >= 0 && denominator > 0);
    (numerator + denominator - 1) / denominator
}

fn prepare_markers(locations: &[Location], kinds: &LocationKinds, step: u8) -> Vec<MarkerPixel> {
    if step < 15 {
        return Vec::new();
    }
    let Some(pixel_size) = STEP_SIZE
        .get(usize::from(step))
        .map(|value| i64::from(*value))
    else {
        return Vec::new();
    };
    let mut pixels = BTreeMap::new();
    for location in locations {
        let Some(kind) = kinds.kinds.get(location.kind_index) else {
            continue;
        };
        let center = WorldPixel {
            x: clamp_i32(
                (i64::from(location.cell.x) * crate::FIXED_PER_CELL + crate::FIXED_PER_CELL / 2)
                    .div_euclid(pixel_size),
            ),
            y: clamp_i32(
                (i64::from(location.cell.y) * crate::FIXED_PER_CELL + crate::FIXED_PER_CELL / 2)
                    .div_euclid(pixel_size),
            ),
        };
        let height = i32::try_from(kind.glyph.len()).unwrap_or_default();
        let width = i32::try_from(kind.glyph.first().map_or(0, Vec::len)).unwrap_or_default();
        let left = center.x - (width >> 1);
        let top = center.y - (height >> 1);
        let mut mask = BTreeMap::new();
        for (row, cells) in kind.glyph.iter().enumerate() {
            for (column, glyph) in cells.iter().enumerate() {
                let pixel = WorldPixel {
                    x: left + i32::try_from(column).unwrap_or_default(),
                    y: top + i32::try_from(row).unwrap_or_default(),
                };
                let colour = match glyph {
                    '#' => Some(kind.light),
                    'd' => Some(kind.dark),
                    _ => None,
                };
                if let Some(colour) = colour {
                    mask.insert(pixel, colour);
                }
            }
        }
        let light_pixels: Vec<_> = mask
            .iter()
            .filter_map(|(pixel, _)| {
                kind.glyph_pixel_at(*pixel, left, top)
                    .filter(|glyph| *glyph == '#')
                    .map(|_| *pixel)
            })
            .collect();
        for pixel in light_pixels {
            for neighbour in [
                WorldPixel {
                    x: pixel.x - 1,
                    y: pixel.y,
                },
                WorldPixel {
                    x: pixel.x + 1,
                    y: pixel.y,
                },
                WorldPixel {
                    x: pixel.x,
                    y: pixel.y - 1,
                },
                WorldPixel {
                    x: pixel.x,
                    y: pixel.y + 1,
                },
            ] {
                mask.entry(neighbour).or_insert(kind.dark);
            }
        }
        pixels.extend(mask);
    }
    pixels
        .into_iter()
        .map(|(pixel, colour)| MarkerPixel { pixel, colour })
        .collect()
}

trait GlyphAt {
    fn glyph_pixel_at(&self, pixel: WorldPixel, left: i32, top: i32) -> Option<char>;
}
impl GlyphAt for LocationKind {
    fn glyph_pixel_at(&self, pixel: WorldPixel, left: i32, top: i32) -> Option<char> {
        let x = usize::try_from(pixel.x - left).ok()?;
        let y = usize::try_from(pixel.y - top).ok()?;
        self.glyph.get(y)?.get(x).copied()
    }
}

fn local_index(crop: Crop, cell: Cell) -> Option<usize> {
    if !crop.contains(cell) {
        return None;
    }
    let x = usize::try_from(i64::from(cell.x) - i64::from(crop.origin.x)).ok()?;
    let y = usize::try_from(i64::from(cell.y) - i64::from(crop.origin.y)).ok()?;
    y.checked_mul(usize::try_from(crop.width).ok()?)?
        .checked_add(x)
}

fn validate_id(value: &str, filename: &str, entry: &str) -> Result<(), LoadError> {
    if value.is_empty()
        || value.len() > 48
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(LoadError::at(
            filename,
            entry,
            "id must match [a-z0-9_]{1,48}",
        ));
    }
    Ok(())
}

fn validate_relative_path(value: &str, filename: &str, entry: &str) -> Result<(), LoadError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(LoadError::at(
            filename,
            entry,
            "must be a relative path inside the scenario directory",
        ));
    }
    Ok(())
}

fn parse_glyph(rows: &[String], filename: &str, entry: &str) -> Result<Vec<Vec<char>>, LoadError> {
    if rows.is_empty() || rows.len() > 9 {
        return Err(LoadError::at(
            filename,
            format!("{entry}.glyph"),
            "must have 1..=9 rows",
        ));
    }
    let width = rows[0].chars().count();
    if width == 0 || width > 9 {
        return Err(LoadError::at(
            filename,
            format!("{entry}.glyph"),
            "rows must have 1..=9 characters",
        ));
    }
    let mut has_light = false;
    let mut glyph = Vec::with_capacity(rows.len());
    for (row_index, row) in rows.iter().enumerate() {
        let chars: Vec<_> = row.chars().collect();
        if chars.len() != width {
            return Err(LoadError::at(
                filename,
                format!("{entry}.glyph[{row_index}]"),
                "glyph rows must be rectangular",
            ));
        }
        if chars.iter().any(|glyph| !matches!(glyph, '#' | 'd' | '.')) {
            return Err(LoadError::at(
                filename,
                format!("{entry}.glyph[{row_index}]"),
                "glyph characters must be #, d or .",
            ));
        }
        has_light |= chars.contains(&'#');
        glyph.push(chars);
    }
    if !has_light {
        return Err(LoadError::at(
            filename,
            format!("{entry}.glyph"),
            "must contain at least one #",
        ));
    }
    Ok(glyph)
}

fn parse_rgb(value: &str, filename: &str, entry: &str, field: &str) -> Result<Rgba8, LoadError> {
    let Some(hex) = value.strip_prefix('#') else {
        return Err(LoadError::at(
            filename,
            format!("{entry}.{field}"),
            "colour must be #rrggbb",
        ));
    };
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LoadError::at(
            filename,
            format!("{entry}.{field}"),
            "colour must be #rrggbb",
        ));
    }
    let channel = |offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap_or_default();
    Ok(Rgba8::opaque(channel(0), channel(2), channel(4)))
}

fn read_scenario_toml<T: for<'de> Deserialize<'de>>(
    directory: &Path,
    filename: &str,
) -> Result<T, LoadError> {
    let path = directory.join(filename);
    let source = fs::read_to_string(path).map_err(|error| LoadError::new(filename, error))?;
    toml::from_str(&source).map_err(|error| LoadError::new(filename, error))
}

fn clamp_i32(value: i64) -> i32 {
    i32::try_from(value.clamp(i64::from(i32::MIN), i64::from(i32::MAX))).unwrap_or_default()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenarioFile {
    id: String,
    map: String,
    start: StartFile,
    locations: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartFile {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocationsFile {
    #[serde(default)]
    location: Vec<LocationFile>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocationFile {
    id: String,
    name: String,
    lat: f64,
    lon: f64,
    kind: String,
    size: i64,
    #[serde(default)]
    buildings: Vec<String>,
    catchment_m: Option<i64>,
    source: String,
    confidence: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocationKindsFile {
    #[serde(default)]
    kind: Vec<LocationKindFile>,
    #[serde(default)]
    building: Vec<BuildingKindFile>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocationKindFile {
    id: String,
    sizes: Vec<i64>,
    catchment_m: Vec<i64>,
    glyph: Vec<String>,
    light: String,
    dark: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildingKindFile {
    id: String,
    glyph: Vec<String>,
    light: String,
    dark: String,
}

#[cfg(test)]
mod tests;
