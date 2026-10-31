//! The owner thread: creates the state, installs the host surface, runs the entry script and then
//! serves `startup()` and `shutdown()` requests until it is told to end or is cancelled.
//!
//! The Lua state is created, used and dropped here, on this thread only.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use mlua::{Function, Lua, LuaOptions, MultiValue, StdLib, Value};

use crate::call::{CallId, CallOutcome, CallResult, Completion};
use crate::config::ConfigValue;
use crate::extension::{Callbacks, Extension};
use crate::interrupt::Fatal;
use crate::{convert, host, interrupt, unwind};

/// What the handle asks the thread to do.
#[derive(Debug)]
pub(crate) enum Command {
    /// Call the global `startup()` if defined.
    Start,
    /// Call the global `shutdown()` if defined, then end.
    Stop,
    /// Look at the cancel flag (sent by cancel and by an abandoned request).
    Wake,
    /// Call the registered callback `callback` with `args`.
    Call {
        /// The call's identifier.
        id: CallId,
        /// The callback's name.
        callback: String,
        /// The arguments.
        args: Vec<ConfigValue>,
    },
}

/// Why the thread could not carry on.
#[derive(Debug)]
pub(crate) enum Failure {
    /// A Lua error.
    Script {
        /// Lua's message.
        message: String,
        /// Lua's traceback text.
        traceback: String,
    },
    /// A panic caught at the thread boundary.
    Panicked {
        /// The panic message.
        message: String,
    },
    /// A running call ended with an error after the cancel flag was set.
    Cancelled,
}

/// What the thread tells the handle.
#[derive(Debug)]
pub(crate) enum Report {
    /// Initialisation finished: the main chunk ran.
    Ready,
    /// The `startup()` or `shutdown()` request succeeded.
    Done,
    /// The thread failed; it ends.
    Failed(Failure),
    /// A posted call ended (it is not the answer to a lifecycle request).
    Call(CallResult),
}

/// The report wake: called on the owner thread after each report, and once at the very end.
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

/// The completion hook: called on the owner thread immediately before the report it completes.
pub(crate) type CompleteHook = Arc<dyn Fn(Completion) + Send + Sync>;

/// Everything the thread is given.
pub(crate) struct Job {
    pub(crate) name: String,
    pub(crate) entry: PathBuf,
    pub(crate) config: ConfigValue,
    pub(crate) extensions: Vec<Extension>,
    pub(crate) on_report: Option<Wake>,
    pub(crate) on_complete: Option<CompleteHook>,
    pub(crate) control: Receiver<Command>,
    pub(crate) reports: Sender<Report>,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) fatal: Fatal,
}

impl Job {
    /// Sends a report, then calls the wake. A panic in the wake is caught and ignored, so it cannot
    /// change the thread's outcome; the wake runs outside any lock this thread holds.
    fn report(&self, report: Report) {
        let _ = self.reports.send(report);
        self.wake();
    }

    /// Reports a ready or failure report, which the tests' `Reported` point marks.
    fn report_and_mark(&self, report: Report) {
        self.report(report);
        #[cfg(test)]
        crate::hooks::notify(&self.name, crate::hooks::Point::Reported);
    }

    /// Runs the completion hook, if there is one, before the report it completes. Unlike the wake, a
    /// panic in it is not caught here: it unwinds to `thread_main`'s `catch_unwind` and is owner-fatal,
    /// so a failed publication is never silent. It runs outside any lock this thread holds.
    fn complete(&self, completion: Completion) {
        if let Some(hook) = &self.on_complete {
            hook(completion);
        }
    }

    fn wake(&self) {
        if let Some(wake) = &self.on_report
            && let Err(payload) = catch_unwind(AssertUnwindSafe(|| wake()))
        {
            unwind::dispose(payload);
        }
    }
}

/// The traceback text for an error that happened with no Lua frame to trace.
const NO_FRAME: &str = "stack traceback:\n\t[C]: in ck-lua (no Lua frame was running)";

impl Failure {
    /// A failure raised by the host rather than by a running script.
    fn host(message: impl Into<String>) -> Self {
        Self::Script {
            message: message.into(),
            traceback: NO_FRAME.to_owned(),
        }
    }

    /// A failure from a Lua error, with its traceback (made up of a fixed line if Lua gave none, as
    /// for a syntax error, which happens before any frame runs).
    fn from_lua(error: &mlua::Error) -> Self {
        let (message, traceback) = match error {
            mlua::Error::CallbackError { cause, traceback } => {
                (cause.to_string(), traceback.clone())
            }
            mlua::Error::RuntimeError(text) => match text.split_once("\nstack traceback:") {
                Some((message, rest)) => (message.to_owned(), format!("stack traceback:{rest}")),
                None => (text.clone(), String::new()),
            },
            other => (other.to_string(), String::new()),
        };
        Self::Script {
            message,
            traceback: if traceback.is_empty() {
                NO_FRAME.to_owned()
            } else {
                traceback
            },
        }
    }
}

/// The Lua state with its stdout flush: it flushes, then drops the state, on every way out
/// (stop, failure, cancel or panic).
struct State {
    flush: Option<Function>,
    /// The registered callbacks: they hold references into `lua`, so they go first.
    callbacks: Callbacks,
    /// Told when teardown begins, so a host panic from a finalizer is told apart from one before.
    fatal: Fatal,
    lua: Lua,
    /// The owner's name, for the test hooks.
    #[cfg(test)]
    name: String,
}

impl Drop for State {
    fn drop(&mut self) {
        self.fatal.begin_teardown();
        // The final flush must run even though the cancel flag is set: take the hook off first.
        self.lua.remove_global_hook();
        if let Some(flush) = self.flush.take() {
            // A failing flush (or a panic raised by it) must not stop the state from being dropped, and
            // nor may the drop of the panic's payload.
            let flushed = catch_unwind(AssertUnwindSafe(|| {
                #[cfg(test)]
                crate::hooks::at(&self.name, crate::hooks::Point::Flushing, Some(&self.lua));
                flush.call::<()>(())
            }));
            if let Err(payload) = flushed {
                unwind::dispose(payload);
            }
        }
        self.callbacks.borrow_mut().clear();
    }
}

/// The thread body: runs the owner and reports a panic as a failure instead of a lost thread. The
/// state has been flushed and dropped when `run` is left, whichever way, so the reply is decided
/// afterwards: a host panic recorded during that teardown (a finalizer calling a host function) turns
/// a successful stop into `Panicked`, and after a failure that was selected before teardown began (the
/// primary cause, whether or not its report has been delivered yet) it is logged as one error record,
/// so it is never lost. The last thing the thread does is the final wake.
pub(crate) fn thread_main(job: &Job) {
    let reply = match catch_unwind(AssertUnwindSafe(|| run(job))) {
        Ok(Ok(reply)) => reply,
        Ok(Err(failure)) => Some(Report::Failed(failure)),
        Err(payload) => {
            let message = unwind::message(payload.as_ref());
            unwind::dispose(payload);
            Some(Report::Failed(Failure::Panicked { message }))
        }
    };
    let reply = match (reply, job.fatal.teardown_message()) {
        (Some(Report::Done), Some(message)) => Some(Report::Failed(Failure::Panicked { message })),
        (reply, Some(message)) => {
            log_teardown_panic(job, &message);
            reply
        }
        (reply, None) => reply,
    };
    #[cfg(test)]
    let reply = reply.filter(|_| !crate::hooks::report_suppressed(&job.name));
    match reply {
        Some(report @ Report::Failed(_)) => job.report_and_mark(report),
        Some(report) => job.report(report),
        None => {}
    }
    job.wake();
    #[cfg(test)]
    crate::hooks::notify(&job.name, crate::hooks::Point::BeforeReturn);
}

/// How long the record of a teardown panic may wait for the logger.
const TEARDOWN_LOG: Duration = Duration::from_millis(100);

/// Logs `host function panicked during teardown: <message>` as an error in `lua.<name>`, through
/// `log_within`, so a stalled logger cannot stall the thread's end.
fn log_teardown_panic(job: &Job, message: &str) {
    if let Some(logger) = ck_log::installed() {
        let _ = logger.log_within(
            ck_log::Level::Error,
            &format!("lua.{}", job.name),
            format_args!("host function panicked during teardown: {message}"),
            Instant::now() + TEARDOWN_LOG,
        );
    }
}

/// Classifies the error that ended a running call: a host function that panicked makes it a panic
/// (whatever the unwinding raised); otherwise, after the cancel flag was set, *any* error that ends the
/// call is a cancellation (by the flag, never by the error's text or value); with the flag clear it is
/// the script's own error.
fn classify(job: &Job, error: &mlua::Error) -> Failure {
    if let Some(message) = job.fatal.message() {
        Failure::Panicked { message }
    } else if job.cancel.load(Ordering::SeqCst) {
        Failure::Cancelled
    } else {
        Failure::from_lua(error)
    }
}

/// Calls the global function `name` if it is defined.
fn call_global(job: &Job, lua: &Lua, name: &str) -> Result<(), Failure> {
    let value: Value = lua
        .globals()
        .get(name)
        .map_err(|error| classify(job, &error))?;
    match value {
        Value::Nil => Ok(()),
        Value::Function(function) => function
            .call::<()>(())
            .map_err(|error| classify(job, &error)),
        _ => Err(Failure::host(format!(
            "the global {name} is not a function"
        ))),
    }
}

/// Runs the entry script's main chunk.
fn run_main(job: &Job, lua: &Lua) -> Result<(), Failure> {
    let source = std::fs::read(&job.entry)
        .map_err(|error| Failure::host(format!("cannot read {}: {error}", job.entry.display())))?;
    lua.load(source)
        .set_name(format!("@{}", job.entry.display()))
        .exec()
        .map_err(|error| classify(job, &error))
}

/// Whether the thread must end now. A host function that panicked is an error (a `Panicked` failure),
/// even when the script went on to finish normally (a panic raised from a finalizer cannot unwind into
/// the script); otherwise it is the cancel flag.
fn ending(job: &Job) -> Result<bool, Failure> {
    match job.fatal.message() {
        Some(message) => Err(Failure::Panicked { message }),
        None => Ok(job.cancel.load(Ordering::SeqCst)),
    }
}

/// The owner: initialise, report ready, serve requests, end. `Ok` is an end without a failure to
/// report: `Some(Done)` is the reply to a stop, to be sent only after the state is flushed and
/// dropped; `None` is a cancel or the handle going away.
fn run(job: &Job) -> Result<Option<Report>, Failure> {
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())
        .map_err(|error| Failure::host(format!("cannot create the Lua state: {error}")))?;
    let mut state = State {
        flush: None,
        callbacks: Callbacks::default(),
        fatal: job.fatal.clone(),
        lua,
        #[cfg(test)]
        name: job.name.clone(),
    };
    state.flush = Some(
        host::capture_flush(&state.lua)
            .map_err(|error| Failure::host(format!("cannot capture stdout: {error}")))?,
    );
    #[cfg(test)]
    crate::hooks::at(
        &job.name,
        crate::hooks::Point::StateCreated,
        Some(&state.lua),
    );
    if ending(job)? {
        return Ok(None);
    }
    interrupt::install(&state.lua, &job.fatal)
        .map_err(|error| Failure::host(format!("cannot install the interruption: {error}")))?;
    host::install(&state.lua, &job.name, &job.config).map_err(|error| Failure::from_lua(&error))?;
    for extension in &job.extensions {
        extension
            .install(&state.lua, &job.fatal, &state.callbacks)
            .map_err(|error| Failure::from_lua(&error))?;
    }
    #[cfg(test)]
    crate::hooks::install(&job.name, &state.lua).map_err(|error| Failure::from_lua(&error))?;
    #[cfg(test)]
    crate::hooks::at(
        &job.name,
        crate::hooks::Point::SurfaceInstalled,
        Some(&state.lua),
    );
    if ending(job)? {
        return Ok(None);
    }
    #[cfg(test)]
    crate::hooks::at(&job.name, crate::hooks::Point::BeforeMain, Some(&state.lua));
    if ending(job)? {
        return Ok(None);
    }
    run_main(job, &state.lua)?;
    #[cfg(test)]
    crate::hooks::at(&job.name, crate::hooks::Point::AfterMain, Some(&state.lua));
    if ending(job)? {
        return Ok(None);
    }
    job.report_and_mark(Report::Ready);
    serve(job, &state)
}

/// Serves requests until stop, cancel or the handle going away. A result that arrives after the
/// cancel flag was set (an abandoned request) is discarded and the thread ends without more Lua.
fn serve(job: &Job, state: &State) -> Result<Option<Report>, Failure> {
    let lua = &state.lua;
    while let Ok(command) = job.control.recv() {
        if ending(job)? {
            return Ok(None);
        }
        match command {
            Command::Wake => {}
            Command::Start => {
                #[cfg(test)]
                crate::hooks::at(&job.name, crate::hooks::Point::BeforeStartup, Some(lua));
                call_global(job, lua, "startup")?;
                if ending(job)? {
                    return Ok(None);
                }
                job.complete(Completion::Startup);
                job.report(Report::Done);
            }
            Command::Stop => {
                #[cfg(test)]
                crate::hooks::at(&job.name, crate::hooks::Point::BeforeShutdown, Some(lua));
                call_global(job, lua, "shutdown")?;
                // The reply waits for the flush and the state drop (see `thread_main`).
                return Ok((!ending(job)?).then_some(Report::Done));
            }
            Command::Call { id, callback, args } => {
                let outcome = run_call(job, state, &callback, &args)?;
                if ending(job)? {
                    return Ok(None);
                }
                job.complete(Completion::Call(id));
                job.report(Report::Call(CallResult {
                    id,
                    callback,
                    outcome,
                }));
            }
        }
    }
    Ok(None)
}

/// Calls the registered callback `callback`. A Lua error with the cancel flag clear is the call's own
/// failure and the VM keeps serving; a cancellation or a host panic ends the thread (`Err`).
fn run_call(
    job: &Job,
    state: &State,
    callback: &str,
    args: &[ConfigValue],
) -> Result<CallOutcome, Failure> {
    // Cloned out of the map: the callback may call its own setter.
    let registered = state.callbacks.borrow().get(callback).cloned();
    let Some(function) = registered else {
        return Ok(CallOutcome::NotRegistered);
    };
    let lua = &state.lua;
    let converted: mlua::Result<MultiValue> =
        args.iter().map(|arg| convert::to_lua(lua, arg)).collect();
    let returned = converted.and_then(|values| function.call::<MultiValue>(values));
    match returned {
        // `serve` checks `ending` next, which catches a panic that could not unwind into the script.
        Ok(values) => Ok(returned_outcome(&values)),
        Err(error) => match classify(job, &error) {
            Failure::Script { message, traceback } => {
                Ok(CallOutcome::Failed { message, traceback })
            }
            fatal => Err(fatal),
        },
    }
}

/// The outcome for the values a callback returned: one entry per value, `None` for `nil`; a value that
/// cannot be converted fails the call with a host message naming its position and the reason.
fn returned_outcome(values: &MultiValue) -> CallOutcome {
    let mut converted = Vec::with_capacity(values.len());
    for (position, value) in values.iter().enumerate() {
        match convert::from_lua(value, 0) {
            Ok(value) => converted.push(value),
            Err(error) => {
                return CallOutcome::Failed {
                    message: format!("return value #{}: {error}", position + 1),
                    traceback: NO_FRAME.to_owned(),
                };
            }
        }
    }
    CallOutcome::Returned(converted)
}
