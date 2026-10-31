//! The notifier: sticky pending flag, native wakes only while armed, a bounded disarm.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use ck_window::{ArmRefused, DisarmTimeout, NativeWake, Notifier, NotifierState};

use crate::fakes::{BOUND, Gate, Native, SHORT, owned};

#[test]
fn a_wake_before_arm_and_after_disarm_sets_only_the_pending_flag() {
    let notifier = Notifier::new();
    let native = Arc::new(Native::default());
    notifier.wake();
    assert!(notifier.take_pending());
    assert!(!notifier.take_pending(), "take_pending clears the flag");
    assert_eq!(native.wakes.load(Ordering::SeqCst), 0);

    notifier
        .arm(Arc::clone(&native) as Arc<dyn NativeWake>)
        .expect("inactive");
    assert_eq!(notifier.state(), NotifierState::Armed);
    notifier.wake();
    assert_eq!(
        native.wakes.load(Ordering::SeqCst),
        1,
        "armed: the native target is called"
    );
    assert!(notifier.take_pending(), "and the flag is set as well");

    notifier.disarm(BOUND).expect("nothing in flight");
    assert_eq!(notifier.state(), NotifierState::Disarmed);
    notifier.wake();
    assert_eq!(
        native.wakes.load(Ordering::SeqCst),
        1,
        "disarmed: no native call"
    );
    assert!(notifier.take_pending(), "but the flag is set");
}

#[test]
fn arm_is_legal_only_when_inactive() {
    let notifier = Notifier::new();
    let native = Arc::new(Native::default());
    notifier
        .arm(Arc::clone(&native) as Arc<dyn NativeWake>)
        .expect("inactive");
    assert_eq!(
        notifier.arm(Arc::clone(&native) as Arc<dyn NativeWake>),
        Err(ArmRefused {
            state: NotifierState::Armed
        })
    );
    notifier.disarm(BOUND).expect("quiet");
    assert_eq!(
        notifier.arm(native as Arc<dyn NativeWake>),
        Err(ArmRefused {
            state: NotifierState::Disarmed
        })
    );
}

#[test]
fn disarm_waits_for_a_wake_in_flight_and_no_native_call_starts_after_disarming() {
    let notifier = Notifier::new();
    let native = Arc::new(Native::default());
    let gate = Arc::new(Gate::default());
    native.gate_wakes(&gate);
    notifier
        .arm(Arc::clone(&native) as Arc<dyn NativeWake>)
        .expect("inactive");

    thread::scope(|scope| {
        // A producer enters the native wake and stays there.
        scope.spawn(|| notifier.wake());
        gate.wait_entered(1);

        // A short disarm times out; the notifier stays Disarming, and a new wake makes no native call.
        assert_eq!(
            notifier.disarm(Duration::from_millis(40)),
            Err(DisarmTimeout)
        );
        assert_eq!(notifier.state(), NotifierState::Disarming);
        notifier.wake();
        assert_eq!(
            native.wakes.load(Ordering::SeqCst),
            1,
            "no native wake starts after Disarming"
        );
        assert!(notifier.take_pending());

        // A longer disarm waits for the producer, which is released after a while.
        let released = Instant::now() + Duration::from_millis(80);
        let finisher = scope.spawn(|| {
            thread::sleep(Duration::from_millis(80));
            gate.release();
        });
        notifier.disarm(BOUND).expect("the in-flight wake returned");
        assert!(
            Instant::now() >= released,
            "disarm returned before the wake did"
        );
        assert_eq!(notifier.state(), NotifierState::Disarmed);
        finisher.join().expect("no panic");
    });
}

struct Reentrant {
    notifier: Arc<Notifier>,
    calls: AtomicUsize,
}

impl NativeWake for Reentrant {
    fn wake(&self) {
        // The notifier's lock is not held across the native call: these take it, and would deadlock
        // if it were.
        assert_eq!(self.notifier.state(), NotifierState::Armed);
        assert!(
            self.notifier.take_pending(),
            "the pending flag was set before the native call"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn the_notifier_lock_is_never_held_across_the_native_call() {
    owned(
        module_path!(),
        "the_notifier_lock_is_never_held_across_the_native_call",
        SHORT,
        || {
            let notifier = Notifier::new();
            let target = Arc::new(Reentrant {
                notifier: Arc::clone(&notifier),
                calls: AtomicUsize::new(0),
            });
            notifier
                .arm(Arc::clone(&target) as Arc<dyn NativeWake>)
                .expect("inactive");
            notifier.wake();
            assert_eq!(target.calls.load(Ordering::SeqCst), 1);
        },
    );
}

struct Panics;

impl NativeWake for Panics {
    fn wake(&self) {
        panic!("a native wake that panics");
    }
}

#[test]
fn a_native_wake_that_panics_still_deregisters_so_disarm_succeeds() {
    let notifier = Notifier::new();
    notifier
        .arm(Arc::new(Panics) as Arc<dyn NativeWake>)
        .expect("inactive");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| notifier.wake()));
    assert!(caught.is_err());
    notifier
        .disarm(Duration::from_millis(200))
        .expect("the panicked wake was deregistered");
}
