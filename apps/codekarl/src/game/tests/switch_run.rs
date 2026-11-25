//! CK-409: the shipped scenario's economy runs ten years with its levels switching methods. The run is read from outside the
//! engine: the level records are compared before and after each 1st, so the engine keeps no counter for it.

use std::{collections::BTreeMap, fmt::Write as _, fs, path::PathBuf};

use ck_economy::{EconomyState, LandCounts, Rules};
use ck_sim::{Date, days_in_month};

use crate::game::{
    economy::{land_counts, starting_economy},
    setup::{max_recruit_range_m, nearby_places},
};

fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// One level's move, as seen from outside: the day it was made, where, and the methods it went from and to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Move {
    pub(super) date: Date,
    pub(super) place: String,
    pub(super) building: String,
    pub(super) level: u32,
    pub(super) from: String,
    pub(super) to: String,
}

/// What a run of the shipped economy saw.
pub(super) struct Run {
    pub(super) moves: Vec<Move>,
    /// The first of the month and the place of each arrival of founders, with the head that arrived by kind of animal.
    pub(super) arrivals: Vec<(Date, String, Vec<u64>)>,
    /// The levels on each method (as `building.method`) at the start and at the end of each year run.
    pub(super) levels: Vec<(String, BTreeMap<String, u64>)>,
    pub(super) state: EconomyState,
}

type Key = (String, usize, u32);

fn snapshot(rules: &Rules, state: &EconomyState) -> BTreeMap<Key, (String, String, Option<u16>)> {
    let mut seen = BTreeMap::new();
    for place in &state.places {
        for (index, building) in place.buildings.iter().enumerate() {
            let Some(definition) = rules.buildings.iter().find(|b| b.id == building.kind) else {
                continue;
            };
            for record in &building.level_records {
                seen.insert(
                    (place.id.clone(), index, record.number),
                    (
                        building.kind.clone(),
                        definition.methods[record.method as usize].id.clone(),
                        record.last_switch_year,
                    ),
                );
            }
        }
    }
    seen
}

fn levels_by_method(rules: &Rules, state: &EconomyState) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for ((_, _, _), (kind, method, _)) in snapshot(rules, state) {
        *counts.entry(format!("{kind}.{method}")).or_default() += 1;
    }
    counts
}

/// The sum of rule 13 for each place and kind of animal over one production pass.
fn assert_herds_add_up(ledger: &ck_economy::ProductionLedger, date: Date) {
    for place in &ledger.places {
        for kind in 0..place.herds_before.len() {
            assert_eq!(
                place.herds_before[kind] + place.herd_births[kind] + place.herd_arrivals[kind],
                place.herds_after[kind] + place.herd_deaths[kind] + place.herd_slaughter[kind],
                "{} kind {kind} on {}-{:02}-{:02}",
                place.place_id,
                date.year,
                date.month,
                date.day
            );
        }
    }
}

/// The shipped scenario's economy at its start with the map's inputs to the daily step: land counts, farmland quality, shelter
/// and neighbours.
struct Shipped {
    rules: Rules,
    state: EconomyState,
    land_by_place: BTreeMap<String, LandCounts>,
    quality: BTreeMap<String, u16>,
    shelter: BTreeMap<String, u64>,
    nearby: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    scenario: ck_map::Scenario,
}

fn shipped_setup() -> Shipped {
    let directory = assets().join("scenarios/ireland-768");
    let scenario = ck_map::load_scenario(&directory).unwrap();
    let rules = Rules::load(assets().join("game/economy.toml")).unwrap();
    let (state, _) = starting_economy(&assets(), &directory, &scenario).unwrap();
    let lands = land_counts(&scenario, &rules.terrain_order).unwrap();
    let mut land_by_place = BTreeMap::<String, LandCounts>::new();
    let mut quality = BTreeMap::new();
    let mut shelter = BTreeMap::new();
    for (index, land) in lands.into_iter().enumerate() {
        let id = scenario.locations[index].id.clone();
        quality.insert(id.clone(), ck_economy::mean_fertility(&land).unwrap());
        shelter.insert(
            id.clone(),
            ck_economy::natural_shelter(&rules, &land).unwrap(),
        );
        land_by_place.insert(id, land);
    }
    let nearby = nearby_places(&scenario, max_recruit_range_m(&rules));
    Shipped {
        rules,
        state,
        land_by_place,
        quality,
        shelter,
        nearby,
        scenario,
    }
}

/// Runs the shipped scenario's economy through `years` years from its start, day by day, as the game's daily system does but
/// without the characters.
pub(super) fn run_shipped(years: u16) -> Run {
    let Shipped {
        rules,
        mut state,
        land_by_place,
        quality,
        shelter,
        nearby,
        scenario,
    } = shipped_setup();
    let mut date = Date {
        year: scenario.start.year,
        month: scenario.start.month,
        day: scenario.start.day,
        hour: 12,
    };
    let last_year = scenario.start.year + years;
    let mut moves = Vec::new();
    let mut arrivals = Vec::new();
    let mut levels = vec![("start".to_owned(), levels_by_method(&rules, &state))];
    let mut moved_in = BTreeMap::<Key, u16>::new();
    loop {
        let before = (date.day == 1).then(|| snapshot(&rules, &state));
        let ledger = ck_economy::run_daily_economy_with_land(
            &rules,
            &mut state,
            date,
            &quality,
            &shelter,
            &nearby,
            &BTreeMap::new(),
            &land_by_place,
        )
        .unwrap();
        if let Some(before) = before {
            assert_herds_add_up(&ledger, date);
            for place in &ledger.places {
                if place.herd_arrivals.iter().any(|head| *head > 0) {
                    arrivals.push((date, place.place_id.clone(), place.herd_arrivals.clone()));
                }
            }
            let after = snapshot(&rules, &state);
            for (key, (kind, method, _)) in &after {
                let (_, was, _) = &before[key];
                if was != method {
                    assert_ne!(
                        moved_in.insert(key.clone(), date.year),
                        Some(date.year),
                        "{key:?} moved twice in {}",
                        date.year
                    );
                    moves.push(Move {
                        date,
                        place: key.0.clone(),
                        building: kind.clone(),
                        level: key.2,
                        from: was.clone(),
                        to: method.clone(),
                    });
                }
            }
        }
        let days = days_in_month(date.year, date.month).unwrap();
        if date.day == days {
            if date.month == 12 {
                levels.push((
                    format!("end of {}", date.year),
                    levels_by_method(&rules, &state),
                ));
            }
            date.day = 1;
            if date.month == 12 {
                date.month = 1;
                date.year += 1;
            } else {
                date.month += 1;
            }
            // The run ends after the whole of the month of the tenth anniversary, so that the 1st of that month, the last monthly
            // pass of the span, is run.
            if (date.year, date.month) > (last_year, scenario.start.month) {
                break;
            }
        } else {
            date.day += 1;
        }
    }
    levels.push((
        "end of the run".to_owned(),
        levels_by_method(&rules, &state),
    ));
    Run {
        moves,
        arrivals,
        levels,
        state,
    }
}

/// The hand-off's table: moves each year by method from and to, the levels on each method at the start and at each year's
/// end, the places with most moves, and any level that moved in two years running.
pub(super) fn table(run: &Run) -> String {
    let mut out = String::new();
    let mut by_year = BTreeMap::<u16, BTreeMap<String, u64>>::new();
    for item in &run.moves {
        *by_year
            .entry(item.date.year)
            .or_default()
            .entry(format!("{}: {} -> {}", item.building, item.from, item.to))
            .or_default() += 1;
    }
    writeln!(
        out,
        "moves in each year, by building and method from and to"
    )
    .unwrap();
    for (year, counts) in &by_year {
        writeln!(out, "{year}: {} moves", counts.values().sum::<u64>()).unwrap();
        for (route, count) in counts {
            writeln!(out, "  {count:5} {route}").unwrap();
        }
    }
    writeln!(out, "\nlevels on each method").unwrap();
    for (when, counts) in &run.levels {
        writeln!(out, "{when}: {counts:?}").unwrap();
    }
    let mut per_place = BTreeMap::<&str, u64>::new();
    for item in &run.moves {
        *per_place.entry(item.place.as_str()).or_default() += 1;
    }
    let mut ranked: Vec<_> = per_place.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    writeln!(out, "\nplaces with the most moves").unwrap();
    for (place, count) in ranked.iter().take(10) {
        writeln!(out, "  {count:4} {place}").unwrap();
    }
    let mut years_of = BTreeMap::<(&str, &str, u32), Vec<u16>>::new();
    for item in &run.moves {
        years_of
            .entry((item.place.as_str(), item.building.as_str(), item.level))
            .or_default()
            .push(item.date.year);
    }
    writeln!(out, "\nlevels that moved in two years running").unwrap();
    let mut running = 0;
    for ((place, building, level), years) in &years_of {
        if years.windows(2).any(|pair| pair[1] == pair[0] + 1) {
            writeln!(out, "  {place} {building} level {level}: {years:?}").unwrap();
            running += 1;
        }
    }
    writeln!(out, "  ({running} levels)").unwrap();
    writeln!(out, "\nfounders that arrived: {} times", run.arrivals.len()).unwrap();
    for (date, place, heads) in run.arrivals.iter().take(10) {
        writeln!(
            out,
            "  {}-{:02}-{:02} {place} {heads:?}",
            date.year, date.month, date.day
        )
        .unwrap();
    }
    out
}

#[test]
// Long run: selected by app.long.
fn app_long_the_shipped_economy_runs_ten_years_with_levels_that_switch_without_an_error() {
    let run = run_shipped(10);
    let text = table(&run);
    if let Ok(path) = std::env::var("CK_SWITCH_TABLE") {
        fs::write(path, &text).unwrap();
    }
    assert!(
        run.state
            .places
            .iter()
            .all(|place| !place.buildings.is_empty()),
        "{text}"
    );
}

#[test]
fn a_card_lists_a_moved_level_under_its_new_method_from_that_day() {
    let Shipped {
        rules,
        mut state,
        land_by_place,
        quality,
        shelter,
        nearby,
        scenario,
    } = shipped_setup();
    let ids: Vec<String> = state.places.iter().map(|place| place.id.clone()).collect();
    let card = |state: &EconomyState, date: Date| {
        crate::game::view::build_with_land(Some(&rules), state, &ids, &land_by_place, date)
    };
    let mut date = Date {
        year: scenario.start.year,
        month: scenario.start.month,
        day: scenario.start.day,
        hour: 12,
    };
    let start = card(&state, date);
    // Through the first 1st that has a move in it.
    loop {
        let before = snapshot(&rules, &state);
        ck_economy::run_daily_economy_with_land(
            &rules,
            &mut state,
            date,
            &quality,
            &shelter,
            &nearby,
            &BTreeMap::new(),
            &land_by_place,
        )
        .unwrap();
        let moved = snapshot(&rules, &state) != before;
        if moved {
            break;
        }
        let days = days_in_month(date.year, date.month).unwrap();
        if date.day == days {
            date.day = 1;
            date.month = if date.month == 12 { 1 } else { date.month + 1 };
            date.year += u16::from(date.month == 1);
        } else {
            date.day += 1;
        }
        assert!(
            date.year < scenario.start.year + 2,
            "a level moved within a year"
        );
    }
    let now = card(&state, date);
    let mut changed = 0;
    for place in &state.places {
        for (index, building) in place.buildings.iter().enumerate() {
            let Some(definition) = rules.buildings.iter().find(|b| b.id == building.kind) else {
                continue;
            };
            if definition.methods.is_empty() {
                continue;
            }
            // The card lists the levels of each method from the records, in method order.
            let mut expected = Vec::new();
            for (method_index, method) in definition.methods.iter().enumerate() {
                let count = building
                    .level_records
                    .iter()
                    .filter(|record| record.method as usize == method_index)
                    .count() as u64;
                if count > 0 {
                    expected.push((method.id.clone(), count));
                }
            }
            let shown = &now.place(&place.id).unwrap().buildings[index].methods;
            assert_eq!(shown, &expected, "{} {}", place.id, building.kind);
            if shown != &start.place(&place.id).unwrap().buildings[index].methods {
                changed += 1;
            }
        }
    }
    assert!(changed > 0, "a card shows a level under its new method");
}

#[test]
#[ignore = "ten simulated years of the shipped game take several minutes; they run in the named methods.year group"]
fn the_shipped_game_runs_ten_years_without_an_error() {
    // The game as it runs: its scheduler with the characters, from its first noon on 9 October 768 through the noon of 9 October
    // 778. Every tick returns no error, every move carries its lock, and no level moves twice in a calendar year.
    let (_dir, mut prepared, _) = super::start_768::shipped(42);
    let end = ck_sim::GameTime::from_date(Date {
        year: 778,
        month: 10,
        day: 9,
        hour: 13,
    })
    .unwrap();
    let record = |state: &EconomyState| {
        let mut methods = BTreeMap::new();
        for place in &state.places {
            for (index, building) in place.buildings.iter().enumerate() {
                for level in &building.level_records {
                    methods.insert(
                        (place.id.clone(), index, level.number),
                        (level.method, level.last_switch_year),
                    );
                }
            }
        }
        methods
    };
    let mut snapshot = record(&prepared.simulation.state().economy);
    let mut moved_in = std::collections::BTreeSet::new();
    let mut moves = 0_u64;
    while prepared.simulation.time() < end {
        let errors = prepared.simulation.step().unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let date = prepared.simulation.date().unwrap();
        if date.hour == 13 && date.day == 1 {
            let now = record(&prepared.simulation.state().economy);
            for (key, (method, year)) in &now {
                if snapshot[key].0 != *method {
                    moves += 1;
                    assert_eq!(*year, Some(date.year), "{key:?} moved without its lock");
                    assert!(
                        moved_in.insert((key.clone(), date.year)),
                        "{key:?} moved twice in {}",
                        date.year
                    );
                }
            }
            snapshot = now;
        }
    }
    assert!(moves > 0, "levels moved in ten years");
}
