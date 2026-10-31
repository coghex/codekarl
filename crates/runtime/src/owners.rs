//! The supervision of owners: the start phase, the cleanup after a start failure, and the stop phase.
//!
//! Every wait is against an absolute deadline, so owners that run concurrently each get the whole
//! phase, and a record logged in a phase never extends it (see the crate documentation).

use std::fmt;
use std::time::{Duration, Instant};

use ck_log::Level;

use crate::{Failure, Owner, OwnerError, OwnerSpawn, Progress, ServiceError, deadline};

/// An owner tracked from the moment it was spawned.
pub(crate) struct Tracked {
    name: String,
    owner: Box<dyn Owner>,
    /// The owner's thread has been observed finished.
    finished: bool,
    /// A stop request was accepted, so the owner is awaited.
    stop_requested: bool,
}

impl Tracked {
    fn new(name: String, owner: Box<dyn Owner>) -> Self {
        Self {
            name,
            owner,
            finished: false,
            stop_requested: false,
        }
    }
}

/// The log wait and the owners that missed a deadline, in order of occurrence.
pub(crate) struct Supervision {
    pub(crate) log_wait: Duration,
    pub(crate) missed: Vec<String>,
}

/// The time left until `until`; zero once it has passed (a zero budget is one non-waiting look).
fn left(until: Instant) -> Duration {
    until.saturating_duration_since(Instant::now())
}

impl Supervision {
    /// Emits one record of an owner phase through `log_within`, capped by that phase's deadline: the
    /// earlier of now plus the log wait and `until`. After `until` this is one non-waiting attempt.
    pub(crate) fn record(&self, until: Instant, level: Level, message: fmt::Arguments<'_>) {
        let at = deadline(self.log_wait).min(until);
        let _ = ck_log::log_within!(level, at, "runtime", "{message}");
    }

    /// Notes that `name` missed a deadline (once, in order of occurrence); `true` the first time.
    fn miss(&mut self, name: &str) -> bool {
        if self.missed.iter().any(|missed| missed == name) {
            false
        } else {
            self.missed.push(name.to_owned());
            true
        }
    }

    /// Logs a start failure or a missed start deadline and returns the failure for the outcome.
    fn start_failure(&mut self, name: &str, error: OwnerError, until: Instant) -> Failure {
        match error {
            OwnerError::Failed { message } => {
                self.record(
                    until,
                    Level::Error,
                    format_args!("{name} failed to start: {message}"),
                );
                Failure {
                    service: name.to_owned(),
                    error: ServiceError::new(message),
                }
            }
            OwnerError::Deadline => {
                self.record(
                    until,
                    Level::Error,
                    format_args!("{name} missed its start deadline"),
                );
                self.miss(name);
                Failure {
                    service: name.to_owned(),
                    error: ServiceError::new("missed its start deadline"),
                }
            }
        }
    }

    /// Logs a failing stop and counts it.
    fn stop_failure(
        &self,
        name: &str,
        message: String,
        until: Instant,
        failures: &mut Vec<Failure>,
    ) {
        self.record(
            until,
            Level::Error,
            format_args!("{name} failed to stop: {message}"),
        );
        failures.push(Failure {
            service: name.to_owned(),
            error: ServiceError::new(message),
        });
    }

    /// Notes a missed stop deadline and logs it, the first time only (an owner that missed it at its
    /// await is not logged again when the last observation of it expires too).
    fn stop_missed(&mut self, name: &str, until: Instant) {
        if self.miss(name) {
            self.record(
                until,
                Level::Error,
                format_args!("{name} missed its stop deadline"),
            );
        }
    }
}

/// What the start phase needs to notice a quit request and to slice its waits.
pub(crate) struct StartControl<'a> {
    /// Whether the quit flag is set (always false without one).
    pub(crate) quit: &'a dyn Fn() -> bool,
    /// The longest a single poll waits.
    pub(crate) slice: Duration,
}

/// How the start phase ended.
pub(crate) enum StartEnd {
    /// Every owner started.
    Complete,
    /// The first failure (including a missed deadline), already logged; it wins over a quit.
    Failed(Failure),
    /// The quit flag was seen set; nothing has been logged or cancelled yet.
    Quit,
}

/// Which result a start-phase wait is for.
#[derive(Clone, Copy)]
enum Stage {
    Ready,
    Started,
}

/// Waits for `owner`'s result in slices of at most `control.slice`, never abandoning it, until it is
/// done (`Ok`) or the phase ends early (`Err`): a returned failure, the phase deadline, or the quit flag.
///
/// The order is fixed. A returned failure wins. After a `Pending` poll the deadline is checked **before**
/// the flag: once it has passed, the existing wait is called exactly once with a zero budget (a result
/// that arrived at the last moment is used normally, otherwise the missed-deadline path runs). The flag is
/// consulted only while the deadline has not passed.
fn wait_for(
    owner: &mut Tracked,
    supervision: &mut Supervision,
    until: Instant,
    control: &StartControl<'_>,
    stage: Stage,
) -> Result<(), StartEnd> {
    loop {
        let within = control.slice.min(left(until));
        let polled = match stage {
            Stage::Ready => owner.owner.poll_ready(within),
            Stage::Started => owner.owner.poll_started(within),
        };
        match polled {
            Ok(Progress::Done) => return Ok(()),
            Err(error) => {
                return Err(StartEnd::Failed(supervision.start_failure(
                    &owner.name,
                    error,
                    until,
                )));
            }
            Ok(Progress::Pending) => {
                if left(until).is_zero() {
                    let last = match stage {
                        Stage::Ready => owner.owner.wait_ready(Duration::ZERO),
                        Stage::Started => owner.owner.await_started(Duration::ZERO),
                    };
                    return last.map_err(|error| {
                        StartEnd::Failed(supervision.start_failure(&owner.name, error, until))
                    });
                }
                if (control.quit)() {
                    return Err(StartEnd::Quit);
                }
            }
        }
    }
}

/// The start phase against the absolute deadline `until`: spawn every owner in order, wait for
/// readiness of each, send `request_start` to **every** owner, then wait for each start; the waits are
/// polls in slices (see [`wait_for`]). The quit flag is checked before each spawn, before each
/// `request_start`, before each owner's first poll of a fresh wait (readiness and start) and between
/// slices. The first failure (including a missed deadline) ends the phase
/// with the failure to report; the owners spawned so far stay in `tracked`.
pub(crate) fn start_phase(
    tracked: &mut Vec<Tracked>,
    supervision: &mut Supervision,
    spawns: Vec<OwnerSpawn>,
    until: Instant,
    control: &StartControl<'_>,
) -> StartEnd {
    for OwnerSpawn { name, spawn } in spawns {
        if (control.quit)() {
            return StartEnd::Quit;
        }
        match spawn() {
            Ok(owner) => tracked.push(Tracked::new(name, owner)),
            Err(message) => {
                return StartEnd::Failed(supervision.start_failure(
                    &name,
                    OwnerError::Failed { message },
                    until,
                ));
            }
        }
    }
    for owner in tracked.iter_mut() {
        // A fresh wait: the quit flag is looked at before the owner's first poll, so a flag set while the
        // previous owner's poll returned `Done` is not left pending through another owner's observation.
        if (control.quit)() {
            return StartEnd::Quit;
        }
        if let Err(end) = wait_for(owner, supervision, until, control, Stage::Ready) {
            return end;
        }
    }
    for owner in tracked.iter_mut() {
        if (control.quit)() {
            return StartEnd::Quit;
        }
        if let Err(error) = owner.owner.request_start() {
            return StartEnd::Failed(supervision.start_failure(&owner.name, error, until));
        }
    }
    for owner in tracked.iter_mut() {
        // Again before each fresh wait, after the previous owner's `started` record.
        if (control.quit)() {
            return StartEnd::Quit;
        }
        if let Err(end) = wait_for(owner, supervision, until, control, Stage::Started) {
            return end;
        }
        supervision.record(until, Level::Info, format_args!("started {}", owner.name));
    }
    StartEnd::Complete
}

/// Logs `did not finish` for every owner not observed finished and returns their names, in order.
fn report_unfinished(
    tracked: &[Tracked],
    supervision: &Supervision,
    until: Instant,
) -> Vec<String> {
    let names: Vec<String> = tracked
        .iter()
        .filter(|owner| !owner.finished)
        .map(|owner| owner.name.clone())
        .collect();
    for name in &names {
        supervision.record(until, Level::Error, format_args!("{name} did not finish"));
    }
    names
}

/// The cleanup after a start failure: cancel **every** spawned owner (no stop request is sent), give
/// each until `within` from now to be observed finished, and return the names of those that were not.
pub(crate) fn cleanup(
    tracked: &mut [Tracked],
    supervision: &Supervision,
    within: Duration,
) -> Vec<String> {
    for owner in tracked.iter_mut() {
        owner.owner.cancel();
    }
    let until = deadline(within);
    for owner in tracked.iter_mut() {
        owner.finished = owner.owner.wait_finished(left(until));
    }
    report_unfinished(tracked, supervision, until)
}

/// What a stop request came to, kept until every request has been sent.
enum Requested {
    /// The owner accepted it and will be awaited.
    Accepted,
    /// The request failed with this message; the owner was cancelled and gets no await.
    Failed(String),
    /// The request itself missed its deadline; the owner was cancelled and gets no await.
    Missed,
}

/// The stop phase against the absolute deadline `until`; returns the stop failures and the owners not
/// observed finished.
///
/// 1. **Dispatch:** every owner is sent its request, in reverse order, and a failing request only
///    cancels its owner; nothing is logged yet, so a failure can never use up the stop window of an
///    owner whose request has not been sent.
/// 2. The requests' results are logged and counted.
/// 3. Each accepted request is awaited in reverse order against the same deadline (a sibling is still
///    observed with no time left).
/// 4. Every owner not yet observed finished is waited for once more with the time left. If that last
///    observation expires too, the owner has missed the stop deadline, whatever else was reported
///    about it (a failure does not replace the missed deadline).
pub(crate) fn stop_phase(
    tracked: &mut [Tracked],
    supervision: &mut Supervision,
    until: Instant,
) -> (Vec<Failure>, Vec<String>) {
    let requested: Vec<Requested> = tracked
        .iter_mut()
        .rev()
        .map(|owner| match owner.owner.request_stop() {
            Ok(()) => Requested::Accepted,
            Err(OwnerError::Failed { message }) => {
                owner.owner.cancel();
                Requested::Failed(message)
            }
            Err(OwnerError::Deadline) => {
                owner.owner.cancel();
                Requested::Missed
            }
        })
        .collect();
    let mut failures = Vec::new();
    for (owner, result) in tracked.iter_mut().rev().zip(requested) {
        match result {
            Requested::Accepted => owner.stop_requested = true,
            Requested::Failed(message) => {
                supervision.stop_failure(&owner.name, message, until, &mut failures);
            }
            Requested::Missed => supervision.stop_missed(&owner.name, until),
        }
    }
    for owner in tracked
        .iter_mut()
        .rev()
        .filter(|owner| owner.stop_requested)
    {
        match owner.owner.await_stopped(left(until)) {
            Ok(()) => {
                owner.finished = true;
                supervision.record(until, Level::Info, format_args!("stopped {}", owner.name));
            }
            Err(OwnerError::Failed { message }) => {
                supervision.stop_failure(&owner.name, message, until, &mut failures);
            }
            Err(OwnerError::Deadline) => {
                supervision.stop_missed(&owner.name, until);
                owner.owner.cancel();
            }
        }
    }
    for owner in tracked.iter_mut().rev().filter(|owner| !owner.finished) {
        owner.finished = owner.owner.wait_finished(left(until));
        if !owner.finished {
            supervision.stop_missed(&owner.name, until);
        }
    }
    let unfinished = report_unfinished(tracked, supervision, until);
    (failures, unfinished)
}
