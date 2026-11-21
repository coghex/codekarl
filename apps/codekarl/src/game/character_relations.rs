//! The relations record (characters slice 9b, CK-377): what the character screen's relations tab shows of one live character, built on request
//! from the facts a published view carries.
//!
//! **What a view carries** ([`RelationsFacts`], §1). The tab must answer from exactly the revision its caller was given: the live characters, the
//! archive and the rules **as of that publication**. So the view holds a copy of the live records, the archive as it was published (an
//! [`ArchiveView`]: a record archived later is not in it, and the old world's files stay readable while any view of them lives), and two
//! reverse indexes built once per publication in one pass over the live characters: the live children of every parent (archived or live), and
//! each court's anchor ruler and guests. A request does lookups by id and by those indexes and nothing else: no archive reopen, no scan of the
//! characters. Everything here is simulation-side code under `game/`, so the build id covers it.
//!
//! **What a record holds** ([`Relations`], §2-§3): the parents, spouses and children, the others row, each shown person with the relation to the
//! character, the formal tie, the strands between them, the opinion each way for the living and the history of the dead, and the passions.

use std::{collections::BTreeMap, sync::Arc};

use ck_characters::{
    ArchiveView, Character, CharacterStore, Departure, Office, PopTargets, Sex, UNKNOWN,
    connections::TARGET_KINDS, cycle, events::age_on, figures::Houses,
};
use ck_economy::{
    EconomyState, Rules,
    events::{ALL_KINDS, BOND_KINDS, PASSION_KINDS},
};
use ck_sim::{Date, GameTime};

use super::character_view::units;

/// The slot kinds of a place held by a person who is not of the household.
const FOSTERLING: &str = "fosterling";
const HOSTAGE: &str = "hostage";
const STUDENT: &str = "student";

/// What a court holds: who is its anchor ruler, and the guests placed at it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Court {
    /// The live character with office ruler whose pop is at the court: the lowest figure key (a real figure before anyone without a key), then
    /// the lowest id.
    anchor: Option<u64>,
    /// The live fosterlings, hostages and students at the court, in id order, with their kinds.
    guests: Vec<(u64, &'static str)>,
}

/// What one publication carries for the relations tab. See the module documentation.
#[derive(Debug)]
struct Inner {
    /// The live characters of the publication. A copy of the records, not of the store: the store carries counters a view must not.
    live: BTreeMap<u64, Character>,
    /// The saved names of the minor houses the live characters and their passions name.
    house_names: BTreeMap<String, String>,
    archive: Option<ArchiveView>,
    rules: Arc<Rules>,
    places: Arc<BTreeMap<String, String>>,
    houses: Arc<Houses>,
    slots: Arc<BTreeMap<String, String>>,
    /// The religion and culture of each live character's pop (for the opinion terms of its passions).
    pops: BTreeMap<u64, PopTargets>,
    /// The date of the publication.
    date: Date,
    /// The year the game started: the year before which an unknown death was.
    start_year: u16,
    /// The live children of each parent, live or archived.
    children: BTreeMap<u64, Vec<u64>>,
    /// The courts, by the id of their place.
    courts: BTreeMap<String, Court>,
}

/// The facts a published view carries for the relations tab; empty until built.
#[derive(Clone, Debug, Default)]
pub(crate) struct RelationsFacts(Option<Arc<Inner>>);

impl PartialEq for RelationsFacts {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}
impl Eq for RelationsFacts {}

/// What building the facts reads besides the store.
pub(crate) struct FactsInputs<'a> {
    pub(crate) rules: &'a Arc<Rules>,
    pub(crate) places: &'a Arc<BTreeMap<String, String>>,
    pub(crate) houses: &'a Arc<Houses>,
    pub(crate) slots: &'a Arc<BTreeMap<String, String>>,
    pub(crate) archive: Option<ArchiveView>,
    pub(crate) start_year: u16,
}

impl RelationsFacts {
    /// The facts of `store` on `date`: one pass over the live characters.
    pub(crate) fn build(
        store: &CharacterStore,
        economy: &EconomyState,
        inputs: &FactsInputs<'_>,
        date: Date,
    ) -> Self {
        let mut children: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
        let mut courts: BTreeMap<String, Court> = BTreeMap::new();
        let mut pops = BTreeMap::new();
        let mut live = BTreeMap::new();
        let mut house_names = BTreeMap::new();
        let mut name_house = |key: &str| {
            if let Some(name) = store.house_name(key) {
                house_names.insert(key.to_owned(), name.to_owned());
            }
        };
        for c in store.iter() {
            live.insert(c.id, c.clone());
            name_house(&c.life.house);
            for passion in &c.ties.passions {
                if TARGET_KINDS.get(usize::from(passion.target.kind)) == Some(&"house") {
                    name_house(&passion.target.key);
                }
            }
            for parent in [c.father, c.mother] {
                if parent != UNKNOWN {
                    children.entry(parent).or_default().push(c.id);
                }
            }
            pops.insert(c.id, PopTargets::of(economy, c));
            if c.office == Some(Office::Ruler) {
                let court = courts.entry(c.pop.place.clone()).or_default();
                let better = court.anchor.is_none_or(|held| {
                    store
                        .get(held)
                        .is_none_or(|held| anchor_order(c) < anchor_order(held))
                });
                if better {
                    court.anchor = Some(c.id);
                }
            }
            if !c.life.slot_key.is_empty() {
                let kind = match c.life.slot_kind.as_str() {
                    FOSTERLING => FOSTERLING,
                    HOSTAGE => HOSTAGE,
                    STUDENT => STUDENT,
                    _ => continue,
                };
                let place = inputs
                    .slots
                    .get(&c.life.slot_key)
                    .unwrap_or(&c.pop.place)
                    .clone();
                courts.entry(place).or_default().guests.push((c.id, kind));
            }
        }
        Self(Some(Arc::new(Inner {
            live,
            house_names,
            archive: inputs.archive.clone(),
            rules: Arc::clone(inputs.rules),
            places: Arc::clone(inputs.places),
            houses: Arc::clone(inputs.houses),
            slots: Arc::clone(inputs.slots),
            pops,
            date,
            start_year: inputs.start_year,
            children,
            courts,
        })))
    }

    /// The archive this publication reads, for tests.
    #[cfg(test)]
    pub(crate) fn archive_for_tests(&self) -> Option<&ArchiveView> {
        self.0.as_deref()?.archive.as_ref()
    }

    /// The relations record of the live character `id`, or the refusal.
    ///
    /// # Errors
    /// An id that is not live in this view, or a view built without facts.
    pub(crate) fn record(&self, id: u64) -> Result<Relations, String> {
        let inner = self
            .0
            .as_deref()
            .ok_or_else(|| "the view holds no relations".to_owned())?;
        let subject = inner
            .live
            .get(&id)
            .ok_or_else(|| format!("no character {id}"))?;
        Ok(inner.relations(subject))
    }
}

/// The order of anchors: a real figure (a key) before anyone without one, then by key, then by id.
fn anchor_order(c: &Character) -> (bool, &str, u64) {
    (c.life.figure.is_empty(), c.life.figure.as_str(), c.id)
}

/// How a person ended, for the dead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum End {
    /// Alive: it has an age.
    Alive,
    /// Died on a date.
    Died(Date),
    /// Died before the game's start, the date unknown.
    DiedBefore(u16),
    /// Abroad: alive or dead unknown to the game.
    Abroad,
}

/// The formal tie between the character and a shown person (§2: the icon under the portrait).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tie {
    /// No formal tie: a friend, a rival or a lover.
    None,
    /// Kin.
    Kin,
    /// Foster kin.
    Foster,
    /// A hostage and its holder.
    Hostage,
}

impl Tie {
    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Kin => "kin",
            Self::Foster => "foster",
            Self::Hostage => "hostage",
        }
    }
}

/// Whose a strand line is: a bond both hold, the character's own feeling, or the other's feeling toward the character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Both,
    Mine,
    Theirs,
}

impl Side {
    pub(crate) const fn word(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::Mine => "mine",
            Self::Theirs => "theirs",
        }
    }
}

/// One strand between the character and a shown person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    /// The kind: one of `ck_economy::events::ALL_KINDS`.
    pub(crate) kind: &'static str,
    pub(crate) strength: u8,
    pub(crate) side: Side,
    /// Whether it is history: read from a dead person's record, not a live bond.
    pub(crate) history: bool,
}

/// One person shown on the tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Person {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) epithet: String,
    pub(crate) female: bool,
    /// Age in whole years, for a live person.
    pub(crate) age: u32,
    pub(crate) end: End,
    /// The relation to the character (`son`, `foster-brother`, `rival`).
    pub(crate) relation: String,
    pub(crate) tie: Tie,
    /// Every strand between them, in the tooltip's order: bonds, then the character's feelings, then the other's.
    pub(crate) lines: Vec<Line>,
    /// The character's opinion of this person and this person's of the character, in whole points; for a live person only.
    pub(crate) opinion: Option<(i64, i64)>,
}

impl Person {
    pub(crate) const fn live(&self) -> bool {
        matches!(self.end, End::Alive)
    }
}

/// What a passion is aimed at, with its name where the simulation can give it (a place or a house; a culture or a religion is named by the UI
/// from its glossary term).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PassionLine {
    pub(crate) kind: &'static str,
    pub(crate) strength: u8,
    pub(crate) target_kind: &'static str,
    pub(crate) key: String,
    pub(crate) name: String,
}

/// The relations tab of one live character.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Relations {
    /// The father, then the mother; `None` for the unknown.
    pub(crate) parents: [Option<Person>; 2],
    /// Whether the spouses are wives (the character is a man) or husbands.
    pub(crate) spouses_are_wives: bool,
    pub(crate) spouses: Vec<Person>,
    pub(crate) children: Vec<Person>,
    pub(crate) others: Vec<Person>,
    pub(crate) passions: Vec<PassionLine>,
}

/// A record found live or in the archive, as a borrowed or owned character.
enum Found<'a> {
    Live(&'a Character),
    Archived(Box<Character>),
}

impl Found<'_> {
    fn get(&self) -> &Character {
        match self {
            Found::Live(c) => c,
            Found::Archived(c) => c,
        }
    }
    const fn is_live(&self) -> bool {
        matches!(self, Found::Live(_))
    }
}

impl Inner {
    fn live_get(&self, id: u64) -> Option<&Character> {
        self.live.get(&id)
    }

    fn find(&self, id: u64) -> Option<Found<'_>> {
        if id == UNKNOWN {
            return None;
        }
        if let Some(c) = self.live_get(id) {
            return Some(Found::Live(c));
        }
        self.archive
            .as_ref()?
            .get(id)
            .ok()
            .flatten()
            .map(|c| Found::Archived(Box::new(c)))
    }

    /// Every child of `parent`, live and archived, by the indexes: the live characters naming it, the archive's by-parent index, and what its
    /// own record lists.
    fn child_ids(&self, parent: u64) -> Vec<u64> {
        let mut ids: Vec<u64> = self.children.get(&parent).cloned().unwrap_or_default();
        if let Some(archive) = &self.archive {
            ids.extend(archive.children_of(parent));
        }
        if let Some(c) = self.live_get(parent) {
            ids.extend(c.children.iter().copied().filter(|id| self.exists(*id)));
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    fn exists(&self, id: u64) -> bool {
        id != UNKNOWN
            && (self.live_get(id).is_some()
                || self.archive.as_ref().is_some_and(|a| a.contains(id)))
    }

    /// `ids` ordered oldest first, then by id.
    fn oldest_first(&self, ids: impl IntoIterator<Item = u64>) -> Vec<u64> {
        let mut keyed: Vec<_> = ids
            .into_iter()
            .filter_map(|id| Some((self.find(id)?.get().birth, id)))
            .collect();
        keyed.sort_unstable();
        keyed.dedup();
        keyed.into_iter().map(|(_, id)| id).collect()
    }

    /// The siblings of `of` (every other child of either parent, half-siblings included), live or dead.
    fn sibling_ids(&self, of: &Character) -> Vec<u64> {
        let mut ids = Vec::new();
        for parent in [of.father, of.mother] {
            if parent != UNKNOWN {
                ids.extend(self.child_ids(parent));
            }
        }
        ids.retain(|id| *id != of.id);
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    fn person(&self, subject: &Character, id: u64, relation: &str, tie: Tie) -> Option<Person> {
        let found = self.find(id)?;
        let c = found.get();
        let female = c.sex == Sex::Female;
        let lifecycle = &self.rules.lifecycle;
        let mut person = Person {
            id,
            name: String::new(),
            epithet: cycle::epithet_text(c, lifecycle).unwrap_or_default(),
            female,
            age: 0,
            end: End::Alive,
            relation: relation.to_owned(),
            tie,
            lines: Vec::new(),
            opinion: None,
        };
        if found.is_live() {
            person.name = cycle::derived_name(c, self.live_get(c.father), lifecycle);
            person.age = c.birth.date().map_or(0, |birth| age_on(birth, self.date));
            person.lines = live_lines(subject, c);
            let own_pops = self.pops.get(&subject.id).cloned().unwrap_or_default();
            let their_pops = self.pops.get(&id).cloned().unwrap_or_default();
            let personality = &self.rules.personality;
            let connections = &self.rules.lifecycle.connections;
            let mine = ck_characters::opinion(personality, connections, subject, c, &their_pops)
                .map_or(0, units);
            let back = ck_characters::opinion(personality, connections, c, subject, &own_pops)
                .map_or(0, units);
            person.opinion = Some((mine, back));
        } else {
            c.name.clone_into(&mut person.name);
            person.end = match (c.death, c.life.departure) {
                (_, Departure::Abroad) => End::Abroad,
                (_, Departure::DeathUnknown) | (None, Departure::None) => {
                    End::DiedBefore(self.start_year)
                }
                (Some(death), _) => death
                    .date()
                    .map_or(End::DiedBefore(self.start_year), End::Died),
            };
            person.lines = history_lines(subject, c);
        }
        Some(person)
    }

    fn relations(&self, subject: &Character) -> Relations {
        let mut shown: Vec<u64> = vec![subject.id];
        let mut record = Relations {
            spouses_are_wives: subject.sex == Sex::Male,
            ..Relations::default()
        };
        let sex_word = |c: Option<Found<'_>>, male: &'static str, female: &'static str| {
            c.map_or(male, |found| {
                if found.get().sex == Sex::Female {
                    female
                } else {
                    male
                }
            })
        };
        // The top line: father, then mother; then the spouses in the stored order, live and dead.
        for (slot, (parent, word)) in [(subject.father, "father"), (subject.mother, "mother")]
            .into_iter()
            .enumerate()
        {
            if let Some(person) = self.person(subject, parent, word, Tie::Kin) {
                shown.push(parent);
                record.parents[slot] = Some(person);
            }
        }
        for spouse in &subject.spouses {
            let word = sex_word(self.find(*spouse), "husband", "wife");
            if let Some(mut person) = self.person(subject, *spouse, word, Tie::Kin) {
                // A spouse with no strand shows no icon: a cold marriage is an empty space (§2), so it carries no tie either.
                if !person
                    .lines
                    .iter()
                    .any(|line| matches!(line.side, Side::Both | Side::Mine))
                {
                    person.tie = Tie::None;
                }
                shown.push(*spouse);
                record.spouses.push(person);
            }
        }
        // Children, oldest first, live and dead.
        let child_ids = self.oldest_first(self.child_ids(subject.id));
        for child in &child_ids {
            let word = sex_word(self.find(*child), "son", "daughter");
            if let Some(person) = self.person(subject, *child, word, Tie::Kin) {
                shown.push(*child);
                record.children.push(person);
            }
        }
        // The others row, in the designer's order; a person shown before is not repeated. A person who is also kin or foster kin carries that
        // tie and word whichever group it is listed in first: a brother who is also a rival is a brother.
        let foster = self.foster_kin(subject);
        let kin = self.living_kin(subject, &child_ids);
        let formal = |id: u64| {
            foster
                .iter()
                .chain(kin.iter())
                .find(|(other, _, _)| *other == id)
        };
        let mut others: Vec<(u64, String, Tie)> = Vec::new();
        for kind in ["lover", "rival", "friend"] {
            for id in self.partners_of(subject, kind) {
                others.push(match formal(id) {
                    Some((_, word, tie)) => (id, word.clone(), *tie),
                    None => (id, kind.to_owned(), Tie::None),
                });
            }
        }
        others.extend(foster.iter().cloned());
        others.extend(kin.iter().cloned());
        for (id, word, tie) in others {
            if shown.contains(&id) {
                continue;
            }
            if let Some(person) = self.person(subject, id, &word, tie) {
                shown.push(id);
                record.others.push(person);
            }
        }
        record.passions = self.passions(subject);
        record
    }

    /// The live partners `subject` holds a strand of `kind` (a bond kind) with, strongest first, then by id.
    fn partners_of(&self, subject: &Character, kind: &str) -> Vec<u64> {
        let Some(at) = ALL_KINDS.iter().position(|name| *name == kind) else {
            return Vec::new();
        };
        let mut found: Vec<(u8, u64)> = subject
            .ties
            .links
            .iter()
            .filter(|(id, link)| link.strands[at].strength > 0 && self.live_get(**id).is_some())
            .map(|(id, link)| (link.strands[at].strength, *id))
            .collect();
        found.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        found.into_iter().map(|(_, id)| id).collect()
    }

    /// The foster kin and hostages (designer ruling): by the court's anchor ruler, in both directions.
    fn foster_kin(&self, subject: &Character) -> Vec<(u64, String, Tie)> {
        let mut out: Vec<(u64, String, Tie)> = Vec::new();
        let word = |id: u64, male: &str, female: &str| -> String {
            match self.live_get(id) {
                Some(c) if c.sex == Sex::Female => female.to_owned(),
                _ => male.to_owned(),
            }
        };
        let court_of = |c: &Character| -> String {
            self.slots
                .get(&c.life.slot_key)
                .unwrap_or(&c.pop.place)
                .clone()
        };
        let anchor_at = |place: &str| self.courts.get(place).and_then(|court| court.anchor);
        let first_living_spouse = |ruler: &Character| {
            ruler
                .spouses
                .iter()
                .copied()
                .find(|spouse| self.live_get(*spouse).is_some())
        };
        // 1. The subject is placed as a fosterling or a hostage at a court.
        if !subject.life.slot_key.is_empty() {
            let court = court_of(subject);
            if let Some(anchor) = anchor_at(&court).and_then(|id| self.live_get(id)) {
                match subject.life.slot_kind.as_str() {
                    FOSTERLING => {
                        out.push((anchor.id, "foster-father".to_owned(), Tie::Foster));
                        if let Some(wife) = first_living_spouse(anchor) {
                            out.push((wife, "foster-mother".to_owned(), Tie::Foster));
                        }
                        for child in self.oldest_first(self.child_ids(anchor.id)) {
                            if child != subject.id
                                && self.live.get(&child).is_some_and(|c| c.pop.place == court)
                            {
                                out.push((
                                    child,
                                    word(child, "foster-brother", "foster-sister"),
                                    Tie::Foster,
                                ));
                            }
                        }
                    }
                    HOSTAGE => out.push((anchor.id, "holder".to_owned(), Tie::Hostage)),
                    _ => {}
                }
            }
        }
        // 2. The subject is a court's anchor, its anchor's first living wife, or a child of the anchor at the court: it sees the court's guests.
        let hosts_of = |place: &str| -> Vec<(u64, &'static str)> {
            self.courts
                .get(place)
                .map(|court| court.guests.clone())
                .unwrap_or_default()
        };
        let guest_lines = |guests: Vec<(u64, &'static str)>, hostages: bool, brother: bool| {
            let mut lines = Vec::new();
            for (id, kind) in guests {
                if id == subject.id {
                    continue;
                }
                match kind {
                    FOSTERLING => {
                        let (male, female) = if brother {
                            ("foster-brother", "foster-sister")
                        } else {
                            ("foster-son", "foster-daughter")
                        };
                        lines.push((id, word(id, male, female), Tie::Foster));
                    }
                    HOSTAGE if hostages => lines.push((id, "hostage".to_owned(), Tie::Hostage)),
                    _ => {}
                }
            }
            lines
        };
        let own = &subject.pop.place;
        if anchor_at(own) == Some(subject.id) {
            out.extend(guest_lines(hosts_of(own), true, false));
        }
        for spouse in &subject.spouses {
            if let Some(ruler) = self.live_get(*spouse)
                && anchor_at(&ruler.pop.place) == Some(ruler.id)
                && first_living_spouse(ruler) == Some(subject.id)
            {
                out.extend(guest_lines(hosts_of(&ruler.pop.place), false, false));
            }
        }
        for parent in [subject.father, subject.mother] {
            if let Some(ruler) = self.live_get(parent)
                && ruler.pop.place == *own
                && anchor_at(own) == Some(ruler.id)
            {
                out.extend(guest_lines(hosts_of(own), false, true));
            }
        }
        self.order_foster(&mut out);
        out
    }

    /// §2: foster kin (the foster-father, the foster-mother, the foster-sons and daughters, then the foster-brothers and sisters), then the
    /// hostages held (or the holder); each group oldest first, then by id, and a person once.
    fn order_foster(&self, out: &mut Vec<(u64, String, Tie)>) {
        let rank = |entry: &(u64, String, Tie)| -> (u8, u8, GameTime, u64) {
            let birth = self
                .live_get(entry.0)
                .map_or_else(|| GameTime::from_hours(0), |c| c.birth);
            let (group, kind) = match entry.1.as_str() {
                "foster-father" => (0, 0),
                "foster-mother" => (0, 1),
                "foster-son" | "foster-daughter" => (0, 2),
                "foster-brother" | "foster-sister" => (0, 3),
                _ => (1, 0),
            };
            (group, kind, birth, entry.0)
        };
        out.sort_by_key(rank);
        out.dedup_by_key(|entry| entry.0);
    }

    /// The other living kin, in the designer's order: siblings, grandchildren, nephews and nieces, then in-laws; each group oldest first.
    fn living_kin(&self, subject: &Character, child_ids: &[u64]) -> Vec<(u64, String, Tie)> {
        let live = |id: &u64| self.live_get(*id).is_some();
        let word = |id: u64, male: &str, female: &str| -> String {
            match self.live_get(id) {
                Some(c) if c.sex == Sex::Female => female.to_owned(),
                _ => male.to_owned(),
            }
        };
        let mut out = Vec::new();
        let siblings = self.sibling_ids(subject);
        for id in self.oldest_first(siblings.iter().copied().filter(live)) {
            out.push((id, word(id, "brother", "sister"), Tie::Kin));
        }
        let grandchildren: Vec<u64> = child_ids
            .iter()
            .flat_map(|child| self.child_ids(*child))
            .filter(live)
            .collect();
        for id in self.oldest_first(grandchildren) {
            out.push((id, word(id, "grandson", "granddaughter"), Tie::Kin));
        }
        let nephews: Vec<u64> = siblings
            .iter()
            .flat_map(|sibling| self.child_ids(*sibling))
            .filter(live)
            .collect();
        for id in self.oldest_first(nephews) {
            out.push((id, word(id, "nephew", "niece"), Tie::Kin));
        }
        // The in-laws are one group, oldest first: the spouses' parents and siblings together, each by its own word.
        let mut in_laws: BTreeMap<u64, String> = BTreeMap::new();
        for spouse in &subject.spouses {
            let Some(found) = self.find(*spouse) else {
                continue;
            };
            let spouse = found.get();
            for id in [spouse.father, spouse.mother].into_iter().filter(live) {
                in_laws
                    .entry(id)
                    .or_insert_with(|| word(id, "father-in-law", "mother-in-law"));
            }
            for id in self.sibling_ids(spouse).into_iter().filter(live) {
                in_laws
                    .entry(id)
                    .or_insert_with(|| word(id, "brother-in-law", "sister-in-law"));
            }
        }
        for id in self.oldest_first(in_laws.keys().copied()) {
            out.push((id, in_laws[&id].clone(), Tie::Kin));
        }
        out
    }

    fn passions(&self, subject: &Character) -> Vec<PassionLine> {
        let mut lines: Vec<(u8, usize, &'static str, String, PassionLine)> = Vec::new();
        for passion in &subject.ties.passions {
            let kind = PASSION_KINDS
                .get(usize::from(passion.kind))
                .copied()
                .unwrap_or("");
            let target_kind = TARGET_KINDS
                .get(usize::from(passion.target.kind))
                .copied()
                .unwrap_or("");
            let key = passion.target.key.clone();
            let name = match target_kind {
                "place" => self
                    .places
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| key.clone()),
                "house" => self
                    .houses
                    .houses
                    .get(&key)
                    .map(|house| house.name.clone())
                    .or_else(|| self.house_names.get(&key).cloned())
                    .unwrap_or_else(|| key.clone()),
                _ => String::new(),
            };
            lines.push((
                u8::MAX - passion.strength,
                usize::from(passion.kind),
                target_kind,
                key.clone(),
                PassionLine {
                    kind,
                    strength: passion.strength,
                    target_kind,
                    key,
                    name,
                },
            ));
        }
        // Strongest first, then by kind and target.
        lines.sort_by(|a, b| (a.0, a.1, a.2, &a.3).cmp(&(b.0, b.1, b.2, &b.3)));
        lines.into_iter().map(|line| line.4).collect()
    }
}

/// The strands between two live characters, in the tooltip's order: the bonds, then `subject`'s feelings toward `other`, then `other`'s toward
/// `subject`.
fn live_lines(subject: &Character, other: &Character) -> Vec<Line> {
    let mut lines = Vec::new();
    let mine = subject.ties.links.get(&other.id);
    let theirs = other.ties.links.get(&subject.id);
    for (index, kind) in ALL_KINDS.iter().enumerate() {
        if BOND_KINDS.contains(kind) {
            let strength = mine.map_or(0, |link| link.strands[index].strength);
            if strength > 0 {
                lines.push(Line {
                    kind,
                    strength,
                    side: Side::Both,
                    history: false,
                });
            }
        }
    }
    for (index, kind) in ALL_KINDS.iter().enumerate() {
        if !BOND_KINDS.contains(kind) {
            let strength = mine.map_or(0, |link| link.strands[index].strength);
            if strength > 0 {
                lines.push(Line {
                    kind,
                    strength,
                    side: Side::Mine,
                    history: false,
                });
            }
        }
    }
    for (index, kind) in ALL_KINDS.iter().enumerate() {
        if !BOND_KINDS.contains(kind) {
            let strength = theirs.map_or(0, |link| link.strands[index].strength);
            if strength > 0 {
                lines.push(Line {
                    kind,
                    strength,
                    side: Side::Theirs,
                    history: false,
                });
            }
        }
    }
    lines
}

/// The ties a **dead** `other` holds toward `subject` on its own record (history): its bonds and its own feelings, and what the start stored of
/// the feelings `subject` held toward it. No living partner's feelings toward it are read: they are gone with the death.
fn history_lines(subject: &Character, other: &Character) -> Vec<Line> {
    let mut lines = Vec::new();
    if let Some(link) = other.ties.links.get(&subject.id) {
        for (index, kind) in ALL_KINDS.iter().enumerate() {
            if BOND_KINDS.contains(kind) && link.strands[index].strength > 0 {
                lines.push(Line {
                    kind,
                    strength: link.strands[index].strength,
                    side: Side::Both,
                    history: true,
                });
            }
        }
    }
    for inbound in &other.ties.inbound {
        if inbound.holder == subject.id
            && let Some(kind) = ALL_KINDS.get(usize::from(inbound.kind))
        {
            lines.push(Line {
                kind,
                strength: inbound.strength,
                side: Side::Mine,
                history: true,
            });
        }
    }
    if let Some(link) = other.ties.links.get(&subject.id) {
        for (index, kind) in ALL_KINDS.iter().enumerate() {
            if !BOND_KINDS.contains(kind) && link.strands[index].strength > 0 {
                lines.push(Line {
                    kind,
                    strength: link.strands[index].strength,
                    side: Side::Theirs,
                    history: true,
                });
            }
        }
    }
    lines
}
