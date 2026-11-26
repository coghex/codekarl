//! Trade, part C: expeditions (CK-432; trade spec section 7 with rules 11b.3, 11c.2 and 11d.1).
//!
//! An **expedition** is a trade job's stockpile set moving: its vehicles (or none, on foot), its people by pop, its cargo and its
//! supplies, along the two itineraries of a **plan** that part D will produce and the tests give directly. This module holds the data
//! (the rules' numbers, a leg's timing at a pace, the plan and the expedition), the arithmetic that needs no place (what each leg
//! carries and who moves it, the pace of the slowest member, how far an hour takes it) and the saved form of all of them. The trade
//! job that makes an expedition, the swap at its destination and the unloading at home are in `trade.rs` and `swap.rs`.
//!
//! Time is exact: a leg takes the sum of its terms' work over the speed the pace gives, plus its fixed delays, and an expedition's
//! position is the leg it is on and the fraction of that leg done, so the fractions of an hour a leg leaves over are carried to the
//! next (11b.3 rule 8).

use ck_sim::{Canonical, CanonicalError, Reader, encode_length};
use num_bigint::{BigInt, BigUint, Sign};
use num_rational::BigRational;
use serde::Deserialize;

use std::collections::BTreeMap;

use crate::trade::{self, Job, JobKind, TradeCounters};
use crate::{DataError, PlaceState, Rules, missing_data};

/// One herd species' numbers for travel: the pace it is driven at on land and the room it takes aboard a boat (section 3.2).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DrivenAnimal {
    /// The species, by its animal's good id.
    pub animal: String,
    /// Its pace on the hoof, in per-mille of walking pace.
    pub pace_pm: u32,
    /// What it takes up aboard a boat, in millilitres; its weight is its good's.
    pub aboard_ml: u64,
}

/// The numbers of an expedition that are not a vehicle's (trade spec sections 3.2 and 3.3), all rules data.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExpeditionRules {
    /// What a person's pack holds on foot, in grams.
    pub pack_g: u64,
    /// What a person's pack holds on foot, in millilitres.
    pub pack_ml: u64,
    /// Walking pace, in per-mille of itself.
    pub walking_pace_pm: u32,
    /// How many driven animals one drover looks after.
    pub animals_per_drover: u64,
    /// How much slower a vehicle goes with its full weight aboard, in per-mille of its pace (T15).
    pub full_load_slowing_pm: u32,
    /// How much slower a vehicle goes at its fewest crew than at its full, in per-mille of its pace (T25).
    pub short_crew_slowing_pm: u32,
    /// The share of its pace an expedition keeps when its supplies are gone, in per-mille (T25).
    pub no_supplies_pace_pm: u32,
    /// The hours of travel for which a vehicle uses up the units of its wear its kind gives (T23: 30 days).
    pub wear_hours: u64,
    /// What a person eats in a day, in hundredths of a person-day (the economy's own).
    pub daily_hunger_centi: u64,
    /// The driven animals' numbers, one for each herd species an expedition may take: a species with none cannot be taken (an
    /// assignment that needs them is not possible).
    #[serde(default)]
    pub animals: Vec<DrivenAnimal>,
}

impl Default for ExpeditionRules {
    fn default() -> Self {
        Self {
            pack_g: 25_000,
            pack_ml: 50_000,
            walking_pace_pm: 1000,
            animals_per_drover: 20,
            full_load_slowing_pm: 250,
            short_crew_slowing_pm: 300,
            no_supplies_pace_pm: 500,
            wear_hours: 720,
            daily_hunger_centi: 100,
            animals: Vec::new(),
        }
    }
}

impl ExpeditionRules {
    /// Refuses numbers an expedition could not be run with.
    pub(crate) fn validate(&self, file: &str) -> Result<(), DataError> {
        let refuse = |field: &str, problem: &str| DataError {
            file: file.to_owned(),
            entry: format!("expedition.{field}"),
            problem: problem.to_owned(),
        };
        if self.walking_pace_pm == 0
            || self.no_supplies_pace_pm == 0
            || self.no_supplies_pace_pm > 1000
        {
            return Err(refuse(
                "walking_pace_pm",
                "paces must be positive, and the no-supplies share at most 1000",
            ));
        }
        if self.full_load_slowing_pm >= 1000 || self.short_crew_slowing_pm >= 1000 {
            return Err(refuse(
                "full_load_slowing_pm",
                "a slowing is under 1000 per-mille",
            ));
        }
        if self.animals_per_drover == 0 || self.wear_hours == 0 || self.daily_hunger_centi == 0 {
            return Err(refuse("animals_per_drover", "must be positive"));
        }
        if self.pack_g == 0 || self.pack_ml == 0 {
            return Err(refuse("pack_g", "a pack holds something"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for animal in &self.animals {
            if animal.pace_pm == 0 || !seen.insert(animal.animal.as_str()) {
                return Err(refuse(
                    "animals",
                    "names a repeated species, or a pace of nothing",
                ));
            }
        }
        Ok(())
    }

    /// The numbers of one herd species.
    pub(crate) fn driven(&self, animal: &str) -> Option<&DrivenAnimal> {
        self.animals.iter().find(|item| item.animal == animal)
    }
}

/// A non-negative exact number of hours, or of a fraction of a leg.
pub type Hours = BigRational;

/// The most bytes either part of a saved fraction may have: far above any journey a reachable game has.
const MOST_RATIO_BYTES: usize = 2048;

/// A whole number as an exact ratio.
pub(crate) fn whole(value: u64) -> Hours {
    BigRational::from_integer(BigInt::from(value))
}

/// The ratio `numer / denom`, which must have a positive denominator.
pub(crate) fn ratio(numer: u128, denom: u128) -> Hours {
    BigRational::new(BigInt::from(numer), BigInt::from(denom))
}

pub(crate) fn encode_ratio(value: &Hours, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    value.numer().magnitude().to_bytes_be().encode(out)?;
    value.denom().magnitude().to_bytes_be().encode(out)
}

pub(crate) fn decode_ratio(input: &mut Reader<'_>) -> Result<Hours, CanonicalError> {
    let numer = Vec::<u8>::decode(input)?;
    let denom = Vec::<u8>::decode(input)?;
    if numer.len() > MOST_RATIO_BYTES || denom.len() > MOST_RATIO_BYTES {
        return Err(CanonicalError::BadTag {
            ty: "ratio",
            tag: 0,
        });
    }
    let numer = BigInt::from_biguint(Sign::Plus, BigUint::from_bytes_be(&numer));
    let denom = BigInt::from_biguint(Sign::Plus, BigUint::from_bytes_be(&denom));
    if denom == BigInt::from(0) {
        return Err(CanonicalError::BadTag {
            ty: "ratio",
            tag: 1,
        });
    }
    let value = BigRational::new(numer.clone(), denom.clone());
    // Only the reduced form is the canonical one.
    if *value.numer() != numer || *value.denom() != denom {
        return Err(CanonicalError::BadTag {
            ty: "ratio",
            tag: 2,
        });
    }
    Ok(value)
}

/// What moves an expedition on a leg: the land vehicles and walkers, or the boats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Ground: roads, paths, tidal flats and the approach to a landing.
    Land,
    /// Water: rivers, loughs, the sea and straits.
    Water,
}

/// The speed a term of a leg is travelled at, in metres an hour, before the expedition's pace is applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Speed {
    /// One speed (a river, lough or sea speed, or a path's).
    Plain(u64),
    /// A road's speed, which is its path's and its road's mixed by the road's condition in thousandths (CK-402 `land_speed`).
    Road {
        /// The path speed at the stretch's level.
        path: u64,
        /// The road speed at the stretch's level.
        road: u64,
        /// The road's condition, in thousandths.
        condition: u16,
    },
}

impl Speed {
    /// The speed at a pace (11b.3 rule 6): each profile speed times the pace over 1000, rounded down, at least 1, and a road's two
    /// speeds then mixed as CK-402 mixes them.
    fn at(&self, pace_pm: u32) -> Hours {
        let scale = |speed: u64| {
            u128::from(speed)
                .saturating_mul(u128::from(pace_pm))
                .checked_div(1000)
                .unwrap_or(0)
                .max(1)
        };
        match self {
            Self::Plain(speed) => ratio(scale(*speed), 1),
            Self::Road {
                path,
                road,
                condition,
            } => {
                let condition = u128::from((*condition).min(1000));
                ratio(
                    scale(*path) * (1000 - condition) + scale(*road) * condition,
                    1000,
                )
            }
        }
    }
}

/// One stretch of a leg whose time depends on the speed: its work (hours times metres an hour) over the speed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    /// The work: the hours the stretch takes at a speed of one metre an hour.
    pub work: Hours,
    /// The speed it is travelled at.
    pub speed: Speed,
}

/// Where a leg's fixed delays lie along it. The network locates the delay of boarding or landing at the shore, which is an end of the
/// leg; it does not locate a ford, a wade or a tide, which are spread along their stretch in proportion to its length (the one stated
/// approximation of the clip, 11b.3 rule 10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delays {
    /// Spread over the leg's stretches in proportion to their work.
    Along,
    /// Before the leg's first stretch: the expedition waits at the leg's first point (a landing).
    AtStart,
    /// After the leg's last stretch: the expedition waits at the leg's last point (a boarding).
    AtEnd,
}

/// One leg of an itinerary: what moves along it, what takes time at the pace and what does not, and where it lies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leg {
    /// What moves the expedition on it.
    pub mode: Mode,
    /// The stretches that depend on speed, in the order they are travelled; each one's work is its distance, in the one unit all the
    /// stretches of a leg share, so the leg's path is divided among them in proportion to their work.
    pub terms: Vec<Term>,
    /// The fixed delays of transfers, fords, wades and tides, which no pace changes (11b.3 rule 6).
    pub fixed: Hours,
    /// Where those delays lie.
    pub delays: Delays,
    /// What kind of leg it is and where it lies, for the application's reading; the economy never looks at it.
    pub key: [u64; 4],
    /// The points the leg runs through, in fixed-point map units, at equal distances along the part of the way it covers, first to last
    /// (frozen when the way was chosen, so a saved expedition's position is the same whatever the network becomes). Empty for a leg that
    /// has none, such as one the tests make up.
    pub path: Vec<[i64; 2]>,
    /// The distance of each point of the path from its first, in millimetres of the way, so the bends of the network keep the places they
    /// are at: the same length as the path, from 0, never decreasing. Empty for a path whose points are at equal distances.
    pub along: Vec<u64>,
}

/// The most points a leg's path holds.
pub const MOST_LEG_PATH: usize = 4096;

/// The pace at which a leg's progress is measured: a leg is a run of pieces (stretches walked, delays waited), and how much of it is
/// done is how much of that run is, each piece weighed by its hours at this pace. Progress is then the same whatever the pace is
/// afterwards: a change of pace changes how long each piece takes, never which piece or how far into it an expedition is.
const REFERENCE_PACE_PM: u32 = 1000;

/// One piece of a leg: a stretch walked at its own speed (with its share of any delay spread along the leg) or a delay waited out.
struct Piece<'a> {
    /// The distance it covers, in the one unit all the stretches of a leg share; none for a delay waited out.
    distance: Hours,
    /// What takes time at the pace: the stretch's work and speed.
    moving: Option<(&'a Hours, &'a Speed)>,
    /// What takes time whatever the pace is.
    fixed: Hours,
}

impl Piece<'_> {
    /// The hours the piece takes at a pace.
    fn hours(&self, pace_pm: u32) -> Hours {
        match self.moving {
            Some((work, speed)) => work / speed.at(pace_pm) + &self.fixed,
            None => self.fixed.clone(),
        }
    }
}

impl Leg {
    /// The leg as the run of pieces it is travelled in: a delay at the start, the stretches in order each with its share of the delays
    /// along them, a delay at the end. A leg with no distance is one delay. No piece takes no time.
    fn pieces(&self) -> Vec<Piece<'_>> {
        let total = self
            .terms
            .iter()
            .fold(whole(0), |sum, term| sum + &term.work);
        let wait = |hours: Hours| Piece {
            distance: whole(0),
            moving: None,
            fixed: hours,
        };
        let mut pieces = Vec::with_capacity(self.terms.len() + 2);
        if total == whole(0) {
            if self.fixed != whole(0) {
                pieces.push(wait(self.fixed.clone()));
            }
            return pieces;
        }
        if self.delays == Delays::AtStart && self.fixed != whole(0) {
            pieces.push(wait(self.fixed.clone()));
        }
        for term in self.terms.iter().filter(|term| term.work != whole(0)) {
            pieces.push(Piece {
                distance: term.work.clone(),
                moving: Some((&term.work, &term.speed)),
                fixed: if self.delays == Delays::Along {
                    &self.fixed * &term.work / &total
                } else {
                    whole(0)
                },
            });
        }
        if self.delays == Delays::AtEnd && self.fixed != whole(0) {
            pieces.push(wait(self.fixed.clone()));
        }
        pieces
    }

    /// Which piece the progress `done` (from 0 up to 1) is in, and how far into it: the piece's index and the share of it done.
    fn locate(pieces: &[Piece<'_>], done: &Hours) -> (usize, Hours) {
        let weights: Vec<Hours> = pieces
            .iter()
            .map(|piece| piece.hours(REFERENCE_PACE_PM))
            .collect();
        let total = weights.iter().fold(whole(0), |sum, weight| sum + weight);
        if total == whole(0) {
            return (pieces.len(), whole(0));
        }
        let mut left = done.clone().max(whole(0)).min(whole(1)) * &total;
        for (index, weight) in weights.iter().enumerate() {
            if left < *weight {
                return (index, left / weight);
            }
            left -= weight;
        }
        (pieces.len(), whole(0))
    }

    /// The progress (from 0 up to 1) that is `share` of the way through piece `index`.
    fn progress_at(pieces: &[Piece<'_>], index: usize, share: &Hours) -> Hours {
        let weights: Vec<Hours> = pieces
            .iter()
            .map(|piece| piece.hours(REFERENCE_PACE_PM))
            .collect();
        let total = weights.iter().fold(whole(0), |sum, weight| sum + weight);
        if total == whole(0) {
            return whole(0);
        }
        let before = weights[..index]
            .iter()
            .fold(whole(0), |sum, weight| sum + weight);
        let spent = before
            + weights
                .get(index)
                .map_or_else(|| whole(0), |weight| weight * share);
        spent / total
    }

    /// The hours still to travel of the leg at a pace when `done` of it is: the rest of the piece it is in and every piece after it,
    /// each at that pace, so a located delay's rest takes the time it was always going to.
    #[must_use]
    pub fn hours_left(&self, done: &Hours, pace_pm: u32) -> Hours {
        let pieces = self.pieces();
        let (index, share) = Self::locate(&pieces, done);
        pieces
            .iter()
            .enumerate()
            .skip(index)
            .fold(whole(0), |sum, (at, piece)| {
                let hours = piece.hours(pace_pm);
                if at == index {
                    sum + hours * (whole(1) - &share)
                } else {
                    sum + hours
                }
            })
    }

    /// The progress after `budget` hours more travel at a pace, from `done`: `budget` must be less than the hours left.
    #[must_use]
    pub fn progress_after(&self, done: &Hours, budget: &Hours, pace_pm: u32) -> Hours {
        let pieces = self.pieces();
        let (mut index, share) = Self::locate(&pieces, done);
        let mut budget = budget.clone();
        let mut share = share;
        while let Some(piece) = pieces.get(index) {
            let hours = piece.hours(pace_pm);
            let rest = &hours * (whole(1) - &share);
            if budget < rest {
                share += budget / hours;
                return Self::progress_at(&pieces, index, &share);
            }
            budget -= rest;
            index += 1;
            share = whole(0);
        }
        whole(1)
    }

    /// The point the expedition has reached when `done` of the leg is (from 0 up to 1), by linear interpolation between the points of
    /// its frozen path, in fixed-point map units; `None` for a leg with no path. It follows the movement: the stretches in order, and
    /// a delay at the shore waited out at the shore, so it depends on the progress alone and not on the pace it was made at.
    #[must_use]
    pub fn position(&self, done: &Hours) -> Option<(Hours, Hours)> {
        let last = self.path.len().checked_sub(1)?;
        let pieces = self.pieces();
        let total: Hours = pieces
            .iter()
            .fold(whole(0), |sum, piece| sum + &piece.distance);
        let (index, share) = Self::locate(&pieces, done);
        let fraction = if total == whole(0) {
            whole(0)
        } else {
            let before = pieces[..index]
                .iter()
                .fold(whole(0), |sum, piece| sum + &piece.distance);
            let inside = pieces
                .get(index)
                .map_or_else(|| whole(0), |piece| &piece.distance * &share);
            (before + inside) / total
        };
        if last == 0 {
            let [x, y] = self.path[0];
            return Some((
                BigRational::from_integer(x.into()),
                BigRational::from_integer(y.into()),
            ));
        }
        let (index, rest) = self.segment(&fraction, last)?;
        let ([ax, ay], [bx, by]) = (self.path[index], self.path[index + 1]);
        let mix = |a: i64, b: i64| {
            BigRational::from_integer(a.into())
                + (BigRational::from_integer(b.into()) - BigRational::from_integer(a.into()))
                    * &rest
        };
        Some((mix(ax, bx), mix(ay, by)))
    }

    /// The segment of the path `fraction` of the leg's distance lies in (the index of its first point) and how far along it, from 0 to 1.
    fn segment(&self, fraction: &Hours, last: usize) -> Option<(usize, Hours)> {
        if self.along.is_empty() {
            let along = fraction * BigInt::from(last);
            let index = usize::try_from(along.floor().to_integer())
                .ok()?
                .min(last - 1);
            return Some((index, along - BigInt::from(index)));
        }
        let total = *self.along.last()?;
        if total == 0 {
            return Some((0, whole(0)));
        }
        let target = fraction * BigInt::from(total);
        let index = self.along[..last]
            .partition_point(|at| BigRational::from_integer(BigInt::from(*at)) <= target)
            .saturating_sub(1);
        let (from, to) = (self.along[index], self.along[index + 1]);
        let rest = if to == from {
            whole(0)
        } else {
            ((target - BigInt::from(from)) / BigInt::from(to - from)).min(whole(1))
        };
        Some((index, rest))
    }

    /// The hours the leg takes at a pace, exactly.
    #[must_use]
    pub fn hours(&self, pace_pm: u32) -> Hours {
        self.terms.iter().fold(self.fixed.clone(), |sum, term| {
            sum + &term.work / term.speed.at(pace_pm)
        })
    }
}

/// A plan's leg list is as long as the longest itinerary a game can hold, and no more.
pub(crate) const MOST_LEGS: usize = 4096;

/// What a vehicle in an expedition is: its number, its kind and the hours of travel it has worn since its last unit of wear.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpeditionVehicle {
    /// The vehicle's number, which stays that of the vehicle at home.
    pub number: u64,
    /// Its kind, an index into the rules' vehicles.
    pub kind: u32,
    /// The hours it has moved since it last used up a unit of its wear.
    pub worn_hours: Hours,
}

/// The plan an expedition is carried out from (11b.4): the place it goes to and how, who goes and what is taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The destination's place index.
    pub destination: u32,
    /// The way out, clipped at both ends (11b.3 rule 10).
    pub out: Vec<Leg>,
    /// The way back, clipped the same way, from the first point 2 km from the destination.
    pub back: Vec<Leg>,
    /// The vehicles by number (empty on foot), all of the trader's.
    pub vehicles: Vec<u64>,
    /// The people by pop at home, `(pop, count)`, the trader's own first where it sends any; no pop twice.
    pub people: Vec<(u32, u64)>,
    /// What is taken to trade, in rules order; a live animal is the animal's own good.
    pub cargo: Vec<u64>,
    /// What the trip is to bring back, in rules order.
    pub wanted: Vec<u64>,
    /// The supplies for the round trip: food and the goods the vehicles wear, in rules order (T25).
    pub supplies: Vec<u64>,
    /// The point the way out ends at (where the expedition waits at the destination), in fixed-point map units, frozen with the way:
    /// what it stands at when the way out has no leg to end on (a trip that starts within the visiting radius).
    pub arrival: Option<[i64; 2]>,
}

/// Where an expedition is in its trip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Going to the destination.
    Outbound,
    /// At the destination, to take part in a noon's exchange.
    Waiting,
    /// Coming home.
    Homeward,
}

/// What sets an expedition's pace (for the panel and for the chip's icon).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limiter {
    /// A moving vehicle, by kind.
    Vehicle(u32),
    /// A driven species.
    Species(u32),
    /// People on foot.
    Walkers,
    /// Nothing limits it: there is nobody to slow it.
    Nothing,
}

/// A group of an expedition's people: all of one pop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// The pop's slot at home.
    pub pop: u32,
    /// How many.
    pub count: u64,
    /// The building each is taken from, as in [`crate::trade::Hands`]: their absence is counted against it.
    pub by_building: Vec<(u32, u64)>,
    /// What the pop was paid, as `(good, units)`, empty for the trader's own people.
    pub paid: Vec<(u32, u64)>,
}

/// An expedition on its way (T23).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expedition {
    /// Its number, in the order expeditions are made.
    pub number: u64,
    /// The sending pop's slot at home.
    pub trader: u32,
    /// The plan it was made from.
    pub plan: Plan,
    /// Its people, by pop.
    pub groups: Vec<Group>,
    /// Its vehicles.
    pub vehicles: Vec<ExpeditionVehicle>,
    /// What it holds to trade, and, once it has traded, what it brings home: in rules order, live animals included.
    pub cargo: Vec<u64>,
    /// What it holds of supplies, in rules order.
    pub supplies: Vec<u64>,
    /// Hunger opened but not yet eaten, in hundredths of a person-day (the pantry of the eating at noon).
    pub pantry_centi: u64,
    /// Whether its supplies are gone.
    pub gone: bool,
    /// The hour of the last noon it ate at, which with the supplies aboard says whether they are gone: they are exactly when it has eaten
    /// and nothing edible is left (a saved state is refused otherwise).
    pub fed: Option<u64>,
    /// Where it is in its trip.
    pub phase: Phase,
    /// Whether it has traded at the destination.
    pub traded: bool,
    /// The leg it is on, of the way out or of the way back as `phase` says.
    pub leg: u32,
    /// How much of that leg is done, from 0 up to but not including 1.
    pub done: Hours,
    /// Its pace on the leg, in per-mille of walking pace (11b.3 rule 7).
    pub pace_pm: u32,
    /// What sets it.
    pub limiter: Limiter,
    /// The supplies aboard when the pace was last reckoned, which the pace is reckoned from and which only fall after it: kept so the
    /// pace and its limiter can be checked against the load that gave them (a saved state is refused otherwise).
    pub reckoned: Vec<u64>,
    /// The kind of leg the pace was last reckoned for.
    pub reckoned_mode: Mode,
    /// The hour it left home.
    pub left: u64,
    /// The hour it arrived at the destination, once it has.
    pub arrived: Option<u64>,
    /// Whether it left in a noon's daily step, and so moved and ate in that very noon, rather than in an hour's step (11c.1).
    pub daily: bool,
    /// The supplies as they were when it was last loaded: leaving home, or turning for home with the return load. Every assignment of
    /// its load is the packing of these, less what has been eaten or worn since, so the assignment in use never fails as supplies fall
    /// (first-fit is not monotone: a smaller load can fail to pack where the larger one packed).
    pub loaded: Vec<u64>,
}

/// What one leg carries and who moves it (11b.3 rule 9, 11c.2 rules 5 and 6, 11d.1 rule 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Assigned {
    /// The moving vehicles as `(index in the expedition's vehicles, crew, weight carried in grams)`, in carrier order.
    pub(crate) moving: Vec<(usize, u64, u128)>,
    /// The people not in a moving vehicle's crew and not drovers, who walk (on land) or ride.
    pub(crate) walkers: u64,
    /// The drovers.
    pub(crate) drovers: u64,
    /// The driven species aboard on land, by herd species index.
    pub(crate) driven: Vec<usize>,
    /// The weight and space of what rides in each carrier or pack, for the panel: `(weight_g, space_ml)` by carrier.
    pub(crate) loads: Vec<(u128, u128)>,
}

/// A carrier with what it has room for.
struct Room {
    weight_g: u128,
    space_ml: u128,
}

/// A kind of thing to place: so many identical units that each weigh and take up so much. Vehicles come first, then animals, then
/// goods (`rank`), each by decreasing space, then decreasing weight, then `key` (kind order and number, or the good's order).
struct Item {
    rank: u8,
    space_ml: u128,
    weight_g: u128,
    key: usize,
    units: u64,
}

/// Puts `units` of an item into the first carrier of `rooms` that has room for each, as many as fit before the next, in whole units.
/// Returns the carrier each batch went to as `(carrier, units)`, or `None` if some unit has no carrier.
fn first_fit(rooms: &mut [Room], item: &Item) -> Option<Vec<(usize, u64)>> {
    let mut left = u128::from(item.units);
    let mut batches = Vec::new();
    for (index, room) in rooms.iter_mut().enumerate() {
        if left == 0 {
            break;
        }
        let by_weight = room
            .weight_g
            .checked_div(item.weight_g)
            .unwrap_or(u128::MAX);
        let by_space = room
            .space_ml
            .checked_div(item.space_ml)
            .unwrap_or(u128::MAX);
        let fit = by_weight.min(by_space).min(left);
        if fit == 0 {
            continue;
        }
        room.weight_g -= fit * item.weight_g;
        room.space_ml -= fit * item.space_ml;
        left -= fit;
        batches.push((index, u64::try_from(fit).ok()?));
    }
    (left == 0).then_some(batches)
}

/// Which species index an animal good is, if it is one.
pub(crate) fn species_of(rules: &Rules, good: usize) -> Option<usize> {
    let id = &rules.goods.get(good)?.id;
    rules
        .herd_species
        .iter()
        .position(|species| species.animal == *id)
}

/// The expedition's vehicles in carrier order: by kind in the rules' order, then by number (11c.2 rule 5).
fn kind_order(vehicles: &[ExpeditionVehicle]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..vehicles.len()).collect();
    order.sort_by_key(|index| (vehicles[*index].kind, vehicles[*index].number));
    order
}

/// Works out who moves on a leg and what each carries, for the load `cargo` and `supplies` (both in rules order), or `None` when the
/// leg cannot be made with these vehicles, people and load.
///
/// On land the vehicles that move are the ones with a pace on land; a boat is carried whole in one of them. On water the boats move
/// and the land vehicles and live animals are carried. On foot each person's pack is a carrier and no vehicle moves (r161).
/// People are assigned the fewest crew of each moving vehicle first, then the drovers, then spare people up to full crews, and anyone
/// left walks (11d.1).
pub(crate) fn assign_leg(
    rules: &Rules,
    vehicles: &[ExpeditionVehicle],
    people: u64,
    cargo: &[u64],
    supplies: &[u64],
    mode: Mode,
) -> Option<Assigned> {
    assign_partly(rules, vehicles, people, cargo, supplies, &[], mode)
}

/// What each carrier holds on a leg when the supplies `loaded` have since lost `eaten` (by good; none where the slice is short): the
/// load is packed as it was loaded and the eaten units are taken out of the last carriers that hold the good, so a load that packed
/// still packs after food is eaten and wear is spent, which a fresh first-fit of the smaller load does not promise (11b.3 rule 9).
#[expect(
    clippy::too_many_lines,
    reason = "One pass fixes the carriers, places the items, then the crew, so the three stay in step."
)]
pub(crate) fn assign_partly(
    rules: &Rules,
    vehicles: &[ExpeditionVehicle],
    people: u64,
    cargo: &[u64],
    supplies: &[u64],
    eaten: &[u64],
    mode: Mode,
) -> Option<Assigned> {
    let order = kind_order(vehicles);
    let kinds: Vec<&crate::VehicleKind> = vehicles
        .iter()
        .map(|vehicle| rules.vehicles.get(vehicle.kind as usize))
        .collect::<Option<_>>()?;
    let on_foot = vehicles.is_empty();
    if on_foot && mode == Mode::Water {
        return None;
    }
    let moves = |kind: &crate::VehicleKind| match mode {
        Mode::Land => kind.land_pace_pm.is_some(),
        Mode::Water => kind.water.is_some(),
    };
    let moving: Vec<usize> = order.iter().copied().filter(|i| moves(kinds[*i])).collect();
    if !on_foot && moving.is_empty() {
        return None;
    }
    // The carriers are the moving vehicles or, on foot, the people's packs.
    let mut rooms: Vec<Room> = if on_foot {
        (0..people)
            .map(|_| Room {
                weight_g: u128::from(rules.expedition.pack_g),
                space_ml: u128::from(rules.expedition.pack_ml),
            })
            .collect()
    } else {
        moving
            .iter()
            .map(|i| Room {
                weight_g: u128::from(kinds[*i].carries_g),
                space_ml: u128::from(kinds[*i].carries_ml),
            })
            .collect()
    };
    let mut items: Vec<Item> = Vec::new();
    // A vehicle that does not move on this leg rides whole, and one nothing can carry cannot make the leg (T15).
    for (position, index) in order.iter().enumerate() {
        if moves(kinds[*index]) {
            continue;
        }
        let (Some(weight), Some(space)) = (kinds[*index].carried_g, kinds[*index].carried_ml)
        else {
            return None;
        };
        items.push(Item {
            rank: 0,
            space_ml: u128::from(space),
            weight_g: u128::from(weight),
            key: position,
            units: 1,
        });
    }
    let mut driven: Vec<(usize, u64)> = Vec::new();
    for (good, item) in rules.goods.iter().enumerate() {
        let cargo_units = cargo.get(good).copied().unwrap_or(0);
        let supply_units = supplies.get(good).copied().unwrap_or(0);
        let total = cargo_units.checked_add(supply_units)?;
        if total == 0 {
            continue;
        }
        if let Some(species) = species_of(rules, good) {
            if supply_units != 0 {
                return None;
            }
            if mode == Mode::Land {
                driven.push((species, cargo_units));
            } else {
                let aboard = rules
                    .expedition
                    .driven(&rules.herd_species[species].animal)?;
                items.push(Item {
                    rank: 1,
                    space_ml: u128::from(aboard.aboard_ml),
                    weight_g: u128::from(item.weight_g),
                    key: good,
                    units: total,
                });
            }
        } else {
            items.push(Item {
                rank: 2,
                space_ml: u128::from(item.volume_ml),
                weight_g: u128::from(item.weight_g),
                key: good,
                units: total,
            });
        }
    }
    items.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then(b.space_ml.cmp(&a.space_ml))
            .then(b.weight_g.cmp(&a.weight_g))
            .then(a.key.cmp(&b.key))
    });
    let mut loads = vec![(0_u128, 0_u128); rooms.len()];
    // Where each batch of goods went, to take what has been eaten out of the last of them.
    let mut placed: Vec<(usize, usize, u128, &Item)> = Vec::new();
    for item in &items {
        for (carrier, count) in first_fit(&mut rooms, item)? {
            let count = u128::from(count);
            loads[carrier].0 += count * item.weight_g;
            loads[carrier].1 += count * item.space_ml;
            if item.rank == 2 {
                placed.push((item.key, carrier, count, item));
            }
        }
    }
    for (good, gone) in eaten.iter().enumerate() {
        let mut gone = u128::from(*gone);
        for (_, carrier, count, item) in placed.iter().rev().filter(|batch| batch.0 == good) {
            let taken = gone.min(*count);
            loads[*carrier].0 -= taken * item.weight_g;
            loads[*carrier].1 -= taken * item.space_ml;
            gone -= taken;
        }
    }
    // The crew (11d.1): the fewest of each moving vehicle, then the drovers, then spare people up to the full crews.
    let driven_total = driven
        .iter()
        .try_fold(0_u64, |sum, (_, units)| sum.checked_add(*units))?;
    let drovers = driven_total.div_ceil(rules.expedition.animals_per_drover);
    let mut crews = vec![0_u64; moving.len()];
    let mut left = people;
    for (slot, index) in moving.iter().enumerate().filter(|_| !on_foot) {
        let fewest = kinds[*index].crew_fewest;
        left = left.checked_sub(fewest)?;
        crews[slot] = fewest;
    }
    left = left.checked_sub(drovers)?;
    for (slot, index) in moving.iter().enumerate().filter(|_| !on_foot) {
        let added = (kinds[*index].crew_full - crews[slot]).min(left);
        crews[slot] += added;
        left -= added;
    }
    Some(Assigned {
        moving: moving
            .iter()
            .enumerate()
            .map(|(slot, index)| (*index, crews[slot], loads[slot].0))
            .collect(),
        walkers: left,
        drovers,
        driven: driven.into_iter().map(|(species, _)| species).collect(),
        loads,
    })
}

/// The assignment of a load that was packed as `loaded` and has `left` of its supplies now: see [`assign_partly`].
pub(crate) fn assign_after(
    rules: &Rules,
    vehicles: &[ExpeditionVehicle],
    people: u64,
    cargo: &[u64],
    loaded: &[u64],
    left: &[u64],
    mode: Mode,
) -> Option<Assigned> {
    let eaten: Vec<u64> = loaded
        .iter()
        .zip(left)
        .map(|(packed, now)| packed.saturating_sub(*now))
        .collect();
    assign_partly(rules, vehicles, people, cargo, loaded, &eaten, mode)
}

/// A moving vehicle's pace: its base pace times its load factor, rounded down, times its crew factor, rounded down (11c.2 rule 4).
fn vehicle_pace(
    rules: &Rules,
    kind: &crate::VehicleKind,
    mode: Mode,
    crew: u64,
    weight_g: u128,
) -> u32 {
    let base = match mode {
        Mode::Land => kind.land_pace_pm.unwrap_or(0),
        // A boat's own speeds are its profile's; its pace is the walking pace's thousand.
        Mode::Water => rules.expedition.walking_pace_pm,
    };
    let slowing = u128::from(rules.expedition.full_load_slowing_pm);
    let load = 1000
        - (slowing * weight_g)
            .checked_div(u128::from(kind.carries_g))
            .unwrap_or(0)
            .min(slowing);
    let paced = u128::from(base) * load / 1000;
    let crew_factor = if kind.crew_full > kind.crew_fewest {
        let spread = u128::from(rules.expedition.short_crew_slowing_pm);
        let short = u128::from(kind.crew_full - crew.min(kind.crew_full));
        1000 - (spread * short) / u128::from(kind.crew_full - kind.crew_fewest)
    } else {
        1000
    };
    u32::try_from(paced * crew_factor / 1000).unwrap_or(u32::MAX)
}

/// The pace of an expedition on a leg, and what sets it (11c.2 rule 4): the least of its moving vehicles' paces (each already lowered
/// by its load and crew), its walkers' pace and its driven species' paces, then lowered again by half when its supplies are gone.
pub(crate) fn pace_of(
    rules: &Rules,
    vehicles: &[ExpeditionVehicle],
    assigned: &Assigned,
    mode: Mode,
    gone: bool,
    on_foot: bool,
) -> (u32, Limiter) {
    let mut best: Option<(u32, Limiter)> = None;
    let mut offer = |pace: u32, limiter: Limiter| {
        if best.is_none_or(|(least, _)| pace < least) {
            best = Some((pace, limiter));
        }
    };
    for (index, crew, weight) in &assigned.moving {
        let vehicle = &vehicles[*index];
        let Some(kind) = rules.vehicles.get(vehicle.kind as usize) else {
            continue;
        };
        offer(
            vehicle_pace(rules, kind, mode, *crew, *weight),
            Limiter::Vehicle(vehicle.kind),
        );
    }
    if mode == Mode::Land && (assigned.walkers > 0 || on_foot) {
        offer(rules.expedition.walking_pace_pm, Limiter::Walkers);
    }
    if mode == Mode::Land {
        for species in &assigned.driven {
            if let Some(animal) = rules
                .herd_species
                .get(*species)
                .and_then(|item| rules.expedition.driven(&item.animal))
            {
                offer(
                    animal.pace_pm,
                    Limiter::Species(u32::try_from(*species).unwrap_or(u32::MAX)),
                );
            }
        }
    }
    let (pace, limiter) = best.unwrap_or((rules.expedition.walking_pace_pm, Limiter::Nothing));
    if gone {
        let pace =
            u32::try_from(u64::from(pace) * u64::from(rules.expedition.no_supplies_pace_pm) / 1000)
                .unwrap_or(u32::MAX);
        return (pace.max(1), limiter);
    }
    (pace.max(1), limiter)
}

/// How far `budget` hours of travel take an expedition along `legs` from `leg` and `done`, at a pace that may change at the next
/// leg: returns the new leg, the new fraction and whether the end was reached, with the pace for each leg given by `pace_at`.
///
/// A leg that takes no time is crossed at once. Reaching the end of the last leg is arriving; the hours the budget has left over are
/// then not used.
pub(crate) fn advance<F, S>(
    legs: &[Leg],
    leg: &mut usize,
    done: &mut Hours,
    mut budget: Hours,
    mut pace_at: F,
    mut spent: S,
) -> bool
where
    F: FnMut(usize) -> u32,
    S: FnMut(usize, &Hours),
{
    loop {
        let Some(current) = legs.get(*leg) else {
            return true;
        };
        let pace = pace_at(*leg);
        let remaining = current.hours_left(done, pace);
        if remaining <= budget {
            spent(*leg, &remaining);
            budget -= remaining;
            *leg += 1;
            *done = whole(0);
            continue;
        }
        spent(*leg, &budget);
        *done = current.progress_after(done, &budget, pace);
        return false;
    }
}

impl Canonical for Mode {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        u8::from(*self == Self::Water).encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        match u8::decode(input)? {
            0 => Ok(Self::Land),
            1 => Ok(Self::Water),
            tag => Err(CanonicalError::BadTag { ty: "Mode", tag }),
        }
    }
}

impl Canonical for Speed {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        match self {
            Self::Plain(speed) => {
                0_u8.encode(out)?;
                speed.encode(out)
            }
            Self::Road {
                path,
                road,
                condition,
            } => {
                1_u8.encode(out)?;
                path.encode(out)?;
                road.encode(out)?;
                condition.encode(out)
            }
        }
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        match u8::decode(input)? {
            0 => Ok(Self::Plain(u64::decode(input)?)),
            1 => Ok(Self::Road {
                path: u64::decode(input)?,
                road: u64::decode(input)?,
                condition: u16::decode(input)?,
            }),
            tag => Err(CanonicalError::BadTag { ty: "Speed", tag }),
        }
    }
}

impl Canonical for Delays {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        match self {
            Self::Along => 0_u8,
            Self::AtStart => 1,
            Self::AtEnd => 2,
        }
        .encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        match u8::decode(input)? {
            0 => Ok(Self::Along),
            1 => Ok(Self::AtStart),
            2 => Ok(Self::AtEnd),
            tag => Err(CanonicalError::BadTag { ty: "Delays", tag }),
        }
    }
}

impl Canonical for Leg {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.mode.encode(out)?;
        encode_length(self.terms.len(), out)?;
        for term in &self.terms {
            encode_ratio(&term.work, out)?;
            term.speed.encode(out)?;
        }
        encode_ratio(&self.fixed, out)?;
        self.delays.encode(out)?;
        for part in self.key {
            part.encode(out)?;
        }
        encode_length(self.path.len(), out)?;
        for [x, y] in &self.path {
            x.encode(out)?;
            y.encode(out)?;
        }
        self.along.encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let mode = Mode::decode(input)?;
        let count = input.length()?;
        let mut terms = Vec::with_capacity(count.min(input.remaining()));
        for _ in 0..count {
            terms.push(Term {
                work: decode_ratio(input)?,
                speed: Speed::decode(input)?,
            });
        }
        let fixed = decode_ratio(input)?;
        let delays = Delays::decode(input)?;
        let mut key = [0_u64; 4];
        for part in &mut key {
            *part = u64::decode(input)?;
        }
        let points = input.length()?;
        let mut path = Vec::with_capacity(points.min(input.remaining()));
        for _ in 0..points {
            path.push([i64::decode(input)?, i64::decode(input)?]);
        }
        let along = Vec::<u64>::decode(input)?;
        Ok(Self {
            mode,
            terms,
            fixed,
            delays,
            key,
            path,
            along,
        })
    }
}

impl Canonical for Plan {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.destination.encode(out)?;
        self.out.encode(out)?;
        self.back.encode(out)?;
        self.vehicles.encode(out)?;
        encode_length(self.people.len(), out)?;
        for (pop, count) in &self.people {
            pop.encode(out)?;
            count.encode(out)?;
        }
        self.cargo.encode(out)?;
        self.wanted.encode(out)?;
        self.supplies.encode(out)?;
        match self.arrival {
            Some([x, y]) => {
                true.encode(out)?;
                x.encode(out)?;
                y.encode(out)
            }
            None => false.encode(out),
        }
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let destination = u32::decode(input)?;
        let out = Vec::<Leg>::decode(input)?;
        let back = Vec::<Leg>::decode(input)?;
        let vehicles = Vec::<u64>::decode(input)?;
        let count = input.length()?;
        let mut people = Vec::with_capacity(count.min(input.remaining()));
        for _ in 0..count {
            people.push((u32::decode(input)?, u64::decode(input)?));
        }
        Ok(Self {
            destination,
            out,
            back,
            vehicles,
            people,
            cargo: Vec::<u64>::decode(input)?,
            wanted: Vec::<u64>::decode(input)?,
            supplies: Vec::<u64>::decode(input)?,
            arrival: if bool::decode(input)? {
                Some([i64::decode(input)?, i64::decode(input)?])
            } else {
                None
            },
        })
    }
}

impl Canonical for Phase {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        match self {
            Self::Outbound => 0_u8,
            Self::Waiting => 1,
            Self::Homeward => 2,
        }
        .encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        match u8::decode(input)? {
            0 => Ok(Self::Outbound),
            1 => Ok(Self::Waiting),
            2 => Ok(Self::Homeward),
            tag => Err(CanonicalError::BadTag { ty: "Phase", tag }),
        }
    }
}

impl Canonical for Limiter {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        match self {
            Self::Vehicle(kind) => {
                0_u8.encode(out)?;
                kind.encode(out)
            }
            Self::Species(species) => {
                1_u8.encode(out)?;
                species.encode(out)
            }
            Self::Walkers => 2_u8.encode(out),
            Self::Nothing => 3_u8.encode(out),
        }
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        match u8::decode(input)? {
            0 => Ok(Self::Vehicle(u32::decode(input)?)),
            1 => Ok(Self::Species(u32::decode(input)?)),
            2 => Ok(Self::Walkers),
            3 => Ok(Self::Nothing),
            tag => Err(CanonicalError::BadTag { ty: "Limiter", tag }),
        }
    }
}

impl Canonical for Expedition {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.number.encode(out)?;
        self.trader.encode(out)?;
        self.plan.encode(out)?;
        encode_length(self.groups.len(), out)?;
        for group in &self.groups {
            group.pop.encode(out)?;
            group.count.encode(out)?;
            encode_length(group.by_building.len(), out)?;
            for (building, count) in &group.by_building {
                building.encode(out)?;
                count.encode(out)?;
            }
            encode_length(group.paid.len(), out)?;
            for (good, units) in &group.paid {
                good.encode(out)?;
                units.encode(out)?;
            }
        }
        encode_length(self.vehicles.len(), out)?;
        for vehicle in &self.vehicles {
            vehicle.number.encode(out)?;
            vehicle.kind.encode(out)?;
            encode_ratio(&vehicle.worn_hours, out)?;
        }
        self.cargo.encode(out)?;
        self.supplies.encode(out)?;
        self.pantry_centi.encode(out)?;
        self.gone.encode(out)?;
        self.fed.encode(out)?;
        self.phase.encode(out)?;
        self.traded.encode(out)?;
        self.leg.encode(out)?;
        encode_ratio(&self.done, out)?;
        self.pace_pm.encode(out)?;
        self.limiter.encode(out)?;
        self.reckoned.encode(out)?;
        self.reckoned_mode.encode(out)?;
        self.left.encode(out)?;
        self.arrived.encode(out)?;
        self.daily.encode(out)?;
        self.loaded.encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let number = u64::decode(input)?;
        let trader = u32::decode(input)?;
        let plan = Plan::decode(input)?;
        let count = input.length()?;
        let mut groups = Vec::with_capacity(count.min(input.remaining()));
        for _ in 0..count {
            let pop = u32::decode(input)?;
            let people = u64::decode(input)?;
            let buildings = input.length()?;
            let mut by_building = Vec::with_capacity(buildings.min(input.remaining()));
            for _ in 0..buildings {
                by_building.push((u32::decode(input)?, u64::decode(input)?));
            }
            let paid_len = input.length()?;
            let mut paid = Vec::with_capacity(paid_len.min(input.remaining()));
            for _ in 0..paid_len {
                paid.push((u32::decode(input)?, u64::decode(input)?));
            }
            groups.push(Group {
                pop,
                count: people,
                by_building,
                paid,
            });
        }
        let count = input.length()?;
        let mut vehicles = Vec::with_capacity(count.min(input.remaining()));
        for _ in 0..count {
            vehicles.push(ExpeditionVehicle {
                number: u64::decode(input)?,
                kind: u32::decode(input)?,
                worn_hours: decode_ratio(input)?,
            });
        }
        Ok(Self {
            number,
            trader,
            plan,
            groups,
            vehicles,
            cargo: Vec::<u64>::decode(input)?,
            supplies: Vec::<u64>::decode(input)?,
            pantry_centi: u64::decode(input)?,
            gone: bool::decode(input)?,
            fed: Option::<u64>::decode(input)?,
            phase: Phase::decode(input)?,
            traded: bool::decode(input)?,
            leg: u32::decode(input)?,
            done: decode_ratio(input)?,
            pace_pm: u32::decode(input)?,
            limiter: Limiter::decode(input)?,
            reckoned: Vec::<u64>::decode(input)?,
            reckoned_mode: Mode::decode(input)?,
            left: u64::decode(input)?,
            arrived: Option::<u64>::decode(input)?,
            daily: bool::decode(input)?,
            loaded: Vec::<u64>::decode(input)?,
        })
    }
}

impl Expedition {
    /// How many people go with it.
    pub(crate) fn people(&self) -> u64 {
        self.groups
            .iter()
            .fold(0_u64, |sum, group| sum.saturating_add(group.count))
    }

    /// The legs of the way it is on.
    fn way(&self) -> &[Leg] {
        match self.phase {
            Phase::Homeward => &self.plan.back,
            Phase::Outbound | Phase::Waiting => &self.plan.out,
        }
    }

    /// Where it is, in fixed-point map units: on the frozen path of the leg it is on, the point its movement has brought it to (which
    /// does not depend on the pace it holds); at the end of the way out while it waits at the destination, and there too when the way
    /// out has no leg. `None` where the data has no point (a leg with no path), and on the way home with no leg.
    #[must_use]
    pub fn position(&self) -> Option<(Hours, Hours)> {
        let arrival = || {
            self.plan.arrival.map(|[x, y]| {
                (
                    BigRational::from_integer(x.into()),
                    BigRational::from_integer(y.into()),
                )
            })
        };
        match self.phase {
            Phase::Waiting => self
                .plan
                .out
                .last()
                .and_then(|leg| leg.position(&whole(1)))
                .or_else(arrival),
            _ => match self.way().get(self.leg as usize) {
                Some(leg) => leg.position(&self.done),
                None if self.phase == Phase::Outbound => arrival(),
                None => None,
            },
        }
    }

    /// Whether the load it would carry home still makes every leg of the way back with these vehicles and people (11b.3 rule 9): at
    /// most the two kinds of leg need to be tried, since a leg's assignment depends only on what moves it.
    pub(crate) fn assigns_home(&self, rules: &Rules, cargo: &[u64]) -> bool {
        let people = self.people();
        [Mode::Land, Mode::Water]
            .into_iter()
            .filter(|mode| self.plan.back.iter().any(|leg| leg.mode == *mode))
            .all(|mode| {
                assign_leg(rules, &self.vehicles, people, cargo, &self.supplies, mode).is_some()
            })
    }

    /// Its pace on a leg of the given kind with what it holds now (11b.3 rule 7), or `None` if the leg cannot be made.
    fn reckon(&self, rules: &Rules, mode: Mode) -> Option<(u32, Limiter)> {
        self.reckon_with(rules, mode, &self.supplies)
    }

    /// Its pace on a leg of the given kind were `left` the supplies aboard: the one reckoning the simulation makes when it fixes a pace
    /// and the saved-state check makes to see that the pace held is one the simulation gave.
    fn reckon_with(&self, rules: &Rules, mode: Mode, left: &[u64]) -> Option<(u32, Limiter)> {
        let assigned = assign_after(
            rules,
            &self.vehicles,
            self.people(),
            &self.cargo,
            &self.loaded,
            left,
            mode,
        )?;
        Some(pace_of(
            rules,
            &self.vehicles,
            &assigned,
            mode,
            self.gone,
            self.vehicles.is_empty(),
        ))
    }

    /// What its people eat in a day, in hundredths of a person-day, or `None` if the count overflows: what the noon's feeding takes.
    fn hunger_centi(&self, rules: &Rules) -> Option<u64> {
        self.people()
            .checked_mul(rules.expedition.daily_hunger_centi)
    }

    /// Whether nothing to eat is left: no edible good aboard and no pantry. The test the noon's feeding sets the supplies gone by.
    fn out_of_food(&self, rules: &Rules) -> bool {
        self.pantry_centi == 0
            && !rules
                .goods
                .iter()
                .zip(&self.supplies)
                .any(|(item, units)| item.hunger_centi > 0 && *units > 0)
    }

    /// Whether its supplies are gone, as the feedings it has been through make them: it has eaten, wanted food, and has none left.
    fn gone_by_feeding(&self, rules: &Rules) -> bool {
        self.fed.is_some()
            && self.hunger_centi(rules).is_some_and(|need| need > 0)
            && self.out_of_food(rules)
    }

    /// Fixes its pace for the leg it is on, when it leaves home, starts back or runs out of supplies.
    pub(crate) fn set_pace(&mut self, rules: &Rules) -> Result<(), DataError> {
        let mode = self
            .way()
            .get(self.leg as usize)
            .map_or(Mode::Land, |leg| leg.mode);
        match self.reckon(rules, mode) {
            Some((pace, limiter)) => {
                self.pace_pm = pace;
                self.limiter = limiter;
                self.reckoned.clone_from(&self.supplies);
                self.reckoned_mode = mode;
                Ok(())
            }
            // A way of no legs needs no pace: it arrives in its first hour.
            None if self.way().is_empty() => {
                self.pace_pm = rules.expedition.walking_pace_pm;
                self.limiter = Limiter::Nothing;
                self.reckoned.clone_from(&self.supplies);
                self.reckoned_mode = mode;
                Ok(())
            }
            None => Err(missing_data(
                format!("expeditions.{}", self.number),
                "a leg cannot be made with what the expedition holds",
            )),
        }
    }

    /// Turns it round at its destination: it goes back along the way home from its first leg, at the pace of the return load.
    pub(crate) fn start_back(&mut self, rules: &Rules) -> Result<(), DataError> {
        self.phase = Phase::Homeward;
        self.leg = 0;
        self.done = whole(0);
        self.loaded.clone_from(&self.supplies);
        self.set_pace(rules)
    }
}

/// The whole ticks a stretch of travel of `hours` takes: it moves one hour at each tick, so the hours rounded up, and at least one.
fn ticks_of(hours: &Hours) -> Option<u64> {
    u64::try_from(hours.ceil().to_integer())
        .ok()
        .map(|ticks| ticks.max(1))
}

/// The days a hired hand is paid for (r5 3.4: the days of the whole job, a trip of hours counting as a day), worked from the ticks the
/// simulation will run: the expedition first moves at tick `first` (the noon tick itself for a job started in the daily step, the next
/// one for a job started in the hour's step), arrives at the tick its outbound travel ends, trades at the first noon whose cutoff of
/// 11:00 it met, and starts back in that noon's step to move in the same tick (11c.1). The days are the hours from the start to the
/// tick it is home, rounded up, never fewer than one. `None` when a leg cannot be made or a time is not representable.
fn plan_days(
    rules: &Rules,
    vehicles: &[ExpeditionVehicle],
    people: u64,
    plan: &Plan,
    now: u64,
    first: u64,
) -> Option<u64> {
    let paced = |legs: &[Leg], cargo: &[u64]| -> Option<u64> {
        let mut total = whole(0);
        for leg in legs {
            let assigned = assign_leg(rules, vehicles, people, cargo, &plan.supplies, leg.mode)?;
            let (pace, _) = pace_of(
                rules,
                vehicles,
                &assigned,
                leg.mode,
                false,
                vehicles.is_empty(),
            );
            total += leg.hours(pace);
        }
        ticks_of(&total)
    };
    let out = paced(&plan.out, &plan.cargo)?;
    let back = paced(&plan.back, &plan.wanted)?;
    // The tick its outbound travel ends in: movement ticks first, first + 1, ...
    let arrival = first.checked_add(out)?.checked_sub(1)?;
    let (day, hour) = (arrival / 24, arrival % 24);
    let day = if hour <= 11 { day } else { day.checked_add(1)? };
    let noon = day.checked_mul(24)?.checked_add(12)?;
    let home = noon.checked_add(back)?.checked_sub(1)?;
    let elapsed = home.checked_sub(now)?;
    Some(elapsed.div_ceil(24).max(1))
}

/// The most units of one good a plan may take, bring back or carry as supplies: with a good's smallest weight a cart's load is far
/// below it, and it bounds the search for what fits (11c.2 rule 5) to a few million amounts at the most.
pub(crate) const MOST_UNITS: u64 = 1 << 22;

/// Whether every leg of both ways of a plan assigns with what it takes out and what it is to bring back (11c.2 rule 7).
fn plan_assigns(rules: &Rules, vehicles: &[ExpeditionVehicle], people: u64, plan: &Plan) -> bool {
    let ways = [(&plan.out, &plan.cargo), (&plan.back, &plan.wanted)];
    ways.iter().all(|(legs, load)| {
        [Mode::Land, Mode::Water]
            .into_iter()
            .filter(|mode| legs.iter().any(|leg| leg.mode == *mode))
            .all(|mode| assign_leg(rules, vehicles, people, load, &plan.supplies, mode).is_some())
    })
}

/// Starts a trade job whose goods are gathered if its people are at home and willing and its legs assign: hired pops are paid in goods,
/// the people are held, and the job's stockpile becomes an expedition (r5 3.4, 3.6; 11c.2 rule 7). Returns whether it started; a job
/// that cannot start keeps gathering and is given up in time.
#[expect(
    clippy::too_many_lines,
    reason = "The checks, the wages and the new expedition are one all-or-nothing start, kept in one place."
)]
pub(crate) fn start_trade(
    rules: &Rules,
    place: &mut PlaceState,
    counters: &mut TradeCounters,
    index: usize,
    now: u64,
    moves_now: bool,
) -> Result<bool, DataError> {
    let job = &place.trade.jobs[index];
    let JobKind::Trade { plan } = &job.kind else {
        return Ok(false);
    };
    let plan = (**plan).clone();
    let trader = job.pop as usize;
    let entry = || format!("trade.start.{}", job.number);
    let number = counters
        .expeditions
        .checked_add(1)
        .ok_or_else(|| missing_data(entry(), "expedition numbers are used up"))?;
    let mut vehicles = Vec::with_capacity(plan.vehicles.len());
    for wanted in &plan.vehicles {
        let found = place
            .trade
            .vehicles
            .iter()
            .find(|vehicle| vehicle.number == *wanted && vehicle.pop as usize == trader)
            .ok_or_else(|| missing_data(entry(), "the plan names a vehicle the trader has not"))?;
        vehicles.push(ExpeditionVehicle {
            number: found.number,
            kind: found.kind,
            worn_hours: whole(0),
        });
    }
    let people = plan
        .people
        .iter()
        .try_fold(0_u64, |sum, (_, count)| sum.checked_add(*count))
        .ok_or_else(|| missing_data(entry(), "the plan's people overflow"))?;
    if !plan_assigns(rules, &vehicles, people, &plan) {
        return Ok(false);
    }
    // The wage is for the days of the trip out of thirty, a trip of hours counting as a day (r5 3.4), worked from the ticks it will run.
    let first = if moves_now {
        now
    } else {
        now.saturating_add(1)
    };
    let Some(days) = plan_days(rules, &vehicles, people, &plan, now, first) else {
        // A trip that cannot be timed is not started; the job keeps gathering and is given up in time.
        return Ok(false);
    };
    let days = u128::from(days);
    let mut work = place.clone();
    let mut groups = Vec::with_capacity(plan.people.len());
    for (pop, count) in &plan.people {
        let pop = *pop as usize;
        let home = work.pops[pop].workers.saturating_sub(work.trade.held(pop));
        if home < *count {
            return Ok(false);
        }
        let mut paid = Vec::new();
        if pop != trader {
            let wage = u128::from(*count)
                .saturating_mul(u128::from(trade::pop_wage(&work, pop)))
                .saturating_mul(days)
                / 30
                + 1;
            let Some((good, units)) = trade::wage_goods(rules, &work, trader, pop, wage)? else {
                return Ok(false);
            };
            trade::pay_from(&mut work, trader, good, units)?;
            if crate::receive_pop(rules, &mut work.pops[pop], good, units)? != 0 {
                return Err(missing_data(
                    "trade.wage".to_owned(),
                    "a wage the store accepted no longer fits",
                ));
            }
            paid.push((trade::convert(good), units));
        }
        groups.push(Group {
            pop: trade::convert(pop),
            count: *count,
            by_building: trade::hand_buildings(&work, pop, *count),
            paid,
        });
    }
    work.trade.jobs.remove(index);
    let mut expedition = Expedition {
        number,
        trader: trade::convert(trader),
        cargo: plan.cargo.clone(),
        supplies: plan.supplies.clone(),
        loaded: plan.supplies.clone(),
        plan,
        groups,
        vehicles,
        pantry_centi: 0,
        gone: false,
        fed: None,
        phase: Phase::Outbound,
        traded: false,
        leg: 0,
        done: whole(0),
        pace_pm: rules.expedition.walking_pace_pm,
        limiter: Limiter::Nothing,
        reckoned: Vec::new(),
        reckoned_mode: Mode::Land,
        left: now,
        arrived: None,
        daily: moves_now,
    };
    expedition.set_pace(rules)?;
    work.trade.expeditions.push(expedition);
    counters.expeditions = number;
    *place = work;
    Ok(true)
}

/// Counts the noon just spent as a day away for the people of one expedition, at the building each is taken from (r5 3.6); `counted`
/// are the hands a build has just released at this noon, whose day was already counted in the build.
fn count_people(place: &mut PlaceState, groups: &[Group], counted: &mut BTreeMap<(u32, u32), u64>) {
    for group in groups {
        for (building, count) in &group.by_building {
            let key = (*building, group.pop);
            let matched = counted.get_mut(&key).map_or(0, |free| {
                let matched = (*free).min(*count);
                *free -= matched;
                matched
            });
            if *count > matched {
                let days = place.trade.days_away.entry(key).or_insert(0);
                *days = days.saturating_add(*count - matched);
            }
        }
    }
}

/// Counts the noon just spent for every expedition of the place that is away (the daily step, after the day's starts).
pub(crate) fn count_noon_all(place: &mut PlaceState) {
    let groups: Vec<Vec<Group>> = place
        .trade
        .expeditions
        .iter()
        .map(|expedition| expedition.groups.clone())
        .collect();
    for list in &groups {
        count_people(place, list, &mut BTreeMap::new());
    }
}

/// Counts the noon just spent for the expeditions that left in the hour's step after it, numbered above `first_new`.
pub(crate) fn count_noon_for(
    place: &mut PlaceState,
    first_new: u64,
    released: &mut BTreeMap<(u32, u32), u64>,
) {
    let groups: Vec<Vec<Group>> = place
        .trade
        .expeditions
        .iter()
        .filter(|expedition| expedition.number > first_new)
        .map(|expedition| expedition.groups.clone())
        .collect();
    for list in &groups {
        count_people(place, list, released);
    }
}

/// Moves every expedition of the place one hour, arriving and unloading those that come home (11c.1 rule 2).
///
/// Returns the people that came home, as `(building, pop)` to how many: their noon was already counted as a day away in the daily
/// step, so one who leaves again in the same hour is not charged for it twice (the same one-person, one-noon rule as a build's hands).
pub(crate) fn move_expeditions(
    rules: &Rules,
    place: &mut PlaceState,
    counters: &mut TradeCounters,
    now: u64,
) -> Result<BTreeMap<(u32, u32), u64>, DataError> {
    let mut released = BTreeMap::new();
    let mut index = 0;
    while index < place.trade.expeditions.len() {
        if step(rules, &mut place.trade.expeditions[index], now)? {
            let expedition = place.trade.expeditions.remove(index);
            for group in &expedition.groups {
                for (building, count) in &group.by_building {
                    let held: &mut u64 = released.entry((*building, group.pop)).or_insert(0);
                    *held = held.saturating_add(*count);
                }
            }
            unload(rules, place, counters, &expedition, now)?;
        } else {
            index += 1;
        }
    }
    Ok(released)
}

/// One hour of one expedition's travel. Returns whether it has come home.
fn step(rules: &Rules, expedition: &mut Expedition, now: u64) -> Result<bool, DataError> {
    if expedition.phase == Phase::Waiting {
        return Ok(false);
    }
    let people = expedition.people();
    let outbound = expedition.phase == Phase::Outbound;
    let started = expedition
        .way()
        .get(expedition.leg as usize)
        .map(|leg| leg.mode);
    let mut failed = false;
    // The hours spent on land and on water since the wear was last settled: what each vehicle that moves in a mode wears by (T23). The
    // wear of a leg is settled before the next leg's pace is reckoned, so that pace is of the supplies actually left.
    let moved = std::cell::RefCell::new((whole(0), whole(0)));
    let mut wear_failed: Option<DataError> = None;
    let (arrived, leg) = {
        let Expedition {
            plan,
            leg,
            done,
            pace_pm,
            limiter,
            reckoned,
            reckoned_mode,
            vehicles,
            cargo,
            supplies,
            loaded,
            gone,
            ..
        } = &mut *expedition;
        let legs = if outbound { &plan.out } else { &plan.back };
        let mut current = started;
        let mut index = *leg as usize;
        let arrived = advance(
            legs,
            &mut index,
            done,
            whole(1),
            |next| {
                let pending = moved.take();
                if pending != (whole(0), whole(0))
                    && let Err(error) = wear(rules, vehicles, supplies, &pending)
                {
                    wear_failed.get_or_insert(error);
                }
                let mode = legs[next].mode;
                if current != Some(mode) {
                    // The vehicles that move change at a transfer (T15): the pace is reckoned again with what is aboard.
                    current = Some(mode);
                    match assign_after(rules, vehicles, people, cargo, loaded, supplies, mode) {
                        Some(assigned) => {
                            let (pace, set_by) = pace_of(
                                rules,
                                vehicles,
                                &assigned,
                                mode,
                                *gone,
                                vehicles.is_empty(),
                            );
                            *pace_pm = pace;
                            *limiter = set_by;
                            reckoned.clone_from(supplies);
                            *reckoned_mode = mode;
                        }
                        None => failed = true,
                    }
                }
                *pace_pm
            },
            |at, hours| match legs[at].mode {
                Mode::Land => moved.borrow_mut().0 += hours,
                Mode::Water => moved.borrow_mut().1 += hours,
            },
        );
        (arrived, index)
    };
    if failed {
        return Err(missing_data(
            format!("expeditions.{}", expedition.number),
            "a leg cannot be made with what the expedition holds",
        ));
    }
    if let Some(error) = wear_failed {
        return Err(error);
    }
    expedition.leg = u32::try_from(leg).unwrap_or(u32::MAX);
    wear(
        rules,
        &mut expedition.vehicles,
        &mut expedition.supplies,
        &moved.take(),
    )?;
    if !arrived {
        return Ok(false);
    }
    if outbound {
        expedition.phase = Phase::Waiting;
        expedition.arrived = Some(now);
        expedition.leg = u32::try_from(expedition.plan.out.len()).unwrap_or(u32::MAX);
        expedition.done = whole(0);
        return Ok(false);
    }
    Ok(true)
}

/// Wears the vehicles by the time they moved this tick: each uses up the units its kind's rules give for its wear, from the supplies as far
/// as there are any, for every `wear_hours` it has moved (T23). A vehicle moves for the hours spent on legs of a mode it has a pace or a way in, so the
/// tick that crosses a transfer wears each for its own part of the hour, and the hours short of a unit are kept exactly.
fn wear(
    rules: &Rules,
    vehicles: &mut [ExpeditionVehicle],
    supplies: &mut [u64],
    moved: &(Hours, Hours),
) -> Result<(), DataError> {
    let unit = whole(rules.expedition.wear_hours);
    for vehicle in vehicles {
        let kind = rules.vehicles.get(vehicle.kind as usize).ok_or_else(|| {
            missing_data(
                "expeditions.vehicles".to_owned(),
                "a vehicle kind is missing",
            )
        })?;
        if kind.land_pace_pm.is_some() {
            vehicle.worn_hours += &moved.0;
        }
        if kind.water.is_some() {
            vehicle.worn_hours += &moved.1;
        }
        while vehicle.worn_hours >= unit {
            vehicle.worn_hours -= &unit;
            let good = trade::good_index(rules, &kind.wear.good)?;
            if let Some(supply) = supplies.get_mut(good) {
                *supply = supply.saturating_sub(kind.wear.units);
            }
        }
    }
    Ok(())
}

/// Records units lost because nothing at the place could take them, on the place's and the pop's counters.
fn record_loss(
    rules: &Rules,
    place: &mut PlaceState,
    pop: usize,
    good: usize,
    units: u64,
) -> Result<(), DataError> {
    if units == 0 {
        return Ok(());
    }
    place.counters.current.losses = place
        .counters
        .current
        .losses
        .checked_add(units)
        .ok_or_else(|| missing_data("counters.losses".to_owned(), "place loss counter overflow"))?;
    crate::add_counter_units(
        &mut place.counters.current.lost,
        rules.goods.len(),
        good,
        units,
        "counters.lost",
    )?;
    let counters = &mut place.pops[pop].counters.current;
    counters.losses = counters
        .losses
        .checked_add(units)
        .ok_or_else(|| missing_data("counters.losses".to_owned(), "pop loss counter overflow"))?;
    Ok(())
}

/// Unloads an expedition that has come home, in the hour it arrives (r134): what it brings goes to its trader's store and accounts by
/// the ordinary rules of receipt and, beyond them, the general overflow rule (CK-403); a live animal goes where an arriving head goes
/// (11b.1 rule 1); its people are home and its vehicles free, and it is gone.
fn unload(
    rules: &Rules,
    place: &mut PlaceState,
    counters: &mut TradeCounters,
    expedition: &Expedition,
    now: u64,
) -> Result<(), DataError> {
    let trader = expedition.trader as usize;
    let mut held = vec![0_u64; rules.herd_species.len()];
    for good in 0..rules.goods.len() {
        let units = expedition.cargo[good]
            .checked_add(expedition.supplies[good])
            .ok_or_else(|| missing_data("trade.unload".to_owned(), "unloaded units overflow"))?;
        if units == 0 {
            continue;
        }
        if let Some(species) = species_of(rules, good) {
            let left = trade::place_head(rules, place, trader, species, units)?;
            held[species] = left;
            continue;
        }
        let mut left = crate::receive_pop(rules, &mut place.pops[trader], good, units)?;
        left = crate::route_pop_surplus(
            rules,
            &mut place.pops,
            &mut place.buildings,
            trader,
            good,
            left,
        )?;
        record_loss(rules, place, trader, good, left)?;
    }
    // A head no job wanted and no herd had room for stays with the trader on offer at the place, slaughtered after the set time.
    if held.iter().any(|head| *head > 0) {
        let number = counters
            .jobs
            .checked_add(1)
            .ok_or_else(|| missing_data("trade.jobs".to_owned(), "job numbers are used up"))?;
        counters.jobs = number;
        place.trade.jobs.push(Job {
            number,
            pop: expedition.trader,
            kind: JobKind::Held {
                origin: held.clone(),
            },
            made: now,
            need_goods: vec![0; rules.goods.len()],
            need_animals: vec![0; rules.herd_species.len()],
            stock: vec![0; rules.goods.len()],
            animals: held,
            hands: None,
            given_up: Some(now),
        });
    }
    for vehicle in &expedition.vehicles {
        if let Some(home) = place
            .trade
            .vehicles
            .iter_mut()
            .find(|home| home.number == vehicle.number)
        {
            home.free = true;
        }
    }
    Ok(())
}

/// The hour of day the daily step runs at, and so the noon each expedition present is fed at.
const NOON: u64 = 12;

/// The first noon at or after `hour`.
fn noon_from(hour: u64) -> u64 {
    let day = hour - hour % 24;
    if hour % 24 <= NOON {
        day + NOON
    } else {
        day + 24 + NOON
    }
}

/// The latest noon whose daily step has run when the saved clock reads `now` (a saved clock is the hour whose systems have not run yet).
fn last_noon_before(now: u64) -> Option<u64> {
    now.checked_sub(NOON + 1)
        .map(|from| from - from % 24 + NOON)
}

/// The first noon whose feeding an expedition that left at `left` is there for: the one it left at when it left in that noon's daily
/// step (which starts it, and feeds it, in that very tick), otherwise the next, since the hour's step starts jobs after the daily one.
fn first_fed_noon(left: u64, daily: bool) -> u64 {
    if daily { left } else { noon_from(left + 1) }
}

/// Each expedition eats from its supplies at noon (3.8 step 8): a person-day each, whole goods opened as the pop's own eating does and
/// the part of a day not eaten kept for the next. When nothing to eat is left its supplies are gone, and it goes at half pace (T25).
pub(crate) fn feed(rules: &Rules, place: &mut PlaceState, now: u64) -> Result<(), DataError> {
    for expedition in &mut place.trade.expeditions {
        expedition.fed = Some(now);
        let need = expedition
            .hunger_centi(rules)
            .ok_or_else(|| missing_data("trade.feed".to_owned(), "daily hunger overflow"))?;
        let mut pantry = expedition.pantry_centi;
        for (good, item) in rules.goods.iter().enumerate() {
            if pantry >= need {
                break;
            }
            if item.hunger_centi == 0 {
                continue;
            }
            let hunger = u64::from(item.hunger_centi);
            let units = (need - pantry)
                .div_ceil(hunger)
                .min(expedition.supplies[good]);
            expedition.supplies[good] -= units;
            pantry += units * hunger;
        }
        pantry -= pantry.min(need);
        expedition.pantry_centi = pantry;
        if !expedition.gone && expedition.gone_by_feeding(rules) {
            expedition.gone = true;
            if expedition.phase != Phase::Waiting {
                expedition.set_pace(rules)?;
            }
        }
    }
    Ok(())
}

/// The most people one expedition holds: far above the few a plan sends, and what keeps a crew's carriers a small table.
pub(crate) const MOST_PEOPLE: u64 = 1024;
/// The most stretches one leg is made of.
const MOST_TERMS: usize = 1024;

fn gram_sum(values: &[u64]) -> bool {
    values
        .iter()
        .try_fold(0_u64, |sum, value| sum.checked_add(*value))
        .is_some()
}

/// What a trade job for a plan needs, by good and by herd species: the cargo and the supplies of a good, and the animals of a species
/// as the cargo of its animal good. `None` when any amount overflows, so a plan whose needs cannot be counted is refused before it is
/// ordered or accepted from a save.
pub(crate) fn job_needs(rules: &Rules, plan: &Plan) -> Option<(Vec<u64>, Vec<u64>)> {
    let mut need_goods = vec![0_u64; rules.goods.len()];
    let mut need_animals = vec![0_u64; rules.herd_species.len()];
    for (good, (cargo, supplies)) in plan.cargo.iter().zip(&plan.supplies).enumerate() {
        match species_of(rules, good) {
            Some(species) => need_animals[species] = need_animals[species].checked_add(*cargo)?,
            None => need_goods[good] = cargo.checked_add(*supplies)?,
        }
    }
    Some((need_goods, need_animals))
}

/// The units of a good that slaughtering `animals` (by herd species) yields, or `None` if the count overflows (11b.1: a slaughtered head
/// yields its meat and hide).
pub(crate) fn slaughter_yield(rules: &Rules, animals: &[u64], good: usize) -> Option<u64> {
    let mut total = 0_u64;
    for (species, head) in animals.iter().enumerate() {
        for yielded in &rules.herd_species.get(species)?.yields {
            if trade::good_index(rules, &yielded.good).ok()? == good {
                total = total.checked_add(head.checked_mul(yielded.units)?)?;
            }
        }
    }
    Some(total)
}

/// Refuses a plan whose shape no game could have made (11b.4): its vectors, people, vehicles and legs. The place's own vehicles are
/// read for the trader's; `place_index` is the home place.
fn validate_plan(
    rules: &Rules,
    state: &crate::EconomyState,
    place_index: usize,
    trader: usize,
    plan: &Plan,
) -> Result<(), &'static str> {
    let place = &state.places[place_index];
    let goods = rules.goods.len();
    if [plan.cargo.len(), plan.wanted.len(), plan.supplies.len()]
        .iter()
        .any(|len| *len != goods)
    {
        return Err("its goods do not match the rules");
    }
    if !gram_sum(&plan.cargo)
        || !gram_sum(&plan.wanted)
        || !gram_sum(&plan.supplies)
        || job_needs(rules, plan).is_none()
    {
        return Err("its amounts overflow");
    }
    if plan
        .cargo
        .iter()
        .zip(&plan.wanted)
        .any(|(cargo, wanted)| *cargo > 0 && *wanted > 0)
    {
        return Err("it takes a good it is to bring back");
    }
    if (0..goods).any(|good| species_of(rules, good).is_some() && plan.supplies[good] != 0) {
        return Err("it takes a live animal as supplies");
    }
    if plan.destination as usize >= state.places.len() || plan.destination as usize == place_index {
        return Err("its destination is not another place of the economy");
    }
    if plan.people.is_empty() {
        return Err("it sends nobody");
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut people = 0_u64;
    for (pop, count) in &plan.people {
        people = people.saturating_add(*count);
        if *pop as usize >= place.pops.len() || *count == 0 || !seen.insert(*pop) {
            return Err("its people name a pop that does not exist, none, or a pop twice");
        }
    }
    if people > MOST_PEOPLE {
        return Err("it sends more people than an expedition holds");
    }
    let mut numbers = std::collections::BTreeSet::new();
    for number in &plan.vehicles {
        let owned = place
            .trade
            .vehicles
            .iter()
            .any(|vehicle| vehicle.number == *number && vehicle.pop as usize == trader);
        if !owned || !numbers.insert(*number) {
            return Err("its vehicles are not the trader's, or one is named twice");
        }
    }
    if [&plan.cargo, &plan.wanted, &plan.supplies]
        .iter()
        .any(|amounts| amounts.iter().any(|units| *units > MOST_UNITS))
    {
        return Err("it takes or brings more of a good than any load holds");
    }
    for legs in [&plan.out, &plan.back] {
        if legs.len() > MOST_LEGS
            || legs
                .iter()
                .any(|leg| leg.terms.len() > MOST_TERMS || leg.path.len() > MOST_LEG_PATH)
        {
            return Err("its itinerary is longer than any a game has");
        }
        if legs.iter().any(|leg| {
            !leg.along.is_empty()
                && (leg.along.len() != leg.path.len()
                    || leg.along[0] != 0
                    || leg.along.windows(2).any(|pair| pair[0] > pair[1]))
        }) {
            return Err("a leg's path points are not in order along it");
        }
    }
    // The slowest the trip can go is at a pace of 1: no time in it may be beyond what the calendar holds, so none is unrepresentable.
    let calendar = whole(trade::last_hour());
    let slowest = plan
        .out
        .iter()
        .chain(&plan.back)
        .fold(whole(0), |sum, leg| sum + leg.hours(1));
    if slowest > calendar {
        return Err("its trip takes longer than the calendar holds");
    }
    Ok(())
}

/// Checks the shape of a trade job or a held job (CK-432), for the saved-state check.
#[expect(
    clippy::too_many_lines,
    reason = "One check for each kind of job and each of its saved fields, in the order they are saved."
)]
pub(crate) fn validate_other_job(
    rules: &Rules,
    state: &crate::EconomyState,
    place_index: usize,
    job: &Job,
    now: u64,
) -> Result<(), DataError> {
    let place = &state.places[place_index];
    let refuse = |problem: &str| {
        missing_data(
            format!("places.{}.trade.jobs.{}", place.id, job.number),
            problem,
        )
    };
    let goods = rules.goods.len();
    let species = rules.herd_species.len();
    if job.pop as usize >= place.pops.len() {
        return Err(refuse("names a pop that does not exist"));
    }
    if [job.need_goods.len(), job.stock.len()]
        .iter()
        .any(|len| *len != goods)
        || [job.need_animals.len(), job.animals.len()]
            .iter()
            .any(|len| *len != species)
    {
        return Err(refuse("goods and animals do not match the rules"));
    }
    let last = trade::last_hour();
    if job.hands.is_some()
        || job.made > last
        || job
            .given_up
            .is_some_and(|hour| hour < job.made || hour > last)
    {
        return Err(refuse("holds hands, or has an hour outside the calendar"));
    }
    match &job.kind {
        JobKind::Build { .. } => Err(refuse("is a build")),
        JobKind::Held { origin } => {
            // A head on offer (before the set time since it was put on offer) or what its slaughter yielded that the trader's store
            // could not yet take (from that time on), never both: the heads are at most those put on offer, the yield at most what
            // those heads give, counted without overflow.
            let slaughtered_at = job
                .given_up
                .map(|since| since.saturating_add(rules.held_animal_hours));
            let on_offer = job.animals.iter().any(|head| *head > 0);
            let yielded = job.stock.iter().any(|units| *units > 0);
            let heads_ok = origin.len() == species
                && job
                    .animals
                    .iter()
                    .zip(origin)
                    .all(|(head, put)| head <= put);
            let yield_ok = (0..goods).all(|good| {
                slaughter_yield(rules, origin, good).is_some_and(|most| job.stock[good] <= most)
            });
            // The clock of a saved state is the hour whose systems have not yet run: the slaughter at hour `at` is still to come at a
            // clock of `at`, and has run at a clock after it.
            let timed =
                slaughtered_at.is_some_and(|at| (!on_offer || now <= at) && (!yielded || now > at));
            if job.given_up.is_none()
                || job.need_goods.iter().any(|units| *units != 0)
                || job.need_animals.iter().any(|head| *head != 0)
                || (on_offer && yielded)
                || !heads_ok
                || !yield_ok
                || !timed
            {
                return Err(refuse(
                    "is held on offer but needs goods, holds more than its heads or their yield, or out of its time",
                ));
            }
            Ok(())
        }
        JobKind::Trade { plan } => {
            validate_plan(rules, state, place_index, job.pop as usize, plan).map_err(&refuse)?;
            let Some((need_goods, need_animals)) = job_needs(rules, plan) else {
                return Err(refuse("its amounts overflow"));
            };
            if job.need_goods != need_goods || job.need_animals != need_animals {
                return Err(refuse("needs what its plan does not take"));
            }
            if job.given_up.is_none() {
                if job
                    .stock
                    .iter()
                    .zip(&job.need_goods)
                    .any(|(have, need)| have > need)
                    || job
                        .animals
                        .iter()
                        .zip(&job.need_animals)
                        .any(|(have, need)| have > need)
                {
                    return Err(refuse("holds more than it needs"));
                }
            } else {
                // A given-up job holds at most what it gathered, with the meat and hide of the animals it slaughtered after the set
                // time on top: the most a reachable game leaves in it, counted without overflow.
                let within = (0..goods).all(|good| {
                    slaughter_yield(rules, &job.need_animals, good)
                        .and_then(|yielded| yielded.checked_add(job.need_goods[good]))
                        .is_some_and(|most| job.stock[good] <= most)
                });
                let slaughtered = job
                    .given_up
                    .is_some_and(|since| now > since.saturating_add(rules.held_animal_hours));
                let beyond_need = job
                    .stock
                    .iter()
                    .zip(&job.need_goods)
                    .any(|(have, need)| have > need);
                if !within
                    || (beyond_need && !slaughtered)
                    || job
                        .animals
                        .iter()
                        .zip(&job.need_animals)
                        .any(|(have, need)| have > need)
                {
                    return Err(refuse(
                        "holds more than it could have gathered and slaughtered",
                    ));
                }
            }
            Ok(())
        }
    }
}

/// Refuses the expeditions of a place, and the vehicles' reservations, that no reachable game could have saved (CK-432): every index
/// names something, every count is bounded, the position is inside the leg list, the phase and the hours agree, the people and vehicles
/// are the plan's, and what is aboard is what the legs ahead can still carry.
#[expect(
    clippy::too_many_lines,
    reason = "One check for each saved field's domain, in the order the fields are saved, so each is visible beside the others."
)]
pub(crate) fn validate_expeditions(
    rules: &Rules,
    state: &crate::EconomyState,
    place_index: usize,
    numbers: &mut std::collections::BTreeSet<u64>,
    now: u64,
) -> Result<(), DataError> {
    let place = &state.places[place_index];
    // No hour of a trip is later than the clock the state was saved at.
    let last = trade::last_hour().min(now);
    let (goods, pops, buildings) = (rules.goods.len(), place.pops.len(), place.buildings.len());
    let mut previous = 0;
    let mut reserved = std::collections::BTreeSet::new();
    for expedition in &place.trade.expeditions {
        let refuse = |problem: &str| {
            missing_data(
                format!(
                    "places.{}.trade.expeditions.{}",
                    place.id, expedition.number
                ),
                problem,
            )
        };
        if expedition.number <= previous
            || expedition.number > state.trade.expeditions
            || !numbers.insert(expedition.number)
        {
            return Err(refuse("numbers are not unique, increasing and issued"));
        }
        previous = expedition.number;
        let plan = &expedition.plan;
        if expedition.trader as usize >= pops {
            return Err(refuse("names a pop that does not exist"));
        }
        validate_plan(rules, state, place_index, expedition.trader as usize, plan)
            .map_err(&refuse)?;
        let vehicle_list: Vec<(u64, u32)> = plan
            .vehicles
            .iter()
            .filter_map(|number| {
                place
                    .trade
                    .vehicles
                    .iter()
                    .find(|vehicle| vehicle.number == *number)
                    .map(|vehicle| (vehicle.number, vehicle.kind))
            })
            .collect();
        if expedition.vehicles.len() != vehicle_list.len()
            || expedition
                .vehicles
                .iter()
                .zip(&vehicle_list)
                .any(|(held, (number, kind))| {
                    held.number != *number
                        || held.kind != *kind
                        || held.worn_hours >= whole(rules.expedition.wear_hours)
                })
        {
            return Err(refuse("its vehicles are not its plan's"));
        }
        for vehicle in &expedition.vehicles {
            if !reserved.insert(vehicle.number) {
                return Err(refuse("holds a vehicle another expedition holds"));
            }
        }
        // The people are the plan's, by pop, each group's absence charged to buildings that exist.
        if expedition.groups.len() != plan.people.len()
            || expedition
                .groups
                .iter()
                .zip(&plan.people)
                .any(|(group, (pop, count))| group.pop != *pop || group.count != *count)
        {
            return Err(refuse("its people are not its plan's"));
        }
        for group in &expedition.groups {
            let held = group
                .by_building
                .iter()
                .try_fold(0_u64, |sum, (_, count)| sum.checked_add(*count));
            let paid_ok = (group.pop == expedition.trader) == group.paid.is_empty();
            if held.is_none_or(|held| held > group.count)
                || group
                    .by_building
                    .windows(2)
                    .any(|pair| pair[0].0 >= pair[1].0)
                || group
                    .by_building
                    .iter()
                    .any(|(building, count)| *building as usize >= buildings || *count == 0)
                || group
                    .paid
                    .iter()
                    .any(|(good, units)| *good as usize >= goods || *units == 0)
                || !paid_ok
            {
                return Err(refuse(
                    "holds people whose buildings do not exist, add up, or whose wage does not fit",
                ));
            }
        }
        if expedition.cargo.len() != goods || expedition.supplies.len() != goods {
            return Err(refuse("its goods do not match the rules"));
        }
        if !gram_sum(&expedition.cargo)
            || !gram_sum(&expedition.supplies)
            || expedition
                .supplies
                .iter()
                .zip(&plan.supplies)
                .any(|(aboard, planned)| aboard > planned)
        {
            return Err(refuse(
                "holds more supplies than its plan took, or amounts that overflow",
            ));
        }
        if expedition.cargo.iter().enumerate().any(|(good, units)| {
            species_of(rules, good).is_some() && *units > 0 && expedition.supplies[good] > 0
        }) {
            return Err(refuse("holds a live animal as supplies"));
        }
        let way = expedition.way();
        let phase_ok = match expedition.phase {
            // A moving expedition is on a leg of its way: crossing the last one ends the way (it waits, or comes home and is gone),
            // so only a way of no legs is ever stood at its end.
            Phase::Outbound => {
                expedition.arrived.is_none()
                    && !expedition.traded
                    && ((expedition.leg as usize) < way.len()
                        || (way.is_empty() && expedition.leg == 0))
                    && expedition.cargo == plan.cargo
            }
            Phase::Waiting => {
                expedition
                    .arrived
                    .is_some_and(|hour| hour >= expedition.left && hour <= last)
                    && !expedition.traded
                    && expedition.leg as usize == way.len()
                    && expedition.cargo == plan.cargo
            }
            Phase::Homeward => {
                expedition
                    .arrived
                    .is_some_and(|hour| hour >= expedition.left && hour <= last)
                    && expedition.traded
                    && ((expedition.leg as usize) < way.len()
                        || (way.is_empty() && expedition.leg == 0))
            }
        };
        let in_leg = expedition.done >= whole(0) && expedition.done < whole(1);
        let at_end = expedition.leg as usize >= way.len();
        if !phase_ok || !in_leg || (at_end && expedition.done != whole(0)) {
            return Err(refuse("its phase, position and hours do not agree"));
        }
        if expedition.left > last || expedition.pace_pm == 0 {
            return Err(refuse(
                "has an hour outside the calendar or the saved clock, or no pace",
            ));
        }
        let carried = if expedition.phase == Phase::Homeward {
            &expedition.cargo
        } else {
            &plan.cargo
        };
        // What can set its pace is a member it has: a moving vehicle of its own kinds, a species it carries, people on foot or
        // walking beside a vehicle; nothing only where it has no leg to travel.
        let limiter_ok = match expedition.limiter {
            Limiter::Vehicle(kind) => expedition
                .vehicles
                .iter()
                .any(|vehicle| vehicle.kind == kind),
            Limiter::Species(species) => carried.iter().enumerate().any(|(good, units)| {
                *units > 0 && species_of(rules, good) == Some(species as usize)
            }),
            Limiter::Walkers => true,
            Limiter::Nothing => way.is_empty(),
        };
        let people = expedition.people();
        let most_pantry = people
            .saturating_mul(rules.expedition.daily_hunger_centi)
            .saturating_add(
                rules
                    .goods
                    .iter()
                    .map(|good| u64::from(good.hunger_centi))
                    .max()
                    .unwrap_or(0),
            );
        // What the feedings it has been through make of it, from the same helpers the noon's feeding uses: it has eaten at every noon
        // since the first it was there for, so the last is the latest noon the saved clock has run, and its supplies are gone exactly
        // when the feeding would have set them so. A state saved with no clock to check against (the last hour of the calendar) is
        // checked for consistency only.
        let clock_known = now < trade::last_hour();
        let first_noon = first_fed_noon(expedition.left, expedition.daily);
        let fed_ok = !clock_known
            || expedition.fed == last_noon_before(now).filter(|noon| *noon >= first_noon);
        // A departure in a noon's daily step is at noon; and at the very next clock it has moved if it left in that step and has not
        // if it left in the hour's step (which moves the expeditions before it starts the jobs).
        let moved = expedition.leg != 0 || expedition.done != whole(0);
        let departure_ok = (!expedition.daily || expedition.left % 24 == NOON)
            && (now != expedition.left + 1
                || expedition.phase != Phase::Outbound
                || expedition.daily == moved);
        if !fed_ok || !departure_ok || expedition.gone != expedition.gone_by_feeding(rules) {
            return Err(refuse(
                "its supplies are gone, or not, against what is aboard and when it last ate",
            ));
        }
        if !limiter_ok || expedition.pantry_centi > most_pantry {
            return Err(refuse(
                "names a limiter that does not exist or holds too much of a day's food",
            ));
        }
        // The pace held is the one the load at its last reckoning gives, for the kind of leg it is on: the supplies then aboard (which only
        // fall afterwards) and the cargo, on the vehicles and people. Anything else is not a pace the simulation produces.
        // The load was packed as `loaded` (all the plan took when it is still on the way out, what it set out with for home after), and
        // the supplies only fall from the packing to the last reckoning and from it to now.
        if expedition.reckoned.len() != goods
            || expedition.loaded.len() != goods
            || expedition
                .loaded
                .iter()
                .zip(&expedition.reckoned)
                .zip(&expedition.supplies)
                .zip(&plan.supplies)
                .any(|(((packed, then), now), took)| packed < then || then < now || packed > took)
            || (expedition.phase != Phase::Homeward && expedition.loaded != plan.supplies)
        {
            return Err(refuse("its pace was reckoned from supplies it never held"));
        }
        if let Some(leg) = way.get(expedition.leg as usize) {
            let reckoned =
                expedition.reckon_with(rules, expedition.reckoned_mode, &expedition.reckoned);
            if expedition.reckoned_mode != leg.mode
                || reckoned != Some((expedition.pace_pm, expedition.limiter))
            {
                return Err(refuse("holds a pace its load and crew cannot give"));
            }
        }
        // What it was loaded with can make every kind of leg ahead of it, and what is aboard now is part of that load.
        let ahead = &way[(expedition.leg as usize).min(way.len())..];
        let assigns = [Mode::Land, Mode::Water]
            .into_iter()
            .filter(|mode| ahead.iter().any(|leg| leg.mode == *mode))
            .all(|mode| {
                assign_leg(
                    rules,
                    &expedition.vehicles,
                    people,
                    carried,
                    &expedition.loaded,
                    mode,
                )
                .is_some()
            });
        if !assigns {
            return Err(refuse("holds a load its legs ahead cannot carry"));
        }
    }
    // A vehicle is free exactly when no job that is not given up and no expedition holds it (11b.2 rule 5).
    for job in &place.trade.jobs {
        if let JobKind::Trade { plan } = &job.kind
            && job.given_up.is_none()
        {
            for number in &plan.vehicles {
                if !reserved.insert(*number) {
                    return Err(missing_data(
                        format!("places.{}.trade.jobs.{}", place.id, job.number),
                        "reserves a vehicle another job or expedition holds",
                    ));
                }
            }
        }
    }
    for vehicle in &place.trade.vehicles {
        if vehicle.free == reserved.contains(&vehicle.number) {
            return Err(missing_data(
                format!("places.{}.trade.vehicles.{}", place.id, vehicle.number),
                "is free while a job holds it, or held by no job",
            ));
        }
    }
    Ok(())
}

impl crate::EconomyState {
    /// Orders a trade: a job of the pop `pop` of the place `place` to carry out `plan`, made at game hour `now`. The plan is given (by
    /// a test, and by part D's choosing); the job reserves the plan's vehicles from now on (11b.2 rule 5) and takes at once whatever its
    /// trader holds of what it needs (r5 3.4). Returns the job's number.
    ///
    /// # Errors
    /// An unknown place or pop, a plan whose shape is wrong (see the saved-state check), or a vehicle that is not free, leaving the
    /// state unchanged.
    pub fn order_trade(
        &mut self,
        rules: &Rules,
        place: usize,
        pop: usize,
        plan: Plan,
        now: u64,
    ) -> Result<u64, DataError> {
        let entry = || "trade.order_trade".to_owned();
        if now > trade::last_hour() {
            return Err(missing_data(entry(), "the hour is outside the calendar"));
        }
        let number = self
            .trade
            .jobs
            .checked_add(1)
            .ok_or_else(|| missing_data(entry(), "job numbers are used up"))?;
        if place >= self.places.len() || pop >= self.places[place].pops.len() {
            return Err(missing_data(entry(), "place or pop is outside the economy"));
        }
        validate_plan(rules, self, place, pop, &plan)
            .map_err(|problem| missing_data(entry(), problem))?;
        let state = &mut self.places[place];
        if plan.vehicles.iter().any(|number| {
            state
                .trade
                .vehicles
                .iter()
                .any(|vehicle| vehicle.number == *number && !vehicle.free)
        }) {
            return Err(missing_data(entry(), "a vehicle of the plan is not free"));
        }
        let (need_goods, need_animals) = job_needs(rules, &plan)
            .ok_or_else(|| missing_data(entry(), "the plan's amounts overflow"))?;
        for vehicle in &mut state.trade.vehicles {
            if plan.vehicles.contains(&vehicle.number) {
                vehicle.free = false;
            }
        }
        let reserved = plan.vehicles.clone();
        state.trade.jobs.push(Job {
            number,
            pop: trade::convert(pop),
            kind: JobKind::Trade {
                plan: Box::new(plan),
            },
            made: now,
            need_goods,
            need_animals,
            stock: vec![0; rules.goods.len()],
            animals: vec![0; rules.herd_species.len()],
            hands: None,
            given_up: None,
        });
        let index = state.trade.jobs.len() - 1;
        let prices = state.prices_milli.clone();
        if let Err(error) = trade::fill_job(rules, state, &prices, index) {
            state.trade.jobs.pop();
            for vehicle in &mut state.trade.vehicles {
                if reserved.contains(&vehicle.number) {
                    vehicle.free = true;
                }
            }
            return Err(error);
        }
        self.trade.jobs = number;
        Ok(number)
    }
}

#[cfg(test)]
mod fit_tests {
    use super::{Item, Room, first_fit};
    use crate::swap::max_fitting;

    /// First-fit places the items in order, each pile into the first carrier with room; whether `x_units` of the first item fit.
    fn assigns(x_units: u64) -> bool {
        // Carriers' rooms are (space, weight): (3, 13), (10, 14), (2, 18); the items, by decreasing space and then weight, are `x` (2, 5),
        // two (1, 9), one (1, 5) and three (1, 1).
        let mut rooms = [
            Room {
                space_ml: 3,
                weight_g: 13,
            },
            Room {
                space_ml: 10,
                weight_g: 14,
            },
            Room {
                space_ml: 2,
                weight_g: 18,
            },
        ];
        let items = [(2_u128, 5_u128, x_units), (1, 9, 2), (1, 5, 1), (1, 1, 3)];
        items
            .iter()
            .enumerate()
            .all(|(key, (space, weight, units))| {
                first_fit(
                    &mut rooms,
                    &Item {
                        rank: 2,
                        space_ml: *space,
                        weight_g: *weight,
                        key,
                        units: *units,
                    },
                )
                .is_some()
            })
    }

    #[test]
    fn first_fit_can_take_three_units_and_refuse_two_so_the_search_finds_the_larger_valid_amount() {
        // The reviewer's witness: one unit fits, two do not (the heavy items split between the second and third carriers, leaving room
        // for one of the last three), and three do (both heavy items go into the third, leaving the second room for the last three).
        assert!(assigns(1));
        assert!(!assigns(2));
        assert!(assigns(3));
        assert_eq!(
            max_fitting(3, &|units| assigns(u64::try_from(units).unwrap())),
            3
        );
        // Whatever the search returns passes the test it was given, and below the largest valid amount it is never beaten.
        for most in 0..=3_u128 {
            let found = max_fitting(most, &|units| assigns(u64::try_from(units).unwrap()));
            assert!(assigns(u64::try_from(found).unwrap()));
            let best = (0..=most)
                .rev()
                .find(|units| assigns(u64::try_from(*units).unwrap()))
                .unwrap();
            assert_eq!(found, best, "most {most}");
        }
    }
}
