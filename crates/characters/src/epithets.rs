//! Epithets (CK-365, characters slice 5b): gained by chance when their conditions hold, kept for life, the most important shown.
//!
//! A `yearly` epithet is tested in the birthday pass and a `happening:<name>` epithet when that happening fires, in file order, for a
//! character that does not hold it yet and whose scope matches. The conditions are slice 4's plus the epithet-only keys; the chance is
//! exact, in parts per billion, rolled on the character's stream of purpose `epithet.<key>`. A gained epithet stores the start of the
//! day it was gained. The shown epithet is derived from the stored state alone, never stored: the held epithet with the greatest key
//! `(importance, earlier acquisition, earlier in the file)`.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, Character, CharacterStore, Error, Populations, character_stream,
    events::{EventError, LogSink, PopFacts, World, age_years, holds, pop_facts},
    genetics::roll,
};
use ck_economy::{
    Epithet, EpithetScope, EpithetTrigger,
    events::Conditions,
    genetics::{Genetics, attribute_index},
};
use ck_sim::{Date, GameTime};
use std::cmp::Reverse;

/// The start of the day of `time`.
fn day_start(time: GameTime) -> Result<GameTime, Error> {
    let d = time.date().map_err(|_| Error::Event)?;
    GameTime::from_date(Date { hour: 0, ..d }).map_err(|_| Error::Event)
}

impl Character {
    /// The epithets held: `(key, the start of the day it was gained)`, in key order.
    pub fn epithets(&self) -> impl Iterator<Item = (&str, GameTime)> {
        self.life
            .epithets
            .iter()
            .map(|(key, at)| (key.as_str(), *at))
    }

    /// The shown epithet among those held, `rules` being the epithets in file order: the greatest `(importance, −acquisition day,
    /// −file position)`. A higher importance wins whatever the order they were gained in; at equal importance the one gained
    /// earlier stays; at equal importance gained the same day the one listed first in the file wins. A held key the rules do not
    /// know is ignored.
    #[must_use]
    pub fn shown_epithet<'a>(&self, rules: &'a [Epithet]) -> Option<&'a Epithet> {
        rules
            .iter()
            .enumerate()
            .filter_map(|(position, epithet)| {
                let gained = *self.life.epithets.get(&epithet.key)?;
                Some((
                    (epithet.importance, Reverse(gained), Reverse(position)),
                    epithet,
                ))
            })
            .max_by_key(|(key, _)| *key)
            .map(|(_, epithet)| epithet)
    }
}

/// Whether the extra conditions an epithet adds to slice 4's vocabulary hold: `grade_min`, `stat_max_by_sex`, `weight_ratio_min`,
/// `count_min` and `adult` (`minor` is whether the character is under majority).
fn extras(conditions: &Conditions, c: &Character, genetics: &Genetics, minor: bool) -> bool {
    let weight = attribute_index("weight").unwrap_or(0);
    conditions
        .grade_min
        .iter()
        .all(|(key, level)| c.grade(key) >= *level)
        && conditions.stat_max_by_sex.iter().all(|(stat, m, f)| {
            let limit = match c.sex {
                crate::Sex::Male => *m,
                crate::Sex::Female => *f,
            };
            c.genetics.shown(genetics, c.sex, *stat) <= limit
        })
        && conditions.weight_ratio_min.is_none_or(|ratio| {
            // The shown weight over the rolled weight is at least `ratio / 1000`, in integers: shown x 1000 >= ratio x rolled.
            let rolled = c.genetics.value[weight];
            rolled > 0
                && i128::from(c.genetics.shown(genetics, c.sex, weight)) * 1000
                    >= i128::from(ratio) * i128::from(rolled)
        })
        && conditions
            .count_min
            .iter()
            .all(|(counter, least)| c.life.counters.get(counter).copied().unwrap_or(0) >= *least)
        && (!conditions.adult || !minor)
}

impl CharacterStore {
    /// Whether the character is of the scope: everyone, the subgroup of its culture, or the religion of its pop.
    fn in_scope<P: Populations, A: ArchiveSink, L: LogSink>(
        world: &World<'_, P, A, L>,
        c: &Character,
        pop: &PopFacts,
        scope: &EpithetScope,
    ) -> bool {
        match scope {
            EpithetScope::Universal => true,
            EpithetScope::Subgroup(group) => pop.group == *group,
            EpithetScope::Religion(religion) => {
                world.populations.religion(&c.pop).as_deref() == Some(religion.as_str())
            }
        }
    }

    /// Test every epithet of `trigger` for a live character, in file order, and store each that is gained (§1): one the character
    /// does not hold, in its scope, whose conditions hold and whose chance (purpose `epithet.<key>`) is rolled. A character that is
    /// not live is skipped. The date stored is the start of `today`.
    ///
    /// # Errors
    /// A date outside the calendar, a pop the engine cannot resolve and arithmetic that does not fit; what was gained before the
    /// failure stays.
    pub(crate) fn gain_epithets<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &World<'_, P, A, L>,
        id: u64,
        today: GameTime,
        trigger: &EpithetTrigger,
    ) -> Result<(), EventError> {
        let rules = world.lifecycle;
        for epithet in &rules.epithets {
            if epithet.trigger != *trigger {
                continue;
            }
            let Some(c) = self.live.get(&id) else {
                return Ok(());
            };
            if c.life.epithets.contains_key(&epithet.key) {
                continue;
            }
            let pop = pop_facts(world.rules, world.populations, &c.pop)?;
            if !Self::in_scope(world, c, &pop, &epithet.scope) {
                continue;
            }
            let age = age_years(
                c.birth.date().map_err(|_| Error::Event)?,
                today.date().map_err(|_| Error::Event)?,
            );
            if !holds(&epithet.when, c, &pop, age)
                || !extras(
                    &epithet.when,
                    c,
                    world.genetics,
                    Self::is_minor(world, c, today)?,
                )
            {
                continue;
            }
            let mut stream =
                character_stream(world.seed, id, &format!("epithet.{}", epithet.key), today);
            if !roll(&mut stream, epithet.chance_ppb)? {
                continue;
            }
            let gained = day_start(today)?;
            if let Some(c) = self.live.get_mut(&id) {
                c.life.epithets.insert(epithet.key.clone(), gained);
            }
        }
        Ok(())
    }
}
