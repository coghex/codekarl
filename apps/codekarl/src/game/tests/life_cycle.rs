//! The life cycle inside the economy's daily system (CK-364 §4): deaths and births commit before the economy's step, count in
//! the pops' monthly births and deaths without being added to them, and what a failing step leaves is left for the next one.
//!
//! The economy is the real 768 start; the arithmetic of the netting itself is pinned in the economy crate.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use ck_characters::{
    Character, CharacterStore, GeneticRecord, PersonalityRecord, PopLocator, Sex, UNKNOWN,
};
use ck_economy::{EconomyState, Rules};
use ck_sim::{Canonical, Date, GameTime, Scheduler, Simulation};

use super::date;
use crate::game::{
    GameCommand, GameState, apply,
    character_storage::Handle,
    economy,
    setup::{death_pressure, register_economy_callback_with_storage},
};

fn at(year: u16, month: u8, day: u8, hour: u8) -> GameTime {
    GameTime::from_date(date(year, month, day, hour)).unwrap()
}

fn assets() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// The shipped rules and the 768 start of Ireland.
fn start() -> &'static (Rules, EconomyState) {
    static START: OnceLock<(Rules, EconomyState)> = OnceLock::new();
    START.get_or_init(|| {
        let directory = assets().join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).unwrap();
        let rules = Rules::load(assets().join("game/economy.toml")).unwrap();
        let (state, _) = economy::starting_economy(&assets(), &directory, &scenario).unwrap();
        (rules, state)
    })
}

/// A pop with workers and dependents to spare, and its place and slot: the pop the characters live in.
fn the_pop(state: &EconomyState) -> PopLocator {
    for place in &state.places {
        for (slot, pop) in place.pops.iter().enumerate() {
            if pop.workers >= 100 && pop.dependents >= 20 {
                return PopLocator {
                    place: place.id.clone(),
                    slot: u32::try_from(slot).unwrap(),
                };
            }
        }
    }
    panic!("the start has a populous pop");
}

fn heads(state: &EconomyState, pop: &PopLocator) -> (u64, u64) {
    let pop = pop_of(state, pop);
    (pop.workers, pop.dependents)
}

fn pop_of<'a>(state: &'a EconomyState, pop: &PopLocator) -> &'a ck_economy::PopState {
    &state
        .places
        .iter()
        .find(|place| place.id == pop.place)
        .unwrap()
        .pops[pop.slot as usize]
}

fn character(id: u64, born: GameTime, pop: &PopLocator, sex: Sex) -> Character {
    let mut genetics = GeneticRecord::default();
    genetics.value[9..13].copy_from_slice(&[5000; 4]);
    for stat in 0..9 {
        genetics.value[stat] = 10_000;
    }
    Character {
        id,
        name: format!("c{id}"),
        sex,
        birth: born,
        death: None,
        pop: pop.clone(),
        father: UNKNOWN,
        mother: UNKNOWN,
        spouses: Vec::new(),
        children: Vec::new(),
        office: None,
        full_until: None,
        genetics: Box::new(genetics),
        personality: PersonalityRecord::default(),
        life: Box::default(),
    }
}

fn store_of(characters: Vec<Character>) -> CharacterStore {
    let next = characters.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    let live: BTreeMap<u64, Character> = characters.into_iter().map(|c| (c.id, c)).collect();
    let mut bytes = Vec::new();
    next.encode(&mut bytes).unwrap();
    live.encode(&mut bytes).unwrap();
    CharacterStore::from_bytes(&bytes).unwrap()
}

/// A game with character storage in a directory of its own, driven one tick at a time.
struct Game {
    simulation: Simulation<GameState, GameCommand>,
    directory: PathBuf,
    handle: Handle,
}

impl Game {
    fn new(rules: Rules, characters: Vec<Character>, from: GameTime, tag: &str) -> Self {
        Self::with_economy(rules, start().1.clone(), characters, from, tag)
    }

    fn with_economy(
        rules: Rules,
        economy: EconomyState,
        characters: Vec<Character>,
        from: GameTime,
        tag: &str,
    ) -> Self {
        let directory =
            std::env::temp_dir().join(format!("ck-life-cycle-{}-{tag}", std::process::id()));
        std::fs::remove_dir_all(&directory).ok();
        std::fs::create_dir_all(&directory).unwrap();
        let handle: Handle = Arc::new(Mutex::new(Some(directory.clone())));
        let state = GameState {
            economy,
            characters: store_of(characters),
        };
        let simulation = Self::simulate(rules, &handle, state, from);
        Self {
            simulation,
            directory,
            handle,
        }
    }

    fn simulate(
        rules: Rules,
        handle: &Handle,
        state: GameState,
        from: GameTime,
    ) -> Simulation<GameState, GameCommand> {
        let mut scheduler = Scheduler::<GameState>::new();
        register_economy_callback_with_storage(&mut scheduler, Arc::new(rules), Arc::clone(handle));
        Simulation::<GameState, GameCommand>::new(from, 42, state, scheduler, apply)
    }

    /// Carry on from where the game stands under other rules (the way a repaired data file would let a failing step succeed).
    fn swap_rules(&mut self, rules: Rules) {
        let now = GameTime::from_date(self.simulation.date().unwrap()).unwrap();
        let state = self.simulation.state().clone();
        self.simulation = Self::simulate(rules, &self.handle, state, now);
    }

    /// Run every tick up to and including the economy's noon system of `day`; the economy system's failures are returned.
    fn through(&mut self, day: Date) -> Vec<String> {
        let mut failures = Vec::new();
        while self.simulation.date().unwrap() <= day {
            for error in self.simulation.step().unwrap() {
                assert_eq!(error.system, "economy.daily");
                failures.push(error.error.to_string());
            }
        }
        failures
    }

    fn state(&self) -> &GameState {
        self.simulation.state()
    }

    fn archived(&self, id: u64) -> Option<Character> {
        ck_characters::Archive::open(self.directory.join("ARCH"))
            .unwrap()
            .get(id)
            .unwrap()
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.directory).ok();
    }
}

fn noon(year: u16, month: u8, day: u8) -> Date {
    date(year, month, day, crate::game::core::ECONOMY_HOUR)
}

#[test]
fn a_certain_death_before_the_step_on_the_1st_counts_in_that_months_deaths_and_clears_the_pending_count()
 {
    let (rules, economy) = start();
    let pop = the_pop(economy);
    let before = heads(economy, &pop);
    // 105 on 768-2-1: the certain death of the pre-economy phase.
    let old = character(1, at(663, 2, 1, 0), &pop, Sex::Male);
    let mut game = Game::new(rules.clone(), vec![old], at(768, 1, 31, 0), "certain-first");
    let failures = game.through(noon(768, 2, 1));
    assert!(failures.is_empty(), "{failures:?}");
    assert!(game.state().characters.get(1).is_none());
    let record = game.archived(1).expect("archived");
    assert_eq!(record.life.death_cause.as_deref(), Some("old_age"));
    let pop_state = pop_of(&game.state().economy, &pop);
    assert_eq!(
        (pop_state.pending_births, pop_state.pending_deaths),
        (0, 0),
        "the monthly step counted and cleared them"
    );
    assert!(
        pop_state.counters.current.deaths >= 1,
        "the character's death is among the month's: {}",
        pop_state.counters.current.deaths
    );
    let after = heads(&game.state().economy, &pop);
    assert!(
        after.0 + after.1 < before.0 + before.1,
        "{before:?} -> {after:?}"
    );
    // Against the same month with no character: the character's death is one of the month's deaths, not one more.
    let mut control = Game::new(
        rules.clone(),
        Vec::new(),
        at(768, 1, 31, 0),
        "certain-control",
    );
    assert!(control.through(noon(768, 2, 1)).is_empty());
    let control_deaths = pop_of(&control.state().economy, &pop)
        .counters
        .current
        .deaths;
    assert_eq!(
        pop_state.counters.current.deaths,
        control_deaths.max(1),
        "the month's deaths: the economy's, with the character's counted among them"
    );
}

#[test]
fn a_death_on_a_later_day_waits_in_the_pending_count_for_the_next_monthly_step_once() {
    let (rules, economy) = start();
    let pop = the_pop(economy);
    let before = heads(economy, &pop);
    // 105 on 768-2-5.
    let old = character(1, at(663, 2, 5, 0), &pop, Sex::Female);
    let mut game = Game::new(rules.clone(), vec![old], at(768, 2, 2, 0), "certain-later");
    assert!(game.through(noon(768, 2, 4)).is_empty());
    assert!(game.state().characters.get(1).is_some());
    assert_eq!(pop_of(&game.state().economy, &pop).pending_deaths, 0);
    assert!(game.through(noon(768, 2, 5)).is_empty());
    assert!(game.state().characters.get(1).is_none());
    let pop_state = pop_of(&game.state().economy, &pop);
    assert_eq!(pop_state.pending_deaths, 1, "committed, not yet counted");
    let removed = heads(&game.state().economy, &pop);
    assert_eq!(
        removed.0 + removed.1,
        before.0 + before.1 - 1,
        "the head left the pop at once"
    );
    // The next monthly step counts it among that month's deaths and clears it.
    assert!(game.through(noon(768, 3, 1)).is_empty());
    let pop_state = pop_of(&game.state().economy, &pop);
    assert_eq!(pop_state.pending_deaths, 0);
    assert!(pop_state.counters.current.deaths >= 1);
}

#[test]
fn a_failing_economy_step_leaves_what_phase_a_committed_and_the_pending_counts() {
    let (rules, economy) = start();
    let pop = the_pop(economy);
    let before = heads(economy, &pop);
    // The monthly step needs the births curve: without it the 1st's step fails, and every other day's does not.
    let mut broken = rules.clone();
    broken.curves.remove("births_food");
    let old = character(1, at(663, 2, 1, 0), &pop, Sex::Male);
    let mut game = Game::new(broken, vec![old], at(768, 1, 31, 0), "failing-step");
    let failures = game.through(noon(768, 2, 1));
    assert_eq!(failures.len(), 1, "{failures:?}");
    // The death committed in phase A stands: archived, its head removed, and still pending for the next successful step.
    assert!(game.state().characters.get(1).is_none());
    assert!(game.archived(1).is_some());
    let pop_state = pop_of(&game.state().economy, &pop);
    assert_eq!(pop_state.pending_deaths, 1);
    let after = heads(&game.state().economy, &pop);
    assert_eq!(after.0 + after.1, before.0 + before.1 - 1);
}

#[test]
fn a_delivery_before_the_step_adds_its_dependent_counts_pending_and_is_counted_by_the_step() {
    let (rules, economy) = start();
    let pop = the_pop(economy);
    let before = heads(economy, &pop);
    let pregnant = || {
        let mut mother = character(1, at(743, 6, 1, 0), &pop, Sex::Female);
        mother.spouses = vec![2];
        mother.life.pregnant_by = Some(2);
        mother.life.due = Some(at(768, 2, 1, 0));
        "irish".clone_into(&mut mother.life.pool);
        let mut father = character(2, at(738, 6, 1, 0), &pop, Sex::Male);
        father.spouses = vec![1];
        "irish".clone_into(&mut father.life.pool);
        vec![mother, father]
    };
    // The step fails: the delivery still stands, with its dependent and its pending birth.
    let mut broken = rules.clone();
    broken.curves.remove("births_food");
    let mut failing = Game::new(broken, pregnant(), at(768, 1, 31, 0), "birth-failing");
    assert_eq!(failing.through(noon(768, 2, 1)).len(), 1);
    let state = failing.state();
    let child = state.characters.get(3).expect("the child");
    assert_eq!((child.father, child.mother), (2, 1));
    assert_eq!(child.pop, pop);
    let pop_state = pop_of(&state.economy, &pop);
    assert_eq!(pop_state.pending_births, 1);
    assert_eq!(heads(&state.economy, &pop), (before.0, before.1 + 1));
    // The step succeeds: the birth is counted among the month's births and cleared.
    let mut game = Game::new(
        without_births(rules),
        pregnant(),
        at(768, 1, 31, 0),
        "birth-counted",
    );
    assert!(game.through(noon(768, 2, 1)).is_empty());
    assert!(game.state().characters.get(3).is_some());
    let pop_state = pop_of(&game.state().economy, &pop);
    assert_eq!(pop_state.pending_births, 0);
    assert_eq!(
        pop_state.counters.current.births, 1,
        "the economy's births are none: the month's are the child alone"
    );
    // The mother is no longer pregnant and nurses for a year.
    let mother = game.state().characters.get(1).expect("the mother lives");
    assert_eq!(mother.life.pregnant_by, None);
    assert_eq!(mother.life.nursing_until, Some(at(769, 2, 1, 0)));
}

#[test]
fn a_newborns_birth_sample_is_the_pops_necessities_after_the_days_step_and_the_creation_sample_stands_if_it_fails()
 {
    // CK-367 §2: a delivery commits before the economy's step, which changes the pop's necessities that same day; the child's age-0
    // sample is the value of the birth day, after the step, recorded once. The shipped 768 start moves some pops' food on 1 February.
    let (rules, economy) = start();
    let pop = the_pop(economy);
    // The pop has no necessities in stock and shows 800: its food falls with every step, so the step of the birth day moves it.
    let mut hungry = economy.clone();
    {
        let place = hungry
            .places
            .iter_mut()
            .find(|p| p.id == pop.place)
            .unwrap();
        let state = &mut place.pops[pop.slot as usize];
        state.consumption.necessities_pm = 800;
        state.consumption.necessities_centi = 0;
    }
    let necessities = |game: &Game| {
        pop_of(&game.state().economy, &pop)
            .consumption
            .necessities_pm
    };
    let pregnant = || {
        let mut mother = character(1, at(743, 6, 1, 0), &pop, Sex::Female);
        mother.spouses = vec![2];
        mother.life.pregnant_by = Some(2);
        mother.life.due = Some(at(768, 2, 1, 0));
        "irish".clone_into(&mut mother.life.pool);
        let mut father = character(2, at(738, 6, 1, 0), &pop, Sex::Male);
        father.spouses = vec![1];
        "irish".clone_into(&mut father.life.pool);
        vec![mother, father]
    };
    let mut game = Game::with_economy(
        rules.clone(),
        hungry.clone(),
        pregnant(),
        at(768, 1, 31, 0),
        "birth-sample",
    );
    assert!(game.through(noon(768, 1, 31)).is_empty());
    let day_before = necessities(&game);
    assert!(game.through(noon(768, 2, 1)).is_empty());
    let birth_day = necessities(&game);
    assert_ne!(day_before, birth_day, "the step changes the pop's food");
    let child = game.state().characters.get(3).expect("the child");
    assert_eq!(
        (
            child.life.food_count,
            child.life.food_sum,
            child.life.food_age
        ),
        (1, u64::from(birth_day), 0),
        "one sample, the birth day's value and not the day before's"
    );
    // The step fails on the 1st: the delivery stands with the sample taken at creation, one sample of the value it saw.
    let mut broken = rules.clone();
    broken.curves.remove("births_food");
    let mut failing = Game::with_economy(
        broken,
        hungry,
        pregnant(),
        at(768, 1, 31, 0),
        "birth-sample-failing",
    );
    assert!(failing.through(noon(768, 1, 31)).is_empty());
    let seen = necessities(&failing);
    assert_eq!(failing.through(noon(768, 2, 1)).len(), 1);
    let child = failing.state().characters.get(3).expect("the child");
    assert_eq!(
        (child.life.food_count, child.life.food_sum),
        (1, u64::from(seen))
    );
}

#[test]
fn the_death_pressure_is_the_economys_own_multipliers_of_the_pop_as_it_stands() {
    let (rules, economy) = start();
    let pop = the_pop(economy);
    let characters = store_of(vec![character(1, at(740, 1, 1, 0), &pop, Sex::Male)]);
    let shelter = BTreeMap::new();
    let pressure = death_pressure(rules, economy, &characters, &shelter).unwrap();
    // Only pops of places with characters, and every pop of such a place.
    let place = economy.places.iter().find(|p| p.id == pop.place).unwrap();
    assert_eq!(pressure.len(), place.pops.len());
    assert!(pressure.keys().all(|locator| locator.place == pop.place));
    let multipliers = ck_economy::death_multipliers(rules, place, 0).unwrap();
    for (slot, multiplier) in multipliers.iter().enumerate() {
        let key = PopLocator {
            place: pop.place.clone(),
            slot: u32::try_from(slot).unwrap(),
        };
        assert_eq!(
            pressure[&key],
            multiplier[0] * multiplier[1] * multiplier[2]
        );
    }
    // A pop that starves is under a greater pressure than one that eats.
    let mut hungry = economy.clone();
    let mut fed = economy.clone();
    for (state, necessities) in [(&mut hungry, 0), (&mut fed, 1000)] {
        let place = state
            .places
            .iter_mut()
            .find(|place| place.id == pop.place)
            .unwrap();
        place.pops[pop.slot as usize].consumption.necessities_pm = necessities;
    }
    let of =
        |state: &EconomyState| death_pressure(rules, state, &characters, &shelter).unwrap()[&pop];
    assert!(of(&hungry) > of(&fed), "{} vs {}", of(&hungry), of(&fed));
    // No characters, no pops to read.
    let none = death_pressure(rules, economy, &CharacterStore::default(), &shelter).unwrap();
    assert!(none.is_empty());
}

/// The shipped rules with no births from the economy, so that the month's births are the characters' alone.
fn without_births(rules: &Rules) -> Rules {
    let mut rules = rules.clone();
    for kind in &mut rules.pop_types {
        kind.births_pm_year = 0;
    }
    rules
}

/// The shipped rules with a fatal event on every birthday of a character of 15 or more.
fn with_fatal_birthdays(rules: &Rules) -> Rules {
    let mut rules = rules.clone();
    let pop_types: Vec<&str> = rules.pop_types.iter().map(|p| p.id.as_str()).collect();
    rules.events = ck_economy::EventRules::parse(
        "character-events.toml",
        r#"
[[event]]
id = "fatal"
trigger = "yearly"
chance = 1
effects = [{ die = "fever" }]
text = "{name} died of a fever."
"#,
        "attributes-skills.toml",
        &std::fs::read_to_string(assets().join("game/events/attributes-skills.toml")).unwrap(),
        &ck_economy::events::Registry {
            peoples: &rules.peoples,
            genetics: &rules.genetics,
            pop_types: &pop_types,
        },
    )
    .unwrap();
    rules
}

#[test]
fn a_fatal_event_in_phase_c_of_the_1st_and_the_next_months_step_count_it_once() {
    let (rules, economy) = start();
    let rules = with_fatal_birthdays(rules);
    let pop = the_pop(economy);
    let before = heads(economy, &pop);
    // 30 on 768-2-1: the birthday pass, after the economy's step, kills it.
    let doomed = character(1, at(738, 2, 1, 0), &pop, Sex::Male);
    let mut game = Game::new(rules.clone(), vec![doomed], at(768, 1, 31, 0), "phase-c");
    assert!(game.through(noon(768, 2, 1)).is_empty());
    assert!(game.state().characters.get(1).is_none());
    assert_eq!(
        game.archived(1).unwrap().life.death_cause.as_deref(),
        Some("fever")
    );
    let february = pop_of(&game.state().economy, &pop);
    assert_eq!(
        february.pending_deaths, 1,
        "after the step: it waits for the next one"
    );
    let after = heads(&game.state().economy, &pop);
    assert!(after.0 + after.1 < before.0 + before.1);
    // The next monthly step counts it among that month's deaths, once, and clears it.
    assert!(game.through(noon(768, 3, 1)).is_empty());
    let march = pop_of(&game.state().economy, &pop);
    assert_eq!(march.pending_deaths, 0);
    let mut control = Game::new(rules, Vec::new(), at(768, 1, 31, 0), "phase-c-control");
    assert!(control.through(noon(768, 3, 1)).is_empty());
    let economys = pop_of(&control.state().economy, &pop)
        .counters
        .current
        .deaths;
    assert_eq!(
        march.counters.current.deaths,
        economys.max(1),
        "the character's death is among March's, not one more"
    );
}

#[test]
fn a_pending_death_and_a_binding_floor_realise_one_death_from_two_requested() {
    let (rules, economy) = start();
    let pop = the_pop(economy);
    // Four workers and nothing else in the pop; the month requests exactly 2 deaths (6,000 per mille a year on flat curves), and three
    // live characters are its floor: one head is removable.
    let mut rules = without_births(rules);
    for kind in &mut rules.pop_types {
        kind.worker_deaths_pm_year = 6_000;
        kind.dependent_deaths_pm_year = 0;
    }
    for curve in ["deaths_hunger", "deaths_comfort", "deaths_crowding"] {
        rules
            .curves
            .insert(curve.to_owned(), vec![[0, 1000], [2000, 1000]]);
    }
    let month = |pending: u64, tag: &str| {
        let mut economy = economy.clone();
        let place = economy
            .places
            .iter_mut()
            .find(|p| p.id == pop.place)
            .unwrap();
        for building in &mut place.buildings {
            building.employment[pop.slot as usize] = ck_economy::Employment::default();
        }
        let state = &mut place.pops[pop.slot as usize];
        state.workers = 4;
        state.dependents = 0;
        state.pending_deaths = pending;
        state.employment = vec![ck_economy::Employment::default(); state.employment.len()];
        let survivors = (1..=3)
            .map(|id| character(id, at(738, 6, 1, 0), &pop, Sex::Male))
            .collect::<Vec<_>>();
        let mut game =
            Game::with_economy(rules.clone(), economy, survivors, at(768, 1, 31, 0), tag);
        let failures = game.through(noon(768, 2, 1));
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(game.state().characters.iter().count(), 3, "the floor lives");
        let state = pop_of(&game.state().economy, &pop);
        assert_eq!(state.pending_deaths, 0);
        state.counters.current.deaths
    };
    // 2 requested, 1 removable above the floor of 3: one death is realised.
    assert_eq!(month(0, "floor-plain"), 1);
    // The same with one death already pending: 2 - 1 = 1 realised, and the month counts 1 + 1.
    assert_eq!(month(1, "floor-pending"), 2);
}

#[test]
fn a_retained_debit_is_consumed_once_by_the_next_successful_step() {
    let (rules, economy) = start();
    let pop = the_pop(economy);
    let mut broken = rules.clone();
    broken.curves.remove("births_food");
    let run = |characters: Vec<Character>, tag: &str| {
        let mut game = Game::new(broken.clone(), characters, at(768, 1, 31, 0), tag);
        assert_eq!(game.through(noon(768, 2, 1)).len(), 1, "the 1st fails");
        let kept = pop_of(&game.state().economy, &pop).pending_deaths;
        game.swap_rules(rules.clone());
        assert!(game.through(noon(768, 3, 1)).is_empty(), "repaired");
        let march = pop_of(&game.state().economy, &pop);
        assert_eq!(march.pending_deaths, 0);
        let counted = march.counters.current.deaths;
        // And it is not counted again the month after.
        assert!(game.through(noon(768, 4, 1)).is_empty());
        assert_eq!(pop_of(&game.state().economy, &pop).pending_deaths, 0);
        (kept, counted)
    };
    let old = character(1, at(663, 2, 1, 0), &pop, Sex::Male);
    let (kept, counted) = run(vec![old], "debit");
    assert_eq!(kept, 1, "the failing step left it");
    let (none, economys) = run(Vec::new(), "debit-control");
    assert_eq!(none, 0);
    assert_eq!(
        counted,
        economys.max(1),
        "once: the economy's own, with it among them"
    );
}
