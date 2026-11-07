//! Speeds and the real-time clock: how many ticks are due, never what they do.

use std::{error::Error, fmt, num::NonZeroU32, time::Duration};

const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// Game hours per real second at speeds 1 to 5.
const HOURS_PER_SECOND: [u32; 5] = [1, 6, 24, 72, 168];

/// The most ticks one wake returns unless the clock is built with another limit.
pub const DEFAULT_MAX_TICKS_PER_WAKE: NonZeroU32 = match NonZeroU32::new(64) {
    Some(limit) => limit,
    None => panic!("64 is not zero"),
};

/// A simulation speed, 1 to 5: 1, 6, 24, 72 and 168 game hours per real second.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Speed(u8);

/// A speed level outside 1 to 5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpeedError {
    /// The refused level.
    pub level: u8,
}

impl fmt::Display for SpeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "speed {} is outside 1 to {}",
            self.level,
            HOURS_PER_SECOND.len()
        )
    }
}

impl Error for SpeedError {}

impl Speed {
    /// Speed 1, the slowest, and the speed a new game starts at.
    pub const SLOWEST: Self = Self(1);

    /// Speed `level`, 1 to 5.
    ///
    /// # Errors
    /// Returns [`SpeedError`] for any other level.
    pub fn new(level: u8) -> Result<Self, SpeedError> {
        if (1..=5).contains(&level) {
            Ok(Self(level))
        } else {
            Err(SpeedError { level })
        }
    }

    /// The level, 1 to 5.
    #[must_use]
    pub const fn level(self) -> u8 {
        self.0
    }

    /// The game hours that pass in one real second at this speed.
    #[must_use]
    pub fn hours_per_second(self) -> u32 {
        HOURS_PER_SECOND
            .get(usize::from(self.0).saturating_sub(1))
            .copied()
            .unwrap_or(1)
    }
}

/// Counts the ticks due from a monotonic time the caller supplies.
///
/// It never reads the system clock, so a test drives it with a fake one. It starts paused at speed 1.
#[derive(Clone, Debug)]
pub struct Clock {
    speed: Speed,
    paused: bool,
    max_ticks_per_wake: NonZeroU32,
    /// The time of the previous [`Clock::due`] (or of the resume); `None` while paused since construction.
    reference: Option<Duration>,
    /// Elapsed nanoseconds times hours per second: a tick is due for each `NANOS_PER_SECOND` in it.
    accumulator: u128,
}

impl Default for Clock {
    fn default() -> Self {
        Self::with_max_ticks_per_wake(DEFAULT_MAX_TICKS_PER_WAKE)
    }
}

impl Clock {
    /// A paused clock at speed 1 that returns at most [`DEFAULT_MAX_TICKS_PER_WAKE`] ticks per wake.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A paused clock at speed 1 that returns at most `max_ticks_per_wake` ticks per wake.
    #[must_use]
    pub const fn with_max_ticks_per_wake(max_ticks_per_wake: NonZeroU32) -> Self {
        Self {
            speed: Speed::SLOWEST,
            paused: true,
            max_ticks_per_wake,
            reference: None,
            accumulator: 0,
        }
    }

    /// The speed.
    #[must_use]
    pub const fn speed(&self) -> Speed {
        self.speed
    }

    /// Whether the clock is paused.
    #[must_use]
    pub const fn is_paused(&self) -> bool {
        self.paused
    }

    /// Sets the speed and clears the remainder of the previous one.
    pub const fn set_speed(&mut self, speed: Speed) {
        self.speed = speed;
        self.accumulator = 0;
    }

    /// Pauses: no time accrues until [`Clock::resume`].
    pub const fn pause(&mut self) {
        self.paused = true;
    }

    /// Resumes at `now`. The time spent paused is not caught up.
    pub const fn resume(&mut self, now: Duration) {
        self.paused = false;
        self.reference = Some(now);
    }

    /// The whole ticks due at `now`, a monotonic time since any fixed start.
    ///
    /// The real time since the previous call is added at the current speed and the fraction of a tick left over is kept
    /// for the next call. A paused clock returns 0. Never more than the per-wake limit comes back: when more were due,
    /// the excess is dropped from the real-time schedule and the remainder is cleared, so after a stall the game runs
    /// slower than real time and no tick is skipped, merged or shortened.
    pub fn due(&mut self, now: Duration) -> u32 {
        let Some(reference) = self.reference.filter(|_| !self.paused) else {
            return 0;
        };
        self.reference = Some(now);
        let elapsed = now.saturating_sub(reference).as_nanos();
        self.accumulator += elapsed * u128::from(self.speed.hours_per_second());
        let ticks = self.accumulator / NANOS_PER_SECOND;
        if ticks > u128::from(self.max_ticks_per_wake.get()) {
            self.accumulator = 0;
            return self.max_ticks_per_wake.get();
        }
        self.accumulator %= NANOS_PER_SECOND;
        u32::try_from(ticks).unwrap_or(self.max_ticks_per_wake.get())
    }
}
