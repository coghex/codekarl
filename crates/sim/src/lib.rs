//! The simulation core: the Julian calendar, the tick scheduler, commands, random streams, saves and the real-time clock.
//!
//! The crate is backend-neutral and windowless, with no Lua, window, GPU or logging dependency. All of its arithmetic is
//! integer, and nothing in [`Simulation`] reads the real clock: [`Clock`] only counts how many ticks are due, from a
//! time the caller supplies, and never changes what a tick does.

#![deny(unsafe_code)]

mod calendar;
mod canonical;
mod clock;
mod random;
mod save;
mod scheduler;
mod simulation;

pub use calendar::{
    CalendarError, Date, FIRST_YEAR, GameTime, LAST_YEAR, days_in_month, is_leap_year,
};
pub use canonical::{Canonical, CanonicalError, Reader, encode_length};
pub use clock::{Clock, DEFAULT_MAX_TICKS_PER_WAKE, Speed, SpeedError};
pub use random::{RandomError, Stream, fnv1a, stream_id};
pub use save::{Expected, HashKind, Save, SaveError, SaveHeader, SaveMeta};
pub use scheduler::{Cadence, Scheduler, SchedulerError, SystemError, SystemResult, Tick};
pub use simulation::{CommandRecord, CommandStamp, Issuer, ReplayError, Simulation};

#[cfg(test)]
mod tests;
