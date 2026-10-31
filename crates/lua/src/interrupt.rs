//! Cancellation of running Lua: an instruction-count hook, and wrappers for the protected calls.
//!
//! The hook runs every [`HOOK_INTERVAL`] VM instructions and, while the owner's cancel flag is set,
//! raises the **interruption**: an error that `mlua` turns into a userdata in Lua, which a script cannot
//! construct and which carries the text `ck-lua: interrupted`. A hook alone cannot get past `pcall`
//! (the next check may fall inside a new protected call, which catches it again), so `pcall`, `xpcall`,
//! `coroutine.resume` and `coroutine.close` are replaced, before any script runs, by wrappers that, once
//! the cancel flag is set, raise the interruption instead of returning, whatever the protected call
//! returned or raised. The flag is never cleared, so a `__close` handler or an `xpcall` message handler
//! that replaced or swallowed the interruption cannot revive the script. With the flag clear a wrapper
//! returns exactly what the standard function returns. The originals are held only by the wrappers, and
//! the debug library is not loaded, so a script cannot reach them.
//!
//! **What it cannot interrupt** (also in the crate docs): a host function that blocks, because the hook
//! runs only between Lua instructions; a `__gc` finalizer, because Lua runs finalizers with hooks
//! disabled; and a `__close` handler of a variable in the very function the hook interrupts, because
//! `mlua`'s hook error path clears the stack with `lua_settop` before `lua_error` (0 on its
//! reserved-failure path, 1 on its new-failure path), which closes that function's own variables inside
//! the hook, where hooks are disabled. Such an owner misses its deadline: the waiting call returns
//! `VmError::Deadline` and the thread keeps running.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use mlua::{Function, HookTriggers, Lua};

/// How many VM instructions run between two looks at the cancel flag.
pub(crate) const HOOK_INTERVAL: u32 = 1000;

/// The interruption: what the hook and the wrappers raise while the cancel flag is set.
#[derive(Debug)]
struct Interrupted;

impl fmt::Display for Interrupted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ck-lua: interrupted")
    }
}

impl std::error::Error for Interrupted {}

/// The error a host function returns to end the script after it recorded a panic: the interruption.
pub(crate) fn interrupted() -> mlua::Error {
    mlua::Error::external(Interrupted)
}

/// A panic in a host function, which ends the owner as a panic and not as a cancellation.
///
/// `mlua` would let `pcall` or `xpcall` catch a Rust panic as a Lua error, so a host function catches
/// it itself, records it here, sets the cancel flag and returns the interruption. The flag then makes
/// every wrapper and the hook end the script; the recorded message makes the run's classification a
/// panic, whatever else the unwinding raised.
#[derive(Clone)]
pub(crate) struct Fatal {
    cancel: Arc<AtomicBool>,
    message: Arc<Mutex<Option<String>>>,
    /// Set when the final flush and the state drop begin: a panic from then on is a teardown panic.
    tearing_down: Arc<AtomicBool>,
    teardown: Arc<Mutex<Option<String>>>,
}

impl Fatal {
    pub(crate) fn new(cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel,
            message: Arc::new(Mutex::new(None)),
            tearing_down: Arc::new(AtomicBool::new(false)),
            teardown: Arc::new(Mutex::new(None)),
        }
    }

    /// Records a panic (the first one wins, separately before and during teardown) and sets the cancel
    /// flag.
    pub(crate) fn record(&self, message: String) {
        let slot = if self.tearing_down.load(Ordering::SeqCst) {
            &self.teardown
        } else {
            &self.message
        };
        slot.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(message);
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// The recorded panic's message, if a host function panicked while the script ran.
    pub(crate) fn message(&self) -> Option<String> {
        self.message
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Marks the start of the final flush and the state drop.
    pub(crate) fn begin_teardown(&self) {
        self.tearing_down.store(true, Ordering::SeqCst);
    }

    /// The first panic recorded during teardown (a finalizer calling a host function, for example).
    pub(crate) fn teardown_message(&self) -> Option<String> {
        self.teardown
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The wrappers. They are given `cancelled()` (whether the flag is set), `interrupt()` (raise the
/// interruption) and `fatal()` (whether a host function panicked), and keep the standard functions as
/// upvalues that no script can reach. Once a host function panicked, an `xpcall` message handler is not
/// run either: the owner is ending as a panic.
const WRAPPERS: &str = "
local cancelled, interrupt, fatal = ...
local type, pcall, xpcall, resume, close = type, pcall, xpcall, coroutine.resume, coroutine.close
local function settle(...)
  if cancelled() then interrupt() end
  return ...
end
_G.pcall = function(...) return settle(pcall(...)) end
_G.xpcall = function(f, handler, ...)
  if type(handler) == 'function' then
    local inner = handler
    handler = function(...)
      if fatal() then return ... end
      return inner(...)
    end
  end
  return settle(xpcall(f, handler, ...))
end
coroutine.resume = function(...) return settle(resume(...)) end
coroutine.close = function(...) return settle(close(...)) end
";

/// Installs the hook and the wrappers in `lua`, before any script runs. The hook is `mlua`'s *global*
/// hook: a thread hook would not reach the coroutines a script creates (the probe showed their hook
/// calls were never made), whereas the global one applies to every coroutine Lua creates and resumes.
pub(crate) fn install(lua: &Lua, fatal: &Fatal) -> mlua::Result<()> {
    let cancel = &fatal.cancel;
    let flag = Arc::clone(cancel);
    lua.set_global_hook(
        HookTriggers {
            every_nth_instruction: Some(HOOK_INTERVAL),
            ..HookTriggers::new()
        },
        move |_, _| {
            if flag.load(Ordering::SeqCst) {
                Err(mlua::Error::external(Interrupted))
            } else {
                Ok(mlua::VmState::Continue)
            }
        },
    )?;
    let flag = Arc::clone(cancel);
    let cancelled = lua.create_function(move |_, ()| Ok(flag.load(Ordering::SeqCst)))?;
    let interrupt = lua
        .create_function(|_, ()| -> mlua::Result<()> { Err(mlua::Error::external(Interrupted)) })?;
    let wrap: Function = lua
        .load(format!("return function(...)\n{WRAPPERS}\nend"))
        .set_name("=ck-lua wrappers")
        .eval()?;
    let panicked = Arc::clone(&fatal.message);
    let fatal = lua.create_function(move |_, ()| {
        Ok(panicked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some())
    })?;
    wrap.call::<()>((cancelled, interrupt, fatal))
}
