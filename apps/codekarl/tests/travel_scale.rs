//! Exhaustive reachability of the shipped network; run by `travel.scale`.

use std::{collections::BTreeMap, path::Path};

use ck_sim::travel::{TravelRules, WayState};
use codekarl::travel_network::TravelPlanner;

#[test]
#[ignore = "run by the travel.scale group and the full milestone check"]
fn ireland_currach_reaches_every_place_from_every_place() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scenario = ck_map::load_scenario(root.join("assets/scenarios/ireland-768")).unwrap();
    let rules =
        TravelRules::parse(&std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap())
            .unwrap();
    let planner = TravelPlanner::new(&scenario, &rules.profiles["currach"], &rules).unwrap();
    let network = scenario.network.as_ref().unwrap();
    let states: BTreeMap<_, _> = network
        .stretches
        .iter()
        .map(|way| {
            (
                way.id,
                WayState {
                    road: way.category == ck_map::LandCategory::Road,
                    level: way.level,
                    condition: 1000,
                },
            )
        })
        .collect();
    let places: Vec<_> = network
        .nodes
        .iter()
        .filter(|node| node.kind == ck_map::LandNodeKind::Place)
        .collect();
    for from in &places {
        let routes = planner.routes_from(&scenario, &states, from.id).unwrap();
        for to in &places {
            assert!(
                routes.to(to.id).is_some(),
                "{} to {}",
                from.place.as_deref().unwrap(),
                to.place.as_deref().unwrap()
            );
        }
    }
}
