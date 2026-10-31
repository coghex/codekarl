//! The process's logger for the tests that read records (a copy of `ck-render-2d`'s test support): one installed
//! once, writing to a buffer, and a lock that keeps the tests that read it from interleaving. Only these tests
//! install a logger in this test process (the children of the boundary cases install their own).

use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use ck_log::{Admission, Filter, FlushOutcome, Level, Logger, Settings};

/// The queue holds this many records, so a test can fill it.
const CAPACITY: usize = 8;
/// The longest a held sink stays held, so a failing test cannot hang the process.
const HOLD_LIMIT: Duration = Duration::from_secs(30);

/// Whether the sink may write: closed by [`Scenario::hold_sink`].
fn gate() -> &'static (Mutex<bool>, Condvar) {
    static GATE: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();
    GATE.get_or_init(|| (Mutex::new(true), Condvar::new()))
}

struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let (open, changed) = gate();
        let give_up = Instant::now() + HOLD_LIMIT;
        let mut open = open.lock().unwrap_or_else(PoisonError::into_inner);
        while !*open {
            let left = give_up.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            open = changed
                .wait_timeout(open, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        drop(open);
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn buffer() -> &'static Arc<Mutex<Vec<u8>>> {
    static BUFFER: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
    BUFFER.get_or_init(|| {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let logger = Logger::with_capacity(
            Box::new(Sink(Arc::clone(&buffer))),
            Settings {
                filter: Filter::new(Level::Trace),
                timestamps: false,
                colour: false,
            },
            CAPACITY,
        )
        .expect("the writer thread starts");
        ck_log::install(logger).expect("no logger is installed yet in this process");
        buffer
    })
}

/// The scenario lock, and a reader of the records the scenario writes.
pub(super) struct Scenario {
    _serial: MutexGuard<'static, ()>,
    taken: usize,
}

fn lines() -> Vec<String> {
    let logger = ck_log::installed().expect("the logger is installed");
    // A test that fills the queue makes the logger count drops for good, so `Complete` is not asked for: the
    // writer must have everything it was given.
    match logger.flush(Duration::from_secs(10)) {
        FlushOutcome::Complete
        | FlushOutcome::Incomplete {
            queued: 0,
            in_flight: false,
            sink_error: None,
            ..
        } => {}
        other @ FlushOutcome::Incomplete { .. } => {
            panic!("the logger must write every record it admitted: {other:?}")
        }
    }
    let text = String::from_utf8(
        buffer()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone(),
    )
    .expect("log output is UTF-8");
    text.lines().map(str::to_owned).collect()
}

/// Starts a scenario: waits for the others, installs the logger if needed, and remembers how many records exist.
pub(super) fn scenario() -> Scenario {
    static SERIAL: Mutex<()> = Mutex::new(());
    let serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = buffer();
    let taken = lines().len();
    Scenario {
        _serial: serial,
        taken,
    }
}

impl Scenario {
    /// The records written since the scenario began.
    pub(super) fn records(&self) -> Vec<String> {
        lines()[self.taken..].to_vec()
    }
}

/// The sink held shut: the writer thread stops inside its first write and the queue fills. Opens again when
/// dropped, whatever happens to the test.
pub(super) struct Hold;

impl Drop for Hold {
    fn drop(&mut self) {
        let (open, changed) = gate();
        *open.lock().unwrap_or_else(PoisonError::into_inner) = true;
        changed.notify_all();
    }
}

/// Holds the sink shut and fills the logger's queue, so the next `try_log` drops its record. Only inside a
/// scenario, which keeps the other tests from the logger meanwhile.
///
/// The writer takes everything queued in one batch and then blocks in its write, so the queue is full for good
/// only once the writer is stuck inside the write *and* the queue has filled again behind it.
pub(super) fn hold_sink_and_fill_the_queue(_scenario: &Scenario) -> Hold {
    let (open, _) = gate();
    *open.lock().unwrap_or_else(PoisonError::into_inner) = false;
    let hold = Hold;
    let logger = ck_log::installed().expect("the logger is installed");
    let give_up = Instant::now() + Duration::from_secs(10);
    while Instant::now() < give_up {
        for _ in 0..(4 * CAPACITY) {
            if ck_log::try_log!(Level::Info, "test", "filler") == Admission::Dropped {
                break;
            }
        }
        let stuck = matches!(
            logger.flush(Duration::from_millis(2)),
            FlushOutcome::Incomplete {
                in_flight: true,
                ..
            }
        );
        if stuck && ck_log::try_log!(Level::Info, "test", "filler") == Admission::Dropped {
            return hold;
        }
    }
    panic!("the logger's queue never filled behind a stuck writer");
}
