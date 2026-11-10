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
use std::collections::BTreeMap;

/// A pop's store, in millilitres per person (design §3.15: 720 L).
const POP_STORE_ML_PER_PERSON: u64 = 720_000;
/// A farm's store, in millilitres per employee (design §3.15: 100 L).
const FARM_STORE_ML_PER_EMPLOYEE: u64 = 100_000;
/// The monastic holdings' store, in millilitres per employee (design §3.15: 1,170 L).
const HOLDINGS_STORE_ML_PER_EMPLOYEE: u64 = 1_170_000;
/// A storehouse level's store, in millilitres: 50 people's worth of a pop store (design §3.18).
const STOREHOUSE_STORE_ML_PER_LEVEL: u64 = 50 * POP_STORE_ML_PER_PERSON;
/// The goods a herd gives (design §3.14): they follow the herd, not the land, so a method's annual output at the land's suitability
/// does not count them. Dairy counts through the herd (see [`DAIRY_LB_PER_COW_A_YEAR`]); the meats do not count at all (the owner's
/// rulings of 2026-11-09: herd is not food, but dairy counts toward the yield and the meat does not); wool and hides are not food.
const HERD_GOODS: [&str; 6] = ["beef", "pork", "mutton", "dairy", "wool", "hides"];
/// What a cow gives a year at full coverage (design §3.14: dairy is `cattle × 72` pounds, set by the herd's size).
const DAIRY_LB_PER_COW_A_YEAR: u64 = 72;
/// The species of a building's herd, in the order the state holds them, with each one's maximum head per farming level
/// (design §3.14).
const HERDS: [(&str, u64); 3] = [("cattle", 12), ("pigs", 8), ("sheep", 7)];

/// One good's price at a place against its base price.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PriceView {
    /// The good's key.
    pub(crate) good: String,
    /// The local price in thousandths of a value unit.
    pub(crate) price_milli: u64,
    /// The base price in whole value units.
    pub(crate) base: u64,
}

/// One pop, as the Pops tab shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PopView {
    pub(crate) kind: String,
    pub(crate) culture: String,
    pub(crate) religion: String,
    pub(crate) workers: u64,
    pub(crate) dependents: u64,
    /// Workers holding a place in a building, owners included.
    pub(crate) employed: u64,
    /// Workers with no place.
    pub(crate) unemployed: u64,
    /// Whole days of food held (the store and the storehouse accounts) at a full daily need.
    pub(crate) food_days: u64,
    /// The three needs, in ‰.
    pub(crate) necessities: u64,
    pub(crate) niceties: u64,
    pub(crate) luxuries: u64,
    /// Satisfaction, −100 to +100.
    pub(crate) satisfaction: i64,
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
    pub(crate) kind: String,
    pub(crate) levels: u64,
    /// The levels of each method that has any, in method order.
    pub(crate) methods: Vec<(String, u64)>,
    /// Paid positions held, and positions open.
    pub(crate) filled: u64,
    pub(crate) open: u64,
    pub(crate) herds: Vec<HerdView>,
    pub(crate) store_used_ml: u64,
    pub(crate) store_capacity_ml: u64,
    /// The mean of the building's last twelve monthly wages in thousandths.
    pub(crate) wage: u64,
    /// Occupants assigned to this building by the pure housing fill.
    pub(crate) housing_occupants: u64,
}

/// One place's economy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlaceView {
    pub(crate) id: String,
    pub(crate) workers: u64,
    pub(crate) dependents: u64,
    /// Whole days of food, the place's pops together.
    pub(crate) food_days: u64,
    pub(crate) storage_used_ml: u64,
    pub(crate) storage_capacity_ml: u64,
    pub(crate) prices: Vec<PriceView>,
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

/// Every place's view, in scenario-file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EconomyView {
    pub(crate) places: Vec<PlaceView>,
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
) -> EconomyView {
    build_with_land(rules, state, place_ids, &BTreeMap::new())
}

pub(crate) fn build_with_land(
    rules: Option<&Rules>,
    state: &EconomyState,
    place_ids: &[String],
    land_by_place: &BTreeMap<String, LandCounts>,
) -> EconomyView {
    let places = place_ids
        .iter()
        .map(|id| {
            let row = state.places.iter().find(|place| &place.id == id);
            match (rules, row) {
                (Some(rules), Some(row)) => place_view(rules, row, land_by_place.get(id)),
                _ => PlaceView {
                    id: id.clone(),
                    ..PlaceView::default()
                },
            }
        })
        .collect();
    EconomyView { places }
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
            sum.saturating_add(units.saturating_mul(good.volume_ml))
        })
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
            sum.saturating_add(employment.owners)
                .saturating_add(employment.workers)
        });
    PopView {
        kind: pop.kind.clone(),
        culture: pop.culture.clone(),
        religion: pop.religion.clone(),
        workers: pop.workers,
        dependents: pop.dependents,
        employed,
        unemployed: pop.workers.saturating_sub(employed),
        food_days: days_of(food_centi(rules, place, index), size_of(pop)),
        necessities: u64::from(pop.consumption.necessities_pm),
        niceties: u64::from(pop.consumption.niceties_pm),
        luxuries: u64::from(pop.consumption.luxuries_pm),
        satisfaction: i64::from(pop.consumption.satisfaction),
        births: pop.counters.last.births,
        deaths: pop.counters.last.deaths,
        basic_housing: housing.map_or(0, |housing| housing.basic),
        makeshift_housing: housing.map_or(0, |housing| housing.makeshift),
        crowding_pm: housing.and_then(|housing| housing.crowding_pm),
    }
}

fn building_capacity_ml(building: &BuildingState, employees: u64) -> u64 {
    match building.kind.as_str() {
        "farm" => employees.saturating_mul(FARM_STORE_ML_PER_EMPLOYEE),
        "holdings" => employees.saturating_mul(HOLDINGS_STORE_ML_PER_EMPLOYEE),
        "storehouse" => u64::from(building.levels).saturating_mul(STOREHOUSE_STORE_ML_PER_LEVEL),
        _ => 0,
    }
}

/// A building's projected annual food, in hundredths of a person-day: for each method with levels, each food good that is not a herd's
/// at its annual output scaled by the land's suitability (`method_suitability` is the sum over the levels, in ‰ of the output at
/// suitability 1000), and the dairy of the building's cattle (72 pounds a cow a year). The meats do not count.
fn projected_yield(rules: &Rules, building: &BuildingState) -> u64 {
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
            if HERD_GOODS.contains(&good_id.as_str()) {
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
            let units = u128::from(*annual) * u128::from(suitability) / 1000;
            total = total.saturating_add(units * u128::from(good.hunger_centi));
        }
    }
    // Dairy follows the herd, whatever the methods: the cattle (the first species) at 72 pounds a head.
    if let (Some(cattle), Some(dairy)) = (
        building.herds.first(),
        rules.goods.iter().find(|good| good.id == "dairy"),
    ) {
        total = total.saturating_add(
            u128::from(*cattle)
                * u128::from(DAIRY_LB_PER_COW_A_YEAR)
                * u128::from(dairy.hunger_centi),
        );
    }
    u64::try_from(total).unwrap_or(u64::MAX)
}

fn building_view(rules: &Rules, building: &BuildingState, housing_occupants: u64) -> BuildingView {
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
    let positions = definition.map_or(0, |definition| {
        u64::from(building.levels).saturating_mul(u64::from(definition.workers_per_level))
    });
    let (filled, employees) =
        building
            .employment
            .iter()
            .fold((0_u64, 0_u64), |(filled, employees), employment| {
                (
                    filled.saturating_add(employment.workers),
                    employees
                        .saturating_add(employment.owners)
                        .saturating_add(employment.workers),
                )
            });
    // A herd's maximum grows with the farming levels: every method that is not fishing (the same count that sets the start).
    let farming_levels = definition.map_or(0, |definition| {
        definition
            .methods
            .iter()
            .zip(&building.method_levels)
            .filter(|(method, _)| method.resource != "sea")
            .fold(0_u64, |sum, (_, levels)| {
                sum.saturating_add(u64::from(*levels))
            })
    });
    let herds = HERDS
        .iter()
        .zip(&building.herds)
        .map(|((species, per_level), head)| HerdView {
            species: (*species).to_owned(),
            head: *head,
            maximum: farming_levels.saturating_mul(*per_level),
        })
        .collect();
    BuildingView {
        kind: building.kind.clone(),
        levels: u64::from(building.levels),
        methods,
        filled,
        open: positions.saturating_sub(filled),
        herds,
        store_used_ml: volume_ml(rules, &building.stock),
        store_capacity_ml: building_capacity_ml(building, employees),
        wage: building.posted_wage(),
        housing_occupants,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "A place view keeps its immutable economy totals, derived land values and per-building projections together."
)]
fn place_view(rules: &Rules, place: &PlaceState, land: Option<&LandCounts>) -> PlaceView {
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
        .map(|(good, price)| PriceView {
            good: good.id.clone(),
            price_milli: *price,
            base: good.base_price,
        })
        .collect();
    PlaceView {
        id: place.id.clone(),
        workers,
        dependents,
        food_days: days_of(food, workers.saturating_add(dependents)),
        storage_used_ml: pops_used.saturating_add(buildings_used),
        storage_capacity_ml: capacity,
        prices,
        overflow_lost: place.counters.last.losses,
        projected_yield: place.buildings.iter().fold(0_u64, |sum, building| {
            sum.saturating_add(projected_yield(rules, building))
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
                let occupied = place
                    .buildings
                    .iter()
                    .filter(|building| matches!(building.kind.as_str(), "farm" | "holdings"))
                    .filter_map(|building| {
                        rules
                            .buildings
                            .iter()
                            .find(|definition| definition.id == building.kind)
                    })
                    .fold(0_u64, |sum, definition| {
                        sum.saturating_add(
                            u64::from(definition.land_per_level).saturating_mul(u64::from(
                                place
                                    .buildings
                                    .iter()
                                    .find(|building| building.kind == definition.id)
                                    .map_or(0, |building| building.levels),
                            )),
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
        CounterBuckets, Employment, NearbyPlace, StorehouseAccounts, run_daily_economy_with_context,
    };
    use ck_log::Level;
    use ck_sim::{Canonical, Date, GameTime, Save, Scheduler, Simulation};
    use std::collections::BTreeMap;
    use std::path::Path;

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
    /// 10 ale, a farm of 3 levels with 10 owners and 25 workers employed, and a storehouse level holding 5 oats for the pop's account.
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
            coastal: false,
            counters: CounterBuckets::default(),
            pops: vec![PopState {
                kind: "peasant".to_owned(),
                culture: "ulaid".to_owned(),
                religion: "christianity".to_owned(),
                literacy_pm: 0,
                workers: 80,
                dependents: 40,
                employment: vec![Employment::default(); 2],
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
            }],
            buildings: vec![
                BuildingState {
                    kind: "farm".to_owned(),
                    levels: 3,
                    method_levels: vec![3, 0],
                    herds: vec![27, 18, 15],
                    employment: vec![Employment {
                        owners: 10,
                        workers: 25,
                    }],
                    stock: farm_stock,
                    storehouse_accounts: None,
                    production_carries: vec![vec![0; goods]; 2],
                    method_suitability: vec![vec![0; goods]; 2],
                    herd_birth_carries: vec![0; 3],
                    herd_death_carries: vec![0; 3],
                    herd_product_carries: vec![0; goods],
                    herd_grazing_sums: vec![0; 3],
                    wage_carries: vec![0; goods],
                    pay_carries: vec![0; goods],
                    fee_carries: Vec::new(),
                    wage_history: vec![1_234; 12],
                },
                BuildingState {
                    kind: "storehouse".to_owned(),
                    levels: 1,
                    method_levels: vec![1, 0, 0],
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
                    fee_carries: vec![vec![0; goods]; 2],
                    wage_history: vec![0; 12],
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
        }
    }

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
        holdings.herds = vec![0; 3];
        holdings.employment.clear();
        holdings.stock = vec![0; goods];
        holdings.production_carries = vec![vec![0; goods]; 2];
        holdings.method_suitability = vec![vec![0; goods]; 2];
        holdings.herd_birth_carries = vec![0; 3];
        holdings.herd_death_carries = vec![0; 3];
        holdings.herd_product_carries = vec![0; goods];
        holdings.herd_grazing_sums = vec![0; 3];
        holdings.wage_carries = vec![0; goods];
        holdings.pay_carries = vec![0; goods];
        holdings.fee_carries.clear();
        holdings.wage_history = vec![100; 12];

        let mut state = EconomyState {
            places: vec![dying, source, destination],
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
        let views = Box::new(move |game: &crate::game::GameState| {
            build(Some(&rules), &game.economy, &view_ids)
        });
        let game_state = crate::game::GameState {
            economy: state.clone(),
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
        assert_eq!(
            saved.state.economy, state,
            "the quicksave retains the post-tick state"
        );

        core.dispatch(
            crate::game::core::Message::Control(crate::game::controls::Control::Quickload),
            std::time::Duration::ZERO,
        );
        let view_after_load = core
            .snapshot()
            .economy
            .expect("quickload rebuilds and publishes the app view");
        assert_eq!(
            *view_before_save, *view_after_load,
            "the complete view is field-for-field stable"
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
                places: vec![place],
            },
            &["fixture".to_owned()],
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
        let view = build(Some(&rules), &state, &ids);
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
    fn a_fixture_place_derives_every_number_the_panel_shows() {
        let rules = shipped_rules();
        let state = EconomyState {
            places: vec![fixture(&rules)],
        };
        let land = BTreeMap::from([(
            "fixture".to_owned(),
            LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 10)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 5_500)]),
                fertile_cells: 10,
                sea: 0,
            },
        )]);
        let view = build_with_land(Some(&rules), &state, &["fixture".to_owned()], &land);
        let place = view.place("fixture").expect("the place is in the view");
        assert_eq!((place.workers, place.dependents), (80, 40));
        // Food: 1,000 oats and 5 more in the pop's storehouse account, 1,800 hundredths each, for 120 people at 100 a day.
        assert_eq!(place.food_days, (1_005 * 1_800) / (120 * 100));
        // Storage: oats, ale and barley by volume; the pop's 720 L a head, a farm's 100 L an employee, one storehouse level.
        let volume = |good: &str, units: u64| rules.goods[index_of(&rules, good)].volume_ml * units;
        assert_eq!(
            place.storage_used_ml,
            volume("oats", 1_000) + volume("ale", 10) + volume("barley", 20) + volume("oats", 5)
        );
        assert_eq!(
            place.storage_capacity_ml,
            120 * 720_000 + 35 * 100_000 + 36_000_000
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
        // The farm's table employs 10 owners and 25 workers of the pop: 35 are employed.
        assert_eq!((pop.employed, pop.unemployed), (35, 45));
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
        assert_eq!((pop.births, pop.deaths), (0, 0));

        let farm = &place.buildings[0];
        assert_eq!(farm.kind, "farm");
        assert_eq!(farm.methods, [("mixed_farming".to_owned(), 3)]);
        // 3 levels at the shipped farm's positions per level, 25 of them paid and filled.
        let per_level = u64::from(rules.buildings[0].workers_per_level);
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
        assert_eq!(farm.store_capacity_ml, 35 * 100_000);
        assert_eq!(farm.wage, 1_234);

        let storehouse = &place.buildings[1];
        assert_eq!(storehouse.levels, 1);
        assert!(storehouse.herds.is_empty());
        assert_eq!(storehouse.store_capacity_ml, 36_000_000);
    }

    #[test]
    fn a_place_of_nobody_and_a_place_the_state_lacks_are_zeroes_not_errors() {
        let rules = shipped_rules();
        let mut empty = fixture(&rules);
        empty.pops.clear();
        empty.buildings.clear();
        let state = EconomyState {
            places: vec![empty],
        };
        let ids = ["fixture".to_owned(), "absent".to_owned()];
        let view = build(Some(&rules), &state, &ids);
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
        let none = build(None, &EconomyState::default(), &ids);
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
            places: vec![place],
        };
        build(Some(rules), &state, &["fixture".to_owned()]).places[0].projected_yield
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
        let view = build(Some(&rules), &state, &ids);
        assert_eq!(view.places.len(), 94);
        assert!(view.places.iter().any(|place| place.workers > 0));
        // The map modes: a value for every place, the stockpile ones on the ramp, and a spread of yields across the map.
        let stockpile = view.stockpile_values();
        let yields = view.yield_values();
        assert_eq!((stockpile.len(), yields.len()), (94, 94));
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

        let view_at_save = build(Some(&rules), &state, &ids);
        let bytes = state.to_bytes().expect("the economy state serializes");
        let loaded = EconomyState::from_bytes(&bytes).expect("the economy state loads");
        assert_eq!(loaded, state, "saved counters and carries are unchanged");
        assert_eq!(
            build(Some(&rules), &loaded, &ids),
            view_at_save,
            "a loaded state's derived housing fields match the view at the save boundary"
        );
    }
}
