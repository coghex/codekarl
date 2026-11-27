//! Trade, part D: choosing a trade (CK-433; trade spec section 8 with rules 11b.4, 11c.3 and 11d.2, and r5 3.5).
//!
//! Each trader keeps the hour it last looked and looks again when it is due: 720 hours after its last look, 12 hours after an expedition
//! of its has unloaded, the hour a vehicle it was building is done, or when it never has (r134). A look **evaluates exactly** every place
//! the network reaches from home but its neighbours (11d.2 rule 3): for each, the plans of rule 12, each worked as the trip would be,
//! with its clipped itineraries out and back at the pace its load gives, the supplies it needs by a fixed point (rule 4), the noon wait,
//! the exchange at the destination worked forward on a copy of the place, and what the trip gains. The cheapest plan of a destination is
//! kept (T22), the destination with the greatest gain wins, and the trip is made a job if it gains (T4) or a vehicle is built for it if
//! the year's gain pays for the vehicle (T13).
//!
//! The network is the application's: this module asks it for itineraries through [`Ways`] and never sees the map.

use std::collections::{BTreeMap, BTreeSet};

use crate::expedition::{
    Expedition, ExpeditionVehicle, Group, Hours, Leg, Limiter, Mode, Phase, Plan, assign_leg,
    pace_of, species_of, ticks_of, whole,
};
use crate::swap::{self, Visit};
use crate::trade::{self, Look, Trip};
use crate::{DataError, EconomyState, PlaceState, Rules, missing_data};

/// What moves a plan, which says which profile its way is found for: the profile of its boat if it has one (a boat's profile reaches
/// the land as well, by the approach to a landing), else the cart's (a land vehicle or people on foot), and whether it may use only
/// water (11c.3 rule 10).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Party {
    /// The travel profile's id.
    pub profile: String,
    /// Whether the plan is a boat alone: its way has no land leg (CK-402 r10 rule 1).
    pub boat_only: bool,
}

/// Which way of a trip is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// From home to the destination.
    Out,
    /// From the destination to home.
    Back,
}

/// The pace a party moves at on each mode for the load it carries, in thousandths of walking pace (the speeds of a mode scaled by it,
/// 11b.3 rule 6). A mode the party cannot go by with the load has none, which is written 0: the search leaves it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Paces {
    /// On land.
    pub land: u32,
    /// On water.
    pub water: u32,
}

impl Paces {
    /// The same pace on both modes.
    #[must_use]
    pub const fn uniform(pace_pm: u32) -> Self {
        Self {
            land: pace_pm,
            water: pace_pm,
        }
    }

    /// The pace on a mode.
    #[must_use]
    pub const fn on(self, mode: Mode) -> u32 {
        match mode {
            Mode::Land => self.land,
            Mode::Water => self.water,
        }
    }
}

/// One way of a trip as the network gives it: the clipped legs and the point it ends at (11b.3 rule 10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Journey {
    /// The legs, in the order they are travelled.
    pub legs: Vec<Leg>,
    /// The point the way ends at, in fixed-point map units.
    pub arrival: [i64; 2],
}

/// The travel network as a trader's look needs it (CK-402 through the application). Places are named by their ids.
pub trait Ways {
    /// Whether two places are neighbours: under twice the visiting radius apart in a straight line (rule 9 of part E).
    fn neighbours(&self, a: &str, b: &str) -> bool;

    /// Every place the network reaches from `home` for a party, in a fixed order, and from which it reaches home (11d.2 rule 3: found
    /// by one forward and one reverse search from home, with no time cut-off).
    ///
    /// # Errors
    /// A message when the network cannot be searched.
    fn destinations(&mut self, home: &str, party: &Party) -> Result<Vec<String>, String>;

    /// The clipped quickest way between `home` and `other` for a party at the paces it has on each mode (the profile's speeds on each
    /// mode scaled by that mode's pace, 11b.3 rule 6, so the way chosen is the quickest at the paces it will really be travelled at):
    /// out is from home, back is from `other`. `None` when there is none.
    ///
    /// # Errors
    /// A message when the network cannot be searched.
    fn journey(
        &mut self,
        home: &str,
        other: &str,
        party: &Party,
        paces: Paces,
        way: Way,
    ) -> Result<Option<Journey>, String>;

    /// The same way as [`Self::journey`] with the points its legs run through frozen (what a plan carries, and a position after a save
    /// is read from). A look asks it only for the way it chooses: freezing the geometry of every candidate's way would be most of what
    /// looking costs, and the time of a way does not depend on it.
    ///
    /// # Errors
    /// A message when the network cannot be searched.
    fn freeze(
        &mut self,
        home: &str,
        other: &str,
        party: &Party,
        paces: Paces,
        way: Way,
    ) -> Result<Option<Journey>, String>;
}

/// What happens in a look, for whoever watches: the evidence's harness and the cases. The economy keeps no counters of its own; it
/// reports each of these as it happens to the observer it is given (`()` watches nothing), so what looking cost is the harness's to
/// tally (11d.2 rule 3: the due traders, the destinations and plans evaluated).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookEvent {
    /// A trader looks, and why it was due (coincident triggers are one look).
    Look {
        /// It had never looked.
        first: bool,
        /// 720 hours had passed since its last look.
        timer: bool,
        /// Its pending rest look's hour came.
        rest: bool,
        /// A vehicle it was building was done.
        build: bool,
    },
    /// A destination is evaluated.
    Destination,
    /// A plan is worked in full.
    Plan,
    /// A plan is refused before the exchange is worked: its time, its load, its people or nothing to be had.
    Refused,
    /// A trade job is made.
    Trade,
    /// A vehicle build is made.
    Build,
}

/// Whoever watches the looks.
pub trait LookObserver {
    /// Sees one event.
    fn observe(&mut self, event: LookEvent);
}

impl LookObserver for () {
    fn observe(&mut self, _event: LookEvent) {}
}

/// The counts of the looks an observer has seen, for the evidence and the cases.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LookReport {
    /// Traders that looked.
    pub looks: u64,
    /// Of them, those whose look was due by the timer.
    pub by_timer: u64,
    /// Those due by the rest after an expedition.
    pub by_rest: u64,
    /// Those due because a vehicle they were building was done.
    pub by_build: u64,
    /// Those that had never looked.
    pub by_first: u64,
    /// Destinations evaluated, over all the looks and all the parties.
    pub destinations: u64,
    /// Plans worked in full.
    pub plans: u64,
    /// Plans refused before the exchange was worked.
    pub refused: u64,
    /// Trade jobs made.
    pub trades: u64,
    /// Vehicle builds made.
    pub builds: u64,
}

impl LookObserver for LookReport {
    fn observe(&mut self, event: LookEvent) {
        match event {
            LookEvent::Look {
                first,
                timer,
                rest,
                build,
            } => {
                self.looks += 1;
                self.by_first += u64::from(first);
                self.by_timer += u64::from(timer);
                self.by_rest += u64::from(rest);
                self.by_build += u64::from(build);
            }
            LookEvent::Destination => self.destinations += 1,
            LookEvent::Plan => self.plans += 1,
            LookEvent::Refused => self.refused += 1,
            LookEvent::Trade => self.trades += 1,
            LookEvent::Build => self.builds += 1,
        }
    }
}

impl LookReport {
    /// Adds another hour's counts.
    pub fn add(&mut self, other: &Self) {
        self.looks += other.looks;
        self.by_timer += other.by_timer;
        self.by_rest += other.by_rest;
        self.by_build += other.by_build;
        self.by_first += other.by_first;
        self.destinations += other.destinations;
        self.plans += other.plans;
        self.refused += other.refused;
        self.trades += other.trades;
        self.builds += other.builds;
    }
}

/// Why a trader looks this hour; coincident triggers make one look (11c.3 rule 12).
#[derive(Clone, Copy, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "The four triggers are independent and a look can have several at once."
)]
struct Due {
    first: bool,
    timer: bool,
    rest: bool,
    build: bool,
}

impl Due {
    fn any(self) -> bool {
        self.first || self.timer || self.rest || self.build
    }
}

fn due_of(rules: &Rules, place: &PlaceState, pop: usize, now: u64, built: bool) -> Due {
    let mut due = Due {
        build: built,
        ..Due::default()
    };
    match place.trade.looks.get(&trade::convert(pop)) {
        None => due.first = true,
        Some(look) => {
            due.timer = now >= look.last.saturating_add(rules.expedition.look_hours);
            due.rest = look.rest.is_some_and(|rest| now >= rest);
        }
    }
    due
}

/// The hour's looks, in the order of places and then of pops (11b.2 rule 4 step 6): each trader that is due looks once, its last look
/// is set to this hour and any pending rest look is cleared, and a job or a build is made if the look finds a trip worth it. `built`
/// are the `(place, pop)` of the builds that completed this hour.
///
/// # Errors
/// A malformed state or a way the network cannot give. Each look is whole; those made before the error stand.
pub(crate) fn run_looks(
    rules: &Rules,
    state: &mut EconomyState,
    now: u64,
    built: &[(usize, u32)],
    ways: &mut dyn Ways,
    observer: &mut dyn LookObserver,
) -> Result<(), DataError> {
    let built: BTreeSet<(usize, u32)> = built.iter().copied().collect();
    for place in 0..state.places.len() {
        for pop in 0..state.places[place].pops.len() {
            let due = due_of(
                rules,
                &state.places[place],
                pop,
                now,
                built.contains(&(place, trade::convert(pop))),
            );
            if !due.any() {
                continue;
            }
            observer.observe(LookEvent::Look {
                first: due.first,
                timer: due.timer,
                rest: due.rest,
                build: due.build,
            });
            // Every look, whatever it finds, sets its last look and clears the pending rest look (rule 4).
            state.places[place].trade.looks.insert(
                trade::convert(pop),
                Look {
                    last: now,
                    rest: None,
                },
            );
            let decision = Looker {
                rules,
                state,
                now,
                ways,
                observer: &mut *observer,
            }
            .look(place, pop)?;
            match decision {
                Some(Decision::Trade(plan)) => {
                    state.order_trade(rules, place, pop, *plan, now)?;
                    observer.observe(LookEvent::Trade);
                    trade::start_new_job(rules, state, place, now)?;
                }
                Some(Decision::Build { kind, trip }) => {
                    state.order_build_for(rules, place, pop, kind, Some(*trip), now)?;
                    observer.observe(LookEvent::Build);
                    trade::start_new_job(rules, state, place, now)?;
                }
                None => {}
            }
        }
    }
    Ok(())
}

/// What a look decides.
enum Decision {
    /// Carry out a plan with vehicles the trader has free, or on foot.
    Trade(Box<Plan>),
    /// Build a vehicle of the kind for a trip.
    Build { kind: usize, trip: Box<Trip> },
}

/// The kinds of crew a plan may be made with (rule 12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Crew {
    Full,
    Fewest,
    /// On foot, this many people.
    Foot(u64),
}

/// A vehicle of a plan: its kind, and its number if the trader has it free, or none if the plan would have it built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Held {
    kind: usize,
    number: Option<u64>,
}

/// One shape of plan (rule 12): the vehicles and the crew. A destination is tried with each.
#[derive(Clone, Debug)]
struct Shape {
    land: Option<Held>,
    boat: Option<Held>,
    crew: Crew,
    boat_only: bool,
    /// The one mode the way out (or the way back) is restricted to, for a land vehicle with a boat: that direction's load is fitted to the
    /// mode alone, so a way wholly on land (or wholly on water) needs its load to assign on that mode only, each direction by its own
    /// itinerary's legs (11d.2).
    out_only: Option<Mode>,
    back_only: Option<Mode>,
}

impl Shape {
    fn held(&self) -> impl Iterator<Item = &Held> {
        self.land.iter().chain(self.boat.iter())
    }

    fn party(&self, rules: &Rules) -> Party {
        let profile = self
            .boat
            .and_then(|boat| rules.vehicles[boat.kind].water.clone())
            .unwrap_or_else(|| "cart".to_owned());
        Party {
            profile,
            boat_only: self.boat_only,
        }
    }

    /// How many people it sends: on foot, as given; with vehicles, the most any leg needs of the crews of the vehicles that move it,
    /// the full crews or the fewest (11c.2 rule 6).
    fn people(&self, rules: &Rules) -> u64 {
        match self.crew {
            Crew::Foot(count) => count,
            crew => self
                .held()
                .map(|held| {
                    let kind = &rules.vehicles[held.kind];
                    if crew == Crew::Full {
                        kind.crew_full
                    } else {
                        kind.crew_fewest
                    }
                })
                .max()
                .unwrap_or(1),
        }
    }

    /// The kinds of leg a way of the plan may meet: on land only, on land and water, or on water only.
    fn modes_for(&self, way: Way) -> Vec<Mode> {
        let only = match way {
            Way::Out => self.out_only,
            Way::Back => self.back_only,
        };
        if let Some(only) = only {
            vec![only]
        } else if self.boat_only || (self.land.is_none() && self.boat.is_some()) {
            vec![Mode::Water]
        } else if self.boat.is_some() {
            vec![Mode::Land, Mode::Water]
        } else {
            vec![Mode::Land]
        }
    }

    fn vehicles(&self) -> Vec<ExpeditionVehicle> {
        let mut spare = u64::MAX;
        self.held()
            .map(|held| {
                let number = held.number.unwrap_or_else(|| {
                    spare -= 1;
                    spare
                });
                ExpeditionVehicle {
                    number,
                    kind: trade::convert(held.kind),
                    worn_hours: whole(0),
                }
            })
            .collect()
    }

    /// The order the ties of rule 13 take: fewer vehicles, then the kinds in the rules' order (on foot last), then fewer people.
    fn order(&self, rules: &Rules) -> (usize, Vec<usize>, u64) {
        let mut kinds: Vec<usize> = self.held().map(|held| held.kind).collect();
        if kinds.is_empty() {
            kinds.push(usize::MAX);
        }
        (self.held().count(), kinds, self.people(rules))
    }
}

/// The trips a year (r5 3.5, the yearly test): the fewer of the round trips that fit in the year, exactly (a trip of 100.9 hours is 86
/// in 8,760, not 87), and the trips the need left unmet takes, which for each good wanted is its unmet deficit over what a trip brings,
/// rounded up: a single trip that fills the need counts once, not as many as time would allow.
pub(crate) fn trips_in_a_year(year_hours: u64, round_trip: &Hours, unmet: &[(u128, u64)]) -> u64 {
    let (numer, denom) = (round_trip.numer(), round_trip.denom());
    let by_time = if numer.sign() == num_bigint::Sign::Plus {
        u64::try_from(num_bigint::BigInt::from(year_hours) * denom / numer).unwrap_or(u64::MAX)
    } else {
        u64::MAX
    };
    let by_need = unmet
        .iter()
        .map(|(deficit, brings)| {
            u64::try_from(deficit.div_ceil(u128::from(*brings))).unwrap_or(u64::MAX)
        })
        .max()
        .unwrap_or(0);
    by_time.min(by_need)
}

/// A plan worked in full.
struct Worked {
    plan: Plan,
    shape: Shape,
    /// The cost of running it, the gain, and the hours of its round trip for the limit and for the tie (exact).
    cost: i128,
    gain: i128,
    hours: Hours,
    /// The paces its two ways were searched at, so the way chosen can be searched again with its geometry frozen.
    paces: (Paces, Paces),
    /// The kinds the plan would have built.
    new: Vec<usize>,
}

/// What the markets and the pop look like at the look, and what the trips under way have spoken for.
struct Forecasts {
    /// Goods the plans under way are to take from each place, `[place][good]`.
    spoken: Vec<Vec<u64>>,
    /// Goods the plans under way are to bring to each pop, by `(place, pop)`.
    expected: BTreeMap<(usize, u32), Vec<u64>>,
}

impl Forecasts {
    /// A plan gives way to what happened (r5 3.5 item 9): a trip that has traded counts what it has aboard as what home is to expect and
    /// takes nothing more from the place; a given-up job counts for nothing.
    fn new(rules: &Rules, state: &EconomyState) -> Self {
        let goods = rules.goods.len();
        let mut spoken = vec![vec![0_u64; goods]; state.places.len()];
        let mut expected: BTreeMap<(usize, u32), Vec<u64>> = BTreeMap::new();
        let mut add = |home: usize, pop: u32, destination: Option<usize>, wanted: &[u64]| {
            let row = expected
                .entry((home, pop))
                .or_insert_with(|| vec![0; goods]);
            for (slot, units) in row.iter_mut().zip(wanted) {
                *slot = slot.saturating_add(*units);
            }
            if let Some(destination) = destination {
                for (slot, units) in spoken[destination].iter_mut().zip(wanted) {
                    *slot = slot.saturating_add(*units);
                }
            }
        };
        for (home, place) in state.places.iter().enumerate() {
            for job in &place.trade.jobs {
                if job.given_up.is_some() {
                    continue;
                }
                match &job.kind {
                    trade::JobKind::Trade { plan } => {
                        add(home, job.pop, Some(plan.destination as usize), &plan.wanted);
                    }
                    trade::JobKind::Build {
                        trip: Some(trip), ..
                    } => add(home, job.pop, Some(trip.destination as usize), &trip.wanted),
                    _ => {}
                }
            }
            for expedition in &place.trade.expeditions {
                if expedition.traded || expedition.phase == Phase::Homeward {
                    // Home is to expect what is aboard, less its supplies.
                    let aboard: Vec<u64> = expedition.cargo.clone();
                    add(home, expedition.trader, None, &aboard);
                } else {
                    add(
                        home,
                        expedition.trader,
                        Some(expedition.plan.destination as usize),
                        &expedition.plan.wanted,
                    );
                }
            }
        }
        Self { spoken, expected }
    }
}

/// One trader's look.
struct Looker<'a> {
    rules: &'a Rules,
    state: &'a EconomyState,
    now: u64,
    ways: &'a mut dyn Ways,
    observer: &'a mut dyn LookObserver,
}

/// A good the trader wants, with how much (after what is already coming) and how it ranks.
struct Want {
    good: usize,
    deficit: u128,
    tier: u8,
    share_pm: u128,
}

fn price(place: &PlaceState, good: usize) -> u128 {
    u128::from(place.prices_milli[good])
}

fn worth(place: &PlaceState, amounts: &[u64]) -> i128 {
    amounts
        .iter()
        .enumerate()
        .map(|(good, units)| {
            i128::try_from(u128::from(*units) * price(place, good)).unwrap_or(i128::MAX)
        })
        .fold(0_i128, i128::saturating_add)
}

impl Looker<'_> {
    fn limit(&self, place: usize, pop: usize) -> Option<u64> {
        let kind = &self.state.places[place].pops[pop].kind;
        self.rules.expedition.limit_hours.get(kind).copied()
    }

    /// The goods the trader wants, necessities first, the biggest share of its target first (r5 3.5 item 1); a hungry trader plans for
    /// food only (T6).
    fn wants(&self, view: &swap::TraderView, expected: Option<&Vec<u64>>) -> Vec<Want> {
        let mut wants = Vec::new();
        for (good, item) in self.rules.goods.iter().enumerate() {
            if view.hungry && item.hunger_centi == 0 {
                continue;
            }
            let coming = expected.map_or(0, |row| u128::from(row[good]));
            let deficit = view.deficit[good].saturating_sub(coming);
            if deficit == 0 {
                continue;
            }
            let target = view.target[good];
            let share_pm = if target == 0 {
                1000
            } else {
                (deficit * 1000 / target).min(1000)
            };
            let tier = u8::from(item.hunger_centi == 0 && item.category_supplied("fuel", 1) == 0);
            wants.push(Want {
                good,
                deficit,
                tier,
                share_pm,
            });
        }
        wants.sort_by(|a, b| {
            a.tier
                .cmp(&b.tier)
                .then(b.share_pm.cmp(&a.share_pm))
                .then(b.deficit.cmp(&a.deficit))
                .then(a.good.cmp(&b.good))
        });
        wants
    }

    #[expect(
        clippy::too_many_lines,
        reason = "One look: the trader's position, the vehicles it could use, every destination and plan, and the choice, in order."
    )]
    fn look(&mut self, home: usize, pop: usize) -> Result<Option<Decision>, DataError> {
        let rules = self.rules;
        let state = self.state;
        let place = &state.places[home];
        let Some(limit) = self.limit(home, pop) else {
            return Ok(None);
        };
        let forecasts = Forecasts::new(rules, state);
        let view = swap::trader_view(rules, place, pop)?;
        let expected = forecasts.expected.get(&(home, trade::convert(pop)));
        let wants = self.wants(&view, expected);
        if wants.is_empty() {
            return Ok(None);
        }
        let shapes = self.shapes(home, pop);
        if shapes.is_empty() {
            return Ok(None);
        }
        let mut parties: BTreeMap<Party, Vec<String>> = BTreeMap::new();
        for shape in &shapes {
            let party = shape.party(rules);
            if let std::collections::btree_map::Entry::Vacant(slot) = parties.entry(party) {
                let found = self
                    .ways
                    .destinations(&place.id, slot.key())
                    .map_err(|problem| {
                        missing_data("trade.look.destinations".to_owned(), &problem)
                    })?;
                slot.insert(found);
            }
        }
        // Each destination is a place index, not home and not a neighbour; the places in the order of the economy.
        let mut destinations: Vec<usize> = Vec::new();
        for (index, other) in state.places.iter().enumerate() {
            if index != home
                && !self.ways.neighbours(&place.id, &other.id)
                && parties.values().any(|ids| ids.contains(&other.id))
            {
                destinations.push(index);
            }
        }
        let mut best: Option<(i128, Hours, String, Worked)> = None;
        for destination in destinations {
            self.observer.observe(LookEvent::Destination);
            let other = &state.places[destination];
            // What the destination can give and wants today, less what trips under way have spoken for there.
            let (dest_want, dest_spare) = swap::place_totals(rules, other)?;
            let mut cheapest: Option<Worked> = None;
            for shape in &shapes {
                let party = shape.party(rules);
                if !parties
                    .get(&party)
                    .is_some_and(|ids| ids.contains(&other.id))
                {
                    continue;
                }
                let Some(worked) = Evaluation {
                    looker: self,
                    home,
                    pop,
                    destination,
                    limit,
                    view: &view,
                    wants: &wants,
                    dest_want: &dest_want,
                    dest_spare: &dest_spare,
                    spoken: &forecasts.spoken[destination],
                }
                .work(shape)?
                else {
                    continue;
                };
                // A plan that does not gain is no plan (T4): it would only be the cheapest because it does nothing.
                if worked.gain <= 0 {
                    continue;
                }
                // The cheapest running cost; then the greater gain; then fewer vehicles, the kinds' order, fewer people (rule 13).
                let better = cheapest.as_ref().is_none_or(|old| {
                    (worked.cost, -worked.gain, worked.shape.order(rules))
                        < (old.cost, -old.gain, old.shape.order(rules))
                });
                if better {
                    cheapest = Some(worked);
                }
            }
            let Some(worked) = cheapest else {
                continue;
            };
            // Among destinations: the greatest gain, then the shorter round trip, then the place id in sort order (rule 13).
            let key = (worked.gain, &worked.hours, &other.id);
            let better = best.as_ref().is_none_or(|(gain, hours, id, _)| {
                key.0 > *gain
                    || (key.0 == *gain
                        && (key.1 < hours || (key.1 == hours && key.2.as_str() < id.as_str())))
            });
            if better {
                best = Some((worked.gain, worked.hours.clone(), other.id.clone(), worked));
            }
        }
        let Some((_, _, _, worked)) = best else {
            return Ok(None);
        };
        if worked.new.is_empty() {
            let mut plan = worked.plan;
            self.freeze(home, &worked.shape, worked.paces, &mut plan)?;
            return Ok(Some(Decision::Trade(Box::new(plan))));
        }
        // A plan that needs a vehicle the trader lacks was kept only if the year's gain pays for it (T13): build the first it lacks.
        let kind = worked.new[0];
        Ok(Some(Decision::Build {
            kind,
            trip: Box::new(Trip {
                destination: worked.plan.destination,
                cargo: worked.plan.cargo.clone(),
                wanted: worked.plan.wanted.clone(),
            }),
        }))
    }

    /// Gives the way chosen its geometry: the two ways searched again at the paces they were worked at, with the points their legs run
    /// through frozen. Only the way chosen is frozen; what was worked is the same legs without them, so nothing about the plan changes.
    fn freeze(
        &mut self,
        home: usize,
        shape: &Shape,
        paces: (Paces, Paces),
        plan: &mut Plan,
    ) -> Result<(), DataError> {
        let party = shape.party(self.rules);
        let (home_id, other_id) = (
            self.state.places[home].id.clone(),
            self.state.places[plan.destination as usize].id.clone(),
        );
        for (way, pace) in [(Way::Out, paces.0), (Way::Back, paces.1)] {
            let frozen = self
                .ways
                .freeze(&home_id, &other_id, &party, pace, way)
                .map_err(|problem| missing_data("trade.look.freeze".to_owned(), &problem))?
                .ok_or_else(|| {
                    missing_data(
                        "trade.look.freeze".to_owned(),
                        "a way that was worked has no geometry",
                    )
                })?;
            let (legs, arrival) = match way {
                Way::Out => (&mut plan.out, Some(frozen.arrival)),
                Way::Back => (&mut plan.back, None),
            };
            *legs = frozen.legs;
            if arrival.is_some() {
                plan.arrival = arrival;
            }
        }
        Ok(())
    }

    /// The trips a year and whether their gain pays for the vehicles a plan lacks (r5 3.5, the yearly test): the fewer of what time
    /// allows and what the need left unmet allows, so a second vehicle is judged on what the trips already under way leave unmet.
    fn pays_for_vehicles(
        &self,
        home: usize,
        pop: usize,
        wants: &[Want],
        worked: &Worked,
    ) -> Result<bool, DataError> {
        // The need the trips must meet is what the trader lacks (after the trips already coming, `wants`) and the home market cannot give
        // it: what the other pops of its place offer above what they want themselves (T13: the need that cannot be met at home).
        let (_, home_offers) = swap::place_totals(self.rules, &self.state.places[home])?;
        let others_want = swap::market_claims_of_others(self.rules, &self.state.places[home], pop)?;
        let unmet: Vec<(u128, u64)> = worked
            .plan
            .wanted
            .iter()
            .enumerate()
            .filter(|(_, brings)| **brings > 0)
            .map(|(good, brings)| {
                let deficit = wants
                    .iter()
                    .find(|want| want.good == good)
                    .map_or(0, |want| want.deficit);
                let at_home = home_offers[good].saturating_sub(others_want[good]);
                (deficit.saturating_sub(at_home), *brings)
            })
            .collect();
        let trips = trips_in_a_year(self.rules.expedition.year_hours, &worked.hours, &unmet);
        let yearly_gain = worked.gain.saturating_mul(i128::from(trips));
        let mut cost = 0_i128;
        for kind in &worked.new {
            cost = cost.saturating_add(self.vehicle_cost(home, pop, *kind)?);
        }
        Ok(yearly_gain >= cost)
    }

    /// What a vehicle costs the trader to have built: its inputs and animal at home prices and the build's wages (r5 3.5).
    fn vehicle_cost(&self, home: usize, pop: usize, kind: usize) -> Result<i128, DataError> {
        let rules = self.rules;
        let place = &self.state.places[home];
        let definition = &rules.vehicles[kind];
        let mut cost = 0_i128;
        for input in &definition.inputs {
            let (good, units) = match (&input.category, &input.good) {
                (Some(category), _) => {
                    let mut best: Option<(u128, usize, u128)> = None;
                    for (index, item) in rules.goods.iter().enumerate() {
                        let Some(units) = item.units_for_category(category, input.amount) else {
                            continue;
                        };
                        let value = units.saturating_mul(price(place, index));
                        if best.is_none_or(|(least, _, _)| value < least) {
                            best = Some((value, index, units));
                        }
                    }
                    let Some((_, index, units)) = best else {
                        return Ok(i128::MAX);
                    };
                    (index, units)
                }
                (None, Some(good)) => (trade::good_index(rules, good)?, u128::from(input.amount)),
                (None, None) => return Ok(i128::MAX),
            };
            cost = cost.saturating_add(
                i128::try_from(units.saturating_mul(price(place, good))).unwrap_or(i128::MAX),
            );
        }
        for animal in &definition.animals {
            let good = trade::good_index(rules, &animal.species)?;
            cost = cost.saturating_add(
                i128::try_from(u128::from(animal.count).saturating_mul(price(place, good)))
                    .unwrap_or(i128::MAX),
            );
        }
        // The hands' wages: the trader's own are not paid; otherwise the cheapest pop with enough at home, for the build's days.
        let days = u128::from(definition.build_hours.div_ceil(24).max(1));
        let own_home = place.pops[pop]
            .workers
            .saturating_sub(place.trade.held(pop));
        if own_home < definition.hands {
            let wage = (0..place.pops.len())
                .filter(|other| {
                    *other != pop
                        && place.pops[*other]
                            .workers
                            .saturating_sub(place.trade.held(*other))
                            >= definition.hands
                })
                .map(|other| {
                    u128::from(definition.hands)
                        * u128::from(trade::pop_wage(rules, place, other))
                        * days
                        / 30
                        + 1
                })
                .min();
            cost = cost.saturating_add(
                wage.map_or(i128::MAX, |wage| i128::try_from(wage).unwrap_or(i128::MAX)),
            );
        }
        Ok(cost)
    }

    /// The plans to try (rule 12): on foot with 1 to `foot_people` people; or one land vehicle of each kind the trader has free (or
    /// could build), alone or with one boat of each kind it has free or could build; each with its full crew or its fewest; and a boat
    /// alone (11c.3 rule 10). Of two free vehicles of one kind the lower number is used (rule 11).
    fn shapes(&self, home: usize, pop: usize) -> Vec<Shape> {
        let rules = self.rules;
        let place = &self.state.places[home];
        let culture = &place.pops[pop].culture;
        let options = |water: bool| -> Vec<Held> {
            let mut found = Vec::new();
            for (kind, definition) in rules.vehicles.iter().enumerate() {
                let moves = if water {
                    definition.water.is_some()
                } else {
                    definition.land_pace_pm.is_some() && definition.water.is_none()
                };
                if !moves {
                    continue;
                }
                let free = place
                    .trade
                    .vehicles
                    .iter()
                    .filter(|vehicle| {
                        vehicle.pop as usize == pop && vehicle.kind as usize == kind && vehicle.free
                    })
                    .map(|vehicle| vehicle.number)
                    .min();
                if let Some(number) = free {
                    found.push(Held {
                        kind,
                        number: Some(number),
                    });
                } else if definition.cultures.is_empty() || definition.cultures.contains(culture) {
                    found.push(Held { kind, number: None });
                }
            }
            found
        };
        let (lands, boats) = (options(false), options(true));
        let mut shapes = Vec::new();
        for people in 1..=rules.expedition.foot_people {
            shapes.push(Shape {
                land: None,
                boat: None,
                crew: Crew::Foot(people),
                boat_only: false,
                out_only: None,
                back_only: None,
            });
        }
        for crew in [Crew::Full, Crew::Fewest] {
            for land in &lands {
                shapes.push(Shape {
                    land: Some(*land),
                    boat: None,
                    crew,
                    boat_only: false,
                    out_only: None,
                    back_only: None,
                });
                for boat in &boats {
                    // The way out and the way back may each be wholly on land, wholly on water, or both: a trip can carry nine units out
                    // by land and two home by water.
                    for out_only in [None, Some(Mode::Land), Some(Mode::Water)] {
                        for back_only in [None, Some(Mode::Land), Some(Mode::Water)] {
                            shapes.push(Shape {
                                land: Some(*land),
                                boat: Some(*boat),
                                crew,
                                boat_only: false,
                                out_only,
                                back_only,
                            });
                        }
                    }
                }
            }
            for boat in &boats {
                shapes.push(Shape {
                    land: None,
                    boat: Some(*boat),
                    crew,
                    boat_only: true,
                    out_only: None,
                    back_only: None,
                });
            }
        }
        shapes
    }
}

/// The working of one plan at one destination.
struct Evaluation<'a, 'b> {
    looker: &'a mut Looker<'b>,
    home: usize,
    pop: usize,
    destination: usize,
    limit: u64,
    view: &'a swap::TraderView,
    wants: &'a [Want],
    dest_want: &'a [u128],
    dest_spare: &'a [u128],
    spoken: &'a [u64],
}

/// The timing of a trip with a load (11d.2 rule 4): its two ways, the hours of its round trip for the limit, the tick it is home, the
/// noons it eats at, and the hours it moves on land and on water (for the wear).
struct Timing {
    out: Journey,
    back: Journey,
    /// The paces the two ways were searched at.
    paces: (Paces, Paces),
    hours: Hours,
    home_tick: u64,
    land: Hours,
    water: Hours,
    days: u64,
}

/// The people a plan sends, as `(pop, count)`, and the value of the wage the hired ones are paid.
type Hired = (Vec<(u32, u64)>, i128);

/// What an exchange gives a trader: what it receives and what it loads for home.
type Exchanged = (Vec<u64>, Vec<u64>);

/// The supplies dispatched, the cargo fitted to the legs with them aboard, and the trip timed with both.
struct Settled {
    supplies: Vec<u64>,
    cargo: Vec<u64>,
    timing: Timing,
}

/// The most units, up to `most`, for which `fits` holds, found by halving (it assumes `fits(0)`): a plan only has to assign, so a load
/// that fits but is not the largest is as good a plan.
fn halve(most: u128, fits: &dyn Fn(u128) -> bool) -> u128 {
    if fits(most) {
        return most;
    }
    let (mut low, mut high) = (0_u128, most);
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

impl Evaluation<'_, '_> {
    fn rules(&self) -> &Rules {
        self.looker.rules
    }

    fn pace(
        &self,
        vehicles: &[ExpeditionVehicle],
        people: u64,
        cargo: &[u64],
        supplies: &[u64],
        mode: Mode,
    ) -> Option<u32> {
        let rules = self.rules();
        let assigned = assign_leg(rules, vehicles, people, cargo, supplies, mode)?;
        Some(pace_of(rules, vehicles, &assigned, mode, false, vehicles.is_empty()).0)
    }

    /// The way in one direction for a load: the quickest at the pace the load gives on each mode (a mode the party cannot go by with it is
    /// left out of the search), timed leg by leg at the pace of each leg's own mode. `None` when the network gives no way.
    fn way(
        &mut self,
        shape: &Shape,
        vehicles: &[ExpeditionVehicle],
        people: u64,
        cargo: &[u64],
        supplies: &[u64],
        way: Way,
    ) -> Result<Option<(Journey, Hours, Paces)>, DataError> {
        let rules = self.looker.rules;
        let party = shape.party(rules);
        let home_id = self.looker.state.places[self.home].id.clone();
        let other_id = self.looker.state.places[self.destination].id.clone();
        let mut paces = Paces::uniform(0);
        for mode in shape.modes_for(way) {
            let pace = self
                .pace(vehicles, people, cargo, supplies, mode)
                .unwrap_or(0);
            match mode {
                Mode::Land => paces.land = pace,
                Mode::Water => paces.water = pace,
            }
        }
        if paces == Paces::uniform(0) {
            return Ok(None);
        }
        let journey = self
            .looker
            .ways
            .journey(&home_id, &other_id, &party, paces, way)
            .map_err(|problem| missing_data("trade.look.journey".to_owned(), &problem))?;
        let Some(journey) = journey else {
            return Ok(None);
        };
        let mut hours = whole(0);
        for leg in &journey.legs {
            let pace = paces.on(leg.mode);
            if pace == 0 {
                return Ok(None);
            }
            hours += leg.hours(pace);
        }
        Ok(Some((journey, hours, paces)))
    }

    /// The timing of a trip with a load out and a load back (the supplies aboard both ways).
    fn timing(
        &mut self,
        shape: &Shape,
        vehicles: &[ExpeditionVehicle],
        people: u64,
        cargo: &[u64],
        wanted: &[u64],
        supplies: &[u64],
    ) -> Result<Option<Timing>, DataError> {
        let Some((out, out_hours, out_pace)) =
            self.way(shape, vehicles, people, cargo, supplies, Way::Out)?
        else {
            return Ok(None);
        };
        let Some((back, back_hours, back_pace)) =
            self.way(shape, vehicles, people, wanted, supplies, Way::Back)?
        else {
            return Ok(None);
        };
        // The expedition first moves in the hour after the look; it arrives in the tick its travel ends; it trades at the first noon its
        // arrival met the 11:00 cut-off of, and starts back in that noon's step to move in the same tick (11c.1).
        let now = self.looker.now;
        let first = now.saturating_add(1);
        let arrival = first.saturating_add(ticks_of(&out_hours).unwrap_or(u64::MAX)) - 1;
        let day = arrival / 24;
        let noon_day = if arrival % 24 <= 11 {
            day
        } else {
            day.saturating_add(1)
        };
        let noon = noon_day.saturating_mul(24).saturating_add(12);
        let home_tick = noon
            .saturating_add(ticks_of(&back_hours).unwrap_or(u64::MAX))
            .saturating_sub(1);
        let hours = &out_hours + whole(noon - arrival) + &back_hours;
        let (mut land, mut water) = (whole(0), whole(0));
        let mut pace_cache: BTreeMap<(bool, u8), u32> = BTreeMap::new();
        for (is_out, journey, load) in [(true, &out, cargo), (false, &back, wanted)] {
            for leg in &journey.legs {
                let key = (is_out, u8::from(leg.mode == Mode::Water));
                let pace = if let Some(pace) = pace_cache.get(&key) {
                    *pace
                } else {
                    let Some(pace) = self.pace(vehicles, people, load, supplies, leg.mode) else {
                        return Ok(None);
                    };
                    pace_cache.insert(key, pace);
                    pace
                };
                match leg.mode {
                    Mode::Land => land += leg.hours(pace),
                    Mode::Water => water += leg.hours(pace),
                }
            }
        }
        Ok(Some(Timing {
            out,
            back,
            paces: (out_pace, back_pace),
            hours,
            home_tick,
            land,
            water,
            days: (home_tick.saturating_sub(now)).div_ceil(24).max(1),
        }))
    }

    /// The supplies a timing needs: food for the people for every noon the trip eats at, and what its vehicles wear (T23, T25). The
    /// food is the one that costs least for its hunger at home prices.
    fn supplies_for(
        &self,
        timing: &Timing,
        vehicles: &[ExpeditionVehicle],
        people: u64,
    ) -> Option<Vec<u64>> {
        let rules = self.rules();
        let place = &self.looker.state.places[self.home];
        let mut supplies = vec![0_u64; rules.goods.len()];
        let first_noon = {
            let from = self.looker.now.saturating_add(1);
            let day = from - from % 24;
            if from % 24 <= 12 { day + 12 } else { day + 36 }
        };
        let noons = if timing.home_tick >= first_noon {
            (timing.home_tick - 12) / 24 - (first_noon - 12) / 24 + 1
        } else {
            0
        };
        let need_centi = noons
            .checked_mul(people)?
            .checked_mul(rules.expedition.daily_hunger_centi)?;
        if need_centi > 0 {
            let mut best: Option<(u128, usize)> = None;
            for (good, item) in rules.goods.iter().enumerate() {
                if item.hunger_centi == 0 || price(place, good) == 0 {
                    continue;
                }
                // Cost for each hundredth of a person-day, cross-multiplied against the best so far.
                let value = price(place, good) * 1_000_000 / u128::from(item.hunger_centi);
                if best.is_none_or(|(least, _)| value < least) {
                    best = Some((value, good));
                }
            }
            let (_, good) = best?;
            supplies[good] = need_centi.div_ceil(u64::from(rules.goods[good].hunger_centi));
        }
        let unit = whole(rules.expedition.wear_hours);
        for vehicle in vehicles {
            let kind = &rules.vehicles[vehicle.kind as usize];
            let mut worn = whole(0);
            if kind.land_pace_pm.is_some() {
                worn += &timing.land;
            }
            if kind.water.is_some() {
                worn += &timing.water;
            }
            let units = u64::try_from((worn / &unit).floor().to_integer()).ok()?;
            let good = trade::good_index(rules, &kind.wear.good).ok()?;
            supplies[good] = supplies[good].checked_add(units.checked_mul(kind.wear.units)?)?;
        }
        Some(supplies)
    }

    /// The supplies by a fixed point (11d.2 rule 4): the supplies for the bare trip, then again with those aboard, until what is needed
    /// is no more than what is aboard, at most eight times. The load dispatched is that last quantity, and the cargo and the return load
    /// are fitted to the legs with it aboard. `None` when it does not settle or a leg cannot be made.
    fn settle(
        &mut self,
        shape: &Shape,
        vehicles: &[ExpeditionVehicle],
        people: u64,
        wanted: &[u64],
        back: &[u64],
    ) -> Result<Option<Settled>, DataError> {
        let goods = self.rules().goods.len();
        let mut aboard = vec![0_u64; goods];
        for _ in 0..9 {
            let cargo = self.cargo(shape, vehicles, people, wanted, &aboard);
            let load = self.fit_load(shape, vehicles, people, back, &aboard);
            let Some(timing) = self.timing(shape, vehicles, people, &cargo, &load, &aboard)? else {
                return Ok(None);
            };
            let Some(need) = self.supplies_for(&timing, vehicles, people) else {
                return Ok(None);
            };
            if need.iter().zip(&aboard).all(|(need, have)| need <= have) {
                // Settled: the quantity dispatched is the one needed, and every check uses it.
                let cargo = self.cargo(shape, vehicles, people, wanted, &need);
                let load = self.fit_load(shape, vehicles, people, back, &need);
                let Some(timing) = self.timing(shape, vehicles, people, &cargo, &load, &need)?
                else {
                    return Ok(None);
                };
                let Some(again) = self.supplies_for(&timing, vehicles, people) else {
                    return Ok(None);
                };
                if again.iter().zip(&need).all(|(again, need)| again <= need) {
                    return Ok(Some(Settled {
                        supplies: need,
                        cargo,
                        timing,
                    }));
                }
                aboard = again;
                continue;
            }
            aboard = need;
        }
        Ok(None)
    }

    /// Works one plan at the destination in full, or `None` when it is not possible: its time, its load on every leg, its people.
    #[expect(
        clippy::too_many_lines,
        reason = "One plan: people, cargo, the exchange worked forward, the supplies, the limit, the cost and the gain."
    )]
    fn work(&mut self, shape: &Shape) -> Result<Option<Worked>, DataError> {
        let rules = self.looker.rules;
        let state = self.looker.state;
        let home = &state.places[self.home];
        let vehicles = shape.vehicles();
        let people = shape.people(rules);
        let goods = rules.goods.len();
        // The wanted goods: the first tier that the destination can give, each as much as is wanted and can be had there (item 3).
        let mut wanted = vec![0_u64; goods];
        let mut tier: Option<u8> = None;
        for want in self.wants {
            let available =
                self.dest_spare[want.good].saturating_sub(u128::from(self.spoken[want.good]));
            if available == 0 || tier.is_some_and(|tier| want.tier != tier) {
                continue;
            }
            tier = Some(want.tier);
            wanted[want.good] = u64::try_from(want.deficit.min(available)).unwrap_or(u64::MAX);
        }
        if tier.is_none() {
            // Nothing the trader wants is to be had there: a trip gains nothing.
            self.looker.observer.observe(LookEvent::Refused);
            return Ok(None);
        }
        // The trip is timed with a return load, which is what the exchange then brings: the load wanted as far as it fits, worked again
        // with what the exchange gave until the load and the exchange agree (at most four times).
        let mut back = vec![0_u64; goods];
        let mut worked: Option<(Settled, Exchanged)> = None;
        for _ in 0..4 {
            let Some(settled) = self.settle(shape, &vehicles, people, &wanted, &back)? else {
                self.looker.observer.observe(LookEvent::Refused);
                return Ok(None);
            };
            // The round trip is within the limit, the noon wait included (rule 17).
            if settled.timing.hours > whole(self.limit) {
                self.looker.observer.observe(LookEvent::Refused);
                return Ok(None);
            }
            let Some((groups, _)) = self.people_for(people, settled.timing.days)? else {
                self.looker.observer.observe(LookEvent::Refused);
                return Ok(None);
            };
            let fitted = self.fit_load(shape, &vehicles, people, &wanted, &settled.supplies);
            let plan = Plan {
                destination: trade::convert(self.destination),
                out: settled.timing.out.legs.clone(),
                back: settled.timing.back.legs.clone(),
                vehicles: shape.held().filter_map(|held| held.number).collect(),
                people: groups,
                cargo: settled.cargo.clone(),
                wanted: fitted,
                supplies: settled.supplies.clone(),
                arrival: Some(settled.timing.out.arrival),
            };
            let (received, loaded) = self.exchange(&vehicles, &plan)?;
            let agreed = loaded == back;
            back.clone_from(&loaded);
            worked = Some((settled, (received, loaded)));
            if agreed {
                break;
            }
        }
        let Some((settled, (received, loaded))) = worked else {
            return Ok(None);
        };
        self.looker.observer.observe(LookEvent::Plan);
        // The way back is the one for the load it really carries; it must still be within the limit and its supplies enough.
        let Some(timing) = self.timing(
            shape,
            &vehicles,
            people,
            &settled.cargo,
            &loaded,
            &settled.supplies,
        )?
        else {
            return Ok(None);
        };
        if timing.hours > whole(self.limit) {
            return Ok(None);
        }
        let Some(need) = self.supplies_for(&timing, &vehicles, people) else {
            return Ok(None);
        };
        if need
            .iter()
            .zip(&settled.supplies)
            .any(|(need, have)| need > have)
        {
            return Ok(None);
        }
        let Some((groups, wage_cost)) = self.people_for(people, timing.days)? else {
            return Ok(None);
        };
        let plan = Plan {
            destination: trade::convert(self.destination),
            out: timing.out.legs.clone(),
            back: timing.back.legs.clone(),
            vehicles: shape.held().filter_map(|held| held.number).collect(),
            people: groups,
            cargo: settled.cargo.clone(),
            wanted: received.clone(),
            supplies: settled.supplies.clone(),
            arrival: Some(timing.out.arrival),
        };
        // The gain at home prices: what it receives less what it gives up, less its running cost (item 8). What it gives up is its cargo
        // less what comes back unsold.
        let given: Vec<u64> = plan
            .cargo
            .iter()
            .zip(&loaded)
            .zip(&received)
            .map(|((cargo, loaded), received)| {
                cargo.saturating_sub(loaded.saturating_sub(*received))
            })
            .collect();
        let supplies_cost = worth(home, &plan.supplies);
        let own_people = people.saturating_sub(
            plan.people
                .iter()
                .filter(|(pop, _)| *pop as usize != self.pop)
                .map(|(_, count)| *count)
                .sum::<u64>(),
        );
        let own = i128::try_from(
            u128::from(own_people)
                * u128::from(trade::pop_wage(self.rules(), home, self.pop))
                * u128::from(timing.days)
                / 30,
        )
        .unwrap_or(i128::MAX);
        let cost = supplies_cost.saturating_add(wage_cost).saturating_add(own);
        let gain = worth(home, &received)
            .saturating_sub(worth(home, &given))
            .saturating_sub(cost);
        let worked = Worked {
            plan,
            shape: shape.clone(),
            cost,
            gain,
            hours: timing.hours,
            paces: timing.paces,
            new: shape
                .held()
                .filter(|held| held.number.is_none())
                .map(|held| held.kind)
                .collect(),
        };
        // A plan that needs a vehicle the trader lacks is possible only if the year's gain pays for the vehicle (T13).
        if !worked.new.is_empty()
            && (worked.gain <= 0
                || !self
                    .looker
                    .pays_for_vehicles(self.home, self.pop, self.wants, &worked)?)
        {
            return Ok(None);
        }
        Ok(Some(worked))
    }

    /// The weights the worth of a load is divided by (r5 3.5 item 3): each good's deficit, after the trips already coming, as a share of
    /// its target (a trader 40 % short of fish and 20 % short of barley gives the fish twice the share, whatever the units).
    fn share_weights(&self, goods: impl Iterator<Item = (usize, u128)>) -> Vec<(usize, u128)> {
        goods
            .map(|(good, deficit)| (good, share_weight(deficit, self.view.target[good])))
            .collect()
    }

    /// The most of a load that fits every kind of leg of the plan's way with the supplies aboard (a plan only has to assign, so a smaller
    /// load that does is as good a plan). When the whole load does not fit, the carrying capacity is shared as the payment is (see
    /// [`fit_shares`]).
    fn fit_load(
        &self,
        shape: &Shape,
        vehicles: &[ExpeditionVehicle],
        people: u64,
        load: &[u64],
        supplies: &[u64],
    ) -> Vec<u64> {
        let rules = self.rules();
        let modes = shape.modes_for(Way::Back);
        // No more of a good than the carriers can hold at all, whatever the market has to give (the sweep below tries every size up to this).
        let capped: Vec<u64> = load
            .iter()
            .enumerate()
            .map(|(good, units)| {
                let held = modes
                    .iter()
                    .map(|mode| carrier_bound(rules, vehicles, people, good, *mode))
                    .min()
                    .unwrap_or(0);
                (*units).min(crate::expedition::MOST_UNITS).min(held)
            })
            .collect();
        let fits = |trial: &[u64]| {
            modes
                .iter()
                .all(|mode| assign_leg(rules, vehicles, people, trial, supplies, *mode).is_some())
        };
        if fits(&capped) {
            return capped;
        }
        let other = &self.looker.state.places[self.destination];
        let weights = self.share_weights(
            self.wants
                .iter()
                .filter(|want| capped[want.good] > 0)
                .map(|want| (want.good, want.deficit)),
        );
        let only_wanted: Vec<u64> = capped
            .iter()
            .enumerate()
            .map(|(good, units)| {
                if weights.iter().any(|(wanted, _)| *wanted == good) {
                    *units
                } else {
                    0
                }
            })
            .collect();
        let mut fitted = fit_shares(&weights, &only_wanted, |good| price(other, good), &fits);
        // What else comes back (unsold cargo) takes what room is left, good by good.
        for good in 0..fitted.len() {
            if weights.iter().any(|(wanted, _)| *wanted == good) || capped[good] == 0 {
                continue;
            }
            let most = u128::from(capped[good]);
            let tries = |units: u128| {
                let Ok(units) = u64::try_from(units) else {
                    return false;
                };
                let mut trial = fitted.clone();
                trial[good] = units;
                fits(&trial)
            };
            fitted[good] = u64::try_from(halve(most, &tries)).unwrap_or(0);
        }
        fitted
    }

    /// The trader's own people first, the rest from the cheapest other pop with enough at home; the value of the wage (r5 3.4).
    fn people_for(&self, count: u64, days: u64) -> Result<Option<Hired>, DataError> {
        let rules = self.rules();
        let home = &self.looker.state.places[self.home];
        let at_home = |pop: usize| home.pops[pop].workers.saturating_sub(home.trade.held(pop));
        let own = at_home(self.pop).min(count);
        let rest = count - own;
        let mut groups = Vec::new();
        if own > 0 {
            groups.push((trade::convert(self.pop), own));
        }
        if rest == 0 {
            return Ok(Some((groups, 0)));
        }
        let mut options: Vec<(u128, usize)> = Vec::new();
        for other in 0..home.pops.len() {
            if other != self.pop && at_home(other) >= rest {
                let wage = u128::from(rest)
                    .saturating_mul(u128::from(trade::pop_wage(self.rules(), home, other)))
                    .saturating_mul(u128::from(days))
                    / 30
                    + 1;
                options.push((wage, other));
            }
        }
        options.sort_unstable();
        for (wage, other) in options {
            if trade::wage_goods(rules, home, self.pop, other, wage)?.is_some() {
                groups.push((trade::convert(other), rest));
                return Ok(Some((groups, i128::try_from(wage).unwrap_or(i128::MAX))));
            }
        }
        Ok(None)
    }

    /// The cargo out for a load: the trader's spare goods the destination wants that are not wanted back, the goods worth most for
    /// their bulk first, as much of each as the legs carry with the supplies aboard, until the worth of what is wanted is covered.
    fn cargo(
        &self,
        shape: &Shape,
        vehicles: &[ExpeditionVehicle],
        people: u64,
        wanted: &[u64],
        supplies: &[u64],
    ) -> Vec<u64> {
        let rules = self.rules();
        let home = &self.looker.state.places[self.home];
        let other = &self.looker.state.places[self.destination];
        let goods = rules.goods.len();
        let mut cargo = vec![0_u64; goods];
        let cap = {
            let covered = worth(other, wanted);
            let loss = i128::from(rules.swap_loss_limit_pm);
            let most_price = (0..goods).map(|good| price(other, good)).max().unwrap_or(0);
            covered
                .saturating_mul(1000)
                .checked_div(1000 - loss)
                .unwrap_or(covered)
                .saturating_add(i128::try_from(most_price).unwrap_or(0))
        };
        let mut order: Vec<usize> = (0..goods)
            .filter(|good| {
                wanted[*good] == 0
                    && species_of(rules, *good).is_none()
                    && self.view.spare[*good] > 0
                    && self.dest_want[*good] > 0
            })
            .collect();
        order.sort_by(|a, b| {
            let bulk = |good: usize| u128::from(rules.goods[good].volume_ml.max(1));
            (price(home, *b) * bulk(*a))
                .cmp(&(price(home, *a) * bulk(*b)))
                .then(a.cmp(b))
        });
        let modes = shape.modes_for(Way::Out);
        let mut value = 0_i128;
        for good in order {
            let unit_price = i128::try_from(price(other, good)).unwrap_or(i128::MAX);
            if value >= cap {
                break;
            }
            let by_value = if unit_price == 0 {
                u128::MAX
            } else {
                u128::try_from((cap - value) / unit_price + 1).unwrap_or(u128::MAX)
            };
            let most = self.view.spare[good]
                .min(self.dest_want[good])
                .min(by_value)
                .min(u128::from(crate::expedition::MOST_UNITS));
            let fits = |units: u128| {
                let Ok(units) = u64::try_from(units) else {
                    return false;
                };
                let mut trial = cargo.clone();
                trial[good] = units;
                modes.iter().all(|mode| {
                    assign_leg(rules, vehicles, people, &trial, supplies, *mode).is_some()
                })
            };
            // The most that fits, found by halving: a plan only has to assign, and a smaller load that does is as good a plan.
            let units = halve(most, &fits);
            cargo[good] = u64::try_from(units).unwrap_or(0);
            value = value.saturating_add(
                i128::try_from(units.saturating_mul(u128::try_from(unit_price).unwrap_or(0)))
                    .unwrap_or(i128::MAX),
            );
        }
        cargo
    }

    /// The exchange at the destination worked forward on a copy of the place with the trips already spoken for there taken out of its
    /// stock (r5 3.5 items 2 and 3, item 9). Returns what the trader would receive and what it would load for home. When it wants
    /// several goods and cannot have all, the worth of what it receives is divided among them in the ratio of their deficits and the
    /// exchange worked again with those amounts as its wants.
    fn exchange(
        &mut self,
        vehicles: &[ExpeditionVehicle],
        plan: &Plan,
    ) -> Result<Exchanged, DataError> {
        let rules = self.rules();
        let state = self.looker.state;
        let home = &state.places[self.home];
        let other = &state.places[self.destination];
        let mut staged = other.clone();
        swap::take_spoken(rules, &mut staged, self.spoken)?;
        let mut first_plan = plan.clone();
        let mut outcome = self.visit(home, &staged, vehicles, &first_plan)?;
        // The worth of the return load divided in the ratio of the deficits of the goods wanted (item 3).
        let wanted_goods: Vec<usize> = (0..plan.wanted.len())
            .filter(|good| plan.wanted[*good] > 0)
            .collect();
        if wanted_goods.len() > 1 {
            let total: i128 = outcome
                .0
                .iter()
                .enumerate()
                .map(|(good, units)| {
                    i128::try_from(u128::from(*units) * price(other, good)).unwrap_or(i128::MAX)
                })
                .fold(0_i128, i128::saturating_add);
            // The shares of target as they stand after the trips already coming (the shares the wants were ranked by).
            let deficits = self.share_weights(wanted_goods.iter().map(|good| {
                let after = self
                    .wants
                    .iter()
                    .find(|want| want.good == *good)
                    .map_or(self.view.deficit[*good], |want| want.deficit);
                (*good, after)
            }));
            let shares = divide_worth(total, &deficits, &plan.wanted, |good| price(other, good));
            if shares != outcome.0 {
                first_plan.wanted = shares;
                outcome = self.visit(home, &staged, vehicles, &first_plan)?;
            }
        }
        Ok(outcome)
    }

    /// The exchange for one visitor: `(received, loaded for home)`.
    fn visit(
        &self,
        home: &PlaceState,
        stage: &PlaceState,
        vehicles: &[ExpeditionVehicle],
        plan: &Plan,
    ) -> Result<Exchanged, DataError> {
        let rules = self.rules();
        let expedition = Expedition {
            number: u64::MAX,
            trader: trade::convert(self.pop),
            plan: plan.clone(),
            groups: plan
                .people
                .iter()
                .map(|(pop, count)| Group {
                    pop: *pop,
                    count: *count,
                    by_building: Vec::new(),
                    paid: Vec::new(),
                })
                .collect(),
            vehicles: vehicles.to_vec(),
            cargo: plan.cargo.clone(),
            supplies: plan.supplies.clone(),
            pantry_centi: 0,
            gone: false,
            fed: None,
            phase: Phase::Waiting,
            traded: false,
            leg: u32::try_from(plan.out.len()).unwrap_or(u32::MAX),
            done: whole(0),
            pace_pm: rules.expedition.walking_pace_pm,
            limiter: Limiter::Nothing,
            reckoned: Vec::new(),
            reckoned_mode: Mode::Land,
            left: self.looker.now,
            arrived: Some(self.looker.now),
            daily: false,
            loaded: plan.supplies.clone(),
        };
        // The goods the job gathers for the trip have left the trader's holding by the time it trades: the visit counts them aboard once.
        let mut departed = home.clone();
        swap::stage_departure(
            rules,
            &mut departed,
            self.pop,
            &expedition.cargo,
            &expedition.supplies,
        )?;
        let visit = Visit::new(rules, &departed, self.home, expedition)?;
        let done = swap::forecast_exchange(rules, stage, visit)?;
        Ok((done.received().to_vec(), done.expedition.cargo.clone()))
    }
}

/// The most units of a good the carriers of a leg could hold at all (a bound, not a fit: the supplies and the other goods take room too):
/// each carrier takes at most what its weight and its space allow of the good, and a unit no carrier can hold is none. A live animal is
/// driven on land and carried aboard on water, by the packer's own rules ([`crate::expedition::assign_leg`]): on land the people left after
/// each moving vehicle's fewest crew drive at most `animals_per_drover` animals each, and on water each boat holds what its weight and the
/// species' space aboard allow, so the sweep never tries sizes the packer would refuse.
pub(crate) fn carrier_bound(
    rules: &Rules,
    vehicles: &[ExpeditionVehicle],
    people: u64,
    good: usize,
    mode: Mode,
) -> u64 {
    if let Some(species) = species_of(rules, good) {
        return animal_bound(rules, vehicles, people, good, species, mode);
    }
    let item = &rules.goods[good];
    let (weight, space) = (u128::from(item.weight_g), u128::from(item.volume_ml));
    let per = |room_g: u128, room_ml: u128| -> u128 {
        let by_weight = room_g.checked_div(weight).unwrap_or(u128::MAX);
        let by_space = room_ml.checked_div(space).unwrap_or(u128::MAX);
        by_weight.min(by_space)
    };
    let mut total = 0_u128;
    if vehicles.is_empty() {
        if mode == Mode::Water {
            return 0;
        }
        let each = per(
            u128::from(rules.expedition.pack_g),
            u128::from(rules.expedition.pack_ml),
        );
        total = each.saturating_mul(u128::from(people));
    } else {
        for vehicle in vehicles {
            let Some(kind) = rules.vehicles.get(vehicle.kind as usize) else {
                continue;
            };
            let moves = match mode {
                Mode::Land => kind.land_pace_pm.is_some(),
                Mode::Water => kind.water.is_some(),
            };
            if moves {
                total = total
                    .saturating_add(per(u128::from(kind.carries_g), u128::from(kind.carries_ml)));
            }
        }
    }
    u64::try_from(total.min(u128::from(crate::expedition::MOST_UNITS))).unwrap_or(0)
}

/// The most head of a live good the carriers of a leg could hold at all (see [`carrier_bound`]).
fn animal_bound(
    rules: &Rules,
    vehicles: &[ExpeditionVehicle],
    people: u64,
    good: usize,
    species: usize,
    mode: Mode,
) -> u64 {
    let most = u128::from(crate::expedition::MOST_UNITS);
    let total = match mode {
        Mode::Land => {
            // The fewest crew of each vehicle that moves on land come first; the people left drive.
            let crew = vehicles
                .iter()
                .filter_map(|vehicle| rules.vehicles.get(vehicle.kind as usize))
                .filter(|kind| kind.land_pace_pm.is_some())
                .fold(0_u64, |sum, kind| sum.saturating_add(kind.crew_fewest));
            u128::from(people.saturating_sub(crew))
                .saturating_mul(u128::from(rules.expedition.animals_per_drover))
        }
        Mode::Water => {
            let Some(aboard) = rules.expedition.driven(&rules.herd_species[species].animal) else {
                return 0;
            };
            let (weight, space) = (
                u128::from(rules.goods[good].weight_g),
                u128::from(aboard.aboard_ml),
            );
            vehicles
                .iter()
                .filter_map(|vehicle| rules.vehicles.get(vehicle.kind as usize))
                .filter(|kind| kind.water.is_some())
                .fold(0_u128, |sum, kind| {
                    let by_weight = u128::from(kind.carries_g)
                        .checked_div(weight)
                        .unwrap_or(u128::MAX);
                    let by_space = u128::from(kind.carries_ml)
                        .checked_div(space)
                        .unwrap_or(u128::MAX);
                    sum.saturating_add(by_weight.min(by_space))
                })
        }
    };
    u64::try_from(total.min(most)).unwrap_or(0)
}

/// The weight scale of a share of target (parts in a billion).
const SHARE_SCALE: u128 = 1_000_000_000;

/// The weight of a good in a division of worth: its deficit as a share of its target (a good with no target counts whole).
pub(crate) fn share_weight(deficit: u128, target: u128) -> u128 {
    if target == 0 {
        SHARE_SCALE
    } else {
        (deficit.saturating_mul(SHARE_SCALE) / target).max(1)
    }
}

/// The carrying capacity shared like a payment (r5 3.5 item 3), exactly over whole units: the load grows one unit at a time, each time
/// by the good whose worth so far is the smallest share of its weight (so the worths stay in the ratio of the weights as closely as whole
/// units allow, and a tie goes to the biggest weight, then the first good), each cut to its limit.
///
/// What fits is whatever `fits` says, and the packing it asks (first-fit, carrier by carrier) is not monotone: a load that does not fit
/// can be followed by a bigger one that does (four goods on four identical packs: `[3, 3, 4, 1]` is refused and `[3, 3, 5, 1]` is
/// taken), and a unit refused now can fit after another good has grown. So a refusal retires a good only from the unit steps; after
/// them every good is tried at every size up to its limit, the largest that fits is taken, and the steps go on from there, until a pass
/// takes nothing. The load that is left is one to which no good can add any number of units.
pub(crate) fn fit_shares(
    weights: &[(usize, u128)],
    limits: &[u64],
    unit_price: impl Fn(usize) -> u128,
    fits: &impl Fn(&[u64]) -> bool,
) -> Vec<u64> {
    let mut load = vec![0_u64; limits.len()];
    // The worth of a good's units so far, over its weight: compared by cross-multiplication.
    let worth = |load: &[u64], good: usize| u128::from(load[good]) * unit_price(good);
    loop {
        // The unit steps: a good that cannot take one more is out of them until the sweep below.
        let mut open: Vec<(usize, u128)> = weights
            .iter()
            .copied()
            .filter(|(good, _)| load[*good] < limits[*good])
            .collect();
        while !open.is_empty() {
            let mut best = 0;
            for index in 1..open.len() {
                let (good, weight) = open[index];
                let (held, held_weight) = open[best];
                let left = worth(&load, good).saturating_mul(held_weight);
                let right = worth(&load, held).saturating_mul(weight);
                if left < right || (left == right && weight > held_weight) {
                    best = index;
                }
            }
            let (good, _) = open[best];
            load[good] += 1;
            if fits(&load) {
                if load[good] == limits[good] {
                    open.remove(best);
                }
            } else {
                load[good] -= 1;
                open.remove(best);
            }
        }
        // The sweep: every good, every size, the largest that fits.
        let mut order: Vec<(usize, u128)> = weights.to_vec();
        order.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut grew = false;
        for (good, _) in order {
            let held = load[good];
            for more in (1..=limits[good].saturating_sub(held)).rev() {
                load[good] = held + more;
                if fits(&load) {
                    grew = true;
                    break;
                }
                load[good] = held;
            }
        }
        if !grew {
            return load;
        }
    }
}

/// Divides the worth `total` among the goods in the ratio of their deficits, each cut to what can be had (`limits`) with what that frees
/// going to the others in the same ratio, in whole units; value left over by rounding goes to the biggest deficit first (r5 3.5 item 3).
fn divide_worth(
    total: i128,
    deficits: &[(usize, u128)],
    limits: &[u64],
    unit_price: impl Fn(usize) -> u128,
) -> Vec<u64> {
    let mut units = vec![0_u64; limits.len()];
    let mut open: Vec<(usize, u128)> = deficits.to_vec();
    let mut left = total.max(0);
    // A good whose share would pass what can be had is cut to it, and the rest shared again.
    loop {
        let weight: u128 = open.iter().map(|(_, deficit)| *deficit).sum();
        if weight == 0 || open.is_empty() {
            break;
        }
        let mut cut = None;
        for (index, (good, deficit)) in open.iter().enumerate() {
            let share = u128::try_from(left).unwrap_or(0) * deficit / weight;
            let price = unit_price(*good).max(1);
            if share / price > u128::from(limits[*good]) {
                cut = Some(index);
                break;
            }
        }
        if let Some(index) = cut {
            let (good, _) = open.remove(index);
            units[good] = limits[good];
            let spent =
                i128::try_from(u128::from(limits[good]) * unit_price(good)).unwrap_or(i128::MAX);
            left = left.saturating_sub(spent);
        } else {
            for (good, deficit) in &open {
                let share = u128::try_from(left).unwrap_or(0) * deficit / weight;
                units[*good] = u64::try_from(share / unit_price(*good).max(1)).unwrap_or(0);
            }
            break;
        }
    }
    // What rounding left over goes to the biggest deficit first, as whole units that fit under the limits.
    let spent: i128 = units
        .iter()
        .enumerate()
        .map(|(good, count)| {
            i128::try_from(u128::from(*count) * unit_price(good)).unwrap_or(i128::MAX)
        })
        .fold(0_i128, i128::saturating_add);
    let mut over = total.max(0).saturating_sub(spent);
    let mut order: Vec<(usize, u128)> = deficits.to_vec();
    order.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (good, _) in order {
        let price = i128::try_from(unit_price(good).max(1)).unwrap_or(i128::MAX);
        let room = limits[good].saturating_sub(units[good]);
        let more = u64::try_from(over / price).unwrap_or(0).min(room);
        units[good] += more;
        over -= i128::from(more) * price;
    }
    units
}

impl EconomyState {
    /// Orders a build as [`EconomyState::order_build`] does, for the trip `trip` a trader's look has chosen it for (11b.4 rule 14).
    ///
    /// # Errors
    /// As [`EconomyState::order_build`].
    pub(crate) fn order_build_for(
        &mut self,
        rules: &Rules,
        place: usize,
        pop: usize,
        kind: usize,
        trip: Option<Trip>,
        now: u64,
    ) -> Result<u64, DataError> {
        let id = rules
            .vehicles
            .get(kind)
            .ok_or_else(|| missing_data("trade.order_build".to_owned(), "vehicle kind is missing"))?
            .id
            .clone();
        let number = self.order_build(rules, place, pop, &id, now)?;
        if let Some(job) = self.places[place]
            .trade
            .jobs
            .iter_mut()
            .find(|job| job.number == number)
            && let trade::JobKind::Build { trip: slot, .. } = &mut job.kind
        {
            *slot = trip.map(Box::new);
        }
        Ok(number)
    }
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    fn mixed(out_only: Option<Mode>, back_only: Option<Mode>) -> Shape {
        let held = Held {
            kind: 0,
            number: Some(1),
        };
        Shape {
            land: Some(held),
            boat: Some(held),
            crew: Crew::Full,
            boat_only: false,
            out_only,
            back_only,
        }
    }

    #[test]
    fn each_way_of_a_land_vehicle_with_a_boat_has_the_modes_its_own_itinerary_may_use() {
        // A trip can go out wholly by land with the payment fitted to land, and come home wholly by water with the return fitted to
        // water: the two directions are restricted independently (11d.2: feasibility on each itinerary's legs).
        let shape = mixed(Some(Mode::Land), Some(Mode::Water));
        assert_eq!(shape.modes_for(Way::Out), [Mode::Land]);
        assert_eq!(shape.modes_for(Way::Back), [Mode::Water]);
        let free = mixed(None, Some(Mode::Land));
        assert_eq!(free.modes_for(Way::Out), [Mode::Land, Mode::Water]);
        assert_eq!(free.modes_for(Way::Back), [Mode::Land]);
        let mut boat_alone = mixed(None, None);
        boat_alone.land = None;
        boat_alone.boat_only = true;
        assert_eq!(boat_alone.modes_for(Way::Out), [Mode::Water]);
    }
}
