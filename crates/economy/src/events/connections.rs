//! Connections and passions data (CK-368, characters slice 6): `connections.toml`.
//!
//! Nothing here uses floating point. A decimal is read from its digits and kept as an exact integer: a probability, a factor or a
//! per-point weight in parts per billion, an opinion weight in thousandths of a point. The inert entries (`live = false` moments
//! and the settings of systems that do not exist yet) are admitted with only their shape checked and never act.
#![deny(clippy::float_arithmetic)]

use super::{Conditions, Parser, Registry, Vocabulary, only_keys, required, small_u32};
use crate::{
    DataError, EventRules,
    genetics::{Node, PPB, Source, Table},
};
use std::collections::BTreeMap;

/// The three mutual bond kinds, in the order the engine rolls them.
pub const BOND_KINDS: [&str; 3] = ["friend", "rival", "lover"];
/// The five one-way feeling kinds, in the order the engine rolls them.
pub const FEELING_KINDS: [&str; 5] = ["love", "respect", "envy", "fear", "hatred"];
/// Every strand kind: the bonds, then the feelings.
pub const ALL_KINDS: [&str; 8] = [
    "friend", "rival", "lover", "love", "respect", "envy", "fear", "hatred",
];
/// The passion kinds.
pub const PASSION_KINDS: [&str; 4] = ["loyalty", "love", "hatred", "fear"];
/// The contact settings of the data.
pub const SETTINGS: [&str; 8] = [
    "household",
    "monastery",
    "court",
    "same_location",
    "council",
    "army",
    "hostage",
    "foster_kin",
];
/// The orientations, in the order of [`Orientation::weights`].
pub const ORIENTATIONS: [&str; 4] = ["straight", "gay", "bisexual", "asexual"];
/// The most an orientation weight may be (so that a draw over the weights times a boost fits 64 bits).
const MOST_WEIGHT: u32 = 1_000_000;
/// The most a factor may be, in whole units.
const MOST_FACTOR: u64 = 1000;

/// One contact setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Setting {
    /// The contact it gives, in parts per billion.
    pub contact_ppb: u32,
    /// Whether the setting acts (an inert setting gives 0).
    pub live: bool,
}

/// `[circle]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Circle {
    /// Partners below this contact are outside the circle, in ppb.
    pub floor_ppb: u32,
    /// The most partners in a circle.
    pub size: u32,
    /// No strands before this age.
    pub min_age: u32,
    /// The age a lover strand needs.
    pub lover_min_age: u32,
}

/// `[orientation]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Orientation {
    /// The weights of [`ORIENTATIONS`], whole numbers.
    pub weights: [u32; 4],
    /// The factor on gay and bisexual for a child of a gay or bisexual parent, in ppb.
    pub parent_boost_ppb: u64,
}

/// A rule that makes strands from a happening.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HappeningRule {
    /// The kinds it rolls, each separately.
    pub kinds: Vec<String>,
    /// The chance of each, in ppb.
    pub chance_ppb: u32,
}

/// The factors of the strand chances. Every value is in parts per billion unless it is a divisor or a count.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StrandRules {
    /// The base chance of each kind, in ppb.
    pub base_ppb: BTreeMap<String, u32>,
    /// A pair or feeling whose contact is below this loses a point a year, in ppb.
    pub fade_below_ppb: u32,
    /// A rivalry at this strength or more can make hatred.
    pub rival_at: u32,
    /// The yearly chance of that hatred, in ppb.
    pub hatred_chance_ppb: u32,
    /// `[strands.friend]`: per point of the pair's mean `solitary_gregarious`.
    pub friend_gregarious: i64,
    /// Per point of the pair's mean `envy_kindness`.
    pub friend_kindness: i64,
    /// The weight of how alike the humours are.
    pub friend_humours_alike: i64,
    /// The factor per melancholy or recluse grade, for each character.
    pub friend_withdrawn: [u64; 7],
    /// The factor for two drunkards, by the lower grade.
    pub friend_drunkards: [u64; 7],
    /// `age_gap_years` of friendship.
    pub friend_age_gap: u32,
    /// `[strands.rival]`: both hold the same power level above 0.
    pub rival_same_power: u64,
    /// Per point of the pair's mean `content_ambitious`.
    pub rival_ambition: i64,
    /// Per point of the pair's mean `envy_kindness`.
    pub rival_envy: i64,
    /// A parent and child.
    pub rival_parent_child: u64,
    /// `age_gap_years` of rivalry.
    pub rival_age_gap: u32,
    /// `[strands.lover]`: per point of the pair's mean `lust_chastity`.
    pub lover_lust: i64,
    /// The factor per beauty grade, for each character.
    pub lover_beauty: [u64; 7],
    /// The factor per `ill_favoured` grade, for each character.
    pub lover_ill_favoured: [u64; 7],
    /// Either is married to someone else.
    pub lover_married_elsewhere: u64,
    /// `age_gap_years` of love between lovers.
    pub lover_age_gap: u32,
    /// `[strands.love]`: a spouse.
    pub love_spouse: u64,
    /// A parent or child.
    pub love_parent_child: u64,
    /// The factor per beauty grade when `a` is attracted to `b` and not kin.
    pub love_beauty: [u64; 7],
    /// `[strands.respect]`: per point of fame.
    pub respect_fame_per: i64,
    /// The divisor of the sum of skills.
    pub respect_skills_divisor: u32,
    /// Per level of power.
    pub respect_power_per: i64,
    /// `[strands.envy]`: the divisor of the negative `envy_kindness`.
    pub envy_divisor: u32,
    /// The floor added to it.
    pub envy_floor: i64,
    /// Per beauty grade of the other.
    pub envy_beauty_per_grade: i64,
    /// The factor when the other has more power.
    pub envy_more_power: u64,
    /// `[strands.fear]`: per `rage_fits` grade of the other.
    pub fear_rage_per_grade: i64,
    /// The factor for an other who is `ill_favoured` S or above.
    pub fear_grim_face: u64,
    /// Per point of the feeler's `wary_daring`.
    pub fear_daring_per_point: i64,
    /// `[strands.hatred]`: per point of `wrath_patience`.
    pub hatred_patience: i64,
    /// Per point of `envy_kindness`.
    pub hatred_kindness: i64,
    /// `[strands.happenings]` by happening name (`battle_comrades`, `rescue`, `insult`, `kin_killed`, `duel`).
    pub happenings: BTreeMap<String, HappeningRule>,
}

/// What starts a passion moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MomentTrigger {
    /// The yearly birthday pass.
    Yearly,
    /// A named happening.
    Happening(String),
}

/// The target of a passion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The house of the killer (`kin_killed`).
    KillerHouse,
    /// The character's location.
    Location,
    /// The character's religion.
    Religion,
    /// The character's house.
    House,
    /// The character's culture.
    Culture,
    /// A target of a system that does not exist yet (an inert moment's).
    Deferred(String),
}

/// One passion moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moment {
    /// The key, `[passions.moments.<key>]`.
    pub key: String,
    /// What starts it.
    pub trigger: MomentTrigger,
    /// The conditions (empty for an inert moment).
    pub when: Conditions,
    /// The passion's kind.
    pub kind: String,
    /// The passion's target.
    pub target: Target,
    /// The chance before the tier factor, in ppb.
    pub chance_ppb: u32,
    /// Whether the moment acts.
    pub live: bool,
}

/// `[passions]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassionRules {
    /// The tier factor of a full-tier character, in ppb.
    pub full_ppb: u32,
    /// The tier factor of a coarse-tier character, in ppb.
    pub coarse_ppb: u32,
    /// A new passion's strength.
    pub start_strength: u32,
    /// Years a passion lasts without a renewing moment per point.
    pub fade_years: u32,
    /// The moments, by key.
    pub moments: Vec<Moment>,
}

/// The log lines of one strand kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StrandText {
    /// When it is made.
    pub made: String,
    /// On reaching 5.
    pub at_5: String,
    /// On reaching 10.
    pub at_10: String,
    /// When it ends.
    pub ended: String,
}

/// The loaded `connections.toml`. [`Connections::default`] is the data of a world without it: nothing is loaded and no strand acts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Connections {
    /// Whether a file was loaded.
    pub loaded: bool,
    /// `[contact.settings]`.
    pub settings: BTreeMap<String, Setting>,
    /// `[contact] extra_setting`, in ppb.
    pub extra_setting_ppb: u32,
    /// `[circle]`.
    pub circle: Circle,
    /// `[strands]`.
    pub strands: StrandRules,
    /// `[orientation]`.
    pub orientation: Orientation,
    /// `[passions]`.
    pub passions: PassionRules,
    /// `[settling_in] max_years`.
    pub max_years: u32,
    /// `[burn_in] years`.
    pub burn_in_years: u32,
    /// `[opinion.strands]`: thousandths of a point per point of strength, by kind.
    pub opinion_strands: BTreeMap<String, i64>,
    /// `[opinion.passions]`: thousandths of a point per point of strength, by passion kind.
    pub opinion_passions: BTreeMap<String, i64>,
    /// `[log.<kind>]`.
    pub log: BTreeMap<String, StrandText>,
    /// `[log.passion] gained_<kind>`.
    pub passion_gained: BTreeMap<String, String>,
    /// `[log.passion] at_10`.
    pub passion_at_10: String,
    /// `[log.passion] faded`.
    pub passion_faded: String,
}

/// A table that holds exactly the `known` keys.
fn exact(source: &Source<'_>, table: &Table, entry: &str, known: &[&str]) -> Result<(), DataError> {
    only_keys(source, table, entry, known)?;
    for key in known {
        if !table.contains_key(*key) {
            return Err(source.error(format!("{entry}.{key}"), "missing value"));
        }
    }
    Ok(())
}

fn section<'t>(
    source: &Source<'_>,
    table: &'t Table,
    entry: &str,
    key: &str,
) -> Result<&'t Table, DataError> {
    source.table(
        required(source, table, entry, key)?,
        &format!("{entry}.{key}"),
    )
}

/// A non-negative factor in ppb, at most 1000.
fn factor(source: &Source<'_>, node: &Node, entry: &str) -> Result<u64, DataError> {
    u64::try_from(source.scaled(node, entry, 9)?)
        .ok()
        .filter(|value| *value <= MOST_FACTOR * u64::from(PPB))
        .ok_or_else(|| source.error(entry, "a factor lies in 0..1000"))
}

/// A signed per-point weight in ppb, between -1000 and 1000.
fn signed(source: &Source<'_>, node: &Node, entry: &str) -> Result<i64, DataError> {
    i64::try_from(source.scaled(node, entry, 9)?)
        .ok()
        .filter(|value| value.unsigned_abs() <= MOST_FACTOR * u64::from(PPB))
        .ok_or_else(|| source.error(entry, "a weight lies in -1000..1000"))
}

/// A divisor or an age gap: a positive whole number.
fn divisor(source: &Source<'_>, node: &Node, entry: &str) -> Result<u32, DataError> {
    match small_u32(source, node, entry)? {
        0 => Err(source.error(entry, "the value must be a positive whole number")),
        value => Ok(value),
    }
}

fn grades(source: &Source<'_>, node: &Node, entry: &str) -> Result<[u64; 7], DataError> {
    let Node::Array(items) = node else {
        return Err(source.error(entry, "expected an array"));
    };
    let values = items
        .iter()
        .map(|item| factor(source, item, entry))
        .collect::<Result<Vec<_>, _>>()?;
    <[u64; 7]>::try_from(values).map_err(|_| {
        source.error(
            entry,
            "a grade table has seven values (none, C, B, A, S, SS, SSS)",
        )
    })
}

fn text(source: &Source<'_>, node: &Node, entry: &str) -> Result<String, DataError> {
    Ok(source.text(node, entry)?.to_owned())
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per strand kind and factor of the data, in the file's own order"
)]
fn parse_strands(source: &Source<'_>, table: &Table) -> Result<StrandRules, DataError> {
    exact(
        source,
        table,
        "strands",
        &[
            "base",
            "fade_below",
            "hatred_from_rivalry",
            "friend",
            "rival",
            "lover",
            "love",
            "respect",
            "envy",
            "fear",
            "hatred",
            "happenings",
        ],
    )?;
    let mut rules = StrandRules::default();
    let base = source.table(&table["base"], "strands.base")?;
    exact(source, base, "strands.base", &ALL_KINDS)?;
    for (kind, node) in base {
        let entry = format!("strands.base.{kind}");
        rules
            .base_ppb
            .insert(kind.clone(), source.ppb(node, &entry)?);
    }
    rules.fade_below_ppb = source.ppb(&table["fade_below"], "strands.fade_below")?;
    let hatred = section(source, table, "strands", "hatred_from_rivalry")?;
    exact(
        source,
        hatred,
        "strands.hatred_from_rivalry",
        &["rival_at", "chance"],
    )?;
    rules.rival_at = small_u32(
        source,
        &hatred["rival_at"],
        "strands.hatred_from_rivalry.rival_at",
    )?;
    rules.hatred_chance_ppb =
        source.ppb(&hatred["chance"], "strands.hatred_from_rivalry.chance")?;
    let kind_table = |name: &str, keys: &[&str]| -> Result<&Table, DataError> {
        let entry = format!("strands.{name}");
        let found = section(source, table, "strands", name)?;
        exact(source, found, &entry, keys)?;
        Ok(found)
    };
    let at = |name: &str, key: &str| format!("strands.{name}.{key}");
    let friend = kind_table(
        "friend",
        &[
            "gregarious_per_point",
            "kindness_per_point",
            "humours_alike",
            "withdrawn",
            "drunkards",
            "age_gap_years",
        ],
    )?;
    rules.friend_gregarious = signed(
        source,
        &friend["gregarious_per_point"],
        &at("friend", "gregarious_per_point"),
    )?;
    rules.friend_kindness = signed(
        source,
        &friend["kindness_per_point"],
        &at("friend", "kindness_per_point"),
    )?;
    rules.friend_humours_alike = signed(
        source,
        &friend["humours_alike"],
        &at("friend", "humours_alike"),
    )?;
    rules.friend_withdrawn = grades(source, &friend["withdrawn"], &at("friend", "withdrawn"))?;
    rules.friend_drunkards = grades(source, &friend["drunkards"], &at("friend", "drunkards"))?;
    rules.friend_age_gap = divisor(
        source,
        &friend["age_gap_years"],
        &at("friend", "age_gap_years"),
    )?;
    let rival = kind_table(
        "rival",
        &[
            "same_power",
            "ambition_per_point",
            "envy_per_point",
            "parent_child",
            "age_gap_years",
        ],
    )?;
    rules.rival_same_power = factor(source, &rival["same_power"], &at("rival", "same_power"))?;
    rules.rival_ambition = signed(
        source,
        &rival["ambition_per_point"],
        &at("rival", "ambition_per_point"),
    )?;
    rules.rival_envy = signed(
        source,
        &rival["envy_per_point"],
        &at("rival", "envy_per_point"),
    )?;
    rules.rival_parent_child =
        factor(source, &rival["parent_child"], &at("rival", "parent_child"))?;
    rules.rival_age_gap = divisor(
        source,
        &rival["age_gap_years"],
        &at("rival", "age_gap_years"),
    )?;
    let lover = kind_table(
        "lover",
        &[
            "lust_per_point",
            "beauty",
            "ill_favoured",
            "married_elsewhere",
            "age_gap_years",
        ],
    )?;
    rules.lover_lust = signed(
        source,
        &lover["lust_per_point"],
        &at("lover", "lust_per_point"),
    )?;
    rules.lover_beauty = grades(source, &lover["beauty"], &at("lover", "beauty"))?;
    rules.lover_ill_favoured =
        grades(source, &lover["ill_favoured"], &at("lover", "ill_favoured"))?;
    rules.lover_married_elsewhere = factor(
        source,
        &lover["married_elsewhere"],
        &at("lover", "married_elsewhere"),
    )?;
    rules.lover_age_gap = divisor(
        source,
        &lover["age_gap_years"],
        &at("lover", "age_gap_years"),
    )?;
    let love = kind_table("love", &["spouse", "parent_child", "beauty_if_attracted"])?;
    rules.love_spouse = factor(source, &love["spouse"], &at("love", "spouse"))?;
    rules.love_parent_child = factor(source, &love["parent_child"], &at("love", "parent_child"))?;
    rules.love_beauty = grades(
        source,
        &love["beauty_if_attracted"],
        &at("love", "beauty_if_attracted"),
    )?;
    let respect = kind_table("respect", &["fame_per", "skills_divisor", "power_per"])?;
    rules.respect_fame_per = signed(source, &respect["fame_per"], &at("respect", "fame_per"))?;
    rules.respect_skills_divisor = divisor(
        source,
        &respect["skills_divisor"],
        &at("respect", "skills_divisor"),
    )?;
    rules.respect_power_per = signed(source, &respect["power_per"], &at("respect", "power_per"))?;
    let envy = kind_table(
        "envy",
        &[
            "envy_divisor",
            "envy_floor",
            "beauty_per_grade",
            "more_power",
        ],
    )?;
    rules.envy_divisor = divisor(source, &envy["envy_divisor"], &at("envy", "envy_divisor"))?;
    rules.envy_floor = signed(source, &envy["envy_floor"], &at("envy", "envy_floor"))?;
    rules.envy_beauty_per_grade = signed(
        source,
        &envy["beauty_per_grade"],
        &at("envy", "beauty_per_grade"),
    )?;
    rules.envy_more_power = factor(source, &envy["more_power"], &at("envy", "more_power"))?;
    let fear = kind_table("fear", &["rage_per_grade", "grim_face", "daring_per_point"])?;
    rules.fear_rage_per_grade = signed(
        source,
        &fear["rage_per_grade"],
        &at("fear", "rage_per_grade"),
    )?;
    rules.fear_grim_face = factor(source, &fear["grim_face"], &at("fear", "grim_face"))?;
    rules.fear_daring_per_point = signed(
        source,
        &fear["daring_per_point"],
        &at("fear", "daring_per_point"),
    )?;
    let hate = kind_table("hatred", &["patience_per_point", "kindness_per_point"])?;
    rules.hatred_patience = signed(
        source,
        &hate["patience_per_point"],
        &at("hatred", "patience_per_point"),
    )?;
    rules.hatred_kindness = signed(
        source,
        &hate["kindness_per_point"],
        &at("hatred", "kindness_per_point"),
    )?;
    let happenings = section(source, table, "strands", "happenings")?;
    exact(
        source,
        happenings,
        "strands.happenings",
        &["battle_comrades", "rescue", "insult", "kin_killed", "duel"],
    )?;
    for (name, node) in happenings {
        let entry = format!("strands.happenings.{name}");
        let rule = source.table(node, &entry)?;
        let plural = rule.contains_key("kinds");
        exact(
            source,
            rule,
            &entry,
            &[if plural { "kinds" } else { "kind" }, "chance"],
        )?;
        let kinds = if plural {
            match &rule["kinds"] {
                Node::Array(items) => items
                    .iter()
                    .map(|item| text(source, item, &format!("{entry}.kinds")))
                    .collect::<Result<Vec<_>, _>>()?,
                _ => return Err(source.error(format!("{entry}.kinds"), "expected an array")),
            }
        } else {
            vec![text(source, &rule["kind"], &format!("{entry}.kind"))?]
        };
        if kinds.is_empty() || kinds.iter().any(|kind| !ALL_KINDS.contains(&kind.as_str())) {
            return Err(source.error(&entry, "a happening rolls known strand kinds"));
        }
        rules.happenings.insert(
            name.clone(),
            HappeningRule {
                kinds,
                chance_ppb: source.ppb(&rule["chance"], &format!("{entry}.chance"))?,
            },
        );
    }
    Ok(rules)
}

fn parse_orientation(source: &Source<'_>, table: &Table) -> Result<Orientation, DataError> {
    exact(
        source,
        table,
        "orientation",
        &["weights", "parent_boost", "clergy"],
    )?;
    let weights = source.table(&table["weights"], "orientation.weights")?;
    exact(source, weights, "orientation.weights", &ORIENTATIONS)?;
    let mut out = Orientation::default();
    for (slot, name) in ORIENTATIONS.iter().enumerate() {
        out.weights[slot] = small_u32(
            source,
            &weights[*name],
            &format!("orientation.weights.{name}"),
        )?;
    }
    if out.weights.iter().any(|weight| *weight > MOST_WEIGHT) {
        return Err(source.error("orientation.weights", "a weight is at most 1000000"));
    }
    if out.weights.iter().map(|w| u64::from(*w)).sum::<u64>() == 0 {
        return Err(source.error("orientation.weights", "the weights must sum above 0"));
    }
    out.parent_boost_ppb = factor(source, &table["parent_boost"], "orientation.parent_boost")?;
    // The boosted distribution (a child of a gay or bisexual parent) must have a weight too: a zero boost would zero gay and bisexual.
    if out.parent_boost_ppb == 0 && out.weights[0] == 0 && out.weights[3] == 0 {
        return Err(source.error(
            "orientation.parent_boost",
            "a zero boost leaves no weight for a child of a gay or bisexual parent",
        ));
    }
    let clergy = source.table(&table["clergy"], "orientation.clergy")?;
    for (key, node) in clergy {
        let entry = format!("orientation.clergy.{key}");
        if !ORIENTATIONS.contains(&key.as_str()) {
            return Err(source.error(entry, "unknown key"));
        }
        factor(source, node, &entry)?;
    }
    Ok(out)
}

fn moment_target(
    source: &Source<'_>,
    entry: &str,
    value: &str,
    live: bool,
) -> Result<Target, DataError> {
    Ok(match value {
        "the killer's house" => Target::KillerHouse,
        "the character's location" => Target::Location,
        "the character's religion" => Target::Religion,
        "the character's house" => Target::House,
        "the character's culture" => Target::Culture,
        _ if !live => Target::Deferred(value.to_owned()),
        _ => {
            return Err(source.error(entry, "a live moment's target is one the engine resolves"));
        }
    })
}

fn parse_moment(
    source: &Source<'_>,
    parser: &Parser<'_>,
    key: &str,
    node: &Node,
) -> Result<Moment, DataError> {
    let entry = format!("passions.moments.{key}");
    let table = source.table(node, &entry)?;
    exact(
        source,
        table,
        &entry,
        &["trigger", "when", "passion", "chance", "live"],
    )?;
    let live = source.boolean(&table["live"], &format!("{entry}.live"))?;
    let trigger_text = text(source, &table["trigger"], &format!("{entry}.trigger"))?;
    let trigger = match trigger_text.strip_prefix("happening:") {
        _ if trigger_text == "yearly" => MomentTrigger::Yearly,
        Some(name) if !name.is_empty() => MomentTrigger::Happening(name.to_owned()),
        _ => {
            return Err(source.error(
                format!("{entry}.trigger"),
                "a trigger is `yearly` or `happening:<name>`",
            ));
        }
    };
    let when_entry = format!("{entry}.when");
    let when = if live {
        parser.conditions(&table["when"], &when_entry, Vocabulary::Moment)?
    } else {
        source.table(&table["when"], &when_entry)?;
        Conditions::default()
    };
    let passion_entry = format!("{entry}.passion");
    let passion = source.table(&table["passion"], &passion_entry)?;
    exact(source, passion, &passion_entry, &["kind", "target"])?;
    let kind = text(source, &passion["kind"], &format!("{passion_entry}.kind"))?;
    if !PASSION_KINDS.contains(&kind.as_str()) {
        return Err(source.error(
            format!("{passion_entry}.kind"),
            "a passion is loyalty, love, hatred or fear",
        ));
    }
    let target = moment_target(
        source,
        &format!("{passion_entry}.target"),
        source.text(&passion["target"], &format!("{passion_entry}.target"))?,
        live,
    )?;
    Ok(Moment {
        key: key.to_owned(),
        trigger,
        when,
        kind,
        target,
        chance_ppb: source.ppb(&table["chance"], &format!("{entry}.chance"))?,
        live,
    })
}

fn parse_passions(
    source: &Source<'_>,
    parser: &Parser<'_>,
    table: &Table,
) -> Result<PassionRules, DataError> {
    exact(
        source,
        table,
        "passions",
        &["tier", "start_strength", "fade_years", "moments"],
    )?;
    let tier = source.table(&table["tier"], "passions.tier")?;
    exact(source, tier, "passions.tier", &["full", "coarse"])?;
    let start = small_u32(source, &table["start_strength"], "passions.start_strength")?;
    if !(1..=10).contains(&start) {
        return Err(source.error("passions.start_strength", "a strength lies in 1..10"));
    }
    let moments = source.table(&table["moments"], "passions.moments")?;
    let mut out = PassionRules {
        full_ppb: source.ppb(&tier["full"], "passions.tier.full")?,
        coarse_ppb: source.ppb(&tier["coarse"], "passions.tier.coarse")?,
        start_strength: start,
        fade_years: divisor(source, &table["fade_years"], "passions.fade_years")?,
        moments: Vec::new(),
    };
    for (key, node) in moments {
        out.moments.push(parse_moment(source, parser, key, node)?);
    }
    Ok(out)
}

fn opinion_weights(
    source: &Source<'_>,
    table: &Table,
    entry: &str,
    kinds: &[&str],
) -> Result<BTreeMap<String, i64>, DataError> {
    exact(source, table, entry, kinds)?;
    let mut out = BTreeMap::new();
    for (key, node) in table {
        let at = format!("{entry}.{key}");
        out.insert(key.clone(), source.milli(node, &at)?);
    }
    Ok(out)
}

fn parse_log(source: &Source<'_>, table: &Table, rules: &mut Connections) -> Result<(), DataError> {
    let mut known: Vec<&str> = ALL_KINDS.to_vec();
    known.push("passion");
    exact(source, table, "log", &known)?;
    for kind in ALL_KINDS {
        let entry = format!("log.{kind}");
        let lines = section(source, table, "log", kind)?;
        exact(source, lines, &entry, &["made", "at_5", "at_10", "ended"])?;
        let line = |key: &str| text(source, &lines[key], &format!("{entry}.{key}"));
        rules.log.insert(
            kind.to_owned(),
            StrandText {
                made: line("made")?,
                at_5: line("at_5")?,
                at_10: line("at_10")?,
                ended: line("ended")?,
            },
        );
    }
    let passion = section(source, table, "log", "passion")?;
    let mut keys: Vec<String> = PASSION_KINDS
        .iter()
        .map(|k| format!("gained_{k}"))
        .collect();
    keys.extend(["at_10".to_owned(), "faded".to_owned()]);
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
    exact(source, passion, "log.passion", &keys)?;
    for kind in PASSION_KINDS {
        rules.passion_gained.insert(
            kind.to_owned(),
            text(
                source,
                &passion[&format!("gained_{kind}")],
                &format!("log.passion.gained_{kind}"),
            )?,
        );
    }
    rules.passion_at_10 = text(source, &passion["at_10"], "log.passion.at_10")?;
    rules.passion_faded = text(source, &passion["faded"], "log.passion.faded")?;
    Ok(())
}

fn parse_contact(
    source: &Source<'_>,
    table: &Table,
    rules: &mut Connections,
) -> Result<(), DataError> {
    exact(source, table, "contact", &["settings", "extra_setting"])?;
    rules.extra_setting_ppb = source.ppb(&table["extra_setting"], "contact.extra_setting")?;
    let settings = source.table(&table["settings"], "contact.settings")?;
    exact(source, settings, "contact.settings", &SETTINGS)?;
    for (name, node) in settings {
        let entry = format!("contact.settings.{name}");
        let setting = source.table(node, &entry)?;
        exact(source, setting, &entry, &["value", "live", "rule"])?;
        source.text(&setting["rule"], &format!("{entry}.rule"))?;
        rules.settings.insert(
            name.clone(),
            Setting {
                contact_ppb: source.ppb(&setting["value"], &format!("{entry}.value"))?,
                live: source.boolean(&setting["live"], &format!("{entry}.live"))?,
            },
        );
    }
    Ok(())
}

impl EventRules {
    /// Parse and validate `connections.toml` (CK-368) against these rules' catalog and skills. A live entry is fully validated: an
    /// unknown key, a probability or contact outside 0..1, a negative strength, size or year count, a zero or negative divisor, a
    /// grade table that is not seven values, orientation weights that do not sum above 0, a live moment's condition outside the
    /// vocabulary, and a live moment's target the engine does not resolve are errors. An inert entry (`live = false`) is admitted:
    /// only its shape is checked.
    ///
    /// # Errors
    /// Returns a source-qualified error for the first invalid entry.
    pub fn parse_connections(
        &self,
        file: &str,
        text: &str,
        registry: &Registry<'_>,
    ) -> Result<Connections, DataError> {
        let source = Source { file };
        let root = source.document(text)?;
        only_keys(
            &source,
            &root,
            "",
            &[
                "contact",
                "circle",
                "strands",
                "orientation",
                "passions",
                "settling_in",
                "burn_in",
                "opinion",
                "log",
            ],
        )?;
        let parser = Parser {
            source: Source { file },
            registry,
            catalog: self.catalog.clone(),
            skills: self.skills.list.clone(),
            counters: self.deeds.iter().map(|d| d.counter.clone()).collect(),
        };
        let mut rules = Connections {
            loaded: true,
            ..Connections::default()
        };
        let top = |name: &str| section(&source, &root, "", name);
        parse_contact(&source, top("contact")?, &mut rules)?;
        let circle = top("circle")?;
        exact(
            &source,
            circle,
            "circle",
            &["floor", "size", "min_age", "lover_min_age"],
        )?;
        rules.circle = Circle {
            floor_ppb: source.ppb(&circle["floor"], "circle.floor")?,
            size: small_u32(&source, &circle["size"], "circle.size")?,
            min_age: small_u32(&source, &circle["min_age"], "circle.min_age")?,
            lover_min_age: small_u32(&source, &circle["lover_min_age"], "circle.lover_min_age")?,
        };
        rules.strands = parse_strands(&source, top("strands")?)?;
        rules.orientation = parse_orientation(&source, top("orientation")?)?;
        rules.passions = parse_passions(&source, &parser, top("passions")?)?;
        let settling = top("settling_in")?;
        exact(&source, settling, "settling_in", &["max_years"])?;
        rules.max_years = small_u32(&source, &settling["max_years"], "settling_in.max_years")?;
        let burn = top("burn_in")?;
        exact(&source, burn, "burn_in", &["years"])?;
        rules.burn_in_years = small_u32(&source, &burn["years"], "burn_in.years")?;
        let opinion = top("opinion")?;
        exact(&source, opinion, "opinion", &["strands", "passions"])?;
        rules.opinion_strands = opinion_weights(
            &source,
            section(&source, opinion, "opinion", "strands")?,
            "opinion.strands",
            &ALL_KINDS,
        )?;
        rules.opinion_passions = opinion_weights(
            &source,
            section(&source, opinion, "opinion", "passions")?,
            "opinion.passions",
            &PASSION_KINDS,
        )?;
        parse_log(&source, top("log")?, &mut rules)?;
        Ok(rules)
    }
}
