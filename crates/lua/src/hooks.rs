//! Test-only hooks into the owner thread, keyed by owner name (each test uses its own names).
//!
//! A hook can hold the thread at a point (a barrier), panic there, or put test values into the Lua
//! state; none of this exists in a non-test build.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use mlua::Lua;

/// A point in the owner thread's life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Point {
    /// The state exists; nothing is installed yet.
    StateCreated,
    /// The host surface is installed.
    SurfaceInstalled,
    /// Just before the main chunk (after the last cancel check but one).
    BeforeMain,
    /// The main chunk has returned.
    AfterMain,
    /// Just before `startup()` is called.
    BeforeStartup,
    /// Just before `shutdown()` is called.
    BeforeShutdown,
    /// A ready or failure report has just been sent.
    Reported,
    /// The final flush is about to run, inside the unwind boundary that guards it.
    Flushing,
    /// The thread has sent its last report and made its final wake, and is about to return.
    BeforeReturn,
}

type PointHook = Box<dyn Fn(Point, Option<&Lua>) + Send + Sync>;
type InstallHook = Box<dyn Fn(&Lua) -> mlua::Result<()> + Send + Sync>;

/// What a test installs for one owner name.
pub(crate) struct Hooks {
    /// Called at every [`Point`].
    pub(crate) at: PointHook,
    /// Called after the host surface is installed, to add test functions to the state.
    pub(crate) install: Option<InstallHook>,
}

fn registry() -> &'static Mutex<HashMap<String, Arc<Hooks>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Arc<Hooks>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn find(name: &str) -> Option<Arc<Hooks>> {
    registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(name)
        .cloned()
}

fn cancel_flags() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static FLAGS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    FLAGS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether an owner called `name` got as far as having a thread created for it.
pub(crate) fn was_spawned(name: &str) -> bool {
    cancel_flags()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains_key(name)
}

fn silent_owners() -> &'static Mutex<HashSet<String>> {
    static SILENT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SILENT.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Makes the owner called `name` end without sending its terminal report, as a thread that is lost
/// before it can report would.
pub(crate) fn suppress_report(name: &str) {
    silent_owners()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(name.to_owned());
}

/// Whether the owner called `name` must end without its terminal report.
pub(crate) fn report_suppressed(name: &str) -> bool {
    silent_owners()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(name)
}

/// Remembers the cancel flag of the owner called `name`, so a test can set it from another thread while
/// the handle is busy waiting (the handle's own `cancel` needs `&mut`).
pub(crate) fn remember_cancel(name: &str, flag: Arc<AtomicBool>) {
    cancel_flags()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(name.to_owned(), flag);
}

/// The cancel flag of the owner called `name`.
pub(crate) fn cancel_flag(name: &str) -> Arc<AtomicBool> {
    cancel_flags()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(name)
        .cloned()
        .unwrap_or_else(|| panic!("no owner called {name} was spawned"))
}

/// Installs `hooks` for the owner called `name`.
pub(crate) fn register(name: &str, hooks: Hooks) {
    registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(name.to_owned(), Arc::new(hooks));
}

/// Runs the owner's hook for `point` with the state.
pub(crate) fn at(name: &str, point: Point, lua: Option<&Lua>) {
    if let Some(hooks) = find(name) {
        (hooks.at)(point, lua);
    }
}

/// Runs the owner's hook for `point` where no state exists any more.
pub(crate) fn notify(name: &str, point: Point) {
    at(name, point, None);
}

/// Lets the owner's hooks put test functions into the state.
pub(crate) fn install(name: &str, lua: &Lua) -> mlua::Result<()> {
    match find(name)
        .as_deref()
        .and_then(|hooks| hooks.install.as_ref())
    {
        Some(install) => install(lua),
        None => Ok(()),
    }
}

/// How many commands of each kind the handle sent to one owner.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sent {
    pub(crate) start: usize,
    pub(crate) stop: usize,
    pub(crate) wake: usize,
    pub(crate) call: usize,
}

fn sent_counts() -> &'static Mutex<HashMap<String, Sent>> {
    static COUNTS: OnceLock<Mutex<HashMap<String, Sent>>> = OnceLock::new();
    COUNTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Counts a command the handle is about to send to the owner called `name`, whether or not the thread
/// is still there to receive it.
pub(crate) fn sent(name: &str, command: &crate::runner::Command) {
    use crate::runner::Command;

    let mut counts = sent_counts().lock().unwrap_or_else(PoisonError::into_inner);
    let entry = counts.entry(name.to_owned()).or_default();
    match command {
        Command::Start => entry.start += 1,
        Command::Stop => entry.stop += 1,
        Command::Wake => entry.wake += 1,
        Command::Call { .. } => entry.call += 1,
    }
}

/// The commands the handle of the owner called `name` has sent so far.
pub(crate) fn sent_so_far(name: &str) -> Sent {
    sent_counts()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(name)
        .copied()
        .unwrap_or_default()
}
