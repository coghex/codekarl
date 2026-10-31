//! The Lua VM owner: one Lua 5.4 state on its own thread.
//!
//! A [`VmOwner`] starts one OS thread, creates a Lua state on it with `mlua`'s safe standard-library
//! set, installs the host surface (`log.*` and a read-only `config`), runs an entry script and then
//! waits for `startup()` and `shutdown()` requests and posted callback calls. The state is created, used and dropped on that
//! thread only: `mlua` is built without its `send` feature, so the state cannot leave it (a compile
//! time guarantee). Owners share no Lua state and never call into one another.
//!
//! This crate supplies the primitive and reports outcomes. It has no supervision policy (parallel
//! start, cancelling every owner on a failure, exit statuses).
//!
//! # States and calls
//!
//! The states are those of the [`VmOwner`] *handle* as its caller observes them. The handle moves only
//! when a call consumes a report: it stays [`VmState::Initialising`] until `wait_ready` consumes the
//! ready or failure report, even if the thread finished its main chunk earlier.
//!
//! | Call | Legal in | Effect |
//! | --- | --- | --- |
//! | [`wait_ready`](VmOwner::wait_ready) | Initialising | waits for the ready report |
//! | [`start`](VmOwner::start) | Ready | calls the global `startup()` if defined: [`request_start`](VmOwner::request_start) then [`await_started`](VmOwner::await_started) |
//! | [`request_start`](VmOwner::request_start) | Ready | sends the start request and returns at once; the handle is Starting |
//! | [`await_started`](VmOwner::await_started) | Starting | waits for the result; `Ok` means Started |
//! | [`poll_ready`](VmOwner::poll_ready) | Initialising | like `wait_ready`, but a missed deadline is `Pending` and changes nothing |
//! | [`poll_started`](VmOwner::poll_started) | Starting | like `await_started`, but a missed deadline is `Pending` and changes nothing |
//! | [`stop`](VmOwner::stop) | Ready, Started | calls the global `shutdown()` if defined, then the thread ends; `Ok` only after the thread was observed finished and joined: [`request_stop`](VmOwner::request_stop) then [`await_stopped`](VmOwner::await_stopped) |
//! | [`request_stop`](VmOwner::request_stop) | Ready, Started | sends the stop request and returns at once; the handle is Stopping |
//! | [`await_stopped`](VmOwner::await_stopped) | Stopping | waits for the result and returns `Ok` only after the thread was observed finished and joined |
//! | [`post`](VmOwner::post) | Started | sends a callback call and returns at once (see "Posted calls"); the state does not change |
//! | [`take_result`](VmOwner::take_result) | any | never waits; the next posted call's result, if one has arrived |
//! | [`take_failure`](VmOwner::take_failure) | Ready, Started | never waits; the owner thread's terminal failure if it has already reported one (see "Observing a failure"); `None` and nothing consumed in any other state |
//! | [`cancel`](VmOwner::cancel) | any | never waits; see below |
//! | [`wait_finished`](VmOwner::wait_finished) | any | `true` once the thread is observed finished and joined |
//! | [`is_finished`](VmOwner::is_finished) | any | whether the thread has ended |
//!
//! The split calls let a supervisor have requests to several owners in flight together: send every
//! request, then await each. Any other call returns [`VmError::State`] and sends nothing. Every wait
//! takes a deadline and never waits past it; a zero deadline makes one non-waiting observation, so a
//! result or a finished thread that is already there is seen as such. A missed deadline **abandons** the request: whenever the running Lua call returns, its
//! result is discarded and the thread ends exactly as if cancelled. A deadline never forces Lua to stop
//! and never joins the thread.
//!
//! # Polls
//!
//! `wait_ready` and `await_started` **abandon** the request when their deadline passes, so they cannot
//! be called in slices. [`poll_ready`](VmOwner::poll_ready) and [`poll_started`](VmOwner::poll_started)
//! wait for the same report for at most `within` and, with no report, return [`Progress::Pending`] and
//! change *nothing*: the cancel flag is not set, no command is sent, the handle's state is unchanged and
//! the request stays in flight, so a later poll, or `wait_ready` or `await_started`, still sees the
//! result. A report that arrived is consumed exactly as the waiting call consumes it ([`Progress::Done`],
//! or the same error and the handle Ending). A zero `within` makes one non-waiting observation. A caller
//! that wants a deadline with slicing polls, and when its own deadline passes calls the abandoning wait
//! with a zero deadline (or `cancel`).
//!
//! # Cancel and ending
//!
//! [`VmOwner::cancel`] sets the cancel flag and wakes an idle thread through its control channel. While
//! idle the thread runs no further Lua (`shutdown()` is **not** called) and ends. During initialisation
//! the flag is checked after the state is created, after the host surface is installed and before the
//! main chunk, and the thread ends at the first check that finds it set. A running Lua call (the main
//! chunk, `startup()` or `shutdown()`) is **interrupted** (see below) and the call that was waiting for
//! it returns [`VmError::Cancelled`]; the thread then ends without calling more Lua. An abandoned
//! request (a missed deadline) sets the same flag, so an endless script no longer outlives its deadline.
//!
//! # Interrupting running Lua
//!
//! Every state gets an `mlua` instruction-count hook, installed before any script runs, that looks at
//! the cancel flag every `HOOK_INTERVAL` (1000) VM instructions, in the main thread and in every
//! coroutine, and, while the flag is set, raises the **interruption**: an error value (the text
//! `ck-lua: interrupted`) that a script cannot construct. The interruption is recognised by the flag,
//! never by its text: a script that raises the same text with the flag clear gets an ordinary
//! [`VmError::Script`]. While the flag is clear the hook costs one flag check per 1000 instructions
//! and changes no result.
//!
//! A hook alone cannot get past `pcall`, so `pcall`, `xpcall`, `coroutine.resume` and `coroutine.close`
//! are replaced, before any script runs, by wrappers. With the flag clear a wrapper returns exactly
//! what the standard function returns. Once the flag is set it raises the interruption instead of
//! returning, whatever the protected call returned or raised: the flag is never cleared, so a `__close`
//! handler or an `xpcall` message handler that replaced or swallowed the interruption cannot revive the
//! script. The originals are reachable only through the wrappers (the debug library is not loaded).
//! A call ended by any error after the flag was set is classified [`VmError::Cancelled`].
//!
//! **What it cannot interrupt.** Such an owner misses its deadline: the waiting call returns
//! [`VmError::Deadline`] and the thread keeps running, so it is not observed finished and its state is
//! not dropped. That is an incomplete shutdown, not a clean cancel:
//! - a host function that blocks (a `print` stuck on a stalled stdout): the hook runs only between Lua
//!   instructions;
//! - a `__gc` finalizer that never returns: Lua runs finalizers with hooks disabled;
//! - a `__close` handler that never returns, of a to-be-closed variable in the very function the hook
//!   interrupts: before `lua_error`, `mlua`'s hook error path clears the stack with `lua_settop` (0 on its
//!   reserved-failure path, 1 on its new-failure path), which closes that function's own variables
//!   inside the hook, where hooks are disabled. (A variable of an outer function is closed
//!   while the error unwinds, with hooks on, and an endless handler there is interrupted.)
//!
//! On every path where the state was created the thread flushes the C library's stdout (through the
//! `io.stdout` stream and `flush` method captured before any script ran, so a script that replaces `io`
//! cannot prevent it), drops the state and ends. Dropping a [`VmOwner`] sets the cancel flag and never
//! blocks: a finished thread is joined, a running one is detached.
//!
//! # Host extensions
//!
//! An [`Extension`] is one global table. A VM has a list of them ([`VmSettings::extensions`]), installed
//! on the owner thread after `log` and `config` and before the entry script runs, in list order (an
//! installation error, in any table, is an initialisation failure, [`VmError::Script`]). Every table's
//! setters register into the VM's one set of callbacks. [`VmOwner::spawn`] refuses an invalid list with
//! `InvalidInput`, before any thread starts: a table name that is not lowercase letters, digits and `_`,
//! or that is `log`, `config`, `startup`, `shutdown` or a standard-library global; a function, setter or
//! callback name that is not a Lua identifier; a name used twice in one table; **two tables with the
//! same name; a callback name registered by two tables** (callback names are unique across the VM
//! because [`post`](VmOwner::post) calls a callback by name). Function and setter names belong to their
//! own table, so two tables may each have, say, `open`. The list order therefore never decides which
//! table wins a name.
//!
//! - **Host functions.** `table.name(...)` converts each argument, calls the Rust function on the owner
//!   thread and returns its values to Lua as plain values (not proxies; a host function cannot return
//!   `nil`). `Err(text)` raises an ordinary Lua error with that text, catchable by `pcall` while the
//!   cancel flag is clear.
//! - **Setters.** `table.setter(fn)` registers `fn` as the callback named in [`Extension::callback`],
//!   replacing an earlier one; `table.setter(nil)` clears it; any other argument is a Lua argument error.
//!   [`Extension::callback_observed`] adds an observer, called on the owner thread after the change with `true`
//!   (registered) or `false` (cleared), outside any lock of this crate; a panic in it is owner-fatal, as in a host
//!   function. [`Extension::unavailable_callback`] adds a setter that validates exactly like the others and then raises
//!   a given message without changing the callback table or calling any observer.
//! - **Argument conversion.** `nil` is `None`; booleans, integers and floats convert directly; a string
//!   converts only if its bytes are valid UTF-8 (embedded NUL bytes are valid); a table with only string
//!   keys is a [`ConfigValue::Table`], one whose keys are exactly `1..n` an [`ConfigValue::Array`], an
//!   empty one an empty table. A function, userdata or thread, a table with mixed or other keys, nesting
//!   deeper than 32 levels (which also catches a cycle) and **a string value or table key that is not
//!   valid UTF-8** are rejected with a Lua argument error naming the argument and the reason, and the
//!   Rust function is not called. Nothing is ever converted lossily. The same rules apply to the values
//!   a posted callback returns.
//! - **A panic in a host function is owner-fatal,** also inside `pcall`, `xpcall` and coroutines. `mlua`
//!   would let Lua catch a Rust panic as an error, so the host function catches it itself, records the
//!   message, sets the cancel flag and raises the interruption; the wrappers and the hook then end the
//!   script, and `xpcall` message handlers are not run. The waiting call (or a later poll, or the
//!   lifecycle wait after a posted call) returns [`VmError::Panicked`] with the panic message, not
//!   [`VmError::Cancelled`]. The wrappers and this guard capture every function they use (including
//!   `type`) before any script runs, so a script that rebinds or removes a global neither bypasses the
//!   guard nor changes an ordinary `xpcall`. Lua's own unwinding still runs `__close` handlers and
//!   `__gc` finalizers, bounded as in "Interrupting running Lua"; the outcome is still `Panicked`.
//! - **A panic during teardown** (the final stdout flush or the state drop, for example a finalizer
//!   that calls a host function) is recorded too. A stop acknowledges success only after the state was
//!   dropped, so `stop` and `await_stopped` return [`VmError::Panicked`] and never `Ok`. When a failure
//!   was already selected before teardown began (a Lua error or a panic that ended the call, whether
//!   or not its report had been delivered yet), that failure is the primary cause and stands, and the
//!   teardown panic is logged as one error record in category `lua.<name>`, `host function panicked during teardown:
//!   <message>`, through `log_within` with a 100 ms deadline; a cancelled owner, which has no caller to
//!   tell, logs it the same way.
//!
//! # Posted calls
//!
//! In Started, [`post`](VmOwner::post) sends a call of a registered callback and returns a [`CallId`] at
//! once. Its errors are checked in this order and none of them sends anything: [`VmError::State`] (not
//! Started), [`VmError::Disconnected`] (the owner thread has ended, so an ended owner is never hidden
//! behind calls left in flight), [`VmError::Full`] (`call_limit` calls in flight; a call counts from
//! `post` until its result is **taken**, so results waiting in the handle count too).
//!
//! The thread runs calls one at a time, in the order posted, interleaved with the other requests in the
//! order sent: a call posted before `request_stop` runs before `shutdown()`. A call ends as a
//! [`CallOutcome`]: `NotRegistered` (no Lua ran), `Returned` (one `Option<ConfigValue>` per returned
//! value, `None` for `nil`, so `return nil, false, nil` is `[None, Some(Bool(false)), None]`, distinct
//! from `return`), or `Failed` with a message and a traceback (a Lua error, or a returned value that
//! cannot be converted). **A failed callback is caught, not rolled back:** the VM keeps serving and
//! whatever the callback did before failing (globals, log records) stays done. An error with the cancel
//! flag set is a cancellation, and a panic is owner-fatal; the thread ends and no result is sent for
//! that call.
//!
//! [`take_result`](VmOwner::take_result) never waits and is legal in every state. Each call yields **at
//! most one** result: calls still queued or running when the thread ends yield none, and once the handle
//! is Finished nothing is in flight. Results that arrive while the handle waits in `await_stopped`,
//! `stop` or a poll are kept for `take_result`, never taken for the awaited report. A looping callback
//! is interrupted by the instruction hook on cancel.
//!
//! # Observing a failure
//!
//! A started owner whose thread fails (a host panic in a posted callback, a panic, or an error that ends a
//! running call after the cancel flag was set) sends a terminal `Failed` report and ends; no call result
//! is sent. [`take_failure`](VmOwner::take_failure) lets a caller see it without waiting.
//!
//! **Contract: the owner thread sends a terminal `Failed` report before its report wake and before its
//! final wake.** So a `take_failure` made after any wake that followed the failure sees it, even while
//! [`is_finished`](VmOwner::is_finished) is still false because the thread has not yet returned.
//!
//! `take_failure` never waits and never sets the cancel flag. It is legal in Ready and Started; in any
//! other state it returns `None` and consumes nothing, so a report awaited by `wait_ready`, a poll,
//! `await_started` or `await_stopped` stays for that call. It takes every report already received, keeps
//! call results for [`take_result`](VmOwner::take_result) in order, and returns a received failure once
//! as the [`VmError`] the waiting calls would return; the handle is then Ending, as `answer` leaves it
//! today. A report channel closed with no report gives [`VmError::Panicked`] ("the owner thread ended
//! without a report"), once. Ending is a logical end, not proof that the thread returned: use
//! `wait_finished` for that.
//!
//! **When a handle has ended for a caller:** `take_failure` returned a failure; the state is Ending or
//! Finished (for example after the caller's own `cancel`, which moves the handle to Ending at once); or
//! [`post`](VmOwner::post) reports [`VmError::Disconnected`]. A cancel that ends an idle thread, or a
//! callback that completes normally after cancellation, sends no failure report, so a caller that
//! cancelled does not wait for one. `take_failure` returns `None` in Ending and Finished.
//!
//! # The report wake
//!
//! When [`VmSettings::on_report`] is set the owner thread calls it, outside any lock it holds, after it
//! sends each report (ready, a start or stop result, a failure, a call result), and once more as the very
//! last thing before the thread ends (after the stdout flush and after the state was dropped). `ck-lua`
//! attaches no meaning to it. A panic in it is caught and ignored, so it cannot change the outcome.
//! **It must be short and must not block:** it runs arbitrary code on the owner thread, and catching a
//! panic does not bound a wake that blocks. **The final wake is a notification, not proof:** when it
//! runs, [`is_finished`](VmOwner::is_finished) may still be false; a caller observes the end with the
//! bounded `wait_finished` (or `await_stopped`).
//!
//! # The completion hook
//!
//! When [`VmSettings::on_complete`] is set the owner thread calls it with a [`Completion`], outside any
//! lock it holds and with no access to the Lua state, **immediately before the report that is about to be
//! sent** and so before that report's wake:
//!
//! - after a start request whose `startup()` returned normally, or with no `startup` defined
//!   ([`Completion::Startup`]), before the `Done` report;
//! - after every posted call that produces a result, whatever its outcome (the callback returned, raised
//!   a Lua error or returned a value that cannot be converted, or none was registered under the name:
//!   [`Completion::Call`] with its [`CallId`]), before that result's report. A callback that fails is
//!   caught, not rolled back, so the hook runs after it exactly as after one that returns: whatever it
//!   changed before the error has happened when the hook runs, and the failed result reaches the caller
//!   only after the hook.
//!
//! It does not run for the main chunk, for `shutdown()`, for a start whose `startup()` failed, or for a
//! request whose result is not sent because the VM was cancelled or a host function panicked.
//!
//! `ck-lua` attaches no meaning to it. **It must be short and must not block or wait for another thread's
//! progress**, like the report wake. **A panic in it is owner-fatal** (unlike the report wake's, which is
//! ignored): it ends the serving loop like a panic anywhere in it, the owner reports
//! [`VmError::Panicked`] and ends, and no result is sent for that request.
//!
//! # Host surface
//!
//!
//! - `log.error`, `log.warn`, `log.info`, `log.debug` and `log.trace` write records in category
//!   `lua.<name>` through the installed global `ck-log` logger, with the waiting call (scripts are
//!   ordinary producers). Without an installed logger they do nothing.
//! - `config` is an immutable userdata proxy over a copy of the [`ConfigValue`] tree. Reading, `#` on
//!   arrays, `pairs` and `ipairs` work; every assignment raises a Lua error, and `rawset` (which accepts
//!   only tables) raises Lua's own argument error. A script can still rebind its own global `config`.
//! - `print` and the rest of the standard library are left exactly as Lua provides them.

mod call;
mod config;
mod convert;
mod error;
mod extension;
#[cfg(test)]
mod hooks;
mod host;
mod interrupt;
mod owner;
mod runner;
#[cfg(test)]
mod tests;
mod unwind;

pub use call::{CallId, CallOutcome, CallResult, Completion, Progress};
pub use config::ConfigValue;
pub use error::{VmError, VmState};
pub use extension::{CallbackObserver, Extension, HostFunction};
pub use owner::{VmOwner, VmSettings};
