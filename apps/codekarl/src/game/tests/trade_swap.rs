//! The daily swap between two pops of one place, at the game's boundary (CK-391 rule 14, 21 and the save witness).
//!
//! A fort of peasants and a noble house: the nobles own the demesne, so only they have market capacity (200 litres a day). They
//! want fish, which the peasants hold above their target, and hold oats above theirs, which the peasants want. A bushel is 35
//! litres and a pound of fish one, so the day's capacity cuts the swap short: more goods would move were the noon run again.
//! The game's own noon step runs the exchange; no counter is added to see it.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use ck_economy::{
    EconomyState, LandCounts, NobleStart, Rules, ScenarioEconomy, StartDefaults, StartLocation,
    StartOverride, starting_state,
};
use ck_sim::{GameTime, Save, Scheduler, Simulation};

use super::{date, identity};
use crate::game::{GameCommand, GameState, apply, setup::register_economy_callback_with_storage};

fn rules() -> Rules {
    Rules::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
    )
    .unwrap()
}

fn at(day: u8, hour: u8) -> GameTime {
    GameTime::from_date(date(768, 1, day, hour)).unwrap()
}

/// The fort at the start of 768-01-02, with the nobles holding oats and the peasants fish, each wanting the other's.
fn fort(rules: &Rules) -> EconomyState {
    let data = ScenarioEconomy {
        horses_always: Vec::new(),
        defaults: BTreeMap::from([(
            "ringfort".to_owned(),
            BTreeMap::from([(
                "1".to_owned(),
                StartDefaults {
                    peasants: 100,
                    monks: 0,
                    labourers: 0,
                    farm_levels: 0,
                    holdings_levels: 0,
                    storehouse_levels: 0,
                    pop_stock: BTreeMap::new(),
                    building_stock: BTreeMap::new(),
                },
            )]),
        )]),
        places: BTreeMap::from([(
            "fort".to_owned(),
            StartOverride {
                culture: Some("ulaid".to_owned()),
                religion: Some("christianity".to_owned()),
                monk_religion: Some("christianity".to_owned()),
                ..StartOverride::default()
            },
        )]),
        nobles: BTreeMap::from([(
            "fort".to_owned(),
            NobleStart {
                culture: "ulaid".to_owned(),
                religion: "christianity".to_owned(),
                people: 40,
                demesne_levels: 2,
                labourers: 20,
            },
        )]),
    };
    let (mut state, _) = starting_state(
        rules,
        &data,
        &[StartLocation {
            id: "fort".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 40)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 24_000)]),
                fertile_cells: 40,
                fertile_cells_by_terrain: BTreeMap::from([("pasture".to_owned(), 40)]),
                sea: 0,
            },
        }],
        9,
    )
    .unwrap();
    let good = |id: &str| rules.goods.iter().position(|item| item.id == id).unwrap();
    let place = &mut state.places[0];
    let nobles = place
        .pops
        .iter()
        .position(|pop| pop.kind == "noble")
        .unwrap();
    let peasants = place
        .pops
        .iter()
        .position(|pop| pop.kind == "peasant")
        .unwrap();
    for pop in &mut place.pops {
        pop.stock.fill(0);
    }
    let heads = |place: &ck_economy::PlaceState, pop: usize| {
        place.pops[pop].workers + place.pops[pop].dependents
    };
    // Per person: the nobles hold 15 bushels of oats (target about 3) and the peasants 25 lb of fish (target about 12), within the
    // stores' limits.
    let (noble_heads, peasant_heads) = (heads(place, nobles), heads(place, peasants));
    place.pops[nobles].stock[good("oats")] = 15 * noble_heads;
    place.pops[peasants].stock[good("fish")] = 25 * peasant_heads;
    state
}

fn scheduler(rules: &Rules) -> Scheduler<GameState> {
    let mut scheduler = Scheduler::<GameState>::new();
    register_economy_callback_with_storage(
        &mut scheduler,
        Arc::new(rules.clone()),
        Arc::new(Mutex::new(None)),
    );
    scheduler
}

fn game(
    rules: &Rules,
    economy: EconomyState,
    from: GameTime,
) -> Simulation<GameState, GameCommand> {
    Simulation::new(
        from,
        42,
        GameState {
            economy,
            characters: ck_characters::CharacterStore::default(),
            stretches: BTreeMap::new(),
            lows: crate::game::lows::Lows::default(),
        },
        scheduler(rules),
        apply,
    )
}

fn run_to(simulation: &mut Simulation<GameState, GameCommand>, target: GameTime) {
    while simulation.time() < target {
        let errors = simulation.step().unwrap();
        assert!(errors.is_empty(), "{errors:?}");
    }
    assert_eq!(simulation.time(), target);
}

/// Saved to bytes and loaded again, as the game's quicksave does.
fn reloaded(
    rules: &Rules,
    simulation: &Simulation<GameState, GameCommand>,
) -> Simulation<GameState, GameCommand> {
    let identity = identity("aa");
    let bytes = simulation.save(identity.meta(42)).to_bytes().unwrap();
    let save = Save::<GameState>::from_bytes(&bytes, &identity.expected()).unwrap();
    Simulation::from_save(save, scheduler(rules), apply).unwrap()
}

/// The place's goods wherever they are held: every pop's store and the buildings' stock. The derived orders are outside it.
fn goods(simulation: &Simulation<GameState, GameCommand>) -> Vec<Vec<u64>> {
    let place = &simulation.state().economy.places[0];
    let mut all = place
        .pops
        .iter()
        .map(|pop| pop.stock.clone())
        .collect::<Vec<_>>();
    all.extend(
        place
            .buildings
            .iter()
            .map(|building| building.stock.clone()),
    );
    all
}

fn units(
    simulation: &Simulation<GameState, GameCommand>,
    rules: &Rules,
    kind: &str,
    id: &str,
) -> u64 {
    let place = &simulation.state().economy.places[0];
    let pop = place.pops.iter().position(|pop| pop.kind == kind).unwrap();
    place.pops[pop].stock[rules.goods.iter().position(|item| item.id == id).unwrap()]
}

/// Everything the comparison holds equal: the digest of the canonical state and the economy but for the derived orders.
fn canonical(simulation: &Simulation<GameState, GameCommand>) -> (u64, EconomyState) {
    let mut economy = simulation.state().economy.clone();
    for place in &mut economy.places {
        place.last_market_orders = None;
    }
    (simulation.digest().unwrap(), economy)
}

#[test]
fn the_noon_exchange_makes_one_capacity_cut_swap_and_a_save_at_eleven_or_thirteen_changes_nothing()
{
    let rules = rules();
    let economy = fort(&rules);
    let before = at(2, 0);

    // The unbroken game: one noon, then the next.
    let mut unbroken = game(&rules, economy.clone(), before);
    run_to(&mut unbroken, at(2, 11));
    let eleven = canonical(&unbroken);
    let held = goods(&unbroken);
    run_to(&mut unbroken, at(2, 13));
    let after_noon = canonical(&unbroken);
    assert_ne!(goods(&unbroken), held);
    // The same day without capacity anywhere has no swap; what differs is the swap, which ate and drew the same either way.
    let mut no_capacity = rules.clone();
    for building in &mut no_capacity.buildings {
        building.market_capacity_ml_per_level = 0;
    }
    let mut control = game(&no_capacity, economy.clone(), before);
    run_to(&mut control, at(2, 13));
    // The nobles' draws are the same either way, so what differs is the swap: 5 bushels of oats for 15 lb of fish, which takes
    // 5 x 35 + 15 = 190 of the nobles' 200 litres; a sixth bushel and the 17 lb it is worth would take 228.
    let moved = |unbroken: &Simulation<GameState, GameCommand>,
                 control: &Simulation<GameState, GameCommand>,
                 id: &str| {
        i128::from(units(unbroken, &rules, "noble", id))
            - i128::from(units(control, &rules, "noble", id))
    };
    assert_eq!(
        (
            moved(&unbroken, &control, "oats"),
            moved(&unbroken, &control, "fish")
        ),
        (-5, 15),
        "one swap, to the literal pair"
    );
    // And no more that day: nothing runs again before the next noon.
    run_to(&mut unbroken, at(2, 23));
    run_to(&mut control, at(2, 23));
    assert_eq!(
        (
            moved(&unbroken, &control, "oats"),
            moved(&unbroken, &control, "fish")
        ),
        (-5, 15)
    );
    assert_eq!(
        canonical(&unbroken).1.places[0].pops,
        after_noon.1.places[0].pops
    );
    run_to(&mut unbroken, at(3, 13));
    let after_second_noon = canonical(&unbroken);
    assert_ne!(after_second_noon.0, after_noon.0);

    // Saved at 11:00, before the noon tick, and loaded: the boundary and the two noons after it match the unbroken game.
    let mut early = game(&rules, economy.clone(), before);
    run_to(&mut early, at(2, 11));
    let mut early = reloaded(&rules, &early);
    assert_eq!(canonical(&early), eleven);
    run_to(&mut early, at(2, 13));
    assert_eq!(
        canonical(&early),
        after_noon,
        "an exchange missed after the load"
    );
    run_to(&mut early, at(3, 13));
    assert_eq!(canonical(&early), after_second_noon);

    // Saved at 13:00, straight after the real noon tick, and loaded: the noon is not run again.
    let mut late = game(&rules, economy, before);
    run_to(&mut late, at(2, 13));
    let mut late = reloaded(&rules, &late);
    assert_eq!(
        canonical(&late),
        after_noon,
        "an exchange made twice after the load"
    );
    run_to(&mut late, at(3, 13));
    assert_eq!(canonical(&late), after_second_noon);
}

#[test]
fn a_building_payment_for_scarce_fish_survives_saves_around_noon() {
    let mut rules = rules();
    let wool = rules.goods.iter().position(|g| g.id == "wool").unwrap();
    rules.goods[wool].stacked_outside = true;
    let mut economy = fort(&rules);
    let place = &mut economy.places[0];
    let oats = rules.goods.iter().position(|g| g.id == "oats").unwrap();
    let fish = rules.goods.iter().position(|g| g.id == "fish").unwrap();
    let peasants = place.pops.iter().position(|p| p.kind == "peasant").unwrap();
    let nobles = place.pops.iter().position(|p| p.kind == "noble").unwrap();
    let labourers = place
        .pops
        .iter()
        .position(|p| p.kind == "labourer")
        .unwrap();
    let demesne = place
        .buildings
        .iter()
        .position(|b| b.kind == "demesne")
        .unwrap();
    place.pops[nobles].stock[oats] = 0;
    for pop in &mut place.pops {
        pop.stock[wool] = 100_000;
    }
    place.pops[labourers].stock[oats] = 400;
    place.pops[peasants].stock[fish] = 1_300;
    place.buildings[demesne].stock[oats] = 1_000;
    let before = at(2, 0);
    let mut unbroken = game(&rules, economy.clone(), before);
    run_to(&mut unbroken, at(2, 11));
    let eleven = canonical(&unbroken);
    run_to(&mut unbroken, at(2, 13));
    let thirteen = canonical(&unbroken);
    let mut no_capacity = rules.clone();
    for building in &mut no_capacity.buildings {
        building.market_capacity_ml_per_level = 0;
    }
    let mut control = game(&no_capacity, economy.clone(), before);
    run_to(&mut control, at(2, 13));
    let actual = &unbroken.state().economy.places[0];
    let idle = &control.state().economy.places[0];
    let orders = actual.last_market_orders.as_ref().unwrap();
    assert_eq!(orders.sell_units[fish], 100);
    assert!(orders.buy_units[fish] > orders.sell_units[fish]);
    let moved = (
        i128::from(actual.buildings[demesne].stock[oats])
            - i128::from(idle.buildings[demesne].stock[oats]),
        i128::from(actual.pops[nobles].stock[fish]) - i128::from(idle.pops[nobles].stock[fish]),
    );
    assert_eq!(moved, (-4, 14));
    let mut larger = rules.clone();
    for building in &mut larger.buildings {
        building.market_capacity_ml_per_level = 1_000_000_000;
    }
    let mut larger_turn = game(&larger, economy.clone(), before);
    run_to(&mut larger_turn, at(2, 13));
    let extra_fish = i128::from(larger_turn.state().economy.places[0].pops[nobles].stock[fish])
        - i128::from(idle.pops[nobles].stock[fish]);
    assert!(
        extra_fish > moved.1,
        "capacity must cut the swap short: {extra_fish}"
    );
    run_to(&mut unbroken, at(3, 13));
    let next = canonical(&unbroken);
    let mut early = game(&rules, economy.clone(), before);
    run_to(&mut early, at(2, 11));
    let mut early = reloaded(&rules, &early);
    assert_eq!(canonical(&early), eleven);
    run_to(&mut early, at(2, 13));
    assert_eq!(canonical(&early), thirteen);
    run_to(&mut early, at(3, 13));
    assert_eq!(canonical(&early), next);
    let mut late = game(&rules, economy, before);
    run_to(&mut late, at(2, 13));
    let mut late = reloaded(&rules, &late);
    assert_eq!(canonical(&late), thirteen);
    run_to(&mut late, at(3, 13));
    assert_eq!(canonical(&late), next);
}

#[test]
fn a_fuel_swap_survives_saves_before_and_after_noon() {
    let mut rules = rules();
    rules
        .goods
        .iter_mut()
        .find(|g| g.id == "hardwood")
        .unwrap()
        .base_price = 100;
    let mut economy = fort(&rules);
    let place = &mut economy.places[0];
    let good = |id: &str| rules.goods.iter().position(|g| g.id == id).unwrap();
    let (wood, peat, fish, oats) = (good("hardwood"), good("peat"), good("fish"), good("oats"));
    let noble = place.pops.iter().position(|p| p.kind == "noble").unwrap();
    let peasant = place.pops.iter().position(|p| p.kind == "peasant").unwrap();
    let noble_heads = place.pops[noble].workers + place.pops[noble].dependents;
    let peasant_heads = place.pops[peasant].workers + place.pops[peasant].dependents;
    place.pops[noble].stock[fish] = noble_heads * 25;
    place.pops[noble].stock[wood] = 0;
    place.pops[noble].stock[peat] = 0;
    place.pops[noble].consumption.hearth_milli = 0;
    place.pops[peasant].stock[wood] = 2;
    place.pops[peasant].consumption.hearth_milli = peasant_heads * 20 * 240;
    place.prices_milli[wood] = 100_000;
    place.prices_milli[oats] = 100_000;

    let mut no_capacity = rules.clone();
    for building in &mut no_capacity.buildings {
        building.market_capacity_ml_per_level = 0;
    }
    let mut control = game(&no_capacity, economy.clone(), at(2, 0));
    run_to(&mut control, at(2, 13));
    let mut unbroken = game(&rules, economy.clone(), at(2, 0));
    run_to(&mut unbroken, at(2, 11));
    let mut early = reloaded(&rules, &unbroken);
    run_to(&mut unbroken, at(2, 13));
    run_to(&mut early, at(2, 13));
    let after = canonical(&unbroken);
    assert_eq!(canonical(&early), after);
    assert!(
        units(&unbroken, &rules, "peasant", "hardwood")
            < units(&control, &rules, "peasant", "hardwood"),
        "the noon actually traded fuel"
    );
    let mut late = reloaded(&rules, &unbroken);
    assert_eq!(canonical(&late), after);
    run_to(&mut unbroken, at(3, 13));
    run_to(&mut early, at(3, 13));
    run_to(&mut late, at(3, 13));
    assert_eq!(canonical(&early), canonical(&unbroken));
    assert_eq!(canonical(&late), canonical(&unbroken));
}
