//! A character's personality, stress and standing, and opinion (CK-362, characters slice 3).
//!
//! No floating point: vice/virtue and manner values, stress and standing are integers in thousandths, and every formula
//! is evaluated exactly as a rational (products of thousandths over the matching power of 1000) and rounded once, where
//! the formula names the rounding. Wide intermediates are `i128` and checked: a result that does not fit is a named
//! [`Error`], never a wrap or a silent clamp.
#![deny(clippy::float_arithmetic)]

use crate::{
    Character, CharacterStore, Context, Error, Office,
    genetics::{normal_z, site},
};
use ck_economy::{
    Personality,
    genetics::{round_div, spread_factor},
    personality::{MANNER_PAIRS, PAIR_COUNT, STRESS_MAX, VICE_BOUND, VICE_PAIRS},
};
use ck_sim::{Canonical, CanonicalError, GameTime, Reader, is_leap_year};

/// Index of the blood humour in a genetic attribute array.
const BLOOD: usize = 9;
/// Index of the yellow bile humour.
const YELLOW_BILE: usize = 10;
/// Index of the black bile humour.
const BLACK_BILE: usize = 11;
/// Index of the phlegm humour.
const PHLEGM: usize = 12;
/// The index of `gluttony_temperance` among the vice/virtue pairs.
const GLUTTONY_TEMPERANCE: usize = 4;
/// The highest trait level (SSS).
pub const MAX_LEVEL: u8 = 6;
/// Millionths per unit: the scale of a product of two thousandths.
const MILLION: i128 = 1_000_000;

/// A character's personality, stress and standing, all in thousandths, and the levels of the traits slice 4 sets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PersonalityRecord {
    /// The seven vice/virtue values, sin negative, held to -7000..=7000.
    pub vice: [i64; 7],
    /// The nine manner values, unbounded (-20..=20 is a display range only).
    pub manner: [i64; 9],
    /// Stress, 0..=100000.
    pub stress: i64,
    /// Popularity, signed and unbounded.
    pub popularity: i64,
    /// Fame, signed and unbounded.
    pub fame: i64,
    /// Honour, signed and unbounded.
    pub honour: i64,
    /// Drunkard level, 0..=6.
    pub drunkard: u8,
    /// Rage fits level, 0..=6.
    pub rage_fits: u8,
    /// Melancholy level, 0..=6.
    pub melancholy: u8,
}

impl Canonical for PersonalityRecord {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        for value in self.vice.iter().chain(&self.manner) {
            value.encode(out)?;
        }
        self.stress.encode(out)?;
        self.popularity.encode(out)?;
        self.fame.encode(out)?;
        self.honour.encode(out)?;
        self.drunkard.encode(out)?;
        self.rage_fits.encode(out)?;
        self.melancholy.encode(out)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let mut record = Self::default();
        for slot in record.vice.iter_mut().chain(record.manner.iter_mut()) {
            *slot = i64::decode(r)?;
        }
        record.stress = i64::decode(r)?;
        record.popularity = i64::decode(r)?;
        record.fame = i64::decode(r)?;
        record.honour = i64::decode(r)?;
        record.drunkard = u8::decode(r)?;
        record.rage_fits = u8::decode(r)?;
        record.melancholy = u8::decode(r)?;
        Ok(record)
    }
}

/// What caused stress: an insult or a defeat also scales with yellow bile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StressKind {
    /// An insult.
    Insult,
    /// A defeat.
    Defeat,
    /// Any other cause.
    Other,
}

/// Whether an event is caused by stress or lifts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// A mind-trait event that stress makes more likely.
    Causing,
    /// A mind-trait event that stress makes less likely.
    Lifting,
}

/// One of the three standing values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Popularity.
    Popularity,
    /// Fame.
    Fame,
    /// Honour.
    Honour,
}

/// A trait whose level slice 4 sets by events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelTrait {
    /// Drunkard.
    Drunkard,
    /// Rage fits.
    RageFits,
    /// Melancholy.
    Melancholy,
}

pub(crate) fn wide(value: i64) -> i128 {
    i128::from(value)
}

pub(crate) fn product(factors: &[i128]) -> Result<i128, Error> {
    factors
        .iter()
        .try_fold(1_i128, |total, factor| total.checked_mul(*factor))
        .ok_or(Error::Arithmetic)
}

/// The product of non-negative factors, saturating at `i128::MAX`. A caller that then divides by a power of 1000 and holds
/// the result to a small bound (stress 0..=100) gets the exact bounded result: a saturated product is far past every such
/// bound, and a zero factor still gives zero.
fn saturating_product(factors: &[i128]) -> i128 {
    factors
        .iter()
        .fold(1_i128, |total, factor| total.saturating_mul(*factor))
}

pub(crate) fn sum(terms: &[i128]) -> Result<i128, Error> {
    terms
        .iter()
        .try_fold(0_i128, |total, term| total.checked_add(*term))
        .ok_or(Error::Arithmetic)
}

fn narrow(value: i128) -> Result<i64, Error> {
    i64::try_from(value).map_err(|_| Error::Arithmetic)
}

/// `1 + weight * (value - 5)` as a multiple of 1/1,000,000, held at zero or above: `weight` and `value` in thousandths.
fn humour_factor(weight: i64, humour: i64) -> Result<i128, Error> {
    Ok(sum(&[MILLION, product(&[wide(weight), wide(humour) - 5000])?])?.max(0))
}

/// `1 + weight * grade` as a multiple of 1/1,000, held at zero or above: `weight` in thousandths, `grade` a level.
fn grade_factor(weight: i64, grade: u8) -> Result<i128, Error> {
    Ok(sum(&[1000, product(&[wide(weight), i128::from(grade)])?])?.max(0))
}

fn humour(character: &Character, index: usize) -> i64 {
    character.genetics.value[index]
}

/// The stress threshold in thousandths: `max(0, 50 + 8 phlegm + 3 blood - 3 yellow bile - 8 black bile)`, exact, with no
/// upper cap.
#[must_use]
pub fn threshold(character: &Character) -> i64 {
    let total =
        50_000_i128 + 8 * wide(humour(character, PHLEGM)) + 3 * wide(humour(character, BLOOD))
            - 3 * wide(humour(character, YELLOW_BILE))
            - 8 * wide(humour(character, BLACK_BILE));
    i64::try_from(total.max(0)).unwrap_or(i64::MAX)
}

/// Store a drawn value: a vice/virtue value is held to -7..=7 in the wide representation (so the bound, not the width,
/// decides a draw near the 64-bit limit); a manner value is unbounded, and one that does not fit is a named error.
fn apply_value(record: &mut PersonalityRecord, index: usize, value: i128) -> Result<(), Error> {
    if let Some(slot) = record.vice.get_mut(index) {
        *slot = narrow(value.clamp(wide(-VICE_BOUND), wide(VICE_BOUND)))?;
    } else if let Some(slot) = record.manner.get_mut(index - VICE_PAIRS.len()) {
        *slot = narrow(value)?;
    }
    Ok(())
}

fn pair_names() -> impl Iterator<Item = &'static str> {
    VICE_PAIRS.iter().chain(&MANNER_PAIRS).copied()
}

fn spread_of(rules: &Personality, index: usize) -> i64 {
    if index < VICE_PAIRS.len() {
        rules.vice_spread
    } else {
        rules.manner_spread
    }
}

/// A founder's personality: each pair a normal draw around the culture mean (§3). `prefix` names a synthetic parent's sites.
pub(crate) fn make_founder(
    ctx: &Context<'_>,
    id: u64,
    prefix: &str,
    means: &[i64; PAIR_COUNT],
) -> Result<PersonalityRecord, Error> {
    let mut record = PersonalityRecord::default();
    for (index, name) in pair_names().enumerate() {
        let mut stream = site(ctx, id, prefix, &format!("personality.{name}"));
        let drawn = crate::genetics::normal_draw_wide(
            &mut stream,
            means[index],
            spread_of(ctx.personality, index),
        )?;
        apply_value(&mut record, index, drawn)?;
    }
    Ok(record)
}

fn value_at(record: &PersonalityRecord, index: usize) -> i64 {
    if index < VICE_PAIRS.len() {
        record.vice[index]
    } else {
        record.manner[index - VICE_PAIRS.len()]
    }
}

/// One pair of a child's personality (§3): `mean + w (F + M - 2 mean) / 2 + e`, `w` the parents' weight, `e` normal with
/// spread `spread * f_w / 1000` and `z` the unrounded variate (millionths). The sum is rounded once, to the nearest
/// thousandth, ties away from zero; over the common denominator 2,000,000,000 it is
/// `mean D + w (F + M - 2 mean) 1,000,000 + 2 spread f_w z`.
///
/// # Errors
/// Refuses a weight outside 0..=1000 and arithmetic that does not fit. The result is not narrowed or bounded.
pub fn child_value(
    mean: i64,
    father: i64,
    mother: i64,
    weight: i64,
    spread: i64,
    z: i128,
) -> Result<i128, Error> {
    const DENOMINATOR: i128 = 2_000_000_000;
    let factor = i128::from(spread_factor(
        u32::try_from(weight).map_err(|_| Error::Arithmetic)?,
    ));
    let mean = wide(mean);
    let parents = wide(father) + wide(mother);
    let total = sum(&[
        product(&[mean, DENOMINATOR])?,
        product(&[wide(weight), parents - 2 * mean, MILLION])?,
        product(&[2, wide(spread), factor, z])?,
    ])?;
    Ok(round_div(total, DENOMINATOR))
}

/// A child's personality: each pair is [`child_value`] of the culture mean, the parents' values and a normal variate from the
/// pair's own stream; vice/virtue is held to -7..=7 and manner is not bounded.
pub(crate) fn make_child(
    ctx: &Context<'_>,
    id: u64,
    means: &[i64; PAIR_COUNT],
    father: &PersonalityRecord,
    mother: &PersonalityRecord,
) -> Result<PersonalityRecord, Error> {
    let mut record = PersonalityRecord::default();
    for (index, name) in pair_names().enumerate() {
        let z = normal_z(&mut site(ctx, id, "", &format!("personality.{name}")))?;
        let value = child_value(
            means[index],
            value_at(father, index),
            value_at(mother, index),
            ctx.personality.parents_weight,
            spread_of(ctx.personality, index),
            z,
        )?;
        apply_value(&mut record, index, value)?;
    }
    Ok(record)
}

/// The stress `character` has after a cause adds `amount` (thousandths): the amount scaled by its phlegm and temperance, and for
/// an insult or a defeat by its yellow bile, rounded down, with stress kept within 0..=100. The character is not changed.
///
/// # Errors
/// Refuses a negative amount and arithmetic that does not fit.
pub(crate) fn stress_after(
    rules: &Personality,
    character: &Character,
    amount: i64,
    kind: StressKind,
) -> Result<i64, Error> {
    if amount < 0 {
        return Err(Error::Amount);
    }
    let s = &rules.stress;
    let phlegm = humour_factor(s.intake_per_phlegm, humour(character, PHLEGM))?;
    let temperance = sum(&[
        MILLION,
        product(&[
            wide(s.intake_per_temperance),
            wide(character.personality.vice[GLUTTONY_TEMPERANCE]),
        ])?,
    ])?
    .max(0);
    // Every factor is non-negative, so the product may saturate: the result is held to 0..=100 below.
    let mut numerator = saturating_product(&[wide(amount), phlegm, temperance]);
    let mut denominator = MILLION * MILLION;
    if matches!(kind, StressKind::Insult | StressKind::Defeat) {
        let yellow = humour_factor(s.insult_per_yellow, humour(character, YELLOW_BILE))?;
        numerator = saturating_product(&[numerator, yellow]);
        denominator *= MILLION;
    }
    let gained = numerator.div_euclid(denominator);
    narrow(
        wide(character.personality.stress)
            .saturating_add(gained)
            .min(wide(STRESS_MAX)),
    )
}

/// The personality of `c` after its humour drift and then its standing fade (the first two stages of a birthday).
pub(crate) fn drift_and_fade(
    rules: &Personality,
    c: &Character,
) -> Result<PersonalityRecord, Error> {
    let mut record = c.personality;
    for (pair, humour_index) in rules.drift_humour.iter().enumerate() {
        let Some(humour_index) = humour_index else {
            continue;
        };
        let pull = product(&[
            wide(rules.drift_per_point),
            wide(humour(c, BLOOD + *humour_index)) - 5000,
        ])?;
        let moved = sum(&[wide(record.vice[pair]), -round_div(pull, 1000)])?;
        record.vice[pair] = narrow(moved.clamp(wide(-VICE_BOUND), wide(VICE_BOUND)))?;
    }
    for value in [&mut record.popularity, &mut record.fame, &mut record.honour] {
        *value = narrow(product(&[wide(*value), wide(rules.yearly_keep)])? / 1000)?;
    }
    Ok(record)
}

fn level_of(character: &Character, name: &str) -> usize {
    usize::from(character.genetics.level.get(name).copied().unwrap_or(0))
}

/// A term of a six-entry grade list by level (1..=6); level 0 adds nothing.
fn grade(list: &[i64; 6], level: usize) -> i128 {
    level
        .checked_sub(1)
        .and_then(|index| list.get(index))
        .map_or(0, |term| wide(*term))
}

/// `a`'s opinion of `b` in thousandths of a point (§6): a pure function of the two records, never stored.
///
/// # Errors
/// Refuses arithmetic that does not fit.
pub fn opinion(rules: &Personality, a: &Character, b: &Character) -> Result<i64, Error> {
    let o = &rules.opinion;
    let manner: i128 = a
        .personality
        .manner
        .iter()
        .zip(&b.personality.manner)
        .map(|(x, y)| wide(*x).abs_diff(wide(*y)).cast_signed())
        .sum();
    let humours: i128 = (BLOOD..=PHLEGM)
        .map(|index| {
            wide(humour(a, index))
                .abs_diff(wide(humour(b, index)))
                .cast_signed()
        })
        .sum();
    let manner_max = wide(o.compat_manner_max);
    let humour_max = wide(o.compat_humour_max);
    let manner_term = round_div(manner_max * 9 - manner, 9).clamp(-manner_max, manner_max);
    let humour_term = round_div(humour_max * 4 - humours, 4).clamp(-humour_max, humour_max);
    // The three products each fit `i128` but their sum may not: saturate it (a saturated sum is far past the cap, and a
    // partial sum that saturates leaves at least 2^126 after the last term), then hold it to the cap before dividing.
    let cap = wide(o.standing_cap) * 1000;
    let standing = round_div(
        product(&[wide(o.honour_weight), wide(b.personality.honour)])?
            .saturating_add(product(&[wide(o.fame_weight), wide(b.personality.fame)])?)
            .saturating_add(product(&[
                wide(o.popularity_weight),
                wide(b.personality.popularity),
            ])?)
            .clamp(-cap, cap),
        1000,
    );
    let fool = level_of(b, "fool");
    let power: i128 = if b.office == Some(Office::Ruler) {
        2
    } else {
        0
    };
    let (drunk_a, drunk_b) = (
        usize::from(a.personality.drunkard),
        usize::from(b.personality.drunkard),
    );
    let drunk = usize::from(b.personality.drunkard);
    let kinship = if drunk_a > 0 && drunk_b > 0 {
        grade(&o.drunkard_kinship, drunk_a.min(drunk_b))
    } else {
        0
    };
    narrow(sum(&[
        manner_term,
        humour_term,
        standing,
        grade(&o.beauty, level_of(b, "beauty")),
        grade(&o.ill_favoured, level_of(b, "ill_favoured")),
        grade(&o.fool, fool),
        product(&[grade(&o.fool_in_power, fool), power])?,
        grade(&o.rage_fits, usize::from(b.personality.rage_fits)),
        grade(&o.madness, level_of(b, "madness")),
        grade(&o.drunkard, drunk),
        kinship,
    ])?)
}

/// Whether `birth` has its birthday on `today`: the same month and day, strictly after the birth date; a 29 February birth
/// has it on 1 March in a common year.
pub(crate) fn has_birthday(birth: ck_sim::Date, today: ck_sim::Date) -> bool {
    if (birth.year, birth.month, birth.day) >= (today.year, today.month, today.day) {
        return false;
    }
    (birth.month, birth.day) == (today.month, today.day)
        || ((birth.month, birth.day) == (2, 29)
            && (today.month, today.day) == (3, 1)
            && !is_leap_year(today.year))
}

impl CharacterStore {
    fn mind(&self, id: u64) -> Result<&Character, Error> {
        self.live.get(&id).ok_or(Error::MissingCharacter)
    }

    /// The stress threshold of a live character, in thousandths (§5).
    ///
    /// # Errors
    /// Refuses a missing character.
    pub fn threshold(&self, id: u64) -> Result<i64, Error> {
        Ok(threshold(self.mind(id)?))
    }

    /// Add stress from a cause (§5): `amount` scaled by the character's phlegm and temperance, and for an insult or a defeat
    /// by its yellow bile, rounded down, with stress kept within 0..=100. Returns the new stress.
    ///
    /// # Errors
    /// Refuses a missing character, a negative amount and arithmetic that does not fit; nothing changes.
    pub fn add_stress(
        &mut self,
        rules: &Personality,
        id: u64,
        amount: i64,
        kind: StressKind,
    ) -> Result<i64, Error> {
        let stress = stress_after(rules, self.mind(id)?, amount, kind)?;
        if let Some(c) = self.live.get_mut(&id) {
            c.personality.stress = stress;
        }
        Ok(stress)
    }

    /// The share of its skill a character brings to a task, in thousandths (§5): 1000 at or below the threshold, else
    /// `max(penalty_floor, 1 - penalty_per_point (stress - threshold))` rounded down.
    ///
    /// # Errors
    /// Refuses a missing character and arithmetic that does not fit.
    pub fn performance(&self, rules: &Personality, id: u64) -> Result<i64, Error> {
        let character = self.mind(id)?;
        let over = wide(character.personality.stress) - wide(threshold(character));
        if over <= 0 {
            return Ok(1000);
        }
        let exact = sum(&[
            1_000_000,
            -product(&[wide(rules.stress.penalty_per_point), over])?,
        ])?;
        narrow(exact.div_euclid(1000).max(wide(rules.stress.penalty_floor)))
    }

    /// The multiplier a mind-trait event is given by stress, in thousandths (§5): causing `1 + k excess`, lifting
    /// `1 / (1 + k excess)`, each rounded down.
    ///
    /// # Errors
    /// Refuses a missing character and arithmetic that does not fit.
    pub fn event_multiplier(
        &self,
        rules: &Personality,
        id: u64,
        kind: EventKind,
    ) -> Result<i64, Error> {
        let character = self.mind(id)?;
        let excess = (wide(character.personality.stress) - wide(threshold(character))).max(0);
        let scaled = sum(&[
            MILLION,
            product(&[wide(rules.stress.events_per_point), excess])?,
        ])?;
        // The loader keeps `events_per_point` at zero or above, so `scaled` is at least 1,000,000; hand-built rules that
        // break that would divide by zero or by a negative, so they are a named error.
        if scaled <= 0 {
            return Err(Error::Arithmetic);
        }
        narrow(match kind {
            EventKind::Causing => scaled.div_euclid(1000),
            EventKind::Lifting => (1_000_000_000_i128).div_euclid(scaled),
        })
    }

    /// Indulge a vice (§5; state only: the game calls [`indulge_with_events`](Self::indulge_with_events), which also runs precedence): stress falls by `indulge_relief`, floored at 0, and the pair moves `indulge_step` toward its
    /// sin, clamped at -7.
    ///
    /// # Errors
    /// Refuses a missing character and any name that is not one of the seven vice/virtue pairs; nothing changes.
    pub(crate) fn indulge(
        &mut self,
        rules: &Personality,
        id: u64,
        pair: &str,
    ) -> Result<(), Error> {
        let index = VICE_PAIRS
            .iter()
            .position(|name| *name == pair)
            .ok_or(Error::NotAVicePair)?;
        self.mind(id)?;
        let s = &rules.stress;
        if let Some(c) = self.live.get_mut(&id) {
            c.personality.stress = c.personality.stress.saturating_sub(s.indulge_relief).max(0);
            c.personality.vice[index] = c.personality.vice[index]
                .saturating_sub(s.indulge_step)
                .max(-VICE_BOUND);
        }
        Ok(())
    }

    /// Confess (§5; state only: the game calls [`confess_with_events`](Self::confess_with_events)): stress falls by `confess_relief`, floored at 0, and the most sinful pair (the lowest value, ties in
    /// data order) moves `confess_step` toward its virtue, clamped at +7.
    ///
    /// # Errors
    /// Refuses a missing character.
    pub(crate) fn confess(&mut self, rules: &Personality, id: u64) -> Result<(), Error> {
        self.mind(id)?;
        let s = &rules.stress;
        if let Some(c) = self.live.get_mut(&id) {
            c.personality.stress = c.personality.stress.saturating_sub(s.confess_relief).max(0);
            let worst = c
                .personality
                .vice
                .iter()
                .enumerate()
                .min_by_key(|(index, value)| (**value, *index))
                .map_or(0, |(index, _)| index);
            c.personality.vice[worst] = c.personality.vice[worst]
                .saturating_add(s.confess_step)
                .min(VICE_BOUND);
        }
        Ok(())
    }

    /// Change a standing value by `delta` thousandths with a checked add; slice 4's deeds call it.
    ///
    /// # Errors
    /// Refuses a missing character and a result outside the signed 64-bit range ([`Error::StandingRange`]); the value is
    /// unchanged.
    pub fn change_standing(&mut self, id: u64, which: Standing, delta: i64) -> Result<(), Error> {
        let character = self.live.get_mut(&id).ok_or(Error::MissingCharacter)?;
        let slot = match which {
            Standing::Popularity => &mut character.personality.popularity,
            Standing::Fame => &mut character.personality.fame,
            Standing::Honour => &mut character.personality.honour,
        };
        *slot = slot.checked_add(delta).ok_or(Error::StandingRange)?;
        Ok(())
    }

    /// Set the level (0..=6) of a trait that events set (slice 4).
    ///
    /// # Errors
    /// Refuses a missing character and a level above 6.
    pub(crate) fn set_trait_level(
        &mut self,
        id: u64,
        which: LevelTrait,
        level: u8,
    ) -> Result<(), Error> {
        if level > MAX_LEVEL {
            return Err(Error::Level);
        }
        let character = self.live.get_mut(&id).ok_or(Error::MissingCharacter)?;
        match which {
            LevelTrait::Drunkard => character.personality.drunkard = level,
            LevelTrait::RageFits => character.personality.rage_fits = level,
            LevelTrait::Melancholy => character.personality.melancholy = level,
        }
        Ok(())
    }

    /// `a`'s opinion of `b` in thousandths (§6). Reading it changes nothing.
    ///
    /// # Errors
    /// Refuses a missing character and arithmetic that does not fit.
    pub fn opinion(&self, rules: &Personality, a: u64, b: u64) -> Result<i64, Error> {
        opinion(rules, self.mind(a)?, self.mind(b)?)
    }

    /// The monthly recovery pass (§4), for every live character in id order: stress falls by
    /// `stress * monthly_recovery * f_blood * f_black * f_melancholy`, rounded down. All results are computed before any is
    /// written, so an error changes nothing.
    ///
    /// # Errors
    /// Refuses arithmetic that does not fit.
    pub fn monthly_recovery(&mut self, rules: &Personality) -> Result<(), Error> {
        let s = &rules.stress;
        let mut next = Vec::new();
        for (id, c) in &self.live {
            // Every factor is non-negative, so the product may saturate: the result is held to 0..=100 below.
            let numerator = saturating_product(&[
                wide(c.personality.stress),
                wide(s.monthly_recovery),
                humour_factor(s.recovery_per_blood, humour(c, BLOOD))?,
                humour_factor(s.recovery_per_black, humour(c, BLACK_BILE))?,
                grade_factor(s.recovery_per_melancholy_grade, c.personality.melancholy)?,
            ]);
            let deduction = numerator.div_euclid(1000 * MILLION * MILLION * 1000);
            next.push((
                *id,
                narrow((wide(c.personality.stress) - deduction).clamp(0, wide(STRESS_MAX)))?,
            ));
        }
        for (id, stress) in next {
            if let Some(c) = self.live.get_mut(&id) {
                c.personality.stress = stress;
            }
        }
        Ok(())
    }

    /// The birthday pass (§4), for the live characters whose birthday is `today` and whose birth date is before it: humour
    /// drift (`pair -= per_point (humour - 5)`, rounded to the nearest thousandth, ties away from zero, held to -7..=7) and
    /// then the standing fade (`value * yearly_keep`, toward zero). All results are computed before any is written.
    ///
    /// # Errors
    /// Refuses a date outside the calendar and arithmetic that does not fit.
    pub fn birthday_pass(&mut self, rules: &Personality, today: GameTime) -> Result<(), Error> {
        let today = today.date().map_err(|_| Error::Overflow)?;
        let mut next = Vec::new();
        for (id, c) in &self.live {
            let birth = c.birth.date().map_err(|_| Error::Overflow)?;
            if !has_birthday(birth, today) {
                continue;
            }
            next.push((*id, drift_and_fade(rules, c)?));
        }
        for (id, record) in next {
            if let Some(c) = self.live.get_mut(&id) {
                c.personality = record;
            }
        }
        Ok(())
    }
}
