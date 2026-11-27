//! The travel network as a trader's look asks for it (CK-433, trade spec 11d.2 rule 3 and 11b.3 rule 10).
//!
//! A look wants, for a home place, every place the network reaches and the clipped way out and back to it for a party at a pace. The
//! way **out** is read from one forward single-source search from home and the way **back** from one reverse search into home, for each
//! effective profile (the party's profile with the speeds of each mode scaled by the pace it has on that mode, rule 6); both are kept for the hour, so every trader of a
//! place due in that hour with the same effective profile shares them, and dropped when the hour passes (a stretch's state may change
//! then). The planners, which measure the ways and the water once for a profile, are kept for the game.

use std::collections::BTreeMap;
use std::sync::Arc;

use ck_economy::{Journey, NearbyPlace, Paces, Party, Way, Ways};
use ck_sim::travel::{TravelProfile, TravelRules, WayState};

use super::expedition_route::{JourneyBuilder, scaled_by_mode};
use super::travel::{Routes, TravelPlanner};

/// What the network is made of, shared by every look of a game: the scenario with its network, the travel rules and profiles, the
/// pairs of neighbour places and the visiting radius.
pub(crate) struct TradeNetwork {
    pub(crate) scenario: ck_map::Scenario,
    pub(crate) rules: TravelRules,
    /// The neighbour places of each place (under twice the visiting radius apart, in a straight line).
    pub(crate) neighbours: BTreeMap<String, Vec<NearbyPlace>>,
    /// The visiting radius, in metres (r155).
    pub(crate) radius_m: u64,
}

/// What the searches cost, for the evidence (11d.2 rule 3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SearchCounts {
    /// Forward single-source searches run.
    pub(crate) forward: u64,
    /// Reverse single-source searches run.
    pub(crate) reverse: u64,
    /// Planners made (the measure of the ways and the water graph for one effective profile).
    pub(crate) planners: u64,
    /// Clipped ways built.
    pub(crate) journeys: u64,
    /// The time spent making planners, running the forward searches, the reverse searches and clipping ways, for the evidence.
    pub(crate) planner_time: std::time::Duration,
    pub(crate) forward_time: std::time::Duration,
    pub(crate) reverse_time: std::time::Duration,
    pub(crate) journey_time: std::time::Duration,
    /// How many ways were clipped (the rest were read from the clipped ways already built) and the time that took.
    pub(crate) clips: u64,
    pub(crate) clip_time: std::time::Duration,
}

/// A tally of the searches, held by whoever watches (the evidence's harness, the cases) and handed to the network.
pub(crate) type SearchWatch = Arc<std::sync::Mutex<SearchCounts>>;

/// A party at its paces on each mode: the effective profile a search is made for.
type Key = (String, bool, Paces);

/// A clipped way: the party (its profile and whether it is a boat alone), the two places, which way, whether its geometry is frozen and
/// the itinerary (the transitions it takes, in travel order).
type ClippedKey = ((String, bool), String, String, bool, bool, Vec<u64>);

/// The state of every stretch as a new game starts, as the searches read it.
pub(crate) fn starting_states(scenario: &ck_map::Scenario) -> BTreeMap<u64, WayState> {
    super::travel::current_way_states(&super::stretch::starting(scenario.network.as_ref()))
}

/// The network for a game's looks.
pub(crate) struct NetworkWays {
    network: Arc<TradeNetwork>,
    states: BTreeMap<u64, WayState>,
    planners: BTreeMap<Key, TravelPlanner>,
    forward: BTreeMap<(String, Key), Routes>,
    reverse: BTreeMap<(String, Key), Routes>,
    journeys: BTreeMap<(String, String, Key, bool, bool), Option<Journey>>,
    /// The clipped ways already built, by the itinerary they clip: the way a pair of places is clipped to depends on the itinerary the
    /// search chose and not on the pace it was searched at, and many paces choose one itinerary.
    clipped: BTreeMap<ClippedKey, Journey>,
    /// Where the searches are tallied, for the evidence's harness and the cases; the game gives none and keeps no counts.
    watch: Option<SearchWatch>,
}

impl NetworkWays {
    pub(crate) fn new(network: Arc<TradeNetwork>, watch: Option<SearchWatch>) -> Self {
        Self {
            network,
            states: BTreeMap::new(),
            planners: BTreeMap::new(),
            forward: BTreeMap::new(),
            reverse: BTreeMap::new(),
            journeys: BTreeMap::new(),
            clipped: BTreeMap::new(),
            watch,
        }
    }

    /// Begins an hour: the searches of the hour before are dropped and the stretches' states are those of now.
    pub(crate) fn begin(&mut self, states: BTreeMap<u64, WayState>) {
        self.states = states;
        self.forward.clear();
        self.reverse.clear();
        self.journeys.clear();
        self.clipped.clear();
    }

    /// Tells the watch, if there is one, of something done.
    fn note(&self, tally: impl FnOnce(&mut SearchCounts)) {
        if let Some(watch) = &self.watch
            && let Ok(mut counts) = watch.lock()
        {
            tally(&mut counts);
        }
    }

    fn base(&self, party: &Party) -> Result<TravelProfile, String> {
        let mut profile = self
            .network
            .rules
            .profiles
            .get(&party.profile)
            .ok_or_else(|| format!("unknown travel profile {}", party.profile))?
            .clone();
        if party.boat_only {
            // A boat alone cannot walk an approach (CK-402 r10 rule 1): the profile has no land speeds.
            profile.path = None;
            profile.road = None;
        }
        Ok(profile)
    }

    fn node(&self, place: &str) -> Result<u64, String> {
        self.network
            .scenario
            .network
            .as_ref()
            .and_then(|network| network.place_node(place))
            .map(|node| node.id)
            .ok_or_else(|| format!("place {place} is not on the network"))
    }

    /// The planner for an effective profile, made once.
    fn planner(&mut self, key: &Key, party: &Party) -> Result<(), String> {
        if !self.planners.contains_key(key) {
            let started = std::time::Instant::now();
            let effective = scaled_by_mode(&self.base(party)?, key.2);
            let planner =
                TravelPlanner::new(&self.network.scenario, &effective, &self.network.rules)?;
            let spent = started.elapsed();
            self.note(|counts| {
                counts.planners += 1;
                counts.planner_time += spent;
            });
            self.planners.insert(key.clone(), planner);
        }
        Ok(())
    }

    /// The forward search from `home` for a key, made once for the hour.
    fn forward_search(&mut self, home: &str, key: &Key, party: &Party) -> Result<(), String> {
        self.planner(key, party)?;
        let node = self.node(home)?;
        let slot = (home.to_owned(), key.clone());
        if !self.forward.contains_key(&slot) {
            let started = std::time::Instant::now();
            let routes =
                self.planners[key].routes_from(&self.network.scenario, &self.states, node)?;
            let spent = started.elapsed();
            self.note(|counts| {
                counts.forward += 1;
                counts.forward_time += spent;
            });
            self.forward.insert(slot, routes);
        }
        Ok(())
    }

    /// The forward search from `home` and the reverse search into it for a key, made once for the hour.
    fn searches(&mut self, home: &str, key: &Key, party: &Party) -> Result<(), String> {
        self.forward_search(home, key, party)?;
        let node = self.node(home)?;
        let slot = (home.to_owned(), key.clone());
        if !self.reverse.contains_key(&slot) {
            let started = std::time::Instant::now();
            let routes =
                self.planners[key].routes_to(&self.network.scenario, &self.states, node)?;
            let spent = started.elapsed();
            self.note(|counts| {
                counts.reverse += 1;
                counts.reverse_time += spent;
            });
            self.reverse.insert(slot, routes);
        }
        Ok(())
    }
}

impl Ways for NetworkWays {
    fn neighbours(&self, a: &str, b: &str) -> bool {
        self.network
            .neighbours
            .get(a)
            .is_some_and(|list| list.iter().any(|other| other.id == b))
    }

    fn destinations(&mut self, home: &str, party: &Party) -> Result<Vec<String>, String> {
        // Reachability does not depend on the pace (a pace only scales the speeds), so the searches are at walking pace on both modes.
        let key: Key = (party.profile.clone(), party.boat_only, Paces::uniform(1000));
        self.searches(home, &key, party)?;
        let slot = (home.to_owned(), key);
        let (forward, reverse) = (&self.forward[&slot], &self.reverse[&slot]);
        let network = self
            .network
            .scenario
            .network
            .as_ref()
            .ok_or("scenario has no network")?;
        Ok(network
            .nodes
            .iter()
            .filter(|node| node.kind == ck_map::LandNodeKind::Place)
            .filter_map(|node| {
                let name = node.place.as_ref()?;
                (name != home && forward.to(node.id).is_some() && reverse.to(node.id).is_some())
                    .then(|| name.clone())
            })
            .collect())
    }

    fn journey(
        &mut self,
        home: &str,
        other: &str,
        party: &Party,
        paces: Paces,
        way: Way,
    ) -> Result<Option<Journey>, String> {
        self.way(home, other, party, paces, way, false)
    }

    fn freeze(
        &mut self,
        home: &str,
        other: &str,
        party: &Party,
        paces: Paces,
        way: Way,
    ) -> Result<Option<Journey>, String> {
        self.way(home, other, party, paces, way, true)
    }

    fn clipped_hours(
        &mut self,
        home: &str,
        other: &str,
        party: &Party,
    ) -> Result<Option<num_rational::BigRational>, String> {
        let paces = Paces::uniform(1000);
        match self.way(home, other, party, paces, Way::Out, false) {
            Ok(journey) => Ok(journey.map(|journey| journey.hours(paces))),
            // A way that never leaves the radius of home is a valid way of no time (the expedition chooser keeps neighbours away from
            // it; the opening's nearness does not).
            Err(problem) if problem == super::expedition_route::WITHIN_RADIUS => {
                Ok(Some(num_rational::BigRational::from_integer(0.into())))
            }
            Err(problem) => Err(problem),
        }
    }

    /// An island place is one the network gives an access entry, offshore at all tides or at high tide (CK-434, the designer): it is
    /// reached only by water. The entries include the lough islands, which have a barge landing and so are given a barge first.
    fn island(&self, place: &str) -> bool {
        self.network
            .scenario
            .network
            .as_ref()
            .is_some_and(|network| network.access.iter().any(|access| access.place == place))
    }
}

impl NetworkWays {
    /// One way of a trip, with its geometry frozen or not.
    fn way(
        &mut self,
        home: &str,
        other: &str,
        party: &Party,
        paces: Paces,
        way: Way,
        geometry: bool,
    ) -> Result<Option<Journey>, String> {
        let key: Key = (party.profile.clone(), party.boat_only, paces);
        let cached = (
            home.to_owned(),
            other.to_owned(),
            key.clone(),
            way == Way::Out,
            geometry,
        );
        if let Some(found) = self.journeys.get(&cached) {
            return Ok(found.clone());
        }
        self.searches(home, &key, party)?;
        let (home_node, other_node) = (self.node(home)?, self.node(other)?);
        let slot = (home.to_owned(), key.clone());
        let base = self.base(party)?;
        let routes = match way {
            Way::Out => &self.forward[&slot],
            Way::Back => &self.reverse[&slot],
        };
        let (from, to) = match way {
            Way::Out => (home_node, other_node),
            Way::Back => (other_node, home_node),
        };
        // A place the network does not connect for this party has no way, which is not an error: both searches answer for `other`.
        let started = std::time::Instant::now();
        let found = if let Some(path) = routes.to(other_node) {
            let clip = (
                (party.profile.clone(), party.boat_only),
                home.to_owned(),
                other.to_owned(),
                way == Way::Out,
                geometry,
                path.edges.clone(),
            );
            if let Some(known) = self.clipped.get(&clip) {
                Some(known.clone())
            } else {
                let builder = JourneyBuilder::with_planner(
                    &self.network.scenario,
                    &self.network.rules,
                    &base,
                    &self.planners[&key],
                    &self.states,
                    self.network.radius_m,
                );
                let clip_started = std::time::Instant::now();
                let journey = builder.way_in(routes, from, to, way == Way::Back, geometry)?;
                let spent = clip_started.elapsed();
                self.note(|counts| {
                    counts.clips += 1;
                    counts.clip_time += spent;
                });
                let built = Journey {
                    legs: journey.legs,
                    arrival: journey.arrival,
                };
                self.clipped.insert(clip, built.clone());
                Some(built)
            }
        } else {
            None
        };
        let spent = started.elapsed();
        self.note(|counts| {
            counts.journeys += 1;
            counts.journey_time += spent;
        });
        self.journeys.insert(cached, found.clone());
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use num_rational::BigRational;

    use super::*;

    /// The shipped scenario, travel rules and the starting state of every stretch.
    fn world() -> (ck_map::Scenario, TravelRules, BTreeMap<u64, WayState>) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scenario = ck_map::load_scenario(root.join("assets/scenarios/ireland-768")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let states = super::super::travel::current_way_states(&super::super::stretch::starting(
            scenario.network.as_ref(),
        ));
        (scenario, rules, states)
    }

    fn network_ways() -> (NetworkWays, SearchWatch, BTreeMap<u64, WayState>) {
        let (scenario, rules, states) = world();
        let neighbours = super::super::setup::trade_neighbours(&scenario);
        let network = Arc::new(TradeNetwork {
            scenario,
            rules,
            neighbours,
            radius_m: 2000,
        });
        let watch = SearchWatch::default();
        let mut ways = NetworkWays::new(network, Some(Arc::clone(&watch)));
        ways.begin(states.clone());
        (ways, watch, states)
    }

    fn party(profile: &str, boat_only: bool) -> Party {
        Party {
            profile: profile.to_owned(),
            boat_only,
        }
    }

    /// A cart's pace: on land, with no way by water.
    fn on_land(pace: u32) -> Paces {
        Paces {
            land: pace,
            water: 0,
        }
    }

    fn hours(journey: &Journey, pace: u32) -> BigRational {
        journey
            .legs
            .iter()
            .fold(BigRational::from_integer(0.into()), |sum, leg| {
                sum + leg.hours(pace)
            })
    }

    /// CK-434: the opening's two questions of the network. An island is a place with an `[[access]]` entry, and the clipped hours of a way
    /// are the hours of its clipped legs, with a way that never leaves the radius of home a way of no time rather than an error.
    #[test]
    fn islands_are_the_places_with_an_access_entry_and_the_clipped_hours_of_a_near_producer_are_no_error()
     {
        let (mut ways, _, _) = network_ways();
        assert!(ways.island("inishmacsaint"), "an offshore place");
        assert!(!ways.island("tara"), "a place on the mainland");
        let cart = party("cart", false);
        let hours_to =
            |ways: &mut NetworkWays, other: &str| ways.clipped_hours("tara", other, &cart).unwrap();
        let journey = ways
            .journey("tara", "armagh", &cart, Paces::uniform(1000), Way::Out)
            .unwrap()
            .unwrap();
        assert_eq!(
            hours_to(&mut ways, "armagh"),
            Some(journey.hours(Paces::uniform(1000))),
            "the hours of the clipped legs"
        );
        // Every pair of neighbour places, which lie under 4 km apart and may lie within the radius of each other, has an answer.
        let pairs: Vec<(String, String)> = ways
            .network
            .neighbours
            .iter()
            .flat_map(|(from, near)| {
                near.iter()
                    .map(move |other| (from.clone(), other.id.clone()))
            })
            .take(8)
            .collect();
        assert!(!pairs.is_empty());
        for (from, other) in pairs {
            let found = ways.clipped_hours(&from, &other, &cart);
            assert!(found.is_ok(), "{from} to {other}: {found:?}");
        }
    }

    /// The reverse search gives a place's time home exactly as a forward search from that place does (itineraries may differ only
    /// among ways of equal time and legs, which the search's own fixed rule tells apart), for every kind of party the plans use.
    #[test]
    fn the_reverse_search_takes_the_time_a_forward_search_from_the_destination_takes() {
        let (scenario, rules, states) = world();
        let n = scenario.network.as_ref().unwrap();
        let home = n.place_node("tara").unwrap().id;
        let places: Vec<u64> = n
            .nodes
            .iter()
            .filter(|node| node.kind == ck_map::LandNodeKind::Place)
            .map(|node| node.id)
            .step_by(7)
            .collect();
        assert!(places.len() >= 10);
        for (name, boat_only) in [
            ("cart", false),
            ("barge", false),
            ("currach", false),
            ("currach", true),
        ] {
            let mut profile = rules.profiles[name].clone();
            if boat_only {
                profile.path = None;
                profile.road = None;
            }
            let planner = TravelPlanner::new(&scenario, &profile, &rules).unwrap();
            let reverse = planner.routes_to(&scenario, &states, home).unwrap();
            let mut reached = 0;
            for place in &places {
                let forward = planner.routes_from(&scenario, &states, *place).unwrap();
                let (back, answer) = (
                    forward.to(home).map(|path| path.hours.clone()),
                    reverse.to(*place).map(|path| path.hours.clone()),
                );
                assert_eq!(
                    back, answer,
                    "{name} boat-only {boat_only}: place {place} to tara"
                );
                // The itinerary has the legs a forward search from the place gives, in the order travelled (ways of equal time and equal
                // legs may differ in the water steps between: the reverse search's own fixed rule chooses among them, and what it chooses
                // is the way evaluated and frozen).
                assert_eq!(
                    forward.to(home).map(|path| path.legs.clone()),
                    reverse.to(*place).map(|path| path.legs.clone()),
                    "{name}: place {place} to tara has the forward search's legs"
                );
                if let Some(path) = reverse.to(*place) {
                    reached += 1;
                    // Its itinerary is in travel order: it expands to legs from the place to home, and they take the path's time.
                    let trip = reverse
                        .itinerary(&scenario, planner.ways(), *place, &rules)
                        .unwrap_or_else(|| panic!("{name}: an itinerary from {place}"));
                    let total = trip
                        .iter()
                        .fold(BigRational::from_integer(0.into()), |sum, leg| {
                            sum + &leg.hours
                        });
                    assert_eq!(total, path.hours, "{name}: the legs add up to the path");
                }
            }
            assert!(reached > 0 || boat_only, "{name} reaches something");
        }
    }

    #[test]
    fn a_look_is_given_every_reachable_place_with_no_time_cut_off_and_the_searches_are_shared_for_the_hour()
     {
        let (mut ways, watch, states) = network_ways();
        let cart = party("cart", false);
        let found = ways.destinations("tara", &cart).unwrap();
        assert!(!found.contains(&"tara".to_owned()));
        assert!(found.contains(&"clonmacnoise".to_owned()));
        assert!(
            found.len() > 40,
            "every place the network reaches, far ones too: {}",
            found.len()
        );
        assert_eq!(
            {
                let counts = watch.lock().unwrap();
                (counts.forward, counts.reverse, counts.planners)
            },
            (1, 1, 1),
            "one forward and one reverse search from home"
        );
        // Ways at one pace for several destinations share those searches; another pace is another effective profile.
        for other in ["clonmacnoise", "clonfert", "inishmacsaint"] {
            let Some(out) = ways
                .journey("tara", other, &cart, on_land(800), Way::Out)
                .unwrap()
            else {
                continue;
            };
            assert!(!out.legs.is_empty(), "{other}");
        }
        assert_eq!(
            {
                let counts = watch.lock().unwrap();
                (counts.forward, counts.reverse, counts.planners)
            },
            (2, 2, 2)
        );
        // Asked again in the same hour, nothing is searched; the next hour searches afresh and keeps the planners.
        ways.journey("tara", "clonmacnoise", &cart, on_land(800), Way::Out)
            .unwrap();
        assert_eq!(watch.lock().unwrap().forward, 2);
        ways.begin(states);
        ways.journey("tara", "clonmacnoise", &cart, on_land(800), Way::Out)
            .unwrap();
        assert_eq!(
            {
                let counts = watch.lock().unwrap();
                (counts.forward, counts.planners)
            },
            (3, 2),
            "a new hour searches again, the planners are kept"
        );
    }

    #[test]
    fn the_clipped_way_out_and_back_is_the_quickest_way_less_the_visiting_radius_at_each_end() {
        let (mut ways, _, _) = network_ways();
        let cart = party("cart", false);
        let out = ways
            .journey("tara", "clonmacnoise", &cart, on_land(1000), Way::Out)
            .unwrap()
            .expect("a way out");
        let back = ways
            .journey("tara", "clonmacnoise", &cart, on_land(1000), Way::Back)
            .unwrap()
            .expect("a way back");
        // The unclipped trips: centre to centre, by a forward search from each end.
        let (scenario, rules, states) = world();
        let n = scenario.network.as_ref().unwrap();
        let planner = TravelPlanner::new(&scenario, &rules.profiles["cart"], &rules).unwrap();
        let (a, b) = (
            n.place_node("tara").unwrap().id,
            n.place_node("clonmacnoise").unwrap().id,
        );
        let centre_out = planner
            .routes_from(&scenario, &states, a)
            .unwrap()
            .to(b)
            .unwrap()
            .hours
            .clone();
        let centre_back = planner
            .routes_from(&scenario, &states, b)
            .unwrap()
            .to(a)
            .unwrap()
            .hours
            .clone();
        assert!(
            hours(&out, 1000) < centre_out,
            "clipped out is shorter than centre to centre"
        );
        assert!(hours(&back, 1000) < centre_back, "so is the way back");
        // The reverse search gives the way back its own direction's time: the same as a forward search from the destination, less the
        // clip (which is the same two ends' radius).
        assert!(hours(&back, 1000) > BigRational::from_integer(0.into()));
        // Places with no way between them for a party have none, which is not an error.
        let island = ways
            .journey("tara", "killeany", &cart, on_land(1000), Way::Out)
            .unwrap();
        assert!(island.is_none(), "a cart cannot reach an island");
    }

    /// A party with a pace on each mode searches with the land speeds scaled by its land pace and the water speeds by its water pace,
    /// and a mode it has no pace on is left out of the search.
    #[test]
    fn the_effective_profile_scales_each_mode_by_its_own_pace_and_drops_a_mode_with_none() {
        let (_, rules, _) = world();
        let base = rules.profiles.get("currach").expect("the currach profile");
        let scaled = scaled_by_mode(
            base,
            Paces {
                land: 500,
                water: 1000,
            },
        );
        let half = |speed: u64| (speed * 500 / 1000).max(1);
        assert_eq!(scaled.path, base.path.map(|speeds| speeds.map(half)));
        assert_eq!(scaled.river, base.river, "a water speed at the water pace");
        assert_eq!(scaled.sea, base.sea);
        let on_water = scaled_by_mode(
            base,
            Paces {
                land: 0,
                water: 800,
            },
        );
        assert!(on_water.path.is_none() && on_water.road.is_none());
        assert!(on_water.river.is_some() || on_water.sea.is_some() || on_water.lough.is_some());
        let on_land = scaled_by_mode(
            base,
            Paces {
                land: 800,
                water: 0,
            },
        );
        assert!(on_land.river.is_none() && on_land.lough.is_none() && on_land.sea.is_none());
        assert!(on_land.path.is_some());
    }

    /// The fixture network (places a to e, an isle and a shore, with a travel profile for each kind of party).
    fn fixture() -> (ck_map::Scenario, TravelRules, BTreeMap<u64, WayState>) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scenario =
            ck_map::load_scenario(root.join("crates/map/tests/network/scenarios/fixture")).unwrap();
        let rules = TravelRules::parse(
            &std::fs::read_to_string(root.join("assets/game/travel.toml")).unwrap(),
        )
        .unwrap();
        let states = super::super::travel::current_way_states(&super::super::stretch::starting(
            scenario.network.as_ref(),
        ));
        (scenario, rules, states)
    }

    /// CK-433 spec 12 part D: a strait drawn across 300 km of map with a stored length of 8 km is crossed in the time its stored length
    /// takes, which is far under the 168-hour limit (at most 41 hours): the way is found with no cut-off on the distance a screen
    /// would have read off the drawing.
    #[test]
    fn a_strait_drawn_300_km_across_with_a_stored_length_of_8_km_takes_the_time_its_stored_length_takes()
     {
        let (mut scenario, rules, states) = fixture();
        let network = scenario.network.as_mut().unwrap();
        network.straits[0].length_m = 8000;
        // Node 7, the landing, is drawn 358 cells (839 m each, about 300 km) east of the isle's node 6.
        let landing = network.nodes.iter_mut().find(|node| node.id == 7).unwrap();
        landing.cell[0] += 358;
        let drawn = {
            let place = network.nodes.iter().find(|node| node.id == 6).unwrap().cell;
            let landing = network.nodes.iter().find(|node| node.id == 7).unwrap().cell;
            let (dx, dy) = (
                f64::from(landing[0] - place[0]),
                f64::from(landing[1] - place[1]),
            );
            dx.hypot(dy) * ck_map::CELL_METRES
        };
        assert!((299_000.0..=301_000.0).contains(&drawn), "{drawn} m");
        let ways = super::super::travel::measure_ways(&scenario, &rules).unwrap();
        let mut boat = rules.profiles["currach"].clone();
        boat.path = None;
        boat.road = None;
        let routes =
            super::super::travel::routes_from(&scenario, &ways, &states, &boat, &rules, 6).unwrap();
        let path = routes.to(7).expect("the strait is crossed");
        let limit = BigRational::from_integer(41.into());
        assert!(path.hours <= limit, "{} h", path.hours);
        // The strait itself takes the time the stored 8 km takes at the currach's sea speed, not 300 km's.
        let trip = routes.itinerary(&scenario, &ways, 7, &rules).unwrap();
        assert_eq!(trip.len(), 3, "a transfer at either end");
        let sea = BigRational::from_integer(8000.into())
            / BigRational::from_integer(boat.sea.unwrap().into());
        assert!(
            trip[1].hours >= sea,
            "the strait takes at least its stored length"
        );
        assert!(
            trip[1].hours < BigRational::from_integer(41.into()),
            "{}",
            trip[1].hours
        );
    }

    /// CK-433 spec 12 part D: a place whose centre-to-centre round trip is over a limit but whose clipped trip (2 km off each end, both
    /// ways) is within it is given to the look, and the way it is given is the clipped one: the search has no cut-off, and the hours it
    /// returns are the clipped trip's, which fit the limit the centre-to-centre trip does not.
    #[test]
    fn a_place_whose_centre_to_centre_round_trip_is_over_the_limit_is_given_with_its_clipped_trip_within_it()
     {
        let (scenario, rules, states) = fixture();
        let neighbours = BTreeMap::new();
        let (a, b) = (
            scenario
                .network
                .as_ref()
                .unwrap()
                .place_node("a")
                .unwrap()
                .id,
            scenario
                .network
                .as_ref()
                .unwrap()
                .place_node("b")
                .unwrap()
                .id,
        );
        let cart = rules.profiles["cart"].clone();
        let planner = TravelPlanner::new(&scenario, &cart, &rules).unwrap();
        let out = planner.routes_from(&scenario, &states, a).unwrap();
        let back = planner.routes_to(&scenario, &states, a).unwrap();
        let centre = &out.to(b).unwrap().hours + &back.to(b).unwrap().hours;
        let network = Arc::new(TradeNetwork {
            scenario,
            rules,
            neighbours,
            radius_m: 2000,
        });
        let mut ways = NetworkWays::new(network, None);
        ways.begin(states);
        let party = party("cart", false);
        // At walking pace on land, as a look's first search is.
        let pace = on_land(1000);
        assert!(
            ways.destinations("a", &party)
                .unwrap()
                .contains(&"b".to_owned()),
            "no cut-off on the time a place is from home"
        );
        let clipped = ways
            .journey("a", "b", &party, pace, Way::Out)
            .unwrap()
            .unwrap();
        let home = ways
            .journey("a", "b", &party, pace, Way::Back)
            .unwrap()
            .unwrap();
        let round = hours(&clipped, 1000) + hours(&home, 1000);
        // A limit an hour over the clipped trip is under the centre-to-centre one (which the clipping shortens by 4 km each way).
        let limit = &round + BigRational::from_integer(1.into());
        assert!(
            round <= limit && limit < centre,
            "{round} <= {limit} < {centre}"
        );
    }

    /// A party whose load fits only on land searches with the land half of a mixed profile: the water capability goes as a unit, so the
    /// profile is valid and a way on land is found (it used to be refused as a class without speeds).
    #[test]
    fn a_mixed_party_with_no_pace_on_water_searches_the_land_alone() {
        let (scenario, rules, states) = fixture();
        let base = rules.profiles["currach"].clone();
        let land_only = scaled_by_mode(&base, on_land(1000));
        land_only.validate().expect("a land-only profile is valid");
        assert!(land_only.water_class.is_none());
        let planner = TravelPlanner::new(&scenario, &land_only, &rules).unwrap();
        let a = scenario
            .network
            .as_ref()
            .unwrap()
            .place_node("a")
            .unwrap()
            .id;
        let b = scenario
            .network
            .as_ref()
            .unwrap()
            .place_node("b")
            .unwrap()
            .id;
        let routes = planner.routes_from(&scenario, &states, a).unwrap();
        assert!(routes.to(b).is_some(), "the way on land is found");
        // The same through the adapter, for the party of a wagon carrying a currach.
        let neighbours = BTreeMap::new();
        let network = Arc::new(TradeNetwork {
            scenario,
            rules,
            neighbours,
            radius_m: 2000,
        });
        let mut ways = NetworkWays::new(network, None);
        ways.begin(states);
        let journey = ways
            .journey("a", "b", &party("currach", false), on_land(1000), Way::Out)
            .unwrap();
        assert!(journey.is_some());
    }

    /// A look at `date` for the trader of place `home` over the fixture network, with `limit` hours as its pop's limit: whether it chose
    /// a trip, and the plan it chose.
    fn looks(
        scenario: &ck_map::Scenario,
        rules: &TravelRules,
        states: &BTreeMap<u64, WayState>,
        economy: &ck_economy::Rules,
        state: &ck_economy::EconomyState,
        date: ck_sim::Date,
        limit: u64,
    ) -> Option<ck_economy::Plan> {
        let mut economy = economy.clone();
        economy
            .expedition
            .limit_hours
            .insert("peasant".to_owned(), limit);
        let network = Arc::new(TradeNetwork {
            scenario: scenario.clone(),
            rules: rules.clone(),
            neighbours: BTreeMap::new(),
            radius_m: 2000,
        });
        let mut ways = NetworkWays::new(network, None);
        ways.begin(states.clone());
        let mut state = state.clone();
        let mut report = ck_economy::LookReport::default();
        ck_economy::run_trade_hour_with_looks(&economy, &mut state, date, &mut ways, &mut report)
            .unwrap();
        let place = &state.places[0];
        // The peasant's trip: the trader of pop 0.
        place
            .trade
            .jobs
            .iter()
            .filter(|job| job.pop == 0)
            .find_map(|job| match &job.kind {
                ck_economy::JobKind::Trade { plan } => Some((**plan).clone()),
                _ => None,
            })
            .or_else(|| {
                place
                    .trade
                    .expeditions
                    .iter()
                    .find(|expedition| expedition.trader == 0)
                    .map(|expedition| expedition.plan.clone())
            })
    }

    /// CK-434, through the real adapter: the opening's first look of the peasant of `a` finds the trip to `b` on the fixture network, makes
    /// it a job, starts it at once (its vehicle and people are at home) part way out by its own draw, and the state it leaves is one a game
    /// at the first tick can hold. The same seed opens the same game.
    #[test]
    fn the_opening_over_the_fixture_network_starts_the_peasants_trip_part_way_out_and_the_same_seed_opens_the_same_game()
     {
        let (scenario, rules, states) = fixture();
        let mut economy = ck_economy::fixtures::rules_with(&["hand_cart"]);
        economy.expedition.looks = true;
        let network = Arc::new(TradeNetwork {
            scenario,
            rules,
            neighbours: BTreeMap::new(),
            radius_m: 2000,
        });
        let start = ck_sim::GameTime::from_date(ck_sim::Date {
            year: 768,
            month: 3,
            day: 10,
            hour: 6,
        })
        .unwrap()
        .hours();
        let open = |seed: u64| {
            let mut state = ck_economy::fixtures::world(vec![
                ck_economy::fixtures::home_place(&economy, "a", 4000, 4000),
                ck_economy::fixtures::dest_place(&economy, "b", 8000, 1500),
            ]);
            let mut ways = NetworkWays::new(Arc::clone(&network), None);
            ways.begin(states.clone());
            let report =
                ck_economy::run_opening(&economy, &mut state, start, seed, &mut ways, &mut ())
                    .unwrap();
            (state, report)
        };
        let (state, report) = open(42);
        assert!(report.looks.jobs >= 1, "{report:?}");
        let peasant = state.places[0]
            .trade
            .expeditions
            .iter()
            .find(|expedition| expedition.trader == 0)
            .expect("the peasant's trip started at once");
        assert_eq!(peasant.plan.destination, 1, "the trip to b");
        assert!(peasant.plan.arrival.is_some(), "its way is frozen");
        assert!(peasant.left < start, "it left before the first tick");
        ck_economy::validate_trade_state_at(&economy, &state, start)
            .expect("a state the first tick can hold");
        let (again, _) = open(42);
        assert!(again == state, "the same seed, the same opening");
    }

    /// CK-433 spec 12 part D, through the real adapter and the look: a trader of `a` whose trip to `b` is over the limit centre to
    /// centre and within it clipped is given the trip, and chooses it at the cap, to the hour: the cap that keeps it is the clipped round
    /// trip with the noon wait, and one hour less refuses it.
    #[test]
    fn a_profitable_trip_over_the_limit_centre_to_centre_is_chosen_at_the_cap_when_its_clipped_trip_fits()
     {
        let (scenario, rules, states) = fixture();
        let economy = ck_economy::fixtures::rules_with(&["hand_cart"]);
        let state = ck_economy::fixtures::world(vec![
            ck_economy::fixtures::home_place(&economy, "a", 40, 40),
            ck_economy::fixtures::dest_place(&economy, "b", 100, 100),
        ]);
        let chosen = |hour: u8, limit: u64| {
            let date = ck_sim::Date {
                year: 768,
                month: 3,
                day: 10,
                hour,
            };
            looks(&scenario, &rules, &states, &economy, &state, date, limit).is_some()
        };
        // The cap that just keeps the trip, at the hour of the day with the shortest noon wait.
        let tightest = |hour: u8| {
            let (mut low, mut high) = (0_u64, 400_u64);
            assert!(chosen(hour, high), "the trip is chosen with room to spare");
            while high - low > 1 {
                let middle = u64::midpoint(low, high);
                if chosen(hour, middle) {
                    high = middle;
                } else {
                    low = middle;
                }
            }
            high
        };
        let cap = (0..24).map(tightest).min().unwrap();
        assert!((0..24).any(|hour| chosen(hour, cap)), "chosen at the cap");
        assert!(
            !(0..24).any(|hour| chosen(hour, cap - 1)),
            "refused an hour under it"
        );
        // The centre-to-centre round trip is over that cap.
        let cart = rules.profiles["cart"].clone();
        let planner = TravelPlanner::new(&scenario, &cart, &rules).unwrap();
        let place = |id: &str| {
            scenario
                .network
                .as_ref()
                .unwrap()
                .place_node(id)
                .unwrap()
                .id
        };
        let out = planner.routes_from(&scenario, &states, place("a")).unwrap();
        let back = planner.routes_to(&scenario, &states, place("a")).unwrap();
        let centre = &out.to(place("b")).unwrap().hours + &back.to(place("b")).unwrap().hours;
        assert!(
            BigRational::from_integer(cap.into()) < centre,
            "{cap} h is under the centre-to-centre round trip of {centre} h at walking pace"
        );
    }

    /// CK-433 spec 12 part D, through the real adapter and the look: the isle's strait is drawn 300 km across and stored 8 km long, and
    /// the isle's own water landings are shut so that the strait is its only way off (a fixture with the isle's other landings open
    /// has the trader board there instead). A trader of the isle goes to `shore` with a wagon and a currach (no one walks): the trip is
    /// chosen at the peasant's limit of 168 hours, noon wait included, its way out begins with the strait, and that leg takes the
    /// time of the stored 8 km (under 41 hours), not of the drawn 300 km.
    #[test]
    fn a_profitable_trip_across_the_strait_drawn_300_km_and_stored_8_km_is_chosen_at_168_hours() {
        let (mut scenario, rules, states) = fixture();
        let network = scenario.network.as_mut().unwrap();
        network.straits[0].length_m = 8000;
        network
            .nodes
            .iter_mut()
            .find(|node| node.id == 7)
            .unwrap()
            .cell[0] += 358;
        for landing in network
            .landings
            .iter_mut()
            .filter(|landing| landing.node == 6)
        {
            landing.bodies.iter_mut().for_each(|body| *body = -1);
        }
        // A wagon that carries the currach on land and is carried by it on water, and light barley.
        let mut economy = ck_economy::fixtures::rules_with(&["covered_wagon", "currach"]);
        economy.vehicles[0].carried_ml = Some(2_000_000);
        economy.expedition.foot_people = 0;
        for item in &mut economy.goods {
            if item.id == "barley" {
                item.weight_g = 500;
            }
        }
        let mut home = ck_economy::fixtures::home_place(&economy, "isle", 40, 40);
        home.trade.vehicles.push(ck_economy::Vehicle {
            number: 2,
            kind: 1,
            pop: u32::try_from(ck_economy::fixtures::PEASANT).unwrap(),
            free: true,
            materials: None,
        });
        let mut state = ck_economy::fixtures::world(vec![
            home,
            ck_economy::fixtures::dest_place(&economy, "shore", 100, 100),
        ]);
        state.trade.vehicles = 2;
        let date = ck_sim::Date {
            year: 768,
            month: 3,
            day: 10,
            hour: 2,
        };
        let plan = looks(&scenario, &rules, &states, &economy, &state, date, 168)
            .expect("the trip across the strait is chosen at the limit");
        assert_eq!(plan.destination, 1);
        let strait = plan
            .out
            .first()
            .filter(|leg| leg.key[0] == 4)
            .unwrap_or_else(|| {
                panic!(
                    "the way out begins with the strait: {:?}",
                    plan.out.iter().map(|leg| leg.key).collect::<Vec<_>>()
                )
            });
        assert_eq!(strait.mode, ck_economy::Mode::Water);
        // The limit is the one that decides: the same trip is refused at a limit it cannot meet (the noon wait and the way home alone
        // are more than ten hours).
        assert!(looks(&scenario, &rules, &states, &economy, &state, date, 10).is_none());
        assert!(
            strait.hours(1000) < BigRational::from_integer(41.into()),
            "the strait takes {} h at walking pace",
            strait.hours(1000)
        );
    }
}
