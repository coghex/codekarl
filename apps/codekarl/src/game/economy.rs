//! Loading M9 economy inputs and adapting the map catchment to plain terrain counts.

use std::path::Path;

use ck_economy::{EconomyState, LandCounts, Rules, ScenarioEconomy, StartLocation};
use ck_map::{Scenario, Surface};

/// Count each owned usable land cell by terrain, plus sea cells; lakes and
/// unusable land are not development resources.
pub(crate) fn land_counts(
    scenario: &Scenario,
    terrain_order: &[String],
) -> Result<Vec<LandCounts>, String> {
    land_counts_from_cells(
        &scenario.catchment_owners,
        &scenario.map.data.cells,
        terrain_order,
        scenario.locations.len(),
    )
}

fn land_counts_from_cells(
    owners: &[Option<usize>],
    cells: &[Surface],
    terrain_order: &[String],
    location_count: usize,
) -> Result<Vec<LandCounts>, String> {
    let mut counts = vec![LandCounts::default(); location_count];
    if owners.len() != cells.len() {
        return Err("scenario catchment owner grid does not match map cells".to_owned());
    }
    for (owner, cell) in owners.iter().zip(cells) {
        let Some(owner) = owner else {
            continue;
        };
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
            Surface::Land {
                terrain,
                usable: true,
            } => {
                let name = terrain_order.get(usize::from(*terrain)).ok_or_else(|| {
                    format!("terrain style index {terrain} is absent from economy rules")
                })?;
                let value = count.usable.entry(name.clone()).or_default();
                *value = value
                    .checked_add(1)
                    .ok_or_else(|| format!("usable {name} cell count overflow"))?;
            }
            Surface::Land { usable: false, .. } | Surface::Lake => {}
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
pub(crate) fn starting_economy(
    asset_root: &Path,
    scenario_directory: &Path,
    scenario: &Scenario,
) -> Result<(EconomyState, Vec<String>), String> {
    let rules_path = asset_root.join("game/economy.toml");
    if !rules_path.exists() {
        return Ok((EconomyState::default(), Vec::new()));
    }
    let rules = Rules::load(&rules_path).map_err(|error| error.to_string())?;
    let scenario_path = scenario_directory.join("economy.toml");
    let data = ScenarioEconomy::load(&scenario_path).map_err(|error| error.to_string())?;
    let land = land_counts(scenario, &rules.terrain_order)?;
    let starts = scenario
        .locations
        .iter()
        .zip(land)
        .map(|(location, land)| StartLocation {
            id: location.id.clone(),
            kind: scenario.location_kinds.kinds[location.kind_index]
                .id
                .clone(),
            size: location.size,
            land,
        })
        .collect::<Vec<_>>();
    ck_economy::starting_state(&rules, &data, &starts).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn ireland_assets() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
    }

    #[test]
    fn land_counts_fixture_assigns_boundary_cells_once_and_ignores_nonresources() {
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
        let terrain = ["pasture".to_owned(), "forest".to_owned()];
        assert_eq!(
            land_counts_from_cells(&owners, &cells, &terrain, 2).unwrap(),
            [
                LandCounts {
                    usable: BTreeMap::new(),
                    sea: 1
                },
                LandCounts {
                    usable: BTreeMap::from([("forest".to_owned(), 1), ("pasture".to_owned(), 1)]),
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
        for (owner, cell) in scenario
            .catchment_owners
            .iter()
            .zip(&scenario.map.data.cells)
        {
            let Some(owner) = owner else {
                continue;
            };
            match cell {
                Surface::Sea { .. } => expected[*owner].sea += 1,
                Surface::Land {
                    terrain,
                    usable: true,
                } => {
                    let name = &rules.terrain_order[usize::from(*terrain)];
                    *expected[*owner].usable.entry(name.clone()).or_default() += 1;
                }
                Surface::Land { usable: false, .. } | Surface::Lake => {}
            }
        }
        assert_eq!(
            counts, expected,
            "only owned sea and usable land cells are resources"
        );
        assert!(counts.iter().any(|count| count.sea > 0));
        assert!(counts.iter().any(|count| !count.usable.is_empty()));

        let (state, report) = starting_economy(&assets, &directory, &scenario).unwrap();
        assert_eq!(state.places.len(), 94);
        assert_eq!(report.len(), 46, "authored cuts are reported: {report:?}");
        assert!(report.contains(
            &"durrow holdings: cut levels 10 -> 3 (no method has enough land or sea)".to_owned()
        ));
        assert!(report.contains(
            &"dunseverick farm: cut levels 4 -> 2 (no method has enough land or sea)".to_owned()
        ));
        let mayo = state
            .places
            .iter()
            .find(|place| place.id == "mayo")
            .unwrap();
        assert!(
            mayo.pops
                .iter()
                .any(|pop| pop.kind == "monk" && pop.culture == "english")
        );
    }
}
