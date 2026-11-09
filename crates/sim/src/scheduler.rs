//! The tick scheduler: named systems on four fixed cadences, run coarser first.

use std::{error::Error, fmt};

use crate::{
    calendar::{Date, GameTime},
    random::{RandomError, Stream, stream_id},
};

/// How often a system runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cadence {
    /// On 1 January at `hour` (0 to 23).
    Yearly {
        /// The hour, 0 to 23.
        hour: u8,
    },
    /// On the 1st of each month at `hour` (0 to 23).
    Monthly {
        /// The hour, 0 to 23.
        hour: u8,
    },
    /// At `hour` (0 to 23) of each day.
    Daily {
        /// The hour, 0 to 23.
        hour: u8,
    },
    /// On every tick.
    Hourly,
}

impl Cadence {
    const fn order(self) -> u8 {
        match self {
            Self::Yearly { .. } => 0,
            Self::Monthly { .. } => 1,
            Self::Daily { .. } => 2,
            Self::Hourly => 3,
        }
    }

    const fn is_due(self, date: Date) -> bool {
        match self {
            Self::Yearly { hour } => date.month == 1 && date.day == 1 && date.hour == hour,
            Self::Monthly { hour } => date.day == 1 && date.hour == hour,
            Self::Daily { hour } => date.hour == hour,
            Self::Hourly => true,
        }
    }
}

/// What a system returns: nothing, or an error the scheduler collects.
pub type SystemResult = Result<(), Box<dyn Error + Send + Sync>>;

/// A failure of one system on one tick.
#[derive(Debug)]
pub struct SystemError {
    /// The name the system was registered under.
    pub system: String,
    /// The date of the tick the system failed on.
    pub date: Date,
    /// The system's own error.
    pub error: Box<dyn Error + Send + Sync>,
}

impl fmt::Display for SystemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "system `{}` failed at {}: {}",
            self.system, self.date, self.error
        )
    }
}

impl Error for SystemError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.error.as_ref())
    }
}

/// A registration the scheduler refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchedulerError {
    /// Another system already has this name.
    DuplicateName {
        /// The refused name.
        name: String,
    },
    /// Another system already has a stream with this name (or one with the same id), or the same name was listed twice.
    DuplicateStream {
        /// The refused stream name.
        name: String,
    },
    /// A yearly, monthly or daily cadence was registered outside hour 0 to 23.
    HourOutOfRange {
        /// The refused hour.
        hour: u8,
    },
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateName { name } => {
                write!(f, "a system named `{name}` is already registered")
            }
            Self::DuplicateStream { name } => {
                write!(f, "a stream named `{name}` is already registered")
            }
            Self::HourOutOfRange { hour } => write!(f, "cadence hour {hour} is outside 0 to 23"),
        }
    }
}

impl Error for SchedulerError {}

/// What a system is given besides the state: the tick's time and date, and its own random streams.
///
/// A system gets only the streams it was registered with, so it cannot reach another system's counters.
pub struct Tick<'a> {
    time: GameTime,
    date: Date,
    streams: &'a mut [Stream],
}

impl Tick<'_> {
    /// The hour being run.
    #[must_use]
    pub const fn time(&self) -> GameTime {
        self.time
    }

    /// The date of the hour being run.
    #[must_use]
    pub const fn date(&self) -> Date {
        self.date
    }

    /// The system's stream called `name`, at this tick.
    ///
    /// # Errors
    /// Returns [`RandomError::UnknownStream`] when the system was not registered with a stream of that name.
    pub fn stream(&mut self, name: &str) -> Result<&mut Stream, RandomError> {
        self.streams
            .iter_mut()
            .find(|stream| stream.name() == name)
            .ok_or_else(|| RandomError::UnknownStream {
                name: name.to_owned(),
            })
    }
}

type System<S> = Box<dyn FnMut(&mut S, &mut Tick<'_>) -> SystemResult + Send>;

struct Registered<S> {
    name: String,
    cadence: Cadence,
    streams: Vec<Stream>,
    run: System<S>,
}

/// Systems over a state `S`, each with a unique name and one cadence.
pub struct Scheduler<S> {
    systems: Vec<Registered<S>>,
}

impl<S> Default for Scheduler<S> {
    fn default() -> Self {
        Self {
            systems: Vec::new(),
        }
    }
}

impl<S> Scheduler<S> {
    /// A scheduler with no systems.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `system` under `name` on `cadence`, with no random streams. Within a cadence, systems run in registration
    /// order.
    ///
    /// # Errors
    /// Returns [`SchedulerError::DuplicateName`] when `name` is already registered; nothing is added.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        cadence: Cadence,
        system: impl FnMut(&mut S, &mut Tick<'_>) -> SystemResult + Send + 'static,
    ) -> Result<(), SchedulerError> {
        self.register_with_streams(name, cadence, &[], system)
    }

    /// Registers `system` under `name` on `cadence`, with the random streams called `streams`.
    ///
    /// Stream names are fixed in code and unique across the scheduler. A system can draw only from its own streams, and
    /// adding a stream never changes another stream's numbers.
    ///
    /// # Errors
    /// Returns [`SchedulerError::DuplicateName`] when `name` is already registered and
    /// [`SchedulerError::DuplicateStream`] when a stream name (or its id) is already taken or listed twice; nothing is added.
    pub fn register_with_streams(
        &mut self,
        name: impl Into<String>,
        cadence: Cadence,
        streams: &[&str],
        system: impl FnMut(&mut S, &mut Tick<'_>) -> SystemResult + Send + 'static,
    ) -> Result<(), SchedulerError> {
        let cadence_hour = match cadence {
            Cadence::Yearly { hour } | Cadence::Monthly { hour } | Cadence::Daily { hour } => {
                Some(hour)
            }
            Cadence::Hourly => None,
        };
        if let Some(hour) = cadence_hour.filter(|hour| *hour >= 24) {
            return Err(SchedulerError::HourOutOfRange { hour });
        }
        let name = name.into();
        if self
            .systems
            .iter()
            .any(|registered| registered.name == name)
        {
            return Err(SchedulerError::DuplicateName { name });
        }
        for (index, stream) in streams.iter().enumerate() {
            let id = stream_id(stream);
            let taken = self
                .systems
                .iter()
                .flat_map(|registered| &registered.streams)
                .any(|existing| existing.id() == id)
                || streams[..index]
                    .iter()
                    .any(|earlier| stream_id(earlier) == id);
            if taken {
                return Err(SchedulerError::DuplicateStream {
                    name: (*stream).to_owned(),
                });
            }
        }
        self.systems.push(Registered {
            name,
            cadence,
            streams: streams
                .iter()
                .map(|stream| Stream::new(0, *stream, GameTime::from_hours(0)))
                .collect(),
            run: Box::new(system),
        });
        Ok(())
    }

    /// Runs the systems due at `date` under the master `seed`, coarser cadences first (yearly, monthly, daily, hourly).
    ///
    /// A failing system is recorded with its name and the date, and the systems after it still run, so the result is
    /// the same whatever fails. The errors come back in the order the systems ran.
    pub fn run(
        &mut self,
        state: &mut S,
        seed: u64,
        time: GameTime,
        date: Date,
    ) -> Vec<SystemError> {
        let mut errors = Vec::new();
        for order in 0..4 {
            for registered in self.systems.iter_mut().filter(|registered| {
                registered.cadence.order() == order && registered.cadence.is_due(date)
            }) {
                for stream in &mut registered.streams {
                    stream.start_tick(seed, time);
                }
                let mut tick = Tick {
                    time,
                    date,
                    streams: &mut registered.streams,
                };
                if let Err(error) = (registered.run)(state, &mut tick) {
                    errors.push(SystemError {
                        system: registered.name.clone(),
                        date,
                        error,
                    });
                }
            }
        }
        errors
    }
}
