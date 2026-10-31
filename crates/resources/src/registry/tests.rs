//! CK-106: the texture registry. Every wait is bounded; a test that fails while a thread is held lets it go first.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::*;

/// The longest any test waits for something that must happen.
const BOUND: Duration = Duration::from_secs(10);
/// How long a thread that must not be blocked has to finish.
const SHORT: Duration = Duration::from_secs(5);

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < BOUND, "gave up waiting for {what}");
        thread::yield_now();
    }
}

fn registry(capacity: u32) -> TextureRegistry<u32> {
    TextureRegistry::new(capacity, None).expect("a registry")
}

/// A registry whose retiring wake counts its calls and notes whether the registry lock was held at the time.
struct Woken {
    calls: Arc<AtomicUsize>,
    held: Arc<AtomicBool>,
}

impl Woken {
    fn registry(capacity: u32) -> (TextureRegistry<u32>, Self) {
        let calls = Arc::new(AtomicUsize::new(0));
        let held = Arc::new(AtomicBool::new(false));
        let cell: Arc<Mutex<Option<TextureRegistry<u32>>>> = Arc::new(Mutex::new(None));
        let wake: RetiringWake = {
            let (calls, held, cell) = (Arc::clone(&calls), Arc::clone(&held), Arc::clone(&cell));
            Arc::new(move || {
                if cell
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(TextureRegistry::lock_is_held)
                {
                    held.store(true, Ordering::SeqCst);
                }
                calls.fetch_add(1, Ordering::SeqCst);
            })
        };
        let registry = TextureRegistry::new(capacity, Some(wake)).expect("a registry");
        *cell.lock().unwrap() = Some(registry.clone());
        (registry, Self { calls, held })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn lock_was_held(&self) -> bool {
        self.held.load(Ordering::SeqCst)
    }
}

fn drained(registry: &TextureRegistry<u32>) -> Vec<TextureKey> {
    let mut out = Vec::new();
    registry.drain_retiring(&mut out, usize::MAX);
    out
}

/// Releases, drains and finishes: the whole retirement of an unreferenced entry.
fn retire(registry: &TextureRegistry<u32>, key: TextureKey) -> u32 {
    registry.release(key).expect("released");
    assert_eq!(drained(registry), [key]);
    registry.finish_retirement(key).expect("finished")
}

// ---------------------------------------------------------------------------------------------
// Identities, kinds
// ---------------------------------------------------------------------------------------------

#[test]
fn two_registries_differ_and_one_made_after_a_drop_has_a_new_identity() {
    let first = registry(1);
    let second = registry(1);
    assert_ne!(first.id(), second.id());
    let dropped = first.id();
    drop(first);
    let third = registry(1);
    assert_ne!(third.id(), dropped);
    assert_ne!(third.id(), second.id());
}

#[test]
fn a_registry_needs_a_slot_and_an_identity() {
    assert_eq!(
        TextureRegistry::<u32>::new(0, None).unwrap_err(),
        NewError::ZeroCapacity
    );
    let exhausted = TextureRegistry::<u32>::with_id(1, None, Err(RegistryIdsExhausted));
    assert_eq!(exhausted.unwrap_err(), NewError::Ids(RegistryIdsExhausted));
}

#[test]
fn a_key_of_another_registry_is_foreign_everywhere_and_the_zero_registry_always_is() {
    let (a, b) = (registry(2), registry(2));
    let key = a.create(1).unwrap();
    let foreign = HandleError::Foreign(key);
    assert_eq!(b.check(key), Err(foreign));
    assert_eq!(b.retain(key).unwrap_err(), foreign);
    assert_eq!(b.release(key), Err(foreign));
    assert_eq!(b.with_value(key, |v| *v), Err(foreign));
    assert_eq!(b.with_entry_value(key, |v| *v), Err(foreign));
    assert_eq!(b.finish_retirement(key).unwrap_err(), foreign);
    // The original is untouched by all of that.
    assert_eq!(a.check(key), Ok(()));
    let zero =
        TextureKey::from_token("ck1:tex:0000000000000000:00000000:0000000000000001").unwrap();
    assert_eq!(zero.registry().get(), 0);
    assert_eq!(a.check(zero), Err(HandleError::Foreign(zero)));
}

#[test]
fn a_quad_token_is_the_wrong_kind_and_a_key_round_trips_through_its_token() {
    let quad = "ck1:quad:0000000000000001:00000000:0000000000000001";
    assert_eq!(
        TextureKey::from_token(quad),
        Err(HandleError::WrongKind {
            expected: TokenKind::Texture,
            found: TokenKind::Quad
        })
    );
    assert!(matches!(
        TextureKey::from_token("nonsense"),
        Err(HandleError::Token(TokenError::FieldCount))
    ));
    let registry = registry(1);
    let key = registry.create(5).unwrap();
    let text = key.to_string();
    assert_eq!(text.len(), 50);
    assert_eq!(TextureKey::from_token(&text), Ok(key));
}

// ---------------------------------------------------------------------------------------------
// Stale handles, ownership
// ---------------------------------------------------------------------------------------------

#[test]
fn after_release_every_use_is_stale_and_the_slot_comes_back_with_the_next_generation() {
    let registry = registry(1);
    let old = registry.create(10).unwrap();
    assert_eq!(registry.release(old), Ok(()));
    let stale = HandleError::Stale(old);
    assert_eq!(registry.check(old), Err(stale));
    assert_eq!(registry.retain(old).unwrap_err(), stale);
    assert_eq!(registry.with_value(old, |v| *v), Err(stale));
    assert_eq!(registry.release(old), Err(stale));
    assert_eq!(drained(&registry), [old]);
    assert_eq!(registry.finish_retirement(old), Ok(10));
    let new = registry.create(20).unwrap();
    assert_eq!(
        (new.slot(), new.generation()),
        (old.slot(), old.generation() + 1)
    );
    assert_eq!(registry.check(old), Err(stale));
    assert_eq!(registry.with_value(new, |v| *v), Ok(20));
    assert_eq!(registry.check(new), Ok(()));
}

#[test]
fn a_reference_keeps_a_released_entry_live_and_it_retires_when_the_last_clone_drops() {
    let (registry, woken) = Woken::registry(1);
    let key = registry.create(7).unwrap();
    let first = registry.retain(key).unwrap();
    let second = first.clone();
    let third = second.clone();
    assert_eq!(first.key(), key);
    registry.release(key).unwrap();
    assert_eq!(registry.stats().live, 1);
    assert_eq!(registry.stats().retiring, 0);
    assert_eq!(registry.create(1), Err(CreateError::Full { capacity: 1 }));
    // The reference still reaches the value though the public token is gone.
    assert_eq!(first.with_value(|v| std::mem::replace(v, 8)), 7);
    assert_eq!(
        registry.with_value(key, |v| *v),
        Err(HandleError::Stale(key))
    );
    drop(first);
    drop(second);
    assert_eq!(
        (registry.stats().retiring, woken.calls()),
        (0, 0),
        "a clone is still held"
    );
    drop(third);
    assert_eq!((registry.stats().live, registry.stats().retiring), (0, 1));
    assert_eq!(woken.calls(), 1);
    assert_eq!(drained(&registry), [key]);
    assert_eq!(registry.finish_retirement(key), Ok(8));
}

#[test]
fn reference_count_follows_clone_and_drop() {
    let (registry, _woken) = Woken::registry(1);
    let key = registry.create(7).unwrap();
    let first = registry.retain(key).unwrap();
    assert_eq!(first.reference_count(), 1);
    let second = first.clone();
    assert_eq!(first.reference_count(), 2);
    assert_eq!(second.reference_count(), 2);
    drop(second);
    assert_eq!(first.reference_count(), 1);
}

#[test]
fn releasing_after_every_reference_dropped_retires_at_once() {
    let (registry, woken) = Woken::registry(2);
    let key = registry.create(1).unwrap();
    let reference = registry.retain(key).unwrap();
    drop(reference);
    // Public ownership keeps it live though no reference is left.
    assert_eq!(
        (
            registry.stats().live,
            registry.stats().retiring,
            woken.calls()
        ),
        (1, 0, 0)
    );
    registry.release(key).unwrap();
    assert_eq!(
        (
            registry.stats().live,
            registry.stats().retiring,
            woken.calls()
        ),
        (0, 1, 1)
    );
}

// ---------------------------------------------------------------------------------------------
// Capacity and retirement order
// ---------------------------------------------------------------------------------------------

#[test]
fn retiring_entries_count_as_used_until_their_retirement_finishes() {
    let registry = registry(1);
    let key = registry.create(1).unwrap();
    registry.release(key).unwrap();
    assert_eq!(registry.create(2), Err(CreateError::Full { capacity: 1 }));
    assert_eq!(registry.stats().free, 0);
    drained(&registry);
    registry.finish_retirement(key).unwrap();
    assert_eq!(registry.stats().free, 1);
    assert!(registry.create(2).is_ok());
}

#[test]
fn drain_returns_at_most_the_limit_oldest_first_and_never_the_same_key_twice() {
    let registry = registry(4);
    let keys: Vec<_> = (0..3).map(|n| registry.create(n).unwrap()).collect();
    for index in [1, 0, 2] {
        registry.release(keys[index]).unwrap();
    }
    let mut out = Vec::new();
    assert_eq!(registry.drain_retiring(&mut out, 2), 2);
    assert_eq!(out, [keys[1], keys[0]]);
    assert_eq!(registry.drain_retiring(&mut out, 0), 0);
    assert_eq!(registry.drain_retiring(&mut out, 5), 1);
    assert_eq!(out, [keys[1], keys[0], keys[2]]);
    assert_eq!(registry.drain_retiring(&mut out, 5), 0);
    assert_eq!(out.len(), 3);
}

#[test]
fn finish_refuses_a_live_entry_a_stale_generation_and_a_key_already_finished() {
    let registry = registry(1);
    let key = registry.create(1).unwrap();
    assert_eq!(
        registry.finish_retirement(key),
        Err(HandleError::Stale(key))
    );
    registry.release(key).unwrap();
    drained(&registry);
    let wrong = TextureKey {
        generation: key.generation() + 1,
        ..key
    };
    assert_eq!(
        registry.finish_retirement(wrong),
        Err(HandleError::Stale(wrong))
    );
    assert_eq!(registry.finish_retirement(key), Ok(1));
    assert_eq!(
        registry.finish_retirement(key),
        Err(HandleError::Stale(key))
    );
    let beyond = TextureKey { slot: 5, ..key };
    assert_eq!(
        registry.finish_retirement(beyond),
        Err(HandleError::Stale(beyond))
    );
}

#[test]
fn finish_refuses_a_retiring_key_that_was_not_drained_and_changes_nothing() {
    let registry = registry(1);
    let key = registry.create(9).unwrap();
    registry.release(key).unwrap();
    for _ in 0..3 {
        assert_eq!(
            registry.finish_retirement(key),
            Err(HandleError::NotDrained(key))
        );
        assert_eq!(registry.queued(), [key], "still queued");
        assert_eq!(registry.stats().retiring, 1);
        assert_eq!(
            registry.with_entry_value(key, |v| *v),
            Ok(9),
            "still the same entry"
        );
    }
    assert_eq!(drained(&registry), [key]);
    assert_eq!(registry.finish_retirement(key), Ok(9));
}

#[test]
fn a_slot_whose_generation_is_used_up_is_never_reused() {
    let registry = registry(1);
    let first = registry.create(1).unwrap();
    let last = registry.force_generation(first, u64::MAX);
    assert_eq!(last.generation(), u64::MAX);
    registry.release(last).unwrap();
    drained(&registry);
    assert_eq!(registry.finish_retirement(last), Ok(1));
    let stats = registry.stats();
    assert_eq!(
        (stats.exhausted, stats.free, stats.live, stats.retiring),
        (1, 0, 0, 0)
    );
    for _ in 0..3 {
        assert_eq!(registry.create(2), Err(CreateError::Full { capacity: 1 }));
    }
    // The generation did not wrap to a value an old token could match.
    assert_eq!(registry.check(last), Err(HandleError::Stale(last)));
    let wrapped = TextureKey {
        generation: 0,
        ..last
    };
    assert_eq!(registry.check(wrapped), Err(HandleError::Stale(wrapped)));
    assert_eq!(
        registry.finish_retirement(last),
        Err(HandleError::Stale(last))
    );
}

#[test]
fn an_exhausted_slot_leaves_the_others_working() {
    let registry = registry(2);
    let burnt = registry.create(1).unwrap();
    let burnt = registry.force_generation(burnt, u64::MAX);
    registry.release(burnt).unwrap();
    drained(&registry);
    registry.finish_retirement(burnt).unwrap();
    let key = registry.create(2).unwrap();
    assert_ne!(key.slot(), burnt.slot());
    assert_eq!(registry.create(3), Err(CreateError::Full { capacity: 2 }));
    assert_eq!(retire(&registry, key), 2);
    let again = registry.create(4).unwrap();
    assert_eq!(
        (again.slot(), again.generation()),
        (key.slot(), key.generation() + 1)
    );
}

#[test]
fn the_oldest_freed_slot_is_taken_first_then_never_used_ones() {
    let registry = registry(3);
    let a = registry.create(0).unwrap();
    let b = registry.create(1).unwrap();
    registry.release(b).unwrap();
    registry.release(a).unwrap();
    drained(&registry);
    registry.finish_retirement(b).unwrap();
    registry.finish_retirement(a).unwrap();
    assert_eq!(
        registry.create(5).unwrap().slot(),
        b.slot(),
        "oldest freed first"
    );
    assert_eq!(registry.create(6).unwrap().slot(), a.slot());
    assert_eq!(
        registry.create(7).unwrap().slot(),
        2,
        "then the next never-used slot"
    );
}

// ---------------------------------------------------------------------------------------------
// Values outside the lock
// ---------------------------------------------------------------------------------------------

/// A value whose drop records whether the registry lock was held at the time.
struct Probe {
    held: Arc<dyn Fn() -> bool + Send + Sync>,
    dropped: Arc<Mutex<Vec<bool>>>,
}

impl Drop for Probe {
    fn drop(&mut self) {
        let held = (self.held)();
        self.dropped.lock().unwrap().push(held);
    }
}

/// What each drop of a `Probe` saw: whether the registry lock was held.
type Dropped = Arc<Mutex<Vec<bool>>>;

/// Answers whether the registry's lock is held right now. It holds the state weakly, so a value in it can carry it.
type Held = Arc<dyn Fn() -> bool + Send + Sync>;

fn probed(capacity: u32) -> (TextureRegistry<Probe>, Dropped, Held) {
    let registry = TextureRegistry::<Probe>::new(capacity, None).unwrap();
    let weak = Arc::downgrade(&registry.inner);
    let held: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
        weak.upgrade().is_some_and(|inner| {
            matches!(
                inner.state.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            )
        })
    });
    (registry, Arc::new(Mutex::new(Vec::new())), held)
}

fn probe(held: &Arc<dyn Fn() -> bool + Send + Sync>, dropped: &Arc<Mutex<Vec<bool>>>) -> Probe {
    Probe {
        held: Arc::clone(held),
        dropped: Arc::clone(dropped),
    }
}

#[test]
fn values_are_dropped_with_the_lock_free_and_close_drops_none() {
    let (registry, dropped, held) = probed(4);
    let finished = registry.create(probe(&held, &dropped)).unwrap();
    let _public_only = registry.create(probe(&held, &dropped)).unwrap();
    let referenced = registry.create(probe(&held, &dropped)).unwrap();
    let reference = registry.retain(referenced).unwrap();
    let _retiring = registry.create(probe(&held, &dropped)).unwrap();
    // finish_retirement hands the value back: it is dropped here, after the call, with the lock free.
    registry.release(finished).unwrap();
    let mut out = Vec::new();
    registry.drain_retiring(&mut out, 8);
    let value = registry.finish_retirement(finished).unwrap();
    assert!(
        dropped.lock().unwrap().is_empty(),
        "not dropped inside the registry"
    );
    drop(value);
    assert_eq!(*dropped.lock().unwrap(), [false]);
    // close drops no value, wherever it leaves the entry (retiring, or live through a reference).
    registry.close();
    assert_eq!(dropped.lock().unwrap().len(), 1);
    drop(reference);
    assert_eq!(
        dropped.lock().unwrap().len(),
        1,
        "the last reference retires the entry, it drops nothing"
    );
    // The last handle goes with three entries unfinished: their values are dropped with the shared state.
    drop(registry);
    let seen = dropped.lock().unwrap().clone();
    assert_eq!(seen.len(), 4, "{seen:?}");
    assert!(
        seen.iter().all(|held| !held),
        "a value was dropped under the lock"
    );
}

// ---------------------------------------------------------------------------------------------
// No lock for clones
// ---------------------------------------------------------------------------------------------

/// Holds the registry lock on another thread until released; returns the release sender and the thread.
fn hold_lock<'scope>(
    scope: &'scope thread::Scope<'scope, '_>,
    registry: &'scope TextureRegistry<u32>,
) -> Sender<()> {
    let (held_tx, held) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    scope.spawn(move || {
        registry.with_lock_held(|| {
            held_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(BOUND);
        });
    });
    held.recv_timeout(BOUND).expect("the lock is held");
    release
}

#[test]
fn cloning_a_reference_and_dropping_a_non_last_clone_need_no_lock() {
    let registry = registry(2);
    let key = registry.create(1).unwrap();
    let reference = registry.retain(key).unwrap();
    thread::scope(|scope| {
        let release = hold_lock(scope, &registry);
        assert!(registry.lock_is_held());
        let (done_tx, done) = mpsc::channel();
        scope.spawn({
            let reference = &reference;
            move || {
                let clone = reference.clone();
                let another = clone.clone();
                drop(clone);
                drop(another);
                done_tx.send(()).unwrap();
            }
        });
        let finished = done.recv_timeout(SHORT);
        release.send(()).unwrap();
        assert!(
            finished.is_ok(),
            "clone or non-last drop waited for the registry lock"
        );
    });
    drop(reference);
}

#[test]
fn dropping_the_last_reference_does_take_the_lock() {
    // The control for the test above: the last drop reconciles under the lock, so it waits for a held lock.
    let registry = registry(2);
    let key = registry.create(2).unwrap();
    let reference = registry.retain(key).unwrap();
    registry.release(key).unwrap();
    thread::scope(|scope| {
        let release = hold_lock(scope, &registry);
        let (done_tx, done) = mpsc::channel();
        scope.spawn(move || {
            drop(reference);
            done_tx.send(()).unwrap();
        });
        assert!(
            done.recv_timeout(Duration::from_millis(300)).is_err(),
            "the last drop finished while the lock was held"
        );
        release.send(()).unwrap();
        done.recv_timeout(BOUND)
            .expect("it finishes once the lock is free");
    });
    assert_eq!(registry.stats().retiring, 1);
}

// ---------------------------------------------------------------------------------------------
// The owner's accessor
// ---------------------------------------------------------------------------------------------

#[test]
fn the_owners_accessor_reaches_an_entry_whatever_its_ownership() {
    let registry = registry(2);
    let key = registry.create(1).unwrap();
    // With public ownership.
    assert_eq!(
        registry.with_entry_value(key, |v| std::mem::replace(v, 2)),
        Ok(1)
    );
    assert_eq!(registry.with_value(key, |v| *v), Ok(2));
    // Public token released, a reference keeps it live: with_value is stale, the accessor is not.
    let reference = registry.retain(key).unwrap();
    registry.release(key).unwrap();
    assert_eq!(
        registry.with_value(key, |v| *v),
        Err(HandleError::Stale(key))
    );
    assert_eq!(
        registry.with_entry_value(key, |v| std::mem::replace(v, 3)),
        Ok(2)
    );
    assert_eq!(reference.with_value(|v| *v), 3);
    // Retiring: with_value and retain are stale, the accessor reaches it.
    drop(reference);
    assert_eq!(registry.stats().retiring, 1);
    assert_eq!(
        registry.with_value(key, |v| *v),
        Err(HandleError::Stale(key))
    );
    assert_eq!(registry.retain(key).unwrap_err(), HandleError::Stale(key));
    assert_eq!(
        registry.with_entry_value(key, |v| std::mem::replace(v, 4)),
        Ok(3)
    );
    // After a drain, still retiring.
    drained(&registry);
    assert_eq!(registry.with_entry_value(key, |v| *v), Ok(4));
    // After close.
    let other = registry.create(10).unwrap();
    registry.close();
    assert_eq!(registry.with_entry_value(other, |v| *v), Ok(10));
    assert_eq!(registry.with_value(other, |v| *v), Err(HandleError::Closed));
    // Stale after the retirement finished, and for the old key once the slot is reused.
    assert_eq!(registry.finish_retirement(key), Ok(4));
    assert_eq!(
        registry.with_entry_value(key, |v| *v),
        Err(HandleError::Stale(key))
    );
}

#[test]
fn the_owners_accessor_does_not_reach_a_reused_slot_through_an_old_key() {
    let registry = registry(1);
    let old = registry.create(1).unwrap();
    retire(&registry, old);
    let new = registry.create(2).unwrap();
    assert_eq!(
        registry.with_entry_value(old, |v| *v),
        Err(HandleError::Stale(old))
    );
    assert_eq!(registry.with_entry_value(new, |v| *v), Ok(2));
}

// ---------------------------------------------------------------------------------------------
// The retiring wake
// ---------------------------------------------------------------------------------------------

#[test]
fn the_last_reference_dropped_on_another_thread_wakes_the_owner_once_and_the_key_is_queued() {
    let (registry, woken) = Woken::registry(2);
    let key = registry.create(1).unwrap();
    let reference = registry.retain(key).unwrap();
    registry.release(key).unwrap();
    assert_eq!(woken.calls(), 0);
    let dropper = thread::spawn(move || drop(reference));
    // No further registry call from this thread: the hook alone tells it.
    wait_until("the retiring wake", || woken.calls() >= 1);
    dropper.join().unwrap();
    assert_eq!(woken.calls(), 1);
    assert_eq!(drained(&registry), [key]);
    assert!(
        !woken.lock_was_held(),
        "the wake ran with the registry lock held"
    );
}

#[test]
fn release_and_close_wake_too() {
    let (registry, woken) = Woken::registry(3);
    let released = registry.create(1).unwrap();
    registry.release(released).unwrap();
    assert_eq!(woken.calls(), 1);
    drained(&registry);
    let (a, b) = (registry.create(2).unwrap(), registry.create(3).unwrap());
    registry.close();
    assert_eq!(woken.calls(), 2, "one wake for both entries close retired");
    assert_eq!(drained(&registry), [a, b]);
    registry.close();
    assert_eq!(woken.calls(), 2, "a second close changes nothing");
    assert!(!woken.lock_was_held());
}

#[test]
fn entries_retiring_before_a_drain_give_one_wake_and_both_keys() {
    let (registry, woken) = Woken::registry(3);
    let (a, b) = (registry.create(1).unwrap(), registry.create(2).unwrap());
    registry.release(a).unwrap();
    registry.release(b).unwrap();
    assert_eq!(woken.calls(), 1);
    assert_eq!(drained(&registry), [a, b]);
}

#[test]
fn a_partial_drain_gives_no_new_wake_and_an_empty_fifo_wakes_again() {
    let (registry, woken) = Woken::registry(4);
    let keys: Vec<_> = (0..3).map(|n| registry.create(n).unwrap()).collect();
    registry.release(keys[0]).unwrap();
    registry.release(keys[1]).unwrap();
    assert_eq!(woken.calls(), 1);
    let mut out = Vec::new();
    registry.drain_retiring(&mut out, 1);
    // One key is still queued: the FIFO, not the wake count, holds the work.
    registry.release(keys[2]).unwrap();
    assert_eq!(woken.calls(), 1);
    registry.drain_retiring(&mut out, 8);
    assert_eq!(out, keys);
    let more = registry.create(9).unwrap();
    registry.release(more).unwrap();
    assert_eq!(woken.calls(), 2, "the FIFO had been emptied");
}

#[test]
fn without_a_wake_the_owner_polls() {
    let registry = registry(1);
    let key = registry.create(1).unwrap();
    registry.release(key).unwrap();
    assert_eq!(drained(&registry), [key]);
}

// ---------------------------------------------------------------------------------------------
// Close
// ---------------------------------------------------------------------------------------------

#[test]
fn after_close_the_key_operations_are_closed_and_retirement_goes_on() {
    let registry = registry(4);
    let plain = registry.create(1).unwrap();
    let referenced = registry.create(2).unwrap();
    let reference = registry.retain(referenced).unwrap();
    registry.close();
    assert_eq!(registry.create(3), Err(CreateError::Closed));
    assert_eq!(registry.check(plain), Err(HandleError::Closed));
    assert_eq!(registry.retain(plain).unwrap_err(), HandleError::Closed);
    assert_eq!(registry.release(plain), Err(HandleError::Closed));
    assert_eq!(registry.with_value(plain, |v| *v), Err(HandleError::Closed));
    // Foreign still wins over Closed.
    let other = self::registry(1).create(1).unwrap();
    assert_eq!(registry.check(other), Err(HandleError::Foreign(other)));
    // The entry without a reference is retiring and drainable; the referenced one waits for its reference.
    assert_eq!(registry.queued(), [plain]);
    assert_eq!(registry.stats().live, 1);
    assert_eq!(reference.with_value(|v| *v), 2);
    drop(reference);
    assert_eq!(registry.queued(), [plain, referenced]);
    assert_eq!(drained(&registry), [plain, referenced]);
    assert_eq!(registry.finish_retirement(plain), Ok(1));
    assert_eq!(registry.finish_retirement(referenced), Ok(2));
    let before = registry.stats();
    registry.close();
    assert_eq!(registry.stats(), before);
    assert_eq!(registry.create(1), Err(CreateError::Closed));
}

#[test]
fn a_reference_outlives_every_registry_handle_and_the_last_drop_frees_the_values() {
    let (registry, dropped, held) = probed(2);
    let key = registry.create(probe(&held, &dropped)).unwrap();
    let reference = registry.retain(key).unwrap();
    let other = registry.create(probe(&held, &dropped)).unwrap();
    registry.release(other).unwrap();
    drop(registry);
    assert!(
        dropped.lock().unwrap().is_empty(),
        "the state is alive while a reference is"
    );
    assert_eq!(reference.with_value(|_| 5), 5);
    let clone = reference.clone();
    drop(reference);
    assert!(dropped.lock().unwrap().is_empty());
    drop(clone);
    // The last reference went: the shared state and the remaining values, with no lock held.
    let seen = dropped.lock().unwrap().clone();
    assert_eq!(seen, [false, false]);
}

// ---------------------------------------------------------------------------------------------
// The last-drop race
// ---------------------------------------------------------------------------------------------

/// A pause for the last-drop action: it announces itself and waits for the test, at most `BOUND`.
struct Pause {
    arrived: Receiver<()>,
    go: Sender<()>,
}

impl Pause {
    fn arm(registry: &TextureRegistry<u32>) -> Self {
        let (arrived_tx, arrived) = mpsc::channel();
        let (go, go_rx) = mpsc::channel::<()>();
        let state = Mutex::new((arrived_tx, go_rx));
        registry.pause_last_drops(Arc::new(move || {
            let state = state.lock().unwrap();
            let _ = state.0.send(());
            let _ = state.1.recv_timeout(BOUND);
        }));
        Self { arrived, go }
    }

    /// Waits for a last drop to be between its decrement and its lock.
    fn wait_paused(&self) {
        self.arrived
            .recv_timeout(BOUND)
            .expect("a last drop paused");
    }

    /// Lets the paused action go on (and the next ones not pause).
    fn resume(&self, registry: &TextureRegistry<u32>) {
        registry.unpause_last_drops();
        self.go.send(()).unwrap();
    }
}

/// Drops `reference` on its own thread, which pauses between the decrement and the lock.
fn drop_paused(
    registry: &TextureRegistry<u32>,
    reference: TextureRef<u32>,
) -> (Pause, thread::JoinHandle<()>) {
    let pause = Pause::arm(registry);
    let dropper = thread::spawn(move || drop(reference));
    pause.wait_paused();
    (pause, dropper)
}

#[test]
fn a_retain_between_the_last_decrement_and_the_lock_keeps_the_entry_live() {
    let (registry, woken) = Woken::registry(2);
    let key = registry.create(1).unwrap();
    let reference = registry.retain(key).unwrap();
    let (pause, dropper) = drop_paused(&registry, reference);
    // The count is zero and the last-drop action has not locked yet: a new reference arrives.
    let second = registry.retain(key).unwrap();
    pause.resume(&registry);
    dropper.join().unwrap();
    assert_eq!((registry.stats().live, registry.stats().retiring), (1, 0));
    assert!(registry.queued().is_empty());
    // The new reference's own last drop does not retire it either while the public token lives; the release does.
    drop(second);
    assert_eq!(registry.stats().retiring, 0);
    registry.release(key).unwrap();
    assert_eq!(registry.queued(), [key]);
    assert_eq!(woken.calls(), 1);
}

#[test]
fn a_release_and_a_retain_between_the_decrement_and_the_lock_retire_it_exactly_once_later() {
    let (registry, woken) = Woken::registry(2);
    let key = registry.create(1).unwrap();
    let reference = registry.retain(key).unwrap();
    let (pause, dropper) = drop_paused(&registry, reference);
    let second = registry.retain(key).unwrap();
    registry.release(key).unwrap();
    assert!(
        registry.queued().is_empty(),
        "a reference exists: not retiring"
    );
    pause.resume(&registry);
    dropper.join().unwrap();
    // The older last-drop action found a reference and did nothing.
    assert_eq!((registry.stats().live, registry.stats().retiring), (1, 0));
    drop(second);
    assert_eq!(registry.queued(), [key]);
    assert_eq!(registry.stats().retiring, 1);
    assert_eq!(woken.calls(), 1);
}

#[test]
fn a_release_between_the_decrement_and_the_lock_retires_it_and_the_older_action_does_nothing() {
    let (registry, woken) = Woken::registry(2);
    let key = registry.create(1).unwrap();
    let reference = registry.retain(key).unwrap();
    let (pause, dropper) = drop_paused(&registry, reference);
    // The count is already zero: the release retires the entry itself.
    registry.release(key).unwrap();
    assert_eq!(registry.queued(), [key]);
    pause.resume(&registry);
    dropper.join().unwrap();
    assert_eq!(registry.queued(), [key], "retired exactly once");
    assert_eq!(registry.stats().retiring, 1);
    assert_eq!(woken.calls(), 1);
}

#[test]
fn an_older_last_drop_cannot_retire_a_later_incarnation_of_the_slot() {
    let registry = registry(1);
    let old = registry.create(1).unwrap();
    let reference = registry.retain(old).unwrap();
    let (pause, dropper) = drop_paused(&registry, reference);
    // The old incarnation retires, is finished and the slot is reused, all while the action waits.
    assert_eq!(retire(&registry, old), 1);
    let new = registry.create(2).unwrap();
    assert_eq!(new.slot(), old.slot());
    let held = registry.retain(new).unwrap();
    pause.resume(&registry);
    dropper.join().unwrap();
    assert_eq!(
        registry.check(new),
        Ok(()),
        "the new incarnation is untouched"
    );
    assert!(registry.queued().is_empty());
    drop(held);
    registry.release(new).unwrap();
    assert_eq!(registry.queued(), [new]);
}

#[test]
fn many_threads_retaining_cloning_and_dropping_never_retire_a_live_entry_and_it_retires_once() {
    let (registry, woken) = Woken::registry(2);
    let key = registry.create(1).unwrap();
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..2000 {
                    // The public token is live throughout, so every retain must succeed.
                    let reference = registry.retain(key).expect("the entry never retired early");
                    let clone = reference.clone();
                    drop(reference);
                    drop(clone);
                }
            });
        }
    });
    assert_eq!((registry.stats().live, registry.stats().retiring), (1, 0));
    assert_eq!(woken.calls(), 0);
    // References racing the release: afterwards it retires exactly once.
    let references: Vec<_> = (0..8).map(|_| registry.retain(key).unwrap()).collect();
    thread::scope(|scope| {
        scope.spawn(|| registry.release(key).unwrap());
        for reference in references {
            scope.spawn(move || {
                let clone = reference.clone();
                drop(reference);
                drop(clone);
            });
        }
    });
    assert_eq!(registry.queued(), [key]);
    assert_eq!((registry.stats().live, registry.stats().retiring), (0, 1));
    assert_eq!(woken.calls(), 1);
}

// ---------------------------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------------------------

#[test]
fn churn_never_grows_storage_or_the_fifo_past_the_capacity() {
    let registry = registry(3);
    for round in 0..200_u32 {
        let keys: Vec<_> = (0..3)
            .map(|n| registry.create(round * 3 + n).unwrap())
            .collect();
        assert_eq!(registry.create(0), Err(CreateError::Full { capacity: 3 }));
        for key in &keys {
            registry.release(*key).unwrap();
        }
        let (slots, queued) = registry.storage();
        assert!(slots <= 3 && queued <= 3, "{slots} slots, {queued} queued");
        for key in drained(&registry) {
            registry.finish_retirement(key).unwrap();
        }
    }
    assert_eq!(registry.storage(), (3, 0));
    assert_eq!(registry.stats().free, 3);
}

#[test]
fn capacity_one_churn_with_an_undrained_finish_in_between_keeps_the_fifo_at_one() {
    let registry = registry(1);
    let mut generation = 1;
    for _ in 0..500 {
        let key = registry.create(0).unwrap();
        assert_eq!(key.generation(), generation);
        registry.release(key).unwrap();
        // Finishing before the drain is refused: the key stays queued and the slot is not reused.
        assert_eq!(
            registry.finish_retirement(key),
            Err(HandleError::NotDrained(key))
        );
        assert_eq!(registry.create(1), Err(CreateError::Full { capacity: 1 }));
        assert_eq!(registry.queued(), [key]);
        assert_eq!(drained(&registry), [key]);
        assert_eq!(registry.finish_retirement(key), Ok(0));
        let (slots, queued) = registry.storage();
        assert_eq!(
            (slots, queued),
            (1, 0),
            "no key of a finished generation stays queued"
        );
        generation += 1;
    }
}

#[test]
fn storage_grows_on_demand_and_not_before() {
    let registry = registry(1000);
    assert_eq!(registry.storage(), (0, 0));
    for n in 0..5 {
        registry.create(n).unwrap();
    }
    assert_eq!(registry.storage().0, 5);
    assert_eq!(registry.stats().free, 995);
}

// ---------------------------------------------------------------------------------------------
// Auto traits, messages
// ---------------------------------------------------------------------------------------------

#[test]
fn the_registry_and_its_references_are_send_and_sync_for_a_value_that_is_only_send() {
    fn assert_send_sync<X: Send + Sync>() {}
    assert_send_sync::<TextureRegistry<std::cell::Cell<u32>>>();
    assert_send_sync::<TextureRef<std::cell::Cell<u32>>>();
    assert_send_sync::<TextureKey>();
}

#[test]
fn an_error_message_gives_the_slot_and_the_generation_as_numbers() {
    let registry = registry(1);
    let key = registry.create(1).unwrap();
    registry.release(key).unwrap();
    let message = registry.check(key).unwrap_err().to_string();
    assert!(
        message.contains(&key.slot().to_string())
            && message.contains(&key.generation().to_string()),
        "{message}"
    );
    let message = registry.finish_retirement(key).unwrap_err().to_string();
    assert!(
        message.contains("slot 0") && message.contains("generation 1"),
        "{message}"
    );
    assert!(HandleError::Closed.to_string().contains("closed"));
    assert!(CreateError::Full { capacity: 7 }.to_string().contains('7'));
}

// ---------------------------------------------------------------------------------------------
// The service layer's view (CK-107): lifecycle under the lock, and the walk over every entry
// ---------------------------------------------------------------------------------------------

#[test]
fn the_lifecycle_of_an_entry_is_seen_under_the_lock_for_exactly_its_generation() {
    let registry = registry(2);
    let key = registry.create(1).unwrap();
    let lifecycle = |key| registry.with_entry_lifecycle(key, |life, _| life);
    assert_eq!(
        lifecycle(key),
        Ok(Lifecycle::Live {
            public: true,
            referenced: false
        })
    );
    let reference = registry.retain(key).unwrap();
    assert_eq!(
        lifecycle(key),
        Ok(Lifecycle::Live {
            public: true,
            referenced: true
        })
    );
    registry.release(key).unwrap();
    assert_eq!(
        lifecycle(key),
        Ok(Lifecycle::Live {
            public: false,
            referenced: true
        })
    );
    drop(reference);
    assert_eq!(lifecycle(key), Ok(Lifecycle::Retiring));
    // The closure may change the value.
    assert_eq!(
        registry.with_entry_lifecycle(key, |_, v| std::mem::replace(v, 5)),
        Ok(1)
    );
    assert_eq!(registry.with_entry_value(key, |v| *v), Ok(5));
    drained(&registry);
    registry.finish_retirement(key).unwrap();
    assert_eq!(lifecycle(key), Err(HandleError::Stale(key)));
    let other = self::registry(1).create(1).unwrap();
    assert_eq!(lifecycle(other), Err(HandleError::Foreign(other)));
    // The new incarnation: the old key still reaches nothing.
    let new = registry.create(2).unwrap();
    assert_eq!(lifecycle(key), Err(HandleError::Stale(key)));
    assert_eq!(
        lifecycle(new),
        Ok(Lifecycle::Live {
            public: true,
            referenced: false
        })
    );
}

#[test]
fn the_walk_over_entries_visits_live_and_retiring_ones_and_not_free_ones() {
    let registry = registry(4);
    let live = registry.create(1).unwrap();
    let retiring = registry.create(2).unwrap();
    let finished = registry.create(3).unwrap();
    registry.release(retiring).unwrap();
    registry.release(finished).unwrap();
    drained(&registry);
    registry.finish_retirement(finished).unwrap();
    let mut seen = Vec::new();
    registry.for_each_entry(|key, life, value| {
        seen.push((key, life, *value));
        *value += 10;
    });
    assert_eq!(
        seen,
        [
            (
                live,
                Lifecycle::Live {
                    public: true,
                    referenced: false
                },
                1
            ),
            (retiring, Lifecycle::Retiring, 2)
        ]
    );
    assert_eq!(registry.with_entry_value(live, |v| *v), Ok(11));
}

#[test]
fn a_refused_create_hands_the_value_back_without_dropping_it_under_the_lock() {
    let (registry, dropped, held) = probed(1);
    let key = registry.create(probe(&held, &dropped)).unwrap();
    let (error, value) = registry
        .create_returning(probe(&held, &dropped))
        .map(|_| ())
        .unwrap_err();
    assert_eq!(error, CreateError::Full { capacity: 1 });
    assert!(
        dropped.lock().unwrap().is_empty(),
        "the value came back, not dropped"
    );
    drop(value);
    assert_eq!(*dropped.lock().unwrap(), [false]);
    registry.close();
    let (error, value) = registry
        .create_returning(probe(&held, &dropped))
        .map(|_| ())
        .unwrap_err();
    assert_eq!(error, CreateError::Closed);
    drop(value);
    let _ = key;
}
