//! How the crate reads and disposes of the payload of a panic it caught.
//!
//! The standard library documents that dropping a payload returned by `catch_unwind` can itself panic (the
//! payload is any `Send` value, and its `Drop` is arbitrary code). A payload dropped implicitly, by a `_`
//! pattern or a discarded `Result`, would then unwind out of the boundary that caught the panic, skipping the
//! cleanup and the result that boundary exists to produce. Every place in this crate that catches an unwind
//! therefore reads what it needs from the payload with [`message`] and then hands the payload to [`dispose`],
//! and nothing else drops one.

use std::any::Any;
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// The payload of a caught panic.
pub(crate) type Payload = Box<dyn Any + Send>;

/// The text of a `String` or `&str` payload, and a fixed text for any other kind. It only borrows the payload.
pub(crate) fn message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_else(|| "a panic with a non-string payload".to_owned())
}

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
pub(crate) fn join(handle: std::thread::JoinHandle<()>) {
    if let Err(payload) = handle.join() {
        dispose(payload);
    }
}

/// A forced panic for the test hooks: with the payload `with` raises if one is chosen, else with `text`.
pub(crate) fn forced_panic(with: Option<fn() -> !>, text: &'static str) -> ! {
    match with {
        Some(raise) => raise(),
        None => std::panic::panic_any(text),
    }
}

#[cfg(test)]
mod tests {
    use super::{Payload, dispose, message};
    use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A payload whose `Drop` panics with `then`, counting its own drop.
    struct Bomb {
        drops: Arc<AtomicUsize>,
        then: Option<Payload>,
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

    fn caught<M: std::any::Any + Send>(payload: M) -> Payload {
        catch_unwind(AssertUnwindSafe(|| panic_any(payload))).expect_err("the closure panics")
    }

    #[test]
    fn the_message_of_a_string_a_str_and_any_other_payload() {
        assert_eq!(message(&*caught(String::from("owned"))), "owned");
        assert_eq!(message(&*caught("borrowed")), "borrowed");
        assert_eq!(
            message(&*caught(7_u32)),
            "a panic with a non-string payload"
        );
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
}
