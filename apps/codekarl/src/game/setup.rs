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
    pub(super) layers: ck_sim::MapLayers,
    pub(super) gameplay: Option<GameplayWiring>,
    /// The shared economy rules the view is derived with; `None` for assets that predate M9.
    economy_rules: Option<Arc<Rules>>,
    /// The house names the character view shows (`houses.toml`, CK-375): empty for a scenario that ships no figures.
    houses: Arc<ck_characters::figures::Houses>,
    /// The place of each authored slot by its key (the load file's `[[slot]]`): the character view reads a spouse place there.
    slots: Arc<BTreeMap<String, String>>,
    /// Map-derived makeshift shelter by place, not saved in `EconomyState`.
    shelter_by_place: BTreeMap<String, u64>,
    saves: PathBuf,
    pub(super) storage: super::character_storage::Storage,
    /// Where the view builder finds the current archive reader (CK-377).
    archive_cell: super::character_storage::SourceCell,
    /// Where the view builder finds the current life-log reader (CK-378).
    log_cell: super::character_storage::LogCell,
    /// Where the scheduler's daily system finds the character storage (CK-363).
    characters_io: super::character_storage::Handle,
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
    let houses = Arc::new(load_houses(directory)?);
    let slots = Arc::new(load_slot_locations(directory)?);
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
    let reach_by_place = marriage_reach(
        &scenario,
        economy_rules
            .as_deref()
            .map_or(0, |rules| rules.lifecycle.marriage.pool.far_m),
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
    let place_names = place_names(&scenario);
    let characters_io = super::character_storage::Handle::default();
    let archive_cell = super::character_storage::SourceCell::default();
    let log_cell = super::character_storage::LogCell::default();
    let scheduler = || {
        gameplay_scheduler_with_rules(
            gameplay_runtime.as_ref(),
            economy_rules.clone(),
            quality_by_place.clone(),
            shelter_by_place.clone(),
            nearby_by_place.clone(),
            reach_by_place.clone(),
            place_names.clone(),
            Some(Arc::clone(&characters_io)),
        )
    };
    let storage_parent = request.game.saves.parent().unwrap_or(Path::new("."));
    let (simulation, storage, layers) = if let Some(path) = request.load {
        let mut file = std::fs::File::open(path)
            .map_err(|error| format!("cannot read save {}: {error}", path.display()))?;
        let staging = super::character_storage::Storage::fresh_path(storage_parent)
            .map_err(|e| e.to_string())?;
        let (save, _) = Save::<GameState>::read_with_sections(
            &mut file,
            &identity.expected(),
            &staging,
            &[*b"ARCH", *b"LOGS"],
        )
        .map_err(|e| format!("save {}: {e}", path.display()))?;
        let storage = super::character_storage::Storage::at(staging, None, None, None);
        let expected = super::stretch::starting(scenario.network.as_ref())
            .into_keys()
            .collect();
        super::stretch::validate(&save.state.stretches, &expected)?;
        super::character_storage::Storage::validate(&storage.path, &save.state)
            .map_err(|e| e.to_string())?;
        let layers = save.meta.layers;
        let simulation = Simulation::from_save(save, scheduler(), apply)
            .map_err(|error| format!("save {}: {error}", path.display()))?;
        let date = simulation.date().unwrap_or(start);
        log(Level::Info, format!("loaded {} at {date}", path.display()));
        (simulation, storage, layers)
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
        let storage =
            super::character_storage::Storage::empty(storage_parent).map_err(|e| e.to_string())?;
        let mut economy_state = economy_state;
        let characters = new_game_characters(
            directory,
            &scenario,
            economy_rules.as_deref(),
            &mut economy_state,
            &storage.path,
            seed,
            start_time,
            log,
        )?;
        (
            Simulation::new(
                start_time,
                seed,
                GameState {
                    economy: economy_state,
                    characters,
                    stretches: super::stretch::starting(scenario.network.as_ref()),
                },
                scheduler(),
                apply,
            ),
            storage,
            ck_sim::MapLayers::default(),
        )
    };
    let mut storage = storage;
    storage.attach(&characters_io, &archive_cell, &log_cell);
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
        layers,
        gameplay,
        economy_rules,
        houses,
        slots,
        shelter_by_place,
        saves: request.game.saves.clone(),
        storage,
        characters_io,
        archive_cell,
        log_cell,
    })
}

/// What the view builder needs: the economy and character views of a state are built from these (CK-375, CK-377).
pub(super) struct ViewWiring {
    rules: Option<Arc<Rules>>,
    place_ids: Vec<String>,
    land_by_place: BTreeMap<String, ck_economy::LandCounts>,
    place_names: Arc<BTreeMap<String, String>>,
    houses: Arc<ck_characters::figures::Houses>,
    slots: Arc<BTreeMap<String, String>>,
    /// Where the current archive reader is: a load replaces what it holds before the next view is built.
    archive_cell: super::character_storage::SourceCell,
    /// Where the current life-log reader is, replaced by a load like the archive reader.
    log_cell: super::character_storage::LogCell,
    start_year: u16,
}

impl Prepared {
    /// The view builder's inputs.
    pub(super) fn view_wiring(&self) -> ViewWiring {
        let place_ids: Vec<String> = self
            .scenario
            .locations
            .iter()
            .map(|location| location.id.clone())
            .collect();
        let land_by_place = if let Some(rules) = self.economy_rules.as_deref() {
            economy::land_counts(&self.scenario, &rules.terrain_order)
                .expect("the already-loaded scenario has valid catchments")
                .into_iter()
                .enumerate()
                .map(|(index, land)| (self.scenario.locations[index].id.clone(), land))
                .collect::<BTreeMap<_, _>>()
        } else {
            BTreeMap::new()
        };
        ViewWiring {
            rules: self.economy_rules.clone(),
            place_ids,
            land_by_place,
            place_names: Arc::new(place_names(&self.scenario)),
            houses: Arc::clone(&self.houses),
            slots: Arc::clone(&self.slots),
            archive_cell: Arc::clone(&self.archive_cell),
            log_cell: Arc::clone(&self.log_cell),
            start_year: self.scenario.start.year,
        }
    }
}

/// The economy view of a state, with the characters built with it so that they carry the same revision (CK-375 §1) and, for the relations tab,
/// the live records and the archive as published now (CK-377 §1).
pub(super) fn view_builder(wiring: ViewWiring) -> super::core::ViewBuilder<GameState> {
    Box::new(move |state: &GameState, date| {
        let mut view = view::build_with_land(
            wiring.rules.as_deref(),
            &state.economy,
            &wiring.place_ids,
            &wiring.land_by_place,
            date,
        );
        if let Some(rules_arc) = wiring.rules.as_ref() {
            let rules: &Rules = rules_arc;
            view.characters = super::character_view::build(
                &state.characters,
                &state.economy,
                &super::character_view::Inputs {
                    rules,
                    places: &wiring.place_names,
                    houses: &wiring.houses,
                    slots: &wiring.slots,
                },
                date,
            );
            // The relations tab reads this publication's world and keeps its archive readable. A failed archive read leaves the view without
            // one, and the tab refuses.
            let archive = wiring
                .archive_cell
                .lock()
                .ok()
                .and_then(|held| held.clone())
                .and_then(|source| source.view().ok());
            // The life log as published now: the records completely written so far, and no later one (CK-378 §2). A failed read leaves the
            // view without one, and `game.character_log` refuses.
            view.characters.log = wiring
                .log_cell
                .lock()
                .ok()
                .and_then(|held| held.clone())
                .and_then(|source| source.view().ok());
            view.characters.relations = super::character_relations::RelationsFacts::build(
                &state.characters,
                &state.economy,
                &super::character_relations::FactsInputs {
                    rules: rules_arc,
                    places: &wiring.place_names,
                    houses: &wiring.houses,
                    slots: &wiring.slots,
                    archive,
                    start_year: wiring.start_year,
                },
                date,
            );
        }
        view
    })
}

/// A core over a prepared game with the real view builder and storage, for tests of a load: the scheduler has no systems.
#[cfg(test)]
pub(super) fn test_core(
    prepared: Prepared,
    identity_saves: PathBuf,
    outputs: Outputs,
) -> Core<GameState, GameCommand> {
    let wiring = prepared.view_wiring();
    let stretch_ids = prepared
        .simulation
        .state()
        .stretches
        .keys()
        .copied()
        .collect();
    let Prepared {
        identity,
        simulation,
        storage,
        layers,
        ..
    } = prepared;
    Core::new_with_views_and_layers(
        simulation,
        identity,
        identity_saves,
        Rebuild {
            scheduler: Box::new(Scheduler::new),
            reset: Box::new(|_| {}),
            apply,
        },
        outputs,
        Some(view_builder(wiring)),
        layers,
    )
    .with_state_validation(move |state: &GameState| {
        super::stretch::validate(&state.stretches, &stretch_ids)
    })
    .with_character_storage(storage, super::character_storage::Storage::validate)
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
    let wiring = prepared.view_wiring();
    let stretch_ids = prepared
        .simulation
        .state()
        .stretches
        .keys()
        .copied()
        .collect();
    let Prepared {
        identity,
        simulation,
        layers,
        gameplay,
        economy_rules,
        shelter_by_place,
        saves,
        storage,
        characters_io,
        scenario,
        ..
    } = prepared;
    let land_by_place = wiring.land_by_place.clone();
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
    let reach_by_place = marriage_reach(
        &scenario,
        economy_rules
            .as_deref()
            .map_or(0, |rules| rules.lifecycle.marriage.pool.far_m),
    );
    let place_names = place_names(&scenario);
    let wiring_gameplay = gameplay.clone();
    let wiring_rules = economy_rules.clone();
    let (sender, receiver) = channel();
    let controls = SimControls::with_wake(sender.clone(), Arc::clone(&wake));
    let slot = SimSlot::new(wake);
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
        let scheduler_reach = reach_by_place.clone();
        let scheduler_names = place_names.clone();
        let scheduler_io = Arc::clone(&characters_io);
        let reset_runtime = runtime.clone();
        let build = move || {
            Core::new_with_views_and_layers(
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
                            scheduler_reach.clone(),
                            scheduler_names.clone(),
                            Some(Arc::clone(&scheduler_io)),
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
                Some(view_builder(wiring)),
                layers,
            )
            .with_state_validation(move |state: &GameState| {
                super::stretch::validate(&state.stretches, &stretch_ids)
            })
            .with_character_storage(storage, super::character_storage::Storage::validate)
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

#[expect(
    clippy::too_many_arguments,
    reason = "the rules and each place table the daily pass reads, the place names and the storage handle"
)]
fn gameplay_scheduler_with_rules(
    gameplay: Option<&Arc<Gameplay>>,
    rules: Option<Arc<Rules>>,
    quality_by_place: BTreeMap<String, u16>,
    shelter_by_place: BTreeMap<String, u64>,
    nearby_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    reach_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    place_names: BTreeMap<String, String>,
    characters_io: Option<super::character_storage::Handle>,
) -> Scheduler<GameState> {
    let mut scheduler = Scheduler::<GameState>::new();
    if let Some(rules) = rules {
        register_economy_callback_with_context(
            &mut scheduler,
            rules,
            quality_by_place,
            shelter_by_place,
            nearby_by_place,
            reach_by_place,
            place_names,
            characters_io,
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

/// The economy's daily system with the character storage published through `handle`, as the running game has it.
#[cfg(test)]
pub(crate) fn register_economy_callback_with_storage(
    scheduler: &mut Scheduler<GameState>,
    rules: Arc<Rules>,
    handle: super::character_storage::Handle,
) {
    register_economy_callback_with_context(
        scheduler,
        rules,
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        Some(handle),
    );
}

#[cfg(test)]
pub(crate) fn register_economy_callback(scheduler: &mut Scheduler<GameState>, rules: Arc<Rules>) {
    register_economy_callback_with_context(
        scheduler,
        rules,
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    );
}

#[expect(
    clippy::too_many_arguments,
    reason = "the rules and each place table the daily pass reads, the place names and the storage handle"
)]
fn register_economy_callback_with_context(
    scheduler: &mut Scheduler<GameState>,
    rules: Arc<Rules>,
    quality_by_place: BTreeMap<String, u16>,
    shelter_by_place: BTreeMap<String, u64>,
    nearby_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    reach_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    place_names: BTreeMap<String, String>,
    characters_io: Option<super::character_storage::Handle>,
) {
    scheduler
        .register_with_streams(
            "economy.daily",
            Cadence::Daily { hour: ECONOMY_HOUR },
            &[CHARACTER_STREAM],
            move |state, tick| {
                // The characters' draws are keyed by this tick's draw of the system's own stream: deterministic under the master
                // seed, and drawn once for all three phases of the day.
                let seed = tick.stream(CHARACTER_STREAM)?.draw();
                // Phase A, before the economy's step: the certain deaths and, on the 1st, the monthly mortality and the
                // deliveries. What it commits stays committed whatever the economy's step does next (CK-364 §4).
                let before = run_pre_economy_passes(
                    &rules,
                    state,
                    tick.time(),
                    seed,
                    &shelter_by_place,
                    &place_names,
                    characters_io.as_ref(),
                );
                let mut floors = BTreeMap::new();
                for character in state.characters.iter() {
                    *floors
                        .entry((character.pop.place.clone(), character.pop.slot as usize))
                        .or_insert(0_u64) += 1;
                }
                ck_economy::run_daily_economy_with_floors(
                    &rules,
                    &mut state.economy,
                    tick.date(),
                    &quality_by_place,
                    &shelter_by_place,
                    &nearby_by_place,
                    &floors,
                )
                .map_err(io::Error::other)
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
                // Phase C, after the economy's step. A failure of phase A is returned once both have run.
                let after = run_character_passes(
                    &rules,
                    state,
                    tick.time(),
                    seed,
                    &reach_by_place,
                    &place_names,
                    characters_io.as_ref(),
                );
                before.and(after)
            },
        )
        .expect("the economy system name is unique");
}

/// The name of the economy system's stream the character events' seed comes from.
const CHARACTER_STREAM: &str = "characters.events";

/// The product of the three death multipliers (‰ each: hunger, comfort, crowding) of every pop that has a live character, as the economy
/// computes them for the pop's workers from its state **now** (CK-364 §2): the pressure the day's mortality pass reads, taken once at
/// its start, before the pass changes any head.
///
/// # Errors
/// A missing curve or malformed housing rules.
pub(crate) fn death_pressure(
    rules: &Rules,
    economy: &ck_economy::EconomyState,
    characters: &ck_characters::CharacterStore,
    shelter_by_place: &BTreeMap<String, u64>,
) -> Result<ck_characters::cycle::Pressure, ck_economy::DataError> {
    let mut pressure = ck_characters::cycle::Pressure::new();
    for place in &economy.places {
        if !characters.iter().any(|c| c.pop.place == place.id) {
            continue;
        }
        let shelter = shelter_by_place.get(&place.id).copied().unwrap_or(0);
        for (slot, multipliers) in ck_economy::death_multipliers(rules, place, shelter)?
            .into_iter()
            .enumerate()
        {
            pressure.insert(
                ck_characters::PopLocator {
                    place: place.id.clone(),
                    slot: u32::try_from(slot).unwrap_or(u32::MAX),
                },
                multipliers
                    .iter()
                    .fold(1_u64, |all, m| all.saturating_mul(*m)),
            );
        }
    }
    Ok(pressure)
}

/// Phase A of a day (CK-364 §4), run inside the economy's daily system **before** its step: the certain deaths and, on the 1st, the
/// monthly mortality and then the deliveries, with the death multipliers of each pop read once, at the start of the pass.
///
/// With no characters, or without a storage handle (the unit fixtures that exercise only the shell), nothing is opened and nothing
/// happens: a death needs the archive.
pub(crate) fn run_pre_economy_passes(
    rules: &Rules,
    state: &mut GameState,
    tick: GameTime,
    seed: u64,
    shelter_by_place: &BTreeMap<String, u64>,
    place_names: &BTreeMap<String, String>,
    storage: Option<&super::character_storage::Handle>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(path) = storage.and_then(|cell| cell.lock().ok()?.clone()) else {
        return Ok(());
    };
    if state.characters.iter().next().is_none() {
        return Ok(());
    }
    state.characters.fix_seed(seed);
    let pressure = death_pressure(rules, &state.economy, &state.characters, shelter_by_place)?;
    let mut archive = open_archive(&path);
    let mut logs = open_logs(&path);
    let ledger = ck_characters::Ledger::new(&mut state.economy).with_place_names(place_names);
    let mut world = ck_characters::World {
        rules: &rules.events,
        personality: &rules.personality,
        genetics: &rules.genetics,
        lifecycle: &rules.lifecycle,
        seed,
        populations: &ledger,
        archive: &mut archive,
        logs: &mut logs,
    };
    // A character promoted to the full tier has its passions backfilled first, before the day's deaths or anything else can consume or
    // change it (CK-368 §9); a failure keeps its saved window for the next pass and is returned after the pass has run.
    state.characters.begin_ancestry_scope();
    let backfilled = state.characters.backfill_promotions(&mut world, tick);
    let passes = state
        .characters
        .run_pre_economy(&mut world, tick, &pressure);
    state.characters.end_ancestry_scope();
    passes?;
    backfilled?;
    logs.finish_scope()?;
    Ok(())
}

/// The character passes of CK-362 and CK-363 and the conceptions of CK-364 (phase C of a day), run inside the economy's daily system
/// after that day's economy work: on the 1st the monthly stress recovery, then the birthday pass (humour drift, standing fade, the yearly
/// events, craft rise, skills, precedence, coming of age, the marriage wish and the strand rolls and passions of CK-368) and, on the 1st, the monthly precedence check and the conceptions. They
/// are not separate systems, and an economy error never reaches them. A life-log or archive failure after a committed change is
/// returned here, as the economy's errors are.
///
/// With no characters nothing is opened. Without a storage handle (the unit fixtures that exercise only the shell) the pass is
/// slice 3's: drift and fade, which needs neither the archive nor the logs.
pub(crate) fn run_character_passes(
    rules: &Rules,
    state: &mut GameState,
    tick: GameTime,
    seed: u64,
    reach_by_place: &BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    place_names: &BTreeMap<String, String>,
    storage: Option<&super::character_storage::Handle>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let first = tick.date()?.day == 1;
    state.characters.fix_seed(seed);
    if first {
        state.characters.monthly_recovery(&rules.personality)?;
    }
    let path = storage.and_then(|cell| cell.lock().ok()?.clone());
    let Some(path) = path else {
        state.characters.birthday_pass(&rules.personality, tick)?;
        return Ok(());
    };
    if state.characters.iter().next().is_none() {
        return Ok(());
    }
    // A storage that cannot be opened must not stop the character state from updating: the archive append is the prerequisite of a
    // death (an unavailable archive refuses it), and a life-log line is output (an unavailable log reports each line it cannot write).
    let mut archive = open_archive(&path);
    let mut logs = open_logs(&path);
    let ledger = ck_characters::Ledger::with_nearby(&mut state.economy, reach_by_place)
        .with_place_names(place_names);
    let mut world = ck_characters::World {
        rules: &rules.events,
        personality: &rules.personality,
        genetics: &rules.genetics,
        lifecycle: &rules.lifecycle,
        seed,
        populations: &ledger,
        archive: &mut archive,
        logs: &mut logs,
    };
    // The birth-day samples of the day's newborns, after the economy's step (CK-367 §2), then the birthdays.
    state.characters.begin_ancestry_scope();
    let samples = state.characters.run_birth_samples(&mut world, tick);
    // On 1 January the closed year's bonds are resolved, once, before the day's birthdays (CK-368 §4).
    let new_year = state.characters.run_new_year(&mut world, tick);
    let birthdays = state.characters.run_birthdays(&mut world, tick);
    // On the 1st: the monthly precedence check, the conceptions and, after them, the monthly weights (CK-367).
    let (monthly, conceptions, weights) = if first {
        (
            state.characters.monthly_precedence(&mut world, tick),
            state.characters.run_conceptions(&mut world, tick),
            state.characters.run_monthly_weights(&mut world),
        )
    } else {
        (Ok(()), Ok(()), Ok(()))
    };
    state.characters.end_ancestry_scope();
    // All ran whatever the first reported; the first failure is the one returned.
    samples
        .and(new_year)
        .and(birthdays)
        .and(monthly)
        .and(conceptions)
        .and(weights)?;
    logs.finish_scope()?;
    Ok(())
}

fn open_archive(path: &Path) -> Archived {
    match ck_characters::Archive::open(path.join("ARCH")) {
        Ok(archive) => {
            archive.begin_scope();
            Archived::Open(archive)
        }
        Err(error) => Archived::Down(error.to_string()),
    }
}

fn open_logs(path: &Path) -> Logged {
    match ck_characters::LifeLog::open(path.join("LOGS")) {
        Ok(mut logs) => {
            logs.begin_scope();
            Logged::Open(logs)
        }
        Err(error) => Logged::Down(error.to_string()),
    }
}

/// The archive of the character storage, or the reason it cannot be opened (every append then fails).
enum Archived {
    Open(ck_characters::Archive),
    Down(String),
}
impl ck_characters::ArchiveSink for Archived {
    fn revision(&self) -> Option<u64> {
        match self {
            Self::Open(archive) => ck_characters::ArchiveSink::revision(archive),
            Self::Down(_) => None,
        }
    }
    fn append(&mut self, record: &ck_characters::Character) -> io::Result<()> {
        match self {
            Self::Open(archive) => archive.append(record),
            Self::Down(reason) => Err(io::Error::other(reason.clone())),
        }
    }
    fn get(&self, id: u64) -> io::Result<Option<ck_characters::Character>> {
        match self {
            Self::Open(archive) => archive.get(id),
            Self::Down(reason) => Err(io::Error::other(reason.clone())),
        }
    }
    fn children_of(&self, parent: u64) -> io::Result<Vec<u64>> {
        match self {
            Self::Open(archive) => ck_characters::ArchiveSink::children_of(archive, parent),
            Self::Down(reason) => Err(io::Error::other(reason.clone())),
        }
    }
    fn house_in_use(&self, house: &str) -> io::Result<bool> {
        match self {
            Self::Open(archive) => ck_characters::ArchiveSink::house_in_use(archive, house),
            Self::Down(reason) => Err(io::Error::other(reason.clone())),
        }
    }
    fn append_all(&mut self, records: &[ck_characters::Character]) -> io::Result<()> {
        // Nothing to publish needs no archive.
        if records.is_empty() {
            return Ok(());
        }
        match self {
            Self::Open(archive) => ck_characters::ArchiveSink::append_all(archive, records),
            Self::Down(reason) => Err(io::Error::other(reason.clone())),
        }
    }
}
/// The life logs of the character storage, or the reason they cannot be opened (every line then fails).
enum Logged {
    Open(ck_characters::LifeLog),
    Down(String),
}
impl Logged {
    fn finish_scope(&mut self) -> Result<(), ck_characters::EventError> {
        match self {
            Self::Open(logs) => logs.finish_scope().map_err(ck_characters::EventError::Log),
            Self::Down(_) => Ok(()),
        }
    }
}
impl ck_characters::LogSink for Logged {
    fn append(&mut self, id: u64, date: GameTime, text: &str) -> io::Result<()> {
        match self {
            Self::Open(logs) => logs.append(id, date, text),
            Self::Down(reason) => Err(io::Error::other(reason.clone())),
        }
    }
}

/// The characters of a new game (CK-370): a scenario that ships its real figures (the research and load files [`hashes::figure_files`] names) starts with them, their families, the slots' people and the fifteen years of the burn-in, on its economy: the safety net may
/// raise a pop's dependents, which the start log reports. Any other scenario starts with none.
///
/// # Errors
/// A figures file that cannot be read or does not load, or a refusal of the start: the game does not start.
#[expect(
    clippy::too_many_arguments,
    reason = "the scenario's directory and map, the rules, the economy to start on, where the storage is, the seed, the day and the log"
)]
fn new_game_characters(
    directory: &Path,
    scenario: &ck_map::Scenario,
    rules: Option<&Rules>,
    economy: &mut ck_economy::EconomyState,
    storage: &Path,
    seed: u64,
    start: GameTime,
    log: &mut dyn FnMut(Level, String),
) -> Result<ck_characters::CharacterStore, String> {
    let figures = hashes::figure_files(directory).map_err(|error| format!("[game] {error}"))?;
    let Some([research, load, houses]) = figures else {
        return Ok(ck_characters::CharacterStore::default());
    };
    let rules = rules.ok_or_else(|| {
        format!(
            "[game] scenario `{}` starts with its real figures and needs the shared rules (`game/economy.toml`)",
            scenario.id
        )
    })?;
    let read = |path: &Path| {
        std::fs::read_to_string(path)
            .map_err(|error| format!("[game] cannot read {}: {error}", path.display()))
    };
    let houses = read(&houses)?;
    let data = ck_characters::figures::FigureData::parse(&read(&research)?, &read(&load)?)
        .and_then(|data| data.with_houses(&houses))
        .map_err(|error| format!("[game] scenario `{}`: {error}", scenario.id))?;
    let names = place_names(scenario);
    // Every place is within reach of every other at the start: the relatives of clergy are placed at the nearest place that has a pop for
    // them, however far (CK-370 §3 step 2), and the slots still apply their own reach.
    let reach = marriage_reach(scenario, u64::MAX);
    let mut archive = open_archive(storage);
    let mut logs = open_logs(storage);
    let ledger = ck_characters::Ledger::with_nearby(economy, &reach)
        .with_place_names(&names)
        .with_start_safety_net();
    let mut world = ck_characters::World {
        rules: &rules.events,
        personality: &rules.personality,
        genetics: &rules.genetics,
        lifecycle: &rules.lifecycle,
        seed,
        populations: &ledger,
        archive: &mut archive,
        logs: &mut logs,
    };
    let mut characters = ck_characters::CharacterStore::default();
    let report = characters
        .start_768(&mut world, &data, start)
        .map_err(|error| format!("[game] scenario `{}`: {error}", scenario.id))?;
    logs.finish_scope()
        .map_err(|error| format!("[game] scenario `{}`: {error}", scenario.id))?;
    for line in report.lines() {
        log(Level::Info, line);
    }
    Ok(characters)
}

/// The house names of the scenario (`houses.toml`, CK-375), for a new game and a loaded one alike: a scenario that ships no figures has none.
///
/// # Errors
/// A figure file that is missing, or a houses file that cannot be read or does not load.
fn load_houses(directory: &Path) -> Result<ck_characters::figures::Houses, String> {
    let Some([_, _, path]) =
        hashes::figure_files(directory).map_err(|error| format!("[game] {error}"))?
    else {
        return Ok(ck_characters::figures::Houses::default());
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("[game] cannot read {}: {error}", path.display()))?;
    ck_characters::figures::Houses::parse(&text).map_err(|error| format!("[game] {error}"))
}

/// The place of each slot of the scenario's load file by the slot's key (CK-375): empty for a scenario that ships no figures.
///
/// # Errors
/// A figure file that is missing or does not load.
fn load_slot_locations(directory: &Path) -> Result<BTreeMap<String, String>, String> {
    let Some([_, load, _]) =
        hashes::figure_files(directory).map_err(|error| format!("[game] {error}"))?
    else {
        return Ok(BTreeMap::new());
    };
    let text = std::fs::read_to_string(&load)
        .map_err(|error| format!("[game] cannot read {}: {error}", load.display()))?;
    ck_characters::figures::slot_locations(&text).map_err(|error| format!("[game] {error}"))
}

/// The display names of the scenario's places by id, which the character log lines show for a place.
pub(crate) fn place_names(scenario: &ck_map::Scenario) -> BTreeMap<String, String> {
    scenario
        .locations
        .iter()
        .map(|location| (location.id.clone(), location.name.clone()))
        .collect()
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

/// The places within `reach_m` of each place, nearest first, by the great-circle distance in whole metres between their authored
/// anchors (CK-366: the marriage pool's reach). The economy's recruitment table is a separate, approximate contract
/// ([`nearby_places`]) and is not used for this.
pub(crate) fn marriage_reach(
    scenario: &ck_map::Scenario,
    reach_m: u64,
) -> BTreeMap<String, Vec<ck_economy::NearbyPlace>> {
    let mut result = BTreeMap::new();
    for (source_index, source) in scenario.locations.iter().enumerate() {
        let mut places = scenario
            .locations
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != source_index)
            .filter_map(|(index, place)| {
                let metres = ck_map::great_circle_metres(source.position, place.position)?;
                (metres <= reach_m).then_some((metres, index, place.id.clone()))
            })
            .collect::<Vec<_>>();
        places.sort_by_key(|(metres, index, _)| (*metres, *index));
        result.insert(
            source.id.clone(),
            places
                .into_iter()
                .filter_map(|(metres, _, id)| {
                    Some(ck_economy::NearbyPlace {
                        id,
                        distance_m: u32::try_from(metres).ok()?,
                    })
                })
                .collect(),
        );
    }
    result
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
    use super::{ground_distance_m, marriage_reach, nearby_places, nearby_places_for_locations};
    use ck_map::Cell;
    use std::{collections::BTreeMap, path::Path};

    #[test]
    fn the_scenarios_place_names_reach_the_character_passes_by_id() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("Ireland scenario loads");
        let names = super::place_names(&scenario);
        assert_eq!(names["armagh"], "Ard Mhacha");
        assert_eq!(names.len(), scenario.locations.len());
    }

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
            ["illaunloughan", "church_island_valentia"]
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

    #[test]
    fn the_marriage_reach_is_the_great_circle_distance_between_the_authored_anchors() {
        let scenario = ck_map::load_scenario(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenarios/ireland-768"),
        )
        .expect("shipped scenario");
        let between = |reach: &BTreeMap<String, Vec<ck_economy::NearbyPlace>>, a: &str, b: &str| {
            reach[a]
                .iter()
                .find(|place| place.id == b)
                .map(|place| place.distance_m)
        };
        // Lorrha and Inisfallen are 150,704 m apart on the sphere between their authored points: out of a 150 km reach, though the
        // economy's approximate ground distance (a separate contract) puts them inside it.
        let near = marriage_reach(&scenario, 150_000);
        assert_eq!(between(&near, "lorrha", "inisfallen"), None);
        assert_eq!(between(&near, "inisfallen", "lorrha"), None);
        let wide = marriage_reach(&scenario, 200_000);
        let metres = between(&wide, "lorrha", "inisfallen").expect("within 200 km");
        assert!((150_600..=150_800).contains(&metres), "{metres}");
        assert_eq!(between(&wide, "inisfallen", "lorrha"), Some(metres));
        // Nearest first, every distance within the reach, and no place is near itself.
        for (place, others) in &wide {
            assert!(
                others
                    .windows(2)
                    .all(|pair| pair[0].distance_m <= pair[1].distance_m)
            );
            assert!(
                others
                    .iter()
                    .all(|other| other.distance_m <= 200_000 && other.id != *place)
            );
        }
    }
}

#[cfg(test)]
mod stretch_save_tests {
    use super::*;
    use crate::game::{controls::Control, core::Message, stretch::StretchState};
    use std::{
        collections::BTreeMap,
        fs,
        sync::{Arc, Mutex},
        time::Duration,
    };

    fn fixture(dir: &Path) -> (GameConfig, Prepared) {
        let game = GameConfig {
            scenario: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/map/tests/network/scenarios/fixture"),
            saves: dir.join("saves"),
        };
        let prepared = prepare(
            &Request {
                game: &game,
                vms: &[],
                load: None,
                seed: Some(42),
            },
            || Ok(42),
            &mut |_, _| {},
        )
        .unwrap();
        (game, prepared)
    }

    fn save_with(prepared: &Prepared, path: &Path, states: BTreeMap<u64, StretchState>) -> u64 {
        let mut save = prepared.simulation.save(prepared.identity.meta(42));
        save.state.stretches = states;
        let digest = save.digest().unwrap();
        let mut archive = fs::File::open(prepared.storage.path.join("ARCH")).unwrap();
        let mut logs = fs::File::open(prepared.storage.path.join("LOGS")).unwrap();
        let lengths = (
            archive.metadata().unwrap().len(),
            logs.metadata().unwrap().len(),
        );
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = fs::File::create(path).unwrap();
        save.write_with_sections(
            &mut file,
            &mut [
                (*b"ARCH", lengths.0, &mut archive),
                (*b"LOGS", lengths.1, &mut logs),
            ],
        )
        .unwrap();
        digest
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The same malformed saves must be checked through both loaders and their storage bindings."
    )]
    fn both_load_paths_refuse_bad_stretch_states_before_installing_storage() {
        let dir = crate::testing::Dir::new();
        let (game, prepared) = fixture(&dir.0);
        let original = prepared.simulation.state().stretches.clone();
        let ids: Vec<_> = original.keys().copied().collect();
        let mut cases = Vec::new();
        let mut missing = original.clone();
        missing.remove(&ids[0]);
        cases.push((missing, format!("missing stretch {}", ids[0])));
        let mut extra = original.clone();
        extra.insert(u64::MAX, original[&ids[0]].clone());
        cases.push((extra, format!("unexpected stretch {}", u64::MAX)));
        let mut both = original.clone();
        both.remove(&ids[0]);
        both.remove(&ids[1]);
        both.insert(u64::MAX, original[&ids[0]].clone());
        cases.push((both, format!("missing stretch {}", ids[0])));
        for (field, value) in [("category", 2), ("level", 6), ("condition", 1001)] {
            let mut bad = original.clone();
            let s = bad.get_mut(&ids[0]).unwrap();
            match field {
                "category" => s.category = u8::try_from(value).unwrap(),
                "level" => s.level = u8::try_from(value).unwrap(),
                _ => s.condition = value,
            }
            cases.push((bad, format!("stretch {}: invalid {field} {value}", ids[0])));
        }
        let path = super::super::saves::quicksave_path(&game.saves);
        let handle = Arc::clone(prepared.storage.handle.as_ref().unwrap());
        let archive = Arc::clone(prepared.storage.cell.as_ref().unwrap());
        let logs = Arc::clone(prepared.storage.log_cell.as_ref().unwrap());
        let original_path = handle.lock().unwrap().clone();
        let original_archive = archive.lock().unwrap().clone().unwrap();
        let original_logs = logs.lock().unwrap().clone().unwrap();
        for (bad, error) in cases {
            save_with(&prepared, &path, bad);
            let startup = prepare(
                &Request {
                    game: &game,
                    vms: &[],
                    load: Some(&path),
                    seed: None,
                },
                || Ok(42),
                &mut |_, _| {},
            );
            assert!(startup.err().unwrap().contains(&error));
            let records = Arc::new(Mutex::new(Vec::new()));
            let output = Arc::clone(&records);
            // Use a fresh prepared game for each refusal, with the actual attached storage and readers.
            let (_, running) = fixture(&dir.0);
            let (_, unbroken) = fixture(&dir.0);
            let running_handle = Arc::clone(running.storage.handle.as_ref().unwrap());
            let running_archive = Arc::clone(running.storage.cell.as_ref().unwrap());
            let running_logs = Arc::clone(running.storage.log_cell.as_ref().unwrap());
            let before_path = running_handle.lock().unwrap().clone();
            let before_archive = running_archive.lock().unwrap().clone().unwrap();
            let before_logs = running_logs.lock().unwrap().clone().unwrap();
            let mut core = test_core(
                running,
                game.saves.clone(),
                Outputs {
                    log: Box::new(move |_, line| output.lock().unwrap().push(line)),
                    publish: Box::new(|_| {}),
                    fail: Box::new(|e| panic!("{e}")),
                },
            );
            let mut control = test_core(
                unbroken,
                game.saves.clone(),
                Outputs {
                    log: Box::new(|_, _| {}),
                    publish: Box::new(|_| {}),
                    fail: Box::new(|e| panic!("{e}")),
                },
            );
            let before = core.digest();
            assert_eq!(before, control.digest());
            core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
            assert!(
                records
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|line| line.contains(&error))
            );
            assert_eq!(core.digest(), before);
            for world in [&mut core, &mut control] {
                world.dispatch(Message::Control(Control::TogglePause), Duration::ZERO);
                for second in 1..=13 {
                    world.run_wake(Duration::from_secs(second), &mut || None);
                }
            }
            assert_eq!(core.digest(), control.digest());
            assert_eq!(core.snapshot().date.hour, 13, "the noon tick has completed");
            assert_eq!(*running_handle.lock().unwrap(), before_path);
            assert!(Arc::ptr_eq(
                running_archive.lock().unwrap().as_ref().unwrap(),
                &before_archive
            ));
            assert!(Arc::ptr_eq(
                running_logs.lock().unwrap().as_ref().unwrap(),
                &before_logs
            ));
        }
        assert_eq!(*handle.lock().unwrap(), original_path);
        assert!(Arc::ptr_eq(
            archive.lock().unwrap().as_ref().unwrap(),
            &original_archive
        ));
        assert!(Arc::ptr_eq(
            logs.lock().unwrap().as_ref().unwrap(),
            &original_logs
        ));
    }

    #[test]
    fn fixture_stretches_remain_unchanged_through_the_real_daily_and_monthly_systems() {
        let dir = crate::testing::Dir::new();
        let (_, prepared) = fixture(&dir.0);
        let initial = prepared.simulation.state().clone();
        let stretches = initial.stretches.clone();
        assert!(!stretches.is_empty());
        let rules = Rules::load(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
        )
        .unwrap();
        let scheduler = gameplay_scheduler_with_rules(
            None,
            Some(Arc::new(rules)),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            prepared.storage.handle.clone(),
        );
        let mut game = Simulation::<GameState, GameCommand>::new(
            prepared.simulation.time(),
            prepared.simulation.seed(),
            initial,
            scheduler,
            apply,
        );
        let start = game.date().unwrap();
        let mut crossed_month = false;
        while game.date().unwrap().year == start.year {
            let errors = game.step().unwrap();
            assert!(errors.is_empty(), "{errors:?}");
            if game.date().unwrap().month != start.month {
                crossed_month = true;
                assert_eq!(game.state().stretches, stretches);
            }
        }
        assert!(crossed_month);
        let target_year = game.date().unwrap().year;
        while game.date().unwrap().year == target_year
            && (
                game.date().unwrap().month,
                game.date().unwrap().day,
                game.date().unwrap().hour,
            ) != (start.month, start.day, start.hour)
        {
            let errors = game.step().unwrap();
            assert!(errors.is_empty(), "{errors:?}");
        }
        assert_eq!(game.date().unwrap().year, start.year + 1);
        assert_eq!(
            (
                game.date().unwrap().month,
                game.date().unwrap().day,
                game.date().unwrap().hour
            ),
            (start.month, start.day, start.hour)
        );
        assert_eq!(game.state().stretches, stretches);
    }

    #[test]
    fn mixed_stretches_round_trip_and_saves_bracket_noon() {
        let dir = crate::testing::Dir::new();
        let (game, mut prepared) = fixture(&dir.0);
        let path = super::super::saves::quicksave_path(&game.saves);
        let mut mixed = prepared.simulation.state().stretches.clone();
        let ids: Vec<_> = mixed.keys().copied().collect();
        for (id, category, level, condition) in [
            (ids[0], 1, 1, 0),
            (ids[1], 1, 1, 437),
            (ids[2], 0, 4, 1000),
            (ids[3], 0, 0, 600),
        ] {
            mixed.insert(
                id,
                StretchState {
                    category,
                    level,
                    condition,
                },
            );
        }
        let digest = save_with(&prepared, &path, mixed.clone());
        let load = || {
            prepare(
                &Request {
                    game: &game,
                    vms: &[],
                    load: Some(&path),
                    seed: None,
                },
                || Ok(42),
                &mut |_, _| {},
            )
            .unwrap()
        };
        let loaded = load();
        assert_eq!(loaded.simulation.state().stretches, mixed);
        assert_eq!(loaded.simulation.digest().unwrap(), digest);
        for hour in [11, 13] {
            while prepared.simulation.date().unwrap().hour != hour {
                prepared.simulation.step().unwrap();
            }
            let state = prepared.simulation.state().stretches.clone();
            let digest = save_with(&prepared, &path, state.clone());
            let mut loaded = load();
            assert_eq!(loaded.simulation.digest().unwrap(), digest);
            assert_eq!(loaded.simulation.state().stretches, state);
            for _ in 0..24 {
                prepared.simulation.step().unwrap();
                loaded.simulation.step().unwrap();
            }
            assert_eq!(
                loaded.simulation.digest().unwrap(),
                prepared.simulation.digest().unwrap()
            );
        }
    }

    #[test]
    fn shipped_network_initializes_every_stretch_and_exactly_the_five_roads() {
        let dir = crate::testing::Dir::new();
        let game = GameConfig {
            scenario: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/scenarios/ireland-768"),
            saves: dir.0.join("saves"),
        };
        let prepared = prepare(
            &Request {
                game: &game,
                vms: &[],
                load: None,
                seed: Some(42),
            },
            || Ok(42),
            &mut |_, _| {},
        )
        .unwrap();
        let network = prepared.scenario.network.as_ref().unwrap();
        let states = &prepared.simulation.state().stretches;
        assert_eq!(states.len(), network.stretches.len());
        let roads: std::collections::BTreeSet<_> = network
            .roads
            .iter()
            .flat_map(|road| &road.links)
            .flat_map(|id| {
                network
                    .links
                    .iter()
                    .find(|link| &link.id == id)
                    .unwrap()
                    .stretches
                    .iter()
                    .map(|(id, _)| *id)
            })
            .collect();
        assert_eq!(network.roads.len(), 5);
        for stretch in &network.stretches {
            let state = &states[&stretch.id];
            assert_eq!(
                state.category,
                u8::from(stretch.category == ck_map::LandCategory::Road)
            );
            assert_eq!(state.level, stretch.level);
            assert_eq!(state.condition, 1000);
            assert_eq!(state.category == 1, roads.contains(&stretch.id));
            if roads.contains(&stretch.id) {
                assert_eq!(state.level, 1);
            }
        }
    }
}
