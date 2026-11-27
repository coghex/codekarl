//! Loading M9 economy inputs and adapting map catchments to land and fertility counts.

use std::path::Path;

use ck_economy::{EconomyState, LandCounts, Rules, ScenarioEconomy, StartLocation};
use ck_map::{Scenario, Surface};

/// Count owned land, fertility and sea cells; glaciers and lakes are not land.
pub(crate) fn land_counts(
    scenario: &Scenario,
    terrain_order: &[String],
) -> Result<Vec<LandCounts>, String> {
    land_counts_from_cells(
        &scenario.catchment_owners,
        &scenario.map.data.cells,
        &scenario.map.data.fertility,
        &scenario.map.data.glacier,
        terrain_order,
        scenario.locations.len(),
    )
}

fn land_counts_from_cells(
    owners: &[Option<usize>],
    cells: &[Surface],
    fertility: &[u16],
    glacier: &[bool],
    terrain_order: &[String],
    location_count: usize,
) -> Result<Vec<LandCounts>, String> {
    let mut counts = vec![LandCounts::default(); location_count];
    if owners.len() != cells.len() || fertility.len() != cells.len() || glacier.len() != cells.len()
    {
        return Err(
            "scenario catchment owner, fertility and glacier grids do not match map cells"
                .to_owned(),
        );
    }
    for (index, (owner, cell)) in owners.iter().zip(cells).enumerate() {
        let Some(owner) = owner else {
            continue;
        };
        if glacier[index] {
            continue;
        }
        let count = counts
            .get_mut(*owner)
            .ok_or_else(|| format!("catchment names unknown location index {owner}"))?;
        match cell {
            Surface::Sea { .. } => {
                count.sea = count
                    .sea
                    .checked_add(1)
                    .ok_or_else(|| "sea cell count overflow".to_owned())?;
            }
            Surface::Land { terrain, .. } => {
                let name = terrain_order.get(usize::from(*terrain)).ok_or_else(|| {
                    format!("terrain style index {terrain} is absent from economy rules")
                })?;
                let cells = count.land_cells.entry(name.clone()).or_default();
                *cells = cells
                    .checked_add(1)
                    .ok_or_else(|| format!("land {name} cell count overflow"))?;
                let total = count.fertility_pm.entry(name.clone()).or_default();
                *total = total
                    .checked_add(u64::from(fertility[index]))
                    .ok_or_else(|| format!("{name} fertility sum overflow"))?;
                if fertility[index] > 0 {
                    count.fertile_cells = count
                        .fertile_cells
                        .checked_add(1)
                        .ok_or_else(|| "fertile cell count overflow".to_owned())?;
                    let usable = count
                        .fertile_cells_by_terrain
                        .entry(name.clone())
                        .or_default();
                    *usable = usable
                        .checked_add(1)
                        .ok_or_else(|| format!("{name} fertile cell count overflow"))?;
                }
            }
            Surface::Lake => {}
        }
    }
    Ok(counts)
}

/// The shared economy rules of `asset_root`, or `None` for assets that predate M9 (no `game/economy.toml`), which have no
/// economy to show.
pub(crate) fn load_rules(asset_root: &Path) -> Result<Option<Rules>, String> {
    let path = asset_root.join("game/economy.toml");
    if !path.exists() {
        return Ok(None);
    }
    Rules::load(&path)
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Build the scenario's deterministic opening economy; fixture scenarios that
/// predate M9 deliberately retain the empty state.
#[cfg(test)]
pub(crate) fn starting_economy(
    asset_root: &Path,
    scenario_directory: &Path,
    scenario: &Scenario,
) -> Result<(EconomyState, Vec<String>), String> {
    starting_economy_seeded(asset_root, scenario_directory, scenario, 0)
}

pub(crate) fn starting_economy_seeded(
    asset_root: &Path,
    scenario_directory: &Path,
    scenario: &Scenario,
    seed: u64,
) -> Result<(EconomyState, Vec<String>), String> {
    let climate = if asset_root.join("game/weather.toml").exists() {
        let rules = load_rules(asset_root)?;
        if let Some(rules) = rules {
            let weather = super::setup::WeatherData::load(asset_root, scenario)?;
            weather.normal_fuel_climate(
                ck_sim::Date {
                    year: scenario.start.year,
                    month: scenario.start.month,
                    day: scenario.start.day,
                    hour: scenario.start.hour,
                },
                rules.fuel_heat_base_tenths,
            )?
        } else {
            std::collections::BTreeMap::new()
        }
    } else {
        std::collections::BTreeMap::new()
    };
    starting_economy_seeded_with_climate(asset_root, scenario_directory, scenario, seed, &climate)
}

pub(crate) fn starting_economy_seeded_with_climate(
    asset_root: &Path,
    scenario_directory: &Path,
    scenario: &Scenario,
    seed: u64,
    climate: &std::collections::BTreeMap<String, ck_economy::StartFuelClimate>,
) -> Result<(EconomyState, Vec<String>), String> {
    let rules_path = asset_root.join("game/economy.toml");
    if !rules_path.exists() {
        return Ok((EconomyState::default(), Vec::new()));
    }
    let rules = Rules::load(&rules_path).map_err(|error| error.to_string())?;
    let scenario_path = scenario_directory.join("economy.toml");
    let data = ScenarioEconomy::load(&scenario_path).map_err(|error| error.to_string())?;
    data.validate_cultures(&rules.peoples, &scenario_path.display().to_string())
        .map_err(|error| error.to_string())?;
    for location in &scenario.locations {
        if location.abandoned && data.places.contains_key(&location.id) {
            return Err(format!(
                "places.{}: abandoned location cannot have an economy entry",
                location.id
            ));
        }
    }
    for id in data.nobles.keys() {
        match scenario
            .locations
            .iter()
            .find(|location| location.id == *id)
        {
            None => return Err(format!("nobles.{id}: no such location")),
            Some(location) if location.abandoned => {
                return Err(format!(
                    "nobles.{id}: abandoned location cannot have a noble house"
                ));
            }
            Some(_) => {}
        }
    }
    let land = land_counts(scenario, &rules.terrain_order)?;
    let starts = scenario
        .locations
        .iter()
        .zip(land)
        .filter(|(location, _)| !location.abandoned)
        .map(|(location, land)| StartLocation {
            id: location.id.clone(),
            kind: scenario.location_kinds.kinds[location.kind_index]
                .id
                .clone(),
            size: location.size,
            land,
        })
        .collect::<Vec<_>>();
    ck_economy::starting_state_at_seed_with_fuel_climate(
        &rules,
        &data,
        &starts,
        ck_sim::Date {
            year: scenario.start.year,
            month: scenario.start.month,
            day: scenario.start.day,
            hour: scenario.start.hour,
        },
        seed,
        climate,
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn ireland_assets() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    /// Every place of `inland` starts with no level of a sea method.
    fn assert_no_sea_levels(rules: &Rules, state: &EconomyState, inland: &[String]) {
        for place in state
            .places
            .iter()
            .filter(|place| inland.contains(&place.id))
        {
            for building in &place.buildings {
                let kind = rules
                    .buildings
                    .iter()
                    .find(|item| item.id == building.kind)
                    .unwrap();
                for (method, levels) in kind.methods.iter().zip(&building.method_levels) {
                    assert!(
                        method.resource != "sea" || *levels == 0,
                        "{}: {} starts with {levels} levels of the sea method {}",
                        place.id,
                        building.kind,
                        method.id
                    );
                }
            }
        }
    }

    #[test]
    fn shipped_start_assigns_all_levels_and_fuel_only_to_farms() {
        let assets = ireland_assets();
        let directory = assets.join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).unwrap();
        let rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        let (state, _) = starting_economy(&assets, &directory, &scenario).unwrap();
        let mut fuel_levels = 0;
        let mut wood_places = 0;
        let mut turf_places = 0;
        let mut neither_places = 0;
        for place in &state.places {
            let farm = place
                .buildings
                .iter()
                .find(|building| building.kind == "farm");
            let wood = farm
                .and_then(|b| b.method_levels.get(2))
                .copied()
                .unwrap_or(0)
                > 0;
            let turf = farm
                .and_then(|b| b.method_levels.get(3))
                .copied()
                .unwrap_or(0)
                > 0;
            wood_places += usize::from(wood);
            turf_places += usize::from(turf);
            neither_places += usize::from(!wood && !turf);
        }
        let counts = land_counts(&scenario, &rules.terrain_order).unwrap();
        let cannot_cut = counts
            .iter()
            .filter(|land| {
                land.land_cells.get("forest").copied().unwrap_or(0) < 4
                    && land.land_cells.get("bog").copied().unwrap_or(0) < 2
            })
            .count();
        eprintln!(
            "fuel start: wood={wood_places} turf={turf_places} neither_started={neither_places} cannot_cut={cannot_cut}"
        );
        for place in &state.places {
            for building in &place.buildings {
                if building.kind == "storehouse" {
                    continue;
                }
                let definition = rules
                    .buildings
                    .iter()
                    .find(|b| b.id == building.kind)
                    .unwrap();
                assert_eq!(
                    building.levels,
                    building.method_levels.iter().sum::<u32>(),
                    "{} {}",
                    place.id,
                    building.kind
                );
                for (method, count) in definition.methods.iter().zip(&building.method_levels) {
                    if ["forest", "bog"].contains(&method.resource.as_str()) {
                        assert_eq!(building.kind, "farm");
                        fuel_levels += *count;
                    }
                }
            }
        }
        assert!(fuel_levels > 0);
    }

    #[test]
    fn the_shipped_start_places_every_level_on_a_method_and_keeps_armaghs_demesne() {
        let assets = ireland_assets();
        let directory = assets.join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("the scenario loads");
        let rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        let (state, report) = starting_economy(&assets, &directory, &scenario).unwrap();
        for place in &state.places {
            for building in &place.buildings {
                let kind = rules
                    .buildings
                    .iter()
                    .find(|item| item.id == building.kind)
                    .unwrap();
                if kind.methods.is_empty() {
                    continue;
                }
                assert_eq!(
                    building.levels,
                    building.method_levels.iter().sum::<u32>(),
                    "{}: every level of {} is on a method",
                    place.id,
                    building.kind
                );
            }
        }
        assert!(
            report
                .iter()
                .all(|line| !line.starts_with("glendalough farm: cut levels")),
            "fuel methods now have room for Glendalough's farm levels: {report:?}"
        );
        let armagh = state
            .places
            .iter()
            .find(|place| place.id == "armagh")
            .unwrap();
        let demesne = armagh
            .buildings
            .iter()
            .find(|building| building.kind == "demesne")
            .expect("a shipped noble seat with a demesne");
        assert_eq!(demesne.levels, 1, "the demesne levels Armagh keeps today");
    }

    #[test]
    fn shipped_places_with_no_sea_cell_open_no_sea_level_and_want_the_fish_they_lack() {
        let assets = ireland_assets();
        let directory = assets.join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("the scenario loads");
        let rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        let land = land_counts(&scenario, &rules.terrain_order).unwrap();
        let (mut state, _) = starting_economy(&assets, &directory, &scenario).unwrap();
        let fish = rules
            .goods
            .iter()
            .position(|good| good.id == "fish")
            .unwrap();
        let inland = scenario
            .locations
            .iter()
            .zip(&land)
            .filter(|(location, counts)| !location.abandoned && counts.sea == 0)
            .map(|(location, _)| location.id.clone())
            .collect::<Vec<_>>();
        assert!(
            !inland.is_empty(),
            "the shipped scenario has places with no sea cell"
        );
        assert_no_sea_levels(&rules, &state, &inland);
        // The expected places come from the opening state: a pop is short of fish when what it holds, in its stock and in
        // its storehouse account, is below its 240-day target of the profile's own fish share (every shipped profile sums
        // to 1000 permille, so the share needs no scaling).
        let hunger = u128::from(rules.goods[fish].hunger_centi);
        let short_of_fish = |place: &ck_economy::PlaceState| {
            place.pops.iter().enumerate().any(|(index, pop)| {
                let shares = &rules.diet_profiles[&pop.kind];
                assert_eq!(
                    shares.values().map(|share| u32::from(*share)).sum::<u32>(),
                    1000
                );
                let share = u128::from(shares.get("fish").copied().unwrap_or(0));
                let people = u128::from(pop.workers + pop.dependents);
                let target = (share * people * 100 * 240).div_ceil(1000 * hunger);
                let account: u128 = place
                    .buildings
                    .iter()
                    .filter_map(|building| building.storehouse_accounts.as_ref())
                    .map(|accounts| u128::from(accounts.pops[index][fish]))
                    .sum();
                u128::from(pop.stock[fish]) + account < target
            })
        };
        let expected = state
            .places
            .iter()
            .filter(|place| inland.contains(&place.id) && short_of_fish(place))
            .map(|place| place.id.clone())
            .collect::<Vec<_>>();
        assert!(
            !expected.is_empty(),
            "some place with no sea cell starts short of fish"
        );
        // The orders the first noon's price step cached, on the scenario's own start date.
        ck_economy::run_daily_economy(
            &rules,
            &mut state,
            ck_sim::Date {
                year: scenario.start.year,
                month: scenario.start.month,
                day: scenario.start.day,
                hour: 12,
            },
        )
        .unwrap();
        for place in state
            .places
            .iter()
            .filter(|place| inland.contains(&place.id))
        {
            let orders = place
                .last_market_orders
                .as_ref()
                .expect("the day's orders are kept");
            assert_eq!(
                orders.buy_units[fish] > 0,
                expected.contains(&place.id),
                "{}: the first noon's fish buy order must match the opening deficit",
                place.id
            );
        }
    }

    #[test]
    fn land_counts_fixture_assigns_boundary_cells_once_and_ignores_water_unowned_and_glacier() {
        let owners = [Some(0), Some(0), Some(0), None, Some(1), Some(1)];
        let cells = [
            Surface::Sea { depth: 2 },
            Surface::Lake,
            Surface::Land {
                terrain: 0,
                usable: false,
            },
            Surface::Land {
                terrain: 0,
                usable: true,
            },
            Surface::Land {
                terrain: 1,
                usable: true,
            },
            Surface::Land {
                terrain: 0,
                usable: true,
            },
        ];
        let fertility = [0, 0, 600, 900, 400, 0];
        let glacier = [false, false, false, false, false, true];
        let terrain = ["pasture".to_owned(), "forest".to_owned()];
        assert_eq!(
            land_counts_from_cells(&owners, &cells, &fertility, &glacier, &terrain, 2).unwrap(),
            [
                LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 1)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), 600)]),
                    fertile_cells: 1,
                    fertile_cells_by_terrain: BTreeMap::from([("pasture".to_owned(), 1)]),
                    sea: 1
                },
                LandCounts {
                    land_cells: BTreeMap::from([("forest".to_owned(), 1)]),
                    fertility_pm: BTreeMap::from([("forest".to_owned(), 400)]),
                    fertile_cells: 1,
                    fertile_cells_by_terrain: BTreeMap::from([("forest".to_owned(), 1)]),
                    sea: 0,
                },
            ]
        );
    }

    #[test]
    fn ireland_start_loads_and_land_counts_exclude_lakes_unusable_land_and_unowned_cells() {
        let assets = ireland_assets();
        let directory = assets.join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).unwrap();
        let rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        let counts = land_counts(&scenario, &rules.terrain_order).unwrap();
        assert_eq!(counts.len(), scenario.locations.len());

        let mut expected = vec![LandCounts::default(); scenario.locations.len()];
        for (index, (owner, cell)) in scenario
            .catchment_owners
            .iter()
            .zip(&scenario.map.data.cells)
            .enumerate()
        {
            let Some(owner) = owner else {
                continue;
            };
            if scenario.map.data.glacier[index] {
                continue;
            }
            match cell {
                Surface::Sea { .. } => expected[*owner].sea += 1,
                Surface::Land { terrain, .. } => {
                    let name = &rules.terrain_order[usize::from(*terrain)];
                    *expected[*owner].land_cells.entry(name.clone()).or_default() += 1;
                    *expected[*owner]
                        .fertility_pm
                        .entry(name.clone())
                        .or_default() += u64::from(scenario.map.data.fertility[index]);
                    if scenario.map.data.fertility[index] > 0 {
                        expected[*owner].fertile_cells += 1;
                        *expected[*owner]
                            .fertile_cells_by_terrain
                            .entry(name.clone())
                            .or_default() += 1;
                    }
                }
                Surface::Lake => {}
            }
        }
        assert_eq!(
            counts, expected,
            "owned glaciers and lakes are excluded from land and fertility"
        );
        assert!(counts.iter().any(|count| count.sea > 0));
        assert!(counts.iter().any(|count| !count.land_cells.is_empty()));

        let (state, report) = starting_economy(&assets, &directory, &scenario).unwrap();
        assert_eq!(state.places.len(), 123);
        assert_eq!(
            report,
            [
                "killeany starts over building cap: footprint 50400 m², prepared 70400 m², max_buildable 47600 m²",
                "tory starts over building cap: footprint 24200 m², prepared 44200 m², max_buildable 23600 m²",
                "rechru starts over building cap: footprint 50400 m², prepared 70400 m², max_buildable 24050 m²",
                "inis_patraic starts over building cap: footprint 24200 m², prepared 44200 m², max_buildable 23600 m²",
            ]
        );
        assert!(
            report
                .iter()
                .filter(|line| line.contains(" farm: cut levels "))
                .all(|line| !line.ends_with("-> 0 (no method has enough land or sea)")),
            "no place loses every authored farm level: {report:?}"
        );
        for id in ["emly", "uisneach", "monasterboice", "lagore"] {
            assert!(
                state
                    .places
                    .iter()
                    .find(|place| place.id == id)
                    .unwrap()
                    .buildings
                    .iter()
                    .any(|building| building.kind == "farm" && building.levels > 0),
                "{id} keeps its authored farm levels"
            );
        }
        let mayo = state
            .places
            .iter()
            .find(|place| place.id == "mayo")
            .unwrap();
        assert!(
            mayo.pops
                .iter()
                .any(|pop| pop.kind == "monk" && pop.culture == "northumbrian")
        );
    }

    /// The smallest household whose workers, at 400‰ dependents, number at least `workers`.
    fn household_heads(workers: u64) -> u64 {
        (workers..=workers * 2)
            .find(|heads| heads - heads * 400 / 1000 >= workers)
            .unwrap()
    }

    #[test]
    fn the_768_start_gives_each_of_the_34_noble_houses_a_demesne_and_its_labourers() {
        let assets = ireland_assets();
        let directory = assets.join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).unwrap();
        let data = ScenarioEconomy::load(directory.join("economy.toml")).unwrap();
        let (state, report) = starting_economy(&assets, &directory, &scenario).unwrap();
        assert!(
            report.iter().all(|line| !line.contains(" demesne: ")),
            "no demesne level is cut at the 768 start: {report:?}"
        );
        // 33 houses of the P1 start and, from CK-370, Carraig Aille (one level).
        assert_eq!(data.nobles.len(), 34);

        let (mut heads_added, mut jobs_filled, mut nobles_total) = (0, 0, 0);
        for (id, noble) in &data.nobles {
            let place = state.places.iter().find(|place| place.id == *id).unwrap();
            // One noble pop of the given culture, religion and heads.
            let nobles = place
                .pops
                .iter()
                .filter(|pop| pop.kind == "noble")
                .collect::<Vec<_>>();
            assert_eq!(nobles.len(), 1, "{id}");
            assert_eq!(
                (nobles[0].culture.as_str(), nobles[0].religion.as_str()),
                (noble.culture.as_str(), noble.religion.as_str()),
                "{id}"
            );
            assert_eq!(
                nobles[0].workers + nobles[0].dependents,
                noble.people,
                "{id}"
            );
            nobles_total += noble.people;

            // The demesne at its authored levels, with the nobles as its owners.
            let demesne_index = place
                .buildings
                .iter()
                .position(|building| building.kind == "demesne")
                .unwrap();
            let demesne = &place.buildings[demesne_index];
            assert_eq!(demesne.levels, noble.demesne_levels, "{id}");
            let noble_index = place
                .pops
                .iter()
                .position(|pop| pop.kind == "noble")
                .unwrap();
            assert_eq!(
                demesne.employment[noble_index]
                    .jobs
                    .get("steward")
                    .copied()
                    .unwrap_or(0),
                u64::from(noble.demesne_levels),
                "{id}: one steward per level"
            );

            // The labourers: one pop of the place's culture and religion; the heads added and the jobs
            // filled are separate numbers.
            let place_culture = data.places[id].culture.as_deref().unwrap();
            let labourers = place
                .pops
                .iter()
                .enumerate()
                .filter(|(_, pop)| pop.kind == "labourer")
                .collect::<Vec<_>>();
            assert_eq!(
                labourers.len(),
                1,
                "{id}: the household joins the place's one labourer pop"
            );
            let (labourer_index, pop) = labourers[0];
            assert_eq!(pop.culture, place_culture, "{id}");
            let location = scenario
                .locations
                .iter()
                .find(|location| location.id == *id)
                .unwrap();
            let kind = &scenario.location_kinds.kinds[location.kind_index].id;
            let (defaults, override_) = data.resolve(kind, location.size, id).unwrap();
            let existing = override_.labourers.unwrap_or(defaults.labourers);
            let added = pop.workers + pop.dependents - existing;
            assert_eq!(
                added,
                household_heads(noble.labourers),
                "{id}: the smallest household with {} workers",
                noble.labourers
            );
            heads_added += added;
            assert_eq!(
                demesne.employment[labourer_index].workers, noble.labourers,
                "{id}: every demesne job is filled"
            );
            jobs_filled += demesne.employment[labourer_index].workers;
        }
        assert_eq!(nobles_total, 1200);
        assert_eq!(jobs_filled, 600, "600 paid workers where no level is cut");
        assert_eq!(
            heads_added,
            16 * 14 + 32 * 17 + 66 * 3,
            "the households of 14, 17 and 3 houses of 1, 2 and 4 levels"
        );
    }

    #[test]
    fn dunseverick_has_a_dal_riatan_house_and_armagh_one_shared_labourer_pop() {
        let assets = ireland_assets();
        let directory = assets.join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).unwrap();
        let (state, _) = starting_economy(&assets, &directory, &scenario).unwrap();
        // Dún Sobhairce's house is dal_riatan in an ulaid place, over ulaid labourers.
        let dunseverick = state
            .places
            .iter()
            .find(|place| place.id == "dunseverick")
            .unwrap();
        let culture_of = |kind: &str| {
            dunseverick
                .pops
                .iter()
                .find(|pop| pop.kind == kind)
                .unwrap()
                .culture
                .clone()
        };
        assert_eq!(
            (
                culture_of("noble").as_str(),
                culture_of("labourer").as_str()
            ),
            ("dal_riatan", "ulaid")
        );

        // Armagh's demesne labourers are the monastery's labourer pop, employed in both buildings.
        let armagh = state
            .places
            .iter()
            .find(|place| place.id == "armagh")
            .unwrap();
        let labourers = armagh
            .pops
            .iter()
            .enumerate()
            .filter(|(_, pop)| pop.kind == "labourer")
            .collect::<Vec<_>>();
        assert_eq!(labourers.len(), 1);
        let employment = &labourers[0].1.employment;
        for kind in ["holdings", "demesne"] {
            let index = armagh
                .buildings
                .iter()
                .position(|building| building.kind == kind)
                .unwrap();
            assert!(
                employment[index].workers > 0,
                "armagh labourers work in the {kind}"
            );
        }
    }

    #[test]
    fn a_noble_house_at_an_unknown_or_abandoned_place_is_refused() {
        let assets = ireland_assets();
        let directory = assets.join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).unwrap();
        let original = std::fs::read_to_string(directory.join("economy.toml")).unwrap();
        for (id, expected) in [
            ("atlantis", "nobles.atlantis: no such location"),
            (
                "emain_macha",
                "nobles.emain_macha: abandoned location cannot have a noble house",
            ),
        ] {
            let temporary = std::env::temp_dir().join(format!("ck359-{id}-{}", std::process::id()));
            std::fs::create_dir_all(&temporary).unwrap();
            std::fs::write(
                temporary.join("economy.toml"),
                format!(
                    "{original}\n[nobles.{id}]\nculture=\"ulaid\"\nreligion=\"christianity\"\npeople=20\ndemesne_levels=1\nlabourers=10\n"
                ),
            )
            .unwrap();
            let error = starting_economy(&assets, &temporary, &scenario).unwrap_err();
            std::fs::remove_dir_all(&temporary).unwrap();
            assert_eq!(error, expected);
        }
    }

    #[test]
    fn r21_new_places_keep_the_designer_cultures_and_religions() {
        let path = ireland_assets().join("scenarios/ireland-768/economy.toml");
        let data = ScenarioEconomy::load(path).unwrap();
        let expected = [
            ("croaghan_hill", "northern_ui_neill"),
            ("gowran", "mumu"),
            ("mullaghmast", "laigin"),
            ("locha_lein", "mumu"),
            ("ui_maine_seat", "connachta"),
            ("tailtiu", "mide_brega"),
            ("tir_amalgaid", "connachta"),
            ("corco_modruad", "connachta"),
            ("ui_echach_seat", "cruthin"),
            ("aidne_seat", "connachta"),
            ("inchbofin", "mide_brega"),
            ("inchcleraun", "mide_brega"),
            ("rechru", "mide_brega"),
            ("inis_patraic", "mide_brega"),
            ("inishmacsaint", "airgialla"),
            ("inishkeel", "northern_ui_neill"),
            ("white_island", "airgialla"),
            ("hare_island", "mide_brega"),
            ("tethbae_seat", "mide_brega"),
            ("church_island_valentia", "mumu"),
            ("illaunloughan", "mumu"),
            ("inishkea_north", "connachta"),
            ("inishglora", "connachta"),
            ("church_island_currane", "mumu"),
            ("spike_island", "mumu"),
            ("cairpre_seat", "northern_ui_neill"),
            ("granard", "mide_brega"),
            ("loegaire_seat", "mide_brega"),
            ("uliathain_seat", "mumu"),
            ("ciarraige_seat", "mumu"),
            ("monaincha", "mumu"),
        ];
        assert_eq!(expected.len(), 31);
        for (id, culture) in expected {
            let place = &data.places[id];
            assert_eq!(place.culture.as_deref(), Some(culture), "{id}");
            assert_eq!(place.religion.as_deref(), Some("christianity"), "{id}");
            assert_eq!(place.monk_religion.as_deref(), Some("christianity"), "{id}");
        }
        assert!(!data.defaults.contains_key("royal_site"));
        assert!(!data.places.contains_key("dunamase") && !data.places.contains_key("cahercommaun"));
    }

    #[test]
    fn abandoned_sites_have_no_economy_and_explicit_entries_are_rejected() {
        let assets = ireland_assets();
        let directory = assets.join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).unwrap();
        let rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        let counts = land_counts(&scenario, &rules.terrain_order).unwrap();
        let (state, _) = starting_economy(&assets, &directory, &scenario).unwrap();
        for (index, place) in scenario
            .locations
            .iter()
            .enumerate()
            .filter(|(_, p)| p.abandoned)
        {
            assert!(!state.places.iter().any(|p| p.id == place.id));
            assert_eq!(counts[index], LandCounts::default());
        }
        assert_eq!(state.places.len(), 123);
        let temp = std::env::temp_dir().join(format!("ck358-abandoned-{}", std::process::id()));
        std::fs::create_dir_all(&temp).unwrap();
        let mut data = std::fs::read_to_string(directory.join("economy.toml")).unwrap();
        data.push_str("\n[places.emain_macha]\nculture = \"ulaid\"\nreligion = \"christianity\"\n");
        std::fs::write(temp.join("economy.toml"), data).unwrap();
        let error = starting_economy(&assets, &temp, &scenario).unwrap_err();
        std::fs::remove_dir_all(temp).unwrap();
        assert!(
            error.contains("emain_macha") && error.contains("abandoned"),
            "{error}"
        );
    }

    #[test]
    fn starting_wage_handoff_uses_the_authored_scenario_month() {
        let assets = ireland_assets();
        let directory = assets.join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).unwrap();
        let rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        assert_eq!(
            scenario.start.month, 10,
            "the shipped fixture starts in October"
        );
        let (october_state, _) = starting_economy(&assets, &directory, &scenario).unwrap();
        let mut june_scenario = scenario.clone();
        june_scenario.start.month = 6;
        let (june_state, _) = starting_economy(&assets, &directory, &june_scenario).unwrap();

        let (place_index, building_index) = october_state
            .places
            .iter()
            .enumerate()
            .find_map(|(place_index, place)| {
                place
                    .buildings
                    .iter()
                    .enumerate()
                    .find_map(|(building_index, building)| {
                        let paid_heads = building
                            .employment
                            .iter()
                            .map(|employment| employment.workers)
                            .sum::<u64>();
                        let value = building
                            .stock
                            .iter()
                            .zip(&rules.goods)
                            .map(|(units, good)| {
                                u128::from(*units) * u128::from(good.base_price) * 1000
                            })
                            .sum::<u128>();
                        (paid_heads > 0 && value > 0).then_some((place_index, building_index))
                    })
            })
            .expect("the fixture has an employed building with a starting stockpile");
        let october_building = &october_state.places[place_index].buildings[building_index];
        let june_building = &june_state.places[place_index].buildings[building_index];
        let definition = rules
            .buildings
            .iter()
            .find(|item| item.id == october_building.kind)
            .unwrap();
        let pools =
            ck_economy::building_job_pools(definition, &october_building.method_levels).unwrap();
        let shares: u128 = october_building
            .employment
            .iter()
            .flat_map(|employment| {
                pools.iter().map(move |pool| {
                    u128::from(employment.jobs.get(&pool.job.id).copied().unwrap_or(0))
                        * u128::from(pool.job.wage_weight_pm)
                })
            })
            .sum();
        let starting_value = october_building
            .stock
            .iter()
            .zip(&rules.goods)
            .map(|(units, good)| u128::from(*units) * u128::from(good.base_price) * 1000)
            .sum::<u128>();
        assert_eq!(
            october_building.wage_history,
            [u64::try_from(starting_value * 1000 / (shares * 11)).unwrap(); 12]
        );
        assert_eq!(
            june_building.wage_history,
            [u64::try_from(starting_value * 1000 / (shares * 3)).unwrap(); 12]
        );
    }
}
