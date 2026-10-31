//! How the crate disposes of the payload of a panic it caught.
//!
//! The standard library documents that dropping a payload returned by `catch_unwind` can itself panic (the
//! payload is any `Send` value, and its `Drop` is arbitrary code). The writer thread's panic payload comes
//! from the sink, which is the caller's code. If the thread ended with it, the payload would be dropped
//! outside any boundary of this crate: by the thread that drops the logger (`JoinHandle::join`), or, when
//! the handle was detached, by the standard library as the thread ends, which aborts the process if that
//! drop panics (`Packet::drop`). The writer therefore catches its own panic and hands the payload to
//! [`dispose`], and nothing else drops one.

use std::any::Any;
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};

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

/// A payload whose `Drop` panics with `then` (or with a text), counting its own drop and noting which
/// thread ran it.
#[cfg(test)]
pub(crate) struct Bomb {
    pub(crate) drops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub(crate) dropped_on: std::sync::Arc<std::sync::Mutex<Vec<std::thread::ThreadId>>>,
    pub(crate) then: Option<Payload>,
}

#[cfg(test)]
impl Bomb {
    /// A bomb that panics with a text when dropped; `drops` counts its drops and `dropped_on` lists the
    /// thread of each.
    pub(crate) fn new(
        drops: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
        dropped_on: &std::sync::Arc<std::sync::Mutex<Vec<std::thread::ThreadId>>>,
    ) -> Self {
        Self {
            drops: std::sync::Arc::clone(drops),
            dropped_on: std::sync::Arc::clone(dropped_on),
            then: None,
        }
    }
}

#[cfg(test)]
impl Drop for Bomb {
    fn drop(&mut self) {
        self.dropped_on
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(std::thread::current().id());
        self.drops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match self.then.take() {
            Some(payload) => std::panic::resume_unwind(payload),
            None => panic!("a payload whose drop panics"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Bomb, Payload, dispose};
    use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

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

    fn threads() -> Arc<Mutex<Vec<std::thread::ThreadId>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn caught<M: std::any::Any + Send>(payload: M) -> Payload {
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
        let payload = caught(Bomb::new(&drops, &threads()));
        // The caller's own unwind boundary: nothing may reach it.
        let escaped = catch_unwind(AssertUnwindSafe(|| dispose(payload)));
        assert!(escaped.is_ok(), "the payload's panic escaped disposal");
        assert_eq!(drops.load(Ordering::SeqCst), 1, "dropped exactly once");
    }

    #[test]
    fn a_payload_whose_drop_panics_with_such_a_payload_is_leaked_not_recursed_into() {
        let outer = counter();
        let inner = counter();
        let mut bomb = Bomb::new(&outer, &threads());
        bomb.then = Some(caught(Bomb::new(&inner, &threads())));
        let payload = caught(bomb);
        let escaped = catch_unwind(AssertUnwindSafe(|| dispose(payload)));
        assert!(escaped.is_ok(), "a nested payload escaped disposal");
        assert_eq!(
            outer.load(Ordering::SeqCst),
            1,
            "the first payload is dropped once"
        );
        assert_eq!(
            inner.load(Ordering::SeqCst),
            0,
            "the second payload is leaked, never dropped"
        );
    }
}
