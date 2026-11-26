//! The journeys of expeditions on the travel network (CK-432, trade spec 11b.3 rules 6 and 10 and section 7 rule 4).
//!
//! An expedition's way is the quickest itinerary CK-402 finds between two places, **clipped** at both ends: it starts from the first
//! point of the itinerary 2 km (in a straight line) from the sending place's centre and ends at the first later point within 2 km of
//! the destination's centre. Each leg of it is given to the economy as a [`ck_economy::Leg`]: the work and speed of each stretch, the
//! fixed delays that no pace changes, and a frozen path of the points it runs through, so the economy can time it at any pace (rule 6
//! scales each profile speed by the pace), carry the fractions of an hour from leg to leg and place the expedition on its way after a
//! save whatever the network becomes.
//!
//! **Where the clip keeps what.** A water leg keeps the work of each of its steps in proportion to the part of that step the clip
//! keeps, at that step's own speed; a landing's fixed delay lies at the shore, at the end of a boarding approach and the start of a
//! landing one, and is kept when the shore lies on the kept journey. A land or tidal stretch's work, and the wades, fords and tide wait
//! of a stretch, are kept in proportion to the length of the stretch that is kept: the network does not say where along a stretch a
//! ford or a wade lies, so this is the one approximation, applied to nothing else. A delay-only transfer (no ground approach, as at a
//! strait) takes the mode of the leg beside it, so a journey that is all on water needs no land carriers.

use std::collections::BTreeMap;

use ck_economy::{Delays, Leg, Mode, Speed, Term};
use ck_sim::travel::{LandWay, TravelProfile, TravelRules, WayState};
use num_bigint::BigInt;
use num_rational::BigRational;

use super::travel::{Routes, TravelPlanner, TripKind, TripLeg, WaterStep};

/// The fixed-point units in a cell side (the drawing positions of CK-402).
const CELL_UNITS: i64 = 32768;

fn whole(value: i64) -> BigRational {
    BigRational::from_integer(BigInt::from(value))
}

fn ratio(numer: i64, denom: i64) -> BigRational {
    BigRational::new(BigInt::from(numer), BigInt::from(denom))
}

/// A point of the map in fixed-point cell units.
type Point = (BigRational, BigRational);

fn centre(scenario: &ck_map::Scenario, cell: [i32; 2]) -> Point {
    let origin = scenario.map.data.crop.origin;
    (
        whole((i64::from(origin.x) + i64::from(cell[0])) * CELL_UNITS + CELL_UNITS / 2),
        whole((i64::from(origin.y) + i64::from(cell[1])) * CELL_UNITS + CELL_UNITS / 2),
    )
}

fn distance_squared(a: &Point, b: &Point) -> BigRational {
    let dx = &a.0 - &b.0;
    let dy = &a.1 - &b.1;
    &dx * &dx + &dy * &dy
}

/// The part of `[a0, a1]` that lies in `[b0, b1]`, as a length.
fn overlap(a0: &BigRational, a1: &BigRational, b0: &BigRational, b1: &BigRational) -> BigRational {
    let low = a0.max(b0);
    let high = a1.min(b1);
    if high > low { high - low } else { whole(0) }
}

/// One way of a journey: its legs for the economy.
pub(crate) struct Journey {
    /// The legs, in the order they are travelled.
    pub(crate) legs: Vec<Leg>,
    /// The point the way ends at (the first point within the visiting radius of its destination), in fixed-point cell units, rounded to
    /// whole map units like the legs' paths: where an expedition on it waits, and the plan's `arrival` for a way with no leg.
    pub(crate) arrival: [i64; 2],
}

impl Journey {
    /// The point of the map an expedition has reached when `done` of leg `index` of the journey is done, in fixed-point cell units
    /// (32,768 to a cell side), read from the leg's frozen path by its movement, or `None` for a leg that does not exist or has no
    /// path. It does not depend on the pace.
    #[must_use]
    pub(crate) fn position(&self, index: usize, done: &BigRational) -> Option<Point> {
        self.legs.get(index)?.position(done)
    }
}

/// Builds journeys for one party over a scenario's network.
pub(crate) struct JourneyBuilder<'a> {
    scenario: &'a ck_map::Scenario,
    rules: &'a TravelRules,
    base: &'a TravelProfile,
    planner: TravelPlanner,
    states: &'a BTreeMap<u64, WayState>,
    radius_m: u64,
}

impl<'a> JourneyBuilder<'a> {
    /// A builder whose legs are timed from the speeds of `base` and whose way is the quickest under `effective`, the same profile at
    /// the pace the expedition travels the first leg at (they are one profile at pace 1000).
    ///
    /// # Errors
    /// A message if the scenario or a profile cannot form a travel graph.
    pub(crate) fn new(
        scenario: &'a ck_map::Scenario,
        rules: &'a TravelRules,
        base: &'a TravelProfile,
        effective: &TravelProfile,
        states: &'a BTreeMap<u64, WayState>,
        radius_m: u64,
    ) -> Result<Self, String> {
        Ok(Self {
            scenario,
            rules,
            base,
            planner: TravelPlanner::new(scenario, effective, rules)?,
            states,
            radius_m,
        })
    }

    /// The clipped quickest way from the place `from` to the place `to` (network node ids).
    ///
    /// # Errors
    /// A message when the places are not connected for this party, or the way never leaves the first 2 km.
    pub(crate) fn way(&self, from: u64, to: u64) -> Result<Journey, String> {
        let n = self
            .scenario
            .network
            .as_ref()
            .ok_or("scenario has no network")?;
        let routes = self.planner.routes_from(self.scenario, self.states, from)?;
        let trip = routes
            .itinerary(self.scenario, self.planner.ways(), to, self.rules)
            .ok_or("the places are not connected for this party")?;
        let node_point = |id: u64| {
            n.nodes
                .iter()
                .find(|node| node.id == id)
                .map(|node| (centre(self.scenario, node.cell), node.cell))
                .ok_or_else(|| format!("unknown node {id}"))
        };
        let (home, home_cell) = node_point(from)?;
        let (away, away_cell) = node_point(to)?;
        let radius = |cell: [i32; 2]| -> Result<BigRational, String> {
            let side = n
                .cell_side_mm
                .get(usize::try_from(cell[1]).map_err(|_| "negative row")?)
                .copied()
                .ok_or("row outside the map")?;
            // The radius in fixed-point units at the row of the centre it is measured from.
            Ok(ratio(
                i64::try_from(self.radius_m * 1000).map_err(|_| "radius too large")? * CELL_UNITS,
                i64::try_from(side).map_err(|_| "cell side too large")?,
            ))
        };
        let (leave_radius, arrive_radius) = (radius(home_cell)?, radius(away_cell)?);
        let clipper = Clipper {
            scenario: self.scenario,
            ways: self.planner.ways(),
            routes: &routes,
            trip: &trip,
            units_per_mm: self.units_per_mm()?,
        };
        let start = clipper.first_outside(&home, &leave_radius)?;
        let end = clipper.first_inside(&away, &arrive_radius, &start)?;
        let mut legs: Vec<Leg> = Vec::new();
        for (index, leg) in trip.iter().enumerate() {
            let length = &leg.length_mm;
            let from_mm = if index == start.leg {
                start.along.clone()
            } else {
                whole(0)
            };
            let to_mm = if index == end.leg {
                end.along.clone()
            } else {
                length.clone()
            };
            // Legs before the start or after the end are not on the way. A leg that takes some of the way is on it; so is a leg of no
            // length (a transfer's delay) that lies before the end, but not one at the arrival point itself.
            let on = index >= start.leg
                && index <= end.leg
                && (to_mm > from_mm || (*length == whole(0) && index < end.leg));
            if !on {
                continue;
            }
            let mut modelled = self.model(leg, &routes, &from_mm, &to_mm)?;
            (modelled.path, modelled.along) = clipper.path(index, &from_mm, &to_mm)?;
            legs.push(modelled);
        }
        adopt_neighbouring_modes(&mut legs);
        let (x, y) = clipper.point(&end)?;
        let round = |value: &BigRational| -> Result<i64, String> {
            i64::try_from((value + ratio(1, 2)).floor().to_integer())
                .map_err(|_| "a point is outside the map".to_owned())
        };
        Ok(Journey {
            legs,
            arrival: [round(&x)?, round(&y)?],
        })
    }

    /// Fixed-point units in a millimetre of ground, at the smallest cell side (so a leg's length in units is never understated).
    fn units_per_mm(&self) -> Result<BigRational, String> {
        let n = self
            .scenario
            .network
            .as_ref()
            .ok_or("scenario has no network")?;
        let side = n
            .cell_side_mm
            .iter()
            .copied()
            .min()
            .ok_or("the network has no cell sides")?;
        Ok(ratio(
            CELL_UNITS,
            i64::try_from(side).map_err(|_| "cell side too large")?,
        ))
    }

    /// The part `[from, to]` (millimetres along the leg) of one CK-402 leg as the economy times it: the work of each stretch over its
    /// profile speed, and the fixed delays, each kept where it lies on the part kept (see the module's note on the one approximation).
    #[expect(
        clippy::too_many_lines,
        reason = "Each kind of leg is given its work, speed and delays here, side by side."
    )]
    fn model(
        &self,
        leg: &TripLeg,
        routes: &Routes,
        from: &BigRational,
        to: &BigRational,
    ) -> Result<Leg, String> {
        let n = self
            .scenario
            .network
            .as_ref()
            .ok_or("scenario has no network")?;
        let slowing = ratio(i64::from(self.base.slowing), 1000);
        let delay = |thousandths: u64| ratio(i64::try_from(thousandths).unwrap_or(i64::MAX), 1000);
        let length = &leg.length_mm;
        // The part of the leg kept, as a share of its length, for what cannot be located along it.
        let share = if *length == whole(0) {
            whole(1)
        } else {
            (to - from) / length
        };
        let mut terms: Vec<Term> = Vec::new();
        let mut fixed = whole(0);
        // The delays of a ford, a wade or a tide are spread along their stretch; a landing's or a boarding's lie at the shore.
        let mut delays = Delays::Along;
        let mode;
        match &leg.kind {
            TripKind::Land { way, backward } | TripKind::Tidal { way, backward } => {
                mode = Mode::Land;
                let way = self.planner.ways().get(*way).ok_or("unknown way")?;
                let state = self.states.get(&way.id).ok_or("missing way state")?;
                let (paths, roads) = self
                    .base
                    .path
                    .zip(self.base.road)
                    .ok_or("the party cannot walk")?;
                let level = usize::from(state.level);
                let speed = if state.road {
                    Speed::Road {
                        path: paths[level],
                        road: roads[level],
                        condition: state.condition,
                    }
                } else {
                    Speed::Plain(paths[level])
                };
                if let Some(length_m) = way.tidal_length_m {
                    terms.push(Term {
                        work: whole(i64::try_from(length_m).map_err(|_| "tidal length")?)
                            * &slowing
                            * &share,
                        speed,
                    });
                    fixed += delay(self.rules.tide_thousandths) * &share;
                } else {
                    let measure = if *backward {
                        &way.backward
                    } else {
                        &way.forward
                    };
                    terms.push(Term {
                        work: measure.hours(state.level, &whole(1)) * &slowing * &share,
                        speed,
                    });
                    let mut delays =
                        delay(self.rules.wade_thousandths * u64::try_from(way.wades).unwrap_or(0));
                    for width in &way.fords {
                        delays += delay(self.rules.ford_thousandths);
                        delays += BigRational::new(
                            BigInt::from(*width) * self.rules.ford_per_metre_thousandths,
                            BigInt::from(10_000),
                        );
                    }
                    fixed += delays * &share;
                }
            }
            TripKind::Transfer {
                landing, boarding, ..
            } => {
                mode = Mode::Land;
                let approach = landing
                    .and_then(|index| n.landings.get(index))
                    .map_or(0, |entry| entry.approach_m);
                if approach > 0 {
                    let path = self
                        .base
                        .path
                        .map(|speeds| speeds[0])
                        .ok_or("the party cannot walk")?;
                    terms.push(Term {
                        work: whole(i64::try_from(approach).map_err(|_| "approach")?)
                            * &slowing
                            * &share,
                        speed: Speed::Plain(path),
                    });
                }
                // The delay of boarding or landing lies at the shore: the end of a boarding approach, the start of a landing one. A
                // delay-only transfer is at its one point, which is on the journey whenever the leg is.
                let at = if *boarding { length.clone() } else { whole(0) };
                delays = if *boarding {
                    Delays::AtEnd
                } else {
                    Delays::AtStart
                };
                if *length == whole(0) || (*from <= at && at <= *to) {
                    fixed += delay(self.rules.transfer_thousandths);
                }
            }
            TripKind::Water { pieces, .. } => {
                mode = Mode::Water;
                // The stretches are kept in the order they are travelled, one for each run at the same speed, so the economy can say
                // where along the leg an expedition is at any hour of it.
                let mut cursor = whole(0);
                for piece in pieces {
                    let step = routes.water_step(*piece).ok_or("unknown water step")?;
                    for (part, speed) in self.step_speeds(step)? {
                        let next = &cursor + &part;
                        let kept = overlap(&cursor, &next, from, to);
                        let work = kept * &slowing / BigInt::from(1000);
                        cursor = next;
                        if work == whole(0) {
                            continue;
                        }
                        match terms.last_mut() {
                            Some(last) if last.speed == Speed::Plain(speed) => last.work += work,
                            _ => terms.push(Term {
                                work,
                                speed: Speed::Plain(speed),
                            }),
                        }
                    }
                }
            }
            TripKind::Strait { index, .. } => {
                mode = Mode::Water;
                let strait = n.straits.get(*index).ok_or("unknown strait")?;
                terms.push(Term {
                    work: whole(i64::try_from(strait.length_m).map_err(|_| "strait length")?)
                        * &slowing
                        * &share,
                    speed: Speed::Plain(self.base.sea.ok_or("missing sea speed")?),
                });
            }
        }
        let (kind, a, b) = match &leg.kind {
            TripKind::Land { way, backward } => (0, *way as u64, u64::from(*backward)),
            TripKind::Tidal { way, backward } => (1, *way as u64, u64::from(*backward)),
            TripKind::Transfer {
                landing, boarding, ..
            } => (
                2,
                landing.map_or(u64::MAX, |index| index as u64),
                u64::from(*boarding),
            ),
            TripKind::Water { from, to, .. } => (3, *from as u64, *to as u64),
            TripKind::Strait { index, backward } => (4, *index as u64, u64::from(*backward)),
        };
        Ok(Leg {
            mode,
            terms,
            fixed,
            delays,
            key: [kind, a, b, 0],
            along: Vec::new(),
            path: Vec::new(),
        })
    }

    /// The pieces of a water step with the profile speed each is travelled at (a step between a lough cell and a sea cell spends
    /// half its length at each speed, as CK-402 times it), in the order they are travelled.
    fn step_speeds(&self, step: &WaterStep) -> Result<Vec<(BigRational, u64)>, String> {
        let n = self
            .scenario
            .network
            .as_ref()
            .ok_or("scenario has no network")?;
        let river = self.base.river.ok_or("missing river speed")?;
        let lough = self.base.lough.ok_or("missing lough speed")?;
        let sea = self.base.sea.ok_or("missing sea speed")?;
        let width = usize::try_from(self.scenario.map.data.crop.width).map_err(|_| "width")?;
        let cell_speed = |cell: [i32; 2]| -> Result<u64, String> {
            let x = usize::try_from(cell[0]).map_err(|_| "negative column")?;
            let y = usize::try_from(cell[1]).map_err(|_| "negative row")?;
            match self.scenario.map.data.cells.get(y * width + x) {
                Some(ck_map::Surface::Lake) => Ok(lough),
                Some(ck_map::Surface::Sea { .. }) => Ok(sea),
                _ => Err("water move enters land".to_owned()),
            }
        };
        let length = step.length(self.scenario).ok_or("water step length")?;
        Ok(match step {
            WaterStep::Reach { reach, .. } => {
                let kind = n.reaches.get(*reach).ok_or("unknown reach")?.kind;
                vec![(
                    length,
                    match kind {
                        ck_map::ReachKind::River => river,
                        ck_map::ReachKind::Lough => lough,
                        ck_map::ReachKind::Tidal => sea,
                    },
                )]
            }
            WaterStep::ReachJoin { .. } => Vec::new(),
            WaterStep::CellJoin { cell, .. } => vec![(length, cell_speed(*cell)?)],
            WaterStep::Open { from, to } => {
                let half = length / BigInt::from(2);
                vec![(half.clone(), cell_speed(*from)?), (half, cell_speed(*to)?)]
            }
        })
    }
}

/// A delay-only transfer (no ground approach, as at a strait) adds its fixed delay and needs no carriers of its own: it takes the mode
/// of the leg before it, or of the leg after it when it is first, so a journey that needs no land travel needs no land carriers (11c.3
/// rule 10). A transfer with a ground approach keeps the land mode.
fn adopt_neighbouring_modes(legs: &mut [Leg]) {
    for index in 0..legs.len() {
        let delay_only = legs[index].key[0] == 2 && legs[index].terms.is_empty();
        if !delay_only {
            continue;
        }
        if index > 0 {
            legs[index].mode = legs[index - 1].mode;
        } else if let Some(next) = legs.get(1).map(|leg| leg.mode) {
            legs[index].mode = next;
        }
    }
}

/// A point along the CK-402 legs: the leg and the distance along it in millimetres.
struct Along {
    leg: usize,
    along: BigRational,
}

/// Finds the points of a route where it leaves or reaches a disc, to the millimetre.
struct Clipper<'a> {
    scenario: &'a ck_map::Scenario,
    ways: &'a [LandWay],
    routes: &'a Routes,
    trip: &'a [TripLeg],
    units_per_mm: BigRational,
}

/// The distance between samples along a leg, in millimetres, where nothing else is known of its shape.
const SAMPLE_MM: i64 = 100_000;

/// Where, between two points of a path that is a straight line between them, it first comes within the radius of `centre`, if it does
/// so at the nearest point of the line to `centre` and not at either end: the distance along the path of that nearest point. A path
/// that dips into the disc and out again between two samples is found by this, not by looking at the samples. `None` when the nearest
/// point is no closer than the radius or the points coincide.
fn chord_entry(
    from: &(BigRational, Point),
    to: &(BigRational, Point),
    centre: &Point,
    limit: &BigRational,
) -> Option<BigRational> {
    let (dx, dy) = (&to.1.0 - &from.1.0, &to.1.1 - &from.1.1);
    let length_squared = &dx * &dx + &dy * &dy;
    if length_squared == whole(0) {
        return None;
    }
    let dot = (&centre.0 - &from.1.0) * &dx + (&centre.1 - &from.1.1) * &dy;
    let t = (dot / &length_squared).max(whole(0)).min(whole(1));
    let nearest = (&from.1.0 + &dx * &t, &from.1.1 + &dy * &t);
    (distance_squared(&nearest, centre) <= *limit).then(|| &from.0 + (&to.0 - &from.0) * t)
}

impl Clipper<'_> {
    fn point(&self, at: &Along) -> Result<Point, String> {
        self.routes
            .position(self.scenario, self.ways, &self.trip[at.leg], &at.along)
            .ok_or_else(|| "a point of the way cannot be placed".to_owned())
    }

    /// The samples of leg `leg` from `from_mm` on: every step, every bend of its path, and the leg's end, in order.
    fn samples(&self, leg: usize, from_mm: &BigRational) -> Vec<BigRational> {
        let length = &self.trip[leg].length_mm;
        let mut out = vec![from_mm.clone()];
        let mut at = from_mm + whole(SAMPLE_MM);
        while at < *length {
            out.push(at.clone());
            at += whole(SAMPLE_MM);
        }
        out.extend(
            self.routes
                .breakpoints(self.scenario, self.ways, &self.trip[leg])
                .into_iter()
                .filter(|bend| bend > from_mm && bend < length),
        );
        if *length > *from_mm {
            out.push(length.clone());
        }
        out.sort();
        out.dedup();
        out
    }

    /// The frozen path of the part `[from_mm, to_mm]` of leg `leg`: its points rounded to whole map units and each one's distance from
    /// the first in millimetres. Every bend of the leg on that part is a point, so the path runs along the network and not across its
    /// corners, and a point is added every [`SAMPLE_MM`] where no bend says otherwise (given up only for a leg too long to hold them).
    fn path(
        &self,
        leg: usize,
        from_mm: &BigRational,
        to_mm: &BigRational,
    ) -> Result<(Vec<[i64; 2]>, Vec<u64>), String> {
        let round = |value: &BigRational| -> Result<i64, String> {
            i64::try_from((value + ratio(1, 2)).floor().to_integer())
                .map_err(|_| "a point is outside the map".to_owned())
        };
        let mut stops: Vec<BigRational> = self
            .routes
            .breakpoints(self.scenario, self.ways, &self.trip[leg])
            .into_iter()
            .filter(|bend| bend > from_mm && bend < to_mm)
            .collect();
        let mut sampled = Vec::new();
        let mut at = from_mm + whole(SAMPLE_MM);
        while at < *to_mm && sampled.len() < ck_economy::MOST_LEG_PATH {
            sampled.push(at.clone());
            at += whole(SAMPLE_MM);
        }
        if stops.len() + sampled.len() + 2 <= ck_economy::MOST_LEG_PATH {
            stops.extend(sampled);
        }
        stops.sort();
        stops.dedup();
        if stops.len() + 2 > ck_economy::MOST_LEG_PATH {
            return Err("a leg has more bends than a path holds".to_owned());
        }
        stops.insert(0, from_mm.clone());
        if to_mm > from_mm {
            stops.push(to_mm.clone());
        }
        let mut points = Vec::with_capacity(stops.len());
        let mut along = Vec::with_capacity(stops.len());
        for stop in &stops {
            let (x, y) = self.point(&Along {
                leg,
                along: stop.clone(),
            })?;
            points.push([round(&x)?, round(&y)?]);
            along.push(
                u64::try_from(round(&(stop - from_mm))?).map_err(|_| "a leg runs backwards")?,
            );
        }
        Ok((points, along))
    }

    /// The least whole-millimetre distance in `(low, high]` along `leg` at which `test` holds, given that it fails at `low` and holds at
    /// `high`.
    fn bisect(
        &self,
        leg: usize,
        mut low: BigRational,
        mut high: BigRational,
        test: &dyn Fn(&Point) -> bool,
    ) -> Result<BigRational, String> {
        while &high - &low > whole(1) {
            let mut middle = ((&low + &high) / BigInt::from(2)).floor();
            if middle <= low {
                // A span of a little over one millimetre (the end of a leg is not a whole millimetre) still shrinks.
                middle = &low + whole(1);
            }
            let point = self.point(&Along {
                leg,
                along: middle.clone(),
            })?;
            if test(&point) {
                high = middle;
            } else {
                low = middle;
            }
        }
        Ok(high)
    }

    /// The first point of the route 2 km (not under) from `centre` in a straight line (rule 10: where the way leaves home).
    fn first_outside(&self, centre: &Point, radius: &BigRational) -> Result<Along, String> {
        let limit = radius * radius;
        let outside = |point: &Point| distance_squared(point, centre) >= limit;
        self.first(
            &Along {
                leg: 0,
                along: whole(0),
            },
            &outside,
            None,
        )?
        .ok_or_else(|| "the way never leaves the radius of its first place".to_owned())
    }

    /// The first point at or after `start` within the radius of `centre` (rule 10: where the way arrives).
    fn first_inside(
        &self,
        centre: &Point,
        radius: &BigRational,
        start: &Along,
    ) -> Result<Along, String> {
        let limit = radius * radius;
        let inside = |point: &Point| distance_squared(point, centre) <= limit;
        let begin = Along {
            leg: start.leg,
            along: start.along.clone(),
        };
        self.first(&begin, &inside, Some((centre, radius)))?
            .ok_or_else(|| "the way never comes within the radius of its last place".to_owned())
    }

    /// The first point at or after `begin` where `test` holds. With `near` (a centre and radius) a leg that cannot come within the
    /// radius, because its start is farther from the centre than the leg is long plus the radius, is skipped without sampling, and
    /// between two samples the straight piece of path is tested at its nearest point to the centre, so a crossing that falls wholly
    /// between samples (a shallow or tangential one) is found.
    fn first(
        &self,
        begin: &Along,
        test: &dyn Fn(&Point) -> bool,
        near: Option<(&Point, &BigRational)>,
    ) -> Result<Option<Along>, String> {
        let mut previous: Option<(usize, BigRational, Point)> = None;
        for leg in begin.leg..self.trip.len() {
            let from = if leg == begin.leg {
                begin.along.clone()
            } else {
                whole(0)
            };
            if let Some((centre, radius)) = near {
                let start = self.point(&Along {
                    leg,
                    along: from.clone(),
                })?;
                let reach = (&self.trip[leg].length_mm - &from) * &self.units_per_mm + radius;
                let gap = distance_squared(&start, centre);
                if gap > &reach * &reach {
                    previous = None;
                    continue;
                }
            }
            for along in self.samples(leg, &from) {
                let here = self.point(&Along {
                    leg,
                    along: along.clone(),
                })?;
                if test(&here) {
                    // Between the last sample that failed and this one lies the first point; at the very first sample it is that one.
                    return Ok(Some(match &previous {
                        Some((before, before_along, _)) if *before == leg => Along {
                            leg,
                            along: self.bisect(leg, before_along.clone(), along, test)?,
                        },
                        _ => Along { leg, along },
                    }));
                }
                if let (Some((centre, radius)), Some((before, before_along, before_point))) =
                    (near, &previous)
                    && *before == leg
                    && let Some(nearest) = chord_entry(
                        &(before_along.clone(), before_point.clone()),
                        &(along.clone(), here.clone()),
                        centre,
                        &(radius * radius),
                    )
                {
                    return Ok(Some(Along {
                        leg,
                        along: self.bisect(leg, before_along.clone(), nearest, test)?,
                    }));
                }
                previous = Some((leg, along, here));
            }
        }
        Ok(None)
    }
}

/// The profile of an expedition at a pace (11b.3 rule 6): each path, road, river, lough and sea speed multiplied by the pace over 1000,
/// rounded down, at least 1, and the slowing unchanged.
#[must_use]
pub(crate) fn scaled_profile(profile: &TravelProfile, pace_pm: u32) -> TravelProfile {
    let scale = |speed: u64| (speed.saturating_mul(u64::from(pace_pm)) / 1000).max(1);
    TravelProfile {
        path: profile.path.map(|speeds| speeds.map(scale)),
        road: profile.road.map(|speeds| speeds.map(scale)),
        water_class: profile.water_class.clone(),
        river: profile.river.map(scale),
        lough: profile.lough.map(scale),
        sea: profile.sea.map(scale),
        slowing: profile.slowing,
    }
}

/// The clipped way out and back between two places at a pace, as the `travel-times --expedition` report prints it: the legs, the
/// hours and where the clipped way starts and ends, in map cells.
///
/// # Errors
/// A message when a way cannot be found or placed.
pub(crate) fn report(
    scenario: &ck_map::Scenario,
    rules: &TravelRules,
    profile: &TravelProfile,
    states: &BTreeMap<u64, WayState>,
    ways: [(u64, u64); 2],
    pace_pm: u32,
) -> Result<String, String> {
    use std::fmt::Write;
    let effective = scaled_profile(profile, pace_pm);
    let builder = JourneyBuilder::new(scenario, rules, profile, &effective, states, 2000)?;
    let n = scenario.network.as_ref().ok_or("scenario has no network")?;
    let name = |id: u64| {
        n.nodes
            .iter()
            .find(|node| node.id == id)
            .and_then(|node| node.place.clone())
            .unwrap_or_else(|| id.to_string())
    };
    let cells = BigRational::from_integer(CELL_UNITS.into());
    let mut output = String::new();
    for (label, (from, to)) in ["out", "back"].into_iter().zip(ways) {
        let journey = builder.way(from, to)?;
        let total = journey
            .legs
            .iter()
            .fold(whole(0), |sum, leg| sum + leg.hours(pace_pm));
        writeln!(
            output,
            "{label} {} -> {} at pace {pace_pm}: {} legs, {} h",
            name(from),
            name(to),
            journey.legs.len(),
            ck_sim::travel::shown_hours(&total)
        )
        .expect("string write");
        let point = |index: usize, done: i64| {
            journey.position(index, &whole(done)).map_or_else(
                || "unknown".to_owned(),
                |(x, y)| {
                    format!(
                        "({}, {})",
                        super::travel::thousandths(&(x / &cells)),
                        super::travel::thousandths(&(y / &cells))
                    )
                },
            )
        };
        if !journey.legs.is_empty() {
            writeln!(
                output,
                "  from {} to {}",
                point(0, 0),
                point(journey.legs.len() - 1, 1)
            )
            .expect("string write");
        }
        writeln!(
            output,
            "  arrives at ({}, {})",
            super::travel::thousandths(&(whole(journey.arrival[0]) / &cells)),
            super::travel::thousandths(&(whole(journey.arrival[1]) / &cells))
        )
        .expect("string write");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use ck_sim::{Canonical, Reader};

    use super::*;

    /// The shipped scenario, rules and the starting state of every stretch.
    fn world() -> (ck_map::Scenario, TravelRules, BTreeMap<u64, WayState>) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scenario = ck_map::load_scenario(root.join("assets/scenarios/ireland-768")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let n = scenario.network.as_ref().unwrap();
        let states = n
            .stretches
            .iter()
            .map(|way| {
                (
                    way.id,
                    WayState {
                        road: way.category == ck_map::LandCategory::Road,
                        level: way.level,
                        condition: 1000,
                    },
                )
            })
            .collect();
        (scenario, rules, states)
    }

    fn node(scenario: &ck_map::Scenario, name: &str) -> u64 {
        scenario
            .network
            .as_ref()
            .unwrap()
            .place_node(name)
            .unwrap()
            .id
    }

    fn hours_of(journey: &Journey, pace: u32) -> BigRational {
        journey
            .legs
            .iter()
            .fold(whole(0), |sum, leg| sum + leg.hours(pace))
    }

    /// With a radius of nothing the whole itinerary is kept, so each leg can be set beside the hours CK-402 gives it.
    #[test]
    fn at_pace_1000_every_leg_takes_exactly_the_hours_the_travel_network_gives_it() {
        let (scenario, rules, states) = world();
        for (profile, from, to) in [
            ("cart", "tara", "clonmacnoise"),
            ("currach", "tara", "killeany"),
            ("barge", "clonmacnoise", "clonfert"),
            ("currach", "killeany", "tara"),
        ] {
            let base = &rules.profiles[profile];
            let builder = JourneyBuilder::new(&scenario, &rules, base, base, &states, 0).unwrap();
            let (a, b) = (node(&scenario, from), node(&scenario, to));
            let journey = builder.way(a, b).unwrap();
            let routes = builder.planner.routes_from(&scenario, &states, a).unwrap();
            let trip = routes
                .itinerary(&scenario, builder.planner.ways(), b, &rules)
                .unwrap();
            assert_eq!(journey.legs.len(), trip.len(), "{profile} {from} {to}");
            for (leg, expected) in journey.legs.iter().zip(&trip) {
                assert_eq!(leg.hours(1000), expected.hours, "{profile} {from} {to}");
                let water = matches!(
                    expected.kind,
                    TripKind::Water { .. } | TripKind::Strait { .. }
                );
                // A delay-only transfer takes the mode of the leg beside it; every other leg is as its kind says.
                if !leg.terms.is_empty() || !matches!(expected.kind, TripKind::Transfer { .. }) {
                    assert_eq!(leg.mode == Mode::Water, water);
                }
            }
            assert_eq!(hours_of(&journey, 1000), routes.to(b).unwrap().hours);
        }
    }

    /// Where an expedition is on a leg follows its movement: the stretches of a water leg are kept in the order they are travelled, so
    /// the end of the first is reached when its own hours are done (not when that share of the leg's hours is), and the delay of a
    /// landing or a boarding is waited out at the shore where the network puts it, at the start or the end of the leg.
    #[test]
    fn a_legs_position_follows_its_stretches_in_order_and_its_shore_delays_wait_at_the_shore() {
        let (scenario, rules, states) = world();
        let (mut mixed, mut shore) = (0, 0);
        for (profile, from, to) in [
            ("currach", "tara", "killeany"),
            ("barge", "clonmacnoise", "clonfert"),
            ("currach", "killeany", "tara"),
            ("cart", "tara", "clonmacnoise"),
        ] {
            let base = &rules.profiles[profile];
            let builder = JourneyBuilder::new(&scenario, &rules, base, base, &states, 0).unwrap();
            let (a, b) = (node(&scenario, from), node(&scenario, to));
            let journey = builder.way(a, b).unwrap();
            for leg in &journey.legs {
                assert!(
                    leg.terms
                        .windows(2)
                        .all(|pair| pair[0].speed != pair[1].speed),
                    "a run at one speed is one stretch"
                );
                let Some(last) = leg.path.len().checked_sub(1) else {
                    continue;
                };
                let end = |at: usize| (whole(leg.path[at][0]), whole(leg.path[at][1]));
                let total = leg.hours(1000);
                if leg.terms.len() >= 2 && leg.delays == Delays::Along && last > 0 {
                    // The first stretch is done when its own hours are: the leg's position is then its share of the work.
                    let first = &leg.terms[0];
                    let work: BigRational = leg
                        .terms
                        .iter()
                        .fold(whole(0), |sum, term| sum + &term.work);
                    // The hours of the first stretch alone, from the leg with only that stretch and its share of the delays.
                    let first_only = Leg {
                        terms: vec![first.clone()],
                        fixed: &leg.fixed * &first.work / &work,
                        ..leg.clone()
                    };
                    let spent = first_only.hours(1000);
                    let fraction = spent / &total;
                    // Its distance along the leg is the first stretch's share; the path point before it, by the distances kept.
                    let target = &first.work / &work * BigInt::from(*leg.along.last().unwrap());
                    let index = (0..last)
                        .rev()
                        .find(|at| whole(i64::try_from(leg.along[*at]).unwrap()) <= target)
                        .unwrap();
                    let (from_at, to_at) = (
                        whole(i64::try_from(leg.along[index]).unwrap()),
                        whole(i64::try_from(leg.along[index + 1]).unwrap()),
                    );
                    let rest = if to_at == from_at {
                        whole(0)
                    } else {
                        (&target - &from_at) / (&to_at - &from_at)
                    };
                    let mix = |axis: usize| {
                        whole(leg.path[index][axis])
                            + (whole(leg.path[index + 1][axis]) - whole(leg.path[index][axis]))
                                * &rest
                    };
                    assert_eq!(
                        leg.position(&fraction),
                        Some((mix(0), mix(1))),
                        "{profile} {from} {to}"
                    );
                    mixed += 1;
                }
                if leg.delays != Delays::Along && leg.fixed > whole(0) && last > 0 {
                    let waited = &leg.fixed / &total;
                    if leg.delays == Delays::AtStart {
                        assert_eq!(leg.position(&(&waited / ratio(2, 1))), Some(end(0)));
                        assert_eq!(leg.position(&waited), Some(end(0)));
                    } else {
                        assert_eq!(leg.position(&(whole(1) - &waited)), Some(end(last)));
                    }
                    shore += 1;
                }
            }
        }
        assert!(mixed > 0, "a shipped route has a leg of mixed speeds");
        assert!(shore > 0, "a shipped route has a delay at a shore");
    }

    /// The frozen route passes through the network's bends: at every bend of a shipped route its position is the real one, to map rounding.
    #[test]
    fn the_frozen_route_passes_through_the_networks_bends_to_map_rounding() {
        let (scenario, rules, states) = world();
        let base = &rules.profiles["cart"];
        let builder = JourneyBuilder::new(&scenario, &rules, base, base, &states, 0).unwrap();
        let (from, to) = (node(&scenario, "tara"), node(&scenario, "clonmacnoise"));
        let routes = builder
            .planner
            .routes_from(&scenario, &states, from)
            .unwrap();
        let ways = builder.planner.ways();
        let trip = routes.itinerary(&scenario, ways, to, &rules).unwrap();
        let clipper = Clipper {
            scenario: &scenario,
            ways,
            routes: &routes,
            trip: &trip,
            units_per_mm: builder.units_per_mm().unwrap(),
        };
        let mut bends = 0;
        let mut worst = (whole(0), 0);
        for (index, leg) in trip.iter().enumerate() {
            if !matches!(leg.kind, TripKind::Land { .. }) || leg.length_mm <= whole(0) {
                continue;
            }
            let mut frozen = builder
                .model(leg, &routes, &whole(0), &leg.length_mm)
                .unwrap();
            assert_eq!(frozen.terms.len(), 1);
            (frozen.path, frozen.along) = clipper.path(index, &whole(0), &leg.length_mm).unwrap();
            for at in routes.breakpoints(&scenario, ways, leg) {
                if at >= leg.length_mm {
                    continue;
                }
                let expected = clipper
                    .point(&Along {
                        leg: index,
                        along: at.clone(),
                    })
                    .unwrap();
                let actual = frozen.position(&(at / &leg.length_mm)).unwrap();
                let error = distance_squared(&expected, &actual);
                if error > worst.0 {
                    worst = (error, index);
                }
                bends += 1;
            }
        }
        assert!(bends > 0);
        assert!(
            worst.0 <= whole(2),
            "{bends} bends checked; largest squared error {} map units on leg {}",
            worst.0.ceil().to_integer(),
            worst.1
        );
    }

    /// The frozen path of every kind of leg on the shipped routes (land, water, strait, transfer) passes through each bend of the network to
    /// map rounding, and does so after the leg is saved and loaded.
    #[test]
    fn every_leg_of_the_shipped_routes_freezes_its_bends_and_keeps_them_through_a_save() {
        let (scenario, rules, states) = world();
        let (mut legs_checked, mut bends) = (0, 0);
        for (profile, from, to) in [
            ("cart", "tara", "clonmacnoise"),
            ("currach", "tara", "killeany"),
            ("barge", "clonmacnoise", "clonfert"),
            ("currach", "killeany", "tara"),
        ] {
            let base = &rules.profiles[profile];
            let builder = JourneyBuilder::new(&scenario, &rules, base, base, &states, 0).unwrap();
            let (a, b) = (node(&scenario, from), node(&scenario, to));
            let routes = builder.planner.routes_from(&scenario, &states, a).unwrap();
            let ways = builder.planner.ways();
            let trip = routes.itinerary(&scenario, ways, b, &rules).unwrap();
            let clipper = Clipper {
                scenario: &scenario,
                ways,
                routes: &routes,
                trip: &trip,
                units_per_mm: builder.units_per_mm().unwrap(),
            };
            for (index, leg) in trip.iter().enumerate() {
                if leg.length_mm <= whole(0) {
                    continue;
                }
                let (path, along) = clipper.path(index, &whole(0), &leg.length_mm).unwrap();
                // One stretch over the whole leg, so progress is the share of the distance and only the path's shape is tested.
                let frozen = Leg {
                    mode: Mode::Land,
                    terms: vec![Term {
                        work: whole(1),
                        speed: Speed::Plain(1),
                    }],
                    fixed: whole(0),
                    delays: Delays::Along,
                    key: [0; 4],
                    along,
                    path,
                };
                let mut bytes = Vec::new();
                frozen.encode(&mut bytes).unwrap();
                let loaded = Leg::decode(&mut Reader::new(&bytes)).unwrap();
                assert_eq!(loaded, frozen);
                for at in routes.breakpoints(&scenario, ways, leg) {
                    if at >= leg.length_mm {
                        continue;
                    }
                    let expected = clipper
                        .point(&Along {
                            leg: index,
                            along: at.clone(),
                        })
                        .unwrap();
                    for held in [&frozen, &loaded] {
                        let actual = held.position(&(&at / &leg.length_mm)).unwrap();
                        assert!(
                            distance_squared(&expected, &actual) <= whole(2),
                            "{profile} {from} {to} leg {index} at {at}"
                        );
                    }
                    bends += 1;
                }
                legs_checked += 1;
            }
        }
        assert!(
            legs_checked > 4 && bends > 20,
            "{legs_checked} legs, {bends} bends"
        );
    }

    /// The way's arrival point is where its last leg's frozen path ends, so a waiting expedition is placed at the clipped end of the way.
    #[test]
    fn the_arrival_point_is_where_the_last_legs_path_ends() {
        let (scenario, rules, states) = world();
        for (profile, from, to) in [
            ("cart", "tara", "clonmacnoise"),
            ("currach", "tara", "killeany"),
            ("barge", "clonmacnoise", "clonfert"),
        ] {
            let base = &rules.profiles[profile];
            let builder =
                JourneyBuilder::new(&scenario, &rules, base, base, &states, 2000).unwrap();
            let journey = builder
                .way(node(&scenario, from), node(&scenario, to))
                .unwrap();
            let last = journey.legs.last().unwrap().path.last().unwrap();
            assert_eq!(*last, journey.arrival, "{profile} {from} {to}");
        }
    }

    /// Rule 6: the pace scales each profile speed, so a leg at pace 800 takes what CK-402 gives for the profile with its speeds scaled.
    #[test]
    fn a_pace_scales_the_profile_speeds_and_the_fixed_delays_stay() {
        let (scenario, rules, states) = world();
        for (profile, from, to) in [
            ("cart", "tara", "clonmacnoise"),
            ("currach", "tara", "killeany"),
        ] {
            let base = &rules.profiles[profile];
            let (a, b) = (node(&scenario, from), node(&scenario, to));
            for pace in [800_u32, 560, 1300] {
                let scaled = scaled_profile(base, pace);
                let builder =
                    JourneyBuilder::new(&scenario, &rules, base, &scaled, &states, 0).unwrap();
                let journey = builder.way(a, b).unwrap();
                let routes = builder.planner.routes_from(&scenario, &states, a).unwrap();
                // The same route, timed by the scaled profile's own search.
                assert_eq!(
                    hours_of(&journey, pace),
                    routes.to(b).unwrap().hours,
                    "{profile} {from} {to} at {pace}"
                );
            }
        }
    }

    #[test]
    fn the_way_is_clipped_to_two_kilometres_from_each_centre() {
        let (scenario, rules, states) = world();
        let base = &rules.profiles["cart"];
        let n = scenario.network.as_ref().unwrap();
        let clipped = JourneyBuilder::new(&scenario, &rules, base, base, &states, 2000).unwrap();
        let whole_way = JourneyBuilder::new(&scenario, &rules, base, base, &states, 0).unwrap();
        let (a, b) = (node(&scenario, "tara"), node(&scenario, "clonmacnoise"));
        let out = clipped.way(a, b).unwrap();
        let full = whole_way.way(a, b).unwrap();
        assert!(
            hours_of(&out, 1000) < hours_of(&full, 1000),
            "the clip takes time off both ends"
        );
        // The first point is where the way leaves 2 km of Tara's centre and the last where it reaches 2 km of the destination's.
        let side = |name: &str| {
            let cell = n.place_node(name).unwrap().cell;
            n.cell_side_mm[usize::try_from(cell[1]).unwrap()]
        };
        let radius = |name: &str| ratio(2_000_000 * CELL_UNITS, i64::try_from(side(name)).unwrap());
        let tara = centre(&scenario, n.place_node("tara").unwrap().cell);
        let there = centre(&scenario, n.place_node("clonmacnoise").unwrap().cell);
        let first = out.position(0, &whole(0)).unwrap();
        let (r, one) = (radius("tara"), whole(1));
        // The frozen path rounds its points to whole map units (a fifteen-thousandth of a cell).
        assert!(
            distance_squared(&first, &tara) >= (&r - &one) * (&r - &one),
            "not inside the radius"
        );
        assert!(
            distance_squared(&first, &tara) <= (&r + &one) * (&r + &one),
            "and at it, to a unit"
        );
        let last = out.legs.len() - 1;
        let end = out.position(last, &whole(1)).unwrap();
        let r = radius("clonmacnoise");
        assert!(
            distance_squared(&end, &there) <= (&r + &one) * (&r + &one),
            "within the radius of the destination, to a rounded unit"
        );
        assert!(
            distance_squared(&end, &there) + &r > (&r - &one) * (&r - &one) - &one,
            "and at its edge"
        );
        // The way back is clipped the same way from the destination.
        let back = clipped.way(b, a).unwrap();
        assert!(!back.legs.is_empty());
        let start = back.position(0, &whole(0)).unwrap();
        let r = radius("clonmacnoise");
        assert!(distance_squared(&start, &there) >= (&r - &one) * (&r - &one));
    }

    /// A straight path that passes just inside the radius between two samples, with both samples outside it, is found at its nearest
    /// point; the samples alone would miss it.
    #[test]
    fn a_shallow_crossing_between_two_samples_is_found_at_the_nearest_point_of_the_path() {
        let centre_point: Point = (whole(0), whole(0));
        let limit = whole(1000) * whole(1000);
        // The path runs along y = 999.9, 4,000 units to each side of the centre: both ends are far outside the disc of radius 1,000.
        let from = (whole(0), (whole(-4000), ratio(9999, 10)));
        let to = (whole(8000), (whole(4000), ratio(9999, 10)));
        assert!(distance_squared(&from.1, &centre_point) > limit);
        assert!(distance_squared(&to.1, &centre_point) > limit);
        let found = chord_entry(&from, &to, &centre_point, &limit).expect("the path dips inside");
        assert_eq!(found, whole(4000), "at the nearest point, half way along");
        // A path that stays outside is not found; one that only touches the circle is, being within it.
        let far = (whole(0), (whole(-4000), whole(1001)));
        let far_to = (whole(8000), (whole(4000), whole(1001)));
        assert_eq!(chord_entry(&far, &far_to, &centre_point, &limit), None);
        let touching = (whole(0), (whole(-4000), whole(1000)));
        let touching_to = (whole(8000), (whole(4000), whole(1000)));
        assert_eq!(
            chord_entry(&touching, &touching_to, &centre_point, &limit),
            Some(whole(4000))
        );
        // Two coincident points have no line between them.
        assert_eq!(chord_entry(&from, &from, &centre_point, &limit), None);
    }

    /// Clipping keeps located stretches and delays where they lie: the kept parts of a leg add up to the whole leg, and a landing's delay
    /// is counted once, with the part of the journey that holds the shore.
    #[test]
    fn a_clipped_water_leg_and_transfer_keep_what_lies_in_the_part_kept() {
        let (scenario, rules, states) = world();
        let base = &rules.profiles["currach"];
        let builder = JourneyBuilder::new(&scenario, &rules, base, base, &states, 0).unwrap();
        let (a, b) = (node(&scenario, "tara"), node(&scenario, "killeany"));
        let routes = builder.planner.routes_from(&scenario, &states, a).unwrap();
        let trip = routes
            .itinerary(&scenario, builder.planner.ways(), b, &rules)
            .unwrap();
        for leg in &trip {
            let length = &leg.length_mm;
            if *length == whole(0) {
                continue;
            }
            let full = builder
                .model(leg, &routes, &whole(0), length)
                .unwrap()
                .hours(1000);
            assert_eq!(full, leg.hours, "the whole leg is CK-402's");
            // Split at several points: the two parts' times add up to the whole except that a delay is counted in the part that holds
            // its place (here the shore) and not in the other.
            for split in [ratio(1, 3), ratio(1, 2), ratio(5, 6)] {
                let at = length * &split;
                let first = builder.model(leg, &routes, &whole(0), &at).unwrap();
                let second = builder.model(leg, &routes, &at, length).unwrap();
                let parts = first.hours(1000) + second.hours(1000);
                match &leg.kind {
                    TripKind::Water { .. } | TripKind::Strait { .. } => {
                        assert_eq!(parts, full, "water work is located by step and adds up");
                    }
                    TripKind::Transfer { .. } => {
                        // The approach walk adds up; the one delay is in exactly one part.
                        assert_eq!(parts, full, "the shore's delay is counted once");
                    }
                    TripKind::Land { .. } | TripKind::Tidal { .. } => {
                        assert_eq!(parts, full, "the proportional approximation also adds up");
                    }
                }
            }
        }
        // Keeping only the end of a water leg charges only what lies there, not the whole leg's time.
        let water = trip
            .iter()
            .find(|leg| {
                matches!(leg.kind, TripKind::Water { .. }) && leg.length_mm > whole(10_000_000)
            })
            .expect("a long water leg");
        let tail = builder
            .model(
                water,
                &routes,
                &(&water.length_mm - whole(1_000_000)),
                &water.length_mm,
            )
            .unwrap();
        assert!(tail.hours(1000) < water.hours.clone() / BigInt::from(2));
    }

    /// The frozen path of a leg places the expedition where the live network does, to the rounding of the path's points, and survives a
    /// save and a load of the economy's own state.
    #[test]
    fn the_frozen_path_places_the_expedition_where_the_live_network_does() {
        let (scenario, rules, states) = world();
        let base = &rules.profiles["cart"];
        let builder = JourneyBuilder::new(&scenario, &rules, base, base, &states, 2000).unwrap();
        let (a, b) = (node(&scenario, "tara"), node(&scenario, "clonmacnoise"));
        let journey = builder.way(a, b).unwrap();
        let first = &journey.legs[0];
        assert!(first.path.len() >= 2);
        // The ends of the path are the leg's own, and a quarter of the way is a quarter of the way along it.
        let start = first.position(&whole(0)).unwrap();
        assert_eq!(start.0, whole(first.path[0][0]));
        let end = first.position(&whole(1)).unwrap();
        assert_eq!(end.1, whole(first.path[first.path.len() - 1][1]));
        let mid = first.position(&ratio(1, 2)).unwrap();
        let on_the_way = journey.position(0, &ratio(1, 2)).unwrap();
        assert_eq!(mid, on_the_way);
        // The path survives the economy's canonical save of an expedition plan.
        let mut bytes = Vec::new();
        first.encode(&mut bytes).unwrap();
        let loaded = Leg::decode(&mut Reader::new(&bytes)).unwrap();
        assert_eq!(&loaded, first);
        assert_eq!(loaded.position(&ratio(1, 3)), first.position(&ratio(1, 3)));
    }

    /// A journey that needs no land travel needs no land carriers: a delay-only transfer takes the mode of the leg beside it.
    #[test]
    fn a_delay_only_transfer_takes_the_mode_of_the_leg_beside_it() {
        let transfer = |terms: Vec<Term>, mode| Leg {
            mode,
            terms,
            fixed: ratio(2, 1),
            delays: Delays::AtStart,
            key: [2, 0, 1, 0],
            along: Vec::new(),
            path: Vec::new(),
        };
        let water = Leg {
            mode: Mode::Water,
            terms: Vec::new(),
            fixed: whole(0),
            delays: Delays::Along,
            key: [4, 0, 0, 0],
            along: Vec::new(),
            path: Vec::new(),
        };
        let approach = vec![Term {
            work: whole(10),
            speed: Speed::Plain(1000),
        }];
        let mut legs = vec![
            transfer(Vec::new(), Mode::Land),
            water.clone(),
            transfer(Vec::new(), Mode::Land),
            transfer(approach, Mode::Land),
        ];
        adopt_neighbouring_modes(&mut legs);
        assert_eq!(legs[0].mode, Mode::Water, "first in the way: the leg after");
        assert_eq!(
            legs[2].mode,
            Mode::Water,
            "after the water: the leg before it"
        );
        assert_eq!(legs[3].mode, Mode::Land, "a ground approach stays on land");
        let mut last = vec![water, transfer(Vec::new(), Mode::Land)];
        adopt_neighbouring_modes(&mut last);
        assert_eq!(
            last[1].mode,
            Mode::Water,
            "at the end of the way, the leg before"
        );
    }

    /// A river reach bends where its survey does, and the breakpoints of a water leg include those bends, in the order and from the end the
    /// reach is travelled, so the exact entry search tests each straight piece of the river.
    #[test]
    fn a_water_leg_breaks_at_the_bends_of_its_surveyed_reaches_in_travel_order() {
        let (scenario, rules, states) = world();
        let base = &rules.profiles["currach"];
        let builder = JourneyBuilder::new(&scenario, &rules, base, base, &states, 0).unwrap();
        let n = scenario.network.as_ref().unwrap();
        let mut checked = 0;
        for (from, to) in [
            ("tara", "killeany"),
            ("clonmacnoise", "clonfert"),
            ("killeany", "tara"),
        ] {
            let (a, b) = (node(&scenario, from), node(&scenario, to));
            let routes = builder.planner.routes_from(&scenario, &states, a).unwrap();
            let trip = routes
                .itinerary(&scenario, builder.planner.ways(), b, &rules)
                .unwrap();
            for leg in &trip {
                let TripKind::Water { pieces, .. } = &leg.kind else {
                    continue;
                };
                if leg.length_mm == whole(0) {
                    continue;
                }
                let breaks = routes.breakpoints(&scenario, builder.planner.ways(), leg);
                assert!(breaks.windows(2).all(|pair| pair[0] <= pair[1]), "in order");
                assert_eq!(
                    breaks.last(),
                    Some(&leg.length_mm),
                    "ending at the leg's end"
                );
                let mut cursor = whole(0);
                for index in pieces {
                    let step = routes.water_step(*index).unwrap();
                    let length = step.length(&scenario).unwrap();
                    if let WaterStep::Reach { reach, downstream } = step {
                        for bend in n.water().bends_along(&n.reaches[*reach]) {
                            let bend = whole(i64::try_from(bend).unwrap());
                            let at = if *downstream {
                                &cursor + &bend
                            } else {
                                &cursor + &length - &bend
                            };
                            assert!(
                                breaks.contains(&at),
                                "a bend of reach {reach} is a breakpoint"
                            );
                            checked += 1;
                        }
                    }
                    cursor += length;
                }
            }
        }
        assert!(
            checked > 0,
            "the shipped routes cross a reach with an interior bend"
        );
    }
    #[test]
    fn backward_land_breakpoints_follow_the_leg_in_its_travel_direction() {
        let (scenario, rules, states) = world();
        let base = &rules.profiles["cart"];
        let builder = JourneyBuilder::new(&scenario, &rules, base, base, &states, 0).unwrap();
        let from = node(&scenario, "tara");
        let to = node(&scenario, "clonmacnoise");
        let routes = builder
            .planner
            .routes_from(&scenario, &states, from)
            .unwrap();
        let trip = routes
            .itinerary(&scenario, builder.planner.ways(), to, &rules)
            .unwrap();
        for mut leg in trip {
            let TripKind::Land { way, .. } = leg.kind else {
                continue;
            };
            leg.kind = TripKind::Land {
                way,
                backward: false,
            };
            let forward = routes.breakpoints(&scenario, builder.planner.ways(), &leg);
            let mut reversed: Vec<_> = forward
                .iter()
                .rev()
                .skip(1)
                .map(|at| &leg.length_mm - at)
                .collect();
            reversed.push(leg.length_mm.clone());
            if forward == reversed {
                continue;
            }
            assert_eq!(forward.last(), Some(&leg.length_mm));
            leg.kind = TripKind::Land {
                way,
                backward: true,
            };
            assert_eq!(
                routes.breakpoints(&scenario, builder.planner.ways(), &leg),
                reversed,
                "shipped land way {way}"
            );
            return;
        }
        panic!("the shipped route contains no land leg with unequal step lengths");
    }
}
