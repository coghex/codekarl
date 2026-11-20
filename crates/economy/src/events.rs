//! Character event data: the catalog of traits and attributes, the events, deeds, precedence and the skills (CK-363).
//!
//! Nothing here uses floating point. Every number is read from its decimal text as an exact integer: chances in parts per
//! billion, every other value in thousandths. A formula over them is evaluated by the engine as one exact rational.
#![deny(clippy::float_arithmetic)]

use crate::{
    DataError, Genetics, Peoples,
    genetics::{HUMOURS, Node, Source, Table, attribute_index},
    personality::{MANNER_PAIRS, VICE_PAIRS},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

/// The grade names by level: level 0 is "none", then C, B, A, S, SS and SSS.
pub const GRADE_NAMES: [&str; 7] = ["none", "C", "B", "A", "S", "SS", "SSS"];
/// The highest grade level.
pub const MAX_GRADE: u8 = 6;
/// Weight of a humour in the humour factor, in thousandths: `1 + 0.15 (humour - 5)`.
pub const HUMOUR_WEIGHT: i64 = 150;
/// The lowest humour factor, in thousandths.
pub const HUMOUR_FLOOR: i64 = 200;
/// Points of stress over the threshold that double a stress factor (the event format's fixed 1/20).
pub const STRESS_FACTOR_DIVISOR: i64 = 20;
/// The lowest and highest virtue or manner factor, in thousandths.
pub const PAIR_FACTOR_BOUNDS: [i64; 2] = [100, 3000];
/// Skill divisor of the craft factor: `0.1 + skill / 20`.
pub const CRAFT_FACTOR_DIVISOR: i64 = 20;
/// The skill level of a full craft-rise chance: the chance is `rise x skill / 15`.
pub const CRAFT_RISE_DIVISOR: i64 = 15;
/// The most a yearly skill growth rate may be, in thousandths (100,000 levels a year): the loader's validated contract, so
/// that a skill's arithmetic always fits.
pub const MAX_GAIN_RATE: i64 = 100_000_000;
/// The most a focus multiplier may be, in thousandths.
pub const MAX_FOCUS: i64 = 10_000;
/// The most a fraction of home education may be (rate, age factor, scholarship speed-up), in thousandths.
pub const MAX_UNIT: i64 = 1000;
/// The mind traits slice 3 keeps as levels of the personality record.
pub const SLICE_THREE_TRAITS: [&str; 3] = ["melancholy", "rage_fits", "drunkard"];

/// What a catalog key is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A trait that passes by genetics.
    InheritedTrait,
    /// A trait of the mind, caused and lifted only by events.
    MindTrait,
    /// An attribute whose grade follows a counter.
    Deed,
    /// An attribute granted by a character moment whose grade rises on occasion.
    Craft,
    /// A church calling.
    Calling,
    /// A station.
    Station,
    /// A background.
    Background,
}

impl Kind {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "inherited_trait" => Self::InheritedTrait,
            "mind_trait" => Self::MindTrait,
            "deed" => Self::Deed,
            "craft" => Self::Craft,
            "calling" => Self::Calling,
            "station" => Self::Station,
            "background" => Self::Background,
            _ => return None,
        })
    }

    /// Whether precedence may remove an attribute of this kind.
    #[must_use]
    pub const fn removable(self) -> bool {
        matches!(self, Self::Calling | Self::Station | Self::Background)
    }
}

/// One key of the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    /// The key's kind.
    pub kind: Kind,
    /// Whether it has grades C to SSS.
    pub graded: bool,
    /// Whether the yearly stress events move its grade.
    pub stress_moves: bool,
    /// Whether genetics also passes it.
    pub inherited: bool,
}

/// What fires an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The birthday check.
    Yearly,
    /// A happening reported for the character.
    Happening(String),
}

/// A sex requirement of a condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SexRule {
    /// Men only.
    Male,
    /// Women only.
    Female,
}

/// The conditions of an event or of a `keep_while` entry; every one that is set must hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Conditions {
    /// Age in whole years, low and high.
    pub age: Option<[u32; 2]>,
    /// Sex.
    pub sex: Option<SexRule>,
    /// Pop types, any of.
    pub pop_type: Vec<String>,
    /// Culture subgroups, any of.
    pub culture_group: Vec<String>,
    /// Keys the character must hold.
    pub has: Vec<String>,
    /// Keys the character must not hold.
    pub lacks: Vec<String>,
    /// Skills at or above a level (thousandths).
    pub skill_min: Vec<(String, i64)>,
    /// Vice/virtue pairs (index) at or above a value (thousandths).
    pub virtue_min: Vec<(usize, i64)>,
    /// Vice/virtue pairs at or below a value.
    pub virtue_max: Vec<(usize, i64)>,
    /// Manner pairs (index) at or above a value.
    pub manner_min: Vec<(usize, i64)>,
    /// Manner pairs at or below a value.
    pub manner_max: Vec<(usize, i64)>,
    /// Stress minus threshold at least this (thousandths).
    pub over_threshold_min: Option<i64>,
    /// Stress above the threshold.
    pub over_threshold: bool,
    /// The holds-land flag is false (`keep_while` only).
    pub lacks_land: bool,
    /// The pop's luxuries fulfilment at least this, in permille (`keep_while` only).
    pub pop_luxuries_min: Option<i64>,
    /// Graded traits at or above a level 1..=6 (epithets only, CK-365).
    pub grade_min: Vec<(String, u8)>,
    /// A shown stat (index into the genetic attributes) at most `(m, f)` in thousandths, for a man and a woman (epithets only).
    pub stat_max_by_sex: Vec<(usize, i64, i64)>,
    /// The shown weight over the rolled weight at least this, in thousandths (epithets only).
    pub weight_ratio_min: Option<i64>,
    /// Deed counters at least a value (epithets only).
    pub count_min: Vec<(String, u64)>,
    /// The character is past the age of majority (epithets only).
    pub adult: bool,
}

/// Which way stress moves a chance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StressFactor {
    /// Multiplies the chance by the stress factor.
    Cause,
    /// Divides the chance by it.
    Lift,
}

/// One multiplier of an event's chance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Factor {
    /// `1 + 0.15 (humour - 5)`, floored at 0.2; the index is into [`HUMOURS`].
    Humour(usize),
    /// `over = 1 + max(0, stress - threshold) / 20`: cause `x over`, lift `/ over`.
    Stress(StressFactor),
    /// `1 + k x value`, clamped to 0.1..3: a vice/virtue pair and `k` (thousandths).
    Virtue {
        /// The pair's index.
        pair: usize,
        /// `k`, thousandths.
        per: i64,
    },
    /// The same for a manner pair.
    Manner {
        /// The pair's index.
        pair: usize,
        /// `k`, thousandths.
        per: i64,
    },
    /// `0.1 + skill / 20`.
    Craft(String),
    /// A table by pop type (missing = 0), thousandths.
    PopType(BTreeMap<String, i64>),
    /// A table by culture subgroup (missing = 1), thousandths.
    CultureGroup(BTreeMap<String, i64>),
    /// By the character's grade of a trait, none to SSS, thousandths.
    Grade {
        /// The trait.
        key: String,
        /// The seven values.
        by: [i64; 7],
    },
}

/// A standing value an event changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandingKind {
    /// Popularity.
    Popularity,
    /// Fame.
    Fame,
    /// Honour.
    Honour,
}

/// One effect of an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// None becomes C, otherwise one grade up, at most SSS.
    RaiseGrade(String),
    /// One grade down; C is removed.
    LowerGrade(String),
    /// C for a graded attribute; no change if held.
    Grant(String),
    /// Remove an attribute.
    Remove(String),
    /// Add to a deed counter.
    Count {
        /// The counter.
        counter: String,
        /// The amount, at least 1.
        add: u64,
    },
    /// Add stress (thousandths); a negative amount is a relief.
    Stress(i64),
    /// Change a standing value (thousandths).
    Standing {
        /// Which value.
        which: StandingKind,
        /// The change.
        add: i64,
    },
    /// The character dies of this cause.
    Die(String),
}

/// A character event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// Unique key.
    pub id: String,
    /// What fires it.
    pub trigger: Trigger,
    /// Conditions.
    pub when: Conditions,
    /// Base chance in ppb.
    pub chance_ppb: u32,
    /// Chance factors, in data order.
    pub factors: Vec<Factor>,
    /// Effects, in data order.
    pub effects: Vec<Effect>,
    /// The life-log line.
    pub text: String,
}

/// A deed: its counter and the counts that give C, B, A, S, SS and SSS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deed {
    /// The attribute key.
    pub key: String,
    /// The counter its grade follows.
    pub counter: String,
    /// Thresholds, strictly increasing.
    pub thresholds: [u64; 6],
}

/// The yearly craft rise.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CraftRise {
    /// The base chance in ppb.
    pub rise_ppb: u32,
    /// The skill each craft rises with.
    pub skills: BTreeMap<String, String>,
}

/// A focus of education.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillRules {
    /// The skills, in the order of the data.
    pub list: Vec<String>,
    /// The aptitude stat of a skill, as an index into the genetic attribute array.
    pub aptitude: BTreeMap<String, usize>,
    /// The age from which skills grow by use.
    pub train_from: u32,
    /// Yearly growth rate (thousandths).
    pub gain_rate: i64,
    /// Yearly decay of a fully used skill.
    pub decay_used: i64,
    /// Yearly decay of an unused skill.
    pub decay_unused: i64,
    /// Spread of a character's own use around its profile.
    pub use_spread: i64,
    /// Use profiles by pop type and sex (`'m'` or `'f'`), thousandths.
    pub profiles: BTreeMap<(String, char), BTreeMap<String, i64>>,
    /// Multiplier of the skills of a focus.
    pub focus_on: i64,
    /// Multiplier of the other skills.
    pub focus_off: i64,
    /// The skills of each focus.
    pub focus_skills: BTreeMap<String, Vec<String>>,
    /// Odds of each focus by pop type and sex, in key order; `balanced` is a focus with no skills.
    pub focus_odds: BTreeMap<(String, char), Vec<(String, u32)>>,
    /// Home education rate (thousandths).
    pub education_rate: i64,
    /// Age factor of home education: from this age, this factor (thousandths).
    pub age_factor: Vec<(u32, i64)>,
    /// Home education speed-up per point of scholarship (thousandths).
    pub scholarship_per_point: i64,
}

/// The loaded event data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventRules {
    /// The catalog, in key order (the "catalog order" precedence runs in).
    pub catalog: BTreeMap<String, CatalogEntry>,
    /// The events, in data order.
    pub events: Vec<Event>,
    /// The deeds, in key order.
    pub deeds: Vec<Deed>,
    /// The craft rise.
    pub craft_rise: CraftRise,
    /// Precedence: the conditions an attribute is kept under, in key order.
    pub keep_while: Vec<(String, Conditions)>,
    /// The catalog's keys in the order the file writes them: the order precedence scans in.
    pub catalog_order: Vec<String>,
    /// The skills.
    pub skills: SkillRules,
    /// The culture subgroup of each culture.
    pub culture_group: BTreeMap<String, String>,
}

/// What the loader checks keys against.
pub struct Registry<'a> {
    /// Culture subgroups and cultures.
    pub peoples: &'a Peoples,
    /// The genetic rules, whose trait keys must be in the catalog.
    pub genetics: &'a Genetics,
    /// Pop type keys.
    pub pop_types: &'a [&'a str],
}

/// Which condition keys a table may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Vocabulary {
    /// An event's `when`.
    Event,
    /// A `keep_while` entry (adds `lacks_land` and `pop_luxuries_min`).
    Keep,
    /// An epithet's `when` (adds `grade_min`, `stat_max_by_sex`, `weight_ratio_min`, `count_min` and `adult`).
    Epithet,
}

struct Parser<'a> {
    source: Source<'a>,
    registry: &'a Registry<'a>,
    catalog: BTreeMap<String, CatalogEntry>,
    skills: Vec<String>,
    counters: BTreeSet<String>,
}

fn array<'t>(source: &Source<'_>, node: &'t Node, entry: &str) -> Result<&'t [Node], DataError> {
    match node {
        Node::Array(items) => Ok(items),
        _ => Err(source.error(entry, "expected an array")),
    }
}

fn strings(source: &Source<'_>, node: &Node, entry: &str) -> Result<Vec<String>, DataError> {
    array(source, node, entry)?
        .iter()
        .map(|item| source.text(item, entry).map(str::to_owned))
        .collect()
}

fn only_keys(
    source: &Source<'_>,
    table: &Table,
    entry: &str,
    allowed: &[&str],
) -> Result<(), DataError> {
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(source.error(format!("{entry}.{key}"), "unknown key"));
        }
    }
    Ok(())
}

fn required<'t>(
    source: &Source<'_>,
    table: &'t Table,
    entry: &str,
    key: &str,
) -> Result<&'t Node, DataError> {
    table
        .get(key)
        .ok_or_else(|| source.error(format!("{entry}.{key}"), "missing value"))
}

fn positive_u64(source: &Source<'_>, node: &Node, entry: &str) -> Result<u64, DataError> {
    u64::try_from(source.integer(node, entry)?)
        .map_err(|_| source.error(entry, "expected a non-negative whole number"))
}

fn small_u32(source: &Source<'_>, node: &Node, entry: &str) -> Result<u32, DataError> {
    u32::try_from(source.integer(node, entry)?)
        .map_err(|_| source.error(entry, "expected a whole number in 0..4294967295"))
}

impl Parser<'_> {
    fn error(&self, entry: impl Into<String>, problem: impl Into<String>) -> DataError {
        self.source.error(entry, problem)
    }

    fn key(&self, name: &str, entry: &str) -> Result<(), DataError> {
        if self.catalog.contains_key(name) {
            Ok(())
        } else {
            Err(self.error(entry, format!("`{name}` is not a key of the catalog")))
        }
    }

    fn skill(&self, name: &str, entry: &str) -> Result<(), DataError> {
        if self.skills.iter().any(|skill| skill == name) {
            Ok(())
        } else {
            Err(self.error(entry, format!("`{name}` is not in `[skills].list`")))
        }
    }

    fn pop_type(&self, name: &str, entry: &str) -> Result<(), DataError> {
        if self.registry.pop_types.contains(&name) {
            Ok(())
        } else {
            Err(self.error(entry, format!("`{name}` is not a pop type")))
        }
    }

    fn subgroup(&self, name: &str, entry: &str) -> Result<(), DataError> {
        if self.registry.peoples.subgroups.iter().any(|g| g == name) {
            Ok(())
        } else {
            Err(self.error(entry, format!("`{name}` is not a culture subgroup")))
        }
    }

    fn pair(names: &[&str], name: &str) -> Option<usize> {
        names.iter().position(|known| *known == name)
    }

    fn pair_values(
        &self,
        node: &Node,
        entry: &str,
        names: &[&str],
    ) -> Result<Vec<(usize, i64)>, DataError> {
        let mut out = Vec::new();
        for (name, value) in self.source.table(node, entry)? {
            let at = format!("{entry}.{name}");
            let index = Self::pair(names, name)
                .ok_or_else(|| self.error(&at, format!("`{name}` is not a known pair")))?;
            out.push((index, self.source.milli(value, &at)?));
        }
        Ok(out)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per condition of the vocabulary, in the file's own order"
    )]
    fn conditions(
        &self,
        node: &Node,
        entry: &str,
        vocabulary: Vocabulary,
    ) -> Result<Conditions, DataError> {
        let table = self.source.table(node, entry)?;
        let mut allowed = vec![
            "age",
            "sex",
            "pop_type",
            "culture_group",
            "has",
            "lacks",
            "skill_min",
            "virtue_min",
            "virtue_max",
            "manner_min",
            "manner_max",
            "over_threshold_min",
            "over_threshold",
        ];
        match vocabulary {
            Vocabulary::Event => {}
            Vocabulary::Keep => allowed.extend(["lacks_land", "pop_luxuries_min"]),
            Vocabulary::Epithet => allowed.extend([
                "grade_min",
                "stat_max_by_sex",
                "weight_ratio_min",
                "count_min",
                "adult",
            ]),
        }
        only_keys(&self.source, table, entry, &allowed)?;
        let mut out = Conditions::default();
        for (key, node) in table {
            let at = format!("{entry}.{key}");
            match key.as_str() {
                "age" => {
                    let items = array(&self.source, node, &at)?;
                    let [low, high] = items else {
                        return Err(self.error(&at, "an age range has exactly two numbers"));
                    };
                    let range = [
                        small_u32(&self.source, low, &at)?,
                        small_u32(&self.source, high, &at)?,
                    ];
                    if range[0] > range[1] {
                        return Err(self.error(&at, "the low age is above the high age"));
                    }
                    out.age = Some(range);
                }
                "sex" => {
                    out.sex = Some(match self.source.text(node, &at)? {
                        "m" => SexRule::Male,
                        "f" => SexRule::Female,
                        _ => return Err(self.error(&at, "a sex is \"m\" or \"f\"")),
                    });
                }
                "pop_type" => {
                    for name in strings(&self.source, node, &at)? {
                        self.pop_type(&name, &at)?;
                        out.pop_type.push(name);
                    }
                }
                "culture_group" => {
                    for name in strings(&self.source, node, &at)? {
                        self.subgroup(&name, &at)?;
                        out.culture_group.push(name);
                    }
                }
                "has" | "lacks" => {
                    let names = strings(&self.source, node, &at)?;
                    for name in &names {
                        self.key(name, &at)?;
                    }
                    if key == "has" {
                        out.has = names;
                    } else {
                        out.lacks = names;
                    }
                }
                "skill_min" => {
                    for (name, value) in self.source.table(node, &at)? {
                        let item = format!("{at}.{name}");
                        self.skill(name, &item)?;
                        out.skill_min
                            .push((name.clone(), self.source.milli(value, &item)?));
                    }
                }
                "virtue_min" => out.virtue_min = self.pair_values(node, &at, &VICE_PAIRS)?,
                "virtue_max" => out.virtue_max = self.pair_values(node, &at, &VICE_PAIRS)?,
                "manner_min" => out.manner_min = self.pair_values(node, &at, &MANNER_PAIRS)?,
                "manner_max" => out.manner_max = self.pair_values(node, &at, &MANNER_PAIRS)?,
                "over_threshold_min" => {
                    out.over_threshold_min = Some(self.source.milli_non_negative(node, &at)?);
                }
                "over_threshold" => {
                    if !self.source.boolean(node, &at)? {
                        return Err(self.error(&at, "`over_threshold` is only ever true"));
                    }
                    out.over_threshold = true;
                }
                "grade_min" => {
                    for (name, value) in self.source.table(node, &at)? {
                        let item = format!("{at}.{name}");
                        self.key(name, &item)?;
                        if !self.catalog[name].graded {
                            return Err(self.error(item, format!("`{name}` is not a graded trait")));
                        }
                        let level = u8::try_from(self.source.integer(value, &item)?)
                            .ok()
                            .filter(|level| (1..=MAX_GRADE).contains(level))
                            .ok_or_else(|| self.error(&item, "a grade is 1 (C) to 6 (SSS)"))?;
                        out.grade_min.push((name.clone(), level));
                    }
                }
                "stat_max_by_sex" => {
                    for (name, value) in self.source.table(node, &at)? {
                        let item = format!("{at}.{name}");
                        let stat = attribute_index(name).ok_or_else(|| {
                            self.error(&item, format!("`{name}` is not a genetic stat"))
                        })?;
                        let limits = self.source.table(value, &item)?;
                        only_keys(&self.source, limits, &item, &["m", "f"])?;
                        let limit = |sex: &str| -> Result<i64, DataError> {
                            self.source.milli_non_negative(
                                required(&self.source, limits, &item, sex)?,
                                &format!("{item}.{sex}"),
                            )
                        };
                        out.stat_max_by_sex.push((stat, limit("m")?, limit("f")?));
                    }
                }
                "weight_ratio_min" => {
                    let ratio = self.source.milli_non_negative(node, &at)?;
                    if ratio == 0 {
                        return Err(self.error(&at, "a weight ratio is above 0"));
                    }
                    out.weight_ratio_min = Some(ratio);
                }
                "count_min" => {
                    for (name, value) in self.source.table(node, &at)? {
                        let item = format!("{at}.{name}");
                        if !self.counters.contains(name) {
                            return Err(
                                self.error(item, format!("`{name}` is not a counter of `[deeds]`"))
                            );
                        }
                        out.count_min
                            .push((name.clone(), positive_u64(&self.source, value, &item)?));
                    }
                }
                "adult" => {
                    if !self.source.boolean(node, &at)? {
                        return Err(self.error(&at, "`adult` is only ever true"));
                    }
                    out.adult = true;
                }
                "lacks_land" => {
                    if !self.source.boolean(node, &at)? {
                        return Err(self.error(&at, "`lacks_land` is only ever true"));
                    }
                    out.lacks_land = true;
                }
                _ => {
                    let value = self.source.integer(node, &at)?;
                    if !(0..=1000).contains(&value) {
                        return Err(self.error(&at, "luxuries are in permille, 0..1000"));
                    }
                    out.pop_luxuries_min = Some(i64::try_from(value).unwrap_or(0));
                }
            }
        }
        Ok(out)
    }

    fn multipliers(
        &self,
        node: &Node,
        entry: &str,
        pop: bool,
    ) -> Result<BTreeMap<String, i64>, DataError> {
        let mut out = BTreeMap::new();
        for (name, value) in self.source.table(node, entry)? {
            let at = format!("{entry}.{name}");
            if pop {
                self.pop_type(name, &at)?;
            } else {
                self.subgroup(name, &at)?;
            }
            out.insert(name.clone(), self.source.milli_non_negative(value, &at)?);
        }
        Ok(out)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per factor of the vocabulary"
    )]
    fn factor(&self, node: &Node, entry: &str) -> Result<Factor, DataError> {
        let table = self.source.table(node, entry)?;
        let first = [
            "humour",
            "stress",
            "virtue",
            "manner",
            "skill",
            "pop_type",
            "culture_group",
            "grade",
        ]
        .into_iter()
        .find(|name| table.contains_key(*name))
        .ok_or_else(|| self.error(entry, "unknown factor"))?;
        let value = |key: &str| required(&self.source, table, entry, key);
        let at = |key: &str| format!("{entry}.{key}");
        let factor = match first {
            "humour" => {
                only_keys(&self.source, table, entry, &["humour"])?;
                let name = self.source.text(value("humour")?, &at("humour"))?;
                Factor::Humour(
                    HUMOURS.iter().position(|h| *h == name).ok_or_else(|| {
                        self.error(at("humour"), format!("`{name}` is not a humour"))
                    })?,
                )
            }
            "stress" => {
                only_keys(&self.source, table, entry, &["stress"])?;
                match self.source.text(value("stress")?, &at("stress"))? {
                    "cause" => Factor::Stress(StressFactor::Cause),
                    "lift" => Factor::Stress(StressFactor::Lift),
                    _ => {
                        return Err(
                            self.error(at("stress"), "a stress factor is \"cause\" or \"lift\"")
                        );
                    }
                }
            }
            "virtue" | "manner" => {
                let is_virtue = first == "virtue";
                only_keys(&self.source, table, entry, &[first, "per"])?;
                let name = self.source.text(value(first)?, &at(first))?;
                let pair = Self::pair(
                    if is_virtue {
                        &VICE_PAIRS
                    } else {
                        &MANNER_PAIRS
                    },
                    name,
                )
                .ok_or_else(|| self.error(at(first), format!("`{name}` is not a known pair")))?;
                let per = self.source.milli(value("per")?, &at("per"))?;
                if is_virtue {
                    Factor::Virtue { pair, per }
                } else {
                    Factor::Manner { pair, per }
                }
            }
            "skill" => {
                only_keys(&self.source, table, entry, &["skill", "craft"])?;
                if !self.source.boolean(value("craft")?, &at("craft"))? {
                    return Err(self.error(
                        at("craft"),
                        "the skill factor is the craft form: `craft = true`",
                    ));
                }
                let name = self.source.text(value("skill")?, &at("skill"))?;
                self.skill(name, &at("skill"))?;
                Factor::Craft(name.to_owned())
            }
            "pop_type" => {
                only_keys(&self.source, table, entry, &["pop_type"])?;
                Factor::PopType(self.multipliers(value("pop_type")?, &at("pop_type"), true)?)
            }
            "culture_group" => {
                only_keys(&self.source, table, entry, &["culture_group"])?;
                Factor::CultureGroup(self.multipliers(
                    value("culture_group")?,
                    &at("culture_group"),
                    false,
                )?)
            }
            "grade" => {
                only_keys(&self.source, table, entry, &["grade", "by"])?;
                let key = self.source.text(value("grade")?, &at("grade"))?;
                self.key(key, &at("grade"))?;
                if !self.catalog[key].graded {
                    return Err(self.error(at("grade"), format!("`{key}` is not graded")));
                }
                let items = array(&self.source, value("by")?, &at("by"))?;
                if items.len() != 7 {
                    return Err(
                        self.error(at("by"), "a grade factor has seven values, none to SSS")
                    );
                }
                let mut by = [0_i64; 7];
                for (slot, item) in by.iter_mut().zip(items) {
                    *slot = self.source.milli_non_negative(item, &at("by"))?;
                }
                Factor::Grade {
                    key: key.to_owned(),
                    by,
                }
            }
            _ => return Err(self.error(entry, "unknown factor")),
        };
        Ok(factor)
    }

    fn graded_kind(&self, key: &str, entry: &str, effect: &str) -> Result<CatalogEntry, DataError> {
        self.key(key, entry)?;
        let found = self.catalog[key];
        let allowed = match effect {
            "raise_grade" | "lower_grade" => found.kind == Kind::MindTrait && found.graded,
            "grant" => matches!(
                found.kind,
                Kind::Craft | Kind::Calling | Kind::Station | Kind::Background
            ),
            _ => found.kind.removable(),
        };
        if allowed {
            Ok(found)
        } else {
            Err(self.error(
                entry,
                format!(
                    "`{effect}` does not apply to `{key}`: its kind is {:?}",
                    found.kind
                ),
            ))
        }
    }

    fn effect(&self, node: &Node, entry: &str) -> Result<Effect, DataError> {
        let table = self.source.table(node, entry)?;
        let first = [
            "raise_grade",
            "lower_grade",
            "grant",
            "remove",
            "count",
            "stress",
            "standing",
            "die",
        ]
        .into_iter()
        .find(|name| table.contains_key(*name))
        .ok_or_else(|| self.error(entry, "unknown effect"))?;
        let at = |key: &str| format!("{entry}.{key}");
        let text = |key: &str| -> Result<String, DataError> {
            Ok(self
                .source
                .text(required(&self.source, table, entry, key)?, &at(key))?
                .to_owned())
        };
        Ok(match first {
            "raise_grade" | "lower_grade" | "grant" | "remove" => {
                only_keys(&self.source, table, entry, &[first])?;
                let key = text(first)?;
                self.graded_kind(&key, &at(first), first)?;
                match first {
                    "raise_grade" => Effect::RaiseGrade(key),
                    "lower_grade" => Effect::LowerGrade(key),
                    "grant" => Effect::Grant(key),
                    _ => Effect::Remove(key),
                }
            }
            "count" => {
                only_keys(&self.source, table, entry, &["count", "add"])?;
                let counter = text("count")?;
                if !self.counters.contains(&counter) {
                    return Err(self.error(
                        at("count"),
                        format!("`{counter}` is not a counter of `[deeds]`"),
                    ));
                }
                let add = positive_u64(
                    &self.source,
                    required(&self.source, table, entry, "add")?,
                    &at("add"),
                )?;
                if add == 0 {
                    return Err(self.error(at("add"), "a count adds at least 1"));
                }
                Effect::Count { counter, add }
            }
            "stress" => {
                only_keys(&self.source, table, entry, &["stress"])?;
                Effect::Stress(self.source.milli(&table["stress"], &at("stress"))?)
            }
            "standing" => {
                only_keys(&self.source, table, entry, &["standing", "add"])?;
                let which = match text("standing")?.as_str() {
                    "popularity" => StandingKind::Popularity,
                    "fame" => StandingKind::Fame,
                    "honour" => StandingKind::Honour,
                    _ => {
                        return Err(
                            self.error(at("standing"), "a standing is popularity, fame or honour")
                        );
                    }
                };
                let add = self
                    .source
                    .milli(required(&self.source, table, entry, "add")?, &at("add"))?;
                Effect::Standing { which, add }
            }
            "die" => {
                only_keys(&self.source, table, entry, &["die"])?;
                Effect::Die(text("die")?)
            }
            _ => return Err(self.error(entry, "unknown effect")),
        })
    }

    fn placeholders(&self, event: &Event, entry: &str) -> Result<(), DataError> {
        // Effects after a `die` never run, so only the ones before it can set a field.
        let reachable = match event
            .effects
            .iter()
            .position(|effect| matches!(effect, Effect::Die(_)))
        {
            Some(death) => &event.effects[..death],
            None => &event.effects[..],
        };
        let sets_trait = reachable.iter().any(|e| {
            matches!(
                e,
                Effect::RaiseGrade(_)
                    | Effect::LowerGrade(_)
                    | Effect::Grant(_)
                    | Effect::Remove(_)
            )
        });
        let sets_count = reachable.iter().any(|e| matches!(e, Effect::Count { .. }));
        let mut rest = event.text.as_str();
        while let Some(open) = rest.find(['{', '}']) {
            if rest.as_bytes()[open] == b'}' {
                return Err(self.error(
                    format!("{entry}.text"),
                    "a closing brace without an opening one",
                ));
            }
            let Some(close) = rest[open..].find('}') else {
                return Err(self.error(format!("{entry}.text"), "an unclosed placeholder"));
            };
            let name = &rest[open + 1..open + close];
            let settable = match name {
                "name" => true,
                "trait" | "grade" => sets_trait,
                "count" => sets_count,
                _ => {
                    return Err(self.error(
                        format!("{entry}.text"),
                        format!("unknown placeholder `{{{name}}}`"),
                    ));
                }
            };
            if !settable {
                return Err(self.error(
                    format!("{entry}.text"),
                    format!("`{{{name}}}` is a field no effect of this event can set"),
                ));
            }
            rest = &rest[open + close + 1..];
        }
        Ok(())
    }

    fn event(&self, node: &Node, index: usize) -> Result<Event, DataError> {
        let table = self.source.table(node, &format!("event[{index}]"))?;
        let id = self
            .source
            .text(
                required(&self.source, table, &format!("event[{index}]"), "id")?,
                "event.id",
            )?
            .to_owned();
        let entry = format!("event.{id}");
        only_keys(
            &self.source,
            table,
            &entry,
            &[
                "id", "trigger", "when", "chance", "factors", "effects", "text",
            ],
        )?;
        let trigger = match self.source.text(
            required(&self.source, table, &entry, "trigger")?,
            &format!("{entry}.trigger"),
        )? {
            "yearly" => Trigger::Yearly,
            other => match other.strip_prefix("happening:") {
                Some(name) if !name.is_empty() => Trigger::Happening(name.to_owned()),
                _ => {
                    return Err(self.error(
                        format!("{entry}.trigger"),
                        "a trigger is `yearly` or `happening:<name>`",
                    ));
                }
            },
        };
        let when = match table.get("when") {
            Some(node) => self.conditions(node, &format!("{entry}.when"), Vocabulary::Event)?,
            None => Conditions::default(),
        };
        let chance_ppb = self.source.ppb(
            required(&self.source, table, &entry, "chance")?,
            &format!("{entry}.chance"),
        )?;
        let mut factors = Vec::new();
        if let Some(node) = table.get("factors") {
            let at = format!("{entry}.factors");
            for (position, item) in array(&self.source, node, &at)?.iter().enumerate() {
                factors.push(self.factor(item, &format!("{at}[{position}]"))?);
            }
        }
        let mut effects = Vec::new();
        let at = format!("{entry}.effects");
        for (position, item) in array(
            &self.source,
            required(&self.source, table, &entry, "effects")?,
            &at,
        )?
        .iter()
        .enumerate()
        {
            effects.push(self.effect(item, &format!("{at}[{position}]"))?);
        }
        let text = self
            .source
            .text(
                required(&self.source, table, &entry, "text")?,
                &format!("{entry}.text"),
            )?
            .to_owned();
        let event = Event {
            id,
            trigger,
            when,
            chance_ppb,
            factors,
            effects,
            text,
        };
        for item in &event.factors {
            if let Factor::Grade { key, .. } = item {
                self.key(key, &format!("{entry}.factors"))?;
            }
        }
        self.placeholders(&event, &entry)?;
        Ok(event)
    }
}

/// The catalog's keys in the order the file writes them. The validated map keeps no file order, so the order is read from the key
/// positions of the parsed source itself (a key's spelling, quoted or not, is the parser's).
fn catalog_order(source: &Source<'_>, text: &str, table: &Table) -> Result<Vec<String>, DataError> {
    let parsed = toml::de::DeTable::parse(text)
        .map_err(|error| source.error("<document>", error.message().to_owned()))?;
    let Some(catalog) = parsed.get_ref().get("catalog") else {
        return Err(source.error("catalog", "missing value"));
    };
    let toml::de::DeValue::Table(entries) = catalog.get_ref() else {
        return Err(source.error("catalog", "expected a table"));
    };
    let mut positions: Vec<(usize, String)> = entries
        .iter()
        .map(|(key, _)| (key.span().start, key.get_ref().as_ref().to_owned()))
        .collect();
    positions.sort();
    let order: Vec<String> = positions.into_iter().map(|(_, key)| key).collect();
    let mut sorted = order.clone();
    sorted.sort();
    if sorted.iter().ne(table.keys()) {
        return Err(source.error("catalog", "the catalog's keys cannot be read in file order"));
    }
    Ok(order)
}

/// Which events and counters exist, for the cross-checks.
fn parse_catalog(
    source: &Source<'_>,
    table: &Table,
) -> Result<BTreeMap<String, CatalogEntry>, DataError> {
    let mut out = BTreeMap::new();
    for (key, node) in table {
        let entry = format!("catalog.{key}");
        let item = source.table(node, &entry)?;
        only_keys(
            source,
            item,
            &entry,
            &["kind", "graded", "stress_moves", "inherited"],
        )?;
        let kind_text = source.text(
            required(source, item, &entry, "kind")?,
            &format!("{entry}.kind"),
        )?;
        let kind = Kind::parse(kind_text).ok_or_else(|| {
            source.error(
                format!("{entry}.kind"),
                format!("`{kind_text}` is not a kind"),
            )
        })?;
        let flag = |name: &str| -> Result<bool, DataError> {
            item.get(name).map_or(Ok(false), |node| {
                source.boolean(node, &format!("{entry}.{name}"))
            })
        };
        let graded = source.boolean(
            required(source, item, &entry, "graded")?,
            &format!("{entry}.graded"),
        )?;
        let stress_moves = flag("stress_moves")?;
        if stress_moves && !(graded && kind == Kind::MindTrait) {
            return Err(source.error(
                format!("{entry}.stress_moves"),
                "only a graded mind trait is moved by stress",
            ));
        }
        if matches!(kind, Kind::Deed | Kind::Craft | Kind::MindTrait) && !graded {
            return Err(source.error(
                format!("{entry}.graded"),
                "a deed, craft or mind trait is graded",
            ));
        }
        if matches!(kind, Kind::Calling | Kind::Station | Kind::Background) && graded {
            return Err(source.error(
                format!("{entry}.graded"),
                "a calling, station or background is ungraded",
            ));
        }
        out.insert(
            key.clone(),
            CatalogEntry {
                kind,
                graded,
                stress_moves,
                inherited: flag("inherited")?,
            },
        );
    }
    Ok(out)
}

/// Whom an epithet may be gained by (CK-365).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EpithetScope {
    /// Everyone.
    Universal,
    /// A character whose culture's subgroup (CK-356) is this key.
    Subgroup(String),
    /// A character whose pop's religion is this key.
    Religion(String),
}

/// What gains an epithet (CK-365).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EpithetTrigger {
    /// The birthday pass.
    Yearly,
    /// A happening reported for the character.
    Happening(String),
    /// A system not built yet: the epithet loads and is never gained.
    Later,
}

/// An epithet: a title a character gains by chance or by an event, kept for life, the most important of those held being shown
/// after the name (CK-365, characters slice 5b).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Epithet {
    /// The key, `[epithet.<key>]`.
    pub key: String,
    /// The text shown after the name.
    pub shown: String,
    /// How important it is: the more important of two held is the one shown.
    pub importance: u32,
    /// Who may gain it.
    pub scope: EpithetScope,
    /// What gains it.
    pub trigger: EpithetTrigger,
    /// The conditions that must hold (empty for a `later` epithet, which has none).
    pub when: Conditions,
    /// The chance in ppb each time the trigger tests it (0 for a `later` epithet).
    pub chance_ppb: u32,
}

/// The scope of an epithet: `universal`, a culture subgroup or a religion (and not both).
fn epithet_scope(
    source: &Source<'_>,
    registry: &Registry<'_>,
    religions: &[String],
    entry: &str,
    value: &str,
) -> Result<EpithetScope, DataError> {
    if value == "universal" {
        return Ok(EpithetScope::Universal);
    }
    let group = registry.peoples.subgroups.iter().any(|g| g == value);
    let religion = religions.iter().any(|r| r == value);
    match (group, religion) {
        (true, false) => Ok(EpithetScope::Subgroup(value.to_owned())),
        (false, true) => Ok(EpithetScope::Religion(value.to_owned())),
        (true, true) => Err(source.error(
            entry,
            format!("`{value}` is both a culture subgroup and a religion"),
        )),
        (false, false) => Err(source.error(
            entry,
            format!("`{value}` is not `universal`, a culture subgroup or a religion"),
        )),
    }
}

/// The trigger of an epithet: `yearly`, `happening:<name>` or `later`.
fn epithet_trigger(
    source: &Source<'_>,
    entry: &str,
    value: &str,
) -> Result<EpithetTrigger, DataError> {
    match value {
        "yearly" => Ok(EpithetTrigger::Yearly),
        "later" => Ok(EpithetTrigger::Later),
        other => match other.strip_prefix("happening:") {
            Some(name) if !name.is_empty() => Ok(EpithetTrigger::Happening(name.to_owned())),
            _ => Err(source.error(
                entry,
                "a trigger is `yearly`, `happening:<name>` or `later`",
            )),
        },
    }
}

/// The epithet keys in the order the file writes them (the validated map keeps no file order).
fn epithet_order(source: &Source<'_>, text: &str, table: &Table) -> Result<Vec<String>, DataError> {
    let parsed = toml::de::DeTable::parse(text)
        .map_err(|error| source.error("<document>", error.message().to_owned()))?;
    let Some(epithets) = parsed.get_ref().get("epithet") else {
        return Err(source.error("epithet", "missing value"));
    };
    let toml::de::DeValue::Table(entries) = epithets.get_ref() else {
        return Err(source.error("epithet", "expected a table"));
    };
    let mut positions: Vec<(usize, String)> = entries
        .iter()
        .map(|(key, _)| (key.span().start, key.get_ref().as_ref().to_owned()))
        .collect();
    positions.sort();
    let order: Vec<String> = positions.into_iter().map(|(_, key)| key).collect();
    let mut sorted = order.clone();
    sorted.sort();
    if sorted.iter().ne(table.keys()) {
        return Err(source.error("epithet", "the epithets cannot be read in file order"));
    }
    Ok(order)
}

impl EventRules {
    /// Parse and validate `epithets.toml` (CK-365) against these rules' catalog, skills and counters, the registry's culture
    /// subgroups and the rules' religions. The epithets are returned in the order the file writes them: that order is the last
    /// tie-break of the shown epithet.
    ///
    /// # Errors
    /// Returns a source-qualified error for the first invalid entry: an unknown key, a bad value, a scope that is neither
    /// `universal`, a culture subgroup nor a religion (or is both), a trigger that is not `yearly`, `happening:<name>` or `later`,
    /// a missing `when` or `chance` on an epithet that is not `later`, and a `when` or `chance` on a `later` one.
    pub fn parse_epithets(
        &self,
        file: &str,
        text: &str,
        registry: &Registry<'_>,
        religions: &[String],
    ) -> Result<Vec<Epithet>, DataError> {
        let source = Source { file };
        let root = source.document(text)?;
        only_keys(&source, &root, "", &["epithet"])?;
        let table = source.table(required(&source, &root, "", "epithet")?, "epithet")?;
        let order = epithet_order(&source, text, table)?;
        let parser = Parser {
            source: Source { file },
            registry,
            catalog: self.catalog.clone(),
            skills: self.skills.list.clone(),
            counters: self.deeds.iter().map(|d| d.counter.clone()).collect(),
        };
        let mut out = Vec::with_capacity(order.len());
        for key in order {
            let entry = format!("epithet.{key}");
            let item = source.table(&table[&key], &entry)?;
            only_keys(
                &source,
                item,
                &entry,
                &[
                    "shown",
                    "importance",
                    "scope",
                    "from",
                    "trigger",
                    "when",
                    "chance",
                ],
            )?;
            let text_of = |name: &str| -> Result<String, DataError> {
                Ok(source
                    .text(
                        required(&source, item, &entry, name)?,
                        &format!("{entry}.{name}"),
                    )?
                    .to_owned())
            };
            let shown = text_of("shown")?;
            if shown.is_empty() {
                return Err(source.error(format!("{entry}.shown"), "the shown text is empty"));
            }
            if let Some(from) = item.get("from") {
                source.text(from, &format!("{entry}.from"))?;
            }
            let importance = small_u32(
                &source,
                required(&source, item, &entry, "importance")?,
                &format!("{entry}.importance"),
            )?;
            let scope = epithet_scope(
                &source,
                registry,
                religions,
                &format!("{entry}.scope"),
                &text_of("scope")?,
            )?;
            let trigger =
                epithet_trigger(&source, &format!("{entry}.trigger"), &text_of("trigger")?)?;
            let (when, chance_ppb) = if trigger == EpithetTrigger::Later {
                for name in ["when", "chance"] {
                    if item.contains_key(name) {
                        return Err(source.error(
                            format!("{entry}.{name}"),
                            "a `later` epithet has no `when` and no `chance`",
                        ));
                    }
                }
                (Conditions::default(), 0)
            } else {
                (
                    parser.conditions(
                        required(&source, item, &entry, "when")?,
                        &format!("{entry}.when"),
                        Vocabulary::Epithet,
                    )?,
                    source.ppb(
                        required(&source, item, &entry, "chance")?,
                        &format!("{entry}.chance"),
                    )?,
                )
            };
            out.push(Epithet {
                key,
                shown,
                importance,
                scope,
                trigger,
                when,
                chance_ppb,
            });
        }
        Ok(out)
    }
}

impl EventRules {
    /// Load `character-events.toml` and `attributes-skills.toml` from `directory` and validate them against the registry.
    ///
    /// # Errors
    /// Returns a source-qualified error for unreadable or invalid data.
    pub fn load(directory: &Path, registry: &Registry<'_>) -> Result<Self, DataError> {
        let read = |name: &str| -> Result<(String, String), DataError> {
            let path = directory.join(name);
            let file = path.display().to_string();
            fs::read_to_string(&path)
                .map(|text| (file.clone(), text))
                .map_err(|error| DataError {
                    file,
                    entry: "<file>".to_owned(),
                    problem: error.to_string(),
                })
        };
        let (events_file, events) = read("character-events.toml")?;
        let (attributes_file, attributes) = read("attributes-skills.toml")?;
        Self::parse(
            &events_file,
            &events,
            &attributes_file,
            &attributes,
            registry,
        )
    }

    /// Parse and validate both files' text; the file names appear in errors.
    ///
    /// # Errors
    /// Returns a source-qualified error for the first invalid entry.
    #[expect(
        clippy::too_many_lines,
        reason = "one pass over both files in the order the sections depend on each other"
    )]
    pub fn parse(
        events_file: &str,
        events_text: &str,
        attributes_file: &str,
        attributes_text: &str,
        registry: &Registry<'_>,
    ) -> Result<Self, DataError> {
        let attributes_source = Source {
            file: attributes_file,
        };
        let root = attributes_source.document(attributes_text)?;
        only_keys(
            &attributes_source,
            &root,
            "",
            &["catalog", "deeds", "craft_rise", "keep_while", "skills"],
        )?;
        let section = |name: &str| -> Result<&Table, DataError> {
            attributes_source.table(required(&attributes_source, &root, "", name)?, name)
        };
        let catalog = parse_catalog(&attributes_source, section("catalog")?)?;
        for name in SLICE_THREE_TRAITS {
            if catalog
                .get(name)
                .is_none_or(|e| !(e.kind == Kind::MindTrait && e.graded))
            {
                return Err(attributes_source.error(
                    format!("catalog.{name}"),
                    "slice 3's trait must be a graded mind trait",
                ));
            }
        }
        for (name, graded) in registry
            .genetics
            .mendelian()
            .keys()
            .map(|n| (n, false))
            .chain(registry.genetics.graded().keys().map(|n| (n, true)))
        {
            match catalog.get(name) {
                Some(found) if found.graded == graded => {}
                Some(_) => {
                    return Err(attributes_source.error(
                        format!("catalog.{name}"),
                        "its graded flag disagrees with genetics/traits.toml",
                    ));
                }
                None => {
                    return Err(attributes_source.error(
                        "catalog",
                        format!("`{name}` of genetics/traits.toml is not a key of the catalog"),
                    ));
                }
            }
        }
        let mut rules = Self {
            catalog,
            ..Self::default()
        };
        rules.culture_group = registry
            .peoples
            .cultures
            .iter()
            .map(|c| (c.id.clone(), c.subgroup.clone()))
            .collect();
        rules.parse_skills(&attributes_source, registry, section("skills")?)?;
        let mut parser = Parser {
            source: Source {
                file: attributes_file,
            },
            registry,
            catalog: rules.catalog.clone(),
            skills: rules.skills.list.clone(),
            counters: BTreeSet::new(),
        };
        rules.parse_deeds(&parser, section("deeds")?)?;
        parser.counters = rules.deeds.iter().map(|d| d.counter.clone()).collect();
        rules.parse_craft_rise(&parser, section("craft_rise")?)?;
        rules.catalog_order =
            catalog_order(&attributes_source, attributes_text, section("catalog")?)?;
        let mut kept = BTreeMap::new();
        for (key, node) in section("keep_while")? {
            let entry = format!("keep_while.{key}");
            parser.key(key, &entry)?;
            if !parser.catalog[key].kind.removable() {
                return Err(parser.error(&entry, format!("`{key}` is not a calling, station or background: only those can be removed by precedence")));
            }
            kept.insert(
                key.clone(),
                parser.conditions(node, &entry, Vocabulary::Keep)?,
            );
        }
        // Precedence scans in the authored catalog order.
        for key in &rules.catalog_order {
            if let Some(conditions) = kept.remove(key) {
                rules.keep_while.push((key.clone(), conditions));
            }
        }
        let events_source = Source { file: events_file };
        let events_root = events_source.document(events_text)?;
        only_keys(&events_source, &events_root, "", &["event"])?;
        parser.source = Source { file: events_file };
        let list = match events_root.get("event") {
            Some(node) => array(&events_source, node, "event")?,
            None => &[],
        };
        let mut seen = BTreeSet::new();
        for (index, node) in list.iter().enumerate() {
            let event = parser.event(node, index)?;
            if !seen.insert(event.id.clone()) {
                return Err(
                    events_source.error(format!("event.{}", event.id), "a duplicate event id")
                );
            }
            rules.events.push(event);
        }
        Ok(rules)
    }

    fn parse_deeds(&mut self, parser: &Parser<'_>, table: &Table) -> Result<(), DataError> {
        for (key, node) in table {
            let entry = format!("deeds.{key}");
            parser.key(key, &entry)?;
            if parser.catalog[key].kind != Kind::Deed {
                return Err(parser.error(&entry, format!("`{key}` is not a deed in the catalog")));
            }
            let item = parser.source.table(node, &entry)?;
            only_keys(&parser.source, item, &entry, &["counter", "thresholds"])?;
            let counter = parser
                .source
                .text(
                    required(&parser.source, item, &entry, "counter")?,
                    &format!("{entry}.counter"),
                )?
                .to_owned();
            let at = format!("{entry}.thresholds");
            let items = array(
                &parser.source,
                required(&parser.source, item, &entry, "thresholds")?,
                &at,
            )?;
            if items.len() != 6 {
                return Err(parser.error(&at, "a deed has six thresholds, for C to SSS"));
            }
            let mut thresholds = [0_u64; 6];
            for (slot, value) in thresholds.iter_mut().zip(items) {
                *slot = positive_u64(&parser.source, value, &at)?;
            }
            if thresholds[0] == 0 || thresholds.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(parser.error(
                    &at,
                    "the thresholds must be strictly increasing from at least 1",
                ));
            }
            self.deeds.push(Deed {
                key: key.clone(),
                counter,
                thresholds,
            });
        }
        Ok(())
    }

    fn parse_craft_rise(&mut self, parser: &Parser<'_>, table: &Table) -> Result<(), DataError> {
        only_keys(&parser.source, table, "craft_rise", &["rise", "skills"])?;
        self.craft_rise.rise_ppb = parser.source.ppb(
            required(&parser.source, table, "craft_rise", "rise")?,
            "craft_rise.rise",
        )?;
        let skills = required(&parser.source, table, "craft_rise", "skills")?;
        for (craft, node) in parser.source.table(skills, "craft_rise.skills")? {
            let entry = format!("craft_rise.skills.{craft}");
            parser.key(craft, &entry)?;
            if parser.catalog[craft].kind != Kind::Craft {
                return Err(
                    parser.error(&entry, format!("`{craft}` is not a craft in the catalog"))
                );
            }
            let skill = parser.source.text(node, &entry)?;
            parser.skill(skill, &entry)?;
            self.craft_rise
                .skills
                .insert(craft.clone(), skill.to_owned());
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one section of the file, read in its own order"
    )]
    fn parse_skills(
        &mut self,
        source: &Source<'_>,
        registry: &Registry<'_>,
        table: &Table,
    ) -> Result<(), DataError> {
        only_keys(
            source,
            table,
            "skills",
            &[
                "list",
                "aptitude",
                "train_from",
                "gain_rate",
                "decay_used",
                "decay_unused",
                "use_spread",
                "use",
                "focus",
                "home_education",
            ],
        )?;
        let list = strings(
            source,
            required(source, table, "skills", "list")?,
            "skills.list",
        )?;
        if list.is_empty() || list.iter().enumerate().any(|(i, s)| list[..i].contains(s)) {
            return Err(source.error(
                "skills.list",
                "the skills are a non-empty list without repeats",
            ));
        }
        let rules = &mut self.skills;
        rules.list = list;
        let skill_names = rules.list.clone();
        let skill = |name: &str, entry: &str| -> Result<(), DataError> {
            if skill_names.iter().any(|s| s == name) {
                Ok(())
            } else {
                Err(source.error(entry, format!("`{name}` is not in `[skills].list`")))
            }
        };
        for (name, node) in source.table(
            required(source, table, "skills", "aptitude")?,
            "skills.aptitude",
        )? {
            let entry = format!("skills.aptitude.{name}");
            skill(name, &entry)?;
            let stat = source.text(node, &entry)?;
            rules.aptitude.insert(
                name.clone(),
                attribute_index(stat)
                    .filter(|index| *index < 9)
                    .ok_or_else(|| source.error(&entry, format!("`{stat}` is not a stat")))?,
            );
        }
        rules.train_from = small_u32(
            source,
            required(source, table, "skills", "train_from")?,
            "skills.train_from",
        )?;
        rules.gain_rate = source.milli_non_negative(
            required(source, table, "skills", "gain_rate")?,
            "skills.gain_rate",
        )?;
        if rules.gain_rate > MAX_GAIN_RATE {
            return Err(source.error(
                "skills.gain_rate",
                "the yearly growth rate is at most 100000",
            ));
        }
        rules.decay_used = i64::from(source.unit_milli(
            required(source, table, "skills", "decay_used")?,
            "skills.decay_used",
        )?);
        rules.decay_unused = i64::from(source.unit_milli(
            required(source, table, "skills", "decay_unused")?,
            "skills.decay_unused",
        )?);
        rules.use_spread = source.milli_non_negative(
            required(source, table, "skills", "use_spread")?,
            "skills.use_spread",
        )?;
        for (pop, node) in source.table(required(source, table, "skills", "use")?, "skills.use")? {
            let entry = format!("skills.use.{pop}");
            if !registry.pop_types.contains(&pop.as_str()) {
                return Err(source.error(&entry, format!("`{pop}` is not a pop type")));
            }
            for (sex, profile) in source.table(node, &entry)? {
                let at = format!("{entry}.{sex}");
                let sex = match sex.as_str() {
                    "m" => 'm',
                    "f" => 'f',
                    _ => return Err(source.error(&at, "a profile is for `m` or `f`")),
                };
                let mut values = BTreeMap::new();
                for (name, value) in source.table(profile, &at)? {
                    skill(name, &format!("{at}.{name}"))?;
                    values.insert(
                        name.clone(),
                        i64::from(source.unit_milli(value, &format!("{at}.{name}"))?),
                    );
                }
                rules.profiles.insert((pop.clone(), sex), values);
            }
        }
        let focus = source.table(required(source, table, "skills", "focus")?, "skills.focus")?;
        for (key, node) in focus {
            let entry = format!("skills.focus.{key}");
            match key.as_str() {
                "on" | "off" => {
                    let value = source.milli_non_negative(node, &entry)?;
                    if value > MAX_FOCUS {
                        return Err(source.error(&entry, "a focus multiplier is at most 10"));
                    }
                    if key == "on" {
                        rules.focus_on = value;
                    } else {
                        rules.focus_off = value;
                    }
                }
                "odds" => {
                    for (name, odds) in source.table(node, &entry)? {
                        let at = format!("{entry}.{name}");
                        let (pop, sex) = name
                            .rsplit_once('_')
                            .and_then(|(pop, sex)| match sex {
                                "m" => Some((pop, 'm')),
                                "f" => Some((pop, 'f')),
                                _ => None,
                            })
                            .filter(|(pop, _)| registry.pop_types.contains(pop))
                            .ok_or_else(|| {
                                source.error(&at, "odds are keyed `<pop type>_m` or `<pop type>_f`")
                            })?;
                        let mut weights = Vec::new();
                        for (focus_name, weight) in source.table(odds, &at)? {
                            weights.push((focus_name.clone(), small_u32(source, weight, &at)?));
                        }
                        if weights.iter().map(|(_, w)| u64::from(*w)).sum::<u64>() == 0 {
                            return Err(source.error(&at, "the odds are all zero"));
                        }
                        rules.focus_odds.insert((pop.to_owned(), sex), weights);
                    }
                }
                _ => {
                    let mut names = Vec::new();
                    for name in strings(source, node, &entry)? {
                        skill(&name, &entry)?;
                        names.push(name);
                    }
                    rules.focus_skills.insert(key.clone(), names);
                }
            }
        }
        rules.focus_skills.insert("balanced".to_owned(), Vec::new());
        for ((pop, sex), weights) in &rules.focus_odds {
            for (name, _) in weights {
                if !rules.focus_skills.contains_key(name) {
                    return Err(source.error(
                        format!("skills.focus.odds.{pop}_{sex}.{name}"),
                        "not a focus",
                    ));
                }
            }
        }
        let education = source.table(
            required(source, table, "skills", "home_education")?,
            "skills.home_education",
        )?;
        only_keys(
            source,
            education,
            "skills.home_education",
            &["rate", "age_factor", "scholarship_per_point"],
        )?;
        rules.education_rate = source.milli_non_negative(
            required(source, education, "skills.home_education", "rate")?,
            "skills.home_education.rate",
        )?;
        rules.scholarship_per_point = source.milli_non_negative(
            required(
                source,
                education,
                "skills.home_education",
                "scholarship_per_point",
            )?,
            "skills.home_education.scholarship_per_point",
        )?;
        if rules.education_rate > MAX_UNIT || rules.scholarship_per_point > MAX_UNIT {
            return Err(source.error(
                "skills.home_education",
                "the rate and the scholarship speed-up are at most 1",
            ));
        }
        let at = "skills.home_education.age_factor";
        for item in array(
            source,
            required(source, education, "skills.home_education", "age_factor")?,
            at,
        )? {
            let [age, factor] = array(source, item, at)? else {
                return Err(source.error(at, "an entry is [age, factor]"));
            };
            rules.age_factor.push((
                small_u32(source, age, at)?,
                source.milli_non_negative(factor, at)?,
            ));
        }
        if rules
            .age_factor
            .iter()
            .any(|(_, factor)| *factor > MAX_UNIT)
        {
            return Err(source.error(at, "an age factor is at most 1"));
        }
        if rules.age_factor.first().is_none_or(|(age, _)| *age != 0)
            || rules
                .age_factor
                .windows(2)
                .any(|pair| pair[0].0 >= pair[1].0)
        {
            return Err(source.error(at, "the ages start at 0 and increase"));
        }
        Ok(())
    }

    /// The deed whose grade follows `counter` for `key`, if any.
    #[must_use]
    pub fn deed(&self, key: &str) -> Option<&Deed> {
        self.deeds.iter().find(|deed| deed.key == key)
    }

    /// The index of a skill in [`SkillRules::list`].
    #[must_use]
    pub fn skill_index(&self, name: &str) -> Option<usize> {
        self.skills.list.iter().position(|skill| skill == name)
    }
}
