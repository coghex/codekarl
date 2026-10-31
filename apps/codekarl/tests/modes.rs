//! Windowed or headless, end to end: which runs are headless, the `window:` record, and the errors of a
//! windowed run. `native-metal` is available in builds with the `metal` feature on macOS and is a
//! configuration error otherwise; `glfw-vulkan` is available in builds with the `vulkan` feature and a
//! configuration error otherwise (the default-feature build checks that). A windowed run whose UI never asks
//! for a window creates no native object at all, so it is safe to run anywhere (the cases that open a real
//! window are in `windowed.rs`).
//!
//! Every child has a deadline and is killed and reaped when it expires. The helpers are in `common`.

use std::{ffi::OsStr, path::Path};

#[expect(
    dead_code,
    reason = "this file uses the child runner and the scratch directory, not the full-pipe helper"
)]
mod common;

use common::{CHILD_DEADLINE, Run, Scratch, run_in};

/// Whether this build can open `native-metal`.
const NATIVE: bool = cfg!(all(feature = "metal", target_os = "macos"));
/// Whether this build can open `glfw-vulkan`.
const VULKAN: bool = cfg!(feature = "vulkan");

const FEATURES: &str = match (cfg!(feature = "metal"), cfg!(feature = "vulkan")) {
    (false, false) => "none",
    (true, false) => "metal",
    (false, true) => "vulkan",
    (true, true) => "metal,vulkan",
};

/// A log section that makes the records easy to compare: no timestamps, no colour.
const PLAIN_LOG: &str = "[log]\ntimestamps = false\ncolour = \"never\"\n";

fn run(args: &[&OsStr]) -> Run {
    let run = run_in(None, args, None, CHILD_DEADLINE);
    assert!(!run.timed_out, "the child outlived its deadline: {run:?}");
    run
}

/// Writes `text` as a configuration and runs the binary on it with `extra` arguments.
fn run_config(text: &str, extra: &[&str]) -> (Run, std::path::PathBuf, Scratch) {
    let scratch = Scratch::new();
    let path = scratch.file("config.toml", text);
    let mut args: Vec<&OsStr> = vec![OsStr::new("--config"), path.as_os_str()];
    args.extend(extra.iter().map(OsStr::new));
    let result = run(&args);
    (result, path, scratch)
}

/// The messages of the records, in order, without the level token or a timestamp.
fn messages(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(|line| {
            let (_, rest) = line
                .split_once("INFO  ")
                .or_else(|| line.split_once("ERROR "))
                .unwrap_or(("", line));
            rest.to_owned()
        })
        .collect()
}

/// The records of a clean headless run of `source`.
fn headless_records(source: &str) -> Vec<String> {
    vec![
        format!("runtime: codekarl {} starting", env!("CARGO_PKG_VERSION")),
        format!("config: {source}"),
        format!("runtime: features: {FEATURES}"),
        "runtime: window: none (headless)".to_owned(),
        "runtime: started resources".to_owned(),
        "runtime: start-up complete".to_owned(),
        "runtime: stopped resources".to_owned(),
        "runtime: shutdown complete".to_owned(),
    ]
}

fn assert_headless(run: &Run, source: &str) {
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stderr, "");
    assert_eq!(
        messages(&run.stdout),
        headless_records(source),
        "{}",
        run.stdout
    );
}

#[test]
fn no_configuration_runs_headless_with_the_mode_record_right_after_the_features() {
    let run = run(&[]);
    assert_eq!(run.code, Some(0), "{run:?}");
    // The default settings have timestamps: compare the messages.
    assert_eq!(
        messages(&run.stdout),
        headless_records("built-in defaults"),
        "{}",
        run.stdout
    );
}

#[test]
fn a_configuration_without_a_window_backend_runs_headless() {
    // The window table goes first: a key after `[log]` would belong to the log table.
    for extra in ["", "[window]\n", "window = {}\n"] {
        let text = format!("{extra}{PLAIN_LOG}");
        let (result, path, _scratch) = run_config(&text, &[]);
        assert_headless(&result, &format!("loaded {}", path.display()));
    }
}

#[test]
fn headless_wins_over_a_configured_backend_and_checks_no_availability() {
    let text = format!("{PLAIN_LOG}[window]\nbackend = \"native-metal\"\n");
    let (result, path, _scratch) = run_config(&text, &["--headless"]);
    assert_headless(&result, &format!("loaded {}", path.display()));
    let text = format!("{PLAIN_LOG}[window]\nbackend = \"glfw-vulkan\"\n");
    let (result, path, _scratch) = run_config(&text, &["--headless"]);
    assert_headless(&result, &format!("loaded {}", path.display()));
    // `--headless` with no configuration at all is also headless.
    let result = run(&[OsStr::new("--headless")]);
    assert_eq!(result.code, Some(0), "{result:?}");
    assert_eq!(
        messages(&result.stdout),
        headless_records("built-in defaults")
    );
}

fn assert_one_config_error(run: &Run, message: &str) {
    assert_eq!(run.code, Some(1), "{run:?}");
    assert_eq!(run.stderr, "");
    assert_eq!(run.stdout, format!("ERROR config: {message}\n"), "{run:?}");
}

#[test]
fn a_configured_backend_that_is_not_available_is_a_configuration_error() {
    let mut backends = Vec::new();
    if !NATIVE {
        backends.push("native-metal");
    }
    if !VULKAN {
        backends.push("glfw-vulkan");
    }
    for backend in backends {
        let text = format!("{PLAIN_LOG}[window]\nbackend = \"{backend}\"\n");
        let (result, _, _scratch) = run_config(&text, &[]);
        assert_one_config_error(
            &result,
            &format!("backend {backend} is not available in this build"),
        );
    }
}

#[test]
fn a_windowed_run_whose_ui_never_opens_a_window_creates_no_window_and_exits_0() {
    if !NATIVE {
        return;
    }
    let text = format!("{PLAIN_LOG}[window]\nbackend = \"native-metal\"\n");
    let (result, path, _scratch) = run_config(&text, &[]);
    assert_eq!(result.code, Some(0), "{result:?}");
    assert_eq!(result.stderr, "");
    let mut wanted = headless_records(&format!("loaded {}", path.display()));
    wanted[3] = "runtime: window: native-metal".to_owned();
    assert_eq!(messages(&result.stdout), wanted, "{}", result.stdout);
}

#[test]
fn backend_overrides_the_configuration_and_the_overriding_backend_is_the_one_checked() {
    let text = format!("{PLAIN_LOG}[window]\nbackend = \"native-metal\"\n");
    if VULKAN {
        // Available: the override is the backend the run uses (its UI never opens a window, so no window
        // exists and this is safe to run anywhere).
        let (result, path, _scratch) = run_config(&text, &["--backend", "glfw-vulkan"]);
        assert_eq!(result.code, Some(0), "{result:?}");
        let mut wanted = headless_records(&format!("loaded {}", path.display()));
        wanted[3] = "runtime: window: glfw-vulkan".to_owned();
        wanted.remove(6); // The compatibility backend composes no resource service.
        wanted.remove(4);
        assert_eq!(messages(&result.stdout), wanted, "{}", result.stdout);
    } else {
        // Without the `vulkan` feature (the default-feature build): the availability error, exit 1.
        let (result, _, _scratch) = run_config(&text, &["--backend", "glfw-vulkan"]);
        assert_one_config_error(
            &result,
            "backend glfw-vulkan is not available in this build",
        );
    }
    if !NATIVE {
        let (result, _, _scratch) = run_config(&text, &["--backend", "native-metal"]);
        assert_one_config_error(
            &result,
            "backend native-metal is not available in this build",
        );
    }
}

#[test]
fn backend_with_a_configuration_that_names_no_backend_is_a_configuration_error() {
    for extra in ["", "[window]\n", "window = {}\n"] {
        let text = format!("{extra}{PLAIN_LOG}");
        let (result, _, _scratch) = run_config(&text, &["--backend", "native-metal"]);
        assert_one_config_error(
            &result,
            "--backend given but the configuration has no [window] backend",
        );
    }
}

#[test]
fn window_table_errors_are_configuration_errors_even_under_headless() {
    for extra in [
        "[window]\nbackend = \"metal\"\n",
        "[window]\nbackend = 3\n",
        "[window]\ntitle = \"x\"\n",
        "window = 3\n",
    ] {
        for flags in [&[][..], &["--headless"][..]] {
            let text = format!("{extra}{PLAIN_LOG}");
            let (result, path, _scratch) = run_config(&text, flags);
            assert_eq!(result.code, Some(1), "{extra:?} {flags:?}: {result:?}");
            assert_eq!(result.stdout.lines().count(), 1, "{}", result.stdout);
            assert!(
                result
                    .stdout
                    .starts_with(&format!("ERROR config: {}: line ", path.display())),
                "{}",
                result.stdout
            );
        }
    }
}

#[test]
fn the_new_command_line_errors_and_the_bare_option_word_rule_are_usage_errors_before_anything_starts()
 {
    let missing = Scratch::new();
    let path = missing.0.join("--headless");
    std::fs::write(&path, PLAIN_LOG).expect("a fixture");
    // A bare option word as the `--config` operand is not a path, even when a file of that name exists
    // in the working directory; an explicit path to it is.
    for word in ["--headless", "--backend"] {
        let run = run_in(
            Some(&missing.0),
            &[OsStr::new("--config"), OsStr::new(word)],
            None,
            CHILD_DEADLINE,
        );
        assert_eq!(run.code, Some(1), "{run:?}");
        assert_eq!(run.stdout, "codekarl: unknown arguments; use --help\n");
    }
    let run = run_in(
        Some(&missing.0),
        &[OsStr::new("--config"), OsStr::new("./--headless")],
        None,
        CHILD_DEADLINE,
    );
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_headless(&run, "loaded ./--headless");
    let _: &Path = &path;
}
