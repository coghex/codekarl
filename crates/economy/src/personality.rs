//! Personality data: the vice/virtue and manner pairs, birth, humour drift, stress, standing and opinion (CK-362).
//!
//! Nothing here uses floating point. Every number is an exact integer in thousandths (a decimal is parsed from its digits,
//! never through a binary float); grade terms are whole opinion points times 1000.
#![deny(clippy::float_arithmetic)]

use crate::{
    DataError,
    genetics::{HUMOURS, Node, Source, Table},
};
use std::{collections::BTreeMap, fs, path::Path};

/// The seven vice/virtue pairs, the sin first, in the order of every vice/virtue array.
pub const VICE_PAIRS: [&str; 7] = [
    "pride_humility",
    "greed_charity",
    "lust_chastity",
    "envy_kindness",
    "gluttony_temperance",
    "wrath_patience",
    "sloth_diligence",
];
/// The nine manner pairs, in the order of every manner array (the data order, used for ties).
pub const MANNER_PAIRS: [&str; 9] = [
    "guarded_open",
    "deceitful_honest",
    "suspicious_trusting",
    "wary_daring",
    "traditional_curious",
    "yielding_stubborn",
    "solitary_gregarious",
    "content_ambitious",
    "harsh_merciful",
];
/// Vice/virtue pairs plus manner pairs: the order of the culture mean arrays.
pub const PAIR_COUNT: usize = 16;
/// The hard bound of a vice/virtue value, in thousandths.
pub const VICE_BOUND: i64 = 7_000;
/// The highest stress, in thousandths.
pub const STRESS_MAX: i64 = 100_000;

/// Stress parameters (`[stress]`), all in thousandths.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StressRules {
    /// Share of stress shed a month.
    pub monthly_recovery: i64,
    /// Recovery factor per point of blood above 5.
    pub recovery_per_blood: i64,
    /// Recovery factor per point of black bile above 5.
    pub recovery_per_black: i64,
    /// Recovery factor per melancholy grade.
    pub recovery_per_melancholy_grade: i64,
    /// Intake factor per point of phlegm above 5.
    pub intake_per_phlegm: i64,
    /// Intake factor per point of temperance.
    pub intake_per_temperance: i64,
    /// Insult and defeat intake factor per point of yellow bile above 5.
    pub insult_per_yellow: i64,
    /// Performance lost per point of stress over the threshold.
    pub penalty_per_point: i64,
    /// The lowest performance.
    pub penalty_floor: i64,
    /// Event multiplier per point of stress over the threshold.
    pub events_per_point: i64,
    /// Stress relieved by indulging a vice.
    pub indulge_relief: i64,
    /// Movement of the pair toward its sin when indulging.
    pub indulge_step: i64,
    /// Stress relieved by confession.
    pub confess_relief: i64,
    /// Movement of the most sinful pair toward its virtue when confessing.
    pub confess_step: i64,
}

/// Opinion parameters (`[opinion]`), in thousandths of a point (grade terms are whole points times 1000).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpinionRules {
    /// The most (and, negated, least) the manner compatibility term can be.
    pub compat_manner_max: i64,
    /// The most (and, negated, least) the humour compatibility term can be.
    pub compat_humour_max: i64,
    /// Weight of the other's honour.
    pub honour_weight: i64,
    /// Weight of the other's fame.
    pub fame_weight: i64,
    /// Weight of the other's popularity.
    pub popularity_weight: i64,
    /// The three standing terms together are held to plus or minus this.
    pub standing_cap: i64,
    /// Beauty term by level 1..=6.
    pub beauty: [i64; 6],
    /// Ill-favoured term by level 1..=6.
    pub ill_favoured: [i64; 6],
    /// Fool term by level 1..=6.
    pub fool: [i64; 6],
    /// Fool term per power level, by fool level 1..=6.
    pub fool_in_power: [i64; 6],
    /// Drunkard term by level 1..=6.
    pub drunkard: [i64; 6],
    /// Added when both are drunkards, by the lower level 1..=6.
    pub drunkard_kinship: [i64; 6],
    /// Rage fits term by level 1..=6.
    pub rage_fits: [i64; 6],
    /// Madness term by level 1..=6.
    pub madness: [i64; 6],
}

/// The loaded personality data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Personality {
    /// Birth spread of a vice/virtue pair.
    pub vice_spread: i64,
    /// Birth spread of a manner pair.
    pub manner_spread: i64,
    /// The manner display range, low and high; the stored value is not bounded by it.
    pub manner_range: [i64; 2],
    /// Weight of the parents' average at birth, 0..=1000.
    pub parents_weight: i64,
    /// A pair's yearly drift per point of its humour away from 5.
    pub drift_per_point: i64,
    /// The humour (an index into [`HUMOURS`]) that pulls each vice/virtue pair, if any.
    pub drift_humour: [Option<usize>; 7],
    /// Stress parameters.
    pub stress: StressRules,
    /// Share of each standing value kept at the yearly fade, 0..=1000.
    pub yearly_keep: i64,
    /// Opinion parameters.
    pub opinion: OpinionRules,
    /// Culture means by culture, per pair in vice-then-manner order, in thousandths.
    pub culture_means: BTreeMap<String, [i64; PAIR_COUNT]>,
}

fn keys(
    source: &Source<'_>,
    table: &Table,
    section: &str,
    allowed: &[&str],
) -> Result<(), DataError> {
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(source.error(format!("{section}.{key}"), "unknown key"));
        }
    }
    for key in allowed {
        if !table.contains_key(*key) {
            return Err(source.error(format!("{section}.{key}"), "missing value"));
        }
    }
    Ok(())
}

fn items<'t>(source: &Source<'_>, node: &'t Node, entry: &str) -> Result<&'t [Node], DataError> {
    match node {
        Node::Array(items) => Ok(items),
        _ => Err(source.error(entry, "expected an array")),
    }
}

fn roster(
    source: &Source<'_>,
    node: &Node,
    entry: &str,
    expected: &[&str],
) -> Result<(), DataError> {
    let found: Vec<&str> = items(source, node, entry)?
        .iter()
        .map(|item| source.text(item, entry))
        .collect::<Result<_, _>>()?;
    if found == expected {
        return Ok(());
    }
    let repeated = found
        .iter()
        .enumerate()
        .any(|(index, name)| found[..index].contains(name));
    let problem = if let Some(extra) = found.iter().find(|name| !expected.contains(name)) {
        format!("`{extra}` is not one of the pairs")
    } else if repeated {
        "a pair is repeated".to_owned()
    } else if let Some(missing) = expected.iter().find(|name| !found.contains(name)) {
        format!("the pair `{missing}` is missing")
    } else {
        "the pairs are not in the required order".to_owned()
    };
    Err(source.error(entry, problem))
}

fn range(source: &Source<'_>, node: &Node, entry: &str) -> Result<[i64; 2], DataError> {
    match items(source, node, entry)? {
        [low, high] => Ok([source.milli(low, entry)?, source.milli(high, entry)?]),
        _ => Err(source.error(entry, "a range has exactly two numbers")),
    }
}

fn grades(source: &Source<'_>, node: &Node, entry: &str) -> Result<[i64; 6], DataError> {
    let list = items(source, node, entry)?;
    if list.len() != 6 {
        return Err(source.error(entry, "a grade list has exactly six entries"));
    }
    let mut out = [0_i64; 6];
    for (slot, item) in out.iter_mut().zip(list) {
        *slot = i64::try_from(
            source
                .integer(item, entry)?
                .checked_mul(1000)
                .ok_or_else(|| source.error(entry, "the value is too large"))?,
        )
        .map_err(|_| source.error(entry, "the value is too large"))?;
    }
    Ok(out)
}

fn unit(source: &Source<'_>, node: &Node, entry: &str) -> Result<i64, DataError> {
    Ok(i64::from(source.unit_milli(node, entry)?))
}

impl Personality {
    /// Load `personality.toml` and validate it against the culture registry.
    ///
    /// # Errors
    /// Returns a source-qualified error for unreadable or invalid data.
    pub fn load(path: &Path, cultures: &[&str]) -> Result<Self, DataError> {
        let file = path.display().to_string();
        let text = fs::read_to_string(path).map_err(|error| DataError {
            file: file.clone(),
            entry: "<file>".to_owned(),
            problem: error.to_string(),
        })?;
        Self::parse(&file, &text, cultures)
    }

    /// Parse and validate the file's text; `file` names it in errors.
    ///
    /// # Errors
    /// Returns a source-qualified error for the first invalid entry.
    pub fn parse(file: &str, text: &str, cultures: &[&str]) -> Result<Self, DataError> {
        let source = Source { file };
        let root = source.document(text)?;
        keys(
            &source,
            &root,
            "",
            &[
                "vice_virtue",
                "manner",
                "birth",
                "humour_drift",
                "stress",
                "standing",
                "opinion",
                "culture_means",
            ],
        )
        .map_err(|error| DataError {
            entry: error.entry.trim_start_matches('.').to_owned(),
            ..error
        })?;
        let section = |name: &str| source.table(&root[name], name);
        let mut rules = Self::default();
        let vice = section("vice_virtue")?;
        keys(&source, vice, "vice_virtue", &["range", "spread", "pairs"])?;
        if range(&source, &vice["range"], "vice_virtue.range")? != [-VICE_BOUND, VICE_BOUND] {
            return Err(source.error("vice_virtue.range", "the range must be exactly [-7, 7]"));
        }
        rules.vice_spread = source.milli_non_negative(&vice["spread"], "vice_virtue.spread")?;
        roster(&source, &vice["pairs"], "vice_virtue.pairs", &VICE_PAIRS)?;
        let manner = section("manner")?;
        keys(&source, manner, "manner", &["range", "spread", "pairs"])?;
        rules.manner_range = range(&source, &manner["range"], "manner.range")?;
        if rules.manner_range[0] >= rules.manner_range[1] {
            return Err(source.error("manner.range", "the low end must be below the high end"));
        }
        rules.manner_spread = source.milli_non_negative(&manner["spread"], "manner.spread")?;
        roster(&source, &manner["pairs"], "manner.pairs", &MANNER_PAIRS)?;
        let birth = section("birth")?;
        keys(&source, birth, "birth", &["parents_weight"])?;
        rules.parents_weight = unit(&source, &birth["parents_weight"], "birth.parents_weight")?;
        rules.parse_drift(&source, section("humour_drift")?)?;
        rules.parse_stress(&source, section("stress")?)?;
        let standing = section("standing")?;
        keys(&source, standing, "standing", &["yearly_keep"])?;
        rules.yearly_keep = unit(&source, &standing["yearly_keep"], "standing.yearly_keep")?;
        rules.parse_opinion(&source, section("opinion")?)?;
        rules.parse_cultures(&source, section("culture_means")?, cultures)?;
        Ok(rules)
    }

    fn parse_drift(&mut self, source: &Source<'_>, table: &Table) -> Result<(), DataError> {
        for key in table.keys() {
            if key != "per_point" && !HUMOURS.contains(&key.as_str()) {
                return Err(source.error(
                    format!("humour_drift.{key}"),
                    "a drift key is `per_point` or one of the four humours",
                ));
            }
        }
        let per_point = table
            .get("per_point")
            .ok_or_else(|| source.error("humour_drift.per_point", "missing value"))?;
        self.drift_per_point = source.milli_non_negative(per_point, "humour_drift.per_point")?;
        for (humour, name) in HUMOURS.iter().enumerate() {
            let Some(node) = table.get(*name) else {
                continue;
            };
            let entry = format!("humour_drift.{name}");
            for item in items(source, node, &entry)? {
                let pair = source.text(item, &entry)?;
                let Some(index) = VICE_PAIRS.iter().position(|known| *known == pair) else {
                    return Err(source.error(&entry, format!("`{pair}` is not a vice/virtue pair")));
                };
                if self.drift_humour[index].is_some() {
                    return Err(source.error(
                        &entry,
                        format!("the pair `{pair}` already drifts with a humour"),
                    ));
                }
                self.drift_humour[index] = Some(humour);
            }
        }
        Ok(())
    }

    fn parse_stress(&mut self, source: &Source<'_>, table: &Table) -> Result<(), DataError> {
        keys(
            source,
            table,
            "stress",
            &[
                "range",
                "monthly_recovery",
                "recovery_per_blood",
                "recovery_per_black",
                "recovery_per_melancholy_grade",
                "intake_per_phlegm",
                "intake_per_temperance",
                "insult_per_yellow",
                "penalty_per_point",
                "penalty_floor",
                "events_per_point",
                "indulge_relief",
                "indulge_step",
                "confess_relief",
                "confess_step",
            ],
        )?;
        if range(source, &table["range"], "stress.range")? != [0, STRESS_MAX] {
            return Err(source.error("stress.range", "the range must be exactly [0, 100]"));
        }
        let signed = |key: &str| source.milli(&table[key], &format!("stress.{key}"));
        let plain = |key: &str| source.milli_non_negative(&table[key], &format!("stress.{key}"));
        let share = |key: &str| unit(source, &table[key], &format!("stress.{key}"));
        self.stress = StressRules {
            monthly_recovery: share("monthly_recovery")?,
            recovery_per_blood: signed("recovery_per_blood")?,
            recovery_per_black: signed("recovery_per_black")?,
            recovery_per_melancholy_grade: signed("recovery_per_melancholy_grade")?,
            intake_per_phlegm: signed("intake_per_phlegm")?,
            intake_per_temperance: signed("intake_per_temperance")?,
            insult_per_yellow: signed("insult_per_yellow")?,
            penalty_per_point: plain("penalty_per_point")?,
            penalty_floor: share("penalty_floor")?,
            events_per_point: plain("events_per_point")?,
            indulge_relief: plain("indulge_relief")?,
            indulge_step: plain("indulge_step")?,
            confess_relief: plain("confess_relief")?,
            confess_step: plain("confess_step")?,
        };
        Ok(())
    }

    fn parse_opinion(&mut self, source: &Source<'_>, table: &Table) -> Result<(), DataError> {
        keys(
            source,
            table,
            "opinion",
            &[
                "compat_manner_max",
                "compat_humour_max",
                "honour_weight",
                "fame_weight",
                "popularity_weight",
                "standing_cap",
                "beauty",
                "ill_favoured",
                "fool",
                "fool_in_power",
                "drunkard",
                "drunkard_kinship",
                "rage_fits",
                "madness",
            ],
        )?;
        let signed = |key: &str| source.milli(&table[key], &format!("opinion.{key}"));
        let plain = |key: &str| source.milli_non_negative(&table[key], &format!("opinion.{key}"));
        let list = |key: &str| grades(source, &table[key], &format!("opinion.{key}"));
        self.opinion = OpinionRules {
            compat_manner_max: plain("compat_manner_max")?,
            compat_humour_max: plain("compat_humour_max")?,
            honour_weight: signed("honour_weight")?,
            fame_weight: signed("fame_weight")?,
            popularity_weight: signed("popularity_weight")?,
            standing_cap: plain("standing_cap")?,
            beauty: list("beauty")?,
            ill_favoured: list("ill_favoured")?,
            fool: list("fool")?,
            fool_in_power: list("fool_in_power")?,
            drunkard: list("drunkard")?,
            drunkard_kinship: list("drunkard_kinship")?,
            rage_fits: list("rage_fits")?,
            madness: list("madness")?,
        };
        Ok(())
    }

    fn parse_cultures(
        &mut self,
        source: &Source<'_>,
        table: &Table,
        cultures: &[&str],
    ) -> Result<(), DataError> {
        for culture in cultures {
            if !table.contains_key(*culture) {
                return Err(source.error(
                    format!("culture_means.{culture}"),
                    "the registry's culture is missing",
                ));
            }
        }
        for (culture, node) in table {
            let entry = format!("culture_means.{culture}");
            if !cultures.contains(&culture.as_str()) {
                return Err(source.error(&entry, "not a culture of the registry"));
            }
            let mut means = [0_i64; PAIR_COUNT];
            for (pair, value) in source.table(node, &entry)? {
                let at = format!("{entry}.{pair}");
                let index = VICE_PAIRS
                    .iter()
                    .chain(&MANNER_PAIRS)
                    .position(|known| *known == pair)
                    .ok_or_else(|| source.error(&at, "not a known pair"))?;
                means[index] = source.milli(value, &at)?;
            }
            self.culture_means.insert(culture.clone(), means);
        }
        Ok(())
    }
}
