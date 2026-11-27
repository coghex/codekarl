//! The process's logger for the tests that read records (a copy of `ck-render-2d`'s test support): one installed
//! once, writing to a buffer, and a lock that keeps the tests that read it from interleaving. Only these tests
//! install a logger in this test process (the children of the boundary cases install their own).

use std::io::{self, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use ck_log::{Admission, Filter, FlushOutcome, Level, Logger, Settings};

/// The queue holds this many records, so a test can fill it.
pub(super) const CAPACITY: usize = 8;
/// The longest a held sink stays held, so a failing test cannot hang the process.
const HOLD_LIMIT: Duration = Duration::from_secs(30);

/// The sink's gate: whether it may write (closed by a hold), and whether the writer thread is parked inside a write that the closed gate
/// is holding, which is how a test knows the writer is blocked in the sink and not somewhere on the queue.
struct Gate {
    state: Mutex<GateState>,
    changed: Condvar,
}

#[derive(Clone, Copy)]
struct GateState {
    open: bool,
    parked: bool,
}

fn gate() -> &'static Gate {
    static GATE: OnceLock<Gate> = OnceLock::new();
    GATE.get_or_init(|| Gate {
        state: Mutex::new(GateState {
            open: true,
            parked: false,
        }),
        changed: Condvar::new(),
    })
}

struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let gate = gate();
        let give_up = Instant::now() + HOLD_LIMIT;
        let mut state = gate.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.open {
            state.parked = true;
            gate.changed.notify_all();
        }
        while !state.open {
            let left = give_up.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = gate
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        state.parked = false;
        drop(state);
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

/// The sink held shut: the writer thread is parked inside its first write and holds no lock of the queue. Opens again when dropped,
/// whatever happens to the test.
pub(super) struct Hold;

impl Drop for Hold {
    fn drop(&mut self) {
        let gate = gate();
        gate.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .open = true;
        gate.changed.notify_all();
    }
}

/// What the logger holds right now: the records admitted and not yet taken by the writer, and every record it has dropped since it began.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Counts {
    pub(super) queued: usize,
    pub(super) dropped: u64,
}

/// The logger's counts, as its own flush reports them (the queue's length and the drops, which contention cannot imitate).
pub(super) fn counts() -> Counts {
    let logger = ck_log::installed().expect("the logger is installed");
    match logger.flush(Duration::from_millis(2)) {
        FlushOutcome::Complete => Counts {
            queued: 0,
            dropped: 0,
        },
        FlushOutcome::Incomplete {
            queued, dropped, ..
        } => Counts { queued, dropped },
    }
}

/// Closes the sink, without waiting for anything: whatever the writer takes from now on, it parks on. Opens again when the hold is dropped.
pub(super) fn close_sink() -> Hold {
    gate()
        .state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .open = false;
    Hold
}

/// Waits until the writer is seen blocked inside the closed sink, at most `limit`: it then holds no lock of the queue.
pub(super) fn await_parked(limit: Duration) {
    let gate = gate();
    let give_up = Instant::now() + limit;
    let mut state = gate.state.lock().unwrap_or_else(PoisonError::into_inner);
    while !state.parked {
        let left = give_up.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "the writer never reached the held sink");
        state = gate
            .changed
            .wait_timeout(state, left)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

/// Whether the writer is seen blocked inside the closed sink.
fn is_parked() -> bool {
    gate()
        .state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .parked
}

/// Closes the sink and waits until the writer is seen blocked inside it, which is the handshake: the writer holds no lock of the queue
/// from then on. A marker record is what sends an idle writer into the sink, and it is sent **once, with a deadline (five seconds) no
/// contention outlasts, and must be admitted**: a marker given a short deadline can drop against the writer's own look at the queue, and
/// the drop's summary record would then be what the writer parks on, with the retry queued behind it. A writer that is already parked
/// (the sink closed on traffic that sent it there) needs no marker, and none is sent. Only inside a scenario.
pub(super) fn park_the_writer(_scenario: &Scenario) -> Hold {
    let hold = close_sink();
    if !is_parked() {
        let marker = Instant::now() + Duration::from_secs(5);
        assert_eq!(
            ck_log::log_within!(Level::Info, marker, "test", "hold marker"),
            Admission::Queued,
            "the marker is admitted, once, with nothing dropped"
        );
    }
    await_parked(Duration::from_secs(10));
    hold
}

/// Holds the sink shut with the writer seen blocked in it and fills the logger's queue with its capacity of ordinary records, each of
/// which must be admitted, so the next `try_log` drops its record. Only in a process of its own, inside a scenario: nothing else writes
/// to that process's logger, so the eight admissions are the whole queue (no other test's record can take a slot or hold its lock, and no
/// reserved error is in it), and the filling is done before the case starts any thread of its own. In a process shared with other tests
/// no count of the logger's could be trusted for this (a refusal can be contention, and the combined count includes a reserved error).
pub(super) fn hold_sink_and_fill_the_queue(scenario: &Scenario) -> Hold {
    let hold = park_the_writer(scenario);
    let deadline = || Instant::now() + Duration::from_secs(5);
    for filled in 0..CAPACITY {
        assert_eq!(
            ck_log::log_within!(Level::Info, deadline(), "test", "filler"),
            Admission::Queued,
            "filler {filled} is admitted behind the parked writer"
        );
    }
    assert_eq!(counts().queued, CAPACITY, "the queue holds the eight");
    hold
}
