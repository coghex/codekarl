//! The optional gameplay VM bridge. Its handle is shared between the runtime thread and the
//! simulation thread; waits use report signals and never hold the handle mutex.

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    sync::{Arc, Condvar, Mutex, PoisonError},
    time::{Duration, Instant},
};

use ck_lua::{CallOutcome, ConfigValue, VmError, VmOwner};
use ck_sim::{Date, GameTime, Stream};

const GAMEPLAY_STREAM: &str = "gameplay.lua";
const DEFAULT_DEADLINE: Duration = Duration::from_secs(5);

#[cfg(test)]
type CallHook = dyn Fn(&str, &[Option<ConfigValue>]) + Send + Sync;

pub(crate) type GameplayVmHandle = Arc<Mutex<VmOwner>>;

#[derive(Default)]
struct Signal {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl Signal {
    fn generation(&self) -> u64 {
        *self
            .generation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn wake(&self) {
        let mut generation = self
            .generation
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *generation = generation.wrapping_add(1);
        self.changed.notify_all();
    }

    fn wait_after(&self, generation: u64, within: Duration) -> bool {
        let guard = self
            .generation
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *guard != generation {
            return true;
        }
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, within, |seen| *seen == generation)
            .unwrap_or_else(PoisonError::into_inner);
        *guard != generation
    }
}

#[derive(Default)]
struct StopState {
    phase: StopPhase,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum StopPhase {
    #[default]
    Running,
    Deferred,
    Released,
    Sent,
    Cancelled,
}

struct RandomState {
    seed: u64,
    time: Option<GameTime>,
    stream: Option<Stream>,
    active: bool,
}

/// Shared by the gameplay VM adapter, the game setup and both scheduler callbacks.
pub(crate) struct Gameplay {
    vm: Mutex<Option<GameplayVmHandle>>,
    reports: Signal,
    stop: Mutex<StopState>,
    stop_changed: Condvar,
    random: Mutex<RandomState>,
    terminal: Mutex<Option<String>>,
    deadline: Duration,
    #[cfg(test)]
    call_hook: Mutex<Option<Arc<CallHook>>>,
    #[cfg(test)]
    stop_forwards: AtomicUsize,
}

#[derive(Clone)]
pub(crate) struct GameplayWiring {
    pub(crate) runtime: Arc<Gameplay>,
    pub(crate) config: ConfigValue,
}

pub(crate) fn canonical_config(scenario: &str, start: Date) -> ConfigValue {
    ConfigValue::Table(
        [
            (
                "scenario".to_owned(),
                ConfigValue::String(scenario.to_owned()),
            ),
            (
                "start".to_owned(),
                ConfigValue::Table(
                    [
                        (
                            "year".to_owned(),
                            ConfigValue::Integer(i64::from(start.year)),
                        ),
                        (
                            "month".to_owned(),
                            ConfigValue::Integer(i64::from(start.month)),
                        ),
                        ("day".to_owned(), ConfigValue::Integer(i64::from(start.day))),
                        (
                            "hour".to_owned(),
                            ConfigValue::Integer(i64::from(start.hour)),
                        ),
                    ]
                    .into(),
                ),
            ),
        ]
        .into(),
    )
}

impl Gameplay {
    pub(crate) fn new(seed: u64) -> Arc<Self> {
        Self::with_deadline(seed, DEFAULT_DEADLINE)
    }

    pub(crate) fn with_deadline(seed: u64, deadline: Duration) -> Arc<Self> {
        Arc::new(Self {
            vm: Mutex::new(None),
            reports: Signal::default(),
            stop: Mutex::new(StopState::default()),
            stop_changed: Condvar::new(),
            random: Mutex::new(RandomState {
                seed,
                time: None,
                stream: None,
                active: false,
            }),
            terminal: Mutex::new(None),
            deadline,
            #[cfg(test)]
            call_hook: Mutex::new(None),
            #[cfg(test)]
            stop_forwards: AtomicUsize::new(0),
        })
    }

    #[cfg(test)]
    pub(crate) fn set_call_hook(&self, hook: Arc<CallHook>) {
        *self
            .call_hook
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(hook);
    }

    #[cfg(test)]
    fn observe_call(&self, callback: &str, values: &[Option<ConfigValue>]) {
        if let Some(hook) = self
            .call_hook
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            hook(callback, values);
        }
    }

    #[cfg(test)]
    pub(crate) fn stop_forwards(&self) -> usize {
        self.stop_forwards.load(Ordering::SeqCst)
    }

    fn forward_stop(&self) {
        if let Some(vm) = self.vm() {
            #[cfg(test)]
            self.stop_forwards.fetch_add(1, Ordering::SeqCst);
            let _ = vm
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .request_stop();
        }
    }

    pub(crate) fn attach_vm(&self, vm: GameplayVmHandle) {
        *self.vm.lock().unwrap_or_else(PoisonError::into_inner) = Some(vm);
    }

    pub(crate) fn set_seed(&self, seed: u64) {
        let mut random = self.random.lock().unwrap_or_else(PoisonError::into_inner);
        random.seed = seed;
        random.time = None;
        random.stream = None;
        random.active = false;
    }

    pub(crate) fn vm(&self) -> Option<GameplayVmHandle> {
        self.vm
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn report_wake(&self) {
        self.reports.wake();
    }

    pub(crate) fn report_generation(&self) -> u64 {
        self.reports.generation()
    }

    pub(crate) fn wait_report(&self, generation: u64, within: Duration) -> bool {
        self.reports.wait_after(generation, within)
    }

    /// Schedules one of the registered Lua callbacks. A script error is a deterministic tick error;
    /// a disconnected VM or a missed deadline is a terminal run failure.
    pub(crate) fn call(
        self: &Arc<Self>,
        callback: &'static str,
        date: Date,
        time: GameTime,
    ) -> Result<(), String> {
        let Some(vm) = self.vm() else {
            return Ok(());
        };
        {
            let mut random = self.random.lock().unwrap_or_else(PoisonError::into_inner);
            if random.time != Some(time) {
                random.time = Some(time);
                random.stream = Some(Stream::new(random.seed, GAMEPLAY_STREAM, time));
            }
            random.active = true;
        }
        let args = vec![date_config(date, time)];
        let posted = vm
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .post(callback, args);
        let call = match posted {
            Ok(call) => call,
            Err(error) => {
                self.deactivate_random();
                if matches!(error, VmError::Disconnected { .. } | VmError::State { .. }) {
                    return Err(
                        self.fail_terminal(format!("gameplay.{callback} VM ended: {error}"))
                    );
                }
                return Err(format!("gameplay.{callback}: {error}"));
            }
        };
        let deadline = Instant::now().checked_add(self.deadline);
        loop {
            let generation = self.reports.generation();
            let outcome = {
                let mut owner = vm.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(result) = owner.take_result() {
                    if result.id == call {
                        Some(Ok(result.outcome))
                    } else {
                        Some(Err(format!(
                            "unexpected gameplay call result {}",
                            result.id
                        )))
                    }
                } else {
                    owner.take_failure().map(|failure| Err(failure.to_string()))
                }
            };
            if let Some(outcome) = outcome {
                self.deactivate_random();
                return match outcome {
                    Ok(CallOutcome::Returned(values)) => {
                        #[cfg(test)]
                        self.observe_call(callback, &values);
                        #[cfg(not(test))]
                        let _ = values;
                        Ok(())
                    }
                    Ok(CallOutcome::NotRegistered) => Ok(()),
                    Ok(CallOutcome::Failed { message, traceback }) => {
                        Err(format!("gameplay.{callback}: {message}\n{traceback}"))
                    }
                    Err(message) => {
                        Err(self.fail_terminal(format!("gameplay.{callback} VM ended: {message}")))
                    }
                };
            }
            let Some(left) = deadline.and_then(|end| end.checked_duration_since(Instant::now()))
            else {
                self.deactivate_random();
                return Err(self.fail_terminal(format!(
                    "gameplay.{callback} missed its {} s deadline",
                    self.deadline.as_secs_f64()
                )));
            };
            if !self.reports.wait_after(generation, left) {
                self.deactivate_random();
                return Err(self.fail_terminal(format!(
                    "gameplay.{callback} missed its {} s deadline",
                    self.deadline.as_secs_f64()
                )));
            }
        }
    }

    fn fail_terminal(&self, message: String) -> String {
        let mut terminal = self.terminal.lock().unwrap_or_else(PoisonError::into_inner);
        if terminal.is_none() {
            *terminal = Some(message.clone());
        }
        terminal.clone().unwrap_or(message)
    }

    pub(crate) fn take_terminal_failure(&self) -> Option<String> {
        self.terminal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    fn deactivate_random(&self) {
        self.random
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active = false;
    }

    pub(crate) fn random(&self, args: &[Option<ConfigValue>]) -> Result<Vec<ConfigValue>, String> {
        let [Some(min), Some(max)] = args else {
            return Err("sim.random expects two integer bounds".to_owned());
        };
        let min = integer(min).ok_or_else(|| "sim.random bound must be an integer".to_owned())?;
        let max = integer(max).ok_or_else(|| "sim.random bound must be an integer".to_owned())?;
        if min > max {
            return Err("the range is empty".to_owned());
        }
        let mut state = self.random.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.active {
            return Err("sim.random is available only in on_day and on_month".to_owned());
        }
        let stream = state
            .stream
            .as_mut()
            .ok_or_else(|| "sim.random has no active tick".to_owned())?;
        let value = stream
            .range_i64(min, max)
            .map_err(|error| error.to_string())?;
        Ok(vec![ConfigValue::Integer(value)])
    }

    /// Called by the runtime VM adapter. Returns true when the VM should receive `request_stop` now.
    pub(crate) fn request_stop(&self) -> bool {
        let mut state = self.stop.lock().unwrap_or_else(PoisonError::into_inner);
        let send = match state.phase {
            StopPhase::Running => {
                state.phase = StopPhase::Deferred;
                false
            }
            StopPhase::Released => {
                state.phase = StopPhase::Sent;
                true
            }
            StopPhase::Deferred | StopPhase::Sent | StopPhase::Cancelled => false,
        };
        self.stop_changed.notify_all();
        send
    }

    /// The simulation thread calls this as its final act. It forwards a deferred stop exactly once.
    pub(crate) fn release_stop(&self) -> bool {
        let mut state = self.stop.lock().unwrap_or_else(PoisonError::into_inner);
        let send = match state.phase {
            StopPhase::Running => {
                state.phase = StopPhase::Released;
                false
            }
            StopPhase::Deferred => {
                state.phase = StopPhase::Sent;
                true
            }
            StopPhase::Released | StopPhase::Sent | StopPhase::Cancelled => false,
        };
        self.stop_changed.notify_all();
        send
    }

    pub(crate) fn simulation_ended(&self) {
        if self.release_stop() {
            self.forward_stop();
        }
    }

    pub(crate) fn send_deferred_stop(&self) {
        self.forward_stop();
    }

    pub(crate) fn cancel_vm(&self) {
        let mut state = self.stop.lock().unwrap_or_else(PoisonError::into_inner);
        state.phase = StopPhase::Cancelled;
        self.stop_changed.notify_all();
        drop(state);
        if let Some(vm) = self.vm() {
            vm.lock().unwrap_or_else(PoisonError::into_inner).cancel();
        }
    }

    pub(crate) fn wait_stop_released(&self, within: Duration) -> bool {
        let state = self.stop.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, _) = self
            .stop_changed
            .wait_timeout_while(state, within, |state| {
                !matches!(
                    state.phase,
                    StopPhase::Released | StopPhase::Sent | StopPhase::Cancelled
                )
            })
            .unwrap_or_else(PoisonError::into_inner);
        matches!(
            state.phase,
            StopPhase::Released | StopPhase::Sent | StopPhase::Cancelled
        )
    }
}

fn integer(value: &ConfigValue) -> Option<i64> {
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    match value {
        ConfigValue::Integer(value) => Some(*value),
        ConfigValue::Float(value) if value.fract() == 0.0 && *value >= -LIMIT && *value < LIMIT =>
        {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the finite integral float is inside the signed 64-bit range"
            )]
            Some(*value as i64)
        }
        _ => None,
    }
}

fn date_config(date: Date, time: GameTime) -> ConfigValue {
    ConfigValue::Table(
        [
            (
                "year".to_owned(),
                ConfigValue::Integer(i64::from(date.year)),
            ),
            (
                "month".to_owned(),
                ConfigValue::Integer(i64::from(date.month)),
            ),
            ("day".to_owned(), ConfigValue::Integer(i64::from(date.day))),
            (
                "hour".to_owned(),
                ConfigValue::Integer(i64::from(date.hour)),
            ),
            (
                "tick".to_owned(),
                ConfigValue::Integer(i64::try_from(time.hours()).unwrap_or(i64::MAX)),
            ),
        ]
        .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ck_lua::{Extension, HostFunction, Profile, VmSettings};

    const BOUND: Duration = Duration::from_secs(2);
    const DATE: Date = Date {
        year: 1,
        month: 1,
        day: 1,
        hour: 0,
    };
    const TIME: GameTime = GameTime::from_hours(0);

    type RunningVm = (
        Arc<Gameplay>,
        Arc<Mutex<VmOwner>>,
        Arc<Mutex<Vec<i64>>>,
        crate::testing::Dir,
    );

    fn running_vm(source: &str, deadline: Duration) -> RunningVm {
        let dir = crate::testing::Dir::new();
        let entry = dir.0.join("gameplay.lua");
        std::fs::write(&entry, source).expect("write the gameplay script");
        let gameplay = Gameplay::with_deadline(1234, deadline);
        let draws = Arc::new(Mutex::new(Vec::new()));
        let random_gameplay = Arc::clone(&gameplay);
        let random_draws = Arc::clone(&draws);
        let random: HostFunction = Arc::new(move |args| {
            let values = random_gameplay.random(args)?;
            if let Some(ConfigValue::Integer(value)) = values.first() {
                random_draws
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(*value);
            }
            Ok(values)
        });
        let mut settings = VmSettings::new(
            "gameplay",
            entry,
            ConfigValue::Table(std::collections::BTreeMap::new()),
        );
        settings.profile = Profile::Deterministic;
        settings.call_limit = 1;
        settings.extensions = vec![
            Extension::new("sim")
                .function("random", random)
                .raw_table_is_empty("is_empty")
                .callback("on_month", "on_month")
                .callback("on_day", "on_day"),
        ];
        let reports = Arc::clone(&gameplay);
        settings.on_report = Some(Arc::new(move || reports.report_wake()));
        let vm = Arc::new(Mutex::new(VmOwner::spawn(settings).expect("spawn the VM")));
        gameplay.attach_vm(Arc::clone(&vm));
        {
            let mut owner = vm.lock().unwrap_or_else(PoisonError::into_inner);
            owner.wait_ready(BOUND).expect("the script loads");
            owner.request_start().expect("request startup");
            owner.await_started(BOUND).expect("startup completes");
        }
        (gameplay, vm, draws, dir)
    }

    fn stop_vm(gameplay: &Gameplay, vm: &Mutex<VmOwner>) {
        assert!(
            !gameplay.request_stop(),
            "stop waits for simulation release"
        );
        gameplay.simulation_ended();
        assert_eq!(gameplay.stop_forwards(), 1);
        let mut vm = vm.lock().unwrap_or_else(PoisonError::into_inner);
        vm.await_stopped(BOUND).expect("the VM stops after release");
        assert!(vm.wait_finished(Duration::ZERO));
    }

    #[test]
    fn when_simulation_ends_first_the_later_stop_request_is_nonblocking_and_forwarded_once() {
        let (gameplay, vm, _, _dir) = running_vm("", BOUND);
        gameplay.simulation_ended();
        let began = Instant::now();
        assert!(
            gameplay.request_stop(),
            "the released VM should stop at once"
        );
        assert!(began.elapsed() < Duration::from_millis(100));
        gameplay.send_deferred_stop();
        assert!(
            !gameplay.request_stop(),
            "a repeated request is not forwarded"
        );
        assert_eq!(gameplay.stop_forwards(), 1);
        let mut owner = vm.lock().unwrap_or_else(PoisonError::into_inner);
        owner
            .await_stopped(BOUND)
            .expect("the VM receives its one stop");
        assert!(owner.wait_finished(BOUND));
    }

    #[test]
    fn callbacks_use_the_date_and_share_one_seeded_random_stream_for_the_tick() {
        let source = r"
sim.on_month(function(date)
  assert(date.year == 1 and date.month == 1 and date.day == 1 and date.hour == 0 and date.tick == 0)
  assert(sim.random(1.0, 100.0) >= 1)
  assert(sim.is_empty({}))
  assert(not sim.is_empty({x = true}))
  sim.on_month(nil)
end)
sim.on_day(function(date)
  assert(date.year == 1 and date.month == 1 and date.day == 1 and date.hour == 0 and date.tick == 0)
  assert(sim.random(1, 100) >= 1)
end)
function on_month() error('a global function is not a registered callback') end
";
        let (gameplay, vm, draws, _dir) = running_vm(source, BOUND);
        gameplay
            .call("on_month", DATE, TIME)
            .expect("registered monthly callback runs");
        gameplay
            .call("on_day", DATE, TIME)
            .expect("registered daily callback runs");
        let actual = draws.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let mut expected = Stream::new(1234, GAMEPLAY_STREAM, TIME);
        assert_eq!(
            actual,
            [
                expected.range_i64(1, 100).expect("first draw"),
                expected.range_i64(1, 100).expect("second draw"),
            ]
        );
        gameplay
            .call("on_month", DATE, TIME)
            .expect("a global function without sim registration is skipped");
        assert_eq!(
            draws.lock().unwrap_or_else(PoisonError::into_inner).len(),
            2
        );
        stop_vm(&gameplay, &vm);
    }

    #[test]
    fn random_refuses_invalid_bounds_and_calls_outside_a_tick() {
        let gameplay = Gameplay::new(1);
        let integer = Some(ConfigValue::Integer(1));
        assert_eq!(
            gameplay.random(&[integer.clone(), integer.clone()]),
            Err("sim.random is available only in on_day and on_month".to_owned())
        );
        assert_eq!(
            gameplay.random(&[Some(ConfigValue::Float(1.5)), integer.clone()]),
            Err("sim.random bound must be an integer".to_owned())
        );
        assert_eq!(
            gameplay.random(&[Some(ConfigValue::Integer(2)), integer]),
            Err("the range is empty".to_owned())
        );
    }

    #[test]
    fn a_script_error_is_a_tick_error_but_a_missed_deadline_is_terminal() {
        let source = "sim.on_day(function() error({}) end)";
        let (gameplay, vm, _, _dir) = running_vm(source, BOUND);
        let message = gameplay
            .call("on_day", DATE, TIME)
            .expect_err("a script error ends this tick");
        assert!(
            message.contains("(error object is a table value)"),
            "{message}"
        );
        assert!(message.contains("gameplay.lua:1:"), "{message}");
        assert_eq!(gameplay.take_terminal_failure(), None);
        stop_vm(&gameplay, &vm);

        let source = "sim.on_day(function() while true do end end)";
        let (gameplay, vm, _, _dir) = running_vm(source, Duration::from_millis(20));
        let message = gameplay
            .call("on_day", DATE, TIME)
            .expect_err("the runaway callback misses its deadline");
        assert!(
            message.contains("gameplay.on_day missed its 0.02 s deadline"),
            "{message}"
        );
        assert_eq!(gameplay.take_terminal_failure(), Some(message));
        gameplay.cancel_vm();
        assert!(
            vm.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .wait_finished(BOUND)
        );
    }

    #[test]
    fn config_contains_only_scenario_and_start() {
        let config = canonical_config(
            "ireland_768",
            Date {
                year: 768,
                month: 10,
                day: 9,
                hour: 0,
            },
        );
        let ConfigValue::Table(config) = config else {
            panic!("canonical config is a table");
        };
        assert_eq!(config.len(), 2);
        assert_eq!(
            config.get("scenario"),
            Some(&ConfigValue::String("ireland_768".to_owned()))
        );
        let Some(ConfigValue::Table(start)) = config.get("start") else {
            panic!("start is a table");
        };
        assert_eq!(start.len(), 4);
        assert_eq!(start.get("year"), Some(&ConfigValue::Integer(768)));
        assert_eq!(start.get("month"), Some(&ConfigValue::Integer(10)));
        assert_eq!(start.get("day"), Some(&ConfigValue::Integer(9)));
        assert_eq!(start.get("hour"), Some(&ConfigValue::Integer(0)));
    }
}
