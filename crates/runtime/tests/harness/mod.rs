//! Fixtures shared by the scenario tests.
//!
//! Every scenario is its own test file, so it runs in its own process and installs its own global
//! `ck_log` logger over an in-memory sink. Scenarios read the records only after a bounded `flush`
//! that must be `Complete`; a lossy or incomplete logger fails the test instead of being retried.
//!
//! # Keeping the logger's writer away from the queue
//!
//! The logger may drop a record when an emitter cannot take the queue lock quickly enough, and a
//! writer that has just been woken is the usual competitor. So that the records of the runtime under
//! test are admitted without that contention, [`Output::run`] first parks the writer inside the
//! sink's `flush`, where it holds no queue lock, and only then runs the runtime:
//!
//! 1. the sink's gate is closed, and a helper thread calls `Logger::flush` on the empty logger. That
//!    request makes the writer thread call the sink's `flush`, which blocks on the closed gate;
//! 2. the test waits (bounded) until the sink reports that its `flush` has begun. No record is
//!    emitted for the fixture, so nothing the logger is allowed to lose is involved;
//! 3. the runtime runs; its records are only queued;
//! 4. the gate opens (also if the test panics), the helper's flush must come back `Complete`, the
//!    helper is joined, and the main thread's own bounded `flush` must be `Complete` too.
//!
//! Only the fixture's waiting is bounded and arranged this way: the assertions on calls, records and
//! outcomes, and the `Complete` flushes, are unchanged, so a lossy or incomplete logger still fails.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use ck_log::{Filter, FlushOutcome, Level, Logger, Settings};
use ck_runtime::{Report, Runtime};

/// The longest the sink holds the writer back, or a test waits for the rendezvous, so that a broken
/// test cannot hang.
const HOLD: Duration = Duration::from_secs(30);
/// The longest the main thread waits for the writer to reach the sink's `flush`.
const RENDEZVOUS: Duration = Duration::from_secs(5);

/// The sink's gate: whether its `flush` may return, and how many flushes have begun.
struct Gate {
    open: bool,
    flush_entered: usize,
}

/// What the sink shares with the test.
struct Shared {
    buffer: Mutex<Vec<u8>>,
    gate: Mutex<Gate>,
    changed: Condvar,
}

impl Shared {
    /// The gate, even if a failing test poisoned it: releasing the writer must never panic again.
    fn lock_gate(&self) -> MutexGuard<'_, Gate> {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Opens the sink's gate when dropped, so that a failing test never leaves the writer held.
struct Release<'a>(&'a Shared);

impl Drop for Release<'_> {
    fn drop(&mut self) {
        self.0.lock_gate().open = true;
        self.0.changed.notify_all();
    }
}

/// The installed logger's in-memory output.
pub(crate) struct Output {
    shared: Arc<Shared>,
    /// How many output lines earlier runs have already accounted for.
    taken: Mutex<usize>,
    /// Makes the next drive fail an assertion right after the runtime ran (a regression probe).
    inject_failure: AtomicBool,
}

/// A sink that collects the bytes written and holds the writer thread inside `flush` while the
/// test keeps the gate closed.
struct Sink(Arc<Shared>);

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .buffer
            .lock()
            .expect("the sink buffer is not poisoned")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let give_up = Instant::now() + HOLD;
        let mut gate = self.0.lock_gate();
        gate.flush_entered += 1;
        self.0.changed.notify_all();
        while !gate.open && Instant::now() < give_up {
            gate = self
                .0
                .changed
                .wait_timeout(gate, Duration::from_millis(50))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        Ok(())
    }
}

/// Installs the process-wide logger over an in-memory sink: every level allowed, no timestamps and
/// no colour, so that records are exactly `LEVEL runtime: message`.
pub(crate) fn install() -> Output {
    let shared = Arc::new(Shared {
        buffer: Mutex::new(Vec::new()),
        gate: Mutex::new(Gate {
            open: true,
            flush_entered: 0,
        }),
        changed: Condvar::new(),
    });
    let settings = Settings {
        filter: Filter::new(Level::Trace),
        timestamps: false,
        colour: false,
    };
    let logger = Logger::new(Box::new(Sink(Arc::clone(&shared))), settings)
        .expect("the writer thread starts");
    ck_log::install(logger).expect("no logger is installed yet in this process");
    Output {
        shared,
        taken: Mutex::new(0),
        inject_failure: AtomicBool::new(false),
    }
}

impl Output {
    /// Makes the next `drive` fail an assertion right after the runtime ran, where the real flush
    /// assertions sit, to prove that a failed harness assertion cannot leave a worker alive.
    pub(crate) fn inject_failure_after_run(&self) {
        self.inject_failure.store(true, Ordering::SeqCst);
    }

    /// Closes the gate and returns how many sink flushes had begun before.
    fn close_gate(&self) -> usize {
        let mut gate = self.shared.lock_gate();
        gate.open = false;
        gate.flush_entered
    }

    /// Waits (bounded) until the writer is inside a sink flush that began after `before`.
    fn wait_for_sink_flush(&self, before: usize) {
        let give_up = Instant::now() + RENDEZVOUS;
        let mut gate = self.shared.lock_gate();
        while gate.flush_entered == before {
            let left = give_up.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the writer never reached the sink's flush");
            gate = self
                .shared
                .changed
                .wait_timeout(gate, left)
                .expect("the gate is not poisoned")
                .0;
        }
    }

    /// Runs `runtime` with the logger's writer parked in the sink's `flush` (see the module
    /// documentation), waits (bounded) for the logger to write everything, and returns the report and
    /// the records written by this run.
    pub(crate) fn drive(&self, runtime: Runtime) -> (Report, Vec<String>) {
        let logger = ck_log::installed().expect("the logger is installed");

        let before = self.close_gate();
        let (report, helper_flush) = thread::scope(|scope| {
            // Opens the gate when this closure ends, normally or by a panic, before the scope joins.
            let release = Release(&self.shared);
            let helper = scope.spawn(|| logger.flush(HOLD));
            self.wait_for_sink_flush(before);
            let report = runtime.run_report();
            drop(release);
            let helper_flush = helper.join().expect("the flushing helper does not panic");
            (report, helper_flush)
        });
        assert!(
            !self.inject_failure.swap(false, Ordering::SeqCst),
            "injected harness failure"
        );
        assert_eq!(
            helper_flush,
            FlushOutcome::Complete,
            "the rendezvous flush must complete once the runtime's records are written"
        );
        assert_eq!(
            logger.flush(Duration::from_secs(5)),
            FlushOutcome::Complete,
            "the logger must write every record"
        );

        let text = String::from_utf8(
            self.shared
                .buffer
                .lock()
                .expect("the sink buffer is not poisoned")
                .clone(),
        )
        .expect("log output is UTF-8");
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let mut taken = self.taken.lock().expect("the offset is not poisoned");
        let records = lines[*taken..].to_vec();
        *taken = lines.len();
        (report, records)
    }
}
