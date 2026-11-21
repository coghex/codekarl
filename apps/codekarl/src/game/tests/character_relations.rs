//! The relations record (CK-377): what the character screen's relations tab reads of one live character. The people are built as records, the
//! dead are written to a real archive file, and the record is built from the facts a published view carries; the shipped 768 start is read at
//! the end.

use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

use ck_characters::{
    Archive, ArchiveSink, ArchiveView, Character, CharacterStore, Departure, Office, PopLocator,
    PopTargets,
    ties::{Link, Passion, PassionTarget, Strand},
};
use ck_economy::Rules;
use ck_sim::GameTime;

use super::character_view::{
    houses, man, on, place_name, pop, rules, slots, store_with, today, woman, world,
};
use crate::{
    game::{
        character_relations::{End, FactsInputs, Person, Relations, RelationsFacts, Side, Tie},
        character_storage::Storage,
        character_view::units,
    },
    testing::Dir,
};

fn rules_arc() -> &'static Arc<Rules> {
    static RULES: OnceLock<Arc<Rules>> = OnceLock::new();
    RULES.get_or_init(|| Arc::new(rules().clone()))
}

/// A store, an archive on disk and the reader of that archive, in a scratch directory.
struct Fixture {
    _dir: Dir,
    path: std::path::PathBuf,
    archive: Archive,
    storage: Storage,
}

impl Fixture {
    fn new() -> Self {
        let dir = Dir::new();
        let storage = Storage::empty(&dir.0).expect("storage");
        let path = storage.path.join("ARCH");
        let archive = Archive::open(&path).expect("an archive");
        Self {
            _dir: dir,
            path,
            archive,
            storage,
        }
    }

    /// Archives `record` (it must have a death or a departure).
    fn bury(&mut self, record: &Character) {
        self.archive.append(record).expect("archived");
    }

    /// The archive as published now.
    fn view(&self) -> ArchiveView {
        self.storage.source().view().expect("a view of the archive")
    }

    fn facts(&self, live: Vec<Character>, slots: &BTreeMap<String, String>) -> RelationsFacts {
        facts_of(&store_with(live, &[]), Some(self.view()), slots)
    }
}

fn facts_of(
    store: &CharacterStore,
    archive: Option<ArchiveView>,
    slots: &BTreeMap<String, String>,
) -> RelationsFacts {
    RelationsFacts::build(
        store,
        &world().0,
        &FactsInputs {
            rules: rules_arc(),
            places: &Arc::new(world().1.clone()),
            houses: &Arc::new(houses().clone()),
            slots: &Arc::new(slots.clone()),
            archive,
            start_year: 768,
        },
        today(),
    )
}

/// `record` as one the archive holds: dead on `year`.
fn dead(mut record: Character, year: u16) -> Character {
    record.death = Some(on(year, 3, 4));
    record
}

fn born(mut record: Character, year: u16) -> Character {
    record.birth = on(year, 1, 1);
    record
}

fn ids(people: &[Person]) -> Vec<u64> {
    people.iter().map(|person| person.id).collect()
}

fn words(people: &[Person]) -> Vec<(u64, &str)> {
    people
        .iter()
        .map(|person| (person.id, person.relation.as_str()))
        .collect()
}

/// A link holding the strands `(kind index, strength)`.
fn link(strands: &[(usize, u8)]) -> Link {
    let mut link = Link::default();
    for (kind, strength) in strands {
        link.strands[*kind] = Strand {
            strength: *strength,
            grew: 767,
            faded: 0,
        };
    }
    link
}

const FRIEND: usize = 0;
const RIVAL: usize = 1;
const LOVER: usize = 2;
const LOVE: usize = 3;
const RESPECT: usize = 4;
const HATRED: usize = 7;

/// A bond between two characters: stored on both.
fn bond(a: &mut Character, b: &mut Character, kind: usize, strength: u8) {
    a.ties.links.entry(b.id).or_default().strands[kind].strength = strength;
    b.ties.links.entry(a.id).or_default().strands[kind].strength = strength;
}

fn record_of(facts: &RelationsFacts, id: u64) -> Relations {
    facts.record(id).expect("a record")
}

fn person_of(people: &[Person], id: u64) -> &Person {
    people
        .iter()
        .find(|person| person.id == id)
        .unwrap_or_else(|| panic!("no person {id} in {:?}", ids(people)))
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one fixture world and every row of the record read from it"
)]
fn the_record_holds_the_parents_spouses_in_stored_order_children_oldest_first_and_the_others_in_the_designers_order()
 {
    let place = pop("noble", 0);
    let mut fixture = Fixture::new();
    // Niall: a dead father (an archived record that lists no child), a live mother, a dead and a live wife, three children (one dead).
    let mut niall = born(man(1, &place), 718);
    niall.father = 2;
    niall.mother = 3;
    niall.spouses = vec![4, 5];
    niall.children = vec![6, 7, 8];
    fixture.bury(&dead(born(man(2, &place), 690), 722));
    let mut mother = born(woman(3, &place), 695);
    mother.children = vec![1, 9];
    let mut dead_wife = dead(born(woman(4, &place), 722), 760);
    dead_wife.spouses = vec![1];
    fixture.bury(&dead_wife);
    let mut wife = born(woman(5, &place), 727);
    wife.spouses = vec![1];
    wife.father = 12;
    wife.mother = 17;
    let mut dead_son = dead(born(man(7, &place), 748), 755);
    dead_son.father = 1;
    dead_son.mother = 4;
    fixture.bury(&dead_son);
    let mut son = born(man(6, &place), 750);
    son.father = 1;
    son.mother = 5;
    let mut daughter = born(woman(8, &place), 755);
    daughter.father = 1;
    // Kin who are not in the top lines: a brother through the archived father, a grandson, a nephew, the wife's father and brother.
    let mut brother = born(man(9, &place), 735);
    brother.father = 2;
    brother.mother = 3;
    let mut grandson = born(man(10, &place), 765);
    grandson.father = 6;
    let mut nephew = born(man(11, &place), 760);
    nephew.father = 9;
    let father_in_law = born(man(12, &place), 700);
    // The wife's mother is younger than her half-brother: the in-laws are one group, oldest first (father, brother, mother).
    let mother_in_law = born(woman(17, &place), 725);
    let mut brother_in_law = born(man(13, &place), 710);
    brother_in_law.father = 12;
    // Bonds: a lover, a rival and a friend, and the brother who is also a rival and the son who is also a lover (shown once, where kin).
    let mut friend = born(man(14, &place), 730);
    let mut rival = born(man(15, &place), 731);
    let mut lover = born(woman(16, &place), 732);
    bond(&mut niall, &mut friend, FRIEND, 2);
    bond(&mut niall, &mut rival, RIVAL, 5);
    bond(&mut niall, &mut lover, LOVER, 4);
    bond(&mut niall, &mut brother, RIVAL, 6);
    bond(&mut niall, &mut son, LOVER, 3);
    let facts = fixture.facts(
        vec![
            niall,
            mother,
            wife,
            son,
            daughter,
            brother,
            grandson,
            nephew,
            father_in_law,
            mother_in_law,
            brother_in_law,
            friend,
            rival,
            lover,
        ],
        &BTreeMap::new(),
    );
    let record = record_of(&facts, 1);
    // Parents: the father is dead (an archived record, by its stored name), the mother live.
    let father = record.parents[0].as_ref().expect("a father");
    assert_eq!((father.id, father.name.as_str()), (2, "M2"));
    assert!(matches!(father.end, End::Died(date) if date.year == 722));
    assert!(record.parents[1].as_ref().expect("a mother").live());
    // The spouses in the stored order, wives for a man, the dead one first.
    assert!(record.spouses_are_wives);
    assert_eq!(words(&record.spouses), [(4, "wife"), (5, "wife")]);
    assert!(!record.spouses[0].live() && record.spouses[1].live());
    // No strand, no tie icon: a cold marriage is an empty space; a spouse with a strand keeps the kin tie.
    assert_eq!(
        (record.spouses[0].tie, record.spouses[1].tie),
        (Tie::None, Tie::None)
    );
    // The children, oldest first, the dead son among them.
    assert_eq!(
        words(&record.children),
        [(7, "son"), (6, "son"), (8, "daughter")]
    );
    assert!(!record.children[0].live());
    // The others: lovers (the lover but not the son, who is shown above), rivals (the brother, strongest first, then the rival), friends, then
    // the living kin: grandsons, nephews, the wife's father, her brother. The brother keeps his tie and word.
    assert_eq!(
        words(&record.others),
        [
            (16, "lover"),
            (9, "brother"),
            (15, "rival"),
            (14, "friend"),
            (10, "grandson"),
            (11, "nephew"),
            (12, "father-in-law"),
            (13, "brother-in-law"),
            (17, "mother-in-law"),
        ]
    );
    let brother = person_of(&record.others, 9);
    assert_eq!(brother.tie, Tie::Kin);
    assert!(
        brother.opinion.is_some(),
        "a brother who is also a rival shows his opinion"
    );
    assert_eq!(person_of(&record.others, 15).tie, Tie::None);
    // A person already shown (the son) is not repeated.
    assert!(!ids(&record.others).contains(&6));
}

#[test]
fn an_abroad_parent_is_shown_as_abroad_and_an_unknown_one_as_nobody() {
    let place = pop("noble", 0);
    let mut fixture = Fixture::new();
    let mut abroad = born(man(41, &place), 700);
    abroad.life.departure = Departure::Abroad;
    abroad.pop = PopLocator {
        place: String::new(),
        slot: 0,
    };
    fixture.bury(&abroad);
    let mut subject = born(man(40, &place), 730);
    subject.father = 41;
    let record = record_of(&fixture.facts(vec![subject], &BTreeMap::new()), 40);
    assert_eq!(
        record.parents[0].as_ref().map(|father| father.end),
        Some(End::Abroad)
    );
    assert!(record.parents[1].is_none(), "the unknown mother is no one");
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one fixture world and every row of the record read from it"
)]
fn foster_kin_and_hostages_are_read_from_the_courts_anchor_ruler_in_both_directions() {
    let court = pop("noble", 0);
    let elsewhere = pop("noble", 1);
    let mut fixture = Fixture::new();
    let slot_map: BTreeMap<String, String> = [
        ("s_foster", court.place.as_str()),
        ("s_hostage", court.place.as_str()),
        ("s_foster2", court.place.as_str()),
        ("s_hostage0", court.place.as_str()),
        ("s_far", elsewhere.place.as_str()),
    ]
    .into_iter()
    .map(|(key, place)| (key.to_owned(), place.to_owned()))
    .collect();
    // The anchor: of three rulers at the court, the lowest figure key (`a_king`); a ruler with no key does not beat a figure.
    let mut anchor = born(man(20, &court), 725);
    anchor.office = Some(Office::Ruler);
    anchor.life.figure = "a_king".to_owned();
    anchor.spouses = vec![22, 21];
    anchor.children = vec![23];
    let mut second = born(man(24, &court), 726);
    second.office = Some(Office::Ruler);
    second.life.figure = "b_king".to_owned();
    let mut unkeyed = born(man(25, &court), 727);
    unkeyed.office = Some(Office::Ruler);
    // His first wife is dead, so his first LIVING spouse is the second: she is the foster-mother.
    let mut first_wife = dead(born(woman(22, &court), 730), 750);
    first_wife.spouses = vec![20];
    fixture.bury(&first_wife);
    let mut wife = born(woman(21, &court), 735);
    wife.spouses = vec![20];
    let mut child = born(man(23, &court), 755);
    child.father = 20;
    child.mother = 21;
    let mut fosterling = born(man(30, &court), 752);
    fosterling.life.slot_key = "s_foster".to_owned();
    fosterling.life.slot_kind = "fosterling".to_owned();
    let mut hostage = born(man(31, &court), 760);
    hostage.life.slot_key = "s_hostage".to_owned();
    hostage.life.slot_kind = "hostage".to_owned();
    // More guests whose ids and ages interleave: an older fosterling with a higher id, and a hostage with a lower id who is younger than the
    // first. Foster kin come first, oldest first; then the hostages, oldest first.
    let mut older = born(man(33, &court), 740);
    older.life.slot_key = "s_foster2".to_owned();
    older.life.slot_kind = "fosterling".to_owned();
    let mut younger_hostage = born(man(29, &court), 761);
    younger_hostage.life.slot_key = "s_hostage0".to_owned();
    younger_hostage.life.slot_kind = "hostage".to_owned();
    // A fosterling at a court with no ruler has no foster-father, foster-mother, brothers or holder.
    let mut orphan = born(man(32, &elsewhere), 753);
    orphan.life.slot_key = "s_far".to_owned();
    orphan.life.slot_kind = "fosterling".to_owned();
    let facts = fixture.facts(
        vec![
            anchor,
            second,
            unkeyed,
            wife,
            child,
            fosterling,
            hostage,
            orphan,
            older,
            younger_hostage,
        ],
        &slot_map,
    );
    // The fosterling: foster-father, foster-mother, foster-brother.
    let fostered = record_of(&facts, 30);
    assert_eq!(
        words(&fostered.others),
        [
            (20, "foster-father"),
            (21, "foster-mother"),
            (23, "foster-brother")
        ]
    );
    assert!(
        fostered
            .others
            .iter()
            .all(|person| person.tie == Tie::Foster)
    );
    // The same anchor from the other side: the ruler sees his foster-son and his hostage; his wife her foster-son; his son a foster-brother.
    assert_eq!(
        words(&record_of(&facts, 20).others),
        [
            (33, "foster-son"),
            (30, "foster-son"),
            (31, "hostage"),
            (29, "hostage")
        ]
    );
    assert_eq!(
        person_of(&record_of(&facts, 20).others, 31).tie,
        Tie::Hostage
    );
    assert_eq!(
        words(&record_of(&facts, 21).others),
        [(33, "foster-son"), (30, "foster-son")]
    );
    assert_eq!(
        words(&record_of(&facts, 23).others),
        [(33, "foster-brother"), (30, "foster-brother")]
    );
    // The hostage's row names its holder.
    let held = record_of(&facts, 31);
    assert_eq!(words(&held.others), [(20, "holder")]);
    assert_eq!(held.others[0].tie, Tie::Hostage);
    // No anchor, no tie.
    assert!(record_of(&facts, 32).others.is_empty());
    // The other rulers of the court see no guests.
    assert!(record_of(&facts, 24).others.is_empty());
    assert!(record_of(&facts, 25).others.is_empty());
}

#[test]
fn strands_run_both_ways_with_their_strengths_and_the_opinion_is_each_ones_own() {
    let place = pop("noble", 0);
    let mut niall = born(man(1, &place), 718);
    let mut other = born(man(2, &place), 730);
    bond(&mut niall, &mut other, FRIEND, 3);
    // Niall's feeling toward him, and his toward Niall: a feeling is held by its holder alone.
    niall.ties.links.entry(2).or_default().strands[RESPECT].strength = 4;
    other.ties.links.entry(1).or_default().strands[HATRED].strength = 7;
    let store = store_with(vec![niall.clone(), other.clone()], &[]);
    let record = record_of(&facts_of(&store, None, &BTreeMap::new()), 1);
    assert!(record.children.is_empty());
    // He is a stranger to Niall by kin; he is reached through the strand (a friend).
    let person = person_of(&record.others, 2);
    let lines: Vec<_> = person
        .lines
        .iter()
        .map(|line| (line.kind, line.strength, line.side))
        .collect();
    assert_eq!(
        lines,
        [
            ("friend", 3, Side::Both),
            ("respect", 4, Side::Mine),
            ("hatred", 7, Side::Theirs)
        ],
        "bonds first, then Niall's feelings, then his"
    );
    // The opinion each way is the simulation's own, whole points, from the two live records.
    let rules = rules();
    let toward = |a: &Character, b: &Character| {
        units(
            ck_characters::opinion(
                &rules.personality,
                &rules.lifecycle.connections,
                a,
                b,
                &PopTargets::of(&world().0, b),
            )
            .expect("an opinion"),
        )
    };
    let (mine, back) = person.opinion.expect("both live");
    assert_eq!(
        (mine, back),
        (toward(&niall, &other), toward(&other, &niall))
    );
    assert_ne!(
        mine, back,
        "an asymmetric pair holds a different opinion each way"
    );
}

#[test]
fn passions_are_ordered_strongest_first_then_by_kind_and_target_and_name_their_targets() {
    let place = pop("noble", 0);
    let mut niall = born(man(1, &place), 718);
    let passion = |kind: u8, target: u8, key: &str, strength: u8| Passion {
        kind,
        target: PassionTarget {
            kind: target,
            key: key.to_owned(),
        },
        strength,
        renewed: GameTime::from_hours(0),
        last_fade: GameTime::from_hours(0),
    };
    // loyalty to a place, love for a house, hatred of a culture, fear of a religion; two of strength 8; a minor house by its saved name; a
    // house nothing names.
    niall.ties.passions = vec![
        passion(0, 0, &place.place, 5),
        passion(2, 3, "gaelic", 8),
        passion(1, 2, "cenel_neogain", 8),
        passion(3, 1, "christianity", 3),
        passion(1, 2, "minor_house_7", 3),
        passion(1, 2, "no_such_house", 2),
    ];
    let store = store_with(vec![niall], &[("minor_house_7", "Saved Minor Name")]);
    let record = record_of(&facts_of(&store, None, &BTreeMap::new()), 1);
    let rows: Vec<_> = record
        .passions
        .iter()
        .map(|p| (p.kind, p.target_kind, p.strength, p.name.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("love", "house", 8, "Cen\u{e9}l nE\u{f3}gain"),
            ("hatred", "culture", 8, ""),
            ("loyalty", "place", 5, place_name(&place)),
            ("love", "house", 3, "Saved Minor Name"),
            ("fear", "religion", 3, ""),
            ("love", "house", 2, "no_such_house"),
        ]
    );
}

#[test]
fn the_dead_are_history_what_their_own_record_holds_and_no_living_partners_feelings() {
    let place = pop("noble", 0);
    let mut fixture = Fixture::new();
    let mut niall = born(man(1, &place), 718);
    niall.spouses = vec![2, 3];
    // A wife who died in play: her own record kept a bond and her own feeling, and the husband's authored feeling toward her (history the start
    // stored). Niall's own record holds a feeling toward her too: a death removes it, and it is never read.
    let mut dead_wife = dead(born(woman(2, &place), 722), 766);
    dead_wife.spouses = vec![1];
    dead_wife
        .ties
        .links
        .insert(1, link(&[(FRIEND, 3), (HATRED, 2)]));
    dead_wife
        .ties
        .inbound
        .push(ck_characters::ties::InboundFeeling {
            holder: 1,
            kind: u8::try_from(LOVE).unwrap(),
            strength: 4,
        });
    fixture.bury(&dead_wife);
    // A figure who died before the start, no date known.
    let mut unknown = born(woman(3, &place), 700);
    unknown.death = None;
    unknown.life.departure = Departure::DeathUnknown;
    unknown.spouses = vec![1];
    fixture.bury(&unknown);
    niall.ties.links.insert(2, link(&[(RESPECT, 6)]));
    let record = record_of(&fixture.facts(vec![niall], &BTreeMap::new()), 1);
    let wife = &record.spouses[0];
    assert_eq!(
        wife.tie,
        Tie::Kin,
        "a spouse with a strand keeps the kin tie"
    );
    assert_eq!(record.spouses[1].tie, Tie::None);
    assert!(matches!(wife.end, End::Died(date) if date.year == 766));
    let lines: Vec<_> = wife
        .lines
        .iter()
        .map(|line| (line.kind, line.strength, line.side, line.history))
        .collect();
    assert_eq!(
        lines,
        [
            ("friend", 3, Side::Both, true),
            ("love", 4, Side::Mine, true),
            ("hatred", 2, Side::Theirs, true),
        ],
        "her bond, the authored feeling toward her, her own feeling: all history; Niall's own record's respect is not read"
    );
    assert_eq!(wife.opinion, None, "no opinion of the dead");
    // The unknown date reads as before the start.
    assert_eq!(record.spouses[1].end, End::DiedBefore(768));
    assert!(record.spouses[1].lines.is_empty());
}

#[test]
fn an_archived_parent_that_lists_no_child_still_gives_its_live_children_a_sibling() {
    // Covered with the full row above; here the index alone: a live sibling is found through an archived parent whose record lacks the
    // backlink, and an unrelated crowd changes nothing about a record.
    let place = pop("noble", 0);
    let mut fixture = Fixture::new();
    fixture.bury(&dead(born(man(2, &place), 690), 722));
    let mut subject = born(man(1, &place), 718);
    subject.father = 2;
    let mut sibling = born(woman(9, &place), 720);
    sibling.father = 2;
    let crowd = |count: u64| -> Vec<Character> {
        (100..100 + count)
            .map(|id| {
                born(
                    man(id, &pop("peasant", 0)),
                    700 + u16::try_from(id % 50).unwrap(),
                )
            })
            .collect()
    };
    let small = fixture.facts(vec![subject.clone(), sibling.clone()], &BTreeMap::new());
    let mut with_crowd = vec![subject, sibling];
    with_crowd.extend(crowd(300));
    let large = fixture.facts(with_crowd, &BTreeMap::new());
    let record = record_of(&small, 1);
    assert_eq!(words(&record.others), [(9, "sister")]);
    assert_eq!(
        record_of(&large, 1),
        record,
        "a record is built from its own links: a crowd of unrelated characters changes nothing"
    );
}

#[test]
fn a_view_reads_the_archive_as_published_and_an_old_world_stays_readable_while_a_view_of_it_lives()
{
    let place = pop("noble", 0);
    let mut old = Fixture::new();
    let mut seven = dead(born(man(7, &place), 690), 700);
    seven.name = "Old Seven".to_owned();
    old.bury(&seven);
    let first = old.view();
    // A record archived later is not in the view made before it, and is in the next.
    old.bury(&dead(born(man(8, &place), 691), 701));
    assert!(!first.contains(8) && first.get(8).unwrap().is_none());
    let second = old.view();
    assert!(second.contains(8) && second.contains(7));
    // Another world whose archive reuses the id 7: each view reads its own.
    let mut new = Fixture::new();
    let mut other_seven = dead(born(man(7, &place), 640), 650);
    other_seven.name = "New Seven".to_owned();
    new.bury(&other_seven);
    let fresh = new.view();
    assert_eq!(fresh.get(7).unwrap().unwrap().name, "New Seven");
    assert_eq!(first.get(7).unwrap().unwrap().name, "Old Seven");
    // The old world's storage is let go (a load replaced it): a view of it still reads, and the files go with the last view.
    let path = old.path.clone();
    let Fixture { storage, .. } = old;
    drop(storage);
    assert!(path.exists(), "a pinned view keeps its archive's files");
    assert_eq!(second.get(7).unwrap().unwrap().name, "Old Seven");
    drop(first);
    assert!(path.exists());
    drop(second);
    assert!(!path.exists(), "released when the last view retires");
}

#[test]
fn game_character_relations_refuses_by_name_and_answers_from_its_revision() {
    use ck_lua::ConfigValue;

    use crate::{
        game::view::EconomyView,
        game_api::{ViewCache, character_relations_function},
    };

    let place = pop("noble", 0);
    let mut fixture = Fixture::new();
    fixture.bury(&dead(born(man(2, &place), 690), 722));
    let mut subject = born(man(1, &place), 718);
    subject.father = 2;
    let store = store_with(vec![subject], &[]);
    let mut view = EconomyView::default();
    view.characters.relations = facts_of(&store, Some(fixture.view()), &BTreeMap::new());
    let cache = Arc::new(ViewCache::default());
    cache.post(7, Arc::new(view.clone()));
    let relations = character_relations_function(Arc::clone(&cache));
    let integer = |value: i64| Some(ConfigValue::Integer(value));
    let answer = relations(&[integer(1), integer(7)]).expect("a record");
    let ConfigValue::Table(fields) = &answer[0] else {
        panic!("a table")
    };
    assert!(fields.contains_key("father") && !fields.contains_key("mother"));
    for key in ["spouse_word", "spouses", "children", "others", "passions"] {
        assert!(fields.contains_key(key), "{key}");
    }
    // An unknown id and a character who is archived, not live: the same refusal. A revision never given; one that has left.
    for id in [99, 2] {
        assert_eq!(
            relations(&[integer(id), integer(7)]).unwrap_err(),
            format!("game.character_relations: no character {id} at revision 7")
        );
    }
    assert_eq!(
        relations(&[integer(1), integer(8)]).unwrap_err(),
        "game.character_relations: revision 8 is missing"
    );
    cache.complete(7);
    cache.post(8, Arc::new(view));
    cache.complete(8);
    assert_eq!(
        relations(&[integer(1), integer(7)]).unwrap_err(),
        "game.character_relations: revision 7 is retired"
    );
}

#[test]
fn a_later_archive_write_leaves_an_earlier_view_with_its_live_relationship() {
    let place = pop("noble", 0);
    let mut fixture = Fixture::new();
    let mut father = born(man(1, &place), 718);
    father.children = vec![2];
    let mut child = born(man(2, &place), 750);
    child.father = 1;
    bond(&mut father, &mut child, FRIEND, 3);
    let early = fixture.facts(vec![father.clone(), child.clone()], &BTreeMap::new());
    // The son dies and is archived (a later write to the same archive); the father's later view has him dead, the earlier one unchanged.
    fixture.bury(&dead(child, 768));
    let mut widowed = father;
    widowed.ties.links.clear();
    let later = fixture.facts(vec![widowed], &BTreeMap::new());
    let before = record_of(&early, 1);
    assert!(before.children[0].live() && before.children[0].opinion.is_some());
    assert_eq!(before.children[0].lines.len(), 1);
    let after = record_of(&later, 1);
    assert!(!after.children[0].live() && after.children[0].opinion.is_none());
    // The earlier view still answers from its own world and publication after the later one was made.
    assert_eq!(record_of(&early, 1), before);
    let early_archive = early.archive_for_tests().expect("an archive");
    assert!(
        !early_archive.contains(2),
        "the record archived later is not in the earlier view"
    );
    assert!(later.archive_for_tests().expect("an archive").contains(2));
    // A new world reusing the id: its subject has no father, while the earlier view of the old world still has one.
    let other = Fixture::new();
    let new_world = other.facts(vec![born(man(1, &place), 600)], &BTreeMap::new());
    assert!(record_of(&new_world, 1).children.is_empty());
    assert!(!record_of(&early, 1).children.is_empty());
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the real start's data, the real functions and the shipped Lua in one witness"
)]
fn the_shipped_ui_shows_niall_frossachs_real_relations_tab_with_the_dead_inert() {
    use ck_lua::ConfigValue;

    use crate::{
        game::{
            character_view::{Inputs, build},
            view::EconomyView,
        },
        game_api::{
            ViewCache, character_function, character_relations_function, characters_function,
        },
    };

    let (_dir, prepared, _) = super::start_768::shipped(7);
    let state = prepared.simulation.state();
    let date = prepared.simulation.date().expect("a date");
    let mut view = build(
        &state.characters,
        &state.economy,
        &Inputs {
            rules: rules(),
            places: &crate::game::setup::place_names(&prepared.scenario),
            houses: houses(),
            slots: slots(),
        },
        date,
    );
    view.relations = RelationsFacts::build(
        &state.characters,
        &state.economy,
        &FactsInputs {
            rules: rules_arc(),
            places: &Arc::new(crate::game::setup::place_names(&prepared.scenario)),
            houses: &Arc::new(houses().clone()),
            slots: &Arc::new(slots().clone()),
            archive: Some(prepared.storage.source().view().expect("the archive")),
            start_year: 768,
        },
        date,
    );
    let niall = state
        .characters
        .iter()
        .find(|c| c.life.figure == "niall_frossach")
        .expect("Niall");
    let rank = view.card(niall.id).expect("his card").importance as usize;
    let cache = Arc::new(ViewCache::default());
    cache.post(
        1,
        Arc::new(EconomyView {
            characters: view,
            ..EconomyView::default()
        }),
    );
    let id = i64::try_from(niall.id).expect("a small id");
    let arguments = [
        Some(ConfigValue::Integer(id)),
        Some(ConfigValue::Integer(1)),
    ];
    let cards =
        characters_function(Arc::clone(&cache))(&[Some(ConfigValue::Integer(1))]).expect("cards");
    let record = character_function(Arc::clone(&cache))(&arguments).expect("his record");
    let relations = character_relations_function(cache)(&arguments).expect("his relations");
    let (notes, portraits, after_press) =
        crate::ui_link::location_tests::character_relations::real_relations(
            &super::character_view::lua_of(&cards[0]),
            &super::character_view::lua_of(&record[0]),
            &super::character_view::lua_of(&relations[0]),
            id,
            rank,
        );
    let ConfigValue::Table(fields) = &relations[0] else {
        panic!("a table")
    };
    let count = |key: &str| match &fields[key] {
        ConfigValue::Array(items) => items.len(),
        other => panic!("{key}: {other:?}"),
    };
    // Parents (both), the spouses, the children and the others: one portrait each, and every tooltip resolves. The others row holds two rows of
    // seven: past that the last place is the "+N" chip, which is a place to hover too.
    let expected = 2 + count("spouses") + count("children") + count("others").min(14);
    assert_eq!(portraits.len(), expected, "{notes:?}");
    if count("others") > 14 {
        let chip = portraits.last().expect("a chip");
        assert!(
            chip.3
                .starts_with(&format!("+{} more / ", count("others") - 13))
                && !chip.2,
            "{}",
            chip.3
        );
    }
    assert!(
        portraits
            .iter()
            .all(|(_, _, _, tip)| !tip.is_empty() && !tip.starts_with("<no tooltip>")),
        "every portrait answers a tooltip through the stack's own provider"
    );
    // The first is the archived father: shown by his stored display name, dead, and a press opens nothing. His wife Dunlaith is live and opens.
    assert!(
        !portraits[0].2,
        "the dead father is inert: {:?}",
        portraits[0]
    );
    assert!(portraits[0].3.contains("father"), "{}", portraits[0].3);
    assert!(
        portraits[1].2 && portraits[1].3.contains("mother"),
        "{:?}",
        portraits[1]
    );
    let spouses: Vec<_> = portraits
        .iter()
        .filter(|p| p.3.contains(" / wife / "))
        .collect();
    assert_eq!(spouses.len(), 2, "{portraits:?}");
    assert!(
        !spouses[0].2 && spouses[1].2,
        "Eithne inert, Dunlaith clickable: {spouses:?}"
    );
    assert!(
        after_press.starts_with(&format!("screen id={id} tab=relations")),
        "a press on a dead portrait leaves the screen as it was: {after_press}"
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "two real games, a real quicksave and quickload and the real functions over both worlds in one story"
)]
fn a_real_load_publishes_the_new_world_and_a_pinned_old_revision_still_reads_its_own() {
    use ck_lua::ConfigValue;

    use crate::{
        game::{
            controls::Control,
            core::{Message, Outputs},
            setup::test_core,
        },
        game_api::{ViewCache, character_relations_function},
    };

    let (dir_a, a, _) = super::start_768::shipped(7);
    let (_dir_b, b, _) = super::start_768::shipped(8);
    let saves = dir_a.0.join("saves");
    let (path_a, path_b) = (a.storage.path.clone(), b.storage.path.clone());
    // Two worlds whose archives reuse an id for different people: a live character of each whose archived parent is that id.
    let archive_a = Archive::open(path_a.join("ARCH")).expect("archive a");
    let archive_b = Archive::open(path_b.join("ARCH")).expect("archive b");
    let mut chosen = None;
    'search: for x in a.simulation.state().characters.iter() {
        for parent in [x.father, x.mother] {
            if parent == 0 || !archive_a.contains(parent) || !archive_b.contains(parent) {
                continue;
            }
            let name_a = archive_a.get(parent).unwrap().unwrap().name;
            let name_b = archive_b.get(parent).unwrap().unwrap().name;
            if name_a == name_b {
                continue;
            }
            if let Some(y) = b
                .simulation
                .state()
                .characters
                .iter()
                .find(|y| y.father == parent || y.mother == parent)
            {
                chosen = Some((x.id, y.id, parent, name_a, name_b));
                break 'search;
            }
        }
    }
    let (x, y, parent, name_a, name_b) = chosen.expect("a reused archive id");
    let quiet = || Outputs {
        log: Box::new(|_, _| {}),
        publish: Box::new(|_| {}),
        fail: Box::new(|message| panic!("{message}")),
    };
    let mut core_a = test_core(a, saves.clone(), quiet());
    let mut core_b = test_core(b, saves, quiet());
    // World B is saved; A is loaded from that save: a real load, in the core, with its storage and its view builder.
    core_b.dispatch(
        Message::Control(Control::Quicksave),
        std::time::Duration::ZERO,
    );
    let old = core_a.snapshot();
    assert_eq!(old.world, 0);
    let old_view = old.economy.clone().expect("a view");
    let parent_name = |relations: &Relations, id: u64| -> String {
        relations
            .parents
            .iter()
            .flatten()
            .find(|person| person.id == id)
            .unwrap_or_else(|| panic!("parent {id}"))
            .name
            .clone()
    };
    assert_eq!(
        parent_name(&old_view.characters.relations_of(x).unwrap(), parent),
        name_a
    );
    core_a.dispatch(
        Message::Control(Control::Quickload),
        std::time::Duration::ZERO,
    );
    let new = core_a.snapshot();
    assert_eq!(new.world, 1, "the load replaced the world");
    assert!(new.economy_revision > old.economy_revision);
    let new_view = new.economy.clone().expect("a view");
    // The new view reads the new world's archive, the old (pinned) one its own; the same id names two people.
    assert_eq!(
        parent_name(&new_view.characters.relations_of(y).unwrap(), parent),
        name_b
    );
    assert_eq!(
        parent_name(&old_view.characters.relations_of(x).unwrap(), parent),
        name_a
    );
    // Through the real function with the old revision pinned (never completed) and the new one posted after it.
    let cache = Arc::new(ViewCache::default());
    cache.post(old.economy_revision, Arc::clone(&old_view));
    cache.post(new.economy_revision, Arc::clone(&new_view));
    let relations = character_relations_function(Arc::clone(&cache));
    let father_name = |id: u64, revision: u64| -> String {
        let answer = relations(&[
            Some(ConfigValue::Integer(i64::try_from(id).unwrap())),
            Some(ConfigValue::Integer(i64::try_from(revision).unwrap())),
        ])
        .expect("the read completes");
        let ConfigValue::Table(fields) = &answer[0] else {
            panic!("a table")
        };
        ["father", "mother"]
            .iter()
            .find_map(|key| match fields.get(*key) {
                Some(ConfigValue::Table(person))
                    if person["id"] == ConfigValue::Integer(i64::try_from(parent).unwrap()) =>
                {
                    match &person["name"] {
                        ConfigValue::String(name) => Some(name.clone()),
                        _ => None,
                    }
                }
                _ => None,
            })
            .expect("the shared parent")
    };
    assert_eq!(father_name(x, old.economy_revision), name_a);
    assert_eq!(father_name(y, new.economy_revision), name_b);
    // The old world's files stay while a view of it lives, and are released with the last one.
    assert!(path_a.exists() && path_b.exists());
    drop((old, old_view, relations, cache));
    assert!(
        !path_a.exists(),
        "the old world's storage is released with its last view"
    );
    assert!(
        path_b.exists(),
        "the game that was loaded from is still running"
    );
    drop((new, new_view));
}
