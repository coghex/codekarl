//! The simulation in the application, windowless: the core under a fake time, saves, the controls and the owner. No test
//! sleeps; the core is given a time and never reads a clock.

use std::{
    error::Error,
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use ck_log::Level;
use ck_sim::{Cadence, Canonical, CanonicalError, Date, GameTime, Reader, Scheduler, Simulation};

use super::{
    GameCommand,
    controls::Control,
    core::{Core, Flow, Message, Outputs, Rebuild, ViewBuilder},
    saves::Identity,
    slot::SimSnapshot,
};
use crate::testing::Dir;

mod core_behaviour;
mod gameplay_runtime;
mod owner;
mod presses;
mod run;
mod setup;
mod storage;
mod views;

/// A state the test systems write: three counters folded from the random streams, and a flag a system may raise.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct World {
    pub(crate) hourly: u64,
    pub(crate) daily: u64,
    pub(crate) monthly: u64,
    pub(crate) ended: Option<String>,
}

impl Canonical for World {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.hourly.encode(out)?;
        self.daily.encode(out)?;
        self.monthly.encode(out)?;
        self.ended.encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            hourly: u64::decode(input)?,
            daily: u64::decode(input)?,
            monthly: u64::decode(input)?,
            ended: Option::decode(input)?,
        })
    }
}

/// What the test systems may do wrong, at a given hour count.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Faults {
    /// A system returns an error at this tick.
    pub(crate) error_at: Option<u64>,
    /// A system raises the terminal flag at this tick.
    pub(crate) end_at: Option<u64>,
}

/// The test systems: an hourly, a daily and a monthly one, each drawing from its own stream and folding the draw into the
/// state, and two that misbehave when asked to.
pub(crate) fn scheduler(faults: Faults) -> Scheduler<World> {
    let mut scheduler = Scheduler::<World>::new();
    scheduler
        .register_with_streams(
            "hourly",
            Cadence::Hourly,
            &["test.hourly"],
            |world, tick| {
                let draw = tick.stream("test.hourly")?.range(0, 99)?;
                world.hourly = world.hourly.wrapping_mul(31).wrapping_add(draw);
                Ok(())
            },
        )
        .unwrap();
    scheduler
        .register_with_streams(
            "daily",
            Cadence::Daily { hour: 0 },
            &["test.daily"],
            |world, tick| {
                let draw = tick.stream("test.daily")?.draw();
                world.daily = world.daily.rotate_left(5) ^ draw;
                Ok(())
            },
        )
        .unwrap();
    scheduler
        .register_with_streams(
            "monthly",
            Cadence::Monthly { hour: 0 },
            &["test.monthly"],
            |world, tick| {
                let draw = tick.stream("test.monthly")?.range_i64(-50, 50)?;
                world.monthly = world.monthly.wrapping_add(draw.cast_unsigned());
                Ok(())
            },
        )
        .unwrap();
    scheduler
        .register("flaky", Cadence::Hourly, move |_, tick| {
            if faults.error_at == Some(tick.time().hours()) {
                return Err(Box::new(Flaw));
            }
            Ok(())
        })
        .unwrap();
    scheduler
        .register("ender", Cadence::Hourly, move |world, tick| {
            if faults.end_at == Some(tick.time().hours()) {
                world.ended = Some("the gameplay VM ended".to_owned());
            }
            Ok(())
        })
        .unwrap();
    scheduler
}

#[derive(Debug)]
struct Flaw;

impl fmt::Display for Flaw {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a test flaw")
    }
}

impl Error for Flaw {}

/// The test command type is `GameCommand`, which has no values: the core never submits commands in this slice.
pub(crate) fn apply_nothing(_: &mut World, command: GameCommand) {
    match command {}
}

pub(super) fn date(year: u16, month: u8, day: u8, hour: u8) -> Date {
    Date {
        year,
        month,
        day,
        hour,
    }
}

/// 0768-01-01 00:00, the start of the test game.
pub(crate) fn start() -> GameTime {
    GameTime::from_date(date(768, 1, 1, 0)).unwrap()
}

/// The tick `days` days and `hours` hours after the start.
pub(super) fn after(days: u64, hours: u64) -> GameTime {
    GameTime::from_hours(start().hours() + days * 24 + hours)
}

pub(super) fn identity(scenario_hash: &str) -> Identity {
    Identity {
        build: "test-build".to_owned(),
        scenario: "test".to_owned(),
        scenario_hash: scenario_hash.to_owned(),
        map_hash: "bb".to_owned(),
        scripts_hash: "cc".to_owned(),
        rules_hash: "dd".to_owned(),
        start: date(768, 1, 1, 0),
    }
}

type Records = Arc<Mutex<Vec<(Level, String)>>>;

/// A core under a fake time, with its records, its published snapshots and its terminal failures collected.
pub(super) struct Harness {
    pub(super) core: Core<World, GameCommand>,
    records: Records,
    wakes: Arc<Mutex<Vec<SimSnapshot>>>,
    failures: Arc<Mutex<Vec<String>>>,
    pub(super) now: Duration,
    pub(super) saves: PathBuf,
    _dir: Dir,
}

fn locked<T>(shared: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Harness {
    /// A game at the test start under `seed`.
    pub(super) fn new(seed: u64) -> Self {
        Self::build(seed, start(), Faults::default(), identity("aa"), None, None)
    }

    /// A game at the test start whose core builds the economy view with `views`.
    pub(super) fn with_views(seed: u64, views: ViewBuilder<World>) -> Self {
        Self::build(
            seed,
            start(),
            Faults::default(),
            identity("aa"),
            None,
            Some(views),
        )
    }

    pub(super) fn with_faults(seed: u64, faults: Faults) -> Self {
        Self::build(seed, start(), faults, identity("aa"), None, None)
    }

    pub(super) fn starting_at(seed: u64, time: GameTime) -> Self {
        Self::build(seed, time, Faults::default(), identity("aa"), None, None)
    }

    /// A game with another identity, sharing the saves directory of `other`.
    pub(super) fn with_identity_in(seed: u64, identity: Identity, other: &Self) -> Self {
        Self::build(
            seed,
            start(),
            Faults::default(),
            identity,
            Some(other.saves.clone()),
            None,
        )
    }

    fn build(
        seed: u64,
        time: GameTime,
        faults: Faults,
        identity: Identity,
        saves: Option<PathBuf>,
        views: Option<ViewBuilder<World>>,
    ) -> Self {
        let dir = Dir::new();
        let saves = saves.unwrap_or_else(|| dir.0.join("saves"));
        let records = Records::default();
        let wakes = Arc::new(Mutex::new(Vec::new()));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let outputs = Outputs {
            log: {
                let records = Arc::clone(&records);
                Box::new(move |level, message| locked(&records).push((level, message)))
            },
            publish: {
                let wakes = Arc::clone(&wakes);
                Box::new(move |snapshot| locked(&wakes).push(snapshot))
            },
            fail: {
                let failures = Arc::clone(&failures);
                Box::new(move |message| locked(&failures).push(message))
            },
        };
        let simulation = Simulation::new(
            time,
            seed,
            World::default(),
            scheduler(faults),
            apply_nothing,
        );
        let core = Core::new_with_views(
            simulation,
            identity,
            saves.clone(),
            Rebuild {
                scheduler: Box::new(move || scheduler(faults)),
                reset: Box::new(|_| {}),
                apply: apply_nothing,
            },
            outputs,
            views,
        )
        .with_failure_probe(|world: &World| world.ended.clone());
        Self {
            core,
            records,
            wakes,
            failures,
            now: Duration::ZERO,
            saves,
            _dir: dir,
        }
    }

    pub(super) fn control(&mut self, control: Control) {
        self.core.dispatch(Message::Control(control), self.now);
    }

    /// Lets `by` of fake time pass and runs the wake, with no control arriving during it.
    pub(super) fn wake(&mut self, by: Duration) -> Flow {
        self.now += by;
        self.core.run_wake(self.now, &mut || None)
    }

    /// Runs at `level` from now on, as the player resuming it would.
    pub(super) fn run_at(&mut self, level: u8) {
        self.control(Control::Speed(level));
        if self.core.snapshot().paused.is_some() {
            self.control(Control::TogglePause);
        }
    }

    pub(super) fn paused(&self) -> Option<super::slot::PauseReason> {
        self.core.snapshot().paused
    }

    /// The records written so far, as `(level, text)`.
    pub(super) fn records(&self) -> Vec<(Level, String)> {
        locked(&self.records).clone()
    }

    /// The text of the records at `level`.
    pub(super) fn texts(&self, level: Level) -> Vec<String> {
        self.records()
            .into_iter()
            .filter(|(found, _)| *found == level)
            .map(|(_, text)| text)
            .collect()
    }

    /// How many times the main thread would have been woken.
    pub(super) fn wake_count(&self) -> usize {
        locked(&self.wakes).len()
    }

    /// The snapshots published so far, in order.
    pub(super) fn published(&self) -> Vec<SimSnapshot> {
        locked(&self.wakes).clone()
    }

    pub(super) fn failures(&self) -> Vec<String> {
        locked(&self.failures).clone()
    }

    pub(super) fn digest(&self) -> u64 {
        self.core.digest()
    }

    /// Runs as the real-time pattern `wake` (in milliseconds, cycled) at `level` until the game has reached `target`.
    pub(super) fn run_until(&mut self, level: u8, wakes: &[u64], target: GameTime) {
        self.run_at(level);
        let mut index = 0;
        while self.core.time() < target {
            assert!(index < 1_000_000, "the run does not reach its target");
            self.wake(Duration::from_millis(wakes[index % wakes.len()]));
            index += 1;
        }
    }

    /// A fresh game at the same start and seed, run one tick per fake second until it reaches exactly `target`.
    pub(super) fn reference_digest(seed: u64, target: GameTime) -> u64 {
        let mut reference = Self::new(seed);
        reference.run_at(1);
        while reference.core.time() < target {
            reference.wake(Duration::from_secs(1));
        }
        assert_eq!(reference.core.time(), target);
        reference.digest()
    }
}

/// Writes a scenario into `directory`, which must be `<root>/scenarios/<name>`: `scenario.toml` naming the map `fixture`, an
/// empty `locations.toml`, the application's fixture map in `<root>/map/fixture/` and the shipped `font.toml` in
/// `<root>/ui/font/` and the shipped `theme.toml` in `<root>/ui/` (a run with a scenario reads both). The scenario starts on `start`.
pub(crate) fn write_scenario(directory: &std::path::Path, start: Date) {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/map");
    let root = directory
        .parent()
        .and_then(std::path::Path::parent)
        .expect("a scenario directory sits two levels below its assets root");
    let map = root.join("map/fixture");
    std::fs::create_dir_all(&map).unwrap();
    std::fs::create_dir_all(directory).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), map.join(entry.file_name())).unwrap();
    }
    std::fs::write(
        directory.join("scenario.toml"),
        format!(
            "id = \"fixture_768\"\nmap = \"fixture\"\nlocations = \"locations.toml\"\n\
             [start]\nyear = {}\nmonth = {}\nday = {}\nhour = {}\n",
            start.year, start.month, start.day, start.hour
        ),
    )
    .unwrap();
    std::fs::write(directory.join("locations.toml"), "").unwrap();
    // A run with a scenario reads the pixel font from the assets root at start-up.
    let font = root.join("ui/font");
    std::fs::create_dir_all(&font).unwrap();
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui/font/font.toml"),
        font.join("font.toml"),
    )
    .unwrap();
    // ... and the theme (`ui/theme.toml`), under the same rule.
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui/theme.toml"),
        root.join("ui/theme.toml"),
    )
    .unwrap();
}

/// Waits, with a bound, for something another thread does: the wake callback notifies, the test checks its condition.
#[derive(Default)]
pub(crate) struct Waiter {
    notified: Mutex<u64>,
    changed: std::sync::Condvar,
}

impl Waiter {
    pub(crate) fn notify(&self) {
        *locked(&self.notified) += 1;
        self.changed.notify_all();
    }

    /// Waits until `condition` holds, at most ten seconds (a bound that only a bug reaches).
    pub(crate) fn wait_for(&self, mut condition: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut guard = locked(&self.notified);
        while !condition() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(!left.is_zero(), "the condition was not reached in time");
            guard = self
                .changed
                .wait_timeout(guard, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// A simulation owner over the test world, started with `preload` already in its control channel.
pub(crate) fn spawn_test_owner(
    faults: Faults,
    waiter: &Arc<Waiter>,
    preload: Vec<Message>,
) -> (
    super::owner::SimOwner,
    std::sync::mpsc::Sender<Message>,
    Arc<super::slot::SimSlot>,
) {
    let (sender, receiver) = super::owner::channel();
    for message in preload {
        sender.send(message).unwrap();
    }
    let waking = Arc::clone(waiter);
    let slot = super::slot::SimSlot::new(Arc::new(move || waking.notify()));
    let publishing = Arc::clone(&slot);
    let failing = Arc::clone(&slot);
    let build = move || {
        Core::new(
            Simulation::new(
                start(),
                42,
                World::default(),
                scheduler(faults),
                apply_nothing,
            ),
            identity("aa"),
            std::env::temp_dir().join("codekarl-never-written"),
            Rebuild {
                scheduler: Box::new(move || scheduler(faults)),
                reset: Box::new(|_| {}),
                apply: apply_nothing,
            },
            Outputs {
                log: Box::new(|_, _| {}),
                publish: Box::new(move |snapshot| publishing.publish(snapshot)),
                fail: Box::new(move |message| failing.fail(message)),
            },
        )
        .with_failure_probe(|world: &World| world.ended.clone())
    };
    let owner =
        super::owner::SimOwner::spawn(sender.clone(), receiver, Arc::clone(&slot), build).unwrap();
    (owner, sender, slot)
}
