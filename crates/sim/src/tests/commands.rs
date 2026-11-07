use crate::{Cadence, CommandStamp, Date, GameTime, Scheduler, Simulation};

/// A state that records what happened to it, in order, so the order of commands and systems shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct State {
    events: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
struct Add(u32);

fn start() -> GameTime {
    GameTime::from_date(Date {
        year: 768,
        month: 10,
        day: 9,
        hour: 5,
    })
    .unwrap()
}

fn simulation() -> Simulation<State, Add> {
    let mut scheduler = Scheduler::<State>::new();
    scheduler
        .register("hourly", Cadence::Hourly, |state, time, _| {
            state.events.push(format!("tick {}", time.hours()));
            Ok(())
        })
        .unwrap();
    Simulation::new(
        start(),
        State::default(),
        scheduler,
        |state, Add(amount)| {
            state.events.push(format!("add {amount}"));
        },
    )
}

#[test]
fn commands_apply_in_the_order_submitted() {
    let mut simulation = simulation();
    for amount in [3, 1, 2] {
        simulation.submit(Add(amount));
    }
    simulation.apply_pending();
    assert_eq!(simulation.state().events, ["add 3", "add 1", "add 2"]);
    // Nothing is applied twice.
    simulation.apply_pending();
    assert_eq!(simulation.state().events.len(), 3);
}

#[test]
fn submitting_stamps_the_current_time_and_the_next_sequence_number() {
    let mut simulation = simulation();
    assert_eq!(
        simulation.submit(Add(1)),
        CommandStamp {
            time: start(),
            sequence: 0
        }
    );
    assert_eq!(
        simulation.submit(Add(2)),
        CommandStamp {
            time: start(),
            sequence: 1
        }
    );
    simulation.step().unwrap();
    // The sequence continues across ticks and after the queue is drained; the time is the new hour.
    assert_eq!(
        simulation.submit(Add(3)),
        CommandStamp {
            time: start().next(),
            sequence: 2
        }
    );
}

#[test]
fn a_step_applies_the_queued_commands_before_the_tick_systems_then_advances_time() {
    let mut simulation = simulation();
    simulation.submit(Add(1));
    simulation.submit(Add(2));
    simulation.step().unwrap();
    assert_eq!(
        simulation.state().events,
        ["add 1", "add 2", &format!("tick {}", start().hours())]
    );
    assert_eq!(simulation.time(), start().next());
}

#[test]
fn a_command_submitted_after_a_tick_waits_for_the_next_tick_and_runs_before_its_systems() {
    let mut simulation = simulation();
    simulation.step().unwrap();
    simulation.submit(Add(7));
    simulation.step().unwrap();
    assert_eq!(
        simulation.state().events,
        [
            format!("tick {}", start().hours()),
            "add 7".to_owned(),
            format!("tick {}", start().next().hours()),
        ]
    );
}

#[test]
fn a_paused_order_applied_at_once_and_a_running_order_applied_by_the_next_step_give_equal_state() {
    // Negative control: applying a paused command after the systems instead of before them changes the event order.
    let mut paused = simulation();
    paused.submit(Add(5));
    paused.apply_pending();
    paused.step().unwrap();

    let mut running = simulation();
    running.submit(Add(5));
    running.step().unwrap();

    assert_eq!(paused.state(), running.state());
    assert_eq!(paused.time(), running.time());
}

#[test]
fn the_equivalence_holds_over_several_commands_and_ticks() {
    let script: [&[u32]; 4] = [&[1, 2], &[], &[3], &[4, 5, 6]];

    let mut paused = simulation();
    let mut running = simulation();
    for commands in script {
        for amount in commands {
            paused.submit(Add(*amount));
            paused.apply_pending();
            running.submit(Add(*amount));
        }
        paused.step().unwrap();
        running.step().unwrap();
    }

    assert_eq!(paused.state(), running.state());
}

#[test]
fn commands_within_one_boundary_keep_arrival_order_across_paused_and_running_submissions() {
    let mut simulation = simulation();
    simulation.submit(Add(1));
    simulation.apply_pending();
    simulation.submit(Add(2));
    simulation.submit(Add(3));
    simulation.step().unwrap();
    assert_eq!(&simulation.state().events[..3], ["add 1", "add 2", "add 3"]);
}
