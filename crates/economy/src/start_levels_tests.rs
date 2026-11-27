//! CK-404: a building's opening levels are filled by the place's needs, then its empty orders.
//!
//! One fixture serves the worked cases of the spec, in the units the start really uses. **Goods:**
//! oats, fish and beef, each with a hunger value of 365 hundredths a unit (a year's hunger of one
//! person is 36,500), base prices 2, 3 and 4; ale, a comfort with no hunger value, base price 5;
//! hardwood and peat, each supplying 365 thousandths of fuel a load. **A person** has a diet of 800
//! oats, 150 fish and 50 beef in the thousand and needs 4 thousandths of fuel a day. **A level a
//! year:** farming, the demesne's method and the plain holdings method make 900 oats and 100 beef;
//! fishing makes 800 fish; the wood focus makes 100 hardwood and 10 beef; turf cutting makes 100
//! peat and 10 beef. Fifty people unless a case says otherwise. Room is ample unless it says not.
//!
//! The cases reach the rule through `starting_state`: the order of a building's levels is read by
//! starting it with one more authored level at a time and seeing which method gained it.

use std::collections::BTreeMap;

use super::*;
use crate::tests::tiny_rules;

/// Ample room: levels of field room and sea cells.
const AMPLE: u32 = 400;

fn good(id: &str, hunger: u32, niceties: u32, base_price: u64, fuel: bool) -> Good {
    Good {
        id: id.to_owned(),
        tags: if fuel {
            vec!["fuel".to_owned()]
        } else {
            Vec::new()
        },
        supplies: if fuel {
            BTreeMap::from([("fuel".to_owned(), 365)])
        } else {
            BTreeMap::new()
        },
        unit: "unit".to_owned(),
        weight_g: 1,
        volume_ml: 1,
        stacked_outside: false,
        hunger_centi: hunger,
        necessities_centi: hunger,
        niceties_centi: niceties,
        luxuries_centi: 0,
        base_price,
        pop_limit_milli: 1000,
    }
}

/// A method whose yearly output, as the start works it out, is `outputs`. The fixture's land has
/// quality 4/3 of the reference, so a land method authors three quarters of each crop, rounded up
/// (the startup estimate scales herd goods by three quarters) and a sea
/// method its output as it is.
fn method(id: &str, resource: &str, serves: &str, outputs: &[(&str, u64)]) -> Method {
    let land = resource == "usable_land";
    let annual_output = outputs
        .iter()
        .map(|(good, units)| {
            // The startup estimate scales a herd good by three quarters: on land the 4/3 of the fixture's quality
            // cancels it, so it is authored as it is; at sea it has no quality, so it is authored a third more.
            let authored = if land && *good != "beef" {
                let authored = (units * 3).div_ceil(4);
                assert_eq!(authored * 4 / 3, *units, "{id}: {good} cannot be authored");
                authored
            } else if !land && *good == "beef" {
                let authored = (units * 4).div_ceil(3);
                assert_eq!(authored * 3 / 4, *units, "{id}: {good} cannot be authored");
                authored
            } else {
                *units
            };
            ((*good).to_owned(), authored)
        })
        .collect();
    Method {
        grows: false,
        jobs: Vec::new(),
        made_by: BTreeMap::new(),
        id: id.to_owned(),
        size_units: 5,
        resource: resource.to_owned(),
        cells_per_level: None,
        annual_output,
        suitability: if land { "crops" } else { "sea" }.to_owned(),
        score_share_pm: BTreeMap::from([(outputs[0].0.to_owned(), 1000)]),
        serves: serves.to_owned(),
    }
}

fn farming(id: &str) -> Method {
    method(id, "usable_land", "food", &[("oats", 900), ("beef", 100)])
}

fn fishing() -> Method {
    method("fishing", "sea", "food", &[("fish", 800)])
}

fn wood() -> Method {
    method("wood", "sea", "fuel", &[("hardwood", 100), ("beef", 10)])
}

fn turf() -> Method {
    method(
        "turf",
        "usable_land",
        "fuel",
        &[("peat", 100), ("beef", 10)],
    )
}

fn building(id: &str, mut methods: Vec<Method>) -> BuildingType {
    let job = |id: &str, places, kinds: &[&str], manager| ProductionJob {
        id: id.into(),
        places,
        pop_types: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
        wage_weight_pm: if manager { 1500 } else { 1000 },
        manager,
    };
    for method in &mut methods {
        method.jobs = match id {
            "holdings" => vec![
                job("prior", 1, &["monk"], true),
                job("monks", 5, &["monk"], false),
                job("field_hands", 5, &["labourer"], false),
            ],
            "demesne" => vec![
                job("steward", 1, &["noble", "labourer"], true),
                job("field_hands", 10, &["labourer"], false),
            ],
            _ => vec![job("peasant_labour", 10, &["peasant"], false)],
        };
    }
    BuildingType {
        jobs: if id == "storehouse" {
            vec![job("keepers", 2, &[], false)]
        } else {
            Vec::new()
        },
        id: id.to_owned(),
        church: id == "holdings",

        land_per_level: 2,

        penalty_pm: 500,

        labour_share_pm: 400,
        recruit_range_m: Some(30_000),
        market_capacity_ml_per_unit: 0,
        // The land buildings keep cattle, so the startup estimate applies the herd-good factor to beef.
        herds: if id == "storehouse" {
            BTreeMap::new()
        } else {
            BTreeMap::from([("cattle".to_owned(), HerdCapacity { maximum: 1 })])
        },
        methods,
    }
}

fn pop_type(id: &str, fuel_need_milli_day: u64) -> PopType {
    PopType {
        id: id.to_owned(),
        dependent_pm: 400,
        min_wage_pm: 0,
        births_pm_year: 67,
        worker_deaths_pm_year: 20,
        dependent_deaths_pm_year: 60,
        necessities_expectation_pm: 1000,
        niceties_expectation_pm: 0,
        luxuries_expectation_pm: 0,
        fuel_need_milli_day,
        fuel_heat_micro_per_degree_day: 0,
        fuel_comfort_niceties_pm: 0,
    }
}

/// The fixture's rules, with the given buildings (each after `farm` in the order given).
fn rules_with(buildings: Vec<BuildingType>, fuel_need_milli_day: u64) -> Rules {
    let mut rules = tiny_rules();
    rules.seasons.remove("food");
    for key in ["fish", "hardwood", "peat", "ale"] {
        rules
            .seasons
            .insert(key.to_owned(), vec![0, 0, 0, 0, 0, 0, 0, 0, 1000, 0, 0, 0]);
    }
    rules.goods = vec![
        good("oats", 365, 0, 2, false),
        good("fish", 365, 0, 3, false),
        good("beef", 365, 0, 4, false),
        good("ale", 0, 365, 5, false),
        good("hardwood", 0, 0, 1, true),
        good("peat", 0, 0, 1, true),
        good("cattle", 0, 0, 10, false),
    ];
    rules.herd_species = vec![HerdSpecies {
        head_per_herder: 40,
        carcass_units: 6,
        animal: "cattle".to_owned(),
        births_pm_year: 0,
        deaths_pm_year: 0,
        emergency_order: 1,
        yields: vec![HerdYield {
            good: "beef".to_owned(),
            units: 1,
        }],
        products: Vec::new(),
    }];
    rules.grazing = BTreeMap::from([
        (
            "pasture".to_owned(),
            BTreeMap::from([("cattle".to_owned(), 1000)]),
        ),
        (
            "bog".to_owned(),
            BTreeMap::from([("cattle".to_owned(), 1000)]),
        ),
    ]);
    rules.fertility_reference_pm = 750;
    let kinds = ["peasant", "noble", "labourer", "monk"];
    rules.pop_types = kinds
        .iter()
        .map(|kind| pop_type(kind, fuel_need_milli_day))
        .collect();
    for kind in kinds {
        rules
            .starting_needs_pm
            .insert(kind.to_owned(), [1000, 0, 0]);
        rules.diet_profiles.insert(
            kind.to_owned(),
            BTreeMap::from([
                ("oats".to_owned(), 800),
                ("fish".to_owned(), 150),
                ("beef".to_owned(), 50),
            ]),
        );
        rules.comfort_profiles.insert(
            kind.to_owned(),
            BTreeMap::from([
                ("niceties".to_owned(), BTreeMap::new()),
                ("luxuries".to_owned(), BTreeMap::new()),
            ]),
        );
    }
    rules.suitability = BTreeMap::from([(
        "pasture".to_owned(),
        BTreeMap::from([("crops".to_owned(), 1000)]),
    )]);
    rules.footprint_m2_per_unit.clear();
    rules.housing_milli_per_unit.clear();
    rules.housing_class.retain(|id, _| id == "storehouse");
    for item in &buildings {
        rules.footprint_m2_per_unit.insert(item.id.clone(), 400);
        rules.housing_milli_per_unit.insert(item.id.clone(), 3400);
        rules
            .housing_class
            .insert(item.id.clone(), "basic".to_owned());
    }
    let storehouse = rules
        .buildings
        .iter()
        .find(|item| item.id == "storehouse")
        .unwrap()
        .clone();
    rules.buildings = buildings;
    rules.buildings.push(storehouse);
    rules
}

/// Ale is the whole of a peasant's niceties, which it expects at `expectation_pm`: a yearly order
/// of 500 ale from fifty people at 100.
fn with_ale_order(rules: &mut Rules, expectation_pm: u16) {
    rules.pop_types[0].niceties_expectation_pm = expectation_pm;
    rules.comfort_profiles.get_mut("peasant").unwrap().insert(
        "niceties".to_owned(),
        BTreeMap::from([("ale".to_owned(), 1000)]),
    );
}

fn land(field_levels: u32, sea: u32) -> LandCounts {
    let cells = field_levels * 2;
    LandCounts {
        land_cells: BTreeMap::from([("pasture".to_owned(), cells)]),
        fertility_pm: BTreeMap::from([("pasture".to_owned(), u64::from(cells) * 1000)]),
        fertile_cells: cells,
        fertile_cells_by_terrain: BTreeMap::from([("pasture".to_owned(), cells)]),
        sea,
    }
}

/// One place `fort` and how it starts: its people, its land and its authored levels.
#[derive(Clone)]
struct Place {
    people: u64,
    land: LandCounts,
    holdings: u32,
    demesne: u32,
    farm: u32,
    /// A noble house of this many people, with the demesne levels above (none when it is 0).
    house: Option<u64>,
}

impl Place {
    fn new(people: u64) -> Self {
        Self {
            people,
            land: land(AMPLE, AMPLE),
            holdings: 0,
            demesne: 0,
            farm: 0,
            house: None,
        }
    }

    fn data(&self) -> ScenarioEconomy {
        ScenarioEconomy {
            horses_always: Vec::new(),
            defaults: BTreeMap::from([(
                "ringfort".to_owned(),
                BTreeMap::from([(
                    "1".to_owned(),
                    StartDefaults {
                        peasants: self.people,
                        monks: 0,
                        labourers: 0,
                        farm_levels: self.farm,
                        holdings_levels: self.holdings,
                        storehouse_levels: 0,
                        pop_stock: BTreeMap::new(),
                        building_stock: BTreeMap::new(),
                    },
                )]),
            )]),
            places: BTreeMap::from([(
                "fort".to_owned(),
                StartOverride {
                    culture: Some("connachta".to_owned()),
                    religion: Some("christianity".to_owned()),
                    monk_religion: Some("christianity".to_owned()),
                    ..StartOverride::default()
                },
            )]),
            nobles: if self.demesne == 0 && self.house.is_none() {
                BTreeMap::new()
            } else {
                BTreeMap::from([(
                    "fort".to_owned(),
                    NobleStart {
                        culture: "connachta".to_owned(),
                        religion: "christianity".to_owned(),
                        people: self.house.unwrap_or(12),
                        demesne_levels: self.demesne,
                        labourers: u64::from(self.demesne) * 10,
                    },
                )])
            },
        }
    }

    fn start(&self, rules: &Rules) -> (EconomyState, Vec<String>) {
        starting_state(
            rules,
            &self.data(),
            &[StartLocation {
                id: "fort".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: self.land.clone(),
            }],
            9,
        )
        .unwrap()
    }

    /// The levels of each method of each producing building, by building.
    fn levels(&self, rules: &Rules) -> BTreeMap<String, Vec<u32>> {
        self.start(rules).0.places[0]
            .buildings
            .iter()
            .map(|item| (item.kind.clone(), item.method_levels.clone()))
            .collect()
    }

    /// The method of each of `kind`'s authored levels in order, as the first letter of its id and
    /// `x` for a level cut: each level is read by starting the place with one more authored level.
    fn sequence(&self, rules: &Rules, kind: &str, authored: u32) -> String {
        let methods = &rules
            .buildings
            .iter()
            .find(|item| item.id == kind)
            .unwrap()
            .methods;
        let mut sequence = String::new();
        let mut before = vec![0_u32; methods.len()];
        for level in 1..=authored {
            let mut place = self.clone();
            match kind {
                "holdings" => place.holdings = level,
                "demesne" => place.demesne = level,
                _ => place.farm = level,
            }
            let counts = place.levels(rules).get(kind).cloned().unwrap_or_default();
            let counts = if counts.is_empty() {
                vec![0; methods.len()]
            } else {
                counts
            };
            let gained = counts
                .iter()
                .zip(&before)
                .position(|(now, then)| now > then);
            sequence.push(match gained {
                Some(index) => letter(&methods[index].id),
                None => 'x',
            });
            before = counts;
        }
        sequence
    }
}

/// The spec's letter for a method.
fn letter(id: &str) -> char {
    match id {
        "farming" => 'F',
        "fishing" => 'S',
        "wood" => 'W',
        "turf" => 'T',
        "monastic" => 'M',
        "demesne_farming" => 'D',
        "pre" => 'P',
        other => panic!("no letter for {other}"),
    }
}

/// Place the levels of the given buildings directly, one after another, with one count of what
/// has been provided; return the method counts and the demand afterwards.
fn place_levels(
    rules: &Rules,
    pops: &[PopState],
    mut land_counts: LandCounts,
    chain: &[(&str, u32)],
) -> (Vec<Vec<u32>>, StartDemand) {
    let mut demand = StartDemand::new(rules, pops).unwrap();
    let mut remaining_sea = land_counts.sea;
    let quality = mean_fertility(&land_counts).unwrap();
    let mut counts = Vec::new();
    for (kind, wanted) in chain {
        let item = rules
            .buildings
            .iter()
            .find(|item| item.id == *kind)
            .unwrap();
        counts.push(
            assign_building_levels(
                rules,
                item,
                *wanted,
                &mut land_counts,
                &mut remaining_sea,
                &mut demand,
                quality,
            )
            .unwrap()
            .method_levels,
        );
    }
    (counts, demand)
}

fn peasants(rules: &Rules, people: u64) -> Vec<PopState> {
    vec![
        make_pop(
            rules,
            "peasant",
            "connachta",
            "christianity",
            people,
            &BTreeMap::new(),
        )
        .unwrap(),
    ]
}

/// What the levels placed provide of each good, oats, fish, beef, ale, hardwood and peat in that order.
fn supply(rules: &Rules, demand: &StartDemand) -> [u128; 6] {
    ["oats", "fish", "beef", "ale", "hardwood", "peat"]
        .map(|id| demand.provided_goods()[units(rules, id)])
}

fn farm_of(methods: Vec<Method>, fuel: u64) -> Rules {
    rules_with(vec![building("farm", methods)], fuel)
}

fn units(rules: &Rules, id: &str) -> usize {
    rules.goods.iter().position(|item| item.id == id).unwrap()
}

// ---------------------------------------------------------------- needs and orders

#[test]
fn the_fixture_people_need_and_order_what_the_spec_states() {
    let rules = farm_of(vec![farming("farming")], 4);
    let demand = StartDemand::new(&rules, &peasants(&rules, 50)).unwrap();
    assert_eq!(demand.food_need(), 1_825_000);
    assert_eq!(demand.fuel_need(), 73_000);
    let mut orders = vec![0_u128; rules.goods.len()];
    orders[units(&rules, "oats")] = 4000;
    orders[units(&rules, "fish")] = 750;
    orders[units(&rules, "beef")] = 250;
    assert_eq!(demand.orders(), orders);
    // A food's order is the existing yearly projection.
    for food in ["oats", "fish", "beef"] {
        assert_eq!(
            demand.orders()[units(&rules, food)],
            projected_buy_units(&rules, &peasants(&rules, 50), food).unwrap(),
            "{food}"
        );
    }
}

#[test]
fn fuel_need_is_zero_without_data_and_shipped_at_twenty() {
    let rules = farm_of(vec![farming("farming")], 0);
    let demand = StartDemand::new(&rules, &peasants(&rules, 50)).unwrap();
    assert_eq!(demand.fuel_need(), 0);
    let shipped =
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
            .unwrap();
    assert!(
        shipped
            .pop_types
            .iter()
            .all(|kind| kind.fuel_need_milli_day == 20)
    );
    let fuel_methods: Vec<_> = shipped
        .buildings
        .iter()
        .flat_map(|b| &b.methods)
        .filter(|m| m.serves == "fuel")
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(fuel_methods, ["wood_focus", "turf_cutting"]);
}

#[test]
fn a_comfort_is_ordered_for_a_year_of_the_tier_rule_and_the_larger_tier_counts() {
    let mut rules = farm_of(vec![farming("farming")], 0);
    with_ale_order(&mut rules, 100);
    let pops = peasants(&rules, 50);
    let demand = StartDemand::new(&rules, &pops).unwrap();
    let ale = units(&rules, "ale");
    assert_eq!(demand.orders()[ale], 500);
    assert_eq!(
        demand.orders()[ale],
        tier_target_units_for_days(&rules, "peasant", 50, ale, 1, 365).unwrap()
    );
    // The live market's own target is for 240 days, and is unchanged.
    assert_eq!(
        tier_target_units(&rules, "peasant", 50, ale, 1).unwrap(),
        329
    );
    // Where a good is in both tiers, the larger amount is ordered, not the sum.
    rules.pop_types[0].luxuries_expectation_pm = 300;
    rules.goods[ale].luxuries_centi = 365;
    rules.comfort_profiles.get_mut("peasant").unwrap().insert(
        "luxuries".to_owned(),
        BTreeMap::from([("ale".to_owned(), 1000)]),
    );
    let demand = StartDemand::new(&rules, &pops).unwrap();
    assert_eq!(
        demand.orders()[ale],
        1500,
        "the luxuries amount is the larger"
    );
}

#[test]
fn two_pops_order_a_good_by_each_pops_larger_amount_and_the_place_adds_them() {
    // Fifty people whose diet gives a food 100‰ with no comfort for it, and fifty others with
    // none of that food in their diet whose niceties are all of it.
    let mut rules = farm_of(vec![farming("farming")], 0);
    rules.goods.push(good("bread", 365, 100, 2, false));
    rules.pop_types[1].niceties_expectation_pm = 100;
    rules.pop_types[0].niceties_expectation_pm = 0;
    rules.diet_profiles.insert(
        "peasant".to_owned(),
        BTreeMap::from([("oats".to_owned(), 900), ("bread".to_owned(), 100)]),
    );
    rules.diet_profiles.insert(
        "noble".to_owned(),
        BTreeMap::from([("oats".to_owned(), 1000)]),
    );
    rules.comfort_profiles.get_mut("noble").unwrap().insert(
        "niceties".to_owned(),
        BTreeMap::from([("bread".to_owned(), 1000)]),
    );
    let pops = vec![
        make_pop(
            &rules,
            "peasant",
            "connachta",
            "christianity",
            50,
            &BTreeMap::new(),
        )
        .unwrap(),
        make_pop(
            &rules,
            "noble",
            "connachta",
            "christianity",
            50,
            &BTreeMap::new(),
        )
        .unwrap(),
    ];
    let bread = units(&rules, "bread");
    assert_eq!(
        projected_pop_order_units(&rules, &pops[0], bread).unwrap(),
        500
    );
    assert_eq!(
        projected_pop_order_units(&rules, &pops[1], bread).unwrap(),
        1825
    );
    let demand = StartDemand::new(&rules, &pops).unwrap();
    assert_eq!(
        demand.orders()[bread],
        2325,
        "not the larger of the sums, 1,825"
    );
}

#[test]
fn a_method_must_serve_food_or_fuel() {
    let mut rules = farm_of(vec![farming("farming")], 0);
    rules.validate("fixture").unwrap();
    rules.buildings[0].methods[0].serves = "ghost".to_owned();
    let error = rules.validate("fixture/economy.toml").unwrap_err();
    assert_eq!(error.entry, "buildings[0].methods.farming.serves");
    assert_eq!(error.problem, "must be `food` or `fuel`");
}

// ---------------------------------------------------------------- the worked cases

#[test]
fn case_a_fuel_and_food_share_the_levels_by_need() {
    let rules = farm_of(vec![farming("farming"), wood()], 4);
    let place = Place::new(50);
    assert_eq!(place.sequence(&rules, "farm", 10), "FWFFWFFWFF");
    assert_eq!(
        place.sequence(&rules, "farm", 40),
        "FWFFWFFWFF".to_owned() + "FWFFWFFFWF" + "FWFFFWFFWF" + "FFWFFWFFFW"
    );
    let (counts, demand) = place_levels(
        &rules,
        &peasants(&rules, 50),
        land(AMPLE, AMPLE),
        &[("farm", 40)],
    );
    assert_eq!(counts, [[28, 12]]);
    // Seven farming levels meet both needs; the eighth level goes to fuel.
    let (_, demand_ten) = place_levels(
        &rules,
        &peasants(&rules, 50),
        land(AMPLE, AMPLE),
        &[("farm", 10)],
    );
    assert_eq!(demand_ten.provided_goods()[units(&rules, "oats")], 6300);
    assert_eq!(demand_ten.provided_goods()[units(&rules, "beef")], 730);
    assert_eq!(demand_ten.provided_goods()[units(&rules, "hardwood")], 300);
    assert_eq!(demand.provided_goods()[units(&rules, "hardwood")], 1200);
    // Forty levels: 28 farming make 25,200 oats and 2,800 beef, 12 wood 120 more beef and 1,200 hardwood.
    assert_eq!(supply(&rules, &demand), [25_200, 0, 2920, 0, 1200, 0]);
    assert_eq!(supply(&rules, &demand_ten), [6300, 0, 730, 0, 300, 0]);
}

#[test]
fn case_a_wood_and_turf_share_the_fuel_levels_and_a_place_without_wood_room_cuts_turf() {
    // The wood focus is a sea method here and turf cutting a land one, so the sea cells decide
    // whether wood has room: this base has no forest or bog resource.
    let rules = farm_of(vec![farming("farming"), wood(), turf()], 4);
    let with_wood = Place::new(50);
    assert_eq!(with_wood.sequence(&rules, "farm", 10), "FWFFTFFWFF");
    let without_wood = Place {
        land: land(AMPLE, 0),
        ..with_wood
    };
    assert_eq!(without_wood.sequence(&rules, "farm", 10), "FTFFTFFTFF");
    // Seven farming levels, and fuel from two wood and one turf, or from three turf where wood has no room.
    for (sea, expected) in [
        (AMPLE, [6300, 0, 730, 0, 200, 100]),
        (0, [6300, 0, 730, 0, 0, 300]),
    ] {
        let (_, demand) = place_levels(
            &rules,
            &peasants(&rules, 50),
            land(AMPLE, sea),
            &[("farm", 10)],
        );
        assert_eq!(supply(&rules, &demand), expected, "sea cells {sea}");
    }
}

#[test]
fn case_b_farming_and_fishing_take_turns_then_the_orders_decide() {
    let rules = farm_of(vec![farming("farming"), fishing()], 0);
    let place = Place::new(50);
    assert_eq!(place.sequence(&rules, "farm", 12), "FSFSFSFFSSFF");
    let (counts, _) = place_levels(
        &rules,
        &peasants(&rules, 50),
        land(AMPLE, AMPLE),
        &[("farm", 12)],
    );
    assert_eq!(counts, [[7, 5]]);
    // Seven farming levels make 6,300 oats and 700 beef, five fishing levels 4,000 fish.
    let (_, demand) = place_levels(
        &rules,
        &peasants(&rules, 50),
        land(AMPLE, AMPLE),
        &[("farm", 12)],
    );
    assert_eq!(supply(&rules, &demand), [6300, 4000, 700, 0, 0, 0]);
}

#[test]
fn case_c_no_fuel_need_means_no_fuel_level_and_a_by_product_is_not_a_reason() {
    let rules = farm_of(vec![farming("farming"), wood()], 0);
    let place = Place {
        land: land(2, AMPLE),
        ..Place::new(50)
    };
    // The wood focus has room and would make beef, and beef may be wanted: it is not taken.
    assert_eq!(place.sequence(&rules, "farm", 4), "FFxx");
    let report = Place {
        farm: 4,
        ..place.clone()
    }
    .start(&rules)
    .1;
    // The wood focus has room, so the cut is for want of a method that serves food or a need, not for want of land or sea.
    assert!(
        report.iter().any(|line| line.ends_with(
            "cut levels 4 -> 2 (a method has room, but none serves food or an unmet need)"
        )),
        "{report:?}"
    );
    let (_, demand) = place_levels(&rules, &peasants(&rules, 50), place.land, &[("farm", 4)]);
    assert_eq!(supply(&rules, &demand), [1800, 0, 200, 0, 0, 0]);
}

#[test]
fn a_cut_for_want_of_land_and_sea_is_reported_as_that() {
    // Farming is the only method and its field room is two levels: the two cut levels have no room, whatever they serve.
    let rules = farm_of(vec![farming("farming")], 0);
    let place = Place {
        land: land(2, 0),
        farm: 4,
        ..Place::new(50)
    };
    let (state, report) = place.start(&rules);
    let farm = state.places[0]
        .buildings
        .iter()
        .find(|building| building.kind == "farm")
        .unwrap();
    assert_eq!(farm.units, 10);
    assert_eq!(
        building_footprint(&rules, std::slice::from_ref(farm)).unwrap(),
        4000
    );
    assert!(
        report
            .iter()
            .any(|line| line.ends_with("cut levels 4 -> 2 (no method has enough land or sea)")),
        "{report:?}"
    );
}

fn holdings_of(methods: Vec<Method>) -> BuildingType {
    building("holdings", methods)
}

#[test]
fn case_d_the_count_of_what_is_provided_carries_from_building_to_building() {
    let rules = rules_with(
        vec![
            holdings_of(vec![farming("monastic")]),
            building("farm", vec![farming("farming"), wood()]),
        ],
        4,
    );
    let place = Place {
        holdings: 2,
        ..Place::new(50)
    };
    assert_eq!(place.sequence(&rules, "holdings", 2), "MM");
    assert_eq!(place.sequence(&rules, "farm", 8), "WFWFFWFF");
    // Seven farming levels, two of them the holdings', make 6,300 oats; the three wood levels add 30 beef to their 700.
    let (_, demand) = place_levels(
        &rules,
        &peasants(&rules, 50),
        land(AMPLE, AMPLE),
        &[("holdings", 2), ("farm", 8)],
    );
    assert_eq!(supply(&rules, &demand), [6300, 0, 730, 0, 300, 0]);
}

#[test]
fn case_e_an_empty_place_is_filled_by_the_fallback() {
    let rules = farm_of(vec![farming("farming"), fishing()], 4);
    let place = Place {
        land: land(2, AMPLE),
        ..Place::new(0)
    };
    assert_eq!(place.sequence(&rules, "farm", 3), "FFS");
    let (_, demand) = place_levels(&rules, &[], land(2, AMPLE), &[("farm", 3)]);
    assert_eq!(supply(&rules, &demand), [1800, 800, 200, 0, 0, 0]);
}

#[test]
fn case_f_a_noble_seat_counts_only_its_authored_pops_and_sizes_its_household_from_placed_levels() {
    let rules = rules_with(
        vec![
            building("demesne", vec![farming("demesne_farming")]),
            building("farm", vec![farming("farming")]),
        ],
        4,
    );
    let place = Place {
        land: land(5, AMPLE),
        demesne: 2,
        ..Place::new(50)
    };
    // The place's needs and orders are those of the fifty peasants alone.
    let demand = StartDemand::new(&rules, &peasants(&rules, 50)).unwrap();
    assert_eq!(
        (demand.food_need(), demand.fuel_need()),
        (1_825_000, 73_000)
    );
    assert_eq!(place.sequence(&rules, "demesne", 2), "DD");
    assert_eq!(place.sequence(&rules, "farm", 4), "FFFx");
    // Five farming levels in all, the demesne's two among them.
    let (_, demand) = place_levels(
        &rules,
        &peasants(&rules, 50),
        land(5, AMPLE),
        &[("demesne", 2), ("farm", 4)],
    );
    assert_eq!(supply(&rules, &demand), [4500, 0, 500, 0, 0, 0]);
    let household = |place: &Place| {
        let (state, _) = place.start(&rules);
        let pops = &state.places[0].pops;
        let demesne = state.places[0]
            .buildings
            .iter()
            .find(|item| item.kind == "demesne")
            .unwrap()
            .levels;
        let labourers = pops
            .iter()
            .filter(|pop| pop.kind == "labourer")
            .map(|pop| pop.workers + pop.dependents)
            .sum::<u64>();
        (demesne, labourers)
    };
    let whole = Place {
        farm: 4,
        ..place.clone()
    };
    let (levels, labourers) = household(&whole);
    assert_eq!(levels, 2);
    assert_eq!(labourers, labourer_household_heads(&rules, 20).unwrap());
    // With field room for one demesne level it is sized from one.
    let narrow = Place {
        land: land(1, AMPLE),
        farm: 4,
        ..place
    };
    let (levels, labourers) = household(&narrow);
    assert_eq!(levels, 1);
    assert_eq!(labourers, labourer_household_heads(&rules, 10).unwrap());
}

#[test]
fn case_g_the_method_that_fills_more_of_what_is_ordered_wins_whatever_its_order_or_size() {
    let holdings = |fish: u64| {
        holdings_of(vec![method(
            "pre",
            "sea",
            "food",
            &[("oats", 3700), ("beef", 800), ("fish", fish)],
        )])
    };
    for (fish, winner, provided) in [
        (650, "farming", [4600_u128, 650, 900]),
        (500, "fishing", [3700, 1300, 800]),
    ] {
        let rules = rules_with(
            vec![
                holdings(fish),
                building("farm", vec![fishing(), farming("farming")]),
            ],
            0,
        );
        let pops = peasants(&rules, 50);
        let (counts, demand) = place_levels(
            &rules,
            &pops,
            land(AMPLE, AMPLE),
            &[("holdings", 1), ("farm", 1)],
        );
        // The holdings feeds everyone (5,150 or 4,500 units of food, in hundredths 365 each).
        let level = if winner == "fishing" { [1, 0] } else { [0, 1] };
        assert_eq!(counts, [vec![1], level.to_vec()], "{winner}");
        let place = Place {
            holdings: 1,
            ..Place::new(50)
        };
        assert_eq!(
            place.sequence(&rules, "farm", 1),
            letter(winner).to_string(),
            "{winner}"
        );
        let got = ["oats", "fish", "beef"].map(|id| demand.provided_goods()[units(&rules, id)]);
        assert_eq!(got, provided, "{winner}: oats, fish and beef provided");
    }
}

#[test]
fn case_h_a_comfort_order_can_decide_the_last_levels() {
    let mut rules = rules_with(
        vec![holdings_of(vec![
            method(
                "monastic",
                "usable_land",
                "food",
                &[("oats", 900), ("beef", 100), ("ale", 50)],
            ),
            fishing(),
        ])],
        0,
    );
    // The fifty people expect niceties at 100‰, ale is the whole of them, and a unit of ale is
    // worth 365 hundredths of niceties: a yearly order of 500 ale.
    with_ale_order(&mut rules, 100);
    let place = Place {
        holdings: 10,
        ..Place::new(50)
    };
    assert_eq!(place.sequence(&rules, "holdings", 10), "MSMSMSMMMM");
    let (_, demand) = place_levels(
        &rules,
        &peasants(&rules, 50),
        land(AMPLE, AMPLE),
        &[("holdings", 10)],
    );
    assert_eq!(supply(&rules, &demand), [6300, 2400, 700, 350, 0, 0]);
    // With nobody ordering ale the same holdings gives M S M S M S M M S S.
    with_ale_order(&mut rules, 0);
    assert_eq!(place.sequence(&rules, "holdings", 10), "MSMSMSMMSS");
    let (_, demand) = place_levels(
        &rules,
        &peasants(&rules, 50),
        land(AMPLE, AMPLE),
        &[("holdings", 10)],
    );
    assert_eq!(supply(&rules, &demand), [4500, 4000, 500, 250, 0, 0]);
}

#[test]
fn case_i_a_raise_that_placed_a_level_carries_to_the_next_building() {
    let rules = rules_with(
        vec![
            holdings_of(vec![fishing()]),
            building("farm", vec![fishing(), farming("farming")]),
        ],
        0,
    );
    let place = Place {
        holdings: 8,
        ..Place::new(50)
    };
    assert_eq!(place.sequence(&rules, "holdings", 8), "SSSSSSSS");
    // The farm starts from the raised need, 2,336,000 of 3,650,000 provided.
    assert_eq!(place.sequence(&rules, "farm", 3), "SFS");
    let (_, demand) = place_levels(
        &rules,
        &peasants(&rules, 50),
        land(AMPLE, AMPLE),
        &[("holdings", 8)],
    );
    assert_eq!(demand.provided_need(), (2_336_000, 0));
    assert_eq!(demand.target_need(), (3_650_000, 0));
    // Ten fishing levels (eight the holdings', two the farm's) and one farming level in all.
    let (_, demand) = place_levels(
        &rules,
        &peasants(&rules, 50),
        land(AMPLE, AMPLE),
        &[("holdings", 8), ("farm", 3)],
    );
    assert_eq!(supply(&rules, &demand), [900, 8000, 100, 0, 0, 0]);
}

#[test]
fn case_j_a_raise_that_ends_in_a_cut_is_undone_so_the_next_building_is_not_overfed() {
    let rules = rules_with(
        vec![
            holdings_of(vec![wood()]),
            building("farm", vec![fishing(), farming("farming")]),
        ],
        1,
    );
    let place = Place {
        holdings: 5,
        ..Place::new(25)
    };
    // The first level meets the fuel need (9,125) four times over; the rest are cut.
    assert_eq!(place.sequence(&rules, "holdings", 5), "Wxxxx");
    // Three levels feed the twenty-five; the last two go to farming by the oats order. Had the
    // failed raise been kept, the farm would be working to two years' food and its fifth level
    // would be fishing.
    assert_eq!(place.sequence(&rules, "farm", 5), "SFSFF");
    let (_, demand) = place_levels(
        &rules,
        &peasants(&rules, 25),
        land(AMPLE, AMPLE),
        &[("holdings", 5)],
    );
    assert_eq!(
        demand.target_need(),
        (912_500, 9_125),
        "the raise was undone"
    );
    // Three farming levels, two fishing, and the one wood level: 2,700 oats, 1,600 fish, 310 beef, 100 hardwood.
    let (_, demand) = place_levels(
        &rules,
        &peasants(&rules, 25),
        land(AMPLE, AMPLE),
        &[("holdings", 5), ("farm", 5)],
    );
    assert_eq!(supply(&rules, &demand), [2700, 1600, 310, 0, 100, 0]);
}

#[test]
fn fuel_has_no_opening_order_even_where_a_comfort_profile_names_it() {
    // Nobody needs fuel, but the peasants expect niceties at 100‰ and hardwood, a fuel good, is the whole of them: the daily market
    // would order it, the opening does not. A holdings level feeds everyone and fills every food order; the farm's one level is then
    // fishing (the first food method, after the year's raise), not farming for the hardwood that farming also makes.
    let mut rules = rules_with(
        vec![
            holdings_of(vec![method(
                "pre",
                "sea",
                "food",
                &[("oats", 5000), ("beef", 800), ("fish", 800)],
            )]),
            building(
                "farm",
                vec![
                    fishing(),
                    method(
                        "farming",
                        "usable_land",
                        "food",
                        &[("oats", 900), ("beef", 100), ("hardwood", 100)],
                    ),
                ],
            ),
        ],
        0,
    );
    let hardwood = units(&rules, "hardwood");
    rules.goods[hardwood].niceties_centi = 365;
    rules.pop_types[0].niceties_expectation_pm = 100;
    rules.comfort_profiles.get_mut("peasant").unwrap().insert(
        "niceties".to_owned(),
        BTreeMap::from([("hardwood".to_owned(), 1000)]),
    );
    let demand = StartDemand::new(&rules, &peasants(&rules, 50)).unwrap();
    assert_eq!(demand.orders()[units(&rules, "hardwood")], 0);
    // The daily market's own order for it is unchanged.
    assert_eq!(
        tier_target_units(&rules, "peasant", 50, units(&rules, "hardwood"), 1).unwrap(),
        329
    );
    let place = Place {
        holdings: 1,
        ..Place::new(50)
    };
    assert_eq!(place.sequence(&rules, "farm", 1), "S");
}

#[test]
fn needs_are_compared_exactly_through_the_start_when_their_products_overflow_128_bits() {
    // Twenty million people each need 10^18 thousandths of fuel a day: a year's fuel need of 7.3 x 10^27 and a year's food need of
    // 7.3 x 10^11. One fishing level provides half the food and none of the fuel; the cross products of the two shares are near
    // 2.7 x 10^39, beyond `u128`, so the comparison is exact in any size. The second level goes to the less covered need, fuel.
    let rules = farm_of(
        vec![
            method("fishing", "sea", "food", &[("fish", 1_000_000_000)]),
            wood(),
        ],
        1_000_000_000_000_000_000,
    );
    let place = Place::new(20_000_000);
    assert_eq!(place.sequence(&rules, "farm", 2), "SW");
}

#[test]
fn a_noble_counted_early_would_change_the_levels_so_it_is_not() {
    // The noble house's people need ten times the fuel of a peasant, so counting its twelve people when the levels are placed would
    // ask for more wood than the fifty peasants alone. The start places the farm's twenty levels for the peasants only, and adds the
    // house afterwards (the demesne's labourer household, when there is a demesne, the same).
    let mut rules = rules_with(
        vec![
            building("demesne", vec![farming("demesne_farming")]),
            building("farm", vec![farming("farming"), wood()]),
        ],
        4,
    );
    rules
        .pop_types
        .iter_mut()
        .find(|kind| kind.id == "noble")
        .unwrap()
        .fuel_need_milli_day = 40;
    let authored = peasants(&rules, 50);
    let mut with_noble = authored.clone();
    with_noble.push(
        make_pop(
            &rules,
            "noble",
            "connachta",
            "christianity",
            12,
            &BTreeMap::new(),
        )
        .unwrap(),
    );
    let wood_levels =
        |pops: &[PopState]| place_levels(&rules, pops, land(AMPLE, AMPLE), &[("farm", 20)]).0[0][1];
    let (peasants_only, counted_early) = (wood_levels(&authored), wood_levels(&with_noble));
    assert!(
        counted_early > peasants_only,
        "the fixture must tell the two apart: {peasants_only} and {counted_early} wood levels"
    );
    let place = Place {
        farm: 20,
        house: Some(12),
        ..Place::new(50)
    };
    let started = place.levels(&rules);
    assert_eq!(
        started["farm"],
        [20 - peasants_only, peasants_only],
        "the farm is placed for the authored peasants alone"
    );
}

#[test]
fn a_fuel_method_is_never_taken_for_the_food_it_also_makes() {
    // Nothing here serves food but a fuel method that makes plenty of it.
    let rules = farm_of(
        vec![method(
            "wood",
            "sea",
            "fuel",
            &[("hardwood", 100), ("oats", 4000), ("beef", 1000)],
        )],
        0,
    );
    let place = Place::new(50);
    assert_eq!(place.sequence(&rules, "farm", 3), "xxx");
}

// ---------------------------------------------------------------- the shipped rules

#[test]
fn the_shipped_rules_start_every_level_on_a_method_or_cut_and_report_it() {
    let rules =
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
            .unwrap();
    let farm = rules
        .buildings
        .iter()
        .find(|item| item.id == "farm")
        .unwrap();
    let place = Place {
        land: land(AMPLE, 0),
        farm: 10,
        ..Place::new(50)
    };
    // With no sea cell only the land method has room.
    let counts = place.levels(&rules);
    assert_eq!(farm.methods[0].id, "mixed_farming");
    assert_eq!(counts["farm"][..2], [10, 0]);
}

#[test]
fn opening_places_only_five_unit_methods() {
    let mut small = farming("small");
    small.size_units = 1;
    let rules = farm_of(vec![small, farming("farming")], 4);
    let (counts, _) = place_levels(&rules, &peasants(&rules, 50), land(8, 0), &[("farm", 2)]);
    assert_eq!(counts[0], vec![0, 2]);
}

#[test]
fn method_size_is_required_and_positive() {
    let mut rules = farm_of(vec![farming("farming")], 4);
    rules.buildings[0].methods[0].size_units = 0;
    assert!(
        rules
            .validate("fixture")
            .unwrap_err()
            .entry
            .ends_with("size_units")
    );

    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
    )
    .unwrap();
    let missing = source.replacen("size_units=5\n", "", 1);
    assert!(toml::from_str::<Rules>(&missing).is_err());
}

#[test]
fn resource_free_method_uses_no_ground_or_cells() {
    let mut processing = farming("processing");
    processing.resource = "none".into();
    let mut rules = farm_of(vec![processing], 4);
    rules.validate("fixture").unwrap();
    let method = &rules.buildings[0].methods[0];
    assert_eq!(
        method_suitability_pm(&rules, &rules.buildings[0], method, "oats", None),
        1000
    );
    let candidates =
        level_candidates(&rules, &rules.buildings[0], &land(0, 0), 0, 1000, &[0]).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].terrain, None);
    let shipped =
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
            .unwrap();
    let farm = shipped
        .buildings
        .iter()
        .find(|building| building.id == "farm")
        .unwrap();
    let mut no_resource = farm.methods[0].clone();
    no_resource.resource = "none".into();
    assert!(herd_supplies(&shipped, farm, "beef"));
    assert_eq!(
        method_suitability_pm(&shipped, farm, &no_resource, "beef", None),
        1000
    );
    rules.buildings[0].methods[0].cells_per_level = Some(1);
    assert!(
        rules
            .validate("fixture")
            .unwrap_err()
            .entry
            .ends_with("cells_per_level")
    );
}
