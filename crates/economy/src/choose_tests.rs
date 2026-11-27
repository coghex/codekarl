//! CK-433 (trade part D): choosing a trade.
//!
//! The cases are the spec's coverage for D (section 12): one trip that pays, one refused, two that compete, the order of candidates not
//! mattering, equal destinations told apart by the rule, every reachable destination evaluated (no screen), the supplies' fixed point,
//! the clocks of looking and the forecast between two traders. They run on the swap tests' fixture places with a table of ways in place of
//! the network, so each way is exactly the hours the case gives it. Nothing sleeps and nothing reads a counter added for it.

use std::collections::BTreeMap;

use super::choose::{carrier_bound, fit_shares, share_weight, trips_in_a_year};
use super::expedition::{Delays, MOST_UNITS, ratio};
use super::expedition_tests::{self, at, date_of, hours};
use super::swap_tests::{
    MONK, PEASANT, THIRD, account, cattle_farm, good, herd_rules, place, set_price,
    settle_the_rest, spare, stock, storehouse, want,
};
use ck_sim::{Canonical, Reader};

use super::fixtures::{dest_place, home_place, rules_with, world};
use super::*;

/// A way of a whole number of hours at every pace, on land.
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

/// A way whose time depends on the pace: `work` hours at walking pace.
fn paced(work: u64) -> Leg {
    Leg {
        mode: Mode::Land,
        terms: vec![Term {
            work: ratio(u128::from(work) * 1000, 1),
            speed: Speed::Plain(1000),
        }],
        fixed: ratio(0, 1),
        delays: Delays::Along,
        key: [0; 4],
        along: Vec::new(),
        path: Vec::new(),
    }
}

/// A table of ways between places by id: `(from, to)` to its legs. What it was asked is kept for the cases that read it.
#[derive(Default)]
struct Table {
    ways: BTreeMap<(String, String), Vec<Leg>>,
    neighbours: Vec<(String, String)>,
    /// Reverses the order the destinations come in.
    reverse: bool,
    asked: Vec<(String, String, Paces)>,
    /// The parties the ways were asked for, as `(profile, boat only)`.
    parties: Vec<(String, bool)>,
    /// Which way each ask was for.
    directions: Vec<Way>,
    /// How many ways were asked for with their geometry frozen.
    frozen: u32,
    /// Ways with several itineraries to choose from, by `(from, to)`: the network gives the quickest at the paces it is asked for.
    choices: BTreeMap<(String, String), Vec<Vec<Leg>>>,
}

/// The hours of an itinerary at the paces, or `None` when a leg is by a mode with none.
fn at_paces(legs: &[Leg], paces: Paces) -> Option<Hours> {
    let mut total = ratio(0, 1);
    for leg in legs {
        let pace = paces.on(leg.mode);
        if pace == 0 {
            return None;
        }
        total += leg.hours(pace);
    }
    Some(total)
}

impl Table {
    /// Gives a pair of places several itineraries each way, from which the quickest at the paces asked for is the way.
    fn either(mut self, a: &str, b: &str, options: Vec<Vec<Leg>>) -> Self {
        self.ways
            .insert((a.to_owned(), b.to_owned()), options[0].clone());
        self.ways
            .insert((b.to_owned(), a.to_owned()), options[0].clone());
        self.choices
            .insert((a.to_owned(), b.to_owned()), options.clone());
        self.choices.insert((b.to_owned(), a.to_owned()), options);
        self
    }

    fn both(mut self, a: &str, b: &str, out: Vec<Leg>, back: Vec<Leg>) -> Self {
        self.ways.insert((a.to_owned(), b.to_owned()), out);
        self.ways.insert((b.to_owned(), a.to_owned()), back);
        self
    }
}

impl Ways for Table {
    fn neighbours(&self, a: &str, b: &str) -> bool {
        self.neighbours
            .iter()
            .any(|(x, y)| (x == a && y == b) || (x == b && y == a))
    }

    fn destinations(&mut self, home: &str, _party: &Party) -> Result<Vec<String>, String> {
        let mut found: Vec<String> = self
            .ways
            .keys()
            .filter(|(from, to)| {
                from == home && self.ways.contains_key(&(to.clone(), home.to_owned()))
            })
            .map(|(_, to)| to.clone())
            .collect();
        if self.reverse {
            found.reverse();
        }
        Ok(found)
    }

    fn journey(
        &mut self,
        home: &str,
        other: &str,
        party: &Party,
        paces: Paces,
        way: Way,
    ) -> Result<Option<Journey>, String> {
        self.asked.push((home.to_owned(), other.to_owned(), paces));
        self.parties.push((party.profile.clone(), party.boat_only));
        self.directions.push(way);
        let key = match way {
            Way::Out => (home.to_owned(), other.to_owned()),
            Way::Back => (other.to_owned(), home.to_owned()),
        };
        if let Some(options) = self.choices.get(&key) {
            let quickest = options
                .iter()
                .filter_map(|legs| at_paces(legs, paces).map(|hours| (hours, legs)))
                .min_by(|a, b| a.0.cmp(&b.0));
            return Ok(quickest.map(|(_, legs)| Journey {
                legs: legs.clone(),
                arrival: [0, 0],
            }));
        }
        Ok(self.ways.get(&key).map(|legs| Journey {
            legs: legs.clone(),
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
        self.frozen += 1;
        self.journey(home, other, party, paces, way)
    }
}

/// The rules of the cases that run with the peasant's own ox-cart and nothing else a trader could build.
fn rules() -> Rules {
    rules_with(&["ox_cart"])
}

/// Runs one hour's looks at the hour of `date`.
fn look(rules: &Rules, state: &mut EconomyState, ways: &mut Table, date: Date) -> LookReport {
    let mut report = LookReport::default();
    run_trade_hour_with_looks(rules, state, date, ways, &mut report).unwrap();
    report
}

fn the_job(state: &EconomyState, place: usize) -> Option<&Plan> {
    state.places[place]
        .trade
        .jobs
        .iter()
        .chain(&[])
        .find_map(|job| match &job.kind {
            JobKind::Trade { plan } => Some(&**plan),
            _ => None,
        })
        .or_else(|| {
            state.places[place]
                .trade
                .expeditions
                .first()
                .map(|expedition| &expedition.plan)
        })
}

#[test]
fn a_trip_that_pays_is_made_a_job_and_the_trader_has_looked() {
    let rules = rules();
    let mut state = world(vec![
        home_place(&rules, "home", 40, 40),
        dest_place(&rules, "dest", 100, 100),
    ]);
    let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
    let date = at(10, 2);
    let report = look(&rules, &mut state, &mut ways, date);
    assert_eq!(
        report.looks, 6,
        "each pop of each place looks the first time"
    );
    assert_eq!(report.by_first, 6);
    let plan = the_job(&state, 0).expect("the peasant's trip pays");
    assert_eq!(plan.destination, 1);
    assert!(plan.wanted[good(&rules, "fish")] > 0);
    assert!(plan.cargo[good(&rules, "barley")] > 0);
    // The peasant's look is recorded, with nothing pending.
    let look = state.places[0].trade.looks[&u32::try_from(PEASANT).unwrap()];
    assert_eq!((look.last, look.rest), (hours(date), None));
    assert_eq!(
        ways.frozen, 2,
        "only the way chosen, out and back, has its geometry frozen"
    );
    let _ = (THIRD, stock(&rules, &state.places[0], PEASANT, "fish"));
}

/// One game hour as the scheduler runs it with looks on: at noon the economy's daily step first, then the trade hour and the looks.
fn step(rules: &Rules, state: &mut EconomyState, ways: &mut Table, hour: u64) -> LookReport {
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
    let mut report = LookReport::default();
    run_trade_hour_with_looks(rules, state, date, ways, &mut report).unwrap();
    report
}

fn peasant() -> u32 {
    u32::try_from(PEASANT).unwrap()
}

/// What the plan of the peasant's trip is, panicking if there is none.
fn plan_of(state: &EconomyState) -> &Plan {
    the_job(state, 0).expect("a trip was made")
}

#[test]
fn a_trip_that_does_not_gain_is_refused_and_the_look_is_still_recorded() {
    let rules = rules();
    // The destination has fish to spare but home values fish no more than the destination does: what is brought is worth what is given
    // up, and the trip costs its supplies.
    let mut home = home_place(&rules, "home", 40, 40);
    set_price(&rules, &mut home, "fish", 1);
    let mut state = world(vec![home, dest_place(&rules, "dest", 100, 100)]);
    let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
    let report = look(&rules, &mut state, &mut ways, at(10, 2));
    assert_eq!(report.trades + report.builds, 0, "{report:?}");
    assert!(the_job(&state, 0).is_none());
    assert_eq!(
        state.places[0].trade.looks[&peasant()].last,
        hours(at(10, 2))
    );
    // Nothing is wanted at a destination that has none of it.
    let mut state = world(vec![
        home_place(&rules, "home", 40, 40),
        dest_place(&rules, "dest", 0, 100),
    ]);
    let report = look(&rules, &mut state, &mut ways, at(10, 2));
    assert_eq!(report.trades + report.builds, 0);
}

#[test]
fn of_two_destinations_the_trader_goes_to_the_one_with_the_greater_gain_whatever_the_order_candidates_come_in()
 {
    let rules = rules();
    let build = |reverse: bool, swapped: bool| {
        let mut places = vec![
            home_place(&rules, "home", 40, 40),
            dest_place(&rules, "near", 5, 100),
            dest_place(&rules, "rich", 100, 100),
        ];
        if swapped {
            places.reverse();
        }
        let mut state = world(places);
        let mut ways = Table {
            reverse,
            ..Table::default()
        }
        .both("home", "near", vec![fixed(6)], vec![fixed(6)])
        .both("home", "rich", vec![fixed(8)], vec![fixed(8)]);
        look(&rules, &mut state, &mut ways, at(10, 2));
        let home = state.places.iter().position(|p| p.id == "home").unwrap();
        let plan = the_job(&state, home).expect("a trip").clone();
        (
            state.places[plan.destination as usize].id.clone(),
            plan.cargo,
            plan.wanted,
        )
    };
    let base = build(false, false);
    assert_eq!(base.0, "rich", "more fish to be had there");
    assert_eq!(
        build(true, false),
        base,
        "reversing the order of the destinations"
    );
    assert_eq!(
        build(false, true),
        base,
        "reversing the order of the places"
    );
    assert_eq!(build(true, true), base);
}

#[test]
fn two_destinations_of_equal_gain_are_told_apart_by_the_shorter_round_trip_then_the_place_id() {
    let rules = rules();
    let go = |out_a: u64, out_b: u64| {
        let mut state = world(vec![
            home_place(&rules, "home", 40, 40),
            dest_place(&rules, "alpha", 100, 100),
            dest_place(&rules, "beta", 100, 100),
        ]);
        let mut ways = Table::default()
            .both("home", "alpha", vec![fixed(out_a)], vec![fixed(out_a)])
            .both("home", "beta", vec![fixed(out_b)], vec![fixed(out_b)]);
        look(&rules, &mut state, &mut ways, at(10, 2));
        state.places[plan_of(&state).destination as usize]
            .id
            .clone()
    };
    assert_eq!(
        go(6, 6),
        "alpha",
        "equal in everything: the place id in sort order"
    );
    assert_eq!(go(8, 6), "beta", "the shorter round trip");
    assert_eq!(go(6, 8), "alpha");
}

#[test]
fn neighbours_are_never_destinations_and_every_other_reachable_place_is_evaluated() {
    let rules = rules();
    let mut state = world(vec![
        home_place(&rules, "home", 40, 40),
        dest_place(&rules, "close", 100, 100),
        dest_place(&rules, "far", 100, 100),
        dest_place(&rules, "farther", 100, 100),
    ]);
    let mut ways = Table::default()
        .both("home", "close", vec![fixed(1)], vec![fixed(1)])
        .both("home", "far", vec![fixed(300)], vec![fixed(300)])
        .both("home", "farther", vec![fixed(900)], vec![fixed(900)]);
    ways.neighbours
        .push(("home".to_owned(), "close".to_owned()));
    let report = look(&rules, &mut state, &mut ways, at(10, 2));
    // The peasant looks at far and farther (a round trip over its limit is evaluated and refused, not skipped); every other pop that is
    // due looks at the places it reaches, and home reaches only these three.
    let from_home: Vec<_> = ways
        .asked
        .iter()
        .filter(|(home, _, _)| home == "home")
        .map(|(_, other, _)| other.as_str())
        .collect();
    assert!(from_home.contains(&"far") && from_home.contains(&"farther"));
    assert!(
        !from_home.contains(&"close"),
        "a neighbour is not a destination"
    );
    assert!(report.destinations >= 2);
    assert!(
        the_job(&state, 0).is_none(),
        "both are beyond a peasant's 168 hours"
    );
}

/// The hours a trip takes by the plan's own legs at the paces its loads give, with the noon wait, worked by the case from the pieces the
/// plan holds and not by the chooser: the round trip of the limit (rule 17) for a look at hour `now`.
fn round_trip(rules: &Rules, state: &EconomyState, plan: &Plan, now: u64) -> Hours {
    let place = &state.places[0];
    let vehicles: Vec<ExpeditionVehicle> = plan
        .vehicles
        .iter()
        .map(|number| {
            let held = place
                .trade
                .vehicles
                .iter()
                .find(|vehicle| vehicle.number == *number)
                .unwrap();
            ExpeditionVehicle {
                number: *number,
                kind: held.kind,
                worn_hours: ratio(0, 1),
            }
        })
        .collect();
    let people: u64 = plan.people.iter().map(|(_, count)| count).sum();
    let way = |legs: &[Leg], load: &[u64]| -> Hours {
        let mut total = ratio(0, 1);
        for leg in legs {
            let assigned =
                expedition::assign_leg(rules, &vehicles, people, load, &plan.supplies, leg.mode)
                    .expect("the load assigns");
            let (leg_pace, _) = expedition::pace_of(
                rules,
                &vehicles,
                &assigned,
                leg.mode,
                false,
                vehicles.is_empty(),
            );
            total += leg.hours(leg_pace);
        }
        total
    };
    let out = way(&plan.out, &plan.cargo);
    let back = way(&plan.back, &plan.wanted);
    // The first tick is the hour after the look; it arrives in the tick its travel ends; it trades at the noon after, if it came by 11:00.
    let ticks = |hours: &Hours| u64::try_from(hours.ceil().to_integer()).unwrap().max(1);
    let arrival = now + ticks(&out);
    let day = arrival / 24;
    let noon = if arrival % 24 <= 11 {
        day * 24 + 12
    } else {
        (day + 1) * 24 + 12
    };
    out + ratio(u128::from(noon - arrival), 1) + back
}

#[test]
fn a_round_trip_exactly_at_the_limit_is_kept_and_the_noon_wait_counts_against_it() {
    let rules = rules();
    // A peasant's limit is 168 hours (r137). Looking at 02:00, a way out of nine hours arrives at 11:00 and trades at that noon after
    // a wait of one hour; one of ten arrives at 12:00 and waits for the next day's noon, 24 hours.
    let kept = |out: u64, back: u64| {
        let mut state = world(vec![
            home_place(&rules, "home", 40, 40),
            dest_place(&rules, "dest", 100, 100),
        ]);
        let mut ways = Table::default().both("home", "dest", vec![fixed(out)], vec![fixed(back)]);
        look(&rules, &mut state, &mut ways, at(10, 2));
        let plan = the_job(&state, 0).cloned();
        if let Some(plan) = &plan {
            assert!(
                round_trip(&rules, &state, plan, hours(at(10, 2))) <= ratio(168, 1),
                "a kept plan is within the limit"
            );
        }
        plan.is_some()
    };
    assert!(kept(9, 158), "9 + 1 + 158 = 168, exactly the limit");
    assert!(!kept(9, 159), "an hour over");
    assert!(!kept(10, 158), "12:00 misses the cut-off: 10 + 24 + 158");
    assert!(kept(10, 134), "10 + 24 + 134 = 168");
    assert!(!kept(10, 135));
}

#[test]
fn a_destination_whose_centre_to_centre_trip_is_over_the_limit_is_evaluated_and_chosen_when_its_clipped_trip_is_within_it()
 {
    // The network hands the look the clipped legs; there is no cut-off in the search it asks for, and nothing here drops a place for a
    // time before it is worked. A destination 167 hours round trip is within 168 whatever its centres' distance was.
    let rules = rules();
    let mut state = world(vec![
        home_place(&rules, "home", 40, 40),
        dest_place(&rules, "dest", 100, 100),
    ]);
    // 6 hours out + the wait to noon of 4 hours + 157 back = 167.
    let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(157)]);
    let report = look(&rules, &mut state, &mut ways, at(10, 2));
    assert!(the_job(&state, 0).is_some());
    assert!(report.destinations >= 1);
}

#[test]
fn the_supplies_are_found_by_a_fixed_point_and_the_dispatched_load_decides_against_the_limit() {
    // Food so heavy that on foot nothing can be carried: only the free hand cart makes the trip, and its pace falls with the weight aboard.
    let mut rules = rules_with(&["hand_cart"]);
    for item in &mut rules.goods {
        if item.id == "barley" {
            item.weight_g = 15_000;
        }
    }
    rules.expedition.foot_people = 1;
    let attempt = |limit: u64| {
        let mut rules = rules.clone();
        rules
            .expedition
            .limit_hours
            .insert("peasant".to_owned(), limit);
        let home = home_place(&rules, "home", 40, 40);
        let mut state = world(vec![home, dest_place(&rules, "dest", 100, 100)]);
        let mut ways = Table::default().both("home", "dest", vec![paced(40)], vec![paced(40)]);
        look(&rules, &mut state, &mut ways, at(10, 2));
        let plan = the_job(&state, 0).cloned();
        (rules, state, plan)
    };
    let (rules_big, state_big, plan) = attempt(10_000);
    let plan = plan.expect("a trip within a very long limit");
    assert!(plan.vehicles.len() == 1, "the cart makes it");
    let hours_with_supplies = round_trip(&rules_big, &state_big, &plan, hours(at(10, 2)));
    // The trip without the supplies' own weight would go faster: the dispatched load is what every check uses.
    let mut bare = plan.clone();
    bare.supplies = vec![0; rules.goods.len()];
    let bare_hours = round_trip(&rules_big, &state_big, &bare, hours(at(10, 2)));
    assert!(
        bare_hours < hours_with_supplies,
        "{bare_hours} < {hours_with_supplies}"
    );
    let limit_down = u64::try_from(hours_with_supplies.floor().to_integer()).unwrap();
    let limit_up = u64::try_from(hours_with_supplies.ceil().to_integer()).unwrap();
    assert!(
        limit_down < limit_up,
        "the trip is not a whole number of hours"
    );
    assert!(
        limit_down >= u64::try_from(bare_hours.ceil().to_integer()).unwrap(),
        "the limit is above what the bare trip needs"
    );
    assert!(
        attempt(limit_down).2.is_none(),
        "over the limit by its supplies' weight"
    );
    assert!(attempt(limit_up).2.is_some(), "within it");
}

/// Every pop of every place has looked at hour `last`, with nothing pending.
fn all_looked(state: &mut EconomyState, last: u64) {
    for place in &mut state.places {
        for pop in 0..place.pops.len() {
            place
                .trade
                .looks
                .insert(u32::try_from(pop).unwrap(), Look { last, rest: None });
        }
    }
}

fn quiet_world(rules: &Rules) -> EconomyState {
    // Nobody wants anything the other place has, so a look makes no job and only the clock is seen.
    let mut a = place(rules, 3);
    a.id = "a".to_owned();
    let mut b = place(rules, 3);
    b.id = "b".to_owned();
    for place in [&mut a, &mut b] {
        settle_the_rest(rules, place, &[]);
    }
    world(vec![a, b])
}

#[test]
fn a_trader_looks_again_after_720_hours_and_a_pending_rest_look_after_it_clears() {
    let rules = rules();
    let mut state = quiet_world(&rules);
    let mut ways = Table::default().both("a", "b", vec![fixed(2)], vec![fixed(2)]);
    let start = hours(at(2, 0));
    all_looked(&mut state, start);
    state.places[0]
        .trade
        .looks
        .get_mut(&peasant())
        .unwrap()
        .rest = Some(start + 100);
    // Nothing is due until the rest look's hour, the 720-hour timer's being later.
    for hour in start + 1..start + 100 {
        assert_eq!(
            step(&rules, &mut state, &mut ways, hour).looks,
            0,
            "hour {hour}"
        );
    }
    let rest = step(&rules, &mut state, &mut ways, start + 100);
    assert_eq!((rest.looks, rest.by_rest, rest.by_timer), (1, 1, 0));
    assert_eq!(
        state.places[0].trade.looks[&peasant()],
        Look {
            last: start + 100,
            rest: None
        },
        "a look sets the last look and clears the pending one"
    );
    assert_eq!(
        step(&rules, &mut state, &mut ways, start + 101).looks,
        0,
        "none the hour after"
    );
    // The others' timers fall due 720 hours after their last look.
    let mut seen = 0;
    for hour in start + 102..=start + 720 {
        let report = step(&rules, &mut state, &mut ways, hour);
        if report.looks > 0 {
            assert_eq!(hour, start + 720);
            assert_eq!(report.by_timer, 5, "the five that had not looked since");
            seen += 1;
        }
    }
    assert_eq!(seen, 1);
    // The peasant that looked at start + 100 is due at start + 820.
    assert_eq!(step(&rules, &mut state, &mut ways, start + 819).looks, 0);
    assert_eq!(step(&rules, &mut state, &mut ways, start + 820).by_timer, 1);
}

#[test]
fn an_early_timer_look_clears_the_rest_look_and_coincident_triggers_make_one_look() {
    let rules = rules_with(&["hand_cart"]);
    let mut state = quiet_world(&rules);
    let mut ways = Table::default().both("a", "b", vec![fixed(2)], vec![fixed(2)]);
    let now = hours(at(12, 6));
    all_looked(&mut state, now);
    // The timer and the rest fall due in the same hour: one look, counted under both.
    state.places[0].trade.looks.insert(
        peasant(),
        Look {
            last: now - 720,
            rest: Some(now),
        },
    );
    let report = step(&rules, &mut state, &mut ways, now);
    assert_eq!((report.looks, report.by_timer, report.by_rest), (1, 1, 1));
    assert_eq!(state.places[0].trade.looks[&peasant()].rest, None);
    // The timer comes first, with the rest look still seven hours away: the look clears it, so nothing is due then.
    state.places[0].trade.looks.insert(
        peasant(),
        Look {
            last: now + 1 - 720,
            rest: Some(now + 8),
        },
    );
    assert_eq!(step(&rules, &mut state, &mut ways, now + 1).by_timer, 1);
    assert_eq!(state.places[0].trade.looks[&peasant()].rest, None);
    for hour in now + 2..now + 12 {
        assert_eq!(
            step(&rules, &mut state, &mut ways, hour).looks,
            0,
            "hour {hour}"
        );
    }
}

#[test]
fn a_return_at_hour_100_gives_one_look_at_112_and_none_at_113_and_an_early_look_clears_it() {
    // A real trip, home in the hour it was expected, and the look that follows the rest.
    let mut rules = herd_rules();
    let mut state = expedition_tests::two_places(&mut rules);
    rules.expedition.looks = true;
    let mut ways = Table::default();
    let made = hours(at(10, 5));
    state
        .order_trade(
            &rules,
            0,
            PEASANT,
            expedition_tests::the_plan(&rules, 6, 5),
            made,
        )
        .unwrap();
    all_looked(&mut state, made - 1);
    let mut home_at = None;
    for hour in made..made + 200 {
        let report = step(&rules, &mut state, &mut ways, hour);
        if let Some(rest) = state.places[0].trade.looks[&peasant()].rest {
            home_at.get_or_insert((hour, rest));
        }
        if let Some((back, rest)) = home_at {
            assert_eq!(
                rest,
                back + 12,
                "the rest look is pending at the hour it unloaded plus 12"
            );
            if hour < rest {
                assert_eq!(report.looks, 0, "hour {hour}");
            } else if hour == rest {
                assert_eq!((report.looks, report.by_rest), (1, 1));
            } else if hour == rest + 1 {
                assert_eq!(report.looks, 0, "none at 113");
                return;
            }
        }
    }
    panic!("the trip never came home");
}

fn water(hours: u64) -> Leg {
    Leg {
        mode: Mode::Water,
        ..fixed(hours)
    }
}

/// A destination whose monks have the money to pay for all they want: the market funds a want from the value of what a pop has to spare
/// (at base prices), so a trade is as big as the monks' budget lets it be, which the cases that count on a size give them.
fn funded(rules: &Rules, mut place: PlaceState) -> PlaceState {
    spare(rules, &mut place, MONK, "oats", 1000);
    place
}

#[test]
fn two_traders_in_one_hour_the_first_ones_forecast_changes_the_seconds_choice() {
    let mut rules = rules();
    // Light barley, so what a cart carries is not what limits the trade.
    for item in &mut rules.goods {
        if item.id == "barley" {
            item.weight_g = 500;
        }
    }
    let mut home = home_place(&rules, "home", 40, 40);
    // A second trader, the labourers, wants the same fish with the same barley to give, and has a cart of its own.
    want(&rules, &mut home, THIRD, "fish", 40);
    spare(&rules, &mut home, THIRD, "barley", 40);
    home.trade.vehicles.push(Vehicle {
        number: 2,
        kind: 0,
        pop: u32::try_from(THIRD).unwrap(),
        free: true,
        materials: None,
    });
    let build = |home: PlaceState| {
        let mut state = world(vec![
            home,
            funded(&rules, dest_place(&rules, "near", 25, 100)),
            funded(&rules, dest_place(&rules, "rich", 50, 100)),
        ]);
        let mut ways = Table::default()
            .both("home", "near", vec![fixed(6)], vec![fixed(6)])
            .both("home", "rich", vec![fixed(12)], vec![fixed(12)]);
        look(&rules, &mut state, &mut ways, at(10, 2));
        state
    };
    let state = build(home.clone());
    let destination = |state: &EconomyState, pop: usize| {
        let trade = &state.places[0].trade;
        let planned = trade
            .jobs
            .iter()
            .find(|job| job.pop as usize == pop)
            .and_then(|job| match &job.kind {
                JobKind::Trade { plan } => Some(plan.destination),
                _ => None,
            })
            .or_else(|| {
                trade
                    .expeditions
                    .iter()
                    .find(|expedition| expedition.trader as usize == pop)
                    .map(|expedition| expedition.plan.destination)
            });
        planned.map(|destination| state.places[destination as usize].id.clone())
    };
    assert_eq!(
        destination(&state, PEASANT).as_deref(),
        Some("rich"),
        "the first has the most fish to gain there"
    );
    assert_eq!(
        destination(&state, THIRD).as_deref(),
        Some("near"),
        "the fish the first is to take from rich is spoken for: 10 are left there against 25 at near"
    );
    // Alone, the second trader would have gone to the richer place.
    let mut alone = home;
    want(&rules, &mut alone, PEASANT, "fish", 0);
    let state = build(alone);
    assert_eq!(destination(&state, THIRD).as_deref(), Some("rich"));
}

#[test]
fn a_plan_on_foot_is_chosen_over_a_cart_when_it_costs_less_to_run() {
    let mut rules = rules();
    // The cart takes four people and eats for them all; a few on foot do the same trade for less food.
    for vehicle in &mut rules.vehicles {
        vehicle.crew_full = 4;
        vehicle.crew_fewest = 4;
    }
    let mut home = home_place(&rules, "home", 40, 40);
    // Barley is dearer than oats for its hunger, so the supplies are oats.
    set_price(&rules, &mut home, "barley", 2);
    let mut state = world(vec![home, dest_place(&rules, "dest", 100, 100)]);
    // A trip of some days, so that the food counts whole units of oats (1,800 hundredths each).
    let mut ways = Table::default().both("home", "dest", vec![fixed(60)], vec![fixed(60)]);
    look(&rules, &mut state, &mut ways, at(10, 2));
    let plan = plan_of(&state);
    assert!(plan.vehicles.is_empty(), "on foot, not the cart");
    assert!(
        plan.people.iter().map(|(_, count)| count).sum::<u64>() < 4,
        "fewer mouths than the cart's four"
    );
}

#[test]
fn of_two_free_carts_of_one_kind_the_lower_numbered_is_used() {
    let rules = rules();
    let go = |numbers: [u64; 2]| {
        let mut home = home_place(&rules, "home", 40, 40);
        home.trade.vehicles.clear();
        for number in numbers {
            home.trade.vehicles.push(Vehicle {
                number,
                kind: 0,
                pop: peasant(),
                free: true,
                materials: None,
            });
        }
        let mut state = world(vec![home, dest_place(&rules, "dest", 100, 100)]);
        state.trade.vehicles = 2;
        let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
        look(&rules, &mut state, &mut ways, at(10, 2));
        plan_of(&state).vehicles.clone()
    };
    assert_eq!(go([1, 2]), vec![1]);
    assert_eq!(
        go([2, 1]),
        vec![1],
        "whatever order the vehicles are listed in"
    );
}

#[test]
fn a_boat_alone_is_a_plan_when_the_way_has_no_land_leg_and_the_party_says_so() {
    let rules = rules_with(&["simple_barge"]);
    let mut state = world(vec![
        home_place(&rules, "home", 40, 40),
        dest_place(&rules, "dest", 100, 100),
    ]);
    let mut ways = Table::default().both("home", "dest", vec![water(6)], vec![water(6)]);
    look(&rules, &mut state, &mut ways, at(10, 2));
    let plan = plan_of(&state);
    assert_eq!(plan.vehicles, vec![1], "the barge");
    assert!(plan.out.iter().all(|leg| leg.mode == Mode::Water));
    assert!(
        ways.parties.contains(&("barge".to_owned(), true)),
        "the way was asked for the boat-only party: {:?}",
        ways.parties
    );
}

#[test]
fn the_way_home_is_asked_for_at_the_pace_of_the_load_it_carries() {
    // A hand cart carries cargo out and fish back: heavy one way, light the other, so the two ways are searched at different paces.
    let rules = rules_with(&["hand_cart"]);
    let mut state = world(vec![
        home_place(&rules, "home", 40, 40),
        dest_place(&rules, "dest", 100, 100),
    ]);
    let mut ways = Table::default().both("home", "dest", vec![paced(10)], vec![paced(10)]);
    look(&rules, &mut state, &mut ways, at(10, 2));
    let plan = plan_of(&state);
    assert_eq!(plan.vehicles, vec![1]);
    let pace = |way: Way| -> Vec<u32> {
        ways.asked
            .iter()
            .zip(&ways.directions)
            .filter(|(asked, direction)| **direction == way && asked.0 == "home")
            .map(|(asked, _)| asked.2.land)
            .collect()
    };
    let (out, back) = (pace(Way::Out), pace(Way::Back));
    assert!(!out.is_empty() && !back.is_empty());
    assert!(
        out.iter().min() < back.iter().max(),
        "out {out:?} is slower than back {back:?}"
    );
}

#[test]
fn a_look_builds_the_vehicle_a_trip_needs_when_the_year_pays_and_looks_again_when_it_is_built() {
    let mut rules = rules_with(&["hand_cart"]);
    // Food so heavy that nothing is carried on foot.
    for item in &mut rules.goods {
        if item.id == "barley" {
            item.weight_g = 15_000;
        }
    }
    let mut home = home_place(&rules, "home", 40, 40);
    home.trade.vehicles.clear();
    // Two hardwood for the cart, in the peasant's store.
    home.pops[PEASANT].stock[good(&rules, "hardwood")] += 2;
    let mut state = world(vec![home, dest_place(&rules, "dest", 100, 100)]);
    let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
    let made = hours(at(10, 2));
    let report = step(&rules, &mut state, &mut ways, made);
    assert_eq!(report.builds, 1, "{report:?}");
    let job = &state.places[0].trade.jobs[0];
    let JobKind::Build { trip, .. } = &job.kind else {
        panic!("a build");
    };
    assert_eq!(
        trip.as_ref().unwrap().destination,
        1,
        "built for the trip to dest"
    );
    // A rest look pending when the build is done is cleared by the look that follows it (rule 16).
    state.places[0]
        .trade
        .looks
        .get_mut(&peasant())
        .unwrap()
        .rest = Some(made + 100);
    // Four hours later the vehicle is done and its trader looks at once, deciding afresh.
    let mut report = LookReport::default();
    for hour in made + 1..=made + 4 {
        report = step(&rules, &mut state, &mut ways, hour);
    }
    assert_eq!(report.by_build, 1, "{report:?}");
    assert_eq!(state.places[0].trade.vehicles.len(), 1);
    assert_eq!(
        state.places[0].trade.looks[&peasant()],
        Look {
            last: made + 4,
            rest: None
        },
        "the look at the hour it was built, which cleared the pending rest look"
    );
    assert!(
        state.places[0]
            .trade
            .jobs
            .iter()
            .any(|job| matches!(job.kind, JobKind::Trade { .. }))
            || !state.places[0].trade.expeditions.is_empty(),
        "and the trip is made now it has the cart"
    );
}

#[test]
fn nothing_looks_when_the_rules_have_looks_off_and_a_save_holds_the_clocks() {
    let mut rules = rules();
    let mut state = world(vec![
        home_place(&rules, "home", 40, 40),
        dest_place(&rules, "dest", 100, 100),
    ]);
    let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
    rules.expedition.looks = false;
    let report = look(&rules, &mut state, &mut ways, at(10, 2));
    assert_eq!(report, LookReport::default());
    assert!(
        state
            .places
            .iter()
            .all(|place| place.trade.looks.is_empty())
    );
    assert!(ways.asked.is_empty());
    // With looks on, the clocks are in the saved state and come back whole.
    rules.expedition.looks = true;
    look(&rules, &mut state, &mut ways, at(10, 2));
    state.places[0].trade.looks.insert(
        peasant(),
        Look {
            last: 5,
            rest: Some(9),
        },
    );
    let mut bytes = Vec::new();
    state.encode(&mut bytes).unwrap();
    let loaded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    assert_eq!(loaded, state);
    validate_trade_state_at(&rules, &loaded, hours(at(10, 3))).unwrap();
    // A look of a pop that does not exist, or one that has not happened yet, is refused.
    let mut bad = state.clone();
    bad.places[0].trade.looks.insert(
        99,
        Look {
            last: 5,
            rest: None,
        },
    );
    assert!(validate_trade_state_at(&rules, &bad, hours(at(10, 3))).is_err());
    let mut bad = state.clone();
    bad.places[0].trade.looks.insert(
        peasant(),
        Look {
            last: hours(at(10, 3)) + 1,
            rest: None,
        },
    );
    assert!(validate_trade_state_at(&rules, &bad, hours(at(10, 3))).is_err());
}

#[test]
fn a_game_saved_between_a_look_and_the_next_noon_goes_on_as_the_unbroken_game_would() {
    let rules = rules();
    let mut unbroken = world(vec![
        home_place(&rules, "home", 40, 40),
        dest_place(&rules, "dest", 100, 100),
    ]);
    let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
    let start = hours(at(10, 2));
    step(&rules, &mut unbroken, &mut ways, start);
    for hour in start + 1..start + 5 {
        step(&rules, &mut unbroken, &mut ways, hour);
    }
    let mut bytes = Vec::new();
    unbroken.encode(&mut bytes).unwrap();
    let mut loaded = EconomyState::decode(&mut Reader::new(&bytes)).unwrap();
    assert_eq!(loaded, unbroken);
    let mut ways_loaded = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
    // Through the noons, then the payday of the 1st of April and its noon, which the looks' state must go through as well.
    let april = Date {
        month: 4,
        day: 1,
        ..at(1, 12)
    };
    let payday = hours(april);
    for hour in start + 5..payday {
        step(&rules, &mut unbroken, &mut ways, hour);
        step(&rules, &mut loaded, &mut ways_loaded, hour);
        assert_eq!(loaded, unbroken, "hour {hour}");
    }
    for game in [&mut unbroken, &mut loaded] {
        run_daily_economy(&rules, game, april).unwrap();
    }
    let mut report = [LookReport::default(), LookReport::default()];
    run_trade_hour_with_looks(&rules, &mut unbroken, april, &mut ways, &mut report[0]).unwrap();
    run_trade_hour_with_looks(&rules, &mut loaded, april, &mut ways_loaded, &mut report[1])
        .unwrap();
    assert_eq!(loaded, unbroken, "the payday and its noon");
    assert_eq!(report[0], report[1]);
}

#[test]
fn when_it_cannot_have_all_it_wants_the_worth_it_can_pay_is_divided_among_the_goods_in_the_ratio_of_their_deficits()
 {
    let mut rules = rules();
    for item in &mut rules.goods {
        if item.id == "barley" {
            item.weight_g = 500;
        }
    }
    // The peasant lacks 30 fish and 10 honey and can pay 12 barley, at one a unit everywhere: not enough for both. Honey is the
    // smaller bulk and would be bought first and in full; the worth is divided 3 to 1 instead.
    let mut home = home_place(&rules, "home", 30, 12);
    want(&rules, &mut home, PEASANT, "honey", 10);
    set_price(&rules, &mut home, "honey", 4);
    let mut dest = dest_place(&rules, "dest", 100, 100);
    spare(&rules, &mut dest, MONK, "honey", 100);
    set_price(&rules, &mut dest, "honey", 1);
    let mut state = world(vec![home, dest]);
    let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
    look(&rules, &mut state, &mut ways, at(10, 2));
    let plan = plan_of(&state);
    let (fish, honey) = (good(&rules, "fish"), good(&rules, "honey"));
    assert_eq!(
        (plan.wanted[fish], plan.wanted[honey]),
        (9, 3),
        "12 of worth in the ratio 30 to 10"
    );
}

#[test]
fn only_a_plan_that_gains_is_a_candidate_so_a_cheap_plan_that_does_nothing_does_not_hide_one_that_pays()
 {
    // On foot with one person nothing can be carried beside the food (barley weighs a pack): it costs least and gains nothing. The cart
    // costs more and pays; the trader takes it.
    let rules = rules();
    let mut state = world(vec![
        home_place(&rules, "home", 40, 40),
        dest_place(&rules, "dest", 100, 100),
    ]);
    let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
    look(&rules, &mut state, &mut ways, at(10, 2));
    let plan = plan_of(&state);
    assert!(
        !plan.cargo.iter().all(|units| *units == 0),
        "it carries something"
    );
    assert!(plan.wanted.iter().any(|units| *units > 0));
}

/// A destination whose monks hold all their fish in a private account in a storehouse, `fish` of it above their target.
fn storehouse_dest(rules: &Rules, id: &str, fish: u64, barley: u64) -> PlaceState {
    let mut dest = dest_place(rules, id, 0, barley);
    let house = storehouse(rules, &mut dest);
    let index = good(rules, "fish");
    let goal = std::mem::take(&mut dest.pops[MONK].stock[index]);
    account(&mut dest, house, MONK, index, goal + fish);
    dest
}

#[test]
fn goods_spoken_for_leave_what_a_place_would_offer_and_the_storehouse_accounts_still_balance() {
    let rules = rules();
    let fish = good(&rules, "fish");
    let mut dest = storehouse_dest(&rules, "dest", 30, 100);
    // What the monks keep for themselves, which no offer touches.
    let kept = {
        let bare = storehouse_dest(&rules, "dest", 0, 100);
        bare.buildings.iter().find_map(|building| {
            building
                .storehouse_accounts
                .as_ref()
                .map(|accounts| accounts.pops[MONK][fish])
        })
    }
    .unwrap();
    let (_, offered) = swap::place_totals(&rules, &dest).unwrap();
    assert_eq!(offered[fish], 30, "the monks offer their account's fish");
    // Twenty are spoken for: the offer falls by twenty, the account with the physical stock, and the market still accepts the copy.
    let mut spoken = vec![0; rules.goods.len()];
    spoken[fish] = 20;
    swap::take_spoken(&rules, &mut dest, &spoken).unwrap();
    let (_, offered) = swap::place_totals(&rules, &dest).unwrap();
    assert_eq!(offered[fish], 10);
    let house = dest
        .buildings
        .iter()
        .position(|building| building.storehouse_accounts.is_some())
        .unwrap();
    assert_eq!(
        dest.buildings[house]
            .storehouse_accounts
            .as_ref()
            .unwrap()
            .pops[MONK][fish],
        kept + 10
    );
    assert_eq!(dest.buildings[house].stock[fish], kept + 10);
    // More than is on offer is spoken for in vain: nothing the place keeps is taken, and the offer is gone, not negative.
    spoken[fish] = 500;
    swap::take_spoken(&rules, &mut dest, &spoken).unwrap();
    let (_, offered) = swap::place_totals(&rules, &dest).unwrap();
    assert_eq!(offered[fish], 0);
    assert_eq!(
        dest.buildings[house]
            .storehouse_accounts
            .as_ref()
            .unwrap()
            .pops[MONK][fish],
        kept,
        "what the monks keep for themselves is not touched"
    );
    assert_eq!(dest.buildings[house].stock[fish], kept);
}

#[test]
fn two_successive_looks_at_a_destination_that_offers_account_backed_stock_both_succeed_and_the_second_finds_less()
 {
    let mut rules = rules();
    for item in &mut rules.goods {
        if item.id == "barley" {
            item.weight_g = 500;
        }
    }
    let mut home = home_place(&rules, "home", 40, 40);
    want(&rules, &mut home, THIRD, "fish", 40);
    spare(&rules, &mut home, THIRD, "barley", 40);
    home.trade.vehicles.push(Vehicle {
        number: 2,
        kind: 0,
        pop: u32::try_from(THIRD).unwrap(),
        free: true,
        materials: None,
    });
    let mut state = world(vec![home, storehouse_dest(&rules, "rich", 50, 100)]);
    let mut ways = Table::default().both("home", "rich", vec![fixed(6)], vec![fixed(6)]);
    // The second look used to fail with "storehouse physical stock differs from its accounts" on the first trip's staged copy.
    look(&rules, &mut state, &mut ways, at(10, 2));
    let fish = good(&rules, "fish");
    let wanted = |pop: usize| {
        state.places[0]
            .trade
            .jobs
            .iter()
            .find(|job| job.pop as usize == pop)
            .and_then(|job| match &job.kind {
                JobKind::Trade { plan } => Some(plan.wanted[fish]),
                _ => None,
            })
            .or_else(|| {
                state.places[0]
                    .trade
                    .expeditions
                    .iter()
                    .find(|expedition| expedition.trader as usize == pop)
                    .map(|expedition| expedition.plan.wanted[fish])
            })
    };
    let (first, second) = (wanted(PEASANT).unwrap(), wanted(THIRD).unwrap());
    assert!(first > 0 && second > 0, "{first} {second}");
    assert!(
        first + second <= 50,
        "the second plans only what the first left: {first} + {second}"
    );
}

/// A land leg of `work` hours at walking pace.
fn land_work(work: u64) -> Leg {
    paced(work)
}

/// A water leg of `work` hours at walking pace.
fn water_work(work: u64) -> Leg {
    Leg {
        mode: Mode::Water,
        ..paced(work)
    }
}

#[test]
fn a_party_on_land_and_water_gets_the_quickest_way_at_the_pace_it_has_on_each_not_at_the_pace_of_its_first_leg()
 {
    // A wagon, slower on land than the currach is on water, carrying the currach over the land. Two ways, both land then water: one mostly on land, one mostly
    // on the water. At the cart's pace on every leg the first is the shorter (41 against 49 hours of walking); at the paces each leg is
    // really travelled at the second is the quicker by far, and it is the only one that gets the round trip under the peasant's 168 hours.
    let mut rules = rules_with(&["covered_wagon", "currach"]);
    // A slow wagon (the only land vehicle with room for the currach), and light barley, so what it carries is not what limits the trade.
    rules.vehicles[0].land_pace_pm = Some(500);
    // Each carries the other: the currach rides in the wagon on land and the wagon, made smaller, rides in the currach on water.
    rules.vehicles[0].carried_ml = Some(2_000_000);
    for item in &mut rules.goods {
        if item.id == "barley" {
            item.weight_g = 500;
        }
    }
    let mut home = home_place(&rules, "home", 40, 40);
    home.trade.vehicles.push(Vehicle {
        number: 2,
        kind: 1,
        pop: peasant(),
        free: true,
        materials: None,
    });
    let mostly_land = vec![land_work(40), water_work(1)];
    let mostly_water = vec![land_work(1), water_work(48)];
    let mut state = world(vec![home, dest_place(&rules, "dest", 100, 100)]);
    let mut ways = Table::default().either(
        "home",
        "dest",
        vec![mostly_land.clone(), mostly_water.clone()],
    );
    look(&rules, &mut state, &mut ways, at(10, 2));
    let plan = the_job(&state, 0).expect("the trip over the water is within the limit");
    assert_eq!(
        plan.vehicles.len(),
        2,
        "the wagon and the currach: {:?}",
        plan.vehicles
    );
    assert_eq!(plan.out, mostly_water);
    assert_eq!(plan.back, mostly_water);
    assert!(round_trip(&rules, &state, plan, hours(at(10, 2))) <= ratio(168, 1));
    // What the case set up: the paces the way was asked at differ by mode, a search at the land pace alone prefers the other way, and
    // that way is over the limit.
    let (_, _, paces) = ways
        .asked
        .iter()
        .find(|(home, _, paces)| home == "home" && paces.land != 0 && paces.water != 0)
        .expect("a way was asked for at the paces of both modes");
    assert!(paces.land < paces.water, "{paces:?}");
    let uniform = Paces::uniform(paces.land);
    assert!(
        at_paces(&mostly_land, uniform) < at_paces(&mostly_water, uniform),
        "at the land pace on every leg the mostly-land way looks quicker"
    );
    let mut slower = plan.clone();
    slower.out = mostly_land.clone();
    slower.back = mostly_land;
    assert!(round_trip(&rules, &state, &slower, hours(at(10, 2))) > ratio(168, 1));
}

#[test]
fn the_yearly_test_counts_the_trips_the_need_left_unmet_needs_not_the_year_s_gross_use() {
    // A cart that costs 60,000 to build (two hardwood at 30) against a trip that gains 26,000: it pays only if the need takes three trips
    // or more. The peasant lacks 9 fish and a trip brings 9, so the need is one trip, and a year of trips on that need would be a
    // single one however many the hours would allow: no vehicle is built. Lacking 40, it takes five trips and the cart is built.
    let build = |lack: u64, at_home: u64, hardwood: u64| {
        let mut rules = rules_with(&["hand_cart"]);
        for item in &mut rules.goods {
            if item.id == "barley" {
                item.weight_g = 15_000;
            }
        }
        let mut home = home_place(&rules, "home", lack, 40);
        home.trade.vehicles.clear();
        home.pops[PEASANT].stock[good(&rules, "hardwood")] += 2;
        set_price(&rules, &mut home, "hardwood", hardwood);
        // What another pop of the place has to spare of the fish the trader lacks.
        if at_home > 0 {
            spare(&rules, &mut home, THIRD, "fish", at_home);
        }
        let mut state = world(vec![home, dest_place(&rules, "dest", 100, 100)]);
        let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
        step(&rules, &mut state, &mut ways, hours(at(10, 2)));
        // The builds of the peasant's place (the destination's own pops look too).
        state.places[0]
            .trade
            .jobs
            .iter()
            .filter(|job| matches!(job.kind, JobKind::Build { .. }))
            .count()
    };
    assert_eq!(
        build(9, 0, 30),
        0,
        "one trip fills the need and cannot pay for the cart"
    );
    assert_eq!(build(40, 0, 30), 1, "five trips can");
    assert_eq!(
        build(40, 40, 30),
        0,
        "with the fish to be had at home the need that cannot be met there is none, and one trip does not pay"
    );
    // The cart cheap enough for one trip to finance (two hardwood at 1: 2,000 against 26,000 a trip): it is built for a need that is
    // real, and not for one the home market covers (the one-trip floor used to build it for no need at all).
    assert_eq!(build(40, 0, 1), 1, "a real need builds the cheap cart");
    assert_eq!(
        build(40, 40, 1),
        0,
        "zero fish unmet at home builds nothing, however cheap the cart"
    );
}

#[test]
fn the_trips_a_year_are_the_fewer_of_the_whole_round_trips_that_fit_and_the_trips_the_unmet_need_takes()
 {
    // 100.9 hours a trip: 86 fit in 8,760, not the 87 that flooring the trip to 100 hours would give.
    assert_eq!(trips_in_a_year(8760, &ratio(1009, 10), &[(1000, 1)]), 86);
    assert_eq!(trips_in_a_year(8760, &ratio(100, 1), &[(1000, 1)]), 87);
    // The need counts: 40 short and 9 a trip is five trips, 9 short is one, however many the year holds.
    assert_eq!(trips_in_a_year(8760, &ratio(16, 1), &[(40, 9)]), 5);
    assert_eq!(trips_in_a_year(8760, &ratio(16, 1), &[(9, 9)]), 1);
    // The goods share trips: the trips are the most any of them takes.
    assert_eq!(trips_in_a_year(8760, &ratio(16, 1), &[(9, 9), (25, 10)]), 3);
    // A need of nothing is no trip: what home and the trips coming cover is not a reason to build.
    assert_eq!(trips_in_a_year(8760, &ratio(16, 1), &[]), 0);
    assert_eq!(trips_in_a_year(8760, &ratio(16, 1), &[(0, 9)]), 0);
}

#[test]
fn a_wagon_carrying_a_currach_is_tried_on_a_way_wholly_on_land_with_its_load_fitted_to_the_land_alone()
 {
    // With the shipped sizes the wagon cannot ride in the currach, so the pair can never make a leg on water, yet it makes a way wholly on
    // land: the plan restricted to land is searched for the party with a pace on land and none on water (11d.2: the load must assign on
    // the itinerary's legs, not on every mode of the shape).
    let mut rules = rules_with(&["covered_wagon", "currach"]);
    for item in &mut rules.goods {
        if item.id == "barley" {
            item.weight_g = 500;
        }
    }
    let mut home = home_place(&rules, "home", 40, 40);
    home.trade.vehicles.push(Vehicle {
        number: 2,
        kind: 1,
        pop: peasant(),
        free: true,
        materials: None,
    });
    let mut state = world(vec![home, dest_place(&rules, "dest", 100, 100)]);
    let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
    look(&rules, &mut state, &mut ways, at(10, 2));
    let on_land =
        ways.asked
            .iter()
            .zip(&ways.parties)
            .any(|((_, _, paces), (profile, boat_only))| {
                profile == "currach" && !boat_only && paces.land != 0 && paces.water == 0
            });
    assert!(on_land, "{:?} {:?}", ways.asked, ways.parties);
    assert!(
        ways.asked
            .iter()
            .all(|(_, _, paces)| paces.land != 0 || paces.water != 0),
        "a way is never asked for with no pace on any mode"
    );
}

#[test]
fn live_animals_spoken_for_leave_the_herd_offer_and_the_given_up_jobs_offer() {
    // A farm of nine cattle with a standard of seven offers two. One head spoken for leaves one on offer, and what is spoken for beyond
    // what is offered takes nothing from the herd's standard. (The staged copy is where the book is read; the real place is untouched.)
    let rules = rules();
    let cattle = good(&rules, "cattle");
    let mut dest = dest_place(&rules, "dest", 0, 100);
    cattle_farm(&rules, &mut dest, 9, &[(MONK, 1)]);
    let untouched = dest.clone();
    let (_, offered) = swap::place_totals(&rules, &dest).unwrap();
    assert_eq!(offered[cattle], 2, "nine head over a standard of seven");
    let mut spoken = vec![0; rules.goods.len()];
    spoken[cattle] = 1;
    swap::take_spoken(&rules, &mut dest, &spoken).unwrap();
    let (_, offered) = swap::place_totals(&rules, &dest).unwrap();
    assert_eq!(offered[cattle], 1);
    spoken[cattle] = 5;
    swap::take_spoken(&rules, &mut dest, &spoken).unwrap();
    let (_, offered) = swap::place_totals(&rules, &dest).unwrap();
    assert_eq!(offered[cattle], 0);
    let farm = dest
        .buildings
        .iter()
        .position(|building| !building.herds.is_empty())
        .unwrap();
    assert_eq!(
        dest.buildings[farm].herds[0], 7,
        "the standard herd is kept"
    );
    // The animals a given-up job offers go the same way.
    let mut dest = untouched;
    dest.trade.jobs.push(Job {
        number: 1,
        pop: u32::try_from(MONK).unwrap(),
        kind: JobKind::Build {
            vehicle: 0,
            trip: None,
        },
        made: 0,
        need_goods: vec![0; rules.goods.len()],
        need_animals: Vec::new(),
        stock: vec![0; rules.goods.len()],
        animals: {
            let mut held = vec![0; rules.herd_species.len()];
            held[0] = 3;
            held
        },
        hands: None,
        given_up: Some(0),
    });
    let (_, offered) = swap::place_totals(&rules, &dest).unwrap();
    assert_eq!(
        offered[cattle], 5,
        "two from the herd and three from the job"
    );
    spoken[cattle] = 4;
    swap::take_spoken(&rules, &mut dest, &spoken).unwrap();
    let (_, offered) = swap::place_totals(&rules, &dest).unwrap();
    assert_eq!(offered[cattle], 1);
}

#[test]
fn a_departing_trader_s_cargo_and_supplies_leave_its_holding_before_the_visit_counts_them_aboard() {
    // The trader's holding at home is 50 barley, 20 of which travel as cargo, and 10 more of its own oats are the supplies. The visit adds
    // what it carries to what the pop holds, so the pop must hold only what stays behind: 30 and 0 (never 70 and 10, which would count the
    // cargo twice and hand the visitor the wealth tie-break it does not have).
    let rules = rules();
    let (barley, oats) = (good(&rules, "barley"), good(&rules, "oats"));
    let mut home = home_place(&rules, "home", 40, 0);
    home.pops[PEASANT].stock[barley] = 50;
    home.pops[PEASANT].stock[oats] = 10;
    let (mut cargo, mut supplies) = (vec![0; rules.goods.len()], vec![0; rules.goods.len()]);
    cargo[barley] = 20;
    supplies[oats] = 10;
    swap::stage_departure(&rules, &mut home, PEASANT, &cargo, &supplies).unwrap();
    assert_eq!(home.pops[PEASANT].stock[barley], 30);
    assert_eq!(home.pops[PEASANT].stock[oats], 0);
    // What the pop does not hold is not taken (a job that did not gather it all), and a storehouse account gives up its part with the
    // physical stock.
    let mut home = home_place(&rules, "home", 40, 0);
    home.pops[PEASANT].stock[barley] = 0;
    let house = storehouse(&rules, &mut home);
    account(&mut home, house, PEASANT, barley, 15);
    cargo[barley] = 20;
    swap::stage_departure(
        &rules,
        &mut home,
        PEASANT,
        &cargo,
        &vec![0; rules.goods.len()],
    )
    .unwrap();
    assert_eq!(
        home.buildings[house]
            .storehouse_accounts
            .as_ref()
            .unwrap()
            .pops[PEASANT][barley],
        0
    );
    assert_eq!(home.buildings[house].stock[barley], 0);
    swap::place_totals(&rules, &home).expect("the accounts still balance");
}

#[test]
fn when_the_pack_binds_the_return_load_is_shared_among_the_wanted_goods_in_the_ratio_of_their_deficits_not_filled_in_order()
 {
    // On foot, a person's pack takes ten units of fish or honey (alike in weight and room): the trader lacks 30 fish and 10 honey and the
    // destination has plenty of both. The load is shared three to one in the ratio of the deficits, not eight of the first and none of
    // the second.
    let mut rules = rules_with(&[]);
    for item in &mut rules.goods {
        match item.id.as_str() {
            "fish" | "honey" => {
                item.weight_g = 1000;
                item.volume_ml = 2000;
            }
            "barley" => {
                item.weight_g = 100;
                item.volume_ml = 100;
            }
            _ => {}
        }
    }
    rules.expedition.foot_people = 1;
    let mut home = home_place(&rules, "home", 30, 40);
    set_price(&rules, &mut home, "fish", 40);
    set_price(&rules, &mut home, "honey", 40);
    want(&rules, &mut home, PEASANT, "honey", 10);
    let mut dest = dest_place(&rules, "dest", 100, 100);
    set_price(&rules, &mut dest, "honey", 1);
    spare(&rules, &mut dest, MONK, "honey", 100);
    let mut state = world(vec![home, dest]);
    let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
    look(&rules, &mut state, &mut ways, at(10, 2));
    let plan = plan_of(&state);
    let (fish, honey) = (
        plan.wanted[good(&rules, "fish")],
        plan.wanted[good(&rules, "honey")],
    );
    assert!(
        fish > 0 && honey > 0,
        "both are carried: {fish} fish, {honey} honey"
    );
    assert!(
        fish > honey && fish <= 3 * honey + 3,
        "in the ratio of 30 to 10 deficits, in whole units: {fish} fish, {honey} honey"
    );
}

#[test]
fn a_fitting_load_is_not_skipped_when_whole_units_move_value_to_a_lighter_good() {
    // Two goods of equal weight in the split, prices 1,000 and 2,000, 14 and 13 to be had, masses 10 and 1, room for 21. Worth 2,000
    // gives (2, 0) and fits, 3,000 gives (3, 0) and does not, 4,000 gives (2, 1) and fits again: halving stops at (2, 0), and the lighter
    // good is left behind. The search must give the load that fits and has the most: (2, 1).
    let price = |good: usize| [1000_u128, 2000][good];
    let mass = |units: &[u64]| 10 * units[0] + units[1];
    let fits = |units: &[u64]| mass(units) <= 21;
    let fitted = fit_shares(&[(0, 1_000_000), (1, 1_000_000)], &[14, 13], price, &fits);
    assert_eq!(fitted, [2, 1]);
    // The second case of review 4: prices 1,000 and 3,000, 41 and 99 to be had, masses 3 and 1, room for 12. The budget of 6,000 splits to
    // (3, 1) at mass 10; a search that stepped over that interval returned (4, 0). The load that grows by the smaller share of its weight
    // keeps both goods, reaches that split and then uses the room that is left: (3, 3), to which nothing can be added.
    let price = |good: usize| [1000_u128, 3000][good];
    let fits = |units: &[u64]| 3 * units[0] + units[1] <= 12;
    let fitted = fit_shares(&[(0, 1_000_000), (1, 1_000_000)], &[41, 99], price, &fits);
    assert_eq!(fitted, [3, 3]);
    assert!(!fits(&[4, 3]) && !fits(&[3, 4]), "nothing can be added");
    // Review 5: four identical packs, first-fit. Goods 0 to 2 at their limits of 3, 3 and 7, good 3 at its limit of 1 (three more aboard as
    // supplies): [3, 3, 3, 1] fits, [3, 3, 4, 1] does not, [3, 3, 5, 1] does again (the packing is not monotone). A good dropped after
    // one refused unit stops at 3; the load nothing can be added to has 5.
    let price = |good: usize| [2000_u128, 4000, 4000, 5000][good];
    let packing = |units: &[u64]| {
        units[0] <= 3 && units[1] <= 3 && units[3] <= 1 && matches!(units[2], 0..=3 | 5)
    };
    let weights = [(0, 1_u128), (1, 1), (2, 1), (3, 3)];
    assert_eq!(
        fit_shares(&weights, &[3, 3, 7, 1], price, &packing),
        [3, 3, 5, 1]
    );
    // A load that fits whole is taken whole, and the share of an unequal split is kept where it fits.
    let price = |good: usize| [1000_u128, 2000][good];
    let mass = |units: &[u64]| 10 * units[0] + units[1];
    let fits = |units: &[u64]| mass(units) <= 21;
    assert_eq!(fit_shares(&[(0, 1), (1, 1)], &[1, 1], price, &fits), [1, 1]);
}

#[test]
fn the_split_is_by_the_share_of_each_target_not_by_the_units_short() {
    // Worked example G (r5 3.5 item 3): 40 short of a target of 100 fish is a 40 % share, 200 short of 1,000 barley is 20 %: the fish
    // weigh twice the barley, though the barley is five times the units.
    let (fish, barley) = (share_weight(40, 100), share_weight(200, 1000));
    assert_eq!(fish, 2 * barley);
    // After a trip already coming brings 20 of the fish the shares are alike, and nothing about the units has changed.
    assert_eq!(share_weight(20, 100), barley);
    assert_eq!(
        share_weight(5, 0),
        share_weight(9, 0),
        "a good with no target counts whole"
    );
}

#[test]
fn the_payment_out_is_fitted_to_the_modes_of_the_way_out_and_the_return_to_those_of_the_way_home() {
    // A wagon carrying a currach goes out wholly by land and comes home wholly by water. On land the wagon has room for nine of the
    // heavy barley it pays with; on water the currach (carrying the wagon) has room for two. The payment is fitted to the way out, so it
    // is more than the water would hold, and the return (fish) is fitted to the way home, so it is what the water holds.
    let mut rules = rules_with(&["covered_wagon", "currach"]);
    rules.vehicles[0].carried_ml = Some(2_000_000);
    rules.vehicles[0].carries_g = 3_000_000;
    rules.vehicles[1].carries_g = 1_000_000;
    for item in &mut rules.goods {
        match item.id.as_str() {
            "barley" => {
                item.weight_g = 120_000;
                item.volume_ml = 1000;
            }
            "fish" => {
                item.weight_g = 10_000;
                item.volume_ml = 1000;
            }
            _ => {}
        }
    }
    let mut home = home_place(&rules, "home", 40, 40);
    // The supplies are oats, which are light, not the heavy barley, and a fish is worth the trip.
    set_price(&rules, &mut home, "barley", 5);
    set_price(&rules, &mut home, "fish", 400);
    home.trade.vehicles.push(Vehicle {
        number: 2,
        kind: 1,
        pop: peasant(),
        free: true,
        materials: None,
    });
    let mut state = world(vec![home, dest_place(&rules, "dest", 100, 100)]);
    state.trade.vehicles = 2;
    let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![water(6)]);
    look(&rules, &mut state, &mut ways, at(10, 2));
    let plan = the_job(&state, 0).expect("the trip is made");
    assert_eq!(plan.vehicles.len(), 2, "the wagon and the currach");
    assert!(plan.out.iter().all(|leg| leg.mode == Mode::Land));
    assert!(plan.back.iter().all(|leg| leg.mode == Mode::Water));
    let barley = plan.cargo[good(&rules, "barley")];
    assert!(
        barley > 2,
        "{barley} barley out: more than the water would hold"
    );
}

#[test]
fn four_identical_foot_packs_with_the_real_packer_take_the_load_a_refused_unit_did_not_end() {
    // Review 5 through the real `assign_leg`: four people on foot, each pack 21 litres and 14 kg; four goods of (5 L, 1 kg), (6 L, 1 kg),
    // (7 L, 1 kg) and (1 L, 6 kg), and three units of the last aboard as supplies. First-fit takes [3, 3, 3, 1], refuses [3, 3, 4, 1] and
    // takes [3, 3, 5, 1]: the fill goes on past the refusal.
    let mut rules = rules_with(&[]);
    rules.expedition.pack_g = 14_000;
    rules.expedition.pack_ml = 21_000;
    let goods = ["oats", "barley", "fish", "honey"].map(|id| good(&rules, id));
    for (index, (volume, weight)) in [(5000, 1000), (6000, 1000), (7000, 1000), (1000, 6000)]
        .into_iter()
        .enumerate()
    {
        rules.goods[goods[index]].volume_ml = volume;
        rules.goods[goods[index]].weight_g = weight;
    }
    let load = |units: [u64; 4]| {
        let mut load = vec![0_u64; rules.goods.len()];
        for (index, count) in units.into_iter().enumerate() {
            load[goods[index]] = count;
        }
        load
    };
    let mut supplies = vec![0_u64; rules.goods.len()];
    supplies[goods[3]] = 3;
    let fits = |trial: &[u64]| {
        let mut four = [0_u64; 4];
        for (index, slot) in four.iter_mut().enumerate() {
            *slot = trial[goods[index]];
        }
        expedition::assign_leg(&rules, &[], 4, &load(four), &supplies, Mode::Land).is_some()
    };
    let fits4 = |units: [u64; 4]| fits(&load(units));
    assert!(fits4([3, 3, 3, 1]));
    assert!(
        !fits4([3, 3, 4, 1]),
        "first-fit refuses one more of the third good"
    );
    assert!(fits4([3, 3, 5, 1]), "and takes two more");
    let price = |good: usize| [2000_u128, 4000, 4000, 5000][good];
    // The chooser works over the four goods as indices 0 to 3.
    let packing = |units: &[u64]| fits4([units[0], units[1], units[2], units[3]]);
    let fitted = fit_shares(
        &[(0, 1), (1, 1), (2, 1), (3, 3)],
        &[3, 3, 7, 1],
        price,
        &packing,
    );
    assert_eq!(fitted, [3, 3, 5, 1]);
}

#[test]
fn a_good_no_pack_can_hold_has_no_sizes_to_try_however_much_the_market_has() {
    // One shipped hardwood load on foot is heavier than a pack can hold: the bound is zero for any amount the market has to give, so the
    // fill never repacks 100,000 sizes of it; and the bound of a good that fits is what the carriers hold (a cart's room, the packs').
    let rules = rules();
    let hardwood = good(&rules, "hardwood");
    assert!(
        rules.goods[hardwood].weight_g > rules.expedition.pack_g,
        "the load is heavier than a pack"
    );
    assert_eq!(carrier_bound(&rules, &[], 4, hardwood, Mode::Land), 0);
    assert_eq!(carrier_bound(&rules, &[], 4, hardwood, Mode::Water), 0);
    let oats = good(&rules, "oats");
    let per_pack = (u128::from(rules.expedition.pack_g) / u128::from(rules.goods[oats].weight_g))
        .min(u128::from(rules.expedition.pack_ml) / u128::from(rules.goods[oats].volume_ml));
    assert_eq!(
        u128::from(carrier_bound(&rules, &[], 4, oats, Mode::Land)),
        4 * per_pack
    );
    // With a cart the carriers are the cart: its room, not the market's, is the most.
    let rules = rules_with(&["hand_cart"]);
    let cart = 0;
    let vehicles = [ExpeditionVehicle {
        number: 1,
        kind: u32::try_from(cart).unwrap(),
        worn_hours: ratio(0, 1),
    }];
    let bound = carrier_bound(&rules, &vehicles, 1, oats, Mode::Land);
    assert!(bound > 0 && bound < MOST_UNITS, "{bound}");
    assert_eq!(carrier_bound(&rules, &vehicles, 1, oats, Mode::Water), 0);
}

#[test]
fn a_look_plans_from_the_traders_own_shortfall_not_from_what_its_market_budget_funds() {
    // Review 6 (designer-7 on CK-445): the trader lacks 40 fish against its own target. What the market funds of that want (from the value
    // of what it has to spare) is another thing and does not limit the trip, nor does a waiting job's lack of the same fish take the
    // household's want away.
    let rules = rules();
    let fish = good(&rules, "fish");
    let mut home = home_place(&rules, "home", 40, 40);
    assert_eq!(
        swap::trader_view(&rules, &home, PEASANT).unwrap().deficit[fish],
        40
    );
    // Almost nothing to spare, so the market would fund little of the want.
    home.pops[PEASANT].stock[good(&rules, "barley")] -= 39;
    assert_eq!(
        swap::trader_view(&rules, &home, PEASANT).unwrap().deficit[fish],
        40
    );
    // A waiting job of the pop lacks five fish: the household's want of fish is still the 40, the job's five its own.
    let mut need = vec![0; rules.goods.len()];
    need[fish] = 5;
    home.trade.jobs.push(Job {
        number: 1,
        pop: peasant(),
        kind: JobKind::Build {
            vehicle: 0,
            trip: None,
        },
        made: 0,
        need_goods: need,
        need_animals: vec![0; rules.herd_species.len()],
        stock: vec![0; rules.goods.len()],
        animals: vec![0; rules.herd_species.len()],
        hands: None,
        given_up: None,
    });
    assert_eq!(
        swap::trader_view(&rules, &home, PEASANT).unwrap().deficit[fish],
        40
    );
}

#[test]
fn a_waiting_ox_carts_draught_cattle_are_the_jobs_want_not_the_traders_deficit_or_target() {
    // Review 7: a real ox-cart build order that lacks one head of cattle, for a pop that owns no herds. The cattle are the job's own want
    // (the exchange funds it), so the look's cattle deficit and share target stay what the pop's own herd position makes them, as the
    // fish case keeps the household's own.
    let rules = rules();
    let (cattle, fish) = (good(&rules, "cattle"), good(&rules, "fish"));
    let species = rules
        .herd_species
        .iter()
        .position(|item| item.animal == "cattle")
        .unwrap();
    let mut home = home_place(&rules, "home", 40, 40);
    let before = swap::trader_view(&rules, &home, PEASANT).unwrap();
    assert_eq!((before.deficit[cattle], before.target[cattle]), (0, 0));
    let mut need_animals = vec![0; rules.herd_species.len()];
    need_animals[species] = 1;
    home.trade.jobs.push(Job {
        number: 1,
        pop: peasant(),
        kind: JobKind::Build {
            vehicle: 0,
            trip: None,
        },
        made: 0,
        need_goods: vec![0; rules.goods.len()],
        need_animals,
        stock: vec![0; rules.goods.len()],
        animals: vec![0; rules.herd_species.len()],
        hands: None,
        given_up: None,
    });
    let (_, job_animals) = trade::lacks(&rules, &home);
    assert_eq!(job_animals[PEASANT][species], 1, "the order lacks the head");
    let after = swap::trader_view(&rules, &home, PEASANT).unwrap();
    assert_eq!(
        (after.deficit[cattle], after.target[cattle]),
        (0, 0),
        "the build's draught animal is no want of the trader's own herd"
    );
    assert_eq!(after.deficit[fish], 40, "the fish case is as it was");
    assert_eq!(after.deficit, before.deficit);
    assert_eq!(after.target, before.target);
}

#[test]
fn live_animals_are_bounded_by_the_drovers_on_land_and_the_room_aboard_on_water() {
    // Review 7: a high-stock market of cattle made the fill ask the packer about 100,001 sizes although 20 head fit. The bound of a live
    // good is what the real packer allows: one drover for every `animals_per_drover` of the people left after the moving vehicles' fewest
    // crews, and on water what the boats hold of the species' space aboard. It is the largest load the packer accepts, for every species.
    let rules = rules();
    let none = vec![0; rules.goods.len()];
    let packs = |rules: &Rules, vehicles: &[ExpeditionVehicle], people, good, units, mode| {
        let mut cargo = vec![0; rules.goods.len()];
        cargo[good] = units;
        expedition::assign_leg(rules, vehicles, people, &cargo, &none, mode).is_some()
    };
    let largest = |rules: &Rules, vehicles: &[ExpeditionVehicle], people, good, mode| {
        (0..=MOST_UNITS.min(400))
            .take_while(|units| packs(rules, vehicles, people, good, *units, mode))
            .last()
            .unwrap()
    };
    let cattle = good(&rules, "cattle");
    // One person on foot drives 20 head and no more.
    let per = rules.expedition.animals_per_drover;
    assert_eq!(carrier_bound(&rules, &[], 1, cattle, Mode::Land), per);
    assert!(packs(&rules, &[], 1, cattle, per, Mode::Land));
    assert!(!packs(&rules, &[], 1, cattle, per + 1, Mode::Land));
    assert_eq!(carrier_bound(&rules, &[], 3, cattle, Mode::Land), 3 * per);
    assert_eq!(carrier_bound(&rules, &[], 3, cattle, Mode::Water), 0);
    // Two people with an ox-cart: the cart takes its fewest crew first, one person drives.
    let cart = [ExpeditionVehicle {
        number: 1,
        kind: 0,
        worn_hours: ratio(0, 1),
    }];
    assert_eq!(carrier_bound(&rules, &cart, 2, cattle, Mode::Land), per);
    assert!(packs(&rules, &cart, 2, cattle, per, Mode::Land));
    assert!(!packs(&rules, &cart, 2, cattle, per + 1, Mode::Land));
    // Every species, on land with a few people and aboard a currach: the bound is exactly the most the packer accepts.
    let boats = rules_with(&["currach"]);
    let boat = [ExpeditionVehicle {
        number: 1,
        kind: 0,
        worn_hours: ratio(0, 1),
    }];
    for name in ["cattle", "sheep", "pigs"] {
        let animal = good(&boats, name);
        for people in [1, 2, 5] {
            assert_eq!(
                carrier_bound(&boats, &[], people, animal, Mode::Land),
                largest(&boats, &[], people, animal, Mode::Land).min(400),
                "{name} on foot, {people} people"
            );
        }
        let aboard = carrier_bound(&boats, &boat, 4, animal, Mode::Water);
        assert!(aboard < MOST_UNITS, "{name} aboard: {aboard}");
        assert_eq!(
            aboard,
            largest(&boats, &boat, 4, animal, Mode::Water),
            "{name} aboard a currach"
        );
    }
}

#[test]
fn a_trip_is_planned_whatever_the_base_price_that_funds_the_market_budget() {
    // The same stocks, quotes and shortfall of 40 fish, with the barley's base price (what the market values a pop's spare at when it
    // funds wants) changed: the look still plans the trip, paid in the barley it takes (a barter trip is not limited by the home budget).
    let planned = |base: u64| {
        let mut rules = rules();
        for item in &mut rules.goods {
            if item.id == "barley" {
                item.base_price = base;
            }
        }
        let mut state = world(vec![
            home_place(&rules, "home", 40, 40),
            dest_place(&rules, "dest", 100, 100),
        ]);
        let mut ways = Table::default().both("home", "dest", vec![fixed(6)], vec![fixed(6)]);
        look(&rules, &mut state, &mut ways, at(10, 2));
        the_job(&state, 0).is_some()
    };
    assert!(planned(1));
    assert!(planned(5));
    assert!(planned(100));
}
