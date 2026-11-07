use crate::{
    Cadence, GameTime, RandomError, Scheduler, SchedulerError, Simulation, Stream,
    random::{fnv1a, mix},
    stream_id,
};

/// 0768-10-09 00:00.
const TICK: u64 = 6_730_272;

fn tick() -> GameTime {
    GameTime::from_hours(TICK)
}

/// The stream the reached vectors are taken from: seed 42, `test.weather`, tick 6,730,272, before its first draw.
fn weather() -> Stream {
    Stream::new(42, "test.weather", tick())
}

const FIRST: u64 = 0x7707_56d3_0195_b849;
const SECOND: u64 = 0x9fe9_7809_7274_c727;

#[test]
fn the_mixer_and_the_hash_match_their_frozen_vectors() {
    assert_eq!(mix(0), 0xe220_a839_7b1d_cdaf);
    assert_eq!(mix(1), 0x910a_2dec_8902_5cc1);
    assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(stream_id("test.weather"), 0x6b9b_eeaa_794c_7487);
    // The draw of all zeros is the nested mix with every input zero.
    assert_eq!(mix(mix(mix(mix(0)))), 0x2130_748a_aac8_0268);
}

#[test]
fn draws_match_their_frozen_vectors_and_the_counter_moves_with_each() {
    let mut stream = weather();
    assert_eq!(stream.counter(), 0);
    assert_eq!(stream.draw(), FIRST);
    assert_eq!(stream.draw(), SECOND);
    assert_eq!(stream.counter(), 2);
    assert_eq!(
        Stream::new(42, "test.weather", GameTime::from_hours(TICK + 1)).draw(),
        0x19f8_15f3_d762_6b00
    );
}

#[test]
fn the_counter_starts_again_on_a_new_tick() {
    let mut stream = weather();
    stream.draw();
    stream.draw();
    stream.start_tick(42, GameTime::from_hours(TICK + 1));
    assert_eq!(stream.counter(), 0);
    assert_eq!(stream.draw(), 0x19f8_15f3_d762_6b00);
}

#[test]
fn below_one_and_below_ten_accept_the_first_draw() {
    let mut stream = weather();
    assert_eq!(stream.below(1), Ok(0));
    assert_eq!(stream.counter(), 1);

    // t = 2^64 mod 10 = 6, and the raw draw is above it: FIRST mod 10 = 5.
    let mut stream = weather();
    assert_eq!(stream.below(10), Ok(5));
    assert_eq!(stream.counter(), 1);
}

#[test]
fn a_draw_below_the_threshold_is_rejected_and_the_next_is_used() {
    // n = 2^63 + 1 gives t = 0x7fffffffffffffff; the first draw is below it and so is rejected.
    let n = (1_u64 << 63) + 1;
    assert_eq!(n.wrapping_neg() % n, 0x7fff_ffff_ffff_ffff);
    const { assert!(FIRST < 0x7fff_ffff_ffff_ffff && SECOND >= 0x7fff_ffff_ffff_ffff) };

    let mut stream = weather();
    assert_eq!(stream.below(n), Ok(0x1fe9_7809_7274_c726));
    assert_eq!(stream.counter(), 2);
}

#[test]
fn a_draw_equal_to_the_threshold_is_accepted() {
    // Only draws strictly below t are rejected. For n = 2^64 - FIRST, t = 2^64 mod n = FIRST, and the first draw is FIRST.
    let n = FIRST.wrapping_neg();
    assert_eq!(n.wrapping_neg() % n, FIRST);
    let mut stream = weather();
    assert_eq!(stream.below(n), Ok(FIRST % n));
    assert_eq!(stream.counter(), 1);
}

#[test]
fn nothing_is_rejected_for_one_and_for_powers_of_two() {
    for shift in 0..64 {
        let n = 1_u64 << shift;
        assert_eq!(n.wrapping_neg() % n, 0, "2^{shift}");
        let mut stream = weather();
        assert_eq!(stream.below(n), Ok(FIRST & (n - 1)));
        assert_eq!(stream.counter(), 1);
    }
}

#[test]
fn an_empty_range_is_refused_and_draws_nothing() {
    let mut stream = weather();
    assert_eq!(stream.below(0), Err(RandomError::EmptyRange));
    assert_eq!(stream.range(5, 4), Err(RandomError::EmptyRange));
    assert_eq!(stream.range_i64(3, -3), Err(RandomError::EmptyRange));
    assert_eq!(stream.counter(), 0);
    // The next draw is still the stream's first.
    assert_eq!(stream.draw(), FIRST);
}

#[test]
fn range_covers_its_bounds_and_the_full_width() {
    let mut stream = weather();
    assert_eq!(stream.range(7, 7), Ok(7));
    assert_eq!(stream.counter(), 1);

    // range(m, n) is m + below(n - m + 1).
    let mut stream = weather();
    assert_eq!(stream.range(100, 109), Ok(105));

    let mut stream = weather();
    assert_eq!(stream.range(0, u64::MAX), Ok(FIRST));
    assert_eq!(stream.counter(), 1);
    assert_eq!(weather().range(u64::MAX, u64::MAX), Ok(u64::MAX));
    // A span of 2^64 - 1 values is not the full width, so it is a below.
    assert_eq!(weather().range(1, u64::MAX), Ok(1 + FIRST % u64::MAX));
}

#[test]
fn range_i64_is_the_offset_added_to_the_lower_bound() {
    // The full span is the raw draw with its top bit flipped.
    assert_eq!(
        weather().range_i64(i64::MIN, i64::MAX),
        Ok(-646_452_557_239_699_383)
    );
    assert_eq!(
        0xf707_56d3_0195_b849_u64.cast_signed(),
        -646_452_557_239_699_383
    );
    // below(7) at that draw is 3, so the result is -3 + 3.
    let mut stream = weather();
    assert_eq!(stream.range_i64(-3, 3), Ok(0));
    assert_eq!(stream.counter(), 1);
    assert_eq!(weather().range_i64(5, 5), Ok(5));
    assert_eq!(weather().range_i64(i64::MIN, i64::MIN), Ok(i64::MIN));
}

#[test]
fn every_result_stays_in_bounds_over_many_draws() {
    let mut stream = weather();
    for _ in 0..2000 {
        assert!((-3..=3).contains(&stream.range_i64(-3, 3).unwrap()));
        assert!((10..=19).contains(&stream.range(10, 19).unwrap()));
        assert!(stream.below(6).unwrap() < 6);
    }
}

#[test]
fn a_stream_is_unchanged_by_what_another_stream_draws() {
    let alone: Vec<u64> = {
        let mut stream = Stream::new(42, "test.weather", tick());
        (0..20).map(|_| stream.draw()).collect()
    };
    let mut weather = Stream::new(42, "test.weather", tick());
    let mut other = Stream::new(42, "test.other", tick());
    let mut interleaved = Vec::new();
    for round in 0..20 {
        for _ in 0..(round % 5) {
            other.draw();
        }
        interleaved.push(weather.draw());
    }
    assert_eq!(alone, interleaved);
    // And the two streams are different streams.
    assert_ne!(
        Stream::new(42, "test.other", tick()).draw(),
        Stream::new(42, "test.weather", tick()).draw()
    );
}

#[test]
fn a_system_draws_only_from_its_own_streams() {
    let mut scheduler = Scheduler::<Vec<Result<u64, RandomError>>>::new();
    scheduler
        .register_with_streams("a", Cadence::Hourly, &["test.a"], |state, tick| {
            state.push(tick.stream("test.a").map(Stream::draw));
            state.push(tick.stream("test.b").map(Stream::draw));
            Ok(())
        })
        .unwrap();
    scheduler
        .register_with_streams("b", Cadence::Hourly, &["test.b"], |state, tick| {
            state.push(tick.stream("test.b").map(Stream::draw));
            Ok(())
        })
        .unwrap();
    let mut simulation = Simulation::<_, ()>::new(tick(), 42, Vec::new(), scheduler, |_, ()| {});
    simulation.step().unwrap();
    assert_eq!(
        *simulation.state(),
        [
            Ok(Stream::new(42, "test.a", tick()).draw()),
            Err(RandomError::UnknownStream {
                name: "test.b".to_owned()
            }),
            Ok(Stream::new(42, "test.b", tick()).draw()),
        ]
    );
}

#[test]
fn each_tick_starts_each_stream_at_counter_zero() {
    let mut scheduler = Scheduler::<Vec<u64>>::new();
    scheduler
        .register_with_streams("a", Cadence::Hourly, &["test.a"], |state, tick| {
            let stream = tick.stream("test.a")?;
            state.push(stream.draw());
            state.push(stream.draw());
            Ok(())
        })
        .unwrap();
    let mut simulation = Simulation::<_, ()>::new(tick(), 42, Vec::new(), scheduler, |_, ()| {});
    simulation.step().unwrap();
    simulation.step().unwrap();
    let expected: Vec<u64> = [TICK, TICK + 1]
        .into_iter()
        .flat_map(|hour| {
            let mut stream = Stream::new(42, "test.a", GameTime::from_hours(hour));
            [stream.draw(), stream.draw()]
        })
        .collect();
    assert_eq!(*simulation.state(), expected);
}

#[test]
fn a_duplicate_stream_name_is_refused_and_adds_nothing() {
    let mut scheduler = Scheduler::<u32>::new();
    scheduler
        .register_with_streams("a", Cadence::Hourly, &["test.shared"], |count, _| {
            *count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        scheduler.register_with_streams("b", Cadence::Hourly, &["test.shared"], |count, _| {
            *count += 100;
            Ok(())
        }),
        Err(SchedulerError::DuplicateStream {
            name: "test.shared".to_owned()
        })
    );
    // Listing a name twice for one system is a duplicate as well.
    assert_eq!(
        scheduler.register_with_streams("c", Cadence::Hourly, &["test.x", "test.x"], |_, _| Ok(())),
        Err(SchedulerError::DuplicateStream {
            name: "test.x".to_owned()
        })
    );
    // Neither refused system was added, and "b" is still a free name.
    scheduler
        .register_with_streams("b", Cadence::Hourly, &["test.y"], |count, _| {
            *count += 10;
            Ok(())
        })
        .unwrap();
    let mut simulation = Simulation::<_, ()>::new(tick(), 0, 0, scheduler, |_, ()| {});
    simulation.step().unwrap();
    assert_eq!(*simulation.state(), 11);
}
