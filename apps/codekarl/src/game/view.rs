//! The economy view (milestone 9, slice 3; design §6): an immutable, plain-data picture of every place's economy that the
//! simulation builds and the UI reads. The UI never reads the live state.
//!
//! A view is built from the state at three moments: at the start, after a load, and after each day's systems (the noon tick).
//! Each carries a revision that rises with every new view, including a load. Monthly births, deaths and overflow losses come
//! from the saved `last` counter buckets; the view does not advance the economy.
//!
//! Everything here is simulation-side code: it lives in `game/` so that the build id covers it (a change to the display rules
//! is a change to what a save means to the UI, and the numbers it derives are checked by tests).

#[cfg(test)]
use ck_economy::PopConsumption;
use ck_economy::{BuildingState, EconomyState, LandCounts, PlaceState, PopState, Rules};
use ck_sim::Date;
use std::collections::BTreeMap;

/// A pop's store, in millilitres per person (design §3.15: 720 L).
const POP_STORE_ML_PER_PERSON: u64 = 720_000;
/// A storehouse level's store, in millilitres: 50 people's worth of a pop store (design §3.18).
const STOREHOUSE_STORE_ML_PER_LEVEL: u64 = 50 * POP_STORE_ML_PER_PERSON;
/// One good's price at a place against its base price.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PriceView {
    /// The good's key.
    pub(crate) good: String,
    /// Open-vocabulary data tags from the good definition, in authored order.
    pub(crate) tags: Vec<String>,
    /// The local price in thousandths of a value unit.
    pub(crate) price_milli: u64,
    /// The base price in whole value units.
    pub(crate) base: u64,
    /// Units bought in the latest daily price step, absent before the first step.
    pub(crate) bought: Option<u128>,
    /// Units offered in the latest daily price step, absent before the first step.
    pub(crate) offered: Option<u128>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StockView {
    pub(crate) good: String,
    pub(crate) amount: u64,
    pub(crate) charged_ml: u64,
    pub(crate) outside: bool,
}

/// One pop, as the Pops tab shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PopView {
    pub(crate) stock: Vec<StockView>,
    pub(crate) room_ml: u64,
    pub(crate) kind: String,
    pub(crate) culture: String,
    pub(crate) religion: String,
    pub(crate) workers: u64,
    pub(crate) dependents: u64,
    /// Workers holding a job in a building.
    pub(crate) employed: u64,
    pub(crate) jobs: Vec<(String, String, u64)>,
    /// Workers with no place.
    pub(crate) unemployed: u64,
    /// Whole days of food held (the store and the storehouse accounts) at a full daily need.
    pub(crate) food_days: u64,
    /// Foods held by this pop, apportioned by their share of daily hunger.
    pub(crate) food: Vec<FoodView>,
    /// The three needs, in ‰.
    pub(crate) necessities: u64,
    pub(crate) niceties: u64,
    pub(crate) luxuries: u64,
    /// Satisfaction, −100 to +100.
    pub(crate) satisfaction: i64,
    /// The satisfaction the pop drifts toward on its present needs met (the economy's baseline for it; CK-378), −100 to +100.
    pub(crate) equilibrium: i64,
    /// Births and deaths in the month completed by the last counter rollover.
    pub(crate) births: u64,
    pub(crate) deaths: u64,
    /// People assigned to basic building housing by the pure housing fill.
    pub(crate) basic_housing: u64,
    /// People assigned to makeshift natural shelter by the pure housing fill.
    pub(crate) makeshift_housing: u64,
    /// Crowding in per-mille, absent for an empty pop.
    pub(crate) crowding_pm: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FoodView {
    pub(crate) good: String,
    pub(crate) units: u64,
    pub(crate) unit: String,
    pub(crate) share_pm: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WageView {
    pub(crate) month: u8,
    pub(crate) year: Option<u16>,
    pub(crate) wage_milli: u64,
    pub(crate) paid: Option<u64>,
}

/// One herd: a species' head count against its maximum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HerdView {
    pub(crate) species: String,
    pub(crate) head: u64,
    pub(crate) maximum: u64,
}

/// One building, as the Buildings tab shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BuildingView {
    pub(crate) stock: Vec<StockView>,
    pub(crate) made: Vec<(String, u64)>,
    pub(crate) kind: String,
    pub(crate) levels: u64,
    /// The levels of each method that has any, in method order.
    pub(crate) methods: Vec<(String, u64)>,
    /// Paid positions held, and positions open.
    pub(crate) filled: u64,
    pub(crate) open: u64,
    pub(crate) jobs: Vec<(String, u64, u64)>,
    pub(crate) abandoned: bool,
    pub(crate) herds: Vec<HerdView>,
    pub(crate) store_used_ml: u64,
    pub(crate) store_capacity_ml: u64,
    /// The mean of the building's last twelve monthly wages in thousandths.
    pub(crate) wage: u64,
    /// Twelve calendar-month payday records, newest first.
    pub(crate) wages: Vec<WageView>,
    /// Occupants assigned to this building by the pure housing fill.
    pub(crate) housing_occupants: u64,
}

/// One place's economy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlaceView {
    pub(crate) id: String,
    pub(crate) temperature: Option<(i64, u8, i64)>,
    pub(crate) wind: Option<(i64, String)>,
    pub(crate) workers: u64,
    pub(crate) dependents: u64,
    /// Whole days of food, the place's pops together.
    pub(crate) food_days: u64,
    pub(crate) storage_used_ml: u64,
    pub(crate) storage_capacity_ml: u64,
    pub(crate) prices: Vec<PriceView>,
    pub(crate) land: LandView,
    pub(crate) last_month: LastMonthView,
    /// Goods lost to overflow in the month completed by the last counter rollover.
    pub(crate) overflow_lost: u64,
    /// The projected annual food of the land, the sea and the dairy herd (design §3.22, the yield map mode): every food good a method
    /// makes from the land's suitability (crops, fish, the garden) over a full year at the levels the place has, and the cattle's
    /// dairy at 72 pounds a cow, in hundredths of a person-day. Full staffing and coverage are assumed, and the meats do not count.
    pub(crate) projected_yield: u64,
    /// Whether the place has an economy at all: pops or buildings. A place of nobody has nothing for a map mode to show.
    pub(crate) has_economy: bool,
    pub(crate) footprint_m2: u64,
    pub(crate) prepared_m2: u64,
    pub(crate) max_buildable_m2: u64,
    pub(crate) development_pm: u32,
    pub(crate) farmland_quality_pm: u16,
    pub(crate) field_room: u64,
    /// Basic housing capacity and occupants, including overflow.
    pub(crate) basic_housing_capacity: u64,
    pub(crate) basic_housing_occupants: u64,
    /// Natural makeshift shelter capacity and occupants, including overflow.
    pub(crate) makeshift_housing_capacity: u64,
    pub(crate) makeshift_housing_occupants: u64,
    /// Whole-place crowding in per-mille.
    pub(crate) crowding_pm: u64,
    pub(crate) pops: Vec<PopView>,
    pub(crate) buildings: Vec<BuildingView>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LandView {
    pub(crate) usable: BTreeMap<String, u32>,
    pub(crate) land: BTreeMap<String, u32>,
    pub(crate) sea: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GoodCounterView {
    pub(crate) good: String,
    pub(crate) units: u64,
    pub(crate) value_milli: Option<u128>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LastMonthView {
    pub(crate) closed: bool,
    pub(crate) produced: Vec<GoodCounterView>,
    pub(crate) used_by_pops: Vec<GoodCounterView>,
    pub(crate) lost: Vec<GoodCounterView>,
}

/// Every place's view, in scenario-file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EconomyView {
    pub(crate) places: Vec<PlaceView>,
    /// The live characters at this revision (CK-375): they are built with the places, so they share its revision.
    pub(crate) characters: super::character_view::CharacterView,
}

impl EconomyView {
    /// The view of the place called `id`.
    pub(crate) fn place(&self, id: &str) -> Option<&PlaceView> {
        self.places.iter().find(|place| place.id == id)
    }

    /// The stockpile map mode's value of each place, in scenario order: stored volume against capacity in thousandths, or
    /// [`ck_map::MODE_UNCOLOURED`] for a place with no capacity (design §3.22).
    pub(crate) fn stockpile_values(&self) -> Vec<u16> {
        self.places
            .iter()
            .map(|place| ck_map::stockpile_value(place.storage_used_ml, place.storage_capacity_ml))
            .collect()
    }

    /// The yield map mode's value of each place, in scenario order: the projected annual yield relative to the other places, red at
    /// the least and green at the most (design §3.22). A place with no economy is [`ck_map::MODE_UNCOLOURED`] and sets neither end.
    pub(crate) fn yield_values(&self) -> Vec<u16> {
        let yields: Vec<Option<u64>> = self
            .places
            .iter()
            .map(|place| place.has_economy.then_some(place.projected_yield))
            .collect();
        ck_map::yield_values(&yields)
    }
}

/// The view of `state` for the scenario's `place_ids`, in that order. A place the state has no row for (a scenario with no
/// economy data, or one the data leaves out) is a place of nobody, with nothing in it. `rules` is `None` for a scenario with
/// no economy data, which has no rows either.
#[cfg(test)]
pub(crate) fn build(
    rules: Option<&Rules>,
    state: &EconomyState,
    place_ids: &[String],
    date: Date,
) -> EconomyView {
    build_with_land(rules, state, place_ids, &BTreeMap::new(), date)
}

pub(crate) fn build_with_land(
    rules: Option<&Rules>,
    state: &EconomyState,
    place_ids: &[String],
    land_by_place: &BTreeMap<String, LandCounts>,
    date: Date,
) -> EconomyView {
    let places = place_ids
        .iter()
        .map(|id| {
            let row = state.places.iter().find(|place| &place.id == id);
            match (rules, row) {
                (Some(rules), Some(row)) => place_view(rules, row, land_by_place.get(id), date),
                _ => PlaceView {
                    id: id.clone(),
                    ..PlaceView::default()
                },
            }
        })
        .collect();
    EconomyView {
        places,
        characters: super::character_view::CharacterView::default(),
    }
}

fn size_of(pop: &PopState) -> u64 {
    pop.workers.saturating_add(pop.dependents)
}

/// Food held, in hundredths of a person-day: the pop's own store and its account at every storehouse.
fn food_centi(rules: &Rules, place: &PlaceState, pop_index: usize) -> u64 {
    let held = |stock: &[u64]| {
        rules
            .goods
            .iter()
            .zip(stock)
            .fold(0_u64, |sum, (good, units)| {
                sum.saturating_add(units.saturating_mul(u64::from(good.hunger_centi)))
            })
    };
    let pop = &place.pops[pop_index];
    let pantry = pop
        .consumption
        .pantry_hunger_centi
        .iter()
        .copied()
        .fold(0_u64, u64::saturating_add);
    let own = held(&pop.stock).saturating_add(pantry);
    place
        .buildings
        .iter()
        .filter_map(|building| building.storehouse_accounts.as_ref())
        .filter_map(|accounts| accounts.pops.get(pop_index))
        .fold(own, |sum, account| sum.saturating_add(held(account)))
}

/// Whole days of full eating that `centi` hundredths of a person-day feed `size` people; no people eat for no days.
fn days_of(centi: u64, size: u64) -> u64 {
    centi.checked_div(size.saturating_mul(100)).unwrap_or(0)
}

fn volume_ml(rules: &Rules, stock: &[u64]) -> u64 {
    rules
        .goods
        .iter()
        .zip(stock)
        .fold(0_u64, |sum, (good, units)| {
            sum.saturating_add(units.saturating_mul(if good.stacked_outside {
                0
            } else {
                good.volume_ml
            }))
        })
}

fn stock_view(rules: &Rules, stock: &[u64]) -> Vec<StockView> {
    rules
        .goods
        .iter()
        .zip(stock)
        .filter(|(_, amount)| **amount > 0)
        .map(|(good, amount)| StockView {
            good: good.id.clone(),
            amount: *amount,
            charged_ml: if good.stacked_outside {
                0
            } else {
                amount.saturating_mul(good.volume_ml)
            },
            outside: good.stacked_outside,
        })
        .collect()
}

fn pop_view(
    rules: &Rules,
    place: &PlaceState,
    index: usize,
    housing: Option<&ck_economy::PopHousing>,
) -> PopView {
    let pop = &place.pops[index];
    let employed = place
        .buildings
        .iter()
        .filter_map(|building| building.employment.get(index))
        .fold(0_u64, |sum, employment| {
            sum.saturating_add(employment.workers)
        });
    let mut jobs = Vec::new();
    for building in &place.buildings {
        let Some(definition) = rules.buildings.iter().find(|kind| kind.id == building.kind) else {
            continue;
        };
        let levels = if definition.methods.is_empty() {
            vec![building.levels]
        } else {
            building.method_levels.clone()
        };
        if let Ok(pools) = ck_economy::building_job_pools(definition, &levels) {
            for pool in pools {
                let count = building
                    .employment
                    .get(index)
                    .and_then(|entry| entry.jobs.get(&pool.job.id))
                    .copied()
                    .unwrap_or(0);
                if count > 0 {
                    jobs.push((building.kind.clone(), pool.job.id, count));
                }
            }
        }
    }
    PopView {
        stock: stock_view(rules, &pop.stock),
        room_ml: size_of(pop).saturating_mul(POP_STORE_ML_PER_PERSON),
        kind: pop.kind.clone(),
        culture: pop.culture.clone(),
        religion: pop.religion.clone(),
        workers: pop.workers,
        dependents: pop.dependents,
        employed,
        jobs,
        unemployed: pop.workers.saturating_sub(employed),
        food_days: days_of(food_centi(rules, place, index), size_of(pop)),
        food: food_view(rules, pop),
        necessities: u64::from(pop.consumption.necessities_pm),
        niceties: u64::from(pop.consumption.niceties_pm),
        luxuries: u64::from(pop.consumption.luxuries_pm),
        satisfaction: i64::from(pop.consumption.satisfaction),
        equilibrium: ck_economy::satisfaction_equilibrium(rules, pop)
            .unwrap_or_else(|| i64::from(pop.consumption.satisfaction)),
        births: pop.counters.last.births,
        deaths: pop.counters.last.deaths,
        basic_housing: housing.map_or(0, |housing| housing.basic),
        makeshift_housing: housing.map_or(0, |housing| housing.makeshift),
        crowding_pm: housing.and_then(|housing| housing.crowding_pm),
    }
}

fn food_view(rules: &Rules, pop: &PopState) -> Vec<FoodView> {
    let eligible = rules
        .goods
        .iter()
        .zip(&pop.stock)
        .filter_map(|(good, units)| {
            (*units > 0 && good.hunger_centi > 0).then_some((
                good,
                *units,
                u128::from(good.hunger_centi) * u128::from(*units),
            ))
        })
        .collect::<Vec<_>>();
    let total = eligible
        .iter()
        .fold(0_u128, |sum, (_, _, hunger)| sum.saturating_add(*hunger));
    if total == 0 {
        return Vec::new();
    }
    eligible
        .into_iter()
        .map(|(good, units, hunger)| FoodView {
            good: good.id.clone(),
            units,
            unit: good.unit.clone(),
            share_pm: u64::try_from(hunger * 1000 / total).unwrap_or(u64::MAX),
        })
        .collect()
}

fn building_capacity_ml(building: &BuildingState) -> u64 {
    if building.kind == "storehouse" {
        return u64::from(building.levels).saturating_mul(STOREHOUSE_STORE_ML_PER_LEVEL);
    }
    ck_economy::store_capacity_ml(building)
        .map(|capacity| u64::try_from(capacity).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// A building's projected annual food, in hundredths of a person-day: for each method with levels, each food good that is not a herd's
/// at its annual output scaled by the land's suitability (`method_suitability` is the sum over the levels, in ‰ of the output at
/// suitability 1000), with farmland fertility applied to non-sea methods, and the dairy of the building's cattle (72 pounds a cow a
/// year). The meats do not count.
fn projected_yield(rules: &Rules, building: &BuildingState, mean_fertility_pm: u16) -> u64 {
    let Some(definition) = rules
        .buildings
        .iter()
        .find(|candidate| candidate.id == building.kind)
    else {
        return 0;
    };
    let mut total = 0_u128;
    for (index, method) in definition.methods.iter().enumerate() {
        if building
            .method_levels
            .get(index)
            .is_none_or(|levels| *levels == 0)
        {
            continue;
        }
        for (good_id, annual) in &method.annual_output {
            if ck_economy::herd_supplies(rules, definition, good_id) {
                continue;
            }
            let Some((good_index, good)) = rules
                .goods
                .iter()
                .enumerate()
                .find(|(_, good)| &good.id == good_id)
            else {
                continue;
            };
            let suitability = building
                .method_suitability
                .get(index)
                .and_then(|row| row.get(good_index))
                .copied()
                .unwrap_or(0);
            let (fertility, fertility_reference) = if method.resource == "usable_land" {
                (
                    u128::from(mean_fertility_pm),
                    u128::from(rules.fertility_reference_pm),
                )
            } else {
                (
                    u128::from(rules.fertility_reference_pm),
                    u128::from(rules.fertility_reference_pm),
                )
            };
            let units = u128::from(*annual) * u128::from(suitability) * fertility
                / (1000 * fertility_reference);
            total = total.saturating_add(units * u128::from(good.hunger_centi));
        }
    }
    for (species, head) in rules.herd_species.iter().zip(&building.herds) {
        if !definition.herds.contains_key(&species.animal) {
            continue;
        }
        for product in &species.products {
            if let Some(good) = rules.goods.iter().find(|good| good.id == product.good) {
                total = total.saturating_add(
                    u128::from(*head)
                        .saturating_mul(u128::from(product.amount_milli.unwrap_or(0)))
                        .saturating_mul(u128::from(good.hunger_centi))
                        / 1000,
                );
            }
        }
    }
    u64::try_from(total).unwrap_or(u64::MAX)
}

fn building_view(
    rules: &Rules,
    building: &BuildingState,
    housing_occupants: u64,
    date: Date,
) -> BuildingView {
    let definition = rules
        .buildings
        .iter()
        .find(|candidate| candidate.id == building.kind);
    let methods = definition
        .map(|definition| {
            definition
                .methods
                .iter()
                .zip(&building.method_levels)
                .filter(|(_, levels)| **levels > 0)
                .map(|(method, levels)| (method.id.clone(), u64::from(*levels)))
                .collect()
        })
        .unwrap_or_default();
    let jobs = definition
        .and_then(|definition| {
            let levels = if definition.methods.is_empty() {
                vec![building.levels]
            } else {
                building.method_levels.clone()
            };
            ck_economy::building_job_pools(definition, &levels).ok()
        })
        .unwrap_or_default()
        .into_iter()
        .map(|pool| {
            let filled = building.employment.iter().fold(0_u64, |sum, entry| {
                sum.saturating_add(entry.jobs.get(&pool.job.id).copied().unwrap_or(0))
            });
            (pool.job.id, filled, pool.places)
        })
        .collect::<Vec<_>>();
    let positions = jobs
        .iter()
        .fold(0_u64, |sum, (_, _, places)| sum.saturating_add(*places));
    let filled = jobs
        .iter()
        .fold(0_u64, |sum, (_, filled, _)| sum.saturating_add(*filled));
    let maxima = ck_economy::herd_maximums(rules, building).unwrap_or_default();
    let herds = rules
        .herd_species
        .iter()
        .zip(&building.herds)
        .zip(maxima)
        .filter_map(|((species, head), maximum)| {
            maximum.map(|maximum| HerdView {
                species: species.animal.clone(),
                head: *head,
                maximum,
            })
        })
        .collect();
    BuildingView {
        stock: stock_view(rules, &building.stock),
        made: rules
            .goods
            .iter()
            .zip(&building.last_made)
            .filter(|(_, n)| **n > 0)
            .map(|(good, n)| (good.id.clone(), *n))
            .collect(),
        kind: building.kind.clone(),
        levels: u64::from(building.levels),
        methods,
        filled,
        open: positions.saturating_sub(filled),
        jobs,
        abandoned: building.abandoned,
        herds,
        store_used_ml: volume_ml(rules, &building.stock),
        store_capacity_ml: building_capacity_ml(building),
        wage: building.posted_wage(),
        wages: wage_views(building, date),
        housing_occupants,
    }
}

fn wage_views(building: &BuildingState, date: Date) -> Vec<WageView> {
    let newest = if date.day > 1 || date.hour > super::core::ECONOMY_HOUR {
        date.month
    } else if date.month == 1 {
        12
    } else {
        date.month - 1
    };
    (0_u16..12)
        .map(|offset| {
            let month = u8::try_from((u16::from(newest) + 11 - offset) % 12 + 1)
                .expect("the month is in 1..=12");
            let index = usize::from(month - 1);
            let year = building
                .paid_years
                .get(index)
                .copied()
                .filter(|year| *year != 0);
            WageView {
                month,
                year,
                wage_milli: building.wage_history.get(index).copied().unwrap_or(0),
                paid: year.and_then(|_| building.paid_history.get(index).copied()),
            }
        })
        .collect()
}

#[expect(
    clippy::too_many_lines,
    reason = "A place view keeps its immutable economy totals, derived land values and per-building projections together."
)]
fn place_view(
    rules: &Rules,
    place: &PlaceState,
    land: Option<&LandCounts>,
    date: Date,
) -> PlaceView {
    let mean_fertility_pm = land
        .and_then(|land| ck_economy::mean_fertility(land).ok())
        .unwrap_or(rules.fertility_reference_pm);
    let shelter = land
        .and_then(|land| ck_economy::natural_shelter(rules, land).ok())
        .unwrap_or_default();
    let housing = ck_economy::housing_fill(rules, place, shelter).ok();
    let pops: Vec<PopView> = (0..place.pops.len())
        .map(|index| {
            pop_view(
                rules,
                place,
                index,
                housing.as_ref().and_then(|fill| fill.pops.get(index)),
            )
        })
        .collect();
    let buildings: Vec<BuildingView> = place
        .buildings
        .iter()
        .enumerate()
        .map(|(index, building)| {
            building_view(
                rules,
                building,
                housing
                    .as_ref()
                    .and_then(|fill| fill.building_occupants.get(index))
                    .copied()
                    .unwrap_or_default(),
                date,
            )
        })
        .collect();
    let workers = place
        .pops
        .iter()
        .fold(0_u64, |sum, pop| sum.saturating_add(pop.workers));
    let dependents = place
        .pops
        .iter()
        .fold(0_u64, |sum, pop| sum.saturating_add(pop.dependents));
    let food = (0..place.pops.len()).fold(0_u64, |sum, index| {
        sum.saturating_add(food_centi(rules, place, index))
    });
    let pops_used = place.pops.iter().fold(0_u64, |sum, pop| {
        sum.saturating_add(volume_ml(rules, &pop.stock))
    });
    let buildings_used = buildings.iter().fold(0_u64, |sum, building| {
        sum.saturating_add(building.store_used_ml)
    });
    let capacity = buildings.iter().fold(
        workers
            .saturating_add(dependents)
            .saturating_mul(POP_STORE_ML_PER_PERSON),
        |sum, building| sum.saturating_add(building.store_capacity_ml),
    );
    let prices = rules
        .goods
        .iter()
        .zip(&place.prices_milli)
        .enumerate()
        .map(|(index, (good, price))| PriceView {
            good: good.id.clone(),
            tags: good.tags.clone(),
            price_milli: *price,
            base: good.base_price,
            bought: place
                .last_market_orders
                .as_ref()
                .and_then(|orders| orders.buy_units.get(index).copied()),
            offered: place
                .last_market_orders
                .as_ref()
                .and_then(|orders| orders.sell_units.get(index).copied()),
        })
        .collect();
    let land_view = land.map_or_else(LandView::default, |counts| LandView {
        usable: counts.fertile_cells_by_terrain.clone(),
        land: counts.land_cells.clone(),
        sea: counts.sea,
    });
    let last_month = if place.counters.closed {
        let entries = |units: &[u64], values: Option<&[u128]>| {
            rules
                .goods
                .iter()
                .enumerate()
                .filter_map(|(index, good)| {
                    let units = units.get(index).copied().unwrap_or_default();
                    (units > 0).then(|| GoodCounterView {
                        good: good.id.clone(),
                        units,
                        value_milli: values.and_then(|values| values.get(index).copied()),
                    })
                })
                .collect()
        };
        LastMonthView {
            closed: true,
            produced: entries(
                &place.counters.last.produced,
                Some(&place.counters.last.produced_value_milli),
            ),
            used_by_pops: entries(
                &place.counters.last.used_by_pops,
                Some(&place.counters.last.used_by_pops_value_milli),
            ),
            lost: entries(&place.counters.last.lost, None),
        }
    } else {
        LastMonthView::default()
    };
    PlaceView {
        id: place.id.clone(),
        temperature: None,
        wind: None,
        workers,
        dependents,
        food_days: days_of(food, workers.saturating_add(dependents)),
        storage_used_ml: pops_used.saturating_add(buildings_used),
        storage_capacity_ml: capacity,
        prices,
        land: land_view,
        last_month,
        overflow_lost: place.counters.last.losses,
        projected_yield: place.buildings.iter().fold(0_u64, |sum, building| {
            sum.saturating_add(projected_yield(rules, building, mean_fertility_pm))
        }),
        has_economy: !place.pops.is_empty() || !place.buildings.is_empty(),
        footprint_m2: place
            .buildings
            .iter()
            .map(|building| {
                u64::try_from(
                    rules
                        .footprint_m2
                        .get(&building.kind)
                        .copied()
                        .unwrap_or_default()
                        .max(0),
                )
                .expect("validated footprints are nonnegative")
                    * u64::from(building.levels)
            })
            .sum(),
        prepared_m2: place.prepared_m2,
        max_buildable_m2: land
            .and_then(|land| ck_economy::max_buildable_m2(rules, place.development_pm, land).ok())
            .unwrap_or_default(),
        development_pm: place.development_pm,
        farmland_quality_pm: land
            .and_then(|land| ck_economy::mean_fertility(land).ok())
            .unwrap_or_default(),
        field_room: land
            .map(|land| {
                let available = land
                    .fertility_pm
                    .values()
                    .copied()
                    .fold(0_u64, u64::saturating_add)
                    / 1000;
                let occupied = place.buildings.iter().fold(0_u64, |sum, building| {
                    let Some(definition) = rules
                        .buildings
                        .iter()
                        .find(|definition| definition.id == building.kind)
                    else {
                        return sum;
                    };
                    let land_levels = definition
                        .methods
                        .iter()
                        .zip(&building.method_levels)
                        .filter(|(method, _)| method.resource == "usable_land")
                        .fold(0_u64, |levels, (_, count)| {
                            levels.saturating_add(u64::from(*count))
                        });
                    sum.saturating_add(
                        u64::from(definition.land_per_level).saturating_mul(land_levels),
                    )
                });
                available.saturating_sub(occupied)
            })
            .unwrap_or_default(),
        basic_housing_capacity: housing.as_ref().map_or(0, |fill| fill.basic_capacity),
        basic_housing_occupants: housing.as_ref().map_or(0, |fill| fill.basic_occupants),
        makeshift_housing_capacity: housing.as_ref().map_or(0, |fill| fill.makeshift_capacity),
        makeshift_housing_occupants: housing.as_ref().map_or(0, |fill| fill.makeshift_occupants),
        crowding_pm: housing.as_ref().map_or(0, |fill| fill.crowding_pm),
        pops,
        buildings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ck_economy::{
        CounterBuckets, Employment, NearbyPlace, PlaceTrade, StorehouseAccounts, TradeCounters,
        run_daily_economy_with_context,
    };
    use ck_log::Level;
    use ck_sim::{Canonical, Date, GameTime, Save, Scheduler, Simulation};
    use std::collections::BTreeMap;
    use std::path::Path;

    const fn test_date() -> Date {
        Date {
            year: 768,
            month: 5,
            day: 2,
            hour: 0,
        }
    }

    fn shipped_rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
            .expect("the shipped rules load")
    }

    fn index_of(rules: &Rules, good: &str) -> usize {
        rules
            .goods
            .iter()
            .position(|candidate| candidate.id == good)
            .expect("the good exists")
    }

    /// One place: a peasant pop of 80 workers and 40 dependents with 1,000 oats (1,800 hundredths of a person-day each) and
    /// 10 ale, a farm of 3 levels with 25 workers employed, and a storehouse level holding 5 oats for the pop's account.
    #[expect(
        clippy::too_many_lines,
        reason = "the fixture keeps its full state together"
    )]
    fn fixture(rules: &Rules) -> PlaceState {
        let goods = rules.goods.len();
        let mut stock = vec![0; goods];
        stock[index_of(rules, "oats")] = 1_000;
        stock[index_of(rules, "ale")] = 10;
        let mut farm_stock = vec![0; goods];
        farm_stock[index_of(rules, "barley")] = 20;
        let mut account = vec![0; goods];
        account[index_of(rules, "oats")] = 5;
        let accounts = StorehouseAccounts {
            pops: vec![account.clone()],
            communal: vec![0; goods],
        };
        PlaceState {
            id: "fixture".to_owned(),
            counters: CounterBuckets::default(),
            pops: vec![PopState {
                id: 0,
                kind: "peasant".to_owned(),
                culture: "ulaid".to_owned(),
                religion: "christianity".to_owned(),
                literacy_pm: 0,
                workers: 80,
                dependents: 40,
                employment: vec![
                    Employment {
                        jobs: std::collections::BTreeMap::from([("peasant_labour".into(), 25)]),
                        workers: 25,
                    },
                    Employment::default(),
                ],
                stock,
                consumption: PopConsumption {
                    pantry_hunger_centi: vec![0; goods],
                    ticking_carries: vec![0; goods],
                    necessities_pm: 700,
                    niceties_pm: 350,
                    luxuries_pm: 100,
                    satisfaction: 5,
                    ..PopConsumption::default()
                },
                counters: CounterBuckets::default(),
                short_on_rent: false,
                housed_last: false,
                genetics: ck_economy::PopGenetics::default(),
                pending_births: 0,
                pending_deaths: 0,
            }],
            buildings: vec![
                BuildingState {
                    heir: None,
                    abandoned: false,
                    kind: "farm".to_owned(),
                    levels: 3,
                    method_levels: vec![3, 0, 0, 0],
                    level_records: Vec::new(),
                    herds: vec![27, 18, 15, 0],
                    employment: vec![Employment {
                        jobs: std::collections::BTreeMap::from([("peasant_labour".into(), 25)]),
                        workers: 25,
                    }],
                    stock: farm_stock,
                    storehouse_accounts: None,
                    production_carries: vec![vec![0; goods]; 4],
                    method_suitability: vec![vec![0; goods]; 4],
                    herd_birth_carries: vec![0; 4],
                    herd_death_carries: vec![0; 4],
                    herd_product_carries: vec![0; 2],
                    herd_grazing_sums: vec![0; 4],
                    wage_carries: vec![0; goods],
                    pay_carries: vec![0; goods],
                    last_made: Vec::new(),
                    fee_carries: Vec::new(),
                    wage_history: vec![1_234; 12],
                    paid_history: vec![0; 12],
                    paid_years: vec![0; 12],
                },
                BuildingState {
                    heir: None,
                    abandoned: false,
                    kind: "storehouse".to_owned(),
                    levels: 1,
                    method_levels: vec![1, 0, 0],
                    level_records: Vec::new(),
                    herds: Vec::new(),
                    employment: vec![Employment::default()],
                    stock: account,
                    storehouse_accounts: Some(accounts),
                    production_carries: Vec::new(),
                    method_suitability: Vec::new(),
                    herd_birth_carries: Vec::new(),
                    herd_death_carries: Vec::new(),
                    herd_product_carries: Vec::new(),
                    herd_grazing_sums: Vec::new(),
                    wage_carries: vec![0; goods],
                    pay_carries: vec![0; goods],
                    last_made: Vec::new(),
                    fee_carries: vec![vec![0; goods]; 2],
                    wage_history: vec![0; 12],
                    paid_history: vec![0; 12],
                    paid_years: vec![0; 12],
                },
            ],
            prices_milli: rules
                .goods
                .iter()
                .map(|good| good.base_price * 1000)
                .collect(),
            prepared_m2: 24_200,
            development_pm: 1000,
            price_drift_carries: vec![0; rules.goods.len()],
            last_market_orders: None,
            trade: PlaceTrade::default(),
        }
    }

    fn field_room_for(rules: &Rules, mut place: PlaceState) -> u64 {
        place.id = "fixture".to_owned();
        let state = EconomyState {
            next_pop_serial: 1,
            places: vec![place],
            trade: TradeCounters::default(),
        };
        let land = BTreeMap::from([(
            "fixture".to_owned(),
            LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 10)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 20_000)]),
                fertile_cells: 10,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        )]);
        build_with_land(
            Some(rules),
            &state,
            &["fixture".to_owned()],
            &land,
            test_date(),
        )
        .place("fixture")
        .expect("the fixture is in the view")
        .field_room
    }

    fn view_for_place(rules: &Rules, mut place: PlaceState) -> PlaceView {
        place.id = "fixture".to_owned();
        let view = build(
            Some(rules),
            &EconomyState {
                next_pop_serial: 1,
                places: vec![place],
                trade: TradeCounters::default(),
            },
            &["fixture".to_owned()],
            test_date(),
        );
        view.place("fixture")
            .expect("the fixture is in the view")
            .clone()
    }

    #[test]
    fn stock_and_made_are_per_owner_in_goods_order_and_use_exact_volume() {
        let mut rules = shipped_rules();
        let honey = index_of(&rules, "honey");
        let peat = index_of(&rules, "peat");
        rules.goods[honey].volume_ml = 350;
        rules.goods[peat].stacked_outside = true;
        let mut place = fixture(&rules);
        place.pops[0].stock[index_of(&rules, "oats")] = 25;
        place.buildings[0].stock.fill(0);
        place.buildings[0].stock[honey] = 10;
        place.buildings[0].stock[peat] = 1;
        place.buildings[0].last_made = vec![0; rules.goods.len()];
        place.buildings[0].last_made[honey] = 17;
        let view = view_for_place(&rules, place);
        let farm = &view.buildings[0];
        let honey = farm.stock.iter().find(|good| good.good == "honey").unwrap();
        assert_eq!(
            (honey.amount, honey.charged_ml, honey.outside),
            (10, 3500, false)
        );
        let peat = farm.stock.iter().find(|good| good.good == "peat").unwrap();
        assert_eq!((peat.amount, peat.charged_ml, peat.outside), (1, 0, true));
        assert_eq!(farm.made, [("honey".to_owned(), 17)]);
        assert_eq!(farm.store_used_ml, 3500);
        assert_eq!(view.pops[0].room_ml, 120 * 720_000);
        assert_eq!(
            view.pops[0]
                .stock
                .iter()
                .find(|good| good.good == "oats")
                .unwrap()
                .amount,
            25
        );
        assert_eq!(
            view.buildings[1]
                .stock
                .iter()
                .find(|good| good.good == "oats")
                .unwrap()
                .amount,
            5
        );
    }

    #[test]
    fn pop_food_lists_only_hunger_goods_with_their_floor_share() {
        let rules = shipped_rules();
        let mut place = fixture(&rules);
        let oats = index_of(&rules, "oats");
        let barley = index_of(&rules, "barley");
        let wool = index_of(&rules, "wool");
        place.pops[0].stock.fill(0);
        place.pops[0].stock[oats] = 300;
        place.pops[0].stock[barley] = 100;
        place.pops[0].stock[wool] = 5;
        let view = view_for_place(&rules, place.clone());
        assert_eq!(
            view.pops[0].food,
            [
                FoodView {
                    good: "oats".to_owned(),
                    units: 300,
                    unit: "bushel".to_owned(),
                    share_pm: 658
                },
                FoodView {
                    good: "barley".to_owned(),
                    units: 100,
                    unit: "bushel".to_owned(),
                    share_pm: 341
                },
            ]
        );
        place.pops[0].stock.fill(0);
        place.pops[0].stock[wool] = 5;
        assert!(view_for_place(&rules, place).pops[0].food.is_empty());
    }

    #[test]
    fn payday_view_orders_calendar_slots_by_date_and_keeps_the_written_year() {
        let rules = shipped_rules();
        let mut place = fixture(&rules);
        let farm = &mut place.buildings[0];
        farm.wage_history = (1..=12).map(|month| 2_000 + month).collect();
        farm.paid_history = vec![15; 12];
        farm.paid_years = vec![768; 12];
        farm.paid_years[1] = 769;
        let make = |date| {
            let state = EconomyState {
                next_pop_serial: 1,
                places: vec![place.clone()],
                trade: TradeCounters::default(),
            };
            build(Some(&rules), &state, &["fixture".to_owned()], date)
                .place("fixture")
                .unwrap()
                .buildings[0]
                .wages
                .clone()
        };
        let before = make(Date {
            year: 768,
            month: 5,
            day: 1,
            hour: 0,
        });
        assert_eq!(before[0].month, 4);
        let before_noon = make(Date {
            year: 768,
            month: 5,
            day: 1,
            hour: 12,
        });
        assert_eq!(before_noon[0].month, 4);
        let after = make(Date {
            year: 768,
            month: 5,
            day: 1,
            hour: 13,
        });
        assert_eq!(after[0].month, 5);
        let next_day = make(Date {
            year: 768,
            month: 5,
            day: 2,
            hour: 0,
        });
        assert_eq!(next_day[0].month, 5);
        let january = make(Date {
            year: 768,
            month: 1,
            day: 1,
            hour: 12,
        });
        assert_eq!(january[0].month, 12);
        let skipped = make(Date {
            year: 769,
            month: 2,
            day: 10,
            hour: 0,
        });
        assert_eq!(skipped.len(), 12);
        assert_eq!((skipped[0].month, skipped[0].year), (2, Some(769)));
        assert_eq!(
            (
                skipped[1].month,
                skipped[1].year,
                skipped[1].paid,
                skipped[1].wage_milli
            ),
            (1, Some(768), Some(15), 2_001)
        );
        assert_eq!(
            skipped.iter().map(|row| row.wage_milli).sum::<u64>() / 12,
            place.buildings[0].posted_wage()
        );
        let seeded = fixture(&rules);
        let seeded_view = view_for_place(&rules, seeded);
        assert!(
            seeded_view.buildings[0]
                .wages
                .iter()
                .all(|row| row.year.is_none() && row.paid.is_none())
        );
    }

    #[expect(
        clippy::too_many_lines,
        reason = "The day-one fixture sets each dimension of its three places in one literal pass."
    )]
    fn day_one_economy_fixture() -> (Rules, EconomyState, Vec<Vec<u64>>, Date) {
        let mut rules = shipped_rules();
        rules.hire_pm = 1000;
        rules.leave_pm = 1000;
        let peasant = rules
            .pop_types
            .iter_mut()
            .find(|kind| kind.id == "peasant")
            .expect("the peasant rule exists");
        peasant.births_pm_year = 0;
        peasant.worker_deaths_pm_year = 10_000;
        for curve_name in ["deaths_hunger", "deaths_comfort", "deaths_crowding"] {
            for point in rules
                .curves
                .get_mut(curve_name)
                .expect("the death curve exists")
            {
                point[1] = 1_200;
            }
        }

        let goods = rules.goods.len();
        let mut dying = fixture(&rules);
        dying.id = "dying-place".to_owned();
        dying.buildings.clear();
        dying.pops.truncate(1);
        dying.pops[0].workers = 1;
        dying.pops[0].dependents = 0;
        dying.pops[0].employment.clear();
        dying.pops[0].stock.fill(0);
        dying.pops[0].consumption.pantry_hunger_centi.fill(0);
        dying.pops[0].consumption.ticking_carries.fill(0);
        dying.counters.last.losses = 99;
        dying.counters.current.losses = 6;
        dying.pops[0].counters.last.births = 98;
        dying.pops[0].counters.last.deaths = 97;
        dying.pops[0].counters.current.births = 4;
        dying.pops[0].counters.current.deaths = 2;

        let mut source = fixture(&rules);
        source.id = "source-place".to_owned();
        source.buildings.clear();
        source.pops.truncate(1);
        source.pops[0].kind = "labourer".to_owned();
        source.pops[0].workers = 20;
        source.pops[0].dependents = 0;
        source.pops[0].employment.clear();
        source.pops[0].stock.fill(0);
        source.pops[0].consumption.pantry_hunger_centi.fill(0);
        source.pops[0].consumption.ticking_carries.fill(0);

        let mut destination = fixture(&rules);
        destination.id = "destination-place".to_owned();
        destination.pops.clear();
        destination.buildings.truncate(1);
        let holdings = destination
            .buildings
            .first_mut()
            .expect("the fixture has a building");
        holdings.kind = "holdings".to_owned();
        holdings.levels = 1;
        holdings.method_levels = vec![1, 0];
        holdings.level_records = vec![ck_economy::LevelRecord {
            number: 0,
            method: 0,
            ground: Some("pasture".to_owned()),
            paydays: ck_economy::MATURE_PAYDAYS,
            unpaid_work: std::collections::BTreeMap::new(),
            last_switch_year: None,
        }];
        holdings.herds = vec![0; 4];
        holdings.employment.clear();
        holdings.stock = vec![0; goods];
        holdings.production_carries = vec![vec![0; goods]; 2];
        let definition = rules.buildings.iter().find(|b| b.id == "holdings").unwrap();
        (holdings.method_suitability, holdings.herd_grazing_sums) =
            ck_economy::derive_level_sums(&rules, definition, &holdings.level_records).unwrap();
        holdings.herd_birth_carries = vec![0; 4];
        holdings.herd_death_carries = vec![0; 4];
        holdings.herd_product_carries = vec![0; 2];
        holdings.wage_carries = vec![0; goods];
        holdings.pay_carries = vec![0; goods];
        holdings.fee_carries.clear();
        holdings.wage_history = vec![100; 12];

        let mut state = EconomyState {
            next_pop_serial: 1,
            places: vec![dying, source, destination],
            trade: TradeCounters::default(),
        };
        let nearby = BTreeMap::from([(
            "destination-place".to_owned(),
            vec![NearbyPlace {
                id: "source-place".to_owned(),
                distance_m: 1_000,
            }],
        )]);
        let date = Date {
            year: 768,
            month: 1,
            day: 1,
            hour: 12,
        };
        let ledger = run_daily_economy_with_context(
            &rules,
            &mut state,
            date,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &nearby,
        )
        .expect("the day-one economy tick completes");
        (rules, state, ledger.movement_by_pop, date)
    }

    fn app_core_for_economy(
        rules: Rules,
        state: &EconomyState,
        ids: &[String],
        start: Date,
    ) -> (
        crate::game::core::Core<crate::game::GameState, crate::game::GameCommand>,
        crate::game::saves::Identity,
        crate::testing::Dir,
        std::path::PathBuf,
    ) {
        let view_ids = ids.to_vec();
        let views = Box::new(move |game: &crate::game::GameState, date| {
            build(Some(&rules), &game.economy, &view_ids, date)
        });
        let game_state = crate::game::GameState {
            characters: ck_characters::CharacterStore::default(),
            economy: state.clone(),
            stretches: std::collections::BTreeMap::new(),
            lows: crate::game::lows::Lows::default(),
        };
        let time = GameTime::from_date(start).expect("the save date is valid");
        let identity = crate::game::saves::Identity {
            build: "ck246-test".to_owned(),
            scenario: "fixture".to_owned(),
            scenario_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_owned(),
            map_hash: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
            scripts_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_owned(),
            rules_hash: "0000000000000000000000000000000000000000000000000000000000000000"
                .to_owned(),
            start,
        };
        let save_dir = crate::testing::Dir::new();
        let save_path = save_dir.0.join("saves");
        let outputs = crate::game::core::Outputs {
            log: Box::new(|_: Level, _: String| {}),
            publish: Box::new(|_| {}),
            fail: Box::new(|_| {}),
        };
        let apply: fn(&mut crate::game::GameState, crate::game::GameCommand) = crate::game::apply;
        let simulation = Simulation::new(
            time,
            42,
            game_state,
            Scheduler::<crate::game::GameState>::new(),
            apply,
        );
        let core = crate::game::core::Core::new_with_views(
            simulation,
            identity.clone(),
            save_path.clone(),
            crate::game::core::Rebuild {
                scheduler: Box::new(Scheduler::<crate::game::GameState>::new),
                reset: Box::new(|_| {}),
                apply,
            },
            outputs,
            Some(views),
        );
        (core, identity, save_dir, save_path)
    }

    #[test]
    fn day_one_death_and_cross_place_movement_survive_app_quicksave_and_quickload() {
        let (rules, state, movement_by_pop, date) = day_one_economy_fixture();
        assert_eq!(
            state.places[0].pops[0].workers, 0,
            "the one-worker pop dies"
        );
        assert_eq!(state.places[0].pops[0].counters.current.deaths, 1);
        assert!(
            movement_by_pop[2].iter().sum::<u64>() > 0,
            "the day-one tick recruits across places"
        );
        assert!(
            state.places[1].pops[0].workers < 20,
            "recruits leave the source pop"
        );
        assert!(
            state.places[2]
                .pops
                .iter()
                .any(|pop| pop.kind == "monk" && pop.workers > 0)
        );

        let ids = state
            .places
            .iter()
            .map(|place| place.id.clone())
            .collect::<Vec<_>>();
        let start = Date { hour: 13, ..date };
        let (mut core, identity, _save_dir, save_path) =
            app_core_for_economy(rules, &state, &ids, start);
        let view_before_save = core
            .snapshot()
            .economy
            .expect("the starting app snapshot has a view");
        let dying_view = view_before_save
            .place("dying-place")
            .expect("the dying place is in the view");
        assert_eq!(
            (
                dying_view.pops[0].births,
                dying_view.pops[0].deaths,
                dying_view.overflow_lost
            ),
            (4, 2, 6),
            "the saved view contains nonzero completed-month counters"
        );
        core.dispatch(
            crate::game::core::Message::Control(crate::game::controls::Control::Quicksave),
            std::time::Duration::ZERO,
        );
        let save_bytes = std::fs::read(crate::game::saves::quicksave_path(&save_path))
            .expect("the app quicksave was written");
        let saved = Save::<crate::game::GameState>::from_bytes(&save_bytes, &identity.expected())
            .expect("the app quicksave decodes");
        let mut saved_economy = state.clone();
        for place in &mut saved_economy.places {
            place.last_market_orders = None;
        }
        assert_eq!(
            saved.state.economy, saved_economy,
            "the quicksave retains canonical state and drops derived market orders"
        );

        core.dispatch(
            crate::game::core::Message::Control(crate::game::controls::Control::Quickload),
            std::time::Duration::ZERO,
        );
        let view_after_load = core
            .snapshot()
            .economy
            .expect("quickload rebuilds and publishes the app view");
        let mut expected_view = (*view_before_save).clone();
        for place in &mut expected_view.places {
            for price in &mut place.prices {
                price.bought = None;
                price.offered = None;
            }
        }
        assert_eq!(
            expected_view, *view_after_load,
            "load keeps canonical view fields and clears derived daily market orders"
        );
        let loaded_dying = view_after_load
            .place("dying-place")
            .expect("quickload keeps the dying place in the view");
        assert_eq!(
            (
                loaded_dying.pops[0].births,
                loaded_dying.pops[0].deaths,
                loaded_dying.overflow_lost
            ),
            (4, 2, 6),
            "quickload preserves the three nonzero panel counters"
        );
        assert_eq!(
            core.state().economy,
            saved.state.economy,
            "the loaded economy equals the saved economy"
        );
    }

    #[test]
    fn a_view_reads_last_month_counters_and_not_current_events() {
        let rules = shipped_rules();
        let mut place = fixture(&rules);
        place.counters.last.losses = 7;
        place.counters.current.losses = 13;
        place.pops[0].counters.last.births = 3;
        place.pops[0].counters.last.deaths = 1;
        place.pops[0].counters.current.births = 17;
        place.pops[0].counters.current.deaths = 19;
        let view = build(
            Some(&rules),
            &EconomyState {
                next_pop_serial: 1,
                places: vec![place],
                trade: TradeCounters::default(),
            },
            &["fixture".to_owned()],
            test_date(),
        );
        let place_view = view.place("fixture").expect("the fixture is in the view");
        assert_eq!(
            (
                place_view.pops[0].births,
                place_view.pops[0].deaths,
                place_view.overflow_lost
            ),
            (3, 1, 7)
        );
    }

    #[test]
    fn a_first_of_month_tick_rolls_the_completed_counters_into_the_view() {
        let (rules, state, _, _) = day_one_economy_fixture();
        let dying = &state.places[0];
        assert_eq!(
            (
                dying.pops[0].counters.last.births,
                dying.pops[0].counters.last.deaths
            ),
            (4, 2),
            "the prior current bucket became last at the month boundary"
        );
        assert_eq!(
            dying.pops[0].counters.current.deaths, 1,
            "a new death occurred after rollover"
        );
        assert_eq!(
            (dying.counters.last.losses, dying.counters.current.losses),
            (6, 0)
        );
        let ids = state
            .places
            .iter()
            .map(|place| place.id.clone())
            .collect::<Vec<_>>();
        let view = build(Some(&rules), &state, &ids, test_date());
        let dying_view = view.place("dying-place").expect("the place is in the view");
        assert_eq!(
            (
                dying_view.pops[0].births,
                dying_view.pops[0].deaths,
                dying_view.overflow_lost
            ),
            (4, 2, 6),
            "the view shows the month just completed, not current-month events"
        );
    }

    #[test]
    fn place_view_shows_closed_month_good_rows_in_rules_order_and_hides_open_months() {
        let rules = shipped_rules();
        let mut place = fixture(&rules);
        place.counters = CounterBuckets::default();
        place.counters.last.produced = vec![0; rules.goods.len()];
        place.counters.last.produced_value_milli = vec![0; rules.goods.len()];
        place.counters.last.used_by_pops = vec![0; rules.goods.len()];
        place.counters.last.used_by_pops_value_milli = vec![0; rules.goods.len()];
        place.counters.last.lost = vec![0; rules.goods.len()];
        let oats = index_of(&rules, "oats");
        let barley = index_of(&rules, "barley");
        let ale = index_of(&rules, "ale");
        place.counters.last.produced[oats] = 2;
        place.counters.last.produced_value_milli[oats] = 210_000;
        place.counters.last.used_by_pops[barley] = 3;
        place.counters.last.used_by_pops_value_milli[barley] = 315_000;
        place.counters.last.lost[ale] = 1;

        let build_place = |place: &PlaceState| {
            build(
                Some(&rules),
                &EconomyState {
                    next_pop_serial: 1,
                    places: vec![place.clone()],
                    trade: TradeCounters::default(),
                },
                &["fixture".to_owned()],
                test_date(),
            )
            .places
            .remove(0)
        };
        let open = build_place(&place);
        assert!(!open.last_month.closed);
        assert!(open.last_month.produced.is_empty());
        assert!(open.last_month.used_by_pops.is_empty());
        assert!(open.last_month.lost.is_empty());

        place.counters.closed = true;
        place.counters.last.produced.fill(0);
        place.counters.last.produced_value_milli.fill(0);
        place.counters.last.used_by_pops.fill(0);
        place.counters.last.used_by_pops_value_milli.fill(0);
        place.counters.last.lost.fill(0);
        let empty_closed = build_place(&place);
        assert!(empty_closed.last_month.closed);
        assert!(empty_closed.last_month.produced.is_empty());
        assert!(empty_closed.last_month.used_by_pops.is_empty());
        assert!(empty_closed.last_month.lost.is_empty());

        place.counters.last.produced[oats] = 2;
        place.counters.last.produced_value_milli[oats] = 210_000;
        place.counters.last.used_by_pops[barley] = 3;
        place.counters.last.used_by_pops_value_milli[barley] = 315_000;
        place.counters.last.lost[ale] = 1;
        let closed = build_place(&place);
        assert_eq!(
            closed.last_month.produced,
            [GoodCounterView {
                good: "oats".to_owned(),
                units: 2,
                value_milli: Some(210_000),
            }]
        );
        assert_eq!(
            closed.last_month.used_by_pops,
            [GoodCounterView {
                good: "barley".to_owned(),
                units: 3,
                value_milli: Some(315_000),
            }]
        );
        assert_eq!(
            closed.last_month.lost,
            [GoodCounterView {
                good: "ale".to_owned(),
                units: 1,
                value_milli: None,
            }]
        );
    }

    #[test]
    fn place_view_keeps_exact_u128_order_units_and_none_before_a_price_step() {
        let rules = shipped_rules();
        let mut place = fixture(&rules);
        let view = view_for_place(&rules, place.clone());
        assert_eq!(
            view.prices
                .iter()
                .find(|price| price.good == "oats")
                .unwrap()
                .tags,
            ["food"]
        );
        assert!(
            view.prices
                .iter()
                .find(|price| price.good == "herbs")
                .unwrap()
                .tags
                .is_empty()
        );
        assert!(
            view.prices
                .iter()
                .all(|price| price.bought.is_none() && price.offered.is_none())
        );

        let oats = index_of(&rules, "oats");
        let mut buy_units = vec![0; rules.goods.len()];
        let mut sell_units = vec![0; rules.goods.len()];
        buy_units[oats] = u128::from(u64::MAX) + 5;
        sell_units[oats] = 1240;
        place.last_market_orders = Some(ck_economy::DailyMarketOrders {
            buy_units,
            sell_units,
        });
        let view = view_for_place(&rules, place);
        let oats = view
            .prices
            .iter()
            .find(|price| price.good == "oats")
            .unwrap();
        assert_eq!(oats.bought, Some(u128::from(u64::MAX) + 5));
        assert_eq!(oats.offered, Some(1240));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one fixture place and every number the panel derives from it, read in order"
    )]
    fn a_fixture_place_derives_every_number_the_panel_shows() {
        let rules = shipped_rules();
        let state = EconomyState {
            next_pop_serial: 1,
            places: vec![fixture(&rules)],
            trade: TradeCounters::default(),
        };
        let land = BTreeMap::from([(
            "fixture".to_owned(),
            LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 10)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 5_500)]),
                fertile_cells: 10,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        )]);
        let view = build_with_land(
            Some(&rules),
            &state,
            &["fixture".to_owned()],
            &land,
            test_date(),
        );
        let place = view.place("fixture").expect("the place is in the view");
        assert_eq!((place.workers, place.dependents), (80, 40));
        // Food: 1,000 oats and 5 more in the pop's storehouse account, 1,800 hundredths each, for 120 people at 100 a day.
        assert_eq!(place.food_days, (1_005 * 1_800) / (120 * 100));
        // Storage: oats, ale and barley by volume; the pop's 720 L a head, a farm's 100 L per worker, one storehouse level.
        let volume = |good: &str, units: u64| rules.goods[index_of(&rules, good)].volume_ml * units;
        assert_eq!(
            place.storage_used_ml,
            volume("oats", 1_000) + volume("ale", 10) + volume("barley", 20) + volume("oats", 5)
        );
        assert_eq!(
            place.storage_capacity_ml,
            120 * 720_000 + 25 * 100_000 + 36_000_000
        );
        assert_eq!(place.overflow_lost, 0);
        assert_eq!(place.footprint_m2, 6_200);
        assert_eq!(place.prepared_m2, 24_200);
        assert_eq!(place.max_buildable_m2, 23_000);
        assert_eq!(place.development_pm, 1000);
        assert_eq!(place.farmland_quality_pm, 550);
        assert_eq!(place.field_room, 0);
        assert_eq!(place.prices.len(), rules.goods.len());
        assert_eq!(place.prices[0].good, "oats");
        assert_eq!(
            (place.prices[0].price_milli, place.prices[0].base),
            (100_000, 100)
        );

        let pop = &place.pops[0];
        assert_eq!(
            (
                pop.kind.as_str(),
                pop.culture.as_str(),
                pop.religion.as_str()
            ),
            ("peasant", "ulaid", "christianity")
        );
        // Owning adds no employment; the farm has 25 workers in its job.
        assert_eq!((pop.employed, pop.unemployed), (25, 55));
        assert_eq!(pop.food_days, place.food_days);
        assert_eq!(
            (
                pop.necessities,
                pop.niceties,
                pop.luxuries,
                pop.satisfaction
            ),
            (700, 350, 100, 5)
        );
        // The equilibrium is the baseline the economy's monthly drift aims at for those needs (CK-378): the one function, not a second formula.
        assert_eq!(
            Some(pop.equilibrium),
            ck_economy::satisfaction_equilibrium(&rules, &state.places[0].pops[0])
        );
        assert!((-100..=100).contains(&pop.equilibrium));
        assert_eq!((pop.births, pop.deaths), (0, 0));

        let farm = &place.buildings[0];
        assert_eq!(farm.kind, "farm");
        assert_eq!(farm.methods, [("mixed_farming".to_owned(), 3)]);
        // 3 levels at the shipped farm's positions per level, 25 of them paid and filled.
        let per_level = u64::from(rules.buildings[0].methods[0].jobs[0].places);
        assert_eq!(rules.buildings[0].id, "farm");
        assert_eq!((farm.filled, farm.open), (25, 3 * per_level - 25));
        assert_eq!(
            farm.herds,
            [
                HerdView {
                    species: "cattle".to_owned(),
                    head: 27,
                    maximum: 36
                },
                HerdView {
                    species: "pigs".to_owned(),
                    head: 18,
                    maximum: 24
                },
                HerdView {
                    species: "sheep".to_owned(),
                    head: 15,
                    maximum: 21
                },
            ]
        );
        assert_eq!(farm.store_used_ml, volume("barley", 20));
        assert_eq!(farm.store_capacity_ml, 25 * 100_000);
        assert_eq!(farm.wage, 1_234);

        let storehouse = &place.buildings[1];
        assert_eq!(storehouse.levels, 1);
        assert!(storehouse.herds.is_empty());
        assert_eq!(storehouse.store_capacity_ml, 36_000_000);
    }

    #[test]
    fn displayed_building_and_place_capacity_uses_workers_only() {
        let rules = shipped_rules();
        let mut farm = fixture(&rules);
        farm.pops.clear();
        farm.buildings.truncate(1);
        farm.buildings[0].employment = vec![Employment {
            jobs: std::collections::BTreeMap::new(),

            workers: 10,
        }];
        farm.buildings[0].stock.fill(0);
        farm.buildings[0].stock[index_of(&rules, "oats")] = 10;
        let view = view_for_place(&rules, farm);
        assert_eq!(view.buildings[0].store_capacity_ml, 1_000_000);
        assert_eq!(view.storage_capacity_ml, 1_000_000);
        assert_eq!(view.storage_used_ml, 350_000);
        assert_eq!(
            ck_map::stockpile_value(view.storage_used_ml, view.storage_capacity_ml),
            350
        );

        let mut holdings = fixture(&rules);
        holdings.pops.clear();
        holdings.buildings.truncate(1);
        holdings.buildings[0].kind = "holdings".to_owned();
        holdings.buildings[0].employment = vec![Employment {
            jobs: std::collections::BTreeMap::new(),

            workers: 10,
        }];
        holdings.buildings[0].stock.fill(0);
        let view = view_for_place(&rules, holdings);
        assert_eq!(view.buildings[0].store_capacity_ml, 11_700_000);
        assert_eq!(view.storage_capacity_ml, 11_700_000);

        let mut owners_only = fixture(&rules);
        owners_only.pops.clear();
        owners_only.buildings.truncate(1);
        owners_only.buildings[0].employment = vec![Employment {
            jobs: std::collections::BTreeMap::new(),

            workers: 0,
        }];
        assert_eq!(
            view_for_place(&rules, owners_only).buildings[0].store_capacity_ml,
            0
        );

        let mut no_employees = fixture(&rules);
        no_employees.pops.clear();
        no_employees.buildings.truncate(1);
        no_employees.buildings[0].employment.clear();
        assert_eq!(
            view_for_place(&rules, no_employees).buildings[0].store_capacity_ml,
            0
        );

        let mut saturating = fixture(&rules);
        saturating.pops.clear();
        saturating.buildings.truncate(1);
        saturating.buildings[0].employment = vec![Employment {
            jobs: std::collections::BTreeMap::new(),

            workers: u64::MAX,
        }];
        assert_eq!(
            view_for_place(&rules, saturating).buildings[0].store_capacity_ml,
            u64::MAX
        );

        let mut invalid_worker_total = fixture(&rules);
        invalid_worker_total.pops.clear();
        invalid_worker_total.buildings.truncate(1);
        invalid_worker_total.buildings[0].employment = vec![
            Employment {
                jobs: std::collections::BTreeMap::new(),

                workers: u64::MAX,
            },
            Employment {
                jobs: std::collections::BTreeMap::new(),

                workers: 1,
            },
        ];
        assert_eq!(
            view_for_place(&rules, invalid_worker_total).buildings[0].store_capacity_ml,
            0
        );
    }

    #[test]
    fn fuel_stacks_do_not_change_displayed_storage_or_field_room() {
        let rules = shipped_rules();
        let plain = fixture(&rules);
        let before = view_for_place(&rules, plain.clone());
        let mut with_fuel = plain.clone();
        let hardwood = index_of(&rules, "hardwood");
        let peat = index_of(&rules, "peat");
        assert_eq!(
            (rules.goods[hardwood].volume_ml, rules.goods[peat].volume_ml),
            (100_000, 150_000)
        );
        with_fuel.pops[0].stock[hardwood] = 20;
        with_fuel.buildings[0].stock[peat] = 30;
        let after = view_for_place(&rules, with_fuel);
        assert_eq!(before.storage_used_ml, after.storage_used_ml);
        assert_eq!(
            before.buildings[0].store_used_ml,
            after.buildings[0].store_used_ml
        );
        let mut field_only = plain.clone();
        field_only.buildings[0].method_levels = vec![1, 0, 0, 0];
        let mut with_fuel_levels = field_only.clone();
        with_fuel_levels.buildings[0].method_levels = vec![1, 0, 1, 1];
        assert_eq!(
            field_room_for(&rules, field_only),
            field_room_for(&rules, with_fuel_levels)
        );
    }

    #[test]
    fn field_room_charges_only_land_method_levels() {
        let rules = shipped_rules();
        let mut mixed = fixture(&rules);
        mixed.buildings[0].levels = 5;
        mixed.buildings[0].method_levels = vec![3, 2];
        assert_eq!(field_room_for(&rules, mixed), 14);

        let mut farming = fixture(&rules);
        farming.buildings[0].levels = 5;
        farming.buildings[0].method_levels = vec![5, 0];
        assert_eq!(field_room_for(&rules, farming), 10);
    }

    #[test]
    fn field_room_uses_each_building_definition_land_cost() {
        let mut rules = shipped_rules();
        let mut custom = rules
            .buildings
            .iter()
            .find(|definition| definition.id == "farm")
            .expect("the farm definition exists")
            .clone();
        custom.id = "custom_farm".to_owned();
        rules.buildings.push(custom);
        let mut custom_farm = fixture(&rules);
        custom_farm.buildings[0].kind = "custom_farm".to_owned();
        assert_eq!(field_room_for(&rules, custom_farm), 14);

        let mut no_land = rules
            .buildings
            .iter()
            .find(|definition| definition.id == "farm")
            .expect("the farm definition exists")
            .clone();
        no_land.id = "custom_no_land".to_owned();
        no_land.land_per_level = 0;
        rules.buildings.push(no_land);
        let mut custom_zero = fixture(&rules);
        custom_zero.buildings[0].kind = "custom_no_land".to_owned();
        assert_eq!(field_room_for(&rules, custom_zero), 20);
    }

    #[test]
    fn a_place_of_nobody_and_a_place_the_state_lacks_are_zeroes_not_errors() {
        let rules = shipped_rules();
        let mut empty = fixture(&rules);
        empty.pops.clear();
        empty.buildings.clear();
        let state = EconomyState {
            next_pop_serial: 1,
            places: vec![empty],
            trade: TradeCounters::default(),
        };
        let ids = ["fixture".to_owned(), "absent".to_owned()];
        let view = build(Some(&rules), &state, &ids, test_date());
        assert_eq!(view.places.len(), 2);
        let fixture = view.place("fixture").unwrap();
        assert_eq!(
            (fixture.workers, fixture.dependents, fixture.food_days),
            (0, 0, 0)
        );
        assert_eq!(fixture.storage_capacity_ml, 0);
        assert!(fixture.pops.is_empty() && fixture.buildings.is_empty());
        assert_eq!(fixture.prices.len(), rules.goods.len());
        assert_eq!(
            view.place("absent"),
            Some(&PlaceView {
                id: "absent".to_owned(),
                ..PlaceView::default()
            })
        );
        assert_eq!(view.place("nowhere"), None);
        // No economy data at all (a scenario that predates M9): every place is nobody's.
        let none = build(None, &EconomyState::default(), &ids, test_date());
        assert_eq!(none.places.len(), 2);
        assert!(
            none.places
                .iter()
                .all(|place| place.workers == 0 && place.pops.is_empty())
        );
    }

    /// A view of places named by `used / capacity` stores and a projected yield (a place with an economy).
    fn view_of(places: &[(u64, u64, u64)]) -> EconomyView {
        EconomyView {
            places: places
                .iter()
                .enumerate()
                .map(|(index, (used, capacity, yield_))| PlaceView {
                    id: format!("place-{index}"),
                    storage_used_ml: *used,
                    storage_capacity_ml: *capacity,
                    projected_yield: *yield_,
                    has_economy: true,
                    ..PlaceView::default()
                })
                .collect(),
            ..EconomyView::default()
        }
    }

    #[test]
    fn the_stockpile_values_are_stored_volume_over_capacity_in_thousandths() {
        let view = view_of(&[
            (0, 1_000, 0),
            (500, 1_000, 0),
            (1_000, 1_000, 0),
            (1, 3, 0),
            (9, 0, 0),
        ]);
        assert_eq!(
            view.stockpile_values(),
            [0, 500, 1000, 333, ck_map::MODE_UNCOLOURED]
        );
        // A place of nobody has no capacity and so no colour; red (empty) is a colour.
        let mut nobody = view_of(&[(0, 1_000, 0)]);
        nobody.places.push(PlaceView {
            id: "absent".to_owned(),
            ..PlaceView::default()
        });
        assert_eq!(nobody.stockpile_values(), [0, ck_map::MODE_UNCOLOURED]);
    }

    #[test]
    fn the_yield_values_are_relative_across_the_places_that_have_an_economy() {
        let mut view = view_of(&[(0, 1, 100), (0, 1, 300), (0, 1, 200), (0, 1, 100)]);
        assert_eq!(view.yield_values(), [0, 1000, 500, 0]);
        // A place of nobody sets neither the least nor the most, and has no colour.
        view.places.push(PlaceView {
            id: "absent".to_owned(),
            projected_yield: 0,
            ..PlaceView::default()
        });
        assert_eq!(
            view.yield_values(),
            [0, 1000, 500, 0, ck_map::MODE_UNCOLOURED]
        );
        // Equal yields everywhere are the middle of the ramp.
        let level = view_of(&[(0, 1, 7), (0, 1, 7)]);
        assert_eq!(level.yield_values(), [500, 500]);
        // A place with an economy and no yield at all is the least, not nobody.
        let barren = view_of(&[(0, 1, 0), (0, 1, 40)]);
        assert_eq!(barren.yield_values(), [0, 1000]);
    }

    /// The fixture place's projected yield with `change` applied to its farm.
    fn yield_with(rules: &Rules, change: impl Fn(&mut BuildingState)) -> u64 {
        let mut place = fixture(rules);
        change(&mut place.buildings[0]);
        let state = EconomyState {
            next_pop_serial: 1,
            places: vec![place],
            trade: TradeCounters::default(),
        };
        build(Some(rules), &state, &["fixture".to_owned()], test_date()).places[0].projected_yield
    }

    fn yield_with_fertility(rules: &Rules, place: PlaceState, fertility_pm: Option<u16>) -> u64 {
        let state = EconomyState {
            next_pop_serial: 1,
            places: vec![place],
            trade: TradeCounters::default(),
        };
        let land = fertility_pm.map(|fertility_pm| {
            BTreeMap::from([(
                "fixture".to_owned(),
                LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 1)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), u64::from(fertility_pm))]),
                    fertile_cells: u32::from(fertility_pm > 0),
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 0,
                },
            )])
        });
        let view = match land.as_ref() {
            Some(land) => build_with_land(
                Some(rules),
                &state,
                &["fixture".to_owned()],
                land,
                test_date(),
            ),
            None => build(Some(rules), &state, &["fixture".to_owned()], test_date()),
        };
        view.places[0].projected_yield
    }

    #[test]
    fn herd_yield_saturates_for_large_authored_amounts_and_counts() {
        let mut rules = shipped_rules();
        rules.herd_species[0].products[0].amount_milli = Some(u64::MAX);
        let mut place = fixture(&rules);
        place.buildings[0].herds[0] = u64::MAX;
        assert_eq!(projected_yield(&rules, &place.buildings[0], 550), u64::MAX);
    }

    #[test]
    fn herd_rows_and_food_yield_follow_each_buildings_species() {
        let mut rules = shipped_rules();
        let mut goat = rules
            .goods
            .iter()
            .find(|g| g.id == "cattle")
            .unwrap()
            .clone();
        goat.id = "goats".into();
        rules.goods.push(goat);
        rules.herd_species.push(ck_economy::HerdSpecies {
            head_per_herder: 40,
            carcass_units: 6,
            animal: "goats".into(),
            births_pm_year: 0,
            deaths_pm_year: 0,
            emergency_order: 3,
            yields: vec![ck_economy::HerdYield {
                good: "hides".into(),
                units: 1,
            }],
            products: vec![ck_economy::HerdProduct {
                good: "oats".into(),
                amount_milli: Some(2500),
                season: None,
                month: Some(6),
            }],
        });
        let farm_definition = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        farm_definition.herds.retain(|id, _| id == "sheep");
        farm_definition
            .herds
            .insert("goats".into(), ck_economy::HerdCapacity { maximum: 13 });
        farm_definition.methods[0].annual_output =
            BTreeMap::from([("dairy".into(), 860), ("oats".into(), 100)]);
        let mut place = fixture(&rules);
        let farm = &mut place.buildings[0];
        farm.method_levels = vec![1, 0, 0, 0];
        farm.herds = vec![0, 0, 3, 0, 2];
        farm.method_suitability[0][index_of(&rules, "dairy")] = 700;
        farm.method_suitability[0][index_of(&rules, "oats")] = 1000;
        let view = building_view(&rules, farm, 0, test_date());
        assert_eq!(
            view.herds,
            [
                HerdView {
                    species: "sheep".into(),
                    head: 3,
                    maximum: 7
                },
                HerdView {
                    species: "goats".into(),
                    head: 2,
                    maximum: 13
                }
            ]
        );
        let dairy = u64::from(rules.goods[index_of(&rules, "dairy")].hunger_centi);
        let oats = u64::from(rules.goods[index_of(&rules, "oats")].hunger_centi);
        assert_eq!(projected_yield(&rules, farm, 550), 602 * dairy + 5 * oats);
        let mut holdings = farm.clone();
        holdings.kind = "holdings".into();
        holdings.method_levels = vec![1, 0];
        holdings.herds = vec![4, 0, 0, 0, 0];
        let other = building_view(&rules, &holdings, 0, test_date());
        assert_eq!(
            other.herds[0],
            HerdView {
                species: "cattle".into(),
                head: 4,
                maximum: 12
            }
        );
        assert_eq!(other.herds.len(), 3);
    }

    #[test]
    fn a_places_projected_yield_is_its_crops_and_fish_a_year_at_the_lands_suitability() {
        let rules = shipped_rules();
        let hunger = |good: &str| u64::from(rules.goods[index_of(&rules, good)].hunger_centi);
        // The fixture farm's herd is 27 cattle: its dairy is the whole yield while the land gives nothing.
        let dairy_only = 27 * 72 * hunger("dairy");
        assert_eq!(yield_with(&rules, |_| {}), dairy_only);
        // Two levels of oats at full suitability and one of barley at half.
        let crops = |farm: &mut BuildingState| {
            farm.method_suitability[0][index_of(&rules, "oats")] = 2_000;
            farm.method_suitability[0][index_of(&rules, "barley")] = 500;
        };
        let expected = 300 * 2_000 / 1_000 * hunger("oats") + 50 * 500 / 1_000 * hunger("barley");
        assert!(expected > 0);
        assert_eq!(yield_with(&rules, crops), dairy_only + expected);
        // A method with no levels adds nothing, whatever its suitability.
        assert_eq!(
            yield_with(&rules, |farm| {
                farm.method_suitability[1][index_of(&rules, "fish")] = 5_000;
            }),
            dairy_only
        );
    }

    #[test]
    fn projected_farming_yield_includes_fertility_with_one_floor_per_good_and_method() {
        let mut rules = shipped_rules();
        rules.fertility_reference_pm = 550;
        let oats_hunger = u64::from(rules.goods[index_of(&rules, "oats")].hunger_centi);
        let farm_rule = rules
            .buildings
            .iter_mut()
            .find(|building| building.id == "farm")
            .expect("the farm rule exists");
        farm_rule.methods[0].annual_output = BTreeMap::from([("oats".to_owned(), 1_000)]);

        let mut place = fixture(&rules);
        place.buildings[0].levels = 1;
        place.buildings[0].method_levels = vec![1, 0];
        place.buildings[0].herds = vec![0, 0, 0];
        place.buildings[0].method_suitability[0][index_of(&rules, "oats")] = 1_000;
        assert_eq!(
            yield_with_fertility(&rules, place.clone(), Some(825)),
            1_500 * oats_hunger
        );
        assert_eq!(
            yield_with_fertility(&rules, place.clone(), Some(275)),
            500 * oats_hunger
        );
        assert_eq!(
            yield_with_fertility(&rules, place.clone(), Some(551)),
            1_001 * oats_hunger
        );
        assert_eq!(yield_with_fertility(&rules, place.clone(), Some(0)), 0);
        assert_eq!(
            yield_with_fertility(&rules, place.clone(), None),
            1_000 * oats_hunger,
            "missing land uses the reference fertility"
        );

        // The annual × suitability × quality expression is floored once, rather than before quality is applied.
        rules
            .buildings
            .iter_mut()
            .find(|building| building.id == "farm")
            .expect("the farm rule exists")
            .methods[0]
            .annual_output = BTreeMap::from([("oats".to_owned(), 1)]);
        place.buildings[0].method_suitability[0][index_of(&rules, "oats")] = 999;
        assert_eq!(
            yield_with_fertility(&rules, place.clone(), Some(825)),
            oats_hunger,
            "floor once: 1 × 999 × 825 ÷ (1000 × 550) = 1"
        );

        // Sea output keeps its suitability-only rule; dairy stays tied to the herd at either quality.
        rules
            .buildings
            .iter_mut()
            .find(|building| building.id == "farm")
            .expect("the farm rule exists")
            .methods[0]
            .annual_output = BTreeMap::from([("oats".to_owned(), 1_000)]);
        place.buildings[0].method_suitability[0][index_of(&rules, "oats")] = 1_000;
        let farm_rule = rules
            .buildings
            .iter_mut()
            .find(|building| building.id == "farm")
            .expect("the farm rule exists");
        farm_rule.methods[1].resource = "sea".to_owned();
        farm_rule.methods[1].annual_output = BTreeMap::from([("oats".to_owned(), 1_000)]);
        place.buildings[0].method_levels = vec![1, 1];
        place.buildings[0].method_suitability[1][index_of(&rules, "oats")] = 1_000;
        place.buildings[0].herds[0] = 12;
        let dairy_hunger = u64::from(rules.goods[index_of(&rules, "dairy")].hunger_centi);
        let unchanged = 1_000 * oats_hunger + 12 * 72 * dairy_hunger;
        assert_eq!(
            yield_with_fertility(&rules, place.clone(), Some(825)),
            1_500 * oats_hunger + unchanged
        );
        assert_eq!(
            yield_with_fertility(&rules, place, Some(275)),
            500 * oats_hunger + unchanged
        );
    }

    #[test]
    fn fertility_changes_the_relative_yield_map_colours() {
        let mut rules = shipped_rules();
        let farm_rule = rules
            .buildings
            .iter_mut()
            .find(|building| building.id == "farm")
            .expect("the farm rule exists");
        farm_rule.methods[0].annual_output = BTreeMap::from([("oats".to_owned(), 1_000)]);
        let mut rich = fixture(&rules);
        rich.id = "rich".to_owned();
        rich.buildings[0].levels = 1;
        rich.buildings[0].method_levels = vec![1, 0];
        rich.buildings[0].herds = vec![0, 0, 0];
        rich.buildings[0].method_suitability[0][index_of(&rules, "oats")] = 1_000;
        let mut poor = rich.clone();
        poor.id = "poor".to_owned();
        let land = BTreeMap::from([
            (
                "rich".to_owned(),
                LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 1)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), 825)]),
                    fertile_cells: 1,
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 0,
                },
            ),
            (
                "poor".to_owned(),
                LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 1)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), 275)]),
                    fertile_cells: 1,
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 0,
                },
            ),
        ]);
        let view = build_with_land(
            Some(&rules),
            &EconomyState {
                next_pop_serial: 1,
                places: vec![rich, poor],
                trade: TradeCounters::default(),
            },
            &["rich".to_owned(), "poor".to_owned()],
            &land,
            test_date(),
        );
        assert_eq!(view.yield_values(), [1000, 0]);
    }

    #[test]
    fn dairy_counts_at_seventy_two_pounds_a_cow_a_year_and_follows_the_herd_not_the_land() {
        let rules = shipped_rules();
        let hunger = u64::from(rules.goods[index_of(&rules, "dairy")].hunger_centi);
        assert!(hunger > 0, "dairy is food");
        // One cow gives 72 pounds; the yield scales with the cattle and with nothing else of the herd.
        assert_eq!(
            yield_with(&rules, |farm| farm.herds = vec![1, 0, 0]),
            72 * hunger
        );
        assert_eq!(
            yield_with(&rules, |farm| farm.herds = vec![10, 18, 15]),
            720 * hunger
        );
        assert_eq!(
            yield_with(&rules, |farm| farm.herds = vec![0, 18, 15]),
            0,
            "pigs and sheep give no dairy"
        );
        // The land's suitability for dairy does not change it: the herd sets the dairy.
        assert_eq!(
            yield_with(&rules, |farm| {
                farm.herds = vec![5, 0, 0];
                farm.method_suitability[0][index_of(&rules, "dairy")] = 9_000;
            }),
            5 * 72 * hunger
        );
    }

    #[test]
    fn meat_does_not_count_toward_the_yield() {
        let rules = shipped_rules();
        let base = yield_with(&rules, |_| {});
        for meat in ["beef", "pork", "mutton"] {
            assert!(
                rules.goods[index_of(&rules, meat)].hunger_centi > 0,
                "{meat} is food"
            );
            // A method's output of the meat at any suitability adds nothing.
            assert_eq!(
                yield_with(&rules, |farm| {
                    farm.method_suitability[0][index_of(&rules, meat)] = 3_000;
                }),
                base,
                "{meat}"
            );
        }
        assert_eq!(
            yield_with(&rules, |farm| farm.herds = vec![27, 1_000, 1_000]),
            base,
            "pigs and sheep, the meat herds, add nothing"
        );
    }

    #[test]
    fn the_shipped_start_builds_a_view_for_every_ireland_place_without_overflow() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let directory = assets.join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("the scenario loads");
        let (state, _) = super::super::economy::starting_economy(&assets, &directory, &scenario)
            .expect("the start builds");
        let rules = shipped_rules();
        let ids: Vec<String> = scenario
            .locations
            .iter()
            .map(|location| location.id.clone())
            .collect();
        let view = build(Some(&rules), &state, &ids, test_date());
        assert_eq!(view.places.len(), 125);
        assert!(view.places.iter().any(|place| place.workers > 0));
        // The map modes: a value for every place, the stockpile ones on the ramp, and a spread of yields across the map.
        let stockpile = view.stockpile_values();
        let yields = view.yield_values();
        assert_eq!((stockpile.len(), yields.len()), (125, 125));
        assert!(
            stockpile
                .iter()
                .all(|value| *value <= 1000 || *value == ck_map::MODE_UNCOLOURED)
        );
        assert!(
            yields
                .iter()
                .all(|value| *value <= 1000 || *value == ck_map::MODE_UNCOLOURED)
        );
        assert!(
            yields.contains(&0) && yields.contains(&1000),
            "the least and the most yield are the ends of the ramp"
        );
        for place in &view.places {
            assert!(
                place.storage_used_ml <= place.storage_capacity_ml,
                "{}: the start fits its stores ({} > {})",
                place.id,
                place.storage_used_ml,
                place.storage_capacity_ml
            );
            for pop in &place.pops {
                assert!(
                    pop.unemployed <= pop.workers,
                    "{}: unemployed never exceed workers",
                    place.id
                );
            }
        }
    }

    #[test]
    fn serialized_economy_rebuilds_the_same_field_for_field_housing_view() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let directory = assets.join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("the scenario loads");
        let (state, _) = super::super::economy::starting_economy(&assets, &directory, &scenario)
            .expect("the start builds");
        let rules = shipped_rules();
        let ids: Vec<String> = scenario
            .locations
            .iter()
            .map(|location| location.id.clone())
            .collect();

        let view_at_save = build(Some(&rules), &state, &ids, test_date());
        let bytes = state.to_bytes().expect("the economy state serializes");
        let loaded = EconomyState::from_bytes(&bytes).expect("the economy state loads");
        assert_eq!(loaded, state, "saved counters and carries are unchanged");
        assert_eq!(
            build(Some(&rules), &loaded, &ids, test_date()),
            view_at_save,
            "a loaded state's derived housing fields match the view at the save boundary"
        );
    }
}
