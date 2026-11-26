//! `game.go_to(kind, id, zoom, revision)` (CK-381): what a thing on the map is, as a point, and the call that moves the camera to it.
//!
//! The UI names a thing by its kind and identity and the economy revision it holds (the revision of the `game_time` call it last
//! received, as the character screen reads). The point is resolved against that revision and no other, so a camera move and the window
//! the UI opens for the same thing never disagree. A new kind of thing is one more arm of [`Targets::resolve`].

use std::collections::BTreeMap;
use std::sync::Arc;

use ck_lua::{ConfigValue, HostFunction};
use ck_map::{FixedPoint, Scenario};

use crate::game::view::EconomyView;
use crate::game_api::ViewCache;
use crate::map_app::SharedController;

/// What a go-to can reach in this run: the points of the scenario's places and of the map's rivers, fixed at load.
pub(crate) struct Targets {
    /// Each place's id and the centre of its cell (the point its marker is drawn around, before quantising to a zoom step).
    places: BTreeMap<String, FixedPoint>,
    /// Each named river's id and its mouth (see [`ck_map::RiverNetwork::mouth`]).
    rivers: BTreeMap<String, FixedPoint>,
}

/// The thing a go-to names, as the UI gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Thing {
    /// A place, by its id.
    Location(String),
    /// A pop: its place and its zero-based slot there.
    Pop(String, u32),
    /// A character, by its world-scoped id.
    Character(u64),
    /// A river, by its id.
    River(String),
}

impl Targets {
    /// The points of `scenario`'s places and rivers.
    pub(crate) fn from_scenario(scenario: &Scenario) -> Self {
        let places = scenario
            .locations
            .iter()
            .map(|location| (location.id.clone(), location.center()))
            .collect();
        let rivers = scenario
            .map
            .river_network
            .as_ref()
            .map(|network| {
                network
                    .rivers
                    .iter()
                    .enumerate()
                    .filter_map(|(index, river)| Some((river.id.clone(), network.mouth(index)?)))
                    .collect()
            })
            .unwrap_or_default();
        Self { places, rivers }
    }

    fn place_point(&self, id: &str) -> Result<FixedPoint, String> {
        self.places
            .get(id)
            .copied()
            .ok_or_else(|| format!("no place `{id}`"))
    }

    /// The point of `thing` as `view` has it. An unknown id, a pop slot the place does not hold at that revision, and a character that
    /// is not live there are refusals.
    pub(crate) fn resolve(&self, thing: &Thing, view: &EconomyView) -> Result<FixedPoint, String> {
        match thing {
            Thing::Location(id) => self.place_point(id),
            Thing::Pop(place, slot) => {
                let held = view
                    .place(place)
                    .ok_or_else(|| format!("no place `{place}` at this revision"))?;
                if usize::try_from(*slot).is_ok_and(|slot| slot < held.pops.len()) {
                    self.place_point(place)
                } else {
                    Err(format!(
                        "place `{place}` has no pop {slot} at this revision"
                    ))
                }
            }
            Thing::Character(id) => {
                let card = view
                    .characters
                    .card(*id)
                    .ok_or_else(|| format!("no character {id} at this revision"))?;
                self.place_point(&card.place)
            }
            Thing::River(id) => self
                .rivers
                .get(id)
                .copied()
                .ok_or_else(|| format!("no river `{id}`")),
        }
    }
}

/// The thing the UI's `kind` and `id` arguments name.
fn thing_of(kind: &str, id: &ConfigValue) -> Result<Thing, String> {
    let text = |what: &str| match id {
        ConfigValue::String(text) => Ok(text.clone()),
        _ => Err(format!("a {what} is named by a string id")),
    };
    match kind {
        "location" => text("location").map(Thing::Location),
        "river" => text("river").map(Thing::River),
        "character" => match id {
            ConfigValue::Integer(id) => u64::try_from(*id)
                .map(Thing::Character)
                .map_err(|_| format!("no character {id}")),
            _ => Err("a character is named by an integer id".to_owned()),
        },
        "pop" => {
            let ConfigValue::Table(fields) = id else {
                return Err("a pop is named by a table {place, slot}".to_owned());
            };
            let (Some(ConfigValue::String(place)), Some(ConfigValue::Integer(slot))) =
                (fields.get("place"), fields.get("slot"))
            else {
                return Err("a pop is named by a table {place, slot}".to_owned());
            };
            let slot =
                u32::try_from(*slot).map_err(|_| format!("place `{place}` has no pop {slot}"))?;
            Ok(Thing::Pop(place.clone(), slot))
        }
        other => Err(format!("unknown kind `{other}`")),
    }
}

/// `game.go_to(kind, id, zoom, revision)`: moves the map camera so that the thing is at the centre of the view, resolving the thing under
/// `revision`, the one the UI holds. `kind` is `"location"` (a place id), `"pop"` (a table `{place, slot}`), `"character"` (an id) or
/// `"river"` (a river id, its mouth). With `zoom` true the step becomes the go-to step (a closer camera stays closer); with it false the
/// step is kept. A revision that is retired, missing or of a replaced world, an unknown kind or id, a slot the place does not hold and a
/// character that is not live there are errors, and the camera is not moved.
pub(crate) fn go_to_function(
    cache: Arc<ViewCache>,
    map: Option<SharedController>,
    targets: Arc<Targets>,
) -> HostFunction {
    Arc::new(move |arguments| {
        let [
            Some(ConfigValue::String(kind)),
            Some(id),
            Some(ConfigValue::Bool(zoom)),
            Some(ConfigValue::Integer(revision)),
        ] = arguments
        else {
            return Err(
                "game.go_to: expected a kind, an id, a zoom flag and a revision".to_owned(),
            );
        };
        let revision = u64::try_from(*revision)
            .map_err(|_| format!("game.go_to: revision {revision} is missing"))?;
        let view = cache.get_in_world("game.go_to", revision)?;
        let thing = thing_of(kind, id).map_err(|message| format!("game.go_to: {message}"))?;
        let point = targets
            .resolve(&thing, &view)
            .map_err(|message| format!("game.go_to: {message}"))?;
        let Some(map) = &map else {
            return Err("game.go_to: there is no map".to_owned());
        };
        crate::map_app::lock(map)
            .go_to(point, *zoom)
            .map_err(|message| format!("game.go_to: {message}"))?;
        Ok(Vec::new())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::character_view::{CharacterCard, CharacterView};
    use crate::game::view::{PlaceView, PopView};
    use ck_render_2d::{Filter, SceneBridge};
    use ck_window::WindowDesc;

    fn scenario() -> Scenario {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/scenarios/ireland-768");
        ck_map::load_scenario(&directory).expect("the shipped scenario loads")
    }

    /// The middle of the cell that holds the authored point, found from the point's latitude and longitude alone.
    fn cell_centre(latitude: f64, longitude: f64) -> FixedPoint {
        let cell = ck_map::cell_of(ck_map::project(latitude, longitude).expect("projectable"));
        FixedPoint {
            x: cell.x * 32_768 + 16_384,
            y: cell.y * 32_768 + 16_384,
        }
    }

    fn pop() -> PopView {
        PopView {
            jobs: Vec::new(),
            stock: Vec::new(),
            room_ml: 0,
            kind: "peasant".to_owned(),
            culture: "ulaid".to_owned(),
            religion: "christianity".to_owned(),
            workers: 80,
            dependents: 40,
            employed: 70,
            unemployed: 10,
            food_days: 215,
            food: Vec::new(),
            necessities: 0,
            niceties: 0,
            luxuries: 0,
            satisfaction: 0,
            equilibrium: 0,
            births: 0,
            deaths: 0,
            basic_housing: 0,
            makeshift_housing: 0,
            crowding_pm: None,
        }
    }

    /// A view whose places each hold `pops` pops, and whose characters stand at `(id, place, slot)`.
    fn view(places: &[(&str, usize)], characters: &[(u64, &str, u32)]) -> Arc<EconomyView> {
        let mut view = CharacterView::default();
        view.cards = characters
            .iter()
            .map(|(id, place, slot)| CharacterCard {
                id: *id,
                place: (*place).to_owned(),
                slot: *slot,
                ..CharacterCard::default()
            })
            .collect();
        Arc::new(EconomyView {
            places: places
                .iter()
                .map(|(id, pops)| PlaceView {
                    id: (*id).to_owned(),
                    pops: vec![pop(); *pops],
                    ..PlaceView::default()
                })
                .collect(),
            characters: view,
        })
    }

    struct Rig {
        cache: Arc<ViewCache>,
        map: SharedController,
        go_to: HostFunction,
    }

    impl Rig {
        fn new() -> Self {
            let scenario = scenario();
            let mut controller = crate::map_app::MapController::from_scenario(
                &scenario,
                SceneBridge::new(Filter::Nearest),
                4,
                20,
                24,
            )
            .expect("the map lane accepts the shipped scenario");
            controller
                .open(&WindowDesc {
                    title: "go-to test".to_owned(),
                    width: 1280,
                    height: 800,
                    clear: [0.0; 3],
                    ..WindowDesc::default()
                })
                .expect("the initial view is valid");
            let map = crate::map_app::shared(controller);
            let cache = Arc::new(ViewCache::default());
            let targets = Arc::new(Targets::from_scenario(&scenario));
            let go_to = go_to_function(Arc::clone(&cache), Some(Arc::clone(&map)), targets);
            Self { cache, map, go_to }
        }

        /// Posts `view` as `revision` of `world`, as the UI link does for each `game_time`.
        fn post(&self, revision: u64, world: u64, view: Arc<EconomyView>) {
            self.cache.post(revision, view);
            self.cache.note_world(revision, world);
        }

        fn camera(&self) -> ck_map::Camera {
            crate::map_app::lock(&self.map)
                .camera()
                .expect("an opened camera")
        }

        fn call(
            &self,
            kind: &str,
            id: ConfigValue,
            zoom: bool,
            revision: i64,
        ) -> Result<Vec<ConfigValue>, String> {
            (self.go_to)(&[
                Some(ConfigValue::String(kind.to_owned())),
                Some(id),
                Some(ConfigValue::Bool(zoom)),
                Some(ConfigValue::Integer(revision)),
            ])
        }

        /// A call that must be refused: the refusal, with the camera shown to be unmoved.
        fn refused(&self, kind: &str, id: ConfigValue, zoom: bool, revision: i64) -> String {
            let before = self.camera();
            let refusal = self
                .call(kind, id, zoom, revision)
                .expect_err("the call is refused");
            assert_eq!(self.camera(), before, "a refusal moves nothing");
            refusal
        }
    }

    fn text(value: &str) -> ConfigValue {
        ConfigValue::String(value.to_owned())
    }

    fn pop_id(place: &str, slot: i64) -> ConfigValue {
        ConfigValue::Table(BTreeMap::from([
            ("place".to_owned(), text(place)),
            ("slot".to_owned(), ConfigValue::Integer(slot)),
        ]))
    }

    /// Armagh's authored point (54.3489 N, 6.6545 W) in the shipped scenario.
    const ARMAGH: (f64, f64) = (54.3489, -6.6545);

    #[test]
    fn a_location_go_to_centres_on_its_cell_keeps_the_step_without_zoom_and_sets_the_go_to_step_with_it()
     {
        let rig = Rig::new();
        rig.post(1, 0, view(&[], &[]));
        let before = rig.camera();
        assert!(before.step < crate::map_app::GO_TO_STEP);
        rig.call("location", text("armagh"), false, 1)
            .expect("armagh");
        let moved = rig.camera();
        assert_eq!(moved.center, cell_centre(ARMAGH.0, ARMAGH.1));
        assert_eq!(moved.step, before.step, "no zoom keeps the step");
        rig.call("location", text("armagh"), true, 1)
            .expect("armagh");
        assert_eq!(rig.camera().step, crate::map_app::GO_TO_STEP);
        assert_eq!(rig.camera().center, cell_centre(ARMAGH.0, ARMAGH.1));
    }

    #[test]
    fn a_pop_go_to_needs_its_slot_at_that_place_in_that_revision() {
        let rig = Rig::new();
        rig.post(1, 0, view(&[("armagh", 2)], &[]));
        for slot in [0, 1] {
            rig.call("pop", pop_id("armagh", slot), false, 1)
                .unwrap_or_else(|error| panic!("slot {slot}: {error}"));
            assert_eq!(rig.camera().center, cell_centre(ARMAGH.0, ARMAGH.1));
        }
        let invalid = rig.camera();
        assert!(
            rig.refused("pop", pop_id("armagh", 2), false, 1)
                .contains("has no pop 2")
        );
        assert!(
            rig.refused("pop", pop_id("armagh", -1), false, 1)
                .contains("has no pop -1")
        );
        assert!(
            rig.refused("pop", pop_id("derry", 0), false, 1)
                .contains("no place `derry` at this revision"),
            "a place the revision does not hold"
        );
        assert!(
            rig.refused("pop", text("armagh"), false, 1)
                .contains("{place, slot}")
        );
        assert_eq!(rig.camera(), invalid);
    }

    #[test]
    fn a_character_go_to_goes_to_the_place_of_its_pop_and_refuses_one_that_is_not_live() {
        let rig = Rig::new();
        rig.post(1, 0, view(&[], &[(7, "armagh", 0)]));
        rig.call("character", ConfigValue::Integer(7), true, 1)
            .expect("character 7");
        assert_eq!(rig.camera().center, cell_centre(ARMAGH.0, ARMAGH.1));
        assert!(
            rig.refused("character", ConfigValue::Integer(99), true, 1)
                .contains("no character 99 at this revision")
        );
        assert!(
            rig.refused("character", ConfigValue::Integer(-1), true, 1)
                .contains("no character -1")
        );
        assert!(
            rig.refused("character", text("7"), true, 1)
                .contains("integer id")
        );
    }

    #[test]
    fn a_go_to_and_its_character_use_the_revision_the_ui_holds_not_a_newer_one() {
        // The link has pinned revision 2 (the character has moved to Derry) but the UI still holds revision 1: the go-to names the held
        // one and lands on its place.
        let rig = Rig::new();
        rig.post(1, 0, view(&[], &[(7, "armagh", 0)]));
        rig.post(2, 0, view(&[], &[(7, "derry", 0)]));
        rig.call("character", ConfigValue::Integer(7), true, 1)
            .expect("character 7 at the held revision");
        assert_eq!(rig.camera().center, cell_centre(ARMAGH.0, ARMAGH.1));
        rig.call("character", ConfigValue::Integer(7), true, 2)
            .expect("character 7 at the newer revision");
        let derry = scenario()
            .locations
            .iter()
            .find(|location| location.id == "derry")
            .expect("derry")
            .center();
        assert_eq!(rig.camera().center, derry);
        assert_ne!(derry, cell_centre(ARMAGH.0, ARMAGH.1));
    }

    #[test]
    fn a_revision_that_is_retired_missing_or_of_a_replaced_world_is_refused_and_moves_nothing() {
        let rig = Rig::new();
        rig.post(1, 0, view(&[("armagh", 1)], &[(7, "armagh", 0)]));
        rig.post(2, 0, view(&[("armagh", 1)], &[(7, "armagh", 0)]));
        // A load: revision 3 is the first of world 1, so revisions 1 and 2 are of a replaced world while they are still held.
        rig.post(3, 1, view(&[("armagh", 1)], &[(7, "armagh", 0)]));
        for revision in [1, 2] {
            let refusal = rig.refused("location", text("armagh"), true, revision);
            assert!(refusal.contains("is of a replaced world"), "{refusal}");
            rig.refused("pop", pop_id("armagh", 0), true, revision);
            rig.refused("character", ConfigValue::Integer(7), true, revision);
        }
        rig.call("location", text("armagh"), false, 3)
            .expect("the current world");
        // Calls complete in order: with 3 complete the older views leave the cache and are retired.
        rig.cache.complete(1);
        rig.cache.complete(2);
        rig.cache.complete(3);
        let refusal = rig.refused("location", text("armagh"), true, 1);
        assert!(refusal.contains("revision 1 is retired"), "{refusal}");
        let refusal = rig.refused("location", text("armagh"), true, 9);
        assert!(refusal.contains("revision 9 is missing"), "{refusal}");
        let refusal = rig.refused("location", text("armagh"), true, -4);
        assert!(refusal.contains("revision -4 is missing"), "{refusal}");
    }

    #[test]
    fn an_unknown_kind_or_id_and_malformed_arguments_are_refused_and_move_nothing() {
        let rig = Rig::new();
        rig.post(1, 0, view(&[], &[]));
        assert!(
            rig.refused("merchant", text("x"), true, 1)
                .contains("unknown kind `merchant`")
        );
        assert!(
            rig.refused("location", text("atlantis"), true, 1)
                .contains("no place `atlantis`")
        );
        assert!(
            rig.refused("river", text("styx"), true, 1)
                .contains("no river `styx`")
        );
        assert!(
            rig.refused("location", ConfigValue::Integer(1), true, 1)
                .contains("string id")
        );
        let before = rig.camera();
        assert!((rig.go_to)(&[Some(text("location")), Some(text("armagh"))]).is_err());
        assert!(
            (rig.go_to)(&[
                Some(text("location")),
                Some(text("armagh")),
                Some(text("yes")),
                Some(ConfigValue::Integer(1)),
            ])
            .is_err(),
            "the zoom flag is a boolean"
        );
        assert_eq!(rig.camera(), before);
    }

    #[test]
    fn a_game_without_a_map_refuses_the_go_to() {
        let cache = Arc::new(ViewCache::default());
        cache.post(1, view(&[], &[]));
        let targets = Arc::new(Targets::from_scenario(&scenario()));
        let go_to = go_to_function(cache, None, targets);
        let refusal = go_to(&[
            Some(text("location")),
            Some(text("armagh")),
            Some(ConfigValue::Bool(true)),
            Some(ConfigValue::Integer(1)),
        ])
        .expect_err("no map");
        assert_eq!(refusal, "game.go_to: there is no map");
    }

    /// The tag of a ledger right-click (`go:<kind>:<id>:<revision>:<world>`, `lib/go_to.lua`) is built from real ids: every shipped place and
    /// river id, in the worst case of a pop's slot, a revision and a world of the widest numbers, fits the tag alphabet (1 to 128 printable
    /// ASCII characters, `render.set_hit_tag`) and the characters the Lua builder admits in an id; and a tag outside the alphabet is refused
    /// by the scene, not by a crash of the game (a hit *key* is a different, narrower alphabet: `[a-z0-9_]{1,48}`, which a tag must never be
    /// used as).
    #[test]
    fn the_tag_of_every_real_place_and_river_fits_the_tag_alphabet_in_the_worst_case() {
        let scenario = scenario();
        let ids: Vec<&str> = scenario
            .locations
            .iter()
            .map(|location| location.id.as_str())
            .chain(
                scenario
                    .map
                    .river_network
                    .iter()
                    .flat_map(|network| network.rivers.iter().map(|river| river.id.as_str())),
            )
            .collect();
        assert!(ids.len() > 125, "the places and the rivers: {}", ids.len());
        let widest = format!("{}", u64::MAX);
        for id in ids {
            for tag in [
                format!("go:location:{id}:{widest}:{widest}"),
                format!("go:pop:{id}#{}:{widest}:{widest}", u32::MAX),
                format!("go:river:{id}:{widest}:{widest}"),
            ] {
                assert!(ck_render_2d::valid_tag(&tag), "{tag}");
                assert!(
                    id.bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_#.-".contains(&byte)),
                    "{id} has a character the Lua tag builder refuses"
                );
            }
        }
        // The longest shipped place id, by name, with a pop slot (the `#` form), a character id and a world number: the widest tag any ledger
        // row can make, and the room that is left of the 128.
        let longest = scenario
            .locations
            .iter()
            .map(|location| location.id.as_str())
            .max_by_key(|id| id.len())
            .expect("places");
        assert_eq!(longest.len(), 22, "the longest shipped place id: {longest}");
        let widest_pop = format!("go:pop:{longest}#{}:{widest}:{widest}", u32::MAX);
        let widest_character = format!("go:character:{widest}:{widest}:{widest}");
        assert!(ck_render_2d::valid_tag(&widest_pop), "{widest_pop}");
        assert!(
            ck_render_2d::valid_tag(&widest_character),
            "{widest_character}"
        );
        assert!(
            widest_pop.len() <= 128 - 20,
            "{} characters leave room",
            widest_pop.len()
        );
        // Real small values too: a pop slot of each place, a character id, the world numbers a session reaches.
        for (place, slot, character, world, revision) in [
            ("armagh", 0_u32, 7_u64, 0_u64, 1_u64),
            (longest, 3, 4096, 12, 1_795_356),
            ("clonmacnoise", 1, 90, 3, 22),
        ] {
            for tag in [
                format!("go:pop:{place}#{slot}:{revision}:{world}"),
                format!("go:character:{character}:{revision}:{world}"),
                format!("go:location:{place}:{revision}:{world}"),
            ] {
                assert!(ck_render_2d::valid_tag(&tag), "{tag}");
                assert!(
                    !ck_render_2d::valid_key(&tag),
                    "a tag is never a valid hit key: {tag}"
                );
            }
        }
        for tag in [
            "",
            "go:location:has space:1:0",
            "go:location:caf\u{e9}:1:0",
            &format!("go:location:{}:1:0", "a".repeat(120)),
        ] {
            assert!(!ck_render_2d::valid_tag(tag), "{tag:?}");
        }
        // A colon is fine in a tag, and a tag is never a valid hit key.
        assert!(ck_render_2d::valid_tag("go:location:rowan:1:0"));
        assert!(!ck_render_2d::valid_key("go:location:rowan:1:0"));
    }

    /// The four rivers of the review (spec r2 section 1), each by the authored last point of the stretch the rule chooses. The expected
    /// points are the authored latitude and longitude of that stretch's end, projected here, not read back from the rule.
    #[test]
    fn a_river_go_to_goes_to_its_mouth_by_the_rule_on_the_shipped_network() {
        let rig = Rig::new();
        rig.post(1, 0, view(&[], &[]));
        let cases = [
            // Erne: `erne-22` joins `arney-02`, `erne-46` reaches the sea (and is also the last id): the sea.
            ("erne", 54.52399, -8.29137),
            // Laune: two sea exits, `laune-02` and `laune-03`; the last id wins.
            ("laune", 52.14609, -9.88143),
            // Swilly: two sea exits, `swilly-08` and `swilly-09`; the last id wins.
            ("swilly", 55.09730, -7.50753),
            // Main, a tributary: it ends where `main-06` joins `bann-09`, not at the Bann's mouth.
            ("main", 54.71701, -6.29704),
        ];
        for (river, latitude, longitude) in cases {
            rig.call("river", text(river), false, 1)
                .unwrap_or_else(|error| panic!("{river}: {error}"));
            assert_eq!(
                rig.camera().center,
                ck_map::project(latitude, longitude).expect("projectable"),
                "{river}"
            );
        }
        // The losing exits are elsewhere: Laune's `laune-02` ends at 52.14490 N 9.82807 W.
        assert_ne!(
            rig.camera().center,
            ck_map::project(52.14490, -9.82807).expect("projectable")
        );
    }

    /// The shipped 768 map: one named location, one character and one named river land on their expected points.
    #[test]
    fn on_the_shipped_map_a_location_a_character_and_a_river_land_on_their_points() {
        let rig = Rig::new();
        rig.post(1, 0, view(&[("armagh", 1)], &[(3, "armagh", 0)]));
        rig.call("location", text("armagh"), true, 1)
            .expect("location");
        let location = rig.camera().center;
        assert_eq!(location, cell_centre(ARMAGH.0, ARMAGH.1));
        rig.call("river", text("erne"), true, 1).expect("river");
        let river = rig.camera().center;
        assert_eq!(
            river,
            ck_map::project(54.52399, -8.29137).expect("projectable")
        );
        assert_ne!(river, location);
        rig.call("character", ConfigValue::Integer(3), true, 1)
            .expect("character");
        assert_eq!(rig.camera().center, location);
    }
}
