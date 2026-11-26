//! Validated economy data and deterministic milestone 9 economy systems and saved state.
//!
//! This crate is independent of maps, windows, Lua and renderers. The application
//! supplies authored scenario data and derived land counts through plain integers.

#![deny(unsafe_code)]

use std::{collections::BTreeMap, error::Error, fmt, fs, path::Path};

use ck_sim::{Canonical, CanonicalError, Date, Reader, days_in_month, encode_length};
use serde::Deserialize;

use start_levels::{Candidate, StartDemand};

pub mod bigint;
pub mod events;
mod fuel;
pub mod genetics;
pub mod lifecycle;
pub mod peoples;
pub mod personality;
mod start_levels;
mod swap;
/// Read-only comparisons of production methods.
pub mod switching;
pub use events::{Epithet, EpithetScope, EpithetTrigger, EventRules};
pub use genetics::{Genetics, PopGenetics};
pub use lifecycle::LifeCycle;
pub use peoples::Peoples;
pub use personality::Personality;

const WAGE_HISTORY_MONTHS: usize = 12;

const fn default_hire_pm() -> u16 {
    100
}

const fn default_leave_pm() -> u16 {
    50
}

/// A fully qualified data error: its source file, entry and problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataError {
    /// Source file or logical source name.
    pub file: String,
    /// Entry path within the file.
    pub entry: String,
    /// Description of the invalid value.
    pub problem: String,
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.file, self.entry, self.problem)
    }
}

impl Error for DataError {}

/// A good and its integer simulation units.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Good {
    /// Stable lower-case key.
    pub id: String,
    /// Open-vocabulary category keys shared by presentation and category queries.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Category units supplied per unit of this good, in thousandths.
    #[serde(default)]
    pub supplies: BTreeMap<String, u64>,
    /// Human-readable unit.
    pub unit: String,
    /// Weight in grams per unit.
    pub weight_g: u64,
    /// Cargo volume in millilitres per unit (zero for live herds).
    pub volume_ml: u64,
    /// Excluded from indoor storage volume, while retaining its cargo volume.
    #[serde(default)]
    pub stacked_outside: bool,
    /// Hunger and tier values in hundredths of a person-day per unit.
    pub hunger_centi: u32,
    /// Needs value in hundredths of a person-day per unit.
    pub necessities_centi: u32,
    /// Niceties value in hundredths of a person-day per unit.
    pub niceties_centi: u32,
    /// Luxuries value in hundredths of a person-day per unit.
    pub luxuries_centi: u32,
    /// Base value in whole notional value units.
    pub base_price: u64,
    /// Per-person storage cap in thousandths of a good unit.
    pub pop_limit_milli: u64,
}

impl Good {
    /// Whole units of this good needed for a category amount, if it belongs to it.
    #[must_use]
    pub fn units_for_category(&self, category: &str, amount: u64) -> Option<u128> {
        if !self.tags.iter().any(|tag| tag == category) {
            return None;
        }
        let supply = u128::from(*self.supplies.get(category).unwrap_or(&1000));
        if supply == 0 {
            return None;
        }
        Some((u128::from(amount) * 1000).div_ceil(supply))
    }

    /// Thousandths of `category` that `units` of this good supply, if it belongs to it.
    #[must_use]
    pub fn category_supplied(&self, category: &str, units: u128) -> u128 {
        if !self.tags.iter().any(|tag| tag == category) {
            return 0;
        }
        units.saturating_mul(u128::from(*self.supplies.get(category).unwrap_or(&1000)))
    }
}

/// A population type's startup and employment rules.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PopType {
    /// Stable lower-case key.
    pub id: String,
    /// Dependents per 1000 people.
    pub dependent_pm: u16,
    /// Births per 1000 workers per year.
    pub births_pm_year: u16,
    /// Deaths per 1000 workers per year.
    pub worker_deaths_pm_year: u16,
    /// Deaths per 1000 dependents per year.
    pub dependent_deaths_pm_year: u16,
    /// Necessities expectation in ‰.
    pub necessities_expectation_pm: u16,
    /// Niceties expectation in ‰.
    pub niceties_expectation_pm: u16,
    /// Luxuries expectation in ‰.
    pub luxuries_expectation_pm: u16,
    /// Fuel each person needs a day, in thousandths of a fuel unit; nothing until data names it.
    #[serde(default)]
    pub fuel_need_milli_day: u64,
}

/// Data describing one production method available to a building.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Method {
    /// Stable key.
    pub id: String,
    /// Maximum levels allowed by the resource this method consumes.
    pub resource: String,
    /// Terrain cells required for a forest or bog level.
    #[serde(default)]
    pub cells_per_level: Option<u32>,
    /// Annual output by good at suitability 1000.
    pub annual_output: BTreeMap<String, u64>,
    /// Per-good suitability category, normally terrain or sea.
    pub suitability: String,
    /// Output share in ‰ used to derive terrain land scores.
    pub score_share_pm: BTreeMap<String, u16>,
    /// The need the opening serves with this method: `food` (the default) or `fuel`. A level is
    /// placed on a method only for the need it serves.
    #[serde(default = "default_serves")]
    pub serves: String,
}

/// The need a method serves when its data names none.
fn default_serves() -> String {
    FOOD_NEED.to_owned()
}

/// The opening need that food methods serve.
pub const FOOD_NEED: &str = "food";
/// The opening need that fuel methods serve.
pub const FUEL_NEED: &str = "fuel";

/// One good received for each slaughtered animal.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HerdYield {
    /// Good identifier.
    pub good: String,
    /// Whole units per head.
    pub units: u64,
}

/// A recurring herd product, with one of season or month.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HerdProduct {
    /// Good identifier.
    pub good: String,
    /// Thousandths of a unit per head per year.
    pub amount_milli: Option<u64>,
    /// Seasonal share table.
    pub season: Option<String>,
    /// Month of payment, when not seasonal.
    pub month: Option<u8>,
}

/// Rules for a species, in display and ledger order.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HerdSpecies {
    /// Live animal good.
    pub animal: String,
    /// Births per thousand per year.
    pub births_pm_year: u64,
    /// Deaths per thousand per year.
    pub deaths_pm_year: u64,
    /// Emergency slaughter priority, lowest first.
    pub emergency_order: u32,
    /// Slaughter outputs in payout order.
    pub yields: Vec<HerdYield>,
    /// Recurring products in authored order.
    #[serde(default)]
    pub products: Vec<HerdProduct>,
}

/// Capacity per farming level.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HerdCapacity {
    /// Maximum head per level.
    pub maximum: u64,
}

/// A building's job positions, land use, owner coverage and production methods.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BuildingType {
    /// Stable key.
    pub id: String,
    /// Total paid positions per level.
    pub workers_per_level: u16,
    /// Cells used by a farming level.
    pub land_per_level: u16,
    /// Workers one owner covers, by the worker population type the building employs.
    pub owner_workers: BTreeMap<String, u16>,
    /// The population type whose members own the building, when it is not the worker kind
    /// named in `owner_workers` (the demesne's workers are labourers but its owners are nobles).
    /// Without it, owners come from the same kinds as the workers.
    #[serde(default)]
    pub owner_kind: Option<String>,
    /// Output penalty when owner coverage is incomplete, in ‰.
    pub penalty_pm: u16,
    /// Value retained for owners after the annual wage pool, in per-mille.
    pub overhead_pm: u16,
    /// Maximum ground-distance in metres for recruiting into this building.
    /// `None` is retained during deserialization so validation can name a missing key.
    pub recruit_range_m: Option<u32>,
    /// Market capacity one level gives to the building's owners, in millilitres a day (CK-391). Nothing when left out.
    #[serde(default)]
    pub market_capacity_ml_per_level: u64,
    /// Whether this building receives local surplus as a church building.
    #[serde(default)]
    pub church: bool,
    /// Species kept by this building, keyed by animal good.
    #[serde(default)]
    pub herds: BTreeMap<String, HerdCapacity>,
    /// Ordered methods; their order resolves equal estimates.
    #[serde(default)]
    pub methods: Vec<Method>,
}

/// Parameters for the seeded opening herd distribution.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HerdOpening {
    /// Chance per mille of keeping pigs and sheep.
    pub keep_pm: u16,
    /// Court sizes separating the three chances.
    pub court_thresholds: [u64; 2],
    /// Horse keeping chances by court size.
    pub court_chances: [u16; 3],
    /// Divisor of the grazing bonus.
    pub grazing_divisor: i64,
    /// Inclusive per-mille spread bounds.
    pub spread: [u64; 2],
    /// Demesne rank multiplier per mille.
    pub demesne_rank_pm: u64,
    /// Minimum head per farming level.
    pub floor_per_level: u64,
}

impl Default for HerdOpening {
    fn default() -> Self {
        Self {
            keep_pm: 750,
            court_thresholds: [40, 80],
            court_chances: [250, 500, 750],
            grazing_divisor: 4,
            spread: [500, 1500],
            demesne_rank_pm: 1500,
            floor_per_level: 1,
        }
    }
}

/// An authored economy rules file.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// Opening share of each herd maximum, numerator and denominator.
    pub herd_start_share: [u64; 2],
    /// A herd's standard level in per-mille of its maximum (CK-430, r163): owners offer the animals above it and want the animals
    /// below it. It is not the opening share.
    pub herd_standard_pm: u64,
    /// Parameters for the opening distribution.
    #[serde(default)]
    pub herd_opening: HerdOpening,
    /// Herd species in canonical order.
    #[serde(default)]
    pub herd_species: Vec<HerdSpecies>,
    /// Goods in canonical authoring order.
    pub goods: Vec<Good>,
    /// Population types.
    pub pop_types: Vec<PopType>,
    /// Starting necessities, niceties and luxuries by population type, in per-mille.
    #[serde(default)]
    pub starting_needs_pm: BTreeMap<String, [u16; 3]>,
    /// Validated culture and language data.
    #[serde(skip)]
    pub peoples: Peoples,
    /// Validated genetic data: haplogroup tables, start sets and trait rules (CK-361).
    #[serde(skip)]
    pub genetics: Genetics,
    /// Validated personality data: pairs, birth, humour drift, stress, standing and opinion (CK-362).
    #[serde(skip)]
    pub personality: Personality,
    /// Validated character event data: the catalog, events, deeds, precedence and skills (CK-363).
    #[serde(skip)]
    pub events: EventRules,
    /// Validated life-cycle data: death, majority, relations, conception and the name pools (CK-364).
    #[serde(skip)]
    pub lifecycle: LifeCycle,
    /// Religions present in the simulation.
    pub religions: Vec<String>,
    /// Land terrain names in the same order as the map terrain palette.
    pub terrain_order: Vec<String>,
    /// Footprint per building level, in square metres.
    pub footprint_m2: BTreeMap<String, i64>,
    /// Housing capacity per building level.
    pub housing_per_level: BTreeMap<String, u32>,
    /// Housing quality class per building type.
    pub housing_class: BTreeMap<String, String>,
    /// Housing classes ordered from lowest to highest quality.
    pub housing_classes: Vec<String>,
    /// Natural makeshift shelter in thousandths of a person per land cell.
    pub shelter_pm: BTreeMap<String, u16>,
    /// Annual dependent deaths at full fuel shortage, per mille.
    #[serde(default)]
    pub fuel_deaths_pm_year: u16,
    /// Fraction of open positions filled each month, in per-mille.
    #[serde(default = "default_hire_pm")]
    pub hire_pm: u16,
    /// Fraction of a source pop that may leave each month, in per-mille.
    #[serde(default = "default_leave_pm")]
    pub leave_pm: u16,
    /// Buildable allowance before terrain contributions, in square metres.
    pub buildable_base_m2: u64,
    /// Buildable allowance per land cell by terrain, in square metres.
    pub buildable_per_cell_m2: BTreeMap<String, u64>,
    /// Starting development in thousandths.
    pub starting_development_pm: u32,
    /// Fertility that preserves the existing output, in per-mille.
    pub fertility_reference_pm: u16,
    /// Building definitions.
    pub buildings: Vec<BuildingType>,
    /// Seasonal shares by product, each summing to 1000.
    pub seasons: BTreeMap<String, Vec<u16>>,
    /// Response curves by name, with strictly increasing x coordinates.
    pub curves: BTreeMap<String, Vec<[i32; 2]>>,
    /// Zero-hunger good consumption, in millionths of a unit per person per day.
    #[serde(default)]
    pub ticking_rates_million: BTreeMap<String, BTreeMap<String, u64>>,
    /// Diet shares by profile and good, in ‰.
    pub diet_profiles: BTreeMap<String, BTreeMap<String, u16>>,
    /// Niceties and luxuries buy-order shares by population type and tier, in ‰.
    #[serde(default)]
    pub comfort_profiles: BTreeMap<String, BTreeMap<String, BTreeMap<String, u16>>>,
    /// Land suitability by terrain and product key, in ‰.
    pub suitability: BTreeMap<String, BTreeMap<String, u16>>,
    /// Grazing suitability by terrain and animal key, in ‰.
    pub grazing: BTreeMap<String, BTreeMap<String, u16>>,
    /// Extra margin a method switch must earn, in per-mille of staying value.
    #[serde(default = "default_method_switch_margin_pm")]
    pub method_switch_margin_pm: u16,
    /// Target-price response in per-mille.
    pub price_k_pm: u32,
    /// The most a swap's starter may lose to rounding, as a share of what it gives, in per-mille (CK-391).
    #[serde(default = "default_swap_loss_limit_pm")]
    pub swap_loss_limit_pm: u16,
}

fn default_method_switch_margin_pm() -> u16 {
    200
}

fn default_swap_loss_limit_pm() -> u16 {
    100
}

impl Rules {
    /// Cheapest single good able to meet the whole need; ties follow goods order.
    /// Missing prices exclude a good; missing stock is zero when stock is supplied.
    #[must_use]
    pub fn choose_category<'a>(
        &'a self,
        category: &str,
        amount: u64,
        prices: &BTreeMap<String, u64>,
        stock: Option<&BTreeMap<String, u64>>,
    ) -> Option<(&'a Good, u128)> {
        self.goods
            .iter()
            .filter_map(|good| {
                let units = good.units_for_category(category, amount)?;
                let price = *prices.get(&good.id)?;
                if stock.is_some_and(|stock| u128::from(*stock.get(&good.id).unwrap_or(&0)) < units)
                {
                    return None;
                }
                let low =
                    |value: u128| u64::try_from(value & u128::from(u64::MAX)).unwrap_or_default();
                let low_product = u128::from(low(units)) * u128::from(price);
                let high_product = (units >> 64) * u128::from(price) + (low_product >> 64);
                Some((good, units, (high_product, low(low_product))))
            })
            .min_by_key(|(_, _, worth)| *worth)
            .map(|(good, units, _)| (good, units))
    }

    /// Parse and validate a strict TOML economy rules file.
    ///
    /// # Errors
    /// Returns a source-qualified error for unreadable, malformed or invalid data.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DataError> {
        let path = path.as_ref();
        let file = path.display().to_string();
        let text = fs::read_to_string(path).map_err(|error| DataError {
            file: file.clone(),
            entry: "<file>".to_owned(),
            problem: error.to_string(),
        })?;
        let mut rules: Self = toml::from_str(&text).map_err(|error| DataError {
            file: file.clone(),
            entry: "<document>".to_owned(),
            problem: error.message().to_owned(),
        })?;
        rules.peoples = Peoples::load(&path.with_file_name("peoples.toml"))?;
        for religion in &rules.religions {
            if rules.peoples.liturgical_language(religion).is_none() {
                return Err(DataError {
                    file: path.with_file_name("peoples.toml").display().to_string(),
                    entry: format!("religions.{religion}"),
                    problem: "missing religion".to_owned(),
                });
            }
        }
        rules.genetics = Genetics::load(&path.with_file_name("genetics"), &rules.peoples)?;
        let cultures: Vec<&str> = rules
            .peoples
            .cultures
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        rules.personality = Personality::load(&path.with_file_name("personality.toml"), &cultures)?;
        let pop_types: Vec<&str> = rules.pop_types.iter().map(|p| p.id.as_str()).collect();
        rules.events = EventRules::load(
            &path.with_file_name("events"),
            &events::Registry {
                peoples: &rules.peoples,
                genetics: &rules.genetics,
                pop_types: &pop_types,
            },
        )?;
        rules.lifecycle = LifeCycle::load(
            &path.with_file_name("lifecycle"),
            &lifecycle::Registry {
                events: &rules.events,
                genetics: &rules.genetics,
                peoples: &rules.peoples,
                pop_types: &pop_types,
                religions: &rules.religions,
            },
        )?;
        rules.validate(&file)?;
        Ok(rules)
    }

    /// Validate uniqueness, ranges, references, curves and normalized profiles.
    ///
    /// # Errors
    /// Returns the first source-qualified validation error, in authoring order.
    #[expect(
        clippy::too_many_lines,
        reason = "Validation order is intentional: authors get the first invalid entry in file order."
    )]
    pub fn validate(&self, file: &str) -> Result<(), DataError> {
        let refuse = |entry: String, problem: String| DataError {
            file: file.to_owned(),
            entry,
            problem,
        };
        if self.method_switch_margin_pm > 1000 {
            return Err(refuse(
                "method_switch_margin_pm".to_owned(),
                "must be at most 1000".to_owned(),
            ));
        }
        if self.herd_start_share[0] == 0
            || self.herd_start_share[1] == 0
            || self.herd_start_share[0] > self.herd_start_share[1]
        {
            return Err(refuse(
                "herd_start_share".to_owned(),
                "invalid share".to_owned(),
            ));
        }
        if self.herd_standard_pm == 0 || self.herd_standard_pm > 1000 {
            return Err(refuse(
                "herd_standard_pm".to_owned(),
                "must be from 1 to 1000".to_owned(),
            ));
        }
        let opening = &self.herd_opening;
        for (entry, bad) in [
            ("keep_pm", opening.keep_pm > 1000),
            (
                "court_chances",
                opening.court_chances.iter().any(|v| *v > 1000),
            ),
            (
                "court_thresholds",
                opening.court_thresholds[0] == 0
                    || opening.court_thresholds[0] >= opening.court_thresholds[1],
            ),
            ("grazing_divisor", opening.grazing_divisor <= 0),
            (
                "spread",
                opening.spread[0] == 0
                    || opening.spread[0] > opening.spread[1]
                    || opening.spread[1] == u64::MAX,
            ),
            ("demesne_rank_pm", opening.demesne_rank_pm == 0),
            ("floor_per_level", opening.floor_per_level == 0),
        ] {
            if bad {
                return Err(refuse(
                    format!("herd_opening.{entry}"),
                    "invalid value".to_owned(),
                ));
            }
        }
        unique_ids(
            file,
            "goods",
            self.goods.iter().map(|item| item.id.as_str()),
        )?;
        for good in &self.goods {
            let entry = format!("goods.{}.tags", good.id);
            let mut found = std::collections::BTreeSet::new();
            for tag in &good.tags {
                let bytes = tag.as_bytes();
                let valid_key = bytes.first().is_some_and(u8::is_ascii_lowercase)
                    && bytes[1..].iter().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_'
                    });
                if !valid_key || bytes.len() > 24 {
                    return Err(refuse(
                        entry,
                        format!("tag \"{tag}\" is not a lower-case key of at most 24 characters"),
                    ));
                }
                if !found.insert(tag) {
                    return Err(refuse(entry, format!("tag \"{tag}\" is listed twice")));
                }
            }
        }
        for good in &self.goods {
            for (category, supply) in &good.supplies {
                let entry = format!("goods.{}.supplies.{category}", good.id);
                if !good.tags.contains(category) {
                    return Err(refuse(
                        entry,
                        "category is not among the good's tags".to_owned(),
                    ));
                }
                if *supply == 0 {
                    return Err(refuse(entry, "supply must be positive".to_owned()));
                }
            }
        }
        unique_ids(
            file,
            "pop_types",
            self.pop_types.iter().map(|item| item.id.as_str()),
        )?;
        unique_ids(
            file,
            "buildings",
            self.buildings.iter().map(|item| item.id.as_str()),
        )?;
        unique_ids(file, "religions", self.religions.iter().map(String::as_str))?;
        unique_ids(
            file,
            "terrain_order",
            self.terrain_order.iter().map(String::as_str),
        )?;
        unique_ids(
            file,
            "housing_classes",
            self.housing_classes.iter().map(String::as_str),
        )?;
        if self.housing_classes.is_empty() {
            return Err(refuse(
                "housing_classes".to_owned(),
                "at least one class is required".to_owned(),
            ));
        }
        for building in &self.buildings {
            if building.recruit_range_m.is_none() {
                return Err(refuse(
                    format!("buildings.{}.recruit_range_m", building.id),
                    "entry is missing".to_owned(),
                ));
            }
            let Some(capacity) = self.housing_per_level.get(&building.id) else {
                return Err(refuse(
                    format!("housing_per_level.{}", building.id),
                    "entry is missing".to_owned(),
                ));
            };
            if *capacity == 0 {
                return Err(refuse(
                    format!("housing_per_level.{}", building.id),
                    "must be positive".to_owned(),
                ));
            }
            let Some(class) = self.housing_class.get(&building.id) else {
                return Err(refuse(
                    format!("housing_class.{}", building.id),
                    "entry is missing".to_owned(),
                ));
            };
            if !self.housing_classes.contains(class) {
                return Err(refuse(
                    format!("housing_class.{}", building.id),
                    "class is absent from housing_classes".to_owned(),
                ));
            }
        }
        for building in self
            .housing_per_level
            .keys()
            .chain(self.housing_class.keys())
        {
            if !self.buildings.iter().any(|item| item.id == *building) {
                return Err(refuse(
                    format!("housing_per_level.{building}"),
                    "unknown building".to_owned(),
                ));
            }
        }
        for terrain in &self.terrain_order {
            let Some(value) = self.shelter_pm.get(terrain) else {
                return Err(refuse(
                    format!("shelter_pm.{terrain}"),
                    "entry is missing".to_owned(),
                ));
            };
            if *value > 1000 {
                return Err(refuse(
                    format!("shelter_pm.{terrain}"),
                    "must be at most 1000".to_owned(),
                ));
            }
        }
        for terrain in self.shelter_pm.keys() {
            if !self.terrain_order.contains(terrain) {
                return Err(refuse(
                    format!("shelter_pm.{terrain}"),
                    "terrain is absent from terrain_order".to_owned(),
                ));
            }
        }
        if self.hire_pm > 1000 {
            return Err(refuse(
                "hire_pm".to_owned(),
                "must be at most 1000".to_owned(),
            ));
        }
        if self.leave_pm > 1000 {
            return Err(refuse(
                "leave_pm".to_owned(),
                "must be at most 1000".to_owned(),
            ));
        }
        if self.buildable_base_m2 == 0 {
            return Err(refuse(
                "buildable_base_m2".to_owned(),
                "must be positive".to_owned(),
            ));
        }
        if self.starting_development_pm == 0 {
            return Err(refuse(
                "starting_development_pm".to_owned(),
                "must be positive".to_owned(),
            ));
        }
        if self.fertility_reference_pm == 0 {
            return Err(refuse(
                "fertility_reference_pm".to_owned(),
                "must be positive".to_owned(),
            ));
        }
        for building in &self.buildings {
            let Some(value) = self.footprint_m2.get(&building.id) else {
                return Err(refuse(
                    format!("footprint_m2.{}", building.id),
                    "entry is missing".to_owned(),
                ));
            };
            if *value <= 0 {
                return Err(refuse(
                    format!("footprint_m2.{}", building.id),
                    "must be positive".to_owned(),
                ));
            }
        }
        for building in self.footprint_m2.keys() {
            if !self.buildings.iter().any(|item| item.id == *building) {
                return Err(refuse(
                    format!("footprint_m2.{building}"),
                    "unknown building".to_owned(),
                ));
            }
        }
        for terrain in &self.terrain_order {
            if !self.buildable_per_cell_m2.contains_key(terrain) {
                return Err(refuse(
                    format!("buildable_per_cell_m2.{terrain}"),
                    "entry is missing".to_owned(),
                ));
            }
        }
        for terrain in self.buildable_per_cell_m2.keys() {
            if !self.terrain_order.contains(terrain) {
                return Err(refuse(
                    format!("buildable_per_cell_m2.{terrain}"),
                    "terrain is absent from terrain_order".to_owned(),
                ));
            }
        }
        if self.price_k_pm > 10_000 {
            return Err(refuse(
                "price_k_pm".to_owned(),
                "must be at most 10000".to_owned(),
            ));
        }
        if self.swap_loss_limit_pm > 1000 {
            return Err(refuse(
                "swap_loss_limit_pm".to_owned(),
                "must be at most 1000".to_owned(),
            ));
        }
        let goods: std::collections::BTreeSet<_> =
            self.goods.iter().map(|good| good.id.as_str()).collect();
        for (index, good) in self.goods.iter().enumerate() {
            if good.id.is_empty()
                || good.unit.is_empty()
                || good.base_price == 0
                || good.base_price > 1_000_000
            {
                return Err(refuse(
                    format!("goods[{index}]"),
                    "id and unit must be non-empty; base_price must be 1..=1000000".to_owned(),
                ));
            }
            if good.pop_limit_milli > 1_000_000_000 {
                return Err(refuse(
                    format!("goods[{index}].pop_limit_milli"),
                    "must be at most 1000000000".to_owned(),
                ));
            }
        }
        for (index, pop) in self.pop_types.iter().enumerate() {
            if pop.id.is_empty() || pop.dependent_pm > 1000 {
                return Err(refuse(
                    format!("pop_types[{index}]"),
                    "invalid id or dependent share".to_owned(),
                ));
            }
            for (field, value) in [
                ("births_pm_year", pop.births_pm_year),
                ("worker_deaths_pm_year", pop.worker_deaths_pm_year),
                ("dependent_deaths_pm_year", pop.dependent_deaths_pm_year),
                ("necessities_expectation_pm", pop.necessities_expectation_pm),
                ("niceties_expectation_pm", pop.niceties_expectation_pm),
                ("luxuries_expectation_pm", pop.luxuries_expectation_pm),
            ] {
                if value > 10_000 {
                    return Err(refuse(
                        format!("pop_types[{}].{field}", pop.id),
                        "must be at most 10000".to_owned(),
                    ));
                }
            }
        }
        for (kind, needs) in &self.starting_needs_pm {
            if !self.pop_types.iter().any(|pop| pop.id == *kind) {
                return Err(refuse(
                    format!("starting_needs_pm.{kind}"),
                    "unknown population type".to_owned(),
                ));
            }
            if needs.iter().any(|value| *value > 1000) {
                return Err(refuse(
                    format!("starting_needs_pm.{kind}"),
                    "each starting tier must be in 0..=1000‰".to_owned(),
                ));
            }
        }
        for pop in &self.pop_types {
            if !self.starting_needs_pm.contains_key(&pop.id) {
                return Err(refuse(
                    format!("starting_needs_pm.{}", pop.id),
                    "starting needs are missing for this population type".to_owned(),
                ));
            }
        }
        for (index, building) in self.buildings.iter().enumerate() {
            if building.id.is_empty()
                || building.workers_per_level == 0
                || building.penalty_pm > 1000
                || building.overhead_pm > 1000
                || (building.methods.is_empty() && building.id != "storehouse")
            {
                return Err(refuse(format!("buildings[{index}]"), "id, workers_per_level and production methods are required (except for storehouses)".to_owned()));
            }
            if (building.id != "storehouse" && building.owner_workers.is_empty())
                || building.owner_workers.values().any(|workers| *workers == 0)
                || building
                    .owner_workers
                    .keys()
                    .any(|kind| !self.pop_types.iter().any(|pop| pop.id == *kind))
            {
                return Err(refuse(
                    format!("buildings[{index}].owner_workers"),
                    "owner ratios must name known population types and be positive (except storehouses)".to_owned(),
                ));
            }
            if let Some(owner_kind) = &building.owner_kind
                && !self.pop_types.iter().any(|pop| pop.id == *owner_kind)
            {
                return Err(refuse(
                    format!("buildings[{index}].owner_kind"),
                    "owner kind must name a known population type".to_owned(),
                ));
            }
            let required_owner_pools: &[&str] = match building.id.as_str() {
                "farm" => &["peasant"],
                "holdings" => &["monk", "labourer"],
                _ => &[],
            };
            if required_owner_pools
                .iter()
                .any(|kind| !building.owner_workers.contains_key(*kind))
            {
                return Err(refuse(
                    format!("buildings[{index}].owner_workers"),
                    format!(
                        "missing owner ratio for {}",
                        required_owner_pools.join(", ")
                    ),
                ));
            }
            unique_ids(
                file,
                &format!("buildings[{}].methods", building.id),
                building.methods.iter().map(|item| item.id.as_str()),
            )?;
            for method in &building.methods {
                if method.id.is_empty()
                    || method.resource.is_empty()
                    || method.annual_output.is_empty()
                {
                    return Err(refuse(
                        format!("buildings[{index}].methods.{}", method.id),
                        "method needs a key, resource and output".to_owned(),
                    ));
                }
                for good in method.annual_output.keys() {
                    if !goods.contains(good.as_str()) {
                        return Err(refuse(
                            format!(
                                "buildings[{index}].methods.{}.annual_output.{good}",
                                method.id
                            ),
                            "unknown good".to_owned(),
                        ));
                    }
                }
                let fuel_outputs = method
                    .annual_output
                    .keys()
                    .filter(|id| {
                        self.goods
                            .iter()
                            .any(|good| &good.id == *id && good.category_supplied("fuel", 1) > 0)
                    })
                    .count();
                if fuel_outputs > 1 {
                    return Err(refuse(
                        format!("buildings[{index}].methods.{}", method.id),
                        "a method may make at most one fuel".to_owned(),
                    ));
                }
                if !["usable_land", "sea", "forest", "bog"].contains(&method.resource.as_str()) {
                    return Err(refuse(
                        format!("buildings[{index}].methods.{}.resource", method.id),
                        "must be `usable_land`, `sea`, `forest` or `bog`".to_owned(),
                    ));
                }
                let terrain_resource = ["forest", "bog"].contains(&method.resource.as_str());
                if (terrain_resource && method.cells_per_level.unwrap_or(0) == 0)
                    || (!terrain_resource && method.cells_per_level.is_some())
                {
                    return Err(refuse(
                        format!("buildings[{index}].methods.{}.cells_per_level", method.id),
                        "forest and bog require positive cells per level; other resources must omit it".to_owned(),
                    ));
                }
                if ![FOOD_NEED, FUEL_NEED].contains(&method.serves.as_str()) {
                    return Err(refuse(
                        format!("buildings[{index}].methods.{}.serves", method.id),
                        "must be `food` or `fuel`".to_owned(),
                    ));
                }
                if method
                    .score_share_pm
                    .values()
                    .copied()
                    .map(u32::from)
                    .sum::<u32>()
                    != 1000
                {
                    return Err(refuse(
                        format!("buildings[{index}].methods.{}.score_share_pm", method.id),
                        "shares must sum to 1000".to_owned(),
                    ));
                }
                for good in method.score_share_pm.keys() {
                    if !goods.contains(good.as_str()) {
                        return Err(refuse(
                            format!(
                                "buildings[{index}].methods.{}.score_share_pm.{good}",
                                method.id
                            ),
                            "unknown good".to_owned(),
                        ));
                    }
                }
            }
        }
        for (name, season) in &self.seasons {
            if season.len() != 12 || season.iter().copied().map(u32::from).sum::<u32>() != 1000 {
                return Err(refuse(
                    format!("seasons.{name}"),
                    "must have 12 monthly shares summing to 1000".to_owned(),
                ));
            }
            if name != "grain" && name != "herd_births" && !goods.contains(name.as_str()) {
                return Err(refuse(
                    format!("seasons.{name}"),
                    "unknown seasonal good".to_owned(),
                ));
            }
        }
        for (name, points) in &self.curves {
            if points.is_empty() || points.windows(2).any(|pair| pair[0][0] >= pair[1][0]) {
                return Err(refuse(
                    format!("curves.{name}"),
                    "x coordinates must be strictly increasing".to_owned(),
                ));
            }
            if points
                .iter()
                .any(|point| !(0..=100_000).contains(&point[1]))
            {
                return Err(refuse(
                    format!("curves.{name}"),
                    "y values must be in 0..=100000".to_owned(),
                ));
            }
        }
        for (good, rates) in &self.ticking_rates_million {
            if !goods.contains(good.as_str()) {
                return Err(refuse(
                    format!("ticking_rates_million.{good}"),
                    "unknown good".to_owned(),
                ));
            }
            for pop_type in rates.keys() {
                if !self.pop_types.iter().any(|item| item.id == *pop_type) {
                    return Err(refuse(
                        format!("ticking_rates_million.{good}.{pop_type}"),
                        "unknown population type".to_owned(),
                    ));
                }
            }
        }
        for (profile, shares) in &self.diet_profiles {
            if shares.values().copied().map(u32::from).sum::<u32>() != 1000 {
                return Err(refuse(
                    format!("diet_profiles.{profile}"),
                    "shares must sum to 1000".to_owned(),
                ));
            }
            for good in shares.keys() {
                if !goods.contains(good.as_str()) {
                    return Err(refuse(
                        format!("diet_profiles.{profile}.{good}"),
                        "unknown good".to_owned(),
                    ));
                }
            }
        }
        for pop_type in &self.pop_types {
            let Some(tiers) = self.comfort_profiles.get(&pop_type.id) else {
                return Err(refuse(
                    format!("comfort_profiles.{}", pop_type.id),
                    "profile is missing".to_owned(),
                ));
            };
            for tier in ["niceties", "luxuries"] {
                if !tiers.contains_key(tier) {
                    return Err(refuse(
                        format!("comfort_profiles.{}.{}", pop_type.id, tier),
                        "tier profile is missing".to_owned(),
                    ));
                }
            }
        }
        for (profile, tiers) in &self.comfort_profiles {
            if !self.pop_types.iter().any(|pop| pop.id == *profile) {
                return Err(refuse(
                    format!("comfort_profiles.{profile}"),
                    "unknown population type".to_owned(),
                ));
            }
            for (tier, shares) in tiers {
                if tier != "niceties" && tier != "luxuries" {
                    return Err(refuse(
                        format!("comfort_profiles.{profile}.{tier}"),
                        "tier must be niceties or luxuries".to_owned(),
                    ));
                }
                if !shares.is_empty()
                    && shares.values().copied().map(u32::from).sum::<u32>() != 1000
                {
                    return Err(refuse(
                        format!("comfort_profiles.{profile}.{tier}"),
                        "shares must sum to 1000".to_owned(),
                    ));
                }
                for (good, share) in shares {
                    let Some(definition) = self.goods.iter().find(|item| item.id == *good) else {
                        return Err(refuse(
                            format!("comfort_profiles.{profile}.{tier}.{good}"),
                            "unknown good".to_owned(),
                        ));
                    };
                    let tier_value = if tier == "niceties" {
                        definition.niceties_centi
                    } else {
                        definition.luxuries_centi
                    };
                    if *share > 1000 || tier_value == 0 {
                        return Err(refuse(
                            format!("comfort_profiles.{profile}.{tier}.{good}"),
                            if tier_value == 0 {
                                "good has zero value in this tier".to_owned()
                            } else {
                                "share must be in 0..=1000".to_owned()
                            },
                        ));
                    }
                }
            }
        }
        for (terrain, values) in &self.suitability {
            if terrain != "sea" && !self.terrain_order.contains(terrain) {
                return Err(refuse(
                    format!("suitability.{terrain}"),
                    "terrain is absent from terrain_order".to_owned(),
                ));
            }
            for (product, value) in values {
                if *value > 1000 {
                    return Err(refuse(
                        format!("suitability.{terrain}.{product}"),
                        "per-mille value must be in 0..=1000".to_owned(),
                    ));
                }
                if product != "crops" && product != "sea" && !goods.contains(product.as_str()) {
                    return Err(refuse(
                        format!("suitability.{terrain}.{product}"),
                        "unknown good or category".to_owned(),
                    ));
                }
            }
        }
        unique_ids(
            file,
            "herd_species",
            self.herd_species.iter().map(|s| s.animal.as_str()),
        )?;
        let mut priorities = std::collections::BTreeSet::new();
        for species in &self.herd_species {
            let entry = format!("herd_species.{}", species.animal);
            if !goods.contains(species.animal.as_str()) {
                return Err(refuse(entry, "unknown animal good".to_owned()));
            }
            if !priorities.insert(species.emergency_order) {
                return Err(refuse(
                    format!("{entry}.emergency_order"),
                    "duplicate emergency order".to_owned(),
                ));
            }
            for yield_item in &species.yields {
                if !goods.contains(yield_item.good.as_str()) {
                    return Err(refuse(
                        format!("{entry}.yields.{}", yield_item.good),
                        "unknown good".to_owned(),
                    ));
                }
            }
            for product in &species.products {
                let product_entry = format!("{entry}.products.{}", product.good);
                if !goods.contains(product.good.as_str()) {
                    return Err(refuse(product_entry, "unknown good".to_owned()));
                }
                if self
                    .goods
                    .iter()
                    .any(|good| good.id == product.good && good.category_supplied("fuel", 1) > 0)
                {
                    return Err(refuse(
                        product_entry,
                        "a recurring herd product cannot supply fuel".to_owned(),
                    ));
                }
                if product.amount_milli.is_none() {
                    return Err(refuse(
                        format!("{product_entry}.amount_milli"),
                        "amount is missing".to_owned(),
                    ));
                }
                match (&product.season, product.month) {
                    (Some(season), None) if self.seasons.contains_key(season) => {}
                    (Some(_), None) => {
                        return Err(refuse(product_entry, "unknown season table".to_owned()));
                    }
                    (None, Some(1..=12)) => {}
                    _ => {
                        return Err(refuse(
                            product_entry,
                            "specify one season table or month in 1..=12".to_owned(),
                        ));
                    }
                }
            }
        }
        for building in &self.buildings {
            for animal in building.herds.keys() {
                let entry = format!("buildings.{}.herds.{animal}", building.id);
                if !self.herd_species.iter().any(|s| s.animal == *animal) {
                    return Err(refuse(entry, "unknown herd species".to_owned()));
                }
            }
        }
        for terrain in &self.terrain_order {
            for species in &self.herd_species {
                if self
                    .grazing
                    .get(terrain)
                    .and_then(|g| g.get(&species.animal))
                    .is_none()
                {
                    return Err(refuse(
                        format!("grazing.{terrain}.{}", species.animal),
                        "value is missing".to_owned(),
                    ));
                }
            }
        }
        for (terrain, values) in &self.grazing {
            if !self.terrain_order.contains(terrain) {
                return Err(refuse(
                    format!("grazing.{terrain}"),
                    "terrain is absent from terrain_order".to_owned(),
                ));
            }
            for (animal, value) in values {
                if *value > 1000 {
                    return Err(refuse(
                        format!("grazing.{terrain}.{animal}"),
                        "per-mille value must be in 0..=1000".to_owned(),
                    ));
                }
                if !self
                    .herd_species
                    .iter()
                    .any(|species| species.animal == *animal)
                {
                    return Err(refuse(
                        format!("grazing.{terrain}.{animal}"),
                        "unknown herd species".to_owned(),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Authored default population, buildings and stock by location kind and size.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StartDefaults {
    /// Starting peasant people; zero is allowed for some monasteries.
    pub peasants: u64,
    /// Starting monk people.
    pub monks: u64,
    /// Starting labourer people.
    pub labourers: u64,
    /// Guessed subsistence farm levels.
    pub farm_levels: u32,
    /// Guessed holdings levels.
    pub holdings_levels: u32,
    /// Guessed storehouse levels.
    pub storehouse_levels: u32,
    /// Starting per-person stock by good.
    #[serde(default)]
    pub pop_stock: BTreeMap<String, u64>,
    /// Starting per-building stock by good.
    #[serde(default)]
    pub building_stock: BTreeMap<String, u64>,
}

/// Sparse authored changes for one location.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StartOverride {
    /// Regional culture for its Irish pops.
    pub culture: Option<String>,
    /// Culture for the monk population.
    pub monk_culture: Option<String>,
    /// Religion for the ordinary population.
    pub religion: Option<String>,
    /// Religion for monks.
    pub monk_religion: Option<String>,
    /// Per-mille peasant population shares by culture; omitted means `culture` for all peasants.
    pub peasant_cultures_pm: Option<BTreeMap<String, u16>>,
    /// Share of peasants belonging to the pagan pocket, in per-mille.
    pub pagan_share_pm: Option<u16>,
    /// Override people counts.
    pub peasants: Option<u64>,
    /// Override monk count.
    pub monks: Option<u64>,
    /// Override labourer count.
    pub labourers: Option<u64>,
    /// Override building levels: farm, holdings, storehouse.
    pub levels: Option<[u32; 3]>,
}

/// The noble house a place starts with: its noble pop and the lord's demesne (CK-359).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NobleStart {
    /// Culture of the noble pop; it need not be the place's own.
    pub culture: String,
    /// Religion of the noble pop.
    pub religion: String,
    /// Noble heads, split into workers and dependents like any pop.
    pub people: u64,
    /// Authored demesne levels, cut to the place's remaining usable land.
    pub demesne_levels: u32,
    /// The paid labourer positions those authored levels carry. Checked against the demesne's
    /// workers per level; the labourers actually added follow the levels left after any cut.
    pub labourers: u64,
}

/// Per-scenario authored economy starting data.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScenarioEconomy {
    /// Defaults indexed by location kind and decimal size.
    pub defaults: BTreeMap<String, BTreeMap<String, StartDefaults>>,
    /// Per-location exceptions.
    #[serde(default)]
    pub places: BTreeMap<String, StartOverride>,
    /// Noble houses by location id.
    #[serde(default)]
    pub nobles: BTreeMap<String, NobleStart>,
    /// Seats guaranteed to keep horses.
    #[serde(default)]
    pub horses_always: Vec<String>,
}

impl ScenarioEconomy {
    /// Load strict authored scenario economy data.
    ///
    /// # Errors
    /// Returns a source-qualified error for unreadable, malformed or invalid data.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DataError> {
        let path = path.as_ref();
        let file = path.display().to_string();
        let text = fs::read_to_string(path).map_err(|error| DataError {
            file: file.clone(),
            entry: "<file>".to_owned(),
            problem: error.to_string(),
        })?;
        let data: Self = toml::from_str(&text).map_err(|error| DataError {
            file: file.clone(),
            entry: "<document>".to_owned(),
            problem: error.message().to_owned(),
        })?;
        data.validate(&file)?;
        Ok(data)
    }

    fn validate(&self, file: &str) -> Result<(), DataError> {
        for (kind, sizes) in &self.defaults {
            for (size, defaults) in sizes {
                if size.parse::<u8>().map_or(true, |size| size == 0) {
                    return Err(DataError {
                        file: file.to_owned(),
                        entry: format!("defaults.{kind}.{size}"),
                        problem: "size must be an integer from 1 to 255".to_owned(),
                    });
                }
                for id in defaults
                    .pop_stock
                    .keys()
                    .chain(defaults.building_stock.keys())
                {
                    if id.is_empty() {
                        return Err(DataError {
                            file: file.to_owned(),
                            entry: format!("defaults.{kind}.{size}"),
                            problem: "stock good key must not be empty".to_owned(),
                        });
                    }
                }
            }
        }
        for id in &self.horses_always {
            if !self.nobles.contains_key(id) {
                return Err(DataError {
                    file: file.to_owned(),
                    entry: format!("horses_always.{id}"),
                    problem: "seat has no noble and demesne".to_owned(),
                });
            }
        }
        for (id, noble) in &self.nobles {
            if id.is_empty() || noble.people == 0 {
                return Err(DataError {
                    file: file.to_owned(),
                    entry: format!("nobles.{id}"),
                    problem: "place id must be non-empty and people positive".to_owned(),
                });
            }
        }
        for (id, place) in &self.places {
            if id.is_empty() || place.pagan_share_pm.is_some_and(|share| share > 1000) {
                return Err(DataError {
                    file: file.to_owned(),
                    entry: format!("places.{id}"),
                    problem: "place id must be non-empty and pagan share at most 1000".to_owned(),
                });
            }
            if let Some(shares) = &place.peasant_cultures_pm {
                let total = shares.values().map(|share| u32::from(*share)).sum::<u32>();
                if shares.is_empty() || shares.values().any(|share| *share == 0) || total != 1000 {
                    return Err(DataError {
                        file: file.to_owned(),
                        entry: format!("places.{id}.peasant_cultures_pm"),
                        problem: "culture shares must be positive and sum to 1000".to_owned(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Check all authored place cultures against the shared registry.
    ///
    /// # Errors
    /// Returns the source and entry of the first unknown culture.
    pub fn validate_cultures(&self, peoples: &Peoples, file: &str) -> Result<(), DataError> {
        for (id, place) in &self.places {
            for (field, key) in [
                ("culture", place.culture.as_deref()),
                ("monk_culture", place.monk_culture.as_deref()),
            ] {
                if let Some(key) = key
                    && peoples.culture(key).is_none()
                {
                    return Err(DataError {
                        file: file.into(),
                        entry: format!("places.{id}.{field}"),
                        problem: format!("unknown culture `{key}`"),
                    });
                }
            }
            for key in place
                .peasant_cultures_pm
                .iter()
                .flat_map(|shares| shares.keys())
            {
                if peoples.culture(key).is_none() {
                    return Err(DataError {
                        file: file.into(),
                        entry: format!("places.{id}.peasant_cultures_pm.{key}"),
                        problem: "unknown culture".into(),
                    });
                }
            }
        }
        for (id, noble) in &self.nobles {
            if peoples.culture(&noble.culture).is_none() {
                return Err(DataError {
                    file: file.into(),
                    entry: format!("nobles.{id}.culture"),
                    problem: format!("unknown culture `{}`", noble.culture),
                });
            }
        }
        Ok(())
    }

    /// Resolve the authored default and a sparse place override.
    ///
    /// # Errors
    /// Returns an error when the requested location kind and size have no authored default.
    pub fn resolve(
        &self,
        kind: &str,
        size: u8,
        id: &str,
    ) -> Result<(StartDefaults, StartOverride), DataError> {
        let file = "scenario economy data".to_owned();
        let defaults = self
            .defaults
            .get(kind)
            .and_then(|sizes| sizes.get(&size.to_string()))
            .cloned()
            .ok_or_else(|| DataError {
                file: file.clone(),
                entry: format!("defaults.{kind}.{size}"),
                problem: "no starting template for this location kind and size".to_owned(),
            })?;
        Ok((defaults, self.places.get(id).cloned().unwrap_or_default()))
    }
}

fn unique_ids<'a>(
    file: &str,
    entry: &str,
    ids: impl Iterator<Item = &'a str>,
) -> Result<(), DataError> {
    let mut found = std::collections::BTreeSet::new();
    for id in ids {
        if id.is_empty() || !found.insert(id) {
            return Err(DataError {
                file: file.to_owned(),
                entry: entry.to_owned(),
                problem: format!("empty or duplicate id `{id}`"),
            });
        }
    }
    Ok(())
}

/// Compute the integer target price from value-denominated buy and sell orders.
///
/// Prices are thousandths of a value unit. `k_pm` is explicit so callers cannot
/// accidentally test only the default response.
///
/// # Errors
/// Returns an error if values exceed the supported ranges or checked integer arithmetic overflows.
pub fn target_price(
    base: u64,
    buy_value: u128,
    sell_value: u128,
    k_pm: u32,
) -> Result<u64, &'static str> {
    if !(1..=1_000_000).contains(&base) || k_pm > 10_000 {
        return Err("base price or k is out of range");
    }
    let base_m = i128::from(base).checked_mul(1000).ok_or("base overflow")?;
    let buy = i128::try_from(buy_value).map_err(|_| "buy value exceeds i128")?;
    let sell = i128::try_from(sell_value).map_err(|_| "sell value exceeds i128")?;
    if buy.checked_add(sell).ok_or("order sum overflow")? == 0 {
        return u64::try_from(base_m).map_err(|_| "price overflow");
    }
    let difference = buy.checked_sub(sell).ok_or("order difference overflow")?;
    let ratio = difference
        .checked_mul(1_000_000)
        .ok_or("ratio overflow")?
        .div_euclid(buy + sell);
    let factor = 1_000_000_000_i128
        .checked_add(
            i128::from(k_pm)
                .checked_mul(ratio)
                .ok_or("response overflow")?,
        )
        .ok_or("response overflow")?;
    let raw = base_m
        .checked_mul(factor)
        .ok_or("target overflow")?
        .div_euclid(1_000_000_000);
    let lower = (base_m * 200).div_euclid(1000).max(1);
    let upper = (base_m * 5000).div_euclid(1000);
    u64::try_from(raw.clamp(lower, upper)).map_err(|_| "target price is outside u64")
}

/// Value whole good units in the local price representation (milli-value units).
///
/// # Errors
/// Returns an error if the checked value multiplication overflows.
pub fn value_at_price(units: u128, price_milli: u64) -> Result<u128, &'static str> {
    units
        .checked_mul(u128::from(price_milli))
        .ok_or("local price value overflow")
}

/// Split a weighted total into integer shares by largest remainder in input order.
///
/// # Errors
/// Returns an error if intermediate checked arithmetic or the resulting shares overflow.
pub fn largest_remainder(total: u64, weights: &[u64]) -> Result<Vec<u64>, &'static str> {
    let sum = weights.iter().try_fold(0_u128, |sum, weight| {
        sum.checked_add(u128::from(*weight))
            .ok_or("weight sum overflow")
    })?;
    if sum == 0 {
        return Ok(vec![0; weights.len()]);
    }
    let mut result = Vec::with_capacity(weights.len());
    let mut remainders = Vec::with_capacity(weights.len());
    let mut assigned = 0_u64;
    for (index, weight) in weights.iter().enumerate() {
        let product = u128::from(total)
            .checked_mul(u128::from(*weight))
            .ok_or("share product overflow")?;
        let quotient = u64::try_from(product / sum).map_err(|_| "share exceeds u64")?;
        assigned = assigned.checked_add(quotient).ok_or("share sum overflow")?;
        result.push(quotient);
        remainders.push((product % sum, index));
    }
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, index) in remainders
        .into_iter()
        .take(usize::try_from(total - assigned).map_err(|_| "remainder too large")?)
    {
        result[index] += 1;
    }
    Ok(result)
}

// Split a receipt by weight, repeatedly redistributing shares that do not fit.
fn split_with_caps(total: u64, weights: &[u64], caps: &[u64]) -> Result<Vec<u64>, &'static str> {
    if weights.len() != caps.len() {
        return Err("weight and capacity dimensions differ");
    }
    let mut result = vec![0_u64; weights.len()];
    let mut remaining = total;
    while remaining > 0 {
        let active: Vec<u64> = weights
            .iter()
            .zip(caps)
            .zip(&result)
            .map(|((&weight, &cap), &used)| if used < cap { weight } else { 0 })
            .collect();
        if active.iter().all(|weight| *weight == 0) {
            break;
        }
        let shares = largest_remainder(remaining, &active)?;
        let mut accepted = 0_u64;
        for (index, share) in shares.into_iter().enumerate() {
            let take = share.min(caps[index] - result[index]);
            result[index] += take;
            accepted += take;
        }
        if accepted == 0 {
            break;
        }
        remaining -= accepted;
    }
    Ok(result)
}

/// Plain, derived cell counts supplied by the map adapter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LandCounts {
    /// All owned land cells by terrain key, excluding glaciers.
    pub land_cells: BTreeMap<String, u32>,
    /// Sum of fertility per-mille by terrain key, excluding glaciers.
    pub fertility_pm: BTreeMap<String, u64>,
    /// Owned land cells whose fertility is above zero, excluding glaciers.
    pub fertile_cells: u32,
    /// Owned land cells whose fertility is above zero, by terrain.
    pub fertile_cells_by_terrain: BTreeMap<String, u32>,
    /// Sea cells in the catchment.
    pub sea: u32,
}

/// One map location plus its already-derived usable and sea cell counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartLocation {
    /// Scenario-local location id.
    pub id: String,
    /// Economy-facing location kind key.
    pub kind: String,
    /// Authored location size.
    pub size: u8,
    /// Derived land counts; never copied into the saved state.
    pub land: LandCounts,
}

/// Derive makeshift shelter from a location's owned land catchment.
///
/// # Errors
/// Returns a named data error when a terrain rate is missing or checked arithmetic overflows.
pub fn natural_shelter(rules: &Rules, land: &LandCounts) -> Result<u64, DataError> {
    let weighted = land
        .land_cells
        .iter()
        .try_fold(0_u128, |sum, (terrain, cells)| {
            let rate = rules.shelter_pm.get(terrain).ok_or_else(|| {
                missing_data(format!("shelter_pm.{terrain}"), "terrain rate is missing")
            })?;
            let part = u128::from(*cells)
                .checked_mul(u128::from(*rate))
                .ok_or_else(|| {
                    missing_data("housing.shelter".to_owned(), "shelter numerator overflow")
                })?;
            sum.checked_add(part).ok_or_else(|| {
                missing_data("housing.shelter".to_owned(), "shelter numerator overflow")
            })
        })?;
    u64::try_from(weighted / 1000)
        .map_err(|_| missing_data("housing.shelter".to_owned(), "shelter exceeds u64"))
}

/// Build the deterministic opening economy from authored scenario data.
///
/// The report names every resource cut and any authored level for which no
/// method had room. Employment is filled directly and has no monthly rate cap. `start_month` is also
/// the first payday's month; callers with a calendar date use [`starting_state_at`].
///
/// # Errors
/// Returns a source-qualified error for invalid rules, missing templates or checked arithmetic overflow.
pub fn starting_state(
    rules: &Rules,
    data: &ScenarioEconomy,
    locations: &[StartLocation],
    start_month: u8,
) -> Result<(EconomyState, Vec<String>), DataError> {
    starting_state_with_payday(rules, data, locations, start_month, start_month, 0)
}

/// Builds the starting economy using the scenario's exact date for opening unpaid work.
///
/// # Errors
/// Returns a source-qualified error for an invalid date, invalid rules or failed initialization.
pub fn starting_state_at(
    rules: &Rules,
    data: &ScenarioEconomy,
    locations: &[StartLocation],
    start: Date,
) -> Result<(EconomyState, Vec<String>), DataError> {
    starting_state_at_seed(rules, data, locations, start, 0)
}

/// Build an opening using the game seed.
///
/// # Errors
/// Returns a data error for invalid input or initialization failure.
pub fn starting_state_at_seed(
    rules: &Rules,
    data: &ScenarioEconomy,
    locations: &[StartLocation],
    start: Date,
    seed: u64,
) -> Result<(EconomyState, Vec<String>), DataError> {
    ck_sim::GameTime::from_date(start)
        .map_err(|error| missing_data("scenario start".into(), &error.to_string()))?;
    let first_payday = if start.day == 1 && start.hour <= 12 {
        start.month
    } else {
        start.month % 12 + 1
    };
    starting_state_with_payday(rules, data, locations, start.month, first_payday, seed)
}

#[expect(
    clippy::too_many_lines,
    reason = "The startup sequence preserves its placement and employment ordering."
)]
fn starting_state_with_payday(
    rules: &Rules,
    data: &ScenarioEconomy,
    locations: &[StartLocation],
    start_month: u8,
    first_payday: u8,
    seed: u64,
) -> Result<(EconomyState, Vec<String>), DataError> {
    if !(1..=12).contains(&start_month) {
        return Err(DataError {
            file: "scenario start".to_owned(),
            entry: "start_month".to_owned(),
            problem: "must be in 1..=12".to_owned(),
        });
    }
    rules.validate("economy rules")?;
    data.validate("scenario economy data")?;
    for id in &data.horses_always {
        if !locations.iter().any(|location| &location.id == id) {
            return Err(missing_data(
                format!("horses_always.{id}"),
                "seat is absent",
            ));
        }
    }
    let months_to_harvest = months_to_grain_harvest(rules, start_month)?;
    let mut state = EconomyState::default();
    let mut report = Vec::new();
    for location in locations {
        let (defaults, override_) = data.resolve(&location.kind, location.size, &location.id)?;
        if override_.pagan_share_pm.unwrap_or(0) > 1000 {
            return Err(DataError {
                file: "scenario economy data".to_owned(),
                entry: format!("places.{}.pagan_share_pm", location.id),
                problem: "must be at most 1000".to_owned(),
            });
        }
        let peasants = override_.peasants.unwrap_or(defaults.peasants);
        let monks = override_.monks.unwrap_or(defaults.monks);
        let labourers = override_.labourers.unwrap_or(defaults.labourers);
        let culture = override_.culture.as_deref().ok_or_else(|| DataError {
            file: "scenario economy data".to_owned(),
            entry: format!("places.{}.culture", location.id),
            problem: "must be authored for each place".to_owned(),
        })?;
        let religion = override_.religion.as_deref().ok_or_else(|| DataError {
            file: "scenario economy data".to_owned(),
            entry: format!("places.{}.religion", location.id),
            problem: "must be authored for each place".to_owned(),
        })?;
        let monk_culture = override_.monk_culture.as_deref().unwrap_or(culture);
        let monk_religion = override_
            .monk_religion
            .as_deref()
            .ok_or_else(|| DataError {
                file: "scenario economy data".to_owned(),
                entry: format!("places.{}.monk_religion", location.id),
                problem: "must be authored for each place".to_owned(),
            })?;
        for (field, value, known) in [
            ("culture", culture, rules.peoples.culture(culture).is_some()),
            (
                "religion",
                religion,
                rules.religions.iter().any(|key| key == religion),
            ),
            (
                "monk_culture",
                monk_culture,
                rules.peoples.culture(monk_culture).is_some(),
            ),
            (
                "monk_religion",
                monk_religion,
                rules.religions.iter().any(|key| key == monk_religion),
            ),
        ] {
            if !known {
                return Err(DataError {
                    file: "scenario economy data".to_owned(),
                    entry: format!("places.{}.{}", location.id, field),
                    problem: format!("unknown key `{value}`"),
                });
            }
        }
        if override_.pagan_share_pm.unwrap_or(0) > 0
            && !rules.religions.iter().any(|key| key == "irish_paganism")
        {
            return Err(DataError {
                file: "economy rules".to_owned(),
                entry: "religions".to_owned(),
                problem: "irish_paganism is required by the scenario's pagan start".to_owned(),
            });
        }
        for good in defaults
            .pop_stock
            .keys()
            .chain(defaults.building_stock.keys())
        {
            if !rules.goods.iter().any(|item| &item.id == good) {
                return Err(DataError {
                    file: "scenario economy data".to_owned(),
                    entry: format!("defaults.{}.{}.stock.{good}", location.kind, location.size),
                    problem: "unknown good".to_owned(),
                });
            }
        }
        let mut pops = Vec::new();
        let pagan = u64::from(override_.pagan_share_pm.unwrap_or(0));
        let culture_shares = override_
            .peasant_cultures_pm
            .clone()
            .unwrap_or_else(|| BTreeMap::from([(culture.to_owned(), 1000)]));
        let culture_weights = culture_shares
            .values()
            .map(|share| u64::from(*share))
            .collect::<Vec<_>>();
        for (peasant_culture, peasant_count) in
            culture_shares
                .keys()
                .zip(
                    largest_remainder(peasants, &culture_weights).map_err(|problem| DataError {
                        file: "scenario economy data".to_owned(),
                        entry: format!("places.{}.peasant_cultures_pm", location.id),
                        problem: problem.to_owned(),
                    })?,
                )
        {
            if rules.peoples.culture(peasant_culture).is_none() {
                return Err(DataError {
                    file: "scenario economy data".to_owned(),
                    entry: format!(
                        "places.{}.peasant_cultures_pm.{peasant_culture}",
                        location.id
                    ),
                    problem: "unknown culture".to_owned(),
                });
            }
            let religion_parts =
                largest_remainder(peasant_count, &[1000 - pagan, pagan]).map_err(|problem| {
                    DataError {
                        file: "scenario economy data".to_owned(),
                        entry: format!("places.{}.pagan_share_pm", location.id),
                        problem: problem.to_owned(),
                    }
                })?;
            if religion_parts[0] > 0 {
                pops.push(make_pop(
                    rules,
                    "peasant",
                    peasant_culture,
                    religion,
                    religion_parts[0],
                    &defaults.pop_stock,
                )?);
            }
            if religion_parts[1] > 0 {
                pops.push(make_pop(
                    rules,
                    "peasant",
                    peasant_culture,
                    "irish_paganism",
                    religion_parts[1],
                    &defaults.pop_stock,
                )?);
            }
        }
        if monks > 0 {
            pops.push(make_pop(
                rules,
                "monk",
                monk_culture,
                monk_religion,
                monks,
                &defaults.pop_stock,
            )?);
        }
        if labourers > 0 {
            pops.push(make_pop(
                rules,
                "labourer",
                culture,
                religion,
                labourers,
                &defaults.pop_stock,
            )?);
        }

        let authored = override_.levels.unwrap_or([
            defaults.farm_levels,
            defaults.holdings_levels,
            defaults.storehouse_levels,
        ]);
        let noble = data.nobles.get(&location.id);
        if let Some(noble) = noble {
            let entry = format!("nobles.{}", location.id);
            for (field, value, known) in [
                (
                    "culture",
                    noble.culture.as_str(),
                    rules.peoples.culture(&noble.culture).is_some(),
                ),
                (
                    "religion",
                    noble.religion.as_str(),
                    rules.religions.contains(&noble.religion),
                ),
            ] {
                if !known {
                    return Err(DataError {
                        file: "scenario economy data".to_owned(),
                        entry: format!("{entry}.{field}"),
                        problem: format!("unknown key `{value}`"),
                    });
                }
            }
            let demesne = rules
                .buildings
                .iter()
                .find(|item| item.id == "demesne")
                .ok_or_else(|| {
                    missing_data("buildings.demesne".to_owned(), "definition is missing")
                })?;
            if noble.labourers
                != u64::from(noble.demesne_levels) * u64::from(demesne.workers_per_level)
            {
                return Err(DataError {
                    file: "scenario economy data".to_owned(),
                    entry: format!("{entry}.labourers"),
                    problem: "must be the demesne's workers per level times its levels".to_owned(),
                });
            }
        }
        if data.horses_always.contains(&location.id) && noble.is_none_or(|n| n.demesne_levels == 0)
        {
            return Err(missing_data(
                format!("horses_always.{}", location.id),
                "seat has no demesne",
            ));
        }
        let mut land = location.land.clone();
        let mut buildings = Vec::new();
        let mut remaining_sea = land.sea;
        let mut production = BTreeMap::<String, u64>::new();
        // The needs and orders are those of the pops counted now: a noble seat's nobles and its
        // labourer household are added only after every level is placed.
        let mut demand = StartDemand::new(rules, &pops)?;
        // Holdings take fertility before the lord's demesne, which takes it before the farm (the
        // lord's land before the free farmers). Fishing needs nearby sea but does not consume a
        // finite sea-cell budget.
        for (kind, wanted) in [
            ("holdings", authored[1]),
            ("demesne", noble.map_or(0, |noble| noble.demesne_levels)),
            ("farm", authored[0]),
        ] {
            if wanted == 0 {
                continue;
            }
            let building = rules
                .buildings
                .iter()
                .find(|item| item.id == kind)
                .ok_or_else(|| {
                    missing_data(format!("buildings.{kind}"), "definition is missing")
                })?;
            let assignment = assign_building_levels(
                rules,
                building,
                wanted,
                &mut land,
                &mut remaining_sea,
                &mut demand,
                mean_fertility(&location.land)?,
            )?;
            let actual = assignment.method_levels.iter().sum();
            if actual < wanted {
                let reason = if assignment.cut_for_room == Some(false) {
                    "a method has room, but none serves food or an unmet need"
                } else {
                    "no method has enough land or sea"
                };
                report.push(format!(
                    "{} {kind}: cut levels {wanted} -> {actual} ({reason})",
                    location.id
                ));
            }
            for (good, quantity) in assignment.output {
                let current = production.entry(good.clone()).or_default();
                *current = (*current).checked_add(quantity).ok_or_else(|| DataError {
                    file: "projection".to_owned(),
                    entry: good,
                    problem: "annual supply overflow".to_owned(),
                })?;
            }
            let herds = opening_herds(
                rules,
                &assignment.method_levels,
                building,
                &assignment.herd_grazing_sums,
                seed,
                &location.id,
                noble.map_or(0, |n| n.people),
                data.horses_always.contains(&location.id),
            )?;
            if kind == "demesne"
                && data.horses_always.contains(&location.id)
                && rules
                    .herd_species
                    .iter()
                    .position(|s| s.animal == "horses")
                    .is_none_or(|i| herds[i] == 0)
            {
                return Err(missing_data(
                    format!("horses_always.{}", location.id),
                    "demesne has no room for horses",
                ));
            }
            let herd_carry = vec![0; herds.len()];
            buildings.push(BuildingState {
                kind: kind.to_owned(),
                levels: actual,
                method_levels: assignment.method_levels,
                level_records: assignment.level_records,
                herds,
                employment: vec![Employment::default(); pops.len()],
                stock: stock_vector(rules, &defaults.building_stock),
                storehouse_accounts: None,
                production_carries: vec![vec![0; rules.goods.len()]; building.methods.len()],
                method_suitability: assignment.method_suitability,
                herd_grazing_sums: assignment.herd_grazing_sums,
                herd_birth_carries: herd_carry.clone(),
                herd_death_carries: herd_carry,
                herd_product_carries: vec![
                    0;
                    rules.herd_species.iter().map(|s| s.products.len()).sum()
                ],
                wage_carries: vec![0; rules.goods.len()],
                pay_carries: vec![0; rules.goods.len()],
                fee_carries: Vec::new(),
                wage_history: vec![0; 12],
                paid_history: vec![0; WAGE_HISTORY_MONTHS],
                paid_years: vec![0; WAGE_HISTORY_MONTHS],
            });
            let _ = (assignment.used_land, assignment.used_sea);
        }
        if let Some(noble) = noble {
            // The new pops are added after every level is assigned, so the estimates that chose
            // the farm's and holdings' methods are the ones they always saw.
            pops.push(make_pop(
                rules,
                "noble",
                &noble.culture,
                &noble.religion,
                noble.people,
                &defaults.pop_stock,
            )?);
            let demesne_levels = buildings
                .iter()
                .find(|building| building.kind == "demesne")
                .map_or(0, |building| building.levels);
            if demesne_levels > 0 {
                let workers_per_level = rules
                    .buildings
                    .iter()
                    .find(|item| item.id == "demesne")
                    .map_or(0, |item| u64::from(item.workers_per_level));
                let household =
                    labourer_household_heads(rules, u64::from(demesne_levels) * workers_per_level)?;
                let household = make_pop(
                    rules,
                    "labourer",
                    culture,
                    religion,
                    household,
                    &defaults.pop_stock,
                )?;
                merge_into_cohort(&mut pops, household)?;
            }
        }
        for building in &mut buildings {
            building
                .employment
                .resize(pops.len(), Employment::default());
        }
        // Every location has a local storehouse. Its levels go to the largest
        // unserved annual goods volume, with grain/cellar/drink tie order.
        let store_levels = authored[2];
        if store_levels > 0 {
            let kinds = storehouse_kinds(rules, &production, store_levels);
            let accounts = empty_storehouse_accounts(rules, pops.len());
            buildings.push(BuildingState {
                kind: "storehouse".to_owned(),
                levels: store_levels,
                method_levels: kinds,
                level_records: Vec::new(),
                herds: Vec::new(),
                employment: vec![Employment::default(); pops.len()],
                stock: storehouse_physical_stock(rules, &accounts),
                storehouse_accounts: Some(accounts),
                production_carries: Vec::new(),
                method_suitability: Vec::new(),
                herd_birth_carries: Vec::new(),
                herd_death_carries: Vec::new(),
                herd_product_carries: Vec::new(),
                wage_carries: vec![0; rules.goods.len()],
                pay_carries: vec![0; rules.goods.len()],
                fee_carries: vec![vec![0; rules.goods.len()]; pops.len() + 1],
                wage_history: vec![0; 12],
                paid_history: vec![0; WAGE_HISTORY_MONTHS],
                paid_years: vec![0; WAGE_HISTORY_MONTHS],
                herd_grazing_sums: Vec::new(),
            });
        }
        fill_starting_employment(rules, &mut pops, &mut buildings);
        seed_unpaid_work(
            rules,
            &pops,
            &mut buildings,
            mean_fertility(&location.land)?,
            first_payday,
        )?;
        fuel::seed_starting_fuel(rules, &mut pops, &buildings)?;
        let prices_milli = rules
            .goods
            .iter()
            .map(|good| {
                good.base_price.checked_mul(1000).ok_or_else(|| DataError {
                    file: "economy rules".to_owned(),
                    entry: format!("goods.{}.base_price", good.id),
                    problem: "base price in thousandths overflows u64".to_owned(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        for building in &mut buildings {
            if building.storehouse_accounts.is_some() {
                building.wage_history = vec![0; WAGE_HISTORY_MONTHS];
                building.paid_history = vec![0; WAGE_HISTORY_MONTHS];
                building.paid_years = vec![0; WAGE_HISTORY_MONTHS];
                continue;
            }
            let paid_heads = building
                .employment
                .iter()
                .try_fold(0_u64, |total, employment| {
                    total.checked_add(employment.workers)
                })
                .ok_or_else(|| {
                    missing_data(
                        format!(
                            "places.{}.buildings.{}.employment",
                            location.id, building.kind
                        ),
                        "paid head count overflow",
                    )
                })?;
            let stock_value = building
                .stock
                .iter()
                .zip(&prices_milli)
                .try_fold(0_u128, |total, (units, price)| {
                    value_at_price(u128::from(*units), *price)
                        .ok()
                        .and_then(|value| total.checked_add(value))
                })
                .ok_or_else(|| {
                    missing_data(
                        format!(
                            "places.{}.buildings.{}.wage_history",
                            location.id, building.kind
                        ),
                        "starting stockpile value overflow",
                    )
                })?;
            let seed = if paid_heads == 0 {
                0
            } else {
                let denominator = u128::from(paid_heads)
                    .checked_mul(u128::from(months_to_harvest))
                    .ok_or_else(|| {
                        missing_data(
                            format!(
                                "places.{}.buildings.{}.wage_history",
                                location.id, building.kind
                            ),
                            "starting wage denominator overflow",
                        )
                    })?;
                u64::try_from(stock_value / denominator).map_err(|_| {
                    missing_data(
                        format!(
                            "places.{}.buildings.{}.wage_history",
                            location.id, building.kind
                        ),
                        "starting posted wage exceeds u64",
                    )
                })?
            };
            building.wage_history = vec![seed; WAGE_HISTORY_MONTHS];
            building.paid_history = vec![0; WAGE_HISTORY_MONTHS];
            building.paid_years = vec![0; WAGE_HISTORY_MONTHS];
        }
        let footprint = building_footprint(rules, &buildings)?;
        let prepared_m2 = footprint
            .checked_add(rules.buildable_base_m2)
            .ok_or_else(|| {
                missing_data(
                    format!("places.{}.prepared_m2", location.id),
                    "prepared area overflow",
                )
            })?;
        let max_buildable = max_buildable_m2(rules, rules.starting_development_pm, &location.land)?;
        if footprint > prepared_m2.min(max_buildable) {
            report.push(format!(
                "{} starts over building cap: footprint {footprint} m², prepared {prepared_m2} m², max_buildable {max_buildable} m²",
                location.id
            ));
        }
        state.places.push(PlaceState {
            id: location.id.clone(),
            pops,
            buildings,
            prices_milli,
            prepared_m2,
            development_pm: rules.starting_development_pm,
            price_drift_carries: vec![0; rules.goods.len()],
            last_market_orders: None,
            counters: CounterBuckets::for_place(rules.goods.len()),
        });
    }
    Ok((state, report))
}

/// The smallest household whose workers, by the labourer type's dependent share, number at least
/// `workers`: the constructor's `h - floor(h * dependent_pm / 1000)` reaches the target.
fn labourer_household_heads(rules: &Rules, workers: u64) -> Result<u64, DataError> {
    let pop_type = rules
        .pop_types
        .iter()
        .find(|item| item.id == "labourer")
        .ok_or_else(|| missing_data("pop_types.labourer".to_owned(), "definition is missing"))?;
    if workers > 0 && pop_type.dependent_pm >= 1000 {
        return Err(missing_data(
            "pop_types.labourer".to_owned(),
            "a household with that dependent share has no workers",
        ));
    }
    let mut heads = workers;
    while heads - heads * u64::from(pop_type.dependent_pm) / 1000 < workers {
        heads += 1;
    }
    Ok(heads)
}

/// Add `household` to the place's pop of the same type, culture and religion, or as a new pop
/// when there is none (what `ensure_pop` does for movers).
fn merge_into_cohort(pops: &mut Vec<PopState>, household: PopState) -> Result<(), DataError> {
    let Some(cohort) = pops.iter_mut().find(|pop| {
        pop.kind == household.kind
            && pop.culture == household.culture
            && pop.religion == household.religion
    }) else {
        pops.push(household);
        return Ok(());
    };
    let before = (cohort.workers, cohort.dependents);
    cohort.workers = cohort
        .workers
        .checked_add(household.workers)
        .ok_or_else(|| missing_data("start.household".to_owned(), "worker count overflow"))?;
    cohort.dependents = cohort
        .dependents
        .checked_add(household.dependents)
        .ok_or_else(|| missing_data("start.household".to_owned(), "dependent count overflow"))?;
    for (stock, added) in cohort.stock.iter_mut().zip(&household.stock) {
        *stock = stock
            .checked_add(*added)
            .ok_or_else(|| missing_data("start.household".to_owned(), "stock overflow"))?;
    }
    // The merged heads join the cohort, so its genetic state is the people-weighted average (a household of the same
    // culture carries the same seeded state, which this leaves unchanged). It runs after the count checks above, so
    // their named refusals come first.
    let heads = |workers: u64, dependents: u64| {
        workers
            .checked_add(dependents)
            .ok_or_else(|| missing_data("start.household".to_owned(), "head count overflow"))
    };
    let (own, added) = (
        heads(before.0, before.1)?,
        heads(household.workers, household.dependents)?,
    );
    cohort
        .genetics
        .blend(own, &household.genetics, added)
        .map_err(|error| missing_data("start.household".to_owned(), &error.to_string()))?;
    Ok(())
}

fn months_to_grain_harvest(rules: &Rules, start_month: u8) -> Result<u8, DataError> {
    if !(1..=12).contains(&start_month) {
        return Err(missing_data(
            "scenario start_month".to_owned(),
            "month is outside 1..=12",
        ));
    }
    let grain = rules
        .seasons
        .get("grain")
        .ok_or_else(|| missing_data("seasons.grain".to_owned(), "season table is missing"))?;
    (1_u8..=12)
        .find(|months| {
            let month_index = (start_month - 1 + *months) % 12;
            grain[usize::from(month_index)] > 0
        })
        .ok_or_else(|| {
            missing_data(
                "seasons.grain".to_owned(),
                "no harvest month has a positive share",
            )
        })
}

fn missing_data(entry: String, problem: &str) -> DataError {
    DataError {
        file: "economy rules".to_owned(),
        entry,
        problem: problem.to_owned(),
    }
}

/// Mean fertility of the land cells that can produce farm room, floored in ‰.
///
/// # Errors
/// Refuses when the fertility sum exceeds `u64` or the mean exceeds `u16`.
pub fn mean_fertility(land: &LandCounts) -> Result<u16, DataError> {
    if land.fertile_cells == 0 {
        return Ok(0);
    }
    let total = land
        .fertility_pm
        .values()
        .try_fold(0_u64, |sum, value| sum.checked_add(*value))
        .ok_or_else(|| missing_data("map fertility".to_owned(), "fertility sum overflow"))?;
    u16::try_from(total / u64::from(land.fertile_cells))
        .map_err(|_| missing_data("map fertility".to_owned(), "mean fertility exceeds u16"))
}

/// Derive maximum buildable area from the current map and development.
///
/// # Errors
/// Refuses missing terrain data or checked area overflow.
pub fn max_buildable_m2(
    rules: &Rules,
    development_pm: u32,
    land: &LandCounts,
) -> Result<u64, DataError> {
    let terrain_area = land
        .land_cells
        .iter()
        .try_fold(0_u128, |sum, (terrain, cells)| {
            let per_cell = rules.buildable_per_cell_m2.get(terrain).ok_or_else(|| {
                missing_data(
                    format!("buildable_per_cell_m2.{terrain}"),
                    "entry is missing",
                )
            })?;
            sum.checked_add(u128::from(*cells) * u128::from(*per_cell))
                .ok_or_else(|| missing_data("max_buildable".to_owned(), "land area overflow"))
        })?;
    let base = u128::from(rules.buildable_base_m2)
        .checked_add(terrain_area)
        .ok_or_else(|| missing_data("max_buildable".to_owned(), "base area overflow"))?;
    let area = base
        .checked_mul(u128::from(development_pm))
        .ok_or_else(|| missing_data("max_buildable".to_owned(), "development scaling overflow"))?
        / 1000;
    u64::try_from(area).map_err(|_| missing_data("max_buildable".to_owned(), "area exceeds u64"))
}

fn building_footprint(rules: &Rules, buildings: &[BuildingState]) -> Result<u64, DataError> {
    buildings.iter().try_fold(0_u64, |sum, building| {
        let per_level = rules
            .footprint_m2
            .get(&building.kind)
            .copied()
            .ok_or_else(|| {
                missing_data(
                    format!("footprint_m2.{}", building.kind),
                    "entry is missing",
                )
            })?;
        let area = u64::try_from(per_level)
            .ok()
            .and_then(|value| value.checked_mul(u64::from(building.levels)))
            .ok_or_else(|| {
                missing_data(
                    format!("footprint_m2.{}", building.kind),
                    "footprint overflow or negative value",
                )
            })?;
        sum.checked_add(area)
            .ok_or_else(|| missing_data("footprint".to_owned(), "footprint sum overflow"))
    })
}

/// True when adding `extra_footprint_m2` leaves the place within both land limits.
///
/// # Errors
/// Refuses footprint overflow or missing map/footprint data.
pub fn can_build(
    rules: &Rules,
    place: &PlaceState,
    land: &LandCounts,
    extra_footprint_m2: u64,
) -> Result<bool, DataError> {
    let footprint = building_footprint(rules, &place.buildings)?;
    let proposed = footprint.checked_add(extra_footprint_m2).ok_or_else(|| {
        missing_data(
            format!("places.{}.footprint", place.id),
            "proposed footprint overflow",
        )
    })?;
    let cap = place
        .prepared_m2
        .min(max_buildable_m2(rules, place.development_pm, land)?);
    Ok(proposed <= cap)
}

fn seed_genetics(rules: &Rules, culture: &str) -> Result<PopGenetics, DataError> {
    if rules.genetics.is_empty() {
        // Rules built without genetic data (unit fixtures) leave the state unseeded.
        return Ok(PopGenetics::default());
    }
    rules.genetics.seed_pop(culture).ok_or_else(|| {
        missing_data(
            format!("culture_start.{culture}"),
            "culture has no genetic start set",
        )
    })
}

fn make_pop(
    rules: &Rules,
    kind: &str,
    culture: &str,
    religion: &str,
    people: u64,
    stock: &BTreeMap<String, u64>,
) -> Result<PopState, DataError> {
    let pop_type = rules
        .pop_types
        .iter()
        .find(|item| item.id == kind)
        .ok_or_else(|| missing_data(format!("pop_types.{kind}"), "definition is missing"))?;
    let dependents = u64::from(pop_type.dependent_pm)
        .checked_mul(people)
        .ok_or_else(|| missing_data(format!("pop_types.{kind}"), "starting population overflow"))?
        / 1000;
    let workers = people - dependents;
    let stock = rules
        .goods
        .iter()
        .map(|good| {
            stock
                .get(&good.id)
                .copied()
                .unwrap_or(0)
                .checked_mul(people)
                .ok_or_else(|| DataError {
                    file: "scenario economy data".to_owned(),
                    entry: format!("pop_stock.{}", good.id),
                    problem: "starting stock overflow".to_owned(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let starting_needs = rules
        .starting_needs_pm
        .get(kind)
        .ok_or_else(|| missing_data(format!("starting_needs_pm.{kind}"), "value is missing"))?;
    Ok(PopState {
        kind: kind.to_owned(),
        culture: culture.to_owned(),
        religion: religion.to_owned(),
        literacy_pm: 0,
        workers,
        dependents,
        employment: Vec::new(),
        stock,
        consumption: PopConsumption {
            pantry_hunger_centi: vec![0; rules.goods.len()],
            ticking_carries: vec![0; rules.goods.len()],
            necessities_pm: starting_needs[0],
            niceties_pm: starting_needs[1],
            luxuries_pm: starting_needs[2],
            ..PopConsumption::default()
        },
        counters: CounterBuckets::default(),
        short_on_rent: false,
        housed_last: false,
        genetics: seed_genetics(rules, culture)?,
        pending_births: 0,
        pending_deaths: 0,
    })
}

fn stock_vector(rules: &Rules, stock: &BTreeMap<String, u64>) -> Vec<u64> {
    rules
        .goods
        .iter()
        .map(|good| stock.get(&good.id).copied().unwrap_or(0))
        .collect()
}

struct BuildingAssignment {
    method_levels: Vec<u32>,
    level_records: Vec<LevelRecord>,
    method_suitability: Vec<Vec<u64>>,
    herd_grazing_sums: Vec<u64>,
    used_land: u32,
    used_sea: u32,
    output: BTreeMap<String, u64>,
    /// Why the building's levels were cut, when they were: no method had room (`Some(true)`), or a method had room and none
    /// served food or an unmet need (`Some(false)`).
    cut_for_room: Option<bool>,
}

/// The methods of `building` that have room for one more level, each with what that level makes.
fn level_candidates(
    rules: &Rules,
    building: &BuildingType,
    land: &LandCounts,
    remaining_sea: u32,
    quality_pm: u16,
    assigned: &[u32],
) -> Result<Vec<Candidate>, DataError> {
    let mut candidates = Vec::new();
    for (index, method) in building.methods.iter().enumerate() {
        let terrain = if method.resource == "sea" {
            if remaining_sea == 0 {
                continue;
            }
            None
        } else if ["forest", "bog"].contains(&method.resource.as_str()) {
            let cells = land.land_cells.get(&method.resource).copied().unwrap_or(0);
            let needed = method.cells_per_level.unwrap_or(0);
            if needed == 0 || assigned[index] >= cells / needed {
                continue;
            }
            Some(method.resource.clone())
        } else {
            let Some(terrain) = choose_terrain(
                rules,
                method,
                land,
                u64::from(building.land_per_level) * 1000,
            ) else {
                continue;
            };
            Some(terrain)
        };
        let mut output = Vec::with_capacity(method.annual_output.len());
        for (good_id, base_output) in &method.annual_output {
            let good_index = rules
                .goods
                .iter()
                .position(|good| &good.id == good_id)
                .ok_or_else(|| {
                    missing_data(
                        format!("goods.{good_id}"),
                        "method output references unknown good",
                    )
                })?;
            let units = starting_method_output(
                rules,
                building,
                method,
                good_id,
                *base_output,
                terrain.as_deref(),
                quality_pm,
            )?;
            output.push((good_index, units));
        }
        candidates.push(Candidate {
            index,
            terrain,
            output,
        });
    }
    Ok(candidates)
}

/// Give each of a building's `wanted` levels a method, one at a time, by the place's needs and then
/// its empty orders (economy design §3.4); `demand` carries from building to building.
fn assign_building_levels(
    rules: &Rules,
    building: &BuildingType,
    wanted: u32,
    land: &mut LandCounts,
    remaining_sea: &mut u32,
    demand: &mut StartDemand,
    quality_pm: u16,
) -> Result<BuildingAssignment, DataError> {
    let mut assigned = vec![0_u32; building.methods.len()];
    let mut level_records = Vec::<LevelRecord>::new();
    let mut used_land = 0_u32;
    let mut used_sea = 0_u32;
    let mut output = BTreeMap::<String, u64>::new();
    let mut cut_for_room = None;
    for _ in 0..wanted {
        let candidates =
            level_candidates(rules, building, land, *remaining_sea, quality_pm, &assigned)?;
        let mut chosen = demand.choose(rules, building, &candidates, &assigned);
        let mut raised = None;
        if chosen.is_none() {
            raised = Some(demand.raise());
            chosen = demand.choose(rules, building, &candidates, &assigned);
            if chosen.is_none() {
                chosen = StartDemand::fallback(building, &candidates);
            }
        }
        let Some(position) = chosen else {
            if let Some(before) = raised {
                demand.undo(before);
            }
            cut_for_room = Some(candidates.is_empty());
            break;
        };
        let Candidate {
            index,
            terrain,
            output: level_output,
        } = candidates
            .into_iter()
            .nth(position)
            .expect("the chosen candidate exists");
        let method = &building.methods[index];
        assigned[index] += 1;
        level_records.push(LevelRecord {
            number: u32::try_from(level_records.len())
                .map_err(|_| missing_data("projection".to_owned(), "level count overflow"))?,
            method: u32::try_from(index)
                .map_err(|_| missing_data("projection".to_owned(), "method count overflow"))?,
            ground: terrain.clone(),
            paydays: MATURE_PAYDAYS,
            unpaid_work: seasonal_accounts(rules, building, method),
            last_switch_year: None,
        });
        demand.provide(rules, &level_output);
        if method.resource == "sea" {
            used_sea = used_sea.saturating_add(1);
        } else if method.resource == "usable_land" {
            let terrain = terrain.as_ref().expect("land method has terrain");
            let fertility = land
                .fertility_pm
                .get_mut(terrain)
                .expect("selected terrain exists");
            let cost = u64::from(building.land_per_level) * 1000;
            *fertility -= cost;
            used_land = used_land.saturating_add(u32::from(building.land_per_level));
        }
        for (good_index, units) in level_output {
            let produced = u64::try_from(units)
                .map_err(|_| missing_data("projection".to_owned(), "annual output exceeds u64"))?;
            let current = output
                .entry(rules.goods[good_index].id.clone())
                .or_default();
            *current = (*current).checked_add(produced).ok_or_else(|| {
                missing_data("projection".to_owned(), "annual output sum overflow")
            })?;
        }
    }
    let (method_suitability, herd_grazing_sums) =
        derive_level_sums(rules, building, &level_records)?;
    Ok(BuildingAssignment {
        method_levels: assigned,
        level_records,
        method_suitability,
        herd_grazing_sums,
        used_land,
        used_sea,
        output,
        cut_for_room,
    })
}

/// What a producing building knows as sums, derived from its level records: the suitability of each method for each of its goods
/// (each level's own, on its own ground, added up) and the grazing of each species (each level's ground, added up).
///
/// # Errors
/// Returns a [`DataError`] when a record names no method of `building`, or a sum overflows.
pub fn derive_level_sums(
    rules: &Rules,
    building: &BuildingType,
    records: &[LevelRecord],
) -> Result<(Vec<Vec<u64>>, Vec<u64>), DataError> {
    let mut method_suitability = vec![vec![0_u64; rules.goods.len()]; building.methods.len()];
    let mut herd_grazing_sums = vec![0_u64; rules.herd_species.len()];
    for record in records {
        let index = usize::try_from(record.method).unwrap_or(usize::MAX);
        let method = building.methods.get(index).ok_or_else(|| {
            missing_data(
                format!("buildings.{}.levels", building.id),
                "level record names no method",
            )
        })?;
        let ground = record.ground.as_deref();
        for good_id in method.annual_output.keys() {
            let good_index = rules
                .goods
                .iter()
                .position(|good| &good.id == good_id)
                .ok_or_else(|| {
                    missing_data(
                        format!("goods.{good_id}"),
                        "method output references unknown good",
                    )
                })?;
            let suitability = method_suitability_pm(rules, building, method, good_id, ground);
            method_suitability[index][good_index] = method_suitability[index][good_index]
                .checked_add(u64::from(suitability))
                .ok_or_else(|| missing_data("projection".to_owned(), "suitability sum overflow"))?;
        }
        if let Some(ground) = ground {
            for (species, animal) in rules.herd_species.iter().enumerate() {
                let grazing = rules
                    .grazing
                    .get(ground)
                    .and_then(|values| values.get(&animal.animal))
                    .copied()
                    .unwrap_or(0);
                herd_grazing_sums[species] = herd_grazing_sums[species]
                    .checked_add(u64::from(grazing))
                    .ok_or_else(|| {
                        missing_data(
                            "projection.grazing".to_owned(),
                            "grazing suitability sum overflow",
                        )
                    })?;
            }
        }
    }
    Ok((method_suitability, herd_grazing_sums))
}

/// Check every producing building's level records and production carries in a loaded economy before it is installed.
///
/// `quality_by_place` is the mean fertility of each place, as the monthly step reads it (a farming method's carry
/// denominator depends on it).
///
/// # Errors
/// Returns a message naming the place, the building and the level (or the method and good of a carry) at fault, when:
/// a level's method is not one of the building's; its ground does not fit its method's resource; its completed paydays are
/// not 0 to 13; the numbers are not each of 0 to one less than the building's levels once, or the records do not add up
/// to the building's levels or method counts or to its stored sums; or a production carry is not below its denominator
/// (or is not nothing where the building has no positions).
pub fn validate_level_records(
    rules: &Rules,
    quality_by_place: &BTreeMap<String, u16>,
    state: &EconomyState,
) -> Result<(), String> {
    for place in &state.places {
        let quality_pm = quality_by_place
            .get(&place.id)
            .copied()
            .unwrap_or(rules.fertility_reference_pm);
        for building in &place.buildings {
            let Some(definition) = rules.buildings.iter().find(|item| item.id == building.kind)
            else {
                continue;
            };
            let name = format!("place {} building {}", place.id, building.kind);
            if definition.methods.is_empty() {
                if let Some(record) = building.level_records.first() {
                    return Err(format!(
                        "{name} level {}: the building has no methods",
                        record.number
                    ));
                }
                continue;
            }
            check_level_records(rules, definition, building, &name)?;
            check_production_carries(rules, definition, building, quality_pm, &name)?;
        }
    }
    Ok(())
}

/// Check that no level of a loaded economy last switched in a year after the game's own, before the economy is installed.
///
/// # Errors
/// Returns a message naming the place, the building and the level whose year of last switch is after `year`.
pub fn validate_switch_years(state: &EconomyState, year: u16) -> Result<(), String> {
    for place in &state.places {
        for building in &place.buildings {
            for record in &building.level_records {
                if let Some(switched) = record.last_switch_year
                    && switched > year
                {
                    return Err(format!(
                        "place {} building {} level {}: last switched in the year {switched}, after the game's year {year}",
                        place.id, building.kind, record.number
                    ));
                }
            }
        }
    }
    Ok(())
}

fn check_unpaid_accounts(
    rules: &Rules,
    definition: &BuildingType,
    method: &Method,
    record: &LevelRecord,
    level: &str,
) -> Result<(), String> {
    let expected = seasonal_accounts(rules, definition, method);
    for good in expected.keys() {
        if !record.unpaid_work.contains_key(good) {
            return Err(format!("{level} good {good}: missing unpaid work account"));
        }
    }
    for good in record.unpaid_work.keys() {
        if !expected.contains_key(good) {
            return Err(format!(
                "{level} good {good}: unexpected unpaid work account"
            ));
        }
    }
    Ok(())
}

fn check_level_records(
    rules: &Rules,
    definition: &BuildingType,
    building: &BuildingState,
    name: &str,
) -> Result<(), String> {
    let mut counts = vec![0_u32; definition.methods.len()];
    let mut seen = vec![false; building.levels as usize];
    for record in &building.level_records {
        let level = format!("{name} level {}", record.number);
        let Some(method) = usize::try_from(record.method)
            .ok()
            .and_then(|index| definition.methods.get(index))
        else {
            return Err(format!(
                "{level}: method {} is not one of the building's",
                record.method
            ));
        };
        let ground = record.ground.as_deref();
        let fits = match method.resource.as_str() {
            "usable_land" => {
                ground.is_some_and(|ground| rules.terrain_order.iter().any(|t| t == ground))
            }
            "forest" => ground == Some("forest"),
            "bog" => ground == Some("bog"),
            "sea" => ground.is_none(),
            _ => false,
        };
        if !fits {
            return Err(format!(
                "{level}: ground {} does not fit the {} method {}",
                ground.unwrap_or("(none)"),
                method.resource,
                method.id
            ));
        }
        check_unpaid_accounts(rules, definition, method, record, &level)?;
        if record.paydays > MATURE_PAYDAYS {
            return Err(format!(
                "{level}: {} completed paydays is more than {MATURE_PAYDAYS}",
                record.paydays
            ));
        }
        match seen.get_mut(record.number as usize) {
            Some(slot) if !*slot => *slot = true,
            Some(_) => return Err(format!("{level}: the number is used twice")),
            None => {
                return Err(format!(
                    "{level}: the number is not below the building's {} levels",
                    building.levels
                ));
            }
        }
        counts[record.method as usize] += 1;
    }
    if building.level_records.len() != building.levels as usize {
        return Err(format!(
            "{name}: {} level records for {} levels",
            building.level_records.len(),
            building.levels
        ));
    }
    if counts != building.method_levels {
        return Err(format!(
            "{name}: the level records count {counts:?} by method, not the saved {:?}",
            building.method_levels
        ));
    }
    let (suitability, grazing) = derive_level_sums(rules, definition, &building.level_records)
        .map_err(|error| format!("{name}: {}", error.problem))?;
    let differs = |stored: &[u64], derived: &[u64]| {
        (0..stored.len().max(derived.len())).any(|index| {
            stored.get(index).copied().unwrap_or(0) != derived.get(index).copied().unwrap_or(0)
        })
    };
    // A building that can produce keeps the matrix whole, a row for each method and a value for each good, and the monthly step
    // refuses anything else; a building with no positions keeps nothing but zeros, in whatever shape.
    let has_positions = building.levels != 0 && definition.workers_per_level != 0;
    let stored_suitability = building
        .method_suitability
        .iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    let derived_suitability = suitability.iter().flatten().copied().collect::<Vec<_>>();
    if (has_positions && building.method_suitability != suitability)
        || differs(&stored_suitability, &derived_suitability)
    {
        return Err(format!(
            "{name}: the saved suitability sums are not those of the level records"
        ));
    }
    if !building.herds.is_empty() && building.herd_grazing_sums.len() != grazing.len() {
        return Err(format!(
            "{name}: the saved grazing sums are not one for each species"
        ));
    }
    if differs(&building.herd_grazing_sums, &grazing) {
        return Err(format!(
            "{name}: the saved grazing sums are not those of the level records"
        ));
    }
    Ok(())
}

fn check_production_carries(
    rules: &Rules,
    definition: &BuildingType,
    building: &BuildingState,
    quality_pm: u16,
    name: &str,
) -> Result<(), String> {
    let positions = u128::from(building.levels) * u128::from(definition.workers_per_level);
    for (method_index, carries) in building.production_carries.iter().enumerate() {
        let method = definition.methods.get(method_index);
        let reference_factor = if method.is_some_and(|method| method.resource == "usable_land") {
            u128::from(reduced_ratio(quality_pm, rules.fertility_reference_pm).1)
        } else {
            1
        };
        let denominator = positions * 12_000_000_000 * reference_factor;
        for (good_index, carry) in carries.iter().enumerate() {
            let good = rules
                .goods
                .get(good_index)
                .map_or("?", |good| good.id.as_str());
            let method_id = method.map_or("?", |method| method.id.as_str());
            if positions == 0 && *carry != 0 {
                return Err(format!(
                    "{name} method {method_id} good {good}: a carry of {carry} where the building has no positions"
                ));
            }
            if positions != 0 && *carry >= denominator {
                return Err(format!(
                    "{name} method {method_id} good {good}: the carry {carry} is not below its denominator {denominator}"
                ));
            }
        }
    }
    if positions != 0
        && (building.production_carries.len() != definition.methods.len()
            || building
                .production_carries
                .iter()
                .any(|carries| carries.len() != rules.goods.len()))
    {
        return Err(format!(
            "{name}: the saved production carries do not match the methods and goods"
        ));
    }
    Ok(())
}

fn starting_method_output(
    rules: &Rules,
    building: &BuildingType,
    method: &Method,
    good_id: &str,
    base_output: u64,
    terrain: Option<&str>,
    quality_pm: u16,
) -> Result<u128, DataError> {
    let suitability = method_suitability_pm(rules, building, method, good_id, terrain);
    let mut numerator = u128::from(base_output)
        .checked_mul(u128::from(suitability))
        .ok_or_else(|| {
            missing_data(
                format!("buildings.methods.{}.annual_output.{good_id}", method.id),
                "annual output estimate overflow",
            )
        })?;
    let mut denominator = 1000_u128;
    if method.resource == "usable_land" {
        numerator = numerator
            .checked_mul(u128::from(quality_pm))
            .ok_or_else(|| {
                missing_data(
                    format!("buildings.methods.{}.annual_output.{good_id}", method.id),
                    "fertility scaled output overflow",
                )
            })?;
        denominator = denominator
            .checked_mul(u128::from(rules.fertility_reference_pm))
            .ok_or_else(|| {
                missing_data(
                    "fertility_reference_pm".to_owned(),
                    "output denominator overflow",
                )
            })?;
    }
    Ok(numerator / denominator)
}

fn building_coverage_pm(
    definition: &BuildingType,
    pops: &[PopState],
    building: &BuildingState,
) -> Result<u16, DataError> {
    let mut needed = 0_u64;
    let mut owners = 0_u64;
    for (pop_kind, workers_per_owner) in &definition.owner_workers {
        if *workers_per_owner == 0 {
            return Err(missing_data(
                format!("buildings.{}.owner_workers.{pop_kind}", definition.id),
                "workers per owner must be positive",
            ));
        }
        let mut workers_of_kind = 0_u64;
        for (index, pop) in pops.iter().enumerate() {
            if pop.kind == *pop_kind {
                let employment = building.employment.get(index).ok_or_else(|| {
                    missing_data(
                        "employment".to_owned(),
                        "building employment does not match pops",
                    )
                })?;
                workers_of_kind =
                    workers_of_kind
                        .checked_add(employment.workers)
                        .ok_or_else(|| {
                            missing_data("employment.workers".to_owned(), "worker count overflow")
                        })?;
                // Owners come from the worker kinds themselves unless the building names its
                // owner kind, which is then counted once however many worker kinds it has.
                if definition.owner_kind.is_none() {
                    owners = owners.checked_add(employment.owners).ok_or_else(|| {
                        missing_data("employment.owners".to_owned(), "owner count overflow")
                    })?;
                }
            }
        }
        let ratio = u64::from(*workers_per_owner);
        let type_needed = workers_of_kind
            .checked_add(ratio - 1)
            .ok_or_else(|| missing_data("employment.coverage".to_owned(), "owner need overflow"))?
            / ratio;
        needed = needed
            .checked_add(type_needed)
            .ok_or_else(|| missing_data("employment.coverage".to_owned(), "owner need overflow"))?;
    }
    if let Some(owner_kind) = &definition.owner_kind {
        for (index, pop) in pops.iter().enumerate() {
            if pop.kind == *owner_kind {
                let employment = building.employment.get(index).ok_or_else(|| {
                    missing_data(
                        "employment".to_owned(),
                        "building employment does not match pops",
                    )
                })?;
                owners = owners.checked_add(employment.owners).ok_or_else(|| {
                    missing_data("employment.owners".to_owned(), "owner count overflow")
                })?;
            }
        }
    }
    if needed == 0 {
        return Ok(1000);
    }
    let coverage = u128::from(owners).checked_mul(1000).ok_or_else(|| {
        missing_data(
            "employment.coverage".to_owned(),
            "coverage numerator overflow",
        )
    })? / u128::from(needed);
    Ok(u16::try_from(coverage.min(1000)).expect("coverage is at most 1000"))
}

#[expect(
    clippy::too_many_lines,
    reason = "The fee debit and credit share one transactional storehouse accounting step."
)]
#[expect(
    clippy::too_many_arguments,
    reason = "The local prices and month index belong to the atomic fee accounting step."
)]
fn pay_storehouse_fee(
    rules: &Rules,
    place_id: &str,
    prices_milli: &[u64],
    month_index: usize,
    year: u16,
    pops: &mut [PopState],
    buildings: &mut [BuildingState],
    building_index: usize,
    place_counters: &mut CounterBuckets,
    ledger: &mut PlaceProductionLedger,
) -> Result<(), DataError> {
    let (fees, worker_weights) = {
        let building = buildings
            .get_mut(building_index)
            .ok_or_else(|| missing_data("storehouse".to_owned(), "storehouse index is missing"))?;
        let accounts = building.storehouse_accounts.as_mut().ok_or_else(|| {
            missing_data("storehouse.accounts".to_owned(), "account table is missing")
        })?;
        if accounts.pops.len() != pops.len()
            || accounts.communal.len() != rules.goods.len()
            || building.fee_carries.len() != accounts.pops.len() + 1
            || building.stock.len() != rules.goods.len()
            || building.employment.len() != pops.len()
            || building.pay_carries.len() != rules.goods.len()
            || building.wage_history.len() != WAGE_HISTORY_MONTHS
            || building.paid_history.len() != WAGE_HISTORY_MONTHS
            || building.paid_years.len() != WAGE_HISTORY_MONTHS
        {
            return Err(missing_data(
                format!("places.{place_id}.buildings.{}.storehouse", building.kind),
                "storehouse accounts, carries, stock and employment dimensions do not match",
            ));
        }
        let mut fees = vec![0_u64; rules.goods.len()];
        for account_index in 0..=accounts.pops.len() {
            let account = if account_index == accounts.pops.len() {
                &mut accounts.communal
            } else {
                &mut accounts.pops[account_index]
            };
            if account.len() != rules.goods.len()
                || building.fee_carries[account_index].len() != rules.goods.len()
            {
                return Err(missing_data(
                    "storehouse.accounts".to_owned(),
                    "account good dimensions do not match rules",
                ));
            }
            for good_index in 0..rules.goods.len() {
                let numerator = u128::from(account[good_index])
                    .checked_mul(10)
                    .and_then(|value| {
                        value.checked_add(u128::from(
                            building.fee_carries[account_index][good_index],
                        ))
                    })
                    .ok_or_else(|| {
                        missing_data("storehouse.fee".to_owned(), "fee numerator overflow")
                    })?;
                let fee = u64::try_from(numerator / 1000)
                    .map_err(|_| missing_data("storehouse.fee".to_owned(), "fee exceeds u64"))?;
                building.fee_carries[account_index][good_index] =
                    u64::try_from(numerator % 1000).expect("fee remainder fits u64");
                account[good_index] -= fee;
                fees[good_index] = fees[good_index].checked_add(fee).ok_or_else(|| {
                    missing_data("storehouse.fee".to_owned(), "fee total overflow")
                })?;
            }
        }
        building.stock = storehouse_physical_stock_checked(rules, accounts)?;
        let worker_weights = building
            .employment
            .iter()
            .map(|item| item.workers)
            .collect::<Vec<_>>();
        (fees, worker_weights)
    };
    let paid_heads = worker_weights
        .iter()
        .try_fold(0_u128, |sum, workers| sum.checked_add(u128::from(*workers)))
        .ok_or_else(|| {
            missing_data(
                format!("places.{place_id}.buildings.storehouse.wage_history"),
                "paid head count overflow",
            )
        })?;
    if paid_heads > 0 {
        // Value this month's fee only; old pay carries released below are not new fees.
        let wage_value = fees
            .iter()
            .enumerate()
            .try_fold(0_u128, |total, (good_index, fee)| {
                let entry = format!("places.{place_id}.buildings.storehouse.wage_history");
                let price = prices_milli.get(good_index).copied().ok_or_else(|| {
                    missing_data(
                        format!("places.{place_id}.prices_milli"),
                        "saved prices do not match goods",
                    )
                })?;
                let value = value_at_price(u128::from(*fee), price)
                    .map_err(|problem| missing_data(entry.clone(), problem))?;
                total
                    .checked_add(value)
                    .ok_or_else(|| missing_data(entry, "payday fee value overflow"))
            })?;
        let posted = u64::try_from(wage_value / paid_heads).map_err(|_| {
            missing_data(
                format!("places.{place_id}.buildings.storehouse.wage_history"),
                "posted payday fee exceeds u64",
            )
        })?;
        buildings[building_index].wage_history[month_index] = posted;
        buildings[building_index].paid_history[month_index] =
            u64::try_from(paid_heads).unwrap_or(u64::MAX);
        buildings[building_index].paid_years[month_index] = year;
    }
    for (good_index, fee) in fees.into_iter().enumerate() {
        let carried = buildings[building_index].pay_carries[good_index];
        let fee = fee
            .checked_add(carried)
            .ok_or_else(|| missing_data("storehouse.fee".to_owned(), "carried fee overflow"))?;
        if worker_weights.iter().all(|count| *count == 0) {
            buildings[building_index].pay_carries[good_index] = fee;
            continue;
        }
        buildings[building_index].pay_carries[good_index] = 0;
        let shares = largest_remainder(fee, &worker_weights)
            .map_err(|problem| missing_data("storehouse.fee.distribution".to_owned(), problem))?;
        let mut overflow = Vec::new();
        for (pop_index, share) in shares.into_iter().enumerate() {
            if share == 0 {
                continue;
            }
            let pop = pops.get_mut(pop_index).ok_or_else(|| {
                missing_data("storehouse.fee.worker".to_owned(), "worker pop is missing")
            })?;
            let unpaid = receive_pop(rules, pop, good_index, share)?;
            if unpaid > 0 {
                overflow.push((Some(pop_index), unpaid));
            }
        }
        if !overflow.is_empty() {
            settle_overflow(
                rules,
                pops,
                buildings,
                building_index,
                good_index,
                &overflow,
                place_counters,
                ledger,
            )?;
        }
    }
    Ok(())
}

/// Whether this building's configured herd supplies a good.
#[must_use]
pub fn herd_supplies(rules: &Rules, building: &BuildingType, good_id: &str) -> bool {
    rules
        .herd_species
        .iter()
        .filter(|species| building.herds.contains_key(&species.animal))
        .any(|species| {
            species.yields.iter().any(|item| item.good == good_id)
                || species.products.iter().any(|item| item.good == good_id)
        })
}

fn seasonal_accounts(
    rules: &Rules,
    building: &BuildingType,
    method: &Method,
) -> BTreeMap<String, bigint::Big> {
    method
        .annual_output
        .keys()
        .filter(|good| !herd_supplies(rules, building, good))
        .map(|good| (good.clone(), bigint::Big::default()))
        .collect()
}

fn seed_unpaid_work(
    rules: &Rules,
    pops: &[PopState],
    buildings: &mut [BuildingState],
    quality: u16,
    start_month: u8,
) -> Result<(), DataError> {
    for building in buildings {
        let definition = rules
            .buildings
            .iter()
            .find(|item| item.id == building.kind)
            .ok_or_else(|| {
                missing_data(
                    format!("buildings.{}", building.kind),
                    "definition is missing",
                )
            })?;
        if definition.methods.is_empty() {
            continue;
        }
        let positions = u64::from(building.levels) * u64::from(definition.workers_per_level);
        if positions == 0 {
            continue;
        }
        let staffed = building
            .employment
            .iter()
            .try_fold(0_u64, |sum, e| sum.checked_add(e.workers))
            .ok_or_else(|| missing_data("employment.workers".into(), "worker count overflow"))?
            .min(positions);
        let coverage = u64::from(building_coverage_pm(definition, pops, building)?);
        let efficiency = 1000 - u64::from(definition.penalty_pm) * (1000 - coverage) / 1000;
        for record in &mut building.level_records {
            let method = &definition.methods[record.method as usize];
            for (good, account) in &mut record.unpaid_work {
                let season_key = if ["oats", "barley", "peas_beans"].contains(&good.as_str()) {
                    "grain"
                } else {
                    good.as_str()
                };
                let season = rules.seasons.get(season_key).ok_or_else(|| {
                    missing_data(format!("seasons.{season_key}"), "season table is missing")
                })?;
                let suitability = method_suitability_pm(
                    rules,
                    definition,
                    method,
                    good,
                    record.ground.as_deref(),
                );
                let (q, reference) = if method.resource == "usable_land" {
                    (quality, rules.fertility_reference_pm)
                } else {
                    (1, 1)
                };
                for offset in 0..12 {
                    let month = usize::from((start_month - 1 + offset) % 12);
                    accrue_unpaid_work(
                        account,
                        method.annual_output[good],
                        suitability,
                        MATURE_PAYDAYS,
                        u128::from(season[month]),
                        staffed,
                        positions,
                        efficiency,
                        q,
                        reference,
                    );
                }
            }
        }
    }
    Ok(())
}

// The account records work at this payday's yield, before the building's output carry.
#[expect(
    clippy::too_many_arguments,
    reason = "The inputs are the distinct factors of one payday's exact production formula."
)]
fn accrue_unpaid_work(
    account: &mut bigint::Big,
    annual: u64,
    suitability: u16,
    count: u8,
    month_share: u128,
    staffed: u64,
    positions: u64,
    coverage: u64,
    quality: u16,
    reference: u16,
) {
    let seasonal = i128::try_from(month_share).unwrap_or(0) * 12;
    let balance = 1000_i128 - seasonal;
    let factors = [
        u128::from(annual),
        u128::from(suitability),
        u128::from(count - 1),
        balance.unsigned_abs(),
        u128::from(staffed),
        u128::from(coverage),
        u128::from(quality),
    ];
    let denominator = u128::from(positions) * 1_000_000 * u128::from(reference);
    let small = factors
        .iter()
        .try_fold(1_u128, |product, factor| product.checked_mul(*factor));
    let (amount, has_remainder) = if let Some(numerator) = small {
        (
            bigint::Big::new(numerator / denominator),
            numerator % denominator != 0,
        )
    } else {
        let (amount, remainder) = bigint::product(&factors).divmod(&bigint::Big::new(denominator));
        (amount, remainder != bigint::Big::default())
    };
    if balance >= 0 {
        *account = account.add(&amount);
    } else {
        let debit = if has_remainder {
            amount.add(&bigint::Big::new(1))
        } else {
            amount
        };
        *account = account.sub(&debit).unwrap_or_default();
    }
}

fn method_suitability_pm(
    rules: &Rules,
    building: &BuildingType,
    method: &Method,
    good_id: &str,
    terrain: Option<&str>,
) -> u16 {
    let suitability = if herd_supplies(rules, building, good_id) {
        // Preserve the authored startup estimate for outputs supplied by this herd.
        750_u128
    } else if method.resource == "sea" {
        1000_u128
    } else {
        terrain
            .and_then(|key| rules.suitability.get(key))
            .and_then(|values| {
                values
                    .get(good_id)
                    .or_else(|| values.get(&method.suitability))
            })
            .copied()
            .unwrap_or(0)
            .into()
    };
    u16::try_from(suitability).unwrap_or(0)
}

/// A place's yearly units of a food: the sum of its pops' amounts, which the start's orders
/// (`projected_pop_order_units`) are built from.
#[cfg(test)]
fn projected_buy_units(rules: &Rules, pops: &[PopState], good_id: &str) -> Result<u128, DataError> {
    let good_index = rules
        .goods
        .iter()
        .position(|good| good.id == good_id)
        .ok_or_else(|| missing_data(format!("goods.{good_id}"), "definition is missing"))?;
    let mut total = 0_u128;
    for pop in pops {
        total = total
            .checked_add(projected_pop_food_units(rules, pop, good_index)?)
            .ok_or_else(|| missing_data("projection".to_owned(), "diet demand sum overflow"))?;
    }
    Ok(total)
}

/// The whole units of a food one pop's diet asks for in a year, rounded down.
fn projected_pop_food_units(
    rules: &Rules,
    pop: &PopState,
    good_index: usize,
) -> Result<u128, DataError> {
    let good = rules
        .goods
        .get(good_index)
        .ok_or_else(|| missing_data("projection".to_owned(), "good index is outside rules"))?;
    if good.hunger_centi == 0 {
        return Ok(0);
    }
    let shares = rules
        .diet_profiles
        .get(&pop.kind)
        .ok_or_else(|| missing_data(format!("diet_profiles.{}", pop.kind), "profile is missing"))?;
    let normalized = normalized_diet_shares(rules, shares)?;
    let share = normalized.get(good_index).copied().unwrap_or(0);
    if share == 0 {
        return Ok(0);
    }
    let people = pop
        .workers
        .checked_add(pop.dependents)
        .ok_or_else(|| missing_data("projection".to_owned(), "population sum overflow"))?;
    let numerator = u128::from(people)
        .checked_mul(365)
        .and_then(|value| value.checked_mul(100))
        .and_then(|value| value.checked_mul(u128::from(share)))
        .ok_or_else(|| missing_data("projection".to_owned(), "diet demand overflow"))?;
    let denominator = 1000_u128
        .checked_mul(u128::from(good.hunger_centi))
        .ok_or_else(|| missing_data("projection".to_owned(), "diet denominator overflow"))?;
    Ok(numerator / denominator)
}

/// The whole units of a good one pop orders for a year when the opening places levels: the larger
/// of its food amount and its comfort amount, as the live market takes the larger within a pop.
fn projected_pop_order_units(
    rules: &Rules,
    pop: &PopState,
    good_index: usize,
) -> Result<u128, DataError> {
    let people = pop
        .workers
        .checked_add(pop.dependents)
        .ok_or_else(|| missing_data("projection".to_owned(), "population sum overflow"))?;
    let food = projected_pop_food_units(rules, pop, good_index)?;
    let comfort = tier_target_units_for_days(rules, &pop.kind, people, good_index, 1, 365)?.max(
        tier_target_units_for_days(rules, &pop.kind, people, good_index, 2, 365)?,
    );
    Ok(food.max(comfort))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// Orders used by a place's latest daily price step; cached for display, never canonicalized.
pub struct DailyMarketOrders {
    /// Units bought by local pops and buildings, in rules good order.
    pub buy_units: Vec<u128>,
    /// Units offered by local pops and buildings, in rules good order.
    pub sell_units: Vec<u128>,
}

fn food_target_units(
    rules: &Rules,
    pop_kind: &str,
    people: u64,
    good_index: usize,
    days: u64,
) -> Result<u128, DataError> {
    let good = rules
        .goods
        .get(good_index)
        .ok_or_else(|| missing_data("market.good".to_owned(), "good index is outside rules"))?;
    if good.hunger_centi == 0 || people == 0 {
        return Ok(0);
    }
    let profile = rules
        .diet_profiles
        .get(pop_kind)
        .ok_or_else(|| missing_data(format!("diet_profiles.{pop_kind}"), "profile is missing"))?;
    let shares = normalized_diet_shares(rules, profile)?;
    let share = u128::from(shares[good_index]);
    if share == 0 {
        return Ok(0);
    }
    let need = u128::from(people)
        .checked_mul(100)
        .ok_or_else(|| missing_data("market.need".to_owned(), "daily need overflow"))?;
    let numerator = share
        .checked_mul(need)
        .and_then(|value| value.checked_mul(u128::from(days)))
        .ok_or_else(|| {
            missing_data("market.food_target".to_owned(), "target numerator overflow")
        })?;
    let denominator = 1000_u128
        .checked_mul(u128::from(good.hunger_centi))
        .ok_or_else(|| {
            missing_data(
                "market.food_target".to_owned(),
                "target denominator overflow",
            )
        })?;
    let whole = numerator / denominator;
    whole
        .checked_add(u128::from(numerator % denominator != 0))
        .ok_or_else(|| missing_data("market.food_target".to_owned(), "target units overflow"))
}

fn tier_target_units(
    rules: &Rules,
    pop_kind: &str,
    people: u64,
    good_index: usize,
    tier: usize,
) -> Result<u128, DataError> {
    tier_target_units_for_days(rules, pop_kind, people, good_index, tier, 240)
}

/// A comfort tier's target units for `days` of the pop's need.
fn tier_target_units_for_days(
    rules: &Rules,
    pop_kind: &str,
    people: u64,
    good_index: usize,
    tier: usize,
    days: u128,
) -> Result<u128, DataError> {
    let good = rules
        .goods
        .get(good_index)
        .ok_or_else(|| missing_data("market.good".to_owned(), "good index is outside rules"))?;
    let pop_type = rules
        .pop_types
        .iter()
        .find(|pop| pop.id == pop_kind)
        .ok_or_else(|| {
            missing_data(
                format!("pop_types.{pop_kind}"),
                "population type is missing",
            )
        })?;
    let (tier_name, expectation, value) = match tier {
        1 => (
            "niceties",
            pop_type.niceties_expectation_pm,
            good.niceties_centi,
        ),
        2 => (
            "luxuries",
            pop_type.luxuries_expectation_pm,
            good.luxuries_centi,
        ),
        _ => {
            return Err(missing_data(
                "market.tier".to_owned(),
                "tier index must be 1 or 2",
            ));
        }
    };
    let share = rules
        .comfort_profiles
        .get(pop_kind)
        .and_then(|profile| profile.get(tier_name))
        .and_then(|shares| shares.get(&good.id))
        .copied()
        .unwrap_or(0);
    if expectation == 0 || share == 0 || value == 0 || people == 0 {
        return Ok(0);
    }
    let need = u128::from(people)
        .checked_mul(100)
        .ok_or_else(|| missing_data("market.need".to_owned(), "daily need overflow"))?;
    let tier_target = u128::from(expectation)
        .checked_mul(need)
        .and_then(|target| target.checked_mul(days))
        .ok_or_else(|| missing_data("market.tier_target".to_owned(), "target overflow"))?
        / 1000;
    let numerator = u128::from(share)
        .checked_mul(tier_target)
        .ok_or_else(|| missing_data("market.tier_want".to_owned(), "want numerator overflow"))?;
    let denominator = 1000_u128
        .checked_mul(u128::from(value))
        .ok_or_else(|| missing_data("market.tier_want".to_owned(), "want denominator overflow"))?;
    let whole = numerator / denominator;
    whole
        .checked_add(u128::from(numerator % denominator != 0))
        .ok_or_else(|| missing_data("market.tier_want".to_owned(), "want units overflow"))
}

fn pop_market_holding(
    place: &PlaceState,
    pop_index: usize,
    good_index: usize,
) -> Result<u128, DataError> {
    let pop = place
        .pops
        .get(pop_index)
        .ok_or_else(|| missing_data("market.pop".to_owned(), "pop index is outside place"))?;
    let mut holding =
        u128::from(*pop.stock.get(good_index).ok_or_else(|| {
            missing_data("market.pop.stock".to_owned(), "good dimension mismatch")
        })?);
    for building in &place.buildings {
        if let Some(accounts) = &building.storehouse_accounts {
            let account = accounts.pops.get(pop_index).ok_or_else(|| {
                missing_data(
                    format!("places.{}.storehouse.accounts", place.id),
                    "pop account dimension mismatch",
                )
            })?;
            let amount = account.get(good_index).ok_or_else(|| {
                missing_data(
                    "market.storehouse.account".to_owned(),
                    "good dimension mismatch",
                )
            })?;
            holding = holding.checked_add(u128::from(*amount)).ok_or_else(|| {
                missing_data("market.pop.holding".to_owned(), "holding sum overflow")
            })?;
        }
    }
    Ok(holding)
}

fn building_market_holding(
    rules: &Rules,
    place: &PlaceState,
    building: &BuildingState,
    good_index: usize,
) -> Result<u128, DataError> {
    if let Some(accounts) = &building.storehouse_accounts {
        if accounts.communal.len() != rules.goods.len()
            || accounts
                .pops
                .iter()
                .any(|account| account.len() != rules.goods.len())
            || storehouse_physical_stock_checked(rules, accounts)? != building.stock
        {
            return Err(missing_data(
                format!("places.{}.storehouse", place.id),
                "storehouse physical stock differs from its accounts",
            ));
        }
        return Ok(u128::from(*accounts.communal.get(good_index).ok_or_else(
            || {
                missing_data(
                    "market.storehouse.communal".to_owned(),
                    "good dimension mismatch",
                )
            },
        )?));
    }
    Ok(u128::from(*building.stock.get(good_index).ok_or_else(
        || {
            missing_data(
                "market.building.stock".to_owned(),
                "good dimension mismatch",
            )
        },
    )?))
}

/// A pop's target for a good: the larger of its 240-day food target and its comfort target.
fn pop_target_units(
    rules: &Rules,
    pop_kind: &str,
    people: u64,
    good_index: usize,
) -> Result<u128, DataError> {
    let target = food_target_units(rules, pop_kind, people, good_index, 240)?;
    let comfort_target = tier_target_units(rules, pop_kind, people, good_index, 1)?
        .max(tier_target_units(rules, pop_kind, people, good_index, 2)?);
    Ok(target.max(comfort_target))
}

fn daily_market_orders(rules: &Rules, place: &PlaceState) -> Result<DailyMarketOrders, DataError> {
    let goods = rules.goods.len();
    let mut orders = DailyMarketOrders {
        buy_units: vec![0; goods],
        sell_units: vec![0; goods],
    };
    for (pop_index, pop) in place.pops.iter().enumerate() {
        let people = pop.workers.checked_add(pop.dependents).ok_or_else(|| {
            missing_data("market.pop.size".to_owned(), "population size overflow")
        })?;
        let daily = fuel::fuel_daily_need(rules, pop)?;
        let target = daily
            .checked_mul(240)
            .ok_or_else(|| missing_data("fuel.target".to_owned(), "target overflow"))?;
        let holding = fuel::fuel_holding(rules, &pop.stock, pop.consumption.hearth_milli)?;
        if let Some(index) = fuel::fuel_ranking(rules, &place.prices_milli, false)?
            .first()
            .copied()
        {
            let supply = rules.goods[index].category_supplied("fuel", 1);
            orders.buy_units[index] = orders.buy_units[index]
                .checked_add(target.saturating_sub(holding).div_ceil(supply))
                .ok_or_else(|| missing_data("fuel.buy".to_owned(), "buy overflow"))?;
        }
        let spare = fuel::fuel_spare(
            rules,
            &pop.stock,
            pop.consumption.hearth_milli,
            target,
            &place.prices_milli,
        )?;
        for (index, units) in spare.into_iter().enumerate() {
            orders.sell_units[index] = orders.sell_units[index]
                .checked_add(u128::from(units))
                .ok_or_else(|| missing_data("fuel.sell".to_owned(), "sell overflow"))?;
        }
        for (good_index, good) in rules.goods.iter().enumerate() {
            if good.category_supplied("fuel", 1) > 0 {
                continue;
            }
            let holding = pop_market_holding(place, pop_index, good_index)?;
            let target = pop_target_units(rules, &pop.kind, people, good_index)?;
            let buy = target.saturating_sub(holding);
            orders.buy_units[good_index] = orders.buy_units[good_index]
                .checked_add(buy)
                .ok_or_else(|| missing_data("market.buy".to_owned(), "buy order overflow"))?;
            // A pop offers whatever it holds above its target, with or without a hunger value (CK-391): the
            // exchange swaps the same spare, so the price step and the exchange see the same offers.
            let sell = holding.saturating_sub(target);
            orders.sell_units[good_index] = orders.sell_units[good_index]
                .checked_add(sell)
                .ok_or_else(|| missing_data("market.sell".to_owned(), "sell order overflow"))?;
        }
    }
    // Owners of a herd below its standard level want animals; the want is a buy order like a good's (CK-430, r163).
    let live = live_species(rules);
    let wants = herd_wants(rules, place)?;
    for (good, species) in live.iter().enumerate() {
        let Some(species) = *species else { continue };
        for building in &wants {
            for pop in building {
                orders.buy_units[good] = orders.buy_units[good]
                    .checked_add(u128::from(pop[species]))
                    .ok_or_else(|| missing_data("market.buy".to_owned(), "buy order overflow"))?;
            }
        }
    }
    for building in building_market_sells(rules, place, &place.prices_milli)? {
        for (good, sell) in building.into_iter().enumerate() {
            orders.sell_units[good] = orders.sell_units[good]
                .checked_add(sell)
                .ok_or_else(|| missing_data("market.sell".to_owned(), "sell order overflow"))?;
        }
    }
    Ok(orders)
}

/// The herd species each good is the live animal of (CK-430), by good index: a species' animal is a good of the same id.
pub(crate) fn live_species(rules: &Rules) -> Vec<Option<usize>> {
    rules
        .goods
        .iter()
        .map(|good| {
            rules
                .herd_species
                .iter()
                .position(|species| species.animal == good.id)
        })
        .collect()
}

/// A herd's standard level (r163): its maximum times the standard per-mille, rounded down.
pub(crate) fn herd_standard_level(rules: &Rules, maximum: u64) -> u64 {
    u64::try_from(u128::from(maximum) * u128::from(rules.herd_standard_pm) / 1000)
        .expect("a share of a u64 fits a u64")
}

/// The animals a building's herds offer above their standard levels and want below them, by species: `(offer, want)`, both zero where
/// the building keeps no such herd. This is the one place that reads the standard level.
pub(crate) fn herd_levels(
    rules: &Rules,
    building: &BuildingState,
) -> Result<Vec<(u64, u64)>, DataError> {
    let definition = rules
        .buildings
        .iter()
        .find(|item| item.id == building.kind)
        .ok_or_else(|| {
            missing_data(
                format!("buildings.{}", building.kind),
                "definition is missing",
            )
        })?;
    let farming_levels = definition
        .methods
        .iter()
        .zip(&building.method_levels)
        .filter(|(method, _)| method.resource != "sea")
        .try_fold(0_u64, |sum, (_, levels)| {
            sum.checked_add(u64::from(*levels))
        })
        .ok_or_else(|| missing_data("herds".to_owned(), "farming level sum overflow"))?;
    rules
        .herd_species
        .iter()
        .enumerate()
        .map(|(species, species_rules)| {
            let Some(capacity) = definition.herds.get(&species_rules.animal) else {
                return Ok((0, 0));
            };
            let maximum = farming_levels
                .checked_mul(capacity.maximum)
                .ok_or_else(|| missing_data("herds".to_owned(), "herd maximum overflow"))?;
            let standard = herd_standard_level(rules, maximum);
            let count = building.herds.get(species).copied().unwrap_or(0);
            Ok((
                count.saturating_sub(standard),
                standard.saturating_sub(count),
            ))
        })
        .collect()
}

/// What each owner pop wants of each building's herds below standard (r163): a herd's shortfall split by the owners' positions as a
/// building's spare stock is, indexed `[building][pop][species]`.
pub(crate) fn herd_wants(
    rules: &Rules,
    place: &PlaceState,
) -> Result<Vec<Vec<Vec<u64>>>, DataError> {
    let species = rules.herd_species.len();
    let mut wants = vec![vec![vec![0_u64; species]; place.pops.len()]; place.buildings.len()];
    for (b, building) in place.buildings.iter().enumerate() {
        if building.herds.is_empty() {
            continue;
        }
        if building.employment.len() != place.pops.len() {
            return Err(missing_data(
                format!("places.{}.buildings.{}.employment", place.id, building.kind),
                "employment dimensions do not match place pops",
            ));
        }
        let weights = building
            .employment
            .iter()
            .map(|job| job.owners)
            .collect::<Vec<_>>();
        for (s, (_, shortfall)) in herd_levels(rules, building)?.into_iter().enumerate() {
            if shortfall == 0 {
                continue;
            }
            let shares = swap::wide_shares(u128::from(shortfall), &weights)
                .map_err(|problem| missing_data("market.herd_want".to_owned(), problem))?;
            for (pop, share) in shares.into_iter().enumerate() {
                wants[b][pop][s] = u64::try_from(share).map_err(|_| {
                    missing_data("market.herd_want".to_owned(), "share exceeds u64")
                })?;
            }
        }
    }
    Ok(wants)
}

/// Each building's offer above its owners' food reserve, also used by the exchange.
fn building_market_sells(
    rules: &Rules,
    place: &PlaceState,
    prices: &[u64],
) -> Result<Vec<Vec<u128>>, DataError> {
    let pop_sizes = place
        .pops
        .iter()
        .map(|pop| {
            pop.workers.checked_add(pop.dependents).ok_or_else(|| {
                missing_data("market.pop.size".to_owned(), "population size overflow")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut owner_people_by_building = vec![vec![0_u64; place.pops.len()]; place.buildings.len()];
    for building in &place.buildings {
        if building.employment.len() != place.pops.len() {
            return Err(missing_data(
                format!("places.{}.buildings.{}.employment", place.id, building.kind),
                "employment dimensions do not match place pops",
            ));
        }
    }
    for (pop_index, people) in pop_sizes.iter().copied().enumerate() {
        let owner_positions = place
            .buildings
            .iter()
            .map(|building| building.employment[pop_index].owners)
            .collect::<Vec<_>>();
        let owner_people = largest_remainder(people, &owner_positions)
            .map_err(|problem| missing_data("market.owner_reserve".to_owned(), problem))?;
        for (building_index, people) in owner_people.into_iter().enumerate() {
            owner_people_by_building[building_index][pop_index] = people;
        }
    }
    let mut sells = vec![vec![0_u128; rules.goods.len()]; place.buildings.len()];
    let live = live_species(rules);
    for (building_index, building) in place.buildings.iter().enumerate() {
        let mut reserve = 0_u128;
        for (pop_index, people) in owner_people_by_building[building_index]
            .iter()
            .copied()
            .enumerate()
        {
            let pop = &place.pops[pop_index];
            let rate = rules
                .pop_types
                .iter()
                .find(|kind| kind.id == pop.kind)
                .ok_or_else(|| {
                    missing_data(format!("pop_types.{}", pop.kind), "definition is missing")
                })?
                .fuel_need_milli_day;
            reserve = reserve
                .checked_add(u128::from(people) * u128::from(rate) * 60)
                .ok_or_else(|| missing_data("fuel.reserve".to_owned(), "reserve overflow"))?;
        }
        let spare = fuel::fuel_spare(rules, &building.stock, 0, reserve, prices)?;
        for (index, units) in spare.into_iter().enumerate() {
            sells[building_index][index] = sells[building_index][index]
                .checked_add(u128::from(units))
                .ok_or_else(|| missing_data("fuel.sell".to_owned(), "sell overflow"))?;
        }
        let herd_offers = herd_levels(rules, building)?;
        for (good_index, good) in rules.goods.iter().enumerate() {
            if good.category_supplied("fuel", 1) > 0 {
                continue;
            }
            if let Some(species) = live[good_index] {
                // A live animal is offered from the herd above its standard level, never from the stock (CK-430).
                sells[building_index][good_index] = u128::from(herd_offers[species].0);
                continue;
            }
            let holding = building_market_holding(rules, place, building, good_index)?;
            let mut owner_reserve = 0_u128;
            for (pop_index, people) in owner_people_by_building[building_index]
                .iter()
                .copied()
                .enumerate()
            {
                if people == 0 {
                    continue;
                }
                let pop = &place.pops[pop_index];
                let target = food_target_units(rules, &pop.kind, people, good_index, 60)?;
                owner_reserve = owner_reserve.checked_add(target).ok_or_else(|| {
                    missing_data("market.owner_reserve".to_owned(), "owner reserve overflow")
                })?;
            }
            sells[building_index][good_index] = if good.hunger_centi > 0 {
                holding.saturating_sub(owner_reserve)
            } else {
                holding
            };
        }
    }
    Ok(sells)
}

fn daily_price_target(
    rules: &Rules,
    good: &Good,
    current_milli: u64,
    buy_units: u128,
    sell_units: u128,
) -> Result<u64, DataError> {
    if buy_units == 0 && sell_units == 0 {
        return Ok(current_milli);
    }
    let buy_value = value_at_price(buy_units, current_milli)
        .map_err(|problem| missing_data("market.buy_value".to_owned(), problem))?;
    let sell_value = value_at_price(sell_units, current_milli)
        .map_err(|problem| missing_data("market.sell_value".to_owned(), problem))?;
    target_price(good.base_price, buy_value, sell_value, rules.price_k_pm)
        .map_err(|problem| missing_data(format!("target_price.{}", good.id), problem))
}

fn update_daily_prices(rules: &Rules, state: &mut EconomyState) -> Result<(), DataError> {
    rules.validate("economy rules")?;
    for place in &mut state.places {
        if place.prices_milli.len() != rules.goods.len()
            || place.price_drift_carries.len() != rules.goods.len()
        {
            return Err(missing_data(
                format!("places.{}.prices", place.id),
                "saved price dimensions do not match goods",
            ));
        }
        let orders = daily_market_orders(rules, place)?;
        for (index, good) in rules.goods.iter().enumerate() {
            let current = place.prices_milli[index];
            let maximum = good.base_price.checked_mul(5000).ok_or_else(|| {
                missing_data(
                    format!("goods.{}.base_price", good.id),
                    "price clamp overflow",
                )
            })?;
            if current == 0 || current > maximum {
                return Err(missing_data(
                    format!("places.{}.prices.{}", place.id, good.id),
                    "saved local price is outside its clamp",
                ));
            }
            let target = daily_price_target(
                rules,
                good,
                current,
                orders.buy_units[index],
                orders.sell_units[index],
            )?;
            let value = i64::try_from(current).map_err(|_| {
                missing_data("market.price".to_owned(), "current price exceeds i64")
            })?;
            let target = i64::try_from(target)
                .map_err(|_| missing_data("market.price".to_owned(), "target price exceeds i64"))?;
            place.prices_milli[index] = u64::try_from(drift_value(
                value,
                target,
                100,
                1000,
                &mut place.price_drift_carries[index],
                1,
                i64::try_from(maximum).unwrap_or(i64::MAX),
            )?)
            .map_err(|_| missing_data("market.price".to_owned(), "drifted price is negative"))?;
        }
        place.last_market_orders = Some(orders);
    }
    Ok(())
}

fn normalized_diet_shares(
    rules: &Rules,
    shares: &BTreeMap<String, u16>,
) -> Result<Vec<u64>, DataError> {
    let weights = rules
        .goods
        .iter()
        .map(|good| u64::from(shares.get(&good.id).copied().unwrap_or(0)))
        .collect::<Vec<_>>();
    largest_remainder(1000, &weights)
        .map_err(|problem| missing_data("diet profile".to_owned(), problem))
}

fn choose_terrain(
    rules: &Rules,
    method: &Method,
    land: &LandCounts,
    fertility_needed: u64,
) -> Option<String> {
    let mut best: Option<(u128, usize, String)> = None;
    for (index, terrain) in rules.terrain_order.iter().enumerate() {
        if land.fertility_pm.get(terrain).copied().unwrap_or(0) < fertility_needed {
            continue;
        }
        let suitability = rules.suitability.get(terrain);
        let score = method
            .score_share_pm
            .iter()
            .fold(0_u128, |sum, (good, share)| {
                let good_suit = suitability
                    .and_then(|values| values.get(good))
                    .copied()
                    .unwrap_or_else(|| {
                        suitability
                            .and_then(|values| values.get(&method.suitability))
                            .copied()
                            .unwrap_or(0)
                    });
                sum + u128::from(*share) * u128::from(good_suit)
            });
        if best
            .as_ref()
            .is_none_or(|(old, old_index, _)| score > *old || (score == *old && index < *old_index))
        {
            best = Some((score, index, terrain.clone()));
        }
    }
    best.map(|(_, _, terrain)| terrain)
}

// Length-prefix each identifier to keep keys independent of list order and unambiguous.
fn herd_draw(seed: u64, keys: &[&str]) -> u64 {
    let mut h = seed ^ 0xcbf2_9ce4_8422_2325;
    for key in keys {
        for byte in (key.len() as u64)
            .to_le_bytes()
            .iter()
            .chain(key.as_bytes())
        {
            h = (h ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h = h.wrapping_add(0x9e37_79b9_7f4a_7c15);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

fn horse_keep_chance(opening: &HerdOpening, people: u64, grazing: u64) -> u64 {
    let base = if people >= opening.court_thresholds[1] {
        opening.court_chances[2]
    } else if people >= opening.court_thresholds[0] {
        opening.court_chances[1]
    } else {
        opening.court_chances[0]
    };
    let bonus = (i128::from(grazing) - 500).div_euclid(i128::from(opening.grazing_divisor));
    u64::try_from((i128::from(base) + bonus).clamp(0, 1000)).expect("clamped")
}

#[expect(
    clippy::too_many_arguments,
    reason = "The opening draw is keyed by the building and its place context."
)]
fn opening_herds(
    rules: &Rules,
    levels: &[u32],
    building: &BuildingType,
    grazing: &[u64],
    seed: u64,
    place: &str,
    people: u64,
    always: bool,
) -> Result<Vec<u64>, DataError> {
    let farming: u64 = building
        .methods
        .iter()
        .zip(levels)
        .filter(|(m, _)| m.resource != "sea")
        .map(|(_, n)| u64::from(*n))
        .sum();
    let o = &rules.herd_opening;
    rules
        .herd_species
        .iter()
        .enumerate()
        .map(|(i, species)| {
            let maximum = farming
                .checked_mul(building.herds.get(&species.animal).map_or(0, |c| c.maximum))
                .ok_or_else(|| {
                    missing_data(
                        format!("buildings.{}.herds.{}", building.id, species.animal),
                        "maximum overflow",
                    )
                })?;
            if maximum == 0 {
                return Ok(0);
            }
            let g = grazing[i] / farming;
            let draw = |purpose| herd_draw(seed, &[place, &building.id, &species.animal, purpose]);
            let kept = match species.animal.as_str() {
                "cattle" => true,
                "horses" => {
                    building.id == "demesne"
                        && (always || draw("keep") % 1000 < horse_keep_chance(o, people, g))
                }
                _ => draw("keep") % 1000 < u64::from(o.keep_pm),
            };
            if !kept {
                return Ok(0);
            }
            let spread = o.spread[0] + draw("spread") % (o.spread[1] - o.spread[0] + 1);
            let rank = if building.id == "demesne" {
                o.demesne_rank_pm
            } else {
                1000
            };
            let count = [
                u128::from(maximum),
                u128::from(rules.herd_start_share[0]),
                u128::from(u64::midpoint(1000, g)),
                u128::from(spread),
                u128::from(rank),
            ]
            .into_iter()
            .try_fold(1_u128, u128::checked_mul)
            .ok_or_else(|| missing_data("herd_opening".to_owned(), "product overflow"))?
                / (u128::from(rules.herd_start_share[1]) * 1_000_000_000);
            Ok(u64::try_from(count.min(u128::from(maximum)).max(u128::from(
                farming.saturating_mul(o.floor_per_level).min(maximum),
            )))
            .expect("bounded by maximum"))
        })
        .collect()
}

#[cfg(test)]
fn herd_counts(
    rules: &Rules,
    method_levels: &[u32],
    building: &BuildingType,
) -> Result<Vec<u64>, DataError> {
    let farming_levels = building
        .methods
        .iter()
        .zip(method_levels)
        .filter(|(method, _)| method.resource != "sea")
        .try_fold(0_u64, |total, (_, levels)| {
            total.checked_add(u64::from(*levels))
        })
        .ok_or_else(|| {
            missing_data(
                format!("buildings.{}.herds", building.id),
                "farming level sum overflow",
            )
        })?;
    rules
        .herd_species
        .iter()
        .map(|species| {
            let start = building
                .herds
                .get(&species.animal)
                .map_or(0, |capacity| capacity.maximum);
            farming_levels
                .checked_mul(start)
                .and_then(|maximum| maximum.checked_mul(rules.herd_start_share[0]))
                .map(|scaled| scaled / rules.herd_start_share[1])
                .ok_or_else(|| {
                    missing_data(
                        format!("buildings.{}.herds.{}", building.id, species.animal),
                        "starting herd count overflow",
                    )
                })
        })
        .collect()
}

fn storehouse_kinds(rules: &Rules, production: &BTreeMap<String, u64>, levels: u32) -> Vec<u32> {
    let goods_by_kind: [&[&str]; 3] = [
        &["oats", "barley", "peas_beans"],
        &[
            "dairy",
            "beef",
            "pork",
            "mutton",
            "fish",
            "apples",
            "vegetables",
        ],
        &["ale", "mead", "wine"],
    ];
    let capacity_ml = 50_u128 * 720 * 1000;
    let mut assigned = [0_u32; 3];
    for _ in 0..levels {
        let values = goods_by_kind
            .iter()
            .enumerate()
            .map(|(index, goods)| {
                goods
                    .iter()
                    .map(|id| {
                        let quantity = u128::from(production.get(*id).copied().unwrap_or(0));
                        let volume = rules
                            .goods
                            .iter()
                            .find(|good| good.id == *id)
                            .map_or(0, |good| u128::from(good.volume_ml));
                        quantity.saturating_mul(volume)
                    })
                    .sum::<u128>()
                    .saturating_sub(u128::from(assigned[index]) * capacity_ml)
            })
            .collect::<Vec<_>>();
        let mut choice = 0;
        for index in 1..3 {
            if values[index] > values[choice] {
                choice = index;
            }
        }
        assigned[choice] += 1;
    }
    assigned.to_vec()
}

#[expect(
    clippy::too_many_lines,
    reason = "Owner and worker allocation stays together so the accepted building order remains explicit."
)]
fn fill_starting_employment(rules: &Rules, pops: &mut [PopState], buildings: &mut [BuildingState]) {
    for pop in pops.iter_mut() {
        pop.employment = vec![Employment::default(); buildings.len()];
    }
    let mut available: Vec<u64> = pops.iter().map(|pop| pop.workers).collect();
    // A building that names its owner kind (the demesne) is filled first: its labourers are
    // merged into a pop other buildings also draw from, and an earlier building's positions
    // must not take them. The other buildings keep their order, so their employment is unchanged.
    let names_owner_kind = |building: &BuildingState| {
        rules
            .buildings
            .iter()
            .any(|kind| kind.id == building.kind && kind.owner_kind.is_some())
    };
    let order = (0..buildings.len())
        .filter(|index| names_owner_kind(&buildings[*index]))
        .chain((0..buildings.len()).filter(|index| !names_owner_kind(&buildings[*index])))
        .collect::<Vec<_>>();
    for building_index in order {
        let building = &mut buildings[building_index];
        let mut remaining_positions = u64::from(building.levels)
            * rules
                .buildings
                .iter()
                .find(|kind| kind.id == building.kind)
                .map_or(0, |kind| u64::from(kind.workers_per_level));
        if building.kind == "holdings" {
            // Owners are monks first; no monthly rate limit applies at setup.
            let config = rules.buildings.iter().find(|kind| kind.id == building.kind);
            let monk_positions = u64::from(building.levels) * 5;
            let labourer_positions = u64::from(building.levels) * 5;
            let monk_ratio = config
                .and_then(|kind| kind.owner_workers.get("monk"))
                .copied()
                .expect("validated holdings declare the monk owner ratio");
            let labourer_ratio = config
                .and_then(|kind| kind.owner_workers.get("labourer"))
                .copied()
                .expect("validated holdings declare the labourer owner ratio");
            let mut owners_needed = monk_positions.div_ceil(u64::from(monk_ratio))
                + labourer_positions.div_ceil(u64::from(labourer_ratio));
            for index in 0..pops.len() {
                if pops[index].kind != "monk" {
                    continue;
                }
                let owners = available[index].min(owners_needed);
                available[index] -= owners;
                owners_needed -= owners;
                building.employment[index].owners += owners;
                pops[index].employment[building_index].owners += owners;
            }
            let mut monk_positions = u64::from(building.levels) * 5;
            let mut labourer_positions = u64::from(building.levels) * 5;
            for kind in ["monk", "labourer"] {
                for index in 0..pops.len() {
                    if pops[index].kind != kind {
                        continue;
                    }
                    let positions = if kind == "monk" {
                        &mut monk_positions
                    } else {
                        &mut labourer_positions
                    };
                    let count = available[index].min(*positions);
                    available[index] -= count;
                    *positions -= count;
                    remaining_positions = remaining_positions.saturating_sub(count);
                    building.employment[index].workers += count;
                    pops[index].employment[building_index].workers += count;
                }
            }
        } else if building.kind == "farm" {
            let owner_workers = rules
                .buildings
                .iter()
                .find(|kind| kind.id == building.kind)
                .and_then(|kind| kind.owner_workers.get("peasant"))
                .copied()
                .expect("validated farms declare the peasant owner ratio");
            let mut owners_needed = remaining_positions.div_ceil(u64::from(owner_workers));
            for index in 0..pops.len() {
                if pops[index].kind != "peasant" {
                    continue;
                }
                let owners = available[index].min(owners_needed);
                available[index] -= owners;
                owners_needed -= owners;
                building.employment[index].owners += owners;
                pops[index].employment[building_index].owners += owners;
            }
            for index in 0..pops.len() {
                if pops[index].kind != "peasant" {
                    continue;
                }
                let count = available[index].min(remaining_positions);
                available[index] -= count;
                remaining_positions -= count;
                building.employment[index].workers += count;
                pops[index].employment[building_index].workers += count;
            }
        } else if let Some((definition, owner_kind)) = rules
            .buildings
            .iter()
            .find(|kind| kind.id == building.kind)
            .and_then(|kind| kind.owner_kind.as_deref().map(|owner| (kind, owner)))
        {
            // Owners are drawn from the owner kind by coverage, one owner per `ratio` workers of
            // each worker kind; the paid positions are then split across the worker kinds.
            let mut owners_needed = 0_u64;
            for (worker_kind, ratio) in &definition.owner_workers {
                let positions = position_capacity(definition, worker_kind, building.levels)
                    .expect("validated positions fit");
                owners_needed += positions.div_ceil(u64::from(*ratio));
            }
            for index in 0..pops.len() {
                if pops[index].kind != owner_kind {
                    continue;
                }
                let owners = available[index].min(owners_needed);
                available[index] -= owners;
                owners_needed -= owners;
                building.employment[index].owners += owners;
                pops[index].employment[building_index].owners += owners;
            }
            for worker_kind in definition.owner_workers.keys() {
                let mut positions = position_capacity(definition, worker_kind, building.levels)
                    .expect("validated positions fit");
                for index in 0..pops.len() {
                    if pops[index].kind != *worker_kind {
                        continue;
                    }
                    let count = available[index].min(positions);
                    available[index] -= count;
                    positions -= count;
                    building.employment[index].workers += count;
                    pops[index].employment[building_index].workers += count;
                }
            }
        } else if building.kind == "storehouse" {
            for index in 0..pops.len() {
                let owns_a_served_building =
                    owns_served_building(&pops[index].employment, building_index);
                if !owns_a_served_building {
                    continue;
                }
                let count = available[index].min(remaining_positions);
                available[index] -= count;
                remaining_positions -= count;
                building.employment[index].workers += count;
                pops[index].employment[building_index].workers += count;
            }
        } else {
            for index in 0..pops.len() {
                let count = available[index].min(remaining_positions);
                available[index] -= count;
                remaining_positions -= count;
                building.employment[index].workers += count;
                pops[index].employment[building_index].workers += count;
            }
        }
    }
}

/// Employment for one pop in one building; owners are separate from producers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Employment {
    /// Unpaid owners/priests.
    pub owners: u64,
    /// Paid, producing workers.
    pub workers: u64,
}

/// Monthly births, deaths and per-good production, use and overflow losses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonthlyCounters {
    /// People born.
    pub births: u64,
    /// People who died.
    pub deaths: u64,
    /// Whole goods units lost after payday overflow.
    pub losses: u64,
    /// Rent that left a tenant pop's store this month (the debit), valued at the local prices, in
    /// thousandths of a value unit.
    pub rent_paid_milli: u64,
    /// Rent due and not paid this month: due minus paid, forgiven residue included, in thousandths
    /// of a value unit.
    pub rent_short_milli: u64,
    /// Whole units produced, indexed by `Rules::goods` order.
    pub produced: Vec<u64>,
    /// Whole units opened or ticked by pops, indexed by `Rules::goods` order.
    pub used_by_pops: Vec<u64>,
    /// Whole units lost to overflow, indexed by `Rules::goods` order.
    pub lost: Vec<u64>,
    /// Produced units valued at the rollover's local prices, in thousandths.
    pub produced_value_milli: Vec<u128>,
    /// Pop-used units valued at the rollover's local prices, in thousandths.
    pub used_by_pops_value_milli: Vec<u128>,
}

/// Display-only counters saved across a quicksave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CounterBuckets {
    /// Events accumulated since this month's first-of-month rollover.
    pub current: MonthlyCounters,
    /// Events from the month completed by the most recent rollover.
    pub last: MonthlyCounters,
    /// Whether `last` contains a completed month rather than an unclosed start state.
    pub closed: bool,
}

fn roll_counter_month(
    counters: &mut CounterBuckets,
    rollover_prices: Option<&[u64]>,
) -> Result<(), DataError> {
    let mut last = counters.current.clone();
    if let Some(prices) = rollover_prices {
        if last.produced.len() != prices.len()
            || last.used_by_pops.len() != prices.len()
            || last.lost.len() != prices.len()
        {
            return Err(missing_data(
                "counters.goods".to_owned(),
                "place counter dimensions do not match goods",
            ));
        }
        last.produced_value_milli = last
            .produced
            .iter()
            .zip(prices)
            .map(|(units, price)| u128::from(*units) * u128::from(*price))
            .collect();
        last.used_by_pops_value_milli = last
            .used_by_pops
            .iter()
            .zip(prices)
            .map(|(units, price)| u128::from(*units) * u128::from(*price))
            .collect();
    }
    counters.last = last;
    counters.current = rollover_prices.map_or_else(MonthlyCounters::default, |prices| {
        MonthlyCounters::for_goods(prices.len())
    });
    counters.closed = true;
    Ok(())
}

impl MonthlyCounters {
    fn for_goods(goods: usize) -> Self {
        Self {
            produced: vec![0; goods],
            used_by_pops: vec![0; goods],
            lost: vec![0; goods],
            produced_value_milli: vec![0; goods],
            used_by_pops_value_milli: vec![0; goods],
            ..Self::default()
        }
    }
}

impl CounterBuckets {
    fn for_place(goods: usize) -> Self {
        Self {
            current: MonthlyCounters::for_goods(goods),
            last: MonthlyCounters::for_goods(goods),
            closed: false,
        }
    }
}

fn prepare_place_counters(counters: &mut CounterBuckets, goods: usize) -> Result<(), DataError> {
    for bucket in [&mut counters.current, &mut counters.last] {
        for values in [
            &mut bucket.produced,
            &mut bucket.used_by_pops,
            &mut bucket.lost,
        ] {
            if values.is_empty() {
                values.resize(goods, 0);
            } else if values.len() != goods {
                return Err(missing_data(
                    "counters.goods".to_owned(),
                    "place counter dimensions do not match goods",
                ));
            }
        }
        for values in [
            &mut bucket.produced_value_milli,
            &mut bucket.used_by_pops_value_milli,
        ] {
            if values.is_empty() {
                values.resize(goods, 0);
            } else if values.len() != goods {
                return Err(missing_data(
                    "counters.goods".to_owned(),
                    "place value counter dimensions do not match goods",
                ));
            }
        }
    }
    Ok(())
}

fn add_counter_units(
    values: &mut Vec<u64>,
    goods: usize,
    index: usize,
    amount: u64,
    path: &str,
) -> Result<(), DataError> {
    if values.is_empty() {
        values.resize(goods, 0);
    }
    let value = values
        .get_mut(index)
        .ok_or_else(|| missing_data(path.to_owned(), "good index is outside counters"))?;
    *value = value
        .checked_add(amount)
        .ok_or_else(|| missing_data(path.to_owned(), "whole unit counter overflow"))?;
    Ok(())
}

fn encode_counters(counters: &CounterBuckets, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    for bucket in [&counters.current, &counters.last] {
        bucket.births.encode(out)?;
        bucket.deaths.encode(out)?;
        bucket.losses.encode(out)?;
        bucket.rent_paid_milli.encode(out)?;
        bucket.rent_short_milli.encode(out)?;
    }
    counters.closed.encode(out)?;
    Ok(())
}

fn encode_place_counters(
    counters: &CounterBuckets,
    out: &mut Vec<u8>,
) -> Result<(), CanonicalError> {
    encode_counters(counters, out)?;
    for bucket in [&counters.current, &counters.last] {
        bucket.produced.encode(out)?;
        bucket.used_by_pops.encode(out)?;
        bucket.lost.encode(out)?;
        encode_u128_values(&bucket.produced_value_milli, out)?;
        encode_u128_values(&bucket.used_by_pops_value_milli, out)?;
    }
    Ok(())
}

fn encode_u128_values(values: &[u128], out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    encode_length(values.len(), out)?;
    for value in values {
        u64::try_from(*value & u128::from(u64::MAX))
            .unwrap_or_default()
            .encode(out)?;
        u64::try_from(*value >> 64)
            .unwrap_or_default()
            .encode(out)?;
    }
    Ok(())
}

fn decode_u128_values(input: &mut Reader<'_>) -> Result<Vec<u128>, CanonicalError> {
    let length = input.length()?;
    let mut values = Vec::with_capacity(length);
    for _ in 0..length {
        let low = u64::decode(input)?;
        let high = u64::decode(input)?;
        values.push(u128::from(low) | (u128::from(high) << 64));
    }
    Ok(values)
}

fn decode_counters(input: &mut Reader<'_>) -> Result<CounterBuckets, CanonicalError> {
    let current = MonthlyCounters {
        births: u64::decode(input)?,
        deaths: u64::decode(input)?,
        losses: u64::decode(input)?,
        rent_paid_milli: u64::decode(input)?,
        rent_short_milli: u64::decode(input)?,
        ..MonthlyCounters::default()
    };
    let last = MonthlyCounters {
        births: u64::decode(input)?,
        deaths: u64::decode(input)?,
        losses: u64::decode(input)?,
        rent_paid_milli: u64::decode(input)?,
        rent_short_milli: u64::decode(input)?,
        ..MonthlyCounters::default()
    };
    let closed = bool::decode(input)?;
    Ok(CounterBuckets {
        current,
        last,
        closed,
    })
}

fn decode_place_counters(input: &mut Reader<'_>) -> Result<CounterBuckets, CanonicalError> {
    let mut counters = decode_counters(input)?;
    for bucket in [&mut counters.current, &mut counters.last] {
        bucket.produced = Vec::<u64>::decode(input)?;
        bucket.used_by_pops = Vec::<u64>::decode(input)?;
        bucket.lost = Vec::<u64>::decode(input)?;
        bucket.produced_value_milli = decode_u128_values(input)?;
        bucket.used_by_pops_value_milli = decode_u128_values(input)?;
    }
    Ok(counters)
}

/// One population in the deterministic initial state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PopState {
    /// Population type key.
    pub kind: String,
    /// Culture key.
    pub culture: String,
    /// Religion key.
    pub religion: String,
    /// Stored literacy in per-mille; unchanged by M9 except people-weighted movement.
    pub literacy_pm: u16,
    /// Authoritative worker count.
    pub workers: u64,
    /// Authoritative dependent count.
    pub dependents: u64,
    /// Initial employment by building index.
    pub employment: Vec<Employment>,
    /// Goods stock in rules order.
    pub stock: Vec<u64>,
    /// Saved daily food, needs and satisfaction state.
    pub consumption: PopConsumption,
    /// Display-only monthly event counters.
    pub counters: CounterBuckets,
    /// Whether the pop's store was worth less than its rent at the last monthly rent pass: the
    /// result of that pass, which the next first-of-month promotes into `housed_last`.
    pub short_on_rent: bool,
    /// Whether the pop is housed last in the spare step this month (rent-r1 rule 5): last month's
    /// rent result, fixed on the 1st before the monthly fill, so the monthly fill and every live
    /// housing query until the next 1st apply the same priority.
    pub housed_last: bool,
    /// The pop's genetic state: baselines, haplogroup shares and trait rates (CK-361).
    pub genetics: PopGenetics,
    /// Character births committed since the last monthly step, which that step counts against its own births (CK-364). Saved.
    pub pending_births: u64,
    /// Character deaths committed since the last monthly step, which that step counts against its own deaths (CK-364). Saved.
    pub pending_deaths: u64,
}

/// Saved balances and carries used by the noon eating system.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopConsumption {
    /// Per-good hunger already opened but not eaten, in rules order.
    pub pantry_hunger_centi: Vec<u64>,
    /// Unconsumed tier balances, in hundredths of a person-day.
    pub necessities_centi: u64,
    /// Unconsumed niceties balance, in hundredths of a person-day.
    pub niceties_centi: u64,
    /// Unconsumed luxuries balance, in hundredths of a person-day.
    pub luxuries_centi: u64,
    /// Per-good ticking carries, in millionths of a unit, in rules order.
    pub ticking_carries: Vec<u64>,
    /// Slowly moving tier fulfilment values in per-mille.
    pub necessities_pm: u16,
    /// Niceties fulfilment in per-mille.
    pub niceties_pm: u16,
    /// Luxuries fulfilment in per-mille.
    pub luxuries_pm: u16,
    /// Satisfaction, bounded to −100..=100.
    pub satisfaction: i16,
    /// Signed remainder carries for the three tier drifts and monthly satisfaction drift.
    pub drift_carries: [i64; 4],
    /// Exact monthly birth-rate remainder.
    pub birth_carry: u64,
    /// Exact monthly worker-death remainder.
    pub worker_death_carry: u64,
    /// Exact monthly dependent-death remainder.
    pub dependent_death_carry: u64,
    /// Fuel already opened, in thousandths.
    pub hearth_milli: u64,
    /// Fuel needed and burned since the last monthly step.
    pub fuel_need_month: u64,
    /// Fuel burned since the last monthly step.
    pub fuel_burned_month: u64,
    /// Exact monthly fuel-death remainder.
    pub fuel_death_carry: u64,
    /// Signed fractional remainder for monthly dependent-share drift.
    pub dependent_drift_carry: i64,
}

/// Completed paydays at which a level is mature: it earns its seasonal goods in full.
pub const MATURE_PAYDAYS: u8 = 13;

/// One level of a producing building: the method it works by, the ground it stands on and its completed paydays.
///
/// A level cut at the start has no record, and a storehouse's levels (kinds of storage) have none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelRecord {
    /// The level's place in the order the start placed the building's levels, from 0. It never changes.
    pub number: u32,
    /// Index of the method in the building definition's methods.
    pub method: u32,
    /// The terrain the level stands on: a land terrain for a usable-land method, forest for a forest method, bog for a bog method
    /// and nothing for a sea method.
    pub ground: Option<String>,
    /// Paydays completed on this method without a break, from 0 to [`MATURE_PAYDAYS`].
    pub paydays: u8,
    /// Unpaid seasonal work, in 144,000ths of a good, keyed by good ID.
    pub unpaid_work: BTreeMap<String, bigint::Big>,
    /// The calendar year of the level's last switch of method, and none for a level that has never switched. A level that
    /// switched in a year does not switch again in that year (owner, S6).
    pub last_switch_year: Option<u16>,
}

/// A building in the initial economy state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildingState {
    /// Building type key.
    pub kind: String,
    /// Total authored levels after resource cuts.
    pub levels: u32,
    /// Levels per method in method order. For a producing building these are the counts of [`Self::level_records`] by method (a load
    /// refuses a save where they differ); a storehouse's are its kinds of storage.
    pub method_levels: Vec<u32>,
    /// A record for each level of a producing building, in level-number order; empty for a storehouse.
    pub level_records: Vec<LevelRecord>,
    /// Herd counts in species order.
    pub herds: Vec<u64>,
    /// Employment by pop index.
    pub employment: Vec<Employment>,
    /// Goods stock in rules order.
    pub stock: Vec<u64>,
    /// Storehouse claims in pop order plus the communal account.
    /// Present only for storehouses; inner vectors use goods order.
    pub storehouse_accounts: Option<StorehouseAccounts>,
    /// Fractional seasonal production carries, in method and good order, in twelfths of the old unit (the denominator is twelve
    /// times the old one, so a carry can pass 64 bits).
    pub production_carries: Vec<Vec<u128>>,
    /// Sum of per-level product suitability in per-mille, by method and good, derived from the level records' grounds.
    pub method_suitability: Vec<Vec<u64>>,
    /// Fractional herd births per species, in 1e12ths of an animal.
    pub herd_birth_carries: Vec<u64>,
    /// Fractional herd deaths per species, in 12,000ths of an animal.
    pub herd_death_carries: Vec<u64>,
    /// Fractional dairy and wool product carries in good order.
    pub herd_product_carries: Vec<u64>,
    /// Sum of per-level grazing suitability in species order, derived from the level records' grounds.
    pub herd_grazing_sums: Vec<u64>,
    /// Fractional wage shares in millionths, in good order.
    pub wage_carries: Vec<u64>,
    /// Whole wage units awaiting workers, in good order.
    pub pay_carries: Vec<u64>,
    /// Storehouse account fee carries in pop order, then communal account.
    pub fee_carries: Vec<Vec<u64>>,
    /// Posted wages in thousandths of a value unit per paid head, by calendar month.
    pub wage_history: Vec<u64>,
    /// Paid heads at each calendar month's payday, zero until the slot is written.
    pub paid_history: Vec<u64>,
    /// Year of the payday that last wrote each calendar-month slot, zero while seeded.
    pub paid_years: Vec<u16>,
}

impl BuildingState {
    /// The floor of the twelve saved monthly wages, or zero for malformed state.
    #[must_use]
    pub fn posted_wage(&self) -> u64 {
        if self.wage_history.len() != WAGE_HISTORY_MONTHS {
            return 0;
        }
        let total = self
            .wage_history
            .iter()
            .map(|wage| u128::from(*wage))
            .sum::<u128>();
        u64::try_from(total / WAGE_HISTORY_MONTHS as u128).unwrap_or(u64::MAX)
    }
}

/// Per-good claims against a storehouse's physical stock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorehouseAccounts {
    /// Accounts for each place pop, including zero accounts for ineligible pops.
    pub pops: Vec<Vec<u64>>,
    /// The owners' communal account.
    pub communal: Vec<u64>,
}

fn empty_storehouse_accounts(rules: &Rules, pop_count: usize) -> StorehouseAccounts {
    let empty = vec![0; rules.goods.len()];
    StorehouseAccounts {
        pops: vec![empty.clone(); pop_count],
        communal: empty,
    }
}

fn storehouse_physical_stock(rules: &Rules, accounts: &StorehouseAccounts) -> Vec<u64> {
    (0..rules.goods.len())
        .map(|good| {
            accounts
                .pops
                .iter()
                .map(|account| account[good])
                .sum::<u64>()
                + accounts.communal[good]
        })
        .collect()
}

fn storehouse_group(good_id: &str) -> Option<usize> {
    match good_id {
        "oats" | "barley" | "peas_beans" => Some(0),
        "dairy" | "beef" | "pork" | "mutton" | "fish" | "apples" | "vegetables" => Some(1),
        "ale" | "mead" | "wine" => Some(2),
        _ => None,
    }
}

fn volume_used(rules: &Rules, stock: &[u64], group: Option<usize>) -> Result<u128, DataError> {
    rules
        .goods
        .iter()
        .zip(stock)
        .try_fold(0_u128, |total, (good, units)| {
            if good.stacked_outside
                || group.is_some_and(|wanted| storehouse_group(&good.id) != Some(wanted))
            {
                return Ok(total);
            }
            total
                .checked_add(u128::from(*units) * u128::from(good.volume_ml))
                .ok_or_else(|| missing_data("storage.volume".to_owned(), "stored volume overflow"))
        })
}

fn worker_count(building: &BuildingState) -> Result<u64, DataError> {
    building
        .employment
        .iter()
        .try_fold(0_u64, |total, employment| {
            total.checked_add(employment.workers).ok_or_else(|| {
                missing_data("employment.workers".to_owned(), "worker count overflow")
            })
        })
}

/// Volume capacity of a farm or holdings building's store, in millilitres.
/// Only employed workers count; owners do not occupy paid worker capacity.
/// Other building kinds have no worker-scaled building store capacity.
///
/// # Errors
///
/// Returns a data error if the building's worker count or capacity overflows.
pub fn store_capacity_ml(building: &BuildingState) -> Result<u128, DataError> {
    let workers = u128::from(worker_count(building)?);
    let per_worker = match building.kind.as_str() {
        "farm" => 100_000,
        // The demesne's store is the holdings' store (CK-359).
        "holdings" | "demesne" => 1_170_000,
        _ => 0,
    };
    workers
        .checked_mul(per_worker)
        .ok_or_else(|| missing_data("storage.capacity".to_owned(), "building capacity overflow"))
}

fn building_good_cap(
    rules: &Rules,
    building: &BuildingState,
    good_index: usize,
) -> Result<u64, DataError> {
    let good = rules
        .goods
        .get(good_index)
        .ok_or_else(|| missing_data("storage.good".to_owned(), "good index is outside rules"))?;
    let days = match building.kind.as_str() {
        "farm" => 60_u128,
        "holdings" | "demesne" => 730,
        _ => 0,
    };
    let cap = u128::from(good.pop_limit_milli)
        .checked_mul(u128::from(worker_count(building)?))
        .and_then(|value| value.checked_mul(days))
        .ok_or_else(|| missing_data("storage.cap".to_owned(), "building good cap overflow"))?
        / (450 * 1000);
    u64::try_from(cap)
        .map_err(|_| missing_data("storage.cap".to_owned(), "building good cap exceeds u64"))
}

fn storehouse_group_levels(building: &BuildingState, group: usize) -> Result<u32, DataError> {
    building.method_levels.get(group).copied().ok_or_else(|| {
        missing_data(
            "storehouse.levels".to_owned(),
            "storehouse kind levels are missing",
        )
    })
}

fn storehouse_good_cap(
    rules: &Rules,
    building: &BuildingState,
    good_index: usize,
) -> Result<u64, DataError> {
    let good = rules
        .goods
        .get(good_index)
        .ok_or_else(|| missing_data("storehouse.good".to_owned(), "good index is outside rules"))?;
    let Some(group) = storehouse_group(&good.id) else {
        return Ok(0);
    };
    let levels = u128::from(storehouse_group_levels(building, group)?);
    let cap = u128::from(good.pop_limit_milli)
        .checked_mul(50)
        .and_then(|value| value.checked_mul(levels))
        .ok_or_else(|| missing_data("storehouse.cap".to_owned(), "good cap overflow"))?
        / 1000;
    u64::try_from(cap)
        .map_err(|_| missing_data("storehouse.cap".to_owned(), "good cap exceeds u64"))
}

fn room_by_cap_and_volume(
    rules: &Rules,
    stock: &[u64],
    good_index: usize,
    good_cap: u64,
    capacity_ml: u128,
    group: Option<usize>,
    offered: u64,
) -> Result<u64, DataError> {
    let good = rules
        .goods
        .get(good_index)
        .ok_or_else(|| missing_data("storage.good".to_owned(), "good index is outside rules"))?;
    let stored_good = *stock.get(good_index).ok_or_else(|| {
        missing_data(
            "storage.stock".to_owned(),
            "stock dimensions do not match goods",
        )
    })?;
    let cap_room = good_cap.saturating_sub(stored_good);
    let used_volume = volume_used(rules, stock, group)?;
    let volume_room = capacity_ml.saturating_sub(used_volume);
    let by_volume = if good.stacked_outside || good.volume_ml == 0 {
        offered
    } else {
        u64::try_from((volume_room / u128::from(good.volume_ml)).min(u128::from(u64::MAX)))
            .expect("bounded by u64::MAX")
    };
    Ok(offered.min(cap_room).min(by_volume))
}

/// How much of `offered` a pop's store (its stock `stock`, for `people` people) takes: the good's per-person limit and the store's
/// volume, as every receipt checks them.
fn pop_store_limits(
    rules: &Rules,
    people: u64,
    good_index: usize,
) -> Result<(u64, u128), DataError> {
    let good = rules
        .goods
        .get(good_index)
        .ok_or_else(|| missing_data("pop.stock".to_owned(), "good index is outside rules"))?;
    let good_cap = u64::try_from(u128::from(good.pop_limit_milli) * u128::from(people) / 1000)
        .map_err(|_| missing_data("pop.cap".to_owned(), "good cap exceeds u64"))?;
    let capacity = u128::from(people)
        .checked_mul(720_000)
        .ok_or_else(|| missing_data("pop.capacity".to_owned(), "volume capacity overflow"))?;
    Ok((good_cap, capacity))
}

fn pop_store_accepts(
    rules: &Rules,
    people: u64,
    stock: &[u64],
    good_index: usize,
    offered: u64,
) -> Result<u64, DataError> {
    let (good_cap, capacity) = pop_store_limits(rules, people, good_index)?;
    room_by_cap_and_volume(rules, stock, good_index, good_cap, capacity, None, offered)
}

fn receive_pop(
    rules: &Rules,
    pop: &mut PopState,
    good_index: usize,
    offered: u64,
) -> Result<u64, DataError> {
    if pop.stock.len() != rules.goods.len() {
        return Err(missing_data(
            "pop.stock".to_owned(),
            "stock dimensions do not match goods",
        ));
    }
    let people = pop
        .workers
        .checked_add(pop.dependents)
        .ok_or_else(|| missing_data("pop.size".to_owned(), "population size overflow"))?;
    let accepted = pop_store_accepts(rules, people, &pop.stock, good_index, offered)?;
    pop.stock[good_index] = pop.stock[good_index]
        .checked_add(accepted)
        .ok_or_else(|| missing_data("pop.stock".to_owned(), "stock addition overflow"))?;
    Ok(offered - accepted)
}

fn receive_building(
    rules: &Rules,
    building: &mut BuildingState,
    good_index: usize,
    offered: u64,
) -> Result<u64, DataError> {
    if building.stock.len() != rules.goods.len() {
        return Err(missing_data(
            "building.stock".to_owned(),
            "stock dimensions do not match goods",
        ));
    }
    let capacity = store_capacity_ml(building)?;
    let cap = building_good_cap(rules, building, good_index)?;
    let accepted = room_by_cap_and_volume(
        rules,
        &building.stock,
        good_index,
        cap,
        capacity,
        None,
        offered,
    )?;
    building.stock[good_index] = building.stock[good_index]
        .checked_add(accepted)
        .ok_or_else(|| missing_data("building.stock".to_owned(), "stock addition overflow"))?;
    Ok(offered - accepted)
}

fn receive_storehouse(
    rules: &Rules,
    building: &mut BuildingState,
    account_pop: Option<usize>,
    good_index: usize,
    offered: u64,
) -> Result<u64, DataError> {
    let group = rules
        .goods
        .get(good_index)
        .and_then(|good| storehouse_group(&good.id));
    let Some(group) = group else {
        return Ok(offered);
    };
    let levels = u128::from(storehouse_group_levels(building, group)?);
    let capacity = levels.checked_mul(36_000_000).ok_or_else(|| {
        missing_data("storehouse.capacity".to_owned(), "volume capacity overflow")
    })?;
    let cap = storehouse_good_cap(rules, building, good_index)?;
    let accounts = building.storehouse_accounts.as_mut().ok_or_else(|| {
        missing_data(
            "storehouse.accounts".to_owned(),
            "storehouse account table is missing",
        )
    })?;
    if accounts.communal.len() != rules.goods.len()
        || accounts
            .pops
            .iter()
            .any(|account| account.len() != rules.goods.len())
        || building.stock.len() != rules.goods.len()
    {
        return Err(missing_data(
            "storehouse.accounts".to_owned(),
            "account dimensions do not match goods",
        ));
    }
    let physical = storehouse_physical_stock_checked(rules, accounts)?;
    if physical != building.stock {
        return Err(missing_data(
            "storehouse.stock".to_owned(),
            "physical stock differs from account totals",
        ));
    }
    let accepted = room_by_cap_and_volume(
        rules,
        &building.stock,
        good_index,
        cap,
        capacity,
        Some(group),
        offered,
    )?;
    if accepted == 0 {
        return Ok(offered);
    }
    match account_pop {
        Some(pop) => {
            let account = accounts.pops.get_mut(pop).ok_or_else(|| {
                missing_data(
                    "storehouse.accounts".to_owned(),
                    "source pop account is missing",
                )
            })?;
            account[good_index] = account[good_index].checked_add(accepted).ok_or_else(|| {
                missing_data("storehouse.accounts".to_owned(), "pop account overflow")
            })?;
        }
        None => {
            accounts.communal[good_index] = accounts.communal[good_index]
                .checked_add(accepted)
                .ok_or_else(|| {
                    missing_data(
                        "storehouse.accounts".to_owned(),
                        "communal account overflow",
                    )
                })?;
        }
    }
    building.stock[good_index] = building.stock[good_index]
        .checked_add(accepted)
        .ok_or_else(|| missing_data("storehouse.stock".to_owned(), "physical stock overflow"))?;
    Ok(offered - accepted)
}

fn storehouse_physical_stock_checked(
    rules: &Rules,
    accounts: &StorehouseAccounts,
) -> Result<Vec<u64>, DataError> {
    let mut stock = vec![0_u64; rules.goods.len()];
    for account in accounts
        .pops
        .iter()
        .chain(std::iter::once(&accounts.communal))
    {
        if account.len() != rules.goods.len() {
            return Err(missing_data(
                "storehouse.accounts".to_owned(),
                "account dimensions do not match goods",
            ));
        }
        for (good, amount) in account.iter().enumerate() {
            stock[good] = stock[good].checked_add(*amount).ok_or_else(|| {
                missing_data("storehouse.stock".to_owned(), "physical stock overflow")
            })?;
        }
    }
    Ok(stock)
}

fn alms_order(
    rules: &Rules,
    pops: &[PopState],
    storehouse: Option<&BuildingState>,
) -> Result<Vec<usize>, DataError> {
    let mut order = Vec::new();
    for (index, _) in pops
        .iter()
        .enumerate()
        .filter(|(_, pop)| pop.kind == "labourer")
    {
        order.push(index);
    }
    let mut hungry = Vec::<(u128, usize)>::new();
    for (index, pop) in pops
        .iter()
        .enumerate()
        .filter(|(_, pop)| pop.kind != "labourer")
    {
        let people = pop.workers.checked_add(pop.dependents).ok_or_else(|| {
            missing_data("alms.population".to_owned(), "population size overflow")
        })?;
        if people == 0 {
            continue;
        }
        let mut food = 0_u128;
        for (good_index, good) in rules.goods.iter().enumerate() {
            if good.hunger_centi == 0 {
                continue;
            }
            let held = u128::from(pop.stock.get(good_index).copied().unwrap_or(0))
                + storehouse
                    .and_then(|building| building.storehouse_accounts.as_ref())
                    .and_then(|accounts| accounts.pops.get(index))
                    .and_then(|account| account.get(good_index))
                    .copied()
                    .map_or(0, u128::from);
            food = food
                .checked_add(
                    held.checked_mul(u128::from(good.hunger_centi))
                        .ok_or_else(|| {
                            missing_data("alms.food".to_owned(), "food value overflow")
                        })?,
                )
                .ok_or_else(|| missing_data("alms.food".to_owned(), "food total overflow"))?;
        }
        let need = u128::from(people) * 100;
        // Compare exact food days, not a rounded display value.
        if food < need * 60 {
            hungry.push((food / need, index));
        }
    }
    hungry.sort_unstable();
    order.extend(hungry.into_iter().map(|(_, index)| index));
    Ok(order)
}

fn receive_storehouse_at(
    rules: &Rules,
    buildings: &mut [BuildingState],
    storehouse_index: usize,
    account_pop: Option<usize>,
    good_index: usize,
    offered: u64,
) -> Result<u64, DataError> {
    let building = buildings
        .get_mut(storehouse_index)
        .ok_or_else(|| missing_data("storehouse".to_owned(), "storehouse index is missing"))?;
    receive_storehouse(rules, building, account_pop, good_index, offered)
}

// Pass a pop's surplus to its account, eligible buildings and other pops.
fn route_pop_surplus(
    rules: &Rules,
    pops: &mut [PopState],
    buildings: &mut [BuildingState],
    giver: usize,
    good: usize,
    mut remaining: u64,
) -> Result<u64, DataError> {
    if remaining == 0 {
        return Ok(0);
    }
    let storehouse = buildings
        .iter()
        .position(|b| b.storehouse_accounts.is_some());
    if let Some(index) = storehouse {
        remaining = receive_storehouse_at(rules, buildings, index, Some(giver), good, remaining)?;
    }
    let owns = buildings
        .iter()
        .any(|b| b.employment.get(giver).is_some_and(|e| e.owners > 0));
    let mut tried = vec![false; buildings.len()];
    for church in [false, true] {
        if remaining == 0 {
            return Ok(0);
        }
        let mut indexes = Vec::new();
        let mut weights = Vec::new();
        let mut caps = Vec::new();
        for (index, building) in buildings.iter().enumerate() {
            let weight = if church {
                if tried[index]
                    || !rules
                        .buildings
                        .iter()
                        .any(|b| b.id == building.kind && b.church)
                {
                    0
                } else {
                    u64::from(building.levels)
                }
            } else {
                building
                    .employment
                    .get(giver)
                    .map_or(0, |e| if owns { e.owners } else { e.workers })
            };
            if weight == 0 {
                continue;
            }
            tried[index] = true;
            let mut copy = building.clone();
            let left = if copy.storehouse_accounts.is_some() {
                receive_storehouse(rules, &mut copy, None, good, remaining)?
            } else {
                receive_building(rules, &mut copy, good, remaining)?
            };
            indexes.push(index);
            weights.push(weight);
            caps.push(remaining - left);
        }
        let shares = split_with_caps(remaining, &weights, &caps)
            .map_err(|error| missing_data("surplus.split".to_owned(), error))?;
        for (index, share) in indexes.into_iter().zip(shares) {
            let building = &mut buildings[index];
            let left = if building.storehouse_accounts.is_some() {
                receive_storehouse(rules, building, None, good, share)?
            } else {
                receive_building(rules, building, good, share)?
            };
            remaining -= share - left;
        }
    }
    if remaining == 0 {
        return Ok(0);
    }
    let order = alms_order(rules, pops, storehouse.and_then(|i| buildings.get(i)))?;
    for index in order {
        if index != giver {
            remaining = receive_pop(rules, &mut pops[index], good, remaining)?;
        }
    }
    Ok(remaining)
}

#[expect(
    clippy::too_many_arguments,
    reason = "The overflow route updates the source, receipts, counters and returned ledger atomically."
)]
fn settle_overflow(
    rules: &Rules,
    pops: &mut [PopState],
    buildings: &mut [BuildingState],
    source_building: usize,
    good_index: usize,
    contributions: &[(Option<usize>, u64)],
    place_counters: &mut CounterBuckets,
    ledger: &mut PlaceProductionLedger,
) -> Result<(), DataError> {
    let storehouse_index = buildings
        .iter()
        .position(|building| building.storehouse_accounts.is_some());
    for (account_pop, amount) in contributions {
        let mut remaining = *amount;
        if let Some(giver) = account_pop {
            remaining = route_pop_surplus(rules, pops, buildings, *giver, good_index, remaining)?;
        } else {
            {
                let source = buildings.get_mut(source_building).ok_or_else(|| {
                    missing_data("payday.building".to_owned(), "source building is missing")
                })?;
                remaining = receive_building(rules, source, good_index, remaining)?;
            }
            if remaining == 0 {
                continue;
            }
            let priorities = alms_order(
                rules,
                pops,
                storehouse_index.and_then(|index| buildings.get(index)),
            )?;
            for pop_index in priorities {
                if remaining == 0 {
                    break;
                }
                let pop = pops.get_mut(pop_index).ok_or_else(|| {
                    missing_data("alms.population".to_owned(), "pop index is missing")
                })?;
                remaining = receive_pop(rules, pop, good_index, remaining)?;
            }
            if remaining == 0 {
                continue;
            }
            if let Some(index) = storehouse_index {
                remaining = receive_storehouse_at(
                    rules,
                    buildings,
                    index,
                    *account_pop,
                    good_index,
                    remaining,
                )?;
            }
        }
        if remaining > 0 {
            place_counters.current.losses = place_counters
                .current
                .losses
                .checked_add(remaining)
                .ok_or_else(|| {
                    missing_data("counters.losses".to_owned(), "place loss counter overflow")
                })?;
            add_counter_units(
                &mut place_counters.current.lost,
                rules.goods.len(),
                good_index,
                remaining,
                "counters.lost",
            )?;
            if let Some(pop_index) = account_pop {
                let pop = pops.get_mut(*pop_index).ok_or_else(|| {
                    missing_data("payday.source".to_owned(), "source pop index is missing")
                })?;
                pop.counters.current.losses = pop
                    .counters
                    .current
                    .losses
                    .checked_add(remaining)
                    .ok_or_else(|| {
                        missing_data("counters.losses".to_owned(), "pop loss counter overflow")
                    })?;
            }
        }
        ledger.lost[good_index] = ledger.lost[good_index]
            .checked_add(remaining)
            .ok_or_else(|| missing_data("ledger.loss".to_owned(), "loss total overflow"))?;
    }
    Ok(())
}

/// A location's economy state and saved local prices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceState {
    /// Scenario-local location key.
    pub id: String,
    /// Pop slots are append-only: existing indices never move or disappear.
    pub pops: Vec<PopState>,
    /// Buildings in authored order.
    pub buildings: Vec<BuildingState>,
    /// Saved local prices in good order, represented in thousandths.
    pub prices_milli: Vec<u64>,
    /// Saved area prepared for construction, in square metres.
    pub prepared_m2: u64,
    /// Saved development in thousandths.
    pub development_pm: u32,
    /// Signed daily price-drift carries in good order, with denominator 1000.
    pub price_drift_carries: Vec<i64>,
    /// Orders from the latest daily price step; derived for display and excluded from saved state.
    pub last_market_orders: Option<DailyMarketOrders>,
    /// Display-only monthly event counters.
    pub counters: CounterBuckets,
}

/// One pop's derived allocation across the place's housing classes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopHousing {
    /// People in basic building housing, including their share of overflow.
    pub basic: u64,
    /// People in makeshift natural shelter, including their share of overflow.
    pub makeshift: u64,
    /// The pop's crowding in per-mille, or `None` when it has no people.
    pub crowding_pm: Option<u64>,
}

/// Recomputed housing allocation; it is deliberately not part of saved state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HousingFill {
    /// Per-pop class allocations in place pop order.
    pub pops: Vec<PopHousing>,
    /// Occupants by building, including attributed basic overflow.
    pub building_occupants: Vec<u64>,
    /// People of each pop housed in each building's basic housing, `housed[pop][building]`:
    /// the workers-first place, the spare place and the overflow, summed. Each building's column
    /// sums to its `building_occupants` entry and each pop's row to its `basic` count. This is what
    /// the rent pass reads to know who lives where.
    pub housed: Vec<Vec<u64>>,
    /// Total basic housing capacity.
    pub basic_capacity: u64,
    /// Natural makeshift shelter capacity.
    pub makeshift_capacity: u64,
    /// People occupying basic housing, including overflow.
    pub basic_occupants: u64,
    /// People occupying makeshift shelter, including overflow.
    pub makeshift_occupants: u64,
    /// Place crowding in per-mille.
    pub crowding_pm: u64,
}

/// Recompute one place's housing without changing or retaining any saved state.
///
/// `shelter` is derived from the scenario catchment and is not serialized.
///
/// # Errors
/// Returns a named data error for malformed housing rules, inconsistent employment, or arithmetic overflow.
#[expect(
    clippy::too_many_lines,
    reason = "The numbered allocation phases mirror the approved housing-fill spec and keep its conservation invariants visible in order."
)]
pub fn housing_fill(
    rules: &Rules,
    place: &PlaceState,
    shelter: u64,
) -> Result<HousingFill, DataError> {
    let pop_count = place.pops.len();
    let building_count = place.buildings.len();
    let mut building_capacity = Vec::with_capacity(building_count);
    let mut capacities = [0_u64; 2]; // basic, makeshift
    for building in &place.buildings {
        let class = rules.housing_class.get(&building.kind).ok_or_else(|| {
            missing_data(
                format!("housing_class.{}", building.kind),
                "housing class is missing",
            )
        })?;
        if class != "basic" {
            return Err(missing_data(
                format!("housing_class.{}", building.kind),
                "this slice supports only basic building housing",
            ));
        }
        let per_level = *rules.housing_per_level.get(&building.kind).ok_or_else(|| {
            missing_data(
                format!("housing_per_level.{}", building.kind),
                "housing capacity is missing",
            )
        })?;
        let capacity = u64::from(building.levels)
            .checked_mul(u64::from(per_level))
            .ok_or_else(|| missing_data("housing.capacity".to_owned(), "capacity overflow"))?;
        capacities[0] = capacities[0]
            .checked_add(capacity)
            .ok_or_else(|| missing_data("housing.basic".to_owned(), "capacity overflow"))?;
        building_capacity.push(capacity);
    }
    capacities[1] = shelter;

    let mut sizes = Vec::with_capacity(pop_count);
    let mut employed_by_building = vec![vec![0_u64; pop_count]; building_count];
    for (building_index, building) in place.buildings.iter().enumerate() {
        if building.employment.len() != pop_count {
            return Err(missing_data(
                format!("places.{}.buildings.{}.employment", place.id, building.kind),
                "employment dimensions do not match place pops",
            ));
        }
        for (pop_index, employment) in building.employment.iter().enumerate() {
            employed_by_building[building_index][pop_index] = employment
                .owners
                .checked_add(employment.workers)
                .ok_or_else(|| {
                    missing_data("housing.employment".to_owned(), "employment total overflow")
                })?;
        }
    }
    let mut claims = vec![vec![0_u64; pop_count]; building_count];
    let mut sizes_again = Vec::with_capacity(pop_count);
    for (pop_index, pop) in place.pops.iter().enumerate() {
        if pop.employment.len() != building_count {
            return Err(missing_data(
                format!("places.{}.pops[{pop_index}].employment", place.id),
                "employment dimensions do not match place buildings",
            ));
        }
        let size = pop
            .workers
            .checked_add(pop.dependents)
            .ok_or_else(|| missing_data("housing.population".to_owned(), "pop size overflow"))?;
        sizes.push(size);
        sizes_again.push(size);
        let mut employed = 0_u64;
        let mut weights = Vec::with_capacity(building_count);
        for (building_index, building_employment) in employed_by_building.iter().enumerate() {
            let employment = building_employment[pop_index];
            employed = employed.checked_add(employment).ok_or_else(|| {
                missing_data("housing.employment".to_owned(), "pop employment overflow")
            })?;
            let mirrored = pop.employment[building_index]
                .owners
                .checked_add(pop.employment[building_index].workers)
                .ok_or_else(|| {
                    missing_data("housing.employment".to_owned(), "employment total overflow")
                })?;
            if mirrored != employment {
                return Err(missing_data(
                    format!("places.{}.pops[{pop_index}].employment", place.id),
                    "pop and building employment disagree",
                ));
            }
            weights.push(employment);
        }
        if employed > pop.workers {
            return Err(missing_data(
                format!("places.{}.pops[{pop_index}].employment", place.id),
                "employed heads exceed pop workers",
            ));
        }
        let claim_total = if pop.workers == 0 {
            0
        } else {
            let numerator = u128::from(employed)
                .checked_mul(u128::from(size))
                .ok_or_else(|| missing_data("housing.claim".to_owned(), "claim overflow"))?;
            u64::try_from(numerator / u128::from(pop.workers))
                .map_err(|_| missing_data("housing.claim".to_owned(), "claim exceeds u64"))?
        };
        let split = largest_remainder(claim_total, &weights)
            .map_err(|problem| missing_data("housing.claim".to_owned(), problem))?;
        for (building_index, claim) in split.into_iter().enumerate() {
            claims[building_index][pop_index] = claim;
        }
    }

    let mut pop_basic = vec![0_u64; pop_count];
    let mut housed = vec![vec![0_u64; building_count]; pop_count];
    let mut occupants = vec![0_u64; building_count];
    let mut spare = Vec::with_capacity(building_count);
    for building_index in 0..building_count {
        let awards = give(building_capacity[building_index], &claims[building_index])?;
        for (pop_index, award) in awards.into_iter().enumerate() {
            housed[pop_index][building_index] = award;
            pop_basic[pop_index] = pop_basic[pop_index]
                .checked_add(award)
                .ok_or_else(|| missing_data("housing.awards".to_owned(), "award overflow"))?;
            occupants[building_index] =
                occupants[building_index]
                    .checked_add(award)
                    .ok_or_else(|| {
                        missing_data("housing.occupancy".to_owned(), "occupancy overflow")
                    })?;
        }
        spare.push(building_capacity[building_index] - occupants[building_index]);
    }

    let mut unhoused = sizes
        .iter()
        .zip(&pop_basic)
        .map(|(size, basic)| {
            size.checked_sub(*basic)
                .ok_or_else(|| missing_data("housing.awards".to_owned(), "awards exceed pop"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let basic_spare = spare.iter().try_fold(0_u64, |sum, value| {
        sum.checked_add(*value)
            .ok_or_else(|| missing_data("housing.basic".to_owned(), "spare capacity overflow"))
    })?;
    // A pop that was short on last month's rent is housed last: the spare places go first to the
    // pops that are not flagged, by the usual largest-remainder award among them, and what is left
    // goes to the flagged pops by the same award. With no flagged pop this is the plain award.
    let flagged = place
        .pops
        .iter()
        .map(|pop| pop.housed_last)
        .collect::<Vec<_>>();
    let pop_order = (0..pop_count)
        .filter(|index| !flagged[*index])
        .chain((0..pop_count).filter(|index| flagged[*index]))
        .collect::<Vec<_>>();
    let masked = |wanted: bool| -> Vec<u64> {
        unhoused
            .iter()
            .zip(&flagged)
            .map(|(count, flag)| if *flag == wanted { *count } else { 0 })
            .collect()
    };
    let first = give(basic_spare, &masked(false))?;
    let first_total = first.iter().try_fold(0_u64, |sum, value| {
        sum.checked_add(*value)
            .ok_or_else(|| missing_data("housing.basic".to_owned(), "pooled total overflow"))
    })?;
    let second = give(basic_spare - first_total, &masked(true))?;
    let pooled = first
        .iter()
        .zip(&second)
        .map(|(first, second)| first + second)
        .collect::<Vec<_>>();
    for (pop_index, amount) in pooled.iter().copied().enumerate() {
        pop_basic[pop_index] = pop_basic[pop_index]
            .checked_add(amount)
            .ok_or_else(|| missing_data("housing.basic".to_owned(), "pop allocation overflow"))?;
        unhoused[pop_index] -= amount;
    }
    let pooled_total = pooled.iter().try_fold(0_u64, |sum, value| {
        sum.checked_add(*value)
            .ok_or_else(|| missing_data("housing.basic".to_owned(), "pooled total overflow"))
    })?;
    let pooled_by_building = largest_remainder(pooled_total, &spare)
        .map_err(|problem| missing_data("housing.basic".to_owned(), problem))?;
    let spare_cells = allocate_people(&pooled, &pooled_by_building, &pop_order)?;
    for (pop_index, row) in spare_cells.into_iter().enumerate() {
        for (building_index, amount) in row.into_iter().enumerate() {
            housed[pop_index][building_index] += amount;
        }
    }
    for (building_index, amount) in pooled_by_building.into_iter().enumerate() {
        occupants[building_index] = occupants[building_index]
            .checked_add(amount)
            .ok_or_else(|| missing_data("housing.occupancy".to_owned(), "occupancy overflow"))?;
        spare[building_index] -= amount;
    }

    let makeshift = give(shelter, &unhoused)?;
    let mut pop_makeshift = makeshift;
    for (pop_index, amount) in pop_makeshift.iter().copied().enumerate() {
        unhoused[pop_index] -= amount;
    }
    let remaining = unhoused.iter().try_fold(0_u64, |sum, value| {
        sum.checked_add(*value)
            .ok_or_else(|| missing_data("housing.overflow".to_owned(), "overflow total overflow"))
    })?;
    let total_capacity = capacities[0]
        .checked_add(capacities[1])
        .ok_or_else(|| missing_data("housing.capacity".to_owned(), "total capacity overflow"))?;
    if remaining > 0 && total_capacity > 0 {
        let overflow_by_class = largest_remainder(remaining, &capacities)
            .map_err(|problem| missing_data("housing.overflow".to_owned(), problem))?;
        let basic_overflow = largest_remainder(overflow_by_class[0], &unhoused)
            .map_err(|problem| missing_data("housing.overflow".to_owned(), problem))?;
        for (pop_index, amount) in basic_overflow.iter().copied().enumerate() {
            pop_basic[pop_index] = pop_basic[pop_index].checked_add(amount).ok_or_else(|| {
                missing_data(
                    "housing.overflow".to_owned(),
                    "basic overflow allocation overflow",
                )
            })?;
            unhoused[pop_index] -= amount;
        }
        let makeshift_overflow = largest_remainder(overflow_by_class[1], &unhoused)
            .map_err(|problem| missing_data("housing.overflow".to_owned(), problem))?;
        for (pop_index, amount) in makeshift_overflow.iter().copied().enumerate() {
            pop_makeshift[pop_index] =
                pop_makeshift[pop_index]
                    .checked_add(amount)
                    .ok_or_else(|| {
                        missing_data(
                            "housing.overflow".to_owned(),
                            "makeshift overflow allocation overflow",
                        )
                    })?;
            unhoused[pop_index] -= amount;
        }
        let building_overflow = largest_remainder(overflow_by_class[0], &building_capacity)
            .map_err(|problem| missing_data("housing.overflow".to_owned(), problem))?;
        let overflow_cells = allocate_people(&basic_overflow, &building_overflow, &pop_order)?;
        for (pop_index, row) in overflow_cells.into_iter().enumerate() {
            for (building_index, amount) in row.into_iter().enumerate() {
                housed[pop_index][building_index] += amount;
            }
        }
        for (building_index, amount) in building_overflow.into_iter().enumerate() {
            occupants[building_index] =
                occupants[building_index]
                    .checked_add(amount)
                    .ok_or_else(|| {
                        missing_data(
                            "housing.occupancy".to_owned(),
                            "overflow occupancy overflow",
                        )
                    })?;
        }
    }

    let basic_occupants = pop_basic.iter().try_fold(0_u64, |sum, value| {
        sum.checked_add(*value)
            .ok_or_else(|| missing_data("housing.basic".to_owned(), "occupant total overflow"))
    })?;
    let makeshift_occupants = pop_makeshift.iter().try_fold(0_u64, |sum, value| {
        sum.checked_add(*value)
            .ok_or_else(|| missing_data("housing.makeshift".to_owned(), "occupant total overflow"))
    })?;
    let class_crowding = |occupants: u64, capacity: u64| -> Result<u64, DataError> {
        if capacity == 0 {
            return Ok(0);
        }
        let value = u128::from(occupants)
            .checked_mul(1000)
            .ok_or_else(|| missing_data("housing.crowding".to_owned(), "crowding overflow"))?
            / u128::from(capacity);
        u64::try_from(value)
            .map_err(|_| missing_data("housing.crowding".to_owned(), "crowding exceeds u64"))
    };
    let basic_crowding = class_crowding(basic_occupants, capacities[0])?;
    let makeshift_crowding = class_crowding(makeshift_occupants, capacities[1])?;
    let total_people = sizes_again.iter().try_fold(0_u64, |sum, value| {
        sum.checked_add(*value).ok_or_else(|| {
            missing_data("housing.population".to_owned(), "place population overflow")
        })
    })?;
    let total_capacity = capacities[0]
        .checked_add(capacities[1])
        .ok_or_else(|| missing_data("housing.capacity".to_owned(), "total capacity overflow"))?;
    let crowding_pm = if total_capacity == 0 {
        if total_people == 0 { 0 } else { 2000 }
    } else {
        u64::try_from(u128::from(total_people) * 1000 / u128::from(total_capacity)).map_err(
            |_| missing_data("housing.crowding".to_owned(), "place crowding exceeds u64"),
        )?
    };
    let pops = sizes_again
        .into_iter()
        .zip(pop_basic)
        .zip(pop_makeshift)
        .map(|((size, basic), makeshift)| {
            let crowding_pm = if size == 0 {
                None
            } else if total_capacity == 0 {
                Some(2000)
            } else {
                let weighted = u128::from(basic)
                    .checked_mul(u128::from(basic_crowding))
                    .and_then(|basic| {
                        u128::from(makeshift)
                            .checked_mul(u128::from(makeshift_crowding))
                            .and_then(|makeshift| basic.checked_add(makeshift))
                    })
                    .ok_or_else(|| {
                        missing_data("housing.crowding".to_owned(), "pop crowding overflow")
                    })?;
                Some(u64::try_from(weighted / u128::from(size)).map_err(|_| {
                    missing_data("housing.crowding".to_owned(), "pop crowding exceeds u64")
                })?)
            };
            Ok(PopHousing {
                basic,
                makeshift,
                crowding_pm,
            })
        })
        .collect::<Result<Vec<_>, DataError>>()?;
    Ok(HousingFill {
        pops,
        building_occupants: occupants,
        housed,
        basic_capacity: capacities[0],
        makeshift_capacity: capacities[1],
        basic_occupants,
        makeshift_occupants,
        crowding_pm,
    })
}

/// Split each pop's people across buildings as one integer pop × building table that conserves
/// both totals: `people[pop]` is each row's sum and `places[building]` each column's.
///
/// Each cell starts at its proportional floor, `people × places / total`. The units still to place
/// go one at a time to the cells with the largest remainders, ties broken by `pop_order` and then
/// by building order, a unit going to a cell only while its pop and its building both still have
/// units to place. Any unit left after that pass goes to the first building, in building order,
/// that still has room, for the first pop, in `pop_order`, that still has people.
fn allocate_people(
    people: &[u64],
    places: &[u64],
    pop_order: &[usize],
) -> Result<Vec<Vec<u64>>, DataError> {
    let refuse = |problem: &'static str| missing_data("housing.allocation".to_owned(), problem);
    let total = places.iter().try_fold(0_u128, |sum, value| {
        sum.checked_add(u128::from(*value))
            .ok_or_else(|| refuse("place total overflow"))
    })?;
    let people_total = people.iter().try_fold(0_u128, |sum, value| {
        sum.checked_add(u128::from(*value))
            .ok_or_else(|| refuse("people total overflow"))
    })?;
    if total != people_total {
        return Err(refuse("people and places do not balance"));
    }
    let mut cells = vec![vec![0_u64; places.len()]; people.len()];
    if total == 0 {
        return Ok(cells);
    }
    let mut people_left = people.to_vec();
    let mut places_left = places.to_vec();
    let mut remainders = Vec::with_capacity(people.len() * places.len());
    for (rank, pop_index) in pop_order.iter().copied().enumerate() {
        for (building_index, place_count) in places.iter().copied().enumerate() {
            let product = u128::from(people[pop_index])
                .checked_mul(u128::from(place_count))
                .ok_or_else(|| refuse("cell product overflow"))?;
            let floor = u64::try_from(product / total).map_err(|_| refuse("cell exceeds u64"))?;
            cells[pop_index][building_index] = floor;
            people_left[pop_index] -= floor;
            places_left[building_index] -= floor;
            remainders.push((product % total, rank, building_index, pop_index));
        }
    }
    remainders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    for (_, _, building_index, pop_index) in remainders {
        if people_left[pop_index] > 0 && places_left[building_index] > 0 {
            cells[pop_index][building_index] += 1;
            people_left[pop_index] -= 1;
            places_left[building_index] -= 1;
        }
    }
    while let Some(building_index) = places_left.iter().position(|left| *left > 0) {
        let Some(pop_index) = pop_order
            .iter()
            .copied()
            .find(|index| people_left[*index] > 0)
        else {
            return Err(refuse("places remain with no people to place"));
        };
        let amount = people_left[pop_index].min(places_left[building_index]);
        cells[pop_index][building_index] += amount;
        people_left[pop_index] -= amount;
        places_left[building_index] -= amount;
    }
    Ok(cells)
}

fn give(capacity: u64, demands: &[u64]) -> Result<Vec<u64>, DataError> {
    let total = demands.iter().try_fold(0_u64, |sum, value| {
        sum.checked_add(*value)
            .ok_or_else(|| missing_data("housing.demands".to_owned(), "demand total overflow"))
    })?;
    if total <= capacity {
        return Ok(demands.to_vec());
    }
    largest_remainder(capacity, demands)
        .map_err(|problem| missing_data("housing.allocation".to_owned(), problem))
}

/// Canonical saved economy state; all lookup maps are represented in stable vectors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EconomyState {
    /// Places in scenario-file order.
    pub places: Vec<PlaceState>,
}

/// Per-system conservation results. These values are returned to the caller and are not saved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProductionLedger {
    /// Whole units produced, in rules good order.
    pub produced: Vec<u64>,
    /// Whole units lost after every permitted receipt path was full, in rules good order.
    pub lost: Vec<u64>,
    /// Herd births, deaths and slaughter by cattle, pigs and sheep order.
    pub herd_births: Vec<u64>,
    /// Herd deaths, by cattle, pigs and sheep order.
    pub herd_deaths: Vec<u64>,
    /// Herds slaughtered, by cattle, pigs and sheep order.
    pub herd_slaughter: Vec<u64>,
    /// Animals that arrived as founders of a herd, by cattle, pigs and sheep order. They are not births.
    pub herd_arrivals: Vec<u64>,
    /// Whole goods held when the pass began, in good order.
    pub stock_before: Vec<u64>,
    /// Whole goods held after the pass, in good order.
    pub stock_after: Vec<u64>,
    /// Whole wage units held for a later payday, in good order.
    pub unpaid_wages: Vec<u64>,
    /// Actual daily intake in per-mille, by place and population order; empty for monthly-only calls.
    pub intake_pm: Vec<Vec<u16>>,
    /// Goods and benefit accounting for the daily drawing/eating phase, when run.
    pub daily_consumption: Option<DailyConsumptionLedger>,
    /// People recruited into each place and pop during this month's demography phase.
    pub movement_by_pop: Vec<Vec<u64>>,
    /// Herd counts when the pass began, by cattle, pigs and sheep.
    pub herds_before: Vec<u64>,
    /// Herd counts after the pass, by cattle, pigs and sheep.
    pub herds_after: Vec<u64>,
    /// Per-place results in scenario order.
    pub places: Vec<PlaceProductionLedger>,
}

/// Daily goods and benefit accounting, in rules order and hundredths of person-days.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DailyConsumptionLedger {
    /// Goods physically held at the start of drawing.
    pub stock_before: Vec<u64>,
    /// Units produced by emergency butchering, including units that did not fit.
    pub butchered: Vec<u64>,
    /// Whole units opened into their benefit and hunger balances.
    pub opened: Vec<u64>,
    /// Whole units consumed by zero-hunger ticking.
    pub ticked: Vec<u64>,
    /// Whole units lost because the pop store could not receive them.
    pub lost: Vec<u64>,
    /// Goods physically held after eating and ticking.
    pub stock_after: Vec<u64>,
    /// Herds held at the start of daily consumption, in cattle/pigs/sheep order.
    pub herds_before: Vec<u64>,
    /// Emergency slaughter in cattle/pigs/sheep order.
    pub herd_slaughter: Vec<u64>,
    /// Herds held after daily consumption, in cattle/pigs/sheep order.
    pub herds_after: Vec<u64>,
    /// Benefit value credited from opened units, by necessities/niceties/luxuries.
    pub opened_benefits_centi: [u128; 3],
    /// Benefit value credited from ticking units, by necessities/niceties/luxuries.
    pub ticked_benefits_centi: [u128; 3],
    /// Benefit value consumed for today's fulfilment, by necessities/niceties/luxuries.
    pub benefits_taken_centi: [u128; 3],
    /// Hunger removed from pop pantries by eating.
    pub hunger_eaten_centi: u128,
}

/// Production and losses for one place in a monthly economy pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlaceProductionLedger {
    /// Stable place id.
    pub place_id: String,
    /// Whole units produced, in rules good order.
    pub produced: Vec<u64>,
    /// Whole units lost after every receipt path was full, in rules good order.
    pub lost: Vec<u64>,
    /// Herd births by cattle, pigs and sheep order.
    pub herd_births: Vec<u64>,
    /// Herd deaths by cattle, pigs and sheep order.
    pub herd_deaths: Vec<u64>,
    /// Herd slaughter by cattle, pigs and sheep order.
    pub herd_slaughter: Vec<u64>,
    /// Animals that arrived as founders of a herd, by cattle, pigs and sheep order. They are not births.
    pub herd_arrivals: Vec<u64>,
    /// Whole goods held when the pass began, in good order.
    pub stock_before: Vec<u64>,
    /// Whole goods held after the pass, in good order.
    pub stock_after: Vec<u64>,
    /// Whole wage units held for a later payday, in good order.
    pub unpaid_wages: Vec<u64>,
    /// Herd counts when the pass began, by cattle, pigs and sheep.
    pub herds_before: Vec<u64>,
    /// Herd counts after the pass, by cattle, pigs and sheep.
    pub herds_after: Vec<u64>,
}

/// Apply one saved signed drift step, preserving the fractional remainder.
fn drift_value(
    value: i64,
    target: i64,
    numerator: i64,
    denominator: i64,
    carry: &mut i64,
    minimum: i64,
    maximum: i64,
) -> Result<i64, DataError> {
    let difference = target - value;
    if *carry != 0 && difference.signum() != carry.signum() {
        *carry = 0;
    }
    let total = difference
        .checked_mul(numerator)
        .and_then(|step| step.checked_add(*carry))
        .ok_or_else(|| missing_data("needs.drift".to_owned(), "drift numerator overflow"))?;
    let step = total / denominator;
    *carry = total % denominator;
    Ok(value.saturating_add(step).clamp(minimum, maximum))
}

fn drift_dependents(
    dependents: u64,
    total: u64,
    base_pm: u16,
    carry: &mut i64,
) -> Result<u64, DataError> {
    let gap = i128::from(dependents) * 1000 - i128::from(total) * i128::from(base_pm);
    if *carry != 0 && gap.signum() != i128::from(carry.signum()) {
        *carry = 0;
    }
    let numerator = gap
        .checked_mul(10)
        .and_then(|value| value.checked_add(i128::from(*carry)))
        .ok_or_else(|| {
            missing_data(
                "demography.dependent_drift".to_owned(),
                "drift numerator overflow",
            )
        })?;
    let step = numerator / 1_000_000;
    *carry = i64::try_from(numerator % 1_000_000).map_err(|_| {
        missing_data(
            "demography.dependent_drift".to_owned(),
            "drift carry exceeds i64",
        )
    })?;
    u64::try_from((i128::from(dependents) - step).clamp(0, i128::from(total))).map_err(|_| {
        missing_data(
            "demography.dependent_drift".to_owned(),
            "result exceeds u64",
        )
    })
}

fn curve_value(points: &[[i32; 2]], x: u64) -> Result<u64, DataError> {
    let x = i64::try_from(x)
        .map_err(|_| missing_data("curves.intake_food_days".to_owned(), "input exceeds i64"))?;
    curve_value_signed(points, x)
}

fn curve_value_signed(points: &[[i32; 2]], x: i64) -> Result<u64, DataError> {
    let first = points
        .first()
        .ok_or_else(|| missing_data("curves.intake_food_days".to_owned(), "curve has no points"))?;
    if x <= i64::from(first[0]) {
        return u64::try_from(i64::from(first[1]).max(0)).map_err(|_| {
            missing_data(
                "curves.intake_food_days".to_owned(),
                "curve value exceeds u64",
            )
        });
    }
    for pair in points.windows(2) {
        let (x0, y0) = (i64::from(pair[0][0]), i64::from(pair[0][1]));
        let (x1, y1) = (i64::from(pair[1][0]), i64::from(pair[1][1]));
        if x <= x1 {
            let numerator = (x - x0).checked_mul(y1 - y0).ok_or_else(|| {
                missing_data(
                    "curves.intake_food_days".to_owned(),
                    "interpolation overflow",
                )
            })?;
            let value = y0 + numerator.div_euclid(x1 - x0);
            return u64::try_from(value.max(0)).map_err(|_| {
                missing_data(
                    "curves.intake_food_days".to_owned(),
                    "curve value exceeds u64",
                )
            });
        }
    }
    let last = points
        .last()
        .ok_or_else(|| missing_data("curves.intake_food_days".to_owned(), "curve has no points"))?;
    u64::try_from(i64::from(last[1]).max(0)).map_err(|_| {
        missing_data(
            "curves.intake_food_days".to_owned(),
            "curve value exceeds u64",
        )
    })
}

/// The satisfaction `pop` drifts toward on its present needs met: the baseline its monthly drift aims at, which the character screen's pop tab
/// shows as the equilibrium (CK-378). `None` when `rules` has no definition of the pop's kind.
#[must_use]
pub fn satisfaction_equilibrium(rules: &Rules, pop: &PopState) -> Option<i64> {
    let kind = rules
        .pop_types
        .iter()
        .find(|candidate| candidate.id == pop.kind)?;
    Some(satisfaction_baseline(
        kind,
        [
            pop.consumption.necessities_pm,
            pop.consumption.niceties_pm,
            pop.consumption.luxuries_pm,
        ],
    ))
}

fn satisfaction_baseline(pop: &PopType, needs: [u16; 3]) -> i64 {
    let n_gap = i64::from(pop.necessities_expectation_pm) - i64::from(needs[0]);
    let w_gap = i64::from(pop.niceties_expectation_pm) - i64::from(needs[1]);
    let l_gap = i64::from(needs[2]) - i64::from(pop.luxuries_expectation_pm);
    let mut baseline = -10 * n_gap.max(0) / 100 - 3 * w_gap.max(0) / 100
        + 2 * (-w_gap).max(0) / 100
        + 3 * l_gap.max(0) / 100;
    if needs[0] < 500 {
        baseline -= 20;
    }
    if l_gap >= 200 {
        baseline += 10;
    }
    baseline.clamp(-100, 100)
}

impl ProductionLedger {
    fn new(good_count: usize, species_count: usize) -> Self {
        Self {
            produced: vec![0; good_count],
            lost: vec![0; good_count],
            herd_births: vec![0; species_count],
            herd_deaths: vec![0; species_count],
            herd_slaughter: vec![0; species_count],
            herd_arrivals: vec![0; species_count],
            stock_before: vec![0; good_count],
            stock_after: vec![0; good_count],
            unpaid_wages: vec![0; good_count],
            intake_pm: Vec::new(),
            daily_consumption: None,
            movement_by_pop: Vec::new(),
            herds_before: vec![0; species_count],
            herds_after: vec![0; species_count],
            places: Vec::new(),
        }
    }
}

impl PlaceProductionLedger {
    fn new(place_id: &str, good_count: usize, species_count: usize) -> Self {
        Self {
            place_id: place_id.to_owned(),
            produced: vec![0; good_count],
            lost: vec![0; good_count],
            herd_births: vec![0; species_count],
            herd_deaths: vec![0; species_count],
            herd_slaughter: vec![0; species_count],
            herd_arrivals: vec![0; species_count],
            stock_before: vec![0; good_count],
            stock_after: vec![0; good_count],
            unpaid_wages: vec![0; good_count],
            herds_before: vec![0; species_count],
            herds_after: vec![0; species_count],
        }
    }
}

fn total_stored_goods(rules: &Rules, place: &PlaceState) -> Result<Vec<u64>, DataError> {
    let mut totals = vec![0_u64; rules.goods.len()];
    for stock in place.pops.iter().map(|pop| pop.stock.as_slice()).chain(
        place
            .buildings
            .iter()
            .map(|building| building.stock.as_slice()),
    ) {
        if stock.len() != totals.len() {
            return Err(missing_data(
                format!("places.{}.stock", place.id),
                "saved stock dimensions do not match goods",
            ));
        }
        for (total, amount) in totals.iter_mut().zip(stock) {
            *total = total
                .checked_add(*amount)
                .ok_or_else(|| missing_data("ledger.stock".to_owned(), "stock total overflow"))?;
        }
    }
    Ok(totals)
}

fn total_state_stored_goods(rules: &Rules, state: &EconomyState) -> Result<Vec<u64>, DataError> {
    let mut totals = vec![0_u64; rules.goods.len()];
    for place in &state.places {
        for (total, amount) in totals.iter_mut().zip(total_stored_goods(rules, place)?) {
            *total = total
                .checked_add(amount)
                .ok_or_else(|| missing_data("ledger.stock".to_owned(), "stock total overflow"))?;
        }
    }
    Ok(totals)
}

fn total_state_herds(state: &EconomyState, species_count: usize) -> Result<Vec<u64>, DataError> {
    let mut totals = vec![0_u64; species_count];
    for place in &state.places {
        for (total, amount) in totals.iter_mut().zip(total_herds(place, species_count)?) {
            *total = total
                .checked_add(amount)
                .ok_or_else(|| missing_data("ledger.herds".to_owned(), "herd total overflow"))?;
        }
    }
    Ok(totals)
}

fn add_daily_good(
    values: &mut [u64],
    index: usize,
    amount: u64,
    entry: &str,
) -> Result<(), DataError> {
    let value = values
        .get_mut(index)
        .ok_or_else(|| missing_data(entry.to_owned(), "ledger index is missing"))?;
    *value = value
        .checked_add(amount)
        .ok_or_else(|| missing_data(entry.to_owned(), "daily goods total overflow"))?;
    Ok(())
}

fn total_herds(place: &PlaceState, species_count: usize) -> Result<Vec<u64>, DataError> {
    let mut totals = vec![0_u64; species_count];
    for building in &place.buildings {
        if !building.herds.is_empty() && building.herds.len() != totals.len() {
            return Err(missing_data(
                format!("places.{}.buildings.{}.herds", place.id, building.kind),
                "saved herd dimensions do not match species",
            ));
        }
        for (total, amount) in totals.iter_mut().zip(&building.herds) {
            *total = total
                .checked_add(*amount)
                .ok_or_else(|| missing_data("ledger.herds".to_owned(), "herd total overflow"))?;
        }
    }
    Ok(totals)
}

fn held_hunger(rules: &Rules, pop: &PopState) -> Result<u128, DataError> {
    if pop.stock.len() != rules.goods.len()
        || pop.consumption.pantry_hunger_centi.len() != rules.goods.len()
    {
        return Err(missing_data(
            "pop.consumption.pantry_hunger_centi".to_owned(),
            "saved dimensions do not match goods",
        ));
    }
    rules
        .goods
        .iter()
        .enumerate()
        .try_fold(0_u128, |sum, (index, good)| {
            let food = u128::from(pop.stock[index])
                .checked_mul(u128::from(good.hunger_centi))
                .and_then(|value| {
                    value.checked_add(u128::from(pop.consumption.pantry_hunger_centi[index]))
                })
                .ok_or_else(|| missing_data("pop.food".to_owned(), "held food overflow"))?;
            sum.checked_add(food)
                .ok_or_else(|| missing_data("pop.food".to_owned(), "held food sum overflow"))
        })
}

fn account_hunger(rules: &Rules, account: &[u64]) -> Result<u128, DataError> {
    if account.len() != rules.goods.len() {
        return Err(missing_data(
            "storehouse.account".to_owned(),
            "account dimensions do not match goods",
        ));
    }
    rules
        .goods
        .iter()
        .zip(account)
        .try_fold(0_u128, |sum, (good, units)| {
            let food = u128::from(*units)
                .checked_mul(u128::from(good.hunger_centi))
                .ok_or_else(|| {
                    missing_data("storehouse.food".to_owned(), "account food overflow")
                })?;
            sum.checked_add(food).ok_or_else(|| {
                missing_data("storehouse.food".to_owned(), "account food sum overflow")
            })
        })
}

fn units_to_fill(target: u128, held: u128, hunger: u32) -> u64 {
    if target <= held || hunger == 0 {
        return 0;
    }
    u64::try_from((target - held).div_ceil(u128::from(hunger))).unwrap_or(u64::MAX)
}

fn pull_into_pop(
    rules: &Rules,
    pop: &mut PopState,
    good_index: usize,
    available: u64,
    target: u128,
    held: &mut u128,
) -> Result<u64, DataError> {
    let good = &rules.goods[good_index];
    let offered = available.min(units_to_fill(target, *held, good.hunger_centi));
    if offered == 0 {
        return Ok(0);
    }
    let leftover = receive_pop(rules, pop, good_index, offered)?;
    let moved = offered - leftover;
    *held = held
        .checked_add(u128::from(moved) * u128::from(good.hunger_centi))
        .ok_or_else(|| missing_data("drawing.food".to_owned(), "drawn food overflow"))?;
    Ok(moved)
}

fn debit_storehouse(
    building: &mut BuildingState,
    pop_index: Option<usize>,
    good_index: usize,
    amount: u64,
) -> Result<(), DataError> {
    let accounts = building.storehouse_accounts.as_mut().ok_or_else(|| {
        missing_data("storehouse.accounts".to_owned(), "account table is missing")
    })?;
    let account = if let Some(pop_index) = pop_index {
        accounts
            .pops
            .get_mut(pop_index)
            .and_then(|values| values.get_mut(good_index))
    } else {
        accounts.communal.get_mut(good_index)
    }
    .ok_or_else(|| {
        missing_data(
            "storehouse.accounts".to_owned(),
            "account dimensions do not match goods",
        )
    })?;
    *account = account.checked_sub(amount).ok_or_else(|| {
        missing_data(
            "storehouse.accounts".to_owned(),
            "draw exceeds account balance",
        )
    })?;
    let stock = building.stock.get_mut(good_index).ok_or_else(|| {
        missing_data(
            "storehouse.stock".to_owned(),
            "stock dimensions do not match goods",
        )
    })?;
    *stock = stock.checked_sub(amount).ok_or_else(|| {
        missing_data(
            "storehouse.stock".to_owned(),
            "account total exceeds physical stock",
        )
    })?;
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "Drawing follows the required account, owner-stock, then communal-source order in one saved-place transaction."
)]
fn draw_pop(rules: &Rules, place: &mut PlaceState, pop_index: usize) -> Result<(), DataError> {
    let people = place.pops[pop_index]
        .workers
        .checked_add(place.pops[pop_index].dependents)
        .ok_or_else(|| missing_data("pop.size".to_owned(), "population size overflow"))?;
    if people == 0 {
        return Ok(());
    }
    let target = u128::from(people) * 6000;
    let mut held = held_hunger(rules, &place.pops[pop_index])?;
    if held >= target {
        return Ok(());
    }
    let storehouse = place
        .buildings
        .iter()
        .position(|building| building.storehouse_accounts.is_some());
    if let Some(index) = storehouse {
        for good_index in 0..rules.goods.len() {
            if held >= target {
                break;
            }
            let amount = place.buildings[index]
                .storehouse_accounts
                .as_ref()
                .and_then(|accounts| accounts.pops.get(pop_index))
                .and_then(|account| account.get(good_index))
                .copied()
                .ok_or_else(|| {
                    missing_data("storehouse.accounts".to_owned(), "pop account is missing")
                })?;
            let moved = pull_into_pop(
                rules,
                &mut place.pops[pop_index],
                good_index,
                amount,
                target,
                &mut held,
            )?;
            if moved != 0 {
                let building = &mut place.buildings[index];
                debit_storehouse(building, Some(pop_index), good_index, moved)?;
            }
        }
    }
    let owner_buildings = place
        .buildings
        .iter()
        .enumerate()
        .filter_map(|(index, building)| {
            building
                .employment
                .get(pop_index)
                .is_some_and(|employment| employment.owners > 0)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    let has_owner_building = !owner_buildings.is_empty();
    for building_index in owner_buildings {
        for good_index in 0..rules.goods.len() {
            if held >= target {
                break;
            }
            let amount = place.buildings[building_index]
                .stock
                .get(good_index)
                .copied()
                .ok_or_else(|| {
                    missing_data(
                        "building.stock".to_owned(),
                        "stock dimensions do not match goods",
                    )
                })?;
            let moved = pull_into_pop(
                rules,
                &mut place.pops[pop_index],
                good_index,
                amount,
                target,
                &mut held,
            )?;
            let stock = place.buildings[building_index]
                .stock
                .get_mut(good_index)
                .ok_or_else(|| {
                    missing_data(
                        "building.stock".to_owned(),
                        "stock dimensions do not match goods",
                    )
                })?;
            *stock = stock.checked_sub(moved).ok_or_else(|| {
                missing_data("building.stock".to_owned(), "draw exceeds building stock")
            })?;
        }
    }
    if has_owner_building && let Some(index) = storehouse {
        for good_index in 0..rules.goods.len() {
            if held >= target {
                break;
            }
            let amount = place.buildings[index]
                .storehouse_accounts
                .as_ref()
                .and_then(|accounts| accounts.communal.get(good_index))
                .copied()
                .ok_or_else(|| {
                    missing_data(
                        "storehouse.accounts".to_owned(),
                        "communal account dimensions do not match goods",
                    )
                })?;
            let moved = pull_into_pop(
                rules,
                &mut place.pops[pop_index],
                good_index,
                amount,
                target,
                &mut held,
            )?;
            let building = &mut place.buildings[index];
            debit_storehouse(building, None, good_index, moved)?;
        }
    }
    Ok(())
}

fn index_for_good(rules: &Rules, id: &str) -> Result<usize, DataError> {
    rules
        .goods
        .iter()
        .position(|good| good.id == id)
        .ok_or_else(|| missing_data(format!("goods.{id}"), "definition is missing"))
}

#[expect(
    clippy::too_many_lines,
    reason = "Emergency slaughter keeps owner access, the breeding floor, species order and per-head yields auditable together."
)]
fn butcher_emergency(
    rules: &Rules,
    place: &mut PlaceState,
    pop_index: usize,
    threshold: u128,
    ledger: &mut DailyConsumptionLedger,
) -> Result<(), DataError> {
    let necessities = place.pops[pop_index].consumption.necessities_pm;
    let waive_floor = necessities < 300;
    let owner_buildings = place
        .buildings
        .iter()
        .enumerate()
        .filter_map(|(index, building)| {
            building
                .employment
                .get(pop_index)
                .is_some_and(|employment| employment.owners > 0)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    let mut species_order: Vec<usize> = (0..rules.herd_species.len()).collect();
    species_order.sort_by_key(|index| rules.herd_species[*index].emergency_order);
    for building_index in owner_buildings {
        if place.buildings[building_index].herds.is_empty() {
            continue;
        }
        let building = &place.buildings[building_index];
        let definition = rules
            .buildings
            .iter()
            .find(|item| item.id == building.kind)
            .ok_or_else(|| {
                missing_data(
                    format!("buildings.{}", building.kind),
                    "definition is missing",
                )
            })?;
        let farming_levels = definition
            .methods
            .iter()
            .zip(&building.method_levels)
            .filter(|(method, _)| method.resource != "sea")
            .map(|(_, levels)| *levels)
            .sum::<u32>();
        for &species in &species_order {
            let species_rules = &rules.herd_species[species];
            if !definition.herds.contains_key(&species_rules.animal)
                || !species_rules.yields.iter().any(|item| {
                    item.units > 0
                        && rules
                            .goods
                            .iter()
                            .any(|good| good.id == item.good && good.hunger_centi > 0)
                })
            {
                continue;
            }
            while held_hunger(rules, &place.pops[pop_index])? < threshold {
                let herd = *place.buildings[building_index]
                    .herds
                    .get(species)
                    .ok_or_else(|| {
                        missing_data("herds".to_owned(), "saved herd dimensions are missing")
                    })?;
                let floor = if waive_floor {
                    0
                } else {
                    u64::from(farming_levels) * 2
                };
                if herd <= floor {
                    break;
                }
                let before = held_hunger(rules, &place.pops[pop_index])?;
                place.buildings[building_index].herds[species] -= 1;
                add_daily_good(
                    &mut ledger.herd_slaughter,
                    species,
                    1,
                    "ledger.daily.herd_slaughter",
                )?;
                for item in &species_rules.yields {
                    let good_index = index_for_good(rules, &item.good)?;
                    add_daily_good(
                        &mut ledger.butchered,
                        good_index,
                        item.units,
                        "ledger.daily.butchered",
                    )?;
                    let extra =
                        receive_pop(rules, &mut place.pops[pop_index], good_index, item.units)?;
                    let lost = route_pop_surplus(
                        rules,
                        &mut place.pops,
                        &mut place.buildings,
                        pop_index,
                        good_index,
                        extra,
                    )?;
                    if lost != 0 {
                        add_daily_good(&mut ledger.lost, good_index, lost, "ledger.daily.lost")?;
                        add_counter_units(
                            &mut place.counters.current.lost,
                            rules.goods.len(),
                            good_index,
                            lost,
                            "counters.lost",
                        )?;
                        place.pops[pop_index].counters.current.losses = place.pops[pop_index]
                            .counters
                            .current
                            .losses
                            .checked_add(lost)
                            .ok_or_else(|| {
                                missing_data("butchering.loss".to_owned(), "loss counter overflow")
                            })?;
                        place.counters.current.losses = place
                            .counters
                            .current
                            .losses
                            .checked_add(lost)
                            .ok_or_else(|| {
                                missing_data("butchering.loss".to_owned(), "loss counter overflow")
                            })?;
                    }
                }
                if held_hunger(rules, &place.pops[pop_index])? == before {
                    break;
                }
            }
        }
        if held_hunger(rules, &place.pops[pop_index])? >= threshold {
            break;
        }
    }
    Ok(())
}

fn apportion_food(total: u128, holdings: &[u128]) -> Result<Vec<u128>, DataError> {
    let sum = holdings
        .iter()
        .try_fold(0_u128, |sum, amount| sum.checked_add(*amount))
        .ok_or_else(|| missing_data("eating.apportionment".to_owned(), "held food sum overflow"))?;
    if total == 0 || sum == 0 {
        return Ok(vec![0; holdings.len()]);
    }
    if total > sum {
        return Err(missing_data(
            "eating.apportionment".to_owned(),
            "intake exceeds held food",
        ));
    }
    let mut result = Vec::with_capacity(holdings.len());
    let mut remainders = Vec::with_capacity(holdings.len());
    let mut apportioned = 0_u128;
    for (index, amount) in holdings.iter().enumerate() {
        let numerator = total.checked_mul(*amount).ok_or_else(|| {
            missing_data(
                "eating.apportionment".to_owned(),
                "share numerator overflow",
            )
        })?;
        let share = numerator / sum;
        apportioned += share;
        result.push(share);
        remainders.push((numerator % sum, index));
    }
    remainders.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    let remaining = usize::try_from(total - apportioned)
        .map_err(|_| missing_data("eating.apportionment".to_owned(), "remainder exceeds usize"))?;
    for (_, index) in remainders.into_iter().take(remaining) {
        result[index] += 1;
    }
    if result.iter().sum::<u128>() != total
        || result
            .iter()
            .zip(holdings)
            .any(|(share, held)| share > held)
    {
        return Err(missing_data(
            "eating.apportionment".to_owned(),
            "apportionment failed conservation",
        ));
    }
    Ok(result)
}

fn credit_benefits(pop: &mut PopState, good: &Good, units: u64) -> Result<(), DataError> {
    for (balance, value, tier) in [
        (
            &mut pop.consumption.necessities_centi,
            good.necessities_centi,
            "necessities",
        ),
        (
            &mut pop.consumption.niceties_centi,
            good.niceties_centi,
            "niceties",
        ),
        (
            &mut pop.consumption.luxuries_centi,
            good.luxuries_centi,
            "luxuries",
        ),
    ] {
        let amount = units
            .checked_mul(u64::from(value))
            .ok_or_else(|| missing_data(format!("eating.{tier}"), "benefit value overflow"))?;
        *balance = balance
            .checked_add(amount)
            .ok_or_else(|| missing_data(format!("eating.{tier}"), "benefit balance overflow"))?;
    }
    Ok(())
}

/// Run drawing, eating, needs drift and the first-of-month satisfaction drift as one atomic daily step.
///
/// # Errors
/// Returns a named data error for invalid saved dimensions, missing rates or curves, or arithmetic overflow;
/// `state` is unchanged on error.
pub fn run_daily_consumption(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
) -> Result<Vec<Vec<u16>>, DataError> {
    run_daily_consumption_with_ledger(rules, state, date).map(|(intake, _)| intake)
}

fn shed_pop_surplus(
    rules: &Rules,
    place: &mut PlaceState,
    giver: usize,
    ledger: &mut DailyConsumptionLedger,
) -> Result<(), DataError> {
    let people = place.pops[giver]
        .workers
        .checked_add(place.pops[giver].dependents)
        .ok_or_else(|| missing_data("pop.size".to_owned(), "population size overflow"))?;
    let mut shed = Vec::new();
    for good in 0..rules.goods.len() {
        let (cap, _) = pop_store_limits(rules, people, good)?;
        let extra = place.pops[giver].stock[good].saturating_sub(cap);
        if extra > 0 {
            place.pops[giver].stock[good] -= extra;
            shed.push((good, extra));
        }
    }
    let (_, capacity) = pop_store_limits(rules, people, 0)?;
    while volume_used(rules, &place.pops[giver].stock, None)? > capacity {
        let good = (0..rules.goods.len())
            .filter(|&i| {
                place.pops[giver].stock[i] > 0
                    && !rules.goods[i].stacked_outside
                    && rules.goods[i].volume_ml > 0
            })
            .min_by(|&a, &b| {
                (u128::from(place.prices_milli[a]) * u128::from(rules.goods[b].volume_ml))
                    .cmp(
                        &(u128::from(place.prices_milli[b]) * u128::from(rules.goods[a].volume_ml)),
                    )
                    .then(a.cmp(&b))
            })
            .ok_or_else(|| {
                missing_data(
                    "surplus.volume".to_owned(),
                    "over capacity without a stored good",
                )
            })?;
        let over = volume_used(rules, &place.pops[giver].stock, None)? - capacity;
        let volume = u128::from(rules.goods[good].volume_ml);
        let units = u64::try_from(
            over.div_ceil(volume)
                .min(u128::from(place.pops[giver].stock[good])),
        )
        .expect("bounded by stock");
        place.pops[giver].stock[good] -= units;
        shed.push((good, units));
    }
    for (good, amount) in shed {
        let lost = route_pop_surplus(
            rules,
            &mut place.pops,
            &mut place.buildings,
            giver,
            good,
            amount,
        )?;
        if lost == 0 {
            continue;
        }
        add_daily_good(&mut ledger.lost, good, lost, "ledger.daily.lost")?;
        add_counter_units(
            &mut place.counters.current.lost,
            rules.goods.len(),
            good,
            lost,
            "counters.lost",
        )?;
        place.counters.current.losses = place
            .counters
            .current
            .losses
            .checked_add(lost)
            .ok_or_else(|| missing_data("counters.losses".to_owned(), "place loss overflow"))?;
        place.pops[giver].counters.current.losses = place.pops[giver]
            .counters
            .current
            .losses
            .checked_add(lost)
            .ok_or_else(|| missing_data("counters.losses".to_owned(), "pop loss overflow"))?;
    }
    Ok(())
}

fn run_daily_consumption_with_ledger(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
) -> Result<(Vec<Vec<u16>>, DailyConsumptionLedger), DataError> {
    let prices: Vec<Vec<u64>> = state
        .places
        .iter()
        .map(|place| place.prices_milli.clone())
        .collect();
    run_daily_consumption_at_prices(rules, state, date, &prices)
}

fn run_daily_consumption_at_prices(
    rules: &Rules,
    state: &mut EconomyState,
    _date: Date,
    prices: &[Vec<u64>],
) -> Result<(Vec<Vec<u16>>, DailyConsumptionLedger), DataError> {
    rules.validate("economy rules")?;
    let mut next = state.clone();
    let mut ledger = DailyConsumptionLedger {
        stock_before: total_state_stored_goods(rules, &next)?,
        herds_before: total_state_herds(&next, rules.herd_species.len())?,
        butchered: vec![0; rules.goods.len()],
        opened: vec![0; rules.goods.len()],
        ticked: vec![0; rules.goods.len()],
        lost: vec![0; rules.goods.len()],
        herd_slaughter: vec![0; rules.herd_species.len()],
        ..DailyConsumptionLedger::default()
    };
    let mut intake_by_place = Vec::with_capacity(next.places.len());
    for (place, day_prices) in next.places.iter_mut().zip(prices) {
        prepare_place_counters(&mut place.counters, rules.goods.len())?;
        let mut place_intake = Vec::with_capacity(place.pops.len());
        let opened_before = ledger.opened.clone();
        let ticked_before = ledger.ticked.clone();
        let butchered_before = ledger.butchered.clone();
        let mut index = 0;
        while index < place.pops.len() {
            draw_pop(rules, place, index)?;
            fuel::draw_and_burn(rules, place, index, day_prices, &mut ledger)?;
            let intake_pm = consume_pop_tracked(rules, place, index, &mut ledger)?;
            place_intake.push(intake_pm);
            index += 1;
        }
        for giver in 0..place.pops.len() {
            shed_pop_surplus(rules, place, giver, &mut ledger)?;
        }
        for good_index in 0..rules.goods.len() {
            let opened = ledger.opened[good_index] - opened_before[good_index];
            let ticked = ledger.ticked[good_index] - ticked_before[good_index];
            let butchered = ledger.butchered[good_index] - butchered_before[good_index];
            add_counter_units(
                &mut place.counters.current.used_by_pops,
                rules.goods.len(),
                good_index,
                opened.checked_add(ticked).ok_or_else(|| {
                    missing_data("counters.used_by_pops".to_owned(), "daily use overflow")
                })?,
                "counters.used_by_pops",
            )?;
            add_counter_units(
                &mut place.counters.current.produced,
                rules.goods.len(),
                good_index,
                butchered,
                "counters.produced",
            )?;
        }
        intake_by_place.push(place_intake);
    }
    ledger.stock_after = total_state_stored_goods(rules, &next)?;
    ledger.herds_after = total_state_herds(&next, rules.herd_species.len())?;
    *state = next;
    Ok((intake_by_place, ledger))
}

#[cfg(test)]
fn consume_pop(rules: &Rules, place: &mut PlaceState, pop_index: usize) -> Result<u16, DataError> {
    let mut ledger = DailyConsumptionLedger {
        butchered: vec![0; rules.goods.len()],
        opened: vec![0; rules.goods.len()],
        ticked: vec![0; rules.goods.len()],
        lost: vec![0; rules.goods.len()],
        ..DailyConsumptionLedger::default()
    };
    consume_pop_tracked(rules, place, pop_index, &mut ledger)
}

#[expect(
    clippy::too_many_lines,
    reason = "The daily routine keeps the specified butchering, apportionment, opening, ticking and fulfilment sequence auditable."
)]
fn consume_pop_tracked(
    rules: &Rules,
    place: &mut PlaceState,
    pop_index: usize,
    ledger: &mut DailyConsumptionLedger,
) -> Result<u16, DataError> {
    let consumption = &place.pops[pop_index].consumption;
    if consumption.ticking_carries.len() != rules.goods.len() {
        return Err(missing_data(
            "pop.consumption.ticking_carries".to_owned(),
            "saved dimensions do not match goods",
        ));
    }
    if [
        consumption.necessities_pm,
        consumption.niceties_pm,
        consumption.luxuries_pm,
    ]
    .into_iter()
    .any(|value| value > 1000)
        || !(-100..=100).contains(&consumption.satisfaction)
        || consumption
            .ticking_carries
            .iter()
            .any(|carry| *carry >= 1_000_000)
        || consumption
            .drift_carries
            .iter()
            .any(|carry| carry.unsigned_abs() >= 1000)
    {
        return Err(missing_data(
            "pop.consumption".to_owned(),
            "saved needs, satisfaction or carries are outside their ranges",
        ));
    }
    if consumption
        .pantry_hunger_centi
        .iter()
        .zip(&rules.goods)
        .any(|(pantry, good)| *pantry != 0 && good.hunger_centi == 0)
    {
        return Err(missing_data(
            "pop.consumption.pantry_hunger_centi".to_owned(),
            "a non-food good has a hunger pantry",
        ));
    }
    let size = place.pops[pop_index]
        .workers
        .checked_add(place.pops[pop_index].dependents)
        .ok_or_else(|| missing_data("pop.size".to_owned(), "population size overflow"))?;
    if size == 0 {
        place.pops[pop_index].consumption.necessities_pm = 1000;
        place.pops[pop_index].consumption.niceties_pm = 1000;
        place.pops[pop_index].consumption.luxuries_pm = 1000;
        return Ok(1000);
    }
    let need = u128::from(size) * 100;
    let account_food = place
        .buildings
        .iter()
        .filter_map(|building| building.storehouse_accounts.as_ref())
        .filter_map(|accounts| accounts.pops.get(pop_index))
        .try_fold(0_u128, |sum, account| {
            sum.checked_add(account_hunger(rules, account)?)
                .ok_or_else(|| {
                    missing_data("storehouse.food".to_owned(), "account food sum overflow")
                })
        })?;
    let mut held = held_hunger(rules, &place.pops[pop_index])?;
    let emergency = need * 500 / 1000;
    if held < emergency {
        butcher_emergency(rules, place, pop_index, emergency, ledger)?;
        held = held_hunger(rules, &place.pops[pop_index])?;
    }
    let days = (held + account_food) / need;
    let intake_pm = curve_value(
        rules.curves.get("intake_food_days").ok_or_else(|| {
            missing_data("curves.intake_food_days".to_owned(), "curve is missing")
        })?,
        u64::try_from(days).unwrap_or(u64::MAX),
    )?;
    if intake_pm > 1000 {
        return Err(missing_data(
            "curves.intake_food_days".to_owned(),
            "intake values must be in 0..=1000‰",
        ));
    }
    let total = (need * u128::from(intake_pm) / 1000).min(held);
    ledger.hunger_eaten_centi = ledger
        .hunger_eaten_centi
        .checked_add(total)
        .ok_or_else(|| missing_data("ledger.daily.hunger".to_owned(), "daily hunger overflow"))?;
    let actual_intake_pm = u16::try_from(total * 1000 / need).map_err(|_| {
        missing_data(
            "curves.intake_food_days".to_owned(),
            "intake exceeds per-mille range",
        )
    })?;
    if total > 0 {
        let goods_held = rules
            .goods
            .iter()
            .enumerate()
            .map(|(index, good)| {
                u128::from(place.pops[pop_index].stock[index]) * u128::from(good.hunger_centi)
                    + u128::from(place.pops[pop_index].consumption.pantry_hunger_centi[index])
            })
            .collect::<Vec<_>>();
        let eaten = apportion_food(total, &goods_held)?;
        let pop = &mut place.pops[pop_index];
        for (index, (good, amount)) in rules.goods.iter().zip(eaten).enumerate() {
            if amount == 0 {
                continue;
            }
            if amount > u128::from(pop.consumption.pantry_hunger_centi[index]) {
                if good.hunger_centi == 0 {
                    return Err(missing_data(
                        format!("goods.{}.hunger_centi", good.id),
                        "eaten good has no hunger value",
                    ));
                }
                let missing = amount - u128::from(pop.consumption.pantry_hunger_centi[index]);
                let units = missing.div_ceil(u128::from(good.hunger_centi));
                let units = u64::try_from(units).map_err(|_| {
                    missing_data("eating.open".to_owned(), "opened unit count exceeds u64")
                })?;
                if units > pop.stock[index] {
                    return Err(missing_data(
                        "eating.open".to_owned(),
                        "held units do not cover apportionment",
                    ));
                }
                pop.stock[index] -= units;
                add_daily_good(&mut ledger.opened, index, units, "ledger.daily.opened")?;
                let opened_hunger =
                    units
                        .checked_mul(u64::from(good.hunger_centi))
                        .ok_or_else(|| {
                            missing_data("eating.pantry".to_owned(), "opened hunger overflow")
                        })?;
                pop.consumption.pantry_hunger_centi[index] = pop.consumption.pantry_hunger_centi
                    [index]
                    .checked_add(opened_hunger)
                    .ok_or_else(|| {
                        missing_data("eating.pantry".to_owned(), "hunger pantry overflow")
                    })?;
                credit_benefits(pop, good, units)?;
                for (tier, value) in [
                    good.necessities_centi,
                    good.niceties_centi,
                    good.luxuries_centi,
                ]
                .into_iter()
                .enumerate()
                {
                    ledger.opened_benefits_centi[tier] = ledger.opened_benefits_centi[tier]
                        .checked_add(u128::from(units) * u128::from(value))
                        .ok_or_else(|| {
                            missing_data(
                                "ledger.daily.opened_benefits".to_owned(),
                                "benefit total overflow",
                            )
                        })?;
                }
            }
            pop.consumption.pantry_hunger_centi[index] -= u64::try_from(amount).map_err(|_| {
                missing_data("eating.pantry".to_owned(), "eaten amount exceeds u64")
            })?;
        }
    }
    // Ticking rates are required by name for herbs and beeswax (owner ruling, clothing r1 excludes wool).
    for id in ["herbs", "beeswax"] {
        let Some(index) = rules.goods.iter().position(|good| good.id == id) else {
            continue;
        };
        let good = &rules.goods[index];
        if place.pops[pop_index].stock[index] == 0 {
            continue;
        }
        let rate = rules
            .ticking_rates_million
            .get(id)
            .and_then(|rates| rates.get(&place.pops[pop_index].kind))
            .copied()
            .ok_or_else(|| {
                missing_data(
                    format!("ticking_rates_million.{id}.{}", place.pops[pop_index].kind),
                    "rate is missing",
                )
            })?;
        let pop = &mut place.pops[pop_index];
        let numerator = u128::from(size)
            .checked_mul(u128::from(rate))
            .and_then(|value| {
                value.checked_add(u128::from(
                    pop.consumption
                        .ticking_carries
                        .get(index)
                        .copied()
                        .unwrap_or(0),
                ))
            })
            .ok_or_else(|| {
                missing_data(
                    format!("ticking_rates_million.{id}"),
                    "tick numerator overflow",
                )
            })?;
        let used = u64::try_from(numerator / 1_000_000)
            .map_err(|_| {
                missing_data(
                    format!("ticking_rates_million.{id}"),
                    "tick amount exceeds u64",
                )
            })?
            .min(pop.stock[index]);
        add_daily_good(&mut ledger.ticked, index, used, "ledger.daily.ticked")?;
        pop.consumption.ticking_carries[index] =
            u64::try_from(numerator % 1_000_000).map_err(|_| {
                missing_data(
                    format!("ticking_rates_million.{id}"),
                    "tick carry exceeds u64",
                )
            })?;
        pop.stock[index] -= used;
        credit_benefits(pop, good, used)?;
        for (tier, value) in [
            good.necessities_centi,
            good.niceties_centi,
            good.luxuries_centi,
        ]
        .into_iter()
        .enumerate()
        {
            ledger.ticked_benefits_centi[tier] = ledger.ticked_benefits_centi[tier]
                .checked_add(u128::from(used) * u128::from(value))
                .ok_or_else(|| {
                    missing_data(
                        "ledger.daily.ticked_benefits".to_owned(),
                        "benefit total overflow",
                    )
                })?;
        }
    }
    let pop_type = rules
        .pop_types
        .iter()
        .find(|item| item.id == place.pops[pop_index].kind)
        .ok_or_else(|| {
            missing_data(
                format!("pop_types.{}", place.pops[pop_index].kind),
                "definition is missing",
            )
        })?;
    let pop = &mut place.pops[pop_index];
    let need_u64 = u64::try_from(need)
        .map_err(|_| missing_data("needs".to_owned(), "daily need exceeds u64"))?;
    let tiers = [
        &mut pop.consumption.necessities_centi,
        &mut pop.consumption.niceties_centi,
        &mut pop.consumption.luxuries_centi,
    ];
    let mut fulfilment = [0_u16; 3];
    for (index, balance) in tiers.into_iter().enumerate() {
        let taken = (*balance).min(need_u64);
        *balance -= taken;
        ledger.benefits_taken_centi[index] = ledger.benefits_taken_centi[index]
            .checked_add(u128::from(taken))
            .ok_or_else(|| {
                missing_data(
                    "ledger.daily.benefits_taken".to_owned(),
                    "benefit total overflow",
                )
            })?;
        fulfilment[index] = u16::try_from(u128::from(taken) * 1000 / u128::from(need_u64))
            .expect("fulfilment is at most 1000");
    }
    let tier_values = [
        &mut pop.consumption.necessities_pm,
        &mut pop.consumption.niceties_pm,
        &mut pop.consumption.luxuries_pm,
    ];
    for (index, value) in tier_values.into_iter().enumerate() {
        *value = u16::try_from(drift_value(
            i64::from(*value),
            i64::from(fulfilment[index]),
            50,
            1000,
            &mut pop.consumption.drift_carries[index],
            0,
            1000,
        )?)
        .map_err(|_| missing_data("needs".to_owned(), "drifted need exceeds u16"))?;
    }
    let _ = pop_type;
    Ok(actual_intake_pm)
}

/// What one payday-style split of a building's output left to settle: the wages' worth, and for each good the amounts that did
/// not fit where they were sent (a store the receiver had no room in, or the building's overhead), by building and good.
type PendingOverflow = Vec<(usize, usize, Vec<(Option<usize>, u64)>)>;

/// Splits `output` (whole units by good) into wages and overhead by the building's wage share, taking the wage carry on from where
/// it stands, and pays the wages among the building's workers by their weights, as payday does; where the building has no workers
/// they are added to its pay carry. The wages go into the workers' stores; what does not fit, and the overhead, are returned to be
/// settled by [`settle_overflow`]. Returns the worth of the wages at the place's prices and what is to be settled.
///
/// # Errors
/// Refuses missing prices or checked arithmetic overflow.
#[expect(
    clippy::too_many_arguments,
    reason = "A split reads the rules, the place's prices and pops, and the building with its index and output."
)]
fn split_and_pay(
    rules: &Rules,
    place_id: &str,
    prices_milli: &[u64],
    pops: &mut [PopState],
    building: &mut BuildingState,
    definition: &BuildingType,
    building_index: usize,
    output: &[u64],
) -> Result<(u128, PendingOverflow), DataError> {
    let mut overflow = PendingOverflow::new();
    let wage_share_pm = u64::from(1000_u16.saturating_sub(definition.overhead_pm)) * 1000;
    let worker_weights = building
        .employment
        .iter()
        .map(|employment| employment.workers)
        .collect::<Vec<_>>();
    let mut wage_value = 0_u128;
    for (good_index, output_units) in output.iter().copied().enumerate() {
        let split_num = u128::from(output_units)
            .checked_mul(u128::from(wage_share_pm))
            .and_then(|value| value.checked_add(u128::from(building.wage_carries[good_index])))
            .ok_or_else(|| missing_data("payday.split".to_owned(), "wage split overflow"))?;
        let wage = u64::try_from(split_num / 1_000_000)
            .map_err(|_| missing_data("payday.split".to_owned(), "wage units exceed u64"))?;
        let price = prices_milli.get(good_index).copied().ok_or_else(|| {
            missing_data(
                format!("places.{place_id}.prices_milli"),
                "saved prices do not match goods",
            )
        })?;
        wage_value = wage_value
            .checked_add(value_at_price(u128::from(wage), price).map_err(|problem| {
                missing_data(
                    format!(
                        "places.{}.buildings.{}.wage_history",
                        place_id, building.kind
                    ),
                    problem,
                )
            })?)
            .ok_or_else(|| {
                missing_data(
                    format!(
                        "places.{}.buildings.{}.wage_history",
                        place_id, building.kind
                    ),
                    "payday wage value overflow",
                )
            })?;
        building.wage_carries[good_index] = u64::try_from(split_num % 1_000_000)
            .map_err(|_| missing_data("payday.split".to_owned(), "wage carry exceeds u64"))?;
        let overhead = output_units
            .checked_sub(wage)
            .ok_or_else(|| missing_data("payday.split".to_owned(), "wage exceeds output"))?;
        let unpaid = building.pay_carries[good_index]
            .checked_add(wage)
            .ok_or_else(|| missing_data("payday.carry".to_owned(), "pay carry overflow"))?;
        let mut contributions = Vec::new();
        if worker_weights.iter().all(|count| *count == 0) {
            building.pay_carries[good_index] = unpaid;
        } else {
            building.pay_carries[good_index] = 0;
            let shares = largest_remainder(unpaid, &worker_weights)
                .map_err(|problem| missing_data("payday.distribution".to_owned(), problem))?;
            for (pop_index, share) in shares.into_iter().enumerate() {
                if share == 0 {
                    continue;
                }
                let pop = pops.get_mut(pop_index).ok_or_else(|| {
                    missing_data("payday.worker".to_owned(), "worker pop is missing")
                })?;
                let overflow = receive_pop(rules, pop, good_index, share)?;
                if overflow > 0 {
                    contributions.push((Some(pop_index), overflow));
                }
            }
        }
        if overhead > 0 {
            contributions.insert(0, (None, overhead));
        }
        if !contributions.is_empty() {
            overflow.push((building_index, good_index, contributions));
        }
    }
    Ok((wage_value, overflow))
}

/// Run the first-of-month seasonal production phase, committing all places together.
///
/// Production uses each level's record: its method and ground, which a switch of methods changes (see
/// [`run_monthly_production_switching`]); the suitability sums are derived from them.
/// Fractional output is retained in each building's canonical carry state.
///
/// # Errors
/// Refuses malformed saved dimensions, missing seasonal data, or checked arithmetic overflow;
/// on refusal, `state` is unchanged.
pub fn run_monthly_production(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
) -> Result<ProductionLedger, DataError> {
    run_monthly_production_with_quality(rules, state, date, &BTreeMap::new())
}

/// Monthly production with map-derived farmland quality per place id.
///
/// # Errors
/// Refuses malformed saved state, missing seasonal data, or checked arithmetic overflow;
/// on refusal, `state` is unchanged.
pub fn run_monthly_production_with_quality(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
    quality_by_place: &BTreeMap<String, u16>,
) -> Result<ProductionLedger, DataError> {
    monthly_production(rules, state, date, quality_by_place, None)
}

/// The land counts of each place by id, and the place's prices as the day began, in place order: what the month's switching looks
/// at (the look's own inputs; none of it is saved).
type SwitchInputs<'a> = (&'a BTreeMap<String, LandCounts>, &'a [Vec<u64>]);

/// Monthly production followed, in each place with land counts, by the month's switching of methods: after the month's
/// production, herds and payday, and before anything else of the day. The look's prices are `day_prices`, the places' prices as
/// the day began, in place order.
///
/// # Errors
/// Refuses malformed saved state, missing seasonal data, or checked arithmetic overflow;
/// on refusal, `state` is unchanged.
pub fn run_monthly_production_switching(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
    quality_by_place: &BTreeMap<String, u16>,
    land_by_place: &BTreeMap<String, LandCounts>,
    day_prices: &[Vec<u64>],
) -> Result<ProductionLedger, DataError> {
    monthly_production(
        rules,
        state,
        date,
        quality_by_place,
        Some((land_by_place, day_prices)),
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "The monthly production and herd updates stay together inside the daily rollback boundary."
)]
fn monthly_production(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
    quality_by_place: &BTreeMap<String, u16>,
    switch_inputs: Option<SwitchInputs<'_>>,
) -> Result<ProductionLedger, DataError> {
    if date.day != 1 {
        return Ok(ProductionLedger::new(
            rules.goods.len(),
            rules.herd_species.len(),
        ));
    }
    if !(1..=12).contains(&date.month) {
        return Err(missing_data(
            "calendar.month".to_owned(),
            "month is outside 1..=12",
        ));
    }
    rules.validate("economy rules")?;
    let mut next = state.clone();
    let mut ledger = ProductionLedger::new(rules.goods.len(), rules.herd_species.len());
    for (place_index, place) in next.places.iter_mut().enumerate() {
        let quality_pm = quality_by_place
            .get(&place.id)
            .copied()
            .unwrap_or(rules.fertility_reference_pm);
        prepare_place_counters(&mut place.counters, rules.goods.len())?;
        roll_counter_month(&mut place.counters, Some(&place.prices_milli))?;
        for pop in &mut place.pops {
            roll_counter_month(&mut pop.counters, None)?;
        }
        let mut place_ledger =
            PlaceProductionLedger::new(&place.id, rules.goods.len(), rules.herd_species.len());
        place_ledger.stock_before = total_stored_goods(rules, place)?;
        place_ledger.herds_before = total_herds(place, rules.herd_species.len())?;
        let mut pending_overflow = Vec::<(usize, usize, Vec<(Option<usize>, u64)>)>::new();
        for (building_index, building) in place.buildings.iter_mut().enumerate() {
            let Some(definition) = rules.buildings.iter().find(|item| item.id == building.kind)
            else {
                return Err(missing_data(
                    format!("buildings.{}", building.kind),
                    "definition is missing",
                ));
            };
            if building.wage_history.len() != WAGE_HISTORY_MONTHS
                || building.paid_history.len() != WAGE_HISTORY_MONTHS
                || building.paid_years.len() != WAGE_HISTORY_MONTHS
            {
                return Err(missing_data(
                    format!(
                        "places.{}.buildings.{}.wage_history",
                        place.id, building.kind
                    ),
                    "saved wage history must have exactly 12 calendar months",
                ));
            }
            if definition.methods.is_empty() {
                continue;
            }
            let has_positions = building.levels != 0 && definition.workers_per_level != 0;
            if definition.methods.len() != building.method_levels.len()
                || (has_positions
                    && (building.production_carries.len() != definition.methods.len()
                        || building.method_suitability.len() != definition.methods.len()))
                || building.stock.len() != rules.goods.len()
            {
                return Err(missing_data(
                    format!("places.{}.buildings.{}", place.id, building.kind),
                    "saved method or good dimensions do not match rules",
                ));
            }
            let positions = u64::from(building.levels)
                .checked_mul(u64::from(definition.workers_per_level))
                .ok_or_else(|| {
                    missing_data("employment.positions".to_owned(), "position count overflow")
                })?;
            let workers = building
                .employment
                .iter()
                .try_fold(0_u64, |sum, item| sum.checked_add(item.workers))
                .ok_or_else(|| {
                    missing_data("employment.workers".to_owned(), "worker count overflow")
                })?;
            let staffed = workers.min(positions);
            let coverage = building_coverage_pm(definition, &place.pops, building)?;
            let coverage_efficiency = 1000_u64
                .checked_sub(
                    u64::from(definition.penalty_pm)
                        .checked_mul(1000 - u64::from(coverage))
                        .ok_or_else(|| {
                            missing_data(
                                "employment.coverage".to_owned(),
                                "coverage penalty overflow",
                            )
                        })?
                        / 1000,
                )
                .ok_or_else(|| {
                    missing_data(
                        "employment.coverage".to_owned(),
                        "coverage efficiency underflow",
                    )
                })?;
            // A payday counts itself first: each level's count for the day is its completed paydays and one more, at most
            // thirteen, and after the day that is the number it has completed.
            let level_groups = payday_groups(&place.id, building, definition)?;
            for record in &mut building.level_records {
                record.paydays = (record.paydays + 1).min(MATURE_PAYDAYS);
            }
            if positions == 0 {
                continue;
            }
            if building.wage_carries.len() != rules.goods.len()
                || building.pay_carries.len() != rules.goods.len()
                || building.stock.len() != rules.goods.len()
            {
                return Err(missing_data(
                    format!("places.{}.buildings.{}", place.id, building.kind),
                    "saved payday dimensions do not match goods",
                ));
            }
            let mut monthly_output = vec![0_u64; rules.goods.len()];
            for (method_index, method) in definition.methods.iter().enumerate() {
                if building.production_carries[method_index].len() != rules.goods.len()
                    || building.method_suitability[method_index].len() != rules.goods.len()
                {
                    return Err(missing_data(
                        format!(
                            "places.{}.buildings.{}.methods[{}]",
                            place.id, building.kind, method.id
                        ),
                        "saved carry or suitability dimensions do not match goods",
                    ));
                }
                let method_levels = building.method_levels[method_index];
                if method_levels == 0 {
                    continue;
                }
                for (good_id, annual) in &method.annual_output {
                    if herd_supplies(rules, definition, good_id) {
                        continue;
                    }
                    let good_index = rules
                        .goods
                        .iter()
                        .position(|good| &good.id == good_id)
                        .ok_or_else(|| {
                            missing_data(
                                format!("goods.{good_id}"),
                                "method output references unknown good",
                            )
                        })?;
                    let season_key = if ["oats", "barley", "peas_beans"].contains(&good_id.as_str())
                    {
                        "grain"
                    } else {
                        good_id.as_str()
                    };
                    let season = rules.seasons.get(season_key).ok_or_else(|| {
                        missing_data(format!("seasons.{season_key}"), "season table is missing")
                    })?;
                    let month_share = u128::from(
                        *season
                            .get(usize::from(date.month.saturating_sub(1)))
                            .ok_or_else(|| {
                                missing_data(
                                    format!("seasons.{good_id}"),
                                    "month is outside 1..=12",
                                )
                            })?,
                    );
                    for record in building
                        .level_records
                        .iter_mut()
                        .filter(|record| record.method as usize == method_index)
                    {
                        let level_suitability = method_suitability_pm(
                            rules,
                            definition,
                            method,
                            good_id,
                            record.ground.as_deref(),
                        );
                        let (quality, reference) = if method.resource == "usable_land" {
                            (quality_pm, rules.fertility_reference_pm)
                        } else {
                            (1, 1)
                        };
                        let account = record.unpaid_work.entry(good_id.clone()).or_default();
                        accrue_unpaid_work(
                            account,
                            *annual,
                            level_suitability,
                            record.paydays,
                            month_share,
                            staffed,
                            positions,
                            coverage_efficiency,
                            quality,
                            reference,
                        );
                    }
                    // Each level's suitability for the good on its own ground is multiplied by its count for the day
                    // less one (its share, in twelfths) before the levels are added.
                    let suitability = level_groups[method_index]
                        .iter()
                        .try_fold(0_u128, |sum, ((ground, count), levels)| {
                            let level_suitability = u128::from(method_suitability_pm(
                                rules,
                                definition,
                                method,
                                good_id,
                                ground.as_deref(),
                            ));
                            sum.checked_add(
                                level_suitability
                                    .checked_mul(u128::from(*count - 1))?
                                    .checked_mul(u128::from(*levels))?,
                            )
                        })
                        .ok_or_else(|| {
                            missing_data(
                                format!(
                                    "places.{}.buildings.{}.production.{good_id}",
                                    place.id, building.kind
                                ),
                                "level suitability sum overflow",
                            )
                        })?;
                    let farming = method.resource == "usable_land";
                    let (quality_factor, reference_factor) = if farming {
                        reduced_ratio(quality_pm, rules.fertility_reference_pm)
                    } else {
                        (1, 1)
                    };
                    // Twelve times the old denominator: the level counts are in twelfths.
                    let denominator = u128::from(positions)
                        .checked_mul(12_000_000_000)
                        .and_then(|value| value.checked_mul(u128::from(reference_factor)))
                        .ok_or_else(|| {
                            missing_data(
                                "production.denominator".to_owned(),
                                "production denominator overflow",
                            )
                        })?;
                    let (quotient, remainder) = payday_division(
                        &[
                            u128::from(*annual),
                            suitability,
                            month_share,
                            u128::from(staffed),
                            u128::from(coverage_efficiency),
                            u128::from(quality_factor),
                        ],
                        building.production_carries[method_index][good_index],
                        denominator,
                    )
                    .ok_or_else(|| {
                        missing_data(
                            format!(
                                "places.{}.buildings.{}.production.{good_id}",
                                place.id, building.kind
                            ),
                            "monthly output numerator overflow",
                        )
                    })?;
                    let output = u64::try_from(quotient).map_err(|_| {
                        missing_data(
                            format!(
                                "places.{}.buildings.{}.production.{good_id}",
                                place.id, building.kind
                            ),
                            "monthly output exceeds u64",
                        )
                    })?;
                    building.production_carries[method_index][good_index] = remainder;
                    monthly_output[good_index] = monthly_output[good_index]
                        .checked_add(output)
                        .ok_or_else(|| {
                            missing_data(
                                format!(
                                    "places.{}.buildings.{}.stock.{good_id}",
                                    place.id, building.kind
                                ),
                                "monthly stock overflow",
                            )
                        })?;
                }
            }
            if !building.herds.is_empty() {
                let species_count = rules.herd_species.len();
                let product_count: usize = rules
                    .herd_species
                    .iter()
                    .map(|species| species.products.len())
                    .sum();
                if building.herds.len() != species_count
                    || building.herd_birth_carries.len() != species_count
                    || building.herd_death_carries.len() != species_count
                    || building.herd_product_carries.len() != product_count
                    || building.herd_grazing_sums.len() != species_count
                {
                    return Err(missing_data(
                        format!("places.{}.buildings.{}.herds", place.id, building.kind),
                        "herd and carry dimensions do not match species",
                    ));
                }
                let birth_season = rules.seasons.get("herd_births").ok_or_else(|| {
                    missing_data("seasons.herd_births".to_owned(), "season table is missing")
                })?;
                let month_share = u128::from(
                    *birth_season
                        .get(usize::from(date.month.saturating_sub(1)))
                        .ok_or_else(|| {
                            missing_data(
                                "seasons.herd_births".to_owned(),
                                "month is outside 1..=12",
                            )
                        })?,
                );
                let farming_levels = definition
                    .methods
                    .iter()
                    .zip(&building.method_levels)
                    .filter(|(method, _)| method.resource != "sea")
                    .try_fold(0_u64, |sum, (_, levels)| {
                        sum.checked_add(u64::from(*levels))
                    })
                    .ok_or_else(|| {
                        missing_data("herds".to_owned(), "farming level sum overflow")
                    })?;
                for (species, species_rules) in rules.herd_species.iter().enumerate() {
                    let Some(capacity) = definition.herds.get(&species_rules.animal) else {
                        continue;
                    };
                    let herd = building.herds[species];
                    let birth_num = u128::from(herd)
                        .checked_mul(u128::from(species_rules.births_pm_year))
                        .and_then(|value| {
                            let grazing = if farming_levels == 0 {
                                0
                            } else {
                                building.herd_grazing_sums[species] / farming_levels
                            };
                            value.checked_mul(u128::from(grazing))
                        })
                        .and_then(|value| value.checked_mul(month_share))
                        .and_then(|value| value.checked_mul(u128::from(coverage_efficiency)))
                        .and_then(|value| {
                            value.checked_add(u128::from(building.herd_birth_carries[species]))
                        })
                        .ok_or_else(|| {
                            missing_data("herds.births".to_owned(), "birth numerator overflow")
                        })?;
                    let births = u64::try_from(birth_num / 1_000_000_000_000).map_err(|_| {
                        missing_data("herds.births".to_owned(), "birth count exceeds u64")
                    })?;
                    place_ledger.herd_births[species] = place_ledger.herd_births[species]
                        .checked_add(births)
                        .ok_or_else(|| {
                            missing_data("ledger.herds".to_owned(), "birth total overflow")
                        })?;
                    building.herd_birth_carries[species] =
                        u64::try_from(birth_num % 1_000_000_000_000).map_err(|_| {
                            missing_data("herds.births".to_owned(), "birth carry exceeds u64")
                        })?;
                    let death_num = u128::from(herd)
                        .checked_mul(u128::from(species_rules.deaths_pm_year))
                        .and_then(|value| {
                            value.checked_add(u128::from(building.herd_death_carries[species]))
                        })
                        .ok_or_else(|| {
                            missing_data("herds.deaths".to_owned(), "death numerator overflow")
                        })?;
                    let deaths = u64::try_from((death_num / 12_000).min(u128::from(herd)))
                        .map_err(|_| {
                            missing_data("herds.deaths".to_owned(), "death count exceeds u64")
                        })?;
                    place_ledger.herd_deaths[species] = place_ledger.herd_deaths[species]
                        .checked_add(deaths)
                        .ok_or_else(|| {
                            missing_data("ledger.herds".to_owned(), "death total overflow")
                        })?;
                    building.herd_death_carries[species] = u64::try_from(death_num % 12_000)
                        .map_err(|_| {
                            missing_data("herds.deaths".to_owned(), "death carry exceeds u64")
                        })?;
                    let after_births = herd
                        .checked_add(births)
                        .ok_or_else(|| missing_data("herds".to_owned(), "herd count overflow"))?;
                    let surviving = after_births.saturating_sub(deaths);
                    let maximum = farming_levels
                        .checked_mul(capacity.maximum)
                        .ok_or_else(|| missing_data("herds".to_owned(), "herd maximum overflow"))?;
                    let slaughtered = surviving.saturating_sub(maximum);
                    place_ledger.herd_slaughter[species] = place_ledger.herd_slaughter[species]
                        .checked_add(slaughtered)
                        .ok_or_else(|| {
                            missing_data("ledger.herds".to_owned(), "slaughter total overflow")
                        })?;
                    building.herds[species] = surviving - slaughtered;
                    for yield_item in &species_rules.yields {
                        let good_id = &yield_item.good;
                        let yield_per_head = yield_item.units;
                        let Some(good_index) =
                            rules.goods.iter().position(|good| good.id == *good_id)
                        else {
                            continue;
                        };
                        let output = slaughtered.checked_mul(yield_per_head).ok_or_else(|| {
                            missing_data(
                                format!("herds.slaughter.{good_id}"),
                                "slaughter yield overflow",
                            )
                        })?;
                        monthly_output[good_index] = monthly_output[good_index]
                            .checked_add(output)
                            .ok_or_else(|| {
                                missing_data(
                                    format!(
                                        "places.{}.buildings.{}.stock.{good_id}",
                                        place.id, building.kind
                                    ),
                                    "slaughter stock overflow",
                                )
                            })?;
                    }
                }
            }
            if !building.herds.is_empty() {
                let mut carry_index = 0;
                for (species_index, species) in rules.herd_species.iter().enumerate() {
                    for product in &species.products {
                        let slot = carry_index;
                        carry_index += 1;
                        if !definition.herds.contains_key(&species.animal) {
                            continue;
                        }
                        let (share, denominator) = if let Some(season) = &product.season {
                            (
                                u128::from(rules.seasons[season][usize::from(date.month - 1)]),
                                1_000_000_000_u128,
                            )
                        } else if product.month == Some(date.month) {
                            (1, 1_000_000_u128)
                        } else {
                            continue;
                        };
                        let good_index = index_for_good(rules, &product.good)?;
                        let numerator = u128::from(building.herds[species_index])
                            .checked_mul(u128::from(product.amount_milli.ok_or_else(|| {
                                missing_data(
                                    format!("herds.products.{}.amount_milli", product.good),
                                    "amount is missing",
                                )
                            })?))
                            .and_then(|v| v.checked_mul(share))
                            .and_then(|v| v.checked_mul(u128::from(coverage_efficiency)))
                            .and_then(|v| {
                                v.checked_add(u128::from(building.herd_product_carries[slot]))
                            })
                            .ok_or_else(|| {
                                missing_data(
                                    format!("herds.products.{}", product.good),
                                    "product numerator overflow",
                                )
                            })?;
                        let quantity = u64::try_from(numerator / denominator).map_err(|_| {
                            missing_data(
                                format!("herds.products.{}", product.good),
                                "product exceeds u64",
                            )
                        })?;
                        building.herd_product_carries[slot] =
                            u64::try_from(numerator % denominator).map_err(|_| {
                                missing_data(
                                    format!("herds.products.{}", product.good),
                                    "product carry exceeds u64",
                                )
                            })?;
                        monthly_output[good_index] = monthly_output[good_index]
                            .checked_add(quantity)
                            .ok_or_else(|| {
                                missing_data(
                                    format!("herds.products.{}", product.good),
                                    "product stock overflow",
                                )
                            })?;
                    }
                }
            }
            for (good_index, output_units) in monthly_output.iter().copied().enumerate() {
                add_counter_units(
                    &mut place.counters.current.produced,
                    rules.goods.len(),
                    good_index,
                    output_units,
                    "counters.produced",
                )?;
                place_ledger.produced[good_index] = place_ledger.produced[good_index]
                    .checked_add(output_units)
                    .ok_or_else(|| {
                        missing_data("ledger.production".to_owned(), "production total overflow")
                    })?;
            }
            let (wage_value, overflow) = split_and_pay(
                rules,
                &place.id,
                &place.prices_milli,
                &mut place.pops,
                building,
                definition,
                building_index,
                &monthly_output,
            )?;
            pending_overflow.extend(overflow);
            if workers > 0 {
                let posted = u64::try_from(wage_value / u128::from(workers)).map_err(|_| {
                    missing_data(
                        format!(
                            "places.{}.buildings.{}.wage_history",
                            place.id, building.kind
                        ),
                        "posted payday wage exceeds u64",
                    )
                })?;
                building.wage_history[usize::from(date.month - 1)] = posted;
                building.paid_history[usize::from(date.month - 1)] = workers;
                building.paid_years[usize::from(date.month - 1)] = date.year;
            }
        }
        for (building_index, good_index, contributions) in pending_overflow {
            settle_overflow(
                rules,
                &mut place.pops,
                &mut place.buildings,
                building_index,
                good_index,
                &contributions,
                &mut place.counters,
                &mut place_ledger,
            )?;
        }
        if let Some((land_by_place, day_prices)) = switch_inputs
            && let Some(land) = land_by_place.get(&place.id)
        {
            let prices = day_prices.get(place_index).ok_or_else(|| {
                missing_data(
                    format!("places.{}.prices_milli", place.id),
                    "the day's prices do not cover every place",
                )
            })?;
            switching::run_place_switches(rules, place, land, date, prices, &mut place_ledger)?;
        }
        place_ledger.stock_after = total_stored_goods(rules, place)?;
        place_ledger.herds_after = total_herds(place, rules.herd_species.len())?;
        for building in &place.buildings {
            if building.pay_carries.len() != rules.goods.len() {
                return Err(missing_data(
                    format!(
                        "places.{}.buildings.{}.pay_carries",
                        place.id, building.kind
                    ),
                    "saved payday dimensions do not match goods",
                ));
            }
            for (total, unpaid) in place_ledger
                .unpaid_wages
                .iter_mut()
                .zip(&building.pay_carries)
            {
                *total = total.checked_add(*unpaid).ok_or_else(|| {
                    missing_data("ledger.unpaid_wages".to_owned(), "unpaid total overflow")
                })?;
            }
        }
        for (total, part) in ledger.produced.iter_mut().zip(&place_ledger.produced) {
            *total = total.checked_add(*part).ok_or_else(|| {
                missing_data("ledger.production".to_owned(), "production total overflow")
            })?;
        }
        for (total, part) in ledger.lost.iter_mut().zip(&place_ledger.lost) {
            *total = total
                .checked_add(*part)
                .ok_or_else(|| missing_data("ledger.loss".to_owned(), "loss total overflow"))?;
        }
        for (total, part) in ledger.herd_births.iter_mut().zip(&place_ledger.herd_births) {
            *total = total
                .checked_add(*part)
                .ok_or_else(|| missing_data("ledger.herds".to_owned(), "birth total overflow"))?;
        }
        for (total, part) in ledger.herd_deaths.iter_mut().zip(&place_ledger.herd_deaths) {
            *total = total
                .checked_add(*part)
                .ok_or_else(|| missing_data("ledger.herds".to_owned(), "death total overflow"))?;
        }
        for (total, part) in ledger
            .herd_slaughter
            .iter_mut()
            .zip(&place_ledger.herd_slaughter)
        {
            *total = total.checked_add(*part).ok_or_else(|| {
                missing_data("ledger.herds".to_owned(), "slaughter total overflow")
            })?;
        }
        for (total, part) in ledger
            .herd_arrivals
            .iter_mut()
            .zip(&place_ledger.herd_arrivals)
        {
            *total = total
                .checked_add(*part)
                .ok_or_else(|| missing_data("ledger.herds".to_owned(), "arrival total overflow"))?;
        }
        for (total, part) in ledger
            .stock_before
            .iter_mut()
            .zip(&place_ledger.stock_before)
        {
            *total = total.checked_add(*part).ok_or_else(|| {
                missing_data("ledger.stock".to_owned(), "opening stock total overflow")
            })?;
        }
        for (total, part) in ledger.stock_after.iter_mut().zip(&place_ledger.stock_after) {
            *total = total.checked_add(*part).ok_or_else(|| {
                missing_data("ledger.stock".to_owned(), "closing stock total overflow")
            })?;
        }
        for (total, part) in ledger
            .unpaid_wages
            .iter_mut()
            .zip(&place_ledger.unpaid_wages)
        {
            *total = total.checked_add(*part).ok_or_else(|| {
                missing_data("ledger.unpaid_wages".to_owned(), "unpaid total overflow")
            })?;
        }
        for (total, part) in ledger
            .herds_before
            .iter_mut()
            .zip(&place_ledger.herds_before)
        {
            *total = total.checked_add(*part).ok_or_else(|| {
                missing_data("ledger.herds".to_owned(), "opening herd total overflow")
            })?;
        }
        for (total, part) in ledger.herds_after.iter_mut().zip(&place_ledger.herds_after) {
            *total = total.checked_add(*part).ok_or_else(|| {
                missing_data("ledger.herds".to_owned(), "closing herd total overflow")
            })?;
        }
        ledger.places.push(place_ledger);
    }
    *state = next;
    Ok(ledger)
}

/// Run one noon economy tick: prices, first-of-month production, then drawing, eating and needs.
/// The state commits only when both phases succeed.
///
/// # Errors
/// Returns the first named production or consumption data error and leaves `state` unchanged.
pub fn run_daily_economy(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
) -> Result<ProductionLedger, DataError> {
    run_daily_economy_with_quality(rules, state, date, &BTreeMap::new())
}

/// Daily economy tick with the map-derived farmland quality for each place.
///
/// # Errors
/// Returns the first named production or consumption data error and leaves `state` unchanged.
pub fn run_daily_economy_with_quality(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
    quality_by_place: &BTreeMap<String, u16>,
) -> Result<ProductionLedger, DataError> {
    run_daily_economy_with_context(
        rules,
        state,
        date,
        quality_by_place,
        &BTreeMap::new(),
        &BTreeMap::new(),
    )
}

/// Daily economy tick with map-derived farmland quality, natural shelter and neighbour order.
///
/// # Errors
/// Returns the first named production, consumption, demographic, or checked-arithmetic data error and leaves `state` unchanged.
pub fn run_daily_economy_with_context(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
    quality_by_place: &BTreeMap<String, u16>,
    shelter_by_place: &BTreeMap<String, u64>,
    nearby_by_place: &BTreeMap<String, Vec<NearbyPlace>>,
) -> Result<ProductionLedger, DataError> {
    run_daily_economy_with_floors(
        rules,
        state,
        date,
        quality_by_place,
        shelter_by_place,
        nearby_by_place,
        &BTreeMap::new(),
    )
}

/// Daily economy with live-character floors by place and stable pop slot.
/// # Errors
/// Returns the first data error without changing the economy.
pub fn run_daily_economy_with_floors(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
    quality_by_place: &BTreeMap<String, u16>,
    shelter_by_place: &BTreeMap<String, u64>,
    nearby_by_place: &BTreeMap<String, Vec<NearbyPlace>>,
    floors: &BTreeMap<(String, usize), u64>,
) -> Result<ProductionLedger, DataError> {
    run_daily_economy_with_land(
        rules,
        state,
        date,
        quality_by_place,
        shelter_by_place,
        nearby_by_place,
        floors,
        &BTreeMap::new(),
    )
}

/// Daily economy with live-character floors and each place's land counts: where a place has them, its buildings look at their
/// levels on the 1st and move at most one level a month (CK-409). A place with no land counts does not switch.
/// # Errors
/// Returns the first data error without changing the economy.
#[expect(
    clippy::too_many_arguments,
    reason = "The daily step takes each place's map-derived inputs and the characters' floors."
)]
pub fn run_daily_economy_with_land(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
    quality_by_place: &BTreeMap<String, u16>,
    shelter_by_place: &BTreeMap<String, u64>,
    nearby_by_place: &BTreeMap<String, Vec<NearbyPlace>>,
    floors: &BTreeMap<(String, usize), u64>,
    land_by_place: &BTreeMap<String, LandCounts>,
) -> Result<ProductionLedger, DataError> {
    run_daily_economy_with_trade_neighbours(
        rules,
        state,
        date,
        quality_by_place,
        shelter_by_place,
        nearby_by_place,
        floors,
        land_by_place,
        &BTreeMap::new(),
    )
}

/// Daily economy with direct trade between the supplied neighbouring places.
/// # Errors
/// Returns the first data error without changing the economy.
#[expect(
    clippy::too_many_arguments,
    reason = "The daily step needs the separate map-derived trade and recruitment tables."
)]
pub fn run_daily_economy_with_trade_neighbours(
    rules: &Rules,
    state: &mut EconomyState,
    date: Date,
    quality_by_place: &BTreeMap<String, u16>,
    shelter_by_place: &BTreeMap<String, u64>,
    nearby_by_place: &BTreeMap<String, Vec<NearbyPlace>>,
    floors: &BTreeMap<(String, usize), u64>,
    land_by_place: &BTreeMap<String, LandCounts>,
    trade_by_place: &BTreeMap<String, Vec<NearbyPlace>>,
) -> Result<ProductionLedger, DataError> {
    let day_prices: Vec<Vec<u64>> = state
        .places
        .iter()
        .map(|place| place.prices_milli.clone())
        .collect();
    let mut next = state.clone();
    update_daily_prices(rules, &mut next)?;
    let mut ledger = monthly_production(
        rules,
        &mut next,
        date,
        quality_by_place,
        Some((land_by_place, &day_prices)),
    )?;
    // The exchange uses moved prices; fuel uses the price as the day began.
    swap::run_neighbour_swaps_at_prices(rules, &mut next, &day_prices, trade_by_place)?;
    let (intake_pm, daily_consumption) =
        run_daily_consumption_at_prices(rules, &mut next, date, &day_prices)?;
    ledger.intake_pm = intake_pm;
    ledger.daily_consumption = Some(daily_consumption);
    if date.day == 1 {
        ledger.movement_by_pop = run_monthly_demography_with_floors(
            rules,
            &mut next,
            shelter_by_place,
            nearby_by_place,
            Some((usize::from(date.month - 1), date.year, &mut ledger)),
            floors,
        )?;
        // Intake is measured before the month's demographic pass. A recruited pop did not
        // exist during that day's consumption, so keep the ledger aligned with the new state
        // and report zero intake for that new row.
        for (place_index, place) in next.places.iter().enumerate() {
            let intake = ledger.intake_pm.get_mut(place_index).ok_or_else(|| {
                missing_data(
                    "ledger.intake_pm".to_owned(),
                    "place dimensions do not match",
                )
            })?;
            intake.resize(place.pops.len(), 0);
        }
    }
    *state = next;
    Ok(ledger)
}

/// Validate saved fuel balances before a world is installed.
///
/// # Errors
/// Returns the named pop and invalid balance, total, or carry.
pub fn validate_fuel_state(rules: &Rules, state: &EconomyState) -> Result<(), DataError> {
    let max_supply = rules
        .goods
        .iter()
        .map(|good| good.category_supplied("fuel", 1))
        .max()
        .unwrap_or(0);
    for place in &state.places {
        for (index, pop) in place.pops.iter().enumerate() {
            let entry = format!("places.{}.pops.{index}.fuel", place.id);
            let fuel = &pop.consumption;
            if (max_supply == 0 && fuel.hearth_milli != 0)
                || (max_supply > 0 && u128::from(fuel.hearth_milli) >= max_supply)
            {
                return Err(missing_data(
                    format!("{entry}.hearth"),
                    &format!("invalid balance {}", fuel.hearth_milli),
                ));
            }
            if fuel.fuel_burned_month > fuel.fuel_need_month {
                return Err(missing_data(
                    format!("{entry}.burned"),
                    &format!(
                        "{} exceeds need {}",
                        fuel.fuel_burned_month, fuel.fuel_need_month
                    ),
                ));
            }
            if fuel.fuel_death_carry >= 12_000_000 {
                return Err(missing_data(
                    format!("{entry}.death_carry"),
                    &format!("invalid carry {}", fuel.fuel_death_carry),
                ));
            }
        }
    }
    Ok(())
}

/// A nearby destination in deterministic nearest-first order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NearbyPlace {
    /// Destination place id.
    pub id: String,
    /// Approximate distance in ground metres.
    pub distance_m: u32,
}

fn exact_monthly_rate(
    pool: u64,
    base_pm_year: u16,
    factors: &[u64],
    carry: &mut u64,
    entry: &str,
) -> Result<u64, DataError> {
    let mut numerator = u128::from(base_pm_year);
    for factor in factors {
        numerator = numerator
            .checked_mul(u128::from(*factor))
            .ok_or_else(|| missing_data(entry.to_owned(), "monthly rate numerator overflow"))?;
    }
    let exponent = u32::try_from(factors.len() + 1)
        .map_err(|_| missing_data(entry.to_owned(), "factor count exceeds u32"))?;
    let denominator =
        12_u128
            .checked_mul(1000_u128.checked_pow(exponent).ok_or_else(|| {
                missing_data(entry.to_owned(), "monthly rate denominator overflow")
            })?)
            .ok_or_else(|| missing_data(entry.to_owned(), "monthly rate denominator overflow"))?;
    if u128::from(*carry) >= denominator {
        return Err(missing_data(
            entry.to_owned(),
            "saved monthly rate carry is out of range",
        ));
    }
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let fractional = u128::from(pool)
        .checked_mul(remainder)
        .and_then(|value| value.checked_add(u128::from(*carry)))
        .ok_or_else(|| missing_data(entry.to_owned(), "monthly rate fraction overflow"))?;
    let whole = u128::from(pool)
        .checked_mul(quotient)
        .and_then(|value| value.checked_add(fractional / denominator))
        .ok_or_else(|| missing_data(entry.to_owned(), "monthly rate whole count overflow"))?;
    *carry = u64::try_from(fractional % denominator)
        .map_err(|_| missing_data(entry.to_owned(), "monthly rate carry exceeds u64"))?;
    u64::try_from(whole).map_err(|_| missing_data(entry.to_owned(), "monthly rate exceeds u64"))
}

fn release_excess_employment(place: &mut PlaceState) -> Result<(), DataError> {
    for pop_index in 0..place.pops.len() {
        let worker_count = place.pops[pop_index].workers;
        let owners = place
            .buildings
            .iter()
            .map(|building| {
                building
                    .employment
                    .get(pop_index)
                    .map_or(0, |job| job.owners)
            })
            .collect::<Vec<_>>();
        let workers = place
            .buildings
            .iter()
            .map(|building| {
                building
                    .employment
                    .get(pop_index)
                    .map_or(0, |job| job.workers)
            })
            .collect::<Vec<_>>();
        let total_owners = owners.iter().try_fold(0_u64, |sum, value| {
            sum.checked_add(*value)
                .ok_or_else(|| missing_data("employment.owners".to_owned(), "owner total overflow"))
        })?;
        let keep_owners = total_owners.min(worker_count);
        let owner_awards = give(keep_owners, &owners)?;
        let remaining_workers = worker_count - keep_owners;
        let keep_workers = workers.iter().sum::<u64>().min(remaining_workers);
        let worker_awards = give(keep_workers, &workers)?;
        for building_index in 0..place.buildings.len() {
            let job = place.buildings[building_index]
                .employment
                .get_mut(pop_index)
                .ok_or_else(|| {
                    missing_data(
                        "employment".to_owned(),
                        "employment dimensions do not match",
                    )
                })?;
            job.owners = owner_awards[building_index];
            job.workers = worker_awards[building_index];
            place.pops[pop_index].employment[building_index] = job.clone();
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "The owner, storehouse-worker, and ordinary-worker passes share one explicit per-building order before movement."
)]
fn fill_monthly_hiring(rules: &Rules, place: &mut PlaceState) -> Result<Vec<[u64; 2]>, DataError> {
    let mut movement_allowance = vec![[0_u64; 2]; place.buildings.len()];
    for (building_index, movement_budget) in movement_allowance.iter_mut().enumerate() {
        let kind = place.buildings[building_index].kind.clone();
        let definition = rules
            .buildings
            .iter()
            .find(|definition| definition.id == kind)
            .ok_or_else(|| missing_data(format!("buildings.{kind}"), "definition is missing"))?;
        let level_positions = u64::from(place.buildings[building_index].levels)
            .checked_mul(u64::from(definition.workers_per_level))
            .ok_or_else(|| {
                missing_data(format!("buildings.{kind}.positions"), "capacity overflow")
            })?;

        let mut owner_kinds = definition.owner_workers.keys().cloned().collect::<Vec<_>>();
        owner_kinds.sort_by_key(|pop_kind| match pop_kind.as_str() {
            "monk" => 0,
            "labourer" => 1,
            "peasant" => 2,
            _ => 3,
        });
        if kind == "holdings" {
            let mut needed = 0_u64;
            for owner_kind in &owner_kinds {
                let ratio = u64::from(definition.owner_workers[owner_kind]);
                if ratio == 0 {
                    return Err(missing_data(
                        format!("buildings.{kind}.owner_workers.{owner_kind}"),
                        "workers per owner must be positive",
                    ));
                }
                let paid = place.buildings[building_index]
                    .employment
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| place.pops[*index].kind == *owner_kind)
                    .try_fold(0_u64, |sum, (_, job)| sum.checked_add(job.workers))
                    .ok_or_else(|| {
                        missing_data("employment.workers".to_owned(), "worker total overflow")
                    })?;
                let kind_need = paid.checked_add(ratio - 1).ok_or_else(|| {
                    missing_data("employment.owners".to_owned(), "owner need overflow")
                })? / ratio;
                needed = needed.checked_add(kind_need).ok_or_else(|| {
                    missing_data("employment.owners".to_owned(), "owner need overflow")
                })?;
            }
            let owners = place.buildings[building_index]
                .employment
                .iter()
                .try_fold(0_u64, |sum, job| sum.checked_add(job.owners))
                .ok_or_else(|| {
                    missing_data("employment.owners".to_owned(), "owner total overflow")
                })?;
            let mut remaining = hiring_cap(needed.saturating_sub(owners), rules.hire_pm)?;
            for pop_index in 0..place.pops.len() {
                if place.pops[pop_index].kind != "monk" || remaining == 0 {
                    continue;
                }
                let hire = unemployed_workers(place, pop_index)?.min(remaining);
                assign_owner(place, building_index, pop_index, hire)?;
                remaining -= hire;
            }
        } else {
            for owner_kind in &owner_kinds {
                let ratio = u64::from(definition.owner_workers[owner_kind]);
                if ratio == 0 {
                    return Err(missing_data(
                        format!("buildings.{kind}.owner_workers.{owner_kind}"),
                        "workers per owner must be positive",
                    ));
                }
                let paid = place.buildings[building_index]
                    .employment
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| place.pops[*index].kind == *owner_kind)
                    .map(|(_, job)| job.workers)
                    .try_fold(0_u64, |sum, value| {
                        sum.checked_add(value).ok_or_else(|| {
                            missing_data("employment.workers".to_owned(), "worker total overflow")
                        })
                    })?;
                let needed = paid.checked_add(ratio - 1).ok_or_else(|| {
                    missing_data("employment.owners".to_owned(), "owner need overflow")
                })? / ratio;
                // The owners are drawn from the building's owner kind when it names one (the
                // demesne's workers are labourers, its owners nobles), else from the worker kind.
                let owner_pop_kind = definition.owner_kind.as_ref().unwrap_or(owner_kind);
                let owners = place.buildings[building_index]
                    .employment
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| place.pops[*index].kind == *owner_pop_kind)
                    .map(|(_, job)| job.owners)
                    .try_fold(0_u64, |sum, value| {
                        sum.checked_add(value).ok_or_else(|| {
                            missing_data("employment.owners".to_owned(), "owner total overflow")
                        })
                    })?;
                let open = needed.saturating_sub(owners);
                let monthly_cap = hiring_cap(open, rules.hire_pm)?;
                let mut remaining = monthly_cap;
                for pop_index in 0..place.pops.len() {
                    if place.pops[pop_index].kind != *owner_pop_kind || remaining == 0 {
                        continue;
                    }
                    let available = unemployed_workers(place, pop_index)?;
                    let hire = available.min(remaining);
                    assign_owner(place, building_index, pop_index, hire)?;
                    remaining -= hire;
                }
            }
        }

        if kind == "storehouse" {
            let filled =
                place.buildings[building_index]
                    .employment
                    .iter()
                    .try_fold(0_u64, |sum, job| {
                        sum.checked_add(job.workers).ok_or_else(|| {
                            missing_data("employment.workers".to_owned(), "worker total overflow")
                        })
                    })?;
            let mut remaining = hiring_cap(level_positions.saturating_sub(filled), rules.hire_pm)?;
            for pop_index in 0..place.pops.len() {
                if remaining == 0
                    || !owns_served_building(&place.pops[pop_index].employment, building_index)
                {
                    continue;
                }
                let available = unemployed_workers(place, pop_index)?;
                let hire = available.min(remaining);
                assign_worker(place, building_index, pop_index, hire)?;
                remaining -= hire;
            }
            continue;
        }

        let mut position_kinds = owner_kinds;
        if position_kinds.is_empty() {
            continue;
        }
        let per_kind = if position_kinds.len() == 1 {
            vec![level_positions]
        } else {
            let base = level_positions / position_kinds.len() as u64;
            let remainder = level_positions % position_kinds.len() as u64;
            (0..position_kinds.len())
                .map(|index| base + u64::from((index as u64) < remainder))
                .collect()
        };
        for (position_kind, capacity) in position_kinds.drain(..).zip(per_kind) {
            let filled = place.buildings[building_index]
                .employment
                .iter()
                .enumerate()
                .filter(|(index, _)| place.pops[*index].kind == position_kind)
                .try_fold(0_u64, |sum, (_, job)| {
                    sum.checked_add(job.workers).ok_or_else(|| {
                        missing_data("employment.workers".to_owned(), "worker total overflow")
                    })
                })?;
            let monthly_cap = hiring_cap(capacity.saturating_sub(filled), rules.hire_pm)?;
            let mut remaining = monthly_cap;
            for pop_index in 0..place.pops.len() {
                if place.pops[pop_index].kind != position_kind || remaining == 0 {
                    continue;
                }
                let available = unemployed_workers(place, pop_index)?;
                let hire = available.min(remaining);
                assign_worker(place, building_index, pop_index, hire)?;
                remaining -= hire;
            }
            match position_kind.as_str() {
                "monk" => movement_budget[0] = remaining,
                "labourer" => movement_budget[1] = remaining,
                _ => {}
            }
        }
    }
    Ok(movement_allowance)
}

fn hiring_cap(open: u64, hire_pm: u16) -> Result<u64, DataError> {
    if open == 0 {
        return Ok(0);
    }
    let rate = u64::from(hire_pm);
    let scaled = open.checked_mul(rate).ok_or_else(|| {
        missing_data(
            "employment.hire_pm".to_owned(),
            "monthly hire count overflow",
        )
    })? / 1000;
    Ok(scaled.max(1))
}

fn unemployed_workers(place: &PlaceState, pop_index: usize) -> Result<u64, DataError> {
    let pop = &place.pops[pop_index];
    let employed = pop.employment.iter().try_fold(0_u64, |sum, job| {
        sum.checked_add(job.owners)
            .and_then(|sum| sum.checked_add(job.workers))
            .ok_or_else(|| missing_data("employment".to_owned(), "employment total overflow"))
    })?;
    pop.workers
        .checked_sub(employed)
        .ok_or_else(|| missing_data("employment".to_owned(), "employed heads exceed pop workers"))
}

fn owns_served_building(employment: &[Employment], before_building: usize) -> bool {
    employment
        .iter()
        .take(before_building)
        .any(|job| job.owners > 0)
}

fn assign_owner(
    place: &mut PlaceState,
    building_index: usize,
    pop_index: usize,
    count: u64,
) -> Result<(), DataError> {
    place.buildings[building_index].employment[pop_index].owners = place.buildings[building_index]
        .employment[pop_index]
        .owners
        .checked_add(count)
        .ok_or_else(|| missing_data("employment.owners".to_owned(), "owner count overflow"))?;
    place.pops[pop_index].employment[building_index].owners = place.pops[pop_index].employment
        [building_index]
        .owners
        .checked_add(count)
        .ok_or_else(|| missing_data("employment.owners".to_owned(), "owner count overflow"))?;
    Ok(())
}

fn assign_worker(
    place: &mut PlaceState,
    building_index: usize,
    pop_index: usize,
    count: u64,
) -> Result<(), DataError> {
    place.buildings[building_index].employment[pop_index].workers = place.buildings[building_index]
        .employment[pop_index]
        .workers
        .checked_add(count)
        .ok_or_else(|| missing_data("employment.workers".to_owned(), "worker count overflow"))?;
    place.pops[pop_index].employment[building_index].workers = place.pops[pop_index].employment
        [building_index]
        .workers
        .checked_add(count)
        .ok_or_else(|| missing_data("employment.workers".to_owned(), "worker count overflow"))?;
    Ok(())
}

fn pop_wage_per_head(place: &PlaceState, pop_index: usize) -> Result<u64, DataError> {
    let workers = place.pops[pop_index].workers;
    if workers == 0 {
        return Ok(0);
    }
    let mut total = 0_u128;
    for (building, employment) in place
        .buildings
        .iter()
        .zip(&place.pops[pop_index].employment)
    {
        total = total
            .checked_add(
                u128::from(employment.workers)
                    .checked_mul(u128::from(building.posted_wage()))
                    .ok_or_else(|| {
                        missing_data("movement.wage".to_owned(), "wage total overflow")
                    })?,
            )
            .ok_or_else(|| missing_data("movement.wage".to_owned(), "wage total overflow"))?;
    }
    u64::try_from(total / u128::from(workers))
        .map_err(|_| missing_data("movement.wage".to_owned(), "wage per head exceeds u64"))
}

fn ensure_pop(place: &mut PlaceState, kind: &str, culture: &str, religion: &str) -> usize {
    if let Some(index) = place
        .pops
        .iter()
        .position(|pop| pop.kind == kind && pop.culture == culture && pop.religion == religion)
    {
        return index;
    }
    let index = place.pops.len();
    place.pops.push(PopState {
        kind: kind.to_owned(),
        culture: culture.to_owned(),
        religion: religion.to_owned(),
        literacy_pm: 0,
        workers: 0,
        dependents: 0,
        employment: vec![Employment::default(); place.buildings.len()],
        stock: vec![0; place.prices_milli.len()],
        consumption: PopConsumption {
            pantry_hunger_centi: vec![0; place.prices_milli.len()],
            ticking_carries: vec![0; place.prices_milli.len()],
            ..PopConsumption::default()
        },
        counters: CounterBuckets::default(),
        short_on_rent: false,
        housed_last: false,
        genetics: PopGenetics::default(),
        pending_births: 0,
        pending_deaths: 0,
    });
    for building in &mut place.buildings {
        building.employment.push(Employment::default());
        if let Some(accounts) = &mut building.storehouse_accounts {
            accounts.pops.push(vec![0; place.prices_milli.len()]);
            // The last fee carry belongs to the communal account. Insert the new pop's
            // carry immediately before it so the account and fee vectors stay aligned.
            let pop_carry_index = accounts.pops.len() - 1;
            building
                .fee_carries
                .insert(pop_carry_index, vec![0; place.prices_milli.len()]);
        }
    }
    index
}

fn scaled_share(amount: u64, movers: u64, source_size: u64) -> Result<u64, DataError> {
    if source_size == 0 {
        return Err(missing_data(
            "movement.share".to_owned(),
            "source pop is empty",
        ));
    }
    u64::try_from(u128::from(amount) * u128::from(movers) / u128::from(source_size))
        .map_err(|_| missing_data("movement.share".to_owned(), "share exceeds u64"))
}

fn receive_pantry_share(
    rules: &Rules,
    destination: &mut PopState,
    good_index: usize,
    offered: u64,
) -> Result<bool, DataError> {
    if offered == 0 {
        return Ok(true);
    }
    let good = rules
        .goods
        .get(good_index)
        .ok_or_else(|| missing_data("movement.pantry".to_owned(), "good index is missing"))?;
    if good.hunger_centi == 0 {
        return Err(missing_data(
            format!("movement.pantry.{}", good.id),
            "a non-food good has a hunger remainder",
        ));
    }
    let pantry = destination
        .consumption
        .pantry_hunger_centi
        .get(good_index)
        .copied()
        .ok_or_else(|| missing_data("movement.pantry".to_owned(), "pantry index is missing"))?;
    let combined = pantry
        .checked_add(offered)
        .ok_or_else(|| missing_data("movement.pantry".to_owned(), "pantry total overflow"))?;
    destination.consumption.pantry_hunger_centi[good_index] = combined;
    Ok(true)
}

fn weighted_average(
    old: u16,
    old_people: u64,
    moved: u16,
    moved_people: u64,
) -> Result<u16, DataError> {
    let total = old_people
        .checked_add(moved_people)
        .ok_or_else(|| missing_data("movement.average".to_owned(), "population overflow"))?;
    if total == 0 {
        return Ok(0);
    }
    let weighted = u128::from(old)
        .checked_mul(u128::from(old_people))
        .and_then(|value| value.checked_add(u128::from(moved) * u128::from(moved_people)))
        .ok_or_else(|| missing_data("movement.average".to_owned(), "weighted average overflow"))?;
    u16::try_from(weighted / u128::from(total))
        .map_err(|_| missing_data("movement.average".to_owned(), "average exceeds u16"))
}

#[expect(
    clippy::too_many_arguments,
    reason = "These indices and counts are one atomic cross-place transfer request; grouping them would obscure the source and destination invariants."
)]
#[expect(
    clippy::too_many_lines,
    reason = "The transfer commits population, employment, goods, pantry, balances, and storehouse claims together so refusals cannot leave partial state."
)]
fn transfer_pop(
    rules: &Rules,
    places: &mut [PlaceState],
    source_place_index: usize,
    source_pop_index: usize,
    destination_place_index: usize,
    destination_pop_kind: &str,
    destination_building_index: usize,
    workers: u64,
    dependents: u64,
    source_workers_removed: u64,
    source_dependents_removed: u64,
) -> Result<usize, DataError> {
    if source_place_index == destination_place_index
        && destination_pop_kind == places[source_place_index].pops[source_pop_index].kind
    {
        let place = &mut places[source_place_index];
        assign_worker(place, destination_building_index, source_pop_index, workers)?;
        return Ok(source_pop_index);
    }
    let source_size = places[source_place_index].pops[source_pop_index]
        .workers
        .checked_add(places[source_place_index].pops[source_pop_index].dependents)
        .ok_or_else(|| missing_data("movement.source".to_owned(), "source size overflow"))?;
    let moved_people = workers
        .checked_add(dependents)
        .ok_or_else(|| missing_data("movement.transfer".to_owned(), "moved size overflow"))?;
    let source_heads_removed = source_workers_removed
        .checked_add(source_dependents_removed)
        .ok_or_else(|| missing_data("movement.transfer".to_owned(), "source removal overflow"))?;
    if source_size == 0 || source_heads_removed > source_size || moved_people == 0 {
        return Err(missing_data(
            "movement.transfer".to_owned(),
            "invalid moved population",
        ));
    }

    let source_snapshot = places[source_place_index].pops[source_pop_index].clone();
    let destination_pop_index = ensure_pop(
        &mut places[destination_place_index],
        destination_pop_kind,
        &source_snapshot.culture,
        &source_snapshot.religion,
    );
    let old_destination_size = places[destination_place_index].pops[destination_pop_index]
        .workers
        .checked_add(places[destination_place_index].pops[destination_pop_index].dependents)
        .ok_or_else(|| {
            missing_data(
                "movement.destination".to_owned(),
                "destination size overflow",
            )
        })?;
    {
        let destination = &mut places[destination_place_index].pops[destination_pop_index];
        destination.literacy_pm = weighted_average(
            destination.literacy_pm,
            old_destination_size,
            source_snapshot.literacy_pm,
            moved_people,
        )?;
        destination.consumption.necessities_pm = weighted_average(
            destination.consumption.necessities_pm,
            old_destination_size,
            source_snapshot.consumption.necessities_pm,
            moved_people,
        )?;
        destination.consumption.niceties_pm = weighted_average(
            destination.consumption.niceties_pm,
            old_destination_size,
            source_snapshot.consumption.niceties_pm,
            moved_people,
        )?;
        destination.consumption.luxuries_pm = weighted_average(
            destination.consumption.luxuries_pm,
            old_destination_size,
            source_snapshot.consumption.luxuries_pm,
            moved_people,
        )?;
        let all_people = old_destination_size
            .checked_add(moved_people)
            .ok_or_else(|| missing_data("movement.average".to_owned(), "population overflow"))?;
        destination
            .genetics
            .blend(
                old_destination_size,
                &source_snapshot.genetics,
                moved_people,
            )
            .map_err(|error| missing_data("movement.genetics".to_owned(), &error.to_string()))?;
        let satisfaction_total = i128::from(destination.consumption.satisfaction)
            .checked_mul(i128::from(old_destination_size))
            .and_then(|value| {
                value.checked_add(
                    i128::from(source_snapshot.consumption.satisfaction) * i128::from(moved_people),
                )
            })
            .ok_or_else(|| {
                missing_data(
                    "movement.average".to_owned(),
                    "satisfaction average overflow",
                )
            })?;
        destination.consumption.satisfaction =
            i16::try_from(satisfaction_total / i128::from(all_people)).map_err(|_| {
                missing_data(
                    "movement.average".to_owned(),
                    "satisfaction average exceeds i16",
                )
            })?;
        destination.workers = destination.workers.checked_add(workers).ok_or_else(|| {
            missing_data("movement.destination".to_owned(), "worker count overflow")
        })?;
        destination.dependents =
            destination
                .dependents
                .checked_add(dependents)
                .ok_or_else(|| {
                    missing_data(
                        "movement.destination".to_owned(),
                        "dependent count overflow",
                    )
                })?;
    }
    assign_worker(
        &mut places[destination_place_index],
        destination_building_index,
        destination_pop_index,
        workers,
    )?;

    let source_pop = &mut places[source_place_index].pops[source_pop_index];
    source_pop.workers = source_pop
        .workers
        .checked_sub(source_workers_removed)
        .ok_or_else(|| missing_data("movement.source".to_owned(), "worker count underflow"))?;
    source_pop.dependents = source_pop
        .dependents
        .checked_sub(source_dependents_removed)
        .ok_or_else(|| missing_data("movement.source".to_owned(), "dependent removal underflow"))?;

    for good_index in 0..rules.goods.len() {
        let offered = scaled_share(source_snapshot.stock[good_index], moved_people, source_size)?;
        let unaccepted = receive_pop(
            rules,
            &mut places[destination_place_index].pops[destination_pop_index],
            good_index,
            offered,
        )?;
        let accepted = offered - unaccepted;
        places[source_place_index].pops[source_pop_index].stock[good_index] -= accepted;
        let pantry = scaled_share(
            source_snapshot.consumption.pantry_hunger_centi[good_index],
            moved_people,
            source_size,
        )?;
        if receive_pantry_share(
            rules,
            &mut places[destination_place_index].pops[destination_pop_index],
            good_index,
            pantry,
        )? {
            places[source_place_index].pops[source_pop_index]
                .consumption
                .pantry_hunger_centi[good_index] -= pantry;
        }
    }
    for (source_balance, destination_balance) in [
        (source_snapshot.consumption.necessities_centi, 0_u8),
        (source_snapshot.consumption.niceties_centi, 1),
        (source_snapshot.consumption.luxuries_centi, 2),
    ] {
        let share = scaled_share(source_balance, moved_people, source_size)?;
        match destination_balance {
            0 => {
                let balance = &mut places[destination_place_index].pops[destination_pop_index]
                    .consumption
                    .necessities_centi;
                *balance = balance
                    .checked_add(share)
                    .ok_or_else(|| missing_data("movement.balance".to_owned(), "needs overflow"))?;
                places[source_place_index].pops[source_pop_index]
                    .consumption
                    .necessities_centi -= share;
            }
            1 => {
                let balance = &mut places[destination_place_index].pops[destination_pop_index]
                    .consumption
                    .niceties_centi;
                *balance = balance.checked_add(share).ok_or_else(|| {
                    missing_data("movement.balance".to_owned(), "niceties overflow")
                })?;
                places[source_place_index].pops[source_pop_index]
                    .consumption
                    .niceties_centi -= share;
            }
            _ => {
                let balance = &mut places[destination_place_index].pops[destination_pop_index]
                    .consumption
                    .luxuries_centi;
                *balance = balance.checked_add(share).ok_or_else(|| {
                    missing_data("movement.balance".to_owned(), "luxuries overflow")
                })?;
                places[source_place_index].pops[source_pop_index]
                    .consumption
                    .luxuries_centi -= share;
            }
        }
    }
    for building_index in 0..places[source_place_index].buildings.len() {
        let building = &mut places[source_place_index].buildings[building_index];
        let Some(accounts) = building.storehouse_accounts.as_mut() else {
            continue;
        };
        let Some(account) = accounts.pops.get_mut(source_pop_index) else {
            continue;
        };
        let snapshot = account.clone();
        for (good_index, amount) in snapshot.into_iter().enumerate() {
            let offered = scaled_share(amount, moved_people, source_size)?;
            let unaccepted = receive_pop(
                rules,
                &mut places[destination_place_index].pops[destination_pop_index],
                good_index,
                offered,
            )?;
            let accepted = offered - unaccepted;
            account[good_index] -= accepted;
            building.stock[good_index] = building.stock[good_index]
                .checked_sub(accepted)
                .ok_or_else(|| {
                    missing_data(
                        "movement.account".to_owned(),
                        "physical storehouse stock underflow",
                    )
                })?;
        }
    }
    Ok(destination_pop_index)
}

fn source_can_fill(source: &PopState, position_kind: &str, posted: u64, wage: u64) -> bool {
    if wage >= posted || source.workers == 0 {
        return false;
    }
    match position_kind {
        "labourer" => source.kind == "peasant",
        "monk" => source.religion == "christianity" && source.kind != "monk",
        _ => false,
    }
}

fn source_order(
    place: &PlaceState,
    position_kind: &str,
    posted: u64,
    leave: &[u64],
) -> Result<Vec<usize>, DataError> {
    let mut sources = Vec::new();
    for (index, source) in place.pops.iter().enumerate() {
        if leave.get(index).copied().unwrap_or(0) == 0 {
            continue;
        }
        let wage = pop_wage_per_head(place, index)?;
        if source_can_fill(source, position_kind, posted, wage) {
            sources.push((wage, index));
        }
    }
    sources.sort_by_key(|(wage, index)| (*wage, *index));
    Ok(sources.into_iter().map(|(_, index)| index).collect())
}

fn position_capacity(
    definition: &BuildingType,
    position_kind: &str,
    levels: u32,
) -> Result<u64, DataError> {
    let mut kinds = definition.owner_workers.keys().cloned().collect::<Vec<_>>();
    kinds.sort_by_key(|kind| match kind.as_str() {
        "monk" => 0,
        "labourer" => 1,
        "peasant" => 2,
        _ => 3,
    });
    let Some(index) = kinds.iter().position(|kind| kind == position_kind) else {
        return Ok(0);
    };
    if kinds.is_empty() {
        return Ok(0);
    }
    let total = u64::from(definition.workers_per_level)
        .checked_mul(u64::from(levels))
        .ok_or_else(|| missing_data("employment.positions".to_owned(), "capacity overflow"))?;
    let count = u64::try_from(kinds.len()).map_err(|_| {
        missing_data(
            "employment.positions".to_owned(),
            "position-kind count overflow",
        )
    })?;
    let base = total / count;
    let remainder = total % count;
    base.checked_add(u64::from((index as u64) < remainder))
        .ok_or_else(|| missing_data("employment.positions".to_owned(), "capacity overflow"))
}

#[expect(
    clippy::too_many_arguments,
    reason = "The function applies different monk and labourer transfer rules to one source and destination move."
)]
fn move_recruits(
    rules: &Rules,
    places: &mut [PlaceState],
    source_place: usize,
    source_pop: usize,
    destination_place: usize,
    destination_building: usize,
    position_kind: &str,
    count: u64,
) -> Result<(u64, usize), DataError> {
    let same_place = source_place == destination_place;
    let mut trial = if same_place {
        vec![places[source_place].clone()]
    } else {
        vec![
            places[source_place].clone(),
            places[destination_place].clone(),
        ]
    };
    let trial_destination = usize::from(!same_place);
    let source = &trial[0].pops[source_pop];
    let (target_kind, moved_workers, moved_dependents, removed_workers, removed_dependents) =
        if position_kind == "monk" {
            let shares = largest_remainder(count, &[source.workers, source.dependents])
                .map_err(|problem| missing_data("movement.monks".to_owned(), problem))?;
            ("monk", count, 0, shares[0], shares[1])
        } else {
            let moved_dependents = u64::try_from(
                u128::from(count) * u128::from(source.dependents) / u128::from(source.workers),
            )
            .map_err(|_| {
                missing_data(
                    "movement.dependents".to_owned(),
                    "dependent transfer exceeds u64",
                )
            })?;
            ("labourer", count, moved_dependents, count, moved_dependents)
        };
    let destination_pop_index = transfer_pop(
        rules,
        &mut trial,
        0,
        source_pop,
        trial_destination,
        target_kind,
        destination_building,
        moved_workers,
        moved_dependents,
        removed_workers,
        removed_dependents,
    )?;
    release_excess_employment(&mut trial[0])?;
    let mut updated = trial.into_iter();
    places[source_place] = updated.next().expect("the source place was cloned");
    if !same_place {
        places[destination_place] = updated.next().expect("the destination place was cloned");
    }
    Ok((
        moved_workers.checked_add(moved_dependents).ok_or_else(|| {
            missing_data("movement.transfer".to_owned(), "moved population overflow")
        })?,
        destination_pop_index,
    ))
}

#[cfg(test)]
fn move_open_positions(
    rules: &Rules,
    places: &mut [PlaceState],
    nearby: &BTreeMap<String, Vec<NearbyPlace>>,
    allowance: &[Vec<[u64; 2]>],
) -> Result<Vec<Vec<u64>>, DataError> {
    move_open_positions_with_floors(rules, places, nearby, allowance, &BTreeMap::new())
}
#[expect(
    clippy::too_many_lines,
    reason = "Destination, monk-first, local-first, then nearest-neighbour ordering is the recruitment rule and stays explicit here."
)]
fn move_open_positions_with_floors(
    rules: &Rules,
    places: &mut [PlaceState],
    nearby_by_place: &BTreeMap<String, Vec<NearbyPlace>>,
    movement_allowance: &[Vec<[u64; 2]>],
    floors: &BTreeMap<(String, usize), u64>,
) -> Result<Vec<Vec<u64>>, DataError> {
    let mut movement_by_pop = places
        .iter()
        .map(|place| vec![0_u64; place.pops.len()])
        .collect::<Vec<_>>();
    let mut leave = places
        .iter()
        .map(|place| {
            place
                .pops
                .iter()
                .map(|pop| {
                    u64::try_from(u128::from(pop.workers) * u128::from(rules.leave_pm) / 1000)
                        .map_err(|_| {
                            missing_data(
                                "employment.leave_pm".to_owned(),
                                "leave budget exceeds u64",
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;

    for destination_place in 0..places.len() {
        let building_count = places[destination_place].buildings.len();
        for destination_building in 0..building_count {
            let kind = places[destination_place].buildings[destination_building]
                .kind
                .clone();
            if kind == "storehouse" {
                continue;
            }
            let definition = rules
                .buildings
                .iter()
                .find(|item| item.id == kind)
                .ok_or_else(|| {
                    missing_data(format!("buildings.{kind}"), "definition is missing")
                })?;
            let recruit_range_m = definition.recruit_range_m.ok_or_else(|| {
                missing_data(
                    format!("buildings.{kind}.recruit_range_m"),
                    "entry is missing",
                )
            })?;
            // Monk positions are first so a labourer who can enter the cloth takes that opening
            // before a lower-priority labourer position is filled.
            for position_kind in ["monk", "labourer"] {
                let capacity = position_capacity(
                    definition,
                    position_kind,
                    places[destination_place].buildings[destination_building].levels,
                )?;
                if capacity == 0 {
                    continue;
                }
                let filled = places[destination_place].buildings[destination_building]
                    .employment
                    .iter()
                    .enumerate()
                    .filter(|(pop_index, job)| {
                        job.workers > 0
                            && places[destination_place].pops[*pop_index].kind == position_kind
                    })
                    .try_fold(0_u64, |sum, (_, job)| {
                        sum.checked_add(job.workers).ok_or_else(|| {
                            missing_data("employment.workers".to_owned(), "position total overflow")
                        })
                    })?;
                let mut open = capacity.saturating_sub(filled);
                let allowance_index = match position_kind {
                    "monk" => 0,
                    "labourer" => 1,
                    _ => continue,
                };
                let mut remaining_monthly = movement_allowance
                    .get(destination_place)
                    .and_then(|place| place.get(destination_building))
                    .map_or(0, |budget| budget[allowance_index]);
                if open == 0 {
                    continue;
                }
                let posted =
                    places[destination_place].buildings[destination_building].posted_wage();
                let local = source_order(
                    &places[destination_place],
                    position_kind,
                    posted,
                    &leave[destination_place],
                )?;
                if !local.is_empty() {
                    for source_pop in local {
                        if open == 0 {
                            break;
                        }
                        let count = open
                            .min(remaining_monthly)
                            .min(leave[destination_place][source_pop]);
                        if count == 0 {
                            continue;
                        }
                        let source = &places[destination_place].pops[source_pop];
                        let same_pop = source.kind == position_kind;
                        let floor = floors
                            .get(&(places[destination_place].id.clone(), source_pop))
                            .copied()
                            .unwrap_or(0);
                        let removable = source
                            .workers
                            .saturating_add(source.dependents)
                            .saturating_sub(floor);
                        let count = if same_pop {
                            count
                        } else if position_kind == "monk" {
                            count.min(removable)
                        } else {
                            recruitable(count, source.workers, source.dependents, removable)
                        };
                        if count == 0 {
                            continue;
                        }
                        let (moved_people, destination_pop) = move_recruits(
                            rules,
                            places,
                            destination_place,
                            source_pop,
                            destination_place,
                            destination_building,
                            position_kind,
                            count,
                        )?;
                        while movement_by_pop[destination_place].len()
                            < places[destination_place].pops.len()
                        {
                            movement_by_pop[destination_place].push(0);
                        }
                        movement_by_pop[destination_place][destination_pop] = movement_by_pop
                            [destination_place][destination_pop]
                            .checked_add(moved_people)
                            .ok_or_else(|| {
                                missing_data(
                                    "movement.ledger".to_owned(),
                                    "destination movement count overflow",
                                )
                            })?;
                        leave[destination_place][source_pop] -= count;
                        open -= count;
                        remaining_monthly -= count;
                    }
                    continue;
                }
                let nearby = nearby_by_place
                    .get(&places[destination_place].id)
                    .cloned()
                    .unwrap_or_default();
                for neighbour in nearby {
                    if open == 0 {
                        break;
                    }
                    if neighbour.distance_m > recruit_range_m {
                        continue;
                    }
                    let Some(source_place) =
                        places.iter().position(|place| place.id == neighbour.id)
                    else {
                        continue;
                    };
                    let sources = source_order(
                        &places[source_place],
                        position_kind,
                        posted,
                        &leave[source_place],
                    )?;
                    for source_pop in sources {
                        if open == 0 {
                            break;
                        }
                        let count = open
                            .min(remaining_monthly)
                            .min(leave[source_place][source_pop]);
                        if count == 0 {
                            continue;
                        }
                        let source = &places[source_place].pops[source_pop];
                        let same_pop =
                            source_place == destination_place && source.kind == position_kind;
                        let floor = floors
                            .get(&(places[source_place].id.clone(), source_pop))
                            .copied()
                            .unwrap_or(0);
                        let removable = source
                            .workers
                            .saturating_add(source.dependents)
                            .saturating_sub(floor);
                        let count = if same_pop {
                            count
                        } else if position_kind == "monk" {
                            count.min(removable)
                        } else {
                            recruitable(count, source.workers, source.dependents, removable)
                        };
                        if count == 0 {
                            continue;
                        }
                        let (moved_people, destination_pop) = move_recruits(
                            rules,
                            places,
                            source_place,
                            source_pop,
                            destination_place,
                            destination_building,
                            position_kind,
                            count,
                        )?;
                        while movement_by_pop[destination_place].len()
                            < places[destination_place].pops.len()
                        {
                            movement_by_pop[destination_place].push(0);
                        }
                        movement_by_pop[destination_place][destination_pop] = movement_by_pop
                            [destination_place][destination_pop]
                            .checked_add(moved_people)
                            .ok_or_else(|| {
                                missing_data(
                                    "movement.ledger".to_owned(),
                                    "destination movement count overflow",
                                )
                            })?;
                        leave[source_place][source_pop] -= count;
                        open -= count;
                        remaining_monthly -= count;
                    }
                }
            }
        }
    }
    Ok(movement_by_pop)
}

#[cfg(test)]
fn run_monthly_demography(
    rules: &Rules,
    state: &mut EconomyState,
    shelter: &BTreeMap<String, u64>,
    nearby: &BTreeMap<String, Vec<NearbyPlace>>,
    fee: Option<(usize, u16, &mut ProductionLedger)>,
) -> Result<Vec<Vec<u64>>, DataError> {
    run_monthly_demography_with_floors(rules, state, shelter, nearby, fee, &BTreeMap::new())
}
#[expect(
    clippy::too_many_lines,
    reason = "The phase order is housing snapshot, births/deaths, dependent drift, hiring, then movement; keeping it together protects the monthly contract."
)]
fn run_monthly_demography_with_floors(
    rules: &Rules,
    state: &mut EconomyState,
    shelter_by_place: &BTreeMap<String, u64>,
    nearby_by_place: &BTreeMap<String, Vec<NearbyPlace>>,
    mut fee_context: Option<(usize, u16, &mut ProductionLedger)>,
    floors: &BTreeMap<(String, usize), u64>,
) -> Result<Vec<Vec<u64>>, DataError> {
    let mut movement_allowance = Vec::with_capacity(state.places.len());
    for (place_index, place) in state.places.iter_mut().enumerate() {
        let shelter = shelter_by_place.get(&place.id).copied().unwrap_or(0);
        // The previous month's rent result takes effect for this month's fill, and so for every
        // housing query until the next 1st; this month's rent result waits for that 1st.
        for pop in &mut place.pops {
            pop.housed_last = pop.short_on_rent;
        }
        let fill = housing_fill(rules, place, shelter)?;
        // Rent follows the housing fill on the 1st. It needs the month's length and the place's
        // ledger, which only the live monthly pass carries.
        if let Some((month_index, year, ledger)) = fee_context.as_mut() {
            let month = u8::try_from(*month_index + 1)
                .map_err(|_| missing_data("calendar.month".to_owned(), "month is out of range"))?;
            let days = days_in_month(*year, month)
                .map_err(|error| missing_data("calendar.month".to_owned(), &error.to_string()))?;
            let place_ledger = ledger
                .places
                .get_mut(place_index)
                .filter(|entry| entry.place_id == place.id)
                .ok_or_else(|| {
                    missing_data(
                        "ledger.places".to_owned(),
                        "place order does not match the economy state",
                    )
                })?;
            collect_monthly_rent(rules, place, &fill, u64::from(days), place_ledger)?;
        }
        for (pop_index, pop) in place.pops.iter_mut().enumerate() {
            let pop_type = rules
                .pop_types
                .iter()
                .find(|candidate| candidate.id == pop.kind)
                .ok_or_else(|| {
                    missing_data(format!("pop_types.{}", pop.kind), "definition is missing")
                })?;
            let crowding = fill.pops[pop_index].crowding_pm.unwrap_or(1000);
            let birth_factors = [
                curve_value(
                    rules.curves.get("births_food").ok_or_else(|| {
                        missing_data("curves.births_food".to_owned(), "curve is missing")
                    })?,
                    u64::from(pop.consumption.necessities_pm),
                )?,
                curve_value(
                    rules.curves.get("births_crowding").ok_or_else(|| {
                        missing_data("curves.births_crowding".to_owned(), "curve is missing")
                    })?,
                    crowding,
                )?,
                curve_value_signed(
                    rules.curves.get("births_satisfaction").ok_or_else(|| {
                        missing_data("curves.births_satisfaction".to_owned(), "curve is missing")
                    })?,
                    i64::from(pop.consumption.satisfaction),
                )?,
                curve_value(
                    rules.curves.get("births_literacy").ok_or_else(|| {
                        missing_data("curves.births_literacy".to_owned(), "curve is missing")
                    })?,
                    u64::from(pop.literacy_pm),
                )?,
            ];
            let requested_births = exact_monthly_rate(
                pop.workers,
                pop_type.births_pm_year,
                &birth_factors,
                &mut pop.consumption.birth_carry,
                "demography.births",
            )?;
            // CK-364: the births the characters already committed this month are among the pop's births, so the step adds only
            // the rest, and the dependent deaths that follow are computed on the reduced count. The counters record both.
            let births = requested_births.saturating_sub(pop.pending_births);
            pop.dependents = pop.dependents.checked_add(births).ok_or_else(|| {
                missing_data("demography.births".to_owned(), "dependent count overflow")
            })?;
            let counted_births = births.checked_add(pop.pending_births).ok_or_else(|| {
                missing_data("counters.births".to_owned(), "birth counter overflow")
            })?;
            pop.counters.current.births = pop
                .counters
                .current
                .births
                .checked_add(counted_births)
                .ok_or_else(|| {
                missing_data("counters.births".to_owned(), "birth counter overflow")
            })?;
            place.counters.current.births = place
                .counters
                .current
                .births
                .checked_add(counted_births)
                .ok_or_else(|| {
                    missing_data("counters.births".to_owned(), "place birth counter overflow")
                })?;

            let worker_death_factors = death_multipliers_of(rules, pop, crowding)?;
            let worker_deaths = exact_monthly_rate(
                pop.workers,
                pop_type.worker_deaths_pm_year,
                &worker_death_factors,
                &mut pop.consumption.worker_death_carry,
                "demography.worker_deaths",
            )?
            .min(pop.workers);
            let dependent_death_factors = [
                worker_death_factors[0],
                worker_death_factors[1],
                worker_death_factors[2],
                worker_death_factors[2],
            ];
            let ordinary_deaths = exact_monthly_rate(
                pop.dependents,
                pop_type.dependent_deaths_pm_year,
                &dependent_death_factors,
                &mut pop.consumption.dependent_death_carry,
                "demography.dependent_deaths",
            )?;
            let need = pop.consumption.fuel_need_month;
            let burned = pop.consumption.fuel_burned_month;
            if burned > need {
                return Err(missing_data(
                    "fuel.burned_month".to_owned(),
                    "burned exceeds need",
                ));
            }
            let shortage = if need == 0 {
                0
            } else {
                u64::try_from(u128::from(need - burned) * 1000 / u128::from(need))
                    .expect("shortage <= 1000")
            };
            let fuel_deaths = exact_monthly_rate(
                pop.dependents,
                rules.fuel_deaths_pm_year,
                &[shortage],
                &mut pop.consumption.fuel_death_carry,
                "demography.fuel_deaths",
            )?;
            pop.consumption.fuel_need_month = 0;
            pop.consumption.fuel_burned_month = 0;
            let dependent_deaths = ordinary_deaths
                .saturating_add(fuel_deaths)
                .min(pop.dependents);
            // CK-364: the deaths the characters already committed are among the pop's deaths: dependents first, then workers.
            let (worker_deaths, dependent_deaths) =
                net_of_pending(worker_deaths, dependent_deaths, pop.pending_deaths);
            let floor = floors
                .get(&(place.id.clone(), pop_index))
                .copied()
                .unwrap_or(0);
            let removable = pop
                .workers
                .saturating_add(pop.dependents)
                .saturating_sub(floor);
            let (worker_deaths, dependent_deaths) =
                clip_deaths(worker_deaths, dependent_deaths, removable);
            pop.workers -= worker_deaths;
            pop.dependents -= dependent_deaths;
            let deaths = worker_deaths
                .checked_add(dependent_deaths)
                .and_then(|realised| realised.checked_add(pop.pending_deaths))
                .ok_or_else(|| {
                    missing_data("demography.deaths".to_owned(), "death count overflow")
                })?;
            // What the characters committed has now been counted: the next monthly step starts from nothing.
            pop.pending_births = 0;
            pop.pending_deaths = 0;
            pop.counters.current.deaths = pop
                .counters
                .current
                .deaths
                .checked_add(deaths)
                .ok_or_else(|| {
                    missing_data("counters.deaths".to_owned(), "death counter overflow")
                })?;
            place.counters.current.deaths = place
                .counters
                .current
                .deaths
                .checked_add(deaths)
                .ok_or_else(|| {
                    missing_data("counters.deaths".to_owned(), "place death counter overflow")
                })?;

            let total = pop.workers.checked_add(pop.dependents).ok_or_else(|| {
                missing_data("demography.population".to_owned(), "pop size overflow")
            })?;
            pop.dependents = drift_dependents(
                pop.dependents,
                total,
                pop_type.dependent_pm,
                &mut pop.consumption.dependent_drift_carry,
            )?;
            pop.workers = total.checked_sub(pop.dependents).ok_or_else(|| {
                missing_data("demography.dependent_drift".to_owned(), "workers underflow")
            })?;
        }
        release_excess_employment(place)?;
        movement_allowance.push(fill_monthly_hiring(rules, place)?);
    }
    let movement_by_pop = move_open_positions_with_floors(
        rules,
        &mut state.places,
        nearby_by_place,
        &movement_allowance,
        floors,
    )?;
    if let Some((month_index, year, ledger)) = fee_context {
        charge_storehouse_fees(rules, state, month_index, year, ledger)?;
    }
    for place in &mut state.places {
        for pop in &mut place.pops {
            let pop_type = rules
                .pop_types
                .iter()
                .find(|candidate| candidate.id == pop.kind)
                .ok_or_else(|| {
                    missing_data(format!("pop_types.{}", pop.kind), "definition is missing")
                })?;
            let target = satisfaction_baseline(
                pop_type,
                [
                    pop.consumption.necessities_pm,
                    pop.consumption.niceties_pm,
                    pop.consumption.luxuries_pm,
                ],
            );
            pop.consumption.satisfaction = i16::try_from(drift_value(
                i64::from(pop.consumption.satisfaction),
                target,
                100,
                1000,
                &mut pop.consumption.drift_carries[3],
                -100,
                100,
            )?)
            .map_err(|_| missing_data("satisfaction".to_owned(), "value exceeds i16"))?;
        }
    }
    Ok(movement_by_pop)
}

/// The three multipliers on a worker's death rate in ‰ (hunger, comfort, crowding), from the pop's state and its place's crowding.
fn death_multipliers_of(
    rules: &Rules,
    pop: &PopState,
    crowding_pm: u64,
) -> Result<[u64; 3], DataError> {
    let comfort = (u64::from(pop.consumption.niceties_pm)
        + u64::from(pop.consumption.luxuries_pm) / 2)
        .min(1000);
    let curve = |name: &str, x: u64| {
        curve_value(
            rules
                .curves
                .get(name)
                .ok_or_else(|| missing_data(format!("curves.{name}"), "curve is missing"))?,
            x,
        )
    };
    Ok([
        curve("deaths_hunger", u64::from(pop.consumption.necessities_pm))?,
        curve("deaths_comfort", comfort)?,
        curve("deaths_crowding", crowding_pm)?,
    ])
}

/// The three death multipliers (‰: hunger, comfort, crowding) the monthly step applies to the workers of each pop of `place`, in
/// pop order, as its pops stand now (CK-364: a character's chance of dying is scaled by them).
///
/// # Errors
/// Returns a named data error for a missing curve or malformed housing rules.
pub fn death_multipliers(
    rules: &Rules,
    place: &PlaceState,
    shelter: u64,
) -> Result<Vec<[u64; 3]>, DataError> {
    let fill = housing_fill(rules, place, shelter)?;
    place
        .pops
        .iter()
        .zip(&fill.pops)
        .map(|(pop, housing)| death_multipliers_of(rules, pop, housing.crowding_pm.unwrap_or(1000)))
        .collect()
}

/// What a removed character head took from a pop, so that it can be put back if the death it belongs to does not commit (CK-364).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeadUndo {
    staged: Option<StagedHead>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StagedHead {
    place: String,
    slot: usize,
    worker: bool,
    /// The building, and whether a paid worker's position (else an owner's), that was released.
    release: Option<(usize, bool)>,
    /// Whether the removal was counted as a pending death (a death), not a move between pops (CK-366).
    counted: bool,
}

/// What a head moved between two pops took from them, so that the move can be put back (CK-366): the source's head and released
/// position, and the destination's head and genetic state as they were.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MoveUndo {
    staged: Option<StagedMove>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StagedMove {
    head: HeadUndo,
    destination: (String, usize),
    worker: bool,
    genetics: PopGenetics,
}

impl EconomyState {
    fn pop_mut(&mut self, place: &str, slot: usize) -> Result<&mut PopState, DataError> {
        let place = self
            .places
            .iter_mut()
            .find(|candidate| candidate.id == place)
            .ok_or_else(|| missing_data(format!("places.{place}"), "no such place"))?;
        place
            .pops
            .get_mut(slot)
            .ok_or_else(|| missing_data(format!("places.{}.pops", place.id), "no such pop slot"))
    }

    /// Add `count` dependents to a pop for `count` character births, and count them as pending (CK-364). Nothing changes on an error.
    ///
    /// # Errors
    /// A missing place or pop slot, or a count that overflows.
    pub fn add_character_births(
        &mut self,
        place: &str,
        slot: usize,
        count: u64,
    ) -> Result<(), DataError> {
        let pop = self.pop_mut(place, slot)?;
        let overflow = || missing_data("demography.births".to_owned(), "dependent count overflow");
        let dependents = pop.dependents.checked_add(count).ok_or_else(overflow)?;
        let pending = pop.pending_births.checked_add(count).ok_or_else(overflow)?;
        pop.dependents = dependents;
        pop.pending_births = pending;
        Ok(())
    }

    /// Add one dependent to a pop at the start of a game (CK-370 §3 step 1, the start-only safety net): the pop would otherwise have fewer
    /// heads than characters. It is **not** a birth: nothing is counted as pending. Nothing changes on an error.
    ///
    /// # Errors
    /// A missing place or pop slot, or a dependent count that overflows.
    pub fn add_start_dependent(&mut self, place: &str, slot: usize) -> Result<(), DataError> {
        let pop = self.pop_mut(place, slot)?;
        pop.dependents = pop.dependents.checked_add(1).ok_or_else(|| {
            missing_data("start.dependents".to_owned(), "dependent count overflow")
        })?;
        Ok(())
    }

    /// Remove the head of a dying character from its pop and count the death as pending (CK-364). A character under majority is a
    /// dependent if the pop has one, else a worker; an adult is a worker if the pop has one, else a dependent. A removed worker is
    /// an unemployed one if the pop has any; otherwise an employed one is removed and one position is released at once, in the
    /// building that employs the most of that pop's workers (ties by building order; owners only when no worker is paid), so no job
    /// is left for a head that has died. Nothing changes on an error.
    ///
    /// # Errors
    /// A missing place or pop slot, or a pop with no head to remove.
    pub fn remove_character_head(
        &mut self,
        place: &str,
        slot: usize,
        minor: bool,
    ) -> Result<HeadUndo, DataError> {
        self.take_character_head(place, slot, minor, true)
    }

    /// Take the head of a character from a pop by the cohort rule of [`remove_character_head`](Self::remove_character_head),
    /// counting it as a pending death only when `death` is set (a move between pops is not a death).
    fn take_character_head(
        &mut self,
        place: &str,
        slot: usize,
        minor: bool,
        death: bool,
    ) -> Result<HeadUndo, DataError> {
        let place_state = self
            .places
            .iter_mut()
            .find(|candidate| candidate.id == place)
            .ok_or_else(|| missing_data(format!("places.{place}"), "no such place"))?;
        let pop = place_state
            .pops
            .get(slot)
            .ok_or_else(|| missing_data(format!("places.{place}.pops"), "no such pop slot"))?;
        let worker = if minor {
            pop.dependents == 0
        } else {
            pop.workers > 0
        };
        let removable = if worker { pop.workers } else { pop.dependents };
        if removable == 0 {
            return Err(missing_data(
                format!("places.{place}.pops"),
                "the pop has no head to remove",
            ));
        }
        let pending = if death {
            pop.pending_deaths.checked_add(1).ok_or_else(|| {
                missing_data("demography.deaths".to_owned(), "death count overflow")
            })?
        } else {
            pop.pending_deaths
        };
        // The job to release, decided before anything is changed.
        let mut release = None;
        if worker {
            let employed = place_state
                .buildings
                .iter()
                .map(|building| {
                    building
                        .employment
                        .get(slot)
                        .map_or(0, |job| job.owners + job.workers)
                })
                .sum::<u64>();
            if employed >= pop.workers {
                // The first building holding the most paid workers; owners only when no worker is paid.
                let most = |paid: bool| {
                    let held = |building: &BuildingState| {
                        building
                            .employment
                            .get(slot)
                            .map_or(0, |job| if paid { job.workers } else { job.owners })
                    };
                    place_state
                        .buildings
                        .iter()
                        .enumerate()
                        .filter(|(_, building)| held(building) > 0)
                        .fold(None, |best: Option<(usize, u64)>, (index, building)| {
                            let count = held(building);
                            best.filter(|(_, most)| *most >= count)
                                .or(Some((index, count)))
                        })
                        .map(|(index, _)| (index, paid))
                };
                release = most(true).or_else(|| most(false));
            }
        }
        if let Some((building, paid)) = release {
            let job = place_state.buildings[building]
                .employment
                .get_mut(slot)
                .ok_or_else(|| {
                    missing_data(
                        "employment".to_owned(),
                        "employment dimensions do not match",
                    )
                })?;
            let held = if paid {
                &mut job.workers
            } else {
                &mut job.owners
            };
            *held -= 1;
            let released = job.clone();
            place_state.pops[slot].employment[building] = released;
        }
        let pop = &mut place_state.pops[slot];
        if worker {
            pop.workers -= 1;
        } else {
            pop.dependents -= 1;
        }
        pop.pending_deaths = pending;
        Ok(HeadUndo {
            staged: Some(StagedHead {
                place: place.to_owned(),
                slot,
                worker,
                release,
                counted: death,
            }),
        })
    }

    /// Put back what [`remove_character_head`](Self::remove_character_head) took: the head, the released position and the pending
    /// death. It undoes only the most recent removal that has not been undone, and does nothing for an empty [`HeadUndo`].
    pub fn restore_character_head(&mut self, undo: &HeadUndo) {
        let Some(staged) = &undo.staged else {
            return;
        };
        let Some(place) = self
            .places
            .iter_mut()
            .find(|place| place.id == staged.place)
        else {
            return;
        };
        if let Some((building, paid)) = staged.release
            && let Some(job) = place.buildings[building].employment.get_mut(staged.slot)
        {
            if paid {
                job.workers += 1;
            } else {
                job.owners += 1;
            }
            let restored = job.clone();
            place.pops[staged.slot].employment[building] = restored;
        }
        let Some(pop) = place.pops.get_mut(staged.slot) else {
            return;
        };
        if staged.worker {
            pop.workers += 1;
        } else {
            pop.dependents += 1;
        }
        if staged.counted {
            pop.pending_deaths = pop.pending_deaths.saturating_sub(1);
        }
    }

    /// Move the head of a married character from one pop to another (CK-366, marriage): the head leaves the source by the cohort rule
    /// of [`remove_character_head`](Self::remove_character_head) (a minor is a dependent if the pop has one, else a worker; an adult
    /// a worker if the pop has one, else a dependent; an employed worker's position is released at once) but is **not** counted as a
    /// death, and arrives in the destination in the same cohort (a worker arrives unemployed). The destination's genetic state
    /// blends with one head of the source's (the people-weighted rule of [`PopGenetics::blend`]); no stock, balance or other state
    /// moves. Nothing changes on an error.
    ///
    /// # Errors
    /// A missing place or pop slot, the same pop on both sides, a source with no head to take, a head count that overflows or a
    /// genetic blend that does not fit.
    pub fn move_character_head(
        &mut self,
        from: (&str, usize),
        to: (&str, usize),
        minor: bool,
    ) -> Result<MoveUndo, DataError> {
        let find = |places: &[PlaceState], (place, slot): (&str, usize)| {
            places
                .iter()
                .find(|candidate| candidate.id == place)
                .and_then(|candidate| candidate.pops.get(slot))
                .cloned()
                .ok_or_else(|| missing_data(format!("places.{place}.pops"), "no such pop slot"))
        };
        if from == to {
            return Err(missing_data(
                "marriage.move".to_owned(),
                "the head would move within one pop",
            ));
        }
        let source = find(&self.places, from)?;
        let destination = find(&self.places, to)?;
        let overflow = || missing_data("marriage.move".to_owned(), "head count overflow");
        let size = destination
            .workers
            .checked_add(destination.dependents)
            .ok_or_else(overflow)?;
        // The arrival must leave the destination's *total* representable, whichever cohort it joins.
        size.checked_add(1).ok_or_else(overflow)?;
        let mut genetics = destination.genetics.clone();
        genetics
            .blend(size, &source.genetics, 1)
            .map_err(|error| missing_data("marriage.genetics".to_owned(), &error.to_string()))?;
        let head = self.take_character_head(from.0, from.1, minor, false)?;
        let worker = head.staged.as_ref().is_some_and(|staged| staged.worker);
        let arrived = self.pop_mut(to.0, to.1)?;
        let undo = MoveUndo {
            staged: Some(StagedMove {
                head,
                destination: (to.0.to_owned(), to.1),
                worker,
                genetics: std::mem::replace(&mut arrived.genetics, genetics),
            }),
        };
        if worker {
            arrived.workers += 1;
        } else {
            arrived.dependents += 1;
        }
        Ok(undo)
    }

    /// Put back what [`move_character_head`](Self::move_character_head) did: the destination's head and genetic state and the
    /// source's head and released position. It undoes only the most recent move that has not been undone, and does nothing for an
    /// empty [`MoveUndo`].
    pub fn restore_character_move(&mut self, undo: &MoveUndo) {
        let Some(staged) = &undo.staged else {
            return;
        };
        let (place, slot) = (&staged.destination.0, staged.destination.1);
        if let Some(pop) = self
            .places
            .iter_mut()
            .find(|candidate| candidate.id == *place)
            .and_then(|candidate| candidate.pops.get_mut(slot))
        {
            pop.genetics = staged.genetics.clone();
            if staged.worker {
                pop.workers = pop.workers.saturating_sub(1);
            } else {
                pop.dependents = pop.dependents.saturating_sub(1);
            }
        }
        self.restore_character_head(&staged.head);
    }
}

/// The population type whose buildings charge rent on their living space (rent-r1 rule 1).
const RENT_OWNER_KIND: &str = "noble";

/// Rent is one twentieth of a person's monthly necessities (rent-r1 rule 3).
const RENT_BASKET_DIVISOR: u128 = 20;

/// The value, in thousandths of a value unit at the place's local prices, of one person's
/// necessities for `days` days: the units the food target gives one person of `kind`, each
/// valued at its local price.
fn rent_basket_milli(
    rules: &Rules,
    place: &PlaceState,
    kind: &str,
    days: u64,
) -> Result<u128, DataError> {
    let mut total = 0_u128;
    for (good_index, price) in place.prices_milli.iter().copied().enumerate() {
        let units = food_target_units(rules, kind, 1, good_index, days)?;
        total = value_at_price(units, price)
            .ok()
            .and_then(|value| total.checked_add(value))
            .ok_or_else(|| missing_data("rent.basket".to_owned(), "basket value overflow"))?;
    }
    Ok(total)
}

/// The whole units of each good a tenant pays towards `due_milli`, taken from `stock`.
///
/// A store worth no more than the rent is paid whole. Otherwise each good gives its proportional
/// share rounded down; then, while what is still owed is at least the price of some good with
/// stock left, units of the cheapest such good (ties in rules order) are taken, as many as fit.
/// What is still owed after that is less than the cheapest remaining unit: forgiven residue.
fn rent_payment_units(
    prices_milli: &[u64],
    stock: &[u64],
    store_milli: u128,
    due_milli: u128,
) -> Result<Vec<u64>, DataError> {
    let refuse =
        |entry: &str, problem: &'static str| missing_data(format!("rent.{entry}"), problem);
    if store_milli <= due_milli {
        return Ok(stock.to_vec());
    }
    // Each share is below the whole stock, because the due is below the store's value.
    let mut taken = stock
        .iter()
        .map(|units| {
            mul_div_floor(due_milli, u128::from(*units), store_milli)
                .and_then(|share| u64::try_from(share).ok())
                .ok_or_else(|| refuse("share", "proportional share overflow"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut taken_value = 0_u128;
    for (units, price) in taken.iter().zip(prices_milli) {
        taken_value = value_at_price(u128::from(*units), *price)
            .ok()
            .and_then(|value| taken_value.checked_add(value))
            .ok_or_else(|| refuse("share", "paid value overflow"))?;
    }
    let mut owed = due_milli - taken_value;
    while let Some((good_index, price)) = prices_milli
        .iter()
        .copied()
        .enumerate()
        .filter(|(index, price)| {
            *price > 0 && taken[*index] < stock[*index] && u128::from(*price) <= owed
        })
        .min_by_key(|(index, price)| (*price, *index))
    {
        let units = (stock[good_index] - taken[good_index])
            .min(u64::try_from(owed / u128::from(price)).unwrap_or(u64::MAX));
        taken[good_index] += units;
        owed -= u128::from(units) * u128::from(price);
    }
    Ok(taken)
}

/// `floor(a * b / c)` exactly, or `None` when `c` is zero or the quotient does not fit `u128`.
/// The product is formed in 256 bits when it does not fit 128, so it cannot overflow or round.
fn mul_div_floor(a: u128, b: u128, c: u128) -> Option<u128> {
    if c == 0 {
        return None;
    }
    if let Some(product) = a.checked_mul(b) {
        return Some(product / c);
    }
    // The 256-bit product as (high, low), from four 64-bit partial products.
    let (a_high, a_low) = (a >> 64, a & u128::from(u64::MAX));
    let (b_high, b_low) = (b >> 64, b & u128::from(u64::MAX));
    let low_low = a_low * b_low;
    let middle_one = a_low * b_high;
    let middle_two = a_high * b_low;
    let (middle, middle_carry) = middle_one.overflowing_add(middle_two);
    let (low, low_carry) = low_low.overflowing_add(middle << 64);
    let high =
        a_high * b_high + (middle >> 64) + (u128::from(middle_carry) << 64) + u128::from(low_carry);
    // The quotient fits 128 bits exactly when the high half is below the divisor.
    if high >= c {
        return None;
    }
    // Binary long division of the 256-bit value by `c`, most significant bit first. The remainder
    // stays below `c`; when doubling it carries out of 128 bits it is certainly above `c`.
    let mut remainder = high;
    let mut quotient = 0_u128;
    for bit in (0..128).rev() {
        let carry = remainder >> 127;
        remainder = (remainder << 1) | ((low >> bit) & 1);
        quotient <<= 1;
        if carry == 1 || remainder >= c {
            remainder = remainder.wrapping_sub(c);
            quotient |= 1;
        }
    }
    Some(quotient)
}

/// Add the change between two whole-place stock totals to a ledger's closing stock.
fn record_stock_change(
    ledger: &mut PlaceProductionLedger,
    before: &[u64],
    after: &[u64],
    what: &str,
) -> Result<(), DataError> {
    for (closing, (before, after)) in ledger.stock_after.iter_mut().zip(before.iter().zip(after)) {
        *closing = if after >= before {
            closing.checked_add(after - before).ok_or_else(|| {
                missing_data("ledger.stock".to_owned(), "closing stock total overflow")
            })?
        } else {
            closing.checked_sub(before - after).ok_or_else(|| {
                missing_data(
                    "ledger.stock".to_owned(),
                    match what {
                        "rent" => "rent loss exceeds the closing stock recorded before demography",
                        _ => "late fee exceeds the closing stock recorded before demography",
                    },
                )
            })?
        };
    }
    Ok(())
}

/// One tenant pop's rent, fixed from the snapshot taken before any rent moves.
struct RentDebit {
    pop_index: usize,
    units: Vec<u64>,
}

/// One tenant's snapshot: the rent due for `people` of its pop at the month's basket, the units it
/// will pay from its store as it stands, its shortfall flag and its counters. The units are debited
/// later, after every tenant of the building has been read.
fn rent_snapshot(
    place: &mut PlaceState,
    pop_index: usize,
    people: u64,
    basket: u128,
) -> Result<RentDebit, DataError> {
    let due_milli = u128::from(people)
        .checked_mul(basket)
        .ok_or_else(|| missing_data("rent.due".to_owned(), "rent overflow"))?
        / (RENT_BASKET_DIVISOR * 1000)
        * 1000;
    let pop = &place.pops[pop_index];
    let store_milli =
        pop.stock
            .iter()
            .zip(&place.prices_milli)
            .try_fold(0_u128, |total, (units, price)| {
                value_at_price(u128::from(*units), *price)
                    .ok()
                    .and_then(|value| total.checked_add(value))
            });
    let store_milli =
        store_milli.ok_or_else(|| missing_data("rent.store".to_owned(), "store value overflow"))?;
    let units = rent_payment_units(&place.prices_milli, &pop.stock, store_milli, due_milli)?;
    let mut paid_milli = 0_u128;
    for (units, price) in units.iter().zip(&place.prices_milli) {
        paid_milli = value_at_price(u128::from(*units), *price)
            .ok()
            .and_then(|value| paid_milli.checked_add(value))
            .ok_or_else(|| missing_data("rent.paid".to_owned(), "paid value overflow"))?;
    }
    // The counters are exact or the pass refuses: a value past `u64` is an error, never a cap.
    let counter = |value: u128| {
        u64::try_from(value)
            .map_err(|_| missing_data("counters.rent".to_owned(), "rent counter exceeds u64"))
    };
    let paid_counter = counter(paid_milli)?;
    let short_counter = counter(due_milli - paid_milli)?;
    let pop = &mut place.pops[pop_index];
    pop.short_on_rent |= store_milli < due_milli;
    pop.counters.current.rent_paid_milli = pop
        .counters
        .current
        .rent_paid_milli
        .checked_add(paid_counter)
        .ok_or_else(|| missing_data("counters.rent".to_owned(), "rent counter overflow"))?;
    pop.counters.current.rent_short_milli = pop
        .counters
        .current
        .rent_short_milli
        .checked_add(short_counter)
        .ok_or_else(|| missing_data("counters.rent".to_owned(), "rent counter overflow"))?;
    Ok(RentDebit { pop_index, units })
}

/// The monthly rent pass for one place, run right after the monthly housing fill.
///
/// Each noble-owned building in building order takes, from every pop housed in it that is not of its
/// owner kind, one twentieth of that many people's monthly necessities at local prices, in whole
/// goods units (rent-r1). The order within a building is fixed: every tenant's store, value and
/// due are read first; then every payment is debited, in pop order; only then are the debited
/// goods delivered, pop by pop, through the overhead path into the building's store, so alms the
/// pass produces cannot fund a tenant's rent in the same pass. What a tenant cannot pay is
/// forgiven; a pop whose store was worth less than its rent is flagged short for the next fill.
///
/// # Errors
/// Refuses malformed saved state or checked arithmetic overflow; `place` is then partly updated and
/// the caller discards it with the rest of the monthly state.
fn collect_monthly_rent(
    rules: &Rules,
    place: &mut PlaceState,
    fill: &HousingFill,
    days: u64,
    ledger: &mut PlaceProductionLedger,
) -> Result<(), DataError> {
    for pop in &mut place.pops {
        pop.short_on_rent = false;
    }
    let stock_before = total_stored_goods(rules, place)?;
    let mut basket_by_kind = BTreeMap::<String, u128>::new();
    for building_index in 0..place.buildings.len() {
        let definition = rules
            .buildings
            .iter()
            .find(|item| item.id == place.buildings[building_index].kind)
            .ok_or_else(|| {
                missing_data(
                    format!("buildings.{}", place.buildings[building_index].kind),
                    "definition is missing",
                )
            })?;
        if definition.owner_kind.as_deref() != Some(RENT_OWNER_KIND) {
            continue;
        }
        // 1. Snapshot: each tenant's people, due, store and the units it will pay.
        let mut debits = Vec::new();
        for pop_index in 0..place.pops.len() {
            let people = fill.housed[pop_index][building_index];
            if people == 0 || place.pops[pop_index].kind == RENT_OWNER_KIND {
                continue;
            }
            let kind = place.pops[pop_index].kind.clone();
            let basket = if let Some(value) = basket_by_kind.get(&kind) {
                *value
            } else {
                let value = rent_basket_milli(rules, place, &kind, days)?;
                basket_by_kind.insert(kind, value);
                value
            };
            debits.push(rent_snapshot(place, pop_index, people, basket)?);
        }
        // 2. Debit every tenant, in pop order.
        for debit in &debits {
            let pop = &mut place.pops[debit.pop_index];
            for (stock, units) in pop.stock.iter_mut().zip(&debit.units) {
                *stock -= units;
            }
        }
        // 3. Receipt: deliver the debited goods through the overhead path, pop by pop.
        for debit in &debits {
            for (good_index, units) in debit.units.iter().copied().enumerate() {
                if units == 0 {
                    continue;
                }
                settle_overflow(
                    rules,
                    &mut place.pops,
                    &mut place.buildings,
                    building_index,
                    good_index,
                    &[(None, units)],
                    &mut place.counters,
                    ledger,
                )?;
            }
        }
    }
    let stock_after = total_stored_goods(rules, place)?;
    record_stock_change(ledger, &stock_before, &stock_after, "rent")
}

fn charge_storehouse_fees(
    rules: &Rules,
    state: &mut EconomyState,
    month_index: usize,
    year: u16,
    ledger: &mut ProductionLedger,
) -> Result<(), DataError> {
    if ledger.places.len() != state.places.len() {
        return Err(missing_data(
            "ledger.places".to_owned(),
            "place dimensions do not match the economy state",
        ));
    }
    for (place_index, place) in state.places.iter_mut().enumerate() {
        let place_ledger = ledger
            .places
            .get_mut(place_index)
            .ok_or_else(|| missing_data("ledger.places".to_owned(), "place entry is missing"))?;
        if place_ledger.place_id != place.id {
            return Err(missing_data(
                "ledger.places".to_owned(),
                "place order does not match the economy state",
            ));
        }
        let stock_before_fees = total_stored_goods(rules, place)?;
        for building_index in 0..place.buildings.len() {
            if place.buildings[building_index]
                .storehouse_accounts
                .is_some()
            {
                pay_storehouse_fee(
                    rules,
                    &place.id,
                    &place.prices_milli,
                    month_index,
                    year,
                    &mut place.pops,
                    &mut place.buildings,
                    building_index,
                    &mut place.counters,
                    place_ledger,
                )?;
            }
        }
        let stock_after_fees = total_stored_goods(rules, place)?;
        record_stock_change(place_ledger, &stock_before_fees, &stock_after_fees, "fee")?;
        place_ledger.unpaid_wages.fill(0);
        for building in &place.buildings {
            if building.pay_carries.len() != rules.goods.len() {
                return Err(missing_data(
                    format!(
                        "places.{}.buildings.{}.pay_carries",
                        place.id, building.kind
                    ),
                    "saved payday dimensions do not match goods",
                ));
            }
            for (total, unpaid) in place_ledger
                .unpaid_wages
                .iter_mut()
                .zip(&building.pay_carries)
            {
                *total = total.checked_add(*unpaid).ok_or_else(|| {
                    missing_data("ledger.unpaid_wages".to_owned(), "unpaid total overflow")
                })?;
            }
        }
    }
    ledger.lost.fill(0);
    ledger.stock_after.fill(0);
    ledger.unpaid_wages.fill(0);
    for place in &ledger.places {
        for (total, part) in ledger.lost.iter_mut().zip(&place.lost) {
            *total = total
                .checked_add(*part)
                .ok_or_else(|| missing_data("ledger.loss".to_owned(), "loss total overflow"))?;
        }
        for (total, part) in ledger.stock_after.iter_mut().zip(&place.stock_after) {
            *total = total
                .checked_add(*part)
                .ok_or_else(|| missing_data("ledger.stock".to_owned(), "stock total overflow"))?;
        }
        for (total, part) in ledger.unpaid_wages.iter_mut().zip(&place.unpaid_wages) {
            *total = total.checked_add(*part).ok_or_else(|| {
                missing_data("ledger.unpaid_wages".to_owned(), "unpaid total overflow")
            })?;
        }
    }
    Ok(())
}

/// The month's output and the carry that is left: `factors` multiplied together and the carry added, divided by `denominator`.
///
/// The product is worked in 128 bits when it fits and exactly in arbitrary size when it does not: the factor of twelve a level's
/// count brings may take a legal payday past 128 bits that the old units held, and what is paid and kept is the same.
fn payday_division(factors: &[u128], carry: u128, denominator: u128) -> Option<(u128, u128)> {
    let narrow = factors
        .iter()
        .try_fold(1_u128, |total, factor| total.checked_mul(*factor))
        .and_then(|total| total.checked_add(carry));
    if let Some(numerator) = narrow {
        return Some((numerator / denominator, numerator % denominator));
    }
    let numerator = bigint::product(factors).add(&bigint::Big::new(carry));
    let (quotient, remainder) = numerator.divmod(&bigint::Big::new(denominator));
    Some((quotient.to_u128()?, remainder.to_u128()?))
}

/// A method's levels grouped by ground and by the day's count: the levels' completed paydays and one more, at most
/// [`MATURE_PAYDAYS`]. Each group holds its number of levels. The records must agree with the building's counts.
type PaydayGroups = Vec<BTreeMap<(Option<String>, u8), u64>>;

fn payday_groups(
    place_id: &str,
    building: &BuildingState,
    definition: &BuildingType,
) -> Result<PaydayGroups, DataError> {
    let mut groups = vec![BTreeMap::new(); definition.methods.len()];
    let mut counts = vec![0_u32; definition.methods.len()];
    for record in &building.level_records {
        let method = usize::try_from(record.method).unwrap_or(usize::MAX);
        if method >= groups.len() || record.paydays > MATURE_PAYDAYS {
            return Err(missing_data(
                format!("places.{place_id}.buildings.{}.levels", building.kind),
                "saved level record names no method or has too many paydays",
            ));
        }
        counts[method] += 1;
        *groups[method]
            .entry((
                record.ground.clone(),
                (record.paydays + 1).min(MATURE_PAYDAYS),
            ))
            .or_insert(0_u64) += 1;
    }
    if counts != building.method_levels {
        return Err(missing_data(
            format!("places.{place_id}.buildings.{}.levels", building.kind),
            "saved level records do not add up to the method levels",
        ));
    }
    Ok(groups)
}

fn reduced_ratio(value: u16, reference: u16) -> (u16, u16) {
    if value == 0 {
        return (0, 1);
    }
    let divisor = gcd_u16(value, reference);
    (value / divisor, reference / divisor)
}

fn gcd_u16(mut left: u16, mut right: u16) -> u16 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left.max(1)
}

impl Canonical for EconomyState {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        encode_length(self.places.len(), out)?;
        for place in &self.places {
            place.id.encode(out)?;
            encode_length(place.pops.len(), out)?;
            for pop in &place.pops {
                pop.kind.encode(out)?;
                pop.culture.encode(out)?;
                pop.religion.encode(out)?;
                pop.literacy_pm.encode(out)?;
                pop.workers.encode(out)?;
                pop.dependents.encode(out)?;
                encode_employment(&pop.employment, out)?;
                pop.stock.encode(out)?;
                encode_pop_consumption(&pop.consumption, out)?;
                encode_counters(&pop.counters, out)?;
                pop.short_on_rent.encode(out)?;
                pop.housed_last.encode(out)?;
                pop.genetics.encode(out)?;
                pop.pending_births.encode(out)?;
                pop.pending_deaths.encode(out)?;
            }
            encode_length(place.buildings.len(), out)?;
            for building in &place.buildings {
                building.kind.encode(out)?;
                building.levels.encode(out)?;
                building.method_levels.encode(out)?;
                encode_level_records(&building.level_records, out)?;
                building.herds.encode(out)?;
                encode_employment(&building.employment, out)?;
                building.stock.encode(out)?;
                match &building.storehouse_accounts {
                    Some(accounts) => {
                        true.encode(out)?;
                        accounts.pops.encode(out)?;
                        accounts.communal.encode(out)?;
                    }
                    None => false.encode(out)?,
                }
                building.production_carries.encode(out)?;
                building.method_suitability.encode(out)?;
                building.herd_birth_carries.encode(out)?;
                building.herd_death_carries.encode(out)?;
                building.herd_product_carries.encode(out)?;
                building.herd_grazing_sums.encode(out)?;
                building.wage_carries.encode(out)?;
                building.pay_carries.encode(out)?;
                building.fee_carries.encode(out)?;
                building.wage_history.encode(out)?;
                building.paid_history.encode(out)?;
                building.paid_years.encode(out)?;
            }
            place.prices_milli.encode(out)?;
            place.prepared_m2.encode(out)?;
            place.development_pm.encode(out)?;
            place.price_drift_carries.encode(out)?;
            encode_place_counters(&place.counters, out)?;
        }
        Ok(())
    }
    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        let place_count = input.length()?;
        let mut places = Vec::with_capacity(place_count);
        for _ in 0..place_count {
            let id = String::decode(input)?;
            let count = input.length()?;
            let mut pops = Vec::with_capacity(count);
            for _ in 0..count {
                pops.push(PopState {
                    kind: String::decode(input)?,
                    culture: String::decode(input)?,
                    religion: String::decode(input)?,
                    literacy_pm: u16::decode(input)?,
                    workers: u64::decode(input)?,
                    dependents: u64::decode(input)?,
                    employment: decode_employment(input)?,
                    stock: Vec::<u64>::decode(input)?,
                    consumption: decode_pop_consumption(input)?,
                    counters: decode_counters(input)?,
                    short_on_rent: bool::decode(input)?,
                    housed_last: bool::decode(input)?,
                    genetics: PopGenetics::decode(input)?,
                    pending_births: u64::decode(input)?,
                    pending_deaths: u64::decode(input)?,
                });
            }
            let count = input.length()?;
            let mut buildings = Vec::with_capacity(count);
            for _ in 0..count {
                buildings.push(BuildingState {
                    kind: String::decode(input)?,
                    levels: u32::decode(input)?,
                    method_levels: Vec::<u32>::decode(input)?,
                    level_records: decode_level_records(input)?,
                    herds: Vec::<u64>::decode(input)?,
                    employment: decode_employment(input)?,
                    stock: Vec::<u64>::decode(input)?,
                    storehouse_accounts: if bool::decode(input)? {
                        Some(StorehouseAccounts {
                            pops: Vec::<Vec<u64>>::decode(input)?,
                            communal: Vec::<u64>::decode(input)?,
                        })
                    } else {
                        None
                    },
                    production_carries: Vec::<Vec<u128>>::decode(input)?,
                    method_suitability: Vec::<Vec<u64>>::decode(input)?,
                    herd_birth_carries: Vec::<u64>::decode(input)?,
                    herd_death_carries: Vec::<u64>::decode(input)?,
                    herd_product_carries: Vec::<u64>::decode(input)?,
                    herd_grazing_sums: Vec::<u64>::decode(input)?,
                    wage_carries: Vec::<u64>::decode(input)?,
                    pay_carries: Vec::<u64>::decode(input)?,
                    fee_carries: Vec::<Vec<u64>>::decode(input)?,
                    wage_history: Vec::<u64>::decode(input)?,
                    paid_history: Vec::<u64>::decode(input)?,
                    paid_years: Vec::<u16>::decode(input)?,
                });
            }
            places.push(PlaceState {
                id,
                pops,
                buildings,
                prices_milli: Vec::<u64>::decode(input)?,
                prepared_m2: u64::decode(input)?,
                development_pm: u32::decode(input)?,
                price_drift_carries: Vec::<i64>::decode(input)?,
                last_market_orders: None,
                counters: decode_place_counters(input)?,
            });
        }
        Ok(Self { places })
    }
}

fn encode_level_records(records: &[LevelRecord], out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    encode_length(records.len(), out)?;
    for record in records {
        record.number.encode(out)?;
        record.method.encode(out)?;
        match &record.ground {
            Some(ground) => {
                true.encode(out)?;
                ground.encode(out)?;
            }
            None => false.encode(out)?,
        }
        record.paydays.encode(out)?;
        record.unpaid_work.encode(out)?;
        match record.last_switch_year {
            Some(year) => {
                true.encode(out)?;
                year.encode(out)?;
            }
            None => false.encode(out)?,
        }
    }
    Ok(())
}

fn decode_level_records(input: &mut Reader<'_>) -> Result<Vec<LevelRecord>, CanonicalError> {
    let count = input.length()?;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        records.push(LevelRecord {
            number: u32::decode(input)?,
            method: u32::decode(input)?,
            ground: if bool::decode(input)? {
                Some(String::decode(input)?)
            } else {
                None
            },
            paydays: u8::decode(input)?,
            unpaid_work: BTreeMap::decode(input)?,
            last_switch_year: if bool::decode(input)? {
                Some(u16::decode(input)?)
            } else {
                None
            },
        });
    }
    Ok(records)
}

fn encode_pop_consumption(value: &PopConsumption, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    value.pantry_hunger_centi.encode(out)?;
    value.necessities_centi.encode(out)?;
    value.niceties_centi.encode(out)?;
    value.luxuries_centi.encode(out)?;
    value.ticking_carries.encode(out)?;
    value.necessities_pm.encode(out)?;
    value.niceties_pm.encode(out)?;
    value.luxuries_pm.encode(out)?;
    value.satisfaction.encode(out)?;
    for carry in value.drift_carries {
        carry.encode(out)?;
    }
    value.birth_carry.encode(out)?;
    value.worker_death_carry.encode(out)?;
    value.dependent_death_carry.encode(out)?;
    value.hearth_milli.encode(out)?;
    value.fuel_need_month.encode(out)?;
    value.fuel_burned_month.encode(out)?;
    value.fuel_death_carry.encode(out)?;
    value.dependent_drift_carry.encode(out)?;
    Ok(())
}

fn decode_pop_consumption(input: &mut Reader<'_>) -> Result<PopConsumption, CanonicalError> {
    Ok(PopConsumption {
        pantry_hunger_centi: Vec::<u64>::decode(input)?,
        necessities_centi: u64::decode(input)?,
        niceties_centi: u64::decode(input)?,
        luxuries_centi: u64::decode(input)?,
        ticking_carries: Vec::<u64>::decode(input)?,
        necessities_pm: u16::decode(input)?,
        niceties_pm: u16::decode(input)?,
        luxuries_pm: u16::decode(input)?,
        satisfaction: i16::decode(input)?,
        drift_carries: [
            i64::decode(input)?,
            i64::decode(input)?,
            i64::decode(input)?,
            i64::decode(input)?,
        ],
        birth_carry: u64::decode(input)?,
        worker_death_carry: u64::decode(input)?,
        dependent_death_carry: u64::decode(input)?,
        hearth_milli: u64::decode(input)?,
        fuel_need_month: u64::decode(input)?,
        fuel_burned_month: u64::decode(input)?,
        fuel_death_carry: u64::decode(input)?,
        dependent_drift_carry: i64::decode(input)?,
    })
}

fn encode_employment(items: &[Employment], out: &mut Vec<u8>) -> Result<(), CanonicalError> {
    encode_length(items.len(), out)?;
    for item in items {
        item.owners.encode(out)?;
        item.workers.encode(out)?;
    }
    Ok(())
}

fn decode_employment(input: &mut Reader<'_>) -> Result<Vec<Employment>, CanonicalError> {
    let count = input.length()?;
    let mut result = Vec::with_capacity(count);
    for _ in 0..count {
        result.push(Employment {
            owners: u64::decode(input)?,
            workers: u64::decode(input)?,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod events_tests;
#[cfg(test)]
mod genetics_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod noble_tests;
#[cfg(test)]
mod personality_tests;
#[cfg(test)]
mod start_levels_tests;
#[cfg(test)]
mod swap_oracle_tests;
#[cfg(test)]
mod swap_tests;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn natural_shelter_uses_literal_weighted_counts_across_three_terrains() {
        let mut rules = tiny_rules();
        rules.shelter_pm.insert("forest".to_owned(), 75);
        let land = LandCounts {
            land_cells: BTreeMap::from([
                ("pasture".to_owned(), 18),
                ("forest".to_owned(), 12),
                ("bog".to_owned(), 22),
            ]),
            ..LandCounts::default()
        };

        // (18 × 50 + 12 × 75 + 22 × 10) / 1000 = 2.02, floored to 2.
        assert_eq!(natural_shelter(&rules, &land).unwrap(), 2);
    }

    #[test]
    fn buildable_area_uses_all_land_and_scales_with_checked_development() {
        let rules = tiny_rules();
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".to_owned(), 1)]),
            fertility_pm: BTreeMap::new(),
            fertile_cells: 0,
            fertile_cells_by_terrain: BTreeMap::new(),
            sea: 0,
        };
        assert_eq!(max_buildable_m2(&rules, 1000, &land).unwrap(), 20_300);
        assert_eq!(max_buildable_m2(&rules, 500, &land).unwrap(), 10_150);
        assert_eq!(max_buildable_m2(&rules, 2000, &land).unwrap(), 40_600);
        let mixed = LandCounts {
            land_cells: BTreeMap::from([("pasture".to_owned(), 2), ("bog".to_owned(), 3)]),
            ..land.clone()
        };
        assert_eq!(max_buildable_m2(&rules, 1000, &mixed).unwrap(), 21_050);
        let huge = LandCounts {
            land_cells: BTreeMap::from([("pasture".to_owned(), u32::MAX)]),
            ..land
        };
        let mut huge_rules = rules.clone();
        huge_rules
            .buildable_per_cell_m2
            .insert("pasture".to_owned(), u64::MAX);
        assert!(max_buildable_m2(&huge_rules, u32::MAX, &huge).is_err());
    }

    #[test]
    fn farmland_quality_is_the_floored_mean_of_fertile_cells_only() {
        assert_eq!(mean_fertility(&LandCounts::default()).unwrap(), 0);
        assert_eq!(
            mean_fertility(&LandCounts {
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 4)]),
                fertile_cells: 1,
                fertile_cells_by_terrain: BTreeMap::new(),
                ..LandCounts::default()
            })
            .unwrap(),
            4
        );
        assert_eq!(
            mean_fertility(&LandCounts {
                fertility_pm: BTreeMap::from(
                    [("pasture".to_owned(), 8), ("forest".to_owned(), 5),]
                ),
                fertile_cells: 3,
                fertile_cells_by_terrain: BTreeMap::new(),
                ..LandCounts::default()
            })
            .unwrap(),
            4
        );
    }

    #[test]
    fn starting_footprints_are_saved_and_construction_obeys_the_smaller_cap() {
        let data = tiny_start_data();
        let location = StartLocation {
            id: "built".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 8)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 8000)]),
                fertile_cells: 8,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        };
        let rules = tiny_rules();
        let (state, report) =
            starting_state(&rules, &data, std::slice::from_ref(&location), 10).unwrap();
        assert!(report.is_empty());
        let place = &state.places[0];
        assert_eq!(place.prepared_m2, 22_200);
        assert_eq!(place.development_pm, 1000);
        let footprint = building_footprint(&rules, &place.buildings).unwrap();
        let mut switched = place.buildings.clone();
        switched[0].method_levels = vec![0, 1];
        assert_eq!(building_footprint(&rules, &switched).unwrap(), footprint);
        assert!(can_build(&rules, place, &location.land, 20_000).unwrap());
        assert!(!can_build(&rules, place, &location.land, 20_001).unwrap());

        let mut overbuilt_rules = rules;
        overbuilt_rules
            .footprint_m2
            .insert("farm".to_owned(), 30_000);
        let (overbuilt, report) =
            starting_state(&overbuilt_rules, &data, std::slice::from_ref(&location), 10).unwrap();
        assert_eq!(
            report,
            [
                "built starts over building cap: footprint 30200 m², prepared 50200 m², max_buildable 22400 m²"
            ]
        );
        let place = &overbuilt.places[0];
        assert!(!can_build(&overbuilt_rules, place, &location.land, 0).unwrap());
        assert!(!can_build(&overbuilt_rules, place, &location.land, 1).unwrap());
    }

    type RuleMutation = Box<dyn Fn(&mut Rules)>;

    fn fixture_culture(id: &str) -> peoples::Culture {
        peoples::Culture {
            id: id.to_owned(),
            subgroup: "fixture".into(),
            superculture: "fixture".into(),
            language: "fixture".into(),
            dialect: None,
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "The compact test rule fixture keeps the complete valid schema visible for every economy vector."
    )]
    pub(crate) fn tiny_rules() -> Rules {
        Rules {
            herd_start_share: [2, 3],
            herd_standard_pm: 700,
            herd_opening: HerdOpening::default(),
            herd_species: Vec::new(),
            goods: vec![Good {
                id: "food".to_owned(),
                tags: Vec::new(),
                supplies: BTreeMap::new(),
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
                stacked_outside: false,
                hunger_centi: 100,
                necessities_centi: 100,
                niceties_centi: 0,
                luxuries_centi: 0,
                base_price: 100,
                pop_limit_milli: 1000,
            }],
            pop_types: vec![PopType {
                id: "peasant".to_owned(),
                dependent_pm: 400,
                births_pm_year: 67,
                worker_deaths_pm_year: 20,
                dependent_deaths_pm_year: 60,
                necessities_expectation_pm: 1000,
                niceties_expectation_pm: 0,
                luxuries_expectation_pm: 0,
                fuel_need_milli_day: 0,
            }],
            starting_needs_pm: BTreeMap::from([("peasant".to_owned(), [1000, 0, 0])]),
            peoples: Peoples {
                cultures: vec![fixture_culture("connachta")],
                ..Peoples::default()
            },
            genetics: Genetics::default(),
            personality: Personality::default(),
            events: EventRules::default(),
            lifecycle: LifeCycle::default(),
            religions: vec!["christianity".to_owned()],
            terrain_order: vec!["pasture".to_owned(), "bog".to_owned()],
            footprint_m2: BTreeMap::from([
                ("farm".to_owned(), 2000),
                ("storehouse".to_owned(), 200),
            ]),
            housing_per_level: BTreeMap::from([
                ("farm".to_owned(), 17),
                ("storehouse".to_owned(), 3),
            ]),
            housing_class: BTreeMap::from([
                ("farm".to_owned(), "basic".to_owned()),
                ("storehouse".to_owned(), "basic".to_owned()),
            ]),
            housing_classes: vec!["makeshift".to_owned(), "basic".to_owned()],
            shelter_pm: BTreeMap::from([("pasture".to_owned(), 50), ("bog".to_owned(), 10)]),
            fuel_deaths_pm_year: 0,
            hire_pm: 100,
            leave_pm: 50,
            buildable_base_m2: 20_000,
            buildable_per_cell_m2: BTreeMap::from([
                ("pasture".to_owned(), 300),
                ("bog".to_owned(), 150),
            ]),
            starting_development_pm: 1000,
            fertility_reference_pm: 550,
            buildings: vec![
                BuildingType {
                    id: "farm".to_owned(),
                    church: false,
                    herds: BTreeMap::new(),
                    workers_per_level: 10,
                    land_per_level: 2,
                    owner_workers: BTreeMap::from([("peasant".to_owned(), 10)]),
                    penalty_pm: 500,
                    overhead_pm: 100,
                    recruit_range_m: Some(30_000),
                    market_capacity_ml_per_level: 0,
                    methods: vec![Method {
                        id: "mixed".to_owned(),
                        resource: "usable_land".to_owned(),
                        cells_per_level: None,
                        annual_output: BTreeMap::from([("food".to_owned(), 1)]),
                        suitability: "crops".to_owned(),
                        score_share_pm: BTreeMap::from([("food".to_owned(), 1000)]),
                        serves: "food".to_owned(),
                    }],
                    owner_kind: None,
                },
                BuildingType {
                    id: "storehouse".to_owned(),
                    church: false,
                    herds: BTreeMap::new(),
                    workers_per_level: 2,
                    land_per_level: 0,
                    owner_workers: BTreeMap::new(),
                    penalty_pm: 0,
                    overhead_pm: 0,
                    recruit_range_m: Some(0),
                    market_capacity_ml_per_level: 0,
                    methods: Vec::new(),
                    owner_kind: None,
                },
            ],
            seasons: BTreeMap::from(
                ["grain", "food"]
                    .map(|key| (key.to_owned(), vec![0, 0, 0, 0, 0, 0, 0, 0, 1000, 0, 0, 0])),
            ),
            curves: BTreeMap::from([
                (
                    "intake_food_days".to_owned(),
                    vec![[30, 500], [120, 800], [240, 1000]],
                ),
                ("births_food".to_owned(), vec![[0, 1000], [2000, 1000]]),
                ("births_crowding".to_owned(), vec![[0, 1000], [2000, 1000]]),
                (
                    "births_satisfaction".to_owned(),
                    vec![[-100, 1000], [100, 1000]],
                ),
                ("births_literacy".to_owned(), vec![[0, 1000], [1000, 1000]]),
                ("deaths_hunger".to_owned(), vec![[0, 1000], [1000, 1000]]),
                ("deaths_comfort".to_owned(), vec![[0, 1000], [1000, 1000]]),
                ("deaths_crowding".to_owned(), vec![[0, 1000], [2000, 1000]]),
            ]),
            ticking_rates_million: BTreeMap::new(),
            diet_profiles: BTreeMap::from([(
                "peasant".to_owned(),
                BTreeMap::from([("food".to_owned(), 1000)]),
            )]),
            comfort_profiles: BTreeMap::from([(
                "peasant".to_owned(),
                BTreeMap::from([
                    ("niceties".to_owned(), BTreeMap::new()),
                    ("luxuries".to_owned(), BTreeMap::new()),
                ]),
            )]),
            suitability: BTreeMap::from([(
                "pasture".to_owned(),
                BTreeMap::from([("crops".to_owned(), 1000), ("food".to_owned(), 1000)]),
            )]),
            grazing: BTreeMap::from([("pasture".to_owned(), BTreeMap::new())]),
            method_switch_margin_pm: 200,
            price_k_pm: 500,
            swap_loss_limit_pm: 100,
        }
    }

    fn add_empty_comfort_profile(rules: &mut Rules, pop_kind: &str) {
        rules.comfort_profiles.insert(
            pop_kind.to_owned(),
            BTreeMap::from([
                ("niceties".to_owned(), BTreeMap::new()),
                ("luxuries".to_owned(), BTreeMap::new()),
            ]),
        );
    }

    fn install_fixture_herds(rules: &mut Rules) {
        let shipped = shipped_rules();
        for id in [
            "cattle", "pigs", "sheep", "beef", "pork", "mutton", "dairy", "wool", "hides",
        ] {
            if !rules.goods.iter().any(|good| good.id == id) {
                rules.goods.push(
                    shipped
                        .goods
                        .iter()
                        .find(|good| good.id == id)
                        .unwrap()
                        .clone(),
                );
            }
        }
        rules.herd_species = shipped.herd_species.into_iter().take(3).collect();
        rules.buildings[0].herds = shipped
            .buildings
            .iter()
            .find(|building| building.id == "farm")
            .unwrap()
            .herds
            .clone();
        for terrain in &rules.terrain_order {
            rules.grazing.entry(terrain.clone()).or_default().extend([
                ("cattle".into(), 1000),
                ("pigs".into(), 1000),
                ("sheep".into(), 1000),
            ]);
        }
        rules
            .seasons
            .entry("dairy".into())
            .or_insert_with(|| vec![0, 0, 0, 0, 0, 1000, 0, 0, 0, 0, 0, 0]);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "This daily fixture wires two pops, owner herds, a storehouse account and rules-indexed stocks together."
    )]
    fn daily_accounting_fixture() -> (Rules, EconomyState) {
        let mut rules = tiny_rules();
        for (id, hunger, necessities, niceties, luxuries, cap) in [
            ("beef", 60, 60, 30, 0, 1_000),
            ("pork", 70, 70, 35, 0, 1_000),
            ("mutton", 60, 60, 10, 0, 1_000),
            ("hides", 0, 0, 0, 0, 1_000),
            ("herbs", 0, 10, 20, 30, 1_000),
            ("beeswax", 0, 0, 20, 0, 1_000),
        ] {
            rules.goods.push(Good {
                id: id.to_owned(),
                tags: Vec::new(),
                supplies: BTreeMap::new(),
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
                stacked_outside: false,
                hunger_centi: hunger,
                necessities_centi: necessities,
                niceties_centi: niceties,
                luxuries_centi: luxuries,
                base_price: 1,
                pop_limit_milli: cap,
            });
        }
        rules.ticking_rates_million.insert(
            "herbs".to_owned(),
            BTreeMap::from([("peasant".to_owned(), 1_000_000)]),
        );
        rules.ticking_rates_million.insert(
            "beeswax".to_owned(),
            BTreeMap::from([("peasant".to_owned(), 1_000_000)]),
        );
        install_fixture_herds(&mut rules);
        let goods = rules.goods.len();
        let make_pop = |necessities_pm| PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 1,
            dependents: 0,
            employment: vec![Employment::default(); 2],
            stock: vec![0; goods],
            consumption: PopConsumption {
                pantry_hunger_centi: vec![0; goods],
                ticking_carries: vec![0; goods],
                necessities_pm,
                ..PopConsumption::default()
            },
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let mut drawer = make_pop(1000);
        let mut starver = make_pop(200);
        let food_index = rules
            .goods
            .iter()
            .position(|good| good.id == "food")
            .unwrap();
        let herb_index = rules
            .goods
            .iter()
            .position(|good| good.id == "herbs")
            .unwrap();
        starver.stock[herb_index] = 1;
        let mut storehouse_stock = vec![0; goods];
        storehouse_stock[food_index] = 1;
        let mut drawer_account = vec![0; goods];
        drawer_account[food_index] = 1;
        let storehouse = BuildingState {
            kind: "storehouse".to_owned(),
            levels: 1,
            method_levels: vec![1, 0, 0],
            level_records: Vec::new(),
            herds: Vec::new(),
            employment: vec![Employment::default(); 2],
            stock: storehouse_stock,
            storehouse_accounts: Some(StorehouseAccounts {
                pops: vec![drawer_account, vec![0; goods]],
                communal: vec![0; goods],
            }),
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: vec![0; 2],
            pay_carries: vec![0; 2],
            fee_carries: vec![vec![0; 2]; 3],
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
            level_records: vec![LevelRecord {
                number: 0,
                method: 0,
                ground: Some("pasture".to_owned()),
                paydays: MATURE_PAYDAYS,
                unpaid_work: BTreeMap::new(),
                last_switch_year: None,
            }],
            herds: vec![0, 1, 0],
            employment: vec![
                Employment::default(),
                Employment {
                    owners: 1,
                    workers: 0,
                },
            ],
            stock: vec![0; goods],
            storehouse_accounts: None,
            production_carries: vec![vec![0; goods]],
            method_suitability: vec![vec![1000; goods]],
            herd_birth_carries: vec![0; 3],
            herd_death_carries: vec![0; 3],
            herd_product_carries: vec![0; 2],
            herd_grazing_sums: vec![1000; 3],
            wage_carries: vec![0; 2],
            pay_carries: vec![0; 2],
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        drawer.employment = vec![Employment::default(); 2];
        starver.employment = vec![Employment::default(); 2];
        let state = EconomyState {
            places: vec![PlaceState {
                id: "daily_fixture".to_owned(),
                pops: vec![drawer, starver],
                buildings: vec![storehouse, farm],
                prices_milli: vec![1000; goods],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: vec![0; goods],
                last_market_orders: None,
                counters: CounterBuckets::for_place(rules.goods.len()),
            }],
        };
        (rules, state)
    }

    /// The ground fixture levels stand on: pasture, which `flatten_pasture` gives a suitability of 1000 for every good and
    /// the first three species grazing of 1000, 400 and 1000, so a fixture level on it makes what the old fixtures' hand-set
    /// sums of 1000 did.
    const FLAT: &str = "pasture";

    fn flatten_pasture(rules: &mut Rules) {
        let mut table = BTreeMap::new();
        for good in &rules.goods {
            table.insert(good.id.clone(), 1000_u16);
        }
        for building in &rules.buildings {
            for method in &building.methods {
                table.insert(method.suitability.clone(), 1000);
            }
        }
        rules.suitability.insert(FLAT.to_owned(), table);
        let grazing = rules
            .herd_species
            .iter()
            .enumerate()
            .map(|(index, species)| {
                (
                    species.animal.clone(),
                    [1000, 400, 1000].get(index).copied().unwrap_or(1000),
                )
            })
            .collect();
        rules.grazing.insert(FLAT.to_owned(), grazing);
    }

    /// Rebuild a fixture building's level records from its method counts, every level mature on the flat ground (forest and bog
    /// levels on their own), and its derived sums from the records.
    fn sync_levels(rules: &Rules, building: &mut BuildingState) {
        let definition = rules
            .buildings
            .iter()
            .find(|item| item.id == building.kind)
            .unwrap();
        if definition.methods.is_empty() {
            return;
        }
        building.level_records.clear();
        for (method, count) in definition
            .methods
            .iter()
            .zip(building.method_levels.clone())
        {
            let ground = match method.resource.as_str() {
                "usable_land" => Some(FLAT.to_owned()),
                "sea" => None,
                other => Some(other.to_owned()),
            };
            let method_index = definition
                .methods
                .iter()
                .position(|item| item.id == method.id)
                .unwrap();
            for _ in 0..count {
                building.level_records.push(LevelRecord {
                    number: u32::try_from(building.level_records.len()).unwrap(),
                    method: u32::try_from(method_index).unwrap(),
                    ground: ground.clone(),
                    paydays: MATURE_PAYDAYS,
                    unpaid_work: seasonal_accounts(rules, definition, method),
                    last_switch_year: None,
                });
            }
        }
        let (suitability, grazing) =
            derive_level_sums(rules, definition, &building.level_records).unwrap();
        building.method_suitability = suitability;
        building.herd_grazing_sums = grazing;
    }

    fn sync_all_levels(rules: &Rules, state: &mut EconomyState) {
        for place in &mut state.places {
            for building in &mut place.buildings {
                sync_levels(rules, building);
            }
        }
    }

    pub(super) fn shipped_production_fixture(rules: &Rules) -> EconomyState {
        assert_eq!(
            rules.suitability[FLAT]["oats"], 1000,
            "flatten the fixture's rules with flatten_pasture"
        );
        let mut state = shipped_production_fixture_unsynced(rules);
        sync_all_levels(rules, &mut state);
        state
    }

    #[expect(
        clippy::too_many_lines,
        reason = "The shipped production fixture keeps all rules-indexed state dimensions visible to its literal season test."
    )]
    fn shipped_production_fixture_unsynced(rules: &Rules) -> EconomyState {
        let goods = rules.goods.len();
        let empty_stock = vec![0; goods];
        let pop = |kind: &str, workers, employment| PopState {
            kind: kind.to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers,
            dependents: 0,
            employment,
            stock: empty_stock.clone(),
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        EconomyState {
            places: vec![PlaceState {
                id: "fixture".to_owned(),
                pops: vec![
                    pop(
                        "peasant",
                        10,
                        vec![
                            Employment {
                                owners: 1,
                                workers: 10,
                            },
                            Employment::default(),
                            Employment::default(),
                        ],
                    ),
                    pop(
                        "monk",
                        5,
                        vec![
                            Employment::default(),
                            Employment {
                                owners: 1,
                                workers: 5,
                            },
                            Employment::default(),
                        ],
                    ),
                    pop(
                        "labourer",
                        5,
                        vec![
                            Employment::default(),
                            Employment {
                                owners: 1,
                                workers: 5,
                            },
                            Employment::default(),
                        ],
                    ),
                ],
                buildings: vec![
                    BuildingState {
                        kind: "farm".to_owned(),
                        levels: 1,
                        method_levels: vec![1, 0, 0, 0],
                        level_records: vec![LevelRecord {
                            number: 0,
                            method: 0,
                            ground: Some("pasture".to_owned()),
                            paydays: MATURE_PAYDAYS,
                            unpaid_work: BTreeMap::new(),
                            last_switch_year: None,
                        }],
                        herds: (0..rules.herd_species.len())
                            .map(|i| [9, 6, 5].get(i).copied().unwrap_or(0))
                            .collect(),
                        employment: vec![
                            Employment {
                                owners: 1,
                                workers: 10,
                            },
                            Employment::default(),
                            Employment::default(),
                        ],
                        stock: empty_stock.clone(),
                        storehouse_accounts: None,
                        production_carries: vec![vec![0; goods]; 4],
                        method_suitability: vec![vec![1000; goods]; 4],
                        herd_birth_carries: vec![0; rules.herd_species.len()],
                        herd_death_carries: vec![0; rules.herd_species.len()],
                        herd_product_carries: vec![
                            0;
                            rules
                                .herd_species
                                .iter()
                                .map(|s| s.products.len())
                                .sum()
                        ],
                        herd_grazing_sums: (0..rules.herd_species.len())
                            .map(|i| [1000, 400, 1000].get(i).copied().unwrap_or(1000))
                            .collect(),
                        wage_carries: vec![0; goods],
                        pay_carries: vec![0; goods],
                        fee_carries: Vec::new(),
                        wage_history: vec![0; 12],
                        paid_history: vec![0; WAGE_HISTORY_MONTHS],
                        paid_years: vec![0; WAGE_HISTORY_MONTHS],
                    },
                    BuildingState {
                        kind: "holdings".to_owned(),
                        levels: 1,
                        method_levels: vec![0, 1],
                        level_records: Vec::new(),
                        herds: Vec::new(),
                        employment: vec![
                            Employment::default(),
                            Employment {
                                owners: 1,
                                workers: 5,
                            },
                            Employment {
                                owners: 1,
                                workers: 5,
                            },
                        ],
                        stock: empty_stock.clone(),
                        storehouse_accounts: None,
                        production_carries: vec![vec![0; goods]; 2],
                        method_suitability: vec![vec![1000; goods]; 2],
                        herd_birth_carries: Vec::new(),
                        herd_death_carries: Vec::new(),
                        herd_product_carries: Vec::new(),
                        herd_grazing_sums: Vec::new(),
                        wage_carries: vec![0; goods],
                        pay_carries: vec![0; goods],
                        fee_carries: Vec::new(),
                        wage_history: vec![0; 12],
                        paid_history: vec![0; WAGE_HISTORY_MONTHS],
                        paid_years: vec![0; WAGE_HISTORY_MONTHS],
                    },
                    BuildingState {
                        kind: "storehouse".to_owned(),
                        levels: 1,
                        method_levels: vec![1, 1, 1],
                        level_records: Vec::new(),
                        herds: Vec::new(),
                        employment: vec![Employment::default(); 3],
                        stock: empty_stock,
                        storehouse_accounts: Some(StorehouseAccounts {
                            pops: vec![vec![0; goods]; 3],
                            communal: vec![0; goods],
                        }),
                        production_carries: Vec::new(),
                        method_suitability: Vec::new(),
                        herd_birth_carries: Vec::new(),
                        herd_death_carries: Vec::new(),
                        herd_product_carries: Vec::new(),
                        herd_grazing_sums: Vec::new(),
                        wage_carries: vec![0; goods],
                        pay_carries: vec![0; goods],
                        fee_carries: vec![vec![0; goods]; 4],
                        wage_history: vec![0; 12],
                        paid_history: vec![0; WAGE_HISTORY_MONTHS],
                        paid_years: vec![0; WAGE_HISTORY_MONTHS],
                    },
                ],
                prices_milli: vec![0; goods],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        }
    }

    fn two_method_staffing_fixture(annual: u64) -> (Rules, EconomyState) {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        let holdings = rules
            .buildings
            .iter_mut()
            .find(|building| building.id == "holdings")
            .unwrap();
        holdings.methods[0].annual_output = BTreeMap::from([("oats".to_owned(), annual)]);
        holdings.methods[0].score_share_pm = BTreeMap::from([("oats".to_owned(), 1000)]);
        holdings.methods[1].annual_output = BTreeMap::from([("fish".to_owned(), annual)]);
        holdings.methods[1].score_share_pm = BTreeMap::from([("fish".to_owned(), 1000)]);
        let season = vec![100, 100, 100, 700, 0, 0, 0, 0, 0, 0, 0, 0];
        rules.seasons.insert("grain".to_owned(), season.clone());
        rules.seasons.insert("fish".to_owned(), season);

        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.buildings[0].levels = 0;
        place.buildings[0].method_levels = vec![0, 0, 0, 0];
        place.buildings[1].levels = 2;
        place.buildings[1].method_levels = vec![1, 1];
        sync_all_levels(&rules, &mut state);
        (rules, state)
    }

    fn set_holdings_staffing(
        state: &mut EconomyState,
        monk_owners: u64,
        monk_workers: u64,
        labourer_owners: u64,
        labourer_workers: u64,
    ) {
        let place = &mut state.places[0];
        for (pop_index, owners, workers) in [
            (1, monk_owners, monk_workers),
            (2, labourer_owners, labourer_workers),
        ] {
            place.buildings[1].employment[pop_index] = Employment { owners, workers };
            place.pops[pop_index].employment[1] = Employment { owners, workers };
            place.pops[pop_index].workers = workers;
        }
    }

    fn daily_storehouse_fee_state() -> (Rules, EconomyState) {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        rules.price_k_pm = 0;
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.prices_milli = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        place.price_drift_carries = vec![0; rules.goods.len()];
        for pop in &mut place.pops {
            pop.consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
            pop.consumption.ticking_carries = vec![0; rules.goods.len()];
        }
        place.pops[0].workers = 20;
        place.buildings[0].employment[0].workers = 10;
        place.pops[0].employment[0].workers = 10;
        place.buildings[1].employment[1].workers = 4;
        place.pops[1].employment[1].workers = 4;
        place.buildings[1].employment[2].workers = 4;
        place.pops[2].employment[1].workers = 4;
        (rules, state)
    }

    #[test]
    fn production_payday_fills_all_worker_stores_before_alms() {
        let (mut rules, mut state) = daily_accounting_fixture();
        let early = 0;
        let later = rules.goods.iter().position(|g| g.id == "beef").unwrap();
        let filler = rules.goods.iter().position(|g| g.id == "hides").unwrap();
        for good in [early, later, filler] {
            rules.goods[good].volume_ml = 720_000;
            rules.goods[good].pop_limit_milli = 1_000;
        }
        let place = &mut state.places[0];
        place.buildings.remove(0);
        let mut first = place.buildings[0].clone();
        first.herds.clear();
        first.method_levels.fill(0);
        first.level_records.clear();
        first.production_carries = vec![vec![0; rules.goods.len()]];
        first.method_suitability = vec![vec![1_000; rules.goods.len()]];
        first.wage_carries = vec![0; rules.goods.len()];
        first.pay_carries = vec![0; rules.goods.len()];
        first.pay_carries[early] = 1;
        first.employment = vec![
            Employment {
                owners: 0,
                workers: 1,
            },
            Employment::default(),
        ];
        let mut second = first.clone();
        second.pay_carries[early] = 0;
        second.pay_carries[later] = 1;
        second.employment = vec![
            Employment::default(),
            Employment {
                owners: 0,
                workers: 1,
            },
        ];
        place.buildings = vec![first, second];
        for pop in &mut place.pops {
            pop.workers = 1;
            pop.dependents = 0;
            pop.stock.fill(0);
            pop.employment = vec![Employment::default(); 2];
        }
        place.pops[0].stock[filler] = 1;
        place.pops[0].employment[0].workers = 1;
        place.pops[1].employment[1].workers = 1;
        let ledger = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let place = &state.places[0];
        assert_eq!(place.pops[1].stock[early], 0);
        assert_eq!(
            place.pops[1].stock[later], 1,
            "the later wage takes its own store first"
        );
        assert_eq!(ledger.lost[early], 1);
        assert_eq!(place.pops[0].counters.current.losses, 1);
    }

    #[test]
    fn production_payday_gives_last_building_room_to_earlier_pop() {
        let (mut rules, mut state) = daily_accounting_fixture();
        let good = 0;
        rules.goods[good].pop_limit_milli = 450_000;
        rules.goods[good].volume_ml = 1;
        let place = &mut state.places[0];
        place.buildings.remove(0);
        let mut source = place.buildings[0].clone();
        source.herds.clear();
        source.method_levels.fill(0);
        source.level_records.clear();
        source.production_carries = vec![vec![0; rules.goods.len()]];
        source.method_suitability = vec![vec![1_000; rules.goods.len()]];
        source.wage_carries = vec![0; rules.goods.len()];
        source.pay_carries = vec![0; rules.goods.len()];
        source.pay_carries[good] = 2;
        source.stock[good] = 120;
        source.employment = vec![
            Employment {
                owners: 0,
                workers: 1
            };
            2
        ];
        let mut target = source.clone();
        target.pay_carries[good] = 0;
        target.stock[good] = 59;
        target.employment = vec![
            Employment {
                owners: 1,
                workers: 1,
            },
            Employment {
                owners: 1,
                workers: 0,
            },
        ];
        place.buildings = vec![source, target];
        for pop in &mut place.pops {
            pop.kind = "labourer".to_owned();
            pop.workers = 1;
            pop.dependents = 0;
            pop.stock.fill(0);
            pop.stock[good] = 450;
            pop.employment = vec![Employment::default(); 2];
        }
        let mut two_goods_rules = rules.clone();
        let later = two_goods_rules
            .goods
            .iter()
            .position(|g| g.id == "hides")
            .unwrap();
        two_goods_rules.goods[good].volume_ml = 100_000;
        two_goods_rules.goods[later].volume_ml = 100_000;
        two_goods_rules.goods[later].pop_limit_milli = 450_000;
        let mut two_goods = state.clone();
        let place = &mut two_goods.places[0];
        place.buildings[0].stock[good] = 2;
        place.buildings[0].pay_carries[good] = 1;
        place.buildings[0].pay_carries[later] = 1;
        place.buildings[0].employment[1].workers = 0;
        place.buildings[1].stock[good] = 0;
        let date = Date {
            year: 768,
            month: 1,
            day: 1,
            hour: 12,
        };
        let ledger = run_monthly_production(&rules, &mut state, date).unwrap();
        let place = &state.places[0];
        assert_eq!(place.buildings[1].stock[good], 60);
        assert_eq!(place.pops[0].counters.current.losses, 0);
        assert_eq!(place.pops[1].counters.current.losses, 1);
        assert_eq!(ledger.lost[good], 1);
        let ledger = run_monthly_production(&two_goods_rules, &mut two_goods, date).unwrap();
        let place = &two_goods.places[0];
        assert_eq!(place.buildings[1].stock[good], 1);
        assert_eq!(place.buildings[1].stock[later], 0);
        assert_eq!(ledger.lost[later], 1);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The same fixture checks the fee both directly and through the monthly step."
    )]
    fn fee_payday_settles_each_good_before_paying_the_next() {
        let (mut rules, mut state) = daily_accounting_fixture();
        let early = 0; // Fixture food is not accepted by a storehouse.
        let later = rules.goods.iter().position(|g| g.id == "beef").unwrap();
        let filler = rules.goods.iter().position(|g| g.id == "hides").unwrap();
        for good in [early, later, filler] {
            rules.goods[good].volume_ml = 720_000;
            rules.goods[good].pop_limit_milli = 1_000;
        }
        let place = &mut state.places[0];
        place.buildings.truncate(1);
        for pop in &mut place.pops {
            pop.kind = "peasant".to_owned();
            pop.workers = 1;
            pop.dependents = 0;
            pop.stock.fill(0);
            pop.employment = vec![Employment {
                owners: 0,
                workers: 1,
            }];
        }
        place.pops[0].stock[filler] = 1;
        let building = &mut place.buildings[0];
        building.method_levels = vec![0, 1, 0];
        building.employment = vec![
            Employment {
                owners: 0,
                workers: 1
            };
            2
        ];
        building.stock.fill(0);
        let accounts = building.storehouse_accounts.as_mut().unwrap();
        for account in &mut accounts.pops {
            account.fill(0);
        }
        accounts.communal.fill(0);
        building.fee_carries = vec![vec![0; rules.goods.len()]; 3];
        building.pay_carries = vec![0; rules.goods.len()];
        building.pay_carries[early] = 1;
        building.pay_carries[later] = 2;
        let mut monthly = EconomyState {
            places: vec![place.clone()],
        };
        let mut monthly_ledger = ProductionLedger::new(rules.goods.len(), rules.herd_species.len());
        monthly_ledger.places.push(PlaceProductionLedger::new(
            &monthly.places[0].id,
            rules.goods.len(),
            rules.herd_species.len(),
        ));
        monthly_ledger.places[0].stock_after =
            total_stored_goods(&rules, &monthly.places[0]).unwrap();
        monthly_ledger.stock_after = monthly_ledger.places[0].stock_after.clone();
        run_monthly_demography_with_floors(
            &rules,
            &mut monthly,
            &BTreeMap::new(),
            &BTreeMap::new(),
            Some((0, 768, &mut monthly_ledger)),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(monthly.places[0].pops[1].stock[early], 1);
        assert_eq!(
            monthly.places[0].buildings[0]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .pops[1][later],
            1
        );
        let mut ledger =
            PlaceProductionLedger::new("fee", rules.goods.len(), rules.herd_species.len());
        pay_storehouse_fee(
            &rules,
            "fee",
            &place.prices_milli,
            0,
            768,
            &mut place.pops,
            &mut place.buildings,
            0,
            &mut place.counters,
            &mut ledger,
        )
        .unwrap();
        assert_eq!(
            place.pops[1].stock[early], 1,
            "earlier good reaches B as alms"
        );
        assert_eq!(place.pops[1].stock[later], 0);
        let accounts = place.buildings[0].storehouse_accounts.as_ref().unwrap();
        assert_eq!(accounts.pops[0][later], 1);
        assert_eq!(
            accounts.pops[1][later], 1,
            "B's later wage goes to its account"
        );
        assert_eq!(
            place.buildings[0].stock,
            storehouse_physical_stock(&rules, accounts)
        );
        assert_eq!(ledger.lost.iter().sum::<u64>(), 0);
        assert_eq!(place.buildings[0].paid_history[0], 2);
        assert_eq!(
            place.buildings[0].wage_history[0], 0,
            "old carries are not new fee value"
        );
    }

    #[test]
    fn outside_stacks_obey_caps_but_do_not_fill_stores() {
        let mut rules = tiny_rules();
        rules.goods[0].volume_ml = 720_000;
        rules.goods[0].pop_limit_milli = 10_000;
        let mut fuel = rules.goods[0].clone();
        fuel.id = "hardwood".into();
        fuel.volume_ml = 100_000;
        fuel.pop_limit_milli = 2_000;
        fuel.stacked_outside = true;
        rules.goods.push(fuel);
        let mut pop = reserve_test_pop(1);
        pop.stock = vec![1, 0];
        assert_eq!(receive_pop(&rules, &mut pop, 1, 3).unwrap(), 1);
        assert_eq!(receive_pop(&rules, &mut pop, 0, 1).unwrap(), 1);
        assert_eq!(volume_used(&rules, &pop.stock, None).unwrap(), 720_000);
        rules.goods[0].volume_ml = 800_000;
        let mut building = reserve_test_building(1, 0);
        building.stock = vec![1, 0];
        building.employment[0].workers = 8;
        assert_eq!(receive_building(&rules, &mut building, 1, 3).unwrap(), 1);
        assert_eq!(volume_used(&rules, &building.stock, None).unwrap(), 800_000);
    }

    #[test]
    fn pop_building_and_storehouse_receipts_obey_good_and_volume_limits() {
        let mut rules = tiny_rules();
        rules.goods[0].id = "oats".to_owned();
        rules.goods[0].volume_ml = 100_000;
        rules.goods[0].pop_limit_milli = 1000;
        let mut pop = PopState {
            kind: "peasant".to_owned(),
            culture: "ulaid".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 1,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        assert_eq!(receive_pop(&rules, &mut pop, 0, 4).unwrap(), 3);
        assert_eq!(pop.stock, [1]);

        rules.goods[0].pop_limit_milli = 1_000_000;
        let mut farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
            level_records: vec![LevelRecord {
                number: 0,
                method: 0,
                ground: Some("pasture".to_owned()),
                paydays: MATURE_PAYDAYS,
                unpaid_work: BTreeMap::new(),
                last_switch_year: None,
            }],
            herds: Vec::new(),
            employment: vec![Employment {
                owners: 0,
                workers: 1,
            }],
            stock: vec![0],
            storehouse_accounts: None,
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: Vec::new(),
            pay_carries: Vec::new(),
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        assert_eq!(receive_building(&rules, &mut farm, 0, 4).unwrap(), 3);
        assert_eq!(
            farm.stock,
            [1],
            "the 100 L volume cap binds before the good cap"
        );

        let mut storehouse = BuildingState {
            kind: "storehouse".to_owned(),
            levels: 1,
            method_levels: vec![1, 0, 0],
            level_records: Vec::new(),
            herds: Vec::new(),
            employment: vec![Employment::default()],
            stock: vec![0],
            storehouse_accounts: Some(StorehouseAccounts {
                pops: vec![vec![0]],
                communal: vec![0],
            }),
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: vec![0],
            pay_carries: vec![0],
            fee_carries: vec![vec![0], vec![0]],
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        assert_eq!(
            receive_storehouse(&rules, &mut storehouse, Some(0), 0, 8).unwrap(),
            0
        );
        assert_eq!(storehouse.stock, [8]);
        assert_eq!(
            storehouse.storehouse_accounts.as_ref().unwrap().pops[0],
            [8]
        );
    }

    #[test]
    fn holdings_receipt_uses_worker_capacity_and_overflows_one_unit_past_it() {
        let mut rules = tiny_rules();
        rules.goods[0].id = "oats".to_owned();
        rules.goods[0].volume_ml = 100_000;
        rules.goods[0].pop_limit_milli = 1_000_000;
        let mut holdings = BuildingState {
            kind: "holdings".to_owned(),
            levels: 1,
            method_levels: vec![1],
            level_records: Vec::new(),
            herds: Vec::new(),
            employment: vec![Employment {
                owners: 2,
                workers: 10,
            }],
            stock: vec![0],
            storehouse_accounts: None,
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: Vec::new(),
            pay_carries: Vec::new(),
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        assert_eq!(store_capacity_ml(&holdings).unwrap(), 11_700_000);
        assert!(building_good_cap(&rules, &holdings, 0).unwrap() >= 118);
        assert_eq!(receive_building(&rules, &mut holdings, 0, 118).unwrap(), 1);
        assert_eq!(
            holdings.stock,
            [117],
            "the 11,700 L worker capacity accepts 117 units and refuses the next"
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The overflow witness lists every account it moves in one literal flow."
    )]
    fn overflow_moves_from_building_through_alms_then_source_accounts() {
        let mut rules = tiny_rules();
        rules.goods[0].id = "oats".to_owned();
        rules.goods[0].volume_ml = 100_000;
        rules.goods[0].pop_limit_milli = 1_000_000;
        let pop = |kind: &str| PopState {
            kind: kind.to_owned(),
            culture: "ulaid".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 1,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![7],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let mut pops = vec![pop("peasant"), pop("labourer")];
        let farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
            level_records: vec![LevelRecord {
                number: 0,
                method: 0,
                ground: Some("pasture".to_owned()),
                paydays: MATURE_PAYDAYS,
                unpaid_work: BTreeMap::new(),
                last_switch_year: None,
            }],
            herds: Vec::new(),
            employment: vec![
                Employment {
                    owners: 0,
                    workers: 1,
                },
                Employment::default(),
            ],
            stock: vec![1],
            storehouse_accounts: None,
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: Vec::new(),
            pay_carries: Vec::new(),
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let storehouse = BuildingState {
            kind: "storehouse".to_owned(),
            levels: 1,
            method_levels: vec![1, 0, 0],
            level_records: Vec::new(),
            herds: Vec::new(),
            employment: vec![Employment::default()],
            stock: vec![0],
            storehouse_accounts: Some(StorehouseAccounts {
                pops: vec![vec![0], vec![0]],
                communal: vec![0],
            }),
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: vec![0],
            pay_carries: vec![0],
            fee_carries: vec![vec![0]; 3],
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let mut buildings = vec![farm, storehouse];
        let mut ledger = PlaceProductionLedger::new("fort", 1, 3);
        settle_overflow(
            &rules,
            &mut pops,
            &mut buildings,
            0,
            0,
            &[(Some(1), 2), (None, 2)],
            &mut CounterBuckets::default(),
            &mut ledger,
        )
        .unwrap();
        assert_eq!(pops[1].stock, [7], "full labourer stores receive no alms");
        let accounts = buildings[1].storehouse_accounts.as_ref().unwrap();
        assert_eq!(
            accounts.pops[1],
            [2],
            "wage overflow keeps its worker account"
        );
        assert_eq!(
            accounts.communal,
            [2],
            "overhead keeps its communal account"
        );
        assert_eq!(buildings[1].stock, [4]);
        assert_eq!(ledger.lost, [0]);
    }

    #[test]
    fn alms_order_puts_labourers_first_then_the_hungriest_pop() {
        let mut rules = tiny_rules();
        rules.goods[0].hunger_centi = 100;
        let pop = |kind: &str, food| PopState {
            kind: kind.to_owned(),
            culture: "ulaid".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 1,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![food],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let pops = vec![pop("peasant", 0), pop("monk", 10), pop("labourer", 100)];
        assert_eq!(alms_order(&rules, &pops, None).unwrap(), [2, 0, 1]);
    }

    fn tiny_start_data() -> ScenarioEconomy {
        let ordinary_place = || StartOverride {
            culture: Some("connachta".to_owned()),
            religion: Some("christianity".to_owned()),
            monk_religion: Some("christianity".to_owned()),
            ..StartOverride::default()
        };
        ScenarioEconomy {
            horses_always: Vec::new(),
            defaults: BTreeMap::from([(
                "ringfort".to_owned(),
                BTreeMap::from([(
                    "1".to_owned(),
                    StartDefaults {
                        peasants: 20,
                        monks: 0,
                        labourers: 0,
                        farm_levels: 1,
                        holdings_levels: 0,
                        storehouse_levels: 1,
                        pop_stock: BTreeMap::from([("food".to_owned(), 1)]),
                        building_stock: BTreeMap::from([("food".to_owned(), 2)]),
                    },
                )]),
            )]),
            places: ["fort", "island", "coast", "inland", "built", "farm"]
                .into_iter()
                .map(|id| (id.to_owned(), ordinary_place()))
                .collect(),
            nobles: BTreeMap::new(),
        }
    }

    fn tiny_storehouse_payday_state() -> (Rules, EconomyState) {
        let mut rules = tiny_rules();
        rules.seasons.insert(
            "food".to_owned(),
            [0, 1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(),
        );
        rules.seasons.insert(
            "herd_births".to_owned(),
            [0, 1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(),
        );
        let mut data = tiny_start_data();
        data.defaults
            .get_mut("ringfort")
            .unwrap()
            .get_mut("1")
            .unwrap()
            .building_stock
            .insert("food".to_owned(), 11);
        let location = StartLocation {
            id: "fort".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 8)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 8000)]),
                fertile_cells: 8,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        };
        let (state, _) = starting_state(&rules, &data, &[location], 10).unwrap();
        (rules, state)
    }

    #[test]
    fn target_price_uses_the_named_k_and_floors_negative_ratios() {
        assert_eq!(target_price(120, 8, 7, 1000), Ok(127_999));
        assert_eq!(target_price(120, 8, 7, 500), Ok(123_999));
        assert_eq!(target_price(120, 7, 8, 1000), Ok(111_999));
        assert_eq!(target_price(120, 0, 0, 500), Ok(120_000));
        assert_eq!(target_price(120, 1, 0, 10_000), Ok(600_000));
        assert_eq!(target_price(120, 0, 1, 10_000), Ok(24_000));
        assert_eq!(value_at_price(12, 1500), Ok(18_000));
        assert_eq!(
            value_at_price(u128::MAX, 2),
            Err("local price value overflow")
        );
    }

    #[test]
    fn comfort_order_targets_match_the_owner_signed_literal_vectors() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        rules.validate("economy rules").unwrap();
        let index = |id: &str| rules.goods.iter().position(|good| good.id == id).unwrap();
        let expectations = [("ale", 213), ("apples", 16), ("mead", 22), ("herbs", 11)];
        for (good, target) in expectations {
            assert_eq!(
                tier_target_units(&rules, "monk", 10, index(good), 1)
                    .unwrap()
                    .max(tier_target_units(&rules, "monk", 10, index(good), 2).unwrap()),
                target,
                "monk {good}"
            );
        }
        for (good, target) in [("ale", 172), ("honey", 18)] {
            assert_eq!(
                tier_target_units(&rules, "labourer", 10, index(good), 1).unwrap(),
                target,
                "labourer {good}"
            );
        }
        assert_eq!(
            tier_target_units(&rules, "peasant", 10, index("ale"), 1),
            Ok(0)
        );

        let mut invalid = rules.clone();
        invalid
            .comfort_profiles
            .get_mut("monk")
            .unwrap()
            .get_mut("niceties")
            .unwrap()
            .insert("ale".to_owned(), 951);
        assert_eq!(
            invalid.validate("economy rules").unwrap_err().entry,
            "comfort_profiles.monk.niceties"
        );
        let mut zero_value = rules;
        zero_value
            .comfort_profiles
            .get_mut("monk")
            .unwrap()
            .get_mut("niceties")
            .unwrap()
            .insert("ale".to_owned(), 707);
        zero_value
            .comfort_profiles
            .get_mut("monk")
            .unwrap()
            .get_mut("niceties")
            .unwrap()
            .insert("oats".to_owned(), 1);
        assert_eq!(
            zero_value.validate("economy rules").unwrap_err().entry,
            "comfort_profiles.monk.niceties.oats"
        );
        zero_value.comfort_profiles.remove("peasant");
        assert_eq!(
            zero_value.validate("economy rules").unwrap_err().entry,
            "comfort_profiles.peasant"
        );
    }

    #[test]
    fn luxury_dominant_comfort_want_uses_the_larger_tier_target() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        rules
            .pop_types
            .iter_mut()
            .find(|pop| pop.id == "monk")
            .unwrap()
            .luxuries_expectation_pm = 500;
        let mead = rules
            .goods
            .iter()
            .position(|good| good.id == "mead")
            .unwrap();
        let pop = PopState {
            kind: "monk".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 10,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![0; rules.goods.len()],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let place = PlaceState {
            id: "luxury_market".to_owned(),
            pops: vec![pop],
            buildings: Vec::new(),
            prices_milli: rules
                .goods
                .iter()
                .map(|good| good.base_price * 1000)
                .collect(),
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: vec![0; rules.goods.len()],
            last_market_orders: None,
            counters: CounterBuckets::default(),
        };
        assert_eq!(tier_target_units(&rules, "monk", 10, mead, 1), Ok(22));
        assert_eq!(tier_target_units(&rules, "monk", 10, mead, 2), Ok(86));
        assert_eq!(
            daily_market_orders(&rules, &place).unwrap().buy_units[mead],
            86
        );
    }

    #[test]
    fn comfort_goods_use_the_same_target_for_pop_buys_and_sells() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        let ale = rules
            .goods
            .iter()
            .position(|good| good.id == "ale")
            .unwrap();
        let oats = rules
            .goods
            .iter()
            .position(|good| good.id == "oats")
            .unwrap();
        rules.goods[ale].hunger_centi = 102;
        let monk_diet = rules.diet_profiles.get_mut("monk").unwrap();
        monk_diet.insert("oats".to_owned(), 329);
        monk_diet.insert("ale".to_owned(), 21);
        assert_eq!(food_target_units(&rules, "monk", 10, ale, 240), Ok(50));
        assert_eq!(
            tier_target_units(&rules, "monk", 10, ale, 1)
                .unwrap()
                .max(tier_target_units(&rules, "monk", 10, ale, 2).unwrap()),
            213
        );
        let oats_target = food_target_units(&rules, "monk", 10, oats, 240).unwrap();
        let place_with = |ale_holding| {
            let mut stock = vec![0; rules.goods.len()];
            stock[ale] = ale_holding;
            stock[oats] = u64::try_from(oats_target + 7).unwrap();
            PlaceState {
                id: "comfort_order".to_owned(),
                pops: vec![PopState {
                    kind: "monk".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
                    literacy_pm: 0,
                    workers: 10,
                    dependents: 0,
                    employment: Vec::new(),
                    stock,
                    consumption: PopConsumption::default(),
                    counters: CounterBuckets::default(),
                    short_on_rent: false,
                    housed_last: false,
                    genetics: PopGenetics::default(),
                    pending_births: 0,
                    pending_deaths: 0,
                }],
                buildings: Vec::new(),
                prices_milli: vec![0; rules.goods.len()],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: vec![0; rules.goods.len()],
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }
        };

        let below_target = daily_market_orders(&rules, &place_with(100)).unwrap();
        assert_eq!(below_target.buy_units[ale], 113);
        assert_eq!(below_target.sell_units[ale], 0);
        assert_eq!(
            below_target.sell_units[oats], 7,
            "food outside comfort still sells above its food target"
        );

        let above_target = daily_market_orders(&rules, &place_with(300)).unwrap();
        assert_eq!(above_target.buy_units[ale], 0);
        assert_eq!(above_target.sell_units[ale], 87);
    }

    fn reserve_test_pop(people: u64) -> PopState {
        PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: people,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        }
    }

    fn reserve_test_building(owners: u64, food_stock: u64) -> BuildingState {
        BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![0],
            level_records: Vec::new(),
            herds: Vec::new(),
            employment: vec![Employment { owners, workers: 0 }],
            stock: vec![food_stock],
            storehouse_accounts: None,
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: Vec::new(),
            pay_carries: Vec::new(),
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        }
    }

    fn reserve_test_place(pop: PopState, buildings: Vec<BuildingState>) -> PlaceState {
        PlaceState {
            id: "owner_reserve".to_owned(),
            pops: vec![pop],
            buildings,
            prices_milli: vec![0],
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: vec![0],
            last_market_orders: None,
            counters: CounterBuckets::default(),
        }
    }

    #[test]
    fn building_food_reserve_covers_whole_owner_pops_and_apportions_them_by_positions() {
        let rules = tiny_rules();

        let mut whole_pop = reserve_test_pop(10);
        whole_pop.dependents = 7;
        let whole_pop_orders = daily_market_orders(
            &rules,
            &reserve_test_place(whole_pop, vec![reserve_test_building(1, 1200)]),
        )
        .unwrap();
        assert_eq!(
            whole_pop_orders.sell_units[0], 180,
            "17 people reserve 1020 food units for 60 days"
        );

        let apportioned_orders = daily_market_orders(
            &rules,
            &reserve_test_place(
                reserve_test_pop(20),
                vec![
                    reserve_test_building(1, 500),
                    reserve_test_building(3, 1100),
                ],
            ),
        )
        .unwrap();
        assert_eq!(
            apportioned_orders.sell_units[0], 400,
            "owner shares [1, 3] apportion 20 people as [5, 15], reserving 300 and 900"
        );

        let non_divisible_and_unowned = daily_market_orders(
            &rules,
            &reserve_test_place(
                reserve_test_pop(3),
                vec![
                    reserve_test_building(1, 100),
                    reserve_test_building(1, 70),
                    reserve_test_building(0, 30),
                ],
            ),
        )
        .unwrap();
        assert_eq!(
            non_divisible_and_unowned.sell_units[0], 40,
            "[1, 1, 0] owner positions apportion 3 people as [2, 1, 0]; the ownerless building offers all 30"
        );
    }

    fn assert_account_order_targets(rules: &Rules, place: &PlaceState) {
        let index = |id: &str| rules.goods.iter().position(|good| good.id == id).unwrap();
        let orders = daily_market_orders(rules, place).unwrap();
        assert_eq!(
            orders.buy_units[index("apples")],
            36,
            "food target 52 wins over comfort target 16, less 16 held"
        );
        for good in ["ale", "mead", "herbs"] {
            assert_eq!(
                orders.buy_units[index(good)],
                0,
                "held {good} target is satisfied"
            );
        }
        assert_eq!(
            pop_market_holding(place, 0, index("apples")).unwrap(),
            16,
            "the pop account, rather than duplicate storehouse stock, supplies its holding"
        );
        assert_eq!(
            orders.sell_units[index("apples")],
            7,
            "the building sells its 20 apples above the owner's 13-unit 60-day draw; storehouse physical stock is not counted again"
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The fixture spells out one full pop state, which now carries the rent flags."
    )]
    fn pop_comfort_orders_subtract_pop_storehouse_accounts_once() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        rules
            .diet_profiles
            .get_mut("monk")
            .unwrap()
            .insert("oats".to_owned(), 250);
        rules
            .diet_profiles
            .get_mut("monk")
            .unwrap()
            .insert("apples".to_owned(), 100);
        let index = |id: &str| rules.goods.iter().position(|good| good.id == id).unwrap();
        let mut account = vec![0; rules.goods.len()];
        for (good, amount) in [("ale", 213), ("apples", 16), ("mead", 22), ("herbs", 11)] {
            account[index(good)] = amount;
        }
        let communal = vec![0; rules.goods.len()];
        let accounts = StorehouseAccounts {
            pops: vec![account.clone()],
            communal,
        };
        let physical = storehouse_physical_stock(&rules, &accounts);
        let pop = PopState {
            kind: "monk".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 10,
            dependents: 0,
            employment: vec![Employment::default(); 2],
            stock: vec![0; rules.goods.len()],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let storehouse = BuildingState {
            kind: "storehouse".to_owned(),
            levels: 1,
            method_levels: vec![1, 0, 0],
            level_records: Vec::new(),
            herds: Vec::new(),
            employment: vec![Employment::default()],
            stock: physical,
            storehouse_accounts: Some(accounts),
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: vec![0; rules.goods.len()],
            pay_carries: vec![0; rules.goods.len()],
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let mut farm_stock = vec![0; rules.goods.len()];
        farm_stock[index("apples")] = 20;
        let farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
            level_records: vec![LevelRecord {
                number: 0,
                method: 0,
                ground: Some("pasture".to_owned()),
                paydays: MATURE_PAYDAYS,
                unpaid_work: BTreeMap::new(),
                last_switch_year: None,
            }],
            herds: vec![0; 3],
            employment: vec![Employment {
                owners: 10,
                workers: 0,
            }],
            stock: farm_stock,
            storehouse_accounts: None,
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: vec![0; rules.goods.len()],
            pay_carries: vec![0; rules.goods.len()],
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let place = PlaceState {
            id: "market_fixture".to_owned(),
            pops: vec![pop],
            buildings: vec![storehouse, farm],
            prices_milli: rules
                .goods
                .iter()
                .map(|good| good.base_price * 1000)
                .collect(),
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: vec![0; rules.goods.len()],
            last_market_orders: None,
            counters: CounterBuckets::default(),
        };
        assert_account_order_targets(&rules, &place);
    }

    #[test]
    fn a_balanced_market_holds_base_price_and_drift_keeps_its_fractional_carry() {
        let rules = tiny_rules();
        let target = food_target_units(&rules, "peasant", 1, 0, 240).unwrap();
        assert_eq!(target, 240);
        let place = PlaceState {
            id: "balanced".to_owned(),
            pops: vec![PopState {
                kind: "peasant".to_owned(),
                culture: "connachta".to_owned(),
                religion: "christianity".to_owned(),
                literacy_pm: 0,
                workers: 1,
                dependents: 0,
                employment: Vec::new(),
                stock: vec![u64::try_from(target).unwrap()],
                consumption: PopConsumption {
                    pantry_hunger_centi: vec![0],
                    ticking_carries: vec![0],
                    ..PopConsumption::default()
                },
                counters: CounterBuckets::default(),
                short_on_rent: false,
                housed_last: false,
                genetics: PopGenetics::default(),
                pending_births: 0,
                pending_deaths: 0,
            }],
            buildings: Vec::new(),
            prices_milli: vec![100_000],
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: vec![0],
            last_market_orders: None,
            counters: CounterBuckets::default(),
        };
        let mut state = EconomyState {
            places: vec![place],
        };
        assert_eq!(
            daily_market_orders(&rules, &state.places[0]).unwrap(),
            DailyMarketOrders {
                buy_units: vec![0],
                sell_units: vec![0],
            }
        );
        update_daily_prices(&rules, &mut state).unwrap();
        assert_eq!(state.places[0].prices_milli, [100_000]);
        assert_eq!(
            state.places[0].last_market_orders,
            Some(DailyMarketOrders {
                buy_units: vec![0],
                sell_units: vec![0],
            }),
            "the cache keeps the exact orders used by the price step"
        );

        state.places[0].pops[0].stock[0] = 0;
        state.places[0].prices_milli[0] = 100_001;
        let orders_used = daily_market_orders(&rules, &state.places[0]).unwrap();
        update_daily_prices(&rules, &mut state).unwrap();
        assert_eq!(state.places[0].prices_milli, [105_000]);
        assert_eq!(state.places[0].last_market_orders, Some(orders_used));
        assert_eq!(state.places[0].price_drift_carries, [900]);
        update_daily_prices(&rules, &mut state).unwrap();
        assert_eq!(state.places[0].prices_milli, [109_500]);
        assert_eq!(state.places[0].price_drift_carries, [900]);
    }

    #[test]
    fn shipped_cultures_resolve_in_registry() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let rules = Rules::load(root.join("game/economy.toml")).unwrap();
        let starts =
            ScenarioEconomy::load(root.join("scenarios/ireland-768/economy.toml")).unwrap();
        assert_eq!(
            starts.places["mayo"].monk_culture.as_deref(),
            Some("northumbrian")
        );
        for place in starts.places.values() {
            for id in [place.culture.as_deref(), place.monk_culture.as_deref()]
                .into_iter()
                .flatten()
            {
                assert!(rules.peoples.culture(id).is_some(), "unknown culture {id}");
            }
            for id in place
                .peasant_cultures_pm
                .iter()
                .flat_map(|shares| shares.keys())
            {
                assert!(rules.peoples.culture(id).is_some(), "unknown culture {id}");
            }
        }
    }

    #[test]
    fn shipped_economy_and_ireland_start_data_are_strictly_valid() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let rules = Rules::load(root.join("game/economy.toml")).unwrap();
        let starts =
            ScenarioEconomy::load(root.join("scenarios/ireland-768/economy.toml")).unwrap();
        assert_eq!(starts.places.len(), 123);
        assert!(starts.places.values().all(|place| {
            place.culture.is_some()
                && place.religion.as_deref() == Some("christianity")
                && place.monk_religion.as_deref() == Some("christianity")
        }));
        assert_eq!(rules.goods.len(), 27);
        assert_eq!(rules.pop_types.len(), 4);
        assert_eq!(
            rules.curves.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "births_crowding",
                "births_food",
                "births_literacy",
                "births_satisfaction",
                "deaths_comfort",
                "deaths_crowding",
                "deaths_hunger",
                "intake_food_days",
            ]
        );
        assert_eq!(
            rules.curves["births_food"],
            [[300, 0], [500, 200], [800, 700], [1000, 1000]]
        );
        assert_eq!(
            rules.curves["births_crowding"],
            [[0, 1100], [1000, 1000], [1500, 700], [2000, 400]]
        );
        assert_eq!(
            rules.curves["births_satisfaction"],
            [[-100, 850], [0, 1000], [100, 1100]]
        );
        assert_eq!(rules.curves["births_literacy"], [[0, 1000], [1000, 1000]]);
        assert_eq!(
            rules.curves["deaths_hunger"],
            [[0, 100_000], [300, 5000], [500, 1200], [800, 1000]]
        );
        assert_eq!(rules.curves["deaths_comfort"], [[0, 1000], [1000, 900]]);
        assert_eq!(
            rules.curves["deaths_crowding"],
            [[0, 900], [1000, 1000], [1500, 1300], [2000, 1800]]
        );
        assert_eq!(
            rules.seasons["vegetables"],
            [0, 0, 0, 0, 0, 0, 250, 250, 250, 250, 0, 0]
        );
        assert_eq!(
            rules.seasons["herd_births"],
            [34, 34, 233, 234, 233, 34, 33, 33, 33, 33, 33, 33]
        );
        assert_eq!(rules.seasons["mead"], rules.seasons["ale"]);
        assert_eq!(rules.seasons["beeswax"], rules.seasons["honey"]);
        assert_eq!(
            starts.places["clonmacnoise"].culture.as_deref(),
            Some("mide_brega")
        );
        assert_eq!(
            starts.places["clonmacnoise"].peasant_cultures_pm,
            Some(BTreeMap::from([
                ("connachta".to_owned(), 500),
                ("mide_brega".to_owned(), 500),
            ]))
        );
        assert_eq!(starts.places["mayo"].culture.as_deref(), Some("connachta"));
        assert_eq!(
            starts.places["mayo"].monk_culture.as_deref(),
            Some("northumbrian")
        );
        assert_eq!(starts.places["skellig_michael"].levels, Some([0, 2, 1]));
        assert_eq!(starts.places["skellig_michael"].peasants, Some(0));
        // CK-370: twelve monks (an abbot and the traditional twelve), so that its slot of eleven fits with a head spare.
        assert_eq!(starts.places["skellig_michael"].monks, Some(12));
        assert_eq!(starts.places["skellig_michael"].labourers, Some(10));
        // CK-370: Clogher gains a small church beside the royal hillfort: ten monks and ten labourers, levels farm 30, holdings 1, storehouse 2.
        assert_eq!(starts.places["clogher"].monks, Some(10));
        assert_eq!(starts.places["clogher"].labourers, Some(10));
        assert_eq!(starts.places["clogher"].levels, Some([30, 1, 2]));
        assert_eq!(starts.places["inisfallen"].levels, Some([0, 2, 1]));
        assert_eq!(starts.places["inisfallen"].peasants, Some(0));
        assert_eq!(starts.places["inishmurray"].levels, Some([3, 2, 1]));
        assert_eq!(starts.places["inishmurray"].peasants, Some(50));
        assert_eq!(starts.places["inishmurray"].monks, Some(10));
        assert_eq!(starts.places["inishmurray"].labourers, Some(10));
        for (id, share) in [("killala", 150), ("cong", 150), ("tory", 100)] {
            assert_eq!(starts.places[id].pagan_share_pm, Some(share));
        }
    }

    #[test]
    fn good_tags_are_optional_ordered_and_validated_as_open_lowercase_keys() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml");
        let source = fs::read_to_string(path).unwrap();
        let tagged_source = source.replacen("tags=[\"food\"]\n", "tags=[\"food\", \"grain\"]\n", 1);
        let rules: Rules = toml::from_str(&tagged_source).unwrap();
        rules.validate("fixture/economy.toml").unwrap();
        assert_eq!(rules.goods[0].tags, ["food", "grain"]);
        assert!(
            rules
                .goods
                .iter()
                .find(|good| good.id == "herbs")
                .unwrap()
                .tags
                .is_empty()
        );

        for (tags, tag, problem) in [
            (
                "[\"Food\"]",
                "Food",
                "tag \"Food\" is not a lower-case key of at most 24 characters",
            ),
            (
                "[\"9lives\"]",
                "9lives",
                "tag \"9lives\" is not a lower-case key of at most 24 characters",
            ),
            (
                "[\"\"]",
                "",
                "tag \"\" is not a lower-case key of at most 24 characters",
            ),
            (
                "[\"abcdefghijklmnopqrstuvwxy\"]",
                "abcdefghijklmnopqrstuvwxy",
                "tag \"abcdefghijklmnopqrstuvwxy\" is not a lower-case key of at most 24 characters",
            ),
            (
                "[\"food\", \"food\"]",
                "food",
                "tag \"food\" is listed twice",
            ),
        ] {
            let invalid_source = source.replacen("tags=[\"food\"]\n", &format!("tags={tags}\n"), 1);
            let invalid: Rules = toml::from_str(&invalid_source).unwrap();
            let error = invalid.validate("fixture/economy.toml").unwrap_err();
            assert_eq!(error.entry, "goods.oats.tags");
            assert_eq!(error.problem, problem, "invalid tag {tag:?}");
        }
    }

    #[test]
    fn fabric_categories_choose_a_whole_affordable_substitute() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml");
        let rules = Rules::load(path).unwrap();
        let hide = rules.goods.iter().find(|g| g.id == "hides").unwrap();
        assert_eq!(hide.units_for_category("fabric", 10), Some(2));
        assert_eq!(hide.units_for_category("food", 10), None);
        assert_eq!(hide.units_for_category("clothing", 10), Some(10));
        let hemp = rules.goods.iter().find(|g| g.id == "hemp").unwrap();
        assert_eq!(hemp.units_for_category("fabric", 10), Some(10));
        let herbs = rules.goods.iter().find(|g| g.id == "herbs").unwrap();
        assert_eq!(herbs.units_for_category("fabric", 10), None);
        let prices = BTreeMap::from([
            ("linen".to_owned(), 120),
            ("hemp".to_owned(), 30),
            ("hides".to_owned(), 200),
        ]);
        assert_eq!(
            rules
                .choose_category("fabric", 10, &prices, None)
                .unwrap()
                .0
                .id,
            "hemp"
        );
        let mut prices = prices;
        prices.insert("hemp".to_owned(), 40);
        assert_eq!(
            rules
                .choose_category("fabric", 10, &prices, None)
                .unwrap()
                .0
                .id,
            "hides"
        );
        let stock = BTreeMap::from([("hemp".to_owned(), 4), ("hides".to_owned(), 2)]);
        assert_eq!(
            rules
                .choose_category("fabric", 10, &prices, Some(&stock))
                .unwrap()
                .0
                .id,
            "hides"
        );
        let stock = BTreeMap::from([("hemp".to_owned(), 4), ("hides".to_owned(), 1)]);
        assert!(
            rules
                .choose_category("fabric", 10, &prices, Some(&stock))
                .is_none()
        );
    }

    #[test]
    fn supplies_must_be_positive_and_belong_to_a_tag() {
        let mut rules = tiny_rules();
        rules.goods[0].supplies.insert("fabric".to_owned(), 0);
        let error = rules.validate("fixture").unwrap_err();
        assert_eq!(error.entry, "goods.food.supplies.fabric");
        assert!(error.problem.contains("not among"));
        rules.goods[0].tags.push("fabric".to_owned());
        let error = rules.validate("fixture").unwrap_err();
        assert_eq!(error.entry, "goods.food.supplies.fabric");
        assert!(error.problem.contains("positive"));
    }

    #[test]
    fn linen_and_hemp_are_fixed_seasonal_farm_outputs() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml");
        let mut rules = Rules::load(path).unwrap();
        flatten_pasture(&mut rules);
        let index = |id: &str| rules.goods.iter().position(|g| g.id == id).unwrap();
        for (id, volume, price, unit) in [
            ("linen", 2000, 120, "pound of cloth"),
            ("hemp", 4000, 30, "pound of fibre"),
        ] {
            let good = &rules.goods[index(id)];
            assert_eq!(
                (
                    good.unit.as_str(),
                    good.weight_g,
                    good.volume_ml,
                    good.base_price,
                    good.pop_limit_milli
                ),
                (unit, 450, volume, price, 5000)
            );
            assert_eq!(
                (
                    good.hunger_centi,
                    good.necessities_centi,
                    good.niceties_centi,
                    good.luxuries_centi
                ),
                (0, 0, 0, 0)
            );
            assert_eq!(
                rules.seasons[id].iter().map(|v| u64::from(*v)).sum::<u64>(),
                1000
            );
            for building in &rules.buildings {
                for method in &building.methods {
                    let expected = building.id == "farm" && method.id == "mixed_farming";
                    assert_eq!(
                        method.annual_output.get(id).copied(),
                        expected.then_some(10),
                        "{}.{} {id}",
                        building.id,
                        method.id
                    );
                    if expected {
                        assert_eq!(method.score_share_pm[id], 5);
                        assert_eq!(
                            method
                                .score_share_pm
                                .values()
                                .map(|v| u64::from(*v))
                                .sum::<u64>(),
                            1000
                        );
                    }
                }
            }
        }
        for workers in [10, 5] {
            let mut state = shipped_production_fixture(&rules);
            state.places[0].buildings[0].employment[0].workers = workers;
            state.places[0].pops[0].employment[0].workers = workers;
            for month in 1..=12 {
                let ledger = run_monthly_production(
                    &rules,
                    &mut state,
                    Date {
                        year: 768,
                        month,
                        day: 1,
                        hour: 12,
                    },
                )
                .unwrap();
                let expected_hemp = if month == 9 { workers } else { 0 };
                let expected_linen = match (workers, month) {
                    (10, 10 | 11) => 5,
                    (5, 10) => 2,
                    (5, 11) => 3,
                    _ => 0,
                };
                assert_eq!(
                    ledger.produced[index("hemp")],
                    expected_hemp,
                    "month {month}, staffing {workers}"
                );
                assert_eq!(
                    ledger.produced[index("linen")],
                    expected_linen,
                    "month {month}, staffing {workers}"
                );
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "The daily trace keeps the shared fixture and isolated per-building witness together."
    )]
    fn herd_daily_trace(fed: bool) -> (String, [[u64; 6]; 3]) {
        use std::fmt::Write as _;

        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        assert_eq!(rules.herd_species.len(), 4);
        assert_eq!(rules.herd_species[3].animal, "horses");
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.prices_milli = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        place.price_drift_carries = vec![0; rules.goods.len()];
        let oats = rules
            .goods
            .iter()
            .position(|good| good.id == "oats")
            .unwrap();
        let wool = rules
            .goods
            .iter()
            .position(|good| good.id == "wool")
            .unwrap();
        let dairy = rules
            .goods
            .iter()
            .position(|good| good.id == "dairy")
            .unwrap();
        for pop in &mut place.pops {
            if fed {
                pop.stock[oats] = 1_000_000;
            }
            pop.workers = 100;
            pop.consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
            pop.consumption.ticking_carries = vec![0; rules.goods.len()];
        }
        place.buildings[0].herd_product_carries = vec![0; 2];
        let farm = place.buildings[0].clone();
        let holdings = &mut place.buildings[1];
        holdings.method_levels = vec![1, 0];
        sync_levels(&rules, holdings);
        holdings.herds = vec![9, 6, 5, 0];
        holdings.herd_birth_carries = vec![0; 4];
        holdings.herd_death_carries = vec![0; 4];
        holdings.herd_product_carries = vec![0; 2];
        holdings.herd_grazing_sums = vec![1000, 400, 1000, 1000];
        let mut demesne = farm;
        demesne.kind = "demesne".to_owned();
        demesne.method_levels = vec![1];
        sync_levels(&rules, &mut demesne);
        demesne.production_carries.truncate(1);
        demesne.method_suitability.truncate(1);
        place.buildings.push(demesne);
        for (index, pop) in place.pops.iter_mut().enumerate() {
            pop.employment
                .push(place.buildings[3].employment[index].clone());
        }
        let mut trace = String::new();
        let mut exercise = [[0_u64; 6]; 3];
        for year in 768..=769 {
            for month in 1..=12 {
                for day in 1..=days_in_month(year, month).unwrap() {
                    let date = Date {
                        year,
                        month,
                        day,
                        hour: 12,
                    };
                    let mut wage_units = Vec::new();
                    if day == 1 {
                        for (slot, index) in [0, 1, 3].into_iter().enumerate() {
                            let mut isolated = state.clone();
                            for (other, building) in
                                isolated.places[0].buildings.iter_mut().enumerate()
                            {
                                if other != index {
                                    building.method_levels.fill(0);
                                    building.level_records.clear();
                                    building.herds.clear();
                                    building.herd_birth_carries.clear();
                                    building.herd_death_carries.clear();
                                    building.herd_grazing_sums.clear();
                                }
                            }
                            let isolated_ledger =
                                run_monthly_production(&rules, &mut isolated, date).unwrap();
                            exercise[slot][0] += isolated_ledger.herd_births.iter().sum::<u64>();
                            exercise[slot][1] += isolated_ledger.herd_deaths.iter().sum::<u64>();
                            exercise[slot][2] += isolated_ledger.herd_slaughter.iter().sum::<u64>();
                            exercise[slot][3] += isolated_ledger.produced[dairy];
                            if month == 6 {
                                exercise[slot][4 + usize::from(year - 768)] +=
                                    isolated_ledger.produced[wool];
                            }
                            let output = isolated_ledger.produced;
                            let building = &state.places[0].buildings[index];
                            let definition = rules
                                .buildings
                                .iter()
                                .find(|b| b.id == building.kind)
                                .unwrap();
                            let units: Vec<_> = output
                                .iter()
                                .enumerate()
                                .map(|(good, amount)| {
                                    if building.employment.iter().all(|e| e.workers == 0) {
                                        0
                                    } else {
                                        (amount * u64::from(1000 - definition.overhead_pm) * 1000
                                            + building.wage_carries[good])
                                            / 1_000_000
                                            + building.pay_carries[good]
                                    }
                                })
                                .collect();
                            wage_units.push(units);
                        }
                    }
                    let ledger = run_daily_economy(&rules, &mut state, date).unwrap();
                    let place = &state.places[0];
                    // The accepted literals predate horses and fuel goods. Project the added species and goods
                    // from the output, while the full shipped rules and state drive the simulation.
                    let goods = rules.goods.len() - 3;
                    assert_eq!(
                        rules.goods[goods..]
                            .iter()
                            .map(|g| g.id.as_str())
                            .collect::<Vec<_>>(),
                        ["hardwood", "peat", "horses"]
                    );
                    let herds: Vec<_> = place
                        .buildings
                        .iter()
                        .map(|b| &b.herds[..b.herds.len().min(3)])
                        .collect();
                    let stocks: Vec<_> =
                        place.buildings.iter().map(|b| &b.stock[..goods]).collect();
                    let pop_stocks: Vec<_> = place.pops.iter().map(|p| &p.stock[..goods]).collect();
                    let wages: Vec<_> = place
                        .buildings
                        .iter()
                        .map(|b| {
                            (
                                &b.wage_carries[..goods],
                                &b.pay_carries[..goods],
                                &b.wage_history,
                            )
                        })
                        .collect();
                    let wage_units: Vec<_> = wage_units.iter().map(|w| &w[..goods]).collect();
                    let mut consumption = ledger.daily_consumption.clone();
                    if let Some(d) = &mut consumption {
                        for v in [
                            &mut d.stock_before,
                            &mut d.butchered,
                            &mut d.opened,
                            &mut d.ticked,
                            &mut d.lost,
                            &mut d.stock_after,
                        ] {
                            v.truncate(goods);
                        }
                        for v in [
                            &mut d.herds_before,
                            &mut d.herd_slaughter,
                            &mut d.herds_after,
                        ] {
                            v.truncate(3);
                        }
                    }
                    writeln!(trace, "{year}-{month:02}-{day:02} {herds:?} {:?} {:?} {:?} {:?} {stocks:?} {pop_stocks:?} {wages:?} {wage_units:?} {:?} {:?}", &ledger.herd_births[..3], &ledger.herd_deaths[..3], &ledger.herd_slaughter[..3], &ledger.produced[..goods], &place.prices_milli[..goods], consumption).unwrap();
                }
            }
        }
        (trace, exercise)
    }

    #[test]
    fn shipped_herd_daily_accepted_witness() {
        let (trace, exercise) = herd_daily_trace(true);
        for (index, counts) in exercise.iter().enumerate() {
            assert!(
                counts.iter().all(|count| *count > 0),
                "building {index} must exercise births, deaths, culling, dairy and both Junes: {counts:?}"
            );
        }
        assert_eq!(trace, include_str!("../tests/herd_daily_accepted.txt"));
    }

    #[test]
    fn shipped_herd_starvation_accepted_witness() {
        let (trace, _) = herd_daily_trace(false);
        assert_eq!(trace, include_str!("../tests/herd_starvation_accepted.txt"));
    }

    #[test]
    fn category_labels_do_not_change_production() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml");
        let mut rules = Rules::load(path).unwrap();
        flatten_pasture(&mut rules);
        let mut unlabelled = rules.clone();
        for good in &mut unlabelled.goods {
            good.tags.retain(|tag| tag != "fabric");
            good.supplies.remove("fabric");
        }
        let mut left = shipped_production_fixture(&rules);
        left.places[0].price_drift_carries = vec![0; rules.goods.len()];
        left.places[0].prices_milli = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        for pop in &mut left.places[0].pops {
            pop.workers = 100;
            pop.consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
            pop.consumption.ticking_carries = vec![0; rules.goods.len()];
        }
        let mut right = left.clone();
        for month in 1..=12 {
            let date = Date {
                year: 768,
                month,
                day: 1,
                hour: 12,
            };
            for day in 1..=days_in_month(768, month).unwrap() {
                let date = Date { day, ..date };
                run_daily_economy(&rules, &mut left, date).unwrap();
                run_daily_economy(&unlabelled, &mut right, date).unwrap();
            }
        }
        assert_eq!(left, right);
    }

    #[test]
    fn shipped_goods_have_the_owner_approved_tags() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml");
        let rules = Rules::load(path).unwrap();
        let expected: BTreeMap<&str, &[&str]> = BTreeMap::from([
            ("oats", &["food"][..]),
            ("barley", &["food"][..]),
            ("peas_beans", &["food"][..]),
            ("vegetables", &["food"][..]),
            ("apples", &["food"][..]),
            ("dairy", &["food"][..]),
            ("beef", &["food"][..]),
            ("pork", &["food"][..]),
            ("mutton", &["food"][..]),
            ("fish", &["food"][..]),
            ("seabirds", &["food"][..]),
            ("ale", &["food"][..]),
            ("mead", &["food"][..]),
            ("wine", &["food"][..]),
            ("honey", &["food"][..]),
            ("wool", &["clothing"][..]),
            ("hides", &["clothing", "fabric"][..]),
            ("linen", &["clothing", "fabric"][..]),
            ("hemp", &["fabric"][..]),
            ("hardwood", &["wood", "fuel"][..]),
            ("peat", &["fuel"][..]),
            ("cattle", &["herd"][..]),
            ("pigs", &["herd"][..]),
            ("sheep", &["herd"][..]),
            ("horses", &["herd"][..]),
        ]);
        for good in &rules.goods {
            let expected_tags = expected.get(good.id.as_str()).copied().unwrap_or_default();
            assert_eq!(
                good.tags.iter().map(String::as_str).collect::<Vec<_>>(),
                expected_tags,
                "goods.{}.tags",
                good.id
            );
        }
    }

    #[test]
    fn shipped_ticking_rate_matrix_matches_owner_signoff() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let rules = Rules::load(root.join("game/economy.toml")).unwrap();
        assert_eq!(
            rules.ticking_rates_million["herbs"],
            BTreeMap::from([
                ("monk".to_owned(), 6_000),
                ("labourer".to_owned(), 2_000),
                ("noble".to_owned(), 2_000),
                ("peasant".to_owned(), 1_000),
            ])
        );
        assert_eq!(
            rules.ticking_rates_million["beeswax"],
            BTreeMap::from([
                ("monk".to_owned(), 4_000),
                ("labourer".to_owned(), 500),
                ("noble".to_owned(), 2_000),
                ("peasant".to_owned(), 250),
            ])
        );
        let beeswax = rules
            .goods
            .iter()
            .find(|good| good.id == "beeswax")
            .unwrap();
        assert_eq!(beeswax.niceties_centi, 500);
        assert!(!rules.ticking_rates_million.contains_key("wool"));
    }

    #[test]
    fn shipped_starting_needs_match_owner_ruling() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let rules = Rules::load(root.join("game/economy.toml")).unwrap();
        for kind in ["peasant", "labourer", "monk"] {
            assert_eq!(rules.starting_needs_pm[kind], [1000, 0, 0], "{kind}");
        }
        assert_eq!(rules.starting_needs_pm["noble"], [1000, 700, 300]);
    }

    #[test]
    fn shipped_holdings_alone_are_marked_as_church_buildings() {
        let rules = shipped_rules();
        let church = rules
            .buildings
            .iter()
            .filter(|building| building.church)
            .map(|building| building.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(church, ["holdings"]);
    }

    #[test]
    fn shipped_building_recruitment_ranges_match_slice_contract() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let rules = Rules::load(root.join("game/economy.toml")).unwrap();
        let range = |id: &str| {
            rules
                .buildings
                .iter()
                .find(|building| building.id == id)
                .unwrap()
                .recruit_range_m
        };
        assert_eq!(range("farm"), Some(30_000));
        assert_eq!(range("holdings"), Some(100_000));
        assert_eq!(range("storehouse"), Some(0));
    }

    #[test]
    fn validation_names_the_file_entry_and_each_invalid_value() {
        let mut rules = tiny_rules();
        rules
            .diet_profiles
            .get_mut("peasant")
            .unwrap()
            .insert("food".to_owned(), 999);
        let error = rules.validate("fixture/economy.toml").unwrap_err();
        assert_eq!(error.file, "fixture/economy.toml");
        assert_eq!(error.entry, "diet_profiles.peasant");
        assert!(error.problem.contains("sum to 1000"));

        let mut rules = tiny_rules();
        rules
            .suitability
            .get_mut("pasture")
            .unwrap()
            .insert("food".to_owned(), 1001);
        let error = rules.validate("fixture/economy.toml").unwrap_err();
        assert_eq!(error.entry, "suitability.pasture.food");
        assert!(error.problem.contains("0..=1000"));

        let mut rules = tiny_rules();
        rules.buildings[0].methods[0]
            .annual_output
            .insert("missing".to_owned(), 1);
        let error = rules.validate("fixture/economy.toml").unwrap_err();
        assert_eq!(
            error.entry,
            "buildings[0].methods.mixed.annual_output.missing"
        );
        assert_eq!(error.problem, "unknown good");

        let mut rules = tiny_rules();
        rules.buildings[0].recruit_range_m = None;
        let error = rules.validate("fixture/economy.toml").unwrap_err();
        assert_eq!(error.entry, "buildings.farm.recruit_range_m");
        assert!(error.problem.contains("missing"));
    }

    #[test]
    fn starting_needs_are_required_and_validated_by_population_type() {
        let mut rules = tiny_rules();
        rules.starting_needs_pm.remove("peasant");
        let error = rules.validate("fixture/economy.toml").unwrap_err();
        assert_eq!(error.entry, "starting_needs_pm.peasant");
        assert!(error.problem.contains("missing"));

        let mut rules = tiny_rules();
        rules
            .starting_needs_pm
            .insert("peasant".to_owned(), [1001, 0, 0]);
        let error = rules.validate("fixture/economy.toml").unwrap_err();
        assert_eq!(error.entry, "starting_needs_pm.peasant");
        assert!(error.problem.contains("0..=1000"));
    }

    #[test]
    fn noon_sheds_stock_after_a_person_is_already_gone() {
        let (rules, mut state) = daily_accounting_fixture();
        let hides = rules.goods.iter().position(|g| g.id == "hides").unwrap();
        let place = &mut state.places[0];
        place.buildings.clear();
        for pop in &mut place.pops {
            pop.employment.clear();
            pop.stock.fill(0);
            pop.stock[hides] = 1;
        }
        // The first pop had two people's allowance before one left.
        place.pops[0].stock[hides] = 2;
        let ledger = run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(state.places[0].pops[0].stock[hides], 1);
        assert_eq!(state.places[0].pops[1].stock[hides], 1);
        assert_eq!(ledger.daily_consumption.unwrap().lost[hides], 1);
        assert_eq!(state.places[0].pops[0].counters.current.losses, 1);
    }

    #[test]
    fn noon_shedding_gives_the_earlier_pop_the_last_building_room() {
        let (mut rules, mut state) = daily_accounting_fixture();
        let hides = rules.goods.iter().position(|g| g.id == "hides").unwrap();
        rules.goods[hides].pop_limit_milli = 450_000;
        let place = &mut state.places[0];
        place.buildings.remove(0);
        let building = &mut place.buildings[0];
        building.herds.fill(0);
        building.stock[hides] = 59;
        building.employment[0] = Employment {
            owners: 1,
            workers: 1,
        };
        building.employment[1] = Employment {
            owners: 1,
            workers: 0,
        };
        for pop in &mut place.pops {
            pop.employment = vec![Employment::default()];
            pop.stock.fill(0);
            pop.stock[hides] = 451;
        }
        let ledger = run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        let place = &state.places[0];
        assert_eq!(place.buildings[0].stock[hides], 60);
        assert_eq!(place.pops[0].counters.current.losses, 0);
        assert_eq!(place.pops[1].counters.current.losses, 1);
        assert_eq!(ledger.daily_consumption.unwrap().lost[hides], 1);
    }

    #[test]
    fn noon_sheds_per_good_limits_before_cheapest_volume() {
        let (mut rules, mut state) = daily_accounting_fixture();
        let cheap = rules.goods.iter().position(|g| g.id == "hides").unwrap();
        let dear = rules.goods.iter().position(|g| g.id == "beeswax").unwrap();
        rules.goods[cheap].volume_ml = 720_000;
        rules.goods[cheap].pop_limit_milli = 2_000;
        rules.goods[dear].volume_ml = 720_000;
        rules.goods[dear].pop_limit_milli = 1_000;
        rules.goods[dear].base_price = 10;
        rules
            .ticking_rates_million
            .get_mut("beeswax")
            .unwrap()
            .insert("peasant".to_owned(), 0);
        let place = &mut state.places[0];
        place.buildings.clear();
        place.pops.truncate(1);
        place.pops[0].employment.clear();
        place.pops[0].stock.fill(0);
        place.pops[0].stock[cheap] = 1;
        place.pops[0].stock[dear] = 2;
        place.prices_milli[cheap] = 1_000;
        place.prices_milli[dear] = 10_000;
        let mut per_good_rules = rules.clone();
        per_good_rules.goods[cheap].volume_ml = 360_000;
        per_good_rules.goods[dear].volume_ml = 360_000;
        let mut per_good_state = state.clone();
        let mut outside_rules = rules.clone();
        outside_rules.goods[cheap].stacked_outside = true;
        outside_rules.goods[cheap].pop_limit_milli = 1_000;
        outside_rules.goods[dear].pop_limit_milli = 2_000;
        let mut outside_state = state.clone();
        outside_state.places[0].pops[0].stock[cheap] = 2;
        let outside = run_daily_economy(
            &outside_rules,
            &mut outside_state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(outside_state.places[0].pops[0].stock[cheap], 1);
        assert_eq!(outside_state.places[0].pops[0].stock[dear], 1);
        let outside_lost = outside.daily_consumption.unwrap().lost;
        assert_eq!((outside_lost[cheap], outside_lost[dear]), (1, 1));
        let first = run_daily_economy(
            &per_good_rules,
            &mut per_good_state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(per_good_state.places[0].pops[0].stock[cheap], 1);
        assert_eq!(per_good_state.places[0].pops[0].stock[dear], 1);
        assert_eq!(first.daily_consumption.unwrap().lost[dear], 1);
        let ledger = run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!((pop.stock[cheap], pop.stock[dear]), (0, 1));
        let lost = ledger.daily_consumption.unwrap().lost;
        assert_eq!((lost[cheap], lost[dear]), (1, 1));
    }

    #[test]
    fn daily_ledger_conserves_drawing_butchering_opening_ticking_and_benefits() {
        let (rules, mut state) = daily_accounting_fixture();
        let system_ledger = run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(system_ledger.intake_pm, [vec![500, 500]]);
        let ledger = system_ledger.daily_consumption.unwrap();
        assert_eq!(ledger.hunger_eaten_centi, 100, "each pop eats T=50");
        assert_eq!(
            ledger.butchered[rules.goods.iter().position(|g| g.id == "pork").unwrap()],
            88
        );
        assert_eq!(
            ledger.lost[rules.goods.iter().position(|g| g.id == "pork").unwrap()],
            86
        );
        assert_eq!(state.places[0].counters.current.losses, 86);
        assert_eq!(state.places[0].pops[1].counters.current.losses, 86);
        let pork = rules
            .goods
            .iter()
            .position(|good| good.id == "pork")
            .unwrap();
        let herbs = rules
            .goods
            .iter()
            .position(|good| good.id == "herbs")
            .unwrap();
        let counters = &state.places[0].counters.current;
        assert_eq!(counters.produced[pork], 88, "slaughter output is produced");
        assert_eq!(
            counters.used_by_pops[herbs], 1,
            "zero-hunger ticking is use"
        );
        assert_eq!(
            counters.lost[pork], 86,
            "overflow loss is attributed by good"
        );
        assert_eq!(counters.lost.iter().sum::<u64>(), counters.losses);
        for good in 0..rules.goods.len() {
            assert_eq!(
                counters.used_by_pops[good],
                ledger.opened[good] + ledger.ticked[good],
                "{} counts both opened and ticked units",
                rules.goods[good].id
            );
        }
        for index in 0..rules.goods.len() {
            assert_eq!(
                ledger.stock_before[index] + ledger.butchered[index],
                ledger.stock_after[index]
                    + ledger.opened[index]
                    + ledger.ticked[index]
                    + ledger.lost[index],
                "{} balances in whole units",
                rules.goods[index].id
            );
        }
        assert_eq!(ledger.herds_before, [0, 1, 0]);
        assert_eq!(ledger.herd_slaughter, [0, 1, 0]);
        assert_eq!(ledger.herds_after, [0, 0, 0]);
        for species in 0..3 {
            assert_eq!(
                ledger.herds_before[species],
                ledger.herds_after[species] + ledger.herd_slaughter[species]
            );
        }
        for tier in 0..3 {
            let remaining = state.places[0]
                .pops
                .iter()
                .map(|pop| match tier {
                    0 => pop.consumption.necessities_centi,
                    1 => pop.consumption.niceties_centi,
                    _ => pop.consumption.luxuries_centi,
                })
                .map(u128::from)
                .sum::<u128>();
            assert_eq!(
                ledger.opened_benefits_centi[tier] + ledger.ticked_benefits_centi[tier],
                remaining + ledger.benefits_taken_centi[tier],
                "tier {tier} credits equal opened and ticked values"
            );
        }
    }

    #[test]
    fn drawing_respects_per_good_cap_without_changing_the_goods_total() {
        let (rules, mut state) = daily_accounting_fixture();
        let food = rules
            .goods
            .iter()
            .position(|good| good.id == "food")
            .unwrap();
        state.places[0].pops[0].stock[food] = 1;
        state.places[0].buildings[0].stock[food] = 2;
        state.places[0].buildings[0]
            .storehouse_accounts
            .as_mut()
            .unwrap()
            .pops[0][food] = 2;
        let before = total_state_stored_goods(&rules, &state).unwrap();
        draw_pop(&rules, &mut state.places[0], 0).unwrap();
        let after = total_state_stored_goods(&rules, &state).unwrap();
        assert_eq!(
            before, after,
            "drawing transfers rather than consumes units"
        );
        assert_eq!(state.places[0].pops[0].stock[food], 1);
        assert_eq!(
            state.places[0].buildings[0]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .pops[0][food],
            2,
            "the full per-good cap leaves the refused source units in the account"
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keeping the refusal matrix together makes every named rule-validation branch auditable."
    )]
    fn rules_validation_refusals_are_table_driven_and_name_the_entry() {
        let cases: Vec<(&str, RuleMutation)> = vec![
            (
                "buildable_base_m2",
                Box::new(|rules| rules.buildable_base_m2 = 0),
            ),
            (
                "starting_development_pm",
                Box::new(|rules| rules.starting_development_pm = 0),
            ),
            (
                "fertility_reference_pm",
                Box::new(|rules| rules.fertility_reference_pm = 0),
            ),
            (
                "footprint_m2.farm",
                Box::new(|rules| {
                    rules.footprint_m2.insert("farm".to_owned(), -1);
                }),
            ),
            (
                "footprint_m2.farm",
                Box::new(|rules| {
                    rules.footprint_m2.remove("farm");
                }),
            ),
            (
                "buildable_per_cell_m2.pasture",
                Box::new(|rules| {
                    rules.buildable_per_cell_m2.remove("pasture");
                }),
            ),
            ("price_k_pm", Box::new(|rules| rules.price_k_pm = 10_001)),
            ("goods[0]", Box::new(|rules| rules.goods[0].base_price = 0)),
            (
                "goods[0].pop_limit_milli",
                Box::new(|rules| rules.goods[0].pop_limit_milli = 1_000_000_001),
            ),
            (
                "pop_types[0]",
                Box::new(|rules| rules.pop_types[0].dependent_pm = 1_001),
            ),
            (
                "diet_profiles.peasant",
                Box::new(|rules| {
                    rules.diet_profiles.get_mut("peasant").unwrap().clear();
                }),
            ),
            (
                "pop_types[peasant].births_pm_year",
                Box::new(|rules| rules.pop_types[0].births_pm_year = 10_001),
            ),
            (
                "buildings[0]",
                Box::new(|rules| rules.buildings[0].workers_per_level = 0),
            ),
            (
                "buildings[0].owner_workers",
                Box::new(|rules| {
                    rules.buildings[0]
                        .owner_workers
                        .insert("peasant".to_owned(), 0);
                }),
            ),
            (
                "buildings[farm].methods",
                Box::new(|rules| rules.buildings[0].methods[0].id.clear()),
            ),
            (
                "buildings[0].methods.mixed",
                Box::new(|rules| rules.buildings[0].methods[0].resource.clear()),
            ),
            (
                "buildings[0].methods.mixed.annual_output.missing",
                Box::new(|rules| {
                    rules.buildings[0].methods[0]
                        .annual_output
                        .insert("missing".to_owned(), 1);
                }),
            ),
            (
                "buildings[0].methods.mixed.resource",
                Box::new(|rules| rules.buildings[0].methods[0].resource = "air".to_owned()),
            ),
            (
                "buildings[0].methods.mixed.score_share_pm",
                Box::new(|rules| rules.buildings[0].methods[0].score_share_pm.clear()),
            ),
            (
                "seasons.food",
                Box::new(|rules| {
                    rules.seasons.insert("food".to_owned(), vec![1; 11]);
                }),
            ),
            (
                "seasons.missing",
                Box::new(|rules| {
                    rules.seasons.insert(
                        "missing".to_owned(),
                        vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    );
                }),
            ),
            (
                "curves.bad",
                Box::new(|rules| {
                    rules.curves.insert("bad".to_owned(), vec![[0, 1], [0, 2]]);
                }),
            ),
            (
                "curves.bad",
                Box::new(|rules| {
                    rules.curves.insert("bad".to_owned(), vec![[0, 100_001]]);
                }),
            ),
            (
                "diet_profiles.peasant",
                Box::new(|rules| {
                    rules
                        .diet_profiles
                        .insert("peasant".to_owned(), BTreeMap::new());
                }),
            ),
            (
                "diet_profiles.peasant.missing",
                Box::new(|rules| {
                    rules.diet_profiles.get_mut("peasant").unwrap().clear();
                    rules
                        .diet_profiles
                        .get_mut("peasant")
                        .unwrap()
                        .insert("missing".to_owned(), 1000);
                }),
            ),
            (
                "suitability.unknown",
                Box::new(|rules| {
                    rules
                        .suitability
                        .insert("unknown".to_owned(), BTreeMap::new());
                }),
            ),
            (
                "suitability.pasture.crops",
                Box::new(|rules| {
                    rules
                        .suitability
                        .get_mut("pasture")
                        .unwrap()
                        .insert("crops".to_owned(), 1001);
                }),
            ),
            (
                "suitability.pasture.missing",
                Box::new(|rules| {
                    rules
                        .suitability
                        .get_mut("pasture")
                        .unwrap()
                        .insert("missing".to_owned(), 1);
                }),
            ),
            (
                "grazing.unknown",
                Box::new(|rules| {
                    rules.grazing.insert("unknown".to_owned(), BTreeMap::new());
                }),
            ),
            (
                "grazing.pasture.cattle",
                Box::new(|rules| {
                    rules
                        .grazing
                        .get_mut("pasture")
                        .unwrap()
                        .insert("cattle".to_owned(), 1001);
                }),
            ),
            (
                "grazing.pasture.goats",
                Box::new(|rules| {
                    rules
                        .grazing
                        .get_mut("pasture")
                        .unwrap()
                        .insert("goats".to_owned(), 1);
                }),
            ),
        ];
        for (expected_entry, invalidate) in cases {
            let mut rules = tiny_rules();
            invalidate(&mut rules);
            let error = rules.validate("fixture/economy.toml").unwrap_err();
            assert_eq!(error.entry, expected_entry, "case {expected_entry}");
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The scenario refusal matrix ties each authored-data fault to its exact source entry."
    )]
    fn scenario_refusal_cases_name_the_invalid_entry() {
        #[derive(Clone, Copy)]
        enum Case {
            MissingCulture,
            MissingReligion,
            MissingMonkReligion,
            UnknownCulture,
            UnknownPeasantCulture,
            UnknownReligion,
            UnknownMonkCulture,
            UnknownMonkReligion,
            UnknownStockGood,
            MissingKind,
            MissingSize,
            ZeroTemplateSize,
            NonnumericTemplateSize,
            EmptyStockKey,
            EmptyPlaceId,
            PaganShareOverLimit,
            InvalidPeasantCultureShares,
        }
        let cases = [
            (Case::MissingCulture, "places.fort.culture"),
            (Case::MissingReligion, "places.fort.religion"),
            (Case::MissingMonkReligion, "places.fort.monk_religion"),
            (Case::UnknownCulture, "places.fort.culture"),
            (
                Case::UnknownPeasantCulture,
                "places.fort.peasant_cultures_pm.missing",
            ),
            (Case::UnknownReligion, "places.fort.religion"),
            (Case::UnknownMonkCulture, "places.fort.monk_culture"),
            (Case::UnknownMonkReligion, "places.fort.monk_religion"),
            (Case::UnknownStockGood, "defaults.ringfort.1.stock.missing"),
            (Case::MissingKind, "defaults.monastery.1"),
            (Case::MissingSize, "defaults.ringfort.2"),
            (Case::ZeroTemplateSize, "defaults.ringfort.0"),
            (Case::NonnumericTemplateSize, "defaults.ringfort.large"),
            (Case::EmptyStockKey, "defaults.ringfort.1"),
            (Case::EmptyPlaceId, "places."),
            (Case::PaganShareOverLimit, "places.fort"),
            (
                Case::InvalidPeasantCultureShares,
                "places.fort.peasant_cultures_pm",
            ),
        ];
        for (case, expected_entry) in cases {
            let rules = tiny_rules();
            let mut data = tiny_start_data();
            let mut location = StartLocation {
                id: "fort".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts::default(),
            };
            match case {
                Case::MissingCulture => {
                    data.places.get_mut("fort").unwrap().culture = None;
                }
                Case::MissingReligion => {
                    data.places.get_mut("fort").unwrap().religion = None;
                }
                Case::MissingMonkReligion => {
                    data.places.get_mut("fort").unwrap().monk_religion = None;
                }
                Case::UnknownCulture => {
                    data.places.entry("fort".to_owned()).or_default().culture =
                        Some("missing".to_owned());
                }
                Case::UnknownPeasantCulture => {
                    data.places.get_mut("fort").unwrap().peasant_cultures_pm =
                        Some(BTreeMap::from([("missing".to_owned(), 1000)]));
                }
                Case::UnknownReligion => {
                    data.places.entry("fort".to_owned()).or_default().religion =
                        Some("missing".to_owned());
                }
                Case::UnknownMonkCulture => {
                    data.places
                        .entry("fort".to_owned())
                        .or_default()
                        .monk_culture = Some("missing".to_owned());
                }
                Case::UnknownMonkReligion => {
                    data.places
                        .entry("fort".to_owned())
                        .or_default()
                        .monk_religion = Some("missing".to_owned());
                }
                Case::UnknownStockGood => {
                    data.defaults
                        .get_mut("ringfort")
                        .unwrap()
                        .get_mut("1")
                        .unwrap()
                        .pop_stock
                        .insert("missing".to_owned(), 1);
                }
                Case::MissingKind => location.kind = "monastery".to_owned(),
                Case::MissingSize => location.size = 2,
                Case::ZeroTemplateSize => {
                    let template = data.defaults["ringfort"]["1"].clone();
                    data.defaults
                        .get_mut("ringfort")
                        .unwrap()
                        .insert("0".to_owned(), template);
                }
                Case::NonnumericTemplateSize => {
                    let template = data.defaults["ringfort"]["1"].clone();
                    data.defaults
                        .get_mut("ringfort")
                        .unwrap()
                        .insert("large".to_owned(), template);
                }
                Case::EmptyStockKey => {
                    data.defaults
                        .get_mut("ringfort")
                        .unwrap()
                        .get_mut("1")
                        .unwrap()
                        .pop_stock
                        .insert(String::new(), 1);
                }
                Case::EmptyPlaceId => {
                    data.places.insert(String::new(), StartOverride::default());
                }
                Case::PaganShareOverLimit => {
                    data.places
                        .entry("fort".to_owned())
                        .or_default()
                        .pagan_share_pm = Some(1001);
                }
                Case::InvalidPeasantCultureShares => {
                    data.places.get_mut("fort").unwrap().peasant_cultures_pm =
                        Some(BTreeMap::from([("connachta".to_owned(), 600)]));
                }
            }
            let error = if matches!(
                case,
                Case::UnknownCulture | Case::UnknownMonkCulture | Case::UnknownPeasantCulture
            ) {
                data.validate_cultures(&rules.peoples, "fixture/scenario.toml")
                    .unwrap_err()
            } else if matches!(
                case,
                Case::ZeroTemplateSize
                    | Case::NonnumericTemplateSize
                    | Case::EmptyStockKey
                    | Case::EmptyPlaceId
                    | Case::PaganShareOverLimit
                    | Case::InvalidPeasantCultureShares
            ) {
                data.validate("fixture/scenario.toml").unwrap_err()
            } else {
                starting_state(&rules, &data, &[location], 10).unwrap_err()
            };
            assert_eq!(error.entry, expected_entry, "case {expected_entry}");
            if matches!(
                case,
                Case::UnknownCulture | Case::UnknownMonkCulture | Case::UnknownPeasantCulture
            ) {
                assert_eq!(error.file, "fixture/scenario.toml");
            }
        }
    }

    #[test]
    fn matching_start_cohorts_keep_distinct_slots_and_floors() {
        let mut rules = tiny_rules();
        if !rules.religions.iter().any(|r| r == "irish_paganism") {
            rules.religions.push("irish_paganism".into());
        }
        rules.pop_types[0].births_pm_year = 0;
        let mut data = tiny_start_data();
        let place = data.places.get_mut("fort").unwrap();
        place.religion = Some("irish_paganism".into());
        place.pagan_share_pm = Some(250);
        let location = StartLocation {
            id: "fort".into(),
            kind: "ringfort".into(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".into(), 8)]),
                fertility_pm: BTreeMap::from([("pasture".into(), 8000)]),
                fertile_cells: 8,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        };
        let (mut state, _) = starting_state(&rules, &data, &[location], 10).unwrap();
        rules.pop_types[0].worker_deaths_pm_year = 12_000;
        rules.pop_types[0].dependent_deaths_pm_year = 12_000;
        let p = &state.places[0].pops;
        assert_eq!(p.len(), 2);
        assert_eq!(
            (
                p[0].workers + p[0].dependents,
                p[1].workers + p[1].dependents
            ),
            (15, 5)
        );
        assert_eq!(
            (&p[0].kind, &p[0].culture, &p[0].religion),
            (&p[1].kind, &p[1].culture, &p[1].religion)
        );
        let floors = BTreeMap::from([(("fort".to_owned(), 1), 5)]);
        run_monthly_demography_with_floors(
            &rules,
            &mut state,
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            &floors,
        )
        .unwrap();
        assert_eq!(
            state.places[0].pops[1].workers + state.places[0].pops[1].dependents,
            5
        );
        let decoded = EconomyState::from_bytes(&state.to_bytes().unwrap()).unwrap();
        assert_eq!(decoded.places[0].pops[1], state.places[0].pops[1]);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The literal expected startup state keeps owner-first employment and the new seeded wage history together."
    )]
    fn starting_state_is_literal_and_fills_owners_before_unlimited_starting_hires() {
        let rules = tiny_rules();
        let data = tiny_start_data();
        let location = StartLocation {
            id: "fort".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 8)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 8000)]),
                fertile_cells: 8,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        };
        let (state, report) = starting_state(&rules, &data, &[location], 10).unwrap();
        assert!(report.is_empty());
        assert_eq!(
            state.places,
            vec![PlaceState {
                id: "fort".to_owned(),
                price_drift_carries: vec![0],
                last_market_orders: None,
                counters: CounterBuckets::for_place(rules.goods.len()),
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
                    literacy_pm: 0,
                    workers: 12,
                    dependents: 8,
                    employment: vec![
                        Employment {
                            owners: 1,
                            workers: 10
                        },
                        Employment {
                            owners: 0,
                            workers: 1
                        }
                    ],
                    stock: vec![20],
                    consumption: PopConsumption {
                        pantry_hunger_centi: vec![0],
                        ticking_carries: vec![0],
                        necessities_pm: 1000,
                        ..PopConsumption::default()
                    },
                    counters: CounterBuckets::default(),
                    short_on_rent: false,
                    housed_last: false,
                    genetics: PopGenetics::default(),
                    pending_births: 0,
                    pending_deaths: 0,
                }],
                buildings: vec![
                    BuildingState {
                        kind: "farm".to_owned(),
                        levels: 1,
                        method_levels: vec![1],
                        level_records: vec![LevelRecord {
                            number: 0,
                            method: 0,
                            ground: Some("pasture".to_owned()),
                            paydays: MATURE_PAYDAYS,
                            unpaid_work: BTreeMap::from([(
                                "food".to_owned(),
                                bigint::Big::default()
                            )]),
                            last_switch_year: None,
                        }],
                        herds: Vec::new(),
                        employment: vec![Employment {
                            owners: 1,
                            workers: 10
                        }],
                        stock: vec![2],
                        storehouse_accounts: None,
                        production_carries: vec![vec![0]],
                        method_suitability: vec![vec![1000]],
                        herd_birth_carries: Vec::new(),
                        herd_death_carries: Vec::new(),
                        herd_product_carries: Vec::new(),
                        herd_grazing_sums: Vec::new(),
                        wage_carries: vec![0],
                        pay_carries: vec![0],
                        fee_carries: Vec::new(),
                        wage_history: vec![1818; 12],
                        paid_history: vec![0; WAGE_HISTORY_MONTHS],
                        paid_years: vec![0; WAGE_HISTORY_MONTHS],
                    },
                    BuildingState {
                        kind: "storehouse".to_owned(),
                        levels: 1,
                        method_levels: vec![1, 0, 0],
                        level_records: Vec::new(),
                        herds: Vec::new(),
                        employment: vec![Employment {
                            owners: 0,
                            workers: 1
                        }],
                        stock: vec![0],
                        storehouse_accounts: Some(StorehouseAccounts {
                            pops: vec![vec![0]],
                            communal: vec![0],
                        }),
                        production_carries: Vec::new(),
                        method_suitability: Vec::new(),
                        herd_birth_carries: Vec::new(),
                        herd_death_carries: Vec::new(),
                        herd_product_carries: Vec::new(),
                        herd_grazing_sums: Vec::new(),
                        wage_carries: vec![0],
                        pay_carries: vec![0],
                        fee_carries: vec![vec![0], vec![0]],
                        wage_history: vec![0; 12],
                        paid_history: vec![0; WAGE_HISTORY_MONTHS],
                        paid_years: vec![0; WAGE_HISTORY_MONTHS],
                    },
                ],
                prices_milli: vec![100_000],
                prepared_m2: 22_200,
                development_pm: 1000,
            }]
        );
    }

    #[test]
    fn grain_harvest_distance_uses_the_smallest_positive_future_month() {
        let mut rules = tiny_rules();
        assert_eq!(months_to_grain_harvest(&rules, 10), Ok(11));
        assert_eq!(months_to_grain_harvest(&rules, 9), Ok(12));
        rules.seasons.insert(
            "grain".to_owned(),
            vec![0, 0, 0, 0, 1000, 0, 0, 0, 1000, 0, 0, 0],
        );
        assert_eq!(months_to_grain_harvest(&rules, 6), Ok(3));
        assert_eq!(months_to_grain_harvest(&rules, 10), Ok(7));
    }

    #[test]
    fn starting_wage_history_spreads_stockpile_value_to_the_next_grain_harvest() {
        let rules = tiny_rules();
        let mut data = tiny_start_data();
        data.defaults
            .get_mut("ringfort")
            .unwrap()
            .get_mut("1")
            .unwrap()
            .building_stock
            .insert("food".to_owned(), 11);
        let location = StartLocation {
            id: "fort".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 8)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 8000)]),
                fertile_cells: 8,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        };
        let (state, _) =
            starting_state(&rules, &data, std::slice::from_ref(&location), 10).unwrap();
        let farm = state.places[0]
            .buildings
            .iter()
            .find(|building| building.kind == "farm")
            .unwrap();
        assert_eq!(farm.stock, [11]);
        assert_eq!(farm.wage_history, [10_000; 12]);
        assert_eq!(farm.posted_wage(), 10_000);
        let storehouse = state.places[0]
            .buildings
            .iter()
            .find(|building| building.kind == "storehouse")
            .unwrap();
        assert_eq!(storehouse.stock, [0]);
        assert_eq!(storehouse.wage_history, [0; 12]);
        assert_eq!(storehouse.posted_wage(), 0);

        data.defaults
            .get_mut("ringfort")
            .unwrap()
            .get_mut("1")
            .unwrap()
            .peasants = 0;
        let (unpaid, _) = starting_state(&rules, &data, &[location], 10).unwrap();
        let farm = unpaid.places[0]
            .buildings
            .iter()
            .find(|building| building.kind == "farm")
            .unwrap();
        assert!(
            farm.employment
                .iter()
                .all(|employment| employment.workers == 0)
        );
        assert_eq!(farm.wage_history, [0; 12]);
    }

    #[test]
    fn starting_state_refuses_invalid_months_before_building_any_state() {
        let rules = tiny_rules();
        let data = tiny_start_data();
        for month in [0, 13] {
            let error = starting_state(&rules, &data, &[], month).unwrap_err();
            assert_eq!(error.entry, "start_month");
            assert_eq!(error.problem, "must be in 1..=12");
        }
    }

    #[test]
    fn a_wrong_length_wage_history_is_a_named_refusal_and_rolls_back() {
        let mut rules = tiny_rules();
        rules.seasons.insert(
            "herd_births".to_owned(),
            vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        let location = StartLocation {
            id: "fort".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 8)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 8000)]),
                fertile_cells: 8,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        };
        let (mut state, _) = starting_state(&rules, &tiny_start_data(), &[location], 10).unwrap();
        state.places[0]
            .buildings
            .iter_mut()
            .find(|building| building.kind == "farm")
            .unwrap()
            .wage_history
            .pop();
        let before = state.clone();
        let error = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap_err();
        assert_eq!(error.entry, "places.fort.buildings.farm.wage_history");
        assert!(error.problem.contains("exactly 12"));
        assert_eq!(state, before, "a refused payday commits no state");
    }

    #[test]
    fn paid_head_history_arrays_must_match_the_calendar_ring_length() {
        let rules = tiny_rules();
        let location = StartLocation {
            id: "fort".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 8)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 8000)]),
                fertile_cells: 8,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        };
        let (starting, _) = starting_state(&rules, &tiny_start_data(), &[location], 10).unwrap();
        for short_paid_years in [false, true] {
            let mut state = starting.clone();
            let farm = state.places[0]
                .buildings
                .iter_mut()
                .find(|building| building.kind == "farm")
                .unwrap();
            if short_paid_years {
                farm.paid_years.pop();
            } else {
                farm.paid_history.pop();
            }
            let before = state.clone();
            let error = run_monthly_production(
                &rules,
                &mut state,
                Date {
                    year: 768,
                    month: 1,
                    day: 1,
                    hour: 12,
                },
            )
            .unwrap_err();
            assert_eq!(error.entry, "places.fort.buildings.farm.wage_history");
            assert_eq!(state, before, "a refused payday commits no state");
        }
    }

    #[test]
    fn a_posted_wage_that_exceeds_u64_is_a_named_refusal_and_rolls_back() {
        let mut rules = tiny_rules();
        rules.buildings[0].methods[0]
            .annual_output
            .insert("food".to_owned(), 48);
        rules.buildings[0].overhead_pm = 500;
        rules.seasons.insert(
            "food".to_owned(),
            vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        rules.seasons.insert(
            "herd_births".to_owned(),
            vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        let location = StartLocation {
            id: "fort".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 8)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 8000)]),
                fertile_cells: 8,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        };
        let (mut state, _) = starting_state(&rules, &tiny_start_data(), &[location], 10).unwrap();
        let place = &mut state.places[0];
        let farm = place
            .buildings
            .iter_mut()
            .find(|building| building.kind == "farm")
            .unwrap();
        farm.employment[0].workers = 1;
        place.prices_milli[0] = u64::MAX;
        let before = state.clone();
        let error = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap_err();
        assert_eq!(error.entry, "places.fort.buildings.farm.wage_history");
        assert!(error.problem.contains("exceeds u64"));
        assert_eq!(state, before, "a refused payday commits no state");
    }

    #[test]
    fn a_harvest_replaces_one_month_of_the_twelve_month_average() {
        let mut rules = tiny_rules();
        rules.buildings[0].methods[0]
            .annual_output
            .insert("food".to_owned(), 2);
        rules.buildings[0].overhead_pm = 500;
        rules.seasons.insert(
            "food".to_owned(),
            vec![0, 0, 0, 0, 0, 0, 0, 0, 1000, 0, 0, 0],
        );
        rules.seasons.insert(
            "herd_births".to_owned(),
            vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        let location = StartLocation {
            id: "fort".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 8)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 8000)]),
                fertile_cells: 8,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        };
        let (mut state, _) = starting_state(&rules, &tiny_start_data(), &[location], 10).unwrap();
        let place = &mut state.places[0];
        let farm = place
            .buildings
            .iter_mut()
            .find(|building| building.kind == "farm")
            .unwrap();
        farm.wage_history = vec![10_000; 12];
        place.prices_milli[0] = 12_345;
        let before = farm.posted_wage();
        run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 9,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let farm = state.places[0]
            .buildings
            .iter()
            .find(|building| building.kind == "farm")
            .unwrap();
        assert_eq!(farm.wage_history[8], 1234);
        assert_eq!(farm.posted_wage(), (11 * 10_000 + 1234) / 12);
        assert_eq!(before, 10_000);
        assert_eq!(farm.posted_wage(), 9_269);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "One sequential payday fixture verifies paid, unpaid, and no-output history transitions."
    )]
    fn payday_records_value_per_paid_head_and_preserves_or_clears_months_correctly() {
        let mut rules = tiny_rules();
        rules.buildings[0].methods[0]
            .annual_output
            .insert("food".to_owned(), 2);
        rules.buildings[0].overhead_pm = 500;
        rules.seasons.insert(
            "food".to_owned(),
            vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        rules.seasons.insert(
            "herd_births".to_owned(),
            vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        let location = StartLocation {
            id: "fort".to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 8)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 8000)]),
                fertile_cells: 8,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        };
        let (mut state, _) = starting_state(&rules, &tiny_start_data(), &[location], 10).unwrap();
        let place = &mut state.places[0];
        let farm = place
            .buildings
            .iter_mut()
            .find(|building| building.kind == "farm")
            .unwrap();
        assert_eq!(
            farm.employment.iter().map(|item| item.workers).sum::<u64>(),
            10
        );
        farm.wage_history = vec![9000; 12];
        place.prices_milli[0] = 12_345;
        run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let farm = state.places[0]
            .buildings
            .iter()
            .find(|building| building.kind == "farm")
            .unwrap();
        assert_eq!(farm.wage_history[0], 1234);
        assert_eq!(farm.paid_history[0], 10);
        assert_eq!(farm.paid_years[0], 768);
        assert_eq!(farm.posted_wage(), (11 * 9000 + 1234) / 12);

        let farm = state.places[0]
            .buildings
            .iter_mut()
            .find(|building| building.kind == "farm")
            .unwrap();
        farm.wage_history[1] = 7777;
        farm.paid_history[1] = 31;
        farm.paid_years[1] = 767;
        for employment in &mut farm.employment {
            employment.workers = 0;
        }
        run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 2,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let farm = state.places[0]
            .buildings
            .iter_mut()
            .find(|building| building.kind == "farm")
            .unwrap();
        assert_eq!(farm.wage_history[1], 7777);
        assert_eq!(farm.paid_history[1], 31);
        assert_eq!(farm.paid_years[1], 767);
        for employment in &mut farm.employment {
            employment.workers = 10;
        }
        run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 3,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let farm = state.places[0]
            .buildings
            .iter()
            .find(|building| building.kind == "farm")
            .unwrap();
        assert_eq!(farm.wage_history[2], 0, "heads with no output record zero");
    }

    #[test]
    fn storehouse_fee_records_only_new_fee_value_per_paid_head_even_with_old_carry_and_overflow() {
        let (mut rules, mut state) = tiny_storehouse_payday_state();
        let place = &mut state.places[0];
        let farm_index = place
            .buildings
            .iter()
            .position(|building| building.kind == "farm")
            .unwrap();
        let storehouse_index = place
            .buildings
            .iter()
            .position(|building| building.kind == "storehouse")
            .unwrap();
        rules.goods[0].pop_limit_milli = 0;
        place.prices_milli[0] = 3001;
        for employment in &mut place.buildings[farm_index].employment {
            employment.workers = 0;
        }
        place.pops[0].employment[farm_index].workers = 0;
        let storehouse = &mut place.buildings[storehouse_index];
        storehouse.employment[0].workers = 2;
        place.pops[0].employment[storehouse_index].workers = 2;
        storehouse.pay_carries[0] = 5;
        storehouse.stock[0] = 100;
        storehouse.storehouse_accounts.as_mut().unwrap().pops[0][0] = 100;
        let opening_pop_food = place.pops[0].stock[0];

        let mut ledger = ProductionLedger {
            lost: vec![0; rules.goods.len()],
            unpaid_wages: vec![0; rules.goods.len()],
            places: vec![PlaceProductionLedger::new("fort", rules.goods.len(), 3)],
            ..ProductionLedger::default()
        };
        ledger.places[0].stock_after = total_stored_goods(&rules, &state.places[0]).unwrap();
        ledger.stock_after = ledger.places[0].stock_after.clone();
        charge_storehouse_fees(&rules, &mut state, 0, 768, &mut ledger).unwrap();
        let place = &state.places[0];
        let storehouse = &place.buildings[storehouse_index];
        assert_eq!(storehouse.wage_history[0], 1500);
        assert_eq!(storehouse.paid_history[0], 2);
        assert_eq!(storehouse.paid_years[0], 768);
        assert_eq!(
            storehouse.pay_carries[0], 0,
            "the five old carried units were released with this month's one fee unit"
        );
        assert_eq!(
            place.pops[0].stock[0], opening_pop_food,
            "the recipient's zero cap overflows all six units"
        );
        assert_eq!(
            ledger.lost[0], 6,
            "all five released carry units and this month's one fee unit overflow"
        );
        assert_eq!(ledger.places[0].lost[0], 6);
        assert_eq!(ledger.unpaid_wages[0], 0);
        assert_eq!(place.counters.current.losses, 6);
        assert_eq!(place.counters.last.losses, 0);
    }

    #[test]
    fn first_of_month_storehouse_fee_uses_staff_hired_that_day() {
        let (mut rules, mut state) = daily_storehouse_fee_state();
        let oats = rules
            .goods
            .iter()
            .position(|good| good.id == "oats")
            .unwrap();
        rules.goods[oats].hunger_centi = 0;
        let place = &mut state.places[0];
        let storehouse_index = place
            .buildings
            .iter()
            .position(|building| building.kind == "storehouse")
            .unwrap();
        let storehouse = &mut place.buildings[storehouse_index];
        storehouse.employment[0].workers = 0;
        storehouse.stock[oats] = 1_000;
        let accounts = storehouse.storehouse_accounts.as_mut().unwrap();
        for account in &mut accounts.pops {
            account[oats] = 0;
        }
        accounts.communal[oats] = 1_000;
        place.pops[0].employment[storehouse_index].workers = 0;

        let ledger = run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let place = &state.places[0];
        let storehouse = &place.buildings[storehouse_index];
        assert_eq!(storehouse.employment[0].workers, 1);
        assert_eq!(
            storehouse.storehouse_accounts.as_ref().unwrap().communal[oats],
            990
        );
        assert_eq!(storehouse.wage_history[0], 1_000_000);
        assert_eq!(storehouse.paid_history[0], 1);
        assert_eq!(storehouse.paid_years[0], 768);
        assert_eq!(place.buildings[0].paid_history[0], 10);
        assert_eq!(place.buildings[0].paid_years[0], 768);
        assert_eq!(ledger.lost[oats], 0);
    }

    #[test]
    fn first_of_month_fee_with_no_staff_keeps_the_fee_and_wage_slot() {
        let (mut rules, mut state) = daily_storehouse_fee_state();
        let oats = rules
            .goods
            .iter()
            .position(|good| good.id == "oats")
            .unwrap();
        rules.goods[oats].hunger_centi = 0;
        let place = &mut state.places[0];
        let storehouse_index = 2;
        let storehouse = &mut place.buildings[storehouse_index];
        storehouse.levels = 0;
        storehouse.employment.fill(Employment::default());
        storehouse.wage_history[0] = 777;
        storehouse.paid_history[0] = 9;
        storehouse.paid_years[0] = 767;
        storehouse.stock[oats] = 100;
        storehouse.storehouse_accounts.as_mut().unwrap().communal[oats] = 100;
        for employment in &mut place.pops {
            employment.employment[storehouse_index] = Employment::default();
        }

        let ledger = run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let storehouse = &state.places[0].buildings[storehouse_index];
        assert_eq!(storehouse.wage_history[0], 777);
        assert_eq!(storehouse.paid_history[0], 9);
        assert_eq!(storehouse.paid_years[0], 767);
        assert_eq!(storehouse.pay_carries[oats], 1);
        assert_eq!(
            storehouse.storehouse_accounts.as_ref().unwrap().communal[oats],
            99
        );
        assert_eq!(ledger.unpaid_wages[oats], 1);
        assert_eq!(ledger.places[0].unpaid_wages[oats], 1);
    }

    #[test]
    fn first_of_month_movement_reads_the_old_storehouse_wage_before_fee_posting() {
        let (mut rules, mut state) = daily_storehouse_fee_state();
        rules.pop_types[0].births_pm_year = 0;
        rules.pop_types[0].worker_deaths_pm_year = 0;
        rules.pop_types[0].dependent_deaths_pm_year = 0;
        let oats = rules
            .goods
            .iter()
            .position(|good| good.id == "oats")
            .unwrap();
        rules.goods[oats].pop_limit_milli = 1_000_000_000;
        rules.goods[oats].volume_ml = 1;
        let place = &mut state.places[0];
        place.pops.truncate(1);
        place.pops[0].workers = 20;
        place.pops[0].dependents = 0;
        place.pops[0].employment = vec![Employment::default(); place.buildings.len()];
        place.pops[0].employment[0].owners = 1;
        place.pops[0].stock[oats] = 10_000;
        place.buildings[0].employment.truncate(1);
        place.buildings[0].employment[0].owners = 1;
        place.buildings[0].employment[0].workers = 0;
        place.buildings[1].employment.truncate(1);
        place.buildings[1].wage_history.fill(11);
        place.buildings[2].employment.truncate(1);
        place.buildings[2].employment[0].workers = 2;
        place.pops[0].employment[2].workers = 2;
        place.buildings[2].wage_history.fill(0);
        place.buildings[2].wage_history[0] = 1_200;
        let storehouse = &mut place.buildings[2];
        storehouse
            .storehouse_accounts
            .as_mut()
            .unwrap()
            .pops
            .truncate(1);
        storehouse.storehouse_accounts.as_mut().unwrap().pops[0][oats] = 0;
        storehouse.storehouse_accounts.as_mut().unwrap().communal[oats] = 100;
        storehouse.fee_carries.truncate(2);
        storehouse.stock[oats] = 100;
        assert_eq!(place.buildings[1].posted_wage(), 11);
        assert_eq!(pop_wage_per_head(place, 0).unwrap(), 10);
        assert!(source_can_fill(&place.pops[0], "labourer", 11, 10));
        let mut local_place = place.clone();
        let allowance = fill_monthly_hiring(&rules, &mut local_place).unwrap();
        assert!(allowance[1][1] > 0);
        let local_movement = move_open_positions(
            &rules,
            std::slice::from_mut(&mut local_place),
            &BTreeMap::new(),
            &[allowance],
        )
        .unwrap();
        assert_eq!(local_movement[0].iter().sum::<u64>(), 1);

        let ledger = run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let place = &state.places[0];
        assert_eq!(ledger.movement_by_pop[0].iter().sum::<u64>(), 1);
        assert_eq!(place.buildings[2].wage_history[0], 50_000);
        assert_eq!(
            place.buildings[2]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .communal[oats],
            99,
            "the communal 100-oat account is unchanged by movement and pays a one-oat fee"
        );
    }

    #[test]
    fn storehouse_fee_uses_the_account_balance_after_a_mover_withdraws_their_share() {
        let (mut rules, mut state) = daily_storehouse_fee_state();
        let oats = rules
            .goods
            .iter()
            .position(|good| good.id == "oats")
            .unwrap();
        rules.goods[oats].pop_limit_milli = 1_000_000_000;
        rules.goods[oats].volume_ml = 1;
        let source = &mut state.places[0];
        source.pops.truncate(1);
        source.pops[0].workers = 10;
        source.pops[0].dependents = 0;
        source.pops[0].employment = vec![Employment::default(); source.buildings.len()];
        source.pops[0].employment[0].owners = 1;
        source.buildings[0].employment.truncate(1);
        source.buildings[0].employment[0].owners = 1;
        source.buildings[0].employment[0].workers = 0;
        source.buildings[1].employment.truncate(1);
        source.buildings[2].employment.truncate(1);
        source.buildings[2].employment[0].workers = 1;
        source.pops[0].employment[2].workers = 1;
        let storehouse = &mut source.buildings[2];
        storehouse
            .storehouse_accounts
            .as_mut()
            .unwrap()
            .pops
            .truncate(1);
        storehouse.storehouse_accounts.as_mut().unwrap().pops[0][oats] = 1_000;
        storehouse.storehouse_accounts.as_mut().unwrap().communal[oats] = 0;
        storehouse.fee_carries.truncate(2);
        storehouse.stock[oats] = 1_000;
        assert_eq!((source.pops[0].workers, source.pops[0].dependents), (10, 0));

        let mut destination = state.places[0].clone();
        destination.id = "destination".to_owned();
        destination.pops.clear();
        for building in &mut destination.buildings {
            building.employment.clear();
            if let Some(accounts) = &mut building.storehouse_accounts {
                accounts.pops.clear();
            }
        }
        let mut places = vec![state.places.remove(0), destination];
        let (moved, _) = move_recruits(&rules, &mut places, 0, 0, 1, 1, "labourer", 1).unwrap();
        assert_eq!(moved, 1);
        let source = &mut places[0];
        assert_eq!(
            source.buildings[2]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .pops[0][oats],
            900
        );
        let mut ledger = PlaceProductionLedger::new(&source.id, rules.goods.len(), 3);
        pay_storehouse_fee(
            &rules,
            &source.id,
            &source.prices_milli,
            0,
            768,
            &mut source.pops,
            &mut source.buildings,
            2,
            &mut source.counters,
            &mut ledger,
        )
        .unwrap();
        assert_eq!(
            source.buildings[2]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .pops[0][oats],
            891
        );
    }

    #[test]
    fn a_storehouse_without_paid_heads_keeps_its_posted_wage_entry() {
        let (rules, mut state) = tiny_storehouse_payday_state();
        let place = &mut state.places[0];
        let storehouse_index = place
            .buildings
            .iter()
            .position(|building| building.kind == "storehouse")
            .unwrap();
        let storehouse = &mut place.buildings[storehouse_index];
        storehouse.wage_history[0] = 777;
        storehouse.stock[0] = 100;
        storehouse.storehouse_accounts.as_mut().unwrap().pops[0][0] = 100;
        storehouse.levels = 0;
        storehouse.employment[0].workers = 0;
        place.pops[0].employment[storehouse_index].workers = 0;

        let place = &mut state.places[0];
        let mut ledger =
            PlaceProductionLedger::new(&place.id, rules.goods.len(), rules.herd_species.len());
        pay_storehouse_fee(
            &rules,
            &place.id,
            &place.prices_milli,
            0,
            768,
            &mut place.pops,
            &mut place.buildings,
            storehouse_index,
            &mut place.counters,
            &mut ledger,
        )
        .unwrap();
        let storehouse = &place.buildings[storehouse_index];
        assert_eq!(storehouse.wage_history[0], 777);
        assert_eq!(storehouse.pay_carries[0], 1);
    }

    #[test]
    fn a_storehouse_fee_wage_overflow_refuses_and_rolls_back_its_history() {
        let (rules, mut state) = tiny_storehouse_payday_state();
        let place = &mut state.places[0];
        let farm_index = place
            .buildings
            .iter()
            .position(|building| building.kind == "farm")
            .unwrap();
        let storehouse_index = place
            .buildings
            .iter()
            .position(|building| building.kind == "storehouse")
            .unwrap();
        for employment in &mut place.buildings[farm_index].employment {
            employment.workers = 0;
        }
        place.pops[0].employment[farm_index].workers = 0;
        place.prices_milli[0] = 500_000;
        let opening_total = total_stored_goods(&rules, place).unwrap()[0];
        let opening_storehouse = place.buildings[storehouse_index].stock[0];
        let account_amount = u64::MAX - (opening_total - opening_storehouse);
        let storehouse = &mut place.buildings[storehouse_index];
        storehouse.wage_history[0] = 1234;
        storehouse.employment[0].workers = 1;
        place.pops[0].employment[storehouse_index].workers = 1;
        storehouse.stock[0] = account_amount;
        storehouse.storehouse_accounts.as_mut().unwrap().pops[0][0] = account_amount;
        let before = state.clone();

        let error = run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap_err();
        assert_eq!(error.entry, "places.fort.buildings.storehouse.wage_history");
        assert!(error.problem.contains("exceeds u64"));
        assert_eq!(
            state, before,
            "the refused fee payday commits no history or payment"
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "This integration case checks seasonal carry, storehouse fee timing, day gating and rollback together."
    )]
    fn monthly_production_keeps_fractional_seasonal_output_and_defers_storehouse_fee() {
        let mut rules = tiny_rules();
        rules.goods[0].pop_limit_milli = 1_000_000_000;
        rules.seasons.insert(
            "food".to_owned(),
            vec![500, 500, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        rules.seasons.insert(
            "herd_births".to_owned(),
            vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        install_fixture_herds(&mut rules);
        let (mut state, _) = starting_state(
            &rules,
            &tiny_start_data(),
            &[StartLocation {
                id: "fort".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 4)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), 4000)]),
                    fertile_cells: 4,
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 0,
                },
            }],
            10,
        )
        .unwrap();
        let farm = state.places[0]
            .buildings
            .iter()
            .position(|building| building.kind == "farm")
            .unwrap();
        let storehouse = state.places[0]
            .buildings
            .iter()
            .position(|building| building.kind == "storehouse")
            .unwrap();
        let accounts = state.places[0].buildings[storehouse]
            .storehouse_accounts
            .as_mut()
            .unwrap();
        accounts.pops[0][0] = 1_000;
        accounts.communal[0] = 1_000;
        state.places[0].buildings[storehouse].stock[0] = 2_000;
        assert_eq!(state.places[0].buildings[farm].stock[0], 2);
        run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(state.places[0].buildings[farm].stock[0], 2);
        assert_eq!(state.places[0].buildings[storehouse].stock[0], 2_000);
        assert_eq!(
            state.places[0].buildings[storehouse]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .pops[0][0],
            1_000
        );
        assert_eq!(
            state.places[0].buildings[storehouse]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .communal[0],
            1_000
        );
        assert_eq!(
            state.places[0].buildings[farm].production_carries[0][0],
            60_000_000_000
        );
        run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 2,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(state.places[0].buildings[farm].stock[0], 3);
        assert_eq!(state.places[0].buildings[farm].production_carries[0][0], 0);
        run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 3,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(state.places[0].buildings[farm].stock[0], 3);
        state.places[0].buildings[farm].herds[0] = u64::MAX;
        let before_refusal = state.clone();
        assert!(
            run_monthly_production(
                &rules,
                &mut state,
                Date {
                    year: 768,
                    month: 1,
                    day: 1,
                    hour: 12
                },
            )
            .is_err()
        );
        assert_eq!(
            state, before_refusal,
            "a later checked arithmetic refusal rolls the whole monthly step back"
        );
    }

    #[test]
    fn partial_building_staffing_is_shared_by_each_production_method() {
        let (rules, mut state) = two_method_staffing_fixture(1200);
        let oats = rules
            .goods
            .iter()
            .position(|good| good.id == "oats")
            .unwrap();
        let fish = rules
            .goods
            .iter()
            .position(|good| good.id == "fish")
            .unwrap();
        let date = Date {
            year: 768,
            month: 1,
            day: 1,
            hour: 12,
        };

        let ledger = run_monthly_production(&rules, &mut state, date).unwrap();
        assert_eq!(
            ledger.produced[oats], 60,
            "old ordered staffing produced 120 oats"
        );
        assert_eq!(
            ledger.produced[fish], 60,
            "old ordered staffing produced no fish"
        );
        assert_eq!(state.places[0].buildings[1].production_carries[0][oats], 0);
        assert_eq!(state.places[0].buildings[1].production_carries[1][fish], 0);

        state.places[0].buildings[1].levels = 1;
        state.places[0].buildings[1].method_levels = vec![1, 0];
        sync_levels(&rules, &mut state.places[0].buildings[1]);
        let ledger = run_monthly_production(&rules, &mut state, Date { month: 2, ..date }).unwrap();
        assert_eq!(ledger.produced[oats], 120);
        assert_eq!(ledger.produced[fish], 0);
    }

    #[test]
    fn uneven_method_levels_keep_building_wide_staffing_carries() {
        let (rules, mut state) = two_method_staffing_fixture(1000);
        state.places[0].buildings[1].levels = 3;
        state.places[0].buildings[1].method_levels = vec![2, 1];
        sync_levels(&rules, &mut state.places[0].buildings[1]);
        let oats = rules
            .goods
            .iter()
            .position(|good| good.id == "oats")
            .unwrap();
        state.places[0].buildings[1].method_suitability[0][oats] = 2000;
        set_holdings_staffing(&mut state, 1, 4, 1, 3);
        let fish = rules
            .goods
            .iter()
            .position(|good| good.id == "fish")
            .unwrap();
        let expected = [
            (46, 240_000_000_000, 23, 120_000_000_000),
            (47, 120_000_000_000, 23, 240_000_000_000),
            (47, 0, 24, 0),
        ];

        for (index, (oats_output, oats_carry, fish_output, fish_carry)) in
            expected.into_iter().enumerate()
        {
            let ledger = run_monthly_production(
                &rules,
                &mut state,
                Date {
                    year: 768,
                    month: u8::try_from(index + 1).unwrap(),
                    day: 1,
                    hour: 12,
                },
            )
            .unwrap();
            assert_eq!(ledger.produced[oats], oats_output);
            assert_eq!(ledger.produced[fish], fish_output);
            assert_eq!(
                state.places[0].buildings[1].production_carries[0][oats],
                oats_carry
            );
            assert_eq!(
                state.places[0].buildings[1].production_carries[1][fish],
                fish_carry
            );
        }
    }

    #[test]
    fn full_building_staffing_preserves_each_method_output() {
        let (rules, mut state) = two_method_staffing_fixture(1000);
        state.places[0].buildings[1].levels = 3;
        state.places[0].buildings[1].method_levels = vec![2, 1];
        sync_levels(&rules, &mut state.places[0].buildings[1]);
        let oats = rules
            .goods
            .iter()
            .position(|good| good.id == "oats")
            .unwrap();
        state.places[0].buildings[1].method_suitability[0][oats] = 2000;
        set_holdings_staffing(&mut state, 3, 15, 2, 15);
        let fish = rules
            .goods
            .iter()
            .position(|good| good.id == "fish")
            .unwrap();
        let ledger = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();

        assert_eq!(ledger.produced[oats], 200);
        assert_eq!(ledger.produced[fish], 100);
    }

    #[test]
    fn coverage_efficiency_remains_building_wide_with_shared_staffing() {
        let (rules, mut state) = two_method_staffing_fixture(1200);
        set_holdings_staffing(&mut state, 1, 10, 0, 0);
        let oats = rules
            .goods
            .iter()
            .position(|good| good.id == "oats")
            .unwrap();
        let fish = rules
            .goods
            .iter()
            .position(|good| good.id == "fish")
            .unwrap();
        let ledger = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();

        assert_eq!(ledger.produced[oats], 45);
        assert_eq!(ledger.produced[fish], 45);
    }

    #[test]
    fn monthly_farm_output_scales_by_fertility_without_changing_the_reference_carry() {
        let run_year = |quality_pm: Option<u16>| {
            let mut rules = tiny_rules();
            rules.seasons.insert(
                "food".to_owned(),
                vec![83, 83, 83, 83, 83, 83, 83, 83, 83, 83, 83, 87],
            );
            rules.seasons.insert(
                "herd_births".to_owned(),
                vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            );
            let (mut state, _) = starting_state(
                &rules,
                &tiny_start_data(),
                &[StartLocation {
                    id: "farm".to_owned(),
                    kind: "ringfort".to_owned(),
                    size: 1,
                    land: LandCounts {
                        land_cells: BTreeMap::from([("pasture".to_owned(), 2)]),
                        fertility_pm: BTreeMap::from([("pasture".to_owned(), 2_000)]),
                        fertile_cells: 2,
                        fertile_cells_by_terrain: BTreeMap::new(),
                        sea: 0,
                    },
                }],
                10,
            )
            .unwrap();
            let qualities = quality_pm
                .map(|quality| BTreeMap::from([("farm".to_owned(), quality)]))
                .unwrap_or_default();
            let mut output = 0_u64;
            let mut final_carry = 0_u128;
            for month in 1..=12 {
                let ledger = if quality_pm.is_some() {
                    run_monthly_production_with_quality(
                        &rules,
                        &mut state,
                        Date {
                            year: 768,
                            month,
                            day: 1,
                            hour: 12,
                        },
                        &qualities,
                    )
                } else {
                    run_monthly_production(
                        &rules,
                        &mut state,
                        Date {
                            year: 768,
                            month,
                            day: 1,
                            hour: 12,
                        },
                    )
                }
                .unwrap();
                output += ledger.produced[0];
                final_carry = state.places[0].buildings[0].production_carries[0][0];
            }
            (output, final_carry)
        };
        let reference = run_year(None);
        assert_eq!(run_year(Some(550)), reference);
        assert_eq!(run_year(Some(1100)).0, reference.0 * 2);
        assert_eq!(run_year(Some(0)), (0, 0));
    }

    #[test]
    fn zero_fertility_suppresses_farm_output_but_keeps_fishing_unchanged() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        flatten_pasture(&mut rules);
        let mut reference = shipped_production_fixture(&rules);
        reference.places[0].buildings[1].method_levels = vec![1, 0];
        sync_levels(&rules, &mut reference.places[0].buildings[1]);
        let mut barren = reference.clone();
        let date = Date {
            year: 768,
            month: 9,
            day: 1,
            hour: 12,
        };
        let baseline = run_monthly_production(&rules, &mut reference, date).unwrap();
        let barren = run_monthly_production_with_quality(
            &rules,
            &mut barren,
            date,
            &BTreeMap::from([("fixture".to_owned(), 0)]),
        )
        .unwrap();
        let good_index = |id: &str| rules.goods.iter().position(|good| good.id == id).unwrap();
        assert_eq!(barren.produced[good_index("apples")], 0);
        assert!(baseline.produced[good_index("apples")] > 0);
        assert_eq!(
            barren.produced[good_index("fish")],
            baseline.produced[good_index("fish")]
        );
    }

    #[test]
    fn conservation_ledger_balances_goods_and_herds_after_monthly_systems() {
        let mut rules = tiny_rules();
        rules.goods[0].pop_limit_milli = 1_000_000_000;
        rules.seasons.insert(
            "food".to_owned(),
            vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        rules.seasons.insert(
            "herd_births".to_owned(),
            vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        install_fixture_herds(&mut rules);
        let (mut state, _) = starting_state(
            &rules,
            &tiny_start_data(),
            &[StartLocation {
                id: "fort".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 4)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), 4000)]),
                    fertile_cells: 4,
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 0,
                },
            }],
            10,
        )
        .unwrap();
        state.places[0].counters.current.births = 7;
        state.places[0].pops[0].counters.current.losses = 3;
        let date = Date {
            year: 768,
            month: 1,
            day: 1,
            hour: 12,
        };
        let ledger = run_monthly_production(&rules, &mut state, date).unwrap();
        let place = &ledger.places[0];
        assert_eq!(state.places[0].counters.last.births, 7);
        assert_eq!(
            state.places[0].counters.current,
            MonthlyCounters {
                losses: place.lost.iter().sum(),
                produced: place.produced.clone(),
                lost: place.lost.clone(),
                ..MonthlyCounters::for_goods(rules.goods.len())
            }
        );
        assert!(state.places[0].counters.closed);
        assert_eq!(state.places[0].pops[0].counters.last.losses, 3);
        for good in 0..rules.goods.len() {
            assert_eq!(
                place.stock_before[good] + place.produced[good],
                place.stock_after[good] + place.unpaid_wages[good] + place.lost[good],
                "good {} must balance",
                rules.goods[good].id
            );
        }
        for species in 0..3 {
            assert_eq!(
                place.herds_before[species] + place.herd_births[species],
                place.herds_after[species]
                    + place.herd_deaths[species]
                    + place.herd_slaughter[species],
                "herd index {species} must balance"
            );
        }
    }

    #[test]
    fn place_counter_rollover_values_goods_at_the_updated_local_price() {
        let mut counters = CounterBuckets::for_place(1);
        counters.current.produced[0] = 12;
        counters.current.used_by_pops[0] = 2;
        counters.current.lost[0] = 3;

        roll_counter_month(&mut counters, Some(&[96_500])).unwrap();

        assert!(counters.closed);
        assert_eq!(counters.last.produced, [12]);
        assert_eq!(counters.last.produced_value_milli, [1_158_000]);
        assert_eq!(counters.last.used_by_pops, [2]);
        assert_eq!(counters.last.used_by_pops_value_milli, [193_000]);
        assert_eq!(counters.last.lost, [3]);
        assert_eq!(counters.current, MonthlyCounters::for_goods(1));
    }

    #[test]
    fn first_of_month_rollover_uses_the_price_updated_by_that_days_market() {
        let rules = tiny_rules();
        let mut place = PlaceState {
            id: "fixture".to_owned(),
            pops: vec![PopState {
                kind: "peasant".to_owned(),
                culture: "connachta".to_owned(),
                religion: "christianity".to_owned(),
                literacy_pm: 0,
                workers: 1,
                dependents: 0,
                employment: Vec::new(),
                stock: vec![0],
                consumption: PopConsumption {
                    pantry_hunger_centi: vec![0],
                    ticking_carries: vec![0],
                    ..PopConsumption::default()
                },
                counters: CounterBuckets::default(),
                short_on_rent: false,
                housed_last: false,
                genetics: PopGenetics::default(),
                pending_births: 0,
                pending_deaths: 0,
            }],
            buildings: Vec::new(),
            prices_milli: vec![100_001],
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: vec![0],
            last_market_orders: None,
            counters: CounterBuckets::for_place(1),
        };
        place.counters.current.produced[0] = 12;
        let mut state = EconomyState {
            places: vec![place],
        };

        run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 3,
                day: 1,
                hour: 0,
            },
        )
        .unwrap();

        let place = &state.places[0];
        assert_eq!(
            place.prices_milli,
            [105_000],
            "the market moves price first"
        );
        assert_eq!(place.counters.last.produced, [12]);
        assert_eq!(place.counters.last.produced_value_milli, [1_260_000]);
    }

    #[test]
    fn mixed_peasant_cultures_are_split_by_authored_per_mille_shares() {
        let mut rules = tiny_rules();
        rules.peoples.cultures.push(fixture_culture("mide_brega"));
        let mut data = tiny_start_data();
        let defaults = data
            .defaults
            .get_mut("ringfort")
            .unwrap()
            .get_mut("1")
            .unwrap();
        defaults.peasants = 20;
        defaults.monks = 0;
        defaults.labourers = 0;
        data.places.get_mut("fort").unwrap().peasant_cultures_pm = Some(BTreeMap::from([
            ("connachta".to_owned(), 500),
            ("mide_brega".to_owned(), 500),
        ]));
        let (state, _) = starting_state(
            &rules,
            &data,
            &[StartLocation {
                id: "fort".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts::default(),
            }],
            10,
        )
        .unwrap();
        assert_eq!(
            state.places[0]
                .pops
                .iter()
                .map(|pop| {
                    (
                        pop.kind.as_str(),
                        pop.culture.as_str(),
                        pop.workers,
                        pop.dependents,
                    )
                })
                .collect::<Vec<_>>(),
            [
                ("peasant", "connachta", 6, 4),
                ("peasant", "mide_brega", 6, 4),
            ]
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "This regression verifies owner coverage and hiring order across every configured position pool in one shared scenario."
    )]
    fn holdings_owners_use_each_position_pools_ratio_from_rules() {
        let mut rules = tiny_rules();
        rules.pop_types.extend([
            PopType {
                id: "monk".to_owned(),
                dependent_pm: 100,
                births_pm_year: 0,
                worker_deaths_pm_year: 20,
                dependent_deaths_pm_year: 60,
                necessities_expectation_pm: 1000,
                niceties_expectation_pm: 0,
                luxuries_expectation_pm: 0,
                fuel_need_milli_day: 0,
            },
            PopType {
                id: "labourer".to_owned(),
                dependent_pm: 400,
                births_pm_year: 67,
                worker_deaths_pm_year: 20,
                dependent_deaths_pm_year: 60,
                necessities_expectation_pm: 1000,
                niceties_expectation_pm: 0,
                luxuries_expectation_pm: 0,
                fuel_need_milli_day: 0,
            },
        ]);
        add_empty_comfort_profile(&mut rules, "monk");
        add_empty_comfort_profile(&mut rules, "labourer");
        rules
            .starting_needs_pm
            .insert("monk".to_owned(), [1000, 0, 0]);
        rules
            .starting_needs_pm
            .insert("labourer".to_owned(), [1000, 0, 0]);
        for kind in ["monk", "labourer"] {
            rules
                .diet_profiles
                .insert(kind.to_owned(), BTreeMap::from([("food".to_owned(), 1000)]));
        }
        rules.buildings.insert(
            1,
            BuildingType {
                id: "holdings".to_owned(),
                church: false,
                herds: BTreeMap::new(),
                workers_per_level: 10,
                land_per_level: 2,
                owner_workers: BTreeMap::from([
                    ("monk".to_owned(), 5),
                    ("labourer".to_owned(), 10),
                ]),
                penalty_pm: 500,
                overhead_pm: 300,
                recruit_range_m: Some(100_000),
                market_capacity_ml_per_level: 0,
                methods: vec![rules.buildings[0].methods[0].clone()],
                owner_kind: None,
            },
        );
        rules.footprint_m2.insert("holdings".to_owned(), 4000);
        rules.housing_per_level.insert("holdings".to_owned(), 14);
        rules
            .housing_class
            .insert("holdings".to_owned(), "basic".to_owned());
        let mut data = tiny_start_data();
        let defaults = data
            .defaults
            .get_mut("ringfort")
            .unwrap()
            .get_mut("1")
            .unwrap();
        defaults.monks = 30;
        defaults.labourers = 25;
        defaults.holdings_levels = 3;
        defaults.farm_levels = 0;
        defaults.storehouse_levels = 1;
        let (state, _) = starting_state(
            &rules,
            &data,
            &[StartLocation {
                id: "fort".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 6)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), 6000)]),
                    fertile_cells: 6,
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 0,
                },
            }],
            10,
        )
        .unwrap();
        let holdings = &state.places[0].buildings[0];
        assert_eq!(holdings.levels, 3);
        assert_eq!(holdings.employment[1].owners, 5);
        assert_eq!(holdings.employment[1].workers, 15);
        assert_eq!(holdings.employment[2].workers, 15);
        let storehouse = &state.places[0].buildings[1];
        assert_eq!(storehouse.kind, "storehouse");
        assert_eq!(
            storehouse.employment[0].workers, 0,
            "non-owner peasants do not staff it"
        );
        assert_eq!(storehouse.employment[1].workers, 2, "monk owners staff it");
        assert_eq!(
            storehouse.employment[2].workers, 0,
            "non-owner labourers do not staff it"
        );
    }

    #[test]
    fn herd_yield_estimates_use_the_starting_herd_instead_of_crop_suitability() {
        let mut rules = tiny_rules();
        install_fixture_herds(&mut rules);
        let method = &rules.buildings[0].methods[0];
        assert_eq!(
            starting_method_output(
                &rules,
                &rules.buildings[0],
                method,
                "beef",
                1100,
                Some("pasture"),
                550
            ),
            Ok(825)
        );
        assert_eq!(
            starting_method_output(
                &rules,
                &rules.buildings[0],
                method,
                "food",
                300,
                Some("pasture"),
                550
            ),
            Ok(300)
        );
        assert_eq!(
            starting_method_output(
                &rules,
                &rules.buildings[0],
                method,
                "food",
                300,
                Some("pasture"),
                275
            ),
            Ok(150)
        );
        assert!(
            starting_method_output(
                &rules,
                &rules.buildings[0],
                method,
                "food",
                300,
                Some("pasture"),
                275
            )
            .unwrap()
                < starting_method_output(
                    &rules,
                    &rules.buildings[0],
                    method,
                    "food",
                    300,
                    Some("pasture"),
                    550
                )
                .unwrap()
        );
    }

    #[test]
    fn diet_profiles_scale_to_a_thousand_in_good_order_with_fish_kept() {
        let mut rules = tiny_rules();
        rules.goods.extend([
            Good {
                id: "grain_2".to_owned(),
                tags: Vec::new(),
                supplies: BTreeMap::new(),
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
                stacked_outside: false,
                hunger_centi: 100,
                necessities_centi: 100,
                niceties_centi: 0,
                luxuries_centi: 0,
                base_price: 100,
                pop_limit_milli: 1000,
            },
            Good {
                id: "fish".to_owned(),
                tags: Vec::new(),
                supplies: BTreeMap::new(),
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
                stacked_outside: false,
                hunger_centi: 100,
                necessities_centi: 100,
                niceties_centi: 0,
                luxuries_centi: 0,
                base_price: 100,
                pop_limit_milli: 1000,
            },
        ]);
        rules.diet_profiles.get_mut("peasant").unwrap().extend([
            ("food".to_owned(), 1),
            ("grain_2".to_owned(), 1),
            ("fish".to_owned(), 1),
        ]);
        let profile =
            normalized_diet_shares(&rules, rules.diet_profiles.get("peasant").unwrap()).unwrap();
        assert_eq!(profile, [334, 333, 333]);
    }

    #[test]
    fn shipped_monk_meat_profile_gives_the_remainder_to_beef() {
        let rules = Rules::load(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"),
        )
        .unwrap();
        let monk = &rules.diet_profiles["monk"];
        assert_eq!([monk["beef"], monk["pork"], monk["mutton"]], [18, 16, 16]);
    }

    #[test]
    fn projected_buy_units_pins_a_literal_diet_profile_projection() {
        let rules = tiny_rules();
        let pop = PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 12,
            dependents: 8,
            employment: Vec::new(),
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        assert_eq!(
            projected_buy_units(&rules, std::slice::from_ref(&pop), "food"),
            Ok(7300)
        );
    }

    #[test]
    fn authored_building_levels_are_cut_when_no_resource_method_fits() {
        let rules = tiny_rules();
        let (state, report) = starting_state(
            &rules,
            &tiny_start_data(),
            &[StartLocation {
                id: "island".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts::default(),
            }],
            10,
        )
        .unwrap();
        assert_eq!(
            report,
            ["island farm: cut levels 1 -> 0 (no method has enough land or sea)"]
        );
        assert_eq!(
            state.places[0].buildings[0].levels, 0,
            "the farm was cut to its zero land maximum"
        );
        assert_eq!(
            state.places[0].buildings[1].levels, 1,
            "the storehouse's authored level remains"
        );
    }

    #[test]
    fn terrain_score_beats_palette_order_and_ties_keep_palette_order() {
        let mut rules = tiny_rules();
        rules.terrain_order.push("forest".to_owned());
        rules.suitability.insert(
            "pasture".to_owned(),
            BTreeMap::from([("food".to_owned(), 100), ("crops".to_owned(), 100)]),
        );
        rules.suitability.insert(
            "forest".to_owned(),
            BTreeMap::from([("food".to_owned(), 900), ("crops".to_owned(), 900)]),
        );
        let method = &rules.buildings[0].methods[0];
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".to_owned(), 2), ("forest".to_owned(), 2)]),
            fertility_pm: BTreeMap::from([
                ("pasture".to_owned(), 2000),
                ("forest".to_owned(), 2000),
            ]),
            fertile_cells: 4,
            fertile_cells_by_terrain: BTreeMap::new(),
            sea: 0,
        };
        assert_eq!(
            choose_terrain(&rules, method, &land, 2000).as_deref(),
            Some("forest")
        );

        rules
            .suitability
            .get_mut("forest")
            .unwrap()
            .insert("food".to_owned(), 100);
        rules
            .suitability
            .get_mut("forest")
            .unwrap()
            .insert("crops".to_owned(), 100);
        assert_eq!(
            choose_terrain(&rules, method, &land, 2000).as_deref(),
            Some("pasture")
        );
    }

    #[test]
    fn field_room_refuses_1999_and_consumes_exactly_2000_fertility() {
        let rules = tiny_rules();
        let building = &rules.buildings[0];
        let mut remaining_sea = 0;
        let mut just_short = LandCounts {
            land_cells: BTreeMap::from([("pasture".to_owned(), 2)]),
            fertility_pm: BTreeMap::from([("pasture".to_owned(), 1999)]),
            fertile_cells: 2,
            fertile_cells_by_terrain: BTreeMap::new(),
            sea: 0,
        };
        let refused = assign_building_levels(
            &rules,
            building,
            1,
            &mut just_short,
            &mut remaining_sea,
            &mut StartDemand::new(&rules, &[]).unwrap(),
            550,
        )
        .unwrap();
        assert_eq!(refused.method_levels, [0]);
        assert_eq!(just_short.fertility_pm["pasture"], 1999);

        let mut exact_room = LandCounts {
            land_cells: BTreeMap::from([("pasture".to_owned(), 2)]),
            fertility_pm: BTreeMap::from([("pasture".to_owned(), 2000)]),
            fertile_cells: 2,
            fertile_cells_by_terrain: BTreeMap::new(),
            sea: 0,
        };
        let accepted = assign_building_levels(
            &rules,
            building,
            1,
            &mut exact_room,
            &mut remaining_sea,
            &mut StartDemand::new(&rules, &[]).unwrap(),
            550,
        )
        .unwrap();
        assert_eq!(accepted.method_levels, [1]);
        assert_eq!(exact_room.fertility_pm["pasture"], 0);
    }

    #[test]
    fn coastal_methods_need_nearby_sea_but_one_sea_cell_allows_many_levels() {
        let mut rules = tiny_rules();
        rules.seasons.insert(
            "fish".to_owned(),
            vec![0, 0, 0, 0, 0, 0, 0, 0, 1000, 0, 0, 0],
        );
        rules.goods.push(Good {
            id: "fish".to_owned(),
            tags: Vec::new(),
            supplies: BTreeMap::new(),
            unit: "pound".to_owned(),
            weight_g: 1,
            volume_ml: 1,
            stacked_outside: false,
            hunger_centi: 100,
            necessities_centi: 100,
            niceties_centi: 0,
            luxuries_centi: 0,
            base_price: 100,
            pop_limit_milli: 1000,
        });
        rules
            .diet_profiles
            .get_mut("peasant")
            .unwrap()
            .insert("food".to_owned(), 1000);
        rules.buildings[0].methods.push(Method {
            id: "coastal".to_owned(),
            resource: "sea".to_owned(),
            cells_per_level: None,
            annual_output: BTreeMap::from([("fish".to_owned(), 1000)]),
            suitability: "sea".to_owned(),
            score_share_pm: BTreeMap::from([("fish".to_owned(), 1000)]),
            serves: "food".to_owned(),
        });
        let mut data = tiny_start_data();
        data.defaults
            .get_mut("ringfort")
            .unwrap()
            .get_mut("1")
            .unwrap()
            .farm_levels = 3;
        let (no_sea, _) = starting_state(
            &rules,
            &data,
            &[StartLocation {
                id: "inland".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 2)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), 2000)]),
                    fertile_cells: 2,
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 0,
                },
            }],
            10,
        )
        .unwrap();
        assert_eq!(no_sea.places[0].buildings[0].method_levels[1], 0);
        let (state, report) = starting_state(
            &rules,
            &data,
            &[StartLocation {
                id: "coast".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 2)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), 2000)]),
                    fertile_cells: 2,
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 1,
                },
            }],
            10,
        )
        .unwrap();
        assert!(
            report.is_empty(),
            "one sea cell must not cap coastal levels: {report:?}"
        );
        let farm = &state.places[0].buildings[0];
        // Field room is one level, farming first by method order; the fish and the crops both
        // feed the unmet food need, so the first level farms and the two after it fish.
        assert_eq!(
            farm.method_levels,
            [1, 2],
            "fishing levels are not capped by sea/4"
        );
        assert_eq!(farm.levels, 3);
    }

    #[test]
    fn fuel_methods_respect_terrain_room_without_using_fields() {
        let rules = shipped_rules();
        let farm = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        let mut land = LandCounts {
            land_cells: BTreeMap::from([("forest".into(), 9), ("bog".into(), 5)]),
            fertility_pm: BTreeMap::new(),
            ..LandCounts::default()
        };
        let result = assign_building_levels(
            &rules,
            farm,
            5,
            &mut land,
            &mut 0,
            &mut StartDemand::fixture(&rules, 0, 100_000_000),
            1000,
        )
        .unwrap();
        assert_eq!(result.method_levels, [0, 0, 2, 2]);
        assert_eq!(result.used_land, 0);
        assert!(land.fertility_pm.is_empty());
        let mut short = LandCounts {
            land_cells: BTreeMap::from([("forest".into(), 3)]),
            ..LandCounts::default()
        };
        let result = assign_building_levels(
            &rules,
            farm,
            1,
            &mut short,
            &mut 0,
            &mut StartDemand::fixture(&rules, 0, 100_000_000),
            550,
        )
        .unwrap();
        assert_eq!(result.method_levels, [0, 0, 0, 0]);
    }

    #[test]
    fn fuel_levels_contribute_to_the_emergency_breeding_floor() {
        let mut rules = shipped_rules();
        for levels in [vec![0, 0, 1, 1], vec![1, 0, 1, 0]] {
            flatten_pasture(&mut rules);
            let mut state = shipped_production_fixture(&rules);
            let place = &mut state.places[0];
            place.pops[0].consumption.necessities_pm = 400;
            place.pops[0].consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
            place.buildings[0].method_levels = levels;
            place.buildings[0].herds = vec![4, 4, 4];
            let mut ledger = DailyConsumptionLedger {
                butchered: vec![0; rules.goods.len()],
                lost: vec![0; rules.goods.len()],
                herd_slaughter: vec![0; 3],
                ..DailyConsumptionLedger::default()
            };
            butcher_emergency(&rules, place, 0, 1_000_000, &mut ledger).unwrap();
            assert_eq!(place.buildings[0].herds, [4, 4, 4]);
            assert_eq!(ledger.herd_slaughter, [0, 0, 0]);
        }
    }

    #[test]
    fn opening_fuel_levels_follow_need_and_terrain_room() {
        let rules = shipped_rules();
        let farm = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        let land = LandCounts {
            land_cells: BTreeMap::from([
                ("forest".into(), 8),
                ("bog".into(), 4),
                ("pasture".into(), 8),
            ]),
            fertility_pm: BTreeMap::from([("pasture".into(), 8000)]),
            ..LandCounts::default()
        };
        let candidates = level_candidates(&rules, farm, &land, 0, 550, &[0; 4]).unwrap();
        for (candidate, food, fuel) in [
            (0, 673_270, 0),
            (1, 172_270, 3_000_000),
            (2, 172_270, 2_800_000),
        ] {
            let output = &candidates[candidate].output;
            let food_actual: u128 = output
                .iter()
                .map(|(index, units)| units * u128::from(rules.goods[*index].hunger_centi))
                .sum();
            let fuel_actual: u128 = output
                .iter()
                .map(|(index, units)| rules.goods[*index].category_supplied("fuel", *units))
                .sum();
            assert_eq!((food_actual, fuel_actual), (food, fuel));
        }
        for (levels, expected) in [
            (1, [1, 0, 0, 0]),
            (2, [1, 0, 1, 0]),
            (3, [2, 0, 1, 0]),
            (4, [2, 0, 1, 1]),
        ] {
            let result = assign_building_levels(
                &rules,
                farm,
                levels,
                &mut land.clone(),
                &mut 0,
                &mut StartDemand::fixture(&rules, 1_825_000, 6_205_000),
                550,
            )
            .unwrap();
            assert_eq!(result.method_levels, expected);
        }
        for (food, fuel, expected) in [
            (1_825_000, 6_205_000, vec![2, 0, 1, 1]),
            (1_825_000, 0, vec![4, 0, 0, 0]),
        ] {
            let result = assign_building_levels(
                &rules,
                farm,
                4,
                &mut land.clone(),
                &mut 0,
                &mut StartDemand::fixture(&rules, food, fuel),
                550,
            )
            .unwrap();
            assert_eq!(result.method_levels, expected);
        }
        let mut limited = land.clone();
        limited.fertility_pm.insert("pasture".into(), 4000);
        let result = assign_building_levels(
            &rules,
            farm,
            4,
            &mut limited,
            &mut 0,
            &mut StartDemand::fixture(&rules, 1_825_000, 0),
            550,
        )
        .unwrap();
        assert_eq!(result.method_levels, [2, 0, 0, 0]);
        assert_eq!(result.cut_for_room, Some(false));
    }

    #[test]
    fn storehouse_refuses_both_fuel_goods() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut building = shipped_production_fixture(&rules)
            .places
            .remove(0)
            .buildings
            .remove(0);
        let before = building.stock.clone();
        for id in ["hardwood", "peat"] {
            assert_eq!(
                receive_storehouse(&rules, &mut building, None, good_at(&rules, id), 3).unwrap(),
                3
            );
            assert_eq!(building.stock, before);
        }
    }

    #[test]
    fn fuel_orders_choose_cheapest_per_unit_and_preserve_fractional_target() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.pops.truncate(1);
        place.buildings.clear();
        place.prices_milli = rules.goods.iter().map(|g| g.base_price * 1000).collect();
        let hardwood = good_at(&rules, "hardwood");
        let peat = good_at(&rules, "peat");
        let pop = &mut place.pops[0];
        pop.workers = 100;
        pop.dependents = 0;
        pop.stock[hardwood] = 300;
        pop.stock[peat] = 100;
        let orders = daily_market_orders(&rules, place).unwrap();
        assert_eq!(
            (orders.buy_units[hardwood], orders.buy_units[peat]),
            (0, 158)
        );
        place.prices_milli[peat] = 8000;
        let orders = daily_market_orders(&rules, place).unwrap();
        assert_eq!(
            (orders.buy_units[hardwood], orders.buy_units[peat]),
            (110, 0)
        );
    }

    #[test]
    fn fuel_burning_carries_balance_and_never_repays_missed_days() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.pops.truncate(1);
        place.buildings.clear();
        place.prices_milli = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        let hardwood = good_at(&rules, "hardwood");
        let pop = &mut place.pops[0];
        pop.workers = 30;
        pop.dependents = 0;
        let prices = place.prices_milli.clone();
        let mut ledger = DailyConsumptionLedger {
            opened: vec![0; rules.goods.len()],
            ..Default::default()
        };
        for _ in 0..5 {
            fuel::draw_and_burn(&rules, place, 0, &prices, &mut ledger).unwrap();
        }
        place.pops[0].stock[hardwood] = 100;
        for _ in 0..31 {
            fuel::draw_and_burn(&rules, place, 0, &prices, &mut ledger).unwrap();
        }
        let consumption = &place.pops[0].consumption;
        assert_eq!(
            (
                consumption.fuel_need_month,
                consumption.fuel_burned_month,
                consumption.hearth_milli,
                ledger.opened[hardwood]
            ),
            (21_600, 18_600, 400, 19)
        );
    }

    #[test]
    fn noon_burn_uses_the_price_before_the_price_step() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.pops.truncate(1);
        place.buildings.clear();
        place.pops[0].employment.clear();
        place.pops[0].workers = 1;
        place.pops[0].dependents = 0;
        place.pops[0].consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
        place.pops[0].consumption.ticking_carries = vec![0; rules.goods.len()];
        let wood = good_at(&rules, "hardwood");
        let peat = good_at(&rules, "peat");
        place.pops[0].stock[wood] = 1;
        place.pops[0].stock[peat] = 1;
        place.prices_milli = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        place.prices_milli[peat] = 6999;
        place.price_drift_carries = vec![0; rules.goods.len()];
        let orders = daily_market_orders(&rules, place).unwrap();
        assert_eq!(orders.buy_units[peat], 5);
        let ledger = run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        let place = &state.places[0];
        assert_eq!(place.prices_milli[peat], 7199);
        assert_eq!(place.pops[0].consumption.hearth_milli, 680);
        assert_eq!(
            (place.pops[0].stock[wood], place.pops[0].stock[peat]),
            (1, 0)
        );
        assert_eq!(ledger.daily_consumption.unwrap().opened[peat], 1);
    }

    #[test]
    fn noon_draw_uses_the_opening_price_and_respects_owner_reserve() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.pops.truncate(1);
        place.buildings.truncate(1);
        place.pops[0].employment.truncate(1);
        place.buildings[0].employment.truncate(1);
        place.pops[0].workers = 1;
        place.pops[0].dependents = 0;
        place.pops[0].consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
        place.pops[0].consumption.ticking_carries = vec![0; rules.goods.len()];
        place.pops[0].consumption.hearth_milli = 500;
        let wood = good_at(&rules, "hardwood");
        let peat = good_at(&rules, "peat");
        place.buildings[0].stock[wood] = 1;
        place.buildings[0].stock[peat] = 1;
        place.prices_milli = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        place.prices_milli[peat] = 6999;
        place.price_drift_carries = vec![0; rules.goods.len()];
        let orders = daily_market_orders(&rules, place).unwrap();
        assert_eq!(
            (
                orders.buy_units[peat],
                orders.sell_units[wood],
                orders.sell_units[peat]
            ),
            (7, 0, 0)
        );
        run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        let place = &state.places[0];
        assert_eq!(place.prices_milli[peat], 7199);
        assert_eq!(place.pops[0].consumption.hearth_milli, 480);
        assert_eq!(
            (
                place.pops[0].stock[peat],
                place.buildings[0].stock[wood],
                place.buildings[0].stock[peat]
            ),
            (1, 1, 0)
        );
    }

    #[test]
    fn fuel_draw_uses_building_order_and_obeys_the_pop_cap() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.pops.truncate(1);
        place.buildings.truncate(1);
        place.pops[0].workers = 1;
        place.pops[0].dependents = 0;
        place.buildings[0].employment.truncate(1);
        let wood = good_at(&rules, "hardwood");
        let peat = good_at(&rules, "peat");
        place.buildings[0].stock[wood] = 2;
        let mut second = place.buildings[0].clone();
        second.stock[wood] = 0;
        second.stock[peat] = 2;
        place.buildings.push(second);
        let prices: Vec<_> = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        let mut ledger = DailyConsumptionLedger {
            opened: vec![0; rules.goods.len()],
            ..Default::default()
        };
        fuel::draw_and_burn(&rules, place, 0, &prices, &mut ledger).unwrap();
        assert_eq!(
            (
                place.pops[0].stock[wood],
                place.buildings[0].stock[wood],
                place.buildings[1].stock[peat]
            ),
            (1, 0, 2)
        );
        assert_eq!(place.pops[0].consumption.hearth_milli, 980);
        place.pops[0].consumption.hearth_milli = 0;
        place.pops[0].stock[wood] = 30;
        place.pops[0].stock[peat] = 0;
        place.buildings[0].stock[wood] = 2;
        place.buildings[1].stock[peat] = 0;
        rules
            .pop_types
            .iter_mut()
            .find(|kind| kind.id == "peasant")
            .unwrap()
            .fuel_need_milli_day = 1000;
        fuel::draw_and_burn(&rules, place, 0, &prices, &mut ledger).unwrap();
        assert_eq!(place.buildings[0].stock[wood], 2);
    }

    #[test]
    fn fuel_start_allocates_by_coverage_and_shares_available_methods() {
        let rules = shipped_rules();
        let farm = rules
            .buildings
            .iter()
            .find(|building| building.id == "farm")
            .unwrap();
        let land = |forest, bog| LandCounts {
            land_cells: BTreeMap::from([
                ("pasture".into(), 52),
                ("forest".into(), forest),
                ("bog".into(), bog),
            ]),
            fertility_pm: BTreeMap::from([("pasture".into(), 52_000)]),
            ..LandCounts::default()
        };
        for (forest, bog, expected) in [
            (8, 0, [24, 0, 2, 0]),
            (8, 4, [24, 0, 1, 1]),
            (0, 0, [26, 0, 0, 0]),
        ] {
            let mut demand = StartDemand::fixture(&rules, 18_250_000, 3_650_000);
            let result = assign_building_levels(
                &rules,
                farm,
                26,
                &mut land(forest, bog),
                &mut 0,
                &mut demand,
                550,
            )
            .unwrap();
            assert_eq!(result.method_levels, expected);
        }
        for (levels, expected) in [
            (1, [1, 0, 0, 0]),
            (2, [1, 0, 1, 0]),
            (24, [23, 0, 1, 0]),
            (25, [23, 0, 1, 1]),
            (26, [24, 0, 1, 1]),
        ] {
            let result = assign_building_levels(
                &rules,
                farm,
                levels,
                &mut land(8, 4),
                &mut 0,
                &mut StartDemand::fixture(&rules, 18_250_000, 3_650_000),
                550,
            )
            .unwrap();
            assert_eq!(result.method_levels, expected);
        }
    }

    #[test]
    fn one_person_starts_with_five_wood_or_seven_peat_loads() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let original = shipped_production_fixture(&rules);
        let wood = good_at(&rules, "hardwood");
        let peat = good_at(&rules, "peat");
        for (levels, expected) in [
            (vec![0, 0, 1, 0], (5, 0)),
            (vec![0, 0, 0, 1], (0, 7)),
            (vec![0, 0, 1, 1], (5, 0)),
            (vec![1, 0, 0, 0], (0, 0)),
        ] {
            let mut place = original.places[0].clone();
            place.pops.truncate(1);
            place.pops[0].workers = 1;
            place.pops[0].dependents = 0;
            place.buildings[0].method_levels = levels;
            fuel::seed_starting_fuel(&rules, &mut place.pops, &place.buildings).unwrap();
            assert_eq!(
                (place.pops[0].stock[wood], place.pops[0].stock[peat]),
                expected
            );
        }
    }

    #[test]
    fn fuel_burn_on_first_closes_the_previous_month_window() {
        let mut rules = shipped_rules();
        for kind in &mut rules.pop_types {
            kind.births_pm_year = 0;
            kind.worker_deaths_pm_year = 0;
            kind.dependent_deaths_pm_year = 0;
        }
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.pops.truncate(1);
        place.buildings.clear();
        place.pops[0].employment.clear();
        place.pops[0].workers = 0;
        place.pops[0].dependents = 1;
        place.pops[0].consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
        place.pops[0].consumption.ticking_carries = vec![0; rules.goods.len()];
        place.pops[0].consumption.hearth_milli = 20;
        place.pops[0].consumption.fuel_need_month = 20;
        place.pops[0].consumption.fuel_burned_month = 0;
        place.prices_milli = rules.goods.iter().map(|g| g.base_price * 1000).collect();
        place.price_drift_carries = vec![0; rules.goods.len()];
        run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!(
            (
                pop.consumption.fuel_need_month,
                pop.consumption.fuel_burned_month
            ),
            (0, 0)
        );
        assert_eq!(pop.consumption.fuel_death_carry, 50_000);
    }

    #[test]
    fn saved_fuel_state_refuses_each_invalid_field_with_pop_location() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        assert!(validate_fuel_state(&rules, &state).is_ok());
        state.places[0].pops[0].consumption.hearth_milli = 1000;
        let error = validate_fuel_state(&rules, &state).unwrap_err();
        assert!(error.entry.contains("pops.0.fuel.hearth"));
        assert!(error.problem.contains("1000"));
        state.places[0].pops[0].consumption.hearth_milli = 999;
        state.places[0].pops[0].consumption.fuel_burned_month = 2;
        state.places[0].pops[0].consumption.fuel_need_month = 1;
        assert!(
            validate_fuel_state(&rules, &state)
                .unwrap_err()
                .entry
                .contains("burned")
        );
        state.places[0].pops[0].consumption.fuel_burned_month = 1;
        state.places[0].pops[0].consumption.fuel_death_carry = 12_000_000;
        assert!(
            validate_fuel_state(&rules, &state)
                .unwrap_err()
                .entry
                .contains("death_carry")
        );
    }

    #[test]
    fn fuel_burns_two_windows_with_exact_remainders() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.pops.truncate(1);
        place.buildings.clear();
        let wood = good_at(&rules, "hardwood");
        place.pops[0].workers = 30;
        place.pops[0].dependents = 0;
        place.pops[0].stock[wood] = 100;
        let prices: Vec<_> = rules.goods.iter().map(|g| g.base_price * 1000).collect();
        let mut ledger = DailyConsumptionLedger {
            opened: vec![0; rules.goods.len()],
            ..Default::default()
        };
        for _ in 0..31 {
            fuel::draw_and_burn(&rules, place, 0, &prices, &mut ledger).unwrap();
        }
        let c = &mut place.pops[0].consumption;
        assert_eq!(
            (
                c.fuel_need_month,
                c.fuel_burned_month,
                c.hearth_milli,
                ledger.opened[wood]
            ),
            (18_600, 18_600, 400, 19)
        );
        c.fuel_need_month = 0;
        c.fuel_burned_month = 0;
        ledger.opened[wood] = 0;
        for _ in 0..28 {
            fuel::draw_and_burn(&rules, place, 0, &prices, &mut ledger).unwrap();
        }
        let c = &place.pops[0].consumption;
        assert_eq!(
            (
                c.fuel_need_month,
                c.fuel_burned_month,
                c.hearth_milli,
                ledger.opened[wood]
            ),
            (16_800, 16_800, 600, 17)
        );
    }

    #[test]
    fn fuel_death_and_ordinary_death_are_capped_together() {
        let mut rules = accounting_rules(0, 0);
        rules.pop_types[0].dependent_pm = 500;
        rules.pop_types[0].dependent_deaths_pm_year = 1;
        rules.fuel_deaths_pm_year = 1;
        let mut state = accounting_state(1, 1);
        let pop = &mut state.places[0].pops[0];
        pop.consumption.fuel_need_month = 1000;
        pop.consumption.fuel_burned_month = 0;
        pop.consumption.fuel_death_carry = 11_999_000;
        pop.consumption.dependent_death_carry = 11_999_000_000_000_000;
        run_monthly_demography(&rules, &mut state, &BTreeMap::new(), &BTreeMap::new(), None)
            .unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!(pop.workers + pop.dependents, 1);
        assert_eq!(pop.counters.current.deaths, 1);
    }

    #[test]
    fn fuel_orders_follow_price_and_abundant_fuels_settle_apart() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut initial = shipped_production_fixture(&rules);
        let place = &mut initial.places[0];
        place.pops.truncate(1);
        place.pops[0].workers = 1;
        place.pops[0].dependents = 0;
        place.pops[0].employment.truncate(1);
        place.buildings.truncate(1);
        place.buildings[0].employment.truncate(1);
        place.prices_milli = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        place.price_drift_carries = vec![0; rules.goods.len()];
        let wood = good_at(&rules, "hardwood");
        let peat = good_at(&rules, "peat");
        place.buildings[0].stock[wood] = 100;
        let mut only_wood = initial.clone();
        assert!(
            daily_market_orders(&rules, &only_wood.places[0])
                .unwrap()
                .buy_units[peat]
                > 0
        );
        for _ in 0..1000 {
            update_daily_prices(&rules, &mut only_wood).unwrap();
        }
        let order = daily_market_orders(&rules, &only_wood.places[0]).unwrap();
        assert!(order.buy_units[wood] > 0);
        assert_eq!(order.buy_units[peat], 0);
        initial.places[0].buildings[0].stock[peat] = 100;
        for _ in 0..1000 {
            update_daily_prices(&rules, &mut initial).unwrap();
        }
        let place = &initial.places[0];
        let order = daily_market_orders(&rules, place).unwrap();
        assert_eq!((order.buy_units[wood], order.buy_units[peat]), (0, 7));
        assert_eq!(
            (place.prices_milli[wood], place.prices_milli[peat]),
            (5000, 3399)
        );
    }

    #[test]
    fn combined_fuel_deaths_respect_pending_deaths_and_living_floor() {
        let mut rules = accounting_rules(0, 0);
        rules.pop_types[0].dependent_pm = 500;
        rules.pop_types[0].dependent_deaths_pm_year = 12_000;
        rules.fuel_deaths_pm_year = 12_000;
        let mut state = accounting_state(1, 2);
        let pop = &mut state.places[0].pops[0];
        pop.pending_deaths = 1;
        pop.consumption.fuel_need_month = 1000;
        let floors = BTreeMap::from([(("owner_reserve".to_owned(), 0), 2)]);
        run_monthly_demography_with_floors(
            &rules,
            &mut state,
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            &floors,
        )
        .unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!(
            (pop.workers, pop.dependents, pop.counters.current.deaths),
            (1, 1, 2)
        );
    }

    #[test]
    fn fuel_owner_reserve_spares_whole_loads_at_mixed_boundary() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.pops.truncate(1);
        place.buildings.truncate(1);
        place.pops[0].workers = 1;
        place.pops[0].dependents = 0;
        place.pops[0].employment.truncate(1);
        place.buildings[0].employment.truncate(1);
        place.prices_milli = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        let wood = good_at(&rules, "hardwood");
        let peat = good_at(&rules, "peat");
        place.buildings[0].stock[wood] = 2;
        place.buildings[0].stock[peat] = 1;
        let orders = daily_market_orders(&rules, place).unwrap();
        assert_eq!((orders.sell_units[wood], orders.sell_units[peat]), (1, 0));
        place.buildings[0].stock[wood] = 1;
        let orders = daily_market_orders(&rules, place).unwrap();
        assert_eq!((orders.sell_units[wood], orders.sell_units[peat]), (0, 0));
    }

    #[test]
    fn fuel_need_is_in_the_start_projection_but_live_orders_buy_peat() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.pops.truncate(1);
        place.buildings.clear();
        place.pops[0].workers = 500;
        place.pops[0].dependents = 0;
        place.pops[0].employment.clear();
        place.prices_milli = rules
            .goods
            .iter()
            .map(|good| good.base_price * 1000)
            .collect();
        let wood = good_at(&rules, "hardwood");
        let peat = good_at(&rules, "peat");
        let demand = StartDemand::new(&rules, &place.pops).unwrap();
        assert_eq!(
            (demand.food_need(), demand.fuel_need()),
            (18_250_000, 3_650_000)
        );
        assert_eq!((demand.orders()[wood], demand.orders()[peat]), (0, 0));
        assert_eq!(
            fuel::fuel_daily_need(&rules, &place.pops[0]).unwrap(),
            10_000
        );
        let orders = daily_market_orders(&rules, place).unwrap();
        assert_eq!((orders.buy_units[wood], orders.buy_units[peat]), (0, 3429));
    }

    #[test]
    fn fuel_method_data_refuses_missing_zero_and_misplaced_cell_limits() {
        for (index, cells) in [
            (2, None),
            (2, Some(0)),
            (3, None),
            (3, Some(0)),
            (0, Some(1)),
            (1, Some(1)),
        ] {
            let mut rules = shipped_rules();
            rules.buildings[0].methods[index].cells_per_level = cells;
            let error = rules.validate("fixture").unwrap_err();
            assert!(error.entry.contains("cells_per_level"), "{error}");
        }
    }

    #[test]
    fn fuel_output_and_estimates_ignore_field_quality() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let farm = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        for (method_index, terrain, good, monthly) in
            [(2, "forest", "hardwood", 750), (3, "bog", "peat", 2000)]
        {
            let method = &farm.methods[method_index];
            assert_eq!(
                starting_method_output(
                    &rules,
                    farm,
                    method,
                    good,
                    method.annual_output[good],
                    Some(terrain),
                    0
                )
                .unwrap(),
                starting_method_output(
                    &rules,
                    farm,
                    method,
                    good,
                    method.annual_output[good],
                    Some(terrain),
                    1000
                )
                .unwrap()
            );
            for quality in [0, 1000] {
                let mut land = LandCounts {
                    land_cells: BTreeMap::from([(
                        terrain.to_owned(),
                        method.cells_per_level.unwrap(),
                    )]),
                    ..LandCounts::default()
                };
                let assignment = assign_building_levels(
                    &rules,
                    farm,
                    1,
                    &mut land,
                    &mut 0,
                    &mut StartDemand::fixture(&rules, 0, 100_000_000),
                    quality,
                )
                .unwrap();
                assert_eq!(assignment.method_levels[method_index], 1);
                assert_eq!(assignment.output[good], method.annual_output[good]);
                let mut state = shipped_production_fixture(&rules);
                let building = &mut state.places[0].buildings[0];
                building.method_levels = assignment.method_levels;
                building.level_records = assignment.level_records;
                building.method_suitability = assignment.method_suitability;
                building.herd_grazing_sums = assignment.herd_grazing_sums;
                for month in 1..=12 {
                    let ledger = run_monthly_production_with_quality(
                        &rules,
                        &mut state,
                        Date {
                            year: 768,
                            month,
                            day: 1,
                            hour: 12,
                        },
                        &BTreeMap::from([("fixture".to_owned(), quality)]),
                    )
                    .unwrap();
                    let cutting = if method_index == 2 {
                        [11, 12, 1, 2].contains(&month)
                    } else {
                        [8, 9].contains(&month)
                    };
                    assert_eq!(
                        ledger.produced[good_at(&rules, good)],
                        if cutting { monthly } else { 0 }
                    );
                    assert_eq!(ledger.produced[good_at(&rules, "oats")], 0);
                    if month == 6 {
                        assert!(!state.places[0].buildings[0].herds.is_empty());
                        assert!(ledger.produced[good_at(&rules, "dairy")] > 0);
                    }
                }
            }
        }
    }

    #[test]
    fn shipped_fuel_goods_have_the_authored_properties() {
        let rules = shipped_rules();
        for (id, tags, supply, volume, price, cap) in [
            ("hardwood", vec!["wood", "fuel"], 1000, 100_000, 10, 30_000),
            ("peat", vec!["fuel"], 700, 150_000, 6, 40_000),
        ] {
            let good = &rules.goods[good_at(&rules, id)];
            assert_eq!(good.tags, tags);
            assert_eq!(good.supplies["fuel"], supply);
            assert_eq!(
                (
                    good.unit.as_str(),
                    good.weight_g,
                    good.volume_ml,
                    good.base_price,
                    good.pop_limit_milli
                ),
                ("load", 50_000, volume, price, cap)
            );
            assert!(good.stacked_outside);
            assert_eq!(
                (
                    good.hunger_centi,
                    good.necessities_centi,
                    good.niceties_centi,
                    good.luxuries_centi
                ),
                (0, 0, 0, 0)
            );
        }
    }

    fn shipped_rules() -> Rules {
        Rules::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/game/economy.toml"))
            .unwrap()
    }

    fn without_horses(mut rules: Rules) -> Rules {
        rules.herd_species.retain(|s| s.animal != "horses");
        for building in &mut rules.buildings {
            building.herds.remove("horses");
        }
        for grazing in rules.grazing.values_mut() {
            grazing.remove("horses");
        }
        rules
    }

    #[test]
    fn fourth_species_runs_its_lifecycle_and_ledger() {
        let mut rules = without_horses(shipped_rules());
        let mut goat = rules
            .goods
            .iter()
            .find(|good| good.id == "cattle")
            .unwrap()
            .clone();
        goat.id = "goats".into();
        rules.goods.push(goat);
        rules.herd_species.push(HerdSpecies {
            animal: "goats".into(),
            births_pm_year: 1000,
            deaths_pm_year: 1200,
            emergency_order: 3,
            yields: vec![HerdYield {
                good: "hides".into(),
                units: 2,
            }],
            products: vec![HerdProduct {
                good: "dairy".into(),
                amount_milli: Some(1500),
                season: None,
                month: Some(1),
            }],
        });
        for terrain in &rules.terrain_order {
            rules
                .grazing
                .get_mut(terrain)
                .unwrap()
                .insert("goats".into(), 500);
        }
        rules.seasons.insert(
            "herd_births".into(),
            vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        );
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .herds
            .insert("goats".into(), HerdCapacity { maximum: 2 });
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let farm = &mut state.places[0].buildings[0];
        farm.herds = vec![0, 0, 0, 10];
        farm.herd_birth_carries = vec![0; 4];
        farm.herd_death_carries = vec![0; 4];
        farm.herd_grazing_sums = vec![1000, 1000, 1000, 500];
        farm.herd_product_carries = vec![0; 3];
        let ledger = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(ledger.herd_births, [0, 0, 0, 5]);
        assert_eq!(ledger.herd_deaths, [0, 0, 0, 1]);
        assert_eq!(ledger.herd_slaughter, [0, 0, 0, 12]);
        assert_eq!(ledger.herds_after, [0, 0, 0, 2]);
        assert_eq!(ledger.places[0].herd_slaughter[3], 12);
        assert_eq!(ledger.produced[good_at(&rules, "hides")], 24);
        assert_eq!(ledger.produced[good_at(&rules, "dairy")], 3);
        let february = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 2,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(february.herd_deaths[3], 0);
        assert_eq!(state.places[0].buildings[0].herd_death_carries[3], 2400);
        let march = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 3,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(march.herd_deaths[3], 0);
        assert_eq!(state.places[0].buildings[0].herd_death_carries[3], 4800);
    }

    #[test]
    fn sheep_only_farm_makes_dairy_as_an_ordinary_crop() {
        let mut rules = shipped_rules();
        let farm_index = rules.buildings.iter().position(|b| b.id == "farm").unwrap();
        rules.buildings[farm_index]
            .herds
            .retain(|animal, _| animal == "sheep");
        rules.buildings[farm_index].methods[0].annual_output =
            BTreeMap::from([("dairy".into(), 860)]);
        rules.buildings[farm_index].methods[0].score_share_pm =
            BTreeMap::from([("dairy".into(), 1000)]);
        let method = &rules.buildings[farm_index].methods[0];
        assert_eq!(
            starting_method_output(
                &rules,
                &rules.buildings[farm_index],
                method,
                "dairy",
                860,
                Some("pasture"),
                550
            )
            .unwrap(),
            602
        );
        flatten_pasture(&mut rules);
        rules
            .suitability
            .get_mut(FLAT)
            .unwrap()
            .insert("dairy".to_owned(), 700);
        let mut state = shipped_production_fixture(&rules);
        let dairy = good_at(&rules, "dairy");
        let farm = &mut state.places[0].buildings[0];
        farm.herds = vec![0, 0, 1, 0];
        let mut total = 0;
        let mut wool = 0;
        for month in 1..=12 {
            let ledger = run_monthly_production(
                &rules,
                &mut state,
                Date {
                    year: 768,
                    month,
                    day: 1,
                    hour: 12,
                },
            )
            .unwrap();
            total += ledger.produced[dairy];
            wool += ledger.produced[good_at(&rules, "wool")];
        }
        assert_eq!(total, 602);
        assert_eq!(wool, 1);
        let cattle = HerdCapacity { maximum: 12 };
        rules.buildings[farm_index]
            .herds
            .insert("cattle".into(), cattle);
        let method = &rules.buildings[farm_index].methods[0];
        assert_eq!(
            starting_method_output(
                &rules,
                &rules.buildings[farm_index],
                method,
                "dairy",
                860,
                Some("pasture"),
                550
            )
            .unwrap(),
            645
        );
        flatten_pasture(&mut rules);
        let mut with_cattle = shipped_production_fixture(&rules);
        with_cattle.places[0].buildings[0].herds = vec![1, 0, 1, 0];
        let january = run_monthly_production(
            &rules,
            &mut with_cattle,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(january.produced[dairy], 1);
    }

    #[test]
    fn emergency_priority_skips_hide_only_species_before_pigs_and_cattle() {
        let mut rules = without_horses(shipped_rules());
        let mut goat = rules
            .goods
            .iter()
            .find(|g| g.id == "cattle")
            .unwrap()
            .clone();
        goat.id = "goats".into();
        rules.goods.push(goat);
        for species in &mut rules.herd_species {
            species.emergency_order += 1;
        }
        rules.herd_species.push(HerdSpecies {
            animal: "goats".into(),
            births_pm_year: 0,
            deaths_pm_year: 0,
            emergency_order: 0,
            yields: vec![HerdYield {
                good: "hides".into(),
                units: 1,
            }],
            products: Vec::new(),
        });
        for terrain in &rules.terrain_order {
            rules
                .grazing
                .get_mut(terrain)
                .unwrap()
                .insert("goats".into(), 1000);
        }
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .herds
            .insert("goats".into(), HerdCapacity { maximum: 10 });
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let place = &mut state.places[0];
        place.buildings[0].herds = vec![1, 1, 0, 1];
        place.pops[0].consumption.necessities_pm = 200;
        place.pops[0].consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
        let mut ledger = DailyConsumptionLedger {
            butchered: vec![0; rules.goods.len()],
            lost: vec![0; rules.goods.len()],
            herd_slaughter: vec![0; 4],
            ..DailyConsumptionLedger::default()
        };
        butcher_emergency(&rules, place, 0, 1, &mut ledger).unwrap();
        assert_eq!(place.buildings[0].herds, [1, 0, 0, 1]);
        assert_eq!(ledger.herd_slaughter, [0, 1, 0, 0]);
        assert_eq!(ledger.butchered[good_at(&rules, "pork")], 88);
    }

    #[test]
    fn herd_products_keep_separate_carries_and_fractional_wool() {
        let mut rules = without_horses(shipped_rules());
        for species in &mut rules.herd_species {
            species.births_pm_year = 0;
            species.deaths_pm_year = 0;
        }
        let mut goat = rules
            .goods
            .iter()
            .find(|good| good.id == "cattle")
            .unwrap()
            .clone();
        goat.id = "goats".into();
        rules.goods.push(goat);
        rules.herd_species.push(HerdSpecies {
            animal: "goats".into(),
            births_pm_year: 0,
            deaths_pm_year: 0,
            emergency_order: 3,
            yields: vec![HerdYield {
                good: "hides".into(),
                units: 1,
            }],
            products: vec![HerdProduct {
                good: "dairy".into(),
                amount_milli: Some(1500),
                season: None,
                month: Some(1),
            }],
        });
        for terrain in &rules.terrain_order {
            rules
                .grazing
                .get_mut(terrain)
                .unwrap()
                .insert("goats".into(), 1000);
        }
        rules
            .seasons
            .insert("dairy".into(), vec![1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .herds
            .insert("goats".into(), HerdCapacity { maximum: 10 });
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let farm = &mut state.places[0].buildings[0];
        farm.herds = vec![1, 0, 1, 1];
        farm.herd_birth_carries = vec![0; 4];
        farm.herd_death_carries = vec![0; 4];
        farm.herd_grazing_sums = vec![1000; 4];
        farm.herd_product_carries = vec![0, 0, 500_000];
        let date = Date {
            year: 768,
            month: 1,
            day: 1,
            hour: 12,
        };
        let january = run_monthly_production(&rules, &mut state, date).unwrap();
        assert_eq!(january.produced[good_at(&rules, "dairy")], 74);
        assert_eq!(state.places[0].buildings[0].herd_product_carries, [0, 0, 0]);
        let june1 = run_monthly_production(&rules, &mut state, Date { month: 6, ..date }).unwrap();
        assert_eq!(june1.produced[good_at(&rules, "wool")], 1);
        let june2 = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 769,
                month: 6,
                ..date
            },
        )
        .unwrap();
        assert_eq!(june2.produced[good_at(&rules, "wool")], 2);
    }

    #[test]
    fn new_species_product_uses_the_herd_startup_estimate() {
        let mut rules = without_horses(shipped_rules());
        let mut goat = rules
            .goods
            .iter()
            .find(|good| good.id == "cattle")
            .unwrap()
            .clone();
        goat.id = "goats".into();
        rules.goods.push(goat);
        rules.herd_species.push(HerdSpecies {
            animal: "goats".into(),
            births_pm_year: 0,
            deaths_pm_year: 0,
            emergency_order: 3,
            yields: Vec::new(),
            products: vec![HerdProduct {
                good: "dairy".into(),
                amount_milli: Some(1000),
                season: None,
                month: Some(6),
            }],
        });
        let farm = rules
            .buildings
            .iter_mut()
            .find(|building| building.id == "farm")
            .unwrap();
        farm.herds.clear();
        farm.herds
            .insert("goats".into(), HerdCapacity { maximum: 4 });
        let farm = &rules.buildings[0];
        assert_eq!(
            starting_method_output(
                &rules,
                farm,
                &farm.methods[0],
                "dairy",
                860,
                Some("pasture"),
                550
            )
            .unwrap(),
            645
        );
        let mut sea_method = farm.methods[0].clone();
        sea_method.resource = "sea".into();
        assert_eq!(
            starting_method_output(&rules, farm, &sea_method, "dairy", 860, None, 550).unwrap(),
            645
        );
    }

    #[test]
    fn economy_save_preserves_herds_and_product_carries_through_production() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut uninterrupted = shipped_production_fixture(&rules);
        uninterrupted.places[0].buildings[0].herd_product_carries[0] = 123_456;
        let bytes = uninterrupted.to_bytes().unwrap();
        let mut loaded = EconomyState::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.places[0].buildings[0].herds, [9, 6, 5, 0]);
        assert_eq!(
            loaded.places[0].buildings[0].herd_product_carries[0],
            123_456
        );
        let date = Date {
            year: 768,
            month: 1,
            day: 1,
            hour: 12,
        };
        let left = run_monthly_production(&rules, &mut uninterrupted, date).unwrap();
        let right = run_monthly_production(&rules, &mut loaded, date).unwrap();
        assert_eq!(right, left);
        assert_eq!(loaded, uninterrupted);
    }

    #[test]
    fn starting_state_creates_the_shipped_head_counts_per_level() {
        let mut rules = tiny_rules();
        install_fixture_herds(&mut rules);
        rules.herd_opening.spread = [1000, 1000];
        rules.herd_opening.keep_pm = 1000;
        let mut data = tiny_start_data();
        let defaults = data
            .defaults
            .get_mut("ringfort")
            .unwrap()
            .get_mut("1")
            .unwrap();
        defaults.farm_levels = 2;
        let location = StartLocation {
            id: "fort".into(),
            kind: "ringfort".into(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".into(), 8)]),
                fertility_pm: BTreeMap::from([("pasture".into(), 8000)]),
                fertile_cells: 8,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        };
        let mut one = data.clone();
        one.defaults
            .get_mut("ringfort")
            .unwrap()
            .get_mut("1")
            .unwrap()
            .farm_levels = 1;
        let (single, _) =
            starting_state(&rules, &one, std::slice::from_ref(&location), 10).unwrap();
        let single_farm = single.places[0]
            .buildings
            .iter()
            .find(|building| building.kind == "farm")
            .unwrap();
        assert_eq!(single_farm.herds, [8, 5, 4]);
        let (state, _) =
            starting_state(&rules, &data, std::slice::from_ref(&location), 10).unwrap();
        let farm = state.places[0]
            .buildings
            .iter()
            .find(|building| building.kind == "farm")
            .unwrap();
        assert_eq!(farm.method_levels, [2]);
        assert_eq!(farm.herds, [16, 10, 9]);
        let mut half = rules.clone();
        half.herd_start_share = [1, 2];
        let (state, _) = starting_state(&half, &data, std::slice::from_ref(&location), 10).unwrap();
        let farm = state.places[0]
            .buildings
            .iter()
            .find(|b| b.kind == "farm")
            .unwrap();
        assert_eq!(farm.herds, [12, 8, 7]);
        assert_eq!(
            herd_counts(&rules, &[10], &rules.buildings[0]).unwrap(),
            [80, 53, 46]
        );
        let mut fixture = rules.clone();
        let mut goat = fixture.herd_species[0].clone();
        goat.animal = "goats".into();
        fixture.herd_species.push(goat);
        fixture.buildings[0]
            .herds
            .insert("goats".into(), HerdCapacity { maximum: 3 });
        assert_eq!(
            herd_counts(&fixture, &[2], &fixture.buildings[0]).unwrap()[3],
            4
        );
    }

    #[test]
    fn seeded_herds_follow_land_rank_and_stable_keys() {
        let mut rules = shipped_rules();
        rules.herd_opening.spread = [1000, 1000];
        rules.herd_opening.keep_pm = 1000;
        let farm = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        let demesne = rules.buildings.iter().find(|b| b.id == "demesne").unwrap();
        let pasture = vec![1000, 400, 1000, 1000];
        let forest = vec![300, 400, 300, 300];
        let levels = |b: &BuildingType| {
            let mut v = vec![0; b.methods.len()];
            v[0] = 1;
            v
        };
        let one = opening_herds(&rules, &levels(farm), farm, &pasture, 42, "a", 0, false).unwrap();
        assert_eq!(one[0], 8);
        assert_eq!(
            opening_herds(&rules, &levels(farm), farm, &forest, 42, "a", 0, false).unwrap()[0],
            5
        );
        assert_eq!(
            opening_herds(
                &rules,
                &levels(demesne),
                demesne,
                &pasture,
                42,
                "a",
                80,
                true
            )
            .unwrap()[0],
            12
        );
        rules.herd_opening.spread = [1, 1];
        let farm = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        assert_eq!(
            opening_herds(&rules, &levels(farm), farm, &pasture, 42, "a", 0, false).unwrap()[0],
            1
        );
        rules.herd_opening.spread = [1500, 1500];
        let demesne = rules.buildings.iter().find(|b| b.id == "demesne").unwrap();
        // Raw cattle count is 12 * 2/3 * 1 * 1.5 * 1.5 = 18, capped at 12.
        assert_eq!(
            opening_herds(
                &rules,
                &levels(demesne),
                demesne,
                &pasture,
                42,
                "a",
                80,
                true
            )
            .unwrap()[0],
            12
        );
        rules.herd_opening.spread = [800, 1200];
        let farm = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        let mut ten_levels = levels(farm);
        ten_levels[0] = 10;
        let ten_grazing = [10_000, 4_000, 10_000, 10_000];
        let counts: std::collections::BTreeSet<_> = (0..10_000)
            .map(|seed| {
                opening_herds(&rules, &ten_levels, farm, &ten_grazing, seed, "a", 0, false).unwrap()
                    [0]
            })
            .collect();
        assert!(counts.iter().all(|n| (64..=96).contains(n)));
        assert!(counts.contains(&64) && counts.contains(&96));
        assert_ne!(
            herd_draw(42, &["a", "farm", "cattle", "keep"]),
            herd_draw(42, &["a", "farm", "cattle", "spread"])
        );
    }

    #[test]
    fn opening_draw_is_stable_and_chance_floors_negative_grazing() {
        let opening = HerdOpening::default();
        assert_eq!(horse_keep_chance(&opening, 20, 1100 / 3), 216);
        let rules = shipped_rules();
        let demesne = rules.buildings.iter().find(|b| b.id == "demesne").unwrap();
        let mut levels = vec![0; demesne.methods.len()];
        levels[0] = 3;
        let grazing = [3000, 1200, 3000, 1100];
        for (seed, draw, kept) in [(1531, 215, true), (1212, 216, false)] {
            assert_eq!(
                herd_draw(seed, &["a", "demesne", "horses", "keep"]) % 1000,
                draw
            );
            let herds =
                opening_herds(&rules, &levels, demesne, &grazing, seed, "a", 20, false).unwrap();
            assert_eq!(herds[3] > 0, kept, "draw {draw}");
        }
        assert_eq!(u64::midpoint(1000, 367), 683);
        assert_eq!(
            herd_draw(42, &["a", "farm", "cattle", "keep"]),
            11_894_029_966_652_781_520
        );
        assert_ne!(herd_draw(42, &["ab", "c"]), herd_draw(42, &["a", "bc"]));
    }

    #[test]
    fn opening_draws_are_independent_of_species_order() {
        let rules = shipped_rules();
        let farm = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        let mut levels = vec![0; farm.methods.len()];
        levels[0] = 1;
        let grazing = vec![1000, 400, 1000, 1000];
        let before = opening_herds(&rules, &levels, farm, &grazing, 42, "a", 0, false).unwrap();
        let mut reversed = rules.clone();
        reversed.herd_species.reverse();
        let mut reverse_grazing = grazing;
        reverse_grazing.reverse();
        let farm = reversed.buildings.iter().find(|b| b.id == "farm").unwrap();
        let mut after = opening_herds(
            &reversed,
            &levels,
            farm,
            &reverse_grazing,
            42,
            "a",
            0,
            false,
        )
        .unwrap();
        after.reverse();
        assert_eq!(before, after);
    }

    #[test]
    fn seed_spreads_herds_and_named_seats_always_keep_horses() {
        let rules = shipped_rules();
        let farm = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        let demesne = rules.buildings.iter().find(|b| b.id == "demesne").unwrap();
        let levels = |b: &BuildingType| {
            let mut v = vec![0; b.methods.len()];
            v[0] = 1;
            v
        };
        let grazing = [300, 400, 300, 300];
        let mut horse_presence = std::collections::BTreeSet::new();
        let mut pig_presence = std::collections::BTreeSet::new();
        let mut starts = std::collections::BTreeSet::new();
        for seed in 0..100 {
            let a =
                opening_herds(&rules, &levels(farm), farm, &grazing, seed, "a", 0, false).unwrap();
            let b =
                opening_herds(&rules, &levels(farm), farm, &grazing, seed, "b", 0, false).unwrap();
            assert!(a[0] > 0 && b[0] > 0);
            assert_eq!(
                b,
                opening_herds(&rules, &levels(farm), farm, &grazing, seed, "b", 0, false).unwrap()
            );
            starts.insert(a.clone());
            pig_presence.insert(a[1] > 0);
            let royal = opening_herds(
                &rules,
                &levels(demesne),
                demesne,
                &grazing,
                seed,
                "a",
                20,
                true,
            )
            .unwrap();
            let ordinary = opening_herds(
                &rules,
                &levels(demesne),
                demesne,
                &grazing,
                seed,
                "a",
                20,
                false,
            )
            .unwrap();
            assert!(royal[3] > 0);
            horse_presence.insert(ordinary[3] > 0);
        }
        assert!(starts.len() > 1);
        assert_eq!(pig_presence.len(), 2);
        assert_eq!(horse_presence.len(), 2);
    }

    #[test]
    fn an_absent_pig_herd_has_no_births() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let pig = rules
            .herd_species
            .iter()
            .position(|s| s.animal == "pigs")
            .unwrap();
        state.places[0].buildings[0].herds[pig] = 0;
        let ledger = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(ledger.herd_births[pig], 0);
        assert_eq!(state.places[0].buildings[0].herds[pig], 0);
    }

    #[test]
    fn herd_opening_rejects_invalid_parameters() {
        let base = shipped_rules();
        let mut reversed = base.clone();
        reversed.herd_opening.spread = [1200, 800];
        assert_eq!(
            reversed.validate("fixture").unwrap_err().entry,
            "herd_opening.spread"
        );
        for (field, change) in [
            ("keep_pm", 0),
            ("court_chances", 1),
            ("grazing_divisor", 2),
            ("spread", 3),
            ("demesne_rank_pm", 4),
            ("floor_per_level", 5),
        ] {
            let mut rules = base.clone();
            match change {
                0 => rules.herd_opening.keep_pm = 1001,
                1 => rules.herd_opening.court_chances[0] = 1001,
                2 => rules.herd_opening.grazing_divisor = 0,
                3 => rules.herd_opening.spread[0] = 0,
                4 => rules.herd_opening.demesne_rank_pm = 0,
                _ => rules.herd_opening.floor_per_level = 0,
            }
            assert!(rules.validate("fixture").unwrap_err().entry.contains(field));
        }
    }

    #[test]
    fn starting_herd_count_overflow_is_refused() {
        let mut rules = shipped_rules();
        let farm = &mut rules.buildings[0];
        farm.herds.get_mut("cattle").unwrap().maximum = u64::MAX;
        let error = herd_counts(&rules, &[2, 0], &rules.buildings[0]).unwrap_err();
        assert_eq!(error.entry, "buildings.farm.herds.cattle");
        assert!(error.problem.contains("overflow"));
    }

    #[test]
    fn shipped_herd_species_values_are_literal() {
        let rules = shipped_rules();
        assert_eq!(
            rules.herd_species,
            vec![
                HerdSpecies {
                    animal: "cattle".into(),
                    births_pm_year: 500,
                    deaths_pm_year: 80,
                    emergency_order: 2,
                    yields: vec![
                        HerdYield {
                            good: "beef".into(),
                            units: 220
                        },
                        HerdYield {
                            good: "hides".into(),
                            units: 1
                        }
                    ],
                    products: vec![HerdProduct {
                        good: "dairy".into(),
                        amount_milli: Some(72_000),
                        season: Some("dairy".into()),
                        month: None
                    }]
                },
                HerdSpecies {
                    animal: "pigs".into(),
                    births_pm_year: 1500,
                    deaths_pm_year: 100,
                    emergency_order: 0,
                    yields: vec![HerdYield {
                        good: "pork".into(),
                        units: 88
                    }],
                    products: Vec::new()
                },
                HerdSpecies {
                    animal: "sheep".into(),
                    births_pm_year: 800,
                    deaths_pm_year: 80,
                    emergency_order: 1,
                    yields: vec![HerdYield {
                        good: "mutton".into(),
                        units: 33
                    }],
                    products: vec![HerdProduct {
                        good: "wool".into(),
                        amount_milli: Some(1500),
                        season: None,
                        month: Some(6)
                    }]
                },
                HerdSpecies {
                    animal: "horses".into(),
                    births_pm_year: 300,
                    deaths_pm_year: 60,
                    emergency_order: 3,
                    yields: vec![HerdYield {
                        good: "hides".into(),
                        units: 1
                    }],
                    products: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn shipped_horses_start_only_at_the_demesne() {
        let rules = shipped_rules();
        let horse = rules.goods.iter().find(|g| g.id == "horses").unwrap();
        assert_eq!(
            (
                horse.unit.as_str(),
                horse.weight_g,
                horse.volume_ml,
                horse.base_price
            ),
            ("head", 400_000, 0, 6_000)
        );
        assert_eq!(
            (
                horse.hunger_centi,
                horse.necessities_centi,
                horse.niceties_centi,
                horse.luxuries_centi
            ),
            (0, 0, 0, 0)
        );
        for (terrain, expected) in [
            ("pasture", 1000),
            ("marsh", 500),
            ("forest", 300),
            ("bog", 300),
            ("mountain", 300),
        ] {
            assert_eq!(rules.grazing[terrain]["horses"], expected);
        }
        for name in ["farm", "holdings", "demesne"] {
            let building = rules.buildings.iter().find(|b| b.id == name).unwrap();
            if name == "demesne" {
                assert_eq!(building.herds["horses"].maximum, 3);
                assert_eq!(herd_counts(&rules, &[2], building).unwrap()[3], 4);
            } else {
                assert!(!building.herds.contains_key("horses"));
            }
        }
    }

    #[test]
    fn horses_breed_die_and_the_surplus_yields_hides() {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let building = &mut state.places[0].buildings[0];
        building.kind = "demesne".into();
        building.levels = 2;
        building.method_levels = vec![2];
        sync_levels(&rules, building);
        // The accepted witness below counts one level's grazing for these two levels, as the fixture always has.
        building.herd_grazing_sums = vec![1000, 400, 1000, 1000];
        building.production_carries.truncate(1);
        building.method_suitability.truncate(1);
        building.herds = vec![0, 0, 0, 4];
        let mut totals = [0, 0, 0, 0];
        for year in 768..788 {
            for month in 1..=12 {
                let ledger = run_monthly_production(
                    &rules,
                    &mut state,
                    Date {
                        year,
                        month,
                        day: 1,
                        hour: 12,
                    },
                )
                .unwrap();
                totals[0] += ledger.herd_births[3];
                totals[1] += ledger.herd_deaths[3];
                totals[2] += ledger.herd_slaughter[3];
                totals[3] += ledger.produced[good_at(&rules, "hides")];
            }
        }
        assert_eq!(
            (state.places[0].buildings[0].herds[3], totals),
            (6, [16, 6, 8, 8])
        );
    }

    #[test]
    fn starving_demesne_never_butchers_horses_even_when_floor_is_waived() {
        let mut rules = shipped_rules();
        for (necessities, expected_herds, expected_slaughter) in [
            (400, [4, 4, 4, 6], [20, 12, 10, 0]),
            (200, [0, 0, 0, 6], [24, 16, 14, 0]),
        ] {
            flatten_pasture(&mut rules);
            let mut state = shipped_production_fixture(&rules);
            let place = &mut state.places[0];
            let building = &mut place.buildings[0];
            building.kind = "demesne".into();
            building.levels = 2;
            building.method_levels = vec![2];
            building.herds = vec![24, 16, 14, 6];
            place.pops[0].workers = 100_000;
            place.pops[0].consumption.necessities_pm = necessities;
            place.pops[0].consumption.pantry_hunger_centi = vec![0; rules.goods.len()];
            let mut ledger = DailyConsumptionLedger {
                butchered: vec![0; rules.goods.len()],
                lost: vec![0; rules.goods.len()],
                herd_slaughter: vec![0; rules.herd_species.len()],
                ..DailyConsumptionLedger::default()
            };
            let threshold = u128::MAX;
            butcher_emergency(&rules, place, 0, threshold, &mut ledger).unwrap();
            assert!(held_hunger(&rules, &place.pops[0]).unwrap() < threshold);
            assert_eq!(place.buildings[0].herds, expected_herds);
            assert_eq!(ledger.herd_slaughter, expected_slaughter);
        }
    }

    #[test]
    fn herd_rules_validate_species_products_capacities_and_grazing() {
        let rules = shipped_rules();
        assert_eq!(
            rules
                .herd_species
                .iter()
                .map(|s| s.animal.as_str())
                .collect::<Vec<_>>(),
            ["cattle", "pigs", "sheep", "horses"]
        );
        for name in ["farm", "holdings", "demesne"] {
            let b = rules.buildings.iter().find(|b| b.id == name).unwrap();
            assert_eq!(b.herds["cattle"], HerdCapacity { maximum: 12 });
            assert_eq!(b.herds["pigs"], HerdCapacity { maximum: 8 });
            assert_eq!(b.herds["sheep"], HerdCapacity { maximum: 7 });
        }
        let mut cases: Vec<(&str, Rules)> = Vec::new();
        let mut bad = rules.clone();
        bad.herd_species[0].animal = "unknown".into();
        cases.push(("herd_species.unknown", bad));
        let mut bad = rules.clone();
        bad.herd_species.push(bad.herd_species[0].clone());
        cases.push(("herd_species", bad));
        let mut bad = rules.clone();
        bad.buildings[0]
            .herds
            .insert("unknown".into(), HerdCapacity { maximum: 1 });
        cases.push(("buildings.farm.herds.unknown", bad));
        let mut bad = rules.clone();
        bad.herd_start_share = [4, 3];
        cases.push(("herd_start_share", bad));
        let mut bad = rules.clone();
        bad.herd_start_share = [0, 0];
        cases.push(("herd_start_share", bad));
        let mut bad = rules.clone();
        bad.herd_start_share = [0, 3];
        cases.push(("herd_start_share", bad));
        let mut bad = rules.clone();
        bad.herd_standard_pm = 0;
        cases.push(("herd_standard_pm", bad));
        let mut bad = rules.clone();
        bad.herd_standard_pm = 1001;
        cases.push(("herd_standard_pm", bad));
        let mut bad = rules.clone();
        bad.herd_species[0].yields[0].good = "unknown".into();
        cases.push(("herd_species.cattle.yields.unknown", bad));
        let mut bad = rules.clone();
        bad.herd_species[0].products[0].good = "unknown".into();
        cases.push(("herd_species.cattle.products.unknown", bad));
        let mut bad = rules.clone();
        bad.herd_species[0].products[0].season = Some("unknown".into());
        cases.push(("herd_species.cattle.products.dairy", bad));
        let mut bad = rules.clone();
        bad.herd_species[2].products[0].month = Some(13);
        cases.push(("herd_species.sheep.products.wool", bad));
        let mut bad = rules.clone();
        bad.herd_species[1].emergency_order = bad.herd_species[0].emergency_order;
        cases.push(("herd_species.pigs.emergency_order", bad));
        let mut bad = rules.clone();
        bad.grazing.get_mut("pasture").unwrap().remove("pigs");
        cases.push(("grazing.pasture.pigs", bad));
        for (entry, rules) in cases {
            assert_eq!(rules.validate("fixture").unwrap_err().entry, entry);
        }
        let mut bad = rules.clone();
        bad.herd_species[0].products[0].amount_milli = None;
        assert_eq!(
            bad.validate("fixture").unwrap_err().entry,
            "herd_species.cattle.products.dairy.amount_milli"
        );
    }

    #[test]
    fn close_startup_methods_share_the_levels_by_need() {
        let mut rules = shipped_rules();
        let farm = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        let mut second = farm.methods[0].clone();
        farm.methods[0].annual_output = BTreeMap::from([("dairy".to_owned(), 860)]);
        farm.methods[0].score_share_pm = BTreeMap::from([("dairy".to_owned(), 1000)]);
        second.id = "oat_alternative".to_owned();
        second.annual_output = BTreeMap::from([("oats".to_owned(), 369)]);
        second.score_share_pm = BTreeMap::from([("oats".to_owned(), 1000)]);
        farm.methods = vec![farm.methods[0].clone(), second];
        let pop = inland_pop(&rules, "peasant", 100, &[]);
        let mut land = LandCounts {
            land_cells: BTreeMap::from([("pasture".to_owned(), 12)]),
            fertility_pm: BTreeMap::from([("pasture".to_owned(), 12000)]),
            fertile_cells: 12,
            fertile_cells_by_terrain: BTreeMap::new(),
            sea: 0,
        };
        let mut demand = StartDemand::new(&rules, &[pop]).unwrap();
        let result = assign_building_levels(
            &rules,
            &rules.buildings[0],
            6,
            &mut land,
            &mut 0,
            &mut demand,
            550,
        )
        .unwrap();
        // Both methods serve food and neither is worth more by an estimate any longer (CK-404): they share the six levels evenly.
        assert_eq!(result.method_levels, [3, 3]);
        assert_eq!(
            result.output,
            BTreeMap::from([("dairy".to_owned(), 1935), ("oats".to_owned(), 774)])
        );
    }

    pub(super) fn good_at(rules: &Rules, id: &str) -> usize {
        rules.goods.iter().position(|good| good.id == id).unwrap()
    }

    fn inland_pop(rules: &Rules, kind: &str, people: u64, held: &[(&str, u64)]) -> PopState {
        let goods = rules.goods.len();
        let mut stock = vec![0; goods];
        for (id, units) in held {
            stock[good_at(rules, id)] = *units;
        }
        PopState {
            kind: kind.to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: people,
            dependents: 0,
            employment: Vec::new(),
            stock,
            consumption: PopConsumption {
                pantry_hunger_centi: vec![0; goods],
                ticking_carries: vec![0; goods],
                ..PopConsumption::default()
            },
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        }
    }

    fn inland_place(
        rules: &Rules,
        pops: Vec<PopState>,
        buildings: Vec<BuildingState>,
    ) -> PlaceState {
        let goods = rules.goods.len();
        PlaceState {
            id: "inland".to_owned(),
            pops,
            buildings,
            prices_milli: rules
                .goods
                .iter()
                .map(|good| good.base_price * 1000)
                .collect(),
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: vec![0; goods],
            last_market_orders: None,
            counters: CounterBuckets::default(),
        }
    }

    #[test]
    fn a_place_with_no_sea_posts_fish_buy_orders_and_no_fish_sell_order() {
        let rules = shipped_rules();
        let fish = good_at(&rules, "fish");
        for (kind, wanted) in [
            ("peasant", 1200),
            ("labourer", 1200),
            ("monk", 4000),
            ("noble", 2000),
        ] {
            let place = inland_place(&rules, vec![inland_pop(&rules, kind, 100, &[])], Vec::new());
            let orders = daily_market_orders(&rules, &place).unwrap();
            assert_eq!(orders.buy_units[fish], wanted, "{kind} fish buy order");
            assert_eq!(orders.sell_units[fish], 0, "{kind} fish sell order");
        }
    }

    #[test]
    fn inland_pops_keep_the_profile_share_of_every_other_food() {
        let rules = shipped_rules();
        let oats = good_at(&rules, "oats");
        assert_eq!(
            normalized_diet_shares(&rules, &rules.diet_profiles["peasant"]).unwrap()[oats],
            400
        );
        assert_eq!(
            normalized_diet_shares(&rules, &rules.diet_profiles["monk"]).unwrap()[oats],
            350
        );
        // 100 peasants at 240 days: ceil(400 * 100 * 100 * 240 / (1000 * 1800)).
        assert_eq!(
            food_target_units(&rules, "peasant", 100, oats, 240),
            Ok(534)
        );
    }

    #[test]
    fn owner_reserve_and_rent_basket_hold_fish_at_a_place_with_no_sea() {
        let rules = shipped_rules();
        let fish = good_at(&rules, "fish");
        let mut building = reserve_test_building(1, 0);
        building.stock = vec![0; rules.goods.len()];
        building.stock[fish] = 1000;
        let place = inland_place(
            &rules,
            vec![inland_pop(&rules, "peasant", 100, &[])],
            vec![building],
        );
        // The owner reserve is the 60-day fish target of 100 peasants: 300 lb. The building offers the rest.
        assert_eq!(
            daily_market_orders(&rules, &place).unwrap().sell_units[fish],
            700
        );
        // One peasant for 30 days: fish 2 lb at 30, among the other foods at their profile shares.
        assert_eq!(
            rent_basket_milli(&rules, &place, "peasant", 30),
            Ok(734_000)
        );
        let mut without_fish = place.clone();
        without_fish.prices_milli[fish] = 0;
        assert_eq!(
            rent_basket_milli(&rules, &without_fish, "peasant", 30),
            Ok(734_000 - 2 * 30_000)
        );
    }

    #[test]
    fn two_places_that_differ_only_in_their_sea_cells_give_the_same_orders_and_basket() {
        let mut rules = tiny_rules();
        rules.goods.push(Good {
            id: "fish".to_owned(),
            tags: Vec::new(),
            supplies: BTreeMap::new(),
            unit: "pound".to_owned(),
            weight_g: 1,
            volume_ml: 1,
            stacked_outside: false,
            hunger_centi: 100,
            necessities_centi: 100,
            niceties_centi: 0,
            luxuries_centi: 0,
            base_price: 100,
            pop_limit_milli: 1000,
        });
        rules
            .diet_profiles
            .get_mut("peasant")
            .unwrap()
            .extend([("food".to_owned(), 700), ("fish".to_owned(), 300)]);
        let mut data = tiny_start_data();
        data.defaults
            .get_mut("ringfort")
            .unwrap()
            .get_mut("1")
            .unwrap()
            .farm_levels = 1;
        data.defaults
            .get_mut("ringfort")
            .unwrap()
            .get_mut("1")
            .unwrap()
            .building_stock
            .insert("fish".to_owned(), 1000);
        let location = |id: &str, sea| StartLocation {
            id: id.to_owned(),
            kind: "ringfort".to_owned(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".to_owned(), 2)]),
                fertility_pm: BTreeMap::from([("pasture".to_owned(), 2000)]),
                fertile_cells: 2,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea,
            },
        };
        let (state, _) = starting_state(
            &rules,
            &data,
            &[location("inland", 0), location("coast", 1)],
            10,
        )
        .unwrap();
        let (inland, coast) = (&state.places[0], &state.places[1]);
        assert_eq!(inland.pops, coast.pops);
        assert_eq!(inland.buildings, coast.buildings);
        assert_eq!(inland.prices_milli, coast.prices_milli);
        let orders = daily_market_orders(&rules, inland).unwrap();
        assert_eq!(orders, daily_market_orders(&rules, coast).unwrap());
        assert!(
            orders.buy_units[1] > 0,
            "the peasants want fish at both places"
        );
        let farm = inland
            .buildings
            .iter()
            .find(|building| building.kind == "farm")
            .expect("both places start the farm");
        assert_eq!(farm.stock[1], 1000, "with fish in its store");
        assert_eq!(
            orders.sell_units[1], 640,
            "the farm offers its 1000 lb of fish less its owners' 60-day reserve of 360 (300 x 20 x 100 x 60 / (1000 x 100)), at both places"
        );
        assert_eq!(
            rent_basket_milli(&rules, inland, "peasant", 30),
            rent_basket_milli(&rules, coast, "peasant", 30)
        );
    }

    #[test]
    fn fish_wanted_and_never_offered_climbs_to_one_and_a_half_times_base_and_stays() {
        let rules = shipped_rules();
        let fish = good_at(&rules, "fish");
        let mut state = EconomyState {
            places: vec![inland_place(
                &rules,
                vec![inland_pop(&rules, "peasant", 100, &[])],
                Vec::new(),
            )],
        };
        assert_eq!(state.places[0].prices_milli[fish], 30_000);
        update_daily_prices(&rules, &mut state).unwrap();
        assert_eq!(state.places[0].prices_milli[fish], 31_500);
        let mut reached = None;
        for day in 2..=120 {
            update_daily_prices(&rules, &mut state).unwrap();
            let price = state.places[0].prices_milli[fish];
            assert!(price <= 45_000, "day {day}: price {price} passed the cap");
            if price == 45_000 && reached.is_none() {
                reached = Some(day);
            }
            if reached.is_some() {
                assert_eq!(price, 45_000, "day {day}: the price left the cap");
            }
        }
        assert_eq!(reached, Some(97));
    }

    #[test]
    fn eating_does_not_depend_on_the_fish_share() {
        let rules = shipped_rules();
        let oats = good_at(&rules, "oats");
        let mut state = EconomyState {
            places: vec![inland_place(
                &rules,
                vec![inland_pop(&rules, "peasant", 100, &[("oats", 1000)])],
                Vec::new(),
            )],
        };
        let intake = run_daily_consumption(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!(995, pop.stock[oats]);
        assert_eq!(intake, [vec![900]]);
        assert_eq!(pop.consumption.necessities_pm, 45);
    }

    #[test]
    fn the_start_up_projection_sees_the_whole_diet_at_a_place_with_no_sea() {
        let rules = shipped_rules();
        let pop = inland_pop(&rules, "peasant", 100, &[]);
        assert_eq!(
            projected_buy_units(&rules, std::slice::from_ref(&pop), "oats"),
            Ok(811)
        );
        assert_eq!(
            projected_buy_units(&rules, std::slice::from_ref(&pop), "fish"),
            Ok(1825)
        );
    }

    #[test]
    fn fish_demand_opens_no_sea_level_at_a_place_with_no_sea_cell() {
        let mut rules = tiny_rules();
        rules.goods.push(Good {
            id: "fish".to_owned(),
            tags: Vec::new(),
            supplies: BTreeMap::new(),
            unit: "pound".to_owned(),
            weight_g: 1,
            volume_ml: 1,
            stacked_outside: false,
            hunger_centi: 100,
            necessities_centi: 100,
            niceties_centi: 0,
            luxuries_centi: 0,
            base_price: 100,
            pop_limit_milli: 1000,
        });
        rules
            .diet_profiles
            .get_mut("peasant")
            .unwrap()
            .extend([("food".to_owned(), 700), ("fish".to_owned(), 300)]);
        rules.buildings[0].methods.push(Method {
            id: "coastal".to_owned(),
            resource: "sea".to_owned(),
            cells_per_level: None,
            annual_output: BTreeMap::from([("fish".to_owned(), 1000)]),
            suitability: "sea".to_owned(),
            score_share_pm: BTreeMap::from([("fish".to_owned(), 1000)]),
            serves: "food".to_owned(),
        });
        let mut data = tiny_start_data();
        data.defaults
            .get_mut("ringfort")
            .unwrap()
            .get_mut("1")
            .unwrap()
            .farm_levels = 3;
        let (state, _) = starting_state(
            &rules,
            &data,
            &[StartLocation {
                id: "inland".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 2)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), 2000)]),
                    fertile_cells: 2,
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 0,
                },
            }],
            10,
        )
        .unwrap();
        let farm = &state.places[0].buildings[0];
        assert_eq!(farm.method_levels[1], 0, "no sea cell, no sea level");
        assert!(
            daily_market_orders(&rules, &state.places[0])
                .unwrap()
                .buy_units[1]
                > 0,
            "the peasants still want the fish they cannot catch"
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The fixture builds the monk and labourer rules the shared-land start needs."
    )]
    fn holdings_take_shared_farming_land_before_the_farm() {
        let mut rules = tiny_rules();
        let farm_method = rules.buildings[0].methods[0].clone();
        rules.buildings.insert(
            1,
            BuildingType {
                id: "holdings".to_owned(),
                church: false,
                herds: BTreeMap::new(),
                workers_per_level: 10,
                land_per_level: 2,
                owner_workers: BTreeMap::from([
                    ("monk".to_owned(), 5),
                    ("labourer".to_owned(), 10),
                ]),
                penalty_pm: 500,
                overhead_pm: 300,
                recruit_range_m: Some(100_000),
                market_capacity_ml_per_level: 0,
                methods: vec![farm_method],
                owner_kind: None,
            },
        );
        rules.footprint_m2.insert("holdings".to_owned(), 4000);
        rules.housing_per_level.insert("holdings".to_owned(), 14);
        rules
            .housing_class
            .insert("holdings".to_owned(), "basic".to_owned());
        rules.pop_types.push(PopType {
            id: "monk".to_owned(),
            dependent_pm: 100,
            births_pm_year: 0,
            worker_deaths_pm_year: 20,
            dependent_deaths_pm_year: 60,
            necessities_expectation_pm: 1000,
            niceties_expectation_pm: 0,
            luxuries_expectation_pm: 0,
            fuel_need_milli_day: 0,
        });
        add_empty_comfort_profile(&mut rules, "monk");
        rules
            .starting_needs_pm
            .insert("monk".to_owned(), [1000, 0, 0]);
        rules.diet_profiles.insert(
            "monk".to_owned(),
            BTreeMap::from([("food".to_owned(), 1000)]),
        );
        rules.pop_types.push(PopType {
            id: "labourer".to_owned(),
            dependent_pm: 400,
            births_pm_year: 67,
            worker_deaths_pm_year: 20,
            dependent_deaths_pm_year: 60,
            necessities_expectation_pm: 1000,
            niceties_expectation_pm: 0,
            luxuries_expectation_pm: 0,
            fuel_need_milli_day: 0,
        });
        add_empty_comfort_profile(&mut rules, "labourer");
        rules
            .starting_needs_pm
            .insert("labourer".to_owned(), [1000, 0, 0]);
        rules.diet_profiles.insert(
            "labourer".to_owned(),
            BTreeMap::from([("food".to_owned(), 1000)]),
        );
        rules.peoples.cultures.push(fixture_culture("english"));
        let mut data = tiny_start_data();
        let defaults = data
            .defaults
            .get_mut("ringfort")
            .unwrap()
            .get_mut("1")
            .unwrap();
        defaults.holdings_levels = 1;
        defaults.farm_levels = 1;
        defaults.monks = 10;
        let (state, report) = starting_state(
            &rules,
            &data,
            &[StartLocation {
                id: "fort".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 2)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), 2000)]),
                    fertile_cells: 2,
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 0,
                },
            }],
            10,
        )
        .unwrap();
        assert_eq!(
            report,
            ["fort farm: cut levels 1 -> 0 (no method has enough land or sea)"]
        );
        assert_eq!(
            state.places[0]
                .buildings
                .iter()
                .map(|building| (building.kind.as_str(), building.levels))
                .collect::<Vec<_>>(),
            [("holdings", 1), ("farm", 0), ("storehouse", 1)]
        );
    }

    #[test]
    fn storehouse_kinds_follow_remaining_production_volume_and_tie_order() {
        let mut rules = tiny_rules();
        for (id, volume_ml) in [("oats", 1000), ("ale", 500), ("dairy", 200)] {
            rules.goods.push(Good {
                id: id.to_owned(),
                tags: Vec::new(),
                supplies: BTreeMap::new(),
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml,
                stacked_outside: false,
                hunger_centi: 0,
                necessities_centi: 0,
                niceties_centi: 0,
                luxuries_centi: 0,
                base_price: 1,
                pop_limit_milli: 0,
            });
        }
        let production = BTreeMap::from([
            ("oats".to_owned(), 2),
            ("ale".to_owned(), 3),
            ("dairy".to_owned(), 1),
        ]);
        assert_eq!(storehouse_kinds(&rules, &production, 2), [1, 0, 1]);
        assert_eq!(storehouse_kinds(&rules, &BTreeMap::new(), 2), [2, 0, 0]);
    }

    #[test]
    fn overflow_split_redistributes_full_buildings_in_order() {
        assert_eq!(
            split_with_caps(12, &[1, 1, 1], &[1, 20, 20]).unwrap(),
            vec![1, 6, 5]
        );
        assert_eq!(
            split_with_caps(2, &[1, 1, 1], &[10, 10, 10]).unwrap(),
            vec![1, 1, 0]
        );
        assert_eq!(split_with_caps(12, &[1, 1], &[1, 2]).unwrap(), vec![1, 2]);
    }

    #[test]
    fn largest_remainder_is_deterministic_and_conserves_the_total() {
        assert_eq!(largest_remainder(7, &[1, 1, 1]), Ok(vec![3, 2, 2]));
        assert_eq!(largest_remainder(0, &[1, 2]), Ok(vec![0, 0]));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The round-trip test encodes one carries/accounts/herds state and compares its next month before and after load."
    )]
    fn canonical_economy_round_trip_is_byte_stable() {
        let state = EconomyState {
            places: vec![PlaceState {
                id: "a".to_owned(),
                price_drift_carries: vec![-321],
                last_market_orders: None,
                counters: CounterBuckets {
                    current: MonthlyCounters {
                        produced: vec![4],
                        used_by_pops: vec![3],
                        lost: vec![2],
                        produced_value_milli: vec![u128::from(u64::MAX) + 9],
                        used_by_pops_value_milli: vec![u128::from(u64::MAX) + 10],
                        ..MonthlyCounters::default()
                    },
                    last: MonthlyCounters {
                        produced: vec![11],
                        used_by_pops: vec![7],
                        lost: vec![5],
                        produced_value_milli: vec![u128::from(u64::MAX) + 17],
                        used_by_pops_value_milli: vec![u128::from(u64::MAX) + 19],
                        ..MonthlyCounters::default()
                    },
                    closed: true,
                },
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "irish".to_owned(),
                    religion: "christianity".to_owned(),
                    literacy_pm: 0,
                    workers: 60,
                    dependents: 40,
                    employment: vec![Employment {
                        owners: 1,
                        workers: 10,
                    }],
                    stock: vec![3],
                    consumption: PopConsumption {
                        pantry_hunger_centi: vec![5],
                        necessities_centi: 123,
                        niceties_centi: 45,
                        luxuries_centi: 67,
                        ticking_carries: vec![654_321],
                        necessities_pm: 450,
                        niceties_pm: 250,
                        luxuries_pm: 100,
                        satisfaction: -4,
                        drift_carries: [-100, 200, -300, 400],
                        birth_carry: 0,
                        worker_death_carry: 0,
                        dependent_death_carry: 0,
                        hearth_milli: 0,
                        fuel_need_month: 0,
                        fuel_burned_month: 0,
                        fuel_death_carry: 0,
                        dependent_drift_carry: -321_654,
                    },
                    counters: CounterBuckets {
                        current: MonthlyCounters {
                            births: 7,
                            deaths: 3,
                            losses: 2,
                            ..MonthlyCounters::default()
                        },
                        last: MonthlyCounters {
                            births: 5,
                            deaths: 4,
                            losses: 1,
                            ..MonthlyCounters::default()
                        },
                        closed: true,
                    },
                    short_on_rent: false,
                    housed_last: false,
                    genetics: PopGenetics::default(),
                    pending_births: 0,
                    pending_deaths: 0,
                }],
                buildings: vec![
                    BuildingState {
                        kind: "farm".to_owned(),
                        levels: 1,
                        method_levels: vec![1],
                        level_records: vec![LevelRecord {
                            number: 0,
                            method: 0,
                            ground: Some("pasture".to_owned()),
                            paydays: MATURE_PAYDAYS,
                            unpaid_work: BTreeMap::new(),
                            last_switch_year: None,
                        }],
                        herds: vec![9, 6, 5],
                        employment: vec![Employment {
                            owners: 1,
                            workers: 10,
                        }],
                        stock: vec![4],
                        storehouse_accounts: None,
                        production_carries: vec![vec![5_000_000_000]],
                        method_suitability: vec![vec![1000]],
                        herd_birth_carries: vec![999_999_999_999, 888, 777],
                        herd_death_carries: vec![11_999, 11_998, 11_997],
                        herd_product_carries: vec![999_999],
                        herd_grazing_sums: vec![1000; 3],
                        wage_carries: vec![999_999],
                        pay_carries: vec![2],
                        fee_carries: Vec::new(),
                        wage_history: (1..=12).collect(),
                        paid_history: (101..=112).collect(),
                        paid_years: vec![768; WAGE_HISTORY_MONTHS],
                    },
                    BuildingState {
                        kind: "storehouse".to_owned(),
                        levels: 1,
                        method_levels: vec![1, 0, 0],
                        level_records: Vec::new(),
                        herds: Vec::new(),
                        employment: vec![Employment::default()],
                        stock: vec![3],
                        storehouse_accounts: Some(StorehouseAccounts {
                            pops: vec![vec![1]],
                            communal: vec![2],
                        }),
                        production_carries: Vec::new(),
                        method_suitability: Vec::new(),
                        herd_birth_carries: Vec::new(),
                        herd_death_carries: Vec::new(),
                        herd_product_carries: Vec::new(),
                        herd_grazing_sums: Vec::new(),
                        wage_carries: vec![0],
                        pay_carries: vec![0],
                        fee_carries: vec![vec![999], vec![998]],
                        wage_history: vec![42; 12],
                        paid_history: vec![7; WAGE_HISTORY_MONTHS],
                        paid_years: vec![769; WAGE_HISTORY_MONTHS],
                    },
                ],
                prices_milli: vec![120_000],
                prepared_m2: 23_456,
                development_pm: 875,
            }],
        };
        let bytes = state.to_bytes().unwrap();
        let mut with_cached_orders = state.clone();
        with_cached_orders.places[0].last_market_orders = Some(DailyMarketOrders {
            buy_units: vec![u128::from(u64::MAX) + 5],
            sell_units: vec![9],
        });
        assert_eq!(
            with_cached_orders.to_bytes().unwrap(),
            bytes,
            "derived daily orders do not change canonical economy bytes"
        );
        let decoded = EconomyState::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, state);
        let decoded_with_orders =
            EconomyState::from_bytes(&with_cached_orders.to_bytes().unwrap()).unwrap();
        assert!(decoded_with_orders.places[0].last_market_orders.is_none());
        assert_eq!(
            decoded.places[0].buildings[0].wage_history,
            (1..=12).collect::<Vec<_>>()
        );
        assert_eq!(decoded.places[0].buildings[0].posted_wage(), 6);
        assert_eq!(decoded.to_bytes().unwrap(), bytes);
        let decoded_counters = &decoded.places[0].counters;
        assert!(decoded_counters.closed);
        assert_eq!(decoded_counters.last.produced, [11]);
        assert_eq!(decoded_counters.last.used_by_pops, [7]);
        assert_eq!(decoded_counters.last.lost, [5]);
        assert_eq!(
            decoded_counters.last.produced_value_milli,
            [u128::from(u64::MAX) + 17]
        );
        assert_eq!(
            decoded_counters.last.used_by_pops_value_milli,
            [u128::from(u64::MAX) + 19]
        );

        let mut before_rollover = state.clone();
        before_rollover.places[0].counters.closed = false;
        let before_bytes = before_rollover.to_bytes().unwrap();
        let before_loaded = EconomyState::from_bytes(&before_bytes).unwrap();
        assert!(!before_loaded.places[0].counters.closed);
        assert_eq!(
            before_loaded.places[0].counters,
            before_rollover.places[0].counters
        );

        let mut after_rollover = state.clone();
        let rollover_prices = after_rollover.places[0].prices_milli.clone();
        roll_counter_month(
            &mut after_rollover.places[0].counters,
            Some(&rollover_prices),
        )
        .unwrap();
        let after_bytes = after_rollover.to_bytes().unwrap();
        let after_loaded = EconomyState::from_bytes(&after_bytes).unwrap();
        assert!(after_loaded.places[0].counters.closed);
        assert_eq!(after_loaded.places[0].counters.last.produced, [4]);
        assert_eq!(
            after_loaded.places[0].counters.last.produced_value_milli,
            [480_000]
        );

        let mut rules = tiny_rules();
        rules.seasons.remove("food");
        rules.goods[0].id = "oats".to_owned();
        rules.goods[0].pop_limit_milli = 1_000_000;
        rules
            .diet_profiles
            .get_mut("peasant")
            .unwrap()
            .remove("food");
        rules
            .diet_profiles
            .get_mut("peasant")
            .unwrap()
            .insert("oats".to_owned(), 1000);
        rules.suitability.get_mut("pasture").unwrap().remove("food");
        rules
            .suitability
            .get_mut("pasture")
            .unwrap()
            .insert("oats".to_owned(), 1000);
        rules.buildings[0].methods[0].annual_output.remove("food");
        rules.buildings[0].methods[0]
            .annual_output
            .insert("oats".to_owned(), 1);
        rules.buildings[0].methods[0].score_share_pm.remove("food");
        rules.buildings[0].methods[0]
            .score_share_pm
            .insert("oats".to_owned(), 1000);
        rules.seasons.insert(
            "grain".to_owned(),
            [1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(),
        );
        rules.seasons.insert(
            "herd_births".to_owned(),
            [1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(),
        );
        let mut uninterrupted = state;
        let mut loaded = decoded;
        for current in [&mut uninterrupted, &mut loaded] {
            for building in &mut current.places[0].buildings {
                building.herds.clear();
                building.herd_birth_carries.clear();
                building.herd_death_carries.clear();
                building.herd_product_carries.clear();
                building.herd_grazing_sums.clear();
            }
        }
        let date = Date {
            year: 768,
            month: 1,
            day: 1,
            hour: 12,
        };
        let uninterrupted_ledger =
            run_monthly_production(&rules, &mut uninterrupted, date).unwrap();
        let loaded_ledger = run_monthly_production(&rules, &mut loaded, date).unwrap();
        assert_eq!(loaded_ledger, uninterrupted_ledger);
        assert_eq!(loaded, uninterrupted);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "This literal vector pins each herd formula and the June-only wool output in one monthly fixture."
    )]
    fn herds_pin_births_deaths_slaughter_dairy_and_june_wool() {
        let mut rules = tiny_rules();
        for id in ["beef", "pork", "mutton", "dairy", "wool", "hides"] {
            rules.goods.push(Good {
                id: id.to_owned(),
                tags: Vec::new(),
                supplies: BTreeMap::new(),
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
                stacked_outside: false,
                hunger_centi: 0,
                necessities_centi: 0,
                niceties_centi: 0,
                luxuries_centi: 0,
                base_price: 100,
                pop_limit_milli: 1_000_000,
            });
        }
        rules.buildings[0].methods[0]
            .annual_output
            .insert("food".to_owned(), 0);
        rules.seasons.insert(
            "food".to_owned(),
            [1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(),
        );
        rules.seasons.insert(
            "herd_births".to_owned(),
            [1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(),
        );
        rules.seasons.insert(
            "dairy".to_owned(),
            [1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(),
        );
        rules.seasons.insert(
            "wool".to_owned(),
            [0, 0, 0, 0, 1000, 0, 0, 0, 0, 0, 0, 0].into(),
        );
        install_fixture_herds(&mut rules);
        let (mut state, _) = starting_state(
            &rules,
            &tiny_start_data(),
            &[StartLocation {
                id: "fort".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts {
                    land_cells: BTreeMap::from([("pasture".to_owned(), 2)]),
                    fertility_pm: BTreeMap::from([("pasture".to_owned(), 2000)]),
                    fertile_cells: 2,
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 0,
                },
            }],
            10,
        )
        .unwrap();
        let farm = state.places[0]
            .buildings
            .iter_mut()
            .find(|building| building.kind == "farm")
            .unwrap();
        farm.herds = vec![20, 12, 1];
        farm.herd_birth_carries = vec![999_999_999_999; 3];
        farm.herd_death_carries = vec![11_999; 3];
        farm.herd_product_carries[1] = 499_900;

        let january = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(january.herd_births, [10, 18, 1]);
        assert_eq!(january.herd_deaths, [1, 1, 1]);
        assert_eq!(january.herd_slaughter, [17, 21, 0]);
        assert_eq!(january.herds_after, [12, 8, 1]);
        let output = &january.produced;
        assert_eq!(
            output[rules
                .goods
                .iter()
                .position(|good| good.id == "beef")
                .unwrap()],
            3740
        );
        assert_eq!(
            output[rules
                .goods
                .iter()
                .position(|good| good.id == "pork")
                .unwrap()],
            1848
        );
        assert_eq!(
            output[rules
                .goods
                .iter()
                .position(|good| good.id == "hides")
                .unwrap()],
            17
        );
        assert_eq!(
            output[rules
                .goods
                .iter()
                .position(|good| good.id == "dairy")
                .unwrap()],
            864
        );
        assert_eq!(
            output[rules
                .goods
                .iter()
                .position(|good| good.id == "wool")
                .unwrap()],
            0
        );

        let june = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 6,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(
            june.produced[rules
                .goods
                .iter()
                .position(|good| good.id == "wool")
                .unwrap()],
            1
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "These literal paydays pin both overhead ratios, fractional carries, worker apportionment and the unpaid-wage path."
    )]
    fn wage_split_pins_farm_holdings_carries_workers_and_ownerless_payday() {
        let mut rules = tiny_rules();
        rules.goods[0].pop_limit_milli = 1_000_000;
        let mut monk = rules.pop_types[0].clone();
        monk.id = "monk".to_owned();
        let mut labourer = rules.pop_types[0].clone();
        labourer.id = "labourer".to_owned();
        rules.pop_types.extend([monk, labourer]);
        add_empty_comfort_profile(&mut rules, "monk");
        add_empty_comfort_profile(&mut rules, "labourer");
        rules
            .starting_needs_pm
            .insert("monk".to_owned(), [1000, 0, 0]);
        rules
            .starting_needs_pm
            .insert("labourer".to_owned(), [1000, 0, 0]);
        let peasant_diet = rules.diet_profiles["peasant"].clone();
        rules
            .diet_profiles
            .insert("monk".to_owned(), peasant_diet.clone());
        rules
            .diet_profiles
            .insert("labourer".to_owned(), peasant_diet);
        rules.seasons.insert(
            "food".to_owned(),
            [1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(),
        );
        rules.seasons.insert(
            "herd_births".to_owned(),
            [1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].into(),
        );
        let mut farm_type = rules.buildings[0].clone();
        farm_type.workers_per_level = 3;
        farm_type.penalty_pm = 0;
        farm_type.methods[0]
            .annual_output
            .insert("food".to_owned(), 10);
        farm_type.overhead_pm = 100;
        let mut holdings_type = farm_type.clone();
        holdings_type.id = "holdings".to_owned();
        holdings_type.overhead_pm = 300;
        holdings_type.owner_workers =
            BTreeMap::from([("monk".to_owned(), 5), ("labourer".to_owned(), 10)]);
        rules.buildings = vec![farm_type.clone(), holdings_type.clone()];
        rules.footprint_m2 =
            BTreeMap::from([("farm".to_owned(), 2000), ("holdings".to_owned(), 4000)]);
        rules.housing_per_level =
            BTreeMap::from([("farm".to_owned(), 17), ("holdings".to_owned(), 14)]);
        rules.housing_class = BTreeMap::from([
            ("farm".to_owned(), "basic".to_owned()),
            ("holdings".to_owned(), "basic".to_owned()),
        ]);

        let pop = |workers, employment| PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers,
            dependents: 0,
            employment,
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let building = |kind: &str, wage_carry| BuildingState {
            kind: kind.to_owned(),
            levels: 1,
            method_levels: vec![1],
            level_records: vec![LevelRecord {
                number: 0,
                method: 0,
                ground: Some("pasture".to_owned()),
                paydays: MATURE_PAYDAYS,
                unpaid_work: BTreeMap::new(),
                last_switch_year: None,
            }],
            herds: Vec::new(),
            employment: vec![
                Employment {
                    owners: 0,
                    workers: 2,
                },
                Employment {
                    owners: 0,
                    workers: 1,
                },
            ],
            stock: vec![0],
            storehouse_accounts: None,
            production_carries: vec![vec![0]],
            method_suitability: vec![vec![1000]],
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: vec![wage_carry],
            pay_carries: vec![0],
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "fort".to_owned(),
                pops: vec![
                    pop(
                        2,
                        vec![
                            Employment {
                                owners: 0,
                                workers: 2,
                            },
                            Employment {
                                owners: 0,
                                workers: 2,
                            },
                        ],
                    ),
                    pop(
                        1,
                        vec![
                            Employment {
                                owners: 0,
                                workers: 1,
                            },
                            Employment {
                                owners: 0,
                                workers: 1,
                            },
                        ],
                    ),
                ],
                buildings: vec![building("farm", 500_000), building("holdings", 700_000)],
                prices_milli: vec![0],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        };
        let ledger = run_monthly_production(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let place = &state.places[0];
        assert_eq!(ledger.produced, [20]);
        assert_eq!(place.pops[0].stock, [11]);
        assert_eq!(place.pops[1].stock, [5]);
        assert_eq!(place.buildings[0].stock, [1]);
        assert_eq!(place.buildings[1].stock, [3]);
        assert_eq!(place.buildings[0].wage_carries, [500_000]);
        assert_eq!(place.buildings[1].wage_carries, [700_000]);

        rules.goods.push(Good {
            id: "beef".to_owned(),
            tags: Vec::new(),
            supplies: BTreeMap::new(),
            unit: "pound".to_owned(),
            weight_g: 1,
            volume_ml: 1,
            stacked_outside: false,
            hunger_centi: 0,
            necessities_centi: 0,
            niceties_centi: 0,
            luxuries_centi: 0,
            base_price: 100,
            pop_limit_milli: 1_000_000,
        });
        rules.goods.push(Good {
            id: "hides".to_owned(),
            tags: Vec::new(),
            supplies: BTreeMap::new(),
            unit: "hide".to_owned(),
            weight_g: 1,
            volume_ml: 1,
            stacked_outside: false,
            hunger_centi: 0,
            necessities_centi: 0,
            niceties_centi: 0,
            luxuries_centi: 0,
            base_price: 100,
            pop_limit_milli: 1_000_000,
        });
        install_fixture_herds(&mut rules);
        let no_workers = EconomyState {
            places: vec![PlaceState {
                id: "ownerless".to_owned(),
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
                    literacy_pm: 0,
                    workers: 0,
                    dependents: 0,
                    employment: vec![Employment::default()],
                    stock: vec![0; rules.goods.len()],
                    consumption: PopConsumption::default(),
                    counters: CounterBuckets::default(),
                    short_on_rent: false,
                    housed_last: false,
                    genetics: PopGenetics::default(),
                    pending_births: 0,
                    pending_deaths: 0,
                }],
                buildings: vec![BuildingState {
                    kind: "farm".to_owned(),
                    levels: 1,
                    method_levels: vec![1],
                    level_records: vec![LevelRecord {
                        number: 0,
                        method: 0,
                        ground: Some("pasture".to_owned()),
                        paydays: MATURE_PAYDAYS,
                        unpaid_work: BTreeMap::new(),
                        last_switch_year: None,
                    }],
                    herds: vec![20, 0, 0],
                    employment: vec![Employment::default()],
                    stock: vec![0; rules.goods.len()],
                    storehouse_accounts: None,
                    production_carries: vec![vec![0; rules.goods.len()]],
                    method_suitability: vec![vec![1000; rules.goods.len()]],
                    herd_birth_carries: vec![0; 3],
                    herd_death_carries: vec![0; 3],
                    herd_product_carries: vec![0; 2],
                    herd_grazing_sums: vec![0; 3],
                    wage_carries: vec![0; rules.goods.len()],
                    pay_carries: vec![0; rules.goods.len()],
                    fee_carries: Vec::new(),
                    wage_history: vec![0; 12],
                    paid_history: vec![0; WAGE_HISTORY_MONTHS],
                    paid_years: vec![0; WAGE_HISTORY_MONTHS],
                }],
                prices_milli: vec![0; rules.goods.len()],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        };
        let mut no_workers = no_workers;
        let ledger = run_monthly_production(
            &rules,
            &mut no_workers,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        let beef = rules
            .goods
            .iter()
            .position(|good| good.id == "beef")
            .unwrap();
        let hides = rules
            .goods
            .iter()
            .position(|good| good.id == "hides")
            .unwrap();
        assert_eq!(ledger.produced[beef], 1760);
        assert_eq!(ledger.produced[hides], 8);
        assert_eq!(no_workers.places[0].buildings[0].pay_carries[beef], 1584);
        assert_eq!(no_workers.places[0].buildings[0].pay_carries[hides], 7);
    }

    #[test]
    fn shipped_seasons_drive_grain_apples_fish_and_herd_births() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut rules = Rules::load(assets.join("game/economy.toml")).unwrap();
        flatten_pasture(&mut rules);
        let mut spring = shipped_production_fixture(&rules);
        let ledger = run_monthly_production(
            &rules,
            &mut spring,
            Date {
                year: 768,
                month: 3,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(ledger.herd_births, [1, 0, 0, 0]);
        let fish = rules
            .goods
            .iter()
            .position(|good| good.id == "fish")
            .unwrap();
        assert_eq!(ledger.produced[fish], 232);

        flatten_pasture(&mut rules);
        let mut harvest = shipped_production_fixture(&rules);
        harvest.places[0].buildings[1].method_levels = vec![1, 0];
        sync_levels(&rules, &mut harvest.places[0].buildings[1]);
        flatten_pasture(&mut rules);
        let mut fishing = shipped_production_fixture(&rules);
        let mut apples_by_month = Vec::new();
        for month in 8..=11 {
            let ledger = run_monthly_production(
                &rules,
                &mut harvest,
                Date {
                    year: 768,
                    month,
                    day: 1,
                    hour: 12,
                },
            )
            .unwrap();
            let good_index = |id: &str| rules.goods.iter().position(|good| good.id == id).unwrap();
            apples_by_month.push(ledger.produced[good_index("apples")]);
            if month == 9 {
                assert_eq!(ledger.produced[good_index("oats")], 600);
                assert_eq!(ledger.produced[good_index("barley")], 100);
                assert_eq!(ledger.produced[good_index("peas_beans")], 12);
            }
            let fish_ledger = run_monthly_production(
                &rules,
                &mut fishing,
                Date {
                    year: 768,
                    month,
                    day: 1,
                    hour: 12,
                },
            )
            .unwrap();
            assert_eq!(
                fish_ledger.produced[fish],
                if month < 10 { 1280 } else { 224 }
            );
        }
        assert_eq!(apples_by_month, [7, 8, 7, 8]);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The shared fixture checks both pop and building routes."
    )]
    fn overflow_route_order_and_alms_full_store_with_account_room() {
        let mut rules = tiny_rules();
        rules.goods[0].id = "oats".to_owned();
        rules.goods[0].volume_ml = 1;
        rules.goods[0].pop_limit_milli = 1000;
        let pop = |kind: &str, food| PopState {
            kind: kind.to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 1,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![food],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
            level_records: vec![LevelRecord {
                number: 0,
                method: 0,
                ground: Some("pasture".to_owned()),
                paydays: MATURE_PAYDAYS,
                unpaid_work: BTreeMap::new(),
                last_switch_year: None,
            }],
            herds: Vec::new(),
            employment: vec![Employment {
                owners: 0,
                workers: 1,
            }],
            stock: vec![133],
            storehouse_accounts: None,
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: Vec::new(),
            pay_carries: Vec::new(),
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let storehouse = BuildingState {
            kind: "storehouse".to_owned(),
            levels: 1,
            method_levels: vec![1, 0, 0],
            level_records: Vec::new(),
            herds: Vec::new(),
            employment: vec![Employment::default()],
            stock: vec![50],
            storehouse_accounts: Some(StorehouseAccounts {
                pops: vec![vec![0], vec![0]],
                communal: vec![50],
            }),
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: vec![0],
            pay_carries: vec![0],
            fee_carries: vec![vec![0]; 3],
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        // The account takes first, followed by the giver's workplace, the church and alms.
        let mut route_rules = rules.clone();
        route_rules.goods[0].pop_limit_milli = 450_000;
        let mut church_type = route_rules.buildings[0].clone();
        church_type.id = "holdings".to_owned();
        church_type.church = true;
        route_rules.buildings.push(church_type);
        let mut workplace = farm.clone();
        workplace.stock[0] = 58;
        let mut church = farm.clone();
        church.kind = "holdings".to_owned();
        church.stock[0] = 728;
        church.employment = vec![
            Employment::default(),
            Employment {
                owners: 0,
                workers: 1,
            },
        ];
        let mut account = storehouse.clone();
        account.stock[0] = 22_498;
        account.storehouse_accounts.as_mut().unwrap().communal[0] = 22_498;
        let mut route_pops = vec![pop("labourer", 450), pop("peasant", 0)];
        let mut route_buildings = vec![workplace, church, account];
        let lost = route_pop_surplus(&route_rules, &mut route_pops, &mut route_buildings, 0, 0, 8)
            .unwrap();
        assert_eq!(lost, 0);
        assert_eq!(
            route_buildings[2]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .pops[0][0],
            2
        );
        assert_eq!(route_buildings[0].stock[0], 60);
        assert_eq!(route_buildings[1].stock[0], 730);
        assert_eq!(route_pops[1].stock[0], 2);

        // Building overhead retains its old alms-then-communal route, skipping the church.
        let mut source = farm.clone();
        source.stock[0] = 60;
        let mut church = farm.clone();
        church.kind = "holdings".to_owned();
        church.stock[0] = 728;
        let mut empty_storehouse = storehouse.clone();
        empty_storehouse.stock[0] = 0;
        empty_storehouse
            .storehouse_accounts
            .as_mut()
            .unwrap()
            .communal[0] = 0;
        let mut overhead_pops = vec![pop("labourer", 450), pop("labourer", 449)];
        let mut overhead_buildings = vec![source, church, empty_storehouse];
        let mut overhead_counters = CounterBuckets::default();
        let mut overhead_ledger = PlaceProductionLedger::new("fort", 1, 3);
        settle_overflow(
            &route_rules,
            &mut overhead_pops,
            &mut overhead_buildings,
            0,
            0,
            &[(None, 2)],
            &mut overhead_counters,
            &mut overhead_ledger,
        )
        .unwrap();
        assert_eq!(overhead_pops[1].stock[0], 450);
        assert_eq!(overhead_buildings[1].stock[0], 728);
        assert_eq!(
            overhead_buildings[2]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .communal[0],
            1
        );

        // Ownership takes precedence over a workplace, even when the workplace has room.
        let mut owned = farm.clone();
        owned.stock[0] = 58;
        owned.employment[0] = Employment {
            owners: 1,
            workers: 1,
        };
        let mut work_only = farm.clone();
        work_only.stock[0] = 0;
        let mut owner_pops = vec![pop("labourer", 450), pop("peasant", 450)];
        let mut owned_buildings = vec![owned, work_only];
        assert_eq!(
            route_pop_surplus(&route_rules, &mut owner_pops, &mut owned_buildings, 0, 0, 4)
                .unwrap(),
            2
        );
        assert_eq!(owned_buildings[0].stock[0], 60);
        assert_eq!(owned_buildings[1].stock[0], 0);

        let mut split_pops = vec![pop("peasant", 450), pop("peasant", 450)];
        let mut split_buildings = vec![farm.clone(), farm.clone(), farm.clone()];
        for (building, initial) in split_buildings.iter_mut().zip([59, 40, 40]) {
            building.stock[0] = initial;
            building.employment[0].owners = 1;
        }
        assert_eq!(
            route_pop_surplus(
                &route_rules,
                &mut split_pops,
                &mut split_buildings,
                0,
                0,
                12
            )
            .unwrap(),
            0
        );
        assert_eq!(
            split_buildings
                .iter()
                .map(|b| b.stock[0])
                .collect::<Vec<_>>(),
            [60, 46, 45]
        );
        let mut weighted = vec![farm.clone(), farm.clone()];
        for building in &mut weighted {
            building.stock[0] = 0;
        }
        weighted[0].employment[0].owners = 2;
        weighted[1].employment[0].owners = 1;
        assert_eq!(
            route_pop_surplus(&route_rules, &mut split_pops, &mut weighted, 0, 0, 6).unwrap(),
            0
        );
        assert_eq!((weighted[0].stock[0], weighted[1].stock[0]), (4, 2));

        // A worker with no ownership splits by workers; no church is present.
        let mut workplaces = vec![farm.clone(), farm.clone()];
        for (building, workers) in workplaces.iter_mut().zip([2, 1]) {
            building.stock[0] = 0;
            building.employment[0] = Employment { owners: 0, workers };
        }
        let mut full_pops = vec![pop("labourer", 450), pop("peasant", 450)];
        assert_eq!(
            route_pop_surplus(&route_rules, &mut full_pops, &mut workplaces, 0, 0, 6).unwrap(),
            0
        );
        assert_eq!((workplaces[0].stock[0], workplaces[1].stock[0]), (4, 2));

        // A worker at another storehouse gives to its communal account after the first is full.
        let mut worker_store = storehouse.clone();
        worker_store.stock[0] = 0;
        worker_store.storehouse_accounts.as_mut().unwrap().communal[0] = 0;
        worker_store.employment[0].workers = 1;
        let mut full_store = storehouse.clone();
        full_store.stock[0] = 22_500;
        full_store.storehouse_accounts.as_mut().unwrap().communal[0] = 22_500;
        let mut stores = vec![full_store, worker_store];
        assert_eq!(
            route_pop_surplus(&route_rules, &mut full_pops, &mut stores, 0, 0, 3).unwrap(),
            0
        );
        let accounts = stores[1].storehouse_accounts.as_ref().unwrap();
        assert_eq!((accounts.pops[0][0], accounts.communal[0]), (0, 3));

        // With no job or ownership, two church buildings share by levels.
        let mut churches = vec![farm.clone(), farm.clone()];
        for (building, levels) in churches.iter_mut().zip([1, 2]) {
            building.kind = "holdings".to_owned();
            building.levels = levels;
            building.stock[0] = 0;
            building.employment = vec![
                Employment::default(),
                Employment {
                    owners: 0,
                    workers: 1,
                },
            ];
        }
        assert_eq!(
            route_pop_surplus(&route_rules, &mut full_pops, &mut churches, 0, 0, 6).unwrap(),
            0
        );
        assert_eq!((churches[0].stock[0], churches[1].stock[0]), (2, 4));

        // An unemployed giver fills the church before giving the balance as alms.
        let mut church = farm.clone();
        church.kind = "holdings".to_owned();
        church.stock[0] = 729;
        church.employment = vec![
            Employment::default(),
            Employment {
                owners: 0,
                workers: 1,
            },
        ];
        let mut unemployed = vec![pop("labourer", 450), pop("labourer", 448)];
        assert_eq!(
            route_pop_surplus(
                &route_rules,
                &mut unemployed,
                std::slice::from_mut(&mut church),
                0,
                0,
                3
            )
            .unwrap(),
            0
        );
        assert_eq!(church.stock[0], 730);
        assert_eq!(unemployed[1].stock[0], 450);

        // A church already filled as the giver's workplace is not retried; alms follows.
        let mut church = farm.clone();
        church.kind = "holdings".to_owned();
        church.stock[0] = 729;
        let mut church_pops = vec![pop("labourer", 450), pop("labourer", 449)];
        assert_eq!(
            route_pop_surplus(
                &route_rules,
                &mut church_pops,
                std::slice::from_mut(&mut church),
                0,
                0,
                3
            )
            .unwrap(),
            1
        );
        assert_eq!(church.stock[0], 730);
        assert_eq!(church_pops[1].stock[0], 450);

        // A full recipient's empty account is not a destination for alms.
        let mut empty_store = storehouse.clone();
        empty_store.stock[0] = 0;
        empty_store.storehouse_accounts.as_mut().unwrap().communal[0] = 0;
        let mut alms_pops = vec![pop("labourer", 450), pop("labourer", 450)];
        // Fill the giver's account to leave space only in the recipient's account.
        empty_store.stock[0] = 22_500;
        empty_store.storehouse_accounts.as_mut().unwrap().pops[0][0] = 22_500;
        assert_eq!(
            route_pop_surplus(
                &route_rules,
                &mut alms_pops,
                std::slice::from_mut(&mut empty_store),
                0,
                0,
                1
            )
            .unwrap(),
            1
        );
        assert_eq!(
            empty_store.storehouse_accounts.as_ref().unwrap().pops[1][0],
            0
        );

        // In the alms path, a full recipient is skipped even though an account has room.
        let mut source = farm.clone();
        source.stock[0] = 60;
        let mut room = storehouse.clone();
        room.stock[0] = 0;
        room.storehouse_accounts.as_mut().unwrap().communal[0] = 0;
        let mut probe = room.clone();
        assert_eq!(
            receive_storehouse(&route_rules, &mut probe, Some(1), 0, 1).unwrap(),
            0
        );
        let mut destinations = vec![source, room];
        let mut alms_pops = vec![pop("labourer", 450), pop("labourer", 450)];
        let mut counters = CounterBuckets::default();
        let mut ledger = PlaceProductionLedger::new("fort", 1, 2);
        settle_overflow(
            &route_rules,
            &mut alms_pops,
            &mut destinations,
            0,
            0,
            &[(None, 1)],
            &mut counters,
            &mut ledger,
        )
        .unwrap();
        let accounts = destinations[1].storehouse_accounts.as_ref().unwrap();
        assert_eq!(accounts.pops[1][0], 0);
        assert_eq!(accounts.communal[0], 1);

        // Exercise each destination with two units of room, then make each one full in turn.
        for full in 0..=5 {
            let mut ordered_pops = vec![
                pop("labourer", if full == 1 { 450 } else { 448 }),
                pop("labourer", if full == 4 { 450 } else { 448 }),
            ];
            let mut owned = farm.clone();
            owned.stock[0] = if full == 2 { 60 } else { 58 };
            owned.employment[0].owners = 1;
            let mut church = farm.clone();
            church.kind = "holdings".to_owned();
            church.stock[0] = if full == 3 { 730 } else { 728 };
            church.employment = vec![
                Employment::default(),
                Employment {
                    owners: 0,
                    workers: 1,
                },
            ];
            let mut account = storehouse.clone();
            account.stock[0] = if full == 0 { 22_500 } else { 22_498 };
            account.storehouse_accounts.as_mut().unwrap().communal[0] = account.stock[0];
            let mut ordered = vec![owned, church, account];
            let extra = receive_pop(&route_rules, &mut ordered_pops[0], 0, 12).unwrap();
            let mut losses = CounterBuckets::default();
            let mut ledger = PlaceProductionLedger::new("fort", 1, 3);
            settle_overflow(
                &route_rules,
                &mut ordered_pops,
                &mut ordered,
                0,
                0,
                &[(Some(0), extra)],
                &mut losses,
                &mut ledger,
            )
            .unwrap();
            assert_eq!(ordered_pops[0].stock[0], 450);
            assert_eq!(ordered[0].stock[0], 60);
            assert_eq!(ordered[1].stock[0], 730);
            assert_eq!(ordered_pops[1].stock[0], 450);
            assert_eq!(
                ordered[2].storehouse_accounts.as_ref().unwrap().pops[0][0],
                if full == 0 { 0 } else { 2 }
            );
            let lost = if full == 5 { 2 } else { 4 };
            assert_eq!(ledger.lost[0], lost);
            assert_eq!(losses.current.losses, lost);
            assert_eq!(ordered_pops[0].counters.current.losses, lost);
        }

        let mut pops = vec![pop("labourer", 1), pop("peasant", 0)];
        let mut buildings = vec![farm, storehouse];
        let mut counters = CounterBuckets::default();
        let mut ledger = PlaceProductionLedger::new("fort", 1, 3);
        settle_overflow(
            &rules,
            &mut pops,
            &mut buildings,
            0,
            0,
            &[(Some(0), 10)],
            &mut counters,
            &mut ledger,
        )
        .unwrap();
        assert_eq!(pops[0].stock, [1], "the source labourer is already full");
        assert_eq!(pops[1].stock, [1], "the hungry pop receives one alms unit");
        assert_eq!(
            buildings[1].stock,
            [50],
            "the full storehouse accepts nothing"
        );
        assert_eq!(ledger.lost, [9]);
        assert_eq!(counters.current.losses, 9);
        assert_eq!(pops[0].counters.current.losses, 9);
    }

    #[test]
    fn intake_curve_and_joint_apportionment_cover_boundaries_without_overdraw() {
        let curve = [[30, 500], [120, 800], [240, 1000]];
        assert_eq!(curve_value(&curve, 240).unwrap(), 1000);
        assert_eq!(curve_value(&curve, 120).unwrap(), 800);
        assert_eq!(curve_value(&curve, 30).unwrap(), 500);
        assert_eq!(curve_value(&curve, 0).unwrap(), 500);

        for (total, holdings) in [
            (7, vec![2, 3, 4]),
            (19, vec![6, 10, 15]),
            (17, vec![8, 8, 8]),
            (20, vec![2, 7, 11]),
        ] {
            let apportioned = apportion_food(total, &holdings).unwrap();
            assert_eq!(apportioned.iter().sum::<u128>(), total);
            assert!(
                apportioned
                    .iter()
                    .zip(&holdings)
                    .all(|(eaten, held)| eaten <= held)
            );
        }
        assert_eq!(apportion_food(24, &[8, 8, 8]).unwrap(), [8, 8, 8]);
    }

    #[test]
    fn needs_drift_reverses_without_carrying_the_old_direction() {
        let mut carry = 500;
        assert_eq!(
            drift_value(100, 0, 50, 1000, &mut carry, 0, 1000).unwrap(),
            95
        );
        assert_eq!(carry, 0);
        assert_eq!(
            drift_value(95, 100, 50, 1000, &mut carry, 0, 1000).unwrap(),
            95
        );
        assert_eq!(carry, 250);
        assert_eq!(
            satisfaction_baseline(
                &PopType {
                    id: "peasant".to_owned(),
                    dependent_pm: 400,
                    births_pm_year: 0,
                    worker_deaths_pm_year: 0,
                    dependent_deaths_pm_year: 0,
                    necessities_expectation_pm: 1000,
                    niceties_expectation_pm: 0,
                    luxuries_expectation_pm: 0,
                    fuel_need_milli_day: 0,
                },
                [400, 0, 0]
            ),
            -80
        );
    }

    #[test]
    fn satisfaction_baseline_covers_both_niceties_sides_luxuries_extra_and_clamp() {
        let mut labourer = tiny_rules().pop_types[0].clone();
        labourer.id = "labourer".to_owned();
        labourer.niceties_expectation_pm = 300;
        assert_eq!(satisfaction_baseline(&labourer, [1000, 200, 0]), -3);
        assert_eq!(satisfaction_baseline(&labourer, [1000, 400, 200]), 18);
        labourer.niceties_expectation_pm = 10_000;
        assert_eq!(satisfaction_baseline(&labourer, [0, 0, 0]), -100);
    }

    #[test]
    fn the_equilibrium_is_the_baseline_of_the_pops_present_needs_and_none_for_an_unknown_kind() {
        let rules = tiny_rules();
        let mut pop = PopState {
            kind: rules.pop_types[0].id.clone(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 0,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        pop.consumption.necessities_pm = 1000;
        pop.consumption.niceties_pm = 200;
        pop.consumption.luxuries_pm = 0;
        assert_eq!(
            satisfaction_equilibrium(&rules, &pop),
            Some(satisfaction_baseline(&rules.pop_types[0], [1000, 200, 0]))
        );
        pop.consumption.necessities_pm = 0;
        assert_eq!(satisfaction_equilibrium(&rules, &pop), Some(-100));
        pop.kind = "no such kind".to_owned();
        assert_eq!(satisfaction_equilibrium(&rules, &pop), None);
    }

    #[test]
    fn monthly_satisfaction_drift_runs_on_day_one_only() {
        let rules = tiny_rules();
        let pop = PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 0,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![0],
            consumption: PopConsumption {
                pantry_hunger_centi: vec![0],
                ticking_carries: vec![0],
                ..PopConsumption::default()
            },
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "empty".to_owned(),
                pops: vec![pop],
                buildings: Vec::new(),
                prices_milli: vec![1000],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: vec![0],
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        };
        run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 1,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(state.places[0].pops[0].consumption.satisfaction, 6);
        run_daily_economy(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(state.places[0].pops[0].consumption.satisfaction, 6);
    }

    #[test]
    fn daily_eating_opens_only_whole_goods_and_drifts_needs() {
        let rules = tiny_rules();
        let goods = rules.goods.len();
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "test".to_owned(),
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
                    literacy_pm: 0,
                    workers: 10,
                    dependents: 0,
                    employment: Vec::new(),
                    stock: vec![6],
                    consumption: PopConsumption {
                        pantry_hunger_centi: vec![0; goods],
                        ticking_carries: vec![0; goods],
                        ..PopConsumption::default()
                    },
                    counters: CounterBuckets::default(),
                    short_on_rent: false,
                    housed_last: false,
                    genetics: PopGenetics::default(),
                    pending_births: 0,
                    pending_deaths: 0,
                }],
                buildings: Vec::new(),
                prices_milli: vec![100_000],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        };
        let intake = run_daily_consumption(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!(pop.stock, [1]);
        assert_eq!(intake, [vec![500]]);
        assert_eq!(pop.consumption.necessities_centi, 0);
        assert_eq!(pop.consumption.necessities_pm, 25);
    }

    #[test]
    fn two_good_food_split_tracks_oats_and_barley_hunger_proportionally() {
        let mut rules = tiny_rules();
        rules.seasons.remove("food");
        rules.goods[0].id = "oats".to_owned();
        rules.goods[0].hunger_centi = 1800;
        rules.goods[0].necessities_centi = 1800;
        let mut barley = rules.goods[0].clone();
        barley.id = "barley".to_owned();
        barley.hunger_centi = 2800;
        barley.necessities_centi = 2800;
        rules.goods.push(barley);
        rules.buildings[0].methods[0].annual_output = BTreeMap::from([("oats".to_owned(), 1)]);
        rules.buildings[0].methods[0].score_share_pm = BTreeMap::from([("oats".to_owned(), 1000)]);
        rules.diet_profiles.insert(
            "peasant".to_owned(),
            BTreeMap::from([("oats".to_owned(), 1000)]),
        );
        rules.suitability.insert(
            "pasture".to_owned(),
            BTreeMap::from([("crops".to_owned(), 1000), ("oats".to_owned(), 1000)]),
        );
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "test".to_owned(),
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
                    literacy_pm: 0,
                    workers: 1,
                    dependents: 0,
                    employment: Vec::new(),
                    stock: vec![1, 1],
                    consumption: PopConsumption {
                        pantry_hunger_centi: vec![0; 2],
                        ticking_carries: vec![0; 2],
                        ..PopConsumption::default()
                    },
                    counters: CounterBuckets::default(),
                    short_on_rent: false,
                    housed_last: false,
                    genetics: PopGenetics::default(),
                    pending_births: 0,
                    pending_deaths: 0,
                }],
                buildings: Vec::new(),
                prices_milli: vec![1000; 2],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        };
        run_daily_consumption(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!(pop.stock, [0, 0]);
        assert_eq!(pop.consumption.pantry_hunger_centi, [1778, 2767]);
        assert_eq!(pop.consumption.necessities_centi, 4500);
        assert_eq!(pop.consumption.necessities_pm, 50);
    }

    #[test]
    fn zero_population_skips_food_and_ticking_divisions() {
        let mut rules = tiny_rules();
        rules.goods.push(Good {
            id: "herbs".to_owned(),
            tags: Vec::new(),
            supplies: BTreeMap::new(),
            unit: "pound".to_owned(),
            weight_g: 1,
            volume_ml: 1,
            stacked_outside: false,
            hunger_centi: 0,
            necessities_centi: 0,
            niceties_centi: 10,
            luxuries_centi: 0,
            base_price: 1,
            pop_limit_milli: 1000,
        });
        let goods = rules.goods.len();
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "test".to_owned(),
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
                    literacy_pm: 0,
                    workers: 0,
                    dependents: 0,
                    employment: Vec::new(),
                    stock: vec![0, 1],
                    consumption: PopConsumption {
                        pantry_hunger_centi: vec![0; goods],
                        ticking_carries: vec![0; goods],
                        ..PopConsumption::default()
                    },
                    counters: CounterBuckets::default(),
                    short_on_rent: false,
                    housed_last: false,
                    genetics: PopGenetics::default(),
                    pending_births: 0,
                    pending_deaths: 0,
                }],
                buildings: Vec::new(),
                prices_milli: vec![1000; goods],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: vec![0; goods],
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        };
        run_daily_consumption(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!(pop.stock[1], 0);
        assert_eq!(pop.consumption.niceties_pm, 1000);
    }

    #[test]
    fn missing_ticking_rate_is_refused_by_good_name() {
        let mut rules = tiny_rules();
        rules.goods.push(Good {
            id: "herbs".to_owned(),
            tags: Vec::new(),
            supplies: BTreeMap::new(),
            unit: "pound".to_owned(),
            weight_g: 1,
            volume_ml: 1,
            stacked_outside: false,
            hunger_centi: 0,
            necessities_centi: 0,
            niceties_centi: 10,
            luxuries_centi: 0,
            base_price: 1,
            pop_limit_milli: 1000,
        });
        let pop = PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 1,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![1, 1],
            consumption: PopConsumption {
                pantry_hunger_centi: vec![0; 2],
                ticking_carries: vec![0; 2],
                ..PopConsumption::default()
            },
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "test".to_owned(),
                pops: vec![pop.clone()],
                buildings: Vec::new(),
                prices_milli: vec![1000, 1000],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        };
        let error = run_daily_consumption(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap_err();
        assert_eq!(error.entry, "ticking_rates_million.herbs.peasant");
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "This fixture checks the three required food drawing sources in their exact priority order."
    )]
    fn drawing_uses_the_pop_account_before_owner_stock_and_communal_account() {
        let mut rules = tiny_rules();
        rules.goods[0].pop_limit_milli = 100_000;
        let goods = rules.goods.len();
        let mut pop = PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 1,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![59],
            consumption: PopConsumption {
                pantry_hunger_centi: vec![0; goods],
                ticking_carries: vec![0; goods],
                ..PopConsumption::default()
            },
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        pop.employment.push(Employment {
            owners: 1,
            workers: 0,
        });
        let storehouse = BuildingState {
            kind: "storehouse".to_owned(),
            levels: 1,
            method_levels: vec![1, 0, 0],
            level_records: Vec::new(),
            herds: Vec::new(),
            employment: vec![Employment::default()],
            stock: vec![2],
            storehouse_accounts: Some(StorehouseAccounts {
                pops: vec![vec![1]],
                communal: vec![1],
            }),
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: vec![0],
            pay_carries: vec![0],
            fee_carries: vec![vec![0], vec![0]],
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
            level_records: vec![LevelRecord {
                number: 0,
                method: 0,
                ground: Some("pasture".to_owned()),
                paydays: MATURE_PAYDAYS,
                unpaid_work: BTreeMap::new(),
                last_switch_year: None,
            }],
            herds: vec![0, 0, 0],
            employment: vec![Employment {
                owners: 1,
                workers: 0,
            }],
            stock: vec![1],
            storehouse_accounts: None,
            production_carries: vec![vec![0]],
            method_suitability: vec![vec![1000]],
            herd_birth_carries: vec![0; 3],
            herd_death_carries: vec![0; 3],
            herd_product_carries: vec![0],
            herd_grazing_sums: vec![1000; 3],
            wage_carries: vec![0],
            pay_carries: vec![0],
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let mut place = PlaceState {
            id: "test".to_owned(),
            pops: vec![pop],
            buildings: vec![storehouse, farm],
            prices_milli: vec![1000],
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: Vec::new(),
            last_market_orders: None,
            counters: CounterBuckets::default(),
        };
        draw_pop(&rules, &mut place, 0).unwrap();
        assert_eq!(place.pops[0].stock, [60]);
        consume_pop(&rules, &mut place, 0).unwrap();
        assert_eq!(
            place.buildings[0]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .pops[0],
            [0]
        );
        assert_eq!(place.buildings[1].stock, [1], "own account has priority");
        assert_eq!(
            place.buildings[0]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .communal,
            [1]
        );
        draw_pop(&rules, &mut place, 0).unwrap();
        assert_eq!(place.pops[0].stock, [60]);
        consume_pop(&rules, &mut place, 0).unwrap();
        assert_eq!(
            place.buildings[1].stock,
            [0],
            "owner stock follows its account"
        );
        assert_eq!(
            place.buildings[0]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .communal,
            [1]
        );
        draw_pop(&rules, &mut place, 0).unwrap();
        assert_eq!(
            place.buildings[0]
                .storehouse_accounts
                .as_ref()
                .unwrap()
                .communal,
            [0]
        );
    }

    #[test]
    fn zero_hunger_tick_spends_goods_and_contributes_tier_benefits() {
        let mut rules = tiny_rules();
        rules.goods.push(Good {
            id: "herbs".to_owned(),
            tags: Vec::new(),
            supplies: BTreeMap::new(),
            unit: "pound".to_owned(),
            weight_g: 1,
            volume_ml: 1,
            stacked_outside: false,
            hunger_centi: 0,
            necessities_centi: 0,
            niceties_centi: 100,
            luxuries_centi: 0,
            base_price: 1,
            pop_limit_milli: 1000,
        });
        rules.ticking_rates_million.insert(
            "herbs".to_owned(),
            BTreeMap::from([("peasant".to_owned(), 500_000)]),
        );
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "test".to_owned(),
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
                    literacy_pm: 0,
                    workers: 1,
                    dependents: 0,
                    employment: Vec::new(),
                    stock: vec![0, 1],
                    consumption: PopConsumption {
                        pantry_hunger_centi: vec![0; 2],
                        ticking_carries: vec![0; 2],
                        ..PopConsumption::default()
                    },
                    counters: CounterBuckets::default(),
                    short_on_rent: false,
                    housed_last: false,
                    genetics: PopGenetics::default(),
                    pending_births: 0,
                    pending_deaths: 0,
                }],
                buildings: Vec::new(),
                prices_milli: vec![1000, 1000],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        };
        run_daily_consumption(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(state.places[0].pops[0].stock[1], 1);
        assert_eq!(
            state.places[0].pops[0].consumption.ticking_carries[1],
            500_000
        );
        run_daily_consumption(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 3,
                hour: 12,
            },
        )
        .unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!(pop.stock[1], 0);
        assert_eq!(pop.consumption.ticking_carries[1], 0);
        assert_eq!(pop.consumption.niceties_pm, 50);
    }

    #[test]
    fn wool_and_hides_are_held_without_ticking_or_niceties() {
        let mut rules = tiny_rules();
        for id in ["wool", "hides"] {
            rules.goods.push(Good {
                id: id.to_owned(),
                tags: Vec::new(),
                supplies: BTreeMap::new(),
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
                stacked_outside: false,
                hunger_centi: 0,
                necessities_centi: 0,
                niceties_centi: 0,
                luxuries_centi: 0,
                base_price: 1,
                pop_limit_milli: 100_000,
            });
        }
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "test".to_owned(),
                pops: vec![PopState {
                    kind: "peasant".to_owned(),
                    culture: "connachta".to_owned(),
                    religion: "christianity".to_owned(),
                    literacy_pm: 0,
                    workers: 1,
                    dependents: 0,
                    employment: Vec::new(),
                    stock: vec![0, 3, 2],
                    consumption: PopConsumption {
                        pantry_hunger_centi: vec![0; 3],
                        ticking_carries: vec![0; 3],
                        ..PopConsumption::default()
                    },
                    counters: CounterBuckets::default(),
                    short_on_rent: false,
                    housed_last: false,
                    genetics: PopGenetics::default(),
                    pending_births: 0,
                    pending_deaths: 0,
                }],
                buildings: Vec::new(),
                prices_milli: vec![1000; 3],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: Vec::new(),
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        };
        run_daily_consumption(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!(pop.stock, [0, 3, 2]);
        assert_eq!(pop.consumption.niceties_centi, 0);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Both ordinary breeding-floor protection and the starvation exception share one literal herd fixture."
    )]
    fn emergency_butchering_respects_and_can_waive_the_breeding_floor() {
        let mut rules = tiny_rules();
        for (id, hunger) in [("beef", 10), ("pork", 10), ("mutton", 10), ("hides", 0)] {
            rules.goods.push(Good {
                id: id.to_owned(),
                tags: Vec::new(),
                supplies: BTreeMap::new(),
                unit: "unit".to_owned(),
                weight_g: 1,
                volume_ml: 1,
                stacked_outside: false,
                hunger_centi: hunger,
                necessities_centi: hunger,
                niceties_centi: 0,
                luxuries_centi: 0,
                base_price: 1,
                pop_limit_milli: 100_000,
            });
        }
        install_fixture_herds(&mut rules);
        let make = |necessities_pm| {
            let goods = rules.goods.len();
            let pop = PopState {
                kind: "peasant".to_owned(),
                culture: "connachta".to_owned(),
                religion: "christianity".to_owned(),
                literacy_pm: 0,
                workers: 1,
                dependents: 0,
                employment: vec![Employment {
                    owners: 1,
                    workers: 0,
                }],
                stock: vec![0; goods],
                consumption: PopConsumption {
                    pantry_hunger_centi: vec![0; goods],
                    ticking_carries: vec![0; goods],
                    necessities_pm,
                    ..PopConsumption::default()
                },
                counters: CounterBuckets::default(),
                short_on_rent: false,
                housed_last: false,
                genetics: PopGenetics::default(),
                pending_births: 0,
                pending_deaths: 0,
            };
            let building = BuildingState {
                kind: "farm".to_owned(),
                levels: 1,
                method_levels: vec![1],
                level_records: vec![LevelRecord {
                    number: 0,
                    method: 0,
                    ground: Some("pasture".to_owned()),
                    paydays: MATURE_PAYDAYS,
                    unpaid_work: BTreeMap::new(),
                    last_switch_year: None,
                }],
                herds: vec![0, 2, 0],
                employment: vec![Employment {
                    owners: 1,
                    workers: 0,
                }],
                stock: vec![0; goods],
                storehouse_accounts: None,
                production_carries: vec![vec![0]],
                method_suitability: vec![vec![1000]],
                herd_birth_carries: vec![0; 3],
                herd_death_carries: vec![0; 3],
                herd_product_carries: vec![0; goods],
                herd_grazing_sums: vec![1000; 3],
                wage_carries: vec![0; goods],
                pay_carries: vec![0; goods],
                fee_carries: Vec::new(),
                wage_history: vec![0; 12],
                paid_history: vec![0; WAGE_HISTORY_MONTHS],
                paid_years: vec![0; WAGE_HISTORY_MONTHS],
            };
            PlaceState {
                id: "test".to_owned(),
                pops: vec![pop],
                buildings: vec![building],
                prices_milli: vec![1000; goods],
                prepared_m2: 0,
                development_pm: 0,
                price_drift_carries: vec![0; goods],
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }
        };
        let protected = make(400);
        let mut protected_state = EconomyState {
            places: vec![protected],
        };
        run_daily_consumption(
            &rules,
            &mut protected_state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        let protected = protected_state.places.remove(0);
        assert_eq!(
            protected.buildings[0].herds[1], 2,
            "the breeding floor is two pigs per farming level"
        );

        let mut starving = make(200);
        let mut state = EconomyState {
            places: vec![starving.clone()],
        };
        run_daily_consumption(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        starving = state.places.remove(0);
        assert_eq!(
            starving.buildings[0].herds[1], 1,
            "necessities below 300‰ waive the floor"
        );
    }

    #[test]
    fn butchering_surplus_reaches_a_later_owner_before_its_meal() {
        let (mut rules, mut state) = daily_accounting_fixture();
        let pork = rules.goods.iter().position(|g| g.id == "pork").unwrap();
        rules.goods[pork].volume_ml = 720_000;
        let herbs = rules.goods.iter().position(|g| g.id == "herbs").unwrap();
        rules
            .herd_species
            .iter_mut()
            .find(|s| s.animal == "pigs")
            .unwrap()
            .yields
            .push(HerdYield {
                good: "herbs".to_owned(),
                units: 1,
            });
        let place = &mut state.places[0];
        place.buildings.remove(0);
        let mut staff = place.pops[0].clone();
        staff.workers = 1_000;
        staff.stock.fill(0);
        staff.employment = vec![Employment {
            owners: 0,
            workers: 1_000,
        }];
        for pop in &mut place.pops {
            pop.stock.fill(0);
            pop.consumption.necessities_pm = 200;
            pop.employment = vec![Employment {
                owners: 1,
                workers: 0,
            }];
        }
        place.pops.push(staff);
        let farm = &mut place.buildings[0];
        farm.herds = vec![0, 2, 0];
        farm.employment = vec![
            Employment {
                owners: 1,
                workers: 0,
            },
            Employment {
                owners: 1,
                workers: 0,
            },
            Employment {
                owners: 0,
                workers: 1_000,
            },
        ];
        let (_, ledger) = run_daily_consumption_with_ledger(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 1,
                day: 2,
                hour: 12,
            },
        )
        .unwrap();
        assert_eq!(ledger.herd_slaughter, [0, 1, 0]);
        assert_eq!(ledger.butchered[pork], 88);
        assert_eq!(ledger.butchered[herbs], 1);
        assert_eq!(ledger.lost[herbs], 0);
        assert_eq!(state.places[0].buildings[0].herds[1], 1);
        assert!(
            ledger.opened[pork] >= 2,
            "both owners ate pork from the one slaughter"
        );
    }

    #[test]
    fn emergency_butchering_uses_pig_then_sheep_order_and_stops_at_threshold() {
        let (mut rules, mut state) = daily_accounting_fixture();
        let pork = rules
            .goods
            .iter()
            .position(|good| good.id == "pork")
            .unwrap();
        rules.goods[pork].pop_limit_milli = 100_000;
        let place = &mut state.places[0];
        place.buildings[1].herds = vec![0, 2, 1];
        let mut ledger = DailyConsumptionLedger {
            butchered: vec![0; rules.goods.len()],
            opened: vec![0; rules.goods.len()],
            ticked: vec![0; rules.goods.len()],
            lost: vec![0; rules.goods.len()],
            herd_slaughter: vec![0; 3],
            ..DailyConsumptionLedger::default()
        };
        butcher_emergency(&rules, place, 1, 7000, &mut ledger).unwrap();
        assert_eq!(place.buildings[1].herds, [0, 0, 1]);
        assert_eq!(place.pops[1].stock[pork], 100);
        let mutton = rules
            .goods
            .iter()
            .position(|good| good.id == "mutton")
            .unwrap();
        assert_eq!(place.pops[1].stock[mutton], 0);
        assert_eq!(ledger.butchered[pork], 176);
        assert_eq!(ledger.lost[pork], 0);
    }

    #[test]
    fn emergency_cattle_butchering_accounts_for_the_hide() {
        let (mut rules, mut state) = daily_accounting_fixture();
        let beef = rules
            .goods
            .iter()
            .position(|good| good.id == "beef")
            .unwrap();
        let hides = rules
            .goods
            .iter()
            .position(|good| good.id == "hides")
            .unwrap();
        rules.goods[beef].pop_limit_milli = 100_000;
        let place = &mut state.places[0];
        place.buildings[1].herds = vec![1, 0, 0];
        let mut ledger = DailyConsumptionLedger {
            butchered: vec![0; rules.goods.len()],
            opened: vec![0; rules.goods.len()],
            ticked: vec![0; rules.goods.len()],
            lost: vec![0; rules.goods.len()],
            herd_slaughter: vec![0; 3],
            ..DailyConsumptionLedger::default()
        };
        butcher_emergency(&rules, place, 1, 1, &mut ledger).unwrap();
        assert_eq!(place.buildings[1].herds, [0, 0, 0]);
        assert_eq!(place.pops[1].stock[beef], 100);
        assert_eq!(place.pops[1].stock[hides], 1);
        assert_eq!(ledger.butchered[beef], 220);
        assert_eq!(ledger.butchered[hides], 1);
        assert_eq!(ledger.lost[beef], 20);
    }

    #[test]
    fn emergency_butchering_skips_owned_buildings_without_herds() {
        let rules = tiny_rules();
        let pop = PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 1,
            dependents: 0,
            employment: vec![Employment {
                owners: 1,
                workers: 0,
            }],
            stock: vec![0],
            consumption: PopConsumption {
                pantry_hunger_centi: vec![0],
                necessities_pm: 200,
                ..PopConsumption::default()
            },
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let building = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
            level_records: vec![LevelRecord {
                number: 0,
                method: 0,
                ground: Some("pasture".to_owned()),
                paydays: MATURE_PAYDAYS,
                unpaid_work: BTreeMap::new(),
                last_switch_year: None,
            }],
            herds: Vec::new(),
            employment: vec![Employment {
                owners: 1,
                workers: 0,
            }],
            stock: vec![0],
            storehouse_accounts: None,
            production_carries: vec![vec![0]],
            method_suitability: vec![vec![1000]],
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: vec![0],
            herd_grazing_sums: Vec::new(),
            wage_carries: vec![0],
            pay_carries: vec![0],
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let mut place = PlaceState {
            id: "test".to_owned(),
            pops: vec![pop],
            buildings: vec![building],
            prices_milli: vec![1000],
            prepared_m2: 0,
            development_pm: 0,
            price_drift_carries: Vec::new(),
            last_market_orders: None,
            counters: CounterBuckets::default(),
        };
        let mut ledger = DailyConsumptionLedger {
            butchered: vec![0],
            opened: vec![0],
            ticked: vec![0],
            lost: vec![0],
            herd_slaughter: vec![0; 3],
            ..DailyConsumptionLedger::default()
        };
        butcher_emergency(&rules, &mut place, 0, 50, &mut ledger).unwrap();
        assert!(place.buildings[0].herds.is_empty());
        assert_eq!(place.pops[0].stock, [0]);
    }

    #[test]
    fn housing_fill_splits_non_divisible_overflow_and_preserves_totals() {
        let rules = tiny_rules();
        let pop = PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 27,
            dependents: 0,
            employment: vec![Employment::default()],
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
            level_records: vec![LevelRecord {
                number: 0,
                method: 0,
                ground: Some("pasture".to_owned()),
                paydays: MATURE_PAYDAYS,
                unpaid_work: BTreeMap::new(),
                last_switch_year: None,
            }],
            herds: Vec::new(),
            employment: vec![Employment::default()],
            stock: vec![0],
            storehouse_accounts: None,
            production_carries: vec![vec![0]],
            method_suitability: vec![vec![1000]],
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: vec![0],
            pay_carries: vec![0],
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let place = PlaceState {
            id: "fort".to_owned(),
            pops: vec![pop],
            buildings: vec![farm],
            prices_milli: vec![0],
            prepared_m2: 0,
            development_pm: 1000,
            price_drift_carries: Vec::new(),
            last_market_orders: None,
            counters: CounterBuckets::default(),
        };
        let fill = housing_fill(&rules, &place, 3).unwrap();
        assert_eq!(fill.basic_capacity, 17);
        assert_eq!(fill.makeshift_capacity, 3);
        assert_eq!(fill.pops[0].basic, 23);
        assert_eq!(fill.pops[0].makeshift, 4);
        assert_eq!(fill.pops[0].crowding_pm, Some(1349));
        assert_eq!(fill.basic_occupants, 23);
        assert_eq!(fill.makeshift_occupants, 4);
        assert_eq!(fill.building_occupants, [23]);
        assert_eq!(fill.crowding_pm, 1350);
    }

    fn housing_test_place(
        rules: &mut Rules,
        per_level: u32,
        levels: &[u32],
        populations: &[(u64, u64)],
        employment_by_pop: &[Vec<Employment>],
    ) -> PlaceState {
        rules.housing_per_level.insert("farm".to_owned(), per_level);
        let buildings = levels
            .iter()
            .enumerate()
            .map(|(building_index, level)| BuildingState {
                kind: "farm".to_owned(),
                levels: *level,
                method_levels: vec![1],
                level_records: Vec::new(),
                herds: Vec::new(),
                employment: employment_by_pop
                    .iter()
                    .map(|employment| employment[building_index].clone())
                    .collect(),
                stock: vec![0],
                storehouse_accounts: None,
                production_carries: vec![vec![0]],
                method_suitability: vec![vec![1000]],
                herd_birth_carries: Vec::new(),
                herd_death_carries: Vec::new(),
                herd_product_carries: Vec::new(),
                herd_grazing_sums: Vec::new(),
                wage_carries: vec![0],
                pay_carries: vec![0],
                fee_carries: Vec::new(),
                wage_history: vec![0; 12],
                paid_history: vec![0; WAGE_HISTORY_MONTHS],
                paid_years: vec![0; WAGE_HISTORY_MONTHS],
            })
            .collect::<Vec<_>>();
        let pops = populations
            .iter()
            .zip(employment_by_pop)
            .map(|((workers, dependents), employment)| PopState {
                kind: "peasant".to_owned(),
                culture: "connachta".to_owned(),
                religion: "christianity".to_owned(),
                literacy_pm: 0,
                workers: *workers,
                dependents: *dependents,
                employment: employment.clone(),
                stock: vec![0],
                consumption: PopConsumption::default(),
                counters: CounterBuckets::default(),
                short_on_rent: false,
                housed_last: false,
                genetics: PopGenetics::default(),
                pending_births: 0,
                pending_deaths: 0,
            })
            .collect();
        PlaceState {
            id: "housing-fixture".to_owned(),
            pops,
            buildings,
            prices_milli: vec![1000],
            prepared_m2: 0,
            development_pm: 1000,
            price_drift_carries: Vec::new(),
            last_market_orders: None,
            counters: CounterBuckets::default(),
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The specification requires one shared conservation assertion across all thirteen housing vectors."
    )]
    fn housing_fill_matches_all_spec_vectors() {
        let check = |name: &str,
                     per_level: u32,
                     levels: &[u32],
                     shelter: u64,
                     populations: &[(u64, u64)],
                     jobs: &[Vec<Employment>],
                     expected_basic: &[u64],
                     expected_makeshift: &[u64],
                     expected_buildings: &[u64],
                     expected_pop_crowding: &[Option<u64>],
                     expected_place_crowding: u64| {
            let mut rules = tiny_rules();
            let place = housing_test_place(&mut rules, per_level, levels, populations, jobs);
            let fill = housing_fill(&rules, &place, shelter).unwrap();
            assert_eq!(
                fill.pops.iter().map(|pop| pop.basic).collect::<Vec<_>>(),
                expected_basic,
                "{name}: basic by pop"
            );
            assert_eq!(
                fill.pops
                    .iter()
                    .map(|pop| pop.makeshift)
                    .collect::<Vec<_>>(),
                expected_makeshift,
                "{name}: makeshift by pop"
            );
            assert_eq!(
                fill.building_occupants, expected_buildings,
                "{name}: basic by building"
            );
            assert_eq!(
                fill.pops
                    .iter()
                    .map(|pop| pop.crowding_pm)
                    .collect::<Vec<_>>(),
                expected_pop_crowding,
                "{name}: per-pop crowding"
            );
            assert_eq!(
                fill.crowding_pm, expected_place_crowding,
                "{name}: place crowding"
            );
            assert_eq!(
                fill.basic_occupants,
                expected_basic.iter().sum::<u64>(),
                "{name}: basic occupants conserve population allocation"
            );
            assert_eq!(
                fill.makeshift_occupants,
                expected_makeshift.iter().sum::<u64>(),
                "{name}: makeshift occupants conserve population allocation"
            );
            assert_eq!(
                fill.basic_occupants,
                expected_buildings.iter().sum::<u64>(),
                "{name}: building occupancy conserves basic allocation"
            );
        };

        check(
            "F1 counterexample",
            1,
            &[1],
            0,
            &[(1, 0), (9, 0)],
            &[
                vec![Employment {
                    owners: 0,
                    workers: 1,
                }],
                vec![Employment::default()],
            ],
            &[1, 9],
            &[0, 0],
            &[10],
            &[Some(10_000), Some(10_000)],
            10_000,
        );
        check(
            "zero-capacity basic",
            17,
            &[],
            5,
            &[(8, 0)],
            &[vec![]],
            &[0],
            &[8],
            &[],
            &[Some(1600)],
            1600,
        );
        check(
            "non-divisible overflow",
            17,
            &[1],
            3,
            &[(27, 0)],
            &[vec![Employment::default()]],
            &[23],
            &[4],
            &[23],
            &[Some(1349)],
            1350,
        );
        check(
            "divisible overflow",
            20,
            &[1],
            10,
            &[(45, 0)],
            &[vec![Employment::default()]],
            &[30],
            &[15],
            &[30],
            &[Some(1500)],
            1500,
        );
        check(
            "two unhoused pops",
            17,
            &[1],
            3,
            &[(10, 0), (17, 0)],
            &[vec![Employment::default()], vec![Employment::default()]],
            &[9, 14],
            &[1, 3],
            &[23],
            &[Some(1350), Some(1348)],
            1350,
        );
        check(
            "underfull building",
            28,
            &[1],
            10,
            &[(10, 10), (12, 0)],
            &[
                vec![Employment {
                    owners: 0,
                    workers: 10,
                }],
                vec![Employment::default()],
            ],
            &[20, 8],
            &[0, 4],
            &[28],
            &[Some(1000), Some(800)],
            842,
        );
        check(
            "split employment",
            17,
            &[1, 1],
            0,
            &[(2, 1)],
            &[vec![
                Employment {
                    owners: 0,
                    workers: 1,
                },
                Employment {
                    owners: 0,
                    workers: 1,
                },
            ]],
            &[3],
            &[0],
            &[2, 1],
            &[Some(88)],
            88,
        );
        check(
            "fully employed over two buildings",
            17,
            &[1, 2],
            0,
            &[(24, 16)],
            &[vec![
                Employment {
                    owners: 0,
                    workers: 12,
                },
                Employment {
                    owners: 0,
                    workers: 12,
                },
            ]],
            &[40],
            &[0],
            &[17, 23],
            &[Some(784)],
            784,
        );
        check(
            "partial employment",
            17,
            &[1],
            0,
            &[(4, 6)],
            &[vec![Employment {
                owners: 0,
                workers: 3,
            }]],
            &[10],
            &[0],
            &[10],
            &[Some(588)],
            588,
        );
        check(
            "zero workers",
            17,
            &[1],
            0,
            &[(0, 5)],
            &[vec![Employment::default()]],
            &[5],
            &[0],
            &[5],
            &[Some(294)],
            294,
        );
        check(
            "two pops over one building",
            17,
            &[1],
            10,
            &[(6, 6), (4, 3)],
            &[
                vec![Employment {
                    owners: 0,
                    workers: 6,
                }],
                vec![Employment {
                    owners: 0,
                    workers: 4,
                }],
            ],
            &[11, 6],
            &[1, 1],
            &[17],
            &[Some(933), Some(885)],
            703,
        );
        check(
            "no housing",
            17,
            &[],
            0,
            &[(30, 0)],
            &[vec![]],
            &[0],
            &[0],
            &[],
            &[Some(2000)],
            2000,
        );
        check(
            "a pop died off",
            17,
            &[1],
            5,
            &[(0, 0), (12, 8)],
            &[
                vec![Employment::default()],
                vec![Employment {
                    owners: 0,
                    workers: 10,
                }],
            ],
            &[0, 17],
            &[0, 3],
            &[17],
            &[None, Some(940)],
            909,
        );
    }

    #[test]
    fn monastery_owners_use_basic_first_and_shelter_only_after_basic_is_full() {
        let mut rules = tiny_rules();
        rules.housing_per_level.insert("farm".to_owned(), 14);
        let mut place = housing_test_place(
            &mut rules,
            14,
            &[1],
            &[(5, 0), (5, 0), (6, 0)],
            &[
                vec![Employment {
                    owners: 0,
                    workers: 5,
                }],
                vec![Employment {
                    owners: 0,
                    workers: 5,
                }],
                vec![Employment::default()],
            ],
        );
        place.pops[0].kind = "monk".to_owned();
        place.pops[1].kind = "labourer".to_owned();
        let fill = housing_fill(&rules, &place, 3).unwrap();
        assert_eq!(
            fill.pops.iter().map(|pop| pop.basic).collect::<Vec<_>>(),
            [5, 5, 4],
            "the monastery's own workers are housed before the homeless peasant"
        );
        assert_eq!(
            fill.pops
                .iter()
                .map(|pop| pop.makeshift)
                .collect::<Vec<_>>(),
            [0, 0, 2],
            "makeshift shelter receives people only after all 14 basic spaces are filled"
        );
        assert_eq!(fill.basic_occupants, 14);
        assert_eq!(fill.makeshift_occupants, 2);
        assert_eq!(fill.building_occupants, [14]);
    }

    #[test]
    fn monthly_demography_keeps_exact_fractional_rate_carries() {
        let rules = tiny_rules();
        let pop = PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 120,
            dependents: 80,
            employment: vec![Employment::default()],
            stock: vec![0],
            consumption: PopConsumption {
                pantry_hunger_centi: vec![0],
                ticking_carries: vec![0],
                necessities_pm: 1000,
                ..PopConsumption::default()
            },
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let farm = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
            level_records: vec![LevelRecord {
                number: 0,
                method: 0,
                ground: Some("pasture".to_owned()),
                paydays: MATURE_PAYDAYS,
                unpaid_work: BTreeMap::new(),
                last_switch_year: None,
            }],
            herds: Vec::new(),
            employment: vec![Employment::default()],
            stock: vec![0],
            storehouse_accounts: None,
            production_carries: vec![vec![0]],
            method_suitability: vec![vec![1000]],
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: vec![0],
            pay_carries: vec![0],
            fee_carries: Vec::new(),
            wage_history: vec![0; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "fort".to_owned(),
                pops: vec![pop],
                buildings: vec![farm],
                prices_milli: vec![0],
                prepared_m2: 0,
                development_pm: 1000,
                price_drift_carries: Vec::new(),
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        };
        run_monthly_demography(&rules, &mut state, &BTreeMap::new(), &BTreeMap::new(), None)
            .unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!((pop.workers, pop.dependents), (120, 80));
        assert_eq!(pop.counters.current.births, 0);
        assert_eq!(pop.counters.current.deaths, 0);
        assert_eq!(pop.consumption.birth_carry, 8_040_000_000_000_000);
        assert_eq!(pop.consumption.worker_death_carry, 2_400_000_000_000);
        assert_eq!(pop.consumption.dependent_death_carry, 4_800_000_000_000_000);
    }

    #[test]
    fn character_floor_clips_mortality_and_books_only_actual_deaths() {
        let mut rules = tiny_rules();
        rules.pop_types[0].births_pm_year = 0;
        rules.pop_types[0].worker_deaths_pm_year = 12_000;
        rules.pop_types[0].dependent_deaths_pm_year = 12_000;
        let pop = PopState {
            kind: "peasant".into(),
            culture: "connachta".into(),
            religion: "christianity".into(),
            literacy_pm: 0,
            workers: 4,
            dependents: 3,
            employment: Vec::new(),
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let place = PlaceState {
            id: "floor".into(),
            pops: vec![pop],
            buildings: Vec::new(),
            prices_milli: vec![1000],
            prepared_m2: 0,
            development_pm: 1000,
            price_drift_carries: Vec::new(),
            last_market_orders: None,
            counters: CounterBuckets::default(),
        };
        let mut state = EconomyState {
            places: vec![place],
        };
        let floors = BTreeMap::from([(("floor".to_owned(), 0), 3)]);
        run_monthly_demography_with_floors(
            &rules,
            &mut state,
            &BTreeMap::new(),
            &BTreeMap::new(),
            None,
            &floors,
        )
        .unwrap();
        let place = &state.places[0];
        assert_eq!(place.pops[0].workers + place.pops[0].dependents, 3);
        assert_eq!(place.pops[0].counters.current.deaths, 4);
        assert_eq!(place.counters.current.deaths, 4);
    }

    #[test]
    fn monthly_death_pins_a_1_62_rate_on_a_one_person_pool() {
        let mut rules = tiny_rules();
        rules.pop_types[0].births_pm_year = 0;
        // Three 1000‰ death factors make 19,440‰/year equal 1.62 deaths this month.
        rules.pop_types[0].worker_deaths_pm_year = 19_440;
        let pop = PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 1,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "one-person-pool".to_owned(),
                pops: vec![pop],
                buildings: Vec::new(),
                prices_milli: vec![1000],
                prepared_m2: 0,
                development_pm: 1000,
                price_drift_carries: Vec::new(),
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        };

        run_monthly_demography(&rules, &mut state, &BTreeMap::new(), &BTreeMap::new(), None)
            .unwrap();

        let pop = &state.places[0].pops[0];
        assert_eq!(
            pop.counters.current.deaths, 1,
            "the one-person pool is capped"
        );
        assert_eq!(pop.workers, 0);
        assert_eq!(pop.consumption.worker_death_carry, 7_440_000_000_000);
    }

    /// A one-place state for the character accounting tests: one peasant pop of `workers` and `dependents`.
    fn accounting_state(workers: u64, dependents: u64) -> EconomyState {
        let mut state = EconomyState {
            places: vec![reserve_test_place(reserve_test_pop(workers), Vec::new())],
        };
        state.places[0].pops[0].dependents = dependents;
        state.places[0].prices_milli = vec![1000];
        state
    }

    fn accounting_rules(births_pm_year: u16, worker_deaths_pm_year: u16) -> Rules {
        let mut rules = tiny_rules();
        rules.pop_types[0].births_pm_year = births_pm_year;
        rules.pop_types[0].worker_deaths_pm_year = worker_deaths_pm_year;
        rules.pop_types[0].dependent_deaths_pm_year = 0;
        rules
    }

    #[test]
    fn monthly_fuel_deaths_keep_their_own_carry_and_do_not_kill_workers() {
        let mut rules = accounting_rules(0, 0);
        rules.fuel_deaths_pm_year = 100;
        rules.pop_types[0].dependent_pm = 1000;
        let mut state = accounting_state(0, 1200);
        for (deaths, carry) in [(10, 0), (9, 11_000_000), (10, 9_100_000)] {
            let pop = &mut state.places[0].pops[0];
            let before = pop.dependents;
            pop.consumption.fuel_need_month = 1000;
            pop.consumption.fuel_burned_month = 0;
            run_monthly_demography(&rules, &mut state, &BTreeMap::new(), &BTreeMap::new(), None)
                .unwrap();
            let pop = &state.places[0].pops[0];
            assert_eq!(before - pop.dependents, deaths);
            assert_eq!(pop.workers, 0);
            assert_eq!(pop.consumption.fuel_death_carry, carry);
            assert_eq!(
                (
                    pop.consumption.fuel_need_month,
                    pop.consumption.fuel_burned_month
                ),
                (0, 0)
            );
        }
    }

    #[test]
    fn character_births_are_among_the_monthly_births_not_added_to_them() {
        // Four 1000‰ birth factors make 5000‰/year on 12 workers exactly 5 births this month.
        for (pending, applied, counted) in [(0, 5, 5), (2, 3, 5), (5, 0, 5), (7, 0, 7)] {
            let rules = accounting_rules(5000, 0);
            let mut state = accounting_state(12, 0);
            state.places[0].pops[0].pending_births = pending;
            run_monthly_demography(&rules, &mut state, &BTreeMap::new(), &BTreeMap::new(), None)
                .unwrap();
            let pop = &state.places[0].pops[0];
            assert_eq!(pop.workers + pop.dependents, 12 + applied, "{pending}");
            assert_eq!(pop.counters.current.births, counted, "{pending}");
            assert_eq!(state.places[0].counters.current.births, counted);
            assert_eq!((pop.pending_births, pop.pending_deaths), (0, 0));
        }
        // Nothing from the economy and one character birth: the month has that one.
        let rules = accounting_rules(0, 0);
        let mut state = accounting_state(12, 0);
        state.places[0].pops[0].pending_births = 1;
        run_monthly_demography(&rules, &mut state, &BTreeMap::new(), &BTreeMap::new(), None)
            .unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!(pop.workers + pop.dependents, 12);
        assert_eq!(pop.counters.current.births, 1);
    }

    #[test]
    fn character_deaths_are_among_the_monthly_deaths_and_the_floor_clips_the_rest() {
        // Three 1000‰ death factors make 12,000‰/year on 2 workers exactly 2 requested deaths this month.
        let rules = accounting_rules(0, 12_000);
        let floors = BTreeMap::from([(("owner_reserve".to_owned(), 0), 1)]);
        let run = |pending: u64, floors: &BTreeMap<(String, usize), u64>| {
            let mut state = accounting_state(2, 0);
            state.places[0].pops[0].pending_deaths = pending;
            run_monthly_demography_with_floors(
                &rules,
                &mut state,
                &BTreeMap::new(),
                &BTreeMap::new(),
                None,
                floors,
            )
            .unwrap();
            let pop = state.places[0].pops[0].clone();
            (
                pop.workers + pop.dependents,
                pop.counters.current.deaths,
                pop.pending_deaths,
            )
        };
        // 2 requested, 1 pending, 1 removable head: 2 - 1 = 1, clipped to 1, so 1 is realised; the month counts 1 + 1.
        assert_eq!(run(1, &floors), (1, 2, 0));
        // Nothing pending and the same floor: the floor clips 2 to 1.
        assert_eq!(run(0, &floors), (1, 1, 0));
        // 3 pending exceed the 2 requested: none is realised and the month counts the 3.
        assert_eq!(run(3, &BTreeMap::new()), (2, 3, 0));
        assert_eq!(run(0, &BTreeMap::new()), (0, 2, 0));
    }

    #[test]
    fn pending_deaths_come_off_dependents_first() {
        assert_eq!(net_of_pending(3, 2, 0), (3, 2));
        assert_eq!(net_of_pending(3, 2, 1), (3, 1));
        assert_eq!(net_of_pending(3, 2, 2), (3, 0));
        assert_eq!(net_of_pending(3, 2, 4), (1, 0));
        assert_eq!(net_of_pending(3, 2, 9), (0, 0));
    }

    #[test]
    fn the_pending_counts_are_saved_and_a_failing_step_leaves_them() {
        let mut state = accounting_state(12, 3);
        state.places[0].pops[0].pending_births = 4;
        state.places[0].pops[0].pending_deaths = 2;
        let bytes = state.to_bytes().unwrap();
        let loaded = EconomyState::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.places[0].pops[0].pending_births, 4);
        assert_eq!(loaded.places[0].pops[0].pending_deaths, 2);
        // A step that fails (a curve is missing) returns before it commits, so the counts stay for the next step.
        let mut rules = accounting_rules(0, 0);
        rules.curves.remove("deaths_hunger");
        let mut failing = state.clone();
        assert!(
            run_daily_economy(
                &rules,
                &mut failing,
                Date {
                    year: 800,
                    month: 1,
                    day: 1,
                    hour: 12
                }
            )
            .is_err()
        );
        assert_eq!(failing, state);
    }

    /// A place with `workers` workers of one pop employed in the given `(owners, workers)` of each building.
    fn employed_place(workers: u64, jobs: &[(u64, u64)]) -> EconomyState {
        let buildings = jobs
            .iter()
            .map(|(owners, paid)| {
                let mut building = reserve_test_building(*owners, 0);
                building.employment[0].workers = *paid;
                building
            })
            .collect::<Vec<_>>();
        let mut pop = reserve_test_pop(workers);
        pop.employment = buildings.iter().map(|b| b.employment[0].clone()).collect();
        EconomyState {
            places: vec![reserve_test_place(pop, buildings)],
        }
    }

    fn jobs(state: &EconomyState) -> Vec<(u64, u64)> {
        state.places[0]
            .buildings
            .iter()
            .map(|b| (b.employment[0].owners, b.employment[0].workers))
            .collect()
    }

    #[test]
    fn a_dying_character_removes_the_head_its_age_names() {
        let mut state = accounting_state(5, 2);
        // A minor takes a dependent while there is one; an adult takes a worker.
        state
            .remove_character_head("owner_reserve", 0, true)
            .unwrap();
        assert_eq!(
            (
                state.places[0].pops[0].workers,
                state.places[0].pops[0].dependents
            ),
            (5, 1)
        );
        state
            .remove_character_head("owner_reserve", 0, false)
            .unwrap();
        assert_eq!(
            (
                state.places[0].pops[0].workers,
                state.places[0].pops[0].dependents
            ),
            (4, 1)
        );
        assert_eq!(state.places[0].pops[0].pending_deaths, 2);
        // A minor in a pop with one worker and no dependents takes the worker; an adult in a pop of dependents takes one.
        let mut one = accounting_state(1, 0);
        one.remove_character_head("owner_reserve", 0, true).unwrap();
        assert_eq!(
            (
                one.places[0].pops[0].workers,
                one.places[0].pops[0].dependents
            ),
            (0, 0)
        );
        let mut children = accounting_state(0, 3);
        children
            .remove_character_head("owner_reserve", 0, false)
            .unwrap();
        assert_eq!(
            (
                children.places[0].pops[0].workers,
                children.places[0].pops[0].dependents
            ),
            (0, 2)
        );
        // No head: refused, nothing changed.
        let mut empty = accounting_state(0, 0);
        assert!(
            empty
                .remove_character_head("owner_reserve", 0, false)
                .is_err()
        );
        assert_eq!(empty, accounting_state(0, 0));
        assert!(empty.remove_character_head("nowhere", 0, false).is_err());
        assert!(
            empty
                .remove_character_head("owner_reserve", 7, false)
                .is_err()
        );
        assert_eq!(empty, accounting_state(0, 0));
    }

    #[test]
    fn an_unemployed_worker_is_removed_before_a_job_is_released() {
        // Five workers, three employed: two are unemployed and no position changes.
        let mut state = employed_place(5, &[(0, 3)]);
        state
            .remove_character_head("owner_reserve", 0, false)
            .unwrap();
        assert_eq!(state.places[0].pops[0].workers, 4);
        assert_eq!(jobs(&state), [(0, 3)]);
        state
            .remove_character_head("owner_reserve", 0, false)
            .unwrap();
        assert_eq!(jobs(&state), [(0, 3)]);
        // All three are now employed: the next death releases a position at once.
        state
            .remove_character_head("owner_reserve", 0, false)
            .unwrap();
        assert_eq!(state.places[0].pops[0].workers, 2);
        assert_eq!(jobs(&state), [(0, 2)]);
        assert_eq!(state.places[0].pops[0].employment[0].workers, 2);
        assert_eq!(state.places[0].pops[0].pending_deaths, 3);
    }

    #[test]
    fn a_removed_head_is_put_back_exactly_and_an_empty_undo_changes_nothing() {
        // Unemployed, employed (a paid position released) and owner removals, each undone to the exact state.
        for (workers, jobs) in [(5, vec![(0, 3)]), (3, vec![(0, 3)]), (2, vec![(2, 0)])] {
            let mut state = employed_place(workers, &jobs);
            let before = state.clone();
            let undo = state
                .remove_character_head("owner_reserve", 0, false)
                .unwrap();
            assert_ne!(state, before);
            state.restore_character_head(&undo);
            assert_eq!(state, before);
        }
        let mut dependent = accounting_state(5, 2);
        let before = dependent.clone();
        let undo = dependent
            .remove_character_head("owner_reserve", 0, true)
            .unwrap();
        dependent.restore_character_head(&undo);
        assert_eq!(dependent, before);
        dependent.restore_character_head(&HeadUndo::default());
        assert_eq!(dependent, before);
    }

    #[test]
    fn the_released_position_is_in_the_building_employing_the_most_ties_by_order() {
        let mut state = employed_place(6, &[(0, 1), (0, 3), (0, 2)]);
        state
            .remove_character_head("owner_reserve", 0, false)
            .unwrap();
        assert_eq!(jobs(&state), [(0, 1), (0, 2), (0, 2)]);
        // Two buildings now employ 2: the earlier one gives the position.
        state
            .remove_character_head("owner_reserve", 0, false)
            .unwrap();
        assert_eq!(jobs(&state), [(0, 1), (0, 1), (0, 2)]);
        assert_eq!(
            state.places[0].pops[0]
                .employment
                .iter()
                .map(|e| e.workers)
                .collect::<Vec<_>>(),
            [1, 1, 2]
        );
        // Owners give a position only when no worker is paid.
        let mut owners = employed_place(3, &[(2, 0), (1, 0)]);
        owners
            .remove_character_head("owner_reserve", 0, false)
            .unwrap();
        assert_eq!(jobs(&owners), [(1, 0), (1, 0)]);
    }

    /// Two places of one pop each: `from` (`workers` workers, `dependents` dependents, `jobs` as in [`employed_place`]) and `to`
    /// (`to_workers` workers and one dependent), with the genetics of each seeded with a different mean.
    fn marriage_state(
        workers: u64,
        dependents: u64,
        jobs: &[(u64, u64)],
        to_workers: u64,
    ) -> EconomyState {
        let mut state = employed_place(workers, jobs);
        state.places[0].pops[0].dependents = dependents;
        state.places[0].id = "from".to_owned();
        let mut destination = reserve_test_pop(to_workers);
        destination.dependents = 1;
        let mut place = reserve_test_place(destination, Vec::new());
        place.id = "to".to_owned();
        state.places.push(place);
        for (index, mean) in [(0, 8000), (1, 2000)] {
            let genetics = &mut state.places[index].pops[0].genetics;
            genetics.mean[0] = mean;
            genetics.paternal.insert("p".to_owned(), 1000);
            genetics.maternal.insert("m".to_owned(), 1000);
        }
        state
    }

    fn heads_of(state: &EconomyState, place: usize) -> (u64, u64) {
        (
            state.places[place].pops[0].workers,
            state.places[place].pops[0].dependents,
        )
    }

    #[test]
    fn a_married_head_moves_by_the_cohort_rule_without_counting_a_death() {
        // An adult takes a worker, a minor a dependent: each arrives in the same cohort.
        let mut state = marriage_state(5, 2, &[], 3);
        state
            .move_character_head(("from", 0), ("to", 0), false)
            .unwrap();
        assert_eq!((heads_of(&state, 0), heads_of(&state, 1)), ((4, 2), (4, 1)));
        state
            .move_character_head(("from", 0), ("to", 0), true)
            .unwrap();
        assert_eq!((heads_of(&state, 0), heads_of(&state, 1)), ((4, 1), (4, 2)));
        for place in 0..2 {
            let pop = &state.places[place].pops[0];
            assert_eq!((pop.pending_deaths, pop.pending_births), (0, 0));
        }
        // A minor in a pop with one worker and no dependents takes the worker; an adult in a pop of dependents takes one.
        let mut lone = marriage_state(1, 0, &[], 3);
        lone.move_character_head(("from", 0), ("to", 0), true)
            .unwrap();
        assert_eq!((heads_of(&lone, 0), heads_of(&lone, 1)), ((0, 0), (4, 1)));
        let mut children = marriage_state(0, 3, &[], 3);
        children
            .move_character_head(("from", 0), ("to", 0), false)
            .unwrap();
        assert_eq!(
            (heads_of(&children, 0), heads_of(&children, 1)),
            ((0, 2), (3, 2))
        );
    }

    #[test]
    fn a_moved_worker_arrives_unemployed_and_an_employed_only_source_releases_one_position() {
        // Five workers, three employed: an unemployed one goes, no position changes, and the destination's employment is untouched.
        let mut state = marriage_state(5, 0, &[(0, 3)], 3);
        let before = state.places[1].pops[0].employment.clone();
        state
            .move_character_head(("from", 0), ("to", 0), false)
            .unwrap();
        assert_eq!(jobs(&state), [(0, 3)]);
        assert_eq!(state.places[1].pops[0].employment, before);
        assert_eq!(heads_of(&state, 1).0, 4);
        // Three workers, all employed: the next move releases one position in the source and the arrival is unemployed.
        let mut employed = marriage_state(3, 0, &[(0, 3)], 3);
        employed
            .move_character_head(("from", 0), ("to", 0), false)
            .unwrap();
        assert_eq!(jobs(&employed), [(0, 2)]);
        assert_eq!(employed.places[0].pops[0].employment[0].workers, 2);
        assert_eq!(heads_of(&employed, 0).0, 2);
        assert_eq!(heads_of(&employed, 1).0, 4);
        assert!(employed.places[1].pops[0].employment.is_empty());
    }

    #[test]
    fn the_destination_genetics_blend_with_one_head_of_the_sources() {
        let mut state = marriage_state(5, 2, &[], 3);
        let source = state.places[0].pops[0].genetics.clone();
        let mut expected = state.places[1].pops[0].genetics.clone();
        // The destination had 3 workers and 1 dependent.
        expected.blend(4, &source, 1).unwrap();
        assert_ne!(expected, state.places[1].pops[0].genetics);
        state
            .move_character_head(("from", 0), ("to", 0), false)
            .unwrap();
        assert_eq!(state.places[1].pops[0].genetics, expected);
        // The source's own state does not change, and no stock moves.
        assert_eq!(state.places[0].pops[0].genetics, source);
        assert_eq!(state.places[0].pops[0].stock, vec![0]);
        assert_eq!(state.places[1].pops[0].stock, vec![0]);
    }

    #[test]
    fn a_moved_head_is_put_back_exactly_and_a_refused_move_changes_nothing() {
        for (workers, dependents, jobs, minor) in [
            (5, 2, vec![(0, 3)], false),
            (3, 0, vec![(0, 3)], false),
            (2, 0, vec![(2, 0)], false),
            (5, 2, vec![], true),
            (1, 0, vec![], true),
        ] {
            let mut state = marriage_state(workers, dependents, &jobs, 3);
            let before = state.clone();
            let undo = state
                .move_character_head(("from", 0), ("to", 0), minor)
                .unwrap();
            assert_ne!(state, before);
            state.restore_character_move(&undo);
            assert_eq!(state, before, "{workers} {dependents} {jobs:?} {minor}");
        }
        let mut state = marriage_state(5, 2, &[], 3);
        let before = state.clone();
        state.restore_character_move(&MoveUndo::default());
        assert_eq!(state, before);
        // Refused: one pop on both sides, a missing place or slot, a source with no head. Nothing changes.
        for (from, to) in [
            (("from", 0), ("from", 0)),
            (("nowhere", 0), ("to", 0)),
            (("from", 0), ("nowhere", 0)),
            (("from", 7), ("to", 0)),
            (("from", 0), ("to", 7)),
        ] {
            assert!(state.move_character_head(from, to, false).is_err());
            assert_eq!(state, before);
        }
        let mut empty = marriage_state(0, 0, &[], 3);
        let before = empty.clone();
        assert!(
            empty
                .move_character_head(("from", 0), ("to", 0), false)
                .is_err()
        );
        assert_eq!(empty, before);
        // A destination whose head count would overflow refuses the move.
        let mut full = marriage_state(5, 2, &[], 3);
        full.places[1].pops[0].workers = u64::MAX;
        let before = full.clone();
        assert!(
            full.move_character_head(("from", 0), ("to", 0), false)
                .is_err()
        );
        assert_eq!(full, before);
        // The total is the limit, not each cohort: a destination whose workers and dependents together already fill a u64 refuses
        // an arrival in either cohort, and a destination one short of that takes it.
        for minor in [false, true] {
            let mut split = marriage_state(5, 2, &[], 3);
            split.places[1].pops[0].workers = u64::MAX - 1;
            split.places[1].pops[0].dependents = 1;
            let before = split.clone();
            assert!(
                split
                    .move_character_head(("from", 0), ("to", 0), minor)
                    .is_err()
            );
            assert_eq!(split, before);
            split.places[1].pops[0].workers = u64::MAX - 2;
            split
                .move_character_head(("from", 0), ("to", 0), minor)
                .unwrap();
            let arrived = &split.places[1].pops[0];
            assert_eq!(arrived.workers + arrived.dependents, u64::MAX);
        }
    }

    #[test]
    fn character_births_add_dependents_and_count_pending_or_change_nothing() {
        let mut state = accounting_state(5, 2);
        state.add_character_births("owner_reserve", 0, 3).unwrap();
        let pop = &state.places[0].pops[0];
        assert_eq!((pop.workers, pop.dependents, pop.pending_births), (5, 5, 3));
        state.places[0].pops[0].pending_births = u64::MAX;
        let before = state.clone();
        assert!(state.add_character_births("owner_reserve", 0, 1).is_err());
        assert_eq!(state, before);
        assert!(state.add_character_births("nowhere", 0, 1).is_err());
        assert!(state.add_character_births("owner_reserve", 3, 1).is_err());
    }

    #[test]
    fn the_death_multipliers_are_the_monthly_steps_own() {
        let mut rules = tiny_rules();
        // Curves that are not flat, so each multiplier differs from the others and from 1000.
        rules
            .curves
            .insert("deaths_hunger".to_owned(), vec![[0, 5000], [1000, 1000]]);
        rules
            .curves
            .insert("deaths_comfort".to_owned(), vec![[0, 1000], [1000, 600]]);
        let mut state = accounting_state(10, 0);
        state.places[0].pops[0].consumption.necessities_pm = 400;
        state.places[0].pops[0].consumption.niceties_pm = 300;
        state.places[0].pops[0].consumption.luxuries_pm = 200;
        let place = &state.places[0];
        let multipliers = death_multipliers(&rules, place, 0).unwrap();
        let crowding = housing_fill(&rules, place, 0).unwrap().pops[0]
            .crowding_pm
            .unwrap_or(1000);
        let at = |name: &str, x: u64| curve_value(&rules.curves[name], x).unwrap();
        assert_eq!(
            multipliers,
            [[
                at("deaths_hunger", 400),
                at("deaths_comfort", 300 + 200 / 2),
                at("deaths_crowding", crowding)
            ]]
        );
    }

    #[test]
    fn monthly_death_uses_before_phase_crowding_when_the_pop_dies_off() {
        let mut rules = tiny_rules();
        rules.pop_types[0].births_pm_year = 0;
        rules.pop_types[0].worker_deaths_pm_year = 12_000;
        rules
            .curves
            .insert("deaths_crowding".to_owned(), vec![[0, 0], [2000, 1000]]);
        let pop = PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers: 1,
            dependents: 0,
            employment: Vec::new(),
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let mut state = EconomyState {
            places: vec![PlaceState {
                id: "emptying-pop".to_owned(),
                pops: vec![pop],
                buildings: Vec::new(),
                prices_milli: vec![1000],
                prepared_m2: 0,
                development_pm: 1000,
                price_drift_carries: Vec::new(),
                last_market_orders: None,
                counters: CounterBuckets::default(),
            }],
        };
        assert_eq!(
            housing_fill(&rules, &state.places[0], 0)
                .unwrap()
                .crowding_pm,
            2000
        );

        run_monthly_demography(&rules, &mut state, &BTreeMap::new(), &BTreeMap::new(), None)
            .unwrap();

        let place = &state.places[0];
        assert_eq!(place.pops[0].workers, 0);
        assert_eq!(place.pops[0].counters.current.deaths, 1);
        assert_eq!(place.pops[0].consumption.worker_death_carry, 0);
        let after_fill = housing_fill(&rules, place, 0).unwrap();
        assert_eq!(after_fill.pops[0].crowding_pm, None);
        assert_eq!(after_fill.crowding_pm, 0);
    }

    #[test]
    fn labourer_movement_transfers_dependents_goods_accounts_and_averages_atomically() {
        let (rules, mut state) = daily_accounting_fixture();
        let mut source = state.places.remove(0);
        source.id = "source".to_owned();
        let source_pop = &mut source.pops[0];
        source_pop.workers = 100;
        source_pop.dependents = 40;
        source_pop.stock[0] = 100;
        source_pop.consumption.pantry_hunger_centi[0] = 140;
        source_pop.consumption.necessities_centi = 1000;
        source_pop.consumption.necessities_pm = 800;
        source_pop.literacy_pm = 600;
        source_pop.employment.fill(Employment::default());
        source.buildings[0].stock[0] = 70;
        source.buildings[0]
            .storehouse_accounts
            .as_mut()
            .unwrap()
            .pops[0][0] = 70;

        let mut destination = source.clone();
        destination.id = "destination".to_owned();
        destination.pops.clear();
        for building in &mut destination.buildings {
            building.employment.clear();
            if let Some(accounts) = &mut building.storehouse_accounts {
                accounts.pops.clear();
            }
        }
        let mut places = vec![source, destination];
        let (moved, destination_pop) =
            move_recruits(&rules, &mut places, 0, 0, 1, 1, "labourer", 10).unwrap();
        assert_eq!(moved, 14);
        let source = &places[0];
        let destination = &places[1];
        let source_pop = &source.pops[0];
        let moved_pop = &destination.pops[destination_pop];
        assert_eq!((source_pop.workers, source_pop.dependents), (90, 36));
        assert_eq!(
            (
                moved_pop.kind.as_str(),
                moved_pop.workers,
                moved_pop.dependents
            ),
            ("labourer", 10, 4)
        );
        assert_eq!(source_pop.stock[0], 90);
        assert_eq!(
            moved_pop.stock[0], 14,
            "the destination pop cap accepts only four account units"
        );
        let source_account = source.buildings[0]
            .storehouse_accounts
            .as_ref()
            .unwrap()
            .pops[0][0];
        assert_eq!(source_account, 66);
        assert_eq!(source.buildings[0].stock[0], 66);
        assert_eq!(
            source_pop.stock[0] + source.buildings[0].stock[0] + moved_pop.stock[0],
            170
        );
        assert_eq!(source_pop.consumption.pantry_hunger_centi[0], 126);
        assert_eq!(moved_pop.consumption.pantry_hunger_centi[0], 14);
        assert_eq!(moved_pop.consumption.necessities_centi, 100);
        assert_eq!(moved_pop.literacy_pm, 600);
    }

    #[test]
    fn movement_gives_a_new_pop_the_movers_genetics_and_blends_into_an_existing_one() {
        let (rules, mut state) = daily_accounting_fixture();
        let mut source = state.places.remove(0);
        source.id = "source".to_owned();
        source.pops[0].workers = 100;
        source.pops[0].dependents = 40;
        let genes = |height: i64, shares: &[(&str, u32)], carrier: u32| {
            let paternal: BTreeMap<String, u32> = shares
                .iter()
                .map(|(name, share)| ((*name).to_owned(), *share))
                .collect();
            PopGenetics {
                mean: [height; genetics::ATTRIBUTE_COUNT],
                sd: [6000; genetics::ATTRIBUTE_COUNT],
                maternal: BTreeMap::from([("H".to_owned(), 1000)]),
                paternal,
                carrier: BTreeMap::from([("albino".to_owned(), carrier)]),
                founder: BTreeMap::new(),
            }
        };
        let first = genes(170_000, &[("R1b", 950), ("other", 50)], 10_000_000);
        source.pops[0].genetics = first.clone();
        source.pops[0].employment.fill(Employment::default());
        let mut destination = source.clone();
        destination.id = "destination".to_owned();
        destination.pops.clear();
        for building in &mut destination.buildings {
            building.employment.clear();
            if let Some(accounts) = &mut building.storehouse_accounts {
                accounts.pops.clear();
            }
        }
        let mut places = vec![source, destination];
        let (_, index) = move_recruits(&rules, &mut places, 0, 0, 1, 1, "labourer", 10).unwrap();
        assert_eq!(
            places[1].pops[index].genetics, first,
            "a new pop takes the movers' state exactly"
        );
        assert_eq!(places[0].pops[0].genetics, first, "the source is unchanged");

        // A different state moves into the now existing pop: both sides are 14 people, so the blend is an equal mix.
        let second = genes(
            180_000,
            &[("R1b", 750), ("I1", 200), ("other", 50)],
            30_000_000,
        );
        places[0].pops[0].genetics = second.clone();
        let heads = |pop: &PopState| pop.workers + pop.dependents;
        let before = heads(&places[1].pops[index]);
        let (moved, again) =
            move_recruits(&rules, &mut places, 0, 0, 1, 1, "labourer", 10).unwrap();
        assert_eq!((again, moved, before), (index, 14, 14));
        let blended = places[1].pops[index].genetics.clone();
        assert_eq!(blended.mean[0], 175_000);
        assert_eq!(blended.carrier["albino"], 20_000_000);
        assert_eq!(
            blended.paternal,
            BTreeMap::from([
                ("I1".to_owned(), 100),
                ("R1b".to_owned(), 850),
                ("other".to_owned(), 50)
            ])
        );
        assert_eq!(blended.maternal, BTreeMap::from([("H".to_owned(), 1000)]));
        assert_eq!(
            places[0].pops[0].genetics, second,
            "the source is unchanged"
        );

        // The state is saved with the pop.
        let saved = EconomyState { places }.to_bytes().unwrap();
        let loaded = EconomyState::from_bytes(&saved).unwrap();
        assert_eq!(loaded.places[1].pops[index].genetics, blended);
    }

    #[test]
    fn monk_recruitment_preserves_source_dependent_share_and_joins_as_workers() {
        let (rules, mut state) = daily_accounting_fixture();
        let mut source = state.places.remove(0);
        source.id = "source".to_owned();
        source.pops[0].workers = 100;
        source.pops[0].dependents = 40;
        source.pops[0].employment.fill(Employment::default());
        let mut destination = source.clone();
        destination.id = "destination".to_owned();
        destination.pops.clear();
        for building in &mut destination.buildings {
            building.employment.clear();
            if let Some(accounts) = &mut building.storehouse_accounts {
                accounts.pops.clear();
            }
        }
        let mut places = vec![source, destination];
        let (moved, destination_pop) =
            move_recruits(&rules, &mut places, 0, 0, 1, 1, "monk", 10).unwrap();
        assert_eq!(moved, 10);
        assert_eq!(
            (places[0].pops[0].workers, places[0].pops[0].dependents),
            (93, 37)
        );
        assert_eq!(
            (
                places[1].pops[destination_pop].kind.as_str(),
                places[1].pops[destination_pop].workers,
                places[1].pops[destination_pop].dependents
            ),
            ("monk", 10, 0),
        );
    }

    #[test]
    fn dependent_drift_keeps_a_small_share_gap_over_many_months() {
        let mut dependents = 3;
        let mut carry = 0;
        for _ in 0..499 {
            dependents = drift_dependents(dependents, 7, 400, &mut carry).unwrap();
            assert_eq!(dependents, 3);
        }
        dependents = drift_dependents(dependents, 7, 400, &mut carry).unwrap();
        assert_eq!(dependents, 2, "0.2 person of excess accumulates to one");
        assert_eq!(carry, 0);
        for _ in 0..124 {
            dependents = drift_dependents(dependents, 7, 400, &mut carry).unwrap();
            assert_eq!(dependents, 2);
        }
        dependents = drift_dependents(dependents, 7, 400, &mut carry).unwrap();
        assert_eq!(dependents, 3, "the reversed 0.8 gap accumulates to one");
    }

    fn holdings_movement_fixture() -> (Rules, PlaceState) {
        let mut rules = tiny_rules();
        rules.leave_pm = 1000;
        rules.hire_pm = 100;
        rules.buildings.push(BuildingType {
            id: "holdings".to_owned(),
            church: false,
            herds: BTreeMap::new(),
            workers_per_level: 10,
            land_per_level: 1,
            owner_workers: BTreeMap::from([("monk".to_owned(), 5), ("labourer".to_owned(), 10)]),
            penalty_pm: 0,
            overhead_pm: 0,
            recruit_range_m: Some(100_000),
            market_capacity_ml_per_level: 0,
            methods: Vec::new(),
            owner_kind: None,
        });
        let pop = |kind: &str, workers: u64, job: Employment| PopState {
            kind: kind.to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers,
            dependents: 0,
            employment: vec![job],
            stock: vec![0],
            consumption: PopConsumption {
                pantry_hunger_centi: vec![0],
                ticking_carries: vec![0],
                ..PopConsumption::default()
            },
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let building = BuildingState {
            kind: "holdings".to_owned(),
            levels: 1,
            method_levels: Vec::new(),
            level_records: Vec::new(),
            herds: Vec::new(),
            employment: vec![
                Employment::default(),
                Employment {
                    owners: 2,
                    workers: 2,
                },
                Employment {
                    owners: 0,
                    workers: 3,
                },
            ],
            stock: vec![0],
            storehouse_accounts: None,
            production_carries: Vec::new(),
            method_suitability: Vec::new(),
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: Vec::new(),
            pay_carries: Vec::new(),
            fee_carries: Vec::new(),
            wage_history: vec![100; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let place = PlaceState {
            id: "holdings-fixture".to_owned(),
            pops: vec![
                pop("peasant", 30, Employment::default()),
                pop(
                    "monk",
                    4,
                    Employment {
                        owners: 2,
                        workers: 2,
                    },
                ),
                pop(
                    "labourer",
                    3,
                    Employment {
                        owners: 0,
                        workers: 3,
                    },
                ),
            ],
            buildings: vec![building],
            prices_milli: vec![1000],
            prepared_m2: 0,
            development_pm: 1000,
            price_drift_carries: Vec::new(),
            last_market_orders: None,
            counters: CounterBuckets::default(),
        };
        (rules, place)
    }

    fn remote_recruitment_result(range_m: u32, distance_m: u32, position_kind: &str) -> u64 {
        let (mut rules, mut source) = holdings_movement_fixture();
        rules.leave_pm = 1000;
        rules
            .buildings
            .iter_mut()
            .find(|building| building.id == "holdings")
            .unwrap()
            .recruit_range_m = Some(range_m);
        source.id = "source".to_owned();
        let source_kind = if position_kind == "monk" {
            "monk"
        } else {
            "peasant"
        };
        let source_pop = source
            .pops
            .iter_mut()
            .find(|pop| pop.kind == source_kind)
            .unwrap();
        source_pop.employment[0] = Employment::default();
        source_pop.workers = 100;

        let mut destination = source.clone();
        destination.id = "destination".to_owned();
        destination.pops.clear();
        destination.buildings[0].employment.clear();
        let nearby = BTreeMap::from([(
            "destination".to_owned(),
            vec![NearbyPlace {
                id: "source".to_owned(),
                distance_m,
            }],
        )]);
        let allowance = match position_kind {
            "monk" => [10, 0],
            "labourer" => [0, 10],
            _ => unreachable!("test uses a supported recruitment kind"),
        };
        let mut places = vec![destination, source];
        let movement =
            move_open_positions(&rules, &mut places, &nearby, &[vec![allowance], Vec::new()])
                .unwrap();
        movement[0].iter().sum()
    }

    #[test]
    fn farm_default_range_is_ground_distance_and_inclusive() {
        assert!(remote_recruitment_result(30_000, 29_959, "labourer") > 0);
        assert_eq!(remote_recruitment_result(30_000, 30_475, "labourer"), 0);
    }

    #[test]
    fn monastery_recruits_both_position_kinds_through_100_ground_km_only() {
        assert!(remote_recruitment_result(100_000, 99_692, "monk") > 0);
        assert!(remote_recruitment_result(100_000, 99_692, "labourer") > 0);
        assert_eq!(remote_recruitment_result(100_000, 100_208, "monk"), 0);
        assert_eq!(remote_recruitment_result(100_000, 100_208, "labourer"), 0);
    }

    #[test]
    fn movement_recruits_from_the_nearer_of_two_in_range_sources_first() {
        let (mut rules, mut near) = holdings_movement_fixture();
        rules.leave_pm = 1000;
        let mut destination = near.clone();
        destination.id = "destination".to_owned();
        destination.pops.clear();
        destination.buildings[0].employment.clear();

        near.id = "near".to_owned();
        let near_pop = near
            .pops
            .iter_mut()
            .find(|pop| pop.kind == "peasant")
            .unwrap();
        near_pop.workers = 30;
        near_pop.employment[0] = Employment::default();

        let mut far = near.clone();
        far.id = "far".to_owned();
        let mut places = vec![destination, near, far];
        let nearby = BTreeMap::from([(
            "destination".to_owned(),
            vec![
                NearbyPlace {
                    id: "near".to_owned(),
                    distance_m: 74_898, // d=145 at row 15777
                },
                NearbyPlace {
                    id: "far".to_owned(),
                    distance_m: 99_692, // d=193 at row 15777
                },
            ],
        )]);

        let moved = move_open_positions(
            &rules,
            &mut places,
            &nearby,
            &[vec![[10, 0]], vec![], vec![]],
        )
        .unwrap();

        assert_eq!(
            places[1].pops[0].workers, 25,
            "the nearer source fills first"
        );
        assert_eq!(
            places[2].pops[0].workers, 30,
            "the farther source is untouched"
        );
        assert_eq!(
            places[0]
                .pops
                .iter()
                .find(|pop| pop.kind == "monk")
                .unwrap()
                .workers,
            5
        );
        assert_eq!(moved[0].iter().sum::<u64>(), 5);
    }

    #[test]
    fn movement_counts_filled_positions_by_the_original_pop_index() {
        let (rules, place) = holdings_movement_fixture();
        let mut places = vec![place];
        let moved =
            move_open_positions(&rules, &mut places, &BTreeMap::new(), &[vec![[5, 5]]]).unwrap();
        let place = &places[0];
        let monk_workers = place
            .pops
            .iter()
            .find(|pop| pop.kind == "monk")
            .unwrap()
            .employment[0]
            .workers;
        let labourer_workers = place
            .pops
            .iter()
            .find(|pop| pop.kind == "labourer")
            .unwrap()
            .employment[0]
            .workers;
        assert_eq!(monk_workers, 5, "three of five monk positions were open");
        assert_eq!(
            labourer_workers, 5,
            "two of five labourer positions were open"
        );
        assert_eq!(
            moved[0].iter().sum::<u64>(),
            5,
            "movement counts workers and dependent shares"
        );
    }

    #[test]
    fn one_source_leave_budget_is_shared_across_two_destinations() {
        let (mut rules, fixture) = holdings_movement_fixture();
        rules.leave_pm = 500;
        let empty_destination = |id: &str| {
            let mut place = fixture.clone();
            place.id = id.to_owned();
            place.pops.clear();
            place.buildings[0].employment.clear();
            place
        };
        let source = PlaceState {
            id: "source".to_owned(),
            pops: vec![PopState {
                kind: "peasant".to_owned(),
                culture: "connachta".to_owned(),
                religion: "christianity".to_owned(),
                literacy_pm: 0,
                workers: 10,
                dependents: 0,
                employment: Vec::new(),
                stock: vec![0],
                consumption: PopConsumption {
                    pantry_hunger_centi: vec![0],
                    ticking_carries: vec![0],
                    ..PopConsumption::default()
                },
                counters: CounterBuckets::default(),
                short_on_rent: false,
                housed_last: false,
                genetics: PopGenetics::default(),
                pending_births: 0,
                pending_deaths: 0,
            }],
            buildings: Vec::new(),
            prices_milli: vec![1000],
            prepared_m2: 0,
            development_pm: 1000,
            price_drift_carries: Vec::new(),
            last_market_orders: None,
            counters: CounterBuckets::default(),
        };
        let mut places = vec![
            empty_destination("first"),
            empty_destination("second"),
            source,
        ];
        let nearby = BTreeMap::from([
            (
                "first".to_owned(),
                vec![NearbyPlace {
                    id: "source".to_owned(),
                    distance_m: 1000,
                }],
            ),
            (
                "second".to_owned(),
                vec![NearbyPlace {
                    id: "source".to_owned(),
                    distance_m: 1000,
                }],
            ),
        ]);
        let moved = move_open_positions(
            &rules,
            &mut places,
            &nearby,
            &[vec![[10, 10]], vec![[10, 10]], vec![]],
        )
        .unwrap();
        assert_eq!(places[2].pops[0].workers, 5);
        assert_eq!(moved[0].iter().sum::<u64>(), 5);
        assert_eq!(moved[1].iter().sum::<u64>(), 0);
    }

    #[test]
    fn monthly_worker_hiring_and_movement_share_the_position_rate_cap() {
        let (rules, mut place) = holdings_movement_fixture();
        let allowance = fill_monthly_hiring(&rules, &mut place).unwrap();
        let before = place
            .pops
            .iter()
            .map(|pop| pop.employment[0].workers)
            .collect::<Vec<_>>();
        let mut places = vec![place];
        move_open_positions(&rules, &mut places, &BTreeMap::new(), &[allowance]).unwrap();
        let place = &places[0];
        for (kind, index) in [("monk", 0), ("labourer", 1)] {
            let after = place
                .pops
                .iter()
                .filter(|pop| pop.kind == kind)
                .map(|pop| pop.employment[0].workers)
                .sum::<u64>();
            assert!(
                after - before[index + 1] <= 1,
                "{kind} intake must stay at max(1, floor(open × 100 / 1000))"
            );
        }
    }

    #[test]
    fn holdings_owner_positions_share_total_coverage_and_only_monks_own_them() {
        let (rules, mut place) = holdings_movement_fixture();
        fill_monthly_hiring(&rules, &mut place).unwrap();
        let monk = place.pops.iter().find(|pop| pop.kind == "monk").unwrap();
        let labourer = place
            .pops
            .iter()
            .find(|pop| pop.kind == "labourer")
            .unwrap();
        assert_eq!(monk.employment[0].owners, 2);
        assert_eq!(labourer.employment[0].owners, 0);
    }

    #[test]
    fn new_holdings_owner_comes_from_monks_when_a_labourer_is_idle() {
        let (rules, mut place) = holdings_movement_fixture();
        let monk = place
            .pops
            .iter_mut()
            .find(|pop| pop.kind == "monk")
            .unwrap();
        monk.workers = 13;
        monk.employment[0].workers = 10;
        place.buildings[0].employment[1].workers = 10;
        let labourer_index = place
            .pops
            .iter()
            .position(|pop| pop.kind == "labourer")
            .unwrap();
        place.pops[labourer_index].workers = 4;
        // The pre-fix per-kind rule would fill a labourer-owner position from this idle labourer.
        assert_eq!(
            unemployed_workers(&place, labourer_index).unwrap(),
            1,
            "the fixture has an idle labourer before hiring"
        );

        fill_monthly_hiring(&rules, &mut place).unwrap();

        let monk = place.pops.iter().find(|pop| pop.kind == "monk").unwrap();
        let labourer = place
            .pops
            .iter()
            .find(|pop| pop.kind == "labourer")
            .unwrap();
        assert_eq!(monk.employment[0].owners, 3);
        assert_eq!(labourer.employment[0].owners, 0);
    }

    #[test]
    fn wage_comparison_excludes_equal_sources_and_orders_below_posted_by_wage_per_head() {
        let pop = |workers, job| PopState {
            kind: "peasant".to_owned(),
            culture: "connachta".to_owned(),
            religion: "christianity".to_owned(),
            literacy_pm: 0,
            workers,
            dependents: 0,
            employment: vec![job],
            stock: vec![0],
            consumption: PopConsumption::default(),
            counters: CounterBuckets::default(),
            short_on_rent: false,
            housed_last: false,
            genetics: PopGenetics::default(),
            pending_births: 0,
            pending_deaths: 0,
        };
        let building = BuildingState {
            kind: "farm".to_owned(),
            levels: 1,
            method_levels: vec![1],
            level_records: vec![LevelRecord {
                number: 0,
                method: 0,
                ground: Some("pasture".to_owned()),
                paydays: MATURE_PAYDAYS,
                unpaid_work: BTreeMap::new(),
                last_switch_year: None,
            }],
            herds: Vec::new(),
            employment: vec![Employment::default(); 3],
            stock: vec![0],
            storehouse_accounts: None,
            production_carries: vec![vec![0]],
            method_suitability: vec![vec![1000]],
            herd_birth_carries: Vec::new(),
            herd_death_carries: Vec::new(),
            herd_product_carries: Vec::new(),
            herd_grazing_sums: Vec::new(),
            wage_carries: vec![0],
            pay_carries: vec![0],
            fee_carries: Vec::new(),
            wage_history: vec![100; 12],
            paid_history: vec![0; WAGE_HISTORY_MONTHS],
            paid_years: vec![0; WAGE_HISTORY_MONTHS],
        };
        let place = PlaceState {
            id: "wage-fixture".to_owned(),
            pops: vec![
                pop(
                    10,
                    Employment {
                        owners: 0,
                        workers: 10,
                    },
                ),
                pop(
                    10,
                    Employment {
                        owners: 0,
                        workers: 9,
                    },
                ),
                pop(
                    10,
                    Employment {
                        owners: 2,
                        workers: 4,
                    },
                ),
            ],
            buildings: vec![building],
            prices_milli: vec![1000],
            prepared_m2: 0,
            development_pm: 1000,
            price_drift_carries: Vec::new(),
            last_market_orders: None,
            counters: CounterBuckets::default(),
        };
        assert_eq!(pop_wage_per_head(&place, 0).unwrap(), 100);
        assert_eq!(pop_wage_per_head(&place, 1).unwrap(), 90);
        assert_eq!(pop_wage_per_head(&place, 2).unwrap(), 40);
        assert!(!source_can_fill(&place.pops[0], "labourer", 100, 100));
        assert!(source_can_fill(&place.pops[1], "labourer", 100, 90));
        let order = source_order(&place, "labourer", 100, &[10, 10, 10]).unwrap();
        assert_eq!(
            order,
            [2, 1],
            "owners and the unemployed lower the average; poorest goes first"
        );
    }

    #[test]
    fn moved_pantry_hunger_is_not_reformed_as_a_second_stock_unit() {
        let (rules, state) = daily_accounting_fixture();
        let mut destination = state.places[0].pops[0].clone();
        destination.stock[0] = 0;
        destination.consumption.pantry_hunger_centi[0] = 90;
        assert!(receive_pantry_share(&rules, &mut destination, 0, 90).unwrap());
        assert_eq!(
            destination.stock[0], 0,
            "already-opened food cannot be credited again"
        );
        assert_eq!(destination.consumption.pantry_hunger_centi[0], 180);
    }

    #[test]
    fn character_floor_clips_recruitment_before_job_and_stock_accounting() {
        let (rules, mut place) = holdings_movement_fixture();
        place.pops.truncate(1);
        place.buildings[0].employment.truncate(1);
        place.pops[0].workers = 2;
        place.pops[0].dependents = 2;
        place.pops[0].stock[0] = 4;
        let mut places = vec![place];
        let floors = BTreeMap::from([(("holdings-fixture".to_owned(), 0), 2)]);
        let moved = move_open_positions_with_floors(
            &rules,
            &mut places,
            &BTreeMap::new(),
            &[vec![[0, 2]]],
            &floors,
        )
        .unwrap();
        assert_eq!(
            (places[0].pops[0].workers, places[0].pops[0].dependents),
            (1, 1)
        );
        let recruit = places[0]
            .pops
            .iter()
            .find(|p| p.kind == "labourer")
            .unwrap();
        assert_eq!(
            (recruit.workers, recruit.dependents, recruit.stock[0]),
            (1, 1, 2)
        );
        assert_eq!(recruit.employment[0].workers, 1);
        assert_eq!(moved[0].iter().sum::<u64>(), 2);
    }

    #[test]
    fn monk_recruitment_clips_head_count_before_employment() {
        let (rules, mut place) = holdings_movement_fixture();
        place.pops.truncate(1);
        place.buildings[0].employment.truncate(1);
        place.pops[0].workers = 2;
        place.pops[0].dependents = 2;
        place.pops[0].stock[0] = 4;
        let floors = BTreeMap::from([((place.id.clone(), 0), 3)]);
        let mut places = vec![place];
        let moved = move_open_positions_with_floors(
            &rules,
            &mut places,
            &BTreeMap::new(),
            &[vec![[4, 0]]],
            &floors,
        )
        .unwrap();
        assert_eq!(places[0].pops[0].workers + places[0].pops[0].dependents, 3);
        let monk = places[0].pops.iter().find(|p| p.kind == "monk").unwrap();
        assert_eq!(
            (
                monk.workers,
                monk.dependents,
                monk.stock[0],
                monk.employment[0].workers
            ),
            (1, 0, 1, 1)
        );
        assert_eq!(moved[0].iter().sum::<u64>(), 1);
    }

    #[test]
    fn neighbour_recruitment_respects_the_source_floor() {
        let (rules, mut source) = holdings_movement_fixture();
        source.id = "source".into();
        source.pops.truncate(1);
        source.buildings[0].employment.truncate(1);
        source.pops[0].workers = 2;
        source.pops[0].dependents = 2;
        source.pops[0].stock[0] = 4;
        let mut destination = source.clone();
        destination.id = "destination".into();
        destination.pops.clear();
        destination.buildings[0].employment.clear();
        let nearby = BTreeMap::from([(
            "destination".into(),
            vec![NearbyPlace {
                id: "source".into(),
                distance_m: 1,
            }],
        )]);
        let floors = BTreeMap::from([(("source".to_owned(), 0), 2)]);
        let mut places = vec![destination, source];
        let movement = move_open_positions_with_floors(
            &rules,
            &mut places,
            &nearby,
            &[vec![[0, 2]], Vec::new()],
            &floors,
        )
        .unwrap();
        assert_eq!(places[1].pops[0].workers + places[1].pops[0].dependents, 2);
        assert_eq!(places[0].pops[0].workers, 1);
        assert_eq!(places[0].pops[0].dependents, 1);
        assert_eq!(places[0].pops[0].stock[0], 2);
        assert_eq!(movement[0][0], 2);
    }

    #[test]
    fn new_pop_slots_do_not_change_existing_indices() {
        let (_, mut place) = holdings_movement_fixture();
        let before = place.pops.clone();
        let index = ensure_pop(&mut place, "new-kind", "new-culture", "new-religion");
        assert_eq!(index, before.len());
        assert_eq!(&place.pops[..before.len()], &before);
    }

    #[test]
    fn refused_cross_place_recruitment_rolls_back_both_places() {
        let (rules, source) = holdings_movement_fixture();
        let mut destination = source.clone();
        destination.id = "bad-destination".to_owned();
        destination.pops.clear();
        destination.buildings[0].employment.clear();
        destination.prices_milli.clear();
        let mut places = vec![source, destination];
        let before = places.clone();
        assert!(move_recruits(&rules, &mut places, 0, 0, 1, 0, "labourer", 1).is_err());
        assert_eq!(
            places, before,
            "a refusal cannot partially commit either place"
        );
    }

    // ---------------------------------------------------------------- CK-406: levels as records, paydays and wide carries

    pub(super) fn level_rules() -> Rules {
        let mut rules = shipped_rules();
        flatten_pasture(&mut rules);
        rules
    }

    const SEPTEMBER_DAY: Date = Date {
        year: 768,
        month: 9,
        day: 1,
        hour: 12,
    };

    pub(super) fn level(
        number: u32,
        method: u32,
        ground: Option<&str>,
        paydays: u8,
    ) -> LevelRecord {
        LevelRecord {
            number,
            method,
            ground: ground.map(str::to_owned),
            paydays,
            unpaid_work: BTreeMap::new(),
            last_switch_year: None,
        }
    }

    /// The fixture's farm with exactly these records: its counts, level total, sums and employment follow them.
    pub(super) fn farm_with(rules: &Rules, state: &mut EconomyState, records: Vec<LevelRecord>) {
        let farm = &mut state.places[0].buildings[0];
        let definition = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        farm.levels = u32::try_from(records.len()).unwrap();
        farm.method_levels = vec![0; definition.methods.len()];
        for record in &records {
            farm.method_levels[record.method as usize] += 1;
        }
        farm.level_records = records;
        for record in &mut farm.level_records {
            record.unpaid_work = seasonal_accounts(
                rules,
                definition,
                &definition.methods[record.method as usize],
            );
        }
        (farm.method_suitability, farm.herd_grazing_sums) =
            derive_level_sums(rules, definition, &farm.level_records).unwrap();
        farm.employment[0].workers =
            u64::from(farm.levels) * u64::from(definition.workers_per_level);
    }

    /// The farm's oats numerator for one payday on these records (its quality is the reference, so no factor remains).
    fn oats_numerator(
        rules: &Rules,
        records: Vec<LevelRecord>,
        date: Date,
    ) -> (u128, EconomyState) {
        let mut state = shipped_production_fixture(rules);
        farm_with(rules, &mut state, records);
        let oats = good_at(rules, "oats");
        let before = state.clone();
        let ledger = run_monthly_production(rules, &mut state, date).unwrap();
        let positions = u128::from(state.places[0].buildings[0].levels) * 10;
        let carry = |s: &EconomyState| s.places[0].buildings[0].production_carries[0][oats];
        let denominator = positions * 12_000_000_000;
        let numerator =
            u128::from(ledger.produced[oats]) * denominator + carry(&state) - carry(&before);
        (numerator, state)
    }

    #[test]
    fn a_building_derives_its_sums_from_levels_on_mixed_ground() {
        let rules = level_rules();
        let farm = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        let index = |id: &str| {
            u32::try_from(farm.methods.iter().position(|m| m.id == id).unwrap()).unwrap()
        };
        let (field, sea, wood, turf) = (
            index("mixed_farming"),
            index("coastal_subsistence"),
            index("wood_focus"),
            index("turf_cutting"),
        );
        // Field levels on pasture, on forest ground and on bog ground; a wood level on forest; a turf level on bog; one at sea.
        let records = vec![
            level(0, field, Some("pasture"), 13),
            level(1, field, Some("forest"), 13),
            level(2, field, Some("bog"), 13),
            level(3, wood, Some("forest"), 13),
            level(4, turf, Some("bog"), 13),
            level(5, sea, None, 13),
        ];
        let (suitability, grazing) = derive_level_sums(&rules, farm, &records).unwrap();
        let crops = |terrain: &str| u64::from(rules.suitability[terrain]["crops"]);
        let oats = good_at(&rules, "oats");
        // The field's oats are the crops of each level's own ground, added up.
        assert_eq!(
            suitability[field as usize][oats],
            1000 + crops("forest") + crops("bog")
        );
        assert_eq!(
            suitability[wood as usize][good_at(&rules, "hardwood")],
            u64::from(rules.suitability["forest"]["hardwood"])
        );
        assert_eq!(
            suitability[turf as usize][good_at(&rules, "peat")],
            u64::from(rules.suitability["bog"]["peat"])
        );
        assert_eq!(suitability[sea as usize][good_at(&rules, "fish")], 1000);
        // Every level that stands on ground, the wood and the turf level among them, adds its ground's grazing; the sea level none.
        let species = |animal: &str| {
            rules
                .herd_species
                .iter()
                .position(|s| s.animal == animal)
                .unwrap()
        };
        for (animal, expected) in [
            ("cattle", 1000 + 300 + 400 + 300 + 400),
            ("pigs", 400 + 1000 + 200 + 1000 + 200),
        ] {
            assert_eq!(grazing[species(animal)], expected, "{animal}");
        }
        let without_sea = derive_level_sums(&rules, farm, &records[..5]).unwrap().1;
        assert_eq!(grazing, without_sea, "a sea level adds no grazing");
    }

    #[test]
    fn a_started_building_keeps_a_record_for_each_level_in_order_and_all_are_mature() {
        let rules = shipped_rules();
        let land = LandCounts {
            land_cells: BTreeMap::from([
                ("pasture".to_owned(), 400),
                ("forest".to_owned(), 400),
                ("bog".to_owned(), 400),
            ]),
            fertility_pm: BTreeMap::from([
                ("pasture".to_owned(), 400_000),
                ("forest".to_owned(), 400_000),
                ("bog".to_owned(), 400_000),
            ]),
            fertile_cells: 1200,
            fertile_cells_by_terrain: BTreeMap::new(),
            sea: 400,
        };
        let data = ScenarioEconomy {
            horses_always: Vec::new(),
            defaults: BTreeMap::from([(
                "ringfort".to_owned(),
                BTreeMap::from([(
                    "1".to_owned(),
                    StartDefaults {
                        peasants: 100,
                        monks: 0,
                        labourers: 0,
                        farm_levels: 12,
                        holdings_levels: 0,
                        storehouse_levels: 0,
                        pop_stock: BTreeMap::from([("oats".to_owned(), 3)]),
                        building_stock: BTreeMap::from([("oats".to_owned(), 2)]),
                    },
                )]),
            )]),
            places: BTreeMap::from([(
                "fort".to_owned(),
                StartOverride {
                    culture: Some("ulaid".to_owned()),
                    religion: Some("christianity".to_owned()),
                    monk_religion: Some("christianity".to_owned()),
                    ..StartOverride::default()
                },
            )]),
            nobles: BTreeMap::new(),
        };
        let (state, _) = starting_state(
            &rules,
            &data,
            &[StartLocation {
                id: "fort".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land,
            }],
            9,
        )
        .unwrap();
        let farm = state.places[0]
            .buildings
            .iter()
            .find(|b| b.kind == "farm")
            .unwrap();
        let definition = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        assert_eq!(farm.level_records.len(), farm.levels as usize);
        assert!(farm.levels > 0);
        for (number, record) in farm.level_records.iter().enumerate() {
            assert_eq!(record.number as usize, number);
            assert_eq!(record.paydays, MATURE_PAYDAYS);
            let method = &definition.methods[record.method as usize];
            let fits = match method.resource.as_str() {
                "usable_land" => record
                    .ground
                    .as_deref()
                    .is_some_and(|g| rules.terrain_order.iter().any(|t| t == g)),
                "sea" => record.ground.is_none(),
                other => record.ground.as_deref() == Some(other),
            };
            assert!(fits, "{record:?}");
        }
        // The counts are the records' counts, and the sums are the records' own.
        let mut counts = vec![0; definition.methods.len()];
        for record in &farm.level_records {
            counts[record.method as usize] += 1;
        }
        assert_eq!(counts, farm.method_levels);
        let (suitability, grazing) =
            derive_level_sums(&rules, definition, &farm.level_records).unwrap();
        assert_eq!(suitability, farm.method_suitability);
        assert_eq!(grazing, farm.herd_grazing_sums);
        validate_level_records(&rules, &BTreeMap::new(), &state).unwrap();
    }

    #[test]
    fn a_level_with_two_paydays_makes_fourteen_twenty_fourths_of_two_mature_levels() {
        let rules = level_rules();
        let mature = |n, p| level(n, 0, Some("pasture"), p);
        let (both, _) = oats_numerator(&rules, vec![mature(0, 13), mature(1, 13)], SEPTEMBER_DAY);
        let (young, state) =
            oats_numerator(&rules, vec![mature(0, 13), mature(1, 2)], SEPTEMBER_DAY);
        assert!(both > 0);
        assert_eq!(young * 24, both * 14, "to the unit and the carry");
        let building = &state.places[0].buildings[0];
        assert_eq!(
            building
                .level_records
                .iter()
                .map(|r| r.paydays)
                .collect::<Vec<_>>(),
            [13, 3]
        );
    }

    #[test]
    fn each_levels_suitability_is_weighted_by_its_own_share_not_averaged() {
        let rules = level_rules();
        let (base, _) = oats_numerator(
            &rules,
            vec![
                level(0, 0, Some("pasture"), 13),
                level(1, 0, Some("pasture"), 13),
            ],
            SEPTEMBER_DAY,
        );
        // Oats are 1,000 on pasture (the fixture's flat ground) and 200 on marsh: the weighted sums are literals.
        assert_eq!(rules.suitability["marsh"]["crops"], 200);
        let (good_then_poor, _) = oats_numerator(
            &rules,
            vec![
                level(0, 0, Some("pasture"), 13),
                level(1, 0, Some("marsh"), 2),
            ],
            SEPTEMBER_DAY,
        );
        let (poor_then_good, _) = oats_numerator(
            &rules,
            vec![
                level(0, 0, Some("marsh"), 13),
                level(1, 0, Some("pasture"), 2),
            ],
            SEPTEMBER_DAY,
        );
        // Two mature levels on pasture weigh 2 × 12 × 1,000 = 24,000.
        assert_eq!(good_then_poor * 24_000, base * 12_400);
        assert_eq!(poor_then_good * 24_000, base * 4_400);
        // The product of the two averages (600 per level, 7 twelfths per level, two levels) would be 8,400 for both.
        assert_ne!(good_then_poor, poor_then_good);
    }

    fn two_year_end_rules() -> Rules {
        let mut rules = level_rules();
        rules.seasons.insert(
            "grain".to_owned(),
            vec![500, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 500],
        );
        rules
    }

    #[test]
    fn two_paydays_in_a_row_pay_by_the_count_and_the_stored_number_follows() {
        let rules = two_year_end_rules();
        let oats = good_at(&rules, "oats");
        let run = |paydays: u8| {
            let mut state = shipped_production_fixture(&rules);
            farm_with(
                &rules,
                &mut state,
                vec![level(0, 0, Some("pasture"), paydays)],
            );
            let mut paid = Vec::new();
            for (year, month) in [(768, 12), (769, 1)] {
                let before = state.clone();
                let ledger = run_monthly_production(
                    &rules,
                    &mut state,
                    Date {
                        year,
                        month,
                        day: 1,
                        hour: 12,
                    },
                )
                .unwrap();
                let building = &state.places[0].buildings[0];
                let carry = |s: &EconomyState| s.places[0].buildings[0].production_carries[0][oats];
                let denominator = 10_u128 * 12_000_000_000;
                paid.push((
                    u128::from(ledger.produced[oats]) * denominator + carry(&state)
                        - carry(&before),
                    building.level_records[0].paydays,
                ));
            }
            paid
        };
        // A whole payday of a mature level: twelve twelfths. Everything else is counted against it.
        let whole = run(12)[0].0;
        assert!(whole > 0);
        for (paydays, twelfths, after) in [
            (2, [2, 3], [3, 4]),
            (0, [0, 1], [1, 2]),
            (12, [12, 12], [13, 13]),
            (13, [12, 12], [13, 13]),
        ] {
            let paid = run(paydays);
            assert_eq!(
                paid[0].0 * 12,
                whole * twelfths[0],
                "paydays {paydays}, December"
            );
            assert_eq!(
                paid[1].0 * 12,
                whole * twelfths[1],
                "paydays {paydays}, January"
            );
            assert_eq!([paid[0].1, paid[1].1], after, "paydays {paydays}");
        }
    }

    #[test]
    fn herd_products_do_not_depend_on_paydays() {
        let rules = two_year_end_rules();
        let run = |paydays: u8| {
            let mut state = shipped_production_fixture(&rules);
            farm_with(
                &rules,
                &mut state,
                vec![level(0, 0, Some("pasture"), paydays)],
            );
            let ledger = run_monthly_production(&rules, &mut state, SEPTEMBER_DAY).unwrap();
            let farm = state.places[0].buildings[0].clone();
            (
                ledger.herd_births,
                ledger.herd_deaths,
                ledger.herd_slaughter,
                farm.herds,
                farm.herd_product_carries,
            )
        };
        assert_eq!(run(0), run(13));
        assert_eq!(run(2), run(13));
    }

    #[test]
    fn a_carry_wider_than_64_bits_is_kept_exactly_for_a_year() {
        let mut rules = level_rules();
        rules.fertility_reference_pm = 999;
        rules.seasons.insert(
            "grain".to_owned(),
            vec![0, 0, 0, 0, 0, 0, 0, 0, 1000, 0, 0, 0],
        );
        let farm = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        farm.workers_per_level = 2000;
        farm.penalty_pm = 0;
        farm.methods[0].annual_output = BTreeMap::from([("oats".to_owned(), 3000)]);
        let oats = good_at(&rules, "oats");
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        let records: Vec<_> = (0..1000)
            .map(|n| level(n, 0, Some("pasture"), 13))
            .collect();
        farm_with(&rules, &mut state, records);
        let farm = &mut state.places[0].buildings[0];
        farm.herds.clear();
        farm.herd_birth_carries.clear();
        farm.herd_death_carries.clear();
        farm.employment[0].workers = 1;
        let quality = BTreeMap::from([("fixture".to_owned(), 550)]);
        let run = |year, month, state: &mut EconomyState| {
            let ledger = run_monthly_production_with_quality(
                &rules,
                state,
                Date {
                    year,
                    month,
                    day: 1,
                    hour: 12,
                },
                &quality,
            )
            .unwrap();
            (
                ledger.produced[oats],
                state.places[0].buildings[0].production_carries[0][oats],
            )
        };
        // The month's share is 1,000: the output is nothing, and the carry is twelve times the old one, past 2^64.
        let first = run(768, 9, &mut state);
        assert_eq!(first, (0, 19_800_000_000_000_000_000));
        assert!(first.1 > u128::from(u64::MAX));
        // The wide carry is a good record: below its denominator (positions times twelve thousand million times 999).
        validate_level_records(&rules, &quality, &state).unwrap();
        for month in 10..=12 {
            assert_eq!(
                run(768, month, &mut state),
                (0, 19_800_000_000_000_000_000),
                "{month}"
            );
        }
        for month in 1..=8 {
            assert_eq!(
                run(769, month, &mut state),
                (0, 19_800_000_000_000_000_000),
                "{month}"
            );
        }
        // A year on, at the same month: one unit, and the rest is kept whole.
        assert_eq!(run(769, 9, &mut state), (1, 15_624_000_000_000_000_000));
        // And a save keeps it exactly.
        let bytes = state.to_bytes().unwrap();
        assert_eq!(EconomyState::from_bytes(&bytes).unwrap(), state);
    }

    #[test]
    fn a_mature_payday_the_old_units_held_is_paid_in_full_though_its_scaled_numerator_passes_128_bits()
     {
        // 100 mature levels of 65,535 positions, fully staffed, in a September share of 1,000 at fertility 550 over 999: the build
        // before CK-406 paid 5,505,505,505,505,505,505 oats and kept a carry. Twelve times the numerator passes u128::MAX, but the
        // quotient and the scaled remainder fit, and they are paid and kept.
        let mut rules = level_rules();
        rules.fertility_reference_pm = 999;
        rules.seasons.insert(
            "grain".to_owned(),
            vec![0, 0, 0, 0, 0, 0, 0, 0, 1000, 0, 0, 0],
        );
        let farm = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        farm.workers_per_level = 65_535;
        farm.penalty_pm = 0;
        farm.methods[0].annual_output =
            BTreeMap::from([("oats".to_owned(), 100_000_000_000_000_000)]);
        let oats = good_at(&rules, "oats");
        flatten_pasture(&mut rules);
        let mut state = shipped_production_fixture(&rules);
        farm_with(
            &rules,
            &mut state,
            (0..100).map(|n| level(n, 0, Some("pasture"), 13)).collect(),
        );
        let farm = &mut state.places[0].buildings[0];
        farm.herds.clear();
        farm.herd_birth_carries.clear();
        farm.herd_death_carries.clear();
        let quality = BTreeMap::from([("fixture".to_owned(), 550)]);
        let ledger = run_monthly_production_with_quality(
            &rules,
            &mut state,
            Date {
                year: 768,
                month: 9,
                day: 1,
                hour: 12,
            },
            &quality,
        )
        .unwrap();
        assert_eq!(ledger.produced[oats], 5_505_505_505_505_505_505);
        assert_eq!(
            state.places[0].buildings[0].production_carries[0][oats],
            39_714_210_000_000_000_000
        );
        validate_level_records(&rules, &quality, &state).unwrap();
    }

    #[test]
    fn a_building_without_levels_has_no_carries_and_the_step_passes_it() {
        let rules = level_rules();
        let mut state = shipped_production_fixture(&rules);
        farm_with(&rules, &mut state, Vec::new());
        let farm = &mut state.places[0].buildings[0];
        farm.production_carries.clear();
        farm.method_suitability.clear();
        farm.herds.clear();
        farm.herd_birth_carries.clear();
        farm.herd_death_carries.clear();
        farm.herd_grazing_sums.clear();
        validate_level_records(&rules, &BTreeMap::new(), &state).unwrap();
        run_monthly_production(&rules, &mut state, SEPTEMBER_DAY).unwrap();
        assert!(state.places[0].buildings[0].production_carries.is_empty());
        // The same save with one carry above nothing is refused, naming the building, the method and the good.
        let farm = &mut state.places[0].buildings[0];
        farm.production_carries = vec![vec![0; rules.goods.len()]; 4];
        farm.production_carries[0][good_at(&rules, "oats")] = 1;
        let error = validate_level_records(&rules, &BTreeMap::new(), &state).unwrap_err();
        assert!(
            error.contains("fixture")
                && error.contains("farm")
                && error.contains("mixed_farming")
                && error.contains("oats"),
            "{error}"
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The complete fixture pins the score, residual fuel demand and unchanged state together."
    )]
    fn march_look_uses_the_exact_unpaid_and_slow_start_costs_without_mutating_state() {
        let mut rules = level_rules();
        rules.fertility_reference_pm = 1000;
        rules
            .seasons
            .insert("grain".into(), vec![0, 0, 0, 0, 0, 0, 0, 0, 1000, 0, 0, 0]);
        rules.seasons.insert(
            "hardwood".into(),
            vec![250, 250, 0, 0, 0, 0, 0, 0, 0, 0, 250, 250],
        );
        let def = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        def.herds.clear();
        def.methods[0].annual_output = BTreeMap::from([("oats".into(), 100)]);
        def.methods[2].annual_output = BTreeMap::from([("hardwood".into(), 600)]);
        let mut state = shipped_production_fixture(&rules);
        farm_with(&rules, &mut state, vec![level(0, 0, Some("pasture"), 13)]);
        let place = &mut state.places[0];
        place.buildings.truncate(1);
        place.buildings[0].herds.fill(0);
        place.buildings[0].level_records[0]
            .unpaid_work
            .insert("oats".into(), bigint::Big::new(50 * 144_000));
        for pop in &mut place.pops {
            pop.workers = 0;
            pop.dependents = 0;
        }
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 10), ("forest".into(), 4)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 10_000)]),
            fertile_cells: 10,
            fertile_cells_by_terrain: BTreeMap::from([("pasture".into(), 10)]),
            sea: 0,
        };
        let prices = vec![1; rules.goods.len()];
        let jobs = BTreeMap::from([("oats".into(), 1000), ("hardwood".into(), 1000)]);
        let before = state.to_bytes().unwrap();
        let first = switching::look(&rules, &state.places[0], &land, 0, 3, &prices, &jobs).unwrap();
        let second =
            switching::look(&rules, &state.places[0], &land, 0, 3, &prices, &jobs).unwrap();
        assert_eq!(first, second);
        assert_eq!(state.to_bytes().unwrap(), before);
        let item = first.moves.iter().find(|m| m.method == 2).unwrap();
        let fraction = |n| (bigint::Big::new(n), bigint::Big::new(1));
        assert_eq!(
            (
                item.staying.numerator.clone(),
                item.staying.denominator.clone()
            ),
            fraction(100)
        );
        assert_eq!(
            (
                item.switching.numerator.clone(),
                item.switching.denominator.clone()
            ),
            fraction(600)
        );
        assert_eq!(
            (
                item.unpaid.numerator.clone(),
                item.unpaid.denominator.clone()
            ),
            fraction(50)
        );
        assert_eq!(
            (
                item.slow_start.numerator.clone(),
                item.slow_start.denominator.clone()
            ),
            fraction(175)
        );
        assert_eq!(
            (
                item.surplus.magnitude.numerator.clone(),
                item.surplus.magnitude.denominator.clone()
            ),
            fraction(255)
        );
        assert!(!item.surplus.negative);
        assert_eq!(first.answer.map(|i| first.moves[i].method), Some(2));
        for (month, wood, account, slow_num, slow_den, loss_num, loss_den) in [
            (8, 400, 13_200_000, 850, 3, 95, 1),
            (10, 400, 1_200_000, 350, 1, 235, 3),
            (8, 600, 13_200_000, 425, 1, 110, 3),
        ] {
            let mut case_rules = rules.clone();
            case_rules
                .buildings
                .iter_mut()
                .find(|b| b.id == "farm")
                .unwrap()
                .methods[2]
                .annual_output
                .insert("hardwood".into(), wood);
            let mut case_state = state.clone();
            case_state.places[0].buildings[0].level_records[0]
                .unpaid_work
                .insert("oats".into(), bigint::Big::new(account));
            let result = switching::look(
                &case_rules,
                &case_state.places[0],
                &land,
                0,
                month,
                &prices,
                &jobs,
            )
            .unwrap();
            let item = result.moves.iter().find(|m| m.method == 2).unwrap();
            assert_eq!(item.slow_start.numerator.to_u128(), Some(slow_num));
            assert_eq!(item.slow_start.denominator.to_u128(), Some(slow_den));
            assert!(item.surplus.negative);
            assert_eq!(item.surplus.magnitude.numerator.to_u128(), Some(loss_num));
            assert_eq!(item.surplus.magnitude.denominator.to_u128(), Some(loss_den));
        }
        // A herd worth 500 on both sides contributes nothing to this level's margin.
        let mut common_rules = rules.clone();
        common_rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .herds
            .insert("cattle".into(), HerdCapacity { maximum: 12 });
        let mut common_state = state.clone();
        let cattle = common_rules
            .herd_species
            .iter()
            .position(|s| s.animal == "cattle")
            .unwrap();
        common_state.places[0].buildings[0].herds[cattle] = 7;
        common_state.places[0].buildings[0].level_records[0]
            .unpaid_work
            .insert("oats".into(), bigint::Big::new(20 * 144_000));
        let common_jobs = BTreeMap::from([
            ("oats".into(), 100),
            ("hardwood".into(), 150),
            ("dairy".into(), 500),
        ]);
        let common = switching::look(
            &common_rules,
            &common_state.places[0],
            &land,
            0,
            3,
            &prices,
            &common_jobs,
        )
        .unwrap();
        let item = common.moves.iter().find(|m| m.method == 2).unwrap();
        assert_eq!(
            (
                item.staying.numerator.to_u128(),
                item.staying.denominator.to_u128()
            ),
            (Some(100), Some(1))
        );
        assert_eq!(
            (
                item.switching.numerator.to_u128(),
                item.switching.denominator.to_u128()
            ),
            (Some(150), Some(1))
        );
        assert_eq!(
            (
                item.surplus.magnitude.numerator.to_u128(),
                item.surplus.magnitude.denominator.to_u128()
            ),
            (Some(10), Some(1))
        );
        assert!(!item.surplus.negative);
        let mut partial_rules = rules.clone();
        partial_rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .workers_per_level = 4;
        let mut partial_state = state.clone();
        partial_state.places[0].buildings[0].employment[0].workers = 3;
        let partial = switching::look(
            &partial_rules,
            &partial_state.places[0],
            &land,
            0,
            3,
            &prices,
            &jobs,
        )
        .unwrap();
        let wood = partial.moves.iter().find(|m| m.method == 2).unwrap();
        assert_eq!(wood.staying.numerator.to_u128(), Some(75));
        assert_eq!(wood.switching.numerator.to_u128(), Some(450));
        let mut two_methods = rules.clone();
        two_methods
            .seasons
            .insert("fish".into(), two_methods.seasons["hardwood"].clone());
        two_methods
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .methods[1]
            .annual_output = BTreeMap::from([("fish".into(), 600)]);
        let mut coastal_land = land.clone();
        coastal_land.sea = 1;
        let mut both_jobs = jobs.clone();
        both_jobs.insert("fish".into(), 1000);
        let tied_methods = switching::look(
            &two_methods,
            &state.places[0],
            &coastal_land,
            0,
            3,
            &prices,
            &both_jobs,
        )
        .unwrap();
        assert_eq!(
            tied_methods.answer.map(|i| tied_methods.moves[i].method),
            Some(1)
        );
        let mut tied_rules = rules.clone();
        tied_rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .methods[2]
            .annual_output
            .insert("hardwood".into(), 240);
        let tied =
            switching::look(&tied_rules, &state.places[0], &land, 0, 3, &prices, &jobs).unwrap();
        let wood = tied.moves.iter().find(|m| m.method == 2).unwrap();
        assert_eq!(wood.surplus.magnitude.numerator.to_u128(), Some(0));
        assert!(tied.answer.is_none());
        // The source level returns its own field room before another land method is tried.
        let mut field_rules = rules.clone();
        let field = &mut field_rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .methods[1];
        field.resource = "usable_land".into();
        field.suitability = "crops".into();
        let mut exactly_one_field = land.clone();
        exactly_one_field.land_cells = BTreeMap::from([("pasture".into(), 2)]);
        exactly_one_field.fertility_pm = BTreeMap::from([("pasture".into(), 2000)]);
        exactly_one_field.fertile_cells = 2;
        exactly_one_field.sea = 0;
        let before_land = exactly_one_field.clone();
        let result = switching::look(
            &field_rules,
            &state.places[0],
            &exactly_one_field,
            0,
            3,
            &prices,
            &jobs,
        )
        .unwrap();
        assert_eq!(
            result
                .moves
                .iter()
                .find(|m| m.method == 1)
                .unwrap()
                .ground
                .as_deref(),
            Some("pasture")
        );
        assert_eq!(exactly_one_field, before_land);
        let mut one_method = rules.clone();
        one_method
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .methods
            .truncate(1);
        let result =
            switching::look(&one_method, &state.places[0], &land, 0, 3, &prices, &jobs).unwrap();
        assert!(result.moves.is_empty() && result.answer.is_none());
        let mut no_forest = land.clone();
        no_forest.land_cells.insert("forest".into(), 0);
        let no_room =
            switching::look(&rules, &state.places[0], &no_forest, 0, 3, &prices, &jobs).unwrap();
        assert!(no_room.moves.iter().all(|m| m.method != 2));

        // One job load is counted before any heat, and unused output is capped.
        let limited_jobs = BTreeMap::from([("oats".into(), 1000), ("hardwood".into(), 1)]);
        let limited = switching::look(
            &rules,
            &state.places[0],
            &land,
            0,
            3,
            &prices,
            &limited_jobs,
        )
        .unwrap();
        let wood = limited.moves.iter().find(|m| m.method == 2).unwrap();
        assert_eq!(
            (
                wood.switching.numerator.clone(),
                wood.switching.denominator.clone()
            ),
            fraction(1)
        );
        assert_eq!(
            (
                wood.slow_start.numerator.clone(),
                wood.slow_start.denominator.clone()
            ),
            fraction(0)
        );
        assert!(limited.answer.is_none());

        let mut heat_rules = rules.clone();
        heat_rules
            .pop_types
            .iter_mut()
            .find(|p| p.id == "peasant")
            .unwrap()
            .fuel_need_milli_day = 1000;
        let mut heat_state = state.clone();
        heat_state.places[0].pops[0].workers = 1;
        let heat = switching::look(
            &heat_rules,
            &heat_state.places[0],
            &land,
            0,
            3,
            &prices,
            &limited_jobs,
        )
        .unwrap();
        let wood = heat.moves.iter().find(|m| m.method == 2).unwrap();
        assert_eq!(
            (
                wood.switching.numerator.clone(),
                wood.switching.denominator.clone()
            ),
            fraction(366)
        );
        let mut valued = state.clone();
        valued.places[0].buildings[0].level_records[0]
            .unpaid_work
            .insert("oats".into(), bigint::Big::new(30 * 144_000));
        let mut valued_prices = prices.clone();
        valued_prices[good_at(&rules, "oats")] = 1000;
        let result = switching::look(
            &rules,
            &valued.places[0],
            &land,
            0,
            3,
            &valued_prices,
            &jobs,
        )
        .unwrap();
        let item = result.moves.iter().find(|m| m.method == 2).unwrap();
        assert_eq!(item.unpaid.numerator.to_u128(), Some(30_000));
        assert_eq!(item.unpaid.denominator.to_u128(), Some(1));
        let large = u128::from(u32::MAX) + 123;
        heat_state.places[0].buildings[0].level_records[0]
            .unpaid_work
            .insert("oats".into(), bigint::Big::new(large));
        let result =
            switching::look(&rules, &heat_state.places[0], &land, 0, 3, &prices, &jobs).unwrap();
        let item = result.moves.iter().find(|m| m.method == 2).unwrap();
        assert_eq!(
            item.unpaid.numerator.mul(&bigint::Big::new(144_000)),
            item.unpaid.denominator.mul(&bigint::Big::new(large))
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "The shared fixture compares herd loss, retained animals, marginal gain and capped founders."
    )]
    fn the_last_land_level_bears_the_lost_herd_products_and_net_animal_cost() {
        let mut rules = level_rules();
        rules.fertility_reference_pm = 1000;
        let farm = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        farm.herds.retain(|animal, _| animal == "cattle");
        farm.herds.get_mut("cattle").unwrap().maximum = 12;
        farm.methods[0].annual_output = BTreeMap::from([("oats".into(), 100)]);
        farm.methods[1].annual_output = BTreeMap::from([("fish".into(), 160)]);
        let species = rules
            .herd_species
            .iter_mut()
            .find(|s| s.animal == "cattle")
            .unwrap();
        species.yields = vec![HerdYield {
            good: "beef".into(),
            units: 150,
        }];
        let mut state = shipped_production_fixture(&rules);
        farm_with(&rules, &mut state, vec![level(0, 0, Some("pasture"), 13)]);
        let place = &mut state.places[0];
        place.buildings.truncate(1);
        place.buildings[0].herds.fill(0);
        let cattle = rules
            .herd_species
            .iter()
            .position(|s| s.animal == "cattle")
            .unwrap();
        place.buildings[0].herds[cattle] = 10;
        for pop in &mut place.pops {
            pop.workers = 0;
            pop.dependents = 0;
        }
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 10)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 10_000)]),
            fertile_cells: 10,
            fertile_cells_by_terrain: BTreeMap::from([("pasture".into(), 10)]),
            sea: 1,
        };
        let mut prices = vec![1; rules.goods.len()];
        prices[good_at(&rules, "cattle")] = 240;
        let jobs = BTreeMap::from([
            ("oats".into(), 1000),
            ("fish".into(), 1000),
            ("dairy".into(), 500),
        ]);
        let result =
            switching::look(&rules, &state.places[0], &land, 0, 3, &prices, &jobs).unwrap();
        let sea = result.moves.iter().find(|m| m.method == 1).unwrap();
        assert_eq!(sea.staying.numerator.to_u128(), Some(600));
        assert_eq!(sea.staying.denominator.to_u128(), Some(1));
        assert_eq!(sea.switching.numerator.to_u128(), Some(160));
        assert_eq!(sea.herd_cost.numerator.to_u128(), Some(900));
        assert_eq!(sea.herd_cost.denominator.to_u128(), Some(1));
        assert!(sea.surplus.negative);

        farm_with(
            &rules,
            &mut state,
            vec![
                level(0, 0, Some("pasture"), 13),
                level(1, 0, Some("pasture"), 13),
            ],
        );
        state.places[0].buildings[0].employment[0].owners = 2;
        let result =
            switching::look(&rules, &state.places[0], &land, 0, 3, &prices, &jobs).unwrap();
        let sea = result
            .moves
            .iter()
            .find(|m| m.method == 1 && m.level == 0)
            .unwrap();
        assert_eq!(sea.herd_cost.numerator.to_u128(), Some(0));
        assert_eq!(sea.staying.numerator.to_u128(), Some(100));

        // A 100-to-150 move with 20 of animal loss clears a 20 margin by 10.
        let mut gain_rules = rules.clone();
        gain_rules.herd_species[cattle].products.clear();
        gain_rules.herd_species[cattle].yields.clear();
        gain_rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .methods[1]
            .annual_output
            .insert("fish".into(), 600);
        let mut gain_state = state.clone();
        farm_with(
            &gain_rules,
            &mut gain_state,
            vec![level(0, 0, Some("pasture"), 13)],
        );
        gain_state.places[0].buildings[0].herds[cattle] = 10;
        let mut gain_prices = vec![1; gain_rules.goods.len()];
        gain_prices[good_at(&gain_rules, "cattle")] = 2;
        let gain_jobs = BTreeMap::from([("oats".into(), 100), ("fish".into(), 150)]);
        let gain = switching::look(
            &gain_rules,
            &gain_state.places[0],
            &land,
            0,
            3,
            &gain_prices,
            &gain_jobs,
        )
        .unwrap();
        let sea = gain.moves.iter().find(|m| m.method == 1).unwrap();
        assert_eq!(sea.staying.numerator.to_u128(), Some(100));
        assert_eq!(sea.switching.numerator.to_u128(), Some(150));
        assert_eq!(sea.slow_start.numerator.to_u128(), Some(0));
        assert_eq!(sea.herd_cost.numerator.to_u128(), Some(20));
        assert_eq!(sea.surplus.magnitude.numerator.to_u128(), Some(10));
        assert!(!sea.surplus.negative);

        // Settlement caps two proposed founders at the one-head capacity.
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .herds
            .get_mut("cattle")
            .unwrap()
            .maximum = 1;
        farm_with(&rules, &mut state, vec![level(0, 1, None, 13)]);
        state.places[0].buildings[0].herds.fill(0);
        let result =
            switching::look(&rules, &state.places[0], &land, 0, 3, &prices, &jobs).unwrap();
        let land_move = result.moves.iter().find(|m| m.method == 0).unwrap();
        assert_eq!(land_move.herd_cost.numerator.to_u128(), Some(240));
        assert_eq!(land_move.herd_cost.denominator.to_u128(), Some(1));
        assert_eq!(land_move.switching.numerator.to_u128(), Some(172));
        assert_eq!(land_move.switching.denominator.to_u128(), Some(1));
    }

    #[test]
    fn another_young_levels_slow_first_year_is_subtracted_before_the_demand_cap() {
        let mut rules = level_rules();
        rules.fertility_reference_pm = 1000;
        rules.seasons.insert(
            "grain".into(),
            vec![83, 83, 83, 83, 84, 84, 84, 84, 83, 83, 83, 83],
        );
        let farm = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        farm.herds.clear();
        farm.methods[0].annual_output = BTreeMap::from([("oats".into(), 240)]);
        let mut state = shipped_production_fixture(&rules);
        farm_with(
            &rules,
            &mut state,
            vec![
                level(0, 0, Some("pasture"), 13),
                level(1, 0, Some("pasture"), 0),
            ],
        );
        let place = &mut state.places[0];
        place.buildings.truncate(1);
        place.buildings[0].herds.fill(0);
        place.buildings[0].employment[0].owners = 2;
        for pop in &mut place.pops {
            pop.workers = 0;
            pop.dependents = 0;
        }
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 10)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 10_000)]),
            fertile_cells: 10,
            fertile_cells_by_terrain: BTreeMap::new(),
            sea: 1,
        };
        let jobs = BTreeMap::from([("oats".into(), 120)]);
        let result = switching::look(
            &rules,
            &state.places[0],
            &land,
            0,
            12,
            &vec![1; rules.goods.len()],
            &jobs,
        )
        .unwrap();
        let entry = result
            .moves
            .iter()
            .find(|m| m.method == 1 && m.level == 0)
            .unwrap();
        assert_eq!(entry.staying.numerator.to_u128(), Some(10));
        assert_eq!(entry.staying.denominator.to_u128(), Some(1));
    }

    #[test]
    fn the_look_chooses_the_poorer_ground_then_the_lower_level_number_on_a_tie() {
        let mut rules = level_rules();
        rules.fertility_reference_pm = 1000;
        let def = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        def.herds.clear();
        def.methods[0].annual_output = BTreeMap::from([("oats".into(), 100)]);
        def.methods[3].annual_output = BTreeMap::from([("peat".into(), 600)]);
        rules
            .suitability
            .get_mut("bog")
            .unwrap()
            .insert("oats".into(), 250);
        let mut state = shipped_production_fixture(&rules);
        farm_with(
            &rules,
            &mut state,
            vec![
                level(0, 0, Some("pasture"), 13),
                level(1, 0, Some("bog"), 13),
            ],
        );
        let place = &mut state.places[0];
        place.buildings.truncate(1);
        place.buildings[0].herds.fill(0);
        place.buildings[0].employment[0].owners = 2;
        for pop in &mut place.pops {
            pop.workers = 0;
            pop.dependents = 0;
        }
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 10), ("bog".into(), 20)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 10_000), ("bog".into(), 20_000)]),
            fertile_cells: 30,
            fertile_cells_by_terrain: BTreeMap::new(),
            sea: 0,
        };
        let prices = vec![1; rules.goods.len()];
        let jobs = BTreeMap::from([("oats".into(), 1000), ("peat".into(), 1000)]);
        let result =
            switching::look(&rules, &state.places[0], &land, 0, 3, &prices, &jobs).unwrap();
        let answer = &result.moves[result.answer.unwrap()];
        assert_eq!((answer.method, answer.level), (3, 1));
        rules
            .suitability
            .get_mut("bog")
            .unwrap()
            .insert("oats".into(), 1000);
        let result =
            switching::look(&rules, &state.places[0], &land, 0, 3, &prices, &jobs).unwrap();
        let answer = &result.moves[result.answer.unwrap()];
        assert_eq!((answer.method, answer.level), (3, 0));
    }

    #[test]
    fn a_herd_product_named_by_the_method_is_counted_once() {
        let mut rules = level_rules();
        rules.fertility_reference_pm = 1000;
        let farm = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        farm.herds.retain(|name, _| name == "cattle");
        farm.methods[0].annual_output = BTreeMap::from([("oats".into(), 999)]);
        farm.methods[1].annual_output = BTreeMap::from([("fish".into(), 1000)]);
        let cattle = rules
            .herd_species
            .iter_mut()
            .find(|s| s.animal == "cattle")
            .unwrap();
        cattle.products = vec![HerdProduct {
            good: "oats".into(),
            amount_milli: Some(10_000),
            season: Some("grain".into()),
            month: None,
        }];
        cattle.yields = vec![HerdYield {
            good: "beef".into(),
            units: 1,
        }];
        let mut state = shipped_production_fixture(&rules);
        farm_with(&rules, &mut state, vec![level(0, 0, Some("pasture"), 13)]);
        let place = &mut state.places[0];
        place.buildings.truncate(1);
        place.buildings[0].herds.fill(0);
        let cattle_index = rules
            .herd_species
            .iter()
            .position(|s| s.animal == "cattle")
            .unwrap();
        place.buildings[0].herds[cattle_index] = 1;
        for pop in &mut place.pops {
            pop.workers = 0;
            pop.dependents = 0;
        }
        let land = LandCounts {
            land_cells: BTreeMap::from([("pasture".into(), 10)]),
            fertility_pm: BTreeMap::from([("pasture".into(), 10_000)]),
            fertile_cells: 10,
            fertile_cells_by_terrain: BTreeMap::new(),
            sea: 1,
        };
        let jobs = BTreeMap::from([("oats".into(), 10), ("fish".into(), 15)]);
        let result = switching::look(
            &rules,
            &state.places[0],
            &land,
            0,
            3,
            &vec![1; rules.goods.len()],
            &jobs,
        )
        .unwrap();
        let sea = result.moves.iter().find(|m| m.method == 1).unwrap();
        assert_eq!(sea.staying.numerator.to_u128(), Some(10));
        assert_eq!(sea.switching.numerator.to_u128(), Some(15));
        assert_eq!(sea.slow_start.numerator.to_u128(), Some(0));
        assert_eq!(sea.herd_cost.numerator.to_u128(), Some(0));
        assert_eq!(sea.surplus.magnitude.numerator.to_u128(), Some(3));
        assert!(!sea.surplus.negative);
        assert!(
            state.places[0].buildings[0].level_records[0]
                .unpaid_work
                .is_empty()
        );

        // A different recurring product contributes its full value to the lost herd.
        rules.herd_species[cattle_index].products[0].good = "ale".into();
        rules.herd_species[cattle_index].products[0].amount_milli = Some(100_000);
        let farm = rules.buildings.iter_mut().find(|b| b.id == "farm").unwrap();
        farm.methods[0].annual_output = BTreeMap::from([("oats".into(), 100)]);
        farm.methods[1].annual_output = BTreeMap::from([("fish".into(), 160)]);
        rules
            .seasons
            .insert("fish".into(), vec![0, 500, 500, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        state.places[0].buildings[0].level_records[0]
            .unpaid_work
            .insert("oats".into(), bigint::Big::default());
        let jobs = BTreeMap::from([
            ("oats".into(), 100),
            ("fish".into(), 160),
            ("ale".into(), 100),
        ]);
        let result = switching::look(
            &rules,
            &state.places[0],
            &land,
            0,
            3,
            &vec![1; rules.goods.len()],
            &jobs,
        )
        .unwrap();
        let sea = result.moves.iter().find(|m| m.method == 1).unwrap();
        assert_eq!(sea.staying.numerator.to_u128(), Some(200));
        assert_eq!(sea.switching.numerator.to_u128(), Some(160));
        assert_eq!(sea.slow_start.numerator.to_u128(), Some(20));
        assert!(sea.surplus.negative);
        assert_eq!(sea.surplus.magnitude.numerator.to_u128(), Some(100));
    }

    #[test]
    fn mature_crop_accounts_follow_the_calendar_across_two_years() {
        let mut account = bigint::Big::new(30 * 144_000);
        for month in (1..=12).cycle().take(24) {
            accrue_unpaid_work(
                &mut account,
                120,
                1000,
                13,
                if month == 9 { 1000 } else { 0 },
                10,
                10,
                1000,
                1,
                1,
            );
            let expected = match month {
                8 => Some(110),
                9 => Some(0),
                10 => Some(10),
                12 => Some(30),
                _ => None,
            };
            if let Some(units) = expected {
                assert_eq!(account.to_u128(), Some(units * 144_000), "month {month}");
            }
        }
    }

    #[test]
    fn opening_uses_the_first_payday_at_or_after_the_actual_start() {
        let mut rules = tiny_rules();
        rules.fertility_reference_pm = 1000;
        rules.buildings[0].methods[0]
            .annual_output
            .insert("food".into(), 120);
        let location = StartLocation {
            id: "fort".into(),
            kind: "ringfort".into(),
            size: 1,
            land: LandCounts {
                land_cells: BTreeMap::from([("pasture".into(), 8)]),
                fertility_pm: BTreeMap::from([("pasture".into(), 8000)]),
                fertile_cells: 8,
                fertile_cells_by_terrain: BTreeMap::new(),
                sea: 0,
            },
        };
        for (month, day, hour, expected) in [
            (10, 9, 0, 10),
            (10, 1, 11, 0),
            (10, 1, 12, 0),
            (10, 1, 13, 10),
            (12, 31, 23, 30),
        ] {
            let (state, _) = starting_state_at(
                &rules,
                &tiny_start_data(),
                std::slice::from_ref(&location),
                Date {
                    year: 768,
                    month,
                    day,
                    hour,
                },
            )
            .unwrap();
            let account = &state.places[0].buildings[0].level_records[0].unpaid_work["food"];
            assert_eq!(
                account.to_u128(),
                Some(expected * 144_000),
                "{month}/{day} {hour}:00"
            );
        }
    }

    #[test]
    fn opening_accounts_cover_the_twelve_months_before_the_first_payday() {
        let mut rules = level_rules();
        rules.fertility_reference_pm = 1000;
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .methods[0]
            .annual_output = BTreeMap::from([("oats".into(), 120)]);
        let mut state = shipped_production_fixture(&rules);
        farm_with(&rules, &mut state, vec![level(0, 0, Some("pasture"), 13)]);
        let place = &mut state.places[0];
        place.buildings.truncate(1);
        for (month, expected) in [(1, 30_u128), (5, 70)] {
            place.buildings[0].level_records[0]
                .unpaid_work
                .get_mut("oats")
                .unwrap()
                .clone_from(&bigint::Big::default());
            seed_unpaid_work(&rules, &place.pops, &mut place.buildings, 1000, month).unwrap();
            assert_eq!(
                place.buildings[0].level_records[0].unpaid_work["oats"].to_u128(),
                Some(expected * 144_000)
            );
        }
        rules
            .buildings
            .iter_mut()
            .find(|b| b.id == "farm")
            .unwrap()
            .methods[1]
            .annual_output = BTreeMap::from([("fish".into(), 1000)]);
        farm_with(&rules, &mut state, vec![level(0, 1, None, 13)]);
        let place = &mut state.places[0];
        for (month, expected) in [(1, 166 * 144_000_u128), (5, 55_200_000)] {
            place.buildings[0].level_records[0]
                .unpaid_work
                .get_mut("fish")
                .unwrap()
                .clone_from(&bigint::Big::default());
            seed_unpaid_work(&rules, &place.pops, &mut place.buildings, 1000, month).unwrap();
            assert_eq!(
                place.buildings[0].level_records[0].unpaid_work["fish"].to_u128(),
                Some(expected)
            );
        }
    }

    #[test]
    fn mature_fish_account_tracks_the_shipped_season_for_two_years() {
        let rules = shipped_rules();
        let mut account = bigint::Big::new(166 * 144_000);
        for month in (1..=12).cycle().take(24) {
            accrue_unpaid_work(
                &mut account,
                1000,
                1000,
                13,
                u128::from(rules.seasons["fish"][month - 1]),
                10,
                10,
                1000,
                1,
                1,
            );
            let expected = match month {
                4 => Some(55_200_000),
                9 => Some(0),
                12 => Some(166 * 144_000),
                _ => None,
            };
            if let Some(expected) = expected {
                assert_eq!(account.to_u128(), Some(expected), "month {month}");
            }
        }
    }

    #[test]
    fn unpaid_work_rounds_each_fractional_month_down() {
        let mut third = bigint::Big::default();
        accrue_unpaid_work(&mut third, 301, 1000, 13, 0, 1, 3, 1000, 1, 1);
        assert_eq!(third.to_u128(), Some(1_204_000));
        let mut account = bigint::Big::default();
        // A yearly amount of 100 + 1/7 units, from one worker in seven positions.
        accrue_unpaid_work(&mut account, 701, 1000, 13, 0, 1, 7, 1000, 1, 1);
        assert_eq!(account.to_u128(), Some(1_201_714));
    }

    #[test]
    fn rules_refuse_multiple_method_fuels_and_fuel_herd_products_and_large_margin() {
        let mut rules = shipped_rules();
        assert_eq!(rules.method_switch_margin_pm, 200);
        rules.method_switch_margin_pm = 1001;
        assert!(
            rules
                .validate("fixture")
                .unwrap_err()
                .entry
                .contains("method_switch_margin_pm")
        );
        rules.method_switch_margin_pm = 200;
        let method = &mut rules
            .buildings
            .iter_mut()
            .find(|b| b.methods.len() > 1)
            .unwrap()
            .methods[0];
        method.annual_output.insert("hardwood".into(), 1);
        method.annual_output.insert("peat".into(), 1);
        let method_id = method.id.clone();
        assert!(
            rules
                .validate("fixture")
                .unwrap_err()
                .entry
                .contains(&method_id)
        );
        let mut defaults = shipped_rules();
        defaults
            .goods
            .iter_mut()
            .find(|g| g.id == "peat")
            .unwrap()
            .supplies
            .remove("fuel");
        let method = &mut defaults
            .buildings
            .iter_mut()
            .find(|b| b.methods.len() > 1)
            .unwrap()
            .methods[0];
        method.annual_output.insert("hardwood".into(), 1);
        method.annual_output.insert("peat".into(), 1);
        let method_id = method.id.clone();
        assert!(
            defaults
                .validate("fixture")
                .unwrap_err()
                .entry
                .contains(&method_id)
        );
        let mut rules = shipped_rules();
        rules
            .goods
            .iter_mut()
            .find(|g| g.id == "peat")
            .unwrap()
            .supplies
            .remove("fuel");
        rules.herd_species[0].products[0].good = "peat".into();
        let error = rules.validate("fixture").unwrap_err();
        assert!(
            error.entry.contains("cattle") && error.entry.contains("peat"),
            "{error}"
        );
    }

    #[test]
    fn unpaid_work_accrues_at_each_months_yield_and_saturates_at_zero() {
        let mut full = bigint::Big::default();
        let mut half = bigint::Big::default();
        // October: the first level is fully staffed, the second half staffed.
        accrue_unpaid_work(&mut full, 100, 1000, 13, 0, 10, 10, 1000, 1, 1);
        accrue_unpaid_work(&mut half, 100, 1000, 13, 0, 5, 10, 1000, 1, 1);
        // November: both are half staffed; previous work retains October's yield.
        for account in [&mut full, &mut half] {
            accrue_unpaid_work(account, 100, 1000, 13, 0, 5, 10, 1000, 1, 1);
        }
        assert_eq!(full.to_u128(), Some(1_800_000));
        assert_eq!(half.to_u128(), Some(1_200_000));
        accrue_unpaid_work(&mut full, 100, 1000, 13, 1000, 5, 10, 1000, 1, 1);
        assert_eq!(full, bigint::Big::default());
        // Grain taken up on 1 December reaches its first payday on 1 January.
        let mut young = bigint::Big::default();
        accrue_unpaid_work(&mut young, 100, 1000, 1, 0, 10, 10, 1000, 1, 1);
        assert_eq!(young, bigint::Big::default());
    }

    #[test]
    fn production_carry_never_enters_a_levels_unpaid_work() {
        let rules = level_rules();
        let mut zero = shipped_production_fixture(&rules);
        farm_with(&rules, &mut zero, vec![level(0, 0, Some("pasture"), 13)]);
        let mut carry = zero.clone();
        let oats = good_at(&rules, "oats");
        carry.places[0].buildings[0].production_carries[0][oats] = 1;
        let date = Date {
            year: 768,
            month: 10,
            day: 1,
            hour: 12,
        };
        run_monthly_production(&rules, &mut zero, date).unwrap();
        run_monthly_production(&rules, &mut carry, date).unwrap();
        assert_eq!(
            zero.places[0].buildings[0].level_records[0].unpaid_work,
            carry.places[0].buildings[0].level_records[0].unpaid_work
        );
    }

    #[test]
    fn unpaid_accounts_round_trip_without_a_fixed_integer_limit() {
        let mut record = level(0, 0, Some("pasture"), 13);
        let amount = bigint::Big::new(u128::MAX).add(&bigint::Big::new(1));
        record.unpaid_work.insert("oats".into(), amount);
        let mut bytes = Vec::new();
        encode_level_records(&[record.clone()], &mut bytes).unwrap();
        let mut reader = Reader::new(&bytes);
        assert_eq!(decode_level_records(&mut reader).unwrap(), vec![record]);
        reader.finish().unwrap();
    }

    #[test]
    fn saved_unpaid_accounts_must_match_the_levels_method_and_herd() {
        let rules = level_rules();
        let mut state = shipped_production_fixture(&rules);
        farm_with(&rules, &mut state, vec![level(0, 0, Some("pasture"), 13)]);
        validate_level_records(&rules, &BTreeMap::new(), &state).unwrap();
        let good = state.places[0].buildings[0].level_records[0]
            .unpaid_work
            .keys()
            .next()
            .unwrap()
            .clone();
        state.places[0].buildings[0].level_records[0]
            .unpaid_work
            .remove(&good);
        let error = validate_level_records(&rules, &BTreeMap::new(), &state).unwrap_err();
        assert!(
            error.contains(&format!("building farm level 0 good {good}: missing")),
            "{error}"
        );
        state.places[0].buildings[0].level_records[0]
            .unpaid_work
            .insert(good, bigint::Big::default());
        state.places[0].buildings[0].level_records[0]
            .unpaid_work
            .insert("not_a_product".into(), bigint::Big::default());
        let error = validate_level_records(&rules, &BTreeMap::new(), &state).unwrap_err();
        assert!(
            error.contains("building farm level 0 good not_a_product: unexpected"),
            "{error}"
        );
    }

    #[test]
    fn each_fault_of_a_saved_level_record_is_refused_naming_the_building_and_the_level() {
        let rules = level_rules();
        let mut state = shipped_production_fixture(&rules);
        let records = vec![
            level(0, 0, Some("pasture"), 13),
            level(1, 0, Some("forest"), 13),
            level(2, 0, Some("bog"), 13),
            level(3, 2, Some("forest"), 13),
            level(4, 3, Some("bog"), 13),
            level(5, 1, None, 13),
        ];
        farm_with(&rules, &mut state, records);
        validate_level_records(&rules, &BTreeMap::new(), &state)
            .expect("land levels on forest and bog ground are good records");
        let refused = |change: &dyn Fn(&mut BuildingState), needle: &str| {
            let mut bad = state.clone();
            change(&mut bad.places[0].buildings[0]);
            let error = validate_level_records(&rules, &BTreeMap::new(), &bad).unwrap_err();
            assert!(error.contains("place fixture building farm"), "{error}");
            assert!(error.contains(needle), "{error}");
        };
        refused(
            &|b| b.level_records[0].method = 9,
            "level 0: method 9 is not one of the building's",
        );
        refused(
            &|b| b.level_records[0].ground = None,
            "level 0: ground (none) does not fit",
        );
        refused(
            &|b| b.level_records[0].ground = Some("sea".into()),
            "level 0: ground sea does not fit",
        );
        refused(
            &|b| b.level_records[3].ground = Some("bog".into()),
            "level 3: ground bog does not fit",
        );
        refused(
            &|b| b.level_records[4].ground = Some("forest".into()),
            "level 4: ground forest does not fit",
        );
        refused(
            &|b| b.level_records[5].ground = Some("pasture".into()),
            "level 5: ground pasture does not fit",
        );
        refused(
            &|b| b.level_records[2].paydays = 14,
            "level 2: 14 completed paydays",
        );
        refused(
            &|b| b.level_records[2].number = 1,
            "the number is used twice",
        );
        refused(
            &|b| b.level_records[2].number = 6,
            "level 6: the number is not below",
        );
        refused(
            &|b| {
                b.level_records.pop();
            },
            "5 level records for 6 levels",
        );
        refused(&|b| b.method_levels[0] = 4, "the level records count");
        refused(
            &|b| b.production_carries[0][0] = u128::MAX,
            "is not below its denominator",
        );
        // The values in order are the derived ones, but a value has moved from one method's row to the next one's.
        refused(
            &|b| {
                let moved = b.method_suitability[0].pop().unwrap();
                b.method_suitability[1].insert(0, moved);
            },
            "the saved suitability sums are not those of the level records",
        );
    }

    // ---------------------------------------------------------------- CK-406: two years of the real start, day by day

    /// A production carry's unit against the build before levels were records: twelve times finer from CK-406 on.
    const CARRY_UNIT: u128 = 12;

    /// The real start of one place on mixed ground, with its farm, holdings, demesne and storehouse, and its pops partly staffed.
    fn levels_witness_state(rules: &Rules) -> EconomyState {
        let data = ScenarioEconomy {
            horses_always: Vec::new(),
            defaults: BTreeMap::from([(
                "ringfort".to_owned(),
                BTreeMap::from([(
                    "1".to_owned(),
                    StartDefaults {
                        peasants: 1500,
                        monks: 24,
                        labourers: 0,
                        farm_levels: 12,
                        holdings_levels: 6,
                        storehouse_levels: 3,
                        pop_stock: BTreeMap::from([
                            ("oats".to_owned(), 30),
                            ("dairy".to_owned(), 4),
                        ]),
                        building_stock: BTreeMap::from([("oats".to_owned(), 20)]),
                    },
                )]),
            )]),
            places: BTreeMap::from([(
                "fort".to_owned(),
                StartOverride {
                    culture: Some("ulaid".to_owned()),
                    religion: Some("christianity".to_owned()),
                    monk_religion: Some("christianity".to_owned()),
                    ..StartOverride::default()
                },
            )]),
            nobles: BTreeMap::from([(
                "fort".to_owned(),
                NobleStart {
                    culture: "ulaid".to_owned(),
                    religion: "christianity".to_owned(),
                    people: 40,
                    demesne_levels: 4,
                    labourers: 40,
                },
            )]),
        };
        let cells = |terrain: &str| (terrain.to_owned(), 300_u32);
        let fertility = |terrain: &str| (terrain.to_owned(), 300_000_u64);
        let (mut state, _) = starting_state(
            rules,
            &data,
            &[StartLocation {
                id: "fort".to_owned(),
                kind: "ringfort".to_owned(),
                size: 1,
                land: LandCounts {
                    land_cells: BTreeMap::from([cells("pasture"), cells("forest"), cells("bog")]),
                    fertility_pm: BTreeMap::from([
                        fertility("pasture"),
                        fertility("forest"),
                        fertility("bog"),
                    ]),
                    fertile_cells: 900,
                    fertile_cells_by_terrain: BTreeMap::new(),
                    sea: 300,
                },
            }],
            9,
        )
        .unwrap();
        // Partly staffed: every building has half of the workers its start gave it.
        for place in &mut state.places {
            for (index, building) in place.buildings.iter_mut().enumerate() {
                for (slot, employment) in building.employment.iter_mut().enumerate() {
                    employment.workers /= 2;
                    place.pops[slot].employment[index].workers = employment.workers;
                }
            }
        }
        state
    }

    #[test]
    fn each_months_look_runs_on_the_partly_staffed_real_start() {
        let rules = shipped_rules();
        let mut state = levels_witness_state(&rules);
        let land = LandCounts {
            land_cells: BTreeMap::from([
                ("pasture".into(), 300),
                ("forest".into(), 300),
                ("bog".into(), 300),
            ]),
            fertility_pm: BTreeMap::from([
                ("pasture".into(), 300_000),
                ("forest".into(), 300_000),
                ("bog".into(), 300_000),
            ]),
            fertile_cells: 900,
            fertile_cells_by_terrain: BTreeMap::new(),
            sea: 300,
        };
        for month in 1..=12 {
            run_monthly_production(
                &rules,
                &mut state,
                Date {
                    year: 768,
                    month,
                    day: 1,
                    hour: 12,
                },
            )
            .unwrap();
            let place = &state.places[0];
            for (i, building) in place.buildings.iter().enumerate() {
                if rules
                    .buildings
                    .iter()
                    .find(|b| b.id == building.kind)
                    .unwrap()
                    .methods
                    .len()
                    > 1
                {
                    switching::look(
                        &rules,
                        place,
                        &land,
                        i,
                        month,
                        &place.prices_milli,
                        &BTreeMap::new(),
                    )
                    .unwrap();
                }
            }
        }
    }

    /// A building with none of its levels, so that a month's production is one other building's alone.
    fn clear_levels(building: &mut BuildingState) {
        building.method_levels.fill(0);
        building.level_records.clear();
    }

    /// Each day of two years, as a digest of everything the daily economy changes: prices, stores, herds, wages, staffing and
    /// the production carries in the old unit. The recording of the build before CK-406 is in `levels_daily_accepted.txt`.
    #[expect(
        clippy::too_many_lines,
        reason = "The witness keeps its wage units and its digest of every quantity in one literal pass over two years."
    )]
    fn levels_two_year_trace() -> String {
        use std::fmt::Write as _;

        let rules = shipped_rules();
        let mut state = levels_witness_state(&rules);
        let mut trace = String::new();
        let mut any_carry = false;
        for year in 768..=769 {
            for month in 1..=12 {
                for day in 1..=days_in_month(year, month).unwrap() {
                    let date = Date {
                        year,
                        month,
                        day,
                        hour: 12,
                    };
                    // The in-kind units each producing building's payday will share (the way the herd trace works them out): the
                    // month's output of that building alone, less the overhead, with the wage carries and the pay carries.
                    let mut wage_units: Vec<Vec<u64>> = Vec::new();
                    if day == 1 {
                        for index in 0..state.places[0].buildings.len() {
                            let building = &state.places[0].buildings[index];
                            let definition = rules
                                .buildings
                                .iter()
                                .find(|b| b.id == building.kind)
                                .unwrap();
                            if definition.methods.is_empty() {
                                continue;
                            }
                            let mut isolated = state.clone();
                            for (other, building) in
                                isolated.places[0].buildings.iter_mut().enumerate()
                            {
                                if other != index {
                                    clear_levels(building);
                                    building.herds.clear();
                                    building.herd_birth_carries.clear();
                                    building.herd_death_carries.clear();
                                    building.herd_grazing_sums.clear();
                                }
                            }
                            let output = run_monthly_production(&rules, &mut isolated, date)
                                .unwrap()
                                .produced;
                            let unstaffed = building.employment.iter().all(|e| e.workers == 0);
                            wage_units.push(
                                output
                                    .iter()
                                    .enumerate()
                                    .map(|(good, amount)| {
                                        if unstaffed {
                                            0
                                        } else {
                                            (amount
                                                * u64::from(1000 - definition.overhead_pm)
                                                * 1000
                                                + building.wage_carries[good])
                                                / 1_000_000
                                                + building.pay_carries[good]
                                        }
                                    })
                                    .collect(),
                            );
                        }
                    }
                    let ledger = run_daily_economy(&rules, &mut state, date).unwrap();
                    let place = &state.places[0];
                    let mut text = format!(
                        "{:?} {:?} {wage_units:?}",
                        place.prices_milli, ledger.produced
                    );
                    for building in &place.buildings {
                        let scaled: Vec<u128> = building
                            .production_carries
                            .iter()
                            .flatten()
                            .map(|carry| {
                                let carry = *carry;
                                assert_eq!(carry % CARRY_UNIT, 0);
                                carry / CARRY_UNIT
                            })
                            .collect();
                        any_carry |= scaled.iter().any(|carry| *carry > 0);
                        write!(
                            text,
                            " {} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
                            building.kind,
                            building.stock,
                            building.herds,
                            building.wage_carries,
                            building.pay_carries,
                            building.wage_history,
                            building.employment,
                            building.herd_product_carries,
                            scaled
                        )
                        .unwrap();
                    }
                    for pop in &place.pops {
                        write!(
                            text,
                            " {} {} {} {:?} {:?}",
                            pop.kind, pop.workers, pop.dependents, pop.stock, pop.employment
                        )
                        .unwrap();
                    }
                    let digest = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
                    });
                    writeln!(trace, "{year}-{month:02}-{day:02} {digest:016x}").unwrap();
                }
            }
        }
        assert!(
            any_carry,
            "the witness has production carries above nothing"
        );
        trace
    }

    #[test]
    fn the_witness_start_has_field_wood_turf_and_sea_levels_partly_staffed() {
        let rules = shipped_rules();
        let state = levels_witness_state(&rules);
        let definition = rules.buildings.iter().find(|b| b.id == "farm").unwrap();
        let place = &state.places[0];
        let farm = place.buildings.iter().find(|b| b.kind == "farm").unwrap();
        let used = |resource: &str| {
            definition
                .methods
                .iter()
                .zip(&farm.method_levels)
                .any(|(method, levels)| method.resource == resource && *levels > 0)
        };
        assert!(used("usable_land") && used("sea") && used("forest") && used("bog"));
    }

    #[test]
    fn two_years_of_the_real_start_follow_seeded_opening() {
        let trace = levels_two_year_trace();
        assert_eq!(trace, include_str!("../tests/levels_daily_accepted.txt"));
    }
}

#[cfg(test)]
mod character_floor_tests {
    #[test]
    fn mixed_recruitment_clips_workers_before_accounting() {
        assert_eq!(super::recruitable(2, 2, 2, 2), 1);
        assert_eq!(super::recruitable(2, 2, 2, 0), 0);
        assert_eq!(super::recruitable(2, 2, 2, 4), 2);
    }
    #[test]
    fn mortality_prioritises_workers() {
        assert_eq!(super::clip_deaths(4, 3, 5), (4, 1));
        assert_eq!(super::clip_deaths(4, 3, 2), (2, 0));
    }
}

fn recruitable(requested: u64, workers: u64, dependents: u64, removable: u64) -> u64 {
    if workers == 0 {
        return 0;
    }
    let mut low = 0_u64;
    let mut high = requested.min(workers);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let heads =
            u128::from(mid) + u128::from(mid) * u128::from(dependents) / u128::from(workers);
        if heads <= u128::from(removable) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}
/// The requested deaths less the deaths characters already committed: dependents first, then workers, each held at 0 (CK-364).
fn net_of_pending(workers: u64, dependents: u64, pending: u64) -> (u64, u64) {
    let from_dependents = pending.min(dependents);
    (
        workers.saturating_sub(pending - from_dependents),
        dependents - from_dependents,
    )
}
fn clip_deaths(workers: u64, dependents: u64, removable: u64) -> (u64, u64) {
    let workers = workers.min(removable);
    (workers, dependents.min(removable - workers))
}
