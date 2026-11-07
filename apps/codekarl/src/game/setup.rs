//! Starting a game: a new one from the scenario's start, or one loaded from a save. Everything here happens before a window
//! opens or a thread starts, so a refusal is a configuration error and nothing is left running.

use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use ck_log::Level;
use ck_runtime::OwnerSpawn;
use ck_sim::{Cadence, Date, GameTime, Save, Scheduler, Simulation};

use super::{
    GameCommand, GameState, apply,
    controls::SimControls,
    core::{Core, Outputs, Rebuild},
    gameplay::{self, Gameplay, GameplayWiring},
    hashes,
    owner::{SimOwner, channel},
    saves::{Identity, build_id},
    slot::SimSlot,
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
    let identity = Identity {
        build: build_id(),
        scenario: scenario.id.clone(),
        scenario_hash: hashes::scenario_hash(directory)
            .map_err(|error| format!("[game] {error}"))?,
        map_hash: hashes::map_directory(directory)
            .and_then(|map| hashes::map_hash(&map))
            .map_err(|error| format!("[game] {error}"))?,
        scripts_hash: hashes::scripts_hash(gameplay_entry)?,
        start,
    };
    let gameplay_runtime = request
        .vms
        .iter()
        .any(|vm| vm.name == GAMEPLAY_VM)
        .then(|| Gameplay::new(0));
    let scheduler = || gameplay_scheduler(gameplay_runtime.as_ref());
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
        Simulation::new(start_time, seed, GameState, scheduler(), apply)
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
pub(crate) fn connect(prepared: Prepared, wake: Arc<dyn Fn() + Send + Sync>) -> Connected {
    let Prepared {
        identity,
        simulation,
        gameplay,
        saves,
        ..
    } = prepared;
    let wiring_gameplay = gameplay.clone();
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
        let reset_runtime = runtime.clone();
        let build = move || {
            Core::new(
                simulation,
                identity,
                saves,
                Rebuild {
                    scheduler: Box::new(move || gameplay_scheduler(scheduler_runtime.as_ref())),
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

pub(crate) fn gameplay_scheduler(gameplay: Option<&Arc<Gameplay>>) -> Scheduler<GameState> {
    let mut scheduler = Scheduler::new();
    let Some(gameplay) = gameplay else {
        return scheduler;
    };
    register_gameplay_callbacks(&mut scheduler, gameplay);
    scheduler
}

pub(crate) fn register_gameplay_callbacks(
    scheduler: &mut Scheduler<GameState>,
    gameplay: &Arc<Gameplay>,
) {
    let monthly = Arc::clone(gameplay);
    scheduler
        .register("gameplay.on_month", Cadence::Monthly, move |_, tick| {
            monthly
                .call("on_month", tick.date(), tick.time())
                .map_err(io::Error::other)
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
        })
        .expect("the gameplay monthly system name is unique");
    let daily = Arc::clone(gameplay);
    scheduler
        .register("gameplay.on_day", Cadence::Daily, move |_, tick| {
            daily
                .call("on_day", tick.date(), tick.time())
                .map_err(io::Error::other)
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
        })
        .expect("the gameplay daily system name is unique");
}
