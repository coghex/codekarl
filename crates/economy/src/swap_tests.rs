//! CK-391 (trade slice 3a): the daily swap between two pops of one place.
//!
//! Each case is one place with fixture rules and prices set by the fixture. A pop's spare and want are set against its real target, so
//! the literals of the spec's worked cases are the pairs and stores asserted. None sleeps and none reads a counter added for it.

use std::{collections::BTreeMap, path::Path};

use super::swap::{Position, market_capacities, pop_position, want_order};
use super::*;

const PEASANT: usize = 0;
const MONK: usize = 1;
const THIRD: usize = 2;

fn shipped() -> Rules {
    Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
        .unwrap()
}

pub(crate) fn good(rules: &Rules, id: &str) -> usize {
    rules.goods.iter().position(|item| item.id == id).unwrap()
}

/// The shipped rules with every pop kind on one diet and comfort profile, so each pop has a target for oats, barley, fish and honey
/// (food) and for herbs (a comfort good with no hunger value), and with room for the stores unless a case narrows it.
pub(crate) fn rules() -> Rules {
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

/// The profiles of the hunger cases: every pop eats fish alone, and wants herbs and beeswax for comfort, so a pop's days of food are
/// its pounds of fish over ten (six people need 600 centi a day, a pound of fish gives 60).
fn hunger_rules() -> Rules {
    let mut rules = rules();
    for kind in ["peasant", "monk", "labourer", "noble"] {
        rules
            .diet_profiles
            .insert(kind.to_owned(), BTreeMap::from([("fish".to_owned(), 1000)]));
        rules.comfort_profiles.insert(
            kind.to_owned(),
            BTreeMap::from([
                (
                    "niceties".to_owned(),
                    BTreeMap::from([("herbs".to_owned(), 500), ("beeswax".to_owned(), 500)]),
                ),
                ("luxuries".to_owned(), BTreeMap::new()),
            ]),
        );
    }
    rules.validate("fixture").unwrap();
    rules
}

/// Every pop of the place has `people` people, all workers.
fn small(place: &mut PlaceState, people: u64) {
    for pop in &mut place.pops {
        pop.workers = people;
        pop.dependents = 0;
    }
}

fn building_state(rules: &Rules, kind: &str, levels: u32, pops: usize) -> BuildingState {
    let goods = rules.goods.len();
    let methods = rules
        .buildings
        .iter()
        .find(|item| item.id == kind)
        .unwrap()
        .methods
        .len();
    BuildingState {
        kind: kind.to_owned(),
        levels,
        method_levels: vec![0; methods],
        level_records: Vec::new(),
        herds: Vec::new(),
        employment: vec![Employment::default(); pops],
        stock: vec![0; goods],
        storehouse_accounts: None,
        production_carries: vec![vec![0; goods]; methods],
        method_suitability: vec![vec![0; goods]; methods],
        herd_birth_carries: Vec::new(),
        herd_death_carries: Vec::new(),
        herd_product_carries: Vec::new(),
        herd_grazing_sums: Vec::new(),
        wage_carries: vec![0; goods],
        pay_carries: vec![0; goods],
        fee_carries: Vec::new(),
        wage_history: vec![0; 12],
        paid_history: vec![0; WAGE_HISTORY_MONTHS],
        paid_years: vec![0; WAGE_HISTORY_MONTHS],
    }
}

/// A place of `pops` pops (peasants, monks, then labourers) of 1000 people each, with prices as the base prices.
pub(crate) fn place(rules: &Rules, pops: usize) -> PlaceState {
    let goods = rules.goods.len();
    let kinds = ["peasant", "monk", "labourer"];
    PlaceState {
        id: "fixture".to_owned(),
        pops: (0..pops)
            .map(|index| PopState {
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
    }
}

fn add_building(rules: &Rules, place: &mut PlaceState, kind: &str, levels: u32) -> usize {
    let mut building = building_state(rules, kind, levels, place.pops.len());
    building.employment = vec![Employment::default(); place.pops.len()];
    place.buildings.push(building);
    for pop in &mut place.pops {
        pop.employment.push(Employment::default());
    }
    place.buildings.len() - 1
}

/// Gives a pop `owners` owner positions in a building.
fn own(place: &mut PlaceState, building: usize, pop: usize, owners: u64) {
    let job = Employment { owners, workers: 0 };
    place.buildings[building].employment[pop] = job.clone();
    place.pops[pop].employment[building] = job;
}

/// Gives a pop a day's capacity of `ml` through a one-level building of its own kind, `farm` for the first pop, `holdings` for the
/// second and `demesne` for the third, the building's number set by the case.
fn capacity(rules: &mut Rules, place: &mut PlaceState, pop: usize, ml: u64) {
    let kind = ["farm", "holdings", "demesne"][pop];
    rules
        .buildings
        .iter_mut()
        .find(|item| item.id == kind)
        .unwrap()
        .market_capacity_ml_per_level = ml;
    let building = add_building(rules, place, kind, 1);
    own(place, building, pop, 1);
}

fn target(rules: &Rules, place: &PlaceState, pop: usize, id: &str) -> u64 {
    let people = place.pops[pop].workers + place.pops[pop].dependents;
    u64::try_from(pop_target_units(rules, &place.pops[pop].kind, people, good(rules, id)).unwrap())
        .unwrap()
}

/// Sets a pop's store so it holds `spare` above its target of a good.
fn spare(rules: &Rules, place: &mut PlaceState, pop: usize, id: &str, units: u64) {
    place.pops[pop].stock[good(rules, id)] = target(rules, place, pop, id) + units;
}

/// Sets a pop's store so it lacks `units` of its target of a good.
fn want(rules: &Rules, place: &mut PlaceState, pop: usize, id: &str, units: u64) {
    let goal = target(rules, place, pop, id);
    assert!(
        goal >= units,
        "{id}: target {goal} is below the want {units}"
    );
    place.pops[pop].stock[good(rules, id)] = goal - units;
}

fn set_price(rules: &Rules, place: &mut PlaceState, id: &str, price: u64) {
    place.prices_milli[good(rules, id)] = price * 1000;
}

fn stock(rules: &Rules, place: &PlaceState, pop: usize, id: &str) -> u64 {
    place.pops[pop].stock[good(rules, id)]
}

fn position(rules: &Rules, place: &PlaceState, pop: usize, id: &str) -> (u128, u128) {
    let found = pop_position(rules, place, pop, good(rules, id)).unwrap();
    (found.spare, found.want)
}

fn exchange(rules: &Rules, place: &mut PlaceState) {
    let mut state = EconomyState {
        places: vec![place.clone()],
    };
    swap::run_daily_swaps(rules, &mut state).unwrap();
    *place = state.places.remove(0);
}

/// Every pop's store and account of every good, and the building stocks: the whole of what an exchange could change.
fn goods_everywhere(place: &PlaceState) -> Vec<Vec<u64>> {
    let mut all = place
        .pops
        .iter()
        .map(|pop| pop.stock.clone())
        .collect::<Vec<_>>();
    for building in &place.buildings {
        all.push(building.stock.clone());
        if let Some(accounts) = &building.storehouse_accounts {
            all.extend(accounts.pops.iter().cloned());
            all.push(accounts.communal.clone());
        }
    }
    all
}

/// The place's physical total of each good: the pops' stores plus the storehouse's stock.
fn total(rules: &Rules, place: &PlaceState) -> Vec<u64> {
    let mut sum = vec![0_u64; rules.goods.len()];
    for pop in &place.pops {
        for (index, units) in pop.stock.iter().enumerate() {
            sum[index] += units;
        }
    }
    for building in &place.buildings {
        for (index, units) in building.stock.iter().enumerate() {
            sum[index] += units;
        }
    }
    sum
}

/// The case of the design's example A, one turn by the named starter: oats 101, fish 30; the peasants have 20 bushels spare and want
/// 40 lb of fish, the monks 25 lb spare and want 6 bushels. Only `starter` has capacity (a day's, `ml`).
fn example_a(starter: usize, ml: u64) -> (Rules, PlaceState) {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    set_price(&rules, &mut place, "oats", 101);
    set_price(&rules, &mut place, "fish", 30);
    spare(&rules, &mut place, PEASANT, "oats", 20);
    want(&rules, &mut place, PEASANT, "fish", 40);
    spare(&rules, &mut place, MONK, "fish", 25);
    want(&rules, &mut place, MONK, "oats", 6);
    // The other goods sit at their targets, so nothing else is offered or wanted.
    settle_the_rest(&rules, &mut place, &["oats", "fish"]);
    capacity(&mut rules, &mut place, starter, ml);
    (rules, place)
}

/// Puts every good a case did not set at each pop's target, so it is neither spare nor wanted.
fn settle_the_rest(rules: &Rules, place: &mut PlaceState, set: &[&str]) {
    for pop in 0..place.pops.len() {
        for item in &rules.goods {
            if !set.contains(&item.id.as_str()) {
                place.pops[pop].stock[good(rules, &item.id)] = target(rules, place, pop, &item.id);
            }
        }
    }
}

#[test]
fn scarce_fish_is_shared_between_both_buyers() {
    for reverse in [false, true] {
        let mut rules = rules();
        let mut place = place(&rules, 3);
        set_price(&rules, &mut place, "oats", 1);
        set_price(&rules, &mut place, "fish", 1);
        for buyer in [PEASANT, THIRD] {
            spare(&rules, &mut place, buyer, "oats", 4);
            want(&rules, &mut place, buyer, "fish", 4);
            capacity(&mut rules, &mut place, buyer, 1_000_000);
        }
        spare(&rules, &mut place, MONK, "fish", 4);
        want(&rules, &mut place, MONK, "oats", 4);
        settle_the_rest(&rules, &mut place, &["oats", "fish"]);
        if reverse {
            place.pops.swap(PEASANT, THIRD);
            for building in &mut place.buildings {
                building.employment.swap(PEASANT, THIRD);
            }
        }
        let before = total(&rules, &place);
        let peasants = stock(&rules, &place, PEASANT, "fish");
        let labourers = stock(&rules, &place, THIRD, "fish");
        exchange(&rules, &mut place);
        assert_eq!(stock(&rules, &place, PEASANT, "fish"), peasants + 2);
        assert_eq!(stock(&rules, &place, THIRD, "fish"), labourers + 2);
        assert_eq!(total(&rules, &place), before);
    }
}

#[test]
fn personal_source_is_followed_by_building_source() {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    set_price(&rules, &mut place, "oats", 1);
    set_price(&rules, &mut place, "fish", 1);
    spare(&rules, &mut place, PEASANT, "oats", 5);
    want(&rules, &mut place, PEASANT, "fish", 5);
    spare(&rules, &mut place, MONK, "fish", 2);
    want(&rules, &mut place, MONK, "oats", 5);
    settle_the_rest(&rules, &mut place, &["oats", "fish"]);
    capacity(&mut rules, &mut place, PEASANT, 1_000_000);
    let building = add_building(&rules, &mut place, "holdings", 1);
    own(&mut place, building, MONK, 1);
    let fish = good(&rules, "fish");
    let people = place.pops[MONK].workers + place.pops[MONK].dependents;
    let reserve = food_target_units(&rules, &place.pops[MONK].kind, people, fish, 60).unwrap();
    place.buildings[building].stock[fish] = u64::try_from(reserve).unwrap() + 10;
    let before = total(&rules, &place);
    let monk_fish = stock(&rules, &place, MONK, "fish");
    let building_fish = place.buildings[building].stock[fish];
    let buyer_fish = stock(&rules, &place, PEASANT, "fish");
    exchange(&rules, &mut place);
    assert_eq!(stock(&rules, &place, PEASANT, "fish"), buyer_fish + 5);
    assert_eq!(stock(&rules, &place, MONK, "fish"), monk_fish - 2);
    assert_eq!(place.buildings[building].stock[fish], building_fish - 3);
    assert_eq!(total(&rules, &place), before);
}

#[test]
fn building_fish_pays_for_six_oats_at_the_example_prices() {
    let (rules, mut place) = example_a(PEASANT, 10_000_000);
    spare(&rules, &mut place, MONK, "fish", 0);
    let building = add_building(&rules, &mut place, "holdings", 1);
    own(&mut place, building, MONK, 1);
    let fish = good(&rules, "fish");
    let people = place.pops[MONK].workers + place.pops[MONK].dependents;
    let reserve =
        u64::try_from(food_target_units(&rules, &place.pops[MONK].kind, people, fish, 60).unwrap())
            .unwrap();
    place.buildings[building].stock[fish] = reserve + 100;
    let before = total(&rules, &place);
    let monk_oats = stock(&rules, &place, MONK, "oats");
    let buyer_fish = stock(&rules, &place, PEASANT, "fish");
    exchange(&rules, &mut place);
    assert_eq!(stock(&rules, &place, MONK, "oats"), monk_oats + 6);
    assert_eq!(stock(&rules, &place, PEASANT, "fish"), buyer_fish + 20);
    assert_eq!(place.buildings[building].stock[fish], reserve + 80);
    assert_eq!(total(&rules, &place), before);
}

/// C1--C6 share the same holder and two buyers.
fn sharing_case() -> (Rules, PlaceState) {
    let rules = rules();
    let mut place = place(&rules, 3);
    set_price(&rules, &mut place, "oats", 1);
    set_price(&rules, &mut place, "fish", 1);
    for pop in [PEASANT, THIRD] {
        spare(&rules, &mut place, pop, "oats", 4);
        want(&rules, &mut place, pop, "fish", 4);
    }
    spare(&rules, &mut place, MONK, "fish", 4);
    want(&rules, &mut place, MONK, "oats", 4);
    settle_the_rest(&rules, &mut place, &["oats", "fish"]);
    (rules, place)
}

#[test]
fn a_buyer_without_payment_gets_no_share() {
    let (mut rules, mut place) = sharing_case();
    want(&rules, &mut place, THIRD, "oats", 1);
    capacity(&mut rules, &mut place, PEASANT, 1_000_000);
    capacity(&mut rules, &mut place, THIRD, 1_000_000);
    let before = total(&rules, &place);
    let first = stock(&rules, &place, PEASANT, "fish");
    let third = stock(&rules, &place, THIRD, "fish");
    exchange(&rules, &mut place);
    assert_eq!(stock(&rules, &place, PEASANT, "fish"), first + 4);
    assert_eq!(stock(&rules, &place, THIRD, "fish"), third);
    assert_eq!(total(&rules, &place), before);
}

#[test]
fn second_pass_redistributes_only_to_a_buyer_who_used_its_share() {
    let (mut rules, mut place) = sharing_case();
    capacity(&mut rules, &mut place, PEASANT, 1_000_000);
    capacity(&mut rules, &mut place, THIRD, 1_000_000);
    // Wool takes one ml and fills the third pop's store to one fish unit of room.
    let wool = good(&rules, "wool");
    rules.goods[wool].volume_ml = 1;
    let people = place.pops[THIRD].workers + place.pops[THIRD].dependents;
    let (_, limit) = pop_store_limits(&rules, people, good(&rules, "fish")).unwrap();
    let used = volume_used(&rules, &place.pops[THIRD].stock, None).unwrap();
    let fish_volume = u128::from(rules.goods[good(&rules, "fish")].volume_ml);
    place.pops[THIRD].stock[wool] = u64::try_from(limit - used - fish_volume).unwrap();
    let before = total(&rules, &place);
    let first = stock(&rules, &place, PEASANT, "fish");
    let third = stock(&rules, &place, THIRD, "fish");
    exchange(&rules, &mut place);
    assert_eq!(stock(&rules, &place, PEASANT, "fish"), first + 3);
    assert_eq!(stock(&rules, &place, THIRD, "fish"), third + 1);
    assert_eq!(total(&rules, &place), before);
}

#[test]
fn second_share_is_capped_at_the_remaining_want_even_with_plenty_left() {
    let (mut rules, mut place) = sharing_case();
    spare(&rules, &mut place, MONK, "fish", 6);
    want(&rules, &mut place, MONK, "oats", 6);
    want(&rules, &mut place, PEASANT, "fish", 2);
    spare(&rules, &mut place, PEASANT, "oats", 2);
    want(&rules, &mut place, THIRD, "fish", 10);
    spare(&rules, &mut place, THIRD, "oats", 10);
    capacity(&mut rules, &mut place, PEASANT, 1_000_000);
    capacity(&mut rules, &mut place, THIRD, 1_000_000);
    let wool = good(&rules, "wool");
    rules.goods[wool].volume_ml = 1;
    let people = place.pops[THIRD].workers + place.pops[THIRD].dependents;
    let (_, limit) = pop_store_limits(&rules, people, good(&rules, "fish")).unwrap();
    let used = volume_used(&rules, &place.pops[THIRD].stock, None).unwrap();
    place.pops[THIRD].stock[wool] = u64::try_from(limit - used).unwrap();
    let before = total(&rules, &place);
    let first = stock(&rules, &place, PEASANT, "fish");
    let third = stock(&rules, &place, THIRD, "fish");
    exchange(&rules, &mut place);
    assert_eq!(stock(&rules, &place, PEASANT, "fish"), first + 2);
    assert_eq!(stock(&rules, &place, THIRD, "fish"), third);
    assert_eq!(total(&rules, &place), before);
}

#[test]
fn holder_can_deliver_a_second_share_to_a_buyer_without_capacity() {
    let (mut rules, mut place) = sharing_case();
    capacity(&mut rules, &mut place, MONK, 1_000_000);
    let wool = good(&rules, "wool");
    rules.goods[wool].volume_ml = 1;
    let people = place.pops[THIRD].workers + place.pops[THIRD].dependents;
    let (_, limit) = pop_store_limits(&rules, people, good(&rules, "fish")).unwrap();
    let used = volume_used(&rules, &place.pops[THIRD].stock, None).unwrap();
    let fish_volume = u128::from(rules.goods[good(&rules, "fish")].volume_ml);
    place.pops[THIRD].stock[wool] = u64::try_from(limit - used - fish_volume).unwrap();
    let before = total(&rules, &place);
    let first = stock(&rules, &place, PEASANT, "fish");
    let third = stock(&rules, &place, THIRD, "fish");
    exchange(&rules, &mut place);
    assert_eq!(stock(&rules, &place, PEASANT, "fish"), first + 3);
    assert_eq!(stock(&rules, &place, THIRD, "fish"), third + 1);
    assert_eq!(total(&rules, &place), before);
}

#[test]
fn holder_spends_its_capacity_for_buyers_without_turns() {
    let (mut rules, mut place) = sharing_case();
    capacity(&mut rules, &mut place, MONK, 1_000_000);
    let before = total(&rules, &place);
    let first = stock(&rules, &place, PEASANT, "fish");
    let third = stock(&rules, &place, THIRD, "fish");
    exchange(&rules, &mut place);
    assert_eq!(stock(&rules, &place, PEASANT, "fish"), first + 2);
    assert_eq!(stock(&rules, &place, THIRD, "fish"), third + 2);
    assert_eq!(total(&rules, &place), before);
}

#[test]
fn indivisible_fish_goes_to_the_wealthier_buyer() {
    for wealthy in [PEASANT, THIRD] {
        let (mut rules, mut place) = sharing_case();
        spare(&rules, &mut place, MONK, "fish", 1);
        place.pops[wealthy].stock[good(&rules, "wool")] = 1_000;
        capacity(&mut rules, &mut place, PEASANT, 1_000_000);
        capacity(&mut rules, &mut place, THIRD, 1_000_000);
        let before = total(&rules, &place);
        let fish = [PEASANT, THIRD].map(|p| stock(&rules, &place, p, "fish"));
        exchange(&rules, &mut place);
        for (index, pop) in [PEASANT, THIRD].into_iter().enumerate() {
            assert_eq!(
                stock(&rules, &place, pop, "fish"),
                fish[index] + u64::from(pop == wealthy)
            );
        }
        assert_eq!(total(&rules, &place), before);
    }
}

#[test]
fn ownerless_communal_stock_is_in_orders_but_not_shares() {
    let (mut rules, mut place) = sharing_case();
    capacity(&mut rules, &mut place, PEASANT, 1_000_000);
    capacity(&mut rules, &mut place, THIRD, 1_000_000);
    let house = storehouse(&rules, &mut place);
    let fish = good(&rules, "fish");
    place.buildings[house]
        .storehouse_accounts
        .as_mut()
        .unwrap()
        .communal[fish] = 4;
    place.buildings[house].stock[fish] = 4;
    assert_eq!(
        daily_market_orders(&rules, &place).unwrap().sell_units[fish],
        8
    );
    let before = total(&rules, &place);
    let first = stock(&rules, &place, PEASANT, "fish");
    let third = stock(&rules, &place, THIRD, "fish");
    exchange(&rules, &mut place);
    assert_eq!(stock(&rules, &place, PEASANT, "fish"), first + 2);
    assert_eq!(stock(&rules, &place, THIRD, "fish"), third + 2);
    assert_eq!(place.buildings[house].stock[fish], 4);
    assert_eq!(total(&rules, &place), before);
}

#[test]
fn each_owner_can_only_give_its_part_of_a_building() {
    let mut rules = rules();
    let mut place = place(&rules, 3);
    for id in ["oats", "fish"] {
        set_price(&rules, &mut place, id, 1);
    }
    settle_the_rest(&rules, &mut place, &[]);
    spare(&rules, &mut place, THIRD, "oats", 30);
    want(&rules, &mut place, THIRD, "fish", 30);
    want(&rules, &mut place, PEASANT, "oats", 20);
    capacity(&mut rules, &mut place, THIRD, 10_000_000);
    let building = add_building(&rules, &mut place, "holdings", 1);
    own(&mut place, building, PEASANT, 2);
    own(&mut place, building, MONK, 1);
    let fish = good(&rules, "fish");
    let reserve = [PEASANT, MONK]
        .into_iter()
        .map(|p| {
            let people = place.pops[p].workers + place.pops[p].dependents;
            food_target_units(&rules, &place.pops[p].kind, people, fish, 60).unwrap()
        })
        .sum::<u128>();
    place.buildings[building].stock[fish] = u64::try_from(reserve).unwrap() + 30;
    let before = total(&rules, &place);
    let buyer = stock(&rules, &place, THIRD, "fish");
    exchange(&rules, &mut place);
    assert_eq!(stock(&rules, &place, THIRD, "fish"), buyer + 20);
    assert_eq!(
        place.buildings[building].stock[fish],
        u64::try_from(reserve).unwrap() + 10
    );
    assert_eq!(total(&rules, &place), before);
}

#[test]
fn food_below_the_owners_reserve_has_no_tradable_part() {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    for id in ["oats", "fish"] {
        set_price(&rules, &mut place, id, 1);
    }
    settle_the_rest(&rules, &mut place, &[]);
    spare(&rules, &mut place, PEASANT, "oats", 4);
    want(&rules, &mut place, PEASANT, "fish", 4);
    want(&rules, &mut place, MONK, "oats", 4);
    capacity(&mut rules, &mut place, PEASANT, 1_000_000);
    let building = add_building(&rules, &mut place, "holdings", 1);
    own(&mut place, building, MONK, 1);
    let fish = good(&rules, "fish");
    let people = place.pops[MONK].workers + place.pops[MONK].dependents;
    let reserve = food_target_units(&rules, &place.pops[MONK].kind, people, fish, 60).unwrap();
    place.buildings[building].stock[fish] = u64::try_from(reserve - 1).unwrap();
    let before = goods_everywhere(&place);
    exchange(&rules, &mut place);
    assert_eq!(goods_everywhere(&place), before);
}

#[test]
fn source_pair_is_not_retried_when_payment_reopens_room() {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    for pop in &mut place.pops {
        pop.workers = 100_000_000;
        pop.dependents = 0;
    }
    for id in ["oats", "fish", "wool"] {
        let index = good(&rules, id);
        rules.goods[index].volume_ml = 1;
    }
    set_price(&rules, &mut place, "oats", 1);
    set_price(&rules, &mut place, "fish", 1);
    spare(&rules, &mut place, PEASANT, "oats", 10_000_000);
    want(&rules, &mut place, PEASANT, "fish", 10_000_000);
    spare(&rules, &mut place, MONK, "fish", 10_000_000);
    want(&rules, &mut place, MONK, "oats", 10_000_000);
    settle_the_rest(&rules, &mut place, &["oats", "fish"]);
    for pop in [PEASANT, MONK] {
        let people = place.pops[pop].workers;
        let (_, limit) = pop_store_limits(&rules, people, good(&rules, "fish")).unwrap();
        let used = volume_used(&rules, &place.pops[pop].stock, None).unwrap();
        place.pops[pop].stock[good(&rules, "wool")] = u64::try_from(limit - used - 1).unwrap();
    }
    capacity(&mut rules, &mut place, PEASANT, 20_000_001);
    let before = total(&rules, &place);
    let oats = stock(&rules, &place, PEASANT, "oats");
    let fish = stock(&rules, &place, PEASANT, "fish");
    exchange(&rules, &mut place);
    assert_eq!(stock(&rules, &place, PEASANT, "oats"), oats - 1);
    assert_eq!(stock(&rules, &place, PEASANT, "fish"), fish + 1);
    assert_eq!(total(&rules, &place), before);
}

#[test]
fn a_storehouse_owner_trades_communal_stock_without_touching_pop_accounts() {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    for id in ["oats", "fish"] {
        set_price(&rules, &mut place, id, 1);
    }
    settle_the_rest(&rules, &mut place, &[]);
    spare(&rules, &mut place, PEASANT, "oats", 4);
    want(&rules, &mut place, PEASANT, "fish", 4);
    want(&rules, &mut place, MONK, "oats", 4);
    capacity(&mut rules, &mut place, PEASANT, 1_000_000);
    let house = storehouse(&rules, &mut place);
    own(&mut place, house, MONK, 1);
    let fish = good(&rules, "fish");
    let people = place.pops[MONK].workers + place.pops[MONK].dependents;
    let reserve =
        u64::try_from(food_target_units(&rules, &place.pops[MONK].kind, people, fish, 60).unwrap())
            .unwrap();
    place.pops[MONK].stock[fish] -= 5;
    let accounts = place.buildings[house].storehouse_accounts.as_mut().unwrap();
    accounts.pops[MONK][fish] = 5;
    accounts.communal[fish] = reserve + 10;
    place.buildings[house].stock[fish] = reserve + 15;
    fill_grain_room_to(&mut rules, &mut place, house, 0);
    for room in [true, false] {
        let mut trial_rules = rules.clone();
        let mut trial = place.clone();
        if !room {
            at_its_limit(&mut trial_rules, &trial, MONK, "oats");
        }
        let before = total(&trial_rules, &trial);
        let everywhere = goods_everywhere(&trial);
        let oats = stock(&trial_rules, &trial, MONK, "oats");
        exchange(&trial_rules, &mut trial);
        let accounts = trial.buildings[house].storehouse_accounts.as_ref().unwrap();
        assert_eq!(accounts.pops[MONK][fish], 5);
        if room {
            assert_eq!(accounts.communal[fish], reserve + 6);
            assert_eq!(trial.buildings[house].stock[fish], reserve + 11);
            assert_eq!(stock(&trial_rules, &trial, MONK, "oats"), oats + 4);
        } else {
            assert_eq!(goods_everywhere(&trial), everywhere);
        }
        assert_eq!(total(&trial_rules, &trial), before);
    }
}

#[test]
fn both_personal_sources_precede_both_communal_parts() {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    for id in ["oats", "fish"] {
        set_price(&rules, &mut place, id, 1);
    }
    settle_the_rest(&rules, &mut place, &[]);
    spare(&rules, &mut place, PEASANT, "oats", 1);
    want(&rules, &mut place, PEASANT, "fish", 1);
    spare(&rules, &mut place, MONK, "fish", 1);
    want(&rules, &mut place, MONK, "oats", 1);
    capacity(&mut rules, &mut place, PEASANT, 1_000_000);
    let house = storehouse(&rules, &mut place);
    own(&mut place, house, PEASANT, 1);
    own(&mut place, house, MONK, 1);
    for id in ["oats", "fish"] {
        let good = good(&rules, id);
        place.buildings[house].stock[good] = 100_000;
        place.buildings[house]
            .storehouse_accounts
            .as_mut()
            .unwrap()
            .communal[good] = 100_000;
    }
    let before = goods_everywhere(&place);
    let total_before = total(&rules, &place);
    exchange(&rules, &mut place);
    assert_eq!(
        place.pops[PEASANT].stock[good(&rules, "oats")],
        before[PEASANT][good(&rules, "oats")] - 1
    );
    assert_eq!(
        place.pops[PEASANT].stock[good(&rules, "fish")],
        before[PEASANT][good(&rules, "fish")] + 1
    );
    assert_eq!(
        place.pops[MONK].stock[good(&rules, "oats")],
        before[MONK][good(&rules, "oats")] + 1
    );
    assert_eq!(
        place.pops[MONK].stock[good(&rules, "fish")],
        before[MONK][good(&rules, "fish")] - 1
    );
    assert_eq!(&goods_everywhere(&place)[2..], &before[2..]);
    assert_eq!(total(&rules, &place), total_before);
}

#[test]
fn a_receipt_does_not_spill_into_a_storehouse_or_building() {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    for id in ["oats", "fish"] {
        set_price(&rules, &mut place, id, 1);
    }
    settle_the_rest(&rules, &mut place, &[]);
    spare(&rules, &mut place, PEASANT, "oats", 2);
    want(&rules, &mut place, PEASANT, "fish", 2);
    want(&rules, &mut place, MONK, "oats", 2);
    capacity(&mut rules, &mut place, PEASANT, 1_000_000);
    let house = storehouse(&rules, &mut place);
    own(&mut place, house, MONK, 1);
    let fish = good(&rules, "fish");
    let oats = good(&rules, "oats");
    let people = place.pops[MONK].workers + place.pops[MONK].dependents;
    let reserve =
        u64::try_from(food_target_units(&rules, &place.pops[MONK].kind, people, fish, 60).unwrap())
            .unwrap();
    place.buildings[house].stock[fish] = reserve + 10;
    place.buildings[house]
        .storehouse_accounts
        .as_mut()
        .unwrap()
        .communal[fish] = reserve + 10;
    rules.goods[oats].pop_limit_milli = (place.pops[MONK].stock[oats] + 1) * 1000 / people;
    let before = total(&rules, &place);
    let monk_oats = place.pops[MONK].stock[oats];
    exchange(&rules, &mut place);
    assert_eq!(place.pops[MONK].stock[oats], monk_oats + 1);
    let accounts = place.buildings[house].storehouse_accounts.as_ref().unwrap();
    assert_eq!(accounts.communal[fish], reserve + 9);
    assert_eq!(accounts.communal[oats], 0);
    assert_eq!(accounts.pops[MONK][oats], 0);
    assert_eq!(place.buildings[house].stock[oats], 0);
    assert_eq!(total(&rules, &place), before);
}

#[test]
fn second_share_excludes_a_buyer_that_has_run_out_of_payment() {
    let mut rules = rules();
    let mut place = place(&rules, 3);
    place.pops.push(place.pops[THIRD].clone());
    for id in ["oats", "fish"] {
        set_price(&rules, &mut place, id, 1);
    }
    settle_the_rest(&rules, &mut place, &[]);
    for pop in [PEASANT, THIRD, 3] {
        want(&rules, &mut place, pop, "fish", 4);
        spare(
            &rules,
            &mut place,
            pop,
            "oats",
            if pop == PEASANT { 2 } else { 4 },
        );
    }
    spare(&rules, &mut place, MONK, "fish", 6);
    want(&rules, &mut place, MONK, "oats", 6);
    capacity(&mut rules, &mut place, PEASANT, 1_000_000);
    capacity(&mut rules, &mut place, THIRD, 1_000_000);
    let wool = good(&rules, "wool");
    rules.goods[wool].volume_ml = 1;
    let people = place.pops[3].workers + place.pops[3].dependents;
    let (_, limit) = pop_store_limits(&rules, people, good(&rules, "fish")).unwrap();
    let used = volume_used(&rules, &place.pops[3].stock, None).unwrap();
    place.pops[3].stock[wool] = u64::try_from(limit - used).unwrap();
    let before = total(&rules, &place);
    let fish: Vec<_> = [PEASANT, THIRD, 3]
        .into_iter()
        .map(|p| stock(&rules, &place, p, "fish"))
        .collect();
    exchange(&rules, &mut place);
    for (pop, gained) in [(PEASANT, 2), (THIRD, 4), (3, 0)] {
        let index = if pop == PEASANT {
            0
        } else if pop == THIRD {
            1
        } else {
            2
        };
        assert_eq!(stock(&rules, &place, pop, "fish"), fish[index] + gained);
    }
    assert_eq!(total(&rules, &place), before);
}

// ---------------------------------------------------------------- rules 1 to 3: spare, want and the orders

#[test]
fn spare_and_want_are_the_holding_above_the_target_and_the_target_less_the_holding() {
    let rules = rules();
    let mut place = place(&rules, 2);
    let oats_target = target(&rules, &place, PEASANT, "oats");
    assert!(oats_target > 20);
    spare(&rules, &mut place, PEASANT, "oats", 20);
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (20, 0));
    want(&rules, &mut place, PEASANT, "oats", 7);
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (0, 7));
    place.pops[PEASANT].stock[good(&rules, "oats")] = oats_target;
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (0, 0));
}

#[test]
fn a_good_with_no_hunger_value_can_be_spare_and_one_with_no_target_is_all_spare() {
    let rules = rules();
    let mut place = place(&rules, 2);
    // Herbs have no hunger value and a comfort target; wool has no target at all.
    assert!(target(&rules, &place, PEASANT, "herbs") > 3);
    spare(&rules, &mut place, PEASANT, "herbs", 3);
    assert_eq!(position(&rules, &place, PEASANT, "herbs"), (3, 0));
    assert_eq!(target(&rules, &place, PEASANT, "wool"), 0);
    place.pops[PEASANT].stock[good(&rules, "wool")] = 9;
    assert_eq!(position(&rules, &place, PEASANT, "wool"), (9, 0));
}

#[test]
fn a_store_and_an_account_together_are_the_holding() {
    let rules = rules();
    let mut place = place(&rules, 2);
    let house = add_building(&rules, &mut place, "storehouse", 1);
    place.buildings[house].method_levels = vec![1, 1, 1];
    place.buildings[house].storehouse_accounts = Some(StorehouseAccounts {
        pops: vec![vec![0; rules.goods.len()]; 2],
        communal: vec![0; rules.goods.len()],
    });
    let oats = good(&rules, "oats");
    let goal = target(&rules, &place, PEASANT, "oats");
    place.pops[PEASANT].stock[oats] = goal - 5;
    place.buildings[house]
        .storehouse_accounts
        .as_mut()
        .unwrap()
        .pops[PEASANT][oats] = 12;
    place.buildings[house].stock[oats] = 12;
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (7, 0));
}

#[test]
fn the_places_sell_orders_now_hold_a_pops_spare_wool_and_the_other_orders_are_what_they_were() {
    let rules = rules();
    let mut place = place(&rules, 2);
    settle_the_rest(&rules, &mut place, &["wool", "oats"]);
    place.pops[PEASANT].stock[good(&rules, "wool")] = 1;
    spare(&rules, &mut place, PEASANT, "oats", 8);
    want(&rules, &mut place, MONK, "oats", 5);
    let orders = daily_market_orders(&rules, &place).unwrap();
    let wool = good(&rules, "wool");
    let oats = good(&rules, "oats");
    assert_eq!(orders.sell_units[wool], 1);
    assert_eq!(orders.buy_units[wool], 0);
    // Food: the peasants' spare is offered and the monks' want is bought, as before.
    assert_eq!(orders.sell_units[oats], 8);
    assert_eq!(orders.buy_units[oats], 5);
    // Herbs, held at the target by both: no order either way.
    let herbs = good(&rules, "herbs");
    assert_eq!((orders.sell_units[herbs], orders.buy_units[herbs]), (0, 0));
}

// ---------------------------------------------------------------- rules 5 to 8: capacity

#[test]
fn a_buildings_capacity_is_its_levels_times_its_number_split_by_owner_positions() {
    let mut rules = rules();
    rules
        .buildings
        .iter_mut()
        .find(|item| item.id == "farm")
        .unwrap()
        .market_capacity_ml_per_level = 50_000;
    let mut place = place(&rules, 3);
    let farm = add_building(&rules, &mut place, "farm", 3);
    // 150,000 ml among positions 1:2 leaves no remainder: 50,000 and 100,000.
    own(&mut place, farm, PEASANT, 1);
    own(&mut place, farm, MONK, 2);
    assert_eq!(
        market_capacities(&rules, &place).unwrap(),
        vec![50_000, 100_000, 0]
    );
}

#[test]
fn a_split_with_a_remainder_sums_to_the_whole_and_a_tie_goes_to_the_earlier_pop() {
    let mut rules = rules();
    rules
        .buildings
        .iter_mut()
        .find(|item| item.id == "farm")
        .unwrap()
        .market_capacity_ml_per_level = 50_000;
    let mut place = place(&rules, 3);
    let farm = add_building(&rules, &mut place, "farm", 1);
    // 50,000 among three equal positions: 16,666 each and two left, which the two earlier pops take.
    for pop in 0..3 {
        own(&mut place, farm, pop, 4);
    }
    let split = market_capacities(&rules, &place).unwrap();
    assert_eq!(split, vec![16_667, 16_667, 16_666]);
    assert_eq!(split.iter().sum::<u128>(), 50_000);
    // A remainder that is not tied: 50,000 among positions 1 and 2 is 16,666.67 and 33,333.33, so the one leftover unit goes to the
    // larger fraction, the first.
    let mut place = place_with_two(&rules);
    assert_eq!(
        market_capacities(&rules, &place).unwrap(),
        vec![16_667, 33_333, 0]
    );
    place.buildings.clear();
    assert_eq!(market_capacities(&rules, &place).unwrap(), vec![0, 0, 0]);
}

fn place_with_two(rules: &Rules) -> PlaceState {
    let mut place = place(rules, 3);
    let farm = add_building(rules, &mut place, "farm", 1);
    own(&mut place, farm, PEASANT, 1);
    own(&mut place, farm, MONK, 2);
    place
}

#[test]
fn a_pop_with_positions_in_two_buildings_has_the_sum_and_one_with_none_has_none() {
    let mut rules = rules();
    for (kind, ml) in [("farm", 50_000), ("holdings", 100_000)] {
        rules
            .buildings
            .iter_mut()
            .find(|item| item.id == kind)
            .unwrap()
            .market_capacity_ml_per_level = ml;
    }
    let mut place = place(&rules, 3);
    let farm = add_building(&rules, &mut place, "farm", 2);
    let holdings = add_building(&rules, &mut place, "holdings", 1);
    own(&mut place, farm, PEASANT, 5);
    own(&mut place, holdings, PEASANT, 1);
    own(&mut place, holdings, MONK, 1);
    // The peasants: all of the farm's 100,000 and half the holdings' 100,000. The monks: the other half. The third pop owns nothing.
    assert_eq!(
        market_capacities(&rules, &place).unwrap(),
        vec![150_000, 50_000, 0]
    );
}

/// Copies a directory tree: the rules sit beside the files they name.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn the_loss_limit_loads_from_0_to_1000_per_mille_and_a_larger_one_is_refused() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game");
    let directory = std::env::temp_dir().join(format!("ck-loss-limit-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    // The rules sit beside the files they name: copy the shipped data and edit the one number.
    copy_tree(&source, &directory);
    let text = fs::read_to_string(source.join("economy.toml")).unwrap();
    let load = |value: u32| {
        let edited = text.replacen(
            "swap_loss_limit_pm = 100",
            &format!("swap_loss_limit_pm = {value}"),
            1,
        );
        assert!(edited.contains(&format!("swap_loss_limit_pm = {value}")));
        fs::write(directory.join("economy.toml"), edited).unwrap();
        Rules::load(directory.join("economy.toml"))
    };
    assert_eq!(load(0).unwrap().swap_loss_limit_pm, 0);
    assert_eq!(load(1000).unwrap().swap_loss_limit_pm, 1000);
    let message = load(1001).unwrap_err().to_string();
    assert!(message.contains("swap_loss_limit_pm"), "{message}");
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn a_building_type_with_no_number_gives_none_and_the_shipped_numbers_are_the_proposal() {
    let rules = shipped();
    let number = |id: &str| {
        rules
            .buildings
            .iter()
            .find(|item| item.id == id)
            .unwrap()
            .market_capacity_ml_per_level
    };
    assert_eq!(number("farm"), 50_000);
    assert_eq!(number("holdings"), 100_000);
    assert_eq!(number("demesne"), 100_000);
    assert_eq!(number("storehouse"), 0);
    assert_eq!(rules.swap_loss_limit_pm, 100);
    let mut place = place(&rules, 2);
    let house = add_building(&rules, &mut place, "storehouse", 4);
    own(&mut place, house, PEASANT, 1);
    assert_eq!(market_capacities(&rules, &place).unwrap(), vec![0, 0]);
}

#[test]
fn yesterdays_unused_capacity_is_not_there_today() {
    let (mut rules, mut place) = example_a(PEASANT, 1_000_000);
    exchange(&rules, &mut place);
    // The capacity is worked out afresh from the buildings: the same day's allowance, whatever was spent.
    assert_eq!(
        market_capacities(&rules, &place).unwrap(),
        vec![1_000_000, 0]
    );
    rules
        .buildings
        .iter_mut()
        .find(|item| item.id == "farm")
        .unwrap()
        .market_capacity_ml_per_level = 600;
    assert_eq!(market_capacities(&rules, &place).unwrap(), vec![600, 0]);
}

// ---------------------------------------------------------------- one swap: the worked cases

#[test]
fn a_an_ordinary_swap_is_six_bushels_for_twenty_pounds() {
    let (rules, mut place) = example_a(PEASANT, 1_000_000);
    let before = total(&rules, &place);
    exchange(&rules, &mut place);
    let oats_target = target(&rules, &place, PEASANT, "oats");
    // The peasants gave 6 bushels and received 20 lb.
    assert_eq!(stock(&rules, &place, PEASANT, "oats"), oats_target + 14);
    assert_eq!(
        stock(&rules, &place, PEASANT, "fish"),
        target(&rules, &place, PEASANT, "fish") - 20
    );
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (14, 0));
    assert_eq!(position(&rules, &place, PEASANT, "fish"), (0, 20));
    assert_eq!(position(&rules, &place, MONK, "fish"), (5, 0));
    assert_eq!(position(&rules, &place, MONK, "oats"), (0, 0));
    // The peasants' capacity is down by 6 x 35 + 20 = 230 litres; the monks' by nothing.
    let capacities = market_capacities(&rules, &place).unwrap();
    assert_eq!(capacities, vec![1_000_000, 0]);
    assert_eq!(total(&rules, &place), before);
}

#[test]
fn b1_equal_prices_ten_for_ten() {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    set_price(&rules, &mut place, "oats", 50);
    set_price(&rules, &mut place, "barley", 50);
    spare(&rules, &mut place, PEASANT, "oats", 10);
    want(&rules, &mut place, PEASANT, "barley", 10);
    spare(&rules, &mut place, MONK, "barley", 10);
    want(&rules, &mut place, MONK, "oats", 10);
    settle_the_rest(&rules, &mut place, &["oats", "barley"]);
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (0, 0));
    assert_eq!(position(&rules, &place, PEASANT, "barley"), (0, 0));
    assert_eq!(position(&rules, &place, MONK, "oats"), (0, 0));
    assert_eq!(position(&rules, &place, MONK, "barley"), (0, 0));
}

/// B2: oats 100, honey 70; the peasants start with `oats` bushels spare and want `honey` lb of honey, which the monks have spare
/// and want oats for.
fn example_b2(honey: u64) -> (Rules, PlaceState) {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    set_price(&rules, &mut place, "oats", 100);
    set_price(&rules, &mut place, "honey", 70);
    spare(&rules, &mut place, PEASANT, "oats", 3);
    want(&rules, &mut place, PEASANT, "honey", honey);
    spare(&rules, &mut place, MONK, "honey", honey);
    want(&rules, &mut place, MONK, "oats", 3);
    settle_the_rest(&rules, &mut place, &["oats", "honey"]);
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    (rules, place)
}

#[test]
fn b2_the_limit_three_bushels_for_four_pounds_and_no_swap_when_only_three_are_to_be_had() {
    let (rules, mut place) = example_b2(4);
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (0, 0));
    assert_eq!(position(&rules, &place, PEASANT, "honey"), (0, 0));
    // 3 bushels (300) for 4 lb (280): a loss of 20 in 300.
    let (rules, mut place) = example_b2(3);
    let before = goods_everywhere(&place);
    exchange(&rules, &mut place);
    // 3 for 3 loses 90 in 300, 2 for 2 loses 60 in 200 and 1 for 1 loses 30 in 100: each over a tenth, so no swap.
    assert_eq!(goods_everywhere(&place), before);
}

#[test]
fn b2_the_honeys_holder_with_capacity_and_a_turn_gives_three_pounds_for_two_bushels() {
    let (mut rules, mut place) = example_b2(3);
    // Were the honey's holder to have capacity and a turn: 3 lb (210) for 2 bushels (200) passes.
    capacity(&mut rules, &mut place, MONK, 10_000_000);
    // The peasants have capacity too; with the monks' turn after theirs, the peasants' own turn finds nothing, and the monks' finds
    // the swap that passes.
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, MONK, "oats"), (0, 1));
    assert_eq!(position(&rules, &place, MONK, "honey"), (0, 0));
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (1, 0));
    assert_eq!(position(&rules, &place, PEASANT, "honey"), (0, 0));
}

#[test]
fn b3_one_unit_each_one_for_one_and_none_when_the_honeys_holder_must_start() {
    // Oats 1,001, honey 1,000, one unit spare on each side. The oats' holder starts: one for one.
    let mut rules = rules();
    let mut place = place(&rules, 2);
    set_price(&rules, &mut place, "oats", 1001);
    set_price(&rules, &mut place, "honey", 1000);
    spare(&rules, &mut place, PEASANT, "oats", 1);
    want(&rules, &mut place, PEASANT, "honey", 1);
    spare(&rules, &mut place, MONK, "honey", 1);
    want(&rules, &mut place, MONK, "oats", 1);
    settle_the_rest(&rules, &mut place, &["oats", "honey"]);
    let mut other = place.clone();
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (0, 0));
    assert_eq!(position(&rules, &place, PEASANT, "honey"), (0, 0));
    assert_eq!(position(&rules, &place, MONK, "oats"), (0, 0));
    assert_eq!(position(&rules, &place, MONK, "honey"), (0, 0));
    // With the honey's holder first in the order and the oats' holder without capacity: no swap (1,000 for 1,001 is not worth it).
    other.pops.swap(PEASANT, MONK);
    other.pops[PEASANT].kind = "peasant".to_owned();
    other.pops[MONK].kind = "monk".to_owned();
    capacity(&mut rules, &mut other, PEASANT, 10_000_000);
    assert_eq!(position(&rules, &other, PEASANT, "honey"), (1, 0));
    assert_eq!(position(&rules, &other, MONK, "oats"), (1, 0));
    let before = goods_everywhere(&other);
    exchange(&rules, &mut other);
    assert_eq!(goods_everywhere(&other), before);
}

#[test]
fn b4_nobody_wants_it_no_swap() {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    spare(&rules, &mut place, PEASANT, "honey", 10);
    want(&rules, &mut place, PEASANT, "oats", 4);
    spare(&rules, &mut place, MONK, "oats", 10);
    // Nobody wants honey: the monks are at their target for it.
    settle_the_rest(&rules, &mut place, &["oats", "honey"]);
    place.pops[MONK].stock[good(&rules, "honey")] = target(&rules, &place, MONK, "honey");
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    let before = goods_everywhere(&place);
    exchange(&rules, &mut place);
    assert_eq!(goods_everywhere(&place), before);
}

#[test]
fn a_swap_not_the_peasants_alone_but_the_other_pop_starts_when_it_is_first_in_the_order() {
    // As A, with the monks first in the order: the monks start for oats. They receive 6 bushels (606) and give the fewest pounds
    // worth that, 21 (630): 21 lb for 6 bushels. The loss, 24, is the monks'.
    let (rules, mut place) = example_a(MONK, 1_000_000);
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, MONK, "oats"), (0, 0));
    assert_eq!(position(&rules, &place, MONK, "fish"), (4, 0));
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (14, 0));
    assert_eq!(position(&rules, &place, PEASANT, "fish"), (0, 19));
}

#[test]
fn no_capacity_no_turn_the_pop_without_a_building_starts_nothing_and_the_monks_turn_gives_the_swap()
{
    let (mut rules, mut place) = example_a(MONK, 1_000_000);
    // The peasants own no building: they have no turn.
    assert_eq!(market_capacities(&rules, &place).unwrap()[PEASANT], 0);
    let before_monks = place.pops[MONK].stock.clone();
    exchange(&rules, &mut place);
    assert_ne!(place.pops[MONK].stock, before_monks);
    // With no capacity anywhere nothing moves.
    let (rules2, mut place2) = example_a(PEASANT, 0);
    let before = goods_everywhere(&place2);
    exchange(&rules2, &mut place2);
    assert_eq!(goods_everywhere(&place2), before);
    let _ = &mut rules;
}

#[test]
fn the_capacity_case_three_bushels_for_ten_pounds_and_five_litres_left() {
    // As A, but the peasants have 120 litres of capacity left. 3 bushels and 10 lb take 115 litres; a fourth bushel does not fit.
    let (rules, mut place) = example_a(PEASANT, 120_000);
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (17, 0));
    assert_eq!(position(&rules, &place, PEASANT, "fish"), (0, 30));
    assert_eq!(position(&rules, &place, MONK, "fish"), (15, 0));
    assert_eq!(position(&rules, &place, MONK, "oats"), (0, 3));
}

#[test]
fn a_swap_never_makes_or_loses_a_unit() {
    for ml in [0, 50_000, 120_000, 230_000, 1_000_000] {
        let (rules, mut place) = example_a(PEASANT, ml);
        let before = total(&rules, &place);
        exchange(&rules, &mut place);
        assert_eq!(total(&rules, &place), before, "capacity {ml}");
    }
}

// ---------------------------------------------------------------- receipt and room

/// The peasants' store can take `room` more pounds of fish.
fn narrow_fish_room(rules: &mut Rules, place: &mut PlaceState, room: u64) {
    let fish = good(rules, "fish");
    let people = place.pops[PEASANT].workers + place.pops[PEASANT].dependents;
    let held = place.pops[PEASANT].stock[fish];
    rules.goods[fish].pop_limit_milli = (held + room) * 1000 / people;
    assert_eq!(
        u128::from(rules.goods[fish].pop_limit_milli) * u128::from(people) / 1000,
        u128::from(held + room)
    );
}

#[test]
fn a_full_store_takes_nothing_even_of_a_good_that_takes_no_room() {
    // Rule 13: a pop whose store is full or over its volume takes nothing. The fish takes no room (0 ml), the peasants' store is at its
    // volume limit and their fish stock is below the good's limit: the example's swap must not happen.
    let (mut rules, mut place) = example_a(PEASANT, 10_000_000);
    let (fish, barley) = (good(&rules, "fish"), good(&rules, "barley"));
    rules.goods[fish].volume_ml = 0;
    let people = place.pops[PEASANT].workers + place.pops[PEASANT].dependents;
    // Barley fills the peasants' store (720 litres a person) to its volume limit: the oats and the fish still have room by their limits.
    let volume = u128::from(rules.goods[barley].volume_ml);
    let fill = u128::from(people) * 720_000;
    let held = u128::from(place.pops[PEASANT].stock[barley]);
    let used = volume_used(&rules, &place.pops[PEASANT].stock, None).unwrap();
    place.pops[PEASANT].stock[barley] +=
        u64::try_from((fill.saturating_sub(used)).div_ceil(volume)).unwrap();
    assert!(held <= u128::from(place.pops[PEASANT].stock[barley]));
    assert!(volume_used(&rules, &place.pops[PEASANT].stock, None).unwrap() >= fill);
    let before = goods_everywhere(&place);
    exchange(&rules, &mut place);
    assert_eq!(goods_everywhere(&place), before);
    // With room the same case swaps: the witness is the volume and nothing else.
    let (mut rules, mut place) = example_a(PEASANT, 10_000_000);
    rules.goods[fish].volume_ml = 0;
    exchange(&rules, &mut place);
    assert_ne!(goods_everywhere(&place), before);
}

#[test]
fn the_allowance_of_a_valid_building_never_stops_the_day() {
    // Two to the sixty-second ml a level is a valid number: four levels owned by one pop are exactly 2^64 ml, and a pop owning half of
    // it has 2^63.
    let mut rules = rules();
    let mut place = place(&rules, 2);
    let building = add_building(&rules, &mut place, "demesne", 4);
    own(&mut place, building, PEASANT, 1);
    own(&mut place, building, MONK, 1);
    rules
        .buildings
        .iter_mut()
        .find(|item| item.id == "demesne")
        .unwrap()
        .market_capacity_ml_per_level = 1 << 62;
    assert_eq!(
        market_capacities(&rules, &place).unwrap(),
        vec![1 << 63, 1 << 63]
    );
    // The greatest number and levels share exactly too, with the odd unit going to the earlier pop on a tie.
    let building = &mut place.buildings[building];
    building.levels = u32::MAX;
    rules
        .buildings
        .iter_mut()
        .find(|item| item.id == "demesne")
        .unwrap()
        .market_capacity_ml_per_level = u64::MAX;
    let whole = u128::from(u32::MAX) * u128::from(u64::MAX);
    let shares = market_capacities(&rules, &place).unwrap();
    assert_eq!(shares.iter().sum::<u128>(), whole);
    assert_eq!(shares[0] - shares[1], whole % 2);
}

#[test]
fn the_wide_shares_are_the_narrow_ones_where_both_fit() {
    for (total, weights) in [
        (7_u64, vec![1_u64, 1, 1]),
        (0, vec![1, 2]),
        (1_000_003, vec![3, 0, 5, 9]),
        (u64::MAX, vec![u64::MAX, 1]),
        (12, vec![0, 0]),
    ] {
        let narrow = largest_remainder(total, &weights).unwrap();
        let wide = swap::wide_shares(u128::from(total), &weights).unwrap();
        assert_eq!(
            wide,
            narrow.iter().map(|&v| u128::from(v)).collect::<Vec<_>>()
        );
    }
}

#[test]
fn the_room_case_three_bushels_for_ten_pounds() {
    // As A, but the peasants' store can take only 12 lb more of fish. 12 lb would need 4 bushels (404 for 360) and 11 lb would need 4
    // (404 for 330): both lose more than a tenth. 10 lb needs 3 (303 for 300): 3 bushels for 10 lb.
    let (mut rules, mut place) = example_a(PEASANT, 10_000_000);
    narrow_fish_room(&mut rules, &mut place, 12);
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, MONK, "oats"), (0, 3));
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (17, 0));
    assert_eq!(position(&rules, &place, PEASANT, "fish"), (0, 30));
}

#[test]
fn goods_received_are_refused_room_for_a_good_the_storehouse_does_not_take() {
    // Honey is no storehouse kind: with the peasants' store full of it and no room, the pair is refused whole.
    let (mut rules, mut place) = example_b2(4);
    let honey = good(&rules, "honey");
    let people = place.pops[PEASANT].workers + place.pops[PEASANT].dependents;
    let held = place.pops[PEASANT].stock[honey];
    rules.goods[honey].pop_limit_milli = held * 1000 / people;
    let house = add_building(&rules, &mut place, "storehouse", 1);
    place.buildings[house].method_levels = vec![1, 1, 1];
    place.buildings[house].storehouse_accounts = Some(StorehouseAccounts {
        pops: vec![vec![0; rules.goods.len()]; 2],
        communal: vec![0; rules.goods.len()],
    });
    let before = goods_everywhere(&place);
    exchange(&rules, &mut place);
    assert_eq!(goods_everywhere(&place), before);
}

pub(crate) fn storehouse(rules: &Rules, place: &mut PlaceState) -> usize {
    let house = add_building(rules, place, "storehouse", 1);
    place.buildings[house].method_levels = vec![1, 1, 1];
    place.buildings[house].storehouse_accounts = Some(StorehouseAccounts {
        pops: vec![vec![0; rules.goods.len()]; place.pops.len()],
        communal: vec![0; rules.goods.len()],
    });
    house
}

pub(crate) fn account(
    place: &mut PlaceState,
    house: usize,
    pop: usize,
    good_index: usize,
    units: u64,
) {
    place.buildings[house]
        .storehouse_accounts
        .as_mut()
        .unwrap()
        .pops[pop][good_index] += units;
    place.buildings[house].stock[good_index] += units;
}

fn account_of(place: &PlaceState, house: usize, pop: usize, id_index: usize) -> u64 {
    place.buildings[house]
        .storehouse_accounts
        .as_ref()
        .unwrap()
        .pops[pop][id_index]
}

#[test]
fn goods_given_leave_the_store_before_the_account() {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    small(&mut place, 10);
    set_price(&rules, &mut place, "oats", 101);
    set_price(&rules, &mut place, "fish", 30);
    let house = storehouse(&rules, &mut place);
    let oats = good(&rules, "oats");
    // The peasants hold their 20 spare bushels as 4 in the store and the rest in the account.
    let goal = target(&rules, &place, PEASANT, "oats");
    place.pops[PEASANT].stock[oats] = 4;
    account(&mut place, house, PEASANT, oats, goal + 16);
    want(&rules, &mut place, PEASANT, "fish", 40);
    spare(&rules, &mut place, MONK, "fish", 25);
    want(&rules, &mut place, MONK, "oats", 6);
    settle_the_rest(&rules, &mut place, &["oats", "fish"]);
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (20, 0));
    exchange(&rules, &mut place);
    // 6 bushels left: the store's 4 first, then 2 from the account.
    assert_eq!(place.pops[PEASANT].stock[oats], 0);
    assert_eq!(account_of(&place, house, PEASANT, oats), goal + 14);
    assert_eq!(place.buildings[house].stock[oats], goal + 14);
    assert_eq!(
        place.pops[MONK].stock[oats],
        target(&rules, &place, MONK, "oats")
    );
}

/// Fills the storehouse's grain kind with the communal account's filler so that exactly `free_ml` of its volume is free.
fn fill_grain_room_to(rules: &mut Rules, place: &mut PlaceState, house: usize, free_ml: u128) {
    let peas = good(rules, "peas_beans");
    rules.goods[peas].volume_ml = 1;
    let capacity = 36_000_000_u128;
    let used: u128 = rules
        .goods
        .iter()
        .zip(&place.buildings[house].stock)
        .filter(|(item, _)| storehouse_group(&item.id) == Some(0))
        .map(|(item, units)| u128::from(item.volume_ml) * u128::from(*units))
        .sum();
    let filler = u64::try_from(capacity - free_ml - used).unwrap();
    place.buildings[house]
        .storehouse_accounts
        .as_mut()
        .unwrap()
        .communal[peas] = filler;
    place.buildings[house].stock[peas] = filler;
}

/// A pop's store takes no more of a good than it holds.
fn at_its_limit(rules: &mut Rules, place: &PlaceState, pop: usize, id: &str) {
    let index = good(rules, id);
    let people = place.pops[pop].workers + place.pops[pop].dependents;
    rules.goods[index].pop_limit_milli = place.pops[pop].stock[index] * 1000 / people;
    // The limit's room is none: at most what the pop holds (the division may round the limit down a unit or two).
    assert!(
        u128::from(rules.goods[index].pop_limit_milli) * u128::from(people) / 1000
            <= u128::from(place.pops[pop].stock[index]),
        "{id}: the pop's store has no room for more"
    );
}

// ---------------------------------------------------------------- the exchange: order, turns and wants

#[test]
fn scarce_fish_goes_to_the_wealthier_claimant_before_want_order() {
    // A holder wants barley first and oats second. Both buyers want its one fish and can pay; equal wealth gives it to the earlier buyer.
    for third_has_capacity in [false, true] {
        let mut rules = rules();
        let mut place = place(&rules, 3);
        set_price(&rules, &mut place, "oats", 100);
        set_price(&rules, &mut place, "barley", 100);
        set_price(&rules, &mut place, "fish", 100);
        let barley = good(&rules, "barley");
        rules.goods[barley].volume_ml = 1;
        spare(&rules, &mut place, PEASANT, "fish", 1);
        want(&rules, &mut place, PEASANT, "barley", 1);
        want(&rules, &mut place, PEASANT, "oats", 1);
        want(&rules, &mut place, MONK, "fish", 1);
        spare(&rules, &mut place, MONK, "oats", 1);
        want(&rules, &mut place, THIRD, "fish", 1);
        spare(&rules, &mut place, THIRD, "barley", 1);
        settle_the_rest(&rules, &mut place, &["oats", "barley", "fish"]);
        capacity(&mut rules, &mut place, PEASANT, 10_000_000);
        if third_has_capacity {
            capacity(&mut rules, &mut place, THIRD, 10_000_000);
        }
        // Barley comes before oats in the holder's wants: its volume for a value unit is the smaller at these prices.
        let order = want_order(&rules, &place.prices_milli);
        let rank = |id: &str| order.iter().position(|i| *i == good(&rules, id)).unwrap();
        assert!(rank("barley") < rank("oats"));
        exchange(&rules, &mut place);
        // Equal wealth is decided by pop order, so the oats holder receives the fish.
        assert_eq!(position(&rules, &place, MONK, "fish"), (0, 0));
        assert_eq!(position(&rules, &place, THIRD, "fish"), (0, 1));
        assert_eq!(position(&rules, &place, PEASANT, "oats"), (0, 0));
        assert_eq!(position(&rules, &place, PEASANT, "barley"), (0, 1));
    }
}

#[test]
fn wants_go_food_first_then_the_least_bulky_for_its_worth_then_the_goods_order() {
    let rules = rules();
    let prices = rules
        .goods
        .iter()
        .map(|item| item.base_price * 1000)
        .collect::<Vec<_>>();
    let order = want_order(&rules, &prices);
    let rank = |id: &str| {
        order
            .iter()
            .position(|index| *index == good(&rules, id))
            .unwrap()
    };
    // Every good with a hunger value comes before every one without.
    for item in &rules.goods {
        if item.hunger_centi == 0 {
            for food in rules.goods.iter().filter(|food| food.hunger_centi > 0) {
                assert!(rank(&food.id) < rank(&item.id));
            }
        }
    }
    // Within food, the smaller volume for a value unit comes first: honey (350 ml at 120) before fish (1,000 ml at 30) before oats
    // (35,000 ml at 100).
    assert!(rank("honey") < rank("fish"));
    assert!(rank("fish") < rank("oats"));
    // The same price for oats and barley and the same volume: the goods' order.
    let mut level = prices.clone();
    level[good(&rules, "oats")] = 100_000;
    level[good(&rules, "barley")] = 100_000;
    let order = want_order(&rules, &level);
    let oats = order
        .iter()
        .position(|i| *i == good(&rules, "oats"))
        .unwrap();
    let barley = order
        .iter()
        .position(|i| *i == good(&rules, "barley"))
        .unwrap();
    assert!(oats < barley);
}

#[test]
fn a_starter_tries_the_partners_in_the_places_order_and_each_try_makes_the_swap() {
    // The first pop wants fish, and both others want its oats. The scarce oats are shared even though it tries partners in order.
    let mut rules = rules();
    let mut place = place(&rules, 3);
    set_price(&rules, &mut place, "oats", 100);
    set_price(&rules, &mut place, "fish", 100);
    spare(&rules, &mut place, PEASANT, "oats", 10);
    want(&rules, &mut place, PEASANT, "fish", 10);
    spare(&rules, &mut place, MONK, "fish", 6);
    want(&rules, &mut place, MONK, "oats", 6);
    spare(&rules, &mut place, THIRD, "fish", 6);
    want(&rules, &mut place, THIRD, "oats", 6);
    settle_the_rest(&rules, &mut place, &["oats", "fish"]);
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, PEASANT, "fish"), (0, 0));
    assert_eq!(position(&rules, &place, MONK, "fish"), (1, 0));
    assert_eq!(position(&rules, &place, THIRD, "fish"), (1, 0));
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (0, 0));
    assert_eq!(position(&rules, &place, MONK, "oats"), (0, 1));
    assert_eq!(position(&rules, &place, THIRD, "oats"), (0, 1));
}

// ---------------------------------------------------------------- hunger

/// Three pops of six people, priced alike at 10 so that a swap is one for one. The first is the swap's other side, the second
/// the pop whose hunger a case tests.
struct Hunger {
    rules: Rules,
    place: PlaceState,
    house: usize,
}

fn hunger_place() -> Hunger {
    let rules = hunger_rules();
    let mut place = place(&rules, 3);
    small(&mut place, 6);
    for id in ["fish", "herbs", "beeswax"] {
        set_price(&rules, &mut place, id, 10);
    }
    settle_the_rest(&rules, &mut place, &[]);
    let house = storehouse(&rules, &mut place);
    // The cases give capacity to the pops that have turns.
    Hunger {
        rules,
        place,
        house,
    }
}

/// Six people eat 600 centi a day and a pound of fish gives 60: ten pounds a day.
fn days_of_fish(days: u64) -> u64 {
    days * 10
}

#[test]
fn a_pop_with_59_days_in_its_store_and_2_in_its_account_is_not_hungry_and_works_a_comfort_want() {
    let Hunger {
        mut rules,
        mut place,
        house,
    } = hunger_place();
    let fish = good(&rules, "fish");
    // The pop being tested is the second; the first has beeswax to spare and wants nothing; the third is the swap's partner.
    place.pops[MONK].stock[fish] = days_of_fish(59);
    account(&mut place, house, MONK, fish, days_of_fish(2));
    want(&rules, &mut place, MONK, "herbs", 20);
    spare(&rules, &mut place, MONK, "beeswax", 50);
    spare(&rules, &mut place, THIRD, "herbs", 20);
    want(&rules, &mut place, THIRD, "beeswax", 50);
    capacity(&mut rules, &mut place, MONK, 10_000_000);
    let mut place2 = place.clone();
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, MONK, "herbs"), (0, 0));
    // The same pop with 59 days in all is hungry: it works no comfort want.
    place2.buildings[house]
        .storehouse_accounts
        .as_mut()
        .unwrap()
        .pops[MONK][fish] = days_of_fish(0);
    place2.buildings[house].stock[fish] = 0;
    let before = goods_everywhere(&place2);
    exchange(&rules, &mut place2);
    assert_eq!(goods_everywhere(&place2), before);
    assert_eq!(position(&rules, &place2, MONK, "herbs"), (0, 20));
}

#[test]
fn a_hungry_pop_that_a_first_swap_takes_past_sixty_days_still_leaves_its_comfort_wants_alone() {
    let Hunger {
        mut rules,
        mut place,
        ..
    } = hunger_place();
    let fish = good(&rules, "fish");
    place.pops[MONK].stock[fish] = days_of_fish(59);
    spare(&rules, &mut place, MONK, "beeswax", 50);
    want(&rules, &mut place, MONK, "herbs", 20);
    // The third pop has fish and herbs to spare and wants beeswax: a food swap first, then a comfort swap that is not made.
    spare(&rules, &mut place, THIRD, "fish", 30);
    spare(&rules, &mut place, THIRD, "herbs", 20);
    want(&rules, &mut place, THIRD, "beeswax", 50);
    capacity(&mut rules, &mut place, MONK, 10_000_000);
    let before_fish = place.pops[MONK].stock[fish];
    exchange(&rules, &mut place);
    // 30 lb of fish for 30 of beeswax, which takes the pop to 62 days; the herbs are not swapped for.
    assert_eq!(place.pops[MONK].stock[fish], before_fish + 30);
    assert_eq!(position(&rules, &place, MONK, "beeswax"), (20, 0));
    assert_eq!(position(&rules, &place, MONK, "herbs"), (0, 20));
    assert_eq!(position(&rules, &place, THIRD, "herbs"), (20, 0));
}

#[test]
fn the_same_hungry_pop_as_a_partner_receives_a_good_without_a_hunger_value() {
    let Hunger {
        mut rules,
        mut place,
        ..
    } = hunger_place();
    let fish = good(&rules, "fish");
    // The peasants, not hungry, start for beeswax; the hungry monks (59 days) want herbs and have beeswax to spare.
    place.pops[MONK].stock[fish] = days_of_fish(59);
    spare(&rules, &mut place, MONK, "beeswax", 20);
    want(&rules, &mut place, MONK, "herbs", 20);
    want(&rules, &mut place, PEASANT, "beeswax", 20);
    spare(&rules, &mut place, PEASANT, "herbs", 20);
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, MONK, "herbs"), (0, 0));
    assert_eq!(position(&rules, &place, PEASANT, "beeswax"), (0, 0));
}

#[test]
fn the_pantry_is_not_counted_and_hunger_is_tested_once_when_the_exchange_begins() {
    let Hunger {
        mut rules,
        mut place,
        ..
    } = hunger_place();
    let fish = good(&rules, "fish");
    // The monks hold 59 days in store and account and two more opened in the pantry: hungry. The peasants, first in the order, start
    // for beeswax and give the monks two days of fish; in their own turn the monks have a comfort swap they could make and do not.
    place.pops[MONK].stock[fish] = days_of_fish(59);
    place.pops[MONK].consumption.pantry_hunger_centi[fish] = 1200;
    spare(&rules, &mut place, MONK, "beeswax", 40);
    want(&rules, &mut place, MONK, "herbs", 20);
    want(&rules, &mut place, PEASANT, "beeswax", 20);
    spare(&rules, &mut place, PEASANT, "fish", 20);
    spare(&rules, &mut place, THIRD, "herbs", 20);
    want(&rules, &mut place, THIRD, "beeswax", 20);
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    capacity(&mut rules, &mut place, MONK, 10_000_000);
    exchange(&rules, &mut place);
    // Received as a partner before its turn: two days of food (20 lb of fish) for 20 of beeswax.
    assert_eq!(place.pops[MONK].stock[fish], days_of_fish(61));
    assert_eq!(position(&rules, &place, MONK, "beeswax"), (20, 0));
    // In its own turn it makes no comfort swap that day.
    assert_eq!(position(&rules, &place, MONK, "herbs"), (0, 20));
    assert_eq!(position(&rules, &place, THIRD, "herbs"), (20, 0));
}

// ---------------------------------------------------------------- nothing else changes

#[test]
fn no_swap_no_goods_move_and_the_day_is_todays_day() {
    // One pop that holds a pound of wool above its target (none), where nobody wants wool: no stock moves.
    let mut rules = rules();
    let mut place = place(&rules, 1);
    place.pops[PEASANT].stock[good(&rules, "wool")] = 1;
    capacity(&mut rules, &mut place, PEASANT, 1_000_000);
    let before = place.clone();
    exchange(&rules, &mut place);
    assert_eq!(place, before);
}

#[test]
fn the_price_step_sees_a_pops_spare_wool_as_a_sell_order() {
    let rules = rules();
    let mut place = place(&rules, 1);
    let wool = good(&rules, "wool");
    place.pops[PEASANT].stock[wool] = 1;
    place.prices_milli[wool] = 40_000;
    let mut state = EconomyState {
        places: vec![place],
    };
    update_daily_prices(&rules, &mut state).unwrap();
    assert_eq!(
        state.places[0]
            .last_market_orders
            .as_ref()
            .unwrap()
            .sell_units[wool],
        1
    );
}

#[test]
fn the_position_type_is_plain_data() {
    let found = Position { spare: 3, want: 0 };
    assert_eq!(found, Position { spare: 3, want: 0 });
}

// ---------------------------------------------------------------- the exchange in the day

fn pasture(cells: u32) -> LandCounts {
    LandCounts {
        land_cells: BTreeMap::from([("pasture".to_owned(), cells)]),
        fertility_pm: BTreeMap::from([("pasture".to_owned(), u64::from(cells) * 600)]),
        fertile_cells: cells,
        fertile_cells_by_terrain: BTreeMap::from([("pasture".to_owned(), cells)]),
        sea: 0,
    }
}

/// One ringfort `fort` of 100 peasants and a noble house of 40 on a demesne of two levels, with its 20 labourers.
fn fort(rules: &Rules) -> EconomyState {
    let data = ScenarioEconomy {
        horses_always: Vec::new(),
        defaults: BTreeMap::from([(
            "ringfort".to_owned(),
            BTreeMap::from([(
                "1".to_owned(),
                StartDefaults {
                    peasants: 100,
                    monks: 0,
                    labourers: 0,
                    farm_levels: 0,
                    holdings_levels: 0,
                    storehouse_levels: 0,
                    pop_stock: BTreeMap::new(),
                    building_stock: BTreeMap::new(),
                },
            )]),
        )]),
        places: BTreeMap::from([(
            "fort".to_owned(),
            StartOverride {
                culture: Some("ulaid".to_owned()),
                religion: Some("christianity".to_owned()),
                monk_religion: Some("christianity".to_owned()),
                ..StartOverride::default()
            },
        )]),
        nobles: BTreeMap::from([(
            "fort".to_owned(),
            NobleStart {
                culture: "ulaid".to_owned(),
                religion: "christianity".to_owned(),
                people: 40,
                demesne_levels: 2,
                labourers: 20,
            },
        )]),
    };
    starting_state(
        rules,
        &data,
        &[StartLocation {
            id: "fort".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: pasture(40),
        }],
        9,
    )
    .unwrap()
    .0
}

fn kind_of(place: &PlaceState, kind: &str) -> usize {
    place.pops.iter().position(|pop| pop.kind == kind).unwrap()
}

fn on(month: u8, day: u8) -> Date {
    Date {
        year: 768,
        month,
        day,
        hour: 12,
    }
}

fn one_day(rules: &Rules, state: &mut EconomyState, date: Date) -> ProductionLedger {
    run_daily_economy_with_context(
        rules,
        state,
        date,
        &BTreeMap::new(),
        &BTreeMap::from([("fort".to_owned(), 30)]),
        &BTreeMap::new(),
    )
    .unwrap()
}

/// The same fort with the nobles' capacity taken away: no swap can be made.
fn without_capacity(rules: &Rules) -> Rules {
    let mut rules = rules.clone();
    for item in &mut rules.buildings {
        item.market_capacity_ml_per_level = 0;
    }
    rules
}

#[test]
fn a_pop_that_swaps_for_food_at_noon_eats_it_the_same_day() {
    let rules = hunger_rules();
    let mut state = fort(&rules);
    let place = &mut state.places[0];
    let (labourers, nobles) = (kind_of(place, "labourer"), kind_of(place, "noble"));
    // The labourers hold no food, only beeswax the nobles want; the nobles hold fish above their target, which the labourers want.
    for pop in 0..place.pops.len() {
        place.pops[pop].stock = vec![0; rules.goods.len()];
    }
    let fish_target = |place: &PlaceState, pop: usize| target(&rules, place, pop, "fish");
    spare(&rules, place, labourers, "beeswax", 6);
    want(&rules, place, nobles, "beeswax", 6);
    let held = fish_target(place, nobles) + 60;
    place.pops[nobles].stock[good(&rules, "fish")] = held;
    for id in ["fish", "beeswax"] {
        set_price(&rules, place, id, 10);
    }
    let day = on(9, 15);
    let mut swapped = state.clone();
    let ledger = one_day(&rules, &mut swapped, day);
    let mut idle = state;
    let idle_ledger = one_day(&without_capacity(&rules), &mut idle, day);
    // Without the exchange the labourers have nothing to eat; with it they ate the fish they swapped for that noon.
    assert_eq!(idle_ledger.intake_pm[0][labourers], 0);
    assert!(ledger.intake_pm[0][labourers] > 0);
    let fish = good(&rules, "fish");
    assert!(swapped.places[0].pops[nobles].stock[fish] < held);
}

#[test]
fn goods_paid_on_the_first_are_swapped_that_noon_and_not_before_the_payday() {
    let mut rules = rules();
    // A pop's room is its store as it stands (rule 13): at the shipped 35 litres a bushel the payday would fill the labourers' store
    // and leave no room for the fish, so the oats are made small, the case being about the order of the day.
    let oats = good(&rules, "oats");
    rules.goods[oats].volume_ml = 1000;
    // The nobles' day's capacity is ample: the case is about the order of the day, not its limit.
    rules
        .buildings
        .iter_mut()
        .find(|item| item.id == "demesne")
        .unwrap()
        .market_capacity_ml_per_level = 1_000_000_000_000;
    let state = fort(&rules);
    let (labourers, nobles) = (
        kind_of(&state.places[0], "labourer"),
        kind_of(&state.places[0], "noble"),
    );
    let oats = good(&rules, "oats");
    let fish = good(&rules, "fish");
    // What the demesne's payday gives the labourers in oats.
    let mut paid = state.clone();
    run_monthly_production(&rules, &mut paid, on(9, 1)).unwrap();
    let pay =
        paid.places[0].pops[labourers].stock[oats] - state.places[0].pops[labourers].stock[oats];
    assert!(pay >= 2, "the labourers are paid oats on the 1st: {pay}");
    // The labourers hold exactly their target of oats before the payday, so the pay is all spare after it; the nobles want a few bushels
    // of it and have fish above their target, which the labourers want.
    let mut state = state;
    let place = &mut state.places[0];
    for pop in 0..place.pops.len() {
        place.pops[pop].stock = vec![0; rules.goods.len()];
        for item in &rules.goods {
            let goal = target(&rules, place, pop, &item.id);
            place.pops[pop].stock[good(&rules, &item.id)] = goal;
        }
    }
    let wanted = (pay / 2).min(10);
    want(&rules, place, nobles, "oats", wanted);
    spare(&rules, place, nobles, "fish", 80);
    want(&rules, place, labourers, "fish", 80);
    // Before the payday the labourers have nothing to give: a swap run then makes none.
    let mut early = state.clone();
    swap::run_daily_swaps(&rules, &mut early).unwrap();
    assert_eq!(early.places[0].pops, state.places[0].pops);
    // The day of the 1st: the exchange runs after the payday, so the labourers give the nobles the oats they want.
    let mut day = state.clone();
    one_day(&rules, &mut day, on(9, 1));
    let mut idle = state.clone();
    one_day(&without_capacity(&rules), &mut idle, on(9, 1));
    let gained = |after: &EconomyState, pop: usize, index: usize| {
        i128::from(after.places[0].pops[pop].stock[index])
    };
    assert_eq!(
        gained(&day, nobles, oats) - gained(&idle, nobles, oats),
        i128::from(wanted)
    );
    assert_eq!(
        gained(&idle, labourers, oats) - gained(&day, labourers, oats),
        i128::from(wanted)
    );
    // The fish went the other way, from the nobles to the labourers, in whole units.
    let moved = gained(&day, labourers, fish) - gained(&idle, labourers, fish);
    assert!(moved > 0);
    assert_eq!(
        gained(&idle, nobles, fish) - gained(&day, nobles, fish),
        moved
    );
}

#[test]
fn rule_20_the_price_after_the_first_noon_is_the_literal_the_formula_gives_for_the_new_order() {
    // One pop that holds a pound of wool above its target, where nobody wants wool and no building offers any, at a price of 40: no
    // stock moves; the place's sell order for wool is one unit; and the price after the first noon is the one the formula gives.
    let rules = rules();
    let mut place = place(&rules, 1);
    let wool = good(&rules, "wool");
    place.pops[PEASANT].stock[wool] = 1;
    set_price(&rules, &mut place, "wool", 40);
    let mut state = EconomyState {
        places: vec![place],
    };
    let before = state.places[0].pops.clone();
    update_daily_prices(&rules, &mut state).unwrap();
    swap::run_daily_swaps(&rules, &mut state).unwrap();
    assert_eq!(state.places[0].pops, before);
    let orders = state.places[0].last_market_orders.as_ref().unwrap();
    assert_eq!(orders.sell_units[wool], 1);
    let expected = daily_price_target(&rules, &rules.goods[wool], 40_000, 0, 1).unwrap();
    assert!(expected < 40_000);
    let drifted = state.places[0].prices_milli[wool];
    assert!(drifted < 40_000 && drifted >= expected);
    // Review 1 computed 38 (where today the price stays 40).
    assert_eq!(drifted / 1000, 38);
}

#[test]
fn a_short_good_is_shared_across_turns() {
    // The first two pops each want 5 fish and have oats to give; the third has only 5 fish. The indivisible unit goes to the earlier equal-wealth buyer.
    for (first, second) in [(PEASANT, MONK), (MONK, PEASANT)] {
        let mut rules = rules();
        let mut place = place(&rules, 3);
        set_price(&rules, &mut place, "oats", 100);
        set_price(&rules, &mut place, "fish", 100);
        for pop in [PEASANT, MONK] {
            want(&rules, &mut place, pop, "fish", 5);
            spare(&rules, &mut place, pop, "oats", 5);
        }
        spare(&rules, &mut place, THIRD, "fish", 5);
        want(&rules, &mut place, THIRD, "oats", 10);
        settle_the_rest(&rules, &mut place, &["oats", "fish"]);
        capacity(&mut rules, &mut place, first, 10_000_000);
        capacity(&mut rules, &mut place, second, 10_000_000);
        // Both receive their share regardless of which capacity building was added first.
        exchange(&rules, &mut place);
        let earlier = first.min(second);
        let later = first.max(second);
        assert_eq!(position(&rules, &place, earlier, "fish"), (0, 2));
        assert_eq!(position(&rules, &place, later, "fish"), (0, 3));
    }
}

// ---------------------------------------------------------------- the search is bounded; the allowance is spent

#[test]
fn a_huge_want_and_spare_with_a_small_allowance_is_one_for_one_at_once() {
    // Equal prices, one bushel of oats to pay with, and fish wanted and spared by the billion, with the 50 litres of a farm level: a
    // pound of fish takes a litre, a bushel 35, so the only pair is one bushel for one pound. The search starts from what can be
    // carried, not from the whole of the want.
    let mut rules = rules();
    let mut place = place(&rules, 2);
    for pop in &mut place.pops {
        pop.workers = 100_000_000;
        pop.dependents = 0;
    }
    set_price(&rules, &mut place, "oats", 100);
    set_price(&rules, &mut place, "fish", 100);
    spare(&rules, &mut place, PEASANT, "oats", 1);
    want(&rules, &mut place, PEASANT, "fish", 8_000_000_000);
    spare(&rules, &mut place, MONK, "fish", 8_000_000_000);
    want(&rules, &mut place, MONK, "oats", 1);
    settle_the_rest(&rules, &mut place, &["oats", "fish"]);
    capacity(&mut rules, &mut place, PEASANT, 50_000);
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, PEASANT, "oats"), (0, 0));
    assert_eq!(
        position(&rules, &place, PEASANT, "fish"),
        (0, 7_999_999_999)
    );
    assert_eq!(position(&rules, &place, MONK, "oats"), (0, 0));
    assert_eq!(position(&rules, &place, MONK, "fish"), (7_999_999_999, 0));
}

/// The first pop starts for fish with `litres` of allowance. The second and third each have 6 fish to spare and want 6 bushels (oats
/// and fish at 100: 6 for 6 takes 6 x 35 + 6 = 216 litres).
fn two_partners(starter_ml: u64) -> (Rules, PlaceState) {
    let mut rules = rules();
    let mut place = place(&rules, 3);
    set_price(&rules, &mut place, "oats", 100);
    set_price(&rules, &mut place, "fish", 100);
    spare(&rules, &mut place, PEASANT, "oats", 12);
    want(&rules, &mut place, PEASANT, "fish", 12);
    for pop in [MONK, THIRD] {
        spare(&rules, &mut place, pop, "fish", 6);
        want(&rules, &mut place, pop, "oats", 6);
    }
    settle_the_rest(&rules, &mut place, &["oats", "fish"]);
    capacity(&mut rules, &mut place, PEASANT, starter_ml);
    (rules, place)
}

#[test]
fn the_starters_allowance_is_spent_by_both_loads_of_each_swap_and_the_next_partner_gets_what_is_left()
 {
    // 216 litres is exactly the first partner's pair; the second partner's otherwise eligible swap finds nothing left.
    let (rules, mut place) = two_partners(216_000);
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, MONK, "fish"), (0, 0));
    assert_eq!(position(&rules, &place, MONK, "oats"), (0, 0));
    assert_eq!(position(&rules, &place, THIRD, "fish"), (6, 0));
    assert_eq!(position(&rules, &place, THIRD, "oats"), (0, 6));
    assert_eq!(position(&rules, &place, PEASANT, "fish"), (0, 6));
    // 288 litres leaves 72: the second pair is cut to the 2 bushels and 2 lb that fit (72), not the 6 and 6.
    let (rules, mut place) = two_partners(288_000);
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, MONK, "oats"), (0, 0));
    assert_eq!(position(&rules, &place, THIRD, "fish"), (4, 0));
    assert_eq!(position(&rules, &place, THIRD, "oats"), (0, 4));
    assert_eq!(position(&rules, &place, PEASANT, "fish"), (0, 4));
    // Each day's allowance is worked out afresh: the next exchange has another 288 litres.
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, THIRD, "fish"), (0, 0));
    assert_eq!(position(&rules, &place, THIRD, "oats"), (0, 0));
    assert_eq!(position(&rules, &place, PEASANT, "fish"), (0, 0));
}

#[test]
fn a_partner_is_not_charged_and_keeps_its_own_allowance_for_its_own_turn() {
    // The first pop's swap with the second uses the first's 216 litres and none of the second's 36, which the second then spends
    // in its own turn on a swap of one bushel of barley for one pound of fish (35 + 1).
    let mut rules = rules();
    let mut place = place(&rules, 3);
    for id in ["oats", "barley", "fish"] {
        set_price(&rules, &mut place, id, 100);
    }
    spare(&rules, &mut place, PEASANT, "oats", 6);
    want(&rules, &mut place, PEASANT, "fish", 6);
    spare(&rules, &mut place, MONK, "fish", 7);
    want(&rules, &mut place, MONK, "oats", 6);
    want(&rules, &mut place, MONK, "barley", 1);
    spare(&rules, &mut place, THIRD, "barley", 1);
    want(&rules, &mut place, THIRD, "fish", 1);
    settle_the_rest(&rules, &mut place, &["oats", "barley", "fish"]);
    capacity(&mut rules, &mut place, PEASANT, 216_000);
    capacity(&mut rules, &mut place, MONK, 36_000);
    exchange(&rules, &mut place);
    assert_eq!(position(&rules, &place, PEASANT, "fish"), (0, 0));
    assert_eq!(position(&rules, &place, MONK, "oats"), (0, 0));
    assert_eq!(position(&rules, &place, MONK, "barley"), (0, 0));
    assert_eq!(position(&rules, &place, THIRD, "fish"), (0, 0));
    assert_eq!(position(&rules, &place, THIRD, "barley"), (0, 0));
    assert_eq!(position(&rules, &place, MONK, "fish"), (0, 0));
}

// ---------------------------------------------------------------- rule 20: the accepted build's day

/// Two pops with market capacity for one of them, where no pop holds a good without a hunger value above its target and nobody has
/// what the other wants: the peasants hold oats above target and want fish, the monks the same with other figures, and neither has
/// fish to spare. The exchange makes no swap and no order changes.
fn nothing_to_swap() -> (Rules, EconomyState) {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    spare(&rules, &mut place, PEASANT, "oats", 10);
    want(&rules, &mut place, PEASANT, "fish", 5);
    spare(&rules, &mut place, MONK, "oats", 3);
    want(&rules, &mut place, MONK, "fish", 7);
    settle_the_rest(&rules, &mut place, &["oats", "fish"]);
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    (
        rules,
        EconomyState {
            places: vec![place],
        },
    )
}

#[test]
fn a_day_with_no_swap_and_no_new_order_ends_exactly_as_the_accepted_build_did() {
    let (rules, mut state) = nothing_to_swap();
    let place = &state.places[0];
    // No swap is possible, and the orders are what they were: food only, since nobody holds a good without a hunger value above target.
    let mut idle = state.clone();
    swap::run_daily_swaps(&rules, &mut idle).unwrap();
    assert_eq!(idle, state);
    let orders = daily_market_orders(&rules, place).unwrap();
    for (index, item) in rules.goods.iter().enumerate() {
        if item.hunger_centi == 0 {
            assert_eq!(orders.sell_units[index], 0, "{}", item.id);
        }
    }
    // The whole day through the economy's own entry point, to the stores, prices and price carries the accepted build (before this
    // slice) gave for this fixture on the same day.
    run_daily_economy_with_context(
        &rules,
        &mut state,
        on(9, 15),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
    )
    .unwrap();
    let place = &state.places[0];
    let held = |pop: usize| -> Vec<(&str, u64)> {
        rules
            .goods
            .iter()
            .zip(&place.pops[pop].stock)
            .filter(|(_, units)| **units > 0)
            .map(|(item, units)| (item.id.as_str(), *units))
            .collect()
    };
    assert_eq!(held(PEASANT), ACCEPTED_PEASANTS);
    assert_eq!(held(MONK), ACCEPTED_MONKS);
    // Every price of a good the accepted build had, and every price carry (all zero), to the unit.
    for (id, price) in ACCEPTED_PRICES {
        assert_eq!(place.prices_milli[good(&rules, id)], price, "{id}");
    }
    assert!(place.price_drift_carries.iter().all(|carry| *carry == 0));
}

// What the accepted build (the build before this slice) left of the day above, taken by running the same fixture there: each pop's
// non-zero stock and every price in thousandths. The prices are keyed by good and cover the 22 goods every build has; the accepted build
// with CK-392 also has linen and hemp, at their base prices, which no order here moves.
const ACCEPTED_PEASANTS: [(&str, u64); 5] = [
    ("oats", 5321),
    ("barley", 1707),
    ("fish", 79661),
    ("honey", 68286),
    ("herbs", 11999),
];
const ACCEPTED_MONKS: [(&str, u64); 5] = [
    ("oats", 5314),
    ("barley", 1707),
    ("fish", 79659),
    ("honey", 68286),
    ("herbs", 11994),
];
const ACCEPTED_PRICES: [(&str, u64); 22] = [
    ("oats", 95_000),
    ("barley", 120_000),
    ("peas_beans", 120_000),
    ("vegetables", 40_000),
    ("apples", 150_000),
    ("dairy", 40_000),
    ("beef", 10_000),
    ("pork", 9_000),
    ("mutton", 8_000),
    ("fish", 31_500),
    ("seabirds", 20_000),
    ("ale", 60_000),
    ("mead", 300_000),
    ("wine", 600_000),
    ("honey", 120_000),
    ("herbs", 100_000),
    ("wool", 40_000),
    ("beeswax", 150_000),
    ("hides", 200_000),
    ("cattle", 2_400_000),
    ("pigs", 600_000),
    ("sheep", 300_000),
];

// ---------------------------------------------------------------- impossible room is never searched pair by pair

/// An exchange that must not stall: the cases below took seconds when impossible receipts were staged one pair at a time, and take
/// a few hundredths when they are ruled out up front; the bound is a hundred times that, so load does not trip it.
fn exchange_promptly(rules: &Rules, place: &mut PlaceState) {
    let started = std::time::Instant::now();
    exchange(rules, place);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "the exchange took {:?}",
        started.elapsed()
    );
}

/// Oats and fish take a millilitre a unit and are priced alike. The starter, 1,000 people, holds 51,000 oats above its target and wants
/// 59,000 lb of fish; the monks, 20,000 people on an oats-only diet, want 51,000 oats and have 51,000 lb of fish to spare. The
/// starter has 100,000 ml of allowance and no storehouse. The returned rules and place have the starter's fish store at its limit
/// (21 a person, 21,000) when `starter_full`, and the monks' oats store at theirs when not.
fn full_store(starter_full: bool) -> (Rules, PlaceState) {
    let mut rules = rules();
    let (oats, fish) = (good(&rules, "oats"), good(&rules, "fish"));
    rules.goods[oats].volume_ml = 1;
    rules.goods[fish].volume_ml = 1;
    rules.diet_profiles.insert(
        "monk".to_owned(),
        BTreeMap::from([("oats".to_owned(), 1000)]),
    );
    let mut place = place(&rules, 2);
    place.pops[MONK].workers = 20_000;
    place.pops[MONK].dependents = 0;
    place.pops[PEASANT].workers = 1_000;
    place.pops[PEASANT].dependents = 0;
    set_price(&rules, &mut place, "oats", 100);
    set_price(&rules, &mut place, "fish", 100);
    spare(&rules, &mut place, PEASANT, "oats", 51_000);
    want(&rules, &mut place, PEASANT, "fish", 59_000);
    want(&rules, &mut place, MONK, "oats", 51_000);
    spare(&rules, &mut place, MONK, "fish", 51_000);
    settle_the_rest(&rules, &mut place, &["oats", "fish"]);
    capacity(&mut rules, &mut place, PEASANT, 100_000);
    if starter_full {
        rules.goods[fish].pop_limit_milli = 21_000;
        assert_eq!(stock(&rules, &place, PEASANT, "fish"), 21_000);
    } else {
        // The mirror: the monks' oats store is at its limit, so the starter's payment cannot be taken at all.
        at_its_limit(&mut rules, &place, MONK, "oats");
    }
    (rules, place)
}

#[test]
fn a_full_store_that_can_take_no_receipt_makes_no_swap_without_staging_pair_by_pair() {
    let (rules, mut place) = full_store(true);
    // The starter's fish store is at its limit and it has no storehouse: nothing it pays with can make room for a receipt.
    assert_eq!(position(&rules, &place, PEASANT, "fish").1, 59_000);
    let before = goods_everywhere(&place);
    exchange_promptly(&rules, &mut place);
    assert_eq!(goods_everywhere(&place), before);
}

#[test]
fn a_partner_whose_store_is_full_of_what_it_would_receive_makes_no_swap_either() {
    let (rules, mut place) = full_store(false);
    let before = goods_everywhere(&place);
    exchange_promptly(&rules, &mut place);
    assert_eq!(goods_everywhere(&place), before);
}

#[test]
fn a_receipt_that_only_volume_rules_out_is_not_searched_pair_by_pair() {
    // Fish take a litre a pound and oats a millilitre; the starter's store is full to the millilitre. To take one pound it must free
    // 1,000 ml by paying, which takes 1,000 oats: a payment far beyond the tenth the loss limit allows at equal prices. No receipt
    // can be made room for, and none is staged.
    let (mut rules, mut place) = full_store(false);
    let (oats, fish, wool) = (
        good(&rules, "oats"),
        good(&rules, "fish"),
        good(&rules, "wool"),
    );
    rules.goods[fish].volume_ml = 1000;
    rules.goods[wool].volume_ml = 1;
    // The monks can take oats again; only the starter's volume is in the way.
    rules.goods[oats].pop_limit_milli = 1_000_000_000;
    let held: u128 = rules
        .goods
        .iter()
        .zip(&place.pops[PEASANT].stock)
        .map(|(item, units)| u128::from(item.volume_ml) * u128::from(*units))
        .sum();
    assert!(held < 720_000_000);
    place.pops[PEASANT].stock[wool] = u64::try_from(720_000_000 - held).unwrap();
    rules.buildings.iter_mut().for_each(|item| {
        if item.id == "farm" {
            item.market_capacity_ml_per_level = 200_000_000;
        }
    });
    let before = goods_everywhere(&place);
    exchange_promptly(&rules, &mut place);
    assert_eq!(goods_everywhere(&place), before);
}

// ---------------------------------------------------------------- rule 13: receipts go into the pop's own store, as it stands

/// Leaves exactly `free_ml` of a pop's store volume free, with wool (one millilitre a unit), which nobody wants.
fn leave_free(rules: &mut Rules, place: &mut PlaceState, pop: usize, free_ml: u128) {
    let wool = good(rules, "wool");
    rules.goods[wool].volume_ml = 1;
    place.pops[pop].stock[wool] = 0;
    let used = volume_used(rules, &place.pops[pop].stock, None).unwrap();
    let people = u128::from(place.pops[pop].workers + place.pops[pop].dependents);
    place.pops[pop].stock[wool] = u64::try_from(people * 720_000 - used - free_ml).unwrap();
}

#[test]
fn goods_received_go_into_the_store_and_never_into_an_account() {
    // As the room case, with a storehouse that has room to spare: the peasants' fish go into their store, whatever the storehouse
    // could take, and no account of theirs or stock of the storehouse's gains a fish.
    let (mut rules, mut place) = example_a(PEASANT, 10_000_000);
    narrow_fish_room(&mut rules, &mut place, 12);
    let house = storehouse(&rules, &mut place);
    let fish = good(&rules, "fish");
    let held = place.pops[PEASANT].stock[fish];
    exchange(&rules, &mut place);
    assert_eq!(place.pops[PEASANT].stock[fish], held + 10);
    assert_eq!(account_of(&place, house, PEASANT, fish), 0);
    assert_eq!(place.buildings[house].stock[fish], 0);
}

#[test]
fn a_full_store_makes_no_swap_though_the_bushels_given_would_free_room() {
    // As A, but the peasants' store has 5 litres free (a pound of fish is a litre). At most 5 lb fit, and no pair that small passes:
    // 5 lb for 2 bushels loses 52 in 202, 3 lb for 1 bushel loses 11 in 101. The bushels would have freed far more than the fish
    // take, and the room is not credited. With 10 litres free: 3 bushels for 10 lb.
    for (free_ml, bushels, pounds) in [(5_000, 0, 0), (10_000, 3, 10)] {
        let (mut rules, mut place) = example_a(PEASANT, 10_000_000);
        leave_free(&mut rules, &mut place, PEASANT, free_ml);
        let mut hungry = self::place(&rules, 3).pops.remove(2);
        hungry.workers = 1;
        hungry.dependents = 0;
        hungry.employment = vec![Employment::default(); place.buildings.len()];
        for building in &mut place.buildings {
            building.employment.push(Employment::default());
        }
        place.pops.push(hungry);
        let _house = storehouse(&rules, &mut place);
        let church = add_building(&rules, &mut place, "holdings", 1);
        place.buildings[church].employment[THIRD].workers = 1;
        place.pops[THIRD].employment[church].workers = 1;
        let buildings_before = place.buildings.clone();
        let hungry_before = place.pops[THIRD].stock.clone();
        let (oats, fish) = (
            stock(&rules, &place, PEASANT, "oats"),
            stock(&rules, &place, PEASANT, "fish"),
        );
        exchange(&rules, &mut place);
        assert_eq!(stock(&rules, &place, PEASANT, "oats"), oats - bushels);
        assert_eq!(stock(&rules, &place, PEASANT, "fish"), fish + pounds);
        assert_eq!(place.buildings, buildings_before);
        assert_eq!(place.pops[THIRD].stock, hungry_before);
    }
}

#[test]
fn a_store_over_its_volume_receives_nothing_in_either_turn() {
    // As A, but the peasants' store holds more than its volume: no swap in the peasants' turn, and none when the monks start for
    // oats and would pay in fish.
    for starter in [PEASANT, MONK] {
        let (mut rules, mut place) = example_a(starter, 10_000_000);
        let wool = good(&rules, "wool");
        rules.goods[wool].volume_ml = 1;
        let people = u128::from(place.pops[PEASANT].workers + place.pops[PEASANT].dependents);
        let used = volume_used(&rules, &place.pops[PEASANT].stock, None).unwrap();
        place.pops[PEASANT].stock[wool] = u64::try_from(people * 720_000 + 1_000 - used).unwrap();
        let before = goods_everywhere(&place);
        exchange(&rules, &mut place);
        assert_eq!(goods_everywhere(&place), before, "starter {starter}");
    }
}

#[test]
fn a_store_at_its_limit_receives_nothing_and_no_account_changes() {
    // Two pops would swap a bushel each, oats one way and barley the other, at one price. Each pop's store holds its limit of the
    // grain it would receive, and both have accounts in a storehouse with room to spare: no swap.
    let mut rules = rules();
    let mut place = place(&rules, 2);
    small(&mut place, 10);
    set_price(&rules, &mut place, "oats", 100);
    set_price(&rules, &mut place, "barley", 100);
    spare(&rules, &mut place, PEASANT, "oats", 1);
    want(&rules, &mut place, PEASANT, "barley", 1);
    spare(&rules, &mut place, MONK, "barley", 1);
    want(&rules, &mut place, MONK, "oats", 1);
    settle_the_rest(&rules, &mut place, &["oats", "barley"]);
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    let house = storehouse(&rules, &mut place);
    fill_grain_room_to(&mut rules, &mut place, house, 1_000_000);
    at_its_limit(&mut rules, &place, PEASANT, "barley");
    at_its_limit(&mut rules, &place, MONK, "oats");
    let before = goods_everywhere(&place);
    exchange(&rules, &mut place);
    assert_eq!(goods_everywhere(&place), before);
}

#[test]
fn paying_from_the_account_and_a_storehouse_over_its_capacity_change_no_swap() {
    // The first pop holds its spare bushel of oats only in its storehouse account, and its store has room for barley. The second pop
    // gives a bushel of barley from its store, which has room for oats: one for one, with either pop starting, whether the storehouse
    // has room to spare or holds more than its capacity.
    for starter in [PEASANT, MONK] {
        for over_capacity in [false, true] {
            let mut rules = rules();
            let mut place = place(&rules, 2);
            small(&mut place, 10);
            set_price(&rules, &mut place, "oats", 100);
            set_price(&rules, &mut place, "barley", 100);
            let (oats, barley) = (good(&rules, "oats"), good(&rules, "barley"));
            want(&rules, &mut place, PEASANT, "barley", 1);
            want(&rules, &mut place, MONK, "oats", 1);
            spare(&rules, &mut place, MONK, "barley", 1);
            settle_the_rest(&rules, &mut place, &["oats", "barley"]);
            let house = storehouse(&rules, &mut place);
            let goal = target(&rules, &place, PEASANT, "oats");
            place.pops[PEASANT].stock[oats] = 0;
            account(&mut place, house, PEASANT, oats, goal + 1);
            capacity(&mut rules, &mut place, starter, 10_000_000);
            fill_grain_room_to(
                &mut rules,
                &mut place,
                house,
                if over_capacity { 0 } else { 1_000_000 },
            );
            if over_capacity {
                let peas = good(&rules, "peas_beans");
                place.buildings[house].stock[peas] += 5_000_000;
                place.buildings[house]
                    .storehouse_accounts
                    .as_mut()
                    .unwrap()
                    .communal[peas] += 5_000_000;
            }
            let everything = total(&rules, &place);
            let monk_oats = place.pops[MONK].stock[oats];
            let (peasant_barley, monk_barley) = (
                place.pops[PEASANT].stock[barley],
                place.pops[MONK].stock[barley],
            );
            exchange(&rules, &mut place);
            let label = format!("starter {starter}, over capacity {over_capacity}");
            assert_eq!(account_of(&place, house, PEASANT, oats), goal, "{label}");
            assert_eq!(
                place.pops[PEASANT].stock[barley],
                peasant_barley + 1,
                "{label}"
            );
            assert_eq!(place.pops[MONK].stock[barley], monk_barley - 1, "{label}");
            assert_eq!(place.pops[MONK].stock[oats], monk_oats + 1, "{label}");
            assert_eq!(account_of(&place, house, MONK, oats), 0, "{label}");
            // The storehouse's physical stock of oats is down by the bushel given, and the place's total is as it was.
            assert_eq!(place.buildings[house].stock[oats], goal, "{label}");
            assert_eq!(total(&rules, &place), everything, "{label}");
        }
    }
}

#[test]
fn the_noon_step_runs_with_the_largest_building_allowance_the_rules_accept() {
    // Two to the sixty-second ml a level is a valid number (a TOML integer): it must not stop the day at noon, whatever the levels.
    let mut rules = hunger_rules();
    for item in &mut rules.buildings {
        item.market_capacity_ml_per_level = 1 << 62;
    }
    rules.validate("rules").unwrap();
    let mut state = fort(&rules);
    for building in &mut state.places[0].buildings {
        building.levels = 4;
    }
    let day = on(1, 2);
    let ledger = run_daily_economy_with_context(
        &rules,
        &mut state,
        day,
        &BTreeMap::new(),
        &BTreeMap::from([("fort".to_owned(), 30)]),
        &BTreeMap::new(),
    );
    assert!(ledger.is_ok(), "{:?}", ledger.err());
}

#[test]
fn fuel_orders_are_unchanged_by_its_participation_in_exchange() {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    small(&mut place, 1);
    settle_the_rest(&rules, &mut place, &["oats", "hardwood", "peat"]);
    want(&rules, &mut place, PEASANT, "oats", 1);
    spare(&rules, &mut place, MONK, "oats", 1);
    place.pops[PEASANT].stock[good(&rules, "hardwood")] = 6;
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    capacity(&mut rules, &mut place, MONK, 10_000_000);
    let wood = good(&rules, "hardwood");
    let peat = good(&rules, "peat");
    let orders = daily_market_orders(&rules, &place).unwrap();
    assert_eq!(
        (
            orders.buy_units[wood],
            orders.buy_units[peat],
            orders.sell_units[wood],
            orders.sell_units[peat]
        ),
        (0, 7, 1, 0)
    );
}

#[test]
fn fuel_for_oats_moves_whole_loads_and_leaves_hearth_untouched() {
    let mut rules = rules();
    let mut place = place(&rules, 2);
    small(&mut place, 1);
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    let oats = good(&rules, "oats");
    let peat = good(&rules, "peat");
    let wood = good(&rules, "hardwood");
    for id in ["barley", "fish", "honey"] {
        spare(&rules, &mut place, PEASANT, id, 0);
        spare(&rules, &mut place, MONK, id, 0);
    }
    spare(&rules, &mut place, PEASANT, "oats", 10);
    want(&rules, &mut place, MONK, "oats", 3);
    let target = fuel::fuel_daily_need(&rules, &place.pops[PEASANT]).unwrap() * 240;
    place.pops[PEASANT].consumption.hearth_milli = u64::try_from(target - 2_000).unwrap();
    place.pops[MONK].consumption.hearth_milli = u64::try_from(target).unwrap();
    place.pops[MONK].stock[peat] = 3;
    set_price(&rules, &mut place, "oats", 100);
    set_price(&rules, &mut place, "peat", 100);
    set_price(&rules, &mut place, "hardwood", 100);
    let before_oats = place.pops[PEASANT].stock[oats];
    let mut expected = goods_everywhere(&place);
    expected[PEASANT][oats] -= 3;
    expected[MONK][oats] += 3;
    expected[PEASANT][peat] += 3;
    expected[MONK][peat] -= 3;
    exchange(&rules, &mut place);
    assert_eq!(goods_everywhere(&place), expected);
    assert_eq!(place.pops[PEASANT].stock[oats], before_oats - 3);
    assert_eq!(place.pops[PEASANT].stock[peat], 3);
    assert_eq!(place.pops[MONK].stock[peat], 0);
    assert_eq!(place.pops[PEASANT].stock[wood], 0);
    assert_eq!(
        place.pops[PEASANT].consumption.hearth_milli,
        u64::try_from(target - 2_000).unwrap()
    );
    assert_eq!(
        place.pops[MONK].consumption.hearth_milli,
        u64::try_from(target).unwrap()
    );
}

/// Each pop has enough food, no fuel above its hearth target, and the starter has oats to exchange.
fn fuel_fixture(pops: usize) -> (Rules, PlaceState, u64) {
    let mut rules = rules();
    let mut place = place(&rules, pops);
    small(&mut place, 10);
    for kind in &mut rules.pop_types {
        kind.fuel_need_milli_day = 2;
    }
    capacity(&mut rules, &mut place, PEASANT, 10_000_000);
    for pop in 0..pops {
        for id in ["oats", "barley", "fish", "honey"] {
            spare(&rules, &mut place, pop, id, 0);
        }
        let target = fuel::fuel_daily_need(&rules, &place.pops[pop]).unwrap() * 240;
        place.pops[pop].consumption.hearth_milli = u64::try_from(target).unwrap();
    }
    spare(&rules, &mut place, PEASANT, "oats", 50);
    for pop in 1..pops {
        want(&rules, &mut place, pop, "oats", 30);
    }
    set_price(&rules, &mut place, "oats", 10);
    set_price(&rules, &mut place, "peat", 60);
    set_price(&rules, &mut place, "hardwood", 100);
    let target = place.pops[PEASANT].consumption.hearth_milli;
    (rules, place, target)
}

#[test]
fn either_fuel_follows_the_live_deficit_and_cheaper_unit_first() {
    for (wood_price, expected_oats, expected_peat, expected_wood) in
        [(100, 16, 1, 1), (80, 16, 0, 2)]
    {
        let (rules, mut place, target) = fuel_fixture(3);
        let peat = good(&rules, "peat");
        let wood = good(&rules, "hardwood");
        let oats = good(&rules, "oats");
        place.pops[PEASANT].consumption.hearth_milli = target - 1_500;
        place.pops[MONK].stock[peat] = 1;
        place.pops[THIRD].stock[wood] = 2;
        set_price(&rules, &mut place, "hardwood", wood_price);
        let before = place.pops[PEASANT].stock[oats];
        exchange(&rules, &mut place);
        assert_eq!(place.pops[PEASANT].stock[oats], before - expected_oats);
        assert_eq!(place.pops[PEASANT].stock[peat], expected_peat);
        assert_eq!(place.pops[PEASANT].stock[wood], expected_wood);
        assert_eq!(place.pops[MONK].stock[peat], 1 - expected_peat);
        assert_eq!(place.pops[THIRD].stock[wood], 2 - expected_wood);
        assert_eq!(place.pops[PEASANT].consumption.hearth_milli, target - 1_500);
    }
}

#[test]
fn a_partner_can_spare_only_the_whole_load_that_preserves_its_fuel_target() {
    let (rules, mut place, target) = fuel_fixture(2);
    let wood = good(&rules, "hardwood");
    let peat = good(&rules, "peat");
    place.pops[PEASANT].consumption.hearth_milli = target - 700;
    place.pops[MONK].consumption.hearth_milli = 0;
    place.pops[MONK].stock[wood] = 5;
    place.pops[MONK].stock[peat] = 1;
    exchange(&rules, &mut place);
    assert_eq!(place.pops[PEASANT].stock[peat], 1);
    assert_eq!(place.pops[MONK].stock[peat], 0);
    assert_eq!(place.pops[MONK].stock[wood], 5);
    assert_eq!(place.pops[MONK].consumption.hearth_milli, 0);
}

#[test]
fn a_hungry_starter_seeks_food_even_when_fuel_is_offered() {
    let (rules, mut place, target) = fuel_fixture(2);
    let fish = good(&rules, "fish");
    let peat = good(&rules, "peat");
    for id in ["oats", "barley", "fish", "honey"] {
        place.pops[PEASANT].stock[good(&rules, id)] = 0;
    }
    // Pay with a comfort good, while holding less than 60 days of food.
    spare(&rules, &mut place, PEASANT, "herbs", 50);
    want(&rules, &mut place, MONK, "herbs", 20);
    spare(&rules, &mut place, MONK, "fish", 30);
    place.pops[PEASANT].consumption.hearth_milli = target - 1_000;
    place.pops[MONK].stock[peat] = 3;
    exchange(&rules, &mut place);
    assert!(place.pops[PEASANT].stock[fish] > 0);
    assert_eq!(place.pops[PEASANT].stock[peat], 0);
    assert_eq!(place.pops[MONK].stock[peat], 3);
}

#[test]
fn food_precedes_fuel_and_peat_precedes_the_less_bulky_hardwood() {
    for offered_oats in [true, false] {
        let (mut rules, mut place, target) = fuel_fixture(2);
        let oats = good(&rules, "oats");
        let peat = good(&rules, "peat");
        let wood = good(&rules, "hardwood");
        let herbs = good(&rules, "herbs");
        // 162 litres covers two oats (78 litres) or one peat (162 litres),
        // but the food swap leaves too little for a second fuel swap.
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .market_capacity_ml_per_level = 162_000;
        want(&rules, &mut place, PEASANT, "oats", 2);
        spare(&rules, &mut place, PEASANT, "herbs", 50);
        want(&rules, &mut place, MONK, "herbs", 20);
        if offered_oats {
            spare(&rules, &mut place, MONK, "oats", 2);
        }
        place.pops[PEASANT].consumption.hearth_milli = target - 1_000;
        place.pops[MONK].stock[peat] = 1;
        place.pops[MONK].stock[wood] = 1;
        set_price(&rules, &mut place, "oats", 20);
        set_price(&rules, &mut place, "herbs", 10);
        let (before_oats, before_herbs) = (
            place.pops[PEASANT].stock[oats],
            place.pops[PEASANT].stock[herbs],
        );
        exchange(&rules, &mut place);
        if offered_oats {
            assert_eq!(
                (
                    place.pops[PEASANT].stock[oats] - before_oats,
                    before_herbs - place.pops[PEASANT].stock[herbs]
                ),
                (2, 4)
            );
            assert_eq!(
                (
                    place.pops[PEASANT].stock[peat],
                    place.pops[PEASANT].stock[wood]
                ),
                (0, 0)
            );
        } else {
            assert_eq!(
                (
                    place.pops[PEASANT].stock[peat],
                    place.pops[PEASANT].stock[wood]
                ),
                (1, 0)
            );
            assert_eq!(before_herbs - place.pops[PEASANT].stock[herbs], 6);
        }
    }
}

#[test]
fn a_buildings_spare_fuel_is_paid_from_it_into_the_buyers_store() {
    let (rules, mut place, target) = fuel_fixture(2);
    let wood = good(&rules, "hardwood");
    let oats = good(&rules, "oats");
    place.pops[MONK].workers = 30;
    let monk_target = fuel::fuel_daily_need(&rules, &place.pops[MONK]).unwrap() * 240;
    place.pops[MONK].consumption.hearth_milli = u64::try_from(monk_target).unwrap();
    want(&rules, &mut place, MONK, "oats", 30);
    let farm = add_building(&rules, &mut place, "holdings", 1);
    own(&mut place, farm, MONK, 1);
    place.buildings[farm].stock[wood] = 6;
    place.pops[PEASANT].consumption.hearth_milli = target - 2_000;
    let before = place.pops[PEASANT].stock[oats];
    let mut expected = goods_everywhere(&place);
    expected[PEASANT][wood] += 2;
    expected[PEASANT][oats] -= 20;
    expected[MONK][oats] += 20;
    expected[place.pops.len() + farm][wood] -= 2;
    exchange(&rules, &mut place);
    assert_eq!(goods_everywhere(&place), expected);
    assert_eq!(place.pops[PEASANT].stock[wood], 2);
    assert_eq!(place.pops[MONK].stock[wood], 0);
    assert_eq!(place.buildings[farm].stock[wood], 4);
    assert_eq!(before - place.pops[PEASANT].stock[oats], 20);
    assert_eq!(place.pops[PEASANT].consumption.hearth_milli, target - 2_000);
}

#[test]
fn scarce_peat_is_shared_and_neither_buyer_exceeds_its_want() {
    let (mut rules, mut place, target) = fuel_fixture(3);
    capacity(&mut rules, &mut place, MONK, 10_000_000);
    let peat = good(&rules, "peat");
    place.pops[PEASANT].consumption.hearth_milli = target - 2_000;
    place.pops[MONK].consumption.hearth_milli = target - 2_000;
    spare(&rules, &mut place, MONK, "oats", 50);
    want(&rules, &mut place, THIRD, "oats", 30);
    place.pops[THIRD].stock[peat] = 2;
    exchange(&rules, &mut place);
    assert_eq!(
        (
            place.pops[PEASANT].stock[peat],
            place.pops[MONK].stock[peat],
            place.pops[THIRD].stock[peat]
        ),
        (1, 1, 0)
    );
    assert_eq!(place.pops[PEASANT].consumption.hearth_milli, target - 2_000);
    assert_eq!(place.pops[MONK].consumption.hearth_milli, target - 2_000);
}

#[test]
fn full_store_takes_fuel_up_to_its_limit_and_capacity_counts_load_volume() {
    for (held, capacity_ml, expected) in [(2, 200_000, 2), (2, 300_000, 3), (5, 300_000, 0)] {
        let (mut rules, mut place, target) = fuel_fixture(2);
        let wood = good(&rules, "hardwood");
        let oats = good(&rules, "oats");
        rules.goods[wood].pop_limit_milli = 500;
        // A zero-volume payment keeps the receiving store over its volume throughout.
        rules.goods[oats].volume_ml = 0;
        place.pops[PEASANT].stock[good(&rules, "barley")] = 1_000;
        place.pops[PEASANT].stock[wood] = held;
        place.pops[PEASANT].consumption.hearth_milli = 0;
        place.pops[MONK].stock[wood] = 4;
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .market_capacity_ml_per_level = capacity_ml;
        let before = place.pops[PEASANT].stock[oats];
        // At equal value, one oat pays for one load.
        set_price(&rules, &mut place, "oats", 100);
        let mut expected_goods = goods_everywhere(&place);
        expected_goods[PEASANT][oats] -= expected;
        expected_goods[MONK][oats] += expected;
        expected_goods[PEASANT][wood] += expected;
        expected_goods[MONK][wood] -= expected;
        exchange(&rules, &mut place);
        assert_eq!(goods_everywhere(&place), expected_goods);
        assert_eq!(place.pops[PEASANT].stock[wood], held + expected);
        assert_eq!(place.pops[MONK].stock[wood], 4 - expected);
        assert_eq!(before - place.pops[PEASANT].stock[oats], expected);
        assert_eq!(place.pops[PEASANT].consumption.hearth_milli, 0);
        assert_eq!(target, 4_800);
    }
}

#[test]
fn opening_fuel_prices_rank_the_swap_but_moved_prices_value_it() {
    let (rules, mut place, target) = fuel_fixture(3);
    let peat = good(&rules, "peat");
    let wood = good(&rules, "hardwood");
    let oats = good(&rules, "oats");
    place.pops[PEASANT].consumption.hearth_milli = target - 700;
    place.pops[MONK].stock[peat] = 1;
    place.pops[THIRD].stock[wood] = 1;
    let opening = place.prices_milli.clone();
    set_price(&rules, &mut place, "peat", 90);
    let before = place.pops[PEASANT].stock[oats];
    let mut state = EconomyState {
        places: vec![place],
    };
    swap::run_daily_swaps_at_prices(&rules, &mut state, &[opening]).unwrap();
    let place = &state.places[0];
    assert_eq!(
        (
            place.pops[PEASANT].stock[peat],
            place.pops[PEASANT].stock[wood]
        ),
        (1, 0)
    );
    assert_eq!(before - place.pops[PEASANT].stock[oats], 9);
}

#[test]
fn a_month_without_fuel_offers_preserves_the_nonfuel_day() {
    let (rules, mut place, _) = fuel_fixture(2);
    place.prices_milli = rules
        .goods
        .iter()
        .map(|good| good.base_price * 1000)
        .collect();
    for pop in &mut place.pops {
        pop.consumption.hearth_milli = 0;
    }
    let mut without_fuel_trade = rules.clone();
    let fuel_goods: Vec<_> = ["hardwood", "peat"]
        .into_iter()
        .map(|id| good(&rules, id))
        .collect();
    for &good in &fuel_goods {
        without_fuel_trade.goods[good]
            .tags
            .retain(|tag| tag != "fuel");
    }
    let mut actual = EconomyState {
        places: vec![place],
    };
    let mut reference = actual.clone();
    for day in 2..=31 {
        let date = on(1, day);
        for state in [&mut actual, &mut reference] {
            update_daily_prices(&rules, state).unwrap();
        }
        // Both see exactly the same orders and consumption; the reference exchange
        // treats the absent fuel goods as ordinary goods with no target or stock.
        swap::run_daily_swaps(&rules, &mut actual).unwrap();
        swap::run_daily_swaps(&without_fuel_trade, &mut reference).unwrap();
        for state in [&mut actual, &mut reference] {
            let prices: Vec<_> = state
                .places
                .iter()
                .map(|p| p.prices_milli.clone())
                .collect();
            run_daily_consumption_at_prices(&rules, state, date, &prices).unwrap();
        }
        let (a, b) = (&actual.places[0], &reference.places[0]);
        assert_eq!(a.pops, b.pops, "day {day}");
        assert_eq!(a.buildings, b.buildings, "day {day}");
        for i in 0..rules.goods.len() {
            if fuel_goods.contains(&i) {
                assert_eq!(
                    a.pops.iter().map(|p| p.stock[i]).sum::<u64>(),
                    0,
                    "day {day}"
                );
            } else {
                assert_eq!(a.prices_milli[i], b.prices_milli[i], "day {day}, good {i}");
            }
        }
    }
}

#[test]
fn partners_also_want_fuel_before_comforts() {
    let (rules, mut place, target) = fuel_fixture(2);
    let peat = good(&rules, "peat");
    let herbs = good(&rules, "herbs");
    want(&rules, &mut place, PEASANT, "oats", 1);
    spare(&rules, &mut place, PEASANT, "herbs", 10);
    want(&rules, &mut place, MONK, "herbs", 1);
    spare(&rules, &mut place, MONK, "oats", 1);
    place.pops[PEASANT].stock[peat] = 1;
    place.pops[MONK].consumption.hearth_milli = target - 700;
    set_price(&rules, &mut place, "oats", 100);
    set_price(&rules, &mut place, "peat", 100);
    set_price(&rules, &mut place, "herbs", 100);
    let before = place.pops[MONK].stock[herbs];
    exchange(&rules, &mut place);
    assert_eq!(place.pops[MONK].stock[peat], 1);
    assert_eq!(place.pops[MONK].stock[herbs], before);
    assert_eq!(place.pops[PEASANT].stock[peat], 0);
}

#[test]
fn a_fuel_share_cannot_make_a_buyer_take_peat_after_wood_met_the_need() {
    let (mut rules, mut place, target) = fuel_fixture(3);
    capacity(&mut rules, &mut place, MONK, 10_000_000);
    let wood = good(&rules, "hardwood");
    let peat = good(&rules, "peat");
    place.pops[PEASANT].consumption.hearth_milli = target - 1_000;
    place.pops[MONK].consumption.hearth_milli = target - 2_000;
    spare(&rules, &mut place, PEASANT, "oats", 100);
    spare(&rules, &mut place, MONK, "oats", 30);
    want(&rules, &mut place, THIRD, "oats", 30);
    place.pops[THIRD].stock[wood] = 1;
    place.pops[THIRD].stock[peat] = 2;
    set_price(&rules, &mut place, "hardwood", 80);
    exchange(&rules, &mut place);
    assert_eq!(place.pops[PEASANT].stock[wood], 1);
    assert_eq!(place.pops[PEASANT].stock[peat], 0);
    assert!(place.pops[MONK].stock[peat] > 0);
    assert_eq!(place.pops[PEASANT].consumption.hearth_milli, target - 1_000);
}

#[test]
fn the_real_price_step_can_reverse_the_fuel_ranking() {
    let (mut rules, mut place, target) = fuel_fixture(3);
    let oats = good(&rules, "oats");
    rules.goods[oats].base_price = 1;
    place.prices_milli = rules.goods.iter().map(|g| g.base_price * 1000).collect();
    let wood = good(&rules, "hardwood");
    let peat = good(&rules, "peat");
    place.prices_milli[peat] = 6_999;
    place.pops[PEASANT].consumption.hearth_milli = target - 4_000;
    place.pops[MONK].stock[peat] = 1;
    place.pops[THIRD].stock[wood] = 1;
    let opening = place.prices_milli.clone();
    let mut state = EconomyState {
        places: vec![place],
    };
    update_daily_prices(&rules, &mut state).unwrap();
    let moved = &state.places[0].prices_milli;
    assert!(u128::from(opening[peat]) * 1000 < u128::from(opening[wood]) * 700);
    assert!(
        u128::from(moved[peat]) * 1000 > u128::from(moved[wood]) * 700,
        "opening {opening:?}, moved {moved:?}"
    );
    rules.swap_loss_limit_pm = 200;
    let peat_payment = moved[peat].div_ceil(moved[oats]);
    let allowance = peat_payment * rules.goods[oats].volume_ml + rules.goods[peat].volume_ml;
    rules
        .buildings
        .iter_mut()
        .find(|b| b.id == "farm")
        .unwrap()
        .market_capacity_ml_per_level = allowance;
    let before_oats = state.places[0].pops[PEASANT].stock[oats];
    swap::run_daily_swaps_at_prices(&rules, &mut state, &[opening]).unwrap();
    let place = &state.places[0];
    assert_eq!(
        (
            place.pops[PEASANT].stock[peat],
            place.pops[PEASANT].stock[wood]
        ),
        (1, 0)
    );
    assert_eq!(before_oats - place.pops[PEASANT].stock[oats], peat_payment);
}
