//! What the design's section 5.4 promises: the same seed and commands give the same state, whatever else differs.

use std::{num::NonZeroU32, time::Duration};

use super::world::{
    Order, World, after, apply, expected, meta, plan, run_to, scheduler, simulation, start,
    submit_due,
};
use crate::{Clock, GameTime, Save, Simulation, Speed};

const DAYS: u64 = 400;

fn end() -> GameTime {
    after(DAYS, 0)
}

fn digest(simulation: &Simulation<World, Order>) -> u64 {
    simulation.digest().unwrap()
}

/// The uninterrupted run: the digest at the end, and the simulation itself.
fn uninterrupted(seed: u64) -> Simulation<World, Order> {
    let mut simulation = simulation(seed);
    run_to(&mut simulation, &plan(), &mut 0, end());
    simulation
}

#[test]
fn the_same_seed_and_commands_give_equal_digests_after_400_days() {
    let first = uninterrupted(42);
    let second = uninterrupted(42);
    assert_eq!(digest(&first), digest(&second));
    assert_eq!(first.state(), second.state());
    assert_eq!(first.journal(), second.journal());
    // The state really moved: this is not two empty worlds agreeing.
    assert_ne!(first.state().hourly, 0);
    assert_ne!(first.state().daily, 0);
    assert_ne!(first.state().monthly, 0);
    assert_eq!(first.state().notes.len(), 14);
}

#[test]
fn another_seed_or_another_command_gives_another_digest() {
    let reference = digest(&uninterrupted(42));
    assert_ne!(reference, digest(&uninterrupted(43)));

    let mut fewer = plan();
    fewer.pop();
    let mut simulation = simulation(42);
    run_to(&mut simulation, &fewer, &mut 0, end());
    assert_ne!(reference, digest(&simulation));
}

#[test]
fn saved_at_day_37_loaded_and_continued_gives_the_uninterrupted_digest() {
    let plan = plan();
    let mut original = simulation(42);
    let mut next = 0;
    run_to(&mut original, &plan, &mut next, after(37, 0));
    // Day 37 holds a command that is still pending at the boundary the save is taken at.
    submit_due(&mut original, &plan, &mut next);
    assert_eq!(original.pending_len(), 1);

    let bytes = original.save(meta(42)).to_bytes().unwrap();
    let save = Save::<World>::from_bytes(&bytes, &expected()).unwrap();
    let mut loaded = Simulation::from_save(save, scheduler(None), apply).unwrap();
    assert_eq!(loaded.time(), original.time());
    assert_eq!(loaded.seed(), 42);
    assert_eq!(loaded.state(), original.state());
    assert_eq!(loaded.pending_len(), 1);
    assert_eq!(loaded.journal(), original.journal());
    assert_eq!(digest(&loaded), digest(&original));

    run_to(&mut loaded, &plan, &mut next, end());
    let straight = uninterrupted(42);
    assert_eq!(digest(&loaded), digest(&straight));
    assert_eq!(loaded.journal(), straight.journal());
}

#[test]
fn saving_at_every_kind_of_boundary_continues_to_the_same_digest() {
    let straight = digest(&uninterrupted(42));
    // Day 0, a month start, a day with no command, the leap day of 768 (day 59) and the last day.
    for day in [0, 31, 59, 100, 399] {
        let plan = plan();
        let mut simulation = simulation(42);
        let mut next = 0;
        run_to(&mut simulation, &plan, &mut next, after(day, 0));
        let bytes = simulation.save(meta(42)).to_bytes().unwrap();
        let save = Save::<World>::from_bytes(&bytes, &expected()).unwrap();
        let mut loaded = Simulation::from_save(save, scheduler(None), apply).unwrap();
        run_to(&mut loaded, &plan, &mut next, end());
        assert_eq!(digest(&loaded), straight, "saved at day {day}");
    }
}

/// Drives the simulation to `target` through a `Clock` under a fake time, as the application's driver will. Running, each
/// command is submitted and the next step applies it; paused, it is applied at once.
///
/// `wakes` are the real milliseconds between wakes, cycled. `speeds` change the speed at a given wake. Each of `pauses` stops
/// the clock when the simulation reaches that tick and resumes it after that many wakes. Returns the simulation and how many
/// commands were applied while paused.
fn drive(
    max_ticks: u32,
    wakes: &[u64],
    speeds: &[(usize, u8)],
    pauses: &[(GameTime, usize)],
) -> (Simulation<World, Order>, usize) {
    let plan = plan();
    let mut simulation = simulation(42);
    let mut next = 0;
    let mut clock = Clock::with_max_ticks_per_wake(NonZeroU32::new(max_ticks).unwrap());
    clock.set_speed(Speed::new(speeds[0].1).unwrap());
    clock.resume(Duration::ZERO);
    let mut now = Duration::ZERO;
    let mut count = 0;
    let mut resume_at = None;
    let mut done = vec![false; pauses.len()];
    let mut paused_applied = 0;
    while simulation.time() < end() {
        assert!(count < 5_000_000, "the driver does not finish");
        now += Duration::from_millis(wakes[count % wakes.len()]);
        if let Some((_, speed)) = speeds.iter().find(|(at, _)| *at == count) {
            clock.set_speed(Speed::new(*speed).unwrap());
        }
        if resume_at == Some(count) {
            clock.resume(now);
            resume_at = None;
        }
        for _ in 0..clock.due(now) {
            let hit = pauses
                .iter()
                .position(|(at, _)| *at == simulation.time())
                .filter(|index| !done[*index]);
            if let Some(index) = hit {
                done[index] = true;
                clock.pause();
                resume_at = Some(count + pauses[index].1);
                break;
            }
            if simulation.time() >= end() {
                break;
            }
            submit_due(&mut simulation, &plan, &mut next);
            assert!(simulation.step().unwrap().is_empty());
        }
        if clock.is_paused() {
            submit_due(&mut simulation, &plan, &mut next);
            paused_applied += simulation.pending_len();
            simulation.apply_pending();
        }
        count += 1;
    }
    (simulation, paused_applied)
}

#[test]
fn speed_pauses_and_wake_sizes_change_nothing() {
    let straight = uninterrupted(42);
    let pauses = [
        // Two commands arrive at day 3 hour 5, one at day 37 and one at the very last hour.
        (after(3, 5), 25),
        (after(37, 0), 3),
        (after(100, 12), 40),
        (after(399, 23), 7),
    ];
    let runs = [
        // Steady speed 5 in big wakes, then in tiny ones, then with a one-tick cap that makes the game slower than real time.
        (drive(64, &[40], &[(0, 5)], &[]), 0),
        (drive(64, &[1, 3, 17, 2], &[(0, 5)], &[]), 0),
        (drive(1, &[500], &[(0, 5)], &[]), 0),
        // Speed changes, and pauses with orders submitted while paused.
        (
            drive(
                7,
                &[33, 90, 5],
                &[(0, 2), (200, 4), (900, 1), (1500, 5)],
                &pauses,
            ),
            5,
        ),
        (drive(1, &[400], &[(0, 5)], &pauses), 5),
    ];
    for ((run, paused_applied), expected_paused) in runs {
        assert_eq!(run.time(), end());
        assert_eq!(digest(&run), digest(&straight));
        assert_eq!(run.journal(), straight.journal());
        assert_eq!(run.journal().len(), 6);
        // Every pause stops at a tick that has commands, so those are the ones applied while paused.
        assert_eq!(paused_applied, expected_paused);
    }
}

#[test]
fn the_pause_patterns_really_apply_commands_while_paused() {
    let (_, paused_applied) = drive(
        1,
        &[400],
        &[(0, 5)],
        &[(after(3, 5), 25), (after(37, 0), 3)],
    );
    assert_eq!(paused_applied, 3);
}

#[test]
fn a_command_applied_while_paused_and_one_applied_by_the_next_step_journal_the_same() {
    let plan = plan();
    let mut paused = simulation(42);
    let mut running = simulation(42);
    let (mut next_paused, mut next_running) = (0, 0);
    while paused.time() < end() {
        submit_due(&mut paused, &plan, &mut next_paused);
        paused.apply_pending();
        assert!(paused.step().unwrap().is_empty());
        submit_due(&mut running, &plan, &mut next_running);
        assert!(running.step().unwrap().is_empty());
    }
    assert_eq!(digest(&paused), digest(&running));
    assert_eq!(paused.journal(), running.journal());
}

#[test]
fn a_stream_is_unchanged_by_a_new_stream_or_by_how_much_another_draws() {
    let plan = plan();
    let quiet = {
        let mut simulation = simulation(42);
        run_to(&mut simulation, &plan, &mut 0, end());
        simulation
    };
    for draws in [0, 1, 17] {
        let mut noisy =
            Simulation::new(start(), 42, World::default(), scheduler(Some(draws)), apply);
        run_to(&mut noisy, &plan, &mut 0, end());
        let (noisy, quiet) = (noisy.state(), quiet.state());
        assert_eq!(noisy.hourly, quiet.hourly, "{draws} extra draws");
        assert_eq!(noisy.daily, quiet.daily);
        assert_eq!(noisy.monthly, quiet.monthly);
        assert_eq!(noisy.orders, quiet.orders);
        assert_eq!(noisy.notes, quiet.notes);
        assert_eq!(noisy.noise != 0, draws % 2 == 1 || draws > 1);
    }
}

#[test]
fn replay_from_day_0_reproduces_the_digest_and_the_journal() {
    let original = uninterrupted(42);
    let mut replayed = simulation(42);
    let errors = replayed.replay(original.journal(), end()).unwrap();
    assert!(errors.is_empty());
    assert_eq!(replayed.time(), end());
    assert_eq!(digest(&replayed), digest(&original));
    assert_eq!(replayed.journal(), original.journal());
    assert_eq!(replayed.state(), original.state());
}

#[test]
fn replay_applies_a_journal_taken_while_paused_at_its_own_tick() {
    // A run that stops with orders applied at the final boundary, as a paused driver leaves it.
    let plan = vec![
        (after(2, 3), Order::Set("a".to_owned(), 1)),
        (after(2, 3), Order::Bump(4)),
        (after(5, 0), Order::Bump(9)),
    ];
    let mut original = simulation(7);
    let mut next = 0;
    run_to(&mut original, &plan, &mut next, after(5, 0));
    submit_due(&mut original, &plan, &mut next);
    original.apply_pending();
    assert_eq!(original.journal().len(), 3);

    let mut replayed = simulation(7);
    replayed.replay(original.journal(), after(5, 0)).unwrap();
    assert_eq!(digest(&replayed), digest(&original));
    assert_eq!(replayed.journal(), original.journal());
}
