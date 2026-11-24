//! Validated world backdrop normals, independent of game state.
use std::path::Path;

use serde::Deserialize;

use crate::loading::{LoadError, decode_gray8_image, decode_gray16_image, read_toml};

const WORLD_CELLS: u32 = 47_766;
const MONTHS: u32 = 12;

#[derive(Deserialize)]
struct Manifest {
    format: u32,
    projection: String,
    cell_metres: f64,
    block_cells: u32,
    first_row: u32,
    rows: u32,
    columns: u32,
    layers: Layers,
}
#[derive(Deserialize)]
struct Layers {
    land: String,
    pressure: String,
    air_temperature: String,
    sea_temperature: String,
}

/// A loaded backdrop and its monthly values.
pub struct Backdrop {
    manifest: Manifest,
    land: Vec<u8>,
    pressure: Vec<u16>,
    air: Vec<u16>,
    sea: Vec<u16>,
}

/// Values for one block and Gregorian month.
#[derive(Debug, PartialEq, Eq)]
pub struct BackdropSample {
    /// True for land.
    pub land: bool,
    /// Sea-level pressure in pascals.
    pub pressure_pa: u32,
    /// Air temperature in tenths of a degree Celsius.
    pub air_temperature_tenths: i32,
    /// Sea temperature in tenths of a degree Celsius, absent on land.
    pub sea_temperature_tenths: Option<i32>,
}

/// Load and validate a backdrop directory.
///
/// # Errors
/// Returns the source file and reason when a manifest or image is invalid.
pub fn load_backdrop(directory: &Path) -> Result<Backdrop, LoadError> {
    let m: Manifest = read_toml(directory, "backdrop.toml")?;
    if m.format != 1 || m.projection != "spherical_mercator" || m.cell_metres != crate::CELL_METRES
    {
        return Err(LoadError::new(
            "backdrop.toml",
            "unexpected format, projection or cell size",
        ));
    }
    if !(16..=8192).contains(&m.block_cells) || !m.block_cells.is_multiple_of(16) {
        return Err(LoadError::new("backdrop.toml", "invalid block_cells"));
    }
    let world_blocks = WORLD_CELLS.div_ceil(m.block_cells);
    if m.columns != world_blocks
        || m.rows == 0
        || m.first_row
            .checked_add(m.rows)
            .is_none_or(|end| end > world_blocks)
    {
        return Err(LoadError::new("backdrop.toml", "invalid columns or rows"));
    }
    let height = m.rows * MONTHS;
    let land = decode_gray8_image(directory, &m.layers.land, m.columns, m.rows)?;
    let pressure = decode_gray16_image(directory, &m.layers.pressure, m.columns, height)?;
    let air = decode_gray16_image(directory, &m.layers.air_temperature, m.columns, height)?;
    let sea = decode_gray16_image(directory, &m.layers.sea_temperature, m.columns, height)?;
    let blocks = land.len();
    for (index, &sample) in land.iter().enumerate() {
        if sample != 0 && sample != 255 {
            return Err(LoadError::at(
                &m.layers.land,
                format!("cell {index}"),
                "expected 0 or 255",
            ));
        }
    }
    for index in 0..pressure.len() {
        let block = index % blocks;
        if !(30_000..=47_000).contains(&pressure[index]) {
            return Err(LoadError::at(
                &m.layers.pressure,
                format!("cell {block} month {}", index / blocks),
                "pressure outside range",
            ));
        }
        if !(300..=1500).contains(&air[index]) {
            return Err(LoadError::at(
                &m.layers.air_temperature,
                format!("cell {block} month {}", index / blocks),
                "air outside range",
            ));
        }
        if (land[block] == 255 && sea[index] != 0)
            || (land[block] == 0 && !(970..=1400).contains(&sea[index]))
        {
            return Err(LoadError::at(
                &m.layers.sea_temperature,
                format!("cell {block} month {}", index / blocks),
                "sea temperature inconsistent with land or outside range",
            ));
        }
    }
    Ok(Backdrop {
        manifest: m,
        land,
        pressure,
        air,
        sea,
    })
}

impl Backdrop {
    /// Block side in world cells.
    #[must_use]
    pub fn block_cells(&self) -> u32 {
        self.manifest.block_cells
    }
    /// Number of columns.
    #[must_use]
    pub fn columns(&self) -> u32 {
        self.manifest.columns
    }
    /// First world block row.
    #[must_use]
    pub fn first_row(&self) -> u32 {
        self.manifest.first_row
    }
    /// Number of rows.
    #[must_use]
    pub fn rows(&self) -> u32 {
        self.manifest.rows
    }
    /// Block containing a world cell, if covered.
    #[must_use]
    pub fn block_of(&self, i: u32, j: u32) -> Option<(u32, u32)> {
        if i >= WORLD_CELLS || j >= WORLD_CELLS {
            return None;
        }
        let (column, row) = (i / self.block_cells(), j / self.block_cells());
        (row >= self.first_row() && row < self.first_row() + self.rows()).then_some((column, row))
    }
    /// East neighbour with world wrap.
    #[must_use]
    pub fn east(&self, column: u32) -> Option<u32> {
        (column < self.columns()).then(|| (column + 1) % self.columns())
    }
    /// Month is zero-based, January through December.
    #[must_use]
    pub fn sample(&self, column: u32, row: u32, month: u32) -> Option<BackdropSample> {
        if column >= self.columns()
            || row < self.first_row()
            || row >= self.first_row() + self.rows()
            || month >= MONTHS
        {
            return None;
        }
        let block = ((row - self.first_row()) * self.columns() + column) as usize;
        let index = month as usize * self.land.len() + block;
        let is_land = self.land[block] == 255;
        Some(BackdropSample {
            land: is_land,
            pressure_pa: u32::from(self.pressure[index]) + 60_000,
            air_temperature_tenths: i32::from(self.air[index]) - 1000,
            sea_temperature_tenths: (!is_land).then(|| i32::from(self.sea[index]) - 1000),
        })
    }
}
