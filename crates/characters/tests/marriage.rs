//! Marriage, the automatic part (CK-366, characters slice 5c): houses, the wish, the pool, scoring, acceptance and the marriage as one
//! transition.
//!
//! Characters whose exact state a case needs are built as records and loaded through the canonical encoding. The events are none (a
//! quiet world) except where the shipped `marriage` happening is the subject. A roll is checked against the stream it names, so an
//! outcome is exact, not statistical.

use ck_characters::{
    Archive, ArchiveSink, Character, CharacterStore, Context, EventError, GeneticRecord, Ledger,
    LifeLog, LifeRecord, Lineage, LogSink, NewCharacter, Office, PersonalityRecord, PopLocator,
    Populations, Sex, UNKNOWN, World, character_stream, genetics::succeeds, personality_opinion,
};
use ck_economy::{
    CounterBuckets, EconomyState, EventRules, LifeCycle, MoveUndo, NearbyPlace, PlaceState,
    PopConsumption, PopGenetics, PopState, Rules, events::Registry,
};
use ck_sim::{Canonical, Date, GameTime};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io,
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

const PPB: u32 = 1_000_000_000;
/// A point of score, in nanopoints.
const POINT: i128 = 1_000_000_000;

fn date(year: u16, month: u8, day: u8) -> GameTime {
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour: 0,
    })
    .expect("valid date")
}
/// The day the marriage wishes of these tests are made: the seekers' birthdays.
fn today() -> GameTime {
    date(768, 3, 3)
}
fn locator(place: &str) -> PopLocator {
    PopLocator {
        place: place.into(),
        slot: 0,
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
/// No events at all: nothing but the life cycle writes a line or changes a character.
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
fn irish() -> PopGenetics {
    rules().genetics.seed_pop("ulaid").expect("Irish start set")
}
/// The shipped life-cycle data with every wish certain (a single point at 1), so that a birthday decides nothing by chance until the
/// candidate's side answers, and **no generated candidates**: these cases are about the pool of 5c as the characters make it (the top-up
/// of a short pool, CK-369 §6, is tested with the generator).
fn eager() -> LifeCycle {
    let mut data = rules().lifecycle.clone();
    data.marriage.wish.men = vec![(12, u64::from(PPB))];
    data.marriage.wish.women = vec![(12, u64::from(PPB))];
    data.marriage.pool.min_candidates = 0;
    data
}

/// The pops: every place is a Christian noble Irish pop (`ulaid`) with room for a million unless a case says otherwise. Head changes
/// are real (a move takes one head from one place and gives it to another) and recorded; a move can be refused.
struct Pops {
    kinds: BTreeMap<&'static str, &'static str>,
    religions: BTreeMap<&'static str, &'static str>,
    distances: BTreeMap<(&'static str, &'static str), u64>,
    heads: RefCell<BTreeMap<String, u64>>,
    moves: RefCell<Vec<(String, String, bool)>>,
    restored: RefCell<u32>,
    refuse_move: bool,
}
impl Pops {
    fn new() -> Self {
        Self {
            kinds: BTreeMap::new(),
            religions: BTreeMap::new(),
            distances: BTreeMap::new(),
            heads: RefCell::new(BTreeMap::new()),
            moves: RefCell::new(Vec::new()),
            restored: RefCell::new(0),
            refuse_move: false,
        }
    }
    fn kind(mut self, place: &'static str, kind: &'static str) -> Self {
        self.kinds.insert(place, kind);
        self
    }
    fn religion(mut self, place: &'static str, religion: &'static str) -> Self {
        self.religions.insert(place, religion);
        self
    }
    fn apart(mut self, a: &'static str, b: &'static str, metres: u64) -> Self {
        self.distances.insert((a, b), metres);
        self
    }
    fn with_heads(self, place: &str, heads: u64) -> Self {
        self.heads.borrow_mut().insert(place.to_owned(), heads);
        self
    }
    fn heads_of(&self, place: &str) -> u64 {
        self.heads(&locator(place)).expect("heads")
    }
    fn moves(&self) -> Vec<(String, String, bool)> {
        self.moves.borrow().clone()
    }
}
impl Populations for Pops {
    fn heads(&self, locator: &PopLocator) -> Option<u64> {
        Some(
            self.heads
                .borrow()
                .get(&locator.place)
                .copied()
                .unwrap_or(1_000_000),
        )
    }
    fn culture(&self, locator: &PopLocator) -> Option<String> {
        Some(
            match locator.place.as_str() {
                "e" => "northumbrian",
                _ => "ulaid",
            }
            .to_owned(),
        )
    }
    fn kind(&self, locator: &PopLocator) -> Option<String> {
        Some(
            self.kinds
                .get(locator.place.as_str())
                .copied()
                .unwrap_or("noble")
                .to_owned(),
        )
    }
    fn luxuries_pm(&self, _: &PopLocator) -> Option<u16> {
        Some(600)
    }
    fn religion(&self, locator: &PopLocator) -> Option<String> {
        Some(
            self.religions
                .get(locator.place.as_str())
                .copied()
                .unwrap_or("christianity")
                .to_owned(),
        )
    }
    fn necessities_pm(&self, _: &PopLocator) -> Option<u16> {
        Some(1000)
    }
    fn genetics(&self, _: &PopLocator) -> Option<PopGenetics> {
        Some(irish())
    }
    fn distance_m(&self, a: &PopLocator, b: &PopLocator) -> Option<u64> {
        if a.place == b.place {
            return Some(0);
        }
        self.distances
            .iter()
            .find(|((x, y), _)| {
                (*x == a.place && *y == b.place) || (*x == b.place && *y == a.place)
            })
            .map(|(_, metres)| *metres)
    }
    fn move_head(
        &self,
        from: &PopLocator,
        to: &PopLocator,
        minor: bool,
    ) -> Result<MoveUndo, ck_characters::Error> {
        if self.refuse_move || self.heads_of(&from.place) == 0 {
            return Err(ck_characters::Error::MissingPop);
        }
        let mut heads = self.heads.borrow_mut();
        *heads.entry(from.place.clone()).or_insert(1_000_000) -= 1;
        *heads.entry(to.place.clone()).or_insert(1_000_000) += 1;
        self.moves
            .borrow_mut()
            .push((from.place.clone(), to.place.clone(), minor));
        Ok(MoveUndo::default())
    }
    fn restore_move(&self, _: &MoveUndo) {
        let (from, to, _) = self.moves.borrow_mut().pop().expect("a move to put back");
        let mut heads = self.heads.borrow_mut();
        *heads.entry(from).or_insert(1_000_000) += 1;
        *heads.entry(to).or_insert(1_000_000) -= 1;
        *self.restored.borrow_mut() += 1;
    }
}

fn path(tag: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "ck-marriage-{}-{tag}-{}",
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
    /// Archive `dead` as having died in 760.
    fn bury(&mut self, mut dead: Character) {
        dead.death = Some(date(760, 6, 6));
        self.archive.append(&dead).expect("append");
    }
}
fn world<'a, P: Populations>(
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

/// A record with ordinary stats, a Christian pop's name pool and a neutral personality, in pop `place`.
fn person(id: u64, birth: GameTime, place: &str, sex: Sex) -> Character {
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
        birth,
        death: None,
        pop: locator(place),
        father: UNKNOWN,
        mother: UNKNOWN,
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
/// A man or a woman who is `age` years old on [`today`], with a birthday on the 1st of January.
fn man(id: u64, age: u16, place: &str) -> Character {
    person(id, date(768 - age, 1, 1), place, Sex::Male)
}
fn woman(id: u64, age: u16, place: &str) -> Character {
    person(id, date(768 - age, 1, 1), place, Sex::Female)
}
/// A seeker, whose birthday is [`today`].
fn seeker(id: u64, age: u16, place: &str, sex: Sex) -> Character {
    person(id, date(768 - age, 3, 3), place, sex)
}
fn housed(mut c: Character, house: &str) -> Character {
    house.clone_into(&mut c.life.house);
    house.clone_into(&mut c.life.house_of_birth);
    c
}
fn ruler(mut c: Character) -> Character {
    c.office = Some(Office::Ruler);
    c
}
fn with_parents(mut c: Character, father: u64, mother: u64) -> Character {
    c.father = father;
    c.mother = mother;
    c
}
fn store_of(characters: Vec<Character>) -> CharacterStore {
    let next = characters.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    let live: BTreeMap<u64, Character> = characters.into_iter().map(|c| (c.id, c)).collect();
    let mut bytes = Vec::new();
    next.encode(&mut bytes).expect("encode");
    live.encode(&mut bytes).expect("encode");
    0_u64.encode(&mut bytes).expect("encode");
    CharacterStore::from_bytes(&bytes).expect("a store of these records")
}
fn get(store: &CharacterStore, id: u64) -> &Character {
    store.get(id).expect("live")
}
fn bytes(store: &CharacterStore) -> Vec<u8> {
    store.to_bytes().expect("encode")
}
/// The roll a stream gives for chance `p`, as the engine takes it.
fn rolls(seed: u64, id: u64, purpose: &str, p: u32) -> bool {
    let draw = character_stream(seed, id, purpose, today())
        .below(u64::from(PPB))
        .expect("a draw");
    succeeds(draw, p)
}

// --- Houses -----------------------------------------------------------------------------------------------------------------

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
fn newborn(place: &str, house: &str) -> NewCharacter {
    NewCharacter {
        name: String::new(),
        sex: Sex::Female,
        birth: date(768, 1, 1),
        pop: locator(place),
        house: house.to_owned(),
    }
}

#[test]
fn a_founder_takes_the_house_its_caller_gives_as_its_house_and_its_house_of_birth() {
    let lifecycle = &rules().lifecycle;
    let pops = Pops::new();
    let mut store = CharacterStore::default();
    let noble = store
        .create_founder(
            &context(lifecycle),
            newborn("a", "ui_neill"),
            &irish(),
            &pops,
        )
        .expect("founder");
    let commoner = store
        .create_founder(&context(lifecycle), newborn("a", ""), &irish(), &pops)
        .expect("founder");
    for (id, house) in [(noble, "ui_neill"), (commoner, "")] {
        assert_eq!(get(&store, id).life.house, house);
        assert_eq!(get(&store, id).life.house_of_birth, house);
    }
}

#[test]
fn a_child_is_born_into_its_fathers_house_else_its_mothers_and_keeps_it_for_life() {
    let lifecycle = &rules().lifecycle;
    let pops = Pops::new();
    let env = Env::new("birth-house");
    let mut store = CharacterStore::default();
    let mut founder = |sex: Sex, house: &str| {
        let mut new = newborn("a", house);
        new.sex = sex;
        store
            .create_founder(&context(lifecycle), new, &irish(), &pops)
            .expect("founder")
    };
    let (father, mother) = (
        founder(Sex::Male, "ui_neill"),
        founder(Sex::Female, "dal_riata"),
    );
    let (poor_father, poor_mother) = (founder(Sex::Male, ""), founder(Sex::Female, ""));
    let mut born = |father: u64, mother: u64| {
        let id = store
            .state_create_child(
                &context(lifecycle),
                newborn("a", "ignored"),
                &irish(),
                &Lineage {
                    father,
                    mother,
                    archive: &env.archive,
                },
                &pops,
            )
            .expect("child");
        let c = get(&store, id).clone();
        (c.life.house, c.life.house_of_birth)
    };
    let own = |house: &str| (house.to_owned(), house.to_owned());
    // The father's house, whatever the mother's; the mother's when the father has none; none when neither does. The house a caller
    // gives a child is not read.
    assert_eq!(born(father, mother), own("ui_neill"));
    assert_eq!(born(poor_father, mother), own("dal_riata"));
    assert_eq!(born(poor_father, poor_mother), own(""));
    assert_eq!(born(father, poor_mother), own("ui_neill"));
}

#[test]
fn a_wife_keeps_her_house_of_birth_for_life_and_the_houses_survive_a_save_and_the_archive() {
    let lifecycle = &eager();
    let pops = Pops::new().with_heads("a", 10);
    let mut env = Env::new("kept");
    let mut store = store_of(vec![
        housed(man(1, 25, "a"), "ui_neill"),
        housed(woman(2, 20, "a"), "dal_riata"),
    ]);
    store
        .arrange_marriage(
            &mut world(quiet(), lifecycle, &mut env, &pops, 7),
            1,
            2,
            today(),
        )
        .expect("marriage");
    let wife = get(&store, 2).clone();
    assert_eq!(wife.life.house, "ui_neill");
    assert_eq!(wife.life.house_of_birth, "dal_riata");
    // Saved and loaded: the same bytes, the same houses.
    let loaded = CharacterStore::from_bytes(&bytes(&store)).expect("load");
    assert_eq!(bytes(&loaded), bytes(&store));
    assert_eq!(get(&loaded, 2).life.house_of_birth, "dal_riata");
    // Dead and archived: read back from the archive with both.
    store
        .state_die(2, date(769, 1, 1), &mut env.archive)
        .expect("die");
    let archived = env.archive.get(2).expect("read").expect("archived");
    assert_eq!(archived.life.house, "ui_neill");
    assert_eq!(archived.life.house_of_birth, "dal_riata");
}

#[test]
fn a_ruling_house_is_a_house_with_a_living_member_who_is_a_ruler_and_its_head_is_the_lowest_id() {
    let mut store = store_of(vec![
        housed(man(4, 30, "a"), "ui_neill"),
        ruler(housed(man(9, 50, "a"), "ui_neill")),
        ruler(housed(man(5, 40, "a"), "ui_neill")),
        ruler(man(6, 40, "a")),
        housed(man(7, 40, "a"), "dal_riata"),
    ]);
    assert!(store.ruling_house("ui_neill"));
    assert_eq!(store.house_head("ui_neill"), Some(5));
    // A house whose only ruler is a member of no house, a house with no ruler and the houseless are not ruling.
    assert!(!store.ruling_house("dal_riata"));
    assert_eq!(store.house_head("dal_riata"), None);
    assert!(!store.ruling_house(""));
    // When the rulers die the house is no longer ruling.
    let mut env = Env::new("ruling");
    for id in [5, 9] {
        store
            .state_die(id, date(769, 1, 1), &mut env.archive)
            .expect("die");
    }
    assert!(!store.ruling_house("ui_neill"));
}

// --- The wish ---------------------------------------------------------------------------------------------------------------

fn wish(c: &Character, pops: &Pops) -> u32 {
    let mut env = Env::new("wish");
    CharacterStore::wish_ppb(
        &world(quiet(), &rules().lifecycle, &mut env, pops, 7),
        c,
        today(),
    )
    .expect("a chance")
}

#[test]
fn the_wish_follows_the_age_curve_of_the_sex_linearly_between_points() {
    let pops = Pops::new();
    // Men: (14, 2%), (17, 15%), (20, 35%), (30, 35%), (45, 10%); women: (12, 5%), (15, 30%), (20, 40%).
    assert_eq!(wish(&man(1, 20, "a"), &pops), 350_000_000);
    assert_eq!(wish(&man(1, 15, "a"), &pops), 63_333_333);
    assert_eq!(wish(&woman(1, 16, "a"), &pops), 320_000_000);
    assert_eq!(wish(&woman(1, 12, "a"), &pops), 50_000_000);
    // The end values beyond the points: 0 from 70 for a man, 0 from 60 for a woman.
    assert_eq!(wish(&man(1, 90, "a"), &pops), 0);
    assert_eq!(wish(&woman(1, 60, "a"), &pops), 0);
}

#[test]
fn the_personality_scales_the_wish_and_is_floored_at_a_tenth() {
    let pops = Pops::new();
    // Lust 2 points (the sign of the data's per-point value is the chastity's): 1 + 0.05 x 2 = 1.1 for lust, here as -2 chastity;
    // gregarious 5 points: 1 + 0.02 x 5 = 1.1. Together 1.21 x 35% = 42.35%.
    let mut c = man(1, 20, "a");
    c.personality.vice[2] = -2000;
    c.personality.manner[6] = 5000;
    assert_eq!(wish(&c, &pops), 423_500_000);
    // Chastity 7 and solitary 45: 0.65 x 0.1 = 0.065, floored at 0.1: 3.5% of the 35%.
    let mut floored = man(1, 20, "a");
    floored.personality.vice[2] = 7000;
    floored.personality.manner[6] = -45_000;
    assert_eq!(wish(&floored, &pops), 35_000_000);
    // A factor below zero is floored too, not negative.
    floored.personality.manner[6] = -100_000;
    assert_eq!(wish(&floored, &pops), 35_000_000);
}

#[test]
fn the_wish_floors_the_signed_product_of_the_two_terms_not_each_term() {
    // With 0.5 of chastity per point (data edited so a term can pass below zero): chastity +7 gives -2.5 and solitary -100 gives -1;
    // the product is +2.5, so the wish is 0.35 x 2.5 = 87.5%, not the floor.
    let mut data = rules().lifecycle.clone();
    data.marriage.wish.lust_per_point = -500_000;
    let pops = Pops::new();
    let wish = |lust: i64, solitary: i64| {
        let mut c = man(1, 20, "a");
        c.personality.vice[2] = lust;
        c.personality.manner[6] = solitary;
        let mut env = Env::new("signed");
        CharacterStore::wish_ppb(&world(quiet(), &data, &mut env, &pops, 7), &c, today())
            .expect("a chance")
    };
    assert_eq!(wish(7000, -100_000), 875_000_000);
    // A positive product under a tenth is floored: -0.1 x -0.5 = 0.05, so 0.1 x 0.35.
    assert_eq!(wish(2200, -75_000), 35_000_000);
    // One negative term, or a zero term, makes a product that is not positive: floored.
    assert_eq!(wish(7000, 5000), 35_000_000);
    assert_eq!(wish(2000, 50_000), 35_000_000);
    // 2 points of chastity make the term exactly 0: a zero product.
    assert_eq!(wish(2000, -100_000), 35_000_000);
}

#[test]
fn the_wish_of_a_monk_pop_is_a_third_of_the_base_and_a_missing_pop_type_is_one() {
    let pops = Pops::new().kind("m", "monk").kind("l", "labourer");
    assert_eq!(wish(&man(1, 20, "m"), &pops), 105_000_000);
    assert_eq!(wish(&man(1, 20, "l"), &pops), 350_000_000);
    assert_eq!(wish(&man(1, 20, "a"), &pops), 350_000_000);
}

#[test]
fn a_chance_over_one_is_capped_at_one() {
    // A man of 20 with lust/chastity -7 and solitary/gregarious +100: 0.35 x 1.35 x 3 = 567/400, held to 1.
    let mut c = man(1, 20, "a");
    c.personality.vice[2] = -7000;
    c.personality.manner[6] = 100_000;
    let pops = Pops::new();
    let mut env = Env::new("cap");
    let w = world(quiet(), &rules().lifecycle, &mut env, &pops, 7);
    let ratios = CharacterStore::wish_ratios(&w, &c, today()).expect("ratios");
    let numerator: u128 = ratios.iter().map(|(n, _)| n).product();
    let denominator: u128 = ratios.iter().map(|(_, d)| d).product();
    assert_eq!(numerator * 400, 567 * denominator);
    assert_eq!(
        CharacterStore::wish_ppb(&w, &c, today()).expect("a chance"),
        PPB
    );
}

#[test]
fn a_widow_or_widower_qualifies_but_a_married_character_and_a_minor_do_not_seek() {
    let pops = Pops::new();
    let mut env = Env::new("qualify");
    // Character 1 is a widow (her husband 9 is not live); 2 is married to a live man; 3 is a bride of 11, below the age of 12.
    let mut widow = woman(1, 30, "a");
    widow.spouses = vec![9];
    let mut wife = woman(2, 30, "a");
    wife.spouses = vec![4];
    let mut husband = man(4, 30, "a");
    husband.spouses = vec![2];
    let store = store_of(vec![widow, wife, husband, woman(3, 11, "a")]);
    assert!(store.unmarried(get(&store, 1)));
    assert!(!store.unmarried(get(&store, 2)));
    assert!(!store.unmarried(get(&store, 4)));
    let w = world(quiet(), &rules().lifecycle, &mut env, &pops, 7);
    assert!(CharacterStore::marriageable(&w, get(&store, 1), today()).expect("age"));
    assert!(!CharacterStore::marriageable(&w, get(&store, 3), today()).expect("age"));
    // Of marriageable age by the pop's religion: a pop of a religion the data gives no age has no wish.
    let pagan = Pops::new().religion("a", "druidry");
    let w = world(quiet(), &rules().lifecycle, &mut env, &pagan, 7);
    assert!(!CharacterStore::marriageable(&w, get(&store, 1), today()).expect("age"));
}

#[test]
fn only_an_unmarried_character_of_marriageable_age_seeks_a_marriage_on_its_birthday() {
    let lifecycle = &eager();
    let pops = Pops::new();
    // Every wish is certain, and there are candidates. Seeker 1 is married, 7 is a girl of 11 and the others have another birthday:
    // nobody seeks, so nobody marries, whatever the seed. On the same day a man of 30 does.
    let mut wife = seeker(1, 30, "a", Sex::Female);
    wife.spouses = vec![4];
    let mut husband = man(4, 30, "a");
    husband.spouses = vec![1];
    let start = vec![
        wife,
        husband,
        man(5, 30, "a"),
        woman(6, 25, "a"),
        seeker(7, 11, "a", Sex::Female),
    ];
    for seed in [3, 8, 21] {
        let mut store = store_of(start.clone());
        let mut env = Env::new("quiet-day");
        store
            .run_birthdays(
                &mut world(quiet(), lifecycle, &mut env, &pops, seed),
                today(),
            )
            .expect("birthdays");
        for c in start.iter().map(|c| c.id) {
            assert_eq!(
                get(&store, c).spouses,
                start.iter().find(|r| r.id == c).unwrap().spouses
            );
        }
        assert!(pops.moves().is_empty());
    }
    // The same people with an unmarried man of 30 whose birthday it is: he seeks (the wish is certain) and the answer is the roll's.
    let mut seen = false;
    for seed in 0..40 {
        let mut all = start.clone();
        all.push(seeker(9, 30, "a", Sex::Male));
        let mut store = store_of(all);
        let mut env = Env::new("seeks");
        store
            .run_birthdays(
                &mut world(quiet(), lifecycle, &mut env, &pops, seed),
                today(),
            )
            .expect("birthdays");
        seen |= !get(&store, 9).spouses.is_empty();
    }
    assert!(seen, "an unmarried man of 30 never married in 40 seeds");
}

// --- The pool ---------------------------------------------------------------------------------------------------------------

/// Places: `a` (the seeker's), `a2` (also `a`'s own place: another slot), `n` 30 km away, `f` 120 km, `z` 200 km, and `u` of unknown
/// distance.
fn map() -> Pops {
    Pops::new()
        .apart("a", "n", 30_000)
        .apart("a", "n2", 30_000)
        .apart("a", "f", 120_000)
        .apart("a", "z", 200_000)
}

fn pool(store: &CharacterStore, pops: &Pops, seeker: u64) -> Vec<u64> {
    let mut env = Env::new("pool");
    store
        .marriage_pool(
            &world(quiet(), &rules().lifecycle, &mut env, pops, 7),
            seeker,
            today(),
        )
        .expect("pool")
}

#[test]
fn the_pool_is_nearest_first_by_the_distance_between_places_and_then_by_id() {
    let store = store_of(vec![
        seeker(1, 25, "a", Sex::Male),
        woman(20, 20, "f"),
        woman(21, 20, "n"),
        woman(22, 20, "a"),
        woman(23, 20, "n2"),
        woman(24, 20, "a"),
        woman(25, 20, "n"),
        woman(26, 20, "z"),
        woman(27, 20, "u"),
    ]);
    // The same place (ids 22, 24), then 30 km (21, 23, 25: ties by id), then 120 km (20); 200 km and an unknown distance are out of
    // reach.
    assert_eq!(pool(&store, &map(), 1), [22, 24, 21, 23, 25, 20]);
}

#[test]
fn the_same_place_comes_first_even_when_another_place_is_no_distance_away() {
    // Place `co` is at 0 m from `a` (a second site at the same anchor): the women of `a` still come first, whatever their ids.
    let pops = Pops::new().apart("a", "co", 0).apart("a", "n", 30_000);
    let store = store_of(vec![
        seeker(1, 25, "a", Sex::Male),
        woman(20, 20, "co"),
        woman(21, 20, "n"),
        woman(22, 20, "a"),
        woman(23, 20, "co"),
    ]);
    assert_eq!(pool(&store, &pops, 1), [22, 20, 23, 21]);
}

#[test]
fn the_pool_never_holds_kin_the_married_the_underage_the_same_sex_or_another_religion() {
    let father = man(10, 55, "a");
    let mother = woman(11, 50, "a");
    let mut seeker_record = with_parents(seeker(1, 25, "a", Sex::Male), 10, 11);
    seeker_record.children = vec![30];
    let mut married = woman(31, 22, "a");
    married.spouses = vec![8];
    let mut her_husband = man(8, 30, "a");
    her_husband.spouses = vec![31];
    let mut widow = woman(32, 22, "a");
    widow.spouses = vec![9];
    let store = store_of(vec![
        seeker_record,
        father,
        mother,
        her_husband,
        // A full sister, a half-sister by the father and one by the mother: every sharing of a known parent is a sibling.
        with_parents(woman(40, 20, "a"), 10, 11),
        with_parents(woman(41, 20, "a"), 10, 99),
        with_parents(woman(42, 20, "a"), 98, 11),
        // His mother and his daughter (children by recorded parentage and by list).
        with_parents(woman(30, 14, "a"), 1, 97),
        // The married, a widow, a girl of 11 (under 12), a girl of 12, a man, a woman of another religion.
        married,
        widow,
        woman(33, 11, "a"),
        woman(34, 12, "a"),
        man(35, 25, "a"),
        woman(36, 22, "p"),
        // An ordinary candidate.
        woman(37, 22, "a"),
    ]);
    let pops = Pops::new().religion("p", "druidry");
    assert_eq!(pool(&store, &pops, 1), [32, 34, 37]);
}

#[test]
fn an_empty_pool_does_nothing_on_the_birthday() {
    let lifecycle = &eager();
    let pops = Pops::new();
    let mut store = store_of(vec![seeker(1, 25, "a", Sex::Male), man(2, 25, "a")]);
    let mut env = Env::new("empty");
    assert_eq!(pool(&store, &pops, 1), Vec::<u64>::new());
    store
        .run_birthdays(&mut world(quiet(), lifecycle, &mut env, &pops, 3), today())
        .expect("birthdays");
    assert!(get(&store, 1).spouses.is_empty());
    assert!(pops.moves().is_empty());
}

// --- Scoring and the proposal -----------------------------------------------------------------------------------------------

fn score(store: &CharacterStore, pops: &Pops, env: &mut Env, seeker: u64, candidate: u64) -> i128 {
    store
        .marriage_score(
            &world(quiet(), &rules().lifecycle, env, pops, 7),
            seeker,
            candidate,
            today(),
        )
        .expect("score")
}

#[test]
fn a_candidate_is_scored_by_rank_age_opinion_fame_and_kinship() {
    let pops = Pops::new().kind("l", "labourer");
    let mut env = Env::new("score");
    // The seeker is a noble man of 30; the candidate a labourer's daughter of 15 with a fame of 10 points. They are first cousins
    // through a grandfather who died (archived), their fathers being brothers.
    let mut cousin = with_parents(woman(2, 15, "l"), 12, 90);
    cousin.personality.fame = 10_000;
    let store = store_of(vec![
        with_parents(seeker(1, 30, "a", Sex::Male), 11, 91),
        cousin,
        with_parents(man(11, 60, "a"), 20, 92),
        with_parents(man(12, 58, "a"), 20, 93),
    ]);
    env.bury(man(20, 90, "a"));
    let seeker_record = get(&store, 1);
    let candidate = get(&store, 2);
    // Rank 3 against 1: -15 x 2 = -30. Age: 15 years apart, 10 free: -2 x 5 = -10. Opinion: 0.5 x what the seeker thinks of her.
    // Fame: 0.2 x 10 points = 2. First cousins: -40.
    let thought = i128::from(
        personality_opinion(&rules().personality, seeker_record, candidate).expect("opinion"),
    );
    let expected = (-30 - 10 - 40) * POINT + thought * 500_000 + 2 * POINT;
    assert_eq!(score(&store, &pops, &mut env, 1, 2), expected);
}

#[test]
fn a_first_cousin_through_a_dead_grandparent_counts_even_when_the_parents_are_dead_too() {
    let pops = Pops::new();
    let mut env = Env::new("dead-line");
    // Their fathers 11 and 12 and the grandfather 20 are all dead: only the archive knows them.
    let store = store_of(vec![
        with_parents(seeker(1, 30, "a", Sex::Male), 11, 91),
        with_parents(woman(2, 30, "a"), 12, 90),
        woman(3, 30, "a"),
    ]);
    env.bury(with_parents(man(11, 60, "a"), 20, 92));
    env.bury(with_parents(man(12, 58, "a"), 20, 93));
    env.bury(man(20, 90, "a"));
    let cousin = score(&store, &pops, &mut env, 1, 2);
    let stranger = score(&store, &pops, &mut env, 1, 3);
    assert_eq!(stranger - cousin, 40 * POINT);
}

#[test]
fn second_cousins_share_a_great_grandparent_and_no_grandparent_and_the_unknown_matches_no_one() {
    let pops = Pops::new();
    let mut env = Env::new("second");
    // Fathers' fathers 21 and 22 are sons of the great-grandfather 30: the seeker's father 11 and her father 12 are first cousins.
    let store = store_of(vec![
        with_parents(seeker(1, 30, "a", Sex::Male), 11, 91),
        with_parents(woman(2, 30, "a"), 12, 90),
        // Two people with no known ancestry at all: the unknown character is nobody's relative.
        seeker(5, 30, "a", Sex::Male),
        woman(6, 30, "a"),
    ]);
    env.bury(with_parents(man(11, 60, "a"), 21, 92));
    env.bury(with_parents(man(12, 58, "a"), 22, 93));
    env.bury(with_parents(man(21, 80, "a"), 30, 94));
    env.bury(with_parents(man(22, 80, "a"), 30, 95));
    env.bury(man(30, 100, "a"));
    let second = score(&store, &pops, &mut env, 1, 2);
    let stranger = score(&store, &pops, &mut env, 5, 6);
    assert_eq!(stranger - second, 15 * POINT);
}

#[test]
fn in_laws_are_the_kin_of_a_spouse_living_or_dead_and_the_spouses_of_ones_siblings_and_children() {
    let pops = Pops::new();
    // The seeker 1 (father 70, mother 71) was married to 50, whose father is 51 and mother 3. Her sister is 2 and her daughter by an
    // earlier marriage is 65. The seeker's brother 62 is married to 64 and his son 61 to 5. Candidate 6 is a stranger, and 66 the wife
    // of a man who is nobody's relation. Every candidate is 30, like the seeker, so only kinship tells them apart.
    let relations = |wife_alive: bool| {
        let mut seeker_record = with_parents(seeker(1, 30, "a", Sex::Male), 70, 71);
        seeker_record.spouses = vec![50];
        seeker_record.children = vec![61];
        let mut wife = with_parents(woman(50, 30, "a"), 51, 3);
        wife.spouses = vec![1];
        let mut son = with_parents(man(61, 12, "a"), 1, 72);
        son.spouses = vec![5];
        let mut brother = with_parents(man(62, 28, "a"), 70, 71);
        brother.spouses = vec![64];
        let mut brothers_wife = woman(64, 30, "a");
        brothers_wife.spouses = vec![62];
        let mut sons_wife = woman(5, 30, "a");
        sons_wife.spouses = vec![61];
        let mut unrelated_wife = woman(66, 30, "a");
        unrelated_wife.spouses = vec![67];
        let mut people = vec![
            seeker_record,
            with_parents(woman(2, 30, "a"), 51, 3),
            with_parents(woman(65, 30, "a"), 99, 50),
            woman(3, 30, "a"),
            woman(6, 30, "a"),
            son,
            brother,
            brothers_wife,
            sons_wife,
            unrelated_wife,
            man(67, 30, "a"),
        ];
        let mut env = Env::new("in-laws");
        if wife_alive {
            people.push(wife);
        } else {
            env.bury(wife);
        }
        (store_of(people), env)
    };
    for alive in [false, true] {
        let (store, mut env) = relations(alive);
        let stranger = score(&store, &pops, &mut env, 1, 6);
        // A spouse's sister (2), mother (3) and child (65); the wife of the seeker's brother (64) and of his son (5): -10 each. The
        // wife of a man who is nobody's relation (66) costs nothing.
        for kin in [2, 3, 65, 64, 5] {
            let kin_score = score(&store, &pops, &mut env, 1, kin);
            assert_eq!(
                stranger - kin_score,
                10 * POINT,
                "{kin} (wife alive: {alive})"
            );
        }
        assert_eq!(
            score(&store, &pops, &mut env, 1, 66),
            stranger,
            "wife alive: {alive}"
        );
    }
}

#[test]
fn kinship_reads_recorded_parentage_not_a_parents_stale_list_of_children() {
    let pops = Pops::new();
    let mut env = Env::new("stale");
    // The seeker's wife 50 died (archived) and her saved list of children still names 2. Candidate 2's record names 50 as her
    // mother: her stepdaughter, an in-law. Then 2 is given two living parents, 70 and 71, by `set_parents`: the archived list of
    // 50 is not changed, and she is no relation.
    let mut hero = seeker(1, 30, "a", Sex::Male);
    hero.spouses = vec![50];
    let mut wife = woman(50, 30, "a");
    wife.spouses = vec![1];
    wife.children = vec![2];
    env.bury(wife);
    let mut store = store_of(vec![
        hero,
        with_parents(woman(2, 30, "a"), 99, 50),
        woman(3, 30, "a"),
        man(70, 60, "a"),
        woman(71, 55, "a"),
    ]);
    let stranger = score(&store, &pops, &mut env, 1, 3);
    assert_eq!(stranger - score(&store, &pops, &mut env, 1, 2), 10 * POINT);
    store.set_parents(2, 70, 71, today()).expect("parents");
    assert_eq!(score(&store, &pops, &mut env, 1, 2), stranger);
    // Nor does a list that names a child the child's record does not name make a parent or a child in the pool.
    let mut father = man(10, 55, "a");
    father.children = vec![1];
    let store = store_of(vec![
        seeker(1, 25, "a", Sex::Male),
        father,
        woman(2, 22, "a"),
    ]);
    assert_eq!(pool(&store, &pops, 1), [2]);
}

#[test]
fn only_the_most_severe_kinship_counts_and_stacking_would_choose_another_candidate() {
    let pops = Pops::new();
    let mut env = Env::new("stack");
    // Candidate 2 is a first cousin of the seeker (their fathers 11 and 12 are brothers) and also the sister of his dead wife 50 (they
    // share both parents): first cousin -40 and in-law -10 count -40, not -50. Candidate 3 is 62: 32 years apart is 22 over the 10
    // free, -2 each, -44. Counted once the cousin is the better match; stacked (-50) the old woman would be.
    let mut hero = with_parents(seeker(1, 30, "a", Sex::Male), 11, 91);
    hero.spouses = vec![50];
    let store = store_of(vec![
        hero,
        with_parents(woman(2, 30, "a"), 12, 80),
        woman(3, 62, "a"),
    ]);
    env.bury(with_parents(man(11, 60, "a"), 20, 92));
    env.bury(with_parents(man(12, 58, "a"), 20, 93));
    env.bury(man(20, 90, "a"));
    let mut wife = with_parents(woman(50, 30, "a"), 12, 80);
    wife.spouses = vec![1];
    env.bury(wife);
    // What each would score with no kinship at all: the same age and rank, neutral personalities.
    let stranger = {
        let alone = store_of(vec![seeker(1, 30, "a", Sex::Male), woman(2, 30, "a")]);
        score(&alone, &pops, &mut env, 1, 2)
    };
    let cousin = score(&store, &pops, &mut env, 1, 2);
    let old = score(&store, &pops, &mut env, 1, 3);
    assert_eq!(stranger - cousin, 40 * POINT);
    assert_eq!(stranger - old, 44 * POINT);
    let proposed = store
        .marriage_proposal(
            &world(quiet(), &rules().lifecycle, &mut env, &pops, 7),
            1,
            today(),
        )
        .expect("proposal")
        .expect("a candidate");
    assert_eq!(proposed, (2, cousin));
    // Had the two kinships stacked, the cousin's -50 would lose to -44.
    assert!(cousin - 10 * POINT < old);
}

#[test]
fn the_best_score_is_proposed_and_a_tie_goes_to_the_first_of_the_pool() {
    let pops = Pops::new().kind("l", "labourer").apart("a", "l", 10_000);
    let mut env = Env::new("best");
    // Women 20 and 21 are the same noble age mates and tie; 22 is a labourer's daughter (rank 1: -30) and 23 is 20 years older (-20).
    let store = store_of(vec![
        seeker(1, 30, "a", Sex::Male),
        woman(21, 30, "a"),
        woman(20, 30, "a"),
        woman(22, 30, "l"),
        woman(23, 50, "a"),
    ]);
    let best = store
        .marriage_proposal(
            &world(quiet(), &rules().lifecycle, &mut env, &pops, 7),
            1,
            today(),
        )
        .expect("proposal")
        .expect("a candidate");
    // The pool is 20, 21, 23 (the same place), then 22 (10 km): 20 and 21 tie on the best score and 20 comes first.
    assert_eq!(best.0, 20);
    assert!(best.1 > score(&store, &pops, &mut env, 1, 22));
    assert!(best.1 > score(&store, &pops, &mut env, 1, 23));
    assert_eq!(best.1, score(&store, &pops, &mut env, 1, 21));
}

// --- Acceptance -------------------------------------------------------------------------------------------------------------

#[test]
fn the_acceptance_chance_is_the_base_plus_the_score_clamped_to_five_and_ninety_five_percent() {
    let accept = &rules().lifecycle.marriage.accept;
    let at =
        |points: i128| CharacterStore::acceptance_ppb(accept, points * POINT).expect("a chance");
    assert_eq!(at(0), 500_000_000);
    assert_eq!(at(20), 700_000_000);
    assert_eq!(at(-20), 300_000_000);
    // The clamps, at both ends and far past them.
    assert_eq!(at(45), 950_000_000);
    assert_eq!(at(46), 950_000_000);
    assert_eq!(at(1_000_000), 950_000_000);
    assert_eq!(at(-45), 50_000_000);
    assert_eq!(at(-46), 50_000_000);
    assert_eq!(at(-1_000_000), 50_000_000);
    // A fraction of a point counts: 1.5 points is 0.5 + 0.015.
    assert_eq!(
        CharacterStore::acceptance_ppb(accept, 1_500_000_000).expect("a chance"),
        515_000_000
    );
}

/// A game of one seeker and its candidate: the seeker's birthday is [`today`], every wish is certain and the candidate's answer is
/// the roll `purpose` names, so the outcome is the roll's.
fn courted(
    seeker_id: u64,
    candidate: Character,
    others: Vec<Character>,
    pops: &Pops,
    seed: u64,
) -> CharacterStore {
    let lifecycle = &eager();
    let mut start = vec![seeker(seeker_id, 25, "a", Sex::Male), candidate];
    start.extend(others);
    let mut store = store_of(start);
    let mut env = Env::new("courted");
    store
        .run_birthdays(
            &mut world(quiet(), lifecycle, &mut env, pops, seed),
            today(),
        )
        .expect("birthdays");
    store
}
fn married(store: &CharacterStore, a: u64, b: u64) -> bool {
    get(store, a).spouses.contains(&b) && get(store, b).spouses.contains(&a)
}
/// The chance the candidate's side accepts a proposal of the seeker 1 to `candidate`, as the engine works it out.
fn answer_chance(store: &CharacterStore, pops: &Pops, candidate: u64) -> u32 {
    let mut env = Env::new("answer");
    let lifecycle = eager();
    let w = world(quiet(), &lifecycle, &mut env, pops, 7);
    let (chosen, points) = store
        .marriage_proposal(&w, 1, today())
        .expect("proposal")
        .expect("a candidate");
    assert_eq!(chosen, candidate);
    CharacterStore::acceptance_ppb(&rules().lifecycle.marriage.accept, points).expect("chance")
}

#[test]
fn an_ordinary_candidate_answers_on_its_own_stream_for_the_seeker_alone() {
    let pops = Pops::new().with_heads("a", 100);
    let start = store_of(vec![seeker(1, 25, "a", Sex::Male), woman(2, 20, "a")]);
    let chance = answer_chance(&start, &pops, 2);
    let (mut yes, mut no) = (0, 0);
    for seed in 0..40 {
        let store = courted(1, woman(2, 20, "a"), Vec::new(), &pops, seed);
        let expected = rolls(seed, 2, "marriage.accept.1", chance);
        assert_eq!(married(&store, 1, 2), expected, "seed {seed}");
        if expected {
            yes += 1;
        } else {
            no += 1;
        }
    }
    // Both answers occur, so the test sees the roll and not a constant.
    assert!(yes > 0 && no > 0, "{yes} {no}");
}

#[test]
fn a_candidate_of_a_ruling_house_is_decided_by_the_lowest_id_ruler_on_the_heads_stream() {
    let pops = Pops::new().with_heads("a", 100);
    // Candidate 2 is of the house `ui_neill`, whose rulers are 9 and 5: the head is 5.
    let house = |n: u64| ruler(housed(man(n, 50, "a"), "ui_neill"));
    let others = || vec![house(9), house(5)];
    let start = store_of({
        let mut all = vec![
            seeker(1, 25, "a", Sex::Male),
            housed(woman(2, 20, "a"), "ui_neill"),
        ];
        all.extend(others());
        all
    });
    let chance = answer_chance(&start, &pops, 2);
    let mut told_apart = false;
    for seed in 0..40 {
        let store = courted(
            1,
            housed(woman(2, 20, "a"), "ui_neill"),
            others(),
            &pops,
            seed,
        );
        let head = rolls(seed, 5, "marriage.accept.1.2", chance);
        assert_eq!(married(&store, 1, 2), head, "seed {seed}");
        // Neither the other ruler's stream nor the candidate's own decides it.
        told_apart |= head != rolls(seed, 2, "marriage.accept.1", chance)
            || head != rolls(seed, 9, "marriage.accept.1.2", chance);
    }
    assert!(told_apart);
}

#[test]
fn two_seekers_proposing_to_one_house_on_one_day_roll_independently_and_a_replay_repeats() {
    let pops = Pops::new().with_heads("a", 100);
    let lifecycle = &eager();
    // Seekers 1 and 3 (both born on 3 March) each have one candidate of the house `ui_neill`: women 2 and 4. The head is 5.
    let start = || {
        store_of(vec![
            seeker(1, 25, "a", Sex::Male),
            seeker(3, 26, "e", Sex::Male),
            housed(woman(2, 20, "a"), "ui_neill"),
            housed(woman(4, 20, "e"), "ui_neill"),
            ruler(housed(man(5, 50, "a"), "ui_neill")),
        ])
    };
    let pops = pops.apart("a", "e", 400_000);
    let chances = {
        let store = start();
        (answer_chance(&store, &pops, 2), {
            let mut env = Env::new("chance-3");
            let w = world(quiet(), lifecycle, &mut env, &pops, 7);
            let (_, points) = store
                .marriage_proposal(&w, 3, today())
                .expect("proposal")
                .expect("a candidate");
            CharacterStore::acceptance_ppb(&rules().lifecycle.marriage.accept, points)
                .expect("chance")
        })
    };
    let mut differ = false;
    for seed in 0..40 {
        let run = || {
            let mut store = start();
            let mut env = Env::new("two-seekers");
            store
                .run_birthdays(
                    &mut world(quiet(), lifecycle, &mut env, &pops, seed),
                    today(),
                )
                .expect("birthdays");
            store
        };
        let (first, again) = (run(), run());
        // The same pair and the same day: the same roll.
        assert_eq!(bytes(&first), bytes(&again), "seed {seed}");
        // Each seeker's own roll: the head's stream with that pair's purpose.
        let (one, two) = (
            rolls(seed, 5, "marriage.accept.1.2", chances.0),
            rolls(seed, 5, "marriage.accept.3.4", chances.1),
        );
        assert_eq!(married(&first, 1, 2), one, "seed {seed}");
        // Seeker 3 proposes after seeker 1 acted: it still has no wife and no candidate has been taken.
        assert_eq!(married(&first, 3, 4), two, "seed {seed}");
        differ |= one != two;
    }
    assert!(differ, "the two proposals never rolled differently");
}

// --- The transition ---------------------------------------------------------------------------------------------------------

fn marry(
    store: &mut CharacterStore,
    pops: &Pops,
    husband: u64,
    wife: u64,
) -> Result<ck_characters::marriage::Wedding, EventError> {
    let mut env = Env::new("marry");
    store.arrange_marriage(
        &mut world(&rules().events, &rules().lifecycle, &mut env, pops, 7),
        husband,
        wife,
        today(),
    )
}

#[test]
fn a_marriage_links_the_spouses_gives_the_wife_her_husbands_house_and_moves_her_head_to_his_pop() {
    let pops = Pops::new().with_heads("a", 10).with_heads("b", 10);
    let mut store = store_of(vec![
        housed(man(1, 25, "a"), "ui_neill"),
        housed(woman(2, 20, "b"), "dal_riata"),
    ]);
    let wedding = marry(&mut store, &pops, 1, 2).expect("marriage");
    assert_eq!((wedding.husband, wedding.wife), (1, 2));
    assert!(wedding.output.is_none());
    assert!(married(&store, 1, 2));
    let wife = get(&store, 2);
    assert_eq!(
        (wife.life.house.as_str(), wife.life.house_of_birth.as_str()),
        ("ui_neill", "dal_riata")
    );
    assert_eq!(wife.pop, locator("a"));
    assert_eq!(get(&store, 1).pop, locator("a"));
    assert_eq!(get(&store, 1).life.house, "ui_neill");
    // One head moved from `b` to `a`, an adult's.
    assert_eq!(pops.moves(), [("b".to_owned(), "a".to_owned(), false)]);
    assert_eq!((pops.heads_of("a"), pops.heads_of("b")), (11, 9));
}

#[test]
fn a_houseless_husband_marrying_into_a_house_takes_it_and_moves_instead_of_his_wife() {
    let pops = Pops::new();
    let mut store = store_of(vec![
        man(1, 25, "a"),
        housed(woman(2, 20, "b"), "dal_riata"),
    ]);
    marry(&mut store, &pops, 1, 2).expect("marriage");
    let husband = get(&store, 1);
    assert_eq!(husband.life.house, "dal_riata");
    // He keeps no house of birth to speak of: it stays what it was, none.
    assert_eq!(husband.life.house_of_birth, "");
    assert_eq!(husband.pop, locator("b"));
    assert_eq!(get(&store, 2).pop, locator("b"));
    assert_eq!(pops.moves(), [("a".to_owned(), "b".to_owned(), false)]);
    // Neither has a house: the wife moves, and neither gains one.
    let mut commoners = store_of(vec![man(1, 25, "a"), woman(2, 20, "b")]);
    marry(&mut commoners, &Pops::new(), 1, 2).expect("marriage");
    assert_eq!(get(&commoners, 2).pop, locator("a"));
    assert_eq!(get(&commoners, 2).life.house, "");
}

#[test]
fn spouses_already_in_one_pop_move_no_head() {
    let pops = Pops::new();
    let mut store = store_of(vec![man(1, 25, "a"), woman(2, 20, "a")]);
    marry(&mut store, &pops, 1, 2).expect("marriage");
    assert!(married(&store, 1, 2));
    assert!(pops.moves().is_empty());
}

#[test]
fn a_marriageable_bride_under_majority_moves_a_minors_head() {
    // A bride of 12 under the Gaelic majority of 14 moves a minor's head; one of 14 moves an adult's.
    let pops = Pops::new();
    let mut store = store_of(vec![man(1, 25, "a"), woman(2, 12, "b")]);
    marry(&mut store, &pops, 1, 2).expect("marriage");
    assert_eq!(pops.moves(), [("b".to_owned(), "a".to_owned(), true)]);
    let adult = Pops::new();
    let mut store = store_of(vec![man(1, 25, "a"), woman(2, 14, "b")]);
    marry(&mut store, &adult, 1, 2).expect("marriage");
    assert_eq!(adult.moves(), [("b".to_owned(), "a".to_owned(), false)]);
}

#[test]
fn two_pops_at_their_character_floors_marry_successfully() {
    // Each pop's heads are exactly its characters: after the move `b` holds none and `a` holds both.
    let pops = Pops::new().with_heads("a", 1).with_heads("b", 1);
    let mut store = store_of(vec![man(1, 25, "a"), woman(2, 20, "b")]);
    marry(&mut store, &pops, 1, 2).expect("marriage");
    assert_eq!((pops.heads_of("a"), pops.heads_of("b")), (2, 0));
    assert!(married(&store, 1, 2));
    assert_eq!(store.pop_count(&locator("a")), 2);
    assert_eq!(store.pop_count(&locator("b")), 0);
}

#[test]
fn a_destination_left_without_room_refuses_the_marriage_and_puts_the_head_back() {
    // `a` has 1 head for the husband and another character: the wife would make 3 characters of 2 heads.
    let pops = Pops::new().with_heads("a", 1).with_heads("b", 5);
    let mut store = store_of(vec![man(1, 25, "a"), woman(2, 20, "b"), man(3, 30, "a")]);
    let before = bytes(&store);
    let error = marry(&mut store, &pops, 1, 2).expect_err("refused");
    assert!(matches!(
        error,
        EventError::Rules(ck_characters::Error::PopFull)
    ));
    assert_eq!(bytes(&store), before);
    assert_eq!((pops.heads_of("a"), pops.heads_of("b")), (1, 5));
    assert_eq!(*pops.restored.borrow(), 1);
}

#[test]
fn a_refused_head_transfer_leaves_pops_links_and_houses_unchanged() {
    let mut pops = Pops::new().with_heads("a", 10).with_heads("b", 10);
    pops.refuse_move = true;
    let mut store = store_of(vec![
        housed(man(1, 25, "a"), "ui_neill"),
        housed(woman(2, 20, "b"), "dal_riata"),
    ]);
    let before = bytes(&store);
    let error = marry(&mut store, &pops, 1, 2).expect_err("refused");
    assert!(matches!(
        error,
        EventError::Rules(ck_characters::Error::MissingPop)
    ));
    assert_eq!(bytes(&store), before);
    assert!(get(&store, 1).spouses.is_empty() && get(&store, 2).spouses.is_empty());
    assert_eq!(get(&store, 2).life.house, "dal_riata");
    assert_eq!((pops.heads_of("a"), pops.heads_of("b")), (10, 10));
}

#[test]
fn a_marriage_between_the_wrong_people_is_refused_with_nothing_changed() {
    let pops = Pops::new();
    let mut taken = woman(3, 22, "a");
    taken.spouses = vec![4];
    let mut husband = man(4, 22, "a");
    husband.spouses = vec![3];
    let mut store = store_of(vec![
        man(1, 25, "a"),
        man(2, 25, "a"),
        taken,
        husband,
        woman(5, 20, "a"),
    ]);
    let before = bytes(&store);
    // Two men; a wife with a living husband; a missing character.
    for (a, b) in [(1, 2), (1, 3), (1, 99)] {
        assert!(marry(&mut store, &pops, a, b).is_err(), "{a} {b}");
        assert_eq!(bytes(&store), before);
    }
    assert!(pops.moves().is_empty());
}

#[test]
fn a_widow_marries_again_and_the_happening_fires_once_for_each_spouse() {
    let pops = Pops::new();
    let mut widow = woman(2, 30, "a");
    widow.spouses = vec![9];
    let mut store = store_of(vec![man(1, 25, "a"), widow]);
    let mut env = Env::new("widow");
    store
        .arrange_marriage(
            &mut world(&rules().events, &rules().lifecycle, &mut env, &pops, 7),
            1,
            2,
            today(),
        )
        .expect("marriage");
    assert_eq!(get(&store, 2).spouses, [9, 1]);
    assert_eq!(get(&store, 1).spouses, [2]);
    for id in [1, 2] {
        let lines = env.lines(id);
        assert_eq!(
            lines.iter().filter(|line| line.contains("married")).count(),
            1,
            "{id}: {lines:?}"
        );
    }
}

/// A log that can append nothing.
struct Failing;
impl LogSink for Failing {
    fn append(&mut self, _: u64, _: GameTime, _: &str) -> io::Result<()> {
        Err(io::Error::other("no log"))
    }
}

#[test]
fn a_marriage_whose_output_fails_stands_and_says_so() {
    let pops = Pops::new();
    let mut store = store_of(vec![man(1, 25, "a"), woman(2, 20, "b")]);
    let env = Env::new("failing");
    let mut archive = env.archive;
    let wedding = store
        .arrange_marriage(
            &mut World {
                rules: &rules().events,
                personality: &rules().personality,
                genetics: &rules().genetics,
                lifecycle: &rules().lifecycle,
                seed: 7,
                populations: &pops,
                archive: &mut archive,
                logs: &mut Failing,
            },
            1,
            2,
            today(),
        )
        .expect("committed");
    assert!(matches!(wedding.output, Some(EventError::Log(_))));
    assert!(married(&store, 1, 2));
    assert_eq!(get(&store, 2).pop, locator("a"));
}

// --- Through the economy ----------------------------------------------------------------------------------------------------

fn pop(workers: u64, dependents: u64, mean: i64) -> PopState {
    let mut genetics = PopGenetics::default();
    genetics.mean[0] = mean;
    genetics.paternal.insert("p".to_owned(), 1000);
    genetics.maternal.insert("m".to_owned(), 1000);
    PopState {
        kind: "noble".to_owned(),
        culture: "ulaid".to_owned(),
        religion: "christianity".to_owned(),
        literacy_pm: 0,
        workers,
        dependents,
        employment: Vec::new(),
        stock: Vec::new(),
        consumption: PopConsumption::default(),
        counters: CounterBuckets::default(),
        short_on_rent: false,
        housed_last: false,
        genetics,
        pending_births: 0,
        pending_deaths: 0,
    }
}
fn place(id: &str, pop: PopState) -> PlaceState {
    PlaceState {
        id: id.to_owned(),
        coastal: false,
        pops: vec![pop],
        buildings: Vec::new(),
        prices_milli: Vec::new(),
        prepared_m2: 0,
        development_pm: 0,
        price_drift_carries: Vec::new(),
        last_market_orders: None,
        counters: CounterBuckets::default(),
    }
}
fn economy(source: PopState, destination: PopState) -> EconomyState {
    EconomyState {
        places: vec![place("a", destination), place("b", source)],
    }
}
fn heads_in(state: &EconomyState, index: usize) -> (u64, u64) {
    (
        state.places[index].pops[0].workers,
        state.places[index].pops[0].dependents,
    )
}

#[test]
fn a_marriage_through_the_economy_moves_the_wifes_worker_head_and_blends_the_genetics() {
    let mut state = economy(pop(5, 2, 8000), pop(3, 1, 2000));
    let source = state.places[1].pops[0].genetics.clone();
    let mut expected = state.places[0].pops[0].genetics.clone();
    expected.blend(4, &source, 1).expect("blend");
    let mut store = store_of(vec![man(1, 25, "a"), woman(2, 20, "b")]);
    let mut env = Env::new("economy");
    {
        let ledger = Ledger::new(&mut state);
        store
            .arrange_marriage(
                &mut world(quiet(), &rules().lifecycle, &mut env, &ledger, 7),
                1,
                2,
                today(),
            )
            .expect("marriage");
    }
    assert_eq!(get(&store, 2).pop, locator("a"));
    // The adult wife's worker head moved, unemployed; no death and no birth was counted; the destination's genetics blended.
    assert_eq!((heads_in(&state, 0), heads_in(&state, 1)), ((4, 1), (4, 2)));
    assert_eq!(state.places[0].pops[0].genetics, expected);
    for index in 0..2 {
        assert_eq!(state.places[index].pops[0].pending_deaths, 0);
        assert_eq!(state.places[index].pops[0].pending_births, 0);
    }
}

#[test]
fn a_marriageable_minor_bride_in_a_pop_of_one_worker_and_no_dependents_moves_that_worker() {
    // A bride of 12 (a minor under the Gaelic 14) in a pop of one worker and no dependents, and a destination at its floor.
    let mut state = economy(pop(1, 0, 8000), pop(1, 0, 2000));
    let mut store = store_of(vec![man(1, 25, "a"), woman(2, 12, "b")]);
    let mut env = Env::new("economy-minor");
    let ledger = Ledger::new(&mut state);
    let wedding = store.arrange_marriage(
        &mut World {
            rules: quiet(),
            personality: &rules().personality,
            genetics: &rules().genetics,
            lifecycle: &rules().lifecycle,
            seed: 7,
            populations: &ledger,
            archive: &mut env.archive,
            logs: &mut env.logs,
        },
        1,
        2,
        today(),
    );
    wedding.expect("marriage");
    assert!(married(&store, 1, 2));
    assert_eq!((heads_in(&state, 0), heads_in(&state, 1)), ((2, 0), (0, 0)));
}

#[test]
fn a_source_that_cannot_write_the_pops_refuses_a_marriage_between_two_pops_but_not_within_one() {
    // The economy through a shared reference has spare heads at both ends but cannot move one: the wedding is refused whole, with
    // nothing changed, rather than committed with the pops' counts and genetics left as they were.
    let state = economy(pop(5, 2, 8000), pop(3, 1, 2000));
    let mut store = store_of(vec![man(1, 25, "a"), woman(2, 20, "b")]);
    let before = bytes(&store);
    let mut env = Env::new("read-only");
    let error = store
        .arrange_marriage(
            &mut world(quiet(), &rules().lifecycle, &mut env, &state, 7),
            1,
            2,
            today(),
        )
        .expect_err("refused");
    assert!(matches!(
        error,
        EventError::Rules(ck_characters::Error::Lifecycle)
    ));
    assert_eq!(bytes(&store), before);
    assert_eq!((heads_in(&state, 0), heads_in(&state, 1)), ((3, 1), (5, 2)));
    // Two people of one pop have no head to move: they marry.
    let mut together = store_of(vec![man(1, 25, "a"), woman(2, 20, "a")]);
    together
        .arrange_marriage(
            &mut world(quiet(), &rules().lifecycle, &mut env, &state, 7),
            1,
            2,
            today(),
        )
        .expect("marriage");
    assert!(married(&together, 1, 2));
    // A bare head-count function cannot write either.
    let counts = |_: &PopLocator| Some(100_u64);
    let mut apart = store_of(vec![man(1, 25, "a"), woman(2, 20, "b")]);
    let error = apart
        .arrange_marriage(
            &mut world(quiet(), &rules().lifecycle, &mut env, &counts, 7),
            1,
            2,
            today(),
        )
        .expect_err("refused");
    assert!(matches!(
        error,
        EventError::Rules(ck_characters::Error::Lifecycle)
    ));
}

#[test]
fn the_ledger_knows_the_distances_of_the_nearby_places_it_was_given() {
    let nearby: BTreeMap<String, Vec<NearbyPlace>> = BTreeMap::from([(
        "a".to_owned(),
        vec![NearbyPlace {
            id: "b".to_owned(),
            distance_m: 42_000,
        }],
    )]);
    let mut state = economy(pop(5, 2, 8000), pop(3, 1, 2000));
    let ledger = Ledger::with_nearby(&mut state, &nearby);
    assert_eq!(
        ledger.distance_m(&locator("a"), &locator("b")),
        Some(42_000)
    );
    assert_eq!(ledger.distance_m(&locator("a"), &locator("a")), Some(0));
    // Not listed (the reverse direction is its own entry), or no such place.
    assert_eq!(ledger.distance_m(&locator("b"), &locator("a")), None);
    assert_eq!(ledger.distance_m(&locator("a"), &locator("nowhere")), None);
    let bare = Ledger::new(&mut state);
    assert_eq!(bare.distance_m(&locator("a"), &locator("b")), None);
    assert_eq!(bare.distance_m(&locator("b"), &locator("b")), Some(0));
}

// --- Determinism ------------------------------------------------------------------------------------------------------------

#[test]
fn a_year_of_wishes_is_deterministic_under_the_seed_and_every_marriage_is_whole() {
    let lifecycle = &eager();
    // Twelve men of 20 to 31 and twelve women, half of them a place away, all with a birthday on the day: every wish is certain.
    let start = || {
        let mut all = Vec::new();
        for n in 0..12_u16 {
            all.push(seeker(1 + u64::from(n), 20 + n, "a", Sex::Male));
            all.push(person(
                100 + u64::from(n),
                date(748 - n, 3, 3),
                if n % 2 == 0 { "a" } else { "n" },
                Sex::Female,
            ));
        }
        all
    };
    let run = |seed: u64| {
        let pops = map().with_heads("a", 1000).with_heads("n", 1000);
        let mut store = store_of(start());
        let mut env = Env::new("year");
        store
            .run_birthdays(
                &mut world(quiet(), lifecycle, &mut env, &pops, seed),
                today(),
            )
            .expect("birthdays");
        (store, pops)
    };
    let mut marriages = 0;
    for seed in 0..6 {
        let ((first, pops), (again, _)) = (run(seed), run(seed));
        // The same seed, the same year.
        assert_eq!(bytes(&first), bytes(&again), "seed {seed}");
        for c in first.iter() {
            for spouse in &c.spouses {
                // Reciprocal, one spouse each, and the wife in her husband's pop with his house.
                let other = get(&first, *spouse);
                assert!(other.spouses.contains(&c.id));
                assert_eq!((c.spouses.len(), other.spouses.len()), (1, 1));
                if c.sex == Sex::Female {
                    assert_eq!(c.pop, other.pop);
                    assert_eq!(c.life.house, other.life.house);
                }
                marriages += 1;
            }
        }
        // Each marriage across places moved one head, and heads agree with characters.
        for place in ["a", "n"] {
            let held = u64::try_from(first.pop_count(&locator(place))).expect("count");
            assert!(pops.heads_of(place) >= held, "seed {seed} {place}");
        }
    }
    assert!(marriages > 0, "no marriage in any seed");
}
