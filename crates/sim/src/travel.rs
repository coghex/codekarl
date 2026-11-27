//! Travel-time arithmetic.

use num_bigint::BigInt;
use num_rational::BigRational;
use serde::Deserialize;
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BinaryHeap},
};

/// Tunable travel rules and named sample parties.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TravelRules {
    /// Climb slowing in thousandths per thousandth of rise over length.
    pub climb_thousandths: u32,
    /// Level-zero terrain slowing, in thousandths.
    pub ground: BTreeMap<String, u16>,
    /// Wade delay, thousandths of an hour.
    pub wade_thousandths: u64,
    /// Fixed ford delay, thousandths of an hour.
    pub ford_thousandths: u64,
    /// Additional ford delay per metre, thousandths of an hour.
    pub ford_per_metre_thousandths: u64,
    /// Transfer delay, thousandths of an hour.
    pub transfer_thousandths: u64,
    /// Tide delay, thousandths of an hour.
    pub tide_thousandths: u64,
    /// Named sample parties.
    pub profiles: BTreeMap<String, TravelProfile>,
}

/// Speeds and modes for a travelling party.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TravelProfile {
    /// Path speeds by level, metres per hour.
    pub path: Option<[u64; 6]>,
    /// Road speeds by level, metres per hour.
    pub road: Option<[u64; 6]>,
    /// Water class, absent when the party cannot sail.
    pub water_class: Option<String>,
    /// River speed in metres per hour.
    pub river: Option<u64>,
    /// Lough speed in metres per hour.
    pub lough: Option<u64>,
    /// Sea speed in metres per hour.
    pub sea: Option<u64>,
    /// Slowing factor in thousandths.
    pub slowing: u32,
}

impl TravelRules {
    /// Parse travel rules from TOML.
    ///
    /// # Errors
    /// Returns an error for malformed TOML or inconsistent profile capabilities.
    pub fn parse(source: &str) -> Result<Self, String> {
        let rules: Self =
            toml::from_str(source).map_err(|error: toml::de::Error| error.to_string())?;
        for (name, profile) in &rules.profiles {
            profile
                .validate()
                .map_err(|error| format!("travel profile {name}: {error}"))?;
        }
        Ok(rules)
    }
}

impl TravelProfile {
    /// Validate capabilities, speeds and slowing for a caller-supplied profile.
    ///
    /// # Errors
    /// Returns a message for incomplete modes, zero speeds or slowing below one.
    pub fn validate(&self) -> Result<(), String> {
        let land_valid = match (&self.path, &self.road) {
            (None, None) => true,
            (Some(path), Some(road)) => path.iter().chain(road).all(|speed| *speed > 0),
            _ => false,
        };
        let water_valid = match (&self.water_class, self.river, self.lough, self.sea) {
            (None, None, None, None) => true,
            (Some(class), Some(river), Some(lough), Some(sea)) => {
                !class.is_empty() && river > 0 && lough > 0 && sea > 0
            }
            _ => false,
        };
        if self.slowing < 1000 || !land_valid || !water_valid {
            return Err("invalid speeds, modes or slowing".into());
        }
        Ok(())
    }
}

/// A directed step; ground values are level-zero slowing in thousandths.
#[derive(Clone, Debug)]
pub struct LandStep {
    /// Ground length in millimetres.
    pub length_mm: BigRational,
    /// Rise in millimetres, or zero for a descent.
    pub rise_mm: u64,
    /// Starting terrain's slowing in thousandths.
    pub ground_from: u16,
    /// Ending terrain's slowing in thousandths.
    pub ground_to: u16,
}

/// A cell on a land course, with the data needed to measure its steps.
#[derive(Clone, Copy, Debug)]
pub struct LandCell {
    /// Crop-relative column.
    pub x: i32,
    /// Crop-relative row.
    pub y: i32,
    /// Ground side length at this row, in millimetres.
    pub side_mm: u64,
    /// Elevation in metres.
    pub elevation_m: i32,
    /// Level-zero terrain slowing in thousandths.
    pub ground: u16,
}

/// Measure a course in the requested direction; unsupported moves return `None`.
#[must_use]
pub fn measure_course(cells: &[LandCell], reverse: bool, climb: u32) -> Option<LandMeasure> {
    let mut steps = Vec::new();
    for pair in cells.windows(2) {
        let (from, to) = if reverse {
            (pair[1], pair[0])
        } else {
            (pair[0], pair[1])
        };
        let dx = (i64::from(from.x) - i64::from(to.x)).abs();
        let dy = (i64::from(from.y) - i64::from(to.y)).abs();
        let rise = (i64::from(to.elevation_m) - i64::from(from.elevation_m)).max(0);
        steps.push(LandStep {
            length_mm: land_step_mm(from.side_mm, to.side_mm, dx, dy)?,
            rise_mm: u64::try_from(rise).ok()? * 1000,
            ground_from: from.ground,
            ground_to: to.ground,
        });
    }
    Some(LandMeasure::from_steps(&steps, climb))
}

/// Exact length of a legal land-line step, including a two-by-one step.
#[must_use]
pub fn land_step_mm(first: u64, second: u64, dx: i64, dy: i64) -> Option<BigRational> {
    let (dx, dy) = (dx.unsigned_abs(), dy.unsigned_abs());
    if dx <= 1 && dy <= 1 && dx + dy > 0 {
        Some(cell_step_mm(first, second, dx == 1 && dy == 1))
    } else if (dx == 2 && dy == 1) || (dx == 1 && dy == 2) {
        Some(cell_step_mm(first, second, false) * BigRational::new(2236.into(), 1000.into()))
    } else {
        None
    }
}

/// Precomputed directional sums for a land course, independent of its current level and speed.
#[derive(Clone, Debug)]
pub struct LandMeasure {
    base: BigRational,
    weighted: BigRational,
}

impl LandMeasure {
    /// Measure each step once, retaining the product of climb and ground for each step.
    #[must_use]
    pub fn from_steps(steps: &[LandStep], climb_thousandths: u32) -> Self {
        let mut base = BigRational::from_integer(0.into());
        let mut weighted = BigRational::from_integer(0.into());
        for step in steps {
            let climbed = &step.length_mm + BigInt::from(step.rise_mm) * climb_thousandths;
            weighted +=
                &climbed * BigInt::from(u32::from(step.ground_from) + u32::from(step.ground_to));
            base += climbed;
        }
        Self { base, weighted }
    }

    /// Exact hours at a level from zero to five and a positive speed in metres per hour.
    ///
    /// # Panics
    /// Panics if the level exceeds five or the speed is not positive.
    #[must_use]
    pub fn hours(&self, level: u8, speed_m_per_hour: &BigRational) -> BigRational {
        assert!(level <= 5 && speed_m_per_hour > &BigRational::from_integer(0.into()));
        let scaled = &self.base * BigInt::from(10_000) + &self.weighted * BigInt::from(5 - level);
        scaled / BigInt::from(10_000_000) / speed_m_per_hour
    }
}

/// Exact length of a cell-to-cell move, in millimetres.
#[must_use]
pub fn cell_step_mm(first: u64, second: u64, diagonal: bool) -> BigRational {
    let mean = BigRational::new(BigInt::from(first) + BigInt::from(second), 2.into());
    if diagonal {
        mean * BigRational::new(1414.into(), 1000.into())
    } else {
        mean
    }
}

/// Exact half-side length for a joint-to-open-water join.
#[must_use]
pub fn half_cell_mm(side: u64) -> BigRational {
    BigRational::new(side.into(), 2.into())
}

/// Exact speed for a path or a road interpolated by its current condition.
///
/// # Panics
/// Panics if a road condition exceeds 1000.
#[must_use]
pub fn land_speed(path: u64, road: u64, condition: Option<u16>) -> BigRational {
    match condition {
        None => BigRational::from_integer(path.into()),
        Some(condition) => {
            assert!(condition <= 1000);
            BigRational::new(
                BigInt::from(path) * (1000 - condition) + BigInt::from(road) * condition,
                1000.into(),
            )
        }
    }
}

/// Apply a party's slowing to travel, leaving fixed delays unchanged.
///
/// # Panics
/// Panics if slowing is below 1000.
#[must_use]
pub fn leg_time(travel: &BigRational, slowing: u32, delays: &BigRational) -> BigRational {
    assert!(slowing >= 1000);
    travel * BigRational::new(slowing.into(), 1000.into()) + delays
}

/// A landing transfer, including its land approach, or `None` when the party cannot walk it.
///
/// # Panics
/// Panics if slowing is below 1000 or a supplied path speed is zero.
#[must_use]
pub fn transfer_hours(
    approach_m: u64,
    path_speed: Option<u64>,
    slowing: u32,
    rules: &TravelRules,
) -> Option<BigRational> {
    let walk = if approach_m == 0 {
        BigRational::from_integer(0.into())
    } else {
        let speed = path_speed?;
        assert!(speed > 0);
        BigRational::new(approach_m.into(), speed.into())
    };
    let delay = BigRational::new(rules.transfer_thousandths.into(), 1000.into());
    Some(leg_time(&walk, slowing, &delay))
}

/// Exact water travel time for a distance in millimetres and speed in metres per hour.
///
/// # Panics
/// Panics if speed is zero or slowing is below 1000.
#[must_use]
pub fn water_travel_hours(length_mm: &BigRational, speed: u64, slowing: u32) -> BigRational {
    assert!(speed > 0 && slowing >= 1000);
    length_mm * BigRational::new(slowing.into(), BigInt::from(speed) * 1_000_000)
}

/// A lake–sea move spends half its distance at each water's speed.
///
/// # Panics
/// Panics if either speed is zero or slowing is below 1000.
#[must_use]
pub fn mixed_water_hours(
    length_mm: &BigRational,
    first_speed: u64,
    second_speed: u64,
    slowing: u32,
) -> BigRational {
    let half = length_mm / BigInt::from(2);
    water_travel_hours(&half, first_speed, slowing)
        + water_travel_hours(&half, second_speed, slowing)
}

/// Strait travel with the fixed transfer at each end.
///
/// # Panics
/// Panics if speed is zero or slowing is below 1000.
#[must_use]
pub fn strait_hours(length_m: u64, speed: u64, slowing: u32, rules: &TravelRules) -> BigRational {
    let length = BigRational::from_integer(BigInt::from(length_m) * 1000);
    water_travel_hours(&length, speed, slowing)
        + BigRational::new(BigInt::from(rules.transfer_thousandths) * 2, 1000.into())
}

/// Tidal land travel with the fixed average wait.
///
/// # Panics
/// Panics if speed is zero or slowing is below 1000.
#[must_use]
pub fn tidal_hours(length_m: u64, speed: u64, slowing: u32, rules: &TravelRules) -> BigRational {
    assert!(speed > 0);
    let travel = BigRational::new(length_m.into(), speed.into());
    let delay = BigRational::new(rules.tide_thousandths.into(), 1000.into());
    leg_time(&travel, slowing, &delay)
}

/// Time for a land leg including wades and each ford, whose width is in tenths of a metre.
///
/// # Panics
/// Panics for a level above five, a nonpositive speed, or slowing below 1000.
#[must_use]
pub fn land_leg_hours(
    measure: &LandMeasure,
    level: u8,
    speed: &BigRational,
    slowing: u32,
    wades: usize,
    ford_widths_tenths: &[u64],
    rules: &TravelRules,
) -> BigRational {
    let mut delay_thousandths =
        BigRational::from_integer(BigInt::from(wades) * rules.wade_thousandths);
    for width in ford_widths_tenths {
        delay_thousandths += BigInt::from(rules.ford_thousandths);
        delay_thousandths += BigRational::new(
            BigInt::from(*width) * rules.ford_per_metre_thousandths,
            10.into(),
        );
    }
    leg_time(
        &measure.hours(level, speed),
        slowing,
        &(delay_thousandths / BigInt::from(1000)),
    )
}

/// A directed candidate leg in a travel graph.
#[derive(Clone, Debug)]
pub struct Edge {
    /// Start state.
    pub from: u64,
    /// End state.
    pub to: u64,
    /// Stable leg ordering key.
    pub id: u64,
    /// Exact nonnegative travel time.
    pub hours: BigRational,
}

/// The best journey to a state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Journey {
    /// Exact time in hours.
    pub hours: BigRational,
    /// Leg IDs in traversal order.
    pub legs: Vec<u64>,
}

/// A graph transition, which may introduce zero or more itinerary legs.
#[derive(Clone, Debug)]
pub struct ItineraryEdge {
    /// Source state.
    pub from: u64,
    /// Destination state.
    pub to: u64,
    /// Unique transition ID.
    pub id: u64,
    /// Exact transition time.
    pub hours: BigRational,
    /// Newly started legs in the fixed order of their IDs.
    pub leg_ids: Vec<u64>,
    /// A water graph step; other transitions end the current step-count tie.
    pub water_step: bool,
}

/// An itinerary and its underlying transitions.
#[derive(Clone, Debug)]
pub struct ItineraryPath {
    /// Exact journey time.
    pub hours: BigRational,
    /// Logical leg ordering keys.
    pub legs: Vec<u64>,
    /// Transition IDs in traversal order.
    pub edges: Vec<u64>,
}

/// Reusable one-pass search results.
pub struct ItinerarySearch {
    start: u64,
    best: BTreeMap<u64, (BigRational, Vec<u64>, usize, u64, u64)>,
}

impl ItinerarySearch {
    /// Read an itinerary to a reachable state.
    #[must_use]
    pub fn path(&self, end: u64) -> Option<ItineraryPath> {
        let (hours, legs, _, _, _) = self.best.get(&end)?.clone();
        let mut node = end;
        let mut edges = Vec::new();
        while node != self.start {
            let (_, _, _, previous, edge) = self.best.get(&node)?;
            edges.push(*edge);
            node = *previous;
        }
        edges.reverse();
        Some(ItineraryPath { hours, legs, edges })
    }
}

/// Find all reachable states in one pass, comparing exact times and logical legs.
///
/// # Panics
/// Panics on a negative transition time.
#[must_use]
pub fn itinerary_search(start: u64, edges: &[ItineraryEdge]) -> ItinerarySearch {
    let mut outgoing: BTreeMap<u64, Vec<&ItineraryEdge>> = BTreeMap::new();
    for edge in edges {
        assert!(edge.hours >= BigRational::from_integer(0.into()));
        outgoing.entry(edge.from).or_default().push(edge);
    }
    let initial = (
        BigRational::from_integer(0.into()),
        0_usize,
        Vec::<u64>::new(),
        0_usize,
        start,
        0_u64,
    );
    let mut pending = BTreeMap::from([(start, initial.clone())]);
    let mut queue = BinaryHeap::from([Reverse((initial, start))]);
    let mut best = BTreeMap::new();
    while let Some(Reverse((key, node))) = queue.pop() {
        if best.contains_key(&node) || pending.get(&node) != Some(&key) {
            continue;
        }
        pending.remove(&node);
        let (hours, _, legs, water_steps, previous, edge_id) = key;
        for edge in outgoing.get(&node).into_iter().flatten() {
            if !best.contains_key(&edge.to) {
                let mut next_legs = legs.clone();
                next_legs.extend_from_slice(&edge.leg_ids);
                let next_steps = if edge.water_step { water_steps + 1 } else { 0 };
                let candidate = (
                    &hours + &edge.hours,
                    next_legs.len(),
                    next_legs,
                    next_steps,
                    node,
                    edge.id,
                );
                if pending.get(&edge.to).is_none_or(|old| candidate < *old) {
                    pending.insert(edge.to, candidate.clone());
                    queue.push(Reverse((candidate, edge.to)));
                }
            }
        }
        best.insert(node, (hours, legs, water_steps, previous, edge_id));
    }
    ItinerarySearch { start, best }
}

/// Reusable results of a search for the ways into one state.
pub struct ItinerarySearchTo {
    target: u64,
    best: BTreeMap<u64, (BigRational, Vec<u64>, u64, u64)>,
}

impl ItinerarySearchTo {
    /// Read the itinerary from a state to the target, in the order it is travelled.
    #[must_use]
    pub fn path(&self, start: u64) -> Option<ItineraryPath> {
        let (hours, legs, _, _) = self.best.get(&start)?.clone();
        let mut node = start;
        let mut edges = Vec::new();
        while node != self.target {
            let (_, _, next, edge) = self.best.get(&node)?;
            edges.push(*edge);
            node = *next;
        }
        Some(ItineraryPath { hours, legs, edges })
    }
}

/// Find, for every state, a quickest itinerary into `target` in one pass over the edges turned round, in travel order.
///
/// **The fixed tie rule** (design 11b.3 rule 10: the quickest way home is by exact time, then the number of legs; among equal ways this
/// search's own rule chooses, and what it chooses is the way evaluated and the way frozen): exact time, then the number of legs, then the
/// legs' ordering keys **in the order they are travelled** (the same comparison a forward search makes, which a search that appended
/// edges to a path built from the target would make from the target back), then the number of water steps the way ends with, then the
/// state and transition it ends with. The forward search's later tie-breaks (per state, from the origin) are not reproduced: two ways
/// that tie on time and on legs may be told apart differently by a forward search from the origin, at the same time and with the same
/// legs (a detour of water steps of the same time before a landing, for one). A search that puts each edge in front of the path it leads
/// into keeps this comparison consistent (a better path into a state stays better with the same edge in front of it).
///
/// # Panics
/// Panics on a negative transition time.
#[must_use]
pub fn itinerary_search_to(target: u64, edges: &[ItineraryEdge]) -> ItinerarySearchTo {
    // A label: the forward comparison's key, whether every transition of the path is a water step (the count it ends with then grows
    // when one is put in front), and the state and transition that follow the state it labels.
    type Label = (
        (BigRational, usize, Vec<u64>, usize, u64, u64),
        bool,
        u64,
        u64,
    );
    let mut incoming: BTreeMap<u64, Vec<&ItineraryEdge>> = BTreeMap::new();
    for edge in edges {
        assert!(edge.hours >= BigRational::from_integer(0.into()));
        incoming.entry(edge.to).or_default().push(edge);
    }
    let initial: Label = (
        (
            BigRational::from_integer(0.into()),
            0,
            Vec::new(),
            0,
            target,
            0,
        ),
        true,
        target,
        0,
    );
    let mut pending = BTreeMap::from([(target, initial.clone())]);
    let mut queue = BinaryHeap::from([Reverse((initial, target))]);
    let mut best: BTreeMap<u64, (BigRational, Vec<u64>, u64, u64)> = BTreeMap::new();
    while let Some(Reverse((label, node))) = queue.pop() {
        if best.contains_key(&node) || pending.get(&node) != Some(&label) {
            continue;
        }
        pending.remove(&node);
        let ((hours, _, legs, trailing, last_from, last_edge), all_water, next, edge_id) = label;
        for edge in incoming.get(&node).into_iter().flatten() {
            if best.contains_key(&edge.from) {
                continue;
            }
            let mut front = edge.leg_ids.clone();
            front.extend_from_slice(&legs);
            let into_target = node == target;
            let candidate: Label = (
                (
                    &hours + &edge.hours,
                    front.len(),
                    front,
                    if all_water && edge.water_step {
                        trailing + 1
                    } else {
                        trailing
                    },
                    if into_target { edge.from } else { last_from },
                    if into_target { edge.id } else { last_edge },
                ),
                all_water && edge.water_step,
                node,
                edge.id,
            );
            if pending.get(&edge.from).is_none_or(|old| candidate < *old) {
                pending.insert(edge.from, candidate.clone());
                queue.push(Reverse((candidate, edge.from)));
            }
        }
        best.insert(node, (hours, legs, next, edge_id));
    }
    ItinerarySearchTo { target, best }
}

/// A measured land stretch; geometry is measured once and state is read for each query.
#[derive(Clone, Debug)]
pub struct LandWay {
    /// Stable stretch ID.
    pub id: u64,
    /// Start node.
    pub from: u64,
    /// End node.
    pub to: u64,
    /// Forward directional measure.
    pub forward: LandMeasure,
    /// Backward directional measure.
    pub backward: LandMeasure,
    /// Number of wades.
    pub wades: usize,
    /// Ford widths in tenths of a metre.
    pub fords: Vec<u64>,
    /// False if any crossing lacks a ford.
    pub land_passable: bool,
    /// Tidal distance in metres, if this is a tidal stretch.
    pub tidal_length_m: Option<u64>,
}

/// Current mutable state of a way.
#[derive(Clone, Copy, Debug)]
pub struct WayState {
    /// Road when true; path when false.
    pub road: bool,
    /// Level zero through five.
    pub level: u8,
    /// Road condition in thousandths.
    pub condition: u16,
}

/// Build directed land candidates in the supplied stable way order.
/// Edge IDs are twice the way's index, plus one for the backward direction.
///
/// # Panics
/// Panics on missing or invalid way state, invalid speeds, or too many ways for edge IDs.
#[must_use]
pub fn land_edges(
    ways: &[LandWay],
    states: &BTreeMap<u64, WayState>,
    profile: &TravelProfile,
    rules: &TravelRules,
) -> Vec<Edge> {
    let (Some(paths), Some(roads)) = (&profile.path, &profile.road) else {
        return Vec::new();
    };
    let mut edges = Vec::new();
    for (index, way) in ways.iter().enumerate() {
        if !way.land_passable {
            continue;
        }
        let state = states[&way.id];
        let level = usize::from(state.level);
        let speed = land_speed(
            paths[level],
            roads[level],
            state.road.then_some(state.condition),
        );
        let id = u64::try_from(index)
            .expect("way index fits u64")
            .checked_mul(2)
            .expect("edge ID fits u64");
        for (from, to, measure, edge_id) in [
            (way.from, way.to, &way.forward, id),
            (way.to, way.from, &way.backward, id + 1),
        ] {
            let hours = if let Some(length) = way.tidal_length_m {
                let travel = BigRational::from_integer(length.into()) / &speed;
                leg_time(
                    &travel,
                    profile.slowing,
                    &BigRational::new(rules.tide_thousandths.into(), 1000.into()),
                )
            } else {
                land_leg_hours(
                    measure,
                    state.level,
                    &speed,
                    profile.slowing,
                    way.wades,
                    &way.fords,
                    rules,
                )
            };
            edges.push(Edge {
                from,
                to,
                id: edge_id,
                hours,
            });
        }
    }
    edges
}

/// Find all reachable states in one pass, ordering ties by leg count then leg IDs.
///
/// # Panics
/// Panics if an edge has a negative time.
#[must_use]
pub fn shortest_paths(start: u64, edges: &[Edge]) -> BTreeMap<u64, Journey> {
    let mut outgoing: BTreeMap<u64, Vec<&Edge>> = BTreeMap::new();
    for edge in edges {
        assert!(edge.hours >= BigRational::from_integer(0.into()));
        outgoing.entry(edge.from).or_default().push(edge);
    }
    let mut queue = BinaryHeap::new();
    queue.push(Reverse((
        BigRational::from_integer(0.into()),
        0_usize,
        Vec::<u64>::new(),
        start,
    )));
    let mut result = BTreeMap::new();
    while let Some(Reverse((hours, _, legs, node))) = queue.pop() {
        if result.contains_key(&node) {
            continue;
        }
        for edge in outgoing.get(&node).into_iter().flatten() {
            if result.contains_key(&edge.to) {
                continue;
            }
            let mut next = legs.clone();
            next.push(edge.id);
            queue.push(Reverse((&hours + &edge.hours, next.len(), next, edge.to)));
        }
        result.insert(node, Journey { hours, legs });
    }
    result
}

/// All states reached within an inclusive whole-hour limit, comparing exact times.
///
/// # Panics
/// Panics if an edge has a negative time.
#[must_use]
pub fn within_hours(start: u64, edges: &[Edge], limit: u64) -> BTreeMap<u64, Journey> {
    let limit = BigRational::from_integer(limit.into());
    shortest_paths(start, edges)
        .into_iter()
        .filter(|(_, journey)| journey.hours <= limit)
        .collect()
}

/// Round an exact journey time up to the displayed whole hours.
#[must_use]
pub fn shown_hours(hours: &BigRational) -> BigInt {
    hours.ceil().to_integer()
}

/// Point between two map points at a proportion from zero to one.
///
/// # Panics
/// Panics if the proportion is outside zero through one.
#[must_use]
pub fn interpolate(
    from: &(BigRational, BigRational),
    to: &(BigRational, BigRational),
    proportion: &BigRational,
) -> (BigRational, BigRational) {
    assert!(
        *proportion >= BigRational::from_integer(0.into())
            && *proportion <= BigRational::from_integer(1.into())
    );
    (
        &from.0 + (&to.0 - &from.0) * proportion,
        &from.1 + (&to.1 - &from.1) * proportion,
    )
}

/// Find a water way, breaking exact ties by steps and then predecessors in node order.
///
/// Nodes must use the prescribed order: joints by ID followed by cells by row and column.
///
/// # Panics
/// Panics if an edge has a negative time.
#[must_use]
pub fn water_path(start: u64, end: u64, edges: &[Edge]) -> Option<Journey> {
    water_search(start, edges).path(end)
}

/// A single-source water search, reusable for every destination in its body.
pub struct WaterSearch {
    start: u64,
    best: BTreeMap<u64, (BigRational, usize)>,
    previous: BTreeMap<u64, (u64, u64)>,
}

/// Compute exact water times and step counts once for one source.
///
/// # Panics
/// Panics if an edge has a negative time.
#[must_use]
pub fn water_search(start: u64, edges: &[Edge]) -> WaterSearch {
    let mut outgoing: BTreeMap<u64, Vec<&Edge>> = BTreeMap::new();
    for edge in edges {
        assert!(edge.hours >= BigRational::from_integer(0.into()));
        outgoing.entry(edge.from).or_default().push(edge);
    }
    let mut queue = BinaryHeap::new();
    queue.push(Reverse((
        BigRational::from_integer(0.into()),
        0_usize,
        start,
    )));
    let mut best = BTreeMap::new();
    while let Some(Reverse((time, steps, node))) = queue.pop() {
        if best.contains_key(&node) {
            continue;
        }
        for edge in outgoing.get(&node).into_iter().flatten() {
            if !best.contains_key(&edge.to) {
                queue.push(Reverse((&time + &edge.hours, steps + 1, edge.to)));
            }
        }
        best.insert(node, (time, steps));
    }
    let mut previous: BTreeMap<u64, (u64, u64)> = BTreeMap::new();
    for edge in edges {
        if let (Some((before, before_steps)), Some((after, after_steps))) =
            (best.get(&edge.from), best.get(&edge.to))
            && before_steps + 1 == *after_steps
            && before + &edge.hours == *after
        {
            let candidate = (edge.from, edge.id);
            previous
                .entry(edge.to)
                .and_modify(|old| *old = (*old).min(candidate))
                .or_insert(candidate);
        }
    }
    WaterSearch {
        start,
        best,
        previous,
    }
}

impl WaterSearch {
    /// Read the selected path back by the earliest predecessor one step nearer the source.
    #[must_use]
    pub fn path(&self, end: u64) -> Option<Journey> {
        let (hours, _) = self.best.get(&end)?.clone();
        let mut node = end;
        let mut legs = Vec::new();
        while node != self.start {
            let &(from, edge_id) = self.previous.get(&node)?;
            legs.push(edge_id);
            node = from;
        }
        legs.reverse();
        Some(Journey { hours, legs })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fraction(numerator: i64, denominator: i64) -> BigRational {
        BigRational::new(numerator.into(), denominator.into())
    }

    #[test]
    fn corner_steps_keep_exact_time_when_the_course_is_cut() {
        let step = LandStep {
            length_mm: fraction(671_650, 1),
            rise_mm: 0,
            ground_from: 0,
            ground_to: 0,
        };
        let one = LandMeasure::from_steps(&[step.clone(), step.clone()], 20);
        let split = LandMeasure::from_steps(&[step], 20);
        let expected = fraction(13_433, 12_000);
        for level in 0..=5 {
            assert_eq!(one.hours(level, &fraction(1_200, 1)), expected);
            assert_eq!(
                split.hours(level, &fraction(1_200, 1)) * BigInt::from(2),
                expected
            );
        }
    }

    #[test]
    fn ground_and_climb_multiply_per_step_and_cutting_preserves_time() {
        let flat_bog = LandStep {
            length_mm: fraction(500_000, 1),
            rise_mm: 0,
            ground_from: 600,
            ground_to: 600,
        };
        let uphill_pasture = LandStep {
            length_mm: fraction(500_000, 1),
            rise_mm: 25_000,
            ground_from: 0,
            ground_to: 0,
        };
        let whole = LandMeasure::from_steps(&[flat_bog.clone(), uphill_pasture.clone()], 20);
        for level in 0..=5 {
            let pieces = LandMeasure::from_steps(std::slice::from_ref(&flat_bog), 20)
                .hours(level, &fraction(1_000, 1))
                + LandMeasure::from_steps(std::slice::from_ref(&uphill_pasture), 20)
                    .hours(level, &fraction(1_000, 1));
            assert_eq!(whole.hours(level, &fraction(1_000, 1)), pieces);
        }
        assert_eq!(whole.hours(0, &fraction(1_000, 1)), fraction(9, 5));
        assert_eq!(whole.hours(5, &fraction(1_000, 1)), fraction(3, 2));
    }

    #[test]
    fn road_condition_interpolates_without_rounding() {
        assert_eq!(land_speed(600, 1_201, None), fraction(600, 1));
        assert_eq!(land_speed(600, 1_201, Some(0)), fraction(600, 1));
        assert_eq!(land_speed(600, 1_201, Some(500)), fraction(1_801, 2));
        assert_eq!(land_speed(600, 1_201, Some(1000)), fraction(1_201, 1));
    }

    #[test]
    fn slowing_changes_travel_but_not_crossing_delays() {
        let travel = fraction(1, 2);
        let delays = fraction(1, 4) + fraction(1, 2) + fraction(3, 10);
        assert_eq!(leg_time(&travel, 1_500, &delays), fraction(9, 5));
    }

    #[test]
    fn routes_compare_exact_time_then_leg_count_then_ids() {
        let edges = vec![
            Edge {
                from: 0,
                to: 1,
                id: 8,
                hours: fraction(1, 1),
            },
            Edge {
                from: 0,
                to: 1,
                id: 2,
                hours: fraction(1, 1),
            },
            Edge {
                from: 1,
                to: 2,
                id: 3,
                hours: fraction(1, 1999),
            },
            Edge {
                from: 0,
                to: 2,
                id: 5,
                hours: fraction(1001, 1000),
            },
        ];
        let paths = shortest_paths(0, &edges);
        assert_eq!(paths[&2].legs, vec![2, 3]);
        assert!(paths[&2].hours > fraction(1, 1));
        assert!(paths[&2].hours < fraction(1001, 1000));
        assert!(!paths.contains_key(&3));
    }

    #[test]
    fn data_supplies_the_specified_sample_profiles_and_delays() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/travel.toml");
        let rules = TravelRules::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            rules.profiles["cart"].path,
            Some([600, 1000, 1150, 1300, 1450, 1600])
        );
        assert_eq!(
            rules.profiles["currach"].water_class.as_deref(),
            Some("currach")
        );
        assert_eq!(rules.transfer_thousandths, 2000);
        assert_eq!(rules.climb_thousandths, 20);
        assert_eq!(rules.ford_per_metre_thousandths, 10);
    }

    #[test]
    fn cell_lengths_are_exact_even_for_odd_sides() {
        assert_eq!(cell_step_mm(475_000, 475_000, true), fraction(671_650, 1));
        assert_eq!(cell_step_mm(1, 2, false), fraction(3, 2));
        assert_eq!(half_cell_mm(475_001), fraction(475_001, 2));
    }

    #[test]
    fn water_ties_use_fewest_steps_then_earliest_predecessor() {
        let edges = vec![
            Edge {
                from: 3,
                to: 2,
                id: 1,
                hours: fraction(1, 1),
            },
            Edge {
                from: 2,
                to: 1,
                id: 2,
                hours: fraction(0, 1),
            },
            Edge {
                from: 1,
                to: 2,
                id: 3,
                hours: fraction(0, 1),
            },
            Edge {
                from: 1,
                to: 4,
                id: 4,
                hours: fraction(1, 1),
            },
            Edge {
                from: 2,
                to: 4,
                id: 5,
                hours: fraction(1, 1),
            },
        ];
        assert_eq!(water_path(3, 2, &edges).unwrap().legs, vec![1]);
        assert_eq!(water_path(3, 4, &edges).unwrap().legs, vec![1, 5]);
        let equal = vec![
            Edge {
                from: 0,
                to: 2,
                id: 1,
                hours: fraction(1, 1),
            },
            Edge {
                from: 0,
                to: 1,
                id: 2,
                hours: fraction(1, 1),
            },
            Edge {
                from: 1,
                to: 4,
                id: 3,
                hours: fraction(1, 1),
            },
            Edge {
                from: 2,
                to: 4,
                id: 4,
                hours: fraction(1, 1),
            },
        ];
        assert_eq!(water_path(0, 4, &equal).unwrap().legs, vec![2, 3]);
    }

    #[test]
    fn equal_ways_around_an_unusable_lake_cell_choose_the_earlier_cells() {
        // Row-major IDs for a three-by-three lake; the middle cell is unusable,
        // so its adjacent corners cannot be cut.
        let pairs = [
            (3, 0),
            (0, 1),
            (1, 2),
            (2, 5),
            (3, 6),
            (6, 7),
            (7, 8),
            (8, 5),
        ];
        let mut edges = Vec::new();
        for (id, (from, to)) in pairs.into_iter().enumerate() {
            edges.push(Edge {
                from,
                to,
                id: u64::try_from(id).unwrap(),
                hours: fraction(1, 3),
            });
            edges.push(Edge {
                from: to,
                to: from,
                id: u64::try_from(id + 8).unwrap(),
                hours: fraction(1, 3),
            });
        }
        for _ in 0..5 {
            let way = water_path(3, 5, &edges).unwrap();
            assert_eq!(way.legs, [0, 1, 2, 3]);
            assert_eq!(way.hours, fraction(4, 3));
        }
    }

    #[test]
    fn lake_reaches_and_the_open_cell_join_each_take_their_distance() {
        let reach = water_travel_hours(&fraction(500_000, 1), 1500, 1000);
        let join = water_travel_hours(&half_cell_mm(500_000), 1500, 1000);
        let edges = vec![
            Edge {
                from: 0,
                to: 1,
                id: 0,
                hours: reach.clone(),
            },
            Edge {
                from: 1,
                to: 2,
                id: 1,
                hours: reach.clone(),
            },
            Edge {
                from: 1,
                to: 10,
                id: 2,
                hours: join.clone(),
            },
        ];
        let search = water_search(0, &edges);
        assert_eq!(search.path(2).unwrap().hours, fraction(2, 3));
        assert_eq!(search.path(10).unwrap().hours, fraction(1, 2));
    }

    #[test]
    fn limit_compares_before_rounding_and_includes_its_boundary() {
        let edges = vec![
            Edge {
                from: 0,
                to: 1,
                id: 1,
                hours: fraction(1, 1),
            },
            Edge {
                from: 1,
                to: 2,
                id: 2,
                hours: fraction(1, 1999),
            },
        ];
        let within = within_hours(0, &edges, 1);
        assert!(within.contains_key(&0));
        assert!(within.contains_key(&1));
        assert!(!within.contains_key(&2));
        assert_eq!(shown_hours(&fraction(2000, 1999)), BigInt::from(2));
    }

    #[test]
    fn land_crossings_add_each_fixed_delay_without_slowing_it() {
        let rules_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/travel.toml");
        let rules = TravelRules::parse(&std::fs::read_to_string(rules_path).unwrap()).unwrap();
        let step = LandStep {
            length_mm: fraction(1_000_000, 1),
            rise_mm: 0,
            ground_from: 0,
            ground_to: 0,
        };
        let measure = LandMeasure::from_steps(&[step], 20);
        let result = land_leg_hours(
            &measure,
            0,
            &fraction(1_000, 1),
            1_500,
            1,
            &[100, 200],
            &rules,
        );
        // 1.5 h travel + 0.25 h wade + (0.5 + 0.1) + (0.5 + 0.2) h for the fords.
        assert_eq!(result, fraction(61, 20));
    }

    #[test]
    fn terrain_and_uphill_multiply_and_level_five_removes_ground() {
        for ground in [0_u16, 200, 400, 500, 600] {
            let uphill = LandStep {
                length_mm: fraction(1_000_000, 1),
                rise_mm: 50_000,
                ground_from: ground,
                ground_to: ground,
            };
            let downhill = LandStep {
                rise_mm: 0,
                ..uphill.clone()
            };
            let up = LandMeasure::from_steps(&[uphill], 20);
            let down = LandMeasure::from_steps(&[downhill], 20);
            assert_eq!(
                up.hours(0, &fraction(1000, 1)),
                fraction(2 * (1000 + i64::from(ground)), 1000)
            );
            assert_eq!(
                down.hours(0, &fraction(1000, 1)),
                fraction(1000 + i64::from(ground), 1000)
            );
            assert_eq!(up.hours(5, &fraction(1000, 1)), fraction(2, 1));
            assert_eq!(down.hours(5, &fraction(1000, 1)), fraction(1, 1));
        }
    }

    #[test]
    fn paths_and_each_road_condition_match_literals_on_every_terrain_and_direction() {
        for (ground, factor_numerator) in [(0, 10), (200, 12), (400, 14), (500, 15), (600, 16)] {
            for rise in [0, 50_000] {
                let measure = LandMeasure::from_steps(
                    &[LandStep {
                        length_mm: fraction(1_000_000, 1),
                        rise_mm: rise,
                        ground_from: ground,
                        ground_to: ground,
                    }],
                    20,
                );
                for (level, path, road) in [(0, 600_u64, 1200), (5, 1600, 2200)] {
                    let factor = if level == 0 { factor_numerator } else { 10 };
                    let climb = if rise == 0 { 1 } else { 2 };
                    for (condition, expected_speed) in [
                        (None, path),
                        (Some(1000), road),
                        (Some(500), path.midpoint(road)),
                        (Some(0), path),
                    ] {
                        let speed = land_speed(path, road, condition);
                        let expected = fraction(
                            i64::from(factor * climb) * 100,
                            i64::try_from(expected_speed).unwrap(),
                        );
                        assert_eq!(measure.hours(level, &speed), expected);
                    }
                }
            }
        }
    }

    #[test]
    fn measured_course_reads_steps_in_direction_of_travel() {
        let samples = [
            LandCell {
                x: 0,
                y: 0,
                side_mm: 475_000,
                elevation_m: 10,
                ground: 600,
            },
            LandCell {
                x: 1,
                y: 1,
                side_mm: 475_000,
                elevation_m: 10,
                ground: 600,
            },
            LandCell {
                x: 2,
                y: 2,
                side_mm: 475_000,
                elevation_m: 20,
                ground: 0,
            },
        ];
        let forward = measure_course(&samples, false, 20).unwrap();
        let backward = measure_course(&samples, true, 20).unwrap();
        assert!(forward.hours(0, &fraction(1200, 1)) > backward.hours(0, &fraction(1200, 1)));
        let first = measure_course(&samples[..2], false, 20)
            .unwrap()
            .hours(0, &fraction(1200, 1));
        let second = measure_course(&samples[1..], false, 20)
            .unwrap()
            .hours(0, &fraction(1200, 1));
        assert_eq!(forward.hours(0, &fraction(1200, 1)), first + second);
    }

    #[test]
    fn long_steps_use_endpoints_and_preserve_a_cut_course() {
        let cells = [
            LandCell {
                x: 100,
                y: 1,
                side_mm: 475_000,
                elevation_m: 0,
                ground: 0,
            },
            LandCell {
                x: 102,
                y: 2,
                side_mm: 475_000,
                elevation_m: 0,
                ground: 0,
            },
            LandCell {
                x: 103,
                y: 2,
                side_mm: 475_000,
                elevation_m: 0,
                ground: 0,
            },
        ];
        let whole = measure_course(&cells, false, 20)
            .unwrap()
            .hours(0, &fraction(1200, 1));
        let first = measure_course(&cells[..2], false, 20)
            .unwrap()
            .hours(0, &fraction(1200, 1));
        let second = measure_course(&cells[1..], false, 20)
            .unwrap()
            .hours(0, &fraction(1200, 1));
        assert_eq!(first, fraction(10621, 12000));
        assert_eq!(whole, fraction(15371, 12000));
        assert_eq!(whole, first + second);
        let changed = [
            LandCell {
                ground: 600,
                ..cells[0]
            },
            LandCell {
                elevation_m: 50,
                ..cells[1]
            },
        ];
        let expected = (fraction(1_062_100, 1) + fraction(1_000_000, 1)) * fraction(13, 10)
            / BigInt::from(1_200_000);
        assert_eq!(
            measure_course(&changed, false, 20)
                .unwrap()
                .hours(0, &fraction(1200, 1)),
            expected
        );
        let other_orientation = [
            cells[0],
            LandCell {
                x: 101,
                y: 3,
                ..cells[1]
            },
        ];
        assert_eq!(
            measure_course(&other_orientation, false, 20)
                .unwrap()
                .hours(0, &fraction(1200, 1)),
            fraction(10621, 12000)
        );
        let unequal = [
            cells[0],
            LandCell {
                side_mm: 475_001,
                ..cells[1]
            },
        ];
        let expected_mm = fraction(950_001, 2) * fraction(2236, 1000);
        assert_eq!(
            measure_course(&unequal, false, 20)
                .unwrap()
                .hours(0, &fraction(1200, 1)),
            expected_mm / BigInt::from(1_200_000)
        );
    }

    #[test]
    fn transfer_allows_a_boat_only_when_no_walk_is_needed() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/travel.toml");
        let rules = TravelRules::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(transfer_hours(0, None, 1500, &rules), Some(fraction(2, 1)));
        assert_eq!(transfer_hours(600, None, 1500, &rules), None);
        assert_eq!(
            transfer_hours(600, Some(600), 1500, &rules),
            Some(fraction(7, 2))
        );
    }

    #[test]
    fn water_lengths_keep_exact_time() {
        assert_eq!(
            water_travel_hours(&fraction(1_062_100, 1), 1_700, 1_500),
            fraction(15_931_500, 17_000_000)
        );
        assert_eq!(
            water_travel_hours(&half_cell_mm(475_001), 1_200, 1_000),
            fraction(475_001, 2_400_000)
        );
    }

    #[test]
    fn mixed_lake_sea_move_spends_half_at_each_speed() {
        let length = fraction(500_000, 1);
        assert_eq!(mixed_water_hours(&length, 1500, 1200, 1000), fraction(3, 8));
        assert_eq!(mixed_water_hours(&length, 1200, 1500, 1000), fraction(3, 8));
    }

    #[test]
    fn strait_and_tide_add_delays_after_slowing() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/travel.toml");
        let rules = TravelRules::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(strait_hours(2_000, 2_000, 1_500, &rules), fraction(11, 2));
        assert_eq!(tidal_hours(600, 600, 1_500, &rules), fraction(15, 2));
    }

    #[test]
    fn invalid_profiles_are_rejected_before_routing() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/travel.toml");
        let source = std::fs::read_to_string(path).unwrap();
        assert!(TravelRules::parse(&source.replace("slowing = 1000", "slowing = 999")).is_err());
        assert!(TravelRules::parse(&source.replace("river = 1500", "river = 0")).is_err());
        assert!(TravelRules::parse(&source.replace("sea = 1200\n", "")).is_err());
    }

    #[test]
    fn positions_on_a_join_and_approach_follow_the_line_both_ways() {
        let a = (fraction(1, 1), fraction(2, 1));
        let b = (fraction(3, 1), fraction(4, 1));
        assert_eq!(interpolate(&a, &b, &fraction(0, 1)), a);
        assert_eq!(
            interpolate(&a, &b, &fraction(1, 2)),
            (fraction(2, 1), fraction(3, 1))
        );
        assert_eq!(interpolate(&a, &b, &fraction(1, 1)), b);
        assert_eq!(
            interpolate(&b, &a, &fraction(1, 2)),
            (fraction(2, 1), fraction(3, 1))
        );
    }

    #[test]
    fn current_road_condition_changes_the_next_answer() {
        let rules_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/travel.toml");
        let rules = TravelRules::parse(&std::fs::read_to_string(rules_path).unwrap()).unwrap();
        let step = LandStep {
            length_mm: fraction(600_000, 1),
            rise_mm: 0,
            ground_from: 0,
            ground_to: 0,
        };
        let measure = LandMeasure::from_steps(&[step], 20);
        let way = LandWay {
            id: 1,
            from: 1,
            to: 2,
            forward: measure.clone(),
            backward: measure,
            wades: 0,
            fords: vec![],
            land_passable: true,
            tidal_length_m: None,
        };
        let profile = &rules.profiles["cart"];
        let mut states = BTreeMap::from([(
            1,
            WayState {
                road: true,
                level: 0,
                condition: 1000,
            },
        )]);
        let first = land_edges(std::slice::from_ref(&way), &states, profile, &rules);
        assert_eq!(shortest_paths(1, &first)[&2].hours, fraction(1, 2));
        states.get_mut(&1).unwrap().condition = 0;
        let ruined = land_edges(std::slice::from_ref(&way), &states, profile, &rules);
        assert_eq!(shortest_paths(1, &ruined)[&2].hours, fraction(1, 1));
        states.get_mut(&1).unwrap().road = false;
        assert_eq!(
            land_edges(&[way], &states, profile, &rules)[0].hours,
            fraction(1, 1)
        );
    }

    #[test]
    fn itinerary_search_counts_a_whole_water_way_as_one_leg() {
        let edge = |from, to, id, time, legs: Vec<u64>, water| ItineraryEdge {
            from,
            to,
            id,
            hours: time,
            leg_ids: legs,
            water_step: water,
        };
        let edges = vec![
            edge(0, 1, 0, fraction(1, 1), vec![10], false),
            edge(0, 2, 1, fraction(0, 1), vec![20, 21], false),
            edge(2, 3, 2, fraction(1, 2), vec![], true),
            edge(3, 4, 3, fraction(1, 2), vec![], true),
            edge(4, 1, 4, fraction(0, 1), vec![22], false),
        ];
        let paths = itinerary_search(0, &edges);
        assert_eq!(paths.path(1).unwrap().legs, vec![10]);
        assert_eq!(paths.path(4).unwrap().legs, vec![20, 21]);
        assert_eq!(paths.path(4).unwrap().edges, vec![1, 2, 3]);
    }

    #[test]
    fn itinerary_water_ties_do_not_loop_on_zero_time_joins() {
        let edge = |from, to, id, hours| ItineraryEdge {
            from,
            to,
            id,
            hours,
            leg_ids: vec![],
            water_step: true,
        };
        let edges = vec![
            edge(3, 2, 0, fraction(1, 1)),
            edge(2, 1, 1, fraction(0, 1)),
            edge(1, 2, 2, fraction(0, 1)),
            edge(1, 4, 3, fraction(1, 1)),
            edge(2, 4, 4, fraction(1, 1)),
        ];
        let paths = itinerary_search(3, &edges);
        assert_eq!(paths.path(2).unwrap().edges, [0]);
        assert_eq!(paths.path(4).unwrap().edges, [0, 4]);
        let equal = vec![
            edge(0, 2, 0, fraction(1, 1)),
            edge(0, 1, 1, fraction(1, 1)),
            edge(1, 4, 2, fraction(1, 1)),
            edge(2, 4, 3, fraction(1, 1)),
        ];
        assert_eq!(itinerary_search(0, &equal).path(4).unwrap().edges, [1, 2]);
    }

    #[test]
    fn itinerary_compares_exact_times_before_display_and_orders_equal_legs() {
        let edge = |to, id, hours, leg| ItineraryEdge {
            from: 0,
            to,
            id,
            hours,
            leg_ids: vec![leg],
            water_step: false,
        };
        let paths = itinerary_search(
            0,
            &[
                edge(1, 0, fraction(1001, 1000), 1),
                edge(1, 1, fraction(2000, 1999), 9),
                edge(2, 2, fraction(1, 1), 5),
                edge(2, 3, fraction(1, 1), 2),
            ],
        );
        assert_eq!(paths.path(1).unwrap().edges, [1]);
        assert_eq!(shown_hours(&paths.path(1).unwrap().hours), BigInt::from(2));
        assert_eq!(paths.path(2).unwrap().edges, [3]);
    }

    fn plain_edge(
        from: u64,
        to: u64,
        id: u64,
        hours: i64,
        legs: &[u64],
        water: bool,
    ) -> ItineraryEdge {
        ItineraryEdge {
            from,
            to,
            id,
            hours: fraction(hours, 1),
            leg_ids: legs.to_vec(),
            water_step: water,
        }
    }

    #[test]
    fn the_search_into_a_state_breaks_equal_ways_in_the_order_they_are_travelled() {
        // Two ways from 0 to 3 of equal time and equal length, whose legs are [1, 9] and [2, 3] in the order travelled. The forward search
        // takes the first; a search from 3 over the edges turned round would compare [9, 1] with [3, 2] and take the second.
        let edges = [
            plain_edge(0, 1, 10, 1, &[1], false),
            plain_edge(1, 3, 11, 1, &[9], false),
            plain_edge(0, 2, 12, 1, &[2], false),
            plain_edge(2, 3, 13, 1, &[3], false),
        ];
        let forward = itinerary_search(0, &edges).path(3).unwrap();
        assert_eq!(
            (forward.legs.clone(), forward.edges.clone()),
            (vec![1, 9], vec![10, 11])
        );
        let into = itinerary_search_to(3, &edges).path(0).unwrap();
        assert_eq!(
            (into.legs, into.edges, into.hours),
            (forward.legs, forward.edges, forward.hours)
        );
    }

    #[test]
    fn the_search_into_a_state_gives_each_state_the_time_and_the_legs_a_forward_search_from_it_gives()
     {
        // Small graphs with many ties (hours of 0 to 2, a few leg keys, some water steps), each state read against its own forward search:
        // the same exact time and the same legs in the order travelled (the edges between may differ among ways of equal time and legs).
        let mut seed = 0x2545_f491_u64;
        let mut roll = move |below: u64| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) % below
        };
        for _ in 0..60 {
            let nodes = 6 + roll(3);
            let mut edges = Vec::new();
            for id in 0..(nodes * 3) {
                let (from, to) = (roll(nodes), roll(nodes));
                if from == to {
                    continue;
                }
                let legs: Vec<u64> = (0..roll(3)).map(|_| roll(12)).collect();
                // A transition that starts no leg and takes no time never ties two ways in the game.
                let hours = i64::try_from(roll(3)).unwrap() + i64::from(legs.is_empty());
                edges.push(plain_edge(from, to, id, hours, &legs, roll(2) == 0));
            }
            let target = roll(nodes);
            let into = itinerary_search_to(target, &edges);
            for start in 0..nodes {
                let forward = itinerary_search(start, &edges).path(target);
                let reverse = into.path(start);
                assert_eq!(
                    forward.as_ref().map(|path| (&path.hours, &path.legs)),
                    reverse.as_ref().map(|path| (&path.hours, &path.legs)),
                    "from {start} to {target} over {edges:?}"
                );
            }
        }
    }

    #[test]
    fn equal_time_water_ways_into_a_landing_have_the_forward_time_and_legs_and_the_reverse_tie_rule_is_fixed()
     {
        // One water step of two hours, or two of one hour each, into the same landing (the shape the application builds: water steps
        // start no leg, the landing starts one). A forward search keeps the direct step; the search into the landing has its own fixed
        // rule among such ways, and either way is two hours and the landing's leg. What it chooses is the same every time.
        let edges = [
            plain_edge(0, 2, 0, 2, &[], true),
            plain_edge(0, 1, 1, 1, &[], true),
            plain_edge(1, 2, 2, 1, &[], true),
            plain_edge(2, 3, 3, 0, &[7], false),
        ];
        let forward = itinerary_search(0, &edges).path(3).unwrap();
        assert_eq!(forward.edges, [0, 3]);
        let first = itinerary_search_to(3, &edges).path(0).unwrap();
        let again = itinerary_search_to(3, &edges).path(0).unwrap();
        assert_eq!((&first.hours, &first.legs), (&forward.hours, &forward.legs));
        assert_eq!(first.hours, fraction(2, 1));
        assert!(
            first.edges == [0, 3] || first.edges == [1, 2, 3],
            "{:?}",
            first.edges
        );
        assert_eq!(first.edges, again.edges, "the rule is fixed");
    }
}
