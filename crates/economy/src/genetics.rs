//! Genetic data (haplogroup tables and trait rules) and the genetic state a pop carries (CK-361).
//!
//! Nothing here uses floating point. Stats, humours, spreads, heritabilities and the inheritance parameters are integers
//! in thousandths; probabilities and rates are integers in parts per billion (ppb). Both TOML files are read as text:
//! a decimal is parsed exactly from its digits, never through a binary float.
#![deny(clippy::float_arithmetic)]

use crate::{DataError, Peoples};
use ck_sim::{Canonical, CanonicalError, Reader};
use std::{collections::BTreeMap, fs, path::Path};
use toml::de::{DeTable, DeValue};

/// The nine stats, in canonical order. Indices `0..9` of an attribute array.
pub const STATS: [&str; 9] = [
    "height",
    "weight",
    "strength",
    "wit",
    "intellect",
    "cunning",
    "longevity",
    "constitution",
    "fecundity",
];
/// The four humours, in canonical order. Indices `9..13` of an attribute array.
pub const HUMOURS: [&str; 4] = ["blood", "yellow_bile", "black_bile", "phlegm"];
/// Number of stats plus humours.
pub const ATTRIBUTE_COUNT: usize = 13;
/// Stats then humours: the order of every attribute array.
pub const ATTRIBUTES: [&str; ATTRIBUTE_COUNT] = [
    "height",
    "weight",
    "strength",
    "wit",
    "intellect",
    "cunning",
    "longevity",
    "constitution",
    "fecundity",
    "blood",
    "yellow_bile",
    "black_bile",
    "phlegm",
];
/// One in parts per billion.
pub const PPB: u32 = 1_000_000_000;
/// The haplogroup name a start set uses for the unlisted lineages (the `[default]` table).
pub const OTHER: &str = "other";
/// The highest graded level a trait may have.
pub const MAX_GRADED_LEVEL: u8 = 6;
const SHARE_TOTAL: u32 = 1000;

/// The index of a stat or humour in an attribute array.
#[must_use]
pub fn attribute_index(name: &str) -> Option<usize> {
    ATTRIBUTES.iter().position(|item| *item == name)
}

/// The shown or genetic value of attribute `attribute`, held to its bounds (§3): humours 0..10 at both ends, the other
/// stats a floor of 1 and no cap, height a floor of 100 cm and weight a floor of 30 kg. Values are in thousandths.
#[must_use]
pub fn clamp_attribute(attribute: usize, value: i64) -> i64 {
    match attribute {
        0 => value.max(100_000),
        1 => value.max(30_000),
        2..=8 => value.max(1_000),
        _ => value.clamp(0, 10_000),
    }
}

/// `numerator / denominator` rounded to the nearest integer, ties away from zero. `denominator` must be positive.
#[must_use]
pub fn round_div(numerator: i128, denominator: i128) -> i128 {
    let negative = numerator < 0;
    let n = numerator.unsigned_abs();
    let d = denominator.unsigned_abs();
    let quotient = n / d;
    let remainder = n % d;
    let rounded = if remainder >= d - remainder {
        quotient + 1
    } else {
        quotient
    };
    let signed = i128::try_from(rounded).unwrap_or(i128::MAX);
    if negative { -signed } else { signed }
}

/// The spread factor of a child's draw (§1), in thousandths: the largest integer `f >= 0` with
/// `2 f^2 <= 2,000,000 - h^2` for a heritability `h` in thousandths, that is `floor(sqrt(1,000,000 - h^2 / 2))` computed
/// exactly (h = 800 gives 824, h = 1 gives 999, h = 0 gives 1000, h = 1000 gives 707).
#[must_use]
pub fn spread_factor(heritability_milli: u32) -> u32 {
    let h = u64::from(heritability_milli);
    let limit = 2_000_000_u64.saturating_sub(h * h);
    let mut f = 0_u64;
    while 2 * (f + 1) * (f + 1) <= limit {
        f += 1;
    }
    u32::try_from(f).unwrap_or(u32::MAX)
}

/// A mean and a spread, in thousandths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Distribution {
    /// Mean, in thousandths.
    pub mean: i64,
    /// Spread (standard deviation), in thousandths.
    pub sd: i64,
}

/// One haplogroup table: only what differs from the default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HaplogroupTable {
    /// Mean overrides by attribute index, in thousandths.
    pub mean: [Option<i64>; ATTRIBUTE_COUNT],
    /// Spread overrides by attribute index, in thousandths.
    pub sd: [Option<i64>; ATTRIBUTE_COUNT],
    /// Founder-rate multipliers by trait, in thousandths (not limited to 1).
    pub trait_rates: BTreeMap<String, u32>,
}

/// A start set: the paternal and maternal haplogroup shares of a starting pop, in thousandths summing to 1000 each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartSet {
    /// Paternal shares by haplogroup; `other` is the default table.
    pub paternal: BTreeMap<String, u32>,
    /// Maternal shares by haplogroup; `other` is the default table.
    pub maternal: BTreeMap<String, u32>,
}

/// How a Mendelian condition is inherited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Autosomal: two copies, one from each parent.
    Ordinary,
    /// X-linked: women two copies, men one; a son gets one of his mother's.
    X,
    /// Father to every son.
    Y,
    /// The mother's line: from a mother to every child, passed on only by daughters.
    Maternal,
}

/// Whether one copy shows or two are needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dominance {
    /// One copy shows.
    Dominant,
    /// Two copies show (a man's single X copy counts).
    Recessive,
}

/// Which sex shows a condition; both always carry it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowsIn {
    /// Either sex.
    Both,
    /// Women only.
    Women,
    /// Men only.
    Men,
}

/// A Mendelian condition's rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mendelian {
    /// Inheritance mode.
    pub mode: Mode,
    /// Dominance (unused for `y` and `maternal`).
    pub dominance: Dominance,
    /// Chance, in ppb, that a character who can show the condition does.
    pub penetrance: u32,
    /// A founder's chance per copy of carrying the condition, in ppb.
    pub allele: u32,
    /// Which sex shows it.
    pub shows_in: ShowsIn,
}

/// A graded trait's rules (levels 0 = none, then 1 up to `max`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Graded {
    /// The mother's weight, in thousandths.
    pub w: u32,
    /// The pull towards none, in thousandths of a level.
    pub d: u32,
    /// Base spread, in thousandths of a level.
    pub s0: u32,
    /// Spread added per level of parental difference, in thousandths.
    pub k: u32,
    /// Chance of a new trait, in ppb.
    pub m: u32,
    /// Each new level is `q` times the one below, in ppb.
    pub q: u32,
    /// The highest level, 1..=6.
    pub max: u8,
    /// A founder's chance of the trait, in ppb.
    pub founder: u32,
    /// Each founder level is `founder_q` times the one below, in ppb.
    pub founder_q: u32,
}

/// The loaded genetic rules: haplogroup tables, start sets, the culture map and the trait rules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Genetics {
    default: [Distribution; ATTRIBUTE_COUNT],
    female: [i64; ATTRIBUTE_COUNT],
    heritability: [u32; ATTRIBUTE_COUNT],
    spread: [u32; ATTRIBUTE_COUNT],
    paternal: BTreeMap<String, HaplogroupTable>,
    maternal: BTreeMap<String, HaplogroupTable>,
    start: BTreeMap<String, StartSet>,
    culture_start: BTreeMap<String, String>,
    mendelian: BTreeMap<String, Mendelian>,
    graded: BTreeMap<String, Graded>,
}

/// The genetic state of one pop: baselines, haplogroup shares and trait rates. Seeded at the start from the pop's culture,
/// then moved only by people-weighted averaging; the data tables are never read for a pop again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopGenetics {
    /// Baseline mean per attribute, in thousandths.
    pub mean: [i64; ATTRIBUTE_COUNT],
    /// Baseline spread per attribute, in thousandths.
    pub sd: [i64; ATTRIBUTE_COUNT],
    /// Paternal haplogroup shares, thousandths summing to 1000 (empty until seeded).
    pub paternal: BTreeMap<String, u32>,
    /// Maternal haplogroup shares, thousandths summing to 1000 (empty until seeded).
    pub maternal: BTreeMap<String, u32>,
    /// Carrier rate per Mendelian condition, in ppb.
    pub carrier: BTreeMap<String, u32>,
    /// Founder rate per graded trait, in ppb.
    pub founder: BTreeMap<String, u32>,
}

// ---------------------------------------------------------------------------------------------------------------------
// Parsing

#[derive(Debug)]
pub(crate) enum Node {
    Int(String),
    Float(String),
    Text(String),
    Array(Vec<Node>),
    Table(BTreeMap<String, Node>),
    Bool(bool),
    Other(&'static str),
}
pub(crate) type Table = BTreeMap<String, Node>;

fn convert(value: &DeValue<'_>) -> Node {
    match value {
        DeValue::String(text) => Node::Text(text.as_ref().to_owned()),
        DeValue::Integer(number) if number.radix() == 10 => Node::Int(number.as_str().to_owned()),
        DeValue::Integer(_) => Node::Other("a non-decimal integer"),
        DeValue::Float(number) => Node::Float(number.as_str().to_owned()),
        DeValue::Boolean(value) => Node::Bool(*value),
        DeValue::Datetime(_) => Node::Other("a datetime"),
        DeValue::Array(items) => {
            Node::Array(items.iter().map(|item| convert(item.get_ref())).collect())
        }
        DeValue::Table(table) => Node::Table(convert_table(table)),
    }
}

fn convert_table(table: &DeTable<'_>) -> Table {
    table
        .iter()
        .map(|(key, value)| (key.get_ref().as_ref().to_owned(), convert(value.get_ref())))
        .collect()
}

pub(crate) struct Source<'a> {
    pub(crate) file: &'a str,
}

impl Source<'_> {
    pub(crate) fn error(&self, entry: impl Into<String>, problem: impl Into<String>) -> DataError {
        DataError {
            file: self.file.to_owned(),
            entry: entry.into(),
            problem: problem.into(),
        }
    }

    pub(crate) fn document(&self, text: &str) -> Result<Table, DataError> {
        let table = DeTable::parse(text)
            .map_err(|error| self.error("<document>", error.message().to_owned()))?;
        Ok(convert_table(table.get_ref()))
    }

    pub(crate) fn table<'t>(&self, node: &'t Node, entry: &str) -> Result<&'t Table, DataError> {
        match node {
            Node::Table(table) => Ok(table),
            other => Err(self.error(
                entry,
                format!("expected a table, found {}", describe(other)),
            )),
        }
    }

    pub(crate) fn text<'t>(&self, node: &'t Node, entry: &str) -> Result<&'t str, DataError> {
        match node {
            Node::Text(text) => Ok(text),
            other => Err(self.error(
                entry,
                format!("expected a string, found {}", describe(other)),
            )),
        }
    }

    /// A number as an exact scaled integer (`scale` decimal places).
    pub(crate) fn scaled(&self, node: &Node, entry: &str, scale: usize) -> Result<i128, DataError> {
        match node {
            Node::Int(text) | Node::Float(text) => {
                decimal(text, scale).map_err(|problem| self.error(entry, problem))
            }
            other => Err(self.error(
                entry,
                format!("expected a number, found {}", describe(other)),
            )),
        }
    }

    /// A probability or rate in ppb: 0..=1, at most nine decimal places.
    pub(crate) fn ppb(&self, node: &Node, entry: &str) -> Result<u32, DataError> {
        let value = self.scaled(node, entry, 9)?;
        u32::try_from(value)
            .ok()
            .filter(|value| *value <= PPB)
            .ok_or_else(|| self.error(entry, "a probability must lie in 0..1"))
    }

    /// A fraction in thousandths: 0..=1.
    pub(crate) fn unit_milli(&self, node: &Node, entry: &str) -> Result<u32, DataError> {
        let value = self.scaled(node, entry, 3)?;
        u32::try_from(value)
            .ok()
            .filter(|value| *value <= 1000)
            .ok_or_else(|| self.error(entry, "the value must lie in 0..1"))
    }

    /// A non-negative quantity in thousandths (a spread or a parameter).
    pub(crate) fn milli_non_negative(&self, node: &Node, entry: &str) -> Result<i64, DataError> {
        let value = self.scaled(node, entry, 3)?;
        if value < 0 {
            return Err(self.error(entry, "the value must not be negative"));
        }
        i64::try_from(value).map_err(|_| self.error(entry, "the value is too large"))
    }

    pub(crate) fn milli(&self, node: &Node, entry: &str) -> Result<i64, DataError> {
        let value = self.scaled(node, entry, 3)?;
        i64::try_from(value).map_err(|_| self.error(entry, "the value is too large"))
    }

    /// A non-negative multiplier in thousandths, not limited to 1.
    pub(crate) fn multiplier(&self, node: &Node, entry: &str) -> Result<u32, DataError> {
        let value = self.scaled(node, entry, 3)?;
        if value < 0 {
            return Err(self.error(entry, "a multiplier must not be negative"));
        }
        u32::try_from(value).map_err(|_| self.error(entry, "the multiplier is too large"))
    }

    pub(crate) fn boolean(&self, node: &Node, entry: &str) -> Result<bool, DataError> {
        match node {
            Node::Bool(value) => Ok(*value),
            other => Err(self.error(
                entry,
                format!("expected true or false, found {}", describe(other)),
            )),
        }
    }

    pub(crate) fn integer(&self, node: &Node, entry: &str) -> Result<i128, DataError> {
        match node {
            Node::Int(text) => decimal(text, 0).map_err(|problem| self.error(entry, problem)),
            other => Err(self.error(
                entry,
                format!("expected an integer, found {}", describe(other)),
            )),
        }
    }
}

fn describe(node: &Node) -> &'static str {
    match node {
        Node::Int(_) => "an integer",
        Node::Float(_) => "a float",
        Node::Text(_) => "a string",
        Node::Table(_) => "a table",
        Node::Array(_) => "an array",
        Node::Bool(_) => "a boolean",
        Node::Other(what) => what,
    }
}

/// Parse a plain decimal (`-12`, `0.0001`) exactly to an integer with `scale` decimal places. A value with more decimal
/// places than `scale`, an exponent, `inf`/`nan` or a digit separator is refused.
fn decimal(text: &str, scale: usize) -> Result<i128, String> {
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
    let plain = !whole.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && !body.ends_with('.');
    if !plain {
        return Err(format!("`{text}` is not a plain decimal number"));
    }
    if fraction.len() > scale {
        return Err(format!("`{text}` has more than {scale} decimal places"));
    }
    let mut value = 0_i128;
    for digit in whole.bytes().chain(fraction.bytes()) {
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add(i128::from(digit - b'0')))
            .ok_or_else(|| format!("`{text}` is too large"))?;
    }
    for _ in fraction.len()..scale {
        value = value
            .checked_mul(10)
            .ok_or_else(|| format!("`{text}` is too large"))?;
    }
    Ok(if negative { -value } else { value })
}

fn attribute(source: &Source<'_>, name: &str, entry: &str) -> Result<usize, DataError> {
    attribute_index(name)
        .ok_or_else(|| source.error(entry, format!("`{name}` is not a stat or humour")))
}

// --- traits.toml ----------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct PartialGraded {
    w: Option<u32>,
    d: Option<u32>,
    s0: Option<u32>,
    k: Option<u32>,
    m: Option<u32>,
    q: Option<u32>,
    max: Option<u8>,
    founder: Option<u32>,
    founder_q: Option<u32>,
}

fn milli_u32(source: &Source<'_>, node: &Node, entry: &str) -> Result<u32, DataError> {
    u32::try_from(source.milli_non_negative(node, entry)?)
        .map_err(|_| source.error(entry, "the value is too large"))
}

impl PartialGraded {
    fn apply(
        &mut self,
        source: &Source<'_>,
        table: &Table,
        entry: &str,
        allow_founder: bool,
    ) -> Result<(), DataError> {
        for (key, node) in table {
            let at = format!("{entry}.{key}");
            match key.as_str() {
                "w" => self.w = Some(source.unit_milli(node, &at)?),
                "d" => self.d = Some(milli_u32(source, node, &at)?),
                "s0" => self.s0 = Some(milli_u32(source, node, &at)?),
                "k" => self.k = Some(milli_u32(source, node, &at)?),
                "m" => self.m = Some(source.ppb(node, &at)?),
                "q" => self.q = Some(source.ppb(node, &at)?),
                "founder_q" => self.founder_q = Some(source.ppb(node, &at)?),
                "founder" if allow_founder => self.founder = Some(source.ppb(node, &at)?),
                "max" => {
                    let level = source.integer(node, &at)?;
                    self.max = Some(
                        u8::try_from(level)
                            .ok()
                            .filter(|level| (1..=MAX_GRADED_LEVEL).contains(level))
                            .ok_or_else(|| source.error(&at, "max must lie in 1..6"))?,
                    );
                }
                _ => return Err(source.error(&at, "unknown graded key")),
            }
        }
        Ok(())
    }

    fn finish(self, source: &Source<'_>, entry: &str) -> Result<Graded, DataError> {
        let need = |value: Option<u32>, key: &str| {
            value.ok_or_else(|| source.error(format!("{entry}.{key}"), "missing value"))
        };
        Ok(Graded {
            w: need(self.w, "w")?,
            d: need(self.d, "d")?,
            s0: need(self.s0, "s0")?,
            k: need(self.k, "k")?,
            m: need(self.m, "m")?,
            q: need(self.q, "q")?,
            max: self
                .max
                .ok_or_else(|| source.error(format!("{entry}.max"), "missing value"))?,
            founder: need(self.founder, "founder")?,
            founder_q: need(self.founder_q, "founder_q")?,
        })
    }
}

type TraitRules = (BTreeMap<String, Mendelian>, BTreeMap<String, Graded>);

fn parse_traits(source: &Source<'_>, text: &str) -> Result<TraitRules, DataError> {
    let root = source.document(text)?;
    for key in root.keys() {
        if !["defaults", "graded", "mendelian"].contains(&key.as_str()) {
            return Err(source.error(key, "unknown section"));
        }
    }
    let mut defaults = PartialGraded::default();
    if let Some(node) = root.get("defaults") {
        let table = source.table(node, "defaults")?;
        for (key, inner) in table {
            if key != "graded" {
                return Err(source.error(format!("defaults.{key}"), "unknown section"));
            }
            defaults.apply(
                source,
                source.table(inner, "defaults.graded")?,
                "defaults.graded",
                false,
            )?;
        }
    }
    let mut graded = BTreeMap::new();
    if let Some(node) = root.get("graded") {
        for (name, trait_node) in source.table(node, "graded")? {
            let entry = format!("graded.{name}");
            let mut partial = defaults;
            partial.apply(source, source.table(trait_node, &entry)?, &entry, true)?;
            graded.insert(name.clone(), partial.finish(source, &entry)?);
        }
    }
    let mut mendelian = BTreeMap::new();
    if let Some(node) = root.get("mendelian") {
        for (name, trait_node) in source.table(node, "mendelian")? {
            if graded.contains_key(name) {
                return Err(source.error(format!("mendelian.{name}"), "a trait is also graded"));
            }
            let entry = format!("mendelian.{name}");
            mendelian.insert(
                name.clone(),
                parse_mendelian(source, source.table(trait_node, &entry)?, &entry)?,
            );
        }
    }
    Ok((mendelian, graded))
}

fn parse_mendelian(
    source: &Source<'_>,
    table: &Table,
    entry: &str,
) -> Result<Mendelian, DataError> {
    let mut mode = None;
    let mut dominance = None;
    let mut penetrance = None;
    let mut allele = None;
    let mut shows_in = ShowsIn::Both;
    for (key, node) in table {
        let at = format!("{entry}.{key}");
        match key.as_str() {
            "mode" => {
                mode = Some(match source.text(node, &at)? {
                    "ordinary" => Mode::Ordinary,
                    "x" => Mode::X,
                    "y" => Mode::Y,
                    "maternal" => Mode::Maternal,
                    other => return Err(source.error(&at, format!("unknown mode `{other}`"))),
                });
            }
            "dominance" => {
                dominance = Some(match source.text(node, &at)? {
                    "dominant" => Dominance::Dominant,
                    "recessive" => Dominance::Recessive,
                    other => return Err(source.error(&at, format!("unknown dominance `{other}`"))),
                });
            }
            "penetrance" => penetrance = Some(source.ppb(node, &at)?),
            "allele" => allele = Some(source.ppb(node, &at)?),
            "shows_in" => {
                shows_in = match source.text(node, &at)? {
                    "both" => ShowsIn::Both,
                    "women" => ShowsIn::Women,
                    "men" => ShowsIn::Men,
                    other => return Err(source.error(&at, format!("unknown shows_in `{other}`"))),
                };
            }
            _ => return Err(source.error(&at, "unknown mendelian key")),
        }
    }
    let missing = |key: &str| source.error(format!("{entry}.{key}"), "missing value");
    let mode = mode.ok_or_else(|| missing("mode"))?;
    let dominance = match (dominance, mode) {
        (Some(dominance), _) => dominance,
        (None, Mode::Y | Mode::Maternal) => Dominance::Dominant,
        (None, _) => return Err(missing("dominance")),
    };
    Ok(Mendelian {
        mode,
        dominance,
        penetrance: penetrance.ok_or_else(|| missing("penetrance"))?,
        allele: allele.ok_or_else(|| missing("allele"))?,
        shows_in,
    })
}

// --- haplogroups.toml -----------------------------------------------------------------------------------------------

impl Genetics {
    /// Load `haplogroups.toml` and `traits.toml` from `directory` and validate them against the culture registry.
    ///
    /// # Errors
    /// Returns a source-qualified error for unreadable or invalid data.
    pub fn load(directory: &Path, peoples: &Peoples) -> Result<Self, DataError> {
        let read = |name: &str| {
            let path = directory.join(name);
            fs::read_to_string(&path)
                .map(|text| (path.display().to_string(), text))
                .map_err(|error| DataError {
                    file: path.display().to_string(),
                    entry: "<file>".to_owned(),
                    problem: error.to_string(),
                })
        };
        let (haplogroup_file, haplogroups) = read("haplogroups.toml")?;
        let (traits_file, traits) = read("traits.toml")?;
        let cultures: Vec<&str> = peoples.cultures.iter().map(|c| c.id.as_str()).collect();
        Self::parse(
            (&haplogroup_file, &haplogroups),
            (&traits_file, &traits),
            &cultures,
        )
    }

    /// Parse and validate the two files' text, each given with the name used in errors.
    ///
    /// # Errors
    /// Returns a source-qualified error for the first invalid entry.
    pub fn parse(
        haplogroups: (&str, &str),
        traits: (&str, &str),
        cultures: &[&str],
    ) -> Result<Self, DataError> {
        let (mendelian, graded) = parse_traits(&Source { file: traits.0 }, traits.1)?;
        let source = Source {
            file: haplogroups.0,
        };
        let root = source.document(haplogroups.1)?;
        for key in root.keys() {
            if ![
                "default",
                "female",
                "heritability",
                "paternal",
                "maternal",
                "start",
                "culture_start",
            ]
            .contains(&key.as_str())
            {
                return Err(source.error(key, "unknown section"));
            }
        }
        let empty = Table::new();
        let section = |name: &str| -> Result<&Table, DataError> {
            root.get(name)
                .map_or(Ok(&empty), |node| source.table(node, name))
        };
        let mut rules = Self {
            mendelian,
            graded,
            ..Self::default()
        };
        rules.parse_default(&source, section("default")?)?;
        rules.parse_female(&source, section("female")?)?;
        rules.parse_heritability(&source, section("heritability")?)?;
        let is_trait =
            |name: &str| rules.mendelian.contains_key(name) || rules.graded.contains_key(name);
        let paternal = parse_tables(&source, section("paternal")?, "paternal", &is_trait)?;
        let maternal = parse_tables(&source, section("maternal")?, "maternal", &is_trait)?;
        rules.paternal = paternal;
        rules.maternal = maternal;
        rules.parse_start(&source, section("start")?)?;
        rules.parse_culture_start(&source, section("culture_start")?, cultures)?;
        Ok(rules)
    }

    fn parse_default(&mut self, source: &Source<'_>, table: &Table) -> Result<(), DataError> {
        let mut seen = [false; ATTRIBUTE_COUNT];
        for (name, node) in table {
            let entry = format!("default.{name}");
            let index = attribute(source, name, &entry)?;
            let inner = source.table(node, &entry)?;
            let mut mean = None;
            let mut sd = None;
            for (key, value) in inner {
                let at = format!("{entry}.{key}");
                match key.as_str() {
                    "mean" => mean = Some(source.milli(value, &at)?),
                    "sd" => sd = Some(source.milli_non_negative(value, &at)?),
                    _ => return Err(source.error(&at, "expected `mean` or `sd`")),
                }
            }
            self.default[index] = Distribution {
                mean: mean.ok_or_else(|| source.error(format!("{entry}.mean"), "missing value"))?,
                sd: sd.ok_or_else(|| source.error(format!("{entry}.sd"), "missing value"))?,
            };
            seen[index] = true;
        }
        for (index, name) in ATTRIBUTES.iter().enumerate() {
            if !seen[index] {
                return Err(source.error(format!("default.{name}"), "missing default"));
            }
        }
        Ok(())
    }

    fn parse_female(&mut self, source: &Source<'_>, table: &Table) -> Result<(), DataError> {
        for (name, node) in table {
            let entry = format!("female.{name}");
            let index = attribute(source, name, &entry)?;
            self.female[index] = source.milli(node, &entry)?;
        }
        Ok(())
    }

    fn parse_heritability(&mut self, source: &Source<'_>, table: &Table) -> Result<(), DataError> {
        let mut seen = [false; ATTRIBUTE_COUNT];
        for (name, node) in table {
            let entry = format!("heritability.{name}");
            let index = attribute(source, name, &entry)?;
            self.heritability[index] = source.unit_milli(node, &entry)?;
            seen[index] = true;
        }
        for (index, name) in ATTRIBUTES.iter().enumerate() {
            if !seen[index] {
                return Err(source.error(format!("heritability.{name}"), "missing heritability"));
            }
            self.spread[index] = spread_factor(self.heritability[index]);
        }
        Ok(())
    }

    fn parse_start(&mut self, source: &Source<'_>, table: &Table) -> Result<(), DataError> {
        for (name, node) in table {
            let entry = format!("start.{name}");
            let set = source.table(node, &entry)?;
            let mut start = StartSet::default();
            for (line, shares) in set {
                let at = format!("{entry}.{line}");
                let (tables, target) = match line.as_str() {
                    "paternal" => (&self.paternal, &mut start.paternal),
                    "maternal" => (&self.maternal, &mut start.maternal),
                    _ => return Err(source.error(&at, "expected `paternal` or `maternal`")),
                };
                let mut total = 0_i128;
                for (haplogroup, share) in source.table(shares, &at)? {
                    let share_at = format!("{at}.{haplogroup}");
                    if haplogroup != OTHER && !tables.contains_key(haplogroup) {
                        return Err(source.error(&share_at, "undeclared haplogroup"));
                    }
                    let percent = source.integer(share, &share_at)?;
                    if !(0..=100).contains(&percent) {
                        return Err(source.error(&share_at, "a share must lie in 0..100"));
                    }
                    total += percent;
                    target.insert(
                        haplogroup.clone(),
                        u32::try_from(percent * 10)
                            .map_err(|_| source.error(&share_at, "share too large"))?,
                    );
                }
                if total != 100 {
                    return Err(source.error(&at, format!("shares sum to {total}, not 100")));
                }
            }
            for (line, shares) in [("paternal", &start.paternal), ("maternal", &start.maternal)] {
                if shares.is_empty() {
                    return Err(source.error(format!("{entry}.{line}"), "missing share set"));
                }
            }
            self.start.insert(name.clone(), start);
        }
        Ok(())
    }

    fn parse_culture_start(
        &mut self,
        source: &Source<'_>,
        table: &Table,
        cultures: &[&str],
    ) -> Result<(), DataError> {
        for (culture, node) in table {
            let entry = format!("culture_start.{culture}");
            let set = source.text(node, &entry)?;
            if !self.start.contains_key(set) {
                return Err(source.error(&entry, format!("`{set}` is not a declared start set")));
            }
            self.culture_start.insert(culture.clone(), set.to_owned());
        }
        for culture in cultures {
            if !self.culture_start.contains_key(*culture) {
                return Err(source.error(
                    format!("culture_start.{culture}"),
                    "culture has no start set",
                ));
            }
        }
        Ok(())
    }
}

fn parse_tables(
    source: &Source<'_>,
    table: &Table,
    kind: &str,
    is_trait: &dyn Fn(&str) -> bool,
) -> Result<BTreeMap<String, HaplogroupTable>, DataError> {
    let mut tables = BTreeMap::new();
    for (name, node) in table {
        let entry = format!("{kind}.{name}");
        if name == OTHER {
            return Err(source.error(
                entry,
                "`other` is reserved for the default table and cannot be declared",
            ));
        }
        let mut parsed = HaplogroupTable::default();
        for (key, value) in source.table(node, &entry)? {
            let at = format!("{entry}.{key}");
            if key == "trait_rates" {
                for (trait_name, rate) in source.table(value, &at)? {
                    let rate_at = format!("{at}.{trait_name}");
                    if !is_trait(trait_name) {
                        return Err(source.error(&rate_at, "not a trait"));
                    }
                    parsed
                        .trait_rates
                        .insert(trait_name.clone(), source.multiplier(rate, &rate_at)?);
                }
                continue;
            }
            let index = attribute(source, key, &at)?;
            let inner = source.table(value, &at)?;
            for (field, number) in inner {
                let field_at = format!("{at}.{field}");
                match field.as_str() {
                    "mean" => parsed.mean[index] = Some(source.milli(number, &field_at)?),
                    "sd" => parsed.sd[index] = Some(source.milli_non_negative(number, &field_at)?),
                    _ => return Err(source.error(&field_at, "expected `mean` or `sd`")),
                }
            }
        }
        tables.insert(name.clone(), parsed);
    }
    Ok(tables)
}

// ---------------------------------------------------------------------------------------------------------------------
// Accessors

impl Genetics {
    /// Whether no genetic data is loaded (rules built without [`Genetics::load`]).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.start.is_empty()
    }

    /// The Mendelian conditions, by name.
    #[must_use]
    pub const fn mendelian(&self) -> &BTreeMap<String, Mendelian> {
        &self.mendelian
    }

    /// The graded traits, by name.
    #[must_use]
    pub const fn graded(&self) -> &BTreeMap<String, Graded> {
        &self.graded
    }

    /// The heritability of an attribute, in thousandths.
    #[must_use]
    pub const fn heritability(&self, attribute: usize) -> u32 {
        self.heritability[attribute]
    }

    /// The precomputed spread factor of an attribute, in thousandths ([`spread_factor`]).
    #[must_use]
    pub const fn spread(&self, attribute: usize) -> u32 {
        self.spread[attribute]
    }

    /// The women's offset of an attribute, in thousandths (0 when unlisted).
    #[must_use]
    pub const fn female_offset(&self, attribute: usize) -> i64 {
        self.female[attribute]
    }

    /// The default table's mean and spread of an attribute.
    #[must_use]
    pub const fn default_distribution(&self, attribute: usize) -> Distribution {
        self.default[attribute]
    }

    /// The start set a culture uses.
    #[must_use]
    pub fn start_set(&self, culture: &str) -> Option<&StartSet> {
        self.start.get(self.culture_start.get(culture)?)
    }

    /// The paternal haplogroup tables, by name.
    #[must_use]
    pub const fn paternal_tables(&self) -> &BTreeMap<String, HaplogroupTable> {
        &self.paternal
    }

    /// The maternal haplogroup tables, by name.
    #[must_use]
    pub const fn maternal_tables(&self) -> &BTreeMap<String, HaplogroupTable> {
        &self.maternal
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Seeding

impl Genetics {
    fn table_value(
        &self,
        tables: &BTreeMap<String, HaplogroupTable>,
        haplogroup: &str,
        attribute: usize,
        spread: bool,
    ) -> i64 {
        let fallback = self.default[attribute];
        let default = if spread { fallback.sd } else { fallback.mean };
        if haplogroup == OTHER {
            return default;
        }
        tables.get(haplogroup).map_or(default, |table| {
            let value = if spread {
                table.sd[attribute]
            } else {
                table.mean[attribute]
            };
            value.unwrap_or(default)
        })
    }

    /// Sum of `share * value` over a share set (thousandths times thousandths).
    fn weighted(
        &self,
        shares: &BTreeMap<String, u32>,
        tables: &BTreeMap<String, HaplogroupTable>,
        attribute: usize,
        spread: bool,
    ) -> i128 {
        shares
            .iter()
            .map(|(haplogroup, share)| {
                i128::from(*share)
                    * i128::from(self.table_value(tables, haplogroup, attribute, spread))
            })
            .sum()
    }

    /// Sum of `share * multiplier` for a trait over a share set; a table without the trait, and `other`, count 1.
    fn weighted_multiplier(
        shares: &BTreeMap<String, u32>,
        tables: &BTreeMap<String, HaplogroupTable>,
        name: &str,
    ) -> u128 {
        shares
            .iter()
            .map(|(haplogroup, share)| {
                let multiplier = tables
                    .get(haplogroup)
                    .and_then(|table| table.trait_rates.get(name))
                    .copied()
                    .unwrap_or(1000);
                u128::from(*share) * u128::from(multiplier)
            })
            .sum()
    }

    fn seeded_rate(&self, base: u32, set: &StartSet, name: &str) -> u32 {
        let paternal = Self::weighted_multiplier(&set.paternal, &self.paternal, name);
        let maternal = Self::weighted_multiplier(&set.maternal, &self.maternal, name);
        // Both averages stay exact numerators: shares and multipliers are both in thousandths, so the product is over
        // (1000 * 1000)^2. Only the final rate is rounded (to the nearest ppb, ties away from zero) and then capped.
        let numerator = u128::from(base) * paternal * maternal;
        let denominator = 1_000_000_000_000_u128;
        let remainder = numerator % denominator;
        let rounded = numerator / denominator + u128::from(remainder >= denominator - remainder);
        u32::try_from(rounded.min(u128::from(PPB))).unwrap_or(PPB)
    }

    /// The genetic state of a new pop of `culture` at the start (§4), or `None` when the culture has no start set.
    #[must_use]
    pub fn seed_pop(&self, culture: &str) -> Option<PopGenetics> {
        let set = self.start_set(culture)?;
        let mut pop = PopGenetics {
            paternal: set.paternal.clone(),
            maternal: set.maternal.clone(),
            ..PopGenetics::default()
        };
        for attribute in 0..ATTRIBUTE_COUNT {
            for (spread, target) in [(false, &mut pop.mean), (true, &mut pop.sd)] {
                // 50/50 of the share-weighted paternal and maternal averages: the numerator over 2 * 1000 (shares).
                let numerator = self.weighted(&set.paternal, &self.paternal, attribute, spread)
                    + self.weighted(&set.maternal, &self.maternal, attribute, spread);
                target[attribute] =
                    i64::try_from(round_div(numerator, 2 * i128::from(SHARE_TOTAL))).ok()?;
            }
        }
        for (name, trait_rules) in &self.mendelian {
            pop.carrier.insert(
                name.clone(),
                self.seeded_rate(trait_rules.allele, set, name),
            );
        }
        for (name, trait_rules) in &self.graded {
            pop.founder.insert(
                name.clone(),
                self.seeded_rate(trait_rules.founder, set, name),
            );
        }
        Some(pop)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// People-weighted movement

/// The people-weighted average of two scalars, or `None` when the exact numerator does not fit.
fn blend_scalar(own: i128, own_heads: u128, other: i128, moved_heads: u128) -> Option<i128> {
    let total = i128::try_from(own_heads.checked_add(moved_heads)?).ok()?;
    let weighted = i128::try_from(own_heads)
        .ok()?
        .checked_mul(own)?
        .checked_add(i128::try_from(moved_heads).ok()?.checked_mul(other)?)?;
    Some(round_div(weighted, total))
}

fn blend_rates(
    own: &mut BTreeMap<String, u32>,
    own_heads: u128,
    other: &BTreeMap<String, u32>,
    moved_heads: u128,
) -> Option<()> {
    // Each key once: a key both sides hold must be blended once, not twice.
    let keys: std::collections::BTreeSet<String> =
        own.keys().chain(other.keys()).cloned().collect();
    for key in keys {
        let mine = i128::from(own.get(&key).copied().unwrap_or(0));
        let theirs = i128::from(other.get(&key).copied().unwrap_or(0));
        let blended = blend_scalar(mine, own_heads, theirs, moved_heads)?;
        own.insert(key, u32::try_from(blended).ok()?);
    }
    Some(())
}

/// Blend two share sets, people-weighted, then apportion to exactly 1000: floors, then the largest remainders, ties
/// broken by haplogroup key in sorted order.
fn blend_shares(
    own: &mut BTreeMap<String, u32>,
    own_heads: u128,
    other: &BTreeMap<String, u32>,
    moved_heads: u128,
) {
    let total = own_heads + moved_heads;
    if total == 0 {
        return;
    }
    let mut lines: Vec<(String, u128, u128)> = Vec::new();
    let keys: std::collections::BTreeSet<&String> = own.keys().chain(other.keys()).collect();
    for key in keys {
        let mine = u128::from(own.get(key).copied().unwrap_or(0));
        let theirs = u128::from(other.get(key).copied().unwrap_or(0));
        let numerator = own_heads * mine + moved_heads * theirs;
        lines.push((key.clone(), numerator / total, numerator % total));
    }
    let floors: u128 = lines.iter().map(|line| line.1).sum();
    let mut deficit = u128::from(SHARE_TOTAL).saturating_sub(floors);
    // `lines` is in key order, so a stable sort by descending remainder breaks ties by key.
    let mut order: Vec<usize> = (0..lines.len()).collect();
    order.sort_by(|a, b| lines[*b].2.cmp(&lines[*a].2));
    for index in order {
        if deficit == 0 {
            break;
        }
        lines[index].1 += 1;
        deficit -= 1;
    }
    own.clear();
    for (key, share, _) in lines {
        own.insert(key, u32::try_from(share).unwrap_or(u32::MAX));
    }
}

impl PopGenetics {
    /// Whether no state was ever seeded (a pop made without genetic data).
    #[must_use]
    pub fn is_unseeded(&self) -> bool {
        self.paternal.is_empty() && self.maternal.is_empty()
    }

    /// Become the people-weighted average of this state (`own_heads` people) and `other` (`moved_heads` people arriving):
    /// scalars round to the nearest unit of their scale, ties away from zero; each share set is apportioned to exactly
    /// 1000. A receiver with no people, or one never seeded, takes the movers' state exactly; the source is not changed.
    ///
    /// # Errors
    /// Refuses, leaving this state unchanged, a blend whose exact numerator or result does not fit its type.
    pub fn blend(
        &mut self,
        own_heads: u64,
        other: &Self,
        moved_heads: u64,
    ) -> Result<(), BlendOverflow> {
        if other.is_unseeded() || moved_heads == 0 {
            return Ok(());
        }
        if self.is_unseeded() || own_heads == 0 {
            *self = other.clone();
            return Ok(());
        }
        let (own, moved) = (u128::from(own_heads), u128::from(moved_heads));
        let mut next = self.clone();
        for attribute in 0..ATTRIBUTE_COUNT {
            for (mine, theirs) in [(&mut next.mean, &other.mean), (&mut next.sd, &other.sd)] {
                mine[attribute] = blend_scalar(
                    i128::from(mine[attribute]),
                    own,
                    i128::from(theirs[attribute]),
                    moved,
                )
                .and_then(|value| i64::try_from(value).ok())
                .ok_or(BlendOverflow)?;
            }
        }
        blend_shares(&mut next.paternal, own, &other.paternal, moved);
        blend_shares(&mut next.maternal, own, &other.maternal, moved);
        blend_rates(&mut next.carrier, own, &other.carrier, moved).ok_or(BlendOverflow)?;
        blend_rates(&mut next.founder, own, &other.founder, moved).ok_or(BlendOverflow)?;
        *self = next;
        Ok(())
    }
}

/// A genetic-state blend whose exact weighted sum or rounded result does not fit its type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlendOverflow;

impl std::fmt::Display for BlendOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the genetic state average does not fit")
    }
}

impl std::error::Error for BlendOverflow {}

impl Canonical for PopGenetics {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        for value in self.mean.iter().chain(&self.sd) {
            value.encode(out)?;
        }
        self.paternal.encode(out)?;
        self.maternal.encode(out)?;
        self.carrier.encode(out)?;
        self.founder.encode(out)
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let mut state = Self::default();
        for slot in state.mean.iter_mut().chain(state.sd.iter_mut()) {
            *slot = i64::decode(r)?;
        }
        state.paternal = BTreeMap::decode(r)?;
        state.maternal = BTreeMap::decode(r)?;
        state.carrier = BTreeMap::decode(r)?;
        state.founder = BTreeMap::decode(r)?;
        Ok(state)
    }
}
