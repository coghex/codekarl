//! The generated past (CK-369 §3): a character made at its birth lives year by year to today, on its own, with the in-play rules and these
//! differences. Upbringing is toward the household of its parents (two typical members of its culture when a parent is not known); the skills
//! grow by use from `skills_from`; the yearly check runs from `yearly_check_from`; a world is concocted around it each year at the data's
//! chances (the happenings, battles, raids, duels and the yearly strain); **no deaths**: any event whose effects include `die` is skipped
//! entirely, with no roll, no effects and no line, and monthly mortality is not run. Only the character's own stages run: not the household,
//! marriage, connection or happening-to-others hooks.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, CharacterStore, Error, Populations, Sex, StressKind, character_stream,
    culture_means,
    events::{
        EventError, FirstError, LogSink, Step, World, age_years, count_up, is_fatal, pop_facts,
    },
    generator::dates,
    genetics::{normal_draw, roll},
    personality::{self, drift_and_fade, recovered_stress, stress_after},
    skills::grown,
};
use ck_economy::{
    EpithetTrigger,
    events::{BATTLE_AGES, HAPPENINGS, Trigger},
    genetics::PPB,
};
use ck_sim::GameTime;

/// Fame a duel won gives, in thousandths of a point.
const DUEL_FAME: i64 = 2_000;

impl CharacterStore {
    /// Live the years of `id`, a live character made at its birth, from its first birthday to the last one before `today` (a birthday that
    /// falls on `today` is the in-play pass's). `parents` are the minds (vice/virtue and manner values) of its father and mother when known;
    /// a parent that is not known is a typical member of its culture, drawn on the character's stream.
    pub(crate) fn live_the_past<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        today: GameTime,
        parents: &[Option<([i64; 7], [i64; 9])>],
    ) -> Result<(), EventError> {
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        let birth = c.birth;
        let years = age_years(dates::date_of(birth)?, dates::date_of(today)?);
        let mut household = Vec::with_capacity(parents.len());
        for (index, parent) in parents.iter().enumerate() {
            household.push(if let Some(minds) = parent {
                *minds
            } else {
                let ctx = crate::Context {
                    rules: world.genetics,
                    personality: world.personality,
                    events: world.rules,
                    lifecycle: world.lifecycle,
                    seed: world.seed,
                    tick: birth,
                };
                let means = culture_means(&ctx, world.populations, &c.pop)?;
                let typical = personality::make_founder(
                    &ctx,
                    id,
                    &format!("past.household.{index}."),
                    means,
                )?;
                (typical.vice, typical.manner)
            });
        }
        let mut first: FirstError = None;
        for age in 1..=years {
            let day = dates::birthday(birth, age)?;
            if day >= today {
                break;
            }
            self.past_year(world, id, day, age, &household, &mut first)?;
        }
        crate::events::conclude(first)
    }

    /// One year of the generated past, on the birthday at which the character turns `age`.
    fn past_year<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        day: GameTime,
        age: u32,
        household: &[([i64; 7], [i64; 9])],
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let rules = world.rules;
        let past = &world.lifecycle.generator.past;
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        // Stages 1 and 2: humour drift, then the standing fade.
        let moved = drift_and_fade(world.personality, c)?;
        let crafts: Vec<String> = rules
            .craft_rise
            .skills
            .keys()
            .filter(|craft| c.grade(craft) > 0)
            .cloned()
            .collect();
        if let Some(c) = self.live.get_mut(&id) {
            c.personality = moved;
        }
        if age >= past.yearly_check_from {
            self.past_strain(world, id, day)?;
            self.past_world(world, id, day, age, first)?;
            // The yearly events, in data order, but none that kills.
            for event in &rules.events {
                if event.trigger != Trigger::Yearly || is_fatal(event) {
                    continue;
                }
                if self.run_event(world, event, id, day, first)? == Step::Applied {
                    self.precedence(world, id, day, first)?;
                }
            }
        }
        // The craft rise, for the crafts held before this year's events.
        for craft in &crafts {
            self.rise(world, id, craft, day, first)?;
        }
        // Growth by use, from `skills_from`.
        if age >= past.skills_from {
            let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
            let pop = pop_facts(rules, world.populations, &c.pop)?;
            let mut levels = Vec::new();
            for skill in &rules.skills.list {
                levels.push((skill.clone(), grown(&rules.skills, c, skill, &pop)?));
            }
            if let Some(c) = self.live.get_mut(&id) {
                c.life.skills.extend(levels);
            }
        }
        self.upbringing(world, id, day, age, household, past.upbringing_to)?;
        self.height_sample(world, id, age)?;
        self.precedence(world, id, day, first)?;
        // The birthday of majority is the `coming_of_age` happening, as in play.
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        let pop = pop_facts(rules, world.populations, &c.pop)?;
        let ages = world.lifecycle.majority.of_group(&pop.group);
        let majority = match c.sex {
            Sex::Male => ages.m,
            Sex::Female => ages.f,
        };
        if majority > 0 && age == majority {
            self.dispatch_with(world, id, "coming_of_age", day, first, true)?;
        }
        self.gain_epithets(world, id, day, &EpithetTrigger::Yearly)?;
        Ok(())
    }

    /// The stress of a year: twelve months of recovery, then the year's strain (a normal draw, never negative) taken in as any stress is.
    fn past_strain<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &World<'_, P, A, L>,
        id: u64,
        day: GameTime,
    ) -> Result<(), Error> {
        let past = &world.lifecycle.generator.past;
        let mut c = self.live.get(&id).ok_or(Error::MissingCharacter)?.clone();
        for _ in 0..12 {
            c.personality.stress = recovered_stress(world.personality, &c)?;
        }
        let strain = normal_draw(
            &mut character_stream(world.seed, id, "past.strain", day),
            past.strain_mean,
            past.strain_sd,
        )?
        .max(0);
        let stress = stress_after(world.personality, &c, strain, StressKind::Other)?;
        if let Some(live) = self.live.get_mut(&id) {
            live.personality.stress = stress;
        }
        Ok(())
    }

    /// The world concocted around a character of a year: the happenings at the data's chances, a new friend (a comfort like a feast), the
    /// battles of a man of fighting age with their command and their defeats, the raids a noble man leads and the duels a skilled man wins.
    fn past_world<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        day: GameTime,
        age: u32,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let past = &world.lifecycle.generator.past;
        let rules = world.rules;
        let seed = world.seed;
        let chance = |purpose: &str, ppb: u32| -> Result<bool, Error> {
            roll(
                &mut character_stream(seed, id, &format!("past.{purpose}"), day),
                ppb,
            )
        };
        for name in HAPPENINGS {
            if chance(&format!("happening.{name}"), past.happenings[name])? {
                self.dispatch_with(world, id, name, day, first, true)?;
            }
        }
        if chance("new_friend", past.new_friend_ppb)? {
            self.dispatch_with(world, id, "feast", day, first, true)?;
        }
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        let kind = world.populations.kind(&c.pop).ok_or(Error::Pop)?;
        let (male, martial) = (c.sex == Sex::Male, c.life.focus == "martial");
        if male && (BATTLE_AGES[0]..=BATTLE_AGES[1]).contains(&age) {
            let base = past.battles.get(&format!("{kind}_m")).copied().unwrap_or(0);
            let ppb = if martial {
                base.saturating_mul(2).min(PPB)
            } else {
                base
            };
            if chance("battle", ppb)? {
                self.dispatch_with(world, id, "battle_survived", day, first, true)?;
                let leadership = self
                    .live
                    .get(&id)
                    .ok_or(Error::MissingCharacter)?
                    .skill("leadership");
                if leadership >= past.command_leadership_min
                    && chance("battle_command", past.command_chance_ppb)?
                {
                    self.dispatch_with(world, id, "battle_won_in_command", day, first, true)?;
                }
                if chance("battle_defeat", past.battle_defeat_ppb)? {
                    self.dispatch_with(world, id, "defeat", day, first, true)?;
                }
            }
        }
        if male && kind == "noble" && (past.raids_ages[0]..=past.raids_ages[1]).contains(&age) {
            let ppb = if martial {
                past.raids_martial_focus_ppb
            } else {
                past.raids_noble_m_ppb
            };
            if chance("raid", ppb)?
                && let Some(c) = self.live.get_mut(&id)
            {
                count_up(c, rules, "raids_led", 1)?;
            }
        }
        let prowess = self
            .live
            .get(&id)
            .ok_or(Error::MissingCharacter)?
            .skill("martial_prowess");
        if prowess >= past.duel_prowess_min
            && chance("duel", past.duel_chance_ppb)?
            && let Some(c) = self.live.get_mut(&id)
        {
            count_up(c, rules, "duels_won", 1)?;
            c.personality.fame = c
                .personality
                .fame
                .checked_add(DUEL_FAME)
                .ok_or(Error::StandingRange)?;
        }
        Ok(())
    }
}
