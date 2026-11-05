//! Strict file loading for the backend-neutral map model.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::{Cell, Crop, MapData, Rgba8, RiverLine, Surface, TerrainStyle, project};

/// Maximum supported crop width or height, in cells.
pub const MAX_CROP_DIMENSION: u32 = 8192;
const ENCODED_ELEVATION_OFFSET: u16 = 11_000;

/// A map terrain class, indexed by the byte stored in packed grid A.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainKind {
    /// Sea cells, with depth read from the elevation grid.
    Sea,
    /// Lake cells.
    Lake,
    /// Ordinary land cells.
    Land,
    /// Mountain cells, which use cap and high-ground colours.
    Mountain,
}

/// Terrain metadata aligned with the index stored in packed grid A.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedTerrain {
    /// The six-digit RGB key used by terrain.png.
    pub key: [u8; 3],
    /// Water or land classification for this terrain index.
    pub kind: TerrainKind,
    /// Solid colour for minimal map rendering; water entries use their water colour.
    pub palette_colour: Rgba8,
    /// Index into `MapData::terrains` for land and mountain entries.
    pub style_index: Option<u16>,
}

/// An RGBA8 row-major grid matching the map crop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackedGrid {
    /// Crop width in cells.
    pub width: u32,
    /// Crop height in cells.
    pub height: u32,
    /// Row-major RGBA pixels; alpha is always 255.
    pub pixels: Vec<[u8; 4]>,
}

/// Fully decoded map inputs and the two packed RGBA8 grids used by the GPU lane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedMap {
    /// Coordinate and reference-colour inputs.
    pub data: MapData,
    /// The terrain table and palette, indexed by grid A's red byte.
    pub terrain_types: Vec<LoadedTerrain>,
    /// Grid A: terrain index, usable bit, zero, opaque alpha.
    pub grid_a: PackedGrid,
    /// Grid B: encoded elevation high byte, low byte, zero, opaque alpha.
    pub grid_b: PackedGrid,
    /// River lines projected into fixed-point world coordinates.
    pub rivers: Vec<RiverLine>,
}

/// A loading or validation error with the source file and, when applicable, entry or pixel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadError {
    message: String,
}

#[derive(Clone, Debug)]
struct ParsedTerrain {
    loaded: LoadedTerrain,
    style: Option<TerrainStyle>,
}

#[derive(Clone, Debug)]
struct KeyImage {
    pixels: Vec<[u8; 3]>,
}

impl LoadError {
    fn new(file: &str, detail: impl Display) -> Self {
        Self {
            message: format!("{file}: {detail}"),
        }
    }

    fn at(file: &str, location: impl Display, detail: impl Display) -> Self {
        Self {
            message: format!("{file} {location}: {detail}"),
        }
    }
}

impl Display for LoadError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for LoadError {}

/// Load and strictly validate one map directory.
///
/// PNG decoding uses the samples as stored; the decoder does not apply gamma or ICC profile transforms.
///
/// # Errors
/// Returns a source-qualified error for unreadable files, malformed TOML/PNG,
/// invalid table values, invalid pixels, and crop or projection mismatches.
// Keep the ordered validation pipeline together so source context stays beside each check.
#[expect(
    clippy::too_many_lines,
    reason = "keep source-qualified validation adjacent to each check"
)]
pub fn load_map(directory: impl AsRef<Path>) -> Result<LoadedMap, LoadError> {
    let directory = directory.as_ref();
    let map_file: MapFile = read_toml(directory, "map.toml")?;
    if map_file.projection != "spherical_mercator" {
        return Err(LoadError::at(
            "map.toml",
            "projection",
            format!(
                "expected spherical_mercator, found {:?}",
                map_file.projection
            ),
        ));
    }
    if map_file.cell_metres != crate::CELL_METRES {
        return Err(LoadError::at(
            "map.toml",
            "cell_metres",
            format!(
                "expected {}, found {}",
                crate::CELL_METRES,
                map_file.cell_metres
            ),
        ));
    }
    let crop = Crop {
        origin: Cell {
            x: map_file.crop.origin[0],
            y: map_file.crop.origin[1],
        },
        width: map_file.crop.width,
        height: map_file.crop.height,
    };
    let pixel_count = validate_crop(crop)?;

    let terrain_file: TerrainFile = read_toml(directory, &map_file.layers.terrain_rules)?;
    let parsed_terrains = parse_terrain_types(&terrain_file, &map_file.layers.terrain_rules)?;
    if parsed_terrains.len() > 256 {
        return Err(LoadError::at(
            &map_file.layers.terrain_rules,
            "terrain",
            format!(
                "has {} terrain types; maximum is 256",
                parsed_terrains.len()
            ),
        ));
    }
    unique_kind_index(
        &parsed_terrains,
        TerrainKind::Sea,
        "sea",
        &map_file.layers.terrain_rules,
    )?;
    unique_kind_index(
        &parsed_terrains,
        TerrainKind::Lake,
        "lake",
        &map_file.layers.terrain_rules,
    )?;
    if terrain_file.shallow_threshold == 0 {
        return Err(LoadError::at(
            &map_file.layers.terrain_rules,
            "shallow_threshold",
            "must be greater than zero",
        ));
    }
    let water = parse_water(&terrain_file.water, &map_file.layers.terrain_rules)?;
    let usable_keys = parse_usable_keys(&terrain_file.usable, &map_file.layers.terrain_rules)?;

    let terrain_image =
        decode_key_image(directory, &map_file.layers.terrain, crop.width, crop.height)?;
    let usable_image =
        decode_key_image(directory, &map_file.layers.usable, crop.width, crop.height)?;
    let elevation_image = decode_elevation_image(
        directory,
        &map_file.layers.elevation,
        crop.width,
        crop.height,
    )?;
    if terrain_image.pixels.len() != pixel_count
        || usable_image.pixels.len() != pixel_count
        || elevation_image.len() != pixel_count
    {
        return Err(LoadError::new(
            "map.toml",
            "image sample count does not match crop",
        ));
    }

    let mut key_to_index = BTreeMap::new();
    for (index, terrain) in parsed_terrains.iter().enumerate() {
        if key_to_index.insert(terrain.loaded.key, index).is_some() {
            return Err(LoadError::at(
                &map_file.layers.terrain_rules,
                format!("terrain[{index}].key"),
                format!("duplicate key colour {}", hex_colour(terrain.loaded.key)),
            ));
        }
    }

    let mut terrain_types = Vec::with_capacity(parsed_terrains.len());
    let mut styles = Vec::new();
    for mut parsed in parsed_terrains {
        if let Some(style) = parsed.style.take() {
            parsed.loaded.style_index = Some(u16::try_from(styles.len()).map_err(|_| {
                LoadError::new(&map_file.layers.terrain_rules, "too many land styles")
            })?);
            styles.push(style);
        }
        terrain_types.push(parsed.loaded);
    }

    let mut cells = Vec::with_capacity(pixel_count);
    let mut packed_a = Vec::with_capacity(pixel_count);
    let mut packed_b = Vec::with_capacity(pixel_count);
    let crop_width = usize::try_from(crop.width)
        .map_err(|_| LoadError::at("map.toml", "crop.width", "does not fit address space"))?;
    for (pixel_index, &raw_elevation) in elevation_image.iter().enumerate() {
        let x = u32::try_from(pixel_index % crop_width).unwrap_or_default();
        let y = u32::try_from(pixel_index / crop_width).unwrap_or_default();
        let Some(&terrain_index) = key_to_index.get(&terrain_image.pixels[pixel_index]) else {
            return Err(LoadError::at(
                &map_file.layers.terrain,
                format!("pixel ({x}, {y})"),
                format!(
                    "unknown terrain key {}",
                    hex_colour(terrain_image.pixels[pixel_index])
                ),
            ));
        };
        let usable = match usable_image.pixels[pixel_index] {
            colour if colour == usable_keys.0 => true,
            colour if colour == usable_keys.1 => false,
            colour => {
                return Err(LoadError::at(
                    &map_file.layers.usable,
                    format!("pixel ({x}, {y})"),
                    format!("unknown usable key {}", hex_colour(colour)),
                ));
            }
        };
        let terrain = &terrain_types[terrain_index];
        let surface = match terrain.kind {
            TerrainKind::Sea => Surface::Sea {
                depth: ENCODED_ELEVATION_OFFSET.saturating_sub(raw_elevation),
            },
            TerrainKind::Lake => Surface::Lake,
            TerrainKind::Land | TerrainKind::Mountain => Surface::Land {
                terrain: terrain.style_index.ok_or_else(|| {
                    LoadError::at(
                        &map_file.layers.terrain_rules,
                        format!("terrain[{terrain_index}]"),
                        "land terrain has no style",
                    )
                })?,
                usable,
            },
        };
        cells.push(surface);
        packed_a.push([
            u8::try_from(terrain_index).unwrap_or_default(),
            u8::from(usable),
            0,
            255,
        ]);
        packed_b.push([
            u8::try_from(raw_elevation >> 8).unwrap_or_default(),
            u8::try_from(raw_elevation & 0xff).unwrap_or_default(),
            0,
            255,
        ]);
    }

    for terrain in &mut terrain_types {
        terrain.palette_colour = match terrain.kind {
            TerrainKind::Sea => water.sea,
            TerrainKind::Lake => water.lake,
            TerrainKind::Land | TerrainKind::Mountain => {
                let index = usize::from(terrain.style_index.ok_or_else(|| {
                    LoadError::new(&map_file.layers.terrain_rules, "land terrain has no style")
                })?);
                styles[index].solid
            }
        };
    }
    let data = MapData {
        crop,
        cells,
        elevations: elevation_image,
        terrains: styles,
        sea: water.sea,
        shallow_sea: water.shallow_sea,
        lake: water.lake,
        river: water.river,
        shallow_threshold: terrain_file.shallow_threshold,
    };
    data.validate()
        .map_err(|error| LoadError::new("map.toml", error))?;
    let rivers = load_rivers(directory, &map_file.layers.rivers, crop)?;
    Ok(LoadedMap {
        data,
        terrain_types,
        grid_a: PackedGrid {
            width: crop.width,
            height: crop.height,
            pixels: packed_a,
        },
        grid_b: PackedGrid {
            width: crop.width,
            height: crop.height,
            pixels: packed_b,
        },
        rivers,
    })
}

fn validate_crop(crop: Crop) -> Result<usize, LoadError> {
    if crop.width == 0 || crop.height == 0 {
        return Err(LoadError::at(
            "map.toml",
            "crop",
            "width and height must be nonzero",
        ));
    }
    if crop.width > MAX_CROP_DIMENSION || crop.height > MAX_CROP_DIMENSION {
        return Err(LoadError::at(
            "map.toml",
            "crop",
            format!(
                "{}x{} exceeds the {}x{} cell limit",
                crop.width, crop.height, MAX_CROP_DIMENSION, MAX_CROP_DIMENSION
            ),
        ));
    }
    if crop.origin.x < 0 || crop.origin.y < 0 {
        return Err(LoadError::at(
            "map.toml",
            "crop.origin",
            "must lie in the nonnegative projected world",
        ));
    }
    let min = i64::from(i32::MIN);
    let max = i64::from(i32::MAX);
    let left = i64::from(crop.origin.x) * crate::FIXED_PER_CELL;
    let top = i64::from(crop.origin.y) * crate::FIXED_PER_CELL;
    let right = (i64::from(crop.origin.x) + i64::from(crop.width)) * crate::FIXED_PER_CELL;
    let bottom = (i64::from(crop.origin.y) + i64::from(crop.height)) * crate::FIXED_PER_CELL;
    if left < min || top < min || right > max || bottom > max {
        return Err(LoadError::at(
            "map.toml",
            "crop",
            "fixed-point bounds exceed world-coordinate range",
        ));
    }
    let width = usize::try_from(crop.width)
        .map_err(|_| LoadError::at("map.toml", "crop", "width does not fit address space"))?;
    let height = usize::try_from(crop.height)
        .map_err(|_| LoadError::at("map.toml", "crop", "height does not fit address space"))?;
    width
        .checked_mul(height)
        .ok_or_else(|| LoadError::at("map.toml", "crop", "pixel count overflows address space"))
}

fn read_toml<T: for<'de> Deserialize<'de>>(
    directory: &Path,
    relative_name: &str,
) -> Result<T, LoadError> {
    let path = input_path(directory, relative_name)?;
    let source = fs::read_to_string(&path).map_err(|error| LoadError::new(relative_name, error))?;
    toml::from_str(&source).map_err(|error| LoadError::new(relative_name, error))
}

fn input_path(directory: &Path, name: &str) -> Result<PathBuf, LoadError> {
    let path = Path::new(name);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(LoadError::new(
            "map.toml",
            format!("input path {name:?} must stay within the map directory"),
        ));
    }
    Ok(directory.join(path))
}

// Required-field and range checks stay beside their corresponding TOML fields.
#[expect(
    clippy::too_many_lines,
    reason = "required-field and range checks stay beside their TOML fields"
)]
fn parse_terrain_types(
    file: &TerrainFile,
    filename: &str,
) -> Result<Vec<ParsedTerrain>, LoadError> {
    if file.terrain.is_empty() {
        return Err(LoadError::at(
            filename,
            "terrain",
            "must contain at least one terrain type",
        ));
    }
    let mut names = std::collections::BTreeSet::new();
    file.terrain
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let location = format!("terrain[{index}]");
            if entry.name.trim().is_empty() {
                return Err(LoadError::at(
                    filename,
                    format!("{location}.name"),
                    "must not be empty",
                ));
            }
            if !names.insert(entry.name.to_lowercase()) {
                return Err(LoadError::at(
                    filename,
                    format!("{location}.name"),
                    format!("duplicate terrain name {:?}", entry.name),
                ));
            }
            let key = parse_colour(&entry.key, filename, &format!("{location}.key"))?;
            let kind = match entry.kind.as_str() {
                "sea" => TerrainKind::Sea,
                "lake" => TerrainKind::Lake,
                "land" => TerrainKind::Land,
                "mountain" => TerrainKind::Mountain,
                other => {
                    return Err(LoadError::at(
                        filename,
                        format!("{location}.kind"),
                        format!("unknown terrain kind {other:?}"),
                    ));
                }
            };
            let style = match kind {
                TerrainKind::Sea | TerrainKind::Lake => None,
                TerrainKind::Land | TerrainKind::Mountain => {
                    let solid = colour_field(entry.solid.as_deref(), filename, &location, "solid")?;
                    let second =
                        colour_field(entry.second.as_deref(), filename, &location, "second")?;
                    let dark = colour_field(entry.dark.as_deref(), filename, &location, "dark")?;
                    let dark_second = colour_field(
                        entry.dark_second.as_deref(),
                        filename,
                        &location,
                        "dark_second",
                    )?;
                    let relief_low =
                        number_field(entry.relief_low, filename, &location, "relief_low")?;
                    let relief_high =
                        number_field(entry.relief_high, filename, &location, "relief_high")?;
                    let mix = number_field(entry.mix, filename, &location, "mix")?;
                    if relief_high <= relief_low {
                        return Err(LoadError::at(
                            filename,
                            format!("{location}.relief_high"),
                            "must be greater than relief_low",
                        ));
                    }
                    if !(0..=100).contains(&mix) {
                        return Err(LoadError::at(
                            filename,
                            format!("{location}.mix"),
                            "must be in 0..=100",
                        ));
                    }
                    let (cap, high_ground, cap_low, cap_high) = if kind == TerrainKind::Mountain {
                        let cap = colour_field(entry.cap.as_deref(), filename, &location, "cap")?;
                        let high_ground = colour_field(
                            entry.high_ground.as_deref(),
                            filename,
                            &location,
                            "high_ground",
                        )?;
                        let cap_low = number_field(entry.cap_low, filename, &location, "cap_low")?;
                        let cap_high =
                            number_field(entry.cap_high, filename, &location, "cap_high")?;
                        if cap_high <= cap_low {
                            return Err(LoadError::at(
                                filename,
                                format!("{location}.cap_high"),
                                "must be greater than cap_low",
                            ));
                        }
                        (cap, high_ground, cap_low, cap_high)
                    } else {
                        (solid, dark, 0, 1)
                    };
                    Some(TerrainStyle {
                        solid,
                        second,
                        dark,
                        dark_second,
                        cap,
                        high_ground,
                        relief_low,
                        relief_high,
                        mix_percent: u8::try_from(mix).map_err(|_| {
                            LoadError::at(
                                filename,
                                format!("{location}.mix"),
                                "does not fit in a byte",
                            )
                        })?,
                        cap_low,
                        cap_high,
                        mountain: kind == TerrainKind::Mountain,
                    })
                }
            };
            let palette_colour = style.map_or(Rgba8::opaque(0, 0, 0), |s| s.solid);
            Ok(ParsedTerrain {
                loaded: LoadedTerrain {
                    key,
                    kind,
                    palette_colour,
                    style_index: None,
                },
                style,
            })
        })
        .collect()
}

fn unique_kind_index(
    terrains: &[ParsedTerrain],
    kind: TerrainKind,
    name: &str,
    filename: &str,
) -> Result<usize, LoadError> {
    let mut matches = terrains
        .iter()
        .enumerate()
        .filter(|(_, terrain)| terrain.loaded.kind == kind);
    let first = matches.next().map(|(index, _)| index).ok_or_else(|| {
        LoadError::at(
            filename,
            "terrain",
            format!("requires exactly one {name} type; found none"),
        )
    })?;
    if matches.next().is_some() {
        return Err(LoadError::at(
            filename,
            "terrain",
            format!("requires exactly one {name} type; found multiple"),
        ));
    }
    Ok(first)
}

fn parse_water(water: &WaterFile, filename: &str) -> Result<WaterColours, LoadError> {
    let sea = parse_colour(&water.sea, filename, "water.sea")?;
    Ok(WaterColours {
        sea: to_rgba(sea),
        shallow_sea: to_rgba(parse_colour(
            &water.shallow_sea,
            filename,
            "water.shallow_sea",
        )?),
        lake: to_rgba(parse_colour(&water.lake, filename, "water.lake")?),
        river: to_rgba(parse_colour(&water.river, filename, "water.river")?),
    })
}

#[derive(Clone, Copy)]
struct WaterColours {
    sea: Rgba8,
    shallow_sea: Rgba8,
    lake: Rgba8,
    river: Rgba8,
}

fn parse_usable_keys(usable: &UsableFile, filename: &str) -> Result<([u8; 3], [u8; 3]), LoadError> {
    let yes = parse_colour(&usable.usable, filename, "usable.usable")?;
    let no = parse_colour(&usable.unusable, filename, "usable.unusable")?;
    if yes == no {
        return Err(LoadError::at(
            filename,
            "usable",
            "usable and unusable key colours must differ",
        ));
    }
    Ok((yes, no))
}

fn colour_field(
    value: Option<&str>,
    filename: &str,
    entry: &str,
    field: &str,
) -> Result<Rgba8, LoadError> {
    let value = value.ok_or_else(|| {
        LoadError::at(
            filename,
            format!("{entry}.{field}"),
            "required field is missing",
        )
    })?;
    Ok(to_rgba(parse_colour(
        value,
        filename,
        &format!("{entry}.{field}"),
    )?))
}

fn number_field(
    value: Option<i32>,
    filename: &str,
    entry: &str,
    field: &str,
) -> Result<i32, LoadError> {
    value.ok_or_else(|| {
        LoadError::at(
            filename,
            format!("{entry}.{field}"),
            "required field is missing",
        )
    })
}

fn parse_colour(value: &str, file: &str, entry: &str) -> Result<[u8; 3], LoadError> {
    if value.len() != 6 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LoadError::at(
            file,
            entry,
            "colour must be exactly six hexadecimal digits",
        ));
    }
    let channel = |offset| u8::from_str_radix(&value[offset..offset + 2], 16).unwrap_or_default();
    Ok([channel(0), channel(2), channel(4)])
}

fn to_rgba(rgb: [u8; 3]) -> Rgba8 {
    Rgba8::opaque(rgb[0], rgb[1], rgb[2])
}

fn hex_colour(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

// Format branches stay together so rejected samples retain their pixel locations.
#[expect(
    clippy::too_many_lines,
    reason = "format branches report errors at the offending pixel"
)]
fn decode_key_image(
    directory: &Path,
    filename: &str,
    width: u32,
    height: u32,
) -> Result<KeyImage, LoadError> {
    let path = input_path(directory, filename)?;
    let file = File::open(&path).map_err(|error| LoadError::new(filename, error))?;
    let decoder = png::Decoder::new(BufReader::new(file));
    let mut reader = decoder
        .read_info()
        .map_err(|error| LoadError::new(filename, error))?;
    let info = reader.info();
    if info.width != width || info.height != height {
        return Err(LoadError::at(
            filename,
            "dimensions",
            format!(
                "expected {width}x{height}, found {}x{}",
                info.width, info.height
            ),
        ));
    }
    if info.color_type == png::ColorType::Indexed && info.bit_depth != png::BitDepth::Eight {
        return Err(LoadError::new(
            filename,
            format!(
                "indexed key image is {}-bit; save as 8-bit indexed or RGB/RGBA",
                bit_depth_bits(info.bit_depth)
            ),
        ));
    }
    if info.bit_depth != png::BitDepth::Eight
        || !matches!(
            info.color_type,
            png::ColorType::Rgb | png::ColorType::Rgba | png::ColorType::Indexed
        )
    {
        return Err(LoadError::new(
            filename,
            "key image must be 8-bit RGB, RGBA, or indexed",
        ));
    }
    let colour_type = info.color_type;
    let palette = info.palette.as_deref().map(ToOwned::to_owned);
    if colour_type == png::ColorType::Indexed && palette.is_none() {
        return Err(LoadError::new(filename, "indexed key image has no palette"));
    }
    let transparency = info.trns.as_deref().map(ToOwned::to_owned);
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| LoadError::new(filename, "decoded image size overflows address space"))?;
    let mut output = vec![0; size];
    let frame = reader
        .next_frame(&mut output)
        .map_err(|error| LoadError::new(filename, error))?;
    let bytes = &output[..frame.buffer_size()];
    let count = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .ok_or_else(|| LoadError::new(filename, "pixel count overflows address space"))?;
    let mut pixels = Vec::with_capacity(count);
    match colour_type {
        png::ColorType::Rgb => {
            for pixel in bytes.chunks_exact(3) {
                pixels.push([pixel[0], pixel[1], pixel[2]]);
            }
        }
        png::ColorType::Rgba => {
            for (index, pixel) in bytes.chunks_exact(4).enumerate() {
                if pixel[3] != 255 {
                    let x = u32::try_from(index % usize::try_from(width).unwrap_or(1))
                        .unwrap_or_default();
                    let y = u32::try_from(index / usize::try_from(width).unwrap_or(1))
                        .unwrap_or_default();
                    return Err(LoadError::at(
                        filename,
                        format!("pixel ({x}, {y})"),
                        "RGBA key image alpha must be 255",
                    ));
                }
                pixels.push([pixel[0], pixel[1], pixel[2]]);
            }
        }
        png::ColorType::Indexed => {
            let palette = palette.as_deref().unwrap_or_default();
            if transparency
                .as_deref()
                .is_some_and(|alpha| alpha.iter().any(|&value| value != 255))
            {
                return Err(LoadError::new(
                    filename,
                    "indexed key image palette must be fully opaque",
                ));
            }
            for (index, &palette_index) in bytes.iter().enumerate() {
                let start = usize::from(palette_index) * 3;
                if start + 2 >= palette.len() {
                    let x = u32::try_from(index % usize::try_from(width).unwrap_or(1))
                        .unwrap_or_default();
                    let y = u32::try_from(index / usize::try_from(width).unwrap_or(1))
                        .unwrap_or_default();
                    return Err(LoadError::at(
                        filename,
                        format!("pixel ({x}, {y})"),
                        "palette index is out of range",
                    ));
                }
                pixels.push([palette[start], palette[start + 1], palette[start + 2]]);
            }
        }
        _ => {
            return Err(LoadError::new(
                filename,
                "unsupported key image colour type",
            ));
        }
    }
    Ok(KeyImage { pixels })
}

fn bit_depth_bits(depth: png::BitDepth) -> u8 {
    match depth {
        png::BitDepth::One => 1,
        png::BitDepth::Two => 2,
        png::BitDepth::Four => 4,
        png::BitDepth::Eight => 8,
        png::BitDepth::Sixteen => 16,
    }
}

fn decode_elevation_image(
    directory: &Path,
    filename: &str,
    width: u32,
    height: u32,
) -> Result<Vec<u16>, LoadError> {
    let path = input_path(directory, filename)?;
    let file = File::open(&path).map_err(|error| LoadError::new(filename, error))?;
    let decoder = png::Decoder::new(BufReader::new(file));
    let mut reader = decoder
        .read_info()
        .map_err(|error| LoadError::new(filename, error))?;
    let info = reader.info();
    if info.width != width || info.height != height {
        return Err(LoadError::at(
            filename,
            "dimensions",
            format!(
                "expected {width}x{height}, found {}x{}",
                info.width, info.height
            ),
        ));
    }
    if info.color_type != png::ColorType::Grayscale || info.bit_depth != png::BitDepth::Sixteen {
        return Err(LoadError::new(
            filename,
            "elevation image must be 16-bit grayscale",
        ));
    }
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| LoadError::new(filename, "decoded image size overflows address space"))?;
    let mut output = vec![0; size];
    let frame = reader
        .next_frame(&mut output)
        .map_err(|error| LoadError::new(filename, error))?;
    let bytes = &output[..frame.buffer_size()];
    Ok(bytes
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect())
}

fn load_rivers(directory: &Path, filename: &str, crop: Crop) -> Result<Vec<RiverLine>, LoadError> {
    let file: RiversFile = read_toml(directory, filename)?;
    let mut lines = Vec::with_capacity(file.river.len());
    let mut names = std::collections::BTreeSet::new();
    for (line_index, river) in file.river.iter().enumerate() {
        if river.name.trim().is_empty() {
            return Err(LoadError::at(
                filename,
                format!("river[{line_index}].name"),
                "must not be empty",
            ));
        }
        if !names.insert(river.name.to_lowercase()) {
            return Err(LoadError::at(
                filename,
                format!("river[{line_index}].name"),
                format!("duplicate river name {:?}", river.name),
            ));
        }
        if river.points.len() < 2 {
            return Err(LoadError::at(
                filename,
                format!("river[{line_index}].points"),
                "requires at least two points",
            ));
        }
        let mut points = Vec::with_capacity(river.points.len());
        for (point_index, point) in river.points.iter().enumerate() {
            let position = project(point.latitude, point.longitude).ok_or_else(|| {
                LoadError::at(
                    filename,
                    format!("river[{line_index}].points[{point_index}]"),
                    "latitude and longitude must be finite and projectable",
                )
            })?;
            let cell = crate::cell_of(position);
            if !crop.contains(cell) {
                return Err(LoadError::at(
                    filename,
                    format!("river[{line_index}].points[{point_index}]"),
                    format!("projects outside crop at cell ({}, {})", cell.x, cell.y),
                ));
            }
            points.push(position);
        }
        lines.push(RiverLine { points });
    }
    Ok(lines)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MapFile {
    projection: String,
    cell_metres: f64,
    crop: CropFile,
    layers: LayersFile,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CropFile {
    origin: [i32; 2],
    width: u32,
    height: u32,
}
#[derive(Deserialize)]
struct LayersFile {
    terrain: String,
    usable: String,
    elevation: String,
    terrain_rules: String,
    rivers: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainFile {
    shallow_threshold: u16,
    water: WaterFile,
    usable: UsableFile,
    terrain: Vec<TerrainEntry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaterFile {
    sea: String,
    shallow_sea: String,
    lake: String,
    river: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsableFile {
    usable: String,
    unusable: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainEntry {
    name: String,
    kind: String,
    key: String,
    solid: Option<String>,
    second: Option<String>,
    dark: Option<String>,
    dark_second: Option<String>,
    relief_low: Option<i32>,
    relief_high: Option<i32>,
    mix: Option<i32>,
    cap: Option<String>,
    high_ground: Option<String>,
    cap_low: Option<i32>,
    cap_high: Option<i32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RiversFile {
    river: Vec<RiverEntry>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RiverEntry {
    name: String,
    points: Vec<LatLon>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LatLon {
    latitude: f64,
    longitude: f64,
}
