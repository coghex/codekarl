//! The simulation core: a state machine with no thread and no real clock.
//!
//! It is given a monotonic time and the controls that arrived, runs the ticks that are due and says what to publish. The
//! thread around it (see [`super::owner`]) only waits for the next control or the next tick and reads the time; tests drive
//! the core with a fake time and never sleep.

use std::{
    path::PathBuf,
    sync::{Arc, mpsc::Sender},
    time::Duration,
};

use ck_log::Level;
use ck_sim::{Canonical, Clock, Date, Save, Scheduler, Simulation, Speed};

use super::{
    controls::{Control, GuardedLoadResult, GuardedResultSink},
    saves::{self, Entry, Fit, Identity, quicksave_path},
    slot::{MapLayer, MapLayers, PauseReason, SimSnapshot},
    view::EconomyView,
};

/// The hour of day at which each day's economy systems run (the `economy.daily` cadence). The economy view is built after the
/// tick of this hour, when all of the day's systems have run.
pub(crate) const ECONOMY_HOUR: u8 = 12;

/// What reaches the simulation owner over its control channel.
#[derive(Debug)]
pub(crate) enum Message {
    /// A control the player asked for.
    Control(Control),
    /// A UI choice that counts as an unsaved change.
    MarkUnsaved,
    /// Atomically set a view layer and mark a change unsaved.
    SetMapLayer(MapLayer, bool),
    SetLogChoices(ck_sim::LogChoices),
    /// A named-save API request; the reply carries only the completed operation outcome.
    Save(SaveCall, Sender<SaveReply>),
    /// A guarded named load; the result is returned to the UI link asynchronously.
    GuardedLoad {
        filename: String,
        reply: GuardedResultSink,
    },
    /// Finish the tick that is running, if any, and end.
    Stop,
    /// End without running another tick.
    Cancel,
}

impl PartialEq for Message {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Control(left), Self::Control(right)) => left == right,
            (Self::SetMapLayer(a, b), Self::SetMapLayer(c, d)) => a == c && b == d,
            (Self::SetLogChoices(a), Self::SetLogChoices(b)) => a == b,
            (Self::MarkUnsaved, Self::MarkUnsaved)
            | (Self::Stop, Self::Stop)
            | (Self::Cancel, Self::Cancel) => true,
            (Self::Save(left, _), Self::Save(right, _)) => left == right,
            (
                Self::GuardedLoad { filename: left, .. },
                Self::GuardedLoad {
                    filename: right, ..
                },
            ) => left == right,
            _ => false,
        }
    }
}

impl Eq for Message {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SaveCall {
    List,
    New(String),
    Over(String),
    Delete(String),
    Load(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SaveReply {
    Listed(Result<Vec<Entry>, String>),
    Finished(Result<(), String>),
}

/// Whether the owner goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flow {
    /// Keep serving.
    Continue,
    /// End the thread.
    End,
}

/// The last representable date, shown when the calendar has ended.
const LAST_DATE: Date = Date {
    year: ck_sim::LAST_YEAR,
    month: 12,
    day: 31,
    hour: 23,
};

/// The core's way out to the world: log records, snapshots and the terminal failure.
pub(crate) struct Outputs {
    /// Writes one record in category `sim`.
    pub(crate) log: Box<dyn FnMut(Level, String) + Send>,
    /// Publishes a snapshot whose displayed fields changed.
    pub(crate) publish: Box<dyn FnMut(SimSnapshot) + Send>,
    /// Records the terminal failure.
    pub(crate) fail: Box<dyn FnMut(String) + Send>,
}

/// How a quickload builds the replacement game's systems and applies its commands.
pub(crate) struct Rebuild<S, C> {
    /// A new scheduler with the game's systems.
    pub(crate) scheduler: Box<dyn Fn() -> Scheduler<S> + Send>,
    /// Resets external deterministic state to the seed of a successfully loaded simulation.
    pub(crate) reset: Box<dyn Fn(u64) + Send>,
    /// How a command changes the state.
    pub(crate) apply: fn(&mut S, C),
}

/// What a state may tell the core after a tick: a terminal failure of the simulation, which is not a pause.
type FailureProbe<S> = Box<dyn FnMut(&S) -> Option<String> + Send>;

/// How the core builds the immutable economy view of a state (design §6).
pub(crate) type ViewBuilder<S> = Box<dyn Fn(&S, Date) -> EconomyView + Send>;
pub(crate) type WeatherBuilder<S> = Box<dyn Fn(&S, Date) -> super::slot::HourlyWeather + Send>;
type ExpeditionBuilder<S> = Box<dyn Fn(&S) -> Vec<super::expedition_map::ExpeditionMap> + Send>;
type TradeBuilder<S> = Box<dyn Fn(&S) -> Vec<super::trade_panel::PopTrade> + Send>;
pub(super) type HistoryBuilder<S> = Box<dyn Fn(&S, u16, u8) -> super::history::MonthRecord + Send>;

type ExternalStorage<S> = (
    super::character_storage::Storage,
    fn(&std::path::Path, &S) -> std::io::Result<()>,
);

/// The check a loaded state passes before it is installed, with the date of the save's tick.
type StateValidation<S> = Box<dyn Fn(&S, Date) -> Result<(), String> + Send>;

/// The game: a [`Simulation`], its clock, why it is paused and the means to save and load it.
pub(crate) struct Core<S, C> {
    simulation: Simulation<S, C>,
    external: Option<ExternalStorage<S>>,
    state_validation: Option<StateValidation<S>>,
    clock: Clock,
    paused: Option<PauseReason>,
    /// Whether the player's game has changed since its last successful save or load.
    unsaved: bool,
    layers: MapLayers,
    log_choices: ck_sim::LogChoices,
    identity: Identity,
    saves: PathBuf,
    rebuild: Rebuild<S, C>,
    outputs: Outputs,
    published: Option<SimSnapshot>,
    failure_probe: FailureProbe<S>,
    terminal_probe: Box<dyn FnMut() -> Option<String> + Send>,
    failed: bool,
    views: Option<ViewBuilder<S>>,
    weather: Option<WeatherBuilder<S>>,
    expeditions: Option<ExpeditionBuilder<S>>,
    pop_trade: Option<TradeBuilder<S>>,
    history: Option<(super::history::Recorder, HistoryBuilder<S>)>,
    /// The revision of `view`; `0` while the core has no view.
    revision: u64,
    view: Option<Arc<EconomyView>>,
    /// The world the snapshots carry: it rises each time the simulation is replaced by a load.
    world: u64,
    events_source: super::events::Fixture,
    events: Arc<Vec<super::events::Event>>,
    events_revision: u64,
    events_world: u64,
}

impl<S, C> Core<S, C>
where
    S: Canonical + Clone + Send + 'static,
    C: Canonical + Send + 'static,
{
    pub(super) fn with_state_validation(
        mut self,
        validate: impl Fn(&S, Date) -> Result<(), String> + Send + 'static,
    ) -> Self {
        self.state_validation = Some(Box::new(validate));
        self
    }
    pub(super) fn with_character_storage(
        mut self,
        storage: super::character_storage::Storage,
        validate: fn(&std::path::Path, &S) -> std::io::Result<()>,
    ) -> Self {
        self.external = Some((storage, validate));
        self
    }
    fn write_save(
        &mut self,
        path: &std::path::Path,
        name: Option<String>,
        replace: bool,
    ) -> std::io::Result<()> {
        let mut save = self.save();
        save.meta.layers = self.layers;
        save.meta.log = self.log_choices;
        if let Some(name) = name {
            save.meta.name = name;
            save.meta.written = saves::utc_now_rfc3339();
        }
        let trailer = self
            .history
            .as_mut()
            .and_then(|(history, _)| history.trailer());
        saves::write_stream_atomically(path, replace, |file| {
            if let Some((storage, _)) = &self.external {
                let mut archive = std::fs::File::open(storage.path.join("ARCH"))?;
                let mut logs = std::fs::File::open(storage.path.join("LOGS"))?;
                let a = archive.metadata()?.len();
                let l = logs.metadata()?.len();
                save.write_with_sections(
                    file,
                    &mut [(*b"ARCH", a, &mut archive), (*b"LOGS", l, &mut logs)],
                )?;
            } else {
                use std::io::Write;
                file.write_all(&save.to_bytes().map_err(std::io::Error::other)?)?;
            }
            if let Some(trailer) = &trailer {
                use std::io::Write;
                file.write_all(trailer)?;
            }
            Ok(())
        })
    }
    /// A game over `simulation`, paused at speed 1 with the reason `load`. It publishes its first snapshot at once.
    #[cfg(test)]
    pub(crate) fn new(
        simulation: Simulation<S, C>,
        identity: Identity,
        saves: PathBuf,
        rebuild: Rebuild<S, C>,
        outputs: Outputs,
    ) -> Self {
        Self::new_with_views(simulation, identity, saves, rebuild, outputs, None)
    }

    /// Like [`new`](Self::new), with `views` building the economy view that every snapshot carries: the first (revision 1) is
    /// in the first snapshot, the next after each day's systems and after a load.
    #[cfg(test)]
    pub(crate) fn new_with_views(
        simulation: Simulation<S, C>,
        identity: Identity,
        saves: PathBuf,
        rebuild: Rebuild<S, C>,
        outputs: Outputs,
        views: Option<ViewBuilder<S>>,
    ) -> Self {
        Self::new_with_views_and_layers(
            simulation,
            identity,
            saves,
            rebuild,
            outputs,
            views,
            MapLayers::default(),
        )
    }

    /// Builds a test core with its initial saved layer choices.
    #[cfg(test)]
    pub(crate) fn new_with_views_and_layers(
        simulation: Simulation<S, C>,
        identity: Identity,
        saves: PathBuf,
        rebuild: Rebuild<S, C>,
        outputs: Outputs,
        views: Option<ViewBuilder<S>>,
        layers: MapLayers,
    ) -> Self {
        Self::new_with_events(
            simulation,
            identity,
            saves,
            rebuild,
            outputs,
            views,
            None,
            (
                layers,
                super::events::Fixture::default(),
                ck_sim::LogChoices::default(),
            ),
        )
    }

    /// Builds all view data before publishing the first snapshot.
    #[expect(
        clippy::too_many_arguments,
        reason = "the two readbacks have independent publication cadences"
    )]
    pub(crate) fn new_with_events(
        simulation: Simulation<S, C>,
        identity: Identity,
        saves: PathBuf,
        rebuild: Rebuild<S, C>,
        outputs: Outputs,
        views: Option<ViewBuilder<S>>,
        weather: Option<WeatherBuilder<S>>,
        initial: (MapLayers, super::events::Fixture, ck_sim::LogChoices),
    ) -> Self {
        let (layers, events_source, log_choices) = initial;
        let mut core = Self {
            simulation,
            external: None,
            state_validation: None,
            clock: Clock::new(),
            paused: Some(PauseReason::Load),
            unsaved: false,
            layers,
            log_choices,
            identity,
            saves,
            rebuild,
            outputs,
            published: None,
            failure_probe: Box::new(|_| None),
            terminal_probe: Box::new(|| None),
            failed: false,
            views,
            weather,
            expeditions: None,
            pop_trade: None,
            history: None,
            revision: 0,
            view: None,
            world: 0,
            events_source,
            events: Arc::new(Vec::new()),
            events_revision: 0,
            events_world: 0,
        };
        core.refresh_view();
        core.publish_if_changed();
        core
    }

    #[cfg(test)]
    pub(super) fn install_events(
        &mut self,
        source: super::events::Fixture,
        choices: ck_sim::LogChoices,
    ) {
        self.log_choices = choices;
        self.events_source = source;
        self.refresh_events();
        self.publish_if_changed();
    }

    fn refresh_events(&mut self) {
        let rows = self.events_source.at(self.simulation.time());
        if self.events_revision == 0 || self.events_world != self.world || *self.events != rows {
            self.events_revision += 1;
            self.events_world = self.world;
            self.events = Arc::new(rows);
        }
    }

    /// Builds the economy view of the state as it is now, under the next revision. A core without a view builder has none.
    fn refresh_view(&mut self) {
        if let Some(views) = &self.views {
            self.revision += 1;
            let date = self.simulation.date().unwrap_or(LAST_DATE);
            self.view = Some(Arc::new(views(self.simulation.state(), date)));
        }
    }

    /// Lets a test state end the simulation with a terminal failure after a tick.
    #[cfg(test)]
    pub(crate) fn with_failure_probe(
        mut self,
        probe: impl FnMut(&S) -> Option<String> + Send + 'static,
    ) -> Self {
        self.failure_probe = Box::new(probe);
        self
    }

    /// Adds the inexpensive hourly expedition readback to snapshots.
    pub(crate) fn with_expeditions(
        mut self,
        build: impl Fn(&S) -> Vec<super::expedition_map::ExpeditionMap> + Send + 'static,
    ) -> Self {
        self.expeditions = Some(Box::new(build));
        self.publish_if_changed();
        self
    }

    pub(crate) fn with_pop_trade(
        mut self,
        build: impl Fn(&S) -> Vec<super::trade_panel::PopTrade> + Send + 'static,
    ) -> Self {
        self.pop_trade = Some(Box::new(build));
        self.publish_if_changed();
        self
    }

    pub(super) fn with_history(
        mut self,
        history: super::history::Recorder,
        builder: HistoryBuilder<S>,
    ) -> Self {
        history.collect_async();
        self.history = Some((history, builder));
        self
    }

    #[cfg(test)]
    pub(super) fn history_months(&self) -> &[super::history::MonthRecord] {
        self.history
            .as_ref()
            .map_or(&[], |(history, _)| history.months())
    }

    #[cfg(test)]
    pub(super) fn without_history(mut self) -> Self {
        self.history = None;
        self
    }

    /// Lets a host callback mark a missed gameplay-VM deadline as a terminal run failure.
    pub(crate) fn with_terminal_probe(
        mut self,
        probe: impl FnMut() -> Option<String> + Send + 'static,
    ) -> Self {
        self.terminal_probe = Box::new(probe);
        self
    }

    fn log(&mut self, level: Level, message: String) {
        (self.outputs.log)(level, message);
    }

    /// What the UI shows now.
    pub(crate) fn snapshot(&self) -> SimSnapshot {
        SimSnapshot {
            tick: self.simulation.time(),
            date: self.simulation.date().unwrap_or(LAST_DATE),
            speed: self.clock.speed(),
            paused: self.paused,
            unsaved: self.unsaved,
            layers: self.layers,
            log_choices: self.log_choices,
            economy_revision: self.revision,
            economy: self.view.clone(),
            expeditions: Arc::new(
                self.expeditions
                    .as_ref()
                    .filter(|_| self.layers.expeditions)
                    .map_or_else(Vec::new, |build| build(self.simulation.state())),
            ),
            pop_trade: Arc::new(
                self.pop_trade
                    .as_ref()
                    .map_or_else(Vec::new, |build| build(self.simulation.state())),
            ),
            weather: self
                .weather
                .as_ref()
                .map(|build| {
                    build(
                        self.simulation.state(),
                        self.simulation.date().unwrap_or(LAST_DATE),
                    )
                })
                .unwrap_or_default(),
            events: Arc::clone(&self.events),
            events_revision: self.events_revision,
            world: self.world,
        }
    }

    /// The state at the current tick boundary, for app-level save/load regression tests.
    #[cfg(test)]
    pub(crate) fn state(&self) -> &S {
        self.simulation.state()
    }

    #[cfg(test)]
    pub(crate) fn has_unsaved_changes(&self) -> bool {
        self.unsaved
    }

    fn publish_if_changed(&mut self) {
        self.refresh_events();
        let snapshot = self.snapshot();
        if self
            .published
            .as_ref()
            .is_none_or(|published| snapshot.differs_from(published))
        {
            self.published = Some(snapshot.clone());
            // Output only, at `debug`: what the UI will be told, for the tests that watch a real run's log.
            self.log(Level::Debug, snapshot.to_string());
            (self.outputs.publish)(snapshot);
        }
    }

    /// How long the thread may wait for the next control before a tick is due: one tick's real time at the current speed, or
    /// no limit while paused or after the end.
    pub(crate) fn wait_hint(&self) -> Option<Duration> {
        if self.paused.is_some() || self.failed {
            return None;
        }
        let per_second = u64::from(self.clock.speed().hours_per_second());
        Some(Duration::from_nanos(1_000_000_000 / per_second))
    }

    /// Acts on one message at time `now`.
    pub(crate) fn dispatch(&mut self, message: Message, now: Duration) -> Flow {
        match message {
            Message::Control(control) => {
                self.control(control, now);
                if self.failed {
                    Flow::End
                } else {
                    Flow::Continue
                }
            }
            Message::SetMapLayer(layer, shown) => {
                let value = match layer {
                    MapLayer::Rivers => &mut self.layers.rivers,
                    MapLayer::Roads => &mut self.layers.roads,
                    MapLayer::Expeditions => &mut self.layers.expeditions,
                };
                if *value != shown {
                    *value = shown;
                    self.unsaved = true;
                    self.publish_if_changed();
                }
                Flow::Continue
            }
            Message::SetLogChoices(choices) => {
                if self.log_choices != choices {
                    self.log_choices = choices;
                    self.unsaved = true;
                    self.publish_if_changed();
                }
                Flow::Continue
            }
            Message::MarkUnsaved => {
                self.unsaved = true;
                self.publish_if_changed();
                Flow::Continue
            }
            Message::Save(call, reply) => {
                let result = self.save_call(call);
                self.publish_if_changed();
                let _ = reply.send(result);
                if self.failed {
                    Flow::End
                } else {
                    Flow::Continue
                }
            }
            Message::GuardedLoad { filename, reply } => {
                let result = if self.unsaved {
                    Err("unsaved".to_owned())
                } else {
                    let result = self.load_named(&filename);
                    self.publish_if_changed();
                    if let Err(reason) = &result {
                        self.log(Level::Error, format!("guarded load refused: {reason}"));
                    }
                    result
                };
                reply.send(GuardedLoadResult { filename, result });
                if self.failed {
                    Flow::End
                } else {
                    Flow::Continue
                }
            }
            Message::Stop | Message::Cancel => Flow::End,
        }
    }

    fn control(&mut self, control: Control, now: Duration) {
        match control {
            Control::TogglePause => self.toggle_pause(now),
            Control::Pause => self.pause_only(),
            Control::Speed(level) => self.set_level(level),
            Control::Faster => self.set_level(self.clock.speed().level().saturating_add(1)),
            Control::Slower => self.set_level(self.clock.speed().level().saturating_sub(1)),
            Control::Quicksave => self.quicksave(),
            Control::Quickload => self.quickload(),
        }
        self.publish_if_changed();
    }

    /// Pauses a running game with the reason `player`. A paused game is left as it is: its reason (`load`, `error` or `player`) stays.
    fn pause_only(&mut self) {
        if self.paused.is_none() {
            self.clock.pause();
            self.paused = Some(PauseReason::Player);
        }
    }

    fn toggle_pause(&mut self, now: Duration) {
        if self.paused.is_none() {
            self.clock.pause();
            self.paused = Some(PauseReason::Player);
        } else if self.simulation.date().is_err() {
            self.log(
                Level::Warn,
                "the calendar has ended; the game stays paused".to_owned(),
            );
        } else {
            self.clock.resume(now);
            self.paused = None;
        }
    }

    /// Sets the speed to `level` clamped to 1 to 5; a paused game stays paused.
    fn set_level(&mut self, level: u8) {
        let level = level.clamp(1, 5);
        if let Ok(speed) = Speed::new(level) {
            self.clock.set_speed(speed);
        }
    }

    /// Runs the ticks due at `now`, at most the clock's per-wake limit. Controls that arrive meanwhile (`poll`) take effect at
    /// the next tick boundary: a pause stops the wake there, and a stop lets the tick that is running finish.
    pub(crate) fn run_wake(
        &mut self,
        now: Duration,
        poll: &mut dyn FnMut() -> Option<Message>,
    ) -> Flow {
        if self.drain(now, poll) == Flow::End {
            return Flow::End;
        }
        let due = if self.paused.is_none() {
            self.clock.due(now)
        } else {
            0
        };
        for _ in 0..due {
            if self.drain(now, poll) == Flow::End {
                return Flow::End;
            }
            if self.paused.is_some() {
                break;
            }
            self.tick();
            if self.failed {
                return Flow::End;
            }
        }
        Flow::Continue
    }

    fn drain(&mut self, now: Duration, poll: &mut dyn FnMut() -> Option<Message>) -> Flow {
        while let Some(message) = poll() {
            if self.dispatch(message, now) == Flow::End {
                return Flow::End;
            }
        }
        Flow::Continue
    }

    /// One whole tick; it is never cut short.
    fn tick(&mut self) {
        self.unsaved = true;
        let began = self.simulation.date();
        match self.simulation.step() {
            Ok(errors) => {
                if let Some(message) = (self.terminal_probe)() {
                    self.log(Level::Error, format!("error sim: {message}"));
                    self.fail(message);
                    self.publish_if_changed();
                    return;
                }
                // The day's systems have all run: the UI's view of the economy is built from what they left.
                if let Ok(date) = began
                    && date.hour == ECONOMY_HOUR
                {
                    if date.day == 1
                        && let Some((history, build)) = &mut self.history
                    {
                        let (year, month) = if date.month == 1 {
                            (date.year.saturating_sub(1), 12)
                        } else {
                            (date.year, date.month - 1)
                        };
                        if year > 0 {
                            history.record(build(self.simulation.state(), year, month));
                        }
                    }
                    self.refresh_view();
                }
                if let Ok(date) = began
                    && date.hour == 0
                {
                    self.log(
                        Level::Info,
                        format!("{:04}-{:02}-{:02}", date.year, date.month, date.day),
                    );
                }
                if !errors.is_empty() {
                    for error in &errors {
                        self.log(Level::Error, error.to_string());
                    }
                    self.pause_for_error();
                }
                if let Some(message) = (self.failure_probe)(self.simulation.state()) {
                    self.fail(message);
                }
            }
            Err(error) => {
                self.log(Level::Error, format!("{error}"));
                self.pause_for_error();
            }
        }
        self.publish_if_changed();
    }

    fn pause_for_error(&mut self) {
        self.clock.pause();
        self.paused = Some(PauseReason::Error);
    }

    /// A terminal failure: recorded once, never a pause, and the thread ends after the tick.
    fn fail(&mut self, message: String) {
        self.failed = true;
        (self.outputs.fail)(message);
    }

    /// The game's save at the current tick boundary.
    fn save(&self) -> Save<S> {
        self.simulation
            .save(self.identity.meta(self.simulation.seed()))
    }

    fn save_call(&mut self, call: SaveCall) -> SaveReply {
        let reply = match call {
            SaveCall::List => SaveReply::Listed(saves::list(&self.saves, &self.identity)),
            SaveCall::New(name) => {
                let result = self.save_as_new(&name);
                if result.is_ok() {
                    self.unsaved = false;
                }
                SaveReply::Finished(result)
            }
            SaveCall::Over(filename) => {
                let result = self.save_over(&filename);
                if result.is_ok() {
                    self.unsaved = false;
                }
                SaveReply::Finished(result)
            }
            SaveCall::Delete(filename) => SaveReply::Finished(self.delete(&filename)),
            SaveCall::Load(filename) => {
                let result = self.load_named(&filename);
                self.publish_if_changed();
                SaveReply::Finished(result)
            }
        };
        match &reply {
            SaveReply::Listed(Ok(entries)) => {
                self.log(Level::Info, format!("listed {} saves", entries.len()));
            }
            SaveReply::Listed(Err(reason)) | SaveReply::Finished(Err(reason)) => {
                self.log(Level::Error, format!("save call refused: {reason}"));
            }
            SaveReply::Finished(Ok(())) => {}
        }
        reply
    }

    fn save_as_new(&mut self, untrimmed_name: &str) -> Result<(), String> {
        let name = saves::valid_name(untrimmed_name)?;
        for _ in 0..128 {
            let (filename, path) = saves::new_save_path(&self.saves, &name)?;
            match self.write_save(&path, Some(name.clone()), false) {
                Ok(()) => {
                    self.log(Level::Info, format!("saved {name} as {filename}"));
                    return Ok(());
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(format!("cannot publish {filename}: {error}")),
            }
        }
        Err("could not reserve a unique save file name".to_owned())
    }

    fn save_over(&mut self, filename: &str) -> Result<(), String> {
        let path = saves::regular_save(&self.saves, filename)?;
        let entry = saves::list(&self.saves, &self.identity)?
            .into_iter()
            .find(|entry| entry.filename == filename)
            .ok_or_else(|| "not a save".to_owned())?;
        if entry.fit != Fit::Fits {
            return Err(format!(
                "save cannot be overwritten: {}",
                entry.fit.as_str()
            ));
        }
        self.write_save(&path, Some(entry.name.clone()), true)
            .map_err(|error| format!("cannot replace {filename}: {error}"))?;
        if let Some((history, _)) = &self.history {
            history.collect_async();
        }
        self.log(
            Level::Info,
            format!("saved over {} ({})", entry.name, filename),
        );
        Ok(())
    }

    fn delete(&mut self, filename: &str) -> Result<(), String> {
        let path = saves::regular_save(&self.saves, filename)?;
        std::fs::remove_file(&path)
            .map_err(|error| format!("cannot delete {filename}: {error}"))?;
        if let Some((history, _)) = &self.history {
            history.collect_async();
        }
        self.log(Level::Info, format!("deleted {filename}"));
        Ok(())
    }

    fn load_named(&mut self, filename: &str) -> Result<(), String> {
        let path = saves::regular_save(&self.saves, filename)?;
        let entry = saves::list(&self.saves, &self.identity)?
            .into_iter()
            .find(|entry| entry.filename == filename)
            .ok_or_else(|| "not a save".to_owned())?;
        if entry.fit != Fit::Fits {
            return Err(format!("save cannot be loaded: {}", entry.fit.as_str()));
        }
        match self.load_from(&path) {
            Ok((simulation, layers, choices)) => {
                self.layers = layers;
                self.log_choices = choices;
                (self.rebuild.reset)(simulation.seed());
                self.simulation = simulation;
                self.clock = Clock::new();
                self.paused = Some(PauseReason::Load);
                self.unsaved = false;
                self.world += 1;
                self.refresh_view();
                self.log(Level::Info, format!("loaded {} ({filename})", entry.name));
                Ok(())
            }
            Err(message) => Err(message),
        }
    }

    /// Writes `<saves>/quicksave.cksave` and carries on as it was.
    fn quicksave(&mut self) {
        let path = quicksave_path(&self.saves);
        let date = self.snapshot().date;
        let result = self
            .write_save(&path, None, true)
            .map_err(|error| format!("cannot write {}: {error}", path.display()));
        match result {
            Ok(()) => {
                if let Some((history, _)) = &self.history {
                    history.collect_async();
                }
                self.unsaved = false;
                self.log(Level::Info, format!("saved {} at {date}", path.display()));
            }
            Err(message) => self.log(Level::Error, format!("quicksave failed: {message}")),
        }
    }

    /// Replaces the game with `<saves>/quicksave.cksave` at this boundary, or refuses and keeps the game as it is.
    fn quickload(&mut self) {
        let path = quicksave_path(&self.saves);
        match self.load_from(&path) {
            Ok((simulation, layers, choices)) => {
                self.layers = layers;
                self.log_choices = choices;
                (self.rebuild.reset)(simulation.seed());
                self.simulation = simulation;
                self.clock = Clock::new();
                self.paused = Some(PauseReason::Load);
                self.unsaved = false;
                self.world += 1;
                self.refresh_view();
                let date = self.snapshot().date;
                self.log(Level::Info, format!("loaded {} at {date}", path.display()));
            }
            Err(message) => self.log(Level::Error, format!("quickload refused: {message}")),
        }
    }

    fn load_from(
        &mut self,
        path: &std::path::Path,
    ) -> Result<(Simulation<S, C>, MapLayers, ck_sim::LogChoices), String> {
        let mut file = saves::open_regular(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let handoff = if self.history.is_some() {
            use std::io::{Seek, SeekFrom};
            let handoff = super::history::Handoff::begin(&mut file, &self.saves);
            file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
            Some(handoff)
        } else {
            None
        };
        if let Some((storage, validate)) = &self.external {
            let parent = storage.path.parent().ok_or("storage has no parent")?;
            let staging =
                super::character_storage::Storage::fresh_path(parent).map_err(|e| e.to_string())?;
            let (save, _) = Save::<S>::read_with_sections(
                &mut file,
                &self.identity.expected(),
                &staging,
                &[*b"ARCH", *b"LOGS"],
            )
            .map_err(|e| format!("{}: {e}", path.display()))?;
            let staged = super::character_storage::Storage::at(
                staging,
                storage.handle.clone(),
                storage.cell.clone(),
                storage.log_cell.clone(),
            );
            if let Some(validate) = &self.state_validation {
                let date = save.time.date().map_err(|e| e.to_string())?;
                validate(&save.state, date)?;
            }
            validate(&staged.path, &save.state).map_err(|e| e.to_string())?;
            let layers = save.meta.layers;
            let choices = save.meta.log;
            let simulation =
                Simulation::from_save(save, (self.rebuild.scheduler)(), self.rebuild.apply)
                    .map_err(|e| e.to_string())?;
            if let Some(handoff) = handoff {
                let date = simulation.date().map_err(|e| e.to_string())?;
                let mut history = handoff
                    .finish(&mut file, &self.saves, date)
                    .map_err(|e| e.to_string())?;
                if let Some((current, _)) = &mut self.history {
                    history.inherit_start(current);
                    *current = history;
                }
            }
            staged.publish();
            self.external = Some((staged, *validate));
            Ok((simulation, layers, choices))
        } else {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut bytes).map_err(|e| e.to_string())?;
            let save = Save::<S>::from_bytes(&bytes, &self.identity.expected())
                .map_err(|e| e.to_string())?;
            if let Some(validate) = &self.state_validation {
                let date = save.time.date().map_err(|e| e.to_string())?;
                validate(&save.state, date)?;
            }
            let layers = save.meta.layers;
            let choices = save.meta.log;
            Simulation::from_save(save, (self.rebuild.scheduler)(), self.rebuild.apply)
                .map(|simulation| (simulation, layers, choices))
                .map_err(|e| e.to_string())
        }
    }

    /// The state digest, for tests.
    #[cfg(test)]
    pub(crate) fn digest(&self) -> u64 {
        if let Some((storage, _)) = &self.external {
            let mut file = std::fs::File::open(storage.path.join("ARCH")).expect("archive");
            let len = file.metadata().expect("archive metadata").len();
            self.simulation
                .digest_with_section(*b"ARCH", len, &mut file)
                .expect("archive digest")
        } else {
            self.simulation.digest().expect("the state encodes")
        }
    }

    /// The tick that runs next, for tests.
    #[cfg(test)]
    pub(crate) fn time(&self) -> ck_sim::GameTime {
        self.simulation.time()
    }

    /// The date and hour that run next, for tests.
    #[cfg(test)]
    pub(crate) fn date(&self) -> Date {
        self.simulation.date().expect("a date in the calendar")
    }

    /// One whole tick as the game runs it, for tests of a game that runs after a load.
    #[cfg(test)]
    pub(crate) fn step_for_tests(&mut self) {
        self.tick();
    }
}
