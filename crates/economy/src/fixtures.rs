//! The fixture places and rules of the economy's own tests (CK-391, CK-433), shared with the application's tests of what reads them: a
//! place of up to three pops with the shipped prices and a diet that gives each a target for oats, barley, fish and honey, helpers that
//! set a pop's spare or lack against its real target, and the trade world of a trader and a destination. Hidden from the documentation:
//! it is for tests, the economy's own and the application's.

#![expect(
    missing_docs,
    clippy::must_use_candidate,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test fixtures: a failure to build one is a failing test"
)]

use std::{collections::BTreeMap, path::Path};

use super::{
    CounterBuckets, EconomyState, HerdCapacity, PlaceState, PlaceTrade, PopConsumption,
    PopGenetics, PopState, Rules, TradeCounters, Vehicle, fuel, pop_target_units,
};

pub const PEASANT: usize = 0;
pub const MONK: usize = 1;
pub const THIRD: usize = 2;

pub fn shipped() -> Rules {
    Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
        .unwrap()
}

pub fn good(rules: &Rules, id: &str) -> usize {
    rules.goods.iter().position(|item| item.id == id).unwrap()
}

/// The shipped rules with every pop kind on one diet and comfort profile, so each pop has a target for oats, barley, fish and honey
/// (food) and for herbs (a comfort good with no hunger value), and with room for the stores unless a case narrows it.
pub fn rules() -> Rules {
    let mut rules = shipped();
    let diet = BTreeMap::from([
        ("oats".to_owned(), 400),
        ("barley".to_owned(), 200),
        ("fish".to_owned(), 200),
        ("honey".to_owned(), 200),
    ]);
    for kind in ["peasant", "monk", "labourer", "noble"] {
        rules.diet_profiles.insert(kind.to_owned(), diet.clone());
        rules.comfort_profiles.insert(
            kind.to_owned(),
            BTreeMap::from([
                (
                    "niceties".to_owned(),
                    BTreeMap::from([("herbs".to_owned(), 1000)]),
                ),
                ("luxuries".to_owned(), BTreeMap::new()),
            ]),
        );
    }
    for item in &mut rules.pop_types {
        if matches!(item.id.as_str(), "peasant" | "monk" | "labourer" | "noble") {
            item.niceties_expectation_pm = 500;
        }
    }
    for item in &mut rules.goods {
        item.pop_limit_milli = 1_000_000_000;
    }
    rules.validate("fixture").unwrap();
    rules
}

/// A place of `pops` pops (peasants, monks, then labourers) of 1000 people each, with prices as the base prices.
pub fn place(rules: &Rules, pops: usize) -> PlaceState {
    let goods = rules.goods.len();
    let kinds = ["peasant", "monk", "labourer"];
    PlaceState {
        id: "fixture".to_owned(),
        pops: (0..pops)
            .map(|index| PopState {
                id: 0,
                kind: kinds[index].to_owned(),
                culture: "gaelic".to_owned(),
                religion: "christianity".to_owned(),
                literacy_pm: 0,
                workers: 600,
                dependents: 400,
                employment: Vec::new(),
                stock: vec![0; goods],
                consumption: PopConsumption {
                    pantry_hunger_centi: vec![0; goods],
                    ticking_carries: vec![0; goods],
                    ..PopConsumption::default()
                },
                counters: CounterBuckets::default(),
                short_on_rent: false,
                housed_last: false,
                genetics: PopGenetics::default(),
                pending_births: 0,
                pending_deaths: 0,
            })
            .collect(),
        buildings: Vec::new(),
        prices_milli: rules
            .goods
            .iter()
            .map(|item| item.base_price * 1000)
            .collect(),
        prepared_m2: 0,
        development_pm: 0,
        price_drift_carries: vec![0; goods],
        last_market_orders: None,
        counters: CounterBuckets::for_place(goods),
        trade: PlaceTrade::default(),
    }
}

pub fn target(rules: &Rules, place: &PlaceState, pop: usize, id: &str) -> u64 {
    let people = place.pops[pop].workers + place.pops[pop].dependents;
    u64::try_from(pop_target_units(rules, &place.pops[pop].kind, people, good(rules, id)).unwrap())
        .unwrap()
}

/// Sets a pop's store so it holds `spare` above its target of a good.
pub fn spare(rules: &Rules, place: &mut PlaceState, pop: usize, id: &str, units: u64) {
    place.pops[pop].stock[good(rules, id)] = target(rules, place, pop, id) + units;
}

/// Sets a pop's store so it lacks `units` of its target of a good.
pub fn want(rules: &Rules, place: &mut PlaceState, pop: usize, id: &str, units: u64) {
    let goal = target(rules, place, pop, id);
    assert!(
        goal >= units,
        "{id}: target {goal} is below the want {units}"
    );
    place.pops[pop].stock[good(rules, id)] = goal - units;
}

pub fn set_price(rules: &Rules, place: &mut PlaceState, id: &str, price: u64) {
    place.prices_milli[good(rules, id)] = price * 1000;
}

/// Puts every good a case did not set at each pop's target, so it is neither spare nor wanted.
pub fn settle_the_rest(rules: &Rules, place: &mut PlaceState, set: &[&str]) {
    let fuel_is_set = rules
        .goods
        .iter()
        .any(|item| item.category_supplied("fuel", 1) > 0 && set.contains(&item.id.as_str()));
    for pop in 0..place.pops.len() {
        for item in &rules.goods {
            if !set.contains(&item.id.as_str()) {
                place.pops[pop].stock[good(rules, &item.id)] = target(rules, place, pop, &item.id);
            }
        }
        if !fuel_is_set {
            let heat = fuel::fuel_daily_need(rules, &place.pops[pop]).unwrap() * 240;
            let (index, supply) = rules
                .goods
                .iter()
                .enumerate()
                .map(|(i, good)| (i, good.category_supplied("fuel", 1)))
                .max_by_key(|(_, supply)| *supply)
                .unwrap();
            if supply > 0 {
                place.pops[pop].stock[index] = u64::try_from(heat / supply).unwrap();
                place.pops[pop].consumption.hearth_milli = u64::try_from(heat % supply).unwrap();
            }
        }
    }
}

/// The case's rules: the farm keeps only cattle, ten head a level, so a one-level herd's maximum is 10 and its standard level 7 (two
/// thirds would be 6); an animal has no pop store, as shipped (its limit is zero). Oats are a food, herbs a comfort.
pub fn herd_rules() -> Rules {
    let mut rules = rules();
    for item in &mut rules.goods {
        if item.tags.iter().any(|tag| tag == "herd") {
            item.pop_limit_milli = 0;
        }
    }
    for building in &mut rules.buildings {
        building.herds.clear();
    }
    rules
        .buildings
        .iter_mut()
        .find(|building| building.id == "farm")
        .unwrap()
        .herds
        .insert("cattle".to_owned(), HerdCapacity { maximum: 10 });
    rules.validate("fixture").unwrap();
    rules
}

/// The rules of the cases: looks on, the vehicles the case names (none are built that it does not name).
pub fn rules_with(vehicles: &[&str]) -> Rules {
    let mut rules = herd_rules();
    rules.expedition.looks = true;
    rules
        .vehicles
        .retain(|vehicle| vehicles.contains(&vehicle.id.as_str()));
    rules
}

/// A home place whose peasant wants `fish` fish and holds `barley` barley spare, nothing else lacking. Its prices are one.
pub fn home_place(rules: &Rules, id: &str, fish: u64, barley: u64) -> PlaceState {
    let mut home = place(rules, 3);
    // The peasant has a free vehicle of the first kind the case's rules hold (the ox-cart in the common rules).
    if !rules.vehicles.is_empty() {
        home.trade.vehicles.push(Vehicle {
            number: 1,
            kind: 0,
            pop: u32::try_from(PEASANT).unwrap(),
            free: true,
            materials: None,
        });
    }
    id.clone_into(&mut home.id);
    for item in ["oats", "barley"] {
        set_price(rules, &mut home, item, 1);
    }
    // Home is short of fish and dearer for it than the destination, so a trip that brings fish for barley gains.
    set_price(rules, &mut home, "fish", 4);
    settle_the_rest(rules, &mut home, &[]);
    want(rules, &mut home, PEASANT, "fish", fish);
    spare(rules, &mut home, PEASANT, "barley", barley);
    home
}

/// A destination whose monks have `fish` fish to spare and want `barley` barley.
pub fn dest_place(rules: &Rules, id: &str, fish: u64, barley: u64) -> PlaceState {
    let mut dest = place(rules, 3);
    id.clone_into(&mut dest.id);
    for item in ["oats", "barley", "fish"] {
        set_price(rules, &mut dest, item, 1);
    }
    settle_the_rest(rules, &mut dest, &[]);
    spare(rules, &mut dest, MONK, "fish", fish);
    want(rules, &mut dest, MONK, "barley", barley);
    dest
}

pub fn world(places: Vec<PlaceState>) -> EconomyState {
    let vehicles = places
        .iter()
        .map(|place| place.trade.vehicles.len() as u64)
        .sum();
    EconomyState {
        places,
        trade: TradeCounters {
            vehicles,
            ..TradeCounters::default()
        },
        next_pop_serial: 1,
    }
}
