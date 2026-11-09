use std::{
    fmt::Write as FmtWrite,
    path::{Path, PathBuf},
};

use ck_economy::{EconomyState, Rules};
use ck_sim::Date;

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
    let (mut state, _) = crate::game::economy::starting_economy(assets, &scenario, &map)?;
    let mut date = Date {
        year: map.start.year,
        month: map.start.month,
        day: 1,
        hour: 12,
    };
    if map.start.day > 1 {
        advance_month(&mut date)?;
    }
    let mut csv = String::from(
        "seed,year,month,place,pop,pop_type,size,workers,dependents,needs,food_days,prices,production,stores,herds,losses\n",
    );
    for _ in 0..u32::from(years) * 12 {
        let ledger = ck_economy::run_monthly_production(&rules, &mut state, date)
            .map_err(|error| error.to_string())?;
        append_month_rows(&mut csv, seed, date, &rules, &state, &ledger)?;
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
    for place in &state.places {
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
            let type_rules = rules
                .pop_types
                .iter()
                .find(|entry| entry.id == pop.kind)
                .ok_or_else(|| format!("unknown population type `{}`", pop.kind))?;
            let need = format!(
                "{}:{}:{}",
                type_rules.necessities_expectation_pm,
                type_rules.niceties_expectation_pm,
                type_rules.luxuries_expectation_pm
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
            let food_centi =
                pop_goods
                    .iter()
                    .enumerate()
                    .try_fold(0_u128, |total, (index, amount)| {
                        total
                            .checked_add(
                                u128::from(*amount) * u128::from(rules.goods[index].hunger_centi),
                            )
                            .ok_or_else(|| "food value overflow".to_owned())
                    })?;
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
            let losses_field = csv_field(&losses);
            writeln!(csv, "{seed},{},{},{place_field},{pop_field},{},{},{},{},{},{food_days_milli},{prices_field},{production_field},{stores_field},{herds_field},{losses_field}",
                date.year, date.month, pop.kind, size, pop.workers, pop.dependents, need)
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
        assert!(first.starts_with(concat!(
            "seed,year,month,place,pop,pop_type,size,workers,dependents,needs,food_days,prices,production,stores,herds,losses\n",
            "768,768,11,armagh,0,peasant,1000,600,400,1000:0:0,385370,oats=100000;barley=120000;peas_beans=120000;vegetables=40000;apples=150000;dairy=40000;beef=10000;pork=9000;mutton=8000;fish=30000;seabirds=20000;ale=60000;mead=300000;wine=600000;honey=120000;herbs=100000;wool=40000;beeswax=150000;hides=200000;cattle=2400000;pigs=600000;sheep=300000,apples=99;dairy=2285;ale=441;mead=44,pop0:oats=12000;barley=4000;peas_beans=1000;vegetables=2000;dairy=3470;beef=1000;pork=1000;mutton=1000|pop1:oats=2400;barley=800;peas_beans=200;vegetables=400;apples=36;dairy=639;beef=200;pork=200;mutton=200;ale=162;mead=16|pop2:oats=1800;barley=600;peas_beans=150;vegetables=300;apples=33;dairy=516;beef=150;pork=150;mutton=150;ale=146;mead=14|building0.holdings:oats=40;barley=12;apples=30;dairy=204;beef=4;pork=4;mutton=4;ale=133;mead=14|building1.farm:oats=40;barley=12;dairy=172;beef=4;pork=4;mutton=4|building2.storehouse:|building2.account.pop0:|building2.account.pop1:|building2.account.pop2:|building2.account.communal:,cattle=635;pigs=425;sheep=357,\n"
        )), "first literal horizon row remains stable");
    }
}
