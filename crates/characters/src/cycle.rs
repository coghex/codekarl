//! Death, conception and delivery, and the names a character shows (CK-364, characters slice 5a).
//!
//! A day's character work runs in phases around the economy's step: **before** it the certain deaths and, on the 1st, the monthly
//! mortality and the deliveries ([`CharacterStore::run_pre_economy`]); **after** it, on the 1st, the conceptions
//! ([`CharacterStore::run_conceptions`]). Every chance is evaluated exactly (parts per billion, one rounding down) and rolled on the
//! stream of its purpose, so the work is deterministic under the master seed. No floating point.
//!
//! The life log is output, not simulation state: a line that cannot be appended after a committed change does not undo the change,
//! and a pass never retries a committed change and always goes on to the next character; the first failure is returned at the end.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, Character, CharacterStore, Context, Error, NewCharacter, PopLocator, Populations,
    Sex, UNKNOWN, character_stream,
    events::{EventError, FirstError, LogSink, World, age_years, exact_ppb, keep, render},
    genetics::{below, roll},
};
use ck_economy::{
    Genetics, LifeCycle, PopGenetics,
    genetics::{ATTRIBUTES, PPB},
    lifecycle::DeathInputs,
};
use ck_sim::{Date, GameTime};
use std::collections::BTreeMap;

/// The line of a death under two years of age (the owner checks the wording).
pub const DIED_IN_INFANCY: &str = "{name} died in infancy.";
/// The line of a death from two to fourteen (the owner checks the wording).
pub const DIED_YOUNG: &str = "{name} died young.";
/// The line of a death from fifteen to fifty-nine (the owner checks the wording).
pub const DIED: &str = "{name} died.";
/// The line of a death at sixty or more (the owner checks the wording).
pub const DIED_IN_OLD_AGE: &str = "{name} died in old age.";
/// The line of a mother who dies at a delivery (the owner checks the wording).
pub const DIED_IN_CHILDBIRTH: &str = "{name} died in childbirth.";
/// The cause recorded for a death by the monthly roll.
pub const CAUSE_NATURAL: &str = "natural";
/// The cause recorded for a death at `certain_at`.
pub const CAUSE_OLD_AGE: &str = "old_age";
/// The cause recorded for a mother's death at a delivery.
pub const CAUSE_CHILDBIRTH: &str = "childbirth";

/// The product of a pop's three death multipliers, in ‰ each, that its characters die under: `1_000_000_000` is a product of 1.
pub const NEUTRAL_PRESSURE: u64 = 1_000_000_000;
/// The pressure of each pop at the start of the day's pass (CK-364 §2: read once, before the pass changes any head).
pub type Pressure = BTreeMap<PopLocator, u64>;

/// The index of a stat in a genetic attribute array.
fn stat(name: &str) -> usize {
    ATTRIBUTES.iter().position(|a| *a == name).unwrap_or(0)
}
/// The index of `lust_chastity` among the vice/virtue pairs.
const LUST_CHASTITY: usize = 2;
/// The pop's religion that keeps the Latin church's days of abstinence.
const LATIN: &str = "christianity";
/// The conditions a mother's chance of dying is multiplied by under 16 or over 40, as a fraction.
const EXTREME_AGE_RISK: (u32, u32) = (3, 2);

/// The line of a death at an age (§2).
#[must_use]
pub fn death_line(age: u32) -> &'static str {
    match age {
        0..=1 => DIED_IN_INFANCY,
        2..=14 => DIED_YOUNG,
        15..=59 => DIED,
        _ => DIED_IN_OLD_AGE,
    }
}

/// The result of a delivery that committed: its children, and the first output failure after the commit (a life-log line that could
/// not be appended, or a happening that could not run), which does not undo it.
#[derive(Debug)]
pub struct Delivery {
    /// The children, in identifier order.
    pub children: Vec<u64>,
    /// The first failure after the commit.
    pub output: Option<EventError>,
}

/// The first of the month `months` after `day`'s month, at hour 0.
fn first_after(day: GameTime, months: u32) -> Result<GameTime, Error> {
    let d = day.date().map_err(|_| Error::Overflow)?;
    let index = u32::from(d.year) * 12 + u32::from(d.month) - 1 + months;
    GameTime::from_date(Date {
        year: u16::try_from(index / 12).map_err(|_| Error::Overflow)?,
        month: u8::try_from(index % 12 + 1).map_err(|_| Error::Overflow)?,
        day: 1,
        hour: 0,
    })
    .map_err(|_| Error::Overflow)
}

/// Whether the day of `a` is on or before the day of `b`.
fn on_or_before(a: GameTime, b: GameTime) -> Result<bool, Error> {
    let (a, b) = (
        a.date().map_err(|_| Error::Overflow)?,
        b.date().map_err(|_| Error::Overflow)?,
    );
    Ok((a.year, a.month, a.day) <= (b.year, b.month, b.day))
}

/// A fecundity curve at an age as the exact ratio `(numerator, denominator)` of a factor: linear between the points, the first
/// point's factor below the first age and the last point's above the last.
fn curve_ratio(points: &[(u32, u64)], age: u32) -> (u128, u128) {
    let ppb = u128::from(PPB);
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return (0, 1);
    };
    if age <= first.0 {
        return (u128::from(first.1), ppb);
    }
    if age >= last.0 {
        return (u128::from(last.1), ppb);
    }
    for pair in points.windows(2) {
        let ((a0, f0), (a1, f1)) = (pair[0], pair[1]);
        if age < a1 {
            let (span, along) = (i128::from(a1 - a0), i128::from(age - a0));
            let value = i128::from(f0) * span + (i128::from(f1) - i128::from(f0)) * along;
            return (
                u128::try_from(value).unwrap_or(0),
                u128::from(a1 - a0) * ppb,
            );
        }
    }
    (u128::from(last.1), ppb)
}

/// The shown name of `c`: its base name and, where its recorded pool has a patronym and its father's recorded pool has the father's
/// base name, the patronym and the father's genitive (CK-364 §5). Derived, never stored, and never through a current pop or culture.
#[must_use]
pub fn shown_name(c: &Character, father: Option<&Character>, lifecycle: &LifeCycle) -> String {
    let word = lifecycle
        .pools
        .get(&c.life.pool)
        .and_then(|pool| pool.patronym.as_ref())
        .map(|patronym| match c.sex {
            Sex::Male => patronym.m.as_str(),
            Sex::Female => patronym.f.as_str(),
        });
    let genitive = father.and_then(|father| {
        lifecycle
            .pools
            .get(&father.life.pool)?
            .men
            .iter()
            .find(|(name, _)| *name == father.name)
            .map(|(_, genitive)| genitive.as_str())
    });
    match (word, genitive) {
        (Some(word), Some(genitive)) => format!("{} {word} {genitive}", c.name),
        _ => c.name.clone(),
    }
}

impl CharacterStore {
    /// The shown name of a live or archived character (see [`shown_name`]).
    ///
    /// # Errors
    /// An archive read failure.
    pub fn shown_name_of(
        &self,
        c: &Character,
        lifecycle: &LifeCycle,
        archive: &impl ArchiveSink,
    ) -> std::io::Result<String> {
        let father = if c.father == UNKNOWN {
            None
        } else if let Some(live) = self.get(c.father) {
            Some(live.clone())
        } else {
            archive.get(c.father)?
        };
        Ok(shown_name(c, father.as_ref(), lifecycle))
    }

    /// Commit the death of a live character with `cause` and write its line: the archive append first (a failure refuses the death
    /// with nothing changed), then the removal of its head from its pop, its line and the bereavement of its relatives.
    fn die_of_cause<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        date: GameTime,
        cause: &str,
        text: &str,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let dead = self.live.get(&id).cloned().ok_or(Error::MissingCharacter)?;
        self.commit_death(world, &dead, date, cause)?;
        Self::line(world, id, date, &render(text, &dead.name, "", "", 0), first);
        self.bereave(world, &dead, date, first);
        Ok(())
    }

    fn age_on(c: &Character, date: GameTime) -> Result<u32, Error> {
        Ok(age_years(
            c.birth.date().map_err(|_| Error::Event)?,
            date.date().map_err(|_| Error::Event)?,
        ))
    }

    /// Phase A of a day, before the economy's step (§4): the certain deaths and, on the 1st, the monthly mortality and then the
    /// deliveries. Each part runs whatever the others reported; the first failure is returned when all are done.
    ///
    /// # Errors
    /// The first failure of a part.
    pub fn run_pre_economy<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        today: GameTime,
        pressure: &Pressure,
    ) -> Result<(), EventError> {
        let mut first: FirstError = None;
        let mut parts = vec![self.run_certain_deaths(world, today)];
        if today.date().map_err(|_| Error::Event)?.day == 1 {
            parts.push(self.run_monthly_deaths(world, today, pressure));
            parts.push(self.run_deliveries(world, today));
        }
        for part in parts {
            if let Err(error) = part {
                keep(&mut first, error);
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// The certain deaths: a character alive on the day it reaches `certain_at` (or found past it) dies that day, cause `old_age`, with
    /// no roll, in identifier order.
    ///
    /// # Errors
    /// The first failure, once every character has been looked at.
    pub fn run_certain_deaths<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        today: GameTime,
    ) -> Result<(), EventError> {
        let certain = world.lifecycle.death.certain_at;
        let ids: Vec<u64> = self.live.keys().copied().collect();
        let mut first: FirstError = None;
        for id in ids {
            let Some(c) = self.live.get(&id) else {
                continue;
            };
            let age = match Self::age_on(c, today) {
                Ok(age) => age,
                Err(error) => {
                    keep(&mut first, error.into());
                    continue;
                }
            };
            if age >= certain
                && let Err(error) =
                    self.die_of_cause(world, id, today, CAUSE_OLD_AGE, death_line(age), &mut first)
            {
                keep(&mut first, error);
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// The annual chance of dying of `c` in ppb on `today` (§2), under the pressure of its pop.
    ///
    /// # Errors
    /// Refuses a date outside the calendar.
    pub fn annual_death_ppb(
        c: &Character,
        genetics: &Genetics,
        lifecycle: &LifeCycle,
        pressure: &Pressure,
        today: GameTime,
    ) -> Result<u32, Error> {
        let death = &lifecycle.death;
        // The shown weight (the genetic value and a woman's offset, held to the bounds) against the rolled one.
        let weight = death.weight_multiplier(
            c.genetics.shown(genetics, c.sex, stat("weight")),
            c.genetics.value[stat("weight")],
        );
        Ok(death.annual_ppb(&DeathInputs {
            age: Self::age_on(c, today)?,
            constitution: c.genetics.value[stat("constitution")],
            longevity: c.genetics.value[stat("longevity")],
            pressure: pressure.get(&c.pop).copied().unwrap_or(NEUTRAL_PRESSURE),
            weight,
        }))
    }

    /// The monthly mortality, on the 1st: for every live character in identifier order the monthly chance that twelve rolls reach
    /// its annual chance, rolled on purpose `death`.
    ///
    /// # Errors
    /// The first failure, once every character has been rolled.
    pub fn run_monthly_deaths<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        today: GameTime,
        pressure: &Pressure,
    ) -> Result<(), EventError> {
        let ids: Vec<u64> = self.live.keys().copied().collect();
        let mut first: FirstError = None;
        for id in ids {
            let Some(c) = self.live.get(&id) else {
                continue;
            };
            // A character at or past `certain_at` belongs to the certain path, with no roll, even when its death was refused.
            if Self::age_on(c, today).is_ok_and(|age| age >= world.lifecycle.death.certain_at) {
                continue;
            }
            let rolled =
                Self::annual_death_ppb(c, world.genetics, world.lifecycle, pressure, today)
                    .and_then(|annual| {
                        roll(
                            &mut character_stream(world.seed, id, "death", today),
                            ck_economy::lifecycle::DeathRules::monthly_ppb(annual),
                        )
                    });
            match rolled {
                Ok(false) => {}
                Ok(true) => {
                    let age = Self::age_on(c, today).unwrap_or(0);
                    if let Err(error) = self.die_of_cause(
                        world,
                        id,
                        today,
                        CAUSE_NATURAL,
                        death_line(age),
                        &mut first,
                    ) {
                        keep(&mut first, error);
                    }
                }
                Err(error) => keep(&mut first, error.into()),
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// The deliveries due on `today` (the 1st), for each pregnant woman whose due 1st has come, in identifier order. A delivery that is
    /// refused leaves the pregnancy as it was, to be tried again on the next 1st.
    ///
    /// # Errors
    /// The first failure, refused or after a commit, once every woman has been looked at.
    pub fn run_deliveries<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        today: GameTime,
    ) -> Result<(), EventError> {
        let mut due = Vec::new();
        for c in self.live.values() {
            if c.life.pregnant_by.is_some()
                && let Some(day) = c.life.due
                && on_or_before(day, today)?
            {
                due.push(c.id);
            }
        }
        let mut first: FirstError = None;
        for id in due {
            if !self.live.contains_key(&id) {
                continue;
            }
            match self.deliver(world, id, today) {
                Ok(delivery) => {
                    if let Some(error) = delivery.output {
                        keep(&mut first, error);
                    }
                }
                Err(error) => keep(&mut first, error),
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// Deliver the child (or twins, or triplets) a pregnant woman carries (§3, §6): one roll `u` on purpose `delivery` gives triplets
    /// when `u < t`, twins when `u < t + w` and one child otherwise, with `t = triplets x b` and `w = twins x b` (`b = twin_bearer`
    /// when the mother shows it, else 1) and `t + w` capped at 1. Each child is made into the mother's **current** pop with the
    /// recorded father (live or archived).
    ///
    /// **One staged transition.** Every child's record, name, genetics, personality and parent links, the heads added, the pending
    /// counts, the cleared pregnancy and the nursing deadline are decided first and committed together, or nothing is: an error
    /// before the commit (a refused child, a father who cannot be read, a pop without room) leaves no id consumed, no link, no head,
    /// no counter and no line, and the pregnancy as it was. Only after the commit do the `child_born` happenings run (for the
    /// mother, then the father, children in identifier order); then the childbirth risk is rolled **once** (x1.5 under 16 or over 40)
    /// and a mother who dies, dies after the delivery.
    ///
    /// # Errors
    /// A refusal, with nothing changed. A failure after the commit is in [`Delivery::output`].
    pub fn deliver<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        mother: u64,
        today: GameTime,
    ) -> Result<Delivery, EventError> {
        let m = self
            .live
            .get(&mother)
            .cloned()
            .ok_or(Error::MissingCharacter)?;
        let father_id = m.life.pregnant_by.ok_or(Error::Lifecycle)?;
        let father = match self.live.get(&father_id) {
            Some(live) => live.clone(),
            None => world
                .archive
                .get(father_id)
                .map_err(|_| Error::Archive)?
                .ok_or(Error::MissingCharacter)?,
        };
        let count = Self::child_count(world, &m, today)?;
        // Room for the heads the delivery adds: the pop's heads after them still cover its live characters.
        let heads = world
            .populations
            .heads(&m.pop)
            .ok_or(Error::MissingPop)?
            .checked_add(count)
            .ok_or(Error::Overflow)?;
        let held = u64::try_from(self.pop_count(&m.pop)).map_err(|_| Error::Overflow)?;
        if held.checked_add(count).is_none_or(|needed| needed > heads) {
            return Err(Error::PopFull.into());
        }
        let pop_genetics: PopGenetics = world.populations.genetics(&m.pop).unwrap_or_default();
        let ctx = Context {
            rules: world.genetics,
            personality: world.personality,
            events: world.rules,
            lifecycle: world.lifecycle,
            seed: world.seed,
            tick: today,
        };
        let mut records = Vec::new();
        let mut next = self.next_id;
        for _ in 0..count {
            let id = next;
            next = id.checked_add(1).ok_or(Error::Overflow)?;
            let sex = if below(&mut character_stream(world.seed, id, "sex", today), 2)? == 0 {
                Sex::Male
            } else {
                Sex::Female
            };
            records.push(crate::child_record(
                &ctx,
                NewCharacter {
                    name: String::new(),
                    sex,
                    birth: today,
                    pop: m.pop.clone(),
                },
                id,
                [Some(&father), Some(&m)],
                &pop_genetics,
                world.populations,
            )?);
        }
        let nursing = first_after(today, world.lifecycle.conception.nursing_months)?;
        // Everything is decided: the heads are the one step that can still refuse, and nothing has changed yet.
        world.populations.add_births(&m.pop, count)?;
        let children: Vec<u64> = records.iter().map(|child| child.id).collect();
        for child in records {
            self.live.insert(child.id, child);
        }
        self.next_id = next;
        for parent in [father_id, mother] {
            if let Some(c) = self.live.get_mut(&parent) {
                c.children.extend(children.iter().copied());
            }
        }
        if let Some(c) = self.live.get_mut(&mother) {
            c.life.pregnant_by = None;
            c.life.due = None;
            c.life.nursing_until = Some(nursing);
        }
        let mut first: FirstError = None;
        for _ in &children {
            self.emit(world, &[mother, father_id], "child_born", today, &mut first);
        }
        self.childbirth_risk(world, &m, today, &mut first);
        Ok(Delivery {
            children,
            output: first,
        })
    }

    /// The number of children of a delivery: one roll `u` on purpose `delivery` gives triplets when `u < t`, twins when `u < t + w` and
    /// one child otherwise, with `t = triplets x b` and `w = twins x b` (`b = twin_bearer` when the mother shows it, else 1) and
    /// `t + w` capped at 1.
    fn child_count<P: Populations, A: ArchiveSink, L: LogSink>(
        world: &World<'_, P, A, L>,
        mother: &Character,
        today: GameTime,
    ) -> Result<u64, Error> {
        let rules = &world.lifecycle.conception;
        let draw = below(
            &mut character_stream(world.seed, mother.id, "delivery", today),
            u64::from(PPB),
        )?;
        let bearer = if mother.grade("twin_bearer") > 0 {
            u128::from(rules.twin_bearer_milli)
        } else {
            1000
        };
        let scaled = |chance: u32| (u128::from(chance) * bearer / 1000).min(u128::from(PPB));
        let triplets = scaled(rules.triplets_ppb);
        let twins = scaled(rules.twins_ppb).min(u128::from(PPB) - triplets);
        Ok(if u128::from(draw) < triplets {
            3
        } else if u128::from(draw) < triplets + twins {
            2
        } else {
            1
        })
    }

    /// The mother's risk at a delivery, rolled once, on purpose `childbirth`: `childbirth` x1.5 under 16 or over 40.
    fn childbirth_risk<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        mother: &Character,
        today: GameTime,
        first: &mut FirstError,
    ) {
        if !self.live.contains_key(&mother.id) {
            return;
        }
        let risk = || -> Result<bool, Error> {
            let age = Self::age_on(mother, today)?;
            let base = u64::from(world.lifecycle.death.childbirth_ppb);
            let chance = if (16..=40).contains(&age) {
                base
            } else {
                base * u64::from(EXTREME_AGE_RISK.0) / u64::from(EXTREME_AGE_RISK.1)
            };
            roll(
                &mut character_stream(world.seed, mother.id, "childbirth", today),
                u32::try_from(chance.min(u64::from(PPB))).map_err(|_| Error::Lifecycle)?,
            )
        };
        match risk() {
            Ok(true) => {
                if let Err(error) = self.die_of_cause(
                    world,
                    mother.id,
                    today,
                    CAUSE_CHILDBIRTH,
                    DIED_IN_CHILDBIRTH,
                    first,
                ) {
                    keep(first, error);
                }
            }
            Ok(false) => {}
            Err(error) => keep(first, error.into()),
        }
    }

    /// The conceptions, on the 1st after the economy's step: each woman who is not pregnant and past her nursing deadline, with her live
    /// husbands in identifier order, the first success winning.
    ///
    /// # Errors
    /// The first failure, once every woman has been looked at.
    pub fn run_conceptions<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        today: GameTime,
    ) -> Result<(), EventError> {
        let women: Vec<u64> = self
            .live
            .values()
            .filter(|c| c.sex == Sex::Female)
            .map(|c| c.id)
            .collect();
        let mut first: FirstError = None;
        for id in women {
            if let Err(error) = self.conceive(world, id, today) {
                keep(&mut first, error.into());
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// The relation mode of a couple: 0 trying, 1 neutral, 2 avoidance (the index of [`ck_economy::lifecycle::MODES`]).
    fn mode<P: Populations, A: ArchiveSink, L: LogSink>(
        world: &World<'_, P, A, L>,
        woman: &Character,
        husband: &Character,
        age: u32,
    ) -> Result<usize, Error> {
        let relations = &world.lifecycle.relations;
        // Trying until the couple has the children its standing wants (live or dead, together), then neutral.
        let kind = world.populations.kind(&husband.pop).ok_or(Error::Pop)?;
        let wanted = relations.wanted_children.get(&kind).copied().unwrap_or(0);
        let together = woman
            .children
            .iter()
            .filter(|child| husband.children.contains(child))
            .count();
        let mut mode = usize::from(u32::try_from(together).unwrap_or(u32::MAX) >= wanted);
        // Avoidance when the wife is of the age or her household's necessities are low.
        let poor = world
            .populations
            .necessities_pm(&woman.pop)
            .is_some_and(|necessities| u32::from(necessities) < relations.avoid_necessities);
        if age >= relations.avoid_age || poor {
            mode = 2;
        }
        // Her lust or chastity moves it a step: chastity toward avoidance, lust toward trying.
        let lust = woman.personality.vice[LUST_CHASTITY];
        if lust >= relations.shift_at {
            mode = (mode + 1).min(2);
        } else if lust <= -relations.shift_at {
            mode = mode.saturating_sub(1);
        }
        Ok(mode)
    }

    /// The monthly chance in ppb that a couple conceives (§3): `min(1, monthly x women(age) x men(age) x (her fecundity / 10) x (his
    /// fecundity / 10) x mode factor x church abstinence)`, exact, rounded down once.
    ///
    /// # Errors
    /// Refuses a pop the rules cannot resolve and a date outside the calendar.
    pub fn conception_ppb<P: Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        woman: &Character,
        husband: &Character,
        today: GameTime,
    ) -> Result<u32, Error> {
        let rules = &world.lifecycle.conception;
        let (age_f, age_m) = (Self::age_on(woman, today)?, Self::age_on(husband, today)?);
        let fecundity = |c: &Character| {
            let shown = c.genetics.shown(world.genetics, c.sex, stat("fecundity"));
            (u128::try_from(shown.max(0)).unwrap_or(0), 10_000)
        };
        let mode = Self::mode(world, woman, husband, age_f)?;
        let mut ratios = vec![
            curve_ratio(&rules.women, age_f),
            curve_ratio(&rules.men, age_m),
            fecundity(woman),
            fecundity(husband),
            (
                u128::from(world.lifecycle.relations.mode_factor[mode]),
                u128::from(PPB),
            ),
        ];
        if world.populations.religion(&woman.pop).as_deref() == Some(LATIN) {
            ratios.push((u128::from(rules.church_abstinence_ppb), u128::from(PPB)));
        }
        Ok(exact_ppb(rules.monthly_ppb, &ratios))
    }

    fn conceive<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &World<'_, P, A, L>,
        id: u64,
        today: GameTime,
    ) -> Result<(), Error> {
        let woman = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        if woman.life.pregnant_by.is_some() {
            return Ok(());
        }
        if let Some(until) = woman.life.nursing_until
            && !on_or_before(until, today)?
        {
            return Ok(());
        }
        let mut husbands: Vec<&Character> = woman
            .spouses
            .iter()
            .filter_map(|spouse| self.live.get(spouse))
            .filter(|spouse| spouse.sex == Sex::Male)
            .collect();
        husbands.sort_by_key(|husband| husband.id);
        let mut conceived = None;
        for husband in husbands {
            let chance = self.conception_ppb(world, woman, husband, today)?;
            let hit = roll(
                &mut character_stream(world.seed, id, &format!("conception.{}", husband.id), today),
                chance,
            )?;
            if hit {
                conceived = Some(husband.id);
                break;
            }
        }
        if let Some(father) = conceived {
            let due = first_after(today, 9)?;
            if let Some(c) = self.live.get_mut(&id) {
                c.life.pregnant_by = Some(father);
                c.life.due = Some(due);
            }
        }
        Ok(())
    }
}
