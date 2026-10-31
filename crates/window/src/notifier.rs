//! The notifier of design §4.3: one lifetime-aware way to wake the main thread.
//!
//! Every producer (the signal thread, the request writer, the UI link's result wake, the presenter's
//! report wake) calls [`Notifier::wake`]. A wake always sets a pending flag first, so it is never lost,
//! even before the backend exists or after it is gone. Only while the notifier is **armed** does it also
//! call the backend's native wake target, and it does so without holding the notifier's lock.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// The native wake target of a backend: it makes a sleeping native pump return.
///
/// **It must not block, and it must be sticky:** `wake` leaves a queued native event (GLFW's
/// `glfwPostEmptyEvent` and an `AppKit` posted application-defined event both queue), so a
/// `pump(UntilEvent)` entered after the wake returns at once. A transient notification that is lost when
/// nobody is waiting does not satisfy the contract: it would open a gap between
/// [`Notifier::take_pending`] returning `false` and the wait.
pub trait NativeWake: Send + Sync {
    /// Wakes the backend's pump. Must not block.
    fn wake(&self);
}

/// Where a [`Notifier`] is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifierState {
    /// Before the backend is initialised: a wake only sets the pending flag.
    Inactive,
    /// Holding the native wake target: a wake also calls it.
    Armed,
    /// No new native call starts; waiting for the ones in flight to return.
    Disarming,
    /// No native call is in flight and none will start: the backend may be torn down.
    Disarmed,
}

/// [`Notifier::arm`] was called when the notifier was not [`NotifierState::Inactive`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArmRefused {
    /// The state it was in.
    pub state: NotifierState,
}

impl fmt::Display for ArmRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the notifier cannot be armed while {:?}",
            self.state
        )
    }
}

impl std::error::Error for ArmRefused {}

/// [`Notifier::disarm`] ran out of time with a native wake still in flight. The notifier stays
/// [`NotifierState::Disarming`] and the native target must not be destroyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisarmTimeout;

impl fmt::Display for DisarmTimeout {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a native wake was still in flight at the disarm deadline")
    }
}

impl std::error::Error for DisarmTimeout {}

struct Inner {
    state: NotifierState,
    in_flight: usize,
    target: Option<Arc<dyn NativeWake>>,
}

/// The shared notifier. See the module documentation.
pub struct Notifier {
    pending: AtomicBool,
    inner: Mutex<Inner>,
    idle: Condvar,
}

impl fmt::Debug for Notifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Notifier")
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

/// Deregisters an in-flight wake when dropped, also if the native call panicked.
struct InFlight<'a>(&'a Notifier);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        let mut inner = self.0.lock();
        inner.in_flight -= 1;
        if inner.in_flight == 0 {
            self.0.idle.notify_all();
        }
    }
}

impl Notifier {
    /// A notifier in the [`NotifierState::Inactive`] state, with no wake pending.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            pending: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                state: NotifierState::Inactive,
                in_flight: 0,
                target: None,
            }),
            idle: Condvar::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The state the notifier is in.
    #[must_use]
    pub fn state(&self) -> NotifierState {
        self.lock().state
    }

    /// Wakes the main thread. It first sets the pending flag; then, only while armed, registers itself as
    /// in flight under the lock, releases the lock, calls the native wake target and deregisters. It never
    /// holds the lock across the native call and never waits for anything else.
    pub fn wake(&self) {
        self.pending.store(true, Ordering::SeqCst);
        let target = {
            let mut inner = self.lock();
            if inner.state == NotifierState::Armed {
                inner.in_flight += 1;
                inner.target.clone()
            } else {
                None
            }
        };
        if let Some(target) = target {
            let _registered = InFlight(self);
            target.wake();
        }
    }

    /// Clears the pending flag and returns whether it was set. The loop calls it before each native wait.
    #[must_use]
    pub fn take_pending(&self) -> bool {
        self.pending.swap(false, Ordering::SeqCst)
    }

    /// Arms the notifier with the backend's native wake target. Legal only when
    /// [`NotifierState::Inactive`].
    ///
    /// # Errors
    ///
    /// [`ArmRefused`] in any other state.
    pub fn arm(&self, target: Arc<dyn NativeWake>) -> Result<(), ArmRefused> {
        let mut inner = self.lock();
        if inner.state != NotifierState::Inactive {
            return Err(ArmRefused { state: inner.state });
        }
        inner.state = NotifierState::Armed;
        inner.target = Some(target);
        Ok(())
    }

    /// Moves to [`NotifierState::Disarming`] (no new native call starts), waits at most `within` for the
    /// native calls in flight to return, then moves to [`NotifierState::Disarmed`] and releases the target.
    /// It never joins a thread.
    ///
    /// # Errors
    ///
    /// [`DisarmTimeout`] when a native call was still in flight at the deadline: the notifier then stays
    /// `Disarming` and keeps the target, which must not be destroyed.
    pub fn disarm(&self, within: Duration) -> Result<(), DisarmTimeout> {
        let give_up = Instant::now().checked_add(within);
        let mut inner = self.lock();
        if inner.state == NotifierState::Armed || inner.state == NotifierState::Inactive {
            inner.state = NotifierState::Disarming;
        }
        while inner.in_flight > 0 && inner.state == NotifierState::Disarming {
            let left = give_up.map_or(within, |limit| {
                limit.saturating_duration_since(Instant::now())
            });
            if left.is_zero() {
                return Err(DisarmTimeout);
            }
            inner = self
                .idle
                .wait_timeout(inner, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        inner.state = NotifierState::Disarmed;
        inner.target = None;
        Ok(())
    }
}
