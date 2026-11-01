//! The milestone 5 scale acceptance on Metal (CK-130, `docs/scene-design.md` §11.3): the frozen recipe as an **optional** harness.
//!
//! Tens of thousands of quads and loaded textures (set A: 32,768 distinct 32 x 32 identity-coded PNGs; set B: 248 images of four
//! sizes), a 1920 x 1080 drawable at 60 Hz, p95 CPU-total and GPU frame time each under 8 ms, measured through CK-129's common
//! windows (5 s warm-up, 30 s sample). It is **owner-scheduled**: it opens a large window and holds the GPU for minutes, so it runs
//! only when `CK_SCALE_RUN=1` is set (`./tools/ck check --group metal.scale` sets it) and the batch never runs it. Without that the
//! executable says so and passes.
//!
//! `AppKit` needs the process's main thread, so this executable is a **supervisor and workers** (`harness = false`): the supervisor
//! records the machine, runs one worker process per phase (Metal validation **off** for the timing runs, **on** for the correctness
//! passes), gives each a whole-process deadline, kills and reaps it on every path, and writes the evidence to the run directory
//! (`CK_SCALE_OUT`, default `scale-evidence/<epoch>` under the target directory). Every worker installs the test-only keyboard filter before its window, so
//! the owner's typing is dropped quietly.
//!
//! Phases (each a worker): `selftest` (the harness's cadence and window edges), `verify` (8 views decode all 32,768 identities, the
//! distinct-identity counters), `loads` (L-cold and L-warm), `static-ordinary` and `static-bindless` (S), `churn` (C, once for timing,
//! once with validation on), `heterogeneous` (H) and `caps` (K1 to K3; K0 is a preflight). The application's Lua profile (M-lua) is
//! `apps/codekarl/tests/scale_lua.rs`.

#[path = "../../window/tests/support/activation_watch.rs"]
mod activation_watch;
#[path = "scale/evidence.rs"]
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
#[path = "scale/phases.rs"]
mod phases;
#[path = "scale/rig.rs"]
mod rig;
#[path = "scale/selftest.rs"]
mod selftest;

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use ck_gpu_metal::{BindlessLimits, LedgerLimits, TextureLimits};
use ck_render_2d::ResourceBackendLimits;
use ck_resources::ResourceLimits;

use json::J;
use rig::Config;

const WORKER_VARIABLE: &str = "CK_SCALE_WORKER";
const RUN_VARIABLE: &str = "CK_SCALE_RUN";
const OUT_VARIABLE: &str = "CK_SCALE_OUT";
const FORWARDING_FIXTURE: &str = "external_forwarding_fixture";
/// The longest one worker may run.
const DEADLINE: Duration = Duration::from_secs(900);
/// What Metal's validation layers print when something is wrong.
const VALIDATION_MARKERS: [&str; 4] = [
    "failed assertion",
    "MTLDebug",
    "Validation failed",
    "validation error",
];

/// One job of the run: its record name, the worker and whether Metal's validation is on, and whether it gates the acceptance.
struct Job {
    name: &'static str,
    worker: &'static str,
    validation: bool,
    gated: bool,
}

const JOBS: [Job; 9] = [
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
        gated: false,
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
        gated: false,
    },
    Job {
        name: "caps",
        worker: "caps",
        validation: true,
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
        backend: ResourceBackendLimits {
            texture: TextureLimits {
                texture_bytes: 1024 * MIB,
                ..TextureLimits::default()
            },
            ledger: LedgerLimits {
                max_extra_revisions: 1024,
                ..LedgerLimits::default()
            },
            max_quads: 65_536,
            binding_geometry_bytes: 128 * MIB,
            bindless: BindlessLimits {
                texture_slots: 131_072,
                max_textures: 65_536,
                max_extra_revisions: 1024,
            },
        },
        service: ResourceLimits::DEFAULT,
        ordinary,
        capture,
        presenter_hooks: ck_gpu_metal::TestHooks::default(),
        assets: None,
        scene_max_quads: 131_072,
        deadline: DEADLINE,
    }
}

/// Phase K's limits, all set explicitly: `max_textures` 1,024, `max_extra_revisions` 64, `bindless_texture_slots` 1,089 (1,024 + 64 +
/// 1), `texture_bytes` 64 MiB.
fn caps_config(assets: PathBuf) -> Config {
    // The captured frames let K2 read the pixels of its waiting textures.
    let mut config = recipe_config("Codekarl scale caps", false, true);
    config.backend.texture.texture_bytes = 64 * MIB;
    config.backend.ledger.max_extra_revisions = 64;
    config.backend.bindless = BindlessLimits {
        texture_slots: 1_089,
        max_textures: 1_024,
        max_extra_revisions: 64,
    };
    config.backend.max_quads = 1_024;
    config.service.max_textures = 1_024;
    config.assets = Some(assets);
    config
}

fn worker_main(worker: &str) -> ExitCode {
    if worker == FORWARDING_FIXTURE {
        println!(
            "{}",
            activation_watch::format_external_summary(
                &std::process::id().to_string(),
                1,
                0,
                0,
                0,
                0,
            )
        );
        return ExitCode::SUCCESS;
    }
    let fixtures = fixtures::Fixtures::create(worker);
    let (result, ended) = match worker {
        "selftest" => {
            let recorded = selftest::Recorded::default();
            let mut config = recipe_config("Codekarl scale self-test", false, false);
            config.presenter_hooks = recorded.hooks();
            rig::run_phase(config, move |scene| selftest::run(scene, &recorded))
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
                max_textures: 1_024,
                max_extra_revisions: 64,
            }
            .validate();
            let k0 = match refused {
                Err(error) => J::o([
                    ("refused", J::Bool(true)),
                    ("message", J::s(format!("{error:?}"))),
                ]),
                Ok(_) => J::o([("refused", J::Bool(false))]),
            };
            let k0_ok = matches!(&k0, J::O(fields) if fields.iter().any(|(name, value)| name == "refused" && *value == J::Bool(true)))
                && matches!(&k0, J::O(fields) if fields.iter().any(|(name, value)| name == "message" && matches!(value, J::S(text) if text.contains("1088") && text.contains("1024") && text.contains("64"))));
            let config = caps_config(fixtures.root().to_path_buf());
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
        other => {
            eprintln!("unknown worker {other:?}");
            return ExitCode::FAILURE;
        }
    };
    activation_watch::assert_clean();
    rig::finish(&result, &ended)
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
}

#[expect(
    clippy::too_many_lines,
    reason = "the bounded worker capture and validation gates stay together"
)]
fn run_job(job: &Job, out: &Path) -> Outcome {
    let exe = env::current_exe().expect("the test binary's path");
    let mut command = Command::new(exe);
    command
        .env(WORKER_VARIABLE, job.worker)
        .env_remove(RUN_VARIABLE)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if job.validation {
        command
            .env("MTL_DEBUG_LAYER", "1")
            .env("MTL_SHADER_VALIDATION", "1");
    } else {
        command
            .env_remove("MTL_DEBUG_LAYER")
            .env_remove("MTL_SHADER_VALIDATION");
    }
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
    let captured = format!(
        "{}\n{}",
        stdout.as_deref().unwrap_or(""),
        stderr.as_deref().unwrap_or("")
    );
    let fixture = job.worker == FORWARDING_FIXTURE;
    let case = if fixture {
        FORWARDING_FIXTURE
    } else {
        job.name
    };
    let forwarded =
        activation_watch::forwarded_external_summaries("metal.scale", case, "quiet", &captured);
    let mut problems = Vec::new();
    for line in &forwarded {
        println!("{line}");
    }
    if fixture
        && let Err(error) =
            activation_watch::check_fixture_forwarding("metal.scale", case, "quiet", &forwarded)
    {
        problems.push(error);
    }
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
        problems.push(format!("Metal validation reported {marker:?}"));
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
    if !fixture && record.is_none() {
        problems.push("the worker printed no SCALE_RESULT record".to_owned());
    }
    let status_line = fixture || status_line;
    Outcome {
        name: job.name,
        record,
        passed: status_line && problems.is_empty(),
        problems,
        seconds,
    }
}

/// Runs one successful no-window worker through the scale supervisor's capture and forwarding path.
fn run_forwarding_fixture(out: &Path) -> Result<(), String> {
    let job = Job {
        name: "activation-forwarding",
        worker: FORWARDING_FIXTURE,
        validation: false,
        gated: true,
    };
    let outcome = run_job(&job, out);
    if outcome.passed {
        Ok(())
    } else {
        Err(outcome.problems.join("; "))
    }
}

/// The text of a command, or why it could not be run.
fn tool(program: &str, arguments: &[&str]) -> String {
    match Command::new(program).args(arguments).output() {
        Ok(output) => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        Err(error) => format!("(could not run {program}: {error})"),
    }
}

/// The machine facts the evidence records: macOS, Xcode and SDK, the GPU, the display, the power source, the build profile.
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
        ("features", J::s("metal")),
        ("command", J::s(env::args().collect::<Vec<_>>().join(" "))),
        (
            "clock",
            J::s("CLOCK_UPTIME_RAW (mach_absolute_time via ck_render_2d::now_ns)"),
        ),
    ])
}

/// The evidence directory when `CK_SCALE_OUT` is not set: `scale-evidence/<name>` under the build's target directory.
fn default_out(name: &str) -> PathBuf {
    // The test executable is `<target>/<profile>/deps/scale-<hash>`.
    let target = env::current_exe()
        .ok()
        .and_then(|exe| exe.ancestors().nth(3).map(Path::to_path_buf))
        .unwrap_or_else(env::temp_dir);
    target.join("scale-evidence").join(name)
}

fn supervisor() -> ExitCode {
    if env::var(RUN_VARIABLE).as_deref() != Ok("1") {
        println!(
            "test scale ... ok (not run: the scale acceptance is owner-scheduled; `{RUN_VARIABLE}=1` runs it, `./tools/ck check --group metal.scale` does)"
        );
        println!("test result: ok. 0 passed; 0 failed; 1 ignored");
        return ExitCode::SUCCESS;
    }
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let out =
        env::var_os(OUT_VARIABLE).map_or_else(|| default_out(&epoch.to_string()), PathBuf::from);
    fs::create_dir_all(&out).expect("the evidence directory");
    let forwarding = run_forwarding_fixture(&out);
    println!(
        "test passing scale fixture's external summary reaches supervisor output ... {}",
        if forwarding.is_ok() { "ok" } else { "FAILED" }
    );
    if let Err(error) = forwarding {
        println!("scale: activation forwarding fixture: {error}");
        return ExitCode::FAILURE;
    }
    let only: Option<Vec<String>> = env::var("CK_SCALE_ONLY")
        .ok()
        .map(|list| list.split(',').map(str::to_owned).collect());
    let mut outcomes = Vec::new();
    for job in &JOBS {
        if only
            .as_ref()
            .is_some_and(|only| !only.iter().any(|name| name == job.name))
        {
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
            if job.gated { " (gated)" } else { " (reported)" }
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
    println!("test scale ... {}", if accepted { "ok" } else { "FAILED" });
    if accepted {
        println!("test result: ok. 1 passed; 0 failed");
        ExitCode::SUCCESS
    } else {
        println!("test result: FAILED. 0 passed; 1 failed");
        ExitCode::FAILURE
    }
}
