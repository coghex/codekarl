//! Loading and validating the scenario's stored land network.

use crate::water::{self, Inputs};
use crate::{Cell, Crop, LoadError, Location};
use crate::{Joint, Landing, Reach, RiverBreak, SeaMark, Strait, Water, WaterClass, WaterJoin};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

/// A validated land network, in file order.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandNetwork {
    format: u32,
    hand_list_sha256: String,
    baseline_sha256: String,
    next_node_id: u64,
    next_stretch_id: u64,
    /// Network nodes.
    #[serde(default, rename = "node")]
    pub nodes: Vec<LandNode>,
    /// Network stretches.
    #[serde(default, rename = "stretch")]
    pub stretches: Vec<LandStretch>,
    /// Ordered routes over stretches.
    #[serde(default, rename = "link")]
    pub links: Vec<LandLink>,
    /// Connections between links.
    #[serde(default, rename = "join")]
    pub joins: Vec<LandJoin>,
    /// Named roads.
    #[serde(default, rename = "road")]
    pub roads: Vec<LandRoad>,
    /// Access qualities for offshore places.
    #[serde(default)]
    pub access: Vec<LandAccess>,
    /// The narrowest river a boat can use, in millimetres: the same for every class.
    pub river_width_mm: i64,
    terrain_sha256: String,
    rivers_sha256: String,
    sea_distance: String,
    sea_distance_sha256: String,
    /// The next landing ID to give.
    pub next_landing_id: u64,
    /// The classes of boat and what water each can use.
    #[serde(default, rename = "water_class")]
    pub classes: Vec<WaterClass>,
    /// Hand-listed breaks in rivers.
    #[serde(default, rename = "river_break")]
    pub breaks: Vec<RiverBreak>,
    /// Places that need a sea-going boat.
    #[serde(default, rename = "mark")]
    pub marks: Vec<SeaMark>,
    /// The joints of the river graph.
    #[serde(default, rename = "joint")]
    pub joints: Vec<Joint>,
    /// The reaches of the river graph.
    #[serde(default, rename = "reach")]
    pub reaches: Vec<Reach>,
    /// What the river graph is joined to.
    #[serde(default, rename = "water_join")]
    pub water_joins: Vec<WaterJoin>,
    /// Where nodes meet water.
    #[serde(default, rename = "landing")]
    pub landings: Vec<Landing>,
    /// Water links between island places and their mainland landing nodes.
    #[serde(default, rename = "strait")]
    pub straits: Vec<Strait>,
    #[serde(skip)]
    water: Water,
    #[serde(skip)]
    incident: BTreeMap<u64, Vec<usize>>,
}
/// A node's role in the graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandNodeKind {
    /// A scenario location.
    Place,
    /// A landing for an offshore location.
    Landing,
    /// A junction between stretches.
    Junction,
    /// A bank at the end of a crossing stretch.
    Bank,
}
/// A stored node; cells are crop-relative column and row.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandNode {
    /// Stable node ID.
    pub id: u64,
    /// Node role.
    pub kind: LandNodeKind,
    /// Crop-relative cell.
    pub cell: [i32; 2],
    /// Location ID for place and landing nodes.
    pub place: Option<String>,
}
/// Starting category of a stretch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandCategory {
    /// A path.
    Path,
    /// A road.
    Road,
}
/// A stored stretch between nodes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandStretch {
    /// Stable stretch ID.
    pub id: u64,
    /// Starting node ID.
    pub from: u64,
    /// Ending node ID.
    pub to: u64,
    /// Crop-relative cells, in stored order.
    pub cells: Vec<[i32; 2]>,
    /// Stored length in metres.
    pub length_m: u64,
    /// Starting category.
    pub category: LandCategory,
    /// Starting level.
    pub level: u8,
    /// Whether this connects a tidal place to its landing.
    pub tidal: bool,
    /// Wide crossings, present only on a crossing stretch.
    pub crossings: Option<Vec<LandCrossing>>,
    /// Narrow crossings along this stretch.
    #[serde(default)]
    pub wades: Vec<LandWade>,
    /// The landing of a crossing stretch's first end, when it has one.
    pub landing_from: Option<u64>,
    /// The landing of a crossing stretch's last end, when it has one.
    pub landing_to: Option<u64>,
}
/// An approved wide crossing.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandCrossing {
    /// River identifier.
    pub river: String,
    /// Width in tenths of a metre.
    pub width_tenths_m: u64,
    /// The approved crossing kind.
    pub kind: LandCrossingKind,
}
/// Whether a wide crossing has a ford.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandCrossingKind {
    /// An approved ford.
    Ford,
    /// No land crossing.
    None,
}
/// A narrow crossing along a stretch.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandWade {
    /// River identifier.
    pub river: String,
    /// Width in tenths of a metre.
    pub width_tenths_m: u64,
    /// Distance along the stretch in metres.
    pub along_m: u64,
}
impl LandStretch {
    /// Whether a cart can traverse the stretch by land.
    #[must_use]
    pub fn is_land(&self) -> bool {
        !self.tidal
            && self
                .crossings
                .as_ref()
                .is_none_or(|items| items.iter().all(|c| c.kind == LandCrossingKind::Ford))
    }
}
/// Direction along a stored stretch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandDirection {
    /// From the stretch's start to its end.
    Forward,
    /// From the stretch's end to its start.
    Backward,
}
/// An ordered link through the network.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandLink {
    /// Stable link ID.
    pub id: String,
    /// Optional road ID.
    pub road: Option<String>,
    /// Starting node.
    pub from_node: u64,
    /// Ending node.
    pub to_node: u64,
    /// Stretch IDs and directions in traversal order.
    pub stretches: Vec<(u64, LandDirection)>,
    /// Opaque tool record.
    pub record: String,
}
/// Which end of a link joins a parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandEnd {
    /// The first node.
    From,
    /// The last node.
    To,
}
/// A connection at a child link's end.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandJoin {
    /// Child link ID.
    pub link: String,
    /// Child endpoint.
    pub end: LandEnd,
    /// Parent link ID.
    pub parent: String,
    /// Shared node ID.
    pub node: u64,
    /// Distance to the first occurrence on the parent.
    pub along_m: u64,
}
/// A named road and its links.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandRoad {
    /// Stable ID.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Ordered link IDs.
    pub links: Vec<String>,
}
/// Offshore access quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandAccessQuality {
    /// Offshore at all tides.
    Offshore,
    /// Offshore at high tide.
    OffshoreAtHighTide,
}
/// A place's access quality and landing.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandAccess {
    /// Scenario location ID.
    pub place: String,
    /// Access quality.
    pub quality: LandAccessQuality,
    /// Landing node ID.
    pub landing: u64,
}

impl LandNetwork {
    /// Stretches incident to the given node, in file order.
    pub fn stretches_at(&self, node: u64) -> impl Iterator<Item = &LandStretch> {
        self.incident
            .get(&node)
            .into_iter()
            .flatten()
            .map(|&index| &self.stretches[index])
    }
    /// The worked water of the network: bodies of each class, moves and positions along reaches.
    #[must_use]
    pub fn water(&self) -> &Water {
        &self.water
    }

    /// The place node for a scenario location.
    #[must_use]
    pub fn place_node(&self, place: &str) -> Option<&LandNode> {
        self.nodes
            .iter()
            .find(|n| n.kind == LandNodeKind::Place && n.place.as_deref() == Some(place))
    }
}

/// The name of the sea distance layer beside the stored network, the one name the loader and the scenario hash both read.
const SEA_DISTANCE_FILE: &str = "sea_distance.png";

fn error(entry: impl std::fmt::Display, fault: impl std::fmt::Display) -> LoadError {
    LoadError::at("roads.toml", entry, fault)
}

/// The files a stored network is worked from, besides the scenario's own.
pub(crate) struct MapFiles<'a> {
    /// The map's directory.
    pub directory: &'a Path,
    /// The loaded map.
    pub map: &'a crate::LoadedMap,
}

#[derive(Deserialize)]
struct MapLayers {
    terrain: String,
    rivers: String,
}

#[derive(Deserialize)]
struct MapNames {
    layers: MapLayers,
}

fn sha_of(directory: &Path, file: &str, label: &str) -> Result<String, LoadError> {
    let bytes = fs::read(directory.join(file)).map_err(|e| LoadError::at(label, "file", e))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn load(
    directory: &Path,
    locations: &[Location],
    files: &MapFiles<'_>,
) -> Result<Option<LandNetwork>, LoadError> {
    let crop = files.map.data.crop;
    let hand = directory.join("network.toml");
    let roads = directory.join("roads.toml");
    if !hand.exists() && !roads.exists() {
        return Ok(None);
    }
    let hand_bytes = fs::read(&hand).map_err(|e| LoadError::at("network.toml", "file", e))?;
    let source = fs::read_to_string(&roads).map_err(|e| LoadError::at("roads.toml", "file", e))?;
    let mut network: LandNetwork = toml::from_str(&source).map_err(|e| parse_error(&source, &e))?;
    if network.format != 3 {
        return Err(error("format", "must be 3"));
    }
    // The layer is read, hashed and covered by the scenario hash under one name (CK-401 rules 3, 18): a stored name that differs would
    // load a file the application's hash never reads.
    if network.sea_distance != SEA_DISTANCE_FILE {
        return Err(error(
            "sea_distance",
            format!("must be {SEA_DISTANCE_FILE}"),
        ));
    }
    if network.hand_list_sha256 != format!("{:x}", Sha256::digest(hand_bytes)) {
        return Err(error("hand_list_sha256", "does not match network.toml"));
    }
    let map_source = fs::read_to_string(files.directory.join("map.toml"))
        .map_err(|e| LoadError::at("map.toml", "file", e))?;
    let names: MapNames =
        toml::from_str(&map_source).map_err(|e| LoadError::at("map.toml", "layers", e))?;
    if network.terrain_sha256 != sha_of(files.directory, &names.layers.terrain, "terrain.png")? {
        return Err(error("terrain_sha256", "does not match the map's terrain"));
    }
    if network.rivers_sha256 != sha_of(files.directory, &names.layers.rivers, "rivers.toml")? {
        return Err(error("rivers_sha256", "does not match the map's rivers"));
    }
    if network.sea_distance_sha256
        != sha_of(directory, &network.sea_distance, &network.sea_distance)?
    {
        return Err(error(
            "sea_distance_sha256",
            "does not match the sea distance layer",
        ));
    }
    let layer = crate::loading::decode_layer_image(
        directory,
        &network.sea_distance,
        crop.width,
        crop.height,
    )?;
    validate(&network, locations, crop)?;
    network.water = water::build(
        &network,
        &Inputs {
            crop,
            cells: &files.map.data.cells,
            layer: &layer,
            rivers: files.map.river_network.as_ref(),
        },
    )?;
    for (index, stretch) in network.stretches.iter().enumerate() {
        network
            .incident
            .entry(stretch.from)
            .or_default()
            .push(index);
        if stretch.to != stretch.from {
            network.incident.entry(stretch.to).or_default().push(index);
        }
    }
    Ok(Some(network))
}

fn parse_error(source: &str, problem: &toml::de::Error) -> LoadError {
    let entry = problem
        .span()
        .and_then(|span| {
            let prefix = source.get(..span.end)?;
            let header = prefix.lines().rev().find(|line| {
                matches!(
                    *line,
                    "[[node]]"
                        | "[[stretch]]"
                        | "[[link]]"
                        | "[[join]]"
                        | "[[road]]"
                        | "[[access]]"
                )
            })?;
            let start = prefix.rfind(header)?;
            let table = source.get(start..)?.split("\n[[").next()?;
            let header = table.lines().next()?;
            let identity = table.lines().find(|line| {
                line.starts_with("id = ")
                    || line.starts_with("link = ")
                    || line.starts_with("place = ")
            });
            Some(format!("{} {}", header, identity.unwrap_or("")))
        })
        .unwrap_or_else(|| "file".to_owned());
    error(entry, problem)
}

#[expect(clippy::too_many_lines, reason = "dependent graph validation")]
fn validate(n: &LandNetwork, locations: &[Location], crop: Crop) -> Result<(), LoadError> {
    let mut nodes = BTreeMap::new();
    let mut places = BTreeMap::new();
    let mut landings = BTreeMap::new();
    let locations: BTreeMap<_, _> = locations.iter().map(|l| (l.id.as_str(), l)).collect();
    for node in &n.nodes {
        let at = format!("node[{}]", node.id);
        if nodes.insert(node.id, node).is_some() {
            return Err(error(&at, "duplicate id"));
        }
        if node.id >= n.next_node_id {
            return Err(error(&at, "id is not below next_node_id"));
        }
        let cell = Cell {
            x: crop
                .origin
                .x
                .checked_add(node.cell[0])
                .ok_or_else(|| error(&at, "cell overflow"))?,
            y: crop
                .origin
                .y
                .checked_add(node.cell[1])
                .ok_or_else(|| error(&at, "cell overflow"))?,
        };
        if !crop.contains(cell) {
            return Err(error(&at, "cell outside crop"));
        }
        match (node.kind, node.place.as_deref()) {
            (LandNodeKind::Junction | LandNodeKind::Bank, None) => (),
            (LandNodeKind::Place | LandNodeKind::Landing, Some(id)) => {
                locations
                    .get(id)
                    .ok_or_else(|| error(&at, "unknown place"))?;
                let map = if node.kind == LandNodeKind::Place {
                    &mut places
                } else {
                    &mut landings
                };
                if map.insert(id, node).is_some() {
                    return Err(error(&at, "duplicate place or landing"));
                }
            }
            _ => return Err(error(&at, "kind and place disagree")),
        }
    }
    for id in locations.keys() {
        if !places.contains_key(id) {
            return Err(error(format!("place[{id}]"), "missing place node"));
        }
    }
    let mut stretches = BTreeMap::new();
    for s in &n.stretches {
        let at = format!("stretch[{}]", s.id);
        if stretches.insert(s.id, s).is_some() {
            return Err(error(&at, "duplicate id"));
        }
        if s.id >= n.next_stretch_id {
            return Err(error(&at, "id is not below next_stretch_id"));
        }
        let from = nodes
            .get(&s.from)
            .ok_or_else(|| error(&at, "unknown from node"))?;
        let to = nodes
            .get(&s.to)
            .ok_or_else(|| error(&at, "unknown to node"))?;
        if s.level > 5 {
            return Err(error(&at, "level outside 0..=5"));
        }
        if let Some(crossings) = &s.crossings {
            if crossings.is_empty() {
                return Err(error(&at, "crossing list is empty"));
            }
            if s.tidal {
                return Err(error(&at, "tidal stretch is a crossing"));
            }
            if crossings.iter().any(|c| c.width_tenths_m < 200) {
                return Err(error(&at, "crossing width is under 20 m"));
            }
        }
        if s.wades
            .iter()
            .any(|w| w.width_tenths_m >= 200 || w.along_m > s.length_m)
        {
            return Err(error(&at, "invalid wade width or distance"));
        }
        if s.cells.first() != Some(&from.cell) || s.cells.last() != Some(&to.cell) {
            return Err(error(&at, "line endpoints differ from nodes"));
        }
        if s.cells.iter().any(|cell| {
            cell[0] < 0
                || cell[1] < 0
                || u32::try_from(cell[0]).map_or(true, |x| x >= crop.width)
                || u32::try_from(cell[1]).map_or(true, |y| y >= crop.height)
        }) {
            return Err(error(&at, "line cell outside crop"));
        }
        if s.tidal {
            let expected = if from.cell == to.cell {
                vec![from.cell]
            } else {
                vec![from.cell, to.cell]
            };
            if from.kind != LandNodeKind::Landing
                || to.kind != LandNodeKind::Place
                || from.place != to.place
                || s.cells != expected
                || (from.cell == to.cell && s.length_m != 0)
            {
                return Err(error(&at, "invalid tidal connector"));
            }
        } else {
            if s.cells.len() < 2 || s.length_m == 0 {
                return Err(error(
                    &at,
                    "non-tidal stretch needs at least two cells and positive length",
                ));
            }
            for pair in s.cells.windows(2) {
                let dx = (i64::from(pair[0][0]) - i64::from(pair[1][0])).abs();
                let dy = (i64::from(pair[0][1]) - i64::from(pair[1][1])).abs();
                if !((dx <= 1 && dy <= 1 && dx + dy > 0)
                    || (dx == 2 && dy == 1)
                    || (dx == 1 && dy == 2))
                {
                    return Err(error(&at, "invalid line step"));
                }
            }
            for cell in &s.cells[1..s.cells.len() - 1] {
                if n.nodes
                    .iter()
                    .any(|node| node.id != s.from && node.id != s.to && node.cell == *cell)
                {
                    return Err(error(&at, "line passes another node"));
                }
            }
        }
    }
    for node in n
        .nodes
        .iter()
        .filter(|node| node.kind == LandNodeKind::Bank)
    {
        if !n
            .stretches
            .iter()
            .any(|s| s.crossings.is_some() && (s.from == node.id || s.to == node.id))
        {
            return Err(error(
                format!("node[{}]", node.id),
                "bank is not an end of a crossing",
            ));
        }
    }
    let mut links = BTreeMap::new();
    for link in &n.links {
        let at = format!("link[{}]", link.id);
        if links.insert(link.id.as_str(), link).is_some() {
            return Err(error(&at, "duplicate id"));
        }
        if !nodes.contains_key(&link.from_node) || !nodes.contains_key(&link.to_node) {
            return Err(error(&at, "unknown end node"));
        }
        let mut current = link.from_node;
        for (id, direction) in &link.stretches {
            let stretch = stretches
                .get(id)
                .ok_or_else(|| error(&at, "unknown stretch"))?;
            if stretch.tidal {
                return Err(error(&at, "link traverses tidal stretch"));
            }
            let (from, to) = match direction {
                LandDirection::Forward => (stretch.from, stretch.to),
                LandDirection::Backward => (stretch.to, stretch.from),
            };
            if from != current {
                return Err(error(&at, "stretch walk is disconnected"));
            }
            current = to;
        }
        if current != link.to_node {
            return Err(error(&at, "stretch walk ends at wrong node"));
        }
    }
    let mut ends = BTreeSet::new();
    for (index, join) in n.joins.iter().enumerate() {
        let at = format!("join[{index}] ({}/{:?})", join.link, join.end);
        if !ends.insert((join.link.as_str(), join.end)) {
            return Err(error(&at, "duplicate child end"));
        }
        let child = links
            .get(join.link.as_str())
            .ok_or_else(|| error(&at, "unknown child link"))?;
        let parent = links
            .get(join.parent.as_str())
            .ok_or_else(|| error(&at, "unknown parent link"))?;
        let end = match join.end {
            LandEnd::From => child.from_node,
            LandEnd::To => child.to_node,
        };
        if join.node != end {
            return Err(error(&at, "node differs from child end"));
        }
        let mut current = parent.from_node;
        let mut distance = 0_u64;
        let mut found = (current == join.node).then_some(distance);
        for (id, direction) in &parent.stretches {
            let s = stretches[id];
            distance = distance
                .checked_add(s.length_m)
                .ok_or_else(|| error(&at, "parent distance overflow"))?;
            current = match direction {
                LandDirection::Forward => s.to,
                LandDirection::Backward => s.from,
            };
            if current == join.node && found.is_none() {
                found = Some(distance);
            }
        }
        if found != Some(join.along_m) {
            return Err(error(&at, "node or first parent distance is wrong"));
        }
    }
    for (id, node) in &places {
        let location = locations[id];
        let cell = Cell {
            x: crop.origin.x + node.cell[0],
            y: crop.origin.y + node.cell[1],
        };
        if cell != location.cell
            && !n.links.iter().any(|link| {
                link.from_node == node.id
                    && link.to_node == node.id
                    && link.stretches.is_empty()
                    && n.joins
                        .iter()
                        .any(|join| join.link == link.id && join.node == node.id)
            })
        {
            return Err(error(
                format!("node[{}]", node.id),
                "place cell differs from location without a joined singleton link",
            ));
        }
    }
    let mut roads = BTreeSet::new();
    for road in &n.roads {
        let at = format!("road[{}]", road.id);
        if !roads.insert(&road.id) {
            return Err(error(&at, "duplicate id"));
        }
        if road.links.iter().any(|id| !links.contains_key(id.as_str())) {
            return Err(error(&at, "unknown link"));
        }
    }
    for link in &n.links {
        if link.road.as_ref().is_some_and(|id| !roads.contains(id)) {
            return Err(error(format!("link[{}]", link.id), "unknown road"));
        }
    }
    let mut access = BTreeSet::new();
    for a in &n.access {
        let at = format!("access[{}]", a.place);
        if !access.insert(a.place.as_str()) {
            return Err(error(&at, "duplicate access"));
        }
        let landing = landings
            .get(a.place.as_str())
            .ok_or_else(|| error(&at, "missing landing"))?;
        if landing.id != a.landing {
            return Err(error(&at, "landing belongs to another place"));
        }
        let place = places[a.place.as_str()];
        let connected: Vec<_> = n
            .stretches
            .iter()
            .filter(|s| s.from == place.id || s.to == place.id)
            .collect();
        let valid = match a.quality {
            LandAccessQuality::Offshore => connected.is_empty(),
            LandAccessQuality::OffshoreAtHighTide => {
                connected.len() == 1 && connected[0].tidal && connected[0].from == landing.id
            }
        };
        if !valid {
            return Err(error(&at, "place has the wrong tidal connection"));
        }
    }
    for id in landings.keys() {
        if !access.contains(id) {
            return Err(error(format!("landing[{id}]"), "missing access entry"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_scenario;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn load_in(dir: &Path, scenario: &crate::Scenario) -> Result<Option<LandNetwork>, LoadError> {
        let map = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/network/map/fixture");
        load(
            dir,
            &scenario.locations,
            &MapFiles {
                directory: &map,
                map: &scenario.map,
            },
        )
    }

    fn fixture() -> crate::Scenario {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/network/scenarios/fixture");
        load_scenario(path).expect("shared fixture loads through scenario validation")
    }

    #[test]
    fn optional_pair_and_file_format_refusals_name_the_fault() {
        let scenario = fixture();
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/network/scenarios/fixture");
        let dir = std::env::temp_dir().join(format!(
            "ck-network-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::copy(
            source.join("sea_distance.png"),
            dir.join("sea_distance.png"),
        )
        .unwrap();
        assert!(load_in(&dir, &scenario).unwrap().is_none());
        fs::copy(source.join("network.toml"), dir.join("network.toml")).unwrap();
        assert!(
            load_in(&dir, &scenario)
                .unwrap_err()
                .to_string()
                .contains("roads.toml")
        );
        let roads = fs::read_to_string(source.join("roads.toml")).unwrap();
        let record = roads
            .lines()
            .find(|line| line.starts_with("record = "))
            .unwrap();
        fs::write(dir.join("roads.toml"), &roads).unwrap();
        assert!(load_in(&dir, &scenario).unwrap().is_some());
        for (bad, name) in [
            (roads.replacen("format = 3", "format = 1", 1), "format"),
            (roads.replacen("format = 3", "format = 'one'", 1), "format"),
            (
                roads.replacen("format = 3", "format = 3\nunknown = 2", 1),
                "unknown",
            ),
            (
                roads.replacen("record = ", "record_missing = ", 1),
                "record",
            ),
            (roads.replacen(record, "", 1), "approach"),
            (roads.replacen(record, "record = 7", 1), "record"),
            (
                roads.replacen("category = \"path\"", "category = \"track\"", 1),
                "stretch",
            ),
            (
                roads.replacen("end = \"to\"", "end = \"middle\"", 1),
                "approach",
            ),
            (
                roads.replacen("quality = \"offshore\"", "quality = \"unknown\"", 1),
                "access",
            ),
        ] {
            fs::write(dir.join("roads.toml"), bad).unwrap();
            let message = load_in(&dir, &scenario).unwrap_err().to_string();
            assert!(
                message.contains("roads.toml") && message.contains(name),
                "{message}"
            );
        }
        fs::write(dir.join("roads.toml"), roads).unwrap();
        fs::write(dir.join("network.toml"), "changed").unwrap();
        assert!(
            load_in(&dir, &scenario)
                .unwrap_err()
                .to_string()
                .contains("hand_list_sha256")
        );
        fs::remove_file(dir.join("network.toml")).unwrap();
        assert!(
            load_in(&dir, &scenario)
                .unwrap_err()
                .to_string()
                .contains("network.toml")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    fn png_of(width: u32, height: u32, depth: png::BitDepth) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(depth);
        let samples = usize::try_from(width * height).unwrap()
            * if depth == png::BitDepth::Sixteen {
                2
            } else {
                1
            };
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&vec![0; samples])
            .unwrap();
        bytes
    }

    #[test]
    fn a_recorded_hash_that_is_not_its_files_and_a_layer_of_the_wrong_shape_are_refused() {
        let scenario = fixture();
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/network/scenarios/fixture");
        let dir = std::env::temp_dir().join(format!(
            "ck-network-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        for name in ["network.toml", "roads.toml", "sea_distance.png"] {
            fs::copy(source.join(name), dir.join(name)).unwrap();
        }
        let roads = fs::read_to_string(source.join("roads.toml")).unwrap();
        assert!(load_in(&dir, &scenario).unwrap().is_some());
        // Each recorded sha256 must be that of the file it names.
        for field in ["terrain_sha256", "rivers_sha256", "sea_distance_sha256"] {
            let start = roads.find(&format!("{field} = \"")).unwrap() + field.len() + 4;
            let mut bad = roads.clone();
            bad.replace_range(
                start..=start,
                if &roads[start..=start] == "0" {
                    "1"
                } else {
                    "0"
                },
            );
            fs::write(dir.join("roads.toml"), bad).unwrap();
            let message = load_in(&dir, &scenario).unwrap_err().to_string();
            assert!(
                message.contains("roads.toml") && message.contains(field),
                "{message}"
            );
        }
        // A layer that is not one channel of 16 bits at the map's size is refused even with the right sha256.
        let recorded = roads
            .lines()
            .find(|line| line.starts_with("sea_distance_sha256 = "))
            .unwrap()
            .to_owned();
        for (layer, fault) in [
            (png_of(14, 10, png::BitDepth::Eight), "16-bit"),
            (png_of(13, 10, png::BitDepth::Sixteen), "dimensions"),
        ] {
            let digest = format!("{:x}", Sha256::digest(&layer));
            fs::write(dir.join("sea_distance.png"), &layer).unwrap();
            fs::write(
                dir.join("roads.toml"),
                roads.replacen(&recorded, &format!("sea_distance_sha256 = \"{digest}\""), 1),
            )
            .unwrap();
            let message = load_in(&dir, &scenario).unwrap_err().to_string();
            assert!(
                message.contains("sea_distance.png") && message.contains(fault),
                "{message}"
            );
        }
        // A stored layer name other than the one the scenario hash reads is refused, though the renamed file is the right bytes with the
        // right sha256: the loader would otherwise accept a network whose hashed file is absent.
        fs::copy(
            source.join("sea_distance.png"),
            dir.join("sea_distance.png"),
        )
        .unwrap();
        fs::rename(dir.join("sea_distance.png"), dir.join("alternate.png")).unwrap();
        fs::write(
            dir.join("roads.toml"),
            roads.replacen(
                "sea_distance = \"sea_distance.png\"",
                "sea_distance = \"alternate.png\"",
                1,
            ),
        )
        .unwrap();
        let message = load_in(&dir, &scenario).unwrap_err().to_string();
        assert!(
            message.contains("roads.toml")
                && message.contains("sea_distance")
                && message.contains("must be sea_distance.png"),
            "{message}"
        );
        fs::write(dir.join("roads.toml"), &roads).unwrap();
        fs::remove_file(dir.join("alternate.png")).unwrap();
        assert!(!dir.join("sea_distance.png").exists());
        assert!(
            load_in(&dir, &scenario)
                .unwrap_err()
                .to_string()
                .contains("sea_distance.png")
        );
        fs::write(
            dir.join("roads.toml"),
            roads.replacen("format = 3", "format = 2", 1),
        )
        .unwrap();
        assert!(
            load_in(&dir, &scenario)
                .unwrap_err()
                .to_string()
                .contains("must be 3")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shared_fixture_keeps_links_joins_and_access() {
        let scenario = fixture();
        let n = scenario.network.unwrap();
        let singleton = n.links.iter().find(|l| l.id == "singleton").unwrap();
        assert_eq!(singleton.from_node, singleton.to_node);
        assert!(singleton.stretches.is_empty());
        assert_eq!(
            n.joins
                .iter()
                .find(|j| j.link == "singleton")
                .unwrap()
                .along_m,
            949
        );
        assert_eq!(
            n.joins.iter().find(|j| j.link == "child").unwrap().along_m,
            3798
        );
        assert_eq!(n.roads[0].links, ["parent", "reverse"]);
        assert!(n.stretches.iter().any(|s| s.cells == [[1, 5], [3, 6]]));
        let parent = n.links.iter().find(|l| l.id == "parent").unwrap();
        let reverse = n.links.iter().find(|l| l.id == "reverse").unwrap();
        assert_eq!(parent.stretches[0], (4, LandDirection::Forward));
        assert_eq!(
            reverse.stretches.last(),
            Some(&(4, LandDirection::Backward))
        );
        assert!(!parent.record.is_empty());
        assert_eq!(n.access.len(), 3);
        let landing = |id: &str| {
            let access = n.access.iter().find(|a| a.place == id).unwrap();
            n.nodes
                .iter()
                .find(|node| node.id == access.landing)
                .unwrap()
        };
        assert_eq!(n.place_node("shore").unwrap().cell, landing("shore").cell);
        assert_ne!(n.place_node("isle").unwrap().cell, landing("isle").cell);
        assert!(n.stretches_at(landing("isle").id).any(|s| !s.tidal));
        assert!(
            n.stretches_at(n.place_node("tide").unwrap().id)
                .any(|s| s.tidal)
        );
    }

    #[test]
    fn crossing_fields_are_validated_and_control_land_access() {
        let scenario = fixture();
        let original = scenario.network.as_ref().unwrap();
        let check = |network: &LandNetwork, fault: &str| {
            let message = validate(network, &scenario.locations, scenario.map.data.crop)
                .unwrap_err()
                .to_string();
            assert!(
                message.contains("roads.toml") && message.contains(fault),
                "{message}"
            );
        };
        let mut n = original.clone();
        n.stretches[0].crossings = Some(vec![]);
        check(&n, "crossing list is empty");
        n.stretches[0].crossings = Some(vec![LandCrossing {
            river: "r".into(),
            width_tenths_m: 199,
            kind: LandCrossingKind::Ford,
        }]);
        check(&n, "crossing width");
        n.stretches[0].crossings.as_mut().unwrap()[0].width_tenths_m = 200;
        assert!(n.stretches[0].is_land());
        assert!(validate(&n, &scenario.locations, scenario.map.data.crop).is_ok());
        n.stretches[0].crossings.as_mut().unwrap()[0].kind = LandCrossingKind::None;
        assert!(!n.stretches[0].is_land());
        n.stretches[0].crossings = None;
        n.stretches[0].wades = vec![LandWade {
            river: "r".into(),
            width_tenths_m: 200,
            along_m: 0,
        }];
        check(&n, "wade");
        n.stretches[0].wades[0].width_tenths_m = 199;
        n.stretches[0].wades[0].along_m = n.stretches[0].length_m + 1;
        check(&n, "wade");
        let mut n = original.clone();
        let tidal = n.stretches.iter_mut().find(|s| s.tidal).unwrap();
        tidal.crossings = Some(vec![LandCrossing {
            river: "r".into(),
            width_tenths_m: 200,
            kind: LandCrossingKind::Ford,
        }]);
        check(&n, "tidal stretch is a crossing");
        let mut n = original.clone();
        let node = n
            .nodes
            .iter_mut()
            .find(|node| node.kind == LandNodeKind::Junction)
            .unwrap();
        node.kind = LandNodeKind::Bank;
        check(&n, "bank is not an end");
        n.nodes
            .iter_mut()
            .find(|node| node.kind == LandNodeKind::Bank)
            .unwrap()
            .place = Some("a".into());
        check(&n, "kind and place disagree");
        assert!(
            toml::from_str::<LandCrossing>("river = 'r'\nwidth_tenths_m = 200\nkind = 'bridge'")
                .is_err()
        );
    }

    #[test]
    fn crossing_fixture_loads_banks_fords_blocked_crossings_and_wades() {
        let scenario = fixture();
        let source = include_str!("../tests/network/scenarios/fixture/crossings-roads.toml");
        let network: LandNetwork = toml::from_str(source).unwrap();
        validate(&network, &scenario.locations, scenario.map.data.crop).unwrap();
        let crossings: Vec<_> = network
            .stretches
            .iter()
            .filter(|s| s.crossings.is_some())
            .collect();
        assert_eq!(crossings.len(), 2);
        let ford = crossings
            .iter()
            .find(|s| s.crossings.as_ref().unwrap().len() == 2)
            .unwrap();
        assert_eq!(
            ford.crossings
                .as_ref()
                .unwrap()
                .iter()
                .map(|c| c.river.as_str())
                .collect::<Vec<_>>(),
            ["wide-a", "wide-b"]
        );
        assert!(ford.is_land());
        let blocked = crossings.iter().find(|s| !s.is_land()).unwrap();
        assert_eq!(
            blocked.crossings.as_ref().unwrap()[0].kind,
            LandCrossingKind::None
        );
        assert!(
            network
                .nodes
                .iter()
                .any(|n| n.kind == LandNodeKind::Bank && n.place.is_none())
        );
        let wades: Vec<_> = network.stretches.iter().flat_map(|s| &s.wades).collect();
        assert_eq!(wades.len(), 1);
        assert_eq!(wades[0].river, "narrow");
        assert_eq!(wades[0].width_tenths_m, 199);
    }

    #[test]
    fn rejects_broken_graph_relationships_with_entry_names() {
        let scenario = fixture();
        let original = scenario.network.as_ref().unwrap();
        let check = |network: &LandNetwork, label: &str| {
            let message = validate(network, &scenario.locations, scenario.map.data.crop)
                .unwrap_err()
                .to_string();
            assert!(
                message.contains("roads.toml") && message.contains(label),
                "{message}"
            );
        };
        let mut n = original.clone();
        n.nodes.push(n.nodes[0].clone());
        check(&n, "node[");
        let mut n = original.clone();
        n.next_node_id = 1;
        check(&n, "node[");
        let mut n = original.clone();
        n.nodes[0].place = Some("unknown".into());
        check(&n, "node[");
        let mut n = original.clone();
        n.stretches.push(n.stretches[0].clone());
        check(&n, "stretch[");
        let mut n = original.clone();
        n.stretches[0].from = 9999;
        check(&n, "stretch[");
        let mut n = original.clone();
        n.stretches[0].cells[1] = [0, 0];
        check(&n, "stretch[");
        let mut n = original.clone();
        n.stretches[0].length_m = 0;
        check(&n, "stretch[");
        let mut n = original.clone();
        n.stretches[0].level = 6;
        check(&n, "stretch[");
        let mut n = original.clone();
        n.links[0].to_node = n.links[0].from_node;
        check(&n, "link[");
        let mut n = original.clone();
        n.links.push(n.links[0].clone());
        check(&n, "link[");
        let mut n = original.clone();
        n.joins[1].node = 3;
        n.joins[1].along_m = 949;
        check(&n, "join[");
        let mut n = original.clone();
        n.joins.push(n.joins[0].clone());
        check(&n, "join[");
        let mut n = original.clone();
        n.joins[0].along_m += 1;
        check(&n, "join[");
        let mut n = original.clone();
        n.roads[0].links.push("missing".into());
        check(&n, "road[");
        let mut n = original.clone();
        n.access[0].landing = 9;
        check(&n, "access[");
        let mut n = original.clone();
        n.access.remove(0);
        check(&n, "landing[");
        let mut n = original.clone();
        n.stretches.retain(|s| !s.tidal);
        check(&n, "access[tide]");
        let mut n = original.clone();
        n.access
            .iter_mut()
            .find(|a| a.place == "tide")
            .unwrap()
            .quality = LandAccessQuality::Offshore;
        check(&n, "access[tide]");
    }

    #[test]
    fn rejects_remaining_node_stretch_link_road_and_access_faults() {
        let scenario = fixture();
        let original = scenario.network.as_ref().unwrap();
        let check = |network: &LandNetwork, label: &str| {
            let message = validate(network, &scenario.locations, scenario.map.data.crop)
                .unwrap_err()
                .to_string();
            assert!(
                message.contains("roads.toml") && message.contains(label),
                "{message}"
            );
        };
        let mut n = original.clone();
        n.nodes.remove(0);
        check(&n, "place[a]");
        let mut n = original.clone();
        n.nodes[1].place = Some("a".into());
        check(&n, "node[2]");
        let mut n = original.clone();
        n.nodes[11].place = Some("a".into());
        check(&n, "node[12]");
        let mut n = original.clone();
        n.nodes[6].place = Some("missing".into());
        check(&n, "node[7]");
        let mut n = original.clone();
        n.next_stretch_id = 1;
        check(&n, "stretch[1]");
        let mut n = original.clone();
        n.stretches[0].to = 999;
        check(&n, "stretch[1]");
        let mut n = original.clone();
        n.stretches[0].cells[0] = [0, 0];
        check(&n, "stretch[1]");
        let mut n = original.clone();
        n.stretches[0].cells.truncate(1);
        check(&n, "stretch[1]");
        let mut n = original.clone();
        n.nodes.push(LandNode {
            id: 15,
            kind: LandNodeKind::Junction,
            cell: [2, 3],
            place: None,
        });
        n.next_node_id = 16;
        check(&n, "stretch[4]");
        let mut n = original.clone();
        n.stretches[11].to = 1;
        check(&n, "stretch[12]");
        let mut n = original.clone();
        n.stretches[11].length_m = 1;
        check(&n, "stretch[12]");
        let mut n = original.clone();
        n.links[0].stretches[0].0 = 999;
        check(&n, "link[approach]");
        let mut n = original.clone();
        n.links[0].stretches[0].1 = LandDirection::Forward;
        check(&n, "link[approach]");
        let mut n = original.clone();
        n.links[0].stretches[0].0 = 12;
        check(&n, "link[approach]");
        let mut n = original.clone();
        n.links[5].to_node = 4;
        check(&n, "link[singleton]");
        let mut n = original.clone();
        n.joins[0].parent = "missing".into();
        check(&n, "join[");
        let mut n = original.clone();
        n.roads.push(n.roads[0].clone());
        check(&n, "road[named]");
        let mut n = original.clone();
        n.access.push(n.access[0].clone());
        check(&n, "access[isle]");
        let mut n = original.clone();
        n.access[0].place = "a".into();
        check(&n, "access[a]");
    }

    #[test]
    fn reports_the_specific_fault_for_join_and_short_stretches() {
        let scenario = fixture();
        let original = scenario.network.as_ref().unwrap();
        let mut n = original.clone();
        n.joins[0].link = "missing".into();
        let message = validate(&n, &scenario.locations, scenario.map.data.crop)
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("join[0]") && message.contains("unknown child link"),
            "{message}"
        );
        let mut n = original.clone();
        n.stretches[0].to = n.stretches[0].from;
        n.stretches[0].cells.truncate(1);
        let message = validate(&n, &scenario.locations, scenario.map.data.crop)
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("stretch[1]") && message.contains("at least two cells"),
            "{message}"
        );
        let mut n = original.clone();
        let cell = n.stretches[11].cells[0];
        n.stretches[11].cells.push(cell);
        let message = validate(&n, &scenario.locations, scenario.map.data.crop)
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("stretch[12]") && message.contains("invalid tidal connector"),
            "{message}"
        );
    }

    #[test]
    fn ireland_reachability_and_access_match_the_committed_network() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenarios/ireland-768");
        let scenario = load_scenario(path).unwrap();
        let n = scenario.network.unwrap();
        assert_eq!(
            n.nodes
                .iter()
                .filter(|node| node.kind == LandNodeKind::Place)
                .count(),
            125
        );
        assert_eq!(
            n.access
                .iter()
                .filter(|a| a.quality == LandAccessQuality::Offshore)
                .count(),
            22
        );
        assert_eq!(
            n.access
                .iter()
                .filter(|a| a.quality == LandAccessQuality::OffshoreAtHighTide)
                .count(),
            2
        );
        let start = n.place_node("tara").unwrap().id;
        let walk = |include_tidal| {
            let mut seen = BTreeSet::from([start]);
            let mut stack = vec![start];
            while let Some(id) = stack.pop() {
                for s in n.stretches_at(id).filter(|s| include_tidal || !s.tidal) {
                    let other = if s.from == id { s.to } else { s.from };
                    if seen.insert(other) {
                        stack.push(other);
                    }
                }
            }
            seen
        };
        let land = walk(false);
        let all = walk(true);
        for node in n
            .nodes
            .iter()
            .filter(|node| node.kind == LandNodeKind::Place)
        {
            let quality = n
                .access
                .iter()
                .find(|a| Some(a.place.as_str()) == node.place.as_deref());
            assert_eq!(land.contains(&node.id), quality.is_none());
            assert_eq!(
                all.contains(&node.id),
                quality.is_none_or(|a| a.quality == LandAccessQuality::OffshoreAtHighTide)
            );
        }
        for access in &n.access {
            assert!(land.contains(&access.landing));
        }
        let inish = n.place_node("inishmacsaint").unwrap();
        assert_eq!(inish.cell, [418, 258]);
        assert_eq!(n.stretches_at(inish.id).count(), 0);
        let landing = n
            .access
            .iter()
            .find(|a| a.place == "inishmacsaint")
            .unwrap();
        assert_eq!(landing.quality, LandAccessQuality::Offshore);
        assert_eq!(
            n.nodes
                .iter()
                .find(|node| node.id == landing.landing)
                .unwrap()
                .cell,
            [418, 259]
        );
    }
}
