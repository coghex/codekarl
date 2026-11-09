use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use ck_log::Level;
use ck_lua::{ConfigValue, Extension, HostFunction, Profile, VmOwner, VmSettings};
use ck_sim::{Cadence, GameTime, Scheduler, Simulation};

use super::{date, identity, start};
use crate::{
    game::{
        GameCommand, GameState,
        controls::Control,
        core::{Core, Outputs, Rebuild},
        gameplay::{Gameplay, canonical_config},
        setup::{register_economy_callback, register_gameplay_lua_callbacks},
    },
    testing::Dir,
    vm::gameplay_extension,
};

const BOUND: Duration = Duration::from_secs(5);
const SECOND: Duration = Duration::from_secs(1);
type CallbackResult = (String, Vec<Option<ConfigValue>>);
type CallResults = Arc<Mutex<Vec<CallbackResult>>>;

fn apply_game(_: &mut GameState, command: GameCommand) {
    match command {}
}

fn locked<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

fn record_extension(lines: Arc<Mutex<Vec<String>>>) -> Extension {
    let record: HostFunction = Arc::new(move |args| {
        let Some(ConfigValue::String(text)) = args.first().and_then(Option::as_ref) else {
            return Err("test.record expects one string".to_owned());
        };
        locked(&lines).push(text.clone());
        Ok(Vec::new())
    });
    Extension::new("test").function("record", record)
}

struct RuntimeHarness {
    core: Core<GameState, GameCommand>,
    now: Duration,
    gameplay: Arc<Gameplay>,
    vm: Arc<Mutex<VmOwner>>,
    lines: Arc<Mutex<Vec<String>>>,
    results: CallResults,
    result_digest: Arc<Mutex<u64>>,
    logs: Arc<Mutex<Vec<(Level, String)>>>,
    failures: Arc<Mutex<Vec<String>>>,
    dir: Dir,
}

impl RuntimeHarness {
    fn new(
        source: &str,
        deadline: Duration,
        host_settings: ConfigValue,
        trace: Option<&Arc<Mutex<Vec<String>>>>,
    ) -> Self {
        Self::with_seed(source, deadline, host_settings, trace, 42)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the test harness wires the real gameplay VM, callbacks, and simulation core together"
    )]
    fn with_seed(
        source: &str,
        deadline: Duration,
        host_settings: ConfigValue,
        trace: Option<&Arc<Mutex<Vec<String>>>>,
        seed: u64,
    ) -> Self {
        let dir = Dir::new();
        let entry = dir.0.join("gameplay.lua");
        std::fs::write(&entry, source).expect("the gameplay script is written");
        let gameplay = Gameplay::with_deadline(seed, deadline);
        let lines = Arc::new(Mutex::new(Vec::new()));
        let results = Arc::new(Mutex::new(Vec::new()));
        let call_results = Arc::clone(&results);
        let result_digest = Arc::new(Mutex::new(0xcbf2_9ce4_8422_2325_u64));
        let call_digest = Arc::clone(&result_digest);
        gameplay.set_call_hook(Arc::new(move |callback, values| {
            locked(&call_results).push((callback.to_owned(), values.to_vec()));
            let record = format!("{callback}:{values:?}\n");
            let mut digest = locked(&call_digest);
            for byte in record.bytes() {
                *digest = (*digest ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
            }
        }));

        let gameplay_config = canonical_config("fixture_768", date(768, 1, 1, 0));
        // Keep host configuration in the fixture and deliberately give the gameplay VM only its
        // reviewed, scenario-derived view, just as setup::prepare does in production.
        let ConfigValue::Table(host_settings) = host_settings else {
            panic!("the varied host settings are a table");
        };
        assert!(host_settings.contains_key("backend"));
        let ConfigValue::Table(gameplay_settings) = &gameplay_config else {
            panic!("the gameplay settings are a table");
        };
        assert!(!gameplay_settings.contains_key("backend"));
        let mut settings = VmSettings::new("gameplay", entry, gameplay_config);
        settings.profile = Profile::Deterministic;
        settings.call_limit = 1;
        settings.extensions = vec![
            gameplay_extension(&gameplay),
            record_extension(Arc::clone(&lines)),
        ];
        if let Some(trace) = &trace {
            let trace = Arc::clone(trace);
            let mark: HostFunction = Arc::new(move |args| {
                let Some(ConfigValue::String(text)) = args.first().and_then(Option::as_ref) else {
                    return Err("test.mark expects one string".to_owned());
                };
                locked(&trace).push(text.clone());
                Ok(Vec::new())
            });
            settings
                .extensions
                .push(Extension::new("test_order").function("mark", mark));
        }
        let report = Arc::clone(&gameplay);
        settings.on_report = Some(Arc::new(move || report.report_wake()));
        let vm = Arc::new(Mutex::new(VmOwner::spawn(settings).expect("the VM starts")));
        gameplay.attach_vm(Arc::clone(&vm));
        {
            let mut owner = locked(&vm);
            owner.wait_ready(BOUND).expect("the gameplay script loads");
            owner.request_start().expect("startup is requested");
            owner.await_started(BOUND).expect("startup completes");
        }

        let logs = Arc::new(Mutex::new(Vec::new()));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let published_logs = Arc::clone(&logs);
        let published_failures = Arc::clone(&failures);
        let economy_rules = Arc::new(
            ck_economy::Rules::load(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../assets/game/economy.toml"),
            )
            .expect("the shipped economy rules load for the runtime witness"),
        );
        let make_scheduler = {
            let runtime = Arc::clone(&gameplay);
            let trace = trace.cloned();
            let rules = Arc::clone(&economy_rules);
            move || {
                let mut scheduler = Scheduler::<GameState>::new();
                register_economy_callback(&mut scheduler, Arc::clone(&rules));
                if let Some(trace_ref) = &trace {
                    let trace = Arc::clone(trace_ref);
                    scheduler
                        .register(
                            "test.economy_complete",
                            Cadence::Daily { hour: 12 },
                            move |_, _| {
                                locked(&trace).push("economy_complete".to_owned());
                                Ok(())
                            },
                        )
                        .expect("the economy order witness is registered after economy");
                    let cadence_trace = Arc::clone(trace_ref);
                    for (name, cadence, label) in [
                        ("test.monthly", Cadence::Daily { hour: 12 }, "rust_monthly"),
                        ("test.daily", Cadence::Daily { hour: 12 }, "rust_daily"),
                        ("test.hourly", Cadence::Hourly, "rust_hourly"),
                    ] {
                        let trace = Arc::clone(&cadence_trace);
                        scheduler
                            .register(name, cadence, move |_, _| {
                                locked(&trace).push(label.to_owned());
                                Ok(())
                            })
                            .expect("test scheduler names are unique");
                    }
                }
                register_gameplay_lua_callbacks(&mut scheduler, &runtime);
                scheduler
            }
        };
        let simulation = Simulation::new(
            start(),
            seed,
            GameState::default(),
            make_scheduler(),
            apply_game,
        );
        let reset_runtime = Arc::clone(&gameplay);
        let core = Core::new(
            simulation,
            identity("aa"),
            dir.0.join("saves"),
            Rebuild {
                scheduler: Box::new(make_scheduler),
                reset: Box::new(move |loaded_seed| reset_runtime.set_seed(loaded_seed)),
                apply: apply_game,
            },
            Outputs {
                log: Box::new(move |level, message| locked(&published_logs).push((level, message))),
                publish: Box::new(|_| {}),
                fail: Box::new(move |message| locked(&published_failures).push(message)),
            },
        )
        .with_terminal_probe({
            let runtime = Arc::clone(&gameplay);
            move || runtime.take_terminal_failure()
        });
        Self {
            core,
            now: Duration::ZERO,
            gameplay,
            vm,
            lines,
            results,
            result_digest,
            logs,
            failures,
            dir,
        }
    }

    fn step(&mut self) {
        self.now += SECOND;
        self.core.run_wake(self.now, &mut || None);
    }

    fn resume(&mut self) {
        self.core.dispatch(
            crate::game::core::Message::Control(Control::Speed(1)),
            self.now,
        );
        self.core.dispatch(
            crate::game::core::Message::Control(Control::TogglePause),
            self.now,
        );
    }

    fn run_until(&mut self, target: GameTime) {
        if self.core.snapshot().paused.is_some() {
            self.resume();
        }
        while self.core.time() < target && self.failures().is_empty() {
            self.step();
        }
        assert_eq!(
            self.core.time(),
            target,
            "the core reaches the requested tick"
        );
    }

    fn run_through(&mut self, target: GameTime) {
        if self.core.snapshot().paused.is_some() {
            self.resume();
        }
        while self.core.time() <= target && self.failures().is_empty() {
            self.step();
        }
        assert!(
            self.core.time() > target,
            "the core ran through the requested tick"
        );
    }

    fn failures(&self) -> Vec<String> {
        locked(&self.failures).clone()
    }

    fn stop(&mut self) {
        if self.gameplay.request_stop() {
            self.gameplay.send_deferred_stop();
        } else {
            self.gameplay.simulation_ended();
        }
        let mut owner = locked(&self.vm);
        owner.await_stopped(BOUND).expect("the VM stops");
    }
}

fn host_settings(backend: &str, width: i64, level: &str) -> ConfigValue {
    ConfigValue::Table(
        [
            (
                "backend".to_owned(),
                ConfigValue::String(backend.to_owned()),
            ),
            ("window".to_owned(), ConfigValue::Integer(width)),
            ("frame".to_owned(), ConfigValue::Integer(width + 1)),
            (
                "log_level".to_owned(),
                ConfigValue::String(level.to_owned()),
            ),
        ]
        .into(),
    )
}

fn start_of_day(days: u64) -> GameTime {
    GameTime::from_hours(start().hours() + days * 24)
}

fn gameplay_source() -> &'static str {
    r"
assert(config.scenario == 'fixture_768' and config.backend == nil and config.window == nil)
local function run(kind, date)
  local values = {}
  local source = {z = 9, b = 2, a = 1, y = 8}
  for key, value in pairs(source) do
    values[#values + 1] = key .. '=' .. value .. ':' .. sim.random(1, 1000000)
  end
  local line = kind .. ' ' .. date.year .. '-' .. date.month .. '-' .. date.day .. ' ' .. table.concat(values, ',')
  log.info(line)
  test.record(line)
  return line
end
sim.on_month(function(date) return run('month', date) end)
sim.on_day(function(date) return run('day', date) end)
"
}

#[test]
fn quickload_restarts_the_per_tick_stream_and_replays_the_saved_noon_draws() {
    let mut run = RuntimeHarness::new(
        gameplay_source(),
        BOUND,
        host_settings("metal", 640, "info"),
        None,
    );
    let eleven = GameTime::from_hours(start().hours() + 11);
    run.run_until(eleven);
    run.core.dispatch(
        crate::game::core::Message::Control(Control::Quicksave),
        run.now,
    );
    run.run_through(GameTime::from_hours(start().hours() + 12));
    let first = locked(&run.results).clone();
    assert_eq!(first.len(), 2, "monthly and daily callbacks ran at noon");
    assert!(first.iter().any(|(name, _)| name == "on_month"));
    assert!(first.iter().any(|(name, _)| name == "on_day"));

    run.core.dispatch(
        crate::game::core::Message::Control(Control::TogglePause),
        run.now,
    );
    run.core.dispatch(
        crate::game::core::Message::Control(Control::Quickload),
        run.now,
    );
    assert_eq!(run.core.time(), eleven);
    locked(&run.results).clear();
    run.resume();
    run.run_through(GameTime::from_hours(start().hours() + 12));
    assert_eq!(
        *locked(&run.results),
        first,
        "the replay uses the reset stream"
    );
    run.stop();
}

#[test]
fn quickload_uses_the_saved_seed_when_the_new_run_has_a_different_seed() {
    let mut seed_a = RuntimeHarness::with_seed(
        gameplay_source(),
        BOUND,
        host_settings("metal", 640, "info"),
        None,
        42,
    );
    let noon = GameTime::from_hours(start().hours() + 12);
    seed_a.run_until(noon);
    seed_a.core.dispatch(
        crate::game::core::Message::Control(Control::Quicksave),
        seed_a.now,
    );
    seed_a.run_through(GameTime::from_hours(start().hours() + 12));
    let expected_draws = locked(&seed_a.results).clone();
    assert_eq!(expected_draws.len(), 2, "seed A ran both noon callbacks");
    let save = std::fs::read(seed_a.dir.0.join("saves/quicksave.cksave"))
        .expect("the seed-A quicksave exists");
    seed_a.stop();

    let mut seed_b = RuntimeHarness::with_seed(
        gameplay_source(),
        BOUND,
        host_settings("metal", 640, "info"),
        None,
        99,
    );
    std::fs::create_dir_all(seed_b.dir.0.join("saves")).unwrap();
    std::fs::write(seed_b.dir.0.join("saves/quicksave.cksave"), save)
        .expect("the seed-A quicksave is copied into run B's save directory");
    seed_b.core.dispatch(
        crate::game::core::Message::Control(Control::Quickload),
        seed_b.now,
    );
    seed_b.resume();
    seed_b.run_through(GameTime::from_hours(start().hours() + 12));
    assert_eq!(
        *locked(&seed_b.results),
        expected_draws,
        "the same save draws from its restored seed regardless of the run seed"
    );
    seed_b.stop();
}

#[test]
fn gameplay_callbacks_follow_all_rust_cadences_at_noon_on_the_first_of_a_month() {
    let trace = Arc::new(Mutex::new(Vec::new()));
    let source = r"
sim.on_month(function() test_order.mark('lua_month') end)
sim.on_day(function() test_order.mark('lua_day') end)
";
    let mut run = RuntimeHarness::new(
        source,
        BOUND,
        host_settings("metal", 640, "info"),
        Some(&trace),
    );
    run.resume();
    run.run_through(GameTime::from_hours(start().hours() + 12));
    let trace = locked(&trace);
    assert_eq!(&trace[..12], ["rust_hourly"; 12]);
    assert_eq!(
        &trace[12..],
        [
            "economy_complete",
            "rust_monthly",
            "rust_daily",
            "lua_month",
            "lua_day",
            "rust_hourly"
        ]
    );
    run.stop();
}

#[test]
fn gameplay_error_pauses_at_the_same_tick_and_logs_its_traceback_in_two_runs() {
    let source = "sim.on_day(function() error('stable gameplay error') end)";
    let mut observed = Vec::new();
    for _ in 0..2 {
        let mut run = RuntimeHarness::new(source, BOUND, host_settings("metal", 640, "info"), None);
        run.resume();
        run.run_through(GameTime::from_hours(start().hours() + 12));
        assert_eq!(
            run.core.snapshot().paused,
            Some(crate::game::slot::PauseReason::Error)
        );
        assert_eq!(run.core.time(), GameTime::from_hours(start().hours() + 13));
        let errors: Vec<_> = locked(&run.logs)
            .iter()
            .filter(|(level, _)| *level == Level::Error)
            .map(|(_, text)| text.clone())
            .collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("stable gameplay error"), "{}", errors[0]);
        assert!(errors[0].contains("gameplay.lua:1:"), "{}", errors[0]);
        observed.push(errors[0].clone());
        run.stop();
    }
    assert_eq!(observed[0], observed[1]);
}

#[test]
fn gameplay_deadline_failure_reaches_the_core_terminal_path_instead_of_pause() {
    let source = "sim.on_day(function() while true do end end)";
    let mut run = RuntimeHarness::new(
        source,
        Duration::from_millis(30),
        host_settings("metal", 640, "info"),
        None,
    );
    run.resume();
    run.run_through(GameTime::from_hours(start().hours() + 12));
    assert_eq!(run.core.snapshot().paused, None);
    assert_eq!(run.failures().len(), 1, "terminal failure is reported once");
    assert!(run.failures()[0].contains("gameplay.on_day missed its 0.03 s deadline"));
    assert_eq!(
        locked(&run.logs)
            .iter()
            .filter(|(level, text)| *level == Level::Error && text.starts_with("error sim:"))
            .count(),
        1
    );
    run.gameplay.cancel_vm();
    assert!(locked(&run.vm).wait_finished(BOUND));
}

#[test]
fn gameplay_skeleton_emits_its_daily_line_with_a_scenario() {
    let skeleton = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/gameplay/main.lua"),
    )
    .expect("the checked-in gameplay skeleton exists");
    let source = format!("log.info = function(line) test.record(line) end\n{skeleton}");
    let mut run = RuntimeHarness::new(&source, BOUND, host_settings("metal", 640, "info"), None);
    run.resume();
    run.run_through(GameTime::from_hours(start().hours() + 12));
    assert!(
        locked(&run.lines)
            .iter()
            .any(|line| line == "day 0768-01-01"),
        "the scenario-backed skeleton logs its first day"
    );
    run.stop();
}

#[test]
fn gameplay_pair_and_random_returns_match_across_host_settings_and_day_37_quickload() {
    let stop = start_of_day(400);
    let first_host = host_settings("metal", 640, "info");
    let second_host = host_settings("vulkan", 1600, "debug");
    let mut first = RuntimeHarness::new(gameplay_source(), BOUND, first_host, None);
    first.run_until(stop);
    let first_digest = *locked(&first.result_digest);
    let first_lines = locked(&first.lines).clone();
    first.stop();

    let mut continued = RuntimeHarness::new(gameplay_source(), BOUND, second_host, None);
    continued.run_until(start_of_day(37));
    let saved_digest = *locked(&continued.result_digest);
    let saved_lines = locked(&continued.lines).clone();
    continued.core.dispatch(
        crate::game::core::Message::Control(Control::Quicksave),
        continued.now,
    );
    continued.run_until(stop);
    let uninterrupted_digest = *locked(&continued.result_digest);
    let uninterrupted_lines = locked(&continued.lines).clone();

    continued.core.dispatch(
        crate::game::core::Message::Control(Control::Quickload),
        continued.now,
    );
    *locked(&continued.result_digest) = saved_digest;
    *locked(&continued.lines) = saved_lines;
    continued.run_until(stop);
    let loaded_digest = *locked(&continued.result_digest);
    let loaded_lines = locked(&continued.lines).clone();
    continued.stop();

    assert_eq!(
        uninterrupted_digest, first_digest,
        "the varied-host runs match"
    );
    assert_eq!(
        uninterrupted_lines, first_lines,
        "gameplay log text matches without timestamps"
    );
    assert_eq!(
        loaded_digest, first_digest,
        "day-37 save-load-continue returns match"
    );
    assert_eq!(
        loaded_lines, first_lines,
        "loaded gameplay log text matches"
    );
}
