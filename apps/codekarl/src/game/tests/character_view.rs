//! The character view (CK-375 §1): the cards and the first tab's values the UI reads. The people are built as records and loaded through the
//! canonical encoding, so that each case controls exactly who exists; the shipped 768 start is read at the end.

use std::{collections::BTreeMap, path::PathBuf, sync::OnceLock};

use ck_characters::{
    Character, CharacterStore, GeneticRecord, Office, PersonalityRecord, PopLocator, Sex, UNKNOWN,
    figures::Houses,
};
use ck_economy::{EconomyState, Rules, events::Kind};
use ck_sim::{Canonical, Date, GameTime};

use crate::game::{
    character_view::{CharacterCard, CharacterView, Inputs, build, units},
    economy::starting_economy,
};

pub(super) fn assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

pub(super) fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES
        .get_or_init(|| Rules::load(assets().join("game/economy.toml")).expect("the shipped rules"))
}

pub(super) fn houses() -> &'static Houses {
    static HOUSES: OnceLock<Houses> = OnceLock::new();
    HOUSES.get_or_init(|| {
        let text = std::fs::read_to_string(assets().join("scenarios/ireland-768/houses.toml"))
            .expect("the shipped houses file");
        Houses::parse(&text).expect("it loads")
    })
}

/// The place of each slot of the shipped load file, by the slot's key.
pub(super) fn slots() -> &'static BTreeMap<String, String> {
    static SLOTS: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    SLOTS.get_or_init(|| {
        let text =
            std::fs::read_to_string(assets().join("scenarios/ireland-768/ireland-768-load.toml"))
                .expect("the shipped load file");
        ck_characters::figures::slot_locations(&text).expect("its slots")
    })
}

/// The shipped scenario's starting economy and the display names of its places.
pub(super) fn world() -> &'static (EconomyState, BTreeMap<String, String>) {
    static WORLD: OnceLock<(EconomyState, BTreeMap<String, String>)> = OnceLock::new();
    WORLD.get_or_init(|| {
        let directory = assets().join("scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("the scenario");
        let (economy, _) = starting_economy(&assets(), &directory, &scenario).expect("an economy");
        (economy, crate::game::setup::place_names(&scenario))
    })
}

/// The first pop of `kind` at the `nth` place that has one.
pub(super) fn pop(kind: &str, nth: usize) -> PopLocator {
    world()
        .0
        .places
        .iter()
        .filter_map(|place| {
            let slot = place.pops.iter().position(|pop| pop.kind == kind)?;
            Some(PopLocator {
                place: place.id.clone(),
                slot: u32::try_from(slot).expect("a slot"),
            })
        })
        .nth(nth)
        .unwrap_or_else(|| panic!("no {kind} pop number {nth}"))
}

pub(super) fn place_name(pop: &PopLocator) -> &'static str {
    world().1.get(&pop.place).expect("a place name")
}

pub(super) fn on(year: u16, month: u8, day: u8) -> GameTime {
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour: 0,
    })
    .expect("a date")
}

/// The day the views are built for: 9 October 768.
pub(super) fn today() -> Date {
    Date {
        year: 768,
        month: 10,
        day: 9,
        hour: 0,
    }
}

pub(super) fn person(
    id: u64,
    name: &str,
    sex: Sex,
    birth: GameTime,
    pop: &PopLocator,
) -> Character {
    Character {
        id,
        name: name.to_owned(),
        sex,
        birth,
        death: None,
        pop: pop.clone(),
        father: UNKNOWN,
        mother: UNKNOWN,
        spouses: Vec::new(),
        children: Vec::new(),
        office: None,
        full_until: None,
        genetics: Box::new(GeneticRecord::default()),
        personality: PersonalityRecord::default(),
        life: Box::default(),
        ties: Box::default(),
    }
}

pub(super) fn man(id: u64, pop: &PopLocator) -> Character {
    person(id, &format!("M{id}"), Sex::Male, on(730, 1, 1), pop)
}

pub(super) fn woman(id: u64, pop: &PopLocator) -> Character {
    person(id, &format!("W{id}"), Sex::Female, on(730, 1, 1), pop)
}

/// A store of `characters` and the saved names of some minor houses, built through the canonical encoding.
pub(super) fn store_with(characters: Vec<Character>, names: &[(&str, &str)]) -> CharacterStore {
    let next = characters.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    let live: BTreeMap<u64, Character> = characters.into_iter().map(|c| (c.id, c)).collect();
    let names: BTreeMap<String, String> = names
        .iter()
        .map(|(key, name)| ((*key).to_owned(), (*name).to_owned()))
        .collect();
    let mut bytes = Vec::new();
    next.encode(&mut bytes).unwrap();
    live.encode(&mut bytes).unwrap();
    0_u64.encode(&mut bytes).unwrap();
    names.encode(&mut bytes).unwrap();
    CharacterStore::from_bytes(&bytes).unwrap()
}

pub(super) fn store_of(characters: Vec<Character>) -> CharacterStore {
    store_with(characters, &[])
}

pub(super) fn viewed(store: &CharacterStore) -> CharacterView {
    build(
        store,
        &world().0,
        &Inputs {
            rules: rules(),
            places: &world().1,
            english: &world().1,
            houses: houses(),
            slots: slots(),
        },
        today(),
    )
}

fn card(view: &CharacterView, id: u64) -> &CharacterCard {
    view.card(id).unwrap_or_else(|| panic!("no card for {id}"))
}

/// Two slots of one place holding pops of the same kind (the shipped Clonmacnoise has two peasant pops).
fn twin_pops() -> (PopLocator, PopLocator) {
    world()
        .0
        .places
        .iter()
        .find_map(|place| {
            place.pops.iter().enumerate().find_map(|(first, pop)| {
                let second = place
                    .pops
                    .iter()
                    .skip(first + 1)
                    .position(|other| other.kind == pop.kind)?
                    + first
                    + 1;
                Some((
                    PopLocator {
                        place: place.id.clone(),
                        slot: u32::try_from(first).expect("a slot"),
                    },
                    PopLocator {
                        place: place.id.clone(),
                        slot: u32::try_from(second).expect("a slot"),
                    },
                ))
            })
        })
        .expect("a place with two pops of one kind")
}

#[test]
fn two_pops_of_one_kind_at_a_place_keep_their_own_characters_most_important_first_and_a_move_shows_the_new_pop()
 {
    let (first, second) = twin_pops();
    assert_eq!(
        (first.place.as_str(), first.slot < second.slot),
        (second.place.as_str(), true)
    );
    let mut ruler = man(3, &first);
    ruler.office = Some(Office::Ruler);
    let view = viewed(&store_of(vec![
        man(1, &first),
        man(2, &second),
        ruler.clone(),
    ]));
    // Each card carries its pop's exact slot, and the pop's members stand most important first (the ruler before the commoner).
    assert_eq!(
        (
            card(&view, 1).slot,
            card(&view, 2).slot,
            card(&view, 3).slot
        ),
        (first.slot, second.slot, first.slot)
    );
    assert_eq!(view.pop_members(&first.place, first.slot), [3, 1]);
    assert_eq!(view.pop_members(&first.place, second.slot), [2]);
    assert!(view.pop_members(&first.place, 99).is_empty());
    // The same character in the other pop in the next revision: the members follow.
    let moved = viewed(&store_of(vec![man(1, &second), man(2, &second), ruler]));
    assert_eq!(moved.pop_members(&first.place, first.slot), [3]);
    assert_eq!(moved.pop_members(&first.place, second.slot), [1, 2]);
}

#[test]
fn a_card_has_the_age_in_whole_years_on_the_views_date_and_the_who_keys() {
    let noble = pop("noble", 0);
    let monk = pop("monk", 0);
    let mut born_today = man(1, &noble);
    born_today.birth = on(754, 10, 9);
    let mut tomorrow = man(2, &noble);
    tomorrow.birth = on(754, 10, 10);
    let mut monk_boy = man(3, &monk);
    monk_boy.birth = on(768, 10, 9);
    let mut abbot = man(4, &noble);
    abbot.office = Some(Office::HighCleric);
    let mut king = woman(5, &noble);
    king.office = Some(Office::Ruler);
    let view = viewed(&store_of(vec![born_today, tomorrow, monk_boy, abbot, king]));
    // 14 on the day of the birthday; 13 the day before it; 0 on the day of birth.
    assert_eq!(card(&view, 1).age, 14);
    assert!(!card(&view, 1).child, "children are under 14");
    assert_eq!(card(&view, 2).age, 13);
    assert!(card(&view, 2).child);
    assert_eq!(card(&view, 3).age, 0);
    // Clergy: a monk pop, or the high_cleric office.
    assert!(card(&view, 3).clergy && card(&view, 4).clergy);
    assert!(!card(&view, 1).clergy && !card(&view, 5).clergy);
    // Rulers and sex.
    assert!(card(&view, 5).ruler && !card(&view, 4).ruler);
    assert!(card(&view, 5).female && !card(&view, 1).female);
    assert_eq!(card(&view, 1).place, noble.place);
    assert_eq!(card(&view, 1).kind, "noble");
    // Cards are in id order.
    assert!(view.cards.windows(2).all(|pair| pair[0].id < pair[1].id));
}

#[test]
fn the_house_line_is_the_dynasty_and_name_a_generated_houses_saved_name_or_none() {
    let noble = pop("noble", 0);
    let mut with_dynasty = man(1, &noble);
    with_dynasty.life.house = "cenel_neogain".to_owned();
    let mut without_dynasty = man(2, &noble);
    without_dynasty.life.house = "ui_maine".to_owned();
    let mut minor = man(3, &noble);
    minor.life.house = "Áed_tara_2".to_owned();
    let commoner = man(4, &noble);
    let mut unlisted = man(5, &noble);
    unlisted.life.house = "some_key".to_owned();
    let view = viewed(&store_with(
        vec![with_dynasty, without_dynasty, minor, commoner, unlisted],
        &[("Áed_tara_2", "Clann Áeda")],
    ));
    assert_eq!(card(&view, 1).house, "Uí Néill · Cenél nEógain");
    assert_eq!(
        card(&view, 2).house,
        "Uí Maine",
        "no dynasty: the name alone"
    );
    assert_eq!(
        card(&view, 3).house,
        "Clann Áeda",
        "the saved name, for a suffixed key too"
    );
    assert_eq!(card(&view, 4).house, "", "no house");
    assert_eq!(
        card(&view, 5).house,
        "some key",
        "a key nobody names shows as words"
    );
}

/// The role line of `c` among `others`, all in a store.
fn role_of(c: Character, others: Vec<Character>) -> String {
    let id = c.id;
    let mut all = others;
    all.push(c);
    card(&viewed(&store_of(all)), id).role.clone()
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one fixture world and each rule of the role line in order, with the cases where an earlier rule wins"
)]
fn the_role_line_follows_the_rules_in_order() {
    let noble = pop("noble", 0);
    let other = pop("noble", 1);
    let monk = pop("monk", 0);
    let peasant = pop("peasant", 0);
    let (here, there) = (place_name(&noble), place_name(&other));
    // 1: a ruler.
    let mut ruler = man(1, &noble);
    ruler.office = Some(Office::Ruler);
    assert_eq!(role_of(ruler.clone(), vec![]), format!("ruler of {here}"));
    // 2: a high cleric: the slot's role where it has one, else the head of the church.
    let mut abbot = man(2, &monk);
    abbot.office = Some(Office::HighCleric);
    assert_eq!(
        role_of(abbot.clone(), vec![]),
        format!("head of the church of {}", place_name(&monk))
    );
    abbot.life.slot_key = "community".to_owned();
    abbot.life.role = "abbot".to_owned();
    assert_eq!(
        role_of(abbot, vec![]),
        format!("abbot of {}", place_name(&monk))
    );
    // 3: a fosterling, hostage or student, by the slot's kind whatever the role field says, so a reused and a generated occupant read alike.
    for (kind, role) in [
        ("fosterling", ""),
        ("hostage", "retainer"),
        ("student", "scribe"),
        ("student", ""),
    ] {
        let mut guest = man(3, &noble);
        guest.life.slot_key = "court".to_owned();
        guest.life.slot_kind = kind.to_owned();
        guest.life.role = role.to_owned();
        assert_eq!(
            role_of(guest, vec![]),
            format!("{kind} at {here}"),
            "{kind}/{role}"
        );
    }
    // 4: a spouse place of a ruler or of a lord: wife for a woman, husband for a man, at the slot's place.
    for (sex, kind, line) in [
        (Sex::Female, "spouse_of_ruler", "wife of the ruler of"),
        (Sex::Male, "spouse_of_ruler", "husband of the ruler of"),
        (Sex::Female, "spouse_of_lord", "wife of the lord of"),
        (Sex::Male, "spouse_of_lord", "husband of the lord of"),
    ] {
        let mut spouse = person(4, "S", sex, on(730, 1, 1), &other);
        spouse.life.slot_key = "court".to_owned();
        spouse.life.slot_kind = kind.to_owned();
        assert_eq!(role_of(spouse, vec![]), format!("{line} {there}"), "{kind}");
        // The role field alone names it too.
        let mut by_role = person(4, "S", sex, on(730, 1, 1), &other);
        by_role.life.slot_key = "court".to_owned();
        by_role.life.role = kind.to_owned();
        assert_eq!(
            role_of(by_role, vec![]),
            format!("{line} {there}"),
            "{kind} by role"
        );
    }
    // Rule 4 names the slot's own place: a widow who married again lives in another pop and still holds the slot she was recorded in.
    let ailech = world().1.get("ailech").expect("the place of the slot");
    assert_ne!(
        ailech, there,
        "the fixture's pops are not at the slot's place"
    );
    for (kind, line) in [
        ("spouse_of_ruler", "wife of the ruler of"),
        ("spouse_of_lord", "wife of the lord of"),
    ] {
        let mut widow = woman(4, &other);
        widow.life.slot_key = "ailech_court".to_owned();
        widow.life.slot_kind = kind.to_owned();
        assert_eq!(
            role_of(widow, vec![]),
            format!("{line} {ailech}"),
            "{kind} at the slot's place"
        );
    }
    // A high cleric reads its slot role by the first-match rule, whatever the word: only §4 rule 2's "slot role" decides.
    let mut cleric = man(4, &monk);
    cleric.office = Some(Office::HighCleric);
    cleric.life.slot_key = "community".to_owned();
    cleric.life.slot_kind = "student".to_owned();
    assert_eq!(
        role_of(cleric, vec![]),
        format!("student of {}", place_name(&monk))
    );
    // 5: any other slot member with a role; the underscores are spaces.
    let mut poet = man(5, &noble);
    poet.life.slot_key = "court".to_owned();
    poet.life.role = "poet".to_owned();
    assert_eq!(role_of(poet, vec![]), format!("poet of {here}"));
    let mut keeper = man(5, &noble);
    keeper.life.slot_key = "court".to_owned();
    keeper.life.slot_kind = "keeper_of_relics".to_owned();
    assert_eq!(
        role_of(keeper, vec![]),
        format!("keeper of relics of {here}")
    );
    // 6: a living ruler's spouse, at the place of the first living ruler in her spouse list, and "husband" for a man.
    let mut king = man(1, &other);
    king.office = Some(Office::Ruler);
    let mut dead_husband = man(6, &noble);
    dead_husband.office = Some(Office::Ruler);
    let mut wife = woman(7, &noble);
    // The first spouse listed (id 99) is not a live character; the next, a live ruler, is the one that counts.
    wife.spouses = vec![99, 1];
    assert_eq!(
        role_of(wife, vec![king.clone(), dead_husband]),
        format!("wife of the ruler of {there}")
    );
    let mut queen = woman(8, &other);
    queen.office = Some(Office::Ruler);
    let mut husband = man(9, &noble);
    husband.spouses = vec![8];
    assert_eq!(
        role_of(husband, vec![queen.clone()]),
        format!("husband of the ruler of {there}")
    );
    // 7: a living ruler's child: the father's place if he is a living ruler, else the mother's.
    let mut son = man(10, &noble);
    son.father = 1;
    assert_eq!(
        role_of(son, vec![king.clone()]),
        format!("son of the ruler of {there}")
    );
    let mut daughter = woman(11, &noble);
    daughter.mother = 8;
    assert_eq!(
        role_of(daughter.clone(), vec![queen.clone()]),
        format!("daughter of the ruler of {there}")
    );
    daughter.father = 6;
    let mut father = man(6, &noble);
    father.office = Some(Office::Ruler);
    assert_eq!(
        role_of(daughter, vec![queen, father]),
        format!("daughter of the ruler of {here}"),
        "the father's place first"
    );
    // 8: anyone else: the pop's kind word.
    assert_eq!(role_of(man(12, &peasant), vec![]), "peasant");
    assert_eq!(role_of(man(12, &noble), vec![]), "noble");
    assert_eq!(role_of(man(12, &monk), vec![]), "monk");
    // The first match wins: an office over a slot, a guest's place over being a ruler's child, a slot role over a ruler's child.
    let mut ruler_and_guest = ruler;
    ruler_and_guest.life.slot_key = "court".to_owned();
    ruler_and_guest.life.slot_kind = "hostage".to_owned();
    assert_eq!(role_of(ruler_and_guest, vec![]), format!("ruler of {here}"));
    let mut fostered_prince = man(13, &noble);
    fostered_prince.father = 1;
    fostered_prince.life.slot_key = "court".to_owned();
    fostered_prince.life.slot_kind = "fosterling".to_owned();
    assert_eq!(
        role_of(fostered_prince, vec![king.clone()]),
        format!("fosterling at {here}")
    );
    let mut prince_and_poet = man(14, &noble);
    prince_and_poet.father = 1;
    prince_and_poet.life.slot_key = "court".to_owned();
    prince_and_poet.life.role = "poet".to_owned();
    assert_eq!(
        role_of(prince_and_poet, vec![king]),
        format!("poet of {here}")
    );
}

#[test]
fn importance_orders_the_bands_older_first_then_by_id() {
    let noble = pop("noble", 0);
    let mut ruler = man(10, &noble);
    ruler.office = Some(Office::Ruler);
    ruler.birth = on(720, 1, 1);
    ruler.spouses = vec![11, 12];
    let mut cleric = man(20, &noble);
    cleric.office = Some(Office::HighCleric);
    cleric.birth = on(740, 1, 1);
    // The ruler's two wives, born in one year: the ties go by id.
    let mut wife_b = woman(12, &noble);
    wife_b.birth = on(730, 6, 1);
    wife_b.spouses = vec![10];
    let mut wife_a = woman(11, &noble);
    wife_a.birth = on(730, 6, 1);
    wife_a.spouses = vec![10];
    // The ruler's children: the elder first.
    let mut younger = man(31, &noble);
    younger.father = 10;
    younger.birth = on(760, 1, 1);
    let mut elder = man(32, &noble);
    elder.father = 10;
    elder.birth = on(750, 1, 1);
    // A real figure, then a slot member, then everyone else.
    let mut figure = man(40, &noble);
    figure.life.figure = "niall_frossach".to_owned();
    figure.birth = on(745, 1, 1);
    let mut member = man(5, &noble);
    member.life.slot_key = "court".to_owned();
    member.life.role = "poet".to_owned();
    member.birth = on(700, 1, 1);
    let mut old_commoner = man(7, &noble);
    old_commoner.birth = on(690, 1, 1);
    let mut young_commoner = man(2, &noble);
    young_commoner.birth = on(760, 1, 1);
    let mut twin = man(3, &noble);
    twin.birth = on(760, 1, 1);
    let view = viewed(&store_of(vec![
        young_commoner,
        twin,
        member,
        old_commoner,
        figure,
        younger,
        elder,
        wife_b,
        wife_a,
        cleric,
        ruler,
    ]));
    let mut by_importance: Vec<_> = view.cards.iter().collect();
    by_importance.sort_by_key(|card| card.importance);
    let order: Vec<u64> = by_importance.iter().map(|card| card.id).collect();
    assert_eq!(
        order,
        // rulers; high clergy; the ruler's spouses (11 and 12 tie on age: by id); his children (elder first); real figures; slot members;
        // everyone else, older first and a tie by id.
        vec![10, 20, 11, 12, 32, 31, 40, 5, 7, 2, 3]
    );
    // The ranks are 0..n, each once.
    let mut ranks: Vec<u32> = view.cards.iter().map(|card| card.importance).collect();
    ranks.sort_unstable();
    assert_eq!(ranks, (0..11).collect::<Vec<u32>>());
}

#[test]
fn a_dead_character_is_not_in_the_next_view() {
    let noble = pop("noble", 0);
    let both = viewed(&store_of(vec![man(1, &noble), man(2, &noble)]));
    assert!(both.card(2).is_some() && both.first_tab(2).is_some());
    let one = viewed(&store_of(vec![man(1, &noble)]));
    assert!(one.card(2).is_none() && one.first_tab(2).is_none());
    assert_eq!(one.cards.len(), 1);
}

#[test]
fn whole_units_are_the_nearest_with_a_tie_away_from_zero() {
    assert_eq!(units(0), 0);
    assert_eq!(units(499), 0);
    assert_eq!(units(500), 1);
    assert_eq!(units(1_499), 1);
    assert_eq!(units(1_500), 2);
    assert_eq!(units(-499), 0);
    assert_eq!(units(-500), -1);
    assert_eq!(units(-1_500), -2);
    // The ends do not overflow.
    assert_eq!(units(i64::MAX), 9_223_372_036_854_775);
    assert_eq!(units(i64::MIN), -9_223_372_036_854_775);
}

#[test]
fn the_first_tab_shows_the_values_the_game_already_shows() {
    let noble = pop("noble", 0);
    let mut c = man(1, &noble);
    // Height 172 cm with a nutrition offset of +3 cm; the weight has moved to 74.5 kg: both are what the game shows.
    c.genetics.value[0] = 172_000;
    c.life.height_offset = 3_000;
    c.life.weight = 74_500;
    c.genetics.value[2..9].copy_from_slice(&[9_400, 13_500, 11_000, 12_000, 7_000, 10_000, 6_499]);
    c.genetics.value[9..13].copy_from_slice(&[4_000, 3_000, 6_000, 12_000]);
    let leadership = rules().events.skills.list[0].clone();
    c.life.skills.insert(leadership.clone(), 11_400);
    let last = rules().events.skills.list.last().expect("skills").clone();
    c.life.skills.insert(last.clone(), 24_000);
    c.personality.vice = [-2_400, 3_000, 1_000, 2_000, -1_000, -3_000, 4_000];
    c.personality.manner = [
        4_000, 12_500, 3_000, -6_000, -8_000, -2_000, -10_000, -12_500, 30_000,
    ];
    c.personality.stress = 38_400;
    c.personality.popularity = 34_000;
    c.personality.fame = -61_000;
    c.personality.honour = 48_500;
    let view = viewed(&store_of(vec![c.clone()]));
    let tab = view.first_tab(1).expect("a first tab");
    // The body: each value through `shown_stat`, height with its nutrition adjustment and the current weight.
    for index in 0..9 {
        assert_eq!(
            tab.body[index],
            units(c.shown_stat(&rules().genetics, index)),
            "{index}"
        );
    }
    assert_eq!(tab.body[0], 175, "172 cm and 3 cm of nutrition");
    assert_eq!(tab.body[1], 75, "74.5 kg is shown as 75");
    assert_eq!(&tab.body[2..], &[9, 14, 11, 12, 7, 10, 6]);
    // The humours are pips, held to 0..=10.
    assert_eq!(tab.humours, [4, 3, 6, 10]);
    // The skills are in the data's order, each with its true number.
    assert_eq!(tab.skills.len(), rules().events.skills.list.len());
    assert_eq!(tab.skills[0], (leadership, 11));
    assert_eq!(
        tab.skills.last(),
        Some(&(last, 24)),
        "a value over the bar's end keeps its true number"
    );
    // Vice and virtue, manner, stress and standing: true signed values, rounded once.
    assert_eq!(tab.virtue, [-2, 3, 1, 2, -1, -3, 4]);
    assert_eq!(tab.manner, [4, 13, 3, -6, -8, -2, -10, -13, 30]);
    assert_eq!(tab.stress, 38);
    assert_eq!(
        tab.threshold,
        units(ck_characters::personality::threshold(&c))
    );
    assert_eq!(tab.standing, [34, -61, 49]);
}

#[test]
fn the_traits_section_lists_the_present_keys_with_a_letter_for_a_graded_one() {
    let noble = pop("noble", 0);
    let catalog = &rules().events.catalog;
    // An attribute (a light chip) of the catalog, graded or not, that genetics does not also pass.
    let pick = |graded: bool| {
        rules()
            .events
            .catalog_order
            .iter()
            .find(|key| {
                let entry = &catalog[*key];
                entry.graded == graded
                    && !entry.inherited
                    && !matches!(entry.kind, Kind::InheritedTrait | Kind::MindTrait)
            })
            .cloned()
    };
    let graded = pick(true).expect("a graded attribute");
    let ungraded = pick(false).expect("an ungraded attribute");
    let mut c = man(1, &noble);
    c.life.grades.insert(graded.clone(), 1);
    c.life.grades.insert(ungraded.clone(), 1);
    let view = viewed(&store_of(vec![c.clone(), man(2, &noble)]));
    let chips = &view.first_tab(1).expect("a tab").traits;
    let find = |key: &str| chips.iter().find(|chip| chip.key == key);
    assert_eq!(
        find(&graded).map(|chip| (chip.attribute, chip.letter)),
        Some((true, "C"))
    );
    assert_eq!(
        find(&ungraded).map(|chip| (chip.attribute, chip.letter)),
        Some((true, ""))
    );
    assert_eq!(chips.len(), 2, "only the present keys: {chips:?}");
    // The words are the key with `_` as a space.
    assert_eq!(
        find(&graded).map(|chip| chip.words.clone()),
        Some(graded.replace('_', " "))
    );
    // A higher grade shows its own letter.
    let mut better = c;
    better.life.grades.insert(graded.clone(), 3);
    let view = viewed(&store_of(vec![better]));
    let chips = &view.first_tab(1).expect("a tab").traits;
    assert_eq!(
        chips
            .iter()
            .find(|chip| chip.key == graded)
            .map(|chip| chip.letter),
        Some("A")
    );
    assert!(view.first_tab(2).is_none());
    // A character with none has an empty section.
    assert!(
        viewed(&store_of(vec![man(3, &noble)]))
            .first_tab(3)
            .expect("a tab")
            .traits
            .is_empty()
    );
}

fn assert_shipped_find_entries(scenario: &ck_map::Scenario, view: &CharacterView) {
    let cache = std::sync::Arc::new(crate::game_api::ViewCache::default());
    cache.post(
        1,
        std::sync::Arc::new(crate::game::view::EconomyView {
            characters: view.clone(),
            ..Default::default()
        }),
    );
    cache.note_world(1, 0);
    let rivers = scenario
        .map
        .river_network
        .as_ref()
        .unwrap()
        .rivers
        .iter()
        .map(|river| (river.id.clone(), river.name.clone()))
        .collect();
    let find = crate::game_api::find_entries_function(
        cache,
        crate::game_api::places_of(scenario),
        rivers,
        crate::place_names::PlaceNameSetting::default(),
    );
    let result = find(&[Some(ck_lua::ConfigValue::Integer(1))]).unwrap();
    let ck_lua::ConfigValue::Array(rows) = &result[0] else {
        panic!("find rows")
    };
    let has = |kind: &str, name: &str, place: Option<&str>| {
        rows.iter().any(|row| {
            let ck_lua::ConfigValue::Table(fields) = row else {
                return false;
            };
            fields.get("kind") == Some(&ck_lua::ConfigValue::String(kind.to_owned()))
                && fields.get("name") == Some(&ck_lua::ConfigValue::String(name.to_owned()))
                && place.is_none_or(|place| {
                    fields.get("place") == Some(&ck_lua::ConfigValue::String(place.to_owned()))
                })
        })
    };
    // The default choice shows English names (CK-423): Grianán Ailigh is Grianan of Aileach.
    assert!(has("location", "Grianan of Aileach", None));
    assert!(!has("location", "Grianán Ailigh", None));
    assert!(has("river", "Erne", None));
    assert!(has(
        "character",
        "Niall mac Fergaile Frossach",
        Some("Grianan of Aileach")
    ));
}

#[test]
fn the_shipped_start_lists_its_live_characters_and_opens_niall_frossach() {
    let (_dir, prepared, _) = super::start_768::shipped(7);
    let state = prepared.simulation.state();
    let date = prepared.simulation.date().expect("a date");
    let view = build(
        &state.characters,
        &state.economy,
        &Inputs {
            rules: rules(),
            places: &crate::game::setup::place_names(&prepared.scenario),
            english: &crate::game::setup::english_place_names(&prepared.scenario),
            houses: houses(),
            slots: slots(),
        },
        date,
    );
    let live = state.characters.iter().count();
    assert!(live > 2_000, "{live}");
    assert_eq!(view.cards.len(), live, "the tab lists the live characters");
    assert!(
        view.cards
            .iter()
            .all(|card| view.first_tab(card.id).is_some())
    );
    // Niall Frossach, by his figure key.
    let niall = state
        .characters
        .iter()
        .find(|c| c.life.figure == "niall_frossach")
        .expect("Niall is a live real figure");
    let card = card(&view, niall.id);
    assert_eq!(card.name, "Niall mac Fergaile");
    assert_eq!(card.epithet, "Frossach");
    assert_eq!(card.house, "Uí Néill · Cenél nEógain");
    assert_eq!(card.age, 50, "born in 718, the start is in 768");
    assert_eq!(card.role, "ruler of Grianán Ailigh");
    assert!(card.ruler && !card.female && !card.clergy && !card.child);
    // The first tab has his values: a veteran of grade C, skills in the data's order, a stress within its bar.
    let tab = view.first_tab(niall.id).expect("his first tab");
    assert_eq!(tab.skills.len(), rules().events.skills.list.len());
    assert!(tab.traits.iter().all(|chip| !chip.key.is_empty()));
    // Everyone has a house line or none, a role line, and a stable place; real figures are all in the view.
    assert!(
        view.cards
            .iter()
            .all(|card| !card.role.is_empty() && !card.place.is_empty())
    );
    let figures = state
        .characters
        .iter()
        .filter(|c| !c.life.figure.is_empty())
        .count();
    assert!(figures > 200, "{figures} real figures live at the start");
    assert_shipped_find_entries(&prepared.scenario, &view);
    // The cost grows with the characters and is paid once a day: the whole view of the start is built in well under a second.
    let started = std::time::Instant::now();
    let again = build(
        &state.characters,
        &state.economy,
        &Inputs {
            rules: rules(),
            places: &crate::game::setup::place_names(&prepared.scenario),
            english: &crate::game::setup::english_place_names(&prepared.scenario),
            houses: houses(),
            slots: slots(),
        },
        date,
    );
    assert_eq!(again, view, "the same state gives the same view");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_chip_says_its_catalog_name_where_the_rules_give_one() {
    let noble = pop("noble", 0);
    let names = &rules().events.catalog_names;
    assert!(!names.is_empty(), "the shipped rules name some keys");
    let mut c = man(1, &noble);
    for key in names.keys() {
        c.life.grades.insert(key.clone(), 1);
    }
    let view = viewed(&store_of(vec![c]));
    let chips = &view.first_tab(1).expect("a tab").traits;
    for (key, words) in names {
        let chip = chips.iter().find(|chip| &chip.key == key);
        // A key genetics owns shows from its own level, not from `life.grades`: only the keys held here can be checked.
        if let Some(chip) = chip {
            assert_eq!(&chip.words, words, "{key}");
        }
    }
    assert!(
        chips.iter().any(|chip| chip.words == "Céle Dé"),
        "{chips:?}"
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the cards, a record's sections and each named refusal of the two functions in one story"
)]
fn game_characters_and_game_character_serve_the_cached_view_and_refuse_by_name() {
    use std::sync::Arc;

    use ck_lua::ConfigValue;

    use crate::game::view::EconomyView;
    use crate::game_api::{ViewCache, character_function, characters_function};

    let noble = pop("noble", 0);
    let mut niall = man(1, &noble);
    niall.office = Some(Office::Ruler);
    niall.life.house = "cenel_neogain".to_owned();
    niall
        .life
        .grades
        .insert(rules().events.catalog_order[0].clone(), 1);
    let view = EconomyView {
        characters: viewed(&store_of(vec![niall, woman(2, &noble)])),
        ..EconomyView::default()
    };
    let cache = Arc::new(ViewCache::default());
    cache.post(7, Arc::new(view.clone()));
    let characters = characters_function(
        Arc::clone(&cache),
        crate::place_names::PlaceNameSetting::default(),
    );
    let character = character_function(
        Arc::clone(&cache),
        crate::place_names::PlaceNameSetting::default(),
    );
    let integer = |value: i64| Some(ConfigValue::Integer(value));
    let table = |value: &ConfigValue| match value {
        ConfigValue::Table(fields) => fields.clone(),
        other => panic!("a table, not {other:?}"),
    };
    // The cards, in id order, with every field the ledger reads.
    let answer = characters(&[integer(7)]).expect("the cards");
    let ConfigValue::Array(cards) = &answer[0] else {
        panic!("an array of cards")
    };
    assert_eq!(cards.len(), 2);
    let first = table(&cards[0]);
    assert_eq!(first["id"], ConfigValue::Integer(1));
    assert_eq!(first["sex"], ConfigValue::String("m".to_owned()));
    assert_eq!(first["ruler"], ConfigValue::Bool(true));
    assert_eq!(
        first["house"],
        ConfigValue::String("Uí Néill · Cenél nEógain".to_owned())
    );
    for key in [
        "name",
        "epithet",
        "age",
        "role",
        "place",
        "kind",
        "clergy",
        "child",
        "importance",
    ] {
        assert!(first.contains_key(key), "{key}");
    }
    // One character: its header and the values of each section.
    let record = table(&character(&[integer(1), integer(7)]).expect("a record")[0]);
    for (key, count) in [
        ("body", 9),
        ("humours", 4),
        ("virtue", 7),
        ("manner", 9),
        ("standing", 3),
    ] {
        let ConfigValue::Array(entries) = &record[key] else {
            panic!("{key} is an array")
        };
        assert_eq!(entries.len(), count, "{key}");
        assert!(entries.iter().all(|entry| {
            let fields = table(entry);
            fields.contains_key("key") && fields.contains_key("value")
        }));
    }
    let ConfigValue::Array(skills) = &record["skills"] else {
        panic!("skills")
    };
    assert_eq!(skills.len(), rules().events.skills.list.len());
    assert_eq!(
        record["health"],
        ConfigValue::Array(Vec::new()),
        "no injuries are simulated yet"
    );
    assert!(matches!(&record["traits"], ConfigValue::Array(chips) if chips.len() == 1));
    assert!(record.contains_key("stress") && record.contains_key("threshold"));
    // Refusals are named: a character who is not there, a revision never given and one that has left, and arguments of the wrong kind.
    assert_eq!(
        character(&[integer(99), integer(7)]).unwrap_err(),
        "game.character: no character 99 at revision 7"
    );
    assert_eq!(
        characters(&[integer(8)]).unwrap_err(),
        "game.characters: revision 8 is missing"
    );
    // Revision 7's call completes, then a later one does: 7 has left the cache.
    cache.complete(7);
    cache.post(8, Arc::new(view));
    cache.complete(8);
    assert_eq!(
        characters(&[integer(7)]).unwrap_err(),
        "game.characters: revision 7 is retired"
    );
    assert_eq!(
        character(&[integer(1), integer(7)]).unwrap_err(),
        "game.character: revision 7 is retired"
    );
    assert!(characters(&[]).is_err() && character(&[integer(1)]).is_err());
    assert!(
        character(&[Some(ConfigValue::String("1".to_owned())), integer(8)]).is_err(),
        "an id is a number"
    );
}

/// A `ConfigValue` as Lua source, for handing the real Rust view to the Lua harness.
pub(super) fn lua_of(value: &ck_lua::ConfigValue) -> String {
    use ck_lua::ConfigValue;
    match value {
        ConfigValue::Integer(number) => number.to_string(),
        ConfigValue::Bool(flag) => flag.to_string(),
        ConfigValue::String(text) => {
            let mut out = String::from("\"");
            for character in text.chars() {
                match character {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    other => out.push(other),
                }
            }
            out.push('"');
            out
        }
        ConfigValue::Array(items) => format!(
            "{{{}}}",
            items.iter().map(lua_of).collect::<Vec<_>>().join(",")
        ),
        ConfigValue::Table(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(key, value)| format!(
                    "[{}]={}",
                    lua_of(&ConfigValue::String(key.clone())),
                    lua_of(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        ConfigValue::Float(number) => panic!("the UI is never given a float: {number}"),
    }
}

#[test]
fn the_real_shipped_start_reaches_the_shipped_ui_through_the_real_functions_and_opens_niall_frossach()
 {
    use std::sync::Arc;

    use ck_lua::ConfigValue;

    use crate::game::view::EconomyView;
    use crate::game_api::{ViewCache, character_function, characters_function};

    let (_dir, prepared, _) = super::start_768::shipped(7);
    let state = prepared.simulation.state();
    let date = prepared.simulation.date().expect("a date");
    let view = build(
        &state.characters,
        &state.economy,
        &Inputs {
            rules: rules(),
            places: &crate::game::setup::place_names(&prepared.scenario),
            english: &crate::game::setup::english_place_names(&prepared.scenario),
            houses: houses(),
            slots: slots(),
        },
        date,
    );
    let niall = state
        .characters
        .iter()
        .find(|c| c.life.figure == "niall_frossach")
        .expect("Niall is a live real figure");
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
    let cards = characters_function(
        Arc::clone(&cache),
        crate::place_names::PlaceNameSetting::default(),
    )(&[Some(ConfigValue::Integer(1))])
    .expect("the real cards");
    let record = character_function(cache, crate::place_names::PlaceNameSetting::default())(&[
        Some(ConfigValue::Integer(id)),
        Some(ConfigValue::Integer(1)),
    ])
    .expect("his real record");
    let notes = crate::ui_link::location_tests::characters::real_data_notes(
        &lua_of(&cards[0]),
        &lua_of(&record[0]),
        id,
        rank,
    );
    // The harness records a height for each card it was given; the messages leave those out.
    let notes: Vec<String> = notes
        .into_iter()
        .filter(|line| !line.starts_with("cardstack"))
        .collect();
    let live = state.characters.iter().count();
    assert!(
        notes.iter().any(|line| line == &format!("listed {live}")),
        "the tab was given every live character: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|line| line.starts_with(&format!("screen id={id} tab=character"))),
        "the click on his card opened his screen: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|line| line == "header true true true true"),
        "his name and epithet, house line, age and role line are drawn: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|line| line.starts_with("section_heights 47,20,70,50,41,60,22,")),
        "{notes:?}"
    );
}
