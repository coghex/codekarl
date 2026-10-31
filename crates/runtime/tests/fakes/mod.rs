//! Fake owners that model the cooperative contract of `ck_runtime::Owner`: no Lua and no sleeps to
//! reach a state.
//!
//! Each fake has a worker thread that does the "work", driven through channels. The contract is
//! kept literally:
//!
//! - `request_*` and `cancel` **never wait**: they note the call and send on a channel;
//! - `await_*` and `wait_finished` wait at most their `within` budget;
//! - a thread is *observed finished* only from the real thing, `JoinHandle::is_finished` (then
//!   `join`), never from a channel that disconnects just before the thread really ends.
//!
//! Deterministic coordination is put where the contract allows waiting, outside the non-waiting
//! calls: [`Script::wait_for_sibling`] makes an `await_stopped` first wait, within its own budget,
//! until a named sibling's thread has really ended, so that a later zero-budget observation of that
//! sibling is guaranteed to see a finished thread; [`Script::await_cap`] makes an owner's `await_*`
//! report `Deadline` after only part of its budget, which leaves time for the cancel it provokes to
//! take effect before the final observation.
//!
//! Every worker is released, and joined with a bound, on every path: [`Cleanup`] does that when it
//! is dropped, which also happens when an assertion fails.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ck_runtime::{Owner, OwnerError, OwnerSpawn, Progress, Service, ServiceError};

/// The longest a stuck worker waits before giving up, so a broken test cannot hang.
const HOLD: Duration = Duration::from_secs(30);
/// The longest a rendezvous waits for the other request: a runtime that serialises them fails fast.
const MEET: Duration = Duration::from_secs(5);
/// The longest cleanup waits for a worker to end after it was released.
const JOIN_BOUND: Duration = Duration::from_secs(10);
/// How often a bounded wait for a thread to end looks at it.
const LOOK: Duration = Duration::from_micros(200);

/// What a worker does at a step of its life.
#[derive(Clone, Copy)]
pub(crate) enum Step {
    /// Succeeds at once.
    Done,
    /// Reports this failure and ends.
    Fail(&'static str),
    /// Reports this failure and then stays alive, stuck, until released.
    FailThenStuck(&'static str),
    /// Does not finish until released (or, if the script says so, until cancelled).
    Stuck,
}

/// How a fake behaves.
#[derive(Clone)]
pub(crate) struct Script {
    pub(crate) ready: Step,
    pub(crate) start: Step,
    pub(crate) stop: Step,
    /// A stuck worker ends at once when cancelled; otherwise it ignores cancel until released.
    pub(crate) ends_on_cancel: bool,
    /// An idle worker ignores cancel too, and stays alive until released.
    pub(crate) ignores_cancel_when_idle: bool,
    /// `request_stop` itself fails with this message.
    pub(crate) request_stop_fails: Option<&'static str>,
    /// Every `await_*` gives up with `Deadline` after at most this much of its budget.
    pub(crate) await_cap: Option<Duration>,
    /// `await_stopped` first waits, within its budget, until this sibling's thread has really ended.
    pub(crate) wait_for_sibling: Option<&'static str>,
    /// Both start requests (or both stop requests) must have arrived before either completes.
    pub(crate) start_meeting: Option<Arc<Rendezvous>>,
    pub(crate) stop_meeting: Option<Arc<Rendezvous>>,
    /// `poll_ready` waits its whole budget and says `Pending` this many times before it looks at the
    /// worker's reply (so a poll count shows that slicing really happened).
    pub(crate) pending_ready_polls: usize,
    /// The same for `poll_started`.
    pub(crate) pending_started_polls: usize,
    /// The polls that never look at the reply: each waits its whole budget and says `Pending`. The
    /// existing `wait_ready` and `await_started` still see a reply that is already there (the "last
    /// moment" result).
    pub(crate) polls_never_complete: NeverCompletes,
    /// `request_start` puts the `Started` reply in at once (no worker scheduling is involved), so a
    /// zero-budget observation after it deterministically finds the result.
    pub(crate) instant_start: bool,
    /// Sets the quit flag at this moment of the owner's life.
    pub(crate) sets_quit: Option<(Arc<AtomicBool>, QuitAt)>,
}

/// Which polls of a fake never report a result.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum NeverCompletes {
    No,
    Ready,
    Started,
    Both,
}

/// When a fake sets the quit flag, just before the call returns.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuitAt {
    /// In the `poll_ready` that returns something other than `Pending`.
    ReadyResult,
    /// In the `poll_started` that returns something other than `Pending`.
    StartedResult,
    /// In the n-th `poll_ready` (from 1) that returns `Pending`.
    ReadyPending(usize),
    /// In the n-th `poll_started` (from 1) that returns `Pending`.
    StartedPending(usize),
    /// In `request_start`.
    RequestStart,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            ready: Step::Done,
            start: Step::Done,
            stop: Step::Done,
            ends_on_cancel: false,
            ignores_cancel_when_idle: false,
            request_stop_fails: None,
            await_cap: None,
            wait_for_sibling: None,
            start_meeting: None,
            stop_meeting: None,
            pending_ready_polls: 0,
            pending_started_polls: 0,
            polls_never_complete: NeverCompletes::No,
            instant_start: false,
            sets_quit: None,
        }
    }
}

/// A rendezvous of `parties` workers that gives up after [`MEET`] instead of waiting for ever.
pub(crate) struct Rendezvous {
    arrived: Mutex<usize>,
    changed: Condvar,
    parties: usize,
}

impl Rendezvous {
    pub(crate) fn new(parties: usize) -> Arc<Self> {
        Arc::new(Self {
            arrived: Mutex::new(0),
            changed: Condvar::new(),
            parties,
        })
    }

    fn meet(&self) -> bool {
        let mut arrived = self.arrived.lock().unwrap_or_else(PoisonError::into_inner);
        *arrived += 1;
        self.changed.notify_all();
        let give_up = Instant::now() + MEET;
        while *arrived < self.parties {
            let left = give_up.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            arrived = self
                .changed
                .wait_timeout(arrived, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}

/// What the test shares with a worker: release and cancel flags, one condition variable.
struct Flags {
    state: Mutex<(bool, bool)>, // (released, cancelled)
    changed: Condvar,
}

impl Flags {
    fn lock(&self) -> MutexGuard<'_, (bool, bool)> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn release(&self) {
        self.lock().0 = true;
        self.changed.notify_all();
    }

    fn cancel(&self) {
        self.lock().1 = true;
        self.changed.notify_all();
    }
}

/// A call on a fake, with when it happened.
#[derive(Debug, Clone)]
pub(crate) struct Event {
    pub(crate) at: Instant,
    pub(crate) what: String,
}

enum Command {
    Start,
    Stop,
    Cancel,
}

enum Reply {
    Ready,
    Started,
    Stopped,
    Failed(String),
}

/// A worker's join handle, shared between its fake and the test's cleanup.
type Handle = Arc<Mutex<Option<JoinHandle<()>>>>;

/// Looks at a worker thread once: `true` if it has really ended (and then joins it).
fn ended(handle: &Handle) -> bool {
    let mut slot = handle.lock().unwrap_or_else(PoisonError::into_inner);
    match slot.take() {
        None => true,
        Some(thread) if thread.is_finished() => {
            drop(slot);
            let _ = thread.join();
            true
        }
        Some(thread) => {
            *slot = Some(thread);
            false
        }
    }
}

/// Waits at most `within` for a worker thread to really end; zero looks exactly once.
fn wait_ended(handle: &Handle, within: Duration) -> bool {
    let give_up = Instant::now() + within;
    loop {
        if ended(handle) {
            return true;
        }
        let left = give_up.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        thread::park_timeout(left.min(LOOK));
    }
}

/// The test's handle on every fake: the call log, and the release and join of the workers.
pub(crate) struct Fakes {
    events: Mutex<Vec<Event>>,
    flags: Mutex<Vec<Arc<Flags>>>,
    workers: Mutex<Vec<(&'static str, Handle)>>,
    /// The `wait_ready` and `await_started` calls, as `wait_ready <name> zero_budget=<bool>`. Kept
    /// apart from `events` so the call lists existing tests assert on do not change.
    waits: Mutex<Vec<String>>,
    /// How many `(poll_ready, poll_started)` calls each fake has had.
    polls: Mutex<HashMap<&'static str, (usize, usize)>>,
}

impl Fakes {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
            flags: Mutex::new(Vec::new()),
            workers: Mutex::new(Vec::new()),
            waits: Mutex::new(Vec::new()),
            polls: Mutex::new(HashMap::new()),
        })
    }

    /// Adds `what` to the call log (a test marks an application step, such as the run closure).
    pub(crate) fn mark(&self, what: &str) {
        self.note(what.to_owned());
    }

    /// The `wait_ready` and `await_started` calls so far, in order.
    pub(crate) fn waits(&self) -> Vec<String> {
        self.waits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// How many `poll_ready` and `poll_started` calls fake `name` has had.
    pub(crate) fn polls_of(&self, name: &str) -> (usize, usize) {
        self.polls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .copied()
            .unwrap_or((0, 0))
    }

    fn note(&self, what: String) {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Event {
                at: Instant::now(),
                what,
            });
    }

    /// Every call so far, in order.
    pub(crate) fn events(&self) -> Vec<Event> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The calls so far as text.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.events().into_iter().map(|event| event.what).collect()
    }

    /// When the call `what` first happened.
    pub(crate) fn when(&self, what: &str) -> Instant {
        self.events()
            .into_iter()
            .find(|event| event.what == what)
            .unwrap_or_else(|| panic!("no call `{what}` in {:?}", self.calls()))
            .at
    }

    fn wait_note(&self, call: &str, name: &str, within: Duration) {
        self.waits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{call} {name} zero_budget={}", within.is_zero()));
    }

    fn handle_of(&self, name: &str) -> Option<Handle> {
        self.workers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(owner, _)| *owner == name)
            .map(|(_, handle)| Arc::clone(handle))
    }

    /// An owner spawn that creates a fake called `name` with `script`.
    pub(crate) fn spawn(self: &Arc<Self>, name: &'static str, script: Script) -> OwnerSpawn {
        let fakes = Arc::clone(self);
        OwnerSpawn::new(name, move || {
            fakes.note(format!("spawn {name}"));
            Ok(Box::new(Fake::start(&fakes, name, script)) as Box<dyn Owner>)
        })
    }

    /// An owner spawn that creates a fake called `name` with `script` and then sets `flag`, as if the
    /// quit request arrived while the owner was being created.
    pub(crate) fn spawn_setting_quit(
        self: &Arc<Self>,
        name: &'static str,
        script: Script,
        flag: &Arc<AtomicBool>,
    ) -> OwnerSpawn {
        let fakes = Arc::clone(self);
        let flag = Arc::clone(flag);
        OwnerSpawn::new(name, move || {
            fakes.note(format!("spawn {name}"));
            let fake = Fake::start(&fakes, name, script);
            flag.store(true, Ordering::SeqCst);
            Ok(Box::new(fake) as Box<dyn Owner>)
        })
    }

    /// An owner spawn that fails with `message`.
    pub(crate) fn spawn_failing(
        self: &Arc<Self>,
        name: &'static str,
        message: &'static str,
    ) -> OwnerSpawn {
        let fakes = Arc::clone(self);
        OwnerSpawn::new(name, move || {
            fakes.note(format!("spawn {name}"));
            Err(message.to_owned())
        })
    }

    /// A service that records `start <name>` and `stop <name>` in the call log.
    pub(crate) fn service(self: &Arc<Self>, name: &'static str) -> Box<dyn Service> {
        Box::new(Svc {
            name,
            fakes: Arc::clone(self),
            sets_quit: None,
            fails_stop: false,
            start_delay: Duration::ZERO,
        })
    }

    /// A service whose start takes `delay` (it waits, as a slow real service would).
    pub(crate) fn slow_service(
        self: &Arc<Self>,
        name: &'static str,
        delay: Duration,
    ) -> Box<dyn Service> {
        Box::new(Svc {
            name,
            fakes: Arc::clone(self),
            sets_quit: None,
            fails_stop: false,
            start_delay: delay,
        })
    }

    /// A service whose stop fails with `<name> refused to stop`.
    pub(crate) fn service_failing_stop(self: &Arc<Self>, name: &'static str) -> Box<dyn Service> {
        Box::new(Svc {
            name,
            fakes: Arc::clone(self),
            sets_quit: None,
            fails_stop: true,
            start_delay: Duration::ZERO,
        })
    }

    /// A service that sets `flag` when it starts.
    pub(crate) fn service_setting_quit(
        self: &Arc<Self>,
        name: &'static str,
        flag: &Arc<AtomicBool>,
    ) -> Box<dyn Service> {
        Box::new(Svc {
            name,
            fakes: Arc::clone(self),
            sets_quit: Some(Arc::clone(flag)),
            fails_stop: false,
            start_delay: Duration::ZERO,
        })
    }

    /// Releases every stuck worker.
    fn release_all(&self) {
        for flags in self
            .flags
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            flags.release();
        }
    }

    /// The names of the workers whose threads have not ended, by one non-waiting look each.
    pub(crate) fn alive_workers(&self) -> Vec<&'static str> {
        let workers: Vec<(&'static str, Handle)> = self
            .workers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        workers
            .into_iter()
            .filter(|(_, handle)| !wait_ended(handle, Duration::ZERO))
            .map(|(name, _)| name)
            .collect()
    }

    /// Releases every worker and joins each, with a bound; the names of those that did not end.
    fn release_and_join(&self) -> Vec<&'static str> {
        self.release_all();
        let workers: Vec<(&'static str, Handle)> = self
            .workers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        workers
            .into_iter()
            .filter(|(_, handle)| !wait_ended(handle, JOIN_BOUND))
            .map(|(name, _)| name)
            .collect()
    }
}

/// Releases and joins every worker, with a bound, when dropped: on a clean exit and after a failed
/// assertion alike. A worker that does not end after its release fails a clean test.
pub(crate) struct Cleanup(pub(crate) Arc<Fakes>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let stuck = self.0.release_and_join();
        // A second panic while unwinding would abort, so only a clean exit asserts.
        if !thread::panicking() {
            assert!(
                stuck.is_empty(),
                "workers did not end after release: {stuck:?}"
            );
        }
    }
}

/// The tracked handle the runtime sees.
struct Fake {
    name: &'static str,
    fakes: Arc<Fakes>,
    request_stop_fails: Option<&'static str>,
    await_cap: Option<Duration>,
    wait_for_sibling: Option<&'static str>,
    control: Sender<Command>,
    replies: Receiver<Reply>,
    handle: Handle,
    flags: Arc<Flags>,
    stopped_reply: bool,
    finished: bool,
    pending_ready_polls: usize,
    pending_started_polls: usize,
    polls_never_complete: NeverCompletes,
    instant_start: bool,
    reply_tx: Sender<Reply>,
    sets_quit: Option<(Arc<AtomicBool>, QuitAt)>,
    /// `(polls, pending polls)` of `poll_ready` and then of `poll_started`.
    ready_polls: (usize, usize),
    started_polls: (usize, usize),
}

impl Fake {
    fn start(fakes: &Arc<Fakes>, name: &'static str, script: Script) -> Self {
        let (control, commands) = mpsc::channel();
        let (replies_tx, replies) = mpsc::channel();
        let flags = Arc::new(Flags {
            state: Mutex::new((false, false)),
            changed: Condvar::new(),
        });
        fakes
            .flags
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::clone(&flags));
        let request_stop_fails = script.request_stop_fails;
        let await_cap = script.await_cap;
        let wait_for_sibling = script.wait_for_sibling;
        let pending_ready_polls = script.pending_ready_polls;
        let pending_started_polls = script.pending_started_polls;
        let polls_never_complete = script.polls_never_complete;
        let instant_start = script.instant_start;
        let reply_tx = replies_tx.clone();
        let sets_quit = script.sets_quit.clone();
        let worker_flags = Arc::clone(&flags);
        let thread = thread::spawn(move || {
            work(&script, &commands, &replies_tx, &worker_flags);
        });
        let handle: Handle = Arc::new(Mutex::new(Some(thread)));
        fakes
            .workers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((name, Arc::clone(&handle)));
        Self {
            name,
            fakes: Arc::clone(fakes),
            request_stop_fails,
            await_cap,
            wait_for_sibling,
            control,
            replies,
            handle,
            flags,
            stopped_reply: false,
            finished: false,
            pending_ready_polls,
            pending_started_polls,
            polls_never_complete,
            instant_start,
            reply_tx,
            sets_quit,
            ready_polls: (0, 0),
            started_polls: (0, 0),
        }
    }

    fn note(&self, call: &str) {
        self.fakes.note(format!("{call} {}", self.name));
    }

    /// The budget an `await_*` really waits: its own, or the script's cap if that is shorter.
    fn budget(&self, within: Duration) -> Duration {
        self.await_cap.map_or(within, |cap| cap.min(within))
    }

    /// Waits at most `within` for a reply; the result of the call it answers.
    fn reply(&self, within: Duration) -> Result<Reply, OwnerError> {
        match self.replies.recv_timeout(within) {
            Ok(Reply::Failed(message)) => Err(OwnerError::Failed { message }),
            Ok(reply) => Ok(reply),
            Err(RecvTimeoutError::Timeout) => Err(OwnerError::Deadline),
            // The worker ended without a reply (it was cancelled while working).
            Err(RecvTimeoutError::Disconnected) => Err(OwnerError::Failed {
                message: format!("{} ended without replying", self.name),
            }),
        }
    }

    /// Sets the quit flag, if the script says to at this moment.
    fn set_quit_at(&self, at: QuitAt) {
        if let Some((flag, when)) = &self.sets_quit
            && *when == at
        {
            self.fakes.note(format!("set quit {}", self.name));
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// Waits at most `within`, giving up early only if the fake is cancelled: the budget of a poll that
    /// has nothing to report is honoured to the end, so the caller reaches its deadline by waiting.
    fn pause(&self, within: Duration) {
        let give_up = Instant::now() + within;
        let mut state = self.flags.lock();
        while !state.1 {
            let left = give_up.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            state = self
                .flags
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// One poll for the reply of the stage: `Pending` after the whole budget when there is none yet.
    fn poll(&mut self, within: Duration, ready: bool) -> Result<Progress, OwnerError> {
        {
            let mut polls = self
                .fakes
                .polls
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let entry = polls.entry(self.name).or_insert((0, 0));
            if ready {
                entry.0 += 1;
            } else {
                entry.1 += 1;
            }
        }
        let (counts, pending_first, never) = if ready {
            (
                &mut self.ready_polls,
                self.pending_ready_polls,
                matches!(
                    self.polls_never_complete,
                    NeverCompletes::Ready | NeverCompletes::Both
                ),
            )
        } else {
            (
                &mut self.started_polls,
                self.pending_started_polls,
                matches!(
                    self.polls_never_complete,
                    NeverCompletes::Started | NeverCompletes::Both
                ),
            )
        };
        counts.0 += 1;
        let gated = never || counts.0 <= pending_first;
        let result = if gated {
            Ok(Progress::Pending)
        } else {
            match self.replies.recv_timeout(within) {
                Ok(Reply::Failed(message)) => Err(OwnerError::Failed { message }),
                Ok(_) => Ok(Progress::Done),
                Err(RecvTimeoutError::Timeout) => Ok(Progress::Pending),
                Err(RecvTimeoutError::Disconnected) => Err(OwnerError::Failed {
                    message: format!("{} ended without replying", self.name),
                }),
            }
        };
        if matches!(result, Ok(Progress::Pending)) {
            if gated {
                self.pause(within);
            }
            let (counts, nth) = if ready {
                (
                    &mut self.ready_polls,
                    QuitAt::ReadyPending as fn(usize) -> QuitAt,
                )
            } else {
                (
                    &mut self.started_polls,
                    QuitAt::StartedPending as fn(usize) -> QuitAt,
                )
            };
            counts.1 += 1;
            let pending = counts.1;
            self.set_quit_at(nth(pending));
        } else {
            self.set_quit_at(if ready {
                QuitAt::ReadyResult
            } else {
                QuitAt::StartedResult
            });
        }
        result
    }

    /// Observes the worker thread really finished within `within` (zero looks once).
    fn observe_finished(&mut self, within: Duration) -> bool {
        self.finished = self.finished || wait_ended(&self.handle, within);
        self.finished
    }
}

impl Owner for Fake {
    fn name(&self) -> &str {
        self.name
    }

    fn wait_ready(&mut self, within: Duration) -> Result<(), OwnerError> {
        self.fakes.wait_note("wait_ready", self.name, within);
        self.reply(within).map(|_| ())
    }

    fn poll_ready(&mut self, within: Duration) -> Result<Progress, OwnerError> {
        self.poll(within, true)
    }

    fn request_start(&mut self) -> Result<(), OwnerError> {
        self.note("request_start");
        self.set_quit_at(QuitAt::RequestStart);
        if self.instant_start {
            let _ = self.reply_tx.send(Reply::Started);
        } else {
            let _ = self.control.send(Command::Start);
        }
        Ok(())
    }

    fn await_started(&mut self, within: Duration) -> Result<(), OwnerError> {
        self.fakes.wait_note("await_started", self.name, within);
        self.reply(self.budget(within)).map(|_| ())
    }

    fn poll_started(&mut self, within: Duration) -> Result<Progress, OwnerError> {
        self.poll(within, false)
    }

    /// Never waits: it notes the call and sends.
    fn request_stop(&mut self) -> Result<(), OwnerError> {
        self.note("request_stop");
        if let Some(message) = self.request_stop_fails {
            return Err(OwnerError::Failed {
                message: message.to_owned(),
            });
        }
        let _ = self.control.send(Command::Stop);
        Ok(())
    }

    fn await_stopped(&mut self, within: Duration) -> Result<(), OwnerError> {
        self.fakes.note(format!(
            "await_stopped {} zero_budget={}",
            self.name,
            within.is_zero()
        ));
        let begun = Instant::now();
        let within = self.budget(within);
        // Where the contract allows waiting (inside the budget), wait for the sibling's thread to
        // have really ended, so a later zero-budget observation of it is deterministic.
        if let Some(sibling) = self.wait_for_sibling
            && let Some(handle) = self.fakes.handle_of(sibling)
        {
            let _ = wait_ended(&handle, within);
        }
        if !self.stopped_reply {
            self.reply(within.saturating_sub(begun.elapsed()))?;
            self.stopped_reply = true;
        }
        // A reply alone is not completion: the thread must be observed finished, within the budget.
        if self.observe_finished(within.saturating_sub(begun.elapsed())) {
            Ok(())
        } else {
            Err(OwnerError::Deadline)
        }
    }

    /// Never waits: it notes the call, sets the flag and sends.
    fn cancel(&mut self) {
        self.note("cancel");
        self.flags.cancel();
        let _ = self.control.send(Command::Cancel);
    }

    fn wait_finished(&mut self, within: Duration) -> bool {
        let finished = self.observe_finished(within);
        self.note(if finished {
            "observed_finished"
        } else {
            "not_finished"
        });
        finished
    }
}

/// How a step ended.
enum Ended {
    Done,
    Failed(&'static str),
    /// Released by the test or cancelled: the worker just ends.
    Gone,
}

/// Blocks until released (or, with `ends_on_cancel`, cancelled), or [`HOLD`] has passed.
fn stay_stuck(ends_on_cancel: bool, flags: &Flags) {
    let give_up = Instant::now() + HOLD;
    let mut state = flags.lock();
    while !(state.0 || ends_on_cancel && state.1) {
        let left = give_up.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        state = flags
            .changed
            .wait_timeout(state, left)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

fn run_step(step: Step, ends_on_cancel: bool, flags: &Flags) -> Ended {
    match step {
        Step::Done => Ended::Done,
        Step::Fail(message) | Step::FailThenStuck(message) => Ended::Failed(message),
        Step::Stuck => {
            stay_stuck(ends_on_cancel, flags);
            Ended::Gone
        }
    }
}

/// The worker's life: initialise, then serve `Start` and `Stop` until the end.
fn work(script: &Script, commands: &Receiver<Command>, replies: &Sender<Reply>, flags: &Flags) {
    match run_step(script.ready, script.ends_on_cancel, flags) {
        Ended::Done => {
            let _ = replies.send(Reply::Ready);
        }
        Ended::Failed(message) => {
            let _ = replies.send(Reply::Failed(message.to_owned()));
            return;
        }
        Ended::Gone => return,
    }
    while let Ok(command) = commands.recv_timeout(HOLD) {
        match command {
            Command::Cancel => {
                if script.ignores_cancel_when_idle {
                    stay_stuck(false, flags);
                }
                return;
            }
            Command::Start => {
                if !meets(script.start_meeting.as_ref(), replies) {
                    return;
                }
                match run_step(script.start, script.ends_on_cancel, flags) {
                    Ended::Done => {
                        let _ = replies.send(Reply::Started);
                    }
                    Ended::Failed(message) => {
                        let _ = replies.send(Reply::Failed(message.to_owned()));
                        return;
                    }
                    Ended::Gone => return,
                }
            }
            Command::Stop => {
                if !meets(script.stop_meeting.as_ref(), replies) {
                    return;
                }
                match run_step(script.stop, script.ends_on_cancel, flags) {
                    Ended::Done => {
                        let _ = replies.send(Reply::Stopped);
                    }
                    Ended::Failed(message) => {
                        let _ = replies.send(Reply::Failed(message.to_owned()));
                        if matches!(script.stop, Step::FailThenStuck(_)) {
                            stay_stuck(false, flags);
                        }
                    }
                    Ended::Gone => {}
                }
                return;
            }
        }
    }
}

/// Waits at the rendezvous, if there is one; a worker that is left waiting reports the failure.
fn meets(meeting: Option<&Arc<Rendezvous>>, replies: &Sender<Reply>) -> bool {
    match meeting {
        Some(meeting) if !meeting.meet() => {
            let _ = replies.send(Reply::Failed("the other request never arrived".to_owned()));
            false
        }
        _ => true,
    }
}

/// A service that logs its calls next to the owners', so their order can be asserted together.
struct Svc {
    name: &'static str,
    fakes: Arc<Fakes>,
    sets_quit: Option<Arc<AtomicBool>>,
    fails_stop: bool,
    start_delay: Duration,
}

impl Service for Svc {
    fn name(&self) -> &str {
        self.name
    }

    fn start(&mut self) -> Result<(), ServiceError> {
        self.fakes.note(format!("start {}", self.name));
        thread::sleep(self.start_delay);
        if let Some(flag) = &self.sets_quit {
            flag.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), ServiceError> {
        self.fakes.note(format!("stop {}", self.name));
        if self.fails_stop {
            return Err(ServiceError::new(format!("{} refused to stop", self.name)));
        }
        Ok(())
    }
}
