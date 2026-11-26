//! CK-432 (trade part C): expeditions.
//!
//! The cases are the spec's coverage for C (section 12): a plan carried out with the slowest member setting the pace, the clipped
//! journey and hourly unloading; a cart and a boat with a transfer and a ford; people on foot driving pigs; a return that would not
//! assign refused at the swap; the crew and the drovers; arrival before and at noon, each also after a save; and an expired job's
//! animal going to a herd on unloading. Each runs on the swap tests' fixture place and its rules. Nothing sleeps and nothing reads a
//! counter added for it.

use super::expedition::{Assigned, Delays, assign_leg, pace_of, ratio, species_of};
use std::collections::BTreeMap;

use ck_sim::{Canonical, GameTime, Reader};

use super::swap_tests::{
    MONK, PEASANT, THIRD, add_building, capacity, cattle_farm, good, head, herd_rules, place,
    set_price, settle_the_rest, spare, stock, target, want,
};
use super::*;

pub(crate) fn kind(rules: &Rules, id: &str) -> u32 {
    u32::try_from(rules.vehicles.iter().position(|v| v.id == id).unwrap()).unwrap()
}

fn vehicles(rules: &Rules, kinds: &[&str]) -> Vec<ExpeditionVehicle> {
    kinds
        .iter()
        .enumerate()
        .map(|(index, id)| ExpeditionVehicle {
            number: u64::try_from(index).unwrap() + 1,
            kind: kind(rules, id),
            worn_hours: ratio(0, 1),
        })
        .collect()
}

/// A level road of 6,000 work at a base speed of 1,500 metres an hour, which takes 4 hours at walking pace (spec 11b.3 rule 8).
fn four_hour_road() -> Leg {
    Leg {
        mode: Mode::Land,
        terms: vec![Term {
            work: ratio(6000, 1),
            speed: Speed::Plain(1500),
        }],
        fixed: ratio(0, 1),
        delays: Delays::Along,
        key: [0; 4],
        along: Vec::new(),
        path: Vec::new(),
    }
}

fn goods(rules: &Rules, list: &[(&str, u64)]) -> Vec<u64> {
    let mut load = vec![0; rules.goods.len()];
    for (id, units) in list {
        load[good(rules, id)] = *units;
    }
    load
}

#[test]
fn the_shipped_expedition_numbers_are_the_specs_and_every_herd_species_has_its_pace() {
    let rules = herd_rules();
    let numbers = &rules.expedition;
    assert_eq!(
        (numbers.pack_g, numbers.pack_ml, numbers.walking_pace_pm),
        (25_000, 50_000, 1000)
    );
    assert_eq!(
        (numbers.animals_per_drover, numbers.full_load_slowing_pm),
        (20, 250)
    );
    assert_eq!(
        (numbers.short_crew_slowing_pm, numbers.no_supplies_pace_pm),
        (300, 500)
    );
    let pace = |animal: &str| numbers.driven(animal).unwrap().pace_pm;
    assert_eq!(
        [pace("cattle"), pace("sheep"), pace("horses"), pace("pigs")],
        [800, 800, 800, 600]
    );
    // A species without numbers cannot be taken: an assignment that needs its pace or room is not possible.
    let mut without = rules.clone();
    without
        .expedition
        .animals
        .retain(|animal| animal.animal != "pigs");
    let pigs = good(&without, "pigs");
    let mut cargo = vec![0; without.goods.len()];
    cargo[pigs] = 1;
    let none = vec![0; without.goods.len()];
    assert!(assign_leg(&without, &[], 2, &cargo, &none, Mode::Land).is_some());
    let boat = vehicles(&without, &["simple_barge"]);
    assert!(assign_leg(&without, &boat, 3, &cargo, &none, Mode::Water).is_none());
    let mut bad = rules.clone();
    bad.expedition.animals_per_drover = 0;
    assert!(bad.validate("fixture").is_err());
}

#[test]
fn a_leg_takes_the_pace_scaled_speed_with_its_fixed_delays_added_unchanged() {
    // The worked times of 11b.3 rule 8, carried as exact fractions.
    let leg = four_hour_road();
    assert_eq!(leg.hours(1000), ratio(4, 1));
    assert_eq!(
        leg.hours(800),
        ratio(5, 1),
        "an empty, fully crewed ox-cart"
    );
    assert_eq!(leg.hours(1300), ratio(40, 13), "a wagon: 3.08 hours");
    assert_eq!(
        leg.hours(560),
        ratio(50, 7),
        "an ox-cart at one of two crew: 7.14"
    );
    assert_eq!(
        leg.hours(600),
        ratio(20, 3),
        "an ox-cart with a full load: 6.67"
    );
    let mut ford = four_hour_road();
    ford.fixed = ratio(3, 4);
    assert_eq!(
        ford.hours(800),
        ratio(23, 4),
        "a ford adds the same at every pace"
    );
    assert_eq!(ford.hours(1300), ratio(40, 13) + ratio(3, 4));
    // Each profile speed is rounded down, and is never below 1.
    let slow = Leg {
        mode: Mode::Land,
        terms: vec![Term {
            work: ratio(10, 1),
            speed: Speed::Plain(3),
        }],
        fixed: ratio(0, 1),
        delays: Delays::Along,
        key: [0; 4],
        along: Vec::new(),
        path: Vec::new(),
    };
    assert_eq!(
        slow.hours(100),
        ratio(10, 1),
        "3 at a tenth is 0, and a speed is at least 1"
    );
    // A road's path and road speeds are scaled and then mixed by its condition, as CK-402 mixes them.
    let road = Leg {
        mode: Mode::Land,
        terms: vec![Term {
            work: ratio(1000, 1),
            speed: Speed::Road {
                path: 1000,
                road: 2000,
                condition: 500,
            },
        }],
        fixed: ratio(0, 1),
        delays: Delays::Along,
        key: [0; 4],
        along: Vec::new(),
        path: Vec::new(),
    };
    assert_eq!(road.hours(1000), ratio(2, 3));
    assert_eq!(road.hours(500), ratio(4, 3));
    // A leg with nothing to travel takes no time.
    let nothing = Leg {
        mode: Mode::Land,
        terms: Vec::new(),
        fixed: ratio(0, 1),
        delays: Delays::Along,
        key: [0; 4],
        along: Vec::new(),
        path: Vec::new(),
    };
    assert_eq!(nothing.hours(1000), ratio(0, 1));
}

#[test]
fn the_slowest_member_sets_the_pace_and_each_vehicles_load_and_crew_lower_its_own() {
    let rules = herd_rules();
    let convoy = vehicles(&rules, &["ox_cart", "covered_wagon"]);
    // An empty ox-cart at full crew (800) with a fully loaded wagon at one of two crew: floor(floor(1300 x 750 / 1000) x 700 / 1000).
    let assigned = Assigned {
        moving: vec![(0, 2, 0), (1, 1, 700_000)],
        walkers: 0,
        drovers: 0,
        driven: Vec::new(),
        loads: vec![(0, 0), (700_000, 0)],
    };
    let (pace, limiter) = pace_of(&rules, &convoy, &assigned, Mode::Land, false, false);
    assert_eq!(pace, 682);
    assert_eq!(limiter, Limiter::Vehicle(kind(&rules, "covered_wagon")));
    // Out of supplies it goes at half of that, rounded down.
    let (pace, _) = pace_of(&rules, &convoy, &assigned, Mode::Land, true, false);
    assert_eq!(pace, 341);
    // A hand cart's crew is always its full: it never slows for it.
    let cart = vehicles(&rules, &["hand_cart"]);
    let alone = Assigned {
        moving: vec![(0, 1, 0)],
        walkers: 0,
        drovers: 0,
        driven: Vec::new(),
        loads: vec![(0, 0)],
    };
    assert_eq!(
        pace_of(&rules, &cart, &alone, Mode::Land, false, false).0,
        1000
    );
    // A person walking beside a faster wagon holds it to walking pace.
    let wagon = vehicles(&rules, &["covered_wagon"]);
    let with_walker = Assigned {
        moving: vec![(0, 2, 0)],
        walkers: 1,
        drovers: 0,
        driven: Vec::new(),
        loads: vec![(0, 0)],
    };
    let (pace, limiter) = pace_of(&rules, &wagon, &with_walker, Mode::Land, false, false);
    assert_eq!((pace, limiter), (1000, Limiter::Walkers));
}

#[test]
fn a_two_person_ox_cart_plan_drives_twenty_sheep_with_one_crew_and_one_drover_and_refuses_twenty_one()
 {
    let rules = herd_rules();
    let cart = vehicles(&rules, &["ox_cart"]);
    let sheep = good(&rules, "sheep");
    let mut cargo = vec![0; rules.goods.len()];
    cargo[sheep] = 20;
    let none = vec![0; rules.goods.len()];
    let assigned = assign_leg(&rules, &cart, 2, &cargo, &none, Mode::Land).expect("possible");
    assert_eq!(
        (assigned.moving[0].1, assigned.drovers, assigned.walkers),
        (1, 1, 0)
    );
    // The cart is at its fewest crew, so its short-crew factor applies: 800 x 700 / 1000.
    let (pace, limiter) = pace_of(&rules, &cart, &assigned, Mode::Land, false, false);
    assert_eq!(pace, 560);
    assert_eq!(limiter, Limiter::Vehicle(kind(&rules, "ox_cart")));
    // The sheep walk at 800, which the cart's 560 is below.
    cargo[sheep] = 21;
    assert!(
        assign_leg(&rules, &cart, 2, &cargo, &none, Mode::Land).is_none(),
        "21 sheep need two drovers and the cart its one crew"
    );
    assert!(assign_leg(&rules, &cart, 3, &cargo, &none, Mode::Land).is_some());
}

#[test]
fn on_foot_each_person_carries_a_pack_and_driven_pigs_set_the_pace_at_600() {
    let rules = herd_rules();
    let pigs = good(&rules, "pigs");
    let oats = good(&rules, "oats");
    let mut cargo = vec![0; rules.goods.len()];
    cargo[pigs] = 5;
    let mut supplies = vec![0; rules.goods.len()];
    supplies[oats] = 2;
    let assigned = assign_leg(&rules, &[], 3, &cargo, &supplies, Mode::Land).expect("possible");
    assert_eq!(assigned.drovers, 1);
    assert_eq!(assigned.walkers, 2);
    let (pace, limiter) = pace_of(&rules, &[], &assigned, Mode::Land, false, true);
    assert_eq!(pace, 600);
    assert_eq!(
        limiter,
        Limiter::Species(u32::try_from(species_of(&rules, pigs).unwrap()).unwrap())
    );
    // No animals: walking pace, set by the walkers.
    let plain = assign_leg(
        &rules,
        &[],
        3,
        &vec![0; rules.goods.len()],
        &supplies,
        Mode::Land,
    )
    .unwrap();
    assert_eq!(
        pace_of(&rules, &[], &plain, Mode::Land, false, true),
        (1000, Limiter::Walkers)
    );
    // A pack holds 25 kg: a load heavier than the people's packs together cannot be taken, and nobody can go by water on foot.
    let heavy = goods(&rules, &[("oats", 100_000)]);
    assert!(
        assign_leg(
            &rules,
            &[],
            3,
            &heavy,
            &vec![0; rules.goods.len()],
            Mode::Land
        )
        .is_none()
    );
    assert!(assign_leg(&rules, &[], 3, &cargo, &supplies, Mode::Water).is_none());
}

#[test]
fn boats_ride_in_carts_on_land_and_carts_ride_in_boats_on_water_by_weight_and_space() {
    let rules = herd_rules();
    let none = vec![0; rules.goods.len()];
    // A currach takes 2 m3 when carried, so it never goes in an ox-cart's 1.5 m3 (11b.3 rule 9) but does in a wagon's 2.5 m3.
    let in_cart = vehicles(&rules, &["ox_cart", "currach"]);
    assert!(
        assign_leg(&rules, &in_cart, 6, &none, &none, Mode::Land).is_none(),
        "a currach's 2 m3 never goes in an ox-cart's 1.5 m3"
    );
    let in_wagon = vehicles(&rules, &["covered_wagon", "currach"]);
    let on_land = assign_leg(&rules, &in_wagon, 6, &none, &none, Mode::Land)
        .expect("the wagon carries the currach");
    assert_eq!(on_land.moving.len(), 1);
    assert_eq!(
        on_land.moving[0].2, 150_000,
        "the currach's weight is the wagon's load"
    );
    // A barge cannot be carried, so no leg of land can be made with one.
    let barge = vehicles(&rules, &["hand_cart", "simple_barge"]);
    assert!(assign_leg(&rules, &barge, 6, &none, &none, Mode::Land).is_none());
    // On water the boat moves with its crew and the cart rides in it.
    let on_water = assign_leg(&rules, &barge, 6, &none, &none, Mode::Water)
        .expect("the barge carries the cart");
    assert_eq!(on_water.moving.len(), 1);
    assert_eq!(on_water.moving[0].1, 3, "the barge's full crew");
    assert_eq!(on_water.walkers, 3);
}

/// A date in March 768 at an hour.
pub(crate) fn at(day: u8, hour: u8) -> Date {
    Date {
        year: 768,
        month: 3,
        day,
        hour,
    }
}

pub(crate) fn hours(date: Date) -> u64 {
    GameTime::from_date(date).unwrap().hours()
}

/// One game hour as the scheduler runs it: at noon the economy's daily step first (the jobs' take, the exchange with the expeditions
/// that arrived by 11:00, the starts, the days away and the expeditions' eating), then the trade hour (11c.1).
pub(crate) fn tick(rules: &Rules, state: &mut EconomyState, date: Date) {
    if date.hour == 12 {
        let now = hours(date);
        let prices: Vec<Vec<u64>> = state
            .places
            .iter()
            .map(|p| p.prices_milli.clone())
            .collect();
        for (place, opening) in state.places.iter_mut().zip(&prices) {
            trade::take_waiting(rules, place, opening).unwrap();
        }
        swap::run_noon_exchange(rules, state, &prices, &BTreeMap::new(), now).unwrap();
        for place in &mut state.places {
            trade::start_ready(rules, place, &mut state.trade, now, true).unwrap();
            trade::count_days_away(place);
            expedition::feed(rules, place, now).unwrap();
        }
    }
    run_trade_hour(rules, state, date).unwrap();
}

/// A leg that takes `hours` whatever the pace, so the clock is plain.
fn fixed_leg(mode: Mode, hours: u64) -> Leg {
    Leg {
        mode,
        terms: Vec::new(),
        fixed: ratio(u128::from(hours), 1),
        delays: Delays::Along,
        key: [0; 4],
        along: Vec::new(),
        path: Vec::new(),
    }
}

/// A home place whose peasant holds the cargo (10 barley) and the supplies (8 oats) of a trip to a destination whose monks want the
/// barley and have fish to spare, and an ox-cart of the peasant's. Everything else at both places is at its target, so nothing else
/// is swapped.
pub(crate) fn two_places(rules: &mut Rules) -> EconomyState {
    let mut home = place(rules, 3);
    home.id = "home".to_owned();
    let mut dest = place(rules, 3);
    dest.id = "dest".to_owned();
    for place in [&mut home, &mut dest] {
        for id in ["oats", "barley", "fish"] {
            set_price(rules, place, id, 1);
        }
        settle_the_rest(rules, place, &[]);
    }
    home.pops[PEASANT].stock[good(rules, "barley")] += 10;
    home.pops[PEASANT].stock[good(rules, "oats")] += 8;
    want(rules, &mut dest, MONK, "barley", 10);
    dest.pops[MONK].stock[good(rules, "fish")] += 10;
    home.trade.vehicles.push(Vehicle {
        number: 1,
        kind: kind(rules, "ox_cart"),
        pop: u32::try_from(PEASANT).unwrap(),
        free: true,
    });
    EconomyState {
        places: vec![home, dest],
        trade: TradeCounters {
            vehicles: 1,
            jobs: 0,
            expeditions: 0,
        },
    }
}

/// The plan of the trip: the ox-cart and two of the peasant's own people, 10 barley out for 10 fish back, 8 oats as supplies, a way
/// each way of `out` and `back` hours.
pub(crate) fn the_plan(rules: &Rules, out: u64, back: u64) -> Plan {
    let mut plan = Plan {
        destination: 1,
        out: vec![fixed_leg(Mode::Land, out)],
        back: vec![fixed_leg(Mode::Land, back)],
        vehicles: vec![1],
        people: vec![(u32::try_from(PEASANT).unwrap(), 2)],
        cargo: vec![0; rules.goods.len()],
        wanted: vec![0; rules.goods.len()],
        supplies: vec![0; rules.goods.len()],
        arrival: None,
    };
    plan.cargo[good(rules, "barley")] = 10;
    plan.wanted[good(rules, "fish")] = 10;
    plan.supplies[good(rules, "oats")] = 8;
    plan
}

pub(crate) fn trip_expedition(state: &EconomyState) -> &Expedition {
    &state.places[0].trade.expeditions[0]
}

#[test]
fn map_icon_names_the_moving_vehicle() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    state
        .order_trade(&rules, 0, PEASANT, the_plan(&rules, 6, 5), hours(at(10, 5)))
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    let expedition = trip_expedition(&state);
    assert_eq!(
        expedition.map_vehicle(&rules),
        Some(kind(&rules, "ox_cart"))
    );
    let mut trip = expedition.clone();
    trip.cargo.fill(0);
    trip.loaded.fill(0);
    trip.reckoned.fill(0);
    trip.groups[0].count = 4;
    trip.vehicles = vehicles(&rules, &["covered_wagon", "ox_cart"]);
    trip.reckoned_mode = Mode::Land;
    assert_eq!(trip.map_vehicle(&rules), Some(kind(&rules, "ox_cart")));
    trip.vehicles = vehicles(&rules, &["currach", "simple_barge"]);
    trip.reckoned_mode = Mode::Water;
    assert_eq!(trip.map_vehicle(&rules), Some(kind(&rules, "simple_barge")));
    trip.vehicles.clear();
    trip.reckoned_mode = Mode::Land;
    assert_eq!(trip.map_vehicle(&rules), None);
}

#[test]
fn map_path_split_uses_the_same_distance_as_the_travelling_position() {
    let mut leg = four_hour_road();
    leg.path = vec![[0, 0], [100, 0], [100, 300]];
    leg.along = vec![0, 100, 400];
    assert_eq!(leg.path_segment(&ratio(1, 2)), Some((1, ratio(1, 3))));
    assert_eq!(
        leg.position(&ratio(1, 2)),
        Some((ratio(100, 1), ratio(100, 1)))
    );
}

#[test]
fn an_expedition_that_arrives_at_11_trades_that_noon_starts_back_and_unloads_in_the_hour_it_comes_home()
 {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let (barley, fish, oats) = (
        good(&rules, "barley"),
        good(&rules, "fish"),
        good(&rules, "oats"),
    );
    let fish_before = stock(&rules, &state.places[0], PEASANT, "fish");
    let made = hours(at(10, 5));
    state
        .order_trade(&rules, 0, PEASANT, the_plan(&rules, 6, 5), made)
        .unwrap();
    assert!(
        !state.places[0].trade.vehicles[0].free,
        "the vehicle is reserved from the moment the job is made"
    );
    // The job is ready at once and starts in the hour it is made; the people are then away and the vehicle is not free.
    tick(&rules, &mut state, at(10, 5));
    assert!(state.places[0].trade.jobs.is_empty());
    assert_eq!(state.places[0].trade.held(PEASANT), 2);
    assert_eq!(trip_expedition(&state).phase, Phase::Outbound);
    assert_eq!(trip_expedition(&state).left, made);
    // Six hours of travel: it arrives in the step of 11:00, a whole hour before the noon step.
    for hour in 6..11 {
        tick(&rules, &mut state, at(10, hour));
        assert_eq!(
            trip_expedition(&state).phase,
            Phase::Outbound,
            "hour {hour}"
        );
    }
    tick(&rules, &mut state, at(10, 11));
    assert_eq!(trip_expedition(&state).phase, Phase::Waiting);
    assert_eq!(trip_expedition(&state).arrived, Some(hours(at(10, 11))));
    // The noon's exchange: 10 barley for 10 fish at the place's prices, the expedition starts back in the same noon step and
    // moves its first hour of the way home in that noon's trade hour.
    tick(&rules, &mut state, at(10, 12));
    let expedition = trip_expedition(&state);
    assert_eq!(expedition.phase, Phase::Homeward);
    assert!(expedition.traded);
    assert_eq!((expedition.cargo[fish], expedition.cargo[barley]), (10, 0));
    assert_eq!(
        stock(&rules, &state.places[1], MONK, "fish"),
        target(&rules, &state.places[1], MONK, "fish")
    );
    assert_eq!(
        state.places[1].pops[MONK].stock[barley],
        target(&rules, &state.places[1], MONK, "barley")
    );
    // Five hours back from the noon step: 12, 13, 14, 15 and 16 are the five hours, so it is home in the step of 16:00.
    for hour in 13..16 {
        tick(&rules, &mut state, at(10, hour));
        assert_eq!(state.places[0].trade.expeditions.len(), 1, "hour {hour}");
    }
    tick(&rules, &mut state, at(10, 16));
    assert!(
        state.places[0].trade.expeditions.is_empty(),
        "unloaded in the hour it arrived"
    );
    assert_eq!(
        state.places[0].trade.held(PEASANT),
        0,
        "its people are home"
    );
    assert!(
        state.places[0].trade.vehicles[0].free,
        "and its vehicle free that hour"
    );
    // What it brought and what is left of its supplies are in the trader's store, and nothing is lost.
    assert_eq!(
        stock(&rules, &state.places[0], PEASANT, "fish"),
        fish_before + 10
    );
    assert!(state.places[0].pops[PEASANT].stock[oats] > 0);
    validate_trade_state(&rules, &state).unwrap();
}

#[test]
fn one_that_arrives_at_12_waits_for_the_next_noon() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let fish = good(&rules, "fish");
    state
        .order_trade(&rules, 0, PEASANT, the_plan(&rules, 7, 5), hours(at(10, 5)))
        .unwrap();
    for hour in 5..=12 {
        tick(&rules, &mut state, at(10, hour));
    }
    // Seven hours from the start at 5:00: its last hour of travel is the noon hour's step, after that noon's exchange.
    let expedition = trip_expedition(&state);
    assert_eq!(expedition.phase, Phase::Waiting);
    assert_eq!(expedition.arrived, Some(hours(at(10, 12))));
    assert!(!expedition.traded, "it has not traded at this noon");
    assert_eq!(expedition.cargo[fish], 0);
    for hour in 13..24 {
        tick(&rules, &mut state, at(10, hour));
    }
    for hour in 0..12 {
        tick(&rules, &mut state, at(11, hour));
        assert_eq!(
            trip_expedition(&state).phase,
            Phase::Waiting,
            "day 11, hour {hour}"
        );
    }
    tick(&rules, &mut state, at(11, 12));
    assert_eq!(trip_expedition(&state).phase, Phase::Homeward);
    assert_eq!(trip_expedition(&state).cargo[fish], 10);
}

#[test]
fn a_game_saved_at_any_hour_of_the_trip_and_loaded_goes_on_as_the_unbroken_game_would() {
    // Both arrival hours (a way out of 6 hours arrives at 11:00 and one of 7 at 12:00), saved at every hour of the trip, before the
    // first noon, between its noons and after the trade.
    for out in [6_u64, 7] {
        for save_hour in 5..=23_u8 {
            let mut rules = herd_rules();
            let mut unbroken = two_places(&mut rules);
            unbroken
                .order_trade(
                    &rules,
                    0,
                    PEASANT,
                    the_plan(&rules, out, 5),
                    hours(at(10, 5)),
                )
                .unwrap();
            for hour in 5..=save_hour {
                tick(&rules, &mut unbroken, at(10, hour));
            }
            // Saved and loaded in the middle of the trip, the position and its fractions included.
            let mut bytes = Vec::new();
            unbroken.encode(&mut bytes).unwrap();
            let mut loaded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
            assert_eq!(loaded, unbroken);
            validate_trade_state(&rules, &loaded).unwrap();
            for day in [10_u8, 11] {
                for hour in 0..24 {
                    if day == 10 && hour <= save_hour {
                        continue;
                    }
                    tick(&rules, &mut unbroken, at(day, hour));
                    tick(&rules, &mut loaded, at(day, hour));
                    assert_eq!(
                        loaded, unbroken,
                        "day {day} hour {hour}, out {out}, saved at {save_hour}"
                    );
                }
            }
            assert!(
                unbroken.places[0].trade.expeditions.is_empty(),
                "it is home by the end of day 11"
            );
        }
    }
}

pub(crate) fn date_of(hour: u64) -> Date {
    GameTime::from_hours(hour).date().unwrap()
}

/// A leg of `work` over a speed of 1,500 metres an hour and `fixed` hours of delay.
fn road(work: u128, fixed: (u128, u128)) -> Leg {
    Leg {
        mode: Mode::Land,
        terms: vec![Term {
            work: ratio(work, 1),
            speed: Speed::Plain(1500),
        }],
        fixed: ratio(fixed.0, fixed.1),
        delays: Delays::Along,
        key: [0; 4],
        along: Vec::new(),
        path: Vec::new(),
    }
}

pub(crate) fn add_vehicle(state: &mut EconomyState, rules: &Rules, id: &str) -> u64 {
    state.trade.vehicles += 1;
    let number = state.trade.vehicles;
    state.places[0].trade.vehicles.push(Vehicle {
        number,
        kind: kind(rules, id),
        pop: u32::try_from(PEASANT).unwrap(),
        free: true,
    });
    number
}

#[test]
fn a_convoy_travels_at_its_slowest_members_pace_and_carries_the_fractions_of_an_hour_to_the_next_leg()
 {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let wagon = add_vehicle(&mut state, &rules, "covered_wagon");
    let mut plan = the_plan(&rules, 6, 5);
    plan.vehicles = vec![1, wagon];
    plan.people = vec![(u32::try_from(PEASANT).unwrap(), 4)];
    plan.cargo[good(&rules, "barley")] = 18;
    // Two legs of 3,000 work each over 1,500 metres an hour.
    plan.out = vec![road(3000, (0, 1)), road(3000, (0, 1))];
    plan.back = vec![road(3000, (0, 1))];
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    // First fit in carrier order fills the ox-cart first: 18 barley and 7 of the 8 oats weigh 493.9 kg of its 500, so its load factor is
    // 1000 - floor(250 x 493,900 / 500,000) = 754 and its pace floor(800 x 754 / 1000) = 603, below the wagon's, which carries one oat.
    let expedition = trip_expedition(&state);
    assert_eq!(expedition.pace_pm, 603);
    assert_eq!(
        expedition.limiter,
        Limiter::Vehicle(kind(&rules, "ox_cart"))
    );
    // At 603 the speed is floor(1,500 x 603 / 1000) = 904, so a leg takes 3000 / 904 hours. After three hours 2,712 of its 3,000 are done.
    for hour in 6..9 {
        tick(&rules, &mut state, at(10, hour));
    }
    let expedition = trip_expedition(&state);
    assert_eq!(
        (expedition.leg, expedition.done.clone()),
        (0, ratio(113, 125))
    );
    // The fourth hour finishes the leg and the 0.18 hour left over is carried: 616 of the second leg's 3,000.
    tick(&rules, &mut state, at(10, 9));
    let expedition = trip_expedition(&state);
    assert_eq!(
        (expedition.leg, expedition.done.clone()),
        (1, ratio(77, 375))
    );
}

#[test]
fn a_cart_and_a_boat_change_what_moves_at_a_transfer_and_the_return_load_sets_the_pace_back() {
    let mut rules = herd_rules();
    // The shipped numbers let no cart carry a boat that can carry it back (a currach takes 2 m3 aboard a cart that holds 1.5, a wagon 5 m3
    // aboard a currach that holds 3), so the case gives the currach the room of a hand cart's load and the cart the room of a currach's.
    let currach = kind(&rules, "currach") as usize;
    rules.vehicles[currach].carried_g = Some(100_000);
    rules.vehicles[currach].carried_ml = Some(400_000);
    let mut state = two_places(&mut rules);
    // A barley is worth four fish, so one barley buys the four the trip is for.
    for place in &mut state.places {
        set_price(&rules, place, "barley", 4);
    }
    let boat = add_vehicle(&mut state, &rules, "currach");
    let (barley, oats, fish) = (
        good(&rules, "barley"),
        good(&rules, "oats"),
        good(&rules, "fish"),
    );
    let mut plan = the_plan(&rules, 1, 1);
    plan.vehicles = vec![1, boat];
    plan.vehicles[0] = add_vehicle(&mut state, &rules, "hand_cart");
    plan.people = vec![(u32::try_from(PEASANT).unwrap(), 4)];
    plan.cargo = vec![0; rules.goods.len()];
    plan.cargo[barley] = 1;
    plan.supplies = vec![0; rules.goods.len()];
    plan.supplies[oats] = 1;
    plan.wanted = vec![0; rules.goods.len()];
    plan.wanted[fish] = 4;
    let water = |hours: u128| Leg {
        mode: Mode::Water,
        terms: vec![Term {
            work: ratio(hours * 1700, 1),
            speed: Speed::Plain(1700),
        }],
        fixed: ratio(0, 1),
        delays: Delays::Along,
        key: [0; 4],
        along: Vec::new(),
        path: Vec::new(),
    };
    let transfer = || Leg {
        mode: Mode::Land,
        terms: Vec::new(),
        fixed: ratio(2, 1),
        delays: Delays::Along,
        key: [0; 4],
        along: Vec::new(),
        path: Vec::new(),
    };
    // The way out crosses a ford (three quarters of an hour of delay) on land, then boards; the way back lands and crosses the ford.
    plan.out = vec![road(1500, (3, 4)), transfer(), water(3)];
    plan.back = vec![water(3), transfer(), road(1500, (3, 4))];
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    // On land the hand cart moves with the currach in it: 100 + 21.8 + 14.5 = 136.3 kg of its 150 kg, so floor(1000 x (1000 - 227) / 1000).
    assert_eq!(trip_expedition(&state).pace_pm, 773);
    assert_eq!(
        trip_expedition(&state).limiter,
        Limiter::Vehicle(kind(&rules, "hand_cart"))
    );
    let mut seen = Vec::new();
    for step in 6..40 {
        tick(&rules, &mut state, date_of(hours(at(10, 5)) + step - 5));
        let Some(expedition) = state.places[0].trade.expeditions.first() else {
            break;
        };
        if expedition.phase != Phase::Outbound {
            break;
        }
        if seen.last() != Some(&(expedition.leg, expedition.pace_pm)) {
            seen.push((expedition.leg, expedition.pace_pm));
        }
    }
    // The pace is reckoned again on the water leg, where the currach moves with the hand cart aboard: 50 + 21.8 + 14.5 = 86.3 kg of its
    // 800 kg and its full crew of four: 1000 - floor(250 x 86,300 / 800,000) = 974.
    let water_pace = seen
        .iter()
        .find(|(leg, _)| *leg == 2)
        .map(|(_, pace)| *pace);
    assert_eq!(water_pace, Some(974));
    assert_eq!(trip_expedition(&state).phase, Phase::Waiting);
    // The noon exchange brings 4 fish, and the way back is lighter (4 fish and the oat, with no barley): its water leg is reckoned with
    // that load when it starts back, so it is faster than the way out.
    tick(&rules, &mut state, at(11, 12));
    let expedition = trip_expedition(&state);
    assert_eq!(expedition.phase, Phase::Homeward);
    assert_eq!(expedition.cargo[fish], 4);
    let mut back = Vec::new();
    for step in 0..40 {
        tick(&rules, &mut state, date_of(hours(at(11, 12)) + 1 + step));
        let Some(expedition) = state.places[0].trade.expeditions.first() else {
            break;
        };
        if back.last() != Some(&(expedition.leg, expedition.pace_pm)) {
            back.push((expedition.leg, expedition.pace_pm));
        }
    }
    assert!(state.places[0].trade.expeditions.is_empty(), "it came home");
    let back_water = back
        .iter()
        .find(|(leg, _)| *leg == 0)
        .map(|(_, pace)| *pace)
        .unwrap();
    // The oat was opened at the first noon, so the return carries only the 4 fish (1.8 kg) with the hand cart aboard: 50 + 1.8 = 51.8 kg
    // of the currach's 800: 1000 - floor(250 x 51,800 / 800,000) = 984.
    assert_eq!(back_water, 984);
    assert!(back_water > 974, "the lighter return load is faster");
    validate_trade_state(&rules, &state).unwrap();
}

/// Sets the job's animals as a swap would have, since the fixture has no seller of sheep.
fn give_the_job_its_sheep(rules: &Rules, state: &mut EconomyState, head: u64) {
    let species = rules
        .herd_species
        .iter()
        .position(|s| s.animal == "sheep")
        .unwrap();
    state.places[0].trade.jobs[0].animals[species] = head;
}

#[test]
fn twenty_sheep_start_with_one_crew_and_one_drover_and_twenty_one_never_start_and_the_job_is_given_up()
 {
    for (sheep, starts) in [(20_u64, true), (21, false)] {
        let mut rules = herd_rules();
        let mut state = two_places(&mut rules);
        let mut plan = the_plan(&rules, 6, 5);
        plan.cargo = vec![0; rules.goods.len()];
        plan.cargo[good(&rules, "sheep")] = sheep;
        let made = hours(at(10, 5));
        state.order_trade(&rules, 0, PEASANT, plan, made).unwrap();
        give_the_job_its_sheep(&rules, &mut state, sheep);
        tick(&rules, &mut state, at(10, 5));
        if starts {
            // One person crews the cart at its fewest and the other drives the sheep, who go at 800. The cart carries the 8 oats of
            // supplies (116 kg of its 500): floor(800 x 942 / 1000) = 753, and at its fewest crew floor(753 x 700 / 1000) = 527.
            let expedition = trip_expedition(&state);
            assert_eq!(expedition.pace_pm, 527);
            assert_eq!(
                expedition.limiter,
                Limiter::Vehicle(kind(&rules, "ox_cart"))
            );
            continue;
        }
        // 21 sheep need two drovers and the cart its one crew: three people, and the plan sends two. It keeps gathering, ready, and
        // is given up in time, which lets its vehicle go and sends the animals back to a herd or on offer.
        assert!(state.places[0].trade.expeditions.is_empty());
        assert_eq!(state.places[0].trade.jobs.len(), 1);
        tick(&rules, &mut state, at(10, 6));
        assert!(state.places[0].trade.expeditions.is_empty());
        assert!(!state.places[0].trade.vehicles[0].free);
        run_trade_hour(&rules, &mut state, date_of(made + rules.job_giveup_hours)).unwrap();
        assert!(
            state.places[0].trade.vehicles[0].free,
            "the given-up job lets its vehicle go"
        );
        assert_eq!(
            state.places[0].trade.held(PEASANT),
            0,
            "and no hands were ever held"
        );
        validate_trade_state(&rules, &state).unwrap();
    }
}

#[test]
fn a_swap_is_made_only_up_to_what_still_assigns_on_every_leg_home_and_what_does_not_fit_is_given_away()
 {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let (oats, barley) = (good(&rules, "oats"), good(&rules, "barley"));
    // A barley is worth three oats. The destination's monks have 40 oats to spare and the visitor wants them all, but a hand cart holds
    // 150 kg and an oat weighs 14.5: with the oat of supplies aboard, ten in all is as many as it holds.
    for place in &mut state.places {
        set_price(&rules, place, "barley", 3);
    }
    state.places[1].pops[MONK].stock[oats] += 40;
    let mut plan = the_plan(&rules, 6, 5);
    plan.vehicles = vec![add_vehicle(&mut state, &rules, "hand_cart")];
    plan.people = vec![(u32::try_from(PEASANT).unwrap(), 1)];
    plan.cargo = vec![0; rules.goods.len()];
    plan.cargo[barley] = 5;
    plan.wanted = vec![0; rules.goods.len()];
    plan.wanted[oats] = 9;
    plan.supplies = vec![0; rules.goods.len()];
    plan.supplies[oats] = 1;
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    // The plan's return load fits (it is checked when the job starts); the case then has the trip want all 40, as if the place offered
    // more than the forecast, so the check at the swap is what keeps the way home possible.
    state.places[0].trade.expeditions[0].plan.wanted[oats] = 40;
    // The noon's eating comes after the exchange of the day, so the oat of supplies is aboard at the swap; the first noon's tick runs
    // the exchange and then the eating, which opens the oat into the pantry.
    for hour in 6..=12 {
        tick(&rules, &mut state, at(10, hour));
    }
    let expedition = trip_expedition(&state);
    assert_eq!(expedition.phase, Phase::Homeward);
    // Three barley bought the nine oats that fit beside the supplies' oat (which that noon's eating then opened); a tenth would have
    // made 159.5 kg.
    assert_eq!((expedition.cargo[oats], expedition.supplies[oats]), (9, 0));
    assert_eq!(
        expedition.pantry_centi,
        1800 - 100,
        "the oat was opened for one person's day"
    );
    let mut with_the_oat = expedition.clone();
    with_the_oat.supplies[oats] = 1;
    assert!(with_the_oat.assigns_home(&rules, &with_the_oat.cargo));
    let mut one_more = with_the_oat.cargo.clone();
    one_more[oats] += 1;
    assert!(
        !with_the_oat.assigns_home(&rules, &one_more),
        "a tenth would not assign"
    );
    // The two barley left do not fit beside them, so they are given to the monks, who want barley and have room: nothing is thrown away.
    assert_eq!(expedition.cargo[barley], 0);
    let monks = &state.places[1];
    assert_eq!(
        monks.pops[MONK].stock[barley],
        target(&rules, monks, MONK, "barley") - 10 + 5
    );
    assert_eq!(monks.counters.current.losses, 0);
    assert_eq!(
        monks.pops[MONK].stock[oats],
        target(&rules, monks, MONK, "oats") + 40 - 9
    );
}

#[test]
fn a_visiting_expedition_takes_its_share_of_a_scarce_good_like_any_pop() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let fish = good(&rules, "fish");
    // Ten fish are spare, and the visitor and a pop of the place each want ten and have barley the monks want.
    capacity(&mut rules, &mut state.places[1], THIRD, 1_000_000);
    want(&rules, &mut state.places[1], THIRD, "fish", 10);
    spare(&rules, &mut state.places[1], THIRD, "barley", 10);
    state.places[1].pops[MONK].stock[fish] = target(&rules, &state.places[1], MONK, "fish") + 10;
    let mut plan = the_plan(&rules, 6, 5);
    plan.cargo[good(&rules, "barley")] = 10;
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    for hour in 5..=12 {
        tick(&rules, &mut state, at(10, hour));
    }
    // Their wants are equal, so each takes half of the ten: the visitor five, the pop five. Neither is left out and neither takes more.
    assert_eq!(trip_expedition(&state).cargo[fish], 5);
    assert_eq!(
        state.places[1].pops[THIRD].stock[fish],
        target(&rules, &state.places[1], THIRD, "fish") - 10 + 5
    );
}

#[test]
fn hired_hands_are_paid_when_the_trip_starts_held_while_away_and_counted_as_days_away_at_each_noon()
{
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    // The monks work a farm of no levels at a posted wage of 6,000: one head for one day is worth 201.
    let farm = add_building(&rules, &mut state.places[0], "farm", 0);
    state.places[0].buildings[farm].employment[MONK].workers = 600;
    state.places[0].pops[MONK].employment[farm].workers = 600;
    state.places[0].buildings[farm].wage_history = vec![6000; 12];
    let mut plan = the_plan(&rules, 6, 5);
    plan.people = vec![
        (u32::try_from(PEASANT).unwrap(), 1),
        (u32::try_from(MONK).unwrap(), 1),
    ];
    // A way out of 30 hours, so it is away at two noons: the first on the way, the second at the destination as it starts back.
    plan.out = vec![fixed_leg(Mode::Land, 30)];
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    let expedition = trip_expedition(&state);
    assert_eq!(expedition.groups.len(), 2);
    assert!(
        expedition.groups[0].paid.is_empty(),
        "the trader's own people are not paid"
    );
    let paid = &expedition.groups[1].paid;
    assert_eq!(
        paid.len(),
        1,
        "the wage is paid in one good, once, at the start"
    );
    assert!(
        state.places[0].pops[MONK].stock[paid[0].0 as usize] >= paid[0].1,
        "into the hired pop's store"
    );
    assert_eq!(
        state.places[0].trade.held(MONK),
        1,
        "the hired hand cannot be hired again"
    );
    assert_eq!(
        expedition.groups[1].by_building,
        vec![(u32::try_from(farm).unwrap(), 1)]
    );
    // Each noon the trip is under way counts a day away from the farm for that hand.
    for hour in 6..=23 {
        tick(&rules, &mut state, at(10, hour));
    }
    let away = state.places[0]
        .trade
        .days_away
        .get(&(u32::try_from(farm).unwrap(), u32::try_from(MONK).unwrap()));
    assert_eq!(away, Some(&1), "one noon so far");
    for hour in 0..=16 {
        tick(&rules, &mut state, at(11, hour));
    }
    assert!(state.places[0].trade.expeditions.is_empty());
    let away = state.places[0]
        .trade
        .days_away
        .get(&(u32::try_from(farm).unwrap(), u32::try_from(MONK).unwrap()));
    assert_eq!(
        away,
        Some(&2),
        "and one more at the second noon, which it spent at the destination"
    );
    assert_eq!(state.places[0].trade.held(MONK), 0);
}

/// A trip of the ox-cart and two of the peasant's people that takes one head of cattle to a destination that does not want it and
/// brings it home.
pub(crate) fn cattle_trip(rules: &mut Rules, owns_a_herd: bool) -> EconomyState {
    let mut state = two_places(rules);
    if owns_a_herd {
        // A herd of three of ten is below its standard level of seven.
        cattle_farm(rules, &mut state.places[0], 3, &[(PEASANT, 1)]);
    }
    let mut plan = the_plan(rules, 6, 5);
    plan.cargo = vec![0; rules.goods.len()];
    plan.cargo[good(rules, "cattle")] = 1;
    plan.wanted = vec![0; rules.goods.len()];
    plan.supplies = vec![0; rules.goods.len()];
    plan.supplies[good(rules, "oats")] = 2;
    let made = hours(at(10, 5));
    state.order_trade(rules, 0, PEASANT, plan, made).unwrap();
    state.places[0].trade.jobs[0].animals[0] = 1;
    state
}

pub(crate) fn run_trip(rules: &Rules, state: &mut EconomyState) {
    for hour in 5..24 {
        tick(rules, state, at(10, hour));
    }
}

#[test]
fn an_animal_it_brings_home_goes_to_a_herd_when_the_job_that_wanted_it_has_expired_and_otherwise_stays_on_offer()
 {
    // With a herd below its standard level and an expired job that lacked the animal, the head goes to the herd, not to the dead job.
    let mut rules = herd_rules();
    let mut state = cattle_trip(&mut rules, true);
    let made = hours(at(10, 5));
    state
        .order_build(&rules, 0, PEASANT, "ox_cart", made)
        .unwrap();
    let expired = state.places[0].trade.jobs.len() - 1;
    state.places[0].trade.jobs[expired].given_up = Some(made);
    run_trip(&rules, &mut state);
    assert!(
        state.places[0].trade.expeditions.is_empty(),
        "home and unloaded"
    );
    let herd = state.places[0]
        .buildings
        .iter()
        .position(|b| b.herds[0] > 0)
        .unwrap();
    assert_eq!(
        head(&state.places[0], herd),
        4,
        "the head joined the herd below its standard level"
    );
    assert!(
        state.places[0]
            .trade
            .jobs
            .iter()
            .all(|job| job.animals[0] == 0),
        "and no job, expired or not, holds it"
    );
    assert_eq!(state.places[0].trade.offered(PEASANT, 0), 0);
    validate_trade_state(&rules, &state).unwrap();
    // With no herd to take it and no job that lacks it, it stays with the trader on offer at the place, never lost, and is slaughtered
    // after the set time.
    let mut rules = herd_rules();
    let mut state = cattle_trip(&mut rules, false);
    run_trip(&rules, &mut state);
    assert!(state.places[0].trade.expeditions.is_empty());
    assert_eq!(
        state.places[0].trade.offered(PEASANT, 0),
        1,
        "a head is never lost"
    );
    let on_offer = state.places[0]
        .trade
        .jobs
        .iter()
        .find(|job| matches!(job.kind, JobKind::Held { .. }))
        .unwrap();
    assert_eq!(on_offer.given_up, Some(hours(at(10, 16))));
    validate_trade_state_at(&rules, &state, hours(at(10, 16))).unwrap();
    let slaughter = hours(at(10, 16)) + rules.held_animal_hours;
    run_trade_hour(&rules, &mut state, date_of(slaughter)).unwrap();
    assert_eq!(state.places[0].trade.offered(PEASANT, 0), 0);
    assert_eq!(
        state.places[0].pops[PEASANT].stock[good(&rules, "beef")],
        220
    );
}

#[test]
fn an_expedition_out_of_food_goes_at_half_pace_and_a_vehicle_uses_up_a_unit_of_wear_for_each_30_days_of_travel()
 {
    let mut rules = herd_rules();
    // A wear of one unit for every three hours of travel makes the 30 days of the data short enough to watch.
    rules.expedition.wear_hours = 3;
    let mut state = two_places(&mut rules);
    let hardwood = good(&rules, "hardwood");
    let mut plan = the_plan(&rules, 30, 5);
    plan.supplies = vec![0; rules.goods.len()];
    plan.supplies[hardwood] = 2;
    plan.cargo = vec![0; rules.goods.len()];
    state.places[0].pops[PEASANT].stock[hardwood] += 2;
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    let full_pace = trip_expedition(&state).pace_pm;
    assert_eq!(trip_expedition(&state).supplies[hardwood], 2);
    for (hour, left) in [(6, 2), (7, 2), (8, 1), (9, 1), (10, 1), (11, 0), (12, 0)] {
        tick(&rules, &mut state, at(10, hour));
        assert_eq!(
            trip_expedition(&state).supplies[hardwood],
            left,
            "hour {hour}"
        );
    }
    // No food was taken, so at the first noon the supplies are gone, and its pace is reckoned again with what is aboard then: the
    // wear is used up, so the empty cart at full crew goes at 800, and half of that is 400. With the 100 kg of hardwood it started at 760.
    assert_eq!(full_pace, 760);
    let expedition = trip_expedition(&state);
    assert!(expedition.gone);
    assert_eq!(expedition.pace_pm, 400);
    // Both reachable states validate: the exhausted one, and the start before any noon's eating, with no food aboard and not yet gone.
    validate_trade_state_at(&rules, &state, hours(at(10, 12)) + 1).unwrap();
    let mut fresh = two_places(&mut rules);
    let mut bare = the_plan(&rules, 30, 5);
    bare.supplies = vec![0; rules.goods.len()];
    bare.cargo = vec![0; rules.goods.len()];
    fresh
        .order_trade(&rules, 0, PEASANT, bare, hours(at(10, 5)))
        .unwrap();
    tick(&rules, &mut fresh, at(10, 5));
    assert!(!fresh.places[0].trade.expeditions[0].gone);
    validate_trade_state_at(&rules, &fresh, hours(at(10, 5)) + 1).unwrap();
}

type Edit = Box<dyn Fn(&mut EconomyState)>;

/// A state in the middle of a trip, and the edits a reachable game never makes to it.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "One line for each edit a reachable game never makes, so each is visible beside the others."
)]
fn a_saved_expedition_that_no_reachable_game_could_hold_is_refused_and_a_reachable_one_is_not() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    state
        .order_trade(
            &rules,
            0,
            PEASANT,
            the_plan(&rules, 30, 5),
            hours(at(10, 5)),
        )
        .unwrap();
    for hour in 5..=9 {
        tick(&rules, &mut state, at(10, hour));
    }
    validate_trade_state(&rules, &state).unwrap();
    let oats = good(&rules, "oats");
    let edits: Vec<(&str, Edit)> = vec![
        (
            "a leg past the end",
            Box::new(|s| s.places[0].trade.expeditions[0].leg = 2),
        ),
        (
            "a whole leg done",
            Box::new(|s| s.places[0].trade.expeditions[0].done = ratio(1, 1)),
        ),
        (
            "waiting before it arrived",
            Box::new(|s| s.places[0].trade.expeditions[0].phase = Phase::Waiting),
        ),
        (
            "homeward untraded",
            Box::new(|s| s.places[0].trade.expeditions[0].phase = Phase::Homeward),
        ),
        (
            "an arrival on the way out",
            Box::new(|s| s.places[0].trade.expeditions[0].arrived = Some(1)),
        ),
        (
            "no pace",
            Box::new(|s| s.places[0].trade.expeditions[0].pace_pm = 0),
        ),
        (
            "more people than the plan",
            Box::new(|s| s.places[0].trade.expeditions[0].groups[0].count = 3),
        ),
        (
            "no people",
            Box::new(|s| s.places[0].trade.expeditions[0].plan.people.clear()),
        ),
        (
            "a crowd",
            Box::new(|s| {
                s.places[0].trade.expeditions[0].plan.people[0].1 = 5000;
                s.places[0].trade.expeditions[0].groups[0].count = 5000;
            }),
        ),
        (
            "the home as its destination",
            Box::new(|s| s.places[0].trade.expeditions[0].plan.destination = 0),
        ),
        (
            "a destination outside the economy",
            Box::new(|s| s.places[0].trade.expeditions[0].plan.destination = 9),
        ),
        (
            "a vehicle not its plan's",
            Box::new(|s| s.places[0].trade.expeditions[0].vehicles[0].number = 7),
        ),
        (
            "a vehicle free while away",
            Box::new(|s| s.places[0].trade.vehicles[0].free = true),
        ),
        (
            "a number not issued",
            Box::new(|s| s.places[0].trade.expeditions[0].number = 9),
        ),
        (
            "a number never issued",
            Box::new(|s| s.trade.expeditions = 0),
        ),
        (
            "goods that do not match the rules",
            Box::new(|s| {
                s.places[0].trade.expeditions[0].cargo.pop();
            }),
        ),
        (
            "more supplies than it took",
            Box::new(move |s| s.places[0].trade.expeditions[0].supplies[oats] = 99),
        ),
        (
            "cargo changed on the way out",
            Box::new(move |s| s.places[0].trade.expeditions[0].cargo[oats] = 1),
        ),
        (
            "a wage for the trader's own people",
            Box::new(|s| s.places[0].trade.expeditions[0].groups[0].paid = vec![(0, 1)]),
        ),
        (
            "a building that does not exist",
            Box::new(|s| s.places[0].trade.expeditions[0].groups[0].by_building = vec![(99, 1)]),
        ),
        (
            "a pantry larger than a day's food",
            Box::new(|s| s.places[0].trade.expeditions[0].pantry_centi = 1_000_000),
        ),
        (
            "a pace limiter that does not exist",
            Box::new(|s| s.places[0].trade.expeditions[0].limiter = Limiter::Species(99)),
        ),
        (
            "a worn-out vehicle",
            Box::new(|s| {
                s.places[0].trade.expeditions[0].vehicles[0].worn_hours = ratio(10_000, 1);
            }),
        ),
    ];
    for (name, edit) in &edits {
        let mut bad = state.clone();
        edit(&mut bad);
        assert!(
            validate_trade_state(&rules, &bad).is_err(),
            "{name} is accepted"
        );
    }
    // A held job with something to need, and a trade job that needs what its plan does not take, are refused too.
    let mut bad = state.clone();
    bad.trade.jobs = 1;
    bad.places[0].trade.jobs.push(Job {
        number: 1,
        pop: 0,
        kind: JobKind::Held {
            origin: vec![0; rules.herd_species.len()],
        },
        made: 5,
        need_goods: vec![1; rules.goods.len()],
        need_animals: vec![0; rules.herd_species.len()],
        stock: vec![0; rules.goods.len()],
        animals: vec![0; rules.herd_species.len()],
        hands: None,
        given_up: Some(5),
    });
    assert!(validate_trade_state(&rules, &bad).is_err());
    let mut jobbed = two_places(&mut rules);
    jobbed
        .order_trade(&rules, 0, PEASANT, the_plan(&rules, 30, 5), 100)
        .unwrap();
    validate_trade_state(&rules, &jobbed).unwrap();
    jobbed.places[0].trade.jobs[0].need_goods[oats] += 1;
    assert!(validate_trade_state(&rules, &jobbed).is_err());
}

// ---- Review 1 (CK-432): the probes of the reviewer and the cases the findings call for ----

#[test]
fn a_plan_whose_cargo_and_supplies_overflow_together_is_declined_before_anything_changes() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let oats = good(&rules, "oats");
    let mut plan = the_plan(&rules, 30, 5);
    plan.cargo.fill(0);
    plan.supplies.fill(0);
    plan.cargo[oats] = u64::MAX;
    plan.supplies[oats] = 1;
    let before = state.clone();
    assert!(state.order_trade(&rules, 0, PEASANT, plan, 100).is_err());
    assert_eq!(state, before);
}

#[test]
fn a_decoded_trade_job_whose_cargo_and_supplies_overflow_together_is_refused_not_a_panic() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let oats = good(&rules, "oats");
    state
        .order_trade(&rules, 0, PEASANT, the_plan(&rules, 30, 5), 100)
        .unwrap();
    let JobKind::Trade { plan } = &mut state.places[0].trade.jobs[0].kind else {
        unreachable!()
    };
    plan.cargo.fill(0);
    plan.supplies.fill(0);
    plan.cargo[oats] = u64::MAX;
    plan.supplies[oats] = 1;
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    assert!(validate_trade_state(&rules, &decoded).is_err());
}

#[test]
fn driven_animals_and_stock_counts_are_counted_with_checked_arithmetic() {
    let mut rules = herd_rules();
    let state = two_places(&mut rules);
    let sheep = good(&rules, "sheep");
    let mut cargo = vec![0; rules.goods.len()];
    cargo[sheep] = u64::MAX;
    let none = vec![0; rules.goods.len()];
    // Two kinds of animal whose heads add past 64 bits are refused, not wrapped.
    let mut both = cargo.clone();
    both[good(&rules, "pigs")] = 2;
    assert!(assign_leg(&rules, &[], 5, &both, &none, Mode::Land).is_none());
    // A given-up trade job holds no more than it gathered and the slaughter of what it held could have added; a held job only what a
    // slaughtered head yields.
    let mut bad = state.clone();
    bad.order_trade(&rules, 0, PEASANT, the_plan(&rules, 30, 5), 100)
        .unwrap();
    bad.places[0].trade.jobs[0].given_up = Some(200);
    bad.places[0].trade.vehicles[0].free = true;
    validate_trade_state(&rules, &bad).unwrap();
    bad.places[0].trade.jobs[0].stock[good(&rules, "hides")] = 5;
    assert!(
        validate_trade_state(&rules, &bad).is_err(),
        "stock that nothing gathered or slaughtered"
    );
}

#[test]
fn a_held_job_after_slaughter_with_a_full_store_keeps_its_yield_saves_and_reloads_and_the_yield_reaches_the_trader_once()
 {
    let mut rules = herd_rules();
    let mut state = cattle_trip(&mut rules, false);
    run_trip(&rules, &mut state);
    let beef = good(&rules, "beef");
    let people = state.places[0].pops[PEASANT].workers + state.places[0].pops[PEASANT].dependents;
    let (cap, _) = pop_store_limits(&rules, people, beef).unwrap();
    state.places[0].pops[PEASANT].stock[beef] = cap;
    let slaughter = hours(at(10, 16)) + rules.held_animal_hours;
    run_trade_hour(&rules, &mut state, date_of(slaughter)).unwrap();
    let held = &state.places[0].trade.jobs[0];
    assert!(matches!(held.kind, JobKind::Held { .. }));
    assert_eq!(
        (held.stock[beef], held.animals.iter().sum::<u64>()),
        (220, 0)
    );
    // The pending-yield state saves and reloads.
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    validate_trade_state(&rules, &decoded).unwrap();
    // Stock a slaughtered head could not have yielded is invented.
    let mut invented = state.clone();
    invented.places[0].trade.jobs[0].stock[good(&rules, "oats")] = 1;
    assert!(validate_trade_state(&rules, &invented).is_err());
    // Freeing room lets the yield reach the trader once, and the held job then ends.
    state.places[0].pops[PEASANT].stock[beef] = 0;
    run_trade_hour(&rules, &mut state, date_of(slaughter + 1)).unwrap();
    assert_eq!(state.places[0].pops[PEASANT].stock[beef], 220, "it arrived");
    assert!(
        state.places[0].trade.jobs.is_empty(),
        "and the held job ended"
    );
    run_trade_hour(&rules, &mut state, date_of(slaughter + 2)).unwrap();
    assert_eq!(state.places[0].pops[PEASANT].stock[beef], 220, "once");
}

#[test]
fn a_visitor_and_a_resident_claim_one_scarce_good_together_when_the_world_has_neighbours() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    capacity(&mut rules, &mut state.places[1], THIRD, 1_000_000);
    want(&rules, &mut state.places[1], THIRD, "fish", 20);
    spare(&rules, &mut state.places[1], THIRD, "barley", 10);
    state
        .order_trade(&rules, 0, PEASANT, the_plan(&rules, 6, 5), hours(at(10, 5)))
        .unwrap();
    for hour in 5..=11 {
        tick(&rules, &mut state, at(10, hour));
    }
    let mut third = state.places[0].clone();
    third.id = "third".to_owned();
    third.trade = PlaceTrade::default();
    let mut fourth = third.clone();
    fourth.id = "fourth".to_owned();
    state.places.extend([third, fourth]);
    let neighbours = BTreeMap::from([(
        "third".to_owned(),
        vec![NearbyPlace {
            id: "fourth".to_owned(),
            distance_m: 3000,
        }],
    )]);
    let prices: Vec<_> = state
        .places
        .iter()
        .map(|place| place.prices_milli.clone())
        .collect();
    let fish = good(&rules, "fish");
    let mut apart = state.clone();
    swap::run_noon_exchange(
        &rules,
        &mut apart,
        &prices,
        &BTreeMap::new(),
        hours(at(10, 12)),
    )
    .unwrap();
    let expected = trip_expedition(&apart).cargo[fish];
    assert!(
        expected > 0 && expected < 10,
        "the ten fish are shared: {expected}"
    );
    // An unrelated neighbouring pair changes nothing for the visitor and nothing panics.
    swap::run_noon_exchange(&rules, &mut state, &prices, &neighbours, hours(at(10, 12))).unwrap();
    assert_eq!(trip_expedition(&state).cargo[fish], expected);
    // Neighbours of the destination itself, with the visitor and a resident claiming together.
    let mut beside = apart.clone();
    let near = BTreeMap::from([(
        "dest".to_owned(),
        vec![NearbyPlace {
            id: "fourth".to_owned(),
            distance_m: 3000,
        }],
    )]);
    let mut extra = beside.places[0].clone();
    extra.id = "fourth".to_owned();
    extra.trade = PlaceTrade::default();
    beside.places.push(extra);
    let prices: Vec<_> = beside
        .places
        .iter()
        .map(|place| place.prices_milli.clone())
        .collect();
    let _ = &near;
    swap::run_noon_exchange(&rules, &mut beside, &prices, &near, hours(at(10, 12))).unwrap();
}

#[test]
fn a_pace_the_simulation_cannot_produce_and_a_position_it_cannot_reach_are_refused() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    state
        .order_trade(
            &rules,
            0,
            PEASANT,
            the_plan(&rules, 30, 5),
            hours(at(10, 5)),
        )
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    validate_trade_state(&rules, &state).unwrap();
    let mut corrupt = state.clone();
    corrupt.places[0].trade.expeditions[0].pace_pm = u32::MAX;
    let mut bytes = Vec::new();
    corrupt.encode(&mut bytes).unwrap();
    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    assert!(validate_trade_state(&rules, &decoded).is_err());
    // Slower than the full load gives, and a limiter it does not have.
    let mut slow = state.clone();
    slow.places[0].trade.expeditions[0].pace_pm = 1;
    assert!(validate_trade_state(&rules, &slow).is_err());
    let mut limiter = state.clone();
    limiter.places[0].trade.expeditions[0].limiter = Limiter::Vehicle(kind(&rules, "currach"));
    assert!(validate_trade_state(&rules, &limiter).is_err());
    // Standing at the end of a way that still has legs, while moving.
    let mut stuck = state.clone();
    stuck.places[0].trade.expeditions[0].leg = 1;
    assert!(validate_trade_state(&rules, &stuck).is_err());
    // A trip that began after the clock the state was saved at, and an arrival before the departure.
    let saved = hours(at(10, 5));
    validate_trade_state_at(&rules, &state, saved).unwrap();
    assert!(validate_trade_state_at(&rules, &state, saved - 1).is_err());
    // A pace the load could have given stays valid through the next noon and as the supplies fall.
    for hour in 6..=14 {
        tick(&rules, &mut state, at(10, hour));
        validate_trade_state_at(&rules, &state, hours(at(10, hour)) + 1).unwrap();
    }
}

#[test]
fn workers_who_return_at_noon_and_leave_again_in_the_same_hour_are_charged_one_noon() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let farm = add_building(&rules, &mut state.places[0], "farm", 0);
    let home = &mut state.places[0];
    home.pops[PEASANT].workers = 2;
    home.pops[PEASANT].employment[farm].workers = 2;
    home.buildings[farm].employment[PEASANT].workers = 2;
    let second_cart = add_vehicle(&mut state, &rules, "ox_cart");
    let mut next = the_plan(&rules, 6, 1);
    next.vehicles = vec![second_cart];
    next.cargo.fill(0);
    next.supplies.fill(0);
    next.wanted.fill(0);
    state
        .order_trade(&rules, 0, PEASANT, the_plan(&rules, 6, 1), hours(at(10, 5)))
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    state
        .order_trade(&rules, 0, PEASANT, next, hours(at(10, 6)))
        .unwrap();
    for hour in 6..=12 {
        tick(&rules, &mut state, at(10, hour));
    }
    assert_eq!(trip_expedition(&state).number, 2);
    assert_eq!(trip_expedition(&state).left, hours(at(10, 12)));
    let key = (
        u32::try_from(farm).unwrap(),
        u32::try_from(PEASANT).unwrap(),
    );
    assert_eq!(
        state.places[0].trade.days_away[&key], 2,
        "two people, one noon each"
    );
}

#[test]
fn workers_away_on_an_expedition_do_not_die_or_move_at_the_monthly_step() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let kind_id = state.places[0].pops[PEASANT].kind.clone();
    rules
        .pop_types
        .iter_mut()
        .find(|pop| pop.id == kind_id)
        .unwrap()
        .worker_deaths_pm_year = 60_000;
    state.places[0].pops[PEASANT].workers = 2;
    state
        .order_trade(
            &rules,
            0,
            PEASANT,
            the_plan(&rules, 30, 5),
            hours(at(10, 5)),
        )
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    assert_eq!(state.places[0].trade.away(PEASANT), 2);
    run_monthly_demography(&rules, &mut state, &BTreeMap::new(), &BTreeMap::new(), None).unwrap();
    assert_eq!(
        state.places[0].pops[PEASANT].workers, 2,
        "the away workers survive the month"
    );
    assert_eq!(state.places[0].trade.away(PEASANT), 2);
}

#[test]
fn a_boat_only_plan_with_an_interior_delay_only_transfer_starts_and_a_leg_path_is_bounded_and_saved()
 {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let barge = add_vehicle(&mut state, &rules, "simple_barge");
    let water = |hours: u64| fixed_leg(Mode::Water, hours);
    // A transfer with no ground approach beside water legs is on the water, as the route builder makes it, so a barge that cannot be
    // carried needs no land carriers for it.
    let mut plan = the_plan(&rules, 1, 1);
    plan.vehicles = vec![barge];
    plan.people = vec![(u32::try_from(PEASANT).unwrap(), 3)];
    plan.out = vec![water(2), water(1), water(2)];
    plan.back = vec![water(2), water(1), water(2)];
    plan.cargo = vec![0; rules.goods.len()];
    plan.cargo[good(&rules, "barley")] = 2;
    plan.supplies = vec![0; rules.goods.len()];
    plan.supplies[good(&rules, "oats")] = 1;
    plan.wanted = vec![0; rules.goods.len()];
    state
        .order_trade(&rules, 0, PEASANT, plan.clone(), hours(at(10, 5)))
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    assert_eq!(
        trip_expedition(&state).phase,
        Phase::Outbound,
        "the boat-only plan started"
    );
    // The same plan with that transfer on land cannot be made: the barge is not carried across ground.
    let mut rules2 = herd_rules();
    let mut state2 = two_places(&mut rules2);
    let barge2 = add_vehicle(&mut state2, &rules2, "simple_barge");
    plan.vehicles = vec![barge2];
    plan.out[1] = fixed_leg(Mode::Land, 1);
    state2
        .order_trade(&rules2, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    tick(&rules2, &mut state2, at(10, 5));
    assert!(
        state2.places[0].trade.expeditions.is_empty(),
        "a land transfer needs a land carrier"
    );

    // A leg's frozen path is saved with it, interpolated by fraction of the way, and bounded.
    let leg = Leg {
        mode: Mode::Land,
        terms: vec![Term {
            work: ratio(3, 1),
            speed: Speed::Plain(3),
        }],
        fixed: ratio(1, 1),
        delays: Delays::Along,
        key: [0; 4],
        along: Vec::new(),
        path: vec![[0, 0], [10, 20], [30, 20]],
    };
    assert_eq!(leg.position(&ratio(0, 1)), Some((ratio(0, 1), ratio(0, 1))));
    assert_eq!(
        leg.position(&ratio(1, 4)),
        Some((ratio(5, 1), ratio(10, 1)))
    );
    assert_eq!(
        leg.position(&ratio(3, 4)),
        Some((ratio(20, 1), ratio(20, 1)))
    );
    assert_eq!(
        leg.position(&ratio(1, 1)),
        Some((ratio(30, 1), ratio(20, 1)))
    );
    assert_eq!(fixed_leg(Mode::Land, 1).position(&ratio(1, 2)), None);
    let mut bytes = Vec::new();
    leg.encode(&mut bytes).unwrap();
    assert_eq!(Leg::decode(&mut Reader::new(&bytes)).unwrap(), leg);
    let mut too_long = the_plan(&rules, 1, 1);
    too_long.out[0].path = vec![[0, 0]; MOST_LEG_PATH + 1];
    let mut fresh = two_places(&mut rules);
    assert!(fresh.order_trade(&rules, 0, PEASANT, too_long, 1).is_err());
}

// ---- Review 2 (CK-432): the reviewer's probes ----
#[test]
fn a_homeward_save_after_the_supplies_were_eaten_before_the_exchange_reloads() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let (barley, fish, oats) = (
        good(&rules, "barley"),
        good(&rules, "fish"),
        good(&rules, "oats"),
    );
    rules.goods[barley].weight_g = 33_000;
    rules.goods[fish].weight_g = 200_000;
    rules.expedition.daily_hunger_centi = u64::from(rules.goods[oats].hunger_centi) * 4;
    let mut plan = the_plan(&rules, 7, 5);
    plan.wanted[fish] = 1;
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    for hour in 5..24 {
        tick(&rules, &mut state, at(10, hour));
    }
    assert_eq!(trip_expedition(&state).phase, Phase::Waiting);
    assert_eq!(trip_expedition(&state).supplies[oats], 0);
    for hour in 0..=12 {
        tick(&rules, &mut state, at(11, hour));
    }
    let trip = trip_expedition(&state);
    assert_eq!(trip.phase, Phase::Homeward);
    assert_eq!(
        (trip.cargo[barley], trip.cargo[fish], trip.supplies[oats]),
        (9, 1, 0)
    );
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    validate_trade_state_at(&rules, &decoded, hours(at(11, 12)) + 1).unwrap();
}

#[test]
fn a_held_job_cannot_reload_more_yield_than_its_heads_could_make() {
    let mut rules = herd_rules();
    let mut state = cattle_trip(&mut rules, false);
    run_trip(&rules, &mut state);
    let beef = good(&rules, "beef");
    let home = &mut state.places[0];
    let people = home.pops[PEASANT].workers + home.pops[PEASANT].dependents;
    let (cap, _) = pop_store_limits(&rules, people, beef).unwrap();
    home.pops[PEASANT].stock[beef] = cap;
    let now = hours(at(10, 16)) + rules.held_animal_hours;
    run_trade_hour(&rules, &mut state, date_of(now)).unwrap();
    assert_eq!(state.places[0].trade.jobs[0].stock[beef], 220);
    // The clock after the hour's systems ran is one hour on (review 3).
    validate_trade_state_at(&rules, &state, now + 1).unwrap();
    state.places[0].trade.jobs[0].stock[beef] = u64::MAX;
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    assert!(validate_trade_state_at(&rules, &decoded, now + 1).is_err());
}

#[test]
fn first_fit_finds_a_valid_run_far_below_the_cap_by_the_exhaustive_oracle() {
    // Carrier order, with each room in (space, weight). Only the first good varies.
    let mut rules = herd_rules();
    let fleet = vehicles(&rules, &["hand_cart", "ox_cart", "covered_wagon"]);
    for (vehicle, (space, weight)) in fleet.iter().zip([(300, 1300), (1000, 1400), (200, 1800)]) {
        let kind = &mut rules.vehicles[vehicle.kind as usize];
        kind.carries_ml = space;
        kind.carries_g = weight;
        kind.water = Some("barge".to_owned());
    }
    // The variable animal is indivisible cargo on water and is assigned before the goods.
    rules
        .expedition
        .animals
        .iter_mut()
        .find(|row| row.animal == "cattle")
        .unwrap()
        .aboard_ml = 2;
    let ids = ["cattle", "oats", "fish", "barley"];
    let indexes = ids.map(|id| good(&rules, id));
    for (index, (space, weight)) in
        indexes
            .into_iter()
            .zip([(2, 5), (100, 900), (100, 500), (100, 100)])
    {
        rules.goods[index].volume_ml = space;
        rules.goods[index].weight_g = weight;
    }
    let mut cargo = vec![0; rules.goods.len()];
    cargo[indexes[1]] = 2;
    cargo[indexes[2]] = 1;
    cargo[indexes[3]] = 3;
    let supplies = vec![0; rules.goods.len()];
    let fits = |amount: u128| {
        let mut trial = cargo.clone();
        trial[indexes[0]] = u64::try_from(amount).unwrap();
        assign_leg(&rules, &fleet, 3, &trial, &supplies, Mode::Water).is_some()
    };
    let exhaustive = (0..=335).rev().find(|amount| fits(*amount)).unwrap();
    assert_eq!(exhaustive, 270);
    assert_eq!(swap::max_fitting(335, &fits), exhaustive);
}

#[test]
fn a_pace_one_unit_above_the_dispatch_load_gives_is_refused() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let now = hours(at(10, 5));
    state
        .order_trade(&rules, 0, PEASANT, the_plan(&rules, 30, 5), now)
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    validate_trade_state_at(&rules, &state, now).unwrap();
    let expedition = &mut state.places[0].trade.expeditions[0];
    assert_eq!(expedition.left, now);
    assert_eq!(expedition.supplies, expedition.plan.supplies);
    assert!(expedition.pace_pm < 799);
    expedition.pace_pm += 1;
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    assert!(validate_trade_state_at(&rules, &decoded, now).is_err());
}

#[test]
fn a_visitors_cargo_does_not_make_a_residents_stock_abundant_for_neighbours() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let fish = good(&rules, "fish");
    state.places[0].pops[PEASANT].stock[fish] += 10;
    state.places[1].pops[MONK].stock[fish] = target(&rules, &state.places[1], MONK, "fish") + 1;
    let mut plan = the_plan(&rules, 6, 5);
    plan.cargo.fill(0);
    plan.cargo[fish] = 10;
    plan.wanted.fill(0);
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    for hour in 5..=11 {
        tick(&rules, &mut state, at(10, hour));
    }
    let mut a = place(&rules, 1);
    a.id = "a".to_owned();
    settle_the_rest(&rules, &mut a, &[]);
    capacity(&mut rules, &mut a, 0, 1_000_000);
    set_price(&rules, &mut a, "barley", 1);
    set_price(&rules, &mut a, "fish", 1);
    want(&rules, &mut a, 0, "fish", 1);
    spare(&rules, &mut a, 0, "barley", 1);
    let mut b = a.clone();
    b.id = "b".to_owned();
    b.pops[0].stock[good(&rules, "hardwood")] += 100;
    let before_a = a.pops[0].stock[fish];
    let before_b = b.pops[0].stock[fish];
    state.places.extend([a, b]);
    let neighbours = BTreeMap::from([(
        "dest".to_owned(),
        vec![
            NearbyPlace {
                id: "a".to_owned(),
                distance_m: 3000,
            },
            NearbyPlace {
                id: "b".to_owned(),
                distance_m: 3000,
            },
        ],
    )]);
    let prices: Vec<_> = state
        .places
        .iter()
        .map(|p| p.prices_milli.clone())
        .collect();
    let mut baseline = state.clone();
    baseline.places[0].trade.expeditions.clear();
    let noon = hours(at(10, 12));
    swap::run_noon_exchange(&rules, &mut baseline, &prices, &neighbours, noon).unwrap();
    assert_eq!(baseline.places[2].pops[0].stock[fish], before_a);
    assert_eq!(baseline.places[3].pops[0].stock[fish], before_b + 1);
    swap::run_noon_exchange(&rules, &mut state, &prices, &neighbours, noon).unwrap();
    assert_eq!(
        (
            state.places[2].pops[0].stock[fish],
            state.places[3].pops[0].stock[fish]
        ),
        (before_a, before_b + 1),
    );
}

#[test]
fn hired_hands_are_paid_for_both_days_of_a_thirty_five_hour_trip() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let farm = add_building(&rules, &mut state.places[0], "farm", 0);
    state.places[0].buildings[farm].employment[MONK].workers = 600;
    state.places[0].pops[MONK].employment[farm].workers = 600;
    state.places[0].buildings[farm].wage_history = vec![6000; 12];
    state.places[0].prices_milli.fill(100);
    let mut plan = the_plan(&rules, 30, 5);
    plan.people = vec![
        (u32::try_from(PEASANT).unwrap(), 1),
        (u32::try_from(MONK).unwrap(), 1),
    ];
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    assert_eq!(trade::pop_wage(&state.places[0], MONK), 6000);
    tick(&rules, &mut state, at(10, 5));
    let paid = &trip_expedition(&state).groups[1].paid;
    assert_eq!(paid.len(), 1);
    // Two days of a 6,000 monthly wage plus the minimum increment is 401.
    assert_eq!(paid[0].1, 5);
}

#[test]
fn a_held_jobs_states_are_checked_against_the_saved_clock() {
    let mut rules = herd_rules();
    let mut state = cattle_trip(&mut rules, false);
    run_trip(&rules, &mut state);
    let since = hours(at(10, 16));
    let slaughter = since + rules.held_animal_hours;
    // The clock of a saved state is the hour whose systems have not run: the head is still on offer at a clock of the slaughter hour
    // (its work is still to come) and not after it.
    validate_trade_state_at(&rules, &state, slaughter - 1).unwrap();
    validate_trade_state_at(&rules, &state, slaughter).unwrap();
    assert!(validate_trade_state_at(&rules, &state, slaughter + 1).is_err());
    // After the slaughter has run (a clock one hour on) the pending yield is valid and not before.
    let beef = good(&rules, "beef");
    let people = state.places[0].pops[PEASANT].workers + state.places[0].pops[PEASANT].dependents;
    let (cap, _) = pop_store_limits(&rules, people, beef).unwrap();
    state.places[0].pops[PEASANT].stock[beef] = cap;
    run_trade_hour(&rules, &mut state, date_of(slaughter)).unwrap();
    validate_trade_state_at(&rules, &state, slaughter + 1).unwrap();
    assert!(validate_trade_state_at(&rules, &state, slaughter).is_err());
}

// ---- Review 3 (CK-432): the reviewer's probes ----
#[test]
fn unsold_livestock_can_walk_home_with_an_expedition_on_foot() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let cattle = good(&rules, "cattle");
    let species = species_of(&rules, cattle).unwrap();
    let mut plan = the_plan(&rules, 6, 5);
    plan.vehicles.clear();
    plan.people = vec![(u32::try_from(PEASANT).unwrap(), 1)];
    plan.cargo.fill(0);
    plan.wanted.fill(0);
    plan.supplies.fill(0);
    plan.cargo[cattle] = 1;
    let made = hours(at(10, 5));
    state.order_trade(&rules, 0, PEASANT, plan, made).unwrap();
    // A local swap delivers the head to its reserved job, as in the shipped cattle-trip fixture.
    state.places[0].trade.jobs[0].animals[species] = 1;
    for hour in 5..=11 {
        tick(&rules, &mut state, at(10, hour));
    }
    let visitor = trip_expedition(&state);
    assert_eq!(visitor.phase, Phase::Waiting);
    assert_eq!(visitor.cargo[cattle], 1);
    assert!(visitor.assigns_home(&rules, &visitor.cargo));
    tick(&rules, &mut state, at(10, 12));
    assert_eq!(trip_expedition(&state).phase, Phase::Homeward);
    assert_eq!(trip_expedition(&state).cargo[cattle], 1);
    for hour in 13..=16 {
        tick(&rules, &mut state, at(10, hour));
    }
    assert_eq!(state.places[0].trade.offered(PEASANT, species), 1);
}

#[test]
fn a_noon_departure_that_reaches_the_destination_at_eleven_pays_no_extra_wait() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let farm = add_building(&rules, &mut state.places[0], "farm", 0);
    state.places[0].buildings[farm].employment[MONK].workers = 600;
    state.places[0].pops[MONK].employment[farm].workers = 600;
    state.places[0].buildings[farm].wage_history = vec![6000; 12];
    state.places[0].prices_milli.fill(100);
    let mut plan = the_plan(&rules, 24, 1);
    plan.people = vec![
        (u32::try_from(PEASANT).unwrap(), 1),
        (u32::try_from(MONK).unwrap(), 1),
    ];
    let start = hours(at(10, 12));
    state.order_trade(&rules, 0, PEASANT, plan, start).unwrap();
    tick(&rules, &mut state, at(10, 12));
    let paid = trip_expedition(&state).groups[1].paid[0].1;
    for hour in (start + 1)..=(start + 23) {
        tick(&rules, &mut state, date_of(hour));
    }
    assert_eq!(trip_expedition(&state).arrived, Some(hours(at(11, 11))));
    tick(&rules, &mut state, at(11, 12));
    assert!(state.places[0].trade.expeditions.is_empty());
    assert!(
        paid <= 5,
        "at most two days of wages are due, but {paid} units were paid"
    );
}

#[test]
fn an_on_offer_head_survives_loading_at_the_clock_of_its_due_hour() {
    use ck_sim::{Cadence, Scheduler, Simulation};

    let mut rules = herd_rules();
    let mut state = cattle_trip(&mut rules, false);
    run_trip(&rules, &mut state);
    let due = hours(at(10, 16)) + rules.held_animal_hours;
    let mut scheduler = Scheduler::new();
    let rules_for_tick = rules.clone();
    scheduler
        .register("trade", Cadence::Hourly, move |state, tick| {
            run_trade_hour(&rules_for_tick, state, tick.date()).map_err(|error| {
                Box::new(std::io::Error::other(error)) as Box<dyn Error + Send + Sync>
            })
        })
        .unwrap();
    let mut game = Simulation::new(
        GameTime::from_hours(due - 1),
        0,
        state,
        scheduler,
        |_, _: u8| {},
    );
    assert!(game.step().unwrap().is_empty());
    assert_eq!(game.time().hours(), due);
    assert_eq!(game.state().places[0].trade.offered(PEASANT, 0), 1);
    let mut bytes = Vec::new();
    game.state().encode(&mut bytes).unwrap();
    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    // This is the same boundary clock both application load paths pass to trade validation.
    validate_trade_state_at(&rules, &decoded, game.time().hours()).unwrap();
}

#[test]
fn stock_a_visitor_will_not_swap_cannot_cancel_scarcity_for_a_local_claimant() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let fish = good(&rules, "fish");
    state.places[0].pops[PEASANT].stock[fish] += 10;
    state.places[1].pops[MONK].stock[fish] = target(&rules, &state.places[1], MONK, "fish") + 1;
    capacity(&mut rules, &mut state.places[1], THIRD, 1_000_000);
    want(&rules, &mut state.places[1], THIRD, "fish", 1);
    spare(&rules, &mut state.places[1], THIRD, "barley", 1);
    let mut plan = the_plan(&rules, 6, 5);
    plan.cargo.fill(0);
    plan.cargo[fish] = 10;
    plan.wanted.fill(0);
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    for hour in 5..=11 {
        tick(&rules, &mut state, at(10, hour));
    }
    let mut neighbour = place(&rules, 1);
    neighbour.id = "neighbour".to_owned();
    settle_the_rest(&rules, &mut neighbour, &[]);
    capacity(&mut rules, &mut neighbour, 0, 1_000_000);
    set_price(&rules, &mut neighbour, "barley", 1);
    set_price(&rules, &mut neighbour, "fish", 1);
    want(&rules, &mut neighbour, 0, "fish", 1);
    spare(&rules, &mut neighbour, 0, "barley", 1);
    neighbour.pops[0].stock[good(&rules, "hardwood")] += 100_000;
    let local_before = state.places[1].pops[THIRD].stock[fish];
    let near_before = neighbour.pops[0].stock[fish];
    state.places.push(neighbour);
    let neighbours = BTreeMap::from([(
        "dest".to_owned(),
        vec![NearbyPlace {
            id: "neighbour".to_owned(),
            distance_m: 3000,
        }],
    )]);
    let prices: Vec<_> = state
        .places
        .iter()
        .map(|p| p.prices_milli.clone())
        .collect();
    let mut baseline = state.clone();
    baseline.places[0].trade.expeditions.clear();
    let noon = hours(at(10, 12));
    swap::run_noon_exchange(&rules, &mut baseline, &prices, &neighbours, noon).unwrap();
    assert_eq!(
        (
            baseline.places[1].pops[THIRD].stock[fish],
            baseline.places[2].pops[0].stock[fish]
        ),
        (local_before, near_before + 1),
    );
    swap::run_noon_exchange(&rules, &mut state, &prices, &neighbours, noon).unwrap();
    assert_eq!(
        (
            state.places[1].pops[THIRD].stock[fish],
            state.places[2].pops[0].stock[fish]
        ),
        (local_before, near_before + 1),
    );
}

#[test]
fn a_trade_plan_past_the_calendar_is_declined_without_a_tick_panic() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let date = at(10, 5);
    let now = hours(date);
    let mut plan = the_plan(&rules, 6, 5);
    plan.out[0].fixed = ratio(u128::MAX - u128::from(now), 1);
    if state.order_trade(&rules, 0, PEASANT, plan, now).is_ok() {
        let after_order = state.clone();
        assert!(run_trade_hour(&rules, &mut state, date).is_err());
        assert_eq!(state, after_order);
    }
}

#[test]
fn a_pending_yield_saved_after_the_slaughter_hour_ran_reloads_and_the_state_before_it_still_holds_the_head()
 {
    use ck_sim::{Cadence, Scheduler, Simulation};

    let mut rules = herd_rules();
    let mut state = cattle_trip(&mut rules, false);
    run_trip(&rules, &mut state);
    let beef = good(&rules, "beef");
    let people = state.places[0].pops[PEASANT].workers + state.places[0].pops[PEASANT].dependents;
    let (cap, _) = pop_store_limits(&rules, people, beef).unwrap();
    state.places[0].pops[PEASANT].stock[beef] = cap;
    let due = hours(at(10, 16)) + rules.held_animal_hours;
    let mut scheduler = Scheduler::new();
    let for_tick = rules.clone();
    scheduler
        .register("trade", Cadence::Hourly, move |state, tick| {
            run_trade_hour(&for_tick, state, tick.date()).map_err(|error| {
                Box::new(std::io::Error::other(error)) as Box<dyn Error + Send + Sync>
            })
        })
        .unwrap();
    let mut game = Simulation::new(
        GameTime::from_hours(due - 1),
        0,
        state,
        scheduler,
        |_, _: u8| {},
    );
    // The two clocks either side of the due hour: before its work runs the head is on offer, after it the pending yield is held.
    for (clock, on_offer) in [(due, true), (due + 1, false)] {
        assert!(game.step().unwrap().is_empty());
        assert_eq!(game.time().hours(), clock);
        let job = &game.state().places[0].trade.jobs[0];
        assert_eq!(job.animals.iter().sum::<u64>() > 0, on_offer);
        assert_eq!(job.stock[beef] > 0, !on_offer);
        let mut bytes = Vec::new();
        game.state().encode(&mut bytes).unwrap();
        let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
        validate_trade_state_at(&rules, &decoded, clock).unwrap();
    }
}

#[test]
fn a_driven_head_is_kept_and_one_can_be_bought_to_drive_home_by_an_expedition_on_foot() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let (cattle, fish) = (good(&rules, "cattle"), good(&rules, "fish"));
    // The destination's herd owner has a head to spare and wants the visitor's barley; the visitor, on foot, wants one head and drives it
    // home with its one person as the drover (cattle are not carried on land, so they take no room in a 25 kg pack).
    let mut plan = the_plan(&rules, 6, 5);
    plan.vehicles.clear();
    plan.people = vec![(u32::try_from(PEASANT).unwrap(), 2)];
    plan.cargo = vec![0; rules.goods.len()];
    plan.cargo[fish] = 0;
    plan.wanted = vec![0; rules.goods.len()];
    plan.wanted[cattle] = 1;
    plan.cargo[good(&rules, "barley")] = 1;
    plan.supplies = vec![0; rules.goods.len()];
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    cattle_farm(&rules, &mut state.places[1], 10, &[(MONK, 1)]);
    set_price(&rules, &mut state.places[1], "cattle", 1);
    set_price(&rules, &mut state.places[1], "barley", 1);
    want(&rules, &mut state.places[1], MONK, "barley", 1);
    for hour in 5..=12 {
        tick(&rules, &mut state, at(10, hour));
    }
    let visitor = trip_expedition(&state);
    assert_eq!(visitor.cargo[cattle], 1, "it took a head to drive home");
    assert!(visitor.assigns_home(&rules, &visitor.cargo));
}

// ---- Review 4 (CK-432): the reviewer's probes ----
#[test]
fn a_local_claimants_visitor_allowance_cannot_spend_the_resident_stock_reserved_for_a_neighbour() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let (fish, barley) = (good(&rules, "fish"), good(&rules, "barley"));
    state.places[0].pops[PEASANT].stock[fish] += 1;
    capacity(&mut rules, &mut state.places[1], THIRD, 1_000_000);
    set_price(&rules, &mut state.places[1], "fish", 9);
    set_price(&rules, &mut state.places[1], "barley", 10);
    spare(&rules, &mut state.places[1], MONK, "fish", 1);
    want(&rules, &mut state.places[1], MONK, "barley", 1);
    want(&rules, &mut state.places[1], THIRD, "fish", 1);
    spare(&rules, &mut state.places[1], THIRD, "barley", 2);

    let mut plan = the_plan(&rules, 6, 5);
    plan.cargo.fill(0);
    plan.wanted.fill(0);
    plan.cargo[fish] = 1;
    plan.wanted[barley] = 1;
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    for hour in 5..=11 {
        tick(&rules, &mut state, at(10, hour));
    }
    assert_eq!(trip_expedition(&state).phase, Phase::Waiting);

    let mut neighbour = place(&rules, 1);
    neighbour.id = "neighbour".to_owned();
    settle_the_rest(&rules, &mut neighbour, &[]);
    capacity(&mut rules, &mut neighbour, 0, 1_000_000);
    set_price(&rules, &mut neighbour, "fish", 9);
    set_price(&rules, &mut neighbour, "barley", 10);
    want(&rules, &mut neighbour, 0, "fish", 2);
    spare(&rules, &mut neighbour, 0, "barley", 1);
    neighbour.pops[0].stock[good(&rules, "hardwood")] += 100_000;
    let before_local = state.places[1].pops[THIRD].stock[fish];
    let before_near = neighbour.pops[0].stock[fish];
    state.places.push(neighbour);
    let adjacent = BTreeMap::from([(
        "dest".to_owned(),
        vec![NearbyPlace {
            id: "neighbour".to_owned(),
            distance_m: 3000,
        }],
    )]);
    let prices: Vec<_> = state
        .places
        .iter()
        .map(|p| p.prices_milli.clone())
        .collect();
    let noon = hours(at(10, 12));
    let mut no_visit = state.clone();
    no_visit.places[0].trade.expeditions.clear();
    swap::run_noon_exchange(&rules, &mut no_visit, &prices, &adjacent, noon).unwrap();
    assert_eq!(
        (
            no_visit.places[1].pops[THIRD].stock[fish],
            no_visit.places[2].pops[0].stock[fish]
        ),
        (before_local, before_near + 1),
        "with only one fish, it goes to the neighbouring claimant",
    );

    // The visitor's 9-value fish cannot start by taking the resident's 10-value barley. The resident can start
    // against the visitor at the exact 10% limit. Its allowance from that source must survive its earlier turn
    // with the resident seller, since the only resident fish was allotted to the neighbouring claimant.
    swap::run_noon_exchange(&rules, &mut state, &prices, &adjacent, noon).unwrap();
    let visitor = trip_expedition(&state);
    assert_eq!(
        (
            state.places[1].pops[THIRD].stock[fish],
            state.places[2].pops[0].stock[fish],
            visitor.cargo[fish],
            visitor.cargo[barley]
        ),
        (before_local + 1, before_near + 1, 0, 1),
    );
}

#[test]
fn a_save_cannot_claim_the_supplies_are_gone_while_food_is_aboard() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let departure = hours(at(10, 5));
    let mut plan = the_plan(&rules, 30, 5);
    plan.out[0] = road(45_000, (0, 1));
    state
        .order_trade(&rules, 0, PEASANT, plan, departure)
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    let clock = departure + 1;
    validate_trade_state_at(&rules, &state, clock).unwrap();
    let oats = good(&rules, "oats");
    let expedition = &mut state.places[0].trade.expeditions[0];
    assert_eq!(expedition.supplies[oats], 8);
    assert!(rules.goods[oats].hunger_centi > 0);
    assert!(!expedition.gone);
    assert_eq!(rules.expedition.no_supplies_pace_pm, 500);
    expedition.gone = true;
    expedition.pace_pm /= 2;
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    assert!(validate_trade_state_at(&rules, &decoded, clock).is_err());
}

// ---- Review 5 (CK-432): the reviewer's probes ----
#[test]
fn an_unspendable_visitor_share_does_not_block_the_second_pass_for_resident_stock() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let (fish, barley) = (good(&rules, "fish"), good(&rules, "barley"));
    state.places[0].pops[PEASANT].stock[fish] += 1;
    capacity(&mut rules, &mut state.places[1], THIRD, 1_000_000);
    for (id, price) in [("fish", 10), ("oats", 10), ("barley", 13)] {
        set_price(&rules, &mut state.places[1], id, price);
    }
    spare(&rules, &mut state.places[1], MONK, "fish", 2);
    want(&rules, &mut state.places[1], MONK, "oats", 2);
    state.places[1].pops[MONK].stock[barley] = target(&rules, &state.places[1], MONK, "barley");
    want(&rules, &mut state.places[1], THIRD, "fish", 3);
    spare(&rules, &mut state.places[1], THIRD, "oats", 2);
    spare(&rules, &mut state.places[1], THIRD, "barley", 1);

    let mut plan = the_plan(&rules, 6, 5);
    plan.cargo.fill(0);
    plan.wanted.fill(0);
    plan.cargo[fish] = 1;
    plan.wanted[barley] = 1;
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    for hour in 5..=11 {
        tick(&rules, &mut state, at(10, hour));
    }

    let mut neighbour = place(&rules, 1);
    neighbour.id = "neighbour".to_owned();
    settle_the_rest(&rules, &mut neighbour, &[]);
    capacity(&mut rules, &mut neighbour, 0, 1_000_000);
    // A high value for the fish the neighbour already holds makes it wealthier. It buys at the destination's price.
    set_price(&rules, &mut neighbour, "fish", 1000);
    set_price(&rules, &mut neighbour, "oats", 30);
    want(&rules, &mut neighbour, 0, "fish", 1);
    spare(&rules, &mut neighbour, 0, "oats", 1);
    let before_local = state.places[1].pops[THIRD].stock[fish];
    let before_near = neighbour.pops[0].stock[fish];
    state.places.push(neighbour);
    let adjacent = BTreeMap::from([(
        "dest".to_owned(),
        vec![NearbyPlace {
            id: "neighbour".to_owned(),
            distance_m: 3000,
        }],
    )]);
    let prices: Vec<_> = state
        .places
        .iter()
        .map(|p| p.prices_milli.clone())
        .collect();
    let noon = hours(at(10, 12));
    let mut no_visit = state.clone();
    no_visit.places[0].trade.expeditions.clear();
    swap::run_noon_exchange(&rules, &mut no_visit, &prices, &adjacent, noon).unwrap();
    assert_eq!(
        (
            no_visit.places[1].pops[THIRD].stock[fish],
            no_visit.places[2].pops[0].stock[fish]
        ),
        (before_local + 2, before_near),
        "the neighbour cannot use its allocated fish, so it goes to the local claimant in the second pass",
    );

    // The visitor's fish trades only for barley. At 10 for 13 neither side can start that swap within the loss limit.
    // Its unused share cannot prevent the local claimant from using the neighbour's leftover *resident* fish.
    swap::run_noon_exchange(&rules, &mut state, &prices, &adjacent, noon).unwrap();
    assert_eq!(trip_expedition(&state).cargo[fish], 1);
    assert_eq!(
        (
            state.places[1].pops[THIRD].stock[fish],
            state.places[2].pops[0].stock[fish]
        ),
        (before_local + 2, before_near),
    );
}

#[test]
fn a_save_cannot_recover_full_pace_after_the_crew_has_eaten_its_last_food() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let oats = good(&rules, "oats");
    rules.expedition.daily_hunger_centi = 4 * u64::from(rules.goods[oats].hunger_centi);
    let departure = hours(at(10, 5));
    let mut plan = the_plan(&rules, 30, 5);
    plan.out[0] = road(45_000, (0, 1));
    state
        .order_trade(&rules, 0, PEASANT, plan, departure)
        .unwrap();
    for hour in 5..=12 {
        tick(&rules, &mut state, at(10, hour));
    }
    let clock = hours(at(10, 12)) + 1;
    validate_trade_state_at(&rules, &state, clock).unwrap();
    let expedition = &mut state.places[0].trade.expeditions[0];
    assert!(expedition.gone);
    assert_eq!((expedition.supplies[oats], expedition.pantry_centi), (0, 0));
    assert_eq!(rules.expedition.no_supplies_pace_pm, 500);
    expedition.gone = false;
    let assignment = assign_leg(
        &rules,
        &expedition.vehicles,
        expedition.people(),
        &expedition.cargo,
        &expedition.reckoned,
        expedition.reckoned_mode,
    )
    .unwrap();
    let (pace, limiter) = pace_of(
        &rules,
        &expedition.vehicles,
        &assignment,
        expedition.reckoned_mode,
        false,
        false,
    );
    expedition.pace_pm = pace;
    expedition.limiter = limiter;
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    assert!(validate_trade_state_at(&rules, &decoded, clock).is_err());
}

#[test]
fn one_hour_on_a_mixed_speed_leg_reaches_the_end_of_its_one_hour_stretch() {
    let leg = Leg {
        mode: Mode::Water,
        terms: vec![
            Term {
                work: ratio(6000, 1),
                speed: Speed::Plain(6000),
            },
            Term {
                work: ratio(6000, 1),
                speed: Speed::Plain(2000),
            },
        ],
        fixed: ratio(0, 1),
        delays: Delays::Along,
        key: [0; 4],
        along: Vec::new(),
        path: vec![[0, 0], [200, 0], [400, 0]],
    };
    let mut index = 0;
    let mut done = ratio(0, 1);
    assert_eq!(leg.hours(1000), ratio(4, 1));
    assert!(!expedition::advance(
        std::slice::from_ref(&leg),
        &mut index,
        &mut done,
        ratio(1, 1),
        |_| 1000,
        |_, _| {},
    ));
    assert_eq!(leg.position(&done), Some((ratio(200, 1), ratio(0, 1))));
}

#[test]
fn a_valid_load_still_reloads_after_consuming_one_unit_of_provisions() {
    // This is the submission's own nonmonotone first-fit case with the variable item as food.
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let fleet = vehicles(&rules, &["hand_cart", "ox_cart", "covered_wagon"]);
    state.places[0].trade.vehicles = fleet
        .iter()
        .map(|v| Vehicle {
            number: v.number,
            kind: v.kind,
            pop: u32::try_from(PEASANT).unwrap(),
            free: true,
        })
        .collect();
    state.trade.vehicles = 3;
    state.places[0].pops[PEASANT].workers = 3;
    for (vehicle, (space, weight)) in fleet.iter().zip([(3, 13), (10, 14), (2, 18)]) {
        let kind = &mut rules.vehicles[vehicle.kind as usize];
        kind.carries_ml = space;
        kind.carries_g = weight;
    }
    for (id, space, weight) in [
        ("oats", 2, 5),
        ("barley", 1, 9),
        ("fish", 1, 5),
        ("hides", 1, 1),
    ] {
        let index = good(&rules, id);
        let item = &mut rules.goods[index];
        item.volume_ml = space;
        item.weight_g = weight;
    }
    let oats = good(&rules, "oats");
    rules.goods[oats].hunger_centi = 3;
    rules.expedition.daily_hunger_centi = 1;
    let mut plan = the_plan(&rules, 30, 5);
    plan.vehicles = vec![1, 2, 3];
    plan.people = vec![(u32::try_from(PEASANT).unwrap(), 3)];
    plan.cargo = goods(&rules, &[("barley", 2), ("fish", 1), ("hides", 3)]);
    plan.wanted.fill(0);
    plan.supplies = goods(&rules, &[("oats", 3)]);
    for (good, units) in plan.cargo.iter().enumerate() {
        state.places[0].pops[PEASANT].stock[good] += units;
    }
    assert!(assign_leg(&rules, &fleet, 3, &plan.cargo, &plan.supplies, Mode::Land).is_some());
    let smaller = goods(&rules, &[("oats", 2)]);
    assert!(assign_leg(&rules, &fleet, 3, &plan.cargo, &smaller, Mode::Land).is_none());
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    for hour in 5..12 {
        tick(&rules, &mut state, at(10, hour));
    }
    validate_trade_state_at(&rules, &state, hours(at(10, 12))).unwrap();
    tick(&rules, &mut state, at(10, 12));
    assert_eq!(trip_expedition(&state).supplies[oats], 2);
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    validate_trade_state_at(&rules, &decoded, hours(at(10, 13))).unwrap();
}

#[test]
fn a_noon_departure_that_already_moved_cannot_erase_its_noon_feeding() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let mut plan = the_plan(&rules, 30, 5);
    plan.supplies.fill(0);
    plan.out[0] = road(45_000, (0, 1));
    let noon = hours(at(10, 12));
    state.order_trade(&rules, 0, PEASANT, plan, noon).unwrap();
    tick(&rules, &mut state, at(10, 12));
    let clock = noon + 1;
    validate_trade_state_at(&rules, &state, clock).unwrap();
    let trip = &mut state.places[0].trade.expeditions[0];
    assert!(trip.done > ratio(0, 1));
    assert_eq!(trip.fed, Some(noon));
    assert!(trip.gone);
    let exhausted_pace = trip.pace_pm;
    trip.fed = None;
    trip.gone = false;
    trip.set_pace(&rules).unwrap();
    assert!(trip.pace_pm > exhausted_pace);
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    assert!(validate_trade_state_at(&rules, &decoded, clock).is_err());
}

#[test]
fn a_residents_unspent_resident_share_does_not_block_a_visitor_share_when_neighbours_drop_out() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let (fish, oats) = (good(&rules, "fish"), good(&rules, "oats"));
    state.places[0].pops[PEASANT].stock[fish] += 1;
    let mut plan = the_plan(&rules, 6, 5);
    plan.cargo.fill(0);
    plan.wanted.fill(0);
    plan.cargo[fish] = 1;
    plan.wanted[oats] = 3;
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    for hour in 5..=11 {
        tick(&rules, &mut state, at(10, hour));
    }

    let dest = &mut state.places[1];
    for (id, price) in [("fish", 10), ("oats", 3), ("barley", 3), ("hides", 100_000)] {
        set_price(&rules, dest, id, price);
    }
    // The wealthier local wins the first visitor share but has only one of the three oats needed.
    dest.pops[PEASANT].stock[good(&rules, "hides")] += 10;
    capacity(&mut rules, dest, THIRD, 1_000_000);
    want(&rules, dest, PEASANT, "fish", 1);
    spare(&rules, dest, PEASANT, "oats", 1);
    want(&rules, dest, THIRD, "fish", 2);
    spare(&rules, dest, THIRD, "oats", 3);
    spare(&rules, dest, MONK, "fish", 2);
    want(&rules, dest, MONK, "barley", 3);
    let before = dest.pops[THIRD].stock[fish];

    let mut neighbour = place(&rules, 1);
    neighbour.id = "neighbour".to_owned();
    settle_the_rest(&rules, &mut neighbour, &[]);
    set_price(&rules, &mut neighbour, "barley", 3);
    want(&rules, &mut neighbour, 0, "fish", 2);
    spare(&rules, &mut neighbour, 0, "barley", 1);
    state.places.push(neighbour);
    let adjacent = BTreeMap::from([(
        "dest".to_owned(),
        vec![NearbyPlace {
            id: "neighbour".to_owned(),
            distance_m: 3000,
        }],
    )]);
    let prices: Vec<_> = state
        .places
        .iter()
        .map(|p| p.prices_milli.clone())
        .collect();
    let noon = hours(at(10, 12));

    // Control: with only the buyer who can pay, the visitor's share buys its one fish.
    let mut control = state.clone();
    control.places[1].pops[PEASANT].stock[fish] =
        target(&rules, &control.places[1], PEASANT, "fish");
    swap::run_noon_exchange(&rules, &mut control, &prices, &adjacent, noon).unwrap();
    assert_eq!(control.places[1].pops[THIRD].stock[fish], before + 1);
    assert_eq!(trip_expedition(&control).cargo[fish], 0);

    // The neighbour gets one resident share and cannot buy it. THIRD gets the other resident share
    // but offers oats, which the resident seller does not want. THIRD can buy the visitor's fish
    // once the unusable visitor share is redistributed, regardless of either resident share.
    swap::run_noon_exchange(&rules, &mut state, &prices, &adjacent, noon).unwrap();
    assert_eq!(state.places[1].pops[THIRD].stock[fish], before + 1);
    assert_eq!(trip_expedition(&state).cargo[fish], 0);
}

// ---- Review 6 (CK-432): the same cases through the other phases and across a transfer ----

/// The submission's own nonmonotone first-fit convoy: three carriers whose first-fit takes three oats but not two, all of which move
/// on land and on water alike, so a transfer packs the same carriers again.
fn three_carrier_convoy() -> (Rules, EconomyState, Plan) {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let fleet = vehicles(&rules, &["hand_cart", "ox_cart", "covered_wagon"]);
    state.places[0].trade.vehicles = fleet
        .iter()
        .map(|v| Vehicle {
            number: v.number,
            kind: v.kind,
            pop: u32::try_from(PEASANT).unwrap(),
            free: true,
        })
        .collect();
    state.trade.vehicles = 3;
    state.places[0].pops[PEASANT].workers = 3;
    let boat = rules.vehicles.iter().find_map(|kind| kind.water.clone());
    for (vehicle, (space, weight)) in fleet.iter().zip([(3, 13), (10, 14), (2, 18)]) {
        let kind = &mut rules.vehicles[vehicle.kind as usize];
        kind.carries_ml = space;
        kind.carries_g = weight;
        kind.water.clone_from(&boat);
    }
    for (id, space, weight) in [
        ("oats", 2, 5),
        ("barley", 1, 9),
        ("fish", 1, 5),
        ("hides", 1, 1),
    ] {
        let index = good(&rules, id);
        let item = &mut rules.goods[index];
        item.volume_ml = space;
        item.weight_g = weight;
    }
    let oats = good(&rules, "oats");
    rules.goods[oats].hunger_centi = 3;
    rules.expedition.daily_hunger_centi = 1;
    let mut plan = the_plan(&rules, 30, 5);
    plan.vehicles = vec![1, 2, 3];
    plan.people = vec![(u32::try_from(PEASANT).unwrap(), 3)];
    plan.cargo = goods(&rules, &[("barley", 2), ("fish", 1), ("hides", 3)]);
    plan.wanted.fill(0);
    plan.supplies = goods(&rules, &[("oats", 3)]);
    for (good, units) in plan.cargo.iter().enumerate() {
        state.places[0].pops[PEASANT].stock[good] += units;
    }
    (rules, state, plan)
}

#[test]
fn a_convoy_that_ate_a_provision_crosses_a_transfer_with_the_load_it_was_packed_with_and_every_saved_hour_reloads()
 {
    let (rules, mut state, mut plan) = three_carrier_convoy();
    let oats = good(&rules, "oats");
    let fleet = vehicles(&rules, &["hand_cart", "ox_cart", "covered_wagon"]);
    // A fresh packing of the two oats left after the first noon fails, which is what a transfer used to do with them.
    let smaller = goods(&rules, &[("oats", 2)]);
    assert!(assign_leg(&rules, &fleet, 3, &plan.cargo, &smaller, Mode::Water).is_none());
    // Ten hours on land (past the first noon, which is seven hours out), then the water.
    plan.out = vec![fixed_leg(Mode::Land, 10), fixed_leg(Mode::Water, 6)];
    let left = hours(at(10, 5));
    state.order_trade(&rules, 0, PEASANT, plan, left).unwrap();
    for step in 0..40 {
        tick(&rules, &mut state, date_of(left + step));
        let clock = left + step + 1;
        validate_trade_state_at(&rules, &state, clock).unwrap();
        let mut bytes = Vec::new();
        state.encode(&mut bytes).unwrap();
        let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
        validate_trade_state_at(&rules, &decoded, clock).unwrap();
        if trip_expedition(&state).phase == Phase::Waiting {
            break;
        }
    }
    let expedition = trip_expedition(&state);
    assert_eq!(expedition.phase, Phase::Waiting, "it crossed the transfer");
    assert_eq!(expedition.supplies[oats], 2, "it ate one at the noon");
}

#[test]
fn a_noon_departure_is_fed_that_noon_from_the_daily_step_and_not_from_the_hours_step_and_each_saves_and_reloads()
 {
    for (daily, fed, moved) in [(true, Some(hours(at(10, 12))), true), (false, None, false)] {
        let mut rules = herd_rules();
        let mut state = two_places(&mut rules);
        let mut plan = the_plan(&rules, 30, 5);
        plan.out[0] = road(45_000, (0, 1));
        let noon = hours(at(10, 12));
        state.order_trade(&rules, 0, PEASANT, plan, noon).unwrap();
        if daily {
            tick(&rules, &mut state, at(10, 12));
        } else {
            // The hour's step alone: the job starts after the expeditions have moved, and nothing is eaten until the next noon.
            let place = &mut state.places[0];
            trade::start_ready(&rules, place, &mut state.trade, noon, false).unwrap();
        }
        let expedition = trip_expedition(&state);
        assert_eq!(expedition.daily, daily);
        assert_eq!(expedition.fed, fed);
        assert_eq!(expedition.leg != 0 || expedition.done != ratio(0, 1), moved);
        let clock = noon + 1;
        validate_trade_state_at(&rules, &state, clock).unwrap();
        let mut bytes = Vec::new();
        state.encode(&mut bytes).unwrap();
        let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
        validate_trade_state_at(&rules, &decoded, clock).unwrap();
        // The other phase's history is not this one's: clearing the feeding, or claiming the other departure, is refused.
        let mut forged = state.clone();
        let trip = &mut forged.places[0].trade.expeditions[0];
        trip.daily = !daily;
        assert!(validate_trade_state_at(&rules, &forged, clock).is_err());
    }
}

#[test]
fn a_mixed_speed_leg_and_a_located_delay_put_the_expedition_where_its_movement_is_after_a_save() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let mut plan = the_plan(&rules, 30, 5);
    // On foot, at the walking pace of 1,000, so the hours are the stretches' own.
    plan.vehicles.clear();
    plan.cargo.fill(0);
    plan.wanted.fill(0);
    plan.supplies.fill(0);
    plan.supplies[good(&rules, "oats")] = 2;
    // Two equal stretches of 6,000 and 2,000 metres an hour (one hour and three), then a boarding delay of five hours at the shore.
    plan.out = vec![Leg {
        mode: Mode::Land,
        terms: vec![
            Term {
                work: ratio(6000, 1),
                speed: Speed::Plain(6000),
            },
            Term {
                work: ratio(6000, 1),
                speed: Speed::Plain(2000),
            },
        ],
        fixed: ratio(5, 1),
        delays: Delays::AtEnd,
        key: [0; 4],
        along: Vec::new(),
        path: vec![[0, 0], [200, 0], [400, 0]],
    }];
    let left = hours(at(10, 5));
    state.order_trade(&rules, 0, PEASANT, plan, left).unwrap();
    let mut seen = Vec::new();
    for step in 0..12 {
        tick(&rules, &mut state, date_of(left + step));
        let clock = left + step + 1;
        validate_trade_state_at(&rules, &state, clock).unwrap();
        let mut bytes = Vec::new();
        state.encode(&mut bytes).unwrap();
        let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
        validate_trade_state_at(&rules, &decoded, clock).unwrap();
        let (live, saved) = (
            trip_expedition(&state).position(),
            trip_expedition(&decoded).position(),
        );
        assert_eq!(live, saved, "a loaded game is where the unbroken one is");
        seen.push(live.map(|(x, _)| x));
        if trip_expedition(&state).phase == Phase::Waiting {
            break;
        }
    }
    // The hour's step starts the job at the end of the first tick, so it moves from the next: the first stretch takes one hour to the
    // middle, a third of the second takes it to two thirds of the way (not one sixth), and the five hours of boarding are waited out
    // at the end before it arrives.
    let at_x = |numer: u128, denom: u128| Some(ratio(numer, denom));
    assert_eq!(seen[0], at_x(0, 1));
    assert_eq!(seen[1], at_x(200, 1), "one hour is the whole first stretch");
    assert_eq!(seen[2], at_x(800, 3));
    assert_eq!(seen[3], at_x(1000, 3));
    assert!(
        seen[4..9].iter().all(|x| *x == at_x(400, 1)),
        "it waits at the shore: {seen:?}"
    );
    assert_eq!(
        seen[9],
        at_x(400, 1),
        "it arrived, so it waits at the end of the way"
    );
}

#[test]
fn a_delay_at_the_start_of_a_leg_is_waited_out_at_its_first_point_and_one_at_the_end_at_its_last() {
    let stretch = |delays, hours: u128| Leg {
        mode: Mode::Land,
        terms: vec![Term {
            work: ratio(3000, 1),
            speed: Speed::Plain(3000),
        }],
        fixed: ratio(hours, 1),
        delays,
        key: [0; 4],
        along: Vec::new(),
        path: vec![[0, 0], [100, 0]],
    };
    let x = |leg: &Leg, fraction: (u128, u128)| {
        leg.position(&ratio(fraction.0, fraction.1)).map(|(x, _)| x)
    };
    // A landing: two hours at the shore, then the one hour of walking.
    let landing = stretch(Delays::AtStart, 2);
    assert_eq!(x(&landing, (1, 3)), Some(ratio(0, 1)), "still at the shore");
    assert_eq!(
        x(&landing, (2, 3)),
        Some(ratio(0, 1)),
        "its last moment there"
    );
    assert_eq!(x(&landing, (5, 6)), Some(ratio(50, 1)));
    assert_eq!(x(&landing, (1, 1)), Some(ratio(100, 1)));
    // A boarding: the walking first, then two hours at the shore.
    let boarding = stretch(Delays::AtEnd, 2);
    assert_eq!(x(&boarding, (1, 6)), Some(ratio(50, 1)));
    assert_eq!(x(&boarding, (1, 3)), Some(ratio(100, 1)));
    assert_eq!(
        x(&boarding, (2, 3)),
        Some(ratio(100, 1)),
        "waiting at the shore"
    );
    // A ford spread along the stretch moves it on all the time.
    let ford = stretch(Delays::Along, 2);
    assert_eq!(x(&ford, (1, 2)), Some(ratio(50, 1)));
    // At half the pace the walking takes twice as long: an hour of it is half the stretch, the same place whichever pace the progress
    // was made at, and the rest of the delay still takes its two hours.
    let walked = boarding.progress_after(&ratio(0, 1), &ratio(1, 1), 500);
    assert_eq!(walked, ratio(1, 6));
    assert_eq!(
        boarding.position(&walked).map(|(x, _)| x),
        Some(ratio(50, 1))
    );
    assert_eq!(boarding.hours_left(&walked, 500), ratio(3, 1));
    assert_eq!(boarding.hours_left(&walked, 2000), ratio(9, 4));
}

#[test]
fn every_hour_of_whole_trips_validates_live_and_after_a_save_whatever_the_departure_hour_and_phase()
{
    // Whole trips from every hour of a day (a noon departure in both its phases), over supplies from none to plenty and legs of a few
    // hours to some days: each hour's state, taken as the saved clock says, validates as it is and as it is after a save and load.
    let mut checked = 0;
    for hour in 0..24_u8 {
        for noon_in_hours_step in [false, true] {
            if noon_in_hours_step && hour != 12 {
                continue;
            }
            for (out, back, oats) in [(3_u64, 2_u64, 0_u64), (20, 5, 1), (30, 30, 3), (60, 20, 8)] {
                let mut rules = herd_rules();
                let mut state = two_places(&mut rules);
                let mut plan = the_plan(&rules, out, back);
                plan.supplies[good(&rules, "oats")] = oats;
                let departure = hours(at(10, hour));
                state
                    .order_trade(&rules, 0, PEASANT, plan, departure)
                    .unwrap();
                for step in 0..200 {
                    let now = departure + step;
                    if noon_in_hours_step && step == 0 {
                        // The hour's step alone starts the job, after the expeditions have moved, with no daily step before it.
                        let place = &mut state.places[0];
                        trade::start_ready(&rules, place, &mut state.trade, now, false).unwrap();
                    } else {
                        tick(&rules, &mut state, date_of(now));
                    }
                    let clock = now + 1;
                    validate_trade_state_at(&rules, &state, clock).unwrap_or_else(|error| {
                        panic!("hour {hour} legs {out}/{back} oats {oats} step {step}: {error:?}")
                    });
                    let mut bytes = Vec::new();
                    state.encode(&mut bytes).unwrap();
                    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
                    validate_trade_state_at(&rules, &decoded, clock).unwrap();
                    assert_eq!(decoded, state);
                    checked += 1;
                    if state.places[0].trade.expeditions.is_empty() {
                        break;
                    }
                }
                assert!(
                    state.places[0].trade.expeditions.is_empty(),
                    "it came home: hour {hour} legs {out}/{back} oats {oats}"
                );
            }
        }
    }
    assert!(checked > 1000, "{checked} hours were checked");
}

/// Every good in the world, by good: the pops' stores, the buildings' stocks and what each expedition carries.
fn world_total(rules: &Rules, state: &EconomyState) -> Vec<u64> {
    let mut sum = vec![0_u64; rules.goods.len()];
    for place in &state.places {
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
        for expedition in &place.trade.expeditions {
            for (index, total) in sum.iter_mut().enumerate() {
                *total += expedition.cargo[index] + expedition.supplies[index];
            }
        }
    }
    sum
}

/// A destination at noon with its pops, a waiting visitor carrying fish and wanting oats home, and a neighbouring place, the stocks,
/// wants, prices and wealth of each rolled, the neighbour adjacent or not. Returns the visitor's cargo of fish as well.
fn exchange_world(
    roll: &mut impl FnMut(u64) -> u64,
) -> (Rules, EconomyState, BTreeMap<String, Vec<NearbyPlace>>, u64) {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let (fish, oats) = (good(&rules, "fish"), good(&rules, "oats"));
    // The visitor takes fish and wants oats home.
    let carried = 1 + roll(4);
    state.places[0].pops[PEASANT].stock[fish] += carried;
    let mut plan = the_plan(&rules, 6, 5);
    plan.cargo.fill(0);
    plan.wanted.fill(0);
    plan.cargo[fish] = carried;
    plan.wanted[oats] = 1 + roll(4);
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    for hour in 5..=11 {
        tick(&rules, &mut state, at(10, hour));
    }
    let dest = &mut state.places[1];
    for (id, price) in [
        ("fish", 1 + roll(3)),
        ("oats", 1 + roll(3)),
        ("barley", 1 + roll(3)),
    ] {
        set_price(&rules, dest, id, price);
    }
    let hides = good(&rules, "hides");
    dest.pops[PEASANT].stock[hides] += roll(40);
    dest.pops[THIRD].stock[hides] += roll(40);
    capacity(&mut rules, dest, THIRD, 1_000_000);
    let clamp = |rules: &Rules, dest: &PlaceState, pop: usize, id: &str, units: u64| {
        units.min(target(rules, dest, pop, id))
    };
    for pop in [PEASANT, THIRD] {
        let units = clamp(&rules, dest, pop, "fish", roll(4));
        want(&rules, dest, pop, "fish", units);
        spare(&rules, dest, pop, "oats", roll(4));
    }
    spare(&rules, dest, MONK, "fish", roll(4));
    let units = clamp(&rules, dest, MONK, "barley", roll(4));
    want(&rules, dest, MONK, "barley", units);
    let mut neighbour = place(&rules, 1);
    neighbour.id = "neighbour".to_owned();
    settle_the_rest(&rules, &mut neighbour, &[]);
    capacity(&mut rules, &mut neighbour, 0, 1_000_000);
    // The neighbour's prices run wide, so it sometimes buys all it wants in the first pass (the last buyer gone) and sometimes cannot
    // pay at all.
    for (id, price) in [("fish", 1 + roll(4)), ("barley", 1 + roll(4))] {
        set_price(&rules, &mut neighbour, id, price);
    }
    let units = clamp(&rules, &neighbour, 0, "fish", roll(4));
    want(&rules, &mut neighbour, 0, "fish", units);
    spare(&rules, &mut neighbour, 0, "barley", roll(3));
    state.places.push(neighbour);
    let near = roll(3) != 0;
    let adjacent = if near {
        BTreeMap::from([(
            "dest".to_owned(),
            vec![NearbyPlace {
                id: "neighbour".to_owned(),
                distance_m: 3000,
            }],
        )])
    } else {
        BTreeMap::new()
    };
    (rules, state, adjacent, carried)
}

#[test]
fn one_noon_exchange_conserves_every_pool_across_both_passes_with_pops_a_visitor_and_neighbours() {
    // A scarce good claimed by the destination's pops, a waiting visitor and a neighbouring place, in many mixes of stock, wants,
    // prices and wealth, with and without the neighbour: nothing is made or lost between the pops, the visitor and the neighbour, no
    // store goes negative or past what it wanted, and whatever a share was kept for is spent or forfeited, never double spent.
    let mut state_seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut roll = move |below: u64| {
        state_seed = state_seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state_seed >> 33) % below
    };
    let (mut traded, mut visitor_traded, mut neighbour_traded) = (0, 0, 0);
    for case in 0..400 {
        let (rules, mut state, adjacent, carried) = exchange_world(&mut roll);
        let (fish, oats, barley) = (
            good(&rules, "fish"),
            good(&rules, "oats"),
            good(&rules, "barley"),
        );
        let prices: Vec<_> = state
            .places
            .iter()
            .map(|p| p.prices_milli.clone())
            .collect();
        let before = world_total(&rules, &state);
        let (barley_before, fish_before) = (
            state.places[2].pops[0].stock[barley],
            state.places[2].pops[0].stock[fish],
        );
        swap::run_noon_exchange(&rules, &mut state, &prices, &adjacent, hours(at(11, 12)))
            .unwrap_or_else(|error| panic!("case {case}: {error:?}"));
        let after = world_total(&rules, &state);
        assert_eq!(after, before, "case {case}: every good is conserved");
        let visitor = trip_expedition(&state);
        if visitor.cargo[fish] != carried || visitor.cargo[oats] != 0 {
            visitor_traded += 1;
        }
        if state.places[2].pops[0].stock[barley] != barley_before
            || state.places[2].pops[0].stock[fish] != fish_before
        {
            neighbour_traded += 1;
        }
        traded += 1;
    }
    assert!(
        visitor_traded > 50,
        "the visitor traded in {visitor_traded} of {traded}"
    );
    assert!(
        neighbour_traded > 20,
        "the neighbour traded in {neighbour_traded} of {traded}"
    );
}

// ---- Review 7 (CK-432): the reviewer's probes ----

#[test]
fn exhausting_provisions_during_a_located_delay_preserves_elapsed_wait_and_position() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let mut plan = the_plan(&rules, 30, 5);
    plan.supplies.fill(0);
    plan.out[0] = road(45_000, (4, 1));
    plan.out[0].delays = Delays::AtStart;
    plan.out[0].path = vec![[0, 0], [1000, 0]];

    let departure = hours(at(10, 5));
    state
        .order_trade(&rules, 0, PEASANT, plan, departure)
        .unwrap();
    for hour in 5..12 {
        tick(&rules, &mut state, at(10, hour));
    }
    let trip = trip_expedition(&state);
    let before = trip.position().unwrap();
    let before_pace = trip.pace_pm;
    assert!(before.0 > ratio(0, 1), "already passed the four-hour delay");
    assert!(!trip.gone);

    // Feeding is the daily step. No movement has run at this noon yet.
    expedition::feed(&rules, &mut state.places[0], hours(at(10, 12))).unwrap();
    let trip = trip_expedition(&state);
    assert!(trip.gone && trip.pace_pm < before_pace);
    assert_eq!(trip.position().unwrap(), before);
}

#[test]
fn a_waiting_expedition_has_the_same_arrival_position_after_a_save() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let mut plan = the_plan(&rules, 6, 5);
    plan.out[0] = road(1_000, (0, 1));
    plan.out[0].path = vec![[0, 0], [1000, 1500]];
    assert_eq!(
        plan.out[0].position(&ratio(1, 1)),
        Some((ratio(1000, 1), ratio(1500, 1)))
    );
    let departure = hours(at(10, 5));
    state
        .order_trade(&rules, 0, PEASANT, plan, departure)
        .unwrap();
    for hour in 5..=11 {
        tick(&rules, &mut state, at(10, hour));
    }
    assert_eq!(trip_expedition(&state).phase, Phase::Waiting);
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let restored = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    validate_trade_state_at(&rules, &restored, hours(at(10, 12))).unwrap();
    assert_eq!(
        trip_expedition(&restored).position(),
        Some((ratio(1000, 1), ratio(1500, 1)))
    );
}

#[test]
fn a_neighbour_finishing_its_purchase_does_not_merge_the_stock_pools_in_the_second_pass() {
    let mut rules = herd_rules();
    let mut state = two_places(&mut rules);
    let (fish, oats) = (good(&rules, "fish"), good(&rules, "oats"));
    state.places[0].pops[PEASANT].stock[fish] += 1;
    let mut plan = the_plan(&rules, 6, 5);
    plan.cargo.fill(0);
    plan.wanted.fill(0);
    plan.cargo[fish] = 1;
    plan.wanted[oats] = 3;
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    for hour in 5..=11 {
        tick(&rules, &mut state, at(10, hour));
    }

    let dest = &mut state.places[1];
    for (id, price) in [("fish", 10), ("oats", 3), ("barley", 3), ("hides", 100_000)] {
        set_price(&rules, dest, id, price);
    }
    // The wealthier local wins the first visitor share but has only one of the three oats needed.
    dest.pops[PEASANT].stock[good(&rules, "hides")] += 10;
    capacity(&mut rules, dest, THIRD, 1_000_000);
    want(&rules, dest, PEASANT, "fish", 1);
    spare(&rules, dest, PEASANT, "oats", 1);
    want(&rules, dest, THIRD, "fish", 2);
    spare(&rules, dest, THIRD, "oats", 3);
    spare(&rules, dest, MONK, "fish", 2);
    want(&rules, dest, MONK, "barley", 3);
    let before = dest.pops[THIRD].stock[fish];

    let mut neighbour = place(&rules, 1);
    neighbour.id = "neighbour".to_owned();
    settle_the_rest(&rules, &mut neighbour, &[]);
    set_price(&rules, &mut neighbour, "fish", 10_000);
    capacity(&mut rules, &mut neighbour, 0, 1_000_000);
    set_price(&rules, &mut neighbour, "barley", 10);
    want(&rules, &mut neighbour, 0, "fish", 1);
    spare(&rules, &mut neighbour, 0, "barley", 1);
    let neighbour_before = neighbour.pops[0].stock[fish];
    state.places.push(neighbour);
    let adjacent = BTreeMap::from([(
        "dest".to_owned(),
        vec![NearbyPlace {
            id: "neighbour".to_owned(),
            distance_m: 3000,
        }],
    )]);
    let prices: Vec<_> = state
        .places
        .iter()
        .map(|p| p.prices_milli.clone())
        .collect();
    let noon = hours(at(10, 12));

    // Control: with only the buyer who can pay, the visitor's share buys its one fish.
    let mut control = state.clone();
    control.places[1].pops[PEASANT].stock[fish] =
        target(&rules, &control.places[1], PEASANT, "fish");
    swap::run_noon_exchange(&rules, &mut control, &prices, &adjacent, noon).unwrap();
    assert_eq!(control.places[1].pops[THIRD].stock[fish], before + 1);
    assert_eq!(trip_expedition(&control).cargo[fish], 0);

    // The neighbour spends its resident share and satisfies its want. THIRD still has an unused
    // resident allowance, but if it keeps its visitor eligibility it can buy the visiting fish.
    swap::run_noon_exchange(&rules, &mut state, &prices, &adjacent, noon).unwrap();
    assert_eq!(state.places[2].pops[0].stock[fish], neighbour_before + 1);
    assert_eq!(state.places[1].pops[THIRD].stock[fish], before + 1);
    assert_eq!(trip_expedition(&state).cargo[fish], 0);
}

#[test]
fn a_legs_progress_time_and_position_come_from_one_account_whatever_the_pace_does() {
    // Legs of mixed speeds with a delay along them, at their start and at their end, and a leg that is only a delay.
    let stretch = |work: u128, speed: u64| Term {
        work: ratio(work, 1),
        speed: Speed::Plain(speed),
    };
    let make = |delays, terms: Vec<Term>, fixed: u128| Leg {
        mode: Mode::Land,
        terms,
        fixed: ratio(fixed, 1),
        delays,
        key: [0; 4],
        along: Vec::new(),
        path: vec![[0, 0], [100, 0], [300, 0]],
    };
    let mut legs = Vec::new();
    for delays in [Delays::Along, Delays::AtStart, Delays::AtEnd] {
        legs.push(make(
            delays,
            vec![stretch(6000, 6000), stretch(6000, 2000)],
            4,
        ));
        legs.push(make(delays, vec![stretch(3000, 1500)], 3));
    }
    legs.push(make(Delays::Along, Vec::new(), 5));
    let paces = [1_u32, 300, 604, 1000, 1700, 5000];
    let mut checked = 0;
    for leg in &legs {
        for first in paces {
            for second in paces {
                // Travel some hours at one pace, then the rest at another: the places visited do not move with the change of pace,
                // and what is left takes what the time still to do at the new pace says.
                let total = leg.hours(first);
                for tenth in 0..10_u128 {
                    let budget = &total * ratio(tenth, 10);
                    if budget >= total {
                        continue;
                    }
                    let done = leg.progress_after(&ratio(0, 1), &budget, first);
                    assert_eq!(leg.hours_left(&done, first), &total - &budget);
                    let before = leg.position(&done);
                    // Whatever pace comes next, the position is the one the progress gave.
                    assert_eq!(leg.position(&done), before);
                    // Travelling on at the new pace, the hours left shrink hour for hour.
                    let left = leg.hours_left(&done, second);
                    if left > ratio(1, 2) {
                        let later = leg.progress_after(&done, &ratio(1, 2), second);
                        assert_eq!(leg.hours_left(&later, second), &left - ratio(1, 2));
                        assert!(later >= done, "progress never goes back");
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 500);
}

// ---- Review 8 (CK-432): the reviewer's wear probe ----

#[test]
fn a_transfer_charges_wear_for_each_vehicles_own_movement() {
    let mut rules = herd_rules();
    rules.expedition.wear_hours = 1;
    let currach = kind(&rules, "currach") as usize;
    rules.vehicles[currach].carried_g = Some(50_000);
    rules.vehicles[currach].carried_ml = Some(200_000);
    let mut state = two_places(&mut rules);
    let cart = add_vehicle(&mut state, &rules, "hand_cart");
    let boat = add_vehicle(&mut state, &rules, "currach");
    let hardwood = good(&rules, "hardwood");
    let hide = good(&rules, "hides");
    let mut plan = the_plan(&rules, 1, 1);
    plan.vehicles = vec![cart, boat];
    plan.people = vec![(u32::try_from(PEASANT).unwrap(), 4)];
    plan.cargo.fill(0);
    plan.wanted.fill(0);
    plan.supplies.fill(0);
    plan.supplies[hardwood] = 1;
    plan.supplies[hide] = 1;
    state.places[0].pops[PEASANT].stock[hardwood] += 1;
    state.places[0].pops[PEASANT].stock[hide] += 1;
    let mut water = road(35_000, (0, 1));
    water.mode = Mode::Water;
    plan.out = vec![road(100, (0, 1)), water.clone()];
    plan.back = vec![water, road(100, (0, 1))];
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    let departed = trip_expedition(&state);
    let land = departed.plan.out[0].hours(departed.pace_pm);
    assert!(land > ratio(0, 1) && land < ratio(1, 1));
    assert_eq!(
        (departed.supplies[hardwood], departed.supplies[hide]),
        (1, 1)
    );

    tick(&rules, &mut state, at(10, 6));
    let aboard = trip_expedition(&state);
    assert_eq!(aboard.leg, 1);
    assert!(aboard.done > ratio(0, 1));
    assert_eq!(
        (aboard.supplies[hardwood], aboard.supplies[hide]),
        (1, 1),
        "neither carrier has moved a whole hour"
    );

    // The part of an hour each has moved is kept, exactly, through a save.
    assert!(
        aboard
            .vehicles
            .iter()
            .any(|vehicle| vehicle.worn_hours > ratio(0, 1) && vehicle.worn_hours < ratio(1, 1))
    );
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let decoded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    validate_trade_state_at(&rules, &decoded, hours(at(10, 7))).unwrap();
    assert_eq!(decoded, state);
    tick(&rules, &mut state, at(10, 7));
    let aboard = trip_expedition(&state);
    assert_eq!(
        (aboard.supplies[hardwood], aboard.supplies[hide]),
        (1, 0),
        "only the boat has now moved at least one hour"
    );
}

#[test]
fn a_path_with_bends_keeps_them_at_their_distances_and_a_disordered_one_is_refused() {
    let mut rules = herd_rules();
    let state = two_places(&mut rules);
    let mut bent = fixed_leg(Mode::Land, 11);
    bent.terms = vec![Term {
        work: ratio(11, 1),
        speed: Speed::Plain(1),
    }];
    bent.fixed = ratio(0, 1);
    // A road that goes ten along and then a hundred up: eleven hours, the corner at the first hour.
    bent.path = vec![[0, 0], [10, 0], [10, 100]];
    bent.along = vec![0, 10, 110];
    assert_eq!(
        bent.position(&ratio(10, 110)),
        Some((ratio(10, 1), ratio(0, 1))),
        "the corner is where it is, not a chord across it"
    );
    assert_eq!(
        bent.position(&ratio(60, 110)),
        Some((ratio(10, 1), ratio(50, 1)))
    );
    let mut bytes = Vec::new();
    bent.encode(&mut bytes).unwrap();
    assert_eq!(Leg::decode(&mut Reader::new(&bytes)).unwrap(), bent);
    for (name, along) in [
        ("out of order", vec![0, 110, 10]),
        ("not from the start", vec![5, 10, 110]),
        ("the wrong number", vec![0, 110]),
    ] {
        let mut plan = the_plan(&rules, 6, 5);
        plan.out = vec![bent.clone()];
        plan.out[0].along = along;
        assert!(
            state
                .clone()
                .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
                .is_err(),
            "{name}"
        );
    }
}

// ---- Review 9 (CK-432): the reviewer's probe ----

#[test]
fn a_transfer_reckons_the_boat_with_supplies_left_after_the_carts_wear() {
    let mut rules = herd_rules();
    rules.expedition.wear_hours = 1;
    let currach = kind(&rules, "currach") as usize;
    rules.vehicles[currach].carried_g = Some(50_000);
    rules.vehicles[currach].carried_ml = Some(200_000);
    let mut state = two_places(&mut rules);
    let cart = add_vehicle(&mut state, &rules, "hand_cart");
    let boat = add_vehicle(&mut state, &rules, "currach");
    let hardwood = good(&rules, "hardwood");
    let mut plan = the_plan(&rules, 1, 1);
    plan.vehicles = vec![cart, boat];
    plan.people = vec![(u32::try_from(PEASANT).unwrap(), 4)];
    plan.cargo.fill(0);
    plan.wanted.fill(0);
    plan.supplies.fill(0);
    plan.supplies[hardwood] = 1;
    state.places[0].pops[PEASANT].stock[hardwood] += 1;

    let fleet = vehicles(&rules, &["hand_cart", "currach"]);
    let make_leg = |mode, time: Hours| {
        let assigned = assign_leg(&rules, &fleet, 4, &plan.cargo, &plan.supplies, mode).unwrap();
        let (pace, _) = pace_of(&rules, &fleet, &assigned, mode, false, false);
        Leg {
            mode,
            terms: vec![Term {
                work: time * ratio(u128::from(pace), 1),
                speed: Speed::Plain(1000),
            }],
            ..fixed_leg(mode, 0)
        }
    };
    // The first movement tick spends a quarter hour on water and three quarters on land.
    // Next tick the cart crosses its one-hour wear threshold, then reaches the water.
    plan.out = vec![
        make_leg(Mode::Water, ratio(1, 4)),
        make_leg(Mode::Land, ratio(5, 4)),
        make_leg(Mode::Water, ratio(10, 1)),
    ];
    plan.back = vec![fixed_leg(Mode::Water, 2)];
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    tick(&rules, &mut state, at(10, 6));
    let trip = trip_expedition(&state);
    assert_eq!(trip.leg, 1);
    assert_eq!(trip.vehicles[0].worn_hours, ratio(3, 4));
    assert_eq!(trip.supplies[hardwood], 1);

    tick(&rules, &mut state, at(10, 7));
    let trip = trip_expedition(&state);
    assert_eq!(trip.leg, 2);
    assert_eq!(trip.vehicles[0].worn_hours, ratio(1, 4));
    assert_eq!(trip.supplies[hardwood], 0);
    let with_actual_load = expedition::assign_after(
        &rules,
        &trip.vehicles,
        trip.people(),
        &trip.cargo,
        &trip.loaded,
        &trip.supplies,
        Mode::Water,
    )
    .unwrap();
    let (expected, _) = pace_of(
        &rules,
        &trip.vehicles,
        &with_actual_load,
        Mode::Water,
        false,
        false,
    );
    assert_eq!(
        trip.pace_pm, expected,
        "hardwood was spent before the boat began moving"
    );
}

// ---- Review 10 (CK-432): the reviewer's probe ----
#[test]
fn wear_spends_the_quantity_in_vehicle_rules() {
    let mut rules = herd_rules();
    rules.expedition.wear_hours = 1;
    let ox_cart = kind(&rules, "ox_cart") as usize;
    rules.vehicles[ox_cart].wear.units = 2;
    rules.validate("review10 rules").unwrap();

    let mut state = two_places(&mut rules);
    let hardwood = good(&rules, "hardwood");
    let mut plan = the_plan(&rules, 30, 5);
    plan.cargo.fill(0);
    plan.wanted.fill(0);
    plan.supplies.fill(0);
    plan.supplies[hardwood] = 3;
    state.places[0].pops[PEASANT].stock[hardwood] += 3;
    state
        .order_trade(&rules, 0, PEASANT, plan, hours(at(10, 5)))
        .unwrap();
    tick(&rules, &mut state, at(10, 5));
    let departed = trip_expedition(&state);
    assert_eq!(departed.vehicles[0].kind as usize, ox_cart);
    assert_eq!(departed.supplies[hardwood], 3);

    tick(&rules, &mut state, at(10, 6));
    let trip = trip_expedition(&state);
    assert_eq!(trip.vehicles[0].worn_hours, ratio(0, 1));
    assert_eq!(trip.supplies[hardwood], 1);
}
