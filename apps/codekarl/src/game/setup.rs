//! Starting a game: a new one from the scenario's start, or one loaded from a save. Everything here happens before a window
//! opens or a thread starts, so a refusal is a configuration error and nothing is left running.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use ck_economy::Rules;
use ck_log::Level;
use ck_runtime::OwnerSpawn;
use ck_sim::{Cadence, Date, GameTime, Save, Scheduler, Simulation};

use super::{
    GameCommand, GameState, apply,
    controls::SimControls,
    core::{Core, ECONOMY_HOUR, Outputs, Rebuild},
    economy,
    gameplay::{self, Gameplay, GameplayWiring},
    hashes,
    owner::{SimOwner, channel},
    saves::{Identity, build_id},
    slot::SimSlot,
    view,
};
use crate::config::{GameConfig, VmConfig};
use crate::lifecycle::emit;

/// The VM whose entry script the scripts hash covers.
const GAMEPLAY_VM: &str = "gameplay";

/// What a game needs before it starts.
pub(crate) struct Request<'a> {
    /// The `[game]` table.
    pub(crate) game: &'a GameConfig,
    /// The configured VMs, for the scripts hash.
    pub(crate) vms: &'a [VmConfig],
    /// `--load PATH`.
    pub(crate) load: Option<&'a Path>,
    /// `--seed N`.
    pub(crate) seed: Option<u64>,
}

/// A game ready to start: the scenario (its map is the application's map) and the simulation.
pub(crate) struct Prepared {
    /// The loaded scenario.
    pub(crate) scenario: ck_map::Scenario,
    pub(super) identity: Identity,
    pub(super) simulation: Simulation<GameState, GameCommand>,
    pub(super) gameplay: Option<GameplayWiring>,
    /// The shared economy rules the view is derived with; `None` for assets that predate M9.
    economy_rules: Option<Arc<Rules>>,
    /// Map-derived makeshift shelter by place, not saved in `EconomyState`.
    shelter_by_place: BTreeMap<String, u64>,
    saves: PathBuf,
}

/// Eight bytes from the operating system, as a seed.
pub(crate) fn operating_system_seed() -> Result<u64, String> {
    let mut bytes = [0_u8; 8];
    // SAFETY: `getentropy` writes at most `bytes.len()` (8, within its limit of 256) bytes to the buffer it is given, which
    // is exactly that long; a failure is reported through the return value, which is checked below.
    let result = unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) };
    if result == 0 {
        Ok(u64::from_le_bytes(bytes))
    } else {
        Err(format!(
            "cannot get a seed from the operating system: {}",
            io::Error::last_os_error()
        ))
    }
}

/// Loads the scenario and builds the game: new, or loaded from `request.load`.
///
/// `entropy` supplies a new game's seed when `--seed` did not. `log` receives the `sim` records: the new game's seed (once),
/// or the loaded save.
///
/// # Errors
/// A message for a configuration error: the scenario or its start date, an unreadable file, a save that is refused (with its
/// named error), or a seed that cannot be had.
#[expect(
    clippy::too_many_lines,
    reason = "Scenario identity, rules-derived qualities, save loading and deterministic startup are configured together before the owner starts."
)]
pub(crate) fn prepare(
    request: &Request<'_>,
    entropy: impl FnOnce() -> Result<u64, String>,
    log: &mut dyn FnMut(Level, String),
) -> Result<Prepared, String> {
    let directory = &request.game.scenario;
    let scenario = ck_map::load_scenario(directory).map_err(|error| format!("[game] {error}"))?;
    let start = Date {
        year: scenario.start.year,
        month: scenario.start.month,
        day: scenario.start.day,
        hour: scenario.start.hour,
    };
    let start_time = GameTime::from_date(start)
        .map_err(|error| format!("[game] scenario `{}`: its start date: {error}", scenario.id))?;
    let gameplay_entry = request
        .vms
        .iter()
        .find(|vm| vm.name == GAMEPLAY_VM)
        .map(|vm| vm.entry.as_path());
    let asset_root = directory.parent().and_then(Path::parent).ok_or_else(|| {
        format!(
            "[game] scenario directory {} is not under assets/scenarios",
            directory.display()
        )
    })?;
    let economy_rules = economy::load_rules(asset_root)?.map(Arc::new);
    let quality_by_place = if let Some(rules) = economy_rules.as_deref() {
        economy::land_counts(&scenario, &rules.terrain_order)?
            .into_iter()
            .enumerate()
            .map(|(index, land)| {
                Ok((
                    scenario.locations[index].id.clone(),
                    ck_economy::mean_fertility(&land).map_err(|error| error.to_string())?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?
    } else {
        BTreeMap::new()
    };
    let shelter_by_place = if let Some(rules) = economy_rules.as_deref() {
        economy::land_counts(&scenario, &rules.terrain_order)?
            .into_iter()
            .enumerate()
            .map(|(index, land)| {
                Ok((
                    scenario.locations[index].id.clone(),
                    ck_economy::natural_shelter(rules, &land).map_err(|error| error.to_string())?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?
    } else {
        BTreeMap::new()
    };
    let nearby_by_place = nearby_places(
        &scenario,
        economy_rules.as_deref().map_or(0, max_recruit_range_m),
    );
    let identity = Identity {
        build: build_id(),
        scenario: scenario.id.clone(),
        scenario_hash: hashes::scenario_hash(directory)
            .map_err(|error| format!("[game] {error}"))?,
        map_hash: hashes::map_directory(directory)
            .and_then(|map| hashes::map_hash(&map))
            .map_err(|error| format!("[game] {error}"))?,
        scripts_hash: hashes::scripts_hash(gameplay_entry)?,
        rules_hash: hashes::rules_hash(asset_root)?,
        start,
    };
    let gameplay_runtime = request
        .vms
        .iter()
        .any(|vm| vm.name == GAMEPLAY_VM)
        .then(|| Gameplay::new(0));
    let scheduler = || {
        gameplay_scheduler_with_rules(
            gameplay_runtime.as_ref(),
            economy_rules.clone(),
            quality_by_place.clone(),
            shelter_by_place.clone(),
            nearby_by_place.clone(),
        )
    };
    let simulation = if let Some(path) = request.load {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("cannot read save {}: {error}", path.display()))?;
        let save = Save::<GameState>::from_bytes(&bytes, &identity.expected())
            .map_err(|error| format!("save {}: {error}", path.display()))?;
        let simulation = Simulation::from_save(save, scheduler(), apply)
            .map_err(|error| format!("save {}: {error}", path.display()))?;
        let date = simulation.date().unwrap_or(start);
        log(Level::Info, format!("loaded {} at {date}", path.display()));
        simulation
    } else {
        let seed = match request.seed {
            Some(seed) => seed,
            None => entropy()?,
        };
        log(Level::Info, format!("new game, seed {seed:016x}"));
        let (economy_state, load_report) =
            economy::starting_economy(asset_root, directory, &scenario)?;
        for line in load_report {
            log(Level::Info, format!("economy start: {line}"));
        }
        Simulation::new(
            start_time,
            seed,
            GameState {
                economy: economy_state,
            },
            scheduler(),
            apply,
        )
    };
    if let Some(gameplay) = &gameplay_runtime {
        gameplay.set_seed(simulation.seed());
    }
    let gameplay = gameplay_runtime.map(|runtime| GameplayWiring {
        runtime,
        config: gameplay::canonical_config(&scenario.id, start),
    });
    Ok(Prepared {
        scenario,
        identity,
        simulation,
        gameplay,
        economy_rules,
        shelter_by_place,
        saves: request.game.saves.clone(),
    })
}

/// A game wired to the run: the controls the UI link sends presses through, the slot it reads, and the owner to spawn.
pub(crate) struct Connected {
    /// The main thread's end of the control channel.
    pub(crate) controls: SimControls,
    /// The slot the owner publishes to.
    pub(crate) slot: Arc<SimSlot>,
    /// The owner, to be spawned after every configured VM.
    pub(crate) spawn: OwnerSpawn,
    pub(crate) gameplay: Option<GameplayWiring>,
}

/// Wires `prepared` to the run: `wake` wakes the main thread whenever the owner publishes.
#[expect(
    clippy::too_many_lines,
    reason = "Connection wiring installs each prepared subsystem once in its dependency order."
)]
pub(crate) fn connect(prepared: Prepared, wake: Arc<dyn Fn() + Send + Sync>) -> Connected {
    let Prepared {
        identity,
        simulation,
        gameplay,
        economy_rules,
        shelter_by_place,
        saves,
        scenario,
    } = prepared;
    let place_ids: Vec<String> = scenario
        .locations
        .iter()
        .map(|location| location.id.clone())
        .collect();
    let land_by_place = if let Some(rules) = economy_rules.as_deref() {
        economy::land_counts(&scenario, &rules.terrain_order)
            .expect("the already-loaded scenario has valid catchments")
            .into_iter()
            .enumerate()
            .map(|(index, land)| (scenario.locations[index].id.clone(), land))
            .collect::<BTreeMap<_, _>>()
    } else {
        BTreeMap::new()
    };
    let quality_by_place = land_by_place
        .iter()
        .map(|(id, land)| {
            (
                id.clone(),
                ck_economy::mean_fertility(land).expect("validated map fertility"),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let nearby_by_place = nearby_places(
        &scenario,
        economy_rules.as_deref().map_or(0, max_recruit_range_m),
    );
    let wiring_gameplay = gameplay.clone();
    let wiring_rules = economy_rules.clone();
    let (sender, receiver) = channel();
    let slot = SimSlot::new(wake);
    let controls = SimControls::new(sender.clone());
    let owned_slot = Arc::clone(&slot);
    let spawn = OwnerSpawn::new("sim", move || {
        let publishing = Arc::clone(&owned_slot);
        let failing = Arc::clone(&owned_slot);
        let runtime = gameplay.as_ref().map(|wiring| Arc::clone(&wiring.runtime));
        let terminal_runtime = runtime.clone();
        let scheduler_runtime = runtime.clone();
        let scheduler_rules = wiring_rules.clone();
        let scheduler_quality = quality_by_place.clone();
        let scheduler_shelter = shelter_by_place.clone();
        let scheduler_nearby = nearby_by_place.clone();
        let reset_runtime = runtime.clone();
        let build = move || {
            Core::new_with_views(
                simulation,
                identity,
                saves,
                Rebuild {
                    scheduler: Box::new(move || {
                        gameplay_scheduler_with_rules(
                            scheduler_runtime.as_ref(),
                            scheduler_rules.clone(),
                            scheduler_quality.clone(),
                            scheduler_shelter.clone(),
                            scheduler_nearby.clone(),
                        )
                    }),
                    reset: Box::new(move |seed| {
                        if let Some(runtime) = &reset_runtime {
                            runtime.set_seed(seed);
                        }
                    }),
                    apply,
                },
                Outputs {
                    log: Box::new(|level, message| emit(level, "sim", format_args!("{message}"))),
                    publish: Box::new(move |snapshot| publishing.publish(snapshot)),
                    fail: Box::new(move |message| failing.fail(message)),
                },
                Some(Box::new(move |state: &GameState| {
                    view::build_with_land(
                        economy_rules.as_deref(),
                        &state.economy,
                        &place_ids,
                        &land_by_place,
                    )
                })),
            )
            .with_terminal_probe(move || {
                terminal_runtime
                    .as_ref()
                    .and_then(|runtime| runtime.take_terminal_failure())
            })
        };
        SimOwner::spawn_with_gameplay(sender, receiver, owned_slot, build, runtime)
            .map(|owner| Box::new(owner) as Box<dyn ck_runtime::Owner>)
    });
    Connected {
        controls,
        slot,
        spawn,
        gameplay: wiring_gameplay,
    }
}

fn gameplay_scheduler_with_rules(
    gameplay: Option<&Arc<Gameplay>>,
    rules: Option<Arc<Rules>>,
    quality_by_place: BTreeMap<String, u16>,
    shelter_by_place: BTreeMap<String, u64>,
    nearby_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
) -> Scheduler<GameState> {
    let mut scheduler = Scheduler::<GameState>::new();
    if let Some(rules) = rules {
        register_economy_callback_with_context(
            &mut scheduler,
            rules,
            quality_by_place,
            shelter_by_place,
            nearby_by_place,
        );
    } else {
        scheduler
            .register(
                "economy.daily",
                Cadence::Daily { hour: ECONOMY_HOUR },
                |_, _| Ok(()),
            )
            .expect("the economy system name is unique");
    }
    if let Some(gameplay) = gameplay {
        register_gameplay_lua_callbacks(&mut scheduler, gameplay);
    }
    scheduler
}

#[cfg(test)]
pub(crate) fn register_economy_callback(scheduler: &mut Scheduler<GameState>, rules: Arc<Rules>) {
    register_economy_callback_with_context(
        scheduler,
        rules,
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    );
}

fn register_economy_callback_with_context(
    scheduler: &mut Scheduler<GameState>,
    rules: Arc<Rules>,
    quality_by_place: BTreeMap<String, u16>,
    shelter_by_place: BTreeMap<String, u64>,
    nearby_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
) {
    scheduler
        .register(
            "economy.daily",
            Cadence::Daily { hour: ECONOMY_HOUR },
            move |state, tick| {
                ck_economy::run_daily_economy_with_context(
                    &rules,
                    &mut state.economy,
                    tick.date(),
                    &quality_by_place,
                    &shelter_by_place,
                    &nearby_by_place,
                )
                .map(|_| ())
                .map_err(io::Error::other)
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
            },
        )
        .expect("the economy system name is unique");
}

pub(crate) fn nearby_places(
    scenario: &ck_map::Scenario,
    max_range_m: u32,
) -> BTreeMap<String, Vec<ck_economy::NearbyPlace>> {
    let locations = scenario
        .locations
        .iter()
        .map(|location| (location.id.clone(), location.cell))
        .collect::<Vec<_>>();
    nearby_places_for_locations(&locations, max_range_m)
}

pub(crate) fn max_recruit_range_m(rules: &Rules) -> u32 {
    rules
        .buildings
        .iter()
        .filter_map(|building| building.recruit_range_m)
        .max()
        .unwrap_or(0)
}

fn nearby_places_for_locations(
    locations: &[(String, ck_map::Cell)],
    max_range_m: u32,
) -> BTreeMap<String, Vec<ck_economy::NearbyPlace>> {
    let mut result = BTreeMap::new();
    for (source_index, (source_id, source_cell)) in locations.iter().enumerate() {
        let mut destinations = locations
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != source_index)
            .filter_map(|(index, (destination_id, destination_cell))| {
                let distance_m = ground_distance_m(*source_cell, *destination_cell)?;
                (distance_m <= u64::from(max_range_m)).then_some((
                    distance_m,
                    index,
                    destination_id.clone(),
                ))
            })
            .collect::<Vec<_>>();
        destinations.sort_by_key(|(distance_m, index, _)| (*distance_m, *index));
        result.insert(
            source_id.clone(),
            destinations
                .into_iter()
                .map(|(distance_m, _, id)| ck_economy::NearbyPlace {
                    id,
                    distance_m: u32::try_from(distance_m)
                        .expect("distance is bounded by the u32 maximum range"),
                })
                .collect(),
        );
    }
    result
}

fn ground_distance_m(first: ck_map::Cell, second: ck_map::Cell) -> Option<u64> {
    let dx = i64::from(first.x) - i64::from(second.x);
    let dy = i64::from(first.y) - i64::from(second.y);
    let squared = u128::try_from(
        i128::from(dx)
            .checked_mul(i128::from(dx))?
            .checked_add(i128::from(dy).checked_mul(i128::from(dy))?)?,
    )
    .ok()?;
    let cell_distance = u64::try_from(squared.isqrt()).ok()?;
    let midpoint_row = (i64::from(first.y) + i64::from(second.y)).div_euclid(2);
    let scale = u64::from(ck_map::ground_scale_for_row(midpoint_row));
    cell_distance
        .checked_mul(839)?
        .checked_mul(scale)
        .map(|scaled| scaled / 65_536)
}

pub(crate) fn register_gameplay_lua_callbacks(
    scheduler: &mut Scheduler<GameState>,
    gameplay: &Arc<Gameplay>,
) {
    let monthly = Arc::clone(gameplay);
    scheduler
        .register(
            "gameplay.on_month",
            Cadence::Daily { hour: ECONOMY_HOUR },
            move |_, tick| {
                if tick.date().day != 1 {
                    return Ok(());
                }
                monthly
                    .call("on_month", tick.date(), tick.time())
                    .map_err(io::Error::other)
                    .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
            },
        )
        .expect("the gameplay monthly system name is unique");
    let daily = Arc::clone(gameplay);
    scheduler
        .register(
            "gameplay.on_day",
            Cadence::Daily { hour: ECONOMY_HOUR },
            move |_, tick| {
                daily
                    .call("on_day", tick.date(), tick.time())
                    .map_err(io::Error::other)
                    .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
            },
        )
        .expect("the gameplay daily system name is unique");
}

#[cfg(test)]
mod nearby_place_tests {
    use super::{ground_distance_m, nearby_places, nearby_places_for_locations};
    use ck_map::Cell;
    use std::path::Path;

    #[test]
    fn ireland_island_monasteries_reach_their_ground_distance_neighbours() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("Ireland scenario loads");
        let nearby = nearby_places(&scenario, 100_000);

        let skellig = &nearby["skellig_michael"];
        assert_eq!(
            skellig
                .iter()
                .take(2)
                .map(|place| place.id.as_str())
                .collect::<Vec<_>>(),
            ["inisfallen", "ardfert"]
        );
        for id in ["inisfallen", "ardfert"] {
            let place = skellig.iter().find(|place| place.id == id).unwrap();
            assert!(place.distance_m <= 100_000);
            assert!(place.distance_m > 30_000);
        }

        let inishmurray = &nearby["inishmurray"];
        let drumcliff = inishmurray
            .iter()
            .find(|place| place.id == "drumcliff")
            .expect("Drumcliff is inside the 100 km list");
        assert!(drumcliff.distance_m <= 30_000);
    }

    #[test]
    fn nearby_distances_use_ground_scale_and_inclusive_range() {
        let source = Cell { x: 0, y: 15_777 };
        let locations = vec![
            ("source".to_owned(), source),
            ("farm-in".to_owned(), Cell { x: 58, y: 15_777 }),
            ("farm-out".to_owned(), Cell { x: 59, y: 15_777 }),
            ("house-in".to_owned(), Cell { x: 193, y: 15_777 }),
            ("house-out".to_owned(), Cell { x: 194, y: 15_777 }),
            ("middle".to_owned(), Cell { x: 145, y: 15_777 }),
        ];
        assert_eq!(ground_distance_m(source, locations[1].1), Some(29_959));
        assert_eq!(ground_distance_m(source, locations[2].1), Some(30_475));
        assert_eq!(ground_distance_m(source, locations[3].1), Some(99_692));
        assert_eq!(ground_distance_m(source, locations[4].1), Some(100_208));
        assert_eq!(ground_distance_m(source, locations[5].1), Some(74_898));

        let farm_range = nearby_places_for_locations(&locations, 30_000);
        let farm_ids = farm_range["source"]
            .iter()
            .map(|place| place.id.as_str())
            .collect::<Vec<_>>();
        assert!(farm_ids.contains(&"farm-in"));
        assert!(!farm_ids.contains(&"farm-out"));
        assert!(!farm_ids.contains(&"house-in"));

        let holdings_range = nearby_places_for_locations(&locations, 100_000);
        let holdings_ids = holdings_range["source"]
            .iter()
            .map(|place| place.id.as_str())
            .collect::<Vec<_>>();
        assert!(holdings_ids.contains(&"house-in"));
        assert!(!holdings_ids.contains(&"house-out"));
        assert_eq!(holdings_ids.first(), Some(&"farm-in"));
    }

    #[test]
    fn ground_nearest_order_wins_over_cell_distance_and_ties_use_file_order() {
        let source = Cell { x: 0, y: 15_777 };
        let south = Cell { x: 0, y: 15_875 };
        let north = Cell { x: 0, y: 15_678 };
        assert_eq!(ground_distance_m(source, north), Some(50_872));
        assert_eq!(ground_distance_m(source, south), Some(50_878));
        let locations = vec![
            ("source".to_owned(), source),
            ("south-first".to_owned(), south),
            ("north-second".to_owned(), north),
            ("tie-first".to_owned(), Cell { x: 2, y: 15_777 }),
            ("tie-second".to_owned(), Cell { x: -2, y: 15_777 }),
        ];
        let nearby = nearby_places_for_locations(&locations, 100_000);
        let ids = nearby["source"]
            .iter()
            .map(|place| place.id.as_str())
            .collect::<Vec<_>>();
        let north_index = ids.iter().position(|id| *id == "north-second").unwrap();
        let south_index = ids.iter().position(|id| *id == "south-first").unwrap();
        assert!(
            north_index < south_index,
            "ground distance sorts before file order"
        );
        let tie_first_index = ids.iter().position(|id| *id == "tie-first").unwrap();
        let tie_second_index = ids.iter().position(|id| *id == "tie-second").unwrap();
        assert!(
            tie_first_index < tie_second_index,
            "equal distances keep file order"
        );
    }
}
