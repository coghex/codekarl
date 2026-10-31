//! A small, reusable start-up and shutdown lifecycle.
//!
//! A [`Runtime`] owns an ordered list of [`Service`]s. [`Runtime::run`] starts them in order, then
//! stops the ones that started in reverse order, and reports what happened as an [`Outcome`]. If a
//! start fails, only the services already started are stopped (also in reverse order) before the
//! failed outcome is returned. Every path ends with exactly one terminal log record.
//!
//! The crate holds no game concepts: no configuration format, no Lua, no Metal. It never exits,
//! aborts or installs signal or panic handlers, and it does not catch panics from services: a panic
//! propagates to the caller, who chooses the exit status from the [`Outcome`]. It writes nothing to
//! stdout or stderr itself; every record goes through [`ck_log`] in category `runtime`, so it
//! appears only if the caller installed a logger.
//!
//! # Log records
//!
//! In order, as they happen (`<name>` is [`Service::name`]):
//!
//! - `info  runtime: started <name>` after each successful start;
//! - `error runtime: <name> failed to start: <error>` for the service whose start failed, after which
//!   nothing further is started;
//! - `info  runtime: start-up complete` once every service has started;
//! - `info  runtime: stopped <name>` after each successful stop, or
//!   `error runtime: <name> failed to stop: <error>` for a failing stop, which does not prevent the
//!   remaining services from stopping;
//! - then, once per run on every path, `info  runtime: shutdown complete` if no stop failed, else
//!   `error runtime: shutdown finished with <n> stop failure(s)`.
//!
//! # Logging on the control path
//!
//! The runtime owns the start-up and shutdown sequence, so it follows `ck_log`'s control-path rule:
//! **every** record, progress and failure records included, goes through `ck_log::log_within`, never
//! through the waiting `info!`/`error!` macros. A full log queue behind a stalled output therefore
//! cannot stall the lifecycle.
//!
//! - Each record is given a **log wait**, a deadline of *now plus the wait* taken when that record is
//!   emitted. [`Runtime::new`] uses 100 ms; [`Runtime::with_log_wait`] sets another value, for
//!   instance to match the caller's own deadlines.
//! - The wait is **per record**. It is not a bound on the whole run: a run emits a handful of records
//!   plus whatever the services themselves take, so its worst case when the logger cannot admit
//!   anything is that many records times the wait.
//! - A record the logger cannot admit in time is dropped and counted by `ck_log`; it never changes the
//!   [`Outcome`]. The loss shows only in the logger's flush result (`FlushOutcome::Incomplete`), which
//!   the caller already turns into an incomplete shutdown.
//! - The runtime emits each record exactly once either way.
//!
//! # Owners
//!
//! Besides services, a runtime supervises **owners**: things that do their work on a thread of their own,
//! such as a Lua VM. An owner is anything that implements [`Owner`]; the runtime knows nothing about
//! what is behind it. Each owner is supplied as an [`OwnerSpawn`] (a name and a closure that returns the
//! tracked [`Owner`]), added with [`Runtime::with_owners`], and tracked from the moment it is spawned.
//!
//! **The cooperative contract.** The runtime relies on it and cannot enforce it against an owner that
//! breaks it:
//!
//! - *Never block:* the spawn closure returns a tracked handle without waiting for readiness, and
//!   [`Owner::name`], [`Owner::request_start`], [`Owner::request_stop`], [`Owner::cancel`] and dropping an
//!   owner never block.
//! - *Return within the budget:* [`Owner::wait_ready`], [`Owner::await_started`],
//!   [`Owner::await_stopped`] and [`Owner::wait_finished`] return within their `within` budget, plus a
//!   small scheduling bound, whatever the owner's thread is doing.
//! - *A zero budget makes exactly one non-waiting observation*, so an owner that has already finished, or
//!   already replied, is seen as such.
//! - *Polls never abandon:* [`Owner::poll_ready`] and [`Owner::poll_started`] return within their
//!   `within` budget plus the scheduling bound. Running out of the budget is [`Progress::Pending`]: nothing
//!   about the owner changed (no cancel, no abandonment) and the request is still in flight. A zero budget
//!   makes one non-waiting observation. A failure is [`OwnerError::Failed`]; they never return
//!   [`OwnerError::Deadline`].
//! - *Completion is observed, never assumed:* `await_stopped` returns `Ok` only after the owner's thread
//!   has been observed finished (a reply alone is not enough), and `wait_finished` returns `true` only
//!   once the thread is observed finished.
//!
//! **Phases.** After the services have started (in order, as above):
//!
//! 1. *start phase*, one absolute deadline (now plus the start deadline, 5 s by default, formed just
//!    before the first spawn, so time spent starting services is never charged to the owners): spawn
//!    every owner in order; wait for readiness of each (the owners initialise concurrently, so waiting
//!    on them in turn gives each the whole phase); `request_start` to **every** owner; then wait for the
//!    start of each against the same deadline. `started <name>` is logged after each success. Each wait
//!    is a series of polls of at most one **slice** ([`Runtime::with_poll_slice`], 10 ms by default),
//!    each with `within` = the smaller of the slice and the time left, so a quit request is noticed
//!    between slices; only when the deadline itself passes does the supervisor call the existing
//!    `wait_ready` or `await_started` once, with a zero budget (a result that arrived at the last moment
//!    is used normally, otherwise the owner missed its start deadline);
//! 2. a final quit check, then `start-up complete`; then the *run phase* (below);
//! 3. *stop phase*, one absolute deadline (now plus the stop deadline, 5 s by default):
//!    `request_stop` to **every** owner in reverse order, then `await_stopped` each in reverse order
//!    against the deadline. Every owner has its request before any wait begins, so a stuck owner does not
//!    delay a sibling's shutdown, and the sibling is observed even with no time left. `stopped <name>` is
//!    logged only after the owner was observed finished;
//! 4. the services stop in reverse order, as above.
//!
//! **The quit request.** [`Runtime::with_quit`] gives the runtime a flag that another thread may set at
//! any time; the runtime only reads it. It is checked before each service start, before each spawn,
//! before each `request_start`, before each owner's first poll of a fresh wait (readiness and start,
//! also after a `started <name>` record), between poll slices and once more immediately before
//! `start-up complete` (after the last service start and, with owners, after the last successful poll).
//! A result already returned by an issued poll is processed first: a quit set while one owner completed
//! is seen before the next owner is polled, and after a `Pending` poll the deadline still comes first. A flag seen set
//! takes the **quit path**: one `info runtime: quit requested during start-up` record (with the log wait
//! if no owner phase has begun, else capped by the start deadline like every owner-phase record); every
//! spawned owner is cancelled and observed against the cleanup deadline, exactly as on a start failure
//! (owners not observed finished are in `unfinished`); no stop request is sent; the started services stop
//! in reverse; `start-up complete` is not logged and the run phase is skipped. The outcome is
//! [`Outcome::Clean`] unless something failed, with [`Report::quit`] set. When a quit coincides with
//! something else the order is fixed: **a returned failure wins** (a start failure, even if the flag is
//! also set); **the deadline is checked before the flag** (after a `Pending` poll the zero-budget final
//! observation runs first); **`Report::quit` is sampled at the end** of the start phase for any reason,
//! and never changes the outcome; and **only returned failures count**: the quit path does not poll or
//! sweep the cancelled siblings, it only observes them finishing.
//!
//! **The run phase.** [`Runtime::with_run`] supplies a closure, called on the calling thread after
//! `start-up complete` and before the stop phase (never after a failed or cut-short start-up). It gets a
//! [`RunContext`] whose [`quit_requested`](RunContext::quit_requested) reads the flag, and ends as a
//! [`RunEnd`]: `Finished`, `Quit` (sets [`Report::quit`]) or `Failed(message)`, which logs one
//! `error runtime: run failed: <message>` record with the whole message and is reported as
//! [`Outcome::RunFailed`]. The stop phase and the service stops then run as usual. The runtime puts no
//! deadline on the closure and does not catch its panics. **Precedence:** `StartFailed`, then
//! `RunFailed`, then `StopFailed`, then `Clean`; `quit` is reported alongside any of them.
//!
//! **Start failure ("stop both, exit 1").** If a spawn, readiness, start request or start wait fails,
//! including with [`OwnerError::Deadline`], the failure is logged and **every** spawned owner is
//! cancelled, whatever its stage; no stop request is sent (so no script `shutdown()` runs). A separate
//! **cleanup deadline** (now plus the cleanup deadline, 1 s by default, an explicit extension beyond the
//! start phase used only on this path) bounds the wait for each owner to finish; then the started
//! services are stopped in reverse and the outcome is [`Outcome::StartFailed`] naming the owner.
//!
//! **Stop results.** A failed `request_stop` is logged and counted as a stop failure, the remaining
//! requests are still sent, and that owner is cancelled and given no await; it is then observed like
//! the others against the stop deadline (a `Deadline` from `request_stop` is logged as a missed
//! deadline instead of a failure). A [`OwnerError::Failed`] await is logged and counted as a stop
//! failure; a [`OwnerError::Deadline`] await is logged as a missed deadline and the owner is cancelled.
//! After the awaits, every owner not yet observed finished is waited for once more with the time left
//! in the phase (or the zero budget).
//!
//! **Missed and unfinished.** [`Runtime::run_report`] returns a [`Report`]. `missed` names, in order of
//! occurrence and without duplicates, every owner that missed a start or stop deadline, whether or not
//! it was later observed finished. `unfinished` names, in order, every owner not observed finished,
//! and each gets an `error runtime: <name> did not finish` record. **With owners, [`Outcome::Clean`] can
//! come with non-empty `missed` or `unfinished` lists, so callers that use owners must use
//! `run_report`;** [`Runtime::run`] returns only the outcome.
//!
//! **Records in owner phases** are capped by the phase's deadline (start, stop or cleanup): a record's
//! deadline is the earlier of now plus the log wait and the phase deadline, and once the phase deadline
//! has passed a record makes one non-waiting attempt. A record never extends a phase. Service records
//! and the terminal record keep their own log waits.
//!
//! # Example
//!
//! ```
//! use ck_runtime::{Outcome, Runtime};
//!
//! // With no services there is nothing to start or stop.
//! assert_eq!(Runtime::new(Vec::new()).run(), Outcome::Clean);
//! ```

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use ck_log::Level;

mod owners;

/// The default log wait of a [`Runtime`], per record.
const DEFAULT_LOG_WAIT: Duration = Duration::from_millis(100);
/// The default start and stop phase deadlines of the owners.
const DEFAULT_PHASE: Duration = Duration::from_secs(5);
/// The default cleanup deadline after an owner start failure.
const DEFAULT_CLEANUP: Duration = Duration::from_secs(1);
/// The default slice of the start phase's polls.
const DEFAULT_SLICE: Duration = Duration::from_millis(10);
/// What an unrepresentably long log wait is capped to, so that a deadline can always be formed.
const LONGEST_LOG_WAIT: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// The deadline for one record or phase: now plus `wait`.
pub(crate) fn deadline(wait: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(wait)
        .or_else(|| now.checked_add(LONGEST_LOG_WAIT))
        .unwrap_or(now)
}

/// Emits one lifecycle record through `ck_log::log_within`, given `wait` to be admitted. A record that
/// is not admitted in time is dropped and counted by the logger, and is deliberately not reported here:
/// logging never changes the outcome of a run.
macro_rules! record {
    ($wait:expr, $level:expr, $($arg:tt)+) => {
        let _ = ck_log::log_within!($level, deadline($wait), "runtime", $($arg)+);
    };
}

/// A failure reported by a [`Service`]: a message for the log and the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceError {
    message: String,
}

impl ServiceError {
    /// An error carrying `message`.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ServiceError {}

/// Something with a start-up and a shutdown step, run by a [`Runtime`].
pub trait Service {
    /// The name used in log records and failures.
    fn name(&self) -> &str;

    /// Starts the service.
    ///
    /// # Errors
    ///
    /// Returns a [`ServiceError`] when the service cannot start; the runtime then stops the services
    /// that did start, in reverse order.
    fn start(&mut self) -> Result<(), ServiceError>;

    /// Stops the service. Called only for a service whose `start` succeeded.
    ///
    /// # Errors
    ///
    /// Returns a [`ServiceError`] when the service cannot stop cleanly; the runtime logs it and still
    /// stops the remaining services.
    fn stop(&mut self) -> Result<(), ServiceError>;
}

/// A service and the error it reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The name of the service.
    pub service: String,
    /// What it reported.
    pub error: ServiceError,
}

/// How a [`Runtime::run`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Everything started and everything stopped.
    Clean,
    /// Everything started, and at least one stop failed.
    StopFailed {
        /// Every failing stop, in the order the stops ran (reverse start order).
        failures: Vec<Failure>,
    },
    /// A start failed. This is the primary result whenever a start fails, whatever happened while
    /// the started services were stopped.
    StartFailed {
        /// The name of the service that failed to start.
        service: String,
        /// What it reported.
        error: ServiceError,
        /// Every failing stop during the rollback, in order; possibly none.
        stop_failures: Vec<Failure>,
    },
    /// Everything started, and the run closure (see [`Runtime::with_run`]) returned
    /// [`RunEnd::Failed`]. Reported before any [`Outcome::StopFailed`].
    RunFailed {
        /// The run's failure message.
        error: ServiceError,
        /// Every failing stop that followed, in the order the stops ran.
        stop_failures: Vec<Failure>,
    },
}

/// How the run closure of [`Runtime::with_run`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    /// The run ended on its own (for example, its window was closed).
    Finished,
    /// The run ended because it saw the quit request.
    Quit,
    /// The run failed. The runtime logs one `error runtime: run failed: <message>` record with the
    /// whole message.
    Failed(String),
}

/// What the run closure is given.
#[derive(Debug)]
pub struct RunContext {
    quit: Option<Arc<AtomicBool>>,
}

impl RunContext {
    /// Whether the quit flag (see [`Runtime::with_quit`]) is set; false without one.
    #[must_use]
    pub fn quit_requested(&self) -> bool {
        self.quit
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }
}

/// The answer of a non-abandoning poll of an [`Owner`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Not yet. Nothing about the owner changed and the request is still in flight.
    Pending,
    /// The awaited result arrived and was consumed.
    Done,
}

/// Why a call on an [`Owner`] did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerError {
    /// The owner reported a failure. The message may span several lines, for example with a traceback.
    Failed {
        /// What the owner reported.
        message: String,
    },
    /// The owner did not answer within the budget it was given.
    Deadline,
}

impl fmt::Display for OwnerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed { message } => formatter.write_str(message),
            Self::Deadline => formatter.write_str("deadline missed"),
        }
    }
}

impl Error for OwnerError {}

/// Something that does its work on a thread of its own and is supervised by a [`Runtime`]. Every
/// request returns at once and every wait takes a budget. See the crate documentation for the
/// cooperative contract an implementation must keep.
pub trait Owner {
    /// The name used in log records and failures.
    fn name(&self) -> &str;

    /// Waits at most `within` for the owner to finish initialising.
    ///
    /// # Errors
    ///
    /// [`OwnerError::Failed`] when initialisation failed, [`OwnerError::Deadline`] when it did not
    /// finish within the budget.
    fn wait_ready(&mut self, within: Duration) -> Result<(), OwnerError>;

    /// Like [`wait_ready`](Self::wait_ready), but running out of `within` is [`Progress::Pending`]: it
    /// changes nothing about the owner (no cancel, no abandonment) and the request stays in flight, so
    /// it can be called again in slices. Returns within `within` plus the usual scheduling bound; a
    /// zero budget makes one non-waiting observation.
    ///
    /// # Errors
    ///
    /// [`OwnerError::Failed`] when initialisation failed. Never [`OwnerError::Deadline`].
    fn poll_ready(&mut self, within: Duration) -> Result<Progress, OwnerError>;

    /// Asks the owner to start; returns at once.
    ///
    /// # Errors
    ///
    /// [`OwnerError::Failed`] when the request cannot be made.
    fn request_start(&mut self) -> Result<(), OwnerError>;

    /// Waits at most `within` for the start requested by [`request_start`](Self::request_start).
    ///
    /// # Errors
    ///
    /// [`OwnerError::Failed`] when starting failed, [`OwnerError::Deadline`] when it did not finish
    /// within the budget.
    fn await_started(&mut self, within: Duration) -> Result<(), OwnerError>;

    /// Like [`await_started`](Self::await_started), but running out of `within` is
    /// [`Progress::Pending`], with the same contract as [`poll_ready`](Self::poll_ready).
    ///
    /// # Errors
    ///
    /// [`OwnerError::Failed`] when starting failed. Never [`OwnerError::Deadline`].
    fn poll_started(&mut self, within: Duration) -> Result<Progress, OwnerError>;

    /// Asks the owner to stop; returns at once.
    ///
    /// # Errors
    ///
    /// [`OwnerError::Failed`] when the request cannot be made, [`OwnerError::Deadline`] when the owner
    /// cannot take it in time.
    fn request_stop(&mut self) -> Result<(), OwnerError>;

    /// Waits at most `within` for the stop requested by [`request_stop`](Self::request_stop), and for
    /// the owner's thread to be observed finished: `Ok` only after that.
    ///
    /// # Errors
    ///
    /// [`OwnerError::Failed`] when stopping failed, [`OwnerError::Deadline`] when it did not finish
    /// within the budget.
    fn await_stopped(&mut self, within: Duration) -> Result<(), OwnerError>;

    /// Cancels the owner: it ends as soon as it can. Never blocks.
    fn cancel(&mut self);

    /// Waits at most `within` for the owner's thread to be observed finished; `true` once it was.
    fn wait_finished(&mut self, within: Duration) -> bool;
}

/// How an [`Owner`] is created: a name and a closure that returns the tracked owner **without waiting
/// for it to become ready**, or the reason it could not be created.
pub struct OwnerSpawn {
    name: String,
    spawn: Box<dyn FnOnce() -> Result<Box<dyn Owner>, String>>,
}

impl OwnerSpawn {
    /// An owner called `name`, created by `spawn` when the runtime reaches its start phase.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        spawn: impl FnOnce() -> Result<Box<dyn Owner>, String> + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            spawn: Box::new(spawn),
        }
    }
}

/// What [`Runtime::run_report`] returns: the [`Outcome`] and what happened to the owners.
///
/// With owners, `Outcome::Clean` can come with non-empty `missed` or `unfinished` lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// How the run ended.
    pub outcome: Outcome,
    /// Every owner that missed a start or stop deadline, in order of occurrence and without
    /// duplicates, whether or not it was later observed finished.
    pub missed: Vec<String>,
    /// Every owner not observed finished, in order.
    pub unfinished: Vec<String>,
    /// The quit flag was seen set during start-up, or the run closure returned [`RunEnd::Quit`]. With
    /// `quit` set, [`Outcome::Clean`] means nothing failed, although start-up may have been cut short.
    /// It never changes the outcome.
    pub quit: bool,
}

/// The run closure of [`Runtime::with_run`].
type RunClosure = Box<dyn FnOnce(&RunContext) -> RunEnd>;

/// An ordered list of services (and owners) and the lifecycle that runs them.
pub struct Runtime {
    services: Vec<Box<dyn Service>>,
    owners: Vec<OwnerSpawn>,
    log_wait: Duration,
    start_deadline: Duration,
    stop_deadline: Duration,
    cleanup_deadline: Duration,
    quit: Option<Arc<AtomicBool>>,
    poll_slice: Duration,
    run: Option<RunClosure>,
}

impl Runtime {
    /// A runtime that will start `services` in the given order, giving each log record 100 ms to be
    /// admitted (see [`with_log_wait`](Self::with_log_wait)).
    #[must_use]
    pub fn new(services: Vec<Box<dyn Service>>) -> Self {
        Self {
            services,
            owners: Vec::new(),
            log_wait: DEFAULT_LOG_WAIT,
            start_deadline: DEFAULT_PHASE,
            stop_deadline: DEFAULT_PHASE,
            cleanup_deadline: DEFAULT_CLEANUP,
            quit: None,
            poll_slice: DEFAULT_SLICE,
            run: None,
        }
    }

    /// The same runtime with a shared quit flag, which another thread may set at any time. The runtime
    /// only ever reads it. See the crate documentation for where it is checked.
    #[must_use]
    pub fn with_quit(mut self, flag: Arc<AtomicBool>) -> Self {
        self.quit = Some(flag);
        self
    }

    /// The same runtime with `slice` as the slice of the start phase's polls (10 ms by default; a zero
    /// slice is treated as the default).
    #[must_use]
    pub fn with_poll_slice(mut self, slice: Duration) -> Self {
        self.poll_slice = if slice.is_zero() {
            DEFAULT_SLICE
        } else {
            slice
        };
        self
    }

    /// The same runtime with a run closure: called on the calling thread after `start-up complete` and
    /// before the stop phase, and not called when start-up failed or was cut short by a quit. The
    /// runtime puts no deadline on it, and does not catch its panics.
    #[must_use]
    pub fn with_run(mut self, run: impl FnOnce(&RunContext) -> RunEnd + 'static) -> Self {
        self.run = Some(Box::new(run));
        self
    }

    /// The same runtime with `owners` added: spawned in order after the services have started, started
    /// together and stopped together, in reverse order, before the services stop.
    #[must_use]
    pub fn with_owners(mut self, owners: Vec<OwnerSpawn>) -> Self {
        self.owners = owners;
        self
    }

    /// The same runtime with the owner deadlines set: the start phase and the stop phase (5 s each by
    /// default) and the cleanup that follows a start failure (1 s by default).
    #[must_use]
    pub fn with_owner_deadlines(
        mut self,
        start: Duration,
        stop: Duration,
        cleanup: Duration,
    ) -> Self {
        self.start_deadline = start;
        self.stop_deadline = stop;
        self.cleanup_deadline = cleanup;
        self
    }

    /// The same runtime with `wait` as the log wait: how long each record may wait to be admitted to
    /// the logger before it is dropped and counted. It is per record, not a bound on the whole run.
    #[must_use]
    pub fn with_log_wait(mut self, wait: Duration) -> Self {
        self.log_wait = wait;
        self
    }

    /// The log wait given to each record.
    #[must_use]
    pub const fn log_wait(&self) -> Duration {
        self.log_wait
    }

    /// Starts every service in order, then stops the started ones in reverse order.
    ///
    /// See the [crate documentation](crate) for the records written and the outcomes. A panic from a
    /// service is not caught. With owners, [`Outcome::Clean`] can come with owners that missed a
    /// deadline or were never observed finished: use [`run_report`](Self::run_report).
    #[must_use]
    pub fn run(self) -> Outcome {
        self.run_report().outcome
    }

    /// Starts the services in order, checking the quit flag before each. Returns how many started, the
    /// failure of the one that did not, and whether the quit flag cut start-up short.
    fn start_services(&mut self, is_quit: &dyn Fn() -> bool) -> (usize, Option<Failure>, bool) {
        let wait = self.log_wait;
        let mut started = 0;
        for service in &mut self.services {
            if is_quit() {
                return (started, None, true);
            }
            match service.start() {
                Ok(()) => {
                    record!(wait, Level::Info, "started {}", service.name());
                    started += 1;
                }
                Err(error) => {
                    record!(
                        wait,
                        Level::Error,
                        "{} failed to start: {error}",
                        service.name()
                    );
                    let failure = Failure {
                        service: service.name().to_owned(),
                        error,
                    };
                    return (started, Some(failure), false);
                }
            }
        }
        (started, None, false)
    }

    /// Stops the first `started` services in reverse order, adding each failing stop to `failures`.
    fn stop_services(&mut self, started: usize, failures: &mut Vec<Failure>) {
        let wait = self.log_wait;
        for service in self.services[..started].iter_mut().rev() {
            match service.stop() {
                Ok(()) => {
                    record!(wait, Level::Info, "stopped {}", service.name());
                }
                Err(error) => {
                    record!(
                        wait,
                        Level::Error,
                        "{} failed to stop: {error}",
                        service.name()
                    );
                    failures.push(Failure {
                        service: service.name().to_owned(),
                        error,
                    });
                }
            }
        }
    }

    /// Calls the run closure, if there is one. A `Quit` end sets `quit`; a `Failed` end is logged and
    /// returned as the run failure.
    fn run_closure(&mut self, quit: &mut bool) -> Option<ServiceError> {
        let run = self.run.take()?;
        let context = RunContext {
            quit: self.quit.clone(),
        };
        match run(&context) {
            RunEnd::Finished => None,
            RunEnd::Quit => {
                *quit = true;
                None
            }
            RunEnd::Failed(message) => {
                record!(self.log_wait, Level::Error, "run failed: {message}");
                Some(ServiceError::new(message))
            }
        }
    }

    /// Runs everything, owners included, and reports what happened to them as well as the outcome.
    ///
    /// See the [crate documentation](crate) for the order of the phases, the start-failure cleanup and
    /// the meaning of [`Report::missed`] and [`Report::unfinished`].
    #[must_use]
    pub fn run_report(mut self) -> Report {
        let wait = self.log_wait;
        let quit_flag = self.quit.clone();
        let is_quit = move || {
            quit_flag
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::SeqCst))
        };
        let mut quit = false;
        let mut owner_phase: Option<Instant> = None;
        let (started, mut start_failure, mut cut_short) = self.start_services(&is_quit);

        let mut supervision = owners::Supervision {
            log_wait: wait,
            missed: Vec::new(),
        };
        let mut tracked = Vec::new();
        let mut unfinished = Vec::new();
        let mut owner_failures = Vec::new();
        if start_failure.is_none() && !cut_short && !self.owners.is_empty() {
            // Formed just before the first spawn, after every service has started: time spent starting
            // services is never charged to the owners.
            let until = deadline(self.start_deadline);
            owner_phase = Some(until);
            let spawns = std::mem::take(&mut self.owners);
            let control = owners::StartControl {
                quit: &is_quit,
                slice: self.poll_slice,
            };
            match owners::start_phase(&mut tracked, &mut supervision, spawns, until, &control) {
                owners::StartEnd::Complete => {}
                owners::StartEnd::Failed(failure) => start_failure = Some(failure),
                owners::StartEnd::Quit => cut_short = true,
            }
        }
        // The final check, immediately before `start-up complete`: after the last service start and,
        // with owners, after the last successful poll. A failure that was returned wins over a quit.
        if start_failure.is_none() && !cut_short && is_quit() {
            cut_short = true;
        }
        // `quit` is sampled once more when the start phase ends, whatever the reason; on success the
        // final check above was that sample.
        quit |= cut_short || (start_failure.is_some() && is_quit());
        if cut_short {
            match owner_phase {
                Some(until) => supervision.record(
                    until,
                    Level::Info,
                    format_args!("quit requested during start-up"),
                ),
                None => {
                    record!(wait, Level::Info, "quit requested during start-up");
                }
            }
        }
        if (start_failure.is_some() || cut_short) && !tracked.is_empty() {
            unfinished = owners::cleanup(&mut tracked, &supervision, self.cleanup_deadline);
        }

        let mut run_failure = None;
        if start_failure.is_none() && !cut_short {
            record!(wait, Level::Info, "start-up complete");
            run_failure = self.run_closure(&mut quit);
            if !tracked.is_empty() {
                let until = deadline(self.stop_deadline);
                (owner_failures, unfinished) =
                    owners::stop_phase(&mut tracked, &mut supervision, until);
            }
        }

        let mut stop_failures = owner_failures;
        self.stop_services(started, &mut stop_failures);
        if stop_failures.is_empty() {
            record!(wait, Level::Info, "shutdown complete");
        } else {
            record!(
                wait,
                Level::Error,
                "shutdown finished with {} stop failure(s)",
                stop_failures.len()
            );
        }

        let outcome = outcome_of(start_failure, run_failure, stop_failures);
        Report {
            outcome,
            missed: supervision.missed,
            unfinished,
            quit,
        }
    }
}

/// The outcome, by precedence: a failed start, then a failed run, then a failed stop, then clean.
fn outcome_of(
    start_failure: Option<Failure>,
    run_failure: Option<ServiceError>,
    stop_failures: Vec<Failure>,
) -> Outcome {
    match (start_failure, run_failure) {
        (Some(Failure { service, error }), _) => Outcome::StartFailed {
            service,
            error,
            stop_failures,
        },
        (None, Some(error)) => Outcome::RunFailed {
            error,
            stop_failures,
        },
        (None, None) if stop_failures.is_empty() => Outcome::Clean,
        (None, None) => Outcome::StopFailed {
            failures: stop_failures,
        },
    }
}
