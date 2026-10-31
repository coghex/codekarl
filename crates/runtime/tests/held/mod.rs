//! Fixtures for the full-queue tests: a logger whose queue is full behind a held sink.
//!
//! Each test file is its own process and installs its own global `ck_log` logger. The logger has a
//! capacity of 1 and a sink that blocks in its first write until the fixture opens its gate, so the
//! writer holds one record and the queue holds another: the queue is full and nothing drains.
//!
//! The fixture reaches that state without a sleep. The first record is emitted and the fixture waits
//! until the sink reports that its write has begun (the writer then holds the record and the queue is
//! empty); the second record then fills the queue. The runtime under test runs on a helper thread so
//! that the test has an overall deadline: a regression that made the runtime wait for space would fail
//! the deadline instead of hanging. The gate is opened by a drop guard on every path, which lets the
//! writer drain and releases any thread still waiting for space.

use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use ck_log::{Filter, FlushOutcome, Level, Logger, Settings};
use ck_runtime::{Outcome, Report, Runtime, Service, ServiceError};

/// The longest a test waits for something that must happen.
const BOUND: Duration = Duration::from_secs(5);
/// The longest the sink holds the writer back, so that a broken test cannot hang.
const HOLD: Duration = Duration::from_secs(30);

/// A service that records its calls; `fail_start` makes its start fail.
pub(crate) struct Spec {
    pub(crate) name: &'static str,
    pub(crate) fail_start: bool,
}

/// What one run of the runtime did, and how long it took.
pub(crate) struct Run {
    pub(crate) outcome: Outcome,
    /// The service calls made, as `start <name>` and `stop <name>`, in order.
    pub(crate) calls: Vec<String>,
    /// How long `Runtime::run` took.
    pub(crate) elapsed: Duration,
}

struct Gate {
    open: bool,
    writes_entered: usize,
}

struct Shared {
    buffer: Mutex<Vec<u8>>,
    gate: Mutex<Gate>,
    changed: Condvar,
}

impl Shared {
    fn lock_gate(&self) -> MutexGuard<'_, Gate> {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn open(&self) {
        self.lock_gate().open = true;
        self.changed.notify_all();
    }
}

/// Opens the sink's gate when dropped, so that a failing test never leaves a thread blocked.
struct OpenOnDrop<'a>(&'a Shared);

impl Drop for OpenOnDrop<'_> {
    fn drop(&mut self) {
        self.0.open();
    }
}

struct Sink(Arc<Shared>);

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let give_up = Instant::now() + HOLD;
        let mut gate = self.0.lock_gate();
        gate.writes_entered += 1;
        self.0.changed.notify_all();
        while !gate.open && Instant::now() < give_up {
            gate = self
                .0
                .changed
                .wait_timeout(gate, Duration::from_millis(50))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        drop(gate);
        self.0
            .buffer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Fake {
    spec: Spec,
    calls: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn note(&self, call: &str) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{call} {}", self.spec.name));
    }
}

impl Service for Fake {
    fn name(&self) -> &str {
        self.spec.name
    }

    fn start(&mut self) -> Result<(), ServiceError> {
        self.note("start");
        if self.spec.fail_start {
            Err(ServiceError::new(format!(
                "{} refused to start",
                self.spec.name
            )))
        } else {
            Ok(())
        }
    }

    fn stop(&mut self) -> Result<(), ServiceError> {
        self.note("stop");
        Ok(())
    }
}

/// The installed logger and the sink it is held behind.
pub(crate) struct Held {
    shared: Arc<Shared>,
}

/// Installs a logger of capacity 1 over a held sink and fills its queue.
pub(crate) fn install_full() -> Held {
    let shared = Arc::new(Shared {
        buffer: Mutex::new(Vec::new()),
        gate: Mutex::new(Gate {
            open: false,
            writes_entered: 0,
        }),
        changed: Condvar::new(),
    });
    let settings = Settings {
        filter: Filter::new(Level::Trace),
        timestamps: false,
        colour: false,
    };
    let logger = Logger::with_capacity(Box::new(Sink(Arc::clone(&shared))), settings, 1)
        .expect("the writer thread starts");
    ck_log::install(logger).expect("no logger is installed yet in this process");
    let held = Held { shared };
    let logger = ck_log::installed().expect("the logger is installed");

    // The first record goes to the writer, which blocks in the sink holding it.
    logger.log(Level::Info, "test", "first");
    held.wait_for_writer_in_the_sink();
    // The queue is empty again; the second record fills it (capacity 1). Nothing can drain now.
    logger.log(Level::Info, "test", "second");
    held
}

impl Held {
    fn wait_for_writer_in_the_sink(&self) {
        let give_up = Instant::now() + BOUND;
        let mut gate = self.shared.lock_gate();
        while gate.writes_entered == 0 {
            let left = give_up.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the writer never reached the sink");
            gate = self
                .shared
                .changed
                .wait_timeout(gate, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Runs a runtime with `wait` as its log wait over fake services built from `specs`, while the
    /// queue is full and the sink held. The run has an overall deadline of five seconds. The sink is
    /// opened when this returns, on every path.
    pub(crate) fn run(&self, wait: Duration, specs: Vec<Spec>) -> Run {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (report, elapsed) = self.run_runtime({
            let calls = Arc::clone(&calls);
            move || {
                // The services are built on the run's thread: a `Service` is not `Send`.
                let services: Vec<Box<dyn Service>> = specs
                    .into_iter()
                    .map(|spec| {
                        Box::new(Fake {
                            spec,
                            calls: Arc::clone(&calls),
                        }) as Box<dyn Service>
                    })
                    .collect();
                Runtime::new(services).with_log_wait(wait)
            }
        });
        let calls = calls.lock().unwrap_or_else(PoisonError::into_inner).clone();
        Run {
            outcome: report.outcome,
            calls,
            elapsed,
        }
    }

    /// Runs the runtime that `build` makes (on the run's thread) while the queue is full and the sink
    /// held, with an overall deadline of five seconds, and returns its report and how long
    /// `Runtime::run_report` took. The sink is opened when this returns, on every path.
    pub(crate) fn run_runtime(&self, build: impl FnOnce() -> Runtime + Send) -> (Report, Duration) {
        let (done, finished) = mpsc::channel();
        thread::scope(|scope| {
            let _open = OpenOnDrop(&self.shared);
            scope.spawn(move || {
                let runtime = build();
                let began = Instant::now();
                let report = runtime.run_report();
                done.send((report, began.elapsed()))
                    .expect("the test is listening");
            });
            finished
                .recv_timeout(BOUND)
                .expect("the run must finish while the log queue stays full")
        })
    }

    /// With the sink open, what the logger says after a bounded flush, and everything it wrote.
    pub(crate) fn finish(&self) -> (FlushOutcome, String) {
        self.shared.open();
        let logger = ck_log::installed().expect("the logger is installed");
        let outcome = logger.flush(BOUND);
        let text = String::from_utf8(
            self.shared
                .buffer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        )
        .expect("log output is UTF-8");
        (outcome, text)
    }
}
