//! Connections and passions (CK-368, characters slice 6): orientation, contact, the circle, the yearly strand rolls, fade, the
//! happenings that make strands, passions, the replays, the lover and namesake hand-offs, opinion terms and log lines.
//!
//! Characters whose exact state a case needs are built as records and loaded through the canonical encoding. The events are none (a
//! quiet world). A roll is checked against the stream it names or forced by the data (a chance of 1), so an outcome is exact, not
//! statistical, except where a rate over a large sample is the subject.

use ck_characters::{
    Archive, ArchiveSink, Character, CharacterStore, Context, GeneticRecord, LifeLog, LifeRecord,
    Lineage, NewCharacter, Office, Orientation, Passion, PassionTarget, PersonalityRecord,
    PopLocator, Populations, Sex, UNKNOWN, World, character_stream, genetics::succeeds,
    ties::Strand,
};
use ck_economy::{EventRules, LifeCycle, PopGenetics, Rules, events::Registry};
use ck_sim::{Canonical, Date, GameTime};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

const PPB: u32 = 1_000_000_000;

fn date(year: u16, month: u8, day: u8) -> GameTime {
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour: 0,
    })
    .expect("valid date")
}
/// The day the yearly rolls of these tests are made: the birthday of every character built by [`person`].
fn today() -> GameTime {
    date(768, 3, 3)
}
fn locator(place: &str, slot: u32) -> PopLocator {
    PopLocator {
        place: place.into(),
        slot,
    }
}
fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        Rules::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
        )
        .expect("shipped rules")
    })
}
/// No events at all: nothing but the life cycle and the connections writes a line or changes a character.
fn quiet() -> &'static EventRules {
    static QUIET: OnceLock<EventRules> = OnceLock::new();
    QUIET.get_or_init(|| {
        let pop_types: Vec<&str> = rules().pop_types.iter().map(|p| p.id.as_str()).collect();
        EventRules::parse(
            "character-events.toml",
            "",
            "attributes-skills.toml",
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../assets/game/events/attributes-skills.toml"),
            )
            .expect("shipped data"),
            &Registry {
                peoples: &rules().peoples,
                genetics: &rules().genetics,
                pop_types: &pop_types,
            },
        )
        .expect("an events file with no events")
    })
}
/// An event file with `text` and the shipped attributes.
fn events_from(text: &str) -> &'static EventRules {
    let pop_types: Vec<&str> = rules().pop_types.iter().map(|p| p.id.as_str()).collect();
    Box::leak(Box::new(
        EventRules::parse(
            "character-events.toml",
            text,
            "attributes-skills.toml",
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../assets/game/events/attributes-skills.toml"),
            )
            .expect("shipped data"),
            &Registry {
                peoples: &rules().peoples,
                genetics: &rules().genetics,
                pop_types: &pop_types,
            },
        )
        .expect("events"),
    ))
}
fn irish() -> PopGenetics {
    rules().genetics.seed_pop("ulaid").expect("Irish start set")
}
/// The shipped data with no marriage wish (a birthday arranges no marriage) and every base chance 0 except those of `kinds`, which are
/// certain wherever the pair's contact is 1 and the factors are neutral: the household's contact is 1 and the passions never roll.
fn certain(kinds: &[&str]) -> LifeCycle {
    let mut data = rules().lifecycle.clone();
    data.marriage.wish.men = vec![(12, 0)];
    data.marriage.wish.women = vec![(12, 0)];
    let rules = &mut data.connections;
    for (kind, base) in &mut rules.strands.base_ppb {
        *base = if kinds.contains(&kind.as_str()) {
            PPB
        } else {
            0
        };
    }
    rules.strands.hatred_chance_ppb = 0;
    rules
        .settings
        .get_mut("household")
        .expect("household")
        .contact_ppb = PPB;
    for moment in &mut rules.passions.moments {
        moment.chance_ppb = 0;
    }
    data
}

/// The pops: every place is a Christian noble Irish pop (`ulaid`) with room for a million, unless a case names a pop's type.
struct Pops {
    kinds: BTreeMap<(&'static str, u32), &'static str>,
    /// Whether the source has display names for passion targets (the economy has none).
    named: bool,
}
impl Pops {
    fn new() -> Self {
        Self {
            kinds: BTreeMap::new(),
            named: false,
        }
    }
    fn named(mut self) -> Self {
        self.named = true;
        self
    }
    fn kind(mut self, place: &'static str, slot: u32, kind: &'static str) -> Self {
        self.kinds.insert((place, slot), kind);
        self
    }
}
impl Populations for Pops {
    fn target_name(&self, kind: &str, key: &str) -> Option<String> {
        self.named.then(|| format!("the {kind} of {key}"))
    }
    fn heads(&self, _: &PopLocator) -> Option<u64> {
        Some(1_000_000)
    }
    fn culture(&self, _: &PopLocator) -> Option<String> {
        Some("ulaid".to_owned())
    }
    fn kind(&self, locator: &PopLocator) -> Option<String> {
        Some(
            self.kinds
                .get(&(locator.place.as_str(), locator.slot))
                .copied()
                .unwrap_or("noble")
                .to_owned(),
        )
    }
    fn luxuries_pm(&self, _: &PopLocator) -> Option<u16> {
        Some(600)
    }
    fn religion(&self, _: &PopLocator) -> Option<String> {
        Some("christianity".to_owned())
    }
    fn necessities_pm(&self, _: &PopLocator) -> Option<u16> {
        Some(1000)
    }
    fn genetics(&self, _: &PopLocator) -> Option<PopGenetics> {
        Some(irish())
    }
    fn distance_m(&self, a: &PopLocator, b: &PopLocator) -> Option<u64> {
        (a.place == b.place).then_some(0)
    }
    fn move_head(
        &self,
        _: &PopLocator,
        _: &PopLocator,
        _: bool,
    ) -> Result<ck_economy::MoveUndo, ck_characters::Error> {
        Ok(ck_economy::MoveUndo::default())
    }
    fn restore_move(&self, _: &ck_economy::MoveUndo) {}
}

fn path(tag: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "ck-connections-{}-{tag}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::remove_file(&path).ok();
    path
}
/// An archive and a life log of one test.
struct Env {
    archive: Archive,
    logs: LifeLog,
}
impl Env {
    fn new(tag: &str) -> Self {
        Self {
            archive: Archive::open(path(&format!("{tag}-arch"))).expect("archive"),
            logs: LifeLog::open(path(&format!("{tag}-logs"))).expect("logs"),
        }
    }
    fn lines(&self, id: u64) -> Vec<String> {
        self.logs
            .read(id)
            .expect("log")
            .into_iter()
            .map(|line| line.text)
            .collect()
    }
}
fn world<'a, P: Populations>(
    lifecycle: &'a LifeCycle,
    env: &'a mut Env,
    pops: &'a P,
    seed: u64,
) -> World<'a, P, Archive, LifeLog> {
    World {
        rules: quiet(),
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle,
        seed,
        populations: pops,
        archive: &mut env.archive,
        logs: &mut env.logs,
    }
}

/// A record with ordinary stats and a neutral personality in pop `place`, slot 0, with its birthday on 3 March.
fn person(id: u64, age: u16, place: &str, sex: Sex) -> Character {
    let mut genetics = GeneticRecord::default();
    genetics.value[9..13].copy_from_slice(&[5000; 4]);
    for stat in 0..9 {
        genetics.value[stat] = 10_000;
    }
    genetics.value[1] = 70_000;
    let mut life = LifeRecord::default();
    "irish".clone_into(&mut life.pool);
    Character {
        id,
        name: format!("c{id}"),
        sex,
        birth: date(768 - age, 3, 3),
        death: None,
        pop: locator(place, 0),
        father: UNKNOWN,
        mother: UNKNOWN,
        spouses: Vec::new(),
        children: Vec::new(),
        office: None,
        // Full tier unless a case makes a character coarse: a coarse character rolls only with full-tier partners.
        full_until: Some(date(900, 1, 1)),
        genetics: Box::new(genetics),
        personality: PersonalityRecord::default(),
        life: Box::new(life),
        ties: Box::default(),
    }
}
fn man(id: u64, age: u16, place: &str) -> Character {
    person(id, age, place, Sex::Male)
}
fn woman(id: u64, age: u16, place: &str) -> Character {
    person(id, age, place, Sex::Female)
}
fn housed(mut c: Character, house: &str) -> Character {
    house.clone_into(&mut c.life.house);
    c
}
fn in_slot(mut c: Character, slot: u32) -> Character {
    c.pop.slot = slot;
    c
}
fn ruler(mut c: Character) -> Character {
    c.office = Some(Office::Ruler);
    c
}
fn coarse(mut c: Character) -> Character {
    c.full_until = None;
    c
}
fn gay(mut c: Character) -> Character {
    c.ties.orientation = Orientation::Gay;
    c
}
fn store_of(characters: Vec<Character>) -> CharacterStore {
    let next = characters.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    let live: BTreeMap<u64, Character> = characters.into_iter().map(|c| (c.id, c)).collect();
    let mut bytes = Vec::new();
    next.encode(&mut bytes).expect("encode");
    live.encode(&mut bytes).expect("encode");
    0_u64.encode(&mut bytes).expect("encode");
    BTreeMap::<String, String>::new()
        .encode(&mut bytes)
        .expect("encode");
    CharacterStore::from_bytes(&bytes).expect("a store of these records")
}
fn get(store: &CharacterStore, id: u64) -> &Character {
    store.get(id).expect("live")
}
fn reloaded(store: &CharacterStore) -> CharacterStore {
    CharacterStore::from_bytes(&store.to_bytes().expect("encode")).expect("decode")
}
/// The roll a stream gives for chance `p`, as the engine takes it.
fn rolls(seed: u64, id: u64, purpose: &str, tick: GameTime, p: u32) -> bool {
    let draw = character_stream(seed, id, purpose, tick)
        .below(u64::from(PPB))
        .expect("a draw");
    succeeds(draw, p)
}
fn birthdays(
    store: &mut CharacterStore,
    lifecycle: &LifeCycle,
    env: &mut Env,
    pops: &Pops,
    day: GameTime,
) {
    store
        .run_birthdays(&mut world(lifecycle, env, pops, 7), day)
        .expect("birthdays");
}

// --- Data and orientation ---------------------------------------------------------------------------------------------------

fn context(lifecycle: &LifeCycle) -> Context<'_> {
    Context {
        rules: &rules().genetics,
        personality: &rules().personality,
        events: &rules().events,
        lifecycle,
        seed: 7,
        tick: date(768, 1, 1),
    }
}
fn newborn(place: &str, sex: Sex) -> NewCharacter {
    NewCharacter {
        name: String::new(),
        sex,
        birth: date(768, 1, 1),
        pop: locator(place, 0),
        house: String::new(),
    }
}

#[test]
fn orientation_is_rolled_at_birth_by_the_weights_and_a_gay_or_bisexual_parent_boosts_gay_and_bisexual()
 {
    let lifecycle = &rules().lifecycle;
    let pops = Pops::new();
    let mut store = CharacterStore::default();
    let count = |store: &CharacterStore, which: Orientation, ids: &[u64]| {
        ids.iter()
            .filter(|id| get(store, **id).ties.orientation == which)
            .count()
    };
    // Founders: 92 straight, 4 gay, 3 bisexual, 1 asexual of 100.
    let founders: Vec<u64> = (0..1500)
        .map(|_| {
            store
                .create_founder(
                    &context(lifecycle),
                    newborn("a", Sex::Male),
                    &irish(),
                    &pops,
                )
                .expect("founder")
        })
        .collect();
    let total = founders.len();
    let near = |got: usize, per_cent: usize| {
        let want = total * per_cent / 100;
        eprintln!("{got} of {total}, want {want}");
        got.abs_diff(want) * 100 <= total * 3
    };
    assert!(near(count(&store, Orientation::Straight, &founders), 92));
    assert!(near(count(&store, Orientation::Gay, &founders), 4));
    assert!(near(count(&store, Orientation::Bisexual, &founders), 3));
    assert!(near(count(&store, Orientation::Asexual, &founders), 1));
    // Children of a gay father and a straight mother: gay and bisexual weights x 1.5, so gay 6 / 101.5 and bisexual 4.5 / 101.5.
    let father = founders
        .iter()
        .copied()
        .find(|id| get(&store, *id).ties.orientation == Orientation::Gay)
        .expect("a gay founder");
    let mother = store
        .create_founder(
            &context(lifecycle),
            newborn("a", Sex::Female),
            &irish(),
            &pops,
        )
        .expect("founder");
    let env = Env::new("orientation");
    let children: Vec<u64> = (0..1500)
        .map(|_| {
            store
                .state_create_child(
                    &context(lifecycle),
                    newborn("a", Sex::Male),
                    &irish(),
                    &Lineage {
                        father,
                        mother,
                        archive: &env.archive,
                    },
                    &pops,
                )
                .expect("child")
        })
        .collect();
    let gay_children = count(&store, Orientation::Gay, &children);
    let bi_children = count(&store, Orientation::Bisexual, &children);
    // The boosted shares (5.9% and 4.4%) are well above the unboosted ones (4% and 3%).
    assert!(
        gay_children * 100 > children.len() * 5,
        "gay {gay_children}"
    );
    assert!(
        bi_children * 100 > children.len() * 4 - children.len() / 2,
        "bisexual {bi_children}"
    );
    // It never changes: a birthday leaves it as it was.
    let mut env = Env::new("orientation-birthday");
    let before = get(&store, children[0]).ties.orientation;
    let mut world = world(lifecycle, &mut env, &pops, 7);
    store
        .run_birthdays(&mut world, date(769, 1, 1))
        .expect("birthdays");
    assert_eq!(get(&store, children[0]).ties.orientation, before);
}

#[test]
fn a_new_character_records_when_it_joined_its_place_and_its_pop() {
    let lifecycle = &rules().lifecycle;
    let pops = Pops::new();
    let mut store = CharacterStore::default();
    let id = store
        .create_founder(
            &context(lifecycle),
            newborn("a", Sex::Male),
            &irish(),
            &pops,
        )
        .expect("founder");
    let c = get(&store, id);
    assert_eq!(c.ties.location_since, date(768, 1, 1));
    assert_eq!(c.ties.in_pop_since, date(768, 1, 1));
    assert_eq!(c.ties.office_since, None);
}

#[test]
fn the_office_marker_dates_only_a_different_value_and_a_move_dates_the_pop_and_the_place() {
    let lifecycle = &rules().lifecycle;
    let pops = Pops::new();
    let mut env = Env::new("since");
    let mut first = man(1, 40, "a");
    first.ties.location_since = date(750, 1, 1);
    first.ties.in_pop_since = date(751, 1, 1);
    let mut store = store_of(vec![first]);
    store
        .set_office(1, Some(Office::Ruler), date(760, 1, 1))
        .expect("office");
    assert_eq!(get(&store, 1).ties.office_since, Some(date(760, 1, 1)));
    store
        .set_office(1, Some(Office::Ruler), date(765, 1, 1))
        .expect("same marker");
    assert_eq!(get(&store, 1).ties.office_since, Some(date(760, 1, 1)));
    store
        .set_office(1, Some(Office::Heir), date(766, 1, 1))
        .expect("a different one");
    assert_eq!(get(&store, 1).ties.office_since, Some(date(766, 1, 1)));
    let mut world = world(lifecycle, &mut env, &pops, 7);
    // A move to another slot of the same place dates the pop only; one to another place dates both.
    store
        .move_to_with_events(&mut world, 1, locator("a", 1), date(767, 1, 1))
        .expect("move");
    assert_eq!(get(&store, 1).ties.in_pop_since, date(767, 1, 1));
    // The place did not change: its date stays the one it had.
    assert_eq!(get(&store, 1).ties.location_since, date(750, 1, 1));
    let before = get(&store, 1).ties.location_since;
    store
        .move_to_with_events(&mut world, 1, locator("b", 0), date(767, 6, 1))
        .expect("move");
    assert_eq!(get(&store, 1).ties.in_pop_since, date(767, 6, 1));
    assert_eq!(get(&store, 1).ties.location_since, date(767, 6, 1));
    assert_ne!(before, date(767, 6, 1));
}

// --- Contact -----------------------------------------------------------------------------------------------------------------

fn contact(store: &CharacterStore, lifecycle: &LifeCycle, pops: &Pops, a: u64, b: u64) -> u32 {
    let mut env = Env::new("contact");
    let world = world(lifecycle, &mut env, pops, 7);
    store.contact_ppb(&world, a, b).expect("contact")
}

#[test]
fn contact_is_the_highest_shared_live_setting_plus_the_extra_for_each_further_one() {
    let lifecycle = &rules().lifecycle;
    let pops = Pops::new()
        .kind("m", 0, "monk")
        .kind("c", 0, "peasant")
        .kind("c", 1, "peasant");
    let store = store_of(vec![
        // household: the same pop and the same non-empty house.
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "h"), "ui_neill"),
        // not a household: unrelated and houseless in one pop.
        man(3, 30, "h"),
        man(4, 30, "h"),
        // a household of close kin without a house: parent and child.
        man(5, 50, "k"),
        {
            let mut son = man(6, 20, "k");
            son.father = 5;
            son
        },
        // monastery: the same monk pop (and no house, no kin).
        man(7, 30, "m"),
        man(8, 30, "m"),
        // same_location: the same place, different pops (peasants, so no court).
        man(9, 30, "c"),
        in_slot(man(10, 30, "c"), 1),
        // somewhere else.
        man(11, 30, "z"),
    ]);
    // household 0.9.
    assert_eq!(contact(&store, lifecycle, &pops, 1, 2), 900_000_000);
    // unrelated houseless characters in one pop are not a household: no setting at all (the pop is a noble one but has no ruler).
    assert_eq!(contact(&store, lifecycle, &pops, 3, 4), 0);
    // parent and child.
    assert_eq!(contact(&store, lifecycle, &pops, 5, 6), 900_000_000);
    // monastery 0.6.
    assert_eq!(contact(&store, lifecycle, &pops, 7, 8), 600_000_000);
    // same_location 0.1.
    assert_eq!(contact(&store, lifecycle, &pops, 9, 10), 100_000_000);
    // another place: none.
    assert_eq!(contact(&store, lifecycle, &pops, 1, 11), 0);
}

#[test]
fn two_nobles_in_one_noble_pop_with_a_ruler_in_another_are_a_court_and_the_extra_setting_caps_at_one()
 {
    let lifecycle = &rules().lifecycle;
    let pops = Pops::new();
    let store = store_of(vec![
        // Two nobles in noble pop slot 0 of place "k", and a ruler in slot 1 of the same place.
        man(1, 30, "k"),
        man(2, 30, "k"),
        ruler(in_slot(man(3, 50, "k"), 1)),
        // The same pair at a place with no ruler: no court.
        man(4, 30, "q"),
        man(5, 30, "q"),
        // A house and a court at once: 0.9 + 0.05.
        housed(man(6, 30, "r"), "ui_neill"),
        housed(man(7, 30, "r"), "ui_neill"),
        ruler(in_slot(man(8, 50, "r"), 1)),
    ]);
    assert_eq!(contact(&store, lifecycle, &pops, 1, 2), 500_000_000);
    assert_eq!(contact(&store, lifecycle, &pops, 4, 5), 0);
    assert_eq!(contact(&store, lifecycle, &pops, 6, 7), 950_000_000);
    // The extra setting never takes the contact past 1.
    let mut capped = lifecycle.clone();
    capped.connections.extra_setting_ppb = 900_000_000;
    assert_eq!(contact(&store, &capped, &pops, 6, 7), PPB);
}

#[test]
fn an_inert_setting_gives_no_contact() {
    let mut lifecycle = rules().lifecycle.clone();
    lifecycle
        .connections
        .settings
        .get_mut("household")
        .expect("household")
        .live = false;
    let pops = Pops::new();
    let store = store_of(vec![
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "h"), "ui_neill"),
    ]);
    assert_eq!(contact(&store, &lifecycle, &pops, 1, 2), 0);
}

// --- The circle ----------------------------------------------------------------------------------------------------------------

#[test]
fn the_circle_holds_the_closest_up_to_its_size_by_contact_then_id_and_ignores_other_locations() {
    let mut lifecycle = rules().lifecycle.clone();
    lifecycle.connections.circle.size = 3;
    let pops = Pops::new();
    let mut env = Env::new("circle");
    // 1 is the subject; 2, 3, 4 are household (0.9), 5, 6 are same_location (0.1); 7 is a child too young to count (age 5).
    let mut characters = vec![housed(man(1, 40, "h"), "ui_neill")];
    for id in [4, 2, 3] {
        characters.push(housed(man(id, 40, "h"), "ui_neill"));
    }
    for id in [6, 5] {
        characters.push(in_slot(man(id, 40, "h"), 1));
    }
    characters.push(housed(man(7, 5, "h"), "ui_neill"));
    let store = store_of(characters);
    let world = world(&lifecycle, &mut env, &pops, 7);
    assert_eq!(
        store.circle_of(&world, 1, today()).expect("circle"),
        vec![2, 3, 4]
    );
    // With room for more, the same-location pair follows the household, by id.
    let mut wide = lifecycle.clone();
    wide.connections.circle.size = 20;
    let mut env = Env::new("circle-wide");
    let world = ck_characters_world(&wide, &mut env, &pops);
    assert_eq!(
        store.circle_of(&world, 1, today()).expect("circle"),
        vec![2, 3, 4, 5, 6]
    );
}
fn ck_characters_world<'a>(
    lifecycle: &'a LifeCycle,
    env: &'a mut Env,
    pops: &'a Pops,
) -> World<'a, Pops, Archive, LifeLog> {
    world(lifecycle, env, pops, 7)
}

#[test]
fn candidate_discovery_does_not_grow_when_characters_are_added_at_an_unrelated_location() {
    let lifecycle = &rules().lifecycle;
    let pops = Pops::new();
    let mut env = Env::new("discovery");
    let mut characters: Vec<Character> = (1..=5)
        .map(|id| housed(man(id, 40, "h"), "ui_neill"))
        .collect();
    let small = store_of(characters.clone());
    characters.extend((100..400).map(|id| man(id, 40, "elsewhere")));
    let large = store_of(characters);
    let work = |store: &CharacterStore, env: &mut Env| {
        store.reset_candidates_visited();
        let world = ck_characters_world(lifecycle, env, &pops);
        store.circle_of(&world, 1, today()).expect("circle");
        store.candidates_visited()
    };
    // The five at the location, once to find the candidates and once to look for a ruler among them.
    assert_eq!(work(&small, &mut env), 10);
    assert_eq!(work(&large, &mut env), work(&small, &mut env));
}

// --- Helpers for the strands -------------------------------------------------------------------------------------------------

fn kind_at(kind: &str) -> usize {
    ck_economy::events::ALL_KINDS
        .iter()
        .position(|known| *known == kind)
        .expect("a strand kind")
}
fn find(characters: &mut [Character], id: u64) -> &mut Character {
    characters
        .iter_mut()
        .find(|c| c.id == id)
        .expect("a character")
}
/// A bond of `kind` between `a` and `b`, stored on both, last grown in `grew`.
fn bond(characters: &mut [Character], a: u64, b: u64, kind: &str, strength: u8, grew: u16) {
    for (x, y) in [(a, b), (b, a)] {
        let link = find(characters, x).ties.links.entry(y).or_default();
        link.strands[kind_at(kind)] = Strand {
            strength,
            grew,
            faded: 0,
        };
    }
}
fn born_on(mut c: Character, month: u8, day: u8) -> Character {
    let year = c.birth.date().expect("date").year;
    c.birth = date(year, month, day);
    c
}
fn strength(store: &CharacterStore, holder: u64, partner: u64, kind: &str) -> u8 {
    get(store, holder).ties.strength(partner, kind)
}
fn manner_index(name: &str) -> usize {
    ck_economy::personality::MANNER_PAIRS
        .iter()
        .position(|known| *known == name)
        .expect("a manner pair")
}
fn chance(store: &CharacterStore, lifecycle: &LifeCycle, a: u64, b: u64, kind: &str) -> u32 {
    let pops = Pops::new();
    let mut env = Env::new("chance");
    let world = world(lifecycle, &mut env, &pops, 7);
    store
        .strand_chance_ppb(&world, a, b, kind, today())
        .expect("a chance")
}

// --- Chances -------------------------------------------------------------------------------------------------------------------

#[test]
fn each_kind_has_a_worked_chance_exact_to_the_billionth() {
    let lifecycle = &rules().lifecycle;
    let mut ruler_in_house = housed(ruler(man(3, 30, "h")), "ui_neill");
    ruler_in_house.life.skills.insert("oratory".into(), 60_000);
    let mut skilled = housed(man(2, 30, "h"), "ui_neill");
    skilled.life.skills.insert("oratory".into(), 60_000);
    let store = store_of(vec![
        housed(man(1, 30, "h"), "ui_neill"),
        skilled,
        ruler_in_house,
        housed(woman(4, 30, "h"), "ui_neill"),
    ]);
    // The pop holds a ruler, so the household's 0.9 gains the court's 0.05: contact 0.95.
    // friend: 0.12 x 0.95 x 1 x 1 x (1 + 0.2 x (1 - 0)) x 1 / (1 + 0)
    assert_eq!(chance(&store, lifecycle, 1, 2, "friend"), 136_800_000);
    // rival: 0.06 x 0.95 (neither holds power).
    assert_eq!(chance(&store, lifecycle, 1, 2, "rival"), 57_000_000);
    // lover: 0.04 x 0.95, a man and a woman, both straight, not kin, both 16 or more; two straight men cannot.
    assert_eq!(chance(&store, lifecycle, 1, 4, "lover"), 38_000_000);
    assert_eq!(chance(&store, lifecycle, 1, 2, "lover"), 0);
    // love: 0.06 x 0.95 (a straight man toward a woman: beauty none = 1).
    assert_eq!(chance(&store, lifecycle, 1, 4, "love"), 57_000_000);
    // respect: 0.06 x 0.95 x (1 + 0) x (skills 60 / 60) x (1 + 0.3 x power), power 0 or 2.
    assert_eq!(chance(&store, lifecycle, 1, 2, "respect"), 57_000_000);
    assert_eq!(chance(&store, lifecycle, 1, 3, "respect"), 91_200_000);
    // envy: 0.04 x 0.95 x (0 / 3 + 0.2) x 1 x 1.
    assert_eq!(chance(&store, lifecycle, 1, 2, "envy"), 7_600_000);
    // fear: 0.04 x 0.95 x (power gap 2) toward the ruler; none toward a peer.
    assert_eq!(chance(&store, lifecycle, 1, 3, "fear"), 76_000_000);
    assert_eq!(chance(&store, lifecycle, 1, 2, "fear"), 0);
    // hatred: 0.006 x 0.95.
    assert_eq!(chance(&store, lifecycle, 1, 2, "hatred"), 5_700_000);
}

#[test]
fn a_spouse_and_a_parent_multiply_love_and_a_negative_product_is_no_chance_at_all() {
    let lifecycle = &rules().lifecycle;
    let mut husband = housed(man(1, 40, "h"), "ui_neill");
    let mut wife = housed(woman(2, 40, "h"), "ui_neill");
    husband.spouses.push(2);
    wife.spouses.push(1);
    let father = housed(man(3, 60, "h"), "ui_neill");
    let mut son = housed(man(4, 30, "h"), "ui_neill");
    son.father = 3;
    let mut reckless = housed(man(5, 30, "h"), "ui_neill");
    reckless.personality.manner[manner_index("wary_daring")] = 51_000;
    let ruler_peer = housed(ruler(man(6, 50, "h")), "ui_neill");
    let store = store_of(vec![husband, wife, father, son, reckless, ruler_peer]);
    // (A ruler lives in the pop: contact 0.95.) love of a husband for his wife: 0.06 x 0.95 x 4 (spouse) x 1.
    assert_eq!(chance(&store, lifecycle, 1, 2, "love"), 228_000_000);
    // love of a son for his father: 0.06 x 0.95 x 5 (a straight man toward a man is not attracted).
    assert_eq!(chance(&store, lifecycle, 4, 3, "love"), 285_000_000);
    // fear: a plain man toward the ruler is possible; for the reckless one `1 - 0.02 x wary_daring` is negative: no chance.
    assert!(chance(&store, lifecycle, 4, 6, "fear") > 0);
    assert_eq!(chance(&store, lifecycle, 5, 6, "fear"), 0);
}

// --- The yearly rolls ------------------------------------------------------------------------------------------------------------

#[test]
fn a_bond_is_rolled_once_a_pair_a_year_a_new_strand_starts_at_one_and_both_lives_log_it() {
    let lifecycle = certain(&["friend"]);
    let pops = Pops::new();
    let mut env = Env::new("once");
    let mut store = store_of(vec![
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "h"), "ui_neill"),
    ]);
    birthdays(&mut store, &lifecycle, &mut env, &pops, today());
    for (a, b) in [(1, 2), (2, 1)] {
        assert_eq!(strength(&store, a, b, "friend"), 1);
        assert_eq!(get(&store, a).ties.links[&b].mutual_roll, 768);
    }
    assert_eq!(env.lines(1), vec!["c1 and c2 became friends."]);
    assert_eq!(env.lines(2), vec!["c2 and c1 became friends."]);
    birthdays(&mut store, &lifecycle, &mut env, &pops, date(769, 3, 3));
    assert_eq!(strength(&store, 1, 2, "friend"), 2);
    assert_eq!(strength(&store, 2, 1, "friend"), 2);
    assert_eq!(get(&store, 1).ties.strand(2, "friend").grew, 769);
    assert_eq!(strength(&store, 1, 2, "rival"), 0);
    assert_eq!(strength(&store, 1, 2, "love"), 0);
}

#[test]
fn a_bond_logs_on_reaching_five_and_ten_and_no_line_for_the_points_between() {
    let lifecycle = certain(&["friend"]);
    let pops = Pops::new();
    let mut env = Env::new("milestones");
    let mut store = store_of(vec![
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "h"), "ui_neill"),
    ]);
    for year in 768..778 {
        birthdays(&mut store, &lifecycle, &mut env, &pops, date(year, 3, 3));
    }
    assert_eq!(strength(&store, 1, 2, "friend"), 10);
    assert_eq!(
        env.lines(1),
        vec![
            "c1 and c2 became friends.",
            "c1 and c2 became close friends.",
            "c1 and c2 became the closest of friends."
        ]
    );
    birthdays(&mut store, &lifecycle, &mut env, &pops, date(778, 3, 3));
    assert_eq!(strength(&store, 1, 2, "friend"), 10);
    assert_eq!(env.lines(1).len(), 3);
}

#[test]
fn a_lover_needs_attraction_both_ways_no_kinship_and_both_sixteen() {
    let lifecycle = certain(&["lover"]);
    let pops = Pops::new();
    let run = |characters: Vec<Character>| {
        let mut env = Env::new("lover");
        let mut store = store_of(characters);
        birthdays(&mut store, &lifecycle, &mut env, &pops, today());
        store
    };
    let house = |c: Character| housed(c, "ui_neill");
    let store = run(vec![house(man(1, 30, "h")), house(woman(2, 30, "h"))]);
    assert_eq!(strength(&store, 1, 2, "lover"), 1);
    let store = run(vec![house(man(1, 30, "h")), house(man(2, 30, "h"))]);
    assert_eq!(strength(&store, 1, 2, "lover"), 0);
    let store = run(vec![
        house(gay(man(1, 30, "h"))),
        house(gay(man(2, 30, "h"))),
    ]);
    assert_eq!(strength(&store, 1, 2, "lover"), 1);
    let store = run(vec![house(gay(man(1, 30, "h"))), house(man(2, 30, "h"))]);
    assert_eq!(strength(&store, 1, 2, "lover"), 0);
    let mut brother = house(man(1, 30, "h"));
    let mut sister = house(woman(2, 30, "h"));
    brother.father = 9;
    sister.father = 9;
    assert_eq!(strength(&run(vec![brother, sister]), 1, 2, "lover"), 0);
    let father = house(man(1, 50, "h"));
    let mut daughter = house(woman(2, 25, "h"));
    daughter.father = 1;
    assert_eq!(strength(&run(vec![father, daughter]), 1, 2, "lover"), 0);
    let store = run(vec![house(man(1, 15, "h")), house(woman(2, 30, "h"))]);
    assert_eq!(strength(&store, 1, 2, "lover"), 0);
}

#[test]
fn a_feeling_is_the_holders_alone_and_rolled_at_its_birthday_once_a_year() {
    let lifecycle = certain(&["respect"]);
    let pops = Pops::new();
    let mut env = Env::new("feeling");
    let mut admired = housed(man(2, 30, "h"), "ui_neill");
    admired.life.skills.insert("oratory".into(), 60_000);
    let mut fan = housed(man(1, 30, "h"), "ui_neill");
    fan.life.skills.insert("oratory".into(), 0);
    let mut store = store_of(vec![fan, admired]);
    birthdays(&mut store, &lifecycle, &mut env, &pops, today());
    assert_eq!(strength(&store, 1, 2, "respect"), 1);
    assert_eq!(strength(&store, 2, 1, "respect"), 0);
    assert_eq!(get(&store, 1).ties.links[&2].feeling_roll, 768);
    assert_eq!(env.lines(1), vec!["c1 came to respect c2."]);
    assert!(env.lines(2).is_empty());
}

// --- Fade and the 1 January resolution ---------------------------------------------------------------------------------------------

fn new_year(
    store: &mut CharacterStore,
    lifecycle: &LifeCycle,
    env: &mut Env,
    pops: &Pops,
    year: u16,
) {
    store
        .run_new_year(&mut world(lifecycle, env, pops, 7), date(year, 1, 1))
        .expect("the 1 January resolution");
}

#[test]
fn friends_who_move_apart_lose_one_point_once_a_year_on_the_first_of_january() {
    let lifecycle = rules().lifecycle.clone();
    let pops = Pops::new();
    let mut env = Env::new("apart");
    let mut characters = vec![
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "z"), "ui_neill"),
    ];
    bond(&mut characters, 1, 2, "friend", 3, 760);
    let mut store = store_of(characters);
    new_year(&mut store, &lifecycle, &mut env, &pops, 768);
    assert_eq!(strength(&store, 1, 2, "friend"), 2);
    assert_eq!(strength(&store, 2, 1, "friend"), 2);
    assert_eq!(get(&store, 1).ties.links[&2].resolved, 767);
    // The same day again, and a save between: the closed year is resolved exactly once.
    new_year(&mut store, &lifecycle, &mut env, &pops, 768);
    let mut store = reloaded(&store);
    new_year(&mut store, &lifecycle, &mut env, &pops, 768);
    assert_eq!(strength(&store, 1, 2, "friend"), 2);
    // A birthday never fades a bond: only the resolution does.
    birthdays(&mut store, &lifecycle, &mut env, &pops, today());
    assert_eq!(strength(&store, 1, 2, "friend"), 2);
    new_year(&mut store, &lifecycle, &mut env, &pops, 769);
    assert_eq!(strength(&store, 1, 2, "friend"), 1);
    new_year(&mut store, &lifecycle, &mut env, &pops, 770);
    assert_eq!(strength(&store, 1, 2, "friend"), 0);
    assert_eq!(env.lines(1), vec!["c1 and c2 drifted apart."]);
    assert_eq!(env.lines(2), vec!["c2 and c1 drifted apart."]);
}

#[test]
fn a_feeling_whose_partner_is_out_of_reach_fades_at_the_birthday_once_and_never_in_the_year_it_grew()
 {
    let lifecycle = rules().lifecycle.clone();
    let pops = Pops::new();
    let mut env = Env::new("feeling-fade");
    let mut characters = vec![
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "z"), "ui_neill"),
    ];
    feeling(&mut characters, 1, 2, "envy", 3, 760);
    feeling(&mut characters, 1, 2, "fear", 3, 768);
    let mut store = store_of(characters);
    birthdays(&mut store, &lifecycle, &mut env, &pops, today());
    // Envy fades (the partner is elsewhere); fear grew this year and keeps its strength.
    assert_eq!(strength(&store, 1, 2, "envy"), 2);
    assert_eq!(strength(&store, 1, 2, "fear"), 3);
    assert_eq!(get(&store, 1).ties.strand(2, "envy").faded, 768);
    // The same year's second pass changes nothing more.
    birthdays(&mut store, &lifecycle, &mut env, &pops, today());
    assert_eq!(strength(&store, 1, 2, "envy"), 2);
}

fn feeling(
    characters: &mut [Character],
    holder: u64,
    partner: u64,
    kind: &str,
    strength: u8,
    grew: u16,
) {
    let link = find(characters, holder)
        .ties
        .links
        .entry(partner)
        .or_default();
    link.strands[kind_at(kind)] = Strand {
        strength,
        grew,
        faded: 0,
    };
}

#[test]
fn a_rivalry_at_five_makes_hatred_each_way_whatever_the_ordinary_hatred_roll_is() {
    let mut lifecycle = certain(&[]);
    lifecycle.connections.strands.hatred_chance_ppb = PPB;
    let pops = Pops::new();
    let mut env = Env::new("hatred");
    let mut characters = vec![
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "h"), "ui_neill"),
    ];
    bond(&mut characters, 1, 2, "rival", 5, 760);
    let mut store = store_of(characters);
    new_year(&mut store, &lifecycle, &mut env, &pops, 768);
    assert_eq!(strength(&store, 1, 2, "hatred"), 1);
    assert_eq!(strength(&store, 2, 1, "hatred"), 1);
    // Below five there is none.
    let mut characters = vec![
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "h"), "ui_neill"),
    ];
    bond(&mut characters, 1, 2, "rival", 4, 760);
    let mut store = store_of(characters);
    new_year(&mut store, &lifecycle, &mut env, &pops, 768);
    assert_eq!(strength(&store, 1, 2, "hatred"), 0);
}

#[test]
fn a_coarse_and_a_full_member_at_low_contact_grow_a_bond_in_either_birthday_order_and_it_does_not_fade_that_year()
 {
    let mut lifecycle = certain(&["friend"]);
    // A friend chance of 1 at contact 0.1.
    lifecycle.connections.strands.friend_withdrawn = [1000 * u64::from(PPB); 7];
    let pops = Pops::new();
    for (full_birthday, coarse_birthday) in [((3, 3), (3, 5)), ((3, 5), (3, 3))] {
        let mut env = Env::new("tiers");
        let mut characters = vec![
            born_on(ruler(man(1, 40, "h")), full_birthday.0, full_birthday.1),
            born_on(
                coarse(in_slot(man(2, 30, "h"), 1)),
                coarse_birthday.0,
                coarse_birthday.1,
            ),
        ];
        bond(&mut characters, 1, 2, "friend", 1, 760);
        let mut store = store_of(characters);
        for (month, day) in [(3, 3), (3, 5)] {
            birthdays(
                &mut store,
                &lifecycle,
                &mut env,
                &pops,
                date(768, month, day),
            );
        }
        // Only the full member's birthday rolls it (the coarse one shares no setting but the same location), and it grew.
        assert_eq!(strength(&store, 1, 2, "friend"), 2);
        assert_eq!(strength(&store, 2, 1, "friend"), 2);
        // The closed year's resolution leaves it: it grew in 768.
        new_year(&mut store, &lifecycle, &mut env, &pops, 769);
        assert_eq!(strength(&store, 1, 2, "friend"), 2);
        assert!(env.lines(1).is_empty() && env.lines(2).is_empty());
    }
}

#[test]
fn two_coarse_characters_never_roll_with_each_other() {
    let lifecycle = certain(&["friend"]);
    let pops = Pops::new();
    let mut env = Env::new("coarse");
    let mut store = store_of(vec![
        coarse(housed(man(1, 30, "h"), "ui_neill")),
        coarse(housed(man(2, 30, "h"), "ui_neill")),
    ]);
    birthdays(&mut store, &lifecycle, &mut env, &pops, today());
    assert_eq!(strength(&store, 1, 2, "friend"), 0);
    assert!(get(&store, 1).ties.links.is_empty());
}

// --- Happenings ------------------------------------------------------------------------------------------------------------------

fn event_world<'a>(
    lifecycle: &'a LifeCycle,
    env: &'a mut Env,
    pops: &'a Pops,
) -> World<'a, Pops, Archive, LifeLog> {
    World {
        rules: &rules().events,
        ..world(lifecycle, env, pops, 7)
    }
}
fn sure(lifecycle: &mut LifeCycle, rule: &str) {
    lifecycle
        .connections
        .strands
        .happenings
        .get_mut(rule)
        .expect("a rule")
        .chance_ppb = PPB;
}

#[test]
fn an_insult_gives_the_stress_event_and_the_insulters_hatred_in_the_insulted() {
    let mut lifecycle = certain(&[]);
    sure(&mut lifecycle, "insult");
    let pops = Pops::new();
    let mut env = Env::new("insult");
    let mut store = store_of(vec![
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "h"), "ui_neill"),
    ]);
    let stress_before = get(&store, 1).personality.stress;
    store
        .report_happening_with(
            &mut event_world(&lifecycle, &mut env, &pops),
            1,
            "insulted",
            Some(2),
            today(),
        )
        .expect("insulted");
    assert_eq!(strength(&store, 1, 2, "hatred"), 1);
    assert_eq!(strength(&store, 2, 1, "hatred"), 0);
    assert!(get(&store, 1).personality.stress > stress_before);
    assert_eq!(
        env.lines(1).last().map(String::as_str),
        Some("c1 came to hate c2.")
    );
}

#[test]
fn a_killing_reported_after_the_victim_is_archived_gives_each_surviving_relative_hatred_and_the_passion_once()
 {
    let mut lifecycle = certain(&[]);
    lifecycle.connections.passions.coarse_ppb = PPB;
    for moment in &mut lifecycle.connections.passions.moments {
        if moment.key == "kin_killed" {
            moment.chance_ppb = PPB;
        }
    }
    let pops = Pops::new();
    let mut env = Env::new("killing");
    // The victim (10) is archived: his father 11, his son 12 and his wife 13 live on; so does the killer, 20, of another house.
    let mut victim = housed(man(10, 40, "h"), "ui_neill");
    victim.father = 11;
    victim.children = vec![12];
    victim.spouses = vec![13];
    victim.death = Some(date(768, 1, 1));
    env.archive.append(&victim).expect("archive");
    let mut son = coarse(housed(man(12, 15, "h"), "ui_neill"));
    son.father = 10;
    let mut wife = housed(woman(13, 38, "h"), "ui_neill");
    wife.spouses = vec![10];
    let mut store = store_of(vec![
        housed(man(11, 65, "h"), "ui_neill"),
        son,
        wife,
        housed(man(20, 35, "h"), "ui_briain"),
    ]);
    store
        .report_killing(
            &mut world(&lifecycle, &mut env, &pops, 7),
            10,
            20,
            date(768, 1, 2),
        )
        .expect("a killing");
    for relative in [11, 12, 13] {
        assert_eq!(
            strength(&store, relative, 20, "hatred"),
            1,
            "relative {relative}"
        );
        let passions = &get(&store, relative).ties.passions;
        assert_eq!(passions.len(), 1, "relative {relative}");
        assert_eq!(
            passions[0].target,
            PassionTarget {
                kind: 2,
                key: "ui_briain".into()
            }
        );
        assert_eq!((passions[0].kind, passions[0].strength), (2, 3));
    }
    assert_eq!(strength(&store, 20, 11, "hatred"), 0);
    assert!(get(&store, 20).ties.passions.is_empty());
}

#[test]
fn a_dead_killer_draws_no_hatred_but_the_passion_toward_his_house_and_a_houseless_one_none() {
    let mut lifecycle = certain(&[]);
    lifecycle.connections.passions.full_ppb = PPB;
    for moment in &mut lifecycle.connections.passions.moments {
        if moment.key == "kin_killed" {
            moment.chance_ppb = PPB;
        }
    }
    let pops = Pops::new();
    let mut env = Env::new("dead-killer");
    let mut killer = housed(man(20, 35, "h"), "ui_briain");
    killer.death = Some(date(768, 1, 2));
    env.archive.append(&killer).expect("archive");
    let mut houseless = man(21, 35, "h");
    houseless.death = Some(date(768, 1, 2));
    env.archive.append(&houseless).expect("archive");
    let mut victim = housed(man(10, 40, "h"), "ui_neill");
    victim.death = Some(date(768, 1, 1));
    victim.spouses = vec![13];
    env.archive.append(&victim).expect("archive");
    let mut wife = housed(woman(13, 38, "h"), "ui_neill");
    wife.spouses = vec![10];
    let mut store = store_of(vec![wife]);
    let mut world = world(&lifecycle, &mut env, &pops, 7);
    store
        .report_killing(&mut world, 10, 20, date(768, 1, 3))
        .expect("a killing");
    assert_eq!(strength(&store, 13, 20, "hatred"), 0);
    assert_eq!(get(&store, 13).ties.passions.len(), 1);
    assert_eq!(get(&store, 13).ties.passions[0].target.key, "ui_briain");
    // A houseless killer: no passion (and no new one).
    store
        .report_killing(&mut world, 10, 21, date(768, 1, 4))
        .expect("a killing");
    assert_eq!(get(&store, 13).ties.passions.len(), 1);
}

// --- Passions --------------------------------------------------------------------------------------------------------------------

fn love_of(place: &str, strength: u8, renewed: GameTime) -> Passion {
    Passion {
        kind: 1,
        target: PassionTarget {
            kind: 0,
            key: place.into(),
        },
        strength,
        renewed,
        last_fade: renewed,
    }
}
fn sure_moment(lifecycle: &mut LifeCycle, key: &str) {
    for moment in &mut lifecycle.connections.passions.moments {
        if moment.key == key {
            moment.chance_ppb = PPB;
        }
    }
}

#[test]
fn long_rule_needs_ten_years_of_the_marker_at_the_location_and_a_move_or_a_new_marker_restarts_it()
{
    let mut lifecycle = certain(&[]);
    sure_moment(&mut lifecycle, "long_rule");
    let pops = Pops::new();
    let run = |office_since: GameTime, location_since: GameTime| {
        let mut ruler = ruler(man(1, 40, "h"));
        ruler.ties.office_since = Some(office_since);
        ruler.ties.location_since = location_since;
        let mut env = Env::new("long-rule");
        let mut store = store_of(vec![ruler]);
        birthdays(&mut store, &lifecycle, &mut env, &pops, today());
        (get(&store, 1).ties.passions.clone(), env.lines(1))
    };
    let (passions, lines) = run(date(750, 1, 1), date(750, 1, 1));
    assert_eq!(passions.len(), 1);
    assert_eq!(
        passions[0].target,
        PassionTarget {
            kind: 0,
            key: "h".into()
        }
    );
    assert_eq!((passions[0].kind, passions[0].strength), (1, 3));
    assert_eq!(lines, vec!["c1 came to love h."]);
    // Nine years at the location, however long the marker; nine years of the marker, however long at the location.
    assert!(run(date(750, 1, 1), date(759, 6, 1)).0.is_empty());
    assert!(run(date(759, 6, 1), date(750, 1, 1)).0.is_empty());
}

#[test]
fn a_passion_that_fades_to_nothing_ends_with_its_line_and_a_tenth_point_logs() {
    let mut lifecycle = certain(&[]);
    sure_moment(&mut lifecycle, "long_rule");
    let pops = Pops::new();
    let mut env = Env::new("passion-lines");
    let mut c = ruler(man(1, 40, "h"));
    c.ties.office_since = Some(date(750, 1, 1));
    c.ties.location_since = date(750, 1, 1);
    c.ties.passions.push(love_of("h", 9, date(767, 1, 1)));
    let mut store = store_of(vec![c]);
    birthdays(&mut store, &lifecycle, &mut env, &pops, today());
    assert_eq!(get(&store, 1).ties.passions[0].strength, 10);
    assert_eq!(
        env.lines(1),
        vec!["c1's love for h became a burning passion."]
    );
    let lifecycle = certain(&[]);
    let mut c = man(2, 40, "h");
    c.ties.passions.push(love_of("h", 1, date(750, 1, 1)));
    let mut store = store_of(vec![c]);
    birthdays(&mut store, &lifecycle, &mut env, &pops, today());
    assert!(get(&store, 2).ties.passions.is_empty());
    assert_eq!(env.lines(2), vec!["c2's love for h faded."]);
}

// --- Settling in and the burn-in -------------------------------------------------------------------------------------------------

#[test]
fn the_burn_in_runs_the_yearly_rolls_for_everyone_in_date_order_and_is_deterministic() {
    let lifecycle = certain(&["friend"]);
    let pops = Pops::new();
    let run = || {
        let mut env = Env::new("burn");
        let mut store = store_of(vec![
            housed(man(1, 30, "h"), "ui_neill"),
            housed(man(2, 30, "h"), "ui_neill"),
            housed(woman(3, 30, "h"), "ui_neill"),
        ]);
        store
            .burn_in(
                &mut world(&lifecycle, &mut env, &pops, 7),
                3,
                date(768, 1, 1),
            )
            .expect("burn in");
        store
    };
    let first = run();
    assert_eq!(strength(&first, 1, 2, "friend"), 3);
    assert_eq!(strength(&first, 2, 3, "friend"), 3);
    assert_eq!(
        first.to_bytes().expect("encode"),
        run().to_bytes().expect("encode")
    );
}

// --- Hand-offs -------------------------------------------------------------------------------------------------------------------

/// The shipped data with every conception certain for a woman and a man of any age in the mode `trying` and none in any other.
fn only_trying(lover_trying_ppb: u32) -> LifeCycle {
    let mut data = certain(&[]);
    data.conception.monthly_ppb = PPB;
    data.conception.women = vec![(0, u64::from(PPB))];
    data.conception.men = vec![(0, u64::from(PPB))];
    data.conception.church_abstinence_ppb = u64::from(PPB);
    data.relations.mode_factor = [u64::from(PPB), 0, 0];
    data.relations.lover_trying_ppb = lover_trying_ppb;
    data
}
fn lovers(woman_id: u64, man_id: u64) -> Vec<Character> {
    let mut characters = vec![
        housed(woman(woman_id, 25, "h"), "ui_neill"),
        housed(man(man_id, 27, "h"), "ui_briain"),
    ];
    bond(&mut characters, woman_id, man_id, "lover", 1, 767);
    characters
}
fn conceive(
    store: &mut CharacterStore,
    lifecycle: &LifeCycle,
    env: &mut Env,
    pops: &Pops,
    day: GameTime,
) {
    store
        .run_conceptions(&mut world(lifecycle, env, pops, 7), day)
        .expect("conceptions");
}

#[test]
fn a_male_lover_who_is_not_a_spouse_can_father_a_child_in_a_year_he_is_trying_and_a_same_sex_lover_never()
 {
    let pops = Pops::new();
    let mut env = Env::new("lover-child");
    // His year's mode is certain trying: the monthly check conceives.
    let lifecycle = only_trying(PPB);
    let mut store = store_of(lovers(1, 2));
    conceive(&mut store, &lifecycle, &mut env, &pops, date(768, 2, 1));
    assert_eq!(get(&store, 1).life.pregnant_by, Some(2));
    // A woman's female lover never enters conception, nor does a man's male one.
    let mut characters = vec![
        housed(woman(1, 25, "h"), "ui_neill"),
        housed(woman(2, 27, "h"), "ui_briain"),
    ];
    bond(&mut characters, 1, 2, "lover", 1, 767);
    let mut store = store_of(characters);
    conceive(&mut store, &lifecycle, &mut env, &pops, date(768, 2, 1));
    assert_eq!(get(&store, 1).life.pregnant_by, None);
    // A spouse who is also a lover gets his one attempt, as a spouse.
    let mut characters = lovers(1, 2);
    find(&mut characters, 1).spouses.push(2);
    find(&mut characters, 2).spouses.push(1);
    // A lover is never trying in this data, so a child can only come by the couple's own mode as spouses (trying: no children yet).
    let mut store = store_of(characters);
    conceive(
        &mut store,
        &only_trying(0),
        &mut env,
        &pops,
        date(768, 2, 1),
    );
    assert_eq!(get(&store, 1).life.pregnant_by, Some(2));
    // The spouse's mode (trying, no children) is certain to fail here and the lover's year (neutral) would conceive: a spouse who is also
    // a lover gets no second attempt as a lover, so there is no child.
    let mut none = only_trying(0);
    none.relations.mode_factor = [0, PPB.into(), 0];
    let mut characters = lovers(1, 2);
    find(&mut characters, 1).spouses.push(2);
    find(&mut characters, 2).spouses.push(1);
    let mut store = store_of(characters);
    conceive(&mut store, &none, &mut env, &pops, date(768, 2, 1));
    assert_eq!(get(&store, 1).life.pregnant_by, None);
}

#[test]
fn a_lovers_mode_is_one_draw_for_the_calendar_year_the_same_every_month_and_after_a_reload() {
    let pops = Pops::new();
    let mut env = Env::new("lover-mode");
    let lifecycle = only_trying(100_000_000);
    // The draw the engine takes: the woman's stream, purpose `lover_mode.2`, at 1 January of the year.
    let trying = rolls(7, 1, "lover_mode.2", date(768, 1, 1), 100_000_000);
    let mut store = store_of(lovers(1, 2));
    conceive(&mut store, &lifecycle, &mut env, &pops, date(768, 2, 1));
    assert_eq!(get(&store, 1).life.pregnant_by.is_some(), trying);
    if !trying {
        // The same pair a month later and after a reload: still not trying, so still no child.
        let mut store = reloaded(&store);
        conceive(&mut store, &lifecycle, &mut env, &pops, date(768, 3, 1));
        conceive(&mut store, &lifecycle, &mut env, &pops, date(768, 4, 1));
        assert_eq!(get(&store, 1).life.pregnant_by, None);
    }
    // Over many pairs both outcomes occur, each as its own draw says.
    let mut seen = [false; 2];
    for woman_id in 1..200_u64 {
        let expected = rolls(7, woman_id, "lover_mode.500", date(768, 1, 1), 100_000_000);
        seen[usize::from(expected)] = true;
    }
    assert_eq!(seen, [true, true]);
}

#[test]
fn a_father_names_his_son_for_a_friend_from_his_live_or_his_archived_record() {
    let mut lifecycle = certain(&[]);
    lifecycle.namesakes.order = vec![("fathers_friend".to_owned(), PPB)];
    let pops = Pops::new();
    let newborn = || NewCharacter {
        name: String::new(),
        sex: Sex::Male,
        birth: date(768, 3, 3),
        pop: locator("h", 0),
        house: String::new(),
    };
    for dead in [false, true] {
        let mut env = Env::new("namesake");
        let mut friend = housed(man(2, 40, "h"), "ui_neill");
        "Cormac".clone_into(&mut friend.name);
        let mut characters = vec![housed(man(1, 45, "h"), "ui_neill"), friend];
        bond(&mut characters, 1, 2, "friend", 4, 760);
        let father = characters.remove(0);
        let mut store = store_of(characters);
        let mut father = father;
        let (father_id, lineage_father) = if dead {
            father.death = Some(date(767, 1, 1));
            env.archive.append(&father).expect("archive");
            (1, 1)
        } else {
            let mut all = store_of(vec![father.clone(), get(&store, 2).clone()]);
            std::mem::swap(&mut store, &mut all);
            (1, 1)
        };
        let id = store
            .state_create_child(
                &Context {
                    seed: 7,
                    tick: date(768, 3, 3),
                    ..context(&lifecycle)
                },
                newborn(),
                &irish(),
                &Lineage {
                    father: lineage_father,
                    mother: UNKNOWN,
                    archive: &env.archive,
                },
                &pops,
            )
            .expect("child");
        let _ = father_id;
        assert_eq!(get(&store, id).name, "Cormac", "dead father: {dead}");
    }
}

// --- Opinion, death and the archive -----------------------------------------------------------------------------------------------

#[test]
fn opinion_adds_each_strand_and_each_passion_aimed_at_the_other_per_point_and_is_pure() {
    let lifecycle = &rules().lifecycle;
    let pops = Pops::new();
    let mut env = Env::new("opinion");
    let mut characters = vec![
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "h"), "ui_briain"),
    ];
    bond(&mut characters, 1, 2, "friend", 4, 760);
    find(&mut characters, 1)
        .ties
        .links
        .get_mut(&2)
        .expect("link")
        .strands[kind_at("love")]
    .strength = 2;
    find(&mut characters, 1)
        .ties
        .links
        .get_mut(&2)
        .expect("link")
        .strands[kind_at("hatred")]
    .strength = 1;
    find(&mut characters, 1).ties.passions.push(Passion {
        kind: 0,
        target: PassionTarget {
            kind: 2,
            key: "ui_briain".into(),
        },
        strength: 3,
        renewed: date(760, 1, 1),
        last_fade: date(760, 1, 1),
    });
    let store = store_of(characters);
    let world = world(lifecycle, &mut env, &pops, 7);
    let plain =
        ck_characters::personality_opinion(&rules().personality, get(&store, 1), get(&store, 2))
            .expect("opinion");
    // friend 4 x 3, love 2 x 4, hatred 1 x -4, loyalty 3 x 2, each in thousandths of a point.
    let expected = plain + (4 * 3 + 2 * 4 - 4 + 3 * 2) * 1000;
    assert_eq!(store.opinion(&world, 1, 2).expect("opinion"), expected);
    assert_eq!(store.opinion(&world, 1, 2).expect("opinion"), expected);
    // The bond counts for both, the one-way feelings and the passion for the holder only.
    let back =
        ck_characters::personality_opinion(&rules().personality, get(&store, 2), get(&store, 1))
            .expect("opinion");
    assert_eq!(
        store.opinion(&world, 2, 1).expect("opinion"),
        back + 4 * 3 * 1000
    );
}

#[test]
fn a_death_ends_the_strands_without_a_line_and_the_archive_keeps_them_as_they_were() {
    let lifecycle = certain(&[]);
    let pops = Pops::new();
    let mut env = Env::new("death");
    let mut characters = vec![
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "h"), "ui_neill"),
    ];
    bond(&mut characters, 1, 2, "friend", 6, 760);
    find(&mut characters, 2)
        .ties
        .passions
        .push(love_of("h", 4, date(760, 1, 1)));
    let mut store = store_of(characters);
    store
        .die_with_events(
            &mut world(&lifecycle, &mut env, &pops, 7),
            2,
            date(768, 1, 5),
            "illness",
        )
        .expect("death");
    assert!(!get(&store, 1).ties.links.contains_key(&2));
    assert!(env.lines(1).is_empty());
    let archived = env.archive.get(2).expect("read").expect("archived");
    assert_eq!(archived.ties.strength(1, "friend"), 6);
    assert_eq!(archived.ties.passions.len(), 1);
}

#[test]
fn the_state_saves_loads_and_replays_to_the_same_bytes() {
    let lifecycle = certain(&["friend", "respect"]);
    let pops = Pops::new();
    let characters = || {
        vec![
            housed(man(1, 30, "h"), "ui_neill"),
            housed(man(2, 30, "h"), "ui_neill"),
            housed(woman(3, 28, "h"), "ui_neill"),
        ]
    };
    let mut straight = store_of(characters());
    let mut env = Env::new("replay-a");
    for year in 768..772 {
        birthdays(&mut straight, &lifecycle, &mut env, &pops, date(year, 3, 3));
    }
    let mut saved = store_of(characters());
    let mut env = Env::new("replay-b");
    for year in 768..772 {
        birthdays(&mut saved, &lifecycle, &mut env, &pops, date(year, 3, 3));
        saved = reloaded(&saved);
    }
    assert_eq!(
        straight.to_bytes().expect("encode"),
        saved.to_bytes().expect("encode")
    );
    assert!(straight.iter().all(|c| !c.ties.links.is_empty()));
}

// --- Review 1 ----------------------------------------------------------------------------------------------------------------------

fn stress_of(store: &CharacterStore, id: u64) -> i64 {
    get(store, id).personality.stress
}

#[test]
fn survivors_of_one_battle_each_get_a_count_one_stress_event_and_each_pair_one_friend_roll() {
    let mut lifecycle = certain(&[]);
    sure(&mut lifecycle, "battle_comrades");
    let pops = Pops::new();
    // The battle event of the shipped data, and a control character who survives one battle alone.
    let events = events_from(
        "[[event]]\nid = \"battle_stress\"\ntrigger = \"happening:battle_survived\"\nchance = 1\neffects = [{ stress = 7 }, { count = \"battles_survived\", add = 1 }]\ntext = \"{name} survived a battle.\"\n",
    );
    let mut env = Env::new("battle");
    let mut store = store_of(vec![
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "h"), "ui_neill"),
        housed(man(3, 30, "h"), "ui_neill"),
    ]);
    {
        let mut world = World {
            rules: events,
            ..world(&lifecycle, &mut env, &pops, 7)
        };
        for survivor in [1, 2, 3] {
            store
                .report_happening_with(&mut world, survivor, "battle_survived", None, today())
                .expect("battle survived");
        }
        for (a, b) in [(1, 2), (1, 3), (2, 3)] {
            store
                .report_happening_with(&mut world, a, "battle_comrades", Some(b), today())
                .expect("comrades");
        }
    }
    for id in [1, 2, 3] {
        // Exactly one stress event and one count each, whatever the pairs did.
        assert_eq!(stress_of(&store, id), 7_000, "survivor {id}");
        assert_eq!(
            get(&store, id).life.counters.get("battles_survived"),
            Some(&1)
        );
    }
    for (a, b) in [(1, 2), (1, 3), (2, 3)] {
        assert_eq!(strength(&store, a, b, "friend"), 1);
        assert_eq!(strength(&store, b, a, "friend"), 1);
    }
}

#[test]
fn a_rescue_and_a_duel_with_a_live_counterpart_and_nothing_for_one_a_fatal_event_killed() {
    let mut lifecycle = certain(&[]);
    sure(&mut lifecycle, "rescue");
    sure(&mut lifecycle, "duel");
    let pops = Pops::new();
    let mut env = Env::new("rescue");
    let mut store = store_of(vec![
        housed(man(1, 30, "h"), "ui_neill"),
        housed(man(2, 30, "h"), "ui_neill"),
        housed(man(3, 30, "h"), "ui_neill"),
        housed(man(4, 30, "h"), "ui_neill"),
    ]);
    {
        let mut world = world(&lifecycle, &mut env, &pops, 7);
        store
            .report_happening_with(&mut world, 1, "rescued", Some(2), today())
            .expect("rescued");
        assert_eq!(
            (
                strength(&store, 1, 2, "love"),
                strength(&store, 1, 2, "respect")
            ),
            (1, 1)
        );
        assert_eq!(
            (
                strength(&store, 2, 1, "love"),
                strength(&store, 2, 1, "respect")
            ),
            (0, 0)
        );
        store
            .report_happening_with(&mut world, 1, "duel", Some(2), today())
            .expect("duel");
        assert_eq!(
            (
                strength(&store, 1, 2, "rival"),
                strength(&store, 2, 1, "rival")
            ),
            (1, 1)
        );
        assert_eq!(
            (
                strength(&store, 1, 2, "respect"),
                strength(&store, 2, 1, "respect")
            ),
            (2, 1)
        );
    }
    // An earlier event kills the character the happening is reported to: its strands never run, for either end.
    let fatal = events_from(
        "[[event]]\nid = \"fatal_rescue\"\ntrigger = \"happening:rescued\"\nchance = 1\neffects = [{ die = \"misadventure\" }]\ntext = \"{name} died.\"\n\n[[event]]\nid = \"fatal_duel\"\ntrigger = \"happening:duel\"\nchance = 1\neffects = [{ die = \"the duel\" }]\ntext = \"{name} died.\"\n",
    );
    let mut world = World {
        rules: fatal,
        ..world(&lifecycle, &mut env, &pops, 7)
    };
    store
        .report_happening_with(&mut world, 3, "rescued", Some(4), today())
        .expect("rescued");
    assert!(store.get(3).is_none());
    assert_eq!(strength(&store, 4, 3, "love"), 0);
    assert!(get(&store, 4).ties.links.is_empty());
    store
        .report_happening_with(&mut world, 1, "duel", Some(4), today())
        .expect("duel");
    assert!(store.get(1).is_none());
    assert_eq!(
        (
            strength(&store, 4, 1, "rival"),
            strength(&store, 4, 1, "respect")
        ),
        (0, 0)
    );
    // A counterpart who is not live gets nothing and is no error.
    store
        .report_happening_with(&mut world, 2, "insulted", Some(99), today())
        .expect("a missing counterpart");
    assert_eq!(strength(&store, 2, 99, "hatred"), 0);
}

#[test]
fn a_renewing_moment_on_the_due_date_prevents_the_fade_that_was_due_that_day() {
    let pops = Pops::new();
    let mut env = Env::new("renewal");
    // Renewed on 3 March 758, so the first point is due exactly on the birthday of 768.
    let build = |lifecycle: &LifeCycle| {
        let mut c = ruler(man(1, 40, "h"));
        c.ties.office_since = Some(date(750, 1, 1));
        c.ties.location_since = date(750, 1, 1);
        c.ties.passions.push(love_of("h", 5, date(758, 3, 3)));
        let mut store = store_of(vec![c]);
        let mut env = Env::new("renewal-run");
        birthdays(&mut store, lifecycle, &mut env, &Pops::new(), today());
        store
    };
    // Without a renewing moment it fades.
    let faded = build(&certain(&[]));
    assert_eq!(get(&faded, 1).ties.passions[0].strength, 4);
    // With one it renews first: a point gained, none lost.
    let mut lifecycle = certain(&[]);
    sure_moment(&mut lifecycle, "long_rule");
    let renewed = build(&lifecycle);
    let passion = &get(&renewed, 1).ties.passions[0];
    assert_eq!((passion.strength, passion.renewed), (6, today()));
    let _ = (&pops, &mut env);
}

#[test]
fn a_passion_fades_one_point_for_each_full_interval_across_a_save() {
    let lifecycle = certain(&[]);
    let pops = Pops::new();
    let mut env = Env::new("passion-fade");
    let mut c = man(1, 40, "h");
    c.ties.passions.push(love_of("h", 5, date(750, 1, 1)));
    let mut store = store_of(vec![c]);
    birthdays(&mut store, &lifecycle, &mut env, &pops, today());
    let passion = &get(&store, 1).ties.passions[0];
    assert_eq!((passion.strength, passion.last_fade), (4, date(760, 1, 1)));
    let mut store = reloaded(&store);
    birthdays(&mut store, &lifecycle, &mut env, &pops, date(769, 3, 3));
    assert_eq!(get(&store, 1).ties.passions[0].strength, 4);
    birthdays(&mut store, &lifecycle, &mut env, &pops, date(770, 3, 3));
    assert_eq!(get(&store, 1).ties.passions[0].strength, 3);
}

#[test]
fn the_birthday_path_does_no_work_for_characters_at_unrelated_locations_tier_and_house_lookups_included()
 {
    let mut lifecycle = certain(&["friend"]);
    sure_moment(&mut lifecycle, "house_glory");
    let pops = Pops::new();
    let group = || {
        let mut glorious = housed(man(1, 30, "h"), "ui_neill");
        glorious.personality.fame = 20_000;
        vec![
            glorious,
            housed(ruler(man(2, 50, "h")), "ui_neill"),
            housed(man(3, 30, "h"), "ui_neill"),
        ]
    };
    let run = |extra: u64| {
        let mut characters = group();
        // Other locations and houses, with birthdays on other days: nothing of theirs is due.
        characters.extend(
            (100..100 + extra)
                .map(|id| housed(born_on(man(id, 30, "elsewhere"), 6, 6), "ui_other")),
        );
        let mut store = store_of(characters);
        store.reset_candidates_visited();
        let mut env = Env::new("bounded");
        store
            .run_birthdays(&mut world(&lifecycle, &mut env, &pops, 7), today())
            .expect("birthdays");
        // The glorious member of a ruling house gained its passion, so the house lookup ran.
        assert_eq!(get(&store, 1).ties.passions.len(), 1);
        store.candidates_visited()
    };
    let small = run(0);
    assert!(small > 0);
    assert_eq!(run(300), small);
}

#[test]
fn a_zero_boost_with_no_straight_or_asexual_weight_is_refused_when_the_data_loads() {
    let mut text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/game/lifecycle/connections.toml"),
    )
    .expect("shipped data");
    text = text.replace(
        "weights = { straight = 92, gay = 4, bisexual = 3, asexual = 1 }",
        "weights = { straight = 0, gay = 4, bisexual = 3, asexual = 0 }",
    );
    text = text.replace("parent_boost = 1.5 ", "parent_boost = 0 ");
    let pop_types: Vec<&str> = rules().pop_types.iter().map(|p| p.id.as_str()).collect();
    let error = rules()
        .events
        .parse_connections(
            "connections.toml",
            &text,
            &Registry {
                peoples: &rules().peoples,
                genetics: &rules().genetics,
                pop_types: &pop_types,
            },
        )
        .unwrap_err();
    assert!(
        error.entry.contains("orientation.parent_boost"),
        "{error:?}"
    );
}

#[test]
fn a_floor_of_zero_admits_a_partner_with_no_contact() {
    let mut lifecycle = rules().lifecycle.clone();
    lifecycle.connections.circle.floor_ppb = 0;
    let pops = Pops::new();
    let mut env = Env::new("zero-floor");
    // Two houseless characters in one noble pop with no ruler: no setting at all, contact 0.
    let store = store_of(vec![man(1, 30, "h"), man(2, 30, "h")]);
    let world = world(&lifecycle, &mut env, &pops, 7);
    assert_eq!(
        store.circle_of(&world, 1, today()).expect("circle"),
        vec![2]
    );
}

#[test]
fn a_passion_line_names_its_target_by_the_sources_display_name_and_state_keeps_the_key() {
    let mut lifecycle = certain(&[]);
    sure_moment(&mut lifecycle, "long_rule");
    let mut ruler = ruler(man(1, 40, "h"));
    ruler.ties.office_since = Some(date(750, 1, 1));
    ruler.ties.location_since = date(750, 1, 1);
    for (pops, line) in [
        (Pops::new(), "c1 came to love h."),
        (Pops::new().named(), "c1 came to love the place of h."),
    ] {
        let mut env = Env::new("names");
        let mut store = store_of(vec![ruler.clone()]);
        birthdays(&mut store, &lifecycle, &mut env, &pops, today());
        assert_eq!(env.lines(1), vec![line]);
        assert_eq!(get(&store, 1).ties.passions[0].target.key, "h");
    }
}

#[test]
fn a_killer_who_is_close_kin_of_the_victim_still_gets_the_event_but_no_hatred_of_himself() {
    let mut lifecycle = certain(&[]);
    lifecycle.connections.passions.full_ppb = PPB;
    for moment in &mut lifecycle.connections.passions.moments {
        if moment.key == "kin_killed" {
            moment.chance_ppb = PPB;
        }
    }
    let events = events_from(
        "[[event]]\nid = \"grief\"\ntrigger = \"happening:kin_killed\"\nchance = 1\neffects = [{ stress = 9 }]\ntext = \"{name} grieved.\"\n",
    );
    let pops = Pops::new();
    let mut env = Env::new("kin-killer");
    let mut victim = housed(man(10, 40, "h"), "ui_neill");
    victim.father = 11;
    victim.death = Some(date(768, 1, 1));
    env.archive.append(&victim).expect("archive");
    let mut brother = housed(man(12, 35, "h"), "ui_neill");
    brother.father = 11;
    let mut store = store_of(vec![housed(man(11, 65, "h"), "ui_neill"), brother]);
    store
        .report_killing(
            &mut World {
                rules: events,
                ..world(&lifecycle, &mut env, &pops, 7)
            },
            10,
            12,
            date(768, 1, 2),
        )
        .expect("a killing");
    // The father and the brother-killer both grieve; the father hates the killer, the killer none of himself.
    assert_eq!(stress_of(&store, 11), 9_000);
    assert_eq!(stress_of(&store, 12), 9_000);
    assert_eq!(strength(&store, 11, 12, "hatred"), 1);
    assert_eq!(strength(&store, 12, 12, "hatred"), 0);
}

#[test]
fn the_complete_opinion_reads_a_passion_aimed_at_each_of_the_four_target_types() {
    let lifecycle = &rules().lifecycle;
    let pops = Pops::new();
    let mut env = Env::new("opinion-targets");
    let mut subject = housed(man(1, 30, "h"), "ui_neill");
    for (kind, key) in [
        (0, "p"),
        (1, "christianity"),
        (2, "ui_briain"),
        (3, "ulaid"),
    ] {
        subject.ties.passions.push(Passion {
            kind: 0,
            target: PassionTarget {
                kind,
                key: key.into(),
            },
            strength: 2,
            renewed: date(760, 1, 1),
            last_fade: date(760, 1, 1),
        });
    }
    // The other is at place `p`, of the Christian religion and the `ulaid` culture (the harness's pops), of house `ui_briain`.
    let other = housed(man(2, 30, "p"), "ui_briain");
    let store = store_of(vec![subject, other]);
    let world = world(lifecycle, &mut env, &pops, 7);
    let plain =
        ck_characters::personality_opinion(&rules().personality, get(&store, 1), get(&store, 2))
            .expect("opinion");
    // Four loyalty passions of strength 2 aimed at the other's place, religion, house and culture: 4 x 2 x 2 points.
    assert_eq!(
        store.opinion(&world, 1, 2).expect("opinion"),
        plain + 16_000
    );
    // The same through the free function.
    let total = ck_characters::opinion(
        world.personality,
        &lifecycle.connections,
        get(&store, 1),
        get(&store, 2),
        &ck_characters::PopTargets::of(&pops, get(&store, 2)),
    )
    .expect("opinion");
    assert_eq!(total, plain + 16_000);
}

#[test]
fn a_fear_whose_exact_product_is_minus_one_over_a_hundred_and_twenty_five_is_no_chance() {
    // base 0.1 x contact 1 x power gap 2 x (1 - 0.02 x 52) = 0.2 x -0.04 = -1/125.
    let mut lifecycle = certain(&[]);
    lifecycle
        .connections
        .strands
        .base_ppb
        .insert("fear".into(), 100_000_000);
    lifecycle.connections.extra_setting_ppb = 0;
    let build = |daring: i64| {
        let mut reckless = housed(man(5, 30, "h"), "ui_neill");
        reckless.personality.manner[manner_index("wary_daring")] = daring;
        store_of(vec![reckless, housed(ruler(man(6, 50, "h")), "ui_neill")])
    };
    assert_eq!(chance(&build(52_000), &lifecycle, 5, 6, "fear"), 0);
    // 40 points: 0.2 x (1 - 0.8) = 0.04.
    assert_eq!(chance(&build(40_000), &lifecycle, 5, 6, "fear"), 40_000_000);
}

// --- Review 2 ----------------------------------------------------------------------------------------------------------------------

#[test]
fn a_lovers_mode_is_the_same_on_every_check_of_the_year_whatever_seed_each_days_pass_is_given() {
    let pops = Pops::new();
    let lifecycle = only_trying(300_000_000);
    let mut seen = [false; 2];
    for woman_id in 1..40_u64 {
        let man_id = woman_id + 1000;
        // The game's own seed is fixed once; the passes are handed a different seed every day, as the scheduler does.
        let mut store = store_of(lovers(woman_id, man_id));
        store.fix_seed(99);
        let trying = rolls(
            99,
            woman_id,
            &format!("lover_mode.{man_id}"),
            date(768, 1, 1),
            300_000_000,
        );
        seen[usize::from(trying)] = true;
        let mut env = Env::new("stable-mode");
        store
            .run_conceptions(
                &mut world(&lifecycle, &mut env, &pops, 1_001),
                date(768, 2, 1),
            )
            .expect("conceptions");
        assert_eq!(
            get(&store, woman_id).life.pregnant_by.is_some(),
            trying,
            "february, woman {woman_id}"
        );
        if !trying {
            // A save, another day's seed, and a later month: still not trying, so still no child.
            let mut store = reloaded(&store);
            store
                .run_conceptions(
                    &mut world(&lifecycle, &mut env, &pops, 2_002),
                    date(768, 3, 1),
                )
                .expect("conceptions");
            assert_eq!(
                get(&store, woman_id).life.pregnant_by,
                None,
                "march, woman {woman_id}"
            );
        }
    }
    assert_eq!(seen, [true, true]);
}

#[test]
fn separate_rivalries_of_one_character_are_separate_draws() {
    let mut lifecycle = certain(&[]);
    lifecycle.connections.strands.hatred_chance_ppb = 500_000_000;
    let pops = Pops::new();
    let mut hits = [0_u32; 2];
    for seed in 0..16_u64 {
        let mut characters = vec![
            housed(man(1, 30, "h"), "ui_neill"),
            housed(man(2, 30, "h"), "ui_neill"),
            housed(man(3, 30, "h"), "ui_neill"),
        ];
        bond(&mut characters, 1, 2, "rival", 5, 760);
        bond(&mut characters, 1, 3, "rival", 5, 760);
        let mut store = store_of(characters);
        let root = (seed * 7 + 1) | 1;
        store.fix_seed(root);
        let mut env = Env::new("two-rivalries");
        new_year(&mut store, &lifecycle, &mut env, &pops, 768);
        hits[0] += u32::from(strength(&store, 1, 2, "hatred"));
        hits[1] += u32::from(strength(&store, 1, 3, "hatred"));
        // Each matches its own pair's draw.
        for other in [2, 3] {
            let want = u8::from(rolls(
                root,
                1,
                &format!("rivalry_hatred.767.{other}"),
                date(768, 1, 1),
                500_000_000,
            ));
            assert_eq!(
                strength(&store, 1, other, "hatred"),
                want,
                "seed {seed}, rival {other}"
            );
        }
    }
    // Were they one draw the two counts would be equal over every seed.
    assert_ne!(hits[0], hits[1]);
}

#[test]
fn a_birthday_at_zero_contact_does_not_claim_the_year_for_a_later_one_with_contact() {
    let mut lifecycle = certain(&["friend"]);
    lifecycle.connections.circle.floor_ppb = 0;
    lifecycle.connections.strands.friend_withdrawn = [1000 * u64::from(PPB); 7];
    let pops = Pops::new();
    let mut env = Env::new("zero-claim");
    // Two houseless characters in one pop with no ruler: contact 0, so 1's birthday on 3 March rolls nothing.
    let mut store = store_of(vec![man(1, 30, "h"), born_on(man(2, 30, "h"), 3, 5)]);
    birthdays(&mut store, &lifecycle, &mut env, &pops, today());
    assert_eq!(
        get(&store, 1)
            .ties
            .links
            .get(&2)
            .map_or(0, |l| l.mutual_roll),
        0
    );
    // 2 moves to another pop of the place before its birthday: contact 0.1, and the friendship rolls then.
    store
        .move_to_with_events(
            &mut world(&lifecycle, &mut env, &pops, 7),
            2,
            locator("h", 1),
            date(768, 3, 4),
        )
        .expect("move");
    birthdays(&mut store, &lifecycle, &mut env, &pops, date(768, 3, 5));
    assert_eq!(strength(&store, 2, 1, "friend"), 1);
}

#[test]
fn the_ledger_names_a_place_by_the_scenarios_display_name_and_nothing_else() {
    let mut economy = ck_economy::EconomyState::default();
    let reach = BTreeMap::new();
    let names: BTreeMap<String, String> = [("armagh".to_owned(), "Ard Mhacha".to_owned())].into();
    let ledger = ck_characters::Ledger::with_nearby(&mut economy, &reach).with_place_names(&names);
    assert_eq!(
        ledger.target_name("place", "armagh").as_deref(),
        Some("Ard Mhacha")
    );
    assert_eq!(ledger.target_name("place", "unknown"), None);
    assert_eq!(ledger.target_name("religion", "armagh"), None);
}

// --- Review 3 ----------------------------------------------------------------------------------------------------------------------

#[test]
fn a_public_calendar_draw_on_an_unseeded_store_fixes_the_seed_so_a_reload_and_another_seed_change_nothing()
 {
    let pops = Pops::new();
    let lifecycle = only_trying(300_000_000);
    let mut seen = [false; 2];
    for woman_id in 1..40_u64 {
        let man_id = woman_id + 1000;
        // No pass has fixed the seed: the first public call does, from its own seed, and it is saved.
        let mut store = store_of(lovers(woman_id, man_id));
        let mut env = Env::new("unseeded");
        store
            .run_conceptions(
                &mut world(&lifecycle, &mut env, &pops, 1_001),
                date(768, 2, 1),
            )
            .expect("conceptions");
        let trying = rolls(
            1_001,
            woman_id,
            &format!("lover_mode.{man_id}"),
            date(768, 1, 1),
            300_000_000,
        );
        seen[usize::from(trying)] = true;
        assert_eq!(
            get(&store, woman_id).life.pregnant_by.is_some(),
            trying,
            "february, woman {woman_id}"
        );
        if !trying {
            let mut store = reloaded(&store);
            store
                .run_conceptions(
                    &mut world(&lifecycle, &mut env, &pops, 2_002),
                    date(768, 3, 1),
                )
                .expect("conceptions");
            assert_eq!(
                get(&store, woman_id).life.pregnant_by,
                None,
                "march, woman {woman_id}"
            );
        }
    }
    assert_eq!(seen, [true, true]);
}

// --- Review 4 ----------------------------------------------------------------------------------------------------------------------

// --- Promotion without a strand replay, and the passion backfill (spec r9) ---------------------------------------------------------

#[test]
fn a_promoted_character_keeps_its_strands_and_rolls_as_full_tier_from_its_next_birthday() {
    let lifecycle = certain(&["friend"]);
    let pops = Pops::new();
    let mut env = Env::new("r9-promotion");
    // Three coarse years (last full roll 765), a friendship at 2 with a full partner, then a ruler's office in 768.
    let mut promoted = coarse(housed(man(1, 30, "h"), "ui_neill"));
    promoted.ties.last_full = 765;
    let mut characters = vec![promoted, born_on(housed(man(2, 30, "h"), "ui_neill"), 6, 6)];
    bond(&mut characters, 1, 2, "friend", 2, 760);
    let mut store = store_of(characters);
    store
        .set_office(1, Some(Office::Ruler), date(768, 2, 1))
        .expect("office");
    let before = (
        get(&store, 1).ties.links.clone(),
        get(&store, 2).ties.links.clone(),
    );
    // The first daily pass after the promotion (no birthday that day): no strand is rolled, faded or resolved for 765-767.
    store
        .backfill_promotions(&mut world(&lifecycle, &mut env, &pops, 7), date(768, 2, 2))
        .expect("backfilled");
    birthdays(&mut store, &lifecycle, &mut env, &pops, date(768, 2, 2));
    assert_eq!(
        (
            get(&store, 1).ties.links.clone(),
            get(&store, 2).ties.links.clone()
        ),
        before
    );
    assert_eq!(get(&store, 1).ties.last_full, 767);
    // Also across a reload; the partner's strands and markers are untouched.
    let mut store = reloaded(&store);
    assert_eq!(get(&store, 2).ties.links, before.1);
    // Its first full-tier roll is its next birthday, a single one for 768.
    birthdays(&mut store, &lifecycle, &mut env, &pops, today());
    assert_eq!(strength(&store, 1, 2, "friend"), 3);
    assert_eq!(get(&store, 1).ties.links[&2].mutual_roll, 768);
    assert_eq!(get(&store, 1).ties.last_full, 768);
}

#[test]
fn the_top_up_chance_makes_the_coarse_roll_and_one_more_the_full_tiers_exactly() {
    let data = &rules().lifecycle.connections.passions;
    let (full, coarse_tier) = (data.full_ppb, data.coarse_ppb);
    // p = 0.2, full 1, coarse 0.1: 0.2 x 0.9 / (1 - 0.02) = 0.18367346938...
    assert_eq!(
        ck_characters::top_up_chance_ppb(200_000_000, full, coarse_tier),
        183_673_469
    );
    // A certain moment is topped up to certain; a chance of 0 stays 0; a certain coarse roll leaves nothing to add.
    assert_eq!(
        ck_characters::top_up_chance_ppb(PPB, full, coarse_tier),
        PPB
    );
    assert_eq!(ck_characters::top_up_chance_ppb(0, full, coarse_tier), 0);
    assert_eq!(ck_characters::top_up_chance_ppb(PPB, PPB, PPB), 0);
    // The year's total chance is the full tier's, to within the rounding down of one ppb.
    for p in [30_000_000_u32, 50_000_000, 200_000_000, 700_000_000] {
        let q = u128::from(ck_characters::top_up_chance_ppb(p, full, coarse_tier));
        let scale = u128::from(PPB);
        let miss = u128::from(p) * u128::from(coarse_tier) / scale;
        let total = scale - (scale - miss) * (scale - q) / scale;
        let want = u128::from(p) * u128::from(full) / scale;
        assert!(total.abs_diff(want) <= 2, "p {p}: {total} vs {want}");
    }
}

/// A ruler of long standing (office and location since 750) at its own place, so each passion is its own.
fn old_ruler(id: u64) -> Character {
    let mut c = ruler(man(id, 40, "h"));
    c.ties.office_since = Some(date(750, 1, 1));
    c.ties.location_since = date(750, 1, 1);
    c
}
fn long_rule_with(lifecycle: &mut LifeCycle, chance: u32) {
    for moment in &mut lifecycle.connections.passions.moments {
        if moment.key == "long_rule" {
            moment.chance_ppb = chance;
        }
    }
}

#[test]
fn the_backfill_rate_of_a_missed_year_is_the_top_up_chance() {
    let mut lifecycle = certain(&[]);
    long_rule_with(&mut lifecycle, 500_000_000);
    let pops = Pops::new();
    let mut env = Env::new("r9-rate");
    let count = 800_u64;
    let mut store = store_of((1..=count).map(old_ruler).collect());
    store.fix_seed(99);
    for id in 1..=count {
        store
            .settle_in(
                &mut world(&lifecycle, &mut env, &pops, 7),
                id,
                766,
                date(768, 1, 1),
            )
            .expect("settled in");
    }
    let gained = store.iter().filter(|c| !c.ties.passions.is_empty()).count();
    // One missed year (767) at q = 0.5 x 0.9 / (1 - 0.05) = 0.4737: 379 of 800, with a sampling spread of about 14.
    assert!(gained.abs_diff(379) <= 45, "{gained}");
}

#[test]
fn the_backfill_gives_a_year_only_what_its_conditions_allowed_that_year() {
    let mut lifecycle = certain(&[]);
    long_rule_with(&mut lifecycle, PPB);
    let pops = Pops::new();
    let mut env = Env::new("r9-conditions");
    // 767: ten years of the marker needed at the birthday (3 March 767). A marker begun in 762 gives five by then; one in 750, seventeen;
    // one begun after the birthday (in 768) gives nothing.
    let mut store = store_of(vec![
        {
            let mut c = old_ruler(1);
            c.ties.office_since = Some(date(762, 1, 1));
            c
        },
        old_ruler(2),
        {
            let mut c = old_ruler(3);
            c.ties.office_since = Some(date(768, 1, 1));
            c
        },
    ]);
    for id in 1..=3 {
        store
            .settle_in(
                &mut world(&lifecycle, &mut env, &pops, 7),
                id,
                766,
                date(768, 3, 5),
            )
            .expect("settled in");
    }
    assert!(get(&store, 1).ties.passions.is_empty());
    assert_eq!(get(&store, 2).ties.passions.len(), 1);
    assert!(get(&store, 3).ties.passions.is_empty());
    // The gain is dated that year's birthday, and nothing is faded.
    let passion = &get(&store, 2).ties.passions[0];
    assert_eq!(
        (passion.renewed, passion.last_fade),
        (date(767, 3, 3), date(767, 3, 3))
    );
    assert_eq!(passion.strength, 3);
    assert_eq!(env.lines(2), vec!["c2 came to love h."]);
}

#[test]
fn a_backfill_renewal_never_moves_renewed_earlier_and_the_lookback_bounds_the_years() {
    let mut lifecycle = certain(&[]);
    long_rule_with(&mut lifecycle, PPB);
    let pops = Pops::new();
    let mut env = Env::new("r9-renewal");
    let love = Passion {
        kind: 1,
        target: PassionTarget {
            kind: 0,
            key: "h".into(),
        },
        strength: 3,
        renewed: date(768, 1, 1),
        last_fade: date(768, 1, 1),
    };
    let mut held = old_ruler(1);
    held.ties.passions.push(love);
    let mut store = store_of(vec![held]);
    // Two missed years (766, 767): +1 each, and `renewed` stays at the later date.
    store
        .settle_in(
            &mut world(&lifecycle, &mut env, &pops, 7),
            1,
            765,
            date(768, 3, 5),
        )
        .expect("settled in");
    let passion = &get(&store, 1).ties.passions[0];
    assert_eq!((passion.strength, passion.renewed), (5, date(768, 1, 1)));
    // `max_years` of 1 bounds the years: only 767.
    let mut short = lifecycle.clone();
    short.connections.max_years = 1;
    let mut store = store_of(vec![old_ruler(1)]);
    store
        .settle_in(
            &mut world(&short, &mut env, &pops, 7),
            1,
            700,
            date(768, 3, 5),
        )
        .expect("settled in");
    assert_eq!(get(&store, 1).ties.passions[0].strength, 3);
}

#[test]
fn kin_killed_is_not_backfilled_and_a_failed_backfill_grants_nothing_keeps_its_window_and_repeats()
{
    let mut lifecycle = certain(&[]);
    long_rule_with(&mut lifecycle, 500_000_000);
    for moment in &mut lifecycle.connections.passions.moments {
        if moment.key == "kin_killed" {
            moment.chance_ppb = PPB;
        }
    }
    let pops = Pops::new();
    let mut env = Env::new("r9-atomic");
    let mut store = store_of((1..=60).map(old_ruler).collect());
    store.fix_seed(99);
    for id in 1..=60 {
        store
            .settle_in(
                &mut world(&lifecycle, &mut env, &pops, 7),
                id,
                764,
                date(768, 1, 1),
            )
            .expect("settled in");
    }
    // Only long_rule's hatred-free love: no passion toward a house, so kin_killed acted on nobody.
    assert!(
        store
            .iter()
            .all(|c| c.ties.passions.iter().all(|p| p.kind == 1))
    );
    // A failure (a pop the engine cannot resolve) changes nothing, and the retry gives what an undisturbed run gives.
    let build = || {
        let mut s = store_of((1..=60).map(old_ruler).collect());
        s.fix_seed(99);
        s
    };
    let mut disturbed = build();
    let blind = Blind(Pops::new());
    let error = disturbed.settle_in(
        &mut world(&lifecycle, &mut env, &blind, 7),
        1,
        764,
        date(768, 1, 1),
    );
    assert!(error.is_err());
    // Nothing was granted; the window fixed on the first attempt stays saved (765 to 767) and the last full year is untouched.
    let held = get(&disturbed, 1);
    assert!(held.ties.passions.is_empty());
    assert_eq!((held.ties.backfill, held.ties.last_full), ((765, 767), 0));
    for id in 1..=60 {
        disturbed
            .settle_in(
                &mut world(&lifecycle, &mut env, &pops, 7),
                id,
                764,
                date(768, 1, 1),
            )
            .expect("settled in");
    }
    assert_eq!(
        disturbed.to_bytes().expect("encode"),
        store.to_bytes().expect("encode")
    );
    // And across a reload before the run.
    let mut reloaded_first = reloaded(&build());
    for id in 1..=60 {
        reloaded_first
            .settle_in(
                &mut world(&lifecycle, &mut env, &pops, 7),
                id,
                764,
                date(768, 1, 1),
            )
            .expect("settled in");
    }
    assert_eq!(
        reloaded_first.to_bytes().expect("encode"),
        store.to_bytes().expect("encode")
    );
}

/// The harness's pops with the type of a pop unknown, so the engine cannot resolve a character's conditions.
struct Blind(Pops);
impl Populations for Blind {
    fn heads(&self, locator: &PopLocator) -> Option<u64> {
        self.0.heads(locator)
    }
    fn culture(&self, locator: &PopLocator) -> Option<String> {
        self.0.culture(locator)
    }
    fn kind(&self, _: &PopLocator) -> Option<String> {
        None
    }
    fn luxuries_pm(&self, locator: &PopLocator) -> Option<u16> {
        self.0.luxuries_pm(locator)
    }
    fn religion(&self, locator: &PopLocator) -> Option<String> {
        self.0.religion(locator)
    }
    fn necessities_pm(&self, locator: &PopLocator) -> Option<u16> {
        self.0.necessities_pm(locator)
    }
    fn genetics(&self, locator: &PopLocator) -> Option<PopGenetics> {
        self.0.genetics(locator)
    }
    fn distance_m(&self, a: &PopLocator, b: &PopLocator) -> Option<u64> {
        self.0.distance_m(a, b)
    }
    fn move_head(
        &self,
        from: &PopLocator,
        to: &PopLocator,
        minor: bool,
    ) -> Result<ck_economy::MoveUndo, ck_characters::Error> {
        self.0.move_head(from, to, minor)
    }
    fn restore_move(&self, undo: &ck_economy::MoveUndo) {
        self.0.restore_move(undo);
    }
}

#[test]
fn a_pair_that_grew_its_friendship_in_the_closed_year_keeps_it_while_the_rivalry_fades_in_january()
{
    // Two characters at one place in different pops (contact 0.1): the friendship grew in 767, the rivalry last in 760; saved in December.
    let lifecycle = rules().lifecycle.clone();
    let pops = Pops::new();
    let mut env = Env::new("r9-january");
    let mut characters = vec![man(1, 30, "h"), in_slot(man(2, 30, "h"), 1)];
    bond(&mut characters, 1, 2, "friend", 2, 767);
    bond(&mut characters, 1, 2, "rival", 2, 760);
    let mut store = reloaded(&store_of(characters));
    new_year(&mut store, &lifecycle, &mut env, &pops, 768);
    assert_eq!(strength(&store, 1, 2, "friend"), 2);
    assert_eq!(strength(&store, 1, 2, "rival"), 1);
    assert_eq!(get(&store, 1).ties.links[&2].resolved, 767);
    // 767 is resolved once.
    new_year(&mut store, &lifecycle, &mut env, &pops, 768);
    assert_eq!(strength(&store, 1, 2, "rival"), 1);
}

// --- Spec r10: the saved window, the edges and the output after the commit --------------------------------------------------------

#[test]
fn the_top_up_is_zero_at_its_edges() {
    let data = &rules().lifecycle.connections.passions;
    assert_eq!(
        ck_characters::top_up_chance_ppb(200_000_000, data.full_ppb, data.coarse_ppb),
        183_673_469
    );
    // The coarse tier is no weaker than the full one: nothing to add.
    assert_eq!(
        ck_characters::top_up_chance_ppb(200_000_000, 100_000_000, 100_000_000),
        0
    );
    assert_eq!(
        ck_characters::top_up_chance_ppb(200_000_000, 50_000_000, 100_000_000),
        0
    );
    // A certain coarse roll leaves nothing to add; a certain full tier over a partial coarse one is certain.
    assert_eq!(ck_characters::top_up_chance_ppb(PPB, PPB, PPB), 0);
    assert_eq!(ck_characters::top_up_chance_ppb(PPB, PPB, 500_000_000), PPB);
}

#[test]
fn a_young_character_never_full_gets_a_window_that_starts_after_its_birth_year() {
    let mut lifecycle = certain(&[]);
    for moment in &mut lifecycle.connections.passions.moments {
        if moment.key == "house_glory" {
            moment.chance_ppb = PPB;
        }
    }
    let pops = Pops::new();
    let mut env = Env::new("r10-young");
    // Born in 766, never full (last full roll 0), in a house with a living ruler and famous: promoted in 768, only 767 is its to backfill.
    let mut young = housed(person(1, 2, "h", Sex::Male), "ui_neill");
    young.personality.fame = 20_000;
    let mut store = store_of(vec![young, housed(ruler(man(2, 50, "h")), "ui_neill")]);
    store
        .settle_in(
            &mut world(&lifecycle, &mut env, &pops, 7),
            1,
            0,
            date(768, 3, 5),
        )
        .expect("settled in");
    let passion = &get(&store, 1).ties.passions[0];
    assert_eq!(
        passion.target,
        PassionTarget {
            kind: 2,
            key: "ui_neill".into()
        }
    );
    assert_eq!((passion.strength, passion.renewed), (3, date(767, 3, 3)));
    assert_eq!(get(&store, 1).ties.backfill, (0, 0));
    assert_eq!(get(&store, 1).ties.last_full, 767);
}

#[test]
fn a_backfill_that_fails_on_new_years_eve_uses_the_window_it_saved_then() {
    let mut lifecycle = certain(&[]);
    long_rule_with(&mut lifecycle, PPB);
    let pops = Pops::new();
    let mut env = Env::new("r10-window");
    let mut promoted = old_ruler(1);
    promoted.ties.last_full = 765;
    promoted.ties.promoted = Some(765);
    let mut store = store_of(vec![promoted]);
    store.fix_seed(99);
    // 31 December 768: the engine cannot resolve the pop, so nothing is granted; the window 766 to 767 is saved.
    let blind = Blind(Pops::new());
    assert!(
        store
            .backfill_promotions(
                &mut world(&lifecycle, &mut env, &blind, 7),
                date(768, 12, 31)
            )
            .is_err()
    );
    assert_eq!(get(&store, 1).ties.backfill, (766, 767));
    assert!(get(&store, 1).ties.passions.is_empty());
    // A save and reload, then 1 January 769: the saved window defines the years (766 and 767), not the new date (which would add 768).
    let mut store = reloaded(&store);
    store
        .backfill_promotions(&mut world(&lifecycle, &mut env, &pops, 7), date(769, 1, 1))
        .expect("backfilled");
    let held = get(&store, 1);
    assert_eq!(held.ties.passions[0].strength, 4);
    assert_eq!((held.ties.backfill, held.ties.last_full), ((0, 0), 767));
}

/// A life log that fails its next append.
struct FlakyLog {
    inner: LifeLog,
    fail: std::cell::Cell<u32>,
}
impl ck_characters::LogSink for FlakyLog {
    fn append(&mut self, id: u64, date: GameTime, text: &str) -> std::io::Result<()> {
        if self.fail.get() > 0 {
            self.fail.set(self.fail.get() - 1);
            return Err(std::io::Error::other("the log is unavailable"));
        }
        self.inner.append(id, date, text)
    }
}

#[test]
fn a_log_failure_after_the_backfill_commit_keeps_the_grants_once_and_is_not_retried() {
    let mut lifecycle = certain(&[]);
    long_rule_with(&mut lifecycle, PPB);
    let pops = Pops::new();
    let mut env = Env::new("r10-log");
    let mut promoted = old_ruler(1);
    promoted.ties.last_full = 765;
    promoted.ties.promoted = Some(765);
    let mut store = store_of(vec![promoted]);
    store.fix_seed(99);
    let mut logs = FlakyLog {
        inner: LifeLog::open(path("r10-log-logs")).expect("logs"),
        fail: std::cell::Cell::new(1),
    };
    let mut run = |store: &mut CharacterStore| {
        store.backfill_promotions(
            &mut World {
                rules: quiet(),
                personality: &rules().personality,
                genetics: &rules().genetics,
                lifecycle: &lifecycle,
                seed: 7,
                populations: &pops,
                archive: &mut env.archive,
                logs: &mut logs,
            },
            date(768, 6, 1),
        )
    };
    let result = run(&mut store);
    assert!(
        matches!(result, Err(ck_characters::EventError::Log(_))),
        "{result:?}"
    );
    // The grants stand once (766 and 767: 3 then 4) and the window is cleared.
    let held = get(&store, 1);
    assert_eq!(held.ties.passions[0].strength, 4);
    assert_eq!((held.ties.backfill, held.ties.last_full), ((0, 0), 767));
    // The next pass does not retry: nothing more is granted.
    run(&mut store).expect("nothing to backfill");
    assert_eq!(get(&store, 1).ties.passions[0].strength, 4);
}

#[test]
fn coarse_monks_then_promoted_meet_the_full_tiers_yearly_incidence() {
    // cleric_fervour at p = 0.5: a coarse year hits at 0.05, the backfill tops it up at 0.4737, and the incidence of at least one hit is 0.5.
    let mut lifecycle = certain(&[]);
    for moment in &mut lifecycle.connections.passions.moments {
        if moment.key == "cleric_fervour" {
            moment.chance_ppb = 500_000_000;
        }
    }
    let pops = Pops::new().kind("h", 0, "monk");
    let mut env = Env::new("r10-monks");
    let count = 800_u64;
    let monk = |id: u64| {
        let mut c = coarse(man(id, 40, "h"));
        c.life.skills.insert("theology".into(), 12_000);
        c.ties.in_pop_since = date(750, 1, 1);
        c.ties.last_full = 766;
        c
    };
    let mut store = store_of((1..=count).map(monk).collect());
    store.fix_seed(99);
    // The coarse year 767: every monk's birthday as a coarse character.
    store
        .run_birthdays(&mut world(&lifecycle, &mut env, &pops, 7), date(767, 3, 3))
        .expect("the coarse birthdays");
    // Promoted in 768, each is settled in for 767.
    for id in 1..=count {
        store
            .set_office(id, Some(Office::Heir), date(768, 1, 1))
            .expect("office");
        store
            .settle_in(
                &mut world(&lifecycle, &mut env, &pops, 7),
                id,
                766,
                date(768, 1, 1),
            )
            .expect("settled in");
    }
    let hit = store.iter().filter(|c| !c.ties.passions.is_empty()).count();
    assert!(hit.abs_diff(400) <= 45, "{hit}");
}

#[test]
fn the_backfill_commit_never_lowers_the_last_full_year_a_live_birthday_set() {
    let mut lifecycle = certain(&[]);
    long_rule_with(&mut lifecycle, PPB);
    let pops = Pops::new();
    let mut env = Env::new("r11-never-lowers");
    // A window of 766 to 767 saved by a failed December-style attempt, and a live full-tier birthday in 768 that ran while it was pending
    // (its last full-tier year is 768).
    let mut pending = old_ruler(1);
    pending.ties.last_full = 768;
    pending.ties.backfill = (766, 767);
    let mut store = store_of(vec![pending]);
    store.fix_seed(99);
    store
        .backfill_promotions(&mut world(&lifecycle, &mut env, &pops, 7), date(769, 1, 1))
        .expect("backfilled");
    let held = get(&store, 1);
    // The window's two years are granted (3 then 4), 768 is never topped up, and the last full-tier year stays at 768.
    assert_eq!(held.ties.passions[0].strength, 4);
    assert_eq!((held.ties.backfill, held.ties.last_full), ((0, 0), 768));
    // The next pass finds nothing owed for 768.
    store
        .backfill_promotions(&mut world(&lifecycle, &mut env, &pops, 7), date(769, 1, 2))
        .expect("nothing to do");
    assert_eq!(get(&store, 1).ties.passions[0].strength, 4);
}

// --- Review 11: the promotion is tracked, not inferred; a completed request is not repeated ----------------------------------------

fn house_glory_certain() -> LifeCycle {
    let mut lifecycle = certain(&[]);
    for moment in &mut lifecycle.connections.passions.moments {
        if moment.key == "house_glory" {
            moment.chance_ppb = PPB;
        }
    }
    lifecycle
}

fn famous_child(last_full: u16) -> CharacterStore {
    let mut young = coarse(housed(person(1, 2, "h", Sex::Male), "ui_neill"));
    young.personality.fame = 20_000;
    young.ties.last_full = last_full;
    let mut store = store_of(vec![young, housed(ruler(man(2, 50, "h")), "ui_neill")]);
    store.fix_seed(99);
    store
}

#[test]
fn a_never_full_character_promoted_by_an_office_is_found_by_the_first_daily_pass() {
    let lifecycle = house_glory_certain();
    let pops = Pops::new();
    let mut env = Env::new("r11-never-full");
    let mut store = famous_child(0);
    assert_eq!(get(&store, 1).ties.promoted, None);
    store
        .set_office(1, Some(Office::Heir), date(768, 2, 1))
        .expect("office");
    assert_eq!(
        get(&store, 1).ties.promoted,
        Some(0),
        "the promotion is recorded with its baseline"
    );
    let mut store = reloaded(&store);
    store
        .backfill_promotions(&mut world(&lifecycle, &mut env, &pops, 7), date(768, 2, 2))
        .expect("backfilled");
    let held = get(&store, 1);
    assert_eq!(held.ties.passions.len(), 1, "the 767 award is owed");
    assert_eq!(
        (held.ties.promoted, held.ties.backfill, held.ties.last_full),
        (None, (0, 0), 767)
    );
}

#[test]
fn a_character_that_stays_full_is_backfilled_once_for_its_one_promotion() {
    let lifecycle = house_glory_certain();
    let pops = Pops::new();
    let mut env = Env::new("r11-stays-full");
    // Prior progress 766; promoted on 1 April 768, after its March birthday.
    let mut store = famous_child(766);
    store
        .set_office(1, Some(Office::Heir), date(768, 4, 1))
        .expect("office");
    store
        .backfill_promotions(&mut world(&lifecycle, &mut env, &pops, 7), date(768, 4, 2))
        .expect("backfilled");
    let strength = get(&store, 1).ties.passions[0].strength;
    // Neither a later daily pass nor a reload, nor the next 1 January's, opens another window for the year in progress.
    for day in [date(768, 4, 3), date(768, 9, 9), date(769, 1, 1)] {
        let mut again = reloaded(&store);
        again
            .backfill_promotions(&mut world(&lifecycle, &mut env, &pops, 7), day)
            .expect("nothing due");
        assert_eq!(
            get(&again, 1).ties.passions[0].strength,
            strength,
            "{day:?}"
        );
        assert_eq!(get(&again, 1).ties.last_full, 767);
    }
}

#[test]
fn a_completed_settle_in_request_issued_again_awards_nothing() {
    let mut lifecycle = certain(&[]);
    long_rule_with(&mut lifecycle, PPB);
    let pops = Pops::new();
    let mut env = Env::new("r11-twice");
    let mut store = store_of(vec![old_ruler(1)]);
    store.fix_seed(99);
    store
        .settle_in(
            &mut world(&lifecycle, &mut env, &pops, 7),
            1,
            766,
            date(768, 6, 1),
        )
        .expect("settled in");
    assert_eq!(get(&store, 1).ties.passions[0].strength, 3);
    assert_eq!(get(&store, 1).ties.last_full, 767);
    let mut store = reloaded(&store);
    store
        .settle_in(
            &mut world(&lifecycle, &mut env, &pops, 7),
            1,
            766,
            date(768, 6, 1),
        )
        .expect("the same request again");
    assert_eq!(
        get(&store, 1).ties.passions[0].strength,
        3,
        "not awarded twice"
    );
}

// --- Review 12: the real transition, and the daily start owns the backfill --------------------------------------------------------

#[test]
fn regaining_a_qualification_during_grace_is_no_promotion() {
    let lifecycle = house_glory_certain();
    let pops = Pops::new();
    let mut env = Env::new("r12-grace");
    // Full by grace (it lost an office earlier), progress 766; a new ruler's office on 1 February 768 changes no tier.
    let store = famous_child(766);
    let mut graced = store.get(1).expect("live").clone();
    graced.full_until = Some(date(900, 1, 1));
    let mut store = {
        let mut keep = vec![graced];
        keep.push(get(&store, 2).clone());
        let mut s = store_of(keep);
        s.fix_seed(99);
        s
    };
    store
        .set_office(1, Some(Office::Ruler), date(768, 2, 1))
        .expect("office");
    assert_eq!(
        get(&store, 1).ties.promoted,
        None,
        "full on both sides of the change"
    );
    let mut store = reloaded(&store);
    store
        .backfill_promotions(&mut world(&lifecycle, &mut env, &pops, 7), date(768, 2, 2))
        .expect("nothing due");
    assert!(get(&store, 1).ties.passions.is_empty());
    assert_eq!(get(&store, 1).ties.last_full, 766);
}

#[test]
fn the_birthday_phase_neither_retries_a_pending_window_nor_backfills_a_promotion() {
    let lifecycle = house_glory_certain();
    let pops = Pops::new();
    let mut env = Env::new("r12-birthday-phase");
    // A window saved by a failed attempt earlier that day stays pending through the birthday phase of the same day.
    let mut pending = old_ruler(1);
    pending.ties.last_full = 766;
    pending.ties.backfill = (767, 767);
    let mut store = store_of(vec![pending]);
    store.fix_seed(99);
    birthdays(&mut store, &lifecycle, &mut env, &pops, date(768, 12, 31));
    assert_eq!(get(&store, 1).ties.backfill, (767, 767));
    assert!(get(&store, 1).ties.passions.is_empty());
    // A promotion that falls on the character's own birthday gives that birthday's live grant only; the older years wait for the next daily
    // pass, which then raises the passion and keeps the date and the progress of the live birthday.
    let mut store = famous_child(0);
    store
        .set_office(1, Some(Office::Heir), date(768, 3, 3))
        .expect("office");
    birthdays(&mut store, &lifecycle, &mut env, &pops, date(768, 3, 3));
    let held = get(&store, 1);
    assert_eq!(held.ties.passions[0].strength, 3, "the live birthday only");
    assert_eq!((held.ties.promoted, held.ties.last_full), (Some(0), 768));
    store
        .backfill_promotions(&mut world(&lifecycle, &mut env, &pops, 7), date(768, 3, 4))
        .expect("backfilled");
    let held = get(&store, 1);
    assert_eq!(
        held.ties.passions[0].strength, 4,
        "767 is topped up the next day"
    );
    assert_eq!(held.ties.passions[0].renewed, date(768, 3, 3));
    assert_eq!((held.ties.promoted, held.ties.last_full), (None, 768));
}

#[test]
fn a_promotion_on_new_years_eve_is_not_topped_up_for_the_year_its_live_birthday_rolled() {
    let lifecycle = house_glory_certain();
    let pops = Pops::new();
    let mut env = Env::new("r13-new-years-eve");
    // Born on 31 December: promoted that day by an office, its 768 birthday that day is a full-tier one (strength 3), and the first daily pass
    // is 1 January 769, whose last completed year is 768 again.
    let mut store = {
        let base = famous_child(0);
        let mut c = born_on(get(&base, 1).clone(), 12, 31);
        c.ties.last_full = 0;
        let mut s = store_of(vec![c, get(&base, 2).clone()]);
        s.fix_seed(99);
        s
    };
    store
        .set_office(1, Some(Office::Heir), date(768, 12, 31))
        .expect("office");
    birthdays(&mut store, &lifecycle, &mut env, &pops, date(768, 12, 31));
    assert_eq!(get(&store, 1).ties.passions[0].strength, 3);
    assert_eq!(
        (get(&store, 1).ties.promoted, get(&store, 1).ties.last_full),
        (Some(0), 768)
    );
    store
        .backfill_promotions(&mut world(&lifecycle, &mut env, &pops, 7), date(769, 1, 1))
        .expect("backfilled");
    let held = get(&store, 1);
    assert_eq!(
        held.ties.passions[0].strength, 4,
        "767 only; 768 was rolled live"
    );
    assert_eq!((held.ties.promoted, held.ties.last_full), (None, 768));
}

#[test]
fn the_lookback_counts_back_from_the_calendars_last_year_not_from_the_cropped_window() {
    let mut lifecycle = house_glory_certain();
    // One year of lookback: on 1 January 769 only 768 is in reach, and 768 was rolled live by the 31 December birthday, so nothing is owed.
    lifecycle.connections.max_years = 1;
    let pops = Pops::new();
    let mut env = Env::new("r14-lookback");
    let mut store = {
        let base = famous_child(0);
        let mut c = born_on(get(&base, 1).clone(), 12, 31);
        c.ties.last_full = 0;
        let mut s = store_of(vec![c, get(&base, 2).clone()]);
        s.fix_seed(99);
        s
    };
    store
        .set_office(1, Some(Office::Heir), date(768, 12, 31))
        .expect("office");
    birthdays(&mut store, &lifecycle, &mut env, &pops, date(768, 12, 31));
    store
        .backfill_promotions(&mut world(&lifecycle, &mut env, &pops, 7), date(769, 1, 1))
        .expect("backfilled");
    let held = get(&store, 1);
    assert_eq!(held.ties.passions[0].strength, 3, "767 is out of reach");
    assert_eq!((held.ties.promoted, held.ties.last_full), (None, 768));
}
