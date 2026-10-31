//! How the crate disposes of the payload of a panic it caught.
//!
//! The standard library documents that dropping a payload returned by `catch_unwind` (or by
//! `JoinHandle::join`) can itself panic: the payload is any `Send` value, and its `Drop` is arbitrary code.
//! A payload dropped implicitly, by a `_` pattern, a closure that ignores it or a discarded `Result`, would
//! then unwind out of the boundary that caught the panic, skipping the cleanup and the result that boundary
//! exists to produce. Every place in this crate that catches an unwind or joins a thread that may have
//! panicked therefore hands the payload to [`dispose`], and nothing else drops one.

use std::any::Any;
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread::JoinHandle;

/// The payload of a caught panic.
pub(crate) type Payload = Box<dyn Any + Send>;

/// Drops `payload` without ever unwinding. The drop runs inside its own `catch_unwind`; if it panics, the
/// second payload is leaked with `mem::forget` (dropping it could panic in turn, without end). Returns
/// normally in every case.
pub(crate) fn dispose(payload: Payload) {
    if let Err(second) = catch_unwind(AssertUnwindSafe(move || drop(payload))) {
        mem::forget(second);
    }
}

/// Joins a thread that has finished, and disposes of the payload of the panic that ended it, if one did.
/// The thread's panic is the thread's own: it never becomes the joiner's.
pub(crate) fn join(handle: JoinHandle<()>) {
    if let Err(payload) = handle.join() {
        dispose(payload);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{Payload, dispose, join};
    use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    /// A payload whose `Drop` panics with `then`, counting its own drop.
    pub(crate) struct Bomb {
        pub(crate) drops: Arc<AtomicUsize>,
        pub(crate) then: Option<Payload>,
    }

    impl Drop for Bomb {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
            match self.then.take() {
                Some(payload) => std::panic::resume_unwind(payload),
                None => panic!("a payload whose drop panics"),
            }
        }
    }

    /// A payload that only counts its drop.
    struct Counted(Arc<AtomicUsize>);

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn counter() -> Arc<AtomicUsize> {
        Arc::new(AtomicUsize::new(0))
    }

    /// Raises a `Bomb` that counts its drop in `drops`.
    pub(crate) fn explode(drops: &Arc<AtomicUsize>) -> ! {
        panic_any(Bomb {
            drops: Arc::clone(drops),
            then: None,
        })
    }

    pub(crate) fn caught<M: std::any::Any + Send>(payload: M) -> Payload {
        catch_unwind(AssertUnwindSafe(|| panic_any(payload))).expect_err("the closure panics")
    }

    #[test]
    fn an_ordinary_payload_is_dropped_once_and_disposal_returns() {
        let drops = counter();
        dispose(caught(Counted(Arc::clone(&drops))));
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_payload_whose_drop_panics_does_not_unwind_out_of_disposal() {
        let drops = counter();
        let payload = caught(Bomb {
            drops: Arc::clone(&drops),
            then: None,
        });
        // The caller's own unwind boundary: nothing may reach it.
        let escaped = catch_unwind(AssertUnwindSafe(|| dispose(payload)));
        assert!(escaped.is_ok(), "the payload's panic escaped disposal");
        assert_eq!(drops.load(Ordering::SeqCst), 1, "dropped exactly once");
    }

    #[test]
    fn a_payload_whose_drop_panics_with_such_a_payload_is_leaked_not_recursed_into() {
        let outer = counter();
        let inner = counter();
        let second: Payload = Box::new(Bomb {
            drops: Arc::clone(&inner),
            then: None,
        });
        let payload = caught(Bomb {
            drops: Arc::clone(&outer),
            then: Some(second),
        });
        let escaped = catch_unwind(AssertUnwindSafe(|| dispose(payload)));
        assert!(escaped.is_ok(), "a payload's panic escaped disposal");
        assert_eq!(outer.load(Ordering::SeqCst), 1);
        assert_eq!(
            inner.load(Ordering::SeqCst),
            0,
            "the second payload was dropped (its drop would panic again) instead of leaked"
        );
    }

    #[test]
    fn a_thread_that_ended_by_such_a_panic_is_joined_without_unwinding() {
        let drops = counter();
        let thread = thread::spawn({
            let drops = Arc::clone(&drops);
            move || explode(&drops)
        });
        while !thread.is_finished() {
            thread::yield_now();
        }
        let escaped = catch_unwind(AssertUnwindSafe(|| join(thread)));
        assert!(escaped.is_ok(), "the thread's payload escaped the join");
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
