//! What the simulation owner publishes: an immutable snapshot in a latest-wins slot, and a terminal failure.

use std::sync::{Arc, Mutex, PoisonError};

use ck_sim::{Date, GameTime, Speed};

/// Why the game is paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PauseReason {
    /// The player paused.
    Player,
    /// A system failed, or the calendar ended.
    Error,
    /// The game was just started or loaded.
    Load,
}

/// What the UI shows of the simulation, at one moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SimSnapshot {
    /// The hour that runs next.
    pub(crate) tick: GameTime,
    /// The date of that hour.
    pub(crate) date: Date,
    /// The speed.
    pub(crate) speed: Speed,
    /// `None` while running.
    pub(crate) paused: Option<PauseReason>,
}

impl std::fmt::Display for SimSnapshot {
    /// `snapshot <date> speed <level> running`, or `... paused (<reason>)`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "snapshot {} speed {} ", self.date, self.speed.level())?;
        match self.paused {
            None => f.write_str("running"),
            Some(PauseReason::Player) => f.write_str("paused (player)"),
            Some(PauseReason::Error) => f.write_str("paused (error)"),
            Some(PauseReason::Load) => f.write_str("paused (load)"),
        }
    }
}

impl SimSnapshot {
    /// Whether anything the UI displays differs: the date (to the hour), the speed or the pause.
    pub(crate) fn differs_from(&self, other: &Self) -> bool {
        self.date != other.date || self.speed != other.speed || self.paused != other.paused
    }
}

#[derive(Default)]
struct Published {
    latest: Option<SimSnapshot>,
    failure: Option<String>,
}

/// The slot the owner writes and the main thread reads. Every change wakes the main thread through `wake`.
pub(crate) struct SimSlot {
    published: Mutex<Published>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl SimSlot {
    /// An empty slot that wakes the main thread through `wake`.
    pub(crate) fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Arc<Self> {
        Arc::new(Self {
            published: Mutex::new(Published::default()),
            wake,
        })
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, Published> {
        self.published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the snapshot and wakes the main thread.
    pub(crate) fn publish(&self, snapshot: SimSnapshot) {
        self.locked().latest = Some(snapshot);
        (self.wake)();
    }

    /// Records the terminal failure, once, and wakes the main thread. A second call changes nothing.
    pub(crate) fn fail(&self, message: String) {
        {
            let mut published = self.locked();
            if published.failure.is_some() {
                return;
            }
            published.failure = Some(message);
        }
        (self.wake)();
    }

    /// The latest snapshot, if one was published.
    pub(crate) fn latest(&self) -> Option<SimSnapshot> {
        self.locked().latest
    }

    /// The terminal failure, if the simulation has one. It is never cleared.
    pub(crate) fn failure(&self) -> Option<String> {
        self.locked().failure.clone()
    }
}
