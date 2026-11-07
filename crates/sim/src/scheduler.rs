//! The tick scheduler: named systems on four fixed cadences, run coarser first.

use std::{error::Error, fmt};

use crate::calendar::{Date, GameTime};

/// How often a system runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cadence {
    /// On 1 January at hour 0.
    Yearly,
    /// On the 1st of each month at hour 0.
    Monthly,
    /// At hour 0 of each day.
    Daily,
    /// On every tick.
    Hourly,
}

impl Cadence {
    /// The order a tick runs the cadences in: coarser first.
    const RUN_ORDER: [Self; 4] = [Self::Yearly, Self::Monthly, Self::Daily, Self::Hourly];

    const fn is_due(self, date: Date) -> bool {
        match self {
            Self::Yearly => date.month == 1 && date.day == 1 && date.hour == 0,
            Self::Monthly => date.day == 1 && date.hour == 0,
            Self::Daily => date.hour == 0,
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
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateName { name } => {
                write!(f, "a system named `{name}` is already registered")
            }
        }
    }
}

impl Error for SchedulerError {}

type System<S> = Box<dyn FnMut(&mut S, GameTime, Date) -> SystemResult + Send>;

struct Registered<S> {
    name: String,
    cadence: Cadence,
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

    /// Registers `system` under `name` on `cadence`. Within a cadence, systems run in registration order.
    ///
    /// # Errors
    /// Returns [`SchedulerError::DuplicateName`] when `name` is already registered; nothing is added.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        cadence: Cadence,
        system: impl FnMut(&mut S, GameTime, Date) -> SystemResult + Send + 'static,
    ) -> Result<(), SchedulerError> {
        let name = name.into();
        if self
            .systems
            .iter()
            .any(|registered| registered.name == name)
        {
            return Err(SchedulerError::DuplicateName { name });
        }
        self.systems.push(Registered {
            name,
            cadence,
            run: Box::new(system),
        });
        Ok(())
    }

    /// Runs the systems due at `date`, coarser cadences first (yearly, monthly, daily, hourly).
    ///
    /// A failing system is recorded with its name and the date, and the systems after it still run, so the result is
    /// the same whatever fails. The errors come back in the order the systems ran.
    pub fn run(&mut self, state: &mut S, time: GameTime, date: Date) -> Vec<SystemError> {
        let mut errors = Vec::new();
        for cadence in Cadence::RUN_ORDER {
            if !cadence.is_due(date) {
                continue;
            }
            for registered in self
                .systems
                .iter_mut()
                .filter(|registered| registered.cadence == cadence)
            {
                if let Err(error) = (registered.run)(state, time, date) {
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
