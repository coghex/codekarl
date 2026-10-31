//! The types of posted callback calls and of the non-abandoning polls.

use std::fmt;

use crate::config::ConfigValue;

/// The answer of a non-abandoning poll ([`poll_ready`](crate::VmOwner::poll_ready),
/// [`poll_started`](crate::VmOwner::poll_started)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// No report yet. Nothing changed: the request is still in flight.
    Pending,
    /// The report arrived and was consumed exactly as the waiting call would have consumed it.
    Done,
}

/// Identifies one posted call; unique per handle, and ordered by posting order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CallId(pub(crate) u64);

impl CallId {
    /// The identifier as a number (the first call a handle posts is 1).
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for CallId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "call {}", self.0)
    }
}

/// What a [`VmSettings::on_complete`](crate::VmSettings::on_complete) hook is told it is completing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    /// A start request: `startup()` returned normally, or no `startup` is defined. The hook runs
    /// immediately before the start's `Done` report.
    Startup,
    /// A posted call that produced a result, whatever its outcome (returned, failed or not registered).
    /// The hook runs immediately before that result's report.
    Call(CallId),
}

/// How a posted call ended.
#[derive(Debug, Clone, PartialEq)]
pub enum CallOutcome {
    /// The callback returned. One entry per value it returned, in order, `None` for each `nil`: `return`
    /// gives `[]`, `return nil` gives `[None]`, `return nil, false, nil` gives
    /// `[None, Some(Bool(false)), None]`.
    Returned(Vec<Option<ConfigValue>>),
    /// No callback was registered under that name; no Lua ran.
    NotRegistered,
    /// The callback raised a Lua error, or returned a value that cannot be converted (the message then
    /// names its position and the reason, and the traceback is a fixed host line). The VM keeps
    /// serving: nothing the callback did before failing is undone.
    Failed {
        /// Lua's error message, or the host's.
        message: String,
        /// Lua's traceback text (never empty).
        traceback: String,
    },
}

/// The result of one posted call, taken with [`take_result`](crate::VmOwner::take_result).
#[derive(Debug, Clone, PartialEq)]
pub struct CallResult {
    /// The call this is the result of.
    pub id: CallId,
    /// The callback name the call was posted with.
    pub callback: String,
    /// How the call ended.
    pub outcome: CallOutcome,
}
