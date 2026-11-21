//! The character view (characters slice 9a, CK-375 §1): an immutable, plain-data picture of the live characters that the simulation builds
//! and the UI reads, built at the same three moments as the economy view and carried in it, so under the same revision. The UI never
//! reads the live store.
//!
//! Per **live** character it holds a card (the ledger's row) and the values of the character screen's first tab. Everything derived here (age,
//! the house and role lines, importance, the shown numbers) is simulation-side code under `game/`, so that the build id covers it. The
//! build is one pass over the characters in id order, with no pairwise work: a role line looks at the character's own spouses, parents and
//! slot only.
//!
//! The numbers are the ones the game already shows, with no new scoring: the body through `Character::shown_stat`, the humours through
//! `GeneticRecord::shown`, and skills, vice/virtue, manner, stress and standing from the record. A value stored in thousandths is shown in whole
//! units, to the nearest, ties away from zero; a value outside a bar's range keeps its true number (the UI draws the bar clamped).

use std::collections::BTreeMap;

use ck_characters::{
    Character, CharacterStore, LogView, Office, Populations, Sex, cycle, events::age_on,
    figures::Houses, personality::threshold,
};
use ck_economy::{
    EconomyState, Rules,
    events::{GRADE_NAMES, Kind},
    genetics::{ATTRIBUTES, HUMOURS},
};
use ck_sim::Date;

use super::character_relations::{Relations, RelationsFacts};

/// A character is a child under this age (the ledger's `children` filter).
pub(crate) const CHILD_UNDER: u32 = 14;

/// The slot kinds of a place held by a person who is not of the household (§4 rule 3).
const GUEST_KINDS: [&str; 3] = ["fosterling", "hostage", "student"];

/// One row of the ledger's Characters tab.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the sex and the three kinds (ruler, clergy, child) are independent facts the ledger's list bar reads"
)]
pub(crate) struct CharacterCard {
    pub(crate) id: u64,
    /// The name, with the patronymic where the father is a live character (the view does not read the archive).
    pub(crate) name: String,
    /// The byname or the shown epithet; empty for none.
    pub(crate) epithet: String,
    pub(crate) female: bool,
    /// Age in whole years on the view's date.
    pub(crate) age: u32,
    /// The house line (§3); empty for no house.
    pub(crate) house: String,
    /// The role line (§4).
    pub(crate) role: String,
    /// The pop's place id.
    pub(crate) place: String,
    /// The pop's slot in its place: with the place, the exact pop (a place can hold two pops of one kind).
    pub(crate) slot: u32,
    /// The pop's kind.
    pub(crate) kind: String,
    /// The filter keys of §2.
    pub(crate) ruler: bool,
    pub(crate) clergy: bool,
    pub(crate) child: bool,
    /// Where the character stands in the importance order (§2): 0 is the most important.
    pub(crate) importance: u32,
}

/// One chip of the traits section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TraitChip {
    pub(crate) key: String,
    /// What the chip says: its form in the rules' `[catalog_names]` where it has one ("Céle Dé"), else the key with `_` as a space.
    pub(crate) words: String,
    /// Whether it is an attribute (a light chip) rather than a trait (a dark one).
    pub(crate) attribute: bool,
    /// The class-level letter of a graded key; empty for an ungraded one.
    pub(crate) letter: &'static str,
}

/// The values of the character screen's first tab.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FirstTab {
    /// Height (cm), weight (kg), strength, wit, intellect, cunning, longevity, constitution, fecundity.
    pub(crate) body: [i64; 9],
    /// Blood, yellow bile, black bile and phlegm, held to 0..=10 pips.
    pub(crate) humours: [i64; 4],
    /// The skills in data order, with their keys.
    pub(crate) skills: Vec<(String, i64)>,
    /// The seven pairs, the sin negative.
    pub(crate) virtue: [i64; 7],
    pub(crate) stress: i64,
    pub(crate) threshold: i64,
    /// The nine manner pairs, true values.
    pub(crate) manner: [i64; 9],
    /// Popularity, fame, honour.
    pub(crate) standing: [i64; 3],
    pub(crate) traits: Vec<TraitChip>,
}

/// The live characters at one revision.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CharacterView {
    /// The cards in id order.
    pub(crate) cards: Vec<CharacterCard>,
    tabs: BTreeMap<u64, FirstTab>,
    /// The live characters of each pop, by its place and slot, most important first (the pop tab's characters row).
    pops: BTreeMap<(String, u32), Vec<u64>>,
    /// The calendar day of the view: the life log shows no line dated after it.
    day: (u16, u8, u8),
    /// The world's life log as of this publication (CK-378 §2); `None` when the log could not be read, and the log refuses.
    pub(crate) log: Option<LogView>,
    /// What the relations tab reads (CK-377): the live characters, the archive as published and the indexes, as of this revision.
    pub(crate) relations: RelationsFacts,
}

impl CharacterView {
    /// The card of `id`.
    pub(crate) fn card(&self, id: u64) -> Option<&CharacterCard> {
        self.cards
            .binary_search_by_key(&id, |card| card.id)
            .ok()
            .map(|at| &self.cards[at])
    }

    /// The relations record of the live character `id` at this revision.
    ///
    /// # Errors
    /// An id that is not live at this revision.
    pub(crate) fn relations_of(&self, id: u64) -> Result<Relations, String> {
        self.relations.record(id)
    }

    /// The live characters of the pop at `place` and `slot` in this revision, most important first.
    pub(crate) fn pop_members(&self, place: &str, slot: u32) -> &[u64] {
        self.pops
            .get(&(place.to_owned(), slot))
            .map_or(&[], Vec::as_slice)
    }

    /// The life log of the live character `id` as of this revision: its lines in date order (the order written for equal dates), the records
    /// written before this publication only, and none dated after the view's day.
    ///
    /// # Errors
    /// An id that is not live at this revision, a log this revision does not carry, or a read of it that failed.
    pub(crate) fn log_of(&self, id: u64) -> Result<Vec<(Date, String)>, String> {
        if self.card(id).is_none() {
            return Err("no character".to_owned());
        }
        let log = self
            .log
            .as_ref()
            .ok_or_else(|| "the life log is not available".to_owned())?;
        let lines = log
            .lines(id)
            .map_err(|error| format!("the life log cannot be read: {error}"))?;
        Ok(lines
            .into_iter()
            .filter_map(|line| Some((line.date.date().ok()?, line.text)))
            .filter(|(date, _)| (date.year, date.month, date.day) <= self.day)
            .collect())
    }

    /// The first tab's values of `id`.
    pub(crate) fn first_tab(&self, id: u64) -> Option<&FirstTab> {
        self.tabs.get(&id)
    }
}

/// What the view reads besides the characters and their pops.
pub(crate) struct Inputs<'a> {
    pub(crate) rules: &'a Rules,
    /// The display name of each place by id.
    pub(crate) places: &'a BTreeMap<String, String>,
    /// The names of the houses the scenario ships.
    pub(crate) houses: &'a Houses,
    /// The place each authored slot is at, by the slot's key: a spouse place is read at the slot's place, not at the pop its holder lives in now.
    pub(crate) slots: &'a BTreeMap<String, String>,
}

/// A value in thousandths in whole units: the nearest, a tie away from zero.
pub(crate) const fn units(thousandths: i64) -> i64 {
    if thousandths >= 0 {
        thousandths.saturating_add(500) / 1000
    } else {
        -(thousandths.saturating_neg().saturating_add(500) / 1000)
    }
}

/// The view of every live character of `store` on `date`.
pub(crate) fn build(
    store: &CharacterStore,
    economy: &EconomyState,
    inputs: &Inputs<'_>,
    date: Date,
) -> CharacterView {
    let mut cards = Vec::new();
    let mut tabs = BTreeMap::new();
    let mut bands: Vec<(u8, ck_sim::GameTime, u64)> = Vec::new();
    for c in store.iter() {
        let age = c.birth.date().map_or(0, |birth| age_on(birth, date));
        let kind = Populations::kind(economy, &c.pop).unwrap_or_default();
        let card = CharacterCard {
            id: c.id,
            name: cycle::derived_name(c, store.get(c.father), &inputs.rules.lifecycle),
            epithet: cycle::epithet_text(c, &inputs.rules.lifecycle).unwrap_or_default(),
            female: c.sex == Sex::Female,
            age,
            house: house_line(store, inputs.houses, &c.life.house),
            role: role_line(store, inputs.places, inputs.slots, c, &kind),
            place: c.pop.place.clone(),
            slot: c.pop.slot,
            clergy: kind == "monk" || c.office == Some(Office::HighCleric),
            ruler: c.office == Some(Office::Ruler),
            child: age < CHILD_UNDER,
            kind,
            importance: 0,
        };
        bands.push((band(store, c), c.birth, c.id));
        tabs.insert(c.id, first_tab(c, inputs.rules));
        cards.push(card);
    }
    // Importance (§2): the bands in their order; within a band, older first, then id.
    bands.sort_unstable();
    for (rank, (_, _, id)) in bands.iter().enumerate() {
        if let Ok(at) = cards.binary_search_by_key(id, |card| card.id) {
            cards[at].importance = u32::try_from(rank).unwrap_or(u32::MAX);
        }
    }
    // The characters of each pop, most important first (the cards are in id order, so the rank decides).
    let mut pops: BTreeMap<(String, u32), Vec<u64>> = BTreeMap::new();
    for card in &cards {
        if let Some(c) = store.get(card.id) {
            pops.entry((c.pop.place.clone(), c.pop.slot))
                .or_default()
                .push(card.id);
        }
    }
    for members in pops.values_mut() {
        members.sort_by_key(|id| {
            cards
                .binary_search_by_key(id, |card| card.id)
                .map_or(u32::MAX, |at| cards[at].importance)
        });
    }
    CharacterView {
        cards,
        tabs,
        pops,
        day: (date.year, date.month, date.day),
        log: None,
        relations: RelationsFacts::default(),
    }
}

/// The importance band of `c` (§2): rulers, high clergy, rulers' spouses, rulers' children, other real figures, slot members, everyone else.
fn band(store: &CharacterStore, c: &Character) -> u8 {
    let living_ruler = |id: u64| {
        store
            .get(id)
            .is_some_and(|other| other.office == Some(Office::Ruler))
    };
    if c.office == Some(Office::Ruler) {
        0
    } else if c.office == Some(Office::HighCleric) {
        1
    } else if c.spouses.iter().any(|spouse| living_ruler(*spouse)) {
        2
    } else if living_ruler(c.father) || living_ruler(c.mother) {
        3
    } else if !c.life.figure.is_empty() {
        4
    } else if !c.life.slot_key.is_empty() {
        5
    } else {
        6
    }
}

/// The house line (§3): `<dynasty> · <name>` or the name of a house `houses.toml` names, the saved name of a generated minor house, and for no
/// house an empty line. A key neither names (not made by this game's data) shows as its words.
fn house_line(store: &CharacterStore, houses: &Houses, key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    if let Some(house) = houses.houses.get(key) {
        return house.line();
    }
    store
        .house_name(key)
        .map_or_else(|| key.replace('_', " "), str::to_owned)
}

/// The role line (§4): the first rule that matches wins. `kind` is the character's pop's kind.
fn role_line(
    store: &CharacterStore,
    places: &BTreeMap<String, String>,
    slots: &BTreeMap<String, String>,
    c: &Character,
    kind: &str,
) -> String {
    let named = |id: &str| places.get(id).map_or(id, String::as_str).to_owned();
    let place = named(&c.pop.place);
    let spouse_word = if c.sex == Sex::Female {
        "wife"
    } else {
        "husband"
    };
    let child_word = if c.sex == Sex::Female {
        "daughter"
    } else {
        "son"
    };
    let slot_kind = c.life.slot_kind.as_str();
    // The role of a slot member: the role field, else what the slot recorded.
    let slot_role = if c.life.slot_key.is_empty() {
        ""
    } else if c.life.role.is_empty() {
        slot_kind
    } else {
        c.life.role.as_str()
    };
    let is_guest = !c.life.slot_key.is_empty() && GUEST_KINDS.contains(&slot_kind);
    let is_spouse_place =
        |word: &str| !c.life.slot_key.is_empty() && (slot_kind == word || slot_role == word);
    let plain_role = |role: &str| role.replace('_', " ");
    // 1 and 2: the offices.
    if c.office == Some(Office::Ruler) {
        return format!("ruler of {place}");
    }
    if c.office == Some(Office::HighCleric) {
        return if slot_role.is_empty() {
            format!("head of the church of {place}")
        } else {
            format!("{} of {place}", plain_role(slot_role))
        };
    }
    // 3: a fosterling, hostage or student, by the slot's kind whatever the role field says.
    if is_guest {
        return format!("{slot_kind} at {place}");
    }
    // 4: a spouse place of a ruler or a lord, at the slot's place: the slot's authored location while it is held, whatever pop the holder has
    // moved to by marrying again (a slot the data does not place is read at the pop).
    let slot_place = slots
        .get(&c.life.slot_key)
        .map_or_else(|| place.clone(), |id| named(id));
    if is_spouse_place("spouse_of_ruler") {
        return format!("{spouse_word} of the ruler of {slot_place}");
    }
    if is_spouse_place("spouse_of_lord") {
        return format!("{spouse_word} of the lord of {slot_place}");
    }
    // 5: any other slot member with a role.
    if !slot_role.is_empty() {
        return format!("{} of {place}", plain_role(slot_role));
    }
    let ruler_place = |id: u64| {
        store
            .get(id)
            .filter(|other| other.office == Some(Office::Ruler))
            .map(|other| named(&other.pop.place))
    };
    // 6: a living ruler's spouse, the first in the spouse list.
    if let Some(ruler) = c.spouses.iter().find_map(|spouse| ruler_place(*spouse)) {
        return format!("{spouse_word} of the ruler of {ruler}");
    }
    // 7: a living ruler's child, the father's place if he is a living ruler, else the mother's.
    if let Some(ruler) = ruler_place(c.father).or_else(|| ruler_place(c.mother)) {
        return format!("{child_word} of the ruler of {ruler}");
    }
    // 8: the pop's kind.
    plain_role(kind)
}

fn first_tab(c: &Character, rules: &Rules) -> FirstTab {
    let genetics = &rules.genetics;
    let mut body = [0_i64; 9];
    for (index, slot) in body.iter_mut().enumerate() {
        *slot = units(c.shown_stat(genetics, index));
    }
    let mut humours = [0_i64; 4];
    for (index, slot) in humours.iter_mut().enumerate() {
        let attribute = ATTRIBUTES.len() - HUMOURS.len() + index;
        *slot = units(c.genetics.shown(genetics, c.sex, attribute)).clamp(0, 10);
    }
    let skills = rules
        .events
        .skills
        .list
        .iter()
        .map(|key| {
            (
                key.clone(),
                units(c.life.skills.get(key).copied().unwrap_or(0)),
            )
        })
        .collect();
    let mut virtue = [0_i64; 7];
    for (slot, value) in virtue.iter_mut().zip(c.personality.vice) {
        *slot = units(value);
    }
    let mut manner = [0_i64; 9];
    for (slot, value) in manner.iter_mut().zip(c.personality.manner) {
        *slot = units(value);
    }
    let traits = rules
        .events
        .catalog_order
        .iter()
        .filter_map(|key| {
            let grade = c.grade(key);
            if grade == 0 {
                return None;
            }
            let entry = rules.events.catalog.get(key)?;
            Some(TraitChip {
                key: key.clone(),
                words: rules
                    .events
                    .catalog_names
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| key.replace('_', " ")),
                attribute: !matches!(entry.kind, Kind::InheritedTrait | Kind::MindTrait),
                letter: if entry.graded {
                    GRADE_NAMES[usize::from(grade).min(GRADE_NAMES.len() - 1)]
                } else {
                    ""
                },
            })
        })
        .collect();
    FirstTab {
        body,
        humours,
        skills,
        virtue,
        stress: units(c.personality.stress),
        threshold: units(threshold(c)),
        manner,
        standing: [
            units(c.personality.popularity),
            units(c.personality.fame),
            units(c.personality.honour),
        ],
        traits,
    }
}
