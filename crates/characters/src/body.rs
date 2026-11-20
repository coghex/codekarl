//! Upbringing, height and weight (CK-367, characters slice 5d).
//!
//! **Upbringing.** Each birthday before `to_age`, every vice/virtue and manner value moves a share of the way toward the average of the
//! character's parents and siblings (as they stood when the day's birthday pass began) and takes a normal draw of the data's noise.
//!
//! **Height.** A character's *base height* is its genetic height plus a woman's offset; its *shown height* is that plus the
//! nutrition offset, held to the floor once, last. The offset comes from the food of childhood: one sample of the pop's necessities
//! at birth (or at creation) and one on each birthday from 1 through 18, kept as a count and a sum.
//!
//! **Weight.** A stored shown weight moves an exact twelfth of the way to its target on the 1st of each month.
//!
//! Every shown value is read through [`Character::shown_height`], [`Character::shown_weight`] and [`Character::shown_stat`].
//!
//! No floating point: everything is exact over integers (thousandths) and rounded once to the thousandth, ties away from zero.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, Character, CharacterStore, Context, Error, Populations, character_stream,
    events::{EventError, FirstError, LogSink, World, age_years, keep},
    genetics::normal_z,
    personality::{product, sum},
    skills::Snapshot,
};
use ck_economy::{
    Genetics,
    bigint::Big,
    genetics::{clamp_attribute, round_div},
    lifecycle::{ADULT_AGE, NutritionRules, UpbringingRules},
    personality::{MANNER_PAIRS, VICE_BOUND, VICE_PAIRS},
};
use ck_sim::GameTime;
use std::cmp::Ordering;

/// The index of `gluttony_temperance` among the vice/virtue pairs.
const GLUTTONY_TEMPERANCE: usize = 4;
/// Index of height in a genetic attribute array.
const HEIGHT: usize = 0;
/// Index of weight in a genetic attribute array.
const WEIGHT: usize = 1;
/// A month's share of the way to the target weight: a twelfth.
const MONTHS: i64 = 12;

fn narrow(value: i128) -> Result<i64, Error> {
    i64::try_from(value).map_err(|_| Error::Arithmetic)
}

impl Character {
    /// The base value of an attribute: its genetic value plus a woman's offset, not held to any bound.
    #[must_use]
    pub fn base_stat(&self, rules: &Genetics, attribute: usize) -> i64 {
        let offset = match self.sex {
            crate::Sex::Female => rules.female_offset(attribute),
            crate::Sex::Male => 0,
        };
        self.genetics.value[attribute].saturating_add(offset)
    }

    /// The base height in thousandths of a centimetre: the genetic height plus a woman's offset. The hereditary inputs never change.
    #[must_use]
    pub fn base_height(&self, rules: &Genetics) -> i64 {
        self.base_stat(rules, HEIGHT)
    }

    /// The base weight in thousandths of a kilogram: the genetic weight plus a woman's offset.
    #[must_use]
    pub fn base_weight(&self, rules: &Genetics) -> i64 {
        self.base_stat(rules, WEIGHT)
    }

    /// The shown height: the base height plus the nutrition offset, held to the floor once, last.
    #[must_use]
    pub fn shown_height(&self, rules: &Genetics) -> i64 {
        clamp_attribute(
            HEIGHT,
            self.base_height(rules)
                .saturating_add(self.life.height_offset),
        )
    }

    /// The shown weight: the stored weight, which the monthly pass moves toward its target.
    #[must_use]
    pub fn shown_weight(&self) -> i64 {
        self.life.weight
    }

    /// The shown value of any stat: height and weight as above, every other stat its genetic value and a woman's offset held to the
    /// attribute's bounds. Every consumer reads a shown body value through this route or the two above.
    #[must_use]
    pub fn shown_stat(&self, rules: &Genetics, attribute: usize) -> i64 {
        match attribute {
            HEIGHT => self.shown_height(rules),
            WEIGHT => self.shown_weight(),
            _ => self.genetics.shown(rules, self.sex, attribute),
        }
    }
}

/// The nutrition offset of a height, in thousandths of a centimetre, from `count` samples of the pop's necessities (permille) that
/// sum to `sum`, for a character of `age` years.
///
/// With `mean = sum / count`, the adult offset is `clamp(height_per_food_point x (mean - height_reference), height_range)`; before 18
/// the offset is the adult offset x `age / 18`. It is computed exactly and rounded once, to the nearest thousandth, ties away from zero.
/// A record with no samples has no offset.
///
/// # Errors
/// Arithmetic that does not fit.
pub fn height_offset(
    rules: &NutritionRules,
    count: u32,
    sum_pm: u64,
    age: u32,
) -> Result<i64, Error> {
    if count == 0 {
        return Ok(0);
    }
    let age = i128::from(age.min(ADULT_AGE));
    let count = i128::from(count);
    let denominator = 1000 * count;
    // per x (sum / count - reference / 1000), over `1000 count`, in unbounded integers: a coefficient the loader admits must reach
    // the cap whatever its size, and never fail on the way to it.
    let adult = Signed::of(&[
        i128::from(rules.height_per_food_milli),
        i128::from(sum_pm) * 1000 - i128::from(rules.height_reference_milli) * count,
    ]);
    let low = Signed::of(&[i128::from(rules.height_low), denominator]);
    let high = Signed::of(&[i128::from(rules.height_high), denominator]);
    let adult = if adult.cmp(&high) == Ordering::Greater {
        high
    } else if adult.cmp(&low) == Ordering::Less {
        low
    } else {
        adult
    };
    adult
        .times(age.unsigned_abs())
        .round_div(&Big::new(
            denominator.unsigned_abs() * u128::from(ADULT_AGE),
        ))
        .to_i64()
}

/// The target weight in thousandths of a kilogram: `base x (weight_base + weight_food x necessities / 1000 + weight_rich x luxuries /
/// 1000) x (1 + weight_gluttony x max(0, -gluttony_temperance))`, exact and rounded once. `gluttony_temperance` is in thousandths.
///
/// # Errors
/// Arithmetic that does not fit.
pub fn weight_target(
    rules: &NutritionRules,
    base: i64,
    necessities_pm: u16,
    luxuries_pm: u16,
    gluttony_temperance: i64,
) -> Result<i64, Error> {
    // The food share is over 1e9 (millionths x 1000), the gluttony share over 1e9 (millionths x thousandths of a point).
    let food = sum(&[
        product(&[i128::from(rules.weight_base), 1000])?,
        product(&[i128::from(rules.weight_food), i128::from(necessities_pm)])?,
        product(&[i128::from(rules.weight_rich), i128::from(luxuries_pm)])?,
    ])?;
    let glutton = sum(&[
        1_000_000_000,
        product(&[
            i128::from(rules.weight_gluttony),
            i128::from(gluttony_temperance.saturating_neg().max(0)),
        ])?,
    ])?;
    narrow(round_div(
        product(&[i128::from(base), food, glutton])?,
        1_000_000_000_000_000_000,
    ))
}

/// One personality value after a year's upbringing: `old + pull x (household average - old) + spread x z / 1,000,000`, `z` the unrounded
/// normal variate in millionths, evaluated exactly (in unbounded integers, so no value the data admits fails before the bound applies)
/// and rounded once. With no household (`members == 0`) only the noise applies.
fn brought_up(
    old: i64,
    household_sum: i128,
    members: i128,
    pull_ppb: u32,
    spread: i64,
    z: i128,
) -> Signed {
    let old = i128::from(old);
    let (household_sum, members) = if members == 0 {
        (old, 1)
    } else {
        (household_sum, members)
    };
    // Over `members x 1e9 x 1e6`.
    let denominator = members.unsigned_abs() * 1_000_000_000_000_000;
    Signed::add(&[
        Signed::of(&[old, members, 1_000_000_000_000_000]),
        Signed::of(&[
            i128::from(pull_ppb),
            1_000_000,
            household_sum - members * old,
        ]),
        Signed::of(&[i128::from(spread), z, members, 1_000_000_000]),
    ])
    .round_div(&Big::new(denominator))
}

/// An integer of any size, for the arithmetic whose intermediate values the loader's domain does not bound.
#[derive(Clone, Debug)]
struct Signed {
    negative: bool,
    magnitude: Big,
}

impl Signed {
    fn zero_checked(negative: bool, magnitude: Big) -> Self {
        let zero = magnitude.cmp(&Big::new(0)) == Ordering::Equal;
        Self {
            negative: negative && !zero,
            magnitude,
        }
    }

    /// The product of `factors`.
    fn of(factors: &[i128]) -> Self {
        let negative = factors.iter().filter(|factor| **factor < 0).count() % 2 == 1;
        let magnitude = factors.iter().fold(Big::new(1), |total, factor| {
            total.mul(&Big::new(factor.unsigned_abs()))
        });
        Self::zero_checked(negative, magnitude)
    }

    /// This times a non-negative factor.
    fn times(&self, factor: u128) -> Self {
        Self::zero_checked(self.negative, self.magnitude.mul(&Big::new(factor)))
    }

    /// The sum of `terms`.
    fn add(terms: &[Self]) -> Self {
        let (mut positive, mut negative) = (Big::new(0), Big::new(0));
        for term in terms {
            if term.negative {
                negative = negative.add(&term.magnitude);
            } else {
                positive = positive.add(&term.magnitude);
            }
        }
        match positive.cmp(&negative) {
            Ordering::Less => {
                Self::zero_checked(true, negative.sub(&positive).unwrap_or_else(|| Big::new(0)))
            }
            _ => Self::zero_checked(
                false,
                positive.sub(&negative).unwrap_or_else(|| Big::new(0)),
            ),
        }
    }

    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => self.magnitude.cmp(&other.magnitude),
            (true, true) => other.magnitude.cmp(&self.magnitude),
        }
    }

    /// This over `divisor`, rounded to the nearest integer, ties away from zero.
    fn round_div(&self, divisor: &Big) -> Self {
        Self::zero_checked(self.negative, self.magnitude.round_div(divisor))
    }

    /// The value, or an arithmetic error when it does not fit 64 bits.
    fn to_i64(&self) -> Result<i64, Error> {
        let magnitude = self.magnitude.to_u128().ok_or(Error::Arithmetic)?;
        let value = i128::try_from(magnitude).map_err(|_| Error::Arithmetic)?;
        narrow(if self.negative { -value } else { value })
    }

    /// The value held to `-bound..=bound`, however large it is.
    fn clamped(&self, bound: i64) -> i64 {
        if self.magnitude.cmp(&Big::new(bound.unsigned_abs().into())) == Ordering::Greater {
            if self.negative { -bound } else { bound }
        } else {
            self.to_i64().unwrap_or(0)
        }
    }
}

impl CharacterStore {
    /// The first sample of a new character's food record and its body (CK-367 §2): its weight starts at its base weight (floored), and
    /// one sample of its pop's necessities, taken at its age on `ctx.tick`, starts its record, from which its height offset follows.
    /// A pop whose necessities the source cannot give leaves the record empty and the offset 0.
    pub(crate) fn start_body(
        ctx: &Context<'_>,
        character: &mut Character,
        populations: &impl Populations,
    ) -> Result<(), Error> {
        character.life.weight = clamp_attribute(WEIGHT, character.base_weight(ctx.rules));
        let birth = character.birth.date().map_err(|_| Error::Event)?;
        let today = ctx.tick.date().map_err(|_| Error::Event)?;
        let age = age_years(birth, today);
        if let Some(necessities) = populations.necessities_pm(&character.pop) {
            character.life.food_count = 1;
            character.life.food_sum = u64::from(necessities);
            character.life.food_age = age;
            character.life.height_offset = height_offset(
                &ctx.lifecycle.nutrition,
                character.life.food_count,
                character.life.food_sum,
                age,
            )?;
        }
        Ok(())
    }

    /// Stage 6a of a birthday (CK-367 §1): while the character is younger than `to_age`, every vice/virtue and manner value moves `pull`
    /// of the way toward the household average, taken from the snapshot, plus a normal draw of the yearly noise on purpose
    /// `upbringing.<pair>`; vice/virtue is held to -7..=7 and manner is not bounded. All values are computed before any is stored.
    pub(crate) fn upbringing<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &World<'_, P, A, L>,
        id: u64,
        today: GameTime,
        age: u32,
        snapshot: &Snapshot,
    ) -> Result<(), Error> {
        let rules: &UpbringingRules = &world.lifecycle.upbringing;
        if age >= rules.to_age {
            return Ok(());
        }
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        let household = snapshot.household(c);
        let mut moved = c.personality;
        let pairs = VICE_PAIRS.iter().chain(&MANNER_PAIRS).copied();
        for (index, name) in pairs.enumerate() {
            let is_vice = index < VICE_PAIRS.len();
            let pick = |mind: &([i64; 7], [i64; 9])| {
                if is_vice {
                    mind.0[index]
                } else {
                    mind.1[index - VICE_PAIRS.len()]
                }
            };
            let total: i128 = household.iter().map(|mind| i128::from(pick(mind))).sum();
            let z = normal_z(&mut character_stream(
                world.seed,
                id,
                &format!("upbringing.{name}"),
                today,
            ))?;
            let old = if is_vice {
                c.personality.vice[index]
            } else {
                c.personality.manner[index - VICE_PAIRS.len()]
            };
            let value = brought_up(
                old,
                total,
                i128::try_from(household.len()).map_err(|_| Error::Arithmetic)?,
                rules.pull_ppb,
                if is_vice {
                    rules.noise_vice_virtue
                } else {
                    rules.noise_manner
                },
                z,
            );
            if is_vice {
                moved.vice[index] = value.clamped(VICE_BOUND);
            } else {
                moved.manner[index - VICE_PAIRS.len()] = value.to_i64()?;
            }
        }
        if let Some(c) = self.live.get_mut(&id) {
            c.personality = moved;
        }
        Ok(())
    }

    /// Stage 6b of a birthday (CK-367 §2): on each birthday from 1 through 18 the pop's necessities that day are one more sample of the
    /// childhood food record, and the height offset follows from it. A birthday already sampled (a character created on its own
    /// birthday) and one past 18 add nothing.
    pub(crate) fn height_sample<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &World<'_, P, A, L>,
        id: u64,
        age: u32,
    ) -> Result<(), Error> {
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        if age > ADULT_AGE || age <= c.life.food_age {
            return Ok(());
        }
        let Some(necessities) = world.populations.necessities_pm(&c.pop) else {
            return Ok(());
        };
        let count = c.life.food_count.checked_add(1).ok_or(Error::Arithmetic)?;
        let total = c
            .life
            .food_sum
            .checked_add(u64::from(necessities))
            .ok_or(Error::Arithmetic)?;
        let offset = height_offset(&world.lifecycle.nutrition, count, total, age)?;
        if let Some(c) = self.live.get_mut(&id) {
            c.life.food_count = count;
            c.life.food_sum = total;
            c.life.food_age = age;
            c.life.height_offset = offset;
        }
        Ok(())
    }

    /// The birth-day sample of every character born today (CK-367 §2), phase C of the day, **after** the economy's step: a delivery
    /// commits before the step (CK-364 §4) and so sees the pop's necessities of the day before the step changes them, while the sample
    /// is the pop's value that day. The sample taken at creation is replaced, so the record still holds exactly one sample for the
    /// birth. When the step fails this pass does not run and the creation sample stands. A pop whose necessities the source cannot
    /// give leaves the record as it was.
    ///
    /// # Errors
    /// The first failure, once every character has been looked at.
    pub fn run_birth_samples<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        today: GameTime,
    ) -> Result<(), EventError> {
        let day = today.date().map_err(|_| Error::Event)?;
        let mut first: FirstError = None;
        let born: Vec<u64> = self
            .live
            .values()
            .filter(|c| {
                c.birth
                    .date()
                    .is_ok_and(|b| (b.year, b.month, b.day) == (day.year, day.month, day.day))
            })
            .map(|c| c.id)
            .collect();
        for id in born {
            let Some(c) = self.live.get_mut(&id) else {
                continue;
            };
            let Some(necessities) = world.populations.necessities_pm(&c.pop) else {
                continue;
            };
            c.life.food_count = 1;
            c.life.food_sum = u64::from(necessities);
            c.life.food_age = 0;
            match height_offset(&world.lifecycle.nutrition, 1, c.life.food_sum, 0) {
                Ok(offset) => c.life.height_offset = offset,
                Err(error) => keep(&mut first, error.into()),
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// The monthly weights, on the 1st after the deliveries and the conceptions (CK-367 §3): for every live character in identifier
    /// order, the stored shown weight moves an exact twelfth (integer division) of the way to its target, then is held to the floor.
    /// A pop whose necessities or luxuries the source cannot give leaves its characters' weights as they were.
    ///
    /// # Errors
    /// The first failure, once every character has been looked at.
    pub fn run_monthly_weights<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
    ) -> Result<(), EventError> {
        let ids: Vec<u64> = self.live.keys().copied().collect();
        let mut first: FirstError = None;
        for id in ids {
            if let Err(error) = self.monthly_weight(world, id) {
                keep(&mut first, error.into());
            }
        }
        first.map_or(Ok(()), Err)
    }

    fn monthly_weight<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &World<'_, P, A, L>,
        id: u64,
    ) -> Result<(), Error> {
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        let (Some(necessities), Some(luxuries)) = (
            world.populations.necessities_pm(&c.pop),
            world.populations.luxuries_pm(&c.pop),
        ) else {
            return Ok(());
        };
        let target = weight_target(
            &world.lifecycle.nutrition,
            c.base_weight(world.genetics),
            necessities,
            luxuries,
            c.personality.vice[GLUTTONY_TEMPERANCE],
        )?;
        let step = target.checked_sub(c.life.weight).ok_or(Error::Arithmetic)? / MONTHS;
        let moved = c.life.weight.checked_add(step).ok_or(Error::Arithmetic)?;
        if let Some(c) = self.live.get_mut(&id) {
            c.life.weight = clamp_attribute(WEIGHT, moved);
        }
        Ok(())
    }
}
