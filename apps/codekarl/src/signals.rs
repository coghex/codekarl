//! SIGINT and SIGTERM for the runtime command: block them, wait for them on one thread, and turn the
//! first into a quit request and the second into the immediate exit.
//!
//! **Installation** ([`install`]) runs on the main thread after the arguments are parsed and before the
//! logger's writer or any other thread exists, so every later thread inherits the blocked mask:
//! 1. save the mask and the dispositions of SIGINT and SIGTERM;
//! 2. **block** both with `pthread_sigmask`, *before* changing any disposition;
//! 3. set both dispositions to `SIG_DFL`, while they are blocked. An inherited `SIG_IGN` would make the
//!    kernel discard the signal and `sigwait` would never see it. A signal arriving now stays pending, and
//!    cannot terminate the process through `SIG_DFL`;
//! 4. spawn the signal thread, which waits with `sigwait`;
//! 5. if any step fails, roll back in reverse, **still blocked**: restore the saved dispositions first,
//!    and the saved mask last. A signal left pending under a restored `SIG_IGN` is then discarded, and one
//!    under a restored `SIG_DFL` behaves as it would have without codekarl. Only what was saved or changed
//!    is restored.
//!
//! **The first signal** is recorded (which one it was, published before anything else), sets the quit flag
//! the runtime watches, **wakes the main thread through the window notifier** (a pending flag, plus the
//! native wake while the window backend is armed, so a sleeping window loop sees the quit at once; the
//! wake comes before the log record, which may wait) and logs one record; **a second signal** makes one non-waiting attempt to log an
//! error and ends the process at once with status 3. No code runs in a signal handler: there is none.
//!
//! The signal thread is never joined, holds no lock while waiting and ends with the process. A child
//! process started from Lua (`os.execute`, `io.popen`) inherits the blocked mask, so it ignores SIGINT and
//! SIGTERM unless it resets its own mask.
//!
//! The `unsafe` here is the signal calls only, each in its own block with a `SAFETY` comment.

use std::io;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::thread::{self, JoinHandle};

use ck_log::Level;
use ck_window::Notifier;

use crate::lifecycle::{emit, immediate_exit};

/// The exit status when a signal asked for the quit and nothing worse happened: 128 plus the number.
const SIGINT_STATUS: u8 = 130;
const SIGTERM_STATUS: u8 = 143;

/// The exit status of a second signal.
const SECOND_SIGNAL_STATUS: u8 = 3;

/// The number of the first signal received, or 0. It is set (by one compare-and-swap, which also tells the
/// first signal from the second) *before* the quit flag, so whoever sees the flag sees the signal.
static FIRST: AtomicI32 = AtomicI32::new(0);

/// The name of a signal this module handles.
fn name(signal: i32) -> &'static str {
    if signal == libc::SIGINT {
        "SIGINT"
    } else {
        "SIGTERM"
    }
}

/// The exit status a received first signal asks for: 130 for SIGINT, 143 for SIGTERM, none for no signal.
pub(crate) fn status_of(first: i32) -> Option<u8> {
    match first {
        libc::SIGINT => Some(SIGINT_STATUS),
        libc::SIGTERM => Some(SIGTERM_STATUS),
        _ => None,
    }
}

/// The status the first signal of this process asks for, if one was received. The main thread reads it
/// after the final flush, so a signal that arrived during the flush still counts.
pub(crate) fn exit_status() -> Option<u8> {
    status_of(FIRST.load(Ordering::SeqCst))
}

// ---------------------------------------------------------------------------------------------
// The system calls, each in its own block
// ---------------------------------------------------------------------------------------------

/// An empty signal set.
fn empty_set() -> io::Result<libc::sigset_t> {
    let mut set = MaybeUninit::<libc::sigset_t>::zeroed();
    // SAFETY: `set` is valid, writable storage for a `sigset_t`, which `sigemptyset` initialises.
    let result = unsafe { libc::sigemptyset(set.as_mut_ptr()) };
    check(result)?;
    // SAFETY: `sigemptyset` succeeded and so initialised the whole set.
    Ok(unsafe { set.assume_init() })
}

/// A signal set holding SIGINT and SIGTERM.
fn both() -> io::Result<libc::sigset_t> {
    let mut set = empty_set()?;
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: `set` is an initialised signal set, and `signal` is a valid signal number.
        let result = unsafe { libc::sigaddset(&raw mut set, signal) };
        check(result)?;
    }
    Ok(set)
}

/// A `-1`-on-error libc return as a result.
fn check(result: i32) -> io::Result<()> {
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// An errno-valued return (`pthread_sigmask`, `sigwait`: zero or the error number) as a result.
fn check_errno(result: i32) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(result))
    }
}

/// The calling thread's current signal mask.
fn current_mask() -> io::Result<libc::sigset_t> {
    let mut old = MaybeUninit::<libc::sigset_t>::zeroed();
    // SAFETY: a null new set leaves the mask unchanged and only stores the current one in `old`, which is
    // valid, writable storage for a `sigset_t`.
    let result =
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), old.as_mut_ptr()) };
    check_errno(result)?;
    // SAFETY: `pthread_sigmask` succeeded and so initialised `old`.
    Ok(unsafe { old.assume_init() })
}

/// Blocks the signals of `set` on the calling thread.
fn block(set: &libc::sigset_t) -> io::Result<()> {
    // SAFETY: `set` is a valid, initialised signal set; the old mask is not wanted (null).
    let result = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, set, std::ptr::null_mut()) };
    check_errno(result)
}

/// Replaces the calling thread's mask with `mask`.
fn restore_mask(mask: &libc::sigset_t) -> io::Result<()> {
    // SAFETY: `mask` is a valid, initialised signal set; the old mask is not wanted (null).
    let result = unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, mask, std::ptr::null_mut()) };
    check_errno(result)
}

/// The current disposition of `signal`.
fn current_action(signal: i32) -> io::Result<libc::sigaction> {
    let mut old = MaybeUninit::<libc::sigaction>::zeroed();
    // SAFETY: a null new action only queries; `old` is valid, writable storage for a `sigaction`.
    let result = unsafe { libc::sigaction(signal, std::ptr::null(), old.as_mut_ptr()) };
    check(result)?;
    // SAFETY: `sigaction` succeeded and so initialised `old`.
    Ok(unsafe { old.assume_init() })
}

/// Sets the disposition of `signal` to `action`.
fn set_action(signal: i32, action: &libc::sigaction) -> io::Result<()> {
    // SAFETY: `action` is a valid, initialised `sigaction`; the old action is not wanted (null).
    let result = unsafe { libc::sigaction(signal, action, std::ptr::null_mut()) };
    check(result)
}

/// The default disposition, with an empty mask and no flags.
fn default_action() -> io::Result<libc::sigaction> {
    Ok(libc::sigaction {
        sa_sigaction: libc::SIG_DFL,
        sa_mask: empty_set()?,
        sa_flags: 0,
    })
}

/// Waits for one of the blocked signals of `set` and returns its number.
fn wait(set: &libc::sigset_t) -> io::Result<i32> {
    let mut signal = 0;
    // SAFETY: `set` is a valid, initialised signal set and `signal` is valid, writable storage.
    let result = unsafe { libc::sigwait(set, &raw mut signal) };
    check_errno(result)?;
    Ok(signal)
}

// ---------------------------------------------------------------------------------------------
// Installation
// ---------------------------------------------------------------------------------------------

/// What the installation changed, so that a failure can put it back.
struct Saved {
    mask: libc::sigset_t,
    /// The saved disposition of SIGINT and of SIGTERM, and whether it was changed.
    actions: [(i32, libc::sigaction, bool); 2],
    /// The mask was changed (the signals are blocked).
    blocked: bool,
}

impl Saved {
    /// Rolls back what was changed, in reverse and still blocked: the dispositions first, the mask last.
    fn roll_back(&self) {
        for (signal, action, changed) in self.actions.iter().rev() {
            if *changed {
                let _ = set_action(*signal, action);
            }
        }
        if self.blocked {
            let _ = restore_mask(&self.mask);
        }
    }
}

/// The signal thread's body type, so tests can inject a spawn that fails.
pub(crate) type Body = Box<dyn FnOnce() + Send>;

/// Installs signal handling for the runtime command and returns the quit flag to give the runtime. The
/// signal thread wakes `notifier` after it sets the flag. The error is the text after
/// `codekarl: cannot start the signal thread: `.
pub(crate) fn install(notifier: Arc<Notifier>) -> Result<Arc<AtomicBool>, String> {
    install_with(notifier, |body| {
        thread::Builder::new().name("ck-signals".into()).spawn(body)
    })
}

/// [`install`] with the thread spawn supplied, so a test can make it fail.
pub(crate) fn install_with(
    notifier: Arc<Notifier>,
    spawn: impl FnOnce(Body) -> io::Result<JoinHandle<()>>,
) -> Result<Arc<AtomicBool>, String> {
    let fail = |error: io::Error| error.to_string();
    let set = both().map_err(fail)?;
    let mask = current_mask().map_err(fail)?;
    let (int_action, term_action) = (
        current_action(libc::SIGINT).map_err(fail)?,
        current_action(libc::SIGTERM).map_err(fail)?,
    );
    let mut saved = Saved {
        mask,
        actions: [
            (libc::SIGINT, int_action, false),
            (libc::SIGTERM, term_action, false),
        ],
        blocked: false,
    };
    let result = (|| {
        block(&set)?;
        saved.blocked = true;
        let default = default_action()?;
        for (signal, _, changed) in &mut saved.actions {
            set_action(*signal, &default)?;
            *changed = true;
        }
        let quit = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&quit);
        spawn(Box::new(move || serve(set, &FIRST, &flag, &notifier)))?;
        Ok(quit)
    })();
    result.map_err(|error: io::Error| {
        saved.roll_back();
        error.to_string()
    })
}

// ---------------------------------------------------------------------------------------------
// The signal thread
// ---------------------------------------------------------------------------------------------

/// The signal thread: waits for a blocked signal, forever.
fn serve(set: libc::sigset_t, first: &AtomicI32, quit: &AtomicBool, notifier: &Arc<Notifier>) {
    let lines = LogLines {
        notifier: Arc::clone(notifier),
    };
    loop {
        match wait(&set) {
            Ok(signal) => received(first, signal, quit, &lines),
            Err(error) => {
                // A failing wait must not become a busy loop, and no retry can help: say so once and end.
                // Signals stay blocked, so the process can then only be ended from outside.
                let _ = ck_log::try_log!(
                    Level::Error,
                    "runtime",
                    "waiting for signals failed, so SIGINT and SIGTERM are no longer handled: {error}"
                );
                return;
            }
        }
    }
}

/// What the signal thread writes, replaceable by a test.
pub(crate) trait Lines {
    /// Wakes the main thread after the quit flag was set (never blocks).
    fn wake(&self);
    /// The record of the first signal (through `log_within`, with the log wait).
    fn first(&self, signal: i32);
    /// The one non-waiting attempt at the record of the second signal.
    fn second(&self);
    /// Ends the process at once.
    fn exit(&self, status: u8);
}

/// The real lines: the notifier's wake, `log_within`, `try_log` and the immediate exit.
struct LogLines {
    notifier: Arc<Notifier>,
}

impl Lines for LogLines {
    fn wake(&self) {
        self.notifier.wake();
    }

    fn first(&self, signal: i32) {
        emit(
            Level::Info,
            "runtime",
            format_args!("received {}, quitting", name(signal)),
        );
    }

    fn second(&self) {
        let _ = ck_log::try_log!(Level::Error, "runtime", "second signal, ending at once");
    }

    fn exit(&self, status: u8) {
        immediate_exit(status);
    }
}

/// Handles one received signal. The first (`first` still 0) is recorded there, published before the quit
/// flag, sets the quit flag and is logged; a later one is logged without waiting and ends the process
/// with status 3.
pub(crate) fn received(first: &AtomicI32, signal: i32, quit: &AtomicBool, lines: &dyn Lines) {
    if first
        .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        quit.store(true, Ordering::SeqCst);
        lines.wake();
        lines.first(signal);
    } else {
        lines.second();
        lines.exit(SECOND_SIGNAL_STATUS);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::process::{Command, Stdio};
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::boundary::supervisor::{spawn_guard, spawn_locked};

    // -----------------------------------------------------------------------------------------
    // The decision, with no process-global state
    // -----------------------------------------------------------------------------------------

    #[test]
    fn the_first_signal_decides_130_for_sigint_and_143_for_sigterm() {
        assert_eq!(status_of(libc::SIGINT), Some(130));
        assert_eq!(status_of(libc::SIGTERM), Some(143));
        assert_eq!(status_of(0), None);
        assert_eq!(status_of(libc::SIGHUP), None);
    }

    /// Records what the signal thread would write and do, in order.
    #[derive(Default)]
    struct Recorded(RefCell<Vec<String>>);

    impl Lines for Recorded {
        fn wake(&self) {
            self.0.borrow_mut().push("wake".to_owned());
        }

        fn first(&self, signal: i32) {
            self.0.borrow_mut().push(format!("first {}", name(signal)));
        }

        fn second(&self) {
            self.0.borrow_mut().push("second".to_owned());
        }

        fn exit(&self, status: u8) {
            self.0.borrow_mut().push(format!("exit {status}"));
        }
    }

    #[test]
    fn the_first_signal_is_published_before_the_quit_flag_and_the_second_ends_the_process() {
        let first = AtomicI32::new(0);
        let quit = AtomicBool::new(false);
        let lines = Recorded::default();
        received(&first, libc::SIGTERM, &quit, &lines);
        assert_eq!(first.load(Ordering::SeqCst), libc::SIGTERM);
        assert!(quit.load(Ordering::SeqCst));
        assert_eq!(
            *lines.0.borrow(),
            ["wake", "first SIGTERM"],
            "the wake follows the flag and comes before the record, which may wait"
        );
        // A second signal, of either kind, is a second signal: it never changes the first.
        received(&first, libc::SIGINT, &quit, &lines);
        assert_eq!(first.load(Ordering::SeqCst), libc::SIGTERM);
        assert_eq!(
            *lines.0.borrow(),
            ["wake", "first SIGTERM", "second", "exit 3"],
            "one non-waiting record and the immediate exit with status 3"
        );
    }

    #[test]
    fn whoever_sees_the_quit_flag_sees_the_signal() {
        // A watcher spins on the quit flag and reads the recorded signal the moment it sees the flag. The
        // two threads run many rounds in lockstep, so a flag set before the signal is recorded is caught.
        const ROUNDS: usize = 200_000;
        let first = Arc::new(AtomicI32::new(0));
        let quit = Arc::new(AtomicBool::new(false));
        let go = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicUsize::new(0));
        let missed = Arc::new(AtomicUsize::new(0));
        let watcher = {
            let (first, quit, go, done, missed) = (
                Arc::clone(&first),
                Arc::clone(&quit),
                Arc::clone(&go),
                Arc::clone(&done),
                Arc::clone(&missed),
            );
            thread::spawn(move || {
                for round in 1..=ROUNDS {
                    while go.load(Ordering::SeqCst) != round {
                        std::hint::spin_loop();
                    }
                    while !quit.load(Ordering::SeqCst) {
                        std::hint::spin_loop();
                    }
                    if first.load(Ordering::SeqCst) != libc::SIGINT {
                        missed.fetch_add(1, Ordering::SeqCst);
                    }
                    done.store(round, Ordering::SeqCst);
                }
            })
        };
        let lines = Recorded::default();
        for round in 1..=ROUNDS {
            first.store(0, Ordering::SeqCst);
            quit.store(false, Ordering::SeqCst);
            go.store(round, Ordering::SeqCst);
            received(&first, libc::SIGINT, &quit, &lines);
            while done.load(Ordering::SeqCst) != round {
                std::hint::spin_loop();
            }
            lines.0.borrow_mut().clear();
        }
        watcher.join().expect("the watcher");
        assert_eq!(
            missed.load(Ordering::SeqCst),
            0,
            "the flag was seen before the signal"
        );
    }

    // -----------------------------------------------------------------------------------------
    // Process-global state: every test below runs in a child process of this test binary
    // -----------------------------------------------------------------------------------------

    /// The variable that tells the re-executed test binary which test body to run.
    const ISOLATED: &str = "CK_SIGNALS_ISOLATED_TEST";

    /// Runs `body` in a child process of this test binary (masks and dispositions are process-global),
    /// owned from spawn: killed and reaped at the deadline and on every other path.
    fn isolated(name: &str, body: impl FnOnce()) {
        let path = format!("signals::tests::{name}");
        if std::env::var(ISOLATED).as_deref() == Ok(path.as_str()) {
            body();
            return;
        }
        let mut command = Command::new(std::env::current_exe().expect("the test binary"));
        command
            .args(["--exact", &path, "--nocapture"])
            .env(ISOLATED, &path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child_result = spawn_locked(&spawn_guard(), &mut command);
        drop(command);
        let mut child = child_result.expect("the test binary re-executes itself");
        let give_up = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().expect("the child can be polled") {
                break Some(status);
            }
            if Instant::now() >= give_up {
                break None;
            }
            thread::sleep(Duration::from_millis(5));
        };
        let Some(status) = status else {
            let _ = child.kill();
            let _ = child.wait();
            panic!("`{path}` did not finish in 30 s: its process was killed and reaped");
        };
        let output = child.wait_with_output().expect("the child's output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            status.success() && stdout.contains("1 passed"),
            "`{path}` failed in its child process ({status:?})\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Whether `signal` is in `set`.
    fn member(set: libc::sigset_t, signal: i32) -> bool {
        // SAFETY: `set` is an initialised signal set and `signal` a valid number.
        unsafe { libc::sigismember(&raw const set, signal) == 1 }
    }

    /// Whether `signal` is blocked on the calling thread.
    fn is_blocked(signal: i32) -> bool {
        member(current_mask().expect("the mask"), signal)
    }

    /// The disposition of `signal` as its handler word (0 is `SIG_DFL`, 1 is `SIG_IGN`).
    fn handler(signal: i32) -> usize {
        current_action(signal).expect("the action").sa_sigaction
    }

    /// Whether `signal` is pending for the calling thread.
    fn is_pending(signal: i32) -> bool {
        let mut set = empty_set().expect("a set");
        // SAFETY: `set` is an initialised signal set that `sigpending` overwrites.
        let result = unsafe { libc::sigpending(&raw mut set) };
        assert_eq!(result, 0);
        member(set, signal)
    }

    /// Sets the disposition of `signal` to `SIG_IGN`, as a parent that ignores it would have.
    fn ignore(signal: i32) {
        let action = libc::sigaction {
            sa_sigaction: libc::SIG_IGN,
            sa_mask: empty_set().expect("a set"),
            sa_flags: 0,
        };
        set_action(signal, &action).expect("SIG_IGN");
    }

    /// Puts the isolated child in a known state whatever it inherited (a background job of a shell starts
    /// with SIGINT ignored): both signals at their default and unblocked.
    fn fresh() {
        restore_mask(&empty_set().expect("a set")).expect("an empty mask");
        for signal in [libc::SIGINT, libc::SIGTERM] {
            set_action(signal, &default_action().expect("the default")).expect("SIG_DFL");
        }
    }

    fn failing_spawn(_: Body) -> io::Result<JoinHandle<()>> {
        Err(io::Error::other("injected spawn failure"))
    }

    #[test]
    fn a_failed_spawn_restores_the_previous_mask_and_dispositions() {
        isolated(
            "a_failed_spawn_restores_the_previous_mask_and_dispositions",
            || {
                fresh();
                assert!(!is_blocked(libc::SIGINT) && !is_blocked(libc::SIGTERM));
                let before = (handler(libc::SIGINT), handler(libc::SIGTERM));
                assert_eq!(before, (0, 0), "the defaults, in a fresh test process");
                let error =
                    install_with(Notifier::new(), failing_spawn).expect_err("the spawn fails");
                assert_eq!(error, "injected spawn failure");
                assert!(
                    !is_blocked(libc::SIGINT) && !is_blocked(libc::SIGTERM),
                    "the mask is back"
                );
                assert_eq!(
                    (handler(libc::SIGINT), handler(libc::SIGTERM)),
                    before,
                    "the dispositions are back"
                );
            },
        );
    }

    #[test]
    fn a_failed_spawn_restores_an_inherited_ignored_disposition_and_a_blocked_signal_stays_blocked()
    {
        isolated(
            "a_failed_spawn_restores_an_inherited_ignored_disposition_and_a_blocked_signal_stays_blocked",
            || {
                fresh();
                ignore(libc::SIGINT);
                // SIGTERM was already blocked by the parent of this process, say.
                block(&both().expect("a set")).expect("block");
                let mask_before = current_mask().expect("mask");
                let error =
                    install_with(Notifier::new(), failing_spawn).expect_err("the spawn fails");
                assert_eq!(error, "injected spawn failure");
                assert_eq!(handler(libc::SIGINT), 1, "SIG_IGN is restored, not SIG_DFL");
                assert_eq!(handler(libc::SIGTERM), 0);
                let after = current_mask().expect("mask");
                for signal in [libc::SIGINT, libc::SIGTERM] {
                    assert_eq!(
                        member(mask_before, signal),
                        member(after, signal),
                        "the mask of {signal} is exactly what it was"
                    );
                }
            },
        );
    }

    #[test]
    fn a_signal_pending_under_an_inherited_ignore_is_discarded_by_the_rollback_not_acted_on() {
        isolated(
            "a_signal_pending_under_an_inherited_ignore_is_discarded_by_the_rollback_not_acted_on",
            || {
                fresh();
                ignore(libc::SIGTERM);
                let error = install_with(Notifier::new(), |_body| {
                    // The signals are blocked and the disposition is the temporary default: the signal
                    // stays pending on this thread, and cannot terminate the process.
                    assert!(is_blocked(libc::SIGTERM));
                    assert_eq!(handler(libc::SIGTERM), 0);
                    // SAFETY: `pthread_self` takes nothing and returns this thread's handle.
                    let this = unsafe { libc::pthread_self() };
                    // SAFETY: raises SIGTERM on this thread, where it is blocked.
                    let raised = unsafe { libc::pthread_kill(this, libc::SIGTERM) };
                    assert_eq!(raised, 0);
                    assert!(is_pending(libc::SIGTERM), "pending while blocked");
                    Err(io::Error::other("injected spawn failure"))
                })
                .expect_err("the spawn fails");
                // Rolling back restores the ignored disposition while still blocked, and the mask last:
                // the pending signal is discarded. (The reverse order would kill this process.)
                assert_eq!(error, "injected spawn failure");
                assert_eq!(handler(libc::SIGTERM), 1);
                assert!(!is_blocked(libc::SIGTERM));
                assert!(!is_pending(libc::SIGTERM), "discarded");
            },
        );
    }

    #[test]
    fn a_successful_installation_resets_an_inherited_ignore_and_turns_a_signal_into_a_quit() {
        isolated(
            "a_successful_installation_resets_an_inherited_ignore_and_turns_a_signal_into_a_quit",
            || {
                fresh();
                ignore(libc::SIGINT);
                ignore(libc::SIGTERM);
                let waiter = RefCell::new(None);
                let notifier = Notifier::new();
                let quit = install_with(Arc::clone(&notifier), |body| {
                    let handle = thread::spawn(body);
                    *waiter.borrow_mut() =
                        Some(std::os::unix::thread::JoinHandleExt::as_pthread_t(&handle));
                    Ok(handle)
                })
                .expect("installed");
                assert!(is_blocked(libc::SIGINT) && is_blocked(libc::SIGTERM));
                assert_eq!((handler(libc::SIGINT), handler(libc::SIGTERM)), (0, 0));
                assert_eq!(exit_status(), None);
                // The signal goes to the waiting thread itself (this test binary has other threads, which
                // do not block it); in the real process every thread does.
                let waiter = waiter.borrow().expect("the waiter");
                // SAFETY: `waiter` is the pthread of the signal thread, which waits for this signal.
                let sent = unsafe { libc::pthread_kill(waiter, libc::SIGINT) };
                assert_eq!(sent, 0);
                let give_up = Instant::now() + Duration::from_secs(10);
                while !quit.load(Ordering::SeqCst) {
                    assert!(Instant::now() < give_up, "the signal never became a quit");
                    thread::sleep(Duration::from_millis(2));
                }
                assert_eq!(
                    exit_status(),
                    Some(130),
                    "whoever sees the flag sees the signal"
                );
                while !notifier.take_pending() {
                    assert!(
                        Instant::now() < give_up,
                        "the signal never woke the notifier"
                    );
                    thread::sleep(Duration::from_millis(2));
                }
            },
        );
    }

    #[test]
    fn threads_created_after_the_installation_inherit_the_blocked_mask() {
        isolated(
            "threads_created_after_the_installation_inherit_the_blocked_mask",
            || {
                fresh();
                let _quit = install(Notifier::new()).expect("installed");
                let (sigint, sigterm) =
                    thread::spawn(|| (is_blocked(libc::SIGINT), is_blocked(libc::SIGTERM)))
                        .join()
                        .expect("the thread");
                assert!(sigint && sigterm);
            },
        );
    }
}
