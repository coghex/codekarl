//! The daily swap between two pops of one place (CK-391, trade slice 3a).
//!
//! Once a day, at noon, each place's pops exchange goods: a pop short of a good swaps with a pop in the opposite position, one good
//! each way, in whole units, in near-equal value at the place's prices. A pop's market capacity comes from the buildings it owns, and
//! a swap spends the starter's. Nothing here is saved: spare, want and capacity are worked out afresh at each exchange.

use crate::{
    DataError, EconomyState, PlaceState, Rules, building_market_holding, building_market_sells,
    fuel, missing_data, mul_div_floor, pop_market_holding, pop_store_limits, pop_target_units,
    receive_pop, volume_used,
};

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
    order.sort_by(|&left, &right| {
        let (a, b) = (&rules.goods[left], &rules.goods[right]);
        let group = |g: &crate::Good| {
            if g.hunger_centi > 0 {
                0
            } else if g.category_supplied("fuel", 1) > 0 {
                1
            } else {
                2
            }
        };
        let food = group(a).cmp(&group(b));
        if group(a) == 1 && group(b) == 1 {
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
}

impl Book {
    /// A book of the given targets and holdings, for the tests that set a pair's positions directly.
    #[cfg(test)]
    pub(crate) fn from_parts(target: Vec<Vec<u128>>, holding: Vec<Vec<u128>>) -> Self {
        let hungry = vec![false; target.len()];
        let fuel = target.iter().map(|row| vec![None; row.len()]).collect();
        Self {
            target,
            holding,
            fuel,
            fuel_prices: Vec::new(),
            hungry,
        }
    }

    fn new(rules: &Rules, place: &PlaceState, opening: &[u64]) -> Result<Self, DataError> {
        let mut book = Self {
            target: Vec::with_capacity(place.pops.len()),
            holding: Vec::with_capacity(place.pops.len()),
            hungry: Vec::with_capacity(place.pops.len()),
            fuel: Vec::new(),
            fuel_prices: opening.to_vec(),
        };
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
        self.fuel[pop_index] = rules
            .goods
            .iter()
            .enumerate()
            .map(|(i, good)| {
                let supply = good.category_supplied("fuel", 1);
                (supply > 0).then(|| Position {
                    spare: u128::from(spare[i]),
                    want: target.saturating_sub(holding).div_ceil(supply),
                })
            })
            .collect();
        Ok(())
    }

    pub(crate) fn position(&self, pop: usize, good: usize) -> Position {
        if let Some(position) = self.fuel[pop][good] {
            return position;
        }
        let (target, holding) = (self.target[pop][good], self.holding[pop][good]);
        Position {
            spare: holding.saturating_sub(target),
            want: target.saturating_sub(holding),
        }
    }

    /// Follows a made swap: the starter gave one good and received the other, the partner the other way about.
    fn settle(&mut self, trade: Trade) {
        let (give, get) = (u128::from(trade.give_units), u128::from(trade.get_units));
        if trade.starter_source == 0 {
            self.holding[trade.starter][trade.give_good] -= give;
        }
        self.holding[trade.starter][trade.get_good] += get;
        self.holding[trade.partner][trade.give_good] += give;
        if trade.partner_source == 0 {
            self.holding[trade.partner][trade.get_good] -= get;
        }
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
    fn apply(&self, place: &mut PlaceState, trade: Trade) -> Result<(), DataError> {
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
        for (pop, good, units) in [
            (trade.starter, trade.get_good, trade.get_units),
            (trade.partner, trade.give_good, trade.give_units),
        ] {
            let unaccepted = receive_pop(self.rules, &mut place.pops[pop], good, units)?;
            if unaccepted != 0 {
                return Err(missing_data(
                    "swap.pair".to_owned(),
                    "an allowed pair no longer has room",
                ));
            }
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

pub(crate) fn best_pair_capped(
    exchange: &Exchange<'_>,
    place: &PlaceState,
    book: &Book,
    [starter, partner]: [usize; 2],
    [give_good, get_good]: [usize; 2],
    capacity_left: u128,
    [give_stock, get_stock, give_limit, get_limit]: [u128; 4],
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
        .min(exchange.room(place, starter, get_good)?);
    let most_give = give_stock
        .min(give_limit)
        .min(book.position(partner, give_good).want)
        .min(exchange.room(place, partner, give_good)?);
    if most_get == 0 || most_give == 0 || prices.give_price == 0 || prices.get_price == 0 {
        return Ok(None);
    }
    let overflow = || missing_data("swap.pair".to_owned(), "pair value overflow");
    let limit = u128::from(rules.swap_loss_limit_pm);
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

fn run_place_exchange(
    rules: &Rules,
    place: &mut PlaceState,
    opening: &[u64],
) -> Result<(), DataError> {
    if place.pops.len() < 2 {
        return Ok(());
    }
    let mut capacity = market_capacities(rules, place)?;
    if capacity.iter().all(|left| *left == 0) {
        return Ok(());
    }
    if place.prices_milli.len() != rules.goods.len() || opening.len() != rules.goods.len() {
        return Err(missing_data(
            format!("places.{}.prices", place.id),
            "saved price dimensions do not match goods",
        ));
    }
    let order = want_order_at_prices(rules, &place.prices_milli, opening);
    let mut book = Book::new(rules, place, opening)?;
    let exchange = Exchange {
        rules,
        house: place
            .buildings
            .iter()
            .position(|building| building.storehouse_accounts.is_some()),
    };
    let mut parts = BuildingParts::new(rules, place, opening)?;
    let mut shares = Shares::new(&book, &parts, place)?;
    for pass in 0..2 {
        if pass == 1 {
            shares.second_pass(&book, &parts)?;
        }
        for (starter, left) in capacity.iter_mut().enumerate() {
            if *left == 0 {
                continue;
            }
            for &get_good in &order {
                if book.hungry[starter] && rules.goods[get_good].hunger_centi == 0 {
                    continue;
                }
                let mut turn = Turn {
                    exchange: &exchange,
                    order: &order,
                    starter,
                    get_good,
                };
                turn.work(place, &mut book, &mut parts, &mut shares, left, pass == 1)?;
                if *left == 0 {
                    break;
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
    fn work(
        &mut self,
        place: &mut PlaceState,
        book: &mut Book,
        parts: &mut BuildingParts,
        shares: &mut Shares,
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
                        let Some((give, get)) = best_pair_capped(
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
                        };
                        self.exchange.apply(place, trade)?;
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
        let ordinary = crate::room_by_cap_and_volume(
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
                crate::room_by_cap_and_volume(&rules, stock, wood, cap, capacity, None, u64::MAX)
                    .unwrap();
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
