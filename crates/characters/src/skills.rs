//! The birthday pass in one order, skills (growth by use and home education), the craft rise and the monthly precedence check
//! (CK-363, characters slice 4).
//!
//! Every formula is evaluated exactly over integers (thousandths) and rounded once to the nearest thousandth, ties away from zero.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, Character, CharacterStore, Error, PopLocator, Populations, Sex, character_stream,
    events::{
        CRAFT_RISE_TEXT, EventError, FirstError, LogSink, PopFacts, Step, World, age_years, keep,
        pop_facts, render,
    },
    genetics::roll,
    personality::{drift_and_fade, has_birthday, product},
};
use ck_economy::{
    EventRules,
    bigint::{self, Big},
    events::{GRADE_NAMES, MAX_GRADE, SkillRules, Trigger},
    genetics::{PPB, round_div},
};
use ck_sim::GameTime;
use std::collections::{BTreeMap, BTreeSet};

/// The age from which a character's yearly events run.
pub const YEARLY_FROM: u32 = 15;
/// Aptitude per point of the stat above 10, in thousandths: `1 + 0.05 (stat - 10)`.
const APTITUDE_PER_POINT: i128 = 50;
/// Diligence per point of `sloth_diligence`, in thousandths: `1 + 0.05 x sloth_diligence`.
const DILIGENCE_PER_POINT: i128 = 50;
/// The index of `sloth_diligence` among the vice/virtue pairs.
const SLOTH_DILIGENCE: usize = 6;
/// The level at which the growth curve halves, in thousandths: the divisor is `1 + (level / 12)^2`.
const CURVE_LEVEL: i128 = 12;
const MILLION: i128 = 1_000_000;

/// The pops, skills and vice/virtue and manner values of every live character when the day's birthday pass starts, and who its
/// parents' children are (CK-367).
pub(crate) struct Snapshot {
    roster: BTreeMap<PopLocator, Vec<u64>>,
    skills: BTreeMap<u64, BTreeMap<String, i64>>,
    /// The vice/virtue and manner values of each live character.
    minds: BTreeMap<u64, ([i64; 7], [i64; 9])>,
    /// The live children of each recorded parent, by recorded parentage.
    children: BTreeMap<u64, BTreeSet<u64>>,
}

impl Snapshot {
    /// The values of `c`'s household: its parents and its siblings (the other children of either parent) that are in the snapshot.
    pub(crate) fn household(&self, c: &Character) -> Vec<([i64; 7], [i64; 9])> {
        let mut members: BTreeSet<u64> = BTreeSet::new();
        for parent in [c.father, c.mother] {
            if parent == crate::UNKNOWN {
                continue;
            }
            members.insert(parent);
            members.extend(self.children.get(&parent).into_iter().flatten());
        }
        members.remove(&c.id);
        members
            .iter()
            .filter_map(|member| self.minds.get(member).copied())
            .collect()
    }
}

fn aptitude(rules: &SkillRules, c: &Character, skill: &str) -> i128 {
    rules.aptitude.get(skill).map_or(MILLION, |stat| {
        MILLION + APTITUDE_PER_POINT * (i128::from(c.genetics.value[*stat]) - 10_000)
    })
}

fn sex_key(c: &Character) -> char {
    match c.sex {
        Sex::Male => 'm',
        Sex::Female => 'f',
    }
}

/// A skill's level after a year's growth by use and decay (§6), in thousandths, evaluated exactly and rounded once:
/// `gain = gain_rate x use x aptitude x diligence / (1 + (level / 12)^2)`, `loss = level x (decay_used x use + decay_unused x (1 - use))`.
pub(crate) fn grown(
    rules: &SkillRules,
    c: &Character,
    skill: &str,
    pop: &PopFacts,
) -> Result<i64, Error> {
    grown_fast(rules, c, skill, pop).or_else(|_| grown_exact(rules, c, skill, pop))
}

/// [`grown`] in 128 bits; it refuses a level whose intermediates do not fit.
fn grown_fast(
    rules: &SkillRules,
    c: &Character,
    skill: &str,
    pop: &PopFacts,
) -> Result<i64, Error> {
    let profile = rules
        .profiles
        .get(&(pop.kind.clone(), sex_key(c)))
        .and_then(|profile| profile.get(skill))
        .copied()
        .unwrap_or(0);
    let factor = c.life.use_factor.get(skill).copied().unwrap_or(1000);
    let focus = if rules.focus_on_use {
        focus_multiplier(rules, c, skill)
    } else {
        1000
    };
    let used = (i128::from(profile) * i128::from(factor) * focus).min(1_000_000_000);
    let diligence = MILLION + DILIGENCE_PER_POINT * i128::from(c.personality.vice[SLOTH_DILIGENCE]);
    let level = i128::from(c.skill(skill));
    let curve = (CURVE_LEVEL * 1000) * (CURVE_LEVEL * 1000) + level * level;
    // gain = gain_num / gain_den; loss = loss_num / 1e12, the level times the decay of the used and unused shares.
    let gain_num = product(&[
        i128::from(rules.gain_rate),
        used,
        aptitude(rules, c, skill),
        diligence,
        (CURVE_LEVEL * 1000) * (CURVE_LEVEL * 1000),
    ])?;
    let gain_den = product(&[1_000_000_000_000_000_000_000, curve])?;
    let loss_num = product(&[
        level,
        i128::from(rules.decay_used) * used
            + i128::from(rules.decay_unused) * (1_000_000_000 - used),
    ])?;
    // level + gain - loss rounded once to the nearest thousandth, ties away from zero, without ever multiplying the level by the
    // common denominator: the whole parts and the remainders of the two fractions are taken apart, over `gain_den`.
    let (gain_whole, gain_rest) = (gain_num.div_euclid(gain_den), gain_num.rem_euclid(gain_den));
    let (loss_whole, loss_rest) = (
        loss_num.div_euclid(1_000_000_000_000),
        loss_num.rem_euclid(1_000_000_000_000),
    );
    let fraction = gain_rest - loss_rest * (gain_den / 1_000_000_000_000);
    let (carry, rest) = (fraction.div_euclid(gain_den), fraction.rem_euclid(gain_den));
    let whole = level + gain_whole - loss_whole + carry;
    let up = rest > gain_den - rest || (rest == gain_den - rest && whole >= 0);
    i64::try_from((whole + i128::from(up)).max(0)).map_err(|_| Error::Event)
}

/// [`grown`] in unbounded integers, for a level whose intermediates do not fit 128 bits: the same formula over the common denominator
/// `1e21 x (12,000^2 + level^2)`, `level x den + gain - loss` rounded once. The loss never exceeds the level, so it is not negative.
fn grown_exact(
    rules: &SkillRules,
    c: &Character,
    skill: &str,
    pop: &PopFacts,
) -> Result<i64, Error> {
    let profile = rules
        .profiles
        .get(&(pop.kind.clone(), sex_key(c)))
        .and_then(|profile| profile.get(skill))
        .copied()
        .unwrap_or(0);
    let factor = c.life.use_factor.get(skill).copied().unwrap_or(1000);
    let focus = if rules.focus_on_use {
        focus_multiplier(rules, c, skill)
    } else {
        1000
    };
    let used =
        u128::try_from((i128::from(profile) * i128::from(factor) * focus).min(1_000_000_000))
            .map_err(|_| Error::Event)?;
    let nonneg = |value: i128| u128::try_from(value.max(0)).unwrap_or(0);
    let diligence =
        nonneg(MILLION + DILIGENCE_PER_POINT * i128::from(c.personality.vice[SLOTH_DILIGENCE]));
    let level = Big::new(nonneg(i128::from(c.skill(skill))));
    let flat = Big::new(144_000_000);
    let curve = flat.add(&level.mul(&level));
    let den = Big::new(1_000_000_000_000_000_000_000).mul(&curve);
    let gain = bigint::product(&[
        nonneg(i128::from(rules.gain_rate)),
        used,
        nonneg(aptitude(rules, c, skill)),
        diligence,
        144_000_000,
    ]);
    let decay = nonneg(
        i128::from(rules.decay_used) * i128::try_from(used).map_err(|_| Error::Event)?
            + i128::from(rules.decay_unused)
                * (1_000_000_000 - i128::try_from(used).map_err(|_| Error::Event)?),
    );
    let loss = level
        .mul(&Big::new(decay))
        .mul(&Big::new(1_000_000_000))
        .mul(&curve);
    let total = level
        .mul(&den)
        .add(&gain)
        .sub(&loss)
        .unwrap_or_else(|| Big::new(0));
    let rounded = total.round_div(&den);
    i64::try_from(rounded.to_u128().ok_or(Error::Event)?).map_err(|_| Error::Event)
}

fn focus_multiplier(rules: &SkillRules, c: &Character, skill: &str) -> i128 {
    match rules.focus_skills.get(&c.life.focus) {
        Some(skills) if !skills.is_empty() => i128::from(if skills.iter().any(|s| s == skill) {
            rules.focus_on
        } else {
            rules.focus_off
        }),
        _ => 1000,
    }
}

fn age_factor(rules: &SkillRules, age: u32) -> i128 {
    rules
        .age_factor
        .iter()
        .rev()
        .find(|(from, _)| *from <= age)
        .map_or(0, |(_, factor)| i128::from(*factor))
}

/// A skill's level after home education (§6): `level += rate x age_factor x aptitude x (teacher - level) x (1 + scholarship_per_point
/// x scholarship) x focus`, when the teacher's level is higher. `teacher` and `scholarship` are the snapshot's.
fn educated(
    rules: &SkillRules,
    c: &Character,
    skill: &str,
    age: u32,
    teacher: i64,
    scholarship: i64,
) -> Result<i64, Error> {
    let level = c.skill(skill);
    if teacher <= level {
        return Ok(level);
    }
    let fast = product(&[
        i128::from(rules.education_rate),
        age_factor(rules, age),
        aptitude(rules, c, skill),
        i128::from(teacher - level),
        MILLION + i128::from(rules.scholarship_per_point) * i128::from(scholarship),
        focus_multiplier(rules, c, skill),
    ]);
    let moved = if let Ok(gain) = fast {
        round_div(gain, 1_000_000_000_000_000_000_000)
    } else {
        // A level this large overflows 128 bits: the same product in unbounded integers.
        let nonneg = |value: i128| u128::try_from(value.max(0)).unwrap_or(0);
        let gain = bigint::product(&[
            nonneg(i128::from(rules.education_rate)),
            nonneg(age_factor(rules, age)),
            nonneg(aptitude(rules, c, skill)),
            nonneg(i128::from(teacher - level)),
            nonneg(MILLION + i128::from(rules.scholarship_per_point) * i128::from(scholarship)),
            nonneg(focus_multiplier(rules, c, skill)),
        ]);
        let rounded = gain.round_div(&Big::new(1_000_000_000_000_000_000_000));
        i128::try_from(rounded.to_u128().ok_or(Error::Event)?).map_err(|_| Error::Event)?
    };
    i64::try_from(i128::from(level) + moved).map_err(|_| Error::Event)
}

/// The chance, in ppb, of a craft's yearly rise: `rise x skill / 15`, rounded down and held to 1.
fn rise_chance(rules: &EventRules, skill: i64) -> Result<u32, Error> {
    let scaled = u128::from(rules.craft_rise.rise_ppb)
        .checked_mul(u128::try_from(skill.max(0)).map_err(|_| Error::Event)?)
        .ok_or(Error::Event)?
        / 15_000;
    u32::try_from(scaled.min(u128::from(PPB))).map_err(|_| Error::Event)
}

impl CharacterStore {
    fn snapshot(&self) -> Snapshot {
        let mut roster: BTreeMap<PopLocator, Vec<u64>> = BTreeMap::new();
        let mut skills = BTreeMap::new();
        let mut minds = BTreeMap::new();
        let mut children: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
        for c in self.live.values() {
            roster.entry(c.pop.clone()).or_default().push(c.id);
            skills.insert(c.id, c.life.skills.clone());
            minds.insert(c.id, (c.personality.vice, c.personality.manner));
            for parent in [c.father, c.mother] {
                if parent != crate::UNKNOWN {
                    children.entry(parent).or_default().insert(c.id);
                }
            }
        }
        Snapshot {
            roster,
            skills,
            minds,
            children,
        }
    }

    /// The birthday pass (§5), for every live character whose birthday is `today` (slice 3's rules), in identifier order: humour
    /// drift, standing fade, the yearly events (age 15 or more), the craft rise (for crafts held before the day's events), growth by
    /// use (from the training age), home education, upbringing (CK-367), the height sample (CK-367) and precedence. A death ends that character's later stages. A snapshot of every
    /// character's pop and skills is taken before the pass starts, and home education reads its teachers and the pupil's
    /// scholarship from it. A failing character does not stop the others: the first failure is returned when the pass is done.
    ///
    /// # Errors
    /// Refuses a date outside the calendar; returns the first failure of a character's pass (a pop the engine cannot resolve,
    /// arithmetic that does not fit, an archive or life-log failure). What a failing character had already committed stays.
    pub fn run_birthdays<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        today: GameTime,
    ) -> Result<(), EventError> {
        let date = today.date().map_err(|_| Error::Event)?;
        self.fix_seed(world.seed);
        let snapshot = self.snapshot();
        let mut due = Vec::new();
        for c in self.live.values() {
            if has_birthday(c.birth.date().map_err(|_| Error::Event)?, date) {
                due.push(c.id);
            }
        }
        let mut first: FirstError = None;
        for id in due {
            if self.live.contains_key(&id)
                && let Err(error) = self.birthday(world, id, today, &snapshot, &mut first)
            {
                keep(&mut first, error);
            }
        }
        first.map_or(Ok(()), Err)
    }

    fn birthday<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        today: GameTime,
        snapshot: &Snapshot,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let rules = world.rules;
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        // Stages 1 and 2: humour drift, then the standing fade.
        let personality = drift_and_fade(world.personality, c)?;
        let pop = pop_facts(rules, world.populations, &c.pop)?;
        let age = age_years(
            c.birth.date().map_err(|_| Error::Event)?,
            today.date().map_err(|_| Error::Event)?,
        );
        let crafts: Vec<String> = rules
            .craft_rise
            .skills
            .keys()
            .filter(|craft| c.grade(craft) > 0)
            .cloned()
            .collect();
        if let Some(c) = self.live.get_mut(&id) {
            c.personality = personality;
        }
        // Stage 3: the yearly events, in data order.
        if age >= YEARLY_FROM {
            for event in &rules.events {
                if event.trigger != Trigger::Yearly {
                    continue;
                }
                match self.run_event(world, event, id, today, first)? {
                    Step::Died => return Ok(()),
                    // A later yearly event reads what this one left: a committed change is followed by the same-day precedence
                    // check, as a happening's events are followed by it (§4). Stage 7 still covers drift, training and education.
                    Step::Applied => self.precedence(world, id, today, first)?,
                    Step::Continue => {}
                }
            }
        }
        // Stage 4: the craft rise, for crafts held before this birthday's events.
        for craft in &crafts {
            self.rise(world, id, craft, today, first)?;
        }
        let skills = &rules.skills;
        // Stage 5: growth by use.
        if age >= skills.train_from {
            let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
            let mut levels = Vec::new();
            for skill in &skills.list {
                levels.push((skill.clone(), grown(skills, c, skill, &pop)?));
            }
            if let Some(c) = self.live.get_mut(&id) {
                c.life.skills.extend(levels);
            }
        }
        // Stage 6: home education, from the snapshot.
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        let scholarship = snapshot
            .skills
            .get(&id)
            .and_then(|s| s.get("scholarship"))
            .copied()
            .unwrap_or(0);
        let mut levels = Vec::new();
        for skill in &skills.list {
            let teacher = snapshot
                .roster
                .get(&c.pop)
                .into_iter()
                .flatten()
                .filter(|other| **other != id)
                .filter_map(|other| snapshot.skills.get(other)?.get(skill).copied())
                .max()
                .unwrap_or(0);
            levels.push((
                skill.clone(),
                educated(skills, c, skill, age, teacher, scholarship)?,
            ));
        }
        if let Some(c) = self.live.get_mut(&id) {
            c.life.skills.extend(levels);
        }
        // Stage 6a (CK-367 §1): upbringing, from the snapshot's household.
        let household = snapshot.household(self.live.get(&id).ok_or(Error::MissingCharacter)?);
        self.upbringing(
            world,
            id,
            today,
            age,
            &household,
            world.lifecycle.upbringing.to_age,
        )?;
        // Stage 6b (CK-367 §2): the height sample and offset.
        self.height_sample(world, id, age)?;
        // Stage 7: precedence (so a vice that upbringing changed and that invalidates a calling removes it the same day).
        self.precedence(world, id, today, first)?;
        // Stage 8 (CK-364 §1): the birthday at which the character reaches majority, by its culture's subgroup and its sex, is the
        // `coming_of_age` happening, once (a character who is created of age, or past it, never has one).
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        let ages = world.lifecycle.majority.of_group(&pop.group);
        let majority = match c.sex {
            Sex::Male => ages.m,
            Sex::Female => ages.f,
        };
        if majority > 0 && age == majority {
            self.dispatch(world, id, "coming_of_age", today, first)?;
        }
        // Stage 9 (CK-365 §1): the yearly epithets, in file order, after precedence and coming of age and before the marriage wish
        // (CK-366); a character that died on the way is not tested.
        self.gain_epithets(world, id, today, &ck_economy::EpithetTrigger::Yearly)?;
        // Stage 10 (CK-366 §2): the marriage wish, after the epithets; a character that died on the way does not seek one.
        self.seek_marriage(world, id, today, first)?;
        // Stage 11 (CK-368 §4, §6): the strand rolls, the feeling fade and the passions, after the marriage wish; a character that died
        // on the way (or married away) is not rolled.
        if self.live.contains_key(&id) {
            self.connection_birthday(world, id, today, first)?;
        }
        Ok(())
    }

    /// One craft's yearly rise: `rise x skill / 15` of one grade up, at most SSS, with a line.
    pub(crate) fn rise<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        craft: &str,
        today: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let rules = world.rules;
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        let level = c.grade(craft);
        if level == 0 || level >= MAX_GRADE {
            return Ok(());
        }
        let skill = rules
            .craft_rise
            .skills
            .get(craft)
            .map_or(0, |skill| c.skill(skill));
        let p = rise_chance(rules, skill)?;
        let mut stream = character_stream(world.seed, id, &format!("craft_rise.{craft}"), today);
        if !roll(&mut stream, p)? {
            return Ok(());
        }
        let text = render(
            CRAFT_RISE_TEXT,
            &c.name,
            craft,
            GRADE_NAMES[usize::from(level + 1)],
            0,
        );
        if let Some(c) = self.live.get_mut(&id) {
            c.set_grade(craft, level + 1);
        }
        Self::line(world, id, today, &text, first);
        Ok(())
    }

    /// The monthly precedence check (§4), for every live character in identifier order: the conditions that read the pop (luxuries)
    /// can fail between birthdays, so on the 1st each attribute is checked and removed with a line when its conditions fail.
    ///
    /// # Errors
    /// Returns the first failure once every character has been checked.
    pub fn monthly_precedence<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        today: GameTime,
    ) -> Result<(), EventError> {
        let ids: Vec<u64> = self.live.keys().copied().collect();
        let mut first: FirstError = None;
        for id in ids {
            if let Err(error) = self.precedence(world, id, today, &mut first) {
                keep(&mut first, error);
            }
        }
        first.map_or(Ok(()), Err)
    }
}
