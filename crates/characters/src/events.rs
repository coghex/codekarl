//! The event engine: happenings, the yearly check, effects, precedence and the life events that emit happenings (CK-363,
//! characters slice 4).
//!
//! No floating point. A chance is `min(1, base x product of factors)` evaluated exactly as one rational, converted once to
//! parts per billion rounded down, and rolled on the stream of purpose `event.<id>` at the tick of the event. An event's effects
//! are computed on a copy of the character and committed together, so an error in any effect leaves the character unchanged.
//!
//! The life log is output, not simulation state: a line that cannot be appended after a committed change does not undo the change.
//! The error is returned to the caller after the work that remains has been done; there is no retry and no rollback.
#![deny(clippy::float_arithmetic)]

use crate::bigint::Big;
use crate::{
    ArchiveSink, Character, CharacterStore, Error, LifeLog, Lineage, NewCharacter, PopLocator,
    Populations, StressKind, character_stream,
    genetics::{Context, roll},
    personality::{stress_after, threshold, wide},
};
use ck_economy::{
    EventRules, Personality, PopGenetics,
    events::{
        CRAFT_FACTOR_DIVISOR, Conditions, Effect, Event, Factor, GRADE_NAMES, HUMOUR_FLOOR,
        HUMOUR_WEIGHT, MAX_GRADE, PAIR_FACTOR_BOUNDS, STRESS_FACTOR_DIVISOR, SexRule, StandingKind,
        StressFactor, Trigger,
    },
    genetics::PPB,
};
use ck_sim::{Date, GameTime, is_leap_year};
use std::io;

/// Index of the first humour (blood) in a genetic attribute array; the four humours follow in the data's order.
const HUMOURS_AT: usize = 9;
/// The line precedence writes for an attribute it removes (drafted in the glossary style; the owner checks the wording).
pub const LOST_TEXT: &str = "{name} lost {trait}.";
/// The line the yearly craft rise writes (drafted in the glossary style; the owner checks the wording).
pub const CRAFT_RISE_TEXT: &str = "{name} grew in skill as {trait} ({grade}).";

/// An error of the event engine: a refusal with nothing changed, or an output failure after a committed change.
#[derive(Debug)]
pub enum EventError {
    /// A refusal of the store or of the arithmetic; nothing was changed by the failing step.
    Rules(Error),
    /// The archive could not append a dying character's record: the death did not commit.
    Archive(io::Error),
    /// A life-log line could not be appended: the change it records stands.
    Log(io::Error),
}

impl From<Error> for EventError {
    fn from(error: Error) -> Self {
        Self::Rules(error)
    }
}

impl std::fmt::Display for EventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rules(error) => write!(f, "{error}"),
            Self::Archive(error) => write!(f, "archive: {error}"),
            Self::Log(error) => write!(f, "life log: {error}"),
        }
    }
}

impl std::error::Error for EventError {}

/// A destination for life-log lines. Failure must leave the log unchanged.
pub trait LogSink {
    /// Append one line for a live or archived character.
    /// # Errors
    /// Returns an I/O error if the line could not be written.
    fn append(&mut self, id: u64, date: GameTime, text: &str) -> io::Result<()>;
}

impl LogSink for LifeLog {
    fn append(&mut self, id: u64, date: GameTime, text: &str) -> io::Result<()> {
        Self::append(self, id, date, text)
    }
}

/// Everything the engine reads and writes besides the store: the rules, the seed, the pops, the archive and the logs.
pub struct World<'a, P: Populations, A: ArchiveSink, L: LogSink> {
    /// The loaded event data.
    pub rules: &'a EventRules,
    /// The loaded personality data (stress intake).
    pub personality: &'a Personality,
    /// The world seed.
    pub seed: u64,
    /// The pops, resolved for pop type, culture and luxuries.
    pub populations: &'a P,
    /// The archive a death is appended to.
    pub archive: &'a mut A,
    /// The life logs.
    pub logs: &'a mut L,
}

/// What an event reads of the pop a character belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PopFacts {
    /// The pop type key.
    pub kind: String,
    /// The culture's subgroup.
    pub group: String,
    /// The pop's luxuries fulfilment in permille.
    pub luxuries_pm: i64,
}

pub(crate) fn pop_facts<P: Populations>(
    rules: &EventRules,
    populations: &P,
    pop: &PopLocator,
) -> Result<PopFacts, Error> {
    let culture = populations.culture(pop).ok_or(Error::Pop)?;
    Ok(PopFacts {
        kind: populations.kind(pop).ok_or(Error::Pop)?,
        group: rules
            .culture_group
            .get(&culture)
            .ok_or(Error::Culture)?
            .clone(),
        luxuries_pm: i64::from(populations.luxuries_pm(pop).ok_or(Error::Pop)?),
    })
}

/// A character's age in whole years on `today`.
pub(crate) fn age_years(birth: Date, today: Date) -> u32 {
    let turned = (today.month, today.day) >= (birth.month, birth.day);
    // A 29 February birthday falls on 1 March in a common year.
    let turned = turned
        && !((birth.month, birth.day) == (2, 29) && today.month == 2 && !is_leap_year(today.year));
    u32::from(today.year.saturating_sub(birth.year)).saturating_sub(u32::from(!turned))
}

fn at_least(have: i64, want: i64) -> bool {
    have >= want
}

/// Whether a character meets every set condition.
pub(crate) fn holds(conditions: &Conditions, c: &Character, pop: &PopFacts, age: u32) -> bool {
    let p = &c.personality;
    let over = wide(p.stress) - wide(threshold(c));
    conditions
        .age
        .is_none_or(|[low, high]| (low..=high).contains(&age))
        && conditions.sex.is_none_or(|sex| match sex {
            SexRule::Male => c.sex == crate::Sex::Male,
            SexRule::Female => c.sex == crate::Sex::Female,
        })
        && (conditions.pop_type.is_empty() || conditions.pop_type.contains(&pop.kind))
        && (conditions.culture_group.is_empty() || conditions.culture_group.contains(&pop.group))
        && conditions.has.iter().all(|key| c.grade(key) > 0)
        && conditions.lacks.iter().all(|key| c.grade(key) == 0)
        && conditions
            .skill_min
            .iter()
            .all(|(skill, min)| at_least(c.skill(skill), *min))
        && conditions
            .virtue_min
            .iter()
            .all(|(pair, min)| at_least(p.vice[*pair], *min))
        && conditions
            .virtue_max
            .iter()
            .all(|(pair, max)| p.vice[*pair] <= *max)
        && conditions
            .manner_min
            .iter()
            .all(|(pair, min)| at_least(p.manner[*pair], *min))
        && conditions
            .manner_max
            .iter()
            .all(|(pair, max)| p.manner[*pair] <= *max)
        && conditions
            .over_threshold_min
            .is_none_or(|min| over >= wide(min))
        && (!conditions.over_threshold || over > 0)
        && (!conditions.lacks_land || !c.life.holds_land)
        && conditions
            .pop_luxuries_min
            .is_none_or(|min| pop.luxuries_pm >= min)
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn unsigned(value: i128) -> Result<u128, Error> {
    u128::try_from(value).map_err(|_| Error::Event)
}

/// A factor as an exact non-negative rational `(numerator, denominator)`.
fn factor_ratio(factor: &Factor, c: &Character, pop: &PopFacts) -> Result<(u128, u128), Error> {
    let milli =
        |value: i64| -> Result<(u128, u128), Error> { Ok((unsigned(i128::from(value))?, 1000)) };
    Ok(match factor {
        Factor::Humour(index) => {
            let humour = i128::from(c.genetics.value[HUMOURS_AT + index]);
            let weighted = 1_000_000 + i128::from(HUMOUR_WEIGHT) * (humour - 5000);
            (
                unsigned(weighted.max(i128::from(HUMOUR_FLOOR) * 1000))?,
                1_000_000,
            )
        }
        Factor::Stress(direction) => {
            let excess = (wide(c.personality.stress) - wide(threshold(c))).max(0);
            let scale = i128::from(STRESS_FACTOR_DIVISOR) * 1000;
            let (base, over) = (unsigned(scale)?, unsigned(scale + excess)?);
            match direction {
                StressFactor::Cause => (over, base),
                StressFactor::Lift => (base, over),
            }
        }
        Factor::Virtue { pair, per } | Factor::Manner { pair, per } => {
            let value = if matches!(factor, Factor::Virtue { .. }) {
                c.personality.vice[*pair]
            } else {
                c.personality.manner[*pair]
            };
            let raw = 1_000_000 + i128::from(*per) * i128::from(value);
            let bounds = PAIR_FACTOR_BOUNDS.map(|bound| i128::from(bound) * 1000);
            (unsigned(raw.clamp(bounds[0], bounds[1]))?, 1_000_000)
        }
        Factor::Craft(skill) => {
            let scale = i128::from(CRAFT_FACTOR_DIVISOR) * 1000;
            (
                unsigned(scale / 10 + i128::from(c.skill(skill)))?,
                unsigned(scale)?,
            )
        }
        Factor::PopType(table) => milli(table.get(&pop.kind).copied().unwrap_or(0))?,
        Factor::CultureGroup(table) => milli(table.get(&pop.group).copied().unwrap_or(1000))?,
        Factor::Grade { key, by } => milli(by[usize::from(c.grade(key).min(MAX_GRADE))])?,
    })
}

/// `floor(min(1, numerator / denominator) x 1e9)` for the factors' exact product, without any intermediate overflow.
fn exact_ppb(base: u32, ratios: &[(u128, u128)]) -> u32 {
    let mut numerator = Big::new(u128::from(base));
    let mut denominator = Big::new(u128::from(PPB));
    for (n, d) in ratios {
        numerator = numerator.mul(&Big::new(*n));
        denominator = denominator.mul(&Big::new(*d));
    }
    if numerator.cmp(&denominator) != std::cmp::Ordering::Less {
        return PPB;
    }
    // The largest p with p x denominator <= numerator x 1e9, by bisection over 0..1e9.
    let scaled = numerator.mul(&Big::new(u128::from(PPB)));
    let (mut low, mut high) = (0_u32, PPB);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if denominator.mul(&Big::new(u128::from(mid))).cmp(&scaled) == std::cmp::Ordering::Greater {
            high = mid - 1;
        } else {
            low = mid;
        }
    }
    low
}

/// The chance of `event` for `c` in parts per billion: `min(1, base x factors)` as one exact rational, rounded down. The product
/// is exact over the whole domain the loader accepts: it is evaluated in 128 bits when it fits and in unbounded integers when it
/// does not, so a later factor can still cap or reduce a large intermediate.
///
/// # Errors
/// Refuses a factor that does not fit 128 bits on its own.
pub fn chance_ppb(event: &Event, c: &Character, pop: &PopFacts) -> Result<u32, Error> {
    let mut ratios = Vec::with_capacity(event.factors.len());
    for factor in &event.factors {
        ratios.push(factor_ratio(factor, c, pop)?);
    }
    let (mut numerator, mut denominator) = (u128::from(event.chance_ppb), u128::from(PPB));
    let mut fits = true;
    for (n, d) in &ratios {
        if let (Some(n), Some(d)) = (numerator.checked_mul(*n), denominator.checked_mul(*d)) {
            let common = gcd(n, d).max(1);
            (numerator, denominator) = (n / common, d / common);
        } else {
            fits = false;
            break;
        }
    }
    if !fits {
        return Ok(exact_ppb(event.chance_ppb, &ratios));
    }
    if numerator >= denominator {
        return Ok(PPB);
    }
    match numerator.checked_mul(u128::from(PPB)) {
        Some(scaled) => u32::try_from(scaled / denominator).map_err(|_| Error::Event),
        None => Ok(exact_ppb(event.chance_ppb, &ratios)),
    }
}

/// Roll `p` (ppb) on the stream of purpose `event.<id>` for character `id` at `tick`.
fn rolls(seed: u64, id: u64, event: &str, tick: GameTime, p: u32) -> Result<bool, Error> {
    roll(
        &mut character_stream(seed, id, &format!("event.{event}"), tick),
        p,
    )
}

/// The result of applying an event's effects to a copy.
struct Outcome {
    copy: Character,
    trait_name: String,
    grade: &'static str,
    count: u64,
    /// Some trait or attribute effect changed something.
    changed: bool,
    /// Whether the event always writes its line on success (no effects, or any stress, standing, count or die).
    always_logs: bool,
    died: Option<String>,
}

fn stress_kind(trigger: &Trigger) -> StressKind {
    match trigger {
        Trigger::Happening(name) if name == "insulted" => StressKind::Insult,
        Trigger::Happening(name) if name == "defeat" => StressKind::Defeat,
        _ => StressKind::Other,
    }
}

fn grade_name(level: u8) -> &'static str {
    GRADE_NAMES[usize::from(level.min(MAX_GRADE))]
}

fn apply_effects(
    event: &Event,
    c: &Character,
    rules: &EventRules,
    personality: &Personality,
) -> Result<Outcome, Error> {
    let mut out = Outcome {
        copy: c.clone(),
        trait_name: String::new(),
        grade: "none",
        count: 0,
        changed: false,
        always_logs: event.effects.is_empty(),
        died: None,
    };
    for effect in &event.effects {
        match effect {
            Effect::RaiseGrade(key)
            | Effect::LowerGrade(key)
            | Effect::Grant(key)
            | Effect::Remove(key) => {
                let before = out.copy.grade(key);
                let after = match effect {
                    Effect::RaiseGrade(_) => before.saturating_add(1).min(MAX_GRADE),
                    Effect::LowerGrade(_) => before.saturating_sub(1),
                    Effect::Grant(_) => before.max(1),
                    _ => 0,
                };
                if after != before {
                    out.copy.set_grade(key, after);
                    out.changed = true;
                }
                out.trait_name.clone_from(key);
                out.grade = grade_name(after);
            }
            Effect::Count { counter, add } => {
                let total = out
                    .copy
                    .life
                    .counters
                    .get(counter)
                    .copied()
                    .unwrap_or(0)
                    .checked_add(*add)
                    .ok_or(Error::Event)?;
                out.copy.life.counters.insert(counter.clone(), total);
                for deed in rules.deeds.iter().filter(|deed| &deed.counter == counter) {
                    let reached = deed
                        .thresholds
                        .iter()
                        .filter(|need| **need <= total)
                        .count();
                    let level = u8::try_from(reached).map_err(|_| Error::Event)?;
                    if level > out.copy.grade(&deed.key) {
                        out.copy.set_grade(&deed.key, level);
                    }
                }
                out.count = total;
                out.always_logs = true;
            }
            Effect::Stress(amount) => {
                out.copy.personality.stress = if *amount >= 0 {
                    stress_after(personality, &out.copy, *amount, stress_kind(&event.trigger))?
                } else {
                    out.copy.personality.stress.saturating_add(*amount).max(0)
                };
                out.always_logs = true;
            }
            Effect::Standing { which, add } => {
                let slot = match which {
                    StandingKind::Popularity => &mut out.copy.personality.popularity,
                    StandingKind::Fame => &mut out.copy.personality.fame,
                    StandingKind::Honour => &mut out.copy.personality.honour,
                };
                *slot = slot.checked_add(*add).ok_or(Error::StandingRange)?;
                out.always_logs = true;
            }
            Effect::Die(cause) => {
                out.died = Some(cause.clone());
                out.always_logs = true;
                break;
            }
        }
    }
    Ok(out)
}

/// A trait key as it reads in a line: underscores become spaces.
pub(crate) fn plain(key: &str) -> String {
    key.replace('_', " ")
}

/// Fill the fields of a life-log line.
pub(crate) fn render(text: &str, name: &str, trait_name: &str, grade: &str, count: u64) -> String {
    text.replace("{name}", name)
        .replace("{trait}", &plain(trait_name))
        .replace("{grade}", grade)
        .replace("{count}", &count.to_string())
}

/// The first error of the output side, kept while the remaining work is done.
pub(crate) type FirstError = Option<EventError>;

pub(crate) fn keep(first: &mut FirstError, error: EventError) {
    first.get_or_insert(error);
}

fn conclude(first: FirstError) -> Result<(), EventError> {
    first.map_or(Ok(()), Err)
}

/// Whether an event's processing ended the character's life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The event did not hold, or its roll failed: nothing changed.
    Continue,
    /// The event committed its effects.
    Applied,
    Died,
}

impl CharacterStore {
    pub(crate) fn line<P: Populations, A: ArchiveSink, L: LogSink>(
        world: &mut World<'_, P, A, L>,
        id: u64,
        date: GameTime,
        text: &str,
        first: &mut FirstError,
    ) {
        if let Err(error) = world.logs.append(id, date, text) {
            keep(first, EventError::Log(error));
        }
    }

    /// Test one event for one live character and, if it holds and its roll succeeds, apply it.
    pub(crate) fn run_event<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        event: &Event,
        id: u64,
        date: GameTime,
        first: &mut FirstError,
    ) -> Result<Step, EventError> {
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
        let pop = pop_facts(world.rules, world.populations, &c.pop)?;
        let age = age_years(
            c.birth.date().map_err(|_| Error::Event)?,
            date.date().map_err(|_| Error::Event)?,
        );
        if !holds(&event.when, c, &pop, age) {
            return Ok(Step::Continue);
        }
        let p = chance_ppb(event, c, &pop)?;
        if !rolls(world.seed, id, &event.id, date, p)? {
            return Ok(Step::Continue);
        }
        let outcome = apply_effects(event, c, world.rules, world.personality)?;
        let line = (outcome.always_logs || outcome.changed).then(|| {
            render(
                &event.text,
                &c.name,
                &outcome.trait_name,
                outcome.grade,
                outcome.count,
            )
        });
        if let Some(cause) = outcome.died {
            // The death commits first: the archive append, then the removal of the live record. A failure changes nothing.
            let dead = outcome.copy;
            self.bury(dead.clone(), date, Some(&cause), world.archive)
                .map_err(EventError::Archive)?;
            if let Some(text) = line {
                Self::line(world, id, date, &text, first);
            }
            self.bereave(world, &dead, date, first);
            return Ok(Step::Died);
        }
        self.live.insert(id, outcome.copy);
        if let Some(text) = line {
            Self::line(world, id, date, &text, first);
        }
        Ok(Step::Applied)
    }

    /// Emit `death_in_family` to each live parent, spouse and child of `dead`, which has just died.
    pub(crate) fn bereave<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        dead: &Character,
        date: GameTime,
        first: &mut FirstError,
    ) {
        let mut family: Vec<u64> = Vec::new();
        for relative in [dead.father, dead.mother]
            .into_iter()
            .chain(dead.spouses.iter().copied())
            .chain(dead.children.iter().copied())
        {
            if relative != dead.id
                && self.live.contains_key(&relative)
                && !family.contains(&relative)
            {
                family.push(relative);
            }
        }
        self.emit(world, &family, "death_in_family", date, first);
    }

    /// Dispatch a happening to each of `recipients` that is still live; a failure is kept and the rest still run.
    pub(crate) fn emit<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        recipients: &[u64],
        happening: &str,
        date: GameTime,
        first: &mut FirstError,
    ) {
        for id in recipients {
            if self.live.contains_key(id)
                && let Err(error) = self.dispatch(world, *id, happening, date, first)
            {
                keep(first, error);
            }
        }
    }

    /// A happening for one character: (1) its events in data order, (2) the flag the happening sets, (3) precedence.
    pub(crate) fn dispatch<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        happening: &str,
        date: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let rules = world.rules;
        for event in &rules.events {
            if matches!(&event.trigger, Trigger::Happening(name) if name == happening)
                && self.run_event(world, event, id, date, first)? == Step::Died
            {
                return Ok(());
            }
        }
        if happening == "inherited_land"
            && let Some(c) = self.live.get_mut(&id)
        {
            c.life.holds_land = true;
        }
        self.precedence(world, id, date, first)
    }

    /// Report a happening for a character (§7): its events run in data order, then the flag the happening sets, then precedence.
    /// Used until the systems that cause a happening exist; the life events below report theirs themselves.
    ///
    /// # Errors
    /// Refuses a missing character, a pop the engine cannot resolve and arithmetic that does not fit (the events that ran before
    /// the failing one stay committed), and returns the first life-log or archive failure once the work is done.
    pub fn report_happening<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        happening: &str,
        date: GameTime,
    ) -> Result<(), EventError> {
        let mut first = None;
        self.dispatch(world, id, happening, date, &mut first)?;
        conclude(first)
    }

    /// Precedence (§4): every attribute with a `keep_while` entry whose conditions fail is removed that day with a line; the check
    /// repeats, in catalog order, until nothing more is removed, so dependants fall the same day.
    pub(crate) fn precedence<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        date: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let rules = world.rules;
        loop {
            let mut removed = false;
            for (key, conditions) in &rules.keep_while {
                let c = self.live.get(&id).ok_or(Error::MissingCharacter)?;
                if c.grade(key) == 0 {
                    continue;
                }
                let pop = pop_facts(rules, world.populations, &c.pop)?;
                let age = age_years(
                    c.birth.date().map_err(|_| Error::Event)?,
                    date.date().map_err(|_| Error::Event)?,
                );
                if holds(conditions, c, &pop, age) {
                    continue;
                }
                let text = render(LOST_TEXT, &c.name, key, "none", 0);
                if let Some(c) = self.live.get_mut(&id) {
                    c.set_grade(key, 0);
                }
                removed = true;
                Self::line(world, id, date, &text, first);
            }
            if !removed {
                return Ok(());
            }
        }
    }

    /// Run precedence for one character now (§4: after a committed change to it).
    ///
    /// # Errors
    /// As [`report_happening`](Self::report_happening).
    pub fn apply_precedence<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        date: GameTime,
    ) -> Result<(), EventError> {
        let mut first = None;
        self.precedence(world, id, date, &mut first)?;
        conclude(first)
    }

    /// Create a child (`create_child`) and, once it commits, emit `child_born` to each **live** parent.
    ///
    /// # Errors
    /// The refusals of `create_child` change nothing. After the child is created a later failure is returned but the child
    /// stands: it is the character with the highest identifier.
    pub fn create_child_with_events<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        ctx: &Context<'_>,
        new: NewCharacter,
        pop_genetics: &PopGenetics,
        lineage: &Lineage<'_>,
    ) -> Result<u64, EventError> {
        let date = new.birth;
        let id = self.create_child(ctx, new, pop_genetics, lineage, world.populations)?;
        let parents: Vec<u64> = if lineage.father == lineage.mother {
            vec![lineage.father]
        } else {
            vec![lineage.father, lineage.mother]
        };
        let mut first = None;
        self.emit(world, &parents, "child_born", date, &mut first);
        conclude(first).map(|()| id)
    }

    /// Kill a character (`die_of`) and, once the death commits, emit `death_in_family` to each live parent, spouse
    /// and child of the dead.
    ///
    /// # Errors
    /// A missing character or a failing archive append changes nothing and emits nothing. After the death commits a later
    /// failure is returned but the death stands.
    pub fn die_with_events<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        day: GameTime,
        cause: &str,
    ) -> Result<(), EventError> {
        let dead = self.live.get(&id).cloned().ok_or(Error::MissingCharacter)?;
        self.bury(dead.clone(), day, Some(cause), world.archive)
            .map_err(EventError::Archive)?;
        let mut first = None;
        self.bereave(world, &dead, day, &mut first);
        conclude(first)
    }

    /// Marry two characters (`marry`) and, when the marriage is new, emit `marriage` to both. Marrying an existing
    /// spouse is a no-op that emits nothing.
    ///
    /// # Errors
    /// The refusals of `marry` change nothing. After the marriage commits a later failure is returned but the marriage stands.
    pub fn marry_with_events<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        a: u64,
        b: u64,
        day: GameTime,
    ) -> Result<(), EventError> {
        let married = |x: u64, y: u64| self.live.get(&x).is_some_and(|c| c.spouses.contains(&y));
        let existing = married(a, b) && married(b, a);
        self.marry(a, b, day)?;
        if existing {
            return Ok(());
        }
        let mut first = None;
        self.emit(world, &[a, b], "marriage", day, &mut first);
        conclude(first)
    }

    /// `indulge`, then precedence (§4): a vice indulged can fail a calling's conditions, and the attribute falls
    /// the same day with its line.
    ///
    /// # Errors
    /// As `indulge`, and as [`apply_precedence`](Self::apply_precedence).
    pub fn indulge_with_events<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        pair: &str,
        day: GameTime,
    ) -> Result<(), EventError> {
        self.indulge(world.personality, id, pair)?;
        self.apply_precedence(world, id, day)
    }

    /// `confess`, then precedence.
    ///
    /// # Errors
    /// As `confess`, and as [`apply_precedence`](Self::apply_precedence).
    pub fn confess_with_events<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        day: GameTime,
    ) -> Result<(), EventError> {
        self.confess(world.personality, id)?;
        self.apply_precedence(world, id, day)
    }

    /// `move_to`, then precedence: a pop's type and luxuries are inputs of the conditions.
    ///
    /// # Errors
    /// As `move_to`, and as [`apply_precedence`](Self::apply_precedence).
    pub fn move_to_with_events<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        pop: PopLocator,
        day: GameTime,
    ) -> Result<(), EventError> {
        self.move_to(id, pop, world.populations)?;
        self.apply_precedence(world, id, day)
    }

    /// `set_trait_level`, then precedence (a condition may read a mind trait).
    ///
    /// # Errors
    /// As `set_trait_level`, and as [`apply_precedence`](Self::apply_precedence).
    pub fn set_trait_level_with_events<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        which: crate::LevelTrait,
        level: u8,
        day: GameTime,
    ) -> Result<(), EventError> {
        self.set_trait_level(id, which, level)?;
        self.apply_precedence(world, id, day)
    }
}
