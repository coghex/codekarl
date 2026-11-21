//! Slots, generated marriage candidates and real households (CK-369 §4, §6, §7).
//!
//! The people are built as records and loaded through the canonical encoding, so that a case controls exactly who exists; what the generator
//! makes is then read back. Every refusal is checked to leave the store, the archive and the pops as they were.

mod support;

use ck_characters::{
    CharacterStore, Error, EventError, Office, Populations, Sex, UNKNOWN,
    generator::{Birth, GenerateRequest, SlotRequest},
};
use ck_economy::LifeCycle;
use ck_sim::GameTime;
use support::*;

const PPB: u64 = 1_000_000_000;

fn today() -> GameTime {
    date(768, 10, 9)
}
fn pops() -> Pops {
    Pops::new()
        .pop("tara", 0, "noble", "ulaid", 1_000_000)
        .pop("tara", 1, "peasant", "ulaid", 1_000_000)
        .pop("emain", 0, "noble", "ulaid", 1_000_000)
        .pop("far", 0, "noble", "ulaid", 1_000_000)
        .pop("york", 0, "noble", "northumbrian", 1_000_000)
        .apart("tara", "emain", 30_000)
        .apart("tara", "far", 200_000)
        .apart("tara", "york", 40_000)
        .apart("emain", "york", 60_000)
        .apart("emain", "far", 170_000)
}
/// A person of `age` whole years on 9 October 768 (born 3 March).
fn aged(
    id: u64,
    sex: Sex,
    age: u16,
    place: &str,
    slot: u32,
    house: &str,
) -> ck_characters::Character {
    person(id, sex, date(768 - age, 3, 3), locator(place, slot), house)
}
fn ruler(mut c: ck_characters::Character) -> ck_characters::Character {
    c.office = Some(Office::Ruler);
    c
}
fn slot(key: &str, kind: &str, house: &str, count: u32) -> SlotRequest {
    SlotRequest {
        key: key.to_owned(),
        kind: kind.to_owned(),
        court: locator("tara", 0),
        house: house.to_owned(),
        count,
        ages: None,
        today: today(),
    }
}
fn fill(
    store: &mut CharacterStore,
    env: &mut Env,
    pops: &Pops,
    asked: &SlotRequest,
) -> Result<ck_characters::generator::SlotOutcome, EventError> {
    store.fill_slot(&mut world(shipped(), env, pops, 5), asked)
}

#[test]
fn repeated_fills_never_choose_a_character_with_a_slot_twice_and_a_missing_child_is_generated() {
    let pops = pops();
    let mut env = Env::new("repeat");
    let kids: Vec<_> = (1..=5)
        .map(|id| aged(id, Sex::Male, 10, "emain", 0, "b"))
        .collect();
    let mut store = store_of(kids);
    let mut placed = Vec::new();
    let mut generated = 0;
    for (n, count) in [3, 3].into_iter().enumerate() {
        let out = fill(
            &mut store,
            &mut env,
            &pops,
            &slot(&format!("k{n}"), "fosterling", "a", count),
        )
        .expect("filled");
        placed.extend(out.placed.iter().copied());
        generated += out.generated.len();
        assert_eq!(out.unfilled, 0);
    }
    assert_eq!(placed.len(), 6);
    assert_eq!(
        placed
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        6,
        "{placed:?}"
    );
    assert_eq!(generated, 1);
    assert!(
        placed[..5].iter().all(|id| *id <= 5),
        "people who exist come first: {placed:?}"
    );
    assert!(placed[5] > 5);
    for id in &placed {
        let c = store.get(*id).expect("live");
        assert_eq!(
            (c.life.slot_kind.as_str(), c.pop.clone()),
            ("fosterling", locator("tara", 0))
        );
        assert!(c.life.slot_key.starts_with('k'));
    }
    // The generated child has real parents of the house it came from.
    let child = store.get(placed[5]).expect("live");
    assert_eq!(child.life.house_of_birth, "b");
    assert_ne!((child.father, child.mother), (UNKNOWN, UNKNOWN));
    assert!(pops.moves.borrow().len() >= 5);
    assert_whole(&store, &env, &pops);
}

#[test]
fn a_rulers_son_is_fostered_out_to_a_house_without_a_living_ruler_and_never_into_one_with() {
    let pops = pops();
    let mut env = Env::new("ruler");
    let mut son = aged(11, Sex::Male, 10, "emain", 0, "b");
    son.father = 10;
    let mut father = ruler(aged(10, Sex::Male, 48, "emain", 0, "b"));
    father.children = vec![11];
    let court_ruler = ruler(aged(12, Sex::Male, 46, "tara", 0, "a"));
    let base = store_of(vec![father, son, court_ruler]);
    // A court whose house has a living ruler does not take him: a child is generated instead.
    let mut store = base.clone();
    let out = fill(
        &mut store,
        &mut env,
        &pops,
        &slot("ruled", "fosterling", "a", 1),
    )
    .expect("filled");
    assert_eq!(out.generated.len(), 1);
    assert_ne!(out.placed[0], 11);
    assert!(store.get(11).expect("live").life.slot_key.is_empty());
    // A court whose house has none takes him.
    let mut store = base.clone();
    let out = fill(
        &mut store,
        &mut env,
        &pops,
        &slot("free", "fosterling", "c", 1),
    )
    .expect("filled");
    assert_eq!((out.placed.clone(), out.generated.len()), (vec![11], 0));
    assert_eq!(store.get(11).expect("live").pop, locator("tara", 0));
    // And never into his own court: a court of house `b` takes none of its own ruler's children.
    let mut store = base.clone();
    let out = fill(
        &mut store,
        &mut env,
        &pops,
        &slot("own", "fosterling", "b", 1),
    )
    .expect("filled");
    assert_ne!(out.placed[0], 11);
    assert_whole(&store, &env, &pops);
}

#[test]
fn a_court_whose_nearest_house_is_its_own_takes_the_next_permitted_house_and_none_in_reach_gets_a_minor_house()
 {
    let pops = pops();
    let mut env = Env::new("nearest");
    let base = store_of(vec![
        aged(20, Sex::Male, 50, "tara", 0, "own"),
        aged(21, Sex::Male, 50, "emain", 0, "other"),
        aged(22, Sex::Male, 50, "far", 0, "farther"),
    ]);
    let mut store = base.clone();
    let out = fill(
        &mut store,
        &mut env,
        &pops,
        &slot("a", "fosterling", "own", 1),
    )
    .expect("filled");
    let child = store.get(out.placed[0]).expect("live");
    assert_eq!(
        child.life.house_of_birth, "other",
        "not its own house, not one out of reach"
    );
    assert_eq!(child.pop, locator("tara", 0));
    // The family is real: a generated couple of that house (live ones placed in the house's pop, which is where the member is).
    assert_ne!((child.father, child.mother), (UNKNOWN, UNKNOWN));
    assert_eq!(record(&store, &env, child.father).life.house, "other");
    // With only its own house within reach, a generated couple of a new minor house at the court's place has the child.
    let alone = store_of(vec![
        aged(20, Sex::Male, 50, "tara", 0, "own"),
        aged(22, Sex::Male, 50, "far", 0, "farther"),
    ]);
    let mut store = alone.clone();
    let out = fill(
        &mut store,
        &mut env,
        &pops,
        &slot("b", "fosterling", "own", 1),
    )
    .expect("filled");
    let child = store.get(out.placed[0]).expect("live");
    let father = record(&store, &env, child.father);
    assert_eq!(child.life.house_of_birth, format!("{}_tara", father.name));
    assert_eq!(father.life.house, child.life.house_of_birth);
    assert_eq!(child.pop, locator("tara", 0));
    // A hostage with no other ruling house leaves the place unfilled and records a warning, and creates nothing.
    let mut store = alone.clone();
    let out = fill(&mut store, &mut env, &pops, &slot("h", "hostage", "own", 1)).expect("filled");
    assert_eq!(
        (out.placed.len(), out.unfilled, out.warnings.len()),
        (0, 1, 1),
        "{out:?}"
    );
    assert_eq!(store, alone);
    assert_whole(&store, &env, &pops);
}

#[test]
fn a_refused_later_step_leaves_nothing() {
    let pops = pops();
    let mut env = Env::new("refused");
    // A child who exists and must move, and (separately) a child to be generated and then moved.
    let existing = store_of(vec![aged(1, Sex::Male, 10, "emain", 0, "b")]);
    let generated = store_of(vec![aged(1, Sex::Male, 40, "emain", 0, "b")]);
    for base in [existing, generated] {
        let mut store = base.clone();
        pops.refuse_moves.set(true);
        let error = fill(
            &mut store,
            &mut env,
            &pops,
            &slot("x", "fosterling", "a", 1),
        )
        .unwrap_err();
        pops.refuse_moves.set(false);
        assert!(
            matches!(error, EventError::Rules(Error::Lifecycle)),
            "{error:?}"
        );
        assert_eq!(store, base);
        assert!(pops.moves.borrow().is_empty());
        assert!(env.archived_ids().is_empty());
    }
    // The same slot then fills.
    let mut store = store_of(vec![aged(1, Sex::Male, 40, "emain", 0, "b")]);
    assert!(
        fill(
            &mut store,
            &mut env,
            &pops,
            &slot("x", "fosterling", "a", 1)
        )
        .is_ok()
    );
    // A second place that is refused undoes the first: the first child's move was made, then the second's was refused.
    let pops = self::pops();
    let base = store_of(vec![
        aged(1, Sex::Male, 10, "emain", 0, "b"),
        aged(2, Sex::Male, 10, "emain", 0, "b"),
    ]);
    let mut store = base.clone();
    pops.moves_allowed.set(Some(1));
    let error = fill(
        &mut store,
        &mut env,
        &pops,
        &slot("t", "fosterling", "a", 2),
    )
    .unwrap_err();
    pops.moves_allowed.set(None);
    assert!(
        matches!(error, EventError::Rules(Error::Lifecycle)),
        "{error:?}"
    );
    assert_eq!(store, base);
    assert!(pops.moves.borrow().is_empty(), "the first move was undone");
    assert_eq!(pops.heads(&locator("tara", 0)), Some(1_000_000));
    assert_eq!(pops.heads(&locator("emain", 0)), Some(1_000_000));
}

#[test]
fn a_hostage_is_a_son_of_another_ruling_house_from_any_distance_and_gains_the_captors_tongue_once()
{
    let pops = pops();
    let mut env = Env::new("hostage");
    let base = store_of(vec![
        ruler(aged(30, Sex::Male, 40, "york", 0, "wessex")),
        aged(31, Sex::Male, 12, "york", 0, "wessex"),
        aged(32, Sex::Male, 12, "emain", 0, "minor"),
        aged(33, Sex::Female, 12, "york", 0, "wessex"),
        ruler(aged(34, Sex::Male, 44, "tara", 0, "own")),
        aged(35, Sex::Male, 12, "tara", 0, "own"),
    ]);
    let mut store = base.clone();
    let out = fill(&mut store, &mut env, &pops, &slot("h", "hostage", "own", 1)).expect("filled");
    assert_eq!(
        out.placed,
        [31],
        "a son of a ruling house that is not the court's"
    );
    let hostage = store.get(31).expect("live");
    assert_eq!(
        (hostage.life.slot_kind.as_str(), hostage.pop.clone()),
        ("hostage", locator("tara", 0))
    );
    let range = shipped().generator.languages.hostage_raised;
    let irish = hostage.life.languages["old_irish"];
    assert!((range.low..=range.high).contains(&irish), "{irish}");
    assert!(
        !hostage.life.languages.contains_key("old_english"),
        "his own tongue is no second language"
    );
    // Placed once: another slot does not choose him again, and his language stands.
    let out = fill(
        &mut store,
        &mut env,
        &pops,
        &slot("h2", "hostage", "own", 1),
    )
    .expect("filled");
    assert_ne!(out.placed[0], 31);
    assert_eq!(
        store.get(31).expect("live").life.languages["old_irish"],
        irish
    );
    assert_whole(&store, &env, &pops);
}

#[test]
fn a_fosterling_placed_among_another_tongue_gains_it_once() {
    // The rules read `northumbrian` as of the court's subgroup, so that an English child is a candidate for an Irish court.
    let mut events = rules().events.clone();
    events
        .culture_group
        .insert("northumbrian".into(), "gaelic".into());
    let events: &'static ck_economy::EventRules = Box::leak(Box::new(events));
    let pops = pops();
    let mut env = Env::new("tongue");
    let mut store = store_of(vec![aged(1, Sex::Female, 10, "york", 0, "wessex")]);
    let out = store
        .fill_slot(
            &mut world_with(events, shipped(), &mut env, &pops, 5),
            &slot("f", "fosterling", "own", 1),
        )
        .expect("filled");
    assert_eq!(out.placed, [1]);
    let range = shipped().generator.languages.fostered_among;
    let strength = store.get(1).expect("live").life.languages["old_irish"];
    assert!((range.low..=range.high).contains(&strength));
    // Another slot does not choose her again, and nothing changes her languages.
    let out = store
        .fill_slot(
            &mut world_with(events, shipped(), &mut env, &pops, 5),
            &slot("g", "fosterling", "own", 1),
        )
        .expect("filled");
    assert_ne!(out.placed[0], 1);
    assert_eq!(
        store.get(1).expect("live").life.languages["old_irish"],
        strength
    );
    assert_eq!(store.get(1).expect("live").life.languages.len(), 1);
}

#[test]
fn a_student_comes_from_a_noble_or_monk_family_else_from_the_places_peasants() {
    let pops = pops()
        .pop("clonard", 0, "monk", "ulaid", 1_000_000)
        .apart("tara", "clonard", 20_000);
    let mut env = Env::new("student");
    let mut store = store_of(vec![
        aged(1, Sex::Male, 14, "emain", 0, "b"),
        aged(2, Sex::Female, 12, "tara", 1, ""),
        aged(3, Sex::Male, 15, "clonard", 0, ""),
        aged(4, Sex::Male, 25, "emain", 0, "b"),
    ]);
    let out = fill(&mut store, &mut env, &pops, &slot("s", "student", "a", 3)).expect("filled");
    // The two of noble and monk families come first; the peasant child only when no such child is left.
    let mut first_two = out.placed[..2].to_vec();
    first_two.sort_unstable();
    assert_eq!(first_two, [1, 3]);
    assert_eq!(out.placed[2], 2);
    assert!(out.generated.is_empty());
    // A man of 25 is no student.
    assert!(store.get(4).expect("live").life.slot_key.is_empty());
    assert_whole(&store, &env, &pops);
}

#[test]
fn kin_of_the_courts_house_weigh_three_times_the_others() {
    let pops = pops();
    let mut kin = 0;
    for seed in 0..200 {
        let mut env = Env::new("kin");
        let mut store = store_of(vec![
            aged(1, Sex::Male, 10, "emain", 0, "a"),
            aged(2, Sex::Male, 10, "emain", 0, "b"),
        ]);
        let out = store
            .fill_slot(
                &mut world(shipped(), &mut env, &pops, seed),
                &slot("w", "fosterling", "a", 1),
            )
            .expect("filled");
        kin += usize::from(out.placed[0] == 1);
    }
    // 3 in 4, over 200 fills: 150 +- 4 standard deviations (4 x 6).
    assert!((126..=174).contains(&kin), "{kin}");
}

#[test]
fn a_top_up_generates_candidates_of_marriageable_age_in_new_minor_houses_with_distinct_keys() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let mut env = Env::new("candidates");
    // A boy of 14 (the age at which a man is marriageable), and a pool of names with one man's name, so that every father is "Áed".
    let mut data: LifeCycle = shipped().clone();
    data.pools.get_mut("irish").expect("pool").men = vec![("Áed".into(), "Áeda".into())];
    let mut store = store_of(vec![aged(1, Sex::Male, 14, "tara", 0, "uí_néill")]);
    let made = store
        .top_up_pool(&mut world(&data, &mut env, &pops, 3), 1, today())
        .expect("generated")
        .made;
    assert_eq!(made.len(), data.marriage.pool.min_candidates as usize);
    let mut houses = std::collections::BTreeSet::new();
    for id in &made {
        let c = store.get(*id).expect("live").clone();
        assert_eq!(c.sex, Sex::Female);
        // Marriageable (12 for a woman), within the seeker's free gap and the fertile ends: 15 to 24.
        assert!(
            CharacterStore::marriageable(&world(&data, &mut env, &pops, 3), &c, today())
                .expect("age")
        );
        assert!(
            (15..=24).contains(&age_on(&c, today())),
            "{}",
            age_on(&c, today())
        );
        assert!(store.unmarried(&c));
        assert_ne!((c.father, c.mother), (UNKNOWN, UNKNOWN));
        let father = record(&store, &env, c.father);
        assert_eq!(father.name, "Áed");
        assert_eq!(c.life.house, father.life.house);
        assert!(c.life.house.starts_with("Áed_tara"), "{}", c.life.house);
        houses.insert(c.life.house.clone());
    }
    assert_eq!(
        houses.len(),
        made.len(),
        "two same-named fathers at one place get distinct keys: {houses:?}"
    );
    assert!(
        houses.contains("Áed_tara")
            && houses.contains("Áed_tara_2")
            && houses.contains("Áed_tara_3")
    );
    // They are in the pool, and the pool is full, so a second top-up makes none.
    let w = world(&data, &mut env, &pops, 3);
    assert_eq!(
        store.marriage_pool(&w, 1, today()).expect("pool").len(),
        made.len()
    );
    let again = store
        .top_up_pool(&mut world(&data, &mut env, &pops, 3), 1, today())
        .expect("generated")
        .made;
    assert!(again.is_empty());
    assert_whole(&store, &env, &pops);
}

#[test]
fn a_commoner_seeker_gets_commoners_without_a_house_and_a_pop_with_no_head_to_spare_yields_none() {
    let pops = Pops::new().pop("farm", 0, "peasant", "ulaid", 1_000_000);
    let mut env = Env::new("commoner");
    let mut store = store_of(vec![aged(1, Sex::Female, 30, "farm", 0, "")]);
    let made = store
        .top_up_pool(&mut world(shipped(), &mut env, &pops, 4), 1, today())
        .expect("generated")
        .made;
    assert_eq!(made.len(), 3);
    for id in &made {
        let c = store.get(*id).expect("live");
        assert_eq!(
            (c.sex, c.pop.clone(), c.life.house.as_str()),
            (Sex::Male, locator("farm", 0), "")
        );
        // A man's age within ten years of hers, and within the fertile curve of men.
        assert!((20..=40).contains(&age_on(c, today())));
    }
    // One head and one character: nothing can be made, nothing is changed.
    let tight = Pops::new().pop("farm", 0, "peasant", "ulaid", 1);
    let mut store = store_of(vec![aged(1, Sex::Female, 30, "farm", 0, "")]);
    let before = store.clone();
    let made = store
        .top_up_pool(&mut world(shipped(), &mut env, &tight, 4), 1, today())
        .expect("no error")
        .made;
    assert!(made.is_empty());
    assert_eq!(store, before);
}

#[test]
fn a_yearly_marriage_wish_with_a_short_pool_generates_the_candidates() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let mut env = Env::new("wired");
    let mut data: LifeCycle = shipped().clone();
    data.marriage.wish.men = vec![(12, PPB)];
    data.marriage.wish.women = vec![(12, PPB)];
    let mut seeker = aged(1, Sex::Male, 20, "tara", 0, "uí_néill");
    seeker.birth = date(748, 3, 3);
    let mut store = store_of(vec![seeker]);
    store
        .run_birthdays(&mut world(&data, &mut env, &pops, 3), date(768, 3, 3))
        .expect("the birthday pass");
    let women: Vec<_> = store
        .iter()
        .filter(|c| c.sex == Sex::Female && (15..=30).contains(&age_on(c, date(768, 3, 3))))
        .collect();
    assert!(women.len() >= 3, "{}", women.len());
    assert!(women.iter().all(|c| c.father != UNKNOWN));
    assert_whole(&store, &env, &pops);
}

/// A man and his wife and their named child, all of one house in one pop, for the household cases.
fn couple() -> CharacterStore {
    let mut husband = aged(1, Sex::Male, 50, "tara", 0, "uí_néill");
    husband.birth = date(718, 5, 5);
    let mut wife = aged(2, Sex::Female, 45, "tara", 0, "uí_néill");
    wife.birth = date(723, 3, 3);
    let mut child = aged(3, Sex::Male, 18, "tara", 0, "uí_néill");
    child.birth = date(750, 6, 1);
    (husband.spouses, wife.spouses) = (vec![2], vec![1]);
    (husband.children, wife.children) = (vec![3], vec![3]);
    (child.father, child.mother) = (1, 2);
    store_of(vec![husband, wife, child])
}

#[test]
fn a_household_gets_its_parents_a_named_spouse_and_children_around_the_named_child_once() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let mut env = Env::new("household");
    let mut store = couple();
    let out = store
        .complete_household(&mut world(shipped(), &mut env, &pops, 6), 1, today())
        .expect("completed");
    assert!(out.completed && out.output.is_none());
    assert_eq!((out.spouse, out.spouse_generated), (Some(2), false));
    // The parents of both, generated into the house, conditioned on their births.
    for id in [1, 2] {
        let c = store.get(id).expect("live");
        assert_ne!((c.father, c.mother), (UNKNOWN, UNKNOWN), "{id}");
        for parent in [c.father, c.mother] {
            let p = record(&store, &env, parent);
            assert_eq!(p.life.house, "uí_néill");
            assert!(p.children.contains(&id));
            assert!(out.parents.contains(&parent));
        }
    }
    // The wedding: the first day both are marriageable (he at 14 in 732, she at 12 in 735).
    assert_eq!(out.wedding, Some(date(735, 3, 3)));
    assert!(!out.children.is_empty(), "thirty-three years of marriage");
    let nursing = shipped().conception.nursing_months;
    let named = (
        months_before(date(750, 6, 1), 9),
        first_after(date(750, 6, 1), nursing),
    );
    let mut births = Vec::new();
    for id in &out.children {
        let c = store.get(*id).expect("live");
        assert_eq!((c.father, c.mother), (1, 2));
        assert!(
            c.birth >= first_after(date(735, 3, 3), 9),
            "none conceived before the wedding"
        );
        assert!(c.birth <= today());
        // No generated interval (conception, birth, nursing) overlaps the named child's window.
        let (from, to) = (months_before(c.birth, 9), first_after(c.birth, nursing));
        assert!(from >= named.1 || to <= named.0, "{:?} overlaps", c.birth);
        assert_eq!(c.life.house, "uí_néill");
        births.push(c.birth);
    }
    assert!(births.windows(2).all(|pair| pair[0] <= pair[1]));
    for id in [1, 2] {
        assert!(store.get(id).expect("live").life.household_complete);
    }
    assert_whole(&store, &env, &pops);
    // A second completion, for either spouse, does nothing.
    let done = store.clone();
    for id in [1, 2] {
        let again = store
            .complete_household(&mut world(shipped(), &mut env, &pops, 6), id, today())
            .expect("nothing to do");
        assert!(!again.completed && again.children.is_empty() && again.created.is_empty());
        assert_eq!(store, done);
    }
}

#[test]
fn a_man_whose_fourteenth_birthday_is_in_july_marries_no_earlier_than_that_birthday() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let mut env = Env::new("july");
    let mut data: LifeCycle = shipped().clone();
    data.marriage.wish.men = vec![(12, PPB)];
    data.marriage.wish.women = vec![(12, PPB)];
    let mut man = aged(1, Sex::Male, 28, "tara", 0, "uí_néill");
    man.birth = date(740, 7, 15);
    let mut store = store_of(vec![man]);
    let out = store
        .complete_household(&mut world(&data, &mut env, &pops, 6), 1, today())
        .expect("completed");
    // Certain in the first year: not 1 January 754 but the birthday, 15 July.
    assert_eq!(out.wedding, Some(date(754, 7, 15)));
    assert!(out.spouse_generated);
    let wife_id = out.spouse.expect("a wife");
    let (husband, wife) = (
        store.get(1).expect("live"),
        store.get(wife_id).expect("live"),
    );
    assert_eq!(
        (&husband.spouses, &wife.spouses),
        (&vec![wife_id], &vec![1])
    );
    assert_eq!(wife.sex, Sex::Female);
    assert_eq!(
        wife.life.house, "uí_néill",
        "the wife takes her husband's house"
    );
    assert!(age_on(wife, date(754, 7, 15)) >= 12);
    // The `marriage` happening is dated as the wedding.
    let lines = env.logs.read(1).expect("log");
    assert!(
        lines.iter().any(|line| line.date == date(754, 7, 15)),
        "{lines:?}"
    );
    assert_whole(&store, &env, &pops);
    // With no wish at all there is no marriage and no children, but the parents are made and the household is complete.
    let mut never: LifeCycle = shipped().clone();
    never.marriage.wish.men = vec![(12, 0)];
    never.marriage.wish.women = vec![(12, 0)];
    let mut lone = aged(1, Sex::Male, 28, "tara", 0, "uí_néill");
    lone.birth = date(740, 7, 15);
    let mut store = store_of(vec![lone]);
    let out = store
        .complete_household(&mut world(&never, &mut env, &pops, 6), 1, today())
        .expect("completed");
    assert_eq!(
        (out.spouse, out.wedding, out.children.len()),
        (None, None, 0)
    );
    assert_eq!(out.parents.len(), 2);
    assert!(store.get(1).expect("live").life.household_complete);
}

/// The life-cycle data with every month a conception (the curves at 1, the factors at 1), so that a month is skipped only by a rule.
fn fertile() -> LifeCycle {
    let mut data: LifeCycle = shipped().clone();
    data.conception.monthly_ppb = u32::try_from(PPB).expect("a probability");
    data.conception.women = vec![(15, PPB), (60, PPB)];
    data.conception.men = vec![(15, PPB), (90, PPB)];
    data.conception.church_abstinence_ppb = PPB;
    data.conception.twins_ppb = 0;
    data.conception.triplets_ppb = 0;
    data.relations.mode_factor = [PPB; 3];
    data
}
/// A young couple, wed on 1 January 764 (both marriageable then), with a fecundity that makes every chance 1.
fn young_couple(named_birth: Option<GameTime>) -> CharacterStore {
    let mut husband = aged(1, Sex::Male, 18, "tara", 0, "uí_néill");
    husband.birth = date(750, 1, 1);
    let mut wife = aged(2, Sex::Female, 16, "tara", 0, "uí_néill");
    wife.birth = date(752, 1, 1);
    for c in [&mut husband, &mut wife] {
        c.genetics.value[8] = 30_000;
    }
    (husband.spouses, wife.spouses) = (vec![2], vec![1]);
    let mut people = vec![];
    if let Some(born) = named_birth {
        let mut child = aged(3, Sex::Female, 1, "tara", 0, "uí_néill");
        child.birth = born;
        (child.father, child.mother) = (1, 2);
        (husband.children, wife.children) = (vec![3], vec![3]);
        people.push(child);
    }
    people.push(husband);
    people.push(wife);
    store_of(people)
}

#[test]
fn a_conception_whose_interval_would_overlap_a_named_births_window_is_skipped() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let data = fertile();
    let run = |named: Option<GameTime>, tag: &str| {
        let mut env = Env::new(tag);
        let mut store = young_couple(named);
        // Their parents are unknown to the loader here, so the household generates them too: ids are above the people's.
        let out = store
            .complete_household(&mut world(&data, &mut env, &pops, 6), 1, date(768, 10, 9))
            .expect("completed");
        let births: Vec<GameTime> = out
            .children
            .iter()
            .map(|id| store.get(*id).expect("live").birth)
            .collect();
        (births, out.wedding, out.pregnant)
    };
    let (baseline, wedding, _) = run(None, "baseline");
    assert_eq!(wedding, Some(date(764, 1, 1)));
    assert!(baseline.len() >= 3, "{baseline:?}");
    // Every month conceives, so one child every 21 months (nine to the birth, twelve of nursing).
    assert_eq!(baseline[0], date(764, 10, 1));
    assert_eq!(baseline[1], date(766, 7, 1));
    // A named child whose conception falls in the nursing of the second generated child: the birth of that child is before the window,
    // but its nursing would run into the named conception.
    let named = first_after(baseline[1], 15);
    let nursing = shipped().conception.nursing_months;
    let (window_from, window_to) = (months_before(named, 9), first_after(named, nursing));
    assert!(baseline[1] < window_from && first_after(baseline[1], nursing) >= window_from);
    let (births, _, _) = run(Some(named), "named");
    assert!(
        !births.contains(&baseline[1]),
        "{births:?}: the month was skipped"
    );
    for birth in &births {
        let (from, to) = (months_before(*birth, 9), first_after(*birth, nursing));
        assert!(
            from > window_to || to < window_from,
            "{birth:?} overlaps {window_from:?}..{window_to:?}"
        );
    }
    // What comes before the window is as it was, and nothing after it fits in what is left of the years: the window and its nursing span
    // twenty-one months, as long as the cycle itself.
    assert_eq!(births, [baseline[0]]);
    // A named child early in the marriage: the generated conceptions that would overlap its window are skipped, and the first
    // that does not (after its nursing) is conceived, so the generated births all come after the window.
    let early = date(765, 4, 1);
    let (births, _, pregnant) = run(Some(early), "early");
    let (from, to) = (months_before(early, 9), first_after(early, nursing));
    // The named window runs from 1 July 764 to 1 April 766, an interval that ends on the day its nursing does: a conception on that day
    // is allowed (5a), so the first child is conceived on 1 April 766 and born on 1 January 767, and the next on 1 January 768, born on
    // 1 October 768, before today.
    assert_eq!(births, [date(767, 1, 1), date(768, 10, 1)]);
    assert_eq!(pregnant, None);
    assert!(births.iter().all(|birth| months_before(*birth, 9) >= to));
    assert!(!births.contains(&baseline[0]) && from < baseline[0]);
}

#[test]
fn a_conception_after_the_last_birth_that_is_not_yet_due_leaves_the_mother_pregnant() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let data = fertile();
    let mut env = Env::new("pregnant");
    let mut store = young_couple(None);
    // Conceptions in 764-01, 765-10, 767-07: the last is due on 768-04, after 767-10-09.
    let day = date(767, 10, 9);
    let out = store
        .complete_household(&mut world(&data, &mut env, &pops, 6), 1, day)
        .expect("completed");
    assert_eq!(out.pregnant, Some(2));
    let wife = store.get(2).expect("live");
    assert_eq!(
        (wife.life.pregnant_by, wife.life.due),
        (Some(1), Some(date(768, 4, 1)))
    );
    assert_eq!(out.children.len(), 2);
    assert_whole(&store, &env, &pops);
}

#[test]
fn a_household_is_one_transaction_and_a_full_pop_leaves_nothing() {
    let data = fertile();
    // Room for the couple and one more head: the first generated child fits, the second does not.
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 4);
    let mut env = Env::new("tight");
    let base = young_couple(None);
    let mut store = base.clone();
    let error = store
        .complete_household(&mut world(&data, &mut env, &pops, 6), 1, date(768, 10, 9))
        .unwrap_err();
    assert!(
        matches!(error, EventError::Rules(Error::PopFull)),
        "{error:?}"
    );
    assert_eq!(store, base);
    assert!(env.archived_ids().is_empty());
    assert!(!store.get(1).expect("live").life.household_complete);
}

#[test]
fn the_same_household_is_the_same_whenever_it_is_completed() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let make = |tag: &str| {
        let mut env = Env::new(tag);
        let mut store = couple();
        let out = store
            .complete_household(&mut world(shipped(), &mut env, &pops, 6), 1, today())
            .expect("completed");
        (
            ck_sim::Canonical::to_bytes(&store).expect("encode"),
            out.children,
            env.archived_ids(),
        )
    };
    assert_eq!(make("a"), make("b"));
}

#[test]
fn generate_request_builds_the_default_request() {
    let asked = GenerateRequest::new(
        locator("tara", 0),
        Sex::Male,
        Birth::On(date(750, 1, 1)),
        "k",
        today(),
    );
    assert_eq!(
        (asked.father, asked.mother, asked.house.as_str()),
        (UNKNOWN, UNKNOWN, "")
    );
}

#[test]
fn nursing_a_household_began_survives_the_hand_over_to_play() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let data = fertile();
    let mut env = Env::new("nursing");
    let mut store = young_couple(None);
    let mut w = world(&data, &mut env, &pops, 6);
    let outcome = store
        .complete_household(&mut w, 1, date(764, 10, 9))
        .expect("completed");
    assert_eq!(outcome.children.len(), 1);
    assert_eq!(
        store.get(outcome.children[0]).expect("live").birth,
        date(764, 10, 1)
    );
    assert_eq!(
        store.get(2).expect("live").life.nursing_until,
        Some(date(765, 10, 1))
    );
    // The real monthly path: no conception in the month after the birth, though the chances are all 1; one on the day the nursing ends.
    store
        .run_conceptions(&mut w, date(764, 11, 1))
        .expect("a month");
    assert_eq!(
        store.get(2).expect("live").life.pregnant_by,
        None,
        "still nursing until October 765"
    );
    store
        .run_conceptions(&mut w, date(765, 10, 1))
        .expect("a month");
    assert_eq!(store.get(2).expect("live").life.pregnant_by, Some(1));
}

#[test]
fn a_named_births_window_ends_on_its_nursing_deadline_and_a_conception_that_day_is_allowed() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let data = fertile();
    let mut env = Env::new("deadline");
    let mut store = young_couple(Some(date(765, 4, 1)));
    let outcome = store
        .complete_household(&mut world(&data, &mut env, &pops, 6), 1, date(767, 3, 1))
        .expect("completed");
    assert_eq!(
        store.get(outcome.children[0]).expect("live").birth,
        date(767, 1, 1),
        "April 766 was the first eligible conception, on the named child's nursing deadline"
    );
}

#[test]
fn a_mothers_named_birth_by_another_father_reserves_her_window() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let data = fertile();
    let mut env = Env::new("other-father");
    let baseline = young_couple(Some(date(765, 4, 1)));
    let mut records: Vec<_> = baseline.iter().cloned().collect();
    records
        .iter_mut()
        .find(|c| c.id == 1)
        .expect("husband")
        .children
        .clear();
    records
        .iter_mut()
        .find(|c| c.id == 3)
        .expect("child")
        .father = 4;
    let mut other = aged(4, Sex::Male, 42, "tara", 0, "other");
    other.children.push(3);
    records.push(other);
    let mut store = store_of(records);
    let outcome = store
        .complete_household(&mut world(&data, &mut env, &pops, 6), 1, date(765, 4, 2))
        .expect("completed");
    assert!(
        outcome.children.is_empty(),
        "her named pregnancy reserves July 764 onward"
    );
}

#[test]
fn a_reused_couple_must_live_where_a_child_may_come_from() {
    let pops = pops();
    let mut env = Env::new("couple-reach");
    // The only couple of the house is 200 km away; its one member in reach is at Emain.
    let mut husband = aged(1, Sex::Male, 35, "far", 0, "same-house");
    let mut wife = aged(2, Sex::Female, 35, "far", 0, "same-house");
    husband.spouses = vec![2];
    wife.spouses = vec![1];
    let near = aged(3, Sex::Male, 50, "emain", 0, "same-house");
    let mut store = store_of(vec![husband, wife, near]);
    let out = fill(
        &mut store,
        &mut env,
        &pops,
        &slot("f", "fosterling", "court", 1),
    )
    .expect("filled");
    let child = store.get(out.placed[0]).expect("live");
    let father = record(&store, &env, child.father);
    assert_ne!(father.id, 1, "not the couple outside the reach");
    assert_eq!(
        father.pop,
        locator("emain", 0),
        "a generated couple in the qualifying pop"
    );
}

#[test]
fn a_new_minor_house_never_takes_a_key_an_archived_family_kept() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let mut data: LifeCycle = shipped().clone();
    data.pools.get_mut("irish").expect("pool").men = vec![("Áed".into(), "Áeda".into())];
    let mut env = Env::new("old-house");
    // The first family of that name is dead and archived: no living member holds its key.
    let mut late = aged(1, Sex::Male, 60, "tara", 0, "Áed_tara");
    late.death = Some(date(765, 8, 1));
    ck_characters::ArchiveSink::append(&mut env.archive, &late).expect("archived");
    let mut store = store_of(vec![aged(2, Sex::Male, 14, "tara", 0, "court")]);
    let made = store
        .top_up_pool(&mut world(&data, &mut env, &pops, 3), 2, today())
        .expect("generated")
        .made;
    let houses: std::collections::BTreeSet<String> = made
        .iter()
        .map(|id| store.get(*id).expect("live").life.house.clone())
        .collect();
    assert!(!houses.contains("Áed_tara"), "{houses:?}");
    assert!(houses.contains("Áed_tara_2"), "{houses:?}");
}

#[test]
fn a_reused_couple_must_have_been_of_age_to_be_the_parents() {
    use ck_characters::generator::SlotAges;
    let pops = pops();
    let mut env = Env::new("chronology");
    // A man of fourteen, married to a woman of forty-two, in a house within reach: he was born three years after a child of seventeen.
    let mut husband = aged(1, Sex::Male, 14, "emain", 0, "h");
    let mut wife = aged(2, Sex::Female, 42, "emain", 0, "h");
    husband.spouses = vec![2];
    wife.spouses = vec![1];
    let mut store = store_of(vec![husband, wife]);
    let mut asked = slot("c", "fosterling", "court", 1);
    asked.ages = Some(SlotAges {
        male: [17, 17],
        female: [17, 17],
    });
    let out = fill(&mut store, &mut env, &pops, &asked).expect("filled");
    let child = store.get(out.placed[0]).expect("live");
    let father = record(&store, &env, child.father);
    assert_ne!(father.id, 1, "he was not yet born");
    assert!(father.birth < child.birth);
    assert_whole(&store, &env, &pops);
}

#[test]
fn a_new_minor_house_never_takes_a_key_a_living_person_was_born_into() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let mut data: LifeCycle = shipped().clone();
    data.pools.get_mut("irish").expect("pool").men = vec![("Áed".into(), "Áeda".into())];
    let mut env = Env::new("natal");
    // A woman born into `Áed_tara` who married into another house: no living member holds the key now.
    let mut wife = aged(1, Sex::Female, 30, "tara", 0, "other");
    wife.life.house_of_birth = "Áed_tara".into();
    let mut store = store_of(vec![wife, aged(2, Sex::Male, 14, "tara", 0, "court")]);
    let made = store
        .top_up_pool(&mut world(&data, &mut env, &pops, 3), 2, today())
        .expect("generated")
        .made;
    let houses: std::collections::BTreeSet<String> = made
        .iter()
        .map(|id| store.get(*id).expect("live").life.house.clone())
        .collect();
    assert!(!houses.contains("Áed_tara"), "{houses:?}");
    assert!(houses.contains("Áed_tara_2"), "{houses:?}");
}

#[test]
fn a_successful_marriage_wish_is_kept_when_no_spouse_fits_on_the_first_of_january() {
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let mut data: LifeCycle = shipped().clone();
    // No wish at 14, a certain one at 15, and no free age gap: on 1 January 755 he is 14 and no wife of fourteen is fertile, but on his
    // fifteenth birthday, when the wish succeeds, a wife of fifteen is.
    data.marriage.wish.men = vec![(14, 0), (15, PPB)];
    data.marriage.score.age_gap_free = 0;
    let mut env = Env::new("narrow-gap");
    let mut man = aged(1, Sex::Male, 15, "tara", 0, "uí_néill");
    man.birth = date(740, 7, 15);
    let mut store = store_of(vec![man]);
    let out = store
        .complete_household(&mut world(&data, &mut env, &pops, 6), 1, date(755, 7, 16))
        .expect("completed");
    assert!(out.spouse_generated, "{out:?}");
    assert_eq!(out.wedding, Some(date(755, 1, 1)));
    let wife = store.get(out.spouse.expect("a wife")).expect("live");
    assert_eq!(age_on(wife, date(755, 7, 15)), 15);
    assert_whole(&store, &env, &pops);
}

#[test]
fn completing_a_completed_household_needs_no_write_access_to_the_archive() {
    use ck_characters::ArchiveSink;
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let mut env = Env::new("readonly");
    let mut store = couple();
    store
        .complete_household(&mut world(shipped(), &mut env, &pops, 6), 1, today())
        .expect("completed");
    // The archive can be read and not written, as a save on a read-only volume is.
    let path = env.archive_path().to_owned();
    let mut permissions = std::fs::metadata(&path).expect("file").permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).expect("read only");
    let done = store.clone();
    let again = store
        .complete_household(&mut world(shipped(), &mut env, &pops, 6), 2, today())
        .expect("nothing to do, nothing to write");
    assert!(!again.completed);
    assert_eq!(store, done);
    // An empty batch is a no-op on such an archive.
    env.archive.append_all(&[]).expect("an empty batch");
    let mut permissions = std::fs::metadata(&path).expect("file").permissions();
    #[expect(
        clippy::permissions_set_readonly_false,
        reason = "a temporary file of a test, made writable again to be removed"
    )]
    permissions.set_readonly(false);
    std::fs::set_permissions(&path, permissions).expect("writable");
}

#[test]
fn a_successful_household_wish_keeps_a_marriageable_spouse_when_the_preferred_age_window_is_empty()
{
    let pops = Pops::new().pop("tara", 0, "noble", "ulaid", 1_000_000);
    let mut data: LifeCycle = shipped().clone();
    // A certain wish at 65: the free gap puts the youngest candidate at 55, beyond the end of the fertile curve of women (51), so the
    // preferred window is empty on 1 January and on the birthday alike.
    data.marriage.wish.men = vec![(64, 0), (65, PPB)];
    let mut env = Env::new("old-groom");
    let mut man = aged(1, Sex::Male, 65, "tara", 0, "uí_néill");
    man.birth = date(703, 1, 1);
    let mut store = store_of(vec![man]);
    let out = store
        .complete_household(&mut world(&data, &mut env, &pops, 6), 1, date(768, 1, 2))
        .expect("completed");
    assert!(out.spouse_generated, "{out:?}");
    assert_eq!(out.wedding, Some(date(768, 1, 1)));
    let wife = store.get(out.spouse.expect("a wife")).expect("live");
    assert!(age_on(wife, date(768, 1, 1)) >= 12, "marriageable");
    assert_whole(&store, &env, &pops);
}
