//! Calendar arithmetic of the generator (CK-369): whole days, years and months back from a date, and the day a character turns an age.
#![deny(clippy::float_arithmetic)]

use crate::Error;
use ck_sim::{Date, GameTime, days_in_month, is_leap_year};

const HOURS_PER_DAY: u64 = 24;

/// The date of a time.
pub(crate) fn date_of(time: GameTime) -> Result<Date, Error> {
    time.date().map_err(|_| Error::Overflow)
}

/// The start of a day.
pub(crate) fn day_start(year: u16, month: u8, day: u8) -> Result<GameTime, Error> {
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour: 0,
    })
    .map_err(|_| Error::Overflow)
}

/// The start of the day `time` is in.
pub(crate) fn start_of(time: GameTime) -> GameTime {
    GameTime::from_hours(time.hours() / HOURS_PER_DAY * HOURS_PER_DAY)
}

/// The whole days from `from` to `to` (negative when `to` is earlier), counted between the starts of their days.
pub(crate) fn days_between(from: GameTime, to: GameTime) -> i64 {
    let day = |time: GameTime| i64::try_from(time.hours() / HOURS_PER_DAY).unwrap_or(i64::MAX);
    day(to) - day(from)
}

/// The start of the day `days` after (or, when negative, before) the day of `time`.
pub(crate) fn add_days(time: GameTime, days: i64) -> Result<GameTime, Error> {
    let day = i64::try_from(time.hours() / HOURS_PER_DAY).map_err(|_| Error::Overflow)?;
    let moved = u64::try_from(day.checked_add(days).ok_or(Error::Overflow)?)
        .map_err(|_| Error::Overflow)?;
    let moved = GameTime::from_hours(moved.checked_mul(HOURS_PER_DAY).ok_or(Error::Overflow)?);
    // Refuse a day outside the calendar.
    date_of(moved)?;
    Ok(moved)
}

/// The same day of the year `years` earlier; 29 February in a year that has none is 28 February.
pub(crate) fn years_before(time: GameTime, years: u32) -> Result<GameTime, Error> {
    let date = date_of(time)?;
    let year = u32::from(date.year)
        .checked_sub(years)
        .and_then(|year| u16::try_from(year).ok())
        .filter(|year| *year >= ck_sim::FIRST_YEAR)
        .ok_or(Error::Overflow)?;
    let day = if date.month == 2 && date.day == 29 && !is_leap_year(year) {
        28
    } else {
        date.day
    };
    day_start(year, date.month, day)
}

/// The same day of the month `months` earlier, held to the length of that month.
pub(crate) fn months_before(time: GameTime, months: u32) -> Result<GameTime, Error> {
    let date = date_of(time)?;
    let index = u32::from(date.year) * 12 + u32::from(date.month) - 1;
    let index = index.checked_sub(months).ok_or(Error::Overflow)?;
    let year = u16::try_from(index / 12).map_err(|_| Error::Overflow)?;
    let month = u8::try_from(index % 12 + 1).map_err(|_| Error::Overflow)?;
    let longest = days_in_month(year, month).map_err(|_| Error::Overflow)?;
    day_start(year, month, date.day.min(longest))
}

/// The day a character born on `birth` turns `age`: the same day of the year, except that a 29 February birthday falls on 1 March in a
/// common year.
pub(crate) fn birthday(birth: GameTime, age: u32) -> Result<GameTime, Error> {
    let date = date_of(birth)?;
    let year = u32::from(date.year)
        .checked_add(age)
        .and_then(|year| u16::try_from(year).ok())
        .ok_or(Error::Overflow)?;
    if date.month == 2 && date.day == 29 && !is_leap_year(year) {
        return day_start(year, 3, 1);
    }
    day_start(year, date.month, date.day)
}

/// The first day of a year.
pub(crate) fn new_year(year: u16) -> Result<GameTime, Error> {
    day_start(year, 1, 1)
}

/// The last day of a year.
pub(crate) fn year_end(year: u16) -> Result<GameTime, Error> {
    day_start(year, 12, 31)
}
