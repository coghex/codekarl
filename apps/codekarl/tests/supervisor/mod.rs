//! Bounded, fail-closed supervision of a child process, for the tests that run one (shared by the
//! application's real-window tests and the failure-boundary self-re-execution, through `#[path]`).
//!
//! The rules (CK-65 fix 1):
//! - the child has a whole-process deadline; on expiry, on a failed poll and on drop it is **killed and
//!   reaped with a bounded poll** (`try_wait` against a budget), never a bare `wait()`;
//! - a failed `kill`, a failed poll and a child that is not reaped within the budget are **reported**, never
//!   ignored;
//! - **the supervisor owns every reader from the moment it is spawned** (a partial spawn failure
//!   included), and its one cleanup ([`Supervised::cleanup`]: finish, a deadline, a failed poll, a pipe
//!   timeout, an assertion or unwind, a drop) visits process *and* readers, even after the process has been
//!   cleared;
//! - a reader signals its **real exit** from a thread-local guard it registers as its very first action:
//!   thread-local destructors run in reverse registration order, so the guard runs after the stream's own
//!   and any other thread-local destructor; the wait for it is bounded by one absolute grace and the thread
//!   is joined only after the signal, when only the OS-level exit remains, so the join cannot be held by the
//!   stream or the test. Neither the end of the read nor `JoinHandle::is_finished` stands in for it (both
//!   are true while a thread-local destructor is still running). A read error is a failure of the case,
//!   never "complete output";
//! - a reader that does not end within the grace (a blocked read cannot be cancelled safely in Rust) is an
//!   explicit, reported failure, and its handle goes to a documented **leak registry** ([`LEAKED`]),
//!   labelled with the test thread that owned it: never a silent drop, never an unbounded join or a
//!   cleanup thread.

use std::io::{self, PipeReader, PipeWriter, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The longest a killed child is polled for before the case fails.
pub(crate) const REAP_BUDGET: Duration = Duration::from_secs(5);
/// The longest the test waits, after the child ended, for each of its pipes to reach the end.
pub(crate) const PIPE_GRACE: Duration = Duration::from_secs(5);
/// The slice of every poll.
const SLICE: Duration = Duration::from_millis(5);

static SPAWN_LOCK: Mutex<()> = Mutex::new(());

/// Holds the executable-wide lock across exactly one pipe or process creation call.
pub(crate) struct SpawnGuard {
    _lock: MutexGuard<'static, ()>,
}

pub(crate) fn spawn_guard() -> SpawnGuard {
    SpawnGuard {
        _lock: SPAWN_LOCK.lock().unwrap_or_else(PoisonError::into_inner),
    }
}

/// The only raw `Command::spawn` call in this executable.
pub(crate) fn spawn_locked(_held: &SpawnGuard, command: &mut Command) -> io::Result<Child> {
    let _ = pipe_locked;
    command.spawn()
}

/// The only raw `std::io::pipe` call in this executable.
pub(crate) fn pipe_locked(_held: &SpawnGuard) -> io::Result<(PipeReader, PipeWriter)> {
    io::pipe()
}

/// What the supervisor needs of a process (a real child, or a fake in the supervisor's own tests).
pub(crate) trait Process {
    /// Polls without waiting.
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
    /// Asks the process to die.
    fn kill(&mut self) -> io::Result<()>;
}

impl Process for Child {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Child::try_wait(self)
    }

    fn kill(&mut self) -> io::Result<()> {
        Child::kill(self)
    }
}

/// Kills `process` and polls for it to be reaped for at most `budget`. Every problem is reported: a failed
/// kill (the poll still runs, since the process may be gone), a failed poll (fail closed at once) and a
/// process that is not reaped in time.
pub(crate) fn kill_and_reap<P: Process>(
    process: &mut P,
    budget: Duration,
) -> Result<ExitStatus, String> {
    let mut problems = Vec::new();
    if let Err(error) = process.kill() {
        problems.push(format!("kill failed: {error}"));
    }
    let give_up = Instant::now() + budget;
    loop {
        match process.try_wait() {
            Ok(Some(status)) if problems.is_empty() => return Ok(status),
            Ok(Some(_)) => return Err(problems.join("; ")),
            Ok(None) => {}
            Err(error) => {
                problems.push(format!("cannot poll the process: {error}"));
                return Err(problems.join("; "));
            }
        }
        if Instant::now() >= give_up {
            problems.push(format!("the process was not reaped within {budget:?}"));
            return Err(problems.join("; "));
        }
        thread::sleep(SLICE);
    }
}

/// Sets a flag when it is dropped: the reader thread's **exit signal**.
struct ExitSignal(Arc<AtomicBool>);

impl Drop for ExitSignal {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

thread_local! {
    /// The reader thread's exit guard. It is registered by the thread's **very first action** (the first
    /// access of a thread-local with a destructor registers that destructor), and thread-local destructors
    /// run in reverse order of registration (verified for this platform by the supervisor's own test), so it
    /// runs **last**: after every thread-local the stream or the reader registers later, whatever those
    /// destructors do. Its drop is the "the thread's own code has fully finished" signal; only the OS-level
    /// thread exit remains after it.
    static EXIT_GUARD: std::cell::RefCell<Option<ExitSignal>> = const { std::cell::RefCell::new(None) };
}

/// One pipe, read by a thread.
pub(crate) struct Reader {
    /// How reading ended, set before the thread exits.
    outcome: Arc<Mutex<Option<Result<(), String>>>>,
    /// Set by the first-registered thread-local guard, i.e. after every thread-local destructor ran.
    exited: Arc<AtomicBool>,
    /// `None` once the thread was joined.
    handle: Option<JoinHandle<()>>,
}

impl Reader {
    /// Starts reading `stream` into `into` as it arrives (decoded lossily, in arrival order with the other
    /// stream when both share `into`).
    pub(crate) fn spawn<R: Read + Send + 'static>(
        stream: R,
        into: Arc<Mutex<String>>,
    ) -> io::Result<Self> {
        Self::start(stream, move |bytes| {
            into.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_str(&String::from_utf8_lossy(bytes));
        })
    }

    /// Starts reading `stream` into `into` as **raw bytes** (nothing decoded, nothing merged with another
    /// stream), under exactly the same ownership, exit signal, grace and leak handling.
    pub(crate) fn spawn_bytes<R: Read + Send + 'static>(
        stream: R,
        into: Arc<Mutex<Vec<u8>>>,
    ) -> io::Result<Self> {
        Self::start(stream, move |bytes| {
            into.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(bytes);
        })
    }

    /// The reader thread: reads `stream` to its end, handing every chunk to `sink`.
    fn start<R: Read + Send + 'static>(
        stream: R,
        mut sink: impl FnMut(&[u8]) + Send + 'static,
    ) -> io::Result<Self> {
        let outcome = Arc::new(Mutex::new(None));
        let result = Arc::clone(&outcome);
        let exited = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&exited);
        let handle = thread::Builder::new()
            .name("supervisor-reader".to_owned())
            .spawn(move || {
                // First of all, before the stream can touch any thread-local: register the exit guard.
                EXIT_GUARD.with(|slot| *slot.borrow_mut() = Some(ExitSignal(signal)));
                let ended = {
                    // The stream lives (and is dropped) in this block.
                    let mut stream = stream;
                    let mut buffer = [0_u8; 4096];
                    loop {
                        match stream.read(&mut buffer) {
                            Ok(0) => break Ok(()),
                            Ok(count) => sink(&buffer[..count]),
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                            Err(error) => break Err(format!("read error: {error}")),
                        }
                    }
                };
                *result.lock().unwrap_or_else(PoisonError::into_inner) = Some(ended);
            })?;
        Ok(Self {
            outcome,
            exited,
            handle: Some(handle),
        })
    }

    /// Whether the thread has not been joined yet.
    pub(crate) fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// Waits at most `grace` for the reader thread's **real exit** (its first-registered thread-local guard
    /// ran, so every thread-local destructor has finished, a panicking thread's included), joins it only
    /// then (only the OS-level exit remains, so the join cannot be held by anything the test or the stream
    /// controls), and gives how reading ended. `Err` for a read error and for a reader that did not exit in
    /// time (it stays owned by `self`: a later call can still finish it).
    pub(crate) fn finish(&mut self, grace: Duration) -> Result<(), String> {
        let give_up = Instant::now() + grace;
        loop {
            if self.handle.is_none() || self.exited.load(Ordering::SeqCst) {
                if let Some(handle) = self.handle.take() {
                    handle
                        .join()
                        .map_err(|_| "a reader thread panicked".to_owned())?;
                }
                return self
                    .outcome
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
                    .unwrap_or_else(|| Err("a reader ended without an outcome".to_owned()));
            }
            if Instant::now() >= give_up {
                return Err(format!("a reader did not exit within {grace:?}"));
            }
            thread::sleep(SLICE);
        }
    }
}

/// The leak registry: readers that did not end within their grace, owned here (labelled with the test
/// thread that owned them) instead of being dropped. A test that provoked one drains its own label.
pub(crate) static LEAKED: Mutex<Vec<(String, Reader)>> = Mutex::new(Vec::new());

/// Moves `readers` into the registry under `label`.
fn leak(label: &str, readers: Vec<Reader>) {
    let mut registry = LEAKED.lock().unwrap_or_else(PoisonError::into_inner);
    registry.extend(readers.into_iter().map(|reader| (label.to_owned(), reader)));
}

/// What a finished child showed.
#[derive(Debug)]
pub(crate) struct Seen {
    /// The exit code, or `None` for a signal.
    pub(crate) code: Option<i32>,
    /// Standard output and standard error, in arrival order.
    pub(crate) stdout: String,
}

/// What a finished child showed, with the two streams kept apart as raw bytes.
#[derive(Debug)]
pub(crate) struct Split {
    /// The exit code, or `None` for a signal.
    pub(crate) code: Option<i32>,
    /// Standard output, byte for byte.
    pub(crate) stdout: Vec<u8>,
    /// Standard error, byte for byte.
    pub(crate) stderr: Vec<u8>,
}

/// The two raw buffers of a supervised process whose streams are read apart: standard output, standard error.
type Buffers = (Arc<Mutex<Vec<u8>>>, Arc<Mutex<Vec<u8>>>);

/// A process under supervision, with every reader it owns. The two graces are public to the tests, which
/// shorten them.
pub(crate) struct Supervised<P: Process = Child> {
    process: Option<P>,
    readers: Vec<Reader>,
    text: Arc<Mutex<String>>,
    /// The raw buffers, when the streams are read apart ([`Supervised::spawn_split`]).
    split: Option<Buffers>,
    started: Instant,
    deadline: Duration,
    /// The one absolute grace for all readers, from the start of a cleanup.
    pub(crate) pipe_grace: Duration,
    /// The bounded poll that reaps the process.
    pub(crate) reap_budget: Duration,
    /// The test thread that owns this (the label of readers it leaks).
    pub(crate) label: String,
}

impl Supervised<Child> {
    /// Spawns `command` with piped output and no input.
    pub(crate) fn spawn(mut command: Command, deadline: Duration) -> Result<Self, String> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child_result = spawn_locked(&spawn_guard(), &mut command);
        drop(command);
        let mut child =
            child_result.map_err(|error| format!("the child did not start: {error}"))?;
        let streams: Vec<Box<dyn Read + Send>> = [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        .collect();
        Self::adopt(
            child,
            streams,
            deadline,
            (PIPE_GRACE, REAP_BUDGET),
            Reader::spawn,
        )
    }

    /// Spawns `command` with piped output and no input, reading standard output and standard error **apart,
    /// as raw bytes**: [`Supervised::finish_split`] gives each stream byte for byte. The same ownership,
    /// deadline, cleanup and leak rules as [`Supervised::spawn`].
    pub(crate) fn spawn_split(command: &mut Command, deadline: Duration) -> Result<Self, String> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn_locked(&spawn_guard(), command)
            .map_err(|error| format!("the child did not start: {error}"))?;
        let streams: Vec<Box<dyn Read + Send>> = [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        .collect();
        Self::adopt_split(
            child,
            streams,
            deadline,
            (PIPE_GRACE, REAP_BUDGET),
            Reader::spawn_bytes,
        )
    }

    /// The child's process id.
    pub(crate) fn id(&self) -> u32 {
        self.process.as_ref().map_or(0, Child::id)
    }
}

impl<P: Process> Supervised<P> {
    /// A supervised process from its parts (also used with a fake process).
    pub(crate) fn from_parts(
        process: P,
        readers: Vec<Reader>,
        text: Arc<Mutex<String>>,
        deadline: Duration,
    ) -> Self {
        Self {
            process: Some(process),
            readers,
            text,
            split: None,
            started: Instant::now(),
            deadline,
            pipe_grace: PIPE_GRACE,
            reap_budget: REAP_BUDGET,
            label: thread::current().name().unwrap_or("unnamed").to_owned(),
        }
    }

    /// Takes ownership of `process` and starts one reader per stream with `spawn`. The supervisor owns
    /// every reader from the moment it exists: if a later reader cannot be started, the earlier ones and the
    /// process are cleaned up (bounded) and the failure is reported.
    pub(crate) fn adopt(
        process: P,
        streams: Vec<Box<dyn Read + Send>>,
        deadline: Duration,
        graces: (Duration, Duration),
        spawn: impl Fn(Box<dyn Read + Send>, Arc<Mutex<String>>) -> io::Result<Reader>,
    ) -> Result<Self, String> {
        let text = Arc::new(Mutex::new(String::new()));
        let mut owned = Self::from_parts(process, Vec::new(), Arc::clone(&text), deadline);
        (owned.pipe_grace, owned.reap_budget) = graces;
        for stream in streams {
            owned.keep(spawn(stream, Arc::clone(&text)))?;
        }
        Ok(owned)
    }

    /// As [`Supervised::adopt`], reading the first stream as standard output and the second as standard
    /// error into separate raw buffers.
    pub(crate) fn adopt_split(
        process: P,
        streams: Vec<Box<dyn Read + Send>>,
        deadline: Duration,
        graces: (Duration, Duration),
        spawn: impl Fn(Box<dyn Read + Send>, Arc<Mutex<Vec<u8>>>) -> io::Result<Reader>,
    ) -> Result<Self, String> {
        let buffers: Buffers = (Arc::default(), Arc::default());
        let mut owned = Self::from_parts(
            process,
            Vec::new(),
            Arc::new(Mutex::new(String::new())),
            deadline,
        );
        (owned.pipe_grace, owned.reap_budget) = graces;
        owned.split = Some((Arc::clone(&buffers.0), Arc::clone(&buffers.1)));
        for (index, stream) in streams.into_iter().enumerate() {
            let into = if index == 0 { &buffers.0 } else { &buffers.1 };
            owned.keep(spawn(stream, Arc::clone(into)))?;
        }
        Ok(owned)
    }

    /// Takes ownership of a started reader. If it could not be started, the earlier readers and the process
    /// are cleaned up (bounded) and the failure is reported.
    fn keep(&mut self, started: io::Result<Reader>) -> Result<(), String> {
        match started {
            Ok(reader) => {
                self.readers.push(reader);
                Ok(())
            }
            Err(error) => {
                let cleanup = self
                    .cleanup()
                    .err()
                    .map_or_else(String::new, |why| format!("; {why}"));
                Err(format!("a reader thread did not start: {error}{cleanup}"))
            }
        }
    }

    /// Everything the child has written so far.
    pub(crate) fn text(&self) -> String {
        self.text
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The one bounded cleanup, on every path: kills and reaps the process if it is still held, then
    /// finishes **every** reader against one absolute grace. A reader that does not end is reported and its
    /// handle moves to the leak registry ([`LEAKED`]); nothing is dropped silently or waited for
    /// without a bound. Every problem is reported.
    pub(crate) fn cleanup(&mut self) -> Result<(), String> {
        let mut problems = Vec::new();
        if let Some(mut process) = self.process.take()
            && let Err(problem) = kill_and_reap(&mut process, self.reap_budget)
        {
            problems.push(problem);
        }
        let give_up = Instant::now() + self.pipe_grace;
        let mut hung = Vec::new();
        for mut reader in std::mem::take(&mut self.readers) {
            if let Err(problem) = reader.finish(give_up.saturating_duration_since(Instant::now())) {
                problems.push(problem);
                if reader.is_running() {
                    hung.push(reader);
                }
            }
        }
        if !hung.is_empty() {
            problems.push(format!(
                "{} reader(s) did not end and were kept in the leak registry",
                hung.len()
            ));
            leak(&self.label, hung);
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    /// `why`, with whatever the cleanup reported.
    fn failed(&mut self, why: &str) -> String {
        match self.cleanup() {
            Ok(()) => format!("{why} (the process was killed and reaped, every reader ended)"),
            Err(problem) => format!("{why}; cleanup failed: {problem}"),
        }
    }

    /// Polls the process; a failed poll is an error, after a cleanup of the process and every reader.
    fn poll(&mut self) -> Result<Option<ExitStatus>, String> {
        let process = self
            .process
            .as_mut()
            .ok_or_else(|| "the process was already reaped".to_owned())?;
        match process.try_wait() {
            Ok(status) => Ok(status),
            Err(error) => Err(self.failed(&format!("cannot poll the process: {error}"))),
        }
    }

    /// Waits until the output contains `needle`; the case fails if the process ends first, the deadline
    /// passes or `within` does.
    pub(crate) fn wait_for(&mut self, needle: &str, within: Duration) -> Result<(), String> {
        let give_up = Instant::now() + within;
        loop {
            if self.text().contains(needle) {
                return Ok(());
            }
            if let Some(status) = self.poll()? {
                let text = self.text();
                return Err(self.failed(&format!(
                    "the process ended ({status}) before {needle:?}:\n{text}"
                )));
            }
            if Instant::now() >= give_up || self.started.elapsed() >= self.deadline {
                let text = self.text();
                return Err(self.failed(&format!("no {needle:?} in time:\n{text}")));
            }
            thread::sleep(SLICE);
        }
    }

    /// Waits for the process to end (killing and reaping it, bounded, at the deadline, which is an error),
    /// then for every reader to end: the exit status. A failed poll, the deadline, a read error, a reader that
    /// does not end and any cleanup problem are errors, each reported after the one bounded cleanup.
    fn wait_and_clean(&mut self) -> Result<ExitStatus, String> {
        let status = loop {
            if let Some(status) = self.poll()? {
                self.process = None;
                break status;
            }
            if self.started.elapsed() >= self.deadline {
                let (text, deadline) = (self.text(), self.deadline);
                return Err(self.failed(&format!(
                    "the process outlived its deadline of {deadline:?}:\n{text}"
                )));
            }
            thread::sleep(SLICE);
        };
        match self.cleanup() {
            Ok(()) => Ok(status),
            Err(problem) => Err(format!("{problem}\n{}", self.text())),
        }
    }

    /// The finished process: its status and everything it wrote (both streams, in arrival order).
    pub(crate) fn finish(mut self) -> Result<Seen, String> {
        let status = self.wait_and_clean()?;
        Ok(Seen {
            code: status.code(),
            stdout: self.text(),
        })
    }

    /// The finished process of a [`Supervised::spawn_split`]: its status and each stream, byte for byte.
    pub(crate) fn finish_split(mut self) -> Result<Split, String> {
        let status = self.wait_and_clean()?;
        let (stdout, stderr) = self.split.take().ok_or("the streams were not read apart")?;
        let bytes = |buffer: Arc<Mutex<Vec<u8>>>| {
            std::mem::take(&mut *buffer.lock().unwrap_or_else(PoisonError::into_inner))
        };
        Ok(Split {
            code: status.code(),
            stdout: bytes(stdout),
            stderr: bytes(stderr),
        })
    }
}

impl<P: Process> Drop for Supervised<P> {
    /// The backstop for every path that leaves something behind (an assertion, an unwind): the same bounded
    /// cleanup, and a failure is reported (a panic in a drop during an unwind would abort, so it is printed
    /// then).
    fn drop(&mut self) {
        if self.process.is_none() && self.readers.is_empty() {
            return;
        }
        if let Err(problem) = self.cleanup() {
            let report = format!("cleanup of a supervised process failed: {problem}");
            if thread::panicking() {
                eprintln!("{report}");
            } else {
                panic!("{report}");
            }
        }
    }
}
