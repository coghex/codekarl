//! Fuel accounting and ordering for the daily economy.
use super::{
    BuildingState, DailyConsumptionLedger, DataError, PlaceState, PopState, Rules,
    debit_storehouse, missing_data, receive_pop,
};
pub(super) fn fuel_ranking(
    rules: &Rules,
    prices: &[u64],
    dearest_first: bool,
) -> Result<Vec<usize>, DataError> {
    if prices.len() != rules.goods.len() {
        return Err(missing_data(
            "fuel.prices".to_owned(),
            "price dimensions do not match goods",
        ));
    }
    let mut indices: Vec<usize> = rules
        .goods
        .iter()
        .enumerate()
        .filter(|(_, good)| good.category_supplied("fuel", 1) > 0)
        .map(|(index, _)| index)
        .collect();
    indices.sort_by(|&a, &b| {
        let sa = rules.goods[a].category_supplied("fuel", 1);
        let sb = rules.goods[b].category_supplied("fuel", 1);
        let order = (u128::from(prices[a]) * sb).cmp(&(u128::from(prices[b]) * sa));
        if dearest_first {
            order.reverse()
        } else {
            order
        }
    });
    Ok(indices)
}

pub(super) fn fuel_holding(rules: &Rules, stock: &[u64], hearth: u64) -> Result<u128, DataError> {
    if stock.len() != rules.goods.len() {
        return Err(missing_data(
            "fuel.stock".to_owned(),
            "stock dimensions do not match goods",
        ));
    }
    rules
        .goods
        .iter()
        .zip(stock)
        .try_fold(u128::from(hearth), |total, (good, amount)| {
            total
                .checked_add(good.category_supplied("fuel", u128::from(*amount)))
                .ok_or_else(|| missing_data("fuel.holding".to_owned(), "holding overflow"))
        })
}

pub(super) fn debit_comfort_first(rules: &Rules, pop: &mut PopState, good: usize, units: u64) {
    let heat = rules.goods[good].category_supplied("fuel", u128::from(units));
    let debit = heat.min(u128::from(pop.consumption.comfort_milli));
    pop.consumption.comfort_milli -= u64::try_from(debit).expect("bounded by comfort balance");
}

pub(super) fn fuel_spare(
    rules: &Rules,
    stock: &[u64],
    hearth: u64,
    target: u128,
    prices: &[u64],
) -> Result<Vec<u64>, DataError> {
    let mut holding = fuel_holding(rules, stock, hearth)?;
    let mut spare = vec![0; rules.goods.len()];
    for index in fuel_ranking(rules, prices, true)? {
        let supply = rules.goods[index].category_supplied("fuel", 1);
        let available = holding.saturating_sub(target) / supply;
        let units = u128::from(stock[index]).min(available);
        spare[index] = u64::try_from(units)
            .map_err(|_| missing_data("fuel.spare".to_owned(), "spare exceeds u64"))?;
        holding -= units * supply;
    }
    Ok(spare)
}

#[cfg(test)]
pub(super) fn fuel_daily_need(rules: &Rules, pop: &PopState) -> Result<u128, DataError> {
    fuel_daily_need_at(rules, pop, 0)
}

fn fuel_daily_need_at(rules: &Rules, pop: &PopState, degree_hours: u64) -> Result<u128, DataError> {
    let people = u128::from(pop.workers) + u128::from(pop.dependents);
    let kind = rules
        .pop_types
        .iter()
        .find(|kind| kind.id == pop.kind)
        .ok_or_else(|| missing_data(format!("pop_types.{}", pop.kind), "definition is missing"))?;
    seasonal_need(
        people,
        u128::from(kind.fuel_need_milli_day),
        u128::from(kind.fuel_heat_micro_per_degree_day),
        u128::from(degree_hours),
    )
}

// Degree-hours are in tenths; heat is in millionths per degree-day.
fn seasonal_need(
    people: u128,
    flat: u128,
    heat: u128,
    degree_hours: u128,
) -> Result<u128, DataError> {
    let overflow = || missing_data("fuel.need".to_owned(), "daily need overflow");
    let cooking = people.checked_mul(flat).ok_or_else(overflow)?;
    let heating = people
        .checked_mul(heat)
        .and_then(|v| v.checked_mul(degree_hours))
        .ok_or_else(overflow)?
        / 240_000;
    cooking.checked_add(heating).ok_or_else(overflow)
}

pub(super) fn fuel_year_need(
    rules: &Rules,
    pop: &PopState,
    days: u64,
    degree_hours: u64,
) -> Result<u128, DataError> {
    let kind = rules
        .pop_types
        .iter()
        .find(|kind| kind.id == pop.kind)
        .ok_or_else(|| missing_data(format!("pop_types.{}", pop.kind), "definition is missing"))?;
    let overflow = || missing_data("fuel.year".to_owned(), "year target overflow");
    let flat = u128::from(kind.fuel_need_milli_day)
        .checked_mul(u128::from(days))
        .ok_or_else(overflow)?;
    let heat = u128::from(kind.fuel_heat_micro_per_degree_day)
        .checked_mul(u128::from(degree_hours))
        .ok_or_else(overflow)?
        / 240_000;
    let people = u128::from(pop.workers) + u128::from(pop.dependents);
    people
        .checked_mul(flat.checked_add(heat).ok_or_else(overflow)?)
        .ok_or_else(overflow)
}

pub(super) fn fuel_year_target(
    rules: &Rules,
    place: &PlaceState,
    pop: &PopState,
) -> Result<u128, DataError> {
    let overflow = || missing_data(format!("places.{}.fuel", place.id), "year record overflow");
    if place.dh_months.len() != 12 || place.dh_month_days.len() != 12 {
        return Err(missing_data(
            format!("places.{}.fuel", place.id),
            "expected twelve months",
        ));
    }
    let degree_hours = place
        .dh_months
        .iter()
        .try_fold(0_u64, |sum, value| sum.checked_add(*value))
        .ok_or_else(overflow)?;
    let days = place
        .dh_month_days
        .iter()
        .map(|days| u64::from(*days))
        .sum();
    fuel_year_need(rules, pop, days, degree_hours)
}

pub(super) fn comfort_target(
    rules: &Rules,
    place: &PlaceState,
    pop: &PopState,
) -> Result<u128, DataError> {
    let kind = rules
        .pop_types
        .iter()
        .find(|kind| kind.id == pop.kind)
        .ok_or_else(|| missing_data(format!("pop_types.{}", pop.kind), "definition is missing"))?;
    let overflow = || {
        missing_data(
            format!("places.{}.fuel.comfort", place.id),
            "comfort target overflow",
        )
    };
    let dh = place
        .dh_months
        .iter()
        .try_fold(0_u128, |sum, v| sum.checked_add(u128::from(*v)))
        .ok_or_else(overflow)?;
    (u128::from(pop.workers) + u128::from(pop.dependents))
        .checked_mul(u128::from(kind.fuel_heat_micro_per_degree_day))
        .and_then(|v| v.checked_mul(dh))
        .and_then(|v| v.checked_mul(u128::from(rules.comfort_heat_pm)))
        .map(|v| v / 240_000_000)
        .ok_or_else(overflow)
}

pub(super) fn assign_comfort(rules: &Rules, place: &mut PlaceState) -> Result<(), DataError> {
    for index in 0..place.pops.len() {
        let pop = &place.pops[index];
        let holding = fuel_holding(rules, &pop.stock, pop.consumption.hearth_milli)?;
        let survival = fuel_year_target(rules, place, pop)?;
        let comfort = comfort_target(rules, place, pop)?;
        let assigned = u128::from(pop.consumption.comfort_milli)
            .min(holding)
            .min(comfort);
        let surplus = holding.saturating_sub(assigned).saturating_sub(survival);
        let new = assigned + surplus.min(comfort - assigned);
        place.pops[index].consumption.comfort_milli = u64::try_from(new).map_err(|_| {
            missing_data(
                format!("places.{}.fuel.comfort", place.id),
                "set-aside exceeds u64",
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn draw_and_burn(
    rules: &Rules,
    place: &mut PlaceState,
    pop_index: usize,
    prices: &[u64],
    ledger: &mut DailyConsumptionLedger,
) -> Result<(), DataError> {
    let (owners, mut allowances) = super::daily_ownership(place, rules.goods.len())?;
    draw_and_burn_with_owners(
        rules,
        place,
        pop_index,
        prices,
        ledger,
        &owners,
        &mut allowances,
        0,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "The daily draw shares ownership, allowances, prices, ledger and weather."
)]
pub(super) fn draw_and_burn_with_owners(
    rules: &Rules,
    place: &mut PlaceState,
    pop_index: usize,
    prices: &[u64],
    ledger: &mut DailyConsumptionLedger,
    owners: &[Vec<u64>],
    allowances: &mut [Vec<u64>],
    degree_hours: u64,
) -> Result<(), DataError> {
    let need = fuel_daily_need_at(rules, &place.pops[pop_index], degree_hours)?;
    let target = fuel_year_target(rules, place, &place.pops[pop_index])?
        .checked_add(comfort_target(rules, place, &place.pops[pop_index])?)
        .ok_or_else(|| missing_data("fuel.draw".to_owned(), "target overflow"))?;
    let ranking = fuel_ranking(rules, prices, false)?;
    let mut holding = fuel_holding(
        rules,
        &place.pops[pop_index].stock,
        place.pops[pop_index].consumption.hearth_milli,
    )?;
    for (building_index, owner_shares) in owners.iter().enumerate().take(place.buildings.len()) {
        if holding >= target {
            break;
        }
        if owner_shares[pop_index] == 0 {
            continue;
        }
        for &index in &ranking {
            if holding >= target {
                break;
            }
            let supply = rules.goods[index].category_supplied("fuel", 1);
            let is_storehouse = place.buildings[building_index]
                .storehouse_accounts
                .is_some();
            let available = if is_storehouse {
                place.buildings[building_index]
                    .storehouse_accounts
                    .as_ref()
                    .expect("storehouse")
                    .communal[index]
                    .min(allowances[pop_index][index])
            } else {
                place.buildings[building_index].stock[index]
            };
            let wanted = (target - holding)
                .div_ceil(supply)
                .min(u128::from(available));
            let offered = u64::try_from(wanted)
                .map_err(|_| missing_data("fuel.draw".to_owned(), "draw exceeds u64"))?;
            let rejected = receive_pop(rules, &mut place.pops[pop_index], index, offered)?;
            let moved = offered - rejected;
            if is_storehouse {
                debit_storehouse(&mut place.buildings[building_index], None, index, moved)?;
                allowances[pop_index][index] -= moved;
            } else {
                place.buildings[building_index].stock[index] -= moved;
            }
            holding += u128::from(moved) * supply;
        }
    }
    assign_comfort(rules, place)?;
    let pop = &mut place.pops[pop_index];
    let mut balance = u128::from(pop.consumption.hearth_milli);
    for &index in &ranking {
        if balance >= need {
            break;
        }
        let supply = rules.goods[index].category_supplied("fuel", 1);
        let opened = (need - balance)
            .div_ceil(supply)
            .min(u128::from(pop.stock[index]));
        let opened = u64::try_from(opened)
            .map_err(|_| missing_data("fuel.burn".to_owned(), "opened exceeds u64"))?;
        pop.stock[index] -= opened;
        ledger.opened[index] = ledger.opened[index]
            .checked_add(opened)
            .ok_or_else(|| missing_data("fuel.burn".to_owned(), "ledger overflow"))?;
        balance = balance
            .checked_add(u128::from(opened) * supply)
            .ok_or_else(|| missing_data("fuel.burn".to_owned(), "balance overflow"))?;
    }
    let burned = balance.min(need);
    pop.consumption.hearth_milli = u64::try_from(balance - burned)
        .map_err(|_| missing_data("fuel.hearth".to_owned(), "balance exceeds u64"))?;
    let need = u64::try_from(need)
        .map_err(|_| missing_data("fuel.need".to_owned(), "need exceeds u64"))?;
    let burned = u64::try_from(burned)
        .map_err(|_| missing_data("fuel.burn".to_owned(), "burned exceeds u64"))?;
    pop.consumption.fuel_need_month = pop
        .consumption
        .fuel_need_month
        .checked_add(need)
        .ok_or_else(|| missing_data("fuel.need_month".to_owned(), "total overflow"))?;
    pop.consumption.fuel_burned_month = pop
        .consumption
        .fuel_burned_month
        .checked_add(burned)
        .ok_or_else(|| missing_data("fuel.burned_month".to_owned(), "total overflow"))?;
    burn_comfort(rules, pop, &ranking, ledger, degree_hours)
}

fn burn_comfort(
    rules: &Rules,
    pop: &mut PopState,
    ranking: &[usize],
    ledger: &mut DailyConsumptionLedger,
    degree_hours: u64,
) -> Result<(), DataError> {
    let holding = fuel_holding(rules, &pop.stock, pop.consumption.hearth_milli)?;
    pop.consumption.comfort_milli =
        u64::try_from(u128::from(pop.consumption.comfort_milli).min(holding))
            .map_err(|_| missing_data("fuel.comfort".to_owned(), "set-aside overflow"))?;
    let kind = rules
        .pop_types
        .iter()
        .find(|kind| kind.id == pop.kind)
        .ok_or_else(|| missing_data(format!("pop_types.{}", pop.kind), "definition is missing"))?;
    let people = u128::from(pop.workers) + u128::from(pop.dependents);
    let overflow = || missing_data("fuel.comfort".to_owned(), "comfort arithmetic overflow");
    let allowance = people
        .checked_mul(u128::from(kind.fuel_heat_micro_per_degree_day))
        .and_then(|v| v.checked_mul(u128::from(degree_hours)))
        .and_then(|v| v.checked_mul(u128::from(rules.comfort_heat_pm)))
        .ok_or_else(overflow)?
        / 240_000_000;
    let comfort_burn = allowance.min(u128::from(pop.consumption.comfort_milli));
    let mut balance = u128::from(pop.consumption.hearth_milli);
    for &index in ranking {
        if balance >= comfort_burn {
            break;
        }
        let supply = rules.goods[index].category_supplied("fuel", 1);
        let opened = (comfort_burn - balance)
            .div_ceil(supply)
            .min(u128::from(pop.stock[index]));
        let opened = u64::try_from(opened).map_err(|_| overflow())?;
        pop.stock[index] -= opened;
        ledger.opened[index] = ledger.opened[index]
            .checked_add(opened)
            .ok_or_else(overflow)?;
        balance = balance
            .checked_add(u128::from(opened) * supply)
            .ok_or_else(overflow)?;
    }
    pop.consumption.hearth_milli = u64::try_from(balance - comfort_burn).map_err(|_| overflow())?;
    pop.consumption.comfort_milli -= u64::try_from(comfort_burn).map_err(|_| overflow())?;
    if allowance != 0 {
        let niceties = 100_u128
            .checked_mul(people)
            .and_then(|v| v.checked_mul(u128::from(kind.fuel_comfort_niceties_pm)))
            .and_then(|v| v.checked_mul(comfort_burn))
            .ok_or_else(overflow)?
            / allowance.checked_mul(1000).ok_or_else(overflow)?;
        let niceties = u64::try_from(niceties).map_err(|_| overflow())?;
        pop.consumption.niceties_centi = pop
            .consumption
            .niceties_centi
            .checked_add(niceties)
            .ok_or_else(overflow)?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn seed_starting_fuel(
    rules: &Rules,
    pops: &mut [PopState],
    buildings: &[BuildingState],
) -> Result<(), DataError> {
    seed_starting_fuel_for_year(rules, pops, buildings, 365, 0)
}

pub(super) fn seed_starting_fuel_for_year(
    rules: &Rules,
    pops: &mut [PopState],
    buildings: &[BuildingState],
    days: u64,
    degree_hours: u64,
) -> Result<(), DataError> {
    if let Some(farm) = buildings.iter().find(|building| building.kind == "farm") {
        let definition = rules
            .buildings
            .iter()
            .find(|building| building.id == "farm")
            .expect("farm exists");
        let count = |id: &str| {
            definition
                .methods
                .iter()
                .position(|method| method.id == id)
                .and_then(|index| farm.method_levels.get(index))
                .copied()
                .unwrap_or(0)
        };
        let (wood, turf) = (count("wood_focus"), count("turf_cutting"));
        if wood > 0 || turf > 0 {
            let id = if wood >= turf { "hardwood" } else { "peat" };
            let index = rules
                .goods
                .iter()
                .position(|good| good.id == id)
                .ok_or_else(|| missing_data(format!("goods.{id}"), "starting fuel missing"))?;
            let supply = rules.goods[index].category_supplied("fuel", 1);
            if supply == 0 {
                return Err(missing_data(
                    format!("goods.{id}"),
                    "starting fuel has no fuel supply",
                ));
            }
            for pop in pops {
                let target = fuel_year_need(rules, pop, days, degree_hours)?;
                let units = u64::try_from(target.div_ceil(supply))
                    .map_err(|_| missing_data("fuel.start".to_owned(), "stock exceeds u64"))?;
                receive_pop(rules, pop, index, units)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod fuel_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn seasonal_need_counts_flat_and_degree_hours_with_exact_rounding() {
        assert_eq!(seasonal_need(3, 20, 9_400, 0).unwrap(), 60);
        assert_eq!(seasonal_need(3, 20, 9_400, 240).unwrap(), 88);
        assert_eq!(seasonal_need(0, 20, 9_400, 240).unwrap(), 0);
        assert!(seasonal_need(u128::MAX, 20, 9_400, 240).is_err());
    }

    #[test]
    fn fuel_order_and_spare_use_price_per_fuel_unit_and_skip_unspareable_loads() {
        let rules = Rules::load(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
        )
        .unwrap();
        let hardwood = rules
            .goods
            .iter()
            .position(|good| good.id == "hardwood")
            .unwrap();
        let peat = rules
            .goods
            .iter()
            .position(|good| good.id == "peat")
            .unwrap();
        let mut prices: Vec<_> = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        assert_eq!(fuel_ranking(&rules, &prices, false).unwrap()[0], peat);
        prices[peat] = 8000;
        assert_eq!(fuel_ranking(&rules, &prices, false).unwrap()[0], hardwood);
        let mut stock = vec![0; rules.goods.len()];
        stock[hardwood] = 5;
        assert_eq!(fuel_holding(&rules, &stock, 0).unwrap(), 5000);
        assert_eq!(
            fuel_spare(&rules, &stock, 0, 4800, &prices).unwrap()[hardwood],
            0
        );
        stock[hardwood] = 6;
        assert_eq!(
            fuel_spare(&rules, &stock, 0, 4800, &prices).unwrap()[hardwood],
            1
        );
        stock[hardwood] = 5;
        stock[peat] = 1;
        prices[peat] = 6000;
        let spare = fuel_spare(&rules, &stock, 0, 4800, &prices).unwrap();
        assert_eq!((spare[hardwood], spare[peat]), (0, 1));
    }
}
