//! The daily swap between two pops of one place (CK-391, trade slice 3a).
//!
//! Once a day, at noon, each place's pops exchange goods: a pop short of a good swaps with a pop in the opposite position, one good
//! each way, in whole units, in near-equal value at the place's prices. A pop's market capacity comes from the buildings it owns, and
//! a swap spends the starter's. Nothing here is saved: spare, want and capacity are worked out afresh at each exchange.

use crate::{
    DataError, EconomyState, PlaceState, Rules, building_good_cap, building_market_holding,
    building_market_sells, fuel, herd_wants, live_species, missing_data, mul_div_floor,
    pop_market_holding, pop_store_limits, pop_target_units, receive_building, receive_pop,
    room_by_cap_and_volume, store_capacity_ml, volume_used,
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

pub(crate) fn run_daily_swaps_at_prices(
    rules: &Rules,
    state: &mut EconomyState,
    prices: &[Vec<u64>],
) -> Result<(), DataError> {
    if prices.len() != state.places.len() {
        return Err(missing_data(
            "swap.prices".to_owned(),
            "price dimensions do not match places",
        ));
    }
    for (place, opening) in state.places.iter_mut().zip(prices) {
        run_place_exchange(rules, place, opening)?;
    }
    Ok(())
}

/// Runs both exchange phases over one set of opening books and market capacities.
#[expect(
    clippy::too_many_lines,
    reason = "The two ordered phases share the same opening state and allocations."
)]
pub(crate) fn run_neighbour_swaps_at_prices(
    rules: &Rules,
    state: &mut EconomyState,
    prices: &[Vec<u64>],
    neighbours: &std::collections::BTreeMap<String, Vec<crate::NearbyPlace>>,
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
        return run_daily_swaps_at_prices(rules, state, prices);
    }
    let mut order: Vec<usize> = (0..count).collect();
    order.sort_by(|&a, &b| state.places[a].id.cmp(&state.places[b].id));
    let mut days = state
        .places
        .iter()
        .zip(prices)
        .map(|(place, opening)| PlaceDay::new(rules, place, opening))
        .collect::<Result<Vec<_>, _>>()?;
    let mut shares = NeighbourShares::new(&days, &state.places, &adjacent, rules.goods.len())?;
    for second in [false, true] {
        if second {
            shares.second_pass(&days, &adjacent)?;
        }
        for &a in &order {
            days[a].shares.scarce.clone_from(&shares.scarce[a]);
            days[a].shares.left.clone_from(&shares.left[a][a]);
            days[a].local_pass(rules, &mut state.places[a], second)?;
            shares.left[a][a].clone_from(&days[a].shares.left);
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
/// holding owner positions in it by those positions (largest remainder, the earlier pop on a tie), summed over the pop's buildings.
pub(crate) fn market_capacities(rules: &Rules, place: &PlaceState) -> Result<Vec<u128>, DataError> {
    let mut capacity = vec![0_u128; place.pops.len()];
    for building in &place.buildings {
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
        let weights = building
            .employment
            .iter()
            .map(|job| job.owners)
            .collect::<Vec<_>>();
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
/// is divided by the sum first, so any total and any owner positions that fit are shared exactly.
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
    target: Vec<Vec<u128>>,
    holding: Vec<Vec<u128>>,
    /// Whether each pop holds less than 60 days of food, counted once when the exchange begins (rule 18).
    hungry: Vec<bool>,
    fuel: Vec<Vec<Option<Position>>>,
    fuel_prices: Vec<u64>,
    /// Whether each good is a live animal: a pop's want for one is its herds' shortfall, it holds none, and what it receives joins a
    /// herd (CK-430).
    live: Vec<bool>,
    /// What each pop has been paid into a building's communal stock for an animal sold from its herd, by good (CK-430): spent from
    /// the pop's want of the good, though it is no part of the pop's own holding, so one want is settled once.
    communal: Vec<Vec<u128>>,
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
            target,
            holding,
            fuel,
            fuel_prices: Vec::new(),
            hungry,
            live,
            communal: vec![vec![0; target_width]; target_height],
        }
    }

    fn new(rules: &Rules, place: &PlaceState, opening: &[u64]) -> Result<Self, DataError> {
        let mut book = Self {
            target: Vec::with_capacity(place.pops.len()),
            holding: Vec::with_capacity(place.pops.len()),
            hungry: Vec::with_capacity(place.pops.len()),
            fuel: Vec::new(),
            fuel_prices: opening.to_vec(),
            live: live_species(rules).iter().map(Option::is_some).collect(),
            communal: vec![vec![0; rules.goods.len()]; place.pops.len()],
        };
        let live = live_species(rules);
        let wants = herd_wants(rules, place)?;
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
                    // The want is what the pop's herds lack of their standard levels; the pop holds no animal itself.
                    target.push(
                        wants
                            .iter()
                            .map(|building| u128::from(building[pop_index][species]))
                            .sum(),
                    );
                    holding.push(0);
                } else {
                    target.push(pop_target_units(rules, &pop.kind, people, good_index)?);
                    holding.push(held);
                }
                food = held
                    .checked_mul(u128::from(good.hunger_centi))
                    .and_then(|value| food.checked_add(value))
                    .ok_or_else(|| missing_data("swap.food".to_owned(), "food value overflow"))?;
            }
            book.hungry
                .push(people > 0 && food < u128::from(people) * 100 * 60);
            book.target.push(target);
            book.holding.push(holding);
        }
        book.fuel = vec![vec![None; rules.goods.len()]; place.pops.len()];
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

    pub(crate) fn position(&self, pop: usize, good: usize) -> Position {
        if let Some(position) = self.fuel[pop][good] {
            // A fuel's want already counts what the pop was paid into a building (see `refresh_fuel`).
            return position;
        }
        let paid = self.communal[pop][good];
        let (target, holding) = (self.target[pop][good], self.holding[pop][good]);
        Position {
            spare: holding.saturating_sub(target),
            want: target.saturating_sub(holding).saturating_sub(paid),
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
        } else {
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
struct BuildingParts {
    spare: Vec<Vec<Vec<u128>>>,
    held: Vec<Vec<Vec<u128>>>,
}

impl BuildingParts {
    fn new(rules: &Rules, place: &PlaceState, opening: &[u64]) -> Result<Self, DataError> {
        let sells = building_market_sells(rules, place, opening)?;
        let mut spare =
            vec![vec![vec![0; rules.goods.len()]; place.pops.len()]; place.buildings.len()];
        let mut held = spare.clone();
        for (b, building) in place.buildings.iter().enumerate() {
            let weights: Vec<_> = building.employment.iter().map(|job| job.owners).collect();
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
    claimants: Vec<Vec<bool>>,
    wealth: Vec<(u128, u128)>,
}

impl Shares {
    fn eligible(book: &Book, parts: &BuildingParts, pop: usize, good: usize) -> bool {
        book.position(pop, good).want > 0
            && (0..book.holding.len()).any(|holder| {
                holder != pop
                    && parts.total(book, holder, good) > 0
                    && (0..book.target[pop].len()).any(|payment| {
                        payment != good
                            && parts.total(book, pop, payment) > 0
                            && book.position(holder, payment).want > 0
                    })
            })
    }

    fn new(book: &Book, parts: &BuildingParts, place: &PlaceState) -> Result<Self, DataError> {
        let pops = place.pops.len();
        let goods = place.prices_milli.len();
        let overflow = || missing_data("swap.shares".to_owned(), "share or wealth overflow");
        let mut wealth = Vec::with_capacity(pops);
        for pop in 0..pops {
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
        let mut result = Self {
            scarce: vec![false; goods],
            left: vec![vec![0; goods]; pops],
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

    fn limit(&self, pop: usize, good: usize) -> u128 {
        if self.scarce[good] {
            self.left[pop][good]
        } else {
            u128::MAX
        }
    }
    fn receive(&mut self, pop: usize, good: usize, units: u64) {
        if self.scarce[good] {
            self.left[pop][good] -= u128::from(units);
        }
    }
}

/// Scarcity allocations for each holder place, over local and directly neighbouring claimants.
struct NeighbourShares {
    scarce: Vec<Vec<bool>>,
    left: Vec<Vec<Vec<Vec<u128>>>>,
    claimants: Vec<Vec<Vec<Vec<bool>>>>,
    wealth: Vec<Vec<(u128, u128)>>,
}

impl NeighbourShares {
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
        let origin = &days[holder];
        (0..origin.book.holding.len()).any(|seller| {
            (source != holder || seller != pop)
                && origin.parts.total(&origin.book, seller, good) > 0
                && (0..origin.book.target[seller].len()).any(|payment| {
                    payment != good
                        && claimant.parts.total(&claimant.book, pop, payment) > 0
                        && origin.book.position(seller, payment).want > 0
                })
        })
    }

    fn new(
        days: &[PlaceDay],
        places: &[PlaceState],
        adjacent: &[Vec<bool>],
        goods: usize,
    ) -> Result<Self, DataError> {
        let overflow = || missing_data("swap.shares".to_owned(), "share or wealth overflow");
        let mut wealth = Vec::new();
        for (day, place) in days.iter().zip(places) {
            let mut values = Vec::new();
            for pop in 0..place.pops.len() {
                let mut value = 0_u128;
                for good in 0..goods {
                    let mut held = pop_market_holding(place, pop, good)?;
                    for building in &day.parts.held {
                        held = held.checked_add(building[pop][good]).ok_or_else(overflow)?;
                    }
                    value = value
                        .checked_add(
                            held.checked_mul(u128::from(place.prices_milli[good]))
                                .ok_or_else(overflow)?,
                        )
                        .ok_or_else(overflow)?;
                }
                values.push((value, u128::from(Exchange::people(place, pop)?.max(1))));
            }
            wealth.push(values);
        }
        let mut out = Self {
            scarce: vec![vec![false; goods]; places.len()],
            left: places
                .iter()
                .enumerate()
                .map(|(holder, _)| {
                    places
                        .iter()
                        .enumerate()
                        .map(|(source, p)| {
                            if source == holder || adjacent[holder][source] {
                                vec![vec![0; goods]; p.pops.len()]
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
                        .map(|(source, p)| {
                            if source == holder || adjacent[holder][source] {
                                vec![vec![false; goods]; p.pops.len()]
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
                let mut demand = 0_u128;
                let mut supply = 0_u128;
                for pop in 0..places[holder].pops.len() {
                    supply = supply
                        .checked_add(days[holder].parts.total(&days[holder].book, pop, good))
                        .ok_or_else(overflow)?;
                }
                for source in 0..places.len() {
                    if source != holder && !adjacent[holder][source] {
                        continue;
                    }
                    for pop in 0..places[source].pops.len() {
                        let eligible = Self::eligible(days, adjacent, holder, source, pop, good);
                        out.claimants[holder][source][pop][good] = eligible;
                        if eligible {
                            demand = demand
                                .checked_add(days[source].book.position(pop, good).want)
                                .ok_or_else(overflow)?;
                        }
                    }
                }
                if demand > supply {
                    out.scarce[holder][good] = true;
                    out.allocate(days, holder, good, supply, demand, false, adjacent)?;
                }
            }
        }
        Ok(out)
    }

    fn second_pass(&mut self, days: &[PlaceDay], adjacent: &[Vec<bool>]) -> Result<(), DataError> {
        let overflow = || missing_data("swap.shares".to_owned(), "second share overflow");
        for holder in 0..days.len() {
            for good in 0..self.scarce[holder].len() {
                if !self.scarce[holder][good] {
                    continue;
                }
                let mut supply = 0_u128;
                let mut demand = 0_u128;
                for pop in 0..days[holder].book.holding.len() {
                    supply = supply
                        .checked_add(days[holder].parts.total(&days[holder].book, pop, good))
                        .ok_or_else(overflow)?;
                }
                for source in 0..days.len() {
                    if source != holder && !adjacent[holder][source] {
                        continue;
                    }
                    for pop in 0..days[source].book.holding.len() {
                        if self.claimants[holder][source][pop][good]
                            && self.left[holder][source][pop][good] == 0
                            && Self::eligible(days, adjacent, holder, source, pop, good)
                        {
                            demand = demand
                                .checked_add(days[source].book.position(pop, good).want)
                                .ok_or_else(overflow)?;
                        }
                    }
                }
                self.allocate(days, holder, good, supply, demand, true, adjacent)?;
            }
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Allocation needs its holder, good, supply, demand and phase."
    )]
    fn allocate(
        &mut self,
        days: &[PlaceDay],
        holder: usize,
        good: usize,
        supply: u128,
        demand: u128,
        second: bool,
        adjacent: &[Vec<bool>],
    ) -> Result<(), DataError> {
        let overflow = || missing_data("swap.shares".to_owned(), "share overflow");
        let mut assigned = 0_u128;
        let mut order = Vec::new();
        for source in 0..days.len() {
            if source != holder && !adjacent[holder][source] {
                continue;
            }
            for pop in 0..days[source].book.holding.len() {
                let eligible = self.claimants[holder][source][pop][good]
                    && (!second
                        || (self.left[holder][source][pop][good] == 0
                            && Self::eligible(days, adjacent, holder, source, pop, good)));
                let want = days[source].book.position(pop, good).want;
                let share = if !eligible {
                    0
                } else if supply >= demand {
                    want
                } else {
                    mul_div_floor(supply, want, demand).ok_or_else(overflow)?
                };
                self.left[holder][source][pop][good] = share;
                assigned = assigned.checked_add(share).ok_or_else(overflow)?;
                if eligible {
                    order.push((source, pop));
                }
            }
        }
        order.sort_by(|&(a, ap), &(b, bp)| {
            let (av, an) = self.wealth[a][ap];
            let (bv, bn) = self.wealth[b][bp];
            (bv / bn)
                .cmp(&(av / an))
                .then_with(|| ((bv % bn) * an).cmp(&((av % an) * bn)))
                .then((a, ap).cmp(&(b, bp)))
        });
        let mut remain = supply.saturating_sub(assigned);
        for (source, pop) in order {
            if remain == 0 {
                break;
            }
            if self.left[holder][source][pop][good] < days[source].book.position(pop, good).want {
                self.left[holder][source][pop][good] += 1;
                remain -= 1;
            }
        }
        Ok(())
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
        herds: &mut HerdWants,
        trade: Trade,
    ) -> Result<(), DataError> {
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
        self.receive(
            place,
            herds,
            trade.starter,
            trade.get_good,
            trade.get_units,
            trade.get_to_building,
        )?;
        self.receive(
            place,
            herds,
            trade.partner,
            trade.give_good,
            trade.give_units,
            trade.give_to_building,
        )?;
        Ok(())
    }

    /// Receives one side of a trade at that side's place.
    fn receive(
        &self,
        place: &mut PlaceState,
        herds: &mut HerdWants,
        pop: usize,
        good: usize,
        units: u64,
        to_building: Option<usize>,
    ) -> Result<(), DataError> {
        if let Some(species) = self.live(good) {
            return Self::receive_head(place, herds, pop, species, units);
        }
        let unaccepted = if let Some(building) = to_building {
            receive_building(self.rules, &mut place.buildings[building], good, units)?
        } else {
            receive_pop(self.rules, &mut place.pops[pop], good, units)?
        };
        if unaccepted != 0 {
            return Err(missing_data(
                "swap.pair".to_owned(),
                "an allowed pair no longer has room",
            ));
        }
        Ok(())
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
        let mut left = units;
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
                "an animal arrived that no herd of the pop wants",
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
    [get_room, give_room]: [Option<u128>; 2],
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
        .min(match get_room {
            Some(room) => room,
            None => exchange.room(place, starter, get_good)?,
        });
    let most_give = give_stock
        .min(give_limit)
        .min(book.position(partner, give_good).want)
        .min(match give_room {
            Some(room) => room,
            None => exchange.room(place, partner, give_good)?,
        });
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

/// The exchange's opening positions and remaining capacity persist through all of its phases.
struct PlaceDay {
    order: Vec<usize>,
    book: Book,
    parts: BuildingParts,
    shares: Shares,
    herds: HerdWants,
    capacity: Vec<u128>,
}

impl PlaceDay {
    fn new(rules: &Rules, place: &PlaceState, opening: &[u64]) -> Result<Self, DataError> {
        if place.prices_milli.len() != rules.goods.len() || opening.len() != rules.goods.len() {
            return Err(missing_data(
                format!("places.{}.prices", place.id),
                "saved price dimensions do not match goods",
            ));
        }
        let book = Book::new(rules, place, opening)?;
        let parts = BuildingParts::new(rules, place, opening)?;
        let shares = Shares::new(&book, &parts, place)?;
        Ok(Self {
            order: want_order_at_prices(rules, &place.prices_milli, opening),
            book,
            parts,
            shares,
            herds: herd_wants(rules, place)?,
            capacity: market_capacities(rules, place)?,
        })
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
        for (starter, left) in self.capacity.iter_mut().enumerate() {
            if *left == 0 {
                continue;
            }
            for &get_good in &self.order {
                if self.book.hungry[starter] && rules.goods[get_good].hunger_centi == 0 {
                    continue;
                }
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
                    left,
                    second,
                )?;
                if *left == 0 {
                    break;
                }
            }
        }
        Ok(())
    }
}

fn run_place_exchange(
    rules: &Rules,
    place: &mut PlaceState,
    opening: &[u64],
) -> Result<(), DataError> {
    if place.pops.len() < 2 {
        return Ok(());
    }
    let mut day = PlaceDay::new(rules, place, opening)?;
    day.local_pass(rules, place, false)?;
    day.shares.second_pass(&day.book, &day.parts)?;
    day.local_pass(rules, place, true)
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
                            let get_room = match gb {
                                Some(b) => se.building_room(starter_place, b, get_good)?,
                                None => se.room(starter_place, starter, get_good)?,
                            };
                            let give_room = match pb {
                                Some(b) => pe.building_room(partner_place, b, give_good)?,
                                None => pe.room(partner_place, partner, give_good)?,
                            };
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
                            se.pay_source(starter_place, ss, starter, give_good, give)?;
                            pe.pay_source(partner_place, ps, partner, get_good, get)?;
                            se.receive(
                                starter_place,
                                &mut starter_day.herds,
                                starter,
                                get_good,
                                get,
                                gb,
                            )?;
                            pe.receive(
                                partner_place,
                                &mut partner_day.herds,
                                partner,
                                give_good,
                                give,
                                pb,
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
                        let rooms = [
                            get_to_building
                                .map(|building| {
                                    self.exchange.building_room(place, building, self.get_good)
                                })
                                .transpose()?,
                            give_to_building
                                .map(|building| {
                                    self.exchange.building_room(place, building, give_good)
                                })
                                .transpose()?,
                        ];
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
                                shares.limit(partner, give_good),
                                shares.limit(self.starter, self.get_good),
                            ],
                            rooms,
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
                        self.exchange.apply(place, herds, trade)?;
                        book.settle(trade);
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
                        shares.receive(partner, give_good, give);
                        shares.receive(self.starter, self.get_good, get);
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
    fn fuel_wants_are_recomputed_from_one_deficit() {
        let rules = swap_tests::rules();
        let mut place = swap_tests::place(&rules, 1);
        let wood = swap_tests::good(&rules, "hardwood");
        let peat = swap_tests::good(&rules, "peat");
        place.pops[0].workers = 1;
        place.pops[0].dependents = 0;
        let target = fuel::fuel_daily_need(&rules, &place.pops[0]).unwrap() * 240;
        for (deficit, w, p) in [
            (1, 1, 1),
            (700, 1, 1),
            (701, 1, 2),
            (1000, 1, 2),
            (1400, 2, 2),
        ] {
            place.pops[0].consumption.hearth_milli = u64::try_from(target - deficit).unwrap();
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
                    (1, 1)
                );
                place.pops[0].stock[peat] = 0;
                place.pops[0].stock[wood] = 1;
                book.refresh_fuel(&rules, &place, 0).unwrap();
                assert_eq!(
                    (book.position(0, wood).want, book.position(0, peat).want),
                    (0, 0)
                );
                place.pops[0].stock[wood] = 0;
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
        let house = swap_tests::storehouse(&rules, &mut place);
        place.buildings[house].employment[0].owners = 2;
        place.buildings[house].employment[1].owners = 1;
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
        assert_eq!(parts.total(&book, 0, wood), 5_996);
        assert_eq!(parts.total(&book, 1, wood), 2_998);
        assert_eq!(shares.wealth, [(6_042, 2), (3_030, 3)]);
    }
}
