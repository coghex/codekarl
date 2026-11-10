use std::{
    collections::BTreeMap,
    fmt::Write as FmtWrite,
    path::{Path, PathBuf},
};

use ck_economy::{EconomyState, Rules};
use ck_sim::{Date, days_in_month};

const DEFAULT_SCENARIO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/scenarios/ireland-768"
);

/// Run the deterministic headless production soak against a real scenario.
pub(super) fn run(arguments: impl IntoIterator<Item = String>) -> Result<String, String> {
    let mut scenario = PathBuf::from(DEFAULT_SCENARIO);
    let mut years = 1_u16;
    let mut seed = 1_u64;
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
            "--help" => {
                return Ok(
                    "Usage: economy-soak [--scenario DIR] [--years N] [--seed N]\n".to_owned(),
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
    let (state, _) = crate::game::economy::starting_economy(assets, &scenario, &map)?;
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
    let mut date = Date {
        year: map.start.year,
        month: map.start.month,
        day: 1,
        hour: 12,
    };
    if map.start.day > 1 {
        advance_month(&mut date)?;
    }
    simulate(&rules, state, date, seed, years, &quality_by_place)
}

fn simulate(
    rules: &Rules,
    mut state: EconomyState,
    mut date: Date,
    seed: u64,
    years: u16,
    quality_by_place: &BTreeMap<String, u16>,
) -> Result<String, String> {
    let mut csv = String::from(
        "seed,year,month,place,pop,pop_type,size,workers,dependents,needs_pm,intake_pm,food_days_milli,prices,production,stores,herds,posted_wages,losses\n",
    );
    for _ in 0..u32::from(years) * 12 {
        let days = days_in_month(date.year, date.month).map_err(|error| error.to_string())?;
        for day in 1..=days {
            date.day = day;
            let ledger = ck_economy::run_daily_economy_with_quality(
                rules,
                &mut state,
                date,
                quality_by_place,
            )
            .map_err(|error| error.to_string())?;
            if day == 1 {
                append_month_rows(&mut csv, seed, date, rules, &state, &ledger)?;
            }
        }
        advance_month(&mut date)?;
    }
    Ok(csv)
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
) -> Result<(), String> {
    let goods = rules
        .goods
        .iter()
        .map(|good| good.id.as_str())
        .collect::<Vec<_>>();
    for (place_index, place) in state.places.iter().enumerate() {
        let place_ledger = ledger
            .places
            .iter()
            .find(|item| item.place_id == place.id)
            .ok_or_else(|| format!("monthly ledger is missing place `{}`", place.id))?;
        let mut herds = [0_u64; 3];
        for building in &place.buildings {
            for (index, count) in building.herds.iter().take(3).enumerate() {
                herds[index] = herds[index]
                    .checked_add(*count)
                    .ok_or("place herd total overflow")?;
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
            .map(|(index, building)| {
                format!(
                    "building{index}.{}={}",
                    building.kind,
                    building.posted_wage()
                )
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
            writeln!(csv, "{seed},{},{},{place_field},{pop_field},{},{},{},{},{},{},{food_days_milli},{prices_field},{production_field},{stores_field},{herds_field},{posted_wages_field},{losses_field}",
                date.year, date.month, pop.kind, size, pop.workers, pop.dependents, needs, intake_pm)
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
            for pair in fields[12].split(';') {
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
        assert!(
            first
                .lines()
                .nth(1)
                .unwrap()
                .starts_with("768,768,11,armagh,0,peasant,1000,600,400,")
        );
        let first_row = first.lines().nth(1).unwrap();
        let mut row_fields = first_row.split(',').collect::<Vec<_>>();
        assert_eq!(row_fields.len(), 18);
        let posted_wages = row_fields.remove(16);
        let wage_entries = posted_wages.split(';').collect::<Vec<_>>();
        assert_eq!(wage_entries.len(), 3, "one posted wage per building");
        assert!(wage_entries.iter().enumerate().all(|(index, entry)| {
            entry.starts_with(&format!("building{index}."))
                && entry
                    .split_once('=')
                    .is_some_and(|(_, wage)| wage.parse::<u64>().is_ok())
        }));
        assert_eq!(
            row_fields.join(","),
            "768,768,11,armagh,0,peasant,1000,600,400,1000:0:0,1000,384370,oats=95000;barley=114000;peas_beans=114000;vegetables=42000;apples=157500;dairy=42000;beef=10500;pork=9450;mutton=8400;fish=30000;seabirds=20000;ale=63000;mead=315000;wine=600000;honey=126000;herbs=105000;wool=40000;beeswax=150000;hides=200000;cattle=2400000;pigs=600000;sheep=300000,apples=78;dairy=2285;ale=346;mead=34,pop0:oats=11968;barley=3989;peas_beans=997;vegetables=1994;dairy=3461;beef=997;pork=997;mutton=997|pop1:oats=2393;barley=797;peas_beans=199;vegetables=398;apples=27;dairy=637;beef=199;pork=199;mutton=199;ale=126;mead=11|pop2:oats=1795;barley=598;peas_beans=149;vegetables=299;apples=25;dairy=514;beef=149;pork=149;mutton=149;ale=114;mead=10|building0.holdings:oats=40;barley=12;apples=24;dairy=204;beef=4;pork=4;mutton=4;ale=104;mead=11|building1.farm:oats=40;barley=12;dairy=172;beef=4;pork=4;mutton=4|building2.storehouse:|building2.account.pop0:|building2.account.pop1:|building2.account.pop2:|building2.account.communal:,cattle=635;pigs=424;sheep=357,"
        );
    }

    #[test]
    fn ten_year_soak_keeps_each_herd_species_alive() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        let mut scenario_data =
            ck_economy::ScenarioEconomy::load(assets.join("scenarios/ireland-768/economy.toml"))
                .unwrap();
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
        )
        .expect("the 10-year fixture soak runs");
        for row in output.lines().skip(1) {
            let fields = row.split(',').collect::<Vec<_>>();
            assert_eq!(fields[3], "herd_fixture");
            let herds = fields[15];
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
}
