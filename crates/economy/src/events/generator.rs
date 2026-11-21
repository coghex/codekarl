//! Generator data (CK-369, characters slice 7): `generator.toml`.
//!
//! Nothing here uses floating point. A decimal is read from its digits and kept as an exact integer: a probability in parts per
//! billion, a skill level, a stress amount, a weight and a language strength in thousandths. The prose `from` fields of the slots and
//! the `[marriage_candidates]` descriptions are descriptions: the behaviour they describe is code.
#![deny(clippy::float_arithmetic)]

use super::{Registry, only_keys, required, small_u32};
use crate::{
    DataError, EventRules,
    genetics::{Node, Source, Table},
};
use std::collections::BTreeMap;

/// The eleven steps of the recipe, in the order a generated character is made.
pub const RECIPE_STEPS: [&str; 11] = [
    "parents",
    "sex_and_birth",
    "lineage",
    "body",
    "inherited_traits",
    "personality",
    "orientation",
    "languages",
    "focus",
    "name",
    "past",
];
/// The role vocabulary: a fixed list in the code, not derived from any data file. A role is stored on the record of a generated character
/// and is separate from the office markers of slice 1 (`ruler`, `heir`, `high_cleric`). Slice 8's slot member roles are validated against it.
pub const ROLES: [&str; 27] = [
    "abbess",
    "abbot",
    "anchorite",
    "bishop",
    "champion",
    "church_servant",
    "church_tenant",
    "community_woman",
    "craftsman",
    "fosterling",
    "herdsman",
    "hostage",
    "judge",
    "keeper_of_relics",
    "kinsman",
    "lector",
    "lord",
    "monk",
    "nun",
    "poet",
    "prior",
    "retainer",
    "scribe",
    "spouse_of_lord",
    "spouse_of_ruler",
    "steward",
    "student",
];
/// The slot kinds the data may describe.
pub const SLOT_KINDS: [&str; 3] = ["fosterling", "hostage", "student"];
/// The concocted happenings of a generated past, each a yearly chance.
pub const HAPPENINGS: [&str; 5] = ["death_in_family", "insulted", "feast", "captured", "exiled"];
/// The reference rates the tests can build, besides `orientation`.
pub const REFERENCE_KEYS: [&str; 9] = [
    "veteran_noble_men_45",
    "scribe_clergy_men_45",
    "latinist_clergy_men_45",
    "melancholy_noble_men_45",
    "melancholy_clergy_men_45",
    "rage_fits_noble_men_45",
    "recluse_clergy_men_45",
    "drunkard_peasant_men_45",
    "madness_any_men_45",
];
/// The ages of a generated past in which a man may fight a battle (a yearly chance of the data's `battles`, doubled by a martial focus).
pub const BATTLE_AGES: [u32; 2] = [16, 55];
/// The second-language sources of `[languages]`, other than `latin_by_office`.
const LANGUAGE_SOURCES: [&str; 5] = [
    "latinist",
    "fostered_among",
    "hostage_raised",
    "foreign_mother",
    "exile",
];
/// The keys of `[slots]` that are not a slot kind.
const SLOT_SETTINGS: [&str; 3] = ["reach_km", "weights", "order"];

/// A range of language strengths in thousandths, `low <= high`, both within 0..=1000.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StrengthRange {
    /// The lowest strength, in thousandths.
    pub low: u32,
    /// The highest strength, in thousandths.
    pub high: u32,
}

/// `[past]`: the generated life.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PastRules {
    /// Personality is pulled to the household until this age.
    pub upbringing_to: u32,
    /// Skills grow by use from this age.
    pub skills_from: u32,
    /// The yearly check runs from this age.
    pub yearly_check_from: u32,
    /// The concocted happenings: the chance of each a year, in ppb, by [`HAPPENINGS`] key.
    pub happenings: BTreeMap<String, u32>,
    /// The chance a year of a new friend, in ppb.
    pub new_friend_ppb: u32,
    /// The yearly chance of a battle for a man (ages [`BATTLE_AGES`]), by `<pop type>_m` key, in ppb; a pop type with no entry has none.
    pub battles: BTreeMap<String, u32>,
    /// The share of battles that were defeats, in ppb.
    pub battle_defeat_ppb: u32,
    /// The leadership a battle may be won in command from, in thousandths of a level.
    pub command_leadership_min: i64,
    /// The chance, in ppb, that a battle fought with that leadership was won in command.
    pub command_chance_ppb: u32,
    /// The yearly chance of leading a raid for a noble man, in ppb.
    pub raids_noble_m_ppb: u32,
    /// The same for a noble man with a martial focus, in ppb.
    pub raids_martial_focus_ppb: u32,
    /// The ages a raid may be led in, both inclusive.
    pub raids_ages: [u32; 2],
    /// The martial prowess a duel may be won from, in thousandths of a level.
    pub duel_prowess_min: i64,
    /// The yearly chance of winning a duel with that prowess, in ppb.
    pub duel_chance_ppb: u32,
    /// The mean of the stress added each year, in thousandths.
    pub strain_mean: i64,
    /// Its spread, in thousandths.
    pub strain_sd: i64,
}

/// A reference rate the tests hold: a rate and the band around it, both in ppb.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reference {
    /// The rate, in ppb.
    pub rate_ppb: u32,
    /// The absolute band around it, in ppb.
    pub band_ppb: u32,
}

/// The orientation reference of a lay character.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OrientationReference {
    /// The rates of straight, gay, bisexual and asexual, in ppb, in that order.
    pub rates_ppb: [u32; 4],
    /// The absolute band around each, in ppb.
    pub band_ppb: u32,
}

/// `[languages]`: the second languages of a character.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LanguageRules {
    /// The Latin a role gives, by role key (a key of [`ROLES`]).
    pub latin_by_office: BTreeMap<String, StrengthRange>,
    /// The Latin of a `latinist`, gained in the past.
    pub latinist: StrengthRange,
    /// The tongue of the fostering family, on a fosterling's move.
    pub fostered_among: StrengthRange,
    /// The captor's tongue, on a hostage's move.
    pub hostage_raised: StrengthRange,
    /// The mother's tongue when it differs.
    pub foreign_mother: StrengthRange,
    /// The host's tongue after an exile (inert: a generated past has no host).
    pub exile: StrengthRange,
}

/// One slot kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotKind {
    /// The ages of a man or boy, both inclusive.
    pub age_m: [u32; 2],
    /// The ages of a woman or girl, both inclusive.
    pub age_f: [u32; 2],
    /// Whether the children of the court's own ruler are fostered out (placed in another house).
    pub fostered_out: bool,
}

/// `[slots]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlotRules {
    /// The kinds, by key of [`SLOT_KINDS`].
    pub kinds: BTreeMap<String, SlotKind>,
    /// How far a fosterling or a student may come from, in kilometres.
    pub reach_km: u32,
    /// The weight of kin of the court's house, in thousandths.
    pub kin_milli: u32,
    /// The weight of a house of another kingdom for a hostage, in thousandths (inert until tags exist).
    pub subject_house_milli: u32,
}

/// A culture's mother tongue: its language and, where the language has dialects, its dialect.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tongue {
    /// The language key of the registry.
    pub language: String,
    /// The dialect key, empty for a language with none.
    pub dialect: String,
}

/// The key of the Latin language in the registry: the second language of the clergy and the learned.
pub const LATIN: &str = "latin";

/// The loaded `generator.toml`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Generator {
    /// Whether a file was loaded. A world without it generates nothing.
    pub loaded: bool,
    /// The mother tongue of each culture of the registry (derived from it, not read from the file).
    pub tongues: BTreeMap<String, Tongue>,
    /// `[past]`.
    pub past: PastRules,
    /// `[reference]`, by key of [`REFERENCE_KEYS`].
    pub reference: BTreeMap<String, Reference>,
    /// `[reference] orientation`.
    pub orientation: OrientationReference,
    /// `[languages]`.
    pub languages: LanguageRules,
    /// `[slots]`.
    pub slots: SlotRules,
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

fn array<'t>(source: &Source<'_>, node: &'t Node, entry: &str) -> Result<&'t [Node], DataError> {
    match node {
        Node::Array(items) => Ok(items),
        _ => Err(source.error(entry, "expected an array")),
    }
}

/// `[low, high]`, two whole ages with `low <= high`.
fn ages(source: &Source<'_>, node: &Node, entry: &str) -> Result<[u32; 2], DataError> {
    let [low, high] = array(source, node, entry)? else {
        return Err(source.error(entry, "an age range is `[low, high]`"));
    };
    let range = [
        small_u32(source, low, entry)?,
        small_u32(source, high, entry)?,
    ];
    if range[0] > range[1] {
        return Err(source.error(entry, "the low age exceeds the high age"));
    }
    Ok(range)
}

/// `[low, high]`, two strengths in 0..=1 with `low <= high`.
fn strength(source: &Source<'_>, node: &Node, entry: &str) -> Result<StrengthRange, DataError> {
    let [low, high] = array(source, node, entry)? else {
        return Err(source.error(entry, "a strength range is `[low, high]`"));
    };
    let range = StrengthRange {
        low: source.unit_milli(low, entry)?,
        high: source.unit_milli(high, entry)?,
    };
    if range.low > range.high {
        return Err(source.error(entry, "the low strength exceeds the high strength"));
    }
    Ok(range)
}

#[expect(
    clippy::too_many_lines,
    reason = "one section of the data file, each field checked and read where it is named"
)]
fn parse_past(
    source: &Source<'_>,
    rules: &EventRules,
    registry: &Registry<'_>,
    table: &Table,
) -> Result<PastRules, DataError> {
    exact(
        source,
        table,
        "past",
        &[
            "upbringing_to",
            "skills_from",
            "yearly_check_from",
            "happenings",
            "new_friend",
            "battles",
            "battle_defeat",
            "battle_won_in_command",
            "raids_led",
            "duels_won",
            "yearly_strain",
            "no_deaths",
        ],
    )?;
    if !source.boolean(&table["no_deaths"], "past.no_deaths")? {
        return Err(source.error(
            "past.no_deaths",
            "a generated past never kills: deaths apply only in play",
        ));
    }
    let skill = |name: &str, entry: &str| -> Result<(), DataError> {
        if rules.skills.list.iter().any(|known| known == name) {
            Ok(())
        } else {
            Err(source.error(entry, format!("`{name}` is not a skill")))
        }
    };
    let mut past = PastRules {
        upbringing_to: small_u32(source, &table["upbringing_to"], "past.upbringing_to")?,
        skills_from: small_u32(source, &table["skills_from"], "past.skills_from")?,
        yearly_check_from: small_u32(
            source,
            &table["yearly_check_from"],
            "past.yearly_check_from",
        )?,
        new_friend_ppb: source.ppb(&table["new_friend"], "past.new_friend")?,
        battle_defeat_ppb: source.ppb(&table["battle_defeat"], "past.battle_defeat")?,
        ..PastRules::default()
    };
    let happenings = section(source, table, "past", "happenings")?;
    exact(source, happenings, "past.happenings", &HAPPENINGS)?;
    for (name, node) in happenings {
        past.happenings.insert(
            name.clone(),
            source.ppb(node, &format!("past.happenings.{name}"))?,
        );
    }
    for (name, node) in section(source, table, "past", "battles")? {
        let kind = name
            .strip_suffix("_m")
            .filter(|kind| registry.pop_types.contains(kind));
        if kind.is_none() {
            return Err(source.error(
                format!("past.battles.{name}"),
                "a battle chance is `<pop type>_m` of a pop type",
            ));
        }
        past.battles.insert(
            name.clone(),
            source.ppb(node, &format!("past.battles.{name}"))?,
        );
    }
    let command = section(source, table, "past", "battle_won_in_command")?;
    exact(
        source,
        command,
        "past.battle_won_in_command",
        &["leadership_min", "chance"],
    )?;
    skill("leadership", "past.battle_won_in_command.leadership_min")?;
    past.command_leadership_min = source.milli_non_negative(
        &command["leadership_min"],
        "past.battle_won_in_command.leadership_min",
    )?;
    past.command_chance_ppb =
        source.ppb(&command["chance"], "past.battle_won_in_command.chance")?;
    let raids = section(source, table, "past", "raids_led")?;
    exact(
        source,
        raids,
        "past.raids_led",
        &["noble_m", "martial_focus", "ages"],
    )?;
    past.raids_noble_m_ppb = source.ppb(&raids["noble_m"], "past.raids_led.noble_m")?;
    past.raids_martial_focus_ppb =
        source.ppb(&raids["martial_focus"], "past.raids_led.martial_focus")?;
    past.raids_ages = ages(source, &raids["ages"], "past.raids_led.ages")?;
    let duels = section(source, table, "past", "duels_won")?;
    exact(
        source,
        duels,
        "past.duels_won",
        &["martial_prowess_min", "chance"],
    )?;
    skill("martial_prowess", "past.duels_won.martial_prowess_min")?;
    past.duel_prowess_min = source.milli_non_negative(
        &duels["martial_prowess_min"],
        "past.duels_won.martial_prowess_min",
    )?;
    past.duel_chance_ppb = source.ppb(&duels["chance"], "past.duels_won.chance")?;
    let strain = section(source, table, "past", "yearly_strain")?;
    exact(source, strain, "past.yearly_strain", &["mean", "sd"])?;
    past.strain_mean = source.milli_non_negative(&strain["mean"], "past.yearly_strain.mean")?;
    past.strain_sd = source.milli_non_negative(&strain["sd"], "past.yearly_strain.sd")?;
    Ok(past)
}

fn parse_reference(
    source: &Source<'_>,
    table: &Table,
    rules: &mut Generator,
) -> Result<(), DataError> {
    for (name, node) in table {
        let entry = format!("reference.{name}");
        let body = source.table(node, &entry)?;
        if name == "orientation" {
            exact(
                source,
                body,
                &entry,
                &["straight", "gay", "bisexual", "asexual", "band"],
            )?;
            let rate = |key: &str| source.ppb(&body[key], &format!("{entry}.{key}"));
            rules.orientation = OrientationReference {
                rates_ppb: [
                    rate("straight")?,
                    rate("gay")?,
                    rate("bisexual")?,
                    rate("asexual")?,
                ],
                band_ppb: rate("band")?,
            };
        } else if REFERENCE_KEYS.contains(&name.as_str()) {
            exact(source, body, &entry, &["rate", "band"])?;
            rules.reference.insert(
                name.clone(),
                Reference {
                    rate_ppb: source.ppb(&body["rate"], &format!("{entry}.rate"))?,
                    band_ppb: source.ppb(&body["band"], &format!("{entry}.band"))?,
                },
            );
        } else {
            return Err(source.error(entry, "a reference the tests cannot build"));
        }
    }
    for key in REFERENCE_KEYS {
        if !rules.reference.contains_key(key) {
            return Err(source.error(format!("reference.{key}"), "missing value"));
        }
    }
    if !table.contains_key("orientation") {
        return Err(source.error("reference.orientation", "missing value"));
    }
    Ok(())
}

fn parse_languages(source: &Source<'_>, table: &Table) -> Result<LanguageRules, DataError> {
    let mut known = vec!["mother_tongue", "latin_by_office"];
    known.extend(LANGUAGE_SOURCES);
    exact(source, table, "languages", &known)?;
    if source.text(&table["mother_tongue"], "languages.mother_tongue")? != "culture" {
        return Err(source.error(
            "languages.mother_tongue",
            "the mother tongue is the culture's (`culture`)",
        ));
    }
    let mut rules = LanguageRules::default();
    let latin = source.table(&table["latin_by_office"], "languages.latin_by_office")?;
    for (role, node) in latin {
        let entry = format!("languages.latin_by_office.{role}");
        if !ROLES.contains(&role.as_str()) {
            return Err(source.error(entry, format!("`{role}` is not in the role vocabulary")));
        }
        rules
            .latin_by_office
            .insert(role.clone(), strength(source, node, &entry)?);
    }
    let one = |key: &str| strength(source, &table[key], &format!("languages.{key}"));
    rules.latinist = one("latinist")?;
    rules.fostered_among = one("fostered_among")?;
    rules.hostage_raised = one("hostage_raised")?;
    rules.foreign_mother = one("foreign_mother")?;
    rules.exile = one("exile")?;
    Ok(rules)
}

fn parse_slots(source: &Source<'_>, table: &Table) -> Result<SlotRules, DataError> {
    let mut rules = SlotRules::default();
    for (name, node) in table {
        if SLOT_SETTINGS.contains(&name.as_str()) {
            continue;
        }
        let entry = format!("slots.{name}");
        if !SLOT_KINDS.contains(&name.as_str()) {
            return Err(source.error(entry, "a slot kind is `fosterling`, `hostage` or `student`"));
        }
        let body = source.table(node, &entry)?;
        let kind = if name == "fosterling" {
            only_keys(source, body, &entry, &["age", "from", "fostered_out"])?;
            let by_sex = source.table(
                required(source, body, &entry, "age")?,
                &format!("{entry}.age"),
            )?;
            exact(source, by_sex, &format!("{entry}.age"), &["m", "f"])?;
            SlotKind {
                age_m: ages(source, &by_sex["m"], &format!("{entry}.age.m"))?,
                age_f: ages(source, &by_sex["f"], &format!("{entry}.age.f"))?,
                fostered_out: match body.get("fostered_out") {
                    Some(flag) => source.boolean(flag, &format!("{entry}.fostered_out"))?,
                    None => false,
                },
            }
        } else {
            only_keys(source, body, &entry, &["age", "from"])?;
            let range = ages(
                source,
                required(source, body, &entry, "age")?,
                &format!("{entry}.age"),
            )?;
            SlotKind {
                age_m: range,
                age_f: range,
                fostered_out: false,
            }
        };
        if let Some(from) = body.get("from") {
            source.text(from, &format!("{entry}.from"))?;
        }
        rules.kinds.insert(name.clone(), kind);
    }
    for kind in SLOT_KINDS {
        if !rules.kinds.contains_key(kind) {
            return Err(source.error(format!("slots.{kind}"), "missing section"));
        }
    }
    let reach = source.integer(
        required(source, table, "slots", "reach_km")?,
        "slots.reach_km",
    )?;
    rules.reach_km = u32::try_from(reach)
        .map_err(|_| source.error("slots.reach_km", "the reach must not be negative"))?;
    let weights = section(source, table, "slots", "weights")?;
    exact(source, weights, "slots.weights", &["kin", "subject_house"])?;
    rules.kin_milli = source.multiplier(&weights["kin"], "slots.weights.kin")?;
    rules.subject_house_milli =
        source.multiplier(&weights["subject_house"], "slots.weights.subject_house")?;
    if let Some(order) = table.get("order") {
        source.text(order, "slots.order")?;
    }
    Ok(rules)
}

impl EventRules {
    /// Parse and validate `generator.toml` (CK-369) against these rules' skills and the registry's pop types. The loader refuses a recipe
    /// order that is not exactly the eleven shipped steps; a probability or band outside 0..1; a reference key the tests cannot build; a
    /// language range outside 0..1 or with its low above its high; a role of `latin_by_office` outside the role vocabulary; a slot kind
    /// other than `fosterling`, `hostage` and `student`; a negative `reach_km` or weight; and a past that kills.
    ///
    /// # Errors
    /// Returns a source-qualified error for the first invalid entry.
    pub fn parse_generator(
        &self,
        file: &str,
        text: &str,
        registry: &Registry<'_>,
    ) -> Result<Generator, DataError> {
        let source = Source { file };
        let root = source.document(text)?;
        only_keys(
            &source,
            &root,
            "",
            &[
                "recipe",
                "past",
                "reference",
                "languages",
                "slots",
                "marriage_candidates",
                "real_households",
            ],
        )?;
        let recipe = section(&source, &root, "", "recipe")?;
        exact(&source, recipe, "recipe", &["order"])?;
        let order: Vec<&str> = array(&source, &recipe["order"], "recipe.order")?
            .iter()
            .map(|step| source.text(step, "recipe.order"))
            .collect::<Result<_, _>>()?;
        if order != RECIPE_STEPS {
            return Err(source.error(
                "recipe.order",
                format!(
                    "the order is exactly the shipped steps: {}",
                    RECIPE_STEPS.join(", ")
                ),
            ));
        }
        if !registry
            .peoples
            .languages
            .iter()
            .any(|language| language.id == LATIN)
        {
            return Err(source.error("languages", "the registry has no `latin` language"));
        }
        let mut rules = Generator {
            loaded: true,
            tongues: registry
                .peoples
                .cultures
                .iter()
                .map(|culture| {
                    (
                        culture.id.clone(),
                        Tongue {
                            language: culture.language.clone(),
                            dialect: culture.dialect.clone().unwrap_or_default(),
                        },
                    )
                })
                .collect(),
            ..Generator::default()
        };
        rules.past = parse_past(
            &source,
            self,
            registry,
            section(&source, &root, "", "past")?,
        )?;
        parse_reference(
            &source,
            section(&source, &root, "", "reference")?,
            &mut rules,
        )?;
        rules.languages = parse_languages(&source, section(&source, &root, "", "languages")?)?;
        rules.slots = parse_slots(&source, section(&source, &root, "", "slots")?)?;
        // The two prose sections are descriptions: their shape only.
        for name in ["marriage_candidates", "real_households"] {
            if let Some(node) = root.get(name) {
                source.table(node, name)?;
            }
        }
        Ok(rules)
    }
}
