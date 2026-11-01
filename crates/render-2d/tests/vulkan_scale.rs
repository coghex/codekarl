//! The milestone 5.5 scale run on Vulkan (CK-162, part C; CK-130's recipe on `glfw-vulkan`/MoltenVK): the frozen recipe as an
//! **optional** harness whose timings are **measured and reported, never gated** (owner, 2026-10-30). Only correctness passes or fails.
//!
//! The recipe is CK-130 r4's: tens of thousands of quads and loaded textures (set A: 32,768 distinct 32 x 32 identity-coded PNGs; set B:
//! 248 images of four sizes), a 1920 x 1080 drawable at 60 Hz, measured through CK-129's common windows (5 s warm-up, 30 s sample).
//! It is **owner-scheduled**: it opens a large window and holds the GPU for minutes, so it runs only when `CK_SCALE_RUN=1` is set
//! (`./tools/ck check --group vulkan.scale` sets it) and the batch never runs it. Without that the executable says so and passes.
//!
//! GLFW's Cocoa window needs the process's main thread, so this executable is a **supervisor and workers** (`harness = false`): the
//! supervisor records the machine, runs one worker process per phase (the Khronos validation layer **off** for the timing runs, **on** for
//! the correctness passes), gives each a whole-process deadline, kills and reaps it on every path, and writes the evidence to the run
//! directory (`CK_SCALE_OUT`, default `vulkan-scale-evidence/<epoch>` under the target directory). Every worker installs the test-only
//! keyboard filter before its window, so the owner's typing is dropped quietly, and nothing here activates, raises or focuses a window.
//!
//! Phases (each a worker): `selftest` (the harness's cadence and window edges), `verify` (8 views decode all 32,768 identities, the
//! distinct-identity counters), `loads` (L-cold and L-warm), `static-ordinary` (per-revision mode, the baseline) and `static-bindless` (S),
//! `churn` (C, once for timing, once with validation on), `heterogeneous` (H) and `caps` (K1 to K3; K0 is a preflight) and `caps-min` (reported, not gated: one 16 MiB request on a fresh presenter at 64 MiB). The application's
//! Lua profile (M-lua) is `apps/codekarl/tests/vulkan_scale_lua.rs`.
//!
//! The recipe's three platform-free modules (`fixtures`, `layout`, `json`) are the Metal harness's, included read-only, so both backends
//! draw the same fixtures and geometry. The rig, the phases, the selftest and the evidence module are Vulkan's own (the duplication is
//! accepted and named in CK-162's follow-up).

#[path = "../../window/tests/support/activation_watch.rs"]
mod activation_watch;
#[path = "vulkan_scale/checks.rs"]
mod checks;
#[path = "vulkan_scale/diagnostic.rs"]
mod diagnostic;
#[path = "vulkan_scale/evidence.rs"]
mod evidence;
#[path = "scale/fixtures.rs"]
mod fixtures;
#[path = "scale/json.rs"]
mod json;
#[path = "scale/layout.rs"]
#[expect(
    dead_code,
    reason = "the layout is shared with the ordinary tests, which also use the geometry predicates"
)]
mod layout;
#[path = "vulkan_scale/phases.rs"]
mod phases;
#[path = "vulkan_scale/rig.rs"]
mod rig;
#[path = "vulkan_scale/selftest.rs"]
mod selftest;

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use ck_gpu_vulkan::{BindlessLimits, LedgerLimits, TextureLimits};
use ck_resources::ResourceLimits;

use json::J;
use rig::Config;

const WORKER_VARIABLE: &str = "CK_SCALE_WORKER";
const RUN_VARIABLE: &str = "CK_SCALE_RUN";
const OUT_VARIABLE: &str = "CK_SCALE_OUT";
/// The longest one worker may run.
const DEADLINE: Duration = Duration::from_secs(900);
/// What the validation layer prints when something is wrong (the rig also collects its messages from the debug messenger, and any that
/// is not tolerated is a problem of the worker).
const VALIDATION_MARKERS: [&str; 2] = ["Validation Error", "validation error"];
/// The variable that turns the Khronos validation layer (with synchronization validation) on in a worker.
const VALIDATION_VARIABLE: &str = "CK_SCALE_VALIDATION";
/// The Khronos validation layer's name, as the loader prints it when it inserts the layer into the instance's chain.
const LAYER_NAME: &str = "VK_LAYER_KHRONOS_validation";
/// The loader's own layer-selection variables. A worker never inherits them: whether the layer is on is `CK_SCALE_VALIDATION` alone
/// (the worker then asks the loader for the layer in its instance options), so an owner's environment cannot put the layer under a
/// timing run or take it from a correctness run.
const LAYER_VARIABLES: [&str; 3] = [
    "VK_INSTANCE_LAYERS",
    "VK_LOADER_LAYERS_ENABLE",
    "VK_LOADER_LAYERS_DISABLE",
];

/// One job of the run: its record name, the worker and whether the Khronos validation layer is on, and whether its **correctness** gates
/// the run (every job's does; no timing does).
struct Job {
    name: &'static str,
    worker: &'static str,
    validation: bool,
    gated: bool,
}

const JOBS: [Job; 11] = [
    Job {
        name: "selftest",
        worker: "selftest",
        validation: false,
        gated: true,
    },
    Job {
        name: "verify",
        worker: "verify",
        validation: true,
        gated: true,
    },
    Job {
        name: "loads",
        worker: "loads",
        validation: false,
        gated: true,
    },
    Job {
        name: "static-ordinary",
        worker: "static-ordinary",
        validation: false,
        gated: true,
    },
    Job {
        name: "static-bindless",
        worker: "static-bindless",
        validation: false,
        gated: true,
    },
    Job {
        name: "churn",
        worker: "churn",
        validation: false,
        gated: true,
    },
    Job {
        name: "churn-checked",
        worker: "churn",
        validation: true,
        gated: true,
    },
    Job {
        name: "heterogeneous",
        worker: "heterogeneous",
        validation: false,
        gated: true,
    },
    Job {
        name: "caps",
        worker: "caps",
        validation: true,
        gated: true,
    },
    // Reported, not gated: what one 16 MiB request does on a fresh presenter at CK-130's 64 MiB (the start-up reservation, shown without K1 or K2 history; not a leak disproof).
    Job {
        name: "caps-min",
        worker: "caps-min",
        validation: true,
        gated: false,
    },
    Job {
        name: "diagnostic",
        worker: "diagnostic",
        validation: false,
        gated: true,
    },
];

fn main() -> ExitCode {
    match env::var(WORKER_VARIABLE).as_deref() {
        Ok(worker) => worker_main(worker),
        Err(_) => supervisor(),
    }
}

// ---------------------------------------------------------------------------------------------
// The workers
// ---------------------------------------------------------------------------------------------

const MIB: u64 = 1024 * 1024;

/// The recipe's limits for phases L, S, C, H and the verification: `max_textures` 65,536, `max_extra_revisions` 1,024,
/// `bindless_texture_slots` 131,072, `texture_bytes` 1 GiB, the other limits at their defaults.
fn recipe_config(title: &'static str, ordinary: bool, capture: bool) -> Config {
    Config {
        title,
        points: phases::window_points(),
        texture: TextureLimits {
            texture_bytes: 1024 * MIB,
            ..TextureLimits::default()
        },
        ledger: LedgerLimits {
            max_extra_revisions: 1024,
            ..LedgerLimits::default()
        },
        max_textures: 65_536,
        bindless_slots: 131_072,
        max_quads: 65_536,
        binding_geometry_bytes: 128 * MIB,
        service: ResourceLimits::DEFAULT,
        ordinary,
        capture,
        validation: env::var(VALIDATION_VARIABLE).as_deref() == Ok("1"),
        assets: None,
        scene_max_quads: 131_072,
        deadline: DEADLINE,
    }
}

/// Phase K's limits, all set explicitly: `max_textures` 1,024, `max_extra_revisions` 64, `bindless_texture_slots` 1,089 (1,024 + 64 +
/// 1), and the given `texture_bytes`: [`phases::CAPS_TEXTURE_BYTES`] for K1 to K3 (CK-130's 64 MiB is one block of the Vulkan image pool and
/// leaves no room for K3's images), [`phases::CAPS_MIN_TEXTURE_BYTES`] for the `caps-min` job.
fn caps_config(assets: PathBuf, texture_bytes: u64) -> Config {
    // The captured frames let K2 read the pixels of its textures.
    let mut config = recipe_config("Codekarl scale caps", false, true);
    config.texture.texture_bytes = texture_bytes;
    config.ledger.max_extra_revisions = 64;
    config.bindless_slots = 1_089;
    config.max_textures = 1_024;
    config.max_quads = 1_024;
    config.service.max_textures = 1_024;
    config.assets = Some(assets);
    config
}

fn worker_main(worker: &str) -> ExitCode {
    let fixtures = fixtures::Fixtures::create(worker);
    let (result, ended) = match worker {
        "selftest" => {
            let config = recipe_config("Codekarl scale self-test", false, false);
            rig::run_phase(config, selftest::run)
        }
        "verify" => {
            fixtures.write_set_a(layout::SET_A);
            let mut config = recipe_config("Codekarl scale verify", false, true);
            config.assets = Some(fixtures.root().to_path_buf());
            rig::run_phase(config, phases::verify)
        }
        "loads" => {
            fixtures.write_set_a(layout::SET_A);
            let mut config = recipe_config("Codekarl scale loads", false, false);
            config.assets = Some(fixtures.root().to_path_buf());
            rig::run_phase(config, phases::loads)
        }
        "static-ordinary" | "static-bindless" => {
            fixtures.write_set_a(layout::SET_A);
            let ordinary = worker == "static-ordinary";
            let label: &'static str = if ordinary {
                "static-ordinary"
            } else {
                "static-bindless"
            };
            let mut config = recipe_config("Codekarl scale static", ordinary, false);
            config.assets = Some(fixtures.root().to_path_buf());
            rig::run_phase(config, move |scene| phases::static_scene(scene, label))
        }
        "diagnostic" => {
            let Some(cell) = diagnostic_cell() else {
                return ExitCode::FAILURE;
            };
            diagnostic_phase(&fixtures, cell)
        }
        "churn" => {
            fixtures.write_set_a(layout::SET_A);
            let mut config = recipe_config("Codekarl scale churn", false, false);
            config.assets = Some(fixtures.root().to_path_buf());
            rig::run_phase(config, phases::churn)
        }
        "heterogeneous" => {
            fixtures.write_set_b();
            let mut config = recipe_config("Codekarl scale heterogeneous", false, false);
            config.assets = Some(fixtures.root().to_path_buf());
            rig::run_phase(config, phases::heterogeneous)
        }
        "caps" => {
            fixtures.write_set_a(1_100);
            fixtures.write_set_b();
            // K0: the same configuration with 1,088 slots is refused at configuration with the three numbers (no window).
            let refused = BindlessLimits {
                texture_slots: 1_088,
            }
            .validate(1_024, 64);
            let k0 = match refused {
                Err(error) => J::o([
                    ("refused", J::Bool(true)),
                    ("message", J::s(error.to_string())),
                ]),
                Ok(_) => J::o([("refused", J::Bool(false))]),
            };
            let k0_ok = matches!(&k0, J::O(fields) if fields.iter().any(|(name, value)| name == "refused" && *value == J::Bool(true)))
                && matches!(&k0, J::O(fields) if fields.iter().any(|(name, value)| name == "message" && matches!(value, J::S(text) if text.contains("1088") && text.contains("1024") && text.contains("64"))));
            let config = caps_config(fixtures.root().to_path_buf(), phases::CAPS_TEXTURE_BYTES);
            let (result, ended) = rig::run_phase(config, move |scene| {
                let mut record = phases::caps(scene)?;
                let passed = record.get("passed") == Some(&J::Bool(true)) && k0_ok;
                record.push("k0_configuration", k0);
                record.push("k0_ok", J::Bool(k0_ok));
                // The pass is the conjunction; the last `passed` wins in the record.
                if let J::O(fields) = &mut record {
                    fields.retain(|(name, _)| name != "passed");
                }
                record.push("passed", J::Bool(passed));
                Ok(record)
            });
            (result, ended)
        }
        "caps-min" => {
            fixtures.write_set_b();
            let config = caps_config(
                fixtures.root().to_path_buf(),
                phases::CAPS_MIN_TEXTURE_BYTES,
            );
            rig::run_phase(config, phases::caps_min)
        }
        other => {
            eprintln!("unknown worker {other:?}");
            return ExitCode::FAILURE;
        }
    };
    activation_watch::assert_clean();
    rig::finish(&result, &ended)
}

fn diagnostic_cell() -> Option<diagnostic::Cell> {
    let Ok(name) = env::var("CK_SCALE_DIAGNOSTIC_CELL") else {
        eprintln!("vulkan_scale: CK_SCALE_DIAGNOSTIC_CELL is required");
        return None;
    };
    let cell = diagnostic::parse(&name);
    if cell.is_none() {
        eprintln!("vulkan_scale: unknown CK-168 diagnostic cell {name:?}");
    }
    cell
}

fn diagnostic_phase(
    fixtures: &fixtures::Fixtures,
    cell: diagnostic::Cell,
) -> (Result<J, String>, rig::Ended) {
    fixtures.write_set_a(cell.live_textures);
    let mut config = recipe_config("CK-168 Vulkan diagnostic", cell.ordinary, false);
    config.max_textures = cell.max_textures;
    config.bindless_slots = cell.bindless_slots;
    config.max_quads = 65_536;
    config.ledger.max_extra_revisions = 1_024;
    config.assets = Some(fixtures.root().to_path_buf());
    rig::run_phase(config, move |scene| phases::diagnostic_scene(scene, cell))
}

// ---------------------------------------------------------------------------------------------
// The supervisor
// ---------------------------------------------------------------------------------------------

/// A child process: killed and reaped when dropped, on every path.
struct Owned(Option<Child>);

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

type Stream = Result<String, String>;

fn read_all(
    name: &'static str,
    mut pipe: impl Read + Send + 'static,
) -> thread::JoinHandle<Stream> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Err(error) = pipe.read_to_end(&mut bytes) {
            return Err(format!(
                "{name}: the read failed after {} bytes: {error}",
                bytes.len()
            ));
        }
        String::from_utf8(bytes).map_err(|error| format!("{name}: not valid UTF-8: {error}"))
    })
}

/// What one worker run came to.
struct Outcome {
    name: &'static str,
    record: Option<String>,
    passed: bool,
    problems: Vec<String>,
    seconds: f64,
    /// Whether the loader's own log shows the validation layer inserted (`None`: the log could not be read).
    layer_loaded: Option<bool>,
}

/// The command that runs `job`'s worker: this executable with the worker's name, and the layer environment the job is meant to have.
/// The loader's layer variables are removed from every child (an owner's `VK_INSTANCE_LAYERS` or `VK_LOADER_LAYERS_ENABLE` must not put
/// the layer under a timing run, nor change a correctness run's), the layer is on through `CK_SCALE_VALIDATION` alone, and the loader
/// is asked to say which layers it inserts.
fn job_command(job: &Job, exe: &Path) -> Command {
    let mut command = Command::new(exe);
    command
        .env(WORKER_VARIABLE, job.worker)
        .env_remove(RUN_VARIABLE)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for variable in LAYER_VARIABLES {
        command.env_remove(variable);
    }
    if job.validation {
        command.env(VALIDATION_VARIABLE, "1");
    } else {
        command.env_remove(VALIDATION_VARIABLE);
        // A timing run also runs without any implicit layer an installation might add.
        command.env("VK_LOADER_LAYERS_DISABLE", "~implicit~");
    }
    // The loader reports the layers it inserts on stderr: the check reads what was actually loaded, not what was asked for.
    command.env("VK_LOADER_DEBUG", "layer");
    command
}

/// Whether the loader's debug text shows the validation layer inserted into a chain.
fn layer_in_loader_log(text: &str) -> bool {
    text.lines()
        .any(|line| line.contains(LAYER_NAME) && line.to_lowercase().contains("insert"))
}

/// What is wrong when the layer's actual state (`loaded`, from the loader's log; `None` when the log could not be read) is not the job's:
/// the layer is on in a correctness job and in no timing job.
fn layer_problem(loaded: Option<bool>, validation: bool) -> Option<String> {
    match (loaded, validation) {
        (Some(true), false) => Some(format!(
            "{LAYER_NAME} was loaded in a timing job (the loader's own log says so)"
        )),
        (Some(false), true) => Some(format!(
            "{LAYER_NAME} was asked for but the loader's log shows no insertion of it (or its log lines are not recognised)"
        )),
        _ => None,
    }
}

fn run_job(job: &Job, out: &Path) -> Outcome {
    let exe = env::current_exe().expect("the test binary's path");
    let mut command = job_command(job, &exe);
    let started = Instant::now();
    let mut child = command.spawn().expect("the worker starts");
    let (stdout, stderr) = (
        read_all("stdout", child.stdout.take().expect("piped")),
        read_all("stderr", child.stderr.take().expect("piped")),
    );
    let mut owned = Owned(Some(child));
    let status = loop {
        let child = owned.0.as_mut().expect("the worker is running");
        if let Some(status) = child.try_wait().expect("the worker can be waited for") {
            break Some(status);
        }
        if started.elapsed() >= DEADLINE {
            break None;
        }
        thread::sleep(Duration::from_millis(50));
    };
    drop(owned);
    let seconds = started.elapsed().as_secs_f64();
    let stdout = stdout
        .join()
        .unwrap_or_else(|_| Err("stdout: the reader panicked".to_owned()));
    let stderr = stderr
        .join()
        .unwrap_or_else(|_| Err("stderr: the reader panicked".to_owned()));
    let mut problems = Vec::new();
    match status {
        None => problems.push("the worker did not finish by the deadline".to_owned()),
        Some(status) if !status.success() => {
            problems.push(format!("the worker exited with {status:?}"));
        }
        Some(_) => {}
    }
    for stream in [&stdout, &stderr] {
        if let Err(error) = stream {
            problems.push(error.clone());
        }
    }
    if let Ok(text) = &stderr
        && let Some(marker) = VALIDATION_MARKERS
            .iter()
            .find(|marker| text.contains(**marker))
    {
        problems.push(format!("the validation layer reported {marker:?}"));
    }
    // What the loader actually inserted, against what the job is: the layer on in a correctness job and in no timing job.
    let layer_loaded = stderr.as_deref().ok().map(layer_in_loader_log);
    if let Some(problem) = layer_problem(layer_loaded, job.validation) {
        problems.push(problem);
    }
    let _ = fs::write(
        out.join(format!("{}.stdout.txt", job.name)),
        stdout.as_deref().unwrap_or(""),
    );
    let _ = fs::write(
        out.join(format!("{}.stderr.txt", job.name)),
        stderr.as_deref().unwrap_or(""),
    );
    let record = stdout.as_deref().ok().and_then(|text| {
        text.lines()
            .find_map(|line| line.strip_prefix("SCALE_RESULT ").map(str::to_owned))
    });
    let status_line = stdout
        .as_deref()
        .ok()
        .is_some_and(|text| text.lines().any(|line| line == "SCALE_STATUS passed=true"));
    if record.is_none() {
        problems.push("the worker printed no SCALE_RESULT record".to_owned());
    }
    Outcome {
        name: job.name,
        record,
        passed: status_line && problems.is_empty(),
        problems,
        seconds,
        layer_loaded,
    }
}

/// The text of a command, or why it could not be run.
fn tool(program: &str, arguments: &[&str]) -> String {
    match Command::new(program).args(arguments).output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        Err(error) => format!("(could not run {program}: {error})"),
    }
}

/// The machine facts the evidence records: macOS, Xcode and SDK, the GPU, the display, the power source, the build profile, the loader
/// and the `MoltenVK` configuration in the environment (each worker's record adds the device's `driverName`/`driverInfo` and API version).
fn machine() -> J {
    J::o([
        ("macos", J::s(tool("sw_vers", &[]))),
        ("xcode", J::s(tool("xcodebuild", &["-version"]))),
        ("sdk", J::s(tool("xcrun", &["--show-sdk-version"]))),
        (
            "cpu",
            J::s(tool("sysctl", &["-n", "machdep.cpu.brand_string"])),
        ),
        ("hardware", J::s(tool("sysctl", &["-n", "hw.model"]))),
        (
            "displays",
            J::s(tool("system_profiler", &["SPDisplaysDataType"])),
        ),
        ("power", J::s(tool("pmset", &["-g", "ps"]))),
        ("rustc", J::s(tool("rustc", &["-V"]))),
        ("uptime", J::s(tool("uptime", &[]))),
        (
            "build_profile",
            J::s(if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }),
        ),
        ("features", J::s("vulkan")),
        ("mvk_config", mvk_config()),
        (
            "vulkan_environment",
            J::s(
                env::vars()
                    .filter(|(name, _)| name.starts_with("VK_") || name.starts_with("VULKAN_"))
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
        ),
        ("command", J::s(env::args().collect::<Vec<_>>().join(" "))),
        (
            "clock",
            J::s(
                "the harness's own monotonic clock (std::time::Instant since the process's first read), given to the presenter as its metrics clock",
            ),
        ),
    ])
}

/// The `MoltenVK` configuration variables in the environment, or the statement that there are none.
fn mvk_config() -> J {
    let set: Vec<String> = env::vars()
        .filter(|(name, _)| name.starts_with("MVK_CONFIG_"))
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    if set.is_empty() {
        J::s("none set")
    } else {
        J::A(set.into_iter().map(J::S).collect())
    }
}

/// The evidence directory when `CK_SCALE_OUT` is not set: `vulkan-scale-evidence/<name>` under the build's target directory.
fn default_out(name: &str) -> PathBuf {
    // The test executable is `<target>/<profile>/deps/vulkan_scale-<hash>`.
    let target = env::current_exe()
        .ok()
        .and_then(|exe| exe.ancestors().nth(3).map(Path::to_path_buf))
        .unwrap_or_else(env::temp_dir);
    target.join("vulkan-scale-evidence").join(name)
}

/// The device-free checks of the harness's own logic, run on every invocation (the "not run" one included): `false` after reporting
/// each failure by name.
fn checks_pass() -> bool {
    let failures = checks::run();
    for failure in &failures {
        println!("test vulkan_scale ... FAILED: {failure}");
    }
    if !failures.is_empty() {
        println!("test result: FAILED. 0 passed; {} failed", failures.len());
    }
    println!(
        "vulkan_scale: {} device-free checks, {} failed",
        checks::COUNT,
        failures.len()
    );
    failures.is_empty()
}

/// The report of a run that was not asked for: the scale run is owner-scheduled.
fn not_run() -> ExitCode {
    println!(
        "test vulkan_scale ... ok (not run: the scale run is owner-scheduled; `{RUN_VARIABLE}=1` runs it, `./tools/ck check --group vulkan.scale` does)"
    );
    println!("test result: ok. 0 passed; 0 failed; 1 ignored");
    ExitCode::SUCCESS
}

fn supervisor() -> ExitCode {
    if !checks_pass() {
        return ExitCode::FAILURE;
    }
    if env::var(RUN_VARIABLE).as_deref() != Ok("1") {
        return not_run();
    }
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let out =
        env::var_os(OUT_VARIABLE).map_or_else(|| default_out(&epoch.to_string()), PathBuf::from);
    fs::create_dir_all(&out).expect("the evidence directory");
    let only: Option<Vec<String>> = env::var("CK_SCALE_ONLY")
        .ok()
        .map(|list| list.split(',').map(str::to_owned).collect());
    let mut outcomes = Vec::new();
    for job in &JOBS {
        if !selected_job(job, only.as_deref()) {
            continue;
        }
        println!(
            "scale: running {} (validation {})",
            job.name,
            if job.validation { "on" } else { "off" }
        );
        let outcome = run_job(job, &out);
        println!(
            "scale: {} {} in {:.1} s{}",
            job.name,
            if outcome.passed {
                "passed"
            } else {
                "DID NOT PASS"
            },
            outcome.seconds,
            if job.gated {
                " (correctness gated; timings reported)"
            } else {
                " (reported)"
            }
        );
        for problem in &outcome.problems {
            println!("scale:   {problem}");
        }
        outcomes.push((job, outcome));
    }
    let accepted = outcomes
        .iter()
        .all(|(job, outcome)| !job.gated || outcome.passed);
    let mut phases_json = String::from("{");
    for (index, (_, outcome)) in outcomes.iter().enumerate() {
        if index != 0 {
            phases_json.push(',');
        }
        phases_json.push_str(&J::s(outcome.name).render());
        phases_json.push(':');
        phases_json.push_str(outcome.record.as_deref().unwrap_or("null"));
    }
    phases_json.push('}');
    let verdicts: Vec<J> = outcomes
        .iter()
        .map(|(job, outcome)| {
            J::o([
                ("phase", J::s(outcome.name)),
                ("gated", J::Bool(job.gated)),
                ("passed", J::Bool(outcome.passed)),
                ("seconds", J::F(outcome.seconds)),
                (
                    "validation_layer_loaded_per_loader_log",
                    outcome.layer_loaded.map_or(J::Null, J::Bool),
                ),
                (
                    "problems",
                    J::A(outcome.problems.iter().cloned().map(J::S).collect()),
                ),
            ])
        })
        .collect();
    let report = format!(
        "{{\"machine\":{},\"verdicts\":{},\"accepted\":{accepted},\"phases\":{phases_json}}}",
        machine().render(),
        J::A(verdicts).render()
    );
    fs::write(out.join("results.json"), &report).expect("the results file");
    println!("scale: evidence in {}", out.display());
    println!(
        "test vulkan_scale ... {}",
        if accepted { "ok" } else { "FAILED" }
    );
    if accepted {
        println!("test result: ok. 1 passed; 0 failed");
        ExitCode::SUCCESS
    } else {
        println!("test result: FAILED. 0 passed; 1 failed");
        ExitCode::FAILURE
    }
}

fn selected_job(job: &Job, only: Option<&[String]>) -> bool {
    if job.name == "diagnostic" && only.is_none() {
        return false;
    }
    only.is_none_or(|only| only.iter().any(|name| name == job.name))
}
