//! The daily swap between two pops of one place (CK-391, trade slice 3a).
//!
//! Once a day, at noon, each place's pops exchange goods: a pop short of a good swaps with a pop in the opposite position, one good
//! each way, in whole units, in near-equal value at the place's prices. A pop's market capacity comes from the buildings it owns, and
//! a swap spends the starter's. Nothing here is saved: spare, want and capacity are worked out afresh at each exchange.

use crate::expedition::Expedition;
use crate::{
    DataError, EconomyState, PlaceState, Rules, building_good_cap, building_market_holding,
    building_market_sells, fuel, herd_wants, live_species, missing_data, mul_div_floor,
    pop_market_holding, pop_store_limits, pop_target_units, receive_building, receive_pop,
    resolve_owner_shares, room_by_cap_and_volume, store_capacity_ml, trade, volume_used,
};

/// Every owner pop's remaining want of each building's herds below standard, `[building][pop][species]` (CK-430): fixed when the
/// exchange begins, and spent by the animals the pops receive, in the buildings' order.
type HerdWants = Vec<Vec<Vec<u64>>>;

/// Runs every place's exchange.
///
/// # Errors
/// The first data error, leaving `state` partly changed: the caller works on a copy.
#[cfg(test)]
pub(crate) fn run_daily_swaps(rules: &Rules, state: &mut EconomyState) -> Result<(), DataError> {
    let prices: Vec<_> = state
        .places
        .iter()
        .map(|p| p.prices_milli.clone())
        .collect();
    run_daily_swaps_at_prices(rules, state, &prices)
}

#[cfg(test)]
pub(crate) fn run_daily_swaps_at_prices(
    rules: &Rules,
    state: &mut EconomyState,
    prices: &[Vec<u64>],
) -> Result<(), DataError> {
    let mut visits = vec![Vec::new(); state.places.len()];
    exchange_phases(
        rules,
        state,
        prices,
        &std::collections::BTreeMap::new(),
        &mut visits,
    )
}

/// Runs both exchange phases over one set of opening books and market capacities.
#[cfg(test)]
pub(crate) fn run_neighbour_swaps_at_prices(
    rules: &Rules,
    state: &mut EconomyState,
    prices: &[Vec<u64>],
    neighbours: &std::collections::BTreeMap<String, Vec<crate::NearbyPlace>>,
) -> Result<(), DataError> {
    let mut visits = vec![Vec::new(); state.places.len()];
    exchange_phases(rules, state, prices, neighbours, &mut visits)
}

/// The noon's exchange at every place, with the expeditions that have arrived at one by 11:00 taking part there (11c.1 rule 3): they are
/// taken from their home places for the exchange, trade, load for home and start back, and are put back (CK-432).
pub(crate) fn run_noon_exchange(
    rules: &Rules,
    state: &mut EconomyState,
    prices: &[Vec<u64>],
    neighbours: &std::collections::BTreeMap<String, Vec<crate::NearbyPlace>>,
    now: u64,
) -> Result<(), DataError> {
    let mut visits = take_visits(rules, state, now)?;
    exchange_phases(rules, state, prices, neighbours, &mut visits)?;
    for visit in visits.into_iter().flatten() {
        let list = &mut state.places[visit.home].trade.expeditions;
        let at = list
            .iter()
            .position(|other| other.number > visit.expedition.number)
            .unwrap_or(list.len());
        list.insert(at, visit.expedition);
    }
    Ok(())
}

/// Takes from every place the expeditions that are waiting at their destination and arrived by 11:00 (before `now`, the noon hour),
/// grouped by destination in the order they arrived and then by number.
fn take_visits(
    rules: &Rules,
    state: &mut EconomyState,
    now: u64,
) -> Result<Vec<Vec<Visit>>, DataError> {
    let mut visits: Vec<Vec<Visit>> = vec![Vec::new(); state.places.len()];
    for home in 0..state.places.len() {
        let mut index = 0;
        while index < state.places[home].trade.expeditions.len() {
            let expedition = &state.places[home].trade.expeditions[index];
            let ready = expedition.phase == crate::Phase::Waiting
                && !expedition.traded
                && expedition.arrived.is_some_and(|hour| hour < now);
            if !ready {
                index += 1;
                continue;
            }
            let destination = expedition.plan.destination as usize;
            let expedition = state.places[home].trade.expeditions.remove(index);
            visits[destination].push(Visit::new(rules, &state.places[home], home, expedition)?);
        }
    }
    for list in &mut visits {
        list.sort_by_key(|visit| (visit.expedition.arrived, visit.expedition.number));
    }
    Ok(visits)
}

/// Runs both exchange phases, the visitors of each place starting first in its local pass.
#[expect(
    clippy::too_many_lines,
    reason = "The two ordered phases share the same opening state and allocations."
)]
fn exchange_phases(
    rules: &Rules,
    state: &mut EconomyState,
    prices: &[Vec<u64>],
    neighbours: &std::collections::BTreeMap<String, Vec<crate::NearbyPlace>>,
    visits: &mut [Vec<Visit>],
) -> Result<(), DataError> {
    if prices.len() != state.places.len() {
        return Err(missing_data(
            "swap.prices".to_owned(),
            "price dimensions do not match places",
        ));
    }
    let count = state.places.len();
    let mut adjacent = vec![vec![false; count]; count];
    for (a, place) in state.places.iter().enumerate() {
        if let Some(list) = neighbours.get(&place.id) {
            for other in list {
                if other.distance_m < 4000
                    && let Some(b) = state.places.iter().position(|p| p.id == other.id)
                    && a != b
                {
                    adjacent[a][b] = true;
                    adjacent[b][a] = true;
                }
            }
        }
    }
    if adjacent
        .iter()
        .all(|row| row.iter().all(|neighbour| !neighbour))
    {
        for (index, (place, opening)) in state.places.iter_mut().zip(prices).enumerate() {
            visits[index] =
                run_place_exchange(rules, place, opening, std::mem::take(&mut visits[index]))?;
        }
        return Ok(());
    }
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|&a, &b| state.places[a].id.cmp(&state.places[b].id));
    let mut days = state
        .places
        .iter()
        .zip(prices)
        .enumerate()
        .map(|(index, (place, opening))| {
            PlaceDay::new_with(rules, place, opening, std::mem::take(&mut visits[index]))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut shares = NeighbourShares::new(&days, &state.places, &adjacent, rules.goods.len())?;
    for second in [false, true] {
        if second {
            shares.second_pass(&days, &adjacent)?;
        }
        for &a in &order {
            days[a].shares.scarce.clone_from(&shares.scarce[a]);
            days[a].shares.left.clone_from(&shares.left[a][a]);
            days[a]
                .shares
                .left_visitor
                .clone_from(&shares.left_visitor[a]);
            days[a].shares.pooled.clone_from(&shares.pooled[a]);
            days[a].local_pass(rules, &mut state.places[a], second)?;
            shares.left[a][a].clone_from(&days[a].shares.left);
            shares.left_visitor[a].clone_from(&days[a].shares.left_visitor);
        }
        for (i, &a) in order.iter().enumerate() {
            for &b in &order[i + 1..] {
                if !adjacent[a][b] {
                    continue;
                }
                let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                let (places_a, places_b) = state.places.split_at_mut(hi);
                let (days_a, days_b) = days.split_at_mut(hi);
                let (low_p, high_p) = (&mut places_a[lo], &mut places_b[0]);
                let (low_d, high_d) = (&mut days_a[lo], &mut days_b[0]);
                if a == lo {
                    neighbour_turn(
                        rules,
                        low_p,
                        high_p,
                        low_d,
                        high_d,
                        &mut shares,
                        a,
                        b,
                        second,
                    )?;
                    neighbour_turn(
                        rules,
                        high_p,
                        low_p,
                        high_d,
                        low_d,
                        &mut shares,
                        b,
                        a,
                        second,
                    )?;
                } else {
                    neighbour_turn(
                        rules,
                        high_p,
                        low_p,
                        high_d,
                        low_d,
                        &mut shares,
                        a,
                        b,
                        second,
                    )?;
                    neighbour_turn(
                        rules,
                        low_p,
                        high_p,
                        low_d,
                        high_d,
                        &mut shares,
                        b,
                        a,
                        second,
                    )?;
                }
            }
        }
    }
    for (index, day) in days.into_iter().enumerate() {
        visits[index] = day.finish(rules, &mut state.places[index])?;
    }
    Ok(())
}

/// What a pop has of a good and what it lacks, against its target (rule 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Position {
    /// The holding above the target.
    pub(crate) spare: u128,
    /// The target less the holding, above zero.
    pub(crate) want: u128,
}

/// A pop's position for a good: the holding is its store plus its storehouse account, the target the one its orders use. The exchange
/// keeps the same figures in its [`Book`]; this reads them afresh from the place.
#[cfg(test)]
pub(crate) fn pop_position(
    rules: &Rules,
    place: &PlaceState,
    pop_index: usize,
    good_index: usize,
) -> Result<Position, DataError> {
    let good = rules
        .goods
        .get(good_index)
        .ok_or_else(|| missing_data("swap.good".to_owned(), "good index is outside rules"))?;
    if good.category_supplied("fuel", 1) > 0 {
        return Ok(Book::new(rules, place, &place.prices_milli)?.position(pop_index, good_index));
    }
    let pop = place
        .pops
        .get(pop_index)
        .ok_or_else(|| missing_data("swap.pop".to_owned(), "pop index is outside place"))?;
    let people = pop
        .workers
        .checked_add(pop.dependents)
        .ok_or_else(|| missing_data("swap.pop.size".to_owned(), "population size overflow"))?;
    let holding = pop_market_holding(place, pop_index, good_index)?;
    let target = pop_target_units(rules, &pop.kind, people, good_index)?;
    Ok(Position {
        spare: holding.saturating_sub(target),
        want: target.saturating_sub(holding),
    })
}

/// Each pop's market capacity for the day in millilitres (rules 5 to 8): a building's levels times its number, split among the pops
/// by their owner shares (largest remainder, the earlier pop on a tie), summed over the pop's buildings.
pub(crate) fn market_capacities(rules: &Rules, place: &PlaceState) -> Result<Vec<u128>, DataError> {
    let mut capacity = vec![0_u128; place.pops.len()];
    let mut resolved = place.clone();
    for (index, building) in place.buildings.iter().enumerate() {
        let per_level = rules
            .buildings
            .iter()
            .find(|definition| definition.id == building.kind)
            .map_or(0, |definition| definition.market_capacity_ml_per_level);
        if per_level == 0 || building.levels == 0 {
            continue;
        }
        if building.employment.len() != place.pops.len() {
            return Err(missing_data(
                format!("places.{}.buildings.{}.employment", place.id, building.kind),
                "employment dimensions do not match place pops",
            ));
        }
        // The building's whole allowance is worked in u128: levels (u32) times the number (u64) cannot overflow it.
        let total = u128::from(building.levels) * u128::from(per_level);
        let weights = resolve_owner_shares(&mut resolved, index)?;
        let shares = wide_shares(total, &weights)
            .map_err(|problem| missing_data("swap.capacity".to_owned(), problem))?;
        for (pop_index, share) in shares.into_iter().enumerate() {
            capacity[pop_index] += share;
        }
    }
    Ok(capacity)
}

/// `largest_remainder` for a total as wide as u128: each share is the total times its weight over the weights' sum, rounded down, and
/// the units left over go one each to the largest remainders, the earlier pop on a tie. The product is never formed whole: the total
/// is divided by the sum first, so any total and any weights that fit are shared exactly.
pub(crate) fn wide_shares(total: u128, weights: &[u64]) -> Result<Vec<u128>, &'static str> {
    let sum = weights.iter().try_fold(0_u128, |sum, weight| {
        sum.checked_add(u128::from(*weight))
            .ok_or("weight sum overflow")
    })?;
    if sum == 0 {
        return Ok(vec![0; weights.len()]);
    }
    let (whole, rest) = (total / sum, total % sum);
    let mut shares = Vec::with_capacity(weights.len());
    let mut remainders = Vec::with_capacity(weights.len());
    let mut assigned = 0_u128;
    for (index, weight) in weights.iter().enumerate() {
        let weight = u128::from(*weight);
        let part = rest.checked_mul(weight).ok_or("share product overflow")?;
        let share = whole * weight + part / sum;
        assigned += share;
        shares.push(share);
        remainders.push((part % sum, index));
    }
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let left = usize::try_from(total - assigned).map_err(|_| "remainder too large")?;
    for (_, index) in remainders.into_iter().take(left) {
        shares[index] += 1;
    }
    Ok(shares)
}

/// The goods in the order a pop works its wants (rule 16): goods with a hunger value first, then the least bulky for its worth (the
/// smaller volume for one value unit at the day's price), equal goods in the goods' order.
#[cfg(test)]
pub(crate) fn want_order(rules: &Rules, prices_milli: &[u64]) -> Vec<usize> {
    want_order_at_prices(rules, prices_milli, prices_milli)
}

fn want_order_at_prices(rules: &Rules, prices_milli: &[u64], opening: &[u64]) -> Vec<usize> {
    let mut order = (0..rules.goods.len()).collect::<Vec<_>>();
    let live = live_species(rules);
    order.sort_by(|&left, &right| {
        let (a, b) = (&rules.goods[left], &rules.goods[right]);
        let group = |g: &crate::Good, index: usize| {
            if g.hunger_centi > 0 {
                0
            } else if g.category_supplied("fuel", 1) > 0 {
                1
            } else if live[index].is_some() {
                // A herd's want for an animal ranks after food and fuel and before the comforts (CK-430, r163).
                2
            } else {
                3
            }
        };
        let food = group(a, left).cmp(&group(b, right));
        if group(a, left) == 1 && group(b, right) == 1 {
            return (u128::from(opening[left]) * b.category_supplied("fuel", 1))
                .cmp(&(u128::from(opening[right]) * a.category_supplied("fuel", 1)))
                .then(left.cmp(&right));
        }
        // volume_a / price_a against volume_b / price_b, cross-multiplied.
        let bulk = (u128::from(a.volume_ml) * u128::from(prices_milli[right]))
            .cmp(&(u128::from(b.volume_ml) * u128::from(prices_milli[left])));
        food.then(bulk).then(left.cmp(&right))
    });
    order
}

/// What every pop of the place holds and aims at for the day's exchange. Targets do not change in it; holdings follow the swaps, a
/// pop's holding being its store and its account together, which a swap moves goods into and out of but never between.
pub(crate) struct Book {
    /// How many rows are the place's pops; the rows after them are the waiting expeditions' (CK-432).
    pops: usize,
    target: Vec<Vec<u128>>,
    holding: Vec<Vec<u128>>,
    /// Whether each pop holds less than 60 days of food, counted once when the exchange begins (rule 18).
    hungry: Vec<bool>,
    fuel: Vec<Vec<Option<Position>>>,
    fuel_prices: Vec<u64>,
    funded: Option<Vec<Vec<crate::budget::Want>>>,
    fuel_data: Vec<(u128, u64)>,
    /// Whether each good is a live animal: a pop's want for one is its herds' shortfall, it holds none, and what it receives joins a
    /// herd (CK-430).
    live: Vec<bool>,
    /// What each pop has been paid into a building's communal stock for an animal sold from its herd, by good (CK-430): spent from
    /// the pop's want of the good, though it is no part of the pop's own holding, so one want is settled once.
    communal: Vec<Vec<u128>>,
    /// What each pop's waiting jobs still lack of each good, which the pop wants on top of its own target and which is spent by the
    /// goods it receives for them (r5 3.4: goods got for a job go to the job). Animals are in the pop's want of the animal.
    lack: Vec<Vec<u128>>,
}

impl Book {
    /// A book of the given targets and holdings, for the tests that set a pair's positions directly.
    #[cfg(test)]
    pub(crate) fn from_parts(target: Vec<Vec<u128>>, holding: Vec<Vec<u128>>) -> Self {
        let hungry = vec![false; target.len()];
        let fuel = target.iter().map(|row| vec![None; row.len()]).collect();
        let live = vec![false; target.first().map_or(0, Vec::len)];
        let (target_height, target_width) = (target.len(), target.first().map_or(0, Vec::len));
        Self {
            pops: target_height,
            target,
            holding,
            fuel,
            fuel_prices: Vec::new(),
            funded: None,
            fuel_data: Vec::new(),
            hungry,
            live,
            communal: vec![vec![0; target_width]; target_height],
            lack: vec![vec![0; target_width]; target_height],
        }
    }

    #[cfg(test)]
    fn new(rules: &Rules, place: &PlaceState, opening: &[u64]) -> Result<Self, DataError> {
        Self::new_with(rules, place, opening, &[])
    }

    /// The day's book with a row after the pops' for each waiting expedition: what it holds to trade is its holding and what it is to
    /// bring home its want, so the shares and the sizes of a swap read it as they read a pop (T45).
    fn new_with(
        rules: &Rules,
        place: &PlaceState,
        opening: &[u64],
        visits: &[Visit],
    ) -> Result<Self, DataError> {
        let rows = place.pops.len() + visits.len();
        let mut book = Self {
            pops: place.pops.len(),
            target: Vec::with_capacity(rows),
            holding: Vec::with_capacity(rows),
            hungry: Vec::with_capacity(rows),
            fuel: Vec::new(),
            fuel_prices: opening.to_vec(),
            funded: Some(crate::budget::orders(rules, place, opening)?),
            fuel_data: rules
                .goods
                .iter()
                .map(|g| (g.category_supplied("fuel", 1), g.base_price))
                .collect(),
            live: live_species(rules).iter().map(Option::is_some).collect(),
            communal: vec![vec![0; rules.goods.len()]; rows],
            lack: vec![vec![0; rules.goods.len()]; rows],
        };
        let live = live_species(rules);
        let wants = herd_wants(rules, place)?;
        let (job_goods, job_animals) = trade::lacks(rules, place);
        for (pop_index, pop) in place.pops.iter().enumerate() {
            let people = pop.workers.checked_add(pop.dependents).ok_or_else(|| {
                missing_data("swap.pop.size".to_owned(), "population size overflow")
            })?;
            let mut target = Vec::with_capacity(rules.goods.len());
            let mut holding = Vec::with_capacity(rules.goods.len());
            // The alms rule's count: less than 60 days of food, the store and the account, the pantry left out.
            let mut food = 0_u128;
            for (good_index, good) in rules.goods.iter().enumerate() {
                let held = pop_market_holding(place, pop_index, good_index)?;
                if good.category_supplied("fuel", 1) > 0 {
                    target.push(0);
                    holding.push(held);
                } else if let Some(species) = live[good_index] {
                    // The want is what the pop's herds lack of their standard levels and its jobs lack of the animal; the pop holds
                    // no animal itself but the ones its given-up jobs have on offer (11b.1).
                    target.push(
                        wants
                            .iter()
                            .map(|building| u128::from(building[pop_index][species]))
                            .sum::<u128>()
                            + u128::from(job_animals[pop_index][species]),
                    );
                    holding.push(u128::from(place.trade.offered(pop_index, species)));
                } else {
                    target.push(pop_target_units(rules, &pop.kind, people, good_index)?);
                    holding.push(held);
                }
                food = held
                    .checked_mul(u128::from(good.hunger_centi))
                    .and_then(|value| food.checked_add(value))
                    .ok_or_else(|| missing_data("swap.food".to_owned(), "food value overflow"))?;
            }
            book.lack[pop_index] = job_goods[pop_index]
                .iter()
                .map(|lack| u128::from(*lack))
                .collect();
            book.hungry
                .push(people > 0 && food < u128::from(people) * 100 * 60);
            book.target.push(target);
            book.holding.push(holding);
        }
        for visit in visits {
            book.target.push(
                visit
                    .expedition
                    .plan
                    .wanted
                    .iter()
                    .map(|units| u128::from(*units))
                    .collect(),
            );
            book.holding
                .push(visit.cargo.iter().map(|units| u128::from(*units)).collect());
            book.hungry.push(false);
        }
        book.fuel = vec![vec![None; rules.goods.len()]; rows];
        for pop in 0..place.pops.len() {
            book.refresh_fuel(rules, place, pop)?;
        }
        Ok(book)
    }

    fn refresh_fuel(
        &mut self,
        rules: &Rules,
        place: &PlaceState,
        pop_index: usize,
    ) -> Result<(), DataError> {
        let pop = &place.pops[pop_index];
        let target = fuel::fuel_daily_need(rules, pop)?
            .checked_mul(240)
            .ok_or_else(|| missing_data("fuel.target".to_owned(), "target overflow"))?;
        let holding = fuel::fuel_holding(rules, &pop.stock, pop.consumption.hearth_milli)?;
        let spare = fuel::fuel_spare(
            rules,
            &pop.stock,
            pop.consumption.hearth_milli,
            target,
            &self.fuel_prices,
        )?;
        // The pop has one fuel shortfall, which each fuel states as a load count. The heat it was paid into a seller's building for an
        // animal (CK-430) spends that shortfall, whichever fuel it was paid in; it is no part of the pop's own holding.
        let paid_heat = rules
            .goods
            .iter()
            .enumerate()
            .map(|(i, good)| self.communal[pop_index][i] * good.category_supplied("fuel", 1))
            .sum::<u128>();
        let short = target.saturating_sub(holding).saturating_sub(paid_heat);
        self.fuel[pop_index] = rules
            .goods
            .iter()
            .enumerate()
            .map(|(i, good)| {
                let supply = good.category_supplied("fuel", 1);
                (supply > 0).then(|| Position {
                    spare: u128::from(spare[i]),
                    want: short.div_ceil(supply),
                })
            })
            .collect();
        Ok(())
    }

    fn raw_position(&self, pop: usize, good: usize) -> Position {
        let lack = self.lack[pop][good];
        if let Some(position) = self.fuel[pop][good] {
            Position {
                spare: position.spare,
                want: position.want + lack,
            }
        } else {
            let paid = self.communal[pop][good];
            let (target, holding) = (self.target[pop][good], self.holding[pop][good]);
            Position {
                spare: holding.saturating_sub(target),
                want: target.saturating_sub(holding).saturating_sub(paid) + lack,
            }
        }
    }

    pub(crate) fn position(&self, pop: usize, good: usize) -> Position {
        let old = self.raw_position(pop, good);
        let Some(rows) = self.funded.as_ref().and_then(|rows| rows.get(pop)) else {
            return old;
        };
        let (supply, base) = self.fuel_data[good];
        let mut heat = 0_u128;
        let mut value = 0_u128;
        let mut want = 0_u128;
        for row in rows {
            match row.destination {
                crate::budget::Destination::Fuel if supply > 0 => {
                    heat = heat.saturating_add(row.funded);
                    value = value.saturating_add(row.funded_value);
                }
                crate::budget::Destination::Household(g)
                | crate::budget::Destination::Herd { good: g, .. }
                | crate::budget::Destination::Job { good: g, .. }
                    if g == good =>
                {
                    want = want.saturating_add(row.funded);
                }
                _ => {}
            }
        }
        Position {
            spare: old.spare,
            want: want.saturating_add(crate::budget::fuel_loads(heat, value, supply, base)),
        }
    }

    /// Follows goods a pop received that went on to its job: they were counted in the pop's holding by [`Self::settle`] and are not
    /// in its store, and the job lacks that much less.
    fn deliver(&mut self, pop: usize, good: usize, units: u64) {
        if units == 0 {
            return;
        }
        let units = u128::from(units);
        self.lack[pop][good] -= units;
        if self.fuel[pop][good].is_none() {
            self.holding[pop][good] -= units;
        }
    }

    /// Follows one side of a made swap; a building payment settles want without entering the pop's holding.
    #[expect(
        clippy::too_many_arguments,
        reason = "A side records both goods, amounts and their source and receipt destination."
    )]
    fn settle_side(
        &mut self,
        pop: usize,
        give_good: usize,
        give: u64,
        own_source: bool,
        get_good: usize,
        get: u64,
        to_building: bool,
    ) {
        if own_source {
            self.holding[pop][give_good] -= u128::from(give);
        }
        if to_building {
            self.communal[pop][get_good] += u128::from(get);
        } else if pop >= self.pops || self.funded.is_none() || !self.live[get_good] {
            // A resident's purchased head belongs to its funded herd or job, not its offer.
            self.holding[pop][get_good] += u128::from(get);
        }
    }

    fn settle(&mut self, trade: Trade) {
        self.settle_side(
            trade.starter,
            trade.give_good,
            trade.give_units,
            trade.starter_source == 0,
            trade.get_good,
            trade.get_units,
            trade.get_to_building.is_some(),
        );
        self.settle_side(
            trade.partner,
            trade.get_good,
            trade.get_units,
            trade.partner_source == 0,
            trade.give_good,
            trade.give_units,
            trade.give_to_building.is_some(),
        );
    }
}

/// Building shares are fixed at the start of the exchange and only decrease.
pub(super) struct BuildingParts {
    pub(super) spare: Vec<Vec<Vec<u128>>>,
    held: Vec<Vec<Vec<u128>>>,
}

impl BuildingParts {
    #[cfg(test)]
    fn new(rules: &Rules, place: &PlaceState, opening: &[u64]) -> Result<Self, DataError> {
        Self::new_with(rules, place, opening, 0)
    }

    /// The buildings' shares with `visitors` rows after the pops' that hold none.
    pub(super) fn new_with(
        rules: &Rules,
        place: &PlaceState,
        opening: &[u64],
        visitors: usize,
    ) -> Result<Self, DataError> {
        let sells = building_market_sells(rules, place, opening)?;
        let mut spare = vec![
            vec![vec![0; rules.goods.len()]; place.pops.len() + visitors];
            place.buildings.len()
        ];
        let mut held = spare.clone();
        let mut resolved = place.clone();
        for (b, building) in place.buildings.iter().enumerate() {
            let weights = resolve_owner_shares(&mut resolved, b)?;
            for good in 0..rules.goods.len() {
                let amount = building_market_holding(rules, place, building, good)?;
                let offer = sells[b][good];
                let shares = wide_shares(offer, &weights)
                    .map_err(|e| missing_data("swap.parts".to_owned(), e))?;
                let holdings = wide_shares(amount, &weights)
                    .map_err(|e| missing_data("swap.parts".to_owned(), e))?;
                for pop in 0..place.pops.len() {
                    spare[b][pop][good] = shares[pop];
                    held[b][pop][good] = holdings[pop];
                }
            }
        }
        Ok(Self { spare, held })
    }

    fn source(&self, book: &Book, source: usize, pop: usize, good: usize) -> u128 {
        if source == 0 {
            book.position(pop, good).spare
        } else {
            self.spare[source - 1][pop][good]
        }
    }

    fn total(&self, book: &Book, pop: usize, good: usize) -> u128 {
        self.spare
            .iter()
            .fold(book.position(pop, good).spare, |sum, b| {
                sum.saturating_add(b[pop][good])
            })
    }
}

/// Daily ceilings for scarce goods. Non-scarce goods have no ceiling.
struct Shares {
    scarce: Vec<bool>,
    left: Vec<Vec<u128>>,
    /// What each claimant may still receive from the expeditions waiting at the place, apart from `left` (what it may receive from the
    /// pops), where the good's stock is not one pool: stock a visitor holds serves only the local claimants, and a claimant's share of
    /// it must not be spent on stock that is reserved for a neighbour (11c.4 rule 15).
    left_visitor: Vec<Vec<u128>>,
    /// Whether a good's shares are one pool (`left` serves every seller) or two (`left` for pops, `left_visitor` for visitors).
    pooled: Vec<bool>,
    claimants: Vec<Vec<bool>>,
    wealth: Vec<(u128, u128)>,
}

impl Shares {
    fn eligible(book: &Book, parts: &BuildingParts, pop: usize, good: usize) -> bool {
        book.position(pop, good).want > 0
            && (0..book.holding.len()).any(|holder| {
                holder != pop
                    // Two waiting expeditions never swap with each other: the exchange runs no such swap.
                    && !(pop >= book.pops && holder >= book.pops)
                    && parts.total(book, holder, good) > 0
                    && (0..book.target[pop].len()).any(|payment| {
                        payment != good
                            && parts.total(book, pop, payment) > 0
                            && book.position(holder, payment).want > 0
                    })
            })
    }

    #[cfg(test)]
    fn new(book: &Book, parts: &BuildingParts, place: &PlaceState) -> Result<Self, DataError> {
        Self::new_with(book, parts, place, &[])
    }

    fn new_with(
        book: &Book,
        parts: &BuildingParts,
        place: &PlaceState,
        visits: &[Visit],
    ) -> Result<Self, DataError> {
        let pops = book.holding.len();
        let goods = place.prices_milli.len();
        let overflow = || missing_data("swap.shares".to_owned(), "share or wealth overflow");
        let mut wealth = Vec::with_capacity(pops);
        for pop in 0..place.pops.len() {
            let mut value = 0_u128;
            for good in 0..goods {
                let mut held = pop_market_holding(place, pop, good)?;
                for building in &parts.held {
                    held = held.checked_add(building[pop][good]).ok_or_else(overflow)?;
                }
                value = held
                    .checked_mul(u128::from(place.prices_milli[good]))
                    .and_then(|v| value.checked_add(v))
                    .ok_or_else(overflow)?;
            }
            let people = Exchange::people(place, pop)?;
            wealth.push((value, u128::from(people.max(1))));
        }
        // A visiting trader's wealth is what its pop holds at home and aboard, at this place's prices (r5 3.7).
        for visit in visits {
            let mut value = 0_u128;
            for (good, held) in visit.holds.iter().enumerate() {
                value = held
                    .checked_mul(u128::from(place.prices_milli[good]))
                    .and_then(|v| value.checked_add(v))
                    .ok_or_else(overflow)?;
            }
            wealth.push((value, u128::from(visit.people.max(1))));
        }
        let mut result = Self {
            scarce: vec![false; goods],
            left: vec![vec![0; goods]; pops],
            left_visitor: vec![vec![0; goods]; pops],
            pooled: vec![true; goods],
            claimants: vec![vec![false; goods]; pops],
            wealth,
        };
        for good in 0..goods {
            let eligible: Vec<bool> = (0..pops)
                .map(|p| Self::eligible(book, parts, p, good))
                .collect();
            let mut demand = 0_u128;
            let mut supply = 0_u128;
            for (p, &is_eligible) in eligible.iter().enumerate() {
                if is_eligible {
                    demand = demand
                        .checked_add(book.position(p, good).want)
                        .ok_or_else(overflow)?;
                }
                supply = supply
                    .checked_add(parts.total(book, p, good))
                    .ok_or_else(overflow)?;
                result.claimants[p][good] = is_eligible;
            }
            if demand > supply {
                result.scarce[good] = true;
                result.allocate(book, good, &eligible, supply, demand)?;
            }
        }
        Ok(result)
    }

    fn allocate(
        &mut self,
        book: &Book,
        good: usize,
        eligible: &[bool],
        supply: u128,
        demand: u128,
    ) -> Result<(), DataError> {
        let overflow = || missing_data("swap.shares".to_owned(), "share overflow");
        let mut assigned = 0_u128;
        for (p, &yes) in eligible.iter().enumerate() {
            let want = book.position(p, good).want;
            let share = if !yes {
                0
            } else if supply >= demand {
                want
            } else {
                mul_div_floor(supply, want, demand).ok_or_else(overflow)?
            };
            self.left[p][good] = share;
            assigned = assigned.checked_add(share).ok_or_else(overflow)?;
        }
        if supply < demand {
            let mut order: Vec<usize> = (0..eligible.len()).filter(|&p| eligible[p]).collect();
            order.sort_by(|&a, &b| {
                let (av, ap) = self.wealth[a];
                let (bv, bp) = self.wealth[b];
                (bv / bp)
                    .cmp(&(av / ap))
                    .then_with(|| ((bv % bp) * ap).cmp(&((av % ap) * bp)))
                    .then(a.cmp(&b))
            });
            let mut remain = supply - assigned;
            for p in order {
                if remain == 0 {
                    break;
                }
                if self.left[p][good] < book.position(p, good).want {
                    self.left[p][good] += 1;
                    remain -= 1;
                }
            }
        }
        Ok(())
    }

    fn second_pass(&mut self, book: &Book, parts: &BuildingParts) -> Result<(), DataError> {
        let overflow = || missing_data("swap.shares".to_owned(), "second share overflow");
        for good in 0..self.scarce.len() {
            if !self.scarce[good] {
                continue;
            }
            let eligible: Vec<bool> = (0..book.holding.len())
                .map(|p| {
                    self.claimants[p][good]
                        && self.left[p][good] == 0
                        && Self::eligible(book, parts, p, good)
                })
                .collect();
            let mut demand = 0_u128;
            let mut supply = 0_u128;
            for (p, &yes) in eligible.iter().enumerate() {
                if yes {
                    demand = demand
                        .checked_add(book.position(p, good).want)
                        .ok_or_else(overflow)?;
                }
                supply = supply
                    .checked_add(parts.total(book, p, good))
                    .ok_or_else(overflow)?;
            }
            self.allocate(book, good, &eligible, supply, demand)?;
        }
        Ok(())
    }

    /// What a claimant may still receive of a good, from a waiting expedition (`from_visitor`) or from a pop.
    fn limit(&self, pop: usize, good: usize, from_visitor: bool) -> u128 {
        if !self.scarce[good] {
            u128::MAX
        } else if from_visitor && !self.pooled[good] {
            self.left_visitor[pop][good]
        } else {
            self.left[pop][good]
        }
    }

    fn receive(&mut self, pop: usize, good: usize, units: u64, from_visitor: bool) {
        if self.scarce[good] {
            let spent = if from_visitor && !self.pooled[good] {
                &mut self.left_visitor[pop][good]
            } else {
                &mut self.left[pop][good]
            };
            *spent -= u128::from(units);
        }
    }
}

/// How many claimant rows of the place `source` take part in the allocation of the place `holder`'s stock: at the holder itself its
/// pops and the expeditions waiting there (CK-432), whose rows come after the pops'; at a neighbour only its pops, since a visitor
/// trades only where it has arrived.
fn rows(days: &[PlaceDay], holder: usize, source: usize) -> usize {
    if source == holder {
        days[source].book.holding.len()
    } else {
        days[source].book.pops
    }
}

/// A claimant of a good's stock at a holder, and which of the two stock pools it may still be given a share of (see
/// [`NeighbourShares::allocate`]).
struct Claim {
    source: usize,
    pop: usize,
    want: u128,
    resident: bool,
    visitor: bool,
}

/// Scarcity allocations for each holder place, over local and directly neighbouring claimants.
struct NeighbourShares {
    scarce: Vec<Vec<bool>>,
    left: Vec<Vec<Vec<Vec<u128>>>>,
    /// For each holder, each local claimant row's share of the stock waiting visitors hold (see [`Shares::left_visitor`]).
    left_visitor: Vec<Vec<Vec<u128>>>,
    /// For each holder and good, whether its shares are one pool or two.
    pooled: Vec<Vec<bool>>,
    claimants: Vec<Vec<Vec<Vec<bool>>>>,
    wealth: Vec<Vec<(u128, u128)>>,
}

impl NeighbourShares {
    /// Whether the seller `seller` of the holder's day could swap a unit of `good` with the claimant `pop` of the day `claimant`: it has
    /// some, and the claimant has something it wants in return. A waiting expedition never swaps with another one.
    fn can_sell(
        days: &[PlaceDay],
        holder: usize,
        seller: usize,
        claimant_day: usize,
        pop: usize,
        good: usize,
    ) -> bool {
        let (origin, claimant) = (&days[holder], &days[claimant_day]);
        if claimant_day == holder
            && (seller == pop || (seller >= origin.book.pops && pop >= origin.book.pops))
        {
            return false;
        }
        origin.parts.total(&origin.book, seller, good) > 0
            && (0..origin.book.target[seller].len()).any(|payment| {
                payment != good
                    && claimant.parts.total(&claimant.book, pop, payment) > 0
                    && origin.book.position(seller, payment).want > 0
            })
    }

    fn eligible(
        days: &[PlaceDay],
        adjacent: &[Vec<bool>],
        holder: usize,
        source: usize,
        pop: usize,
        good: usize,
    ) -> bool {
        let claimant = &days[source];
        if (source != holder && !adjacent[holder][source])
            || claimant.book.position(pop, good).want == 0
        {
            return false;
        }
        // The stock a neighbour can claim is the holder's pops'; a local claimant may also claim a waiting visitor's cargo.
        let sellers = if source == holder {
            days[holder].book.holding.len()
        } else {
            days[holder].book.pops
        };
        (0..sellers).any(|seller| Self::can_sell(days, holder, seller, source, pop, good))
    }

    /// What the visitors waiting at the holder can swap with the local claimants `claimants` (pop and want): the stock of those that
    /// some local claimant could buy from, and for each claimant whether it is one that could. Stock a visitor cannot trade to any
    /// of them covers no demand.
    fn visitor_side(
        days: &[PlaceDay],
        holder: usize,
        good: usize,
        claimants: &[(usize, u128)],
    ) -> (u128, Vec<bool>) {
        let day = &days[holder];
        let mut usable = 0_u128;
        let mut served = vec![false; claimants.len()];
        for seller in day.book.pops..day.book.holding.len() {
            let stock = day.parts.total(&day.book, seller, good);
            let mut any = false;
            for (index, (pop, _)) in claimants.iter().enumerate() {
                if *pop < day.book.pops && Self::can_sell(days, holder, seller, holder, *pop, good)
                {
                    served[index] = true;
                    any = true;
                }
            }
            if any {
                usable = usable.saturating_add(stock);
            }
        }
        (usable, served)
    }

    fn new(
        days: &[PlaceDay],
        places: &[PlaceState],
        adjacent: &[Vec<bool>],
        goods: usize,
    ) -> Result<Self, DataError> {
        let overflow = || missing_data("swap.shares".to_owned(), "share or wealth overflow");
        // Each place's wealth by row is the one its own day's shares worked out, visitors' rows included.
        let wealth: Vec<Vec<(u128, u128)>> =
            days.iter().map(|day| day.shares.wealth.clone()).collect();
        let mut out = Self {
            scarce: vec![vec![false; goods]; places.len()],
            left_visitor: (0..places.len())
                .map(|holder| vec![vec![0; goods]; rows(days, holder, holder)])
                .collect(),
            pooled: vec![vec![true; goods]; places.len()],
            left: places
                .iter()
                .enumerate()
                .map(|(holder, _)| {
                    places
                        .iter()
                        .enumerate()
                        .map(|(source, _)| {
                            if source == holder || adjacent[holder][source] {
                                vec![vec![0; goods]; rows(days, holder, source)]
                            } else {
                                Vec::new()
                            }
                        })
                        .collect()
                })
                .collect(),
            claimants: places
                .iter()
                .enumerate()
                .map(|(holder, _)| {
                    places
                        .iter()
                        .enumerate()
                        .map(|(source, _)| {
                            if source == holder || adjacent[holder][source] {
                                vec![vec![false; goods]; rows(days, holder, source)]
                            } else {
                                Vec::new()
                            }
                        })
                        .collect()
                })
                .collect(),
            wealth,
        };
        for holder in 0..places.len() {
            for good in 0..goods {
                let resident = Self::resident_supply(days, holder, good)?;
                let mut local_claimants: Vec<(usize, u128)> = Vec::new();
                let mut neighbour = 0_u128;
                for source in 0..places.len() {
                    if source != holder && !adjacent[holder][source] {
                        continue;
                    }
                    for pop in 0..rows(days, holder, source) {
                        let eligible = Self::eligible(days, adjacent, holder, source, pop, good);
                        out.claimants[holder][source][pop][good] = eligible;
                        if eligible {
                            let want = days[source].book.position(pop, good).want;
                            if source == holder {
                                local_claimants.push((pop, want));
                            } else {
                                neighbour = neighbour.checked_add(want).ok_or_else(overflow)?;
                            }
                        }
                    }
                }
                let (usable, served) = Self::visitor_side(days, holder, good, &local_claimants);
                let local = local_claimants
                    .iter()
                    .try_fold(0_u128, |sum, (_, want)| sum.checked_add(*want))
                    .ok_or_else(overflow)?;
                let served_want: u128 = local_claimants
                    .iter()
                    .zip(&served)
                    .filter(|(_, served)| **served)
                    .map(|((_, want), _)| *want)
                    .sum();
                if Self::short(resident, usable, local, neighbour, served_want) {
                    out.scarce[holder][good] = true;
                    out.allocate(days, holder, good, resident, false, adjacent)?;
                }
            }
        }
        Ok(out)
    }

    /// What the holder's own pops have of a good to swap: stock every claimant, local or neighbouring, can receive.
    fn resident_supply(days: &[PlaceDay], holder: usize, good: usize) -> Result<u128, DataError> {
        let overflow = || missing_data("swap.shares".to_owned(), "supply overflow");
        let mut resident = 0_u128;
        for pop in 0..days[holder].book.pops {
            resident = resident
                .checked_add(days[holder].parts.total(&days[holder].book, pop, good))
                .ok_or_else(overflow)?;
        }
        Ok(resident)
    }

    /// Whether the claimants want more than the stock they can receive: with no neighbour claiming, everything the place has (a visitor's
    /// stock included, as with no neighbours at all); with neighbours, what visitors hold can serve only the local claimants that could
    /// buy it, so it covers no more of the demand than those make.
    fn short(resident: u128, visitor: u128, local: u128, neighbour: u128, served: u128) -> bool {
        if neighbour == 0 {
            local > resident.saturating_add(visitor)
        } else {
            (local + neighbour).saturating_sub(visitor.min(served)) > resident
        }
    }

    fn second_pass(&mut self, days: &[PlaceDay], adjacent: &[Vec<bool>]) -> Result<(), DataError> {
        let overflow = || missing_data("swap.shares".to_owned(), "second share overflow");
        for holder in 0..days.len() {
            for good in 0..self.scarce[holder].len() {
                if !self.scarce[holder][good] {
                    continue;
                }
                let resident = Self::resident_supply(days, holder, good)?;
                self.allocate(days, holder, good, resident, true, adjacent)
                    .map_err(|_| overflow())?;
            }
        }
        Ok(())
    }

    /// Divides the stock of a good among the claimants that can still claim it (`second`: those that have no share left) and sets each
    /// claimant's limit. Stock only the local claimants can receive (what visitors hold) goes to them first, in proportion to their
    /// wants, and the stock every claimant can receive is shared over what is left of the wants, the whole units left over going to
    /// the wealthiest first (T44).
    #[expect(
        clippy::too_many_lines,
        reason = "Collecting the claimants of each pool, the visitors' stage and the pops' stage are one allocation, kept together."
    )]
    fn allocate(
        &mut self,
        days: &[PlaceDay],
        holder: usize,
        good: usize,
        resident: u128,
        second: bool,
        adjacent: &[Vec<bool>],
    ) -> Result<(), DataError> {
        let overflow = || missing_data("swap.shares".to_owned(), "share overflow");
        // A claimant takes part in redistributing each stock pool on its own (the pops' stock, and the stock waiting visitors hold)
        // when it has none of that pool's allowance left; what it keeps of the other pool counts against its want (11c.4 rule 15).
        let mut claimants: Vec<Claim> = Vec::new();
        // What the neighbouring claimants that can claim want, which decides at the opening whether the holder's stock is one pool or two.
        let mut neighbouring = 0_u128;
        for source in 0..days.len() {
            if source != holder && !adjacent[holder][source] {
                continue;
            }
            for pop in 0..rows(days, holder, source) {
                let kept_resident = self.left[holder][source][pop][good];
                let kept_visitor = if source == holder {
                    self.left_visitor[holder][pop][good]
                } else {
                    0
                };
                // Allowances are set afresh: one with some of a pool's allowance left in the second pass forfeits it and takes no part in
                // that pool's redistribution; one with none left takes part.
                self.left[holder][source][pop][good] = 0;
                if source == holder {
                    self.left_visitor[holder][pop][good] = 0;
                }
                let base = self.claimants[holder][source][pop][good]
                    && (!second || Self::eligible(days, adjacent, holder, source, pop, good));
                let resident = base && (!second || kept_resident == 0);
                let visitor = base && source == holder && (!second || kept_visitor == 0);
                if base && source != holder {
                    neighbouring = neighbouring
                        .checked_add(days[source].book.position(pop, good).want)
                        .ok_or_else(overflow)?;
                }
                if resident || visitor {
                    claimants.push(Claim {
                        source,
                        pop,
                        want: days[source].book.position(pop, good).want,
                        resident,
                        visitor,
                    });
                }
            }
        }
        claimants.sort_by(|a, b| {
            let (av, an) = self.wealth[a.source][a.pop];
            let (bv, bn) = self.wealth[b.source][b.pop];
            (bv / bn)
                .cmp(&(av / an))
                .then_with(|| ((bv % bn) * an).cmp(&((av % an) * bn)))
                .then((a.source, a.pop).cmp(&(b.source, b.pop)))
        });
        // One pool or two is fixed at the opening and kept through both passes, whoever has stopped claiming since.
        let pooled = if second {
            self.pooled[holder][good]
        } else {
            neighbouring == 0
        };
        self.pooled[holder][good] = pooled;
        let mut taken = vec![0_u128; claimants.len()];
        let shared = if pooled {
            // With nobody neighbouring to claim, every stock of the holder, the visitors' included, is the place's like any other.
            let day = &days[holder];
            let visitors = (day.book.pops..day.book.holding.len())
                .try_fold(0_u128, |sum, row| {
                    sum.checked_add(day.parts.total(&day.book, row, good))
                })
                .ok_or_else(overflow)?;
            resident.checked_add(visitors).ok_or_else(overflow)?
        } else {
            // With neighbours, what visitors hold serves only the local claimants that could buy it, and goes to them first, in
            // proportion to their wants.
            let locals: Vec<(usize, u128)> = claimants
                .iter()
                .filter(|claim| claim.source == holder && claim.visitor)
                .map(|claim| (claim.pop, claim.want))
                .collect();
            let (usable, served) = Self::visitor_side(days, holder, good, &locals);
            let mut next_local = 0;
            let mut wants = Vec::with_capacity(claimants.len());
            for claim in &claimants {
                if claim.source == holder && claim.visitor {
                    let reachable = served[next_local];
                    next_local += 1;
                    wants.push(if reachable { claim.want } else { 0 });
                } else {
                    wants.push(0);
                }
            }
            let served_total: u128 = wants.iter().sum();
            taken = Self::split(usable.min(served_total), &wants)?;
            resident
        };
        // The shared stock, over what each still wants of it.
        let wants: Vec<u128> = claimants
            .iter()
            .zip(&taken)
            .map(|(claim, taken)| {
                if claim.resident {
                    claim.want.saturating_sub(*taken)
                } else {
                    0
                }
            })
            .collect();
        let rest = Self::split(shared, &wants)?;
        for ((claim, first), second_share) in claimants.iter().zip(&taken).zip(&rest) {
            if claim.resident {
                self.left[holder][claim.source][claim.pop][good] = *second_share;
            }
            if claim.source == holder && claim.visitor {
                self.left_visitor[holder][claim.pop][good] = *first;
            }
        }
        Ok(())
    }

    /// `supply` over `wants` in proportion, each share rounded down and no more than its want, and the whole units left over one
    /// each, in the order the wants are listed (the wealthiest first), to those still short of their want.
    fn split(supply: u128, wants: &[u128]) -> Result<Vec<u128>, DataError> {
        let overflow = || missing_data("swap.shares".to_owned(), "share overflow");
        let demand = wants
            .iter()
            .try_fold(0_u128, |sum, want| sum.checked_add(*want))
            .ok_or_else(overflow)?;
        if supply >= demand {
            return Ok(wants.to_vec());
        }
        let mut shares = Vec::with_capacity(wants.len());
        for want in wants {
            shares.push(mul_div_floor(supply, *want, demand).ok_or_else(overflow)?);
        }
        let mut remain = supply.saturating_sub(shares.iter().sum::<u128>());
        for (share, want) in shares.iter_mut().zip(wants) {
            if remain == 0 {
                break;
            }
            if *share < *want {
                *share += 1;
                remain -= 1;
            }
        }
        Ok(shares)
    }
}

/// One swap: the starter's pop, the partner's pop, what the starter gives and what it receives.
#[derive(Clone, Copy)]
struct Trade {
    starter: usize,
    partner: usize,
    give_good: usize,
    give_units: u64,
    get_good: usize,
    get_units: u64,
    starter_source: usize,
    partner_source: usize,
    /// The building whose herd sold the animal the starter gives, which the starter's receipt goes to instead of its own store.
    get_to_building: Option<usize>,
    /// The building whose herd sold the animal the partner gives, which the partner's receipt goes to instead of its own store.
    give_to_building: Option<usize>,
}

pub(crate) struct Exchange<'a> {
    pub(crate) rules: &'a Rules,
    /// The place's storehouse, where it has one: a pop's goods given leave its account there after its store.
    pub(crate) house: Option<usize>,
}

impl Exchange<'_> {
    fn people(place: &PlaceState, pop_index: usize) -> Result<u64, DataError> {
        let pop = &place.pops[pop_index];
        pop.workers
            .checked_add(pop.dependents)
            .ok_or_else(|| missing_data("swap.pop.size".to_owned(), "population size overflow"))
    }

    /// The units of a good a pop's own store can take as it stands (rule 13): what the good's limit leaves, and what the store's free
    /// volume holds. A store at its limit, full or over its volume takes none.
    fn room(&self, place: &PlaceState, pop: usize, good_index: usize) -> Result<u128, DataError> {
        // What the pop's jobs lack goes straight into their stockpiles, past the store, so it is room beyond the store's own (r5 3.4).
        Ok(self
            .store_room(place, pop, good_index)?
            .saturating_add(u128::from(trade::lack_of(place, pop, good_index))))
    }

    /// The units of a good a pop's own store can take as it stands.
    fn store_room(
        &self,
        place: &PlaceState,
        pop: usize,
        good_index: usize,
    ) -> Result<u128, DataError> {
        if self.live(good_index).is_some() {
            // An animal never enters a store (its limit is zero); it joins a herd, and the want bounds what comes.
            return Ok(u128::MAX);
        }
        let people = Self::people(place, pop)?;
        let stock = &place.pops[pop].stock;
        let (good_cap, capacity) = pop_store_limits(self.rules, people, good_index)?;
        let by_limit = u128::from(good_cap.saturating_sub(stock[good_index]));
        if self.rules.goods[good_index].stacked_outside {
            return Ok(by_limit);
        }
        let volume = u128::from(self.rules.goods[good_index].volume_ml);
        // A store that is full or over its volume takes nothing, a good that takes no room included (rule 13).
        let free = capacity.saturating_sub(volume_used(self.rules, stock, None)?);
        if free == 0 {
            return Ok(0);
        }
        if volume == 0 {
            return Ok(by_limit);
        }
        Ok(by_limit.min(free / volume))
    }

    fn funded_route(
        &self,
        place: &PlaceState,
        book: &Book,
        pop: usize,
        good: usize,
        limit: u128,
        to_building: Option<usize>,
    ) -> Result<Option<crate::budget::Route>, DataError> {
        let Some(rows) = book.funded.as_ref().and_then(|rows| rows.get(pop)) else {
            return Ok(None);
        };
        let room = if let Some(building) = to_building {
            self.building_room(place, building, good)?
        } else {
            self.store_room(place, pop, good)?
        };
        let data = &self.rules.goods[good];
        let route = crate::budget::route(
            rows,
            good,
            data.category_supplied("fuel", 1),
            data.base_price,
            limit,
            room,
            to_building.is_some(),
            |destination| match destination {
                crate::budget::Destination::Job { job, good, species } => {
                    place.trade.jobs.get(job).map_or(0, |job| {
                        u128::from(
                            species
                                .map_or_else(|| job.lack(good), |species| job.animal_lack(species)),
                        )
                    })
                }
                crate::budget::Destination::Herd { .. } => u128::MAX,
                _ => 0,
            },
        )?;
        Ok(Some(route))
    }

    fn ensure_route(route: Option<&crate::budget::Route>, units: u64) -> Result<(), DataError> {
        if route.is_some_and(|route| route.units != u128::from(units)) {
            return Err(missing_data(
                "swap.route".into(),
                "funded route is shorter than the swap",
            ));
        }
        Ok(())
    }

    fn funded_room(
        &self,
        place: &PlaceState,
        book: &Book,
        pop: usize,
        good: usize,
        building: Option<usize>,
    ) -> Result<u128, DataError> {
        if let Some(route) = self.funded_route(place, book, pop, good, u128::MAX, building)? {
            return Ok(route.units);
        }
        if let Some(building) = building {
            self.building_room(place, building, good)
        } else {
            self.room(place, pop, good)
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "The fixed route names the book, place, receipt and destination together."
    )]
    fn receive_funded(
        &self,
        place: &mut PlaceState,
        book: &mut Book,
        herds: &mut HerdWants,
        pop: usize,
        good: usize,
        units: u64,
        to_building: Option<usize>,
        route: Option<crate::budget::Route>,
    ) -> Result<u64, DataError> {
        let Some(route) = route else {
            return self.receive(place, herds, pop, good, units, to_building);
        };
        if route.units != u128::from(units) {
            return Err(missing_data(
                "swap.route".into(),
                "funded route is shorter than the swap",
            ));
        }
        let mut job_goods = 0_u64;
        for (destination, amount) in route.deliveries {
            let amount = u64::try_from(amount)
                .map_err(|_| missing_data("swap.route".into(), "receipt exceeds u64"))?;
            match destination {
                crate::budget::Destination::Fuel | crate::budget::Destination::Household(_) => {
                    let rejected = if let Some(building) = to_building {
                        receive_building(self.rules, &mut place.buildings[building], good, amount)?
                    } else {
                        receive_pop(self.rules, &mut place.pops[pop], good, amount)?
                    };
                    if rejected != 0 {
                        return Err(missing_data("swap.route".into(), "destination lost room"));
                    }
                }
                crate::budget::Destination::Job { job, good, species } => {
                    let job = &mut place.trade.jobs[job];
                    if let Some(species) = species {
                        job.animals[species] =
                            job.animals[species].checked_add(amount).ok_or_else(|| {
                                missing_data("swap.route".into(), "job animals overflow")
                            })?;
                    } else {
                        job.stock[good] = job.stock[good].checked_add(amount).ok_or_else(|| {
                            missing_data("swap.route".into(), "job stock overflow")
                        })?;
                        job_goods = job_goods.checked_add(amount).ok_or_else(|| {
                            missing_data("swap.route".into(), "job receipts overflow")
                        })?;
                    }
                }
                crate::budget::Destination::Herd {
                    building, species, ..
                } => {
                    place.buildings[building].herds[species] = place.buildings[building].herds
                        [species]
                        .checked_add(amount)
                        .ok_or_else(|| missing_data("swap.route".into(), "herd overflow"))?;
                    herds[building][pop][species] = herds[building][pop][species]
                        .checked_sub(amount)
                        .ok_or_else(|| missing_data("swap.route".into(), "herd want underflow"))?;
                }
            }
        }
        book.funded.as_mut().expect("funded route has a book")[pop] = route.rows;
        Ok(job_goods)
    }

    /// The herd species a good is the live animal of.
    fn live(&self, good_index: usize) -> Option<usize> {
        let id = &self.rules.goods.get(good_index)?.id;
        self.rules
            .herd_species
            .iter()
            .position(|species| species.animal == *id)
    }

    /// The units of a good a building's own stock can take as it stands, for a payment into its communal account (CK-430). A
    /// storehouse is never a herd's building and takes none.
    fn building_room(
        &self,
        place: &PlaceState,
        building: usize,
        good_index: usize,
    ) -> Result<u128, DataError> {
        let building = &place.buildings[building];
        if building.storehouse_accounts.is_some() {
            return Ok(0);
        }
        let room = room_by_cap_and_volume(
            self.rules,
            &building.stock,
            good_index,
            building_good_cap(self.rules, building, good_index)?,
            store_capacity_ml(building)?,
            None,
            u64::MAX,
        )?;
        Ok(u128::from(room))
    }

    /// Takes `units` of a good from a pop: its store first, then its storehouse account (rule 13).
    fn pay(
        &self,
        place: &mut PlaceState,
        pop: usize,
        good_index: usize,
        units: u64,
    ) -> Result<(), DataError> {
        let from_store = place.pops[pop].stock[good_index].min(units);
        let from_account = units - from_store;
        if from_account > 0 {
            let short = || missing_data("swap.pair".to_owned(), "a pop gives more than it holds");
            let house = self.house.ok_or_else(short)?;
            let building = &mut place.buildings[house];
            let held = building
                .storehouse_accounts
                .as_mut()
                .and_then(|accounts| accounts.pops.get_mut(pop))
                .and_then(|account| account.get_mut(good_index))
                .ok_or_else(short)?;
            if *held < from_account {
                return Err(short());
            }
            *held -= from_account;
            building.stock[good_index] -= from_account;
        }
        place.pops[pop].stock[good_index] -= from_store;
        Ok(())
    }

    fn pay_source(
        &self,
        place: &mut PlaceState,
        source: usize,
        pop: usize,
        good: usize,
        units: u64,
    ) -> Result<(), DataError> {
        if let Some(species) = self.live(good) {
            if source == 0 {
                // An animal the pop's given-up jobs have on offer, the oldest job's first (11b.1).
                let mut left = units;
                for job in &mut place.trade.jobs {
                    let taken = if job.given_up.is_some() && job.pop as usize == pop {
                        job.animals[species].min(left)
                    } else {
                        0
                    };
                    job.animals[species] -= taken;
                    left -= taken;
                }
                if left != 0 {
                    return Err(missing_data(
                        "swap.pair".to_owned(),
                        "a pop offers an animal no given-up job holds",
                    ));
                }
                return Ok(());
            }
            // An animal given leaves its herd, which only a building holds; the herd keeps the head count.
            let herd = source
                .checked_sub(1)
                .and_then(|building| place.buildings.get_mut(building))
                .and_then(|building| building.herds.get_mut(species))
                .ok_or_else(|| {
                    missing_data(
                        "swap.pair".to_owned(),
                        "a pop gives an animal it has no herd for",
                    )
                })?;
            *herd = herd.checked_sub(units).ok_or_else(|| {
                missing_data("swap.parts".to_owned(), "herd head count underflow")
            })?;
            return Ok(());
        }
        if source == 0 {
            return self.pay(place, pop, good, units);
        }
        let building = &mut place.buildings[source - 1];
        if let Some(accounts) = &mut building.storehouse_accounts {
            accounts.communal[good] = accounts.communal[good]
                .checked_sub(units)
                .ok_or_else(|| missing_data("swap.parts".to_owned(), "communal stock underflow"))?;
        }
        building.stock[good] = building.stock[good]
            .checked_sub(units)
            .ok_or_else(|| missing_data("swap.parts".to_owned(), "building stock underflow"))?;
        Ok(())
    }

    /// Makes a swap: both loads leave their pops, and what each pop receives goes into its own store and nowhere else (rule 13).
    fn apply(
        &self,
        place: &mut PlaceState,
        book: &mut Book,
        herds: &mut HerdWants,
        trade: Trade,
    ) -> Result<[u64; 2], DataError> {
        let starter_route = self.funded_route(
            place,
            book,
            trade.starter,
            trade.get_good,
            u128::from(trade.get_units),
            trade.get_to_building,
        )?;
        let partner_route = self.funded_route(
            place,
            book,
            trade.partner,
            trade.give_good,
            u128::from(trade.give_units),
            trade.give_to_building,
        )?;
        Self::ensure_route(starter_route.as_ref(), trade.get_units)?;
        Self::ensure_route(partner_route.as_ref(), trade.give_units)?;
        self.pay_source(
            place,
            trade.starter_source,
            trade.starter,
            trade.give_good,
            trade.give_units,
        )?;
        self.pay_source(
            place,
            trade.partner_source,
            trade.partner,
            trade.get_good,
            trade.get_units,
        )?;
        let starter = self.receive_funded(
            place,
            book,
            herds,
            trade.starter,
            trade.get_good,
            trade.get_units,
            trade.get_to_building,
            starter_route,
        )?;
        let partner = self.receive_funded(
            place,
            book,
            herds,
            trade.partner,
            trade.give_good,
            trade.give_units,
            trade.give_to_building,
            partner_route,
        )?;
        Ok([starter, partner])
    }

    /// Receives one side of a trade at that side's place. What the pop gets for its jobs goes straight to the oldest job that lacks it
    /// (r5 3.4), past its store; the rest goes to the store. Returns the units that went to jobs.
    fn receive(
        &self,
        place: &mut PlaceState,
        herds: &mut HerdWants,
        pop: usize,
        good: usize,
        units: u64,
        to_building: Option<usize>,
    ) -> Result<u64, DataError> {
        if let Some(species) = self.live(good) {
            Self::receive_head(place, herds, pop, species, units)?;
            return Ok(0);
        }
        let mut delivered = 0;
        let unaccepted = if let Some(building) = to_building {
            receive_building(self.rules, &mut place.buildings[building], good, units)?
        } else {
            delivered = trade::deliver_good(place, pop, good, units);
            receive_pop(self.rules, &mut place.pops[pop], good, units - delivered)?
        };
        if unaccepted != 0 {
            return Err(missing_data(
                "swap.pair".to_owned(),
                "an allowed pair no longer has room",
            ));
        }
        Ok(delivered)
    }

    /// Adds `units` animals of a species that a pop received to the herds it wants them for: the first herd, in the buildings' order,
    /// where the pop's want is not spent (11b.1). A head is never lost or duplicated, and no herd goes past its standard level.
    fn receive_head(
        place: &mut PlaceState,
        herds: &mut HerdWants,
        pop: usize,
        species: usize,
        units: u64,
    ) -> Result<(), DataError> {
        // A head goes first to the trader's oldest job that still lacks the species (11b.1 rule 1).
        let mut left = units;
        for job in &mut place.trade.jobs {
            let taken = job.animal_lack(species).min(left);
            if job.pop as usize == pop && taken > 0 {
                job.animals[species] += taken;
                left -= taken;
            }
        }
        for (building, wants) in place.buildings.iter_mut().zip(herds.iter_mut()) {
            if left == 0 {
                break;
            }
            let taken = wants[pop][species].min(left);
            if taken == 0 {
                continue;
            }
            let herd = building.herds.get_mut(species).ok_or_else(|| {
                missing_data("swap.pair".to_owned(), "a want has no herd to receive it")
            })?;
            *herd = herd
                .checked_add(taken)
                .ok_or_else(|| missing_data("herds".to_owned(), "herd count overflow"))?;
            wants[pop][species] -= taken;
            left -= taken;
        }
        if left != 0 {
            return Err(missing_data(
                "swap.pair".to_owned(),
                "an animal arrived that no job or herd of the pop wants",
            ));
        }
        Ok(())
    }
}

/// The prices and volumes of the two goods of a pair.
#[derive(Clone, Copy)]
struct Prices {
    give_price: u128,
    get_price: u128,
    give_volume: u128,
    get_volume: u128,
}

impl Prices {
    /// The fewest units given that are worth `get_units` received.
    fn least_payment(&self, get_units: u128) -> Option<u128> {
        Some(
            get_units
                .checked_mul(self.get_price)?
                .div_ceil(self.give_price)
                .max(1),
        )
    }

    /// Whether the starter's rounding loss on `give_units` given for `get_units` received is within `limit_pm` of what it gives
    /// (rule 10), the pair being worth at least what it receives.
    fn within_loss(&self, give_units: u128, get_units: u128, limit_pm: u128) -> Option<bool> {
        let given = give_units.checked_mul(self.give_price)?;
        let got = get_units.checked_mul(self.get_price)?;
        Some(given >= got && (given - got).checked_mul(1000)? <= limit_pm.checked_mul(given)?)
    }
}

/// The most units the starter can receive that pass spare and want, room and capacity, each paid for with the fewest units worth
/// it, or `None`: every one of those tests only gets harder as the receipt grows, so it is found by halving.
fn largest_receipt(
    prices: &Prices,
    most_get: u128,
    most_give: u128,
    capacity_left: u128,
) -> Option<u128> {
    let fits = |get_units: u128| -> bool {
        let Some(least) = prices.least_payment(get_units) else {
            return false;
        };
        least <= most_give
            && get_units
                .checked_mul(prices.get_volume)
                .zip(least.checked_mul(prices.give_volume))
                .and_then(|(get_load, give_load)| get_load.checked_add(give_load))
                .is_some_and(|load| load <= capacity_left)
    };
    if !fits(1) {
        return None;
    }
    let (mut low, mut high) = (1_u128, most_get);
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if fits(middle) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    Some(low)
}

const fn gcd(mut left: u128, mut right: u128) -> u128 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

/// The pair of rule 11 for one starter, partner and two goods, or `None`: of the allowed pairs the one that receives the most
/// units, of those the one that gives the fewest. Found in a bounded number of steps, for any amounts.
///
/// The most receipt that passes everything but the loss is found by halving. If its least payment passes the loss, that is the swap.
/// Otherwise that payment is small (a payment of at least `1,000 / limit` always passes), so the payments below it are tried, largest
/// first, each with the most it is worth, and the first that passes gives the receipt. A limit of nothing admits only pairs of equal
/// worth, the multiples of one smallest pair.
#[cfg(test)]
pub(crate) fn best_pair(
    exchange: &Exchange<'_>,
    place: &PlaceState,
    book: &Book,
    [starter, partner]: [usize; 2],
    [give_good, get_good]: [usize; 2],
    capacity_left: u128,
) -> Result<Option<(u64, u64)>, DataError> {
    best_pair_capped(
        exchange,
        place,
        book,
        [starter, partner],
        [give_good, get_good],
        capacity_left,
        [
            book.position(starter, give_good).spare,
            book.position(partner, get_good).spare,
            u128::MAX,
            u128::MAX,
        ],
    )
}

#[cfg(test)]
pub(crate) fn best_pair_capped(
    exchange: &Exchange<'_>,
    place: &PlaceState,
    book: &Book,
    pops: [usize; 2],
    goods: [usize; 2],
    capacity_left: u128,
    limits: [u128; 4],
) -> Result<Option<(u64, u64)>, DataError> {
    best_pair_rooms(
        exchange,
        place,
        book,
        pops,
        goods,
        capacity_left,
        limits,
        [None, None],
    )
}

/// [`best_pair_capped`] where a receipt that goes to a herd's building and not to a pop's store has that building's room in
/// `rooms`: the starter's for the good it gets, then the partner's for the good it is given.
#[expect(
    clippy::too_many_arguments,
    reason = "The pair is fixed by both pops, both goods, the starter's capacity, four ceilings and two receipt rooms."
)]
fn best_pair_rooms(
    exchange: &Exchange<'_>,
    place: &PlaceState,
    book: &Book,
    [starter, partner]: [usize; 2],
    [give_good, get_good]: [usize; 2],
    capacity_left: u128,
    [give_stock, get_stock, give_limit, get_limit]: [u128; 4],
    [get_building, give_building]: [Option<usize>; 2],
) -> Result<Option<(u64, u64)>, DataError> {
    let rules = exchange.rules;
    let prices = Prices {
        give_price: u128::from(place.prices_milli[give_good]),
        get_price: u128::from(place.prices_milli[get_good]),
        give_volume: u128::from(rules.goods[give_good].volume_ml),
        get_volume: u128::from(rules.goods[get_good].volume_ml),
    };
    let most_get = book
        .position(starter, get_good)
        .want
        .min(get_stock)
        .min(get_limit)
        .min(exchange.funded_room(place, book, starter, get_good, get_building)?);
    let most_give = give_stock
        .min(give_limit)
        .min(book.position(partner, give_good).want)
        .min(exchange.funded_room(place, book, partner, give_good, give_building)?);
    size_pair(
        prices,
        most_get,
        most_give,
        capacity_left,
        rules.swap_loss_limit_pm,
    )
}

/// Sizes a trade after each side's own want, available stock and receipt room have been checked.
fn size_pair(
    prices: Prices,
    most_get: u128,
    most_give: u128,
    capacity_left: u128,
    loss_limit_pm: u16,
) -> Result<Option<(u64, u64)>, DataError> {
    if most_get == 0 || most_give == 0 || prices.give_price == 0 || prices.get_price == 0 {
        return Ok(None);
    }
    let overflow = || missing_data("swap.pair".to_owned(), "pair value overflow");
    let limit = u128::from(loss_limit_pm);
    let narrow = |units: u128| u64::try_from(units).map_err(|_| overflow());
    if limit == 0 {
        // Pairs of equal worth are the whole multiples of the two prices, each divided by their greatest common divisor.
        let divisor = gcd(prices.give_price, prices.get_price);
        let (give_unit, get_unit) = (prices.get_price / divisor, prices.give_price / divisor);
        let load = give_unit
            .checked_mul(prices.give_volume)
            .zip(get_unit.checked_mul(prices.get_volume))
            .and_then(|(give_load, get_load)| give_load.checked_add(get_load))
            .ok_or_else(overflow)?;
        let mut multiples = (most_get / get_unit).min(most_give / give_unit);
        if let Some(by_capacity) = capacity_left.checked_div(load) {
            multiples = multiples.min(by_capacity);
        }
        if multiples == 0 {
            return Ok(None);
        }
        return Ok(Some((
            narrow(multiples * give_unit)?,
            narrow(multiples * get_unit)?,
        )));
    }
    let Some(get_units) = largest_receipt(&prices, most_get, most_give, capacity_left) else {
        return Ok(None);
    };
    let give_units = prices.least_payment(get_units).ok_or_else(overflow)?;
    if prices
        .within_loss(give_units, get_units, limit)
        .ok_or_else(overflow)?
    {
        return Ok(Some((narrow(give_units)?, narrow(get_units)?)));
    }
    // A payment under `1,000 / limit` units: at most 999 are tried, largest first.
    for payment in (1..give_units).rev() {
        let receipt = payment
            .checked_mul(prices.give_price)
            .ok_or_else(overflow)?
            / prices.get_price;
        if receipt == 0 {
            break;
        }
        if prices
            .within_loss(payment, receipt, limit)
            .ok_or_else(overflow)?
        {
            let fewest = prices.least_payment(receipt).ok_or_else(overflow)?;
            return Ok(Some((narrow(fewest)?, narrow(receipt)?)));
        }
    }
    Ok(None)
}

/// An expedition waiting at its destination for the noon's exchange, with what it has traded so far (CK-432, r5 3.6 and 3.7).
#[derive(Clone)]
pub(crate) struct Visit {
    /// The home place's index, which the expedition returns to.
    pub(crate) home: usize,
    /// The expedition itself.
    pub(crate) expedition: Expedition,
    /// What it still has to trade, in rules order: its cargo less what it has given.
    cargo: Vec<u64>,
    /// What it has traded for so far, in rules order, which is what it will bring home.
    received: Vec<u64>,
    /// What its pop holds at home and it holds aboard, by good, for its wealth at this place's prices.
    holds: Vec<u128>,
    /// The people of its pop at home, for its wealth for each of them.
    people: u64,
}

/// The most units, up to `most`, for which `fits` holds, where `fits` is not known to hold below the largest amount that does: a
/// first-fit assignment can take three units of a good and refuse two, and the amounts that fit can be runs with gaps between them
/// (11c.2 rule 5 makes the placement depend on the amounts). So the amounts are tried from `most` down, one by one, until one fits:
/// the first to do so is the largest, exactly. No plan holds more than [`crate::expedition::MOST_UNITS`] of a good, which bounds the
/// amounts tried; `most` above it is brought down to it.
pub(crate) fn max_fitting(most: u128, fits: &dyn Fn(u128) -> bool) -> u128 {
    let most = most.min(u128::from(crate::expedition::MOST_UNITS));
    let mut units = most;
    loop {
        if fits(units) {
            return units;
        }
        match units.checked_sub(1) {
            Some(less) => units = less,
            None => return 0,
        }
    }
}

/// The most units of a good any assignment of the expedition could carry, found from what an assignment must give each unit: a unit of
/// a good takes its weight and its space in some carrier, so the units cannot outweigh or outspace all the carriers together (each
/// limit counted only where the good has that size); a live animal driven on land takes none of that but needs its share of a drover.
/// This is a true upper bound for the search, never a guess: it keeps [`max_fitting`] to the amounts that could fit.
fn capacity_bound(rules: &Rules, expedition: &Expedition, good: usize) -> u128 {
    let (weight, space) = (
        u128::from(rules.goods[good].weight_g),
        u128::from(rules.goods[good].volume_ml),
    );
    let people = u128::from(expedition.people());
    let (mut weight_room, mut space_room) = (
        people * u128::from(rules.expedition.pack_g),
        people * u128::from(rules.expedition.pack_ml),
    );
    for vehicle in &expedition.vehicles {
        if let Some(kind) = rules.vehicles.get(vehicle.kind as usize) {
            weight_room += u128::from(kind.carries_g);
            space_room += u128::from(kind.carries_ml);
        }
    }
    let by = |room: u128, each: u128| room.checked_div(each);
    if crate::expedition::species_of(rules, good).is_some() {
        // On land it walks, one drover for each `animals_per_drover` of the people; on water it is carried by its weight.
        let driven = people * u128::from(rules.expedition.animals_per_drover);
        return driven.max(by(weight_room, weight).unwrap_or(u128::MAX));
    }
    match (by(weight_room, weight), by(space_room, space)) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => u128::MAX,
    }
}

impl Visit {
    /// A visitor made of an expedition of the pop `expedition.trader` of `home_place` (the place at index `home`): what it has to trade is
    /// its cargo, what it holds for its wealth is its pop's holding and what it carries (T45).
    pub(crate) fn new(
        rules: &Rules,
        home_place: &PlaceState,
        home: usize,
        expedition: Expedition,
    ) -> Result<Self, DataError> {
        let trader = expedition.trader as usize;
        let people = {
            let pop = &home_place.pops[trader];
            pop.workers.checked_add(pop.dependents).ok_or_else(|| {
                missing_data("swap.pop.size".to_owned(), "population size overflow")
            })?
        };
        let mut holds = Vec::with_capacity(rules.goods.len());
        for good in 0..rules.goods.len() {
            holds.push(
                pop_market_holding(home_place, trader, good)?
                    .saturating_add(u128::from(expedition.cargo[good]))
                    .saturating_add(u128::from(expedition.supplies[good])),
            );
        }
        let cargo = expedition.cargo.clone();
        Ok(Self {
            home,
            expedition,
            cargo,
            received: vec![0; rules.goods.len()],
            holds,
            people,
        })
    }

    /// What it traded for, which with any unsold cargo that fits is what it brings home.
    pub(crate) fn received(&self) -> &[u64] {
        &self.received
    }

    /// Whether it can bring home `units` more of a good beside what it has traded for: the units and what it holds from the trade, with
    /// the supplies, must still assign on every leg home (11b.3 rule 9). Unsold cargo gives way and is not counted (r5 3.6).
    fn fits(&self, rules: &Rules, good: usize, units: u128) -> bool {
        let Ok(units) = u64::try_from(units) else {
            return false;
        };
        let mut load = self.received.clone();
        let Some(total) = load[good].checked_add(units) else {
            return false;
        };
        load[good] = total;
        self.expedition.assigns_home(rules, &load)
    }

    /// The most units of a good it can still take in, up to `most`.
    fn room(&self, rules: &Rules, good: usize, most: u128) -> u128 {
        let bound = capacity_bound(rules, &self.expedition, good);
        max_fitting(most.min(bound), &|units| self.fits(rules, good, units))
    }
}

/// The two sides of a swap between a waiting expedition and a pop: what the visitor receives from the pop and what it gives it.
struct VisitPair {
    visitor: usize,
    resident: usize,
    /// The source of the good the pop gives: its own store (0) or one of the buildings' spare stock (the building's number plus one).
    resident_source: usize,
    visitor_gets: usize,
    visitor_gives: usize,
}

/// The exchange's opening positions and remaining capacity persist through all of its phases.
struct PlaceDay {
    order: Vec<usize>,
    book: Book,
    parts: BuildingParts,
    shares: Shares,
    herds: HerdWants,
    capacity: Vec<u128>,
    /// Whether each pop owns a building that gives a day's capacity: a pop that owns none has no turn (r5 3.7), though one whose
    /// capacity is used up still tries the waiting expeditions, which spend none (T30).
    owns: Vec<bool>,
    /// The expeditions waiting at the place for this noon, in the order they arrived (CK-432).
    visitors: Vec<Visit>,
}

impl PlaceDay {
    fn new_with(
        rules: &Rules,
        place: &PlaceState,
        opening: &[u64],
        visitors: Vec<Visit>,
    ) -> Result<Self, DataError> {
        if place.prices_milli.len() != rules.goods.len() || opening.len() != rules.goods.len() {
            return Err(missing_data(
                format!("places.{}.prices", place.id),
                "saved price dimensions do not match goods",
            ));
        }
        let capacity = market_capacities(rules, place)?;
        let book = Book::new_with(rules, place, opening, &visitors)?;
        let parts = BuildingParts::new_with(rules, place, opening, visitors.len())?;
        let shares = Shares::new_with(&book, &parts, place, &visitors)?;
        Ok(Self {
            order: want_order_at_prices(rules, &place.prices_milli, opening),
            book,
            parts,
            shares,
            herds: herd_wants(rules, place)?,
            owns: capacity.iter().map(|ml| *ml > 0).collect(),
            capacity,
            visitors,
        })
    }

    /// The waiting expeditions' swaps as starters, each working down its wants against the place's pops in their order, at the place's
    /// prices and spending no capacity (T30). What it gives leaves its cargo and what it receives is what it brings home.
    fn visitor_pass(
        &mut self,
        rules: &Rules,
        place: &mut PlaceState,
        exchange: &Exchange<'_>,
        second: bool,
    ) -> Result<(), DataError> {
        let pops = self.book.pops;
        for visitor in 0..self.visitors.len() {
            let row = pops + visitor;
            for &get_good in &self.order.clone() {
                if self.book.position(row, get_good).want == 0 {
                    continue;
                }
                for partner in 0..pops {
                    if self.parts.total(&self.book, partner, get_good) == 0 {
                        continue;
                    }
                    for &give_good in &self.order.clone() {
                        if self.book.position(row, get_good).want == 0 {
                            break;
                        }
                        if give_good == get_good
                            || self.book.position(partner, give_good).want == 0
                            || self.book.position(row, give_good).spare == 0
                            || (second
                                && !self.shares.scarce[give_good]
                                && !self.shares.scarce[get_good])
                        {
                            continue;
                        }
                        for source in 0..=place.buildings.len() {
                            self.visit_pair(
                                rules,
                                place,
                                exchange,
                                &VisitPair {
                                    visitor,
                                    resident: partner,
                                    resident_source: source,
                                    visitor_gets: get_good,
                                    visitor_gives: give_good,
                                },
                                true,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// A pop's turn with the waiting expeditions as its partners, after its turn with the pops: the starter works down its want
    /// `get_good` against each visitor's spare cargo (r5 3.7 lets any pop start a swap), spending no capacity on either side (T30).
    fn resident_turn_with_visitors(
        &mut self,
        rules: &Rules,
        place: &mut PlaceState,
        exchange: &Exchange<'_>,
        starter: usize,
        get_good: usize,
        second: bool,
    ) -> Result<(), DataError> {
        let pops = self.book.pops;
        for visitor in 0..self.visitors.len() {
            let row = pops + visitor;
            if self.book.position(row, get_good).spare == 0 {
                continue;
            }
            for &give_good in &self.order.clone() {
                if self.book.position(starter, get_good).want == 0 {
                    return Ok(());
                }
                if give_good == get_good
                    || self.book.position(row, give_good).want == 0
                    || self.parts.total(&self.book, starter, give_good) == 0
                    || (second && !self.shares.scarce[give_good] && !self.shares.scarce[get_good])
                {
                    continue;
                }
                for source in 0..=place.buildings.len() {
                    self.visit_pair(
                        rules,
                        place,
                        exchange,
                        &VisitPair {
                            visitor,
                            resident: starter,
                            resident_source: source,
                            visitor_gets: give_good,
                            visitor_gives: get_good,
                        },
                        false,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// One swap, if any is allowed, between a waiting expedition and a pop: the largest receipt for the starter that passes every test
    /// at once (r5 3.7), where the visitor's receipt must also assign on every leg home for the very amount (not for a larger one).
    #[expect(
        clippy::too_many_lines,
        reason = "Sizing, the room retry, both receipts and every ledger of the day are one step and are kept together."
    )]
    fn visit_pair(
        &mut self,
        rules: &Rules,
        place: &mut PlaceState,
        exchange: &Exchange<'_>,
        pair: &VisitPair,
        visitor_starts: bool,
    ) -> Result<bool, DataError> {
        let VisitPair {
            visitor,
            resident,
            resident_source,
            visitor_gets,
            visitor_gives,
        } = *pair;
        let row = self.book.pops + visitor;
        // What the visitor still wants is read afresh at each source: an earlier source may have filled it.
        let want = self.book.position(row, visitor_gets).want;
        let get_spare = self
            .parts
            .source(&self.book, resident_source, resident, visitor_gets);
        let give_spare = self.book.position(row, visitor_gives).spare;
        // An animal sold from a herd is paid for into the herd's building, not the pop's store (CK-430).
        let (give_live, get_live) = (self.book.live[visitor_gives], self.book.live[visitor_gets]);
        let to_building =
            (get_live && !give_live && resident_source > 0).then(|| resident_source - 1);
        let resident_room =
            exchange.funded_room(place, &self.book, resident, visitor_gives, to_building)?;
        let resident_most = self
            .book
            .position(resident, visitor_gives)
            .want
            .min(give_spare)
            .min(self.shares.limit(resident, visitor_gives, true))
            .min(resident_room);
        let mut visitor_most = want
            .min(get_spare)
            .min(self.shares.limit(row, visitor_gets, false))
            .min(self.visitors[visitor].room(rules, visitor_gets, want));
        let (price_gets, price_gives) = (
            u128::from(place.prices_milli[visitor_gets]),
            u128::from(place.prices_milli[visitor_gives]),
        );
        let (volume_gets, volume_gives) = (
            u128::from(rules.goods[visitor_gets].volume_ml),
            u128::from(rules.goods[visitor_gives].volume_ml),
        );
        let (give, get) = loop {
            let sized = if visitor_starts {
                size_pair(
                    Prices {
                        give_price: price_gives,
                        get_price: price_gets,
                        give_volume: volume_gives,
                        get_volume: volume_gets,
                    },
                    visitor_most,
                    resident_most,
                    u128::MAX,
                    rules.swap_loss_limit_pm,
                )?
            } else {
                size_pair(
                    Prices {
                        give_price: price_gets,
                        get_price: price_gives,
                        give_volume: volume_gets,
                        get_volume: volume_gives,
                    },
                    resident_most,
                    visitor_most,
                    u128::MAX,
                    rules.swap_loss_limit_pm,
                )?
                .map(|(resident_gives, resident_gets)| (resident_gets, resident_gives))
            };
            let Some((give, get)) = sized else {
                return Ok(false);
            };
            // A smaller pair that barter pricing chose may not assign where a larger one does: the amount itself is tested.
            if self.visitors[visitor].fits(rules, visitor_gets, u128::from(get)) {
                break (give, get);
            }
            if get <= 1 {
                return Ok(false);
            }
            visitor_most = self.visitors[visitor].room(rules, visitor_gets, u128::from(get) - 1);
            if visitor_most == 0 {
                return Ok(false);
            }
        };
        let route = exchange.funded_route(
            place,
            &self.book,
            resident,
            visitor_gives,
            u128::from(give),
            to_building,
        )?;
        Exchange::ensure_route(route.as_ref(), give)?;
        exchange.pay_source(place, resident_source, resident, visitor_gets, get)?;
        let delivered = exchange.receive_funded(
            place,
            &mut self.book,
            &mut self.herds,
            resident,
            visitor_gives,
            give,
            to_building,
            route,
        )?;
        let visit = &mut self.visitors[visitor];
        visit.cargo[visitor_gives] -= give;
        visit.received[visitor_gets] += get;
        self.book
            .settle_side(row, visitor_gives, give, true, visitor_gets, get, false);
        self.book.settle_side(
            resident,
            visitor_gets,
            get,
            resident_source == 0,
            visitor_gives,
            give,
            to_building.is_some(),
        );
        self.book.deliver(resident, visitor_gives, delivered);
        if rules.goods[visitor_gives].category_supplied("fuel", 1) > 0
            || rules.goods[visitor_gets].category_supplied("fuel", 1) > 0
        {
            self.book.refresh_fuel(rules, place, resident)?;
        }
        if resident_source > 0 {
            self.parts.spare[resident_source - 1][resident][visitor_gets] -= u128::from(get);
        }
        self.shares.receive(resident, visitor_gives, give, true);
        self.shares.receive(row, visitor_gets, get, false);
        Ok(true)
    }

    /// Free cargo follows unbudgeted need and does not spend purchase orders.
    fn give_unsold(
        &mut self,
        exchange: &Exchange<'_>,
        place: &mut PlaceState,
        good: usize,
        mut left: u64,
    ) -> Result<u64, DataError> {
        let mut takers: Vec<(u128, usize)> = (0..self.book.pops)
            .filter_map(|pop| {
                let want = self.book.raw_position(pop, good).want;
                (want > 0).then_some((want, pop))
            })
            .collect();
        takers.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (want, pop) in takers {
            let room = exchange.room(place, pop, good)?;
            let give = u128::from(left).min(want).min(room);
            if give == 0 {
                continue;
            }
            let give = u64::try_from(give).unwrap_or(0);
            exchange.receive(place, &mut self.herds, pop, good, give, None)?;
            left -= give;
        }
        Ok(left)
    }

    /// After the exchange each visitor loads for home and turns back, in the same noon step (r5 3.6): what it traded for is already
    /// aboard; then its unsold cargo as far as it fits, the goods worth most for their bulk first; what does not fit is given for
    /// nothing to the pops there that want it and have room, the neediest first, and thrown away only when nobody does (T38).
    fn finish(mut self, rules: &Rules, place: &mut PlaceState) -> Result<Vec<Visit>, DataError> {
        let exchange = Exchange {
            rules,
            house: place
                .buildings
                .iter()
                .position(|b| b.storehouse_accounts.is_some()),
        };
        let goods = rules.goods.len();
        for visitor in 0..self.visitors.len() {
            let mut loaded = self.visitors[visitor].received.clone();
            let mut worth: Vec<usize> = (0..goods)
                .filter(|good| self.visitors[visitor].cargo[*good] > 0)
                .collect();
            worth.sort_by(|a, b| {
                let bulk = |good: usize| u128::from(rules.goods[good].volume_ml.max(1));
                (u128::from(place.prices_milli[*b]) * bulk(*a))
                    .cmp(&(u128::from(place.prices_milli[*a]) * bulk(*b)))
                    .then(a.cmp(b))
            });
            for good in worth {
                let have = u128::from(self.visitors[visitor].cargo[good]);
                let expedition = &self.visitors[visitor].expedition;
                let bound = capacity_bound(rules, &self.visitors[visitor].expedition, good);
                let low = max_fitting(have.min(bound), &|units| {
                    let mut trial = loaded.clone();
                    let Some(total) = u64::try_from(units)
                        .ok()
                        .and_then(|units| trial[good].checked_add(units))
                    else {
                        return false;
                    };
                    trial[good] = total;
                    expedition.assigns_home(rules, &trial)
                });
                let take = u64::try_from(low).unwrap_or(0);
                loaded[good] += take;
                self.visitors[visitor].cargo[good] -= take;
            }
            for good in 0..goods {
                let mut left = self.visitors[visitor].cargo[good];
                if left == 0 {
                    continue;
                }
                left = self.give_unsold(&exchange, place, good, left)?;
                if left > 0 {
                    place.counters.current.losses = place
                        .counters
                        .current
                        .losses
                        .checked_add(left)
                        .ok_or_else(|| {
                            missing_data(
                                "counters.losses".to_owned(),
                                "place loss counter overflow",
                            )
                        })?;
                    crate::add_counter_units(
                        &mut place.counters.current.lost,
                        goods,
                        good,
                        left,
                        "counters.lost",
                    )?;
                }
                self.visitors[visitor].cargo[good] = 0;
            }
            let visit = &mut self.visitors[visitor];
            visit.expedition.cargo = loaded;
            visit.expedition.traded = true;
            visit.expedition.start_back(rules)?;
        }
        Ok(self.visitors)
    }

    fn local_pass(
        &mut self,
        rules: &Rules,
        place: &mut PlaceState,
        second: bool,
    ) -> Result<(), DataError> {
        let exchange = Exchange {
            rules,
            house: place
                .buildings
                .iter()
                .position(|b| b.storehouse_accounts.is_some()),
        };
        // The expeditions waiting here start their swaps first, in the order they arrived (r5 3.7).
        self.visitor_pass(rules, place, &exchange, second)?;
        for starter in 0..self.capacity.len() {
            if !self.owns[starter] {
                continue;
            }
            let mut left = self.capacity[starter];
            for get_good in self.order.clone() {
                if self.book.hungry[starter] && rules.goods[get_good].hunger_centi == 0 {
                    continue;
                }
                // For each want the starter tries the other pops in the place's order while it has capacity, then the waiting
                // expeditions in the order they arrived, which spend none on either side (T30).
                if left > 0 {
                    let mut turn = Turn {
                        exchange: &exchange,
                        order: &self.order,
                        starter,
                        get_good,
                    };
                    turn.work(
                        place,
                        &mut self.book,
                        &mut self.parts,
                        &mut self.shares,
                        &mut self.herds,
                        &mut left,
                        second,
                    )?;
                }
                if !self.visitors.is_empty() {
                    self.resident_turn_with_visitors(
                        rules, place, &exchange, starter, get_good, second,
                    )?;
                }
            }
            self.capacity[starter] = left;
        }
        Ok(())
    }
}

/// The exchange a visitor would find at `place` as it stands, worked on a copy of the place: nothing in the real state moves (r5 3.5 item
/// 2: the swap of 3.7 worked forward against the destination's pops as they stand today, at its prices). The visitor comes back as the
/// exchange leaves it: what it received, and what it loads for home.
pub(crate) fn forecast_exchange(
    rules: &Rules,
    place: &PlaceState,
    visit: Visit,
) -> Result<Visit, DataError> {
    let mut copy = place.clone();
    let opening = place.prices_milli.clone();
    let mut done = run_place_exchange(rules, &mut copy, &opening, vec![visit])?;
    done.pop()
        .ok_or_else(|| missing_data("swap.forecast".to_owned(), "the visitor was lost"))
}

/// Takes the goods that trips already under way are to take from a place out of what it could offer, on a copy (r5 3.5 item 9: a
/// forecast moves no goods and reserves none in the game, so this is only for the copy a look works on). Only the goods the place's
/// owners and pops would offer today go: a building's share of its stock that it sells, then each pop's spare above its own target,
/// never what they keep. A storehouse's goods leave its accounts as well as its physical stock, so the copy still passes the market's
/// accounting; the pop's own store goes before its account.
pub(crate) fn take_spoken(
    rules: &Rules,
    place: &mut PlaceState,
    spoken: &[u64],
) -> Result<(), DataError> {
    let opening = place.prices_milli.clone();
    let sells = building_market_sells(rules, place, &opening)?;
    let book = Book::new_with(rules, place, &opening, &[])?;
    let live = live_species(rules);
    for (good, units) in spoken.iter().enumerate().take(rules.goods.len()) {
        let mut left = *units;
        if let Some(species) = live[good] {
            // A live animal is offered from a herd above its standard level and from the animals given-up jobs hold (CK-430, 11b.1), never
            // from a stock: the herd and the jobs' animals fall by what is spoken for, within what is on offer.
            for (index, building) in place.buildings.iter_mut().enumerate() {
                let offer = u64::try_from(sells[index][good]).unwrap_or(u64::MAX);
                let Some(herd) = building.herds.get_mut(species) else {
                    continue;
                };
                let taken = left.min(offer).min(*herd);
                *herd -= taken;
                left -= taken;
            }
            for job in place
                .trade
                .jobs
                .iter_mut()
                .filter(|job| job.given_up.is_some())
            {
                let taken = left.min(job.animals[species]);
                job.animals[species] -= taken;
                left -= taken;
            }
            continue;
        }
        for (index, building) in place.buildings.iter_mut().enumerate() {
            if left == 0 {
                break;
            }
            let offer = u64::try_from(sells[index][good]).unwrap_or(u64::MAX);
            let held = building
                .storehouse_accounts
                .as_ref()
                .map_or(building.stock[good], |accounts| accounts.communal[good]);
            let taken = left.min(offer).min(held).min(building.stock[good]);
            building.stock[good] -= taken;
            if let Some(accounts) = &mut building.storehouse_accounts {
                accounts.communal[good] -= taken;
            }
            left -= taken;
        }
        for pop in 0..place.pops.len() {
            if left == 0 {
                break;
            }
            let spare = u64::try_from(book.position(pop, good).spare).unwrap_or(u64::MAX);
            let owed = left.min(spare);
            left -= owed - take_from_pop(place, pop, good, owed);
        }
    }
    Ok(())
}

/// Takes up to `units` of a good out of a pop's holding on a copy: its own store first, then its account in a storehouse (whose physical
/// stock falls with it, so the accounts still balance). Returns how many it could not take.
fn take_from_pop(place: &mut PlaceState, pop: usize, good: usize, units: u64) -> u64 {
    let own = units.min(place.pops[pop].stock[good]);
    place.pops[pop].stock[good] -= own;
    let mut owed = units - own;
    for building in &mut place.buildings {
        if owed == 0 {
            break;
        }
        if let Some(accounts) = &mut building.storehouse_accounts {
            let taken = owed.min(accounts.pops[pop][good]).min(building.stock[good]);
            accounts.pops[pop][good] -= taken;
            building.stock[good] -= taken;
            owed -= taken;
        }
    }
    owed
}

/// Stages the departure of an expedition of the pop `pop` on a copy of its home: what the job gathers from the pop (its cargo and its
/// supplies, as far as it holds them) has left its holding, as it has by the time the expedition stands at the destination, so the visit
/// counts it once, aboard, and not also at home (T45).
///
/// # Errors
/// A holding that cannot be read.
pub(crate) fn stage_departure(
    rules: &Rules,
    place: &mut PlaceState,
    pop: usize,
    cargo: &[u64],
    supplies: &[u64],
) -> Result<(), DataError> {
    for good in 0..rules.goods.len() {
        let wanted = cargo[good].saturating_add(supplies[good]);
        let held = u64::try_from(pop_market_holding(place, pop, good)?).unwrap_or(u64::MAX);
        take_from_pop(place, pop, good, wanted.min(held));
    }
    Ok(())
}

/// What a place's pops want of each good and what they can offer of it today, in rules order, as the exchange counts them (positions
/// against targets, the buildings' shares of spare stock included in what can be offered): where a trader may sell and where it may buy.
pub(crate) fn place_totals(
    rules: &Rules,
    place: &PlaceState,
) -> Result<(Vec<u128>, Vec<u128>), DataError> {
    let book = Book::new_with(rules, place, &place.prices_milli, &[])?;
    let parts = BuildingParts::new_with(rules, place, &place.prices_milli, 0)?;
    let goods = rules.goods.len();
    let (mut want, mut spare) = (vec![0_u128; goods], vec![0_u128; goods]);
    for pop in 0..place.pops.len() {
        for good in 0..goods {
            want[good] = want[good].saturating_add(book.position(pop, good).want);
            spare[good] = spare[good].saturating_add(parts.total(&book, pop, good));
        }
    }
    Ok((want, spare))
}

/// One pop's position for every good as the exchange counts it, without what its jobs lack (which is not a want of its own): what it
/// wants (a deficit against its target) and what it can offer (its spare and its shares of the buildings'), and whether it is hungry
/// (under 60 days of food, rule 18).
pub(crate) struct TraderView {
    /// The deficit against its target, by good.
    pub(crate) deficit: Vec<u128>,
    /// The target, by good.
    pub(crate) target: Vec<u128>,
    /// What it could offer, by good.
    pub(crate) spare: Vec<u128>,
    /// Whether it holds less than 60 days of food.
    pub(crate) hungry: bool,
}

pub(crate) fn trader_view(
    rules: &Rules,
    place: &PlaceState,
    pop: usize,
) -> Result<TraderView, DataError> {
    let book = Book::new_with(rules, place, &place.prices_milli, &[])?;
    let parts = BuildingParts::new_with(rules, place, &place.prices_milli, 0)?;
    let goods = rules.goods.len();
    let (mut deficit, mut target, mut spare) = (
        vec![0_u128; goods],
        vec![0_u128; goods],
        vec![0_u128; goods],
    );
    let live = live_species(rules);
    let (_, job_animals) = trade::lacks(rules, place);
    for good in 0..goods {
        // The trader's own shortfall against its own target (r5 3.5 item 1), not what its market budget funds: the look plans from the raw
        // want, less the waiting jobs' lack (which stays the jobs' own want and never cancels the household's of the same good). A live
        // good's raw target also holds the jobs' lack of the animal (a waiting ox-cart's draught cattle: `Book::new_with` adds it so the
        // exchange can fund it), which is no deficit against the trader's own herd and no share of its target: both come from its own herd
        // position, and the exchange keeps the funded claim of the build.
        let waiting_draught = live[good].map_or(0, |species| u128::from(job_animals[pop][species]));
        let position = book.raw_position(pop, good);
        deficit[good] = position
            .want
            .saturating_sub(book.lack[pop][good])
            .saturating_sub(waiting_draught);
        target[good] = book.target[pop][good].saturating_sub(waiting_draught);
        spare[good] = parts.total(&book, pop, good);
    }
    Ok(TraderView {
        deficit,
        target,
        spare,
        hungry: book.hungry[pop],
    })
}

/// What the other pops of a place want of each good in its market, on the funded basis the market's own totals use ([`place_totals`]): what
/// a trader's home market has to give it is what is offered less what these pops claim.
pub(crate) fn market_claims_of_others(
    rules: &Rules,
    place: &PlaceState,
    pop: usize,
) -> Result<Vec<u128>, DataError> {
    let book = Book::new_with(rules, place, &place.prices_milli, &[])?;
    let goods = rules.goods.len();
    let mut claims = vec![0_u128; goods];
    for other in (0..place.pops.len()).filter(|other| *other != pop) {
        for (good, claim) in claims.iter_mut().enumerate() {
            *claim = claim.saturating_add(book.position(other, good).want);
        }
    }
    Ok(claims)
}

fn run_place_exchange(
    rules: &Rules,
    place: &mut PlaceState,
    opening: &[u64],
    visitors: Vec<Visit>,
) -> Result<Vec<Visit>, DataError> {
    if place.pops.len() < 2 && visitors.is_empty() {
        return Ok(visitors);
    }
    let mut day = PlaceDay::new_with(rules, place, opening, visitors)?;
    day.local_pass(rules, place, false)?;
    day.shares.second_pass(&day.book, &day.parts)?;
    day.local_pass(rules, place, true)?;
    day.finish(rules, place)
}

/// A turn between two places uses the seller's price for each good and the starter's capacity.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "A turn settles both places against their opening ledgers and shared allocations."
)]
fn neighbour_turn(
    rules: &Rules,
    starter_place: &mut PlaceState,
    partner_place: &mut PlaceState,
    starter_day: &mut PlaceDay,
    partner_day: &mut PlaceDay,
    shares: &mut NeighbourShares,
    starter_index: usize,
    partner_index: usize,
    second: bool,
) -> Result<(), DataError> {
    let se = Exchange {
        rules,
        house: starter_place
            .buildings
            .iter()
            .position(|b| b.storehouse_accounts.is_some()),
    };
    let pe = Exchange {
        rules,
        house: partner_place
            .buildings
            .iter()
            .position(|b| b.storehouse_accounts.is_some()),
    };
    for starter in 0..starter_place.pops.len() {
        for &get_good in &starter_day.order {
            if starter_day.book.hungry[starter] && rules.goods[get_good].hunger_centi == 0 {
                continue;
            }
            for partner in 0..partner_place.pops.len() {
                for &give_good in &starter_day.order {
                    if starter_day.capacity[starter] == 0
                        || starter_day.book.position(starter, get_good).want == 0
                    {
                        break;
                    }
                    if give_good == get_good
                        || partner_day.book.position(partner, give_good).want == 0
                        || (second
                            && !shares.scarce[starter_index][give_good]
                            && !shares.scarce[partner_index][get_good])
                    {
                        continue;
                    }
                    for ss in 0..=starter_place.buildings.len() {
                        for ps in 0..=partner_place.buildings.len() {
                            let give_stock =
                                starter_day
                                    .parts
                                    .source(&starter_day.book, ss, starter, give_good);
                            let get_stock =
                                partner_day
                                    .parts
                                    .source(&partner_day.book, ps, partner, get_good);
                            let gb = (starter_day.book.live[give_good]
                                && !starter_day.book.live[get_good]
                                && ss > 0)
                                .then(|| ss - 1);
                            let pb = (partner_day.book.live[get_good]
                                && !partner_day.book.live[give_good]
                                && ps > 0)
                                .then(|| ps - 1);
                            let get_room = se.funded_room(
                                starter_place,
                                &starter_day.book,
                                starter,
                                get_good,
                                gb,
                            )?;
                            let give_room = pe.funded_room(
                                partner_place,
                                &partner_day.book,
                                partner,
                                give_good,
                                pb,
                            )?;
                            let get_limit = if shares.scarce[partner_index][get_good] {
                                shares.left[partner_index][starter_index][starter][get_good]
                            } else {
                                u128::MAX
                            };
                            let give_limit = if shares.scarce[starter_index][give_good] {
                                shares.left[starter_index][partner_index][partner][give_good]
                            } else {
                                u128::MAX
                            };
                            let prices = Prices {
                                give_price: u128::from(starter_place.prices_milli[give_good]),
                                get_price: u128::from(partner_place.prices_milli[get_good]),
                                give_volume: u128::from(rules.goods[give_good].volume_ml),
                                get_volume: u128::from(rules.goods[get_good].volume_ml),
                            };
                            let Some((give, get)) = size_pair(
                                prices,
                                starter_day
                                    .book
                                    .position(starter, get_good)
                                    .want
                                    .min(get_stock)
                                    .min(get_limit)
                                    .min(get_room),
                                partner_day
                                    .book
                                    .position(partner, give_good)
                                    .want
                                    .min(give_stock)
                                    .min(give_limit)
                                    .min(give_room),
                                starter_day.capacity[starter],
                                rules.swap_loss_limit_pm,
                            )?
                            else {
                                continue;
                            };
                            let starter_route = se.funded_route(
                                starter_place,
                                &starter_day.book,
                                starter,
                                get_good,
                                u128::from(get),
                                gb,
                            )?;
                            let partner_route = pe.funded_route(
                                partner_place,
                                &partner_day.book,
                                partner,
                                give_good,
                                u128::from(give),
                                pb,
                            )?;
                            Exchange::ensure_route(starter_route.as_ref(), get)?;
                            Exchange::ensure_route(partner_route.as_ref(), give)?;
                            se.pay_source(starter_place, ss, starter, give_good, give)?;
                            pe.pay_source(partner_place, ps, partner, get_good, get)?;
                            let starter_delivered = se.receive_funded(
                                starter_place,
                                &mut starter_day.book,
                                &mut starter_day.herds,
                                starter,
                                get_good,
                                get,
                                gb,
                                starter_route,
                            )?;
                            let partner_delivered = pe.receive_funded(
                                partner_place,
                                &mut partner_day.book,
                                &mut partner_day.herds,
                                partner,
                                give_good,
                                give,
                                pb,
                                partner_route,
                            )?;
                            starter_day.book.settle_side(
                                starter,
                                give_good,
                                give,
                                ss == 0,
                                get_good,
                                get,
                                gb.is_some(),
                            );
                            partner_day.book.settle_side(
                                partner,
                                get_good,
                                get,
                                ps == 0,
                                give_good,
                                give,
                                pb.is_some(),
                            );
                            starter_day
                                .book
                                .deliver(starter, get_good, starter_delivered);
                            partner_day
                                .book
                                .deliver(partner, give_good, partner_delivered);
                            if ss > 0 {
                                starter_day.parts.spare[ss - 1][starter][give_good] -=
                                    u128::from(give);
                            }
                            if ps > 0 {
                                partner_day.parts.spare[ps - 1][partner][get_good] -=
                                    u128::from(get);
                            }
                            if shares.scarce[starter_index][give_good] {
                                shares.left[starter_index][partner_index][partner][give_good] -=
                                    u128::from(give);
                            }
                            if shares.scarce[partner_index][get_good] {
                                shares.left[partner_index][starter_index][starter][get_good] -=
                                    u128::from(get);
                            }
                            starter_day.capacity[starter] -= u128::from(give) * prices.give_volume
                                + u128::from(get) * prices.get_volume;
                            if rules.goods[give_good].category_supplied("fuel", 1) > 0
                                || rules.goods[get_good].category_supplied("fuel", 1) > 0
                            {
                                starter_day
                                    .book
                                    .refresh_fuel(rules, starter_place, starter)?;
                                partner_day
                                    .book
                                    .refresh_fuel(rules, partner_place, partner)?;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// One starter's work on one want (rule 17): the other pops in the place's order, and with each the goods it wants that the starter
/// has to spare.
struct Turn<'a> {
    exchange: &'a Exchange<'a>,
    order: &'a [usize],
    starter: usize,
    get_good: usize,
}

impl Turn<'_> {
    #[expect(
        clippy::too_many_arguments,
        reason = "One turn works the place, the day's book, parts, shares and herd wants with the starter's capacity."
    )]
    fn work(
        &mut self,
        place: &mut PlaceState,
        book: &mut Book,
        parts: &mut BuildingParts,
        shares: &mut Shares,
        herds: &mut HerdWants,
        capacity: &mut u128,
        second: bool,
    ) -> Result<(), DataError> {
        let rules = self.exchange.rules;
        for partner in 0..place.pops.len() {
            if partner == self.starter || parts.total(book, partner, self.get_good) == 0 {
                continue;
            }
            for &give_good in self.order {
                if *capacity == 0 || book.position(self.starter, self.get_good).want == 0 {
                    return Ok(());
                }
                if give_good == self.get_good
                    || book.position(partner, give_good).want == 0
                    || parts.total(book, self.starter, give_good) == 0
                    || (second && !shares.scarce[give_good] && !shares.scarce[self.get_good])
                {
                    continue;
                }
                for starter_source in 0..=place.buildings.len() {
                    for partner_source in 0..=place.buildings.len() {
                        let give_spare =
                            parts.source(book, starter_source, self.starter, give_good);
                        let get_spare = parts.source(book, partner_source, partner, self.get_good);
                        // An animal sold from a herd is paid for into the herd's building, not the pop's store (CK-430).
                        let (give_live, get_live) =
                            (book.live[give_good], book.live[self.get_good]);
                        let get_to_building = (give_live && !get_live && starter_source > 0)
                            .then(|| starter_source - 1);
                        let give_to_building = (get_live && !give_live && partner_source > 0)
                            .then(|| partner_source - 1);
                        let Some((give, get)) = best_pair_rooms(
                            self.exchange,
                            place,
                            book,
                            [self.starter, partner],
                            [give_good, self.get_good],
                            *capacity,
                            [
                                give_spare,
                                get_spare,
                                shares.limit(partner, give_good, false),
                                shares.limit(self.starter, self.get_good, false),
                            ],
                            [get_to_building, give_to_building],
                        )?
                        else {
                            continue;
                        };
                        let trade = Trade {
                            starter: self.starter,
                            partner,
                            give_good,
                            give_units: give,
                            get_good: self.get_good,
                            get_units: get,
                            starter_source,
                            partner_source,
                            get_to_building,
                            give_to_building,
                        };
                        let delivered = self.exchange.apply(place, book, herds, trade)?;
                        book.settle(trade);
                        book.deliver(self.starter, self.get_good, delivered[0]);
                        book.deliver(partner, give_good, delivered[1]);
                        if rules.goods[give_good].category_supplied("fuel", 1) > 0
                            || rules.goods[self.get_good].category_supplied("fuel", 1) > 0
                        {
                            book.refresh_fuel(rules, place, self.starter)?;
                            book.refresh_fuel(rules, place, partner)?;
                        }
                        if starter_source > 0 {
                            parts.spare[starter_source - 1][self.starter][give_good] -=
                                u128::from(give);
                        }
                        if partner_source > 0 {
                            parts.spare[partner_source - 1][partner][self.get_good] -=
                                u128::from(get);
                        }
                        shares.receive(partner, give_good, give, false);
                        shares.receive(self.starter, self.get_good, get, false);
                        *capacity -= u128::from(give)
                            * u128::from(rules.goods[give_good].volume_ml)
                            + u128::from(get) * u128::from(rules.goods[self.get_good].volume_ml);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod share_tests {
    use super::*;
    use crate::swap_tests;

    #[test]
    fn a_residents_received_head_does_not_become_an_offer() {
        let mut book = Book::from_parts(vec![vec![0, 0]], vec![vec![0, 1]]);
        book.live[0] = true;
        book.funded = Some(vec![vec![]]);
        book.fuel_data = vec![(0, 1), (0, 1)];
        book.settle_side(0, 1, 1, true, 0, 1, false);
        assert_eq!(book.holding[0][0], 0);
        assert_eq!(book.position(0, 0).spare, 0);
    }

    #[test]
    fn exchange_fuel_wants_use_the_same_funded_heat_and_value_as_price_orders() {
        let rules = swap_tests::rules();
        let wood = swap_tests::good(&rules, "hardwood");
        let peat = swap_tests::good(&rules, "peat");
        let width = rules.goods.len();
        let mut book = Book::from_parts(vec![vec![0; width]], vec![vec![0; width]]);
        book.fuel_data = rules
            .goods
            .iter()
            .map(|g| (g.category_supplied("fuel", 1), g.base_price))
            .collect();
        book.funded = Some(vec![vec![crate::budget::Want {
            tier: 0,
            units: 1050,
            value: 9000,
            funded: 1050,
            funded_value: 9000,
            destination: crate::budget::Destination::Fuel,
        }]]);
        assert_eq!(book.position(0, wood).want, 0);
        assert_eq!(book.position(0, peat).want, 1);
        book.funded.as_mut().unwrap()[0][0].funded_value = 10_000;
        assert_eq!(book.position(0, wood).want, 1);
    }

    #[test]
    fn food_then_opening_price_fuel_then_comfort() {
        let rules = swap_tests::rules();
        let oats = swap_tests::good(&rules, "oats");
        let wood = swap_tests::good(&rules, "hardwood");
        let peat = swap_tests::good(&rules, "peat");
        let herbs = swap_tests::good(&rules, "herbs");
        let mut opening = vec![100; rules.goods.len()];
        opening[peat] = 60;
        let mut moved = opening.clone();
        moved[peat] = 90;
        let order = want_order_at_prices(&rules, &moved, &opening);
        let rank = |good| order.iter().position(|&g| g == good).unwrap();
        assert!(rank(oats) < rank(peat));
        assert!(rank(peat) < rank(wood));
        assert!(rank(wood) < rank(herbs));
    }

    #[test]
    fn outside_room_uses_the_goods_cap_even_when_the_store_is_full() {
        let rules = swap_tests::rules();
        let mut place = swap_tests::place(&rules, 1);
        let wood = swap_tests::good(&rules, "hardwood");
        let oats = swap_tests::good(&rules, "oats");
        place.pops[0].stock[oats] = u64::MAX / 100_000;
        let exchange = Exchange {
            rules: &rules,
            house: None,
        };
        let people = Exchange::people(&place, 0).unwrap();
        let (cap, capacity) = pop_store_limits(&rules, people, wood).unwrap();
        let ordinary = room_by_cap_and_volume(
            &rules,
            &place.pops[0].stock,
            wood,
            cap,
            capacity,
            None,
            u64::MAX,
        )
        .unwrap();
        assert_eq!(
            exchange.room(&place, 0, wood).unwrap(),
            u128::from(ordinary)
        );
        assert_eq!(exchange.room(&place, 0, oats).unwrap(), 0);
    }

    #[test]
    fn room_grid_keeps_ordinary_full_store_rule_and_outside_receipt_parity() {
        let mut rules = swap_tests::rules();
        let wood = swap_tests::good(&rules, "hardwood");
        let oats = swap_tests::good(&rules, "oats");
        let fish = swap_tests::good(&rules, "fish");
        rules.goods[fish].volume_ml = 0;
        let mut place = swap_tests::place(&rules, 1);
        place.pops[0].workers = 1;
        place.pops[0].dependents = 0;
        rules.goods[wood].pop_limit_milli = 5_000;
        for (stock_oats, held_wood) in [(0, 0), (20, 2), (21, 5), (22, 6)] {
            place.pops[0].stock[oats] = stock_oats;
            place.pops[0].stock[wood] = held_wood;
            let exchange = Exchange {
                rules: &rules,
                house: None,
            };
            let (cap, capacity) = pop_store_limits(&rules, 1, wood).unwrap();
            let stock = &place.pops[0].stock;
            let ordinary =
                room_by_cap_and_volume(&rules, stock, wood, cap, capacity, None, u64::MAX).unwrap();
            assert_eq!(
                exchange.room(&place, 0, wood).unwrap(),
                u128::from(ordinary)
            );
            let free = capacity.saturating_sub(volume_used(&rules, stock, None).unwrap());
            for good in [oats, fish] {
                let (cap, _) = pop_store_limits(&rules, 1, good).unwrap();
                let by_cap = u128::from(cap.saturating_sub(stock[good]));
                let volume = u128::from(rules.goods[good].volume_ml);
                let expected = if free == 0 {
                    0
                } else if volume == 0 {
                    by_cap
                } else {
                    by_cap.min(free / volume)
                };
                assert_eq!(exchange.room(&place, 0, good).unwrap(), expected);
            }
        }
    }

    #[test]
    fn fuel_wants_use_one_fixed_funded_deficit() {
        let rules = swap_tests::rules();
        let mut place = swap_tests::place(&rules, 1);
        place.pops[0].workers = 1;
        place.pops[0].dependents = 0;
        swap_tests::settle_the_rest(&rules, &mut place, &[]);
        swap_tests::spare(&rules, &mut place, 0, "wool", 1000);
        let wood = swap_tests::good(&rules, "hardwood");
        let peat = swap_tests::good(&rules, "peat");
        let target = fuel::fuel_daily_need(&rules, &place.pops[0]).unwrap() * 240;
        let supply = rules.goods[wood].category_supplied("fuel", 1);
        for (deficit, w, p) in [
            (1, 0, 0),
            (700, 0, 1),
            (701, 0, 1),
            (1000, 0, 1),
            (1400, 1, 2),
        ] {
            let held = target - deficit;
            place.pops[0].stock[wood] = u64::try_from(held / supply).unwrap();
            place.pops[0].stock[peat] = 0;
            place.pops[0].consumption.hearth_milli = u64::try_from(held % supply).unwrap();
            let mut book = Book::new(&rules, &place, &place.prices_milli).unwrap();
            assert_eq!(
                (book.position(0, wood).want, book.position(0, peat).want),
                (w, p)
            );
            if deficit == 1000 {
                place.pops[0].stock[peat] = 1;
                book.refresh_fuel(&rules, &place, 0).unwrap();
                assert_eq!(
                    (book.position(0, wood).want, book.position(0, peat).want),
                    (0, 1),
                    "refresh never refills or spends fixed orders"
                );
                let next = Book::new(&rules, &place, &place.prices_milli).unwrap();
                assert_eq!(
                    (next.position(0, wood).want, next.position(0, peat).want),
                    (0, 0)
                );
            }
        }
    }

    #[test]
    fn fuel_spare_is_recounted_after_a_load_leaves() {
        let rules = swap_tests::rules();
        let mut place = swap_tests::place(&rules, 1);
        place.pops[0].workers = 1;
        place.pops[0].dependents = 0;
        let wood = swap_tests::good(&rules, "hardwood");
        let peat = swap_tests::good(&rules, "peat");
        place.prices_milli[wood] = 100;
        place.prices_milli[peat] = 60;
        place.pops[0].stock[wood] = 5;
        place.pops[0].stock[peat] = 1;
        let mut book = Book::new(&rules, &place, &place.prices_milli).unwrap();
        assert_eq!(
            (book.position(0, wood).spare, book.position(0, peat).spare),
            (0, 1)
        );
        place.pops[0].stock[peat] = 0;
        book.refresh_fuel(&rules, &place, 0).unwrap();
        assert_eq!(
            (book.position(0, wood).spare, book.position(0, peat).spare),
            (0, 0)
        );
        assert_eq!(place.pops[0].consumption.hearth_milli, 0);
    }

    #[test]
    fn wealth_counts_personal_accounts_and_whole_building_holdings_once() {
        let rules = swap_tests::rules();
        let mut place = swap_tests::place(&rules, 2);
        for pop in &mut place.pops {
            pop.stock.fill(0);
            pop.dependents = 0;
        }
        place.pops[0].workers = 2;
        place.pops[1].workers = 3;
        let fish = swap_tests::good(&rules, "fish");
        let wood = swap_tests::good(&rules, "hardwood");
        place.prices_milli.fill(1);
        place.pops[0].stock[fish] = 10;
        place.pops[0].stock[wood] = 7;
        place.pops[1].stock[fish] = 20;
        let farm = swap_tests::add_building(&rules, &mut place, "farm", 1);
        swap_tests::own(&mut place, farm, 0, 1);
        swap_tests::own(&mut place, farm, 1, 1);
        let house = swap_tests::storehouse(&rules, &mut place);
        let accounts = place.buildings[house].storehouse_accounts.as_mut().unwrap();
        accounts.pops[0][fish] = 5;
        accounts.communal[fish] = 30;
        accounts.communal[wood] = 9_000;
        place.buildings[house].stock[fish] = 35;
        place.buildings[house].stock[wood] = 9_000;
        let book = Book::new(&rules, &place, &place.prices_milli).unwrap();
        let parts = BuildingParts::new(&rules, &place, &place.prices_milli).unwrap();
        let shares = Shares::new(&book, &parts, &place).unwrap();
        assert!(
            building_market_sells(&rules, &place, &place.prices_milli).unwrap()[house][wood] > 0
        );
        assert_eq!(parts.total(&book, 0, wood), 3_598);
        assert_eq!(parts.total(&book, 1, wood), 5_398);
        assert_eq!(shares.wealth, [(3_634, 2), (5_438, 3)]);
    }
}

#[cfg(test)]
mod visit_tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::expedition_tests::{at, hours, the_plan, tick, two_places};
    use crate::swap_tests::{PEASANT, THIRD, good, herd_rules, set_price, spare, target, want};

    #[test]
    fn free_unsold_food_follows_raw_need_without_spending_buy_orders() {
        let rules = crate::swap_tests::rules();
        let fish = good(&rules, "fish");
        let wool = good(&rules, "wool");
        for with_surplus in [false, true] {
            let mut place = crate::swap_tests::place(&rules, 1);
            place.pops[0].stock.fill(0);
            if with_surplus {
                place.pops[0].stock[wool] = 10_000;
            }
            let opening = place.prices_milli.clone();
            let mut day = PlaceDay::new_with(&rules, &place, &opening, vec![]).unwrap();
            let funded = |book: &Book| {
                book.funded.as_ref().unwrap()[0]
                    .iter()
                    .map(|row| (row.destination, row.funded, row.funded_value))
                    .collect::<Vec<_>>()
            };
            let before = funded(&day.book);
            if with_surplus {
                assert!(before.iter().any(|row| row.1 > 0));
            } else {
                assert!(before.iter().all(|row| row.1 == 0));
                assert_eq!(day.book.position(0, fish).want, 0);
            }
            assert!(day.book.raw_position(0, fish).want >= 3);
            let exchange = Exchange {
                rules: &rules,
                house: None,
            };
            assert_eq!(day.give_unsold(&exchange, &mut place, fish, 3).unwrap(), 0);
            assert_eq!(place.pops[0].stock[fish], 3);
            assert_eq!(funded(&day.book), before, "a gift spends no funded rows");
        }
    }

    #[test]
    fn visitor_gives_unsold_food_to_a_resident_with_no_surplus() {
        let mut rules = herd_rules();
        let mut state = two_places(&mut rules);
        let fish = good(&rules, "fish");
        state
            .order_trade(&rules, 0, PEASANT, the_plan(&rules, 6, 5), hours(at(10, 5)))
            .unwrap();
        for hour in 5..=11 {
            tick(&rules, &mut state, at(10, hour));
        }
        let mut visits = take_visits(&rules, &mut state, hours(at(10, 12))).unwrap();
        let place = &mut state.places[1];
        place.buildings.clear();
        for pop in &mut place.pops {
            pop.stock.fill(0);
        }
        assert_eq!(visits[1].len(), 1);
        visits[1][0].cargo.fill(0);
        visits[1][0].cargo[fish] = 1_000_000;
        let opening = place.prices_milli.clone();
        let day =
            PlaceDay::new_with(&rules, place, &opening, std::mem::take(&mut visits[1])).unwrap();
        assert_eq!(
            day.book.position(0, fish).want,
            0,
            "no surplus means no buy order"
        );
        assert!(day.book.raw_position(0, fish).want > 0);
        let returned = day.finish(&rules, place).unwrap();
        assert!(returned[0].expedition.cargo[fish] < 1_000_000);
        assert!(place.pops[0].stock[fish] > 0, "unsold food was given free");
    }

    /// A pop of the place can start a swap with a waiting expedition, as any pop may start one (r5 3.7), and neither side spends capacity.
    #[test]
    fn a_resident_starts_a_swap_with_a_visitor_and_spends_no_capacity() {
        let mut rules = herd_rules();
        let mut state = two_places(&mut rules);
        let (barley, fish) = (good(&rules, "barley"), good(&rules, "fish"));
        // The destination's third pop wants the visitor's barley and has the fish the visitor wants, with a day's capacity.
        crate::swap_tests::capacity(&mut rules, &mut state.places[1], THIRD, 1_000_000);
        want(&rules, &mut state.places[1], THIRD, "barley", 10);
        spare(&rules, &mut state.places[1], THIRD, "fish", 10);
        // The monks, who were the visitor's partners in the other cases, want and hold nothing here.
        state.places[1].pops[1].stock[barley] = target(&rules, &state.places[1], 1, "barley");
        state.places[1].pops[1].stock[fish] = target(&rules, &state.places[1], 1, "fish");
        set_price(&rules, &mut state.places[1], "barley", 9);
        set_price(&rules, &mut state.places[1], "fish", 10);
        state
            .order_trade(&rules, 0, PEASANT, the_plan(&rules, 6, 5), hours(at(10, 5)))
            .unwrap();
        for hour in 5..=11 {
            tick(&rules, &mut state, at(10, hour));
        }
        let now = hours(at(10, 12));
        let mut visits = take_visits(&rules, &mut state, now).unwrap();
        let opening = state.places[1].prices_milli.clone();
        let mut day = PlaceDay::new_with(
            &rules,
            &state.places[1],
            &opening,
            std::mem::take(&mut visits[1]),
        )
        .unwrap();
        let capacity_before = day.capacity.clone();
        let exchange = Exchange {
            rules: &rules,
            house: None,
        };
        let mut place = state.places[1].clone();
        let barley_before = place.pops[THIRD].stock[barley];
        day.resident_turn_with_visitors(&rules, &mut place, &exchange, THIRD, barley, false)
            .unwrap();
        assert!(
            day.visitors[0].received[fish] > 0,
            "the visitor received fish from the resident's turn"
        );
        assert!(
            place.pops[THIRD].stock[barley] > barley_before,
            "and the resident its barley"
        );
        assert_eq!(
            day.capacity, capacity_before,
            "no capacity was spent on either side"
        );
        let _ = BTreeMap::<u8, u8>::new();
    }

    /// Builds the day at the destination with one barley aboard the visitor, a barley worth 10 and a fish 11: the visitor cannot start a
    /// swap (its one barley is worth less than the one fish it would take, and two would be more than it has), but a resident that
    /// gives a fish worth 11 for the barley worth 10 loses under a tenth, which it may.
    fn rounding_day() -> (Rules, EconomyState, PlaceDay, usize, usize) {
        let mut rules = herd_rules();
        let mut state = two_places(&mut rules);
        let (barley, fish) = (good(&rules, "barley"), good(&rules, "fish"));
        crate::swap_tests::capacity(&mut rules, &mut state.places[1], THIRD, 1_000_000);
        want(&rules, &mut state.places[1], THIRD, "barley", 10);
        spare(&rules, &mut state.places[1], THIRD, "fish", 10);
        state.places[1].pops[1].stock[barley] = target(&rules, &state.places[1], 1, "barley");
        state.places[1].pops[1].stock[fish] = target(&rules, &state.places[1], 1, "fish");
        set_price(&rules, &mut state.places[1], "barley", 10);
        set_price(&rules, &mut state.places[1], "fish", 11);
        let mut plan = the_plan(&rules, 6, 5);
        plan.cargo[barley] = 1;
        state
            .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
            .unwrap();
        for hour in 5..=11 {
            tick(&rules, &mut state, at(10, hour));
        }
        let mut visits = take_visits(&rules, &mut state, hours(at(10, 12))).unwrap();
        let opening = state.places[1].prices_milli.clone();
        let day = PlaceDay::new_with(
            &rules,
            &state.places[1],
            &opening,
            std::mem::take(&mut visits[1]),
        )
        .unwrap();
        (rules, state, day, barley, fish)
    }

    /// The ten-for-nine case: where the visitor cannot start a swap at the loss limit a resident can, and one whose capacity is used up
    /// still tries the visitors (they spend none), while a pop that owns nothing has no turn at all (r5 3.7, T30).
    #[test]
    fn a_resident_starts_where_the_visitor_cannot_and_one_with_no_capacity_left_still_tries_visitors()
     {
        for (owns, used_up, traded) in [
            (true, false, true),
            (true, true, true),
            (false, false, false),
        ] {
            let (rules, state, mut day, barley, fish) = rounding_day();
            let mut place = state.places[1].clone();
            day.owns[THIRD] = owns;
            if used_up {
                day.capacity[THIRD] = 0;
            }
            // The monks are the only other pop that wants the barley, and they cannot start; the visitor's own turn finds no pair.
            day.visitor_pass(
                &rules,
                &mut place,
                &Exchange {
                    rules: &rules,
                    house: None,
                },
                false,
            )
            .unwrap();
            assert_eq!(
                day.visitors[0].received[fish], 0,
                "the visitor cannot start this one"
            );
            day.local_pass(&rules, &mut place, false).unwrap();
            assert_eq!(
                day.visitors[0].received[fish] > 0,
                traded,
                "owns {owns}, used up {used_up}"
            );
            if traded {
                assert_eq!(day.visitors[0].received[fish], 1);
                assert_eq!(day.visitors[0].cargo[barley], 0);
            }
        }
    }
}
