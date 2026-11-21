//! Connections and passions (CK-368, characters slice 6): contact, the circle, the yearly strand rolls, growth and fade, the
//! happenings that make strands, passions, opinion terms, log lines, and the replays a late arrival and the start of the game need.
//!
//! No floating point. A chance is `base x contact x factors` evaluated as one exact rational (a **negative** result is 0), capped at 1,
//! converted once to parts per billion rounded down, and rolled on the stream of the rolling character with the purpose the spec names.
//! The life log is output, not simulation state: a line that cannot be appended after a committed change does not undo the change.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, Character, CharacterStore, Error, Office, Populations, Sex, Tier, UNKNOWN,
    character_stream,
    events::{
        EventError, FirstError, LogSink, World, age_years, conclude, exact_ppb, holds, keep,
        pop_facts,
    },
    genetics::{self, Context, roll},
    ties::{Link, MAX_STRENGTH, Orientation, Passion, PassionTarget, Ties},
};
use ck_economy::{
    events::{BOND_KINDS, Connections, Moment, MomentTrigger, ORIENTATIONS, PASSION_KINDS, Target},
    genetics::PPB,
    personality::{MANNER_PAIRS, VICE_PAIRS},
};
use ck_sim::{Date, GameTime, is_leap_year};
use std::collections::{BTreeMap, BTreeSet};

const FRIEND: usize = 0;
const RIVAL: usize = 1;
const LOVER: usize = 2;
const LOVE: usize = 3;
const RESPECT: usize = 4;
const ENVY: usize = 5;
const FEAR: usize = 6;
const HATRED: usize = 7;
/// Index of the first humour (blood) in a genetic attribute array.
const HUMOURS_AT: usize = 9;
/// One, in parts per billion, as a wide integer.
const ONE: i128 = 1_000_000_000;
/// The kinds of passion target, by [`PassionTarget::kind`].
pub const TARGET_KINDS: [&str; 4] = ["place", "religion", "house", "culture"];

fn is_bond(kind: usize) -> bool {
    kind < 3
}

fn vice(c: &Character, name: &str) -> i128 {
    VICE_PAIRS
        .iter()
        .position(|known| *known == name)
        .map_or(0, |at| i128::from(c.personality.vice[at]))
}

fn manner(c: &Character, name: &str) -> i128 {
    MANNER_PAIRS
        .iter()
        .position(|known| *known == name)
        .map_or(0, |at| i128::from(c.personality.manner[at]))
}

fn power(c: &Character) -> i128 {
    if c.office == Some(Office::Ruler) {
        2
    } else {
        0
    }
}

fn date_of(time: GameTime) -> Result<Date, Error> {
    time.date().map_err(|_| Error::Event)
}

fn year_of(time: GameTime) -> Result<u16, Error> {
    Ok(date_of(time)?.year)
}

/// The start of `month`/`day` in `year`; 29 February falls on 1 March in a common year.
fn time_of(year: u16, month: u8, day: u8) -> Result<GameTime, Error> {
    let (month, day) = if month == 2 && day == 29 && !is_leap_year(year) {
        (3, 1)
    } else {
        (month, day)
    };
    GameTime::from_date(Date {
        year,
        month,
        day,
        hour: 0,
    })
    .map_err(|_| Error::Overflow)
}

/// The start of the day of `time`: a draw's tick, so that the draw is the same at noon, when the scheduler takes it, and at midnight,
/// when a replay addresses the same date.
fn day_start(time: GameTime) -> Result<GameTime, Error> {
    let date = date_of(time)?;
    GameTime::from_date(Date { hour: 0, ..date }).map_err(|_| Error::Overflow)
}

/// The start of 1 January of the year of `time`.
pub(crate) fn new_year(time: GameTime) -> Result<GameTime, Error> {
    time_of(year_of(time)?, 1, 1)
}

/// `time` plus `years` calendar years.
fn plus_years(time: GameTime, years: u32) -> Result<GameTime, Error> {
    let date = date_of(time)?;
    let year =
        u16::try_from(u32::from(date.year).saturating_add(years)).map_err(|_| Error::Overflow)?;
    time_of(year, date.month, date.day)
}

/// The day of `c`'s birthday in `year`.
fn birthday_in(c: &Character, year: u16) -> Result<GameTime, Error> {
    let birth = date_of(c.birth)?;
    time_of(year, birth.month, birth.day)
}

fn age_at(c: &Character, when: GameTime) -> Result<u32, Error> {
    Ok(age_years(date_of(c.birth)?, date_of(when)?))
}

/// The exact product of a chance: non-negative ratios, and a sign.
#[derive(Default)]
struct Chance {
    ratios: Vec<(u128, u128)>,
    negative: bool,
    zero: bool,
}

impl Chance {
    fn push(&mut self, numerator: i128, denominator: u128) {
        match numerator {
            0 => self.zero = true,
            n if n < 0 => {
                self.negative = !self.negative;
                self.ratios.push((n.unsigned_abs(), denominator));
            }
            n => self.ratios.push((n.unsigned_abs(), denominator)),
        }
    }
    /// A table factor in parts per billion.
    fn ppb(&mut self, value: u64) {
        self.push(i128::from(value), ONE.unsigned_abs());
    }
    /// `1 + per x sum / (scale x n)` with `per` in ppb and `sum` in thousandths, held at `floor` (in ppb) when one is given.
    fn linear(&mut self, per: i64, sum: i128, n: i128, floor: Option<i128>) {
        let denominator = n * 1000 * ONE;
        let mut numerator = denominator + i128::from(per) * sum;
        if let Some(floor) = floor {
            numerator = numerator.max(floor * n * 1000);
        }
        self.push(numerator, denominator.unsigned_abs());
    }
    fn finish(self, base: u32) -> u32 {
        if self.zero || self.negative {
            return 0;
        }
        exact_ppb(base, &self.ratios)
    }
}

/// The ancestors of a character that kinship reads: its parents and its grandparents.
type Lines = [BTreeSet<u64>; 2];

/// What a pop gives a character's passions and opinion terms that a record does not hold: its religion and culture.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopTargets {
    /// The religion key of the pop.
    pub religion: Option<String>,
    /// The culture key of the pop.
    pub culture: Option<String>,
}

impl PopTargets {
    /// The religion and culture of the pop of `c`.
    #[must_use]
    pub fn of(populations: &impl Populations, c: &Character) -> Self {
        Self {
            religion: populations.religion(&c.pop),
            culture: populations.culture(&c.pop),
        }
    }
}

/// Whether the strand kind `name` is a bond (friend, rival, lover: stored on both members) and not a feeling (stored on its holder alone).
pub(crate) fn is_bond_kind(name: &str) -> bool {
    kind_index(name).is_some_and(is_bond)
}

/// Whether `name` is a strand kind of slice 6 (`friend`, `rival`, `lover`, `love`, `respect`, `envy`, `fear` or `hatred`).
pub(crate) fn is_strand_kind(name: &str) -> bool {
    kind_index(name).is_some()
}

/// The strand kind named `name`.
fn kind_index(name: &str) -> Option<usize> {
    ck_economy::events::ALL_KINDS
        .iter()
        .position(|known| *known == name)
}

/// How the clergy weights of the data apply to a drawn orientation (CK-369 §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Clergy {
    /// A character of any other pop: the weights of the data alone.
    No,
    /// A character generated in a monk pop: gay, bisexual and asexual weighted by the data's clergy factors.
    Monk,
    /// A cleric who has children: only straight and bisexual are drawn (the clergy factors apply to bisexual).
    MonkWithChildren,
}

/// The orientation of a character created by `ctx` with the id `id`, drawn once on its stream (purpose `orientation`): the data's weights,
/// the boost for a child of a gay or bisexual parent and, for a generated cleric, the clergy factors (CK-369).
pub(crate) fn draw_orientation(
    ctx: &Context<'_>,
    id: u64,
    parents: [Option<&Character>; 2],
    clergy: Clergy,
) -> Result<Orientation, Error> {
    let orientation = &ctx.lifecycle.connections.orientation;
    let boosted = parents.iter().flatten().any(|parent| {
        matches!(
            parent.ties.orientation,
            Orientation::Gay | Orientation::Bisexual
        )
    });
    let weights: Vec<u64> = orientation
        .weights
        .iter()
        .zip(ORIENTATIONS)
        .zip(Orientation::ALL)
        .zip(orientation.clergy_ppb)
        .map(|(((weight, name), which), clergy_ppb)| {
            let factor = if boosted && (name == "gay" || name == "bisexual") {
                orientation.parent_boost_ppb
            } else {
                u64::from(PPB)
            };
            let base = u64::from(*weight).saturating_mul(factor);
            if clergy == Clergy::No {
                return base;
            }
            if clergy == Clergy::MonkWithChildren
                && matches!(which, Orientation::Gay | Orientation::Asexual)
            {
                return 0;
            }
            // A clergy factor scales the weight; a sum of four such weights still fits 64 bits.
            let scaled = u128::from(base) * u128::from(clergy_ppb) / u128::from(PPB);
            u64::try_from(scaled.min(u128::from(u64::MAX / 8))).unwrap_or(u64::MAX / 8)
        })
        .collect();
    let total: u64 = weights.iter().sum();
    let mut pick = genetics::below(&mut genetics::site(ctx, id, "", "orientation"), total)?;
    for (weight, which) in weights.iter().zip(Orientation::ALL) {
        if pick < *weight {
            return Ok(which);
        }
        pick -= *weight;
    }
    Ok(Orientation::default())
}

/// The orientation, the dates and the empty strands of a character created by `ctx`, whose parents are `parents`.
pub(crate) fn make_ties(
    ctx: &Context<'_>,
    id: u64,
    sex: Sex,
    parents: [Option<&Character>; 2],
) -> Result<Ties, Error> {
    let _ = sex;
    let mut ties = Ties {
        location_since: ctx.tick,
        in_pop_since: ctx.tick,
        last_full: ctx.tick.date().map_or(0, |date| date.year),
        ..Ties::default()
    };
    if !ctx.lifecycle.connections.loaded {
        return Ok(ties);
    }
    ties.orientation = draw_orientation(ctx, id, parents, Clergy::No)?;
    Ok(ties)
}

/// `a`'s opinion of `b` (CK-368 §7), the complete calculation: the slice 3 personality opinion ([`crate::personality_opinion`]) plus the terms of its strands and passions (CK-368 §7), in thousandths of a point: the slice 3 opinion, plus
/// per point of strength the weight of each strand `a` holds toward `b` (bonds are stored on both, a feeling on its holder) and of each of
/// `a`'s passions whose target is `b`'s house, religion, culture or location. A pure function of the records and `b_pop`.
///
/// # Errors
/// Refuses arithmetic that does not fit.
pub fn opinion(
    personality: &ck_economy::Personality,
    connections: &Connections,
    a: &Character,
    b: &Character,
    b_pop: &PopTargets,
) -> Result<i64, Error> {
    let base = i128::from(crate::personality::personality_opinion(personality, a, b)?);
    if !connections.loaded {
        return i64::try_from(base).map_err(|_| Error::Arithmetic);
    }
    let mut total = base;
    if let Some(link) = a.ties.links.get(&b.id) {
        for (kind, name) in ck_economy::events::ALL_KINDS.iter().enumerate() {
            let weight = connections.opinion_strands.get(*name).copied().unwrap_or(0);
            total += i128::from(link.strands[kind].strength) * i128::from(weight);
        }
    }
    for passion in &a.ties.passions {
        let aimed = match (
            TARGET_KINDS.get(usize::from(passion.target.kind)),
            &passion.target.key,
        ) {
            (Some(&"place"), key) => *key == b.pop.place,
            (Some(&"religion"), key) => b_pop.religion.as_deref() == Some(key.as_str()),
            (Some(&"house"), key) => !key.is_empty() && *key == b.life.house,
            (Some(&"culture"), key) => b_pop.culture.as_deref() == Some(key.as_str()),
            _ => false,
        };
        if aimed {
            let name = PASSION_KINDS
                .get(usize::from(passion.kind))
                .copied()
                .unwrap_or("");
            let weight = connections.opinion_passions.get(name).copied().unwrap_or(0);
            total += i128::from(passion.strength) * i128::from(weight);
        }
    }
    i64::try_from(total).map_err(|_| Error::Arithmetic)
}

/// A candidate of a circle: its id, the contact in ppb and whether the pair shares a setting other than the same location.
type Candidate = (u64, u32, bool);

impl CharacterStore {
    /// The stream of `id` and `purpose` for a draw dated `when`: the game's stable seed and the start of that day, so a live draw and a
    /// replay of the same date, and the draws of a calendar year, are the same whenever they are taken.
    pub(crate) fn dated_stream(
        &self,
        pass_seed: u64,
        id: u64,
        purpose: &str,
        when: GameTime,
    ) -> Result<ck_sim::Stream, Error> {
        Ok(character_stream(
            self.stable_seed(pass_seed),
            id,
            purpose,
            day_start(when)?,
        ))
    }

    /// Whether a living `ruler`-marked character lives at `place` (in any pop).
    fn ruler_at(&self, place: &str) -> bool {
        self.by_place.get(place).is_some_and(|ids| {
            ids.iter().any(|id| {
                #[cfg(feature = "test-fixtures")]
                self.visited.0.set(self.visited.0.get() + 1);
                self.live
                    .get(id)
                    .is_some_and(|c| c.office == Some(Office::Ruler))
            })
        })
    }

    /// The contact of `a` and `b` in ppb, and whether they share a live setting other than the same location (CK-368 §3). Computed,
    /// never stored: the highest shared live setting plus `extra_setting` for each further one, capped at 1.
    pub(crate) fn contact<P: Populations>(
        rules: &Connections,
        populations: &P,
        a: &Character,
        b: &Character,
        ruler_here: bool,
    ) -> (u32, bool) {
        if a.pop.place != b.pop.place || !rules.loaded {
            return (0, false);
        }
        let live = |name: &str| {
            rules
                .settings
                .get(name)
                .filter(|setting| setting.live)
                .map(|setting| setting.contact_ppb)
        };
        let mut shared: Vec<u32> = Vec::new();
        let mut elsewhere = false;
        if a.pop == b.pop {
            if let Some(value) = live("household") {
                let house = !a.life.house.is_empty() && a.life.house == b.life.house;
                if house || close_kin(a, b) {
                    shared.push(value);
                    elsewhere = true;
                }
            }
            if let Some(value) = live("monastery")
                && populations.kind(&a.pop).as_deref() == Some("monk")
            {
                shared.push(value);
                elsewhere = true;
            }
        } else if let Some(value) = live("same_location") {
            shared.push(value);
        }
        if let Some(value) = live("court")
            && ruler_here
            && populations.kind(&a.pop).as_deref() == Some("noble")
            && populations.kind(&b.pop).as_deref() == Some("noble")
        {
            shared.push(value);
            elsewhere = true;
        }
        let Some(best) = shared.iter().copied().max() else {
            return (0, false);
        };
        let extra = u64::from(rules.extra_setting_ppb) * (shared.len() as u64 - 1);
        let total = (u64::from(best) + extra).min(u64::from(PPB));
        (u32::try_from(total).unwrap_or(PPB), elsewhere)
    }

    /// The contact of two live characters now.
    pub(crate) fn contact_now<P: Populations>(
        &self,
        rules: &Connections,
        populations: &P,
        a: &Character,
        b: &Character,
    ) -> u32 {
        Self::contact(rules, populations, a, b, self.ruler_at(&a.pop.place)).0
    }

    /// The circle of `a` on `when` (CK-368 §4): the live characters at its location aged `min_age` or more whose contact is at least the
    /// floor, the `size` closest by contact, ties by id. Found through the place index: the work is the characters at `a`'s location.
    pub(crate) fn circle<P: Populations>(
        &self,
        rules: &Connections,
        populations: &P,
        a: &Character,
        when: GameTime,
    ) -> Result<Vec<Candidate>, Error> {
        let mut found: Vec<Candidate> = Vec::new();
        let ruler_here = self.ruler_at(&a.pop.place);
        for id in self.by_place.get(&a.pop.place).into_iter().flatten() {
            #[cfg(feature = "test-fixtures")]
            self.visited.0.set(self.visited.0.get() + 1);
            let Some(b) = self.live.get(id) else {
                continue;
            };
            if b.id == a.id || age_at(b, when)? < rules.circle.min_age {
                continue;
            }
            let (contact, elsewhere) = Self::contact(rules, populations, a, b, ruler_here);
            if contact >= rules.circle.floor_ppb {
                found.push((b.id, contact, elsewhere));
            }
        }
        found.sort_by_key(|(id, contact, _)| (std::cmp::Reverse(*contact), *id));
        found.truncate(rules.circle.size as usize);
        Ok(found)
    }

    /// The parents and grandparents of `c`, live or archived, held in `cache`.
    fn lines<A: ArchiveSink>(
        &self,
        archive: &A,
        cache: &mut BTreeMap<u64, Lines>,
        c: &Character,
    ) -> Result<Lines, Error> {
        if let Some(lines) = cache.get(&c.id) {
            return Ok(lines.clone());
        }
        let [parents, grandparents, _] = self.ancestors(archive, c)?;
        let lines = [parents, grandparents];
        cache.insert(c.id, lines.clone());
        Ok(lines)
    }

    /// Whether `a` and `b` are kin for a lover or a love: they share a parent or a grandparent, or one is the other's parent.
    fn kin<A: ArchiveSink>(
        &self,
        archive: &A,
        cache: &mut BTreeMap<u64, Lines>,
        a: &Character,
        b: &Character,
    ) -> Result<bool, Error> {
        if [a.father, a.mother].contains(&b.id) || [b.father, b.mother].contains(&a.id) {
            return Ok(true);
        }
        let (x, y) = (
            self.lines(archive, cache, a)?,
            self.lines(archive, cache, b)?,
        );
        Ok((0..2).any(|generation| x[generation].intersection(&y[generation]).next().is_some()))
    }

    /// The chance in ppb that a bond of `kind` (friend, rival or lover) forms or grows between `a` and `b` in a year: `base x contact x
    /// factors` (CK-368 §4), exact, a negative result 0, capped at 1.
    ///
    /// # Errors
    /// An archive read failure and a date outside the calendar.
    #[expect(
        clippy::too_many_arguments,
        clippy::many_single_char_names,
        reason = "the rules, the archive and its cache, the kind, both characters, the contact and the date are all inputs; a and b are the pair"
    )]
    pub(crate) fn bond_chance<A: ArchiveSink>(
        &self,
        rules: &Connections,
        archive: &A,
        cache: &mut BTreeMap<u64, Lines>,
        kind: usize,
        a: &Character,
        b: &Character,
        contact: u32,
        when: GameTime,
    ) -> Result<u32, Error> {
        let s = &rules.strands;
        let (age_a, age_b) = (age_at(a, when)?, age_at(b, when)?);
        let gap = |years: u32| {
            (
                u128::from(years),
                u128::from(years) + u128::from(age_a.abs_diff(age_b)),
            )
        };
        let mut chance = Chance::default();
        chance.push(i128::from(contact), ONE.unsigned_abs());
        match kind {
            FRIEND => {
                chance.linear(
                    s.friend_gregarious,
                    manner(a, "solitary_gregarious") + manner(b, "solitary_gregarious"),
                    2,
                    Some(100_000_000),
                );
                chance.linear(
                    s.friend_kindness,
                    vice(a, "envy_kindness") + vice(b, "envy_kindness"),
                    2,
                    Some(100_000_000),
                );
                let humours: i128 = (0..4)
                    .map(|i| {
                        i128::from(a.genetics.value[HUMOURS_AT + i])
                            .abs_diff(i128::from(b.genetics.value[HUMOURS_AT + i]))
                            .cast_signed()
                    })
                    .sum();
                // `1 + w x (1 - sum / 20)`, the sum in thousandths of a point.
                let denominator = 20_000 * ONE;
                chance.push(
                    denominator + i128::from(s.friend_humours_alike) * (20_000 - humours),
                    denominator.unsigned_abs(),
                );
                for c in [a, b] {
                    chance.ppb(s.friend_withdrawn[usize::from(c.grade("melancholy"))]);
                    chance.ppb(s.friend_withdrawn[usize::from(c.grade("recluse"))]);
                }
                let (drunk_a, drunk_b) = (a.grade("drunkard"), b.grade("drunkard"));
                if drunk_a > 0 && drunk_b > 0 {
                    chance.ppb(s.friend_drunkards[usize::from(drunk_a.min(drunk_b))]);
                }
                let (n, d) = gap(s.friend_age_gap);
                chance.push(i128::try_from(n).unwrap_or(0), d);
            }
            RIVAL => {
                if power(a) == power(b) && power(a) > 0 {
                    chance.ppb(s.rival_same_power);
                }
                chance.linear(
                    s.rival_ambition,
                    manner(a, "content_ambitious") + manner(b, "content_ambitious"),
                    2,
                    Some(100_000_000),
                );
                chance.linear(
                    s.rival_envy,
                    vice(a, "envy_kindness") + vice(b, "envy_kindness"),
                    2,
                    Some(100_000_000),
                );
                if [a.father, a.mother].contains(&b.id) || [b.father, b.mother].contains(&a.id) {
                    chance.ppb(s.rival_parent_child);
                }
                let (n, d) = gap(s.rival_age_gap);
                chance.push(i128::try_from(n).unwrap_or(0), d);
            }
            LOVER => {
                if !(attracted(a, b) && attracted(b, a))
                    || age_a.min(age_b) < rules.circle.lover_min_age
                    || self.kin(archive, cache, a, b)?
                {
                    return Ok(0);
                }
                chance.linear(
                    s.lover_lust,
                    vice(a, "lust_chastity") + vice(b, "lust_chastity"),
                    2,
                    Some(50_000_000),
                );
                for c in [a, b] {
                    chance.ppb(s.lover_beauty[usize::from(c.grade("beauty"))]);
                    chance.ppb(s.lover_ill_favoured[usize::from(c.grade("ill_favoured"))]);
                }
                let elsewhere = |c: &Character, other: u64| {
                    c.spouses
                        .iter()
                        .any(|spouse| *spouse != other && self.live.contains_key(spouse))
                };
                if elsewhere(a, b.id) || elsewhere(b, a.id) {
                    chance.ppb(s.lover_married_elsewhere);
                }
                let (n, d) = gap(s.lover_age_gap);
                chance.push(i128::try_from(n).unwrap_or(0), d);
            }
            _ => return Ok(0),
        }
        Ok(chance.finish(s.base_ppb.get(BOND_KINDS[kind]).copied().unwrap_or(0)))
    }

    /// The chance in ppb of `a`'s feeling of `kind` (love, respect, envy, fear or hatred) toward `b` in a year (CK-368 §4).
    ///
    /// # Errors
    /// An archive read failure.
    #[expect(
        clippy::too_many_arguments,
        reason = "the rules, the archive and its cache, the kind, both characters and the contact are all inputs"
    )]
    pub(crate) fn feeling_chance<A: ArchiveSink>(
        &self,
        rules: &Connections,
        archive: &A,
        cache: &mut BTreeMap<u64, Lines>,
        kind: usize,
        a: &Character,
        b: &Character,
        contact: u32,
    ) -> Result<u32, Error> {
        let s = &rules.strands;
        let mut chance = Chance::default();
        chance.push(i128::from(contact), ONE.unsigned_abs());
        match kind {
            LOVE => {
                if a.spouses.contains(&b.id) {
                    chance.ppb(s.love_spouse);
                }
                if [a.father, a.mother].contains(&b.id) || [b.father, b.mother].contains(&a.id) {
                    chance.ppb(s.love_parent_child);
                }
                if attracted(a, b) && !self.kin(archive, cache, a, b)? {
                    chance.ppb(s.love_beauty[usize::from(b.grade("beauty"))]);
                }
            }
            RESPECT => {
                // (1 + fame_per x max(0, b's fame) / 10) x (b's skills / divisor) x (1 + power_per x b's power).
                chance.linear(
                    s.respect_fame_per,
                    i128::from(b.personality.fame.max(0)),
                    1,
                    None,
                );
                let skills: i128 = b.life.skills.values().map(|level| i128::from(*level)).sum();
                chance.push(skills, 1000 * u128::from(s.respect_skills_divisor));
                chance.push(
                    ONE + i128::from(s.respect_power_per) * power(b),
                    ONE.unsigned_abs(),
                );
            }
            ENVY => {
                let envy = (-vice(a, "envy_kindness")).max(0);
                let divisor = i128::from(s.envy_divisor);
                chance.push(
                    envy * ONE + i128::from(s.envy_floor) * 1000 * divisor,
                    (1000 * divisor * ONE).unsigned_abs(),
                );
                chance.push(
                    ONE + i128::from(s.envy_beauty_per_grade) * i128::from(b.grade("beauty")),
                    ONE.unsigned_abs(),
                );
                if power(b) > power(a) {
                    chance.ppb(s.envy_more_power);
                }
            }
            FEAR => {
                chance.push((power(b) - power(a)).max(0), 1);
                chance.push(
                    ONE + i128::from(s.fear_rage_per_grade) * i128::from(b.grade("rage_fits")),
                    ONE.unsigned_abs(),
                );
                if b.grade("ill_favoured") >= 4 {
                    chance.ppb(s.fear_grim_face);
                }
                chance.linear(s.fear_daring_per_point, manner(a, "wary_daring"), 1, None);
            }
            HATRED => {
                chance.linear(s.hatred_patience, vice(a, "wrath_patience"), 1, None);
                chance.linear(s.hatred_kindness, vice(a, "envy_kindness"), 1, None);
            }
            _ => return Ok(0),
        }
        Ok(chance.finish(
            s.base_ppb
                .get(ck_economy::events::ALL_KINDS[kind])
                .copied()
                .unwrap_or(0),
        ))
    }
}

/// Whether `a` is drawn to `b` by orientation alone.
fn attracted(a: &Character, b: &Character) -> bool {
    a.ties.orientation.attracted(a.sex == b.sex)
}

/// Whether two characters are close kin: a parent, child, sibling (sharing a known parent) or spouse.
fn close_kin(a: &Character, b: &Character) -> bool {
    let parents = |c: &Character| [c.father, c.mother].into_iter().filter(|p| *p != UNKNOWN);
    a.spouses.contains(&b.id)
        || b.spouses.contains(&a.id)
        || parents(a).any(|p| p == b.id)
        || parents(b).any(|p| p == a.id)
        || parents(a).any(|p| parents(b).any(|q| p == q))
}

// ---------------------------------------------------------------------------------------------------------------------
// Writing strands: growth, fade, the life-log lines

impl CharacterStore {
    /// The name a strand line gives a character (CK-370 §1): its derived name, the name and its patronymic. A figure's authored patronymic and
    /// a father's authored genitive take precedence over the pool's forms, and a generated character's line carries the pool's.
    fn name_of<P: Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        id: u64,
    ) -> String {
        let Some(c) = self.live.get(&id) else {
            return String::new();
        };
        let father = (c.father != UNKNOWN)
            .then(|| self.record(&*world.archive, c.father).ok().flatten())
            .flatten();
        crate::cycle::derived_name(c, father.as_ref(), world.lifecycle)
    }

    fn link_mut(&mut self, holder: u64, partner: u64) -> Option<&mut Link> {
        Some(
            self.live
                .get_mut(&holder)?
                .ties
                .links
                .entry(partner)
                .or_default(),
        )
    }

    /// Raise a marker of the pair (on both members) and never lower it.
    fn mark_pair(&mut self, a: u64, b: u64, mark: impl Fn(&mut Link) -> &mut u16, year: u16) {
        for (x, y) in [(a, b), (b, a)] {
            if let Some(link) = self.link_mut(x, y) {
                let slot = mark(link);
                *slot = (*slot).max(year);
            }
        }
    }

    /// Set the strand of `kind` between two live characters to an authored `strength` (CK-370 §3 step 8), overriding what the burn-in made for
    /// the pair and kind: a bond (friend, rival, lover) on both members, a feeling on `holder` alone. The strand's last growth is `year`.
    ///
    /// # Errors
    /// Refuses a kind that is not slice 6's, a strength above 10 and a character that is not live.
    pub(crate) fn set_authored_strand(
        &mut self,
        holder: u64,
        partner: u64,
        kind: &str,
        strength: u8,
        year: u16,
    ) -> Result<(), Error> {
        let kind = kind_index(kind).ok_or(Error::Event)?;
        if strength > MAX_STRENGTH
            || !self.live.contains_key(&holder)
            || !self.live.contains_key(&partner)
        {
            return Err(Error::Event);
        }
        let sides: Vec<(u64, u64)> = if is_bond(kind) {
            vec![(holder, partner), (partner, holder)]
        } else {
            vec![(holder, partner)]
        };
        for (x, y) in sides {
            let link = self.link_mut(x, y).ok_or(Error::MissingCharacter)?;
            link.strands[kind].strength = strength;
            link.strands[kind].grew = link.strands[kind].grew.max(year);
        }
        Ok(())
    }

    /// Add 1 to the strand `holder` has toward `partner` (a new strand starts at 1, at most 10) and set its last growth year; a bond is
    /// written on both members. Returns the strength before and after.
    pub(crate) fn bump(
        &mut self,
        holder: u64,
        partner: u64,
        kind: usize,
        year: u16,
    ) -> Option<(u8, u8)> {
        let old = self
            .live
            .get(&holder)?
            .ties
            .links
            .get(&partner)
            .map_or(0, |link| link.strands[kind].strength);
        let new = (old + 1).min(MAX_STRENGTH);
        let sides: Vec<(u64, u64)> = if is_bond(kind) {
            vec![(holder, partner), (partner, holder)]
        } else {
            vec![(holder, partner)]
        };
        for (x, y) in sides {
            let link = self.link_mut(x, y)?;
            link.strands[kind].strength = new;
            link.strands[kind].grew = link.strands[kind].grew.max(year);
        }
        Some((old, new))
    }

    /// Take 1 from the strand `holder` has toward `partner` (a bond from both members) and set a feeling's last fade year. Returns the
    /// strength before and after; the strand ends at 0.
    fn lose(&mut self, holder: u64, partner: u64, kind: usize, year: u16) -> Option<(u8, u8)> {
        let old = self.live.get(&holder)?.ties.links.get(&partner)?.strands[kind].strength;
        let new = old.saturating_sub(1);
        let sides: Vec<(u64, u64)> = if is_bond(kind) {
            vec![(holder, partner), (partner, holder)]
        } else {
            vec![(holder, partner)]
        };
        for (x, y) in sides {
            let link = self.link_mut(x, y)?;
            link.strands[kind].strength = new;
            if !is_bond(kind) {
                link.strands[kind].faded = link.strands[kind].faded.max(year);
            }
        }
        Some((old, new))
    }

    /// Write the log lines of a strand that went from `old` to `new` (CK-368 §8): when made, on reaching 5 and 10, and when ended; a
    /// bond to both lives' logs, a feeling to the holder's.
    #[expect(
        clippy::too_many_arguments,
        reason = "the world, the strand's kind and change, both ends, the date and the first error are all inputs"
    )]
    fn strand_lines<P: Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &mut World<'_, P, A, L>,
        kind: usize,
        change: (u8, u8),
        holder: u64,
        partner: u64,
        date: GameTime,
        first: &mut FirstError,
    ) {
        let lifecycle = world.lifecycle;
        let Some(text) = lifecycle
            .connections
            .log
            .get(ck_economy::events::ALL_KINDS[kind])
        else {
            return;
        };
        let (old, new) = change;
        let line = if new == 0 && old > 0 {
            &text.ended
        } else if old == 0 {
            &text.made
        } else if old < 5 && (5..10).contains(&new) {
            &text.at_5
        } else if old < 10 && new == 10 {
            &text.at_10
        } else {
            return;
        };
        let owners: Vec<(u64, u64)> = if is_bond(kind) {
            vec![(holder, partner), (partner, holder)]
        } else {
            vec![(holder, partner)]
        };
        for (owner, other) in owners {
            let rendered = line
                .replace("{name}", &self.name_of(world, owner))
                .replace("{other}", &self.name_of(world, other));
            Self::line(world, owner, date, &rendered, first);
        }
    }
}

/// The top-up chance in ppb that makes a coarse year's roll and one more together the full tier's (CK-368 §9): for a moment of chance `p`
/// before the tier factor and tier factors `full` and `coarse`, `q = p x (full - coarse) / (1 - p x coarse)`, so that
/// `1 - (1 - p x coarse)(1 - q) = p x full`. Exact, rounded down once; 0 when the coarse roll is already certain.
#[must_use]
pub fn top_up_chance_ppb(p: u32, full: u32, coarse: u32) -> u32 {
    let (p, full, coarse) = (u128::from(p), u128::from(full), u128::from(coarse));
    let scale = u128::from(PPB);
    let denominator = (scale * scale).saturating_sub(p * coarse);
    if denominator == 0 {
        return 0;
    }
    u32::try_from((scale * p * full.saturating_sub(coarse) / denominator).min(scale)).unwrap_or(PPB)
}

/// The strand rule (`[strands.happenings]`) a happening of this name is bound to, or `""`.
fn strand_rule_name(happening: &str) -> &'static str {
    match happening {
        "battle_comrades" => "battle_comrades",
        "rescued" => "rescue",
        "insulted" => "insult",
        "kin_killed" => "kin_killed",
        "duel" => "duel",
        _ => "",
    }
}

/// The rolls one birthday would take with one partner (CK-368 §4): the feeling and bond kinds with a positive chance.
struct Planned {
    partner: u64,
    feelings: Vec<(usize, u32)>,
    bonds: Vec<(usize, u32)>,
}

// ---------------------------------------------------------------------------------------------------------------------
// The yearly rolls and the feeling fade

impl CharacterStore {
    /// A character's yearly feeling and bond rolls on `when` (its birthday; CK-368 §4), after its marriage wish: with its whole circle if
    /// it is full tier, otherwise only with full-tier partners that share a setting other than the same location. Each (a, b) pair of
    /// feelings is rolled at most once a year, each pair of bonds once a calendar year at the first birthday of either member that
    /// permits it; a marker is claimed only when a roll happens.
    ///
    /// # Errors
    /// Refuses arithmetic or an archive read that fails; a life-log failure is kept in `first`.
    pub(crate) fn connection_rolls<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        when: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let lifecycle = world.lifecycle;
        let rules = &lifecycle.connections;
        let Some(a) = self.live.get(&id).cloned() else {
            return Ok(());
        };
        if !rules.loaded || age_at(&a, when)? < rules.circle.min_age {
            return Ok(());
        }
        let year = year_of(when)?;
        let coarse = self.tier(id, when) != Some(Tier::Full);
        let circle = self.circle(rules, world.populations, &a, when)?;
        // Plan first: for each partner the rolls this birthday would actually take (a positive chance, a marker still unclaimed, the tier
        // rule met), before any deferred year is touched.
        let mut plan: Vec<Planned> = Vec::new();
        {
            let archive = &*world.archive;
            let mut cache = BTreeMap::new();
            for (b_id, contact, elsewhere) in circle {
                if coarse && !(elsewhere && self.tier(b_id, when) == Some(Tier::Full)) {
                    continue;
                }
                let Some(b) = self.live.get(&b_id) else {
                    continue;
                };
                let link = a.ties.links.get(&b_id).copied().unwrap_or_default();
                // A pair's year is claimed only when a roll actually happens (a chance of 0 rolls nothing).
                let mut feelings = Vec::new();
                if link.feeling_roll < year {
                    for kind in LOVE..=HATRED {
                        let chance =
                            self.feeling_chance(rules, archive, &mut cache, kind, &a, b, contact)?;
                        if chance > 0 {
                            feelings.push((kind, chance));
                        }
                    }
                }
                let mut bonds = Vec::new();
                if link.mutual_roll < year {
                    for kind in FRIEND..=LOVER {
                        let chance = self
                            .bond_chance(rules, archive, &mut cache, kind, &a, b, contact, when)?;
                        if chance > 0 {
                            bonds.push((kind, chance));
                        }
                    }
                }
                if !feelings.is_empty() || !bonds.is_empty() {
                    plan.push(Planned {
                        partner: b_id,
                        feelings,
                        bonds,
                    });
                }
            }
        }
        let mut feeling_rolled = Vec::new();
        let mut bond_rolled = Vec::new();
        let mut hits: Vec<(u64, usize)> = Vec::new();
        for planned in &plan {
            let b_id = planned.partner;
            for (kind, chance) in &planned.feelings {
                let purpose = format!("strand.{}.{b_id}", ck_economy::events::ALL_KINDS[*kind]);
                let mut stream = self.dated_stream(world.seed, id, &purpose, when)?;
                if roll(&mut stream, *chance)? {
                    hits.push((b_id, *kind));
                }
            }
            if !planned.feelings.is_empty() {
                feeling_rolled.push(b_id);
            }
            for (kind, chance) in &planned.bonds {
                let purpose = format!("bond.{}.{b_id}", BOND_KINDS[*kind]);
                let mut stream = self.dated_stream(world.seed, id, &purpose, when)?;
                if roll(&mut stream, *chance)? {
                    hits.push((b_id, *kind));
                }
            }
            if !planned.bonds.is_empty() {
                bond_rolled.push(b_id);
            }
        }
        for b_id in feeling_rolled {
            if let Some(link) = self.link_mut(id, b_id) {
                link.feeling_roll = link.feeling_roll.max(year);
            }
        }
        for b_id in bond_rolled {
            self.mark_pair(id, b_id, |link| &mut link.mutual_roll, year);
        }
        for (b_id, kind) in hits {
            if let Some(change) = self.bump(id, b_id, kind, year) {
                self.strand_lines(world, kind, change, id, b_id, when, first);
            }
        }
        Ok(())
    }

    /// The feeling fade of `id` on `when` (CK-368 §4): each of its feelings, in or out of its circle, whose partner's contact is now below
    /// `fade_below`, which did not grow this year and has not faded this year, loses 1 and sets its last fade year; at 0 it ends.
    ///
    /// # Errors
    /// Refuses a date outside the calendar; a life-log failure is kept in `first`.
    pub(crate) fn connection_fade<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        when: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let lifecycle = world.lifecycle;
        let rules = &lifecycle.connections;
        if !rules.loaded || !self.live.contains_key(&id) {
            return Ok(());
        }
        let year = year_of(when)?;
        let Some(a) = self.live.get(&id).cloned() else {
            return Ok(());
        };
        let ruler_here = self.ruler_at(&a.pop.place);
        let mut fading = Vec::new();
        for (b_id, link) in &a.ties.links {
            let Some(b) = self.live.get(b_id) else {
                continue;
            };
            let (contact, _) = Self::contact(rules, world.populations, &a, b, ruler_here);
            if contact >= rules.strands.fade_below_ppb {
                continue;
            }
            for kind in LOVE..=HATRED {
                let strand = link.strands[kind];
                if strand.strength > 0 && strand.grew < year && strand.faded < year {
                    fading.push((*b_id, kind));
                }
            }
        }
        for (b_id, kind) in fading {
            if let Some(change) = self.lose(id, b_id, kind, year) {
                self.strand_lines(world, kind, change, id, b_id, when, first);
            }
        }
        Ok(())
    }

    /// The resolution of the closed year `closed` for one pair, on `when` (CK-368 §4): each of its bonds that did not grow in `closed` and
    /// whose pair's contact is now below `fade_below` loses 1 (at 0 it ends); then a rivalry at `rival_at` or more adds hatred +1 each way
    /// with its chance; then the pair's last resolved year becomes `closed`. A pair already resolved for `closed` is left alone.
    ///
    /// # Errors
    /// Refuses a date outside the calendar and a failed roll; a life-log failure is kept in `first`.
    pub(crate) fn resolve_pair_year<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        a_id: u64,
        b_id: u64,
        closed: u16,
        when: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let lifecycle = world.lifecycle;
        let rules = &lifecycle.connections;
        let (Some(a), Some(b)) = (self.live.get(&a_id).cloned(), self.live.get(&b_id).cloned())
        else {
            return Ok(());
        };
        let resolved = [(&a, b_id), (&b, a_id)]
            .iter()
            .filter_map(|(c, other)| c.ties.links.get(other))
            .map(|link| link.resolved)
            .max()
            .unwrap_or(0);
        if resolved >= closed || !rules.loaded {
            return Ok(());
        }
        let year = year_of(when)?;
        let contact = self.contact_now(rules, world.populations, &a, &b);
        let strands = a.ties.links.get(&b_id).copied().unwrap_or_default().strands;
        if contact < rules.strands.fade_below_ppb {
            for (kind, strand) in strands.iter().copied().enumerate().take(LOVER + 1) {
                if strand.strength > 0
                    && strand.grew < closed
                    && let Some(change) = self.lose(a_id, b_id, kind, year)
                {
                    self.strand_lines(world, kind, change, a_id, b_id, when, first);
                }
            }
        }
        let rivalry = self
            .live
            .get(&a_id)
            .and_then(|c| c.ties.links.get(&b_id))
            .map_or(0, |link| link.strands[RIVAL].strength);
        if u32::from(rivalry) >= rules.strands.rival_at && rivalry > 0 {
            // One roll for the rivalry (the canonical unordered pair and the closed year), on the lower identifier's stream with the filed
            // purpose and the other's identifier, so separate rivalries of one character are separate draws; a hit adds hatred each way.
            let (low, high) = (a_id.min(b_id), a_id.max(b_id));
            let purpose = format!("rivalry_hatred.{closed}.{high}");
            let mut stream = self.dated_stream(world.seed, low, &purpose, when)?;
            if roll(&mut stream, rules.strands.hatred_chance_ppb)? {
                for (holder, other) in [(low, high), (high, low)] {
                    if let Some(change) = self.bump(holder, other, HATRED, year) {
                        self.strand_lines(world, HATRED, change, holder, other, when, first);
                    }
                }
            }
        }
        self.mark_pair(a_id, b_id, |link| &mut link.resolved, closed);
        Ok(())
    }

    /// Every unordered pair of live characters that holds a link, in id order.
    fn linked_pairs(&self) -> BTreeSet<(u64, u64)> {
        let mut pairs = BTreeSet::new();
        for c in self.live.values() {
            for partner in c.ties.links.keys() {
                if self.live.contains_key(partner) {
                    pairs.insert((c.id.min(*partner), c.id.max(*partner)));
                }
            }
        }
        pairs
    }

    /// The resolution of the closed year of every linked pair, run on 1 January (CK-368 §4): `today` is 1 January of the year after the
    /// one it closes. A closed year is resolved exactly once.
    ///
    /// # Errors
    /// The first failure, once every pair has been looked at.
    pub fn run_new_year<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        today: GameTime,
    ) -> Result<(), EventError> {
        let date = date_of(today)?;
        if (date.month, date.day) != (1, 1) || !world.lifecycle.connections.loaded {
            return Ok(());
        }
        self.fix_seed(world.seed);
        let closed = date.year.saturating_sub(1);
        let mut first: FirstError = None;
        for (a, b) in self.linked_pairs() {
            if let Err(error) = self.resolve_pair_year(world, a, b, closed, today, &mut first) {
                keep(&mut first, error);
            }
        }
        conclude(first)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Passions

impl CharacterStore {
    /// Whether `c` meets the extra conditions of a live moment (CK-368 §6): `ruler_years_min`, `years_in_pop_min`, `in_ruling_house` and
    /// `standing_min`, on top of the conditions of slice 4.
    fn moment_holds(&self, c: &Character, moment: &Moment, when: GameTime) -> Result<bool, Error> {
        let since = |from: GameTime| -> Result<u32, Error> {
            Ok(age_years(date_of(from)?, date_of(when)?))
        };
        let w = &moment.when;
        if let Some(years) = w.ruler_years_min {
            let from = c.ties.office_since.map_or(c.ties.location_since, |office| {
                office.max(c.ties.location_since)
            });
            if c.office != Some(Office::Ruler) || since(from)? < years {
                return Ok(false);
            }
        }
        if let Some(years) = w.years_in_pop_min
            && since(c.ties.in_pop_since)? < years
        {
            return Ok(false);
        }
        if w.in_ruling_house && !self.ruling_house(&c.life.house) {
            return Ok(false);
        }
        if let Some(fame) = w.fame_min
            && c.personality.fame < fame
        {
            return Ok(false);
        }
        Ok(true)
    }

    /// The target of a moment's passion for `c` (CK-368 §6), or `None` when it has none (a houseless character's house, an unresolved pop).
    fn passion_target<P: Populations>(
        populations: &P,
        c: &Character,
        target: &Target,
        killer_house: Option<&str>,
    ) -> Option<PassionTarget> {
        let (kind, key) = match target {
            Target::KillerHouse => (2, killer_house?.to_owned()),
            Target::Location => (0, c.pop.place.clone()),
            Target::Religion => (1, populations.religion(&c.pop)?),
            Target::House => (2, c.life.house.clone()),
            Target::Culture => (3, populations.culture(&c.pop)?),
            Target::Deferred(_) => return None,
        };
        (!key.is_empty()).then_some(PassionTarget { kind, key })
    }

    /// Run the live moments that `trigger` starts for `id` on `when`: each whose conditions hold rolls its chance times the tier factor
    /// (purpose `passion.<moment>`) and, on success, starts or renews a passion.
    fn run_moments<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        trigger: &MomentTrigger,
        killer_house: Option<&str>,
        when: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let lifecycle = world.lifecycle;
        let rules = &lifecycle.connections;
        let full = self.tier(id, when) == Some(Tier::Full);
        let tier = if full {
            rules.passions.full_ppb
        } else {
            rules.passions.coarse_ppb
        };
        for moment in rules
            .passions
            .moments
            .iter()
            .filter(|m| m.live && m.trigger == *trigger)
        {
            let Some(c) = self.live.get(&id) else {
                return Ok(());
            };
            let pop = pop_facts(world.rules, world.populations, &c.pop)?;
            if !holds(&moment.when, c, &pop, age_at(c, when)?)
                || !self.moment_holds(c, moment, when)?
            {
                continue;
            }
            let chance = exact_ppb(moment.chance_ppb, &[(u128::from(tier), u128::from(PPB))]);
            let purpose = format!("passion.{}", moment.key);
            if chance == 0
                || !roll(
                    &mut self.dated_stream(world.seed, id, &purpose, when)?,
                    chance,
                )?
            {
                continue;
            }
            self.gain_passion(world, id, moment, killer_house, when, first);
        }
        Ok(())
    }

    /// Start a passion at `start_strength` or add 1 to the same kind and target (max 10), setting `renewed`; the lines are written after.
    fn gain_passion<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        moment: &Moment,
        killer_house: Option<&str>,
        when: GameTime,
        first: &mut FirstError,
    ) {
        let lifecycle = world.lifecycle;
        let rules = &lifecycle.connections;
        let Some(c) = self.live.get_mut(&id) else {
            return;
        };
        let Some(target) = Self::passion_target(world.populations, c, &moment.target, killer_house)
        else {
            return;
        };
        let Some(kind) = PASSION_KINDS.iter().position(|known| *known == moment.kind) else {
            return;
        };
        let kind = u8::try_from(kind).unwrap_or(0);
        let name = c.name.clone();
        let (line, new) = if let Some(passion) = c
            .ties
            .passions
            .iter_mut()
            .find(|p| p.kind == kind && p.target == target)
        {
            let old = passion.strength;
            passion.strength = (old + 1).min(MAX_STRENGTH);
            passion.renewed = passion.renewed.max(when);
            (
                (old < MAX_STRENGTH && passion.strength == MAX_STRENGTH)
                    .then(|| rules.passion_at_10.clone()),
                passion.strength,
            )
        } else {
            let start = u8::try_from(rules.passions.start_strength)
                .unwrap_or(1)
                .clamp(1, MAX_STRENGTH);
            c.ties.passions.push(Passion {
                kind,
                target: target.clone(),
                strength: start,
                renewed: when,
                last_fade: when,
            });
            (rules.passion_gained.get(&moment.kind).cloned(), start)
        };
        let _ = new;
        if let Some(text) = line {
            let rendered = text
                .replace("{name}", &name)
                .replace("{target}", &Self::target_text(world.populations, &target))
                .replace("{kind}", &moment.kind);
            Self::line(world, id, when, &rendered, first);
        }
    }

    /// The text a log line shows for a passion's target: the source's display name, else the canonical key.
    fn target_text<P: Populations>(populations: &P, target: &PassionTarget) -> String {
        TARGET_KINDS
            .get(usize::from(target.kind))
            .and_then(|kind| populations.target_name(kind, &target.key))
            .unwrap_or_else(|| target.key.clone())
    }

    /// The fade of `id`'s passions on `when` (CK-368 §6): while `when` is at or past the later of `renewed` and `last_fade` plus
    /// `fade_years`, a passion loses 1 and its last fade advances by `fade_years`; at 0 it ends.
    fn fade_passions<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        when: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let lifecycle = world.lifecycle;
        let rules = &lifecycle.connections;
        let years = rules.passions.fade_years;
        let Some(c) = self.live.get_mut(&id) else {
            return Ok(());
        };
        let name = c.name.clone();
        let mut ended = Vec::new();
        for passion in &mut c.ties.passions {
            loop {
                let due = plus_years(passion.renewed.max(passion.last_fade), years)?;
                if when < due {
                    break;
                }
                passion.strength = passion.strength.saturating_sub(1);
                passion.last_fade = due;
                if passion.strength == 0 {
                    ended.push((passion.kind, passion.target.clone()));
                    break;
                }
            }
        }
        c.ties.passions.retain(|p| p.strength > 0);
        for (kind, target) in ended {
            let label = PASSION_KINDS.get(usize::from(kind)).copied().unwrap_or("");
            let rendered = rules
                .passion_faded
                .replace("{name}", &name)
                .replace("{target}", &Self::target_text(world.populations, &target))
                .replace("{kind}", label);
            Self::line(world, id, when, &rendered, first);
        }
        Ok(())
    }

    /// A character's yearly passions on `when`: the yearly moments first (a renewal on a due date happens before the fade), then the fade.
    fn connection_passions<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        when: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        if !world.lifecycle.connections.loaded {
            return Ok(());
        }
        self.run_moments(world, id, &MomentTrigger::Yearly, None, when, first)?;
        self.fade_passions(world, id, when, first)
    }

    /// Stage 11 of the birthday (CK-368 §4, §6): the strand rolls, the feeling fade and the passions, after the marriage wish.
    pub(crate) fn connection_birthday<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        when: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let lifecycle = world.lifecycle;
        let rules = &lifecycle.connections;
        if !rules.loaded {
            return Ok(());
        }
        let year = year_of(when)?;
        let full = self.tier(id, when) == Some(Tier::Full);
        // A character promoted to the full tier keeps its strands and rolls as full tier from this birthday. Its passions for the years it
        // spent in the coarse tier are backfilled by the first daily pass after the promotion, which starts the day (CK-368 §9), never here.
        self.connection_rolls(world, id, when, first)?;
        self.connection_fade(world, id, when, first)?;
        if full && let Some(c) = self.live.get_mut(&id) {
            c.ties.last_full = c.ties.last_full.max(year);
        }
        self.connection_passions(world, id, when, first)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Happenings that make strands (CK-368 §5)

impl CharacterStore {
    /// The strand rules and passion moments of a happening reported to `id`, after its events (CK-368 §5, §6). The strand rules run only
    /// if both characters are still alive.
    ///
    /// | happening | rule | strands |
    /// |---|---|---|
    /// | `battle_comrades` | `battle_comrades` | friend (a bond, one roll for the pair) |
    /// | `rescued` | `rescue` | love and respect of `id` toward the counterpart |
    /// | `insulted` | `insult` | hatred of `id` toward the counterpart |
    /// | `kin_killed` | `kin_killed` | hatred of `id` toward the counterpart (the killer) |
    /// | `duel` | `duel` | rival (a bond) and respect each toward the other |
    ///
    /// # Errors
    /// Refuses arithmetic that does not fit; a life-log failure is kept in `first`.
    pub(crate) fn after_happening<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        happening: &str,
        counterpart: Option<u64>,
        date: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let lifecycle = world.lifecycle;
        let rules = &lifecycle.connections;
        if !rules.loaded || !self.live.contains_key(&id) {
            return Ok(());
        }
        // The draws below are on the game's own seed: fixed here, after the happening's events have committed, if no pass has fixed it.
        self.fix_seed(world.seed);
        let rule = strand_rule_name(happening);
        if let Some(other) = counterpart
            && other != id
            && let Some(rule_data) = rules.strands.happenings.get(rule)
            // A rule whose chance is zero changes no strand, so it reaches no pair (CK-368 §9).
            && rule_data.chance_ppb > 0
        {
            let year = year_of(date)?;
            for kind_name in &rule_data.kinds {
                let Some(kind) = kind_index(kind_name) else {
                    continue;
                };
                let both = rule == "duel" && !is_bond(kind);
                let sides: Vec<(u64, u64)> = if both {
                    vec![(id, other), (other, id)]
                } else {
                    vec![(id, other)]
                };
                for (holder, partner) in sides {
                    if !self.live.contains_key(&holder) || !self.live.contains_key(&partner) {
                        continue;
                    }
                    let purpose = format!("happening.{rule}.{kind_name}.{partner}");
                    if roll(
                        &mut self.dated_stream(world.seed, holder, &purpose, date)?,
                        rule_data.chance_ppb,
                    )? && let Some(change) = self.bump(holder, partner, kind, year)
                    {
                        self.strand_lines(world, kind, change, holder, partner, date, first);
                    }
                }
            }
        }
        // The moments the happening starts: a killer's house comes from his live or archived record.
        let killer_house = match counterpart {
            Some(other) if happening == "kin_killed" => self
                .record(&*world.archive, other)?
                .map(|killer| killer.life.house),
            _ => None,
        };
        if self.live.contains_key(&id) {
            self.run_moments(
                world,
                id,
                &MomentTrigger::Happening(happening.to_owned()),
                killer_house.as_deref(),
                date,
                first,
            )?;
        }
        Ok(())
    }

    /// Report a killing (CK-368 §5), used after the victim's death has committed: the victim's record is read from the archive (or live,
    /// if called before death) and no event is dispatched for the victim. `kin_killed` is dispatched with the killer as counterpart to
    /// each surviving close kin of the victim (parents, children, siblings and spouses, each once); the hatred applies only if the
    /// killer is alive, and the passion goes toward the killer's house from his live or archived record.
    ///
    /// # Errors
    /// Refuses a victim nobody holds and an archive read that fails; the first failure of a later step is returned once every relative has
    /// been looked at.
    pub fn report_killing<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        victim: u64,
        killer: u64,
        date: GameTime,
    ) -> Result<(), EventError> {
        self.fix_seed(world.seed);
        let record = self
            .record(&*world.archive, victim)?
            .ok_or(Error::MissingCharacter)?;
        let mut kin: BTreeSet<u64> = BTreeSet::new();
        let parents: Vec<u64> = [record.father, record.mother]
            .into_iter()
            .filter(|parent| *parent != UNKNOWN)
            .collect();
        kin.extend(parents.iter().copied());
        kin.extend(record.spouses.iter().copied());
        kin.extend(self.children_of(&*world.archive, victim)?);
        for parent in &parents {
            kin.extend(self.children_of(&*world.archive, *parent)?);
        }
        kin.remove(&victim);
        let mut first: FirstError = None;
        for relative in kin {
            if !self.live.contains_key(&relative) {
                continue;
            }
            if let Err(error) =
                self.report_happening_with(world, relative, "kin_killed", Some(killer), date)
            {
                keep(&mut first, error);
            }
        }
        conclude(first)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Settling in and the burn-in (CK-368 §9)

impl CharacterStore {
    /// Backfill the passions of a character promoted to the full tier (CK-368 §9). On the first pass after the promotion the **window** is
    /// fixed and saved on the character as its own commit. `from` is the later of `last_full_year + 1`, the unadjusted calendar cutoff
    /// (the last completed year that day minus `max_years - 1`) and the year after birth. `to` is that last completed year, stopping before
    /// any year a full-tier birthday already rolled live since the promotion; this does not change the cutoff. An empty window saves nothing
    /// and backfills nothing. Then each
    /// yearly live moment whose condition holds for a year of the window rolls once at the top-up chance (purpose
    /// `passion.backfill.<moment>.<year>`, seeded by the saved game seed and that year's birthday), and a success is the gain or renewal of
    /// §6 as of that birthday. All of it is **one commit** with the clearing of the saved window and the last full-tier year set to the later
    /// of its current value and `to` (never lowered)
    /// (a character's own passions and record only); a failure before it commits nothing, keeps the window and is tried again by the next
    /// pass with the same years and draws. A failed life-log line after the commit is not retried. No strand is replayed and no fade applied.
    ///
    /// # Errors
    /// Refuses a pop the engine cannot resolve and arithmetic that does not fit, before anything is committed.
    pub(crate) fn backfill_passions<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        last_full_year: u16,
        today: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let lifecycle = world.lifecycle;
        let rules = &lifecycle.connections;
        if !rules.loaded {
            return Ok(());
        }
        self.fix_seed(world.seed);
        let Some(c) = self.live.get(&id).cloned() else {
            return Ok(());
        };
        let (mut from, mut to) = c.ties.backfill;
        if from == 0 {
            // The window, fixed on this first pass.
            let current = year_of(today)?;
            let reach = u16::try_from(rules.max_years).unwrap_or(u16::MAX);
            // The calendar's last completed year, from which the lookback counts whatever else shortens the window.
            let calendar = current.saturating_sub(1);
            to = calendar;
            // A full-tier birthday that ran after the promotion (the marker is ahead of the baseline) already gave its year its live roll: the
            // window stops before it, so that year is never topped up as well.
            if c.ties.last_full > last_full_year {
                to = to.min(c.ties.last_full.saturating_sub(1));
            }
            from = last_full_year
                .saturating_add(1)
                .max(calendar.saturating_add(1).saturating_sub(reach))
                .max(date_of(c.birth)?.year.saturating_add(1));
            if from > to {
                if let Some(live) = self.live.get_mut(&id) {
                    live.ties.promoted = None;
                    live.ties.last_full = live.ties.last_full.max(to);
                }
                return Ok(());
            }
            if let Some(live) = self.live.get_mut(&id) {
                live.ties.promoted = None;
                live.ties.backfill = (from, to);
            }
        }
        let pop = pop_facts(world.rules, world.populations, &c.pop)?;
        let mut wins: Vec<(GameTime, usize)> = Vec::new();
        for year in from..=to {
            let when = birthday_in(&c, year)?;
            for (index, moment) in rules.passions.moments.iter().enumerate() {
                if !moment.live || moment.trigger != MomentTrigger::Yearly {
                    continue;
                }
                // The conditions for that year: the years of a marker, a move or a pop are counted up to its birthday; the rest are the
                // values on the day of the attempt (no history is stored).
                if !holds(&moment.when, &c, &pop, age_at(&c, when)?)
                    || !self.moment_holds(&c, moment, when)?
                {
                    continue;
                }
                let q = top_up_chance_ppb(
                    moment.chance_ppb,
                    rules.passions.full_ppb,
                    rules.passions.coarse_ppb,
                );
                if q == 0 {
                    continue;
                }
                let purpose = format!("passion.backfill.{}.{year}", moment.key);
                let mut stream = self.dated_stream(world.seed, id, &purpose, when)?;
                if roll(&mut stream, q)? {
                    wins.push((when, index));
                }
            }
        }
        // Everything is decided: one commit, then the output.
        if let Some(live) = self.live.get_mut(&id) {
            live.ties.backfill = (0, 0);
            live.ties.last_full = live.ties.last_full.max(to);
        }
        for (when, index) in wins {
            self.gain_passion(world, id, &rules.passions.moments[index], None, when, first);
        }
        Ok(())
    }

    /// Settle in a character promoted to the full tier, named by its caller with the year of its last full-tier roll (CK-368 §9): its passions
    /// are backfilled for the years it missed, and nothing else (a character keeps its strands and rolls as full tier from its next birthday).
    ///
    /// # Errors
    /// Refuses a pop the engine cannot resolve and arithmetic that does not fit: nothing of the backfill is committed on a failure (the saved
    /// window, fixed on the first attempt, stays for the next pass).
    pub fn settle_in<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        last_full_year: u16,
        today: GameTime,
    ) -> Result<(), EventError> {
        let mut first: FirstError = None;
        // A year already completed (the character's recorded progress) is never backfilled again, whatever baseline the caller gives.
        let baseline = self
            .live
            .get(&id)
            .map_or(last_full_year, |c| last_full_year.max(c.ties.last_full));
        self.backfill_passions(world, id, baseline, today, &mut first)?;
        conclude(first)
    }

    /// Backfill the passions of every character that has been promoted to the full tier since its last full-tier roll, at the start of a daily
    /// pass and before its other work (CK-368 §9): the first daily pass after the promotion does it once, and records the last completed year.
    ///
    /// # Errors
    /// The first failure, once every promoted character has been looked at; a failed one commits nothing and is tried again by the next pass.
    pub fn backfill_promotions<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        today: GameTime,
    ) -> Result<(), EventError> {
        if !world.lifecycle.connections.loaded {
            return Ok(());
        }
        // The characters whose promotion is not yet backfilled, and those with a saved window still to finish.
        let promoted: Vec<(u64, u16)> = self
            .live
            .values()
            .filter(|c| c.ties.promoted.is_some() || c.ties.backfill.0 > 0)
            .map(|c| (c.id, c.ties.promoted.unwrap_or(c.ties.last_full)))
            .collect();
        let mut first: FirstError = None;
        for (id, last) in promoted {
            if let Err(error) = self.backfill_passions(world, id, last, today, &mut first) {
                keep(&mut first, error);
            }
        }
        conclude(first)
    }

    /// The burn-in (CK-368 §9): `years` yearly rolls for every live character in date order, the passion moments included, ending with the
    /// last completed year before `today`; slice 8 calls it at the 768 start. Each year every character rolls on its birthday of that year
    /// (in date order, then id), and the closed year is then resolved for every linked pair.
    ///
    /// # Errors
    /// The first failure, once every year has been run.
    pub fn burn_in<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        years: u32,
        today: GameTime,
    ) -> Result<(), EventError> {
        self.burn_in_with(world, years, today, |_, _, _, _, _| Ok(()))
    }

    /// The burn-in with a step of the caller's own at the head of every character's year (CK-370 §3 step 7): `yearly(store, world, id, day,
    /// age)` runs on the character's birthday of the year, **before** its strand and passion stages, so that one pass over the years does
    /// slice 4's yearly stages (events, crafts, skills, standing) and slice 6's (strands, passions) together. A failure is kept and the years
    /// go on, as for the stages themselves.
    ///
    /// # Errors
    /// The first failure, once every year has been run.
    pub(crate) fn burn_in_with<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        years: u32,
        today: GameTime,
        mut yearly: impl FnMut(
            &mut Self,
            &mut World<'_, P, A, L>,
            u64,
            GameTime,
            u32,
        ) -> Result<(), EventError>,
    ) -> Result<(), EventError> {
        if !world.lifecycle.connections.loaded {
            return Ok(());
        }
        self.fix_seed(world.seed);
        let current = year_of(today)?;
        let from = current.saturating_sub(u16::try_from(years).unwrap_or(u16::MAX));
        let mut first: FirstError = None;
        for year in from..current {
            let mut order: Vec<(GameTime, u64)> = Vec::new();
            for c in self.live.values() {
                if date_of(c.birth)?.year < year {
                    order.push((birthday_in(c, year)?, c.id));
                }
            }
            order.sort();
            for (when, id) in order {
                let Some(born) = self.live.get(&id).map(|c| c.birth) else {
                    continue;
                };
                let age = u32::from(year.saturating_sub(date_of(born)?.year));
                if let Err(error) = yearly(self, world, id, when, age) {
                    keep(&mut first, error);
                }
                for step in [0, 1, 2] {
                    let result = match step {
                        0 => self.connection_rolls(world, id, when, &mut first),
                        1 => self.connection_fade(world, id, when, &mut first),
                        _ => self.connection_passions(world, id, when, &mut first),
                    };
                    if let Err(error) = result {
                        keep(&mut first, error);
                    }
                }
            }
            let closing = time_of(year.saturating_add(1), 1, 1)?;
            for (a, b) in self.linked_pairs() {
                if let Err(error) = self.resolve_pair_year(world, a, b, year, closing, &mut first) {
                    keep(&mut first, error);
                }
            }
        }
        conclude(first)
    }
}

#[cfg(feature = "test-fixtures")]
impl CharacterStore {
    /// How many characters candidate discovery has visited since the store was made or the count was reset: the work the circles did.
    #[must_use]
    pub fn candidates_visited(&self) -> u64 {
        self.visited.0.get()
    }

    /// Reset the count of [`candidates_visited`](Self::candidates_visited).
    pub fn reset_candidates_visited(&self) {
        self.visited.0.set(0);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The public reads

impl CharacterStore {
    /// The contact of two live characters now, in ppb (CK-368 §3): computed from where they are, never stored.
    ///
    /// # Errors
    /// Refuses a missing character.
    pub fn contact_ppb<P: Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        a: u64,
        b: u64,
    ) -> Result<u32, Error> {
        let (a, b) = (
            self.live.get(&a).ok_or(Error::MissingCharacter)?,
            self.live.get(&b).ok_or(Error::MissingCharacter)?,
        );
        Ok(self.contact_now(&world.lifecycle.connections, world.populations, a, b))
    }

    /// The circle of a live character on `when` (CK-368 §4): the ids of its closest partners, by contact and then id.
    ///
    /// # Errors
    /// Refuses a missing character and a date outside the calendar.
    pub fn circle_of<P: Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        id: u64,
        when: GameTime,
    ) -> Result<Vec<u64>, Error> {
        let a = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        Ok(self
            .circle(&world.lifecycle.connections, world.populations, a, when)?
            .into_iter()
            .map(|(id, _, _)| id)
            .collect())
    }

    /// The yearly chance in ppb of a strand of `kind` from `from` toward `to` on `when` (CK-368 §4), with the pair's current contact: `base
    /// x contact x factors`, exact, a negative result 0, capped at 1. For a bond either end gives the same chance.
    ///
    /// # Errors
    /// Refuses a missing character, an unknown kind and an archive read that fails.
    pub fn strand_chance_ppb<P: Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        from: u64,
        to: u64,
        kind: &str,
        when: GameTime,
    ) -> Result<u32, Error> {
        let rules = &world.lifecycle.connections;
        let kind = kind_index(kind).ok_or(Error::Event)?;
        let (a, b) = (
            self.live.get(&from).ok_or(Error::MissingCharacter)?,
            self.live.get(&to).ok_or(Error::MissingCharacter)?,
        );
        let contact = self.contact_now(rules, world.populations, a, b);
        let mut cache = BTreeMap::new();
        if is_bond(kind) {
            self.bond_chance(
                rules,
                &*world.archive,
                &mut cache,
                kind,
                a,
                b,
                contact,
                when,
            )
        } else {
            self.feeling_chance(rules, &*world.archive, &mut cache, kind, a, b, contact)
        }
    }

    /// `a`'s opinion of `b` with the terms of its strands and passions (CK-368 §7): a pure function of the two records and `b`'s pop.
    ///
    /// # Errors
    /// Refuses a missing character and arithmetic that does not fit.
    pub fn opinion<P: Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        a: u64,
        b: u64,
    ) -> Result<i64, Error> {
        let (a, b) = (
            self.live.get(&a).ok_or(Error::MissingCharacter)?,
            self.live.get(&b).ok_or(Error::MissingCharacter)?,
        );
        opinion(
            world.personality,
            &world.lifecycle.connections,
            a,
            b,
            &PopTargets::of(world.populations, b),
        )
    }
}
