//! SIGINT and SIGTERM, end to end: the built binary runs with a Lua VM that waits on fixture files, and
//! the test sends it signals.
//!
//! Every child has a whole-process deadline and is killed and reaped on every path (a drop guard), before
//! any assertion can fail. A test never waits by sleeping for a state: it waits for a marker file the
//! script writes, or for a record the child logs (`received SIGINT, quitting` proves that the first signal
//! was consumed before the second is sent). The signals are sent with the `kill` command, and a parent
//! that ignores a signal is a shell with `trap '' SIGNAL`, which `exec`s the binary: the ignored
//! disposition is inherited.

use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[expect(
    dead_code,
    reason = "this file starts its own children: it uses the binary's path, the scratch directory and the full pipe"
)]
mod common;

use common::{BINARY, Scratch, full_pipe};

/// The longest any child may run.
const DEADLINE: Duration = Duration::from_secs(30);
/// How long after a signal the child must have ended, when it must end promptly.
const PROMPT: Duration = Duration::from_secs(5);

/// Lua helpers: `touch(path)` writes a marker, `wait_for(path)` polls for one, failing after 5 s.
const HELPERS: &str = r#"
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

fn lua_path(path: &Path) -> String {
    format!("[==[{}]==]", path.to_str().expect("a UTF-8 path"))
}

/// What the `ui` script does.
enum Script {
    /// `startup()` writes `started` and waits for `release` (never given in these tests).
    StartupWaits,
    /// `shutdown()` writes `stopping` and waits for `release`; `stopped` is written at its end.
    ShutdownWaits,
    /// `shutdown()` only writes `stopped`.
    ShutdownMarks,
}

/// A scratch directory with the fixture files of one test.
struct Fixture {
    scratch: Scratch,
}

impl Fixture {
    fn new(script: &Script) -> Self {
        let scratch = Scratch::new();
        let marker = |name: &str| lua_path(&scratch.0.join(name));
        let body = match script {
            Script::StartupWaits => format!(
                "function startup() touch({}) wait_for({}) end\n",
                marker("started"),
                marker("release")
            ),
            Script::ShutdownWaits => format!(
                "function shutdown() touch({}) wait_for({}) touch({}) end\n",
                marker("stopping"),
                marker("release"),
                marker("stopped")
            ),
            Script::ShutdownMarks => {
                format!("function shutdown() touch({}) end\n", marker("stopped"))
            }
        };
        let entry = scratch.file("ui.lua", &format!("{HELPERS}\n{body}"));
        scratch.file(
            "config.toml",
            &format!(
                "[log]\ntimestamps = false\ncolour = \"never\"\n\n\
                 [runtime]\nstart_timeout = 30\nstop_timeout = 30\n\n\
                 [lua.ui]\nentry = '{}'\n",
                entry.to_str().expect("a UTF-8 path")
            ),
        );
        Self { scratch }
    }

    fn config(&self) -> PathBuf {
        self.scratch.0.join("config.toml")
    }

    fn marker(&self, name: &str) -> PathBuf {
        self.scratch.0.join(name)
    }

    /// Creates the marker `name`.
    fn give(&self, name: &str) {
        self.scratch.file(name, "x");
    }

    /// Waits (bounded) until the marker `name` exists.
    fn wait_for(&self, name: &str) {
        let give_up = Instant::now() + DEADLINE;
        while !self.marker(name).exists() {
            assert!(Instant::now() < give_up, "the marker {name} never appeared");
            thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A running `codekarl`, owned from spawn: killed and reaped when dropped.
struct Proc {
    child: Child,
    lines: Receiver<String>,
    seen: Vec<String>,
    readers: Vec<JoinHandle<()>>,
}

/// Which signal a parent ignores before it `exec`s the binary.
#[derive(Clone, Copy)]
enum Ignored {
    Nothing,
    Term,
    Int,
}

impl Proc {
    fn spawn(config: &Path, ignored: Ignored, stdout: Option<Stdio>) -> Self {
        let mut command = match ignored {
            Ignored::Nothing => Command::new(BINARY),
            Ignored::Term | Ignored::Int => {
                let signal = if matches!(ignored, Ignored::Term) {
                    "TERM"
                } else {
                    "INT"
                };
                let mut command = Command::new("sh");
                command
                    .arg("-c")
                    .arg(format!("trap '' {signal}; exec \"$0\" \"$@\""))
                    .arg(BINARY);
                command
            }
        };
        command
            .arg("--config")
            .arg(config)
            .stdin(Stdio::null())
            .stderr(Stdio::piped());
        let own_stdout = stdout.is_none();
        command.stdout(stdout.unwrap_or_else(Stdio::piped));
        let mut child = command.spawn().expect("the binary starts");
        let (sender, lines) = mpsc::channel();
        let mut readers = Vec::new();
        if own_stdout {
            let out = child.stdout.take().expect("piped");
            readers.push(thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
            }));
        }
        let mut err = child.stderr.take().expect("piped");
        readers.push(thread::spawn(move || {
            let mut sink = Vec::new();
            let _ = err.read_to_end(&mut sink);
        }));
        Self {
            child,
            lines,
            seen: Vec::new(),
            readers,
        }
    }

    /// Sends `signal` (`INT` or `TERM`) with the `kill` command.
    fn send(&self, signal: &str) {
        let status = Command::new("kill")
            .args(["-s", signal, &self.child.id().to_string()])
            .status()
            .expect("kill runs");
        assert!(status.success(), "kill -s {signal} failed");
    }

    /// Waits (bounded) for a record containing `needle`; the records read so far are kept in `seen`.
    fn wait_record(&mut self, needle: &str) {
        let give_up = Instant::now() + DEADLINE;
        if self.seen.iter().any(|line| line.contains(needle)) {
            return;
        }
        loop {
            let left = give_up.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    let found = line.contains(needle);
                    self.seen.push(line);
                    if found {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    panic!("no record containing {needle:?}; seen: {:#?}", self.seen)
                }
            }
        }
    }

    /// Waits at most `within` for the child to exit.
    fn exit_within(&mut self, within: Duration) -> ExitStatus {
        let give_up = Instant::now() + within;
        loop {
            if let Some(status) = self.child.try_wait().expect("the child can be polled") {
                self.collect();
                return status;
            }
            assert!(
                Instant::now() < give_up,
                "the child did not exit within {within:?}; seen: {:#?}",
                self.seen
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Reads the rest of the records once the child has exited.
    fn collect(&mut self) {
        while let Ok(line) = self.lines.recv_timeout(Duration::from_millis(500)) {
            self.seen.push(line);
        }
    }

    fn output(&self) -> String {
        self.seen.join("\n")
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

fn code(status: ExitStatus) -> Option<i32> {
    status.code()
}

// ---------------------------------------------------------------------------------------------
// A quit during start-up
// ---------------------------------------------------------------------------------------------

fn quit_during_startup(signal: &str, ignored: Ignored, expected: i32) {
    let fixture = Fixture::new(&Script::StartupWaits);
    let mut child = Proc::spawn(&fixture.config(), ignored, None);
    fixture.wait_for("started");
    let begun = Instant::now();
    child.send(signal);
    let status = child.exit_within(PROMPT);
    assert!(
        begun.elapsed() < Duration::from_secs(10),
        "well before the 30 s start deadline: {:?}",
        begun.elapsed()
    );
    assert_eq!(code(status), Some(expected), "{}", child.output());
    let out = child.output();
    assert!(
        out.contains(&format!("received SIG{signal}, quitting")),
        "{out}"
    );
    assert!(out.contains("quit requested during start-up"), "{out}");
    assert!(!out.contains("start-up complete"), "{out}");
    assert!(!out.contains("ERROR"), "a quit is not an error: {out}");
}

#[test]
fn sigterm_during_startup_quits_promptly_with_143() {
    quit_during_startup("TERM", Ignored::Nothing, 143);
}

#[test]
fn sigint_during_startup_quits_promptly_with_130() {
    quit_during_startup("INT", Ignored::Nothing, 130);
}

#[test]
fn a_parent_that_ignored_sigterm_or_sigint_does_not_stop_the_signal_from_being_seen() {
    quit_during_startup("TERM", Ignored::Term, 143);
    quit_during_startup("INT", Ignored::Int, 130);
}

// ---------------------------------------------------------------------------------------------
// A signal during the stop phase, and the second signal
// ---------------------------------------------------------------------------------------------

#[test]
fn sigint_during_shutdown_lets_the_stop_phase_finish_as_usual_and_exits_130() {
    let fixture = Fixture::new(&Script::ShutdownWaits);
    let mut child = Proc::spawn(&fixture.config(), Ignored::Nothing, None);
    fixture.wait_for("stopping");
    child.send("INT");
    child.wait_record("received SIGINT, quitting");
    // The quit changed nothing: the stop phase is still waiting. Let it finish.
    fixture.give("release");
    let status = child.exit_within(PROMPT);
    assert_eq!(code(status), Some(130), "{}", child.output());
    let out = child.output();
    assert!(
        fixture.marker("stopped").exists(),
        "shutdown() ran to its end"
    );
    assert!(
        out.contains("stopped ui") && out.contains("shutdown complete"),
        "{out}"
    );
    assert!(!out.contains("quit requested"), "{out}");
}

fn two_signals(signal: &str, ignored: Ignored) {
    let fixture = Fixture::new(&Script::ShutdownWaits);
    let mut child = Proc::spawn(&fixture.config(), ignored, None);
    fixture.wait_for("stopping");
    child.send(signal);
    // Proof that the first was consumed, not a sleep: its own record.
    child.wait_record(&format!("received SIG{signal}, quitting"));
    child.send(signal);
    let status = child.exit_within(PROMPT);
    assert_eq!(code(status), Some(3), "{}", child.output());
    assert!(
        !fixture.marker("stopped").exists(),
        "shutdown() never finished"
    );
}

#[test]
fn two_sigints_while_shutdown_waits_end_the_process_at_once_with_3() {
    two_signals("INT", Ignored::Nothing);
}

#[test]
fn two_sigterms_end_the_process_at_once_with_3_also_when_the_parent_ignored_sigterm() {
    two_signals("TERM", Ignored::Nothing);
    two_signals("TERM", Ignored::Term);
    two_signals("INT", Ignored::Int);
}

// ---------------------------------------------------------------------------------------------
// A signal during the final flush
// ---------------------------------------------------------------------------------------------

#[test]
fn a_first_signal_that_arrives_during_the_final_flush_still_gives_130() {
    let fixture = Fixture::new(&Script::ShutdownMarks);
    // Stdout is a pipe that is completely full and that nobody reads: the logger's writer blocks in its
    // first write, so the final flush waits for the test to drain the pipe (it allows 2 s).
    let (reader, writer) = full_pipe();
    let mut child = Proc::spawn(
        &fixture.config(),
        Ignored::Nothing,
        Some(Stdio::from(writer)),
    );
    fixture.wait_for("stopped");
    // `shutdown()` has ended: the child is at, or about to enter, the final flush.
    thread::sleep(Duration::from_millis(200));
    child.send("INT");
    thread::sleep(Duration::from_millis(100));
    let drain = thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = fs::File::from(reader).read_to_end(&mut sink);
    });
    let status = child.exit_within(PROMPT);
    drain.join().expect("the drain ends at the child's exit");
    assert_eq!(
        code(status),
        Some(130),
        "a clean run with a signal during the flush"
    );
}

#[test]
fn a_run_with_no_signal_still_exits_0() {
    let fixture = Fixture::new(&Script::ShutdownMarks);
    let mut child = Proc::spawn(&fixture.config(), Ignored::Nothing, None);
    let status = child.exit_within(DEADLINE);
    assert_eq!(code(status), Some(0), "{}", child.output());
    assert!(!child.output().contains("received"), "{}", child.output());
}
