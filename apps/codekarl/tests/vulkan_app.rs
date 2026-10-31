//! `codekarl` with a real GLFW window and Vulkan, end to end (builds with the `vulkan` feature).
//!
//! The cases need a desktop session, the Vulkan loader and a GPU (`MoltenVK` on macOS): they **fail rather
//! than skip** without them (owner). Every child is supervised with a whole-process deadline, killed and
//! reaped on every path (also when the test panics), and its pipes are read by threads that are waited for
//! with a bound (the shared supervisor).
//!
//! **The owner may be typing while this runs, Escape included** (spec r5, the 2026-10-19 typing rule): every
//! fixture `ui` script registers an `on_close_request` that **declines** and logs the source, no assertion
//! depends on a quiet keyboard, and records are looked for in order with declined close requests allowed
//! between them. The cases close through `window.close()` or a signal.
//!
//! **No dynamic-loader variable is needed**: the program loads the Vulkan loader itself and hands its
//! `vkGetInstanceProcAddr` to GLFW, so every `DYLD_*` and `LD_LIBRARY_PATH` variable is removed from every
//! child's environment here, and the Vulkan validation layer is enabled (the loader's own
//! `VK_INSTANCE_LAYERS`), so a validation message anywhere in the output fails the case. The path's
//! failure-boundary cases (fakes, drop observers, refused destroys) are in `src/vulkan/tests.rs`.
//!
//! The production binary has no test hook: the scripts leave files with Lua's standard `io.open`.

#![cfg(feature = "vulkan")]

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

#[expect(
    dead_code,
    reason = "this file uses the scratch directory and the binary path, not the runner or the full pipe"
)]
mod common;

use common::{BINARY, Scratch};

#[expect(
    dead_code,
    reason = "this file reads its child's output as one text; the raw split capture is the windowed test's"
)]
#[path = "supervisor/mod.rs"]
mod supervisor;
use supervisor::{Seen, Supervised};

/// The longest a window child may run.
const DEADLINE: Duration = Duration::from_secs(60);

/// One window at a time: the cases share the desktop and the keyboard focus.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Lua helpers for the fixture scripts: `touch(path)` leaves a marker file, `wait_for(path)` polls for one
/// in a loop that fails the script after 5 seconds of CPU time.
const LUA_HELPERS: &str = r"
local function touch(path)
  local file = assert(io.open(path, 'w'))
  file:write('x')
  file:close()
end
local function wait_for(path)
  local started = os.clock()
  while true do
    local file = io.open(path, 'r')
    if file then
      file:close()
      return
    end
    if os.clock() - started > 5 then
      error('timed out waiting for ' .. path)
    end
  end
end
";

/// The close callback every windowed fixture registers: a real Escape or close-button press, from an owner
/// who is typing, is declined and logged, so it changes nothing.
const DECLINE: &str = "window.on_close_request(function(source)\n  log.info('declined close request from ' .. source)\n  return false\nend)\n";

/// A scratch directory with a configuration and scripts.
struct Fixture {
    scratch: Scratch,
}

fn lua_path(path: &Path) -> String {
    format!("[==[{}]==]", path.to_str().expect("a UTF-8 path"))
}

impl Fixture {
    fn new() -> Self {
        Self {
            scratch: Scratch::new(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.scratch.0.join(name)
    }

    fn has(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    /// The `ui` script `body` after the helpers and the declining callback, with the shutdown logs.
    fn windowed(&self, extra_config: &str, body: &str) -> PathBuf {
        let ui = format!(
            "{LUA_HELPERS}\n{DECLINE}\n{body}\nfunction shutdown()\n  log.info('ui shutdown')\n  touch({})\nend\n",
            lua_path(&self.path("ui_shutdown"))
        );
        let gameplay = format!(
            "{LUA_HELPERS}\nfunction shutdown()\n  log.info('gameplay shutdown')\n  touch({})\nend\n",
            lua_path(&self.path("gameplay_shutdown"))
        );
        let ui = self.scratch.file("ui.lua", &ui);
        let gameplay = self.scratch.file("gameplay.lua", &gameplay);
        self.scratch.file(
            "config.toml",
            &format!(
                "[log]\ntimestamps = false\ncolour = \"never\"\n\n{extra_config}\n[lua.gameplay]\nentry = '{}'\n\n[lua.ui]\nentry = '{}'\n",
                gameplay.display(),
                ui.display()
            ),
        )
    }

    /// A windowed configuration (glfw-vulkan) running `body`.
    fn vulkan(&self, body: &str) -> PathBuf {
        self.windowed("[window]\nbackend = \"glfw-vulkan\"\n", body)
    }
}

// ---------------------------------------------------------------------------------------------
// Supervision (the shared bounded supervisor: `tests/supervisor`)
// ---------------------------------------------------------------------------------------------

/// The names of the dynamic-loader variables in this process's environment.
fn dynamic_loader_variables() -> Vec<std::ffi::OsString> {
    std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| {
            let name = name.to_string_lossy();
            name.starts_with("DYLD_") || name == "LD_LIBRARY_PATH"
        })
        .collect()
}

/// Starts the binary under the supervisor with no dynamic-loader variable and the validation layer on.
fn start(args: &[&OsStr]) -> Supervised {
    let mut command = Command::new(BINARY);
    command.args(args);
    for name in dynamic_loader_variables() {
        command.env_remove(name);
    }
    command.env("VK_INSTANCE_LAYERS", "VK_LAYER_KHRONOS_validation");
    Supervised::spawn(command, DEADLINE).unwrap_or_else(|error| panic!("{error}"))
}

/// Waits until the child's output contains `needle`.
fn wait_for(live: &mut Supervised, needle: &str, within: Duration) {
    live.wait_for(needle, within)
        .unwrap_or_else(|error| panic!("{error}"));
}

/// Sends SIGTERM to the child (through `kill(1)`, so this file needs no `unsafe`).
fn terminate(live: &Supervised) {
    let sent = Command::new("/bin/kill")
        .args(["-TERM", &live.id().to_string()])
        .status()
        .expect("kill(1) ran");
    assert!(sent.success(), "kill failed: {sent}");
}

/// Waits for the child to end and reads everything it wrote; a deadline, a failed poll, kill or reap and a
/// read error fail the case.
fn finish(live: Supervised) -> Seen {
    live.finish().unwrap_or_else(|error| panic!("{error}"))
}

/// Runs the binary on `config` (and `extra` arguments) to its end.
fn run(config: &Path, extra: &[&str]) -> Seen {
    let mut args: Vec<&OsStr> = vec![OsStr::new("--config"), config.as_os_str()];
    args.extend(extra.iter().map(OsStr::new));
    finish(start(&args))
}

/// Asserts that each needle occurs in `text` after the previous one (other records, such as declined
/// close requests, may sit between them).
fn assert_in_order(text: &str, needles: &[&str]) {
    let mut from = 0;
    for needle in needles {
        match text[from..].find(needle) {
            Some(at) => from += at + needle.len(),
            None => panic!("{needle:?} not found in order after offset {from} in:\n{text}"),
        }
    }
}

/// Asserts that every needle occurs after `after` and before `before`.
fn assert_between(text: &str, after: &str, before: &str, needles: &[&str]) {
    let start = text
        .find(after)
        .unwrap_or_else(|| panic!("{after:?} not found in:\n{text}"));
    let end = text
        .rfind(before)
        .unwrap_or_else(|| panic!("{before:?} not found in:\n{text}"));
    for needle in needles {
        let at = text[start..].find(needle).map(|at| start + at);
        assert!(
            at.is_some_and(|at| at < end),
            "{needle:?} is not between {after:?} and {before:?} in:\n{text}"
        );
    }
}

/// How many times `needle` occurs in `text`.
fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

/// A validation-layer message anywhere in the output fails the case.
fn assert_no_validation(seen: &Seen) {
    assert!(
        !seen.stdout.contains("Validation") && !seen.stdout.contains("VUID"),
        "a validation message:\n{}",
        seen.stdout
    );
}

// ---------------------------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------------------------

#[test]
fn a_window_opened_and_closed_by_the_script_exits_0_and_both_shutdowns_follow_the_close() {
    let _one_window = serial();
    let fixture = Fixture::new();
    let config = fixture.vulkan(
        "window.open{title='close me', width=200, height=150, clear={0.2, 0.3, 0.4}}\nwindow.on_opened(function() window.close() end)",
    );
    let seen = run(&config, &[]);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "runtime: window: glfw-vulkan",
            "runtime: start-up complete",
            "window: window opened",
            "window: window closed",
            "runtime: shutdown complete",
        ],
    );
    assert_between(
        &seen.stdout,
        "window: window closed",
        "runtime: shutdown complete",
        &["ui shutdown", "gameplay shutdown"],
    );
    assert!(
        fixture.has("ui_shutdown") && fixture.has("gameplay_shutdown"),
        "both shutdown() ran"
    );
    assert_eq!(seen.stdout.matches("window: window opened").count(), 1);
    assert!(!seen.stdout.contains("ERROR"), "{}", seen.stdout);
    assert_no_validation(&seen);
}

/// The repository's committed `assets` directory (it holds the demonstration's placeholder image).
fn shipped_assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .canonicalize()
        .expect("the shipped assets directory")
}

/// Milestone 4.5 on a real window: the quad set before `window.open` is drawn by the Vulkan presenter, and the
/// first-frame record names the generation of that write. The fixture registers the declining close callback
/// (a real Escape or close-button press by the owner, who may be typing, changes nothing) and closes the window
/// itself from `on_opened`, once the test has seen the record and says go. The validation layer is on for the
/// child, so a message anywhere in the output fails the case.
#[test]
fn the_demo_placeholder_is_drawn_with_the_quads_generation_in_the_first_frame_and_the_run_closes_cleanly()
 {
    let _one_window = serial();
    let fixture = Fixture::new();
    let go = fixture.path("go");
    // The quad is placed before the window opens (scene generation 1; the attach makes generation 2, which
    // carries it), then the window stays open until the test has seen the first frame's record and says go.
    let ui = format!(
        "render.quad{{image='demo-quad.png', x=10, y=10, width=128, height=128}}\n\
         window.open{{title='quad', width=200, height=150, clear={{0.1, 0.2, 0.3}}}}\n\
         window.on_opened(function()\n  wait_for({})\n  window.close()\nend)",
        lua_path(&go)
    );
    let config = fixture.windowed(
        &format!(
            "[window]\nbackend = \"glfw-vulkan\"\n\n[render]\nfilter = \"nearest\"\nassets = '{}'\n\n\
             [log.categories]\nrender = \"debug\"\n",
            shipped_assets().display()
        ),
        &ui,
    );
    let mut live = start(&[OsStr::new("--config"), config.as_os_str()]);
    wait_for(
        &mut live,
        "render: first frame drawn, generation 2",
        Duration::from_secs(30),
    );
    std::fs::write(&go, "x").expect("the go file");
    let seen = finish(live);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "runtime: start-up complete",
            "render: first frame drawn, generation 2",
            "window: window closed",
            "runtime: shutdown complete",
            "render: 1 scene write(s) accepted, 0 discarded after the run ended",
        ],
    );
    assert_eq!(
        count(&seen.stdout, "window: window opened"),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(&seen.stdout, "render: first frame drawn"),
        1,
        "{}",
        seen.stdout
    );
    assert_eq!(
        count(&seen.stdout, "arrive in milestone 4.5"),
        0,
        "the Vulkan path draws the quad now: {}",
        seen.stdout
    );
    assert!(!seen.stdout.contains("ERROR"), "{}", seen.stdout);
    assert_no_validation(&seen);
}

#[test]
fn sigterm_after_window_opened_shuts_down_cleanly_and_exits_143() {
    let _one_window = serial();
    let fixture = Fixture::new();
    let config =
        fixture.vulkan("window.open{title='wait', width=200, height=150, clear={0, 0, 0}}");
    let mut live = start(&[OsStr::new("--config"), config.as_os_str()]);
    wait_for(&mut live, "window: window opened", Duration::from_secs(30));
    terminate(&live);
    let seen = finish(live);
    assert_eq!(seen.code, Some(143), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "window: window opened",
            "received SIGTERM, quitting",
            "window: window closed",
            "runtime: shutdown complete",
        ],
    );
    assert!(fixture.has("ui_shutdown") && fixture.has("gameplay_shutdown"));
    assert_no_validation(&seen);
}

#[test]
fn backend_glfw_vulkan_over_a_configuration_naming_native_metal_opens_the_glfw_window() {
    let _one_window = serial();
    let fixture = Fixture::new();
    // The configured backend is not the one used, so it need not be available in this build.
    let config = fixture.windowed(
        "[window]\nbackend = \"native-metal\"\n",
        "window.open{title='override', width=200, height=150, clear={0.3, 0.2, 0.1}}\nwindow.on_opened(function() window.close() end)",
    );
    let seen = run(&config, &["--backend", "glfw-vulkan"]);
    assert_eq!(seen.code, Some(0), "{}", seen.stdout);
    assert_in_order(
        &seen.stdout,
        &[
            "runtime: window: glfw-vulkan",
            "window: window opened",
            "window: window closed",
            "runtime: shutdown complete",
        ],
    );
    assert!(!seen.stdout.contains("native-metal"), "{}", seen.stdout);
    assert!(!seen.stdout.contains("ERROR"), "{}", seen.stdout);
    assert_no_validation(&seen);
}

#[test]
fn the_shipped_demonstration_configurations_run_headless() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    for name in [
        "demo-glfw-vulkan.toml",
        "demo-glfw-vulkan-decline.toml",
        "demo-glfw-vulkan-callback-error.toml",
        "demo-quad-vulkan.toml",
    ] {
        let config = root.join(name);
        let text = std::fs::read_to_string(&config).expect("a shipped configuration");
        assert!(text.contains("backend = \"glfw-vulkan\""), "{name}");
        // `--headless` loads the configuration, runs the scripts without a window and ends (the
        // callback-error demonstration fails only when its window opens).
        let seen = finish(start(&[
            OsStr::new("--config"),
            config.as_os_str(),
            OsStr::new("--headless"),
        ]));
        assert_eq!(seen.code, Some(0), "{name}: {}", seen.stdout);
        assert!(
            seen.stdout.contains("runtime: window: none (headless)")
                && seen.stdout.contains("window.open ignored: headless run"),
            "{name}: {}",
            seen.stdout
        );
    }
}
