//! A character's genetic record and how it is made (CK-361, characters slice 2).
//!
//! No floating point: values are integers in thousandths, probabilities integers in parts per billion. Every random draw
//! comes from a stream named for the character and the draw site, so a creation appended after another leaves it alone.
#![deny(clippy::float_arithmetic)]

use crate::{Error, Sex, character_stream};
use ck_economy::{
    Genetics, PopGenetics,
    genetics::{
        ATTRIBUTE_COUNT, ATTRIBUTES, Dominance, Mendelian, Mode, PPB, ShowsIn, clamp_attribute,
        round_div,
    },
};
use ck_sim::{Canonical, CanonicalError, GameTime, Reader, Stream};
use std::collections::BTreeMap;

/// What creating a character needs besides the character's own data: the rules, the world seed and the tick the
/// character's streams run at.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    /// The loaded genetic rules.
    pub rules: &'a Genetics,
    /// The world seed.
    pub seed: u64,
    /// The tick the creation's streams are drawn at.
    pub tick: GameTime,
}

/// One Mendelian condition in a character: how many copies, and whether it shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mendel {
    /// Copies carried (0..=2).
    pub copies: u8,
    /// Whether the condition shows (decided once, at birth).
    pub shows: bool,
}

/// A character's genetics, final at creation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GeneticRecord {
    /// Paternal haplogroup (a daughter records her father's line).
    pub paternal: String,
    /// Maternal haplogroup.
    pub maternal: String,
    /// Baseline mean per attribute, in thousandths, never clamped.
    pub mean: [i64; ATTRIBUTE_COUNT],
    /// Baseline spread per attribute, in thousandths, never clamped.
    pub sd: [i64; ATTRIBUTE_COUNT],
    /// Genetic value per attribute, in thousandths, clamped at birth. A woman's offset is not part of it.
    pub value: [i64; ATTRIBUTE_COUNT],
    /// Mendelian conditions by name.
    pub mendelian: BTreeMap<String, Mendel>,
    /// Graded trait levels by name (0 = none).
    pub level: BTreeMap<String, u8>,
}

impl GeneticRecord {
    /// The shown value of an attribute: the genetic value plus a woman's offset, held to the attribute's bounds. It is
    /// computed on demand and never stored, so it never passes to a child.
    #[must_use]
    pub fn shown(&self, rules: &Genetics, sex: Sex, attribute: usize) -> i64 {
        let offset = match sex {
            Sex::Female => rules.female_offset(attribute),
            Sex::Male => 0,
        };
        clamp_attribute(attribute, self.value[attribute].saturating_add(offset))
    }
}

impl Canonical for Mendel {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.copies.encode(out)?;
        self.shows.encode(out)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            copies: u8::decode(r)?,
            shows: bool::decode(r)?,
        })
    }
}

impl Canonical for GeneticRecord {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.paternal.encode(out)?;
        self.maternal.encode(out)?;
        for value in self.mean.iter().chain(&self.sd).chain(&self.value) {
            value.encode(out)?;
        }
        self.mendelian.encode(out)?;
        self.level.encode(out)
    }
    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let mut record = Self {
            paternal: String::decode(r)?,
            maternal: String::decode(r)?,
            ..Self::default()
        };
        for slot in record
            .mean
            .iter_mut()
            .chain(record.sd.iter_mut())
            .chain(record.value.iter_mut())
        {
            *slot = i64::decode(r)?;
        }
        record.mendelian = BTreeMap::decode(r)?;
        record.level = BTreeMap::decode(r)?;
        Ok(record)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Arithmetic

/// Whether a chance `p` (ppb) succeeds on a uniform draw below one billion: `draw < p`.
#[must_use]
pub fn succeeds(draw: u64, p: u32) -> bool {
    draw < u64::from(p)
}

fn below(stream: &mut Stream, n: u64) -> Result<u64, Error> {
    stream.below(n).map_err(|_| Error::Genetics)
}

fn roll(stream: &mut Stream, p: u32) -> Result<bool, Error> {
    Ok(succeeds(below(stream, u64::from(PPB))?, p))
}

/// A normal draw with spread `numerator / denominator` (thousandths): `z` is the sum of twelve uniform draws below
/// 1,000,000 minus 6,000,000 (so in millionths), and the value is `mean + spread * z / 1,000,000` rounded to the nearest
/// thousandth, ties away from zero (the whole value, not the offset alone). The spread stays an exact fraction.
fn normal_exact(
    stream: &mut Stream,
    mean: i64,
    numerator: i128,
    denominator: i128,
) -> Result<i64, Error> {
    let mut z = -6_000_000_i128;
    for _ in 0..12 {
        z += i128::from(below(stream, 1_000_000)?);
    }
    // One rounding of the whole value (`mean + spread * z / 1,000,000`), so a tie goes away from zero as a whole.
    let scale = denominator * 1_000_000;
    let value = round_div(i128::from(mean) * scale + numerator * z, scale);
    i64::try_from(value).map_err(|_| Error::Genetics)
}

/// A normal draw with mean `mean` and spread `spread` (both in thousandths).
///
/// # Errors
/// Refuses a result that does not fit.
pub fn normal_draw(stream: &mut Stream, mean: i64, spread: i64) -> Result<i64, Error> {
    normal_exact(stream, mean, i128::from(spread), 1)
}

/// A level `1..=max` weighted `q^(n-1)`: integer weights in ppb by repeated multiplication rounding down, then a uniform
/// draw below their sum, levels in ascending order.
fn weighted_level(stream: &mut Stream, q: u32, max: u8) -> Result<u8, Error> {
    let mut weights = Vec::with_capacity(usize::from(max));
    let mut weight = u128::from(PPB);
    for _ in 0..max {
        weights.push(weight);
        weight = weight * u128::from(q) / u128::from(PPB);
    }
    let total: u128 = weights.iter().sum();
    let mut pick = u128::from(below(
        stream,
        u64::try_from(total).map_err(|_| Error::Genetics)?,
    )?);
    for (index, weight) in weights.iter().enumerate() {
        if pick < *weight {
            return u8::try_from(index + 1).map_err(|_| Error::Genetics);
        }
        pick -= *weight;
    }
    Err(Error::Genetics)
}

/// A haplogroup drawn from a share set of thousandths, keys in ascending order.
fn pick_haplogroup(stream: &mut Stream, shares: &BTreeMap<String, u32>) -> Result<String, Error> {
    let total: u64 = shares.values().map(|share| u64::from(*share)).sum();
    if total == 0 {
        return Err(Error::Genetics);
    }
    let mut pick = below(stream, total)?;
    for (name, share) in shares {
        let share = u64::from(*share);
        if pick < share {
            return Ok(name.clone());
        }
        pick -= share;
    }
    Err(Error::Genetics)
}

// ---------------------------------------------------------------------------------------------------------------------
// Expression

/// Whether a condition can show with `copies` copies in a character of `sex`: dominant with one or more, recessive with
/// two (a man's single X copy counts), `y` and `maternal` when present.
fn can_show(rule: &Mendelian, sex: Sex, copies: u8) -> bool {
    match rule.mode {
        Mode::Y | Mode::Maternal => copies >= 1,
        Mode::Ordinary | Mode::X => match rule.dominance {
            Dominance::Dominant => copies >= 1,
            Dominance::Recessive if rule.mode == Mode::X && sex == Sex::Male => copies >= 1,
            Dominance::Recessive => copies >= 2,
        },
    }
}

/// One penetrance roll at birth for a condition that can show, then the sex limit.
fn express(rule: &Mendelian, sex: Sex, copies: u8, stream: &mut Stream) -> Result<bool, Error> {
    if !can_show(rule, sex, copies) {
        return Ok(false);
    }
    let rolled = roll(stream, rule.penetrance)?;
    let allowed = match rule.shows_in {
        ShowsIn::Both => true,
        ShowsIn::Women => sex == Sex::Female,
        ShowsIn::Men => sex == Sex::Male,
    };
    Ok(rolled && allowed)
}

// ---------------------------------------------------------------------------------------------------------------------
// Making a record

/// The stream of one draw site for character `id`; `prefix` names a synthetic parent's sites.
fn site(ctx: &Context<'_>, id: u64, prefix: &str, name: &str) -> Stream {
    character_stream(ctx.seed, id, &format!("{prefix}{name}"), ctx.tick)
}

/// A founder: haplogroups from the pop's share sets, values from the pop's baseline, copies from the pop's carrier rates
/// and graded levels from its founder rates. `prefix` is empty for a character's own sites and `synthetic_father.` or
/// `synthetic_mother.` for a synthetic parent.
pub(crate) fn make_founder(
    ctx: &Context<'_>,
    id: u64,
    prefix: &str,
    sex: Sex,
    pop: &PopGenetics,
) -> Result<GeneticRecord, Error> {
    let mut record = GeneticRecord {
        paternal: pick_haplogroup(
            &mut site(ctx, id, prefix, "haplogroup.paternal"),
            &pop.paternal,
        )?,
        maternal: pick_haplogroup(
            &mut site(ctx, id, prefix, "haplogroup.maternal"),
            &pop.maternal,
        )?,
        mean: pop.mean,
        sd: pop.sd,
        ..GeneticRecord::default()
    };
    for (attribute, name) in ATTRIBUTES.iter().enumerate() {
        let mut stream = site(ctx, id, prefix, &format!("value.{name}"));
        let drawn = normal_draw(&mut stream, pop.mean[attribute], pop.sd[attribute])?;
        record.value[attribute] = clamp_attribute(attribute, drawn);
    }
    for (name, rule) in ctx.rules.mendelian() {
        let rate = pop.carrier.get(name).copied().unwrap_or(0);
        let mut stream = site(ctx, id, prefix, &format!("copies.{name}"));
        let draws = match (rule.mode, sex) {
            (Mode::Ordinary, _) | (Mode::X, Sex::Female) => 2,
            (Mode::X | Mode::Y, Sex::Male) | (Mode::Maternal, _) => 1,
            (Mode::Y, Sex::Female) => 0,
        };
        let mut copies = 0_u8;
        for _ in 0..draws {
            copies += u8::from(roll(&mut stream, rate)?);
        }
        record.mendelian.insert(
            name.clone(),
            Mendel {
                copies,
                shows: express(
                    rule,
                    sex,
                    copies,
                    &mut site(ctx, id, prefix, &format!("shows.{name}")),
                )?,
            },
        );
    }
    for (name, rule) in ctx.rules.graded() {
        let rate = pop.founder.get(name).copied().unwrap_or(0);
        let mut stream = site(ctx, id, prefix, &format!("graded.{name}"));
        let level = if roll(&mut stream, rate)? {
            weighted_level(&mut stream, rule.founder_q, rule.max)?
        } else {
            0
        };
        record.level.insert(name.clone(), level);
    }
    Ok(record)
}

fn copies_of(record: &GeneticRecord, name: &str) -> u8 {
    record.mendelian.get(name).map_or(0, |m| m.copies)
}

/// The copies a parent passes on when one of its alleles is picked 50:50: a parent with `copies` copies of two passes the
/// condition with chance `copies / 2`. The draw is made whatever the parent carries, so the stream stays aligned.
fn pass_on(stream: &mut Stream, copies: u8) -> Result<u8, Error> {
    Ok(u8::from(below(stream, 2)? < u64::from(copies)))
}

/// A child of `father` and `mother` (their records, or synthetic founders already made).
pub(crate) fn make_child(
    ctx: &Context<'_>,
    id: u64,
    sex: Sex,
    father: &GeneticRecord,
    mother: &GeneticRecord,
) -> Result<GeneticRecord, Error> {
    let mut record = GeneticRecord {
        paternal: father.paternal.clone(),
        maternal: mother.maternal.clone(),
        ..GeneticRecord::default()
    };
    for attribute in 0..ATTRIBUTE_COUNT {
        record.mean[attribute] = i64::try_from(round_div(
            i128::from(father.mean[attribute]) + i128::from(mother.mean[attribute]),
            2,
        ))
        .map_err(|_| Error::Genetics)?;
        record.sd[attribute] = i64::try_from(round_div(
            i128::from(father.sd[attribute]) + i128::from(mother.sd[attribute]),
            2,
        ))
        .map_err(|_| Error::Genetics)?;
    }
    for (attribute, name) in ATTRIBUTES.iter().enumerate() {
        let mean = i128::from(record.mean[attribute]);
        let parents = i128::from(father.value[attribute]) + i128::from(mother.value[attribute]);
        // mean_b + h * (average(F, M) - mean_b) + e, with e a normal draw of spread sd_b * f / 1000.
        let pull = round_div(
            i128::from(ctx.rules.heritability(attribute)) * (parents - 2 * mean),
            2000,
        );
        let mut stream = site(ctx, id, "", &format!("value.{name}"));
        let noise = normal_exact(
            &mut stream,
            0,
            i128::from(record.sd[attribute]) * i128::from(ctx.rules.spread(attribute)),
            1000,
        )?;
        let value = i64::try_from(mean + pull + i128::from(noise)).map_err(|_| Error::Genetics)?;
        record.value[attribute] = clamp_attribute(attribute, value);
    }
    for (name, rule) in ctx.rules.mendelian() {
        let mut stream = site(ctx, id, "", &format!("copies.{name}"));
        let (from_father, from_mother) = (copies_of(father, name), copies_of(mother, name));
        let copies = match (rule.mode, sex) {
            (Mode::Ordinary, _) => {
                pass_on(&mut stream, from_father)? + pass_on(&mut stream, from_mother)?
            }
            (Mode::X, Sex::Male) => pass_on(&mut stream, from_mother)?,
            (Mode::X, Sex::Female) => pass_on(&mut stream, from_mother)? + from_father.min(1),
            (Mode::Y, Sex::Male) => from_father.min(1),
            (Mode::Y, Sex::Female) => 0,
            (Mode::Maternal, _) => from_mother.min(1),
        };
        record.mendelian.insert(
            name.clone(),
            Mendel {
                copies,
                shows: express(
                    rule,
                    sex,
                    copies,
                    &mut site(ctx, id, "", &format!("shows.{name}")),
                )?,
            },
        );
    }
    for (name, rule) in ctx.rules.graded() {
        let level_of =
            |record: &GeneticRecord| i128::from(record.level.get(name).copied().unwrap_or(0));
        let (m, f) = (level_of(mother), level_of(father));
        let w = i128::from(rule.w);
        // mu = w * M + (1 - w) * F - d and sigma = s0 + k * |M - F|, in thousandths of a level.
        let mu = w * m + (1000 - w) * f - i128::from(rule.d);
        let sigma = i128::from(rule.s0) + i128::from(rule.k) * (m - f).abs();
        let mut stream = site(ctx, id, "", &format!("graded.{name}"));
        let drawn = normal_exact(
            &mut stream,
            i64::try_from(mu).map_err(|_| Error::Genetics)?,
            sigma,
            1,
        )?;
        let rounded = round_div(i128::from(drawn), 1000).clamp(0, i128::from(rule.max));
        let mut level = u8::try_from(rounded).map_err(|_| Error::Genetics)?;
        if roll(&mut stream, rule.m)? {
            level = level.max(weighted_level(&mut stream, rule.q, rule.max)?);
        }
        record.level.insert(name.clone(), level);
    }
    Ok(record)
}
