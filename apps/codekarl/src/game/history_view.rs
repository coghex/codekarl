//! What the graph viewer reads of the recorded history (history slice B, CK-441).
//!
//! The recorder ([`super::history`]) lives in the simulation owner. After every change to its history (a month recorded, a load) the
//! owner publishes an immutable [`View`] to a [`Host`] shared with the UI link. The UI asks the host for the subjects it can graph
//! ([`Host::subjects`]) and for graphs ([`Host::request`]); a graph is computed on a thread of its own from the year files, the
//! years held in memory and the running year, and its answer waits in a queue the UI takes from ([`Host::take`]). Nothing here
//! reads or changes simulation state, and nothing runs on the simulation thread.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use ck_lua::ConfigValue;
use sha2::{Digest, Sha256};

use super::history::{MonthRecord, PlaceRecord, PopRecord, decode_year};

/// The `global` subject's name.
pub(crate) const GLOBAL: &str = "global";
/// A chart has at most this many columns (rule 35).
pub(crate) const MAX_COLUMNS: u32 = 250;
/// A stacked chart names this many series by good before the rest are `other` (rule 32).
const NAMED_GOODS: usize = 6;

static REVISIONS: AtomicU64 = AtomicU64::new(1);

/// A history revision no earlier one has had: the host advances it whenever the running history changes (rule 42).
pub(crate) fn next_revision() -> u64 {
    REVISIONS.fetch_add(1, Ordering::Relaxed)
}

/// The running history at one revision, immutable.
pub(super) struct View {
    pub(super) revision: u64,
    pub(super) saves: PathBuf,
    pub(super) campaign: u64,
    /// The history's first month (rule 10).
    pub(super) origin: (u16, u8),
    /// The finished years, by year and file hash.
    pub(super) years: Vec<(u16, [u8; 32])>,
    /// Years whose file may not be on disk yet: their bytes by hash.
    pub(super) pending: Vec<([u8; 32], Arc<Vec<u8>>)>,
    /// The unfinished year's months.
    pub(super) running: Arc<Vec<MonthRecord>>,
    /// The last month the history should hold (rule 34); `None` before the first record is due.
    pub(super) horizon: Option<(u16, u8)>,
}

/// The last month the running history should hold when the clock stands at `date` (the hour that runs next): the month before the
/// current one once its 1st has been recorded, otherwise the one before that.
pub(crate) fn horizon_at(date: ck_sim::Date) -> Option<(u16, u8)> {
    let recorded = date.day > 1 || date.hour > super::core::ECONOMY_HOUR;
    let back = if recorded { 1 } else { 2 };
    let index = index_of(date.year, date.month) - back;
    (index >= index_of(1, 1)).then(|| of_index(index))
}

fn index_of(year: u16, month: u8) -> i64 {
    i64::from(year) * 12 + i64::from(month) - 1
}

fn of_index(index: i64) -> (u16, u8) {
    (
        u16::try_from(index.div_euclid(12)).unwrap_or(0),
        u8::try_from(index.rem_euclid(12) + 1).unwrap_or(1),
    )
}

/// How a window's range is chosen (rule 34).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Range {
    Year,
    Years3,
    Years10,
    All,
}

impl Range {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "1y" => Self::Year,
            "3y" => Self::Years3,
            "10y" => Self::Years10,
            "all" => Self::All,
            _ => return None,
        })
    }
}

/// The span of one column (rule 35), smallest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Step {
    Month,
    Season,
    Year,
    Decade,
    Quarter,
    Century,
}

impl Step {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Month => "month",
            Self::Season => "season",
            Self::Year => "year",
            Self::Decade => "decade",
            Self::Quarter => "quarter century",
            Self::Century => "century",
        }
    }

    fn larger(self) -> Option<Self> {
        Some(match self {
            Self::Month => Self::Season,
            Self::Season => Self::Year,
            Self::Year => Self::Decade,
            Self::Decade => Self::Quarter,
            Self::Quarter => Self::Century,
            Self::Century => return None,
        })
    }

    /// The column a month falls in. Seasons run December to February, March to May, June to August and September to November;
    /// decades, quarter centuries and centuries start at years divisible by 10, 25 and 100.
    fn bucket(self, month: i64) -> i64 {
        let year = month.div_euclid(12);
        match self {
            Self::Month => month,
            Self::Season => (month + 1).div_euclid(3),
            Self::Year => year,
            Self::Decade => year.div_euclid(10),
            Self::Quarter => year.div_euclid(25),
            Self::Century => year.div_euclid(100),
        }
    }

    /// The first and last month a column holds.
    fn span(self, bucket: i64) -> (i64, i64) {
        match self {
            Self::Month => (bucket, bucket),
            Self::Season => (bucket * 3 - 1, bucket * 3 + 1),
            Self::Year => (bucket * 12, bucket * 12 + 11),
            Self::Decade => (bucket * 120, bucket * 120 + 119),
            Self::Quarter => (bucket * 300, bucket * 300 + 299),
            Self::Century => (bucket * 1200, bucket * 1200 + 1199),
        }
    }
}

/// What a window shows the history of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Subject {
    Global,
    Tag(String),
    Place(String),
    Pop(u64),
}

/// A graph of a subject (rule 32).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Graph {
    People,
    BirthsDeaths,
    FoodDays,
    Price(String),
    Made,
    Used,
}

impl Graph {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "people" => Self::People,
            "births_deaths" => Self::BirthsDeaths,
            "food_days" => Self::FoodDays,
            "made" => Self::Made,
            "used" => Self::Used,
            other => Self::Price(other.strip_prefix("price:")?.to_owned()),
        })
    }

    fn stacked(&self) -> bool {
        matches!(self, Self::People | Self::Made | Self::Used)
    }

    fn rate(&self) -> bool {
        matches!(self, Self::BirthsDeaths)
    }
}

/// One request for a graph (rule 42).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub(crate) subject: Subject,
    pub(crate) graph: Graph,
    pub(crate) range: Range,
    /// The plot's column budget (rule 35).
    pub(crate) budget: u32,
    /// The window's number for this request, sent back with the answer.
    pub(crate) serial: u64,
}

/// One series' value in one column.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Value {
    Number(f64),
    /// A rate whose recorded months have no people (rule 36).
    Undefined,
}

/// One column of a chart.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Column {
    pub(crate) from: (u16, u8),
    pub(crate) to: (u16, u8),
    /// The months of the column that could be read; `0` is no record.
    pub(crate) months: u32,
    pub(crate) values: Vec<Value>,
    /// A rate graph's counts, one per series.
    pub(crate) counts: Vec<u64>,
    /// A rate graph's average people.
    pub(crate) people: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Chart {
    pub(crate) step: Step,
    pub(crate) horizon: (u16, u8),
    pub(crate) stacked: bool,
    pub(crate) rate: bool,
    pub(crate) series: Vec<String>,
    pub(crate) columns: Vec<Column>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Outcome {
    /// The horizon is before the history's first month (rule 34).
    NoRecords,
    /// Even centuries outnumber the plot's pixels (rule 35).
    TooNarrow,
    Chart(Chart),
}

/// The answer to one request.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Answer {
    pub(crate) request: u64,
    pub(crate) revision: u64,
    pub(crate) budget: u32,
    pub(crate) outcome: Outcome,
    /// For a pop's graph, the pop's own latest record in the whole history read, whatever range is plotted (its type, culture, religion and
    /// place), with what its label adds to its type and its number as the picker would give them at that record's month, so a window can
    /// name it exactly when the latest month's subjects do not list it (rules 23, 23a).
    pub(super) pop: Option<Labelled>,
}

/// Decoded year files by hash. Files never change, so an entry is right for as long as it is kept; a load drops them all.
#[derive(Default)]
pub(crate) struct Cache {
    years: HashMap<[u8; 32], Arc<Vec<MonthRecord>>>,
}

impl Cache {
    #[cfg(test)]
    fn len(&self) -> usize {
        self.years.len()
    }
}

/// The months of one year, read as rule 15 says: from the running year, from the held bytes of a year whose file may not be written
/// yet, or from its file, whose hash is checked before it is used. `None` is a gap (rule 13).
fn year_months(view: &View, cache: &Mutex<Cache>, year: u16) -> Option<Arc<Vec<MonthRecord>>> {
    if view.running.first().is_some_and(|month| month.year == year) {
        return Some(Arc::clone(&view.running));
    }
    let hash = view
        .years
        .iter()
        .find(|(listed, _)| *listed == year)
        .map(|(_, hash)| *hash)?;
    if let Some(found) = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .years
        .get(&hash)
    {
        return Some(Arc::clone(found));
    }
    let bytes = match view.pending.iter().find(|(held, _)| *held == hash) {
        Some((_, bytes)) => bytes.as_ref().clone(),
        None => std::fs::read(
            view.saves
                .join("history")
                .join(format!("{:016x}", view.campaign))
                .join(format!("{year:04}-{}.ckhist", hex(&hash))),
        )
        .ok()?,
    };
    if Sha256::digest(&bytes).as_slice() != hash {
        return None;
    }
    let months = Arc::new(decode_year(&bytes)?);
    cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .years
        .insert(hash, Arc::clone(&months));
    Some(months)
}

fn hex(hash: &[u8; 32]) -> String {
    use std::fmt::Write;
    hash.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// The recorded months in a span, read year by year as they are asked for.
struct Months<'a> {
    view: &'a View,
    cache: &'a Mutex<Cache>,
    years: HashMap<u16, Option<Arc<Vec<MonthRecord>>>>,
}

impl<'a> Months<'a> {
    fn new(view: &'a View, cache: &'a Mutex<Cache>) -> Self {
        Self {
            view,
            cache,
            years: HashMap::new(),
        }
    }

    fn get(&mut self, index: i64) -> Option<&MonthRecord> {
        let (year, month) = of_index(index);
        let (view, cache) = (self.view, self.cache);
        self.years
            .entry(year)
            .or_insert_with(|| year_months(view, cache, year))
            .as_ref()?
            .iter()
            .find(|record| record.month == month && record.year == year)
    }
}

/// The part of a month a subject's figures come from.
enum Cut<'a> {
    Places(Vec<&'a PlaceRecord>),
    Pop(&'a PopRecord),
}

fn cut<'a>(subject: &Subject, month: &'a MonthRecord) -> Option<Cut<'a>> {
    match subject {
        Subject::Global => Some(Cut::Places(month.places.iter().collect())),
        Subject::Tag(tag) => Some(Cut::Places(
            month
                .places
                .iter()
                .filter(|place| place.tag.as_deref() == Some(tag.as_str()))
                .collect(),
        )),
        Subject::Place(id) => month
            .places
            .iter()
            .find(|place| &place.id == id)
            .map(|place| Cut::Places(vec![place])),
        Subject::Pop(id) => month
            .places
            .iter()
            .flat_map(|place| place.pops.iter())
            .find(|pop| pop.id == *id)
            .map(Cut::Pop),
    }
}

/// One month's figures for a subject.
#[derive(Default)]
struct Sample {
    people: u64,
    workers: u64,
    dependents: u64,
    food_days: f64,
    births: u64,
    deaths: u64,
    price: Option<f64>,
    /// Value per good, in thousandths.
    made: BTreeMap<String, f64>,
    used: BTreeMap<String, f64>,
}

#[expect(
    clippy::cast_precision_loss,
    reason = "People and counts are far below 2^52; the figures are for drawing"
)]
fn sample(graph: &Graph, cut: &Cut<'_>) -> Sample {
    match cut {
        Cut::Pop(pop) => Sample {
            people: pop.people,
            workers: pop.workers,
            dependents: pop.dependents,
            food_days: pop.food_days as f64,
            births: pop.births,
            deaths: pop.deaths,
            ..Sample::default()
        },
        Cut::Places(places) => {
            let people: u64 = places.iter().map(|place| place.people).sum();
            let mut result = Sample {
                people,
                workers: places.iter().map(|place| place.workers).sum(),
                dependents: places.iter().map(|place| place.dependents).sum(),
                births: places.iter().map(|place| place.births).sum(),
                deaths: places.iter().map(|place| place.deaths).sum(),
                ..Sample::default()
            };
            // Food days and prices do not add: they are averaged weighted by people (rule 32).
            result.food_days = if people == 0 {
                0.0
            } else {
                places
                    .iter()
                    .map(|place| place.food_days as f64 * place.people as f64)
                    .sum::<f64>()
                    / people as f64
            };
            if let Graph::Price(good) = graph {
                let priced: Vec<(f64, f64)> = places
                    .iter()
                    .filter_map(|place| {
                        place
                            .goods
                            .iter()
                            .find(|entry| &entry.id == good)
                            .map(|entry| (entry.price as f64, place.people as f64))
                    })
                    .collect();
                let weight: f64 = priced.iter().map(|(_, people)| people).sum();
                result.price = if priced.is_empty() {
                    None
                } else if weight == 0.0 {
                    Some(priced.iter().map(|(price, _)| price).sum::<f64>() / priced.len() as f64)
                } else {
                    Some(
                        priced
                            .iter()
                            .map(|(price, people)| price * people)
                            .sum::<f64>()
                            / weight,
                    )
                };
            }
            if matches!(graph, Graph::Made | Graph::Used) {
                for place in places {
                    for entry in &place.goods {
                        *result.made.entry(entry.id.clone()).or_default() +=
                            entry.produced_value as f64;
                        *result.used.entry(entry.id.clone()).or_default() +=
                            entry.used_value as f64;
                    }
                }
            }
            result
        }
    }
}

/// The chart for `request` at `view`.
#[expect(
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    reason = "One pass picks the step, reads the months and builds the columns; counts are far below 2^52"
)]
pub(super) fn graph(view: &View, cache: &Mutex<Cache>, request: &Request) -> Answer {
    let answer = |outcome| Answer {
        request: request.serial,
        revision: view.revision,
        budget: request.budget,
        outcome,
        pop: None,
    };
    let Some(horizon) = view.horizon else {
        return answer(Outcome::NoRecords);
    };
    let origin = index_of(view.origin.0, view.origin.1);
    let last = index_of(horizon.0, horizon.1);
    if last < origin {
        return answer(Outcome::NoRecords);
    }
    let first = match request.range {
        Range::Year => last - 11,
        Range::Years3 => last - 35,
        Range::Years10 => last - 119,
        Range::All => origin,
    };
    let budget = i64::from(request.budget.min(MAX_COLUMNS));
    let mut step = match request.range {
        Range::Year | Range::Years3 => Step::Month,
        Range::Years10 => Step::Season,
        Range::All => Step::Year,
    };
    let count = |step: Step| step.bucket(last) - step.bucket(first) + 1;
    while count(step) > budget {
        match step.larger() {
            Some(larger) => step = larger,
            None => return answer(Outcome::TooNarrow),
        }
    }
    let graph = &request.graph;
    let mut months = Months::new(view, cache);
    let mut raw = Vec::new();
    for bucket in step.bucket(first)..=step.bucket(last) {
        let (low, high) = step.span(bucket);
        let (low, high) = (low.max(first), high.min(last));
        let mut samples = Vec::new();
        for index in low.max(origin)..=high {
            if let Some(month) = months.get(index)
                && let Some(part) = cut(&request.subject, month)
            {
                samples.push(sample(graph, &part));
            }
        }
        raw.push((low, high, samples));
    }
    let mut keys: Vec<String> = match graph {
        Graph::People => vec!["workers".into(), "dependents".into()],
        Graph::BirthsDeaths => vec!["births".into(), "deaths".into()],
        Graph::FoodDays => vec!["food days".into()],
        Graph::Price(good) => vec![good.clone()],
        Graph::Made | Graph::Used => {
            let mut totals: BTreeMap<&String, f64> = BTreeMap::new();
            for (_, _, samples) in &raw {
                for one in samples {
                    let by_good = if *graph == Graph::Made {
                        &one.made
                    } else {
                        &one.used
                    };
                    for (good, value) in by_good {
                        *totals.entry(good).or_default() += value;
                    }
                }
            }
            let mut ranked: Vec<(&String, f64)> = totals
                .into_iter()
                .filter(|(_, total)| *total > 0.0)
                .collect();
            ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            let mut named: Vec<String> = ranked
                .iter()
                .take(NAMED_GOODS)
                .map(|(good, _)| (*good).clone())
                .collect();
            if ranked.len() > NAMED_GOODS {
                named.push("other".into());
            }
            named
        }
    };
    let columns = raw
        .into_iter()
        .map(|(low, high, samples)| {
            let n = samples.len();
            let (from, to) = (of_index(low), of_index(high.max(low)));
            if n == 0 {
                return Column {
                    from,
                    to,
                    months: 0,
                    values: Vec::new(),
                    counts: Vec::new(),
                    people: 0.0,
                };
            }
            let nf = n as f64;
            let mean = |pick: &dyn Fn(&Sample) -> f64| samples.iter().map(pick).sum::<f64>() / nf;
            let people_sum: u64 = samples.iter().map(|one| one.people).sum();
            let mut counts = Vec::new();
            let values = match graph {
                Graph::People => vec![
                    Value::Number(mean(&|one| one.workers as f64)),
                    Value::Number(mean(&|one| one.dependents as f64)),
                ],
                Graph::BirthsDeaths => {
                    counts = vec![
                        samples.iter().map(|one| one.births).sum::<u64>(),
                        samples.iter().map(|one| one.deaths).sum::<u64>(),
                    ];
                    counts
                        .iter()
                        .map(|count| {
                            if people_sum == 0 {
                                Value::Undefined
                            } else {
                                // count / average people * 12 / months, per 1,000; the months cancel.
                                Value::Number(*count as f64 * 12_000.0 / people_sum as f64)
                            }
                        })
                        .collect()
                }
                Graph::FoodDays => vec![Value::Number(mean(&|one| one.food_days))],
                Graph::Price(_) => {
                    let priced: Vec<f64> = samples.iter().filter_map(|one| one.price).collect();
                    vec![if priced.is_empty() {
                        Value::Undefined
                    } else {
                        Value::Number(priced.iter().sum::<f64>() / priced.len() as f64)
                    }]
                }
                Graph::Made | Graph::Used => {
                    let mut sums: BTreeMap<&str, f64> = BTreeMap::new();
                    for one in &samples {
                        let by_good = if *graph == Graph::Made {
                            &one.made
                        } else {
                            &one.used
                        };
                        for (good, value) in by_good {
                            let key = if keys.contains(good) {
                                good.as_str()
                            } else {
                                "other"
                            };
                            *sums.entry(key).or_default() += value / 1000.0;
                        }
                    }
                    keys.iter()
                        .map(|key| Value::Number(sums.get(key.as_str()).copied().unwrap_or(0.0)))
                        .collect()
                }
            };
            Column {
                from,
                to,
                months: u32::try_from(n).unwrap_or(u32::MAX),
                values,
                counts,
                people: people_sum as f64 / nf,
            }
        })
        .collect();
    let mut result = answer(Outcome::Chart(Chart {
        step,
        horizon,
        stacked: graph.stacked(),
        rate: graph.rate(),
        series: std::mem::take(&mut keys),
        columns,
    }));
    if let Subject::Pop(id) = &request.subject {
        result.pop = pop_label(&mut months, origin, last, *id);
    }
    result
}

/// A pop's own latest record in the history from `origin` to `last` and its label as the picker gives it at that record's month (rules
/// 23, 23a): found from the newest month back, whatever range is plotted. `None` when the pop is in no month.
fn pop_label(months: &mut Months<'_>, origin: i64, last: i64, id: u64) -> Option<Labelled> {
    for index in (origin..=last).rev() {
        // A month that cannot be read has nothing to say of the pop: the search goes on to the month before it.
        let Some(month) = months.get(index) else {
            continue;
        };
        let Some(pops) = month
            .places
            .iter()
            .find(|place| place.pops.iter().any(|pop| pop.id == id))
            .map(|place| place.pops.clone())
        else {
            continue;
        };
        let mut ties = Vec::new();
        let mut rows = label_pops(&pops, &mut ties);
        let first = if ties.is_empty() {
            HashMap::new()
        } else {
            first_seen(months, origin, index, &ties)
        };
        number_ties(&mut rows, &first);
        return rows.into_iter().find(|row| row.pop.id == id);
    }
    None
}

fn text(value: &str) -> ConfigValue {
    ConfigValue::String(value.to_owned())
}

fn int(value: impl Into<i64>) -> ConfigValue {
    ConfigValue::Integer(value.into())
}

fn table<const N: usize>(fields: [(&str, ConfigValue); N]) -> ConfigValue {
    ConfigValue::Table(
        fields
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

fn period(month: (u16, u8)) -> [(&'static str, ConfigValue); 2] {
    [("year", int(month.0)), ("month", int(month.1))]
}

fn date_table(month: (u16, u8)) -> ConfigValue {
    table(period(month))
}

impl Answer {
    /// The table the UI gets (rule 42).
    pub(crate) fn to_value(&self) -> ConfigValue {
        let mut fields = BTreeMap::from([
            (
                "request".to_owned(),
                int(i64::try_from(self.request).unwrap_or(i64::MAX)),
            ),
            (
                "revision".to_owned(),
                int(i64::try_from(self.revision).unwrap_or(i64::MAX)),
            ),
            ("budget".to_owned(), int(self.budget)),
        ]);
        let mut add = |key: &str, value: ConfigValue| {
            fields.insert(key.to_owned(), value);
        };
        if let Some(pop) = &self.pop {
            add(
                "pop",
                table([
                    ("kind", text(&pop.pop.kind)),
                    ("culture", text(&pop.pop.culture)),
                    ("religion", text(&pop.pop.religion)),
                    ("place", text(&pop.pop.place)),
                    ("adds", text(pop.adds)),
                    ("ordinal", int(pop.ordinal)),
                ]),
            );
        }
        match &self.outcome {
            Outcome::NoRecords => add("message", text("no records yet")),
            Outcome::TooNarrow => add("message", text("too narrow")),
            Outcome::Chart(chart) => {
                add("step", text(chart.step.key()));
                add("horizon", date_table(chart.horizon));
                add("stacked", ConfigValue::Bool(chart.stacked));
                add("rate", ConfigValue::Bool(chart.rate));
                add(
                    "series",
                    ConfigValue::Array(chart.series.iter().map(|key| text(key)).collect()),
                );
                add(
                    "columns",
                    ConfigValue::Array(chart.columns.iter().map(Column::to_value).collect()),
                );
            }
        }
        ConfigValue::Table(fields)
    }
}

impl Column {
    fn to_value(&self) -> ConfigValue {
        let mut fields = BTreeMap::from([
            ("from".to_owned(), date_table(self.from)),
            ("to".to_owned(), date_table(self.to)),
            ("months".to_owned(), int(self.months)),
            ("record".to_owned(), ConfigValue::Bool(self.months > 0)),
        ]);
        if self.months > 0 {
            fields.insert(
                "values".to_owned(),
                ConfigValue::Array(
                    self.values
                        .iter()
                        .map(|value| match value {
                            Value::Number(number) => ConfigValue::Float(*number),
                            Value::Undefined => ConfigValue::Bool(false),
                        })
                        .collect(),
                ),
            );
            if !self.counts.is_empty() {
                fields.insert(
                    "counts".to_owned(),
                    ConfigValue::Array(
                        self.counts
                            .iter()
                            .map(|count| int(i64::try_from(*count).unwrap_or(i64::MAX)))
                            .collect(),
                    ),
                );
                fields.insert("people".to_owned(), ConfigValue::Float(self.people));
            }
        }
        ConfigValue::Table(fields)
    }
}

/// The subjects and graphs the columns offer (rules 23a, 25, 42), from the latest month that can be read.
pub(super) fn subjects(view: &View, cache: &Mutex<Cache>) -> ConfigValue {
    let mut months = Months::new(view, cache);
    let origin = index_of(view.origin.0, view.origin.1);
    let latest = view
        .horizon
        .map(|(year, month)| index_of(year, month))
        .and_then(|last| {
            (origin..=last)
                .rev()
                .find(|index| months.get(*index).is_some())
        });
    let Some(latest) = latest else {
        return table([
            (
                "revision",
                int(i64::try_from(view.revision).unwrap_or(i64::MAX)),
            ),
            ("tags", ConfigValue::Array(vec![text(GLOBAL)])),
            ("places", table([(GLOBAL, ConfigValue::Array(Vec::new()))])),
            ("goods", ConfigValue::Array(Vec::new())),
            ("pops", ConfigValue::Table(BTreeMap::new())),
        ]);
    };
    let record = months.get(latest).cloned().expect("the month was found");
    let mut tags: BTreeSet<&str> = BTreeSet::new();
    for place in &record.places {
        if let Some(tag) = &place.tag {
            tags.insert(tag);
        }
    }
    let mut by_tag: BTreeMap<String, Vec<ConfigValue>> =
        BTreeMap::from([(GLOBAL.to_owned(), Vec::new())]);
    for place in &record.places {
        by_tag
            .entry(GLOBAL.to_owned())
            .or_default()
            .push(text(&place.id));
        if let Some(tag) = &place.tag {
            by_tag.entry(tag.clone()).or_default().push(text(&place.id));
        }
    }
    let goods = record
        .places
        .iter()
        .flat_map(|place| place.goods.iter().map(|good| good.id.clone()))
        .collect::<BTreeSet<_>>();
    let mut ties = Vec::new();
    let mut by_place = BTreeMap::new();
    for place in &record.places {
        let rows = label_pops(&place.pops, &mut ties);
        by_place.insert(place.id.clone(), rows);
    }
    let first_seen = if ties.is_empty() {
        HashMap::new()
    } else {
        first_seen(&mut months, origin, latest, &ties)
    };
    let pops = by_place
        .into_iter()
        .map(|(place, mut rows)| {
            number_ties(&mut rows, &first_seen);
            (
                place,
                ConfigValue::Array(rows.into_iter().map(Labelled::into_value).collect()),
            )
        })
        .collect();
    table([
        (
            "revision",
            int(i64::try_from(view.revision).unwrap_or(i64::MAX)),
        ),
        (
            "tags",
            ConfigValue::Array(std::iter::once(GLOBAL).chain(tags).map(text).collect()),
        ),
        (
            "places",
            ConfigValue::Table(
                by_tag
                    .into_iter()
                    .map(|(tag, places)| (tag, ConfigValue::Array(places)))
                    .collect(),
            ),
        ),
        (
            "goods",
            ConfigValue::Array(goods.iter().map(|good| text(good)).collect()),
        ),
        ("pops", ConfigValue::Table(pops)),
    ])
}

/// A pop and what its label adds to its type (rule 23a).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Labelled {
    pub(super) pop: PopRecord,
    /// `none`, `religion`, `culture` or `both`.
    adds: &'static str,
    /// `1`, `2`... when even `adds` leaves pops alike; `0` otherwise.
    ordinal: u32,
}

impl Labelled {
    fn into_value(self) -> ConfigValue {
        table([
            ("id", text(&format!("{:016x}", self.pop.id))),
            ("kind", text(&self.pop.kind)),
            ("culture", text(&self.pop.culture)),
            ("religion", text(&self.pop.religion)),
            ("adds", text(self.adds)),
            ("ordinal", int(self.ordinal)),
        ])
    }
}

/// Pops of one place with what tells same-type pops apart: their religion, then their culture, then both; pops still alike are
/// numbered once their first months are known (`ties` collects their ids).
fn label_pops(pops: &[PopRecord], ties: &mut Vec<u64>) -> Vec<Labelled> {
    let mut rows: Vec<Labelled> = pops
        .iter()
        .map(|pop| Labelled {
            pop: pop.clone(),
            adds: "none",
            ordinal: 0,
        })
        .collect();
    let kinds: BTreeSet<&str> = pops.iter().map(|pop| pop.kind.as_str()).collect();
    for kind in kinds {
        let group: Vec<usize> = (0..pops.len()).filter(|i| pops[*i].kind == kind).collect();
        if group.len() < 2 {
            continue;
        }
        let distinct = |key: &dyn Fn(&PopRecord) -> (&str, &str)| {
            group
                .iter()
                .map(|i| key(&pops[*i]))
                .collect::<BTreeSet<_>>()
                .len()
                == group.len()
        };
        let adds = if distinct(&|pop| (pop.religion.as_str(), "")) {
            "religion"
        } else if distinct(&|pop| (pop.culture.as_str(), "")) {
            "culture"
        } else {
            "both"
        };
        for i in &group {
            rows[*i].adds = adds;
        }
        if !distinct(&|pop| (pop.religion.as_str(), pop.culture.as_str())) {
            for i in &group {
                let alike = group.iter().filter(|j| {
                    pops[**j].religion == pops[*i].religion && pops[**j].culture == pops[*i].culture
                });
                if alike.count() > 1 {
                    rows[*i].ordinal = 1;
                    ties.push(pops[*i].id);
                }
            }
        }
    }
    rows
}

/// Numbers the pops that are still alike by first appearance, then by id (rule 23a).
fn number_ties(rows: &mut [Labelled], first_seen: &HashMap<u64, i64>) {
    let marked: Vec<usize> = (0..rows.len()).filter(|i| rows[*i].ordinal > 0).collect();
    for i in &marked {
        let mut alike: Vec<usize> = marked
            .iter()
            .copied()
            .filter(|j| {
                rows[*j].pop.kind == rows[*i].pop.kind
                    && rows[*j].pop.religion == rows[*i].pop.religion
                    && rows[*j].pop.culture == rows[*i].pop.culture
            })
            .collect();
        alike.sort_by_key(|j| {
            (
                first_seen
                    .get(&rows[*j].pop.id)
                    .copied()
                    .unwrap_or(i64::MAX),
                rows[*j].pop.id,
            )
        });
        let position = alike.iter().position(|j| j == i).unwrap_or(0);
        rows[*i].ordinal = u32::try_from(position + 1).unwrap_or(u32::MAX);
    }
}

/// The first readable month each of `ids` appears in, looking from the history's origin and stopping once all are found.
fn first_seen(months: &mut Months<'_>, origin: i64, latest: i64, ids: &[u64]) -> HashMap<u64, i64> {
    let mut found = HashMap::new();
    for index in origin..=latest {
        let Some(month) = months.get(index) else {
            continue;
        };
        for pop in month.places.iter().flat_map(|place| place.pops.iter()) {
            if ids.contains(&pop.id) {
                found.entry(pop.id).or_insert(index);
            }
        }
        if found.len() == ids.len() {
            break;
        }
    }
    found
}

/// What the owner publishes and the UI reads. Cheap to clone.
#[derive(Clone)]
pub(crate) struct Host {
    shared: Arc<Shared>,
}

struct Shared {
    view: Mutex<Option<Arc<View>>>,
    cache: Mutex<Cache>,
    answers: Mutex<VecDeque<Answer>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Host {
    pub(crate) fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            shared: Arc::new(Shared {
                view: Mutex::new(None),
                cache: Mutex::new(Cache::default()),
                answers: Mutex::new(VecDeque::new()),
                wake,
            }),
        }
    }

    /// Replaces the running history. `reset` is a load or a new game: the decoded years are dropped (rule 43).
    pub(super) fn publish(&self, view: View, reset: bool) {
        if reset {
            self.shared
                .cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .years
                .clear();
        }
        *self
            .shared
            .view
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(view));
    }

    fn view(&self) -> Option<Arc<View>> {
        self.shared
            .view
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The subjects and graphs now on offer (rule 42), read on the caller's thread.
    pub(crate) fn subjects(&self) -> Option<ConfigValue> {
        let view = self.view()?;
        Some(subjects(&view, &self.shared.cache))
    }

    /// Computes the graph on a thread of its own and queues the answer for [`take`](Self::take).
    pub(crate) fn request(&self, request: Request) {
        let shared = Arc::clone(&self.shared);
        let view = self.view();
        std::thread::spawn(move || {
            let answer = match view {
                Some(view) => graph(&view, &shared.cache, &request),
                None => Answer {
                    request: request.serial,
                    revision: 0,
                    budget: request.budget,
                    outcome: Outcome::NoRecords,
                    pop: None,
                },
            };
            shared
                .answers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(answer);
            (shared.wake)();
        });
    }

    /// Writes a chart table beside the saves, creating a distinct file on each click.
    pub(crate) fn export(&self, name: &str, bytes: &str) -> Result<String, String> {
        use std::io::Write;
        let view = self.view().ok_or("history unavailable")?;
        let directory = view.saves.join("exports");
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let mut slug = String::new();
        for ch in name.to_lowercase().chars() {
            if ch.is_alphanumeric() {
                slug.push(ch);
            } else if !slug.is_empty() && !slug.ends_with('-') {
                slug.push('-');
            }
        }
        let slug = slug.trim_end_matches('-');
        let slug = if slug.is_empty() { "graph" } else { slug };
        for number in 1_u32.. {
            let filename = if number == 1 {
                format!("{slug}.csv")
            } else {
                format!("{slug}-{number}.csv")
            };
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(directory.join(&filename))
            {
                Ok(mut file) => {
                    if let Err(error) = file.write_all(bytes.as_bytes()) {
                        drop(file);
                        let _ = std::fs::remove_file(directory.join(&filename));
                        return Err(error.to_string());
                    }
                    return Ok(filename);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("no available filename".into())
    }

    /// The answers that are ready, oldest first.
    pub(crate) fn take(&self) -> Vec<Answer> {
        self.shared
            .answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn cached_years(&self) -> usize {
        self.shared
            .cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::history::{GoodRecord, encode_year};

    fn place(id: &str, workers: u64, dependents: u64, births: u64, deaths: u64) -> PlaceRecord {
        PlaceRecord {
            id: id.into(),
            tag: None,
            people: workers + dependents,
            workers,
            dependents,
            food_days: 10,
            births,
            deaths,
            goods: Vec::new(),
            pops: Vec::new(),
        }
    }

    fn month(year: u16, month: u8, places: Vec<PlaceRecord>) -> MonthRecord {
        MonthRecord {
            year,
            month,
            places,
        }
    }

    fn view(origin: (u16, u8), horizon: Option<(u16, u8)>, running: Vec<MonthRecord>) -> View {
        View {
            revision: 7,
            saves: std::env::temp_dir().join("ck-history-view-missing"),
            campaign: 1,
            origin,
            years: Vec::new(),
            pending: Vec::new(),
            running: Arc::new(running),
            horizon,
        }
    }

    #[test]
    fn export_names_are_safe_and_collisions_preserve_existing_files() {
        let directory = std::env::temp_dir().join(format!(
            "ck-export-{}-{}",
            std::process::id(),
            next_revision()
        ));
        let mut fixture = view((770, 1), None, Vec::new());
        fixture.saves = directory.clone();
        let host = Host::new(Arc::new(|| {}));
        host.publish(fixture, true);
        let first = host.export("Cong/People 0770-01", "first\n").unwrap();
        let second = host.export("Cong/People 0770-01", "second\n").unwrap();
        assert_eq!(first, "cong-people-0770-01.csv");
        assert_eq!(second, "cong-people-0770-01-2.csv");
        assert_eq!(
            std::fs::read_to_string(directory.join("exports").join(first)).unwrap(),
            "first\n"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("exports").join(second)).unwrap(),
            "second\n"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn ask(view: &View, graph: Graph, range: Range, budget: u32) -> Outcome {
        let request = Request {
            subject: Subject::Global,
            graph,
            range,
            budget,
            serial: 3,
        };
        let answer = super::graph(view, &Mutex::new(Cache::default()), &request);
        assert_eq!(
            (answer.request, answer.revision, answer.budget),
            (3, 7, budget)
        );
        answer.outcome
    }

    fn chart(outcome: Outcome) -> Chart {
        let Outcome::Chart(chart) = outcome else {
            panic!("expected a chart, got {outcome:?}");
        };
        chart
    }

    fn date(year: u16, month: u8, day: u8, hour: u8) -> ck_sim::Date {
        ck_sim::Date {
            year,
            month,
            day,
            hour,
        }
    }

    #[test]
    fn the_horizon_follows_the_first_of_the_month_being_recorded() {
        let noon = super::super::core::ECONOMY_HOUR;
        assert_eq!(horizon_at(date(768, 11, 1, noon)), Some((768, 9)));
        assert_eq!(horizon_at(date(768, 11, 1, noon + 1)), Some((768, 10)));
        assert_eq!(horizon_at(date(768, 11, 2, 0)), Some((768, 10)));
        assert_eq!(horizon_at(date(769, 1, 1, noon + 1)), Some((768, 12)));
        assert_eq!(horizon_at(date(769, 1, 1, 0)), Some((768, 11)));
        assert_eq!(horizon_at(date(1, 1, 5, 0)), None);
    }

    #[test]
    fn steps_follow_the_range_and_grow_to_fit_the_budget() {
        let long = view((768, 10), Some((9999, 12)), Vec::new());
        // 768 to 9999 is 93 centuries; the narrowest window's plot holds them all.
        let all = chart(ask(&long, Graph::People, Range::All, 300));
        assert_eq!((all.step, all.columns.len()), (Step::Century, 93));
        assert_eq!(all.columns[0].from, (768, 10));
        assert_eq!(all.columns[92].to, (9999, 12));
        assert_eq!(
            ask(&long, Graph::People, Range::All, 92),
            Outcome::TooNarrow
        );
        assert_eq!(
            chart(ask(&long, Graph::People, Range::All, 93))
                .columns
                .len(),
            93
        );
        let quarter = chart(ask(&long, Graph::People, Range::All, 250));
        assert_eq!(
            quarter.step,
            Step::Century,
            "370 quarter centuries are too many"
        );
        // Ranges of the running game.
        let game = view((768, 10), Some((772, 5)), Vec::new());
        for (range, step, columns) in [
            (Range::Year, Step::Month, 12),
            (Range::Years3, Step::Month, 36),
            (Range::Years10, Step::Season, 40),
            (Range::All, Step::Year, 5),
        ] {
            let result = chart(ask(&game, Graph::People, range, 250));
            assert_eq!(
                (result.step, result.columns.len()),
                (step, columns),
                "{range:?}"
            );
        }
        // A plot narrower than a range's columns takes the next larger step.
        let narrow = chart(ask(&game, Graph::People, Range::Years3, 20));
        assert_eq!((narrow.step, narrow.columns.len()), (Step::Season, 12));
        let narrower = chart(ask(&game, Graph::People, Range::Years3, 5));
        assert_eq!((narrower.step, narrower.columns.len()), (Step::Year, 4));
        let none = view((768, 10), Some((772, 5)), Vec::new());
        assert_eq!(
            ask(&none, Graph::People, Range::Years3, 0),
            Outcome::TooNarrow
        );
    }

    #[test]
    fn seasons_run_december_to_february_and_so_on() {
        // Ten years back from November 770 begin in December 760.
        let game = view((760, 12), Some((770, 11)), Vec::new());
        let result = chart(ask(&game, Graph::People, Range::Years10, 250));
        assert_eq!(result.columns.len(), 40);
        let spans: Vec<_> = result
            .columns
            .iter()
            .take(5)
            .map(|c| (c.from, c.to))
            .collect();
        assert_eq!(
            spans,
            [
                ((760, 12), (761, 2)),
                ((761, 3), (761, 5)),
                ((761, 6), (761, 8)),
                ((761, 9), (761, 11)),
                ((761, 12), (762, 2)),
            ]
        );
    }

    #[test]
    fn before_the_first_record_there_is_nothing_to_draw() {
        assert_eq!(
            ask(
                &view((768, 10), None, Vec::new()),
                Graph::People,
                Range::All,
                250
            ),
            Outcome::NoRecords
        );
        assert_eq!(
            ask(
                &view((768, 10), Some((768, 9)), Vec::new()),
                Graph::People,
                Range::All,
                250
            ),
            Outcome::NoRecords
        );
    }

    #[test]
    fn counts_rates_and_averages_follow_rule_36() {
        let running = vec![
            month(768, 10, vec![place("cong", 6, 4, 1, 2)]),
            month(768, 11, vec![place("cong", 8, 2, 0, 1)]),
            month(768, 12, vec![place("cong", 5, 5, 3, 0)]),
        ];
        let game = view((768, 10), Some((768, 12)), running);
        let people = chart(ask(&game, Graph::People, Range::All, 250));
        assert_eq!(people.series, ["workers", "dependents"]);
        assert_eq!(people.columns.len(), 1);
        assert_eq!(
            people.columns[0].values,
            [Value::Number(19.0 / 3.0), Value::Number(11.0 / 3.0)]
        );
        assert!(people.stacked && !people.rate);
        let monthly = chart(ask(&game, Graph::BirthsDeaths, Range::Year, 250));
        assert_eq!(monthly.columns.len(), 12);
        let december = &monthly.columns[11];
        assert_eq!(december.counts, [3, 0]);
        // 3 births among 10 people in a month: 36 a year per 10, so 3,600 per 1,000 people a year.
        assert_eq!(
            december.values,
            [Value::Number(3_600.0), Value::Number(0.0)]
        );
        let all = chart(ask(&game, Graph::BirthsDeaths, Range::All, 250));
        let column = &all.columns[0];
        assert_eq!(column.counts, [4, 3]);
        assert_eq!(column.values[1], Value::Number(3.0 * 12_000.0 / 30.0));
        assert!((column.people - 10.0).abs() < 1e-9);
        assert!(all.rate && !all.stacked);
    }

    #[test]
    fn a_gap_is_no_record_and_a_rate_without_people_is_undefined() {
        let running = vec![
            month(770, 1, vec![place("cong", 0, 0, 0, 2)]),
            month(770, 3, vec![place("cong", 4, 1, 0, 0)]),
        ];
        let game = view((770, 1), Some((770, 3)), running);
        let result = chart(ask(&game, Graph::BirthsDeaths, Range::Year, 250));
        assert_eq!(
            result.columns.len(),
            12,
            "the year's columns stay, the history is shorter"
        );
        let (january, february, march) =
            (&result.columns[9], &result.columns[10], &result.columns[11]);
        assert_eq!((january.months, february.months, march.months), (1, 0, 1));
        assert_eq!(january.values, [Value::Undefined, Value::Undefined]);
        assert_eq!(january.counts, [0, 2], "an undefined rate keeps its count");
        assert!(february.values.is_empty() && february.counts.is_empty());
        assert_eq!(
            march.values,
            [Value::Number(0.0), Value::Number(0.0)],
            "a measured zero is a value"
        );
        assert!(
            result.columns[..9].iter().all(|column| column.months == 0),
            "before the origin"
        );
    }

    #[test]
    fn tags_add_their_places_and_weight_food_and_prices_by_people() {
        let mut a = place("a", 10, 0, 0, 0);
        let mut b = place("b", 30, 0, 0, 0);
        let c = place("c", 100, 0, 0, 0);
        a.tag = Some("mide".into());
        b.tag = Some("mide".into());
        a.food_days = 10;
        b.food_days = 20;
        a.goods = vec![GoodRecord {
            id: "oats".into(),
            price: 100,
            produced: 0,
            used: 0,
            lost: 0,
            produced_value: 0,
            used_value: 0,
        }];
        b.goods = vec![GoodRecord {
            id: "oats".into(),
            price: 200,
            produced: 0,
            used: 0,
            lost: 0,
            produced_value: 0,
            used_value: 0,
        }];
        let game = view(
            (768, 10),
            Some((768, 10)),
            vec![month(768, 10, vec![a, b, c])],
        );
        let request = |subject, graph| Request {
            subject,
            graph,
            range: Range::Year,
            budget: 250,
            serial: 1,
        };
        let run = |subject, graph| {
            let Outcome::Chart(chart) = super::graph(
                &game,
                &Mutex::new(Cache::default()),
                &request(subject, graph),
            )
            .outcome
            else {
                panic!("a chart");
            };
            chart.columns[11].values.clone()
        };
        assert_eq!(
            run(Subject::Tag("mide".into()), Graph::People),
            [Value::Number(40.0), Value::Number(0.0)]
        );
        assert_eq!(
            run(Subject::Tag("mide".into()), Graph::FoodDays),
            [Value::Number(17.5)]
        );
        assert_eq!(
            run(Subject::Tag("mide".into()), Graph::Price("oats".into())),
            [Value::Number(175.0)]
        );
        assert_eq!(
            run(Subject::Global, Graph::People),
            [Value::Number(140.0), Value::Number(0.0)]
        );
        assert_eq!(
            run(Subject::Place("b".into()), Graph::People),
            [Value::Number(30.0), Value::Number(0.0)]
        );
        assert_eq!(
            run(Subject::Place("c".into()), Graph::Price("oats".into())),
            [Value::Undefined]
        );
    }

    #[test]
    fn value_graphs_name_the_six_largest_goods_and_fold_the_rest_into_other() {
        let mut p = place("cong", 1, 0, 0, 0);
        p.goods = (0..8_u64)
            .map(|i| GoodRecord {
                id: format!("good{i}"),
                price: 0,
                produced: 0,
                used: 0,
                lost: 0,
                produced_value: u128::from(1000 * (i + 1)),
                used_value: 0,
            })
            .collect();
        let game = view((768, 10), Some((768, 10)), vec![month(768, 10, vec![p])]);
        let result = chart(ask(&game, Graph::Made, Range::Year, 250));
        assert_eq!(
            result.series,
            [
                "good7", "good6", "good5", "good4", "good3", "good2", "other"
            ]
        );
        assert_eq!(
            result.columns[11].values[6],
            Value::Number(3.0),
            "good1 and good0 are 2 + 1 value units"
        );
        assert_eq!(result.columns[11].values[0], Value::Number(8.0));
        assert!(result.stacked);
        let used = chart(ask(&game, Graph::Used, Range::Year, 250));
        assert!(used.series.is_empty(), "nothing was used, so no series");
    }

    #[test]
    fn year_files_are_read_with_their_hash_checked_and_a_missing_or_damaged_one_is_a_gap() {
        let dir = std::env::temp_dir().join(format!("ck-history-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.join("history").join(format!("{:016x}", 1_u64));
        std::fs::create_dir_all(&folder).unwrap();
        let years = |year: u16, people: u64| {
            let bytes =
                encode_year(&[month(year, 12, vec![place("cong", people, 0, 0, 0)])]).unwrap();
            let hash: [u8; 32] = Sha256::digest(&bytes).into();
            (bytes, hash)
        };
        let (first, first_hash) = years(768, 10);
        let (second, second_hash) = years(769, 20);
        let (third, third_hash) = years(770, 30);
        std::fs::write(
            folder.join(format!("0768-{}.ckhist", hex(&first_hash))),
            &first,
        )
        .unwrap();
        let mut damaged = second.clone();
        *damaged.last_mut().unwrap() ^= 1;
        std::fs::write(
            folder.join(format!("0769-{}.ckhist", hex(&second_hash))),
            &damaged,
        )
        .unwrap();
        let game = View {
            revision: 7,
            saves: dir.clone(),
            campaign: 1,
            origin: (768, 12),
            years: vec![(768, first_hash), (769, second_hash), (770, third_hash)],
            // 770's file is not written yet: its bytes are held.
            pending: vec![(third_hash, Arc::new(third))],
            running: Arc::new(Vec::new()),
            horizon: Some((770, 12)),
        };
        let cache = Mutex::new(Cache::default());
        let request = Request {
            subject: Subject::Global,
            graph: Graph::People,
            range: Range::All,
            budget: 250,
            serial: 1,
        };
        let answer = graph(&game, &cache, &request);
        let result = chart(answer.outcome);
        let workers: Vec<_> = result
            .columns
            .iter()
            .map(|c| c.values.first().copied())
            .collect();
        assert_eq!(
            workers,
            [Some(Value::Number(10.0)), None, Some(Value::Number(30.0))]
        );
        assert_eq!(
            cache.lock().unwrap().len(),
            2,
            "the damaged file is not kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_last_year_keeps_the_right_edge_and_a_missing_first_year_keeps_the_left() {
        let running = vec![month(770, 1, vec![place("cong", 5, 0, 0, 0)])];
        // The history began in October 768 and has read nothing before 770, and the horizon is 770-03.
        let game = view((768, 10), Some((770, 3)), running);
        let result = chart(ask(&game, Graph::People, Range::All, 250));
        assert_eq!(result.columns.len(), 3, "768, 769 and 770");
        assert_eq!(
            result.columns[0].from,
            (768, 10),
            "all starts at the origin"
        );
        assert_eq!(
            result.columns.iter().map(|c| c.months).collect::<Vec<_>>(),
            [0, 0, 1]
        );
        assert_eq!(result.horizon, (770, 3));
        let short = view(
            (768, 10),
            Some((770, 3)),
            vec![month(770, 1, vec![place("cong", 5, 0, 0, 0)])],
        );
        let recent = chart(ask(&short, Graph::People, Range::Years3, 250));
        assert_eq!(
            recent.columns.last().unwrap().to,
            (770, 3),
            "the chart never slides back"
        );
    }

    #[test]
    fn pops_are_labelled_by_what_tells_them_apart_and_numbered_when_nothing_does() {
        let pop = |id: u64, kind: &str, culture: &str, religion: &str| PopRecord {
            id,
            place: "cong".into(),
            kind: kind.into(),
            culture: culture.into(),
            religion: religion.into(),
            people: 1,
            workers: 1,
            dependents: 0,
            food_days: 1,
            births: 0,
            deaths: 0,
        };
        let mut ties = Vec::new();
        let split = label_pops(
            &[
                pop(1, "peasant", "irish", "christian"),
                pop(2, "peasant", "irish", "pagan"),
                pop(3, "lord", "irish", "christian"),
            ],
            &mut ties,
        );
        assert_eq!(
            split.iter().map(|row| row.adds).collect::<Vec<_>>(),
            ["religion", "religion", "none"]
        );
        assert!(ties.is_empty());
        let by_culture = label_pops(
            &[
                pop(1, "peasant", "irish", "christian"),
                pop(2, "peasant", "norse", "christian"),
            ],
            &mut ties,
        );
        assert_eq!(by_culture[0].adds, "culture");
        let mut alike = label_pops(
            &[
                pop(30, "peasant", "irish", "christian"),
                pop(20, "peasant", "irish", "christian"),
                pop(10, "peasant", "irish", "pagan"),
            ],
            &mut ties,
        );
        assert_eq!(alike[0].adds, "both");
        assert_eq!(ties, [30, 20]);
        // Pop 30 appeared before pop 20; ids break the rest.
        number_ties(&mut alike, &HashMap::from([(30, 5), (20, 9)]));
        assert_eq!(
            alike.iter().map(|row| row.ordinal).collect::<Vec<_>>(),
            [1, 2, 0]
        );
        number_ties(&mut alike, &HashMap::new());
        assert_eq!(
            alike.iter().map(|row| row.ordinal).collect::<Vec<_>>(),
            [2, 1, 0],
            "same month: lower id first"
        );
    }

    #[test]
    fn a_pops_answer_carries_its_own_latest_record_even_when_the_latest_month_lacks_it() {
        let pop = |place: &str, religion: &str| PopRecord {
            id: 9,
            place: place.into(),
            kind: "peasant".into(),
            culture: "irish".into(),
            religion: religion.into(),
            people: 5,
            workers: 3,
            dependents: 2,
            food_days: 4,
            births: 0,
            deaths: 0,
        };
        let mut early = place("cong", 3, 2, 0, 0);
        early.pops = vec![pop("cong", "pagan")];
        let mut moved = place("armagh", 3, 2, 0, 0);
        moved.pops = vec![pop("armagh", "christian")];
        let running = vec![
            month(770, 1, vec![early]),
            month(770, 2, vec![moved]),
            // The pop is in no place this month.
            month(770, 3, vec![place("cong", 1, 0, 0, 0)]),
        ];
        let game = view((770, 1), Some((770, 3)), running);
        let request = Request {
            subject: Subject::Pop(9),
            graph: Graph::People,
            range: Range::Year,
            budget: 250,
            serial: 1,
        };
        let answer = graph(&game, &Mutex::new(Cache::default()), &request);
        let record = answer.pop.expect("the pop's last record").pop;
        assert_eq!(
            (record.place.as_str(), record.religion.as_str()),
            ("armagh", "christian"),
            "its latest month, not the latest month of the game"
        );
        assert!(
            graph(
                &game,
                &Mutex::new(Cache::default()),
                &Request {
                    subject: Subject::Global,
                    ..request
                }
            )
            .pop
            .is_none()
        );
    }

    #[test]
    fn a_pops_label_is_its_own_latest_records_whatever_range_is_plotted_and_alike_pops_are_numbered()
     {
        let pop = |id: u64| PopRecord {
            id,
            place: "cong".into(),
            kind: "peasant".into(),
            culture: "irish".into(),
            religion: "pagan".into(),
            people: 5,
            workers: 3,
            dependents: 2,
            food_days: 4,
            births: 0,
            deaths: 0,
        };
        // Two pops alike in type, culture and religion at one place in 770, then neither is in any place for two years: a last record
        // far outside a year's range, and absent from the latest month's subjects.
        let mut early = place("cong", 6, 4, 0, 0);
        early.pops = vec![pop(9), pop(8)];
        let held = encode_year(&[month(770, 1, vec![early])]).unwrap();
        let hash: [u8; 32] = Sha256::digest(&held).into();
        let game = View {
            revision: 7,
            saves: std::env::temp_dir().join("ck-history-view-missing"),
            campaign: 1,
            origin: (770, 1),
            years: vec![(770, hash)],
            pending: vec![(hash, Arc::new(held))],
            running: Arc::new(vec![month(772, 6, vec![place("cong", 1, 0, 0, 0)])]),
            horizon: Some((772, 6)),
        };
        for range in [Range::Year, Range::Years3, Range::All] {
            for (id, ordinal) in [(8, 1), (9, 2)] {
                let request = Request {
                    subject: Subject::Pop(id),
                    graph: Graph::People,
                    range,
                    budget: 250,
                    serial: 1,
                };
                let answer = graph(&game, &Mutex::new(Cache::default()), &request);
                let named = answer.pop.expect("its own latest record, in range or not");
                assert_eq!(
                    (
                        named.pop.id,
                        named.pop.place.as_str(),
                        named.adds,
                        named.ordinal
                    ),
                    (id, "cong", "both", ordinal),
                    "{range:?}: alike pops are told apart as the picker does, by first appearance then id"
                );
            }
        }
        // What the Lua window reads carries the label's two parts.
        let request = Request {
            subject: Subject::Pop(9),
            graph: Graph::People,
            range: Range::Year,
            budget: 250,
            serial: 1,
        };
        let ConfigValue::Table(fields) =
            graph(&game, &Mutex::new(Cache::default()), &request).to_value()
        else {
            panic!("an answer is a table");
        };
        let ConfigValue::Table(pop) = &fields["pop"] else {
            panic!("a pop's answer names the pop");
        };
        assert_eq!(pop["adds"], text("both"));
        assert_eq!(pop["ordinal"], int(2));
    }

    #[test]
    fn subjects_list_global_and_tags_and_each_places_pops_from_the_latest_readable_month() {
        let mut a = place("a", 1, 0, 0, 0);
        a.tag = Some("mide".into());
        a.pops = vec![PopRecord {
            id: 0xabc,
            place: "a".into(),
            kind: "peasant".into(),
            culture: "irish".into(),
            religion: "pagan".into(),
            people: 1,
            workers: 1,
            dependents: 0,
            food_days: 1,
            births: 0,
            deaths: 0,
        }];
        let b = place("b", 1, 0, 0, 0);
        let game = view((768, 10), Some((768, 11)), vec![month(768, 10, vec![a, b])]);
        let ConfigValue::Table(result) = subjects(&game, &Mutex::new(Cache::default())) else {
            panic!("a table");
        };
        assert_eq!(
            result["tags"],
            ConfigValue::Array(vec![text("global"), text("mide")])
        );
        let ConfigValue::Table(places) = &result["places"] else {
            panic!("places")
        };
        assert_eq!(
            places["global"],
            ConfigValue::Array(vec![text("a"), text("b")])
        );
        assert_eq!(places["mide"], ConfigValue::Array(vec![text("a")]));
        let ConfigValue::Table(pops) = &result["pops"] else {
            panic!("pops")
        };
        let ConfigValue::Array(rows) = &pops["a"] else {
            panic!("rows")
        };
        let ConfigValue::Table(row) = &rows[0] else {
            panic!("row")
        };
        assert_eq!(row["id"], text("0000000000000abc"));
        // Without any record there is `global` alone (rule 25).
        let ConfigValue::Table(empty) = subjects(
            &view((768, 10), None, Vec::new()),
            &Mutex::new(Cache::default()),
        ) else {
            panic!("a table");
        };
        assert_eq!(empty["tags"], ConfigValue::Array(vec![text("global")]));
    }

    #[test]
    fn the_host_answers_off_thread_and_keeps_years_until_a_load_drops_them() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let host = Host::new(Arc::new(move || {
            let _ = sender.send(());
        }));
        let game = view(
            (768, 10),
            Some((768, 10)),
            vec![month(768, 10, vec![place("cong", 2, 0, 0, 0)])],
        );
        assert!(host.subjects().is_none(), "no history is published yet");
        host.publish(game, false);
        let request = |number| Request {
            subject: Subject::Global,
            graph: Graph::People,
            range: Range::Year,
            budget: 250,
            serial: number,
        };
        host.request(request(1));
        host.request(request(2));
        receiver.recv().unwrap();
        receiver.recv().unwrap();
        let mut answers = host.take();
        answers.sort_by_key(|answer| answer.request);
        assert_eq!(
            answers
                .iter()
                .map(|a| (a.request, a.revision))
                .collect::<Vec<_>>(),
            [(1, 7), (2, 7)]
        );
        assert!(host.take().is_empty());
        host.publish(view((768, 10), None, Vec::new()), true);
        assert_eq!(host.cached_years(), 0);
    }
}
