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
    pub(super) log_choices: ck_sim::LogChoices,
    pub(super) gameplay: Option<GameplayWiring>,
    /// The shared economy rules the view is derived with; `None` for assets that predate M9.
    economy_rules: Option<Arc<Rules>>,
    weather: Option<Arc<WeatherData>>,
    pub(super) events: super::events::Fixture,
    /// The house names the character view shows (`houses.toml`, CK-375): empty for a scenario that ships no figures.
    houses: Arc<ck_characters::figures::Houses>,
    /// The place of each authored slot by its key (the load file's `[[slot]]`): the character view reads a spouse place there.
    slots: Arc<BTreeMap<String, String>>,
    /// Map-derived makeshift shelter by place, not saved in `EconomyState`.
    shelter_by_place: BTreeMap<String, u64>,
    saves: PathBuf,
    history: super::history::Recorder,
    pub(super) storage: super::character_storage::Storage,
    /// Where the view builder finds the current archive reader (CK-377).
    archive_cell: super::character_storage::SourceCell,
    /// Where the view builder finds the current life-log reader (CK-378).
    log_cell: super::character_storage::LogCell,
    /// Where the scheduler's daily system finds the character storage (CK-363).
    characters_io: super::character_storage::Handle,
    /// The travel network the traders' looks read, when the rules have looks on (CK-433); none otherwise.
    trade_network: Option<Arc<super::trade_ways::TradeNetwork>>,
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
    let weather = if economy_rules.is_some() {
        Some(Arc::new(WeatherData::load(asset_root, &scenario)?))
    } else {
        None
    };
    let events = if let Some(path) = &request.game.events_fixture {
        let goods = economy_rules.as_ref().map_or_else(Vec::new, |rules| {
            rules.goods.iter().map(|g| g.id.clone()).collect()
        });
        let places = scenario
            .locations
            .iter()
            .map(|p| p.id.clone())
            .collect::<Vec<_>>();
        super::events::Fixture::load(path, &goods, &places)?
    } else {
        super::events::Fixture::default()
    };
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
    let land_for_switching = if let Some(rules) = economy_rules.as_deref() {
        economy::land_counts(&scenario, &rules.terrain_order)?
            .into_iter()
            .enumerate()
            .map(|(index, land)| (scenario.locations[index].id.clone(), land))
            .collect::<BTreeMap<_, _>>()
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
    let trade_by_place = trade_neighbours(&scenario);
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
            .and_then(|map| {
                hashes::map_hash_with_backdrop(&map, asset_root, economy_rules.is_some())
            })
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
    let trade_network = trade_network(
        &scenario,
        asset_root,
        economy_rules.as_deref(),
        &trade_by_place,
    )?;
    let characters_io = super::character_storage::Handle::default();
    let archive_cell = super::character_storage::SourceCell::default();
    let log_cell = super::character_storage::LogCell::default();
    let scheduler = || {
        gameplay_scheduler_with_rules(
            gameplay_runtime.as_ref(),
            economy_rules.clone(),
            quality_by_place.clone(),
            land_for_switching.clone(),
            shelter_by_place.clone(),
            nearby_by_place.clone(),
            reach_by_place.clone(),
            trade_by_place.clone(),
            place_names.clone(),
            weather.clone(),
            Some(Arc::clone(&characters_io)),
            trade_network.clone(),
        )
    };
    let storage_parent = request.game.saves.parent().unwrap_or(Path::new("."));
    let (simulation, storage, layers, log_choices, history) = if let Some(path) = request.load {
        use std::io::{Seek, SeekFrom};
        let mut file = std::fs::File::open(path)
            .map_err(|error| format!("cannot read save {}: {error}", path.display()))?;
        let handoff = super::history::Handoff::begin(&mut file, &request.game.saves);
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
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
        if let Some(rules) = economy_rules.as_deref() {
            ck_economy::validate_fuel_state(rules, &save.state.economy)
                .map_err(|e| e.to_string())?;
            ck_economy::validate_trade_state_at(rules, &save.state.economy, save.time.hours())
                .map_err(|e| e.to_string())?;
        }
        let expected = super::stretch::starting(scenario.network.as_ref())
            .into_keys()
            .collect();
        super::stretch::validate(&save.state.stretches, &expected)?;
        if let Some(rules) = economy_rules.as_deref() {
            ck_economy::validate_level_records(rules, &quality_by_place, &save.state.economy)
                .map_err(|error| format!("save {}: {error}", path.display()))?;
            let date = save
                .time
                .date()
                .map_err(|error| format!("save {}: {error}", path.display()))?;
            ck_economy::validate_switch_years(&save.state.economy, date.year, date.month)
                .map_err(|error| format!("save {}: {error}", path.display()))?;
        }
        validate_weather_list(weather.as_deref(), &save.state.lows)?;
        super::character_storage::Storage::validate(&storage.path, &save.state)
            .map_err(|e| e.to_string())?;
        let layers = save.meta.layers;
        let log_choices = save.meta.log;
        let simulation = Simulation::from_save(save, scheduler(), apply)
            .map_err(|error| format!("save {}: {error}", path.display()))?;
        let date = simulation.date().unwrap_or(start);
        let history = handoff
            .finish(&mut file, &request.game.saves, date)
            .map_err(|e| e.to_string())?;
        log(Level::Info, format!("loaded {} at {date}", path.display()));
        (simulation, storage, layers, log_choices, history)
    } else {
        let seed = match request.seed {
            Some(seed) => seed,
            None => entropy()?,
        };
        log(Level::Info, format!("new game, seed {seed:016x}"));
        let (economy_state, load_report) =
            economy::starting_economy_seeded(asset_root, directory, &scenario, seed)?;
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
                    lows: crate::game::lows::Lows::default(),
                },
                scheduler(),
                apply,
            ),
            storage,
            ck_sim::MapLayers::default(),
            ck_sim::LogChoices::default(),
            super::history::Recorder::new(
                &request.game.saves,
                operating_system_seed()?,
                (start.year, start.month),
            ),
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
        log_choices,
        gameplay,
        economy_rules,
        weather,
        events,
        houses,
        slots,
        shelter_by_place,
        trade_network,
        saves: request.game.saves.clone(),
        history,
        storage,
        characters_io,
        archive_cell,
        log_cell,
    })
}

struct WeatherData {
    backdrop: Option<ck_map::Backdrop>,
    steering_cache: Arc<std::sync::OnceLock<Vec<i64>>>,
    rules: ck_sim::weather::WeatherRules,
    places: Vec<(String, ck_sim::weather::WeatherPlace, [i64; 12])>,
    winds: Vec<Option<[(i64, i64, i64); 12]>>,
    cells: Vec<Option<(u32, u32)>>,
}

impl WeatherData {
    fn load(root: &Path, scenario: &ck_map::Scenario) -> Result<Self, String> {
        use ck_sim::weather::{WeatherPlace, WeatherRules};
        let path = root.join("game/weather.toml");
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let rules = WeatherRules::parse(&text)?;
        let backdrop_path = root.join("map/backdrop");
        let backdrop = if backdrop_path.exists() {
            Some(ck_map::load_backdrop(&backdrop_path).map_err(|e| e.to_string())?)
        } else {
            None
        };
        let field = backdrop
            .as_ref()
            .map(|b| super::wind::WindField::new(b, &rules))
            .transpose()?;
        let mut winds = Vec::new();
        let mut cells = Vec::new();
        let data = &scenario.map.data;
        let crop = data.crop;
        let mut places = Vec::new();
        for location in &scenario.locations {
            let x = usize::try_from(location.cell.x - crop.origin.x).map_err(|e| e.to_string())?;
            let y = usize::try_from(location.cell.y - crop.origin.y).map_err(|e| e.to_string())?;
            let index = y * crop.width as usize + x;
            let place = WeatherPlace {
                latitude_e4: i64::from(location.latitude_e4),
                longitude_e4: i64::from(location.longitude_e4),
                mean: i64::from(data.climate_temperature_tenths[index]),
                half_range: i64::from(data.climate_half_range[index]),
                nearness: i64::from(data.climate_sea_nearness[index]),
                phase: i64::from(data.climate_phase),
            };
            let expected = place.expected_year(&rules, i64::from(scenario.start.year))?;
            let monthly = field.as_ref().and_then(|field| {
                let i = u32::try_from(location.cell.x).ok()?;
                let j = u32::try_from(location.cell.y).ok()?;
                let mut values = [(0, 0, 0); 12];
                for (month, value) in values.iter_mut().enumerate() {
                    *value = field.monthly(i, j, month)?;
                }
                Some(values)
            });
            winds.push(monthly);
            cells.push(
                u32::try_from(location.cell.x)
                    .ok()
                    .zip(u32::try_from(location.cell.y).ok()),
            );
            places.push((location.id.clone(), place, expected));
        }
        let steering_cache = field
            .as_ref()
            .map(|f| f.steering_cache.clone())
            .unwrap_or_default();
        Ok(Self {
            backdrop,
            steering_cache,
            rules,
            places,
            winds,
            cells,
        })
    }

    fn validate_lows(&self, lows: &super::lows::Lows) -> Result<(), String> {
        match &self.backdrop {
            Some(backdrop) => {
                super::wind::WindField::new(backdrop, &self.rules)?.validate_list(lows)
            }
            None if lows.next >= 1 && lows.entries.is_empty() => Ok(()),
            None => Err("weather list: no backdrop or invalid next number".into()),
        }
    }

    fn step_lows(&self, lows: &mut super::lows::Lows, date: Date) {
        if lows.entries.is_empty() {
            return;
        }
        if let Some(backdrop) = &self.backdrop {
            let offset = ck_sim::weather::julian_day(
                i64::from(date.year),
                i64::from(date.month),
                i64::from(date.day),
            ) - ck_sim::weather::julian_day(1, 1, 1);
            let (year, month, day) = ck_sim::weather::gregorian(offset).expect("validated date");
            let mut field =
                super::wind::WindField::new(backdrop, &self.rules).expect("validated backdrop");
            field.steering_cache = self.steering_cache.clone();
            field.step(
                lows,
                year,
                usize::try_from(month).expect("valid month"),
                day,
            );
        }
    }

    fn daily_means(&self, date: Date) -> Result<BTreeMap<String, i64>, String> {
        let day = ck_sim::weather::julian_day(
            i64::from(date.year),
            i64::from(date.month),
            i64::from(date.day),
        ) - ck_sim::weather::julian_day(1, 1, 1);
        self.places
            .iter()
            .map(|(id, place, _)| Ok((id.clone(), place.daily_mean(&self.rules, day)?)))
            .collect()
    }

    fn hourly(&self, lows: &super::lows::Lows, date: Date) -> super::slot::HourlyWeather {
        let day = ck_sim::weather::julian_day(
            i64::from(date.year),
            i64::from(date.month),
            i64::from(date.day),
        ) - ck_sim::weather::julian_day(1, 1, 1);
        let (year, month, day_of_month) =
            ck_sim::weather::gregorian(day).expect("validated weather day");
        let field = self.backdrop.as_ref().map(|backdrop| {
            super::wind::WindField::new(backdrop, &self.rules).expect("validated backdrop")
        });
        self.places
            .iter()
            .enumerate()
            .map(|(index, (id, place, _))| {
                let temperature = place
                    .hour(&self.rules, day, i64::from(date.hour))
                    .expect("validated weather hour");
                let wind = field
                    .as_ref()
                    .zip(self.cells.get(index).copied().flatten())
                    .and_then(|(field, (i, j))| {
                        field.at(
                            lows,
                            i,
                            j,
                            year,
                            usize::try_from(month).expect("valid month"),
                            day_of_month,
                        )
                    })
                    .map(|(u, v, _)| {
                        let (knots, direction) = super::wind::display(u, v);
                        (knots, direction.to_owned())
                    });
                (id.clone(), (temperature, wind))
            })
            .collect()
    }

    fn add_to_view(&self, view: &mut view::EconomyView, date: Date) {
        let day = ck_sim::weather::julian_day(
            i64::from(date.year),
            i64::from(date.month),
            i64::from(date.day),
        ) - ck_sim::weather::julian_day(1, 1, 1)
            - i64::from(date.hour <= 12);
        let month = if date.hour <= 12 && date.day == 1 {
            if date.month == 1 { 12 } else { date.month - 1 }
        } else {
            date.month
        };
        let (year, gregorian_month, gregorian_day) =
            ck_sim::weather::gregorian(day).expect("validated weather day");
        for (index, (view, (_, place, expected))) in
            view.places.iter_mut().zip(&self.places).enumerate()
        {
            if let Some(Some(months)) = self.winds.get(index) {
                let (u, v, _) = super::wind::day(
                    months,
                    year,
                    usize::try_from(gregorian_month).expect("valid month"),
                    gregorian_day,
                );
                let (knots, direction) = super::wind::display(u, v);
                view.wind = Some((knots, direction.to_owned()));
            }
            view.temperature = Some((
                place
                    .daily_mean(&self.rules, day)
                    .expect("validated weather day"),
                month,
                expected[usize::from(month - 1)],
            ));
        }
    }
}

/// What the view builder needs: the economy and character views of a state are built from these (CK-375, CK-377).
pub(super) struct ViewWiring {
    rules: Option<Arc<Rules>>,
    weather: Option<Arc<WeatherData>>,
    place_ids: Vec<String>,
    land_by_place: BTreeMap<String, ck_economy::LandCounts>,
    place_names: Arc<BTreeMap<String, String>>,
    /// The English-first name of each place by id: its English name, else its local one (CK-423).
    place_names_english: Arc<BTreeMap<String, String>>,
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
            weather: self.weather.clone(),
            place_ids,
            land_by_place,
            place_names: Arc::new(place_names(&self.scenario)),
            place_names_english: Arc::new(english_place_names(&self.scenario)),
            houses: Arc::clone(&self.houses),
            slots: Arc::clone(&self.slots),
            archive_cell: Arc::clone(&self.archive_cell),
            log_cell: Arc::clone(&self.log_cell),
            start_year: self.scenario.start.year,
        }
    }
}

impl ViewWiring {
    /// The check a loaded state's building levels pass before it is installed (CK-406): every level record fits its method, and the
    /// carries are below their denominators, which depend on each place's mean fertility.
    pub(super) fn level_validation(
        &self,
    ) -> impl Fn(&GameState, Date) -> Result<(), String> + Send + 'static {
        let rules = self.rules.clone();
        let weather = self.weather.clone();
        let quality = self
            .land_by_place
            .iter()
            .filter_map(|(id, land)| Some((id.clone(), ck_economy::mean_fertility(land).ok()?)))
            .collect::<BTreeMap<_, _>>();
        move |state, date| {
            validate_weather_list(weather.as_deref(), &state.lows)?;
            match rules.as_deref() {
                Some(rules) => {
                    ck_economy::validate_level_records(rules, &quality, &state.economy)?;
                    ck_economy::validate_switch_years(&state.economy, date.year, date.month)
                }
                None => Ok(()),
            }
        }
    }
}

fn validate_weather_list(
    weather: Option<&WeatherData>,
    lows: &super::lows::Lows,
) -> Result<(), String> {
    match weather {
        Some(data) => data.validate_lows(lows),
        None if lows.next >= 1 && lows.entries.is_empty() => Ok(()),
        None => Err("weather list: no backdrop or invalid next number".into()),
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
        if let Some(weather) = &wiring.weather {
            weather.add_to_view(&mut view, date);
        }
        if let Some(rules_arc) = wiring.rules.as_ref() {
            let rules: &Rules = rules_arc;
            view.characters = super::character_view::build(
                &state.characters,
                &state.economy,
                &super::character_view::Inputs {
                    rules,
                    places: &wiring.place_names,
                    english: &wiring.place_names_english,
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
                    english: &wiring.place_names_english,
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
    core_over(prepared, identity_saves, outputs, Scheduler::new)
}

/// The same, with the scheduler the running game builds after a load: its daily system, with the map's land, quality and shelter.
#[cfg(test)]
pub(super) fn test_core_running(
    prepared: Prepared,
    identity_saves: PathBuf,
    outputs: Outputs,
) -> Core<GameState, GameCommand> {
    let inputs = SchedulerInputs::new(&prepared);
    core_over(prepared, identity_saves, outputs, move || {
        inputs.scheduler()
    })
}

#[cfg(test)]
fn core_over(
    prepared: Prepared,
    identity_saves: PathBuf,
    outputs: Outputs,
    scheduler: impl Fn() -> Scheduler<GameState> + Send + 'static,
) -> Core<GameState, GameCommand> {
    let wiring = prepared.view_wiring();
    let level_validation = wiring.level_validation();
    let hourly_weather = wiring.weather.clone();
    let fuel_rules = prepared.economy_rules.clone();
    let expedition_rules = prepared.economy_rules.clone();
    let trade_panel_rules = prepared.economy_rules.clone();
    let history_rules = prepared.economy_rules.clone();
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
        events,
        log_choices,
        history,
        ..
    } = prepared;
    Core::new_with_events(
        simulation,
        identity,
        identity_saves,
        Rebuild {
            scheduler: Box::new(scheduler),
            reset: Box::new(|_| {}),
            apply,
        },
        outputs,
        Some(view_builder(wiring)),
        Some(Box::new(move |state: &GameState, date| {
            hourly_weather
                .as_ref()
                .map(|weather| weather.hourly(&state.lows, date))
                .unwrap_or_default()
        })),
        (layers, events, log_choices),
    )
    .with_history(history, history_builder(history_rules))
    .with_expeditions(move |state: &GameState| {
        expedition_rules.as_deref().map_or_else(Vec::new, |rules| {
            super::expedition_map::build(&state.economy, rules)
        })
    })
    .with_pop_trade(move |state: &GameState| {
        trade_panel_rules.as_deref().map_or_else(Vec::new, |rules| {
            super::trade_panel::build(&state.economy, rules)
        })
    })
    .with_state_validation(move |state: &GameState, date| {
        super::stretch::validate(&state.stretches, &stretch_ids)?;
        level_validation(state, date)?;
        if let Some(rules) = fuel_rules.as_deref() {
            ck_economy::validate_fuel_state(rules, &state.economy).map_err(|e| e.to_string())?;
            ck_economy::validate_trade_state_at(rules, &state.economy, hours_of(date)?)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .with_character_storage(storage, super::character_storage::Storage::validate)
}

fn history_builder(rules: Option<Arc<Rules>>) -> super::core::HistoryBuilder<GameState> {
    Box::new(move |state, year, month| {
        rules.as_ref().map_or_else(
            || super::history::MonthRecord {
                year,
                month,
                places: Vec::new(),
            },
            |rules| super::history::MonthRecord::sample(rules, &state.economy, year, month),
        )
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

/// What a game's scheduler is built from, kept to build it again after each load: the running game and the tests of a load during
/// play build their scheduler from the same inputs.
struct SchedulerInputs {
    runtime: Option<Arc<Gameplay>>,
    rules: Option<Arc<Rules>>,
    quality_by_place: BTreeMap<String, u16>,
    land_by_place: BTreeMap<String, ck_economy::LandCounts>,
    shelter_by_place: BTreeMap<String, u64>,
    nearby_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    reach_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    trade_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    place_names: BTreeMap<String, String>,
    weather: Option<Arc<WeatherData>>,
    characters_io: super::character_storage::Handle,
    trade_network: Option<Arc<super::trade_ways::TradeNetwork>>,
}

impl SchedulerInputs {
    fn new(prepared: &Prepared) -> Self {
        let wiring = prepared.view_wiring();
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
        let rules = prepared.economy_rules.clone();
        Self {
            runtime: prepared
                .gameplay
                .as_ref()
                .map(|wiring| Arc::clone(&wiring.runtime)),
            nearby_by_place: nearby_places(
                &prepared.scenario,
                rules.as_deref().map_or(0, max_recruit_range_m),
            ),
            trade_by_place: trade_neighbours(&prepared.scenario),
            reach_by_place: marriage_reach(
                &prepared.scenario,
                rules
                    .as_deref()
                    .map_or(0, |rules| rules.lifecycle.marriage.pool.far_m),
            ),
            place_names: place_names(&prepared.scenario),
            weather: wiring.weather.clone(),
            characters_io: Arc::clone(&prepared.characters_io),
            trade_network: prepared.trade_network.clone(),
            shelter_by_place: prepared.shelter_by_place.clone(),
            rules,
            quality_by_place,
            land_by_place,
        }
    }

    fn scheduler(&self) -> Scheduler<GameState> {
        gameplay_scheduler_with_rules(
            self.runtime.as_ref(),
            self.rules.clone(),
            self.quality_by_place.clone(),
            self.land_by_place.clone(),
            self.shelter_by_place.clone(),
            self.nearby_by_place.clone(),
            self.reach_by_place.clone(),
            self.trade_by_place.clone(),
            self.place_names.clone(),
            self.weather.clone(),
            Some(Arc::clone(&self.characters_io)),
            self.trade_network.clone(),
        )
    }
}

/// Wires `prepared` to the run: `wake` wakes the main thread whenever the owner publishes.
#[expect(
    clippy::too_many_lines,
    reason = "The connected simulation wires its hourly readbacks and lifecycle together."
)]
pub(crate) fn connect(prepared: Prepared, wake: Arc<dyn Fn() + Send + Sync>) -> Connected {
    let wiring = prepared.view_wiring();
    let level_validation = wiring.level_validation();
    let hourly_weather = wiring.weather.clone();
    let scheduler_inputs = SchedulerInputs::new(&prepared);
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
        log_choices,
        gameplay,
        economy_rules,
        events,
        saves,
        storage,
        history,
        ..
    } = prepared;
    let wiring_gameplay = gameplay.clone();
    let fuel_rules = economy_rules.clone();
    let expedition_rules = economy_rules.clone();
    let trade_panel_rules = economy_rules.clone();
    let history_rules = economy_rules.clone();
    let (sender, receiver) = channel();
    let controls = SimControls::with_wake(sender.clone(), Arc::clone(&wake));
    let history_host = controls.history().clone();
    let slot = SimSlot::new(wake);
    let owned_slot = Arc::clone(&slot);
    let spawn = OwnerSpawn::new("sim", move || {
        let publishing = Arc::clone(&owned_slot);
        let failing = Arc::clone(&owned_slot);
        let runtime = gameplay.as_ref().map(|wiring| Arc::clone(&wiring.runtime));
        let terminal_runtime = runtime.clone();
        let reset_runtime = runtime.clone();
        let build = move || {
            Core::new_with_events(
                simulation,
                identity,
                saves,
                Rebuild {
                    scheduler: Box::new(move || scheduler_inputs.scheduler()),
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
                Some(Box::new(move |state: &GameState, date| {
                    hourly_weather
                        .as_ref()
                        .map(|weather| weather.hourly(&state.lows, date))
                        .unwrap_or_default()
                })),
                (layers, events, log_choices),
            )
            .with_history(history, history_builder(history_rules))
            .with_history_host(history_host)
            .with_expeditions(move |state: &GameState| {
                expedition_rules.as_deref().map_or_else(Vec::new, |rules| {
                    super::expedition_map::build(&state.economy, rules)
                })
            })
            .with_pop_trade(move |state: &GameState| {
                trade_panel_rules.as_deref().map_or_else(Vec::new, |rules| {
                    super::trade_panel::build(&state.economy, rules)
                })
            })
            .with_state_validation(move |state: &GameState, date| {
                super::stretch::validate(&state.stretches, &stretch_ids)?;
                level_validation(state, date)?;
                if let Some(rules) = fuel_rules.as_deref() {
                    ck_economy::validate_fuel_state(rules, &state.economy)
                        .map_err(|e| e.to_string())?;
                    ck_economy::validate_trade_state_at(rules, &state.economy, hours_of(date)?)
                        .map_err(|e| e.to_string())?;
                }
                Ok(())
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

/// The game hour of a date the state validation is given, for checks of what the clock has already passed.
fn hours_of(date: Date) -> Result<u64, String> {
    GameTime::from_date(date)
        .map(GameTime::hours)
        .map_err(|_| "the date is outside the calendar".to_owned())
}

#[expect(
    clippy::too_many_arguments,
    reason = "the rules and each place table the daily pass reads, the place names and the storage handle"
)]
fn gameplay_scheduler_with_rules(
    gameplay: Option<&Arc<Gameplay>>,
    rules: Option<Arc<Rules>>,
    quality_by_place: BTreeMap<String, u16>,
    land_by_place: BTreeMap<String, ck_economy::LandCounts>,
    shelter_by_place: BTreeMap<String, u64>,
    nearby_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    reach_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    trade_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    place_names: BTreeMap<String, String>,
    weather: Option<Arc<WeatherData>>,
    characters_io: Option<super::character_storage::Handle>,
    trade_network: Option<Arc<super::trade_ways::TradeNetwork>>,
) -> Scheduler<GameState> {
    let mut scheduler = Scheduler::<GameState>::new();
    if let Some(weather) = weather.clone() {
        scheduler
            .register("weather.hourly", Cadence::Hourly, move |state, tick| {
                weather.step_lows(&mut state.lows, tick.date());
                Ok(())
            })
            .expect("weather system name is unique");
    }
    if let Some(rules) = rules {
        register_trade_hourly(&mut scheduler, Arc::clone(&rules), trade_network);
        register_economy_callback_with_context(
            &mut scheduler,
            rules,
            quality_by_place,
            land_by_place,
            shelter_by_place,
            nearby_by_place,
            reach_by_place,
            trade_by_place,
            place_names,
            weather,
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

/// The travel network the traders' looks read (CK-433): loaded only when the rules have looks on, which the shipped rules do not until
/// part F opens a game with its first looks, so a game without them reads no travel rules and holds no copy of the map.
fn trade_network(
    scenario: &ck_map::Scenario,
    asset_root: &Path,
    rules: Option<&Rules>,
    neighbours: &BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
) -> Result<Option<Arc<super::trade_ways::TradeNetwork>>, String> {
    let Some(rules) = rules.filter(|rules| rules.expedition.looks) else {
        return Ok(None);
    };
    if scenario.network.is_none() {
        return Ok(None);
    }
    let travel = std::fs::read_to_string(asset_root.join("game/travel.toml"))
        .map_err(|error| format!("[game] game/travel.toml: {error}"))?;
    Ok(Some(Arc::new(super::trade_ways::TradeNetwork {
        scenario: scenario.clone(),
        rules: ck_sim::travel::TravelRules::parse(&travel)
            .map_err(|error| format!("[game] game/travel.toml: {error}"))?,
        neighbours: neighbours.clone(),
        radius_m: rules.expedition.visit_radius_m,
    })))
}

/// The trade systems' hour (CK-431, trade spec 11c.1): builds that are done complete, ready jobs start, expired jobs are given up. The
/// scheduler runs a tick's daily systems before its hourly ones, so at noon this follows the economy's daily step.
pub(crate) fn register_trade_hourly(
    scheduler: &mut Scheduler<GameState>,
    rules: Arc<Rules>,
    network: Option<Arc<super::trade_ways::TradeNetwork>>,
) {
    // The traders' looks (CK-433, part D) run after the hour's jobs when the rules have them on and the game has a network to look at;
    // the searches they share are kept for the hour in `ways`.
    let mut ways = network
        .filter(|_| rules.expedition.looks)
        .map(|network| super::trade_ways::NetworkWays::new(network, None));
    scheduler
        .register("economy.trade", Cadence::Hourly, move |state, tick| {
            let result = match ways.as_mut() {
                Some(ways) => {
                    ways.begin(super::travel::current_way_states(&state.stretches));
                    ck_economy::run_trade_hour_with_looks(
                        &rules,
                        &mut state.economy,
                        tick.date(),
                        ways,
                        &mut (),
                    )
                }
                None => ck_economy::run_trade_hour(&rules, &mut state.economy, tick.date()),
            };
            result
                .map_err(io::Error::other)
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
        })
        .expect("the trade system name is unique");
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
        BTreeMap::new(),
        BTreeMap::new(),
        None,
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
        BTreeMap::new(),
        BTreeMap::new(),
        None,
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
    land_by_place: BTreeMap<String, ck_economy::LandCounts>,
    shelter_by_place: BTreeMap<String, u64>,
    nearby_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    reach_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    trade_by_place: BTreeMap<String, Vec<ck_economy::NearbyPlace>>,
    place_names: BTreeMap<String, String>,
    weather: Option<Arc<WeatherData>>,
    characters_io: Option<super::character_storage::Handle>,
) {
    scheduler
        .register_with_streams(
            "economy.daily",
            Cadence::Daily { hour: ECONOMY_HOUR },
            &[CHARACTER_STREAM],
            move |state, tick| {
                // Derived temperatures are available for future economy rules and remain unsaved.
                let _daily_temperatures = weather
                    .as_ref()
                    .map(|data| data.daily_means(tick.date()))
                    .transpose()
                    .map_err(io::Error::other)?;
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
                ck_economy::run_daily_economy_with_trade_neighbours(
                    &rules,
                    &mut state.economy,
                    tick.date(),
                    &quality_by_place,
                    &shelter_by_place,
                    &nearby_by_place,
                    &floors,
                    &land_by_place,
                    &trade_by_place,
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

/// The English-first name of each of the scenario's places by id: its English name when it has one, else its local name (CK-423). The
/// character views carry the role lines and passion names built with these beside the ones built with [`place_names`], and the UI
/// calls pick between them by the player's choice.
pub(crate) fn english_place_names(scenario: &ck_map::Scenario) -> BTreeMap<String, String> {
    scenario
        .locations
        .iter()
        .map(|location| {
            (
                location.id.clone(),
                location
                    .name_en
                    .clone()
                    .unwrap_or_else(|| location.name.clone()),
            )
        })
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

/// Neighbours trade directly when their rounded ground distance is under 4 km.
pub(crate) fn trade_neighbours(
    scenario: &ck_map::Scenario,
) -> BTreeMap<String, Vec<ck_economy::NearbyPlace>> {
    marriage_reach(scenario, 3_999)
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

    /// The shipped Ireland scenario's English names (CK-423): the four places the hand check could not find, the fallback for a place with
    /// none, and the local names the character log prose is written with.
    #[test]
    fn ireland_has_english_names_where_one_is_established_and_the_log_names_stay_local() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("Ireland scenario loads");
        let english = |id: &str| {
            scenario
                .locations
                .iter()
                .find(|location| location.id == id)
                .unwrap_or_else(|| panic!("{id} is shipped"))
                .name_en
                .as_deref()
        };
        for (id, name) in [
            ("armagh", "Armagh"),
            ("bangor", "Bangor"),
            ("birr", "Birr"),
            ("cork", "Cork"),
            ("skellig_michael", "Skellig Michael"),
            // From the CK-423 review: sites a first pass missed or named for another monument.
            ("rathmore", "Rathmore"),
            ("aidne_seat", "Rath Durlais"),
            ("dun_ailinne", "Dun Aillinne"),
            ("ui_maine_seat", "Dunamon"),
        ] {
            assert_eq!(english(id), Some(name), "{id}");
        }
        // A people's seat has no established English place name, and gets none: nothing is invented.
        assert_eq!(english("cairpre_seat"), None);
        let without: Vec<&str> = scenario
            .locations
            .iter()
            .filter(|location| location.name_en.is_none())
            .map(|location| location.id.as_str())
            .collect();
        assert_eq!(
            without,
            [
                "carraig_aille",
                "tir_amalgaid",
                "corco_modruad",
                "tethbae_seat",
                "cairpre_seat",
                "loegaire_seat",
                "uliathain_seat",
                "ciarraige_seat",
            ]
        );
        // Both maps are by id over every place: the English-first one falls back to the local name, and the one the log prose uses
        // is the local name whatever the English name is.
        let local = super::place_names(&scenario);
        let first = super::english_place_names(&scenario);
        assert_eq!(local["birr"], "Biorra");
        assert_eq!(first["birr"], "Birr");
        assert_eq!(local["cairpre_seat"], first["cairpre_seat"]);
        assert_eq!(first.len(), scenario.locations.len());
        for location in &scenario.locations {
            assert_eq!(local[&location.id], location.name);
            assert_eq!(
                first[&location.id],
                location.name_en.as_ref().unwrap_or(&location.name).as_str()
            );
        }
    }

    #[test]
    fn trade_neighbours_are_symmetric_pairs_strictly_inside_four_kilometres() {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("Ireland scenario loads");
        let neighbours = super::trade_neighbours(&scenario);
        let count: usize = neighbours.values().map(Vec::len).sum();
        assert_eq!(count, 22, "eleven pairs");
        for (id, adjacent) in &neighbours {
            for other in adjacent {
                assert!(other.distance_m < 4_000);
                assert!(
                    neighbours[&other.id]
                        .iter()
                        .any(|back| back.id == *id && back.distance_m == other.distance_m)
                );
            }
        }
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
            events_fixture: None,
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

    #[test]
    fn damaged_history_loads_through_both_real_paths() {
        use sha2::{Digest, Sha256};
        let dir = crate::testing::Dir::new();
        let (game, prepared) = fixture(&dir.0);
        let expected_date = prepared.simulation.date().unwrap();
        let mut original = test_core_running(
            prepared,
            game.saves.clone(),
            Outputs {
                log: Box::new(|_, _| {}),
                publish: Box::new(|_| {}),
                fail: Box::new(|e| panic!("{e}")),
            },
        );
        original.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        let path = super::super::saves::quicksave_path(&game.saves);
        let bytes = fs::read(&path).unwrap();
        let length = usize::try_from(u64::from_le_bytes(
            bytes[bytes.len() - 16..bytes.len() - 8].try_into().unwrap(),
        ))
        .unwrap();
        let start = bytes.len() - 16 - length;
        let original_index = super::super::history::Index::decode(&bytes[start..]).unwrap();
        let mut variants = vec![bytes[..start].to_vec(), bytes[..bytes.len() - 1].to_vec()];
        let mut damaged = bytes.clone();
        damaged[start + 20] ^= 1;
        variants.push(damaged);
        let mut unknown = bytes.clone();
        unknown[start + 8] = 2;
        let hash_at = bytes.len() - 16 - 32;
        let hash = Sha256::digest(&unknown[start..hash_at]);
        unknown[hash_at..hash_at + 32].copy_from_slice(&hash);
        variants.push(unknown);
        let mut garbage = bytes[..start].to_vec();
        garbage.extend_from_slice(b"garbage after end");
        variants.push(garbage);
        for variant in variants {
            fs::write(&path, variant).unwrap();
            let messages = Arc::new(Mutex::new(Vec::<String>::new()));
            let recorded = Arc::clone(&messages);
            let mut loaded = prepare(
                &Request {
                    game: &game,
                    vms: &[],
                    load: Some(&path),
                    seed: None,
                },
                || Ok(42),
                &mut |_, text| recorded.lock().unwrap().push(text.clone()),
            )
            .unwrap();
            assert_eq!(loaded.simulation.date().unwrap(), expected_date);
            let index =
                super::super::history::Index::decode(&loaded.history.trailer().unwrap()).unwrap();
            assert_ne!(index.campaign, original_index.campaign);
            assert!(index.years.is_empty());
            assert_eq!(index.origin, (expected_date.year, expected_date.month));
            assert!(
                messages
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|line| !line.to_lowercase().contains("history"))
            );
            let (_, fresh) = fixture(&dir.0);
            let recorded = Arc::clone(&messages);
            let mut playing = test_core_running(
                fresh,
                game.saves.clone(),
                Outputs {
                    log: Box::new(move |_, text| recorded.lock().unwrap().push(text.clone())),
                    publish: Box::new(|_| {}),
                    fail: Box::new(|e| panic!("{e}")),
                },
            );
            playing.step_for_tests();
            playing.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
            assert_eq!(playing.date(), expected_date);
            playing.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
            let saved = fs::read(&path).unwrap();
            let length = usize::try_from(u64::from_le_bytes(
                saved[saved.len() - 16..saved.len() - 8].try_into().unwrap(),
            ))
            .unwrap();
            let index =
                super::super::history::Index::decode(&saved[saved.len() - 16 - length..]).unwrap();
            assert_ne!(index.campaign, original_index.campaign);
            assert!(index.years.is_empty());
            assert_eq!(index.origin, (expected_date.year, expected_date.month));
            assert!(
                messages
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|line| !line.to_lowercase().contains("history"))
            );
        }
    }

    #[test]
    fn changing_campaign_after_prefix_makes_a_gap_on_both_load_paths() {
        let dir = crate::testing::Dir::new();
        let (game, prepared) = fixture(&dir.0);
        let date = prepared.simulation.date().unwrap();
        let outputs = || Outputs {
            log: Box::new(|_, _| {}),
            publish: Box::new(|_| {}),
            fail: Box::new(|e| panic!("{e}")),
        };
        let mut source = test_core_running(prepared, game.saves.clone(), outputs());
        source.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        let path = super::super::saves::quicksave_path(&game.saves);
        let bytes = fs::read(&path).unwrap();
        let len = usize::try_from(u64::from_le_bytes(
            bytes[bytes.len() - 16..bytes.len() - 8].try_into().unwrap(),
        ))
        .unwrap();
        let start = bytes.len() - 16 - len;
        let mut index = super::super::history::Index::decode(&bytes[start..]).unwrap();
        let old_campaign = index.campaign;
        index.campaign = old_campaign ^ 1;
        let mut replaced = bytes[..start].to_vec();
        replaced.extend(index.encode().unwrap());
        let arrange_change = || {
            fs::write(&path, &bytes).unwrap();
            let path = path.clone();
            let replaced = replaced.clone();
            super::super::history::BEFORE_HANDOFF_FINISH.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || fs::write(path, replaced).unwrap()));
            });
        };
        arrange_change();
        let mut loaded = prepare(
            &Request {
                game: &game,
                vms: &[],
                load: Some(&path),
                seed: None,
            },
            || Ok(42),
            &mut |_, _| {},
        )
        .unwrap();
        let gap = super::super::history::Index::decode(&loaded.history.trailer().unwrap()).unwrap();
        assert_ne!(gap.campaign, old_campaign);
        assert_ne!(gap.campaign, index.campaign);
        assert!(gap.years.is_empty());
        assert_eq!(gap.origin, (date.year, date.month));
        let (_, fresh) = fixture(&dir.0);
        let mut playing = test_core_running(fresh, game.saves.clone(), outputs());
        arrange_change();
        playing.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
        playing.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        let saved = fs::read(&path).unwrap();
        let len = usize::try_from(u64::from_le_bytes(
            saved[saved.len() - 16..saved.len() - 8].try_into().unwrap(),
        ))
        .unwrap();
        let gap = super::super::history::Index::decode(&saved[saved.len() - 16 - len..]).unwrap();
        assert_ne!(gap.campaign, old_campaign);
        assert_ne!(gap.campaign, index.campaign);
        assert!(gap.years.is_empty());
        assert_eq!(gap.origin, (date.year, date.month));
    }

    #[test]
    fn damaged_core_with_intact_history_is_refused_and_keeps_running_lock() {
        let dir = crate::testing::Dir::new();
        let (game, prepared) = fixture(&dir.0);
        let date = prepared.simulation.date().unwrap();
        let mut playing = test_core_running(
            prepared,
            game.saves.clone(),
            Outputs {
                log: Box::new(|_, _| {}),
                publish: Box::new(|_| {}),
                fail: Box::new(|e| panic!("{e}")),
            },
        );
        playing.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        let path = super::super::saves::quicksave_path(&game.saves);
        let bytes = fs::read(&path).unwrap();
        let len = usize::try_from(u64::from_le_bytes(
            bytes[bytes.len() - 16..bytes.len() - 8].try_into().unwrap(),
        ))
        .unwrap();
        let index = super::super::history::Index::decode(&bytes[bytes.len() - 16 - len..]).unwrap();
        let lock = fs::File::open(
            game.saves
                .join("history/locks")
                .join(format!("{:016x}.lock", index.campaign)),
        )
        .unwrap();
        for tag in [b"STAT", b"ARCH", b"LOGS", b"END\0"] {
            let mut corrupt = bytes.clone();
            let at = corrupt
                .windows(4)
                .position(|window| window == tag)
                .expect("section tag");
            corrupt[at + if tag == b"END\0" { 4 } else { 12 }] ^= 1;
            fs::write(&path, corrupt).unwrap();
            assert!(
                prepare(
                    &Request {
                        game: &game,
                        vms: &[],
                        load: Some(&path),
                        seed: None
                    },
                    || Ok(42),
                    &mut |_, _| {}
                )
                .is_err()
            );
            playing.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
            assert_eq!(playing.date(), date);
            assert!(
                lock.try_lock().is_err(),
                "running campaign must remain locked"
            );
        }
        playing.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        let saved = fs::read(&path).unwrap();
        let len = usize::try_from(u64::from_le_bytes(
            saved[saved.len() - 16..saved.len() - 8].try_into().unwrap(),
        ))
        .unwrap();
        let after = super::super::history::Index::decode(&saved[saved.len() - 16 - len..]).unwrap();
        assert_eq!(after.campaign, index.campaign);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the fixture matrix keeps both load paths and recovery together"
    )]
    fn unavailable_lock_stores_detach_both_load_paths_with_or_without_trailer() {
        use std::os::unix::fs::PermissionsExt;
        fn watched(f: impl FnOnce()) {
            let (tx, rx) = std::sync::mpsc::channel();
            super::super::history::COLLECTION_DONE
                .with(|hook| *hook.borrow_mut() = Some(Box::new(move || tx.send(()).unwrap())));
            f();
            rx.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        for case in 0..3 {
            for intact in [false, true] {
                let dir = crate::testing::Dir::new();
                let (game, prepared) = fixture(&dir.0);
                let date = prepared.simulation.date().unwrap();
                let messages = Arc::new(Mutex::new(Vec::<String>::new()));
                let outputs = || Outputs {
                    log: {
                        let messages = Arc::clone(&messages);
                        Box::new(move |_, text| messages.lock().unwrap().push(text.clone()))
                    },
                    publish: Box::new(|_| {}),
                    fail: Box::new(|e| panic!("{e}")),
                };
                let mut source = None;
                watched(|| {
                    source = Some(test_core_running(prepared, game.saves.clone(), outputs()));
                });
                let mut source = source.unwrap();
                watched(|| {
                    source.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
                });
                drop(source);
                let path = super::super::saves::quicksave_path(&game.saves);
                let mut bytes = fs::read(&path).unwrap();
                let len = usize::try_from(u64::from_le_bytes(
                    bytes[bytes.len() - 16..bytes.len() - 8].try_into().unwrap(),
                ))
                .unwrap();
                if !intact {
                    bytes.truncate(bytes.len() - 16 - len);
                }
                fs::write(&path, &bytes).unwrap();
                let root = game.saves.join("history");
                fs::remove_dir_all(&root).unwrap();
                let locks = root.join("locks");
                match case {
                    0 => fs::write(&root, b"root").unwrap(),
                    1 => {
                        fs::create_dir(&root).unwrap();
                        fs::write(&locks, b"locks").unwrap();
                    }
                    _ => {
                        fs::create_dir_all(&locks).unwrap();
                        fs::create_dir(root.join("000000000000002a")).unwrap();
                        fs::write(root.join("000000000000002a/orphan.ckhist"), b"keep").unwrap();
                        fs::set_permissions(&locks, fs::Permissions::from_mode(0o555)).unwrap();
                    }
                }
                let mut loaded = prepare(
                    &Request {
                        game: &game,
                        vms: &[],
                        load: Some(&path),
                        seed: None,
                    },
                    || Ok(42),
                    &mut |_, text| messages.lock().unwrap().push(text.clone()),
                )
                .unwrap();
                assert_eq!(loaded.simulation.date().unwrap(), date);
                let index =
                    super::super::history::Index::decode(&loaded.history.trailer().unwrap())
                        .unwrap();
                assert!(index.years.is_empty());
                assert_eq!(index.origin, (date.year, date.month));
                assert!(
                    messages
                        .lock()
                        .unwrap()
                        .iter()
                        .all(|line| !line.to_lowercase().contains("history"))
                );
                super::super::history::COLLECTION_SCHEDULED.with(|count| count.set(0));
                loaded.history.collect_async();
                super::super::history::COLLECTION_SCHEDULED
                    .with(|count| assert_eq!(count.get(), 0));
                loaded.history.record(super::super::history::MonthRecord {
                    year: date.year,
                    month: 12,
                    places: vec![],
                });
                let index =
                    super::super::history::Index::decode(&loaded.history.trailer().unwrap())
                        .unwrap();
                assert!(index.years.is_empty());
                let (_, mut fresh) = fixture(&dir.0);
                let new_index =
                    super::super::history::Index::decode(&fresh.history.trailer().unwrap())
                        .unwrap();
                assert!(new_index.years.is_empty());
                assert_eq!(new_index.origin, (date.year, date.month));
                super::super::history::COLLECTION_SCHEDULED.with(|count| count.set(0));
                let mut playing = test_core_running(fresh, game.saves.clone(), outputs());
                super::super::history::COLLECTION_SCHEDULED
                    .with(|count| assert_eq!(count.get(), 0));
                playing.step_for_tests();
                playing.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
                assert_eq!(playing.date(), date);
                playing.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
                let saved = fs::read(&path).unwrap();
                let len = usize::try_from(u64::from_le_bytes(
                    saved[saved.len() - 16..saved.len() - 8].try_into().unwrap(),
                ))
                .unwrap();
                let index =
                    super::super::history::Index::decode(&saved[saved.len() - 16 - len..]).unwrap();
                assert!(index.years.is_empty());
                assert_eq!(index.origin, (date.year, date.month));
                match case {
                    0 => {
                        assert_eq!(fs::read(&root).unwrap(), b"root");
                        fs::remove_file(&root).unwrap();
                    }
                    1 => {
                        assert_eq!(fs::read(&locks).unwrap(), b"locks");
                        fs::remove_file(&locks).unwrap();
                    }
                    _ => {
                        assert_eq!(fs::read_dir(&locks).unwrap().count(), 0);
                        assert_eq!(
                            fs::read(root.join("000000000000002a/orphan.ckhist")).unwrap(),
                            b"keep"
                        );
                        fs::set_permissions(&locks, fs::Permissions::from_mode(0o755)).unwrap();
                    }
                }
                drop(loaded);
                drop(playing);
                let recovered = prepare(
                    &Request {
                        game: &game,
                        vms: &[],
                        load: Some(&path),
                        seed: None,
                    },
                    || Ok(42),
                    &mut |_, _| {},
                )
                .unwrap();
                let lock =
                    fs::File::open(locks.join(format!("{:016x}.lock", index.campaign))).unwrap();
                assert!(lock.try_lock().is_err());
                drop(recovered);
            }
        }
    }

    #[test]
    fn unavailable_lock_store_loads_silently_and_keeps_its_bytes() {
        let dir = crate::testing::Dir::new();
        let (game, prepared) = fixture(&dir.0);
        let date = prepared.simulation.date().unwrap();
        let outputs = || Outputs {
            log: Box::new(|_, _| {}),
            publish: Box::new(|_| {}),
            fail: Box::new(|e| panic!("{e}")),
        };
        let mut source = test_core_running(prepared, game.saves.clone(), outputs());
        source.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        drop(source);
        let root = game.saves.join("history");
        fs::remove_dir_all(&root).unwrap();
        fs::write(&root, b"unavailable lock store").unwrap();
        let path = super::super::saves::quicksave_path(&game.saves);
        let loaded = prepare(
            &Request {
                game: &game,
                vms: &[],
                load: Some(&path),
                seed: None,
            },
            || Ok(42),
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(loaded.simulation.date().unwrap(), date);
        let (_, fresh) = fixture(&dir.0);
        let mut playing = test_core_running(fresh, game.saves.clone(), outputs());
        playing.step_for_tests();
        playing.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
        assert_eq!(playing.date(), date);
        playing.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        let bytes = fs::read(path).unwrap();
        let len = usize::try_from(u64::from_le_bytes(
            bytes[bytes.len() - 16..bytes.len() - 8].try_into().unwrap(),
        ))
        .unwrap();
        let index = super::super::history::Index::decode(&bytes[bytes.len() - 16 - len..]).unwrap();
        assert!(index.years.is_empty());
        assert_eq!(fs::read(root).unwrap(), b"unavailable lock store");
    }

    #[test]
    fn deleting_and_overwriting_saves_collects_other_campaign_but_not_own() {
        use crate::game::core::{SaveCall, SaveReply};
        use std::time::Duration;
        fn watched(f: impl FnOnce()) {
            let (tx, rx) = std::sync::mpsc::channel();
            super::super::history::COLLECTION_DONE
                .with(|hook| *hook.borrow_mut() = Some(Box::new(move || tx.send(()).unwrap())));
            f();
            rx.recv_timeout(Duration::from_secs(10))
                .expect("collection completed");
        }
        fn call(core: &mut Core<GameState, GameCommand>, action: SaveCall) {
            let (tx, rx) = std::sync::mpsc::channel();
            core.dispatch(Message::Save(action, tx), Duration::ZERO);
            assert!(matches!(rx.recv().unwrap(), SaveReply::Finished(Ok(()))));
        }
        let dir = crate::testing::Dir::new();
        let (game, prepared) = fixture(&dir.0);
        let outputs = || Outputs {
            log: Box::new(|_, _| {}),
            publish: Box::new(|_| {}),
            fail: Box::new(|e| panic!("{e}")),
        };
        let mut core = None;
        watched(|| core = Some(test_core_running(prepared, game.saves.clone(), outputs())));
        let mut core = core.unwrap();
        watched(|| {
            core.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        });
        let quick = super::super::saves::quicksave_path(&game.saves);
        let bytes = fs::read(&quick).unwrap();
        let len = usize::try_from(u64::from_le_bytes(
            bytes[bytes.len() - 16..bytes.len() - 8].try_into().unwrap(),
        ))
        .unwrap();
        let start = bytes.len() - 16 - len;
        let own = super::super::history::Index::decode(&bytes[start..])
            .unwrap()
            .campaign;
        let other = own ^ 1;
        drop(super::super::history::Recorder::new(
            &game.saves,
            other,
            (768, 10),
        ));
        let folder = game.saves.join("history").join(format!("{other:016x}"));
        fs::create_dir_all(&folder).unwrap();
        let names = [
            format!("0768-{}.ckhist", "01".repeat(32)),
            format!("0769-{}.ckhist", "02".repeat(32)),
        ];
        for name in &names {
            fs::write(folder.join(name), b"year").unwrap();
        }
        let mut index = super::super::history::Index::decode(&bytes[start..]).unwrap();
        index.campaign = other;
        index.years = vec![(768, [1; 32]), (769, [2; 32])];
        let mut a = bytes[..start].to_vec();
        a.extend(index.encode().unwrap());
        fs::write(game.saves.join("a.cksave"), a).unwrap();
        index.years.pop();
        let mut b = bytes[..start].to_vec();
        b.extend(index.encode().unwrap());
        fs::write(game.saves.join("b.cksave"), b).unwrap();
        watched(|| call(&mut core, SaveCall::Delete("a.cksave".into())));
        assert!(folder.join(&names[0]).exists());
        assert!(!folder.join(&names[1]).exists());
        watched(|| call(&mut core, SaveCall::Over("b.cksave".into())));
        assert!(!folder.exists());
        let own_folder = game.saves.join("history").join(format!("{own:016x}"));
        fs::create_dir_all(&own_folder).unwrap();
        fs::write(own_folder.join(&names[0]), b"unsaved").unwrap();
        watched(|| call(&mut core, SaveCall::Over("b.cksave".into())));
        assert!(own_folder.join(&names[0]).exists());
        watched(|| call(&mut core, SaveCall::Delete("b.cksave".into())));
        assert!(own_folder.join(&names[0]).exists());
        let quick_name = quick.file_name().unwrap().to_str().unwrap().to_owned();
        watched(|| call(&mut core, SaveCall::Delete(quick_name)));
        assert!(own_folder.join(&names[0]).exists());
        drop(core);
        let (_, next) = fixture(&dir.0);
        let mut next_core = None;
        watched(|| next_core = Some(test_core_running(next, game.saves.clone(), outputs())));
        assert!(!own_folder.exists());
        drop(next_core);
    }

    #[test]
    fn long_history_survives_start_quickload_and_named_save() {
        use crate::game::core::{SaveCall, SaveReply};
        use sha2::{Digest, Sha256};
        let dir = crate::testing::Dir::new();
        let (game, prepared) = fixture(&dir.0);
        let outputs = || Outputs {
            log: Box::new(|_, _| {}),
            publish: Box::new(|_| {}),
            fail: Box::new(|e| panic!("{e}")),
        };
        let mut source = test_core_running(prepared, game.saves.clone(), outputs());
        source.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        let path = super::super::saves::quicksave_path(&game.saves);
        let mut bytes = fs::read(&path).unwrap();
        let len = usize::try_from(u64::from_le_bytes(
            bytes[bytes.len() - 16..bytes.len() - 8].try_into().unwrap(),
        ))
        .unwrap();
        let start = bytes.len() - 16 - len;
        let mut index = super::super::history::Index::decode(&bytes[start..]).unwrap();
        let folder = game
            .saves
            .join("history")
            .join(format!("{:016x}", index.campaign));
        fs::create_dir_all(&folder).unwrap();
        for year in 1..=2000_u16 {
            let month = super::super::history::MonthRecord {
                year,
                month: 1,
                places: vec![],
            };
            let content = super::super::history::encode_year(&[month]).unwrap();
            let hash: [u8; 32] = Sha256::digest(&content).into();
            fs::write(
                folder.join(format!(
                    "{year:04}-{}.ckhist",
                    super::super::history::hash_hex(&hash)
                )),
                content,
            )
            .unwrap();
            index.years.push((year, hash));
        }
        bytes.truncate(start);
        bytes.extend_from_slice(&index.encode().unwrap());
        assert!(bytes.len() - start > 65_536);
        fs::write(&path, &bytes).unwrap();
        let loaded = prepare(
            &Request {
                game: &game,
                vms: &[],
                load: Some(&path),
                seed: None,
            },
            || Ok(42),
            &mut |_, _| {},
        )
        .unwrap();
        let mut playing = test_core_running(loaded, game.saves.clone(), outputs());
        let (tx, rx) = std::sync::mpsc::channel();
        playing.dispatch(
            Message::Save(SaveCall::New("Long".into()), tx),
            Duration::ZERO,
        );
        assert!(matches!(rx.recv().unwrap(), SaveReply::Finished(Ok(()))));
        source.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
        source.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        let references = super::super::history::save_references(&game.saves).unwrap();
        assert_eq!(references.len(), 2);
        assert!(references.iter().all(|item| item.years == index.years));
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
    #[expect(
        clippy::too_many_lines,
        reason = "Each invalid fuel field is checked through both real load paths and their storage readers."
    )]
    // Long run: selected by app.long.
    fn app_long_both_load_paths_refuse_invalid_fuel_and_older_builds_without_installing_storage() {
        let dir = crate::testing::Dir::new();
        let game = GameConfig {
            scenario: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/scenarios/ireland-768"),
            saves: dir.0.join("saves"),
            events_fixture: None,
        };
        let new_game = || {
            prepare(
                &Request {
                    game: &game,
                    vms: &[],
                    load: None,
                    seed: Some(42),
                },
                || Ok(42),
                &mut |_, _| {},
            )
            .unwrap()
        };
        let source = new_game();
        let path = super::super::saves::quicksave_path(&game.saves);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for (case, expected) in [
            (0, "hearth"),
            (1, "burned"),
            (2, "death_carry"),
            (3, "save is from build"),
        ] {
            let mut save = source.simulation.save(source.identity.meta(42));
            let pop = save
                .state
                .economy
                .places
                .iter_mut()
                .flat_map(|place| &mut place.pops)
                .next()
                .unwrap();
            match case {
                0 => pop.consumption.hearth_milli = 1000,
                1 => {
                    pop.consumption.fuel_need_month = 1;
                    pop.consumption.fuel_burned_month = 2;
                }
                2 => pop.consumption.fuel_death_carry = 12_000_000,
                _ => save.meta.build = format!("{}+0000000000000000", env!("CARGO_PKG_VERSION")),
            }
            let mut archive = fs::File::open(source.storage.path.join("ARCH")).unwrap();
            let mut logs = fs::File::open(source.storage.path.join("LOGS")).unwrap();
            let lengths = (
                archive.metadata().unwrap().len(),
                logs.metadata().unwrap().len(),
            );
            let mut file = fs::File::create(&path).unwrap();
            save.write_with_sections(
                &mut file,
                &mut [
                    (*b"ARCH", lengths.0, &mut archive),
                    (*b"LOGS", lengths.1, &mut logs),
                ],
            )
            .unwrap();
            let result = prepare(
                &Request {
                    game: &game,
                    vms: &[],
                    load: Some(&path),
                    seed: None,
                },
                || Ok(42),
                &mut |_, _| {},
            );
            assert!(result.err().unwrap().contains(expected));
            let running = new_game();
            let handle = Arc::clone(running.storage.handle.as_ref().unwrap());
            let archive = Arc::clone(running.storage.cell.as_ref().unwrap());
            let logs = Arc::clone(running.storage.log_cell.as_ref().unwrap());
            let before_path = handle.lock().unwrap().clone();
            let before_archive = archive.lock().unwrap().clone().unwrap();
            let before_logs = logs.lock().unwrap().clone().unwrap();
            let records = Arc::new(Mutex::new(Vec::new()));
            let output = Arc::clone(&records);
            let mut core = test_core(
                running,
                game.saves.clone(),
                Outputs {
                    log: Box::new(move |_, line| output.lock().unwrap().push(line)),
                    publish: Box::new(|_| {}),
                    fail: Box::new(|e| panic!("{e}")),
                },
            );
            let before = core.digest();
            core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
            assert!(
                records
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|line| line.contains(expected))
            );
            assert_eq!(core.digest(), before);
            assert_eq!(*handle.lock().unwrap(), before_path);
            assert!(Arc::ptr_eq(
                archive.lock().unwrap().as_ref().unwrap(),
                &before_archive
            ));
            assert!(Arc::ptr_eq(
                logs.lock().unwrap().as_ref().unwrap(),
                &before_logs
            ));
        }
    }

    /// The first farm with two or more levels: the building each level-record case changes.
    fn target(save: &mut Save<GameState>) -> &mut ck_economy::BuildingState {
        save.state
            .economy
            .places
            .iter_mut()
            .flat_map(|place| &mut place.buildings)
            .find(|building| building.kind == "farm" && building.level_records.len() >= 2)
            .expect("the start has a farm of two levels")
    }

    /// A field level's ground changed, as a played game's field on forest or bog ground would have it.
    fn field_on(building: &mut ck_economy::BuildingState, ground: &str) {
        let at = building
            .level_records
            .iter()
            .position(|record| record.method == 0)
            .expect("the farm has a field level");
        building.level_records[at].ground = Some(ground.to_owned());
    }

    /// A farm whose every level was cut at the start: no levels, no records, no counts.
    fn cut_farm(building: &mut ck_economy::BuildingState) {
        building.levels = 0;
        building.level_records.clear();
        building.method_levels.fill(0);
    }

    /// A cut farm has no job places; clear both mirrors just as a switch would.
    fn clear_cut_employment(save: &mut Save<GameState>) {
        for place in &mut save.state.economy.places {
            for (index, building) in place.buildings.iter_mut().enumerate() {
                if building.kind == "farm" && building.levels == 0 {
                    building.employment.fill(ck_economy::Employment::default());
                    for pop in &mut place.pops {
                        pop.employment[index] = ck_economy::Employment::default();
                    }
                }
            }
        }
    }

    /// The sums of a building follow its records, as a played game's do.
    fn fix_sums(rules: &Rules, building: &mut ck_economy::BuildingState) {
        let definition = rules
            .buildings
            .iter()
            .find(|item| item.id == building.kind)
            .unwrap();
        (building.method_suitability, building.herd_grazing_sums) =
            ck_economy::derive_level_sums(rules, definition, &building.level_records).unwrap();
    }

    /// A running game's quickload of the save written last installs it, with no refusal logged.
    fn assert_quickload_installs(
        game: &GameConfig,
        new_game: &impl Fn() -> Prepared,
        label: &str,
        expected: &ck_economy::EconomyState,
    ) {
        let running = new_game();
        let records = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&records);
        let mut core = test_core(
            running,
            game.saves.clone(),
            Outputs {
                log: Box::new(move |_, line| output.lock().unwrap().push(line)),
                publish: Box::new(|_| {}),
                fail: Box::new(|e| panic!("{e}")),
            },
        );
        let before = core.digest();
        core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
        let lines = records.lock().unwrap();
        assert!(
            !lines.iter().any(|line| line.contains("refused")),
            "{label}: {lines:?}"
        );
        assert_ne!(core.digest(), before, "{label}: the save is installed");
        assert_eq!(
            &core.state().economy,
            expected,
            "{label}: exactly the saved economy"
        );
    }

    type LevelChange = fn(&mut ck_economy::BuildingState);

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Each invalid level record is checked through both real load paths and their storage readers."
    )]
    // Long run: selected by app.long.
    fn app_long_both_load_paths_refuse_invalid_level_records_and_accept_land_levels_on_forest_and_bog()
     {
        let dir = crate::testing::Dir::new();
        let game = GameConfig {
            scenario: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/scenarios/ireland-768"),
            saves: dir.0.join("saves"),
            events_fixture: None,
        };
        let rules = Rules::load(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
        )
        .unwrap();
        let new_game = || {
            prepare(
                &Request {
                    game: &game,
                    vms: &[],
                    load: None,
                    seed: Some(42),
                },
                || Ok(42),
                &mut |_, _| {},
            )
            .unwrap()
        };
        let source = new_game();
        let path = super::super::saves::quicksave_path(&game.saves);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // The first farm with two or more levels is the building each case changes.
        let write = |save: &Save<GameState>| {
            let mut archive = fs::File::open(source.storage.path.join("ARCH")).unwrap();
            let mut logs = fs::File::open(source.storage.path.join("LOGS")).unwrap();
            let lengths = (
                archive.metadata().unwrap().len(),
                logs.metadata().unwrap().len(),
            );
            let mut file = fs::File::create(&path).unwrap();
            save.write_with_sections(
                &mut file,
                &mut [
                    (*b"ARCH", lengths.0, &mut archive),
                    (*b"LOGS", lengths.1, &mut logs),
                ],
            )
            .unwrap();
        };
        let start_from = || {
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
        };
        // Each fault of rule 12 in turn: the method, the ground against the resource, the paydays, the numbers, the counts and the
        // carries (a farm's methods are the field, the sea, the wood focus and the turf cutting).
        let refusals: [(&str, LevelChange); 16] = [
            ("is not one of the building's", |b| {
                b.level_records[0].method = 99;
            }),
            ("does not fit", |b| b.level_records[0].ground = None),
            ("does not fit", |b| {
                // A field level on ground that is no land terrain.
                b.level_records[0].ground = Some("sea".to_owned());
            }),
            ("does not fit", |b| {
                // A wood level on bog ground.
                b.level_records[0].method = 2;
                b.level_records[0].ground = Some("bog".to_owned());
            }),
            ("does not fit", |b| {
                // A turf level on forest ground.
                b.level_records[0].method = 3;
                b.level_records[0].ground = Some("forest".to_owned());
            }),
            ("does not fit", |b| {
                // A sea level with ground.
                b.level_records[0].method = 1;
                b.level_records[0].ground = Some("pasture".to_owned());
            }),
            ("missing unpaid work account", |b| {
                let good = b.level_records[0]
                    .unpaid_work
                    .keys()
                    .next()
                    .unwrap()
                    .clone();
                b.level_records[0].unpaid_work.remove(&good);
            }),
            ("unexpected unpaid work account", |b| {
                b.level_records[0]
                    .unpaid_work
                    .insert("not_a_product".into(), ck_economy::bigint::Big::default());
            }),
            ("completed paydays", |b| b.level_records[0].paydays = 14),
            ("the number is used twice", |b| {
                b.level_records[0].number = b.level_records[1].number;
            }),
            ("the number is not below", |b| {
                b.level_records[0].number = b.levels;
            }),
            ("the level records count", |b| b.method_levels[0] += 1),
            ("the level records count", |b| {
                b.level_records.pop();
            }),
            ("the saved suitability sums are not those", |b| {
                // The values in order are the derived ones, but one has moved from a method's row to the next one's.
                let moved = b.method_suitability[0].pop().unwrap();
                b.method_suitability[1].insert(0, moved);
            }),
            ("is invalid for denominator", |b| {
                b.production_carries[0][0] = u128::MAX;
            }),
            ("method mixed_farming good", |b| {
                // A farm whose every level was cut, with one carry above nothing.
                cut_farm(b);
                b.method_suitability
                    .iter_mut()
                    .flatten()
                    .for_each(|v| *v = 0);
                b.herd_grazing_sums.fill(0);
                b.production_carries[0][0] = 1;
            }),
        ];
        for (expected, change) in refusals {
            let mut save = source.simulation.save(source.identity.meta(42));
            change(target(&mut save));
            clear_cut_employment(&mut save);
            write(&save);
            let result = start_from();
            let message = result.err().expect("a start from the save refuses it");
            assert!(message.contains(expected), "{message}");
            assert!(message.contains("building farm"), "{message}");
            let running = new_game();
            let handle = Arc::clone(running.storage.handle.as_ref().unwrap());
            let archive = Arc::clone(running.storage.cell.as_ref().unwrap());
            let logs = Arc::clone(running.storage.log_cell.as_ref().unwrap());
            let before_path = handle.lock().unwrap().clone();
            let before_archive = archive.lock().unwrap().clone().unwrap();
            let before_logs = logs.lock().unwrap().clone().unwrap();
            let records = Arc::new(Mutex::new(Vec::new()));
            let output = Arc::clone(&records);
            let mut core = test_core(
                running,
                game.saves.clone(),
                Outputs {
                    log: Box::new(move |_, line| output.lock().unwrap().push(line)),
                    publish: Box::new(|_| {}),
                    fail: Box::new(|e| panic!("{e}")),
                },
            );
            let before = core.digest();
            core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
            assert!(
                records
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|line| line.contains(expected)),
                "{expected}"
            );
            assert_eq!(core.digest(), before, "the running game is as it was");
            assert_eq!(*handle.lock().unwrap(), before_path);
            assert!(Arc::ptr_eq(
                archive.lock().unwrap().as_ref().unwrap(),
                &before_archive
            ));
            assert!(Arc::ptr_eq(
                logs.lock().unwrap().as_ref().unwrap(),
                &before_logs
            ));
        }
        // Saves that are good records, loaded by both ways: a field level on forest ground and one on bog ground (when the sums follow
        // them as a played game's do), a farm whose every level was cut with its carries written as nothing and with them left
        // out, and a carry wider than 64 bits at a place whose denominator holds it.
        let definition = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        let goods = rules.goods.len();
        let good: Vec<(&str, LevelChange)> = vec![
            ("forest ground", |b| field_on(b, "forest")),
            ("bog ground", |b| field_on(b, "bog")),
            ("a farm with every level cut, carries as nothing", |b| {
                cut_farm(b);
                b.production_carries.iter_mut().for_each(|row| row.fill(0));
            }),
            ("a farm with every level cut, carries left out", |b| {
                cut_farm(b);
                b.production_carries.clear();
            }),
        ];
        for (label, change) in good {
            let mut save = source.simulation.save(source.identity.meta(42));
            let farm = target(&mut save);
            change(farm);
            fix_sums(&rules, farm);
            clear_cut_employment(&mut save);
            write(&save);
            let loaded = start_from().unwrap_or_else(|e| panic!("{label}: {e}"));
            assert_eq!(
                loaded.simulation.state().economy,
                save.state.economy,
                "{label}"
            );
            assert_quickload_installs(&game, &new_game, label, &save.state.economy);
        }
        // A large account is legal on both paths and stays legal after the next crop payday.
        let mut large = source.simulation.save(source.identity.meta(42));
        let farm = target(&mut large);
        let record = farm
            .level_records
            .iter_mut()
            .find(|record| record.method == 0)
            .unwrap();
        let level_number = record.number;
        record.unpaid_work.insert(
            "oats".into(),
            ck_economy::bigint::Big::new(u128::from(u32::MAX)),
        );
        write(&large);
        let loaded = start_from().expect("a large unpaid account loads");
        assert_eq!(loaded.simulation.state().economy, large.state.economy);
        assert_quickload_installs(
            &game,
            &new_game,
            "a large unpaid account",
            &large.state.economy,
        );
        ck_economy::run_monthly_production(
            &rules,
            &mut large.state.economy,
            Date {
                year: 768,
                month: 10,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let updated = target(&mut large)
            .level_records
            .iter()
            .find(|r| r.number == level_number)
            .unwrap()
            .unpaid_work["oats"]
            .to_u128()
            .unwrap();
        assert!(
            updated > u128::from(u32::MAX),
            "payday crossed the 32-bit boundary"
        );
        write(&large);
        let loaded = start_from().expect("the updated account loads again");
        assert_eq!(loaded.simulation.state().economy, large.state.economy);
        assert_quickload_installs(
            &game,
            &new_game,
            "the updated unpaid account",
            &large.state.economy,
        );
        // The same cut farm with one carry above nothing is refused, naming the building, the method and the good.
        let mut save = source.simulation.save(source.identity.meta(42));
        let farm = target(&mut save);
        cut_farm(farm);
        farm.production_carries = vec![vec![0; goods]; farm.method_levels.len()];
        farm.production_carries[0][0] = 1;
        fix_sums(&rules, farm);
        clear_cut_employment(&mut save);
        write(&save);
        let message = start_from().err().expect("the carry is refused");
        assert!(
            message.contains("building farm method mixed_farming good"),
            "{message}"
        );
        // A carry past 2^64, at the place whose denominator (positions, twelve thousand million, and the quality's own factor)
        // holds it: the levels are enough in number to make it.
        let mut save = source.simulation.save(source.identity.meta(42));
        let place = save
            .state
            .economy
            .places
            .iter()
            .position(|place| {
                place
                    .buildings
                    .iter()
                    .any(|b| b.kind == "farm" && b.level_records.len() >= 2)
            })
            .unwrap();
        let factor = {
            let wiring = source.view_wiring();
            let land = &wiring.land_by_place[&save.state.economy.places[place].id];
            let quality = u128::from(ck_economy::mean_fertility(land).unwrap());
            let reference = u128::from(rules.fertility_reference_pm);
            let (mut a, mut b) = (quality, reference);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            reference / a.max(1)
        };
        assert!(
            factor >= 100,
            "this place's quality gives a factor of {factor}"
        );
        let wanted = 19_800_000_000_000_000_001_u128;
        let per_level = u128::from(definition.methods[0].jobs[0].places) * 12_000_000_000 * factor;
        let levels = u32::try_from(wanted / per_level + 1000).unwrap();
        let farm = save.state.economy.places[place]
            .buildings
            .iter_mut()
            .find(|b| b.kind == "farm")
            .unwrap();
        farm.levels = levels;
        farm.units = levels * 5;
        farm.method_levels = vec![0; farm.method_levels.len()];
        farm.method_levels[0] = levels;
        let unpaid_work = definition.methods[0]
            .annual_output
            .keys()
            .filter(|good| !ck_economy::herd_supplies(&rules, definition, good))
            .map(|good| (good.clone(), ck_economy::bigint::Big::default()))
            .collect::<std::collections::BTreeMap<_, _>>();
        farm.level_records = (0..levels)
            .map(|number| ck_economy::LevelRecord {
                number,
                method: 0,
                ground: Some("pasture".to_owned()),
                paydays: ck_economy::MATURE_PAYDAYS,
                unpaid_work: unpaid_work.clone(),
                last_switch: None,
            })
            .collect();
        farm.production_carries[0][0] = 19_800_000_000_000_000_000;
        fix_sums(&rules, farm);
        write(&save);
        let loaded = start_from().expect("a wide carry below its denominator loads");
        assert_eq!(loaded.simulation.state().economy, save.state.economy);
        assert_quickload_installs(&game, &new_game, "a carry of 19.8e18", &save.state.economy);
    }

    #[test]
    fn real_wind_readback_uses_the_current_day() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let scenario = ck_map::load_scenario(root.join("scenarios/ireland-768")).unwrap();
        let weather = WeatherData::load(&root, &scenario).unwrap();
        for (year, month, day, hour, expected) in [
            (768, 10, 9, 0, (2, "SW")),
            (769, 1, 9, 13, (4, "SW")),
            (769, 1, 10, 6, (4, "S")),
            (769, 1, 10, 12, (4, "S")),
            (769, 1, 10, 13, (4, "S")),
        ] {
            let values = weather.hourly(
                &super::super::lows::Lows::default(),
                Date {
                    year,
                    month,
                    day,
                    hour,
                },
            );
            assert_eq!(
                values["birr"].1.as_ref().map(|(k, d)| (*k, d.as_str())),
                Some(expected)
            );
        }
        let values = weather.hourly(
            &super::super::lows::Lows::default(),
            Date {
                year: 1,
                month: 1,
                day: 1,
                hour: 0,
            },
        );
        assert!(values["birr"].1.is_some());
    }

    #[test]
    fn birr_wind_at_noon_survives_real_save_and_load() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let scenario = ck_map::load_scenario(root.join("scenarios/ireland-768")).unwrap();
        let actual = WeatherData::load(&root, &scenario).unwrap();
        let birr = scenario
            .locations
            .iter()
            .position(|p| p.id == "birr")
            .unwrap();
        let monthly = actual.winds[birr].unwrap();
        for (year, month, day, hour, expected) in [
            (768, 10, 9, 0, (2, "SW")),
            (769, 1, 9, 13, (4, "SW")),
            (769, 1, 10, 6, (4, "SW")),
            (769, 1, 10, 12, (4, "SW")),
            (769, 1, 10, 13, (4, "S")),
        ] {
            let dir = crate::testing::Dir::new();
            let (game, mut prepared) = fixture(&dir.0);
            let rules = ck_sim::weather::WeatherRules::parse(include_str!(
                "../../../../assets/game/weather.toml"
            ))
            .unwrap();
            let place = ck_sim::weather::WeatherPlace {
                latitude_e4: 531_000,
                longitude_e4: -79_000,
                mean: 94,
                half_range: 44,
                nearness: 170,
                phase: 650,
            };
            let expected_year = place.expected_year(&rules, 768).unwrap();
            let count = prepared.scenario.locations.len();
            let places = prepared
                .scenario
                .locations
                .iter()
                .map(|location| (location.id.clone(), place, expected_year))
                .collect();
            prepared.weather = Some(Arc::new(WeatherData {
                steering_cache: Arc::default(),
                backdrop: None,
                rules,
                places,
                winds: vec![Some(monthly); count],
                cells: vec![],
            }));
            let date = Date {
                year,
                month,
                day,
                hour,
            };
            prepared.simulation = Simulation::new(
                GameTime::from_date(date).unwrap(),
                42,
                prepared.simulation.state().clone(),
                Scheduler::new(),
                apply,
            );
            let mut core = test_core(
                prepared,
                game.saves,
                Outputs {
                    log: Box::new(|_, _| {}),
                    publish: Box::new(|_| {}),
                    fail: Box::new(|error| panic!("{error}")),
                },
            );
            let wind = |core: &Core<GameState, GameCommand>| {
                core.snapshot().economy.unwrap().places[0]
                    .wind
                    .clone()
                    .unwrap()
            };
            assert_eq!(wind(&core), (expected.0, expected.1.to_owned()));
            core.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
            core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
            assert_eq!(wind(&core), (expected.0, expected.1.to_owned()));
        }
    }

    #[test]
    fn hourly_weather_reads_the_current_hour() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let scenario = ck_map::load_scenario(root.join("scenarios/ireland-768")).unwrap();
        let weather = WeatherData::load(&root, &scenario).unwrap();
        let date = Date {
            year: 768,
            month: 10,
            day: 9,
            hour: 6,
        };
        let next = Date { hour: 7, ..date };
        let lows = super::super::lows::Lows::default();
        let first = weather.hourly(&lows, date);
        let second = weather.hourly(&lows, next);
        let birr = weather.places.iter().position(|p| p.0 == "birr").unwrap();
        let day = ck_sim::weather::julian_day(768, 10, 9) - ck_sim::weather::julian_day(1, 1, 1);
        assert_eq!(
            first["birr"].0,
            weather.places[birr].1.hour(&weather.rules, day, 6).unwrap()
        );
        assert_eq!(
            second["birr"].0,
            weather.places[birr].1.hour(&weather.rules, day, 7).unwrap()
        );
        assert_ne!(first["birr"].0, second["birr"].0);
    }

    #[test]
    fn hourly_temperature_keeps_the_latest_noon_month() {
        let rules = ck_sim::weather::WeatherRules::parse(include_str!(
            "../../../../assets/game/weather.toml"
        ))
        .unwrap();
        let coast = ck_sim::weather::WeatherPlace {
            latitude_e4: 519_000,
            longitude_e4: -103_000,
            mean: 109,
            half_range: 40,
            nearness: 1000,
            phase: 650,
        };
        let expected = coast.expected_year(&rules, 768).unwrap();
        let weather = WeatherData {
            steering_cache: Arc::default(),
            backdrop: None,
            rules,
            places: vec![("coast".to_owned(), coast, expected)],
            winds: vec![],
            cells: vec![],
        };
        for (hour, value) in [
            (0, (128, 9, 135)),
            (12, (128, 9, 135)),
            (13, (127, 10, 117)),
        ] {
            let mut view = view::EconomyView {
                places: vec![view::PlaceView::default()],
                ..Default::default()
            };
            weather.add_to_view(
                &mut view,
                Date {
                    year: 768,
                    month: 10,
                    day: 1,
                    hour,
                },
            );
            assert_eq!(view.places[0].temperature, Some(value));
            let date = Date {
                year: 768,
                month: 10,
                day: 1,
                hour,
            };
            let day =
                ck_sim::weather::julian_day(768, 10, 1) - ck_sim::weather::julian_day(1, 1, 1);
            assert_eq!(
                weather.hourly(&super::super::lows::Lows::default(), date)["coast"].0,
                coast.hour(&weather.rules, day, i64::from(hour)).unwrap()
            );
        }
    }

    #[test]
    fn weather_view_survives_save_load_at_the_noon_cutoff() {
        for (opening_hour, expected) in [
            (0, (128, 9, 135)),
            (12, (128, 9, 135)),
            (13, (127, 10, 117)),
        ] {
            let dir = crate::testing::Dir::new();
            let (game, mut prepared) = fixture(&dir.0);
            let rules = ck_sim::weather::WeatherRules::parse(include_str!(
                "../../../../assets/game/weather.toml"
            ))
            .unwrap();
            let place = ck_sim::weather::WeatherPlace {
                latitude_e4: 519_000,
                longitude_e4: -103_000,
                mean: 109,
                half_range: 40,
                nearness: 1000,
                phase: 650,
            };
            let year = place.expected_year(&rules, 768).unwrap();
            let places = prepared
                .scenario
                .locations
                .iter()
                .map(|location| (location.id.clone(), place, year))
                .collect();
            prepared.weather = Some(Arc::new(WeatherData {
                steering_cache: Arc::default(),
                backdrop: None,
                rules,
                places,
                winds: vec![Some([(100, 100, 101_000); 12]); prepared.scenario.locations.len()],
                cells: vec![],
            }));
            let date = Date {
                year: 768,
                month: 10,
                day: 1,
                hour: opening_hour,
            };
            prepared.simulation = Simulation::new(
                GameTime::from_date(date).unwrap(),
                42,
                prepared.simulation.state().clone(),
                Scheduler::new(),
                apply,
            );
            let mut core = test_core(
                prepared,
                game.saves,
                Outputs {
                    log: Box::new(|_, _| {}),
                    publish: Box::new(|_| {}),
                    fail: Box::new(|error| panic!("{error}")),
                },
            );
            let temp = |core: &Core<GameState, GameCommand>| {
                core.snapshot().economy.unwrap().places[0]
                    .temperature
                    .unwrap()
            };
            assert_eq!(temp(&core), expected);
            let wind = |core: &Core<GameState, GameCommand>| {
                core.snapshot().economy.unwrap().places[0].wind.clone()
            };
            assert_eq!(wind(&core), Some((3, "SW".to_owned())));
            core.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
            core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
            assert_eq!(temp(&core), expected);
            assert_eq!(wind(&core), Some((3, "SW".to_owned())));
            if opening_hour == 0 {
                core.dispatch(Message::Control(Control::TogglePause), Duration::ZERO);
                for hour in 1..=13 {
                    core.run_wake(Duration::from_secs(hour), &mut || None);
                    if hour >= 11 {
                        let expected = if hour <= 12 {
                            (128, 9, 135)
                        } else {
                            (127, 10, 117)
                        };
                        assert_eq!(temp(&core), expected);
                        core.dispatch(
                            Message::Control(Control::Quicksave),
                            Duration::from_secs(hour),
                        );
                        core.dispatch(
                            Message::Control(Control::Quickload),
                            Duration::from_secs(hour),
                        );
                        assert_eq!(temp(&core), expected);
                        core.dispatch(
                            Message::Control(Control::TogglePause),
                            Duration::from_secs(hour),
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn weather_first_view_and_later_load_keep_the_start_year() {
        let rules = ck_sim::weather::WeatherRules::parse(include_str!(
            "../../../../assets/game/weather.toml"
        ))
        .unwrap();
        let north = ck_sim::weather::WeatherPlace {
            latitude_e4: 700_000,
            longitude_e4: 250_000,
            mean: -20,
            half_range: 150,
            nearness: 600,
            phase: 650,
        };
        let expected = north.expected_year(&rules, 1).unwrap();
        let weather = WeatherData {
            steering_cache: Arc::default(),
            backdrop: None,
            rules,
            places: vec![("north".into(), north, expected)],
            winds: vec![],
            cells: vec![],
        };
        let mut view = view::EconomyView {
            places: vec![view::PlaceView::default()],
            ..Default::default()
        };
        weather.add_to_view(
            &mut view,
            Date {
                year: 1,
                month: 1,
                day: 1,
                hour: 0,
            },
        );
        assert_eq!(view.places[0].temperature, Some((-146, 12, -121)));

        let dir = crate::testing::Dir::new();
        let (game, mut prepared) = fixture(&dir.0);
        let rules = ck_sim::weather::WeatherRules::parse(include_str!(
            "../../../../assets/game/weather.toml"
        ))
        .unwrap();
        let coast = ck_sim::weather::WeatherPlace {
            latitude_e4: 519_000,
            longitude_e4: -103_000,
            mean: 109,
            half_range: 40,
            nearness: 1000,
            phase: 650,
        };
        let expected = coast.expected_year(&rules, 768).unwrap();
        let places = prepared
            .scenario
            .locations
            .iter()
            .map(|location| (location.id.clone(), coast, expected))
            .collect();
        prepared.weather = Some(Arc::new(WeatherData {
            steering_cache: Arc::default(),
            backdrop: None,
            rules,
            places,
            winds: vec![],
            cells: vec![],
        }));
        let date = Date {
            year: 769,
            month: 10,
            day: 1,
            hour: 13,
        };
        prepared.simulation = Simulation::new(
            GameTime::from_date(date).unwrap(),
            42,
            prepared.simulation.state().clone(),
            Scheduler::new(),
            apply,
        );
        let mut core = test_core(
            prepared,
            game.saves,
            Outputs {
                log: Box::new(|_, _| {}),
                publish: Box::new(|_| {}),
                fail: Box::new(|error| panic!("{error}")),
            },
        );
        let before = core.snapshot().economy.unwrap().places[0]
            .temperature
            .unwrap();
        assert_eq!((before.1, before.2), (10, 117));
        core.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
        assert_eq!(
            core.snapshot().economy.unwrap().places[0].temperature,
            Some(before)
        );
    }

    #[test]
    fn ireland_weather_expected_year_matches_the_climate() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let scenario = ck_map::load_scenario(root.join("scenarios/ireland-768")).unwrap();
        let weather = WeatherData::load(&root, &scenario).unwrap();
        assert_eq!(weather.places.len(), scenario.locations.len());
        for (id, place, year) in weather.places {
            let warmest = *year.iter().max().unwrap();
            let coldest = *year.iter().min().unwrap();
            assert!(year[6] == warmest || year[7] == warmest, "{id}: {year:?}");
            assert!(year[0] == coldest || year[1] == coldest, "{id}: {year:?}");
            assert!(
                (year.iter().sum::<i64>() - place.mean * 12).abs() <= 36,
                "{id}: {year:?}"
            );
        }
    }

    #[test]
    fn adding_daily_weather_keeps_a_year_of_economy_identical() {
        let dir = crate::testing::Dir::new();
        let (_, prepared) = fixture(&dir.0);
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let rules = Arc::new(Rules::load(root.join("game/economy.toml")).unwrap());
        let weather_rules = ck_sim::weather::WeatherRules::parse(
            &fs::read_to_string(root.join("game/weather.toml")).unwrap(),
        )
        .unwrap();
        let place = ck_sim::weather::WeatherPlace {
            latitude_e4: 531_000,
            longitude_e4: -79_000,
            mean: 94,
            half_range: 44,
            nearness: 170,
            phase: 650,
        };
        let expected = place.expected_year(&weather_rules, 768).unwrap();
        let weather = Arc::new(WeatherData {
            steering_cache: Arc::default(),
            backdrop: None,
            rules: weather_rules,
            places: vec![("fixture".into(), place, expected)],
            winds: vec![],
            cells: vec![],
        });
        let scheduler = |data| {
            gameplay_scheduler_with_rules(
                None,
                Some(Arc::clone(&rules)),
                BTreeMap::new(),
                BTreeMap::new(),
                BTreeMap::new(),
                BTreeMap::new(),
                BTreeMap::new(),
                BTreeMap::new(),
                BTreeMap::new(),
                data,
                None,
                None,
            )
        };
        let mut baseline = Simulation::<GameState, GameCommand>::new(
            prepared.simulation.time(),
            42,
            prepared.simulation.state().clone(),
            scheduler(None),
            apply,
        );
        let mut with_weather = Simulation::<GameState, GameCommand>::new(
            prepared.simulation.time(),
            42,
            prepared.simulation.state().clone(),
            scheduler(Some(weather)),
            apply,
        );
        for _ in 0..(366 * 24) {
            assert!(baseline.step().unwrap().is_empty());
            assert!(with_weather.step().unwrap().is_empty());
        }
        assert_eq!(baseline.digest().unwrap(), with_weather.digest().unwrap());
        assert_eq!(baseline.state().economy, with_weather.state().economy);
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
            BTreeMap::new(),
            BTreeMap::new(),
            None,
            prepared.storage.handle.clone(),
            None,
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

    /// A new game of the shipped scenario, as the game starts it.
    fn shipped_start(dir: &Path, load: Option<&Path>) -> Prepared {
        let game = GameConfig {
            scenario: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/scenarios/ireland-768"),
            saves: dir.join("saves"),
            events_fixture: None,
        };
        prepare(
            &Request {
                game: &game,
                vms: &[],
                load,
                seed: if load.is_some() { None } else { Some(42) },
            },
            || Ok(42),
            &mut |_, _| {},
        )
        .unwrap()
    }

    /// CK-433 (11c.8 rule 25): a save written under other travel rules is refused by the same build, though nothing but `game/travel.toml`
    /// differs: the rules hash it carries is the hash of the rules it was played under, and the travel rules are in it.
    #[test]
    fn a_same_build_save_under_other_travel_rules_is_refused_at_start() {
        let dir = crate::testing::Dir::new();
        let source = shipped_start(&dir.0, None);
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        // The rules files the hash covers, with the travel rules edited.
        let other = dir.0.join("other-assets");
        fs::create_dir_all(other.join("map/ireland")).unwrap();
        copy_tree(&assets.join("game"), &other.join("game"));
        fs::copy(
            assets.join("map/ireland/terrain.toml"),
            other.join("map/ireland/terrain.toml"),
        )
        .unwrap();
        let travel = fs::read_to_string(other.join("game/travel.toml")).unwrap();
        fs::write(
            other.join("game/travel.toml"),
            format!("{travel}\n# slower\n"),
        )
        .unwrap();
        let mut meta = source.identity.meta(42);
        assert_eq!(
            hashes::rules_hash(&assets).unwrap(),
            meta.rules_hash,
            "the save's own rules are the shipped ones"
        );
        meta.rules_hash = hashes::rules_hash(&other).unwrap();
        assert_ne!(meta.rules_hash, source.identity.rules_hash);
        let save = source.simulation.save(meta);
        let path = super::super::saves::quicksave_path(&dir.0.join("saves"));
        write_save(&source, &save, &path);
        let game = GameConfig {
            scenario: assets.join("scenarios/ireland-768"),
            saves: dir.0.join("saves"),
            events_fixture: None,
        };
        let refused = prepare(
            &Request {
                game: &game,
                vms: &[],
                load: Some(&path),
                seed: None,
            },
            || Ok(42),
            &mut |_, _| {},
        );
        let message = refused.err().expect("the save is refused");
        assert!(message.contains("rules"), "{message}");
    }

    /// A directory copied with its files and folders.
    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    #[test]
    fn recorder_copies_the_completed_month_without_changing_the_simulation() {
        let dir = crate::testing::Dir::new();
        let source = shipped_start(&dir.0, None);
        let rules = source.economy_rules.clone().unwrap();
        let outputs = || Outputs {
            log: Box::new(|_, _| {}),
            publish: Box::new(|_| {}),
            fail: Box::new(|e| panic!("{e}")),
        };
        let mut recorded = test_core_running(source, dir.0.join("saves"), outputs());
        let mut plain =
            test_core_running(shipped_start(&dir.0, None), dir.0.join("saves"), outputs())
                .without_history();
        while recorded.date()
            < (Date {
                year: 768,
                month: 11,
                day: 1,
                hour: 13,
            })
        {
            assert!(recorded.history_months().is_empty());
            recorded.step_for_tests();
            plain.step_for_tests();
        }
        assert_eq!(
            recorded.history_months(),
            &[super::super::history::MonthRecord::sample(
                &rules,
                &recorded.state().economy,
                768,
                10
            )]
        );
        let state = &recorded.state().economy;
        let place = state
            .places
            .iter()
            .find(|place| !place.pops.is_empty())
            .unwrap();
        let record = recorded.history_months()[0]
            .places
            .iter()
            .find(|item| item.id == place.id)
            .unwrap();
        assert_eq!(
            (record.births, record.deaths),
            (place.counters.last.births, place.counters.last.deaths)
        );
        assert_eq!(record.goods[0].id, rules.goods[0].id);
        assert_eq!(record.goods[0].price, place.prices_milli[0]);
        assert_eq!(record.goods[0].produced, place.counters.last.produced[0]);
        let pop = &place.pops[0];
        let pop_record = record.pops.iter().find(|item| item.id == pop.id).unwrap();
        assert_eq!(
            (
                pop_record.workers,
                pop_record.dependents,
                pop_record.births,
                pop_record.deaths
            ),
            (
                pop.workers,
                pop.dependents,
                pop.counters.last.births,
                pop.counters.last.deaths
            )
        );
        let mut reordered = state.clone();
        for place in &mut reordered.places {
            place.pops.reverse();
        }
        let resampled = super::super::history::MonthRecord::sample(&rules, &reordered, 768, 10);
        let reordered_place = resampled
            .places
            .iter()
            .find(|item| item.id == place.id)
            .unwrap();
        for pop in &place.pops {
            let row = reordered_place
                .pops
                .iter()
                .find(|row| row.id == pop.id)
                .unwrap();
            assert_eq!(
                (row.workers, row.dependents, row.births, row.deaths),
                (
                    pop.workers,
                    pop.dependents,
                    pop.counters.last.births,
                    pop.counters.last.deaths
                )
            );
        }
        assert_eq!(
            ck_sim::Canonical::to_bytes(recorded.state()).unwrap(),
            ck_sim::Canonical::to_bytes(plain.state()).unwrap()
        );
        recorded.step_for_tests();
        assert_eq!(recorded.history_months().len(), 1);
    }

    /// CK-441: the graph viewer reads what the core publishes. The history revision rises when a month is recorded and when a load replaces the
    /// history, the horizon follows the clock, and a graph asked of the host is answered from the recorded month.
    #[test]
    fn the_graph_viewer_reads_the_history_the_core_publishes_and_its_revision_rises_with_each_change()
     {
        use super::super::history_view::{Graph, Host, Outcome, Range, Request, Subject};
        let dir = crate::testing::Dir::new();
        let source = shipped_start(&dir.0, None);
        let outputs = || Outputs {
            log: Box::new(|_, _| {}),
            publish: Box::new(|_| {}),
            fail: Box::new(|e| panic!("{e}")),
        };
        let (ready, woken) = std::sync::mpsc::channel();
        let host = Host::new(Arc::new(move || {
            let _ = ready.send(());
        }));
        let mut core = test_core_running(source, dir.0.join("saves"), outputs())
            .with_history_host(host.clone());
        let first = core.snapshot().history_revision;
        assert!(first > 0, "a game with history publishes it at once");
        // Nothing is recorded before the first 1st: no subjects beyond `global` and no chart.
        let request = |number| Request {
            subject: Subject::Global,
            graph: Graph::People,
            range: Range::All,
            budget: 250,
            serial: number,
        };
        host.request(request(1));
        woken.recv().unwrap();
        assert_eq!(host.take()[0].outcome, Outcome::NoRecords);
        while core.date()
            < (Date {
                year: 768,
                month: 11,
                day: 1,
                hour: 13,
            })
        {
            assert_eq!(
                core.snapshot().history_revision,
                first,
                "no change, no new revision"
            );
            core.step_for_tests();
        }
        let recorded = core.snapshot().history_revision;
        assert!(recorded > first, "a recorded month is a new revision");
        host.request(request(2));
        woken.recv().unwrap();
        let answer = host.take().remove(0);
        assert_eq!((answer.request, answer.revision), (2, recorded));
        let Outcome::Chart(chart) = answer.outcome else {
            panic!("a chart of the recorded month");
        };
        assert_eq!(chart.horizon, (768, 10));
        assert_eq!(
            chart.columns.len(),
            1,
            "all starts at the origin: one year, 768"
        );
        assert_eq!(chart.columns[0].months, 1);
        // A save and a quickload: the load replaces the history, so the revision rises again and the host serves it.
        core.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        core.step_for_tests();
        core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
        let loaded = core.snapshot().history_revision;
        assert!(loaded > recorded, "a load is a new revision");
        host.request(request(3));
        woken.recv().unwrap();
        let after = host.take().remove(0);
        assert_eq!(after.revision, loaded);
        assert!(matches!(after.outcome, Outcome::Chart(_)));
    }

    /// CK-431: a same-build save is read by the same code, and trade state that names a vehicle kind the rules do not have is refused when
    /// the game starts from it, before it is installed.
    #[test]
    fn a_same_build_save_with_malformed_trade_state_is_refused_at_start() {
        let dir = crate::testing::Dir::new();
        let source = shipped_start(&dir.0, None);
        let mut save = source.simulation.save(source.identity.meta(42));
        let goods = save.state.economy.places[0].pops[0].stock.len();
        save.state.economy.trade.jobs = 1;
        save.state.economy.places[0]
            .trade
            .jobs
            .push(ck_economy::Job {
                number: 1,
                pop: 0,
                kind: ck_economy::JobKind::Build {
                    vehicle: 99,
                    trip: None,
                },
                made: 0,
                need_goods: vec![0; goods],
                need_animals: Vec::new(),
                stock: vec![0; goods],
                animals: Vec::new(),
                hands: None,
                given_up: None,
            });
        let path = super::super::saves::quicksave_path(&dir.0.join("saves"));
        write_save(&source, &save, &path);
        let game = GameConfig {
            scenario: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/scenarios/ireland-768"),
            saves: dir.0.join("saves"),
            events_fixture: None,
        };
        let refused = prepare(
            &Request {
                game: &game,
                vms: &[],
                load: Some(&path),
                seed: None,
            },
            || Ok(42),
            &mut |_, _| {},
        );
        let message = refused.err().expect("the save is refused").clone();
        assert!(message.contains("trade"), "{message}");
    }

    #[test]
    fn hourly_low_wind_survives_a_real_save_and_load() {
        let dir = crate::testing::Dir::new();
        let mut prepared = shipped_start(&dir.0, None);
        let weather = Arc::clone(prepared.weather.as_ref().unwrap());
        let field =
            super::super::wind::WindField::new(weather.backdrop.as_ref().unwrap(), &weather.rules)
                .unwrap();
        let mut state = prepared.simulation.state().clone();
        field
            .place(&mut state.lows, 22833 - 105, 15538 - 105, 300_000, 6000)
            .unwrap();
        let date = Date {
            year: 768,
            month: 10,
            day: 9,
            hour: 6,
        };
        let scheduler = move || {
            let mut scheduler = Scheduler::<GameState>::new();
            let weather = Arc::clone(&weather);
            scheduler
                .register("weather.hourly", Cadence::Hourly, move |state, tick| {
                    weather.step_lows(&mut state.lows, tick.date());
                    Ok(())
                })
                .unwrap();
            scheduler
        };
        prepared.simulation = Simulation::new(
            GameTime::from_date(date).unwrap(),
            42,
            state,
            scheduler(),
            apply,
        );
        let mut core = core_over(
            prepared,
            dir.0.join("saves"),
            Outputs {
                log: Box::new(|_, _| {}),
                publish: Box::new(|_| {}),
                fail: Box::new(|error| panic!("{error}")),
            },
            scheduler,
        );
        let first = core.snapshot();
        assert_eq!(first.weather["birr"].1, Some((18, "S".into())));
        core.dispatch(Message::Control(Control::Quicksave), Duration::ZERO);
        core.dispatch(Message::Control(Control::TogglePause), Duration::ZERO);
        core.run_wake(Duration::from_secs(1), &mut || None);
        let second = core.snapshot();
        assert_eq!(second.date.hour, 7);
        assert_eq!(second.economy_revision, first.economy_revision);
        assert_eq!(second.weather["birr"].1, Some((16, "SW".into())));
        core.dispatch(Message::Control(Control::Quickload), Duration::from_secs(1));
        let loaded = core.snapshot();
        assert_eq!(loaded.date.hour, 6);
        assert_eq!(loaded.weather["birr"], first.weather["birr"]);
    }

    #[test]
    fn a_root_without_backdrop_refuses_a_nonempty_weather_list() {
        let dir = crate::testing::Dir::new();
        let (game, source) = fixture(&dir.0);
        let path = super::super::saves::quicksave_path(&game.saves);
        let mut save = source.simulation.save(source.identity.meta(42));
        save.state.lows.next = 2;
        save.state.lows.entries.push(super::super::lows::Low {
            number: 1,
            x: 21_361_500,
            y: 15_777_500,
            radius: 500_000,
            depth: 3000,
            strongest: 4489,
        });
        write_save(&source, &save, &path);
        let result = prepare(
            &Request {
                game: &game,
                vms: &[],
                load: Some(&path),
                seed: None,
            },
            || Ok(42),
            &mut |_, _| {},
        );
        assert!(result.err().unwrap().contains("weather list"));
        let lines = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&lines);
        let (_, running) = fixture(&dir.0);
        let mut core = test_core(
            running,
            game.saves.clone(),
            Outputs {
                log: Box::new(move |_, line| captured.lock().unwrap().push(line)),
                publish: Box::new(|_| {}),
                fail: Box::new(|e| panic!("{e}")),
            },
        );
        let before = core.digest();
        core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
        assert_eq!(core.digest(), before);
        assert!(
            lines
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("weather list"))
        );
    }

    #[test]
    // Long run: selected by app.long.
    fn app_long_malformed_weather_lists_are_refused_by_both_load_paths() {
        let dir = crate::testing::Dir::new();
        let source = shipped_start(&dir.0, None);
        let game = GameConfig {
            scenario: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/scenarios/ireland-768"),
            saves: dir.0.join("saves"),
            events_fixture: None,
        };
        let weather = source.weather.as_ref().unwrap();
        let field =
            super::super::wind::WindField::new(weather.backdrop.as_ref().unwrap(), &weather.rules)
                .unwrap();
        let mut valid = super::super::lows::Lows::default();
        field
            .place(&mut valid, 21361, 15777, 500_000, 3000)
            .unwrap();
        field
            .place(&mut valid, 21892, 15336, 800_000, 2000)
            .unwrap();
        let mut cases = Vec::new();
        let mut order = valid.clone();
        order.entries.reverse();
        cases.push(order);
        let mut number = valid.clone();
        number.next = 2;
        cases.push(number);
        let mut radius = valid.clone();
        radius.entries[0].radius = 49_999;
        cases.push(radius);
        let mut centre = valid;
        centre.entries[0].y = 0;
        cases.push(centre);
        let path = super::super::saves::quicksave_path(&game.saves);
        for bad in cases {
            let mut save = source.simulation.save(source.identity.meta(42));
            save.state.lows = bad;
            write_save(&source, &save, &path);
            let result = prepare(
                &Request {
                    game: &game,
                    vms: &[],
                    load: Some(&path),
                    seed: None,
                },
                || Ok(42),
                &mut |_, _| {},
            );
            assert!(result.err().unwrap().contains("weather list"));
            let lines = Arc::new(Mutex::new(Vec::new()));
            let captured = Arc::clone(&lines);
            let mut core = test_core_running(
                shipped_start(&dir.0, None),
                game.saves.clone(),
                Outputs {
                    log: Box::new(move |_, line| captured.lock().unwrap().push(line)),
                    publish: Box::new(|_| {}),
                    fail: Box::new(|e| panic!("{e}")),
                },
            );
            let before = core.digest();
            core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
            assert_eq!(core.digest(), before);
            assert!(
                lines
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|line| line.contains("weather list"))
            );
        }
    }

    #[test]
    fn trial_a_six_hour_save_resumes_the_same_low() {
        let dir = crate::testing::Dir::new();
        let source = shipped_start(&dir.0, None);
        let weather = source.weather.as_ref().unwrap();
        let field =
            super::super::wind::WindField::new(weather.backdrop.as_ref().unwrap(), &weather.rules)
                .unwrap();
        let mut save = source.simulation.save(source.identity.meta(42));
        field
            .place(&mut save.state.lows, 21361, 15777, 500_000, 3000)
            .unwrap();
        for _ in 0..6 {
            field.step(&mut save.state.lows, 769, 1, 7);
        }
        assert_eq!(
            (save.state.lows.entries[0].x, save.state.lows.entries[0].y),
            (21_769_841, 15_530_381)
        );
        save.time = GameTime::from_date(Date {
            year: 769,
            month: 1,
            day: 3,
            hour: 6,
        })
        .unwrap();
        let mut expected = save.state.lows.clone();
        let path = super::super::saves::quicksave_path(&dir.0.join("saves"));
        write_save(&source, &save, &path);
        let mut loaded = shipped_start(&dir.0, Some(&path));
        assert_eq!(loaded.simulation.state().lows, expected);
        field.step(&mut expected, 769, 1, 7);
        loaded.simulation.step().unwrap();
        assert_eq!(loaded.simulation.state().lows, expected);
    }

    #[test]
    fn placed_low_follows_clock_and_both_load_paths() {
        let dir = crate::testing::Dir::new();
        let source = shipped_start(&dir.0, None);
        assert_eq!(
            hashes::rules_hash(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets"))
                .unwrap(),
            "51b9cb83438cf7a6"
        );
        let mut save = source.simulation.save(source.identity.meta(42));
        let weather = source.weather.as_ref().unwrap();
        let field =
            super::super::wind::WindField::new(weather.backdrop.as_ref().unwrap(), &weather.rules)
                .unwrap();
        field
            .place(&mut save.state.lows, 21361, 15777, 500_000, 3000)
            .unwrap();
        assert_eq!(
            super::super::wind::display(save.state.lows.entries[0].strongest, 0).0,
            45
        );
        let path = super::super::saves::quicksave_path(&dir.0.join("saves"));
        write_save(&source, &save, &path);
        let mut game = shipped_start(&dir.0, Some(&path));
        let game_config = GameConfig {
            scenario: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/scenarios/ireland-768"),
            saves: dir.0.join("saves"),
            events_fixture: None,
        };
        let mut loaded = None;
        for (hour, expected) in [
            (7, (21_731_229, 15_643_875)),
            (12, (21_997_712, 15_588_624)),
            (13, (22_052_089, 15_578_602)),
        ] {
            while game.simulation.date().unwrap().hour != hour {
                game.simulation.step().unwrap();
            }
            let low = &game.simulation.state().lows.entries[0];
            assert_eq!((low.x, low.y), expected);
            let view = view_builder(game.view_wiring())(
                game.simulation.state(),
                game.simulation.date().unwrap(),
            );
            assert_eq!(
                view.places
                    .iter()
                    .find(|p| p.id == "birr")
                    .unwrap()
                    .wind
                    .as_ref()
                    .unwrap(),
                &(2, "SW".to_owned())
            );
            if hour == 7 {
                loaded = Some(load_both_ways(&dir.0, &game_config, &game, "weather"));
            } else {
                follow_to(
                    loaded.as_mut().unwrap(),
                    &game,
                    game.simulation.date().unwrap(),
                );
            }
        }
        let end = Date {
            year: 768,
            month: 10,
            day: 10,
            hour: 0,
        };
        while game.simulation.date().unwrap() != end {
            game.simulation.step().unwrap();
        }
        follow_to(loaded.as_mut().unwrap(), &game, end);
        let low = &game.simulation.state().lows.entries[0];
        assert_eq!((low.x, low.y), (22_589_673, 15_510_428));
        let view = view_builder(game.view_wiring())(game.simulation.state(), end);
        assert_eq!(
            view.places
                .iter()
                .find(|p| p.id == "birr")
                .unwrap()
                .wind
                .as_ref()
                .unwrap(),
            &(2, "SW".to_owned())
        );
    }

    #[test]
    fn shipped_ninth_october_start_opens_with_october_crop_work() {
        let dir = crate::testing::Dir::new();
        let game = shipped_start(&dir.0, None);
        let start = game.simulation.date().unwrap();
        assert_eq!((start.month, start.day, start.hour), (10, 9, 0));
        let rules = Rules::load(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
        )
        .unwrap();
        let land = game.view_wiring().land_by_place;
        let quality = land
            .iter()
            .map(|(id, land)| (id.clone(), ck_economy::mean_fertility(land).unwrap()))
            .collect();
        let scenario_dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenarios/ireland-768");
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let (original, _) =
            economy::starting_economy(&assets, &scenario_dir, &game.scenario).unwrap();
        for (built, installed) in original
            .places
            .iter()
            .zip(&game.simulation.state().economy.places)
        {
            for (built, installed) in built.buildings.iter().zip(&installed.buildings) {
                for (built, installed) in built.level_records.iter().zip(&installed.level_records) {
                    assert_eq!(built.unpaid_work, installed.unpaid_work);
                }
            }
        }
        let mut october = original.clone();
        for place in &mut october.places {
            for building in &mut place.buildings {
                for level in &mut building.level_records {
                    for value in level.unpaid_work.values_mut() {
                        *value = ck_economy::bigint::Big::default();
                    }
                }
            }
        }
        ck_economy::run_monthly_production_with_quality(
            &rules,
            &mut october,
            Date {
                year: 768,
                month: 10,
                day: 1,
                hour: 12,
            },
            &quality,
        )
        .unwrap();
        let mut positive = 0;
        for (actual, simulated) in original.places.iter().zip(&october.places) {
            for (a, b) in actual.buildings.iter().zip(&simulated.buildings) {
                for (a, b) in a.level_records.iter().zip(&b.level_records) {
                    if let (Some(actual), Some(expected)) =
                        (a.unpaid_work.get("oats"), b.unpaid_work.get("oats"))
                    {
                        assert_eq!(actual, expected, "level {}", a.number);
                        positive += usize::from(actual != &ck_economy::bigint::Big::default());
                    }
                }
            }
        }
        assert!(positive > 0);
    }

    #[test]
    fn a_shipped_game_saved_at_11_and_13_on_payday_matches_the_unbroken_game() {
        // The two saves straddle a first-of-month noon, when seasonal production and unpaid work are updated.
        let dir = crate::testing::Dir::new();
        let path = super::super::saves::quicksave_path(&dir.0.join("saves"));
        for hour in [11, 13] {
            let mut prepared = shipped_start(&dir.0, None);
            assert!(
                !prepared.simulation.state().economy.places.is_empty(),
                "the start has populated the economy"
            );
            while {
                let date = prepared.simulation.date().unwrap();
                date.day != 1 || date.hour != hour
            } {
                prepared.simulation.step().unwrap();
            }
            assert_eq!(prepared.simulation.date().unwrap().day, 1);
            let state = prepared.simulation.state().stretches.clone();
            let digest = save_with(&prepared, &path, state);
            let mut loaded = shipped_start(&dir.0, Some(&path));
            assert_eq!(loaded.simulation.digest().unwrap(), digest);
            // Through the following noon: an exchange missed after the 11:00 load, or made twice after the 13:00 load, would differ.
            for _ in 0..24 {
                prepared.simulation.step().unwrap();
                loaded.simulation.step().unwrap();
            }
            assert_eq!(
                loaded.simulation.digest().unwrap(),
                prepared.simulation.digest().unwrap(),
                "saved at {hour}:00"
            );
        }
    }

    /// Every level of the economy as (place, building, level number) with its method and year of last switch.
    type LevelMethod = (String, usize, u32, u32, Option<(u16, u8)>);

    fn level_methods(prepared: &Prepared) -> Vec<LevelMethod> {
        let mut levels = Vec::new();
        for place in &prepared.simulation.state().economy.places {
            for (index, building) in place.buildings.iter().enumerate() {
                for record in &building.level_records {
                    levels.push((
                        place.id.clone(),
                        index,
                        record.number,
                        record.method,
                        record.last_switch,
                    ));
                }
            }
        }
        levels
    }

    fn step_to(prepared: &mut Prepared, month: u8, day: u8, hour: u8) {
        loop {
            let date = prepared.simulation.date().unwrap();
            if (date.month, date.day, date.hour) == (month, day, hour) {
                return;
            }
            prepared.simulation.step().unwrap();
        }
    }

    /// A save written from `save` beside `source`'s character storage.
    fn write_save(source: &Prepared, save: &Save<GameState>, path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut archive = fs::File::open(source.storage.path.join("ARCH")).unwrap();
        let mut logs = fs::File::open(source.storage.path.join("LOGS")).unwrap();
        let lengths = (
            archive.metadata().unwrap().len(),
            logs.metadata().unwrap().len(),
        );
        let mut file = fs::File::create(path).unwrap();
        save.write_with_sections(
            &mut file,
            &mut [
                (*b"ARCH", lengths.0, &mut archive),
                (*b"LOGS", lengths.1, &mut logs),
            ],
        )
        .unwrap();
    }

    /// A game loaded two ways from one save: by a start from it and by a load during play into a core that runs the game's own
    /// scheduler.
    struct Loaded {
        label: String,
        started: Prepared,
        playing: Core<GameState, GameCommand>,
    }

    /// Saves `unbroken` where it stands and loads the save both ways; each load must hold the saved state.
    fn load_both_ways(dir: &Path, game: &GameConfig, unbroken: &Prepared, label: &str) -> Loaded {
        let path = super::super::saves::quicksave_path(&dir.join("saves"));
        let digest = save_with(
            unbroken,
            &path,
            unbroken.simulation.state().stretches.clone(),
        );
        let started = shipped_start(dir, Some(&path));
        assert_eq!(started.simulation.digest().unwrap(), digest, "{label}");
        assert_eq!(level_methods(&started), level_methods(unbroken), "{label}");
        let mut playing = test_core_running(
            shipped_start(dir, None),
            game.saves.clone(),
            Outputs {
                log: Box::new(|_, _| {}),
                publish: Box::new(|_| {}),
                fail: Box::new(|e| panic!("{e}")),
            },
        );
        playing.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
        // What a save holds is the state without the day's cached market orders, which the next noon rebuilds.
        let saved = <ck_economy::EconomyState as ck_sim::Canonical>::from_bytes(
            &ck_sim::Canonical::to_bytes(&unbroken.simulation.state().economy).unwrap(),
        )
        .unwrap();
        assert_eq!(
            playing.state().economy,
            saved,
            "{label}: a load during play"
        );
        assert_eq!(
            playing.date(),
            unbroken.simulation.date().unwrap(),
            "{label}"
        );
        Loaded {
            label: label.to_owned(),
            started,
            playing,
        }
    }

    /// Steps both loaded games to `end` and compares them with the unbroken game there, state for state.
    fn follow_to(loaded: &mut Loaded, unbroken: &Prepared, end: Date) {
        while loaded.started.simulation.date().unwrap() != end {
            loaded.started.simulation.step().unwrap();
        }
        while loaded.playing.date() != end {
            loaded.playing.step_for_tests();
        }
        let label = &loaded.label;
        assert_eq!(
            loaded.started.simulation.digest().unwrap(),
            unbroken.simulation.digest().unwrap(),
            "{label}: the started game follows the unbroken one"
        );
        assert_eq!(
            ck_sim::Canonical::to_bytes(loaded.playing.state()).unwrap(),
            ck_sim::Canonical::to_bytes(unbroken.simulation.state()).unwrap(),
            "{label}: the game that loaded during play follows the unbroken one"
        );
        assert_eq!(
            level_methods(&loaded.started),
            level_methods(unbroken),
            "{label}"
        );
    }

    /// A start in which every farm has all its levels at sea and no animals, so that the first look moves levels to farming and
    /// founders arrive.
    fn engineered_start(dir: &Path) -> Prepared {
        let source = shipped_start(dir, None);
        let rules = Rules::load(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
        )
        .unwrap();
        let definition = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        let mut save = source.simulation.save(source.identity.meta(42));
        for place in &mut save.state.economy.places {
            for farm in place
                .buildings
                .iter_mut()
                .filter(|b| b.kind == "farm" && !b.level_records.is_empty())
            {
                for record in &mut farm.level_records {
                    record.method = 1;
                    record.ground = None;
                    record.unpaid_work = ["fish", "seabirds"]
                        .into_iter()
                        .map(|good| (good.to_owned(), ck_economy::bigint::Big::default()))
                        .collect();
                }
                farm.method_levels = vec![0; definition.methods.len()];
                farm.method_levels[1] = farm.levels;
                farm.herds.fill(0);
                fix_sums(&rules, farm);
            }
        }
        let path = super::super::saves::quicksave_path(&dir.join("engineered"));
        write_save(&source, &save, &path);
        shipped_start(dir, Some(&path))
    }

    /// The places and buildings whose herds began with no animals and have some, in a level that moved: the founders.
    fn founder_buildings(before: &Prepared, after: &Prepared) -> Vec<(String, usize)> {
        let mut found = Vec::new();
        for (was, now) in before
            .simulation
            .state()
            .economy
            .places
            .iter()
            .zip(&after.simulation.state().economy.places)
        {
            for (index, (b, a)) in was.buildings.iter().zip(&now.buildings).enumerate() {
                let moved = a
                    .level_records
                    .iter()
                    .any(|r| r.method != 1 && r.last_switch == Some((768, 11)));
                if moved && b.herds.iter().all(|h| *h == 0) && a.herds.iter().any(|h| *h > 0) {
                    found.push((now.id.clone(), index));
                }
            }
        }
        found
    }

    #[test]
    // Long run: selected by app.long.
    fn app_long_a_shipped_game_saved_at_a_move_with_founders_and_at_the_year_end_loads_both_ways_and_follows_the_unbroken_game()
     {
        let dir = crate::testing::Dir::new();
        let game = GameConfig {
            scenario: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/scenarios/ireland-768"),
            saves: dir.0.join("saves"),
            events_fixture: None,
        };
        let mut unbroken = engineered_start(&dir.0);
        // The first 1st: saved at 11:00 and at 13:00 through the move. Every farm began at sea with no animals, so levels move to
        // farming and founders arrive in the places that look to it.
        step_to(&mut unbroken, 11, 1, 11);
        let mut checkpoints = vec![load_both_ways(
            &dir.0,
            &game,
            &unbroken,
            "11:00 on the first 1st",
        )];
        for _ in 0..2 {
            unbroken.simulation.step().unwrap();
        }
        let methods_now = level_methods(&unbroken);
        assert_ne!(
            methods_now,
            level_methods(&checkpoints[0].started),
            "a level moved at that noon"
        );
        let founders = founder_buildings(&checkpoints[0].started, &unbroken);
        assert!(
            !founders.is_empty(),
            "founders arrived in a level that moved"
        );
        checkpoints.push(load_both_ways(
            &dir.0,
            &game,
            &unbroken,
            "13:00 on the first 1st",
        ));
        // Both loads preserve the new method, founders, paydays and switch stamp.
        for loaded in &checkpoints[1..] {
            for (place, index) in &founders {
                let want = unbroken
                    .simulation
                    .state()
                    .economy
                    .places
                    .iter()
                    .find(|p| &p.id == place)
                    .unwrap();
                for state in [loaded.started.simulation.state(), loaded.playing.state()] {
                    let got = state
                        .economy
                        .places
                        .iter()
                        .find(|p| &p.id == place)
                        .unwrap();
                    assert_eq!(got.buildings[*index], want.buildings[*index], "{place}");
                    assert!(got.buildings[*index].herds.iter().any(|h| *h > 0));
                    assert!(
                        got.buildings[*index]
                            .level_records
                            .iter()
                            .any(|r| { r.method != 1 && r.last_switch == Some((768, 11)) })
                    );
                }
            }
        }
        // Save at year end and around the next monthly look, then compare both load paths.
        step_to(&mut unbroken, 12, 31, 13);
        assert!(
            level_methods(&unbroken)
                .iter()
                .any(|level| level.4.is_some_and(|(year, _)| year == 768)),
            "a level retains a switch stamp from 768"
        );
        checkpoints.push(load_both_ways(&dir.0, &game, &unbroken, "31 December"));
        step_to(&mut unbroken, 1, 1, 11);
        checkpoints.push(load_both_ways(
            &dir.0,
            &game,
            &unbroken,
            "11:00 on 1 January",
        ));
        for _ in 0..2 {
            unbroken.simulation.step().unwrap();
        }
        checkpoints.push(load_both_ways(
            &dir.0,
            &game,
            &unbroken,
            "13:00 on 1 January",
        ));
        // Every loaded game, kept and stepped from its checkpoint through 2 January, makes the unbroken game's moves.
        step_to(&mut unbroken, 1, 2, 13);
        let end = unbroken.simulation.date().unwrap();
        for loaded in &mut checkpoints {
            follow_to(loaded, &unbroken, end);
        }
    }

    fn reject_switch_stamp_on_both_load_paths(stamp: (u16, u8)) {
        let dir = crate::testing::Dir::new();
        let game = GameConfig {
            scenario: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/scenarios/ireland-768"),
            saves: dir.0.join("saves"),
            events_fixture: None,
        };
        let new_game = || shipped_start(&dir.0, None);
        let source = new_game();
        let path = super::super::saves::quicksave_path(&game.saves);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut save = source.simulation.save(source.identity.meta(42));
        let farm = target(&mut save);
        farm.level_records[0].last_switch = Some(stamp);
        let mut archive = fs::File::open(source.storage.path.join("ARCH")).unwrap();
        let mut logs = fs::File::open(source.storage.path.join("LOGS")).unwrap();
        let lengths = (
            archive.metadata().unwrap().len(),
            logs.metadata().unwrap().len(),
        );
        let mut file = fs::File::create(&path).unwrap();
        save.write_with_sections(
            &mut file,
            &mut [
                (*b"ARCH", lengths.0, &mut archive),
                (*b"LOGS", lengths.1, &mut logs),
            ],
        )
        .unwrap();
        // A start from the save names the building and the level.
        let message = prepare(
            &Request {
                game: &game,
                vms: &[],
                load: Some(&path),
                seed: None,
            },
            || Ok(42),
            &mut |_, _| {},
        )
        .err()
        .expect("a start from the save refuses it");
        assert!(
            message.contains("building farm")
                && message.contains("level 0")
                && message.contains(&stamp.0.to_string()),
            "{message}"
        );
        // A load during play refuses it too, and the running game, its storage and its readers are as they were.
        let running = new_game();
        let handle = Arc::clone(running.storage.handle.as_ref().unwrap());
        let archive = Arc::clone(running.storage.cell.as_ref().unwrap());
        let logs = Arc::clone(running.storage.log_cell.as_ref().unwrap());
        let before_path = handle.lock().unwrap().clone();
        let before_archive = archive.lock().unwrap().clone().unwrap();
        let before_logs = logs.lock().unwrap().clone().unwrap();
        let records = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&records);
        let mut core = test_core(
            running,
            game.saves.clone(),
            Outputs {
                log: Box::new(move |_, line| output.lock().unwrap().push(line)),
                publish: Box::new(|_| {}),
                fail: Box::new(|e| panic!("{e}")),
            },
        );
        let before = core.digest();
        core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
        assert!(
            records
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("building farm") && line.contains("level 0")),
            "{:?}",
            records.lock().unwrap()
        );
        assert_eq!(core.digest(), before, "the running game is as it was");
        assert_eq!(*handle.lock().unwrap(), before_path);
        assert!(Arc::ptr_eq(
            archive.lock().unwrap().as_ref().unwrap(),
            &before_archive
        ));
        assert!(Arc::ptr_eq(
            logs.lock().unwrap().as_ref().unwrap(),
            &before_logs
        ));
    }

    #[test]
    fn both_load_paths_accept_current_and_previous_december_switch_stamps() {
        for (date, stamp) in [
            (
                Date {
                    year: 768,
                    month: 10,
                    day: 9,
                    hour: 0,
                },
                (768, 10),
            ),
            (
                Date {
                    year: 769,
                    month: 1,
                    day: 1,
                    hour: 0,
                },
                (768, 12),
            ),
        ] {
            let dir = crate::testing::Dir::new();
            let game = GameConfig {
                scenario: Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../assets/scenarios/ireland-768"),
                saves: dir.0.join("saves"),
                events_fixture: None,
            };
            let source = shipped_start(&dir.0, None);
            let mut save = source.simulation.save(source.identity.meta(42));
            save.time = GameTime::from_date(date).unwrap();
            target(&mut save).level_records[0].last_switch = Some(stamp);
            let path = super::super::saves::quicksave_path(&game.saves);
            write_save(&source, &save, &path);
            let started = shipped_start(&dir.0, Some(&path));
            assert!(
                level_methods(&started)
                    .iter()
                    .any(|level| level.4 == Some(stamp))
            );
            let running = shipped_start(&dir.0, None);
            let mut core = test_core(
                running,
                game.saves.clone(),
                Outputs {
                    log: Box::new(|_, _| {}),
                    publish: Box::new(|_| {}),
                    fail: Box::new(|e| panic!("{e}")),
                },
            );
            core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
            assert!(
                core.state()
                    .economy
                    .places
                    .iter()
                    .flat_map(|p| &p.buildings)
                    .flat_map(|b| &b.level_records)
                    .any(|record| record.last_switch == Some(stamp))
            );
        }
    }

    #[test]
    fn both_load_paths_refuse_invalid_and_future_switch_months() {
        for stamp in [(768, 12), (768, 0), (768, 13), (769, 1)] {
            reject_switch_stamp_on_both_load_paths(stamp);
        }
    }

    #[test]
    #[ignore = "the full shipped-year witness runs in the named methods.year group and milestone full check"]
    fn shipped_yearly_monthly_looks_report_passing_moves() {
        let dir = crate::testing::Dir::new();
        let mut game = shipped_start(&dir.0, None);
        let land = game.view_wiring().land_by_place;
        let rules = Rules::load(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
        )
        .unwrap();
        for _ in 0..12 {
            while {
                let date = game.simulation.date().unwrap();
                date.day != 1 || date.hour != 11
            } {
                game.simulation.step().unwrap();
            }
            let prices: Vec<_> = game
                .simulation
                .state()
                .economy
                .places
                .iter()
                .map(|p| p.prices_milli.clone())
                .collect();
            game.simulation.step().unwrap();
            game.simulation.step().unwrap();
            let date = game.simulation.date().unwrap();
            let mut passing = std::collections::BTreeMap::<(String, String), usize>::new();
            for (place_index, place) in game.simulation.state().economy.places.iter().enumerate() {
                for (index, building) in place.buildings.iter().enumerate() {
                    let def = rules
                        .buildings
                        .iter()
                        .find(|d| d.id == building.kind)
                        .unwrap();
                    if def.methods.len() < 2 {
                        continue;
                    }
                    let result = ck_economy::switching::look(
                        &rules,
                        place,
                        &land[&place.id],
                        index,
                        date.month,
                        &prices[place_index],
                        &std::collections::BTreeMap::new(),
                    )
                    .unwrap();
                    for item in result.moves.iter().filter(|item| {
                        !item.surplus.negative
                            && item.surplus.magnitude.numerator
                                != ck_economy::bigint::Big::default()
                    }) {
                        let record = building
                            .level_records
                            .iter()
                            .find(|r| r.number == item.level)
                            .unwrap();
                        *passing
                            .entry((
                                def.methods[record.method as usize].id.clone(),
                                def.methods[item.method].id.clone(),
                            ))
                            .or_default() += 1;
                    }
                }
            }
            println!("{}-{:02}: {passing:?}", date.year, date.month);
            while game.simulation.date().unwrap().day == 1 {
                game.simulation.step().unwrap();
            }
        }
    }

    #[test]
    fn shipped_network_initializes_every_stretch_and_exactly_the_five_roads() {
        let dir = crate::testing::Dir::new();
        let game = GameConfig {
            scenario: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/scenarios/ireland-768"),
            saves: dir.0.join("saves"),
            events_fixture: None,
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
        let crossing_stretches: Vec<_> = network
            .stretches
            .iter()
            .filter(|s| s.crossings.is_some())
            .collect();
        assert_eq!(crossing_stretches.len(), 45);
        assert_eq!(
            crossing_stretches.iter().filter(|s| s.is_land()).count(),
            43
        );
        assert_eq!(
            crossing_stretches
                .iter()
                .map(|s| s.crossings.as_ref().unwrap().len())
                .sum::<usize>(),
            47
        );
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

/// CK-409: a small game of one place, five years of its daily system with levels that switch, saved at named points and loaded
/// both ways, each loaded game stepped in lockstep with the unbroken one.
#[cfg(test)]
mod switch_replay_tests {
    use super::*;
    use ck_economy::{
        ScenarioEconomy, StartDefaults, StartLocation, StartOverride, starting_state,
    };
    use ck_sim::GameTime;
    use std::{fs, time::Duration};

    use crate::game::{controls::Control, core::Message, tests::identity};

    fn rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
            .unwrap()
    }

    fn land() -> ck_economy::LandCounts {
        ck_economy::LandCounts {
            land_cells: BTreeMap::from([
                ("pasture".to_owned(), 40),
                ("forest".to_owned(), 12),
                ("bog".to_owned(), 6),
            ]),
            fertility_pm: BTreeMap::from([("pasture".to_owned(), 24_000)]),
            fertile_cells: 40,
            fertile_cells_by_terrain: BTreeMap::from([("pasture".to_owned(), 40)]),
            sea: 1,
        }
    }

    /// One fort of 400 peasants and 20 monks, with a farm of fourteen levels and holdings of four, every level at sea and no animals:
    /// the place wants oats, so levels of both buildings move to farming from the first look, and founders arrive.
    fn fort(rules: &Rules) -> ck_economy::EconomyState {
        let data = ScenarioEconomy {
            horses_always: Vec::new(),
            defaults: BTreeMap::from([(
                "ringfort".to_owned(),
                BTreeMap::from([(
                    "1".to_owned(),
                    StartDefaults {
                        peasants: 400,
                        monks: 20,
                        labourers: 0,
                        farm_levels: 14,
                        holdings_levels: 4,
                        storehouse_levels: 1,
                        pop_stock: BTreeMap::new(),
                        building_stock: BTreeMap::new(),
                    },
                )]),
            )]),
            places: BTreeMap::from([(
                "fort".to_owned(),
                StartOverride {
                    culture: Some("ulaid".to_owned()),
                    religion: Some("christianity".to_owned()),
                    monk_culture: Some("ulaid".to_owned()),
                    monk_religion: Some("christianity".to_owned()),
                    ..StartOverride::default()
                },
            )]),
            nobles: BTreeMap::new(),
        };
        let (mut state, _) = starting_state(
            rules,
            &data,
            &[StartLocation {
                id: "fort".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: land(),
            }],
            1,
        )
        .unwrap();
        for building in state.places[0].buildings.iter_mut().filter(|b| {
            ["farm", "holdings"].contains(&b.kind.as_str()) && !b.level_records.is_empty()
        }) {
            let definition = rules
                .buildings
                .iter()
                .find(|b| b.id == building.kind)
                .unwrap();
            // The goods a sea level is paid by season: those its method makes that no herd of the building supplies.
            let accounts: BTreeMap<String, ck_economy::bigint::Big> = definition.methods[1]
                .annual_output
                .keys()
                .filter(|good| !ck_economy::herd_supplies(rules, definition, good))
                .map(|good| (good.clone(), ck_economy::bigint::Big::default()))
                .collect();
            for record in &mut building.level_records {
                record.method = 1;
                record.ground = None;
                record.paydays = ck_economy::MATURE_PAYDAYS;
                record.unpaid_work.clone_from(&accounts);
            }
            building.method_levels = vec![0; definition.methods.len()];
            building.method_levels[1] = building.levels;
            building.herds.fill(0);
            (building.method_suitability, building.herd_grazing_sums) =
                ck_economy::derive_level_sums(rules, definition, &building.level_records).unwrap();
        }
        state
    }

    /// The scheduler the running game builds, for this one place.
    fn scheduler_source(rules: &Arc<Rules>) -> Arc<dyn Fn() -> Scheduler<GameState> + Send + Sync> {
        let rules = Arc::clone(rules);
        let land = BTreeMap::from([("fort".to_owned(), land())]);
        let quality = BTreeMap::from([(
            "fort".to_owned(),
            ck_economy::mean_fertility(&land["fort"]).unwrap(),
        )]);
        let shelter = BTreeMap::from([(
            "fort".to_owned(),
            ck_economy::natural_shelter(&rules, &land["fort"]).unwrap(),
        )]);
        let names = BTreeMap::from([("fort".to_owned(), "Fort".to_owned())]);
        Arc::new(move || {
            gameplay_scheduler_with_rules(
                None,
                Some(Arc::clone(&rules)),
                quality.clone(),
                land.clone(),
                shelter.clone(),
                BTreeMap::new(),
                BTreeMap::new(),
                BTreeMap::new(),
                names.clone(),
                None,
                None,
                None,
            )
        })
    }

    fn new_game(
        rules: &Arc<Rules>,
        make: &Arc<dyn Fn() -> Scheduler<GameState> + Send + Sync>,
    ) -> Simulation<GameState, GameCommand> {
        Simulation::new(
            GameTime::from_date(Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 0,
            })
            .unwrap(),
            42,
            GameState {
                economy: fort(rules),
                characters: ck_characters::CharacterStore::default(),
                stretches: BTreeMap::new(),
                lows: crate::game::lows::Lows::default(),
            },
            make(),
            apply,
        )
    }

    /// The load checks of both load paths, on a state and the date of its tick.
    fn level_checks(rules: &Rules, state: &GameState, date: Date) -> Result<(), String> {
        let quality = BTreeMap::from([(
            "fort".to_owned(),
            ck_economy::mean_fertility(&land()).unwrap(),
        )]);
        ck_economy::validate_level_records(rules, &quality, &state.economy)?;
        ck_economy::validate_switch_years(&state.economy, date.year, date.month)
    }

    /// A start from the saved bytes: the checks of a start, then the simulation.
    fn started(
        rules: &Arc<Rules>,
        make: &Arc<dyn Fn() -> Scheduler<GameState> + Send + Sync>,
        bytes: &[u8],
    ) -> Simulation<GameState, GameCommand> {
        let identity = identity("aa");
        let save = Save::<GameState>::from_bytes(bytes, &identity.expected()).unwrap();
        let date = save.time.date().unwrap();
        level_checks(rules, &save.state, date).unwrap();
        Simulation::from_save(save, make(), apply).unwrap()
    }

    /// A load during play of the saved bytes, into a core that runs the game's own systems.
    fn playing(
        rules: &Arc<Rules>,
        make: &Arc<dyn Fn() -> Scheduler<GameState> + Send + Sync>,
        bytes: &[u8],
    ) -> Core<GameState, GameCommand> {
        let dir = crate::testing::Dir::new();
        let saves = dir.0.join("saves");
        let path = super::super::saves::quicksave_path(&saves);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        let (for_scheduler, for_checks) = (Arc::clone(make), Arc::clone(rules));
        let mut core = Core::new_with_views_and_layers(
            new_game(rules, make),
            identity("aa"),
            saves,
            Rebuild {
                scheduler: Box::new(move || for_scheduler()),
                reset: Box::new(|_| {}),
                apply,
            },
            Outputs {
                log: Box::new(|_, _| {}),
                publish: Box::new(|_| {}),
                fail: Box::new(|e| panic!("{e}")),
            },
            None,
            ck_sim::MapLayers::default(),
        )
        .with_state_validation(move |state: &GameState, date| {
            level_checks(&for_checks, state, date)
        });
        core.dispatch(Message::Control(Control::Quickload), Duration::ZERO);
        // The temporary directory outlives the load: the core has read the file.
        drop(dir);
        core
    }

    struct Branch {
        label: String,
        started: Simulation<GameState, GameCommand>,
        playing: Core<GameState, GameCommand>,
    }

    type MethodSnapshot = (usize, u32, u32, Option<(u16, u8)>);

    fn methods(state: &GameState) -> Vec<MethodSnapshot> {
        let mut all = Vec::new();
        for (index, building) in state.economy.places[0].buildings.iter().enumerate() {
            for record in &building.level_records {
                all.push((index, record.number, record.method, record.last_switch));
            }
        }
        all
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "One replay is saved at each named point, loaded both ways and compared tick by tick."
    )]
    // Long run: selected by app.long.
    fn app_long_five_years_of_switching_saved_at_the_named_points_follow_the_unbroken_game_by_both_load_paths()
     {
        let rules = Arc::new(rules());
        let make = scheduler_source(&rules);
        let mut unbroken = new_game(&rules, &make);
        let save = |game: &Simulation<GameState, GameCommand>| {
            game.save(identity("aa").meta(42)).to_bytes().unwrap()
        };
        let end = GameTime::from_date(Date {
            year: 773,
            month: 1,
            day: 2,
            hour: 13,
        })
        .unwrap();
        let mut branches: Vec<Branch> = Vec::new();
        let mut candidate: Option<Vec<u8>> = None;
        let mut found = false;
        let mut year_end_done = [false; 3];
        let mut moves: Vec<(Date, usize, u32, u32, u32)> = Vec::new();
        let mut snapshot = methods(unbroken.state());
        while unbroken.time() < end {
            let now = unbroken.date().unwrap();
            // The named points. Before the first move has been found, each 1st at 11:00 is kept in case that noon moves a level.
            if !found && now.day == 1 && now.hour == 11 {
                candidate = Some(save(&unbroken));
            }
            let errors = unbroken.step().unwrap();
            assert!(errors.is_empty(), "{errors:?}");
            let after = unbroken.date().unwrap();
            // Every loaded game taken so far steps with the unbroken one, and holds its state after every tick.
            for branch in &mut branches {
                let errors = branch.started.step().unwrap();
                assert!(errors.is_empty(), "{errors:?}");
                branch.playing.step_for_tests();
                let digest = unbroken.digest().unwrap();
                assert_eq!(
                    branch.started.digest().unwrap(),
                    digest,
                    "{}: a start from the save, at {after:?}",
                    branch.label
                );
                assert_eq!(
                    branch.playing.digest(),
                    digest,
                    "{}: a load during play, at {after:?}",
                    branch.label
                );
            }
            let current = methods(unbroken.state());
            if after.hour == 13 && after.day == 1 {
                for (was, is) in snapshot.iter().zip(&current) {
                    if was.2 != is.2 {
                        moves.push((after, is.0, is.1, was.2, is.2));
                    }
                }
                snapshot.clone_from(&current);
            }
            let take = |label: &str, bytes: Vec<u8>, branches: &mut Vec<Branch>| {
                branches.push(Branch {
                    label: label.to_owned(),
                    started: started(&rules, &make, &bytes),
                    playing: playing(&rules, &make, &bytes),
                });
            };
            if !found && after.day == 1 && after.hour == 13 && !moves.is_empty() {
                found = true;
                // The 11:00 save is stepped through the noon that moved the level, and holds the unbroken game's state after it.
                let at_eleven = candidate.take().unwrap();
                take(
                    "11:00 on the first 1st with a move",
                    at_eleven,
                    &mut branches,
                );
                let branch = branches.last_mut().unwrap();
                for _ in 0..2 {
                    branch.started.step().unwrap();
                    branch.playing.step_for_tests();
                }
                assert_eq!(branch.started.digest().unwrap(), unbroken.digest().unwrap());
                assert_eq!(branch.playing.digest(), unbroken.digest().unwrap());
                take(
                    "13:00 on the first 1st with a move",
                    save(&unbroken),
                    &mut branches,
                );
            }
            if found {
                let years_end = (after.month, after.day, after.hour) == (12, 31, 13);
                let jan_eleven = (after.month, after.day, after.hour) == (1, 1, 11);
                let jan_thirteen = (after.month, after.day, after.hour) == (1, 1, 13);
                if years_end && !year_end_done[0] {
                    year_end_done[0] = true;
                    assert!(
                        current
                            .iter()
                            .any(|level| level.3.is_some_and(|(year, _)| year == after.year)),
                        "a level retains a switch stamp from this year"
                    );
                    take("31 December", save(&unbroken), &mut branches);
                } else if jan_eleven && year_end_done[0] && !year_end_done[1] {
                    year_end_done[1] = true;
                    take("11:00 on 1 January", save(&unbroken), &mut branches);
                } else if jan_thirteen && year_end_done[1] && !year_end_done[2] {
                    year_end_done[2] = true;
                    take("13:00 on 1 January", save(&unbroken), &mut branches);
                }
            }
        }
        // The branches took their saves at the named points, each one on a different tick: no branch is the unbroken game.
        let labels: Vec<_> = branches.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "11:00 on the first 1st with a move",
                "13:00 on the first 1st with a move",
                "31 December",
                "11:00 on 1 January",
                "13:00 on 1 January"
            ]
        );
        // The replay moved levels in more than one year.
        assert!(moves.len() >= 5, "{moves:?}");
        let years: std::collections::BTreeSet<_> = moves.iter().map(|m| m.0.year).collect();
        assert!(years.len() >= 2, "{years:?}");
        // Both productive buildings moved levels, so the compare tells them apart.
        let buildings: std::collections::BTreeSet<_> = moves.iter().map(|m| m.1).collect();
        assert_eq!(buildings.len(), 2, "{moves:?}");
        // Each loaded game holds the unbroken game's levels (method and lock) at the end, as the compare each tick implies.
        for branch in &branches {
            assert_eq!(methods(branch.started.state()), methods(unbroken.state()));
            assert_eq!(methods(branch.playing.state()), methods(unbroken.state()));
        }
        // No level moved twice in a calendar month.
        let mut seen = std::collections::BTreeSet::new();
        for (date, building, level, _, _) in &moves {
            assert!(seen.insert((date.year, date.month, *building, *level)));
        }
    }

    /// CK-431: a simple barge ordered by the peasants at 08:00 on 31 January, built by their own three hands in the twelve hours to 20:00,
    /// across the day's noon and then the payday of 1 February: saved before the noon, after it and the next morning, and each loaded
    /// both ways in lockstep with the unbroken game.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "One replay is saved at three named points, each loaded both ways and compared tick by tick."
    )]
    fn a_build_under_way_saved_and_loaded_follows_the_unbroken_game_through_noon_and_payday() {
        let rules = Arc::new(rules());
        let make = scheduler_source(&rules);
        let start = Date {
            year: 768,
            month: 1,
            day: 31,
            hour: 8,
        };
        let start_hour = GameTime::from_date(start).unwrap();
        let mut economy = fort(&rules);
        let good = |id: &str| rules.goods.iter().position(|g| g.id == id).unwrap();
        assert_eq!(economy.places[0].pops[0].kind, "peasant");
        economy.places[0].pops[0].stock[good("hardwood")] += 8;
        economy.places[0].pops[0].stock[good("hides")] += 1;
        economy
            .order_build(&rules, 0, 0, "simple_barge", start_hour.hours())
            .unwrap();
        assert_eq!(economy.places[0].trade.jobs[0].need_goods[good("hides")], 1);
        let mut unbroken = Simulation::new(
            start_hour,
            42,
            GameState {
                economy,
                characters: ck_characters::CharacterStore::default(),
                stretches: BTreeMap::new(),
                lows: crate::game::lows::Lows::default(),
            },
            make(),
            apply,
        );
        let save = |game: &Simulation<GameState, GameCommand>| {
            game.save(identity("aa").meta(42)).to_bytes().unwrap()
        };
        let end = GameTime::from_date(Date {
            year: 768,
            month: 2,
            day: 2,
            hour: 0,
        })
        .unwrap();
        let mut branches: Vec<Branch> = Vec::new();
        let (mut saw_hands, mut saw_away, mut saw_vehicle) = (false, false, false);
        while unbroken.time() < end {
            let now = unbroken.date().unwrap();
            let named = match (now.month, now.day, now.hour) {
                (1, 31, 11) => Some("11:00, before the noon"),
                (1, 31, 13) => Some("13:00, after the noon"),
                (2, 1, 9) => Some("09:00 the next morning, the build done"),
                _ => None,
            };
            if let Some(label) = named {
                let bytes = save(&unbroken);
                branches.push(Branch {
                    label: label.to_owned(),
                    started: started(&rules, &make, &bytes),
                    playing: playing(&rules, &make, &bytes),
                });
            }
            let errors = unbroken.step().unwrap();
            assert!(errors.is_empty(), "{errors:?}");
            let after = unbroken.date().unwrap();
            for branch in &mut branches {
                let errors = branch.started.step().unwrap();
                assert!(errors.is_empty(), "{errors:?}");
                branch.playing.step_for_tests();
                let digest = unbroken.digest().unwrap();
                assert_eq!(
                    branch.started.digest().unwrap(),
                    digest,
                    "{}: a start from the save, at {after:?}",
                    branch.label
                );
                assert_eq!(
                    branch.playing.digest(),
                    digest,
                    "{}: a load during play, at {after:?}",
                    branch.label
                );
            }
            let trade = &unbroken.state().economy.places[0].trade;
            saw_hands |= trade.jobs.iter().any(|job| job.hands.is_some());
            saw_away |= trade.days_away.values().sum::<u64>() == 3;
            saw_vehicle |= !trade.vehicles.is_empty();
        }
        assert!(saw_hands, "the build started");
        assert!(
            saw_away,
            "the noon counted a day away for each of its three hands"
        );
        assert!(saw_vehicle, "and it ended in a vehicle for the peasants");
        let trade = &unbroken.state().economy.places[0].trade;
        assert!(
            trade.days_away.is_empty(),
            "the payday counted the days and cleared them"
        );
        assert_eq!(trade.vehicles.len(), 1);
        assert_eq!(branches.len(), 3);
        for branch in &branches {
            assert_eq!(
                branch.started.state().economy.places[0].trade,
                *trade,
                "{}",
                branch.label
            );
        }
    }

    /// CK-432 (trade spec 11b.9 rule 35): an expedition of the peasants' ox-cart leaves at 08:00 on 31 January with its goods, is away at that
    /// noon, waits at its destination, trades at the noon of the payday of 1 February, starts back and comes home. It is saved before the
    /// first noon, after it, at the destination and after the trade, and each save is loaded both ways in lockstep with the unbroken game,
    /// through the next noon and the payday.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "One replay is saved at named points, each loaded both ways and compared tick by tick."
    )]
    fn an_expedition_under_way_saved_and_loaded_follows_the_unbroken_game_through_noon_and_payday()
    {
        let rules = Arc::new(rules());
        let make = scheduler_source(&rules);
        let start = Date {
            year: 768,
            month: 1,
            day: 31,
            hour: 8,
        };
        let start_hour = GameTime::from_date(start).unwrap();
        let mut economy = fort(&rules);
        let good = |id: &str| rules.goods.iter().position(|g| g.id == id).unwrap();
        // A second place for the trip to go to, of the same make, and an ox-cart of the peasants' own.
        let mut destination = economy.places[0].clone();
        destination.id = "elsewhere".to_owned();
        destination.trade = ck_economy::PlaceTrade::default();
        economy.places.push(destination);
        economy.trade.vehicles = 1;
        economy.places[0].trade.vehicles.push(ck_economy::Vehicle {
            number: 1,
            kind: u32::try_from(
                rules
                    .vehicles
                    .iter()
                    .position(|v| v.id == "ox_cart")
                    .unwrap(),
            )
            .unwrap(),
            pop: 0,
            free: true,
            materials: None,
        });
        economy.places[0].pops[0].stock[good("oats")] += 20;
        economy.places[0].pops[0].stock[good("barley")] += 10;
        let leg = |hours: u128| ck_economy::Leg {
            mode: ck_economy::Mode::Land,
            terms: Vec::new(),
            fixed: ck_economy::Hours::new(hours.into(), 1u8.into()),
            delays: ck_economy::Delays::Along,
            key: [0; 4],
            along: Vec::new(),
            path: Vec::new(),
        };
        let goods = rules.goods.len();
        let mut plan = ck_economy::Plan {
            destination: 1,
            out: vec![leg(20)],
            back: vec![leg(5)],
            vehicles: vec![1],
            people: vec![(0, 2)],
            cargo: vec![0; goods],
            wanted: vec![0; goods],
            supplies: vec![0; goods],
            arrival: None,
        };
        plan.cargo[good("barley")] = 10;
        plan.supplies[good("oats")] = 8;
        economy
            .order_trade(&rules, 0, 0, plan, start_hour.hours())
            .unwrap();
        let gathering = crate::game::trade_panel::build(&economy, &rules);
        let pop = gathering
            .iter()
            .find(|p| p.place == economy.places[0].id && p.pop == 0)
            .unwrap();
        assert!(
            pop.jobs
                .iter()
                .any(|job| job.label == "trip to elsewhere" && job.state == "gathering")
        );
        assert_eq!(pop.vehicles.len(), 1);
        let mut building = economy.clone();
        building.places[0].pops[0].stock[good("hardwood")] = 0;
        building
            .order_build(&rules, 0, 0, "hand_cart", start_hour.hours())
            .unwrap();
        let views = crate::game::trade_panel::build(&building, &rules);
        let pop = views
            .iter()
            .find(|p| p.place == building.places[0].id && p.pop == 0)
            .unwrap();
        assert!(pop.jobs.iter().any(|job| job.label == "building hand_cart"
            && job.state == "gathering"
            && !job.lacks.is_empty()));
        let mut unbroken = Simulation::new(
            start_hour,
            42,
            GameState {
                economy,
                characters: ck_characters::CharacterStore::default(),
                stretches: BTreeMap::new(),
                lows: crate::game::lows::Lows::default(),
            },
            make(),
            apply,
        );
        let save = |game: &Simulation<GameState, GameCommand>| {
            let mut meta = identity("aa").meta(42);
            meta.layers.expeditions = true;
            game.save(meta).to_bytes().unwrap()
        };
        let end = GameTime::from_date(Date {
            year: 768,
            month: 2,
            day: 2,
            hour: 0,
        })
        .unwrap();
        let mut branches: Vec<Branch> = Vec::new();
        let (mut saw_out, mut saw_waiting, mut saw_home) = (false, false, false);
        while unbroken.time() < end {
            let now = unbroken.date().unwrap();
            let named = match (now.month, now.day, now.hour) {
                (1, 31, 11) => Some("11:00 on the 31st, before the first noon"),
                (1, 31, 13) => Some("13:00 on the 31st, after the first noon"),
                (2, 1, 9) => Some("09:00 on the 1st, waiting at the destination"),
                (2, 1, 13) => Some("13:00 on the 1st, after the trade and the payday"),
                _ => None,
            };
            if let Some(label) = named {
                let bytes = save(&unbroken);
                branches.push(Branch {
                    label: label.to_owned(),
                    started: started(&rules, &make, &bytes),
                    playing: playing(&rules, &make, &bytes),
                });
            }
            let errors = unbroken.step().unwrap();
            assert!(errors.is_empty(), "{errors:?}");
            let after = unbroken.date().unwrap();
            for branch in &mut branches {
                let errors = branch.started.step().unwrap();
                assert!(errors.is_empty(), "{errors:?}");
                branch.playing.step_for_tests();
                assert!(
                    branch.playing.snapshot().layers.expeditions,
                    "{}",
                    branch.label
                );
                let digest = unbroken.digest().unwrap();
                assert_eq!(
                    branch.started.digest().unwrap(),
                    digest,
                    "{}: a start from the save, at {after:?}",
                    branch.label
                );
                assert_eq!(
                    branch.playing.digest(),
                    digest,
                    "{}: a load during play, at {after:?}",
                    branch.label
                );
            }
            let trade = &unbroken.state().economy.places[0].trade;
            if let Some(expedition) = trade.expeditions.first() {
                let panel = crate::game::expedition_panel::build(
                    &unbroken.state().economy,
                    0,
                    expedition,
                    &rules,
                )
                .expect("a live expedition has a panel assignment");
                assert_eq!(panel.destination, "elsewhere");
                assert_eq!(panel.people.iter().map(|group| group.count).sum::<u64>(), 2);
                assert_eq!(panel.vehicles[0].number, 1);
                assert_eq!(panel.people[0].roles.iter().map(|(_, n)| n).sum::<u64>(), 2);
                if expedition.phase == ck_economy::Phase::Waiting {
                    assert_eq!(panel.remaining, 0);
                }
                let views = crate::game::trade_panel::build(&unbroken.state().economy, &rules);
                let pop = views
                    .iter()
                    .find(|p| p.place == panel.home && p.pop == 0)
                    .unwrap();
                assert_eq!(pop.vehicles[0].expedition, Some(expedition.number));
                assert!(pop.jobs.iter().any(|job| job.label == "trip to elsewhere"));
                assert!(panel.cargo.iter().any(|good| good.id == "barley") || expedition.traded);
                saw_out |= expedition.phase == ck_economy::Phase::Outbound;
                saw_waiting |= expedition.phase == ck_economy::Phase::Waiting;
            } else if saw_waiting {
                saw_home = true;
            }
        }
        assert!(saw_out && saw_waiting, "it travelled out and waited");
        assert!(saw_home, "and came home, unloaded");
        let economy = &unbroken.state().economy;
        assert!(economy.places[0].trade.expeditions.is_empty());
        assert!(economy.places[0].trade.vehicles[0].free);
        assert_eq!(branches.len(), 4);
        for branch in &branches {
            // The saved trade state is the same; what the economy derives for display is not saved, so it is not compared.
            let loaded = &branch.started.state().economy;
            assert_eq!(loaded.trade, economy.trade, "{}", branch.label);
            for (place, expected) in loaded.places.iter().zip(&economy.places) {
                assert_eq!(place.trade, expected.trade, "{}", branch.label);
            }
        }
    }

    /// CK-432 (review 3): a head on offer is slaughtered at the due hour's tick, and a saved state's clock is the hour whose systems have
    /// not yet run. A save made at the clock before the due hour and at the due hour itself still holds the head; one made after the
    /// slaughter ran holds its result. Each loads both ways and follows the unbroken game tick by tick.
    #[test]
    fn a_head_on_offer_saved_either_side_of_its_due_hour_loads_both_ways() {
        let mut shipped = rules();
        shipped.held_animal_hours = 3;
        let rules = Arc::new(shipped);
        let make = scheduler_source(&rules);
        let start = Date {
            year: 768,
            month: 1,
            day: 10,
            hour: 8,
        };
        let start_hour = GameTime::from_date(start).unwrap();
        let mut economy = fort(&rules);
        let goods = rules.goods.len();
        let species = rules.herd_species.len();
        let mut origin = vec![0; species];
        origin[0] = 1;
        let mut animals = vec![0; species];
        animals[0] = 1;
        economy.trade.jobs = 1;
        economy.places[0].trade.jobs.push(ck_economy::Job {
            number: 1,
            pop: 0,
            kind: ck_economy::JobKind::Held { origin },
            made: start_hour.hours(),
            need_goods: vec![0; goods],
            need_animals: vec![0; species],
            stock: vec![0; goods],
            animals,
            hands: None,
            given_up: Some(start_hour.hours()),
        });
        let mut unbroken = Simulation::new(
            start_hour,
            42,
            GameState {
                economy,
                characters: ck_characters::CharacterStore::default(),
                stretches: BTreeMap::new(),
                lows: crate::game::lows::Lows::default(),
            },
            make(),
            apply,
        );
        let due = start_hour.hours() + rules.held_animal_hours;
        let mut branches: Vec<Branch> = Vec::new();
        while unbroken.time().hours() < due + 3 {
            let clock = unbroken.time().hours();
            let label = if clock == due - 1 {
                Some("the clock before the due hour")
            } else if clock == due {
                Some("the due hour's clock, its work still to run")
            } else if clock == due + 1 {
                Some("the clock after the slaughter ran")
            } else {
                None
            };
            if let Some(label) = label {
                let bytes = unbroken.save(identity("aa").meta(42)).to_bytes().unwrap();
                branches.push(Branch {
                    label: label.to_owned(),
                    started: started(&rules, &make, &bytes),
                    playing: playing(&rules, &make, &bytes),
                });
                let on_offer = !unbroken.state().economy.places[0]
                    .trade
                    .jobs
                    .iter()
                    .all(|job| job.animals.iter().all(|head| *head == 0));
                assert_eq!(on_offer, clock <= due, "{label}");
            }
            let errors = unbroken.step().unwrap();
            assert!(errors.is_empty(), "{errors:?}");
            for branch in &mut branches {
                let errors = branch.started.step().unwrap();
                assert!(errors.is_empty(), "{errors:?}");
                branch.playing.step_for_tests();
                let digest = unbroken.digest().unwrap();
                assert_eq!(branch.started.digest().unwrap(), digest, "{}", branch.label);
                assert_eq!(branch.playing.digest(), digest, "{}", branch.label);
            }
        }
        assert_eq!(branches.len(), 3);
    }
}
