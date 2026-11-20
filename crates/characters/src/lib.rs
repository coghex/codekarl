//! Backend-neutral live characters and their derived tiers.
#![deny(unsafe_code)]

use ck_economy::PopGenetics;
use ck_sim::{Canonical, CanonicalError, Date, GameTime, Reader, Stream};
use std::collections::BTreeMap;

mod bigint;
pub mod events;
mod fixtures;
pub mod genetics;
pub mod life;
pub mod personality;
pub mod skills;
pub use events::{EventError, LogSink, World};
pub use genetics::{Context, GeneticRecord, Mendel};
pub use life::LifeRecord;
pub use personality::{
    EventKind, LevelTrait, PersonalityRecord, Standing, StressKind, opinion, threshold,
};

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
}
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
}
/// A bare head-count function knows no cultures, so a character cannot be created into its pops.
impl<F: Fn(&PopLocator) -> Option<u64>> Populations for F {
    fn heads(&self, locator: &PopLocator) -> Option<u64> {
        self(locator)
    }
    fn culture(&self, _locator: &PopLocator) -> Option<String> {
        None
    }
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
    /// Attributes, deed counters, skills, use factors, focus and the holds-land flag (CK-363).
    pub life: LifeRecord,
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
        self.life.encode(o)
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
            life: LifeRecord::decode(r)?,
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
}
impl NewCharacter {
    fn into_record(
        self,
        id: u64,
        father: u64,
        mother: u64,
        genetics: GeneticRecord,
        personality: PersonalityRecord,
        life: LifeRecord,
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
            life,
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
}
impl Default for CharacterStore {
    fn default() -> Self {
        Self {
            next_id: 1,
            live: BTreeMap::new(),
        }
    }
}
impl Canonical for CharacterStore {
    fn encode(&self, o: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.next_id.encode(o)?;
        self.live.encode(o)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            next_id: u64::decode(r)?,
            live: BTreeMap::decode(r)?,
        })
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
impl CharacterStore {
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
        self.live.values().filter(|c| &c.pop == pop).count()
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
    /// founder rates). The record is stored with its genetics in one step: on any error nothing is created.
    /// # Errors
    /// Refuses a missing or full pop, an unseeded pop and exhausted identifiers.
    pub fn create_founder(
        &mut self,
        ctx: &Context<'_>,
        new: NewCharacter,
        pop_genetics: &PopGenetics,
        populations: &impl Populations,
    ) -> Result<u64, Error> {
        self.check_room(&new.pop, populations)?;
        let means = culture_means(ctx, populations, &new.pop)?;
        let id = self.next_id;
        let next = id.checked_add(1).ok_or(Error::Overflow)?;
        let genetics = genetics::make_founder(ctx, id, "", new.sex, pop_genetics)?;
        let personality = personality::make_founder(ctx, id, "", means)?;
        let life = life::make(ctx, id, new.sex, populations.kind(&new.pop).as_deref())?;
        self.live.insert(
            id,
            new.into_record(id, UNKNOWN, UNKNOWN, genetics, personality, life),
        );
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
        let means = culture_means(ctx, populations, &new.pop)?;
        let parents = [
            (lineage.father, Sex::Male, "synthetic_father."),
            (lineage.mother, Sex::Female, "synthetic_mother."),
        ];
        let id = self.next_id;
        let next = id.checked_add(1).ok_or(Error::Overflow)?;
        let mut records = Vec::with_capacity(2);
        for (parent, sex, prefix) in parents {
            records.push(if parent == UNKNOWN {
                (
                    genetics::make_founder(ctx, id, prefix, sex, pop_genetics)?,
                    personality::make_founder(ctx, id, prefix, means)?,
                )
            } else {
                let found = self
                    .lookup(parent, lineage.archive)
                    .map_err(|_| Error::Archive)?
                    .ok_or(Error::MissingCharacter)?;
                if found.sex != sex {
                    return Err(Error::WrongSex);
                }
                (*found.genetics, found.personality)
            });
        }
        let genetics = genetics::make_child(ctx, id, new.sex, &records[0].0, &records[1].0)?;
        let personality = personality::make_child(ctx, id, means, &records[0].1, &records[1].1)?;
        let life = life::make(ctx, id, new.sex, populations.kind(&new.pop).as_deref())?;
        let record = new.into_record(
            id,
            lineage.father,
            lineage.mother,
            genetics,
            personality,
            life,
        );
        // Everything above is validated and computed, so the writes below cannot fail. A child can only add a
        // qualification (a live ruler's child is full at once, by `tier`'s derivation), never remove one, so no grace
        // period is started.
        self.live.insert(id, record);
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
        let Some(c) = self.get(id) else {
            return false;
        };
        c.office.is_some()
            || self.live.values().any(|r| {
                r.office == Some(Office::Ruler)
                    && (r.spouses.contains(&id) || r.children.contains(&id))
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
        let end = grace_end(day)?;
        f(self);
        for id in before {
            if !self.qualifies(id)
                && let Some(c) = self.live.get_mut(&id)
            {
                c.full_until = Some(end);
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
/// Disk archive with only an identifier-to-offset index in memory.
#[derive(Debug)]
pub struct Archive {
    path: std::path::PathBuf,
    index: BTreeMap<u64, u64>,
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
        }
        Ok(Self { path, index })
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
        Ok(())
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
            s.live.remove(&id);
        })
        .map_err(std::io::Error::other)
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
        if let Some(c) = self.live.get_mut(&id) {
            c.pop = pop;
        }
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
