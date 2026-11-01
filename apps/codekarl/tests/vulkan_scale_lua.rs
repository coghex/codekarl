//! The milestone 5.5 all-quads-moving Lua profile on Vulkan (CK-162, part C; CK-130's M-lua on `glfw-vulkan`), run through the
//! application's own code in process behind the test-only keyboard filter (builds with the `vulkan` feature on macOS).
//!
//! **Measured, not gated** (owner, 2026-10-30; CK-130: "Measure it, don't gate it"). It is **owner-scheduled** like the rest of the scale
//! run: it opens a large window and holds the GPU for about a minute, so it runs only when `CK_SCALE_RUN=1` is set (`./tools/ck check
//! --group vulkan.scale` sets it); otherwise it says so and passes.
//!
//! Like `vulkan_app.rs` this executable is a **supervisor and a worker** (`harness = false`): GLFW's Cocoa window needs the process's main
//! thread, so the supervisor starts it again as the worker, with a whole-process deadline, kills and reaps it on every path, and reads
//! what it printed. The worker blocks SIGINT and SIGTERM on the main thread first (before any other thread exists, as the application's
//! signal handling requires), installs the keyboard filter (the owner's keys are dropped before any dispatch), lets the Lua updates run
//! while the window is unfocused (CK-163's test switch: nothing here activates, raises or focuses a window) and calls
//! `codekarl::main_entry` with `--config`: the real Lua path runs. `config/scale-lua-vulkan.toml` is the run's configuration with two
//! placeholders the supervisor fills in (the fresh temporary fixtures directory and this repository's root);
//! `scripts/scale/lua_moving.lua` is the script, unchanged.
//!
//! The report: the application's one `metrics` record, whole, and the facts it holds. The measured window is the application's (CK-129
//! r3): from the script's mark (it starts 250 ms after the call) to the run's end timestamp, so **its actual duration, the delivered and
//! missed updates and the sample counts are reported as they are**, not called 30 s. A low frame or update rate under this load is a
//! finding, not a failure. A source the device does not offer (GPU timestamps, presentation feedback) shows in the record as
//! unavailable, with its reason, and is reported as that, never as zero.

#![cfg(all(feature = "vulkan", target_os = "macos"))]

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::mem::MaybeUninit;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};
#[path = "../../../crates/window/tests/support/activation_watch.rs"]
mod activation_watch;

#[expect(
    dead_code,
    reason = "the layout module is shared with the render-2d harness, which uses what this one does not"
)]
#[path = "../../../crates/render-2d/tests/scale/layout.rs"]
mod layout;

#[expect(
    dead_code,
    reason = "the fixtures are shared with the render-2d harness, which also writes set B"
)]
#[path = "../../../crates/render-2d/tests/scale/fixtures.rs"]
mod fixtures;

unsafe extern "C" {
    fn ck_app_t_install_input_filter();
    fn ck_app_t_dropped_keys() -> u64;
}

const WORKER_FLAG: &str = "--ck-scale-lua-worker";
const RUN_VARIABLE: &str = "CK_SCALE_RUN";
const OUT_VARIABLE: &str = "CK_SCALE_OUT";
/// What the worker says about the activation watch. It prints the first before `codekarl::main_entry` and the second after it, and
/// `main_entry` redirects the process's standard error onto its standard output (`dup2(1, 2)`, `apps/codekarl/src/lib.rs`), so the second
/// one reaches the supervisor on the stdout pipe: a marker is looked for on both streams.
const WORKER_MARKERS: [&str; 2] = [
    "vulkan-scale-lua worker: activation watch installed",
    "vulkan-scale-lua worker: activation watch clean",
];

/// The worker markers that appear on neither stream.
fn missing_markers(stdout: &str, stderr: &str) -> Vec<&'static str> {
    WORKER_MARKERS
        .into_iter()
        .filter(|marker| !stdout.contains(marker) && !stderr.contains(marker))
        .collect()
}

/// The Khronos validation layer's name, as the loader prints it when it inserts the layer into an instance's chain.
const LAYER_NAME: &str = "VK_LAYER_KHRONOS_validation";
/// The loader's own layer-selection variables: the worker never inherits them, so an owner's environment cannot put the layer under this
/// timing run (the application asks for no validation of its own).
const LAYER_VARIABLES: [&str; 3] = [
    "VK_INSTANCE_LAYERS",
    "VK_LOADER_LAYERS_ENABLE",
    "VK_LOADER_LAYERS_DISABLE",
];

/// Whether the loader's debug text shows the validation layer inserted into a chain.
fn layer_in_loader_log(text: &str) -> bool {
    text.lines()
        .any(|line| line.contains(LAYER_NAME) && line.to_lowercase().contains("insert"))
}

/// Whether the loader's debug text is there at all (it says which layers it looks for and inserts; without it "no insertion" proves nothing).
fn loader_log_present(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim_start().contains("LAYER:"))
}

/// What is wrong with the layer's state in this timing run, from the loader's own log on both streams (`main_entry` moves standard error
/// onto standard output): the layer must not be inserted, and the log must be there to say so.
fn layer_problem(stdout: &str, stderr: &str) -> Option<String> {
    let both = format!("{stdout}\n{stderr}");
    if layer_in_loader_log(&both) {
        return Some(format!(
            "{LAYER_NAME} was loaded in this timing run (the loader's own log says so)"
        ));
    }
    if !loader_log_present(&both) {
        return Some(
            "the loader's log (VK_LOADER_DEBUG=layer) was not produced, so the layer being off is not shown"
                .to_owned(),
        );
    }
    None
}

/// How many marker cases and layer cases `checks` makes, and so how many checks.
const MARKER_CASES: usize = 6;
const LAYER_CASES: usize = 4;
const CHECK_COUNT: usize = MARKER_CASES + LAYER_CASES;

/// The device-free checks of the supervisor's own reading, run on every invocation (the "not run" one included): the failures, each named.
fn checks() -> Vec<String> {
    let [installed, clean] = WORKER_MARKERS;
    let both = format!("{installed}\n{clean}\n");
    // (what, stdout, stderr, the markers that must be reported missing)
    let cases: [(&str, &str, &str, Vec<&str>); MARKER_CASES] = [
        (
            "the real run: installed on stderr before main_entry, clean on stdout after its redirect",
            clean,
            installed,
            Vec::new(),
        ),
        ("both on stderr (no redirect)", "", &both, Vec::new()),
        ("both on stdout", &both, "", Vec::new()),
        (
            "the clean marker on neither stream (control: the old stderr-only reading fails the real run, this one fails this)",
            "",
            installed,
            vec![clean],
        ),
        (
            "the installed marker on neither stream",
            clean,
            "",
            vec![installed],
        ),
        ("no marker at all", "", "", vec![installed, clean]),
    ];
    let mut failures: Vec<String> = cases
        .into_iter()
        .filter_map(|(what, stdout, stderr, missing)| {
            let got = missing_markers(stdout, stderr);
            (got != missing)
                .then(|| format!("{what}: reported missing {got:?}, expected {missing:?}"))
        })
        .collect();
    let inserted = format!(
        "INFO | LAYER:  Insert instance layer \"{LAYER_NAME}\" (libVkLayer_khronos_validation.dylib)\n"
    );
    let manifest = format!(
        "INFO | LAYER:  Found manifest file, layer \"{LAYER_NAME}\" in /usr/local/share/vulkan/explicit_layer.d\n"
    );
    let searching = "LAYER:             Searching for implicit layer manifest files\n";
    let layer_cases: [(&str, bool); LAYER_CASES] = [
        (
            "an insertion of the layer on stdout (after the redirect) is a problem",
            layer_problem(&inserted, "").is_some_and(|problem| problem.contains("was loaded")),
        ),
        (
            "an insertion of the layer on stderr is a problem",
            layer_problem("", &inserted).is_some_and(|problem| problem.contains("was loaded")),
        ),
        (
            "a manifest line naming the layer, with the loader log present and no insertion, is no problem",
            layer_problem(&manifest, searching).is_none(),
        ),
        (
            "no loader log at all is a problem (the layer being off is not shown)",
            layer_problem("", "").is_some_and(|problem| problem.contains("was not produced")),
        ),
    ];
    for (what, holds) in layer_cases {
        if !holds {
            failures.push(what.to_owned());
        }
    }
    failures
}

/// The longest the worker may run: loading, the 5 s settle, the 30 s sample and the teardown, with room.
const DEADLINE: Duration = Duration::from_secs(600);

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    if arguments.next().as_deref() == Some(std::ffi::OsStr::new(WORKER_FLAG)) {
        return worker(arguments.collect());
    }
    supervisor()
}

/// Step 1: blocks SIGINT and SIGTERM on the calling (main) thread, so every thread created afterwards inherits them.
fn block_signals() {
    let mut set = MaybeUninit::<libc::sigset_t>::uninit();
    // SAFETY: `sigemptyset` initialises the set it is given.
    let emptied = unsafe { libc::sigemptyset(set.as_mut_ptr()) };
    assert_eq!(emptied, 0, "sigemptyset failed");
    // SAFETY: `set` was initialised above and SIGINT is a valid signal number.
    let added = unsafe { libc::sigaddset(set.as_mut_ptr(), libc::SIGINT) };
    assert_eq!(added, 0, "sigaddset failed");
    // SAFETY: `set` was initialised above and SIGTERM is a valid signal number.
    let added = unsafe { libc::sigaddset(set.as_mut_ptr(), libc::SIGTERM) };
    assert_eq!(added, 0, "sigaddset failed");
    // SAFETY: blocks the signals of `set` on this thread only; the old mask is not wanted.
    let blocked =
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, set.as_ptr(), std::ptr::null_mut()) };
    assert_eq!(blocked, 0, "pthread_sigmask failed");
}

/// The worker: the application plus the keyboard filter.
fn worker(arguments: Vec<std::ffi::OsString>) -> ExitCode {
    block_signals();
    // The owner is using the machine: the window is opened without asking the system to activate the application or make it key.
    ck_window::test_support::open_windows_without_activation();
    // SAFETY: main thread; installs one local event monitor, once, before the first window.
    unsafe { ck_app_t_install_input_filter() };
    activation_watch::install(false);
    eprintln!("vulkan-scale-lua worker: activation watch installed");
    // The window is not focused (nothing here takes focus, and the owner is using the machine), so the update source must keep running.
    let _unfocused_updates = ck_window::test_support::allow_updates_while_unfocused();
    let code = codekarl::main_entry(arguments);
    activation_watch::assert_clean();
    eprintln!("vulkan-scale-lua worker: activation watch clean");
    // SAFETY: reads an atomic counter.
    let dropped = unsafe { ck_app_t_dropped_keys() };
    eprintln!("vulkan-scale-lua worker: the filter dropped {dropped} keys");
    code
}

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

fn read_all(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<Result<String, String>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)
            .map_err(|error| format!("the read failed after {} bytes: {error}", bytes.len()))?;
        String::from_utf8(bytes).map_err(|error| format!("not valid UTF-8: {error}"))
    })
}

/// The number after `key` at or after `from` in `text`.
fn number_after(text: &str, from: &str, key: &str) -> Option<u64> {
    let at = text.find(from)?;
    let rest = &text[at..];
    let start = rest.find(key)? + key.len();
    let digits: String = rest[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn escape(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[expect(
    clippy::too_many_lines,
    reason = "the supervisor is one sequential protocol: fixtures, worker, record"
)]
fn supervisor() -> ExitCode {
    let failures = checks();
    println!(
        "vulkan_scale_lua: {CHECK_COUNT} device-free checks, {} failed",
        failures.len()
    );
    if !failures.is_empty() {
        for failure in &failures {
            println!("vulkan_scale_lua: {failure}");
        }
        println!("test vulkan_scale_lua ... FAILED");
        return ExitCode::FAILURE;
    }
    if env::var(RUN_VARIABLE).as_deref() != Ok("1") {
        println!(
            "test vulkan_scale_lua ... ok (not run: the scale run is owner-scheduled; `{RUN_VARIABLE}=1` runs it, `./tools/ck check --group vulkan.scale` does)"
        );
        println!("test result: ok. 0 passed; 0 failed; 1 ignored");
        return ExitCode::SUCCESS;
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .join("../..")
        .canonicalize()
        .expect("the repository root");
    let fixtures = fixtures::Fixtures::create("lua");
    fixtures.write_set_a(layout::SET_A);
    let template = fs::read_to_string(root.join("config/scale-lua-vulkan.toml"))
        .expect("config/scale-lua-vulkan.toml");
    let config_text = template
        .replace(
            "@FIXTURES@",
            fixtures.root().to_str().expect("a UTF-8 path"),
        )
        .replace("@ROOT@", root.to_str().expect("a UTF-8 path"));
    assert!(
        !config_text.contains('@'),
        "an unfilled placeholder in the configuration"
    );
    let config = fixtures.root().join("scale-lua.toml");
    fs::write(&config, config_text).expect("the run's configuration");

    let exe = env::current_exe().expect("this executable's path");
    let mut command = Command::new(exe);
    // A timing run: no layer from the owner's environment, and no implicit layer an installation might add.
    for variable in LAYER_VARIABLES {
        command.env_remove(variable);
    }
    command.env("VK_LOADER_LAYERS_DISABLE", "~implicit~");
    command
        .arg(WORKER_FLAG)
        .arg("--config")
        .arg(&config)
        // The loader reports the layers it inserts: the check reads what was actually loaded, not what was asked for.
        .env("VK_LOADER_DEBUG", "layer")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = command.spawn().expect("the worker starts");
    let (stdout, stderr) = (
        read_all(child.stdout.take().expect("piped")),
        read_all(child.stderr.take().expect("piped")),
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
    let out = env::var_os(OUT_VARIABLE).map_or_else(
        || {
            env::var_os("CARGO_TARGET_DIR")
                .map_or_else(|| manifest.join("../../target"), PathBuf::from)
                .join("vulkan-scale-evidence")
                .join(format!("lua-{}", std::process::id()))
        },
        PathBuf::from,
    );
    fs::create_dir_all(&out).expect("the evidence directory");
    let (stdout_text, stderr_text) = (
        stdout.clone().unwrap_or_default(),
        stderr.clone().unwrap_or_default(),
    );
    let _ = fs::write(out.join("vulkan-scale-lua.stdout.txt"), &stdout_text);
    let _ = fs::write(out.join("vulkan-scale-lua.stderr.txt"), &stderr_text);

    let mut problems = Vec::new();
    match status {
        None => problems.push("the worker did not finish by the deadline".to_owned()),
        Some(status) if !status.success() => {
            problems.push(format!("the worker exited with {status:?}"));
        }
        Some(_) => {}
    }
    for (name, stream) in [("stdout", &stdout), ("stderr", &stderr)] {
        if let Err(error) = stream {
            problems.push(format!("{name}: {error}"));
        }
    }
    for marker in missing_markers(&stdout_text, &stderr_text) {
        problems.push(format!("worker did not report {marker:?}"));
    }
    if let Some(problem) = layer_problem(&stdout_text, &stderr_text) {
        problems.push(problem);
    }
    let layer_loaded = layer_in_loader_log(&format!("{stdout_text}\n{stderr_text}"));
    let records: Vec<&str> = stdout_text
        .lines()
        .filter(|line| line.contains("metrics: "))
        .collect();
    if records.len() != 1 {
        problems.push(format!(
            "{} metrics records, not exactly one",
            records.len()
        ));
    }
    let record = records.first().copied().unwrap_or("");
    if stdout_text.contains("ERROR") {
        problems.push("the run logged an ERROR".to_owned());
    }
    let script_lines: Vec<&str> = stdout_text
        .lines()
        .filter(|line| line.contains("scale-lua:"))
        .collect();
    // The facts the report needs, as the record states them.
    // The record prints the measured window's duration in microseconds (`duration=4000000us`).
    let duration_us = number_after(record, "; window id=1", "duration=");
    let window_frames = number_after(record, "; window: frames=", "frames=");
    let cpu_samples = number_after(record, "; window: frames=", "cpu_total[n=");
    let gpu_samples = number_after(record, "; window: frames=", "gpu[n=");
    let presentation_intervals =
        number_after(record, "; window: frames=", "presentation_interval[n=");
    // A source the device does not offer is stated in the record with its reason (never a zero).
    let gpu_unavailable = record.contains("gpu: unavailable");
    let presentation_unavailable = record.contains("presentation: unavailable");
    let delivered = number_after(record, "; loop window", "window updates delivered=");
    let missed = number_after(record, "; loop window", "missed=");
    let complete_readings = record.matches("complete=true").count();
    if duration_us.is_none() || window_frames.is_none() {
        problems
            .push("the metrics record has no measured window (the script never marked)".to_owned());
    }
    if complete_readings != 2 {
        problems.push(format!("{complete_readings} of the 2 readings were final"));
    }
    let passed = problems.is_empty();
    let option =
        |value: Option<u64>| value.map_or_else(|| "null".to_owned(), |value| value.to_string());
    let report = format!(
        "{{\"phase\":\"m-lua\",\"gated\":false,\"passed\":{passed},\"seconds\":{seconds:.1},\"window_duration_us\":{},\"window_frames\":{},\"window_cpu_samples\":{},\"window_gpu_samples\":{},\"window_presentation_intervals\":{},\"gpu_unavailable\":{gpu_unavailable},\"presentation_unavailable\":{presentation_unavailable},\"loop_window_updates_delivered\":{},\"loop_window_missed\":{},\"validation_layer_loaded_per_loader_log\":{layer_loaded},\"final_readings\":{complete_readings},\"problems\":[{}],\"script\":[{}],\"metrics_record\":{}}}",
        option(duration_us),
        option(window_frames),
        option(cpu_samples),
        option(gpu_samples),
        option(presentation_intervals),
        option(delivered),
        option(missed),
        problems
            .iter()
            .map(|problem| escape(problem))
            .collect::<Vec<_>>()
            .join(","),
        script_lines
            .iter()
            .map(|line| escape(line))
            .collect::<Vec<_>>()
            .join(","),
        escape(record),
    );
    let _ = fs::write(out.join("vulkan-scale-lua.json"), &report);
    println!(
        "vulkan_scale_lua: {seconds:.1} s; evidence in {}",
        out.display()
    );
    for line in &script_lines {
        println!("vulkan_scale_lua: {line}");
    }
    println!("vulkan_scale_lua: {record}");
    for problem in &problems {
        println!("vulkan_scale_lua: {problem}");
    }
    println!(
        "test vulkan_scale_lua ... {}",
        if passed { "ok" } else { "FAILED" }
    );
    if passed {
        println!("test result: ok. 1 passed; 0 failed");
        ExitCode::SUCCESS
    } else {
        println!("test result: FAILED. 0 passed; 1 failed");
        ExitCode::FAILURE
    }
}
