//! The states of a [`VmOwner`](crate::VmOwner) handle and the errors its calls return.

use std::fmt;

/// The state of a [`VmOwner`](crate::VmOwner) handle, as its caller observes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    /// The thread is creating the state and running the main chunk. The handle stays here until
    /// `wait_ready` consumes the ready or failure report.
    Initialising,
    /// Idle, after readiness.
    Ready,
    /// `request_start` was sent; the handle stays here until `await_started` consumes the result.
    Starting,
    /// Idle, after `start` (or `request_start` and a successful `await_started`).
    Started,
    /// `request_stop` was sent; the handle stays here until `await_stopped` consumes the result and
    /// observes the thread finished.
    Stopping,
    /// The thread is ending: after stop, a failure, cancel or abandonment.
    Ending,
    /// The thread has been observed ended and joined.
    Finished,
}

impl fmt::Display for VmState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Initialising => "initialising",
            Self::Ready => "ready",
            Self::Starting => "starting",
            Self::Started => "started",
            Self::Stopping => "stopping",
            Self::Ending => "ending",
            Self::Finished => "finished",
        })
    }
}

/// Why a call on a [`VmOwner`](crate::VmOwner) failed. Every variant names the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VmError {
    /// A Lua error, with Lua's traceback text. The owner goes to [`VmState::Ending`].
    Script {
        /// The owner's name.
        owner: String,
        /// Lua's error message.
        message: String,
        /// Lua's traceback text (never empty).
        traceback: String,
    },
    /// A panic on the owner thread, caught at the thread boundary. The owner goes to
    /// [`VmState::Ending`].
    Panicked {
        /// The owner's name.
        owner: String,
        /// The panic message.
        message: String,
    },
    /// The call was interrupted by cancellation: the owner's cancel flag was set (by `cancel`, by
    /// dropping the handle, or by an abandoned request) and the running Lua call ended with an error
    /// afterwards, normally the interruption the instruction hook raises. The owner is then Ending.
    Cancelled {
        /// The owner's name.
        owner: String,
    },
    /// The deadline passed first. The request is abandoned: whenever the running call returns, its
    /// result is discarded and the thread ends as if cancelled. The owner is not joined by the call;
    /// use `wait_finished`.
    Deadline {
        /// The owner's name.
        owner: String,
    },
    /// The thread could not be created.
    Spawn {
        /// The owner's name.
        owner: String,
        /// The operating system's error.
        message: String,
    },
    /// `post` found the owner's thread ended (the handle is still Started). Nothing was sent.
    Disconnected {
        /// The owner's name.
        owner: String,
    },
    /// `post` found `call_limit` calls in flight (results not yet taken count). Nothing was sent.
    Full {
        /// The owner's name.
        owner: String,
    },
    /// A call that is not legal in the handle's current state. Nothing was sent.
    State {
        /// The owner's name.
        owner: String,
        /// The call that was refused.
        call: &'static str,
        /// The state the handle was in.
        state: VmState,
    },
}

impl VmError {
    /// The name of the owner this error is about.
    #[must_use]
    pub fn owner(&self) -> &str {
        match self {
            Self::Script { owner, .. }
            | Self::Panicked { owner, .. }
            | Self::Cancelled { owner }
            | Self::Deadline { owner }
            | Self::Spawn { owner, .. }
            | Self::Disconnected { owner }
            | Self::Full { owner }
            | Self::State { owner, .. } => owner,
        }
    }
}

impl fmt::Display for VmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Script {
                owner,
                message,
                traceback,
            } => write!(formatter, "lua.{owner}: {message}\n{traceback}"),
            Self::Panicked { owner, message } => {
                write!(formatter, "lua.{owner}: panicked: {message}")
            }
            Self::Cancelled { owner } => write!(formatter, "lua.{owner}: cancelled"),
            Self::Deadline { owner } => write!(formatter, "lua.{owner}: deadline missed"),
            Self::Spawn { owner, message } => {
                write!(formatter, "lua.{owner}: cannot start the thread: {message}")
            }
            Self::Disconnected { owner } => {
                write!(formatter, "lua.{owner}: the owner thread has ended")
            }
            Self::Full { owner } => {
                write!(formatter, "lua.{owner}: too many calls in flight")
            }
            Self::State { owner, call, state } => {
                write!(formatter, "lua.{owner}: {call} is not legal while {state}")
            }
        }
    }
}

impl std::error::Error for VmError {}
