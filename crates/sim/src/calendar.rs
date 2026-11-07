//! Game time and the Julian calendar, in exact integer arithmetic.

use std::{error::Error, fmt};

/// The first year the calendar represents.
pub const FIRST_YEAR: u16 = 1;

/// The last year the calendar represents.
pub const LAST_YEAR: u16 = 9999;

const HOURS_PER_DAY: u64 = 24;
const DAYS_PER_COMMON_YEAR: u64 = 365;
/// Three common years and one leap year: the Julian leap year is every fourth year, so the cycle repeats exactly.
const DAYS_PER_CYCLE: u64 = 4 * DAYS_PER_COMMON_YEAR + 1;
const COMMON_MONTH_LENGTHS: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// A count of hours since 0001-01-01 00:00 in the Julian calendar; tick `n` is the hour starting at `GameTime(n)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GameTime(u64);

/// A calendar date and hour. Years run from 1 to 9999, months from 1 to 12 and hours from 0 to 23.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    /// The year, 1 to 9999.
    pub year: u16,
    /// The month, 1 to 12.
    pub month: u8,
    /// The day of the month, from 1 to the month's length.
    pub day: u8,
    /// The hour of the day, 0 to 23.
    pub hour: u8,
}

/// Why a date or a time cannot be converted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarError {
    /// The year is outside 1 to 9999.
    YearOutOfRange {
        /// The refused year.
        year: u16,
    },
    /// The month is outside 1 to 12.
    MonthOutOfRange {
        /// The refused month.
        month: u8,
    },
    /// The day does not exist in that month of that year, for example 769-02-29.
    DayOutOfRange {
        /// The year of the refused date.
        year: u16,
        /// The month of the refused date.
        month: u8,
        /// The refused day.
        day: u8,
    },
    /// The hour is outside 0 to 23.
    HourOutOfRange {
        /// The refused hour.
        hour: u8,
    },
    /// The hour count lies after the last representable hour, 9999-12-31 23:00.
    TimeOutOfRange {
        /// The refused count of hours.
        hours: u64,
    },
}

impl fmt::Display for CalendarError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::YearOutOfRange { year } => {
                write!(f, "year {year} is outside {FIRST_YEAR} to {LAST_YEAR}")
            }
            Self::MonthOutOfRange { month } => write!(f, "month {month} is outside 1 to 12"),
            Self::DayOutOfRange { year, month, day } => {
                write!(f, "day {day} does not exist in {year:04}-{month:02}")
            }
            Self::HourOutOfRange { hour } => write!(f, "hour {hour} is outside 0 to 23"),
            Self::TimeOutOfRange { hours } => {
                write!(
                    f,
                    "{hours} hours is after the last calendar hour, 9999-12-31 23:00"
                )
            }
        }
    }
}

impl Error for CalendarError {}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:00",
            self.year, self.month, self.day, self.hour
        )
    }
}

/// Whether `year` is a Julian leap year: every year divisible by 4.
#[must_use]
pub const fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4)
}

/// The number of days in `month` of `year`.
///
/// # Errors
/// Returns [`CalendarError::YearOutOfRange`] or [`CalendarError::MonthOutOfRange`] for a year or month the calendar
/// does not have.
pub fn days_in_month(year: u16, month: u8) -> Result<u8, CalendarError> {
    check_year(year)?;
    let index = usize::from(month)
        .checked_sub(1)
        .ok_or(CalendarError::MonthOutOfRange { month })?;
    let common = *COMMON_MONTH_LENGTHS
        .get(index)
        .ok_or(CalendarError::MonthOutOfRange { month })?;
    Ok(if month == 2 && is_leap_year(year) {
        common + 1
    } else {
        common
    })
}

const fn check_year(year: u16) -> Result<(), CalendarError> {
    if year < FIRST_YEAR || year > LAST_YEAR {
        Err(CalendarError::YearOutOfRange { year })
    } else {
        Ok(())
    }
}

/// The days from 0001-01-01 to the first day of `year`.
fn days_before_year(year: u16) -> u64 {
    let elapsed = u64::from(year) - 1;
    elapsed * DAYS_PER_COMMON_YEAR + elapsed / 4
}

/// The hour just after 9999-12-31 23:00, the first count of hours that has no date.
fn end_of_calendar() -> u64 {
    days_before_year(LAST_YEAR + 1) * HOURS_PER_DAY
}

impl GameTime {
    /// The time `hours` hours after 0001-01-01 00:00.
    #[must_use]
    pub const fn from_hours(hours: u64) -> Self {
        Self(hours)
    }

    /// The hours since 0001-01-01 00:00.
    #[must_use]
    pub const fn hours(self) -> u64 {
        self.0
    }

    /// The time one hour later.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// The time of `date`.
    ///
    /// # Errors
    /// Refuses a date outside the calendar or one that does not exist, such as 769-02-29, with the part that is wrong.
    pub fn from_date(date: Date) -> Result<Self, CalendarError> {
        let Date {
            year,
            month,
            day,
            hour,
        } = date;
        let length = days_in_month(year, month)?;
        if day == 0 || day > length {
            return Err(CalendarError::DayOutOfRange { year, month, day });
        }
        if hour >= 24 {
            return Err(CalendarError::HourOutOfRange { hour });
        }
        let mut days = days_before_year(year);
        for earlier in 1..month {
            days += u64::from(days_in_month(year, earlier)?);
        }
        days += u64::from(day) - 1;
        Ok(Self(days * HOURS_PER_DAY + u64::from(hour)))
    }

    /// The date and hour of this time.
    ///
    /// # Errors
    /// Returns [`CalendarError::TimeOutOfRange`] after 9999-12-31 23:00.
    pub fn date(self) -> Result<Date, CalendarError> {
        let out_of_range = CalendarError::TimeOutOfRange { hours: self.0 };
        if self.0 >= end_of_calendar() {
            return Err(out_of_range);
        }
        let days = self.0 / HOURS_PER_DAY;
        let hour = u8::try_from(self.0 % HOURS_PER_DAY).map_err(|_| out_of_range)?;
        // The leap day is the last day of each four-year cycle, so the fourth year of a cycle is the long one.
        let cycle = days / DAYS_PER_CYCLE;
        let in_cycle = days % DAYS_PER_CYCLE;
        let year_in_cycle = (in_cycle / DAYS_PER_COMMON_YEAR).min(3);
        let mut day_of_year = in_cycle - year_in_cycle * DAYS_PER_COMMON_YEAR;
        let year = u16::try_from(1 + cycle * 4 + year_in_cycle).map_err(|_| out_of_range)?;
        for month in 1..=12 {
            let length = u64::from(days_in_month(year, month).map_err(|_| out_of_range)?);
            if day_of_year < length {
                let day = u8::try_from(day_of_year + 1).map_err(|_| out_of_range)?;
                return Ok(Date {
                    year,
                    month,
                    day,
                    hour,
                });
            }
            day_of_year -= length;
        }
        Err(out_of_range)
    }
}
