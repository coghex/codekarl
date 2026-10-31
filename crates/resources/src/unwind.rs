//! How the crate disposes of the payload of a panic it caught.
//!
//! This is the crate's own copy of the contract of the other crates' `unwind` modules (`crates/log/src/unwind.rs`):
//! the standard library documents that dropping a payload returned by `catch_unwind` can itself panic (the payload is
//! any `Send` value, and its `Drop` is arbitrary code). A loader thread that ended with such a payload would leave it
//! to be dropped outside any boundary of this crate, by a joiner or, once the handle was detached, by the standard
//! library as the thread ends, which aborts the process if that drop panics. So a loader catches its own panics, reads
//! what it needs from the payload and hands it to [`dispose`]; nothing else drops one.

use std::any::Any;
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// The payload of a caught panic.
pub(crate) type Payload = Box<dyn Any + Send>;

/// The text of a payload that is not a string.
pub(crate) const NON_STRING_PAYLOAD: &str = "a panic with a payload that is not text";

/// The most bytes of a panic's text kept: a message is bounded before it is stored.
const MAX_MESSAGE_BYTES: usize = 512;

/// The text of `payload` (a `String` or a `&str`; anything else gets a fixed text), cut at a character boundary to at
/// most [`MAX_MESSAGE_BYTES`] bytes. It only reads the payload.
pub(crate) fn message(payload: &Payload) -> String {
    let text = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or(NON_STRING_PAYLOAD);
    let mut end = text.len().min(MAX_MESSAGE_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// Drops `payload` without ever unwinding. The drop runs inside its own `catch_unwind`; if it panics, the second
/// payload is leaked with `mem::forget` (dropping it could panic in turn, without end). Returns normally in every case.
pub(crate) fn dispose(payload: Payload) {
    if let Err(second) = catch_unwind(AssertUnwindSafe(move || drop(payload))) {
        mem::forget(second);
    }
}

/// A payload whose `Drop` panics, counting its own drop and noting which thread ran it (tests only).
#[cfg(test)]
pub(crate) struct Bomb {
    pub(crate) drops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub(crate) dropped_on: std::sync::Arc<std::sync::Mutex<Vec<std::thread::ThreadId>>>,
    pub(crate) then: Option<Payload>,
}

#[cfg(test)]
impl Bomb {
    /// A bomb that panics with a text when dropped.
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
    use super::{Bomb, NON_STRING_PAYLOAD, Payload, dispose, message};
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

    #[test]
    fn the_message_of_a_string_a_str_and_anything_else() {
        let from = |panic: Box<dyn FnOnce() + std::panic::UnwindSafe>| -> Payload {
            catch_unwind(panic).expect_err("it panicked")
        };
        assert_eq!(message(&from(Box::new(|| panic!("plain")))), "plain");
        assert_eq!(
            message(&from(Box::new(|| panic!("with {}", "format")))),
            "with format"
        );
        assert_eq!(
            message(&from(Box::new(|| panic_any(7_u32)))),
            NON_STRING_PAYLOAD
        );
    }

    #[test]
    fn a_long_message_is_cut_at_a_character_boundary() {
        let long = "é".repeat(600);
        let payload: Payload = Box::new(long);
        let text = message(&payload);
        assert!(text.len() <= 512, "{}", text.len());
        assert!(text.chars().all(|c| c == 'é'));
    }

    #[test]
    fn an_ordinary_payload_is_dropped_once() {
        let drops = Arc::new(AtomicUsize::new(0));
        dispose(Box::new(Counted(Arc::clone(&drops))));
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_payload_whose_drop_panics_does_not_unwind_out_of_disposal() {
        let drops = Arc::new(AtomicUsize::new(0));
        let on = Arc::new(Mutex::new(Vec::new()));
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            dispose(Box::new(Bomb::new(&drops, &on)));
        }));
        assert!(outcome.is_ok(), "disposal returned normally");
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_payload_whose_drop_panics_with_such_a_payload_is_leaked_not_recursed_into() {
        let (outer, inner) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let on = Arc::new(Mutex::new(Vec::new()));
        let mut bomb = Bomb::new(&outer, &on);
        bomb.then = Some(Box::new(Bomb::new(&inner, &on)));
        let outcome = catch_unwind(AssertUnwindSafe(|| dispose(Box::new(bomb))));
        assert!(outcome.is_ok(), "disposal returned normally");
        assert_eq!(outer.load(Ordering::SeqCst), 1);
        assert_eq!(
            inner.load(Ordering::SeqCst),
            0,
            "the second payload was leaked, not dropped"
        );
    }
}
