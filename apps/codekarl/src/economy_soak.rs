use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as FmtWrite,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use ck_economy::{EconomyState, Rules};
use ck_sim::{Date, days_in_month};

const DEFAULT_SCENARIO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/scenarios/ireland-768"
);

/// Run the deterministic headless production soak against a real scenario.
#[expect(
    clippy::too_many_lines,
    reason = "The options, the scenario's load and the two modes (the CSV soak and the looks' report) are read in one place."
)]
pub(super) fn run(arguments: impl IntoIterator<Item = String>) -> Result<String, String> {
    let mut scenario = PathBuf::from(DEFAULT_SCENARIO);
    let mut years = 1_u16;
    let mut seed = 1_u64;
    let mut looks = false;
    let mut hours_limit: Option<u64> = None;
    let mut args = arguments.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--scenario" => scenario = PathBuf::from(args.next().ok_or("--scenario needs a path")?),
            "--years" => {
                years = args
                    .next()
                    .ok_or("--years needs a number")?
                    .parse()
                    .map_err(|_| "--years must be 1..=1000")?;
            }
            "--seed" => {
                seed = args
                    .next()
                    .ok_or("--seed needs a number")?
                    .parse()
                    .map_err(|_| "--seed must be an unsigned integer")?;
            }
            "--looks" => looks = true,
            "--hours" => {
                hours_limit = Some(
                    args.next()
                        .ok_or("--hours needs a number")?
                        .parse()
                        .map_err(|_| "--hours must be a whole number")?,
                );
            }
            "--help" => {
                return Ok(
                    "Usage: economy-soak [--scenario DIR] [--years N] [--seed N] [--looks [--hours N]]\n\
                     --looks opens the game with trade under way (CK-434), runs the traders' looks hour by hour (CK-433) and prints what the\n                     opening and looking cost instead of the CSV.\n"
                        .to_owned(),
                );
            }
            other => return Err(format!("unknown argument `{other}`; use --help")),
        }
    }
    if years == 0 || years > 1000 {
        return Err("--years must be 1..=1000".to_owned());
    }
    let scenario = scenario
        .canonicalize()
        .map_err(|error| format!("scenario {}: {error}", scenario.display()))?;
    let assets = scenario
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "scenario must be under assets/scenarios/<name>".to_owned())?;
    let map = ck_map::load_scenario(&scenario).map_err(|error| error.to_string())?;
    let rules = Rules::load(assets.join("game/economy.toml")).map_err(|error| error.to_string())?;
    let (state, _) = crate::game::economy::starting_economy_seeded(assets, &scenario, &map, seed)?;
    let quality_by_place = crate::game::economy::land_counts(&map, &rules.terrain_order)?
        .into_iter()
        .enumerate()
        .map(|(index, land)| {
            Ok((
                map.locations[index].id.clone(),
                ck_economy::mean_fertility(&land).map_err(|error| error.to_string())?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let land_by_place = crate::game::economy::land_counts(&map, &rules.terrain_order)?;
    let shelter_by_place = land_by_place
        .iter()
        .enumerate()
        .map(|(index, land)| {
            Ok((
                map.locations[index].id.clone(),
                ck_economy::natural_shelter(&rules, land).map_err(|error| error.to_string())?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let nearby_by_place =
        crate::game::setup::nearby_places(&map, crate::game::setup::max_recruit_range_m(&rules));
    let trade_by_place = crate::game::setup::trade_neighbours(&map);
    if looks {
        let tables = crate::game::setup::DayTables::build(&map, Some(&rules))?;
        // The shipped climate, which the game's day reads for the heating of each place (CK-446): none for a scenario without it.
        let weather = if assets.join("game/weather.toml").exists() {
            Some(crate::game::setup::WeatherData::load(assets, &map)?)
        } else {
            None
        };
        return looks_report(
            assets,
            &map,
            rules,
            state,
            seed,
            years,
            hours_limit,
            // The tables and the climate the game's day reads, built by the game's own code (CK-434 review 6).
            &tables,
            weather.as_ref(),
        );
    }
    let mut date = Date {
        year: map.start.year,
        month: map.start.month,
        day: 1,
        hour: 12,
    };
    if map.start.day > 1 {
        advance_month(&mut date)?;
    }
    simulate(
        &rules,
        state,
        date,
        seed,
        years,
        &quality_by_place,
        &shelter_by_place,
        &nearby_by_place,
        &trade_by_place,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "The soak passes each map-derived economy input explicitly so its simulation loop matches the production API."
)]
fn simulate(
    rules: &Rules,
    state: EconomyState,
    date: Date,
    seed: u64,
    years: u16,
    quality_by_place: &BTreeMap<String, u16>,
    shelter_by_place: &BTreeMap<String, u64>,
    nearby_by_place: &BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    trade_by_place: &BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
) -> Result<String, String> {
    simulate_with_supply(
        rules,
        state,
        date,
        seed,
        years,
        quality_by_place,
        shelter_by_place,
        nearby_by_place,
        trade_by_place,
        |_, _| {},
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "The test supply is applied at each noon alongside the normal soak inputs."
)]
fn simulate_with_supply(
    rules: &Rules,
    mut state: EconomyState,
    mut date: Date,
    seed: u64,
    years: u16,
    quality_by_place: &BTreeMap<String, u16>,
    shelter_by_place: &BTreeMap<String, u64>,
    nearby_by_place: &BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    trade_by_place: &BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    mut supply: impl FnMut(&Rules, &mut EconomyState),
) -> Result<String, String> {
    let mut csv = String::from(
        "seed,year,month,place,pop,pop_type,size,workers,dependents,needs_pm,intake_pm,food_days_milli,births,deaths,employed_workers,moved_people,crowding_pm,prices,production,stores,herds,wage_offers,losses\n",
    );
    for _ in 0..u32::from(years) * 12 {
        let days = days_in_month(date.year, date.month).map_err(|error| error.to_string())?;
        for day in 1..=days {
            date.day = day;
            supply(rules, &mut state);
            let ledger = ck_economy::run_daily_economy_with_trade_neighbours(
                rules,
                &mut state,
                date,
                quality_by_place,
                shelter_by_place,
                nearby_by_place,
                &BTreeMap::new(),
                &BTreeMap::new(),
                trade_by_place,
            )
            .map_err(|error| error.to_string())?;
            if day == 1 {
                append_month_rows(
                    &mut csv,
                    seed,
                    date,
                    rules,
                    &state,
                    &ledger,
                    shelter_by_place,
                )?;
            }
        }
        advance_month(&mut date)?;
    }
    Ok(csv)
}

/// A look report that says on standard error how far the opening has got, for a run that takes a while.
#[derive(Default)]
struct ProgressReport {
    report: ck_economy::LookReport,
    since: Option<Instant>,
}

impl ck_economy::LookObserver for ProgressReport {
    fn observe(&mut self, event: ck_economy::LookEvent) {
        let since = *self.since.get_or_insert_with(Instant::now);
        let looks = self.report.looks;
        self.report.observe(event);
        if self.report.looks != looks && self.report.looks.is_multiple_of(50) {
            eprintln!(
                "opening: {} first looks after {:.1} s",
                self.report.looks,
                since.elapsed().as_secs_f64()
            );
        }
    }
}

/// The opening's vehicles by kind and owner type, and its expeditions by what moves them (section 13 evidence), one line each.
fn opening_tally(rules: &Rules, state: &EconomyState) -> Result<String, String> {
    let mut vehicles: BTreeMap<(String, String), u64> = BTreeMap::new();
    let mut moving: BTreeMap<String, u64> = BTreeMap::new();
    let mut gathering = 0_u64;
    for place in &state.places {
        for vehicle in &place.trade.vehicles {
            let owner = place.pops[vehicle.pop as usize].kind.clone();
            let kind = rules.vehicles[vehicle.kind as usize].id.clone();
            *vehicles.entry((kind, owner)).or_insert(0) += 1;
        }
        for expedition in &place.trade.expeditions {
            let by = expedition.vehicles.first().map_or_else(
                || "on foot".to_owned(),
                |vehicle| rules.vehicles[vehicle.kind as usize].id.clone(),
            );
            *moving.entry(by).or_insert(0) += 1;
        }
        gathering += place
            .trade
            .jobs
            .iter()
            .filter(|job| matches!(job.kind, ck_economy::JobKind::Trade { .. }))
            .count() as u64;
    }
    let mut out = String::new();
    for ((kind, owner), count) in &vehicles {
        writeln!(out, "opening vehicles: {kind} of {owner} pops: {count}")
            .map_err(|error| error.to_string())?;
    }
    for (by, count) in &moving {
        writeln!(out, "opening expeditions part way out: {by}: {count}")
            .map_err(|error| error.to_string())?;
    }
    writeln!(out, "opening trade jobs gathering: {gathering}")
        .map_err(|error| error.to_string())?;
    Ok(out)
}

/// What a run of the traders' looks cost (CK-433, trade spec 11d.2 rule 3 and section 13): the shipped scenario's economy stepped hour
/// by hour with looks on, read-only evidence and not a game. The opening (CK-434) is run first, before the first tick, with every
/// trader's first look; the year is every hour after it.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "The harness passes each map-derived economy input explicitly, as the soak's own loop does, and reports each of its counts."
)]
fn looks_report(
    assets: &Path,
    map: &ck_map::Scenario,
    mut rules: Rules,
    mut state: EconomyState,
    seed: u64,
    years: u16,
    hours_limit: Option<u64>,
    tables: &crate::game::setup::DayTables,
    weather: Option<&crate::game::setup::WeatherData>,
) -> Result<String, String> {
    use crate::game::trade_ways::{NetworkWays, TradeNetwork, starting_states};
    rules.expedition.looks = true;
    let travel = std::fs::read_to_string(assets.join("game/travel.toml"))
        .map_err(|error| format!("travel.toml: {error}"))?;
    let network = std::sync::Arc::new(TradeNetwork {
        scenario: map.clone(),
        rules: ck_sim::travel::TravelRules::parse(&travel)?,
        neighbours: tables.trade_by_place.clone(),
        radius_m: rules.expedition.visit_radius_m,
    });
    let watch = crate::game::trade_ways::SearchWatch::default();
    let mut ways = NetworkWays::new(network, Some(std::sync::Arc::clone(&watch)));
    let states = starting_states(map);
    let mut date = Date {
        year: map.start.year,
        month: map.start.month,
        day: map.start.day,
        hour: map.start.hour,
    };
    let hours = hours_limit.unwrap_or(u64::from(years) * 365 * 24);
    let start = ck_sim::GameTime::from_date(date)
        .map_err(|error| error.to_string())?
        .hours();
    // The same game without trade, for the hunger and deaths to be read against (section 13), when a whole year is run.
    let baseline = if hours_limit.is_none() {
        // The same hours from the same first hour, the daily economy alone: the same calendar and reporting boundaries as the run with trade.
        let (mut alone, mut at, mut csv) = (state.clone(), date, String::new());
        for _ in 0..hours {
            if at.hour == 12 {
                let degree_hours = crate::game::setup::degree_hours_for(weather, &rules, at)?;
                let ledger = crate::game::setup::run_economy_day(
                    &rules,
                    &mut alone,
                    at,
                    &BTreeMap::new(),
                    &crate::game::setup::DayInputs {
                        quality_by_place: &tables.quality_by_place,
                        shelter_by_place: &tables.shelter_by_place,
                        nearby_by_place: &tables.nearby_by_place,
                        land_by_place: &tables.land_by_place,
                        trade_by_place: &tables.trade_by_place,
                        degree_hours: &degree_hours,
                    },
                )?;
                if at.day == 1 {
                    append_month_rows(
                        &mut csv,
                        seed,
                        at,
                        &rules,
                        &alone,
                        &ledger,
                        &tables.shelter_by_place,
                    )?;
                }
            }
            step_date(&mut at)?;
        }
        Some(summarize_months(&csv))
    } else {
        None
    };
    // The opening (CK-434, part F): the pops' goods, the fleets and every trader's first look, in one go before the first tick.
    let mut opening = ProgressReport::default();
    ways.begin(states.clone());
    let opened = Instant::now();
    let opened_with =
        ck_economy::run_opening(&rules, &mut state, start, seed, &mut ways, &mut opening)
            .map_err(|error| error.to_string())?;
    let opening_time = opened.elapsed();
    let opening = opening.report;
    let opening_searches = watch.lock().ok().map(|counts| counts.clone());
    let opening_vehicles = opening_tally(&rules, &state)?;
    let mut year = ck_economy::LookReport::default();
    let (mut day_time, mut day_max) = (Duration::ZERO, Duration::ZERO);
    let (mut looking_days, mut daily_total) = (0_u64, Duration::ZERO);
    // What the year's trade did, for the evidence: the expeditions made by what moves them, the goods they took out and meant to bring
    // back, and (with the daily economy's monthly rows) the hunger and deaths against the same year without trade.
    let mut moved = Movement::new(rules_goods(&rules), state.trade.expeditions);
    let mut monthly = String::new();
    for _ in 0..hours {
        if date.hour == 12 {
            let degree_hours = crate::game::setup::degree_hours_for(weather, &rules, date)?;
            let ledger = crate::game::setup::run_economy_day(
                &rules,
                &mut state,
                date,
                &BTreeMap::new(),
                &crate::game::setup::DayInputs {
                    quality_by_place: &tables.quality_by_place,
                    shelter_by_place: &tables.shelter_by_place,
                    nearby_by_place: &tables.nearby_by_place,
                    land_by_place: &tables.land_by_place,
                    trade_by_place: &tables.trade_by_place,
                    degree_hours: &degree_hours,
                },
            )?;
            if date.day == 1 {
                append_month_rows(
                    &mut monthly,
                    seed,
                    date,
                    &rules,
                    &state,
                    &ledger,
                    &tables.shelter_by_place,
                )?;
            }
            // After the noon exchange and before the hour's travel: the load an expedition that has just traded starts home with.
            moved.see(&rules, &state);
        }
        ways.begin(states.clone());
        let started = Instant::now();
        let mut report = ck_economy::LookReport::default();
        ck_economy::run_trade_hour_with_looks(&rules, &mut state, date, &mut ways, &mut report)
            .map_err(|error| error.to_string())?;
        let spent = started.elapsed();
        moved.see(&rules, &state);
        year.add(&report);
        if report.looks > 0 {
            day_time += spent;
            daily_total += spent;
        }
        if date.hour == 23 {
            if day_time > Duration::ZERO {
                looking_days += 1;
                day_max = day_max.max(day_time);
            }
            day_time = Duration::ZERO;
        }
        step_date(&mut date)?;
    }
    let counts = watch
        .lock()
        .map_err(|_| "the search tally is poisoned".to_owned())?
        .clone();
    let counts = &counts;
    let shown = |report: &ck_economy::LookReport| {
        format!(
            "looks {} (first {}, timer {}, rest {}, build {}), destinations evaluated {}, plans worked {}, plans refused early {}, trade jobs {}, builds {}",
            report.looks,
            report.by_first,
            report.by_timer,
            report.by_rest,
            report.by_build,
            report.destinations,
            report.plans,
            report.refused,
            report.trades,
            report.builds,
        )
    };
    let mut out = String::new();
    writeln!(
        out,
        "trade looks: seed {seed}, {hours} hours from {date_start}",
        date_start = format_args!(
            "{}-{:02}-{:02}",
            map.start.year, map.start.month, map.start.day
        )
    )
    .map_err(|error| error.to_string())?;
    writeln!(out, "opening (before the first tick): {}", shown(&opening))
        .map_err(|error| error.to_string())?;
    writeln!(
        out,
        "opening gave: {} vehicles as fleets and {} with a chosen plan, {} trips chosen of which {} started part way out ({} hours travelled in all), {} traders with no job given a last look",
        opened_with.fleets.len(),
        opened_with.looks.given,
        opened_with.looks.jobs,
        opened_with.looks.started.len(),
        opened_with.looks.started.iter().map(|(_, _, hours)| hours).sum::<u64>(),
        opened_with.last_looks,
    )
    .map_err(|error| error.to_string())?;
    out.push_str(&opening_vehicles);
    for (index, (units, pops)) in opened_with.goods.iter().enumerate() {
        if *units > 0 {
            writeln!(
                out,
                "opening goods: {} {units} units to {pops} pops",
                rules.goods[index].id
            )
            .map_err(|error| error.to_string())?;
        }
    }
    if let Some(counts) = &opening_searches {
        writeln!(
            out,
            "opening searches: forward {} ({:.1} s), reverse {} ({:.1} s), planners {} ({:.1} s), ways asked {} ({:.1} s, of which {} clipped in {:.1} s); the opening hour took {:.1} s",
            counts.forward,
            counts.forward_time.as_secs_f64(),
            counts.reverse,
            counts.reverse_time.as_secs_f64(),
            counts.planners,
            counts.planner_time.as_secs_f64(),
            counts.journeys,
            counts.journey_time.as_secs_f64(),
            counts.clips,
            counts.clip_time.as_secs_f64(),
            opening_time.as_secs_f64()
        )
        .map_err(|error| error.to_string())?;
    }
    writeln!(out, "after the opening: {}", shown(&year)).map_err(|error| error.to_string())?;
    writeln!(
        out,
        "all searches: forward {}, reverse {}, planners {}, ways {}",
        counts.forward, counts.reverse, counts.planners, counts.journeys
    )
    .map_err(|error| error.to_string())?;
    let mean = if looking_days == 0 {
        0.0
    } else {
        daily_total.as_secs_f64() / f64::from(u32::try_from(looking_days).unwrap_or(u32::MAX))
    };
    writeln!(
        out,
        "a day's looks after the opening: {looking_days} days with looks, largest {:.3} s, mean {mean:.3} s",
        day_max.as_secs_f64()
    )
    .map_err(|error| error.to_string())?;
    let (mut away, mut jobs) = (0_usize, 0_usize);
    for place in &state.places {
        away += place.trade.expeditions.len();
        jobs += place.trade.jobs.len();
    }
    writeln!(out, "at the end: {away} expeditions away, {jobs} jobs")
        .map_err(|error| error.to_string())?;
    moved.write(&mut out)?;
    let (deaths, hungry, rows) = summarize_months(&monthly);
    writeln!(
        out,
        "with trade: {deaths} deaths and {hungry} of {rows} pop-months holding under 60 days of food, over the {hours} hours run"
    )
    .map_err(|error| error.to_string())?;
    if let Some((deaths, hungry, rows)) = baseline {
        writeln!(
            out,
            "without trade (the accepted base, the daily economy alone): {deaths} deaths and {hungry} of {rows} pop-months holding under 60 days of food"
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(out)
}

/// The goods of the rules, by id.
fn rules_goods(rules: &Rules) -> Vec<String> {
    rules.goods.iter().map(|good| good.id.clone()).collect()
}

/// What the expeditions made did, tallied as each is first seen (section 13): by what moves them, and the goods each took out and meant to
/// bring back.
struct Movement {
    goods: Vec<String>,
    seen: u64,
    /// The load each expedition that has traded brings home, as last seen; counted once, when the expedition has unloaded and is gone.
    returning: BTreeMap<u64, Vec<u64>>,
    by_kind: BTreeMap<String, u64>,
    out: Vec<u64>,
    back: Vec<u64>,
}

impl Movement {
    fn new(goods: Vec<String>, seen: u64) -> Self {
        let count = goods.len();
        Self {
            goods,
            seen,
            returning: BTreeMap::new(),
            by_kind: BTreeMap::new(),
            out: vec![0; count],
            back: vec![0; count],
        }
    }

    /// Tallies the expeditions made since the last look by what moves them and what they took out, remembers the load each that has
    /// traded brings home, and counts that load once the expedition is gone from its place (it has unloaded at home). Seen at noon between
    /// the exchange and the hour's travel as well as after the hour, so a return of an hour or less is seen too; a trip still travelling
    /// when the run ends has brought nothing home yet and is not counted.
    fn see(&mut self, rules: &Rules, state: &EconomyState) {
        let mut present = BTreeSet::new();
        for expedition in state
            .places
            .iter()
            .flat_map(|place| &place.trade.expeditions)
        {
            present.insert(expedition.number);
            if expedition.number > self.seen {
                let by = expedition.vehicles.first().map_or_else(
                    || "on foot".to_owned(),
                    |vehicle| rules.vehicles[vehicle.kind as usize].id.clone(),
                );
                *self.by_kind.entry(by).or_insert(0) += 1;
                for (total, units) in self.out.iter_mut().zip(&expedition.plan.cargo) {
                    *total += units;
                }
            }
            if expedition.traded {
                self.returning
                    .insert(expedition.number, expedition.cargo.clone());
            }
        }
        let home: Vec<u64> = self
            .returning
            .keys()
            .copied()
            .filter(|number| !present.contains(number))
            .collect();
        for number in home {
            if let Some(load) = self.returning.remove(&number) {
                for (total, units) in self.back.iter_mut().zip(&load) {
                    *total += units;
                }
            }
        }
        self.seen = state.trade.expeditions;
    }

    fn write(&self, out: &mut String) -> Result<(), String> {
        for (by, count) in &self.by_kind {
            writeln!(out, "expeditions made after the opening: {by}: {count}")
                .map_err(|error| error.to_string())?;
        }
        for (index, good) in self.goods.iter().enumerate() {
            if self.out[index] > 0 || self.back[index] > 0 {
                writeln!(
                    out,
                    "goods moved between places: {good}: {} taken out, {} brought home and unloaded",
                    self.out[index], self.back[index]
                )
                .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }
}

/// The deaths, the pop-months under 60 days of food and the pop-months of a monthly CSV (the soak's own columns).
fn summarize_months(csv: &str) -> (u64, u64, u64) {
    let (mut deaths, mut hungry, mut rows) = (0_u64, 0_u64, 0_u64);
    for line in csv.lines().filter(|line| !line.starts_with("seed,")) {
        let fields: Vec<&str> = line.split(',').collect();
        let (Some(food), Some(dead)) = (fields.get(11), fields.get(13)) else {
            continue;
        };
        let (Ok(food), Ok(dead)) = (food.parse::<u64>(), dead.parse::<u64>()) else {
            continue;
        };
        rows += 1;
        deaths += dead;
        hungry += u64::from(food < 60_000);
    }
    (deaths, hungry, rows)
}

/// The next hour.
fn step_date(date: &mut Date) -> Result<(), String> {
    date.hour += 1;
    if date.hour == 24 {
        date.hour = 0;
        date.day += 1;
        if date.day > days_in_month(date.year, date.month).map_err(|error| error.to_string())? {
            advance_month(date)?;
            date.hour = 0;
        }
    }
    Ok(())
}

fn advance_month(date: &mut Date) -> Result<(), String> {
    if date.month == 12 {
        date.month = 1;
        date.year = date
            .year
            .checked_add(1)
            .ok_or("soak horizon exceeds calendar range")?;
    } else {
        date.month += 1;
    }
    date.day = 1;
    date.hour = 12;
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "One ordered CSV pass keeps each place and pop snapshot row aligned with the ledger."
)]
fn append_month_rows(
    csv: &mut String,
    seed: u64,
    date: Date,
    rules: &Rules,
    state: &EconomyState,
    ledger: &ck_economy::ProductionLedger,
    shelter_by_place: &BTreeMap<String, u64>,
) -> Result<(), String> {
    let goods = rules
        .goods
        .iter()
        .map(|good| good.id.as_str())
        .collect::<Vec<_>>();
    for (place_index, place) in state.places.iter().enumerate() {
        let shelter = shelter_by_place.get(&place.id).copied().unwrap_or(0);
        let housing =
            ck_economy::housing_fill(rules, place, shelter).map_err(|error| error.to_string())?;
        let place_ledger = ledger
            .places
            .iter()
            .find(|item| item.place_id == place.id)
            .ok_or_else(|| format!("monthly ledger is missing place `{}`", place.id))?;
        let mut herds = [0_u64; 3];
        for building in &place.buildings {
            for (column, animal) in ["cattle", "pigs", "sheep"].iter().enumerate() {
                if let Some(index) = rules
                    .herd_species
                    .iter()
                    .position(|species| species.animal == *animal)
                {
                    herds[column] = herds[column]
                        .checked_add(building.herds.get(index).copied().unwrap_or(0))
                        .ok_or("place herd total overflow")?;
                }
            }
        }
        let prices = goods
            .iter()
            .enumerate()
            .map(|(index, good)| format!("{good}={}", place.prices_milli[index]))
            .collect::<Vec<_>>()
            .join(";");
        let production = quantities(&goods, &place_ledger.produced);
        let losses = quantities(&goods, &place_ledger.lost);
        let herds_field = format!("cattle={};pigs={};sheep={}", herds[0], herds[1], herds[2]);
        let posted_wages = place
            .buildings
            .iter()
            .enumerate()
            .flat_map(|(index, building)| {
                building.wage_offers.iter().map(move |(job, offer)| {
                    format!("building{index}.{}.{job}={offer}", building.kind)
                })
            })
            .collect::<Vec<_>>()
            .join(";");
        let mut stores = Vec::new();
        for (index, pop) in place.pops.iter().enumerate() {
            stores.push(format!("pop{index}:{}", quantities(&goods, &pop.stock)));
        }
        for (index, building) in place.buildings.iter().enumerate() {
            stores.push(format!(
                "building{index}.{}:{}",
                building.kind,
                quantities(&goods, &building.stock)
            ));
            if let Some(accounts) = &building.storehouse_accounts {
                for (account_index, account) in accounts.pops.iter().enumerate() {
                    stores.push(format!(
                        "building{index}.account.pop{account_index}:{}",
                        quantities(&goods, account)
                    ));
                }
                stores.push(format!(
                    "building{index}.account.communal:{}",
                    quantities(&goods, &accounts.communal)
                ));
            }
        }
        let stores = stores.join("|");
        for (pop_index, pop) in place.pops.iter().enumerate() {
            let size = pop
                .workers
                .checked_add(pop.dependents)
                .ok_or("population size overflow")?;
            let _type_rules = rules
                .pop_types
                .iter()
                .find(|entry| entry.id == pop.kind)
                .ok_or_else(|| format!("unknown population type `{}`", pop.kind))?;
            let needs = format!(
                "{}:{}:{}",
                pop.consumption.necessities_pm,
                pop.consumption.niceties_pm,
                pop.consumption.luxuries_pm
            );
            let mut pop_goods = pop.stock.clone();
            for building in &place.buildings {
                if let Some(account) = building
                    .storehouse_accounts
                    .as_ref()
                    .and_then(|accounts| accounts.pops.get(pop_index))
                {
                    if account.len() != pop_goods.len() {
                        return Err(format!(
                            "storehouse account dimensions mismatch at `{}`",
                            place.id
                        ));
                    }
                    for (total, amount) in pop_goods.iter_mut().zip(account) {
                        *total = total
                            .checked_add(*amount)
                            .ok_or("pop food store overflow")?;
                    }
                }
            }
            let food_centi = pop_goods
                .iter()
                .enumerate()
                .try_fold(0_u128, |total, (index, amount)| {
                    total
                        .checked_add(
                            u128::from(*amount) * u128::from(rules.goods[index].hunger_centi),
                        )
                        .ok_or_else(|| "food value overflow".to_owned())
                })?
                .checked_add(
                    pop.consumption
                        .pantry_hunger_centi
                        .iter()
                        .map(|amount| u128::from(*amount))
                        .sum::<u128>(),
                )
                .ok_or("food pantry overflow")?;
            let daily_need = u128::from(size) * 100;
            let food_days_milli = if daily_need == 0 {
                0
            } else {
                food_centi.checked_mul(1000).ok_or("food days overflow")? / daily_need
            };
            let employed_workers = pop.employment.iter().try_fold(0_u64, |sum, employment| {
                sum.checked_add(employment.workers)
                    .ok_or("employment total overflow")
            })?;
            let moved_people = ledger
                .movement_by_pop
                .get(place_index)
                .and_then(|values| values.get(pop_index))
                .copied()
                .unwrap_or(0);
            let crowding_pm = housing
                .pops
                .get(pop_index)
                .and_then(|item| item.crowding_pm)
                .unwrap_or(0);
            let prices_field = csv_field(&prices);
            let place_field = csv_field(&place.id);
            let pop_field = csv_field(&format!("{pop_index}"));
            let production_field = csv_field(&production);
            let stores_field = csv_field(&stores);
            let herds_field = csv_field(&herds_field);
            let posted_wages_field = csv_field(&posted_wages);
            let losses_field = csv_field(&losses);
            let intake_pm = ledger
                .intake_pm
                .get(place_index)
                .and_then(|values| values.get(pop_index))
                .copied()
                .ok_or("daily intake ledger dimensions do not match places and pops")?;
            writeln!(csv, "{seed},{},{},{place_field},{pop_field},{},{},{},{},{},{},{food_days_milli},{},{},{employed_workers},{moved_people},{crowding_pm},{prices_field},{production_field},{stores_field},{herds_field},{posted_wages_field},{losses_field}",
                date.year, date.month, pop.kind, size, pop.workers, pop.dependents, needs, intake_pm,
                pop.counters.current.births, pop.counters.current.deaths)
                .expect("writing to a String is infallible");
        }
    }
    Ok(())
}

fn quantities(goods: &[&str], amounts: &[u64]) -> String {
    goods
        .iter()
        .zip(amounts)
        .filter(|(_, amount)| **amount > 0)
        .map(|(good, amount)| format!("{good}={amount}"))
        .collect::<Vec<_>>()
        .join(";")
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// An expedition of the trader that has traded and holds `cargo` (the load it starts home with).
    fn traded(number: u64, goods: usize, cargo: Vec<u64>) -> ck_economy::Expedition {
        ck_economy::Expedition {
            number,
            trader: 0,
            plan: ck_economy::Plan {
                destination: 1,
                out: Vec::new(),
                back: Vec::new(),
                vehicles: Vec::new(),
                people: Vec::new(),
                cargo: vec![0; goods],
                wanted: vec![0; goods],
                supplies: vec![0; goods],
                arrival: None,
            },
            groups: Vec::new(),
            vehicles: Vec::new(),
            cargo,
            supplies: vec![0; goods],
            pantry_centi: 0,
            gone: false,
            fed: None,
            phase: ck_economy::Phase::Homeward,
            traded: true,
            leg: 0,
            done: ck_economy::Hours::from_integer(0.into()),
            pace_pm: 1000,
            limiter: ck_economy::Limiter::Nothing,
            reckoned: vec![0; goods],
            reckoned_mode: ck_economy::Mode::Land,
            left: 0,
            arrived: Some(0),
            daily: false,
            loaded: vec![0; goods],
        }
    }

    /// The year witness counts what an expedition brings home when it has unloaded: one seen between the noon exchange and the hour's
    /// travel and gone from its place after it (a return of an hour or less) is counted, and one still travelling when the run ends is not.
    #[test]
    fn the_witness_counts_a_return_when_it_has_unloaded_and_not_while_it_travels() {
        let rules = ck_economy::fixtures::rules_with(&["ox_cart"]);
        let goods = rules.goods.len();
        let mut load = vec![0_u64; goods];
        load[0] = 10;
        let mut state = ck_economy::fixtures::world(vec![ck_economy::fixtures::home_place(
            &rules, "home", 40, 40,
        )]);
        state.places[0]
            .trade
            .expeditions
            .push(traded(1, goods, load.clone()));
        state.places[0]
            .trade
            .expeditions
            .push(traded(2, goods, load));
        state.trade.expeditions = 2;
        let mut moved = Movement::new(rules_goods(&rules), 0);
        // Seen after the noon exchange: both have traded and are starting home.
        moved.see(&rules, &state);
        assert_eq!(moved.back[0], 0, "nothing has been brought home yet");
        // Expedition 1 returned and unloaded within the hour; 2 is still on the road when the run ends.
        state.places[0].trade.expeditions.remove(0);
        moved.see(&rules, &state);
        assert_eq!(moved.back[0], 10, "the same-hour return is counted once");
        moved.see(&rules, &state);
        assert_eq!(
            moved.back[0], 10,
            "and only once; the one on the road is not counted"
        );
    }

    /// A running digest of everything the daily economy changes: prices, stores, herds, wages, staffing, the product carries and
    /// the production carries in the old unit, and every pop's numbers and stores.
    struct Digest(u64);

    impl std::fmt::Write for Digest {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            for byte in text.bytes() {
                self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            }
            Ok(())
        }
    }

    /// Production carries come in twelfths of the old unit from CK-406 on.
    const CARRY_UNIT: u128 = 12;

    fn state_digest(state: &EconomyState) -> u64 {
        let mut digest = Digest(0xcbf2_9ce4_8422_2325);
        for place in &state.places {
            write!(digest, "{} {:?}", place.id, place.prices_milli).unwrap();
            for building in &place.buildings {
                let scaled: Vec<u128> = building
                    .production_carries
                    .iter()
                    .flatten()
                    .map(|carry| {
                        let carry = *carry;
                        assert_eq!(carry % CARRY_UNIT, 0);
                        carry / CARRY_UNIT
                    })
                    .collect();
                write!(
                    digest,
                    " {} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
                    building.kind,
                    building.stock,
                    building.herds,
                    building.wage_offers,
                    building.wage_arrears,
                    building.wage_current,
                    building.fee_pending,
                    building.wage_history,
                    building.employment,
                    building.herd_product_carries,
                    scaled
                )
                .unwrap();
            }
            for pop in &place.pops {
                write!(
                    digest,
                    " {} {} {} {:?} {:?}",
                    pop.kind, pop.workers, pop.dependents, pop.stock, pop.employment
                )
                .unwrap();
            }
        }
        digest.0
    }

    /// The soak's own inputs for the shipped scenario, as `run` makes them: the start, the map's fertility, shelter and nearby
    /// places, and the first noon the soak runs on.
    #[test]
    fn a_year_of_the_shipped_scenario_follows_its_seeded_opening() {
        let scenario = PathBuf::from(DEFAULT_SCENARIO).canonicalize().unwrap();
        let assets = scenario.parent().and_then(Path::parent).unwrap();
        let map = ck_map::load_scenario(&scenario).unwrap();
        let rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        let (state, _) = crate::game::economy::starting_economy(assets, &scenario, &map).unwrap();
        let land_by_place = crate::game::economy::land_counts(&map, &rules.terrain_order).unwrap();
        let quality_by_place = land_by_place
            .iter()
            .enumerate()
            .map(|(index, land)| {
                (
                    map.locations[index].id.clone(),
                    ck_economy::mean_fertility(land).unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let shelter_by_place = land_by_place
            .iter()
            .enumerate()
            .map(|(index, land)| {
                (
                    map.locations[index].id.clone(),
                    ck_economy::natural_shelter(&rules, land).unwrap(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let nearby_by_place = crate::game::setup::nearby_places(
            &map,
            crate::game::setup::max_recruit_range_m(&rules),
        );
        let mut date = Date {
            year: map.start.year,
            month: map.start.month,
            day: 1,
            hour: 12,
        };
        if map.start.day > 1 {
            advance_month(&mut date).unwrap();
        }
        // The state each day starts from, a year of them from the scenario's own start: production reads each place's mean
        // fertility here, as the game's own scheduler gives it.
        let mut lines = String::new();
        let mut day = 0_u32;
        simulate_with_supply(
            &rules,
            state,
            date,
            1,
            1,
            &quality_by_place,
            &shelter_by_place,
            &nearby_by_place,
            &BTreeMap::new(),
            |_, state| {
                writeln!(lines, "{day} {:016x}", state_digest(state)).unwrap();
                day += 1;
            },
        )
        .unwrap();
        assert!(day >= 365);
        assert_eq!(lines, include_str!("economy_soak_year_accepted.txt"));
    }

    #[test]
    fn fixed_seed_and_horizon_produce_byte_identical_csv() {
        let scenario = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/scenarios/ireland-768")
            .to_string_lossy()
            .into_owned();
        let arguments = [
            "--scenario".to_owned(),
            scenario,
            "--years".to_owned(),
            "1".to_owned(),
            "--seed".to_owned(),
            "768".to_owned(),
        ];
        let first = run(arguments.clone()).expect("the Ireland soak runs");
        let second = run(arguments).expect("the same Ireland soak runs again");
        assert_eq!(first.as_bytes(), second.as_bytes());
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        let mut price_extrema = BTreeMap::<String, (u64, u64)>::new();
        for row in first.lines().skip(1) {
            let fields = row.split(',').collect::<Vec<_>>();
            for pair in fields[17].split(';') {
                let (good, price) = pair.split_once('=').unwrap();
                let price = price.parse::<u64>().unwrap();
                let extrema = price_extrema
                    .entry(good.to_owned())
                    .or_insert((price, price));
                extrema.0 = extrema.0.min(price);
                extrema.1 = extrema.1.max(price);
            }
        }
        for good in &rules.goods {
            let (minimum, maximum) = price_extrema[&good.id];
            assert!(
                minimum >= good.base_price * 200,
                "{} minimum {minimum}",
                good.id
            );
            assert!(
                maximum <= good.base_price * 5000,
                "{} maximum {maximum}",
                good.id
            );
        }
        assert!(
            price_extrema
                .values()
                .any(|(minimum, maximum)| minimum < maximum),
            "the fixture year must show a seasonal price change"
        );
        assert!(first.starts_with("seed,year,month,place,pop,pop_type,size,workers,dependents,needs_pm,intake_pm,food_days_milli,"));
        let first_row = first.lines().nth(1).unwrap();
        let row_fields = first_row.split(',').collect::<Vec<_>>();
        assert_eq!(row_fields.len(), 23);
        assert_eq!(
            &row_fields[..6],
            ["768", "768", "11", "armagh", "0", "peasant"]
        );
        assert_eq!(row_fields[7], "598", "Armagh peasant worker golden row");
        assert_eq!(row_fields[8], "401", "Armagh peasant dependent golden row");
        assert_eq!(row_fields[15], "0", "movement column for the first row");
        row_fields[12].parse::<u64>().expect("births are numeric");
        row_fields[13].parse::<u64>().expect("deaths are numeric");
        let posted_wages = row_fields[21];
        let wage_entries = posted_wages.split(';').collect::<Vec<_>>();
        assert!(
            !wage_entries.is_empty(),
            "the opening has committed job offers"
        );
        assert!(wage_entries.iter().all(|entry| {
            entry.starts_with("building")
                && entry
                    .split_once('=')
                    .is_some_and(|(_, wage)| wage.parse::<u64>().is_ok())
        }));
        assert!(row_fields[17].contains("oats=95000"));
        assert_eq!(
            row_fields[18],
            "apples=83;dairy=1394;ale=373;mead=38;linen=111;hardwood=3750"
        );
        assert!(row_fields[19].contains("pop0:"));
        let herds = row_fields[20].split(';').collect::<Vec<_>>();
        assert_eq!(herds.len(), 3);
        for (entry, animal) in herds.iter().zip(["cattle", "pigs", "sheep"]) {
            let (name, count) = entry.split_once('=').unwrap();
            assert_eq!(name, animal);
            count.parse::<u64>().expect("herd head is numeric");
        }
        assert_eq!(row_fields[22], "", "losses column");
    }

    #[test]
    fn ten_year_soak_keeps_each_herd_species_alive() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        rules.herd_opening.keep_pm = 1000;
        rules.herd_opening.spread = [1000, 1000];
        let mut scenario_data =
            ck_economy::ScenarioEconomy::load(assets.join("scenarios/ireland-768/economy.toml"))
                .unwrap();
        scenario_data.horses_always.clear();
        scenario_data.places.insert(
            "herd_fixture".to_owned(),
            ck_economy::StartOverride {
                culture: Some("mide_brega".to_owned()),
                religion: Some("christianity".to_owned()),
                monk_religion: Some("christianity".to_owned()),
                ..ck_economy::StartOverride::default()
            },
        );
        let (state, report) = ck_economy::starting_state(
            &rules,
            &scenario_data,
            &[ck_economy::StartLocation {
                id: "herd_fixture".to_owned(),
                kind: "monastery".to_owned(),
                size: 1,
                land: ck_economy::LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 100)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), 100_000)]),
                    fertile_cells: 100,
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 0,
                },
            }],
            1,
        )
        .expect("the non-starving herd fixture starts with its authored levels");
        assert!(report.is_empty(), "{report:?}");
        let output = simulate(
            &rules,
            state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
            768,
            10,
            &BTreeMap::from([("herd_fixture".to_owned(), 1000)]),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .expect("the 10-year fixture soak runs");
        for row in output.lines().skip(1) {
            let fields = row.split(',').collect::<Vec<_>>();
            assert_eq!(fields[3], "herd_fixture");
            let herds = fields[20];
            let current = herds
                .split(';')
                .map(|species| {
                    species
                        .split('=')
                        .nth(1)
                        .expect("CSV herd has a count")
                        .parse::<u64>()
                        .expect("CSV herd count is numeric")
                })
                .collect::<Vec<_>>();
            assert_eq!(current.len(), 3);
            assert!(current.iter().all(|count| *count > 0), "{row}");
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "This regression sets up the controlled island fixture and checks all survival, recruitment, and food-day outcomes together."
    )]
    fn fifty_year_island_soak_keeps_monks_alive_and_labourers_fed() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let scenario_directory = assets.join("scenarios/ireland-768");
        let map = ck_map::load_scenario(&scenario_directory).expect("Ireland scenario loads");
        let rules = Rules::load(assets.join("game/economy.toml")).expect("economy rules load");
        let (mut state, report) =
            crate::game::economy::starting_economy(&assets, &scenario_directory, &map)
                .expect("island starting economy loads");
        let island_ids = ["skellig_michael", "inishmurray"];
        assert!(
            report
                .iter()
                .all(|line| !island_ids.iter().any(|id| line.contains(id))),
            "the authored island starts fit their resources: {report:?}"
        );
        let authored_land = crate::game::economy::land_counts(&map, &rules.terrain_order)
            .expect("island land counts load");
        for id in island_ids {
            let location_index = map
                .locations
                .iter()
                .position(|location| location.id == id)
                .expect("island location exists");
            let place = state
                .places
                .iter()
                .find(|place| place.id == id)
                .expect("island starting place exists");
            let shelter = ck_economy::natural_shelter(&rules, &authored_land[location_index])
                .expect("island natural shelter is valid");
            let fill = ck_economy::housing_fill(&rules, place, shelter)
                .expect("island starting housing fills");
            assert!(
                fill.crowding_pm <= 1000,
                "{id} authored population starts at or below the housing soft cap, got {}‰",
                fill.crowding_pm
            );
        }
        state
            .places
            .retain(|place| island_ids.contains(&place.id.as_str()));
        let skellig_index = state
            .places
            .iter()
            .position(|place| place.id == "skellig_michael")
            .expect("Skellig Michael is in the scenario");
        let skellig = &mut state.places[skellig_index];
        let monk_index = skellig
            .pops
            .iter()
            .position(|pop| pop.kind == "monk")
            .expect("Skellig starts with monks");
        let pop_employment = skellig.pops[monk_index].employment.clone();
        for building in &mut skellig.buildings {
            building.employment = vec![building.employment[monk_index].clone()];
            if let Some(accounts) = &mut building.storehouse_accounts {
                accounts.pops = vec![accounts.pops[monk_index].clone()];
            }
            if !building.fee_carries.is_empty() {
                let communal = building.fee_carries.last().cloned().unwrap_or_default();
                building.fee_carries = vec![building.fee_carries[monk_index].clone(), communal];
            }
        }
        let mut monks = skellig.pops.remove(monk_index);
        skellig.pops.clear();
        monks.workers = 9;
        monks.dependents = 1;
        monks.employment = pop_employment;
        monks.employment.fill(ck_economy::Employment::default());
        let holdings_index = skellig
            .buildings
            .iter()
            .position(|building| building.kind == "holdings")
            .expect("Skellig has holdings");
        monks.employment[holdings_index].workers = 9;
        for (building, employment) in skellig.buildings.iter_mut().zip(&monks.employment) {
            building.employment[0] = employment.clone();
        }
        skellig.buildings[holdings_index]
            .wage_history
            .fill(u64::MAX);
        skellig.pops.push(monks);

        let inishmurray_index = state
            .places
            .iter()
            .position(|place| place.id == "inishmurray")
            .expect("Inishmurray is in the scenario");
        let inishmurray = &mut state.places[inishmurray_index];
        let peasant_index = inishmurray
            .pops
            .iter()
            .position(|pop| pop.kind == "peasant")
            .expect("Inishmurray has a peasant source");
        inishmurray.pops[peasant_index]
            .employment
            .fill(ck_economy::Employment::default());
        inishmurray.pops[peasant_index].religion = "christianity".to_owned();
        inishmurray.pops[peasant_index].workers = 1_000;
        inishmurray.pops[peasant_index].dependents = 400;
        for building in &mut inishmurray.buildings {
            building.employment[peasant_index] = ck_economy::Employment::default();
        }

        let oats = rules
            .goods
            .iter()
            .position(|good| good.id == "oats")
            .expect("oats are a food good");
        let land = crate::game::economy::land_counts(&map, &rules.terrain_order)
            .expect("Ireland catchments load");
        let mut quality_by_place = BTreeMap::new();
        let mut shelter_by_place = BTreeMap::new();
        for (location, land) in map.locations.iter().zip(&land) {
            if island_ids.contains(&location.id.as_str()) {
                quality_by_place.insert(
                    location.id.clone(),
                    ck_economy::mean_fertility(land).expect("island fertility is valid"),
                );
                shelter_by_place.insert(
                    location.id.clone(),
                    ck_economy::natural_shelter(&rules, land).expect("island shelter is valid"),
                );
            }
        }
        for id in island_ids {
            shelter_by_place.insert(id.to_owned(), 1_000_000);
        }
        let nearby = BTreeMap::from([(
            "skellig_michael".to_owned(),
            vec![ck_economy::NearbyPlace {
                id: "inishmurray".to_owned(),
                distance_m: 25_000,
            }],
        )]);
        let mut date = Date {
            year: map.start.year,
            month: map.start.month,
            day: 1,
            hour: 12,
        };
        if map.start.day > 1 {
            advance_month(&mut date).expect("scenario date advances");
        }
        let output = simulate_with_supply(
            &rules,
            state,
            date,
            768,
            50,
            &quality_by_place,
            &shelter_by_place,
            &nearby,
            &BTreeMap::new(),
            |rules, state| {
                // Explicit daily provisions replace the old impossible billion-bushel reserve. Each
                // pop receives the same target of 100 bushels, bounded by its ordinary cap and
                // remaining volume. Population sizes still determine the food available per head.
                for place in &mut state.places {
                    for pop in &mut place.pops {
                        let people = u128::from(pop.workers) + u128::from(pop.dependents);
                        let target = 100_u128;
                        let used: u128 = rules
                            .goods
                            .iter()
                            .zip(&pop.stock)
                            .filter(|(good, _)| !good.stacked_outside)
                            .map(|(good, units)| u128::from(*units) * u128::from(good.volume_ml))
                            .sum();
                        let volume_room = (people * 720_000).saturating_sub(used);
                        let cap = people * u128::from(rules.goods[oats].pop_limit_milli) / 1000;
                        let held = u128::from(pop.stock[oats]);
                        let offered = target
                            .saturating_sub(held)
                            .min(cap.saturating_sub(held))
                            .min(volume_room / u128::from(rules.goods[oats].volume_ml));
                        pop.stock[oats] +=
                            u64::try_from(offered).expect("fixture supply fits in u64");
                    }
                }
            },
        )
        .expect("the deterministic 50-year island soak runs");
        let mut monks = BTreeMap::<String, u64>::new();
        let mut labourers = BTreeMap::<String, u64>::new();
        let mut food_days = BTreeMap::<(String, String, String), u64>::new();
        for row in output.lines().skip(1) {
            let fields = row.split(',').collect::<Vec<_>>();
            let place = fields[3].to_owned();
            let pop_kind = fields[5];
            let size = fields[6].parse::<u64>().expect("population is numeric");
            if (place == "skellig_michael" || place == "inishmurray") && pop_kind == "monk" {
                monks
                    .entry(place.clone())
                    .and_modify(|minimum| *minimum = (*minimum).min(size))
                    .or_insert(size);
            }
            if place == "inishmurray" && pop_kind == "labourer" {
                labourers
                    .entry(place.clone())
                    .and_modify(|minimum| *minimum = (*minimum).min(size))
                    .or_insert(size);
            }
            if place == "inishmurray" && (pop_kind == "labourer" || pop_kind == "peasant") {
                let days = fields[11].parse::<u64>().expect("food days are numeric");
                food_days.insert(
                    (
                        fields[1].to_owned(),
                        fields[2].to_owned(),
                        pop_kind.to_owned(),
                    ),
                    days,
                );
            }
        }
        for place in ["skellig_michael", "inishmurray"] {
            assert!(
                monks.get(place).is_some_and(|minimum| *minimum > 0),
                "monks must survive at {place}; minimum {:?}; last row {:?}; Skellig monk movement total {}",
                monks.get(place),
                output
                    .lines()
                    .rev()
                    .find(|row| row.split(',').nth(3) == Some(place)),
                output
                    .lines()
                    .skip(1)
                    .filter_map(|row| {
                        let fields = row.split(',').collect::<Vec<_>>();
                        (fields[3] == "skellig_michael" && fields[5] == "monk")
                            .then(|| fields[15].parse::<u64>().expect("movement is numeric"))
                    })
                    .sum::<u64>()
            );
        }
        assert!(
            labourers
                .get("inishmurray")
                .is_some_and(|minimum| *minimum > 0),
            "the island fixture must retain labourers throughout the soak"
        );
        let skellig_monk_movement = output
            .lines()
            .skip(1)
            .filter_map(|row| {
                let fields = row.split(',').collect::<Vec<_>>();
                (fields[3] == "skellig_michael" && fields[5] == "monk")
                    .then(|| fields[15].parse::<u64>().expect("movement is numeric"))
            })
            .sum::<u64>();
        assert!(
            skellig_monk_movement > 0,
            "a Christian source within 30 km should replenish the open monk post"
        );
        for (year, month, kind) in food_days.keys().filter(|(_, _, kind)| kind == "peasant") {
            let peasant_days = food_days[&(year.clone(), month.clone(), kind.clone())];
            let labourer_days = food_days[&(year.clone(), month.clone(), String::from("labourer"))];
            assert!(
                labourer_days >= peasant_days,
                "Inishmurray labourers have {labourer_days} food days, below peasants' {peasant_days} in {year}-{month}"
            );
        }
    }
}
