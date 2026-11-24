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
    id: Option<String>,
    map: String,
    locations: String,
}

#[derive(Deserialize)]
struct MapKeys {
    layers: std::collections::BTreeMap<String, String>,
}

/// The scenario's own files: `scenario.toml`, the locations file it names, its economy and the real figures it ships.
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
    // Both network files are scenario content; a partial pair is an error.
    let hand_list = directory.join("network.toml").exists();
    let roads = directory.join("roads.toml").exists();
    if hand_list || roads {
        files.push(read(directory, "network.toml")?);
        files.push(read(directory, "roads.toml")?);
    }
    // The real figures a scenario ships (CK-370) join its hash.
    for path in figure_files(directory)?.iter().flatten() {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        files.push(read(directory, name)?);
    }
    Ok(content_hash(files))
}

/// The scenarios that start with real figures (CK-370): by id, so that a scenario moved or renamed is still found.
const FIGURE_SCENARIOS: [&str; 1] = ["ireland_768"];

/// The three files of the real figures a supported scenario ships, the research file and the load file (CK-370), named by the scenario's **own id**
/// (`ireland_768` is `ireland-768.toml` and `ireland-768-load.toml`) wherever its directory is, and the house names `houses.toml` (CK-375), so that
/// loading and hashing find the same files of a scenario that is moved or renamed. A scenario that is not one of [`FIGURE_SCENARIOS`] is outside the 768 start (`None`); a supported one
/// without either file is refused, never started without its characters.
///
/// # Errors
/// An unreadable `scenario.toml`, or a supported scenario missing any of its three files.
pub(crate) fn figure_files(directory: &Path) -> Result<Option<[PathBuf; 3]>, String> {
    let (_, scenario) = read(directory, "scenario.toml")?;
    let Some(id) = scenario_keys(&scenario)?.id else {
        return Ok(None);
    };
    // Only the scenarios that ship real figures have this start; another scenario is outside it, whatever files its folder holds.
    if !FIGURE_SCENARIOS.contains(&id.as_str()) {
        return Ok(None);
    }
    let base = id.replace('_', "-");
    let files = [
        directory.join(format!("{base}.toml")),
        directory.join(format!("{base}-load.toml")),
        directory.join("houses.toml"),
    ];
    let missing: Vec<String> = files
        .iter()
        .filter(|path| !path.exists())
        .filter_map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .collect();
    if missing.is_empty() {
        Ok(Some(files))
    } else {
        Err(format!(
            "scenario `{id}` starts with its real figures and needs its figure files ({base}.toml, {base}-load.toml, houses.toml): missing {}",
            missing.join(", ")
        ))
    }
}

/// Hash shared economy rules (including the genetic, personality, character event, life-cycle and generator data) and the map's terrain suitability inputs.
pub(crate) fn rules_hash(asset_root: &Path) -> Result<String, String> {
    let game_rules = asset_root.join("game/economy.toml");
    if !game_rules.exists() {
        // Unit fixtures that exercise only the simulation shell have no M9 data.
        return Ok(content_hash(Vec::new()));
    }
    Ok(content_hash(vec![
        read(asset_root, "game/economy.toml")?,
        read(asset_root, "game/weather.toml")?,
        read(asset_root, "game/peoples.toml")?,
        read(asset_root, "game/genetics/haplogroups.toml")?,
        read(asset_root, "game/genetics/traits.toml")?,
        read(asset_root, "game/personality.toml")?,
        read(asset_root, "game/events/character-events.toml")?,
        read(asset_root, "game/events/attributes-skills.toml")?,
        read(asset_root, "game/lifecycle/life-cycle.toml")?,
        read(asset_root, "game/lifecycle/names.toml")?,
        read(asset_root, "game/lifecycle/epithets.toml")?,
        read(asset_root, "game/lifecycle/connections.toml")?,
        read(asset_root, "game/lifecycle/generator.toml")?,
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
