//! A condition-variable signal for a VM thread's last project-owned action.

use std::cell::RefCell;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct Exit {
    done: Mutex<bool>,
    changed: Condvar,
}

/// A clonable wait handle for one VM thread's exit. It does not own or join the VM handle.
#[derive(Clone, Debug)]
pub struct ExitWatch(Arc<Exit>);

impl ExitWatch {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Exit {
            done: Mutex::new(false),
            changed: Condvar::new(),
        }))
    }

    /// Waits up to `within` for the VM thread's exit guard to run.
    ///
    /// A zero duration still makes one non-waiting observation. This signal never joins the thread.
    pub fn wait(&self, within: Duration) -> bool {
        let limit = Instant::now().checked_add(within);
        let mut done = self.0.done.lock().unwrap_or_else(PoisonError::into_inner);
        while !*done {
            if within.is_zero() {
                return false;
            }
            match limit {
                Some(limit) => {
                    let left = limit.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return false;
                    }
                    done = self
                        .0
                        .changed
                        .wait_timeout(done, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                None => {
                    done = self
                        .0
                        .changed
                        .wait(done)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
        }
        true
    }
}

struct ExitSignal(Arc<Exit>);

impl Drop for ExitSignal {
    fn drop(&mut self) {
        *self.0.done.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.0.changed.notify_all();
    }
}

thread_local! {
    /// Registered by the VM thread's first action; reverse TLS drop order makes this its last.
    static EXIT_GUARD: RefCell<Option<ExitSignal>> = const { RefCell::new(None) };
}

/// Installs the exit signal as the first action on the VM thread.
pub(crate) fn install(watch: ExitWatch) {
    EXIT_GUARD.with(|slot| *slot.borrow_mut() = Some(ExitSignal(watch.0)));
}
