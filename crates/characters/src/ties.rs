//! A character's ties (CK-368, characters slice 6): its orientation, the dates its place and pop were joined, its strands with the
//! years each stage last handled them, and its passions.
//!
//! The record is canonical and saved; contact is never stored (it is computed from where characters are). Every year is a calendar
//! year; 0 means none.
#![deny(clippy::float_arithmetic)]

use ck_sim::{Canonical, CanonicalError, GameTime, Reader};
use std::collections::BTreeMap;

/// The number of strand kinds (`ck_economy::events::ALL_KINDS`: the three bonds, then the five feelings).
pub const KINDS: usize = 8;
/// The highest strand or passion strength.
pub const MAX_STRENGTH: u8 = 10;

/// Whom a character is drawn to, rolled once at birth and never changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    /// Drawn to the other sex.
    #[default]
    Straight,
    /// Drawn to the same sex.
    Gay,
    /// Drawn to both.
    Bisexual,
    /// Drawn to neither.
    Asexual,
}

impl Orientation {
    /// The orientations in the order of `ck_economy::events::ORIENTATIONS`.
    pub const ALL: [Self; 4] = [Self::Straight, Self::Gay, Self::Bisexual, Self::Asexual];

    /// Whether this orientation draws a character toward a character of the sex `same` says (the same sex as its own or not).
    #[must_use]
    pub const fn attracted(self, same_sex: bool) -> bool {
        match self {
            Self::Straight => !same_sex,
            Self::Gay => same_sex,
            Self::Bisexual => true,
            Self::Asexual => false,
        }
    }
}

impl Canonical for Orientation {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        let tag: u8 = match self {
            Self::Straight => 0,
            Self::Gay => 1,
            Self::Bisexual => 2,
            Self::Asexual => 3,
        };
        tag.encode(out)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        match u8::decode(r)? {
            0 => Ok(Self::Straight),
            1 => Ok(Self::Gay),
            2 => Ok(Self::Bisexual),
            3 => Ok(Self::Asexual),
            tag => Err(CanonicalError::BadTag {
                ty: "Orientation",
                tag,
            }),
        }
    }
}

/// One strand of one kind: its strength (0 is none) and the years it last grew and, for a feeling, last faded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Strand {
    /// 0 for none, else 1..=10.
    pub strength: u8,
    /// The calendar year of the last growth (a roll, a +1 or a happening), 0 for none.
    pub grew: u16,
    /// The calendar year of the last fade (a feeling's), 0 for none.
    pub faded: u16,
}

/// What a character holds toward one partner: the strands of every kind, and the years each stage last handled the pair.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Link {
    /// The year of the pair's last mutual (bond) roll. Stored on both members.
    pub mutual_roll: u16,
    /// The closed year the pair's 1 January resolution last handled. Stored on both members.
    pub resolved: u16,
    /// The year of the holder's last feeling roll toward the partner. Held by the holder alone.
    pub feeling_roll: u16,
    /// The strands, by kind index. A bond is stored on both members, a feeling on its holder alone.
    pub strands: [Strand; KINDS],
}

impl Link {
    /// Whether the link holds nothing worth keeping: no strand and no marker.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mutual_roll == 0
            && self.resolved == 0
            && self.feeling_roll == 0
            && self.strands.iter().all(|s| s.strength == 0)
    }
}

/// What a passion is aimed at: a place, a religion, a house or a culture, by key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PassionTarget {
    /// 0 place (a location id), 1 religion, 2 house, 3 culture.
    pub kind: u8,
    /// The key.
    pub key: String,
}

/// A passion: a kind (an index of `ck_economy::events::PASSION_KINDS`), its target, strength and the two dates its fade runs from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Passion {
    /// The kind: loyalty, love, hatred or fear.
    pub kind: u8,
    /// The target.
    pub target: PassionTarget,
    /// The strength, 1..=10.
    pub strength: u8,
    /// The date it was last renewed (or gained).
    pub renewed: GameTime,
    /// The date its fade last advanced to (or it was gained).
    pub last_fade: GameTime,
}

/// The state slice 6 adds to a character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ties {
    /// Whom the character is drawn to.
    pub orientation: Orientation,
    /// When the `ruler`-or-other office marker was last set to a different value; `None` with no office.
    pub office_since: Option<GameTime>,
    /// When the character was created or last moved to a pop at another location.
    pub location_since: GameTime,
    /// When the character was created or last moved to another pop.
    pub in_pop_since: GameTime,
    /// The calendar year of the character's last full-tier roll, 0 when none is recorded (a record made before it was kept is never
    /// replayed). A character made in a year starts at that year: what came before it is not its to replay.
    pub last_full: u16,
    /// `Some(baseline)` while the character has been promoted from the coarse to the full tier and its passion backfill is yet to be started
    /// (CK-368 §9): set by the transition itself with the last full-tier year **at that moment**, and cleared when the first daily pass after it
    /// fixes the window. It is the promotion, not the age of `last_full`, that makes a backfill due, and the baseline, not the marker a live
    /// birthday may have advanced since, that starts the window.
    pub promoted: Option<u16>,
    /// The window of years whose passions are being backfilled after a promotion, `(from, to)`, fixed and saved on the first daily pass
    /// after it (CK-368 §9); `(0, 0)` when there is none. The saved window, not the day, defines the years on every attempt.
    pub backfill: (u16, u16),
    /// The links, by partner id.
    pub links: BTreeMap<u64, Link>,
    /// The passions, in the order gained.
    pub passions: Vec<Passion>,
}

impl Default for Ties {
    fn default() -> Self {
        Self {
            orientation: Orientation::default(),
            office_since: None,
            location_since: GameTime::from_hours(0),
            in_pop_since: GameTime::from_hours(0),
            last_full: 0,
            promoted: None,
            backfill: (0, 0),
            links: BTreeMap::new(),
            passions: Vec::new(),
        }
    }
}

impl Canonical for PassionTarget {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.kind.encode(out)?;
        self.key.encode(out)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            kind: u8::decode(r)?,
            key: String::decode(r)?,
        })
    }
}
impl Canonical for Passion {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.kind.encode(out)?;
        self.target.encode(out)?;
        self.strength.encode(out)?;
        self.renewed.encode(out)?;
        self.last_fade.encode(out)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            kind: u8::decode(r)?,
            target: PassionTarget::decode(r)?,
            strength: u8::decode(r)?,
            renewed: GameTime::decode(r)?,
            last_fade: GameTime::decode(r)?,
        })
    }
}
impl Canonical for Link {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.mutual_roll.encode(out)?;
        self.resolved.encode(out)?;
        self.feeling_roll.encode(out)?;
        for strand in &self.strands {
            strand.strength.encode(out)?;
            strand.grew.encode(out)?;
            strand.faded.encode(out)?;
        }
        Ok(())
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let mut link = Self {
            mutual_roll: u16::decode(r)?,
            resolved: u16::decode(r)?,
            feeling_roll: u16::decode(r)?,
            strands: [Strand::default(); KINDS],
        };
        for strand in &mut link.strands {
            strand.strength = u8::decode(r)?;
            strand.grew = u16::decode(r)?;
            strand.faded = u16::decode(r)?;
        }
        Ok(link)
    }
}
impl Canonical for Ties {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.orientation.encode(out)?;
        self.office_since.encode(out)?;
        self.location_since.encode(out)?;
        self.in_pop_since.encode(out)?;
        self.last_full.encode(out)?;
        self.promoted.encode(out)?;
        self.backfill.0.encode(out)?;
        self.backfill.1.encode(out)?;
        self.links.encode(out)?;
        self.passions.encode(out)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            orientation: Orientation::decode(r)?,
            office_since: Option::decode(r)?,
            location_since: GameTime::decode(r)?,
            in_pop_since: GameTime::decode(r)?,
            last_full: u16::decode(r)?,
            promoted: Option::<u16>::decode(r)?,
            backfill: (u16::decode(r)?, u16::decode(r)?),
            links: BTreeMap::decode(r)?,
            passions: Vec::decode(r)?,
        })
    }
}

impl Ties {
    /// The strength (0 for none) of the strand of `kind` (`friend`, `rival`, `lover`, `love`, `respect`, `envy`, `fear` or `hatred`) this
    /// character holds toward `partner`.
    #[must_use]
    pub fn strength(&self, partner: u64, kind: &str) -> u8 {
        ck_economy::events::ALL_KINDS
            .iter()
            .position(|known| *known == kind)
            .and_then(|at| {
                self.links
                    .get(&partner)
                    .map(|link| link.strands[at].strength)
            })
            .unwrap_or(0)
    }

    /// The strand of `kind` toward `partner`, with the years it last grew and faded.
    #[must_use]
    pub fn strand(&self, partner: u64, kind: &str) -> Strand {
        ck_economy::events::ALL_KINDS
            .iter()
            .position(|known| *known == kind)
            .and_then(|at| self.links.get(&partner).map(|link| link.strands[at]))
            .unwrap_or_default()
    }
}
