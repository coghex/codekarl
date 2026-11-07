//! The simulation: time, state, systems and the command queue, one tick at a time.

use std::{collections::VecDeque, error::Error, fmt};

use crate::{
    calendar::{CalendarError, Date, GameTime},
    canonical::{Canonical, CanonicalError, Reader},
    save::{Save, SaveError, SaveMeta, state_digest},
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

/// Who gave a command. Only the player exists now; the other tags are reserved and refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Issuer {
    /// The player.
    Player,
}

impl Canonical for Issuer {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        match self {
            Self::Player => 0_u8.encode(out),
        }
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        match u8::decode(input)? {
            0 => Ok(Self::Player),
            tag => Err(CanonicalError::BadTag { ty: "Issuer", tag }),
        }
    }
}

/// A command as the journal and the save hold it: its place in the order, who gave it, and its canonical bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRecord {
    /// The hour whose boundary the command applies at.
    pub time: GameTime,
    /// The command's place in the order of submission, from 0.
    pub sequence: u64,
    /// Who gave the command.
    pub issuer: Issuer,
    /// The command's canonical bytes.
    pub bytes: Vec<u8>,
}

impl Canonical for CommandRecord {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.time.encode(out)?;
        self.sequence.encode(out)?;
        self.issuer.encode(out)?;
        self.bytes.encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            time: GameTime::decode(input)?,
            sequence: u64::decode(input)?,
            issuer: Issuer::decode(input)?,
            bytes: Vec::decode(input)?,
        })
    }
}

/// Why a journal could not be replayed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    /// The calendar ended before the replay reached its end.
    Calendar(CalendarError),
    /// A journalled command's bytes do not decode.
    Command {
        /// The entry's sequence number.
        sequence: u64,
        /// What was wrong with the bytes.
        error: CanonicalError,
    },
    /// An entry is not where the order puts it: its sequence number is not the next one, or its time is in the past.
    OutOfOrder {
        /// The entry's sequence number.
        sequence: u64,
    },
    /// An entry lies after the time the replay was asked to stop at.
    BeyondEnd {
        /// The entry's sequence number.
        sequence: u64,
    },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Calendar(error) => write!(f, "the replay ran out of calendar: {error}"),
            Self::Command { sequence, error } => {
                write!(f, "journal entry {sequence} does not decode: {error}")
            }
            Self::OutOfOrder { sequence } => {
                write!(f, "journal entry {sequence} is out of order")
            }
            Self::BeyondEnd { sequence } => {
                write!(f, "journal entry {sequence} is after the end of the replay")
            }
        }
    }
}

impl Error for ReplayError {}

type Apply<S, C> = Box<dyn FnMut(&mut S, C) + Send>;

/// A command waiting for the next boundary, with the bytes the journal and the save will hold.
struct Queued<C> {
    record: CommandRecord,
    command: C,
}

/// Game time, a state `S`, the systems that change it and the commands `C` that wait to.
///
/// Nothing here reads the real clock: how many ticks to run is decided by the caller. Randomness is a pure function of the
/// master seed, so the state at a tick is a function of the starting state, the seed and the journal alone.
pub struct Simulation<S, C> {
    time: GameTime,
    seed: u64,
    state: S,
    scheduler: Scheduler<S>,
    queue: VecDeque<Queued<C>>,
    journal: Vec<CommandRecord>,
    next_sequence: u64,
    apply: Apply<S, C>,
}

impl<S, C> Simulation<S, C> {
    /// A simulation at `time` over `state` under the master `seed`, with `apply` changing the state for each command.
    pub fn new(
        time: GameTime,
        seed: u64,
        state: S,
        scheduler: Scheduler<S>,
        apply: impl FnMut(&mut S, C) + Send + 'static,
    ) -> Self {
        Self {
            time,
            seed,
            state,
            scheduler,
            queue: VecDeque::new(),
            journal: Vec::new(),
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

    /// The master seed.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// The state.
    #[must_use]
    pub const fn state(&self) -> &S {
        &self.state
    }

    /// Every command applied so far, in order.
    #[must_use]
    pub fn journal(&self) -> &[CommandRecord] {
        &self.journal
    }

    /// How many commands are queued for the next boundary.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.queue.len()
    }

    /// Applies every queued command in the order submitted, and journals each one.
    ///
    /// A paused driver calls this right after [`Simulation::submit`] so the effect shows at once; a running one lets the next
    /// [`Simulation::step`] do it. Both land at the same boundary in the same order, so they give the same state.
    pub fn apply_pending(&mut self) {
        while let Some(queued) = self.queue.pop_front() {
            self.journal.push(queued.record);
            (self.apply)(&mut self.state, queued.command);
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
        let errors = self
            .scheduler
            .run(&mut self.state, self.seed, self.time, date);
        self.time = self.time.next();
        Ok(errors)
    }
}

impl<S, C: Canonical> Simulation<S, C> {
    /// Queues `command` from the player at the current boundary, before this hour's systems, after the commands already
    /// queued.
    ///
    /// # Errors
    /// Returns [`CanonicalError::TooLong`] when the command is too long to encode; nothing is queued.
    pub fn submit(&mut self, command: C) -> Result<CommandStamp, CanonicalError> {
        let stamp = CommandStamp {
            time: self.time,
            sequence: self.next_sequence,
        };
        let record = CommandRecord {
            time: stamp.time,
            sequence: stamp.sequence,
            issuer: Issuer::Player,
            bytes: command.to_bytes()?,
        };
        self.next_sequence += 1;
        self.queue.push_back(Queued { record, command });
        Ok(stamp)
    }

    /// Re-applies `journal` on top of this simulation: each entry at its own tick, in sequence order, at the command phase
    /// before that tick's systems, until the time reaches `until`. The commands of tick `until` are applied too, as a paused
    /// driver would apply them, but its systems do not run.
    ///
    /// Started from the original initial state and seed, it gives the same state as the original run. Systems that failed
    /// along the way are returned in the order they ran.
    ///
    /// # Errors
    /// Returns a [`ReplayError`] when an entry does not decode, is out of order or lies after `until`, or when the
    /// calendar ends first. The simulation is left wherever the replay stopped.
    pub fn replay(
        &mut self,
        journal: &[CommandRecord],
        until: GameTime,
    ) -> Result<Vec<SystemError>, ReplayError> {
        let mut errors = Vec::new();
        let mut entries = journal.iter().peekable();
        loop {
            while let Some(record) = entries.next_if(|record| record.time <= self.time) {
                if record.time < self.time || record.sequence != self.next_sequence {
                    return Err(ReplayError::OutOfOrder {
                        sequence: record.sequence,
                    });
                }
                let command =
                    C::from_bytes(&record.bytes).map_err(|error| ReplayError::Command {
                        sequence: record.sequence,
                        error,
                    })?;
                self.next_sequence += 1;
                self.queue.push_back(Queued {
                    record: record.clone(),
                    command,
                });
            }
            if self.time >= until {
                break;
            }
            errors.extend(self.step().map_err(ReplayError::Calendar)?);
        }
        if let Some(record) = entries.next() {
            return Err(ReplayError::BeyondEnd {
                sequence: record.sequence,
            });
        }
        self.apply_pending();
        Ok(errors)
    }

    /// Rebuilds a simulation from `save`: its time, seed, state, pending commands and journal exactly as saved.
    ///
    /// A loaded simulation has not run a tick since the save; the driver starts it paused.
    ///
    /// # Errors
    /// Returns a [`SaveError`] when the save's commands are not in a consistent order or a pending command does not decode.
    pub fn from_save(
        save: Save<S>,
        scheduler: Scheduler<S>,
        apply: impl FnMut(&mut S, C) + Send + 'static,
    ) -> Result<Self, SaveError> {
        save.check_commands()?;
        let mut queue = VecDeque::with_capacity(save.pending.len());
        for record in save.pending {
            let command = C::from_bytes(&record.bytes).map_err(|error| SaveError::Command {
                sequence: record.sequence,
                error,
            })?;
            queue.push_back(Queued { record, command });
        }
        let next_sequence = (save.journal.len() + queue.len()) as u64;
        Ok(Self {
            time: save.time,
            seed: save.meta.seed,
            state: save.state,
            scheduler,
            queue,
            journal: save.journal,
            next_sequence,
            apply: Box::new(apply),
        })
    }
}

impl<S: Canonical, C> Simulation<S, C> {
    /// The state digest: the FNV-1a 64-bit hash of the canonical clock, state and pending-command sections, in that order.
    ///
    /// The journal is left out, so a replayed run compares equal to the original. Tests use it; nothing in the game shows it.
    ///
    /// # Errors
    /// Returns [`CanonicalError::TooLong`] when the state is too long to encode.
    pub fn digest(&self) -> Result<u64, CanonicalError> {
        state_digest(
            self.time,
            &self.state,
            self.queue.iter().map(|queued| &queued.record),
        )
    }
}

impl<S: Clone, C> Simulation<S, C> {
    /// The save of this simulation at the current tick boundary, with `meta` as its header.
    #[must_use]
    pub fn save(&self, meta: SaveMeta) -> Save<S> {
        Save {
            meta,
            time: self.time,
            state: self.state.clone(),
            pending: self
                .queue
                .iter()
                .map(|queued| queued.record.clone())
                .collect(),
            journal: self.journal.clone(),
        }
    }
}
