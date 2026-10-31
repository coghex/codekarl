//! The process lifecycle around the runtime: the logger on stdout, the control-path emission rule, the
//! panic hook and the two exits.
//!
//! **Control-path rule.** The main thread and the panic hook own the process exit, so every record they
//! emit goes through `ck_log::log_within`, never through the waiting macros: a full log queue behind a
//! stalled stdout must not stall them. A record's deadline is the earlier of now plus [`LOG_WAIT`] and the
//! shutdown deadline, if one is already active.
//!
//! **Exit.** One decision function ([`decide`]) maps the logger's flush result to "return the status" or
//! "end the process at once with status 3". Step 7 ([`conclude`]) and the panic hook ([`handle_panic`])
//! both use it, and both take the flush, the error grace and the exit as parameters, so tests inject them.
//!
//! **The resource latch** ([`ResourceLatch`]). The windowed run sets it immediately before it creates its
//! first native or GPU object and clears it only when `ck-window`'s teardown is proven (every end other
//! than `Unproven`). The panic hook reads it: a panic on **any thread that is not a `ck-lua` VM owner**
//! (`ck-lua-<name>`) while it is set is treated as an incomplete shutdown whatever the flush says, and the
//! process ends at once with status 3 instead of unwinding over resources that may still be in use. VM-owner
//! panics keep their accepted handling (the owner's failure, then the normal shutdown).
//!
//! **Error grace.** Progress records may be lost under load; error records are reserved by the logger
//! and, when the flush was incomplete, given one more bounded wait before the process ends ([`ERROR_GRACE`],
//! spent at most once per process by [`spend_grace`]). With no error record unwritten the wait returns at
//! once, so a normal run never pauses. The exit status is unchanged: an incomplete flush still ends with
//! status 3, even when the errors were then written.

use std::fmt;
use std::io;
use std::panic::{self, PanicHookInfo};
use std::process::ExitCode;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ck_log::{FlushOutcome, Level, Logger, Settings};

/// How long a record emitted on the control path may wait to be admitted.
pub(crate) const LOG_WAIT: Duration = Duration::from_millis(100);
/// How long the final flush may take, from the moment shutdown begins.
pub(crate) const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);
/// The longest the process waits, once, for an error record that is still unwritten at exit.
pub(crate) const ERROR_GRACE: Duration = Duration::from_secs(2);
/// The exit status when the log was not fully written.
pub(crate) const INCOMPLETE_SHUTDOWN: u8 = 3;

/// The deadline of the shutdown, recorded when step 7 starts so that a panic during shutdown reuses it
/// instead of starting a new one.
pub(crate) static SHUTDOWN_DEADLINE: OnceLock<Instant> = OnceLock::new();

/// Whether this process has already spent its one error grace.
static GRACE_SPENT: AtomicBool = AtomicBool::new(false);

/// The resource latch: native or GPU objects may exist and no proven teardown has released them.
pub(crate) struct ResourceLatch(AtomicBool);

impl ResourceLatch {
    /// A latch that is not set.
    pub(crate) const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    /// Sets the latch: called immediately before the first native or GPU object is created.
    pub(crate) fn set(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Clears the latch: called only when the teardown is proven.
    pub(crate) fn clear(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    /// Whether resources may still be in use.
    pub(crate) fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// The process's latch, set and cleared by the windowed run and read by the panic hook.
pub(crate) static RESOURCES: ResourceLatch = ResourceLatch::new();

/// The thread-name prefix `ck-lua` gives its VM owner threads.
const VM_OWNER_PREFIX: &str = "ck-lua-";

/// Whether a panic on a thread called `name` leaves resources unproven: the latch is set and the thread is
/// not a VM owner (any other thread, an unnamed one included, may have been using them).
pub(crate) fn leaves_resources_unproven(latch_set: bool, name: Option<&str>) -> bool {
    latch_set && !name.is_some_and(|name| name.starts_with(VM_OWNER_PREFIX))
}

/// The deadline of a control-path record: the earlier of now plus [`LOG_WAIT`] and `shutdown`.
pub(crate) fn control_deadline(shutdown: Option<Instant>) -> Instant {
    let own = Instant::now() + LOG_WAIT;
    shutdown.map_or(own, |shutdown| shutdown.min(own))
}

/// Emits one record from the main thread through `log_within`. A record that is not admitted in time is
/// dropped and counted by the logger, which makes the final flush incomplete (status 3): it is not
/// reported here.
pub(crate) fn emit(level: Level, category: &str, message: fmt::Arguments<'_>) {
    let deadline = control_deadline(SHUTDOWN_DEADLINE.get().copied());
    let _ = ck_log::log_within!(level, deadline, category, "{message}");
}

/// The settings every record is written under until the configuration is applied: level `info`, no
/// timestamps, no colour.
pub(crate) fn fixed_settings() -> Settings {
    Settings {
        filter: ck_log::Filter::new(Level::Info),
        timestamps: false,
        colour: false,
    }
}

/// Installs the process-wide logger on stdout with the fixed settings.
pub(crate) fn install_logger() -> io::Result<()> {
    let logger = Logger::new(Box::new(io::stdout()), fixed_settings())?;
    ck_log::install(logger).map_err(io::Error::other)
}

/// Replaces the settings of the installed logger.
pub(crate) fn apply_settings(settings: Settings) {
    if let Some(logger) = ck_log::installed() {
        logger.set_settings(settings);
    }
}

/// Flushes the installed logger within `deadline`; with no logger installed there is nothing unwritten.
pub(crate) fn flush_logger(deadline: Duration) -> FlushOutcome {
    ck_log::installed().map_or(FlushOutcome::Complete, |logger| logger.flush(deadline))
}

/// How a run ended, as the executable sees it: the status code, and whether an owner missed a deadline
/// (design §4.4), which ends the process at once with status 3 even when the log was fully written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Status {
    /// 0 for a clean run, otherwise 1.
    pub(crate) code: u8,
    /// Some owner missed a deadline or was not observed finished: the shutdown is incomplete.
    pub(crate) owners_incomplete: bool,
}

/// What to do once the logger has been flushed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Exit {
    /// Carry on and return this status normally.
    Return(u8),
    /// End the process at once with [`INCOMPLETE_SHUTDOWN`].
    Immediately,
}

/// The one decision: a complete flush returns the status, and ends the process at once only when an
/// owner left the shutdown incomplete; an incomplete flush always ends it at once.
pub(crate) fn decide(outcome: &FlushOutcome, status: Status) -> Exit {
    match outcome {
        FlushOutcome::Complete if !status.owners_incomplete => Exit::Return(status.code),
        FlushOutcome::Complete | FlushOutcome::Incomplete { .. } => Exit::Immediately,
    }
}

/// Spends the process's one error grace: calls `await_errors` with [`ERROR_GRACE`] the first time and
/// returns its answer, and does nothing (and does not wait) every time after that. `spent` is the
/// process-wide flag; it is a parameter so that tests use their own.
pub(crate) fn spend_grace(spent: &AtomicBool, await_errors: impl FnOnce(Duration) -> bool) -> bool {
    if spent.swap(true, Ordering::SeqCst) {
        return false;
    }
    await_errors(ERROR_GRACE)
}

/// The real grace: waits on the installed logger, once per process.
fn error_grace() {
    let _ = spend_grace(&GRACE_SPENT, ck_log::await_errors_global);
}

/// Ends the process at once, without running exit handlers or the C library's stdout cleanup.
///
/// `std::process::exit` and C `exit` run both, and with a stalled stdout that cleanup can block again
/// after every deadline has passed. Queued and in-flight log records are lost, and no further
/// diagnostic is attempted: the distinct status is the report.
pub(crate) fn immediate_exit(status: u8) {
    // SAFETY: `_exit` takes only an integer status and has no memory-safety preconditions. It never
    // returns, which is the point: no destructor or exit handler may run after it is called.
    unsafe { libc::_exit(i32::from(status)) }
}

/// Step 7: flushes the logger within the shutdown deadline (recorded in `shutdown` on the first call and
/// reused after that) and returns the status to exit with. If the flush was incomplete, or an owner left
/// the shutdown incomplete (`status.owners_incomplete`), it first spends the error `grace` (which returns
/// at once when no error record is unwritten), then ends the process at once through `exit`; a complete
/// flush with every owner finished in time never uses the grace.
///
/// **The status, in order:** 3 (an incomplete flush or owners), 1 (a failed outcome), 130 or 143 (a signal
/// asked for the quit; `signal` is read *after* the flush, so one that arrived during it counts), 0.
pub(crate) fn conclude(
    status: Status,
    shutdown: &OnceLock<Instant>,
    flush: impl FnOnce(Duration) -> FlushOutcome,
    grace: impl FnOnce(),
    exit: impl FnOnce(u8),
    signal: impl FnOnce() -> Option<u8>,
) -> u8 {
    let deadline = *shutdown.get_or_init(|| Instant::now() + SHUTDOWN_WAIT);
    let outcome = flush(deadline.saturating_duration_since(Instant::now()));
    match decide(&outcome, status) {
        Exit::Return(0) => signal().unwrap_or(0),
        Exit::Return(status) => status,
        Exit::Immediately => {
            grace();
            exit(INCOMPLETE_SHUTDOWN);
            INCOMPLETE_SHUTDOWN
        }
    }
}

/// Step 7 for the real process, as the status number.
pub(crate) fn finish_code(status: Status) -> u8 {
    conclude(
        status,
        &SHUTDOWN_DEADLINE,
        flush_logger,
        error_grace,
        immediate_exit,
        crate::signals::exit_status,
    )
}

/// Step 7 for the real process.
pub(crate) fn finish(status: Status) -> ExitCode {
    ExitCode::from(finish_code(status))
}

/// What the panic hook does for one panic. It emits exactly one panic record through `write_record`, with the control
/// deadline; flushes with the shutdown deadline if one is already active (which may already have
/// expired), otherwise within [`SHUTDOWN_WAIT`]; and, if the flush was incomplete **or the panic leaves
/// resources unproven** (`resources_unproven`, see [`leaves_resources_unproven`]), spends the error `grace`
/// and then ends the process at once through `exit`. It never stops services or cleans up. After a
/// complete flush with nothing unproven it returns, without the grace, and unwinding continues.
pub(crate) fn handle_panic(
    shutdown: Option<Instant>,
    write_record: impl FnOnce(Instant),
    flush: impl FnOnce(Duration) -> FlushOutcome,
    grace: impl FnOnce(),
    exit: impl FnOnce(u8),
    resources_unproven: bool,
) {
    write_record(control_deadline(shutdown));
    let wait = shutdown.map_or(SHUTDOWN_WAIT, |deadline| {
        deadline.saturating_duration_since(Instant::now())
    });
    let clean = Status {
        code: 0,
        owners_incomplete: resources_unproven,
    };
    if decide(&flush(wait), clean) == Exit::Immediately {
        grace();
        exit(INCOMPLETE_SHUTDOWN);
    }
}

fn panic_message(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

/// Installs the panic hook: it replaces Rust's default stderr message with one `runtime` record and
/// decides the exit from the flush result (see [`handle_panic`]).
pub(crate) fn install_panic_hook() {
    panic::set_hook(Box::new(|info| {
        let place = info.location().map_or_else(
            || "<unknown>".to_owned(),
            |at| format!("{}:{}", at.file(), at.line()),
        );
        let record = format!("panic: {} at {place}", panic_message(info));
        handle_panic(
            SHUTDOWN_DEADLINE.get().copied(),
            |deadline| {
                let _ = ck_log::log_within!(Level::Error, deadline, "runtime", "{record}");
            },
            flush_logger,
            error_grace,
            immediate_exit,
            leaves_resources_unproven(RESOURCES.is_set(), std::thread::current().name()),
        );
    }));
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::io::{self, Write};
    use std::sync::{Arc, Condvar, Mutex, PoisonError};
    use std::thread;

    use ck_log::Admission;

    use super::*;

    /// A status whose owners all finished in time.
    fn status(code: u8) -> Status {
        Status {
            code,
            owners_incomplete: false,
        }
    }

    fn incomplete() -> FlushOutcome {
        FlushOutcome::Incomplete {
            queued: 1,
            in_flight: true,
            dropped: 0,
            sink_error: None,
        }
    }

    #[test]
    fn the_decision_returns_the_status_or_ends_the_process() {
        for code in [0, 1] {
            assert_eq!(
                decide(&FlushOutcome::Complete, status(code)),
                Exit::Return(code)
            );
            assert_eq!(decide(&incomplete(), status(code)), Exit::Immediately);
        }
    }

    #[test]
    fn an_owner_that_left_the_shutdown_incomplete_ends_the_process_at_once_even_after_a_complete_flush()
     {
        for code in [0, 1] {
            let owners_missed = Status {
                code,
                owners_incomplete: true,
            };
            assert_eq!(
                decide(&FlushOutcome::Complete, owners_missed),
                Exit::Immediately,
                "a complete flush must not return the status when an owner missed a deadline"
            );
            assert_eq!(decide(&incomplete(), owners_missed), Exit::Immediately);
        }
    }

    #[test]
    fn the_control_deadline_is_the_earlier_of_the_log_wait_and_the_shutdown_deadline() {
        let before = Instant::now();
        let free = control_deadline(None);
        assert!(free >= before + LOG_WAIT && free <= Instant::now() + LOG_WAIT);
        let soon = Instant::now() + Duration::from_millis(10);
        assert_eq!(control_deadline(Some(soon)), soon);
        let later = Instant::now() + Duration::from_secs(60);
        let capped = control_deadline(Some(later));
        assert!(capped <= Instant::now() + LOG_WAIT);
        let past = Instant::now()
            .checked_sub(Duration::from_millis(5))
            .expect("clock");
        assert_eq!(
            control_deadline(Some(past)),
            past,
            "an expired deadline stays expired"
        );
    }

    #[test]
    fn step_7_returns_the_status_after_a_complete_flush_and_does_not_exit_or_wait() {
        for code in [0, 1] {
            let exited = Cell::new(None);
            let graced = Cell::new(0);
            let shutdown = OnceLock::new();
            let given = Cell::new(Duration::ZERO);
            let returned = conclude(
                status(code),
                &shutdown,
                |wait| {
                    given.set(wait);
                    FlushOutcome::Complete
                },
                || graced.set(graced.get() + 1),
                |code| exited.set(Some(code)),
                || None,
            );
            assert_eq!(returned, code);
            assert_eq!(
                exited.get(),
                None,
                "a complete flush does not end the process"
            );
            assert_eq!(graced.get(), 0, "a complete flush never uses the grace");
            assert!(
                given.get() <= SHUTDOWN_WAIT
                    && given.get() > SHUTDOWN_WAIT - Duration::from_millis(500)
            );
            assert!(
                shutdown.get().is_some(),
                "the shutdown deadline is recorded"
            );
        }
    }

    #[test]
    fn step_7_spends_the_grace_once_and_then_ends_the_process_with_status_3_after_an_incomplete_flush()
     {
        let events = RefCell::new(Vec::new());
        let returned = conclude(
            status(0),
            &OnceLock::new(),
            |_| incomplete(),
            || events.borrow_mut().push("grace".to_owned()),
            |code| events.borrow_mut().push(format!("exit {code}")),
            || None,
        );
        assert_eq!(
            *events.borrow(),
            ["grace", "exit 3"],
            "the grace, then the exit"
        );
        assert_eq!(returned, 3);
    }

    #[test]
    fn step_7_ends_the_process_at_once_with_status_3_for_an_owner_miss_even_after_a_complete_flush()
    {
        for code in [0, 1] {
            let events = RefCell::new(Vec::new());
            let returned = conclude(
                Status {
                    code,
                    owners_incomplete: true,
                },
                &OnceLock::new(),
                |_| FlushOutcome::Complete,
                || events.borrow_mut().push("grace".to_owned()),
                |exit_code| events.borrow_mut().push(format!("exit {exit_code}")),
                || None,
            );
            assert_eq!(
                *events.borrow(),
                ["grace", "exit 3"],
                "the error grace, then the immediate exit, although the flush was complete"
            );
            assert_eq!(returned, INCOMPLETE_SHUTDOWN);
        }
    }

    #[test]
    fn step_7_reuses_a_deadline_that_is_already_recorded() {
        let recorded = Instant::now() + Duration::from_millis(50);
        let shutdown = OnceLock::new();
        shutdown.set(recorded).expect("empty cell");
        let given = Cell::new(Duration::MAX);
        conclude(
            status(0),
            &shutdown,
            |wait| {
                given.set(wait);
                FlushOutcome::Complete
            },
            || {},
            |_| {},
            || None,
        );
        assert!(
            given.get() <= Duration::from_millis(50),
            "{:?}",
            given.get()
        );
        assert_eq!(shutdown.get(), Some(&recorded));
    }

    #[test]
    fn the_hook_returns_after_a_complete_flush_having_emitted_one_record() {
        let emitted = Cell::new(0);
        let graced = Cell::new(0);
        let exits = RefCell::new(Vec::new());
        handle_panic(
            None,
            |_| emitted.set(emitted.get() + 1),
            |_| FlushOutcome::Complete,
            || graced.set(graced.get() + 1),
            |code| exits.borrow_mut().push(code),
            false,
        );
        assert_eq!(emitted.get(), 1, "exactly one panic record");
        assert!(exits.borrow().is_empty(), "unwinding continues: no exit");
        assert_eq!(graced.get(), 0, "a complete flush never uses the grace");
    }

    #[test]
    fn the_hook_spends_the_grace_and_ends_the_process_once_with_status_3_after_an_incomplete_flush()
    {
        let emitted = Cell::new(0);
        let events = RefCell::new(Vec::new());
        handle_panic(
            None,
            |_| emitted.set(emitted.get() + 1),
            |_| incomplete(),
            || events.borrow_mut().push("grace".to_owned()),
            |code| events.borrow_mut().push(format!("exit {code}")),
            false,
        );
        assert_eq!(*events.borrow(), ["grace", "exit 3"]);
        assert_eq!(emitted.get(), 1, "nothing further is written");
    }

    #[test]
    fn a_panic_after_the_shutdown_deadline_expired_still_uses_the_grace() {
        let expired = Instant::now()
            .checked_sub(Duration::from_millis(5))
            .expect("clock");
        let events = RefCell::new(Vec::new());
        let given = Cell::new(Duration::MAX);
        handle_panic(
            Some(expired),
            |_| {},
            |wait| {
                given.set(wait);
                incomplete()
            },
            || events.borrow_mut().push("grace".to_owned()),
            |code| events.borrow_mut().push(format!("exit {code}")),
            false,
        );
        assert_eq!(given.get(), Duration::ZERO, "the flush had no time left");
        assert_eq!(*events.borrow(), ["grace", "exit 3"]);
    }

    #[test]
    fn the_hook_gives_the_flush_the_two_second_deadline_unless_shutdown_is_under_way() {
        let given = Cell::new(Duration::ZERO);
        handle_panic(
            None,
            |_| {},
            |wait| {
                given.set(wait);
                FlushOutcome::Complete
            },
            || {},
            |_| {},
            false,
        );
        assert_eq!(given.get(), SHUTDOWN_WAIT);

        let deadline = Instant::now() + Duration::from_millis(50);
        let asked = Cell::new(None);
        handle_panic(
            Some(deadline),
            |at| asked.set(Some(at)),
            |wait| {
                given.set(wait);
                FlushOutcome::Complete
            },
            || {},
            |_| {},
            false,
        );
        assert!(
            given.get() <= Duration::from_millis(50),
            "reuses the active deadline, not 2 s"
        );
        assert!(
            asked.get().expect("the record was emitted") <= deadline,
            "the record's deadline is capped by the active shutdown deadline"
        );
    }

    #[test]
    fn a_panic_that_leaves_resources_unproven_ends_the_process_with_status_3_after_a_complete_flush()
     {
        let emitted = Cell::new(0);
        let events = RefCell::new(Vec::new());
        handle_panic(
            None,
            |_| emitted.set(emitted.get() + 1),
            |_| FlushOutcome::Complete,
            || events.borrow_mut().push("grace".to_owned()),
            |code| events.borrow_mut().push(format!("exit {code}")),
            true,
        );
        assert_eq!(
            *events.borrow(),
            ["grace", "exit 3"],
            "no return into an unwind past unproven resources"
        );
        assert_eq!(emitted.get(), 1, "one record");
    }

    #[test]
    fn the_latch_rule_covers_every_thread_except_the_vm_owners() {
        // Unset: nothing is unproven, whoever panics.
        for name in [
            None,
            Some("main"),
            Some("ck-lua-ui"),
            Some("ck-metal-render"),
        ] {
            assert!(!leaves_resources_unproven(false, name), "{name:?}");
        }
        // Set: every thread but the `ck-lua-<name>` owners, an unnamed thread and the presenter's included
        // (no presenter thread name is relied on).
        for name in [
            None,
            Some("main"),
            Some("ck-metal-render"),
            Some("ck-signals"),
            Some("ck-log-writer"),
            Some("ck-lua"),
            Some("my-ck-lua-ui"),
        ] {
            assert!(leaves_resources_unproven(true, name), "{name:?}");
        }
        for name in ["ck-lua-ui", "ck-lua-gameplay", "ck-lua-x"] {
            assert!(!leaves_resources_unproven(true, Some(name)), "{name}");
        }
    }

    #[test]
    fn the_latch_is_set_before_and_cleared_only_by_the_proven_teardown() {
        let latch = ResourceLatch::new();
        assert!(!latch.is_set());
        latch.set();
        assert!(latch.is_set());
        latch.set();
        latch.clear();
        assert!(!latch.is_set());
    }

    #[test]
    fn the_grace_waits_two_seconds_once_per_process_and_not_again() {
        let spent = AtomicBool::new(false);
        let asked = RefCell::new(Vec::new());
        assert!(spend_grace(&spent, |wait| {
            asked.borrow_mut().push(wait);
            true
        }));
        assert!(
            !spend_grace(&spent, |wait| {
                asked.borrow_mut().push(wait);
                true
            }),
            "a second request does not wait"
        );
        assert_eq!(*asked.borrow(), [ERROR_GRACE], "once, with the two seconds");
        assert_eq!(ERROR_GRACE, Duration::from_secs(2));
    }

    /// What the held sink shares with the test: whether it is open, and what was written.
    #[derive(Default)]
    struct HeldState {
        open: bool,
        written: Vec<u8>,
    }

    /// A sink that holds every write until it is opened, and keeps what was written.
    #[derive(Clone, Default)]
    struct HeldSink(Arc<(Mutex<HeldState>, Condvar)>);

    impl HeldSink {
        fn open(&self) {
            let (state, changed) = &*self.0;
            state.lock().unwrap_or_else(PoisonError::into_inner).open = true;
            changed.notify_all();
        }

        fn text(&self) -> String {
            let (state, _) = &*self.0;
            let state = state.lock().unwrap_or_else(PoisonError::into_inner);
            String::from_utf8(state.written.clone()).expect("utf-8")
        }
    }

    impl Write for HeldSink {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let (state, changed) = &*self.0;
            let give_up = Instant::now() + Duration::from_secs(30);
            let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
            while !state.open && Instant::now() < give_up {
                state = changed
                    .wait_timeout(state, Duration::from_millis(20))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            state.written.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Opens the sink when dropped, so that a failing test never leaves the writer blocked.
    struct OpenOnDrop(HeldSink);

    impl Drop for OpenOnDrop {
        fn drop(&mut self) {
            self.0.open();
        }
    }

    /// A logger whose writer is blocked in the write of one progress record.
    fn held_logger() -> (Arc<Logger>, HeldSink, OpenOnDrop) {
        let sink = HeldSink::default();
        let logger = Logger::new(Box::new(sink.clone()), fixed_settings()).expect("logger");
        let guard = OpenOnDrop(sink.clone());
        let _ = logger.log_within(
            Level::Info,
            "test",
            "progress",
            Instant::now() + Duration::from_secs(5),
        );
        let give_up = Instant::now() + Duration::from_secs(5);
        while logger.flush(Duration::ZERO)
            == (FlushOutcome::Incomplete {
                queued: 1,
                in_flight: false,
                dropped: 0,
                sink_error: None,
            })
        {
            assert!(Instant::now() < give_up, "the writer never took the record");
            thread::yield_now();
        }
        (Arc::new(logger), sink, guard)
    }

    #[test]
    fn an_ordinary_queue_error_is_written_inside_the_grace_before_the_exit() {
        let (logger, sink, _open) = held_logger();
        let expired = Instant::now()
            .checked_sub(Duration::from_millis(5))
            .expect("clock");
        // Admitted to the ordinary queue (space and the lock are free), behind the held write.
        assert_eq!(
            logger.log_within(Level::Error, "runtime", "start failed", expired),
            Admission::Queued
        );
        let spent = AtomicBool::new(false);
        let output_at_exit = RefCell::new(None);
        let opener = {
            let sink = sink.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                sink.open();
            })
        };
        let returned = conclude(
            status(1),
            &OnceLock::new(),
            |_| logger.flush(Duration::ZERO),
            || {
                assert!(spend_grace(&spent, |wait| logger.await_errors(wait)));
            },
            |code| *output_at_exit.borrow_mut() = Some((code, sink.text())),
            || None,
        );
        opener.join().expect("the opener ends");
        assert_eq!(returned, INCOMPLETE_SHUTDOWN, "still status 3");
        let (code, text) = output_at_exit.into_inner().expect("the exit was called");
        assert_eq!(code, 3);
        assert!(
            text.contains("ERROR runtime: start failed\n"),
            "the error was written before the exit: {text:?}"
        );
    }

    #[test]
    fn with_only_progress_pending_the_grace_returns_at_once_and_the_exit_is_still_status_3() {
        let (logger, _sink, _open) = held_logger();
        let _ = logger.log_within(
            Level::Info,
            "test",
            "more progress",
            Instant::now() + Duration::from_secs(5),
        );
        let spent = AtomicBool::new(false);
        let events = RefCell::new(Vec::new());
        let begun = Instant::now();
        conclude(
            status(0),
            &OnceLock::new(),
            |_| logger.flush(Duration::ZERO),
            || {
                assert!(spend_grace(&spent, |wait| logger.await_errors(wait)));
                events.borrow_mut().push("grace".to_owned());
            },
            |code| events.borrow_mut().push(format!("exit {code}")),
            || None,
        );
        assert_eq!(*events.borrow(), ["grace", "exit 3"]);
        assert!(
            begun.elapsed() < Duration::from_millis(500),
            "no pause when no error is pending: {:?}",
            begun.elapsed()
        );
    }

    /// What `conclude` returns, and whether it ended the process, for a status, a flush and a signal.
    fn conclude_with(status: Status, flush: FlushOutcome, signal: Option<u8>) -> (u8, Option<u8>) {
        let exited = Cell::new(None);
        let returned = conclude(
            status,
            &OnceLock::new(),
            |_| flush,
            || {},
            |code| exited.set(Some(code)),
            || signal,
        );
        (returned, exited.get())
    }

    #[test]
    fn the_exit_status_is_3_then_1_then_the_signals_then_0_for_every_combination() {
        for signal in [None, Some(130), Some(143)] {
            for code in [0, 1] {
                for owners_incomplete in [false, true] {
                    for complete in [true, false] {
                        let flush = if complete {
                            FlushOutcome::Complete
                        } else {
                            incomplete()
                        };
                        let (returned, exited) = conclude_with(
                            Status {
                                code,
                                owners_incomplete,
                            },
                            flush,
                            signal,
                        );
                        let context = format!(
                            "code {code}, owners_incomplete {owners_incomplete}, complete {complete}, signal {signal:?}"
                        );
                        if owners_incomplete || !complete {
                            // 3 wins over everything: an incomplete shutdown ends the process at once.
                            assert_eq!((returned, exited), (3, Some(3)), "{context}");
                        } else if code == 1 {
                            // 1 wins over a signal.
                            assert_eq!((returned, exited), (1, None), "{context}");
                        } else {
                            assert_eq!(
                                (returned, exited),
                                (signal.unwrap_or(0), None),
                                "{context}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_signal_that_arrives_during_the_final_flush_still_counts() {
        // The signal is read after the flush: here it arrives inside it.
        let arrived = Cell::new(false);
        let returned = conclude(
            status(0),
            &OnceLock::new(),
            |_| {
                arrived.set(true);
                FlushOutcome::Complete
            },
            || {},
            |_| {},
            || arrived.get().then_some(130),
        );
        assert_eq!(returned, 130);
    }

    #[test]
    fn the_signal_is_not_even_read_when_the_status_is_already_decided() {
        let read = Cell::new(false);
        let returned = conclude(
            status(1),
            &OnceLock::new(),
            |_| FlushOutcome::Complete,
            || {},
            |_| {},
            || {
                read.set(true);
                Some(143)
            },
        );
        assert_eq!(returned, 1);
        assert!(!read.get());
    }
}
