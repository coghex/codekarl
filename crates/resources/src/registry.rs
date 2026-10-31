//! The texture registry: generational keys, public ownership, retained references and retirement
//! (`docs/scene-design.md` §4.1, §4.2, §4.4).
//!
//! # Identity
//!
//! A [`TextureKey`] names one *incarnation* of a logical slot: the registry's identity, the slot and a generation that
//! advances each time the slot is reused, so an old key can never reach a new resource. Its [`Display`](fmt::Display)
//! is its handle token ([`Token`]). Tokens are process-local identities, not saved data and not a security boundary.
//!
//! # Ownership
//!
//! A live entry has **public ownership** (one bit: the token a script or a Rust producer holds) and **retained
//! references** ([`TextureRef`]s, which quads, snapshots and frames will hold). When it has neither it becomes
//! *retiring*: no key reaches it, its slot is not reused, and its key is appended once to a FIFO that the owner
//! drains. The owner calls [`TextureRegistry::finish_retirement`] when nothing can use the resource any more; that
//! frees the slot with its generation advanced by one and hands back the value, to be dropped outside the lock.
//!
//! **A job holds a [`TextureKey`], never a [`TextureRef`].** A job that held a reference would keep the entry live, so
//! releasing the last public token and dropping the last quad would no longer begin retirement and cancel the job
//! (`docs/scene-design.md` §4.3). The owner reaches the entry of a key, whatever its ownership, with
//! [`TextureRegistry::with_entry_value`].
//!
//! # The last-reference race, and why it is correct
//!
//! A [`TextureRef`] is an `Arc` of the registry's shared state, the key and a per-slot atomic count of the
//! references to the entry. Everything that can change whether an entry is retiring happens under the one registry
//! lock, except the count's own atomic increments and decrements, and those are arranged so that nothing is lost:
//!
//! - **Clone takes no lock.** A clone is made from an existing reference, so the count is at least one while it
//!   increments. A count of zero is never incremented by a clone.
//! - **Only [`retain`](TextureRegistry::retain) raises the count from zero**, and it does so under the lock, after
//!   checking live public ownership.
//! - **Dropping a reference decrements the count without the lock.** Only the drop that took it from one to zero
//!   goes on, and it takes the lock once and *re-checks under it*: the entry is still this incarnation (the slot's
//!   generation is the key's), is live, has lost its public ownership and the count is still zero. Otherwise it does
//!   nothing.
//! - A new `retain` that ran between the decrement and the lock raised the count again, so the re-check sees a
//!   non-zero count and the entry stays live; that new reference's own last drop reconciles later. A `retain`
//!   that runs after the lock finds the entry already retiring, which has no public ownership, and is refused.
//! - The transition is `Live -> Retiring`, made only from `Live`, so two last-drop actions (an older one and a newer
//!   one), or one of them and [`release`](TextureRegistry::release) or [`close`](TextureRegistry::close), cannot
//!   both retire the entry: the second finds `Retiring` and does nothing. An action left over from an earlier
//!   incarnation carries an older generation and finds nothing.
//!
//! So an entry retires exactly once, never while a reference exists, and never because of a stale decision.
//!
//! # Locking
//!
//! Every operation takes the one registry lock at most once, does no I/O and never runs while holding another lock
//! of this crate. No value is ever dropped while it is held: [`finish_retirement`](TextureRegistry::finish_retirement)
//! returns the value, [`close`](TextureRegistry::close) drops none and the shared state, with the values left in it,
//! is dropped when the last registry handle or reference goes.

use std::collections::VecDeque;
use std::fmt;
#[cfg(test)]
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::token::{RegistryId, RegistryIdsExhausted, Token, TokenError, TokenKind};

/// The retiring wake: called when an entry becomes retiring and the FIFO was empty, after the registry released its
/// lock. It is a wake, not a delivery: the owner drains the FIFO. It must be short, must not block, must not call the
/// registry synchronously and must not panic (a panic reaches the caller of the operation that retired the entry).
pub type RetiringWake = Arc<dyn Fn() + Send + Sync>;

/// Names one incarnation of a logical texture slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureKey {
    registry: RegistryId,
    slot: u32,
    generation: u64,
}

impl TextureKey {
    /// The key naming `slot` at `generation` in `registry` (crate-private: the queues rebuild keys they store in parts).
    pub(crate) fn new(registry: RegistryId, slot: u32, generation: u64) -> Self {
        Self {
            registry,
            slot,
            generation,
        }
    }

    /// The registry this key belongs to.
    #[must_use]
    pub fn registry(self) -> RegistryId {
        self.registry
    }

    /// The logical slot.
    #[must_use]
    pub fn slot(self) -> u32 {
        self.slot
    }

    /// The slot's generation.
    #[must_use]
    pub fn generation(self) -> u64 {
        self.generation
    }

    /// The key a texture token names. A token is only well formed here; the registry says whether it is usable.
    ///
    /// # Errors
    ///
    /// [`HandleError::Token`] for a string that is not a token, and [`HandleError::WrongKind`] for a well-formed token
    /// of another kind.
    pub fn from_token(text: &str) -> Result<Self, HandleError> {
        let token: Token = text.parse().map_err(HandleError::Token)?;
        if token.kind != TokenKind::Texture {
            return Err(HandleError::WrongKind {
                expected: TokenKind::Texture,
                found: token.kind,
            });
        }
        Ok(Self {
            registry: token.registry,
            slot: token.slot,
            generation: token.generation,
        })
    }

    /// The key as a parsed token.
    #[must_use]
    pub fn token(self) -> Token {
        Token {
            kind: TokenKind::Texture,
            registry: self.registry,
            slot: self.slot,
            generation: self.generation,
        }
    }
}

impl fmt::Display for TextureKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.token().fmt(formatter)
    }
}

/// Why a key could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleError {
    /// A string that is not a handle token.
    Token(TokenError),
    /// A well-formed token of another kind.
    WrongKind {
        /// The kind the operation takes.
        expected: TokenKind,
        /// The kind the token has.
        found: TokenKind,
    },
    /// The key's registry is not this one.
    Foreign(TextureKey),
    /// The slot is free, retiring or reused, or the public ownership was released.
    Stale(TextureKey),
    /// The registry is closed.
    Closed,
    /// `finish_retirement` of a retiring key that `drain_retiring` has not yet handed out.
    NotDrained(TextureKey),
}

impl fmt::Display for HandleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(error) => error.fmt(formatter),
            Self::WrongKind { expected, found } => {
                write!(
                    formatter,
                    "a {found} handle was given where a {expected} handle is needed"
                )
            }
            Self::Foreign(key) => write!(
                formatter,
                "texture handle (registry {}, slot {}, generation {}) belongs to another registry",
                key.registry, key.slot, key.generation
            ),
            Self::Stale(key) => write!(
                formatter,
                "texture handle (slot {}, generation {}) is no longer usable",
                key.slot, key.generation
            ),
            Self::Closed => formatter.write_str("the texture registry is closed"),
            Self::NotDrained(key) => write!(
                formatter,
                "texture (slot {}, generation {}) is retiring but has not been drained yet",
                key.slot, key.generation
            ),
        }
    }
}

impl std::error::Error for HandleError {}

/// Why a registry could not be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewError {
    /// A registry holds at least one slot.
    ZeroCapacity,
    /// Every registry identity has been issued.
    Ids(RegistryIdsExhausted),
}

impl fmt::Display for NewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCapacity => formatter.write_str("a registry holds at least one slot"),
            Self::Ids(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for NewError {}

/// Why a texture could not be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateError {
    /// No slot is free. Retiring and permanently retired slots count as used.
    Full {
        /// The registry's capacity.
        capacity: u32,
    },
    /// The registry is closed.
    Closed,
    /// The memory for another slot could not be reserved.
    OutOfMemory,
}

impl fmt::Display for CreateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full { capacity } => write!(formatter, "all {capacity} texture slots are in use"),
            Self::Closed => formatter.write_str("the texture registry is closed"),
            Self::OutOfMemory => formatter.write_str("no memory for another texture slot"),
        }
    }
}

impl std::error::Error for CreateError {}

/// Counters kept up to date by every transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistryStats {
    /// The registry's slot capacity.
    pub capacity: u32,
    /// Entries with public ownership or retained references.
    pub live: u32,
    /// Entries retiring: freed logically, not yet finished.
    pub retiring: u32,
    /// Slots whose generation is used up: never reused.
    pub exhausted: u32,
    /// Slots a `create` can take.
    pub free: u32,
}

/// What a registry entry's ownership is, as the service layer sees it under the registry lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lifecycle {
    /// Live: `public` is the public ownership bit, `referenced` whether a `TextureRef` exists.
    Live {
        /// Whether the public token is still held.
        public: bool,
        /// Whether a retained reference exists.
        referenced: bool,
    },
    /// Retiring: no key reaches it any more.
    Retiring,
}

/// Where an entry is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Free,
    /// `public` is the one bit of public ownership.
    Live {
        public: bool,
    },
    /// `drained` is set when `drain_retiring` has handed the key out.
    Retiring {
        drained: bool,
    },
    /// The slot's generation is `u64::MAX` and it was freed: never reused.
    Exhausted,
}

struct Slot<T> {
    generation: u64,
    phase: Phase,
    value: Option<T>,
    /// The number of `TextureRef`s of this slot's current entry. One allocation serves every incarnation of the
    /// slot: a reference left over from an earlier one has already decremented, and acts only through its key's
    /// generation.
    refs: Arc<AtomicUsize>,
}

struct State<T> {
    /// Slots ever used, in index order; never longer than the capacity.
    slots: Vec<Slot<T>>,
    /// Freed slots, oldest first.
    free: VecDeque<u32>,
    /// Retiring keys not yet drained, oldest first; every entry is in it at most once.
    fifo: VecDeque<TextureKey>,
    closed: bool,
    live: u32,
    retiring: u32,
    exhausted: u32,
}

struct Inner<T> {
    id: RegistryId,
    capacity: u32,
    on_retiring: Option<RetiringWake>,
    state: Mutex<State<T>>,
    /// Called by a dropped last reference between its decrement and its lock (tests only).
    #[cfg(test)]
    drop_pause: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

/// The registry of textures: a cloneable handle on shared state.
pub struct TextureRegistry<T: Send> {
    inner: Arc<Inner<T>>,
}

impl<T: Send> Clone for TextureRegistry<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: Send> fmt::Debug for TextureRegistry<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TextureRegistry")
            .field("id", &self.inner.id)
            .field("stats", &self.stats())
            .finish()
    }
}

impl<T> State<T> {
    /// The slot of `key` if the key names its current incarnation.
    fn entry(&mut self, key: TextureKey) -> Option<&mut Slot<T>> {
        self.slots
            .get_mut(usize::try_from(key.slot).ok()?)
            .filter(|slot| slot.generation == key.generation)
    }

    /// Makes the live entry at `index` retiring and appends its key to the FIFO. Returns whether the FIFO was empty,
    /// that is whether the retiring wake is owed.
    fn retire(&mut self, registry: RegistryId, index: usize) -> bool {
        let slot = &mut self.slots[index];
        debug_assert!(matches!(slot.phase, Phase::Live { .. }));
        slot.phase = Phase::Retiring { drained: false };
        let key = TextureKey {
            registry,
            slot: u32::try_from(index).unwrap_or(u32::MAX),
            generation: slot.generation,
        };
        self.live -= 1;
        self.retiring += 1;
        let was_empty = self.fifo.is_empty();
        self.fifo.push_back(key);
        was_empty
    }

    /// Room for slot number `self.slots.len()` and the queues that can hold it, grown in steps and never past
    /// `capacity`. A failure reserves nothing it cannot use and changes no entry.
    fn grow(&mut self, capacity: u32) -> Result<(), ()> {
        let have = self.slots.len();
        if have < self.slots.capacity()
            && have < self.free.capacity()
            && have < self.fifo.capacity()
        {
            return Ok(());
        }
        let want = usize::try_from(capacity).unwrap_or(usize::MAX);
        let target = have.saturating_mul(2).max(4).min(want);
        self.slots
            .try_reserve_exact(target - have)
            .map_err(|_| ())?;
        self.free
            .try_reserve_exact(target.saturating_sub(self.free.len()))
            .map_err(|_| ())?;
        self.fifo
            .try_reserve_exact(target.saturating_sub(self.fifo.len()))
            .map_err(|_| ())?;
        Ok(())
    }
}

/// The registry lock's guard. In tests it also records, per thread, that the lock is held (`lockdebug`).
#[cfg(test)]
struct Locked<'a, T> {
    guard: MutexGuard<'a, State<T>>,
    _held: crate::lockdebug::Held,
}

#[cfg(test)]
impl<T> Deref for Locked<'_, T> {
    type Target = State<T>;

    fn deref(&self) -> &State<T> {
        &self.guard
    }
}

#[cfg(test)]
impl<T> DerefMut for Locked<'_, T> {
    fn deref_mut(&mut self) -> &mut State<T> {
        &mut self.guard
    }
}

#[cfg(not(test))]
type Locked<'a, T> = MutexGuard<'a, State<T>>;

impl<T: Send> Inner<T> {
    fn lock(&self) -> Locked<'_, T> {
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        #[cfg(test)]
        {
            Locked {
                guard,
                _held: crate::lockdebug::Held::new(),
            }
        }
        #[cfg(not(test))]
        guard
    }

    fn wake(&self, owed: bool) {
        if owed && let Some(wake) = &self.on_retiring {
            wake();
        }
    }

    /// The failure for a key that is not this registry's.
    fn check_registry(&self, key: TextureKey) -> Result<(), HandleError> {
        if key.registry == self.id {
            Ok(())
        } else {
            Err(HandleError::Foreign(key))
        }
    }

    /// The live entry of a key with public ownership, for the script-facing operations.
    fn public_entry<'a>(
        &self,
        state: &'a mut State<T>,
        key: TextureKey,
    ) -> Result<&'a mut Slot<T>, HandleError> {
        self.check_registry(key)?;
        if state.closed {
            return Err(HandleError::Closed);
        }
        match state.entry(key) {
            Some(slot) if matches!(slot.phase, Phase::Live { public: true }) => Ok(slot),
            _ => Err(HandleError::Stale(key)),
        }
    }

    /// A dropped last reference's reconciliation (see the module documentation).
    fn last_reference_dropped(&self, key: TextureKey) {
        #[cfg(test)]
        {
            let pause = self
                .drop_pause
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(pause) = pause {
                pause();
            }
        }
        let owed = {
            let mut state = self.lock();
            let index = usize::try_from(key.slot).unwrap_or(usize::MAX);
            let retire = state.entry(key).is_some_and(|slot| {
                matches!(slot.phase, Phase::Live { public: false })
                    && slot.refs.load(Ordering::SeqCst) == 0
            });
            retire && state.retire(self.id, index)
        };
        self.wake(owed);
    }
}

impl<T: Send> TextureRegistry<T> {
    /// A registry of up to `capacity` logical texture slots. `on_retiring` is the retiring wake (`None`: the owner
    /// polls [`drain_retiring`](Self::drain_retiring)).
    ///
    /// # Errors
    ///
    /// [`NewError::ZeroCapacity`] for 0 and [`NewError::Ids`] when the process has issued every registry identity.
    pub fn new(capacity: u32, on_retiring: Option<RetiringWake>) -> Result<Self, NewError> {
        Self::with_id(capacity, on_retiring, RegistryId::allocate())
    }

    pub(crate) fn with_id(
        capacity: u32,
        on_retiring: Option<RetiringWake>,
        id: Result<RegistryId, RegistryIdsExhausted>,
    ) -> Result<Self, NewError> {
        if capacity == 0 {
            return Err(NewError::ZeroCapacity);
        }
        let id = id.map_err(NewError::Ids)?;
        Ok(Self {
            inner: Arc::new(Inner {
                id,
                capacity,
                on_retiring,
                state: Mutex::new(State {
                    slots: Vec::new(),
                    free: VecDeque::new(),
                    fifo: VecDeque::new(),
                    closed: false,
                    live: 0,
                    retiring: 0,
                    exhausted: 0,
                }),
                #[cfg(test)]
                drop_pause: Mutex::new(None),
            }),
        })
    }

    /// This registry's identity.
    #[must_use]
    pub fn id(&self) -> RegistryId {
        self.inner.id
    }

    /// The most slots the registry holds.
    #[must_use]
    pub fn capacity(&self) -> u32 {
        self.inner.capacity
    }

    /// Takes a free slot (the oldest freed first, otherwise the next never-used one), stores `value`, gives it public
    /// ownership and returns its key.
    ///
    /// # Errors
    ///
    /// [`CreateError::Closed`], [`CreateError::Full`] when no slot is free (retiring and permanently retired slots
    /// count as used), and [`CreateError::OutOfMemory`] when the memory for a new slot cannot be reserved.
    pub fn create(&self, value: T) -> Result<TextureKey, CreateError> {
        // On a refusal the value comes back and is dropped here, after the registry lock was released.
        self.create_returning(value)
            .map_err(|(error, _value)| error)
    }

    /// `create`, handing the value back on a refusal so that a caller that holds another lock can drop it after
    /// releasing that one.
    pub(crate) fn create_returning(&self, value: T) -> Result<TextureKey, (CreateError, T)> {
        let mut state = self.inner.lock();
        if state.closed {
            return Err((CreateError::Closed, value));
        }
        let index = if let Some(index) = state.free.pop_front() {
            usize::try_from(index).unwrap_or(usize::MAX)
        } else if state.slots.len() < usize::try_from(self.inner.capacity).unwrap_or(usize::MAX) {
            if state.grow(self.inner.capacity).is_err() {
                return Err((CreateError::OutOfMemory, value));
            }
            state.slots.push(Slot {
                generation: 1,
                phase: Phase::Free,
                value: None,
                refs: Arc::new(AtomicUsize::new(0)),
            });
            state.slots.len() - 1
        } else {
            return Err((
                CreateError::Full {
                    capacity: self.inner.capacity,
                },
                value,
            ));
        };
        let slot = &mut state.slots[index];
        slot.phase = Phase::Live { public: true };
        slot.value = Some(value);
        let key = TextureKey {
            registry: self.inner.id,
            slot: u32::try_from(index).unwrap_or(u32::MAX),
            generation: slot.generation,
        };
        state.live += 1;
        Ok(key)
    }

    /// Whether `key` may be used now: live public ownership in this registry.
    ///
    /// # Errors
    ///
    /// [`HandleError::Foreign`], [`HandleError::Closed`] or [`HandleError::Stale`].
    pub fn check(&self, key: TextureKey) -> Result<(), HandleError> {
        let mut state = self.inner.lock();
        self.inner.public_entry(&mut state, key).map(|_| ())
    }

    /// A retained reference to the entry of `key`, which needs live public ownership (a quad is created from a public
    /// token).
    ///
    /// # Errors
    ///
    /// As [`check`](Self::check).
    pub fn retain(&self, key: TextureKey) -> Result<TextureRef<T>, HandleError> {
        let mut state = self.inner.lock();
        let slot = self.inner.public_entry(&mut state, key)?;
        // Raised from zero only here, under the lock, and only while the entry has public ownership.
        slot.refs.fetch_add(1, Ordering::SeqCst);
        let refs = Arc::clone(&slot.refs);
        drop(state);
        Ok(TextureRef {
            inner: Arc::clone(&self.inner),
            key,
            refs,
        })
    }

    /// Ends public ownership of `key` at once. An entry with no retained reference becomes retiring.
    ///
    /// # Errors
    ///
    /// As [`check`](Self::check); a second release is [`HandleError::Stale`].
    pub fn release(&self, key: TextureKey) -> Result<(), HandleError> {
        let owed = {
            let mut state = self.inner.lock();
            let slot = self.inner.public_entry(&mut state, key)?;
            slot.phase = Phase::Live { public: false };
            let unreferenced = slot.refs.load(Ordering::SeqCst) == 0;
            let index = usize::try_from(key.slot).unwrap_or(usize::MAX);
            unreferenced && state.retire(self.inner.id, index)
        };
        self.inner.wake(owed);
        Ok(())
    }

    /// Runs `f` on the value of `key`, under the registry lock, and returns its result. The key needs live public
    /// ownership.
    ///
    /// `f` must be short, must not call the registry, and must not drop a [`TextureRef`] or a large value: it would
    /// deadlock, or hold the lock across a large free.
    ///
    /// # Errors
    ///
    /// As [`check`](Self::check).
    pub fn with_value<R>(
        &self,
        key: TextureKey,
        f: impl FnOnce(&mut T) -> R,
    ) -> Result<R, HandleError> {
        let mut state = self.inner.lock();
        let slot = self.inner.public_entry(&mut state, key)?;
        slot.value.as_mut().map(f).ok_or(HandleError::Stale(key))
    }

    /// The owner's accessor: runs `f` on the value of the entry of exactly this key's generation, under the registry
    /// lock, **whatever its ownership**: live with public ownership, live through retained references only, or
    /// retiring. It keeps working after [`close`](Self::close). It is for the owner of the per-resource state, not for
    /// validating a script's token: script-facing operations use [`check`](Self::check), [`retain`](Self::retain),
    /// [`release`](Self::release) and [`with_value`](Self::with_value), which require live public ownership.
    ///
    /// `f` has the rules of [`with_value`](Self::with_value). A job holds a [`TextureKey`] and reaches its entry
    /// through this method, never through a [`TextureRef`].
    ///
    /// # Errors
    ///
    /// [`HandleError::Foreign`], or [`HandleError::Stale`] once the entry's retirement has finished.
    pub fn with_entry_value<R>(
        &self,
        key: TextureKey,
        f: impl FnOnce(&mut T) -> R,
    ) -> Result<R, HandleError> {
        self.inner.check_registry(key)?;
        let mut state = self.inner.lock();
        match state.entry(key) {
            Some(slot) if matches!(slot.phase, Phase::Live { .. } | Phase::Retiring { .. }) => {
                slot.value.as_mut().map(f).ok_or(HandleError::Stale(key))
            }
            _ => Err(HandleError::Stale(key)),
        }
    }

    /// The entry of `key` with its lifecycle, for the service layer: runs `f` under the registry lock on the value of the
    /// entry of exactly this key's generation, live (with or without public ownership) or retiring, and returns its
    /// result. The decision to accept work is made inside `f`, so a last-reference drop (which needs this lock to make
    /// the entry retiring) falls either before it or after it. `f` has the rules of [`with_value`](Self::with_value);
    /// it may move values out for the caller to drop, and drops none itself.
    pub(crate) fn with_entry_lifecycle<R>(
        &self,
        key: TextureKey,
        f: impl FnOnce(Lifecycle, &mut T) -> R,
    ) -> Result<R, HandleError> {
        self.inner.check_registry(key)?;
        let mut state = self.inner.lock();
        match state.entry(key) {
            Some(slot) => {
                let lifecycle = match slot.phase {
                    Phase::Live { public } => Lifecycle::Live {
                        public,
                        referenced: slot.refs.load(Ordering::SeqCst) > 0,
                    },
                    Phase::Retiring { .. } => Lifecycle::Retiring,
                    Phase::Free | Phase::Exhausted => return Err(HandleError::Stale(key)),
                };
                slot.value
                    .as_mut()
                    .map(|value| f(lifecycle, value))
                    .ok_or(HandleError::Stale(key))
            }
            None => Err(HandleError::Stale(key)),
        }
    }

    /// Runs `f` on every live or retiring entry, under the registry lock, in slot order (for the service's `close`,
    /// which is O(capacity) like the registry's own). `f` has the rules of
    /// [`with_value`](Self::with_value).
    pub(crate) fn for_each_entry(&self, mut f: impl FnMut(TextureKey, Lifecycle, &mut T)) {
        let mut state = self.inner.lock();
        let id = self.inner.id;
        for (index, slot) in state.slots.iter_mut().enumerate() {
            let lifecycle = match slot.phase {
                Phase::Live { public } => Lifecycle::Live {
                    public,
                    referenced: slot.refs.load(Ordering::SeqCst) > 0,
                },
                Phase::Retiring { .. } => Lifecycle::Retiring,
                Phase::Free | Phase::Exhausted => continue,
            };
            let key = TextureKey {
                registry: id,
                slot: u32::try_from(index).unwrap_or(u32::MAX),
                generation: slot.generation,
            };
            if let Some(value) = slot.value.as_mut() {
                f(key, lifecycle, value);
            }
        }
    }

    /// Moves at most `limit` retiring keys into `out`, oldest first, and marks them drained. A key is handed out
    /// once.
    pub fn drain_retiring(&self, out: &mut Vec<TextureKey>, limit: usize) -> usize {
        // Reserved before the lock, so nothing below allocates under it; the FIFO never holds more than `capacity`.
        out.reserve(limit.min(usize::try_from(self.inner.capacity).unwrap_or(usize::MAX)));
        let mut state = self.inner.lock();
        let mut moved = 0;
        while moved < limit {
            let Some(key) = state.fifo.pop_front() else {
                break;
            };
            if let Some(slot) = state.entry(key) {
                slot.phase = Phase::Retiring { drained: true };
            }
            out.push(key);
            moved += 1;
        }
        moved
    }

    /// The proof point the owners call once nothing can use the resource any more: frees the slot with its generation
    /// advanced by one (a slot whose generation is `u64::MAX` is never reused) and returns the value, to be dropped
    /// outside the lock.
    ///
    /// # Errors
    ///
    /// [`HandleError::Foreign`]; [`HandleError::Stale`] for an entry that is not retiring with that generation (live,
    /// already finished, reused); [`HandleError::NotDrained`] for a retiring key that [`drain_retiring`](Self::drain_retiring)
    /// has not handed out, which changes nothing. It works after [`close`](Self::close).
    pub fn finish_retirement(&self, key: TextureKey) -> Result<T, HandleError> {
        self.inner.check_registry(key)?;
        let mut state = self.inner.lock();
        let Some(slot) = state.entry(key) else {
            return Err(HandleError::Stale(key));
        };
        match slot.phase {
            Phase::Retiring { drained: true } => {}
            Phase::Retiring { drained: false } => return Err(HandleError::NotDrained(key)),
            _ => return Err(HandleError::Stale(key)),
        }
        let value = slot.value.take().ok_or(HandleError::Stale(key))?;
        let exhausted = slot.generation == u64::MAX;
        if exhausted {
            slot.phase = Phase::Exhausted;
        } else {
            slot.generation += 1;
            slot.phase = Phase::Free;
        }
        state.retiring -= 1;
        if exhausted {
            state.exhausted += 1;
        } else {
            state.free.push_back(key.slot);
        }
        drop(state);
        Ok(value)
    }

    /// Ends the run: ends public ownership of every live entry (so a forgotten release cannot outlive it) and makes
    /// every later `create` and key operation [`Closed`](HandleError::Closed). It drops no value. Entries with no
    /// retained reference become retiring at once; the rest retire when their last [`TextureRef`] drops.
    /// [`drain_retiring`](Self::drain_retiring), [`finish_retirement`](Self::finish_retirement),
    /// [`with_entry_value`](Self::with_entry_value), [`TextureRef::with_value`] and the retiring wake keep working, so
    /// teardown can finish. A second close does nothing.
    pub fn close(&self) {
        let owed = {
            let mut state = self.inner.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            let mut owed = false;
            for index in 0..state.slots.len() {
                let slot = &mut state.slots[index];
                if slot.phase != (Phase::Live { public: true }) {
                    continue;
                }
                slot.phase = Phase::Live { public: false };
                if slot.refs.load(Ordering::SeqCst) == 0 {
                    owed |= state.retire(self.inner.id, index);
                }
            }
            owed
        };
        self.inner.wake(owed);
    }

    /// The counters.
    #[must_use]
    pub fn stats(&self) -> RegistryStats {
        let state = self.inner.lock();
        let never_used = self
            .inner
            .capacity
            .saturating_sub(u32::try_from(state.slots.len()).unwrap_or(u32::MAX));
        RegistryStats {
            capacity: self.inner.capacity,
            live: state.live,
            retiring: state.retiring,
            exhausted: state.exhausted,
            free: u32::try_from(state.free.len()).unwrap_or(u32::MAX) + never_used,
        }
    }
}

/// A retained reference to a texture entry: it keeps the entry (and the registry's shared state) alive after the
/// public token is released or the registry is closed, so existing quads keep working.
///
/// Cloning takes no registry lock (it raises an atomic count); dropping a reference that is not the entry's last takes
/// none either; dropping the last takes it once. A job must not hold one: it holds the [`TextureKey`] (see the module
/// documentation).
pub struct TextureRef<T: Send> {
    inner: Arc<Inner<T>>,
    key: TextureKey,
    refs: Arc<AtomicUsize>,
}

impl<T: Send> TextureRef<T> {
    /// The key of the entry this reference retains.
    #[must_use]
    pub fn key(&self) -> TextureKey {
        self.key
    }

    /// The number of references retaining this entry (this one included) at the moment of the call.
    ///
    /// For tests: a snapshot, which other holders may change at once. It reads the count with the same ordering the references use.
    #[doc(hidden)]
    #[must_use]
    pub fn reference_count(&self) -> usize {
        self.refs.load(Ordering::SeqCst)
    }

    /// Runs `f` on the value, under the registry lock. It works for any holder of a reference, whatever the entry's
    /// public ownership and even after the registry is closed or every registry handle is dropped.
    ///
    /// `f` must be short, must not call the registry, and must not drop a [`TextureRef`] or a large value.
    ///
    /// # Panics
    ///
    /// Only if the registry's own invariant is broken: an entry with a retained reference is always live and holds
    /// its value.
    pub fn with_value<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut state = self.inner.lock();
        let value = state
            .entry(self.key)
            .and_then(|slot| slot.value.as_mut())
            .expect("an entry with a retained reference is live and has its value");
        f(value)
    }
}

impl<T: Send> Clone for TextureRef<T> {
    fn clone(&self) -> Self {
        // This reference holds the count above zero, so the count is never raised from zero here (only `retain`, under
        // the lock, does that).
        if self.refs.fetch_add(1, Ordering::SeqCst) > usize::MAX / 2 {
            std::process::abort();
        }
        Self {
            inner: Arc::clone(&self.inner),
            key: self.key,
            refs: Arc::clone(&self.refs),
        }
    }
}

impl<T: Send> Drop for TextureRef<T> {
    fn drop(&mut self) {
        if self.refs.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.inner.last_reference_dropped(self.key);
        }
    }
}

impl<T: Send> fmt::Debug for TextureRef<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TextureRef")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
impl<T: Send> TextureRegistry<T> {
    /// Whether another holder has the registry lock right now (tests only: a drop probe).
    pub(crate) fn lock_is_held(&self) -> bool {
        matches!(
            self.inner.state.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        )
    }

    /// Runs `body` with the registry lock held (tests only).
    pub(crate) fn with_lock_held<R>(&self, body: impl FnOnce() -> R) -> R {
        let _guard = self.inner.lock();
        body()
    }

    /// How many slots have ever been used, and how many keys wait in the retiring FIFO (tests only).
    pub(crate) fn storage(&self) -> (usize, usize) {
        let state = self.inner.lock();
        (state.slots.len(), state.fifo.len())
    }

    /// The keys waiting in the retiring FIFO, oldest first (tests only).
    pub(crate) fn queued(&self) -> Vec<TextureKey> {
        self.inner.lock().fifo.iter().copied().collect()
    }

    /// Sets the generation of the live entry of `key` and returns the key with it (tests only: reaching
    /// `u64::MAX` takes 2^64 reuses otherwise).
    pub(crate) fn force_generation(&self, key: TextureKey, generation: u64) -> TextureKey {
        let mut state = self.inner.lock();
        let slot = state.entry(key).expect("a live entry");
        slot.generation = generation;
        TextureKey { generation, ..key }
    }

    /// Makes a dropped last reference call `pause` between its decrement and its lock (tests only).
    pub(crate) fn pause_last_drops(&self, pause: Arc<dyn Fn() + Send + Sync>) {
        *self
            .inner
            .drop_pause
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(pause);
    }

    /// Removes the pause (tests only).
    pub(crate) fn unpause_last_drops(&self) {
        *self
            .inner
            .drop_pause
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}
