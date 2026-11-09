//! The economy view (milestone 9, slice 3; design §6): an immutable, plain-data picture of every place's economy that the
//! simulation builds and the UI reads. The UI never reads the live state.
//!
//! A view is built from the state at three moments: at the start, after a load, and after each day's systems (the noon tick).
//! Each carries a revision that rises with every new view, including a load. The numbers this slice's state does not have
//! yet (the three needs, satisfaction, wages, monthly births, deaths and overflow losses) are `0` until the slices that
//! produce them.
//!
//! Everything here is simulation-side code: it lives in `game/` so that the build id covers it (a change to the display rules
//! is a change to what a save means to the UI, and the numbers it derives are checked by tests).

use ck_economy::{BuildingState, EconomyState, PlaceState, PopState, Rules};

/// A pop's store, in millilitres per person (design §3.15: 720 L).
const POP_STORE_ML_PER_PERSON: u64 = 720_000;
/// A farm's store, in millilitres per employee (design §3.15: 100 L).
const FARM_STORE_ML_PER_EMPLOYEE: u64 = 100_000;
/// The monastic holdings' store, in millilitres per employee (design §3.15: 1,170 L).
const HOLDINGS_STORE_ML_PER_EMPLOYEE: u64 = 1_170_000;
/// A storehouse level's store, in millilitres: 50 people's worth of a pop store (design §3.18).
const STOREHOUSE_STORE_ML_PER_LEVEL: u64 = 50 * POP_STORE_ML_PER_PERSON;
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
    /// The three needs, in ‰ (`0` until the eating slice).
    pub(crate) necessities: u64,
    pub(crate) niceties: u64,
    pub(crate) luxuries: u64,
    /// Satisfaction, −100 to +100 (`0` until the eating slice).
    pub(crate) satisfaction: i64,
    /// Last month's births and deaths (`0` until the births slice).
    pub(crate) births: u64,
    pub(crate) deaths: u64,
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
    /// The posted wage (`0` until the wage slice).
    pub(crate) wage: u64,
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
    /// Last month's losses to overflow (`0` until the payday slice).
    pub(crate) overflow_lost: u64,
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
}

/// The view of `state` for the scenario's `place_ids`, in that order. A place the state has no row for (a scenario with no
/// economy data, or one the data leaves out) is a place of nobody, with nothing in it. `rules` is `None` for a scenario with
/// no economy data, which has no rows either.
pub(crate) fn build(
    rules: Option<&Rules>,
    state: &EconomyState,
    place_ids: &[String],
) -> EconomyView {
    let places = place_ids
        .iter()
        .map(|id| {
            let row = state.places.iter().find(|place| &place.id == id);
            match (rules, row) {
                (Some(rules), Some(row)) => place_view(rules, row),
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
    let own = held(&place.pops[pop_index].stock);
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

fn pop_view(rules: &Rules, place: &PlaceState, index: usize) -> PopView {
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
        necessities: 0,
        niceties: 0,
        luxuries: 0,
        satisfaction: 0,
        births: 0,
        deaths: 0,
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

fn building_view(rules: &Rules, building: &BuildingState) -> BuildingView {
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
        wage: 0,
    }
}

fn place_view(rules: &Rules, place: &PlaceState) -> PlaceView {
    let pops: Vec<PopView> = (0..place.pops.len())
        .map(|index| pop_view(rules, place, index))
        .collect();
    let buildings: Vec<BuildingView> = place
        .buildings
        .iter()
        .map(|building| building_view(rules, building))
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
        overflow_lost: 0,
        pops,
        buildings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ck_economy::{Employment, StorehouseAccounts};
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
            pops: vec![PopState {
                kind: "peasant".to_owned(),
                culture: "ulaid".to_owned(),
                religion: "christianity".to_owned(),
                workers: 80,
                dependents: 40,
                employment: vec![Employment::default(); 2],
                stock,
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
                },
                BuildingState {
                    kind: "storehouse".to_owned(),
                    levels: 1,
                    method_levels: vec![1, 0, 0],
                    herds: Vec::new(),
                    employment: vec![Employment::default()],
                    stock: account,
                    storehouse_accounts: Some(accounts),
                },
            ],
            prices_milli: rules
                .goods
                .iter()
                .map(|good| good.base_price * 1000)
                .collect(),
        }
    }

    #[test]
    fn a_fixture_place_derives_every_number_the_panel_shows() {
        let rules = shipped_rules();
        let state = EconomyState {
            places: vec![fixture(&rules)],
        };
        let view = build(Some(&rules), &state, &["fixture".to_owned()]);
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
            (0, 0, 0, 0)
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
        assert_eq!(farm.wage, 0);

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
}
