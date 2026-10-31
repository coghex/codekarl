//! Line formatting: timestamps, colour and multi-line messages.

use std::ffi::OsStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::Level;

const SECONDS_PER_DAY: u64 = 86_400;

/// When log lines carry ANSI colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourMode {
    /// Colour only when the output is a terminal and `NO_COLOR` is unset or empty.
    Auto,
    /// Always colour.
    Always,
    /// Never colour.
    Never,
}

impl ColourMode {
    /// Whether colour is on, given whether the output is a terminal and the value of `NO_COLOR`
    /// (`None` when it is unset).
    ///
    /// This is a pure function of its inputs; the caller reads the environment and the terminal.
    #[must_use]
    pub fn enabled(self, is_terminal: bool, no_color: Option<&OsStr>) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => is_terminal && no_color.is_none_or(OsStr::is_empty),
        }
    }
}

/// The ANSI colour escape for a level's name.
const fn colour_code(level: Level) -> &'static str {
    match level {
        Level::Error => "\x1b[31m",
        Level::Warn => "\x1b[33m",
        Level::Info => "\x1b[32m",
        Level::Debug => "\x1b[34m",
        Level::Trace => "\x1b[35m",
    }
}

const COLOUR_RESET: &str = "\x1b[0m";

/// `time` as `YYYY-MM-DDTHH:MM:SS.mmmZ` in UTC, without a time-zone database.
///
/// A time before the Unix epoch prints as the epoch.
pub(crate) fn timestamp(time: SystemTime) -> String {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let seconds = since_epoch.as_secs();
    let (year, month, day) = civil_date(seconds / SECONDS_PER_DAY);
    let in_day = seconds % SECONDS_PER_DAY;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        in_day / 3600,
        in_day % 3600 / 60,
        in_day % 60,
        since_epoch.subsec_millis(),
    )
}

/// The proleptic Gregorian date of `days` days after 1970-01-01, as (year, month, day).
///
/// Howard Hinnant's `civil_from_days`, shifted to start at 0000-03-01 so leap days fall at the end
/// of a year.
fn civil_date(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

/// One record as a line (or several, for a multi-line message), ending in one `\n`.
///
/// `<timestamp> <LEVEL padded to 5> <category>: <message>`, with the timestamp left out when
/// `time` is `None`. Exactly one trailing `\n` is removed from the message first, so `"done\n"`
/// reads as `"done"`; every other newline starts a continuation line prefixed with two spaces.
/// With `colour`, only the level token is wrapped in its ANSI colour; the padding stays outside.
pub(crate) fn line(
    level: Level,
    category: &str,
    message: &str,
    time: Option<SystemTime>,
    colour: bool,
) -> String {
    let mut out = String::with_capacity(message.len() + category.len() + 48);
    if let Some(time) = time {
        out.push_str(&timestamp(time));
        out.push(' ');
    }
    let name = level.name();
    if colour {
        out.push_str(colour_code(level));
        out.push_str(name);
        out.push_str(COLOUR_RESET);
    } else {
        out.push_str(name);
    }
    for _ in name.len()..5 {
        out.push(' ');
    }
    out.push(' ');
    out.push_str(category);
    out.push_str(": ");
    let message = message.strip_suffix('\n').unwrap_or(message);
    for (index, text) in message.split('\n').enumerate() {
        if index > 0 {
            out.push_str("\n  ");
        }
        out.push_str(text);
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64, millis: u32) -> SystemTime {
        UNIX_EPOCH + Duration::new(seconds, millis * 1_000_000)
    }

    #[test]
    fn utc_timestamps_at_fixed_instants() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        // 2024-02-29T12:34:56.789Z: a leap day.
        assert_eq!(
            timestamp(at(1_709_210_096, 789)),
            "2024-02-29T12:34:56.789Z"
        );
        assert_eq!(
            timestamp(at(1_767_225_599, 999)),
            "2025-12-31T23:59:59.999Z"
        );
        assert_eq!(timestamp(at(1_767_225_600, 0)), "2026-01-01T00:00:00.000Z");
        // The example in the design: 2026-10-18T20:30:00.123Z.
        assert_eq!(
            timestamp(at(1_792_355_400, 123)),
            "2026-10-18T20:30:00.123Z"
        );
        // 2100 is not a leap year (century rule), 2000 is (400 rule).
        assert_eq!(timestamp(at(4_107_542_400, 0)), "2100-03-01T00:00:00.000Z");
        assert_eq!(timestamp(at(951_782_400, 0)), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn a_time_before_the_epoch_prints_as_the_epoch() {
        assert_eq!(
            timestamp(UNIX_EPOCH - Duration::from_millis(1)),
            "1970-01-01T00:00:00.000Z"
        );
        assert_eq!(
            timestamp(UNIX_EPOCH - Duration::from_secs(86_400 * 400)),
            "1970-01-01T00:00:00.000Z"
        );
    }

    #[test]
    fn milliseconds_truncate_rather_than_round() {
        let time = UNIX_EPOCH + Duration::new(1, 999_999_999);
        assert_eq!(timestamp(time), "1970-01-01T00:00:01.999Z");
    }

    #[test]
    fn exact_lines_without_timestamps_for_every_level() {
        let cases = [
            (Level::Error, "ERROR runtime: start-up complete\n"),
            (Level::Warn, "WARN  runtime: start-up complete\n"),
            (Level::Info, "INFO  runtime: start-up complete\n"),
            (Level::Debug, "DEBUG runtime: start-up complete\n"),
            (Level::Trace, "TRACE runtime: start-up complete\n"),
        ];
        for (level, expected) in cases {
            assert_eq!(
                line(level, "runtime", "start-up complete", None, false),
                expected
            );
        }
    }

    #[test]
    fn exact_lines_with_timestamps_for_every_level() {
        let time = Some(at(1_792_355_400, 123));
        let cases = [
            (
                Level::Error,
                "2026-10-18T20:30:00.123Z ERROR lua.ui: hello\n",
            ),
            (
                Level::Warn,
                "2026-10-18T20:30:00.123Z WARN  lua.ui: hello\n",
            ),
            (
                Level::Info,
                "2026-10-18T20:30:00.123Z INFO  lua.ui: hello\n",
            ),
            (
                Level::Debug,
                "2026-10-18T20:30:00.123Z DEBUG lua.ui: hello\n",
            ),
            (
                Level::Trace,
                "2026-10-18T20:30:00.123Z TRACE lua.ui: hello\n",
            ),
        ];
        for (level, expected) in cases {
            assert_eq!(line(level, "lua.ui", "hello", time, false), expected);
        }
    }

    #[test]
    fn a_multi_line_message_indents_continuation_lines_and_ends_with_one_newline() {
        assert_eq!(
            line(Level::Info, "c", "first\nsecond\n\nfourth", None, false),
            "INFO  c: first\n  second\n  \n  fourth\n"
        );
        // Exactly one trailing newline is removed; any further newline is a continuation line.
        assert_eq!(
            line(Level::Info, "c", "one\n", None, false),
            "INFO  c: one\n"
        );
        assert_eq!(
            line(Level::Info, "c", "one\n\n", None, false),
            "INFO  c: one\n  \n"
        );
        assert_eq!(line(Level::Info, "c", "\n", None, false), "INFO  c: \n");
        assert_eq!(line(Level::Info, "c", "", None, false), "INFO  c: \n");
    }

    #[test]
    fn colour_wraps_only_the_level_token_and_padding_stays_outside() {
        let cases = [
            (Level::Error, "\x1b[31mERROR\x1b[0m c: m\n"),
            (Level::Warn, "\x1b[33mWARN\x1b[0m  c: m\n"),
            (Level::Info, "\x1b[32mINFO\x1b[0m  c: m\n"),
            (Level::Debug, "\x1b[34mDEBUG\x1b[0m c: m\n"),
            (Level::Trace, "\x1b[35mTRACE\x1b[0m c: m\n"),
        ];
        for (level, expected) in cases {
            assert_eq!(line(level, "c", "m", None, true), expected);
        }
        assert_eq!(
            line(Level::Warn, "c", "m", Some(UNIX_EPOCH), true),
            "1970-01-01T00:00:00.000Z \x1b[33mWARN\x1b[0m  c: m\n"
        );
    }

    #[test]
    fn auto_colour_needs_a_terminal_and_no_non_empty_no_color() {
        let unset = None;
        let empty = Some(OsStr::new(""));
        let set = Some(OsStr::new("1"));
        for (terminal, no_color, expected) in [
            (true, unset, true),
            (true, empty, true),
            (true, set, false),
            (false, unset, false),
            (false, empty, false),
            (false, set, false),
        ] {
            assert_eq!(
                ColourMode::Auto.enabled(terminal, no_color),
                expected,
                "terminal {terminal}, NO_COLOR {no_color:?}"
            );
        }
    }

    #[test]
    fn always_and_never_ignore_their_inputs() {
        for terminal in [true, false] {
            for no_color in [None, Some(OsStr::new("")), Some(OsStr::new("1"))] {
                assert!(ColourMode::Always.enabled(terminal, no_color));
                assert!(!ColourMode::Never.enabled(terminal, no_color));
            }
        }
    }
}
