use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
};

use crate::{Cadence, Date, GameTime, Save, Scheduler, SchedulerError, Simulation, SystemResult};

type Log = Arc<Mutex<Vec<String>>>;

#[derive(Debug)]
struct Broken;

impl fmt::Display for Broken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("broken")
    }
}

impl Error for Broken {}

fn at(year: u16, month: u8, day: u8, hour: u8) -> GameTime {
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour,
    })
    .unwrap()
}

/// A scheduler with two systems per cadence, registered coarse-last to show the cadence order is not the registration order.
fn scheduler(log: &Log) -> Scheduler<()> {
    let mut scheduler = Scheduler::new();
    for (cadence, label) in [
        (Cadence::Hourly, "hourly"),
        (Cadence::Daily { hour: 0 }, "daily"),
        (Cadence::Monthly { hour: 0 }, "monthly"),
        (Cadence::Yearly { hour: 0 }, "yearly"),
    ] {
        for number in [1, 2] {
            let log = Arc::clone(log);
            scheduler
                .register(
                    format!("{label}-{number}"),
                    cadence,
                    move |(), _| -> SystemResult {
                        log.lock().unwrap().push(format!("{label}-{number}"));
                        Ok(())
                    },
                )
                .unwrap();
        }
    }
    scheduler
}

fn ran_at(time: GameTime) -> Vec<String> {
    let log = Log::default();
    let mut scheduler = scheduler(&log);
    let errors = scheduler.run(&mut (), 0, time, time.date().unwrap());
    assert!(errors.is_empty());
    log.lock().unwrap().clone()
}

#[test]
fn the_new_year_runs_yearly_then_monthly_then_daily_then_hourly_each_in_registration_order() {
    assert_eq!(
        ran_at(at(769, 1, 1, 0)),
        [
            "yearly-1",
            "yearly-2",
            "monthly-1",
            "monthly-2",
            "daily-1",
            "daily-2",
            "hourly-1",
            "hourly-2"
        ]
    );
}

#[test]
fn noon_cadences_run_coarse_first_and_do_not_run_at_midnight() {
    let log = Log::default();
    let mut scheduler = Scheduler::new();
    for (name, cadence) in [
        ("hourly", Cadence::Hourly),
        ("daily", Cadence::Daily { hour: 12 }),
        ("monthly", Cadence::Monthly { hour: 12 }),
        ("yearly", Cadence::Yearly { hour: 12 }),
    ] {
        let log = Arc::clone(&log);
        scheduler
            .register(name, cadence, move |(), _| {
                log.lock().unwrap().push(name.to_owned());
                Ok(())
            })
            .unwrap();
    }
    let noon = at(769, 1, 1, 12);
    assert!(
        scheduler
            .run(&mut (), 0, noon, noon.date().unwrap())
            .is_empty()
    );
    assert_eq!(
        *log.lock().unwrap(),
        ["yearly", "monthly", "daily", "hourly"]
    );

    log.lock().unwrap().clear();
    let midnight = at(769, 1, 1, 0);
    assert!(
        scheduler
            .run(&mut (), 0, midnight, midnight.date().unwrap())
            .is_empty()
    );
    assert_eq!(*log.lock().unwrap(), ["hourly"]);
}

#[test]
fn a_coarse_cadence_refuses_an_hour_outside_the_day() {
    let mut scheduler = Scheduler::<()>::new();
    assert_eq!(
        scheduler.register("bad", Cadence::Daily { hour: 24 }, |(), _| Ok(())),
        Err(SchedulerError::HourOutOfRange { hour: 24 })
    );
}

#[test]
fn a_noon_schedule_saved_at_eleven_or_noon_continues_to_the_same_digest() {
    use super::world::{expected, meta, start};

    fn scheduler() -> Scheduler<u64> {
        let mut scheduler = Scheduler::new();
        scheduler
            .register("economy", Cadence::Daily { hour: 12 }, |balance, _| {
                *balance += 1;
                Ok(())
            })
            .unwrap();
        scheduler
    }

    let finish = GameTime::from_hours(start().hours() + 72);
    for save_hour in [11, 12] {
        let mut original = Simulation::<u64, u64>::new(start(), 42, 0, scheduler(), |_, _| {});
        let save_time = GameTime::from_hours(start().hours() + save_hour);
        while original.time() < save_time {
            original.step().unwrap();
        }
        let bytes = original.save(meta(42)).to_bytes().unwrap();
        let save = Save::from_bytes(&bytes, &expected()).unwrap();
        let mut loaded = Simulation::<u64, u64>::from_save(save, scheduler(), |_, _| {}).unwrap();
        assert_eq!(loaded.digest().unwrap(), original.digest().unwrap());

        while original.time() < finish {
            original.step().unwrap();
        }
        while loaded.time() < finish {
            loaded.step().unwrap();
        }
        assert_eq!(loaded.digest().unwrap(), original.digest().unwrap());
        assert_eq!(*loaded.state(), 3, "one economy tick per noon");
    }
}

#[test]
fn an_ordinary_hour_runs_only_the_hourly_systems() {
    assert_eq!(ran_at(at(768, 10, 9, 5)), ["hourly-1", "hourly-2"]);
}

#[test]
fn midnight_runs_daily_and_hourly() {
    assert_eq!(
        ran_at(at(768, 10, 9, 0)),
        ["daily-1", "daily-2", "hourly-1", "hourly-2"]
    );
}

#[test]
fn the_first_of_a_month_runs_monthly_daily_and_hourly() {
    assert_eq!(
        ran_at(at(768, 11, 1, 0)),
        [
            "monthly-1",
            "monthly-2",
            "daily-1",
            "daily-2",
            "hourly-1",
            "hourly-2"
        ]
    );
}

#[test]
fn the_first_of_a_month_at_another_hour_is_not_a_boundary() {
    assert_eq!(ran_at(at(768, 11, 1, 1)), ["hourly-1", "hourly-2"]);
}

#[test]
fn the_leap_day_is_an_ordinary_day() {
    assert_eq!(
        ran_at(at(768, 2, 29, 0)),
        ["daily-1", "daily-2", "hourly-1", "hourly-2"]
    );
}

#[test]
fn systems_receive_the_state_the_time_and_the_date() {
    let mut scheduler = Scheduler::<Vec<(GameTime, Date)>>::new();
    scheduler
        .register("record", Cadence::Hourly, |state, tick| {
            state.push((tick.time(), tick.date()));
            Ok(())
        })
        .unwrap();
    let time = at(768, 10, 9, 5);
    let date = time.date().unwrap();
    let mut state = Vec::new();
    assert!(scheduler.run(&mut state, 0, time, date).is_empty());
    assert_eq!(state, [(time, date)]);
}

#[test]
fn a_duplicate_name_is_refused_even_on_another_cadence_and_adds_nothing() {
    let mut scheduler = Scheduler::<u32>::new();
    scheduler
        .register("economy", Cadence::Daily { hour: 0 }, |count, _| {
            *count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        scheduler.register("economy", Cadence::Hourly, |count, _| {
            *count += 100;
            Ok(())
        }),
        Err(SchedulerError::DuplicateName {
            name: "economy".to_owned()
        })
    );
    let time = at(768, 10, 9, 0);
    let mut count = 0;
    scheduler.run(&mut count, 0, time, time.date().unwrap());
    assert_eq!(count, 1);
}

#[test]
fn a_failing_system_is_collected_and_its_peers_still_run() {
    let log = Log::default();
    let mut simulation_scheduler = Scheduler::<()>::new();
    for (name, fails) in [("before", false), ("fails", true), ("after", false)] {
        let log = Arc::clone(&log);
        simulation_scheduler
            .register(name, Cadence::Hourly, move |(), _| -> SystemResult {
                log.lock().unwrap().push(name.to_owned());
                if fails { Err(Box::new(Broken)) } else { Ok(()) }
            })
            .unwrap();
    }
    let mut simulation =
        Simulation::<(), ()>::new(at(768, 10, 9, 5), 0, (), simulation_scheduler, |(), ()| {});

    let errors = simulation.step().unwrap();

    assert_eq!(*log.lock().unwrap(), ["before", "fails", "after"]);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].system, "fails");
    assert_eq!(
        errors[0].date,
        Date {
            year: 768,
            month: 10,
            day: 9,
            hour: 5
        }
    );
    assert_eq!(
        errors[0].to_string(),
        "system `fails` failed at 0768-10-09 05:00: broken"
    );
    // The tick completed: time moved on and the next tick runs normally.
    assert_eq!(simulation.time(), at(768, 10, 9, 6));
    assert_eq!(simulation.step().unwrap().len(), 1);
}

#[test]
fn several_failures_in_one_tick_come_back_in_run_order() {
    let mut scheduler = Scheduler::<()>::new();
    for (name, cadence) in [
        ("hour", Cadence::Hourly),
        ("day", Cadence::Daily { hour: 0 }),
        ("year", Cadence::Yearly { hour: 0 }),
    ] {
        scheduler
            .register(name, cadence, |(), _| Err(Box::new(Broken)))
            .unwrap();
    }
    let time = at(769, 1, 1, 0);
    let errors = scheduler.run(&mut (), 0, time, time.date().unwrap());
    assert_eq!(
        errors
            .iter()
            .map(|error| error.system.as_str())
            .collect::<Vec<_>>(),
        ["year", "day", "hour"]
    );
}

#[test]
fn the_calendar_ending_stops_the_step_without_running_anything() {
    let last = GameTime::from_date(Date {
        year: 9999,
        month: 12,
        day: 31,
        hour: 23,
    })
    .unwrap();
    let mut scheduler = Scheduler::<u32>::new();
    scheduler
        .register("count", Cadence::Hourly, |count, _| {
            *count += 1;
            Ok(())
        })
        .unwrap();
    let mut simulation = Simulation::<u32, ()>::new(last, 0, 0, scheduler, |_, ()| {});

    assert!(simulation.step().unwrap().is_empty());
    assert_eq!(*simulation.state(), 1);
    assert!(simulation.step().is_err());
    assert_eq!(*simulation.state(), 1);
    assert_eq!(simulation.time(), last.next());
}
