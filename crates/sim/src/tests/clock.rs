use std::time::Duration;

use crate::{Clock, DEFAULT_MAX_TICKS_PER_WAKE, Speed, SpeedError};

fn speed(level: u8) -> Speed {
    Speed::new(level).unwrap()
}

/// A clock that is running at `level` from time zero.
fn running(level: u8) -> Clock {
    let mut clock = Clock::new();
    clock.set_speed(speed(level));
    clock.resume(Duration::ZERO);
    clock
}

#[test]
fn the_speed_table_is_1_6_24_72_and_168_hours_per_second() {
    let rates: Vec<u32> = (1..=5)
        .map(|level| speed(level).hours_per_second())
        .collect();
    assert_eq!(rates, [1, 6, 24, 72, 168]);
}

#[test]
fn speeds_outside_1_to_5_are_refused() {
    assert_eq!(Speed::new(0), Err(SpeedError { level: 0 }));
    assert_eq!(Speed::new(6), Err(SpeedError { level: 6 }));
    assert_eq!(speed(3).level(), 3);
}

#[test]
fn a_new_clock_is_paused_at_speed_1_and_returns_nothing() {
    let mut clock = Clock::new();
    assert!(clock.is_paused());
    assert_eq!(clock.speed(), Speed::SLOWEST);
    assert_eq!(clock.due(Duration::from_secs(10)), 0);
}

#[test]
fn each_speed_gives_exactly_the_tables_rate_over_ten_seconds() {
    for (level, per_second) in [(1, 1), (2, 6), (3, 24), (4, 72), (5, 168)] {
        let mut clock = running(level);
        // One call per second keeps each wake under the per-wake limit at every speed but the last; the sum is what counts.
        let mut total = 0;
        for second in 1..=10u64 {
            // Split each second in 100 so speed 5 stays under the limit as well (1.68 ticks per call).
            for part in 1..=100u64 {
                total += clock.due(Duration::from_millis((second - 1) * 1000 + part * 10));
            }
        }
        assert_eq!(total, per_second * 10, "speed {level}");
    }
}

#[test]
fn the_remainder_carries_over_so_a_slow_frame_rate_does_not_drift() {
    // Speed 2 is 6 hours per second; a 50 ms call is 0.3 of a tick. Over 1000 calls (50 s) exactly 300 ticks are due,
    // and after every call the total is the floor of the exact figure.
    let mut clock = running(2);
    let mut total = 0u64;
    for call in 1..=1000u64 {
        total += u64::from(clock.due(Duration::from_millis(call * 50)));
        assert_eq!(total, call * 3 / 10, "call {call}");
    }
    assert_eq!(total, 300);
}

#[test]
fn a_pause_accrues_nothing_and_resuming_does_not_catch_up() {
    let mut clock = running(3);
    assert_eq!(clock.due(Duration::from_secs(1)), 24);
    clock.pause();
    assert!(clock.is_paused());
    assert_eq!(clock.due(Duration::from_secs(2)), 0);
    assert_eq!(clock.due(Duration::from_secs(60)), 0);
    // Resumed after 60 s paused: the next second is one second of ticks, not 59.
    clock.resume(Duration::from_secs(60));
    assert_eq!(clock.due(Duration::from_secs(61)), 24);
}

#[test]
fn resuming_without_any_call_while_paused_still_does_not_catch_up() {
    let mut clock = running(3);
    clock.pause();
    clock.resume(Duration::from_secs(500));
    assert_eq!(clock.due(Duration::from_secs(501)), 24);
}

#[test]
fn a_speed_change_clears_the_remainder() {
    // Negative control: keeping the 0.3 tick left at speed 2 would make the 30 ms below 0.3 + 0.72 = 1.02 ticks.
    let mut clock = running(2);
    assert_eq!(clock.due(Duration::from_millis(50)), 0); // 0.3 of a tick left over at 6 hours per second
    clock.set_speed(speed(3));
    // 30 ms at 24 hours per second is 0.72 of a tick, and the old 0.3 is gone.
    assert_eq!(clock.due(Duration::from_millis(80)), 0);
    // Another 0.72 makes 1.44 in all: one tick, with 0.44 left.
    assert_eq!(clock.due(Duration::from_millis(110)), 1);
}

#[test]
fn a_long_stall_returns_the_per_wake_limit_then_normal_rates_without_a_burst() {
    let limit = DEFAULT_MAX_TICKS_PER_WAKE.get();
    let mut clock = running(5);
    // A 30 s stall at 168 hours per second is 5040 ticks due; only the limit comes back.
    assert_eq!(clock.due(Duration::from_secs(30)), limit);
    // The excess is gone: the next 1/60 s frames give the normal 2.8 ticks per frame, never the dropped backlog.
    let mut total = 0;
    for frame in 1..=60u64 {
        let due =
            clock.due(Duration::from_secs(30) + Duration::from_nanos(frame * 1_000_000_000 / 60));
        assert!(due <= 3, "frame {frame}: {due}");
        total += due;
    }
    assert_eq!(total, 168);
}

#[test]
fn the_limit_is_a_construction_parameter() {
    let limit = std::num::NonZeroU32::new(5).unwrap();
    let mut clock = Clock::with_max_ticks_per_wake(limit);
    clock.set_speed(speed(5));
    clock.resume(Duration::ZERO);
    assert_eq!(clock.due(Duration::from_secs(1)), 5);
    assert_eq!(
        clock.due(Duration::from_secs(1) + Duration::from_millis(10)),
        1
    );
}

#[test]
fn a_call_with_no_time_elapsed_returns_nothing() {
    let mut clock = running(5);
    assert_eq!(clock.due(Duration::ZERO), 0);
    assert_eq!(clock.due(Duration::ZERO), 0);
}

#[test]
fn a_time_that_runs_backwards_adds_nothing() {
    let mut clock = running(5);
    assert_eq!(clock.due(Duration::from_millis(100)), 16);
    assert_eq!(clock.due(Duration::from_millis(50)), 0);
}
