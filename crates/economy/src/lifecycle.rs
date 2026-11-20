//! Life-cycle data: death, majority, marriageable age, relations, conception and name pools (CK-364, characters slice 5a).
//!
//! Nothing here uses floating point. A decimal is read from its digits and stored as an exact integer: a probability or a factor in
//! parts per billion, a per-point value in millionths, every other quantity in thousandths. `life-cycle.toml` also holds the sections
//! of the later issues (marriage, upbringing, nutrition); they load untouched (their text is part of the save's rules hash) and
//! nothing here reads them. `names.toml` also holds the namesake rules (CK-365) and `epithets.toml` the epithets (CK-365).
#![deny(clippy::float_arithmetic)]

use crate::{
    DataError, EventRules, Genetics, Peoples,
    bigint::Big,
    events::{self, Epithet},
    genetics::{Node, PPB, Source, Table},
};
use std::{collections::BTreeMap, fs, path::Path};

/// The latest age death is certain from (the tables have a row for every age).
const MOST_CERTAIN_AT: u32 = 200;
/// The most years of `infant_years` and `old_doubling`.
const MOST_YEARS: u32 = 1000;
/// One in parts per billion, as a wide integer.
const ONE: u128 = 1_000_000_000;

/// The three relation modes, in the order of [`RelationRules::mode_factor`].
pub const MODES: [&str; 3] = ["trying", "neutral", "avoidance"];

/// An age in whole years for men and for women.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ages {
    /// Men.
    pub m: u32,
    /// Women.
    pub f: u32,
}

/// `[death]`: the annual chance by age, scaled by the character's own values and the pop's conditions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeathRules {
    /// The infant term at age 0, in ppb.
    pub infant_ppb: u32,
    /// Years over which the infant term falls by a factor of e (a positive whole number).
    pub infant_years: u32,
    /// The base term, in ppb.
    pub base_ppb: u32,
    /// The old-age term at `old_from`, in ppb.
    pub old_ppb: u32,
    /// The age at which the old-age term equals `old_ppb`.
    pub old_from: u32,
    /// Years for the old-age term to double (a positive whole number).
    pub old_doubling: u32,
    /// The age from which death is certain.
    pub certain_at: u32,
    /// Longevity's scaling of the old-age term per point from 10, in millionths.
    pub longevity_per_point: i64,
    /// Constitution's scaling of the base term per point from 10, in millionths.
    pub constitution_per_point: i64,
    /// Whether the pop's death multipliers apply.
    pub use_pop_curves: bool,
    /// A shown weight below this share of the rolled weight is an extreme, in thousandths.
    pub weight_low: i64,
    /// A shown weight above this share of the rolled weight is an extreme, in thousandths.
    pub weight_high: i64,
    /// The multiplier of the first step beyond the limit, in thousandths.
    pub weight_factor: i64,
    /// Added for each further 5% step, in thousandths.
    pub weight_step: i64,
    /// A mother's chance of dying at a delivery, in ppb.
    pub childbirth_ppb: u32,
    /// The tables of the infant and old-age terms, built when the data loads.
    pub tables: DeathTables,
}

/// The two tables the annual chance reads, in ppb, built from the rules when they load (CK-364 §2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeathTables {
    /// `E(a) = e^(-a / infant_years)` for every age `0..=certain_at`.
    pub infant: Vec<u64>,
    /// `P(k) = R^k` for `k = 0..=max(old_from, certain_at - old_from)`, `R` being `2^(1 / old_doubling)`.
    pub exponent: Vec<Big>,
}

/// What the annual chance of dying depends on besides the rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeathInputs {
    /// Age in whole years.
    pub age: u32,
    /// The character's constitution, in thousandths.
    pub constitution: i64,
    /// The character's longevity, in thousandths.
    pub longevity: i64,
    /// The product of the pop's three death multipliers in ‰ each (`1_000_000_000` when they are all 1).
    pub pressure: u64,
    /// The weight multiplier in thousandths (`1000` for none): see [`DeathRules::weight_multiplier`].
    pub weight: i64,
}

/// `[majority]`: the age at which a character is of age.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Majority {
    /// The ages when the culture's subgroup has none of its own.
    pub default: Ages,
    /// The ages by culture subgroup (CK-356).
    pub by_group: BTreeMap<String, Ages>,
}

impl Majority {
    /// The ages of a culture subgroup.
    #[must_use]
    pub fn of_group(&self, group: &str) -> Ages {
        self.by_group.get(group).copied().unwrap_or(self.default)
    }
}

/// `[relations]`: how willing a couple is to conceive.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelationRules {
    /// The factor on the monthly chance by mode (`trying`, `neutral`, `avoidance`), in ppb.
    pub mode_factor: [u64; 3],
    /// Children a couple wants, by the husband's pop type.
    pub wanted_children: BTreeMap<String, u32>,
    /// The wife's age from which the couple avoids conception.
    pub avoid_age: u32,
    /// The household's necessities (‰) below which the couple avoids conception.
    pub avoid_necessities: u32,
    /// The lust/chastity value (thousandths) at which the mode moves a step.
    pub shift_at: i64,
    /// A lover strand's yearly trying chance, in ppb (read for slice 6; unused here).
    pub lover_trying_ppb: u32,
}

/// `[conception]`: the monthly chance, fecundity by age, nursing and multiple births.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConceptionRules {
    /// The monthly chance before its factors, in ppb.
    pub monthly_ppb: u32,
    /// A woman's fecundity by age: `(age, factor in ppb)` points, linear between.
    pub women: Vec<(u32, u64)>,
    /// A man's fecundity by age.
    pub men: Vec<(u32, u64)>,
    /// Months after a delivery in which she cannot conceive.
    pub nursing_months: u32,
    /// The factor for the days of abstinence of Latin Christianity, in ppb.
    pub church_abstinence_ppb: u64,
    /// The chance of twins at a delivery, in ppb.
    pub twins_ppb: u32,
    /// The chance of triplets at a delivery, in ppb.
    pub triplets_ppb: u32,
    /// The multiplier on both for a mother who shows `twin_bearer`, in thousandths.
    pub twin_bearer_milli: u32,
}

/// The patronymic words of a pool: "son of" and "daughter of".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patronym {
    /// For a son.
    pub m: String,
    /// For a daughter.
    pub f: String,
}

/// A culture's name pool.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NamePool {
    /// The patronymic words, when the culture uses them.
    pub patronym: Option<Patronym>,
    /// Men's names with their genitives, in file order.
    pub men: Vec<(String, String)>,
    /// Women's names, in file order.
    pub women: Vec<String>,
}

/// The kinds of relative a child may be named for, the keys of `[namesakes].order`.
pub const NAMESAKE_KINDS: [&str; 5] = [
    "father",
    "grandfather",
    "uncle",
    "fathers_friend",
    "ancestor",
];

/// `[namesakes]`: the kinds of relative tried in order, each with its base chance (CK-365).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Namesakes {
    /// `(kind, base chance in ppb)` in the order they are tried.
    pub order: Vec<(String, u32)>,
}

/// The loaded life-cycle data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LifeCycle {
    /// `[death]`.
    pub death: DeathRules,
    /// `[majority]`.
    pub majority: Majority,
    /// `[marriageable]`: the ages by religion.
    pub marriageable: BTreeMap<String, Ages>,
    /// `[relations]`.
    pub relations: RelationRules,
    /// `[conception]`.
    pub conception: ConceptionRules,
    /// The name pools by pool key.
    pub pools: BTreeMap<String, NamePool>,
    /// The pool key of each culture.
    pub culture_pool: BTreeMap<String, String>,
    /// `[namesakes]` of `names.toml` (CK-365).
    pub namesakes: Namesakes,
    /// The epithets of `epithets.toml`, in the order the file writes them (CK-365).
    pub epithets: Vec<Epithet>,
}

/// The text of the three files, each with the name errors report it by.
pub struct Files<'a> {
    /// `life-cycle.toml`: `(file name, text)`.
    pub life: (&'a str, &'a str),
    /// `names.toml`.
    pub names: (&'a str, &'a str),
    /// `epithets.toml`.
    pub epithets: (&'a str, &'a str),
}

/// What the life-cycle data is validated against.
pub struct Registry<'a> {
    /// The loaded character events, whose catalog, skills and deed counters an epithet's conditions are checked against (CK-365).
    pub events: &'a EventRules,
    /// The genetic rules the event data was checked against.
    pub genetics: &'a Genetics,
    /// The culture registry.
    pub peoples: &'a Peoples,
    /// The population type keys.
    pub pop_types: &'a [&'a str],
    /// The religion keys.
    pub religions: &'a [String],
}

fn table_keys(
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

fn whole(source: &Source<'_>, node: &Node, entry: &str) -> Result<u32, DataError> {
    u32::try_from(source.integer(node, entry)?).map_err(|_| {
        source.error(
            entry,
            "expected a whole number, not negative and below 2^32",
        )
    })
}

fn positive(source: &Source<'_>, node: &Node, entry: &str) -> Result<u32, DataError> {
    match whole(source, node, entry)? {
        0 => Err(source.error(entry, "the value must be a positive whole number")),
        value => Ok(value),
    }
}

/// A non-negative factor in ppb, not limited to 1 (at most 1000).
fn factor(source: &Source<'_>, node: &Node, entry: &str) -> Result<u64, DataError> {
    let value = source.scaled(node, entry, 9)?;
    u64::try_from(value)
        .ok()
        .filter(|value| *value <= 1000 * u64::from(PPB))
        .ok_or_else(|| source.error(entry, "a factor lies in 0..1000"))
}

/// A signed per-point value in millionths.
fn per_point(source: &Source<'_>, node: &Node, entry: &str) -> Result<i64, DataError> {
    let value = source.scaled(node, entry, 6)?;
    i64::try_from(value)
        .ok()
        .filter(|value| value.unsigned_abs() <= 1_000_000)
        .ok_or_else(|| source.error(entry, "a per-point value lies in -1..1"))
}

fn array<'t>(source: &Source<'_>, node: &'t Node, entry: &str) -> Result<&'t [Node], DataError> {
    match node {
        Node::Array(items) => Ok(items),
        _ => Err(source.error(entry, "expected an array")),
    }
}

fn ages(source: &Source<'_>, node: &Node, entry: &str) -> Result<Ages, DataError> {
    let table = source.table(node, entry)?;
    table_keys(source, table, entry, &["m", "f"])?;
    Ok(Ages {
        m: positive(source, &table["m"], &format!("{entry}.m"))?,
        f: positive(source, &table["f"], &format!("{entry}.f"))?,
    })
}

/// An age curve: `(age, factor)` points with strictly increasing ages and non-negative factors.
fn curve(source: &Source<'_>, node: &Node, entry: &str) -> Result<Vec<(u32, u64)>, DataError> {
    let mut points: Vec<(u32, u64)> = Vec::new();
    for item in array(source, node, entry)? {
        let [age, value] = array(source, item, entry)? else {
            return Err(source.error(entry, "a point is `[age, factor]`"));
        };
        let age = whole(source, age, entry)?;
        let negative = source.scaled(value, entry, 9)? < 0;
        if negative {
            return Err(source.error(entry, "a factor must not be negative"));
        }
        if points.last().is_some_and(|(last, _)| *last >= age) {
            return Err(source.error(entry, "the ages must be strictly increasing"));
        }
        points.push((age, factor(source, value, entry)?));
    }
    if points.is_empty() {
        return Err(source.error(entry, "a curve needs at least one point"));
    }
    Ok(points)
}

/// A table of keys each in `known`, giving `T` for every key of `known` (a complete table).
fn complete<T>(
    source: &Source<'_>,
    node: &Node,
    entry: &str,
    known: &[&str],
    mut parse: impl FnMut(&Node, &str) -> Result<T, DataError>,
) -> Result<BTreeMap<String, T>, DataError> {
    let table = source.table(node, entry)?;
    let mut out = BTreeMap::new();
    for (key, value) in table {
        if !known.contains(&key.as_str()) {
            return Err(source.error(format!("{entry}.{key}"), "unknown key"));
        }
        out.insert(key.clone(), parse(value, &format!("{entry}.{key}"))?);
    }
    for key in known {
        if !out.contains_key(*key) {
            return Err(source.error(format!("{entry}.{key}"), "missing value"));
        }
    }
    Ok(out)
}

fn parse_death(source: &Source<'_>, table: &Table) -> Result<DeathRules, DataError> {
    table_keys(
        source,
        table,
        "death",
        &[
            "infant",
            "infant_years",
            "base",
            "old",
            "old_from",
            "old_doubling",
            "certain_at",
            "longevity_per_point",
            "constitution_per_point",
            "use_pop_curves",
            "weight_low",
            "weight_high",
            "weight_factor",
            "weight_step",
            "childbirth",
        ],
    )?;
    let at = |key: &str| format!("death.{key}");
    let milli = |key: &str| source.milli_non_negative(&table[key], &at(key));
    let death = DeathRules {
        infant_ppb: source.ppb(&table["infant"], "death.infant")?,
        infant_years: positive(source, &table["infant_years"], "death.infant_years")?,
        base_ppb: source.ppb(&table["base"], "death.base")?,
        old_ppb: source.ppb(&table["old"], "death.old")?,
        old_from: whole(source, &table["old_from"], "death.old_from")?,
        old_doubling: positive(source, &table["old_doubling"], "death.old_doubling")?,
        certain_at: positive(source, &table["certain_at"], "death.certain_at")?,
        longevity_per_point: per_point(
            source,
            &table["longevity_per_point"],
            "death.longevity_per_point",
        )?,
        constitution_per_point: per_point(
            source,
            &table["constitution_per_point"],
            "death.constitution_per_point",
        )?,
        use_pop_curves: source.boolean(&table["use_pop_curves"], "death.use_pop_curves")?,
        weight_low: milli("weight_low")?,
        weight_high: milli("weight_high")?,
        weight_factor: milli("weight_factor")?,
        weight_step: milli("weight_step")?,
        childbirth_ppb: source.ppb(&table["childbirth"], "death.childbirth")?,
        tables: DeathTables::default(),
    };
    if death.old_from >= death.certain_at {
        return Err(source.error("death.old_from", "must be below certain_at"));
    }
    for (key, value, most) in [
        ("certain_at", death.certain_at, MOST_CERTAIN_AT),
        ("infant_years", death.infant_years, MOST_YEARS),
        ("old_doubling", death.old_doubling, MOST_YEARS),
    ] {
        if value > most {
            return Err(source.error(at(key), format!("must be at most {most}")));
        }
    }
    if death.weight_low >= death.weight_high {
        return Err(source.error("death.weight_low", "must be below weight_high"));
    }
    let tables = DeathTables::build(&death).map_err(|problem| source.error("death", problem))?;
    Ok(DeathRules { tables, ..death })
}

fn parse_relations(
    source: &Source<'_>,
    table: &Table,
    registry: &Registry<'_>,
) -> Result<RelationRules, DataError> {
    table_keys(
        source,
        table,
        "relations",
        &[
            "mode_factor",
            "wanted_children",
            "avoid_age",
            "avoid_necessities",
            "personality_shift_at",
            "lover_trying",
        ],
    )?;
    let factors = complete(
        source,
        &table["mode_factor"],
        "relations.mode_factor",
        &MODES,
        |node, entry| factor(source, node, entry),
    )?;
    let wanted = complete(
        source,
        &table["wanted_children"],
        "relations.wanted_children",
        registry.pop_types,
        |node, entry| whole(source, node, entry),
    )?;
    Ok(RelationRules {
        mode_factor: [factors["trying"], factors["neutral"], factors["avoidance"]],
        wanted_children: wanted,
        avoid_age: positive(source, &table["avoid_age"], "relations.avoid_age")?,
        avoid_necessities: u32::try_from(
            source.integer(&table["avoid_necessities"], "relations.avoid_necessities")?,
        )
        .ok()
        .filter(|value| *value <= 1000)
        .ok_or_else(|| source.error("relations.avoid_necessities", "must lie in 0..1000"))?,
        shift_at: source.milli_non_negative(
            &table["personality_shift_at"],
            "relations.personality_shift_at",
        )?,
        lover_trying_ppb: source.ppb(&table["lover_trying"], "relations.lover_trying")?,
    })
}

fn parse_conception(source: &Source<'_>, table: &Table) -> Result<ConceptionRules, DataError> {
    table_keys(
        source,
        table,
        "conception",
        &[
            "monthly",
            "women",
            "men",
            "nursing_months",
            "church_abstinence",
            "twins",
            "triplets",
            "twin_bearer",
        ],
    )?;
    let multiplier = source.multiplier(&table["twin_bearer"], "conception.twin_bearer")?;
    Ok(ConceptionRules {
        monthly_ppb: source.ppb(&table["monthly"], "conception.monthly")?,
        women: curve(source, &table["women"], "conception.women")?,
        men: curve(source, &table["men"], "conception.men")?,
        nursing_months: whole(
            source,
            &table["nursing_months"],
            "conception.nursing_months",
        )?,
        church_abstinence_ppb: factor(
            source,
            &table["church_abstinence"],
            "conception.church_abstinence",
        )?,
        twins_ppb: source.ppb(&table["twins"], "conception.twins")?,
        triplets_ppb: source.ppb(&table["triplets"], "conception.triplets")?,
        twin_bearer_milli: multiplier,
    })
}

fn text_list(source: &Source<'_>, node: &Node, entry: &str) -> Result<Vec<String>, DataError> {
    array(source, node, entry)?
        .iter()
        .map(|item| source.text(item, entry).map(str::to_owned))
        .collect()
}

fn parse_pool(source: &Source<'_>, node: &Node, key: &str) -> Result<NamePool, DataError> {
    let entry = format!("pool.{key}");
    let table = source.table(node, &entry)?;
    for name in table.keys() {
        if !["patronym", "men", "women"].contains(&name.as_str()) {
            return Err(source.error(format!("{entry}.{name}"), "unknown key"));
        }
    }
    let field = |name: &str| {
        table
            .get(name)
            .ok_or_else(|| source.error(format!("{entry}.{name}"), "missing value"))
    };
    let patronym_entry = format!("{entry}.patronym");
    let words = source.table(field("patronym")?, &patronym_entry)?;
    let patronym = if words.is_empty() {
        None
    } else {
        table_keys(source, words, &patronym_entry, &["m", "f"])?;
        Some(Patronym {
            m: source.text(&words["m"], &patronym_entry)?.to_owned(),
            f: source.text(&words["f"], &patronym_entry)?.to_owned(),
        })
    };
    let men_entry = format!("{entry}.men");
    let mut men = Vec::new();
    for item in array(source, field("men")?, &men_entry)? {
        let [name, genitive] = array(source, item, &men_entry)? else {
            return Err(source.error(&men_entry, "a man's name is `[name, genitive]`"));
        };
        let (name, genitive) = (
            source.text(name, &men_entry)?,
            source.text(genitive, &men_entry)?,
        );
        if name.is_empty() {
            return Err(source.error(&men_entry, "a name must not be empty"));
        }
        if patronym.is_some() && genitive.is_empty() {
            return Err(source.error(
                &men_entry,
                format!("`{name}` has no genitive, and the pool has a patronym"),
            ));
        }
        men.push((name.to_owned(), genitive.to_owned()));
    }
    let women_entry = format!("{entry}.women");
    let women = text_list(source, field("women")?, &women_entry)?;
    if women.iter().any(String::is_empty) {
        return Err(source.error(&women_entry, "a name must not be empty"));
    }
    if men.is_empty() {
        return Err(source.error(men_entry, "a pool needs men's names"));
    }
    if women.is_empty() {
        return Err(source.error(women_entry, "a pool needs women's names"));
    }
    Ok(NamePool {
        patronym,
        men,
        women,
    })
}

/// `[namesakes]`: `order = [[kind, chance], ...]`, each kind one of [`NAMESAKE_KINDS`] at most once and each chance in 0..1.
fn parse_namesakes(source: &Source<'_>, node: &Node) -> Result<Namesakes, DataError> {
    let table = source.table(node, "namesakes")?;
    for key in table.keys() {
        if key != "order" {
            return Err(source.error(format!("namesakes.{key}"), "unknown key"));
        }
    }
    let entry = "namesakes.order";
    let list = table
        .get("order")
        .ok_or_else(|| source.error(entry, "missing value"))?;
    let mut order: Vec<(String, u32)> = Vec::new();
    for item in array(source, list, entry)? {
        let [kind, chance] = array(source, item, entry)? else {
            return Err(source.error(entry, "an entry is `[kind, chance]`"));
        };
        let kind = source.text(kind, entry)?;
        if !NAMESAKE_KINDS.contains(&kind) {
            return Err(source.error(entry, format!("`{kind}` is not a kind of namesake")));
        }
        if order.iter().any(|(seen, _)| seen == kind) {
            return Err(source.error(entry, format!("`{kind}` is listed twice")));
        }
        order.push((kind.to_owned(), source.ppb(chance, entry)?));
    }
    Ok(Namesakes { order })
}

impl LifeCycle {
    /// Load `life-cycle.toml`, `names.toml` and `epithets.toml` from `directory` and validate them against the registry.
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
        let (life_file, life) = read("life-cycle.toml")?;
        let (names_file, names) = read("names.toml")?;
        let (epithets_file, epithets) = read("epithets.toml")?;
        Self::parse(
            &Files {
                life: (&life_file, &life),
                names: (&names_file, &names),
                epithets: (&epithets_file, &epithets),
            },
            registry,
        )
    }

    /// Parse and validate the three files' text; the file names appear in errors.
    ///
    /// # Errors
    /// Returns a source-qualified error for the first invalid entry.
    pub fn parse(files: &Files<'_>, registry: &Registry<'_>) -> Result<Self, DataError> {
        let (life_file, life) = files.life;
        let (names_file, names) = files.names;
        let (epithets_file, epithets) = files.epithets;
        let source = Source { file: life_file };
        let root = source.document(life)?;
        for key in root.keys() {
            if ![
                "death",
                "majority",
                "marriageable",
                "relations",
                "conception",
                "marriage",
                "upbringing",
                "nutrition",
            ]
            .contains(&key.as_str())
            {
                return Err(source.error(key.clone(), "unknown section"));
            }
        }
        let section = |name: &str| {
            root.get(name)
                .ok_or_else(|| source.error(name, "missing section"))
                .and_then(|node| source.table(node, name))
        };
        let mut rules = Self {
            death: parse_death(&source, section("death")?)?,
            ..Self::default()
        };
        rules.parse_majority(&source, section("majority")?, registry)?;
        rules.marriageable = complete_known(
            &source,
            section("marriageable")?,
            "marriageable",
            registry.religions.iter().map(String::as_str),
        )?;
        rules.relations = parse_relations(&source, section("relations")?, registry)?;
        rules.conception = parse_conception(&source, section("conception")?)?;
        rules.parse_names(names_file, names, registry)?;
        rules.epithets = registry.events.parse_epithets(
            epithets_file,
            epithets,
            &events::Registry {
                peoples: registry.peoples,
                genetics: registry.genetics,
                pop_types: registry.pop_types,
            },
            registry.religions,
        )?;
        Ok(rules)
    }

    fn parse_majority(
        &mut self,
        source: &Source<'_>,
        table: &Table,
        registry: &Registry<'_>,
    ) -> Result<(), DataError> {
        for key in table.keys() {
            if key != "default" && key != "by_culture_group" {
                return Err(source.error(format!("majority.{key}"), "unknown key"));
            }
        }
        let missing = |key: &str| source.error(format!("majority.{key}"), "missing value");
        self.majority.default = ages(
            source,
            table.get("default").ok_or_else(|| missing("default"))?,
            "majority.default",
        )?;
        let groups = source.table(
            table
                .get("by_culture_group")
                .ok_or_else(|| missing("by_culture_group"))?,
            "majority.by_culture_group",
        )?;
        for (group, node) in groups {
            let entry = format!("majority.by_culture_group.{group}");
            if !registry.peoples.subgroups.contains(group) {
                return Err(source.error(entry, "not a culture subgroup of the registry"));
            }
            self.majority
                .by_group
                .insert(group.clone(), ages(source, node, &entry)?);
        }
        Ok(())
    }

    fn parse_names(
        &mut self,
        file: &str,
        text: &str,
        registry: &Registry<'_>,
    ) -> Result<(), DataError> {
        let source = Source { file };
        let root = source.document(text)?;
        for key in root.keys() {
            if !["pool", "culture_pool", "namesakes"].contains(&key.as_str()) {
                return Err(source.error(key.clone(), "unknown section"));
            }
        }
        let pools = source.table(
            root.get("pool")
                .ok_or_else(|| source.error("pool", "missing section"))?,
            "pool",
        )?;
        for (key, node) in pools {
            self.pools
                .insert(key.clone(), parse_pool(&source, node, key)?);
        }
        let assignments = source.table(
            root.get("culture_pool")
                .ok_or_else(|| source.error("culture_pool", "missing section"))?,
            "culture_pool",
        )?;
        for (culture, node) in assignments {
            let entry = format!("culture_pool.{culture}");
            if !registry.peoples.cultures.iter().any(|c| c.id == *culture) {
                return Err(source.error(entry, "not a culture of the registry"));
            }
            let pool = source.text(node, &entry)?;
            if !self.pools.contains_key(pool) {
                return Err(source.error(entry, format!("`{pool}` is not a pool")));
            }
            self.culture_pool.insert(culture.clone(), pool.to_owned());
        }
        for culture in &registry.peoples.cultures {
            if !self.culture_pool.contains_key(&culture.id) {
                return Err(source.error(
                    format!("culture_pool.{}", culture.id),
                    "the culture has no name pool",
                ));
            }
        }
        self.namesakes = parse_namesakes(
            &source,
            root.get("namesakes")
                .ok_or_else(|| source.error("namesakes", "missing section"))?,
        )?;
        Ok(())
    }

    /// The pool key of a culture, when it has one.
    #[must_use]
    pub fn pool_key(&self, culture: &str) -> Option<&str> {
        self.culture_pool.get(culture).map(String::as_str)
    }
}

/// The `[marriageable]` table: ages for religions, each a known religion (a religion may be left out).
fn complete_known<'a>(
    source: &Source<'_>,
    table: &Table,
    section: &str,
    known: impl Iterator<Item = &'a str>,
) -> Result<BTreeMap<String, Ages>, DataError> {
    let known: Vec<&str> = known.collect();
    let mut out = BTreeMap::new();
    for (key, node) in table {
        let entry = format!("{section}.{key}");
        if !known.contains(&key.as_str()) {
            return Err(source.error(entry, "not a religion of the rules"));
        }
        out.insert(key.clone(), ages(source, node, &entry)?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------------------
// Death arithmetic (CK-364 §2). No floating point: the tables and the chance are exact integers over big-integer fractions.

impl DeathTables {
    /// Build both tables from the rules.
    ///
    /// `E1 = e^(-1 / infant_years)` is the alternating series summed exactly as a rational until the next term is below 10^-12,
    /// then rounded down to ppb; `E(a) = E1^a`, rounded down after each multiplication. `R` is the largest ppb value with
    /// `R^old_doubling <= 2` (an exact big-integer comparison); `P(k) = R^k`, rounded down after each multiplication.
    ///
    /// # Errors
    /// A message when an intermediate does not fit.
    pub fn build(rules: &DeathRules) -> Result<Self, String> {
        let one = ONE;
        let e1 = series_e1(rules.infant_years)?;
        let mut infant = vec![one];
        for age in 1..=rules.certain_at {
            let previous = infant[usize::try_from(age - 1).map_err(|e| e.to_string())?];
            infant.push(previous * e1 / one);
        }
        let doubling = rules.old_doubling;
        let limit = Big::new(2).mul(&Big::new(one).pow(doubling));
        let (mut low, mut high) = (one, 2 * one);
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            if Big::new(mid).pow(doubling).cmp(&limit) == std::cmp::Ordering::Greater {
                high = mid - 1;
            } else {
                low = mid;
            }
        }
        let root = low;
        let most = rules.old_from.max(rules.certain_at - rules.old_from);
        // `P(k) = P(k - 1) x R / 10^9`, rounded down, in unbounded integers, and kept so: a short doubling over a long span of ages
        // makes the powers larger than any fixed width, and the table holds them exactly.
        let mut exponent = vec![Big::new(one)];
        for _ in 1..=most {
            let previous = exponent.last().cloned().unwrap_or_default();
            let (next, _) = previous.mul(&Big::new(root)).divmod(&Big::new(one));
            exponent.push(next);
        }
        let infant = infant
            .into_iter()
            .map(|value| u64::try_from(value).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { infant, exponent })
    }
}

/// `e^(-1 / years)` in ppb, rounded down: the series `sum (-1)^n / (years^n n!)` taken exactly up to the term before the first that is
/// below 10^-12.
fn series_e1(years: u32) -> Result<u128, String> {
    let years = u128::from(years);
    // The last index N whose successor's term `1 / (years^(N+1) (N+1)!)` is below 10^-12.
    let (mut last, mut size) = (0_u128, 1_u128);
    loop {
        let next = last + 1;
        size = size
            .checked_mul(years)
            .and_then(|s| s.checked_mul(next))
            .ok_or("the infant series does not fit")?;
        if size > 1_000_000_000_000 {
            break;
        }
        last = next;
    }
    // Over the common denominator `years^N N!` the term n is `years^(N-n) (N! / n!)`.
    let denominator = Big::new(years)
        .pow(u32::try_from(last).map_err(|e| e.to_string())?)
        .mul(&Big::new(factorial(1, last)?));
    let (mut even, mut odd) = (Big::new(0), Big::new(0));
    for n in 0..=last {
        let term = Big::new(years)
            .pow(u32::try_from(last - n).map_err(|e| e.to_string())?)
            .mul(&Big::new(factorial(n + 1, last)?));
        if n % 2 == 0 {
            even = even.add(&term);
        } else {
            odd = odd.add(&term);
        }
    }
    let sum = even.sub(&odd).ok_or("the infant series is negative")?;
    let (rounded, _) = sum.mul(&Big::new(ONE)).divmod(&denominator);
    rounded
        .to_u128()
        .ok_or_else(|| "e1 does not fit".to_owned())
}

/// `from x (from + 1) x ... x to` (1 when `from > to`).
fn factorial(from: u128, to: u128) -> Result<u128, String> {
    (from..=to).try_fold(1_u128, |product, k| {
        product
            .checked_mul(k)
            .ok_or_else(|| "the infant series does not fit".to_owned())
    })
}

impl DeathRules {
    /// `E(a)`: the infant term's factor at an age, in ppb.
    #[must_use]
    pub fn infant_factor(&self, age: u32) -> u64 {
        let last = self.tables.infant.len().saturating_sub(1);
        self.tables
            .infant
            .get(usize::try_from(age).unwrap_or(last).min(last))
            .copied()
            .unwrap_or(0)
    }

    /// `G(a)`: the old-age term's factor at an age, in ppb: `P(a - old_from)` from `old_from` on, and `10^18 / P(old_from - a)` before.
    #[must_use]
    pub fn old_age_factor(&self, age: u32) -> Big {
        let power = |k: u32| {
            usize::try_from(k)
                .ok()
                .and_then(|k| self.tables.exponent.get(k))
                .cloned()
                .unwrap_or_else(|| Big::new(1))
        };
        if age >= self.old_from {
            power(age - self.old_from)
        } else {
            let (quotient, _) =
                Big::new(u128::from(PPB) * u128::from(PPB)).divmod(&power(self.old_from - age));
            quotient
        }
    }

    /// The annual chance of dying in ppb: `infant x E(a) + base x c x K + old x G(a) x l x K`, multiplied by the weight multiplier and
    /// capped at 1, as one exact rational rounded down once. `c = max(0.3, 1 + constitution_per_point x (constitution - 10))` and
    /// `l` likewise with longevity. From `certain_at` on the chance is 1.
    #[must_use]
    pub fn annual_ppb(&self, inputs: &DeathInputs) -> u32 {
        if inputs.age >= self.certain_at {
            return PPB;
        }
        let scale = |per_point: i64, value: i64| -> u128 {
            // `1 + per_point x (value - 10)` over 10^9, held at 0.3 or above: `per_point` in millionths, `value` in thousandths.
            let raw = 1_000_000_000_i128 + i128::from(per_point) * (i128::from(value) - 10_000);
            u128::try_from(raw.max(300_000_000)).unwrap_or(300_000_000)
        };
        let pressure = if self.use_pop_curves {
            u128::from(inputs.pressure)
        } else {
            ONE
        };
        let infant = Big::new(u128::from(self.infant_ppb))
            .mul(&Big::new(u128::from(self.infant_factor(inputs.age))))
            .mul(&Big::new(ONE * ONE));
        let base = Big::new(u128::from(self.base_ppb))
            .mul(&Big::new(scale(
                self.constitution_per_point,
                inputs.constitution,
            )))
            .mul(&Big::new(pressure))
            .mul(&Big::new(ONE));
        let old = Big::new(u128::from(self.old_ppb))
            .mul(&self.old_age_factor(inputs.age))
            .mul(&Big::new(scale(self.longevity_per_point, inputs.longevity)))
            .mul(&Big::new(pressure));
        let weight = Big::new(u128::try_from(inputs.weight.max(0)).unwrap_or(0));
        let numerator = infant.add(&base).add(&old).mul(&weight);
        let denominator = Big::new(ONE).pow(4).mul(&Big::new(1000));
        if numerator.cmp(&denominator) != std::cmp::Ordering::Less {
            return PPB;
        }
        let (floor, _) = numerator.mul(&Big::new(ONE)).divmod(&denominator);
        floor
            .to_u128()
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or(PPB)
    }

    /// The multiplier on the whole annual chance when the shown weight is an extreme of the rolled weight, in thousandths (`1000`
    /// for none): below `weight_low` or above `weight_high` of it, `weight_factor + weight_step x` the whole 5% steps further out.
    #[must_use]
    pub fn weight_multiplier(&self, shown: i64, rolled: i64) -> i64 {
        if rolled <= 0 {
            return 1000;
        }
        let (shown, rolled) = (i128::from(shown) * 1000, i128::from(rolled));
        let (low, high) = (
            i128::from(self.weight_low) * rolled,
            i128::from(self.weight_high) * rolled,
        );
        let beyond = if shown < low {
            low - shown
        } else if shown > high {
            shown - high
        } else {
            return 1000;
        };
        let steps = beyond / (50 * rolled);
        i64::try_from(i128::from(self.weight_factor) + i128::from(self.weight_step) * steps)
            .unwrap_or(i64::MAX)
    }

    /// The monthly chance in ppb that twelve rolls give at least the annual chance `annual`: the smallest `m` with
    /// `(10^9 - m)^12 <= (10^9 - annual) x 10^99`, tested exactly. Twelve rolls overshoot the annual chance by less than twelve ppb.
    #[must_use]
    pub fn monthly_ppb(annual: u32) -> u32 {
        let target = Big::new(u128::from(PPB - annual.min(PPB))).mul(&Big::new(ONE).pow(11));
        let (mut low, mut high) = (0_u32, annual.min(PPB));
        while low < high {
            let mid = low + (high - low) / 2;
            if Big::new(u128::from(PPB - mid)).pow(12).cmp(&target) == std::cmp::Ordering::Greater {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low
    }
}
