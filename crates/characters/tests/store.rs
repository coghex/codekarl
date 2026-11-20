//! Character store contracts.

use ck_characters::{
    CharacterStore, Context, Error, NewCharacter, Office, PopLocator, Populations, Sex, Tier,
    UNKNOWN, character_stream,
};
use ck_economy::Rules;
use ck_sim::{Canonical, Date, GameTime};
use std::sync::OnceLock;

fn date(year: u16, month: u8, day: u8) -> GameTime {
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour: 0,
    })
    .expect("valid date")
}
fn pop() -> PopLocator {
    PopLocator {
        place: "a".into(),
        slot: 0,
    }
}
fn rules_all() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        Rules::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
        )
        .expect("shipped rules")
    })
}
/// Every pop is Irish (`ulaid`); the head count comes from the wrapped `Populations`.
struct Irish<'a, P>(&'a P);
impl<P: Populations> Populations for Irish<'_, P> {
    fn heads(&self, locator: &PopLocator) -> Option<u64> {
        self.0.heads(locator)
    }
    fn culture(&self, _: &PopLocator) -> Option<String> {
        Some("ulaid".to_owned())
    }
    fn distance_m(&self, a: &PopLocator, b: &PopLocator) -> Option<u64> {
        self.0.distance_m(a, b)
    }
    fn move_head(
        &self,
        from: &PopLocator,
        to: &PopLocator,
        minor: bool,
    ) -> Result<ck_economy::MoveUndo, Error> {
        self.0.move_head(from, to, minor)
    }
    fn restore_move(&self, undo: &ck_economy::MoveUndo) {
        self.0.restore_move(undo);
    }
}
/// The slice 1 tests make plain characters; they are founders of an Irish pop under the shipped genetic rules.
trait CreateFounder {
    fn create(
        &mut self,
        name: String,
        sex: Sex,
        birth: GameTime,
        pop: PopLocator,
        populations: &impl Populations,
    ) -> Result<u64, Error>;
}
impl CreateFounder for CharacterStore {
    fn create(
        &mut self,
        name: String,
        sex: Sex,
        birth: GameTime,
        pop: PopLocator,
        populations: &impl Populations,
    ) -> Result<u64, Error> {
        let rules = &rules_all().genetics;
        let context = Context {
            rules,
            personality: &rules_all().personality,
            events: &rules_all().events,
            lifecycle: &rules_all().lifecycle,
            seed: 7,
            tick: birth,
        };
        let state = rules.seed_pop("ulaid").expect("Irish start set");
        self.create_founder(
            &context,
            NewCharacter {
                name,
                sex,
                birth,
                pop,
                house: String::new(),
            },
            &state,
            &Irish(populations),
        )
    }
}

#[test]
fn empty_store_and_streams_are_deterministic() {
    let store = CharacterStore::default();
    assert_eq!(
        store.to_bytes().expect("encode"),
        [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    let mut a = character_stream(5, 1, "birth", date(800, 1, 1));
    let first = a.draw();
    assert_ne!(first, a.draw());
    assert_eq!(
        first,
        character_stream(5, 1, "birth", date(800, 1, 1)).draw()
    );
    assert_ne!(
        first,
        character_stream(5, 1, "death", date(800, 1, 1)).draw()
    );
    assert_ne!(
        first,
        character_stream(5, 1, "birth", date(800, 1, 2)).draw()
    );
}

#[test]
fn ruler_relations_lose_last_qualification_on_replacement_and_death() {
    let mut s = CharacterStore::default();
    let r = s
        .create(
            "r".into(),
            Sex::Male,
            date(770, 1, 1),
            pop(),
            &|_: &PopLocator| Some(3),
        )
        .expect("create");
    let wife = s
        .create(
            "w".into(),
            Sex::Female,
            date(770, 1, 1),
            pop(),
            &|_: &PopLocator| Some(3),
        )
        .expect("create");
    let child = s
        .create(
            "c".into(),
            Sex::Male,
            date(790, 1, 1),
            pop(),
            &|_: &PopLocator| Some(3),
        )
        .expect("create");
    s.state_marry(r, wife, date(800, 1, 1)).expect("marry");
    s.set_parents(child, r, UNKNOWN, date(800, 1, 1))
        .expect("parents");
    s.set_office(r, Some(Office::Ruler), date(800, 1, 1))
        .expect("office");
    assert_eq!(s.tier(wife, date(800, 1, 1)), Some(Tier::Full));
    s.set_office(r, Some(Office::Heir), date(800, 2, 29))
        .expect("office");
    assert_eq!(s.tier(r, date(805, 3, 1)), Some(Tier::Full));
    assert_eq!(s.tier(wife, date(805, 2, 28)), Some(Tier::Full));
    assert_eq!(s.tier(wife, date(805, 3, 1)), Some(Tier::Coarse));
    assert_eq!(s.tier(child, date(805, 3, 1)), Some(Tier::Coarse));
}

#[test]
fn capacity_and_roundtrip_preserve_ids_and_locators() {
    let mut s = CharacterStore::default();
    let id = s
        .create(
            "A".into(),
            Sex::Female,
            date(770, 1, 1),
            pop(),
            &|_: &PopLocator| Some(1),
        )
        .expect("create");
    assert!(
        s.create(
            "B".into(),
            Sex::Male,
            date(770, 1, 1),
            pop(),
            &|_: &PopLocator| Some(1)
        )
        .is_err()
    );
    let bytes = s.to_bytes().expect("encode");
    let mut loaded = CharacterStore::from_bytes(&bytes).expect("decode");
    assert_eq!(s, loaded);
    assert_eq!(loaded.get(id).expect("live").pop, pop());
    assert_eq!(
        loaded
            .create(
                "B".into(),
                Sex::Male,
                date(770, 1, 1),
                pop(),
                &|_: &PopLocator| Some(2)
            )
            .expect("create"),
        2
    );
}

#[test]
fn death_is_transactional_and_dead_parent_remains_readable() {
    use ck_characters::{Archive, ArchiveSink};
    struct Fail;
    impl ArchiveSink for Fail {
        fn append(&mut self, _: &ck_characters::Character) -> std::io::Result<()> {
            Err(std::io::Error::other("injected"))
        }
        // A sink that holds no records: no parent has an archived child.
        fn children_of(&self, _: u64) -> std::io::Result<Vec<u64>> {
            Ok(Vec::new())
        }
    }
    let path = std::env::temp_dir().join(format!("ck-characters-{}-archive", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut archive = Archive::open(&path).expect("archive");
    let mut s = CharacterStore::default();
    let dad = s
        .create(
            "dad".into(),
            Sex::Male,
            date(770, 1, 1),
            pop(),
            &|_: &PopLocator| Some(2),
        )
        .expect("dad");
    let child = s
        .create(
            "child".into(),
            Sex::Female,
            date(790, 1, 1),
            pop(),
            &|_: &PopLocator| Some(2),
        )
        .expect("child");
    s.set_parents(child, dad, UNKNOWN, date(800, 1, 1))
        .expect("parents");
    s.set_office(dad, Some(Office::Ruler), date(800, 1, 1))
        .expect("office");
    let before = s.clone();
    assert!(s.state_die(dad, date(800, 2, 29), &mut Fail).is_err());
    assert_eq!(s, before);
    s.state_die(dad, date(800, 2, 29), &mut archive)
        .expect("death");
    assert!(s.get(dad).is_none());
    assert_eq!(
        archive
            .get(s.get(child).expect("child").father)
            .expect("read")
            .expect("dead")
            .name,
        "dad"
    );
    assert_eq!(s.tier(child, date(805, 2, 28)), Some(Tier::Full));
    assert_eq!(s.tier(child, date(805, 3, 1)), Some(Tier::Coarse));
    drop(archive);
    let archive = Archive::open(&path).expect("rebuild index");
    assert!(archive.get(dad).expect("read").is_some());
    std::fs::remove_file(path).expect("cleanup");
}

#[test]
fn logs_are_read_on_demand_in_date_order_and_do_not_change_store() {
    let path = std::env::temp_dir().join(format!("ck-characters-{}-log", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut s = CharacterStore::default();
    let id = s
        .create(
            "A".into(),
            Sex::Male,
            date(770, 1, 1),
            pop(),
            &|_: &PopLocator| Some(1),
        )
        .expect("create");
    let before = s.clone();
    let mut logs = ck_characters::LifeLog::open(&path).expect("open");
    logs.append(id, date(800, 2, 1), "later").expect("append");
    logs.append(id, date(800, 1, 1), "earlier").expect("append");
    let lines = logs.read(id).expect("read");
    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        ["earlier", "later"]
    );
    assert_eq!(before, s);
    std::fs::remove_file(path).expect("cleanup");
}

#[test]
fn overlapping_qualifications_and_requalification_control_demotion() {
    let mut s = CharacterStore::default();
    let ruler = s
        .create(
            "r".into(),
            Sex::Male,
            date(770, 1, 1),
            pop(),
            &|_: &PopLocator| Some(3),
        )
        .expect("ruler");
    let heir = s
        .create(
            "h".into(),
            Sex::Female,
            date(790, 1, 1),
            pop(),
            &|_: &PopLocator| Some(3),
        )
        .expect("heir");
    s.set_parents(heir, ruler, UNKNOWN, date(800, 1, 1))
        .expect("parents");
    s.set_office(ruler, Some(Office::Ruler), date(800, 1, 1))
        .expect("ruler");
    s.set_office(heir, Some(Office::Heir), date(800, 1, 1))
        .expect("heir");
    s.set_office(ruler, Some(Office::HighCleric), date(800, 1, 2))
        .expect("replacement");
    assert_eq!(s.tier(heir, date(810, 1, 1)), Some(Tier::Full));
    s.set_office(heir, None, date(810, 1, 1)).expect("loss");
    assert_eq!(s.tier(heir, date(814, 12, 31)), Some(Tier::Full));
    s.set_office(heir, Some(Office::Heir), date(814, 1, 1))
        .expect("regain");
    s.set_office(heir, None, date(814, 1, 2))
        .expect("loss again");
    assert_eq!(s.tier(heir, date(819, 1, 1)), Some(Tier::Full));
    assert_eq!(s.tier(heir, date(819, 1, 2)), Some(Tier::Coarse));
}

#[test]
fn unknown_missing_pops_and_record_size() {
    assert!(size_of::<ck_characters::Character>() <= 512);
    let mut s = CharacterStore::default();
    assert!(s.get(UNKNOWN).is_none());
    assert!(
        s.create(
            "missing".into(),
            Sex::Male,
            date(770, 1, 1),
            pop(),
            &|_: &PopLocator| None
        )
        .is_err()
    );
    let id = s
        .create(
            "valid".into(),
            Sex::Male,
            date(770, 1, 1),
            pop(),
            &|_: &PopLocator| Some(1),
        )
        .expect("create");
    let before = s.clone();
    assert!(
        s.state_move_to(
            id,
            PopLocator {
                place: "bad".into(),
                slot: 99
            },
            &|_: &PopLocator| None
        )
        .is_err()
    );
    assert_eq!(s, before);
    assert_eq!(s.get(id).expect("live").father, UNKNOWN);
}

#[test]
fn matching_cohort_slots_have_independent_capacity() {
    let mut s = CharacterStore::default();
    let first = PopLocator {
        place: "connachta".into(),
        slot: 0,
    };
    let second = PopLocator {
        place: "connachta".into(),
        slot: 1,
    };
    for n in 0..5 {
        s.create(
            format!("{n}"),
            Sex::Female,
            date(770, 1, 1),
            second.clone(),
            &|_: &PopLocator| Some(5),
        )
        .expect("slot capacity");
    }
    assert!(
        s.create(
            "overflow".into(),
            Sex::Male,
            date(770, 1, 1),
            second.clone(),
            &|_: &PopLocator| Some(5)
        )
        .is_err()
    );
    assert_eq!(s.pop_count(&first), 0);
    assert_eq!(s.pop_count(&second), 5);
    let restored = CharacterStore::from_bytes(&s.to_bytes().expect("encode")).expect("decode");
    assert!(
        restored
            .validate_pops(|p| if *p == first {
                Some(15)
            } else if *p == second {
                Some(5)
            } else {
                None
            })
            .is_ok()
    );
    assert!(
        restored
            .validate_pops(|p| if *p == second { Some(4) } else { None })
            .is_err()
    );
    assert_eq!(ck_characters::UNKNOWN_PORTRAIT_KEY, "unknown");
}

#[test]
fn ruler_death_and_divorce_end_only_the_last_qualification() {
    struct Sink;
    impl ck_characters::ArchiveSink for Sink {
        fn append(&mut self, _: &ck_characters::Character) -> std::io::Result<()> {
            Ok(())
        }
        // A sink that holds no records: no parent has an archived child.
        fn children_of(&self, _: u64) -> std::io::Result<Vec<u64>> {
            Ok(Vec::new())
        }
    }
    let heads = |_: &PopLocator| Some(4);
    let mut s = CharacterStore::default();
    let ruler = s
        .create("r".into(), Sex::Male, date(770, 1, 1), pop(), &heads)
        .expect("ruler");
    let spouse = s
        .create("s".into(), Sex::Female, date(770, 1, 1), pop(), &heads)
        .expect("spouse");
    let child = s
        .create("c".into(), Sex::Male, date(790, 1, 1), pop(), &heads)
        .expect("child");
    s.state_marry(ruler, spouse, date(800, 1, 1))
        .expect("marry");
    s.set_parents(child, ruler, UNKNOWN, date(800, 1, 1))
        .expect("parent");
    s.set_office(ruler, Some(Office::Ruler), date(800, 1, 1))
        .expect("ruler");
    s.set_office(child, Some(Office::Heir), date(800, 1, 1))
        .expect("heir");
    s.divorce(ruler, spouse, date(800, 1, 2)).expect("divorce");
    assert_eq!(s.tier(spouse, date(805, 1, 1)), Some(Tier::Full));
    assert_eq!(s.tier(spouse, date(805, 1, 2)), Some(Tier::Coarse));
    s.state_die(ruler, date(800, 1, 3), &mut Sink)
        .expect("death");
    assert_eq!(s.tier(child, date(810, 1, 1)), Some(Tier::Full));
}

#[test]
fn random_draws_ignore_processing_order_and_later_creations() {
    let heads = |_: &PopLocator| Some(4);
    let make = || {
        let mut store = CharacterStore::default();
        let a = store
            .create("A".into(), Sex::Female, date(770, 1, 1), pop(), &heads)
            .expect("A");
        let b = store
            .create("B".into(), Sex::Male, date(770, 1, 1), pop(), &heads)
            .expect("B");
        (store, a, b)
    };
    let (mut first, a, b) = make();
    let (_, a2, b2) = make();
    assert_eq!((a, b), (a2, b2));
    let tick = date(800, 1, 1);
    let a_draw = character_stream(42, a, "purpose", tick).draw();
    let b_draw = character_stream(42, b, "purpose", tick).draw();
    let b_reordered = character_stream(42, b2, "purpose", tick).draw();
    let _unrelated = character_stream(99, 444, "elsewhere", tick).draw();
    let a_reordered = character_stream(42, a2, "purpose", tick).draw();
    assert_eq!((a_draw, b_draw), (a_reordered, b_reordered));
    first
        .create("later".into(), Sex::Female, date(790, 1, 1), pop(), &heads)
        .expect("later");
    assert_eq!(a_draw, character_stream(42, a, "purpose", tick).draw());
}

#[test]
fn changed_archive_length_is_refused_without_allocating_its_claimed_size() {
    use std::io::{Seek, SeekFrom, Write};
    let path = std::env::temp_dir().join(format!("ck-characters-corrupt-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut archive = ck_characters::Archive::open(&path).expect("archive");
    let mut store = CharacterStore::default();
    let id = store
        .create(
            "A".into(),
            Sex::Male,
            date(770, 1, 1),
            pop(),
            &|_: &PopLocator| Some(1),
        )
        .expect("create");
    store
        .state_die(id, date(800, 1, 1), &mut archive)
        .expect("death");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("file");
    file.seek(SeekFrom::Start(0)).expect("seek");
    file.write_all(&u32::MAX.to_le_bytes())
        .expect("corrupt length");
    assert!(archive.get(id).is_err());
    assert!(ck_characters::Archive::open(&path).is_err());
    std::fs::remove_file(path).expect("cleanup");
}

#[test]
fn ruler_replaced_by_high_cleric_starts_spouse_grace() {
    let heads = |_: &PopLocator| Some(2);
    let mut store = CharacterStore::default();
    let ruler = store
        .create("r".into(), Sex::Male, date(770, 1, 1), pop(), &heads)
        .expect("ruler");
    let spouse = store
        .create("s".into(), Sex::Female, date(770, 1, 1), pop(), &heads)
        .expect("spouse");
    store
        .state_marry(ruler, spouse, date(800, 1, 1))
        .expect("marry");
    store
        .set_office(ruler, Some(Office::Ruler), date(800, 1, 1))
        .expect("ruler");
    store
        .set_office(ruler, Some(Office::HighCleric), date(800, 1, 2))
        .expect("replace");
    assert_eq!(store.tier(ruler, date(805, 1, 2)), Some(Tier::Full));
    assert_eq!(store.tier(spouse, date(805, 1, 1)), Some(Tier::Full));
    assert_eq!(store.tier(spouse, date(805, 1, 2)), Some(Tier::Coarse));
}
