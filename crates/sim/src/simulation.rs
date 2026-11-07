//! The simulation: time, state, systems and the command queue, one tick at a time.

use std::collections::VecDeque;

use crate::{
    calendar::{CalendarError, Date, GameTime},
    scheduler::{Scheduler, SystemError},
};

/// Where a submitted command sits in the order: the hour whose boundary it applies at, and its sequence number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandStamp {
    /// The simulation's time when the command was submitted; it applies before this hour's systems.
    pub time: GameTime,
    /// The command's place in the order of submission, from 0.
    pub sequence: u64,
}

type Apply<S, C> = Box<dyn FnMut(&mut S, C) + Send>;

/// Game time, a state `S`, the systems that change it and the commands `C` that wait to.
///
/// Nothing here reads the real clock: how many ticks to run is decided by the caller.
pub struct Simulation<S, C> {
    time: GameTime,
    state: S,
    scheduler: Scheduler<S>,
    queue: VecDeque<C>,
    next_sequence: u64,
    apply: Apply<S, C>,
}

impl<S, C> Simulation<S, C> {
    /// A simulation at `time` over `state`, with `apply` changing the state for each command.
    pub fn new(
        time: GameTime,
        state: S,
        scheduler: Scheduler<S>,
        apply: impl FnMut(&mut S, C) + Send + 'static,
    ) -> Self {
        Self {
            time,
            state,
            scheduler,
            queue: VecDeque::new(),
            next_sequence: 0,
            apply: Box::new(apply),
        }
    }

    /// The hour the next [`Simulation::step`] runs.
    #[must_use]
    pub const fn time(&self) -> GameTime {
        self.time
    }

    /// The date of the next hour to run.
    ///
    /// # Errors
    /// Returns [`CalendarError::TimeOutOfRange`] after 9999-12-31 23:00.
    pub fn date(&self) -> Result<Date, CalendarError> {
        self.time.date()
    }

    /// The state.
    #[must_use]
    pub const fn state(&self) -> &S {
        &self.state
    }

    /// Queues `command` at the current boundary, before this hour's systems, after the commands already queued.
    pub fn submit(&mut self, command: C) -> CommandStamp {
        let stamp = CommandStamp {
            time: self.time,
            sequence: self.next_sequence,
        };
        self.next_sequence += 1;
        self.queue.push_back(command);
        stamp
    }

    /// Applies every queued command in the order submitted.
    ///
    /// A paused driver calls this right after [`Simulation::submit`] so the effect shows at once; a running one lets the next
    /// [`Simulation::step`] do it. Both land at the same boundary in the same order, so they give the same state.
    pub fn apply_pending(&mut self) {
        while let Some(command) = self.queue.pop_front() {
            (self.apply)(&mut self.state, command);
        }
    }

    /// Runs one tick: the queued commands, then the systems due this hour, then time moves on one hour.
    ///
    /// The returned errors are the systems that failed; the tick completed and time advanced regardless, and the caller
    /// decides whether to pause.
    ///
    /// # Errors
    /// Returns [`CalendarError::TimeOutOfRange`] when the calendar has ended; nothing ran and nothing changed.
    pub fn step(&mut self) -> Result<Vec<SystemError>, CalendarError> {
        let date = self.time.date()?;
        self.apply_pending();
        let errors = self.scheduler.run(&mut self.state, self.time, date);
        self.time = self.time.next();
        Ok(errors)
    }
}
