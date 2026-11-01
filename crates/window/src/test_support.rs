//! Test-only controls for exercising update-driven application fixtures while the OS gives the app no focus.
//!
//! This process-local opt-in is compiled only with `test-support`. It does not change application configuration,
//! and dropping the guard restores the real focus gate.

use std::{
    marker::PhantomData,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

static ALLOW_UNFOCUSED_UPDATES: AtomicBool = AtomicBool::new(false);
static QUEUED_FOCUS_LOST: AtomicBool = AtomicBool::new(false);
static WINDOWS_WITHOUT_ACTIVATION: AtomicBool = AtomicBool::new(false);

/// Prevent new test-worker windows from requesting application or key-window activation.
///
/// This process-local switch is one-way and idempotent. Set it before creating a backend. It is
/// available only to test-support builds; ordinary application builds retain the production path.
pub fn open_windows_without_activation() {
    WINDOWS_WITHOUT_ACTIVATION.store(true, Ordering::Release);
}

pub(crate) fn windows_without_activation() -> bool {
    WINDOWS_WITHOUT_ACTIVATION.load(Ordering::Acquire)
}

/// Temporarily let update scheduling continue while the window is unfocused.
///
/// Intended only for an update-driven integration-test worker that must remain useful when the owner or another app
/// takes focus. This bypasses only the scheduler's focus eligibility predicate: native focus events still update the
/// input snapshot, and minimized/closed/disabled windows remain ineligible. Do not use it in focus-specific tests.
#[must_use = "keep the guard alive for the duration of the test worker"]
pub fn allow_updates_while_unfocused() -> AllowUnfocusedUpdates {
    ALLOW_UNFOCUSED_UPDATES
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .expect("the process-local focus override must have one owner");
    AllowUnfocusedUpdates(PhantomData)
}

/// The process-local opt-in, exposed only to the window core.
pub(crate) fn updates_may_run_unfocused() -> bool {
    ALLOW_UNFOCUSED_UPDATES.load(Ordering::Acquire)
}

/// Queue one synthetic focus-loss event for the next shared window-loop pump.
///
/// This is for integration-test workers only: it exercises the common event path without asking the OS to change
/// application focus. It is available only with the `test-support` feature.
pub fn queue_focus_lost_on_next_pump() {
    assert!(
        QUEUED_FOCUS_LOST
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok(),
        "only one synthetic focus-loss event may be queued per worker"
    );
}

/// The shared loop consumes the queued event once, immediately after its next native pump.
pub(crate) fn take_queued_focus_lost() -> bool {
    QUEUED_FOCUS_LOST.swap(false, Ordering::AcqRel)
}

/// Resets the focus override when the update-driven worker exits its guarded scope.
pub struct AllowUnfocusedUpdates(PhantomData<Rc<()>>);

impl Drop for AllowUnfocusedUpdates {
    fn drop(&mut self) {
        ALLOW_UNFOCUSED_UPDATES.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        allow_updates_while_unfocused, open_windows_without_activation, updates_may_run_unfocused,
        windows_without_activation,
    };

    #[test]
    fn unfocused_update_override_is_scoped_to_its_guard() {
        assert!(!updates_may_run_unfocused());
        {
            let _guard = allow_updates_while_unfocused();
            assert!(updates_may_run_unfocused());
        }
        assert!(!updates_may_run_unfocused());
    }

    #[test]
    fn window_activation_switch_is_one_way_and_idempotent() {
        assert!(crate::window_activation_allowed());
        open_windows_without_activation();
        assert!(!crate::window_activation_allowed());
        open_windows_without_activation();
        assert!(!crate::window_activation_allowed());
        assert!(windows_without_activation());
    }
}
