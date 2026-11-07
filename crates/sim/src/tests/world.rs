//! A small world for the reproducibility and save tests: an hourly, a daily and a monthly system, each drawing from its own
//! stream and folding the result into the state, and two kinds of command.

use std::collections::BTreeMap;

use crate::{
    Cadence, Canonical, CanonicalError, Date, GameTime, Reader, SaveMeta, Scheduler, Simulation,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct World {
    pub(super) hourly: u64,
    pub(super) daily: u64,
    pub(super) monthly: u64,
    pub(super) noise: u64,
    pub(super) orders: BTreeMap<String, u64>,
    pub(super) notes: Vec<String>,
}

impl Canonical for World {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.hourly.encode(out)?;
        self.daily.encode(out)?;
        self.monthly.encode(out)?;
        self.noise.encode(out)?;
        self.orders.encode(out)?;
        self.notes.encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            hourly: u64::decode(input)?,
            daily: u64::decode(input)?,
            monthly: u64::decode(input)?,
            noise: u64::decode(input)?,
            orders: BTreeMap::decode(input)?,
            notes: Vec::decode(input)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Order {
    Set(String, u64),
    Bump(u64),
}

impl Canonical for Order {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        match self {
            Self::Set(key, value) => {
                0_u8.encode(out)?;
                key.encode(out)?;
                value.encode(out)
            }
            Self::Bump(amount) => {
                1_u8.encode(out)?;
                amount.encode(out)
            }
        }
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        match u8::decode(input)? {
            0 => Ok(Self::Set(String::decode(input)?, u64::decode(input)?)),
            1 => Ok(Self::Bump(u64::decode(input)?)),
            tag => Err(CanonicalError::BadTag { ty: "Order", tag }),
        }
    }
}

pub(super) fn start_date() -> Date {
    Date {
        year: 768,
        month: 1,
        day: 1,
        hour: 0,
    }
}

pub(super) fn start() -> GameTime {
    GameTime::from_date(start_date()).unwrap()
}

/// The tick `days` days and `hours` hours after the start.
pub(super) fn after(days: u64, hours: u64) -> GameTime {
    GameTime::from_hours(start().hours() + days * 24 + hours)
}

/// The world's systems. `noise` adds another hourly system on its own stream, registered first, that draws that many times.
pub(super) fn scheduler(noise: Option<u32>) -> Scheduler<World> {
    let mut scheduler = Scheduler::new();
    if let Some(draws) = noise {
        scheduler
            .register_with_streams(
                "noise",
                Cadence::Hourly,
                &["test.noise"],
                move |world: &mut World, tick| {
                    let stream = tick.stream("test.noise")?;
                    for _ in 0..draws {
                        world.noise ^= stream.draw();
                    }
                    Ok(())
                },
            )
            .unwrap();
    }
    scheduler
        .register_with_streams(
            "hourly",
            Cadence::Hourly,
            &["test.hourly"],
            |world, tick| {
                let draw = tick.stream("test.hourly")?.range(0, 99)?;
                world.hourly = world.hourly.wrapping_mul(31).wrapping_add(draw);
                Ok(())
            },
        )
        .unwrap();
    scheduler
        .register_with_streams("daily", Cadence::Daily, &["test.daily"], |world, tick| {
            let draw = tick.stream("test.daily")?.draw();
            world.daily = world.daily.rotate_left(5) ^ draw;
            Ok(())
        })
        .unwrap();
    scheduler
        .register_with_streams(
            "monthly",
            Cadence::Monthly,
            &["test.monthly"],
            |world, tick| {
                let draw = tick.stream("test.monthly")?.range_i64(-50, 50)?;
                world.monthly = world.monthly.wrapping_add(draw.cast_unsigned());
                world.notes.push(format!("month {}", tick.date()));
                Ok(())
            },
        )
        .unwrap();
    scheduler
}

pub(super) fn apply(world: &mut World, order: Order) {
    match order {
        Order::Set(key, value) => {
            world.orders.insert(key, value);
        }
        Order::Bump(amount) => world.hourly = world.hourly.wrapping_add(amount),
    }
}

pub(super) fn simulation(seed: u64) -> Simulation<World, Order> {
    Simulation::new(start(), seed, World::default(), scheduler(None), apply)
}

pub(super) fn meta(seed: u64) -> SaveMeta {
    SaveMeta {
        build: "test-build".to_owned(),
        scenario: "test-scenario".to_owned(),
        scenario_hash: "aa11".to_owned(),
        map_hash: "bb22".to_owned(),
        scripts_hash: "cc33".to_owned(),
        start: start_date(),
        seed,
    }
}

pub(super) fn expected() -> crate::Expected {
    crate::Expected {
        build: "test-build".to_owned(),
        scenario_hash: "aa11".to_owned(),
        map_hash: "bb22".to_owned(),
        scripts_hash: "cc33".to_owned(),
    }
}

/// The commands of a run, by the tick they arrive at. Two arrive at the same boundary, one exactly at day 37.
pub(super) fn plan() -> Vec<(GameTime, Order)> {
    vec![
        (after(3, 5), Order::Set("a".to_owned(), 1)),
        (after(3, 5), Order::Bump(7)),
        (after(37, 0), Order::Set("b".to_owned(), 2)),
        (after(100, 12), Order::Bump(3)),
        (after(250, 1), Order::Set("a".to_owned(), 9)),
        (after(399, 23), Order::Bump(11)),
    ]
}

/// Submits the commands due at the simulation's current tick.
pub(super) fn submit_due(
    simulation: &mut Simulation<World, Order>,
    plan: &[(GameTime, Order)],
    next: &mut usize,
) {
    while let Some((time, order)) = plan.get(*next) {
        if *time != simulation.time() {
            break;
        }
        simulation.submit(order.clone()).unwrap();
        *next += 1;
    }
}

/// Runs the simulation to `target` as a running driver would: each tick's commands are submitted, and the step applies them.
pub(super) fn run_to(
    simulation: &mut Simulation<World, Order>,
    plan: &[(GameTime, Order)],
    next: &mut usize,
    target: GameTime,
) {
    while simulation.time() < target {
        submit_due(simulation, plan, next);
        assert!(simulation.step().unwrap().is_empty());
    }
}
