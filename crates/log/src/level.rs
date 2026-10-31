//! Severity levels.

use std::fmt;
use std::str::FromStr;

/// A record's severity, in verbosity order: `Error` is the least verbose and `Trace` the most.
///
/// `Error < Warn < Info < Debug < Trace`, so a record is emitted when its level is at most as
/// verbose as the threshold that applies to its category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Something failed.
    Error,
    /// Something is wrong but the work continues.
    Warn,
    /// Normal progress.
    Info,
    /// Detail for diagnosing a problem.
    Debug,
    /// Finer detail than `Debug`.
    Trace,
}

impl Level {
    /// The upper-case name used in log lines: `ERROR`, `WARN`, `INFO`, `DEBUG` or `TRACE`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// The text given to [`Level::from_str`] is not a level name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseLevelError {
    value: String,
}

impl ParseLevelError {
    /// The rejected text.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for ParseLevelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown log level {:?}: expected error, warn, info, debug or trace",
            self.value
        )
    }
}

impl std::error::Error for ParseLevelError {}

impl FromStr for Level {
    type Err = ParseLevelError;

    /// Accepts exactly the lower-case names `error`, `warn`, `info`, `debug` and `trace`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            _ => Err(ParseLevelError {
                value: text.to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exactly_the_five_lower_case_names() {
        for (text, level) in [
            ("error", Level::Error),
            ("warn", Level::Warn),
            ("info", Level::Info),
            ("debug", Level::Debug),
            ("trace", Level::Trace),
        ] {
            assert_eq!(text.parse::<Level>(), Ok(level));
        }
    }

    #[test]
    fn rejects_anything_else_naming_the_value() {
        for text in ["Info", "INFO", "warning", "", " info", "info ", "off"] {
            let error = text.parse::<Level>().expect_err("must be rejected");
            assert_eq!(error.value(), text);
            assert!(error.to_string().contains(&format!("{text:?}")), "{error}");
        }
    }

    #[test]
    fn orders_by_verbosity_with_error_least_verbose() {
        assert!(Level::Error < Level::Warn);
        assert!(Level::Warn < Level::Info);
        assert!(Level::Info < Level::Debug);
        assert!(Level::Debug < Level::Trace);
    }

    #[test]
    fn names_are_upper_case() {
        let names: Vec<_> = [
            Level::Error,
            Level::Warn,
            Level::Info,
            Level::Debug,
            Level::Trace,
        ]
        .map(|level| level.to_string())
        .into();
        assert_eq!(names, ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"]);
    }
}
