//! Fuel accounting and ordering for the daily economy.
use super::{
    BuildingState, DailyConsumptionLedger, DataError, PlaceState, PopState, Rules, missing_data,
    receive_pop,
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

pub(super) fn fuel_daily_need(rules: &Rules, pop: &PopState) -> Result<u128, DataError> {
    let people = u128::from(pop.workers) + u128::from(pop.dependents);
    let rate = rules
        .pop_types
        .iter()
        .find(|kind| kind.id == pop.kind)
        .ok_or_else(|| missing_data(format!("pop_types.{}", pop.kind), "definition is missing"))?
        .fuel_need_milli_day;
    people
        .checked_mul(u128::from(rate))
        .ok_or_else(|| missing_data("fuel.need".to_owned(), "daily need overflow"))
}

pub(super) fn draw_and_burn(
    rules: &Rules,
    place: &mut PlaceState,
    pop_index: usize,
    prices: &[u64],
    ledger: &mut DailyConsumptionLedger,
) -> Result<(), DataError> {
    let need = fuel_daily_need(rules, &place.pops[pop_index])?;
    let target = need
        .checked_mul(60)
        .ok_or_else(|| missing_data("fuel.draw".to_owned(), "target overflow"))?;
    let ranking = fuel_ranking(rules, prices, false)?;
    let mut holding = fuel_holding(
        rules,
        &place.pops[pop_index].stock,
        place.pops[pop_index].consumption.hearth_milli,
    )?;
    for building_index in 0..place.buildings.len() {
        if holding >= target {
            break;
        }
        if place.buildings[building_index]
            .employment
            .get(pop_index)
            .is_none_or(|job| job.owners == 0)
        {
            continue;
        }
        for &index in &ranking {
            if holding >= target {
                break;
            }
            let supply = rules.goods[index].category_supplied("fuel", 1);
            let wanted = (target - holding)
                .div_ceil(supply)
                .min(u128::from(place.buildings[building_index].stock[index]));
            let offered = u64::try_from(wanted)
                .map_err(|_| missing_data("fuel.draw".to_owned(), "draw exceeds u64"))?;
            let rejected = receive_pop(rules, &mut place.pops[pop_index], index, offered)?;
            let moved = offered - rejected;
            place.buildings[building_index].stock[index] -= moved;
            holding += u128::from(moved) * supply;
        }
    }
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
    Ok(())
}

pub(super) fn seed_starting_fuel(
    rules: &Rules,
    pops: &mut [PopState],
    buildings: &[BuildingState],
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
                let target = fuel_daily_need(rules, pop)?
                    .checked_mul(240)
                    .ok_or_else(|| missing_data("fuel.start".to_owned(), "target overflow"))?;
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
