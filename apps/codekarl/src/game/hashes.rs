//! The content hashes a save is checked against: the scenario, the map and the gameplay scripts.
//!
//! A hash is the 16-hex-digit FNV-1a 64 of a framing of named files. The files are taken in byte order of their names; each
//! contributes its name (a `u32` length and the UTF-8 bytes), then its contents (a `u64` length and the bytes). It detects an
//! accidental mismatch, not tampering.

use std::{
    fs,
    path::{Path, PathBuf},
};

use ck_sim::fnv1a;
use serde::Deserialize;

/// The hash of `files` (name, contents), in any order.
pub(crate) fn content_hash(mut files: Vec<(String, Vec<u8>)>) -> String {
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut framed = Vec::new();
    for (name, contents) in &files {
        frame(&mut framed, name, contents);
    }
    format!("{:016x}", fnv1a(&framed))
}

fn frame(out: &mut Vec<u8>, name: &str, contents: &[u8]) {
    out.extend_from_slice(&u32::try_from(name.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&(contents.len() as u64).to_le_bytes());
    out.extend_from_slice(contents);
}

fn read(directory: &Path, name: &str) -> Result<(String, Vec<u8>), String> {
    let path = directory.join(name);
    fs::read(&path)
        .map(|bytes| (name.to_owned(), bytes))
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

#[derive(Deserialize)]
struct ScenarioKeys {
    map: String,
    locations: String,
}

#[derive(Deserialize)]
struct MapKeys {
    layers: std::collections::BTreeMap<String, String>,
}

/// The scenario's own files: `scenario.toml` and the locations file it names.
///
/// # Errors
/// A file that cannot be read, or a `scenario.toml` without its `map` and `locations` keys.
pub(crate) fn scenario_hash(directory: &Path) -> Result<String, String> {
    let (_, scenario) = read(directory, "scenario.toml")?;
    let keys = scenario_keys(&scenario)?;
    let mut files = vec![
        ("scenario.toml".to_owned(), scenario),
        read(directory, &keys.locations)?,
    ];
    if directory.join("economy.toml").exists() {
        files.push(read(directory, "economy.toml")?);
    }
    Ok(content_hash(files))
}

/// Hash shared economy rules and the map's terrain suitability inputs.
pub(crate) fn rules_hash(asset_root: &Path) -> Result<String, String> {
    let game_rules = asset_root.join("game/economy.toml");
    if !game_rules.exists() {
        // Unit fixtures that exercise only the simulation shell have no M9 data.
        return Ok(content_hash(Vec::new()));
    }
    Ok(content_hash(vec![
        read(asset_root, "game/economy.toml")?,
        read(asset_root, "map/ireland/terrain.toml")?,
    ]))
}

fn scenario_keys(scenario: &[u8]) -> Result<ScenarioKeys, String> {
    let text = std::str::from_utf8(scenario).map_err(|error| format!("scenario.toml: {error}"))?;
    toml::from_str(text).map_err(|error| format!("scenario.toml: {}", error.message()))
}

/// The directory of the scenario's map: the scenario names it (`map = "ireland"`), and the loader resolves it as
/// `<root>/map/<name>` where the scenario directory is `<root>/scenarios/<directory>`.
///
/// # Errors
/// An unreadable or malformed `scenario.toml`, or a scenario directory outside the assets layout.
pub(crate) fn map_directory(directory: &Path) -> Result<PathBuf, String> {
    let (_, scenario) = read(directory, "scenario.toml")?;
    let name = scenario_keys(&scenario)?.map;
    let absolute = std::path::absolute(directory)
        .map_err(|error| format!("cannot make {} absolute: {error}", directory.display()))?;
    let root = absolute
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| format!("{} is not below an assets root", directory.display()))?;
    Ok(root.join("map").join(name))
}

/// The map's files: `map.toml`, every file its `[layers]` names, and `location_kinds.toml` when present.
///
/// # Errors
/// A file that cannot be read, or a `map.toml` without `[layers]`.
pub(crate) fn map_hash(directory: &Path) -> Result<String, String> {
    let (_, map) = read(directory, "map.toml")?;
    let text = std::str::from_utf8(&map).map_err(|error| format!("map.toml: {error}"))?;
    let keys: MapKeys =
        toml::from_str(text).map_err(|error| format!("map.toml: {}", error.message()))?;
    // Keyed by name, so a file two layers name counts once.
    let mut files = std::collections::BTreeMap::from([("map.toml".to_owned(), map)]);
    for name in keys.layers.values() {
        let (name, bytes) = read(directory, name)?;
        files.insert(name, bytes);
    }
    if directory.join("location_kinds.toml").exists() {
        let (name, bytes) = read(directory, "location_kinds.toml")?;
        files.insert(name, bytes);
    }
    Ok(content_hash(files.into_iter().collect()))
}

/// The gameplay scripts: the entry script of the VM named `gameplay`, under the name `gameplay`, or no files.
///
/// # Errors
/// An entry script that cannot be read.
pub(crate) fn scripts_hash(gameplay_entry: Option<&Path>) -> Result<String, String> {
    let files = match gameplay_entry {
        None => Vec::new(),
        Some(entry) => {
            let bytes = fs::read(entry)
                .map_err(|error| format!("cannot read {}: {error}", entry.display()))?;
            vec![("gameplay".to_owned(), bytes)]
        }
    };
    Ok(content_hash(files))
}
