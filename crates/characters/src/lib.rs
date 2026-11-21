//! Backend-neutral live characters and their derived tiers.
#![deny(unsafe_code)]

use ck_economy::{HeadUndo, MoveUndo, NearbyPlace, PopGenetics};
use ck_sim::{Canonical, CanonicalError, Date, GameTime, Reader, Stream};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

pub mod body;
pub mod connections;
pub mod cycle;
pub mod epithets;
pub mod events;
mod fixtures;
pub mod genetics;
pub mod life;
pub mod marriage;
pub(crate) mod names;
pub mod personality;
pub mod skills;
pub mod ties;
pub use connections::{PopTargets, opinion, top_up_chance_ppb};
pub use events::{EventError, LogSink, World};
pub use genetics::{Context, GeneticRecord, Mendel};
pub use life::LifeRecord;
pub use personality::{
    EventKind, LevelTrait, PersonalityRecord, Standing, StressKind, personality_opinion, threshold,
};
pub use ties::{Orientation, Passion, PassionTarget, Strand, Ties};

/// The placeholder for an ungenerated ancestor. It is never a live character.
pub const UNKNOWN: u64 = 0;
/// Portrait key reserved for the unknown character.
pub const UNKNOWN_PORTRAIT_KEY: &str = "unknown";
/// Years for which a character stays full after losing its last qualification.
pub const FULL_GRACE_YEARS: u16 = 5;
/// Stable location of a pop: economy slots must only be appended, never removed or reordered.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PopLocator {
    /// Economy place identifier.
    pub place: String,
    /// Index within the place's pop slots.
    pub slot: u32,
}
/// Read population sizes by stable location.
pub trait Populations {
    /// Return the current head count, or `None` when the place or slot does not exist.
    fn heads(&self, locator: &PopLocator) -> Option<u64>;
    /// Return the culture key of the pop, or `None` when the place or slot does not exist. A character is created with the
    /// culture of the pop it is created into (CK-362).
    fn culture(&self, locator: &PopLocator) -> Option<String>;
    /// Return the pop type key of the pop (`noble`, `peasant`, ...), or `None` when it is unknown. A character's focus of
    /// education is drawn from the odds of its pop type; a pop whose type is unknown gives a balanced one, and the event engine
    /// refuses a character whose pop type it cannot resolve (CK-363).
    fn kind(&self, _locator: &PopLocator) -> Option<String> {
        None
    }
    /// Return the pop's luxuries fulfilment in permille, or `None` when it is unknown (CK-363, `pop_luxuries_min`).
    fn luxuries_pm(&self, _locator: &PopLocator) -> Option<u16> {
        None
    }
    /// Return the display name of a passion's target (CK-368 §8: the `{target}` of a log line): `kind` is `place`, `religion`, `house` or
    /// `culture` and `key` the canonical key that state and opinion keep. `None` shows the key itself. The economy holds no display names,
    /// so a source with names (the UI's) overrides this; a source that wraps another should forward it.
    fn target_name(&self, _kind: &str, _key: &str) -> Option<String> {
        None
    }
    /// Return the religion key of the pop, or `None` when it is unknown (CK-364: the Latin church's days of abstinence).
    fn religion(&self, _locator: &PopLocator) -> Option<String> {
        None
    }
    /// Return the pop's necessities fulfilment in permille, or `None` when it is unknown (CK-364: a couple avoids conception
    /// when its household's necessities are low).
    fn necessities_pm(&self, _locator: &PopLocator) -> Option<u16> {
        None
    }
    /// Return the pop's genetic state, or `None` when it is unknown (CK-364: a child's genetics need its parents' pop only for a
    /// synthetic parent, but a delivery is refused without it).
    fn genetics(&self, _locator: &PopLocator) -> Option<PopGenetics> {
        None
    }
    /// Add `count` dependents to the pop for character births, counted as pending births (CK-364). A source with no ledger has
    /// nothing to change. Nothing changes on an error.
    ///
    /// # Errors
    /// Refuses a missing pop or a count that overflows.
    fn add_births(&self, _locator: &PopLocator, _count: u64) -> Result<(), Error> {
        Ok(())
    }
    /// Remove the head of a dying character from the pop, counted as a pending death (CK-364): see
    /// [`ck_economy::EconomyState::remove_character_head`]. A source with no ledger has nothing to change. Nothing changes on an
    /// error.
    ///
    /// # Errors
    /// Refuses a missing pop or one with no head to remove.
    fn remove_head(&self, _locator: &PopLocator, _minor: bool) -> Result<HeadUndo, Error> {
        Ok(HeadUndo::default())
    }
    /// Put back a head removed by [`remove_head`](Self::remove_head) whose death did not commit (CK-364). A source with no ledger has
    /// nothing to put back.
    fn restore_head(&self, _undo: &HeadUndo) {}
    /// Return the distance in whole metres between the places of two pops (CK-366: the marriage pool), 0 for pops of one place, or
    /// `None` when either is unknown or the places are beyond what the source measures. There is no default: a source that wraps
    /// another must forward the query, so that it cannot silently report every place as out of reach.
    fn distance_m(&self, a: &PopLocator, b: &PopLocator) -> Option<u64>;
    /// Move the head of a married character from the pop `from` to the pop `to`, in the same cohort, blending the destination's
    /// genetics with one head of the source's: see [`ck_economy::EconomyState::move_character_head`]. A source that cannot write
    /// the pops (a shared reference to the economy, a bare function) **refuses**: a marriage between two pops moves a head, and one
    /// that did not would leave the pops' counts and genetics wrong. Nothing changes on an error. There is no default: a source that
    /// wraps another must forward the move.
    ///
    /// # Errors
    /// Refuses a missing pop, one with no head to take, a source that cannot write the pops, or a move the economy cannot make.
    fn move_head(&self, from: &PopLocator, to: &PopLocator, minor: bool)
    -> Result<MoveUndo, Error>;
    /// Put back a move made by [`move_head`](Self::move_head) whose marriage did not commit (CK-366). A source with no ledger has
    /// nothing to put back. There is no default.
    fn restore_move(&self, undo: &MoveUndo);
}
/// The economy's pops as the passes use them: reads and, through a [`Ledger`], head changes.
impl Populations for ck_economy::EconomyState {
    fn heads(&self, locator: &PopLocator) -> Option<u64> {
        let place = self.places.iter().find(|place| place.id == locator.place)?;
        let pop = place.pops.get(locator.slot as usize)?;
        pop.workers.checked_add(pop.dependents)
    }
    fn culture(&self, locator: &PopLocator) -> Option<String> {
        let place = self.places.iter().find(|place| place.id == locator.place)?;
        Some(place.pops.get(locator.slot as usize)?.culture.clone())
    }
    fn kind(&self, locator: &PopLocator) -> Option<String> {
        let place = self.places.iter().find(|place| place.id == locator.place)?;
        Some(place.pops.get(locator.slot as usize)?.kind.clone())
    }
    fn luxuries_pm(&self, locator: &PopLocator) -> Option<u16> {
        let place = self.places.iter().find(|place| place.id == locator.place)?;
        Some(
            place
                .pops
                .get(locator.slot as usize)?
                .consumption
                .luxuries_pm,
        )
    }
    fn religion(&self, locator: &PopLocator) -> Option<String> {
        let place = self.places.iter().find(|place| place.id == locator.place)?;
        Some(place.pops.get(locator.slot as usize)?.religion.clone())
    }
    fn necessities_pm(&self, locator: &PopLocator) -> Option<u16> {
        let place = self.places.iter().find(|place| place.id == locator.place)?;
        Some(
            place
                .pops
                .get(locator.slot as usize)?
                .consumption
                .necessities_pm,
        )
    }
    fn genetics(&self, locator: &PopLocator) -> Option<PopGenetics> {
        pop_genetics(self, locator).cloned()
    }
    /// The economy holds no map positions: only pops of one place have a known distance (0).
    fn distance_m(&self, a: &PopLocator, b: &PopLocator) -> Option<u64> {
        (a.place == b.place
            && Populations::heads(self, a).is_some()
            && Populations::heads(self, b).is_some())
        .then_some(0)
    }
    /// A shared reference cannot change a pop: a head cannot be moved through it, so the move is refused.
    fn move_head(
        &self,
        _from: &PopLocator,
        _to: &PopLocator,
        _minor: bool,
    ) -> Result<MoveUndo, Error> {
        Err(Error::Lifecycle)
    }
    fn restore_move(&self, _undo: &MoveUndo) {}
}
/// The economy with its head changes enabled: the passes read the pops through [`Populations`] and change their heads through the
/// same shared reference, so the economy sits behind a cell for the length of a pass.
#[derive(Debug)]
pub struct Ledger<'a> {
    economy: RefCell<&'a mut ck_economy::EconomyState>,
    /// The places within reach of each place, nearest first, for the marriage pool (CK-366); none known without a map.
    nearby: Option<&'a BTreeMap<String, Vec<NearbyPlace>>>,
    /// The display names of places by id, for the log lines that name a place (CK-368); none known without a map.
    place_names: Option<&'a BTreeMap<String, String>>,
}
impl<'a> Ledger<'a> {
    /// Lend the economy to a pass. Without a map, only pops of one place are a known distance apart.
    pub fn new(economy: &'a mut ck_economy::EconomyState) -> Self {
        Self {
            economy: RefCell::new(economy),
            nearby: None,
            place_names: None,
        }
    }
    /// Lend the economy to a pass together with the places within reach of each place and their distances in ground metres.
    pub fn with_nearby(
        economy: &'a mut ck_economy::EconomyState,
        nearby: &'a BTreeMap<String, Vec<NearbyPlace>>,
    ) -> Self {
        Self {
            economy: RefCell::new(economy),
            nearby: Some(nearby),
            place_names: None,
        }
    }
    /// Give the pass the display names of places by id, which the log lines that name a place show.
    #[must_use]
    pub fn with_place_names(mut self, names: &'a BTreeMap<String, String>) -> Self {
        self.place_names = Some(names);
        self
    }
}
impl Populations for Ledger<'_> {
    fn target_name(&self, kind: &str, key: &str) -> Option<String> {
        if kind == "place"
            && let Some(name) = self.place_names.and_then(|names| names.get(key))
        {
            return Some(name.clone());
        }
        Populations::target_name(&**self.economy.borrow(), kind, key)
    }
    fn heads(&self, locator: &PopLocator) -> Option<u64> {
        Populations::heads(&**self.economy.borrow(), locator)
    }
    fn culture(&self, locator: &PopLocator) -> Option<String> {
        Populations::culture(&**self.economy.borrow(), locator)
    }
    fn kind(&self, locator: &PopLocator) -> Option<String> {
        Populations::kind(&**self.economy.borrow(), locator)
    }
    fn luxuries_pm(&self, locator: &PopLocator) -> Option<u16> {
        Populations::luxuries_pm(&**self.economy.borrow(), locator)
    }
    fn religion(&self, locator: &PopLocator) -> Option<String> {
        Populations::religion(&**self.economy.borrow(), locator)
    }
    fn necessities_pm(&self, locator: &PopLocator) -> Option<u16> {
        Populations::necessities_pm(&**self.economy.borrow(), locator)
    }
    fn genetics(&self, locator: &PopLocator) -> Option<PopGenetics> {
        Populations::genetics(&**self.economy.borrow(), locator)
    }
    fn add_births(&self, locator: &PopLocator, count: u64) -> Result<(), Error> {
        self.economy
            .borrow_mut()
            .add_character_births(&locator.place, locator.slot as usize, count)
            .map_err(|_| Error::MissingPop)
    }
    fn remove_head(&self, locator: &PopLocator, minor: bool) -> Result<HeadUndo, Error> {
        self.economy
            .borrow_mut()
            .remove_character_head(&locator.place, locator.slot as usize, minor)
            .map_err(|_| Error::MissingPop)
    }
    fn restore_head(&self, undo: &HeadUndo) {
        self.economy.borrow_mut().restore_character_head(undo);
    }
    fn distance_m(&self, a: &PopLocator, b: &PopLocator) -> Option<u64> {
        if a.place == b.place {
            return Populations::distance_m(&**self.economy.borrow(), a, b);
        }
        let heads = |locator: &PopLocator| Populations::heads(&**self.economy.borrow(), locator);
        heads(a)?;
        heads(b)?;
        self.nearby?
            .get(&a.place)?
            .iter()
            .find(|place| place.id == b.place)
            .map(|place| u64::from(place.distance_m))
    }
    fn move_head(
        &self,
        from: &PopLocator,
        to: &PopLocator,
        minor: bool,
    ) -> Result<MoveUndo, Error> {
        self.economy
            .borrow_mut()
            .move_character_head(
                (&from.place, from.slot as usize),
                (&to.place, to.slot as usize),
                minor,
            )
            .map_err(|_| Error::MissingPop)
    }
    fn restore_move(&self, undo: &MoveUndo) {
        self.economy.borrow_mut().restore_character_move(undo);
    }
}
/// A bare head-count function knows no cultures, so a character cannot be created into its pops.
impl<F: Fn(&PopLocator) -> Option<u64>> Populations for F {
    fn heads(&self, locator: &PopLocator) -> Option<u64> {
        self(locator)
    }
    fn culture(&self, _locator: &PopLocator) -> Option<String> {
        None
    }
    /// A bare head-count function knows no places: only pops of one place are a known distance apart.
    fn distance_m(&self, a: &PopLocator, b: &PopLocator) -> Option<u64> {
        (a.place == b.place && self(a).is_some() && self(b).is_some()).then_some(0)
    }
    /// A bare head-count function cannot change a pop: a head cannot be moved through it, so the move is refused.
    fn move_head(
        &self,
        _from: &PopLocator,
        _to: &PopLocator,
        _minor: bool,
    ) -> Result<MoveUndo, Error> {
        Err(Error::Lifecycle)
    }
    fn restore_move(&self, _undo: &MoveUndo) {}
}
impl Canonical for PopLocator {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.place.encode(out)?;
        self.slot.encode(out)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            place: String::decode(r)?,
            slot: u32::decode(r)?,
        })
    }
}
/// Character sex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sex {
    /// Male.
    Male,
    /// Female.
    Female,
}
/// Office that qualifies a character for full simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Office {
    /// Ruler.
    Ruler,
    /// Heir.
    Heir,
    /// High cleric.
    HighCleric,
}
macro_rules! enum_canonical {
    ($ty:ident { $($variant:ident = $tag:literal),+ $(,)? }) => {
        impl Canonical for $ty {
            fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> { { let tag: u8 = match self { $(Self::$variant => $tag,)+ }; tag.encode(out) } }
            fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> { match u8::decode(r)? { $($tag => Ok(Self::$variant),)+ tag => Err(CanonicalError::BadTag { ty: stringify!($ty), tag }) } }
        }
    };
}
enum_canonical!(Sex { Male = 0, Female = 1 });
enum_canonical!(Office { Ruler = 0, Heir = 1, HighCleric = 2 });
/// A live or archived character. Death dates are present only in the archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Character {
    /// Creation-order identifier.
    pub id: u64,
    /// Display name.
    pub name: String,
    /// Sex.
    pub sex: Sex,
    /// Birth date.
    pub birth: GameTime,
    /// Date of death, for archived records.
    pub death: Option<GameTime>,
    /// Stable pop location.
    pub pop: PopLocator,
    /// Father, or [`UNKNOWN`].
    pub father: u64,
    /// Mother, or [`UNKNOWN`].
    pub mother: u64,
    /// Spouse identifiers.
    pub spouses: Vec<u64>,
    /// Child identifiers.
    pub children: Vec<u64>,
    /// Current office.
    pub office: Option<Office>,
    /// Exclusive end of the grace period.
    pub full_until: Option<GameTime>,
    /// Genetics, final when the character is created. Boxed to keep the record small.
    pub genetics: Box<GeneticRecord>,
    /// Personality, stress and standing (CK-362).
    pub personality: PersonalityRecord,
    /// Attributes, deed counters, skills, use factors, focus and the holds-land flag (CK-363), the name pool and the pregnancy
    /// (CK-364). Boxed, as the genetics are, to keep the record small.
    pub life: Box<LifeRecord>,
    /// Orientation, the dates the place and pop were joined, strands and passions (CK-368). Boxed to keep the record small.
    pub ties: Box<Ties>,
}
impl Canonical for Character {
    fn encode(&self, o: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.id.encode(o)?;
        self.name.encode(o)?;
        self.sex.encode(o)?;
        self.birth.encode(o)?;
        self.death.encode(o)?;
        self.pop.encode(o)?;
        self.father.encode(o)?;
        self.mother.encode(o)?;
        self.spouses.encode(o)?;
        self.children.encode(o)?;
        self.office.encode(o)?;
        self.full_until.encode(o)?;
        (*self.genetics).encode(o)?;
        self.personality.encode(o)?;
        (*self.life).encode(o)?;
        (*self.ties).encode(o)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            id: u64::decode(r)?,
            name: String::decode(r)?,
            sex: Sex::decode(r)?,
            birth: GameTime::decode(r)?,
            death: Option::decode(r)?,
            pop: PopLocator::decode(r)?,
            father: u64::decode(r)?,
            mother: u64::decode(r)?,
            spouses: Vec::decode(r)?,
            children: Vec::decode(r)?,
            office: Option::decode(r)?,
            full_until: Option::decode(r)?,
            genetics: Box::new(GeneticRecord::decode(r)?),
            personality: PersonalityRecord::decode(r)?,
            life: Box::new(LifeRecord::decode(r)?),
            ties: Box::new(Ties::decode(r)?),
        })
    }
}
/// The data of a character about to be created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewCharacter {
    /// Display name.
    pub name: String,
    /// Sex.
    pub sex: Sex,
    /// Birth date.
    pub birth: GameTime,
    /// Pop the character belongs to.
    pub pop: PopLocator,
    /// The house of a founder (CK-366): the key a caller gives (slice 8 gives real figures theirs), empty for a commoner. A child
    /// ignores it and takes the house of its birth from its parents.
    pub house: String,
}
impl NewCharacter {
    #[expect(
        clippy::too_many_arguments,
        reason = "the record is made of the id, both parents and the genetics, personality, life and ties made for it"
    )]
    fn into_record(
        self,
        id: u64,
        father: u64,
        mother: u64,
        genetics: GeneticRecord,
        personality: PersonalityRecord,
        life: LifeRecord,
        ties: Ties,
    ) -> Character {
        Character {
            id,
            name: self.name,
            sex: self.sex,
            birth: self.birth,
            death: None,
            pop: self.pop,
            father,
            mother,
            spouses: Vec::new(),
            children: Vec::new(),
            office: None,
            full_until: None,
            genetics: Box::new(genetics),
            personality,
            life: Box::new(life),
            ties: Box::new(ties),
        }
    }
}
/// The parents of a child about to be created, and the archive that resolves an archived one.
#[derive(Clone, Copy, Debug)]
pub struct Lineage<'a> {
    /// Father: a live or archived man, or [`UNKNOWN`].
    pub father: u64,
    /// Mother: a live or archived woman, or [`UNKNOWN`].
    pub mother: u64,
    /// The archive that holds the dead.
    pub archive: &'a Archive,
}
/// The genetic state of the pop at `locator` in an economy.
#[must_use]
pub fn pop_genetics<'a>(
    economy: &'a ck_economy::EconomyState,
    locator: &PopLocator,
) -> Option<&'a PopGenetics> {
    let place = economy
        .places
        .iter()
        .find(|place| place.id == locator.place)?;
    Some(&place.pops.get(locator.slot as usize)?.genetics)
}
/// The derived simulation tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Detailed simulation.
    Full,
    /// Coarse simulation.
    Coarse,
}
/// An invalid store operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The character is missing or the placeholder.
    MissingCharacter,
    /// The target pop has no spare head.
    PopFull,
    /// The target place or pop slot does not exist.
    MissingPop,
    /// An identifier or grace date cannot be represented.
    Overflow,
    /// A relationship is invalid.
    InvalidRelationship,
    /// A father who is not a man or a mother who is not a woman.
    WrongSex,
    /// The genetic data does not allow the character to be made (an unseeded pop, or arithmetic that does not fit).
    Genetics,
    /// A parent's archived record could not be read.
    Archive,
    /// The pop's culture cannot be resolved, or the personality data has no mean for it (CK-362).
    Culture,
    /// Personality arithmetic whose result does not fit.
    Arithmetic,
    /// A standing change whose result does not fit signed 64 bits.
    StandingRange,
    /// `indulge` was given a name that is not one of the seven vice/virtue pairs.
    NotAVicePair,
    /// A negative stress amount.
    Amount,
    /// A trait level above 6.
    Level,
    /// A pop whose type, culture subgroup or luxuries the event engine cannot resolve (CK-363).
    Pop,
    /// An event whose arithmetic does not fit, or a counter that would overflow (CK-363).
    Event,
    /// Life-cycle arithmetic that does not fit, a pregnancy that cannot be delivered, or a pop whose religion or necessities the
    /// passes cannot resolve (CK-364).
    Lifecycle,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
/// Live state. Archive and logs are owned separately by the game session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharacterStore {
    next_id: u64,
    live: BTreeMap<u64, Character>,
    /// The game's own seed for draws that must not depend on the day they are taken on (CK-368: a calendar year's lover mode, the
    /// strand rolls of a year and their replay): fixed once by [`fix_seed`](Self::fix_seed) from the first pass's draw and saved, 0 until
    /// then.
    root: u64,
    /// The live characters by place (location id): derived from `live`, never saved, rebuilt on load (CK-368: a character's candidates
    /// for strands are the characters at its location, so the work does not grow with the characters elsewhere).
    by_place: BTreeMap<String, BTreeSet<u64>>,
    /// The live characters by pop: derived like `by_place`.
    by_pop: BTreeMap<PopLocator, BTreeSet<u64>>,
    /// The live characters by house (a commoner is in none): derived like `by_place`, so a house's head and ruling status are found
    /// among its members, not among everyone.
    by_house: BTreeMap<String, BTreeSet<u64>>,
    /// How many characters candidate discovery has visited (a test-only count of the work done).
    #[cfg(feature = "test-fixtures")]
    visited: Visits,
}
/// A test-only counter that never takes part in a comparison of two stores.
#[cfg(feature = "test-fixtures")]
#[derive(Clone, Debug, Default)]
pub(crate) struct Visits(pub(crate) std::cell::Cell<u64>);
#[cfg(feature = "test-fixtures")]
impl PartialEq for Visits {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
#[cfg(feature = "test-fixtures")]
impl Eq for Visits {}
impl Default for CharacterStore {
    fn default() -> Self {
        Self {
            next_id: 1,
            live: BTreeMap::new(),
            root: 0,
            by_place: BTreeMap::new(),
            by_pop: BTreeMap::new(),
            by_house: BTreeMap::new(),
            #[cfg(feature = "test-fixtures")]
            visited: Visits::default(),
        }
    }
}
impl Canonical for CharacterStore {
    fn encode(&self, o: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.next_id.encode(o)?;
        self.live.encode(o)?;
        self.root.encode(o)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let next_id = u64::decode(r)?;
        let live: BTreeMap<u64, Character> = BTreeMap::decode(r)?;
        let root = u64::decode(r)?;
        let mut store = Self {
            next_id,
            root,
            ..Self::default()
        };
        for (_, record) in live {
            store.put(record);
        }
        Ok(store)
    }
}
/// An independent stream for a character and purpose. Re-requesting the same tuple restarts at the first draw;
/// callers needing several draws hold one stream or use distinct purposes.
#[must_use]
pub fn character_stream(seed: u64, id: u64, purpose: &str, tick: GameTime) -> Stream {
    Stream::new(seed, format!("character:{id}:{purpose}"), tick)
}
/// The culture means of the pop a character is created into: its pop's culture key resolves through `populations`, and the
/// personality data must hold the culture. Either failing refuses the creation.
fn culture_means<'a>(
    ctx: &Context<'a>,
    populations: &impl Populations,
    pop: &PopLocator,
) -> Result<&'a [i64; ck_economy::personality::PAIR_COUNT], Error> {
    let culture = populations.culture(pop).ok_or(Error::Culture)?;
    ctx.personality
        .culture_means
        .get(&culture)
        .ok_or(Error::Culture)
}
fn grace_end(day: GameTime) -> Result<GameTime, Error> {
    let d = day.date().map_err(|_| Error::Overflow)?;
    let year = d
        .year
        .checked_add(FULL_GRACE_YEARS)
        .ok_or(Error::Overflow)?;
    let (month, day) = if d.month == 2 && d.day == 29 {
        (3, 1)
    } else {
        (d.month, d.day)
    };
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour: 0,
    })
    .map_err(|_| Error::Overflow)
}
/// The pool key of the pop's culture and the base name of a character about to be created (CK-364): the name the caller gave, or
/// a draw from the culture's pool on the character's own stream (purpose `name`). The draw is among the pool's names that are not in
/// `excluded` (each equally likely), or among all of them when every name is excluded (CK-365).
fn base_name(
    ctx: &Context<'_>,
    populations: &impl Populations,
    new: &NewCharacter,
    id: u64,
    excluded: &BTreeSet<String>,
) -> Result<(String, String), Error> {
    let culture = populations.culture(&new.pop).ok_or(Error::Culture)?;
    let pool = ctx
        .lifecycle
        .pool_key(&culture)
        .unwrap_or_default()
        .to_owned();
    if !new.name.is_empty() {
        return Ok((pool, new.name.clone()));
    }
    let names = ctx.lifecycle.pools.get(&pool).ok_or(Error::Culture)?;
    let all: Vec<&String> = match new.sex {
        Sex::Male => names.men.iter().map(|(name, _)| name).collect(),
        Sex::Female => names.women.iter().collect(),
    };
    let open: Vec<&String> = all
        .iter()
        .copied()
        .filter(|name| !excluded.contains(*name))
        .collect();
    let list = if open.is_empty() { all } else { open };
    let mut stream = genetics::site(ctx, id, "", "name");
    let at = genetics::below(&mut stream, list.len() as u64)?;
    let drawn = list
        .get(usize::try_from(at).map_err(|_| Error::Overflow)?)
        .map(|name| (*name).clone());
    Ok((pool, drawn.ok_or(Error::Culture)?))
}
/// The record of a child of `parents` (father, mother; `None` is an unknown parent, replaced by a synthetic founder of the child's
/// pop), with the identifier `id`. Pure: it changes no store.
fn child_record(
    ctx: &Context<'_>,
    mut new: NewCharacter,
    id: u64,
    parents: [Option<&Character>; 2],
    pop_genetics: &PopGenetics,
    populations: &impl Populations,
    excluded: &BTreeSet<String>,
) -> Result<Character, Error> {
    let means = culture_means(ctx, populations, &new.pop)?;
    let (pool, name) = base_name(ctx, populations, &new, id, excluded)?;
    new.name = name;
    let kinds = [
        (Sex::Male, "synthetic_father."),
        (Sex::Female, "synthetic_mother."),
    ];
    let mut records = Vec::with_capacity(2);
    for (parent, (sex, prefix)) in parents.into_iter().zip(kinds) {
        records.push(match parent {
            None => (
                genetics::make_founder(ctx, id, prefix, sex, pop_genetics)?,
                personality::make_founder(ctx, id, prefix, means)?,
            ),
            Some(found) => {
                if found.sex != sex {
                    return Err(Error::WrongSex);
                }
                ((*found.genetics).clone(), found.personality)
            }
        });
    }
    let genetics = genetics::make_child(ctx, id, new.sex, &records[0].0, &records[1].0)?;
    let personality = personality::make_child(ctx, id, means, &records[0].1, &records[1].1)?;
    let mut life = life::make(ctx, id, new.sex, populations.kind(&new.pop).as_deref())?;
    life.pool = pool;
    // The house of birth (CK-366): the father's house, else the mother's; the child's house is its house of birth.
    life.house_of_birth = parents[0]
        .map(|father| father.life.house.clone())
        .filter(|house| !house.is_empty())
        .or_else(|| parents[1].map(|mother| mother.life.house.clone()))
        .unwrap_or_default();
    life.house.clone_from(&life.house_of_birth);
    let ties = connections::make_ties(ctx, id, new.sex, parents)?;
    let mut record = new.into_record(
        id,
        parents[0].map_or(UNKNOWN, |parent| parent.id),
        parents[1].map_or(UNKNOWN, |parent| parent.id),
        genetics,
        personality,
        life,
        ties,
    );
    // The body (CK-367): its weight and the first sample of its food record.
    CharacterStore::start_body(ctx, &mut record, populations)?;
    Ok(record)
}

impl CharacterStore {
    /// Insert or replace the live record of `id`, keeping the indexes by place, pop and house current.
    pub(crate) fn put(&mut self, record: Character) {
        let id = record.id;
        let (pop, house) = (record.pop.clone(), record.life.house.clone());
        if let Some(old) = self.live.insert(id, record) {
            self.unindex(id, &old.pop, &old.life.house);
        }
        self.index(id, &pop, &house);
    }
    fn index(&mut self, id: u64, pop: &PopLocator, house: &str) {
        self.by_place
            .entry(pop.place.clone())
            .or_default()
            .insert(id);
        self.by_pop.entry(pop.clone()).or_default().insert(id);
        if !house.is_empty() {
            self.by_house
                .entry(house.to_owned())
                .or_default()
                .insert(id);
        }
    }
    fn unindex(&mut self, id: u64, pop: &PopLocator, house: &str) {
        fn drop_from<K: Ord + Clone>(map: &mut BTreeMap<K, BTreeSet<u64>>, key: &K, id: u64) {
            if let Some(ids) = map.get_mut(key) {
                ids.remove(&id);
                if ids.is_empty() {
                    map.remove(key);
                }
            }
        }
        drop_from(&mut self.by_place, &pop.place, id);
        drop_from(&mut self.by_pop, pop, id);
        if !house.is_empty() {
            drop_from(&mut self.by_house, &house.to_owned(), id);
        }
    }
    /// Remove the live record of `id`, keeping the indexes current.
    pub(crate) fn take(&mut self, id: u64) -> Option<Character> {
        let old = self.live.remove(&id)?;
        self.unindex(id, &old.pop, &old.life.house);
        Some(old)
    }
    /// Set the house of the live character `id`, keeping the house index current.
    pub(crate) fn set_house(&mut self, id: u64, house: &str) {
        let Some(mut record) = self.live.get(&id).cloned() else {
            return;
        };
        house.clone_into(&mut record.life.house);
        self.put(record);
    }
    /// Move the live character `id` into `pop` on `day`: its pop, the place index and the dates it joined the pop and the place.
    pub(crate) fn relocate(&mut self, id: u64, pop: PopLocator, day: GameTime) {
        let Some(mut record) = self.live.get(&id).cloned() else {
            return;
        };
        if record.pop == pop {
            return;
        }
        if record.pop.place != pop.place {
            record.ties.location_since = day;
        }
        record.ties.in_pop_since = day;
        record.pop = pop;
        self.put(record);
    }
    /// Fix the game's own seed from `seed` (the first pass's draw) unless it is fixed already. It is saved with the store, so every later
    /// draw that must be the same whenever it is taken (an annual mode, a year's rolls and their replay) is a function of the game, the
    /// character and the date alone.
    pub fn fix_seed(&mut self, seed: u64) {
        if self.root == 0 {
            self.root = seed | 1;
        }
    }
    /// The seed of draws that must not depend on the day they are taken: the game's own once fixed, else the pass's.
    #[must_use]
    pub(crate) const fn stable_seed(&self, pass_seed: u64) -> u64 {
        if self.root == 0 { pass_seed } else { self.root }
    }
    /// Look up a live record.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Character> {
        self.live.get(&id)
    }
    /// Iterate live records in identifier order.
    pub fn iter(&self) -> impl Iterator<Item = &Character> {
        self.live.values()
    }
    /// Number of live characters attached to a pop.
    #[must_use]
    pub fn pop_count(&self, pop: &PopLocator) -> usize {
        self.by_pop.get(pop).map_or(0, BTreeSet::len)
    }
    fn check_room(&self, pop: &PopLocator, populations: &impl Populations) -> Result<(), Error> {
        let heads = populations.heads(pop).ok_or(Error::MissingPop)?;
        if u64::try_from(self.pop_count(pop)).map_err(|_| Error::Overflow)? >= heads {
            return Err(Error::PopFull);
        }
        Ok(())
    }
    /// Create a founder: a character with no live or archived parents, whose genetics come from the pop's genetic state
    /// (haplogroups from its share sets, values from its baseline, copies from its carrier rates, graded levels from its
    /// founder rates). The record is stored with its genetics in one step: on any error nothing is created. A founder with no
    /// name given is named from the pool of its pop's culture (CK-364).
    /// # Errors
    /// Refuses a missing or full pop, an unseeded pop, a culture without a name pool when a name is drawn, and exhausted
    /// identifiers.
    pub fn create_founder(
        &mut self,
        ctx: &Context<'_>,
        mut new: NewCharacter,
        pop_genetics: &PopGenetics,
        populations: &impl Populations,
    ) -> Result<u64, Error> {
        self.check_room(&new.pop, populations)?;
        let means = culture_means(ctx, populations, &new.pop)?;
        let id = self.next_id;
        let next = id.checked_add(1).ok_or(Error::Overflow)?;
        let (pool, name) = base_name(ctx, populations, &new, id, &BTreeSet::new())?;
        new.name = name;
        let genetics = genetics::make_founder(ctx, id, "", new.sex, pop_genetics)?;
        let personality = personality::make_founder(ctx, id, "", means)?;
        let mut life = life::make(ctx, id, new.sex, populations.kind(&new.pop).as_deref())?;
        life.pool = pool;
        life.house.clone_from(&new.house);
        life.house_of_birth.clone_from(&new.house);
        let ties = connections::make_ties(ctx, id, new.sex, [None, None])?;
        let mut record = new.into_record(id, UNKNOWN, UNKNOWN, genetics, personality, life, ties);
        Self::start_body(ctx, &mut record, populations)?;
        self.put(record);
        self.next_id = next;
        Ok(id)
    }
    /// Create a child of `lineage.father` and `lineage.mother`, live, archived or [`UNKNOWN`]. State only: the game calls
    /// [`create_child_with_events`](Self::create_child_with_events), which also emits `child_born`. An unknown parent is
    /// replaced by a synthetic parent of the right sex, made as a founder of the child's pop on the child's own stream
    /// (purposes `synthetic_father.*`, `synthetic_mother.*`); it is not stored. One transaction: the record, the child's
    /// parent ids and the child in each **live** parent's children list are written together and tiers re-evaluated. An
    /// archived parent is linked from the child only.
    /// # Errors
    /// Refuses, with nothing changed (no id allocated, no link, no record), a father who is not a man or a mother who is
    /// not a woman, a parent that is neither live, archived nor [`UNKNOWN`], a pop that does not resolve or is full, an
    /// archive read failure and an unseeded pop.
    pub(crate) fn create_child(
        &mut self,
        ctx: &Context<'_>,
        new: NewCharacter,
        pop_genetics: &PopGenetics,
        lineage: &Lineage<'_>,
        populations: &impl Populations,
    ) -> Result<u64, Error> {
        self.check_room(&new.pop, populations)?;
        let fetch = |parent: u64| -> Result<Option<Character>, Error> {
            if parent == UNKNOWN {
                return Ok(None);
            }
            self.lookup(parent, lineage.archive)
                .map_err(|_| Error::Archive)?
                .ok_or(Error::MissingCharacter)
                .map(Some)
        };
        let (father, mother) = (fetch(lineage.father)?, fetch(lineage.mother)?);
        let id = self.next_id;
        let next = id.checked_add(1).ok_or(Error::Overflow)?;
        let mut new = new;
        let mut excluded = BTreeSet::new();
        // A child created without a name is named by the namesake rules, else from the pool without its siblings' names (CK-365).
        if new.name.is_empty() {
            let plan = names::plan(
                ctx,
                self,
                lineage.archive,
                id,
                new.sex,
                new.birth,
                [father.as_ref(), mother.as_ref()],
                &BTreeSet::new(),
            )?;
            if let Some(name) = plan.namesake {
                new.name = name;
            }
            excluded = plan.excluded;
        }
        let record = child_record(
            ctx,
            new,
            id,
            [father.as_ref(), mother.as_ref()],
            pop_genetics,
            populations,
            &excluded,
        )?;
        // Everything above is validated and computed, so the writes below cannot fail. A child can only add a
        // qualification (a live ruler's child is full at once, by `tier`'s derivation), never remove one, so no grace
        // period is started.
        self.put(record);
        self.next_id = next;
        for parent in [lineage.father, lineage.mother] {
            if let Some(c) = self.live.get_mut(&parent)
                && !c.children.contains(&id)
            {
                c.children.push(id);
            }
        }
        Ok(id)
    }
    fn qualifies(&self, id: u64) -> bool {
        #[cfg(feature = "test-fixtures")]
        self.visited.0.set(self.visited.0.get() + 1);
        let Some(c) = self.get(id) else {
            return false;
        };
        // A spouse or child of a living ruler: found through the character's own spouses and parents, whose lists name it back.
        c.office.is_some()
            || c.spouses.iter().chain([&c.father, &c.mother]).any(|other| {
                self.live.get(other).is_some_and(|r| {
                    r.office == Some(Office::Ruler)
                        && (r.spouses.contains(&id) || r.children.contains(&id))
                })
            })
    }
    /// Derived tier on the supplied day.
    #[must_use]
    pub fn tier(&self, id: u64, day: GameTime) -> Option<Tier> {
        self.get(id).map(|c| {
            if self.qualifies(id) || c.full_until.is_some_and(|end| day < end) {
                Tier::Full
            } else {
                Tier::Coarse
            }
        })
    }
    fn change(&mut self, day: GameTime, f: impl FnOnce(&mut Self)) -> Result<(), Error> {
        let before: Vec<_> = self
            .live
            .keys()
            .copied()
            .filter(|id| self.qualifies(*id))
            .collect();
        // Who was in the full tier, by the effective tier (a lapsed qualification still in its grace period is full), on the day of the change.
        let was: BTreeSet<u64> = self
            .live
            .keys()
            .copied()
            .filter(|id| self.tier(*id, day) == Some(Tier::Full))
            .collect();
        let end = grace_end(day)?;
        f(self);
        for id in before {
            if !self.qualifies(id)
                && let Some(c) = self.live.get_mut(&id)
            {
                c.full_until = Some(end);
            }
        }
        // A character in the full tier now that was coarse on the day of the change has been promoted: its passion backfill is due at the
        // first daily pass after it, with the baseline of that moment, recorded so that it survives a save (CK-368 §9). Regaining a
        // qualification during grace, or keeping one, is no promotion.
        let promoted: Vec<u64> = self
            .live
            .keys()
            .copied()
            .filter(|id| !was.contains(id) && self.tier(*id, day) == Some(Tier::Full))
            .collect();
        for id in promoted {
            if let Some(c) = self.live.get_mut(&id)
                && c.ties.promoted.is_none()
            {
                c.ties.promoted = Some(c.ties.last_full);
            }
        }
        Ok(())
    }
    /// Change an office, starting grace periods for every lost qualification.
    /// # Errors
    /// Refuses missing characters or an unrepresentable grace date.
    pub fn set_office(
        &mut self,
        id: u64,
        office: Option<Office>,
        day: GameTime,
    ) -> Result<(), Error> {
        if !self.live.contains_key(&id) {
            return Err(Error::MissingCharacter);
        }
        self.change(day, |s| {
            if let Some(c) = s.live.get_mut(&id) {
                if c.office != office {
                    c.ties.office_since = office.map(|_| day);
                }
                c.office = office;
            }
        })
    }
    /// Add a reciprocal marriage. State only: the game calls [`marry_with_events`](Self::marry_with_events), which also emits
    /// the `marriage` happening.
    /// # Errors
    /// Refuses missing or identical partners and unrepresentable dates.
    pub(crate) fn marry(&mut self, a: u64, b: u64, day: GameTime) -> Result<(), Error> {
        if a == b {
            return Err(Error::InvalidRelationship);
        }
        if !self.live.contains_key(&a) || !self.live.contains_key(&b) {
            return Err(Error::MissingCharacter);
        }
        self.change(day, |s| {
            for (x, y) in [(a, b), (b, a)] {
                if let Some(c) = s.live.get_mut(&x)
                    && !c.spouses.contains(&y)
                {
                    c.spouses.push(y);
                }
            }
        })
    }
    /// Set both parents, updating live parents' child links. It does **not** change genetics: a character's genetic record
    /// is final when it is created, so this only edits the recorded relationship.
    /// # Errors
    /// Refuses missing live parents, self-parentage or an unrepresentable date.
    pub fn set_parents(
        &mut self,
        child: u64,
        father: u64,
        mother: u64,
        day: GameTime,
    ) -> Result<(), Error> {
        if !self.live.contains_key(&child)
            || [father, mother]
                .iter()
                .any(|p| *p != UNKNOWN && !self.live.contains_key(p))
        {
            return Err(Error::MissingCharacter);
        }
        if father == child || mother == child {
            return Err(Error::InvalidRelationship);
        }
        self.change(day, |s| {
            for c in s.live.values_mut() {
                c.children.retain(|id| *id != child);
            }
            if let Some(c) = s.live.get_mut(&child) {
                c.father = father;
                c.mother = mother;
            }
            for p in [father, mother] {
                if let Some(c) = s.live.get_mut(&p)
                    && !c.children.contains(&child)
                {
                    c.children.push(child);
                }
            }
        })
    }
}

/// An append-only destination for an archived record. Failure must leave the destination unchanged.
pub trait ArchiveSink {
    /// Append a record or roll back a failed append.
    /// # Errors
    /// Returns an I/O error if the record could not be persisted.
    fn append(&mut self, record: &Character) -> std::io::Result<()>;
    /// Read an archived record, which a delivery needs for a father who has died (CK-364). A sink that cannot read holds none.
    /// # Errors
    /// Returns an I/O or decoding error.
    fn get(&self, _id: u64) -> std::io::Result<Option<Character>> {
        Ok(None)
    }
    /// The identifiers of the archived records whose recorded father or mother is `parent`, in identifier order (CK-365: a child born
    /// after its parent was archived is linked from the child only, so no list of the parent's children names it). A sink that cannot
    /// read holds none. There is no default: a sink that wraps an archive must forward the query, so that it cannot silently report
    /// an empty archive.
    /// # Errors
    /// Returns an I/O error.
    fn children_of(&self, parent: u64) -> std::io::Result<Vec<u64>>;
}
// Grow only with bytes actually read, never directly from an on-disk length.
fn read_record(input: &mut impl std::io::Read, length: u32) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut left = u64::from(length);
    let mut buffer = [0_u8; 16384];
    while left > 0 {
        let count =
            usize::try_from(left.min(buffer.len() as u64)).map_err(std::io::Error::other)?;
        let read = input.read(&mut buffer[..count])?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated record",
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
        left -= read as u64;
    }
    Ok(bytes)
}
/// Disk archive with only an identifier-to-offset index and a parent-to-children index in memory.
#[derive(Debug)]
pub struct Archive {
    path: std::path::PathBuf,
    index: BTreeMap<u64, u64>,
    /// The archived identifiers by recorded parent (a known father or mother), rebuilt when the archive opens.
    by_parent: BTreeMap<u64, BTreeSet<u64>>,
}
/// Record `record` under each of its known parents.
fn index_parents(by_parent: &mut BTreeMap<u64, BTreeSet<u64>>, record: &Character) {
    for parent in [record.father, record.mother] {
        if parent != UNKNOWN {
            by_parent.entry(parent).or_default().insert(record.id);
        }
    }
}
impl Archive {
    /// Open or create an archive, decoding every record to rebuild its index.
    /// # Errors
    /// Refuses malformed, truncated or duplicate records and I/O errors.
    pub fn open(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        use std::io::{Read, Seek};
        let path = path.as_ref().to_owned();
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        let mut index = BTreeMap::new();
        let mut by_parent: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
        let size = file.metadata()?.len();
        while file.stream_position()? < size {
            let offset = file.stream_position()?;
            let mut length = [0; 4];
            file.read_exact(&mut length)?;
            let bytes = read_record(&mut file, u32::from_le_bytes(length))?;
            let record = Character::from_bytes(&bytes).map_err(std::io::Error::other)?;
            if record.id == UNKNOWN
                || record.death.is_none()
                || index.insert(record.id, offset).is_some()
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid or duplicate archive id",
                ));
            }
            index_parents(&mut by_parent, &record);
        }
        Ok(Self {
            path,
            index,
            by_parent,
        })
    }
    /// Retrieve one archived record, reading it from disk on demand.
    /// # Errors
    /// Returns an I/O or decoding error.
    pub fn get(&self, id: u64) -> std::io::Result<Option<Character>> {
        use std::io::{Read, Seek, SeekFrom};
        let Some(offset) = self.index.get(&id) else {
            return Ok(None);
        };
        let mut file = std::fs::File::open(&self.path)?;
        file.seek(SeekFrom::Start(*offset))?;
        let mut len = [0; 4];
        file.read_exact(&mut len)?;
        let bytes = read_record(&mut file, u32::from_le_bytes(len))?;
        Character::from_bytes(&bytes)
            .map(Some)
            .map_err(std::io::Error::other)
    }
    /// Whether an identifier is in this archive.
    #[must_use]
    pub fn contains(&self, id: u64) -> bool {
        self.index.contains_key(&id)
    }
}
impl ArchiveSink for Archive {
    fn append(&mut self, record: &Character) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        if record.id == UNKNOWN || record.death.is_none() || self.index.contains_key(&record.id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid or duplicate archive id",
            ));
        }
        let bytes = record.to_bytes().map_err(std::io::Error::other)?;
        let len = u32::try_from(bytes.len()).map_err(std::io::Error::other)?;
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)?;
        let offset = file.seek(SeekFrom::End(0))?;
        if let Err(error) = (|| {
            file.write_all(&len.to_le_bytes())?;
            file.write_all(&bytes)?;
            file.flush()
        })() {
            file.set_len(offset)?;
            return Err(error);
        }
        self.index.insert(record.id, offset);
        index_parents(&mut self.by_parent, record);
        Ok(())
    }
    fn get(&self, id: u64) -> std::io::Result<Option<Character>> {
        Self::get(self, id)
    }
    fn children_of(&self, parent: u64) -> std::io::Result<Vec<u64>> {
        Ok(self
            .by_parent
            .get(&parent)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default())
    }
}
impl CharacterStore {
    #[cfg(feature = "test-fixtures")]
    /// Archive a character before removing it, leaving all live state intact on append failure. State only: the game calls
    /// [`die_with_events`](Self::die_with_events), which also emits `death_in_family`.
    /// # Errors
    /// Refuses missing characters, an unrepresentable grace date or an archive failure.
    pub(crate) fn die(
        &mut self,
        id: u64,
        day: GameTime,
        archive: &mut impl ArchiveSink,
    ) -> std::io::Result<()> {
        self.die_of(id, day, None, archive)
    }
    #[cfg(feature = "test-fixtures")]
    /// `die` with a cause, recorded in the archived record (CK-363).
    /// # Errors
    /// As `die`.
    pub(crate) fn die_of(
        &mut self,
        id: u64,
        day: GameTime,
        cause: Option<&str>,
        archive: &mut impl ArchiveSink,
    ) -> std::io::Result<()> {
        let dead = self
            .get(id)
            .cloned()
            .ok_or_else(|| std::io::Error::other(Error::MissingCharacter))?;
        self.bury(dead, day, cause, archive)
    }
    /// Archive `dead` (the live record of its id, possibly with an event's effects applied) and remove the live record.
    pub(crate) fn bury(
        &mut self,
        mut dead: Character,
        day: GameTime,
        cause: Option<&str>,
        archive: &mut impl ArchiveSink,
    ) -> std::io::Result<()> {
        let id = dead.id;
        grace_end(day).map_err(std::io::Error::other)?;
        dead.death = Some(day);
        dead.life.death_cause = cause.map(str::to_owned);
        archive.append(&dead)?;
        self.change(day, |s| {
            s.take(id);
        })
        .map_err(std::io::Error::other)?;
        // Its strands end without a line (CK-368 §4): no live character keeps a link to the dead.
        for c in self.live.values_mut() {
            c.ties.links.remove(&id);
        }
        Ok(())
    }
    /// End a marriage, updating both partners and any resulting grace periods.
    /// # Errors
    /// Refuses missing partners or an unrepresentable date.
    pub fn divorce(&mut self, a: u64, b: u64, day: GameTime) -> Result<(), Error> {
        if !self.live.contains_key(&a) || !self.live.contains_key(&b) {
            return Err(Error::MissingCharacter);
        }
        self.change(day, |s| {
            for (x, y) in [(a, b), (b, a)] {
                if let Some(c) = s.live.get_mut(&x) {
                    c.spouses.retain(|id| *id != y);
                }
            }
        })
    }
    /// Move a character to a caller-validated pop with the given number of heads. State only: the game calls
    /// [`move_to_with_events`](Self::move_to_with_events), which also runs precedence.
    /// # Errors
    /// Refuses missing characters and full destination pops.
    pub(crate) fn move_to(
        &mut self,
        id: u64,
        pop: PopLocator,
        day: GameTime,
        populations: &impl Populations,
    ) -> Result<(), Error> {
        let heads = populations.heads(&pop).ok_or(Error::MissingPop)?;
        let old = &self.get(id).ok_or(Error::MissingCharacter)?.pop;
        if *old == pop {
            return Ok(());
        }
        if u64::try_from(self.pop_count(&pop)).map_err(|_| Error::Overflow)? >= heads {
            return Err(Error::PopFull);
        }
        self.relocate(id, pop, day);
        Ok(())
    }
}

/// One dated output line in a character's life log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    /// Character whose life this line records.
    pub id: u64,
    /// Date of the event.
    pub date: GameTime,
    /// Text of the event.
    pub text: String,
}
impl Canonical for LogLine {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.id.encode(out)?;
        self.date.encode(out)?;
        self.text.encode(out)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            id: u64::decode(r)?,
            date: GameTime::decode(r)?,
            text: String::decode(r)?,
        })
    }
}
/// Append-only on-disk life logs. No log contents are retained in memory.
#[derive(Debug)]
pub struct LifeLog {
    path: std::path::PathBuf,
}
impl LifeLog {
    /// Open or create the log file.
    /// # Errors
    /// Returns an I/O error if it cannot be opened.
    pub fn open(path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_owned();
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)?;
        Ok(Self { path })
    }
    /// Append a line; the caller must first verify the id is live or archived.
    /// # Errors
    /// Returns an error if encoding or writing fails.
    pub fn append(&mut self, id: u64, date: GameTime, text: &str) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let bytes = LogLine {
            id,
            date,
            text: text.to_owned(),
        }
        .to_bytes()
        .map_err(std::io::Error::other)?;
        let len = u32::try_from(bytes.len()).map_err(std::io::Error::other)?;
        let mut file = std::fs::OpenOptions::new().write(true).open(&self.path)?;
        let offset = file.seek(SeekFrom::End(0))?;
        if let Err(error) = (|| {
            file.write_all(&len.to_le_bytes())?;
            file.write_all(&bytes)?;
            file.flush()
        })() {
            file.set_len(offset)?;
            return Err(error);
        }
        Ok(())
    }
    /// Read one character's lines in date order, preserving append order for equal dates.
    /// # Errors
    /// Refuses truncated or invalid records and I/O errors.
    pub fn read(&self, id: u64) -> std::io::Result<Vec<LogLine>> {
        let mut lines = Vec::new();
        self.visit(|line| {
            if line.id == id {
                lines.push(line);
            }
            Ok(())
        })?;
        lines.sort_by_key(|line| line.date);
        Ok(lines)
    }
    /// Visit each line in file order, without loading the whole log.
    /// # Errors
    /// Refuses malformed records, I/O errors or an error returned by the visitor.
    pub fn visit(
        &self,
        mut visit: impl FnMut(LogLine) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        use std::io::{Read, Seek};
        let mut f = std::fs::File::open(&self.path)?;
        let size = f.metadata()?.len();
        while f.stream_position()? < size {
            let mut len = [0; 4];
            f.read_exact(&mut len)?;
            let bytes = read_record(&mut f, u32::from_le_bytes(len))?;
            visit(LogLine::from_bytes(&bytes).map_err(std::io::Error::other)?)?;
        }
        Ok(())
    }
}

impl Archive {
    /// The archived identifiers, in identifier order.
    pub fn ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.index.keys().copied()
    }
}
impl CharacterStore {
    /// Validate every pop locator and its capacity against the current economy.
    /// # Errors
    /// Returns an error for a missing pop or a pop below its character floor.
    pub fn validate_pops(
        &self,
        mut heads: impl FnMut(&PopLocator) -> Option<u64>,
    ) -> Result<(), Error> {
        let mut counts = BTreeMap::<&PopLocator, u64>::new();
        for character in self.live.values() {
            *counts.entry(&character.pop).or_default() += 1;
        }
        for (locator, count) in counts {
            if heads(locator).is_none_or(|size| size < count) {
                return Err(Error::PopFull);
            }
        }
        Ok(())
    }
    /// Validate live identifiers against the archive and the next-id counter.
    /// # Errors
    /// Refuses invalid or duplicated identifiers.
    pub fn validate_archive(&self, archive: &Archive) -> std::io::Result<()> {
        if self.next_id == 0
            || self.live.iter().any(|(id, c)| {
                *id == UNKNOWN
                    || *id != c.id
                    || *id >= self.next_id
                    || c.death.is_some()
                    || archive.contains(*id)
            })
            || archive.ids().any(|id| id >= self.next_id)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ARCH section: invalid character ids",
            ));
        }
        Ok(())
    }
}

impl CharacterStore {
    /// Resolve a live or archived character. The unknown placeholder has no record.
    /// # Errors
    /// Returns an archive I/O error.
    pub fn lookup(&self, id: u64, archive: &Archive) -> std::io::Result<Option<Character>> {
        if let Some(c) = self.get(id) {
            Ok(Some(c.clone()))
        } else {
            archive.get(id)
        }
    }
    /// Append a line for a known live or archived character.
    /// # Errors
    /// Refuses unknown identifiers and log I/O errors.
    pub fn log(
        &self,
        archive: &Archive,
        logs: &mut LifeLog,
        id: u64,
        day: GameTime,
        text: &str,
    ) -> std::io::Result<()> {
        if self.get(id).is_none() && !archive.contains(id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "unknown character",
            ));
        }
        logs.append(id, day, text)
    }
}
