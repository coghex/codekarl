//! The request slot of design §4.2: the window's desired state, shared between the UI VM's thread (a
//! [`RequestWriter`]) and the main thread (a [`RequestReader`]).
//!
//! The slot is **state, not a queue**: `set_clear` overwrites the colour (the last call wins), `open` and
//! `close` are one-shot flags, and the reader takes what changed since its last take in constant work,
//! however many writes happened. No call waits for the main thread. The slot's lock is held only to move
//! or copy a small value in or out: never while Lua runs, while a record is logged or while a native call
//! is made. Every accepted write calls the notifier's wake, after the lock is released.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::desc::{InvalidColour, OpenError, WindowDesc, clamp_colour};
use crate::notifier::Notifier;

/// A one-shot request: made at most once, and taken at most once.
#[derive(Default)]
enum OneShot<T> {
    /// Not requested yet.
    #[default]
    Unrequested,
    /// Requested, not yet taken.
    Pending(T),
    /// Requested and taken.
    Taken,
}

impl<T> OneShot<T> {
    fn is_requested(&self) -> bool {
        !matches!(self, Self::Unrequested)
    }

    /// Takes a pending value, leaving the request recorded as taken.
    fn take(&mut self) -> Option<T> {
        match std::mem::replace(self, Self::Taken) {
            Self::Pending(value) => Some(value),
            Self::Unrequested => {
                *self = Self::Unrequested;
                None
            }
            Self::Taken => None,
        }
    }
}

#[derive(Default)]
struct State {
    /// `open`: a second one is [`OpenError::AlreadyOpened`].
    open: OneShot<WindowDesc>,
    /// The latest colour, not yet taken.
    clear: Option<[f32; 3]>,
    /// `close`: a second one changes nothing.
    close: OneShot<()>,
    /// The loop has finished: writes change nothing visible.
    ended: bool,
}

struct Shared {
    state: Mutex<State>,
    discarded: AtomicU64,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What changed in the slot since the last [`RequestReader::take`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RequestChanges {
    /// The description of an `open` newly requested.
    pub open: Option<WindowDesc>,
    /// The latest clear colour, if it was written since the last take.
    pub clear: Option<[f32; 3]>,
    /// A `close` newly requested.
    pub close: bool,
}

/// The request slot. Create it with [`Requests::new`], then hand a [`RequestWriter`] to the UI VM's
/// thread and a [`RequestReader`] to the main loop.
#[derive(Clone)]
pub struct Requests {
    shared: Arc<Shared>,
}

impl Default for Requests {
    fn default() -> Self {
        Self::new()
    }
}

impl Requests {
    /// An empty slot.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                discarded: AtomicU64::new(0),
            }),
        }
    }

    /// A writer for the UI VM's thread; every accepted write calls `notifier.wake()`.
    #[must_use]
    pub fn writer(&self, notifier: Arc<Notifier>) -> RequestWriter {
        RequestWriter {
            shared: Arc::clone(&self.shared),
            notifier,
        }
    }

    /// The reader for the main loop.
    #[must_use]
    pub fn reader(&self) -> RequestReader {
        RequestReader {
            shared: Arc::clone(&self.shared),
        }
    }

    /// How many writes arrived after the slot was ended and were discarded.
    #[must_use]
    pub fn discarded(&self) -> u64 {
        self.shared.discarded.load(Ordering::SeqCst)
    }
}

/// The UI VM's side of the slot. Cloneable and usable from any thread; no call ever waits for the main
/// thread.
#[derive(Clone)]
pub struct RequestWriter {
    shared: Arc<Shared>,
    notifier: Arc<Notifier>,
}

impl RequestWriter {
    /// Requests the window. One-shot.
    ///
    /// # Errors
    ///
    /// [`OpenError::InvalidDesc`] for a description that is not valid (nothing is recorded, and nothing
    /// is counted); [`OpenError::AlreadyOpened`] for a second `open`, **also after the slot ended** (the
    /// one-shot check comes first). On a slot that ended without an `open`, the write is discarded and
    /// counted, and returns `Ok`.
    pub fn open(&self, desc: WindowDesc) -> Result<(), OpenError> {
        let desc = desc.validated()?;
        {
            let mut state = self.shared.lock();
            if state.open.is_requested() {
                return Err(OpenError::AlreadyOpened);
            }
            if state.ended {
                self.shared.discarded.fetch_add(1, Ordering::SeqCst);
            } else {
                state.open = OneShot::Pending(desc);
            }
        }
        self.notifier.wake();
        Ok(())
    }

    /// Sets the clear colour. Finite components are clamped to 0..=1; the last call wins.
    ///
    /// # Errors
    ///
    /// [`InvalidColour`] for a non-finite component (nothing is recorded).
    pub fn set_clear(&self, rgb: [f32; 3]) -> Result<(), InvalidColour> {
        let rgb = clamp_colour(rgb)?;
        {
            let mut state = self.shared.lock();
            if state.ended {
                self.shared.discarded.fetch_add(1, Ordering::SeqCst);
            } else {
                state.clear = Some(rgb);
            }
        }
        self.notifier.wake();
        Ok(())
    }

    /// Requests the window be closed, without asking the close callback. One-shot: a second `close`
    /// changes nothing.
    pub fn close(&self) {
        {
            let mut state = self.shared.lock();
            if state.ended {
                self.shared.discarded.fetch_add(1, Ordering::SeqCst);
            } else if !state.close.is_requested() {
                state.close = OneShot::Pending(());
            }
        }
        self.notifier.wake();
    }
}

/// The main loop's side of the slot.
pub struct RequestReader {
    shared: Arc<Shared>,
}

impl RequestReader {
    /// What changed since the last take. Constant work, however many writes happened.
    #[must_use]
    pub fn take(&self) -> RequestChanges {
        let mut state = self.shared.lock();
        RequestChanges {
            open: state.open.take(),
            clear: state.clear.take(),
            close: state.close.take().is_some(),
        }
    }

    /// Marks the slot ended (the loop has finished). Later writes return at once, change nothing visible
    /// and are counted by [`Requests::discarded`].
    pub fn end(&self) {
        self.shared.lock().ended = true;
    }
}
