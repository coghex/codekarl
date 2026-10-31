//! `codekarl` with Lua VMs, end to end: the built binary is run with fixture configurations and scripts
//! written to a fresh temporary directory, and its output, files and exit status are checked.
//!
//! Every child has a whole-process deadline and is killed and reaped when it expires. Native `print`
//! bytes may land inside a log line (design §8), so content is checked in separate runs: log-only
//! fixtures check the records, print-only fixtures (with `[log] level = "error"`, so a clean run writes
//! no record) check the printed lines, and the mixed runs of the shipped skeletons check the outcome
//! only. The scripts write marker files with Lua's standard `io.open`; the production binary has no
//! test hook.
//!
//! The helpers for running the binary and for the full pipe are in `common`.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fmt::Write as _,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

mod common;

use common::{
    BINARY, CHILD_DEADLINE, Run, STALLED_DEADLINE, Scratch, full_pipe, run_in, spawn_guard,
    spawn_locked,
};

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

/// A configuration file with the given `[lua.<name>]` tables (name, entry path) and `extra` text.
fn config_text(vms: &[(&str, &Path)], extra: &str) -> String {
    let mut text = String::from(extra);
    text.push('\n');
    for (name, entry) in vms {
        writeln!(
            text,
            "[lua.{name}]\nentry = '{}'",
            entry.to_str().expect("a UTF-8 path")
        )
        .expect("a string write");
    }
    text
}

/// A log section that makes the records easy to parse: no timestamps, no colour.
const PLAIN_LOG: &str = "[log]\ntimestamps = false\ncolour = \"never\"\n";

/// Lua helpers for the scripts: `touch(path)` writes a marker file, `wait_for(path)` polls for one in a
/// loop that fails the script after 5 seconds (`os.clock` is CPU time, and the loop spins, so it is a
/// bound and not a clock).
const LUA_HELPERS: &str = r#"
local function touch(path)
  local file = assert(io.open(path, "w"))
  file:write("x")
  file:close()
end
local function wait_for(path)
  local started = os.clock()
  while true do
    local file = io.open(path, "r")
    if file then
      file:close()
      return
    end
    if os.clock() - started > 5 then
      error("timed out waiting for " .. path)
    end
  end
end
"#;

/// The text of a Lua string literal for `path`.
fn lua_path(path: &Path) -> String {
    format!("[==[{}]==]", path.to_str().expect("a UTF-8 path"))
}

/// A scratch directory that also knows where its files are.
struct Fixture {
    scratch: Scratch,
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

    /// Writes a script and returns its path.
    fn script(&self, name: &str, text: &str) -> PathBuf {
        self.scratch.file(name, text)
    }

    /// Whether the marker file `name` exists.
    fn has(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    /// Writes the configuration and runs the binary on it with `deadline`.
    fn run(&self, config: &str, deadline: Duration) -> Run {
        let path = self.scratch.file("config.toml", config);
        run_in(
            None,
            &[OsStr::new("--config"), path.as_os_str()],
            None,
            deadline,
        )
    }
}

/// One record parsed from the output: level token (trimmed), category and message (a multi-line
/// message, a traceback for example, is one record).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rec {
    level: String,
    category: String,
    message: String,
}

/// Parses an output made only of log records written without timestamps. A line that does not start a
/// record continues the previous one.
fn records(stdout: &str) -> Vec<Rec> {
    let mut found: Vec<Rec> = Vec::new();
    for line in stdout.lines() {
        let start = ["ERROR", "WARN ", "INFO ", "DEBUG", "TRACE"]
            .iter()
            .find_map(|level| {
                let rest = line.strip_prefix(level)?.strip_prefix(' ')?;
                let (category, message) = rest.split_once(": ")?;
                let valid = !category.is_empty()
                    && category.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._".contains(&byte)
                    });
                valid.then(|| Rec {
                    level: level.trim().to_owned(),
                    category: category.to_owned(),
                    message: message.to_owned(),
                })
            });
        match (start, found.last_mut()) {
            (Some(record), _) => found.push(record),
            (None, Some(previous)) => {
                previous.message.push('\n');
                previous.message.push_str(line);
            }
            (None, None) => panic!("output starts with a line that is not a record: {line:?}"),
        }
    }
    found
}

/// The messages of the records in `category`, in order.
fn messages(found: &[Rec], category: &str) -> Vec<String> {
    found
        .iter()
        .filter(|record| record.category == category)
        .map(|record| record.message.clone())
        .collect()
}

/// Whether some record has `level` and `category` and a message starting with `prefix`.
fn has_record(found: &[Rec], level: &str, category: &str, prefix: &str) -> bool {
    found.iter().any(|record| {
        record.level == level && record.category == category && record.message.starts_with(prefix)
    })
}

/// Waits (bounded) for the marker file `path` to exist.
fn wait_for_file(path: &Path, bound: Duration) -> bool {
    let give_up = Instant::now() + bound;
    while Instant::now() < give_up {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    path.exists()
}

fn assert_in_time(run: &Run) {
    assert!(!run.timed_out, "the child outlived its deadline: {run:?}");
}

// ---------------------------------------------------------------------------------------------
// Runs that complete
// ---------------------------------------------------------------------------------------------

/// A script that only logs, in its main chunk, `startup()` and `shutdown()`.
fn logging_script(vm: &str) -> String {
    format!(
        "log.info('{vm} main')\nfunction startup() log.info('{vm} startup') end\n\
         function shutdown() log.info('{vm} shutdown') end\n"
    )
}

#[test]
fn log_only_vms_log_in_category_order_and_the_runtime_records_are_in_their_documented_order() {
    let fixture = Fixture::new();
    let ui = fixture.script("ui.lua", &logging_script("ui"));
    let gameplay = fixture.script("gameplay.lua", &logging_script("gameplay"));
    let run = fixture.run(
        &config_text(&[("ui", &ui), ("gameplay", &gameplay)], PLAIN_LOG),
        CHILD_DEADLINE,
    );
    assert_in_time(&run);
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stderr, "");
    let found = records(&run.stdout);
    assert_eq!(
        messages(&found, "lua.ui"),
        ["ui main", "ui startup", "ui shutdown"]
    );
    assert_eq!(
        messages(&found, "lua.gameplay"),
        ["gameplay main", "gameplay startup", "gameplay shutdown"]
    );
    assert_eq!(
        messages(&found, "runtime")[3..],
        [
            "started resources",
            "started gameplay",
            "started ui",
            "start-up complete",
            "stopped ui",
            "stopped gameplay",
            "stopped resources",
            "shutdown complete"
        ],
        "VMs start in name order and stop in reverse: {}",
        run.stdout
    );
    assert!(
        found.iter().all(|record| record.level == "INFO"),
        "{}",
        run.stdout
    );
    assert_eq!(
        found.len(),
        messages(&found, "lua.ui").len()
            + messages(&found, "lua.gameplay").len()
            + messages(&found, "config").len()
            + messages(&found, "runtime").len(),
        "no record in any other category: {}",
        run.stdout
    );
    assert_eq!(messages(&found, "config").len(), 1);
    assert_eq!(messages(&found, "runtime").len(), 11, "{}", run.stdout);
    assert_eq!(
        messages(&found, "runtime")[2],
        "window: none (headless)",
        "the mode record follows `features:`"
    );
}

/// A script that only prints, ordered with its sibling through marker files: in each phase `ui` prints
/// and then creates the phase's marker, and `gameplay` waits for it before printing. (Lua 5.4's `print`
/// writes each argument and its newline in separate C calls, so two VMs printing at once could
/// interleave inside a line.)
fn printing_script(vm: &str, dir: &Path) -> String {
    let marker = |phase: &str| lua_path(&dir.join(format!("phase_{phase}")));
    let step = |phase: &str| {
        if vm == "ui" {
            format!("print('ui: {phase}') touch({})", marker(phase))
        } else {
            format!("wait_for({}) print('gameplay: {phase}')", marker(phase))
        }
    };
    format!(
        "{LUA_HELPERS}\n{}\nfunction startup() {} end\nfunction shutdown() {} end\n",
        step("loaded"),
        step("startup"),
        step("shutdown")
    )
}

#[test]
fn print_only_vms_print_their_lines_exactly_once_and_in_order() {
    let fixture = Fixture::new();
    let ui = fixture.script("ui.lua", &printing_script("ui", &fixture.scratch.0));
    let gameplay = fixture.script(
        "gameplay.lua",
        &printing_script("gameplay", &fixture.scratch.0),
    );
    let run = fixture.run(
        &config_text(
            &[("ui", &ui), ("gameplay", &gameplay)],
            "[log]\nlevel = \"error\"\n",
        ),
        CHILD_DEADLINE,
    );
    assert_in_time(&run);
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stderr, "");
    assert_eq!(
        run.stdout,
        "ui: loaded\ngameplay: loaded\nui: startup\ngameplay: startup\nui: shutdown\ngameplay: shutdown\n"
    );
}

#[test]
fn a_vm_sees_the_whole_configuration_file_as_its_config() {
    let fixture = Fixture::new();
    let ui = fixture.script(
        "ui.lua",
        "log.info(string.format('%s %s %s', config.log.colour, config.runtime.stop_timeout, config.lua.ui.entry))\n",
    );
    let run = fixture.run(
        &config_text(
            &[("ui", &ui)],
            &format!("{PLAIN_LOG}[runtime]\nstop_timeout = 2.5\n"),
        ),
        CHILD_DEADLINE,
    );
    assert_in_time(&run);
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(
        messages(&records(&run.stdout), "lua.ui"),
        [format!("never 2.5 {}", ui.display())],
        "{}",
        run.stdout
    );
}

/// The repository root, where `config/` and `scripts/` live.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root exists")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("a directory");
    for entry in fs::read_dir(from).expect("a readable directory") {
        let entry = entry.expect("an entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("a copied file");
        }
    }
}

#[test]
fn the_shipped_skeletons_run_clean_from_another_working_directory() {
    let fixture = Fixture::new();
    let root = fixture.path("tree");
    copy_tree(&repository().join("config"), &root.join("config"));
    copy_tree(&repository().join("scripts"), &root.join("scripts"));
    let elsewhere = fixture.path("elsewhere");
    fs::create_dir_all(&elsewhere).expect("a working directory");
    let config = root.join("config/codekarl.toml");
    let run = run_in(
        Some(&elsewhere),
        &[
            OsStr::new("--config"),
            config.as_os_str(),
            OsStr::new("--headless"),
        ],
        None,
        CHILD_DEADLINE,
    );
    assert_in_time(&run);
    // Outcome only: print and log output may share lines.
    assert_eq!(run.code, Some(0), "{run:?}");
    assert!(
        run.stdout.contains("shutdown complete"),
        "relative entries resolved against the configuration's directory: {}",
        run.stdout
    );
    assert!(!run.stdout.contains("ERROR"), "{}", run.stdout);
    for line in [
        "ui: loaded",
        "gameplay: loaded",
        "ui: shutdown",
        "gameplay: shutdown",
    ] {
        assert!(run.stdout.contains(line), "{line:?} in {}", run.stdout);
    }
}

#[test]
fn without_a_configuration_no_vm_runs_and_the_records_are_milestone_1s() {
    let fixture = Fixture::new();
    let run = run_in(Some(&fixture.scratch.0), &[], None, CHILD_DEADLINE);
    assert_in_time(&run);
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stdout.lines().count(), 8, "{}", run.stdout);
    for line in run.stdout.lines() {
        assert!(!line.contains("lua"), "{line}");
        assert!(!line.contains("started ui"), "{line}");
        assert!(!line.contains("started gameplay"), "{line}");
    }
    assert!(run.stdout.contains("started resources"), "{}", run.stdout);
    assert!(run.stdout.contains("stopped resources"), "{}", run.stdout);
    // The same for a configuration without a Lua table.
    let run = fixture.run(
        &format!("{PLAIN_LOG}[runtime]\nstop_timeout = 1\n"),
        CHILD_DEADLINE,
    );
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stdout.lines().count(), 8, "{}", run.stdout);
    assert!(!run.stdout.contains("lua"), "{}", run.stdout);
}

// ---------------------------------------------------------------------------------------------
// Start failures: exit 1, and no `shutdown()` runs
// ---------------------------------------------------------------------------------------------

/// A script whose `shutdown()` leaves the marker `name` behind.
fn marker_script(extra: &str, dir: &Path, name: &str) -> String {
    format!(
        "{LUA_HELPERS}\n{extra}\nfunction shutdown() touch({}) end\n",
        lua_path(&dir.join(name))
    )
}

#[test]
fn a_syntax_error_in_ui_stops_both_with_one_record_and_never_runs_a_shutdown() {
    let fixture = Fixture::new();
    let ui = fixture.script("ui_main.lua", "this is not lua\n");
    let gameplay = fixture.script(
        "gameplay.lua",
        &marker_script("", &fixture.scratch.0, "gameplay_shutdown"),
    );
    let run = fixture.run(
        &config_text(&[("ui", &ui), ("gameplay", &gameplay)], PLAIN_LOG),
        CHILD_DEADLINE,
    );
    assert_in_time(&run);
    assert_eq!(run.code, Some(1), "{run:?}");
    let found = records(&run.stdout);
    let failed: Vec<&Rec> = found
        .iter()
        .filter(|record| record.message.starts_with("ui failed to start: "))
        .collect();
    assert_eq!(failed.len(), 1, "exactly one record: {}", run.stdout);
    assert_eq!(
        (failed[0].level.as_str(), failed[0].category.as_str()),
        ("ERROR", "runtime")
    );
    assert!(
        failed[0].message.contains("ui_main.lua"),
        "{}",
        failed[0].message
    );
    assert!(
        failed[0].message.contains("stack traceback"),
        "the traceback is in the same record: {}",
        failed[0].message
    );
    assert!(
        !fixture.has("gameplay_shutdown"),
        "gameplay's shutdown() must never run"
    );
    assert!(run.stdout.contains("stopped resources"), "{}", run.stdout);
    assert!(run.stdout.contains("shutdown complete"), "{}", run.stdout);
    assert!(!run.stdout.contains("stopped ui"), "{}", run.stdout);
    assert!(!run.stdout.contains("stopped gameplay"), "{}", run.stdout);
}

#[test]
fn an_error_in_gameplays_startup_stops_both_and_neither_shutdown_runs() {
    let fixture = Fixture::new();
    let ui = fixture.script(
        "ui.lua",
        &marker_script("", &fixture.scratch.0, "ui_shutdown"),
    );
    let gameplay = fixture.script(
        "gameplay.lua",
        &marker_script(
            "function startup() error('gameplay startup failed') end",
            &fixture.scratch.0,
            "gameplay_shutdown",
        ),
    );
    let run = fixture.run(
        &config_text(&[("ui", &ui), ("gameplay", &gameplay)], PLAIN_LOG),
        CHILD_DEADLINE,
    );
    assert_in_time(&run);
    assert_eq!(run.code, Some(1), "{run:?}");
    let found = records(&run.stdout);
    assert!(
        found.iter().any(|record| record.level == "ERROR"
            && record.category == "runtime"
            && record.message.starts_with("gameplay failed to start: ")
            && record.message.contains("gameplay startup failed")),
        "{}",
        run.stdout
    );
    assert!(
        !fixture.has("ui_shutdown"),
        "ui's shutdown() must never run"
    );
    assert!(
        !fixture.has("gameplay_shutdown"),
        "gameplay's shutdown() must never run"
    );
}

#[test]
fn a_missing_entry_file_is_a_start_failure_that_names_the_path() {
    let fixture = Fixture::new();
    let missing = fixture.path("no/such/ui.lua");
    let run = fixture.run(&config_text(&[("ui", &missing)], PLAIN_LOG), CHILD_DEADLINE);
    assert_in_time(&run);
    assert_eq!(run.code, Some(1), "{run:?}");
    let found = records(&run.stdout);
    assert!(
        found.iter().any(|record| record.level == "ERROR"
            && record.message.starts_with("ui failed to start: ")
            && record.message.contains(missing.to_str().expect("UTF-8"))),
        "{}",
        run.stdout
    );
    assert!(
        !run.stdout.contains("ERROR config:"),
        "not a configuration error: {}",
        run.stdout
    );
}

#[test]
fn the_start_timeout_from_the_configuration_ends_a_looping_main_chunk_with_status_3() {
    let fixture = Fixture::new();
    let ui = fixture.script("ui.lua", "while true do end\n");
    let run = fixture.run(
        &config_text(
            &[("ui", &ui)],
            &format!("{STAMPED_LOG}[runtime]\nstart_timeout = 0.3\n"),
        ),
        STALLED_DEADLINE,
    );
    assert_in_time(&run);
    // The `window:` record is the runtime's last before its start phase begins waiting on the deadline.
    let waited = interval(
        &run.stdout,
        "INFO  runtime: window:",
        "ERROR runtime: ui missed its start deadline",
    );
    assert_configured_deadline(waited, 300, &run.stdout);
    // A start failure is exit 1, but an owner that missed a deadline makes the shutdown incomplete.
    assert_eq!(run.code, Some(3), "{run:?}");
    let found = records(&unstamped(&run.stdout));
    assert!(
        has_record(&found, "ERROR", "runtime", "ui missed its start deadline"),
        "{}",
        run.stdout
    );
}

#[test]
fn the_cleanup_timeout_from_the_configuration_bounds_the_wait_for_an_uninterruptible_start() {
    let fixture = Fixture::new();
    // The main chunk collects garbage whose finalizer never returns: hooks are off in `__gc`, so the
    // cancel after the start deadline cannot interrupt it, and the cleanup wait runs out.
    let ui = fixture.script(
        "ui.lua",
        "local garbage = setmetatable({}, {__gc = function() while true do end end})\n\
         garbage = nil\ncollectgarbage()\ncollectgarbage()\n",
    );
    // Timestamps on: the cleanup wait is measured between the two records that bracket it, so the
    // process start, the configuration and the exit (slow under a loaded batch) do not count.
    let run = fixture.run(
        &config_text(
            &[("ui", &ui)],
            "[log]\ntimestamps = true\ncolour = \"never\"\n\
             [runtime]\nstart_timeout = 0.2\ncleanup_timeout = 0.2\n",
        ),
        STALLED_DEADLINE,
    );
    assert_in_time(&run);
    assert_eq!(run.code, Some(3), "{run:?}");
    let missed = stamped(&run.stdout, "ERROR runtime: ui missed its start deadline");
    let unfinished = stamped(&run.stdout, "ERROR runtime: ui did not finish");
    let (Some(missed), Some(unfinished)) = (missed, unfinished) else {
        panic!("both records, each with a timestamp: {}", run.stdout);
    };
    // The start record is made before the cleanup starts and the other after its deadline expired.
    // With the 1 s default the interval is never under 1 s.
    let interval = unfinished - missed;
    assert!(
        (190..900).contains(&interval),
        "the configured 0.2 s cleanup wait, not the 1 s default: {interval} ms\n{}",
        run.stdout
    );
}

/// The time, in milliseconds since the Unix epoch, of the first timestamped line whose text after the
/// timestamp starts with `record` (`ck-log`'s `YYYY-MM-DDTHH:MM:SS.mmmZ` stamp, made when the record is).
fn stamped(stdout: &str, record: &str) -> Option<i64> {
    stdout.lines().find_map(|line| {
        let (stamp, rest) = line.split_once(' ')?;
        rest.starts_with(record).then(|| epoch_millis(stamp))?
    })
}

/// Parses a `YYYY-MM-DDTHH:MM:SS.mmmZ` UTC timestamp into milliseconds since the Unix epoch.
fn epoch_millis(stamp: &str) -> Option<i64> {
    let bytes = stamp.as_bytes();
    if bytes.len() != 24 || bytes[23] != b'Z' {
        return None;
    }
    let number = |from: usize, to: usize| stamp.get(from..to)?.parse::<i64>().ok();
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second, milli) = (
        number(11, 13)?,
        number(14, 16)?,
        number(17, 19)?,
        number(20, 23)?,
    );
    // Days from the civil date (Howard Hinnant's algorithm), so a run across midnight still subtracts.
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let year_of_era = shifted - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some((((days * 24 + hour) * 60 + minute) * 60 + second) * 1000 + milli)
}

/// A log section with timestamps on (and no colour), for the cases that time a deadline between two records.
const STAMPED_LOG: &str = "[log]\ntimestamps = true\ncolour = \"never\"\n";

/// The output with each line's leading timestamp removed, so `records` can parse a timestamped run.
fn unstamped(stdout: &str) -> String {
    let mut out = String::new();
    for line in stdout.lines() {
        let rest = line
            .split_once(' ')
            .filter(|(stamp, _)| epoch_millis(stamp).is_some())
            .map_or(line, |(_, rest)| rest);
        out.push_str(rest);
        out.push('\n');
    }
    out
}

/// Milliseconds from the first record starting with `from` to the first starting with `to` (both after the
/// timestamp), with both records present; the runtime stamps a record when it is made.
fn interval(stdout: &str, from: &str, to: &str) -> i64 {
    let (Some(start), Some(end)) = (stamped(stdout, from), stamped(stdout, to)) else {
        panic!("both {from:?} and {to:?}, each with a timestamp: {stdout}");
    };
    end - start
}

/// The interval shows the configured deadline, not the 5 s default: at least the configured value (less the
/// records' millisecond rounding) and under half the default. Process start-up and exit do not count.
fn assert_configured_deadline(interval_ms: i64, configured_ms: i64, stdout: &str) {
    assert!(
        (configured_ms - 10..2500).contains(&interval_ms),
        "the configured {configured_ms} ms deadline, not the 5 s default: {interval_ms} ms\n{stdout}"
    );
}

#[test]
fn timestamps_parse_to_epoch_milliseconds() {
    // The fixed instants `ck-log`'s own formatting tests use.
    assert_eq!(epoch_millis("1970-01-01T00:00:01.999Z"), Some(1_999));
    assert_eq!(
        epoch_millis("2026-01-01T00:00:00.000Z"),
        Some(1_767_225_600_000)
    );
    assert_eq!(
        epoch_millis("2000-02-29T00:00:00.000Z"),
        Some(951_782_400_000)
    );
    assert_eq!(
        epoch_millis("2100-03-01T00:00:00.000Z"),
        Some(4_107_542_400_000)
    );
    assert_eq!(epoch_millis("2026-01-01T00:00:00.000"), None);
    assert_eq!(
        stamped(
            "2026-01-01T00:00:00.250Z ERROR runtime: a b\n",
            "ERROR runtime: a"
        ),
        Some(1_767_225_600_250)
    );
}

// ---------------------------------------------------------------------------------------------
// Missed deadlines: exit 3 through the immediate exit
// ---------------------------------------------------------------------------------------------

#[test]
fn a_shutdown_that_loops_forever_misses_its_deadline_recovers_and_still_exits_with_status_3() {
    let fixture = Fixture::new();
    let ui = fixture.script("ui.lua", "function shutdown() while true do end end\n");
    let gameplay = fixture.script(
        "gameplay.lua",
        &marker_script("", &fixture.scratch.0, "gameplay_shutdown"),
    );
    let run = fixture.run(
        &config_text(
            &[("ui", &ui), ("gameplay", &gameplay)],
            &format!("{STAMPED_LOG}[runtime]\nstop_timeout = 0.2\n"),
        ),
        STALLED_DEADLINE,
    );
    assert_in_time(&run);
    // With no run phase, the stop phase follows `start-up complete` at once.
    let waited = interval(
        &run.stdout,
        "INFO  runtime: start-up complete",
        "ERROR runtime: ui missed its stop deadline",
    );
    assert_configured_deadline(waited, 200, &run.stdout);
    assert_eq!(run.code, Some(3), "{run:?}");
    let found = records(&unstamped(&run.stdout));
    assert!(
        has_record(&found, "ERROR", "runtime", "ui missed its stop deadline"),
        "{}",
        run.stdout
    );
    assert!(fixture.has("gameplay_shutdown"));
}

#[test]
fn an_uninterruptible_vm_exits_with_status_3_and_leaves_its_sibling_stopped() {
    let fixture = Fixture::new();
    let ui = fixture.script(
        "ui.lua",
        "function shutdown()\n  local garbage = setmetatable({}, {__gc = function() while true do end end})\n\
         \x20 garbage = nil\n  collectgarbage()\n  collectgarbage()\nend\n",
    );
    let gameplay = fixture.script(
        "gameplay.lua",
        &marker_script("", &fixture.scratch.0, "gameplay_shutdown"),
    );
    let run = fixture.run(
        &config_text(
            &[("ui", &ui), ("gameplay", &gameplay)],
            &format!("{STAMPED_LOG}[runtime]\nstop_timeout = 0.5\n"),
        ),
        STALLED_DEADLINE,
    );
    assert_in_time(&run);
    let waited = interval(
        &run.stdout,
        "INFO  runtime: start-up complete",
        "ERROR runtime: ui missed its stop deadline",
    );
    assert_configured_deadline(waited, 500, &run.stdout);
    assert_eq!(run.code, Some(3), "{run:?}");
    let found = records(&unstamped(&run.stdout));
    assert!(
        has_record(&found, "ERROR", "runtime", "ui missed its stop deadline"),
        "{}",
        run.stdout
    );
    assert!(
        has_record(&found, "ERROR", "runtime", "ui did not finish"),
        "{}",
        run.stdout
    );
    // `ui` used up the whole stop phase, so `stopped gameplay` is logged after its deadline: the runtime
    // makes one non-waiting attempt, and under load the logger may count the record as dropped instead
    // (`WARN log: N record(s) dropped`). Either way gameplay was stopped, and never reported as late.
    let stopped = has_record(&found, "INFO", "runtime", "stopped gameplay");
    let dropped = found
        .iter()
        .any(|record| record.category == "log" && record.message.contains("dropped"));
    assert!(stopped || dropped, "{}", run.stdout);
    assert!(
        !found
            .iter()
            .any(|record| record.message.starts_with("gameplay ")),
        "no 'gameplay missed its stop deadline' or 'gameplay did not finish': {}",
        run.stdout
    );
    assert!(fixture.has("gameplay_shutdown"));
}

// ---------------------------------------------------------------------------------------------
// A stdout that blocks: exit 3 within the deadline
// ---------------------------------------------------------------------------------------------

#[test]
fn a_print_blocked_inside_c_stdout_ends_the_process_with_status_3_and_gameplays_shutdown_runs() {
    let fixture = Fixture::new();
    let dir = &fixture.scratch.0;
    let ui = fixture.script(
        "ui.lua",
        &format!(
            "{LUA_HELPERS}\nfunction shutdown()\n  touch({})\n  print('blocked')\n  touch({})\nend\n",
            lua_path(&dir.join("ui_before_print")),
            lua_path(&dir.join("ui_after_print"))
        ),
    );
    let gameplay = fixture.script("gameplay.lua", &marker_script("", dir, "gameplay_shutdown"));
    let config = fixture.scratch.file(
        "config.toml",
        &config_text(
            &[("ui", &ui), ("gameplay", &gameplay)],
            "[log]\nlevel = \"error\"\n[runtime]\nstop_timeout = 0.5\n",
        ),
    );
    let (reader, writer) = full_pipe();
    let run = run_in(
        None,
        &[OsStr::new("--config"), config.as_os_str()],
        Some(Stdio::from(writer)),
        STALLED_DEADLINE,
    );
    assert!(
        !run.timed_out,
        "a normal exit would block in the C library's stdout cleanup: {run:?}"
    );
    // The status 3 is the immediate exit: a process blocked in C stdout cannot exit normally.
    assert_eq!(run.code, Some(3), "{run:?}");
    assert!(fixture.has("ui_before_print"), "ui's shutdown() ran");
    assert!(!fixture.has("ui_after_print"), "the print never returned");
    // Only the callback ran: whether gameplay's own thread finished can depend on the shared stdout.
    assert!(
        fixture.has("gameplay_shutdown"),
        "gameplay's stop request was sent and its shutdown() ran"
    );
    drop(reader);
}

#[test]
fn output_left_in_the_c_stdout_buffer_blocks_the_owners_own_flush_and_the_exit_is_status_3() {
    const FRAGMENT: &str = "BUFFERED-FRAGMENT";
    let fixture = Fixture::new();
    let dir = &fixture.scratch.0;
    let ui = fixture.script(
        "ui.lua",
        &format!(
            "{LUA_HELPERS}\nio.stdout:setvbuf('full')\nfunction shutdown()\n  io.stdout:write('{FRAGMENT}')\n  touch({})\nend\n",
            lua_path(&dir.join("ui_wrote"))
        ),
    );
    let gameplay = fixture.script("gameplay.lua", &marker_script("", dir, "gameplay_shutdown"));
    let config = fixture.scratch.file(
        "config.toml",
        &config_text(
            &[("ui", &ui), ("gameplay", &gameplay)],
            "[log]\nlevel = \"error\"\n[runtime]\nstop_timeout = 0.3\n",
        ),
    );
    let (reader, writer) = full_pipe();
    let mut command = Command::new(BINARY);
    command
        .arg("--config")
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::null());
    let mut child = spawn_locked(&spawn_guard(), &mut command).expect("the binary starts");
    drop(command);
    let give_up = Instant::now() + STALLED_DEADLINE;
    // shutdown() returned without blocking: the bytes are in the C stdout buffer, since a write to the
    // full pipe would have blocked. Nothing in the script can block after this marker (level "error"
    // keeps `log.*` from queueing), so what holds the thread is the owner's own flush at the end.
    let wrote = wait_for_file(&dir.join("ui_wrote"), STALLED_DEADLINE);
    // Hold the pipe full until the stop phase is over: the owner's flush is still blocked, so `ui`
    // misses its 0.3 s stop deadline. The final log flush gets 2 s, so draining after 1 s leaves time.
    thread::sleep(Duration::from_secs(1));
    // Now let the flush through: the fragment reaches the reader only because the owner flushed it, and
    // the logger can write what it was holding, so the log flush is complete.
    let drained = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = fs::File::from(reader).read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let status = loop {
        if let Some(status) = child.try_wait().expect("the child can be polled") {
            break Some(status);
        }
        if Instant::now() >= give_up {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(5));
    };
    let output = drained
        .join()
        .expect("the reader ends once the child has exited");
    assert!(wrote, "ui's shutdown() never wrote its marker");
    let status = status.expect("the child exited within its deadline and was not killed");
    assert_eq!(status.code(), Some(3), "{status:?} {output:?}");
    assert!(
        output.contains(FRAGMENT),
        "the owner's flush delivered the fragment: {output:?}"
    );
    assert!(fixture.has("gameplay_shutdown"));
}

// ---------------------------------------------------------------------------------------------
// Configuration errors: one `ERROR config:` record and exit 1
// ---------------------------------------------------------------------------------------------

#[test]
fn every_lua_and_runtime_configuration_error_gives_one_config_record_and_status_1() {
    let cases: BTreeMap<&str, &str> = BTreeMap::from([
        (
            "unknown key in a VM",
            "[lua.ui]\nentry = 'm.lua'\nextra = 1\n",
        ),
        ("missing entry", "[lua.ui]\n"),
        ("non-string entry", "[lua.ui]\nentry = 7\n"),
        ("uppercase name", "[lua.UI]\nentry = 'm.lua'\n"),
        ("dash in name", "[lua.'a-b']\nentry = 'm.lua'\n"),
        ("lua is not a table", "lua = 3\n"),
        ("lua is an array", "lua = [1]\n"),
        ("a VM is not a table", "[lua]\nui = 'm.lua'\n"),
        ("a VM is an array", "[lua]\nui = ['m.lua']\n"),
        ("unknown runtime key", "[runtime]\ntimeout = 5\n"),
        ("runtime is an array", "runtime = [5]\n"),
        ("string timeout", "[runtime]\nstart_timeout = '5'\n"),
        ("NaN timeout", "[runtime]\nstop_timeout = nan\n"),
        ("infinite timeout", "[runtime]\ncleanup_timeout = inf\n"),
        ("zero timeout", "[runtime]\nstart_timeout = 0\n"),
        ("negative timeout", "[runtime]\nstop_timeout = -1\n"),
        ("timeout above 60", "[runtime]\ncleanup_timeout = 60.5\n"),
    ]);
    let fixture = Fixture::new();
    for (what, text) in cases {
        let run = fixture.run(text, CHILD_DEADLINE);
        assert_in_time(&run);
        assert_eq!(run.code, Some(1), "{what}: {run:?}");
        let lines: Vec<&str> = run.stdout.lines().collect();
        assert_eq!(lines.len(), 1, "{what}: exactly one record: {}", run.stdout);
        assert!(
            lines[0].starts_with("ERROR config: "),
            "{what}: {}",
            run.stdout
        );
        assert!(
            lines[0].contains("config.toml"),
            "{what} names the file: {}",
            run.stdout
        );
    }
}
