//! The caller's handle on one VM owner thread.

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::call::{CallId, CallResult, Completion, Progress};
use crate::config::ConfigValue;
use crate::error::{VmError, VmState};
use crate::extension::{self, Extension};
use crate::interrupt::Fatal;
use crate::runner::{self, Command, Failure, Job, Report, Wake};
use crate::unwind;

/// How often `wait_finished` looks at whether the thread has ended.
const POLL: Duration = Duration::from_millis(1);

/// What a VM owner is created from.
#[derive(Clone)]
pub struct VmSettings {
    /// The owner's name: lowercase letters, digits and `_`, non-empty. It names the thread and the
    /// log category `lua.<name>`.
    pub name: String,
    /// The path of the entry script.
    pub entry: PathBuf,
    /// The configuration, copied in and shown to the script as the read-only `config`.
    pub config: ConfigValue,
    /// The global tables of host functions and callback setters, installed in this order before the entry
    /// script runs (default empty). Table names are unique across the list, and so are callback names
    /// (a posted call names its callback); function and setter names belong to their own table.
    /// [`VmOwner::spawn`] refuses a list that breaks either rule.
    pub extensions: Vec<Extension>,
    /// How many posted calls the handle may have in flight; a result counts until it is taken
    /// (default 0: [`post`](VmOwner::post) is refused with [`VmError::Full`]).
    pub call_limit: usize,
    /// The report wake (default `None`): called on the owner thread after each report is sent, and
    /// once more as the very last thing before the thread ends. It must be short and must not block,
    /// and a call at the end is a notification, not proof that [`is_finished`](VmOwner::is_finished) is
    /// true. A panic in it is caught and ignored.
    pub on_report: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The completion hook (default `None`): called on the owner thread, outside any lock and with no
    /// access to the Lua state, after `startup()` returned and after every posted call that has a
    /// result, immediately before its report and wake (see the crate documentation, "The completion
    /// hook"). It must be short and must not block or wait for another thread's progress. A panic in it
    /// ends the owner with [`VmError::Panicked`] and no result is sent.
    pub on_complete: Option<Arc<dyn Fn(Completion) + Send + Sync>>,
}

impl VmSettings {
    /// Settings with no extensions, no posted calls, no report wake and no completion hook.
    #[must_use]
    pub fn new(name: impl Into<String>, entry: impl Into<PathBuf>, config: ConfigValue) -> Self {
        Self {
            name: name.into(),
            entry: entry.into(),
            config,
            extensions: Vec::new(),
            call_limit: 0,
            on_report: None,
            on_complete: None,
        }
    }
}

impl fmt::Debug for VmSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VmSettings")
            .field("name", &self.name)
            .field("entry", &self.entry)
            .field("config", &self.config)
            .field("extensions", &self.extensions)
            .field("call_limit", &self.call_limit)
            .field(
                "on_report",
                &self.on_report.as_ref().map_or("absent", |_| "present"),
            )
            .field(
                "on_complete",
                &self.on_complete.as_ref().map_or("absent", |_| "present"),
            )
            .finish()
    }
}

/// A handle on one Lua VM owner: one OS thread with its own Lua state. See the crate documentation
/// for the states, the legal calls, cancel and ending.
#[derive(Debug)]
pub struct VmOwner {
    name: String,
    state: VmState,
    control: Sender<Command>,
    reports: Receiver<Report>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Reports a non-blocking drain met that are not call results: kept for the next wait.
    held: VecDeque<Report>,
    /// Results of posted calls, not yet taken.
    results: VecDeque<CallResult>,
    /// Posted calls whose result has not arrived (until the thread is observed finished).
    outstanding: usize,
    call_limit: usize,
    next_call: u64,
}

/// Whether `name` is a valid owner name.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

impl Failure {
    fn into_error(self, owner: &str) -> VmError {
        match self {
            Self::Script { message, traceback } => VmError::Script {
                owner: owner.to_owned(),
                message,
                traceback,
            },
            Self::Panicked { message } => VmError::Panicked {
                owner: owner.to_owned(),
                message,
            },
            Self::Cancelled => VmError::Cancelled {
                owner: owner.to_owned(),
            },
        }
    }
}

impl VmOwner {
    /// Starts the owner thread and returns at once; the thread creates its state, installs the host
    /// surface, runs the entry script and reports ready (see [`wait_ready`](Self::wait_ready)).
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a name that is empty or has anything but lowercase letters, digits and `_`, or
    /// an invalid list of extensions (an invalid one, two with the same table name, a callback name used by
    /// two) (no thread is started), or the operating system's error when the thread cannot be created.
    pub fn spawn(settings: VmSettings) -> io::Result<Self> {
        let VmSettings {
            name,
            entry,
            config,
            extensions,
            call_limit,
            on_report,
            on_complete,
        } = settings;
        if !valid_name(&name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("`{name}` is not a valid VM name (lowercase letters, digits and `_`)"),
            ));
        }
        if let Err(why) = extension::validate_all(&extensions) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, why));
        }
        let (control, control_rx) = mpsc::channel();
        let (reports_tx, reports) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        #[cfg(test)]
        crate::hooks::remember_cancel(&name, Arc::clone(&cancel));
        let wake: Option<Wake> = on_report;
        let job = Job {
            name: name.clone(),
            entry,
            config,
            extensions,
            on_report: wake,
            on_complete,
            control: control_rx,
            reports: reports_tx,
            fatal: Fatal::new(Arc::clone(&cancel)),
            cancel: Arc::clone(&cancel),
        };
        let thread = thread::Builder::new()
            .name(format!("ck-lua-{name}"))
            .spawn(move || runner::thread_main(&job))?;
        Ok(Self {
            name,
            state: VmState::Initialising,
            control,
            reports,
            cancel,
            thread: Some(thread),
            held: VecDeque::new(),
            results: VecDeque::new(),
            outstanding: 0,
            call_limit,
            next_call: 0,
        })
    }

    /// The owner's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The handle's state.
    #[must_use]
    pub fn state(&self) -> VmState {
        self.state
    }

    fn require(&self, call: &'static str, legal: &[VmState]) -> Result<(), VmError> {
        if legal.contains(&self.state) {
            Ok(())
        } else {
            Err(VmError::State {
                owner: self.name.clone(),
                call,
                state: self.state,
            })
        }
    }

    /// Sends a command to the thread (counted by the tests, so a refusal can be shown to send nothing).
    fn send(&self, command: Command) -> Result<(), mpsc::SendError<Command>> {
        #[cfg(test)]
        crate::hooks::sent(&self.name, &command);
        self.control.send(command)
    }

    /// Sets the cancel flag and wakes an idle thread. The handle is then Ending.
    fn abandon(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.send(Command::Wake);
        if self.state != VmState::Finished {
            self.state = VmState::Ending;
        }
    }

    /// Keeps a call result for `take_result`: it is no longer outstanding, but it still counts as in
    /// flight until it is taken.
    fn keep(&mut self, result: CallResult) {
        self.outstanding = self.outstanding.saturating_sub(1);
        self.results.push_back(result);
    }

    /// Waits at most `within` for the next lifecycle report; call results that arrive meanwhile are
    /// kept, never taken for the awaited report. `None` is a timeout, and changes nothing.
    fn receive(&mut self, within: Duration) -> Result<Option<Report>, VmError> {
        if let Some(report) = self.held.pop_front() {
            return Ok(Some(report));
        }
        let limit = Instant::now().checked_add(within);
        let mut left = within;
        loop {
            match self.reports.recv_timeout(left) {
                Ok(Report::Call(result)) => {
                    self.keep(result);
                    left = limit.map_or(within, |limit| {
                        limit.saturating_duration_since(Instant::now())
                    });
                }
                Ok(report) => return Ok(Some(report)),
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => {
                    self.state = VmState::Ending;
                    return Err(VmError::Panicked {
                        owner: self.name.clone(),
                        message: "the owner thread ended without a report".to_owned(),
                    });
                }
            }
        }
    }

    /// Waits for a report; a missed deadline abandons the request.
    fn await_report(&mut self, deadline: Duration) -> Result<Report, VmError> {
        if let Some(report) = self.receive(deadline)? {
            return Ok(report);
        }
        self.abandon();
        Err(VmError::Deadline {
            owner: self.name.clone(),
        })
    }

    /// Consumes the next report as the answer to a request that succeeds into `next`.
    fn answer(
        &mut self,
        deadline: Duration,
        expected: Expected,
        next: VmState,
    ) -> Result<(), VmError> {
        let report = self.await_report(deadline)?;
        self.apply(report, expected, next)
    }

    /// Like [`answer`](Self::answer), but a missed deadline is `Pending` and changes nothing.
    fn poll_answer(
        &mut self,
        within: Duration,
        expected: Expected,
        next: VmState,
    ) -> Result<Progress, VmError> {
        match self.receive(within)? {
            None => Ok(Progress::Pending),
            Some(report) => self.apply(report, expected, next).map(|()| Progress::Done),
        }
    }

    /// Applies a report to the handle: the expected one moves it to `next`, a failure to Ending.
    fn apply(&mut self, report: Report, expected: Expected, next: VmState) -> Result<(), VmError> {
        match report {
            Report::Ready if expected == Expected::Ready => {
                self.state = next;
                Ok(())
            }
            Report::Done if expected == Expected::Done => {
                self.state = next;
                Ok(())
            }
            Report::Failed(failure) => {
                self.state = VmState::Ending;
                Err(failure.into_error(&self.name))
            }
            Report::Ready | Report::Done | Report::Call(_) => {
                self.state = VmState::Ending;
                Err(VmError::Panicked {
                    owner: self.name.clone(),
                    message: "the owner thread sent an unexpected report".to_owned(),
                })
            }
        }
    }

    /// Waits for the ready report, which the thread sends after the main chunk ran. A report that
    /// arrived earlier is consumed at once.
    ///
    /// # Errors
    ///
    /// [`VmError::State`] unless the handle is Initialising; [`VmError::Script`] or
    /// [`VmError::Panicked`] when initialisation failed (the owner is then Ending);
    /// [`VmError::Deadline`] when the deadline passed first (the request is abandoned).
    pub fn wait_ready(&mut self, deadline: Duration) -> Result<(), VmError> {
        self.require("wait_ready", &[VmState::Initialising])?;
        self.answer(deadline, Expected::Ready, VmState::Ready)
    }

    /// Waits at most `within` for the report that [`wait_ready`](Self::wait_ready) would consume, and
    /// **never abandons**: with no report in time it returns [`Progress::Pending`] and changes nothing
    /// (no cancel flag, no command, the same state, the request still in flight), so it can be called
    /// again in slices. A report that arrived is consumed exactly as `wait_ready` does (`Done` moves
    /// the handle to Ready; a failure returns the same error and moves it to Ending). A zero `within`
    /// makes one non-waiting observation.
    ///
    /// # Errors
    ///
    /// [`VmError::State`] unless the handle is Initialising; otherwise as
    /// [`wait_ready`](Self::wait_ready) except [`VmError::Deadline`], which a poll never returns.
    pub fn poll_ready(&mut self, within: Duration) -> Result<Progress, VmError> {
        self.require("poll_ready", &[VmState::Initialising])?;
        self.poll_answer(within, Expected::Ready, VmState::Ready)
    }

    /// Asks the thread to call the global `startup()` if it is defined.
    ///
    /// # Errors
    ///
    /// [`VmError::State`] unless the handle is Ready; [`VmError::Script`] or [`VmError::Panicked`]
    /// when the call failed (the owner is then Ending); [`VmError::Deadline`] when the deadline passed
    /// first: the request is abandoned and the thread ends, without calling `shutdown()`, when the
    /// running call returns.
    pub fn start(&mut self, deadline: Duration) -> Result<(), VmError> {
        self.require("start", &[VmState::Ready])?;
        self.send_start();
        self.finish_start(deadline)
    }

    /// Sends the start request and returns at once, without waiting; the handle moves to Starting.
    /// [`await_started`](Self::await_started) then waits for the result, so a supervisor can have
    /// requests to several owners in flight together.
    ///
    /// # Errors
    ///
    /// [`VmError::State`] unless the handle is Ready (a second `request_start` is refused).
    pub fn request_start(&mut self) -> Result<(), VmError> {
        self.require("request_start", &[VmState::Ready])?;
        self.send_start();
        Ok(())
    }

    /// Waits at most `deadline` for the result of [`request_start`](Self::request_start): `Ok` means
    /// the handle is Started. A zero deadline makes one non-waiting observation.
    ///
    /// # Errors
    ///
    /// [`VmError::State`] unless the handle is Starting; otherwise as [`start`](Self::start): a Lua
    /// error or panic ([`VmError::Script`], [`VmError::Panicked`], the owner is then Ending) or
    /// [`VmError::Deadline`], which abandons the request exactly as `start` does.
    pub fn await_started(&mut self, deadline: Duration) -> Result<(), VmError> {
        self.require("await_started", &[VmState::Starting])?;
        self.finish_start(deadline)
    }

    /// Waits at most `within` for the result that [`await_started`](Self::await_started) would
    /// consume, and **never abandons**: with no result in time it returns [`Progress::Pending`] and
    /// changes nothing. See [`poll_ready`](Self::poll_ready).
    ///
    /// # Errors
    ///
    /// [`VmError::State`] unless the handle is Starting; otherwise as
    /// [`await_started`](Self::await_started) except [`VmError::Deadline`].
    pub fn poll_started(&mut self, within: Duration) -> Result<Progress, VmError> {
        self.require("poll_started", &[VmState::Starting])?;
        self.poll_answer(within, Expected::Done, VmState::Started)
    }

    fn send_start(&mut self) {
        let _ = self.send(Command::Start);
        self.state = VmState::Starting;
    }

    fn finish_start(&mut self, deadline: Duration) -> Result<(), VmError> {
        self.answer(deadline, Expected::Done, VmState::Started)
    }

    /// Asks the thread to call the global `shutdown()` if it is defined and then end, and returns
    /// `Ok` only after observing the thread finished (`JoinHandle::is_finished`, then `join`).
    ///
    /// # Errors
    ///
    /// [`VmError::State`] unless the handle is Ready or Started; [`VmError::Script`] or
    /// [`VmError::Panicked`] when the call failed (the owner is then Ending);
    /// [`VmError::Deadline`] when the reply or the end of the thread (including the stdout flush)
    /// took longer than the deadline.
    pub fn stop(&mut self, deadline: Duration) -> Result<(), VmError> {
        self.require("stop", &[VmState::Ready, VmState::Started])?;
        self.send_stop();
        self.finish_stop(deadline)
    }

    /// Sends the stop request and returns at once, without waiting; the handle moves to Stopping.
    /// [`await_stopped`](Self::await_stopped) then waits for the result and for the thread to end.
    ///
    /// # Errors
    ///
    /// [`VmError::State`] unless the handle is Ready or Started.
    pub fn request_stop(&mut self) -> Result<(), VmError> {
        self.require("request_stop", &[VmState::Ready, VmState::Started])?;
        self.send_stop();
        Ok(())
    }

    /// Waits at most `deadline` for the result of [`request_stop`](Self::request_stop), and returns `Ok`
    /// only after observing the thread finished (`JoinHandle::is_finished`, then `join`). A zero
    /// deadline makes one non-waiting observation: a result and a finished thread that are already
    /// there are seen as such.
    ///
    /// # Errors
    ///
    /// [`VmError::State`] unless the handle is Stopping; otherwise as [`stop`](Self::stop).
    pub fn await_stopped(&mut self, deadline: Duration) -> Result<(), VmError> {
        self.require("await_stopped", &[VmState::Stopping])?;
        self.finish_stop(deadline)
    }

    fn send_stop(&mut self) {
        let _ = self.send(Command::Stop);
        self.state = VmState::Stopping;
    }

    fn finish_stop(&mut self, deadline: Duration) -> Result<(), VmError> {
        let limit = Instant::now().checked_add(deadline);
        self.answer(deadline, Expected::Done, VmState::Ending)?;
        let left = limit.map_or(deadline, |limit| {
            limit.saturating_duration_since(Instant::now())
        });
        if self.wait_finished(left) {
            Ok(())
        } else {
            Err(VmError::Deadline {
                owner: self.name.clone(),
            })
        }
    }

    /// Sets the cancel flag and wakes an idle thread; it never waits. An idle thread ends without
    /// calling `shutdown()`; initialisation ends at its next check; a running Lua call completes
    /// first and the thread then ends without calling more Lua. The handle is then Ending (unless it
    /// is already Finished).
    pub fn cancel(&mut self) {
        self.abandon();
    }

    /// Waits at most `deadline` for the thread to end; `true` once it was observed finished and
    /// joined (the handle is then Finished).
    pub fn wait_finished(&mut self, deadline: Duration) -> bool {
        let limit = Instant::now().checked_add(deadline);
        loop {
            match self.thread.take() {
                None => {
                    self.settle();
                    return true;
                }
                Some(handle) if handle.is_finished() => {
                    unwind::join(handle);
                    self.settle();
                    return true;
                }
                Some(handle) => self.thread = Some(handle),
            }
            let left = limit.map_or(POLL, |limit| {
                limit.saturating_duration_since(Instant::now())
            });
            if left.is_zero() {
                return false;
            }
            thread::sleep(left.min(POLL));
        }
    }

    /// The thread was observed ended and joined: the handle is Finished, every result it sent is
    /// kept, and a call that has no result by now never will, so nothing is outstanding.
    fn settle(&mut self) {
        let _ = self.drain();
        self.outstanding = 0;
        self.state = VmState::Finished;
    }

    /// Takes whatever the thread has already sent, without waiting: call results are kept, anything
    /// else is held for the next wait. `true` when the channel is closed (the thread has ended).
    fn drain(&mut self) -> bool {
        loop {
            match self.reports.try_recv() {
                Ok(Report::Call(result)) => self.keep(result),
                Ok(report) => self.held.push_back(report),
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => return true,
            }
        }
    }

    /// Posts a call of the callback registered under `callback` and returns at once, never waiting.
    /// Legal in Started only. The thread runs calls one at a time, in the order posted and interleaved
    /// with other requests in the order sent (a call posted before
    /// [`request_stop`](Self::request_stop) runs before `shutdown()`); [`take_result`](Self::take_result)
    /// returns the results.
    ///
    /// # Errors
    ///
    /// Checked in this order, and none of them sends anything: [`VmError::State`] unless the handle is
    /// Started; [`VmError::Disconnected`] when the owner's thread has ended (this comes before the
    /// limit, so an ended owner is never hidden behind calls left in flight); [`VmError::Full`] when
    /// `call_limit` calls are in flight (a call counts from `post` until its result is taken).
    pub fn post(&mut self, callback: &str, args: Vec<ConfigValue>) -> Result<CallId, VmError> {
        self.require("post", &[VmState::Started])?;
        // Ended, as seen through the closed channel or the thread handle.
        if self.drain() || self.is_finished() {
            return Err(VmError::Disconnected {
                owner: self.name.clone(),
            });
        }
        if self.in_flight() >= self.call_limit {
            return Err(VmError::Full {
                owner: self.name.clone(),
            });
        }
        let id = CallId(self.next_call + 1);
        let command = Command::Call {
            id,
            callback: callback.to_owned(),
            args,
        };
        if self.send(command).is_err() {
            // The thread's receiver is gone: the command came back undelivered.
            return Err(VmError::Disconnected {
                owner: self.name.clone(),
            });
        }
        self.next_call += 1;
        self.outstanding += 1;
        Ok(id)
    }

    /// The oldest result not yet taken, in the order the calls ran; it never waits and is legal in every
    /// state. Taking a result frees one place under the call limit. Each call yields at most one result:
    /// calls still queued or running when the thread ends yield none.
    pub fn take_result(&mut self) -> Option<CallResult> {
        let _ = self.drain();
        self.results.pop_front()
    }

    /// Takes the terminal failure of the owner thread if it has already reported one, and **never
    /// waits** and never sets the cancel flag. Legal in Ready and Started; in any other state it returns
    /// `None` and consumes nothing, so a report that `wait_ready`, a poll, `await_started` or
    /// `await_stopped` is waiting for stays for that call.
    ///
    /// It takes every report already received, keeping call results (in order) for
    /// [`take_result`](Self::take_result). A lifecycle failure that was received, including one a
    /// non-waiting drain by `post` or `take_result` had set aside, comes back once as the [`VmError`]
    /// the waiting calls would return ([`VmError::Script`], [`VmError::Panicked`] or
    /// [`VmError::Cancelled`]), and the handle is then Ending, as after a failed `await_started`. A
    /// report channel that is closed with no failure reported gives [`VmError::Panicked`] ("the owner
    /// thread ended without a report"), once, and the handle is Ending. Otherwise `None`.
    ///
    /// The owner thread sends a terminal failure **before** its report wake and its final wake, so a
    /// call made after any wake that followed the failure sees it. Ending is the caller's logical end,
    /// not proof that the thread returned: [`wait_finished`](Self::wait_finished) still gives that.
    pub fn take_failure(&mut self) -> Option<VmError> {
        if !matches!(self.state, VmState::Ready | VmState::Started) {
            return None;
        }
        let closed = self.drain();
        let failed = self
            .held
            .iter()
            .position(|report| matches!(report, Report::Failed(_)))
            .and_then(|position| self.held.remove(position));
        let error = match failed {
            Some(Report::Failed(failure)) => failure.into_error(&self.name),
            _ if closed => VmError::Panicked {
                owner: self.name.clone(),
                message: "the owner thread ended without a report".to_owned(),
            },
            _ => return None,
        };
        self.state = VmState::Ending;
        Some(error)
    }

    /// The posted calls in flight: sent with no result yet, plus results not yet taken.
    pub(crate) fn in_flight(&self) -> usize {
        self.outstanding + self.results.len()
    }

    /// Whether the thread has ended.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

/// Which report answers a request.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Expected {
    Ready,
    Done,
}

impl Drop for VmOwner {
    /// Sets the cancel flag and never blocks: a finished thread is joined, a running one is detached.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.send(Command::Wake);
        if let Some(handle) = self.thread.take()
            && handle.is_finished()
        {
            unwind::join(handle);
        }
    }
}
