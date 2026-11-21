//! Shared fixtures of the generator tests (CK-369): the shipped rules, a population source with pops of chosen kind, culture and room, an
//! archive and a life log on disk, and a world over them.
#![expect(dead_code, reason = "each test file uses part of these shared helpers")]

use ck_characters::{
    Archive, ArchiveSink, Character, CharacterStore, Error, GeneticRecord, LifeLog, LifeRecord,
    PersonalityRecord, PopLocator, Populations, Sex, World,
};
use ck_economy::{EventRules, LifeCycle, PopGenetics, Rules, events::Registry};
use ck_sim::{Canonical, Date, GameTime};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

pub(crate) fn date(year: u16, month: u8, day: u8) -> GameTime {
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour: 0,
    })
    .expect("valid date")
}
pub(crate) fn locator(place: &str, slot: u32) -> PopLocator {
    PopLocator {
        place: place.into(),
        slot,
    }
}
pub(crate) fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        Rules::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
        )
        .expect("shipped rules")
    })
}
/// The age in whole years of a record on `day`.
pub(crate) fn age_on(c: &Character, day: GameTime) -> u32 {
    let (born, today) = (c.birth.date().expect("a date"), day.date().expect("a date"));
    let turned = (today.month, today.day) >= (born.month, born.day);
    u32::from(today.year - born.year) - u32::from(!turned)
}

/// One pop of the fixture.
#[derive(Clone, Debug)]
pub(crate) struct PopInfo {
    pub(crate) kind: &'static str,
    pub(crate) culture: &'static str,
    pub(crate) heads: u64,
}

/// The pops: each place/slot has a kind, a culture and a number of heads; places are a given distance apart (none known unless named).
pub(crate) struct Pops {
    by_slot: RefCell<BTreeMap<(String, u32), PopInfo>>,
    apart: BTreeMap<(String, String), u64>,
    /// The head moves made and not undone, in order.
    pub(crate) moves: RefCell<Vec<(PopLocator, PopLocator)>>,
    /// Whether the next head moves are refused.
    pub(crate) refuse_moves: Cell<bool>,
    /// How many more head moves are made before the rest are refused (`None`: no limit).
    pub(crate) moves_allowed: Cell<Option<u32>>,
    /// How many births were booked and how many heads were removed for deaths: generation books none.
    pub(crate) births: Cell<u64>,
    pub(crate) deaths: Cell<u64>,
    /// The start's safety net (CK-370): whether a full pop is raised by one head instead of refusing a character.
    pub(crate) net: Cell<bool>,
    /// The raises the net made, by pop.
    pub(crate) raised: RefCell<BTreeMap<PopLocator, u64>>,
}
impl Pops {
    pub(crate) fn new() -> Self {
        Self {
            by_slot: RefCell::new(BTreeMap::new()),
            apart: BTreeMap::new(),
            moves: RefCell::new(Vec::new()),
            refuse_moves: Cell::new(false),
            moves_allowed: Cell::new(None),
            births: Cell::new(0),
            deaths: Cell::new(0),
            net: Cell::new(false),
            raised: RefCell::new(BTreeMap::new()),
        }
    }
    pub(crate) fn pop(
        self,
        place: &str,
        slot: u32,
        kind: &'static str,
        culture: &'static str,
        heads: u64,
    ) -> Self {
        self.by_slot.borrow_mut().insert(
            (place.to_owned(), slot),
            PopInfo {
                kind,
                culture,
                heads,
            },
        );
        self
    }
    pub(crate) fn apart(mut self, a: &str, b: &str, metres: u64) -> Self {
        self.apart.insert((a.to_owned(), b.to_owned()), metres);
        self.apart.insert((b.to_owned(), a.to_owned()), metres);
        self
    }
    pub(crate) fn set_heads(&self, place: &str, slot: u32, heads: u64) {
        self.by_slot
            .borrow_mut()
            .get_mut(&(place.to_owned(), slot))
            .expect("pop")
            .heads = heads;
    }
    fn info(&self, locator: &PopLocator) -> Option<PopInfo> {
        self.by_slot
            .borrow()
            .get(&(locator.place.clone(), locator.slot))
            .cloned()
    }
}
fn genetics_of(culture: &str) -> PopGenetics {
    rules()
        .genetics
        .seed_pop(culture)
        .expect("a culture with a start set")
}
impl Populations for Pops {
    fn heads(&self, locator: &PopLocator) -> Option<u64> {
        self.info(locator).map(|pop| pop.heads)
    }
    fn culture(&self, locator: &PopLocator) -> Option<String> {
        self.info(locator).map(|pop| pop.culture.to_owned())
    }
    fn kind(&self, locator: &PopLocator) -> Option<String> {
        self.info(locator).map(|pop| pop.kind.to_owned())
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
    fn genetics(&self, locator: &PopLocator) -> Option<PopGenetics> {
        self.info(locator).map(|pop| genetics_of(pop.culture))
    }
    fn distance_m(&self, a: &PopLocator, b: &PopLocator) -> Option<u64> {
        self.info(a)?;
        self.info(b)?;
        if a.place == b.place {
            return Some(0);
        }
        self.apart.get(&(a.place.clone(), b.place.clone())).copied()
    }
    fn add_births(&self, _: &PopLocator, count: u64) -> Result<(), Error> {
        self.births.set(self.births.get() + count);
        Ok(())
    }
    fn all_pops(&self) -> Vec<(PopLocator, String, String)> {
        self.by_slot
            .borrow()
            .iter()
            .map(|((place, slot), pop)| {
                (
                    PopLocator {
                        place: place.clone(),
                        slot: *slot,
                    },
                    pop.kind.to_owned(),
                    pop.culture.to_owned(),
                )
            })
            .collect()
    }
    fn make_room(&self, locator: &PopLocator) -> Result<bool, Error> {
        if !self.net.get() {
            return Ok(false);
        }
        self.by_slot
            .borrow_mut()
            .get_mut(&(locator.place.clone(), locator.slot))
            .ok_or(Error::MissingPop)?
            .heads += 1;
        *self.raised.borrow_mut().entry(locator.clone()).or_default() += 1;
        Ok(true)
    }
    fn raised_at_start(&self) -> Vec<(PopLocator, u64)> {
        self.raised
            .borrow()
            .iter()
            .map(|(pop, count)| (pop.clone(), *count))
            .collect()
    }
    fn remove_head(&self, _: &PopLocator, _: bool) -> Result<ck_economy::HeadUndo, Error> {
        self.deaths.set(self.deaths.get() + 1);
        Ok(ck_economy::HeadUndo::default())
    }
    fn move_head(
        &self,
        from: &PopLocator,
        to: &PopLocator,
        _: bool,
    ) -> Result<ck_economy::MoveUndo, Error> {
        if self.refuse_moves.get() {
            return Err(Error::Lifecycle);
        }
        if let Some(allowed) = self.moves_allowed.get() {
            if allowed == 0 {
                return Err(Error::Lifecycle);
            }
            self.moves_allowed.set(Some(allowed - 1));
        }
        let mut pops = self.by_slot.borrow_mut();
        let key = |locator: &PopLocator| (locator.place.clone(), locator.slot);
        pops.get_mut(&key(from)).ok_or(Error::MissingPop)?.heads -= 1;
        pops.get_mut(&key(to)).ok_or(Error::MissingPop)?.heads += 1;
        self.moves.borrow_mut().push((from.clone(), to.clone()));
        Ok(ck_economy::MoveUndo::default())
    }
    fn restore_move(&self, _: &ck_economy::MoveUndo) {
        let Some((from, to)) = self.moves.borrow_mut().pop() else {
            return;
        };
        let mut pops = self.by_slot.borrow_mut();
        let key = |locator: &PopLocator| (locator.place.clone(), locator.slot);
        pops.get_mut(&key(&from)).expect("pop").heads += 1;
        pops.get_mut(&key(&to)).expect("pop").heads -= 1;
    }
}

pub(crate) fn path(tag: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "ck-generator-{}-{tag}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::remove_file(&path).ok();
    path
}
/// An archive and a life log of one test.
pub(crate) struct Env {
    pub(crate) archive: Archive,
    pub(crate) logs: LifeLog,
    archive_path: PathBuf,
}
impl Env {
    pub(crate) fn new(tag: &str) -> Self {
        let archive_path = path(&format!("{tag}-arch"));
        Self {
            archive: Archive::open(&archive_path).expect("archive"),
            logs: LifeLog::open(path(&format!("{tag}-logs"))).expect("logs"),
            archive_path,
        }
    }
    pub(crate) fn archive_path(&self) -> &std::path::Path {
        &self.archive_path
    }
    pub(crate) fn lines(&self, id: u64) -> Vec<String> {
        self.logs
            .read(id)
            .expect("log")
            .into_iter()
            .map(|line| line.text)
            .collect()
    }
    pub(crate) fn archived_ids(&self) -> Vec<u64> {
        self.archive.ids().collect()
    }
}
pub(crate) fn world<'a, P: Populations>(
    lifecycle: &'a LifeCycle,
    env: &'a mut Env,
    pops: &'a P,
    seed: u64,
) -> World<'a, P, Archive, LifeLog> {
    world_with(&rules().events, lifecycle, env, pops, seed)
}
/// A world over the event data `events`.
pub(crate) fn world_with<'a, P: Populations>(
    events: &'a EventRules,
    lifecycle: &'a LifeCycle,
    env: &'a mut Env,
    pops: &'a P,
    seed: u64,
) -> World<'a, P, Archive, LifeLog> {
    World {
        rules: events,
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle,
        seed,
        populations: pops,
        archive: &mut env.archive,
        logs: &mut env.logs,
    }
}
/// The shipped life-cycle data.
pub(crate) fn shipped() -> &'static LifeCycle {
    &rules().lifecycle
}
/// The shipped life-cycle data with the generated relatives' death curve **off** (CK-370 §5): every generated spouse and child survives to
/// the day it is made, as in the first draft of the generator.
pub(crate) fn without_relative_deaths() -> &'static LifeCycle {
    static OFF: OnceLock<LifeCycle> = OnceLock::new();
    OFF.get_or_init(|| {
        let mut data = rules().lifecycle.clone();
        data.generator.relatives.survive_by_death_curve = false;
        data
    })
}
/// The characters of a store, by id.
pub(crate) fn people(store: &CharacterStore) -> Vec<&Character> {
    store.iter().collect()
}
/// An archive that refuses once `ok` records have been taken; with `batch` it publishes several records as one step (all or none), without
/// it it refuses a batch of more than one before writing.
pub(crate) struct FailAfter {
    pub(crate) inner: Archive,
    pub(crate) ok: Cell<u32>,
    pub(crate) batch: bool,
}
impl ArchiveSink for FailAfter {
    fn append(&mut self, record: &Character) -> std::io::Result<()> {
        if self.ok.get() == 0 {
            return Err(std::io::Error::other("the archive is full"));
        }
        self.ok.set(self.ok.get() - 1);
        self.inner.append(record)
    }
    fn append_all(&mut self, records: &[Character]) -> std::io::Result<()> {
        match records {
            [] => Ok(()),
            [one] => self.append(one),
            _ if !self.batch => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "no batches",
            )),
            _ => {
                let need = u32::try_from(records.len()).expect("a small batch");
                if self.ok.get() < need {
                    return Err(std::io::Error::other("the archive is full"));
                }
                self.ok.set(self.ok.get() - need);
                self.inner.append_all(records)
            }
        }
    }
    fn get(&self, id: u64) -> std::io::Result<Option<Character>> {
        self.inner.get(id)
    }
    fn children_of(&self, parent: u64) -> std::io::Result<Vec<u64>> {
        ArchiveSink::children_of(&self.inner, parent)
    }
    fn house_in_use(&self, house: &str) -> std::io::Result<bool> {
        self.inner.house_in_use(house)
    }
}

/// The shipped attribute data with the events of `text` (leaked for the life of the test).
pub(crate) fn events_from(text: &str) -> &'static EventRules {
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

/// A record with ordinary stats and a neutral personality in `pop`, born on `birth`, for a case that needs a character it made itself.
pub(crate) fn person(
    id: u64,
    sex: Sex,
    birth: GameTime,
    pop: PopLocator,
    house: &str,
) -> Character {
    let mut genetics = GeneticRecord::default();
    genetics.value[9..13].copy_from_slice(&[5000; 4]);
    for stat in 0..9 {
        genetics.value[stat] = 10_000;
    }
    genetics.value[1] = 70_000;
    let mut life = LifeRecord::default();
    "irish".clone_into(&mut life.pool);
    "balanced".clone_into(&mut life.focus);
    house.clone_into(&mut life.house);
    house.clone_into(&mut life.house_of_birth);
    Character {
        id,
        name: format!("c{id}"),
        sex,
        birth,
        death: None,
        pop,
        father: ck_characters::UNKNOWN,
        mother: ck_characters::UNKNOWN,
        spouses: Vec::new(),
        children: Vec::new(),
        office: None,
        full_until: None,
        genetics: Box::new(genetics),
        personality: PersonalityRecord::default(),
        life: Box::new(life),
        ties: Box::default(),
    }
}
/// A store holding `records`, built through the canonical encoding (the next identifier above the highest).
pub(crate) fn store_of(records: Vec<Character>) -> CharacterStore {
    let mut bytes = Vec::new();
    let next = records.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    next.encode(&mut bytes).expect("encode");
    let live: BTreeMap<u64, Character> = records.into_iter().map(|c| (c.id, c)).collect();
    live.encode(&mut bytes).expect("encode");
    0_u64.encode(&mut bytes).expect("encode");
    BTreeMap::<String, String>::new()
        .encode(&mut bytes)
        .expect("encode");
    CharacterStore::from_bytes(&bytes).expect("a store")
}

/// The same day of the month `months` earlier, held to the length of that month.
pub(crate) fn months_before(time: GameTime, months: u32) -> GameTime {
    let d = time.date().expect("a date");
    let index = u32::from(d.year) * 12 + u32::from(d.month) - 1 - months;
    let (year, month) = (
        u16::try_from(index / 12).expect("a year"),
        u8::try_from(index % 12 + 1).expect("a month"),
    );
    let longest = ck_sim::days_in_month(year, month).expect("a month");
    date(year, month, d.day.min(longest))
}
/// The 1st of the month `months` after the month of `time`.
pub(crate) fn first_after(time: GameTime, months: u32) -> GameTime {
    let d = time.date().expect("a date");
    let index = u32::from(d.year) * 12 + u32::from(d.month) - 1 + months;
    date(
        u16::try_from(index / 12).expect("a year"),
        u8::try_from(index % 12 + 1).expect("a month"),
        1,
    )
}
/// The record of `id`, live or archived.
pub(crate) fn record(store: &CharacterStore, env: &Env, id: u64) -> Character {
    store
        .lookup(id, &env.archive)
        .expect("readable")
        .unwrap_or_else(|| panic!("no record of {id}"))
}
/// The record of `id`, live or archived, read from `archive`.
pub(crate) fn record_in(store: &CharacterStore, archive: &Archive, id: u64) -> Character {
    store
        .lookup(id, archive)
        .expect("readable")
        .unwrap_or_else(|| panic!("no record of {id}"))
}
/// What every state of slices 1-6 must satisfy: identifiers and pops valid, every parent a record of the right sex that names its live child,
/// spouses reciprocal, every child named by a recorded parent, and the canonical encoding a round trip.
pub(crate) fn assert_whole(store: &CharacterStore, env: &Env, pops: &Pops) {
    assert_whole_in(store, &env.archive, pops);
}
/// [`assert_whole`] over an archive of its own.
pub(crate) fn assert_whole_in(store: &CharacterStore, archive: &Archive, pops: &Pops) {
    store.validate_archive(archive).expect("identifiers");
    store
        .validate_pops(|locator| pops.heads(locator))
        .expect("pops");
    for c in store.iter() {
        for (parent, sex) in [(c.father, Sex::Male), (c.mother, Sex::Female)] {
            if parent == ck_characters::UNKNOWN {
                continue;
            }
            let p = record_in(store, archive, parent);
            assert_eq!(p.sex, sex, "{} parent of {}", parent, c.id);
            if let Some(live) = store.get(parent) {
                assert!(live.children.contains(&c.id), "{parent} lists {}", c.id);
            }
        }
        for spouse in &c.spouses {
            if let Some(other) = store.get(*spouse) {
                assert!(
                    other.spouses.contains(&c.id),
                    "{spouse} and {} are wed",
                    c.id
                );
            }
        }
        for child in &c.children {
            let child = record_in(store, archive, *child);
            assert!(
                child.father == c.id || child.mother == c.id,
                "{} names {}",
                child.id,
                c.id
            );
        }
    }
    let bytes = store.to_bytes().expect("encode");
    assert_eq!(&CharacterStore::from_bytes(&bytes).expect("decode"), store);
}
