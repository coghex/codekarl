//! Budgeted market wants, in priority order.

use crate::{
    DataError, PlaceState, Rules, food_target_units, fuel, herd_wants, live_species, missing_data,
    mul_div_floor, pop_market_holding, pop_target_units, swap,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Destination {
    Household(usize),
    Fuel,
    Herd {
        building: usize,
        species: usize,
        good: usize,
    },
    Job {
        job: usize,
        good: usize,
        species: Option<usize>,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Want {
    pub(super) tier: usize,
    pub(super) units: u128,
    pub(super) value: u128,
    pub(super) funded: u128,
    pub(super) funded_value: u128,
    pub(super) destination: Destination,
}

fn fund(rows: &mut [Want], mut remaining: u128) -> Result<(), DataError> {
    for tier in 0..6 {
        let value = rows
            .iter()
            .filter(|row| row.tier == tier)
            .try_fold(0_u128, |sum, row| {
                sum.checked_add(row.value)
                    .ok_or_else(|| missing_data("market.budget".into(), "tier value overflow"))
            })?;
        if value == 0 {
            continue;
        }
        if remaining >= value {
            for row in rows.iter_mut().filter(|row| row.tier == tier) {
                row.funded = row.units;
                row.funded_value = row.value;
            }
            remaining -= value;
        } else {
            for row in rows.iter_mut().filter(|row| row.tier == tier) {
                row.funded_value = mul_div_floor(row.value, remaining, value)
                    .ok_or_else(|| missing_data("market.budget".into(), "funding overflow"))?;
                row.funded = mul_div_floor(row.units, remaining, value)
                    .ok_or_else(|| missing_data("market.budget".into(), "funding overflow"))?;
            }
            break;
        }
    }
    Ok(())
}

/// Whole fuel loads bounded by both funded heat and funded base value.
pub(super) fn fuel_loads(heat: u128, value: u128, supply: u128, base: u64) -> u128 {
    if supply == 0 || base == 0 {
        return 0;
    }
    (heat / supply).min(value / (1000 * u128::from(base)))
}

pub(super) fn price_fuel_order(
    rules: &Rules,
    prices: &[u64],
    heat: u128,
    value: u128,
) -> Result<Option<(usize, u128)>, DataError> {
    for good in fuel::fuel_ranking(rules, prices, false)? {
        let definition = &rules.goods[good];
        let loads = fuel_loads(
            heat,
            value,
            definition.category_supplied("fuel", 1),
            definition.base_price,
        );
        if loads > 0 {
            return Ok(Some((good, loads)));
        }
    }
    Ok(None)
}

/// A receipt is planned against the funded rows and destination room before either party pays.
#[derive(Debug)]
pub(super) struct Route {
    pub(super) rows: Vec<Want>,
    pub(super) deliveries: Vec<(Destination, u128)>,
    pub(super) units: u128,
}

#[expect(
    clippy::too_many_arguments,
    reason = "The route fixes the good, its fuel value, room and payment destination together."
)]
pub(super) fn route<F>(
    rows: &[Want],
    good: usize,
    supply: u128,
    base: u64,
    limit: u128,
    household_room: u128,
    building_payment: bool,
    mut room: F,
) -> Result<Route, DataError>
where
    F: FnMut(Destination) -> u128,
{
    let mut rows = rows.to_vec();
    let mut deliveries = Vec::new();
    let mut left = limit;
    let mut house = household_room;
    let mut fuel_done = false;
    for tier in 0..6 {
        for index in 0..rows.len() {
            if left == 0 {
                break;
            }
            let destination = rows[index].destination;
            if rows[index].tier != tier {
                continue;
            }
            if destination == Destination::Fuel {
                if fuel_done || supply == 0 {
                    continue;
                }
                fuel_done = true;
                let mut heat = 0_u128;
                let mut value = 0_u128;
                for row in rows
                    .iter()
                    .filter(|row| row.destination == Destination::Fuel)
                {
                    heat = heat.checked_add(row.funded).ok_or_else(overflow)?;
                    value = value.checked_add(row.funded_value).ok_or_else(overflow)?;
                }
                let take = fuel_loads(heat, value, supply, base).min(left).min(house);
                if take == 0 {
                    continue;
                }
                let mut heat_spent = take.checked_mul(supply).ok_or_else(overflow)?;
                let mut value_spent = take
                    .checked_mul(u128::from(base))
                    .and_then(|v| v.checked_mul(1000))
                    .ok_or_else(overflow)?;
                for row in rows
                    .iter_mut()
                    .filter(|row| row.destination == Destination::Fuel)
                {
                    let h = row.funded.min(heat_spent);
                    row.funded -= h;
                    heat_spent -= h;
                    let v = row.funded_value.min(value_spent);
                    row.funded_value -= v;
                    value_spent -= v;
                }
                deliveries.push((Destination::Fuel, take));
                left -= take;
                house -= take;
                continue;
            }
            let matching = match destination {
                Destination::Household(g) => g == good,
                Destination::Herd { good: g, .. } | Destination::Job { good: g, .. } => {
                    g == good && !building_payment
                }
                Destination::Fuel => false,
            };
            if !matching {
                continue;
            }
            let cap = if matches!(destination, Destination::Household(_)) {
                house
            } else {
                room(destination)
            };
            let take = rows[index].funded.min(left).min(cap);
            if take == 0 {
                continue;
            }
            rows[index].funded -= take;
            if matches!(destination, Destination::Household(_)) {
                house -= take;
            }
            left -= take;
            deliveries.push((destination, take));
        }
    }
    Ok(Route {
        rows,
        deliveries,
        units: limit - left,
    })
}

fn overflow() -> DataError {
    missing_data("market.budget".into(), "value overflow")
}

fn add_value(total: &mut u128, units: u128, base: u64) -> Result<(), DataError> {
    let value = units
        .checked_mul(u128::from(base))
        .and_then(|v| v.checked_mul(1000))
        .ok_or_else(overflow)?;
    *total = total.checked_add(value).ok_or_else(overflow)?;
    Ok(())
}

fn push_good(
    rows: &mut Vec<Want>,
    tier: usize,
    units: u128,
    good: usize,
    destination: Destination,
    rules: &Rules,
) -> Result<(), DataError> {
    if units == 0 {
        return Ok(());
    }
    let mut value = 0;
    add_value(&mut value, units, rules.goods[good].base_price)?;
    rows.push(Want {
        tier,
        units,
        value,
        funded: 0,
        funded_value: 0,
        destination,
    });
    Ok(())
}

fn surplus_value(
    rules: &Rules,
    place: &PlaceState,
    pop_index: usize,
    prices: &[u64],
    parts: &[u128],
) -> Result<u128, DataError> {
    let pop = &place.pops[pop_index];
    let people = pop
        .workers
        .checked_add(pop.dependents)
        .ok_or_else(overflow)?;
    let target_heat = fuel::fuel_daily_need(rules, pop)?
        .checked_mul(240)
        .ok_or_else(overflow)?;
    let fuel_spare = fuel::fuel_spare(
        rules,
        &pop.stock,
        pop.consumption.hearth_milli,
        target_heat,
        prices,
    )?;
    let live = live_species(rules);
    let mut value = 0_u128;
    for (good, item) in rules.goods.iter().enumerate() {
        if live[good].is_none() {
            add_value(&mut value, parts[good], item.base_price)?;
        }
        if item.category_supplied("fuel", 1) > 0 {
            add_value(&mut value, u128::from(fuel_spare[good]), item.base_price)?;
        } else if live[good].is_none() {
            let held = pop_market_holding(place, pop_index, good)?;
            let target = pop_target_units(rules, &pop.kind, people, good)?;
            add_value(&mut value, held.saturating_sub(target), item.base_price)?;
        }
    }
    Ok(value)
}

/// Fixed funded rows at either the opening price snapshot or the start of exchange.
#[expect(
    clippy::too_many_lines,
    reason = "One snapshot keeps the shared budget and its six destination tiers together."
)]
pub(super) fn orders(
    rules: &Rules,
    place: &PlaceState,
    prices: &[u64],
) -> Result<Vec<Vec<Want>>, DataError> {
    let live = live_species(rules);
    let herds = herd_wants(rules, place)?;
    let building_parts = swap::BuildingParts::new_with(rules, place, prices, 0)?;
    let mut parts = vec![vec![0_u128; rules.goods.len()]; place.pops.len()];
    for building in &building_parts.spare {
        for (pop, shares) in building.iter().enumerate() {
            for (good, share) in shares.iter().enumerate() {
                if live[good].is_none() {
                    parts[pop][good] = parts[pop][good].checked_add(*share).ok_or_else(overflow)?;
                }
            }
        }
    }
    let reference = rules
        .goods
        .iter()
        .enumerate()
        .filter_map(|(i, good)| {
            let supply = good.category_supplied("fuel", 1);
            (supply > 0).then_some((i, supply))
        })
        .min_by(|(a, sa), (b, sb)| {
            (u128::from(rules.goods[*a].base_price) * *sb)
                .cmp(&(u128::from(rules.goods[*b].base_price) * *sa))
        });
    let mut result = Vec::with_capacity(place.pops.len());
    for (pop_index, pop) in place.pops.iter().enumerate() {
        let people = pop
            .workers
            .checked_add(pop.dependents)
            .ok_or_else(overflow)?;
        let heat_target = fuel::fuel_daily_need(rules, pop)?
            .checked_mul(240)
            .ok_or_else(overflow)?;
        let heat = fuel::fuel_holding(rules, &pop.stock, pop.consumption.hearth_milli)?;
        let budget = surplus_value(rules, place, pop_index, prices, &parts[pop_index])?;
        let mut rows = Vec::new();
        for (g, good) in rules.goods.iter().enumerate() {
            if good.category_supplied("fuel", 1) > 0 {
                continue;
            }
            if live[g].is_some() {
                continue;
            }
            let held = pop_market_holding(place, pop_index, g)?;
            let necessity = food_target_units(rules, &pop.kind, people, g, 240)?;
            let target = pop_target_units(rules, &pop.kind, people, g)?;
            let near_n = mul_div_floor(necessity, u128::from(rules.food_near_pm), 1000)
                .ok_or_else(overflow)?
                .saturating_sub(held);
            let need = necessity.saturating_sub(held);
            let near_all = mul_div_floor(target, u128::from(rules.comfort_near_pm), 1000)
                .ok_or_else(overflow)?
                .saturating_sub(held);
            let total = target.saturating_sub(held);
            let near_comfort = near_all.saturating_sub(need);
            for (tier, units) in [
                (0, near_n),
                (1, need.saturating_sub(near_n)),
                (4, near_comfort),
                (5, total.saturating_sub(need).saturating_sub(near_comfort)),
            ] {
                push_good(&mut rows, tier, units, g, Destination::Household(g), rules)?;
            }
        }
        if let Some((reference_good, supply)) = reference {
            let near = mul_div_floor(heat_target, u128::from(rules.fuel_near_pm), 1000)
                .ok_or_else(overflow)?
                .saturating_sub(heat);
            let short = heat_target.saturating_sub(heat);
            for (tier, units) in [(0, near), (1, short.saturating_sub(near))] {
                if units == 0 {
                    continue;
                }
                let numerator = units
                    .checked_mul(u128::from(rules.goods[reference_good].base_price))
                    .and_then(|v| v.checked_mul(1000))
                    .ok_or_else(overflow)?;
                let value = numerator.div_ceil(supply);
                rows.push(Want {
                    tier,
                    units,
                    value,
                    funded: 0,
                    funded_value: 0,
                    destination: Destination::Fuel,
                });
            }
        }
        for (good, species) in live
            .iter()
            .enumerate()
            .filter_map(|(g, s)| s.map(|s| (g, s)))
        {
            for (building, wants) in herds.iter().enumerate() {
                push_good(
                    &mut rows,
                    2,
                    u128::from(wants[pop_index][species]),
                    good,
                    Destination::Herd {
                        building,
                        species,
                        good,
                    },
                    rules,
                )?;
            }
        }
        for (job_index, job) in place
            .trade
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, job)| job.pop as usize == pop_index)
        {
            for (good, species) in live.iter().copied().enumerate() {
                let lack = species.map_or_else(|| job.lack(good), |s| job.animal_lack(s));
                push_good(
                    &mut rows,
                    3,
                    u128::from(lack),
                    good,
                    Destination::Job {
                        job: job_index,
                        good,
                        species,
                    },
                    rules,
                )?;
            }
        }
        fund(&mut rows, budget)?;
        result.push(rows);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_skips_a_full_household_and_uses_only_a_funded_job() {
        let job = Destination::Job {
            job: 0,
            good: 0,
            species: None,
        };
        let rows = [
            Want {
                tier: 0,
                units: 4,
                value: 4000,
                funded: 4,
                funded_value: 4000,
                destination: Destination::Household(0),
            },
            Want {
                tier: 3,
                units: 4,
                value: 4000,
                funded: 2,
                funded_value: 2000,
                destination: job,
            },
        ];
        let full = route(&rows, 0, 0, 1, 10, 0, false, |_| 4).unwrap();
        assert_eq!(full.units, 2);
        assert_eq!(full.deliveries, [(job, 2)]);
        let partial = route(&rows, 0, 0, 1, 10, 1, false, |_| 4).unwrap();
        assert_eq!(partial.units, 3);
        assert_eq!(
            partial.deliveries,
            [(Destination::Household(0), 1), (job, 2)]
        );
        assert_eq!(rows[0].funded, 4, "planning does not spend the source rows");
        assert_eq!(partial.rows[0].funded, 3);
    }

    #[test]
    fn fuel_load_spends_heat_and_value_across_survival_tiers_before_a_job() {
        let job = Destination::Job {
            job: 0,
            good: 0,
            species: None,
        };
        let rows = [
            Want {
                tier: 0,
                units: 300,
                value: 3000,
                funded: 300,
                funded_value: 3000,
                destination: Destination::Fuel,
            },
            Want {
                tier: 1,
                units: 400,
                value: 3000,
                funded: 400,
                funded_value: 3000,
                destination: Destination::Fuel,
            },
            Want {
                tier: 3,
                units: 1,
                value: 6000,
                funded: 1,
                funded_value: 6000,
                destination: job,
            },
        ];
        let plan = route(&rows, 0, 700, 6, 1, 10, false, |_| 1).unwrap();
        assert_eq!(plan.deliveries, [(Destination::Fuel, 1)]);
        assert_eq!(plan.rows[0].funded + plan.rows[1].funded, 0);
        assert_eq!(plan.rows[2].funded, 1);
    }

    #[test]
    fn herd_rows_precede_job_rows_and_a_sale_deposit_excludes_both() {
        let herd = Destination::Herd {
            building: 0,
            species: 0,
            good: 1,
        };
        let job = Destination::Job {
            job: 0,
            good: 1,
            species: Some(0),
        };
        let rows = [
            Want {
                tier: 2,
                units: 1,
                value: 1000,
                funded: 1,
                funded_value: 1000,
                destination: herd,
            },
            Want {
                tier: 3,
                units: 1,
                value: 1000,
                funded: 1,
                funded_value: 1000,
                destination: job,
            },
        ];
        let plan = route(&rows, 1, 0, 1, 1, 10, false, |_| 1).unwrap();
        assert_eq!(plan.deliveries, [(herd, 1)]);
        assert_eq!(plan.rows[1].funded, 1);
        assert_eq!(route(&rows, 1, 0, 1, 2, 10, true, |_| 1).unwrap().units, 0);
    }

    #[test]
    fn target_kinds_read_their_own_near_term_shares() {
        use crate::swap_tests::{good, herd_rules, place};
        let mut rules = herd_rules();
        let place = place(&rules, 1);
        let oats = good(&rules, "oats");
        let herbs = good(&rules, "herbs");
        let defaults = orders(&rules, &place, &place.prices_milli).unwrap();
        assert!(
            defaults[0]
                .iter()
                .any(|row| row.tier == 0 && row.destination == Destination::Household(oats))
        );
        assert!(
            defaults[0]
                .iter()
                .any(|row| row.tier == 0 && row.destination == Destination::Fuel)
        );
        assert!(
            defaults[0]
                .iter()
                .any(|row| row.tier == 4 && row.destination == Destination::Household(herbs))
        );
        rules.food_near_pm = 0;
        rules.comfort_near_pm = 0;
        rules.fuel_near_pm = 0;
        let later = orders(&rules, &place, &place.prices_milli).unwrap();
        assert!(!later[0].iter().any(|row| row.tier == 0
            && (row.destination == Destination::Household(oats)
                || row.destination == Destination::Fuel)));
        assert!(
            !later[0]
                .iter()
                .any(|row| row.tier == 4 && row.destination == Destination::Household(herbs))
        );
        for which in 0..3 {
            let mut invalid = rules.clone();
            match which {
                0 => invalid.food_near_pm = 1001,
                1 => invalid.comfort_near_pm = 1001,
                _ => invalid.fuel_near_pm = 1001,
            }
            assert!(invalid.validate("fixture").is_err());
        }
    }

    #[test]
    fn budget_sums_two_goods_at_base_values() {
        use crate::swap_tests::{good, herd_rules, place, settle_the_rest, spare};
        let rules = herd_rules();
        let mut place = place(&rules, 1);
        settle_the_rest(&rules, &mut place, &[]);
        spare(&rules, &mut place, 0, "oats", 10);
        spare(&rules, &mut place, 0, "honey", 7);
        let oats = good(&rules, "oats");
        let honey = good(&rules, "honey");
        let parts = vec![0; rules.goods.len()];
        let amount = surplus_value(&rules, &place, 0, &place.prices_milli, &parts).unwrap();
        assert_eq!(
            amount,
            (10 * u128::from(rules.goods[oats].base_price)
                + 7 * u128::from(rules.goods[honey].base_price))
                * 1000
        );
    }

    #[test]
    fn budget_uses_the_exchange_parts_for_resolved_building_owners() {
        use crate::swap_tests::{add_building, good, herd_rules, own, place, settle_the_rest};
        let rules = herd_rules();
        let mut place = place(&rules, 2);
        settle_the_rest(&rules, &mut place, &[]);
        let building = add_building(&rules, &mut place, "farm", 1);
        own(&mut place, building, 0, 1);
        own(&mut place, building, 1, 3);
        let herbs = good(&rules, "herbs");
        place.buildings[building].stock[herbs] = 80;
        let split = swap::BuildingParts::new_with(&rules, &place, &place.prices_milli, 0).unwrap();
        let shares = [
            split.spare[building][0][herbs],
            split.spare[building][1][herbs],
        ];
        assert_eq!(shares, [20, 60]);
        for (pop, share) in shares.into_iter().enumerate() {
            let amount = surplus_value(
                &rules,
                &place,
                pop,
                &place.prices_milli,
                &split.spare[building][pop],
            )
            .unwrap();
            assert_eq!(
                amount,
                share * u128::from(rules.goods[herbs].base_price) * 1000
            );
        }
    }

    #[test]
    fn a_private_storehouse_account_funds_only_its_holder() {
        use crate::swap_tests::{add_building, good, herd_rules, place, settle_the_rest};
        let rules = herd_rules();
        let mut place = place(&rules, 2);
        settle_the_rest(&rules, &mut place, &[]);
        let house = add_building(&rules, &mut place, "storehouse", 1);
        let herbs = good(&rules, "herbs");
        let mut accounts = crate::empty_storehouse_accounts(&rules, 2);
        accounts.pops[1][herbs] = 30;
        place.buildings[house].stock[herbs] = 30;
        place.buildings[house].storehouse_accounts = Some(accounts);
        let parts = swap::BuildingParts::new_with(&rules, &place, &place.prices_milli, 0).unwrap();
        assert_eq!(parts.spare[house][0][herbs], 0);
        assert_eq!(parts.spare[house][1][herbs], 0);
        assert_eq!(
            surplus_value(
                &rules,
                &place,
                0,
                &place.prices_milli,
                &parts.spare[house][0]
            )
            .unwrap(),
            0
        );
        assert_eq!(
            surplus_value(
                &rules,
                &place,
                1,
                &place.prices_milli,
                &parts.spare[house][1]
            )
            .unwrap(),
            30 * u128::from(rules.goods[herbs].base_price) * 1000
        );
    }

    #[test]
    fn first_of_month_production_can_fund_the_exchange_but_not_opening_orders() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml");
        let mut rules = Rules::load(path).unwrap();
        crate::tests::flatten_pasture(&mut rules);
        let mut state = crate::tests::shipped_production_fixture(&rules);
        for pop in &mut state.places[0].pops {
            pop.stock.fill(0);
        }
        for building in &mut state.places[0].buildings {
            building.stock.fill(0);
            if let Some(accounts) = &mut building.storehouse_accounts {
                accounts.communal.fill(0);
                for account in &mut accounts.pops {
                    account.fill(0);
                }
            }
        }
        let prices = state.places[0].prices_milli.clone();
        let opening = orders(&rules, &state.places[0], &prices).unwrap();
        assert!(opening.iter().flatten().all(|row| row.funded_value == 0));
        crate::run_monthly_production(
            &rules,
            &mut state,
            crate::Date {
                year: 768,
                month: 3,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let exchange = orders(&rules, &state.places[0], &prices).unwrap();
        assert!(exchange.iter().flatten().any(|row| row.funded > 0));
    }

    #[test]
    fn waiting_job_changes_the_exchange_snapshot_after_taking_stock() {
        use crate::swap_tests::{good, herd_rules, place, settle_the_rest, spare};
        let rules = herd_rules();
        let mut place = place(&rules, 1);
        settle_the_rest(&rules, &mut place, &[]);

        spare(&rules, &mut place, 0, "oats", 10);
        let oats = good(&rules, "oats");
        let mut needs = vec![0; rules.goods.len()];
        needs[oats] = 4;
        place.trade.jobs.push(crate::Job {
            number: 1,
            pop: 0,
            kind: crate::JobKind::Build {
                vehicle: 0,
                trip: None,
            },
            made: 0,
            need_goods: needs,
            need_animals: vec![0; rules.herd_species.len()],
            stock: vec![0; rules.goods.len()],
            animals: vec![0; rules.herd_species.len()],
            hands: None,
            given_up: None,
        });
        let prices = place.prices_milli.clone();
        let before = orders(&rules, &place, &prices).unwrap();
        let job_amount = |rows: &[Want]| {
            rows.iter().filter(|row| matches!(row.destination, Destination::Job { job: 0, good, .. } if good == oats)).map(|row| row.funded).sum::<u128>()
        };
        assert_eq!(job_amount(&before[0]), 4);
        crate::trade::take_waiting(&rules, &mut place, &prices).unwrap();
        let after = orders(&rules, &place, &prices).unwrap();
        assert_eq!(job_amount(&after[0]), 0);
        assert_eq!(place.trade.jobs[0].stock[oats], 4);
        assert_eq!(
            pop_market_holding(&place, 0, oats).unwrap()
                - pop_target_units(&rules, &place.pops[0].kind, 1000, oats).unwrap(),
            6
        );
    }

    #[test]
    fn price_fuel_skips_a_cheaper_heat_good_when_its_base_value_is_unfunded() {
        let rules = crate::swap_tests::herd_rules();
        let wood = crate::swap_tests::good(&rules, "hardwood");
        let peat = crate::swap_tests::good(&rules, "peat");
        let mut prices: Vec<_> = rules.goods.iter().map(|g| g.base_price * 1000).collect();
        prices[wood] = 1000;
        prices[peat] = 1400;
        assert_eq!(
            price_fuel_order(&rules, &prices, 1050, 9000).unwrap(),
            Some((peat, 1))
        );
        assert_eq!(
            price_fuel_order(&rules, &prices, 1050, 10000).unwrap(),
            Some((wood, 1))
        );
    }

    #[test]
    fn fuel_loads_obey_both_heat_and_base_value() {
        assert_eq!(fuel_loads(1050, 9000, 1000, 10), 0);
        assert_eq!(fuel_loads(1050, 9000, 700, 6), 1);
        assert_eq!(fuel_loads(1050, 10000, 1000, 10), 1);
        assert_eq!(fuel_loads(699, 6000, 700, 6), 0);
    }

    #[test]
    fn no_spare_means_no_funded_orders() {
        let rules = crate::swap_tests::herd_rules();
        let mut place = crate::swap_tests::place(&rules, 1);
        place.pops[0].stock.fill(0);
        place.buildings.clear();
        let rows = orders(&rules, &place, &place.prices_milli).unwrap();
        assert!(rows[0].iter().any(|row| row.units > 0));
        assert!(rows[0].iter().all(|row| row.funded == 0));
        let market = crate::daily_market_orders(&rules, &place).unwrap();
        assert!(market.buy_units.iter().all(|units| *units == 0));
        let opening = place.prices_milli.clone();
        let mut state = crate::EconomyState {
            places: vec![place],
            ..crate::EconomyState::default()
        };
        crate::update_daily_prices(&rules, &mut state).unwrap();
        assert_eq!(state.places[0].prices_milli, opening);
    }

    #[test]
    fn a_real_pop_cannot_order_more_base_value_than_its_spare() {
        use crate::swap_tests::{good, herd_rules, place, spare};
        let rules = herd_rules();
        let mut place = place(&rules, 1);
        spare(&rules, &mut place, 0, "oats", 10);
        let oats = good(&rules, "oats");
        let budget = 10 * u128::from(rules.goods[oats].base_price) * 1000;
        let rows = orders(&rules, &place, &place.prices_milli).unwrap();
        let funded: u128 = rows[0].iter().map(|row| row.funded_value).sum();
        assert!(funded > 0);
        assert!(funded <= budget);
    }

    #[test]
    fn a_partial_tier_shares_the_remaining_budget_and_blocks_later_tiers() {
        let mut rows = [
            Want {
                tier: 0,
                units: 5,
                value: 10,
                funded: 0,
                funded_value: 0,
                destination: Destination::Household(0),
            },
            Want {
                tier: 0,
                units: 8,
                value: 16,
                funded: 0,
                funded_value: 0,
                destination: Destination::Household(0),
            },
            Want {
                tier: 1,
                units: 7,
                value: 7,
                funded: 0,
                funded_value: 0,
                destination: Destination::Household(0),
            },
        ];
        fund(&mut rows, 13).unwrap();
        assert_eq!(rows.map(|row| row.funded), [2, 4, 0]);
    }

    #[test]
    fn full_tiers_precede_the_partial_tier() {
        let mut rows = [
            Want {
                tier: 0,
                units: 4,
                value: 8,
                funded: 0,
                funded_value: 0,
                destination: Destination::Household(0),
            },
            Want {
                tier: 1,
                units: 3,
                value: 6,
                funded: 0,
                funded_value: 0,
                destination: Destination::Household(0),
            },
            Want {
                tier: 4,
                units: 10,
                value: 20,
                funded: 0,
                funded_value: 0,
                destination: Destination::Household(0),
            },
        ];
        fund(&mut rows, 24).unwrap();
        assert_eq!(rows.map(|row| row.funded), [4, 3, 5]);
    }
}
