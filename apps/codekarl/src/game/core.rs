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
    controls::Control,
    saves::{self, Entry, Fit, Identity, quicksave_path, write_atomically, write_new_atomically},
    slot::{PauseReason, SimSnapshot},
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
    /// A named-save API request; the reply carries only the completed operation outcome.
    Save(SaveCall, Sender<SaveReply>),
    /// Finish the tick that is running, if any, and end.
    Stop,
    /// End without running another tick.
    Cancel,
}

impl PartialEq for Message {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Control(left), Self::Control(right)) => left == right,
            (Self::Save(left, _), Self::Save(right, _)) => left == right,
            (Self::Stop, Self::Stop) | (Self::Cancel, Self::Cancel) => true,
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

/// The game: a [`Simulation`], its clock, why it is paused and the means to save and load it.
pub(crate) struct Core<S, C> {
    simulation: Simulation<S, C>,
    clock: Clock,
    paused: Option<PauseReason>,
    identity: Identity,
    saves: PathBuf,
    rebuild: Rebuild<S, C>,
    outputs: Outputs,
    published: Option<SimSnapshot>,
    failure_probe: FailureProbe<S>,
    terminal_probe: Box<dyn FnMut() -> Option<String> + Send>,
    failed: bool,
    views: Option<ViewBuilder<S>>,
    /// The revision of `view`; `0` while the core has no view.
    revision: u64,
    view: Option<Arc<EconomyView>>,
}

impl<S, C> Core<S, C>
where
    S: Canonical + Clone + Send + 'static,
    C: Canonical + Send + 'static,
{
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
    pub(crate) fn new_with_views(
        simulation: Simulation<S, C>,
        identity: Identity,
        saves: PathBuf,
        rebuild: Rebuild<S, C>,
        outputs: Outputs,
        views: Option<ViewBuilder<S>>,
    ) -> Self {
        let mut core = Self {
            simulation,
            clock: Clock::new(),
            paused: Some(PauseReason::Load),
            identity,
            saves,
            rebuild,
            outputs,
            published: None,
            failure_probe: Box::new(|_| None),
            terminal_probe: Box::new(|| None),
            failed: false,
            views,
            revision: 0,
            view: None,
        };
        core.refresh_view();
        core.publish_if_changed();
        core
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
            economy_revision: self.revision,
            economy: self.view.clone(),
        }
    }

    /// The state at the current tick boundary, for app-level save/load regression tests.
    #[cfg(test)]
    pub(crate) fn state(&self) -> &S {
        self.simulation.state()
    }

    fn publish_if_changed(&mut self) {
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
            Message::Save(call, reply) => {
                let result = self.save_call(call);
                let _ = reply.send(result);
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

    fn save_with_name(&self, name: String) -> Result<Vec<u8>, String> {
        let mut save = self.save();
        save.meta.name = name;
        save.meta.written = saves::utc_now_rfc3339();
        save.to_bytes().map_err(|error| error.to_string())
    }

    fn save_call(&mut self, call: SaveCall) -> SaveReply {
        let reply = match call {
            SaveCall::List => SaveReply::Listed(saves::list(&self.saves, &self.identity)),
            SaveCall::New(name) => SaveReply::Finished(self.save_as_new(&name)),
            SaveCall::Over(filename) => SaveReply::Finished(self.save_over(&filename)),
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
        let bytes = self.save_with_name(name.clone())?;
        for _ in 0..128 {
            let (filename, path) = saves::new_save_path(&self.saves, &name)?;
            match write_new_atomically(&path, &bytes) {
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
        let bytes = self.save_with_name(entry.name.clone())?;
        write_atomically(&path, &bytes)
            .map_err(|error| format!("cannot replace {filename}: {error}"))?;
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
            Ok(simulation) => {
                (self.rebuild.reset)(simulation.seed());
                self.simulation = simulation;
                self.clock = Clock::new();
                self.paused = Some(PauseReason::Load);
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
            .save()
            .to_bytes()
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                write_atomically(&path, &bytes)
                    .map_err(|error| format!("cannot write {}: {error}", path.display()))
            });
        match result {
            Ok(()) => self.log(Level::Info, format!("saved {} at {date}", path.display())),
            Err(message) => self.log(Level::Error, format!("quicksave failed: {message}")),
        }
    }

    /// Replaces the game with `<saves>/quicksave.cksave` at this boundary, or refuses and keeps the game as it is.
    fn quickload(&mut self) {
        let path = quicksave_path(&self.saves);
        match self.load_from(&path) {
            Ok(simulation) => {
                (self.rebuild.reset)(simulation.seed());
                self.simulation = simulation;
                self.clock = Clock::new();
                self.paused = Some(PauseReason::Load);
                self.refresh_view();
                let date = self.snapshot().date;
                self.log(Level::Info, format!("loaded {} at {date}", path.display()));
            }
            Err(message) => self.log(Level::Error, format!("quickload refused: {message}")),
        }
    }

    fn load_from(&self, path: &std::path::Path) -> Result<Simulation<S, C>, String> {
        let mut file = saves::open_regular(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let save = Save::<S>::from_bytes(&bytes, &self.identity.expected())
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Simulation::from_save(save, (self.rebuild.scheduler)(), self.rebuild.apply)
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    /// The state digest, for tests.
    #[cfg(test)]
    pub(crate) fn digest(&self) -> u64 {
        self.simulation.digest().expect("the state encodes")
    }

    /// The tick that runs next, for tests.
    #[cfg(test)]
    pub(crate) fn time(&self) -> ck_sim::GameTime {
        self.simulation.time()
    }
}
