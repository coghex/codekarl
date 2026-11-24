//! The daily swap between two pops of one place (CK-391, trade slice 3a).
//!
//! Once a day, at noon, each place's pops exchange goods: a pop short of a good swaps with a pop in the opposite position, one good
//! each way, in whole units, in near-equal value at the place's prices. A pop's market capacity comes from the buildings it owns, and
//! a swap spends the starter's. Nothing here is saved: spare, want and capacity are worked out afresh at each exchange.

use crate::{
    DataError, EconomyState, PlaceState, Rules, missing_data, pop_market_holding, pop_store_limits,
    pop_target_units, receive_pop, volume_used,
};

/// Runs every place's exchange.
///
/// # Errors
/// The first data error, leaving `state` partly changed: the caller works on a copy.
pub(crate) fn run_daily_swaps(rules: &Rules, state: &mut EconomyState) -> Result<(), DataError> {
    for place in &mut state.places {
        run_place_exchange(rules, place)?;
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
pub(crate) fn want_order(rules: &Rules, prices_milli: &[u64]) -> Vec<usize> {
    let mut order = (0..rules.goods.len()).collect::<Vec<_>>();
    order.sort_by(|&left, &right| {
        let (a, b) = (&rules.goods[left], &rules.goods[right]);
        let food = (a.hunger_centi == 0).cmp(&(b.hunger_centi == 0));
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
}

impl Book {
    /// A book of the given targets and holdings, for the tests that set a pair's positions directly.
    #[cfg(test)]
    pub(crate) fn from_parts(target: Vec<Vec<u128>>, holding: Vec<Vec<u128>>) -> Self {
        let hungry = vec![false; target.len()];
        Self {
            target,
            holding,
            hungry,
        }
    }

    fn new(rules: &Rules, place: &PlaceState) -> Result<Self, DataError> {
        let mut book = Self {
            target: Vec::with_capacity(place.pops.len()),
            holding: Vec::with_capacity(place.pops.len()),
            hungry: Vec::with_capacity(place.pops.len()),
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
                target.push(pop_target_units(rules, &pop.kind, people, good_index)?);
                let held = pop_market_holding(place, pop_index, good_index)?;
                holding.push(held);
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
        Ok(book)
    }

    pub(crate) fn position(&self, pop: usize, good: usize) -> Position {
        let (target, holding) = (self.target[pop][good], self.holding[pop][good]);
        Position {
            spare: holding.saturating_sub(target),
            want: target.saturating_sub(holding),
        }
    }

    /// Follows a made swap: the starter gave one good and received the other, the partner the other way about.
    fn settle(&mut self, trade: Trade) {
        let (give, get) = (u128::from(trade.give_units), u128::from(trade.get_units));
        self.holding[trade.starter][trade.give_good] -= give;
        self.holding[trade.starter][trade.get_good] += get;
        self.holding[trade.partner][trade.give_good] += give;
        self.holding[trade.partner][trade.get_good] -= get;
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

    /// Makes a swap: both loads leave their pops, and what each pop receives goes into its own store and nowhere else (rule 13).
    fn apply(&self, place: &mut PlaceState, trade: Trade) -> Result<(), DataError> {
        self.pay(place, trade.starter, trade.give_good, trade.give_units)?;
        self.pay(place, trade.partner, trade.get_good, trade.get_units)?;
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
pub(crate) fn best_pair(
    exchange: &Exchange<'_>,
    place: &PlaceState,
    book: &Book,
    [starter, partner]: [usize; 2],
    [give_good, get_good]: [usize; 2],
    capacity_left: u128,
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
        .min(book.position(partner, get_good).spare)
        .min(exchange.room(place, starter, get_good)?);
    let most_give = book
        .position(starter, give_good)
        .spare
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

fn run_place_exchange(rules: &Rules, place: &mut PlaceState) -> Result<(), DataError> {
    if place.pops.len() < 2 {
        return Ok(());
    }
    let mut capacity = market_capacities(rules, place)?;
    if capacity.iter().all(|left| *left == 0) {
        return Ok(());
    }
    if place.prices_milli.len() != rules.goods.len() {
        return Err(missing_data(
            format!("places.{}.prices", place.id),
            "saved price dimensions do not match goods",
        ));
    }
    let order = want_order(rules, &place.prices_milli);
    let mut book = Book::new(rules, place)?;
    let exchange = Exchange {
        rules,
        house: place
            .buildings
            .iter()
            .position(|building| building.storehouse_accounts.is_some()),
    };
    for (starter, left) in capacity.iter_mut().enumerate() {
        // A pop with no capacity has no turn (rule 8).
        if *left == 0 {
            continue;
        }
        for &get_good in &order {
            // A hungry starter works only its wants for goods with a hunger value (rule 18).
            if book.hungry[starter] && rules.goods[get_good].hunger_centi == 0 {
                continue;
            }
            let mut turn = Turn {
                exchange: &exchange,
                order: &order,
                starter,
                get_good,
            };
            turn.work(place, &mut book, left)?;
            if *left == 0 {
                break;
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
        capacity: &mut u128,
    ) -> Result<(), DataError> {
        let rules = self.exchange.rules;
        for partner in 0..place.pops.len() {
            if partner == self.starter || book.position(partner, self.get_good).spare == 0 {
                continue;
            }
            for &give_good in self.order {
                // Until the want is met or the capacity is used up.
                if *capacity == 0 || book.position(self.starter, self.get_good).want == 0 {
                    return Ok(());
                }
                if give_good == self.get_good
                    || book.position(partner, give_good).want == 0
                    || book.position(self.starter, give_good).spare == 0
                {
                    continue;
                }
                let Some((give, get)) = best_pair(
                    self.exchange,
                    place,
                    book,
                    [self.starter, partner],
                    [give_good, self.get_good],
                    *capacity,
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
                };
                self.exchange.apply(place, trade)?;
                book.settle(trade);
                *capacity -= u128::from(give) * u128::from(rules.goods[give_good].volume_ml)
                    + u128::from(get) * u128::from(rules.goods[self.get_good].volume_ml);
            }
        }
        Ok(())
    }
}
