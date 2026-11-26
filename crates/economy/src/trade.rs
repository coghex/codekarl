//! Trade, part B: vehicles and jobs (CK-431; trade spec section 6 with rules 11b.2, 11b.1 and 11c.1).
//!
//! A **vehicle** is a kind from the rules, the pop that owns it and whether it is free. A **job** is a temporary one-off building:
//! it has its own stockpile and its own animals, it wants goods the way a pop does, it hires hands when its goods are gathered, and
//! it ends in a vehicle for its trader (a build) or is given up when it was not started in time. This slice builds vehicles from a
//! given build order; deciding when to build is part D's, and the trips are part C's.
//!
//! The order of a game hour is the spec's (11c.1): at noon the economy's daily step first, with the jobs' take before the exchange
//! and the starts after it, before the day's eating; then, every hour, builds that are done complete, ready jobs start in their
//! numbers' order, expired jobs are given up and what a given-up job still holds goes back.

use std::collections::BTreeMap;

use ck_sim::{Canonical, CanonicalError, Date, GameTime, Reader, encode_length};
use serde::Deserialize;

use crate::expedition::{Expedition, Plan};
use crate::{
    BuildingState, DataError, EconomyState, PlaceState, Rules, building_market_sells, herd_levels,
    herd_maximums, herd_standard_level, missing_data, pop_market_holding, pop_target_units,
    receive_pop, resolve_owner_shares, swap::wide_shares,
};

/// The hour of the day the economy's daily step runs.
pub(crate) const NOON: u8 = 12;

/// One thing a vehicle is built from: a category any of whose goods may supply it (the cheapest at the place is taken), or one good.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VehicleInput {
    /// The category, such as `wood` or `fabric`, where the input is not one good.
    #[serde(default)]
    pub category: Option<String>,
    /// The good, where the input is one good.
    #[serde(default)]
    pub good: Option<String>,
    /// Category units (as the goods' `supplies` count them) or whole units of the good.
    pub amount: u64,
}

/// A live animal a vehicle is built with and keeps (r139).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VehicleAnimal {
    /// The herd species, by its animal's good id.
    pub species: String,
    /// Head.
    pub count: u64,
}

/// What a stretch of 30 days' travel uses up (T23).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VehicleWear {
    /// The good used up.
    pub good: String,
    /// Whole units for each 30 days of travel.
    pub units: u64,
}

/// One kind of vehicle: a row of the spec's section 3.1, in the order the rules list them.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VehicleKind {
    /// Stable lower-case key.
    pub id: String,
    /// Hours the hands spend building it.
    pub build_hours: u32,
    /// Hands a build holds for those hours.
    pub hands: u64,
    /// The cultures that may build it; empty when anyone may (r47).
    #[serde(default)]
    pub cultures: Vec<String>,
    /// What it is built from.
    pub inputs: Vec<VehicleInput>,
    /// The animals it is built with, which are part of it from then on.
    #[serde(default)]
    pub animals: Vec<VehicleAnimal>,
    /// What it holds, in grams.
    pub carries_g: u64,
    /// What it holds, in millilitres.
    pub carries_ml: u64,
    /// What it takes up when carried, animal included, in grams; none for a vehicle nothing carries.
    #[serde(default)]
    pub carried_g: Option<u64>,
    /// What it takes up when carried, in millilitres.
    #[serde(default)]
    pub carried_ml: Option<u64>,
    /// Its full crew.
    pub crew_full: u64,
    /// The fewest people it moves with.
    pub crew_fewest: u64,
    /// Its pace on land in per-mille of walking pace; none for a vehicle that cannot move on land by itself.
    #[serde(default)]
    pub land_pace_pm: Option<u32>,
    /// The travel profile that moves it on water; none for a vehicle that is carried over water.
    #[serde(default)]
    pub water: Option<String>,
    /// What it uses up as it travels.
    pub wear: VehicleWear,
}

impl VehicleKind {
    /// Refuses a kind the economy cannot build or carry.
    pub(crate) fn validate(&self, rules: &Rules, file: &str) -> Result<(), DataError> {
        let refuse = |field: &str, problem: &str| DataError {
            file: file.to_owned(),
            entry: format!("vehicles.{}.{field}", self.id),
            problem: problem.to_owned(),
        };
        if self.id.is_empty() {
            return Err(refuse("id", "must not be empty"));
        }
        if self.build_hours == 0 || self.hands == 0 {
            return Err(refuse("build_hours", "a build needs hours and hands"));
        }
        if self.crew_fewest == 0 || self.crew_fewest > self.crew_full {
            return Err(refuse("crew_fewest", "must be from 1 to the full crew"));
        }
        if self.carried_g.is_some() != self.carried_ml.is_some() {
            return Err(refuse(
                "carried_g",
                "weight and space are both given or neither",
            ));
        }
        if self.land_pace_pm == Some(0) {
            return Err(refuse("land_pace_pm", "must be positive"));
        }
        if self.land_pace_pm.is_none() && self.water.is_none() && self.carried_g.is_none() {
            return Err(refuse(
                "land_pace_pm",
                "moves on no ground and cannot be carried",
            ));
        }
        if self.inputs.is_empty() {
            return Err(refuse("inputs", "a vehicle is built from something"));
        }
        for input in &self.inputs {
            let ok = match (&input.category, &input.good) {
                (Some(category), None) => rules
                    .goods
                    .iter()
                    .any(|good| good.units_for_category(category, 1).is_some()),
                (None, Some(good)) => rules.goods.iter().any(|item| item.id == *good),
                _ => false,
            };
            if !ok || input.amount == 0 {
                return Err(refuse(
                    "inputs",
                    "an input names one category or one good that exist, and an amount",
                ));
            }
        }
        for animal in &self.animals {
            if animal.count == 0
                || !rules
                    .herd_species
                    .iter()
                    .any(|species| species.animal == animal.species)
            {
                return Err(refuse("animals", "names an unknown species or no head"));
            }
        }
        if !rules.goods.iter().any(|good| good.id == self.wear.good) || self.wear.units == 0 {
            return Err(refuse("wear", "names an unknown good or no units"));
        }
        Ok(())
    }
}

/// How many vehicles, jobs and expeditions the economy has numbered so far; the next of each is one more. Numbers are the economy's, across
/// every place and trader, and are saved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TradeCounters {
    /// Vehicles numbered.
    pub vehicles: u64,
    /// Jobs numbered.
    pub jobs: u64,
    /// Expeditions numbered (CK-432).
    pub expeditions: u64,
}

/// A vehicle in the game (T7, T14): a kind, the pop that owns it and whether it is free. It lasts and is never bought or sold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vehicle {
    /// The vehicle's number.
    pub number: u64,
    /// Its kind, an index into the rules' vehicles.
    pub kind: u32,
    /// The owning pop's slot in the place.
    pub pop: u32,
    /// Whether it is free for a job; one reserved by a trade job or away on a trip is not (part C).
    pub free: bool,
}

/// What a job is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobKind {
    /// Build a vehicle of the rules' vehicle kind (an index).
    Build {
        /// The vehicle kind.
        vehicle: u32,
    },
    /// Carry out a plan: gather the cargo, the animals and the supplies, and when its people can be had and its legs assign, send
    /// the expedition (CK-432; its stockpile becomes the expedition's).
    Trade {
        /// The plan, which also names the vehicles the job has reserved.
        plan: Box<Plan>,
    },
    /// Animals an unloaded expedition brought home that no job wanted and no herd of the trader had room for: held on offer at the
    /// place and slaughtered after the set time, as a given-up job's are (11b.1 rule 1). It has no needs and is already given up.
    Held {
        /// The head put on offer, by herd species, kept after the slaughter so what its yield can be is bounded by it.
        origin: Vec<u64>,
    },
}

/// The hands a started job holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hands {
    /// The pop they come from.
    pub pop: u32,
    /// How many.
    pub count: u64,
    /// The building each hand works in, as `(building, hands)` in building order: the building that employs most of the pop's
    /// ordinary workers takes the next hand, ties by building order; managers are used only when no ordinary job is held.
    pub by_building: Vec<(u32, u64)>,
    /// The hour the job started.
    pub started: u64,
    /// The hour the build is done.
    pub done: u64,
    /// What the hired pop was paid, as `(good, units)`; empty for the trader's own people.
    pub paid: Vec<(u32, u64)>,
}

/// A job: a temporary one-off building with its own stockpile and animals (T20).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    /// The job's number, in the order jobs are made.
    pub number: u64,
    /// The trader pop's slot in the place.
    pub pop: u32,
    /// What it is for.
    pub kind: JobKind,
    /// The hour it was made.
    pub made: u64,
    /// The goods it needs, in rules order.
    pub need_goods: Vec<u64>,
    /// The animals it needs, by herd species.
    pub need_animals: Vec<u64>,
    /// What it has gathered, in rules order; it is no longer the trader's to eat, offer or count.
    pub stock: Vec<u64>,
    /// The animals it holds, by herd species: they neither breed nor yield and need nothing.
    pub animals: Vec<u64>,
    /// The hands it holds once it has started.
    pub hands: Option<Hands>,
    /// The hour it was given up, once it has been.
    pub given_up: Option<u64>,
}

impl Job {
    /// What the job still lacks of a good: its trader's buy order. A job that has started or been given up lacks nothing.
    pub(crate) fn lack(&self, good: usize) -> u64 {
        if self.hands.is_some() || self.given_up.is_some() {
            return 0;
        }
        self.need_goods[good].saturating_sub(self.stock[good])
    }

    /// What the job still lacks of an animal species.
    pub(crate) fn animal_lack(&self, species: usize) -> u64 {
        if self.hands.is_some() || self.given_up.is_some() {
            return 0;
        }
        self.need_animals[species].saturating_sub(self.animals[species])
    }

    fn waiting(&self) -> bool {
        self.hands.is_none() && self.given_up.is_none()
    }

    pub(crate) fn ready(&self) -> bool {
        self.waiting()
            && self
                .need_goods
                .iter()
                .zip(&self.stock)
                .all(|(need, have)| have >= need)
            && self
                .need_animals
                .iter()
                .zip(&self.animals)
                .all(|(need, have)| have >= need)
    }
}

/// A place's vehicles and jobs, and the days its buildings' workers were away on jobs since each building's last payday.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlaceTrade {
    /// The vehicles of the place's pops, in the order they were built.
    pub vehicles: Vec<Vehicle>,
    /// The jobs, in the order they were made (so in their numbers' order).
    pub jobs: Vec<Job>,
    /// Person-days away from the work of a building, by `(building, pop)`, counted at each noon a hand is on a build and cleared by
    /// the building's payday (r5 3.6). Saved.
    pub days_away: BTreeMap<(u32, u32), u64>,
    /// The expeditions of the place's pops that are away, in the order they were made (CK-432). Saved.
    pub expeditions: Vec<Expedition>,
}

impl PlaceTrade {
    /// The hands of started jobs and the people of expeditions held from a pop, who cannot be hired by another job.
    pub(crate) fn held(&self, pop: usize) -> u64 {
        self.jobs
            .iter()
            .filter_map(|job| job.hands.as_ref())
            .filter(|hands| hands.pop as usize == pop)
            .fold(0_u64, |sum, hands| sum.saturating_add(hands.count))
            .saturating_add(self.away(pop))
    }

    /// The people of a pop who are away with an expedition: they eat from its supplies and not at home (r5 3.6).
    pub(crate) fn away(&self, pop: usize) -> u64 {
        self.expeditions
            .iter()
            .flat_map(|expedition| &expedition.groups)
            .filter(|group| group.pop as usize == pop)
            .fold(0, |sum, group| sum.saturating_add(group.count))
    }

    /// The animals of a species that given-up jobs of a pop offer at the place, and no herd of the pop could take back (11b.1).
    pub(crate) fn offered(&self, pop: usize, species: usize) -> u64 {
        self.jobs
            .iter()
            .filter(|job| job.given_up.is_some() && job.pop as usize == pop)
            .fold(0, |sum, job| sum.saturating_add(job.animals[species]))
    }
}

pub(crate) fn convert(value: usize) -> u32 {
    u32::try_from(value).expect("a pop, building or kind index fits 32 bits")
}

/// The place's whole lacks, as `(goods by pop, animals by pop)`: what the pops' waiting jobs want and have not got, which is added to
/// their buy orders (r5 3.4) and to what they seek in the exchange.
pub(crate) fn lacks(rules: &Rules, place: &PlaceState) -> (Vec<Vec<u64>>, Vec<Vec<u64>>) {
    let mut goods = vec![vec![0_u64; rules.goods.len()]; place.pops.len()];
    let mut animals = vec![vec![0_u64; rules.herd_species.len()]; place.pops.len()];
    for job in &place.trade.jobs {
        let pop = job.pop as usize;
        for (good, total) in goods[pop].iter_mut().enumerate() {
            *total = total.saturating_add(job.lack(good));
        }
        for (species, total) in animals[pop].iter_mut().enumerate() {
            *total = total.saturating_add(job.animal_lack(species));
        }
    }
    (goods, animals)
}

/// The units of a good a pop's waiting jobs still lack in all: what a swap may deliver straight into their stockpiles, beyond what the
/// pop's own store has room for (r5 3.4: the job's stockpile is its own receiving account).
pub(crate) fn lack_of(place: &PlaceState, pop: usize, good: usize) -> u64 {
    place
        .trade
        .jobs
        .iter()
        .filter(|job| job.pop as usize == pop)
        .fold(0, |sum, job| sum.saturating_add(job.lack(good)))
}

/// Delivers goods a pop received for its jobs straight into the oldest of its jobs that lack them, never through its store, and
/// returns how many units went in (r5 3.4: goods got for a job go to the job).
pub(crate) fn deliver_good(place: &mut PlaceState, pop: usize, good: usize, units: u64) -> u64 {
    let mut left = units;
    let mut moved = 0;
    for job in &mut place.trade.jobs {
        if left == 0 {
            break;
        }
        if job.pop as usize != pop {
            continue;
        }
        let take = job.lack(good).min(left);
        job.stock[good] += take;
        left -= take;
        moved += take;
    }
    moved
}

/// Where an arriving head goes (11b.1 rule 1): to the trader's oldest job that still lacks that species, else to the first herd of
/// that species, by building order, that the trader owns and that is below its standard level, else to the first such herd below its
/// maximum. Returns how many head found no place.
pub(crate) fn place_head(
    rules: &Rules,
    place: &mut PlaceState,
    pop: usize,
    species: usize,
    mut head: u64,
) -> Result<u64, DataError> {
    for job in &mut place.trade.jobs {
        let take = job.animal_lack(species).min(head);
        if job.pop as usize == pop && take > 0 {
            job.animals[species] += take;
            head -= take;
        }
    }
    let ownership = (0..place.buildings.len())
        .map(|index| resolve_owner_shares(place, index))
        .collect::<Result<Vec<_>, _>>()?;
    for to_standard in [true, false] {
        for (index, building) in place.buildings.iter_mut().enumerate() {
            if head == 0 {
                break;
            }
            if ownership[index].get(pop).copied().unwrap_or(0) == 0 {
                continue;
            }
            let Some(maximum) = herd_maximums(rules, building)?[species] else {
                continue;
            };
            let limit = if to_standard {
                herd_standard_level(rules, maximum)
            } else {
                maximum
            };
            let room = limit.saturating_sub(building.herds[species]);
            let taken = room.min(head);
            building.herds[species] += taken;
            head -= taken;
        }
    }
    Ok(head)
}

/// Takes from a pop what it holds of the given needs, for a job: its store first, then its storehouse account, then its part of the
/// spare stock of the buildings it owns, animals from the herds it owns above their standard levels (r5 3.4). Returns what was got.
fn take(
    rules: &Rules,
    place: &mut PlaceState,
    prices: &[u64],
    pop: usize,
    goods: &[u64],
    animals: &[u64],
) -> Result<(Vec<u64>, Vec<u64>), DataError> {
    let mut got = vec![0_u64; goods.len()];
    let mut head = vec![0_u64; animals.len()];
    for (good, lack) in goods.iter().enumerate() {
        let moved = (*lack).min(place.pops[pop].stock[good]);
        place.pops[pop].stock[good] -= moved;
        got[good] += moved;
    }
    if let Some(house) = place
        .buildings
        .iter()
        .position(|building| building.storehouse_accounts.is_some())
    {
        let building = &mut place.buildings[house];
        if let Some(accounts) = &mut building.storehouse_accounts {
            for (good, lack) in goods.iter().enumerate() {
                let moved = (*lack - got[good]).min(accounts.pops[pop][good]);
                accounts.pops[pop][good] -= moved;
                building.stock[good] -= moved;
                got[good] += moved;
            }
        }
    }
    let sells = building_market_sells(rules, place, prices)?;
    let ownership = (0..place.buildings.len())
        .map(|index| resolve_owner_shares(place, index))
        .collect::<Result<Vec<_>, _>>()?;
    for (b, building) in place.buildings.iter_mut().enumerate() {
        let weights = &ownership[b];
        if weights.get(pop).is_none_or(|owners| *owners == 0) {
            continue;
        }
        for (good, lack) in goods.iter().enumerate() {
            let wanted = *lack - got[good];
            if wanted == 0 {
                continue;
            }
            let share = wide_shares(sells[b][good], weights)
                .map_err(|problem| missing_data("trade.take".to_owned(), problem))?[pop];
            let moved = u64::try_from(share).unwrap_or(u64::MAX).min(wanted);
            if moved == 0 {
                continue;
            }
            if let Some(accounts) = &mut building.storehouse_accounts {
                accounts.communal[good] = accounts.communal[good]
                    .checked_sub(moved)
                    .ok_or_else(|| missing_data("trade.take".to_owned(), "communal underflow"))?;
            }
            building.stock[good] = building.stock[good]
                .checked_sub(moved)
                .ok_or_else(|| missing_data("trade.take".to_owned(), "building stock underflow"))?;
            got[good] += moved;
        }
        let offers = herd_levels(rules, building)?;
        for (species, lack) in animals.iter().enumerate() {
            let wanted = *lack - head[species];
            if wanted == 0 {
                continue;
            }
            let share = wide_shares(u128::from(offers[species].0), weights)
                .map_err(|problem| missing_data("trade.take".to_owned(), problem))?[pop];
            let moved = u64::try_from(share).unwrap_or(u64::MAX).min(wanted);
            if moved == 0 {
                continue;
            }
            building.herds[species] -= moved;
            head[species] += moved;
        }
    }
    Ok((got, head))
}

/// Makes a job take, from its trader, what it lacks and the trader holds.
pub(crate) fn fill_job(
    rules: &Rules,
    place: &mut PlaceState,
    prices: &[u64],
    index: usize,
) -> Result<(), DataError> {
    let job = &place.trade.jobs[index];
    let pop = job.pop as usize;
    let goods: Vec<u64> = (0..rules.goods.len()).map(|good| job.lack(good)).collect();
    let animals: Vec<u64> = (0..rules.herd_species.len())
        .map(|species| job.animal_lack(species))
        .collect();
    if goods.iter().chain(&animals).all(|lack| *lack == 0) {
        return Ok(());
    }
    let (got, head) = take(rules, place, prices, pop, &goods, &animals)?;
    let job = &mut place.trade.jobs[index];
    for (stock, more) in job.stock.iter_mut().zip(got) {
        *stock += more;
    }
    for (held, more) in job.animals.iter_mut().zip(head) {
        *held += more;
    }
    Ok(())
}

/// Each waiting job takes again what its trader has come to hold of what it lacks, the jobs of a trader oldest first (r5 3.4). Run
/// each noon before the exchange, at the prices the exchange begins with.
pub(crate) fn take_waiting(
    rules: &Rules,
    place: &mut PlaceState,
    prices: &[u64],
) -> Result<(), DataError> {
    for index in 0..place.trade.jobs.len() {
        if place.trade.jobs[index].waiting() {
            fill_job(rules, place, prices, index)?;
        }
    }
    Ok(())
}

pub(crate) fn good_index(rules: &Rules, id: &str) -> Result<usize, DataError> {
    rules
        .goods
        .iter()
        .position(|good| good.id == id)
        .ok_or_else(|| missing_data(format!("goods.{id}"), "good is missing"))
}

/// The good a category need is filled from: the qualifying good that costs least at the place's prices for the amount needed, equal
/// costs in the goods' order (r5 3.2). Returns it with the whole units needed.
fn fill_category(
    rules: &Rules,
    place: &PlaceState,
    category: &str,
    amount: u64,
) -> Result<(usize, u64), DataError> {
    let mut best: Option<(u128, usize, u128)> = None;
    for (index, good) in rules.goods.iter().enumerate() {
        let Some(units) = good.units_for_category(category, amount) else {
            continue;
        };
        let cost = units.saturating_mul(u128::from(place.prices_milli[index]));
        if best.is_none_or(|(least, _, _)| cost < least) {
            best = Some((cost, index, units));
        }
    }
    let (_, index, units) = best.ok_or_else(|| {
        missing_data(
            format!("categories.{category}"),
            "no good supplies the category",
        )
    })?;
    let units = u64::try_from(units)
        .map_err(|_| missing_data(format!("categories.{category}"), "units exceed u64"))?;
    Ok((index, units))
}

impl EconomyState {
    /// Orders a build: a job of the pop `pop` of the place `place` to build a vehicle of kind `vehicle` (the rules' vehicle id), made at
    /// game hour `now`. The order is given (by a test, and by part D's choosing); the job takes at once whatever its trader holds of
    /// what it needs (r5 3.4). Returns the job's number.
    ///
    /// # Errors
    /// An unknown place, pop or vehicle, or a pop whose culture may not build the vehicle (the currach is for Irish pops, r47),
    /// leaving the state unchanged.
    pub fn order_build(
        &mut self,
        rules: &Rules,
        place: usize,
        pop: usize,
        vehicle: &str,
        now: u64,
    ) -> Result<u64, DataError> {
        let entry = || format!("trade.order_build.{vehicle}");
        let kind = rules
            .vehicles
            .iter()
            .position(|kind| kind.id == vehicle)
            .ok_or_else(|| missing_data(entry(), "vehicle kind is missing"))?;
        if now > last_hour() {
            return Err(missing_data(entry(), "the hour is outside the calendar"));
        }
        let number = self
            .trade
            .jobs
            .checked_add(1)
            .ok_or_else(|| missing_data(entry(), "job numbers are used up"))?;
        let state = self
            .places
            .get_mut(place)
            .ok_or_else(|| missing_data(entry(), "place is outside the economy"))?;
        let culture = state
            .pops
            .get(pop)
            .ok_or_else(|| missing_data(entry(), "pop is outside the place"))?
            .culture
            .clone();
        let definition = &rules.vehicles[kind];
        if !definition.cultures.is_empty() && !definition.cultures.contains(&culture) {
            return Err(missing_data(entry(), "the pop's culture may not build it"));
        }
        let mut need_goods = vec![0_u64; rules.goods.len()];
        for input in &definition.inputs {
            let (good, units) = match (&input.category, &input.good) {
                (Some(category), _) => fill_category(rules, state, category, input.amount)?,
                (None, Some(good)) => (good_index(rules, good)?, input.amount),
                (None, None) => return Err(missing_data(entry(), "an input names nothing")),
            };
            need_goods[good] = need_goods[good]
                .checked_add(units)
                .ok_or_else(|| missing_data(entry(), "need overflow"))?;
        }
        let mut need_animals = vec![0_u64; rules.herd_species.len()];
        for animal in &definition.animals {
            let species = rules
                .herd_species
                .iter()
                .position(|species| species.animal == animal.species)
                .ok_or_else(|| missing_data(entry(), "species is missing"))?;
            need_animals[species] = need_animals[species].saturating_add(animal.count);
        }
        state.trade.jobs.push(Job {
            number,
            pop: convert(pop),
            kind: JobKind::Build {
                vehicle: convert(kind),
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
        let result = fill_job(rules, state, &prices, index);
        if let Err(error) = result {
            state.trade.jobs.pop();
            return Err(error);
        }
        self.trade.jobs = number;
        Ok(number)
    }
}

fn job_heads(rules: &Rules, building: &BuildingState, pop: usize, manager: bool) -> u64 {
    let Some(definition) = rules.buildings.iter().find(|kind| kind.id == building.kind) else {
        return 0;
    };
    let Some(employment) = building.employment.get(pop) else {
        return 0;
    };
    employment
        .jobs
        .iter()
        .filter(|(id, _)| {
            definition
                .jobs
                .iter()
                .chain(definition.methods.iter().flat_map(|method| &method.jobs))
                .any(|job| job.id == **id && job.manager == manager)
        })
        .fold(0_u64, |sum, (_, count)| sum.saturating_add(*count))
}

/// The ordinary posted wage of the building from which a hand is drawn.
pub(crate) fn pop_wage(rules: &Rules, place: &PlaceState, pop: usize) -> u64 {
    let ordinary = place
        .buildings
        .iter()
        .any(|building| job_heads(rules, building, pop, false) > 0);
    place
        .buildings
        .iter()
        .map(|building| {
            (
                job_heads(rules, building, pop, !ordinary),
                building.posted_wage(),
            )
        })
        .enumerate()
        .filter(|(_, (workers, _))| *workers > 0)
        .max_by(|(a, (left, _)), (b, (right, _))| left.cmp(right).then_with(|| b.cmp(a)))
        .map_or(0, |(_, (_, wage))| wage)
}

/// Allocate hands to ordinary jobs first, or managers when no ordinary job is held.
pub(crate) fn hand_buildings(
    rules: &Rules,
    place: &PlaceState,
    pop: usize,
    count: u64,
) -> Vec<(u32, u64)> {
    let paid: Vec<u64> = place
        .buildings
        .iter()
        .map(|building| job_heads(rules, building, pop, false))
        .collect();
    let mut left = if paid.iter().any(|workers| *workers > 0) {
        paid
    } else {
        place
            .buildings
            .iter()
            .map(|building| job_heads(rules, building, pop, true))
            .collect()
    };
    // What the pop's hands held by other jobs already take of each building's positions.
    for job in &place.trade.jobs {
        if let Some(hands) = job.hands.as_ref().filter(|hands| hands.pop as usize == pop) {
            for (building, held) in &hands.by_building {
                if let Some(slot) = left.get_mut(*building as usize) {
                    *slot = slot.saturating_sub(*held);
                }
            }
        }
    }
    for group in place
        .trade
        .expeditions
        .iter()
        .flat_map(|expedition| &expedition.groups)
        .filter(|group| group.pop as usize == pop)
    {
        for (building, held) in &group.by_building {
            if let Some(slot) = left.get_mut(*building as usize) {
                *slot = slot.saturating_sub(*held);
            }
        }
    }
    let mut hands = vec![0_u64; left.len()];
    for _ in 0..count {
        let mut best = 0;
        for (index, workers) in left.iter().enumerate() {
            if *workers > left[best] {
                best = index;
            }
        }
        if left.get(best).is_none_or(|workers| *workers == 0) {
            break;
        }
        left[best] -= 1;
        hands[best] += 1;
    }
    hands
        .into_iter()
        .enumerate()
        .filter(|(_, count)| *count > 0)
        .map(|(building, count)| (convert(building), count))
        .collect()
}

/// What the trader pays a hired pop and in what goods: the fewest whole units of one good worth at least the wage, from the goods the
/// hired pop has buy orders for, the biggest deficit first, then any food, in the first of which the trader holds enough and the hired
/// pop's store takes all of it (r5 3.4). `None` when nothing can pay it.
pub(crate) fn wage_goods(
    rules: &Rules,
    place: &PlaceState,
    payer: usize,
    hired: usize,
    wage_milli: u128,
) -> Result<Option<(usize, u64)>, DataError> {
    let people = place.pops[hired]
        .workers
        .checked_add(place.pops[hired].dependents)
        .ok_or_else(|| missing_data("trade.wage".to_owned(), "population size overflow"))?;
    let mut candidates = Vec::new();
    for (good, item) in rules.goods.iter().enumerate() {
        if rules
            .herd_species
            .iter()
            .any(|species| species.animal == item.id)
        {
            continue;
        }
        let deficit = pop_target_units(rules, &place.pops[hired].kind, people, good)?
            .saturating_sub(pop_market_holding(place, hired, good)?);
        candidates.push((deficit, good));
    }
    // The biggest deficit first, the goods' order between equals; then any food that is not already listed.
    let mut order: Vec<usize> = {
        let mut wanted: Vec<_> = candidates
            .iter()
            .filter(|(deficit, _)| *deficit > 0)
            .collect();
        wanted.sort_by(|(a, left), (b, right)| b.cmp(a).then(left.cmp(right)));
        wanted.into_iter().map(|(_, good)| *good).collect()
    };
    for (good, item) in rules.goods.iter().enumerate() {
        if item.hunger_centi > 0 && !order.contains(&good) {
            order.push(good);
        }
    }
    for good in order {
        let price = u128::from(place.prices_milli[good]);
        if price == 0 {
            continue;
        }
        let units = u64::try_from(wage_milli.div_ceil(price).max(1))
            .map_err(|_| missing_data("trade.wage".to_owned(), "wage units exceed u64"))?;
        if pop_market_holding(place, payer, good)? < u128::from(units) {
            continue;
        }
        let people = place.pops[hired]
            .workers
            .checked_add(place.pops[hired].dependents)
            .ok_or_else(|| missing_data("trade.wage".to_owned(), "population size overflow"))?;
        let accepts =
            crate::pop_store_accepts(rules, people, &place.pops[hired].stock, good, units)?;
        if accepts == units {
            return Ok(Some((good, units)));
        }
    }
    Ok(None)
}

/// Takes `units` of a good from a pop: its store first, then its storehouse account.
pub(crate) fn pay_from(
    place: &mut PlaceState,
    pop: usize,
    good: usize,
    units: u64,
) -> Result<(), DataError> {
    let from_store = place.pops[pop].stock[good].min(units);
    let from_account = units - from_store;
    if from_account > 0 {
        let house = place
            .buildings
            .iter()
            .position(|building| building.storehouse_accounts.is_some())
            .ok_or_else(|| missing_data("trade.wage".to_owned(), "the payer holds too little"))?;
        let building = &mut place.buildings[house];
        let held = building
            .storehouse_accounts
            .as_mut()
            .and_then(|accounts| accounts.pops.get_mut(pop))
            .and_then(|account| account.get_mut(good))
            .ok_or_else(|| missing_data("trade.wage".to_owned(), "the payer holds too little"))?;
        *held = held
            .checked_sub(from_account)
            .ok_or_else(|| missing_data("trade.wage".to_owned(), "the payer holds too little"))?;
        building.stock[good] -= from_account;
    }
    place.pops[pop].stock[good] -= from_store;
    Ok(())
}

/// Starts a job whose goods are gathered if enough people are at home and willing at its wage (11b.2 rule 4, 11c.1): the trader's own
/// people are not paid; otherwise the pop at the lowest wage with enough people at home is paid in goods and its people are held for
/// the build (r5 3.4). A job that cannot pay does not start. Returns whether it started.
fn try_start(
    rules: &Rules,
    place: &mut PlaceState,
    counters: &mut TradeCounters,
    index: usize,
    now: u64,
    moves_now: bool,
) -> Result<bool, DataError> {
    let job = &place.trade.jobs[index];
    if !job.ready() {
        return Ok(false);
    }
    let vehicle = match &job.kind {
        JobKind::Build { vehicle } => *vehicle,
        JobKind::Trade { .. } => {
            return crate::expedition::start_trade(rules, place, counters, index, now, moves_now);
        }
        JobKind::Held { .. } => return Ok(false),
    };
    let kind = &rules.vehicles[vehicle as usize];
    let (trader, count) = (job.pop as usize, kind.hands);
    let days = u128::from(kind.build_hours.div_ceil(24).max(1));
    let mut options: Vec<(u128, usize)> = Vec::new();
    for pop in 0..place.pops.len() {
        let home = place.pops[pop]
            .workers
            .saturating_sub(place.trade.held(pop));
        if home < count {
            continue;
        }
        let wage = if pop == trader {
            0
        } else {
            u128::from(count)
                .saturating_mul(u128::from(pop_wage(rules, place, pop)))
                .saturating_mul(days)
                / 30
                + 1
        };
        options.push((wage, pop));
    }
    options.sort_unstable();
    for (wage, pop) in options {
        let mut paid = Vec::new();
        if wage > 0 {
            let Some((good, units)) = wage_goods(rules, place, trader, pop, wage)? else {
                continue;
            };
            pay_from(place, trader, good, units)?;
            let unaccepted = receive_pop(rules, &mut place.pops[pop], good, units)?;
            if unaccepted != 0 {
                return Err(missing_data(
                    "trade.wage".to_owned(),
                    "a wage the store accepted no longer fits",
                ));
            }
            paid.push((convert(good), units));
        }
        let by_building = hand_buildings(rules, place, pop, count);
        place.trade.jobs[index].hands = Some(Hands {
            pop: convert(pop),
            count,
            by_building,
            started: now,
            done: now.saturating_add(u64::from(kind.build_hours)),
            paid,
        });
        return Ok(true);
    }
    Ok(false)
}

/// Starts every ready job of a place, in the order of their numbers (11b.2 rule 4).
/// `moves_now` is whether an expedition started in this call moves in this very tick: a job started in the noon's daily step does (the
/// hourly step follows it in the same tick), one started in an hour's own step moves from the next tick.
pub(crate) fn start_ready(
    rules: &Rules,
    place: &mut PlaceState,
    counters: &mut TradeCounters,
    now: u64,
    moves_now: bool,
) -> Result<(), DataError> {
    // A trade job that starts leaves the list, so the next job takes its index.
    let mut index = 0;
    while index < place.trade.jobs.len() {
        let before = place.trade.jobs.len();
        try_start(rules, place, counters, index, now, moves_now)?;
        if place.trade.jobs.len() == before {
            index += 1;
        }
    }
    Ok(())
}

/// Counts the noon just spent on one job's hands as a day away from the other work of each, in the building that hand works in (r5
/// 3.6): it counts towards the building's next payday. A hand of a given building that `counted` says a build has just released was
/// counted at this noon in that build, so each such absence is matched once and not counted again.
fn count_job(place: &mut PlaceState, index: usize, counted: &mut BTreeMap<(u32, u32), u64>) {
    let mut add: Vec<((u32, u32), u64)> = Vec::new();
    if let Some(hands) = &place.trade.jobs[index].hands {
        for (building, count) in &hands.by_building {
            let key = (*building, hands.pop);
            let matched = counted.get_mut(&key).map_or(0, |free| {
                let matched = (*free).min(*count);
                *free -= matched;
                matched
            });
            if *count > matched {
                add.push((key, *count - matched));
            }
        }
    }
    for (key, count) in add {
        let days = place.trade.days_away.entry(key).or_insert(0);
        *days = days.saturating_add(count);
    }
}

/// Counts the noon just spent on every started build (the daily step, after the day's starts).
pub(crate) fn count_days_away(place: &mut PlaceState) {
    for index in 0..place.trade.jobs.len() {
        count_job(place, index, &mut BTreeMap::new());
    }
    crate::expedition::count_noon_all(place);
}

/// Completes the builds whose time is up: the inputs are gone, the vehicle belongs to the trader pop, free, its animal part of it from
/// then on, and the job ends (r5 3.4, 11b.2 rule 4). Returns the hands released, by pop.
fn complete_builds(
    place: &mut PlaceState,
    counters: &mut TradeCounters,
    now: u64,
) -> Result<BTreeMap<(u32, u32), u64>, DataError> {
    let mut released = BTreeMap::new();
    let mut index = 0;
    while index < place.trade.jobs.len() {
        let done = place.trade.jobs[index]
            .hands
            .as_ref()
            .is_some_and(|hands| hands.done <= now);
        if !done {
            index += 1;
            continue;
        }
        // A number that cannot be issued is an error before anything is committed, never a duplicate.
        let number = counters.vehicles.checked_add(1).ok_or_else(|| {
            missing_data("trade.vehicles".to_owned(), "vehicle numbers are used up")
        })?;
        counters.vehicles = number;
        let job = place.trade.jobs.remove(index);
        if let Some(hands) = &job.hands {
            for (building, count) in &hands.by_building {
                let held: &mut u64 = released.entry((*building, hands.pop)).or_insert(0);
                *held = held.saturating_add(*count);
            }
        }
        let JobKind::Build { vehicle } = job.kind else {
            return Err(missing_data(
                "trade.builds".to_owned(),
                "a job that is not a build holds hands",
            ));
        };
        place.trade.vehicles.push(Vehicle {
            number,
            kind: vehicle,
            pop: job.pop,
            free: true,
        });
    }
    Ok(released)
}

/// Gives up a job that was not started in time: it holds no people, a live animal goes back as 11b.1 says and the rest stays with it
/// and is on offer, and its goods go back to the trader as room allows (r5 3.4).
fn give_up(rules: &Rules, place: &mut PlaceState, index: usize, now: u64) -> Result<(), DataError> {
    place.trade.jobs[index].given_up = Some(now);
    // A trade job that is given up lets go of the vehicles it reserved (11b.2 rule 5).
    if let JobKind::Trade { plan } = &place.trade.jobs[index].kind {
        for number in &plan.vehicles {
            if let Some(vehicle) = place
                .trade
                .vehicles
                .iter_mut()
                .find(|vehicle| vehicle.number == *number)
            {
                vehicle.free = true;
            }
        }
    }
    for species in 0..rules.herd_species.len() {
        let head = std::mem::take(&mut place.trade.jobs[index].animals[species]);
        if head == 0 {
            continue;
        }
        let pop = place.trade.jobs[index].pop as usize;
        let left = place_head(rules, place, pop, species, head)?;
        place.trade.jobs[index].animals[species] = left;
    }
    Ok(())
}

/// Returns what a given-up job holds of goods to its trader's store as room allows, nothing destroyed, and ends the job when it holds
/// nothing at all.
fn settle_given_up(rules: &Rules, place: &mut PlaceState, index: usize) -> Result<(), DataError> {
    let pop = place.trade.jobs[index].pop as usize;
    for good in 0..rules.goods.len() {
        let units = place.trade.jobs[index].stock[good];
        if units == 0 {
            continue;
        }
        let left = receive_pop(rules, &mut place.pops[pop], good, units)?;
        place.trade.jobs[index].stock[good] = left;
    }
    Ok(())
}

/// Slaughters the animals a given-up job still holds once the set time since it was given up has passed: their meat and hide go to
/// the job's stock, and so back to the trader (r5 3.2).
fn slaughter(
    rules: &Rules,
    place: &mut PlaceState,
    index: usize,
    now: u64,
) -> Result<(), DataError> {
    let Some(since) = place.trade.jobs[index].given_up else {
        return Ok(());
    };
    if now < since.saturating_add(rules.held_animal_hours) {
        return Ok(());
    }
    for (species, rules_species) in rules.herd_species.iter().enumerate() {
        let head = std::mem::take(&mut place.trade.jobs[index].animals[species]);
        for yielded in &rules_species.yields {
            let good = good_index(rules, &yielded.good)?;
            let units = head
                .checked_mul(yielded.units)
                .ok_or_else(|| missing_data("trade.slaughter".to_owned(), "yield overflow"))?;
            place.trade.jobs[index].stock[good] = place.trade.jobs[index].stock[good]
                .checked_add(units)
                .ok_or_else(|| missing_data("trade.slaughter".to_owned(), "yield overflow"))?;
        }
    }
    Ok(())
}

/// One game hour of the trade systems, after the day's step where it is noon (11c.1 rule 2): builds that are done complete, ready jobs
/// start in the order of their numbers, jobs at their give-up hour that did not start are given up, and what a given-up job holds goes
/// back. A job made ready in its give-up hour starts.
///
/// # Errors
/// A malformed saved job or place; the economy is left unchanged.
pub fn run_trade_hour(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
) -> Result<(), DataError> {
    let now = GameTime::from_date(date)
        .map_err(|_| missing_data("trade.hour".to_owned(), "the date is outside the calendar"))?
        .hours();
    // Only a place with a job has anything to do this hour; its changes are worked on a copy and committed together.
    let mut counters = state.trade.clone();
    let mut worked: Vec<(usize, PlaceState)> = Vec::new();
    for (index, original) in state.places.iter().enumerate() {
        if original.trade.jobs.is_empty() && original.trade.expeditions.is_empty() {
            continue;
        }
        let mut place = original.clone();
        hour_of_place(rules, &mut place, &mut counters, date, now)?;
        worked.push((index, place));
    }
    state.trade = counters;
    for (index, place) in worked {
        state.places[index] = place;
    }
    Ok(())
}

fn hour_of_place(
    rules: &Rules,
    place: &mut PlaceState,
    counters: &mut TradeCounters,
    date: Date,
    now: u64,
) -> Result<(), DataError> {
    let mut released = complete_builds(place, counters, now)?;
    // Expeditions move one hour, arrive and unload, before the day's jobs start (11c.1 rule 2), so the people and vehicles that come
    // home this hour can be hired and reserved by a job that starts in it.
    let came_home = crate::expedition::move_expeditions(rules, place, counters, now)?;
    for (key, count) in came_home {
        let held: &mut u64 = released.entry(key).or_insert(0);
        *held = held.saturating_add(count);
    }
    let waiting: Vec<u64> = place
        .trade
        .jobs
        .iter()
        .filter(|job| job.waiting())
        .map(|job| job.number)
        .collect();
    let first_new = counters.expeditions;
    start_ready(rules, place, counters, now, false)?;
    if date.hour == NOON {
        // The noon's count has run in the daily step; a job that starts now has spent this noon too (11c.1 rule 1), but a hand a build
        // has just released was counted at that noon in the build, at the building it worked in, and is one person away for one day,
        // not two: a new job counts only the hands at each building beyond those released from it.
        for number in waiting {
            if let Some(index) = place.trade.jobs.iter().position(|job| job.number == number)
                && place.trade.jobs[index].hands.is_some()
            {
                count_job(place, index, &mut released);
            }
        }
        // The people of an expedition that left in this hour's step are away at this noon too.
        crate::expedition::count_noon_for(place, first_new, &mut released);
    }
    for index in 0..place.trade.jobs.len() {
        let job = &place.trade.jobs[index];
        if job.waiting() && job.made.saturating_add(rules.job_giveup_hours) <= now {
            give_up(rules, place, index, now)?;
        }
    }
    for index in 0..place.trade.jobs.len() {
        slaughter(rules, place, index, now)?;
        if place.trade.jobs[index].given_up.is_some() {
            settle_given_up(rules, place, index)?;
        }
    }
    place.trade.jobs.retain(|job| {
        job.given_up.is_none()
            || job.stock.iter().any(|units| *units > 0)
            || job.animals.iter().any(|head| *head > 0)
    });
    Ok(())
}

/// The days a building's workers were away since its last payday, by pop, and clears them (the payday has counted them).
pub(crate) fn take_days_away(place: &mut PlaceState, building: usize) -> Vec<u64> {
    let mut away = vec![0_u64; place.pops.len()];
    let keys: Vec<(u32, u32)> = place
        .trade
        .days_away
        .range((convert(building), 0)..=(convert(building), u32::MAX))
        .map(|(key, _)| *key)
        .collect();
    for key in keys {
        if let Some(days) = place.trade.days_away.remove(&key)
            && let Some(slot) = away.get_mut(key.1 as usize)
        {
            *slot = days;
        }
    }
    away
}

impl Canonical for PlaceTrade {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        encode_length(self.vehicles.len(), out)?;
        for vehicle in &self.vehicles {
            vehicle.number.encode(out)?;
            vehicle.kind.encode(out)?;
            vehicle.pop.encode(out)?;
            vehicle.free.encode(out)?;
        }
        encode_length(self.jobs.len(), out)?;
        for job in &self.jobs {
            job.number.encode(out)?;
            job.pop.encode(out)?;
            match &job.kind {
                JobKind::Build { vehicle } => {
                    0_u8.encode(out)?;
                    vehicle.encode(out)?;
                }
                JobKind::Trade { plan } => {
                    1_u8.encode(out)?;
                    plan.encode(out)?;
                }
                JobKind::Held { origin } => {
                    2_u8.encode(out)?;
                    origin.encode(out)?;
                }
            }
            job.made.encode(out)?;
            job.need_goods.encode(out)?;
            job.need_animals.encode(out)?;
            job.stock.encode(out)?;
            job.animals.encode(out)?;
            match &job.hands {
                Some(hands) => {
                    true.encode(out)?;
                    hands.pop.encode(out)?;
                    hands.count.encode(out)?;
                    encode_length(hands.by_building.len(), out)?;
                    for (building, count) in &hands.by_building {
                        building.encode(out)?;
                        count.encode(out)?;
                    }
                    hands.started.encode(out)?;
                    hands.done.encode(out)?;
                    encode_length(hands.paid.len(), out)?;
                    for (good, units) in &hands.paid {
                        good.encode(out)?;
                        units.encode(out)?;
                    }
                }
                None => false.encode(out)?,
            }
            match job.given_up {
                Some(hour) => {
                    true.encode(out)?;
                    hour.encode(out)?;
                }
                None => false.encode(out)?,
            }
        }
        encode_length(self.days_away.len(), out)?;
        for ((building, pop), days) in &self.days_away {
            building.encode(out)?;
            pop.encode(out)?;
            days.encode(out)?;
        }
        self.expeditions.encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let count = input.length()?;
        let mut vehicles = Vec::with_capacity(count);
        for _ in 0..count {
            vehicles.push(Vehicle {
                number: u64::decode(input)?,
                kind: u32::decode(input)?,
                pop: u32::decode(input)?,
                free: bool::decode(input)?,
            });
        }
        let count = input.length()?;
        let mut jobs = Vec::with_capacity(count);
        for _ in 0..count {
            let number = u64::decode(input)?;
            let pop = u32::decode(input)?;
            let kind = match u8::decode(input)? {
                0 => JobKind::Build {
                    vehicle: u32::decode(input)?,
                },
                1 => JobKind::Trade {
                    plan: Box::new(Plan::decode(input)?),
                },
                2 => JobKind::Held {
                    origin: Vec::<u64>::decode(input)?,
                },
                tag => return Err(CanonicalError::BadTag { ty: "JobKind", tag }),
            };
            let made = u64::decode(input)?;
            let need_goods = Vec::<u64>::decode(input)?;
            let need_animals = Vec::<u64>::decode(input)?;
            let stock = Vec::<u64>::decode(input)?;
            let animals = Vec::<u64>::decode(input)?;
            let hands = if bool::decode(input)? {
                let pop = u32::decode(input)?;
                let count = u64::decode(input)?;
                let buildings = input.length()?;
                let mut by_building = Vec::with_capacity(buildings);
                for _ in 0..buildings {
                    by_building.push((u32::decode(input)?, u64::decode(input)?));
                }
                let started = u64::decode(input)?;
                let done = u64::decode(input)?;
                let paid_len = input.length()?;
                let mut paid = Vec::with_capacity(paid_len);
                for _ in 0..paid_len {
                    paid.push((u32::decode(input)?, u64::decode(input)?));
                }
                Some(Hands {
                    pop,
                    count,
                    by_building,
                    started,
                    done,
                    paid,
                })
            } else {
                None
            };
            let given_up = if bool::decode(input)? {
                Some(u64::decode(input)?)
            } else {
                None
            };
            jobs.push(Job {
                number,
                pop,
                kind,
                made,
                need_goods,
                need_animals,
                stock,
                animals,
                hands,
                given_up,
            });
        }
        let count = input.length()?;
        let mut days_away = BTreeMap::new();
        for _ in 0..count {
            let key = (u32::decode(input)?, u32::decode(input)?);
            days_away.insert(key, u64::decode(input)?);
        }
        Ok(Self {
            vehicles,
            jobs,
            days_away,
            expeditions: Vec::<Expedition>::decode(input)?,
        })
    }
}

impl Canonical for TradeCounters {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.vehicles.encode(out)?;
        self.jobs.encode(out)?;
        self.expeditions.encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            vehicles: u64::decode(input)?,
            jobs: u64::decode(input)?,
            expeditions: u64::decode(input)?,
        })
    }
}

/// The most person-days a building's pop can have been away between two paydays: 31 days of 2^32 people.
const MOST_DAYS_AWAY: u64 = 31 << 32;

/// The last hour the game calendar reaches: 23:00 on 31 December 9999.
pub(crate) fn last_hour() -> u64 {
    GameTime::from_date(Date {
        year: 9999,
        month: 12,
        day: 31,
        hour: 23,
    })
    .expect("the last hour of the calendar is a date")
    .hours()
}

/// Refuses a saved economy whose vehicles, jobs, hands and days away are outside their domains (CK-431): a same-build save is read by
/// the same code and must not carry a value the trade systems, the payday or the tick would trust. Every index names something that
/// exists; every count is bounded by the people or units it can come from; every hour is within the calendar, and so is every deadline
/// made from it; job and vehicle numbers are unique, below the next number, and the next number can be issued.
///
/// # Errors
/// The first value out of its domain, named by place.
pub fn validate_trade_state(rules: &Rules, state: &EconomyState) -> Result<(), DataError> {
    validate_trade_state_at(rules, state, last_hour())
}

/// [`validate_trade_state`] for a state saved at game hour `now`: no hour of an expedition's trip is after it (CK-432).
///
/// # Errors
/// The first value out of its domain, named by place.
#[expect(
    clippy::too_many_lines,
    reason = "One check for each saved field's domain, in the order the fields are saved, so each is visible beside the others."
)]
pub fn validate_trade_state_at(
    rules: &Rules,
    state: &EconomyState,
    now: u64,
) -> Result<(), DataError> {
    let last = last_hour();
    let in_calendar = |hour: u64, then: u64| hour.checked_add(then).is_some_and(|end| end <= last);
    if state.trade.vehicles == u64::MAX
        || state.trade.jobs == u64::MAX
        || state.trade.expeditions == u64::MAX
    {
        return Err(missing_data(
            "trade.counters".to_owned(),
            "the next vehicle or job number cannot be issued",
        ));
    }
    let mut vehicle_numbers = std::collections::BTreeSet::new();
    let mut job_numbers = std::collections::BTreeSet::new();
    let mut expedition_numbers = std::collections::BTreeSet::new();
    for (place_index, place) in state.places.iter().enumerate() {
        let refuse = |entry: String, problem: &str| {
            missing_data(format!("places.{}.trade.{entry}", place.id), problem)
        };
        let (pops, buildings) = (place.pops.len(), place.buildings.len());
        for vehicle in &place.trade.vehicles {
            if vehicle.kind as usize >= rules.vehicles.len()
                || vehicle.pop as usize >= pops
                || vehicle.number == 0
                || vehicle.number > state.trade.vehicles
                || !vehicle_numbers.insert(vehicle.number)
            {
                return Err(refuse(
                    format!("vehicles.{}", vehicle.number),
                    "names a kind or pop that does not exist, or a number out of range or used twice",
                ));
            }
        }
        let mut last_job = 0;
        for job in &place.trade.jobs {
            let entry = || format!("jobs.{}", job.number);
            if job.number <= last_job
                || job.number > state.trade.jobs
                || !job_numbers.insert(job.number)
            {
                return Err(refuse(
                    entry(),
                    "numbers are not unique, increasing and issued",
                ));
            }
            last_job = job.number;
            let vehicle = match &job.kind {
                JobKind::Build { vehicle } => *vehicle,
                JobKind::Trade { .. } | JobKind::Held { .. } => {
                    crate::expedition::validate_other_job(rules, state, place_index, job, now)?;
                    continue;
                }
            };
            let Some(kind) = rules.vehicles.get(vehicle as usize) else {
                return Err(refuse(entry(), "names a vehicle kind that does not exist"));
            };
            if job.pop as usize >= pops {
                return Err(refuse(entry(), "names a pop that does not exist"));
            }
            let goods = rules.goods.len();
            let species = rules.herd_species.len();
            if [job.need_goods.len(), job.stock.len()]
                .iter()
                .any(|len| *len != goods)
                || [job.need_animals.len(), job.animals.len()]
                    .iter()
                    .any(|len| *len != species)
            {
                return Err(refuse(entry(), "goods and animals do not match the rules"));
            }
            // What a build needs of animals is what its kind is built with; what a job that is not given up has gathered is at most
            // what it needs (a started one has all of it).
            let wanted_animals = rules.herd_species.iter().map(|rules_species| {
                kind.animals
                    .iter()
                    .filter(|animal| animal.species == rules_species.animal)
                    .fold(0_u64, |sum, animal| sum.saturating_add(animal.count))
            });
            if wanted_animals
                .zip(&job.need_animals)
                .any(|(wanted, need)| wanted != *need)
            {
                return Err(refuse(
                    entry(),
                    "needs animals its vehicle is not built with",
                ));
            }
            if job.given_up.is_none()
                && (job
                    .stock
                    .iter()
                    .zip(&job.need_goods)
                    .any(|(have, need)| have > need)
                    || job
                        .animals
                        .iter()
                        .zip(&job.need_animals)
                        .any(|(have, need)| have > need))
            {
                return Err(refuse(entry(), "holds more than it needs"));
            }
            if job.hands.is_some() && job.given_up.is_some() {
                return Err(refuse(entry(), "is both started and given up"));
            }
            if job.given_up.is_none() {
                // What was recorded at the order still meets the vehicle's inputs, whatever good a category was filled from.
                // Each good's recorded need is bounded by what the recipe could ask of that good, so the sums below cannot overflow.
                let ceiling = |index: usize| {
                    kind.inputs.iter().fold(0_u128, |sum, input| {
                        let units = match (&input.category, &input.good) {
                            (Some(category), _) => rules.goods[index]
                                .units_for_category(category, input.amount)
                                .unwrap_or(0),
                            (None, Some(good)) if rules.goods[index].id == *good => {
                                u128::from(input.amount)
                            }
                            _ => 0,
                        };
                        sum.saturating_add(units)
                    })
                };
                if job
                    .need_goods
                    .iter()
                    .enumerate()
                    .any(|(index, need)| u128::from(*need) > ceiling(index))
                {
                    return Err(refuse(
                        entry(),
                        "needs more of a good than its vehicle could ask",
                    ));
                }
                let met = kind
                    .inputs
                    .iter()
                    .all(|input| match (&input.category, &input.good) {
                        (Some(category), _) => {
                            let supplied = rules.goods.iter().zip(&job.need_goods).fold(
                                0_u128,
                                |sum, (good, need)| {
                                    sum.saturating_add(
                                        good.category_supplied(category, u128::from(*need)),
                                    )
                                },
                            );
                            supplied >= u128::from(input.amount) * 1000
                        }
                        (None, Some(good)) => rules
                            .goods
                            .iter()
                            .position(|item| item.id == *good)
                            .is_some_and(|index| job.need_goods[index] >= input.amount),
                        (None, None) => false,
                    });
                if !met {
                    return Err(refuse(entry(), "needs less than its vehicle is built from"));
                }
                // A job that has started has gathered every input.
                if job.hands.is_some()
                    && (job.stock != job.need_goods || job.animals != job.need_animals)
                {
                    return Err(refuse(entry(), "started without all it needs"));
                }
            }
            // Every saved hour is in the calendar. A deadline made from one (the gathering give-up of a waiting job, the slaughter of a
            // given-up one) may fall beyond the calendar's end, where the clock never reaches it; the arithmetic that makes it saturates.
            if !in_calendar(job.made, 0)
                || job
                    .given_up
                    .is_some_and(|hour| hour < job.made || !in_calendar(hour, 0))
            {
                return Err(refuse(entry(), "has an hour outside the calendar"));
            }
            if let Some(hands) = &job.hands {
                let held = hands
                    .by_building
                    .iter()
                    .try_fold(0_u64, |sum, (_, count)| sum.checked_add(*count));
                let ordered = hands
                    .by_building
                    .windows(2)
                    .all(|pair| pair[0].0 < pair[1].0);
                // The buildings were charged when the build started; people have changed since, so only what cannot change is checked.
                let placed = hands.by_building.iter().all(|(_, count)| *count > 0);
                if hands.pop as usize >= pops
                    || hands.count != kind.hands
                    || held.is_none_or(|held| held > hands.count)
                    || !ordered
                    || !placed
                    || hands
                        .by_building
                        .iter()
                        .any(|(building, _)| *building as usize >= buildings)
                    || hands
                        .paid
                        .iter()
                        .any(|(good, units)| *good as usize >= goods || *units == 0)
                    || hands.started < job.made
                    || !in_calendar(hands.started, 0)
                    || hands.done != hands.started.saturating_add(u64::from(kind.build_hours))
                {
                    return Err(refuse(
                        entry(),
                        "holds hands that do not exist, add up, or fit the build and the calendar",
                    ));
                }
            }
        }
        crate::expedition::validate_expeditions(
            rules,
            state,
            place_index,
            &mut expedition_numbers,
            now,
        )?;
        // Days away are person-days since a building's last payday, kept after the people they were of have changed (a worker who built a
        // cart and died keeps the days until the payday), so they are bounded by what no population reaches, a month of every person the
        // game can hold, which also keeps every sum of them inside 64 bits.
        for ((building, pop), days) in &place.trade.days_away {
            if *building as usize >= buildings
                || *pop as usize >= pops
                || *days == 0
                || *days > MOST_DAYS_AWAY
            {
                return Err(refuse(
                    "days_away".to_owned(),
                    "names a building or pop that does not exist, or more days than any population could have been away",
                ));
            }
        }
    }
    Ok(())
}
