//! Death, coming of age, conception, delivery and names (CK-364, characters slice 5a).
//!
//! Characters whose exact state a case needs are built as records and loaded through the canonical encoding. The events are none
//! (a quiet world), so a happening's roll cannot hide the behaviour under test, except where the shipped events are the subject.

use ck_characters::{
    Archive, ArchiveSink, Character, CharacterStore, Context, EventError, GeneticRecord, LifeLog,
    LogSink, NewCharacter, PersonalityRecord, PopLocator, Populations, Sex, UNKNOWN, World,
    cycle::{self, Pressure, shown_name},
    genetics::Mendel,
};
use ck_economy::{EventRules, LifeCycle, PopGenetics, Rules, events::Registry};
use ck_sim::{Canonical, Date, GameTime, Stream};
use std::{cell::RefCell, collections::BTreeMap, io, path::PathBuf, sync::OnceLock};

fn date(year: u16, month: u8, day: u8) -> GameTime {
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour: 0,
    })
    .expect("valid date")
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
/// No events at all: a death or a birth then writes only the lines the life cycle itself writes.
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

/// The pops: `a` and `m` Irish nobles (`ulaid`), `e` an English one (`northumbrian`) and `p` a Pictish one. Every pop has `heads`
/// heads; head changes are recorded in `calls`, and a refused addition of births is a field.
struct Pops {
    heads: u64,
    religion: &'static str,
    necessities: u16,
    refuse_births: bool,
    refuse_heads: bool,
    calls: RefCell<Vec<String>>,
}
impl Pops {
    fn new() -> Self {
        Self {
            heads: 1_000_000,
            religion: "",
            necessities: 1000,
            refuse_births: false,
            refuse_heads: false,
            calls: RefCell::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}
impl Populations for Pops {
    fn heads(&self, _: &PopLocator) -> Option<u64> {
        Some(self.heads)
    }
    fn culture(&self, locator: &PopLocator) -> Option<String> {
        Some(
            match locator.place.as_str() {
                "e" => "northumbrian",
                "p" => "pictish",
                _ => "ulaid",
            }
            .to_owned(),
        )
    }
    fn kind(&self, _: &PopLocator) -> Option<String> {
        Some("noble".to_owned())
    }
    fn luxuries_pm(&self, _: &PopLocator) -> Option<u16> {
        Some(600)
    }
    fn religion(&self, _: &PopLocator) -> Option<String> {
        Some(self.religion.to_owned())
    }
    fn necessities_pm(&self, _: &PopLocator) -> Option<u16> {
        Some(self.necessities)
    }
    fn genetics(&self, _: &PopLocator) -> Option<PopGenetics> {
        Some(irish())
    }
    fn add_births(&self, locator: &PopLocator, count: u64) -> Result<(), ck_characters::Error> {
        if self.refuse_births {
            return Err(ck_characters::Error::MissingPop);
        }
        self.calls
            .borrow_mut()
            .push(format!("births {} {count}", locator.place));
        Ok(())
    }
    fn remove_head(
        &self,
        locator: &PopLocator,
        minor: bool,
    ) -> Result<ck_economy::HeadUndo, ck_characters::Error> {
        if self.refuse_heads {
            return Err(ck_characters::Error::MissingPop);
        }
        self.calls.borrow_mut().push(format!(
            "death {} {}",
            locator.place,
            if minor { "minor" } else { "adult" }
        ));
        Ok(ck_economy::HeadUndo::default())
    }
    fn restore_head(&self, _: &ck_economy::HeadUndo) {
        self.calls.borrow_mut().push("restored".to_owned());
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

/// The file of a test's storage, as it is now.
fn existing(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ck-lifecycle-{}-{tag}", std::process::id()))
}
/// A new, empty file of a test's storage.
fn path(tag: &str) -> PathBuf {
    let path = existing(tag);
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
fn world<'a>(
    events: &'a EventRules,
    lifecycle: &'a LifeCycle,
    env: &'a mut Env,
    pops: &'a Pops,
) -> World<'a, Pops, Archive, LifeLog> {
    World {
        rules: events,
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle,
        seed: 7,
        populations: pops,
        archive: &mut env.archive,
        logs: &mut env.logs,
    }
}

/// A record with ordinary stats, humours of 5 and fecundity 10, in pop `place`.
fn person(id: u64, birth: GameTime, place: &str, sex: Sex) -> Character {
    let mut genetics = GeneticRecord::default();
    genetics.value[9..13].copy_from_slice(&[5000; 4]);
    for stat in 0..9 {
        genetics.value[stat] = 10_000;
    }
    // A man of 70 kg: the shown weight is held to a floor of 30 kg, so a weight of 10 would be an extreme.
    genetics.value[1] = 70_000;
    let mut life = ck_characters::LifeRecord::default();
    "irish".clone_into(&mut life.pool);
    let mut character = Character {
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
    };
    // Its weight is the base weight, as a character's is when it is created (CK-367).
    character.life.weight = character.base_weight(&rules().genetics);
    character
}
fn store_of(characters: Vec<Character>) -> CharacterStore {
    let next = characters.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    store_with_next(characters, next)
}
fn store_with_next(characters: Vec<Character>, next: u64) -> CharacterStore {
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

/// A married couple: woman 1 (born `woman`) and man 2 (born `man`) in pop `a`.
fn couple(woman: GameTime, man: GameTime) -> Vec<Character> {
    let mut wife = person(1, woman, "a", Sex::Female);
    let mut husband = person(2, man, "a", Sex::Male);
    wife.spouses = vec![2];
    husband.spouses = vec![1];
    vec![wife, husband]
}

// --- Coming of age ----------------------------------------------------------------------------------------------------------

#[test]
fn a_character_comes_of_age_once_on_the_birthday_of_its_subgroup_and_sex() {
    // Gaelic (the Irish pops) 17 and 14, English 15 and 12.
    let lifecycle = &rules().lifecycle;
    for (place, sex, majority) in [
        ("a", Sex::Male, 17),
        ("a", Sex::Female, 14),
        ("e", Sex::Male, 15),
        ("e", Sex::Female, 12),
    ] {
        let born = date(740, 5, 5);
        let mut store = store_of(vec![person(1, born, place, sex)]);
        let pops = Pops::new();
        let mut env = Env::new(&format!("age-{place}-{majority}"));
        let came = |env: &Env| {
            env.lines(1)
                .iter()
                .filter(|line| line.as_str() == "c1 came of age.")
                .count()
        };
        for years in 1..=majority + 3 {
            store
                .run_birthdays(
                    &mut world(&rules().events, lifecycle, &mut env, &pops),
                    date(740 + years, 5, 5),
                )
                .expect("birthdays");
            let expected = usize::from(years >= majority);
            assert_eq!(came(&env), expected, "{place} {sex:?} at {years}");
        }
        // Not on any other day of the year.
        store
            .run_birthdays(
                &mut world(&rules().events, lifecycle, &mut env, &pops),
                date(740 + majority + 4, 5, 6),
            )
            .expect("birthdays");
        assert_eq!(came(&env), 1);
    }
}

#[test]
fn a_character_created_of_age_never_comes_of_age() {
    let lifecycle = &rules().lifecycle;
    let mut store = store_of(vec![person(1, date(700, 5, 5), "a", Sex::Male)]);
    let pops = Pops::new();
    let mut env = Env::new("never");
    for years in 60..65 {
        store
            .run_birthdays(
                &mut world(&rules().events, lifecycle, &mut env, &pops),
                date(700 + years, 5, 5),
            )
            .expect("birthdays");
    }
    assert!(!env.lines(1).iter().any(|line| line.contains("came of age")));
}

// --- Death ------------------------------------------------------------------------------------------------------------------

#[test]
fn death_is_certain_at_the_105th_birthday_once_before_the_monthly_roll_and_for_a_character_found_older()
 {
    let lifecycle = &rules().lifecycle;
    // A is 105 on 805-3-1, which is a 1st; B was already older when found; C is 104.
    let mut a = person(1, date(700, 3, 1), "a", Sex::Male);
    a.name = "Alpha".into();
    let b = person(2, date(690, 1, 1), "a", Sex::Female);
    let c = person(3, date(700, 6, 1), "a", Sex::Male);
    let mut store = store_of(vec![a, b, c]);
    let pops = Pops::new();
    let mut env = Env::new("certain");
    // No pressure on the pop: nothing but certain death can kill C on the 1st.
    let pressure = Pressure::from([(locator("a"), 0)]);
    store
        .run_pre_economy(
            &mut world(quiet(), lifecycle, &mut env, &pops),
            date(805, 3, 1),
            &pressure,
        )
        .expect("the 1st");
    assert!(store.get(1).is_none() && store.get(2).is_none());
    assert!(store.get(3).is_some(), "104 is not certain");
    for id in [1, 2] {
        let record = env.archive.get(id).expect("read").expect("archived");
        assert_eq!(record.death, Some(date(805, 3, 1)));
        assert_eq!(record.life.death_cause.as_deref(), Some("old_age"));
    }
    assert_eq!(env.lines(1), ["Alpha died in old age."], "once");
    assert_eq!(env.lines(2), ["c2 died in old age."]);
    // Each death removed one head, counted once, as an adult.
    assert_eq!(pops.calls(), ["death a adult", "death a adult"]);
}

#[test]
fn the_monthly_roll_kills_with_the_line_of_the_age_at_death() {
    let lifecycle = &rules().lifecycle;
    // Ages 1, 10, 40 and 70 on 800-1-1; a pressure so high that the annual chance is capped at 1 and every monthly roll succeeds.
    let mut store = store_of(vec![
        person(1, date(798, 6, 1), "a", Sex::Female),
        person(2, date(789, 6, 1), "a", Sex::Male),
        person(3, date(759, 6, 1), "a", Sex::Female),
        person(4, date(729, 6, 1), "a", Sex::Male),
    ]);
    let pops = Pops::new();
    let mut env = Env::new("lines");
    let pressure = Pressure::from([(locator("a"), 1_000_000_000_000_000)]);
    store
        .run_monthly_deaths(
            &mut world(quiet(), lifecycle, &mut env, &pops),
            date(800, 1, 1),
            &pressure,
        )
        .expect("the monthly roll");
    assert_eq!(store.iter().count(), 0);
    assert_eq!(env.lines(1), ["c1 died in infancy."]);
    assert_eq!(env.lines(2), ["c2 died young."]);
    assert_eq!(env.lines(3), ["c3 died."]);
    assert_eq!(env.lines(4), ["c4 died in old age."]);
    assert_eq!(
        env.archive
            .get(3)
            .unwrap()
            .unwrap()
            .life
            .death_cause
            .as_deref(),
        Some("natural")
    );
    // Minors (under the Irish 14 and 17) take a dependent first; adults a worker.
    assert_eq!(
        pops.calls(),
        [
            "death a minor",
            "death a minor",
            "death a adult",
            "death a adult"
        ]
    );
}

#[test]
fn the_annual_chance_reads_the_characters_own_values_and_its_pops_pressure() {
    let lifecycle = &rules().lifecycle;
    let today = date(800, 1, 1);
    let mut man = person(1, date(759, 6, 1), "a", Sex::Male);
    let chance = |c: &Character, pressure: &Pressure| {
        CharacterStore::annual_death_ppb(c, &rules().genetics, lifecycle, pressure, today)
            .expect("chance")
    };
    assert_eq!(chance(&man, &Pressure::new()), 9_828_427, "age 40, neutral");
    // K comes from the character's pop.
    let pressure = Pressure::from([(locator("a"), 2_000_000_000)]);
    assert_eq!(chance(&man, &pressure), 19_656_854);
    let elsewhere = Pressure::from([(locator("m"), 2_000_000_000)]);
    assert_eq!(chance(&man, &elsewhere), 9_828_427);
    // Constitution (4th stat from the end) scales the base term, longevity the old-age term, both floored at 0.3.
    man.genetics.value[7] = 14_000;
    assert_eq!(chance(&man, &Pressure::new()), 8_708_427);
    man.genetics.value[7] = 40_000;
    assert_eq!(chance(&man, &Pressure::new()), 4_928_427);
    let mut old = person(2, date(719, 6, 1), "a", Sex::Male);
    old.genetics.value[6] = 3_000;
    assert_eq!(chance(&old, &Pressure::new()), 129_188_046);
    old.genetics.value[6] = 40_000;
    assert_eq!(chance(&old, &Pressure::new()), 34_152_899);
}

#[test]
fn an_archive_failure_refuses_a_death_with_nothing_changed_and_the_next_pass_tries_again() {
    let lifecycle = &rules().lifecycle;
    let mut store = store_of(vec![person(1, date(690, 1, 1), "a", Sex::Male)]);
    let before = store.to_bytes().expect("encode");
    let pops = Pops::new();
    let mut logs = LifeLog::open(path("flaky-archive-logs")).expect("logs");
    let mut archive = FlakyArchive {
        inner: Archive::open(path("flaky-archive-arch")).expect("archive"),
        armed: true,
    };
    {
        let mut world = World {
            rules: quiet(),
            personality: &rules().personality,
            genetics: &rules().genetics,
            lifecycle,
            seed: 7,
            populations: &pops,
            archive: &mut archive,
            logs: &mut logs,
        };
        let error = store
            .run_certain_deaths(&mut world, date(800, 1, 1))
            .unwrap_err();
        assert!(matches!(error, EventError::Archive(_)), "{error:?}");
    }
    assert_eq!(store.to_bytes().expect("encode"), before, "nothing changed");
    assert_eq!(
        pops.calls(),
        ["death a adult", "restored"],
        "the head was put back"
    );
    assert!(logs.read(1).expect("log").is_empty(), "no line was written");
    archive.armed = false;
    let mut world = World {
        rules: quiet(),
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle,
        seed: 7,
        populations: &pops,
        archive: &mut archive,
        logs: &mut logs,
    };
    store
        .run_certain_deaths(&mut world, date(800, 2, 1))
        .expect("the next pass");
    assert!(store.get(1).is_none());
    assert_eq!(pops.calls(), ["death a adult", "restored", "death a adult"]);
}

/// An archive whose append fails while `armed`.
struct FlakyArchive {
    inner: Archive,
    armed: bool,
}
impl ArchiveSink for FlakyArchive {
    fn append(&mut self, record: &Character) -> io::Result<()> {
        if self.armed {
            Err(io::Error::other("injected archive failure"))
        } else {
            self.inner.append(record)
        }
    }
    fn get(&self, id: u64) -> io::Result<Option<Character>> {
        self.inner.get(id)
    }
    fn children_of(&self, parent: u64) -> io::Result<Vec<u64>> {
        self.inner.children_of(parent)
    }
}
/// A log whose append fails while `armed`.
struct FlakyLog {
    inner: LifeLog,
    armed: bool,
}
impl LogSink for FlakyLog {
    fn append(&mut self, id: u64, date: GameTime, text: &str) -> io::Result<()> {
        if self.armed {
            Err(io::Error::other("injected log failure"))
        } else {
            self.inner.append(id, date, text)
        }
    }
}

// --- Conception -------------------------------------------------------------------------------------------------------------

fn conception(couple: &[Character], pops: &Pops, lifecycle: &LifeCycle, today: GameTime) -> u32 {
    let store = store_of(couple.to_vec());
    let mut env = Env::new("conception-chance");
    let world = world(quiet(), lifecycle, &mut env, pops);
    store
        .conception_ppb(&world, get(&store, 1), get(&store, 2), today)
        .expect("chance")
}

#[test]
fn the_conception_chance_is_the_worked_product_exactly() {
    let lifecycle = &rules().lifecycle;
    let today = date(768, 1, 1);
    // A woman of 35 (0.85) and a man of 40 (1.0), both of fecundity 10, trying: 0.2 x 0.85.
    let couple = couple(date(732, 6, 1), date(727, 6, 1));
    let pops = Pops::new();
    assert_eq!(conception(&couple, &pops, lifecycle, today), 170_000_000);
    // The Latin church's days of abstinence: x 0.85.
    let christian = Pops {
        religion: "christianity",
        ..Pops::new()
    };
    assert_eq!(
        conception(&couple, &christian, lifecycle, today),
        144_500_000
    );
    // Both fecundities 25 on the plateau: 0.2 x 2.5 x 2.5 is above 1, and the chance is capped at 1.
    let mut fertile = self::couple(date(743, 6, 1), date(738, 6, 1));
    for c in &mut fertile {
        c.genetics.value[8] = 25_000;
    }
    assert_eq!(conception(&fertile, &pops, lifecycle, today), 1_000_000_000);
    assert_eq!(
        conception(&fertile, &christian, lifecycle, today),
        1_000_000_000,
        "0.2 x 6.25 x 0.85 is still above 1"
    );
    // Fecundity 5 each: 0.2 x 0.5 x 0.5 (the stat is shown as thousandths, divided by 10).
    let mut barren = self::couple(date(743, 6, 1), date(738, 6, 1));
    for c in &mut barren {
        c.genetics.value[8] = 5_000;
    }
    assert_eq!(conception(&barren, &pops, lifecycle, today), 50_000_000);
}

#[test]
fn the_age_curves_are_linear_between_points_and_flat_beyond_their_ends() {
    let lifecycle = &rules().lifecycle;
    let today = date(768, 1, 1);
    let pops = Pops::new();
    // The woman's age and the man's, and the chance, with the other on the plateau (25 and 30).
    let chance = |woman_age: u16, man_age: u16| {
        conception(
            &couple(
                date(768 - woman_age - 1, 6, 1),
                date(768 - man_age - 1, 6, 1),
            ),
            &pops,
            lifecycle,
            today,
        )
    };
    assert_eq!(chance(25, 30), 200_000_000);
    assert_eq!(
        chance(14, 30),
        0,
        "below the first point's age: its factor 0"
    );
    assert_eq!(chance(60, 30), 0, "above the last: 0");
    // 47 lies at 0.17 between (45, 0.25) and (50, 0.05); a wife past 42 is avoiding (x 0.1).
    assert_eq!(chance(47, 30), 3_400_000);
    // A man of 16 lies a third of the way from (15, 0) to (18, 1): 0.2 / 3 rounded down.
    assert_eq!(chance(25, 16), 66_666_666);
    assert_eq!(chance(25, 90), 10_000_000, "above 80 the factor stays 0.05");
}

#[test]
fn the_relation_mode_sets_the_factor() {
    let lifecycle = &rules().lifecycle;
    let today = date(768, 1, 1);
    let pops = Pops::new();
    let mode = |edit: &dyn Fn(&mut [Character]), woman_age: u16| {
        let mut pair = couple(date(768 - woman_age - 1, 6, 1), date(727, 6, 1));
        edit(&mut pair);
        conception(&pair, &pops, lifecycle, today)
    };
    let nothing = |_: &mut [Character]| {};
    assert_eq!(mode(&nothing, 25), 200_000_000, "trying");
    // A noble husband wants four children: with four together it is neutral (x 0.5), with three it is still trying.
    let children = |count: u64| {
        move |pair: &mut [Character]| {
            for c in pair.iter_mut() {
                c.children = (10..10 + count).collect();
            }
        }
    };
    assert_eq!(mode(&children(3), 25), 200_000_000);
    assert_eq!(mode(&children(4), 25), 100_000_000, "neutral");
    // Avoidance (x 0.1) from the wife's 42nd year: 43 is 0.39 on the curve.
    assert_eq!(mode(&nothing, 43), 7_800_000);
    assert_eq!(mode(&nothing, 42), 9_200_000, "42 is 0.46 on the curve");
    // Lust or chastity moves it one step: chastity (+5) trying to neutral and neutral to avoidance, lust (-5) the other way.
    let lust = |value: i64| move |pair: &mut [Character]| pair[0].personality.vice[2] = value;
    assert_eq!(mode(&lust(5000), 25), 100_000_000);
    assert_eq!(mode(&lust(4999), 25), 200_000_000);
    assert_eq!(
        mode(&lust(-5000), 25),
        200_000_000,
        "trying cannot go further"
    );
    let neutral_then = |value: i64| {
        move |pair: &mut [Character]| {
            children(4)(pair);
            pair[0].personality.vice[2] = value;
        }
    };
    assert_eq!(
        mode(&neutral_then(5000), 25),
        20_000_000,
        "neutral to avoidance"
    );
    assert_eq!(
        mode(&neutral_then(-5000), 25),
        200_000_000,
        "neutral to trying"
    );
    // Low necessities in the wife's pop are avoidance too.
    let poor = Pops {
        necessities: 599,
        ..Pops::new()
    };
    let pair = couple(date(742, 6, 1), date(727, 6, 1));
    assert_eq!(conception(&pair, &poor, lifecycle, today), 20_000_000);
    let enough = Pops {
        necessities: 600,
        ..Pops::new()
    };
    assert_eq!(conception(&pair, &enough, lifecycle, today), 200_000_000);
}

/// The 1st on which conceptions are run, for couples whose chance is 1 or 0.
fn conceive(store: &mut CharacterStore, today: GameTime) -> Result<(), EventError> {
    let lifecycle = &rules().lifecycle;
    let pops = Pops::new();
    let mut env = Env::new("conceive");
    store.run_conceptions(&mut world(quiet(), lifecycle, &mut env, &pops), today)
}
/// A couple whose conception chance is 1: fecundity 25 on the plateau.
fn sure_couple() -> Vec<Character> {
    let mut pair = couple(date(743, 6, 1), date(738, 6, 1));
    for c in &mut pair {
        c.genetics.value[8] = 25_000;
    }
    pair
}

#[test]
fn a_conception_sets_the_father_and_the_due_1st_nine_months_on_and_survives_a_save() {
    let mut store = store_of(sure_couple());
    conceive(&mut store, date(768, 1, 1)).expect("conceptions");
    let wife = get(&store, 1);
    assert_eq!(wife.life.pregnant_by, Some(2));
    assert_eq!(wife.life.due, Some(date(768, 10, 1)));
    assert_eq!(get(&store, 2).life.pregnant_by, None, "men do not conceive");
    // A pregnant woman does not conceive again.
    conceive(&mut store, date(768, 2, 1)).expect("conceptions");
    assert_eq!(get(&store, 1).life.due, Some(date(768, 10, 1)));
    // The pregnancy is canonical, saved state.
    let bytes = store.to_bytes().expect("encode");
    let loaded = CharacterStore::from_bytes(&bytes).expect("decode");
    assert_eq!(loaded, store);
    assert_eq!(get(&loaded, 1).life.pregnant_by, Some(2));
    assert_eq!(get(&loaded, 1).life.due, Some(date(768, 10, 1)));
}

#[test]
fn the_first_success_wins_among_husbands_in_identifier_order() {
    // Three husbands listed out of order: 7 is barren, 5 and 3 are sure; 3 comes first by identifier.
    let mut wife = person(1, date(743, 6, 1), "a", Sex::Female);
    wife.genetics.value[8] = 25_000;
    wife.spouses = vec![7, 5, 3];
    let husband = |id: u64, fecundity: i64| {
        let mut h = person(id, date(738, 6, 1), "a", Sex::Male);
        h.genetics.value[8] = fecundity;
        h.spouses = vec![1];
        h
    };
    let mut store = store_of(vec![
        wife.clone(),
        husband(7, 0),
        husband(5, 25_000),
        husband(3, 25_000),
    ]);
    conceive(&mut store, date(768, 1, 1)).expect("conceptions");
    assert_eq!(get(&store, 1).life.pregnant_by, Some(3));
    // Without 3 and 5 the barren husband cannot succeed.
    let mut store = store_of(vec![wife, husband(7, 0)]);
    conceive(&mut store, date(768, 1, 1)).expect("conceptions");
    assert_eq!(get(&store, 1).life.pregnant_by, None);
}

#[test]
fn a_woman_may_conceive_again_on_the_1st_of_her_nursing_deadline() {
    let mut pair = sure_couple();
    pair[0].life.nursing_until = Some(date(769, 1, 1));
    let mut store = store_of(pair);
    conceive(&mut store, date(768, 12, 1)).expect("conceptions");
    assert_eq!(get(&store, 1).life.pregnant_by, None, "still nursing");
    conceive(&mut store, date(769, 1, 1)).expect("conceptions");
    assert_eq!(
        get(&store, 1).life.pregnant_by,
        Some(2),
        "on the deadline's own 1st"
    );
}

#[test]
fn only_live_husbands_count_and_only_men() {
    let mut pair = sure_couple();
    pair[0].spouses = vec![2, 9];
    let mut store = store_of(pair);
    conceive(&mut store, date(768, 1, 1)).expect("conceptions");
    assert_eq!(get(&store, 1).life.pregnant_by, Some(2), "9 is not live");
    let mut widow = sure_couple();
    widow[0].spouses = vec![9];
    let mut store = store_of(vec![widow[0].clone()]);
    conceive(&mut store, date(768, 1, 1)).expect("conceptions");
    assert_eq!(get(&store, 1).life.pregnant_by, None);
}

// --- Delivery ---------------------------------------------------------------------------------------------------------------

/// A pregnant woman 1 (in pop `place`) and her husband 2, due on 768-10-1.
fn expecting(place: &str) -> Vec<Character> {
    let mut pair = couple(date(743, 6, 1), date(738, 6, 1));
    pair[0].pop = locator(place);
    pair[0].life.pregnant_by = Some(2);
    pair[0].life.due = Some(date(768, 10, 1));
    pair
}
fn deliver(
    store: &mut CharacterStore,
    lifecycle: &LifeCycle,
    pops: &Pops,
    tag: &str,
) -> Result<(), EventError> {
    let mut env = Env::new(tag);
    store.run_deliveries(
        &mut world(quiet(), lifecycle, &mut env, pops),
        date(768, 10, 1),
    )
}

#[test]
fn a_delivery_makes_the_child_into_the_mothers_pop_with_a_drawn_name_and_clears_the_pregnancy() {
    let lifecycle = &rules().lifecycle;
    let mut store = store_of(expecting("a"));
    let pops = Pops::new();
    deliver(&mut store, lifecycle, &pops, "delivery").expect("delivery");
    let child = get(&store, 3);
    assert_eq!((child.father, child.mother), (2, 1));
    assert_eq!(child.pop, locator("a"));
    assert_eq!(child.birth, date(768, 10, 1));
    assert_eq!(child.life.pool, "irish");
    let pool = &lifecycle.pools["irish"];
    match child.sex {
        Sex::Male => assert!(pool.men.iter().any(|(name, _)| *name == child.name)),
        Sex::Female => assert!(pool.women.contains(&child.name)),
    }
    assert_eq!(get(&store, 1).children, [3]);
    assert_eq!(get(&store, 2).children, [3]);
    let mother = get(&store, 1);
    assert_eq!((mother.life.pregnant_by, mother.life.due), (None, None));
    assert_eq!(mother.life.nursing_until, Some(date(769, 10, 1)));
    // One dependent added to the mother's pop, counted as one pending birth.
    assert_eq!(pops.calls(), ["births a 1"]);
    // The same world again gives the same child.
    let mut again = store_of(expecting("a"));
    deliver(&mut again, lifecycle, &Pops::new(), "delivery-again").expect("delivery");
    assert_eq!(again, store);
}

#[test]
fn a_delivery_with_an_archived_father_and_after_the_mother_moved_pop() {
    let mut risk_free = rules().lifecycle.clone();
    risk_free.death.childbirth_ppb = 0;
    let lifecycle = &risk_free;
    // The father died and is archived; the mother lives in pop `m` now.
    let mut pair = expecting("m");
    let mut father = pair.remove(1);
    father.death = Some(date(768, 5, 5));
    pair[0].spouses.clear();
    // The archived father's identifier is spent: the next one is 3.
    let mut store = store_with_next(pair, 3);
    let mut env = Env::new("archived-father");
    env.archive.append(&father).expect("archive the father");
    let pops = Pops::new();
    store
        .run_deliveries(
            &mut world(quiet(), lifecycle, &mut env, &pops),
            date(768, 10, 1),
        )
        .expect("delivery");
    let child = get(&store, 3);
    assert_eq!((child.father, child.mother), (2, 1));
    assert_eq!(child.pop, locator("m"), "the mother's current pop");
    assert_eq!(pops.calls(), ["births m 1"]);
    // An archived father is linked from the child only.
    assert_eq!(get(&store, 1).children, [3]);
}

/// Sixty mothers `100 + 2k` with a husband `101 + 2k` each (the mothers shown as `bearer` when asked to be), all due on 768-10-1.
fn many_mothers(count: u64, bearer: bool) -> CharacterStore {
    let mut all = Vec::new();
    for k in 0..count {
        let mut mother = person(100 + 2 * k, date(743, 6, 1), "a", Sex::Female);
        mother.life.pregnant_by = Some(101 + 2 * k);
        mother.life.due = Some(date(768, 10, 1));
        if bearer {
            mother.genetics.mendelian.insert(
                "twin_bearer".to_owned(),
                Mendel {
                    copies: 2,
                    shows: true,
                },
            );
        }
        all.push(mother);
        all.push(person(101 + 2 * k, date(738, 6, 1), "a", Sex::Male));
    }
    store_of(all)
}

#[test]
fn the_number_of_children_is_one_roll_with_twin_bearer_tripling_the_chances() {
    // Chances large enough for a small sample: triplets 0.1 and twins 0.3, tripled for a bearer (0.3 and 0.9, capped so t + w <= 1).
    let mut lifecycle = rules().lifecycle.clone();
    lifecycle.conception.twins_ppb = 300_000_000;
    lifecycle.conception.triplets_ppb = 100_000_000;
    lifecycle.death.childbirth_ppb = 0;
    let count = |bearer: bool| {
        let mut store = many_mothers(240, bearer);
        let pops = Pops::new();
        deliver(&mut store, &lifecycle, &pops, &format!("multiple-{bearer}")).expect("deliveries");
        let mut sizes = [0_u32; 4];
        for k in 0..240 {
            sizes[get(&store, 100 + 2 * k).children.len()] += 1;
        }
        sizes
    };
    let plain = count(false);
    assert_eq!(plain.iter().sum::<u32>(), 240);
    // 0.6 singles, 0.3 twins, 0.1 triplets of 240: 144, 72, 24.
    assert!((120..=170).contains(&plain[1]), "singles {plain:?}");
    assert!((50..=95).contains(&plain[2]), "twins {plain:?}");
    assert!((10..=40).contains(&plain[3]), "triplets {plain:?}");
    let bearers = count(true);
    // t = 0.3, w = min(0.9, 0.7): no singles at all, 0.7 twins and 0.3 triplets: 168 and 72.
    assert_eq!(bearers[1], 0, "{bearers:?}");
    assert!((140..=195).contains(&bearers[2]), "{bearers:?}");
    assert!((45..=100).contains(&bearers[3]), "{bearers:?}");
}

#[test]
fn the_roll_decides_the_count_at_its_thresholds() {
    // The draw of mother 1 on purpose `delivery`: triplets when it is below t, twins when below t + w, one child otherwise.
    let lifecycle = &rules().lifecycle;
    let draw = Stream::new(7, "character:1:delivery".to_owned(), date(768, 10, 1))
        .below(1_000_000_000)
        .expect("draw");
    let children = |triplets: u32, twins: u32| {
        let mut lifecycle = lifecycle.clone();
        lifecycle.conception.triplets_ppb = triplets;
        lifecycle.conception.twins_ppb = twins;
        lifecycle.death.childbirth_ppb = 0;
        let mut store = store_of(expecting("a"));
        deliver(&mut store, &lifecycle, &Pops::new(), "threshold").expect("delivery");
        get(&store, 1).children.len()
    };
    let draw = u32::try_from(draw).expect("a draw below 1e9");
    assert_eq!(children(draw + 1, 0), 3, "u < t");
    assert_eq!(children(draw, 0), 1, "u = t is not below it");
    assert_eq!(children(0, draw + 1), 2, "u < t + w");
    assert_eq!(children(0, draw), 1);
    assert_eq!(
        children(draw / 2 + 1, draw - draw / 2),
        2,
        "u >= t but u < t + w"
    );
}

#[test]
fn the_childbirth_risk_is_rolled_once_per_delivery_and_is_greater_under_16_and_over_40() {
    let mut lifecycle = rules().lifecycle.clone();
    // Twins always: one risk for the delivery, not one per child.
    lifecycle.conception.twins_ppb = 1_000_000_000;
    lifecycle.conception.triplets_ppb = 0;
    lifecycle.death.childbirth_ppb = 1_000_000_000;
    let mut store = store_of(expecting("a"));
    let pops = Pops::new();
    let mut env = Env::new("risk-once");
    store
        .run_deliveries(
            &mut world(quiet(), &lifecycle, &mut env, &pops),
            date(768, 10, 1),
        )
        .expect("delivery");
    assert!(store.get(1).is_none(), "the mother died after the delivery");
    assert!(
        store.get(3).is_some() && store.get(4).is_some(),
        "both children stand"
    );
    assert_eq!(env.lines(1), ["c1 died in childbirth."], "once");
    assert_eq!(
        env.archive
            .get(1)
            .unwrap()
            .unwrap()
            .life
            .death_cause
            .as_deref(),
        Some("childbirth")
    );
    assert_eq!(pops.calls(), ["births a 2", "death a adult"]);
    // x1.5 at the extremes: a draw that is above the ordinary chance and below the raised one kills only the young mother.
    let draw = Stream::new(7, "character:1:childbirth".to_owned(), date(768, 10, 1))
        .below(1_000_000_000)
        .expect("draw");
    let base = u32::try_from(2 * draw / 3 + 2).expect("a chance");
    assert!(base <= u32::try_from(draw).unwrap() && 3 * u64::from(base) / 2 > draw);
    let dies = |born: GameTime| {
        let mut lifecycle = rules().lifecycle.clone();
        lifecycle.conception.twins_ppb = 0;
        lifecycle.conception.triplets_ppb = 0;
        lifecycle.death.childbirth_ppb = base;
        let mut pair = expecting("a");
        pair[0].birth = born;
        let mut store = store_of(pair);
        deliver(&mut store, &lifecycle, &Pops::new(), "risk-age").expect("delivery");
        store.get(1).is_none()
    };
    assert!(!dies(date(743, 6, 1)), "25: the ordinary chance");
    assert!(dies(date(753, 6, 1)), "15: x1.5");
    assert!(dies(date(726, 6, 1)), "42: x1.5");
    assert!(!dies(date(728, 6, 1)), "40: the ordinary chance");
}

#[test]
fn a_pop_at_its_character_floor_still_delivers() {
    // The pop has exactly as many heads as characters: the delivery adds the child's head first.
    let lifecycle = &rules().lifecycle;
    let mut store = store_of(expecting("a"));
    let pops = Pops {
        heads: 2,
        ..Pops::new()
    };
    deliver(&mut store, lifecycle, &pops, "floor").expect("delivery");
    assert!(store.get(3).is_some());
    assert_eq!(pops.calls(), ["births a 1"]);
}

#[test]
fn a_delivery_that_is_refused_leaves_the_pregnancy_for_the_next_1st() {
    let lifecycle = &rules().lifecycle;
    let mut store = store_of(expecting("a"));
    let before = store.to_bytes().expect("encode");
    let refusing = Pops {
        refuse_births: true,
        ..Pops::new()
    };
    let mut env = Env::new("refused");
    let error = store
        .run_deliveries(
            &mut world(quiet(), lifecycle, &mut env, &refusing),
            date(768, 10, 1),
        )
        .unwrap_err();
    assert!(matches!(error, EventError::Rules(_)), "{error:?}");
    assert_eq!(store.to_bytes().expect("encode"), before);
    assert!(env.lines(1).is_empty() && env.lines(2).is_empty());
    // The next 1st: it delivers.
    let pops = Pops::new();
    store
        .run_deliveries(
            &mut world(quiet(), lifecycle, &mut env, &pops),
            date(768, 11, 1),
        )
        .expect("the next 1st");
    assert!(store.get(3).is_some());
    // A pop without room for the heads is refused too.
    let mut crowded = store_of(expecting("a"));
    let full = Pops {
        heads: 1,
        ..Pops::new()
    };
    let before = crowded.to_bytes().expect("encode");
    assert!(deliver(&mut crowded, lifecycle, &full, "crowded").is_err());
    assert_eq!(crowded.to_bytes().expect("encode"), before);
}

#[test]
fn the_refusal_of_the_second_twin_leaves_no_prefix_visible() {
    let mut lifecycle = rules().lifecycle.clone();
    lifecycle.conception.twins_ppb = 1_000_000_000;
    lifecycle.conception.triplets_ppb = 0;
    // Identifiers run out at the second child: the first would be u64::MAX - 1, the second u64::MAX, whose successor does not exist.
    let mut store = store_with_next(expecting("a"), u64::MAX - 1);
    let before = store.to_bytes().expect("encode");
    let pops = Pops::new();
    let mut env = Env::new("second-twin");
    let error = store
        .run_deliveries(
            &mut world(quiet(), &lifecycle, &mut env, &pops),
            date(768, 10, 1),
        )
        .unwrap_err();
    assert!(
        matches!(error, EventError::Rules(ck_characters::Error::Overflow)),
        "{error:?}"
    );
    assert_eq!(
        store.to_bytes().expect("encode"),
        before,
        "no id, child or link"
    );
    assert!(pops.calls().is_empty(), "no head");
    assert!(
        env.lines(1).is_empty() && env.lines(2).is_empty(),
        "no line"
    );
    let mother = get(&store, 1);
    assert_eq!(mother.life.pregnant_by, Some(2), "the pregnancy as it was");
    assert_eq!(
        mother.life.due,
        Some(date(768, 10, 1)),
        "due again on the next 1st"
    );
    assert_eq!(mother.life.nursing_until, None);
}

#[test]
fn a_failing_log_after_a_twin_delivery_leaves_both_children_committed_and_reports_it() {
    let mut lifecycle = rules().lifecycle.clone();
    lifecycle.conception.twins_ppb = 1_000_000_000;
    lifecycle.conception.triplets_ppb = 0;
    lifecycle.death.childbirth_ppb = 1_000_000_000;
    let mut store = store_of(expecting("a"));
    let pops = Pops::new();
    let mut env = Env::new("flaky-log");
    let mut logs = FlakyLog {
        inner: LifeLog::open(path("flaky-log-logs")).expect("logs"),
        armed: true,
    };
    let mut world = World {
        rules: quiet(),
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle: &lifecycle,
        seed: 7,
        populations: &pops,
        archive: &mut env.archive,
        logs: &mut logs,
    };
    let delivery = store
        .deliver(&mut world, 1, date(768, 10, 1))
        .expect("the delivery committed");
    assert_eq!(delivery.children, [3, 4]);
    assert!(
        matches!(delivery.output, Some(EventError::Log(_))),
        "{:?}",
        delivery.output
    );
    assert!(store.get(3).is_some() && store.get(4).is_some());
    assert!(store.get(1).is_none(), "the mother's death stands too");
    assert_eq!(pops.calls(), ["births a 2", "death a adult"]);
}

// --- Names ------------------------------------------------------------------------------------------------------------------

fn founder(store: &mut CharacterStore, sex: Sex, place: &str, birth: GameTime) -> u64 {
    let lifecycle = &rules().lifecycle;
    let ctx = Context {
        rules: &rules().genetics,
        personality: &rules().personality,
        events: quiet(),
        lifecycle,
        seed: 11,
        tick: birth,
    };
    store
        .create_founder(
            &ctx,
            NewCharacter {
                name: String::new(),
                sex,
                birth,
                pop: locator(place),
                house: String::new(),
            },
            &irish(),
            &Pops::new(),
        )
        .expect("founder")
}

#[test]
fn a_founder_is_named_from_the_pool_of_its_culture_by_sex_and_records_the_pool() {
    let lifecycle = &rules().lifecycle;
    let mut store = CharacterStore::default();
    for (place, pool) in [("a", "irish"), ("e", "english"), ("p", "pictish")] {
        for sex in [Sex::Male, Sex::Female] {
            for _ in 0..12 {
                let id = founder(&mut store, sex, place, date(740, 1, 1));
                let c = get(&store, id);
                assert_eq!(c.life.pool, pool);
                let names = &lifecycle.pools[pool];
                match sex {
                    Sex::Male => assert!(
                        names.men.iter().any(|(name, _)| *name == c.name),
                        "{}",
                        c.name
                    ),
                    Sex::Female => assert!(names.women.contains(&c.name), "{}", c.name),
                }
            }
        }
    }
    // Names differ between characters, and the same draw gives the same name.
    let names: std::collections::BTreeSet<_> = store.iter().map(|c| c.name.clone()).collect();
    assert!(names.len() > 12, "{names:?}");
    let mut again = CharacterStore::default();
    for place in ["a", "e", "p"] {
        for sex in [Sex::Male, Sex::Female] {
            for _ in 0..12 {
                founder(&mut again, sex, place, date(740, 1, 1));
            }
        }
    }
    assert_eq!(again, store);
}

#[test]
fn a_name_the_caller_gives_is_kept_and_a_culture_without_a_pool_cannot_be_drawn() {
    let ctx = |lifecycle: &'static LifeCycle| Context {
        rules: &rules().genetics,
        personality: &rules().personality,
        events: quiet(),
        lifecycle,
        seed: 1,
        tick: date(740, 1, 1),
    };
    let mut store = CharacterStore::default();
    let new = |name: &str| NewCharacter {
        name: name.to_owned(),
        sex: Sex::Male,
        birth: date(740, 1, 1),
        pop: locator("a"),
        house: String::new(),
    };
    let id = store
        .create_founder(
            &ctx(&rules().lifecycle),
            new("Named"),
            &irish(),
            &Pops::new(),
        )
        .expect("founder");
    assert_eq!(get(&store, id).name, "Named");
    assert_eq!(get(&store, id).life.pool, "irish");
    let empty: &'static LifeCycle = Box::leak(Box::default());
    let error = store
        .create_founder(&ctx(empty), new(""), &irish(), &Pops::new())
        .unwrap_err();
    assert_eq!(error, ck_characters::Error::Culture);
}

fn named(id: u64, sex: Sex, name: &str, pool: &str) -> Character {
    let mut c = person(id, date(740, 6, 1), "a", sex);
    name.clone_into(&mut c.name);
    pool.clone_into(&mut c.life.pool);
    c
}

#[test]
fn the_shown_name_adds_the_patronym_and_the_fathers_genitive_from_the_recorded_pools() {
    let lifecycle = &rules().lifecycle;
    let father = named(2, Sex::Male, "Domnall", "irish");
    let mut son = named(3, Sex::Male, "Áed", "irish");
    son.father = 2;
    let mut daughter = named(4, Sex::Female, "Brigit", "irish");
    daughter.father = 2;
    assert_eq!(
        shown_name(&son, Some(&father), lifecycle),
        "Áed mac Domnaill"
    );
    assert_eq!(
        shown_name(&daughter, Some(&father), lifecycle),
        "Brigit ingen Domnaill"
    );
    // English: no patronymic.
    let english = named(5, Sex::Male, "Offa", "english");
    let english_father = named(6, Sex::Male, "Cuthbert", "english");
    assert_eq!(
        shown_name(&english, Some(&english_father), lifecycle),
        "Offa"
    );
    // An unknown father, a father whose name is not in his pool, and a child with no pool: none.
    assert_eq!(shown_name(&son, None, lifecycle), "Áed");
    let stranger = named(7, Sex::Male, "Nobody", "irish");
    assert_eq!(shown_name(&son, Some(&stranger), lifecycle), "Áed");
    let poolless = named(8, Sex::Male, "Áed", "");
    assert_eq!(shown_name(&poolless, Some(&father), lifecycle), "Áed");
}

#[test]
fn the_shown_name_is_derived_from_stored_state_and_survives_a_save_a_move_and_the_fathers_death() {
    let lifecycle = &rules().lifecycle;
    // A Pictish child (map / merch) of an Irish father: the genitive comes from the father's recorded pool.
    let father = named(2, Sex::Male, "Domnall", "irish");
    let mut child = named(3, Sex::Male, "Bridei", "pictish");
    child.father = 2;
    let mut store = store_of(vec![father, child]);
    let shown = |store: &CharacterStore, archive: &Archive| {
        let c = store.lookup(3, archive).expect("read").expect("child");
        store
            .shown_name_of(&c, lifecycle, archive)
            .expect("shown name")
    };
    let mut env = Env::new("shown");
    let before = shown(&store, &env.archive);
    assert_eq!(before, "Bridei map Domnaill");
    // The father moves to an English pop and dies: the recorded pools decide, never the current pop or culture.
    let pops = Pops::new();
    store
        .move_to_with_events(
            &mut world(quiet(), lifecycle, &mut env, &pops),
            2,
            locator("e"),
            date(768, 1, 1),
        )
        .expect("move");
    store
        .die_with_events(
            &mut world(quiet(), lifecycle, &mut env, &pops),
            2,
            date(768, 2, 1),
            "fever",
        )
        .expect("death");
    assert_eq!(shown(&store, &env.archive), before);
    // And after a save and a load of the store and a reopening of the archive.
    let loaded = CharacterStore::from_bytes(&store.to_bytes().expect("encode")).expect("decode");
    let reopened = Archive::open(existing("shown-arch")).expect("reopen");
    assert_eq!(shown(&loaded, &reopened), before);
}

// --- The records, the archive and determinism ------------------------------------------------------------------------------

#[test]
fn an_archived_record_keeps_the_pool_the_pregnancy_and_the_cause_across_a_reopening() {
    let lifecycle = &rules().lifecycle;
    let mut pair = expecting("a");
    pair[0].life.nursing_until = Some(date(769, 1, 1));
    pair[0].birth = date(690, 1, 1);
    let mut store = store_of(pair);
    let pops = Pops::new();
    let mut env = Env::new("readback");
    store
        .run_certain_deaths(
            &mut world(quiet(), lifecycle, &mut env, &pops),
            date(800, 1, 1),
        )
        .expect("deaths");
    let record = env.archive.get(1).unwrap().unwrap();
    assert_eq!(record.life.pool, "irish");
    assert_eq!(record.life.pregnant_by, Some(2));
    assert_eq!(record.life.due, Some(date(768, 10, 1)));
    assert_eq!(record.life.nursing_until, Some(date(769, 1, 1)));
    assert_eq!(record.life.death_cause.as_deref(), Some("old_age"));
    let reopened = Archive::open(existing("readback-arch")).expect("reopen");
    assert_eq!(reopened.get(1).unwrap().unwrap(), record);
}

#[test]
fn the_same_world_gives_the_same_deaths_births_and_conceptions() {
    let lifecycle = &rules().lifecycle;
    let run = |tag: &str| {
        let mut all = Vec::new();
        for k in 0..30_u64 {
            let mut wife = person(10 + 2 * k, date(740, 6, 1), "a", Sex::Female);
            wife.genetics.value[8] = 14_000 + 100 * i64::try_from(k).unwrap();
            let mut husband = person(11 + 2 * k, date(735, 6, 1), "a", Sex::Male);
            wife.spouses = vec![husband.id];
            husband.spouses = vec![wife.id];
            all.push(wife);
            all.push(husband);
        }
        let mut store = store_of(all);
        let pops = Pops::new();
        let mut env = Env::new(tag);
        for month in 1..=12_u8 {
            let today = date(768, month, 1);
            let mut world = world(quiet(), lifecycle, &mut env, &pops);
            store
                .run_pre_economy(&mut world, today, &Pressure::new())
                .expect("phase A");
            store
                .run_conceptions(&mut world, today)
                .expect("conceptions");
        }
        (
            store.to_bytes().expect("encode"),
            pops.calls(),
            std::fs::read(path(&format!("{tag}-logs"))).unwrap_or_default(),
        )
    };
    let (first, calls, _) = run("determinism-a");
    let (second, again, _) = run("determinism-b");
    assert_eq!(first, second);
    assert_eq!(calls, again);
    assert!(
        calls.iter().any(|call| call.starts_with("births")),
        "the year had births: {calls:?}"
    );
}

#[test]
fn the_death_lines_are_the_designers_wording() {
    for (age, line) in [
        (0, "{name} died in infancy."),
        (1, "{name} died in infancy."),
        (2, "{name} died young."),
        (14, "{name} died young."),
        (15, "{name} died."),
        (59, "{name} died."),
        (60, "{name} died in old age."),
        (110, "{name} died in old age."),
    ] {
        assert_eq!(cycle::death_line(age), line, "{age}");
    }
    assert_eq!(cycle::DIED_IN_CHILDBIRTH, "{name} died in childbirth.");
}

#[test]
fn a_pop_without_a_ledger_changes_no_heads() {
    // The unit fixtures that exercise only the shell have no ledger: a bare head-count function accepts and ignores both changes.
    let heads = |_: &PopLocator| Some(5_u64);
    assert!(heads.add_births(&locator("a"), 1).is_ok());
    assert!(heads.remove_head(&locator("a"), true).is_ok());
    assert!(heads.genetics(&locator("a")).is_none());
    assert_eq!(UNKNOWN, 0);
}

// --- The review's findings ---------------------------------------------------------------------------------------------------

#[test]
fn the_live_chance_reads_the_shown_weight_against_the_base_weight() {
    // CK-367 §3a: the shown weight (the stored one) against the base weight, the genetic roll and a woman's offset together. A woman
    // at her base weight is not an extreme, whatever the offset; one who has fallen below 80% of it, or a man risen above 125%, is.
    let lifecycle = &rules().lifecycle;
    let today = date(800, 1, 1);
    let chance = |c: &Character| {
        CharacterStore::annual_death_ppb(c, &rules().genetics, lifecycle, &Pressure::new(), today)
            .expect("chance")
    };
    let mut woman = person(1, date(759, 6, 1), "a", Sex::Female);
    let base = woman.base_weight(&rules().genetics);
    assert!(base < 70_000, "the woman's offset lowers her base: {base}");
    assert_eq!(chance(&woman), 9_828_427, "at her base weight");
    woman.life.weight = base * 79 / 100;
    assert_eq!(chance(&woman), 11_794_112, "x1.2 of 9,828,427 below 80%");
    woman.life.weight = base * 80 / 100;
    assert_eq!(chance(&woman), 9_828_427, "exactly 80% is not an extreme");
    let mut man = person(2, date(759, 6, 1), "a", Sex::Male);
    assert_eq!(chance(&man), 9_828_427);
    man.life.weight = 70_000 * 126 / 100;
    assert_eq!(chance(&man), 11_794_112, "above 125%");
}

#[test]
fn a_refused_head_change_leaves_the_character_whole_on_every_death_path() {
    let lifecycle = &rules().lifecycle;
    let refusing = Pops {
        refuse_heads: true,
        ..Pops::new()
    };
    let fatal = fatal_events();
    for (tag, path) in [
        ("certain", 0),
        ("monthly", 1),
        ("fatal", 2),
        ("explicit", 3),
    ] {
        let mut store = store_of(vec![person(1, date(690, 1, 1), "a", Sex::Male)]);
        let before = store.to_bytes().expect("encode");
        let mut env = Env::new(&format!("refused-head-{tag}"));
        let events = if path == 2 { &fatal } else { quiet() };
        let mut world = world(events, lifecycle, &mut env, &refusing);
        let today = date(800, 1, 1);
        let result = match path {
            0 => store.run_certain_deaths(&mut world, today),
            1 => {
                // The monthly roll for a character under the terminal age, certain to succeed.
                let mut young = store_of(vec![person(1, date(759, 6, 1), "a", Sex::Male)]);
                let pressure = Pressure::from([(locator("a"), 1_000_000_000_000_000)]);
                let result = young.run_monthly_deaths(&mut world, today, &pressure);
                assert!(young.get(1).is_some(), "{tag}");
                result
            }
            2 => store.report_happening(&mut world, 1, "test", today),
            _ => store.die_with_events(&mut world, 1, today, "fever"),
        };
        let error = result.unwrap_err();
        assert!(matches!(error, EventError::Rules(_)), "{tag}: {error:?}");
        if path != 1 {
            assert_eq!(store.to_bytes().expect("encode"), before, "{tag}");
            assert!(store.get(1).is_some(), "{tag}");
        }
        assert!(env.archive.get(1).unwrap().is_none(), "{tag}: not archived");
        assert!(env.lines(1).is_empty(), "{tag}: no line");
    }
}

/// A world with one event: a fatal one on the happening `test`.
fn fatal_events() -> EventRules {
    let pop_types: Vec<&str> = rules().pop_types.iter().map(|p| p.id.as_str()).collect();
    EventRules::parse(
        "character-events.toml",
        r#"
[[event]]
id = "fatal"
trigger = "happening:test"
chance = 1
effects = [{ die = "fever" }]
text = "{name} died of a fever."
"#,
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
    .expect("a world with one fatal event")
}

/// An archive whose first append fails and whose later ones succeed, counting the attempts.
struct FailOnce {
    inner: Archive,
    attempts: u32,
}
impl ArchiveSink for FailOnce {
    fn append(&mut self, record: &Character) -> io::Result<()> {
        self.attempts += 1;
        if self.attempts == 1 {
            Err(io::Error::other("injected archive failure"))
        } else {
            self.inner.append(record)
        }
    }
    fn get(&self, id: u64) -> io::Result<Option<Character>> {
        self.inner.get(id)
    }
    fn children_of(&self, parent: u64) -> io::Result<Vec<u64>> {
        self.inner.children_of(parent)
    }
}

#[test]
fn a_certain_death_refused_on_the_1st_is_not_rolled_again_in_the_monthly_pass() {
    let lifecycle = &rules().lifecycle;
    // 105 on 805-3-1, a 1st: the certain path's one attempt fails; the monthly pass must not make a second.
    let mut store = store_of(vec![person(1, date(700, 3, 1), "a", Sex::Male)]);
    let pops = Pops::new();
    let mut logs = LifeLog::open(path("fail-once-logs")).expect("logs");
    let mut archive = FailOnce {
        inner: Archive::open(path("fail-once-arch")).expect("archive"),
        attempts: 0,
    };
    let mut world = World {
        rules: quiet(),
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle,
        seed: 7,
        populations: &pops,
        archive: &mut archive,
        logs: &mut logs,
    };
    let error = store
        .run_pre_economy(&mut world, date(805, 3, 1), &Pressure::new())
        .unwrap_err();
    assert!(matches!(error, EventError::Archive(_)), "{error:?}");
    assert!(store.get(1).is_some(), "refused, so alive");
    assert_eq!(archive.attempts, 1, "one attempt, not two");
    // The next pre-economy phase tries again and the death commits.
    let mut world = World {
        rules: quiet(),
        personality: &rules().personality,
        genetics: &rules().genetics,
        lifecycle,
        seed: 7,
        populations: &pops,
        archive: &mut archive,
        logs: &mut logs,
    };
    store
        .run_pre_economy(&mut world, date(805, 3, 2), &Pressure::new())
        .expect("the next day");
    assert!(store.get(1).is_none());
    assert_eq!(archive.attempts, 2);
}
