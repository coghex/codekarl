//! Lossless logging with back-pressure for Codekarl programs.
//!
//! A [`Logger`] filters records by level and category, formats each one in the calling thread and
//! hands the whole line to one writer thread through a bounded queue. [`Logger::flush`] waits at
//! most a deadline and says whether everything was written.
//!
//! The crate depends on std only and knows nothing about configuration files, Lua or Metal.
//!
//! # Three ways in
//!
//! - **[`Logger::log`] and the macros `error!` … `trace!` never drop a line to contention or to a
//!   full queue.** They may wait: briefly on a lock collision, and, when the queue is full, until
//!   the writer has made room. There is no timeout. If the sink never makes progress the emitting
//!   thread waits for ever, as a program writing to a full pipe does. A record is lost, and counted,
//!   only if the sink has failed (the queue is closed), its category is invalid or its message does
//!   not format. Ordinary producers (services, Lua VMs, application code) use these.
//! - **[`Logger::log_within`] / `log_within!`** waits like `log` but never past a deadline, and
//!   never calls a blocking lock or a condition-variable wait. It can drop whenever admission
//!   cannot complete by the deadline (lock contention lasting that long, a queue that stays full
//!   behind a slow or stalled sink, a closed queue), and says so with an [`Admission`].
//! - **[`Logger::try_log`] / `try_log!`** does not wait at all, and can drop under ordinary lock
//!   contention.
//!
//! **Control-path rule.** Code that owns deadlines, cancellation or the process exit (a
//! supervisor, a runtime's lifecycle, the executable's main thread, the panic hook) emits every
//! record, ordinary progress and failure records included, through `log_within` (or `try_log`
//! where no wait at all is acceptable), never through the waiting `log`: otherwise a full queue
//! behind a stalled sink would stall the code that enforces the deadlines. Its deadline is the
//! earlier of a preferred allowance of 100 ms (configurable by the caller) and its own next
//! deadline, which logging never extends or resets; a deadline that has already passed means one
//! non-waiting attempt. A control-path record can therefore still be dropped, and is then counted.
//!
//! # Errors are reserved
//!
//! The owner's rule: progress records may be dropped under load, but errors are reserved and given a
//! bounded grace. An error-level record given to `log_within` skips the filter and the settings lock
//! (it uses retained format flags), and if normal admission fails by its deadline it goes to a
//! separate reserve of [`RESERVE_SLOTS`] (1024) slots, without ever waiting for a lock, and the call
//! returns [`Admission::Queued`]. Before it takes each new batch, and before it answers a flush, the
//! writer writes every reserved record, oldest first; a reserved error can therefore go out ahead of
//! earlier, ordinary records that were still queued (E, A, B or A, E, B or A, B, E), and that is the
//! only reordering. [`Logger::await_errors`] waits (bounded) until every error admitted before the
//! call, by either route, has been written and the sink flushed after it; it returns at once when
//! none is undelivered, whatever progress records are pending. An error can still be lost, and
//! counted, when the output is stalled or has failed, when it is so slow that the error is not
//! written in time, or when more than 1024 errors are pending or every free slot is contended at
//! once. Delivery is tracked by per-record sequence numbers and a contiguous watermark, never by
//! totals; see the `logger` module's source documentation for the closure protocol and the bound on the
//! out-of-order set.
//!
//! # What is guaranteed, and for what
//!
//! - **Scope.** The guarantees cover records emitted *through this logger* (`log`, `log_within`,
//!   `try_log` and the macros) only. Other output to the same stream (Lua's `print`, a native
//!   library's writes, bytes written to stderr once it follows stdout) bypasses the queue: it can
//!   block, it can interleave with log lines, and the logger neither counts nor flushes it.
//! - **Lines are whole.** Each record is written with one `write_all`, by the writer thread alone,
//!   so lines from different threads never interleave. Records from one thread stay in order.
//! - **Loss is counted.** A dropped record is added to a cumulative total that is never reset, and
//!   a flush is [`FlushOutcome::Complete`] only if that total is zero. The writer reports drops with
//!   a `WARN  log: <n> record(s) dropped` line as soon as it can.
//! - **Waiting is bounded where it has to be.** `log_within` returns at most about one attempt plus
//!   one 1 ms park after its deadline, whatever any other thread holds; `try_log` never waits;
//!   [`Logger::flush`] is bounded by its deadline even while emitters wait; and dropping a
//!   [`Logger`] never joins a writer that is blocked in a write. The caller's own formatting work
//!   (including the `Display` implementation it passes) and operating-system scheduling are outside
//!   these bounds.
//!
//! # Example
//!
//! ```
//! use std::time::Duration;
//! use ck_log::{Filter, FlushOutcome, Level, Logger, Settings};
//!
//! let settings = Settings {
//!     filter: Filter::new(Level::Info),
//!     timestamps: false,
//!     colour: false,
//! };
//! let logger = Logger::new(Box::new(std::io::sink()), settings)?;
//! logger.log(Level::Info, "runtime", "start-up complete");
//! assert_eq!(logger.flush(Duration::from_secs(5)), FlushOutcome::Complete);
//! # Ok::<(), std::io::Error>(())
//! ```

mod filter;
mod format;
mod global;
mod level;
mod logger;
mod unwind;

pub use filter::{Filter, InvalidCategory, is_valid_category};
pub use format::ColourMode;
pub use global::{InstallError, install, installed};
#[doc(hidden)]
pub use global::{await_errors_global, log_global, log_within_global, try_log_global};
pub use level::{Level, ParseLevelError};
pub use logger::{Admission, FlushOutcome, Logger, RESERVE_SLOTS, Settings};

/// Emits an [`Level::Error`] record to the installed logger: `error!("category", "text {value}")`.
///
/// Waits like [`Logger::log`] (and so never drops the line to contention or a full queue); not for
/// code on a control path, which uses `log_within!`. Does nothing before [`install`].
#[macro_export]
macro_rules! error {
    ($category:expr, $($arg:tt)+) => {
        $crate::log_global($crate::Level::Error, $category, ::core::format_args!($($arg)+))
    };
}

/// Emits a [`Level::Warn`] record to the installed logger: `warn!("category", "text {value}")`.
///
/// Waits like [`Logger::log`]; not for code on a control path, which uses `log_within!`. Does
/// nothing before [`install`].
#[macro_export]
macro_rules! warn {
    ($category:expr, $($arg:tt)+) => {
        $crate::log_global($crate::Level::Warn, $category, ::core::format_args!($($arg)+))
    };
}

/// Emits an [`Level::Info`] record to the installed logger: `info!("category", "text {value}")`.
///
/// Waits like [`Logger::log`]; not for code on a control path, which uses `log_within!`. Does
/// nothing before [`install`].
#[macro_export]
macro_rules! info {
    ($category:expr, $($arg:tt)+) => {
        $crate::log_global($crate::Level::Info, $category, ::core::format_args!($($arg)+))
    };
}

/// Emits a [`Level::Debug`] record to the installed logger: `debug!("category", "text {value}")`.
///
/// Waits like [`Logger::log`]; not for code on a control path, which uses `log_within!`. Does
/// nothing before [`install`].
#[macro_export]
macro_rules! debug {
    ($category:expr, $($arg:tt)+) => {
        $crate::log_global($crate::Level::Debug, $category, ::core::format_args!($($arg)+))
    };
}

/// Emits a [`Level::Trace`] record to the installed logger: `trace!("category", "text {value}")`.
///
/// Waits like [`Logger::log`]; not for code on a control path, which uses `log_within!`. Does
/// nothing before [`install`].
#[macro_export]
macro_rules! trace {
    ($category:expr, $($arg:tt)+) => {
        $crate::log_global($crate::Level::Trace, $category, ::core::format_args!($($arg)+))
    };
}

/// Emits a record to the installed logger within a deadline and says what happened to it:
/// `log_within!(Level::Info, deadline, "category", "text {value}")`, where `deadline` is an
/// [`Instant`](std::time::Instant). See [`Logger::log_within`]: this is the macro for code on a
/// control path.
///
/// Before [`install`] nothing is emitted and nothing is lost, and the result is
/// [`Admission::Filtered`].
#[macro_export]
macro_rules! log_within {
    ($level:expr, $deadline:expr, $category:expr, $($arg:tt)+) => {
        $crate::log_within_global($level, $deadline, $category, ::core::format_args!($($arg)+))
    };
}

/// Emits a record to the installed logger without waiting and says what happened to it:
/// `try_log!(Level::Info, "category", "text {value}")`. See [`Logger::try_log`]: it can drop under
/// ordinary lock contention.
///
/// Before [`install`] nothing is emitted and nothing is lost, and the result is
/// [`Admission::Filtered`].
#[macro_export]
macro_rules! try_log {
    ($level:expr, $category:expr, $($arg:tt)+) => {
        $crate::try_log_global($level, $category, ::core::format_args!($($arg)+))
    };
}
