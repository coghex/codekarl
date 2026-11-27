//! CK-434 (trade part F): the opening.
//!
//! The cases are the spec's coverage for F (section 12): the worked amount of 11c.5 (38), goods competing for a store, an unreachable
//! producer, the same seed the same opening and another seed another, the fleets (an island monastery's currach), the one opening choice
//! (a vehicle of the trader's own before a new one, a new vehicle given with no inputs spent), trips part way out at draws 0 and 999 and a
//! one-hour trip against the unbroken game, a save before the first tick advanced past the first noon, and every good and hand counted
//! once. They run on the swap tests' fixture places with a table of ways in place of the network.

use std::collections::{BTreeMap, BTreeSet};

use ck_sim::Canonical;

use super::choose::{Journey, Paces, Party, Way, Ways};
use super::expedition::{Delays, Leg, Mode, Phase, ratio, whole};
use super::expedition_tests::{self, at, date_of, hours};
use super::fixtures::{dest_place, home_place, rules_with, target, world};
use super::opening::{made_goods, opening_amount, run_opening};
use super::swap_tests::{
    MONK, PEASANT, add_building, cattle_farm, good, herd_rules, place, settle_the_rest,
};
use super::*;

/// A way that takes `hours` whatever the pace.
fn fixed(hours: u64) -> Leg {
    Leg {
        mode: Mode::Land,
        terms: Vec::new(),
        fixed: ratio(u128::from(hours), 1),
        delays: Delays::Along,
        key: [0; 4],
        along: Vec::new(),
        path: Vec::new(),
    }
}

/// A table of ways between places by id, the islands, and the places a barge alone leaves from.
#[derive(Default)]
struct Net {
    ways: BTreeMap<(String, String), u64>,
    islands: BTreeSet<String>,
    landings: BTreeSet<String>,
    /// The hours the clipped way is shorter than the centre-to-centre way by (the visiting radius at each end).
    clip: u64,
    /// The hours the clipped way to a place is shorter by, more than `clip`.
    clips: BTreeMap<String, u64>,
}

impl Net {
    fn both(mut self, a: &str, b: &str, hours: u64) -> Self {
        self.ways.insert((a.to_owned(), b.to_owned()), hours);
        self.ways.insert((b.to_owned(), a.to_owned()), hours);
        self
    }

    fn hours(&self, home: &str, other: &str, way: Way) -> Option<u64> {
        let key = match way {
            Way::Out => (home.to_owned(), other.to_owned()),
            Way::Back => (other.to_owned(), home.to_owned()),
        };
        self.ways.get(&key).copied()
    }
}

impl Ways for Net {
    fn neighbours(&self, _a: &str, _b: &str) -> bool {
        false
    }

    fn destinations(&mut self, home: &str, party: &Party) -> Result<Vec<String>, String> {
        if party.boat_only && !self.landings.contains(home) {
            return Ok(Vec::new());
        }
        Ok(self
            .ways
            .keys()
            .filter(|(from, to)| {
                from == home && self.ways.contains_key(&(to.clone(), home.to_owned()))
            })
            .map(|(_, to)| to.clone())
            .collect())
    }

    fn journey(
        &mut self,
        home: &str,
        other: &str,
        _party: &Party,
        _paces: Paces,
        way: Way,
    ) -> Result<Option<Journey>, String> {
        Ok(self.hours(home, other, way).map(|hours| Journey {
            legs: vec![fixed(hours.saturating_sub(
                self.clip + self.clips.get(other).copied().unwrap_or(0),
            ))],
            arrival: [0, 0],
        }))
    }

    fn freeze(
        &mut self,
        home: &str,
        other: &str,
        party: &Party,
        paces: Paces,
        way: Way,
    ) -> Result<Option<Journey>, String> {
        self.journey(home, other, party, paces, way)
    }

    fn island(&self, place: &str) -> bool {
        self.islands.contains(place)
    }
}

#[test]
fn the_shipped_opening_data_names_vehicles_that_exist_and_trade_stays_off() {
    // A rules file may prune its vehicles (the cases do), so a fleet naming none gives none; the shipped one must name real kinds.
    let rules = fixtures::shipped();
    assert!(
        !rules.expedition.looks,
        "trade stays off in the shipped game until looking is cheap (CK-434 look profile)"
    );
    let fleets = &rules.expedition.opening.fleets;
    for id in [
        &fleets.noble_horses,
        &fleets.noble,
        &fleets.barge,
        &fleets.sea,
        &fleets.monastic,
        &fleets.monastic_island,
    ] {
        let id = id.as_ref().expect("every shipped fleet is named");
        assert!(rules.vehicles.iter().any(|v| v.id == *id), "{id}");
    }
}

/// The first hour of the game in the cases: a morning in March, so the first noon is a few ticks off.
fn start() -> u64 {
    hours(at(10, 6))
}

/// The farm of a producer place with a level on the sea method, so the place makes fish (and the farming methods' own goods).
fn fishery(rules: &Rules, place: &mut PlaceState) {
    farm_with(rules, place, &["sea"]);
}

/// A farm of the place with a level on each method of the resources named, so the place makes what they make.
fn farm_with(rules: &Rules, place: &mut PlaceState, resources: &[&str]) {
    let building = add_building(rules, place, "farm", 1);
    let definition = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
    for (index, method) in definition.methods.iter().enumerate() {
        if resources.contains(&method.resource.as_str()) {
            place.buildings[building].method_levels[index] = 1;
        }
    }
}

fn fish() -> usize {
    good(&herd_rules(), "fish")
}

/// What the opening gave a pop of a good: the stock now, since the cases start with nothing.
fn given(state: &EconomyState, place: usize, pop: usize, id: &str, rules: &Rules) -> u64 {
    state.places[place].pops[pop].stock[good(rules, id)]
}

/// The pops at `home` start with empty stores, so they want everything their diet names.
fn empty_home(rules: &Rules, id: &str) -> PlaceState {
    let mut home = place(rules, 3);
    id.clone_into(&mut home.id);
    home
}

#[test]
fn the_worked_amount_of_the_spec_is_38_and_a_producer_half_the_limit_away_or_more_gives_none() {
    let rules = rules_with(&[]);
    let opening = &rules.expedition.opening;
    // 11c.5 rule 17: T = 480, seeded share 750, the nearest fish-making place 12 hours away, limit 168.
    assert_eq!(opening_amount(opening, 480, 750, &whole(12), 168), 38);
    // 480 x 30 / 240 = 60; at the whole of the share and no distance, all of it.
    assert_eq!(opening_amount(opening, 480, 1000, &whole(0), 168), 60);
    // Nothing at half the limit, nothing beyond it, and a producer not yet half way gives something.
    assert_eq!(opening_amount(opening, 480, 750, &whole(84), 168), 0);
    assert_eq!(opening_amount(opening, 480, 750, &whole(100), 168), 0);
    assert!(opening_amount(opening, 480, 750, &whole(40), 168) > 0);
    // The nearness is a whole per-mille (rule 17 floors it, 857 here), then the units are rounded down: 5,600 x 30 / 240 = 700, and 700 x
    // 857 / 1000 is 599.9, so 599 (the fractional 857.14 would give 600).
    assert_eq!(opening_amount(opening, 5600, 1000, &whole(12), 168), 599);
    // The base is whole first (rule 22): a target of 15 is 15 x 30 / 240 = 1.875, so 1; one hour away is 988 per-mille, and 1 x 988 / 1000
    // is no whole unit (the fractional base would give 1).
    assert_eq!(opening_amount(opening, 15, 1000, &whole(1), 168), 0);
}

#[test]
fn goods_the_place_does_not_make_come_by_the_nearness_of_a_producer_and_an_unreachable_one_gives_none()
 {
    let mut rules = rules_with(&[]);
    rules.expedition.looks = true;
    let mut home = empty_home(&rules, "home");
    // Neither of the other places can be reached but `near` and `far`; `lost` makes fish too and has no way to home.
    let (mut near, mut far, mut lost) = (
        empty_home(&rules, "near"),
        empty_home(&rules, "far"),
        empty_home(&rules, "lost"),
    );
    fishery(&rules, &mut near);
    fishery(&rules, &mut far);
    // `lost` is the only place that makes oats (the farming method) and no way leads to it.
    farm_with(&rules, &mut lost, &["usable_land"]);
    for item in [&mut home, &mut near, &mut far, &mut lost] {
        settle_the_rest(&rules, item, &["fish", "oats"]);
    }
    let mut state = world(vec![home, near, far, lost]);
    let mut net = Net::default()
        .both("home", "near", 12)
        .both("home", "far", 100);
    let seed = 42;
    let report = run_opening(&rules, &mut state, start(), seed, &mut net, &mut ()).unwrap();
    // The peasant (limit 168) is given fish by the producer 12 hours away; the monk (limit 720) by the nearest of the two, also 12.
    let limits = [(PEASANT, 168_u64), (MONK, 720)];
    for (pop, limit) in limits {
        let share = rules.expedition.opening.share_pm[0]
            + herd_draw(seed, &["home", &pop.to_string(), "opening-share", "fish"])
                % (rules.expedition.opening.share_pm[1] - rules.expedition.opening.share_pm[0] + 1);
        let target = u128::from(target(&rules, &state.places[0], pop, "fish"));
        assert_eq!(
            given(&state, 0, pop, "fish", &rules),
            opening_amount(&rules.expedition.opening, target, share, &whole(12), limit),
            "pop {pop}"
        );
        assert!(given(&state, 0, pop, "fish", &rules) > 0);
    }
    // The goods the near place makes itself are not given there, and a good no place makes (honey comes from no building here) is not
    // given anywhere.
    assert_eq!(given(&state, 1, PEASANT, "fish", &rules), 0);
    assert_eq!(
        given(&state, 0, PEASANT, "honey", &rules),
        target(&rules, &state.places[0], PEASANT, "honey")
    );
    // An unreachable producer gives none: nobody has a way to `lost`, the only place that makes oats, so home is given no oats.
    assert_eq!(given(&state, 0, PEASANT, "oats", &rules), 0);
    assert_eq!(given(&state, 0, MONK, "oats", &rules), 0);
    assert_eq!(report.goods[good(&rules, "oats")], (0, 0));
    assert!(report.goods[fish()].0 > 0);
    // Live animals are never given.
    assert_eq!(given(&state, 0, PEASANT, "cattle", &rules), 0);
}

#[test]
fn a_producer_too_far_for_a_pop_gives_it_nothing_while_a_pop_with_a_longer_limit_is_given() {
    let mut rules = rules_with(&[]);
    rules.expedition.looks = true;
    let mut home = empty_home(&rules, "home");
    let mut far = empty_home(&rules, "far");
    fishery(&rules, &mut far);
    for item in [&mut home, &mut far] {
        settle_the_rest(&rules, item, &["fish"]);
    }
    let mut state = world(vec![home, far]);
    let mut net = Net::default().both("home", "far", 90);
    run_opening(&rules, &mut state, start(), 7, &mut net, &mut ()).unwrap();
    // 90 hours is past half the peasant's 168-hour limit (84) and well within half the monk's 720.
    assert_eq!(given(&state, 0, PEASANT, "fish", &rules), 0);
    assert!(given(&state, 0, MONK, "fish", &rules) > 0);
}

#[test]
fn two_goods_competing_for_a_store_are_capped_in_data_order_and_the_later_takes_what_is_left() {
    let mut rules = rules_with(&[]);
    rules.expedition.looks = true;
    // Each of oats and fish may fill a quarter of the people's holding limit; the room left of a store whose volume is short is what the
    // later good takes.
    let (oats, fish_good) = (good(&rules, "oats"), good(&rules, "fish"));
    assert!(oats < fish_good, "oats come before fish in the goods' data");
    let mut home = empty_home(&rules, "home");
    home.pops[PEASANT].workers = 6;
    home.pops[PEASANT].dependents = 4;
    let mut near = empty_home(&rules, "near");
    farm_with(&rules, &mut near, &["sea", "usable_land"]);
    // The near place makes oats too (the farming method), so the home pop wants both and both are made there.
    let goods = made_goods(&rules, &near);
    assert!(
        goods.contains(&oats) && goods.contains(&fish_good),
        "{goods:?}"
    );
    // A store that takes only 3 oats and any amount of fish.
    rules.goods[oats].pop_limit_milli = 300;
    let mut state = world(vec![home, near]);
    let mut net = Net::default().both("home", "near", 6);
    run_opening(&rules, &mut state, start(), 3, &mut net, &mut ()).unwrap();
    let held = given(&state, 0, PEASANT, "oats", &rules);
    assert_eq!(
        held, 3,
        "ten people at 300 per-mille of a unit each: 3 whole units"
    );
    assert!(
        given(&state, 0, PEASANT, "fish", &rules) > 0,
        "the later good took what room was left"
    );
}

#[test]
fn the_same_seed_gives_the_same_opening_and_another_seed_another() {
    let mut rules = rules_with(&["ox_cart"]);
    rules.expedition.looks = true;
    let build = || {
        let mut home = empty_home(&rules, "home");
        let mut near = empty_home(&rules, "near");
        let mut other = empty_home(&rules, "other");
        fishery(&rules, &mut near);
        fishery(&rules, &mut other);
        for item in [&mut home, &mut near, &mut other] {
            settle_the_rest(&rules, item, &["fish", "oats"]);
        }
        world(vec![home, near, other])
    };
    let net = || {
        Net::default()
            .both("home", "near", 10)
            .both("home", "other", 14)
    };
    let run = |seed| {
        let mut state = build();
        run_opening(&rules, &mut state, start(), seed, &mut net(), &mut ()).unwrap();
        state.to_bytes().unwrap()
    };
    assert_eq!(run(1), run(1));
    assert_ne!(run(1), run(2));
}

/// A noble pop at place `id` over a demesne whose horses are `horses` head, a monk, and a peasant.
fn noble_place(rules: &Rules, id: &str, horses: u64) -> PlaceState {
    let mut place = empty_home(rules, id);
    place.pops[PEASANT].kind = "noble".to_owned();
    let demesne = add_building(rules, &mut place, "demesne", 1);
    let definition = rules.buildings.iter().find(|b| b.id == "demesne").unwrap();
    let farming = definition
        .methods
        .iter()
        .position(|method| method.resource != "sea")
        .unwrap();
    let species = rules
        .herd_species
        .iter()
        .position(|species| species.animal == "horses")
        .unwrap();
    place.buildings[demesne].method_levels[farming] = 1;
    place.buildings[demesne].herds = vec![0; rules.herd_species.len()];
    place.buildings[demesne].herds[species] = horses;
    place
}

fn fleet_of(state: &EconomyState, place: usize, pop: usize, rules: &Rules) -> Vec<String> {
    state.places[place]
        .trade
        .vehicles
        .iter()
        .filter(|vehicle| vehicle.pop as usize == pop)
        .map(|vehicle| rules.vehicles[vehicle.kind as usize].id.clone())
        .collect()
}

#[test]
fn fleets_follow_the_nobles_horses_the_landing_and_the_island_and_include_their_animals() {
    let mut rules = fixtures::rules();
    rules.expedition.looks = true;
    let horses_max = rules
        .buildings
        .iter()
        .find(|b| b.id == "demesne")
        .unwrap()
        .herds["horses"]
        .maximum;
    assert!(horses_max > 1);
    let mut places = vec![
        noble_place(&rules, "wagon", horses_max),
        noble_place(&rules, "cart", 0),
        noble_place(&rules, "landing", 0),
        noble_place(&rules, "isle", 0),
        empty_home(&rules, "monastery"),
        empty_home(&rules, "isle_monastery"),
        empty_home(&rules, "isle_foreign"),
    ];
    for index in [4, 5, 6] {
        places[index].pops[MONK].kind = "monk".to_owned();
    }
    // Only the Irish build currachs (r47): the island noble and monastery are Irish, the third island's monks are not.
    for index in [3, 5] {
        for pop in &mut places[index].pops {
            "ulaid".clone_into(&mut pop.culture);
        }
    }
    "northumbrian".clone_into(&mut places[6].pops[MONK].culture);
    let herds_before: Vec<Vec<u64>> = places
        .iter()
        .flat_map(|p| p.buildings.iter().map(|b| b.herds.clone()))
        .collect();
    let mut state = world(places);
    let mut net = Net::default()
        .both("wagon", "cart", 5)
        .both("landing", "monastery", 5);
    net.landings.insert("landing".to_owned());
    net.islands.insert("isle".to_owned());
    net.islands.insert("isle_monastery".to_owned());
    net.islands.insert("isle_foreign".to_owned());
    run_opening(&rules, &mut state, start(), 9, &mut net, &mut ()).unwrap();
    assert_eq!(fleet_of(&state, 0, PEASANT, &rules), ["covered_wagon"]);
    assert_eq!(fleet_of(&state, 1, PEASANT, &rules), ["ox_cart"]);
    assert_eq!(
        fleet_of(&state, 2, PEASANT, &rules),
        ["ox_cart", "simple_barge"],
        "a landing a barge can use"
    );
    assert_eq!(
        fleet_of(&state, 3, PEASANT, &rules),
        ["ox_cart", "currach"],
        "a place on an island with no barge landing"
    );
    assert_eq!(fleet_of(&state, 4, MONK, &rules), ["ox_cart"]);
    assert_eq!(
        fleet_of(&state, 5, MONK, &rules),
        ["currach"],
        "an island monastery's currach"
    );
    assert_eq!(
        fleet_of(&state, 6, MONK, &rules),
        ["ox_cart"],
        "the cart where its culture may not build a currach"
    );
    // The vehicles are free, built (no materials spent) and no herd lost an animal for them.
    assert!(
        state
            .places
            .iter()
            .flat_map(|p| &p.trade.vehicles)
            .all(|v| v.materials.is_none())
    );
    let herds_after: Vec<Vec<u64>> = state
        .places
        .iter()
        .flat_map(|p| p.buildings.iter().map(|b| b.herds.clone()))
        .collect();
    assert_eq!(herds_before, herds_after);
    let numbers: Vec<u64> = state
        .places
        .iter()
        .flat_map(|p| p.trade.vehicles.iter().map(|v| v.number))
        .collect();
    let distinct: BTreeSet<u64> = numbers.iter().copied().collect();
    assert_eq!(
        numbers.len(),
        distinct.len(),
        "each vehicle has its own number"
    );
    assert_eq!(usize::try_from(state.trade.vehicles), Ok(numbers.len()));
}

/// A trader's world for the opening choice: a peasant at `home` who wants fish, a destination that has fish to spare.
fn trade_world(rules: &Rules, owned: bool) -> EconomyState {
    let mut home = home_place(rules, "home", 4000, 4000);
    if !owned {
        home.trade.vehicles.clear();
    }
    let dest = dest_place(rules, "dest", 8000, 1500);
    world(vec![home, dest])
}

/// The peasant's vehicles at home.
fn peasants_vehicles(state: &EconomyState) -> Vec<&Vehicle> {
    state.places[0]
        .trade
        .vehicles
        .iter()
        .filter(|vehicle| vehicle.pop as usize == PEASANT)
        .collect()
}

/// Rules in which nobody can carry a trip on foot, so a plan with a vehicle is the only one.
fn needing_a_vehicle(vehicles: &[&str]) -> Rules {
    let mut rules = rules_with(vehicles);
    rules.expedition.pack_g = 1_000;
    rules.expedition.pack_ml = 1_000;
    rules
}

#[test]
fn the_opening_uses_a_vehicle_the_trader_owns_before_a_new_one_and_makes_a_job() {
    // The cart the peasant owns is the only kind the rules hold: a trip that pays is made with it and nothing is given.
    let rules = needing_a_vehicle(&["ox_cart"]);
    let mut state = trade_world(&rules, true);
    let mut net = Net::default().both("home", "dest", 6);
    let report = run_opening(&rules, &mut state, start(), 5, &mut net, &mut ()).unwrap();
    assert_eq!(report.looks.given, 0);
    assert!(report.looks.jobs >= 1, "the peasant's trip pays");
    assert_eq!(peasants_vehicles(&state).len(), 1);
    // Every trader has looked once; the peasant at the opening's own hour, the idle ones within the timer before the start.
    let peasant = &state.places[0].trade.looks[&u32::try_from(PEASANT).unwrap()];
    // A first look that made a job has last look S, the first tick's hour (the designer's intent), not the hour before it that stages trips.
    assert_eq!(peasant.last, start());
    for (place, slot) in &report.looks.idle {
        let look = state.places[*place].trade.looks[slot];
        assert!(look.last > start() - rules.expedition.look_hours && look.last <= start());
    }
    assert_eq!(
        report.looks.order.len(),
        state.places.iter().map(|p| p.pops.len()).sum::<usize>(),
        "every trader looked once"
    );
}

#[test]
fn a_cart_the_trader_owns_is_used_before_a_cheaper_one_that_would_be_given() {
    // The peasant owns a free ox-cart (the first kind in the shipped data order is the hand cart, a cheaper one to give), has the
    // hardwood to make a hand cart of, and nothing is given: what it owns costs nothing in the ranking.
    let rules = needing_a_vehicle(&["hand_cart", "ox_cart"]);
    let mut state = trade_world(&rules, true);
    state.places[0].trade.vehicles[0].kind = u32::try_from(
        rules
            .vehicles
            .iter()
            .position(|v| v.id == "ox_cart")
            .unwrap(),
    )
    .unwrap();
    state.places[0].pops[PEASANT].stock[good(&rules, "hardwood")] += 20;
    let mut net = Net::default().both("home", "dest", 6);
    let report = run_opening(&rules, &mut state, start(), 5, &mut net, &mut ()).unwrap();
    let mine = peasants_vehicles(&state);
    assert_eq!(report.looks.given, 0, "{report:?}");
    assert_eq!(mine.len(), 1);
    assert_eq!(rules.vehicles[mine[0].kind as usize].id, "ox_cart");
    assert!(!mine[0].free, "it is on the trip");
}

#[test]
fn of_the_kinds_a_place_could_make_the_one_that_costs_least_over_a_year_is_given() {
    // The peasant has the hardwood for a hand cart and an ox-cart and cattle above the standard level; the hand cart costs less.
    let rules = needing_a_vehicle(&["hand_cart", "ox_cart"]);
    let mut state = trade_world(&rules, false);
    state.places[0].pops[PEASANT].stock[good(&rules, "hardwood")] += 40;
    let farm = cattle_farm(&rules, &mut state.places[0], 10, &[(PEASANT, 1)]);
    let _ = farm;
    let mut net = Net::default().both("home", "dest", 6);
    let report = run_opening(&rules, &mut state, start(), 5, &mut net, &mut ()).unwrap();
    assert_eq!(report.looks.given, 1, "{report:?}");
    let mine = peasants_vehicles(&state);
    assert_eq!(rules.vehicles[mine[0].kind as usize].id, "hand_cart");
}

#[test]
fn a_vehicle_a_chosen_plan_lacks_is_given_already_built_and_no_input_or_animal_is_spent() {
    let rules = needing_a_vehicle(&["ox_cart", "hand_cart"]);
    let mut state = trade_world(&rules, false);
    // The peasant has the hardwood and the cattle above the standard level that a cart is made of; nothing is spent for the gift.
    let hardwood = good(&rules, "hardwood");
    state.places[0].pops[PEASANT].stock[hardwood] += 20;
    let before = state.places[0].pops[PEASANT].stock[hardwood];
    let mut net = Net::default().both("home", "dest", 6);
    let report = run_opening(&rules, &mut state, start(), 5, &mut net, &mut ()).unwrap();
    assert!(report.looks.given >= 1, "{report:?}");
    let vehicles = peasants_vehicles(&state);
    assert_eq!(vehicles.len() as u64, report.looks.given);
    assert!(vehicles.iter().all(|v| v.materials.is_none()));
    // The hardwood is where it was (the trip's own cargo and supplies aside, which are not hardwood).
    assert!(
        state.places[0].pops[PEASANT].stock[hardwood]
            + state.places[0]
                .trade
                .expeditions
                .iter()
                .chain(&[])
                .map(|e| e.supplies[hardwood] + e.cargo[hardwood])
                .sum::<u64>()
            >= before,
        "no hardwood went into the vehicle"
    );
    // The vehicle is reserved by the job or away with the expedition, not lying free.
    assert!(vehicles.iter().all(|v| !v.free));
    expedition_tests::trip_expedition(&state);
}

#[test]
fn a_plan_whose_vehicle_has_no_inputs_at_the_place_is_not_given_one() {
    let rules = needing_a_vehicle(&["ox_cart"]);
    let mut state = trade_world(&rules, false);
    let mut net = Net::default().both("home", "dest", 6);
    let report = run_opening(&rules, &mut state, start(), 5, &mut net, &mut ()).unwrap();
    assert_eq!(
        report.looks.given, 0,
        "no hardwood and no cattle at the place"
    );
    assert!(peasants_vehicles(&state).is_empty());
}

/// Orders the trip of `expedition_tests::the_plan` for the peasant at hour `at` and starts it there, as a look would.
fn start_trip(rules: &Rules, state: &mut EconomyState, out: u64, back: u64, now: u64) {
    let plan = expedition_tests::the_plan(rules, out, back);
    state.order_trade(rules, 0, PEASANT, plan, now).unwrap();
    trade::start_new_job(rules, state, 0, now).unwrap();
}

/// What a state's trade holds that the opening and the unbroken game must agree on.
fn trade_of(state: &EconomyState) -> Vec<u8> {
    let mut bytes = Vec::new();
    for place in &state.places {
        place.trade.encode(&mut bytes).unwrap();
        for pop in &place.pops {
            bytes.extend(pop.stock.iter().flat_map(|units| units.to_le_bytes()));
        }
    }
    bytes
}

#[test]
fn a_trip_put_part_way_out_is_where_the_unbroken_game_would_have_it_at_draws_0_and_999_and_for_a_short_trip()
 {
    let start = hours(at(10, 6));
    // The hour the opening looked in: the hour before the first tick.
    let now = start - 1;
    // Trips of 60 and of 1 outbound hours, over every draw that matters; the first tick is a morning and the span crosses a noon.
    for (out, back) in [(60_u64, 30_u64), (1, 4), (30, 10)] {
        for draw in [0_u64, 1, 250, 500, 999] {
            let mut rules = herd_rules();
            let mut opened = expedition_tests::two_places(&mut rules);
            start_trip(&rules, &mut opened, out, back, now);
            let moved = expedition::advance_opening(&rules, &mut opened.places[0], 1, draw, now)
                .unwrap_or_else(|error| panic!("{out} {draw}: {error}"));
            // The unbroken game: the same trip ordered `moved` hours earlier and ticked through to the hour before the first tick.
            let mut unbroken = expedition_tests::two_places(&mut rules);
            let departed = now - moved;
            start_trip(&rules, &mut unbroken, out, back, departed);
            for hour in departed + 1..=now {
                expedition_tests::tick(&rules, &mut unbroken, date_of(hour));
            }
            assert_eq!(
                trade_of(&opened),
                trade_of(&unbroken),
                "trip of {out} outbound hours at draw {draw} ({moved} hours along)"
            );
            let expedition = expedition_tests::trip_expedition(&opened);
            assert_eq!(expedition.left, departed);
            if out <= 1 {
                assert_eq!(moved, 0, "a trip of an hour or less starts at 0");
            }
            assert_eq!(expedition.phase, Phase::Outbound, "never at its arrival");
        }
    }
}

#[test]
fn the_first_look_of_a_pop_with_no_job_is_a_last_look_within_the_timer_and_its_next_look_is_in_the_first_30_days()
 {
    let mut rules = rules_with(&["ox_cart"]);
    rules.expedition.looks = true;
    // Nobody has anything to trade for: no trader makes a job.
    let mut state = world(vec![
        empty_home(&rules, "home"),
        empty_home(&rules, "other"),
    ]);
    for item in &mut state.places {
        settle_the_rest(&rules, item, &[]);
    }
    let mut net = Net::default().both("home", "other", 6);
    let report = run_opening(&rules, &mut state, start(), 11, &mut net, &mut ()).unwrap();
    assert_eq!(report.looks.jobs, 0);
    assert_eq!(report.last_looks, 6);
    let mut spread = BTreeSet::new();
    for place in &state.places {
        for look in place.trade.looks.values() {
            assert!(look.last <= start() && look.last > start() - 720);
            assert_eq!(look.rest, None);
            spread.insert(look.last);
        }
    }
    assert!(
        spread.len() > 1,
        "the last looks are spread, not all at one hour"
    );
    // Each is next due 720 hours after its last, within the first 30 days.
    for place in &state.places {
        for look in place.trade.looks.values() {
            let due = look.last + 720;
            assert!(due > start() && due <= start() + 720);
        }
    }
}

/// One game hour as the scheduler runs it with looks on: at noon the economy's daily step first, then the trade hour and the looks.
fn step(rules: &Rules, state: &mut EconomyState, net: &mut Net, hour: u64) {
    let date = date_of(hour);
    if date.hour == 12 {
        let prices: Vec<Vec<u64>> = state
            .places
            .iter()
            .map(|p| p.prices_milli.clone())
            .collect();
        for (place, opening) in state.places.iter_mut().zip(&prices) {
            trade::take_waiting(rules, place, opening).unwrap();
        }
        swap::run_noon_exchange(rules, state, &prices, &BTreeMap::new(), hour).unwrap();
        for place in &mut state.places {
            trade::start_ready(rules, place, &mut state.trade, hour, true).unwrap();
            trade::count_days_away(place);
            expedition::feed(rules, place, hour).unwrap();
        }
    }
    run_trade_hour_with_looks(rules, state, date, net, &mut ()).unwrap();
}

#[test]
fn a_save_before_the_first_tick_advanced_past_the_first_noon_equals_the_unbroken_game_and_every_good_and_hand_counts_once()
 {
    let mut rules = rules_with(&["ox_cart"]);
    rules.expedition.looks = true;
    let mut state = trade_world(&rules, true);
    let totals = |state: &EconomyState| {
        let mut goods = vec![0_u64; rules.goods.len()];
        for place in &state.places {
            for pop in &place.pops {
                for (total, units) in goods.iter_mut().zip(&pop.stock) {
                    *total += units;
                }
            }
            for job in &place.trade.jobs {
                for (total, units) in goods.iter_mut().zip(&job.stock) {
                    *total += units;
                }
            }
            for expedition in &place.trade.expeditions {
                for (good, total) in goods.iter_mut().enumerate() {
                    *total += expedition.cargo[good] + expedition.supplies[good];
                }
            }
        }
        goods
    };
    let before = totals(&state);
    let mut net = Net::default().both("home", "dest", 30);
    let report = run_opening(&rules, &mut state, start(), 5, &mut net, &mut ()).unwrap();
    assert!(report.looks.jobs >= 1);
    // Nothing is made or lost by the opening but what it gave and what the hours already travelled ate and wore: goods that are neither
    // food nor wear are counted exactly once, in the stores, a gathering job's stock or an expedition.
    let after = totals(&state);
    for (index, item) in rules.goods.iter().enumerate() {
        let eaten_or_worn = item.hunger_centi > 0 || item.id == "hardwood" || item.id == "hides";
        if !eaten_or_worn {
            assert_eq!(
                after[index],
                before[index] + report.goods[index].0,
                "{} is counted once",
                item.id
            );
        }
    }
    // Hands: nobody is held twice, and no pop holds more than its workers.
    for place in &state.places {
        for pop in 0..place.pops.len() {
            assert!(place.trade.held(pop) <= place.pops[pop].workers);
        }
    }
    validate_trade_state_at(&rules, &state, start()).unwrap();
    // A save before the first tick loads to the same opening and, advanced past the first noon, is the unbroken game.
    let saved = state.to_bytes().unwrap();
    let mut loaded = EconomyState::from_bytes(&saved).unwrap();
    assert_eq!(loaded.to_bytes().unwrap(), saved);
    validate_trade_state_at(&rules, &loaded, start()).unwrap();
    let mut net_a = Net::default().both("home", "dest", 30);
    let mut net_b = Net::default().both("home", "dest", 30);
    for hour in start()..start() + 30 {
        step(&rules, &mut state, &mut net_a, hour);
        step(&rules, &mut loaded, &mut net_b, hour);
    }
    assert_eq!(state.to_bytes().unwrap(), loaded.to_bytes().unwrap());
}

#[test]
fn the_grant_follows_the_clipped_hours_where_they_cross_half_the_limit_and_the_centre_hours_only_order_the_candidates()
 {
    let mut rules = rules_with(&[]);
    rules.expedition.looks = true;
    let build = |clip: u64| {
        let mut home = empty_home(&rules, "home");
        let mut far = empty_home(&rules, "far");
        fishery(&rules, &mut far);
        for item in [&mut home, &mut far] {
            settle_the_rest(&rules, item, &["fish"]);
        }
        let mut state = world(vec![home, far]);
        let mut net = Net {
            clip,
            ..Net::default().both("home", "far", 84)
        };
        run_opening(&rules, &mut state, start(), 7, &mut net, &mut ()).unwrap();
        given(&state, 0, PEASANT, "fish", &rules)
    };
    // 84 hours centre to centre is half the peasant's 168-hour limit: nothing. Clipped by 2 hours it is 82, and the grant is made.
    assert_eq!(build(0), 0);
    assert!(build(2) > 0);
}

#[test]
fn a_scenario_that_starts_within_a_look_timer_of_the_calendars_first_hour_is_refused_not_panicked_on()
 {
    let mut rules = rules_with(&[]);
    rules.expedition.looks = true;
    let mut state = world(vec![
        empty_home(&rules, "home"),
        empty_home(&rules, "other"),
    ]);
    let mut net = Net::default().both("home", "other", 6);
    for hour in [0, 1, 100, rules.expedition.look_hours] {
        assert!(
            run_opening(&rules, &mut state, hour, 11, &mut net, &mut ()).is_err(),
            "{hour}"
        );
    }
    // One hour more than the timer is the earliest start, and the idle last looks stay in the calendar.
    let first = rules.expedition.look_hours + 1;
    run_opening(&rules, &mut state, first, 11, &mut net, &mut ()).unwrap();
    assert!(
        state
            .places
            .iter()
            .flat_map(|p| p.trade.looks.values())
            .all(|look| look.last >= 1)
    );
}

#[test]
fn a_producer_that_overtakes_an_earlier_one_after_clipping_is_the_nearest() {
    let mut rules = rules_with(&[]);
    rules.expedition.looks = true;
    let mut home = empty_home(&rules, "home");
    let (mut first, mut second) = (empty_home(&rules, "first"), empty_home(&rules, "second"));
    fishery(&rules, &mut first);
    fishery(&rules, &mut second);
    for item in [&mut home, &mut first, &mut second] {
        settle_the_rest(&rules, item, &["fish"]);
    }
    let mut state = world(vec![home, first, second]);
    // `first` is 85 hours centre to centre and stays 85 (past half the limit: nothing); `second` is 88 and clips to 80.
    let mut net = Net::default()
        .both("home", "first", 85)
        .both("home", "second", 88);
    net.clips.insert("second".to_owned(), 8);
    let seed = 42;
    run_opening(&rules, &mut state, start(), seed, &mut net, &mut ()).unwrap();
    let share = rules.expedition.opening.share_pm[0]
        + herd_draw(
            seed,
            &["home", &PEASANT.to_string(), "opening-share", "fish"],
        ) % (rules.expedition.opening.share_pm[1] - rules.expedition.opening.share_pm[0] + 1);
    let target = u128::from(target(&rules, &state.places[0], PEASANT, "fish"));
    assert_eq!(
        given(&state, 0, PEASANT, "fish", &rules),
        opening_amount(&rules.expedition.opening, target, share, &whole(80), 168)
    );
    assert!(given(&state, 0, PEASANT, "fish", &rules) > 0);
}

#[test]
fn the_vehicles_a_plan_lacks_are_makeable_only_from_one_shared_pool() {
    use super::choose::Available;
    let rules = fixtures::shipped();
    let kind = |id: &str| rules.vehicles.iter().find(|v| v.id == id).unwrap();
    let (hand_cart, currach, ox_cart) = (kind("hand_cart"), kind("currach"), kind("ox_cart"));
    let pool = |wood: u128, hides: u128, cattle: u64| {
        let mut goods = vec![0_u128; rules.goods.len()];
        goods[good(&rules, "hardwood")] = wood;
        goods[good(&rules, "hides")] = hides;
        let mut animals = vec![0_u64; rules.herd_species.len()];
        let species = rules
            .herd_species
            .iter()
            .position(|s| s.animal == "cattle")
            .unwrap();
        animals[species] = cattle;
        Available { goods, animals }
    };
    // A hand cart takes 2 hardwood and a currach 4 hardwood and 4 hides: either alone is makeable from 4 wood, both are not.
    let four = pool(4, 4, 0);
    assert!(four.makes_all(&rules, &[hand_cart]) && four.makes_all(&rules, &[currach]));
    assert!(
        !four.makes_all(&rules, &[hand_cart, currach]),
        "the same four wood is not counted twice"
    );
    // With six wood both are.
    assert!(pool(6, 4, 0).makes_all(&rules, &[hand_cart, currach]));
    // A covered wagon takes 6 wood, 8 fabric (a hide or 8 lb of linen supplies it) and a horse; the currach takes its 4 hides by name. With
    // 10 wood, 4 hides, 8 linen and a horse spare both are makeable in either order: the wagon's fabric is the linen, not a hide.
    let wagon = kind("covered_wagon");
    let mut goods = vec![0_u128; rules.goods.len()];
    goods[good(&rules, "hardwood")] = 10;
    goods[good(&rules, "hides")] = 4;
    goods[good(&rules, "linen")] = 8;
    let mut animals = vec![0_u64; rules.herd_species.len()];
    animals[rules
        .herd_species
        .iter()
        .position(|s| s.animal == "horses")
        .unwrap()] = 1;
    let flexible = Available { goods, animals };
    assert!(
        flexible.makes_all(&rules, &[wagon, currach]),
        "the wagon first"
    );
    assert!(
        flexible.makes_all(&rules, &[currach, wagon]),
        "the currach first"
    );
    // Where first-fit fails and another substitute is needed: two wagons need 16 fabric. Hides come before linen in the goods' order and each
    // supplies 8, so a first-fit that gives the first wagon a hide and the second the other hide works, but with one hide and 8 linen and
    // a second flexible consumer the search must still find hide plus linen (it tries each substitute first).
    let mut goods = vec![0_u128; rules.goods.len()];
    goods[good(&rules, "hardwood")] = 12;
    goods[good(&rules, "hides")] = 1;
    goods[good(&rules, "linen")] = 8;
    let mut animals = vec![0_u64; rules.herd_species.len()];
    animals[rules
        .herd_species
        .iter()
        .position(|s| s.animal == "horses")
        .unwrap()] = 2;
    let two_wagons = Available { goods, animals };
    assert!(
        two_wagons.makes_all(&rules, &[wagon, wagon]),
        "a hide for one wagon, linen for the other"
    );
    // And it is not made up: with the linen short by a pound the pair is refused.
    let mut short = two_wagons.goods.clone();
    short[good(&rules, "linen")] = 7;
    let short = Available {
        goods: short,
        animals: two_wagons.animals.clone(),
    };
    assert!(!short.makes_all(&rules, &[wagon, wagon]));
    // Greedy quantities fail where one of each substitute per vehicle fits: two vehicles of 9 fabric each from two hides (8 each) and two
    // linen (1 each) take a hide and a linen apiece.
    let mut nine = wagon.clone();
    for input in &mut nine.inputs {
        if input.category.as_deref() == Some("fabric") {
            input.amount = 9;
        }
    }
    let mut goods = vec![0_u128; rules.goods.len()];
    goods[good(&rules, "hardwood")] = 12;
    goods[good(&rules, "hides")] = 2;
    goods[good(&rules, "linen")] = 2;
    let mut animals = vec![0_u64; rules.herd_species.len()];
    animals[rules
        .herd_species
        .iter()
        .position(|s| s.animal == "horses")
        .unwrap()] = 2;
    let nines = Available { goods, animals };
    assert!(
        nines.makes_all(&rules, &[&nine, &nine]),
        "a hide and a linen for each"
    );
    let mut short = nines.goods.clone();
    short[good(&rules, "linen")] = 1;
    let short = Available {
        goods: short,
        animals: nines.animals.clone(),
    };
    assert!(
        !short.makes_all(&rules, &[&nine, &nine]),
        "18 fabric are needed and 17 are there"
    );
    // One head of cattle makes one ox-cart, not two.
    let one = pool(8, 0, 1);
    assert!(one.makes_all(&rules, &[ox_cart]));
    assert!(!one.makes_all(&rules, &[ox_cart, ox_cart]));
}

/// Whether every wanted `(category, per-mille)` can be given some whole units of each good that supplies it, by trying every number of
/// units of every good (no more of a good than the want needs), the pool shrinking as it goes.
fn give(rules: &Rules, wanted: &[(String, u128)], goods: &mut [u128]) -> bool {
    let Some(((category, need), rest)) = wanted.split_first() else {
        return true;
    };
    pick(rules, (category, *need), 0, goods, rest)
}

/// One want's units, choosing the goods from `from` on.
fn pick(
    rules: &Rules,
    (category, need): (&str, u128),
    from: usize,
    goods: &mut [u128],
    rest: &[(String, u128)],
) -> bool {
    if need == 0 {
        return give(rules, rest, goods);
    }
    for index in from..rules.goods.len() {
        let each = rules.goods[index].category_supplied(category, 1);
        if each == 0 {
            continue;
        }
        let most = goods[index].min(need.div_ceil(each));
        for units in 1..=most {
            goods[index] -= units;
            let left = need.saturating_sub(rules.goods[index].category_supplied(category, units));
            let found = pick(rules, (category, left), index + 1, goods, rest);
            goods[index] += units;
            if found {
                return true;
            }
        }
    }
    false
}

/// Whether the kinds could be made from the pool, by brute force and independent of `Available::makes_all`: the named inputs and animals
/// are deducted, then every number of units of every good for every category input is tried (no more of a good than the input needs).
fn oracle(rules: &Rules, goods: &[u128], animals: &[u64], kinds: &[&VehicleKind]) -> bool {
    let (mut goods, mut animals) = (goods.to_vec(), animals.to_vec());
    let mut wanted: Vec<(String, u128)> = Vec::new();
    for kind in kinds {
        for input in &kind.inputs {
            match (&input.category, &input.good) {
                (Some(category), _) => {
                    wanted.push((category.clone(), u128::from(input.amount) * 1000));
                }
                (None, Some(id)) => {
                    let index = rules.goods.iter().position(|g| g.id == *id).unwrap();
                    if goods[index] < u128::from(input.amount) {
                        return false;
                    }
                    goods[index] -= u128::from(input.amount);
                }
                (None, None) => return false,
            }
        }
        for animal in &kind.animals {
            let index = rules
                .herd_species
                .iter()
                .position(|s| s.animal == animal.species)
                .unwrap();
            if animals[index] < animal.count {
                return false;
            }
            animals[index] -= animal.count;
        }
    }
    give(rules, &wanted, &mut goods)
}

#[test]
fn the_joint_check_agrees_with_a_brute_force_oracle_over_a_grid_of_stocks_and_vehicle_sets() {
    use super::choose::Available;
    let rules = fixtures::shipped();
    let kind = |id: &str| rules.vehicles.iter().find(|v| v.id == id).unwrap().clone();
    let (wagon, currach, hand_cart, ox_cart) = (
        kind("covered_wagon"),
        kind("currach"),
        kind("hand_cart"),
        kind("ox_cart"),
    );
    // A wagon whose fabric is 9: a hide (8) and a linen (1) make it, as the reviewer's case.
    let mut nine = wagon.clone();
    for input in &mut nine.inputs {
        if input.category.as_deref() == Some("fabric") {
            input.amount = 9;
        }
    }
    let sets: Vec<Vec<&VehicleKind>> = vec![
        vec![&wagon],
        vec![&wagon, &wagon],
        vec![&wagon, &currach],
        vec![&currach, &wagon],
        vec![&nine, &nine],
        vec![&nine, &wagon, &currach],
        vec![&hand_cart, &currach],
        vec![&ox_cart, &ox_cart],
    ];
    let species = |id: &str| {
        rules
            .herd_species
            .iter()
            .position(|s| s.animal == id)
            .unwrap()
    };
    let (mut agree, mut yes, mut no) = (0_u32, 0_u32, 0_u32);
    for hardwood in [0_u128, 6, 10, 12] {
        for hides in [0_u128, 1, 2, 3] {
            for linen in [0_u128, 2, 8, 9] {
                for hemp in [0_u128, 1, 8] {
                    for (horses, cattle) in [(0_u64, 0_u64), (1, 1), (2, 2)] {
                        let mut goods = vec![0_u128; rules.goods.len()];
                        goods[good(&rules, "hardwood")] = hardwood;
                        goods[good(&rules, "hides")] = hides;
                        goods[good(&rules, "linen")] = linen;
                        goods[good(&rules, "hemp")] = hemp;
                        let mut animals = vec![0_u64; rules.herd_species.len()];
                        animals[species("horses")] = horses;
                        animals[species("cattle")] = cattle;
                        let pool = Available {
                            goods: goods.clone(),
                            animals: animals.clone(),
                        };
                        for kinds in &sets {
                            let expected = oracle(&rules, &goods, &animals, kinds);
                            assert_eq!(
                                pool.makes_all(&rules, kinds),
                                expected,
                                "wood {hardwood}, hides {hides}, linen {linen}, hemp {hemp}, horses {horses}, cattle {cattle}, {} vehicles starting {}",
                                kinds.len(),
                                kinds[0].id
                            );
                            agree += 1;
                            if expected {
                                yes += 1;
                            } else {
                                no += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(
        yes > 100 && no > 100,
        "the grid has both answers: {yes} yes, {no} no of {agree}"
    );
}
