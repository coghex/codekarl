//! A character's attributes, deed counters, skills, use factors, focus of education and holds-land flag (CK-363,
//! characters slice 4).
//!
//! The record holds what slices 1-3 do not: the levels of the keys that events set. A key's level is read and written where its
//! meaning already lives: the three mind traits of slice 3 in the personality record, a graded trait genetics gives (and
//! `madness`) in the genetic levels, the Mendelian conditions' shown flag, and every other key (`recluse`, deeds, crafts, callings,
//! stations and backgrounds) in [`LifeRecord::grades`].
#![deny(clippy::float_arithmetic)]

use crate::{
    Character, Context, Error, Sex,
    genetics::{below, normal_draw_wide, site},
};
use ck_sim::{Canonical, CanonicalError, GameTime, Reader};
use std::collections::BTreeMap;

/// The state slice 4 adds to a character.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LifeRecord {
    /// The level (1..=6) of every other graded key held, and 1 for an ungraded key held; a key not held is absent.
    pub grades: BTreeMap<String, u8>,
    /// The deed counters, which never decrease.
    pub counters: BTreeMap<String, u64>,
    /// Each skill's level in thousandths, 0 at birth.
    pub skills: BTreeMap<String, i64>,
    /// How much of its pop type's use of each skill the character takes up, in thousandths (drawn once at birth).
    pub use_factor: BTreeMap<String, i64>,
    /// The focus of education, drawn once at birth; empty or `balanced` changes nothing.
    pub focus: String,
    /// Whether the character holds land (set by the `inherited_land` happening; a stand-in until titles and land).
    pub holds_land: bool,
    /// The cause of death, in an archived record.
    pub death_cause: Option<String>,
    /// The key of the name pool the base name was drawn from (CK-364): immutable, and what a shown name reads, never the pop's
    /// current culture. Empty for a character created without life-cycle data.
    pub pool: String,
    /// The father of the child a woman carries, while she is pregnant (CK-364).
    pub pregnant_by: Option<u64>,
    /// The 1st on which a pregnant woman delivers (CK-364).
    pub due: Option<GameTime>,
    /// The 1st from which a woman who delivered may conceive again (CK-364).
    pub nursing_until: Option<GameTime>,
    /// The epithets held, each with the start of the day it was gained (CK-365). Kept for life, shown or not: the shown one is derived
    /// by [`Character::shown_epithet`] from this stored state alone.
    pub epithets: BTreeMap<String, GameTime>,
    /// The house the character belongs to (CK-366): a key, empty for a commoner. A wife takes her husband's house on marriage, a
    /// houseless husband his wife's.
    pub house: String,
    /// The house of the character's birth (CK-366): its father's house at the birth, else its mother's. Kept for life.
    pub house_of_birth: String,
}

impl Canonical for LifeRecord {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.grades.encode(out)?;
        self.counters.encode(out)?;
        self.skills.encode(out)?;
        self.use_factor.encode(out)?;
        self.focus.encode(out)?;
        self.holds_land.encode(out)?;
        self.death_cause.encode(out)?;
        self.pool.encode(out)?;
        self.pregnant_by.encode(out)?;
        self.due.encode(out)?;
        self.nursing_until.encode(out)?;
        self.epithets.encode(out)?;
        self.house.encode(out)?;
        self.house_of_birth.encode(out)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            grades: BTreeMap::decode(r)?,
            counters: BTreeMap::decode(r)?,
            skills: BTreeMap::decode(r)?,
            use_factor: BTreeMap::decode(r)?,
            focus: String::decode(r)?,
            holds_land: bool::decode(r)?,
            death_cause: Option::decode(r)?,
            pool: String::decode(r)?,
            pregnant_by: Option::decode(r)?,
            due: Option::decode(r)?,
            nursing_until: Option::decode(r)?,
            epithets: BTreeMap::decode(r)?,
            house: String::decode(r)?,
            house_of_birth: String::decode(r)?,
        })
    }
}

impl Character {
    /// The level of a catalog key: 0 for none, 1..=6 for C to SSS, and 1 for an ungraded key held (a shown Mendelian
    /// condition, a calling, a station or a background).
    #[must_use]
    pub fn grade(&self, key: &str) -> u8 {
        match key {
            "melancholy" => self.personality.melancholy,
            "rage_fits" => self.personality.rage_fits,
            "drunkard" => self.personality.drunkard,
            _ => {
                if let Some(level) = self.genetics.level.get(key) {
                    *level
                } else if let Some(condition) = self.genetics.mendelian.get(key) {
                    u8::from(condition.shows)
                } else {
                    self.life.grades.get(key).copied().unwrap_or(0)
                }
            }
        }
    }

    /// Set the level of a key an event may change; 0 removes it.
    pub(crate) fn set_grade(&mut self, key: &str, level: u8) {
        match key {
            "melancholy" => self.personality.melancholy = level,
            "rage_fits" => self.personality.rage_fits = level,
            "drunkard" => self.personality.drunkard = level,
            _ => {
                if let Some(slot) = self.genetics.level.get_mut(key) {
                    *slot = level;
                } else if level == 0 {
                    self.life.grades.remove(key);
                } else {
                    self.life.grades.insert(key.to_owned(), level);
                }
            }
        }
    }

    /// A skill's level in thousandths (0 when it has none).
    #[must_use]
    pub fn skill(&self, name: &str) -> i64 {
        self.life.skills.get(name).copied().unwrap_or(0)
    }
}

/// A new character's life record: skills at 0, a use factor for every skill (`clamp(normal(1, use_spread), 0, 1)`, purpose
/// `skill_use.<skill>`) and a focus of education drawn from the odds of its pop type and sex (purpose `focus`; a pop type the
/// caller cannot name draws nothing and is balanced).
pub(crate) fn make(
    ctx: &Context<'_>,
    id: u64,
    sex: Sex,
    kind: Option<&str>,
) -> Result<LifeRecord, Error> {
    let skills = &ctx.events.skills;
    let mut record = LifeRecord::default();
    for name in &skills.list {
        record.skills.insert(name.clone(), 0);
        let mut stream = site(ctx, id, "", &format!("skill_use.{name}"));
        // Bound the draw while it is wide: a spread near the 64-bit limit must clamp, not fail.
        let drawn = normal_draw_wide(&mut stream, 1000, skills.use_spread)?;
        let bounded = i64::try_from(drawn.clamp(0, 1000)).map_err(|_| Error::Genetics)?;
        record.use_factor.insert(name.clone(), bounded);
    }
    "balanced".clone_into(&mut record.focus);
    let key = (
        kind.unwrap_or_default().to_owned(),
        match sex {
            Sex::Male => 'm',
            Sex::Female => 'f',
        },
    );
    if let Some(odds) = skills.focus_odds.get(&key) {
        let total: u64 = odds.iter().map(|(_, weight)| u64::from(*weight)).sum();
        let mut pick = below(&mut site(ctx, id, "", "focus"), total)?;
        for (name, weight) in odds {
            if pick < u64::from(*weight) {
                record.focus.clone_from(name);
                break;
            }
            pick -= u64::from(*weight);
        }
    }
    Ok(record)
}
