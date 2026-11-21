//! The real figures of the 768 start (CK-370, characters slice 8 §1): the loader and its validation.
//!
//! Two files of the scenario hold them, and **each field is read from the file that owns it**:
//!
//! - the **research file** (`ireland-768.toml`): key, name, genitive, byname, patronymic, sex, `location`, `mother_tongue`, `second_languages`
//!   and the `forced` traits. `beats`, `role` and `note` are kept and never acted on. Its prose `born`, `died`, `pop` and parent fields are
//!   **not read**. `[defaults].excluded` lists the traits no real figure is given;
//! - the **load file** (`ireland-768-load.toml`): `house`, `subhouse` (kept for L7), `house_of_birth`, `office`, the parent and spouse keys,
//!   `pop_kind`, the structured dates, the `dead_parent` entries, the `strand` entries and the `slot` entries with their members.
//!
//! A year alone means 1 January. Nothing here uses floating point: a language range is read from its digits as thousandths.
//!
//! [`FigureData::parse`] reads and checks what the two files say by themselves; [`FigureData::check_pops`] joins them to the pops of a scenario
//! (the place and the pop of each figure, dead parent and slot member). Every refusal is a named [`FigureError`].
#![deny(clippy::float_arithmetic)]

use crate::{Office, PopLocator, Sex, connections::is_strand_kind};
use ck_economy::events::ROLES;
use std::collections::{BTreeMap, BTreeSet};
use toml::de::{DeTable, DeValue};

/// The pop of a place a figure joins, as the load file's `pop_kind` says: the place's first pop of that type in slot order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopKind {
    /// The place's first noble pop (Dún Sobhairce's is `dal_riatan`).
    Noble,
    /// The place's first monk pop (Mayo's is `northumbrian`).
    Monk,
}

impl PopKind {
    /// The pop type key of the economy.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Noble => "noble",
            Self::Monk => "monk",
        }
    }
}

/// A year of the load file with whether it is an estimate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Year {
    /// The calendar year; the date is its 1 January.
    pub year: u16,
    /// Whether the year is an estimate.
    pub estimated: bool,
}

/// A range of language strengths in thousandths, `low <= high`, both within 0..=1000.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    /// The lowest strength.
    pub low: u32,
    /// The highest strength.
    pub high: u32,
}

/// A real figure: the research file's fixed aspects joined to the load file's structure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Figure {
    /// The key both files name the figure by.
    pub key: String,
    /// The base name.
    pub name: String,
    /// The genitive of the name, for the patronymics of its children; empty for none.
    pub genitive: String,
    /// The byname; empty for none.
    pub byname: String,
    /// The patronymic as the sources give it (`mac Fergaile`); empty for none. Kept: the shown name derives its own.
    pub patronymic: String,
    /// The sex.
    pub sex: Sex,
    /// The place the figure lives in (`location`, a location id of the scenario).
    pub location: String,
    /// The language key of the mother tongue.
    pub mother_tongue: String,
    /// The second languages, each a range rolled once at the start on the figure's own stream.
    pub second_languages: Vec<(String, Range)>,
    /// The traits the sources give: key and level.
    pub forced: Vec<(String, u8)>,
    /// The traits this figure never has: the defaults', or its own list.
    pub excluded: Vec<(String, u8)>,
    /// The life beats, kept and inert (L8).
    pub beats: Vec<String>,
    /// The role in prose, kept and inert.
    pub role: String,
    /// The note, kept and inert.
    pub note: String,
    /// The house the figure belongs to at the start; empty for none.
    pub house: String,
    /// The subhouse, kept for L7.
    pub subhouse: String,
    /// The house of birth where it differs from `house`; empty otherwise.
    pub house_of_birth: String,
    /// The office marker, if any.
    pub office: Option<Office>,
    /// The father's key (a figure or a dead parent); empty for none.
    pub father: String,
    /// The mother's key; empty for none.
    pub mother: String,
    /// The spouse's key; empty for none.
    pub spouse: String,
    /// The pop the figure joins at its place.
    pub pop_kind: PopKind,
    /// The birth.
    pub born: Year,
    /// The death year, when the load file gives one (history: the death curve runs as for anyone while the figure is alive).
    pub died: Option<Year>,
    /// The day of a death known to the day, `(year, month, day)`.
    pub died_date: Option<(u16, u8, u8)>,
    /// Died before 768 in a year nobody recorded.
    pub died_before_start: bool,
    /// Whether the figure is live on 9 October 768. Authoritative.
    pub alive_at_start: bool,
}

/// A dead father or mother the figures name that is not in the figures file: an archived character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadParent {
    /// The key.
    pub key: String,
    /// The base name.
    pub name: String,
    /// The genitive.
    pub genitive: String,
    /// The sex.
    pub sex: Sex,
    /// The birth (the eldest named child's minus 30, estimated).
    pub born: Year,
    /// The death year; `None` when it is not recorded (archived with an **unknown** death).
    pub died: Option<u16>,
    /// The house.
    pub house: String,
    /// The pop it would have been in.
    pub pop_kind: PopKind,
    /// The place.
    pub location: String,
}

/// A known strand between two figures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthoredStrand {
    /// One end.
    pub a: String,
    /// The other.
    pub b: String,
    /// The kind (slice 6's: `friend`, `rival`, `lover`, ...).
    pub kind: String,
    /// The strength, 1..=10.
    pub strength: u8,
}

/// A member row of a slot: `count` people of a role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotMember {
    /// The role, a key of [`ROLES`].
    pub role: String,
    /// How many.
    pub count: u32,
    /// The pop type of the place they join (`noble`, `monk`, `labourer`, `peasant`).
    pub pop: String,
    /// The sex.
    pub sex: Sex,
    /// For a student: taken from noble families within reach.
    pub from_noble_families: bool,
}

/// A court or a community: a place and the people its slot names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotData {
    /// The key.
    pub key: String,
    /// The place.
    pub location: String,
    /// The member rows, in file order.
    pub members: Vec<SlotMember>,
}

/// The two files, read and joined.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FigureData {
    /// The figures, in the order of the load file.
    pub figures: Vec<Figure>,
    /// The dead parents, in file order.
    pub dead_parents: Vec<DeadParent>,
    /// The known strands, in file order.
    pub strands: Vec<AuthoredStrand>,
    /// The slots, in file order.
    pub slots: Vec<SlotData>,
    /// The traits no real figure is given (the research file's `[defaults].excluded`): key and the lowest level excluded.
    pub excluded: Vec<(String, u8)>,
}

/// A refusal of the loader, named (CK-370 §1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FigureError {
    /// A file that is not TOML.
    Syntax {
        /// The file.
        file: String,
        /// What the parser said.
        problem: String,
    },
    /// A field that is missing, mistyped or out of range.
    Field {
        /// The file.
        file: String,
        /// The entry (`figure.niall_frossach`).
        entry: String,
        /// The problem.
        problem: String,
    },
    /// A key that two entries of one table share.
    DuplicateKey {
        /// The entry.
        entry: String,
        /// The key.
        key: String,
    },
    /// A load-file key that is not a figure or a dead parent.
    UnknownKey {
        /// The entry that names it.
        entry: String,
        /// The key.
        key: String,
    },
    /// An office other than `ruler` and `high_cleric`.
    Office {
        /// The entry.
        entry: String,
        /// What it said.
        office: String,
    },
    /// A slot member role outside the role registry.
    Role {
        /// The entry.
        entry: String,
        /// The role.
        role: String,
    },
    /// A strand kind that is not one of slice 6's.
    StrandKind {
        /// The entry.
        entry: String,
        /// The kind.
        kind: String,
    },
    /// A father who is a woman or a mother who is a man.
    ParentSex {
        /// The child.
        entry: String,
        /// The parent named.
        parent: String,
    },
    /// A cycle in the parent keys.
    ParentCycle {
        /// A key on the cycle.
        key: String,
    },
    /// Named births of one mother less than nine months apart that are not twins.
    BirthGap {
        /// The mother.
        mother: String,
        /// The two children.
        children: [String; 2],
    },
    /// A figure, dead parent or slot whose place or pop does not exist in the scenario.
    Pop {
        /// The entry.
        entry: String,
        /// The place.
        place: String,
        /// The pop type that is missing there; empty when the place itself is.
        kind: String,
    },
    /// A trait a figure is both given and excluded.
    ForcedExcluded {
        /// The figure.
        entry: String,
        /// The trait.
        key: String,
    },
    /// A date that is not a day of the calendar (`2026-02-30`).
    Date {
        /// The entry.
        entry: String,
        /// What it said.
        value: String,
    },
    /// A death before its birth, or a living figure whose death is before the start.
    Dates {
        /// The entry.
        entry: String,
        /// The problem.
        problem: String,
    },
}

impl std::fmt::Display for FigureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax { file, problem } => write!(f, "{file}: {problem}"),
            Self::Field {
                file,
                entry,
                problem,
            } => write!(f, "{file}: {entry}: {problem}"),
            Self::DuplicateKey { entry, key } => {
                write!(f, "{entry}: the key `{key}` is used twice")
            }
            Self::UnknownKey { entry, key } => {
                write!(f, "{entry}: `{key}` is not a figure or a dead parent")
            }
            Self::Office { entry, office } => write!(
                f,
                "{entry}: the office `{office}` is not `ruler` or `high_cleric`"
            ),
            Self::Role { entry, role } => {
                write!(f, "{entry}: the role `{role}` is not in the role registry")
            }
            Self::StrandKind { entry, kind } => {
                write!(f, "{entry}: `{kind}` is not a strand kind of slice 6")
            }
            Self::ParentSex { entry, parent } => {
                write!(f, "{entry}: the parent `{parent}` is of the wrong sex")
            }
            Self::ParentCycle { key } => write!(f, "the parent keys form a cycle through `{key}`"),
            Self::BirthGap { mother, children } => write!(
                f,
                "the children `{}` and `{}` of `{mother}` were born less than nine months apart and are not twins",
                children[0], children[1]
            ),
            Self::Pop { entry, place, kind } if kind.is_empty() => {
                write!(f, "{entry}: the place `{place}` does not exist")
            }
            Self::Pop { entry, place, kind } => {
                write!(f, "{entry}: the place `{place}` has no {kind} pop")
            }
            Self::ForcedExcluded { entry, key } => {
                write!(f, "{entry}: the trait `{key}` is both forced and excluded")
            }
            Self::Date { entry, value } => {
                write!(f, "{entry}: `{value}` is not a day of the calendar")
            }
            Self::Dates { entry, problem } => write!(f, "{entry}: {problem}"),
        }
    }
}

impl std::error::Error for FigureError {}

// ---------------------------------------------------------------------------------------------------------------------
// A small exact reader of TOML: numbers keep their digits.

/// A value of the document with its number text kept.
#[derive(Debug)]
enum Node {
    Int(String),
    Float(String),
    Text(String),
    Bool(bool),
    Array(Vec<Node>),
    Table(BTreeMap<String, Node>),
    Other,
}

fn convert(value: &DeValue<'_>) -> Node {
    match value {
        DeValue::String(text) => Node::Text(text.as_ref().to_owned()),
        DeValue::Integer(number) if number.radix() == 10 => Node::Int(number.as_str().to_owned()),
        DeValue::Float(number) => Node::Float(number.as_str().to_owned()),
        DeValue::Boolean(value) => Node::Bool(*value),
        DeValue::Array(items) => {
            Node::Array(items.iter().map(|item| convert(item.get_ref())).collect())
        }
        DeValue::Table(table) => Node::Table(convert_table(table)),
        DeValue::Integer(_) | DeValue::Datetime(_) => Node::Other,
    }
}

fn convert_table(table: &DeTable<'_>) -> BTreeMap<String, Node> {
    table
        .iter()
        .map(|(key, value)| (key.get_ref().as_ref().to_owned(), convert(value.get_ref())))
        .collect()
}

/// A file being read: its name for the errors.
struct File<'a> {
    name: &'a str,
}

type Table = BTreeMap<String, Node>;

impl File<'_> {
    fn error(&self, entry: &str, problem: impl Into<String>) -> FigureError {
        FigureError::Field {
            file: self.name.to_owned(),
            entry: entry.to_owned(),
            problem: problem.into(),
        }
    }

    fn document(&self, text: &str) -> Result<Table, FigureError> {
        let table = DeTable::parse(text).map_err(|error| FigureError::Syntax {
            file: self.name.to_owned(),
            problem: error.message().to_owned(),
        })?;
        Ok(convert_table(table.get_ref()))
    }

    /// The array of tables `key` of `table`, empty when absent.
    fn tables<'t>(&self, table: &'t Table, key: &str) -> Result<Vec<&'t Table>, FigureError> {
        match table.get(key) {
            None => Ok(Vec::new()),
            Some(Node::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Node::Table(inner) => Ok(inner),
                    _ => Err(self.error(key, "expected an array of tables")),
                })
                .collect(),
            Some(_) => Err(self.error(key, "expected an array of tables")),
        }
    }

    fn text<'t>(&self, entry: &str, table: &'t Table, key: &str) -> Result<&'t str, FigureError> {
        match table.get(key) {
            Some(Node::Text(text)) => Ok(text),
            Some(_) => Err(self.error(entry, format!("`{key}` must be a string"))),
            None => Err(self.error(entry, format!("`{key}` is missing"))),
        }
    }

    fn optional_text<'t>(
        &self,
        entry: &str,
        table: &'t Table,
        key: &str,
    ) -> Result<&'t str, FigureError> {
        match table.get(key) {
            None => Ok(""),
            Some(Node::Text(text)) => Ok(text),
            Some(_) => Err(self.error(entry, format!("`{key}` must be a string"))),
        }
    }

    fn boolean(&self, entry: &str, table: &Table, key: &str) -> Result<Option<bool>, FigureError> {
        match table.get(key) {
            None => Ok(None),
            Some(Node::Bool(value)) => Ok(Some(*value)),
            Some(_) => Err(self.error(entry, format!("`{key}` must be true or false"))),
        }
    }

    fn year(&self, entry: &str, table: &Table, key: &str) -> Result<Option<u16>, FigureError> {
        match table.get(key) {
            None => Ok(None),
            Some(Node::Int(text)) => text
                .parse::<u16>()
                .ok()
                .filter(|year| *year >= 1)
                .map(Some)
                .ok_or_else(|| self.error(entry, format!("`{key}` is not a year"))),
            Some(_) => Err(self.error(entry, format!("`{key}` must be a year"))),
        }
    }

    fn number(&self, entry: &str, table: &Table, key: &str) -> Result<u32, FigureError> {
        match table.get(key) {
            Some(Node::Int(text)) => text
                .parse::<u32>()
                .map_err(|_| self.error(entry, format!("`{key}` is not a whole number"))),
            Some(_) => Err(self.error(entry, format!("`{key}` must be a whole number"))),
            None => Err(self.error(entry, format!("`{key}` is missing"))),
        }
    }

    fn strings(&self, entry: &str, table: &Table, key: &str) -> Result<Vec<String>, FigureError> {
        match table.get(key) {
            None => Ok(Vec::new()),
            Some(Node::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Node::Text(text) => Ok(text.clone()),
                    _ => Err(self.error(entry, format!("`{key}` must hold strings"))),
                })
                .collect(),
            Some(_) => Err(self.error(entry, format!("`{key}` must be an array of strings"))),
        }
    }

    fn sex(&self, entry: &str, table: &Table) -> Result<Sex, FigureError> {
        match self.text(entry, table, "sex")? {
            "m" => Ok(Sex::Male),
            "f" => Ok(Sex::Female),
            other => Err(self.error(entry, format!("`sex` is `m` or `f`, not `{other}`"))),
        }
    }

    fn pop_kind(&self, entry: &str, table: &Table) -> Result<PopKind, FigureError> {
        match self.text(entry, table, "pop_kind")? {
            "noble" => Ok(PopKind::Noble),
            "monk" => Ok(PopKind::Monk),
            other => Err(self.error(
                entry,
                format!("`pop_kind` is `noble` or `monk`, not `{other}`"),
            )),
        }
    }
}

/// A plain decimal as thousandths, exactly: `0.95` is 950. More than three decimal places, an exponent or a sign is refused.
fn thousandths(text: &str) -> Option<u32> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let plain = !whole.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && !text.ends_with('.')
        && fraction.len() <= 3;
    if !plain {
        return None;
    }
    let mut value: u32 = 0;
    for digit in whole.bytes().chain(fraction.bytes()) {
        value = value
            .checked_mul(10)?
            .checked_add(u32::from(digit - b'0'))?;
    }
    for _ in fraction.len()..3 {
        value = value.checked_mul(10)?;
    }
    Some(value)
}

/// The grade letters of the trait catalog: C is 1 up to SSS at 6.
fn grade(letter: &str) -> Option<u8> {
    ["C", "B", "A", "S", "SS", "SSS"]
        .iter()
        .position(|known| *known == letter)
        .and_then(|at| u8::try_from(at + 1).ok())
}

/// A trait list entry: `key` (level 1, the whole trait) or `key:GRADE` (that grade).
fn trait_entry(file: &File<'_>, entry: &str, text: &str) -> Result<(String, u8), FigureError> {
    match text.split_once(':') {
        None if !text.is_empty() => Ok((text.to_owned(), 1)),
        Some((key, letter)) if !key.is_empty() => grade(letter)
            .map(|level| (key.to_owned(), level))
            .ok_or_else(|| file.error(entry, format!("`{text}`: `{letter}` is not a grade"))),
        _ => Err(file.error(entry, format!("`{text}` is not a trait"))),
    }
}

/// The day 9 October 768, on which the game starts: a death before it is a death before the start.
const START: (u16, u8, u8) = (768, 10, 9);

/// The date of `YYYY-MM-DD`, a real day of the calendar (a month that has the day, in a leap year or not).
fn day_of(text: &str) -> Option<(u16, u8, u8)> {
    let mut parts = text.split('-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let (year, month, day) = (
        year.parse::<u16>().ok()?,
        month.parse::<u8>().ok()?,
        day.parse::<u8>().ok()?,
    );
    let length = ck_sim::days_in_month(year, month).ok()?;
    (day >= 1 && day <= length).then_some((year, month, day))
}

// ---------------------------------------------------------------------------------------------------------------------
// The loader

/// What the research file says of one figure.
struct Research {
    name: String,
    genitive: String,
    byname: String,
    patronymic: String,
    sex: Sex,
    location: String,
    mother_tongue: String,
    second_languages: Vec<(String, Range)>,
    forced: Vec<(String, u8)>,
    excluded: Option<Vec<(String, u8)>>,
    beats: Vec<String>,
    role: String,
    note: String,
}

fn read_research(file: &File<'_>, table: &Table) -> Result<(String, Research), FigureError> {
    let key = file.text("figure", table, "key")?.to_owned();
    let entry = format!("figure.{key}");
    let mut second_languages = Vec::new();
    match table.get("second_languages") {
        None => {}
        Some(Node::Table(languages)) => {
            for (language, range) in languages {
                let at = format!("{entry}.second_languages.{language}");
                let bounds = match range {
                    Node::Array(items) if items.len() == 2 => items
                        .iter()
                        .map(|item| match item {
                            Node::Int(text) | Node::Float(text) => thousandths(text)
                                .filter(|value| *value <= 1000)
                                .ok_or_else(|| {
                                    file.error(
                                        &at,
                                        "a strength lies in 0..1 with at most three decimals",
                                    )
                                }),
                            _ => Err(file.error(&at, "a strength is a number")),
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    _ => return Err(file.error(&at, "a range is `[low, high]`")),
                };
                if bounds[0] > bounds[1] {
                    return Err(file.error(&at, "the low strength exceeds the high strength"));
                }
                second_languages.push((
                    language.clone(),
                    Range {
                        low: bounds[0],
                        high: bounds[1],
                    },
                ));
            }
        }
        Some(_) => return Err(file.error(&entry, "`second_languages` must be a table")),
    }
    let forced = file
        .strings(&entry, table, "forced")?
        .iter()
        .map(|text| trait_entry(file, &entry, text))
        .collect::<Result<Vec<_>, _>>()?;
    let excluded = if table.contains_key("excluded") {
        Some(
            file.strings(&entry, table, "excluded")?
                .iter()
                .map(|text| trait_entry(file, &entry, text))
                .collect::<Result<Vec<_>, _>>()?,
        )
    } else {
        None
    };
    Ok((
        key,
        Research {
            name: file.text(&entry, table, "name")?.to_owned(),
            genitive: file.optional_text(&entry, table, "genitive")?.to_owned(),
            byname: file.optional_text(&entry, table, "byname")?.to_owned(),
            patronymic: file.optional_text(&entry, table, "patronymic")?.to_owned(),
            sex: file.sex(&entry, table)?,
            location: file.text(&entry, table, "location")?.to_owned(),
            mother_tongue: file.text(&entry, table, "mother_tongue")?.to_owned(),
            second_languages,
            forced,
            excluded,
            beats: file.strings(&entry, table, "beats")?,
            role: file.optional_text(&entry, table, "role")?.to_owned(),
            note: file.optional_text(&entry, table, "note")?.to_owned(),
        },
    ))
}

impl FigureData {
    /// Read the two files and check what they say by themselves: the keys, the offices, the roles, the strand kinds, the parents' sexes,
    /// the parent cycle, the gaps between a mother's births, and that no forced trait is excluded. The places and pops are joined by
    /// [`check_pops`](Self::check_pops).
    ///
    /// # Errors
    /// The first refusal, named.
    #[expect(
        clippy::too_many_lines,
        reason = "the two files are read in one pass, each table with its own fields"
    )]
    pub fn parse(research: &str, load: &str) -> Result<Self, FigureError> {
        let research_file = File {
            name: "ireland-768.toml",
        };
        let load_file = File {
            name: "ireland-768-load.toml",
        };
        let research_doc = research_file.document(research)?;
        let load_doc = load_file.document(load)?;

        // The research file: the figures by key, and the defaults' exclusions.
        let defaults: Vec<(String, u8)> = match research_doc.get("defaults") {
            None => Vec::new(),
            Some(Node::Table(table)) => research_file
                .strings("defaults", table, "excluded")?
                .iter()
                .map(|text| trait_entry(&research_file, "defaults.excluded", text))
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(research_file.error("defaults", "expected a table")),
        };
        let mut researched: BTreeMap<String, Research> = BTreeMap::new();
        for table in research_file.tables(&research_doc, "figure")? {
            let (key, figure) = read_research(&research_file, table)?;
            if researched.insert(key.clone(), figure).is_some() {
                return Err(FigureError::DuplicateKey {
                    entry: "figure".to_owned(),
                    key,
                });
            }
        }

        // The load file.
        let mut figures: Vec<Figure> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for table in load_file.tables(&load_doc, "figure")? {
            let key = load_file.text("figure", table, "key")?.to_owned();
            let entry = format!("figure.{key}");
            if !seen.insert(key.clone()) {
                return Err(FigureError::DuplicateKey {
                    entry: "figure".to_owned(),
                    key,
                });
            }
            let said = researched
                .remove(&key)
                .ok_or_else(|| FigureError::UnknownKey {
                    entry: format!("{entry} (the load file)"),
                    key: key.clone(),
                })?;
            let office = match load_file.optional_text(&entry, table, "office")? {
                "" => None,
                "ruler" => Some(Office::Ruler),
                "high_cleric" => Some(Office::HighCleric),
                other => {
                    return Err(FigureError::Office {
                        entry,
                        office: other.to_owned(),
                    });
                }
            };
            let born = Year {
                year: load_file
                    .year(&entry, table, "born")?
                    .ok_or_else(|| load_file.error(&entry, "`born` is missing"))?,
                estimated: load_file
                    .boolean(&entry, table, "born_estimated")?
                    .unwrap_or(false),
            };
            let died_estimated = load_file
                .boolean(&entry, table, "died_estimated")?
                .unwrap_or(false);
            let died = load_file.year(&entry, table, "died")?.map(|year| Year {
                year,
                estimated: died_estimated,
            });
            let died_date = match load_file.optional_text(&entry, table, "died_date")? {
                "" => None,
                text => Some(day_of(text).ok_or_else(|| FigureError::Date {
                    entry: entry.clone(),
                    value: text.to_owned(),
                })?),
            };
            let alive_at_start = load_file
                .boolean(&entry, table, "alive_at_start")?
                .ok_or_else(|| load_file.error(&entry, "`alive_at_start` is missing"))?;
            let died_before_start = load_file
                .boolean(&entry, table, "died_before_start")?
                .unwrap_or(false);
            // The defaults' list is kept when a figure lists its own: a figure's entries add to it (and a trait both lists keeps the stricter).
            let mut excluded = defaults.clone();
            for (key, from) in said.excluded.unwrap_or_default() {
                match excluded.iter_mut().find(|(known, _)| *known == key) {
                    Some(known) => known.1 = known.1.min(from),
                    None => excluded.push((key, from)),
                }
            }
            // A trait is refused only when the level it is given is one the exclusion forbids (`poet` at C is allowed beside `poet:B`).
            for (forced, level) in &said.forced {
                if excluded
                    .iter()
                    .any(|(key, from)| key == forced && level >= from)
                {
                    return Err(FigureError::ForcedExcluded {
                        entry,
                        key: forced.clone(),
                    });
                }
            }
            figures.push(Figure {
                key,
                name: said.name,
                genitive: said.genitive,
                byname: said.byname,
                patronymic: said.patronymic,
                sex: said.sex,
                location: said.location,
                mother_tongue: said.mother_tongue,
                second_languages: said.second_languages,
                forced: said.forced,
                excluded,
                beats: said.beats,
                role: said.role,
                note: said.note,
                house: load_file.optional_text(&entry, table, "house")?.to_owned(),
                subhouse: load_file
                    .optional_text(&entry, table, "subhouse")?
                    .to_owned(),
                house_of_birth: load_file
                    .optional_text(&entry, table, "house_of_birth")?
                    .to_owned(),
                office,
                father: load_file.optional_text(&entry, table, "father")?.to_owned(),
                mother: load_file.optional_text(&entry, table, "mother")?.to_owned(),
                spouse: load_file.optional_text(&entry, table, "spouse")?.to_owned(),
                pop_kind: load_file.pop_kind(&entry, table)?,
                born,
                died,
                died_date,
                died_before_start,
                alive_at_start,
            });
        }
        // A figure of the research file that the load file does not name has no structure to load.
        if let Some(key) = researched.keys().next() {
            return Err(FigureError::UnknownKey {
                entry: "figure (the research file)".to_owned(),
                key: key.clone(),
            });
        }

        let mut dead_parents: Vec<DeadParent> = Vec::new();
        for table in load_file.tables(&load_doc, "dead_parent")? {
            let key = load_file.text("dead_parent", table, "key")?.to_owned();
            let entry = format!("dead_parent.{key}");
            if !seen.insert(key.clone()) {
                return Err(FigureError::DuplicateKey {
                    entry: "dead_parent".to_owned(),
                    key,
                });
            }
            dead_parents.push(DeadParent {
                name: load_file.text(&entry, table, "name")?.to_owned(),
                genitive: load_file
                    .optional_text(&entry, table, "genitive")?
                    .to_owned(),
                sex: load_file.sex(&entry, table)?,
                born: Year {
                    year: load_file
                        .year(&entry, table, "born")?
                        .ok_or_else(|| load_file.error(&entry, "`born` is missing"))?,
                    estimated: load_file
                        .boolean(&entry, table, "born_estimated")?
                        .unwrap_or(false),
                },
                died: load_file.year(&entry, table, "died")?,
                house: load_file.optional_text(&entry, table, "house")?.to_owned(),
                pop_kind: load_file.pop_kind(&entry, table)?,
                location: load_file.text(&entry, table, "location")?.to_owned(),
                key,
            });
        }

        let mut strands: Vec<AuthoredStrand> = Vec::new();
        for table in load_file.tables(&load_doc, "strand")? {
            let entry = format!(
                "strand.{}-{}",
                load_file.optional_text("strand", table, "a")?,
                load_file.optional_text("strand", table, "b")?
            );
            let strength = load_file.number(&entry, table, "strength")?;
            let kind = load_file.text(&entry, table, "kind")?.to_owned();
            if !is_strand_kind(&kind) {
                return Err(FigureError::StrandKind { entry, kind });
            }
            strands.push(AuthoredStrand {
                a: load_file.text(&entry, table, "a")?.to_owned(),
                b: load_file.text(&entry, table, "b")?.to_owned(),
                kind,
                strength: u8::try_from(strength)
                    .ok()
                    .filter(|value| (1..=10).contains(value))
                    .ok_or_else(|| load_file.error(&entry, "`strength` lies in 1..=10"))?,
            });
        }

        let mut slots: Vec<SlotData> = Vec::new();
        let mut slot_keys: BTreeSet<String> = BTreeSet::new();
        for table in load_file.tables(&load_doc, "slot")? {
            let key = load_file.text("slot", table, "key")?.to_owned();
            let entry = format!("slot.{key}");
            if !slot_keys.insert(key.clone()) {
                return Err(FigureError::DuplicateKey {
                    entry: "slot".to_owned(),
                    key,
                });
            }
            let mut members = Vec::new();
            for member in load_file.tables(table, "member")? {
                let role = load_file.text(&entry, member, "role")?.to_owned();
                if !ROLES.contains(&role.as_str()) {
                    return Err(FigureError::Role { entry, role });
                }
                members.push(SlotMember {
                    count: load_file.number(&entry, member, "count")?,
                    pop: load_file.text(&entry, member, "pop")?.to_owned(),
                    sex: load_file.sex(&entry, member)?,
                    from_noble_families: load_file
                        .boolean(&entry, member, "from_noble_families")?
                        .unwrap_or(false),
                    role,
                });
            }
            slots.push(SlotData {
                location: load_file.text(&entry, table, "location")?.to_owned(),
                members,
                key,
            });
        }

        let data = Self {
            figures,
            dead_parents,
            strands,
            slots,
            excluded: defaults,
        };
        data.check_structure()?;
        Ok(data)
    }

    /// The figure of `key`.
    #[must_use]
    pub fn figure(&self, key: &str) -> Option<&Figure> {
        self.figures.iter().find(|figure| figure.key == key)
    }

    /// The sex and birth year of the figure or dead parent `key`.
    fn person(&self, key: &str) -> Option<(Sex, u16)> {
        self.figure(key)
            .map(|figure| (figure.sex, figure.born.year))
            .or_else(|| {
                self.dead_parents
                    .iter()
                    .find(|parent| parent.key == key)
                    .map(|parent| (parent.sex, parent.born.year))
            })
    }

    /// The checks the files make by themselves: every key resolves, the parents are of the right sex, no figure is its own ancestor, no mother
    /// bore two children less than nine months apart (unless twins), and the dates agree.
    fn check_structure(&self) -> Result<(), FigureError> {
        for figure in &self.figures {
            let entry = format!("figure.{}", figure.key);
            for (parent, sex) in [(&figure.father, Sex::Male), (&figure.mother, Sex::Female)] {
                if parent.is_empty() {
                    continue;
                }
                let (found, _) = self.person(parent).ok_or_else(|| FigureError::UnknownKey {
                    entry: entry.clone(),
                    key: parent.clone(),
                })?;
                if found != sex {
                    return Err(FigureError::ParentSex {
                        entry,
                        parent: parent.clone(),
                    });
                }
            }
            if !figure.spouse.is_empty() && self.figure(&figure.spouse).is_none() {
                return Err(FigureError::UnknownKey {
                    entry,
                    key: figure.spouse.clone(),
                });
            }
            // A figure alive at the start may carry a later death as history (the death curve runs as for anyone); one dated before the start
            // contradicts `alive_at_start`.
            if figure.alive_at_start
                && (figure.died_before_start
                    || figure.died_date.is_some_and(|date| date < START)
                    || figure.died.is_some_and(|died| died.year < START.0))
            {
                return Err(FigureError::Dates {
                    entry,
                    problem: "a figure alive at the start has a death before it".to_owned(),
                });
            }
            if let Some(died) = figure.died
                && !figure.alive_at_start
                && died.year < figure.born.year
            {
                return Err(FigureError::Dates {
                    entry,
                    problem: "died before it was born".to_owned(),
                });
            }
        }
        for parent in &self.dead_parents {
            if let Some(died) = parent.died
                && died < parent.born.year
            {
                return Err(FigureError::Dates {
                    entry: format!("dead_parent.{}", parent.key),
                    problem: "died before it was born".to_owned(),
                });
            }
        }
        for strand in &self.strands {
            for key in [&strand.a, &strand.b] {
                if self.figure(key).is_none() {
                    return Err(FigureError::UnknownKey {
                        entry: format!("strand.{}-{}", strand.a, strand.b),
                        key: key.clone(),
                    });
                }
            }
        }
        self.check_cycles()?;
        self.check_birth_gaps()
    }

    /// The parent of `key` that is named (a figure's father and mother); dead parents name none.
    fn parents_of(&self, key: &str) -> Vec<&str> {
        self.figure(key)
            .map(|figure| {
                [&figure.father, &figure.mother]
                    .into_iter()
                    .filter(|parent| !parent.is_empty())
                    .map(String::as_str)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn check_cycles(&self) -> Result<(), FigureError> {
        // 1 on the path being walked, 2 done.
        let mut state: BTreeMap<&str, u8> = BTreeMap::new();
        for figure in &self.figures {
            self.visit(&figure.key, &mut state)?;
        }
        Ok(())
    }

    fn visit<'a>(
        &'a self,
        key: &'a str,
        state: &mut BTreeMap<&'a str, u8>,
    ) -> Result<(), FigureError> {
        match state.get(key) {
            Some(2) => return Ok(()),
            Some(_) => {
                return Err(FigureError::ParentCycle {
                    key: key.to_owned(),
                });
            }
            None => {}
        }
        state.insert(key, 1);
        for parent in self.parents_of(key) {
            self.visit(parent, state)?;
        }
        state.insert(key, 2);
        Ok(())
    }

    fn check_birth_gaps(&self) -> Result<(), FigureError> {
        // Births carry a year alone (1 January), so two children of one mother are twins (the same year) or twelve months or more apart: the
        // nine-month rule of slice 7 refuses nothing in the shipped data. It is checked on the dates all the same, so that it binds when
        // a finer date arrives.
        let mut by_mother: BTreeMap<&str, Vec<(&str, u16)>> = BTreeMap::new();
        for figure in &self.figures {
            if !figure.mother.is_empty() {
                by_mother
                    .entry(figure.mother.as_str())
                    .or_default()
                    .push((figure.key.as_str(), figure.born.year));
            }
        }
        for (mother, children) in by_mother {
            for (index, (first, born)) in children.iter().enumerate() {
                for (second, other) in &children[index + 1..] {
                    let months = u32::from(born.abs_diff(*other)) * 12;
                    if months != 0 && months < 9 {
                        return Err(FigureError::BirthGap {
                            mother: mother.to_owned(),
                            children: [(*first).to_owned(), (*second).to_owned()],
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Join the files to the pops of a scenario (CK-370 §1): every figure's and dead parent's place has a pop of its `pop_kind`, and every slot's
    /// place has a pop of each member's pop type. `pops` is every pop of the map as `(locator, pop type, culture)`.
    ///
    /// # Errors
    /// The first figure, dead parent or slot whose place or pop does not exist, named.
    pub fn check_pops(&self, pops: &[(PopLocator, String, String)]) -> Result<(), FigureError> {
        let places: BTreeSet<&str> = pops.iter().map(|(pop, _, _)| pop.place.as_str()).collect();
        let has = |place: &str, kind: &str| {
            pops.iter()
                .any(|(pop, of, _)| pop.place == place && of == kind)
        };
        let check = |entry: String, place: &str, kind: &str| -> Result<(), FigureError> {
            if !places.contains(place) {
                return Err(FigureError::Pop {
                    entry,
                    place: place.to_owned(),
                    kind: String::new(),
                });
            }
            if !has(place, kind) {
                return Err(FigureError::Pop {
                    entry,
                    place: place.to_owned(),
                    kind: kind.to_owned(),
                });
            }
            Ok(())
        };
        for figure in &self.figures {
            check(
                format!("figure.{}", figure.key),
                &figure.location,
                figure.pop_kind.key(),
            )?;
        }
        for parent in &self.dead_parents {
            check(
                format!("dead_parent.{}", parent.key),
                &parent.location,
                parent.pop_kind.key(),
            )?;
        }
        for slot in &self.slots {
            for member in &slot.members {
                check(format!("slot.{}", slot.key), &slot.location, &member.pop)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decimal_is_read_as_exact_thousandths() {
        assert_eq!(thousandths("0.95"), Some(950));
        assert_eq!(thousandths("0.6"), Some(600));
        assert_eq!(thousandths("1"), Some(1000));
        assert_eq!(thousandths("0"), Some(0));
        assert_eq!(thousandths("0.0005"), None);
        assert_eq!(thousandths("1e-1"), None);
        assert_eq!(thousandths("-0.5"), None);
        assert_eq!(thousandths("."), None);
    }

    #[test]
    fn a_trait_entry_names_a_trait_or_a_grade_of_it() {
        let file = File { name: "f" };
        assert_eq!(
            trait_entry(&file, "e", "blind").unwrap_or_default(),
            ("blind".to_owned(), 1)
        );
        assert_eq!(
            trait_entry(&file, "e", "fool:S").unwrap_or_default(),
            ("fool".to_owned(), 4)
        );
        assert!(trait_entry(&file, "e", "fool:Z").is_err());
        assert!(trait_entry(&file, "e", "").is_err());
    }
}
