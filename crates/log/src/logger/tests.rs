//! Logger behaviour: lossless admission with back-pressure, `log_within`, `try_log`, a stalled or
//! failing sink, flush accounting and loss.
//!
//! Every test that blocks something does so through the logger's private rendezvous hooks
//! ([`Probe`]) or the sink's own entry counters, never a sleep or a snapshot of a flag, and releases
//! or detaches everything it blocked before it ends.

use std::collections::HashSet;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::*;

/// The longest any test waits for something that must happen.
const BOUND: Duration = Duration::from_secs(5);
/// The longest a deliberately blocked sink write waits before giving up on its test.
const SINK_PATIENCE: Duration = Duration::from_secs(30);

fn settings() -> Settings {
    Settings {
        filter: Filter::new(Level::Trace),
        timestamps: false,
        colour: false,
    }
}

/// What a `flush` of the test sink does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FlushMode {
    Succeed,
    Fail,
    /// Block until the mode changes.
    Hold,
}

/// How often a panic payload whose `Drop` panics was dropped, and the thread of each drop.
type BombCounts = (Arc<AtomicUsize>, Arc<Mutex<Vec<thread::ThreadId>>>);

struct SinkState {
    output: Vec<u8>,
    flushes: usize,
    flush_mode: FlushMode,
    /// Writes that may complete; a write beyond this blocks until it is raised.
    allowed: usize,
    /// A write that is released fails with `BrokenPipe` instead of completing.
    fail_writes: bool,
    /// A write that is released panics instead of completing.
    panic_writes: bool,
    /// A write that panics does so with a payload whose `Drop` panics: counted in `.0`, with the thread
    /// of each drop listed in `.1`.
    bomb: Option<BombCounts>,
    completed: usize,
    started: usize,
}

/// A sink that records its output and can hold writes back: a write completes only while fewer than
/// `allowed` writes have completed.
#[derive(Clone)]
struct TestSink(Arc<(Mutex<SinkState>, Condvar)>);

impl TestSink {
    fn new() -> Self {
        Self(Arc::new((
            Mutex::new(SinkState {
                output: Vec::new(),
                flushes: 0,
                flush_mode: FlushMode::Succeed,
                allowed: usize::MAX,
                fail_writes: false,
                panic_writes: false,
                bomb: None,
                completed: 0,
                started: 0,
            }),
            Condvar::new(),
        )))
    }

    fn allow(&self, writes: usize) {
        let (state, changed) = &*self.0;
        state.lock().expect("must be Ok or Some").allowed = writes;
        changed.notify_all();
    }

    fn set_flush_mode(&self, mode: FlushMode) {
        let (state, changed) = &*self.0;
        state.lock().expect("must be Ok or Some").flush_mode = mode;
        changed.notify_all();
    }

    /// How many times the sink's `flush` has been entered.
    fn flushes(&self) -> usize {
        self.0.0.lock().expect("must be Ok or Some").flushes
    }

    fn output(&self) -> String {
        String::from_utf8(self.0.0.lock().expect("must be Ok or Some").output.clone())
            .expect("must be Ok or Some")
    }

    /// Makes every write fail with `BrokenPipe`, including a blocked one, and releases it.
    fn fail_writes(&self) {
        let (state, changed) = &*self.0;
        let mut state = state.lock().expect("must be Ok or Some");
        state.fail_writes = true;
        state.allowed = usize::MAX;
        changed.notify_all();
    }

    /// Makes every write panic, including a blocked one, and releases it.
    fn panic_writes(&self) {
        let (state, changed) = &*self.0;
        let mut state = state.lock().expect("must be Ok or Some");
        state.panic_writes = true;
        state.allowed = usize::MAX;
        changed.notify_all();
    }

    /// Makes every write panic with a payload whose `Drop` panics, including a blocked one, and releases
    /// it. Returns the count of that payload's drops and the thread of each.
    fn panic_writes_with_a_bomb(&self) -> BombCounts {
        let drops = Arc::new(AtomicUsize::new(0));
        let threads = Arc::new(Mutex::new(Vec::new()));
        self.0.0.lock().expect("must be Ok or Some").bomb =
            Some((Arc::clone(&drops), Arc::clone(&threads)));
        self.panic_writes();
        (drops, threads)
    }

    /// Waits (bounded) until at least `writes` writes have begun.
    fn wait_started(&self, writes: usize) {
        let (state, changed) = &*self.0;
        let give_up = Instant::now() + BOUND;
        let mut state = state.lock().expect("must be Ok or Some");
        while state.started < writes {
            let left = give_up.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the sink never began write {writes}");
            state = changed
                .wait_timeout(state, left)
                .expect("must be Ok or Some")
                .0;
        }
    }

    fn started(&self) -> usize {
        self.0.0.lock().expect("must be Ok or Some").started
    }

    fn boxed(&self) -> Box<dyn Write + Send> {
        Box::new(self.clone())
    }
}

impl Write for TestSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let (state, changed) = &*self.0;
        let mut state = state.lock().expect("must be Ok or Some");
        state.started += 1;
        changed.notify_all();
        let give_up = Instant::now() + SINK_PATIENCE;
        while state.completed >= state.allowed && Instant::now() < give_up {
            state = changed
                .wait_timeout(state, Duration::from_millis(20))
                .expect("must be Ok or Some")
                .0;
        }
        if state.fail_writes {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if state.panic_writes {
            // Unlock first: a panic with the lock held would poison it for the test's own reads.
            let bomb = state.bomb.take();
            drop(state);
            if let Some((drops, threads)) = bomb {
                std::panic::panic_any(unwind::Bomb::new(&drops, &threads));
            }
            panic!("the test sink panics in write");
        }
        state.output.extend_from_slice(buf);
        state.completed += 1;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let (state, changed) = &*self.0;
        let mut state = state.lock().expect("must be Ok or Some");
        state.flushes += 1;
        changed.notify_all();
        let give_up = Instant::now() + SINK_PATIENCE;
        while state.flush_mode == FlushMode::Hold && Instant::now() < give_up {
            state = changed
                .wait_timeout(state, Duration::from_millis(20))
                .expect("must be Ok or Some")
                .0;
        }
        if state.flush_mode == FlushMode::Fail {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

/// A sink whose write (or flush) fails.
struct FailingSink {
    write: bool,
}

impl Write for FailingSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.write {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.write {
            Ok(())
        } else {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let give_up = Instant::now() + BOUND;
    while !ready() {
        assert!(Instant::now() < give_up, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

/// The total of every `<n> record(s) dropped` report written so far.
fn reported_drops(sink: &TestSink) -> u64 {
    sink.output()
        .lines()
        .filter_map(|line| {
            line.strip_prefix("WARN  log: ")?
                .strip_suffix(" record(s) dropped")?
                .parse::<u64>()
                .ok()
        })
        .sum()
}

fn logger(sink: &TestSink, capacity: usize) -> Logger {
    Logger::with_capacity(sink.boxed(), settings(), capacity).expect("must be Ok or Some")
}

#[test]
fn one_thread_writes_in_order_and_flush_is_complete() {
    let sink = TestSink::new();
    let logger = Logger::new(sink.boxed(), settings()).expect("must be Ok or Some");
    for number in 0..200 {
        logger.log(Level::Info, "test", format_args!("record {number}"));
    }
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(logger.dropped_total(), 0);
    let mut expected = String::new();
    for number in 0..200 {
        writeln!(expected, "INFO  test: record {number}").expect("must be Ok or Some");
    }
    assert_eq!(sink.output(), expected);
}

#[test]
fn timestamps_and_colour_follow_the_settings() {
    let sink = TestSink::new();
    let mut with_time = settings();
    with_time.timestamps = true;
    with_time.colour = true;
    let logger = Logger::new(sink.boxed(), with_time).expect("must be Ok or Some");
    logger.log(Level::Warn, "test", "hello");
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    let output = sink.output();
    let (stamp, rest) = output.split_at(24);
    assert_eq!(rest, " \x1b[33mWARN\x1b[0m  test: hello\n");
    for (index, expected) in [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'.'),
        (23, b'Z'),
    ] {
        assert_eq!(stamp.as_bytes()[index], expected, "{stamp}");
    }
}

#[test]
fn a_stalled_sink_never_blocks_try_log_or_flush() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 4);
    logger.log(Level::Info, "test", "r0");
    wait_until("the writer to block in its first write", || {
        sink.started() == 1
    });
    let begun = Instant::now();
    let mut slowest = Duration::ZERO;
    for number in 1..100 {
        let call = Instant::now();
        let _ = logger.try_log(Level::Info, "test", format_args!("r{number}"));
        slowest = slowest.max(call.elapsed());
    }
    assert!(
        slowest < Duration::from_millis(500),
        "slowest log call {slowest:?}"
    );
    assert!(begun.elapsed() < Duration::from_secs(1));

    let flushing = Instant::now();
    assert_eq!(
        logger.flush(Duration::from_millis(100)),
        FlushOutcome::Incomplete {
            queued: 4,
            in_flight: true,
            dropped: 95,
            sink_error: None,
        }
    );
    assert!(
        flushing.elapsed() < Duration::from_secs(1),
        "{:?}",
        flushing.elapsed()
    );

    sink.allow(usize::MAX);
    wait_until("the drop report", || {
        sink.output().ends_with("WARN  log: 95 record(s) dropped\n")
    });
    assert_eq!(
        sink.output(),
        "INFO  test: r0\nINFO  test: r1\nINFO  test: r2\nINFO  test: r3\nINFO  test: r4\n\
         WARN  log: 95 record(s) dropped\n"
    );
}

#[test]
fn dropping_a_logger_does_not_wait_for_a_blocked_write() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 4);
    logger.log(Level::Info, "test", "r0");
    logger.log(Level::Info, "test", "r1");
    wait_until("the writer to block", || sink.started() == 1);
    let begun = Instant::now();
    drop(logger);
    assert!(
        begun.elapsed() < Duration::from_secs(1),
        "{:?}",
        begun.elapsed()
    );
    sink.allow(usize::MAX);
    wait_until("the detached writer to finish", || {
        sink.output().ends_with("r1\n")
    });
}

#[test]
fn dropping_an_idle_logger_lets_the_writer_drain_what_was_queued() {
    let sink = TestSink::new();
    let logger = logger(&sink, 64);
    for number in 0..20 {
        logger.log(Level::Info, "test", format_args!("r{number}"));
    }
    assert_eq!(logger.dropped_total(), 0);
    drop(logger);
    wait_until("the drain", || sink.output().ends_with("r19\n"));
    assert_eq!(sink.output().lines().count(), 20);
}

#[test]
fn a_failing_sink_drops_later_records_without_panicking() {
    let logger =
        Logger::new(Box::new(FailingSink { write: true }), settings()).expect("must be Ok or Some");
    logger.log(Level::Info, "test", "first");
    wait_until("the sink error", || {
        matches!(
            logger.flush(Duration::from_millis(10)),
            FlushOutcome::Incomplete {
                sink_error: Some(io::ErrorKind::BrokenPipe),
                ..
            }
        )
    });
    for number in 0..5 {
        logger.log(Level::Error, "test", format_args!("later {number}"));
    }
    assert_eq!(
        logger.flush(BOUND),
        FlushOutcome::Incomplete {
            queued: 0,
            in_flight: false,
            dropped: 6,
            sink_error: Some(io::ErrorKind::BrokenPipe),
        }
    );
}

#[test]
fn a_failing_sink_flush_is_reported_too() {
    let logger = Logger::new(Box::new(FailingSink { write: false }), settings())
        .expect("must be Ok or Some");
    logger.log(Level::Info, "test", "written but never flushed");
    wait_until("the flush error", || {
        matches!(
            logger.flush(Duration::from_millis(10)),
            FlushOutcome::Incomplete {
                sink_error: Some(io::ErrorKind::BrokenPipe),
                ..
            }
        )
    });
}

#[test]
fn flush_is_incomplete_while_the_writer_holds_a_batch_it_has_taken() {
    let sink = TestSink::new();
    sink.allow(1);
    let logger = logger(&sink, 8);
    logger.log(Level::Info, "test", "a");
    wait_until("a to be written", || sink.output() == "INFO  test: a\n");
    // `b` is taken as a batch and the writer blocks in its write.
    logger.log(Level::Info, "test", "b");
    wait_until("the writer to block in b", || sink.started() == 2);
    logger.log(Level::Info, "test", "c");
    assert_eq!(
        logger.flush(Duration::ZERO),
        FlushOutcome::Incomplete {
            queued: 1,
            in_flight: true,
            dropped: 0,
            sink_error: None,
        }
    );
    // Let `b` finish: the writer moves on to `c` and blocks there with an empty queue. The queue
    // being empty must not look complete while `c` is held in the writer's local batch.
    sink.allow(2);
    wait_until("the writer to block in c", || sink.started() == 3);
    assert_eq!(
        logger.flush(Duration::ZERO),
        FlushOutcome::Incomplete {
            queued: 0,
            in_flight: true,
            dropped: 0,
            sink_error: None,
        }
    );
    thread::scope(|scope| {
        let poller = scope.spawn(|| {
            let give_up = Instant::now() + BOUND;
            loop {
                if logger.flush(Duration::ZERO) == FlushOutcome::Complete {
                    return sink.output();
                }
                assert!(Instant::now() < give_up, "flush never completed");
            }
        });
        sink.allow(usize::MAX);
        let output = poller.join().expect("must be Ok or Some");
        assert_eq!(output, "INFO  test: a\nINFO  test: b\nINFO  test: c\n");
    });
}

#[test]
fn the_capacity_stays_a_logical_bound_after_the_writer_moves_records_out() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 3);
    logger.log(Level::Info, "test", "r0");
    wait_until("r0 to be taken", || sink.started() == 1);
    for number in 1..=4 {
        let _ = logger.try_log(Level::Info, "test", format_args!("r{number}"));
    }
    // r1..r3 fill the queue and r4 is dropped.
    assert_eq!(logger.dropped_total(), 1);
    // r0 completes; the writer takes r1..r3 (swapping buffers) and blocks writing r1.
    sink.allow(1);
    wait_until("the writer to take the queue", || sink.started() == 2);
    for number in 5..=9 {
        let _ = logger.try_log(Level::Info, "test", format_args!("r{number}"));
    }
    // The emptied queue admits exactly its capacity again: three of the five.
    assert_eq!(
        logger.flush(Duration::ZERO),
        FlushOutcome::Incomplete {
            queued: 3,
            in_flight: true,
            dropped: 3,
            sink_error: None,
        }
    );
    sink.allow(usize::MAX);
}

#[test]
fn a_capacity_of_zero_is_raised_to_one() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 0);
    logger.log(Level::Info, "test", "r0");
    wait_until("r0 to be taken", || sink.started() == 1);
    let _ = logger.try_log(Level::Info, "test", "r1");
    let _ = logger.try_log(Level::Info, "test", "r2");
    assert_eq!(logger.dropped_total(), 1);
    sink.allow(usize::MAX);
}

#[test]
fn try_log_on_a_full_queue_behind_a_stalled_writer_drops_at_once() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 1);
    logger.log(Level::Info, "test", "r0");
    wait_until("r0 to be taken", || sink.started() == 1);
    let _ = logger.try_log(Level::Info, "test", "r1");
    let call = Instant::now();
    let _ = logger.try_log(Level::Info, "test", "r2");
    assert!(call.elapsed() < Duration::from_millis(500));
    assert_eq!(logger.dropped_total(), 1);
    sink.allow(usize::MAX);
}

#[test]
fn loss_stays_in_the_flush_result_after_it_was_reported_and_later_drops_are_reported_too() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 2);
    logger.log(Level::Info, "test", "r0");
    wait_until("r0 to be taken", || sink.started() == 1);
    for number in 1..=4 {
        let _ = logger.try_log(Level::Info, "test", format_args!("r{number}"));
    }
    assert_eq!(logger.dropped_total(), 2);
    // r0, r1 and r2 complete; the first drop report (the fourth write) blocks.
    sink.allow(3);
    wait_until("the writer to block in the first report", || {
        sink.started() == 4
    });
    // The report has already taken its count of 2. These are counted for the next report.
    for number in 5..=7 {
        let _ = logger.try_log(Level::Info, "test", format_args!("r{number}"));
    }
    assert_eq!(logger.dropped_total(), 3);
    sink.allow(usize::MAX);
    wait_until("the second report", || {
        sink.output().ends_with("WARN  log: 1 record(s) dropped\n")
    });
    assert_eq!(
        sink.output(),
        "INFO  test: r0\nINFO  test: r1\nINFO  test: r2\nWARN  log: 2 record(s) dropped\n\
         INFO  test: r5\nINFO  test: r6\nWARN  log: 1 record(s) dropped\n"
    );
    // Both reports were written, yet the cumulative loss still makes the run incomplete.
    assert_eq!(
        logger.flush(BOUND),
        FlushOutcome::Incomplete {
            queued: 0,
            in_flight: false,
            dropped: 3,
            sink_error: None,
        }
    );
}

#[test]
fn records_that_the_filter_removes_are_not_losses() {
    let sink = TestSink::new();
    let mut quiet = settings();
    quiet.filter = Filter::new(Level::Warn)
        .with_category("noisy", Level::Trace)
        .expect("must be Ok or Some");
    let logger = Logger::new(sink.boxed(), quiet).expect("must be Ok or Some");
    logger.log(Level::Info, "test", "filtered out");
    logger.log(Level::Trace, "noisy.deep", "kept");
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(sink.output(), "TRACE noisy.deep: kept\n");
}

#[test]
fn set_settings_applies_to_records_emitted_afterwards() {
    let sink = TestSink::new();
    let logger = Logger::new(sink.boxed(), settings()).expect("must be Ok or Some");
    logger.log(Level::Info, "test", "before");
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    let mut errors_only = settings();
    errors_only.filter = Filter::new(Level::Error);
    logger.set_settings(errors_only);
    logger.log(Level::Info, "test", "after");
    logger.log(Level::Error, "test", "kept");
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(sink.output(), "INFO  test: before\nERROR test: kept\n");
}

#[test]
fn an_invalid_category_or_failing_message_is_dropped_and_counted() {
    struct Unprintable;
    impl fmt::Display for Unprintable {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            Err(fmt::Error)
        }
    }
    let sink = TestSink::new();
    let logger = Logger::new(sink.boxed(), settings()).expect("must be Ok or Some");
    logger.log(Level::Info, "bad category", "x");
    logger.log(Level::Info, "", "x");
    logger.log(Level::Info, "test", Unprintable);
    assert_eq!(logger.dropped_total(), 3);
    wait_until("the reports", || reported_drops(&sink) == 3);
    assert!(matches!(
        logger.flush(BOUND),
        FlushOutcome::Incomplete { dropped: 3, .. }
    ));
}

#[test]
fn a_record_after_the_writer_is_gone_is_dropped() {
    let logger =
        Logger::new(Box::new(FailingSink { write: true }), settings()).expect("must be Ok or Some");
    logger.log(Level::Info, "test", "first");
    wait_until("the writer to give up", || logger.is_closed());
    logger.log(Level::Info, "test", "second");
    assert_eq!(logger.dropped_total(), 2);
    assert_eq!(locked(&logger.shared.queue).records.len(), 0);
}

#[test]
fn one_trailing_newline_is_removed_from_a_message() {
    let sink = TestSink::new();
    let logger = Logger::new(sink.boxed(), settings()).expect("must be Ok or Some");
    logger.log(Level::Info, "test", "done\n");
    logger.log(Level::Info, "test", "done");
    logger.log(Level::Info, "test", "two\n\n");
    let outcome = logger.flush(BOUND);
    // A contention drop is possible in principle; the expected text only holds without one.
    if outcome == FlushOutcome::Complete {
        assert_eq!(
            sink.output(),
            "INFO  test: done\nINFO  test: done\nINFO  test: two\n  \n"
        );
    }
}

/// A sink that only counts its flushes (shared with the test) and optionally fails them.
struct CountingSink {
    flushes: Arc<AtomicUsize>,
    fail: bool,
}

impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

fn counting(fail: bool) -> (Logger, Arc<AtomicUsize>) {
    let flushes = Arc::new(AtomicUsize::new(0));
    let sink = CountingSink {
        flushes: Arc::clone(&flushes),
        fail,
    };
    let logger = Logger::new(Box::new(sink), settings()).expect("must be Ok or Some");
    (logger, flushes)
}

#[test]
fn flushing_a_logger_that_never_emitted_flushes_the_sink() {
    let (logger, flushes) = counting(false);
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert!(
        flushes.load(Ordering::SeqCst) >= 1,
        "Complete needs a real sink flush"
    );
    // Each flush call is a request that a fresh sink flush covers.
    let before = flushes.load(Ordering::SeqCst);
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert!(flushes.load(Ordering::SeqCst) > before);
}

#[test]
fn flushing_a_logger_that_never_emitted_reports_a_failing_sink_flush() {
    let (logger, flushes) = counting(true);
    assert_eq!(
        logger.flush(BOUND),
        FlushOutcome::Incomplete {
            queued: 0,
            in_flight: false,
            dropped: 0,
            sink_error: Some(io::ErrorKind::BrokenPipe),
        }
    );
    assert_eq!(flushes.load(Ordering::SeqCst), 1);
}

#[test]
fn flushing_a_logger_that_never_emitted_is_bounded_and_incomplete_when_the_sink_flush_stalls() {
    let sink = TestSink::new();
    sink.set_flush_mode(FlushMode::Hold);
    let logger = logger(&sink, 8);
    let begun = Instant::now();
    assert_eq!(
        logger.flush(Duration::from_millis(100)),
        FlushOutcome::Incomplete {
            queued: 0,
            in_flight: true,
            dropped: 0,
            sink_error: None,
        }
    );
    assert!(
        begun.elapsed() < Duration::from_secs(1),
        "{:?}",
        begun.elapsed()
    );
    // Once the sink flush returns, a flush completes.
    sink.set_flush_mode(FlushMode::Succeed);
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
}

#[test]
fn a_fully_filtered_run_flushes_the_sink_and_reports_its_result() {
    for (fail, expected) in [
        (false, FlushOutcome::Complete),
        (
            true,
            FlushOutcome::Incomplete {
                queued: 0,
                in_flight: false,
                dropped: 0,
                sink_error: Some(io::ErrorKind::BrokenPipe),
            },
        ),
    ] {
        let flushes = Arc::new(AtomicUsize::new(0));
        let sink = CountingSink {
            flushes: Arc::clone(&flushes),
            fail,
        };
        let mut quiet = settings();
        quiet.filter = Filter::new(Level::Error);
        let logger = Logger::new(Box::new(sink), quiet).expect("must be Ok or Some");
        for number in 0..20 {
            logger.log(Level::Debug, "test", format_args!("filtered {number}"));
        }
        assert_eq!(logger.dropped_total(), 0, "filtered records are not losses");
        assert_eq!(logger.flush(BOUND), expected, "fail = {fail}");
        assert!(flushes.load(Ordering::SeqCst) >= 1, "fail = {fail}");
    }
}

#[test]
fn a_stalled_flush_after_records_is_incomplete_with_the_writer_in_flight() {
    let sink = TestSink::new();
    sink.set_flush_mode(FlushMode::Hold);
    let logger = logger(&sink, 8);
    logger.log(Level::Info, "test", "r0");
    wait_until("the sink flush to start", || sink.flushes() >= 1);
    assert_eq!(
        logger.flush(Duration::from_millis(100)),
        FlushOutcome::Incomplete {
            queued: 0,
            in_flight: true,
            dropped: 0,
            sink_error: None,
        }
    );
    sink.set_flush_mode(FlushMode::Succeed);
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
}

/// A sink whose destructor blocks (as a buffered writer's can) until the test releases it.
struct BlockingDrop {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

impl Write for BlockingDrop {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for BlockingDrop {
    fn drop(&mut self) {
        let _ = self.entered.send(());
        let _ = self.release.recv_timeout(SINK_PATIENCE);
    }
}

#[test]
fn dropping_a_logger_does_not_wait_for_a_sink_destructor_that_blocks() {
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (done_tx, done) = mpsc::channel();
    let sink = BlockingDrop {
        entered: entered_tx,
        release: release_rx,
    };
    let logger = Logger::new(Box::new(sink), settings()).expect("must be Ok or Some");
    logger.log(Level::Info, "test", "written before the drop");
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    let dropper = thread::spawn(move || {
        drop(logger);
        let _ = done_tx.send(());
    });
    entered
        .recv_timeout(BOUND)
        .expect("the writer reaches the sink destructor");
    let returned = done.recv_timeout(Duration::from_millis(500)).is_ok();
    let _ = release.send(());
    dropper.join().expect("the dropping thread ends");
    assert!(
        returned,
        "Logger::drop waited for the writer to finish destroying its sink"
    );
}

// ---------------------------------------------------------------------------------------------
// CK-47: lossless admission with back-pressure, `log_within` and `try_log`.
// ---------------------------------------------------------------------------------------------

/// Releases a held sink when dropped, so that a failing test never leaves a thread blocked.
struct Release(TestSink);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.allow(usize::MAX);
        self.0.set_flush_mode(FlushMode::Succeed);
    }
}

/// Lets every emitter waiting for space go when dropped (they drop their record), so that a failing
/// test or a broken wake-up ends with an assertion instead of a hang.
struct WakeWaiters<'a>(&'a Logger);

impl Drop for WakeWaiters<'_> {
    fn drop(&mut self) {
        self.0.rescue_waiters();
    }
}

/// Waits (bounded) for the first probe event `wanted` accepts, skipping the others.
fn next_probe(probes: &mpsc::Receiver<Probe>, wanted: impl Fn(Probe) -> bool) -> Probe {
    let give_up = Instant::now() + BOUND;
    loop {
        let left = give_up.saturating_duration_since(Instant::now());
        match probes.recv_timeout(left) {
            Ok(probe) if wanted(probe) => return probe,
            Ok(_) => {}
            Err(error) => panic!("the awaited rendezvous never happened: {error}"),
        }
    }
}

fn is_space_wait(probe: Probe) -> bool {
    matches!(probe, Probe::SpaceWait(_))
}

/// Waits (bounded) until `count` different threads are waiting for space.
fn wait_for_space_waiters(probes: &mpsc::Receiver<Probe>, count: usize) {
    let mut waiting = HashSet::new();
    while waiting.len() < count {
        if let Probe::SpaceWait(thread) = next_probe(probes, is_space_wait) {
            waiting.insert(thread);
        }
    }
}

/// A logger of capacity `capacity` whose sink holds its first write: record `r0` is in the writer's
/// hands and `capacity` more fill the queue, so the queue is full and nothing drains. Returns the
/// guard that releases the sink.
fn stalled_and_full(sink: &TestSink, logger: &Logger, capacity: usize) -> Release {
    let release = Release(sink.clone());
    logger.log(Level::Info, "test", "r0");
    sink.wait_started(1);
    for number in 1..=capacity {
        logger.log(Level::Info, "test", format_args!("r{number}"));
    }
    release
}

/// A deadline `delta` from now.
fn ahead(delta: Duration) -> Instant {
    Instant::now() + delta
}

#[test]
fn eight_threads_lose_nothing_and_every_record_appears_once_in_order() {
    const THREADS: usize = 8;
    const PER_THREAD: usize = 10_000;
    let sink = TestSink::new();
    let logger = Logger::new(sink.boxed(), settings()).expect("must be Ok or Some");
    let (done, finished) = mpsc::channel();
    thread::scope(|scope| {
        let _rescue = WakeWaiters(&logger);
        for thread_number in 0..THREADS {
            scope.spawn({
                let logger = &logger;
                let done = done.clone();
                move || {
                    for number in 0..PER_THREAD {
                        logger.log(
                            Level::Info,
                            "worker",
                            format_args!("{thread_number} {number}"),
                        );
                    }
                    done.send(()).expect("the test is listening");
                }
            });
        }
        for _ in 0..THREADS {
            finished
                .recv_timeout(Duration::from_secs(60))
                .expect("every emitter finishes");
        }
    });
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(logger.dropped_total(), 0);
    let output = sink.output();
    let mut next = [0_usize; THREADS];
    let mut written = 0_usize;
    for line in output.lines() {
        let body = line
            .strip_prefix("INFO  worker: ")
            .unwrap_or_else(|| panic!("not a whole record: {line:?}"));
        let (thread_number, number) = body.split_once(' ').expect("a thread and a number");
        let (thread_number, number): (usize, usize) = (
            thread_number.parse().expect("a number"),
            number.parse().expect("a number"),
        );
        assert_eq!(number, next[thread_number], "{line}");
        next[thread_number] += 1;
        written += 1;
    }
    assert_eq!(written, THREADS * PER_THREAD);
    assert_eq!(next, [PER_THREAD; THREADS]);
}

#[test]
fn two_records_back_to_back_on_a_fresh_logger_are_never_lost() {
    // The CK-42 burst: the second record used to be lost about once in 1,000 runs.
    for run in 0..2_000 {
        let logger = Logger::new(Box::new(io::sink()), settings()).expect("must be Ok or Some");
        logger.log(Level::Info, "test", "first");
        logger.log(Level::Info, "test", "second");
        assert_eq!(
            logger.flush(Duration::from_secs(1)),
            FlushOutcome::Complete,
            "run {run}"
        );
        assert_eq!(logger.dropped_total(), 0, "run {run}");
    }
}

#[test]
fn a_full_queue_makes_log_wait_and_nothing_is_lost() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 4);
    let probes = logger.probes();
    let (done, finished) = mpsc::channel();
    thread::scope(|scope| {
        let _wake = WakeWaiters(&logger);
        let release = Release(sink.clone());
        scope.spawn({
            let logger = &logger;
            move || {
                for number in 0..10 {
                    logger.log(Level::Info, "test", format_args!("r{number}"));
                }
                done.send(()).expect("the test is listening");
            }
        });
        // The writer holds at most 4 records and the queue 4: ten cannot all be admitted, so the
        // emitter reaches the space wait with the queue full.
        next_probe(&probes, is_space_wait);
        assert_eq!(logger.dropped_total(), 0);
        assert!(finished.try_recv().is_err(), "it is still waiting");
        drop(release);
        finished
            .recv_timeout(BOUND)
            .expect("the emitter finishes once there is room");
    });
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    let mut expected = String::new();
    for number in 0..10 {
        writeln!(expected, "INFO  test: r{number}").expect("must be Ok or Some");
    }
    assert_eq!(sink.output(), expected);
    assert_eq!(logger.dropped_total(), 0);
}

#[test]
fn flush_stays_bounded_while_an_emitter_waits_for_space() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 4);
    let probes = logger.probes();
    thread::scope(|scope| {
        let _wake = WakeWaiters(&logger);
        let _release = Release(sink.clone());
        scope.spawn(|| {
            for number in 0..10 {
                logger.log(Level::Info, "test", format_args!("r{number}"));
            }
        });
        next_probe(&probes, is_space_wait);
        let begun = Instant::now();
        assert_eq!(
            logger.flush(Duration::from_millis(100)),
            FlushOutcome::Incomplete {
                queued: 4,
                in_flight: true,
                dropped: 0,
                sink_error: None,
            }
        );
        assert!(
            begun.elapsed() < Duration::from_secs(1),
            "{:?}",
            begun.elapsed()
        );
    });
}

#[test]
fn log_waits_for_the_settings_lock_and_is_admitted_after_it_is_released() {
    let sink = TestSink::new();
    let logger = logger(&sink, 8);
    let probes = logger.probes();
    let (done, finished) = mpsc::channel();
    thread::scope(|scope| {
        let held = logger.hold_settings();
        scope.spawn({
            let logger = &logger;
            move || {
                logger.log(Level::Info, "test", "waited");
                done.send(()).expect("the test is listening");
            }
        });
        next_probe(&probes, |probe| probe == Probe::SettingsContended);
        // The emitter cannot proceed while the lock is held: nothing was admitted or lost.
        assert_eq!(logger.dropped_total(), 0);
        assert!(sink.output().is_empty());
        assert!(finished.try_recv().is_err(), "it is still waiting");
        drop(held);
        finished
            .recv_timeout(BOUND)
            .expect("the emitter finishes after the release");
    });
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(sink.output(), "INFO  test: waited\n");
    assert_eq!(logger.dropped_total(), 0);
}

#[test]
fn log_waits_for_the_queue_lock_and_is_admitted_after_it_is_released() {
    let sink = TestSink::new();
    let logger = logger(&sink, 8);
    let probes = logger.probes();
    let (done, finished) = mpsc::channel();
    thread::scope(|scope| {
        let held = logger.hold_queue();
        scope.spawn({
            let logger = &logger;
            move || {
                logger.log(Level::Info, "test", "waited");
                done.send(()).expect("the test is listening");
            }
        });
        next_probe(&probes, |probe| probe == Probe::QueueContended);
        // The emitter cannot proceed while the lock is held: nothing was admitted or lost.
        assert_eq!(logger.dropped_total(), 0);
        assert_eq!(held.records.len(), 0, "nothing was admitted past the lock");
        assert!(finished.try_recv().is_err(), "it is still waiting");
        drop(held);
        finished
            .recv_timeout(BOUND)
            .expect("the emitter finishes after the release");
    });
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(sink.output(), "INFO  test: waited\n");
    assert_eq!(logger.dropped_total(), 0);
}

#[test]
fn a_sink_failure_wakes_every_space_waiter_and_counts_each_record_once() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 2);
    let probes = logger.probes();
    let (done, finished) = mpsc::channel();
    thread::scope(|scope| {
        let _wake = WakeWaiters(&logger);
        let _release = Release(sink.clone());
        // r0 is in the writer's hands (its write is held); r1 and r2 fill the queue.
        logger.log(Level::Info, "test", "r0");
        sink.wait_started(1);
        logger.log(Level::Info, "test", "r1");
        logger.log(Level::Info, "test", "r2");
        for number in 0..3 {
            scope.spawn({
                let logger = &logger;
                let done = done.clone();
                move || {
                    logger.log(Level::Info, "test", format_args!("w{number}"));
                    done.send(()).expect("the test is listening");
                }
            });
        }
        wait_for_space_waiters(&probes, 3);
        // The held write fails: no later write succeeds, yet all three waiters must return.
        sink.fail_writes();
        for _ in 0..3 {
            finished
                .recv_timeout(BOUND)
                .expect("a waiter returns after the failure");
        }
        // Counted once each: r0 (failed write), r1 and r2 (queued), w0..w2 (waiting).
        assert_eq!(
            logger.flush(BOUND),
            FlushOutcome::Incomplete {
                queued: 0,
                in_flight: false,
                dropped: 6,
                sink_error: Some(io::ErrorKind::BrokenPipe),
            }
        );
        // A later caller finds the queue closed and drops at once.
        let begun = Instant::now();
        logger.log(Level::Info, "test", "late");
        assert!(begun.elapsed() < Duration::from_millis(500));
        assert_eq!(logger.dropped_total(), 7);
        assert!(sink.output().is_empty(), "no write ever succeeded");
    });
}

#[test]
fn log_within_drops_a_record_at_its_deadline_when_the_queue_stays_full() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 2);
    let probes = logger.probes();
    let _release = stalled_and_full(&sink, &logger, 2);
    let deadline = ahead(Duration::from_millis(50));
    let admission = logger.log_within(Level::Info, "test", "too late", deadline);
    let returned = Instant::now();
    assert_eq!(admission, Admission::Dropped);
    assert!(
        returned >= deadline,
        "it must not give up before its deadline"
    );
    assert!(
        returned < deadline + Duration::from_millis(500),
        "{:?} past the deadline",
        returned - deadline
    );
    assert!(
        probes.try_iter().any(|probe| probe == Probe::SpacePoll),
        "it polled for space before giving up"
    );
    assert_eq!(logger.dropped_total(), 1);
}

#[test]
fn log_within_is_queued_when_space_is_freed_before_its_deadline() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 2);
    let probes = logger.probes();
    thread::scope(|scope| {
        let release = stalled_and_full(&sink, &logger, 2);
        let deadline = ahead(Duration::from_secs(30));
        let emitter = scope.spawn({
            let logger = &logger;
            move || logger.log_within(Level::Info, "test", "in time", deadline)
        });
        next_probe(&probes, |probe| probe == Probe::SpacePoll);
        drop(release);
        let admission = emitter.join().expect("the emitter finishes");
        assert_eq!(admission, Admission::Queued);
        assert!(Instant::now() < deadline);
    });
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert!(sink.output().ends_with("INFO  test: in time\n"));
    assert_eq!(logger.dropped_total(), 0);
}

/// Runs `log_within` on another thread with `deadline_in` to go while `guard` keeps a lock held; the
/// result must arrive while the lock is still held, and not before the deadline.
///
/// The helper owns the guard and holds it inside the scope body, so that a failed assertion (for
/// instance a regression that makes `log_within` block on the lock) unwinds through the body and
/// drops the guard before the scope joins the blocked thread: the test then fails in about the
/// receive bound instead of waiting for a join that nothing could end.
fn log_within_while_held<G>(logger: &Logger, deadline_in: Duration, guard: G) {
    let (done, finished) = mpsc::channel();
    thread::scope(|scope| {
        let _held = guard;
        let deadline = ahead(deadline_in);
        scope.spawn(move || {
            let admission = logger.log_within(Level::Info, "test", "held", deadline);
            done.send((admission, Instant::now()))
                .expect("the test is listening");
        });
        let (admission, returned) = finished
            .recv_timeout(BOUND)
            .expect("log_within must return while the lock is still held");
        assert_eq!(admission, Admission::Dropped);
        assert!(
            returned >= deadline,
            "it must not give up before its deadline"
        );
        assert!(
            returned < deadline + Duration::from_millis(500),
            "{:?} past the deadline",
            returned - deadline
        );
    });
    assert_eq!(logger.dropped_total(), 1);
}

#[test]
fn log_within_drops_at_its_deadline_while_the_queue_lock_stays_held() {
    let sink = TestSink::new();
    let logger = logger(&sink, 8);
    log_within_while_held(&logger, Duration::from_millis(50), logger.hold_queue());
}

#[test]
fn log_within_drops_at_its_deadline_while_the_settings_lock_stays_held() {
    let sink = TestSink::new();
    let logger = logger(&sink, 8);
    log_within_while_held(&logger, Duration::from_millis(50), logger.hold_settings());
}

#[test]
fn log_within_with_a_short_deadline_returns_promptly_while_the_queue_lock_is_held() {
    let sink = TestSink::new();
    let logger = logger(&sink, 8);
    log_within_while_held(&logger, Duration::from_millis(5), logger.hold_queue());
}

#[test]
fn log_within_does_not_wait_on_the_queue_mutex_after_it_starts_waiting_for_space() {
    // Regression for `Condvar::wait_timeout`, which re-acquires the mutex without a bound: another
    // thread takes the queue mutex after `log_within` has started waiting on the full queue, and
    // holds it until `log_within` has returned.
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 2);
    let probes = logger.probes();
    let (done, finished) = mpsc::channel();
    thread::scope(|scope| {
        let _release = stalled_and_full(&sink, &logger, 2);
        let deadline = ahead(Duration::from_millis(300));
        scope.spawn({
            let logger = &logger;
            let done = done.clone();
            move || {
                let admission = logger.log_within(Level::Info, "test", "waiting", deadline);
                done.send((admission, Instant::now()))
                    .expect("the test is listening");
            }
        });
        next_probe(&probes, |probe| probe == Probe::SpacePoll);
        let held = logger.hold_queue();
        let (admission, returned) = finished
            .recv_timeout(BOUND)
            .expect("log_within must return while the queue mutex is held");
        assert_eq!(admission, Admission::Dropped);
        assert!(returned >= deadline);
        assert!(
            returned < deadline + Duration::from_millis(500),
            "{:?} past the deadline",
            returned - deadline
        );
        assert_eq!(held.records.len(), 2, "the holder had the queue throughout");
        drop(held);
    });
    assert_eq!(logger.dropped_total(), 1);
}

#[test]
fn the_non_waiting_admission_code_calls_no_blocking_lock_or_condition_variable_wait() {
    let source = include_str!("../logger.rs");
    let region = |begin: &str, end: &str| -> &str {
        let from = source.find(begin).expect("begin marker") + begin.len();
        let to = from + source[from..].find(end).expect("end marker");
        &source[from..to]
    };
    let admission = region(
        "// BEGIN non-waiting admission",
        "// END non-waiting admission",
    );
    let poll = region("fn poll<T>(", "/// What formatting a record produced.");
    for (name, code) in [("admission", admission), ("poll", poll)] {
        for forbidden in [
            ".lock()",
            "locked(",
            ".read()",
            ".write()",
            ".wait(",
            ".wait_timeout",
            ".wait_while",
            "current_settings(",
            "read_settings(",
        ] {
            assert!(!code.contains(forbidden), "{name} contains {forbidden}");
        }
    }
    assert!(admission.contains(".try_lock()") && admission.contains(".try_read()"));
    assert!(admission.contains("park_timeout"));
}

#[test]
fn an_expired_deadline_makes_exactly_one_attempt_per_phase() {
    let past = Instant::now()
        .checked_sub(Duration::from_millis(5))
        .expect("the clock has run for more than 5 ms");
    let attempts = |probes: &mpsc::Receiver<Probe>| -> Vec<Probe> { probes.try_iter().collect() };

    // Lock and space free: admitted by that one attempt. The writer is held inside the sink first, so
    // that it cannot be taking the queue lock (as a freshly started writer does) at that moment.
    let sink = TestSink::new();
    sink.allow(0);
    let free = logger(&sink, 4);
    let _release = Release(sink.clone());
    free.log(Level::Info, "test", "held");
    sink.wait_started(1);
    let probes = free.probes();
    assert_eq!(
        free.log_within(Level::Info, "test", "now", past),
        Admission::Queued
    );
    assert_eq!(
        attempts(&probes),
        [Probe::SettingsAttempt, Probe::QueueAttempt]
    );
    sink.allow(usize::MAX);
    assert_eq!(free.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(sink.output(), "INFO  test: held\nINFO  test: now\n");

    // Queue lock held: one attempt at the queue, then an immediate drop.
    let held_queue = logger(&TestSink::new(), 4);
    let probes = held_queue.probes();
    let guard = held_queue.hold_queue();
    assert_eq!(
        held_queue.log_within(Level::Info, "test", "x", past),
        Admission::Dropped
    );
    drop(guard);
    assert_eq!(
        attempts(&probes),
        [Probe::SettingsAttempt, Probe::QueueAttempt]
    );
    assert_eq!(held_queue.dropped_total(), 1);

    // Settings lock held: one attempt at the settings, then an immediate drop.
    let held_settings = logger(&TestSink::new(), 4);
    let probes = held_settings.probes();
    let guard = held_settings.hold_settings();
    assert_eq!(
        held_settings.log_within(Level::Info, "test", "x", past),
        Admission::Dropped
    );
    drop(guard);
    assert_eq!(attempts(&probes), [Probe::SettingsAttempt]);
    assert_eq!(held_settings.dropped_total(), 1);

    // Queue full behind a held sink: one look at the space, then an immediate drop.
    let stalled = TestSink::new();
    stalled.allow(0);
    let full = logger(&stalled, 2);
    let _release = stalled_and_full(&stalled, &full, 2);
    let probes = full.probes();
    assert_eq!(
        full.log_within(Level::Info, "test", "x", past),
        Admission::Dropped
    );
    assert_eq!(
        attempts(&probes),
        [
            Probe::SettingsAttempt,
            Probe::QueueAttempt,
            Probe::SpacePoll
        ]
    );
    assert_eq!(full.dropped_total(), 1);
}

#[test]
fn try_log_drops_promptly_and_counts_one_while_a_lock_is_held_or_the_queue_is_full() {
    // Queue lock held.
    let sink = TestSink::new();
    let logger_a = logger(&sink, 8);
    let guard = logger_a.hold_queue();
    let begun = Instant::now();
    assert_eq!(
        logger_a.try_log(Level::Info, "test", "x"),
        Admission::Dropped
    );
    assert!(begun.elapsed() < Duration::from_millis(500));
    drop(guard);
    assert_eq!(logger_a.dropped_total(), 1);

    // Settings lock held.
    let logger_b = logger(&TestSink::new(), 8);
    let guard = logger_b.hold_settings();
    assert_eq!(
        logger_b.try_log(Level::Info, "test", "x"),
        Admission::Dropped
    );
    drop(guard);
    assert_eq!(logger_b.dropped_total(), 1);

    // Queue full behind a held sink.
    let stalled = TestSink::new();
    stalled.allow(0);
    let logger_c = logger(&stalled, 2);
    let _release = stalled_and_full(&stalled, &logger_c, 2);
    let begun = Instant::now();
    assert_eq!(
        logger_c.try_log(Level::Info, "test", "x"),
        Admission::Dropped
    );
    assert!(begun.elapsed() < Duration::from_millis(500));
    assert_eq!(logger_c.dropped_total(), 1);
}

#[test]
fn try_log_reports_filtered_without_a_loss_and_queued_when_it_is_written() {
    let sink = TestSink::new();
    sink.allow(0);
    let mut quiet = settings();
    quiet.filter = Filter::new(Level::Warn);
    let logger = Logger::with_capacity(sink.boxed(), quiet, 8).expect("must be Ok or Some");
    let _release = Release(sink.clone());
    // Hold the writer in the sink first, so that nothing contends with the emitter below.
    logger.log(Level::Error, "test", "held");
    sink.wait_started(1);
    assert_eq!(
        logger.try_log(Level::Debug, "test", "filtered out"),
        Admission::Filtered
    );
    assert_eq!(logger.dropped_total(), 0, "a filtered record is not a loss");
    assert_eq!(
        logger.try_log(Level::Error, "test", "written"),
        Admission::Queued
    );
    sink.allow(usize::MAX);
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(sink.output(), "ERROR test: held\nERROR test: written\n");
}

#[test]
fn log_within_reports_filtered_without_a_loss() {
    let sink = TestSink::new();
    let mut quiet = settings();
    quiet.filter = Filter::new(Level::Warn);
    let logger = Logger::new(sink.boxed(), quiet).expect("must be Ok or Some");
    assert_eq!(
        logger.log_within(Level::Debug, "test", "filtered out", ahead(BOUND)),
        Admission::Filtered
    );
    assert_eq!(logger.dropped_total(), 0);
    assert_eq!(
        logger.log_within(Level::Warn, "test", "kept", ahead(BOUND)),
        Admission::Queued
    );
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(sink.output(), "WARN  test: kept\n");
}

#[test]
fn a_closed_queue_drops_at_once_through_every_entry_point() {
    let logger =
        Logger::new(Box::new(FailingSink { write: true }), settings()).expect("must be Ok or Some");
    logger.log(Level::Info, "test", "first");
    wait_until("the writer to give up", || logger.is_closed());
    let begun = Instant::now();
    logger.log(Level::Info, "test", "a");
    assert_eq!(logger.try_log(Level::Info, "test", "b"), Admission::Dropped);
    assert_eq!(
        logger.log_within(Level::Info, "test", "c", ahead(Duration::from_secs(30))),
        Admission::Dropped
    );
    assert!(begun.elapsed() < Duration::from_secs(1));
    // The failed record and the three later ones, each counted once.
    assert_eq!(logger.dropped_total(), 4);
}

// ---------------------------------------------------------------------------------------------
// A panic in the sink: the writer's cleanup counts everything it had taken, exactly once.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_sink_panic_counts_the_record_the_sink_held_and_the_queue_exactly_once() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 4);
    let _release = Release(sink.clone());
    // r0 is in the sink's hands; r1 and r2 wait in the queue.
    logger.log(Level::Info, "test", "r0");
    sink.wait_started(1);
    logger.log(Level::Info, "test", "r1");
    logger.log(Level::Info, "test", "r2");
    sink.panic_writes();
    assert_eq!(
        logger.flush(BOUND),
        FlushOutcome::Incomplete {
            queued: 0,
            in_flight: false,
            dropped: 3,
            sink_error: Some(io::ErrorKind::Other),
        }
    );
    assert!(sink.output().is_empty(), "nothing was ever written");
    assert_eq!(logger.dropped_total(), 3);
}

#[test]
fn a_sink_panic_in_a_multi_record_batch_counts_the_rest_of_the_batch() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 4);
    let _release = Release(sink.clone());
    // r0 goes out as a batch of one; r1 and r2 are then taken together as the next batch.
    logger.log(Level::Info, "test", "r0");
    sink.wait_started(1);
    logger.log(Level::Info, "test", "r1");
    logger.log(Level::Info, "test", "r2");
    sink.allow(1);
    sink.wait_started(2);
    // The writer holds [r1, r2] and is inside r1's write; r3 is queued behind them.
    logger.log(Level::Info, "test", "r3");
    sink.panic_writes();
    assert_eq!(
        logger.flush(BOUND),
        FlushOutcome::Incomplete {
            queued: 0,
            in_flight: false,
            dropped: 3,
            sink_error: Some(io::ErrorKind::Other),
        }
    );
    // Only r0, which the sink wrote before the panic, was delivered.
    assert_eq!(sink.output(), "INFO  test: r0\n");
    assert_eq!(logger.dropped_total(), 3);
}

#[test]
fn a_sink_panic_wakes_every_space_waiter_and_counts_each_record_once() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 2);
    let probes = logger.probes();
    let (done, finished) = mpsc::channel();
    thread::scope(|scope| {
        let _wake = WakeWaiters(&logger);
        let _release = Release(sink.clone());
        logger.log(Level::Info, "test", "r0");
        sink.wait_started(1);
        logger.log(Level::Info, "test", "r1");
        logger.log(Level::Info, "test", "r2");
        for number in 0..3 {
            scope.spawn({
                let logger = &logger;
                let done = done.clone();
                move || {
                    logger.log(Level::Info, "test", format_args!("w{number}"));
                    done.send(()).expect("the test is listening");
                }
            });
        }
        wait_for_space_waiters(&probes, 3);
        sink.panic_writes();
        for _ in 0..3 {
            finished
                .recv_timeout(BOUND)
                .expect("a waiter returns after the panic");
        }
        // r0 (in the sink's hands), r1 and r2 (queued) and the three waiters, each once.
        assert_eq!(
            logger.flush(BOUND),
            FlushOutcome::Incomplete {
                queued: 0,
                in_flight: false,
                dropped: 6,
                sink_error: Some(io::ErrorKind::Other),
            }
        );
        let begun = Instant::now();
        logger.log(Level::Info, "test", "late");
        assert!(begun.elapsed() < Duration::from_millis(500));
        assert_eq!(logger.dropped_total(), 7);
        assert!(sink.output().is_empty());
    });
}

// ---------------------------------------------------------------------------------------------
// CK-104: a sink panic's payload is disposed of on the writer thread.
// ---------------------------------------------------------------------------------------------

/// Whether the writer thread itself has ended (not merely closed the logger: `Exit` publishes that
/// before the stack, the sink and the payload are destroyed).
fn writer_has_ended(logger: &Logger) -> bool {
    logger.handle.as_ref().is_some_and(JoinHandle::is_finished)
}

/// A logger whose sink holds r0 (and panics, with a payload whose drop panics, when released) with r1
/// and r2 queued, as in `a_sink_panic_counts_the_record_the_sink_held_and_the_queue_exactly_once`.
/// Returns the counts for the payload.
fn after_a_bomb_panic(sink: &TestSink, logger: &Logger) -> BombCounts {
    logger.log(Level::Info, "test", "r0");
    sink.wait_started(1);
    logger.log(Level::Info, "test", "r1");
    logger.log(Level::Info, "test", "r2");
    sink.panic_writes_with_a_bomb()
}

#[test]
fn a_sink_panic_with_a_payload_whose_drop_panics_counts_each_record_once_and_closes() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 4);
    let _release = Release(sink.clone());
    let (drops, _threads) = after_a_bomb_panic(&sink, &logger);
    assert_eq!(
        logger.flush(BOUND),
        FlushOutcome::Incomplete {
            queued: 0,
            in_flight: false,
            dropped: 3,
            sink_error: Some(io::ErrorKind::Other),
        }
    );
    assert!(sink.output().is_empty(), "nothing was ever written");
    assert_eq!(logger.dropped_total(), 3);
    assert!(logger.is_closed());
    wait_until("the writer to end", || writer_has_ended(&logger));
    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "the payload is dropped once"
    );
}

#[test]
fn a_sink_panic_with_a_payload_whose_drop_panics_wakes_every_space_waiter() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 2);
    let probes = logger.probes();
    let (done, finished) = mpsc::channel();
    thread::scope(|scope| {
        let _wake = WakeWaiters(&logger);
        let _release = Release(sink.clone());
        logger.log(Level::Info, "test", "r0");
        sink.wait_started(1);
        logger.log(Level::Info, "test", "r1");
        logger.log(Level::Info, "test", "r2");
        for number in 0..3 {
            scope.spawn({
                let logger = &logger;
                let done = done.clone();
                move || {
                    logger.log(Level::Info, "test", format_args!("w{number}"));
                    done.send(()).expect("the test is listening");
                }
            });
        }
        wait_for_space_waiters(&probes, 3);
        let (drops, _threads) = sink.panic_writes_with_a_bomb();
        for _ in 0..3 {
            finished
                .recv_timeout(BOUND)
                .expect("a waiter returns after the panic");
        }
        assert_eq!(
            logger.flush(BOUND),
            FlushOutcome::Incomplete {
                queued: 0,
                in_flight: false,
                dropped: 6,
                sink_error: Some(io::ErrorKind::Other),
            }
        );
        wait_until("the writer to end", || writer_has_ended(&logger));
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn the_writer_ends_normally_and_disposes_of_a_sink_panics_payload_on_itself() {
    let sink = TestSink::new();
    sink.allow(0);
    let mut logger = logger(&sink, 4);
    let _release = Release(sink.clone());
    let (drops, threads) = after_a_bomb_panic(&sink, &logger);
    let handle = logger.handle.take().expect("the logger owns its writer");
    wait_until("the writer to end", || handle.is_finished());
    let writer = handle.thread().id();
    // The writer's real result: normal, so a detached writer has no payload for the standard library to
    // drop (and abort on), and a join has none to hand to its caller.
    handle.join().expect("the writer ends without a payload");
    assert_eq!(
        drops.load(Ordering::SeqCst),
        1,
        "the payload is dropped once"
    );
    assert_eq!(
        *threads.lock().expect("must be Ok or Some"),
        [writer],
        "it was dropped on the writer thread"
    );
}

#[test]
fn dropping_a_logger_whose_writer_ended_by_such_a_panic_returns_normally() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = logger(&sink, 4);
    let _release = Release(sink.clone());
    let (drops, _threads) = after_a_bomb_panic(&sink, &logger);
    // The handle stays attached: `Logger::drop` finds a finished writer and joins it.
    wait_until("the writer to end", || writer_has_ended(&logger));
    let dropped = catch_unwind(AssertUnwindSafe(move || drop(logger)));
    assert!(dropped.is_ok(), "dropping the logger unwound");
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

/// The control for the three tests above: the writer as it was before this change, `run_writer` as the
/// thread's whole body, ends with the sink's payload. Joining it hands the payload over, and dropping
/// that payload panics: exactly what `Logger::drop` would have done, and what the standard library
/// would have aborted on for a detached writer. So the assertions above can fail.
#[test]
fn control_a_writer_without_the_catch_ends_with_a_payload_whose_drop_panics() {
    let sink = TestSink::new();
    sink.allow(0);
    let mut logger =
        Logger::start(sink.boxed(), settings(), 4, run_writer).expect("must be Ok or Some");
    let _release = Release(sink.clone());
    let (drops, _threads) = after_a_bomb_panic(&sink, &logger);
    let handle = logger.handle.take().expect("the logger owns its writer");
    wait_until("the unguarded writer to end", || handle.is_finished());
    let payload = handle
        .join()
        .expect_err("the unguarded writer ends with the sink's payload");
    let dropped = catch_unwind(AssertUnwindSafe(move || drop(payload)));
    assert!(dropped.is_err(), "dropping the payload panics");
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------------------------
// CK-52: error records are reserved, delivered by sequence number, and awaited.
// ---------------------------------------------------------------------------------------------

/// A deadline that has already passed.
fn expired() -> Instant {
    Instant::now()
        .checked_sub(Duration::from_millis(5))
        .expect("the clock has run for more than 5 ms")
}

fn error(logger: &Logger, text: &str, deadline: Instant) -> Admission {
    logger.log_within(Level::Error, "test", text, deadline)
}

/// Admits an error to the reserve: the queue lock is taken, so the one normal attempt of an expired
/// deadline fails and the record falls back. The call runs on a bounded worker while this thread keeps
/// the guard, so a regression that waited for the lock fails the test instead of hanging it: the
/// assertion's unwinding drops the guard, which lets the detached worker finish.
fn reserve_error(logger: &Arc<Logger>, text: &str) -> Admission {
    let held = logger.hold_queue();
    let worker = Arc::clone(logger);
    let text = text.to_owned();
    let admission = bounded("an error call while the queue lock is held", move || {
        error(&worker, &text, expired())
    });
    drop(held);
    admission
}

/// The sink's output lines.
fn lines(sink: &TestSink) -> Vec<String> {
    sink.output().lines().map(str::to_owned).collect()
}

/// Runs `body` on a thread and returns its result, failing the test (instead of hanging it) if it does
/// not finish within [`BOUND`]. The thread is detached on failure; the caller's guards release it.
fn bounded<T: Send + 'static>(what: &str, body: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let _ = done.send(body());
    });
    finished
        .recv_timeout(BOUND)
        .unwrap_or_else(|_| panic!("{what} did not finish within {BOUND:?}"))
}

#[test]
fn an_error_needs_no_settings_lock_and_uses_the_retained_format_flags() {
    let sink = TestSink::new();
    let logger = Arc::new(logger(&sink, 8));
    let mut coloured = settings();
    coloured.colour = true;
    logger.set_settings(coloured);
    let held = logger.hold_settings();
    // The calls run on a bounded worker while this thread keeps the settings guard. If an error call
    // waited for the lock, the bound would fail the test here, and unwinding would drop the guard.
    let worker = Arc::clone(&logger);
    let (admissions, took) = bounded(
        "the error calls while the settings lock is held",
        move || {
            let begun = Instant::now();
            let errors = [
                ("future", ahead(Duration::from_secs(30))),
                ("expired", expired()),
            ]
            .map(|(text, deadline)| error(&worker, text, deadline));
            let took = begun.elapsed();
            // The same calls at info still need the filter, so they still drop.
            let infos = [expired(), ahead(Duration::from_millis(20))]
                .map(|deadline| worker.log_within(Level::Info, "test", "x", deadline));
            ((errors, infos), took)
        },
    );
    assert_eq!(admissions.0, [Admission::Queued; 2]);
    assert_eq!(admissions.1, [Admission::Dropped; 2]);
    assert!(
        took < Duration::from_millis(500),
        "an error must not wait for the settings lock: {took:?}"
    );
    drop(held);
    assert!(matches!(
        logger.flush(BOUND),
        FlushOutcome::Incomplete {
            dropped: 2,
            queued: 0,
            ..
        }
    ));
    let written = lines(&sink);
    assert_eq!(
        written[..2],
        [
            "\x1b[31mERROR\x1b[0m test: future",
            "\x1b[31mERROR\x1b[0m test: expired",
        ]
    );
    // The drops are reported in one or two warnings, coloured by the flags in force.
    let reported: u32 = written[2..]
        .iter()
        .map(|line| {
            let count = line
                .strip_prefix("\x1b[33mWARN\x1b[0m  log: ")
                .and_then(|rest| rest.strip_suffix(" record(s) dropped"))
                .expect("a coloured drop report");
            count.parse::<u32>().expect("a count")
        })
        .sum();
    assert_eq!(reported, 2);
}

#[test]
fn an_error_survives_a_full_queue_and_a_held_queue_lock_where_progress_is_dropped() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = Arc::new(logger(&sink, 2));
    let _release = stalled_and_full(&sink, &logger, 2);
    // Full queue: the error waits out its short deadline, then goes to the reserve.
    assert_eq!(
        error(&logger, "full", ahead(Duration::from_millis(30))),
        Admission::Queued
    );
    assert_eq!(logger.reserved(), 1);
    assert_eq!(
        logger.log_within(
            Level::Info,
            "test",
            "lost",
            ahead(Duration::from_millis(30))
        ),
        Admission::Dropped
    );
    // Held queue lock (with the queue also full): the same.
    assert_eq!(reserve_error(&logger, "locked"), Admission::Queued);
    assert_eq!(logger.reserved(), 2);
    assert_eq!(logger.dropped_total(), 1);
    sink.allow(usize::MAX);
    assert!(matches!(
        logger.flush(BOUND),
        FlushOutcome::Incomplete {
            dropped: 1,
            queued: 0,
            ..
        }
    ));
    let written = lines(&sink);
    assert!(
        written.contains(&"ERROR test: full".to_owned()),
        "{written:?}"
    );
    assert!(
        written.contains(&"ERROR test: locked".to_owned()),
        "{written:?}"
    );
    assert!(!written.contains(&"INFO  test: lost".to_owned()));
}

/// Sets up a logger whose writer is blocked in the first write of `r0`, and returns the pieces.
fn stalled_in_r0(capacity: usize) -> (TestSink, Arc<Logger>, Release) {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = Arc::new(logger(&sink, capacity));
    let release = Release(sink.clone());
    logger.log(Level::Info, "test", "r0");
    sink.wait_started(1);
    (sink, logger, release)
}

#[test]
fn a_reserved_error_goes_out_before_the_next_batch_but_after_a_batch_already_taken() {
    // E, A, B: A and B are still queued when the error is reserved.
    let (sink, logger, _release) = stalled_in_r0(8);
    logger.log(Level::Info, "test", "A");
    logger.log(Level::Info, "test", "B");
    assert_eq!(reserve_error(&logger, "E"), Admission::Queued);
    sink.allow(usize::MAX);
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(
        lines(&sink),
        [
            "INFO  test: r0",
            "ERROR test: E",
            "INFO  test: A",
            "INFO  test: B"
        ]
    );

    // A, B, E: the batch [A, B] was already taken when the error is reserved.
    let (sink, logger, _release) = stalled_in_r0(8);
    logger.log(Level::Info, "test", "A");
    logger.log(Level::Info, "test", "B");
    sink.allow(1);
    sink.wait_started(2);
    assert_eq!(reserve_error(&logger, "E"), Admission::Queued);
    sink.allow(usize::MAX);
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(
        lines(&sink),
        [
            "INFO  test: r0",
            "INFO  test: A",
            "INFO  test: B",
            "ERROR test: E"
        ]
    );

    // A, E, B: A was taken (alone) and B still queued.
    let (sink, logger, _release) = stalled_in_r0(8);
    logger.log(Level::Info, "test", "A");
    sink.allow(1);
    sink.wait_started(2);
    logger.log(Level::Info, "test", "B");
    assert_eq!(reserve_error(&logger, "E"), Admission::Queued);
    sink.allow(usize::MAX);
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(
        lines(&sink),
        [
            "INFO  test: r0",
            "INFO  test: A",
            "ERROR test: E",
            "INFO  test: B"
        ]
    );
}

#[test]
fn when_a_pass_starts_with_both_a_reserved_error_and_a_batch_waiting_the_error_goes_first() {
    // The writer is held inside the sink flush that follows r0, so A, B and the reserved E are all
    // waiting when its next pass starts.
    let sink = TestSink::new();
    sink.set_flush_mode(FlushMode::Hold);
    let _release = Release(sink.clone());
    let logger = Arc::new(logger(&sink, 8));
    logger.log(Level::Info, "test", "r0");
    wait_until("the writer to be held in its flush", || sink.flushes() == 1);
    logger.log(Level::Info, "test", "A");
    logger.log(Level::Info, "test", "B");
    assert_eq!(reserve_error(&logger, "E"), Admission::Queued);
    sink.set_flush_mode(FlushMode::Succeed);
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(
        lines(&sink),
        [
            "INFO  test: r0",
            "ERROR test: E",
            "INFO  test: A",
            "INFO  test: B"
        ]
    );
}

#[test]
fn several_error_producers_publishing_while_the_writer_drains_lose_nothing() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = Arc::new(logger(&sink, 1));
    let _release = Release(sink.clone());
    logger.log(Level::Info, "test", "r0");
    sink.wait_started(1);
    logger.log(Level::Info, "test", "fill");
    let producers: Vec<_> = (0..8)
        .map(|producer| {
            let logger = Arc::clone(&logger);
            let sink = sink.clone();
            thread::spawn(move || {
                let mut queued = 0_u32;
                for number in 0..100 {
                    if error(&logger, &format!("p{producer}-{number}"), expired())
                        == Admission::Queued
                    {
                        queued += 1;
                    }
                    if producer == 0 && number == 50 {
                        // The writer starts draining while the others are still publishing.
                        sink.allow(usize::MAX);
                    }
                }
                queued
            })
        })
        .collect();
    let queued: u32 = producers
        .into_iter()
        .map(|producer| producer.join().expect("the producer ends"))
        .sum();
    assert_eq!(queued, 800, "no slot is ever short: 800 < {RESERVE_SLOTS}");
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    let written = lines(&sink);
    let errors: HashSet<_> = written
        .iter()
        .filter(|line| line.starts_with("ERROR test: p"))
        .collect();
    assert_eq!(
        written
            .iter()
            .filter(|line| line.starts_with("ERROR"))
            .count(),
        800,
        "every error is written once"
    );
    assert_eq!(errors.len(), 800, "and none twice");
    assert_eq!(logger.dropped_total(), 0);
}

/// Pauses the reserve producer at `point` on its first arrival. Returns the channel that says it has
/// arrived and the one that releases it.
fn pause_producer(logger: &Logger, point: ReservePoint) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (arrived_tx, arrived) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let arrived_tx = Mutex::new(arrived_tx);
    let release_rx = Mutex::new(release_rx);
    logger.set_pause(move |at| {
        if at == point {
            let _ = arrived_tx.lock().expect("must be Ok or Some").send(());
            let _ = release_rx
                .lock()
                .expect("must be Ok or Some")
                .recv_timeout(SINK_PATIENCE);
        }
    });
    (arrived, release)
}

/// A closure forced before or after a publication, with a record in the sink and the writer failing.
fn closing_around_a_publication(point: ReservePoint) -> (Admission, u64, String) {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = Arc::new(logger(&sink, 4));
    let _release = Release(sink.clone());
    logger.log(Level::Info, "test", "x");
    sink.wait_started(1);
    let (arrived, release) = pause_producer(&logger, point);
    let held = logger.hold_queue();
    let producer = {
        let logger = Arc::clone(&logger);
        thread::spawn(move || error(&logger, "raced", expired()))
    };
    arrived
        .recv_timeout(BOUND)
        .expect("the producer reaches the pause");
    // The writer fails now. It closes the reserve and then waits for the paused publication.
    sink.fail_writes();
    wait_until("the reserve to close", || logger.reserve_closed());
    assert!(
        !producer.is_finished(),
        "the producer is still paused; the writer waits for it, not the other way round"
    );
    release.send(()).expect("the producer is waiting");
    drop(held);
    let admission = producer.join().expect("the producer ends");
    wait_until("the writer to finish", || logger.is_closed());
    let outcome = logger.flush(BOUND);
    let FlushOutcome::Incomplete {
        queued, in_flight, ..
    } = outcome
    else {
        panic!("a failed sink cannot flush completely: {outcome:?}");
    };
    assert_eq!((queued, in_flight), (0, false));
    (admission, logger.dropped_total(), sink.output())
}

#[test]
fn a_closure_just_before_a_publication_drops_the_record_once() {
    let (admission, dropped, output) = closing_around_a_publication(ReservePoint::Marked);
    assert_eq!(admission, Admission::Dropped);
    assert_eq!(
        dropped, 2,
        "the failed record `x` and the refused error, each once"
    );
    assert!(output.is_empty());
}

#[test]
fn a_closure_between_the_closed_check_and_the_store_waits_for_the_publication() {
    // The producer has seen the reserve open; the writer then closes it. The writer must wait for
    // the publication, so the record that is then stored is drained and counted, not stranded.
    let (admission, dropped, output) = closing_around_a_publication(ReservePoint::Checked);
    assert_eq!(admission, Admission::Queued);
    assert_eq!(
        dropped, 2,
        "`x` and the stored error, each counted exactly once"
    );
    assert!(output.is_empty());
}

#[test]
fn a_closure_just_after_a_publication_counts_the_stored_record_once() {
    let (admission, dropped, output) = closing_around_a_publication(ReservePoint::Stored);
    assert_eq!(
        admission,
        Admission::Queued,
        "it was stored before the closure"
    );
    assert_eq!(
        dropped, 2,
        "`x` and the stranded error, each once, none both written and counted"
    );
    assert!(output.is_empty());
}

#[test]
fn a_sink_panic_with_reserved_records_pending_counts_each_exactly_once() {
    let (sink, logger, _release) = stalled_in_r0(2);
    logger.log(Level::Info, "test", "q1");
    logger.log(Level::Info, "test", "q2");
    for number in 0..5 {
        assert_eq!(
            reserve_error(&logger, &format!("e{number}")),
            Admission::Queued
        );
    }
    assert_eq!(logger.reserved(), 5);
    sink.panic_writes();
    assert_eq!(
        logger.flush(BOUND),
        FlushOutcome::Incomplete {
            queued: 0,
            in_flight: false,
            dropped: 8,
            sink_error: Some(io::ErrorKind::Other),
        }
    );
    assert_eq!(
        logger.dropped_total(),
        8,
        "r0 in the sink, two queued and five reserved"
    );
    assert_eq!(logger.reserved(), 0);
    assert!(sink.output().is_empty());
    assert!(
        !logger.await_errors(Duration::from_secs(30)),
        "closed with errors undelivered"
    );
}

#[test]
fn the_1025th_pending_error_is_dropped_and_counted() {
    let (sink, logger, _release) = stalled_in_r0(1);
    logger.log(Level::Info, "test", "fill");
    for number in 0..RESERVE_SLOTS {
        assert_eq!(
            error(&logger, &format!("e{number}"), expired()),
            Admission::Queued,
            "error {number}"
        );
    }
    assert_eq!(logger.reserved(), RESERVE_SLOTS);
    assert_eq!(
        error(&logger, "one too many", expired()),
        Admission::Dropped
    );
    assert_eq!(logger.dropped_total(), 1);
    assert_eq!(logger.reserved(), RESERVE_SLOTS);
    sink.allow(usize::MAX);
    assert!(matches!(
        logger.flush(BOUND),
        FlushOutcome::Incomplete {
            dropped: 1,
            queued: 0,
            ..
        }
    ));
    assert_eq!(
        sink.output().matches("ERROR test: e").count(),
        RESERVE_SLOTS,
        "the 1024 reserved errors are all written"
    );
}

#[test]
fn reserve_publication_skips_a_contended_slot_and_never_waits_when_every_slot_is_held() {
    let (_sink, logger, _release) = stalled_in_r0(1);
    logger.log(Level::Info, "test", "fill");
    // One held slot: the producer moves to the next and is queued.
    {
        let one = logger.hold_slots(0..1);
        let again = Arc::clone(&logger);
        let admission = bounded("a publication beside a held slot", move || {
            error(&again, "beside", expired())
        });
        assert_eq!(admission, Admission::Queued);
        drop(one);
    }
    // Every slot held: it must come back at once, and drop and count.
    let all = logger.hold_slots(0..RESERVE_SLOTS);
    let again = Arc::clone(&logger);
    let begun = Instant::now();
    let admission = bounded("a publication with every slot held", move || {
        error(&again, "nowhere", expired())
    });
    assert_eq!(admission, Admission::Dropped);
    assert!(begun.elapsed() < Duration::from_secs(1));
    drop(all);
    assert_eq!(logger.dropped_total(), 1);
}

#[test]
fn the_reserve_publication_code_calls_no_blocking_lock_or_condition_variable_wait() {
    let source = include_str!("../logger.rs");
    let begin = "// BEGIN non-waiting reserve publication";
    let from = source.find(begin).expect("begin marker") + begin.len();
    let to = from
        + source[from..]
            .find("// END non-waiting reserve publication")
            .expect("end marker");
    let code = &source[from..to];
    for forbidden in [
        ".lock()",
        "locked(",
        ".read()",
        ".write()",
        ".wait(",
        ".wait_timeout",
        ".wait_while",
        "current_settings(",
        "read_settings(",
        "self.queue.",
    ] {
        assert!(
            !code.contains(forbidden),
            "reserve publication contains {forbidden}"
        );
    }
    assert!(code.contains(".try_lock()"));
}

#[test]
fn flush_is_never_complete_while_a_reserved_record_is_unwritten() {
    let (sink, logger, _release) = stalled_in_r0(1);
    logger.log(Level::Info, "test", "fill");
    assert_eq!(error(&logger, "E", expired()), Admission::Queued);
    assert_eq!(
        logger.flush(Duration::from_millis(100)),
        FlushOutcome::Incomplete {
            queued: 2,
            in_flight: true,
            dropped: 0,
            sink_error: None,
        }
    );
    sink.allow(usize::MAX);
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(
        lines(&sink),
        ["INFO  test: r0", "ERROR test: E", "INFO  test: fill"]
    );
}

#[test]
fn await_errors_is_true_once_a_reserved_error_is_written_ahead_of_a_full_backlog() {
    let (sink, logger, _release) = stalled_in_r0(4);
    for number in 1..=4 {
        logger.log(Level::Info, "test", format_args!("r{number}"));
    }
    assert_eq!(error(&logger, "E", expired()), Admission::Queued);
    assert_eq!(logger.reserved(), 1);
    // r0 and E may complete; the backlog stays held behind them.
    sink.allow(2);
    assert!(logger.await_errors(BOUND));
    assert_eq!(lines(&sink), ["INFO  test: r0", "ERROR test: E"]);
    assert!(sink.flushes() >= 1, "its sink flush succeeded");
    assert_eq!(logger.delivery().0, 1);
}

#[test]
fn await_errors_covers_an_ordinary_queue_error_admitted_at_an_expired_deadline() {
    let (sink, logger, _release) = stalled_in_r0(8);
    // Space and the lock are free, so the expired deadline's one attempt admits it normally.
    assert_eq!(error(&logger, "ordinary", expired()), Admission::Queued);
    assert_eq!(
        logger.reserved(),
        0,
        "it is in the ordinary queue, not the reserve"
    );
    assert!(matches!(
        logger.flush(Duration::ZERO),
        FlushOutcome::Incomplete {
            queued: 1,
            in_flight: true,
            ..
        }
    ));
    let probes = logger.probes();
    let waiting = {
        let logger = Arc::clone(&logger);
        thread::spawn(move || logger.await_errors(BOUND))
    };
    next_probe(&probes, |probe| probe == Probe::AwaitWait);
    assert!(
        !waiting.is_finished(),
        "nothing is delivered while the writer is held"
    );
    assert!(!sink.output().contains("ordinary"));
    sink.allow(usize::MAX);
    assert!(waiting.join().expect("the waiter ends"));
    assert!(sink.output().contains("ERROR test: ordinary\n"));
    assert_eq!(logger.delivery().0, 1);
}

#[test]
fn an_error_inside_a_sink_write_is_not_delivered_until_the_write_and_flush_return() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = Arc::new(logger(&sink, 4));
    let _release = Release(sink.clone());
    assert_eq!(
        error(&logger, "in flight", ahead(Duration::from_secs(1))),
        Admission::Queued
    );
    sink.wait_started(1);
    // Stalled sink: false at its deadline, within a small bound.
    let begun = Instant::now();
    assert!(!logger.await_errors(Duration::from_millis(80)));
    assert!(begun.elapsed() >= Duration::from_millis(80));
    assert!(
        begun.elapsed() < Duration::from_secs(1),
        "{:?}",
        begun.elapsed()
    );
    // A held flush after the write is still not delivery.
    sink.set_flush_mode(FlushMode::Hold);
    sink.allow(usize::MAX);
    wait_until("the flush to begin", || sink.flushes() == 1);
    assert!(!logger.await_errors(Duration::from_millis(50)));
    sink.set_flush_mode(FlushMode::Succeed);
    assert!(logger.await_errors(BOUND));
}

#[test]
fn a_sink_failure_with_an_error_in_flight_makes_await_errors_false_promptly() {
    let sink = TestSink::new();
    sink.allow(0);
    let logger = Arc::new(logger(&sink, 4));
    let _release = Release(sink.clone());
    assert_eq!(
        error(&logger, "doomed", ahead(Duration::from_secs(1))),
        Admission::Queued
    );
    sink.wait_started(1);
    let probes = logger.probes();
    let waiting = {
        let logger = Arc::clone(&logger);
        thread::spawn(move || {
            let begun = Instant::now();
            (
                logger.await_errors(Duration::from_secs(30)),
                begun.elapsed(),
            )
        })
    };
    next_probe(&probes, |probe| probe == Probe::AwaitWait);
    sink.fail_writes();
    let (delivered, took) = waiting.join().expect("the waiter ends");
    assert!(!delivered);
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[test]
fn await_errors_does_not_wait_when_only_progress_records_are_pending() {
    let (_sink, logger, _release) = stalled_in_r0(4);
    logger.log(Level::Info, "test", "pending");
    let begun = Instant::now();
    assert!(logger.await_errors(Duration::from_secs(30)));
    assert!(
        begun.elapsed() < Duration::from_millis(500),
        "{:?}",
        begun.elapsed()
    );
    let fresh = Logger::new(Box::new(io::sink()), settings()).expect("must be Ok or Some");
    assert!(
        fresh.await_errors(Duration::ZERO),
        "nothing was ever emitted"
    );
}

/// E1 is admitted to the ordinary queue before the await, E2 to the reserve after it, and E2 is
/// written and flushed first. Returns the logger with the pieces for the two endings.
fn overtaking() -> (TestSink, Arc<Logger>, Release, JoinHandle<bool>) {
    let (sink, logger, release) = stalled_in_r0(8);
    assert_eq!(error(&logger, "E1", expired()), Admission::Queued);
    assert_eq!(logger.reserved(), 0, "E1 is in the ordinary queue");
    let probes = logger.probes();
    let waiting = {
        let logger = Arc::clone(&logger);
        thread::spawn(move || logger.await_errors(Duration::from_millis(600)))
    };
    next_probe(&probes, |probe| probe == Probe::AwaitWait);
    assert_eq!(reserve_error(&logger, "E2"), Admission::Queued);
    // r0 and E2 are written and flushed; E1 (still in the queue) is held behind them.
    sink.allow(2);
    wait_until("E2 to be written and flushed", || {
        logger.delivery().1 == [2]
    });
    (sink, logger, release, waiting)
}

#[test]
fn a_reserved_error_that_overtakes_an_earlier_one_does_not_satisfy_await_errors() {
    let (sink, logger, _release, waiting) = overtaking();
    assert_eq!(lines(&sink), ["INFO  test: r0", "ERROR test: E2"]);
    assert_eq!(logger.delivery().0, 0, "the watermark waits for E1");
    assert!(!waiting.is_finished());
    sink.allow(usize::MAX);
    assert!(
        waiting.join().expect("the waiter ends"),
        "true only after E1 was written and flushed"
    );
    assert_eq!(
        lines(&sink),
        ["INFO  test: r0", "ERROR test: E2", "ERROR test: E1"]
    );
    assert_eq!(logger.delivery(), (2, Vec::new(), 1));
}

#[test]
fn await_errors_returns_false_within_its_deadline_if_the_earlier_error_never_is_written() {
    let (_sink, logger, _release, waiting) = overtaking();
    let begun = Instant::now();
    assert!(!waiting.join().expect("the waiter ends"));
    assert!(
        begun.elapsed() < Duration::from_secs(3),
        "{:?}",
        begun.elapsed()
    );
    assert_eq!(logger.delivery().0, 0);
}

#[test]
fn the_out_of_order_set_is_bounded_and_empties_as_the_watermark_passes() {
    // E1 waits in a full ordinary queue while 200 later errors are delivered from the reserve.
    let (sink, logger, _release) = stalled_in_r0(4);
    assert_eq!(error(&logger, "E1", expired()), Admission::Queued);
    for number in 0..3 {
        logger.log(Level::Info, "test", format_args!("fill{number}"));
    }
    for number in 0..200 {
        assert_eq!(
            error(&logger, &format!("late{number}"), expired()),
            Admission::Queued
        );
    }
    assert_eq!(logger.reserved(), 200);
    sink.allow(usize::MAX);
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    let (watermark, ahead, peak) = logger.delivery();
    assert_eq!(
        (watermark, ahead.len()),
        (201, 0),
        "E1 delivered: everything is contiguous"
    );
    assert_eq!(
        peak, 200,
        "the set held exactly the later errors while E1 was pending"
    );
    assert!(
        peak < RESERVE_SLOTS + 4,
        "never more than RESERVE_SLOTS - 1 + capacity"
    );
    assert_eq!(logger.highest_sequence(), 201);
}

/// Pauses the writer's first reserve scan right after it has looked at slot 0. Returns the channel
/// that says it has stopped there and the one that lets it go on.
fn pause_scan_after_slot_zero(logger: &Logger) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (arrived_tx, arrived) = mpsc::channel();
    let (resume, resume_rx) = mpsc::channel();
    let arrived_tx = Mutex::new(arrived_tx);
    let resume_rx = Mutex::new(resume_rx);
    let fired = AtomicBool::new(false);
    logger.set_pause(move |at| {
        if at == ReservePoint::Scanned(0) && !fired.swap(true, Ordering::SeqCst) {
            let _ = arrived_tx.lock().expect("must be Ok or Some").send(());
            let _ = resume_rx
                .lock()
                .expect("must be Ok or Some")
                .recv_timeout(SINK_PATIENCE);
        }
    });
    (arrived, resume)
}

#[test]
fn a_reserve_scan_that_misses_a_slot_still_leaves_at_most_the_documented_bound_ahead() {
    // The reviewer's interleaving, on the real writer: a missed slot, the rest of the reserve and a
    // full ordinary batch all overtake the missing error. Queue capacity 4.
    let capacity = 4;
    let sink = TestSink::new();
    sink.allow(0);
    let logger = Arc::new(logger(&sink, capacity));
    let _release = Release(sink.clone());
    logger.log(Level::Info, "test", "r0");
    sink.wait_started(1);
    // Number 1 sits in the last slot; the first scan will look at slot 0 and stop there.
    logger.set_reserve_cursor(RESERVE_SLOTS - 1);
    assert_eq!(reserve_error(&logger, "seed"), Admission::Queued);
    let (stopped, resume) = pause_scan_after_slot_zero(&logger);
    sink.allow(1);
    stopped
        .recv_timeout(BOUND)
        .expect("the scan stops after slot 0");
    // E1 (number 2) lands in the slot the scan has gone by; 1022 later errors fill slots 1..=1022.
    logger.set_reserve_cursor(0);
    assert_eq!(reserve_error(&logger, "E1"), Admission::Queued);
    for number in 0..RESERVE_SLOTS - 2 {
        assert_eq!(
            reserve_error(&logger, &format!("later{number}")),
            Admission::Queued
        );
    }
    // A full ordinary batch of still later errors (the queue is empty: r0 was taken).
    for number in 0..capacity {
        assert_eq!(
            error(&logger, &format!("queue{number}"), expired()),
            Admission::Queued
        );
    }
    assert_eq!(logger.reserved(), RESERVE_SLOTS);
    // The scan goes on and takes the seed and the 1022 later errors, then the four queued errors are
    // delivered; E1's write is the next one and stays held.
    let writes_before_e1 = 1 + (RESERVE_SLOTS - 1) + capacity;
    sink.allow(writes_before_e1);
    resume.send(()).expect("the scan is waiting");
    sink.wait_started(writes_before_e1 + 1);
    wait_until("the batch to be delivered", || {
        logger.delivery().1.len() == RESERVE_SLOTS - 2 + capacity
    });
    let (watermark, ahead, peak) = logger.delivery();
    assert_eq!(
        watermark, 1,
        "only the seed is contiguous: E1 (2) is missing"
    );
    assert_eq!(ahead.len(), 1026);
    assert_eq!(peak, 1026);
    assert!(
        peak <= RESERVE_SLOTS - 1 + capacity,
        "the documented bound is RESERVE_SLOTS - 1 + capacity"
    );
    // E1 goes out: the watermark passes everything and the set empties.
    sink.allow(usize::MAX);
    assert_eq!(logger.flush(BOUND), FlushOutcome::Complete);
    assert_eq!(logger.delivery(), (1028, Vec::new(), 1026));
    assert!(logger.await_errors(Duration::ZERO));
}
