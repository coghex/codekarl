//! The logger: a bounded queue, one writer thread and a sink.
//!
//! # Concurrency design
//!
//! Records are formatted in the calling thread and handed, whole, to one writer thread through a
//! bounded queue (`Mutex<Queue>`, with a *space* condition variable on the same mutex). The writer
//! is the only code that writes to the sink; it holds the queue lock only to swap the queued
//! records out, never during sink I/O, and notifies every space waiter after each take.
//!
//! There are three ways in:
//!
//! - [`Logger::log`] is **lossless and may wait**. It reads the settings and takes the queue lock
//!   with blocking calls (each first tries without waiting, so that a test can observe the
//!   collision), and when the queue is full and open it waits on the space condition variable,
//!   rechecking capacity and closure after every wake. It has no timeout: if the sink never makes
//!   progress, the emitter waits, as a program writing to a full pipe does. It drops a record only
//!   if the queue is closed (the sink failed), or the category is invalid or the message does not
//!   format.
//! - [`Logger::log_within`] waits like `log` but never past a deadline. It never calls a blocking
//!   lock, a blocking settings read or a condition-variable wait (`Condvar::wait_timeout` would
//!   re-acquire the queue mutex without a bound): it polls with `try_read` and `try_lock`, and on a
//!   full queue it releases the lock and parks for at most 1 ms. It returns at most about one
//!   attempt plus one park slice after its deadline whatever any other thread holds, and it can
//!   drop whenever admission cannot complete by then.
//! - [`Logger::try_log`] waits not at all: one `try_read`, a bounded number of `try_lock`
//!   attempts, and an immediate drop if the queue is full or closed. It can drop under ordinary
//!   lock contention.
//!
//! **Control-path rule.** Code that owns deadlines, cancellation or the process exit (a
//! supervisor, a runtime's lifecycle, the executable's main thread, the panic hook) must emit every
//! record, ordinary progress and failure records included, through `log_within` (or `try_log`
//! where no wait at all is acceptable), never through the waiting `log`: a full queue behind a
//! stalled sink would otherwise stall the code that is supposed to enforce the deadlines. The
//! deadline is the earlier of a preferred allowance (100 ms unless the caller configures another)
//! and the code's own next deadline, never extended to make room for logging; a deadline that has
//! already passed means one non-waiting attempt per phase. Ordinary producers (services, Lua VMs,
//! application code) use the waiting `log`.
//!
//! ## Closed state
//!
//! When the writer ends for any reason (a failed sink write or flush, a panic in the sink, or a
//! stop), it sets `closed` under the queue mutex and notifies **all** space waiters. A waiter that
//! finds the queue closed returns without enqueueing and counts its record as dropped once; every
//! later caller of any entry point finds it closed and drops and counts at once. Records queued or
//! in flight when the sink failed are counted once by the writer. A record is never counted twice.
//!
//! ## Errors are reserved
//!
//! Progress records may be dropped under load; **error records are reserved**. A [`Level::Error`]
//! record given to [`Logger::log_within`] never reads the filter (every filter admits an error) and
//! never takes the settings lock: it is formatted with a retained snapshot of the two format flags,
//! which `set_settings` updates after it stores the new settings. If normal admission fails by the
//! deadline (the queue is full, or its lock is not acquired in time; an expired deadline makes one
//! normal attempt first), the record goes to a separate **reserve** of [`RESERVE_SLOTS`] slots and
//! the call returns [`Admission::Queued`].
//!
//! - **Publication never waits.** A producer try-locks the reserve's slots, one after another from
//!   a rotating start, and publishes into the first free one. A contended or occupied slot is
//!   skipped. It never blocks, and never takes the queue lock or the settings lock. The record is
//!   dropped and counted only if no slot can be taken: all 1024 hold unwritten records, or every
//!   free slot is contended at once.
//! - **Closure.** A producer first marks a publication in progress (an atomic count), then checks
//!   the reserve's closed flag; if closed it unmarks and drops and counts the record, otherwise it
//!   publishes and then unmarks. When the writer ends it sets the flag, then *waits* until no
//!   publication is in progress (only the writer ever waits), then drains the reserve for its final
//!   count. A record that returned `Queued` is therefore either written or counted dropped exactly
//!   once.
//! - **Order.** Before it takes each new batch from the queue, and before it answers a flush ticket,
//!   the writer takes and writes every reserved record, oldest (by sequence number) first. A batch
//!   already taken is written to completion first. For ordinary records A and B and a reserved error
//!   E the output can therefore be E, A, B or A, E, B or A, B, E. Records admitted normally keep
//!   their relative order; the reserved errors are the exception.
//!
//! ## Error delivery
//!
//! Each error-level record receives a unique **sequence number** from an atomic counter at the
//! moment its admission is committed: while the queue lock is held for the push, or while the
//! reserve slot is held after the closed check passed. A refused record never has a number. The
//! number travels with the record. The writer marks it **delivered** only after the record's write
//! and the sink flush that follows it both succeeded (it flushes right after writing any record that
//! carries a number). It keeps the *contiguous delivered watermark* W, the largest n such that every
//! number 1..=n is delivered, and a set of numbers delivered out of order (a reserved error can
//! overtake an earlier error that is still in the queue) that is emptied as W passes them.
//!
//! **Bound on that set.** A number enters it only while a smaller number is undelivered; let P be
//! the smallest undelivered one. A reserve pass collects *every* slot first, sorts what it found by
//! number, writes and flushes it, and marks it delivered in one step; a queue batch (at most
//! `capacity` records, in number order) is written, flushed and marked the same way. Each writer
//! iteration is one reserve pass followed by one queue batch. The numbers above P that are
//! delivered before P itself are:
//!
//! - P is in the ordinary queue: the one reserve pass of the iteration that takes P, at most
//!   [`RESERVE_SLOTS`] numbers. P's own batch is marked together with P.
//! - P is in the reserve, stored just after the pass went by its slot: the other slots that pass
//!   collected (at most `RESERVE_SLOTS - 1`, the slot P sits in being the one it missed), then the
//!   batch of the same iteration (at most `capacity`). The next pass collects P, since a number is
//!   taken and stored under one slot lock that the writer waits for, and marks it with everything
//!   else it finds, so P is not missed twice.
//! - P was assigned during the iteration's reserve write or its batch: at most that batch.
//!
//! The set therefore never holds more than `RESERVE_SLOTS - 1 + capacity` numbers (1027 at capacity
//! 4); it is emptied as the watermark passes them. The 1024 slots alone would not bound it, and
//! the larger of the two alone is not enough (the second case adds them).
//!
//! [`Logger::await_errors`] snapshots the highest number given out and waits until W reaches it.
//!
//! ## Flush
//!
//! Every `flush` call is a *request*: it draws a ticket, and the writer, after it has written and
//! drained everything it has taken, flushes the sink once more and publishes the highest ticket
//! that flush covers. `flush` is `Complete` only when its own ticket is covered by such a
//! successful sink flush (so a logger that never emitted, or filtered everything, still flushes its
//! sink), nothing is queued or in flight, the sink never failed and no record was ever dropped.
//! The caller never touches the sink, and `flush` is bounded by its own deadline even while
//! emitters are waiting for space: it waits on a separate gate lock, which no emitter takes.
//!
//! ## Outstanding-work accounting
//!
//! `queued` counts records admitted but not yet taken by the writer; `in_flight` is true from the
//! moment the writer takes records (it is set under the queue lock, *before* `queued` is reduced)
//! until the records have been written and the sink flushed. A reader therefore loads `queued`
//! first and `in_flight` second: if it sees `queued == 0` because the writer took a batch, the
//! earlier `in_flight` store is visible too, so a batch that is still unwritten is never mistaken
//! for completed work.
//!
//! The queue's capacity is a logical bound kept in `capacity`; it is compared with the queue's
//! `len()`, never its allocation, so swapping buffers does not change it.

use std::collections::{BTreeSet, VecDeque};
use std::fmt::{self, Write as _};
use std::io::{self, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock, TryLockError};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant, SystemTime};

use crate::filter::is_valid_category;
use crate::{Filter, Level, format, unwind};

/// The default queue capacity, in records.
const DEFAULT_CAPACITY: usize = 4096;
/// How many error records the reserve holds at most.
pub const RESERVE_SLOTS: usize = 1024;
/// How many times `try_log` tries the queue lock before it drops its record.
const ADMISSION_ATTEMPTS: u32 = 64;
/// How many attempts `log_within` spins before it starts to yield between attempts.
const SPIN_ATTEMPTS: u32 = 64;
/// The longest `log_within` parks between looks at a full queue.
const SPACE_POLL: Duration = Duration::from_millis(1);
/// How long an idle writer waits before it looks again (a missed wake-up only delays it).
const IDLE_WAIT: Duration = Duration::from_millis(50);
/// How long dropping a logger waits for an idle writer thread to finish.
const EXIT_GRACE: Duration = Duration::from_millis(50);
/// How often dropping a logger looks at whether the writer thread has finished.
const EXIT_POLL: Duration = Duration::from_micros(200);
/// The longest single wait of a flush, so that an enormous deadline cannot overflow a timer.
const LONGEST_WAIT: Duration = Duration::from_secs(1);

/// What a logger does with the records it is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Which records are emitted.
    pub filter: Filter,
    /// Whether each line starts with a UTC timestamp.
    pub timestamps: bool,
    /// Whether the level is wrapped in an ANSI colour (see [`ColourMode`](crate::ColourMode)).
    pub colour: bool,
}

/// How a [`Logger::flush`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlushOutcome {
    /// Every record admitted so far was written and the sink flushed, none was ever dropped and the
    /// sink never failed.
    Complete,
    /// The writer did not finish in time, or records were lost.
    Incomplete {
        /// Records admitted but not yet taken by the writer.
        queued: usize,
        /// Whether the writer was inside a write or flush of the sink.
        in_flight: bool,
        /// Every record dropped since the logger started (never reset).
        dropped: u64,
        /// The kind of the first sink error, if the sink failed.
        sink_error: Option<io::ErrorKind>,
    },
}

/// What `try_log` and `log_within` did with a record.
///
/// Only a [`Dropped`](Self::Dropped) record is lost, and every loss is counted: it makes the final
/// flush `Incomplete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Admission {
    /// The record was queued for the writer.
    Queued,
    /// The filter rejected the record. Nothing is lost and nothing is counted.
    Filtered,
    /// The record was dropped and counted: it could not be admitted in time, the queue is closed,
    /// its category is invalid or its message failed to format.
    Dropped,
}

/// One admitted record: its formatted line and, for an error-level record, its delivery sequence
/// number (0 for every other level).
pub(crate) struct Entry {
    line: String,
    seq: u64,
}

/// One reserve slot: empty, or holding an error record that the writer has not yet taken.
type Slot = Mutex<Option<Entry>>;

/// The queue and its closed flag, both protected by [`Shared::queue`].
struct Queue {
    records: VecDeque<Entry>,
    /// The writer has gone: every record is dropped and counted.
    closed: bool,
}

/// State shared by the logger handle and its writer thread.
struct Shared {
    settings: RwLock<Arc<Settings>>,
    queue: Mutex<Queue>,
    /// Notified (with the queue mutex) when the writer has taken records or the queue has closed.
    space: Condvar,
    capacity: usize,
    /// Asks the writer to drain the queue and exit.
    stop: AtomicBool,
    queued: AtomicUsize,
    in_flight: AtomicBool,
    dropped_total: AtomicU64,
    dropped_pending: AtomicU64,
    /// Tickets drawn by `flush` calls; the writer's next sink flush covers every ticket drawn so far.
    flush_requested: AtomicU64,
    gate: Mutex<Gate>,
    changed: Condvar,
    /// The reserve: [`RESERVE_SLOTS`] slots, each try-locked by producers (never waited for).
    reserve: Box<[Slot]>,
    /// Where the next publication starts looking for a free slot.
    reserve_cursor: AtomicUsize,
    /// Records published into the reserve and not yet taken by the writer.
    reserved: AtomicUsize,
    /// Publications in progress (marked before the closed check, unmarked after the publish).
    publishing: AtomicUsize,
    /// The reserve takes no more records: the writer is ending.
    reserve_closed: AtomicBool,
    /// The highest error sequence number given out.
    next_seq: AtomicU64,
    /// The retained format flags that an error record uses instead of the settings lock.
    flag_timestamps: AtomicBool,
    flag_colour: AtomicBool,
    /// Where tests receive the emitter-side rendezvous events (see [`Probe`]).
    #[cfg(test)]
    probes: Mutex<Option<std::sync::mpsc::Sender<Probe>>>,
    /// Set by a failing test so that no emitter waits for space any more (see `rescue_waiters`).
    #[cfg(test)]
    rescued: AtomicBool,
    /// Called at the reserve's interleaving points, so that a test can pause a producer there.
    #[cfg(test)]
    pauses: Mutex<Option<PauseHook>>,
}

/// What a test installs to pause a reserve producer at its interleaving points.
#[cfg(test)]
type PauseHook = Arc<dyn Fn(ReservePoint) + Send + Sync>;

/// A point in the reserve's publication where a test can pause the producer.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReservePoint {
    /// The publication is marked in progress; the closed flag has not been read yet.
    Marked,
    /// The closed flag was read and was clear; no slot has been taken yet.
    Checked,
    /// The record is in its slot (the slot lock is released); the mark has not been removed yet.
    Stored,
    /// The writer's reserve scan has looked at this slot (its lock is released) and has not moved on.
    Scanned(usize),
}

/// A point in an emitter's path that a test can wait for, so that it never relies on a sleep or a
/// snapshot of a flag.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Probe {
    /// `log` found the settings lock held and is about to wait for it.
    SettingsContended,
    /// `log` found the queue lock held and is about to wait for it.
    QueueContended,
    /// `log` found the queue full and is about to wait for space (it holds the queue lock).
    SpaceWait(thread::ThreadId),
    /// `log_within` found the queue full, released the lock and is about to park.
    SpacePoll,
    /// `log_within` tries to read the settings.
    SettingsAttempt,
    /// `log_within` tries to take the queue lock.
    QueueAttempt,
    /// `await_errors` has taken its target and is about to wait for the watermark.
    AwaitWait,
}

/// What `flush` and the writer coordinate on, under [`Shared::gate`].
struct Gate {
    sink_error: Option<io::ErrorKind>,
    /// The writer thread's body has finished (its sink may still be dropping).
    exited: bool,
    /// The highest `flush` ticket covered by a successful sink flush.
    flushed: u64,
    /// The contiguous delivered watermark: every error number up to it is written and flushed.
    watermark: u64,
    /// Error numbers delivered ahead of the watermark (a smaller one is still undelivered).
    ahead: BTreeSet<u64>,
    /// The most numbers `ahead` ever held (read by tests, to check the bound).
    #[cfg(test)]
    ahead_peak: usize,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Repeats `attempt` until it yields a value or `deadline` has passed: spinning for the first
/// [`SPIN_ATTEMPTS`] attempts and yielding after that. A deadline at or before the call allows
/// exactly one attempt. It never waits on anything but the clock.
fn poll<T>(deadline: Instant, mut attempt: impl FnMut() -> Option<T>) -> Option<T> {
    let mut attempts = 0_u32;
    loop {
        if let Some(found) = attempt() {
            return Some(found);
        }
        if Instant::now() >= deadline {
            return None;
        }
        attempts = attempts.saturating_add(1);
        if attempts < SPIN_ATTEMPTS {
            std::hint::spin_loop();
        } else {
            thread::yield_now();
        }
    }
}

/// What formatting a record produced.
enum Prepared {
    /// The formatted line, ready to queue.
    Line(String),
    /// The filter rejected the record.
    Filtered,
    /// The category is invalid or the message failed to format: the record is a counted loss.
    Invalid,
}

/// Filters and formats one record in the calling thread, taking the timestamp now.
fn prepare(
    settings: &Settings,
    level: Level,
    category: &str,
    message: &dyn fmt::Display,
) -> Prepared {
    if !settings.filter.allows(level, category) {
        return Prepared::Filtered;
    }
    render(
        settings.timestamps,
        settings.colour,
        level,
        category,
        message,
    )
}

/// Formats one record in the calling thread, taking the timestamp now; the filter has already
/// admitted it.
fn render(
    timestamps: bool,
    colour: bool,
    level: Level,
    category: &str,
    message: &dyn fmt::Display,
) -> Prepared {
    let mut text = String::new();
    if !is_valid_category(category) || write!(text, "{message}").is_err() {
        return Prepared::Invalid;
    }
    let time = timestamps.then(SystemTime::now);
    Prepared::Line(format::line(level, category, &text, time, colour))
}

impl Shared {
    fn count_dropped(&self, records: u64) {
        self.dropped_pending.fetch_add(records, Ordering::SeqCst);
        self.dropped_total.fetch_add(records, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn probe(&self, event: Probe) {
        if let Some(sender) = locked(&self.probes).as_ref() {
            let _ = sender.send(event);
        }
    }

    /// Wakes `flush` callers and `Logger::drop`. Takes the gate so that a waiter that has read the
    /// counters cannot miss the change.
    fn wake_waiters(&self) {
        let _gate = locked(&self.gate);
        self.changed.notify_all();
    }

    /// Records that a successful sink flush covers every `flush` ticket up to `requested`.
    fn publish_flushed(&self, requested: u64) {
        let mut gate = locked(&self.gate);
        gate.flushed = gate.flushed.max(requested);
        self.changed.notify_all();
    }

    /// The current settings; may wait for a settings swap, so only the writer calls it.
    fn current_settings(&self) -> Arc<Settings> {
        let guard = self.settings.read().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(&guard)
    }

    /// The settings for `log`: tries without waiting, and waits only if a swap is in progress.
    fn read_settings(&self) -> Arc<Settings> {
        match self.settings.try_read() {
            Ok(guard) => Arc::clone(&guard),
            Err(TryLockError::Poisoned(poisoned)) => Arc::clone(&poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => {
                #[cfg(test)]
                self.probe(Probe::SettingsContended);
                self.current_settings()
            }
        }
    }

    /// Pushes `line`; the caller holds the queue lock and has checked closure and capacity. An error
    /// record receives its sequence number here, while the lock is held, so a refused record never
    /// has one.
    fn push(&self, queue: &mut Queue, line: String, level: Level) {
        let seq = if level == Level::Error {
            self.next_seq.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            0
        };
        queue.records.push_back(Entry { line, seq });
        self.queued.fetch_add(1, Ordering::SeqCst);
    }

    /// Admits `line` for `log`: waits for the queue lock, and for space while the queue is full and
    /// open, without a timeout. Returns `false` if the queue is closed.
    fn admit_waiting(&self, line: String, level: Level) -> bool {
        let mut queue = match self.queue.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                #[cfg(test)]
                self.probe(Probe::QueueContended);
                locked(&self.queue)
            }
        };
        loop {
            if queue.closed {
                return false;
            }
            if queue.records.len() < self.capacity {
                self.push(&mut queue, line, level);
                return true;
            }
            // Every wake, spurious or not, comes back to the checks above.
            #[cfg(test)]
            if self.rescued.load(Ordering::SeqCst) {
                return false;
            }
            #[cfg(test)]
            self.probe(Probe::SpaceWait(thread::current().id()));
            queue = self
                .space
                .wait(queue)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Admits `line` for `try_log`: a bounded number of `try_lock` attempts, and an immediate drop
    /// if the queue is full or closed.
    fn admit_try(&self, line: String, level: Level) -> bool {
        for _ in 0..ADMISSION_ATTEMPTS {
            let mut queue = match self.queue.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => {
                    std::hint::spin_loop();
                    continue;
                }
            };
            if queue.closed || queue.records.len() >= self.capacity {
                return false;
            }
            self.push(&mut queue, line, level);
            return true;
        }
        false
    }

    // BEGIN non-waiting admission: the code between these markers must not call a blocking lock,
    // a blocking settings read or a condition-variable wait (a structural test checks this).

    /// The settings for `log_within`: `try_read` until `deadline`.
    fn read_settings_within(&self, deadline: Instant) -> Option<Arc<Settings>> {
        poll(deadline, || {
            #[cfg(test)]
            self.probe(Probe::SettingsAttempt);
            match self.settings.try_read() {
                Ok(guard) => Some(Arc::clone(&guard)),
                Err(TryLockError::Poisoned(poisoned)) => Some(Arc::clone(&poisoned.into_inner())),
                Err(TryLockError::WouldBlock) => None,
            }
        })
    }

    /// Admits `line` for `log_within`: `try_lock` until `deadline`; on a full open queue it releases
    /// the lock and parks for at most [`SPACE_POLL`] (or the time left) before it looks again. A
    /// record that is not admitted is handed back, for the reserve.
    fn admit_within(&self, line: String, level: Level, deadline: Instant) -> Result<(), String> {
        loop {
            let queue = poll(deadline, || {
                #[cfg(test)]
                self.probe(Probe::QueueAttempt);
                match self.queue.try_lock() {
                    Ok(guard) => Some(guard),
                    Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
                    Err(TryLockError::WouldBlock) => None,
                }
            });
            let Some(mut queue) = queue else {
                return Err(line);
            };
            if queue.closed {
                return Err(line);
            }
            if queue.records.len() < self.capacity {
                self.push(&mut queue, line, level);
                return Ok(());
            }
            drop(queue);
            #[cfg(test)]
            self.probe(Probe::SpacePoll);
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(line);
            }
            thread::park_timeout(left.min(SPACE_POLL));
        }
    }

    // END non-waiting admission

    // BEGIN non-waiting reserve publication: the code between these markers must not call a
    // blocking lock or a condition-variable wait (a structural test checks this).

    /// Publishes an error record into the reserve without waiting: marks a publication in progress,
    /// then checks the closed flag, then takes the first free slot it can try-lock. Returns `false`
    /// (the caller counts the loss) if the reserve is closed or no slot can be taken.
    fn publish_reserved(&self, line: String) -> bool {
        self.publishing.fetch_add(1, Ordering::SeqCst);
        #[cfg(test)]
        self.pause(ReservePoint::Marked);
        let admitted = !self.reserve_closed.load(Ordering::SeqCst) && {
            #[cfg(test)]
            self.pause(ReservePoint::Checked);
            self.fill_slot(line)
        };
        #[cfg(test)]
        self.pause(ReservePoint::Stored);
        self.publishing.fetch_sub(1, Ordering::SeqCst);
        admitted
    }

    /// Stores `line` in the first free slot that can be try-locked, starting from a rotating slot.
    /// The number is taken, and both counts raised, while the slot is held, so the writer (which
    /// locks the slot to take the record) sees them all.
    fn fill_slot(&self, line: String) -> bool {
        let start = self.reserve_cursor.fetch_add(1, Ordering::Relaxed) % RESERVE_SLOTS;
        for offset in 0..RESERVE_SLOTS {
            let slot = &self.reserve[(start + offset) % RESERVE_SLOTS];
            let mut held = match slot.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => continue,
            };
            if held.is_some() {
                continue;
            }
            let seq = self.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
            self.queued.fetch_add(1, Ordering::SeqCst);
            self.reserved.fetch_add(1, Ordering::SeqCst);
            *held = Some(Entry { line, seq });
            return true;
        }
        false
    }

    // END non-waiting reserve publication

    #[cfg(test)]
    fn pause(&self, point: ReservePoint) {
        let hook = locked(&self.pauses).clone();
        if let Some(hook) = hook {
            hook(point);
        }
    }

    /// Takes every record out of the reserve, locking each slot, into `taken`.
    fn drain_reserve(&self, taken: &mut VecDeque<Entry>) -> usize {
        let mut count = 0;
        let mut found = Vec::new();
        for (index, slot) in self.reserve.iter().enumerate() {
            if let Some(entry) = locked(slot).take() {
                found.push(entry);
                count += 1;
            }
            #[cfg(test)]
            self.pause(ReservePoint::Scanned(index));
            #[cfg(not(test))]
            let _ = index;
        }
        found.sort_by_key(|entry| entry.seq);
        taken.extend(found);
        count
    }

    /// Records that these error numbers were written and the sink flushed: advances the watermark
    /// over them and every number that was waiting behind them, and keeps the rest in the set.
    fn mark_delivered(&self, written: &mut Vec<u64>) {
        if written.is_empty() {
            return;
        }
        written.sort_unstable();
        let mut gate = locked(&self.gate);
        for number in written.drain(..) {
            if number == gate.watermark + 1 {
                gate.watermark = number;
                loop {
                    let next = gate.watermark + 1;
                    if !gate.ahead.remove(&next) {
                        break;
                    }
                    gate.watermark = next;
                }
            } else {
                gate.ahead.insert(number);
            }
        }
        #[cfg(test)]
        {
            gate.ahead_peak = gate.ahead_peak.max(gate.ahead.len());
        }
        self.changed.notify_all();
    }
}

/// A logger with one writer thread and a bounded queue.
///
/// [`log`](Self::log) never loses a record to contention or to a full open queue, but it may wait;
/// [`log_within`](Self::log_within) and [`try_log`](Self::try_log) are the bounded and the
/// non-waiting ways in. See the [crate documentation](crate) for the guarantees, the control-path
/// rule and their scope.
pub struct Logger {
    shared: Arc<Shared>,
    thread: Thread,
    handle: Option<JoinHandle<()>>,
}

impl fmt::Debug for Logger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Logger")
            .field("capacity", &self.shared.capacity)
            .finish_non_exhaustive()
    }
}

impl Logger {
    /// Starts a logger with the default queue capacity of 4096 records.
    ///
    /// # Errors
    ///
    /// Returns the error from the operating system when the writer thread cannot be started.
    pub fn new(sink: Box<dyn Write + Send>, settings: Settings) -> io::Result<Self> {
        Self::with_capacity(sink, settings, DEFAULT_CAPACITY)
    }

    /// Starts a logger whose queue holds `capacity` records (a capacity of 0 is raised to 1).
    ///
    /// # Errors
    ///
    /// Returns the error from the operating system when the writer thread cannot be started.
    pub fn with_capacity(
        sink: Box<dyn Write + Send>,
        settings: Settings,
        capacity: usize,
    ) -> io::Result<Self> {
        Self::start(sink, settings, capacity, writer_main)
    }

    /// Starts a logger whose writer thread runs `body`: always [`writer_main`], except in the one test
    /// that shows what the thread did without its catch.
    fn start(
        sink: Box<dyn Write + Send>,
        settings: Settings,
        capacity: usize,
        body: fn(&Shared, Box<dyn Write + Send>),
    ) -> io::Result<Self> {
        let capacity = capacity.max(1);
        let (timestamps, colour) = (settings.timestamps, settings.colour);
        let shared = Arc::new(Shared {
            settings: RwLock::new(Arc::new(settings)),
            queue: Mutex::new(Queue {
                records: VecDeque::with_capacity(capacity.min(1024)),
                closed: false,
            }),
            space: Condvar::new(),
            capacity,
            stop: AtomicBool::new(false),
            queued: AtomicUsize::new(0),
            in_flight: AtomicBool::new(false),
            dropped_total: AtomicU64::new(0),
            dropped_pending: AtomicU64::new(0),
            flush_requested: AtomicU64::new(0),
            gate: Mutex::new(Gate {
                sink_error: None,
                exited: false,
                flushed: 0,
                watermark: 0,
                ahead: BTreeSet::new(),
                #[cfg(test)]
                ahead_peak: 0,
            }),
            changed: Condvar::new(),
            reserve: (0..RESERVE_SLOTS).map(|_| Mutex::new(None)).collect(),
            reserve_cursor: AtomicUsize::new(0),
            reserved: AtomicUsize::new(0),
            publishing: AtomicUsize::new(0),
            reserve_closed: AtomicBool::new(false),
            next_seq: AtomicU64::new(0),
            flag_timestamps: AtomicBool::new(timestamps),
            flag_colour: AtomicBool::new(colour),
            #[cfg(test)]
            probes: Mutex::new(None),
            #[cfg(test)]
            rescued: AtomicBool::new(false),
            #[cfg(test)]
            pauses: Mutex::new(None),
        });
        let writer = Arc::clone(&shared);
        let handle = thread::Builder::new()
            .name("ck-log-writer".to_owned())
            .spawn(move || body(&writer, sink))?;
        Ok(Self {
            shared,
            thread: handle.thread().clone(),
            handle: Some(handle),
        })
    }

    /// Replaces the filter and format for records emitted afterwards.
    ///
    /// The settings' write lock is held only to swap one `Arc`.
    pub fn set_settings(&self, settings: Settings) {
        let new = Arc::new(settings);
        let (timestamps, colour) = (new.timestamps, new.colour);
        let old = {
            let mut current = self
                .shared
                .settings
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            std::mem::replace(&mut *current, new)
        };
        drop(old);
        // After the swap, so an error record that reads them sees either the old or the new pair.
        self.shared
            .flag_timestamps
            .store(timestamps, Ordering::SeqCst);
        self.shared.flag_colour.store(colour, Ordering::SeqCst);
    }

    /// Emits one record, losslessly: it may wait, but contention and a full open queue never drop it.
    ///
    /// The record is filtered and formatted in the calling thread, with the timestamp taken now. The
    /// settings and the queue lock are taken with blocking calls (held only to read an `Arc` or to
    /// push), and when the queue is full the call waits, without a timeout, until the writer has
    /// taken records. If the sink never makes progress the call waits indefinitely, as a write to a
    /// full pipe does: code that owns deadlines must use [`log_within`](Self::log_within) instead
    /// (see the crate documentation).
    ///
    /// The record is lost, and counted, only if the queue has closed because the sink failed, if its
    /// `category` is not valid (see [`is_valid_category`]) or if its `message` fails to format.
    pub fn log(&self, level: Level, category: &str, message: impl fmt::Display) {
        let shared = &*self.shared;
        let settings = shared.read_settings();
        match prepare(&settings, level, category, &message) {
            Prepared::Filtered => {}
            Prepared::Invalid => shared.count_dropped(1),
            Prepared::Line(line) => {
                if shared.admit_waiting(line, level) {
                    self.thread.unpark();
                } else {
                    shared.count_dropped(1);
                }
            }
        }
    }

    /// Emits one record, waiting like [`log`](Self::log) but never past `deadline`.
    ///
    /// It never calls a blocking lock, a blocking settings read or a condition-variable wait. It
    /// tries the settings and the queue lock without waiting (spinning, then yielding) until the
    /// deadline, and on a full open queue releases the lock and parks for at most 1 ms (or the time
    /// left) before it looks again. A deadline at or before the call allows one attempt per phase.
    /// It returns at most about one attempt plus one 1 ms park after the deadline, whatever any
    /// other thread holds. Caller formatting and operating-system scheduling are outside that bound.
    ///
    /// **It can drop** whenever admission cannot complete by the deadline: lock contention lasting
    /// past it, a queue that stays full (or whose freed slots other producers keep taking) while
    /// the output is healthy but slower than the producers, a stalled sink, or a closed queue. Every
    /// drop is counted and makes the final flush `Incomplete`. Code on a control path uses this,
    /// never the waiting `log` (see the crate documentation).
    ///
    /// **An error record is reserved.** At [`Level::Error`] the call reads no filter and takes no
    /// settings lock, and when normal admission fails by the deadline the record goes to the
    /// reserve and the result is `Queued`; it is lost and counted only if all [`RESERVE_SLOTS`]
    /// slots are occupied or every free slot is contended, or the logger is closed. See the module
    /// documentation.
    pub fn log_within(
        &self,
        level: Level,
        category: &str,
        message: impl fmt::Display,
        deadline: Instant,
    ) -> Admission {
        let shared = &*self.shared;
        if level == Level::Error {
            return self.log_error_within(category, &message, deadline);
        }
        let Some(settings) = shared.read_settings_within(deadline) else {
            shared.count_dropped(1);
            return Admission::Dropped;
        };
        match prepare(&settings, level, category, &message) {
            Prepared::Filtered => Admission::Filtered,
            Prepared::Invalid => {
                shared.count_dropped(1);
                Admission::Dropped
            }
            Prepared::Line(line) => self.finish(shared.admit_within(line, level, deadline).is_ok()),
        }
    }

    /// Emits one record without waiting at all: one `try_read` of the settings, a bounded number of
    /// `try_lock` attempts, and an immediate drop if the queue is full or closed.
    ///
    /// **It can drop under ordinary lock contention**, so it is only for code that cannot wait even
    /// briefly; a control path uses [`log_within`](Self::log_within). Every drop is counted.
    pub fn try_log(&self, level: Level, category: &str, message: impl fmt::Display) -> Admission {
        let shared = &*self.shared;
        let settings = match shared.settings.try_read() {
            Ok(guard) => Arc::clone(&guard),
            Err(TryLockError::Poisoned(poisoned)) => Arc::clone(&poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => {
                shared.count_dropped(1);
                return Admission::Dropped;
            }
        };
        match prepare(&settings, level, category, &message) {
            Prepared::Filtered => Admission::Filtered,
            Prepared::Invalid => {
                shared.count_dropped(1);
                Admission::Dropped
            }
            Prepared::Line(line) => self.finish(shared.admit_try(line, level)),
        }
    }

    /// `log_within` for an error record: no filter and no settings lock (the retained format flags
    /// stand in), normal admission by the deadline, and then the reserve instead of a drop.
    fn log_error_within(
        &self,
        category: &str,
        message: &dyn fmt::Display,
        deadline: Instant,
    ) -> Admission {
        let shared = &*self.shared;
        let timestamps = shared.flag_timestamps.load(Ordering::SeqCst);
        let colour = shared.flag_colour.load(Ordering::SeqCst);
        match render(timestamps, colour, Level::Error, category, message) {
            // `render` never reports `Filtered`; an error is admitted by every filter.
            Prepared::Filtered => Admission::Filtered,
            Prepared::Invalid => {
                shared.count_dropped(1);
                Admission::Dropped
            }
            Prepared::Line(line) => match shared.admit_within(line, Level::Error, deadline) {
                Ok(()) => self.finish(true),
                Err(line) => self.finish(shared.publish_reserved(line)),
            },
        }
    }

    /// Wakes the writer for an admitted record, or counts a dropped one.
    fn finish(&self, admitted: bool) -> Admission {
        if admitted {
            self.thread.unpark();
            Admission::Queued
        } else {
            self.shared.count_dropped(1);
            Admission::Dropped
        }
    }

    /// Waits at most `deadline` until every error-level record admitted before this call, by either
    /// route (the queue or the reserve), has been written and the sink flushed after it.
    ///
    /// It snapshots the highest error sequence number given out so far and waits for the contiguous
    /// delivered watermark to reach it, so a reserved error written early cannot stand in for an
    /// earlier one still in the queue. It returns `true` at once when no error is undelivered
    /// (whatever progress records are pending), and `false` at the deadline or as soon as the
    /// writer has ended with an error undelivered. Errors admitted after the call are outside it.
    #[must_use]
    pub fn await_errors(&self, deadline: Duration) -> bool {
        let shared = &*self.shared;
        let target = shared.next_seq.load(Ordering::SeqCst);
        let limit = Instant::now().checked_add(deadline);
        let mut gate = locked(&shared.gate);
        #[cfg(test)]
        let mut announced = false;
        loop {
            if gate.watermark >= target {
                return true;
            }
            #[cfg(test)]
            if !announced {
                announced = true;
                shared.probe(Probe::AwaitWait);
            }
            let remaining = limit.map_or(LONGEST_WAIT, |limit| {
                limit.saturating_duration_since(Instant::now())
            });
            if gate.exited || remaining.is_zero() {
                return false;
            }
            gate = shared
                .changed
                .wait_timeout(gate, remaining.min(LONGEST_WAIT))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Waits at most `deadline` for the writer to finish what was admitted before this call and to
    /// flush the sink.
    ///
    /// The call asks the writer to flush its sink (even if nothing was ever emitted). The outcome is
    /// [`FlushOutcome::Complete`] only when, before the deadline, that sink flush succeeded, nothing
    /// is queued, the writer is not inside a write or flush, the sink never failed and no record has
    /// ever been dropped. The calling thread never touches the sink, and a writer blocked in a write
    /// or flush is never joined: the result is then `Incomplete` when the deadline passes, even
    /// while emitters are waiting for space.
    #[must_use]
    pub fn flush(&self, deadline: Duration) -> FlushOutcome {
        let shared = &*self.shared;
        let limit = Instant::now().checked_add(deadline);
        let ticket = shared.flush_requested.fetch_add(1, Ordering::SeqCst) + 1;
        self.thread.unpark();
        let mut gate = locked(&shared.gate);
        loop {
            // `queued` before `in_flight`: see the module documentation.
            let queued = shared.queued.load(Ordering::SeqCst);
            let in_flight = shared.in_flight.load(Ordering::SeqCst);
            let dropped = shared.dropped_total.load(Ordering::SeqCst);
            let sink_error = gate.sink_error;
            let settled = queued == 0 && !in_flight && gate.flushed >= ticket;
            if settled && dropped == 0 && sink_error.is_none() {
                return FlushOutcome::Complete;
            }
            let remaining = limit.map_or(LONGEST_WAIT, |limit| {
                limit.saturating_duration_since(Instant::now())
            });
            if settled || gate.exited || remaining.is_zero() {
                return FlushOutcome::Incomplete {
                    queued,
                    in_flight,
                    dropped,
                    sink_error,
                };
            }
            gate = shared
                .changed
                .wait_timeout(gate, remaining.min(LONGEST_WAIT))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for Logger {
    /// Asks the writer to drain the queue and exit, and waits briefly for an idle writer. The wait
    /// ends when the writer *thread* has finished (`JoinHandle::is_finished`), not when the writer
    /// says it is done, because the sink is dropped after that: a sink destructor can block too. A
    /// writer that is inside a write, or has not finished within the grace period, is detached and
    /// ends by itself; the sink is never destroyed by the thread that drops the logger.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        self.thread.unpark();
        let Some(handle) = self.handle.take() else {
            return;
        };
        if self.shared.in_flight.load(Ordering::SeqCst) {
            return;
        }
        let deadline = Instant::now() + EXIT_GRACE;
        while !handle.is_finished() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            thread::sleep(remaining.min(EXIT_POLL));
        }
        // The thread has finished, so this returns at once. `writer_main` catches the sink's panic and
        // disposes of its payload on the writer, so the join carries no payload: there is nothing to
        // drop here that could panic.
        let _ = handle.join();
    }
}

/// Closes the logger when the writer thread ends, however it ends (normal stop, sink failure or a
/// panic in the sink).
///
/// It owns the writer's local `batch`, so that whatever was taken from the queue and not delivered
/// is counted exactly once on every way out: after an `io::Error`, the failing record and the rest
/// of the batch are still in it, and on a panic the record the sink was holding is too (a record
/// leaves the batch only after its write has succeeded).
struct Exit<'a> {
    shared: &'a Shared,
    /// The kind of the sink error that stopped the writer, if one did.
    error: Option<io::ErrorKind>,
    /// The records the writer has taken from the queue or the reserve and not yet delivered.
    batch: VecDeque<Entry>,
}

impl Drop for Exit<'_> {
    fn drop(&mut self) {
        let shared = self.shared;
        let error = self
            .error
            .take()
            .or_else(|| thread::panicking().then_some(io::ErrorKind::Other));
        let mut lost = u64::try_from(self.batch.len()).unwrap_or(u64::MAX);
        self.batch.clear();
        lost += close_reserve(shared);
        let leftover = {
            let mut queue = locked(&shared.queue);
            queue.closed = true;
            let leftover = std::mem::take(&mut queue.records);
            shared.queued.fetch_sub(leftover.len(), Ordering::SeqCst);
            // Every space waiter wakes, finds the queue closed and counts its own record.
            shared.space.notify_all();
            leftover
        };
        lost += u64::try_from(leftover.len()).unwrap_or(u64::MAX);
        drop(leftover);
        if lost > 0 {
            shared.count_dropped(lost);
        }
        let mut gate = locked(&shared.gate);
        if gate.sink_error.is_none() {
            gate.sink_error = error;
        }
        shared.in_flight.store(false, Ordering::SeqCst);
        gate.exited = true;
        shared.changed.notify_all();
    }
}

/// Closes the reserve and counts what it still holds: sets the closed flag, waits (the writer is
/// the only code that ever waits) until no publication is in progress, then takes every record out.
/// Returns how many it took; they are counted dropped by the caller.
fn close_reserve(shared: &Shared) -> u64 {
    shared.reserve_closed.store(true, Ordering::SeqCst);
    while shared.publishing.load(Ordering::SeqCst) != 0 {
        thread::yield_now();
    }
    let mut stranded = VecDeque::new();
    let count = shared.drain_reserve(&mut stranded);
    shared.queued.fetch_sub(count, Ordering::SeqCst);
    shared.reserved.fetch_sub(count, Ordering::SeqCst);
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// Moves every reserved record into `batch` (which must be empty), oldest first, and marks the writer
/// in flight. `in_flight` is set before `queued` is reduced, as for the queue.
fn take_reserved(shared: &Shared, batch: &mut VecDeque<Entry>) -> bool {
    debug_assert!(batch.is_empty());
    if shared.reserved.load(Ordering::SeqCst) == 0 {
        return false;
    }
    shared.in_flight.store(true, Ordering::SeqCst);
    let count = shared.drain_reserve(batch);
    shared.queued.fetch_sub(count, Ordering::SeqCst);
    shared.reserved.fetch_sub(count, Ordering::SeqCst);
    count > 0
}

/// Moves every queued record into `batch` (which must be empty) and marks the writer in flight.
///
/// `in_flight` is set under the queue lock and before `queued` is reduced: see the module
/// documentation.
fn take_batch(shared: &Shared, batch: &mut VecDeque<Entry>) -> bool {
    debug_assert!(batch.is_empty());
    let mut queue = locked(&shared.queue);
    if queue.records.is_empty() {
        return false;
    }
    shared.in_flight.store(true, Ordering::SeqCst);
    std::mem::swap(&mut queue.records, batch);
    shared.queued.fetch_sub(batch.len(), Ordering::SeqCst);
    // Room has been made: every emitter waiting for space looks again.
    shared.space.notify_all();
    true
}

/// Writes every record in `batch`, whole, in order, and notes the sequence number of each error
/// record in `written`. A record leaves the batch only after its write succeeded, so on an error
/// (or a panic in the sink) the failing record and the rest of the batch are still in it for
/// [`Exit`] to count.
fn write_batch(
    sink: &mut dyn Write,
    batch: &mut VecDeque<Entry>,
    written: &mut Vec<u64>,
) -> Result<(), io::ErrorKind> {
    while let Some(entry) = batch.front() {
        sink.write_all(entry.line.as_bytes())
            .map_err(|error| error.kind())?;
        if entry.seq != 0 {
            written.push(entry.seq);
        }
        batch.pop_front();
    }
    Ok(())
}

/// Which store a writer pass takes records from.
#[derive(Clone, Copy)]
enum Source {
    Reserve,
    Queue,
}

/// The body of the writer thread: [`run_writer`] inside the thread's one `catch_unwind`.
///
/// The catch is entered once per thread, around the whole of `run_writer`, and adds nothing to the
/// per-record path. [`Exit`] is a local of `run_writer`, so it is dropped during the unwind, inside this
/// catch: `thread::panicking()` still tells it the writer is panicking, and its accounting is as before.
/// The payload is the sink's (arbitrary code), so it is disposed of here, on the writer, and the thread
/// ends normally: neither `Logger::drop`'s join nor, for a detached writer, the standard library's
/// `Packet::drop` (which aborts if the drop of an unconsumed result panics) is left a payload to drop.
fn writer_main(shared: &Shared, sink: Box<dyn Write + Send>) {
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| run_writer(shared, sink))) {
        unwind::dispose(payload);
    }
}

/// The writer thread: the only code that writes to the sink.
///
/// Each pass takes every reserved record first and writes it, then takes the queued batch and writes
/// it, so a reserved error goes out before the next batch is taken and a batch already taken is
/// written to completion. After a pass that wrote an error record the sink is flushed at once, and
/// only then are those records' sequence numbers marked delivered.
fn run_writer(shared: &Shared, mut sink: Box<dyn Write + Send>) {
    let mut exit = Exit {
        shared,
        error: None,
        batch: VecDeque::new(),
    };
    let mut written: Vec<u64> = Vec::new();
    let mut unflushed = false;
    let mut served = 0_u64;
    loop {
        let mut worked = false;
        for source in [Source::Reserve, Source::Queue] {
            let taken = match source {
                Source::Reserve => take_reserved(shared, &mut exit.batch),
                Source::Queue => take_batch(shared, &mut exit.batch),
            };
            if !taken {
                continue;
            }
            worked = true;
            if let Err(kind) = write_batch(&mut *sink, &mut exit.batch, &mut written) {
                exit.error = Some(kind);
                return;
            }
            unflushed = true;
            if !written.is_empty() {
                if let Err(error) = sink.flush() {
                    exit.error = Some(error.kind());
                    return;
                }
                unflushed = false;
                shared.mark_delivered(&mut written);
            }
        }
        if worked || shared.reserved.load(Ordering::SeqCst) > 0 {
            continue;
        }
        if shared.dropped_pending.load(Ordering::SeqCst) > 0 {
            shared.in_flight.store(true, Ordering::SeqCst);
            let count = shared.dropped_pending.swap(0, Ordering::SeqCst);
            if count > 0 {
                let settings = shared.current_settings();
                let warning = format::line(
                    Level::Warn,
                    "log",
                    &format!("{count} record(s) dropped"),
                    settings.timestamps.then(SystemTime::now),
                    settings.colour,
                );
                if let Err(error) = sink.write_all(warning.as_bytes()) {
                    exit.error = Some(error.kind());
                    return;
                }
                unflushed = true;
            }
            continue;
        }
        let requested = shared.flush_requested.load(Ordering::SeqCst);
        if unflushed || requested > served {
            shared.in_flight.store(true, Ordering::SeqCst);
            if let Err(error) = sink.flush() {
                exit.error = Some(error.kind());
                return;
            }
            unflushed = false;
            served = requested;
            shared.publish_flushed(requested);
        }
        if shared.in_flight.swap(false, Ordering::SeqCst) {
            shared.wake_waiters();
        }
        if shared.stop.load(Ordering::SeqCst) {
            if locked(&shared.queue).records.is_empty()
                && shared.reserved.load(Ordering::SeqCst) == 0
            {
                return;
            }
            continue;
        }
        thread::park_timeout(IDLE_WAIT);
    }
}

#[cfg(test)]
impl Logger {
    /// Holds the settings' write lock, as `set_settings` does while it swaps.
    pub(crate) fn hold_settings(&self) -> std::sync::RwLockWriteGuard<'_, Arc<Settings>> {
        self.shared
            .settings
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Holds the queue lock, as the writer does while it moves records out.
    fn hold_queue(&self) -> MutexGuard<'_, Queue> {
        locked(&self.shared.queue)
    }

    /// Lets every emitter waiting for space go, now and later (they drop their record and return),
    /// so that a test that fails while emitters wait ends with its assertion instead of a hang.
    pub(crate) fn rescue_waiters(&self) {
        self.shared.rescued.store(true, Ordering::SeqCst);
        let _queue = locked(&self.shared.queue);
        self.shared.space.notify_all();
    }

    /// Whether the queue has closed.
    pub(crate) fn is_closed(&self) -> bool {
        locked(&self.shared.queue).closed
    }

    /// Whether the reserve has closed.
    pub(crate) fn reserve_closed(&self) -> bool {
        self.shared.reserve_closed.load(Ordering::SeqCst)
    }

    /// How many records the reserve holds.
    pub(crate) fn reserved(&self) -> usize {
        self.shared.reserved.load(Ordering::SeqCst)
    }

    /// The contiguous delivered watermark, the numbers delivered ahead of it, and the most numbers
    /// that set ever held.
    pub(crate) fn delivery(&self) -> (u64, Vec<u64>, usize) {
        let gate = locked(&self.shared.gate);
        (
            gate.watermark,
            gate.ahead.iter().copied().collect(),
            gate.ahead_peak,
        )
    }

    /// The highest error sequence number given out.
    pub(crate) fn highest_sequence(&self) -> u64 {
        self.shared.next_seq.load(Ordering::SeqCst)
    }

    /// Holds the reserve slots `range`, as a producer does while it publishes.
    pub(crate) fn hold_slots(
        &self,
        range: std::ops::Range<usize>,
    ) -> Vec<MutexGuard<'_, Option<Entry>>> {
        self.shared.reserve[range].iter().map(locked).collect()
    }

    /// Where the next reserve publication starts looking for a free slot.
    pub(crate) fn set_reserve_cursor(&self, slot: usize) {
        self.shared.reserve_cursor.store(slot, Ordering::SeqCst);
    }

    /// Pauses a reserve producer at the given points through `hook`, until the test releases it.
    pub(crate) fn set_pause(&self, hook: impl Fn(ReservePoint) + Send + Sync + 'static) {
        *locked(&self.shared.pauses) = Some(Arc::new(hook));
    }

    /// Every record dropped so far.
    pub(crate) fn dropped_total(&self) -> u64 {
        self.shared.dropped_total.load(Ordering::SeqCst)
    }

    /// Starts reporting the emitter-side rendezvous events to the returned receiver.
    pub(crate) fn probes(&self) -> std::sync::mpsc::Receiver<Probe> {
        let (sender, receiver) = std::sync::mpsc::channel();
        *locked(&self.shared.probes) = Some(sender);
        receiver
    }
}

#[cfg(test)]
mod tests;
