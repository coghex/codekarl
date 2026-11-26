//! CK-431 (trade part B): vehicles and jobs.
//!
//! The cases are the spec's coverage for B (section 12): a build from a given order, the timed build and its ownership; hired hands
//! paid in goods at the start and fed that noon; an animal got by a swap and the order of wants; a given-up job's animal; hands held;
//! two jobs competing for a hand a build frees at noon, and saves. Each runs on the swap tests' fixture place and its rules (cattle the
//! only herd, a standard level of 7 of 10). Nothing sleeps and nothing reads a counter added for it.

use std::path::Path;

use ck_sim::{Canonical, GameTime, Reader};

use super::swap_tests::{
    MONK, PEASANT, THIRD, add_building, capacity, cattle_farm, good, head, herd_rules, place,
    set_price, settle_the_rest, spare, stock, want,
};
use super::*;

/// A date in March 768 at an hour.
fn at(day: u8, hour: u8) -> Date {
    Date {
        year: 768,
        month: 3,
        day,
        hour,
    }
}

fn hours(date: Date) -> u64 {
    GameTime::from_date(date).unwrap().hours()
}

fn state_of(place: PlaceState) -> EconomyState {
    EconomyState {
        places: vec![place],
        trade: TradeCounters::default(),
    }
}

fn wood(rules: &Rules) -> usize {
    good(rules, "hardwood")
}

/// Every pop but `only` has no workers at home, so no job can hire any of theirs.
fn only_workers(place: &mut PlaceState, only: usize, workers: u64) {
    for (index, pop) in place.pops.iter_mut().enumerate() {
        pop.workers = if index == only { workers } else { 0 };
    }
}

/// A farm of no levels in which `pop` works with `workers`, whose wage history makes its posted wage 6,000 (6 value units for each head
/// for a month), so one head for one day is worth 201.
fn farm_workers(rules: &Rules, place: &mut PlaceState, pop: usize, workers: u64) -> usize {
    let farm = add_building(rules, place, "farm", 0);
    let employment = Employment {
        jobs: if workers == 0 {
            BTreeMap::new()
        } else {
            BTreeMap::from([("peasant_labour".into(), workers)])
        },
        workers,
    };
    place.buildings[farm].employment[pop] = employment.clone();
    place.pops[pop].employment[farm] = employment;
    place.buildings[farm].wage_history = vec![6000; WAGE_HISTORY_MONTHS];
    farm
}

fn kind(rules: &Rules, id: &str) -> u32 {
    u32::try_from(rules.vehicles.iter().position(|v| v.id == id).unwrap()).unwrap()
}

#[test]
fn the_shipped_vehicles_are_one_entry_for_each_kind_with_every_field_of_the_table() {
    let rules = herd_rules();
    let ids: Vec<&str> = rules.vehicles.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "hand_cart",
            "ox_cart",
            "covered_wagon",
            "simple_barge",
            "currach"
        ]
    );
    let row = |id: &str| rules.vehicles.iter().find(|v| v.id == id).unwrap();
    let cart = row("hand_cart");
    assert_eq!(
        (
            cart.build_hours,
            cart.hands,
            cart.carries_g,
            cart.carries_ml
        ),
        (4, 1, 150_000, 500_000)
    );
    assert_eq!(
        (cart.carried_g, cart.carried_ml),
        (Some(50_000), Some(600_000))
    );
    assert_eq!(
        (cart.crew_full, cart.crew_fewest, cart.land_pace_pm),
        (1, 1, Some(1000))
    );
    let ox = row("ox_cart");
    assert_eq!(
        (ox.build_hours, ox.hands, ox.land_pace_pm),
        (8, 2, Some(800))
    );
    assert_eq!(ox.animals[0].species, "cattle");
    assert_eq!(
        (ox.carried_g, ox.carried_ml, ox.carries_g),
        (Some(400_000), Some(3_000_000), 500_000)
    );
    let wagon = row("covered_wagon");
    assert_eq!(wagon.land_pace_pm, Some(1300));
    assert_eq!(wagon.animals[0].species, "horses");
    let barge = row("simple_barge");
    assert_eq!(
        (barge.carried_g, barge.land_pace_pm, barge.water.as_deref()),
        (None, None, Some("barge"))
    );
    assert_eq!((barge.hands, barge.crew_full, barge.crew_fewest), (3, 3, 2));
    let currach = row("currach");
    assert_eq!((currach.build_hours, currach.crew_full), (48, 4));
    assert_eq!(
        (currach.carried_g, currach.carried_ml),
        (Some(150_000), Some(2_000_000))
    );
    assert_eq!(currach.water.as_deref(), Some("currach"));
    assert!(
        currach
            .cultures
            .iter()
            .all(|c| rules.peoples.culture(c).is_some())
    );
    // A kind with no way to move or be carried, or whose input names nothing, is refused.
    let mut bad = rules.clone();
    bad.vehicles[0].land_pace_pm = None;
    bad.vehicles[0].carried_g = None;
    bad.vehicles[0].carried_ml = None;
    assert!(bad.validate("fixture").is_err());
    let mut bad = rules.clone();
    bad.vehicles[0].inputs[0].category = Some("nothing_supplies_this".to_owned());
    assert!(bad.validate("fixture").is_err());
    let mut bad = rules.clone();
    bad.vehicles[1].animals[0].species = "dragons".to_owned();
    assert!(bad.validate("fixture").is_err());
}

#[test]
fn a_hand_cart_is_built_from_the_pops_own_wood_and_people_in_four_hours_and_it_owns_it() {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    place.pops[PEASANT].stock[wood(&rules)] = 2;
    let mut state = state_of(place);
    let now = hours(at(10, 5));
    let job = state
        .order_build(&rules, 0, PEASANT, "hand_cart", now)
        .unwrap();
    assert_eq!(job, 1);
    assert_eq!(
        state.places[0].pops[PEASANT].stock[wood(&rules)],
        0,
        "what the trader held is set aside at once"
    );
    assert_eq!(state.places[0].trade.jobs[0].stock[wood(&rules)], 2);
    assert!(state.places[0].trade.jobs[0].hands.is_none());
    // The job is ready and its trader's own people are at home: it starts that hour, unpaid.
    run_trade_hour(&rules, &mut state, at(10, 5)).unwrap();
    let hands = state.places[0].trade.jobs[0].hands.clone().unwrap();
    assert_eq!(
        (hands.pop as usize, hands.count, hands.started, hands.done),
        (PEASANT, 1, now, now + 4)
    );
    assert!(hands.paid.is_empty(), "a pop's own people are not paid");
    assert_eq!(state.places[0].trade.held(PEASANT), 1);
    // Three hours on it is not done; at the fourth hour it is, before anything else that hour.
    run_trade_hour(&rules, &mut state, at(10, 8)).unwrap();
    assert!(state.places[0].trade.vehicles.is_empty());
    run_trade_hour(&rules, &mut state, at(10, 9)).unwrap();
    assert_eq!(
        state.places[0].trade.vehicles,
        [Vehicle {
            number: 1,
            kind: kind(&rules, "hand_cart"),
            pop: u32::try_from(PEASANT).unwrap(),
            free: true,
        }]
    );
    assert!(state.places[0].trade.jobs.is_empty(), "the job ends");
    assert_eq!(
        state.places[0].trade.held(PEASANT),
        0,
        "the hands are released"
    );
    assert_eq!(
        state.trade,
        TradeCounters {
            vehicles: 1,
            jobs: 1,
            expeditions: 0
        }
    );
    assert_eq!(
        state.places[0].pops[PEASANT].stock[wood(&rules)],
        0,
        "the inputs are gone"
    );
}

#[test]
fn a_build_names_its_culture_and_its_category_fills_from_the_cheapest_good() {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    let mut state = state_of(place.clone());
    // The currach is for Irish pops: the fixture's pop is of culture `gaelic`, which is none of the named cultures.
    assert!(state.order_build(&rules, 0, PEASANT, "currach", 0).is_err());
    assert!(
        state.places[0].trade.jobs.is_empty(),
        "refused, nothing changed"
    );
    place.pops[PEASANT].culture = "ulaid".to_owned();
    let mut state = state_of(place.clone());
    state.order_build(&rules, 0, PEASANT, "currach", 0).unwrap();
    let job = &state.places[0].trade.jobs[0];
    assert_eq!(job.need_goods[wood(&rules)], 4);
    assert_eq!(job.need_goods[good(&rules, "hides")], 4);
    // Eight fabric from the cheapest good at the place: one hide (200) costs less than eight pounds of hemp (240) or of linen.
    place.pops[PEASANT].culture = "gaelic".to_owned();
    let mut state = state_of(place.clone());
    state
        .order_build(&rules, 0, PEASANT, "covered_wagon", 0)
        .unwrap();
    let job = &state.places[0].trade.jobs[0];
    assert_eq!(job.need_goods[good(&rules, "hides")], 1);
    assert_eq!(job.need_goods[good(&rules, "hemp")], 0);
    assert_eq!(job.need_goods[wood(&rules)], 6);
    assert_eq!(
        job.need_animals.iter().sum::<u64>(),
        1,
        "and a horse, whatever the species' place in the rules"
    );
    // Where a hide costs more, hemp fills it: eight pounds.
    place.prices_milli[good(&rules, "hides")] = 1_000_000;
    let mut state = state_of(place);
    state
        .order_build(&rules, 0, PEASANT, "covered_wagon", 0)
        .unwrap();
    let job = &state.places[0].trade.jobs[0];
    assert_eq!(job.need_goods[good(&rules, "hemp")], 8);
    assert_eq!(job.need_goods[good(&rules, "hides")], 0);
}

#[test]
fn hired_hands_are_paid_in_goods_when_the_job_starts_and_eat_that_noon() {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    set_price(&rules, &mut place, "oats", 1);
    settle_the_rest(&rules, &mut place, &["oats", "hardwood"]);
    only_workers(&mut place, MONK, 5);
    let farm = farm_workers(&rules, &mut place, MONK, 5);
    place.pops[MONK].stock[good(&rules, "oats")] = 0;
    spare(&rules, &mut place, PEASANT, "oats", 5);
    place.pops[PEASANT].stock[wood(&rules)] = 2;
    let paid_before = stock(&rules, &place, PEASANT, "oats");
    let mut state = state_of(place);
    state
        .order_build(&rules, 0, PEASANT, "hand_cart", hours(at(14, 8)))
        .unwrap();
    // Before noon nothing has started on its own accord at 08:00 but the hand is hired by the hourly phase at once; this case wants
    // the noon's step, so no hourly run comes first.
    run_daily_economy(&rules, &mut state, at(14, 12)).unwrap();
    let hands = state.places[0].trade.jobs[0].hands.clone().unwrap();
    assert_eq!(hands.pop as usize, MONK, "the only pop with a hand at home");
    assert_eq!(
        hands.started,
        hours(at(14, 12)),
        "it starts at the noon step"
    );
    assert_eq!(
        hands.paid,
        [(u32::try_from(good(&rules, "oats")).unwrap(), 1)],
        "the fewest whole units worth the wage (201 of a 1,000 bushel), of the good the hired pop most lacks"
    );
    assert!(
        stock(&rules, &state.places[0], PEASANT, "oats") < paid_before,
        "the payer gave it"
    );
    assert_eq!(
        stock(&rules, &state.places[0], MONK, "oats"),
        0,
        "and the hired pop ate it that noon: the start came before the day's eating"
    );
    assert_eq!(
        state.places[0]
            .trade
            .days_away
            .get(&(u32::try_from(farm).unwrap(), 1)),
        Some(&1),
        "the noon spent counts as a day away from the hand's other work"
    );
}

#[test]
fn a_job_that_cannot_pay_does_not_start() {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    set_price(&rules, &mut place, "oats", 1);
    settle_the_rest(&rules, &mut place, &["oats", "hardwood"]);
    only_workers(&mut place, MONK, 5);
    farm_workers(&rules, &mut place, MONK, 5);
    place.pops[MONK].stock[good(&rules, "oats")] = 0;
    // The trader holds the wood and no goods at all to pay with.
    place.pops[PEASANT].stock.fill(0);
    place.pops[PEASANT].stock[wood(&rules)] = 2;
    let mut state = state_of(place);
    state
        .order_build(&rules, 0, PEASANT, "hand_cart", hours(at(1, 0)))
        .unwrap();
    run_trade_hour(&rules, &mut state, at(1, 0)).unwrap();
    assert!(state.places[0].trade.jobs[0].hands.is_none());
    assert_eq!(state.places[0].trade.held(MONK), 0, "nobody is held");
}

#[test]
fn an_ox_cart_gets_its_cattle_by_a_swap_and_an_older_job_outranks_two_herds_below_standard() {
    let rules = herd_rules();
    let mut rules = rules;
    let mut place = place(&rules, 3);
    set_price(&rules, &mut place, "cattle", 24);
    set_price(&rules, &mut place, "oats", 1);
    settle_the_rest(&rules, &mut place, &["oats"]);
    spare(&rules, &mut place, THIRD, "oats", 1000);
    want(&rules, &mut place, MONK, "oats", 100);
    capacity(&mut rules, &mut place, THIRD, 100_000_000);
    let seller = cattle_farm(&rules, &mut place, 8, &[(MONK, 1)]);
    // The buyer's two herds are below standard: shortfalls 3 and 2.
    let first = cattle_farm(&rules, &mut place, 4, &[(THIRD, 1)]);
    let second = cattle_farm(&rules, &mut place, 5, &[(THIRD, 1)]);
    place.pops[THIRD].stock[wood(&rules)] = 4;
    let mut state = state_of(place);
    state.order_build(&rules, 0, THIRD, "ox_cart", 0).unwrap();
    let cattle = good(&rules, "cattle");
    let job = &state.places[0].trade.jobs[0];
    assert_eq!(job.stock[wood(&rules)], 4, "the wood was set aside at once");
    assert_eq!(job.animal_lack(0), 1);
    // Its lack and the herds' shortfalls are separate wants of the trader's: 1 + 3 + 2, all in the place's buy orders.
    let orders = daily_market_orders(&rules, &state.places[0]).unwrap();
    assert_eq!(
        (orders.buy_units[cattle], orders.sell_units[cattle]),
        (6, 1)
    );
    swap::run_daily_swaps(&rules, &mut state).unwrap();
    let place = &state.places[0];
    assert_eq!(head(place, seller), 7, "one head left the seller's offer");
    assert_eq!(place.trade.jobs[0].animals[0], 1, "the older job took it");
    assert_eq!(
        (head(place, first), head(place, second)),
        (4, 5),
        "neither herd below standard did"
    );
    assert!(place.trade.jobs[0].ready());
    assert_eq!(place.pops[THIRD].stock[cattle], 0, "never in a store");
}

#[test]
fn a_given_up_jobs_animal_goes_back_to_a_herd_below_its_standard_and_its_goods_to_its_trader() {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    let farm = cattle_farm(&rules, &mut place, 4, &[(THIRD, 1)]);
    place.pops[THIRD].stock[wood(&rules)] = 3;
    let mut state = state_of(place);
    let made = hours(at(2, 0));
    state
        .order_build(&rules, 0, THIRD, "ox_cart", made)
        .unwrap();
    // The job holds three of its four wood and, as if by a swap, one head.
    state.places[0].trade.jobs[0].animals[0] = 1;
    assert_eq!(state.places[0].pops[THIRD].stock[wood(&rules)], 0);
    let total_wood = |state: &EconomyState| {
        state.places[0].pops[THIRD].stock[wood(&rules)]
            + state.places[0]
                .trade
                .jobs
                .iter()
                .map(|j| j.stock[wood(&rules)])
                .sum::<u64>()
    };
    // Just short of the set time nothing is given up; at it the job is.
    let giveup = made + rules.job_giveup_hours;
    let date = |hour: u64| GameTime::from_hours(hour).date().unwrap();
    run_trade_hour(&rules, &mut state, date(giveup - 1)).unwrap();
    assert!(state.places[0].trade.jobs[0].given_up.is_none());
    run_trade_hour(&rules, &mut state, date(giveup)).unwrap();
    let place = &state.places[0];
    assert_eq!(
        head(place, farm),
        5,
        "the head went back to the herd below its standard"
    );
    assert!(
        place.trade.jobs.is_empty(),
        "and with its goods returned the job ended"
    );
    assert_eq!(
        place.pops[THIRD].stock[wood(&rules)],
        3,
        "the wood went back, none destroyed"
    );
    assert_eq!(total_wood(&state), 3);
}

#[test]
fn a_given_up_jobs_animal_with_no_herd_to_take_it_is_offered_and_then_slaughtered_to_its_trader() {
    let rules = herd_rules();
    let place = place(&rules, 3);
    // The trader owns no herd for the head to go back to.
    let mut state = state_of(place);
    let made = hours(at(2, 0));
    state
        .order_build(&rules, 0, THIRD, "ox_cart", made)
        .unwrap();
    state.places[0].trade.jobs[0].animals[0] = 1;
    let date = |hour: u64| GameTime::from_hours(hour).date().unwrap();
    let given_up = made + rules.job_giveup_hours;
    run_trade_hour(&rules, &mut state, date(given_up)).unwrap();
    let place = &state.places[0];
    assert_eq!(place.trade.jobs.len(), 1, "the animal stays with the job");
    assert_eq!(place.trade.offered(THIRD, 0), 1, "on offer at the place");
    let cattle = good(&rules, "cattle");
    let orders = daily_market_orders(&rules, place).unwrap();
    assert_eq!(orders.sell_units[cattle], 1);
    // Nobody takes it: just short of the set time it is still there; at it, it is slaughtered, its meat and hide to the trader.
    let slaughter = given_up + rules.held_animal_hours;
    run_trade_hour(&rules, &mut state, date(slaughter - 1)).unwrap();
    assert_eq!(state.places[0].trade.offered(THIRD, 0), 1);
    run_trade_hour(&rules, &mut state, date(slaughter)).unwrap();
    let place = &state.places[0];
    assert!(place.trade.jobs.is_empty());
    assert_eq!(place.trade.offered(THIRD, 0), 0);
    assert_eq!(place.pops[THIRD].stock[good(&rules, "beef")], 220);
    assert_eq!(place.pops[THIRD].stock[good(&rules, "hides")], 1);
}

#[test]
fn an_animal_on_offer_is_sold_in_the_exchange_like_a_herds() {
    let mut rules = herd_rules();
    let mut place = place(&rules, 3);
    set_price(&rules, &mut place, "cattle", 24);
    set_price(&rules, &mut place, "oats", 1);
    settle_the_rest(&rules, &mut place, &["oats"]);
    spare(&rules, &mut place, PEASANT, "oats", 1000);
    capacity(&mut rules, &mut place, PEASANT, 100_000_000);
    // THIRD's job was given up holding a head the trader owns no herd for; MONK wants a head for its herd and has oats to spare.
    let buyer = cattle_farm(&rules, &mut place, 4, &[(PEASANT, 1)]);
    place.pops[THIRD].stock[good(&rules, "oats")] = 0;
    want(&rules, &mut place, THIRD, "oats", 100);
    let mut state = state_of(place);
    state.order_build(&rules, 0, THIRD, "ox_cart", 0).unwrap();
    state.places[0].trade.jobs[0].animals[0] = 1;
    let jobs = &mut state.places[0].trade.jobs;
    jobs[0].given_up = Some(0);
    jobs[0].need_goods.fill(0);
    jobs[0].need_animals.fill(0);
    swap::run_daily_swaps(&rules, &mut state).unwrap();
    let place = &state.places[0];
    assert_eq!(
        head(place, buyer),
        5,
        "the offered head joined the buyer's herd"
    );
    assert_eq!(place.trade.offered(THIRD, 0), 0, "and left the job");
    assert!(
        stock(&rules, place, THIRD, "oats") > 0,
        "paid for into the seller's store"
    );
}

#[test]
fn a_hand_is_held_for_a_build_and_another_job_waits_for_it() {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    only_workers(&mut place, PEASANT, 1);
    place.pops[PEASANT].stock[wood(&rules)] = 4;
    let mut state = state_of(place);
    let now = hours(at(5, 6));
    for _ in 0..2 {
        state
            .order_build(&rules, 0, PEASANT, "hand_cart", now)
            .unwrap();
    }
    run_trade_hour(&rules, &mut state, at(5, 6)).unwrap();
    let jobs = &state.places[0].trade.jobs;
    assert!(jobs[0].hands.is_some(), "the older job starts");
    assert!(jobs[1].hands.is_none(), "the one hand at home is held");
    run_trade_hour(&rules, &mut state, at(5, 9)).unwrap();
    assert!(state.places[0].trade.jobs[1].hands.is_none());
    // The hour the first is done its hand is free and the second starts: completion comes before starts in the hour.
    run_trade_hour(&rules, &mut state, at(5, 10)).unwrap();
    assert_eq!(state.places[0].trade.vehicles.len(), 1);
    let second = state.places[0].trade.jobs[0].hands.clone().unwrap();
    assert_eq!(second.started, hours(at(5, 10)));
}

/// Two traders' jobs and a build that frees the one hand at home at noon of `day` of March: the place of the case.
fn competing_hands(day: u8) -> (Rules, EconomyState) {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    set_price(&rules, &mut place, "oats", 1);
    settle_the_rest(&rules, &mut place, &["oats", "hardwood"]);
    only_workers(&mut place, PEASANT, 1);
    farm_workers(&rules, &mut place, PEASANT, 1);
    // The peasants' store is empty, so it has room for the wage whatever the day's eating and payday leave of its people.
    place.pops[PEASANT].stock.fill(0);
    for pop in [PEASANT, MONK, THIRD] {
        place.pops[pop].stock[wood(&rules)] = 2;
    }
    for pop in [MONK, THIRD] {
        spare(&rules, &mut place, pop, "oats", 5);
    }
    let mut state = state_of(place);
    let start = hours(at(day, 8));
    for pop in [PEASANT, MONK, THIRD] {
        state
            .order_build(&rules, 0, pop, "hand_cart", start)
            .unwrap();
    }
    (rules, state)
}

#[test]
fn two_jobs_compete_for_the_hand_a_build_frees_at_noon_and_the_lower_number_starts() {
    let (rules, mut state) = competing_hands(1);
    // 08:00: the peasants' own build starts with the one hand at home; the other two jobs are ready and have none to hire.
    run_trade_hour(&rules, &mut state, at(1, 8)).unwrap();
    let place = &state.places[0];
    assert!(place.trade.jobs[0].hands.is_some());
    assert!(place.trade.jobs[1].hands.is_none() && place.trade.jobs[2].hands.is_none());
    // 12:00, the daily step first: the build is not done in the daily phase, so the hand is still held and nobody starts; the noon
    // counts a day away for it.
    run_daily_economy(&rules, &mut state, at(1, 12)).unwrap();
    let place = &state.places[0];
    assert_eq!(place.trade.jobs.len(), 3);
    assert!(place.trade.jobs[1].hands.is_none() && place.trade.jobs[2].hands.is_none());
    assert_eq!(place.trade.days_away.values().sum::<u64>(), 1);
    // Then the hourly phase: the build completes, and of the two ready jobs the lower number starts, paying its wage; the other keeps
    // gathering. A hand the build released was counted at this noon already; the start counts only hands beyond those (11c.1).
    // What the hired pop lacks most of, reckoned as the hourly phase begins: the biggest deficit against its target, the goods' order
    // between equals; the wage is paid in it.
    let lacks_most = {
        let place = &state.places[0];
        let people = place.pops[PEASANT].workers + place.pops[PEASANT].dependents;
        (0..rules.goods.len())
            .filter(|good| rules.goods[*good].tags.iter().all(|tag| tag != "herd"))
            .map(|good| {
                let target = pop_target_units(&rules, "peasant", people, good).unwrap();
                let held = pop_market_holding(place, PEASANT, good).unwrap();
                (target.saturating_sub(held), good)
            })
            .max_by(|(a, left), (b, right)| a.cmp(b).then(right.cmp(left)))
            .unwrap()
            .1
    };
    run_trade_hour(&rules, &mut state, at(1, 12)).unwrap();
    let place = &state.places[0];
    assert_eq!(place.trade.vehicles.len(), 1);
    assert_eq!(place.trade.jobs.len(), 2);
    let (winner, loser) = (&place.trade.jobs[0], &place.trade.jobs[1]);
    assert_eq!((winner.number, loser.number), (2, 3));
    let hands = winner.hands.clone().unwrap();
    assert_eq!(
        hands.pop as usize, PEASANT,
        "the peasants' hand is the one hired"
    );
    assert_eq!(hands.paid.len(), 1, "its wage, paid in goods");
    assert_eq!(hands.paid[0].1, 1, "the fewest whole units worth it");
    assert_eq!(
        hands.paid[0].0 as usize, lacks_most,
        "of the good the hired pop lacked most of when the job started"
    );
    assert!(loser.hands.is_none(), "the other is still gathering");
    assert_eq!(
        place.trade.days_away.values().sum::<u64>(),
        1,
        "the one hand freed and hired again that noon is away one day, not two"
    );
}

#[test]
fn a_game_saved_in_the_middle_of_a_build_loads_and_goes_on_as_the_unbroken_game_would() {
    let (rules, mut state) = competing_hands(14);
    run_trade_hour(&rules, &mut state, at(14, 8)).unwrap();
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let mut reader = Reader::new(&bytes);
    let mut loaded = EconomyState::decode(&mut reader).unwrap();
    assert_eq!(loaded, state, "what a save holds is the state");
    // Both games go through the noon of the 14th (its daily step, then its hourly phase), every later noon of the month and the payday
    // of the 1st of April.
    let april = Date {
        month: 4,
        day: 1,
        ..at(14, 12)
    };
    let noons = (14..=31).map(|day| at(day, 12)).chain([april]);
    for date in noons {
        for game in [&mut state, &mut loaded] {
            run_daily_economy(&rules, game, date).unwrap();
            run_trade_hour(&rules, game, date).unwrap();
        }
    }
    assert_eq!(loaded, state);
    assert_eq!(
        state.places[0].trade.vehicles.len(),
        3,
        "each of the three builds went through, one after another, on the one hand"
    );
    assert!(
        state.places[0].trade.days_away.is_empty(),
        "the payday counted the days away and cleared them"
    );
}

#[test]
fn days_away_count_a_worker_as_that_much_less_of_a_worker_at_the_payday() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml");
    let mut rules = Rules::load(path).unwrap();
    tests::flatten_pasture(&mut rules);
    let mut base = tests::shipped_production_fixture(&rules);
    base.places[0].price_drift_carries = vec![0; rules.goods.len()];
    base.places[0].prices_milli = rules
        .goods
        .iter()
        .map(|good| good.base_price * 1000)
        .collect();
    for pop in &mut base.places[0].pops {
        pop.workers = 100;
        pop.consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
        pop.consumption.ticking_carries = vec![0; rules.goods.len()];
    }
    // The first building has ten positions, filled by two pops' five workers each: a worker away is a worker less of its output.
    let positions = {
        let farm = &base.places[0].buildings[0];
        let definition = rules.buildings.iter().find(|b| b.id == farm.kind).unwrap();
        u64::from(farm.levels) * ordinary_places_per_level(definition)
    };
    assert_eq!(positions, 10);
    for pop in 0..2 {
        let employment = Employment {
            jobs: BTreeMap::from([("peasant_labour".into(), 5)]),
            workers: 5,
        };
        base.places[0].buildings[0].employment[pop] = employment.clone();
        base.places[0].pops[pop].employment[0] = employment;
    }
    // The first of October pays the 30 days of September, a harvest month.
    let date = Date {
        year: 768,
        month: 10,
        day: 1,
        hour: 12,
    };
    let run = |days_away: Option<u64>| {
        let mut state = base.clone();
        if let Some(days) = days_away {
            state.places[0].trade.days_away.insert((0, 0), days);
        }
        let ledger = run_daily_economy(&rules, &mut state, date).unwrap();
        (state, ledger)
    };
    let (whole, whole_ledger) = run(None);
    // The first pop's five workers were away fifteen of the thirty days: two and a half workers' worth, of ten.
    let (half, half_ledger) = run(Some(5 * 15));
    // All of them were away all of the thirty days: five of the ten workers are left.
    let (full, full_ledger) = run(Some(5 * 30));
    for state in [&half, &full] {
        assert!(
            state.places[0].trade.days_away.is_empty(),
            "the payday cleared them"
        );
    }
    let farm_output = |ledger: &ProductionLedger| -> Vec<u64> { ledger.produced.clone() };
    let (w, h, f) = (
        farm_output(&whole_ledger),
        farm_output(&half_ledger),
        farm_output(&full_ledger),
    );
    for good in 0..rules.goods.len() {
        // What the farm makes follows its staffing (ten workers, seven and a half, five), and the rest of what the place makes does
        // not: every good falls with the workers away, and by as much for the second step as the first, each a unit short at most.
        assert!(w[good] >= h[good] && h[good] >= f[good], "good {good}");
        let (lost_half, lost_full) = (w[good] - h[good], w[good] - f[good]);
        assert!(
            (lost_half * 2).abs_diff(lost_full) <= 1,
            "good {good}: lost {lost_half} of {} at seven and a half workers and {lost_full} at five",
            w[good]
        );
    }
    assert!(w[good(&rules, "vegetables")] > f[good(&rules, "vegetables")]);
    assert!(
        w.iter().any(|units| *units > 20),
        "the case produces something"
    );
    // The wage is split by the workers each pop has, less its days away: with the first pop's five away all month it gets none of the
    // wage and the second all of it.
    let oats = good(&rules, "vegetables");
    let first = |state: &EconomyState| state.places[0].pops[0].stock[oats];
    let second = |state: &EconomyState| state.places[0].pops[1].stock[oats];
    assert!(first(&whole) > first(&half) && first(&half) > first(&full));
    assert!(
        second(&full) >= second(&whole),
        "the second pop's share is of the workers left, so it does not fall with the output"
    );
}

#[test]
fn an_economy_saved_before_the_vehicles_and_jobs_slice_does_not_decode() {
    let rules = herd_rules();
    let state = state_of(place(&rules, 3));
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    // The counters of vehicles and jobs end the encoding, and the place's trade state ends each place: an older encoding has neither.
    let older = &bytes[..bytes.len() - 16];
    assert!(EconomyState::decode(&mut Reader::new(older)).is_err());
    assert_eq!(
        EconomyState::decode(&mut Reader::new(&bytes)).unwrap(),
        state
    );
}

#[test]
fn a_game_saved_before_the_noon_of_the_first_follows_the_unbroken_game_with_one_day_away() {
    let (rules, mut state) = competing_hands(1);
    run_trade_hour(&rules, &mut state, at(1, 8)).unwrap();
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let mut loaded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    assert_eq!(loaded, state);
    for game in [&mut state, &mut loaded] {
        run_daily_economy(&rules, game, at(1, 12)).unwrap();
        run_trade_hour(&rules, game, at(1, 12)).unwrap();
    }
    assert_eq!(loaded, state);
    let place = &state.places[0];
    assert_eq!(
        place
            .trade
            .jobs
            .iter()
            .map(|job| (job.number, job.hands.is_some()))
            .collect::<Vec<_>>(),
        [(2, true), (3, false)],
        "the lower number starts, the other is still gathering"
    );
    assert_eq!(place.trade.jobs[0].hands.as_ref().unwrap().paid.len(), 1);
    assert_eq!(place.trade.days_away.values().sum::<u64>(), 1);
}

#[test]
fn two_builds_held_at_once_charge_their_hands_to_different_positions_of_the_pop() {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    // The peasants have two workers, one in each of two farms; the other pops have none at home.
    only_workers(&mut place, PEASANT, 2);
    let first = farm_workers(&rules, &mut place, PEASANT, 1);
    let second = farm_workers(&rules, &mut place, PEASANT, 1);
    place.pops[PEASANT].stock[wood(&rules)] = 4;
    let mut state = state_of(place);
    for _ in 0..2 {
        state
            .order_build(&rules, 0, PEASANT, "hand_cart", hours(at(14, 8)))
            .unwrap();
    }
    run_trade_hour(&rules, &mut state, at(14, 8)).unwrap();
    let places = &state.places[0];
    let by_building: Vec<_> = places
        .trade
        .jobs
        .iter()
        .map(|job| job.hands.clone().unwrap().by_building)
        .collect();
    let (a, b) = (
        u32::try_from(first).unwrap(),
        u32::try_from(second).unwrap(),
    );
    assert_eq!(by_building, [vec![(a, 1)], vec![(b, 1)]]);
    run_daily_economy(&rules, &mut state, at(14, 12)).unwrap();
    let away = &state.places[0].trade.days_away;
    assert_eq!(away.get(&(a, 0)), Some(&1));
    assert_eq!(away.get(&(b, 0)), Some(&1));
    assert_eq!(away.len(), 2, "one absence against each farm");
}

/// A buyer whose job lacks four hides, with a store that takes three of them, and a seller of four hides for oats at equal prices:
/// `starter` is the pop that has the day's capacity and so starts the swap.
fn hides_for_a_job(starter: usize) -> (Rules, EconomyState) {
    let mut rules = herd_rules();
    let hides = good(&rules, "hides");
    rules.goods[hides].pop_limit_milli = 1000;
    rules.validate("fixture").unwrap();
    let mut place = place(&rules, 3);
    // Three people each: a store of three hides at most, and room for little else.
    for pop in &mut place.pops {
        pop.workers = 3;
        pop.dependents = 0;
    }
    set_price(&rules, &mut place, "hides", 1);
    set_price(&rules, &mut place, "oats", 1);
    settle_the_rest(&rules, &mut place, &["oats", "hides", "hardwood"]);
    for pop in &mut place.pops {
        pop.stock[hides] = 0;
    }
    place.pops[THIRD].culture = "ulaid".to_owned();
    // The buyer (THIRD) has wood for the job and oats to spare; the seller (MONK) has four hides and wants oats.
    place.pops[THIRD].stock[wood(&rules)] = 4;
    spare(&rules, &mut place, THIRD, "oats", 100);
    want(&rules, &mut place, MONK, "oats", 4);
    place.pops[MONK].stock[hides] = 4;
    capacity(&mut rules, &mut place, starter, 100_000_000);
    let mut state = state_of(place);
    state.order_build(&rules, 0, THIRD, "currach", 0).unwrap();
    (rules, state)
}

#[test]
fn a_jobs_swap_receipt_is_not_capped_by_its_traders_store_whichever_pop_starts_the_swap() {
    for starter in [THIRD, MONK] {
        let (rules, mut state) = hides_for_a_job(starter);
        let hides = good(&rules, "hides");
        assert_eq!(state.places[0].trade.jobs[0].lack(hides), 4);
        swap::run_daily_swaps(&rules, &mut state).unwrap();
        let place = &state.places[0];
        assert_eq!(
            place.trade.jobs[0].stock[hides], 4,
            "all four hides reached the job when pop {starter} started"
        );
        assert_eq!(
            place.pops[THIRD].stock[hides], 0,
            "none went through the store"
        );
        assert_eq!(place.pops[MONK].stock[hides], 0);
    }
}

#[test]
fn a_saved_trade_state_that_names_what_does_not_exist_is_refused() {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    place.pops[PEASANT].stock[wood(&rules)] = 2;
    let mut state = state_of(place);
    state
        .order_build(&rules, 0, PEASANT, "hand_cart", 0)
        .unwrap();
    validate_trade_state(&rules, &state).unwrap();
    let broken = |change: &dyn Fn(&mut EconomyState)| {
        let mut bad = state.clone();
        change(&mut bad);
        // The save decodes whatever it holds: only the validator refuses it.
        let mut bytes = Vec::new();
        bad.encode(&mut bytes).unwrap();
        let loaded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
        validate_trade_state(&rules, &loaded).is_err()
    };
    let kinds = u32::try_from(rules.vehicles.len()).unwrap();
    assert!(broken(
        &|s| s.places[0].trade.jobs[0].kind = JobKind::Build { vehicle: kinds }
    ));
    assert!(broken(&|s| s.places[0].trade.jobs[0].pop = 99));
    assert!(broken(&|s| s.places[0].trade.jobs[0]
        .stock
        .pop()
        .map(drop)
        .unwrap_or_default()));
    assert!(broken(&|s| s.places[0].trade.jobs[0].number = 0));
    assert!(broken(&|s| s.trade.jobs = 0));
    assert!(broken(&|s| {
        s.places[0].trade.jobs[0].hands = Some(Hands {
            pop: 0,
            count: 1,
            by_building: vec![(99, 1)],
            started: 0,
            done: 4,
            paid: Vec::new(),
        });
    }));
    assert!(broken(&|s| {
        s.places[0].trade.vehicles.push(Vehicle {
            number: 1,
            kind: kinds,
            pop: 0,
            free: true,
        });
    }));
    assert!(broken(&|s| {
        s.places[0].trade.days_away.insert((9, 0), 1);
    }));
}

#[test]
fn a_crew_of_a_rehired_hand_and_a_new_one_is_counted_against_each_ones_own_farm() {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    // The peasants have two workers, one in each of two farms; nobody else has any.
    only_workers(&mut place, PEASANT, 2);
    let a = farm_workers(&rules, &mut place, PEASANT, 1);
    let b = farm_workers(&rules, &mut place, PEASANT, 1);
    place.pops[PEASANT].stock[wood(&rules)] = 8;
    let mut state = state_of(place);
    let order = |state: &mut EconomyState, vehicle: &str, hour: u8| {
        state
            .order_build(&rules, 0, PEASANT, vehicle, hours(at(14, hour)))
            .unwrap()
    };
    // 07:00 a hand cart holds the first farm's worker until 11:00; 08:00 another holds the second farm's until noon.
    order(&mut state, "hand_cart", 7);
    run_trade_hour(&rules, &mut state, at(14, 7)).unwrap();
    order(&mut state, "hand_cart", 8);
    run_trade_hour(&rules, &mut state, at(14, 8)).unwrap();
    // An ox-cart of two hands, its cattle already got, waits for both.
    let ox = order(&mut state, "ox_cart", 8);
    state.places[0].trade.jobs[2].animals[0] = 1;
    assert_eq!(ox, 3);
    for hour in 9..=11 {
        run_trade_hour(&rules, &mut state, at(14, hour)).unwrap();
    }
    assert!(
        state.places[0]
            .trade
            .jobs
            .iter()
            .any(|job| job.number == 3 && job.hands.is_none())
    );
    // At noon the daily phase counts the second cart's hand at its farm; then the cart is done and the ox-cart starts with the rehired
    // hand and the one the first cart released at 11:00.
    run_daily_economy(&rules, &mut state, at(14, 12)).unwrap();
    run_trade_hour(&rules, &mut state, at(14, 12)).unwrap();
    let place = &state.places[0];
    let started = place.trade.jobs.iter().find(|job| job.number == 3).unwrap();
    assert_eq!(
        started.hands.as_ref().unwrap().by_building,
        [
            (u32::try_from(a).unwrap(), 1),
            (u32::try_from(b).unwrap(), 1)
        ]
    );
    let away = &place.trade.days_away;
    assert_eq!(away.get(&(u32::try_from(a).unwrap(), 0)), Some(&1));
    assert_eq!(away.get(&(u32::try_from(b).unwrap(), 0)), Some(&1));
    assert_eq!(away.len(), 2, "one person-day against each farm");
}

#[test]
fn saved_counts_and_times_that_overflow_are_refused_and_never_panic_the_tick() {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    place.pops[PEASANT].stock[wood(&rules)] = 2;
    let mut state = state_of(place);
    add_building(&rules, &mut state.places[0], "farm", 0);
    add_building(&rules, &mut state.places[0], "farm", 0);
    state
        .order_build(&rules, 0, PEASANT, "hand_cart", 0)
        .unwrap();
    let round_trip = |state: &EconomyState| {
        let mut bytes = Vec::new();
        state.encode(&mut bytes).unwrap();
        EconomyState::decode(&mut Reader::new(&bytes)).unwrap()
    };
    // A started job whose hands at two buildings add past the largest count.
    let mut sum = state.clone();
    sum.places[0].trade.jobs[0].hands = Some(Hands {
        pop: 0,
        count: u64::MAX,
        by_building: vec![(0, u64::MAX), (1, 1)],
        started: 0,
        done: 4,
        paid: Vec::new(),
    });
    let sum = round_trip(&sum);
    assert!(validate_trade_state(&rules, &sum).is_err());
    let mut tick = sum.clone();
    let _ = run_trade_hour(&rules, &mut tick, at(14, 12));
    // A waiting job made at an hour whose give-up deadline the clock cannot reach.
    let mut late = state.clone();
    late.places[0].trade.jobs[0].made = u64::MAX;
    let late = round_trip(&late);
    assert!(validate_trade_state(&rules, &late).is_err());
    let mut tick = late.clone();
    run_trade_hour(&rules, &mut tick, at(14, 12)).unwrap();
    let mut gone = state.clone();
    gone.places[0].trade.jobs[0].given_up = Some(u64::MAX);
    assert!(validate_trade_state(&rules, &round_trip(&gone)).is_err());
}

/// A valid saved trade state of every kind: two farms, a started build holding a hand at the first, a waiting job, a vehicle and the
/// days the started build's hand was away.
fn jobs_of(state: &mut EconomyState) -> &mut Vec<Job> {
    &mut state.places[0].trade.jobs
}

fn saved_trade_state() -> (Rules, EconomyState) {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    only_workers(&mut place, PEASANT, 1);
    farm_workers(&rules, &mut place, PEASANT, 1);
    farm_workers(&rules, &mut place, PEASANT, 1);
    place.pops[PEASANT].stock[wood(&rules)] = 4;
    let mut state = state_of(place);
    for _ in 0..2 {
        state
            .order_build(&rules, 0, PEASANT, "hand_cart", hours(at(14, 8)))
            .unwrap();
    }
    run_trade_hour(&rules, &mut state, at(14, 8)).unwrap();
    state.places[0].trade.vehicles.push(Vehicle {
        number: 1,
        kind: 0,
        pop: 0,
        free: true,
    });
    state.trade.vehicles = 1;
    trade::count_days_away(&mut state.places[0]);
    validate_trade_state(&rules, &state).unwrap();
    (rules, state)
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "One out-of-domain case for each saved trade field, beside the others, so a missing one is seen."
)]
fn every_saved_trade_field_is_checked_against_its_whole_domain() {
    let (rules, state) = saved_trade_state();
    let last = trade::last_hour();
    let refused = |change: &dyn Fn(&mut EconomyState)| {
        let mut bad = state.clone();
        change(&mut bad);
        let mut bytes = Vec::new();
        bad.encode(&mut bytes).unwrap();
        let loaded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
        validate_trade_state(&rules, &loaded).is_err()
    };
    // Counters: the next number must be issuable and above every number in use.
    assert!(refused(&|s| s.trade.vehicles = u64::MAX));
    assert!(refused(&|s| s.trade.jobs = u64::MAX));
    assert!(refused(&|s| s.trade.vehicles = 0));
    assert!(refused(&|s| s.trade.jobs = 1));
    // Numbers: unique across the economy, in range, increasing.
    assert!(refused(&|s| s.places[0].trade.vehicles.push(Vehicle {
        number: 1,
        kind: 0,
        pop: 0,
        free: true
    })));
    assert!(refused(&|s| jobs_of(s)[1].number = jobs_of(s)[0].number));
    assert!(refused(&|s| jobs_of(s)[0].number = 0));
    // Indices and dimensions.
    let kinds = u32::try_from(rules.vehicles.len()).unwrap();
    assert!(refused(
        &|s| jobs_of(s)[0].kind = JobKind::Build { vehicle: kinds }
    ));
    assert!(refused(&|s| jobs_of(s)[0].pop = 99));
    assert!(refused(&|s| s.places[0].trade.vehicles[0].kind = kinds));
    assert!(refused(&|s| s.places[0].trade.vehicles[0].pop = 99));
    assert!(refused(&|s| {
        jobs_of(s)[0].need_goods.pop();
    }));
    // What a build needs is what its kind is built with; a job holds no more than it needs.
    assert!(refused(&|s| jobs_of(s)[1].need_animals[0] = 1));
    assert!(refused(&|s| jobs_of(s)[1].stock[0] = 1));
    // Hands: the build's own count, in buildings that exist, ordered, each within what the pop works there, paid in real goods.
    let with_hands = |change: &dyn Fn(&mut Hands)| {
        refused(&|s| {
            let mut h = s.places[0].trade.jobs[0].hands.clone().unwrap();
            change(&mut h);
            s.places[0].trade.jobs[0].hands = Some(h);
        })
    };
    assert!(with_hands(&|h| h.count = 2));
    assert!(with_hands(&|h| h.pop = 99));
    assert!(with_hands(&|h| h.by_building = vec![(0, u64::MAX), (1, 1)]));
    assert!(with_hands(&|h| h.by_building = vec![(99, 1)]));
    assert!(with_hands(&|h| h.by_building = vec![(1, 1), (0, 1)]));
    assert!(with_hands(&|h| h.by_building = vec![(0, 2)]));
    assert!(with_hands(&|h| h.paid = vec![(99, 1)]));
    assert!(with_hands(&|h| h.paid = vec![(0, 0)]));
    assert!(with_hands(&|h| h.done += 1));
    assert!(with_hands(&|h| h.started = 0));
    assert!(with_hands(&|h| {
        h.started = last + 1;
        h.done = last + 5;
    }));
    assert!(refused(&|s| jobs_of(s)[0].given_up = Some(0)));
    // Hours: every saved hour is in the calendar. The first hour past it is refused apart from integer overflow; a deadline made from an
    // hour inside it may fall beyond the end, where it is never reached.
    let waiting = |s: &mut EconomyState| {
        s.places[0]
            .trade
            .jobs
            .iter()
            .position(|j| j.hands.is_none())
            .unwrap()
    };
    assert!(refused(&|s| {
        let i = waiting(s);
        jobs_of(s)[i].made = last + 1;
    }));
    assert!(!refused(&|s| {
        let i = waiting(s);
        jobs_of(s)[i].made = last;
    }));
    assert!(refused(&|s| {
        let i = waiting(s);
        jobs_of(s)[i].made = u64::MAX;
    }));
    assert!(refused(&|s| {
        let i = waiting(s);
        jobs_of(s)[i].made = 0;
        jobs_of(s)[i].given_up = Some(last + 1);
    }));
    assert!(!refused(&|s| {
        let i = waiting(s);
        jobs_of(s)[i].made = 0;
        jobs_of(s)[i].given_up = Some(last);
    }));
    // Days away: a building and a pop that exist, and no more than a month of the people the pop has at work.
    assert!(refused(&|s| {
        s.places[0].trade.days_away.insert((99, 0), 1);
    }));
    assert!(refused(&|s| {
        s.places[0].trade.days_away.insert((0, 99), 1);
    }));
    assert!(refused(&|s| {
        s.places[0].trade.days_away.insert((0, 0), u64::MAX);
    }));
    assert!(refused(&|s| {
        s.places[0].trade.days_away.insert((0, 0), 0);
    }));
    assert!(refused(&|s| {
        s.places[0].trade.days_away.insert((0, 0), (31 << 32) + 1);
    }));
    // A started job holds everything it needs, and what it needs meets what its vehicle is built from.
    let started = |s: &mut EconomyState| {
        s.places[0]
            .trade
            .jobs
            .iter()
            .position(|j| j.hands.is_some())
            .unwrap()
    };
    let wood_index = wood(&rules);
    assert!(refused(&|s| {
        let i = started(s);
        jobs_of(s)[i].stock[wood_index] = 0;
    }));
    assert!(refused(&|s| {
        let i = started(s);
        jobs_of(s)[i].need_goods[wood_index] = 0;
        jobs_of(s)[i].stock[wood_index] = 0;
    }));
    assert!(refused(&|s| {
        let i = waiting(s);
        jobs_of(s)[i].need_goods[wood_index] = 1;
    }));
}

#[test]
fn a_worker_who_dies_after_building_leaves_a_save_that_loads_and_pays() {
    let (rules, mut state) = saved_trade_state();
    // Both builds are done by 16:00; the peasants' one worker dies, leaving no workers and no held hands.
    run_trade_hour(&rules, &mut state, at(14, 12)).unwrap();
    run_trade_hour(&rules, &mut state, at(14, 16)).unwrap();
    assert_eq!(state.places[0].trade.vehicles.len(), 3);
    assert!(state.places[0].trade.jobs.is_empty());
    state.places[0].pops[PEASANT].workers = 0;
    for job in &mut state.places[0].pops[PEASANT].employment {
        job.workers = 0;
    }
    for building in &mut state.places[0].buildings {
        building.employment[PEASANT].workers = 0;
    }
    assert!(!state.places[0].trade.days_away.is_empty());
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let mut loaded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    validate_trade_state(&rules, &loaded).unwrap();
    // The payday after counts the days (a worker who is gone has none to lose) and clears them.
    let april = Date {
        month: 4,
        day: 1,
        ..at(14, 12)
    };
    run_daily_economy(&rules, &mut loaded, april).unwrap();
    assert!(loaded.places[0].trade.days_away.is_empty());
}

#[test]
fn a_build_ordered_in_the_last_days_of_the_calendar_can_be_saved_and_loaded() {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    only_workers(&mut place, PEASANT, 1);
    farm_workers(&rules, &mut place, PEASANT, 1);
    place.pops[PEASANT].stock[wood(&rules)] = 2;
    let mut state = state_of(place);
    let now = trade::last_hour() - 14;
    let date = GameTime::from_hours(now).date().unwrap();
    // The give-up deadline of a waiting job would be past the calendar; the job starts at once, so it never needs it.
    state
        .order_build(&rules, 0, PEASANT, "hand_cart", now)
        .unwrap();
    run_trade_hour(&rules, &mut state, date).unwrap();
    assert!(state.places[0].trade.jobs[0].hands.is_some());
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let mut loaded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    validate_trade_state(&rules, &loaded).unwrap();
    let done = GameTime::from_hours(now + 4).date().unwrap();
    for game in [&mut state, &mut loaded] {
        run_trade_hour(&rules, game, done).unwrap();
    }
    assert_eq!(state, loaded);
    assert_eq!(state.places[0].trade.vehicles.len(), 1);
    // An order for an hour outside the calendar is refused.
    assert!(
        state
            .order_build(&rules, 0, PEASANT, "hand_cart", trade::last_hour() + 1)
            .is_err()
    );
}

#[test]
fn saved_absences_that_overflow_do_not_panic_the_payday_and_a_vehicle_number_is_never_issued_twice()
{
    // Two absence records of one farm whose sum overflows: refused on load, and the payday that reads them cannot panic.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml");
    let mut rules = Rules::load(path).unwrap();
    tests::flatten_pasture(&mut rules);
    let mut state = tests::shipped_production_fixture(&rules);
    state.places[0].price_drift_carries = vec![0; rules.goods.len()];
    state.places[0].prices_milli = rules.goods.iter().map(|g| g.base_price * 1000).collect();
    for pop in &mut state.places[0].pops {
        pop.consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
        pop.consumption.ticking_carries = vec![0; rules.goods.len()];
    }
    state.places[0].trade.days_away.insert((0, 0), u64::MAX);
    state.places[0].trade.days_away.insert((0, 1), 1);
    assert!(validate_trade_state(&rules, &state).is_err());
    let first_of_october = Date {
        year: 768,
        month: 10,
        day: 1,
        hour: 12,
    };
    let _ = run_daily_economy(&rules, &mut state, first_of_october);
    // Two builds due together with one number left: the tick fails and changes nothing, rather than issuing a number twice.
    let (rules, mut full) = saved_trade_state();
    full.places[0].trade.jobs.retain(|j| j.hands.is_some());
    let mut second = full.places[0].trade.jobs[0].clone();
    second.number = 3;
    full.trade.jobs = 3;
    second.hands.as_mut().unwrap().by_building = vec![(1, 1)];
    full.places[0].trade.jobs.push(second);
    full.trade.vehicles = u64::MAX - 1;
    full.places[0].trade.vehicles.clear();
    let before = full.clone();
    let due = GameTime::from_hours(full.places[0].trade.jobs[0].hands.as_ref().unwrap().done)
        .date()
        .unwrap();
    assert!(run_trade_hour(&rules, &mut full, due).is_err());
    assert_eq!(full, before, "nothing was committed");
}

#[test]
fn a_build_whose_completion_falls_past_the_calendars_end_saves_and_loads_through_the_hours_left() {
    let rules = herd_rules();
    let mut place = place(&rules, 3);
    only_workers(&mut place, PEASANT, 1);
    farm_workers(&rules, &mut place, PEASANT, 1);
    place.pops[PEASANT].stock[wood(&rules)] = 2;
    let mut state = state_of(place);
    // 21:00 on the last day: a four-hour cart is due two hours after the calendar's last hour.
    let now = trade::last_hour() - 2;
    let date = |hour: u64| GameTime::from_hours(hour).date().unwrap();
    state
        .order_build(&rules, 0, PEASANT, "hand_cart", now)
        .unwrap();
    run_trade_hour(&rules, &mut state, date(now)).unwrap();
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let mut loaded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    validate_trade_state(&rules, &loaded).unwrap();
    for game in [&mut state, &mut loaded] {
        for hour in [now + 1, now + 2] {
            run_trade_hour(&rules, game, date(hour)).unwrap();
        }
    }
    assert_eq!(state, loaded);
    assert!(
        state.places[0].trade.jobs[0].hands.is_some(),
        "still under way at the last hour"
    );
}

#[test]
fn recorded_needs_beyond_what_any_recipe_could_ask_are_refused_and_never_overflow() {
    // Three goods that supply the wood category in the largest multipliers the rules allow: the rules are valid, an ordinary cart is
    // ordered and validates, and a save with impossible needs across them is refused, not panicked on.
    let mut rules = herd_rules();
    for id in ["oats", "barley", "peas_beans"] {
        let good = good(&rules, id);
        rules.goods[good].tags.push("wood".to_owned());
        rules.goods[good]
            .supplies
            .insert("wood".to_owned(), 9_000_000_000_000_000_000);
    }
    rules.validate("fixture").unwrap();
    let mut place = place(&rules, 3);
    place.pops[PEASANT].stock[wood(&rules)] = 2;
    let mut state = state_of(place);
    state
        .order_build(&rules, 0, PEASANT, "hand_cart", 0)
        .unwrap();
    validate_trade_state(&rules, &state).unwrap();
    let mut bad = state.clone();
    for id in ["oats", "barley", "peas_beans"] {
        bad.places[0].trade.jobs[0].need_goods[good(&rules, id)] = u64::MAX;
    }
    assert!(validate_trade_state(&rules, &bad).is_err());
}
