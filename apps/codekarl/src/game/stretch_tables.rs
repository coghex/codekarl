//! Exact tables for placing a point along a leg of a way (CK-455, fix A).
//!
//! The clipper of a trader's way asks for the point at a distance along a leg tens of thousands of times for each way (a sample every
//! 100 m, then a bisection to the millimetre), and [`Routes::position`](super::travel::Routes::position) answers each by walking the
//! leg from its first step, taking the exact length of every step (a rational and a `gcd` for each), and for a land leg finding the
//! stretch by a scan of every stretch of the network. The values it works from do not depend on the distance asked for, so this module
//! computes them once: the cell centres, the exact length of each step and the running sums of them. A point is then found by a binary
//! search of the sums and one interpolation, **with the very exact arithmetic the walk ends in**: the walk stops at the first step
//! whose running sum reaches the distance and interpolates there by the distance left, and the table finds that same step and the same
//! distance left (`distance - sum before`), so every point, and every breakpoint, is exactly the one the walk gives.
//!
//! Every length of a step is a whole number of micrometres (the cell sides are whole millimetres and the factors of a diagonal and of a
//! knight's move are thousandths), and so is every distance the clipper samples along a leg. The tables keep the sums as whole
//! micrometres in `i128`, and a distance given as a rational is used in that form only when it is exactly a whole number of
//! micrometres; any other distance, any length that is not, and any step the tables do not model are answered by the rational walk of
//! `Routes::position`. Nothing here rounds.

use std::{
    cell::OnceCell,
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

use ck_map::ReachLine;
use num_bigint::BigInt;
use num_rational::BigRational;

use super::travel::{Routes, TripKind, TripLeg, WaterStep, centre, land_step_length};

/// A point of the map in fixed-point cell units.
pub(crate) type Point = (BigRational, BigRational);

/// Micrometres in a millimetre.
const UM: i128 = 1000;

fn zero() -> BigRational {
    BigRational::from_integer(BigInt::from(0))
}

/// `along / length`, or 0 for a step of no length (the walk's own proportion).
fn proportion(length: &BigRational, along: &BigRational) -> BigRational {
    if *length == zero() {
        zero()
    } else {
        along / length
    }
}

/// A distance in millimetres as whole micrometres, when it is exactly that.
pub(crate) fn to_um(distance_mm: &BigRational) -> Option<i128> {
    let denom = i128::try_from(distance_mm.denom()).ok()?;
    if denom <= 0 || UM % denom != 0 {
        return None;
    }
    i128::try_from(distance_mm.numer())
        .ok()?
        .checked_mul(UM / denom)
}

/// A whole number of micrometres as millimetres.
pub(crate) fn from_um(micrometres: i128) -> BigRational {
    BigRational::new(BigInt::from(micrometres), BigInt::from(UM))
}

/// A point of a straight step: `a + (b - a) * left / length`, exactly, as one fraction in each coordinate.
fn along_step(a: (i128, i128), b: (i128, i128), left: i128, length: i128) -> Option<Point> {
    if length == 0 {
        return Some((
            BigRational::from_integer(BigInt::from(a.0)),
            BigRational::from_integer(BigInt::from(a.1)),
        ));
    }
    let numerator = |from: i128, to: i128| -> Option<i128> {
        from.checked_mul(length)?
            .checked_add((to - from).checked_mul(left)?)
    };
    Some((
        BigRational::new(BigInt::from(numerator(a.0, b.0)?), BigInt::from(length)),
        BigRational::new(BigInt::from(numerator(a.1, b.1)?), BigInt::from(length)),
    ))
}

fn integer_point(point: &Point) -> Option<(i128, i128)> {
    if !point.0.is_integer() || !point.1.is_integer() {
        return None;
    }
    Some((
        i128::try_from(point.0.to_integer()).ok()?,
        i128::try_from(point.1.to_integer()).ok()?,
    ))
}

/// What a land stretch gives the clipper, in the order it is travelled.
pub(crate) struct LandTable {
    tidal: bool,
    /// The cell centres, in travel order.
    centres: Vec<Point>,
    /// The exact length of each step, in millimetres, up to the first step that has no legal length.
    steps: Vec<BigRational>,
    /// The running sums of `steps`: `sums[k]` is the distance from the start to the end of step `k`.
    sums: Vec<BigRational>,
    /// Whether a step with no legal length follows the ones in `steps` (a point beyond them cannot be placed).
    broken: bool,
    /// The same in whole micrometres and whole map units, when every length is a whole number of micrometres and every centre a whole
    /// point.
    whole: Option<LandWhole>,
}

struct LandWhole {
    centres: Vec<(i128, i128)>,
    steps_um: Vec<i128>,
    sums_um: Arc<[i128]>,
}

impl LandTable {
    fn build(scenario: &ck_map::Scenario, way_id: u64, backward: bool) -> Option<Self> {
        let network = scenario.network.as_ref()?;
        let stretch = network
            .stretches
            .iter()
            .find(|stretch| stretch.id == way_id)?;
        let mut cells = stretch.cells.clone();
        if backward {
            cells.reverse();
        }
        let centres: Vec<Point> = cells.iter().map(|cell| centre(scenario, *cell)).collect();
        let (mut steps, mut sums) = (Vec::new(), Vec::new());
        let mut total = zero();
        let mut broken = false;
        for pair in cells.windows(2) {
            let Some(length) = land_step_length(scenario, pair[0], pair[1]) else {
                broken = true;
                break;
            };
            total += &length;
            steps.push(length);
            sums.push(total.clone());
        }
        let whole = (|| {
            let steps_um: Vec<i128> = steps.iter().map(to_um).collect::<Option<_>>()?;
            let sums_um: Vec<i128> = sums.iter().map(to_um).collect::<Option<_>>()?;
            let centres: Vec<(i128, i128)> =
                centres.iter().map(integer_point).collect::<Option<_>>()?;
            Some(LandWhole {
                centres,
                steps_um,
                sums_um: sums_um.into(),
            })
        })();
        Some(Self {
            tidal: stretch.tidal,
            centres,
            steps,
            sums,
            broken,
            whole,
        })
    }

    /// The point at `distance_mm` along the leg, which the walk of `Routes::position` gives.
    pub(crate) fn position(&self, leg: &TripLeg, distance_mm: &BigRational) -> Option<Point> {
        if *distance_mm < zero() {
            return None;
        }
        if !self.tidal
            && let Some(whole) = &self.whole
            && let (Some(along), Some(length)) = (to_um(distance_mm), to_um(&leg.length_mm))
        {
            let distance = along.min(length);
            let step = whole.sums_um.partition_point(|sum| *sum < distance);
            if step < whole.sums_um.len() {
                let before = if step == 0 {
                    0
                } else {
                    whole.sums_um[step - 1]
                };
                if let Some(point) = along_step(
                    whole.centres[step],
                    whole.centres[step + 1],
                    distance - before,
                    whole.steps_um[step],
                ) {
                    return Some(point);
                }
            }
        }
        self.position_rational(leg, distance_mm)
    }

    /// The same point by the rational walk's own arithmetic over the table.
    fn position_rational(&self, leg: &TripLeg, distance_mm: &BigRational) -> Option<Point> {
        let distance = distance_mm.min(&leg.length_mm).clone();
        if self.tidal {
            return Some(ck_sim::travel::interpolate(
                self.centres.first()?,
                self.centres.last()?,
                &proportion(&leg.length_mm, &distance),
            ));
        }
        // The first step whose running sum reaches the distance (the walk's `left <= length`).
        let step = self.sums.partition_point(|sum| *sum < distance);
        if step < self.sums.len() {
            let before = if step == 0 {
                zero()
            } else {
                self.sums[step - 1].clone()
            };
            let left = &distance - before;
            return Some(ck_sim::travel::interpolate(
                &self.centres[step],
                &self.centres[step + 1],
                &proportion(&self.steps[step], &left),
            ));
        }
        if self.broken {
            None
        } else {
            self.centres.last().cloned()
        }
    }

    /// Whether the table's stretch is a land one of whole steps (not tidal), whose path moves along its stretch by the exact lengths of
    /// the steps between the cells.
    pub(crate) fn is_whole_land(&self) -> bool {
        !self.tidal && !self.broken && self.whole.is_some()
    }

    /// The same distances as whole micrometres, when they are.
    pub(crate) fn breakpoints_um(&self) -> Option<Arc<[i128]>> {
        if self.broken {
            return Some(Arc::from(Vec::new()));
        }
        self.whole.as_ref().map(|whole| Arc::clone(&whole.sums_um))
    }

    /// The distances along the leg where its path bends, which `Routes::breakpoints` gives for a land leg: empty if any step has no
    /// legal length.
    pub(crate) fn breakpoints(&self) -> Vec<BigRational> {
        if self.broken {
            Vec::new()
        } else {
            self.sums.clone()
        }
    }
}

/// A box of whole map units, `[x least, y least, x greatest, y greatest]`.
pub(crate) type Bbox = [i128; 4];

/// What a water leg gives the clipper: its steps' lengths and running sums, in whole micrometres, and the box that holds every point of
/// each step.
pub(crate) struct WaterTable {
    /// The index in the search's water steps of each piece of the leg, in order.
    pieces: Vec<usize>,
    lengths_um: Vec<i128>,
    sums_um: Vec<i128>,
    /// The distances where the leg's path bends (see `Routes::breakpoints`), made when first asked for.
    bends_um: OnceCell<Option<Arc<[i128]>>>,
    /// The box of each piece, made when first asked for (`None` for a step whose points are not all known).
    boxes: OnceCell<Vec<Option<Bbox>>>,
}

impl WaterTable {
    /// The table of a water leg, or `None` when a piece has no step or a length that is not whole micrometres.
    fn build(
        scenario: &ck_map::Scenario,
        routes: &Routes,
        leg: &TripLeg,
        tables: &StretchTables,
    ) -> Option<Self> {
        let TripKind::Water { pieces, .. } = &leg.kind else {
            return None;
        };
        let (mut lengths_um, mut sums_um) = (Vec::with_capacity(pieces.len()), Vec::new());
        let mut total = 0_i128;
        for index in pieces {
            let length = tables.lengths.of(scenario, routes.water_step(*index)?)?;
            total = total.checked_add(length)?;
            lengths_um.push(length);
            sums_um.push(total);
        }
        Some(Self {
            pieces: pieces.clone(),
            lengths_um,
            sums_um,
            bends_um: OnceCell::new(),
            boxes: OnceCell::new(),
        })
    }

    /// The point at `distance_mm` along the leg, when the table can place it and the walk of `Routes::position` would stop at a step
    /// (`None` leaves the point to that walk).
    pub(crate) fn position(
        &self,
        scenario: &ck_map::Scenario,
        routes: &Routes,
        tables: &StretchTables,
        leg: &TripLeg,
        distance_mm: &BigRational,
    ) -> Option<Point> {
        if *distance_mm < zero() {
            return None;
        }
        let (along, length) = (to_um(distance_mm)?, to_um(&leg.length_mm)?);
        let distance = along.min(length);
        let at = self.sums_um.partition_point(|sum| *sum < distance);
        if at >= self.sums_um.len() {
            return None;
        }
        let before = if at == 0 { 0 } else { self.sums_um[at - 1] };
        let (left, step_length) = (distance - before, self.lengths_um[at]);
        let step = routes.water_step(self.pieces[at])?;
        match step {
            WaterStep::Open { from, to } => {
                let (a, b) = (centre(scenario, *from), centre(scenario, *to));
                along_step(integer_point(&a)?, integer_point(&b)?, left, step_length)
            }
            WaterStep::Reach { reach, downstream } => {
                // The walk's own distance along the reach: the share of the step done times the reach's length, cut to whole millimetres.
                let done = if *downstream {
                    left
                } else {
                    step_length - left
                };
                let point = tables
                    .reach_line(scenario, *reach)?
                    .position(u64::try_from(done / UM).ok()?)?;
                Some((
                    BigRational::from_integer(BigInt::from(point.x)),
                    BigRational::from_integer(BigInt::from(point.y)),
                ))
            }
            _ => step.position(scenario, &proportion(&from_um(step_length), &from_um(left))),
        }
    }

    /// The distances along the leg where its path bends, in whole micrometres, as `Routes::breakpoints` gives them for a water leg.
    pub(crate) fn breakpoints_um(
        &self,
        scenario: &ck_map::Scenario,
        routes: &Routes,
        tables: &StretchTables,
    ) -> Option<Arc<[i128]>> {
        self.bends_um
            .get_or_init(|| {
                let network = scenario.network.as_ref()?;
                let mut at: Vec<i128> = Vec::new();
                let mut cursor = 0_i128;
                for (piece, length) in self.pieces.iter().zip(&self.lengths_um) {
                    if let WaterStep::Reach { reach, downstream } = routes.water_step(*piece)?
                        && network.reaches.get(*reach).is_some()
                    {
                        // A river reach bends where its survey does; the bends are placed from the end the step is travelled from.
                        let mut inside: Vec<i128> = tables
                            .reach_line(scenario, *reach)
                            .map_or_else(Vec::new, |line| {
                                line.bends()
                                    .iter()
                                    .map(|bend| i128::from(*bend) * UM)
                                    .collect()
                            });
                        if !*downstream {
                            inside = inside.into_iter().map(|bend| length - bend).collect();
                        }
                        inside.sort_unstable();
                        at.extend(inside.into_iter().map(|bend| cursor + bend));
                    }
                    cursor = cursor.checked_add(*length)?;
                    at.push(cursor);
                }
                Some(Arc::from(at))
            })
            .clone()
    }

    /// The cumulative lengths of the leg's pieces, in whole micrometres.
    pub(crate) fn sums_um(&self) -> &[i128] {
        &self.sums_um
    }

    /// The piece that holds the point at `distance_um` along the leg: the first whose running sum reaches it.
    pub(crate) fn piece_at(&self, distance_um: i128) -> usize {
        self.sums_um.partition_point(|sum| *sum < distance_um)
    }

    /// The box of a piece, when every point of it is known.
    pub(crate) fn piece_box(
        &self,
        scenario: &ck_map::Scenario,
        routes: &Routes,
        tables: &StretchTables,
        piece: usize,
    ) -> Option<Bbox> {
        self.boxes
            .get_or_init(|| {
                self.pieces
                    .iter()
                    .map(|index| tables.step_box(scenario, routes.water_step(*index)?))
                    .collect()
            })
            .get(piece)
            .copied()
            .flatten()
    }
}

/// What a water step's length depends on: its kind, and the reach or the rows of the cells it joins and whether it is diagonal.
type StepKind = (u8, i64, i64, bool);

/// The length of a water step in whole micrometres, made once for each kind of step (a length depends only on the reach or the rows of
/// the cells it joins).
#[derive(Default)]
struct StepLengths {
    known: Mutex<HashMap<StepKind, Option<i128>>>,
}

impl StepLengths {
    fn of(&self, scenario: &ck_map::Scenario, step: &WaterStep) -> Option<i128> {
        let key = match step {
            WaterStep::Reach { reach, .. } => (0, i64::try_from(*reach).ok()?, 0, false),
            WaterStep::ReachJoin { .. } => (1, 0, 0, false),
            WaterStep::CellJoin { cell, .. } => (2, i64::from(cell[1]), 0, false),
            WaterStep::Open { from, to } => (
                3,
                i64::from(from[1]),
                i64::from(to[1]),
                from[0] != to[0] && from[1] != to[1],
            ),
        };
        *self
            .known
            .lock()
            .ok()?
            .entry(key)
            .or_insert_with(|| step.length(scenario).as_ref().and_then(to_um))
    }
}

/// The land tables made so far, by stretch and direction.
type LandTables = BTreeMap<(u64, bool), Option<Arc<LandTable>>>;

/// The tables the clips of a network have asked for, made once each: land stretches by stretch and direction, the lengths of water
/// steps, the surveyed lines of the reaches and the points of the joints.
#[derive(Default)]
pub(crate) struct StretchTables {
    tables: Mutex<LandTables>,
    lengths: StepLengths,
    reaches: Mutex<HashMap<usize, Option<Arc<ReachLine>>>>,
    joints: Mutex<HashMap<usize, Option<(i128, i128)>>>,
}

impl StretchTables {
    /// The table of a stretch in a direction, or `None` for a stretch the network does not hold.
    pub(crate) fn land(
        &self,
        scenario: &ck_map::Scenario,
        way_id: u64,
        backward: bool,
    ) -> Option<Arc<LandTable>> {
        self.tables
            .lock()
            .ok()?
            .entry((way_id, backward))
            .or_insert_with(|| LandTable::build(scenario, way_id, backward).map(Arc::new))
            .clone()
    }

    /// The table of a land or tidal leg, the way `Routes::position` finds its stretch.
    pub(crate) fn of_leg(
        &self,
        scenario: &ck_map::Scenario,
        ways: &[ck_sim::travel::LandWay],
        leg: &TripLeg,
    ) -> Option<Arc<LandTable>> {
        match &leg.kind {
            TripKind::Land { way, backward } | TripKind::Tidal { way, backward } => {
                self.land(scenario, ways.get(*way)?.id, *backward)
            }
            _ => None,
        }
    }

    /// The table of a water leg of the search `routes`.
    pub(crate) fn water(
        &self,
        scenario: &ck_map::Scenario,
        routes: &Routes,
        leg: &TripLeg,
    ) -> Option<WaterTable> {
        WaterTable::build(scenario, routes, leg, self)
    }

    /// The surveyed line of reach number `reach`, measured once.
    pub(crate) fn reach_line(
        &self,
        scenario: &ck_map::Scenario,
        reach: usize,
    ) -> Option<Arc<ReachLine>> {
        self.reaches
            .lock()
            .ok()?
            .entry(reach)
            .or_insert_with(|| {
                let network = scenario.network.as_ref()?;
                network
                    .water()
                    .reach_line(network.reaches.get(reach)?)
                    .map(Arc::new)
            })
            .clone()
    }

    /// The point of a joint, as `WaterStep::position` finds it: the end of the first usable reach that touches it.
    fn joint_point(&self, scenario: &ck_map::Scenario, joint: usize) -> Option<(i128, i128)> {
        if let Some(found) = self.joints.lock().ok()?.get(&joint) {
            return *found;
        }
        let found = (|| {
            let network = scenario.network.as_ref()?;
            let (index, reach) =
                network.reaches.iter().enumerate().find(|(_, reach)| {
                    reach.usable && (reach.from == joint || reach.to == joint)
                })?;
            let distance = if reach.from == joint {
                0
            } else {
                reach.length_mm
            };
            let point = self.reach_line(scenario, index)?.position(distance)?;
            Some((i128::from(point.x), i128::from(point.y)))
        })();
        self.joints.lock().ok()?.insert(joint, found);
        found
    }

    /// The box that holds every point of a water step, when they are all known.
    fn step_box(&self, scenario: &ck_map::Scenario, step: &WaterStep) -> Option<Bbox> {
        let spanned = |a: (i128, i128), b: (i128, i128)| -> Bbox {
            [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
        };
        match step {
            WaterStep::Reach { reach, .. } => {
                let [x0, y0, x1, y1] = self.reach_line(scenario, *reach)?.extent()?;
                Some([
                    i128::from(x0),
                    i128::from(y0),
                    i128::from(x1),
                    i128::from(y1),
                ])
            }
            WaterStep::ReachJoin { from, to } => {
                // The step's point is the joint it comes from, once the joint it goes to is known too.
                self.joint_point(scenario, *to)?;
                let point = self.joint_point(scenario, *from)?;
                Some(spanned(point, point))
            }
            WaterStep::CellJoin { joint, cell, .. } => {
                let joint = self.joint_point(scenario, *joint)?;
                Some(spanned(joint, integer_point(&centre(scenario, *cell))?))
            }
            WaterStep::Open { from, to } => Some(spanned(
                integer_point(&centre(scenario, *from))?,
                integer_point(&centre(scenario, *to))?,
            )),
        }
    }
}
