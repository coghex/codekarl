//! The process-wide logger and its macros.

use std::fmt;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::{Admission, Level, Logger};

static GLOBAL: OnceLock<Logger> = OnceLock::new();

/// A global logger is already installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallError;

impl fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a global logger is already installed")
    }
}

impl std::error::Error for InstallError {}

/// Installs `logger` as the process-wide logger that the macros use.
///
/// # Errors
///
/// Returns [`InstallError`] when a logger is already installed; the first one stays in place and
/// `logger` is dropped.
pub fn install(logger: Logger) -> Result<(), InstallError> {
    GLOBAL.set(logger).map_err(|_| InstallError)
}

/// The installed logger, for `set_settings` and `flush`, or `None` before [`install`].
#[must_use]
pub fn installed() -> Option<&'static Logger> {
    GLOBAL.get()
}

/// The macros' entry point: emits to the installed logger, and does nothing before [`install`].
#[doc(hidden)]
pub fn log_global(level: Level, category: &str, message: fmt::Arguments<'_>) {
    if let Some(logger) = GLOBAL.get() {
        logger.log(level, category, message);
    }
}

/// The `log_within!` macro's entry point: emits to the installed logger within `deadline`. Before
/// [`install`] nothing is emitted and nothing is lost, so the result is [`Admission::Filtered`].
#[doc(hidden)]
pub fn log_within_global(
    level: Level,
    deadline: Instant,
    category: &str,
    message: fmt::Arguments<'_>,
) -> Admission {
    GLOBAL.get().map_or(Admission::Filtered, |logger| {
        logger.log_within(level, category, message, deadline)
    })
}

/// The `try_log!` macro's entry point: emits to the installed logger without waiting. Before
/// [`install`] nothing is emitted and nothing is lost, so the result is [`Admission::Filtered`].
#[doc(hidden)]
pub fn try_log_global(level: Level, category: &str, message: fmt::Arguments<'_>) -> Admission {
    GLOBAL.get().map_or(Admission::Filtered, |logger| {
        logger.try_log(level, category, message)
    })
}

/// The entry point for the executable's exit path: waits at most `deadline` until every error record
/// admitted so far has been written and flushed (see [`Logger::await_errors`]). With no logger
/// installed there is no error to wait for, so the result is `true`.
#[doc(hidden)]
#[must_use]
pub fn await_errors_global(deadline: Duration) -> bool {
    GLOBAL
        .get()
        .is_none_or(|logger| logger.await_errors(deadline))
}
