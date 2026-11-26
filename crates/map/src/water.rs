//! The water of the stored network (format 3): the classes of boat, the river graph, the landings and the straits.
//!
//! The stored river graph is the only source of the rivers' water: this module reads the survey's lines only to find a point to draw
//! ([`Water::position_along`]). Open water comes from the terrain and the sea-distance layer; the bodies of each class are worked from
//! both, and the stored counts, the landings' bodies and the joins' classes are checked against them.

use crate::network::{LandEnd, LandNetwork, LandNodeKind};
use crate::{Crop, FixedPoint, LoadError, RiverNetwork, RiverStretchKind, Surface};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// The furthest a class's reach may be, in metres: a layer sample of more than this was cut short.
pub const MAX_REACH_M: u32 = 65_533;
const MICRO: u128 = 1_000_000;

/// A kind of boat and the water it can use at sea.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaterClass {
    /// Class name.
    pub id: String,
    /// The furthest from land a sea cell may be, in metres.
    pub reach_m: u32,
    /// Whether the class may be at places that need a sea-going boat.
    pub sea_going: bool,
    /// The count of bodies of this class, as the tool worked it.
    pub bodies: usize,
}

/// A hand-listed break in a river, where falls or rapids cut it for every class.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiverBreak {
    /// The break's name.
    pub name: String,
    /// The surveyed stretch it cuts.
    pub stretch: String,
    /// The crop-relative cell it is in.
    pub cell: [i32; 2],
    /// The joint at the end of the part above the break.
    pub above: usize,
    /// The joint at the start of the part below the break.
    pub below: usize,
}

/// A place that needs a sea-going boat.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeaMark {
    /// Scenario location ID.
    pub place: String,
}

/// A point of a surveyed stretch where the river graph is divided.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Joint {
    /// The joint's number; joints are numbered from 0 in file order.
    pub id: usize,
    /// The surveyed stretch.
    pub stretch: String,
    /// The crop-relative cell that holds the joint.
    pub cell: [i32; 2],
}

/// The surveyed kind of a reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReachKind {
    /// Inland river.
    River,
    /// Tidal water.
    Tidal,
    /// A line through a lough.
    Lough,
}

/// The line between two joints next to each other on a stretch, going downstream.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reach {
    /// The reach's number; reaches are numbered from 0 in file order.
    pub id: usize,
    /// The surveyed stretch.
    pub stretch: String,
    /// The upstream joint.
    pub from: usize,
    /// The downstream joint.
    pub to: usize,
    /// Whether boats can use it.
    pub usable: bool,
    /// Its ground length in whole millimetres, rounded up.
    pub length_mm: u64,
    /// The stretch's kind.
    pub kind: ReachKind,
    /// Where it begins along its stretch, in millionths of the stretch's length.
    pub begin: u32,
    /// Where it ends along its stretch, in millionths of the stretch's length.
    pub end: u32,
}

/// A join: two reaches' ends, or a reach and an open cell.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaterJoin {
    /// The reach whose end (when `to_reach` is given) or start (for a cell) is joined.
    pub reach: usize,
    /// The first reach of the stretch the first one ends in.
    pub to_reach: Option<usize>,
    /// The crop-relative open cell the reach's first joint is joined to.
    pub cell: Option<[i32; 2]>,
    /// The classes that can use the join.
    pub classes: Vec<String>,
}

/// A bank of a river, facing downstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bank {
    /// The left bank.
    Left,
    /// The right bank.
    Right,
}

/// A landing: where a node meets water.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Landing {
    /// Stable landing ID, kept while its key is unchanged.
    pub id: u64,
    /// The node.
    pub node: u64,
    /// The bank, for a river contact.
    pub bank: Option<Bank>,
    /// The surveyed stretch of a river contact that is not a crossing end's.
    pub survey: Option<String>,
    /// The land cell where the traveller meets the water.
    pub shore: [i32; 2],
    /// Which run of the surveyed stretch in the shore cell, counted along the stretch.
    pub run: Option<u32>,
    /// The open cell of an open contact.
    pub cell: Option<[i32; 2]>,
    /// The joint of a river contact.
    pub joint: Option<usize>,
    /// The crossing stretch of a crossing end's landing.
    pub crossing: Option<u64>,
    /// Which end of that crossing stretch.
    pub end: Option<LandEnd>,
    /// The approach over land, in whole metres.
    pub approach_m: u64,
    /// For each class, the body its contact is in, or -1.
    pub bodies: Vec<i64>,
}

/// A water link between a place and its mainland landing node.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Strait {
    /// The place.
    pub place: String,
    /// The place's node.
    pub from: u64,
    /// The mainland landing node.
    pub to: u64,
    /// Length in metres.
    pub length_m: u64,
    /// The classes that may use it.
    pub classes: Vec<String>,
}

/// What a class can use and which bodies of water it has, worked from the stored graph.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ClassWater {
    usable: Vec<bool>,
    cell_body: Vec<i64>,
    joint_body: Vec<i64>,
    count: usize,
}

/// The worked water of a stored network: bodies, moves and positions along reaches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Water {
    width: usize,
    height: usize,
    classes: Vec<ClassWater>,
    survey: BTreeMap<String, Vec<FixedPoint>>,
}

/// What the water is worked from: the map's terrain, the rivers and the layer.
pub(crate) struct Inputs<'a> {
    pub crop: Crop,
    pub cells: &'a [Surface],
    pub layer: &'a [u16],
    pub rivers: Option<&'a RiverNetwork>,
}

fn error(entry: impl std::fmt::Display, fault: impl std::fmt::Display) -> LoadError {
    LoadError::at("roads.toml", entry, fault)
}

struct Sets(Vec<usize>);

impl Sets {
    fn new(size: usize) -> Self {
        Self((0..size).collect())
    }
    fn find(&mut self, mut item: usize) -> usize {
        while self.0[item] != item {
            self.0[item] = self.0[self.0[item]];
            item = self.0[item];
        }
        item
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b);
        }
    }
}

fn class_usable(class: &WaterClass, inputs: &Inputs<'_>) -> Vec<bool> {
    inputs
        .cells
        .iter()
        .zip(inputs.layer)
        .map(|(surface, distance)| match surface {
            Surface::Lake => true,
            Surface::Sea { .. } => u32::from(*distance) <= class.reach_m,
            Surface::Land { .. } => false,
        })
        .collect()
}

fn index(cell: [i32; 2], width: usize, height: usize) -> Option<usize> {
    let x = usize::try_from(cell[0]).ok()?;
    let y = usize::try_from(cell[1]).ok()?;
    (x < width && y < height).then_some(y * width + x)
}

fn is_open(surface: Surface) -> bool {
    matches!(surface, Surface::Sea { .. } | Surface::Lake)
}

/// A stored join's classes, as the loader finds them.
fn derived_classes(
    join: &WaterJoin,
    classes: &[WaterClass],
    usable: &[Vec<bool>],
    width: usize,
    height: usize,
) -> Vec<String> {
    match join.cell {
        None => classes.iter().map(|c| c.id.clone()).collect(),
        Some(cell) => {
            let at = index(cell, width, height);
            classes
                .iter()
                .zip(usable)
                .filter(|(_, cells)| at.is_some_and(|i| cells[i]))
                .map(|(c, _)| c.id.clone())
                .collect()
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "each stored table is checked against the others in turn"
)]
pub(crate) fn build(n: &LandNetwork, inputs: &Inputs<'_>) -> Result<Water, LoadError> {
    let (width, height) = (
        usize::try_from(inputs.crop.width).unwrap_or(0),
        usize::try_from(inputs.crop.height).unwrap_or(0),
    );
    let survey: BTreeMap<&str, (usize, &crate::RiverStretch)> = inputs
        .rivers
        .map(|r| {
            r.stretches
                .iter()
                .enumerate()
                .map(|(i, s)| (s.id.as_str(), (i, s)))
                .collect()
        })
        .unwrap_or_default();
    let mut names = BTreeSet::new();
    for (number, class) in n.classes.iter().enumerate() {
        let at = format!("water_class[{}]", class.id);
        if class.id.is_empty() || !names.insert(class.id.as_str()) {
            return Err(error(
                format!("water_class[{number}]"),
                "id is empty or repeated",
            ));
        }
        if class.reach_m > MAX_REACH_M {
            return Err(error(&at, "reach is outside nothing to 65,533 m"));
        }
    }
    if n.river_width_mm < 0 {
        return Err(error("river_width_mm", "the narrowest river is negative"));
    }
    for (number, joint) in n.joints.iter().enumerate() {
        let at = format!("joint[{number}]");
        if joint.id != number {
            return Err(error(&at, "joints are not numbered from 0 in order"));
        }
        if !survey.contains_key(joint.stretch.as_str()) {
            return Err(error(
                &at,
                format!("unknown surveyed stretch {}", joint.stretch),
            ));
        }
        if index(joint.cell, width, height).is_none() {
            return Err(error(&at, "cell outside crop"));
        }
    }
    for (number, reach) in n.reaches.iter().enumerate() {
        let at = format!("reach[{number}]");
        if reach.id != number {
            return Err(error(&at, "reaches are not numbered from 0 in order"));
        }
        let Some(&(_, stretch)) = survey.get(reach.stretch.as_str()) else {
            return Err(error(
                &at,
                format!("unknown surveyed stretch {}", reach.stretch),
            ));
        };
        for joint in [reach.from, reach.to] {
            let found = n
                .joints
                .get(joint)
                .ok_or_else(|| error(&at, format!("names joint {joint}, which does not exist")))?;
            if found.stretch != reach.stretch {
                return Err(error(&at, format!("joint {joint} is on another stretch")));
            }
        }
        if reach.length_mm == 0 {
            return Err(error(&at, "the reach's length is nothing"));
        }
        if reach.begin > reach.end {
            return Err(error(&at, "the reach begins after it ends"));
        }
        if u128::from(reach.end) > MICRO {
            return Err(error(&at, "the reach ends outside its stretch"));
        }
        let kind = match stretch.kind {
            RiverStretchKind::River => ReachKind::River,
            RiverStretchKind::Tidal => ReachKind::Tidal,
            RiverStretchKind::Lough => ReachKind::Lough,
        };
        if reach.kind != kind {
            return Err(error(&at, "kind differs from the surveyed stretch"));
        }
    }
    let usable: Vec<Vec<bool>> = n.classes.iter().map(|c| class_usable(c, inputs)).collect();
    for (number, join) in n.water_joins.iter().enumerate() {
        let at = format!("water_join[{number}]");
        let reach = n
            .reaches
            .get(join.reach)
            .ok_or_else(|| error(&at, "names a reach that does not exist"))?;
        if !reach.usable {
            return Err(error(&at, "is a join to a reach that is not usable"));
        }
        match (join.to_reach, join.cell) {
            (Some(other), None) => {
                let other = n
                    .reaches
                    .get(other)
                    .ok_or_else(|| error(&at, "names a reach that does not exist"))?;
                if !other.usable {
                    return Err(error(&at, "is a join to a reach that is not usable"));
                }
                let (&(_, own), &(next, _)) = (
                    survey
                        .get(reach.stretch.as_str())
                        .ok_or_else(|| error(&at, "unknown stretch"))?,
                    survey
                        .get(other.stretch.as_str())
                        .ok_or_else(|| error(&at, "unknown stretch"))?,
                );
                if own.ends != Some(next) {
                    return Err(error(&at, "the first stretch does not end in the second"));
                }
            }
            (None, Some(cell)) => {
                let at_cell =
                    index(cell, width, height).ok_or_else(|| error(&at, "cell outside crop"))?;
                if !is_open(inputs.cells[at_cell]) {
                    return Err(error(&at, "joins a cell that is not open water"));
                }
            }
            _ => return Err(error(&at, "must name either a second reach or a cell")),
        }
        if join.classes != derived_classes(join, &n.classes, &usable, width, height) {
            return Err(error(
                &at,
                "classes differ from those that can use the join",
            ));
        }
    }
    for item in &n.breaks {
        let at = format!("river_break[{}]", item.name);
        for joint in [item.above, item.below] {
            let found = n
                .joints
                .get(joint)
                .ok_or_else(|| error(&at, format!("names joint {joint}, which does not exist")))?;
            if found.stretch != item.stretch {
                return Err(error(&at, "a joint is on another stretch"));
            }
        }
        if index(item.cell, width, height).is_none() {
            return Err(error(&at, "cell outside crop"));
        }
    }
    let mut water = Water {
        width,
        height,
        classes: Vec::new(),
        survey: survey
            .iter()
            .map(|(id, (_, s))| ((*id).to_owned(), s.points.clone()))
            .collect(),
    };
    for (number, class) in n.classes.iter().enumerate() {
        let worked = work_class(n, number, usable[number].clone(), width, height);
        if worked.count != class.bodies {
            return Err(error(
                format!("water_class[{}]", class.id),
                format!(
                    "stored count of bodies {} is not the loader's {}",
                    class.bodies, worked.count
                ),
            ));
        }
        water.classes.push(worked);
    }
    check_landings(n, &water, inputs)?;
    check_straits(n)?;
    Ok(water)
}

fn work_class(
    n: &LandNetwork,
    class: usize,
    usable: Vec<bool>,
    width: usize,
    height: usize,
) -> ClassWater {
    let cells = width * height;
    let mut sets = Sets::new(cells + n.joints.len());
    for y in 0..height {
        for x in 0..width {
            let here = y * width + x;
            if !usable[here] {
                continue;
            }
            if x + 1 < width && usable[here + 1] {
                sets.union(here, here + 1);
            }
            if y + 1 < height && usable[here + width] {
                sets.union(here, here + width);
            }
        }
    }
    let mut member = vec![false; n.joints.len()];
    for reach in n.reaches.iter().filter(|r| r.usable) {
        member[reach.from] = true;
        member[reach.to] = true;
        sets.union(cells + reach.from, cells + reach.to);
    }
    for join in &n.water_joins {
        let first = &n.reaches[join.reach];
        match (join.to_reach, join.cell) {
            (Some(other), _) => sets.union(cells + first.to, cells + n.reaches[other].from),
            (None, Some(cell)) => {
                if let Some(at) = index(cell, width, height).filter(|&at| usable[at]) {
                    sets.union(cells + first.from, at);
                }
            }
            (None, None) => {}
        }
    }
    let _ = class;
    let mut ids: BTreeMap<usize, i64> = BTreeMap::new();
    let mut cell_body = vec![-1; cells];
    for (at, &open) in usable.iter().enumerate() {
        if open {
            let root = sets.find(at);
            let next = i64::try_from(ids.len()).unwrap_or(i64::MAX);
            cell_body[at] = *ids.entry(root).or_insert(next);
        }
    }
    let mut joint_body = vec![-1; n.joints.len()];
    for (at, &is_member) in member.iter().enumerate() {
        if is_member {
            let root = sets.find(cells + at);
            let next = i64::try_from(ids.len()).unwrap_or(i64::MAX);
            joint_body[at] = *ids.entry(root).or_insert(next);
        }
    }
    ClassWater {
        usable,
        cell_body,
        joint_body,
        count: ids.len(),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "each landing is checked as a contact, against its bodies and against the crossing stretches that name it"
)]
fn check_landings(n: &LandNetwork, water: &Water, inputs: &Inputs<'_>) -> Result<(), LoadError> {
    let nodes: BTreeMap<u64, _> = n.nodes.iter().map(|node| (node.id, node)).collect();
    let marked: BTreeSet<&str> = n.marks.iter().map(|m| m.place.as_str()).collect();
    for mark in &n.marks {
        if !n.nodes.iter().any(|node| {
            node.kind == LandNodeKind::Place && node.place.as_deref() == Some(mark.place.as_str())
        }) {
            return Err(error(format!("mark[{}]", mark.place), "unknown place"));
        }
    }
    let mut ids = BTreeSet::new();
    let mut by_end: BTreeMap<(u64, LandEnd), u64> = BTreeMap::new();
    for landing in &n.landings {
        let at = format!("landing[{}]", landing.id);
        if !ids.insert(landing.id) {
            return Err(error(&at, "duplicate id"));
        }
        if landing.id >= n.next_landing_id {
            return Err(error(&at, "id is not below next_landing_id"));
        }
        let node = nodes
            .get(&landing.node)
            .ok_or_else(|| error(&at, "names a node that does not exist"))?;
        if landing.bodies.len() != n.classes.len() {
            return Err(error(&at, "needs one body for each class"));
        }
        let shore = index(landing.shore, water.width, water.height)
            .ok_or_else(|| error(&at, "shore cell outside crop"))?;
        if !matches!(inputs.cells[shore], Surface::Land { .. }) {
            return Err(error(&at, "shore cell is not land"));
        }
        let contact = match (landing.cell, landing.joint, landing.crossing) {
            (Some(cell), None, None)
                if landing.survey.is_none()
                    && landing.bank.is_none()
                    && landing.end.is_none()
                    && landing.run.is_none() =>
            {
                let at_cell = index(cell, water.width, water.height)
                    .ok_or_else(|| error(&at, "contact cell outside crop"))?;
                if !is_open(inputs.cells[at_cell])
                    || !water.classes.iter().any(|c| c.usable[at_cell])
                {
                    return Err(error(&at, "contact is not usable water"));
                }
                Contact::Cell(at_cell)
            }
            (None, Some(joint), None)
                if landing.survey.is_some()
                    && landing.bank.is_some()
                    && landing.run.is_some()
                    && landing.end.is_none() =>
            {
                let found = n
                    .joints
                    .get(joint)
                    .ok_or_else(|| error(&at, "names a joint that does not exist"))?;
                if Some(&found.stretch) != landing.survey.as_ref() {
                    return Err(error(&at, "contact joint is on another stretch"));
                }
                Contact::Joint(joint)
            }
            (None, Some(joint), Some(crossing))
                if landing.survey.is_none()
                    && landing.bank.is_some()
                    && landing.run.is_none()
                    && landing.end.is_some() =>
            {
                n.joints
                    .get(joint)
                    .ok_or_else(|| error(&at, "names a joint that does not exist"))?;
                let end = landing.end.unwrap_or(LandEnd::From);
                if by_end.insert((crossing, end), landing.id).is_some() {
                    return Err(error(
                        &at,
                        "another landing is keyed by the same crossing end",
                    ));
                }
                Contact::Joint(joint)
            }
            _ => {
                return Err(error(
                    &at,
                    "must have one contact: an open cell, a river joint or a crossing end",
                ));
            }
        };
        if let Contact::Joint(joint) = contact
            && water.classes.iter().all(|c| c.joint_body[joint] < 0)
            && !n.classes.is_empty()
        {
            return Err(error(&at, "contact is not usable water"));
        }
        let place_marked = node.place.as_deref().is_some_and(|p| marked.contains(p));
        for (class, stored) in n.classes.iter().zip(&landing.bodies) {
            let number = n.classes.iter().position(|c| c.id == class.id).unwrap_or(0);
            let found = if place_marked && !class.sea_going {
                -1
            } else {
                match contact {
                    Contact::Cell(at_cell) => water.classes[number].cell_body[at_cell],
                    Contact::Joint(joint) => water.classes[number].joint_body[joint],
                }
            };
            if *stored != found {
                return Err(error(
                    &at,
                    format!(
                        "stored body {stored} for class {} is not what the loader finds, {found}",
                        class.id
                    ),
                ));
            }
        }
    }
    for stretch in &n.stretches {
        for (end, landing_id, node) in [
            (LandEnd::From, stretch.landing_from, stretch.from),
            (LandEnd::To, stretch.landing_to, stretch.to),
        ] {
            let at = format!("stretch[{}]", stretch.id);
            let Some(landing_id) = landing_id else {
                continue;
            };
            if stretch.crossings.is_none() {
                return Err(error(&at, "names a landing but is not a crossing stretch"));
            }
            let landing = n
                .landings
                .iter()
                .find(|l| l.id == landing_id)
                .ok_or_else(|| error(&at, "crossing end names a landing that does not exist"))?;
            if landing.crossing != Some(stretch.id)
                || landing.end != Some(end)
                || landing.node != node
            {
                return Err(error(
                    &at,
                    "crossing end names a landing that is not keyed by that crossing stretch and end, or whose node is not that end's node",
                ));
            }
        }
    }
    for ((crossing, end), landing_id) in by_end {
        let named = n
            .stretches
            .iter()
            .find(|s| s.id == crossing)
            .and_then(|s| match end {
                LandEnd::From => s.landing_from,
                LandEnd::To => s.landing_to,
            });
        if named != Some(landing_id) {
            return Err(error(
                format!("landing[{landing_id}]"),
                "is keyed by a crossing end that no crossing stretch names, or another names",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Contact {
    Cell(usize),
    Joint(usize),
}

fn check_straits(n: &LandNetwork) -> Result<(), LoadError> {
    let marked: BTreeSet<&str> = n.marks.iter().map(|m| m.place.as_str()).collect();
    let mut places = BTreeSet::new();
    for strait in &n.straits {
        let at = format!("strait[{}]", strait.place);
        if !places.insert(strait.place.as_str()) {
            return Err(error(&at, "duplicate strait"));
        }
        let access = n
            .access
            .iter()
            .find(|a| a.place == strait.place)
            .ok_or_else(|| error(&at, "place has no access quality"))?;
        let place = n
            .place_node(&strait.place)
            .ok_or_else(|| error(&at, "unknown place"))?;
        if strait.from != place.id || strait.to != access.landing {
            return Err(error(
                &at,
                "does not join the place to its mainland landing node",
            ));
        }
        if strait.length_m == 0 {
            return Err(error(&at, "length is nothing"));
        }
        let expected: Vec<String> = n
            .classes
            .iter()
            .filter(|c| c.sea_going || !marked.contains(strait.place.as_str()))
            .map(|c| c.id.clone())
            .collect();
        if strait.classes != expected {
            return Err(error(
                &at,
                "classes are not every class less those a sea-going mark takes away",
            ));
        }
    }
    for access in &n.access {
        let has_landing = n.landings.iter().any(|l| l.node == access.landing);
        if !has_landing && !places.contains(access.place.as_str()) {
            return Err(error(
                format!("access[{}]", access.place),
                "place has neither a landing nor a strait",
            ));
        }
    }
    Ok(())
}

impl Water {
    /// The number of bodies of a class.
    #[must_use]
    pub fn body_count(&self, class: usize) -> usize {
        self.classes.get(class).map_or(0, |c| c.count)
    }

    /// Whether a class can use an open cell (crop-relative): every lake cell, and the sea within its reach.
    #[must_use]
    pub fn cell_usable(&self, class: usize, cell: [i32; 2]) -> bool {
        index(cell, self.width, self.height)
            .zip(self.classes.get(class))
            .is_some_and(|(at, c)| c.usable[at])
    }

    /// The body a usable open cell is in.
    #[must_use]
    pub fn cell_body(&self, class: usize, cell: [i32; 2]) -> Option<usize> {
        let at = index(cell, self.width, self.height)?;
        usize::try_from(self.classes.get(class)?.cell_body[at]).ok()
    }

    /// The body a joint of the river graph is in, when it is in usable water.
    #[must_use]
    pub fn joint_body(&self, class: usize, joint: usize) -> Option<usize> {
        usize::try_from(*self.classes.get(class)?.joint_body.get(joint)?).ok()
    }

    /// The legal moves of a boat over open water from a usable cell: to a cell sharing an edge, and to a cell touching
    /// at a corner only where both cells beside that corner are usable too.
    #[must_use]
    pub fn moves(&self, class: usize, cell: [i32; 2]) -> Vec<[i32; 2]> {
        let usable = |c: [i32; 2]| self.cell_usable(class, c);
        if !usable(cell) {
            return Vec::new();
        }
        let mut result = Vec::new();
        for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
            let next = [cell[0] + dx, cell[1] + dy];
            if usable(next) {
                result.push(next);
            }
        }
        for (dx, dy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
            let next = [cell[0] + dx, cell[1] + dy];
            if usable(next) && usable([cell[0] + dx, cell[1]]) && usable([cell[0], cell[1] + dy]) {
                result.push(next);
            }
        }
        result
    }

    /// The distances along a reach, in millimetres, strictly between its two ends, at which its surveyed line bends: the vertices of the
    /// line that lie between the reach's stored beginning and end, placed as [`Self::position_along`] places them. Between two of them
    /// (and the ends) the reach is a straight line. Empty for a reach whose line is unknown or straight.
    #[must_use]
    pub fn bends_along(&self, reach: &Reach) -> Vec<u64> {
        let Some(line) = self.survey.get(&reach.stretch) else {
            return Vec::new();
        };
        let length = u128::from(reach.length_mm);
        let (begin, end) = (u128::from(reach.begin), u128::from(reach.end));
        let Some(span) = end.checked_sub(begin).filter(|span| *span > 0) else {
            return Vec::new();
        };
        let mut lengths = Vec::new();
        for pair in line.windows(2) {
            let dx = i128::from(pair[1].x) - i128::from(pair[0].x);
            let dy = i128::from(pair[1].y) - i128::from(pair[0].y);
            let Ok(squared) = u128::try_from(dx * dx + dy * dy) else {
                return Vec::new();
            };
            lengths.push(isqrt(squared));
        }
        let total: u128 = lengths.iter().sum();
        let mut bends = Vec::new();
        let mut before = 0_u128;
        for segment in lengths.iter().take(lengths.len().saturating_sub(1)) {
            before += segment;
            // The vertex lies at `before / total` of the line, a millionth-scaled position; the reach covers `begin..end` of it.
            let at = before * MICRO;
            if total > 0 && at > begin * total && at < end * total {
                let along = (at - begin * total) * length / (span * total);
                if let Ok(along) = u64::try_from(along)
                    && along > 0
                    && u128::from(along) < length
                {
                    bends.push(along);
                }
            }
        }
        bends.dedup();
        bends
    }

    /// The point of the surveyed line a distance along a reach, in proportion between its stored beginning and end.
    #[must_use]
    pub fn position_along(&self, reach: &Reach, distance_mm: u64) -> Option<FixedPoint> {
        let line = self.survey.get(&reach.stretch)?;
        let length = u128::from(reach.length_mm);
        let along = u128::from(distance_mm).min(length);
        let begin = u128::from(reach.begin);
        let span = u128::from(reach.end.checked_sub(reach.begin)?);
        let position = begin * length + span * along; // millionths of the stretch, times length
        let mut lengths = Vec::new();
        for pair in line.windows(2) {
            let dx = i128::from(pair[1].x) - i128::from(pair[0].x);
            let dy = i128::from(pair[1].y) - i128::from(pair[0].y);
            lengths.push(isqrt(u128::try_from(dx * dx + dy * dy).ok()?));
        }
        let total: u128 = lengths.iter().sum();
        let target = total * position; // scaled by MICRO * length
        let scale = MICRO * length;
        let mut before = 0_u128;
        for (pair, &segment) in line.windows(2).zip(&lengths) {
            if segment == 0 {
                continue;
            }
            if target <= (before + segment) * scale {
                let into = i128::try_from(target.saturating_sub(before * scale)).ok()?;
                let size = i128::try_from(segment * scale).ok()?;
                let lerp = |a: i32, b: i32| -> Option<i32> {
                    let (a, b) = (i128::from(a), i128::from(b));
                    i32::try_from(a + ((b - a) * into + size / 2).div_euclid(size)).ok()
                };
                return Some(FixedPoint {
                    x: lerp(pair[0].x, pair[1].x)?,
                    y: lerp(pair[0].y, pair[1].y)?,
                });
            }
            before += segment;
        }
        line.last().copied()
    }
}

fn isqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut x = value;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = x.midpoint(value / x);
    }
    x
}

/// Where the cells of a landing's shore are looked up by the game.
impl LandNetwork {
    /// The landings of a node, in file order.
    pub fn landings_of(&self, node: u64) -> impl Iterator<Item = &Landing> {
        self.landings.iter().filter(move |l| l.node == node)
    }

    /// The landings of a crossing stretch's two ends, `from` first.
    #[must_use]
    pub fn crossing_landings(&self, stretch: u64) -> [Option<&Landing>; 2] {
        let found = |end: LandEnd| {
            self.landings
                .iter()
                .find(|l| l.crossing == Some(stretch) && l.end == Some(end))
        };
        [found(LandEnd::From), found(LandEnd::To)]
    }

    /// The index of the class with an id.
    #[must_use]
    pub fn class_index(&self, id: &str) -> Option<usize> {
        self.classes.iter().position(|c| c.id == id)
    }
}
