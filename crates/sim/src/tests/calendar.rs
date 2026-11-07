use crate::{CalendarError, Date, GameTime, LAST_YEAR, days_in_month, is_leap_year};

fn date(year: u16, month: u8, day: u8, hour: u8) -> Date {
    Date {
        year,
        month,
        day,
        hour,
    }
}

fn time_of(year: u16, month: u8, day: u8, hour: u8) -> GameTime {
    GameTime::from_date(date(year, month, day, hour)).unwrap()
}

/// The Julian Day Number of a Julian-calendar date (the standard integer formula), the independent reference.
fn julian_day_number(year: i64, month: i64, day: i64) -> i64 {
    let a = (14 - month) / 12;
    let y = year + 4800 - a;
    let m = month + 12 * a - 3;
    day + (153 * m + 2) / 5 + 365 * y + y / 4 - 32083
}

/// The day count from 0001-01-01, by the reference formula.
fn reference_day(year: u16, month: u8, day: u8) -> u64 {
    let days = julian_day_number(i64::from(year), i64::from(month), i64::from(day))
        - julian_day_number(1, 1, 1);
    u64::try_from(days).unwrap()
}

#[test]
fn the_vectors_match_the_stated_day_counts() {
    for (year, month, day, expected_day) in [
        (768, 10, 9, 280_428),
        (768, 2, 29, 280_205),
        (769, 3, 1, 280_571),
        (800, 12, 25, 292_193),
        (1000, 1, 1, 364_884),
    ] {
        let time = time_of(year, month, day, 0);
        assert_eq!(time.hours(), expected_day * 24, "{year}-{month}-{day}");
        assert_eq!(
            reference_day(year, month, day),
            expected_day,
            "{year}-{month}-{day} reference"
        );
    }
    assert_eq!(time_of(768, 10, 9, 0).hours(), 6_730_272);
}

#[test]
fn the_epoch_is_the_first_hour_of_year_one() {
    assert_eq!(GameTime::from_hours(0).date(), Ok(date(1, 1, 1, 0)));
    assert_eq!(time_of(1, 1, 1, 0).hours(), 0);
    assert_eq!(GameTime::from_hours(23).date(), Ok(date(1, 1, 1, 23)));
    assert_eq!(GameTime::from_hours(24).date(), Ok(date(1, 1, 2, 0)));
}

#[test]
fn every_month_start_and_end_of_every_year_agrees_with_the_day_number_formula() {
    // Negative control: a calendar that treats 1000 as common (Gregorian rules) or drops a leap day moves a vector off.
    for year in 1..=LAST_YEAR {
        for month in 1..=12 {
            let length = days_in_month(year, month).unwrap();
            for day in [1, length] {
                let time = time_of(year, month, day, 0);
                assert_eq!(
                    time.hours(),
                    reference_day(year, month, day) * 24,
                    "{year}-{month}-{day}"
                );
                assert_eq!(time.date(), Ok(date(year, month, day, 0)));
            }
        }
    }
}

#[test]
fn a_round_trip_holds_for_every_hour_across_767_to_770() {
    let first = time_of(767, 1, 1, 0).hours();
    let end = time_of(771, 1, 1, 0).hours();
    for hours in first..end {
        let time = GameTime::from_hours(hours);
        assert_eq!(
            GameTime::from_date(time.date().unwrap()),
            Ok(time),
            "hour {hours}"
        );
    }
}

#[test]
fn a_round_trip_holds_for_sampled_hours_up_to_9999() {
    let end = time_of(LAST_YEAR, 12, 31, 23).hours() + 1;
    // A prime stride visits every hour of the day, day of the month and month over the whole range.
    for hours in (0..end).step_by(9973).chain([end - 1]) {
        let time = GameTime::from_hours(hours);
        assert_eq!(
            GameTime::from_date(time.date().unwrap()),
            Ok(time),
            "hour {hours}"
        );
    }
}

#[test]
fn the_last_hour_is_9999_december_31_23_and_the_next_has_no_date() {
    let last = time_of(LAST_YEAR, 12, 31, 23);
    assert_eq!(last.date(), Ok(date(LAST_YEAR, 12, 31, 23)));
    assert_eq!(
        last.next().date(),
        Err(CalendarError::TimeOutOfRange {
            hours: last.hours() + 1
        })
    );
}

#[test]
fn dates_outside_the_calendar_or_that_do_not_exist_are_refused_by_name() {
    for (candidate, expected) in [
        (date(0, 1, 1, 0), CalendarError::YearOutOfRange { year: 0 }),
        (
            date(10_000, 1, 1, 0),
            CalendarError::YearOutOfRange { year: 10_000 },
        ),
        (
            date(769, 2, 29, 0),
            CalendarError::DayOutOfRange {
                year: 769,
                month: 2,
                day: 29,
            },
        ),
        (
            date(768, 2, 30, 0),
            CalendarError::DayOutOfRange {
                year: 768,
                month: 2,
                day: 30,
            },
        ),
        (
            date(768, 4, 31, 0),
            CalendarError::DayOutOfRange {
                year: 768,
                month: 4,
                day: 31,
            },
        ),
        (
            date(768, 4, 0, 0),
            CalendarError::DayOutOfRange {
                year: 768,
                month: 4,
                day: 0,
            },
        ),
        (
            date(768, 13, 1, 0),
            CalendarError::MonthOutOfRange { month: 13 },
        ),
        (
            date(768, 0, 1, 0),
            CalendarError::MonthOutOfRange { month: 0 },
        ),
        (
            date(768, 1, 1, 24),
            CalendarError::HourOutOfRange { hour: 24 },
        ),
    ] {
        assert_eq!(GameTime::from_date(candidate), Err(expected), "{candidate}");
    }
}

#[test]
fn every_month_has_its_julian_length_in_a_leap_and_a_common_year() {
    const COMMON: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for (year, february) in [(768, 29), (769, 28), (1000, 29), (1900, 29), (2023, 28)] {
        for (index, common) in COMMON.iter().enumerate() {
            let month = u8::try_from(index + 1).unwrap();
            let expected = if month == 2 { february } else { *common };
            assert_eq!(days_in_month(year, month), Ok(expected), "{year}-{month}");
        }
    }
    assert!(is_leap_year(768) && is_leap_year(1000) && is_leap_year(1900));
    assert!(!is_leap_year(769) && !is_leap_year(766) && !is_leap_year(2023));
}

#[test]
fn a_date_prints_with_zero_padding() {
    assert_eq!(date(768, 10, 9, 5).to_string(), "0768-10-09 05:00");
}
